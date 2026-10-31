//! v31：**合法化后完整 IR 校验**（`CompileOptions::with_verify(On)`）。
//!
//! 这两条用例把契约钉在公开面上：
//! 1. GEP 展开产生的 `add ptr, i64` 是 `binop_ptr_off` 的**正式形状**，合法化后的
//!    IR 必须能通过完整 `Verifier`；
//! 2. 真把类型改坏时，`Verify` 错误必须带着函数名与错误明细从编译入口返回
//!    （而不是静默生成错码）。

use forge_codegen::arch::x86::TargetMachine;
use forge_codegen::{CompileOptions, CompileSession, Stage, VerifyLevel};
use forge_ir::ir::builder::FunctionBuilder;
use forge_ir::ir::types::{FunctionSignature, TypeContext};
use forge_ir::{Function, InstFlags, IrError, Opcode, TypeId};

fn ptr_gep_function() -> Function {
    let i64 = TypeId::I64;
    let sig = FunctionSignature::new(&[(i64, "base"), (i64, "idx")], &[i64]);
    let mut b = FunctionBuilder::new("gep_dyn", TypeContext::new(), sig);
    let (blk, p) = b.create_block_with_params(&[(i64, "base"), (i64, "idx")]);
    b.switch_to_block(blk);
    // 地址先物化成 ptr，再做 GEP（GEP 展开会生成 `add ptr, i64` = binop_ptr_off）。
    let addr = b.inttoptr(p[0], TypeId::PTR);
    let slot = b.gep(addr, &[p[1]], i64);
    let v = b.load(slot, i64);
    b.ret(&[v]);
    b.finish().expect("build gep_dyn")
}

/// 类型真坏掉时（绕过 builder 的 `Iadd` 类型检查）——完整校验必须 fail-closed。
fn ill_typed_function() -> Function {
    let i64 = TypeId::I64;
    let sig = FunctionSignature::new(&[(i64, "base")], &[i64]);
    let mut b = FunctionBuilder::new("bad_gep", TypeContext::new(), sig);
    let (blk, p) = b.create_block_with_params(&[(i64, "base")]);
    b.switch_to_block(blk);
    // 强制 legalize 跑起来（GEP 是 `needs_agg_expand` 的触发条件之一）。
    let addr = b.inttoptr(p[0], TypeId::PTR);
    let zero = b.const_int(0i64, i64);
    let slot = b.gep(addr, &[zero], i64);
    // 故意造一条类型不一致的 Iadd（i64 + f64 → i64），绕过 builder 的 upcast 断言。
    let f = b.const_float(1.0f64.to_bits(), TypeId::F64);
    let _bad = b.emit1(
        Opcode::Iadd,
        vec![p[0], f],
        vec![],
        TypeId::I64,
        InstFlags::NONE,
    );
    let v = b.load(slot, i64);
    b.ret(&[v]);
    b.finish().expect("build bad_gep")
}

#[test]
fn gep_expansion_passes_full_verifier() {
    let opts = CompileOptions::off().with_verify(VerifyLevel::On);
    let cf = CompileSession::new(TargetMachine::new())
        .with_options(opts)
        .compile(&ptr_gep_function())
        .expect("GEP 展开后的 IR 必须通过完整 Verifier");
    assert!(!cf.code.is_empty());
}

#[test]
fn ill_typed_ir_is_rejected_with_verify_error() {
    let opts = CompileOptions::off().with_verify(VerifyLevel::On);
    let err = CompileSession::new(TargetMachine::new())
        .with_options(opts)
        .compile(&ill_typed_function())
        .expect_err("坏类型必须被完整校验拒绝");
    assert_eq!(err.stage(), Stage::Legalize, "校验发生在合法化阶段");
    assert!(
        matches!(err.ir_error(), IrError::Verify(_)),
        "应报 IrError::Verify，实测：{:?}",
        err.ir_error()
    );
    assert!(
        format!("{err}").contains("bad_gep"),
        "错误里应点名函数：{err}"
    );
}
