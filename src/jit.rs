//! JIT 编译器 — 高层编译 API。
//!
//! 提供类型安全的 JIT 编译接口，自动管理可执行内存生命周期和符号解析。
//!
//! # Example
//! ```ignore
//! use codegen_lib::jit::JitCompiler;
//! use codegen_lib::backend::x86_64::X86Isa;
//! use codegen_lib::ir::*;
//!
//! let mut jit = JitCompiler::<X86Isa>::new();
//!
//! // 编译函数
//! let sig = Signature::new(&[(Type::I32, "a"), (Type::I32, "b")], &[Type::I32]);
//! jit.add_function("add", &sig, |b| {
//!     let (entry, params) = b.create_block_with_params(&[(Type::I32, "a"), (Type::I32, "b")]);
//!     b.switch_to_block(entry);
//!     let sum = b.iadd(params[0], params[1]);
//!     b.return_(&[sum]);
//! })?;
//!
//! // 获取类型安全的函数指针
//! let add: extern "C" fn(i32, i32) -> i32 = jit.get_fn("add")?;
//! assert_eq!(add(3, 4), 7);
//! ```

use crate::backend::{FunctionCompiler, InstructionSet};
use crate::executable_memory::ExecutableMemory;
use crate::ir::{FunctionBuilder, Module, Signature};
use crate::{CompileError, CompiledFunction, RelocKind, Relocation};
use std::collections::HashMap;

/// 符号解析器 — 将符号名映射到绝对地址。
pub type SymbolResolver<'a> = dyn Fn(&str) -> Option<u64> + 'a;

/// JIT 编译器 — 管理函数的编译、缓存和执行。
///
/// 类型参数 `I` 是目标 ISA（例如 `X86Isa`）。
pub struct JitCompiler<I: InstructionSet + ?Sized> {
    /// 已编译函数的可执行内存（按名称索引）。
    compiled: HashMap<String, (CompiledFunction, ExecutableMemory)>,
    /// 符号表：函数名 → 入口地址（用于跨函数调用解析）。
    symbols: HashMap<String, u64>,
    /// 待应用的跨函数重定位。
    pending_relocs: Vec<(String, Relocation)>,
    _phantom: std::marker::PhantomData<I>,
}

impl<I: InstructionSet> JitCompiler<I> {
    /// 创建新的 JIT 编译器。
    pub fn new() -> Self {
        Self {
            compiled: HashMap::new(),
            symbols: HashMap::new(),
            pending_relocs: Vec::new(),
            _phantom: std::marker::PhantomData,
        }
    }

    /// 编译一个函数并存入缓存。
    ///
    /// `name` 必须唯一 — 可用于后续 `get_fn()` 检索，或作为跨函数调用的符号名。
    /// `signature` 定义函数的参数和返回值类型。
    /// `build_fn` 使用 [`FunctionBuilder`] 构造 IR 体。
    pub fn add_function(
        &mut self,
        name: &str,
        signature: &Signature,
        build_fn: impl FnOnce(&mut FunctionBuilder),
    ) -> Result<(), CompileError> {
        let mut builder = FunctionBuilder::new(name, signature.clone());
        build_fn(&mut builder);
        let func = builder.finish();

        let mut compiled = FunctionCompiler::<I>::compile_raw(&func)?;

        // 尝试解析已记录的跨函数重定位
        self.apply_relocations_to(&mut compiled)?;

        // 分配可执行内存
        let mem = ExecutableMemory::new(&compiled.code)?;

        // 记录符号
        let entry_addr = mem.as_ptr() as u64;
        self.symbols.insert(name.to_string(), entry_addr);

        self.compiled.insert(name.to_string(), (compiled, mem));

        // 重新解析之前未能解析的重定位
        self.resolve_pending()?;

        Ok(())
    }

    /// 编译 Module 中的所有函数。
    pub fn compile_module(&mut self, module: &Module) -> Result<(), CompileError> {
        for &func_ref in &module.func_refs() {
            if let Some(func) = module.get(func_ref) {
                let mut compiled = FunctionCompiler::<I>::compile_raw(func)?;

                self.apply_relocations_to(&mut compiled)?;

                let mem = ExecutableMemory::new(&compiled.code)?;
                let entry_addr = mem.as_ptr() as u64;
                self.symbols.insert(func.name.clone(), entry_addr);
                self.compiled.insert(func.name.clone(), (compiled, mem));
            }
        }
        self.resolve_pending()?;
        Ok(())
    }

