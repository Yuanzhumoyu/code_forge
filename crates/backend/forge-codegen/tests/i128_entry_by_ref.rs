//! v32 M1b：直接 IR 的 `i128` by-ref 入口与 XMM0 返回（win64）真执行。
//!
//! 形参改写成 `ptr` + `load i128`；返回把两个肢 store 进 16B 栈缓冲、再 load
//! `<2 x i64>` 走 XMM0。sysv64 整数对/by-value 仍待做。
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

/// win64 i128 返回走 XMM0：栈缓冲打包 low/high，再以 i128 真执行。
#[test]
fn i128_return_xmm0() {
    x86::ensure_registered();
    let mut jit = JitLoader::new(x86::TargetMachine::new());
    let i128 = TypeId::I128;
    let i64 = TypeId::I64;
    let sig = FunctionSignature::new(&[(i64, "a")], &[i128]);
    jit.add_function("widen", &sig, |b| {
        let (entry, p) = b.create_block_with_params(&[(i64, "a")]);
        b.switch_to_block(entry);
        let wide = b.uextend(p[0], i128);
        b.ret(&[wide]);
    })
    .expect("widen");

    let f = unsafe { jit.bind_raw::<extern "C" fn(i64) -> i128>("widen") }.expect("bind_raw widen");
    let got = (f.raw())(7i64);
    assert_eq!(got, 7i128, "i64→i128 返回应经 XMM0 完整回传");
    println!("i128_return_xmm0(7)={got}");
}
