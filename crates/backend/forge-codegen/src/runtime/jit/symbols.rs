//! JIT 符号解析的宿主侧部分（v30 M3：从 `jit.rs` 拆出，**纯搬移**）。
//!
//! 两条来源：
//! - 平台动态符号：Windows `GetProcAddress` / Unix `dlsym`（JIT 没有链接器）；
//! - `__chkstk` 栈探测桩：大帧函数的序言会 `call __chkstk`，AOT 由 MSVC 运行时提供，
//!   JIT 必须自己注册（见下面汇编的注释）。

/// 栈探测符号名（与 `isa/x86.toml` 的 `[machine.frame.stack_probe].symbol` 一致）。
pub(super) const STACK_PROBE_SYMBOL: &str = "__chkstk";

/// Resolve a symbol against the host process's dynamic symbols.
#[cfg(windows)]
pub(super) fn platform_symbol_lookup(name: &str) -> Option<u64> {
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
pub(super) fn platform_symbol_lookup(name: &str) -> Option<u64> {
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

// JIT 侧的 `__chkstk` 实现（汇编）。
//
// **为什么必须是汇编**：序言在**任何**栈调整之前调用它，此刻 `rsp ≡ 0 (mod 16)`，
// 而 Rust 函数的序言假定入口是 `≡ 8` —— 一个 Rust 桩会在自己的 `movaps` 上直接 AV
// （实测：符号解析、reloc 补丁全对，桩却进不去）。真实运行时的 `__chkstk` 同样是手写汇编。
//
// 语义与 x64 `__chkstk` 一致：分配量在 **RAX**，**从近往远**逐页触摸（先摸远页会跳过
// 中间的 guard page ⇒ AV），**不改 RSP**。
#[cfg(target_arch = "x86_64")]
core::arch::global_asm!(
    ".globl __forge_jit_chkstk",
    "__forge_jit_chkstk:",
    "mov r11, rsp",
    "2:",
    "sub r11, 4096",
    "mov byte ptr [r11], 0",
    "sub rax, 4096",
    "ja 2b",
    "ret",
);

#[cfg(target_arch = "x86_64")]
unsafe extern "C" {
    /// 上面那段汇编的入口（只在取地址时引用）。
    fn __forge_jit_chkstk();
}

/// 栈探测桩的入口地址（非 x86_64 宿主为 0；x86 谱才会发这条重定位）。
pub(super) fn stack_probe_addr() -> u64 {
    #[cfg(target_arch = "x86_64")]
    {
        __forge_jit_chkstk as *const () as usize as u64
    }
    #[cfg(not(target_arch = "x86_64"))]
    {
        0
    }
}