    /// 注册外部符号（例如 libc 函数）。
    ///
    /// 当 JIT 代码调用 `Call` 指令引用这些符号时，地址会在编译时被 patch。
    pub fn register_external(&mut self, name: &str, addr: u64) {
        self.symbols.insert(name.to_string(), addr);
        // 重新解析待处理的重定位
        let _ = self.resolve_pending();
    }

    /// 获取已编译函数的类型安全函数指针。
    ///
    /// `F` 必须是匹配函数签名的 `extern "C" fn(...) -> ...` 类型。
    ///
    /// # Panics
    /// 如果找不到 `name` 对应的函数。
    pub fn get_fn<F>(&self, name: &str) -> Result<F, CompileError> {
        let (_, mem) = self
            .compiled
            .get(name)
            .ok_or_else(|| {
                CompileError::Internal(format!("function '{}' not found in JIT cache", name))
            })?;
        unsafe { mem.get_fn::<F>(0) }
    }

    /// 查找给定名称符号的地址。
    /// 用于外部重定位解析器。
    pub fn lookup_symbol(&self, name: &str) -> Option<u64> {
        self.symbols.get(name).copied()
    }

    /// 将已编译的机器码作为命名函数直接加载（跳过 IR 编译阶段）。
    ///
    /// 用于汇编器等外部工具将预编译的 `CompiledFunction` 注入 JIT 缓存，
    /// 之后可通过 `get_fn::<F>(name)` 获取类型安全的函数指针。
    pub fn add_compiled(
        &mut self,
        name: &str,
        compiled: CompiledFunction,
    ) -> Result<(), CompileError> {
        let mem = ExecutableMemory::new(&compiled.code)?;
        let entry_addr = mem.as_ptr() as u64;
        self.symbols.insert(name.to_string(), entry_addr);
        self.compiled
            .insert(name.to_string(), (compiled, mem));
        self.resolve_pending()?;
        Ok(())
    }

