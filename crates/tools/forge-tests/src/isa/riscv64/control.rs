//! riscv64 控制流指令测试（test-control feature 门控）— compile-only。
//!
//! riscv64 无本机执行；此处验证 icmp/select/分支组合能编译成功，
//! 执行语义经 `exec-unicorn` 验证。

#![cfg(test)]

use code_forge::prelude::*;

/// compile-only 断言（riscv64 TargetMachine）。
fn compile_ok(name: &str, build: fn(&mut FunctionBuilder) -> Value) {
    crate::exec::harness::compile_ok(
        name,
        code_forge::backend::riscv64::TargetMachine::new,
        TypeId::I32,
        build,
    );
}

/// icmp 相等。
#[test]
fn control_icmp_eq() {
    compile_ok("riscv64_control_icmp_eq", |b| {
        let a = b.iconst_i32(42);
        let bv = b.iconst_i32(42);
        b.icmp(code_forge::ir::IntCC::Equal, a, bv)
    });
}

/// icmp + select 三目。
#[test]
fn control_icmp_select() {
    compile_ok("riscv64_control_icmp_select", |b| {
        let a = b.iconst_i32(10);
        let bv = b.iconst_i32(5);
        let c = b.icmp(code_forge::ir::IntCC::SignedGreaterThan, a, bv);
        let t = b.iconst_i32(42);
        let f = b.iconst_i32(0);
        b.select(c, t, f)
    });
}

/// 双块 + branch 分支结构（if 语义）。
#[test]
fn control_two_blocks() {
    compile_ok("riscv64_control_two_blocks", |b| {
        let a = b.iconst_i32(1);
        let then_blk = b.create_block();
        let else_blk = b.create_block();
        let merge = b.create_block();
        let zero = b.iconst_i32(0);
        b.branch(a, then_blk, &[], else_blk, &[]);
        b.switch_to_block(then_blk);
        let t = b.iconst_i32(42);
        b.jump(merge, &[]);
        b.switch_to_block(else_blk);
        let f = b.iconst_i32(0);
        b.jump(merge, &[]);
        b.switch_to_block(merge);
        let _ = (zero, t, f);
        b.iconst_i32(0)
    });
}
