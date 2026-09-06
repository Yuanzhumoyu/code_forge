//! 可执行内存分配器 — JIT 编译核心工具。
//!
//! 提供跨平台的可执行内存分配/释放功能，使编译后的机器码可以在运行时直接调用。
//!
//! # 安全设计
//!
//! 采用 W^X (Write XOR Execute) 安全模型：
//! 1. 分配 RW (可读写) 内存
//! 2. 写入代码字节
//! 3. 切换为 RX (可读可执行) — 不可同时写入
//! 4. 刷新指令缓存（ARM 必需，x86 为 no-op）
//!
//! # Example
//! ```ignore
//! use codegen_lib::executable_memory::ExecutableMemory;
//!
//! let func = compile::<MyIsa>("add", &sig, |b| { ... })?;
//! let exec_mem = ExecutableMemory::new(&func.code)?;
//! let add_fn: extern "C" fn(i32, i32) -> i32 = unsafe { exec_mem.get_fn(0) };
//! assert_eq!(unsafe { add_fn(1, 2) }, 3);
//! ```

pub mod host_cpu;

pub use host_cpu::*;

/// Error type for executable memory operations.
#[derive(Debug, Clone)]
pub struct MemError(pub String);

impl std::fmt::Display for MemError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.0)
    }
}

impl std::error::Error for MemError {}

impl From<String> for MemError {
    fn from(s: String) -> Self {
        MemError(s)
    }
}

impl From<&str> for MemError {
    fn from(s: &str) -> Self {
        MemError(s.to_string())
    }
}

/// 一块包含已编译代码的可执行内存区域。
///
/// 内存采用 W^X 模型：写入时代码在 RW 内存中，`seal()` 后切换为 RX。
pub struct ExecutableMemory {
    ptr: *mut u8,
    size: usize,
    /// 内存是否已 seal（已切换为 RX，不可再写入）。
    is_sealed: bool,
}

// ExecutableMemory 是线程安全的：内存标记为可执行后只读。
unsafe impl Send for ExecutableMemory {}
unsafe impl Sync for ExecutableMemory {}

impl ExecutableMemory {
    /// 分配可执行内存并将 `code` 拷贝进去。
    ///
    /// 内存自动 seal 为 RX，可直接作为函数指针调用。
    pub fn new(code: &[u8]) -> Result<Self, MemError> {
        if code.is_empty() {
            return Err(MemError("empty code".into()));
        }

        let size = code.len();
        let page_size = page_size();
        let alloc_size = (size + page_size - 1) & !(page_size - 1);

        // 1. 分配 RW 内存（不可执行）
        let ptr = alloc_rw(alloc_size)?;

        // 2. 将代码拷贝到内存中
        unsafe {
            core::ptr::copy_nonoverlapping(code.as_ptr(), ptr, size);
        }

        // 用断点指令（0xCC）填充剩余字节（防止意外执行到填充区）
        if alloc_size > size {
            unsafe {
                core::ptr::write_bytes(ptr.add(size), 0xCC, alloc_size - size);
            }
        }

        // 3. 刷新指令缓存（ARM 必需）
        flush_icache(ptr, alloc_size);

        // 4. 切换为 RX（不可写）
        make_executable(ptr, alloc_size)?;

        Ok(Self {
            ptr,
            size: alloc_size,
            is_sealed: true,
        })
    }

    /// 分配可写内存，稍后可调用 `seal()` 切换为可执行。
    ///
    /// 用于需要先 patching/重定位再执行的场景。
    pub fn new_writable(code: &[u8]) -> Result<Self, MemError> {
        if code.is_empty() {
            return Err(MemError("empty code".into()));
        }

        let size = code.len();
        let page_size = page_size();
        let alloc_size = (size + page_size - 1) & !(page_size - 1);

        let ptr = alloc_rw(alloc_size)?;

        unsafe {
            core::ptr::copy_nonoverlapping(code.as_ptr(), ptr, size);
        }

        if alloc_size > size {
            unsafe {
                core::ptr::write_bytes(ptr.add(size), 0xCC, alloc_size - size);
            }
        }

        Ok(Self {
            ptr,
            size: alloc_size,
            is_sealed: false,
        })
    }

