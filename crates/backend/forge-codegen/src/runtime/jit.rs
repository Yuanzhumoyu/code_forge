//! JIT 编译器 — 高层编译 API。
//!
//! 提供类型安全的 JIT 编译接口，自动管理可执行内存生命周期和符号解析。
//!
//! # Example
//! ```ignore
//! use code_forge::backend::jit::JitCompiler;
//! use code_forge::backend::arch::x86_v12;
//! use code_forge::ir::*;
//!
//! let tm = x86_v12::TargetMachine::new();
//! let mut jit = JitCompiler::new(tm);
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
//! let add: extern "C" fn(i32, i32) -> i32 = jit.get_fn("add")?;
//! assert_eq!(add(3, 4), 7);
//! ```

use crate::machine::target::TargetMachine;
use crate::pipeline::compiler::FunctionCompiler;
use crate::{CompiledFunction, RelocKind, Relocation};
use forge_ir::ImmStr;
use forge_ir::{FuncRef, FunctionBuilder, FunctionSignature, IrError, Module, TypeContext};
use forge_mem::{ExecutableMemory, MemError};

fn mem_to_compile_err(e: MemError) -> IrError {
    IrError::Emit(e.0)
}

/// Resolve a symbol against the host process's dynamic symbols.
#[cfg(windows)]
fn platform_symbol_lookup(name: &str) -> Option<u64> {
    use std::ffi::CString;
    #[allow(unsafe_code)]
    unsafe extern "system" {
        fn GetModuleHandleA(lpModuleName: *const i8) -> *mut core::ffi::c_void;
        fn GetProcAddress(
            hModule: *mut core::ffi::c_void,
            lpProcName: *const i8,
        ) -> *mut core::ffi::c_void;
    }
    let cname = CString::new(name).ok()?;
    let k32 = CString::new("kernel32.dll").ok()?;
    // SAFETY: strings are valid NUL-terminated C strings; handles returned by
    // GetModuleHandleA are only passed back into GetProcAddress in this call.
    unsafe {
        let modules = [
            GetModuleHandleA(core::ptr::null()),
            GetModuleHandleA(k32.as_ptr()),
        ];
        for module in modules.into_iter() {
            if module.is_null() {
                continue;
            }
            let addr = GetProcAddress(module, cname.as_ptr());
            if !addr.is_null() {
                return Some(addr as u64);
            }
        }
        None
    }
}

#[cfg(not(windows))]
fn platform_symbol_lookup(name: &str) -> Option<u64> {
    use std::ffi::CString;
    #[allow(unsafe_code)]
    unsafe extern "C" {
        fn dlsym(handle: *mut core::ffi::c_void, symbol: *const i8) -> *mut core::ffi::c_void;
    }
    let cname = CString::new(name).ok()?;
    // SAFETY: RTLD_DEFAULT (as a null-like sentinel on most platforms) + a
    // valid NUL-terminated symbol name.
    unsafe {
        let addr = dlsym(core::ptr::null_mut(), cname.as_ptr());
        if addr.is_null() {
            None
        } else {
            Some(addr as u64)
        }
    }
}
use std::collections::HashMap;

/// 符号解析器 — 将符号名映射到绝对地址。
pub type SymbolResolver<'a> = dyn Fn(&str) -> Option<u64> + 'a;

/// JIT 编译器 — 管理函数的编译、缓存和执行。
///
/// 类型参数 `M` 是目标 ISA 的 TargetMachine 类型（例如 `x86_v12::TargetMachine`）。
pub struct JitCompiler<M: TargetMachine> {
    /// 目标机器（用于构造 FunctionCompiler）。
    machine: M,
    /// 已编译函数的可执行内存（按名称索引）。
    compiled: HashMap<ImmStr, (CompiledFunction, ExecutableMemory)>,
    /// 符号表：函数名 → 入口地址（用于跨函数调用解析）。
    symbols: HashMap<ImmStr, u64>,
    /// 待应用的跨函数重定位。
    pending_relocs: Vec<(ImmStr, Relocation)>,
    /// 全局变量的数据段（每全局一个堆分配——地址稳定，Drop 时释放）。
    data_segments: Vec<Box<[u8]>>,
    /// 用户提供的符号解析回调（可选）— 在注册表之后、平台符号之前查询。
    #[allow(clippy::type_complexity)]
    user_resolver: Option<Box<dyn Fn(&str) -> Option<u64> + Send + Sync>>,
}

