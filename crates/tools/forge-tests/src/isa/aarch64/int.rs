//! aarch64 整数指令测试（test-int feature 门控）— compile-only。
//!
//! aarch64 无本机执行；此处验证代表性整数指令组合能编译成功（产物非空），
//! 执行语义经 `exec-unicorn` feature 的 cross_arch_exec 验证。

#![cfg(test)]

use code_forge::prelude::*;

/// compile-only 断言（aarch64 TargetMachine）。
fn compile_ok(name: &str, build: fn(&mut FunctionBuilder) -> Value) {
    crate::exec::harness::compile_ok(
        name,
        code_forge::backend::aarch64::TargetMachine::new,
        TypeId::I32,
        build,
    );
}

/// iadd + imul 组合链。
#[test]
fn int_arith_chain() {
    compile_ok("aarch64_int_arith_chain", |b| {
        let a = b.iconst_i32(6);
        let bv = b.iconst_i32(7);
        let s = b.iadd(a, bv);
        let three = b.iconst_i32(3);
        b.imul(s, three)
    });
}

/// isub + 常量。
#[test]
fn int_sub_const() {
    compile_ok("aarch64_int_sub_const", |b| {
        let a = b.iconst_i32(50);
        let bv = b.iconst_i32(8);
        b.isub(a, bv)
    });
}

/// 移位链：ishl + ushr。
#[test]
fn int_shift_chain() {
    compile_ok("aarch64_int_shift_chain", |b| {
        let a = b.iconst_i32(1);
        let sh = b.iconst_i32(4);
        let l = b.ishl(a, sh);
        let two = b.iconst_i32(2);
        b.ushr(l, two)
    });
}

/// 位运算链：band + bor + bxor。
#[test]
fn int_bitwise_chain() {
    compile_ok("aarch64_int_bitwise_chain", |b| {
        let a = b.iconst_i32(0xFF);
        let bv = b.iconst_i32(0x0F);
        let x = b.band(a, bv);
        let c1 = b.iconst_i32(0x30);
        let y = b.bor(x, c1);
        let c2 = b.iconst_i32(0x0A);
        b.bxor(y, c2)
    });
}
