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

/// 构建 fcmp(cond, a, b) 并 JIT 执行，返回 I1 扩展为 I64 的结果。
fn run_fcmp_i64(name: &str, cond: FloatCC, a: f64, b: f64) -> i64 {
    let sig = FunctionSignature::new(&[], &[TypeId::I64]);
    let mut fb = FunctionBuilder::new(name, TypeContext::new(), sig);
    fb.create_block_here();
    let av = fb.fconst_f64(a);
    let bv = fb.fconst_f64(b);
    let c = fb.fcmp(cond, av, bv);
    let ext = fb.uextend(c, TypeId::I64);
    fb.ret(&[ext]);
    let func = fb.finish().expect("build");
    let compiled = crate::exec::harness::compile_x86_64(name, &func);
    assert!(!compiled.code.is_empty(), "{name}: empty code");
    let mem =
        code_forge::mem::ExecutableMemory::new(&compiled.code).expect("ExecutableMemory::new");
    let f: extern "C" fn() -> i64 = unsafe { mem.get_fn(0).unwrap() };
    f()
}

/// fcmp NaN 语义回归（P1）：UCOMISS 对 NaN 置 ZF=PF=CF=1。
/// `o*` 有序条件必须排除 NaN（结果为 0）；`u*` 无序-or 条件 NaN 为真（结果为 1）。
/// 曾 bug：Equal/LessThan/LessThanOrEqual 直接 sete/setb/setbe 对 NaN 返回 1
/// （实际是 ueq/ult/ule 语义）——测试固定 NaN 输入验证修正。
#[test]
fn test_fcmp_nan_semantics() {
    let nan = f64::from_bits(0x7FF8_0000_0000_0000);
    let one = 1.0f64;

    // 有序条件：NaN 参与 → 0
    assert_eq!(run_fcmp_i64("fcmp_nan_oeq", FloatCC::Equal, nan, one), 0);
    assert_eq!(run_fcmp_i64("fcmp_nan_olt", FloatCC::LessThan, nan, one), 0);
    assert_eq!(
        run_fcmp_i64("fcmp_nan_ole", FloatCC::LessThanOrEqual, nan, one),
        0
    );
    assert_eq!(
        run_fcmp_i64("fcmp_nan_ogt", FloatCC::GreaterThan, nan, one),
        0
    );
    assert_eq!(
        run_fcmp_i64("fcmp_nan_oge", FloatCC::GreaterThanOrEqual, nan, one),
        0
    );
    assert_eq!(run_fcmp_i64("fcmp_nan_one", FloatCC::NotEqual, nan, one), 0);
    assert_eq!(run_fcmp_i64("fcmp_nan_ord", FloatCC::Ordered, nan, one), 0);

    // 无序-or 条件：NaN 参与 → 1
    assert_eq!(
        run_fcmp_i64("fcmp_nan_uno", FloatCC::Unordered, nan, one),
        1
    );
    assert_eq!(run_fcmp_i64("fcmp_nan_ueq", FloatCC::Ueq, nan, one), 1);
    assert_eq!(run_fcmp_i64("fcmp_nan_une", FloatCC::Une, nan, one), 1);
    assert_eq!(run_fcmp_i64("fcmp_nan_ult", FloatCC::Ult, nan, one), 1);
    assert_eq!(run_fcmp_i64("fcmp_nan_ule", FloatCC::Ule, nan, one), 1);
    assert_eq!(run_fcmp_i64("fcmp_nan_ugt", FloatCC::Ugt, nan, one), 1);
    assert_eq!(run_fcmp_i64("fcmp_nan_uge", FloatCC::Uge, nan, one), 1);

    // 常量条件
    assert_eq!(run_fcmp_i64("fcmp_false", FloatCC::False, nan, one), 0);
    assert_eq!(run_fcmp_i64("fcmp_true", FloatCC::True, nan, one), 1);

    // 正常值 sanity：oeq(1,1)=1、ult(0.5,1)=1、NaN vs NaN
    assert_eq!(run_fcmp_i64("fcmp_oeq_ok", FloatCC::Equal, 1.0, 1.0), 1);
    assert_eq!(run_fcmp_i64("fcmp_ult_ok", FloatCC::Ult, 0.5, 1.0), 1);
    assert_eq!(
        run_fcmp_i64("fcmp_nan_nan_oeq", FloatCC::Equal, nan, nan),
        0
    );
    assert_eq!(run_fcmp_i64("fcmp_nan_nan_ueq", FloatCC::Ueq, nan, nan), 1);
}