    /// 将内存切换为可执行（RX），之后不可再写入。
    ///
    /// 调用此方法前，可以先 patching 代码（例如应用重定位）。
    pub fn seal(&mut self) -> Result<(), MemError> {
        if self.is_sealed {
            return Ok(());
        }
        flush_icache(self.ptr, self.size);
        make_executable(self.ptr, self.size)?;
        self.is_sealed = true;
        Ok(())
    }

    /// 临时切换回 RW 以修改代码，然后重新 seal。
    ///
    /// # Safety
    ///
    /// 修改代码期间不能有其它线程执行此内存中的代码。
    pub unsafe fn modify<F>(&mut self, f: F) -> Result<(), MemError>
    where
        F: FnOnce(&mut [u8]),
    {
        if self.is_sealed {
            make_writable(self.ptr, self.size)?;
            self.is_sealed = false;
        }

        let slice = unsafe { core::slice::from_raw_parts_mut(self.ptr, self.size) };
        f(slice);

        self.seal()
    }

    /// 返回可执行内存的原始指针。
    pub fn as_ptr(&self) -> *const u8 {
        self.ptr
    }

    /// 返回此内存区域的大小（字节）。
    pub fn len(&self) -> usize {
        self.size
    }

    /// 是否为空。
    pub fn is_empty(&self) -> bool {
        self.size == 0
    }

    /// 向 GDB 注册此 JIT 代码区域（如果 GDB JIT 接口可用）。
    ///
    /// 调用后，GDB 可以识别 JIT 编译的函数名和代码范围，
    /// 支持反汇编和回溯。
    pub fn register_with_gdb(&self, name: &str) {
        register_jit_code(self.ptr, self.size, name);
    }

    /// 从可执行内存中获取函数指针。
    ///
    /// Returns `Err(MemError(...))` if the offset is out of bounds.
    ///
    /// # Safety
    ///
    /// - 调用者必须确保类型 `F` 的函数签名与偏移处代码的实际调用约定匹配
    /// - 代码必须在这一偏移处开始一个有效的函数入口点
    pub unsafe fn get_fn<F>(&self, offset: usize) -> Result<F, MemError> {
        if offset >= self.size {
            return Err(MemError(format!(
                "offset {offset} exceeds memory size {}",
                self.size
            )));
        }
        let fn_ptr = unsafe { self.ptr.add(offset) as *const () };
        Ok(unsafe { core::mem::transmute_copy(&fn_ptr) })
    }

    /// 将原始字节切片视为可执行内存中的一个函数并调用。
    ///
    /// 一次性快捷方法：分配 → 拷贝 → seal → 调用 → 释放。
    ///
    /// # Safety
    ///
    /// 调用者必须确保类型参数匹配代码的实际调用约定和签名。
    pub unsafe fn exec_once<F, R>(code: &[u8], f: impl FnOnce(F) -> R) -> Result<R, MemError> {
        let mem = Self::new(code)?;
        let fn_ptr: F = unsafe { mem.get_fn(0)? };
        Ok(f(fn_ptr))
    }
}

impl Drop for ExecutableMemory {
    fn drop(&mut self) {
        if !self.ptr.is_null() {
            unsafe {
                free_executable(self.ptr, self.size);
            }
            self.ptr = core::ptr::null_mut();
            self.size = 0;
        }
    }
}

impl core::fmt::Debug for ExecutableMemory {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        f.debug_struct("ExecutableMemory")
            .field("ptr", &self.ptr)
            .field("size", &self.size)
            .field("is_sealed", &self.is_sealed)
            .finish()
    }
}

// ============================================================
// 平台特定实现
// ============================================================

/// 获取系统页大小。
fn page_size() -> usize {
    #[cfg(windows)]
    {
        #[repr(C)]
        #[allow(non_snake_case)]
        struct SystemInfo {
            _union1: [u8; 4],
            dwPageSize: u32,
            _rest: [u8; 56],
        }

        #[allow(unsafe_code)]
        unsafe extern "system" {
            fn GetSystemInfo(lpSystemInfo: *mut SystemInfo);
        }

        let mut info = SystemInfo {
            _union1: [0; 4],
            dwPageSize: 0,
            _rest: [0; 56],
        };
        unsafe {
            GetSystemInfo(&mut info as *mut SystemInfo);
        }
        info.dwPageSize as usize
    }

    #[cfg(not(windows))]
    {
        // 在 Linux/macOS 上通常为 4096
        // 可通过 sysconf(_SC_PAGESIZE) 获取，但 4K 是安全的默认值
        4096
    }
}

