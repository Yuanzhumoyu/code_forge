//! v30 M4：类型化 JIT 调用的正/反例（`get` 的签名校验 + `get_unchecked` 逃生口）。
#![cfg(target_arch = "x86_64")]

use super::*;
use crate::arch::x86;

fn jit_with_add() -> JitLoader<x86::TargetMachine> {
    x86::ensure_registered();
    let mut jit = JitLoader::new(x86::TargetMachine::new());
    let sig = FunctionSignature::new(&[(TypeId::I64, "a"), (TypeId::I64, "b")], &[TypeId::I64]);
    jit.add_function("add", &sig, |b| {
        let (blk, p) = b.create_block_with_params(&[(TypeId::I64, "a"), (TypeId::I64, "b")]);
        b.switch_to_block(blk);
        let sum = b.iadd(p[0], p[1]);
        b.ret(&[sum]);
    })
    .expect("add");
    jit
}

#[test]
fn typed_get_checks_and_calls() {
    let jit = jit_with_add();
    let f: extern "C" fn(i64, i64) -> i64 = jit.get("add").expect("typed get");
    assert_eq!(f(2, 3), 5);
    assert!(jit.get_ptr("add").is_some(), "裸地址可查");
    assert!(jit.function("add").is_some(), "产物可 introspection");
}

#[test]
fn wrong_arity_is_rejected_as_semantic() {
    let jit = jit_with_add();
    let err = jit
        .get::<extern "C" fn(i64) -> i64>("add")
        .expect_err("少一个参数必须被拒");
    assert!(matches!(err, IrError::Semantic(_)), "{err:?}");
    assert!(format!("{err}").contains("形参不匹配"), "{err}");
}

#[test]
fn wrong_return_is_rejected() {
    let jit = jit_with_add();
    let err = jit
        .get::<extern "C" fn(i64, i64) -> f64>("add")
        .expect_err("返回类型不匹配必须被拒");
    assert!(matches!(err, IrError::Semantic(_)), "{err:?}");
    assert!(format!("{err}").contains("返回不匹配"), "{err}");
}

#[test]
fn variadic_function_requires_unchecked() {
    x86::ensure_registered();
    let mut jit = JitLoader::new(x86::TargetMachine::new());
    let sig = FunctionSignature::new(&[(TypeId::I64, "n")], &[TypeId::I64]).with_variadic(true);
    jit.add_function("variadic", &sig, |b| {
        let (blk, p) = b.create_block_with_params(&[(TypeId::I64, "n")]);
        b.switch_to_block(blk);
        b.ret(&[p[0]]);
    })
    .expect("variadic");
    let err = jit
        .get::<extern "C" fn(i64) -> i64>("variadic")
        .expect_err("变参必须走 get_unchecked");
    assert!(format!("{err}").contains("变参"), "{err}");
    // 逃生口仍可用（调用者断言 ABI）。
    let f: extern "C" fn(i64) -> i64 = unsafe { jit.get_unchecked("variadic") }.expect("unchecked");
    assert_eq!(f(7), 7);
}

/// `PTR` 与 `I64` 在 64 位 ABI 上是同一个槽（IR 地址常写成 i64）——类型化 `get` 接受。
#[test]
fn pointer_and_i64_share_a_slot() {
    let jit = jit_with_add();
    let _f: extern "C" fn(*mut i64, i64) -> i64 = jit.get("add").expect("PTR 与 I64 应互相兼容");
}

#[test]
fn missing_function_is_internal_not_semantic() {
    let jit = jit_with_add();
    let err = jit
        .get::<extern "C" fn() -> i64>("nope")
        .expect_err("不存在的函数");
    assert!(matches!(err, IrError::Internal(_)), "{err:?}");
}
