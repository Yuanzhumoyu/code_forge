//! RISC-V64 反汇编测试（disasm! 宏 + 从根 tests/riscv64_disasm_tests.rs 迁移的用例）。

#![cfg(test)]

use code_forge::backend::arch::riscv64::*;
use code_forge::backend::machine::target::TargetMachine as _;
use code_forge::prelude::VReg;

fn r(n: u32) -> VReg {
    VReg(n)
}
fn disasm(inst: &Inst) -> String {
    let tm = TargetMachine::new();
    tm.disassembler().unwrap().disassemble(inst)
}

#[test]
fn test_disasm_nop() {
    assert_eq!(disasm(&Inst::Nop), "nop");
}
#[test]
fn test_disasm_add() {
    let s = disasm(&Inst::Add {
        dest: r(0),
        src1: r(1),
        src2: r(2),
    });
    assert!(s.contains("add"), "got: {s}");
}
#[test]
fn test_disasm_sub() {
    let s = disasm(&Inst::Sub {
        dest: r(0),
        src1: r(1),
        src2: r(2),
    });
    assert!(s.contains("sub"), "got: {s}");
}
#[test]
fn test_disasm_all_no_panic() {
    let tm = TargetMachine::new();
    let d = tm.disassembler().unwrap();
    let insts = [
        Inst::Nop,
        Inst::Add {
            dest: r(0),
            src1: r(1),
            src2: r(2),
        },
    ];
    for inst in &insts {
        assert!(!d.disassemble(inst).is_empty());
    }
}
