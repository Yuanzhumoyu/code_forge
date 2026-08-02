//! x86_64 控制流指令测试（test-control feature 门控）。
//! 从根 crate `tests/jit_integration.rs` 迁移的代表性控制流测试。

#![cfg(test)]

use code_forge::backend::arch::x86_64::ensure_registered;
use code_forge::prelude::*;

/// 无参返回 i32：统一 harness（`exec::harness::run_i32`）。
fn run(name: &str, build: fn(&mut FunctionBuilder) -> Value) -> i32 {
    ensure_registered();
    crate::exec::harness::run_i32(name, build)
}

#[test]
fn control_icmp_eq() {
    let r = run("control_icmp_eq", |b| {
        let a = b.iconst_i32(42);
        let bv = b.iconst_i32(42);
        b.icmp(code_forge::ir::IntCC::Equal, a, bv)
    });
    assert_eq!(r, 1);
}

#[test]
fn control_icmp_sgt() {
    let r = run("control_icmp_sgt", |b| {
        let a = b.iconst_i32(10);
        let bv = b.iconst_i32(5);
        b.icmp(code_forge::ir::IntCC::SignedGreaterThan, a, bv)
    });
    assert_eq!(r, 1);
}

#[test]
fn control_select() {
    let r = run("control_select", |b| {
        let cond = b.iconst_i32(1);
        let t = b.iconst_i32(42);
        let f = b.iconst_i32(0);
        b.select(cond, t, f)
    });
    assert_eq!(r, 42);
}