    /// 列出所有已编译的函数名。
    pub fn function_names(&self) -> Vec<&str> {
        self.compiled.keys().map(|s| s.as_str()).collect()
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
            RelocKind::Rel(_w, adj) => {
                // PC-relative: target - patch_site + adj
                // x86 Rel(4, -4): target - patch_site - 4
                (target_addr as i64 - patch_site as i64 + adj as i64) as u64
            }
            RelocKind::Abs(_) | RelocKind::Isa(_) => target_addr,
        }
    }

    /// 在代码装入可执行内存之前 patch 已解析的重定位。
    /// 修改 `compiled.code` 字节，使其在装入 ExecutableMemory 后直接可用。
    ///
    /// 注意：PC-relative（Rel）类重定位依赖最终加载地址，只在这里做标记，
    /// 实际 patch 延迟到 `resolve_pending` 中执行。
    fn apply_relocations_to(
        &mut self,
        compiled: &mut CompiledFunction,
    ) -> Result<(), CompileError> {
        let mut unresolved = Vec::new();
        for reloc in &compiled.relocations {
            if let Some(&target_addr) = self.symbols.get(&reloc.symbol) {
                let offset = reloc.offset;
                match reloc.kind {
                    RelocKind::Rel(_, _) => {
                        // PC-relative 重定位依赖最终加载地址，延迟到 resolve_pending 处理
                        unresolved.push((String::new(), reloc.clone()));
                    }
                    RelocKind::Abs(w) => {
                        let n = w as usize;
                        if offset + n <= compiled.code.len() {
                            compiled.code[offset..offset + n]
                                .copy_from_slice(&target_addr.to_le_bytes()[..n]);
                        } else {
                            unresolved.push((String::new(), reloc.clone()));
                        }
                    }
                    RelocKind::Isa(_) => {
                        // ISA 特定重定位留待 encode_isa_reloc 处理
                        unresolved.push((String::new(), reloc.clone()));
                    }
                }
            } else {
                unresolved.push((String::new(), reloc.clone()));
            }
        }
        self.pending_relocs = unresolved;
        Ok(())
    }

    /// 重新解析待处理的重定位，在已装入的可执行内存中 patch。
    fn resolve_pending(&mut self) -> Result<(), CompileError> {
        if self.pending_relocs.is_empty() {
            return Ok(());
        }
        let mut still_pending = Vec::new();
        for (func_name, reloc) in &self.pending_relocs {
            if let Some(&target_addr) = self.symbols.get(&reloc.symbol) {
                if let Some((_, mem)) = self.compiled.get_mut(func_name.as_str())
                    && !func_name.is_empty()
                {
                    let src_addr = mem.as_ptr() as u64;
                    let patch_addr = (src_addr + reloc.offset as u64) as *mut u8;
                    let patch_value =
                        Self::compute_patch_value(reloc.kind, target_addr, patch_addr as u64);
                    // 将 patch 值写入可执行内存
                    let n = match reloc.kind {
                        RelocKind::Rel(w, _) | RelocKind::Abs(w) => w as usize,
                        RelocKind::Isa(_) => {
                            // ISA 特定重定位由编码器处理，仍保持 pending
                            still_pending.push((func_name.clone(), reloc.clone()));
                            continue;
                        }
                    };
                    let bytes = patch_value.to_le_bytes();
                    // 使用 unsafe 写入可执行内存（ExecutableMemory 本身就是可执行的）
                    unsafe {
                        std::ptr::copy_nonoverlapping(bytes.as_ptr(), patch_addr, n);
                    }
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

impl<I: InstructionSet> Default for JitCompiler<I> {
    fn default() -> Self {
        Self::new()
    }
}

// ============================================================
// 测试
// ============================================================

#[cfg(test)]
mod tests {
    use super::*;
    use crate::backend::x86_64::{X86Isa, ensure_registered};
    use crate::ir::*;

    #[test]
    fn test_jit_compiler_new() {
        let jit = JitCompiler::<X86Isa>::new();
        assert!(jit.is_empty());
        assert_eq!(jit.len(), 0);
    }

    #[test]
    fn test_jit_register_external() {
        let mut jit = JitCompiler::<X86Isa>::new();
        jit.register_external("malloc", 0xDEAD_BEEF);
        assert_eq!(jit.lookup_symbol("malloc"), Some(0xDEAD_BEEF));
    }

    #[test]
    fn test_jit_function_names() {
        let mut jit = JitCompiler::<X86Isa>::new();
        // 注册符号（不实际编译）
        jit.register_external("foo", 0x1000);
        jit.register_external("bar", 0x2000);
        assert_eq!(jit.symbols.len(), 2);
    }

    /// E2E: 编译 `fn answer() -> i32 { 42 }` 并通过 JitCompiler 调用。
    #[cfg(target_arch = "x86_64")]
    #[test]
    fn test_jit_compile_and_call_constant() {
        ensure_registered();
        let mut jit = JitCompiler::<X86Isa>::new();

        let sig = Signature::new(&[], &[Type::I32]);
        jit.add_function("answer", &sig, |b| {
            let entry = b.create_block();
            b.switch_to_block(entry);
            let v = b.iconst_i32(42);
            b.return_(&[v]);
        })
        .expect("compile");

        let answer: extern "C" fn() -> i32 = jit.get_fn("answer").expect("get_fn");
        assert_eq!(answer(), 42);
    }

    /// E2E: 编译 `fn add(a: i32, b: i32) -> i32 { a + b }` 并通过 JitCompiler 调用。
    #[cfg(target_arch = "x86_64")]
    #[test]
    fn test_jit_compile_and_call_add() {
        ensure_registered();
        let mut jit = JitCompiler::<X86Isa>::new();

        let sig = Signature::new(&[(Type::I32, "a"), (Type::I32, "b")], &[Type::I32]);
        jit.add_function("add", &sig, |b| {
            let (entry, params) = b.create_block_with_params(&[(Type::I32, "a"), (Type::I32, "b")]);
            b.switch_to_block(entry);
            let sum = b.iadd(params[0], params[1]);
            b.return_(&[sum]);
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
        ensure_registered();
        let mut jit = JitCompiler::<X86Isa>::new();

        // 编译 answer
        let sig0 = Signature::new(&[], &[Type::I32]);
        jit.add_function("answer", &sig0, |b| {
            let entry = b.create_block();
            b.switch_to_block(entry);
            let v = b.iconst_i32(99);
            b.return_(&[v]);
        })
        .expect("compile answer");

        // 编译 double
        let sig1 = Signature::new(&[(Type::I32, "x")], &[Type::I32]);
        jit.add_function("double", &sig1, |b| {
            let (entry, params) = b.create_block_with_params(&[(Type::I32, "x")]);
            b.switch_to_block(entry);
            let two = b.iconst_i32(2);
            let result = b.imul(params[0], two);
            b.return_(&[result]);
        })
        .expect("compile double");

        assert_eq!(jit.len(), 2);

        let answer: extern "C" fn() -> i32 = jit.get_fn("answer").expect("get_fn");
        assert_eq!(answer(), 99);

        let double: extern "C" fn(i32) -> i32 = jit.get_fn("double").expect("get_fn");
        assert_eq!(double(21), 42);
    }
}
