//! x86_64 反汇编测试（disasm! 宏 + 从根 tests/disasm_tests.rs 迁移的用例）。

#![cfg(test)]

use code_forge::backend::arch::x86_64::*;
use code_forge::backend::machine::target::TargetMachine as _;
use code_forge::ir::RegClass;

fn r(n: u32) -> Reg {
    <Reg as code_forge::ir::PhysReg>::from_index(n as u8, RegClass::Int)
}
fn disasm(inst: &Inst) -> String {
    TargetMachine::new()
        .disassembler()
        .unwrap()
        .disassemble(inst)
}

#[test]
fn test_disasm_ret() {
    assert_eq!(disasm(&Inst::Ret), "ret");
}
#[test]
fn test_disasm_nop() {
    assert_eq!(disasm(&Inst::Nop), "nop");
}
#[test]
fn test_disasm_mov_rr() {
    let s = disasm(&Inst::MovRRm {
        dest: r(0),
        src: r(1),
        opsize: 8,
    });
    assert!(s.contains("mov"), "got: {s}");
}
#[test]
fn test_disasm_add_rr() {
    let s = disasm(&Inst::AddRmR {
        dest: r(0),
        src: r(1),
        opsize: 8,
    });
    assert!(s.contains("add"), "got: {s}");
}
#[test]
fn test_disasm_sub_rr() {
    let s = disasm(&Inst::SubRmR {
        dest: r(0),
        src: r(1),
        opsize: 8,
    });
    assert!(s.contains("sub"), "got: {s}");
}
#[test]
fn test_disasm_mov_imm() {
    let s = disasm(&Inst::MovRegImm64 { reg: r(0), imm: 42 });
    assert!(s.contains("mov"), "got: {s}");
}
#[test]
fn test_disasm_all_no_panic() {
    let tm = TargetMachine::new();
    let d = tm.disassembler().unwrap();
    let insts = [
        Inst::Ret,
        Inst::Nop,
        Inst::MovRRm {
            dest: r(0),
            src: r(1),
            opsize: 8,
        },
        Inst::AddRmR {
            dest: r(0),
            src: r(1),
            opsize: 8,
        },
        Inst::SubRmR {
            dest: r(0),
            src: r(1),
            opsize: 8,
        },
    ];
    for inst in &insts {
        assert!(!d.disassemble(inst).is_empty());
    }
}
