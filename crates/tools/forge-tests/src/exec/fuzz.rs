//! 性质/模糊测试 — 验证 lowering + emit 管线正确性（从根 tests/fuzz_lowering.rs 迁移）。
//!
//! 对 IR 函数组合进行 JIT 编译执行，验证:
//! 1. 结果确定性（两次运行结果一致）
//! 2. 基本算术恒等式（a+b == b+a 等）
//! 3. 边界值正确性

#![cfg(test)]

use code_forge::backend::FunctionCompiler;
use code_forge::backend::arch::x86_64::{self, TargetMachine};
use code_forge::mem::ExecutableMemory;
use code_forge::prelude::*;
/// 编译并通过 JIT 执行一个返回 I64 的函数。
fn jit_execute(func: &Function) -> Result<i64, String> {
    x86_64::ensure_registered();
    let compiled = FunctionCompiler::new(TargetMachine::new())
        .compile_raw(func)
        .map_err(|e| format!("compile: {e}"))?;
    let mem = ExecutableMemory::new(&compiled.code).map_err(|e| format!("alloc: {e}"))?;
    let f: extern "C" fn() -> i64 = unsafe { mem.get_fn(0).unwrap() };
    Ok(f())
}

/// 构建一个返回 I64 的 IR 函数。
fn build_func(name: &str, build: impl Fn(&mut FunctionBuilder) -> Value) -> Function {
    let sig = FunctionSignature::new(&[], &[TypeId::I64]);
    let mut b = FunctionBuilder::new(name, TypeContext::new(), sig);
    let entry = b.create_block();
    b.switch_to_block(entry);
    let ret_val = build(&mut b);
    b.ret(&[ret_val]);
    b.finish()
}

/// 验证 JIT 执行结果等于期望值。
fn check_result(name: &str, expected: i64, build: impl Fn(&mut FunctionBuilder) -> Value) {
    let func = build_func(name, build);
    match jit_execute(&func) {
        Ok(actual) => assert_eq!(
            actual, expected,
            "{name}: expected {expected}, got {actual}"
        ),
        Err(e) => panic!("{name}: JIT failed: {e}"),
    }
}

/// 验证两次 JIT 执行结果一致（确定性）。
fn check_deterministic(name: &str, build: impl Fn(&mut FunctionBuilder) -> Value) {
    let func = build_func(name, &build);
    let r1 = jit_execute(&func).unwrap_or_else(|e| panic!("{name}: run1: {e}"));
    let r2 = jit_execute(&func).unwrap_or_else(|e| panic!("{name}: run2: {e}"));
    assert_eq!(r1, r2, "{name}: non-deterministic: {r1} vs {r2}");
}

/// 验证两个不同实现产生相同结果（语义等价检查）。
fn check_equivalent(
    name: &str,
    build_a: impl Fn(&mut FunctionBuilder) -> Value,
    build_b: impl Fn(&mut FunctionBuilder) -> Value,
) {
    let func_a = build_func(&format!("{name}_a"), build_a);
    let func_b = build_func(&format!("{name}_b"), build_b);
    let ra = jit_execute(&func_a).unwrap_or_else(|e| panic!("{name}_a: {e}"));
    let rb = jit_execute(&func_b).unwrap_or_else(|e| panic!("{name}_b: {e}"));
    assert_eq!(ra, rb, "{name}: {ra} vs {rb}");
}

// ============================================================
// 确定性测试 — 确保相同代码产生相同结果
// ============================================================

#[test]
fn fuzz_deterministic_constants() {
    for val in [0i64, 1, -1, 42, i64::MAX, i64::MIN] {
        let v = val; // capture by value
        check_deterministic(&format!("const_{val}"), move |b| b.iconst_i64(v));
    }
}

// ============================================================
// 语义等价测试 — 不同 IR 序列产生相同结果
// ============================================================

#[test]
fn fuzz_commutative_add() {
    // a + b == b + a
    check_equivalent(
        "comm_add",
        |b| {
            let a = b.iconst_i64(30);
            let c = b.iconst_i64(12);
            b.iadd(a, c)
        },
        |b| {
            let a = b.iconst_i64(12);
            let c = b.iconst_i64(30);
            b.iadd(a, c)
        },
    );
}

#[test]
fn fuzz_commutative_mul() {
    // a * b == b * a
    check_equivalent(
        "comm_mul",
        |b| {
            let a = b.iconst_i64(7);
            let c = b.iconst_i64(11);
            b.imul(a, c)
        },
        |b| {
            let a = b.iconst_i64(11);
            let c = b.iconst_i64(7);
            b.imul(a, c)
        },
    );
}