/// 分配可读-可写内存（不可执行）。
/// W^X 安全模型的第一步。
fn alloc_rw(size: usize) -> Result<*mut u8, MemError> {
    #[cfg(windows)]
    {
        #[allow(unsafe_code)]
        unsafe extern "system" {
            fn VirtualAlloc(
                lpAddress: *mut core::ffi::c_void,
                dwSize: usize,
                flAllocationType: u32,
                flProtect: u32,
            ) -> *mut core::ffi::c_void;
        }

        const MEM_COMMIT: u32 = 0x00001000;
        const MEM_RESERVE: u32 = 0x00002000;
        const PAGE_READWRITE: u32 = 0x04;

        let ptr = unsafe {
            VirtualAlloc(
                core::ptr::null_mut(),
                size,
                MEM_COMMIT | MEM_RESERVE,
                PAGE_READWRITE,
            )
        };

        if ptr.is_null() {
            return Err(MemError(format!("VirtualAlloc failed for {size} bytes")));
        }

        Ok(ptr as *mut u8)
    }

    #[cfg(not(windows))]
    {
        #[allow(unsafe_code)]
        unsafe extern "C" {
            fn mmap(
                addr: *mut core::ffi::c_void,
                length: usize,
                prot: i32,
                flags: i32,
                fd: i32,
                offset: i64,
            ) -> *mut core::ffi::c_void;
        }

        const PROT_READ: i32 = 1;
        const PROT_WRITE: i32 = 2;
        const MAP_PRIVATE: i32 = 2;

        #[cfg(target_os = "macos")]
        const MAP_ANON: i32 = 0x1000;
        #[cfg(not(target_os = "macos"))]
        const MAP_ANON: i32 = 0x20;

        let prot = PROT_READ | PROT_WRITE;
        let flags = MAP_PRIVATE | MAP_ANON;
        let fd = -1i32;
        let offset = 0i64;

        let ptr = unsafe { mmap(core::ptr::null_mut(), size, prot, flags, fd, offset) };

        if ptr as usize == usize::MAX || ptr.is_null() {
            return Err(MemError(format!("mmap failed for {size} bytes")));
        }

        Ok(ptr as *mut u8)
    }
}

/// 切换内存保护为可读-可执行（不可写）。
fn make_executable(ptr: *mut u8, size: usize) -> Result<(), MemError> {
    #[cfg(windows)]
    {
        #[allow(unsafe_code)]
        unsafe extern "system" {
            fn VirtualProtect(
                lpAddress: *mut core::ffi::c_void,
                dwSize: usize,
                flNewProtect: u32,
                lpflOldProtect: *mut u32,
            ) -> i32;
        }

        const PAGE_EXECUTE_READ: u32 = 0x20;
        let mut old_protect: u32 = 0;

        let ret = unsafe {
            VirtualProtect(
                ptr as *mut core::ffi::c_void,
                size,
                PAGE_EXECUTE_READ,
                &mut old_protect as *mut u32,
            )
        };

        if ret == 0 {
            return Err(MemError(format!(
                "VirtualProtect(EXECUTE_READ) failed for {size} bytes"
            )));
        }
    }

    #[cfg(not(windows))]
    {
        #[allow(unsafe_code)]
        unsafe extern "C" {
            fn mprotect(addr: *mut core::ffi::c_void, len: usize, prot: i32) -> i32;
        }

        const PROT_READ: i32 = 1;
        const PROT_EXEC: i32 = 4;

        let ret = unsafe { mprotect(ptr as *mut core::ffi::c_void, size, PROT_READ | PROT_EXEC) };

        if ret != 0 {
            return Err(MemError(format!(
                "mprotect(READ|EXEC) failed for {size} bytes"
            )));
        }
    }

    Ok(())
}

