//! AArch64 反汇编测试（disasm! 宏 + 从根 tests/aarch64_disasm_tests.rs 迁移的用例）。

#![cfg(test)]

use code_forge::backend::arch::aarch64::*;
use code_forge::backend::machine::target::TargetMachine as TargetMachineTrait;
use code_forge::prelude::VReg;

fn r(n: u32) -> VReg {
    VReg(n)
}

fn disasm(inst: &Inst) -> String {
    let tm = TargetMachine::new();
    tm.disassembler().unwrap().disassemble(inst)
}

// ═══════════════════════════════════════════════════
// No-operand instructions
// ═══════════════════════════════════════════════════

#[test]
fn test_disasm_ret() {
    assert_eq!(disasm(&Inst::SdRet), "ret");
}

#[test]
fn test_disasm_nop() {
    assert_eq!(disasm(&Inst::SdNop), "nop");
}

#[test]
fn test_disasm_ud2() {
    assert_eq!(disasm(&Inst::SdUd2), "brk #0");
}

// ═══════════════════════════════════════════════════
// Register-register
// ═══════════════════════════════════════════════════

#[test]
fn test_disasm_mov() {
    let s = disasm(&Inst::SdMov {
        dest: r(0),
        src: r(1),
    });
    assert!(s.contains("mov"), "got: {s}");
}

#[test]
fn test_disasm_add() {
    let s = disasm(&Inst::SdAdd {
        dest: r(0),
        src1: r(1),
        src2: r(2),
    });
    assert!(s.contains("add"), "got: {s}");
}

#[test]
fn test_disasm_sub() {
    let s = disasm(&Inst::SdSub {
        dest: r(0),
        src1: r(1),
        src2: r(2),
    });
    assert!(s.contains("sub"), "got: {s}");
}

#[test]
fn test_disasm_mov_imm() {
    let s = disasm(&Inst::SdMovImm {
        dest: r(0),
        imm: 42,
    });
    assert!(s.contains("mov"), "got: {s}");
}

#[test]
fn test_disasm_mul() {
    let s = disasm(&Inst::SdMul {
        dest: r(0),
        src1: r(1),
        src2: r(2),
    });
    assert!(s.contains("mul"), "got: {s}");
}

#[test]
fn test_disasm_all_insts_no_panic() {
    let tm = TargetMachine::new();
    let d = tm.disassembler().unwrap();
    let insts: Vec<Inst> = vec![
        Inst::SdMov {
            dest: r(0),
            src: r(1),
        },
        Inst::SdMovImm {
            dest: r(0),
            imm: 42,
        },
        Inst::SdAdd {
            dest: r(0),
            src1: r(1),
            src2: r(2),
        },
        Inst::SdSub {
            dest: r(0),
            src1: r(1),
            src2: r(2),
        },
        Inst::SdMul {
            dest: r(0),
            src1: r(1),
            src2: r(2),
        },
        Inst::SdAnd {
            dest: r(0),
            src1: r(1),
            src2: r(2),
        },
        Inst::SdOr {
            dest: r(0),
            src1: r(1),
            src2: r(2),
        },
        Inst::SdXor {
            dest: r(0),
            src1: r(1),
            src2: r(2),
        },
        Inst::SdRet,
        Inst::SdNop,
        Inst::SdUd2,
    ];
    for inst in &insts {
        let s = d.disassemble(inst);
        assert!(!s.is_empty(), "disasm should produce non-empty string");
    }
}
