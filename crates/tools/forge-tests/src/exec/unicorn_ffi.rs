//! 手写 unicorn FFI 绑定（替代 unicorn-engine-sys 的 bindgen 生成）。
//!
//! 只绑定 execute-from-buffer 执行测试所需的少量接口（uc_open / uc_mem_map /
//! uc_mem_write / uc_mem_read / uc_emu_start / uc_reg_read / uc_close）与
//! 寄存器/模式常量。无需 libclang（bindgen）、pkg-config 或系统 unicorn——
//! 静态链接 vendor/unicorn 由 build.rs 用 cmake 编译。

#![allow(non_upper_case_globals, non_camel_case_types, dead_code)]

use std::os::raw::{c_char, c_int, c_uchar, c_void};

type uc_handle = *mut c_void;
pub const UC_ERR_OK: c_int = 0;

/// 目标架构（unicorn/unicorn.h 的 enum uc_arch）。
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Arch {
    Arm,
    Arm64,
    Mips,
    X86,
    Ppc,
    Sparc,
    M68k,
    Riscv,
}

impl Arch {
    pub fn code(self) -> c_int {
        match self {
            Arch::Arm => 1,
            Arch::Arm64 => 2,
            Arch::Mips => 3,
            Arch::X86 => 4,
            Arch::Ppc => 5,
            Arch::Sparc => 6,
            Arch::M68k => 7,
            Arch::Riscv => 8,
        }
    }
}

/// 模拟模式（unicorn/unicorn.h 的 UC_MODE_*）。
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Mode {
    LittleEndian,
    BigEndian,
    Bits16,
    Bits32,
    Bits64,
}

impl Mode {
    pub fn code(self) -> c_int {
        match self {
            Mode::LittleEndian => 0,
            Mode::BigEndian => 1 << 30,
            Mode::Bits16 => 1 << 1,
            Mode::Bits32 => 1 << 2,
            Mode::Bits64 => 1 << 3,
        }
    }
}

/// 内存页权限（unicorn/unicorn.h 的 UC_PROT_* 的组合）。
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct Prot(pub(crate) c_int);

impl Prot {
    pub const NONE: Prot = Prot(0);
    pub const READ: Prot = Prot(1);
    pub const WRITE: Prot = Prot(2);
    pub const EXEC: Prot = Prot(4);
    pub const ALL: Prot = Prot(1 | 2 | 4);
}

/// x86 寄存器（unicorn/x86.h 的 enum uc_x86_reg 序号）。
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum X86Reg {
    Rax,
    Rcx,
    Rdx,
    Rsp,
}

impl X86Reg {
    pub fn code(self) -> c_int {
        match self {
            X86Reg::Rax => 35,
            X86Reg::Rcx => 38,
            X86Reg::Rdx => 40,
            X86Reg::Rsp => 44,
        }
    }
}

/// ARM64 寄存器（unicorn/arm64.h 的 enum uc_arm64_reg 序号）。
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Arm64Reg {
    X0,
    X1,
    X30,
    Sp,
}

impl Arm64Reg {
    pub fn code(self) -> c_int {
        match self {
            // 枚举序号：INVALID=0, X29=1, X30=2, NZCV=3, SP=4, WSP=5, ... X0=199, X1=200
            Arm64Reg::Sp => 4,
            Arm64Reg::X0 => 199,
            Arm64Reg::X1 => 200,
            Arm64Reg::X30 => 2,
        }
    }
}

/// RISC-V 寄存器（unicorn/riscv.h 的 enum uc_riscv_reg 序号；A0-A1 = X10-X11）。
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum RiscvReg {
    X1,
    X2,
    X10,
    X11,
}

impl RiscvReg {
    pub fn code(self) -> c_int {
        match self {
            // 枚举序号：INVALID=0, X0=1, X1(ra)=2, X2(sp)=3, ... X10(a0)=11, X11(a1)=12
            RiscvReg::X1 => 2,
            RiscvReg::X2 => 3,
            RiscvReg::X10 => 11,
            RiscvReg::X11 => 12,
        }
    }
}

/// 内存页权限（unicorn/unicorn.h 的 UC_PROT_*）。
const UC_PROT_NONE: c_int = 0;
const UC_PROT_READ: c_int = 1;
const UC_PROT_WRITE: c_int = 2;
const UC_PROT_EXEC: c_int = 4;
const UC_PROT_ALL: c_int = UC_PROT_READ | UC_PROT_WRITE | UC_PROT_EXEC;

