//! v32 M1b：直接 IR 的 `i128` 入口参数（win64 C ABI 为 by-ref）编译并真执行。
//!
//! 形参被改写成 `ptr` + `load i128`，再由 `wide_split` 拆肢；返回暂时收窄到 i64，
//! 128 位返回（win64 XMM0）仍待做。
use forge_codegen::CompileSession;
use forge_codegen::jit::JitLoader;
use forge_codegen::x86;
use forge_ir::TypeId;
use forge_ir::ir::builder::FunctionBuilder;
use forge_ir::ir::types::{FunctionSignature, TypeContext};

#[test]
fn i128_entry_by_ref_compiles() {
    x86::ensure_registered();
    let i128 = TypeId::I128;
    let i64 = TypeId::I64;
    let sig = FunctionSignature::new(&[(i128, "a"), (i128, "b")], &[i64]);
    let mut b = FunctionBuilder::new("add128", TypeContext::new(), sig);
    let (entry, p) = b.create_block_with_params(&[(i128, "a"), (i128, "b")]);
    b.switch_to_block(entry);
    let s = b.iadd(p[0], p[1]);
    let narrowed = b.ireduce(s, i64);
    b.ret(&[narrowed]);
    let f = b.finish().expect("build");
    let r = CompileSession::new(x86::TargetMachine::new()).compile(&f);
    match r {
        Ok(c) => println!("i128 by-ref entry compiled: code len {}", c.code.len()),
        Err(e) => panic!("i128 by-ref entry should compile: {e}"),
    }
}

#[test]
fn i128_entry_by_ref_runs() {
    x86::ensure_registered();
    let mut jit = JitLoader::new(x86::TargetMachine::new());
    let i128 = TypeId::I128;
    let i64 = TypeId::I64;
    let sig = FunctionSignature::new(&[(i128, "a"), (i128, "b")], &[i64]);
    jit.add_function("add128", &sig, |b| {
        let (entry, p) = b.create_block_with_params(&[(i128, "a"), (i128, "b")]);
        b.switch_to_block(entry);
        let s = b.iadd(p[0], p[1]);
        let narrowed = b.ireduce(s, i64);
        b.ret(&[narrowed]);
    })
    .expect("add128");

    // 生成的 ABI 是 `(ptr, ptr) -> i64`（win64 i128 形参 by-ref）；bind_raw 只做
    // 装载、不校验签名，调用者按这个 ABI 传指针。
    let f = unsafe { jit.bind_raw::<extern "C" fn(*const i128, *const i128) -> i64>("add128") }
        .expect("bind_raw add128");
    let (a, b) = (2i128, 3i128);
    let got = (f.raw())(&a, &b);
    assert_eq!(got, 5, "by-ref i128 形参应读到 2+3 的低 64 位");
    println!("i128_by_ref_add(2,3)={got}");
}

/// 128 位返回尚未实现：聚合返回展开已不再把 i128 当聚合污染 IR；
/// 直接 IR 现在必须是明确的 WIDE_INT_SHAPE fail-closed，而不是坏 IR。
#[test]
fn i128_return_fail_closed() {
    x86::ensure_registered();
    let i128 = TypeId::I128;
    let i64 = TypeId::I64;
    let sig = FunctionSignature::new(&[(i64, "a")], &[i128]);
    let mut b = FunctionBuilder::new("widen", TypeContext::new(), sig);
    let (entry, p) = b.create_block_with_params(&[(i64, "a")]);
    b.switch_to_block(entry);
    let wide = b.uextend(p[0], i128);
    b.ret(&[wide]);
    let f = b.finish().expect("build");
    match CompileSession::new(x86::TargetMachine::new()).compile(&f) {
        Ok(_) => panic!("i128 直接返回尚未实现，应 fail-closed"),
        Err(e) => {
            let msg = format!("{e}");
            println!("i128 return fail-closed: {msg}");
            assert!(
                msg.contains("WIDE_INT_SHAPE"),
                "应为明确的 WIDE_INT_SHAPE（不是坏 IR/panic）: {msg}"
            );
        }
    }
}