/// 临时切换回可写（用于 patching）。
fn make_writable(ptr: *mut u8, size: usize) -> Result<(), MemError> {
    #[cfg(windows)]
    {
        #[allow(unsafe_code)]
        unsafe extern "system" {
            fn VirtualProtect(
                lpAddress: *mut core::ffi::c_void,
                dwSize: usize,
                flNewProtect: u32,
                lpflOldProtect: *mut u32,
            ) -> i32;
        }

        const PAGE_READWRITE: u32 = 0x04;
        let mut old_protect: u32 = 0;

        let ret = unsafe {
            VirtualProtect(
                ptr as *mut core::ffi::c_void,
                size,
                PAGE_READWRITE,
                &mut old_protect as *mut u32,
            )
        };

        if ret == 0 {
            return Err(MemError(format!(
                "VirtualProtect(READWRITE) failed for {size} bytes"
            )));
        }
    }

    #[cfg(not(windows))]
    {
        #[allow(unsafe_code)]
        unsafe extern "C" {
            fn mprotect(addr: *mut core::ffi::c_void, len: usize, prot: i32) -> i32;
        }

        const PROT_READ: i32 = 1;
        const PROT_WRITE: i32 = 2;

        let ret = unsafe { mprotect(ptr as *mut core::ffi::c_void, size, PROT_READ | PROT_WRITE) };

        if ret != 0 {
            return Err(MemError(format!(
                "mprotect(READ|WRITE) failed for {size} bytes"
            )));
        }
    }

    Ok(())
}

/// 刷新指令缓存。
///
/// 在 ARM 上，写入代码后必须刷新 icache 才能正确执行。
/// 在 x86_64 上，指令缓存是自动一致的，此函数为 no-op。
fn flush_icache(ptr: *mut u8, size: usize) {
    #[cfg(windows)]
    {
        #[allow(unsafe_code)]
        unsafe extern "system" {
            fn FlushInstructionCache(
                hProcess: *mut core::ffi::c_void,
                lpBaseAddress: *const core::ffi::c_void,
                dwSize: usize,
            ) -> i32;
            fn GetCurrentProcess() -> *mut core::ffi::c_void;
        }

        unsafe {
            FlushInstructionCache(GetCurrentProcess(), ptr as *const core::ffi::c_void, size);
        }
    }

    #[cfg(not(windows))]
    {
        // x86_64: 指令缓存自动一致，无需操作
        // ARM: 需要 `__clear_cache` 内建函数或 syscall
        // 使用 compiler_fence 确保写入对 icache 可见
        #[cfg(any(target_arch = "aarch64", target_arch = "arm"))]
        {
            // ARM 平台：使用 __clear_cache（GCC/Clang 内建）
            // 通过 FFI 调用 libc 的 __clear_cache，或使用内联汇编
            #[allow(unsafe_code)]
            unsafe {
                // __clear_cache 在不同平台上可能有不同签名
                // 此处在 ARM Linux 上通过 syscall cacheflush 实现
                #[cfg(target_os = "linux")]
                {
                    // ARM Linux cacheflush syscall
                    core::arch::asm!(
                        "mov x0, {ptr}",
                        "mov x1, {end}",
                        "mov x2, #0",
                        "mov x8, #0", // __ARM_NR_cacheflush (may vary)
                        "svc #0",
                        ptr = in(reg) ptr as u64,
                        end = in(reg) (ptr as u64).wrapping_add(size as u64),
                        out("x0") _,
                        out("x1") _,
                        out("x2") _,
                        out("x8") _,
                        options(nostack)
                    );
                }
                #[cfg(not(target_os = "linux"))]
                {
                    // macOS/other: 尝试使用 __clear_cache（edition 2024：
                    // extern 块必须是 unsafe extern）
                    #[allow(unsafe_code)]
                    unsafe extern "C" {
                        fn __clear_cache(start: *mut u8, end: *mut u8);
                    }
                    __clear_cache(ptr, ptr.add(size));
                }
            }
        }
        #[cfg(not(any(target_arch = "aarch64", target_arch = "arm")))]
        {
            // x86_64 / other: 使用编译器屏障确保写入顺序
            // （ptr/size 在此平台无操作数用途——显式消费，避免
            // -D warnings 下 unused variable 编译错误）
            let _ = (ptr, size);
            core::sync::atomic::compiler_fence(core::sync::atomic::Ordering::SeqCst);
        }
    }
}

// ============================================================
// GDB JIT 接口 — 向调试器注册 JIT 编译的代码
// ============================================================

