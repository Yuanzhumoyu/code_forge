//! riscv64 IO/内存指令测试（test-io feature 门控）— compile-only。
//!
//! riscv64 无本机执行；此处验证 load/stack_addr/alloca/gep 代表性组合能编译成功。
//! 注意：riscv64 后端当前不支持 `Store` lowering（见 coverage 矩阵，Store 不在 75 ops 列表），
//! 故用例仅覆盖 load 侧；执行语义经 `exec-unicorn` 验证。

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

/// stack_addr → load（compile-only）。
#[test]
fn io_load_stack_addr() {
    compile_ok("riscv64_io_load_stack_addr", |b| {
        let addr = b.stack_addr(-8);
        b.load(addr, TypeId::I32)
    });
}

/// alloca → load（compile-only）。
#[test]
fn io_alloca_load() {
    compile_ok("riscv64_io_alloca_load", |b| {
        let ptr = b.alloca(TypeId::I32, 1);
        b.load(ptr, TypeId::I32)
    });
}

/// alloca 4 槽 → gep 第 2 元素 → load（compile-only）。
#[test]
fn io_gep_load() {
    compile_ok("riscv64_io_gep_load", |b| {
        let base = b.alloca(TypeId::I32, 4);
        let idx = b.iconst_i32(2);
        let elem = b.gep(base, &[idx], TypeId::I32);
        b.load(elem, TypeId::I32)
    });
}
