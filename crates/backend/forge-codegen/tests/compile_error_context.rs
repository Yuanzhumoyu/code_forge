//! v30 M1：编译错误必须带**阶段 + 函数 + 缺口 ID**。
//!
//! 用 `landingpad` 造一个确定性的能力缺口（`[cap:EXCEPTION_LANDINGPAD]`，
//! 前置检查在任何 lowering 之前就拒绝），不依赖宿主 CPU 能力。

use forge_codegen::arch::x86::TargetMachine;
use forge_codegen::{CompileSession, Stage};
use forge_ir::ir::builder::FunctionBuilder;
use forge_ir::ir::types::{FunctionSignature, TypeContext};
use forge_ir::{Function, InstFlags, Opcode, TypeId};

fn landingpad_fn() -> Function {
    let sig = FunctionSignature::new(&[], &[TypeId::I64]);
    let mut b = FunctionBuilder::new("f", TypeContext::new(), sig);
    b.create_block_here();
    let _lp = b.emit1(
        Opcode::LandingPad,
        vec![],
        vec![],
        TypeId::I64,
        InstFlags::NONE,
    );
    let zero = b.const_int(0i64, TypeId::I64);
    b.ret(&[zero]);
    b.finish().expect("build landingpad 函数")
}

#[test]
fn compile_error_carries_stage_function_and_gap() {
    let f = landingpad_fn();
    let err = CompileSession::new(TargetMachine::new())
        .compile(&f)
        .expect_err("landingpad 必须 fail-closed");

    assert_eq!(err.stage(), Stage::Legalize, "前置能力门属于合法化阶段");
    assert_eq!(err.function(), Some("f"));
    assert_eq!(err.gap_id().as_deref(), Some("EXCEPTION_LANDINGPAD"));

    // 折叠回 IrError 后缺口文本不变（erased/插件边界的契约）。
    let ir = err.into_ir();
    assert_eq!(ir.gap_id().as_deref(), Some("EXCEPTION_LANDINGPAD"));
}