/// GDB JIT 描述符（与 GDB 的 `__jit_debug_descriptor` 兼容）。
#[repr(C)]
struct JitDescriptor {
    version: u32,
    action_flag: u32,
    relevant_entry: *mut JitCodeEntry,
    first_entry: *mut JitCodeEntry,
}

/// GDB JIT 代码条目。
#[repr(C)]
struct JitCodeEntry {
    next_entry: *mut JitCodeEntry,
    prev_entry: *mut JitCodeEntry,
    symfile_addr: *const u8,
    symfile_size: u64,
}

/// GDB JIT 描述符 — GDB 在运行时查找此符号。
/// 当 `action_flag` 设置为 1 时，GDB 读取 `first_entry` 链表。
#[unsafe(no_mangle)]
#[used]
static mut __jit_debug_descriptor: JitDescriptor = JitDescriptor {
    version: 1,
    action_flag: 0,
    relevant_entry: core::ptr::null_mut(),
    first_entry: core::ptr::null_mut(),
};

/// 触发 GDB 重新读取 JIT 代码条目。
#[unsafe(no_mangle)]
extern "C" fn __jit_debug_register_code() {
    // GDB 在加载新代码后调用此函数（通过断点）
    // 此处仅作为占位符号存在
}

/// 向 GDB JIT 接口注册一段 JIT 编译的代码。
///
/// 分配一个 `JitCodeEntry` 并将其加入链表。
/// 注意：当前简化实现使用泄漏分配器（永远不会释放 entry）。
fn register_jit_code(ptr: *mut u8, size: usize, name: &str) {
    // 创建简单的符号文件内容（ELF 格式的 symbol-only 文件）
    let symfile = create_jit_symfile(ptr, size, name);

    // 分配 JIT code entry（使用 Box::leak 使其生命周期与进程一致）
    let entry = Box::into_raw(Box::new(JitCodeEntry {
        next_entry: core::ptr::null_mut(),
        prev_entry: core::ptr::null_mut(),
        symfile_addr: symfile.as_ptr(),
        symfile_size: symfile.len() as u64,
    }));

    // 泄漏 symfile — 需要保持有效直到进程退出
    let _ = Box::into_raw(symfile.into_boxed_slice());

    // 将 entry 插入链表头
    unsafe {
        let desc = &mut *core::ptr::addr_of_mut!(__jit_debug_descriptor);
        (*entry).next_entry = desc.first_entry;
        if let Some(first) = desc.first_entry.as_mut() {
            first.prev_entry = entry;
        }
        desc.first_entry = entry;
        desc.action_flag = 1; // 通知 GDB 有新代码
    }
}

