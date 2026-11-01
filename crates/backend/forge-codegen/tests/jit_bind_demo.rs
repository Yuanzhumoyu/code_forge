//! v32 M1 效果演示：目标感知的 `bind`（ABI 校验）+ `JitFn::call`。
//!
//! 运行：`cargo test -p forge-codegen --all-features --test jit_bind_demo -- --nocapture`
//! 预期输出（x86_64 宿主）：
//! ```text
//! add(2,3)=5
//! not(true)=false
//! ```

#![cfg(all(feature = "jit", target_arch = "x86_64"))]

use forge_codegen::jit::JitLoader;
use forge_codegen::x86;
use forge_ir::{FunctionSignature, Opcode, TypeId};

#[test]
fn bind_scalar_and_bool() {
    x86::ensure_registered();
    let mut jit = JitLoader::new(x86::TargetMachine::new());

    // 标量：add(i64, i64) -> i64
    let sig = FunctionSignature::new(&[(TypeId::I64, "a"), (TypeId::I64, "b")], &[TypeId::I64]);
    jit.add_function("add", &sig, |b| {
        let (entry, p) = b.create_block_with_params(&[(TypeId::I64, "a"), (TypeId::I64, "b")]);
        b.switch_to_block(entry);
        let sum = b.iadd(p[0], p[1]);
        b.ret(&[sum]);
    })
    .expect("add");

    let add = jit
        .bind::<extern "C" fn(i64, i64) -> i64>("add")
        .expect("bind add");
    assert!(add.is_host_compatible(), "x86_64 宿主应可安全调用");
    println!("add(2,3)={}", add.call(2, 3));
    assert_eq!(add.call(2, 3), 5);

    // bool：not(i1) -> i1；ABI 槽是 8 位整数（C99 `_Bool`）
    let sig = FunctionSignature::new(&[(TypeId::BOOL, "a")], &[TypeId::BOOL]);
    jit.add_function("not", &sig, |b| {
        let (entry, p) = b.create_block_with_params(&[(TypeId::BOOL, "a")]);
        b.switch_to_block(entry);
        let r = b.bnot(p[0]);
        b.ret(&[r]);
    })
    .expect("not");

    let not = jit
        .bind::<extern "C" fn(bool) -> bool>("not")
        .expect("bind not");
    println!("not(true)={}", not.call(true));
    assert!(!not.call(true));

    // source 模式：`u8` 与 IR 的 `i1` 语义不同，必须报错而不是静默放宽。
    let err = jit
        .bind_source::<extern "C" fn(u8) -> bool>("not")
        .expect_err("u8 不应匹配 i1 源码签名");
    assert!(matches!(
        err,
        forge_codegen::jit::JitBindError::ParamType { index: 0, .. }
    ));

    // 源码模式正向：bool 的 IR 源签名可校验。
    jit.bind_source::<extern "C" fn(bool) -> bool>("not")
        .expect("bool 源码签名应匹配 i1");

    // 让编译器知道 `Opcode` 被引用（早期探针保留的导入）。
    let _ = Opcode::Nop;
}
