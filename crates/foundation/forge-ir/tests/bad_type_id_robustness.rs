//! 坏 IR 的越界 `TypeId`/`SigRef` 必须上报诊断，**不许 panic**（v3 S3 边界切片）。
//!
//! 类型面的读取口此前只有 fail-closed 的 `TypeStore::get`/`get_signature`
//! （直接索引 `entries`/`signatures`）。IR 里的 `TypeId`/`SigRef` 是**数据**
//! （文本解析、跨模块拼接、pass 写错都可能产生越界值），校验器与 display 一旦
//! 拿它去索引就 panic —— 而"打印坏 IR 给人看"正是最需要不 panic 的时刻。
//!
//! 契约：①校验器对所有越界类型引用返回诊断（错误码 `BadTypeId`），不 panic；
//! ②display 对越界 `TypeId` 输出占位符（`<bad-type:9999>`）而不是 panic。

use forge_ir::ir::builder::FunctionBuilder;
use forge_ir::ir::types::{FunctionSignature, TypeContext};
use forge_ir::verify::{Verifier, VerifyError};
use forge_ir::{Module, SigRef, TypeId};

const BAD: u32 = 9999;

/// 一个合法函数 + 把 `v` 的值类型改成越界 id。
fn func_with_bad_value_type(ctx: &TypeContext) -> forge_ir::Function {
    let sig = FunctionSignature::new(&[], &[ctx.i32_ty()]);
    let mut fb = FunctionBuilder::new("bad_value_ty", ctx.clone(), sig);
    let (_entry, _) = fb.create_entry_block();
    let v = fb.iconst_i32(7);
    fb.ret(&[v]);
    let mut func = fb.finish().expect("build");
    assert!(
        func.dfg.set_value_type(v, TypeId::new(BAD)),
        "值类型改写在范围内"
    );
    func
}

/// 越界值类型：校验器必须报 `BadTypeId`（而不是索引越界 panic）。
#[test]
fn verifier_reports_bad_value_type() {
    let ctx = TypeContext::new();
    let func = func_with_bad_value_type(&ctx);
    let mut verifier = Verifier::with_ctx(ctx);
    let errs = verifier.verify(&func).expect_err("越界类型必须上报");
    assert!(
        errs.iter().any(|e| matches!(
            e,
            VerifyError::BadTypeId { ty, .. } if ty.index() == BAD
        )),
        "应报 BadTypeId（实际：{errs:?}）"
    );
}

/// 越界签名返回类型：签名本身也是数据，校验器同样必须报而不是 panic。
#[test]
fn verifier_reports_bad_signature_return_type() {
    let ctx = TypeContext::new();
    let sig = FunctionSignature::new(&[], &[TypeId::new(BAD)]);
    let sr = ctx.register_signature(sig);
    let mut fb = FunctionBuilder::new("bad_sig", ctx.clone(), FunctionSignature::new(&[], &[]));
    let (_entry, _) = fb.create_entry_block();
    fb.ret(&[]);
    let mut func = fb.finish().expect("build");
    func.signature = sr;

    let mut verifier = Verifier::with_ctx(ctx);
    let errs = verifier.verify(&func).expect_err("越界返回类型必须上报");
    assert!(
        errs.iter()
            .any(|e| matches!(e, VerifyError::BadTypeId { .. })),
        "应报 BadTypeId（实际：{errs:?}）"
    );
}

/// 越界 `SigRef`：签名句柄本身越界也要报（`get_signature` 是 fail-closed 索引）。
#[test]
fn verifier_reports_bad_sig_ref() {
    let ctx = TypeContext::new();
    let sig = FunctionSignature::new(&[], &[ctx.i32_ty()]);
    let mut fb = FunctionBuilder::new("bad_sigref", ctx.clone(), sig);
    let (_entry, _) = fb.create_entry_block();
    let v = fb.iconst_i32(3);
    fb.ret(&[v]);
    let mut func = fb.finish().expect("build");
    func.signature = SigRef::new(BAD);

    let mut verifier = Verifier::with_ctx(ctx);
    let errs = verifier.verify(&func).expect_err("越界 SigRef 必须上报");
    assert!(
        errs.iter()
            .any(|e| matches!(e, VerifyError::BadSigRef { .. })),
        "应报 BadSigRef（实际：{errs:?}）"
    );
}

/// display：坏 IR 也要能打印（占位符），不许 panic——诊断路径的基本要求。
///
/// 用"签名返回类型越界"这个形状：它一定会被打印到 `define <ty> @f()` 头部，
/// 因此能同时验证"不 panic"与"打出了可读的占位符"。
#[test]
fn display_tolerates_bad_value_type() {
    let mut m = Module::new();
    let ctx = m.types.clone();
    let sr = ctx.register_signature(FunctionSignature::new(&[], &[TypeId::new(BAD)]));
    let mut fb = FunctionBuilder::new(
        "bad_sig_display",
        ctx.clone(),
        FunctionSignature::new(&[], &[]),
    );
    let (_entry, _) = fb.create_entry_block();
    fb.ret(&[]);
    let mut func = fb.finish().expect("build");
    func.signature = sr;
    m.add_function(func);

    let text = format!("{m}");
    assert!(
        text.contains("bad_sig_display"),
        "函数名应在输出里（实际输出：{text}）"
    );
    assert!(
        text.contains(&format!("<bad-type:{BAD}>")),
        "越界类型应打印占位符（实际输出：{text}）"
    );
}