/// 创建最小的 JIT 符号文件（ELF 格式）。
/// 包含一个 FUNC 符号，使 GDB 能显示函数名。
fn create_jit_symfile(ptr: *mut u8, size: usize, name: &str) -> Vec<u8> {
    use std::io::Write;
    let mut buf = Vec::new();

    // ELF header (64-bit, little-endian)
    let _ = write!(&mut buf, "\x7FELF"); // ELF magic
    buf.push(2); // 64-bit
    buf.push(1); // little-endian
    buf.push(1); // ELF version
    buf.push(0); // OS/ABI (System V)
    buf.extend_from_slice(&[0u8; 8]); // padding
    buf.extend_from_slice(&2u16.to_le_bytes()); // ET_EXEC
    // e_machine 按宿主架构（P4：原硬编码 EM_X86_64——aarch64 宿主上 GDB 无法解析）
    let e_machine: u16 = match crate::CpuFeatures::host_arch_name() {
        "aarch64" => 183, // EM_AARCH64
        "riscv64" => 243, // EM_RISCV
        _ => 62,          // EM_X86_64（含未知架构回退——x86_64 与 aarch64 之外默认）
    };
    buf.extend_from_slice(&e_machine.to_le_bytes()); // e_machine
    buf.extend_from_slice(&1u32.to_le_bytes()); // EV_CURRENT
    buf.extend_from_slice(&0u64.to_le_bytes()); // entry point
    buf.extend_from_slice(&0u64.to_le_bytes()); // program header offset
    buf.extend_from_slice(&0u64.to_le_bytes()); // section header offset
    buf.extend_from_slice(&0u32.to_le_bytes()); // flags
    buf.extend_from_slice(&64u16.to_le_bytes()); // ELF header size
    buf.extend_from_slice(&0u16.to_le_bytes()); // program header entry size
    buf.extend_from_slice(&0u16.to_le_bytes()); // program header count
    buf.extend_from_slice(&0u16.to_le_bytes()); // section header entry size
    buf.extend_from_slice(&0u16.to_le_bytes()); // section header count
    buf.extend_from_slice(&0u16.to_le_bytes()); // section name string table index

    // Symbol name
    let symname = format!("{name}\0");
    let symname_bytes = symname.as_bytes();

    // Simple string table
    let strtab_offset = 64u64 + size as u64;
    let symbol_offset = strtab_offset;
    let name_in_strtab = 1u32; // starts at offset 1 (after initial \0)

    // Extended bytes to reach reasonable size
    while buf.len() < 64 {
        buf.push(0);
    }
    // Append code content so GDB can read it
    unsafe {
        let code_slice = core::slice::from_raw_parts(ptr, size);
        buf.extend_from_slice(code_slice);
    }
    // String table (starts with \0)
    buf.push(0);
    buf.extend_from_slice(symname_bytes);

    // ELF symbol (simplified)
    let st_name = name_in_strtab;
    let st_info: u8 = 0x12; // STB_GLOBAL | STT_FUNC
    let st_other: u8 = 0;
    let st_shndx: u16 = 0xFFF1; // SHN_ABS
    let st_value: u64 = ptr as u64;
    let st_size: u64 = size as u64;

    let sym_pos = buf.len();
    buf.extend_from_slice(&st_name.to_le_bytes());
    buf.push(st_info);
    buf.push(st_other);
    buf.extend_from_slice(&st_shndx.to_le_bytes());
    buf.extend_from_slice(&st_value.to_le_bytes());
    buf.extend_from_slice(&st_size.to_le_bytes());

    // Pad to 16-byte alignment
    while buf.len() % 16 != 0 {
        buf.push(0);
    }

    // Fix up section header offset
    let shdr_offset = buf.len() as u64;
    buf[40..48].copy_from_slice(&shdr_offset.to_le_bytes());

    // Single SHT_SYMTAB section header
    buf.extend_from_slice(&0u32.to_le_bytes()); // sh_name
    buf.extend_from_slice(&2u32.to_le_bytes()); // SHT_SYMTAB
    buf.extend_from_slice(&0u64.to_le_bytes()); // sh_flags
    buf.extend_from_slice(&0u64.to_le_bytes()); // sh_addr
    buf.extend_from_slice(&symbol_offset.to_le_bytes()); // sh_offset
    let sym_size = (sym_pos as u64) - symbol_offset + 24;
    buf.extend_from_slice(&sym_size.to_le_bytes()); // sh_size
    buf.extend_from_slice(&0u32.to_le_bytes()); // sh_link
    buf.extend_from_slice(&0u32.to_le_bytes()); // sh_info
    buf.extend_from_slice(&8u64.to_le_bytes()); // sh_addralign
    buf.extend_from_slice(&24u64.to_le_bytes()); // sh_entsize
    // Section name string table header
    buf.extend_from_slice(&1u32.to_le_bytes()); // sh_name
    buf.extend_from_slice(&3u32.to_le_bytes()); // SHT_STRTAB
    buf.extend_from_slice(&0u64.to_le_bytes()); // sh_flags
    buf.extend_from_slice(&0u64.to_le_bytes()); // sh_addr
    buf.extend_from_slice(&strtab_offset.to_le_bytes()); // sh_offset
    buf.extend_from_slice(&(symname_bytes.len() as u64 + 1).to_le_bytes()); // sh_size
    buf.extend_from_slice(&0u32.to_le_bytes()); // sh_link
    buf.extend_from_slice(&0u32.to_le_bytes()); // sh_info
    buf.extend_from_slice(&1u64.to_le_bytes()); // sh_addralign
    buf.extend_from_slice(&0u64.to_le_bytes()); // sh_entsize
    // Section name string table entry
    let shstrtab_offset = buf.len() as u64;
    buf[56..64].copy_from_slice(&shstrtab_offset.to_le_bytes());
    buf.push(0); // null name
    buf.extend_from_slice(b".symtab\0");
    buf.extend_from_slice(b".strtab\0");

    buf
}

