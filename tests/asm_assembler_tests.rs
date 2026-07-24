//! 汇编器集成测试 — 验证 ASM 文本 → 解析 → 编码 → JIT 执行的完整管线。

use codegen_lib::backend::x86_64::X86Isa;
use codegen_lib::backend::Assembler;

// ============================================================
// parse_insts 测试 — 解析正确性
// ============================================================

#[test]
fn test_parse_ret() {
    let insts = X86Isa::parse_insts("ret").unwrap();
    assert!(!insts.is_empty());
}

#[test]
fn test_parse_push_reg() {
    let insts = X86Isa::parse_insts("push RBP").unwrap();
    assert!(!insts.is_empty());
}

#[test]
fn test_parse_mov_reg_imm() {
    let insts = X86Isa::parse_insts("mov_imm RAX, 42").unwrap();
    assert!(!insts.is_empty());
}

#[test]
fn test_parse_mov_reg_reg() {
    let insts = X86Isa::parse_insts("mov RAX, RBX").unwrap();
    assert!(!insts.is_empty());
}

#[test]
fn test_parse_add_reg_reg() {
    let insts = X86Isa::parse_insts("add RAX, RBX").unwrap();
    assert!(!insts.is_empty());
}

#[test]
fn test_parse_multiple_insts() {
    let insts = X86Isa::parse_insts("push RBP\nmov_imm RAX, 42\nret").unwrap();
    assert!(insts.len() >= 1, "expected at least 1 instruction, got {}", insts.len());
}

// ============================================================
// assemble 测试 — 完整编码
// ============================================================

#[test]
fn test_assemble_ret() {
    let jit = X86Isa::assemble("ret_only", "ret").unwrap();
    assert!(jit.len() > 0);
}

#[test]
fn test_assemble_push_ret() {
    let jit = X86Isa::assemble("push_ret", "push RBP\nret").unwrap();
    assert!(jit.len() > 0);
}

// ============================================================
// assemble + JIT 执行测试 (仅 x86_64)
// ============================================================

#[cfg(target_arch = "x86_64")]
#[test]
fn test_assemble_and_exec_ret_42() {
    let jit = X86Isa::assemble("ret42",
        "mov_imm RAX, 42\nret"
    ).unwrap();
    let f: extern "C" fn() -> i64 = jit.get_fn("ret42").unwrap();
    assert_eq!(f(), 42);
}

#[cfg(target_arch = "x86_64")]
#[test]
fn test_assemble_and_exec_add() {
    let jit = X86Isa::assemble("add_20_22",
        "mov_imm RAX, 20\nmov_imm RCX, 22\nadd RAX, RCX\nret"
    ).unwrap();
    let f: extern "C" fn() -> i64 = jit.get_fn("add_20_22").unwrap();
    assert_eq!(f(), 42);
}

#[cfg(target_arch = "x86_64")]
#[test]
fn test_assemble_and_exec_sub() {
    let jit = X86Isa::assemble("sub_84_42",
        "mov_imm RAX, 84\nmov_imm RCX, 42\nsub RAX, RCX\nret"
    ).unwrap();
    let f: extern "C" fn() -> i64 = jit.get_fn("sub_84_42").unwrap();
    assert_eq!(f(), 42);
}
