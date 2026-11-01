//! JIT 编译器 — 高层编译 API。
//!
//! 提供类型安全的 JIT 编译接口，自动管理可执行内存生命周期和符号解析。
//!
//! # Example
//! ```ignore
//! use code_forge::backend::jit::JitLoader;
//! use code_forge::backend::arch::x86;
//! use code_forge::ir::*;
//!
//! let tm = x86::TargetMachine::new();
//! let mut jit = JitLoader::new(tm);
//!
//! // 编译函数
//! let sig = FunctionSignature::new(&[(TypeId::I32, "a"), (TypeId::I32, "b")], &[TypeId::I32]);
//! jit.add_function("add", &sig, |b| {
//!     let (entry, params) = b.create_block_with_params(&[(TypeId::I32, "a"), (TypeId::I32, "b")]);
//!     b.switch_to_block(entry);
//!     let sum = b.iadd(params[0], params[1]);
//!     b.ret(&[sum]);
//! })?;
//!
//! // 获取类型安全的函数指针
//! let add: extern "C" fn(i32, i32) -> i32 = jit.get("add")?;
//! assert_eq!(add(3, 4), 7);
//! ```

use std::sync::Arc;

use crate::machine::target::TargetMachine;
use crate::pipeline::compiler::CompileSession;
use crate::pipeline::error::CompileError;
use crate::pipeline::session::CompiledModule;
use crate::pipeline::signature::JitSig;
use crate::pipeline::signature::JitSignatureInfo;
use crate::{CompiledFunction, RelocKind, Relocation};
use forge_ir::ImmStr;
use forge_ir::{
    FuncSigEntry, FunctionBuilder, FunctionSignature, IrError, Module, ModuleSigs, TypeId,
};
use forge_isa_runtime::mem::{ExecutableMemory, MemError};

mod binding;
#[cfg(feature = "jit_rust_abi")]
pub mod rust_abi;
mod signature;
mod symbols;

pub use binding::{JitBindError, JitFn, JitVariadicFn, VariadicTermination};
pub use signature::{JitCallSignature, JitParam, JitReturn, JitSignature, JitValue};
use symbols::{STACK_PROBE_SYMBOL, platform_symbol_lookup, stack_probe_addr};

fn mem_to_compile_err(e: MemError) -> IrError {
    IrError::Emit(e.0)
}

/// 类型化 `get` 的签名校验（不匹配 = 使用错误，归 `Semantic`，不进缺口台账）。
fn check_typed_call(
    name: &str,
    sig: &JitSignatureInfo,
    params: Vec<TypeId>,
    ret: Option<TypeId>,
) -> Result<(), IrError> {
    if sig.variadic {
        return Err(IrError::Semantic(format!(
            "JIT 函数 `{name}` 是变参：类型化 `get` 不表达 `...`，请用 `get_unchecked`"
        )));
    }
    if !sig.conv.is_c() {
        return Err(IrError::Semantic(format!(
            "JIT 函数 `{name}` 的调用约定是 {:?}，类型化 `get` 只接受平台 C 约定（c）",
            sig.conv
        )));
    }
    if sig.params.len() != params.len()
        || !sig
            .params
            .iter()
            .zip(&params)
            .all(|(a, b)| types_compatible(*a, *b))
    {
        return Err(IrError::Semantic(format!(
            "JIT 函数 `{name}` 形参不匹配：产物 {:?}，请求 {:?}",
            sig.params, params
        )));
    }
    let want_ret: Vec<TypeId> = ret.into_iter().collect();
    let ret_ok = match (sig.returns.as_slice(), want_ret.as_slice()) {
        ([], []) => true,
        ([a], [b]) => types_compatible(*a, *b),
        _ => false,
    };
    if !ret_ok {
        return Err(IrError::Semantic(format!(
            "JIT 函数 `{name}` 返回不匹配：产物 {:?}，请求 {:?}",
            sig.returns, want_ret
        )));
    }
    Ok(())
}

/// 类型化调用里的**等价类型**：`PTR` 与 `I64` 同为一个 64 位槽。
///
/// 为什么允许：IR 里的地址常用 `i64` 表示（GEP/legalize 的既有约定），而 Rust 侧
/// 写 `*mut T`；两者在 64 位 ABI 上是同一个槽。非 64 位目标不会走 JIT 类型化面
/// （发行 JIT 后端只有 x86_64；`demo8` 等夹具没有 JIT 矩阵）。
fn types_compatible(want: TypeId, got: TypeId) -> bool {
    want == got
        || matches!(
            (want, got),
            (TypeId::PTR, TypeId::I64) | (TypeId::I64, TypeId::PTR)
        )
}

