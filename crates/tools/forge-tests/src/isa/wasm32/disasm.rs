//! WASM32 反汇编测试（disasm! 宏 + 从根 tests/wasm32_disasm_tests.rs 迁移的用例）。

#![cfg(test)]

use code_forge::backend::arch::wasm32::*;
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
    assert_eq!(disasm(&Inst::SdNop), "nop");
}
#[test]
fn test_disasm_ret() {
    assert_eq!(disasm(&Inst::SdRet), "return");
}
#[test]
fn test_disasm_add() {
    let s = disasm(&Inst::SdAdd {
        dest: r(0),
        src: r(1),
    });
    assert!(s.contains("add"), "got: {s}");
}
#[test]
fn test_disasm_all_no_panic() {
    let tm = TargetMachine::new();
    let d = tm.disassembler().unwrap();
    let insts = [Inst::SdNop, Inst::SdRet];
    for inst in &insts {
        assert!(!d.disassemble(inst).is_empty());
    }
}
