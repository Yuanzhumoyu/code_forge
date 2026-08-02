//! aarch64 IO/内存指令测试（test-io feature 门控）— compile-only。
//!
//! aarch64 无本机执行；此处验证 load/store/stack_addr/alloca/gep
//! 代表性组合能编译成功，执行语义经 `exec-unicorn` 验证。

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

/// stack_addr → store → load 往返。
#[test]
fn io_store_load_roundtrip() {
    compile_ok("aarch64_io_store_load_roundtrip", |b| {
        let addr = b.stack_addr(-8);
        let v = b.iconst_i32(42);
        b.store(v, addr);
        b.load(addr, TypeId::I32)
    });
}

/// alloca → store → load。
#[test]
fn io_alloca_store_load() {
    compile_ok("aarch64_io_alloca_store_load", |b| {
        let ptr = b.alloca(TypeId::I32, 1);
        let v = b.iconst_i32(1234);
        b.store(v, ptr);
        b.load(ptr, TypeId::I32)
    });
}

/// alloca 4 槽 → gep 第 2 元素 → store → load。
#[test]
fn io_gep_store_load() {
    compile_ok("aarch64_io_gep_store_load", |b| {
        let base = b.alloca(TypeId::I32, 4);
        let idx = b.iconst_i32(2);
        let elem = b.gep(base, &[idx], TypeId::I32);
        let v = b.iconst_i32(99);
        b.store(v, elem);
        b.load(elem, TypeId::I32)
    });
}