use std::collections::HashMap;

/// 符号解析器 — 将符号名映射到绝对地址。
pub type SymbolResolver<'a> = dyn Fn(&str) -> Option<u64> + 'a;

/// 装载后的一项：机器码 + 可执行内存 + 源码签名（类型化调用校验用）。
struct CompiledEntry {
    function: CompiledFunction,
    mem: ExecutableMemory,
    sig: JitSignatureInfo,
    /// v32 自包含签名（源码 + ABI + 目标事实）。
    jit_sig: JitSig,
}

/// JIT 装载器 — 把编译产物装进可执行内存，并管理符号解析与执行。
///
/// 类型参数 `M` 是目标 ISA 的 TargetMachine 类型（例如 `x86::TargetMachine`）。
///
/// **编译不在本类型里**：`add_function` / `compile_module` 是薄委托——都转调
/// [`CompileSession`]（唯一编译实现），再走 [`Self::install`] 装载。
pub struct JitLoader<M: TargetMachine> {
    /// 目标机器（`Arc` 持有：编译会话按函数共享同一份机器，不再要求 `M: Clone`）。
    machine: Arc<M>,
    /// 已编译函数（可执行内存 + 机器码 + 源码签名，按名称索引）。
    compiled: HashMap<ImmStr, CompiledEntry>,
    /// 符号表：函数名 → 入口地址（用于跨函数调用解析）。
    symbols: HashMap<ImmStr, u64>,
    /// 待应用的跨函数重定位。
    pending_relocs: Vec<(ImmStr, Relocation)>,
    /// 全局变量的数据段（每全局一个堆分配——地址稳定，Drop 时释放）。
    data_segments: Vec<Box<[u8]>>,
    /// **本 loader 逐个 `add_function` 累积的签名表**（按调用序 = `FuncRef` 序）。
    ///
    /// 单函数入口此前不装签名表 ⇒ 后面编译的函数按"被调方非变参"规划落点（变参 D6
    /// 的坏分支）。装了以后，`add_function("printf", 变参签名)` 后定义的第 0 个函数
    /// 用 `FuncRef(0)` 调用它就会按变参规划。整模块编译（`compile_module`）以模块自己的
    /// 表为准，与这份累积无关。
    sigs: ModuleSigs,
    /// 用户提供的符号解析回调（可选）— 在注册表之后、平台符号之前查询。
    #[allow(clippy::type_complexity)]
    user_resolver: Option<Box<dyn Fn(&str) -> Option<u64> + Send + Sync>>,
}

impl<M: TargetMachine> JitLoader<M> {
    /// 创建新的 JIT 装载器。
    pub fn new(machine: M) -> Self {
        let mut symbols: HashMap<ImmStr, u64> = HashMap::new();
        // **栈探测符号**（v27 M6-79）：大帧函数的序言会 `call __chkstk`（由 ISA 数据声明，
        // 见 `[machine.frame.stack_probe]`）。AOT 由 MSVC 运行时提供它；**JIT 没有链接器**
        // ⇒ 必须在这里注册，否则那条重定位解析不到、调用会跳进垃圾地址。
        symbols.insert(ImmStr::from(STACK_PROBE_SYMBOL), stack_probe_addr());
        Self {
            machine: Arc::new(machine),
            compiled: HashMap::new(),
            symbols,
            pending_relocs: Vec::new(),
            data_segments: Vec::new(),
            sigs: ModuleSigs::default(),
            user_resolver: None,
        }
    }

    /// 设置用户符号解析回调。查询顺序：
    /// JIT 符号表（已编译/已注册）→ 用户回调 → 平台动态符号
    /// （Windows `GetProcAddress` / Unix `dlsym`）。
    pub fn set_symbol_resolver(
        &mut self,
        resolver: impl Fn(&str) -> Option<u64> + Send + Sync + 'static,
    ) {
        self.user_resolver = Some(Box::new(resolver));
    }

