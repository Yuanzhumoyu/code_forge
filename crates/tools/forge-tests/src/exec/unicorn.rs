//! Unicorn 跨架构执行引擎（exec-unicorn feature）——基于手写 FFI 绑定。
//!
//! 把 forge-codegen 的 JIT 裸机器码（`CompiledFunction.code: Vec<u8>`）
//! 写入 Unicorn 模拟器的代码页执行，执行完成后读取返回值寄存器：
//! - x86_64: RAX
//! - aarch64: X0
//! - riscv64: X10
//!
//! 构建：build.rs 用 cmake 编译 vendored unicorn C 源码（vendor/unicorn）
//! 为静态库——只需 cmake + C 编译器，无需 libclang / pkg-config / 系统 unicorn /
//! 预编译 DLL。换环境（另一台机器）只需有 cmake + C 编译器即可。

use super::unicorn_ffi::{Arch, Arm64Reg, Mode, Prot, RiscvReg, Unicorn, X86Reg};
use code_forge::backend::CompiledFunction;

/// 代码页基址（4KB 对齐；unicorn 官方示例用 0x1000）。
pub const CODE_BASE: u64 = 0x1000;
/// 栈页基址。
pub const STACK_BASE: u64 = 0x8000_0000;
/// 页大小。
pub const PAGE_SIZE: u64 = 0x4000;

/// 在 Unicorn 模拟器中执行裸机器码并读取返回值。
///
/// `arch` 决定架构与返回值寄存器：
/// - `"x86_64"` → RAX
/// - `"aarch64"` → X0
/// - `"riscv64"` → X10
pub fn exec_code(arch: &str, code: &[u8], args: &[u64]) -> u64 {
    match arch {
        "x86_64" => exec_x86(code, args),
        "aarch64" => exec_aarch64(code, args),
        "riscv64" => exec_riscv64(code, args),
        other => panic!("unicorn exec: unsupported arch {other}"),
    }
}

fn exec_x86(code: &[u8], args: &[u64]) -> u64 {
    let mut uc = Unicorn::new(Arch::X86, Mode::Bits64).expect("unicorn x86 init");
    uc.mem_map(CODE_BASE, PAGE_SIZE, Prot::ALL)
        .expect("map code page");
    if let Some(&a) = args.first() {
        uc.reg_write(X86Reg::Rcx, a).expect("write rcx");
    }
    if let Some(&a) = args.get(1) {
        uc.reg_write(X86Reg::Rdx, a).expect("write rdx");
    }
    uc.mem_write(CODE_BASE, code).expect("write code");
    // until 在 ret 前停（unicorn 的 RSP reg_write 在此构建下不稳）
    let until = CODE_BASE + code.len().saturating_sub(1) as u64;
    uc.emu_start(CODE_BASE, until, 10_000_000_000, 0)
        .expect("emu_start");
    uc.reg_read(X86Reg::Rax).expect("read rax")
}

fn exec_aarch64(code: &[u8], args: &[u64]) -> u64 {
    let mut uc = Unicorn::new(Arch::Arm64, Mode::LittleEndian).expect("unicorn aarch64 init");
    uc.mem_map(CODE_BASE, PAGE_SIZE, Prot::ALL)
        .expect("map code page");
    uc.mem_map(STACK_BASE, PAGE_SIZE, Prot::ALL)
        .expect("map stack page");
    if let Some(&a) = args.first() {
        uc.reg_write(Arm64Reg::X0, a).expect("write x0");
    }
    if let Some(&a) = args.get(1) {
        uc.reg_write(Arm64Reg::X1, a).expect("write x1");
    }
    // SP 指向栈页（prologue 的 stp [sp, -16]! 需要）
    let sp = STACK_BASE + PAGE_SIZE - 0x100;
    let _ = uc.reg_write(Arm64Reg::Sp, sp);
    // LR（x30）指向 until——函数的 ret 跳回这里即停止（返回值已就绪）
    let until = CODE_BASE + code.len().saturating_sub(4) as u64;
    let _ = uc.reg_write(Arm64Reg::X30, until);
    uc.mem_write(CODE_BASE, code).expect("write code");
    uc.emu_start(CODE_BASE, until, 10_000_000_000, 0)
        .expect("emu_start");
    uc.reg_read(Arm64Reg::X0).expect("read x0")
}

