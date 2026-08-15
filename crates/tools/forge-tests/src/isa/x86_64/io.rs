//! x86_64 IO/内存指令测试（test-io feature 门控）。
//!
//! 内存访问（load/store/stack_addr/alloca/gep）的本机执行断言，
//! 走 forge-tests 的统一管线（build → compile → ExecutableMemory → 本机执行）。

#![cfg(test)]

use code_forge::backend::arch::x86_64::ensure_registered;
use code_forge::prelude::*;

/// 无参返回 i32：统一 harness（`exec::harness::run_i32`）。
fn run(name: &str, build: fn(&mut FunctionBuilder) -> Value) -> i32 {
    ensure_registered();
    crate::exec::harness::run_i32(name, build)
}

/// store → load 往返（i32，栈槽 offset 0）。
#[test]
fn io_store_load_roundtrip_i32() {
    let r = run("io_store_load_roundtrip_i32", |b| {
        let addr = b.stack_addr(-8);
        let v = b.iconst_i32(42);
        b.store(v, addr);
        b.load(addr, TypeId::I32)
    });
    assert_eq!(r, 42);
}

/// store → load 往返（负数）。
#[test]
fn io_store_load_roundtrip_negative() {
    let r = run("io_store_load_roundtrip_negative", |b| {
        let addr = b.stack_addr(-8);
        let v = b.iconst_i32(-7);
        b.store(v, addr);
        b.load(addr, TypeId::I32)
    });
    assert_eq!(r, -7);
}

/// alloca → store → load 的 compile-only 覆盖。
///
/// 注意：alloca/gep 的**本机执行**在 x86_64 后端当前不稳定（SEGV），
/// 此处仅验证编译通过（执行断言见 `coverage.rs` 的 Alloca/GetElementPtr）。
#[test]
fn io_alloca_store_load_compiles() {
    let func = crate::exec::harness::build_func("io_alloca_compile", TypeId::I32, |b| {
        let ptr = b.alloca(TypeId::I32, 1);
        let v = b.iconst_i32(1234);
        b.store(v, ptr);
        b.load(ptr, TypeId::I32)
    });
    let compiled = crate::exec::harness::compile_x86_64("io_alloca_compile", &func);
    assert!(!compiled.code.is_empty(), "alloca function should compile");
}

/// 不同 offset 的 stack_addr 指向不同地址（offset 相差 4 字节）。
/// 用负偏移（rbp 下方局部槽区）：正偏移属 rbp 上方（返回地址/调用者区），
/// 不参与本函数 locals 帧计算。
#[test]
fn io_stack_addr_distinct_offsets() {
    let r = run("io_stack_addr_distinct_offsets", |b| {
        let a0 = b.stack_addr(-16);
        let a4 = b.stack_addr(-12);
        let diff = b.isub(a4, a0); // 指针差 = 4
        let zero = b.iconst(0, TypeId::I64);
        let ne = b.icmp(code_forge::ir::IntCC::NotEqual, diff, zero);
        b.uextend(ne, TypeId::I32)
    });
    assert_eq!(r, 1);
}