    /// 编译并装载**单个**函数（便捷入口）。
    ///
    /// 编译走 [`CompileSession::add_function`]（带上本 loader 已累积的签名表，
    /// 因此跨函数变参调用有规划依据）。
    pub fn add_function(
        &mut self,
        name: &str,
        signature: &FunctionSignature,
        build_fn: impl FnOnce(&mut FunctionBuilder),
    ) -> Result<(), IrError> {
        let session =
            CompileSession::from_arc(Arc::clone(&self.machine)).with_module_sigs(self.sigs.clone());
        let (compiled, jit_sig) = session
            .add_function_with_jit_sig(name, signature, build_fn)
            .map_err(CompileError::into_ir)?;
        let sig = JitSignatureInfo::from_signature(signature);
        // 记进累积表：本函数的 `FuncRef` 序即"第几个被 add_function 的函数"。
        self.sigs.push(FuncSigEntry {
            variadic: signature.variadic,
            fixed_args: signature.params.len() as u32,
        });
        // **`@N` 符号别名必须一起注册**（FuncRef 序 = 添加序）：生成物的跨函数
        // `Call`/`Jmp` 用 `@N` 形式的符号发重定位（`RelocKind::CALL`）。只有
        // `install`（模块路径）注册过它，`add_function` 这条路径此前**不注册**
        // ⇒ 调用点重定位永远解析不到，编码出的 `E8` 位移保持 -1，跳进垃圾地址
        // ⇒ 进程 AV（实测：JIT 里"先 add 被调方、再 add 调用方"必崩；
        // 单函数编译/直接调被调方都正常）。
        let func_ref = self.sigs.len() - 1;
        self.load_function(ImmStr::from(name), compiled, sig, jit_sig, Some(func_ref))?;
        // 重新解析之前未能解析的重定位
        self.resolve_pending()
    }

    /// 编译 Module 中的所有函数并装载。
    ///
    /// 编译由 [`CompileSession::compile_module`] 完成（大模块并行）；
    /// 装载串行进行（`FuncRef` 序），保证跨函数 `Call` 的符号可见性。
    pub fn compile_module(&mut self, module: &Module) -> Result<(), IrError> {
        let session = CompileSession::from_arc(Arc::clone(&self.machine));
        let compiled = session
            .compile_module(module)
            .map_err(CompileError::into_ir)?;
        self.install(compiled)?;
        self.resolve_pending()
    }

    /// **装载**一份整模块产物：核对目标 ISA → 登记全局符号 → 逐函数（按 `FuncRef` 序）
    /// 重定位 + 分配 + 登记。
    ///
    /// ISA 不匹配 ⇒ `IrError::Unsupported`（fail-closed）：把 x86 的机器码装进 riscv 的
    /// 装载器会一路"装载成功"然后跑出任意行为。
    pub fn install(&mut self, compiled: CompiledModule) -> Result<(), IrError> {
        let isa = self.machine.isa_info().name();
        if compiled.isa() != isa {
            return Err(IrError::Semantic(format!(
                "JIT 装载：产物是给 ISA `{}` 编的，装载器是 `{isa}`——拒绝装载",
                compiled.isa()
            )));
        }
        let (units, globals, data_segments) = compiled.into_parts();
        // 全局先登记（`GlobalAddr` 的绝对重定位在逐函数应用时就要能解析）。
        for (name, ref_name, addr) in globals {
            self.symbols.insert(name, addr);
            self.symbols.insert(ref_name, addr);
        }
        self.data_segments.extend(data_segments);
        for (i, unit) in units.into_iter().enumerate() {
            self.load_function(unit.name, unit.function, unit.sig, unit.jit_sig, Some(i))?;
        }
        Ok(())
    }

    /// 装载一条函数：应用重定位 → 分配可执行内存 → 登记符号。
    ///
    /// `func_ref` = 该函数在模块函数表里的下标（登记 `@N` 别名，跨函数 `Call` 靠它解析）；
    /// 单函数入口传 `None`（没有模块位置）。
    fn load_function(
        &mut self,
        name: ImmStr,
        mut compiled: CompiledFunction,
        sig: JitSignatureInfo,
        jit_sig: JitSig,
        func_ref: Option<usize>,
    ) -> Result<(), IrError> {
        // 尝试解析已记录的跨函数重定位
        self.apply_relocations_to(name.as_str(), &mut compiled)?;
        let mem = ExecutableMemory::new(&compiled.code).map_err(mem_to_compile_err)?;
        let entry_addr = mem.as_ptr() as u64;
        self.symbols.insert(name.clone(), entry_addr);
        if let Some(i) = func_ref {
            self.symbols
                .insert(ImmStr::from(format!("@{i}")), entry_addr);
        }
        self.compiled.insert(
            name,
            CompiledEntry {
                function: compiled,
                mem,
                sig,
                jit_sig,
            },
        );
        Ok(())
    }

