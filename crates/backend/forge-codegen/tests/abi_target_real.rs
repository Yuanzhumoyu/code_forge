//! **宿主适配器**（v20 A3）：真实后端的 `TargetMachine` → `forge-abi` 引擎 → `AbiPlan`。
//!
//! 本文件的价值是**交叉核对**：A1 的快照是在"合成目标"上算的，这里在真机上再算一遍，
//! 同一个签名必须给出同一份落点（x86 的 `win64`：参数 RCX/XMM1、返回 RAX、callee-saved
//! 与 clobber 来自 `TargetRegInfo`）。差异 = 适配器或数据有一边错了。

use forge_abi::AbiTarget;
use forge_abi::builtin;
use forge_codegen::arch::x86_v12::TargetMachine;
use forge_codegen::pipeline::abi_target::{MachineAbiTarget, plan_for_function};
use forge_ir::CallConvId;
use forge_ir::ConvName;
use forge_ir::TypeId;
use forge_ir::ir::builder::FunctionBuilder;
use forge_ir::ir::function::Function;
use forge_ir::ir::types::{FunctionSignature, TypeContext};

/// `fn f(i64, f64) -> i64`，用 `win64` 约定。
fn win64_probe() -> Function {
    let ctx = TypeContext::new();
    let sig = FunctionSignature::new(&[(TypeId::I64, "n"), (TypeId::F64, "x")], &[TypeId::I64])
        .with_calling_convention(CallConvId::builtin(ConvName::Win64));
    let mut b = FunctionBuilder::new("win64_probe", ctx, sig);
    let (entry, params) = b.create_block_with_params(&[(TypeId::I64, "n"), (TypeId::F64, "x")]);
    b.switch_to_block(entry);
    b.ret(&[params[0]]);
    b.finish().expect("build")
}

#[test]
fn adapter_exposes_the_real_register_file() {
    let tm = TargetMachine::new();
    let t = MachineAbiTarget::new(&tm);
    assert_eq!(t.isa_name(), "x86_64_v12");
    assert_eq!(t.reg_count(), 32, "16 GPR + 16 XMM");
    // 名字解析与 A1 的绑定文件口径一致（RCX=1、XMM0=16）。
    assert_eq!(t.reg_index("RCX"), Some(1));
    assert_eq!(t.reg_index("XMM0"), Some(16));
    assert_eq!(t.reg_name(1).as_deref(), Some("RCX"));
    // sp/fp 固定用途（生成器已把它们从可分配表里排除）。
    let rsp = t.reg_index("RSP").expect("RSP");
    assert!(t.pinned(rsp), "RSP 必须 pinned");
    assert!(!t.allocatable().contains(&rsp));
    assert!(t.allocatable().contains(&t.reg_index("RAX").unwrap()));
    // 能力来自 ISA 自己的申报（DSL 从 roles 生成）。
    assert_eq!(t.cap(forge_abi::Capability::GprMov), Some(64));
    assert_eq!(t.cap(forge_abi::Capability::Call), Some(64));
}

/// 真机 plan 与 A1 的合成目标快照一致（同一份数据、两条路）。
#[test]
fn real_machine_plan_matches_the_synthetic_goldens() {
    let tm = TargetMachine::new();
    let reg = builtin::registry().expect("内置注册表");
    let func = win64_probe();
    let plan = plan_for_function(&tm, &reg, "win64", &func).expect("win64 plan");

    assert_eq!(plan.conv, "win64");
    // 实参：按**位置**计数——第 1 个整数进 RCX、第 2 个浮点进 XMM1。
    match &plan.args[0].place {
        forge_abi::Placement::Reg { reg, .. } => assert_eq!(reg.name, "RCX"),
        other => panic!("{other:?}"),
    }
    match &plan.args[1].place {
        forge_abi::Placement::Reg { reg, .. } => assert_eq!(reg.name, "XMM1"),
        other => panic!("{other:?}"),
    }
    // 返回：返回池（RAX），不是参数池的 RCX。
    match &plan.ret {
        forge_abi::RetLoc::Reg { reg } => assert_eq!(reg.name, "RAX"),
        other => panic!("{other:?}"),
    }
    // callee-saved 与 clobber 都来自真机的寄存器表。
    let cs: Vec<&str> = plan
        .callee_saved
        .regs
        .iter()
        .map(|r| r.name.as_str())
        .collect();
    assert!(cs.contains(&"RBX") && cs.contains(&"RDI"), "{cs:?}");
    assert!(plan.clobbers.iter().any(|r| r.name == "RAX"));
    assert_eq!(plan.stack.shadow_bytes, 32, "Win64 的 shadow space");
}

/// 未注册的约定名在真机上同样 fail-closed（错误来自引擎，不是某处兜底）。
#[test]
fn unregistered_convention_is_rejected_on_a_real_machine() {
    let tm = TargetMachine::new();
    let reg = builtin::registry().expect("内置注册表");
    let err = plan_for_function(&tm, &reg, "nope_conv", &win64_probe()).unwrap_err();
    assert!(err.to_string().contains("未注册"), "{err}");
}

/// **前置核对**（A3b 的准备工作）：引擎算出的计划与**当前**发射路径的 `AllocResult`
/// 必须一致——这是"把发射切到 plan 上"之前唯一能先做的正确性检查。
///
/// 比的是两边都有的可观测量：有没有 sret、逐参数是否按引用、栈参数区字节数。
#[test]
fn plan_agrees_with_the_existing_lowering_result() {
    use forge_codegen::FunctionCompiler;

    let compiler = FunctionCompiler::new(TargetMachine::new());
    let func = win64_probe();
    let (_cf, alloc) = compiler.compile_with_alloc(&func).expect("compile");

    let tm = TargetMachine::new();
    let reg = builtin::registry().expect("内置注册表");
    let plan = plan_for_function(&tm, &reg, "win64", &func).unwrap_or_else(|e| panic!("plan: {e}"));

    assert_eq!(
        matches!(plan.ret, forge_abi::RetLoc::Indirect { .. }),
        alloc.sret,
        "sret：plan 与现有发射路径必须一致"
    );
    let by_ref: Vec<bool> = plan
        .args
        .iter()
        .map(|a| matches!(a.place, forge_abi::Placement::Indirect { .. }))
        .collect();
    assert_eq!(by_ref, alloc.param_by_ref, "逐参数 by-ref 判定");
    // 栈参数区：plan 的 arg_area 扣掉 shadow 后应与现有路径的栈参数字节数一致。
    assert_eq!(
        plan.stack
            .arg_area_bytes
            .saturating_sub(plan.stack.shadow_bytes),
        alloc.stack_arg_bytes,
        "栈参数区字节数"
    );
}