unsafe extern "C" {
    fn uc_open(arch: c_int, mode: c_int, uc: *mut uc_handle) -> c_int;
    fn uc_close(uc: uc_handle) -> c_int;
    fn uc_mem_map(uc: uc_handle, address: u64, size: u64, perms: c_int) -> c_int;
    fn uc_mem_write(uc: uc_handle, address: u64, bytes: *const c_uchar, size: usize) -> c_int;
    fn uc_mem_read(uc: uc_handle, address: u64, bytes: *mut c_uchar, size: usize) -> c_int;
    fn uc_emu_start(uc: uc_handle, begin: u64, until: u64, timeout: u64, count: usize) -> c_int;
    fn uc_reg_read(uc: uc_handle, regid: c_int, value: *mut u64) -> c_int;
    fn uc_reg_write(uc: uc_handle, regid: c_int, value: *const u64) -> c_int;
    fn uc_strerror(err: c_int) -> *const c_char;
}

/// 便捷错误包装：把 FFI 返回值转成 Result。
pub(crate) fn check(err: c_int) -> Result<(), String> {
    if err == UC_ERR_OK {
        Ok(())
    } else {
        let msg = unsafe {
            let p = uc_strerror(err);
            if p.is_null() {
                format!("unicorn error {err}")
            } else {
                std::ffi::CStr::from_ptr(p).to_string_lossy().into_owned()
            }
        };
        Err(msg)
    }
}

/// Safe Rust API — Unicorn 模拟器的对象式封装。
///
/// 使用方式：
/// ```no_run
/// use forge_tests::exec::unicorn_ffi::{Arch, Mode, Prot, Unicorn, X86Reg};
/// # let code: Vec<u8> = vec![0xC3];
/// let mut uc = Unicorn::new(Arch::X86, Mode::Bits64)?;
/// uc.mem_map(0x1000, 0x4000, Prot::ALL)?;
/// uc.mem_write(0x1000, &code)?;
/// uc.emu_start(0x1000, 0x1000 + code.len() as u64, 0, 0)?;
/// let rax = uc.reg_read(X86Reg::Rax)?;
/// # Ok::<(), String>(())
/// ```
pub struct Unicorn {
    handle: uc_handle,
}

/// 寄存器枚举 → 寄存器 id（供 reg_read/reg_write 使用）。
pub trait RegId {
    fn id(self) -> c_int;
}

impl RegId for X86Reg {
    fn id(self) -> c_int {
        self.code()
    }
}
impl RegId for Arm64Reg {
    fn id(self) -> c_int {
        self.code()
    }
}
impl RegId for RiscvReg {
    fn id(self) -> c_int {
        self.code()
    }
}

impl Unicorn {
    /// 打开指定架构/模式的模拟器。
    pub fn new(arch: Arch, mode: Mode) -> Result<Self, String> {
        let mut handle: uc_handle = std::ptr::null_mut();
        unsafe {
            check(uc_open(arch.code(), mode.code(), &mut handle))?;
        }
        if handle.is_null() {
            return Err("uc_open returned null handle".into());
        }
        Ok(Self { handle })
    }

    /// 映射一页内存（4KB 对齐）。
    pub fn mem_map(&mut self, address: u64, size: u64, perms: Prot) -> Result<(), String> {
        unsafe { check(uc_mem_map(self.handle, address, size, perms.0)) }
    }

    /// 向内存写入字节。
    pub fn mem_write(&mut self, address: u64, bytes: &[u8]) -> Result<(), String> {
        unsafe {
            check(uc_mem_write(
                self.handle,
                address,
                bytes.as_ptr(),
                bytes.len(),
            ))
        }
    }

    /// 从内存读取字节。
    pub fn mem_read(&mut self, address: u64, bytes: &mut [u8]) -> Result<(), String> {
        unsafe {
            check(uc_mem_read(
                self.handle,
                address,
                bytes.as_mut_ptr(),
                bytes.len(),
            ))
        }
    }

    /// 开始模拟执行。
    pub fn emu_start(
        &mut self,
        begin: u64,
        until: u64,
        timeout: u64,
        count: usize,
    ) -> Result<(), String> {
        unsafe { check(uc_emu_start(self.handle, begin, until, timeout, count)) }
    }

    /// 读寄存器。
    pub fn reg_read(&self, reg: impl RegId) -> Result<u64, String> {
        let mut val: u64 = 0;
        unsafe {
            check(uc_reg_read(self.handle, reg.id(), &mut val))?;
        }
        Ok(val)
    }

    /// 写寄存器。
    pub fn reg_write(&mut self, reg: impl RegId, value: u64) -> Result<(), String> {
        unsafe { check(uc_reg_write(self.handle, reg.id(), &value)) }
    }
}

impl Drop for Unicorn {
    fn drop(&mut self) {
        unsafe {
            uc_close(self.handle);
        }
    }
}
