//! x86_64 整数指令测试（test-int feature 门控）。
//!
//! 从根 crate `tests/jit_integration.rs` 迁移的代表性整数测试，

#![cfg(test)]
//! 走 forge-tests 的统一管线（build → compile → ExecutableMemory → 本机执行）。

use code_forge::backend::arch::x86_64::ensure_registered;
use code_forge::prelude::*;

/// 无参返回 i32：统一 harness（`exec::harness::run_i32`）。
fn run(name: &str, build: fn(&mut FunctionBuilder) -> Value) -> i32 {
    ensure_registered();
    crate::exec::harness::run_i32(name, build)
}

#[test]
fn int_mov_ri() {
    let r = run("int_mov_ri", |b| b.iconst_i32(42));
    assert_eq!(r, 42);
}

#[test]
fn int_add() {
    let r = run("int_add", |b| {
        let __a = b.iconst_i32(20);
        let __b = b.iconst_i32(22);
        b.iadd(__a, __b)
    });
    assert_eq!(r, 42);
}

#[test]
fn int_sub() {
    let r = run("int_sub", |b| {
        let __a = b.iconst_i32(50);
        let __b = b.iconst_i32(8);
        b.isub(__a, __b)
    });
    assert_eq!(r, 42);
}

#[test]
fn int_mul() {
    let r = run("int_mul", |b| {
        let __a = b.iconst_i32(6);
        let __b = b.iconst_i32(7);
        b.imul(__a, __b)
    });
    assert_eq!(r, 42);
}

#[test]
fn int_band() {
    let r = run("int_band", |b| {
        let __a = b.iconst_i32(0xFF);
        let __b = b.iconst_i32(0x2A);
        b.band(__a, __b)
    });
    assert_eq!(r, 0x2A);
}

#[test]
fn int_bor() {
    let r = run("int_bor", |b| {
        let __a = b.iconst_i32(0x20);
        let __b = b.iconst_i32(0x0A);
        b.bor(__a, __b)
    });
    assert_eq!(r, 0x2A);
}

#[test]
fn int_bxor() {
    let r = run("int_bxor", |b| {
        let __a = b.iconst_i32(0x30);
        let __b = b.iconst_i32(0x1A);
        b.bxor(__a, __b)
    });
    assert_eq!(r, 0x2A);
}