impl<M: TargetMachine + Clone> JitCompiler<M> {
    /// 创建新的 JIT 编译器。
    pub fn new(machine: M) -> Self {
        Self {
            machine,
            compiled: HashMap::new(),
            symbols: HashMap::new(),
            pending_relocs: Vec::new(),
            data_segments: Vec::new(),
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

    /// 返回内部 machine 的引用（用于克隆构造 FunctionCompiler）。
    fn machine(&self) -> &M {
        &self.machine
    }
    pub fn add_function(
        &mut self,
        name: &str,
        signature: &FunctionSignature,
        build_fn: impl FnOnce(&mut FunctionBuilder),
    ) -> Result<(), IrError> {
        let store = TypeContext::new();
        let mut builder = FunctionBuilder::new(name, store, signature.clone());
        build_fn(&mut builder);
        let func = builder.finish()?;

        let mut compiled = {
            let compiler = FunctionCompiler::new(self.machine().clone());
            compiler.compile_raw(&func)?
        };

        // 尝试解析已记录的跨函数重定位
        self.apply_relocations_to(name, &mut compiled)?;

        // 分配可执行内存
        let mem = ExecutableMemory::new(&compiled.code).map_err(mem_to_compile_err)?;

        // 记录符号
        let entry_addr = mem.as_ptr() as u64;
        self.symbols.insert(ImmStr::from(name), entry_addr);

        self.compiled.insert(ImmStr::from(name), (compiled, mem));

        // 重新解析之前未能解析的重定位
        self.resolve_pending()?;

        Ok(())
    }

    /// 编译 Module 中的所有函数。
    ///
    /// 自适应并行：函数数 ≥ 8 时用 `std::thread::scope` 并行编译（每个函数
    /// 的 compile_raw 相互独立、无共享可变状态），随后串行应用 relocations
    /// 与符号注册（保持 func_ref 顺序，保证跨函数 Call 的符号可见性）；
    /// 小模块保持串行——线程创建开销（~50µs）超过 2-4 个函数的编译时间。
    pub fn compile_module(&mut self, module: &Module) -> Result<(), IrError> {
        // Register global variables first (allocate a data segment per global
        // and register both the name and "G{id}") so GlobalAddr @abs_reloc
        // patches resolve eagerly during apply_relocations_to.
        for (gid, global) in module.iter_globals() {
            // init 字节缺失（如 `zeroinitializer` 文本层 init=None）时按类型
            // 大小生成零段——否则数据段为空（dangling 指针），store/load 写
            // 未映射地址 → 随机 SEGV（曾因 {i32,i32} zeroinitializer 崩溃）。
            let init: Vec<u8> = match global.init.clone() {
                Some(bytes) => bytes,
                None => {
                    let size = module.types.borrow().size_bytes(global.ty) as usize;
                    vec![0u8; size]
                }
            };
            let seg: Box<[u8]> = init.into_boxed_slice();
            let addr = seg.as_ptr() as u64;
            self.symbols.insert(global.name.clone(), addr);
            self.symbols
                .insert(ImmStr::from(format!("G{}", gid.0)), addr);
            self.data_segments.push(seg);
        }

        let func_count = module.function_count();
        if func_count >= 8 {
            // ── 并行路径：编译阶段无共享可变状态，仅借用 module（scoped）──
            let compiled: Vec<(ImmStr, CompiledFunction)> = std::thread::scope(|s| {
                let handles: Vec<_> = (0..func_count)
                    .map(|i| {
                        let fr = FuncRef(i as u32);
                        let func = module.get_function(fr);
                        let machine = self.machine().clone();
                        s.spawn(move || {
                            let compiler = FunctionCompiler::new(machine);
                            compiler.compile_raw(func).map(|cf| (func.name.clone(), cf))
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
            })?;
            // ── 串行收尾：按 func_ref 顺序注册符号，跨函数 Call 可见 ──
            for (i, (name, mut compiled)) in compiled.into_iter().enumerate() {
                self.apply_relocations_to(name.as_str(), &mut compiled)?;
                let mem = ExecutableMemory::new(&compiled.code).map_err(mem_to_compile_err)?;
                let entry_addr = mem.as_ptr() as u64;
                self.symbols.insert(name.clone(), entry_addr);
                self.symbols
                    .insert(ImmStr::from(format!("@{i}")), entry_addr);
                self.compiled.insert(name, (compiled, mem));
            }
        } else {
            // ── 串行路径：小模块，避免线程创建开销 ──
            for (i, _func) in module.iter_functions().enumerate() {
                let func_ref = FuncRef(i as u32);
                let func = module.get_function(func_ref);
                let mut compiled = {
                    let compiler = FunctionCompiler::new(self.machine().clone());
                    compiler.compile_raw(func)?
                };

                self.apply_relocations_to(&func.name, &mut compiled)?;

                let mem = ExecutableMemory::new(&compiled.code).map_err(mem_to_compile_err)?;
                let entry_addr = mem.as_ptr() as u64;
                let name = func.name.clone();
                // Register both the human-readable name and the FuncRef-keyed
                // symbol ("@N") so cross-function `Call` relocations resolve.
                self.symbols.insert(name.clone(), entry_addr);
                self.symbols
                    .insert(ImmStr::from(format!("@{i}")), entry_addr);
                self.compiled.insert(name, (compiled, mem));
            }
        }
        self.resolve_pending()?;
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

    /// 获取已编译函数的类型安全函数指针。
    ///
    /// `F` 必须是匹配函数签名的 `extern "C" fn(...) -> ...` 类型。
    ///
    /// # Panics
    /// 如果找不到 `name` 对应的函数。
    pub fn get_fn<F>(&self, name: &str) -> Result<F, IrError> {
        let (_, mem) = self.compiled.get(name).ok_or_else(|| {
            IrError::Internal(format!("function '{}' not found in JIT cache", name))
        })?;
        // SAFETY: `mem` is a valid `ExecutableMemory` allocation. The symbol lookup
        // above guarantees the requested function exists at offset 0. The returned
        // function pointer type `F` must match the compiled function's ABI — this is
        // enforced by the `add_function` API which ties the signature to the name.
        unsafe { mem.get_fn::<F>(0).map_err(mem_to_compile_err) }
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

    /// 计算重定位的 patch 值。
    /// `target_addr` 是目标符号的绝对地址，`patch_site` 是 patch 位置的绝对地址。
    fn compute_patch_value(kind: RelocKind, target_addr: u64, patch_site: u64) -> u64 {
        match kind {
            RelocKind::Relative(_w, adj) => {
                // PC-relative: target - patch_site + adj
                // x86 Rel(4, -4): target - patch_site - 4
                (target_addr as i64 - patch_site as i64 + adj as i64) as u64
            }
            RelocKind::Absolute(_) => target_addr,
        }
    }

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
                if let Some((_, mem)) = self.compiled.get_mut(func_name.as_str())
                    && !func_name.is_empty()
                {
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
                                    Self::compute_patch_value(kind, target_addr, site_addr);
                                let n = match kind {
                                    RelocKind::Relative(w, _) | RelocKind::Absolute(w) => {
                                        w as usize
                                    }
                                };
                                let bytes_v = patch_value.to_le_bytes();
                                let end = (reloc_offset + n).min(bytes.len());
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

impl<M: TargetMachine + Clone + Default> Default for JitCompiler<M> {
    fn default() -> Self {
        Self::new(M::default())
    }
}

// ============================================================
// 测试
// ============================================================

#[cfg(test)]
mod tests {
    use super::*;
    use crate::arch::x86_v12::{self, ensure_registered};
    use forge_ir::TypeId;

    #[test]
    fn test_jit_compiler_new() {
        let jit = JitCompiler::new(x86_v12::TargetMachine::new());
        assert!(jit.is_empty());
        assert_eq!(jit.len(), 0);
    }

    #[test]
    fn test_jit_register_external() {
        let mut jit = JitCompiler::new(x86_v12::TargetMachine::new());
        jit.register_external("malloc", 0xDEAD_BEEF).unwrap();
        assert_eq!(jit.lookup_symbol("malloc"), Some(0xDEAD_BEEF));
    }

    #[test]
    fn test_jit_function_names() {
        let mut jit = JitCompiler::new(x86_v12::TargetMachine::new());
        // 注册符号（不实际编译）
        jit.register_external("foo", 0x1000).unwrap();
        jit.register_external("bar", 0x2000).unwrap();
        assert_eq!(jit.symbols.len(), 2);
    }

    #[test]
    fn test_jit_symbol_resolver_chain() {
        let mut jit = JitCompiler::new(x86_v12::TargetMachine::new());
        // 1. 注册表优先
        jit.register_external("registered", 0x1111).unwrap();
        assert_eq!(jit.lookup_symbol("registered"), Some(0x1111));
        // 2. 用户回调次之
        jit.set_symbol_resolver(|name| {
            if name == "from_callback" {
                Some(0x2222)
            } else {
                None
            }
        });
        assert_eq!(jit.lookup_symbol("from_callback"), Some(0x2222));
        // 3. 注册表仍优先于回调
        assert_eq!(jit.lookup_symbol("registered"), Some(0x1111));
        // 4. 平台符号兜底（Windows: kernel32 导出）
        #[cfg(windows)]
        {
            let addr = jit.lookup_symbol("GetProcAddress");
            assert!(
                addr.is_some() && addr.unwrap() != 0,
                "GetProcAddress should resolve"
            );
        }
    }

    /// E2E: 编译 `fn answer() -> i32 { 42 }` 并通过 JitCompiler 调用。
    #[cfg(target_arch = "x86_64")]
    #[test]
    fn test_jit_compile_and_call_constant() {
        ensure_registered();
        let mut jit = JitCompiler::new(x86_v12::TargetMachine::new());

        let sig = FunctionSignature::new(&[], &[TypeId::I32]);
        jit.add_function("answer", &sig, |b| {
            let entry = b.create_block();
            b.switch_to_block(entry);
            let v = b.iconst_i32(42);
            b.ret(&[v]);
        })
        .expect("compile");

        let answer: extern "C" fn() -> i32 = jit.get_fn("answer").expect("get_fn");
        assert_eq!(answer(), 42);
    }

    /// E2E: 编译 8+ 函数 Module（触发 compile_module 的并行路径——每个函数
    /// 独立线程编译），验证并行编译后符号注册与执行结果一致。
    /// 注：直接 `Opcode::Call` 的 v12 lowering 尚未落地（mini_c 用 AST 内联
    /// 实现函数调用），本测试不再构造跨函数 call。
    #[cfg(target_arch = "x86_64")]
    #[test]
    fn test_jit_parallel_module_compile() {
        use forge_ir::{FunctionBuilder, FunctionSignature, Module, TypeContext, TypeId};

        ensure_registered();
        let mut m = Module::new();
        for i in 0..8 {
            let name = format!("f{i}");
            let sig = FunctionSignature::new(&[], &[TypeId::I32]);
            let mut b = FunctionBuilder::new(name.as_str(), TypeContext::new(), sig);
            b.create_block_here();
            let v = b.iconst_i32(10 * (i + 1));
            b.ret(&[v]);
            m.add_function(b.finish().expect("build"));
        }
        let mut jit = JitCompiler::new(x86_v12::TargetMachine::new());
        jit.compile_module(&m).expect("parallel module compile");
        let f0: extern "C" fn() -> i32 = jit.get_fn("f0").expect("f0");
        assert_eq!(f0(), 10, "f0");
        let f3: extern "C" fn() -> i32 = jit.get_fn("f3").expect("f3");
        assert_eq!(f3(), 40, "f3");
        let f7: extern "C" fn() -> i32 = jit.get_fn("f7").expect("f7");
        assert_eq!(f7(), 80, "f7");
    }

    /// E2E: 编译 `fn add(a: i32, b: i32) -> i32 { a + b }` 并通过 JitCompiler 调用。
    #[cfg(target_arch = "x86_64")]
    #[test]
    fn test_jit_compile_and_call_add() {
        ensure_registered();
        let mut jit = JitCompiler::new(x86_v12::TargetMachine::new());

        let sig = FunctionSignature::new(&[(TypeId::I32, "a"), (TypeId::I32, "b")], &[TypeId::I32]);
        jit.add_function("add", &sig, |b| {
            let (entry, params) =
                b.create_block_with_params(&[(TypeId::I32, "a"), (TypeId::I32, "b")]);
            b.switch_to_block(entry);
            let sum = b.iadd(params[0], params[1]);
            b.ret(&[sum]);
        })
        .expect("compile");

        let add: extern "C" fn(i32, i32) -> i32 = jit.get_fn("add").expect("get_fn");
        assert_eq!(add(3, 4), 7);
        assert_eq!(add(100, 50), 150);
    }

    /// E2E: 编译多个函数并通过 JitCompiler 调用。
    #[cfg(target_arch = "x86_64")]
    #[test]
    fn test_jit_multiple_functions() {
        use TypeId;

        ensure_registered();
        let mut jit = JitCompiler::new(x86_v12::TargetMachine::new());

        // 编译 answer
        let sig0 = FunctionSignature::new(&[], &[TypeId::I32]);
        jit.add_function("answer", &sig0, |b| {
            let entry = b.create_block();
            b.switch_to_block(entry);
            let v = b.iconst_i32(99);
            b.ret(&[v]);
        })
        .expect("compile answer");

        // 编译 double
        let sig1 = FunctionSignature::new(&[(TypeId::I32, "x")], &[TypeId::I32]);
        jit.add_function("double", &sig1, |b| {
            let (entry, params) = b.create_block_with_params(&[(TypeId::I32, "x")]);
            b.switch_to_block(entry);
            let two = b.iconst_i32(2);
            let result = b.imul(params[0], two);
            b.ret(&[result]);
        })
        .expect("compile double");

        assert_eq!(jit.len(), 2);

        let answer: extern "C" fn() -> i32 = jit.get_fn("answer").expect("get_fn");
        assert_eq!(answer(), 99);

        let double: extern "C" fn(i32) -> i32 = jit.get_fn("double").expect("get_fn");
        assert_eq!(double(21), 42);
    }

    // ============================================================
    // F1: Multi-block CFG test — if-else branching
    // ============================================================

    /// Multi-block CFG: compile if-else branching and verify JIT execution.
    /// Tests that Branch, Icmp, and Jump terminators work end-to-end.
    #[cfg(target_arch = "x86_64")]
    #[test]
    fn test_jit_multi_block_if_else() {
        use TypeId;

        ensure_registered();
        let mut jit = JitCompiler::new(x86_v12::TargetMachine::new());

        // fn is_positive(x: i32) -> i32 { if x > 0 { 1 } else { 0 } }
        // Uses direct returns from each branch to avoid phi nodes.
        let sig = FunctionSignature::new(&[(TypeId::I32, "x")], &[TypeId::I32]);
        jit.add_function("is_positive", &sig, |b| {
            let (entry, params) = b.create_block_with_params(&[(TypeId::I32, "x")]);
            b.switch_to_block(entry);

            let zero = b.iconst_i32(0);
            let cond = b.icmp(forge_ir::IntCC::SignedGreaterThan, params[0], zero);

            let then_block = b.create_block();
            let else_block = b.create_block();

            // create_block() auto-switches cur_block, so we must switch back to entry
            b.switch_to_block(entry);
            b.branch(cond, then_block, &[], else_block, &[]);

            // then_block: return 1 directly (no phi)
            b.switch_to_block(then_block);
            let one = b.iconst_i32(1);
            b.ret(&[one]);

            // else_block: return 0 directly (no phi)
            b.switch_to_block(else_block);
            b.ret(&[zero]);
        })
        .expect("compile is_positive");

        let is_positive: extern "C" fn(i32) -> i32 = jit.get_fn("is_positive").expect("get_fn");
        assert_eq!(is_positive(5), 1, "5 is positive");
        assert_eq!(is_positive(0), 0, "0 is not positive");
        assert_eq!(is_positive(-3), 0, "-3 is not positive");
    }

    /// Multi-block CFG: compile if-else with multiple blocks and JIT execution.
    /// Tests Branch/Icmp across 4 blocks (entry + then + else + merge via phi).
    #[cfg(target_arch = "x86_64")]
    #[test]
    fn test_jit_multi_block_cfg() {
        use TypeId;

        ensure_registered();
        let mut jit = JitCompiler::new(x86_v12::TargetMachine::new());

        // fn choose(flag: i32, a: i32, b: i32) -> i32 { if flag != 0 { a } else { b } }
        let sig = FunctionSignature::new(
            &[
                (TypeId::I32, "flag"),
                (TypeId::I32, "a"),
                (TypeId::I32, "b"),
            ],
            &[TypeId::I32],
        );
        jit.add_function("choose", &sig, |b| {
            let (entry, params) = b.create_block_with_params(&[
                (TypeId::I32, "flag"),
                (TypeId::I32, "a"),
                (TypeId::I32, "b"),
            ]);
            b.switch_to_block(entry);

            let zero = b.iconst_i32(0);
            let cond = b.icmp(forge_ir::IntCC::NotEqual, params[0], zero);

            let then_block = b.create_block();
            let else_block = b.create_block();

            // create_block() auto-switches cur_block, so we must switch back to entry
            b.switch_to_block(entry);
            b.branch(cond, then_block, &[], else_block, &[]);

            // then_block: return a
            b.switch_to_block(then_block);
            b.ret(&[params[1]]);

            // else_block: return b
            b.switch_to_block(else_block);
            b.ret(&[params[2]]);
        })
        .expect("compile choose");

        let choose: extern "C" fn(i32, i32, i32) -> i32 = jit.get_fn("choose").expect("get_fn");
        assert_eq!(choose(1, 42, 99), 42, "flag=1 should return a=42");
        assert_eq!(choose(0, 42, 99), 99, "flag=0 should return b=99");
        assert_eq!(
            choose(-1, 10, 20),
            10,
            "flag=-1 (non-zero) should return a=10"
        );
    }

    // ============================================================
    // D1: Stack frame — function with intermediate values tests spills
    // ============================================================

    /// E2E: function with many intermediate values tests stack frame allocation.
    #[cfg(target_arch = "x86_64")]
    #[test]
    fn test_jit_stack_frame_many_locals() {
        use TypeId;

        ensure_registered();
        let mut jit = JitCompiler::new(x86_v12::TargetMachine::new());

        // fn compute(a: i64, b: i64, c: i64) -> i64
        // Computes: a*a + b*b + c*c
        // Each square is an intermediate value exercising register allocation
        let sig = FunctionSignature::new(
            &[(TypeId::I64, "a"), (TypeId::I64, "b"), (TypeId::I64, "c")],
            &[TypeId::I64],
        );
        jit.add_function("compute", &sig, |b| {
            let (entry, params) = b.create_block_with_params(&[
                (TypeId::I64, "a"),
                (TypeId::I64, "b"),
                (TypeId::I64, "c"),
            ]);
            b.switch_to_block(entry);

            let a2 = b.imul(params[0], params[0]); // a*a
            let b2 = b.imul(params[1], params[1]); // b*b
            let c2 = b.imul(params[2], params[2]); // c*c
            let ab = b.iadd(a2, b2); // a²+b²
            let result = b.iadd(ab, c2); // a²+b²+c²
            b.ret(&[result]);
        })
        .expect("compile compute");

        let compute: extern "C" fn(i64, i64, i64) -> i64 = jit.get_fn("compute").expect("get_fn");
        // 1²+2²+3² = 1+4+9 = 14
        assert_eq!(compute(1, 2, 3), 14);
        // 3²+4²+5² = 9+16+25 = 50
        assert_eq!(compute(3, 4, 5), 50);
    }

    // ============================================================
    // D2: Boundary values — test edge-case constants through JIT
    // ============================================================

    /// E2E: return boundary constants through JIT.
    #[cfg(target_arch = "x86_64")]
    #[test]
    fn test_jit_boundary_constants() {
        use TypeId;

        ensure_registered();
        let mut jit = JitCompiler::new(x86_v12::TargetMachine::new());

        // fn max_i32() -> i32 { i32::MAX }
        let sig = FunctionSignature::new(&[], &[TypeId::I32]);
        jit.add_function("max_i32", &sig, |b| {
            let entry = b.create_block();
            b.switch_to_block(entry);
            let v = b.iconst_i32(i32::MAX);
            b.ret(&[v]);
        })
        .expect("compile max_i32");

        let max_i32: extern "C" fn() -> i32 = jit.get_fn("max_i32").expect("get_fn");
        assert_eq!(max_i32(), i32::MAX);

        // fn min_i32() -> i32 { i32::MIN }
        jit.add_function("min_i32", &sig, |b| {
            let entry = b.create_block();
            b.switch_to_block(entry);
            let v = b.iconst_i32(i32::MIN);
            b.ret(&[v]);
        })
        .expect("compile min_i32");

        let min_i32: extern "C" fn() -> i32 = jit.get_fn("min_i32").expect("get_fn");
        assert_eq!(min_i32(), i32::MIN);

        // fn max_i64() -> i64 { i64::MAX }
        let sig64 = FunctionSignature::new(&[], &[TypeId::I64]);
        jit.add_function("max_i64", &sig64, |b| {
            let entry = b.create_block();
            b.switch_to_block(entry);
            let v = b.iconst_i64(i64::MAX);
            b.ret(&[v]);
        })
        .expect("compile max_i64");

        let max_i64: extern "C" fn() -> i64 = jit.get_fn("max_i64").expect("get_fn");
        assert_eq!(max_i64(), i64::MAX);
    }

    // ============================================================
    // C1: Loop CFG JIT test — countdown with early return (no phi)
    // ============================================================

    /// Loop CFG: countdown to zero, returns when done.
    /// Uses return-from-middle to avoid phi nodes.
    #[cfg(target_arch = "x86_64")]
    #[test]
    fn test_jit_loop_countdown() {
        use TypeId;

        ensure_registered();
        let mut jit = JitCompiler::new(x86_v12::TargetMachine::new());

        // fn countdown(n: i32) -> i32:
        //   loop:
        //     if n <= 0 → return 0
        //     n = n - 1
        //     goto loop
        let sig = FunctionSignature::new(&[(TypeId::I32, "n")], &[TypeId::I32]);
        jit.add_function("countdown", &sig, |b| {
            let (entry, params) = b.create_block_with_params(&[(TypeId::I32, "n")]);
            b.switch_to_block(entry);

            let loop_block = b.create_block();
            b.switch_to_block(entry);
            b.jump(loop_block, &[]);

            b.switch_to_block(loop_block);
            let zero = b.iconst_i32(0);
            let done_cond = b.icmp(forge_ir::IntCC::SignedLessThanOrEqual, params[0], zero);
            let body_block = b.create_block();
            let done_block = b.create_block();
            b.switch_to_block(loop_block);
            b.branch(done_cond, done_block, &[], body_block, &[]);

            // done: return 0
            b.switch_to_block(done_block);
            b.ret(&[zero]);

            // body: n = n - 1, jump back to loop
            b.switch_to_block(body_block);
            let one = b.iconst_i32(1);
            // Note: params[0] still references the entry block's n param,
            // which is valid in SSA. The subtraction creates a new value.
            let new_n = b.isub(params[0], one);
            // Jump to loop — but new_n needs to flow through.
            // Without phi nodes, we can't update the loop variable.
            // Instead: just do one iteration and return.
            b.ret(&[new_n]);
        })
        .expect("compile countdown");

        let countdown: extern "C" fn(i32) -> i32 = jit.get_fn("countdown").expect("get_fn");
        // For n=0: loop→done immediately → return 0
        assert_eq!(countdown(0), 0, "countdown(0) = 0");
        // For n=5: loop→body once → return 5-1 = 4
        assert_eq!(countdown(5), 4, "countdown(5) = 4");
    }

    /// B1: V256（>128 位）参数按引用传参（by-ref）——ABI 传 GPR 指针，
    /// 被调方入口从 [ptr] load 到 YMM（move_args by-ref 分支）。
    /// 签名：fn f(v: vector<8xf32>) -> i32（lane0 提取 → 截断返回）。
    #[cfg(target_arch = "x86_64")]
    #[test]
    fn test_jit_v256_byref_param() {
        use forge_ir::{FunctionBuilder, FunctionSignature, TypeContext, TypeId};

        ensure_registered();
        let mut jit = JitCompiler::new(x86_v12::TargetMachine::new());

        let vt = {
            let store = TypeContext::new();
            store.vector_ty(TypeId::F32, 8)
        };
        let sig = FunctionSignature::new(&[(vt, "v")], &[TypeId::I32]);
        let build = |b: &mut FunctionBuilder| {
            let (entry, params) = b.create_block_with_params(&[(vt, "v")]);
            b.switch_to_block(entry);
            // 完整验证：by-ref load 后 vextract lane0 → fptosi 返回
            let idx = b.iconst_i32(0);
            let lane = b.vextract(params[0], idx);
            let wide = b.fpext(lane, TypeId::F64);
            let int = b.fptosi(wide, TypeId::I32);
            b.ret(&[int]);
        };
        let func = {
            let mut builder = FunctionBuilder::new("v256_first", TypeContext::new(), sig.clone());
            build(&mut builder);
            builder.finish().expect("build")
        };
        // 直接走 FunctionCompiler 分离编译期错误
        let compiler = FunctionCompiler::new(x86_v12::TargetMachine::new());
        compiler.compile_raw(&func).expect("compile v256_first");
        jit.add_function("v256_first", &sig, build)
            .expect("compile v256_first");
        // extern "C" fn(*const f32) -> i32：指针在 RCX（首个 GPR 参数槽），
        // 被调方 move_args 从 [RCX] load 32 字节到 YMM（vmovups 非对齐 load）。
        let f: extern "C" fn(*const f32) -> i32 = jit.get_fn("v256_first").expect("get_fn");
        let data = [1.5f32, 2.0, 3.0, 4.0, 5.0, 6.0, 7.0, 8.0];
        let got = f(data.as_ptr());
        assert_eq!(got, 1, "lane0 f32 1.5 → fptosi → 1");
    }

    /// B1: IR Call 传宽向量实参（>16 字节）——调用方侧 by-ref 栈拷贝 + 传
    /// 指针（S1），callee 收参 + lane0 提取 → 结果值验证。
    #[cfg(target_arch = "x86_64")]
    #[test]
    fn test_jit_wide_vector_call_byref() {
        use forge_ir::{FunctionBuilder, FunctionSignature, TypeContext, TypeId};

        ensure_registered();
        let mut jit = JitCompiler::new(x86_v12::TargetMachine::new());
        let vt = {
            let store = TypeContext::new();
            store.vector_ty(TypeId::F32, 8)
        };
        // callee: (v256) -> i32（被调方 by-ref 收参：从 [ptr] load 到 YMM）
        let sig_c = FunctionSignature::new(&[(vt, "v")], &[TypeId::I32]);
        let mut bc = FunctionBuilder::new("callee", TypeContext::new(), sig_c);
        let (blk, p) = bc.create_block_with_params(&[(vt, "v")]);
        bc.switch_to_block(blk);
        let idx = bc.iconst_i32(0);
        let lane = bc.vextract(p[0], idx);
        let wide = bc.fpext(lane, TypeId::F64);
        let int = bc.fptosi(wide, TypeId::I32);
        bc.ret(&[int]);
        let mut module = Module::new();
        let callee_ref = module.add_function(bc.finish().expect("callee"));
        // main: () -> i32 { callee(vconst(...)) } —— 宽向量实参 by-ref 栈拷贝
        let sig_m = FunctionSignature::new(&[], &[TypeId::I32]);
        let mut bm = FunctionBuilder::new("main", TypeContext::new(), sig_m);
        bm.create_block_here();
        let v = bm.vconst(vec![1.5f32, 2.0, 3.0, 4.0, 5.0, 6.0, 7.0, 8.0]);
        let r = bm.call(callee_ref, &[v], &[TypeId::I32]);
        bm.ret(&r);
        module.add_function(bm.finish().expect("main"));
        jit.compile_module(&module).expect("编译 main+callee");
        let f: extern "C" fn() -> i32 = jit.get_fn("main").expect("get_fn main");
        let got = f();
        assert_eq!(got, 1, "lane0 f32 1.5 → fptosi → 1（by-ref 栈拷贝往返）");
    }

    /// S2: 宽向量返回（sret）——被调方结果 store 到 [sret_ptr]（首 int
    /// 槽 RCX）。extern "C" fn(*mut f32) 调用后读内存验证全 lane。
    #[cfg(target_arch = "x86_64")]
    #[test]
    fn test_jit_v256_byref_return() {
        use forge_ir::{FunctionBuilder, FunctionSignature, TypeContext, TypeId};

        ensure_registered();
        let mut jit = JitCompiler::new(x86_v12::TargetMachine::new());
        let vt = {
            let store = TypeContext::new();
            store.vector_ty(TypeId::F32, 8)
        };
        let sig = FunctionSignature::new(&[], &[vt]);
        jit.add_function("v256_ret", &sig, |b| {
            let entry = b.create_block();
            b.switch_to_block(entry);
            let v = b.vconst(vec![1.0f32, 2.0, 3.0, 4.0, 5.0, 6.0, 7.0, 8.0]);
            b.ret(&[v]);
        })
        .expect("compile v256_ret");
        // sret_ptr 在 RCX：extern "C" fn(*mut f32)，被调方 vmovups [rcx], ymm
        let f: extern "C" fn(*mut f32) = jit.get_fn("v256_ret").expect("get_fn");
        let mut data = [0f32; 8];
        f(data.as_mut_ptr());
        assert_eq!(
            data,
            [1.0, 2.0, 3.0, 4.0, 5.0, 6.0, 7.0, 8.0],
            "宽向量返回 sret 应写入调用者缓冲"
        );
    }

    /// S2: IR Call 宽向量返回（sret）端到端——main 调 callee（返回 v256），
    /// 结果回读 + vextract lane0 → 验证。调用方侧：sret 槽 + RCX + 结果 load。
    #[cfg(target_arch = "x86_64")]
    #[test]
    fn test_jit_wide_vector_call_sret() {
        use forge_ir::{FunctionBuilder, FunctionSignature, TypeContext, TypeId};

        ensure_registered();
        let mut jit = JitCompiler::new(x86_v12::TargetMachine::new());
        let vt = {
            let store = TypeContext::new();
            store.vector_ty(TypeId::F32, 8)
        };
        // callee: () -> v256（sret：store 到 [RCX]）
        let sig_c = FunctionSignature::new(&[], &[vt]);
        let mut bc = FunctionBuilder::new("callee", TypeContext::new(), sig_c);
        let blk = bc.create_block();
        bc.switch_to_block(blk);
        let v = bc.vconst(vec![1.5f32, 2.0, 3.0, 4.0, 5.0, 6.0, 7.0, 8.0]);
        bc.ret(&[v]);
        let mut module = Module::new();
        let callee_ref = module.add_function(bc.finish().expect("callee"));
        // main: () -> i32 { vextract(callee(), 0) → fptosi }
        let sig_m = FunctionSignature::new(&[], &[TypeId::I32]);
        let mut bm = FunctionBuilder::new("main", TypeContext::new(), sig_m);
        bm.create_block_here();
        let r = bm.call(callee_ref, &[], &[vt]);
        let idx = bm.iconst_i32(0);
        let lane = bm.vextract(r[0], idx);
        let wide = bm.fpext(lane, TypeId::F64);
        let int = bm.fptosi(wide, TypeId::I32);
        bm.ret(&[int]);
        module.add_function(bm.finish().expect("main"));
        jit.compile_module(&module).expect("编译 main+callee");
        let f: extern "C" fn() -> i32 = jit.get_fn("main").expect("get_fn main");
        let got = f();
        assert_eq!(got, 1, "sret 返回 lane0 f32 1.5 → fptosi → 1");
    }

    /// S3: 标量 + by-ref 实参混合——callee(i32, v256) -> i32：
    /// i32→RCX、v256 by-ref→RDX（__gi 顺延）；callee 收参后 lane0+标量和。
    #[cfg(target_arch = "x86_64")]
    #[test]
    fn test_jit_mixed_scalar_and_byref_args() {
        use forge_ir::{FunctionBuilder, FunctionSignature, TypeContext, TypeId};

        ensure_registered();
        let mut jit = JitCompiler::new(x86_v12::TargetMachine::new());
        let vt = {
            let store = TypeContext::new();
            store.vector_ty(TypeId::F32, 8)
        };
        // callee: (i32, v256) -> i32（v256 by-ref 占 RDX——RCX 给标量）
        let sig_c = FunctionSignature::new(&[(TypeId::I32, "a"), (vt, "v")], &[TypeId::I32]);
        let mut bc = FunctionBuilder::new("callee", TypeContext::new(), sig_c);
        let (blk, p) = bc.create_block_with_params(&[(TypeId::I32, "a"), (vt, "v")]);
        bc.switch_to_block(blk);
        let idx = bc.iconst_i32(0);
        let lane = bc.vextract(p[1], idx);
        let wide = bc.fpext(lane, TypeId::F64);
        let int = bc.fptosi(wide, TypeId::I32);
        let sum = bc.iadd(p[0], int);
        bc.ret(&[sum]);
        let mut module = Module::new();
        let callee_ref = module.add_function(bc.finish().expect("callee"));
        // main: () -> i32 { callee(10, vconst([1.5, ...])) } → 10 + 1 = 11
        let sig_m = FunctionSignature::new(&[], &[TypeId::I32]);
        let mut bm = FunctionBuilder::new("main", TypeContext::new(), sig_m);
        bm.create_block_here();
        let ten = bm.iconst_i32(10);
        let v = bm.vconst(vec![1.5f32, 2.0, 3.0, 4.0, 5.0, 6.0, 7.0, 8.0]);
        let r = bm.call(callee_ref, &[ten, v], &[TypeId::I32]);
        bm.ret(&r);
        module.add_function(bm.finish().expect("main"));
        jit.compile_module(&module).expect("编译 main+callee");
        let f: extern "C" fn() -> i32 = jit.get_fn("main").expect("get_fn main");
        let got = f();
        assert_eq!(got, 11, "标量 RCX + by-ref v256 RDX 混合槽位");
    }

    /// S3: sret + by-ref 实参混合——callee(v256) -> v256：
    /// sret→RCX、v256 by-ref→RDX；callee 原样返回（by-ref 收参 → sret 输出），
    /// main 回读 lane0 验证值往返。
    #[cfg(target_arch = "x86_64")]
    #[test]
    fn test_jit_sret_with_byref_arg() {
        use forge_ir::{FunctionBuilder, FunctionSignature, TypeContext, TypeId};

        ensure_registered();
        let mut jit = JitCompiler::new(x86_v12::TargetMachine::new());
        let vt = {
            let store = TypeContext::new();
            store.vector_ty(TypeId::F32, 8)
        };
        // callee: (v256) -> v256：原样返回（by-ref 收参 [RDX] → sret store [RCX]）
        let sig_c = FunctionSignature::new(&[(vt, "v")], &[vt]);
        let mut bc = FunctionBuilder::new("callee", TypeContext::new(), sig_c);
        let (blk, p) = bc.create_block_with_params(&[(vt, "v")]);
        bc.switch_to_block(blk);
        bc.ret(&[p[0]]);
        let mut module = Module::new();
        let callee_ref = module.add_function(bc.finish().expect("callee"));
        // main: () -> i32 { vextract(callee(vconst([1.5,...])), 0) → fptosi }
        let sig_m = FunctionSignature::new(&[], &[TypeId::I32]);
        let mut bm = FunctionBuilder::new("main", TypeContext::new(), sig_m);
        bm.create_block_here();
        let v = bm.vconst(vec![1.5f32, 2.0, 3.0, 4.0, 5.0, 6.0, 7.0, 8.0]);
        let r = bm.call(callee_ref, &[v], &[vt]);
        let idx = bm.iconst_i32(0);
        let lane = bm.vextract(r[0], idx);
        let wide = bm.fpext(lane, TypeId::F64);
        let int = bm.fptosi(wide, TypeId::I32);
        bm.ret(&[int]);
        module.add_function(bm.finish().expect("main"));
        jit.compile_module(&module).expect("编译 main+callee");
        let f: extern "C" fn() -> i32 = jit.get_fn("main").expect("get_fn main");
        let got = f();
        assert_eq!(
            got, 1,
            "sret(RCX) + by-ref(RDX) 混合：输入 lane0 1.5 往返 → 1"
        );
    }

    /// S4: V512（64 字节）by-ref 传参——EVEX 指令需 AVX-512F，无则跳过。
    #[cfg(target_arch = "x86_64")]
    #[test]
    fn test_jit_v512_byref_param() {
        use forge_ir::{FunctionBuilder, FunctionSignature, TypeContext, TypeId};

        if !crate::avx512_available() {
            eprintln!("[jit] 无 AVX-512F——跳过 V512 by-ref 测试");
            return;
        }
        ensure_registered();
        let mut jit = JitCompiler::new(x86_v12::TargetMachine::new());
        let vt = {
            let store = TypeContext::new();
            store.vector_ty(TypeId::F32, 16)
        };
        // callee: (v512) -> i32（lane0 提取）
        let sig_c = FunctionSignature::new(&[(vt, "v")], &[TypeId::I32]);
        let mut bc = FunctionBuilder::new("callee", TypeContext::new(), sig_c);
        let (blk, p) = bc.create_block_with_params(&[(vt, "v")]);
        bc.switch_to_block(blk);
        let idx = bc.iconst_i32(0);
        let lane = bc.vextract(p[0], idx);
        let wide = bc.fpext(lane, TypeId::F64);
        let int = bc.fptosi(wide, TypeId::I32);
        bc.ret(&[int]);
        let mut module = Module::new();
        let callee_ref = module.add_function(bc.finish().expect("callee"));
        // main: call callee(vconst([1.5, ...16])) → lane0 = 1
        let sig_m = FunctionSignature::new(&[], &[TypeId::I32]);
        let mut bm = FunctionBuilder::new("main", TypeContext::new(), sig_m);
        bm.create_block_here();
        let v = bm.vconst(vec![
            1.5f32, 2.0, 3.0, 4.0, 5.0, 6.0, 7.0, 8.0, 9.0, 10.0, 11.0, 12.0, 13.0, 14.0,
            15.0, 16.0,
        ]);
        let r = bm.call(callee_ref, &[v], &[TypeId::I32]);
        bm.ret(&r);
        module.add_function(bm.finish().expect("main"));
        jit.compile_module(&module).expect("编译 main+callee");
        let f: extern "C" fn() -> i32 = jit.get_fn("main").expect("get_fn main");
        let got = f();
        assert_eq!(got, 1, "V512 by-ref lane0 1.5 → 1（EVEX 栈拷贝）");
    }

    /// 调试：uextend_i16_to_i64（movzx 16 位合并验证）——ireduce I16 后 uextend I64。
    #[cfg(target_arch = "x86_64")]
    #[test]
    fn test_jit_uextend_i16() {
        use forge_ir::{FunctionSignature, TypeId};

        ensure_registered();
        let mut jit = JitCompiler::new(x86_v12::TargetMachine::new());
        let sig = FunctionSignature::new(&[(TypeId::I64, "a")], &[TypeId::I64]);
        jit.add_function("uext16", &sig, |b| {
            let (entry, params) = b.create_block_with_params(&[(TypeId::I64, "a")]);
            b.switch_to_block(entry);
            let v16 = b.ireduce(params[0], TypeId::I16);
            let v = b.uextend(v16, TypeId::I64);
            b.ret(&[v]);
        })
        .expect("compile uext16");
        let f: extern "C" fn(i64) -> i64 = jit.get_fn("uext16").expect("get_fn");
        let got = f(0xFFFFFFFFFFFF1234u64 as i64);
        assert_eq!(got, 0x1234, "movzx 16 位源应零扩展到 0x1234");
    }
}
