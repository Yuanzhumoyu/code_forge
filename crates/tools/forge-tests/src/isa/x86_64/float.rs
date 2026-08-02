//! x86_64 浮点指令测试（test-float feature 门控）。
//! 从根 crate `tests/jit_integration.rs` 迁移的代表性浮点测试。

#![cfg(test)]

use code_forge::backend::arch::x86_64::ensure_registered;
use code_forge::prelude::*;

/// 无参返回 f64：统一 harness（`exec::harness::run_f64`）。
fn run_f64(name: &str, build: fn(&mut FunctionBuilder) -> Value) -> f64 {
    ensure_registered();
    crate::exec::harness::run_f64(name, build)
}



#[test]
fn float_fadd() {
    let r = run_f64("float_fadd", |b| {
        let a = b.fconst_f64(20.5);
        let bv = b.fconst_f64(21.5);
        b.fadd(a, bv)
    });
    assert!((r - 42.0).abs() < 0.001, "fadd: got {r}, expected 42.0");
}

#[test]
fn float_fmul() {
    let r = run_f64("float_fmul", |b| {
        let a = b.fconst_f64(6.0);
        let bv = b.fconst_f64(7.0);
        b.fmul(a, bv)
    });
    assert!((r - 42.0).abs() < 0.001, "fmul: got {r}, expected 42.0");
}

#[test]
fn float_fsub() {
    let r = run_f64("float_fsub", |b| {
        let a = b.fconst_f64(50.0);
        let bv = b.fconst_f64(8.0);
        b.fsub(a, bv)
    });
    assert!((r - 42.0).abs() < 0.001, "fsub: got {r}, expected 42.0");
}

#[test]
fn float_fabs() {
    let r = run_f64("float_fabs", |b| {
        let a = b.fconst_f64(-42.0);
        b.fabs(a)
    });
    assert!((r - 42.0).abs() < 0.001, "fabs: got {r}, expected 42.0");
}
