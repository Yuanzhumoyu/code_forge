//! 模块级编译编排 —— [`CompiledModule`] 与 [`CompileSession`] 的整模块/单函数入口。
//!
//! 分工：`compiler/` 是**单函数**管线（IR → 机器码），本文件是**宿主面**——
//! 把"一个模块怎么编译"（数据段、符号、并按 `FuncRef` 序逐个编译）唯一地实现一次。
//! JIT（`runtime/jit.rs`）与对象文件出口（`forge-object`）都从这里取产物，
//! 不再各自重抄一遍"并行/串行 + 签名表 + 符号名"的编排。
//!
//! v30 M1：产物/入口改带 [`CompileError`]（阶段 + 函数）；并行策略由
//! [`CompileSession::options`] 决定（`Parallelism::{Auto, Serial, Threads}`）。
//! v30 M4：每个产物单元携带 [`JitSignatureInfo`]（JIT 类型化调用校验用），
//! 变参事实来自模块自己的 [`ModuleSigs`]。

use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};

use crate::machine::target::TargetMachine;
use crate::pipeline::compiler::CompileSession;
use crate::pipeline::error::CompileError;
use crate::pipeline::options::Parallelism;
use crate::pipeline::signature::JitSignatureInfo;
use crate::pipeline::stage::Stage;
use crate::{CompiledFunction, IrError};
use forge_ir::ir::function::Function;
use forge_ir::ir::types::FunctionSignature;
use forge_ir::{FuncRef, ImmStr, Module, ModuleSigs};

/// 一个产物函数：机器码 + 源码签名（JIT 装载器按 `FuncRef` 序安装）。
pub struct CompiledUnit {
    /// 函数名（与 IR 一致）。
    pub name: ImmStr,
    /// 机器码与重定位。
    pub function: CompiledFunction,
    /// 源码签名（形参/返回/约定/变参）。
    pub sig: JitSignatureInfo,
}

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
    functions: Vec<CompiledUnit>,
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

    /// 按名字取产物单元（机器码 + 签名）。
    pub fn unit(&self, name: &str) -> Option<&CompiledUnit> {
        self.functions.iter().find(|u| u.name.as_str() == name)
    }

    /// 按名字取机器码。
    pub fn function(&self, name: &str) -> Option<&CompiledFunction> {
        self.unit(name).map(|u| &u.function)
    }

    /// 按名字取源码签名。
    pub fn signature(&self, name: &str) -> Option<&JitSignatureInfo> {
        self.unit(name).map(|u| &u.sig)
    }

    /// 逐单元遍历（`FuncRef` 序）。
    pub fn iter_units(&self) -> impl Iterator<Item = &CompiledUnit> {
        self.functions.iter()
    }

    /// 逐函数遍历（`FuncRef` 序）。
    pub fn iter_functions(&self) -> impl Iterator<Item = (&ImmStr, &CompiledFunction)> {
        self.functions.iter().map(|u| (&u.name, &u.function))
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
        Vec<CompiledUnit>,
        Vec<(ImmStr, ImmStr, u64)>,
        Vec<Box<[u8]>>,
    ) {
        (self.functions, self.globals, self.data_segments)
    }
}

/// 编译模块里的一个函数（串行/并行两条路径共用；会话只是"机器 + 签名表"的搬运工）。
fn compile_one<M: TargetMachine>(
    machine: &Arc<M>,
    sigs: &Arc<ModuleSigs>,
    func: &Function,
    variadic: bool,
) -> Result<CompiledUnit, CompileError> {
    let function = CompileSession::from_arc(Arc::clone(machine))
        .with_module_sigs((**sigs).clone())
        .compile(func)?;
    Ok(CompiledUnit {
        name: func.name.clone(),
        function,
        sig: JitSignatureInfo::from_function(func, variadic),
    })
}

/// 逐函数编译（串行或固定大小的 worker 池；结果按 `FuncRef` 序回收）。
///
/// `Parallelism::Auto` 保持历史行为：函数数 ≥ 8 时**每函数一个** scoped 线程；
/// `Threads(n)` 起 n 个 worker 从共享下标取活（并发度 = min(n, 函数数)）。
fn compile_functions<M: TargetMachine>(
    machine: &Arc<M>,
    sigs: &Arc<ModuleSigs>,
    module: &Module,
    parallel: Parallelism,
) -> Result<Vec<CompiledUnit>, CompileError> {
    let count = module.function_count();
    let variadic_at = |i: usize| sigs.get(i).is_some_and(|e| e.variadic);
    let workers = parallel.worker_count(count);
    if workers <= 1 {
        return (0..count)
            .map(|i| {
                compile_one(
                    machine,
                    sigs,
                    module.get_function(FuncRef::new(i as u32)),
                    variadic_at(i),
                )
            })
            .collect();
    }

    let next = AtomicUsize::new(0);
    let slots: Mutex<Vec<Option<Result<CompiledUnit, CompileError>>>> =
        Mutex::new((0..count).map(|_| None).collect());

    std::thread::scope(|s| -> Result<(), CompileError> {
        let mut handles = Vec::with_capacity(workers);
        for _ in 0..workers {
            let next = &next;
            let slots = &slots;
            handles.push(s.spawn(move || {
                loop {
                    let i = next.fetch_add(1, Ordering::Relaxed);
                    if i >= count {
                        break;
                    }
                    let r = compile_one(
                        machine,
                        sigs,
                        module.get_function(FuncRef::new(i as u32)),
                        variadic_at(i),
                    );
                    slots.lock().expect("compile slots poisoned")[i] = Some(r);
                }
            }));
        }
        for handle in handles {
            if handle.join().is_err() {
                return Err(CompileError::module(IrError::Internal(
                    "compile thread panicked".into(),
                )));
            }
        }
        Ok(())
    })?;

    slots
        .into_inner()
        .expect("compile slots poisoned")
        .into_iter()
        .enumerate()
        .map(|(i, slot)| {
            slot.unwrap_or_else(|| {
                Err(CompileError::module(IrError::Internal(format!(
                    "compile worker 未产出函数 {i}"
                ))))
            })
        })
        .collect()
}

impl<M: TargetMachine> CompileSession<M> {
    /// **整模块编译**（唯一实现）：登记全局数据段与符号 → 逐函数编译（大模块并行）
    /// → 按 `FuncRef` 序返回。
    ///
    /// 签名表由模块自己给出（[`Module::signature_table`]），**每条函数**都装同一份
    /// （变参调用点的落点规划靠它）；每个产物单元同时记下源码签名（JIT 校验用）。
    pub fn compile_module(&self, module: &Module) -> Result<CompiledModule, CompileError> {
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

        // ② 签名表与逐函数编译（并行策略由选项决定；各函数互不共享可变状态）。
        let sigs = Arc::new(module.signature_table());
        let functions =
            compile_functions(self.machine_arc(), &sigs, module, self.options().parallel())?;

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
    ) -> Result<CompiledFunction, CompileError> {
        let store = forge_ir::ir::types::TypeContext::new();
        let mut builder =
            forge_ir::ir::builder::FunctionBuilder::new(name, store, signature.clone());
        build_fn(&mut builder);
        let func: Function = builder
            .finish()
            .map_err(|e| CompileError::at(Stage::ISel, &ImmStr::from(name), e))?;
        self.compile(&func)
    }
}