fn exec_riscv64(code: &[u8], args: &[u64]) -> u64 {
    let mut uc = Unicorn::new(Arch::Riscv, Mode::Bits64).expect("unicorn riscv init");
    uc.mem_map(CODE_BASE, PAGE_SIZE, Prot::ALL)
        .expect("map code page");
    uc.mem_map(STACK_BASE, PAGE_SIZE, Prot::ALL)
        .expect("map stack page");
    if let Some(&a) = args.first() {
        uc.reg_write(RiscvReg::X10, a).expect("write a0");
    }
    if let Some(&a) = args.get(1) {
        uc.reg_write(RiscvReg::X11, a).expect("write a1");
    }
    let sp = STACK_BASE + PAGE_SIZE - 0x100;
    let _ = uc.reg_write(RiscvReg::X2, sp);
    // ra（x1）指向 until——函数的 ret（jalr ra）跳回这里即停止。
    // until 取最后一条指令起始（riscv 指令对齐；ret 为 4 字节）。
    let until = CODE_BASE + code.len().saturating_sub(4) as u64;
    let _ = uc.reg_write(RiscvReg::X1, until);
    uc.mem_write(CODE_BASE, code).expect("write code");
    uc.emu_start(CODE_BASE, until, 10_000_000_000, 0)
        .expect("emu_start");
    uc.reg_read(RiscvReg::X10).expect("read a0")
}

/// 便捷入口：编译产物 → unicorn 执行 → 返回值。
pub fn run_compiled(arch: &str, compiled: &CompiledFunction, args: &[u64]) -> u64 {
    exec_code(arch, &compiled.code, args)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// x86_64: mov eax, 0x2a; ret（0xB8 2A 00 00 00 C3）→ RAX = 42
    #[test]
    fn x86_mov_imm_ret() {
        let code = [0xB8u8, 0x2A, 0x00, 0x00, 0x00, 0xC3];
        let r = exec_code("x86_64", &code, &[]);
        assert_eq!(r, 42);
    }

    /// aarch64: mov x0, #42; ret（D2 80 05 40 C0 03 5F D6）
    #[test]
    fn aarch64_mov_imm_ret() {
        let code = [0x40u8, 0x05, 0x80, 0xD2, 0xC0, 0x03, 0x5F, 0xD6];
        let r = exec_code("aarch64", &code, &[]);
        assert_eq!(r, 42);
    }

    /// 实验：带 prologue（stp 写栈）的手写 aarch64 函数。
    #[test]
    fn aarch64_prologue_mov_ret() {
        // stp x29, x30, [sp, #-16]!     = a9 bf 7b fd
        // mov x0, #42                    = d2 80 05 40
        // ldp x29, x30, [sp], #16        = a8 c1 7b fd
        // ret                            = c0 03 5f d6
        let code = [
            0xFD, 0x7B, 0xBF, 0xA9, // stp x29, x30, [sp, #-16]!
            0x40, 0x05, 0x80, 0xD2, // mov x0, #42
            0xFD, 0x7B, 0xC1, 0xA8, // ldp x29, x30, [sp], #16
            0xC0, 0x03, 0x5F, 0xD6, // ret
        ];
        let mut uc = Unicorn::new(Arch::Arm64, Mode::LittleEndian).expect("unicorn init");
        uc.mem_map(CODE_BASE, PAGE_SIZE, Prot::ALL)
            .expect("map code");
        uc.mem_map(STACK_BASE, PAGE_SIZE, Prot::ALL)
            .expect("map stack");
        let sp = STACK_BASE + PAGE_SIZE - 0x100;
        let until = CODE_BASE + code.len().saturating_sub(4) as u64;
        let _ = uc.reg_write(Arm64Reg::Sp, sp);
        let _ = uc.reg_write(Arm64Reg::X30, until);
        uc.mem_write(CODE_BASE, &code).expect("write code");
        uc.emu_start(CODE_BASE, until, 10_000_000_000, 0)
            .expect("emu_start");
        let r = uc.reg_read(Arm64Reg::X0).expect("read x0");
        assert_eq!(r, 42);
    }

    /// 寄存器 id 绑定回归：写入再读回应一致（验证 unicorn_ffi 绑定值正确性）。
    #[test]
    fn reg_id_roundtrip() {
        let mut uc_a = Unicorn::new(Arch::Arm64, Mode::LittleEndian).expect("aarch64 init");
        for (reg, val) in [
            (Arm64Reg::X0, 0x1234u64),
            (Arm64Reg::X1, 0x5678u64),
            (Arm64Reg::X30, 0x9999u64),
            (Arm64Reg::Sp, 0x8000_0000u64),
        ] {
            uc_a.reg_write(reg, val).expect("write");
            let r = uc_a.reg_read(reg).expect("read");
            assert_eq!(r, val, "arm64 {reg:?} id roundtrip");
        }

        let mut uc_r = Unicorn::new(Arch::Riscv, Mode::Bits64).expect("riscv init");
        for (reg, val) in [
            (RiscvReg::X2, 0x8000_0000u64),
            (RiscvReg::X10, 0x1111u64),
            (RiscvReg::X11, 0x2222u64),
        ] {
            uc_r.reg_write(reg, val).expect("write");
            let r = uc_r.reg_read(reg).expect("read");
            assert_eq!(r, val, "riscv {reg:?} id roundtrip");
        }
    }
}