#[test]
fn fuzz_distributive() {
    // (a+b)*c == a*c + b*c
    check_equivalent(
        "distrib",
        |b| {
            let a = b.iconst_i64(3);
            let d = b.iconst_i64(5);
            let sum = b.iadd(a, d);
            let c = b.iconst_i64(7);
            b.imul(sum, c)
        },
        |b| {
            let a = b.iconst_i64(3);
            let d = b.iconst_i64(5);
            let c = b.iconst_i64(7);
            let ac = b.imul(a, c);
            let dc = b.imul(d, c);
            b.iadd(ac, dc)
        },
    );
}

#[test]
fn fuzz_add_sub_identity() {
    // (a+b)-b == a
    check_equivalent(
        "add_sub_id",
        |b| b.iconst_i64(42),
        |b| {
            let a = b.iconst_i64(42);
            let d = b.iconst_i64(15);
            let sum = b.iadd(a, d);
            b.isub(sum, d)
        },
    );
}

#[test]
fn fuzz_double_negate() {
    // bnot(bnot(x)) == x
    check_equivalent(
        "double_not",
        |b| b.iconst_i64(0x12345678),
        |b| {
            let x = b.iconst_i64(0x12345678);
            let n1 = b.bnot(x);
            b.bnot(n1)
        },
    );
}

#[test]
fn fuzz_xor_self_zero() {
    // x ^ x == 0
    check_result("xor_self", 0, |b| {
        let x = b.iconst_i64(0xDEADBEEF);
        b.bxor(x, x)
    });
}

#[test]
fn fuzz_and_zero() {
    // x & 0 == 0
    check_result("and_zero", 0, |b| {
        let x = b.iconst_i64(0xFFFFFFFF);
        let z = b.iconst_i64(0);
        b.band(x, z)
    });
}

#[test]
fn fuzz_or_all_ones() {
    // x | 0 == x
    check_result("or_identity", 0xABCD, |b| {
        let x = b.iconst_i64(0xABCD);
        let z = b.iconst_i64(0);
        b.bor(x, z)
    });
}

#[test]
fn fuzz_shift_left_right() {
    // (x << 8) >> 8 == x (logical, for positive numbers)
    check_result("shl8_shr8", 0x1234, |b| {
        let x = b.iconst_i64(0x1234);
        let s = b.iconst_i64(8);
        let shl = b.ishl(x, s);
        b.ushr(shl, s)
    });
}

#[test]
fn fuzz_div_mul_identity() {
    // (x / y) * y == x - (x % y)  (use small values to avoid urem issues)
    check_result("div_mul_check", 98, |b| {
        let x = b.iconst_i64(100);
        let y = b.iconst_i64(7);
        let q = b.udiv(x, y); // 100/7 = 14
        b.imul(q, y) // 14*7 = 98
    });
}

// ============================================================
// 64-bit boundary tests — REX.W enabled via opsize propagation (Phase 3)
// ============================================================

#[test]
fn fuzz_boundary_i64_max_add() {
    // i64::MAX + 1 wraps to i64::MIN (requires REX.W)
    check_result("i64_max_add_1", i64::MIN, |b| {
        let a = b.iconst_i64(i64::MAX);
        let c = b.iconst_i64(1);
        b.iadd(a, c)
    });
}

#[test]
fn fuzz_boundary_i64_min_sub() {
    // i64::MIN - 1 wraps to i64::MAX (requires REX.W)
    check_result("i64_min_sub_1", i64::MAX, |b| {
        let a = b.iconst_i64(i64::MIN);
        let c = b.iconst_i64(1);
        b.isub(a, c)
    });
}

#[test]
fn fuzz_boundary_large_mul() {
    // 1_000_000_000 × 1_000_000_000 = 10^18 (> 2^32, requires REX.W)
    check_result("large_mul", 1_000_000_000_000_000_000, |b| {
        let a = b.iconst_i64(1_000_000_000);
        let c = b.iconst_i64(1_000_000_000);
        b.imul(a, c)
    });
}

#[test]
fn fuzz_boundary_power_of_two_shl() {
    // 1 << 62 = 4611686018427387904 (> 2^32, requires REX.W)
    check_result("shl_62", 1i64 << 62, |b| {
        let a = b.iconst_i64(1);
        let c = b.iconst_i64(62);
        b.ishl(a, c)
    });
}

#[test]
fn fuzz_boundary_not_i64_max() {
    // NOT(i64::MAX) = i64::MIN (all bits inverted, requires REX.W)
    check_result("not_max", i64::MIN, |b| {
        let a = b.iconst_i64(i64::MAX);
        b.bnot(a)
    });
}