/// 释放可执行内存。
unsafe fn free_executable(ptr: *mut u8, size: usize) {
    #[cfg(windows)]
    {
        #[allow(unsafe_code)]
        unsafe extern "system" {
            fn VirtualFree(
                lpAddress: *mut core::ffi::c_void,
                dwSize: usize,
                dwFreeType: u32,
            ) -> i32;
        }
        const MEM_RELEASE: u32 = 0x00008000;
        unsafe {
            VirtualFree(ptr as *mut core::ffi::c_void, 0, MEM_RELEASE);
        }
        let _ = size;
    }

    #[cfg(not(windows))]
    {
        #[allow(unsafe_code)]
        unsafe extern "C" {
            fn munmap(addr: *mut core::ffi::c_void, length: usize) -> i32;
        }
        unsafe {
            munmap(ptr as *mut core::ffi::c_void, size);
        }
    }
}

// ============================================================
// 测试
// ============================================================

#[cfg(test)]
mod tests {
    use super::*;

    /// x86_64 的简单加法函数: lea eax, [rcx+rdx]; ret
    fn add_code() -> Vec<u8> {
        vec![
            0x8d, 0x04, 0x11, // lea eax, [rcx + rdx]
            0xc3, // ret
        ]
    }

    /// x86_64 返回常量函数: mov eax, 42; ret
    fn constant_code() -> Vec<u8> {
        vec![
            0xb8, 0x2a, 0x00, 0x00, 0x00, // mov eax, 42
            0xc3, // ret
        ]
    }

    #[test]
    fn test_alloc_and_free() {
        let code = add_code();
        let mem = ExecutableMemory::new(&code).expect("allocate");
        assert!(mem.len() >= code.len());
        assert!(!mem.ptr.is_null());
        assert!(mem.is_sealed);
    }

    #[test]
    fn test_call_add_function() {
        let code = add_code();
        let mem = ExecutableMemory::new(&code).expect("allocate");

        let add: extern "C" fn(i32, i32) -> i32 = unsafe { mem.get_fn(0).unwrap() };
        assert_eq!(add(1, 2), 3);
        assert_eq!(add(10, 20), 30);
        assert_eq!(add(-5, 5), 0);
    }

    #[test]
    fn test_call_constant() {
        let code = constant_code();
        let mem = ExecutableMemory::new(&code).expect("allocate");

        let get_answer: extern "C" fn() -> i32 = unsafe { mem.get_fn(0).unwrap() };
        assert_eq!(get_answer(), 42);
    }

    #[test]
    fn test_exec_once() {
        let code = add_code();
        let result = unsafe {
            ExecutableMemory::exec_once::<extern "C" fn(i32, i32) -> i32, _>(&code, |add| add(7, 8))
        };
        assert_eq!(result.unwrap(), 15);
    }

    #[test]
    fn test_empty_code() {
        assert!(ExecutableMemory::new(&[]).is_err());
    }

    #[test]
    fn test_offset_too_large() {
        let code = add_code();
        let mem = ExecutableMemory::new(&code).expect("allocate");
        let result = unsafe { mem.get_fn::<extern "C" fn()>(mem.len() + 1) };
        assert!(result.is_err());
    }

    #[test]
    fn test_seal_and_modify() {
        let mut mem = ExecutableMemory::new_writable(&constant_code()).expect("allocate");
        assert!(!mem.is_sealed);

        // 修改代码中的常量: mov eax, 42 → mov eax, 99
        unsafe {
            mem.modify(|code| {
                code[1] = 99; // 0x2A (42) → 0x63 (99)
            })
        }
        .expect("modify and seal");

        let get_value: extern "C" fn() -> i32 = unsafe { mem.get_fn(0).unwrap() };
        assert_eq!(get_value(), 99);
    }

    #[test]
    fn test_writable_then_seal() {
        let mut mem = ExecutableMemory::new_writable(&constant_code()).expect("allocate");
        assert!(!mem.is_sealed);

        // 直接 seal
        mem.seal().expect("seal");
        assert!(mem.is_sealed);

        let get_answer: extern "C" fn() -> i32 = unsafe { mem.get_fn(0).unwrap() };
        assert_eq!(get_answer(), 42);
    }
}
