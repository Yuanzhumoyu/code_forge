//! 模块级编译编排 —— [`CompiledModule`] 与 [`CompileSession`] 的整模块/单函数入口。
//!
//! 分工：`compiler.rs` 是**单函数**管线（IR → 机器码），本文件是**宿主面**——
//! 把"一个模块怎么编译"（数据段、符号、并按 `FuncRef` 序逐个编译）唯一地实现一次。
//! JIT（`runtime/jit.rs`）与对象文件出口（`forge-object`）都从这里取产物，
//! 不再各自重抄一遍"并行/串行 + 签名表 + 符号名"的编排。

use crate::machine::target::TargetMachine;
use crate::pipeline::compiler::CompileSession;
use crate::{CompiledFunction, IrError};
use forge_ir::ir::function::Function;
use forge_ir::ir::types::FunctionSignature;
use forge_ir::{FuncRef, ImmStr, Module};

/// 一次整模块编译的产物（**纯数据**：机器码 + 符号 + 数据段，不含可执行内存）。
///
/// 函数按 `FuncRef::index()` 序（= 模块函数表序）排列，与签名表同序——跨函数
/// 重定位按名（`name`）与按引用（`@{i}`）两种符号都由这里登记。
///
/// **类型无关**（无 `M` 参数）：产物里不带目标机器实例，只带 [`isa`](Self::isa)
/// 这个名字标签——装载器拿自己的机器与它核对，不匹配就 fail-closed（x86 的码
/// 不该被 riscv 的装载器装进可执行内存）。
pub struct CompiledModule {
    isa: ImmStr,
    functions: Vec<(ImmStr, CompiledFunction)>,
    /// `(源码名, `@N` 引用键, 数据段地址)`——每个全局两条符号名指向同一段。
    globals: Vec<(ImmStr, ImmStr, u64)>,
    /// 每全局一段堆分配（地址稳定；随本结构存活）。
    data_segments: Vec<Box<[u8]>>,
}

impl CompiledModule {
    /// 目标 ISA 名（与 `IsaInfo::name()` 同一口径；装载器用它核对机器）。
    pub fn isa(&self) -> &str {
        self.isa.as_str()
    }

    /// 已编译函数数。
    pub fn function_count(&self) -> usize {
        self.functions.len()
    }

    /// 按名字取编译产物。
    pub fn function(&self, name: &str) -> Option<&CompiledFunction> {
        self.functions
            .iter()
            .find(|(n, _)| n.as_str() == name)
            .map(|(_, cf)| cf)
    }

    /// 逐函数遍历（`FuncRef` 序）。
    pub fn iter_functions(&self) -> impl Iterator<Item = (&ImmStr, &CompiledFunction)> {
        self.functions.iter().map(|(n, cf)| (n, cf))
    }

    /// 全局变量的符号三元组。
    pub fn globals(&self) -> &[(ImmStr, ImmStr, u64)] {
        &self.globals
    }

    /// 数据段（须与本结构同寿命——地址被机器码直接引用）。
    pub fn data_segments(&self) -> &[Box<[u8]>] {
        &self.data_segments
    }

    /// 拆开所有权（装载器要装进可执行内存时用）。
    pub fn into_parts(
        self,
    ) -> (
        Vec<(ImmStr, CompiledFunction)>,
        Vec<(ImmStr, ImmStr, u64)>,
        Vec<Box<[u8]>>,
    ) {
        (self.functions, self.globals, self.data_segments)
    }
}

/// 编译模块里的一个函数（串行/并行两条路径共用；会话只是"机器 + 签名表"的搬运工）。
fn compile_one<M: TargetMachine + Clone>(
    machine: &M,
    sigs: &std::sync::Arc<forge_ir::ModuleSigs>,
    func: &Function,
) -> Result<(ImmStr, CompiledFunction), IrError> {
    CompileSession::new(machine.clone())
        .with_module_sigs((**sigs).clone())
        .compile(func)
        .map(|cf| (func.name.clone(), cf))
}

impl<M: TargetMachine + Clone> CompileSession<M> {
    /// **整模块编译**（唯一实现）：登记全局数据段与符号 → 逐函数编译（大模块并行）
    /// → 按 `FuncRef` 序返回。
    ///
    /// 签名表由模块自己给出（[`Module::signature_table`]），**每条函数**都装同一份
    /// （变参调用点的落点规划靠它）。
    pub fn compile_module(&self, module: &Module) -> Result<CompiledModule, IrError> {
        // ① 全局变量：每全局一段堆分配，登记源码名与 `@N` 两种符号名。
        //
        // init 字节缺失（如 `zeroinitializer` 文本层 init=None）时按类型大小生成零段——
        // 否则数据段为空（dangling 指针），store/load 写未映射地址 → 随机 SEGV。
        let mut globals: Vec<(ImmStr, ImmStr, u64)> = Vec::new();
        let mut data_segments: Vec<Box<[u8]>> = Vec::new();
        for (gid, global) in module.iter_globals() {
            let init: Vec<u8> = match global.init.clone() {
                Some(bytes) => bytes,
                None => {
                    let size = module.types.borrow().size_bytes(global.ty) as usize;
                    vec![0u8; size]
                }
            };
            let seg: Box<[u8]> = init.into_boxed_slice();
            let addr = seg.as_ptr() as u64;
            globals.push((
                global.name.clone(),
                ImmStr::from(format!("G{}", gid.index())),
                addr,
            ));
            data_segments.push(seg);
        }

        // ② 签名表与逐函数编译（≥8 个函数走 scoped 并行：各函数互不共享可变状态）。
        let sigs = std::sync::Arc::new(module.signature_table());
        let func_count = module.function_count();
        let functions: Vec<(ImmStr, CompiledFunction)> = if func_count >= 8 {
            std::thread::scope(|s| {
                let handles: Vec<_> = (0..func_count)
                    .map(|i| {
                        let machine = self.machine().clone();
                        let sigs = sigs.clone();
                        s.spawn(move || {
                            compile_one(
                                &machine,
                                &sigs,
                                module.get_function(FuncRef::new(i as u32)),
                            )
                        })
                    })
                    .collect();
                handles
                    .into_iter()
                    .map(|h| {
                        h.join()
                            .map_err(|_| IrError::Internal("compile thread panicked".into()))?
                    })
                    .collect::<Result<Vec<_>, IrError>>()
            })?
        } else {
            (0..func_count)
                .map(|i| {
                    compile_one(
                        self.machine(),
                        &sigs,
                        module.get_function(FuncRef::new(i as u32)),
                    )
                })
                .collect::<Result<_, _>>()?
        };

        Ok(CompiledModule {
            isa: ImmStr::from(TargetMachine::isa_info(self.machine()).name()),
            functions,
            globals,
            data_segments,
        })
    }

    /// **单函数便捷入口**：用 `build_fn` 建 IR 再编译，返回编译产物。
    ///
    /// 与 [`Self::compile_module`] 走同一条 `compile` 实现；会话上装了模块签名表时
    /// 同样生效（跨函数变参调用不再因为"单函数入口"而失去规划依据）。
    pub fn add_function(
        &self,
        name: &str,
        signature: &FunctionSignature,
        build_fn: impl FnOnce(&mut forge_ir::ir::builder::FunctionBuilder),
    ) -> Result<CompiledFunction, IrError> {
        let store = forge_ir::ir::types::TypeContext::new();
        let mut builder =
            forge_ir::ir::builder::FunctionBuilder::new(name, store, signature.clone());
        build_fn(&mut builder);
        let func: Function = builder.finish()?;
        self.compile(&func)
    }
}
