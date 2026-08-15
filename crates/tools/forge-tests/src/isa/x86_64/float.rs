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

#[test]
fn test_fpr_spill_20_values() {
    use code_forge::backend::FunctionCompiler;
    use code_forge::backend::arch::x86_64::{TargetMachine, ensure_registered};
    use code_forge::prelude::*;
    ensure_registered();
    // 20 个 fabs 结果同时活跃（超 XMM0-15 池 → 触发 FPR spill，
    // 覆盖 MOVSD_RM/MOVSD_MR 的 XMM≥8 scratch 编码路径）
    let func = crate::exec::harness::build_func("fpr_spill_20", TypeId::F64, |b| {
        let consts: Vec<Value> = (1..=20).map(|i| b.fconst_f64(-(i as f64))).collect();
        let vals: Vec<Value> = consts.iter().map(|c| b.fabs(*c)).collect();
        let mut acc = b.fconst_f64(0.0);
        for v in &vals {
            acc = b.fadd(acc, *v);
        }
        acc
    });
    let compiled = FunctionCompiler::new(TargetMachine::new())
        .compile_raw(&func)
        .unwrap();
    let mem = code_forge::mem::ExecutableMemory::new(&compiled.code).unwrap();
    let f: extern "C" fn() -> f64 = unsafe { mem.get_fn(0).unwrap() };
    let res = f();
    // Σ|−i| for i in 1..=20 = 210.0
    assert!(
        (res - 210.0).abs() < 1e-9,
        "fpr spill 求和结果错误: {res} (期望 210.0)"
    );
    // 关键断言：FPR spill 的 movsd load 编码必须带 F2 前缀 + REX.R（XMM≥8）——
    // 曾缺失 F2 → movups 垃圾字节（F2 44 0F 10 = movsd xmm10+, [mem]）
    let code = compiled.code;
    assert!(
        code.windows(4).any(|w| w == [0xF2, 0x44, 0x0F, 0x10]),
        "缺少 F2 44 0F 10 (movsd xmm≥8 load) 编码: {}",
        code.iter()
            .map(|b| format!("{b:02X}"))
            .collect::<Vec<_>>()
            .join(" ")
    );
}