    /// 注册外部符号（例如 libc 函数）。
    ///
    /// 当 JIT 代码调用 `Call` 指令引用这些符号时，地址会在编译时被 patch。
    /// 第二十九轮:错误透传(原静默吞错)——重定位解析失败不再被忽略。
    pub fn register_external(&mut self, name: &str, addr: u64) -> Result<(), IrError> {
        self.symbols.insert(ImmStr::from(name), addr);
        // 重新解析待处理的重定位(错误透传)
        self.resolve_pending()
    }

    /// 获取**类型化**函数指针（v30 M4）。
    ///
    /// `F` 必须是 `extern "C" fn(..) -> ..`（0–6 个标量/瘦指针参数）。装载期用
    /// 产物携带的源码签名逐项校验：参数个数与类型、返回类型、非变参、平台 C 约定。
    /// 不匹配 ⇒ `IrError::Semantic`（**不是**缺口，不进台账）。
    ///
    /// 变参 / 向量 / 聚合 / `i128` 用 [`Self::get_unchecked`]（显式 unsafe）。
    pub fn get<F: JitSignature>(&self, name: &str) -> Result<F, IrError> {
        let entry = self.entry(name)?;
        check_typed_call(name, &entry.sig, F::params(), F::ret())?;
        // SAFETY: `entry.mem` 是有效可执行内存；签名已按产物事实逐项校验，且
        // `JitSignature` 只由 `extern "C" fn` 实现（平台 C ABI 与产物一致）。
        unsafe { entry.mem.get_fn::<F>(0).map_err(mem_to_compile_err) }
    }

    /// **逃生口**：不做签名校验，直接按 `F` 取指针。
    ///
    /// # Safety
    /// 调用者必须保证 `F` 与产物 ABI 完全一致（变参/向量/聚合/`i128` 等）。
    pub unsafe fn get_unchecked<F>(&self, name: &str) -> Result<F, IrError> {
        let entry = self.entry(name)?;
        unsafe { entry.mem.get_fn::<F>(0).map_err(mem_to_compile_err) }
    }

    /// 已编译函数的入口地址（不涉及类型；给需要裸地址的宿主）。
    pub fn get_ptr(&self, name: &str) -> Option<*const u8> {
        self.compiled.get(name).map(|e| e.mem.as_ptr())
    }

    /// 已编译函数的机器码/重定位（introspection 用；执行内存仍由本 loader 持有）。
    pub fn function(&self, name: &str) -> Option<&CompiledFunction> {
        self.compiled.get(name).map(|e| &e.function)
    }

    fn entry(&self, name: &str) -> Result<&CompiledEntry, IrError> {
        self.compiled
            .get(name)
            .ok_or_else(|| IrError::Internal(format!("function '{name}' not found in JIT cache")))
    }

    /// 查找给定名称符号的地址（解析链）：
    /// JIT 符号表 → 用户回调 → 平台动态符号（GetProcAddress / dlsym）。
    pub fn lookup_symbol(&self, name: &str) -> Option<u64> {
        self.symbols
            .get(name)
            .copied()
            .or_else(|| self.user_resolver.as_ref().and_then(|r| r(name)))
            .or_else(|| platform_symbol_lookup(name))
    }

    /// 返回已编译函数的数量。
    pub fn len(&self) -> usize {
        self.compiled.len()
    }

    /// 是否为空。
    pub fn is_empty(&self) -> bool {
        self.compiled.is_empty()
    }

    // --- 内部方法 ---

    /// 在代码装入可执行内存之前 patch 已解析的重定位。
    /// 修改 `compiled.code` 字节，使其在装入 ExecutableMemory 后直接可用。
    ///
    /// 注意：PC-relative（Rel）类重定位依赖最终加载地址，只在这里做标记，
    /// 实际 patch 延迟到 `resolve_pending` 中执行。
    fn apply_relocations_to(
        &mut self,
        caller: &str,
        compiled: &mut CompiledFunction,
    ) -> Result<(), IrError> {
        let mut unresolved = Vec::new();
        for reloc in &compiled.relocations {
            if let Some(&target_addr) = self.symbols.get(&reloc.symbol) {
                let offset = reloc.offset;
                match reloc.kind {
                    RelocKind::Relative(_, _) => {
                        // PC-relative 重定位依赖最终加载地址，延迟到 resolve_pending 处理
                        unresolved.push((ImmStr::from(caller), reloc.clone()));
                    }
                    RelocKind::Absolute(w) => {
                        let n = w as usize;
                        if offset + n <= compiled.code.len() {
                            compiled.code[offset..offset + n]
                                .copy_from_slice(&target_addr.to_le_bytes()[..n]);
                        } else {
                            unresolved.push((ImmStr::from(caller), reloc.clone()));
                        }
                    }
                }
            } else {
                unresolved.push((ImmStr::from(caller), reloc.clone()));
            }
        }
        // 追加而非覆盖：多函数模块中每个 caller 的 pending relocation 都要保留，
        // 否则后编译的无 reloc 函数会把前面函数的待解析列表清空（8+ 函数模块中
        // 中间函数的跨函数 call 会丢失 patch，执行返回垃圾值）。
        self.pending_relocs.extend(unresolved);
        Ok(())
    }

