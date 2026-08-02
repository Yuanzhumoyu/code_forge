//! aarch64 浮点指令测试（test-float feature 门控）— compile-only。
//!
//! aarch64 无本机执行；此处验证代表性浮点指令组合能编译成功，
//! 执行语义经 `exec-unicorn` feature 的 cross_arch_exec 验证。

#![cfg(test)]

use code_forge::prelude::*;

/// compile-only 断言（aarch64 TargetMachine，返回 f64）。
fn compile_ok_f64(name: &str, build: fn(&mut FunctionBuilder) -> Value) {
    crate::exec::harness::compile_ok(
        name,
        code_forge::backend::aarch64::TargetMachine::new,
        TypeId::F64,
        build,
    );
}

/// fadd + fmul 组合链。
#[test]
fn float_arith_chain() {
    compile_ok_f64("aarch64_float_arith_chain", |b| {
        let a = b.fconst_f64(20.5);
        let bv = b.fconst_f64(21.5);
        let s = b.fadd(a, bv);
        let two = b.fconst_f64(2.0);
        b.fmul(s, two)
    });
}

/// fsub + 常量。
#[test]
fn float_sub() {
    compile_ok_f64("aarch64_float_sub", |b| {
        let a = b.fconst_f64(50.0);
        let bv = b.fconst_f64(8.0);
        b.fsub(a, bv)
    });
}

/// 一元运算链：fneg + fabs。
#[test]
fn float_unary_chain() {
    compile_ok_f64("aarch64_float_unary_chain", |b| {
        let a = b.fconst_f64(-42.0);
        let n = b.fneg(a);
        b.fabs(n)
    });
}

/// 浮点比较 → 整型。
#[test]
fn float_compare() {
    compile_ok_f64("aarch64_float_compare", |b| {
        let a = b.fconst_f64(3.0);
        let bv = b.fconst_f64(4.0);
        let c = b.fcmp(code_forge::ir::FloatCC::LessThan, a, bv);
        let one = b.iconst_i64(1);
        b.select(c, a, one)
    });
}