    /// 重新解析待处理的重定位，在已装入的可执行内存中 patch。
    fn resolve_pending(&mut self) -> Result<(), IrError> {
        if self.pending_relocs.is_empty() {
            return Ok(());
        }
        let mut still_pending = Vec::new();
        for (func_name, reloc) in &self.pending_relocs {
            if let Some(&target_addr) = self.symbols.get(&reloc.symbol) {
                if let Some(entry) = self.compiled.get_mut(func_name.as_str())
                    && !func_name.is_empty()
                {
                    let mem = &mut entry.mem;
                    let reloc_offset = reloc.offset;
                    let kind = reloc.kind;
                    let site_addr = mem.as_ptr() as u64 + reloc.offset as u64;
                    let mut patch_err: Result<(), IrError> = Ok(());
                    // SAFETY: `mem.modify` temporarily switches the page to
                    // RW, runs the closure on the code buffer, then re-seals
                    // and flushes the icache. `reloc.offset` was validated
                    // against the emitted code length during emission.
                    unsafe {
                        mem.modify(|bytes| {
                            // **PC 相对重定位的可达性检查**（`WA-51`）：x86 的
                            // `RelocKind::Relative(4, _)` 只能表达 ±2GB 的位移。JIT 代码页
                            // 常在低地址（VirtualAlloc），而外部符号（可执行文件里的函数）
                            // 在 `0x7ff6…` 高位——两者差到上万亿字节，位移会被**截断**成
                            // 垃圾地址（实测：桩永远进不去、进程 AV）。这里改成明确报错。
                            if let RelocKind::Relative(w, adj) = kind
                                && w >= 4
                                && let Some(patcher) = self.machine.reloc_patcher()
                            {
                                let _ = (patcher, adj);
                            }
                            if let RelocKind::Relative(w, adj) = kind
                                && w >= 4
                                && self.machine.reloc_patcher().is_none()
                            {
                                let disp =
                                    target_addr as i64 - site_addr as i64 + adj as i64;
                                if disp < i32::MIN as i64 || disp > i32::MAX as i64 {
                                    patch_err = Err(IrError::Internal(format!(
                                        "JIT: PC 相对重定位超出 ±2GB（符号 {:?}：目标 {target_addr:#x} / 站点 {site_addr:#x}）——\
                                         JIT 代码页与外部符号距离过远，需要绝对地址 thunk（见 WA-51）",
                                        reloc.symbol
                                    )));
                                }
                            }
                            if let Some(patcher) = self.machine.reloc_patcher() {
                                let end = (reloc_offset + 8).min(bytes.len());
                                let slice = &mut bytes[reloc_offset..end];
                                if let Err(e) =
                                    patcher.apply(slice, 0, kind, target_addr, site_addr)
                                {
                                    patch_err = Err(e);
                                }
                            } else {
                                let patch_value =
                                    // v23 M5：口径在 RelocKind::patch_value（运行时单点），JIT 不再自己算。
            kind.patch_value(target_addr, site_addr);
                                let bytes_v = kind.encode_value(patch_value);
                                let end = (reloc_offset + bytes_v.len()).min(bytes.len());
                                bytes[reloc_offset..end]
                                    .copy_from_slice(&bytes_v[..end - reloc_offset]);
                            }
                        })
                        .map_err(mem_to_compile_err)?;
                    }
                    patch_err?;
                    // 成功 patch，不放入 still_pending
                    continue;
                }
                // 函数已存在但名称为空或找不到 → 仍需 pending
                still_pending.push((func_name.clone(), reloc.clone()));
            } else {
                still_pending.push((func_name.clone(), reloc.clone()));
            }
        }
        self.pending_relocs = still_pending;
        Ok(())
    }
}

impl<M: TargetMachine + Clone + Default> Default for JitLoader<M> {
    fn default() -> Self {
        Self::new(M::default())
    }
}

// ============================================================
// 测试
// ============================================================

#[cfg(test)]
mod signature_tests;
#[cfg(test)]
mod tests;
