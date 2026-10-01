//! **宿主适配器**（v20 A3）：真实后端的 `TargetMachine` → `forge-abi` 引擎 → `AbiPlan`。
//!
//! 本文件的价值是**交叉核对**：A1 的快照是在"合成目标"上算的，这里在真机上再算一遍，
//! 同一个签名必须给出同一份落点（x86 的 `win64`：参数 RCX/XMM1、返回 RAX、callee-saved
//! 与 clobber 来自 `TargetRegInfo`）。差异 = 适配器或数据有一边错了。

use forge_abi::AbiTarget;
use forge_abi::builtin;
use forge_codegen::arch::x86_v12::TargetMachine;
use forge_codegen::machine::target::TargetMachine as _;
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

/// `fn f(i64 × 6) -> i64`：win64 只有 4 个整数槽，第 5/6 个参数走栈。
fn win64_stack_args_probe() -> Function {
    let ctx = TypeContext::new();
    let params: Vec<(TypeId, &str)> = (0..6).map(|_| (TypeId::I64, "a")).collect();
    let sig = FunctionSignature::new(&params, &[TypeId::I64])
        .with_calling_convention(CallConvId::builtin(ConvName::Win64));
    let mut b = FunctionBuilder::new("win64_stack_args", ctx, sig);
    let (entry, p) = b.create_block_with_params(&params);
    b.switch_to_block(entry);
    b.ret(&[p[0]]);
    b.finish().expect("build")
}

/// **栈参数的偏移核对**（v20 A3b-2b-2c 的入场券，A5-3 起是唯一事实源）：布局给的
/// `Stack { offset }` 必须等于 **x86 约定的算式**
/// `first_offset_slots × slot + shadow + (pos − n_int) × slot`
/// （数值来自 `AbiRules`：win64 = 2 / 32 / 8；谱面已不再声明 `[abi.stack_args]`）。
///
/// 这是"把栈参数收参也切到布局"的正确性检查：**偏移差一**就是读错值
/// （不是崩，而是静默错值），矩阵未必抓得到，必须先钉住。
#[test]
fn stack_arg_offsets_match_the_x86_convention_numbers() {
    use forge_isa_runtime::machine::call_layout::ArgPlace;

    let tm = TargetMachine::new();
    let reg = builtin::registry().expect("内置注册表");
    let func = win64_stack_args_probe();
    let plan = plan_for_function(&tm, &reg, "win64", &func).expect("plan");
    let layout = forge_codegen::pipeline::abi_target::call_layout(&plan, &tm);

    // x86 约定的口径（AbiRules）：first_offset_slots = 2、shadow = 32、slot = 8。
    let slot = layout.slot_bytes as i64;
    let first = 2 * slot; // first_offset_slots × slot
    let shadow = layout.shadow_bytes as i64;
    let n_int = 4; // win64 整数参数槽

    // 前 4 个进寄存器，第 5/6 个（索引 4/5）在栈上。
    for (i, arg) in layout.args.iter().enumerate() {
        let expect = match i {
            4 | 5 => first + shadow + (i as i64 - n_int) * slot,
            _ => -1,
        };
        match &arg.place {
            ArgPlace::Reg { .. } if expect < 0 => {}
            ArgPlace::Stack { offset, .. } if expect >= 0 => {
                assert_eq!(
                    *offset as i64, expect,
                    "第 {i} 个参数的栈偏移：布局 {offset} ≠ 约定算式 {expect}"
                );
            }
            other => panic!(
                "第 {i} 个参数落点不符（期望 {}）：{other:?}",
                if expect < 0 { "寄存器" } else { "栈" }
            ),
        }
    }
    // 首个栈参数的**调用方**视角偏移 = shadow（`caller_offset(0)`）。
    assert_eq!(layout.caller_offset(0), layout.shadow_bytes as i32);
}

/// `fn f(i64, i64, i64) -> i64`，用 `lp64d` 约定（riscv64 后端）。
///
/// **只用整数**：riscv64 的浮点参数在现有发射路径上就 fail-closed（见下一条测试），
/// 这里要比的是"同一条链路上两者一致"，所以先取两边都能走的签名。
fn lp64d_probe() -> Function {
    let ctx = TypeContext::new();
    let sig = FunctionSignature::new(
        &[(TypeId::I64, "a"), (TypeId::I64, "b"), (TypeId::I64, "c")],
        &[TypeId::I64],
    )
    .with_calling_convention(CallConvId::builtin(ConvName::Lp64d));
    let mut b = FunctionBuilder::new("lp64d_probe", ctx, sig);
    let (entry, params) =
        b.create_block_with_params(&[(TypeId::I64, "a"), (TypeId::I64, "b"), (TypeId::I64, "c")]);
    b.switch_to_block(entry);
    b.ret(&[params[0]]);
    b.finish().expect("build")
}

/// **第二台机器上的同一条核对**：riscv64/lp64d 上 plan 与现有发射路径的 `AllocResult`
/// 也必须一致（有没有 sret、逐参数 by-ref、栈参数区字节数）。
///
/// 两台机器都过，才说明"按 plan 发射"有一个可信的起点；任何一条不一致都是 A3b-2
/// 必须先解掉的差异——本测试就是提前把它暴露出来。
#[test]
fn plan_agrees_with_the_existing_lowering_result_on_riscv64() {
    use forge_codegen::FunctionCompiler;
    use forge_codegen::arch::riscv64_v12::TargetMachine as RvTm;

    let compiler = FunctionCompiler::new(RvTm::new());
    let func = lp64d_probe();
    let (_cf, alloc) = compiler.compile_with_alloc(&func).expect("compile");

    let reg = builtin::registry().expect("内置注册表");
    let plan = plan_for_function(&RvTm::new(), &reg, "lp64d", &func)
        .unwrap_or_else(|e| panic!("plan: {e}"));

    assert_eq!(
        matches!(plan.ret, forge_abi::RetLoc::Indirect { .. }),
        alloc.sret,
        "lp64d：sret 判定"
    );
    let by_ref: Vec<bool> = plan
        .args
        .iter()
        .map(|a| matches!(a.place, forge_abi::Placement::Indirect { .. }))
        .collect();
    assert_eq!(by_ref, alloc.param_by_ref, "lp64d：逐参数 by-ref 判定");
    assert_eq!(
        plan.stack
            .arg_area_bytes
            .saturating_sub(plan.stack.shadow_bytes),
        alloc.stack_arg_bytes,
        "lp64d：栈参数区字节数"
    );
}

/// **把已知差异钉住**（A3b-2 的任务清单，2026-09-25 实测）：
///
/// riscv64 上"引擎能算出浮点参数的落点"（谱里有 F 寄存器组、绑定给了 `float` 池），
/// 但**现有发射路径**对浮点参数是 fail-closed（`Emit("v12 float args (MOVSD/MOVSS missing)")`
/// ——riscv64 谱里没有 `fpr_mov` 角色）。也就是说：切换发射之前，必须先补上 riscv64 的
/// 浮点搬运角色/指令，否则"按 plan 发射"会在这一步与现状同样卡住（甚至更早）。
///
/// 另一头也一并钉住：arm64 连 FPR 寄存器组都没有 ⇒ 引擎侧直接报缺池（A1 静态体检的 6 条缺口）。
#[test]
fn riscv64_float_gap_is_engine_ok_but_emission_closed() {
    use forge_codegen::FunctionCompiler;
    use forge_codegen::arch::riscv64_v12::TargetMachine as RvTm;

    let ctx = TypeContext::new();
    let sig = FunctionSignature::new(&[(TypeId::I64, "n"), (TypeId::F64, "x")], &[TypeId::I64])
        .with_calling_convention(CallConvId::builtin(ConvName::Lp64d));
    let mut b = FunctionBuilder::new("rv_f64_probe", ctx, sig);
    let (entry, params) = b.create_block_with_params(&[(TypeId::I64, "n"), (TypeId::F64, "x")]);
    b.switch_to_block(entry);
    b.ret(&[params[0]]);
    let func = b.finish().expect("build");

    // 引擎侧：能算（int 槽 X10 + 浮点槽 F10）。
    let reg = builtin::registry().expect("内置注册表");
    let plan = plan_for_function(&RvTm::new(), &reg, "lp64d", &func).expect("引擎应能规划浮点参数");
    assert_eq!(place_name(&plan, 0), "X10");
    assert_eq!(place_name(&plan, 1), "F10");

    // 发射侧：现状 fail-closed（消息点名缺 MOVSD/MOVSS）。
    let err = FunctionCompiler::new(RvTm::new())
        .compile(&func)
        .expect_err("现状：riscv64 的浮点参数必须 fail-closed");
    let msg = err.to_string();
    assert!(msg.contains("MOVSD") || msg.contains("float args"), "{msg}");
}

fn place_name(plan: &forge_abi::AbiPlan, i: usize) -> String {
    match &plan.args[i].place {
        forge_abi::Placement::Reg { reg, .. } => reg.name.clone(),
        forge_abi::Placement::RegPair { lo, hi } => format!("{}:{}", lo.name, hi.name),
        other => panic!("{other:?}"),
    }
}

/// **plan → 运行时调用布局**（v20 A3b-2 的地基）：生成物与管线以后读
/// `machine::call_layout::CallLayout`（运行时**不依赖 forge-abi**），因此这条转换必须
/// 无损：寄存器折回 **(类, 类内号)**、隐藏 sret 指针、栈区尺寸、callee-saved 全都要对。
#[test]
fn plan_converts_into_the_runtime_call_layout() {
    use forge_codegen::pipeline::abi_target::call_layout;
    use forge_isa_runtime::machine::call_layout::{ArgPlace, RetPlace};

    let tm = TargetMachine::new();
    let reg = builtin::registry().expect("内置注册表");
    let func = win64_probe();
    let plan = plan_for_function(&tm, &reg, "win64", &func).expect("plan");
    let layout = call_layout(&plan, &tm);

    assert_eq!(layout.conv, "win64");
    assert_eq!(layout.shadow_bytes, 32);
    assert_eq!(layout.stack_align, 16);
    assert_eq!(layout.slot_bytes, 8);

    // 实参：RCX（GPR 类 1 号）、XMM1（FPR 类 1 号）。
    let rcx = layout.arg(0).expect("arg0");
    match &rcx.place {
        ArgPlace::Reg {
            class,
            index,
            ext,
            sret,
        } => {
            assert_eq!((*class, *index), (forge_ir::RegClass::GPR(8), 1));
            assert_eq!(*ext, forge_isa_runtime::machine::call_layout::Ext::None);
            assert!(!*sret, "普通参数不是 sret");
        }
        other => panic!("{other:?}"),
    }
    let xmm1 = layout.arg(1).expect("arg1");
    match &xmm1.place {
        ArgPlace::Reg { class, index, .. } => {
            assert_eq!((*class, *index), (forge_ir::RegClass::FPR(16), 1));
        }
        other => panic!("{other:?}"),
    }
    // 返回：RAX（GPR 类 0 号）。
    match layout.ret.as_ref().expect("ret") {
        RetPlace::Reg { class, index, .. } => {
            assert_eq!((*class, *index), (forge_ir::RegClass::GPR(8), 0))
        }
        other => panic!("{other:?}"),
    }
    // callee-saved 折成 (类, 号)：RBX = GPR 3 号（x86 的物理编号）。
    assert!(
        layout
            .callee_saved
            .iter()
            .any(|(c, i)| *c == forge_ir::RegClass::GPR(8) && *i == 3),
        "{:?}",
        layout.callee_saved
    );
    // 调用方视角的栈槽偏移 = shadow + k*slot。
    assert_eq!(layout.caller_offset(0), 32);
    assert_eq!(layout.caller_offset(2), 48);
}

/// **缺省约定也要能规划**（v20 A3b-2b-2 的入场券）：IR 的缺省约定是抽象的 `c`，
/// 它在真机上解析成**整套**平台约定（`win64` 的规则 + `win64` 的绑定，靠规则的
/// `aliases = ["c"]`）。这是"发射真的会切到布局"的前提——缺省约定规划不出来，
/// `call_layout` 就永远是 `None`，生成物只走旧路径（典型症状：改了生成器却没有一处生效）。
#[test]
fn the_default_convention_plans_and_reaches_the_frame_lowering() {
    use forge_codegen::FunctionCompiler;
    use forge_isa_runtime::machine::call_layout::ArgPlace;

    let ctx = TypeContext::new();
    let sig = FunctionSignature::new(&[(TypeId::I64, "n"), (TypeId::F64, "x")], &[TypeId::I64])
        .with_calling_convention(CallConvId::builtin(ConvName::C));
    let mut b = FunctionBuilder::new("c_probe", ctx, sig);
    let (entry, params) = b.create_block_with_params(&[(TypeId::I64, "n"), (TypeId::F64, "x")]);
    b.switch_to_block(entry);
    b.ret(&[params[0]]);
    let func = b.finish().expect("build");

    let (_cf, alloc) = FunctionCompiler::new(TargetMachine::new())
        .compile_with_alloc(&func)
        .expect("compile");
    let layout = alloc
        .call_layout
        .as_ref()
        .expect("缺省约定 `c` 必须能规划出布局（靠规则的 aliases）");
    // 解析成**整套** Win64：by_position 的槽位（第 2 个参数进 XMM1），而不是 `c` 自己的
    // by_class 规则（那会给出 XMM0，实测参数读错）。
    assert_eq!(layout.conv, "win64");
    match &layout.args[0].place {
        ArgPlace::Reg { class, index, .. } => {
            assert_eq!((*class, *index), (forge_ir::RegClass::GPR(8), 1))
        }
        other => panic!("{other:?}"),
    }
    match &layout.args[1].place {
        ArgPlace::Reg { class, index, .. } => {
            assert_eq!((*class, *index), (forge_ir::RegClass::FPR(16), 1), "XMM1")
        }
        other => panic!("{other:?}"),
    }
}

/// **序言/尾声侧的通路**（v20 A3b-2b）：`TargetFrameLowering::emit_prologue/epilogue` 只拿到
/// `AllocResult`（拿不到 `LowerCtx`），所以调用布局必须挂在 `AllocResult` 上——本测试钉住
/// "管线确实把它带到了那里"，否则"生成物改读 plan"在序/尾声这一半无从下手。
#[test]
fn alloc_result_carries_the_call_layout_for_the_frame_lowering() {
    use forge_codegen::FunctionCompiler;

    let compiler = FunctionCompiler::new(TargetMachine::new());
    let func = win64_probe();
    let (_cf, alloc) = compiler.compile_with_alloc(&func).expect("compile");

    let layout = alloc
        .call_layout
        .as_ref()
        .expect("管线必须把调用布局交给帧件");
    assert_eq!(layout.conv, "win64");
    assert_eq!(layout.shadow_bytes, 32);
    // 首个实参落在 RCX = (GPR(8), 1)——与 plan/既有路径一致。
    match &layout.args[0].place {
        forge_isa_runtime::machine::call_layout::ArgPlace::Reg { class, index, .. } => {
            assert_eq!((*class, *index), (forge_ir::RegClass::GPR(8), 1))
        }
        other => panic!("{other:?}"),
    }
    // callee-saved 也在里面（序言据此保存、尾声据此恢复）。
    assert!(!layout.callee_saved.is_empty());
}

/// **约定事实读 plan 的交叉核对**（v20 A5-3 ④ 的入场券）：把 regalloc / 帧布局 /
/// 发射三处从"读谱里声明的 `[abi]` 表"切到"读 plan"之前，必须先说清**两边的差异**。
///
/// `$equal_cs = true` = 该约定就是谱 `[abi]` 描述的那一份 ⇒ plan 与谱必须**逐项相等**；
/// `false` = 同一台机器上的**另一份**约定（x86 的 sysv64）⇒ 差异必须是**预期的那几项**。
///
/// 逐机器核对三件事：
///
/// 1. `callee_saved`：plan（绑定 `cs_gpr`）对**机器事实** `[machine].callee_saved_gpr`。相等的那三份
///    逐项同值；x86 的 sysv64 少 `RDI/RSI`②（win64 里它们是 callee-saved，sysv64 里是
///    **参数寄存器**）。plan 侧可能多出 FPR（AAPCS64 的 `cs_fpr`），但发射侧还没按类分派
///    保存（A6）⇒ 消费者只取 GPR 类，这里也按 GPR 类比。
/// 2. `frame_padding`：plan（规则字段）与机器事实 `[machine].frame_padding` 必须相等
///    （同一台机器的帧机制给出同一个值；规则侧那份随约定而变，是 plan 的来源）。
/// 3. `clobbers`：plan = 可用池 − callee-saved。它与谱曾经声明的 `[abi].call_clobbers`
///    允许差在**固定用途寄存器**上（riscv 的 X1/X3/X4、arm64 的 X18/X30）——那些寄存器
///    regalloc 根本分不到，差它们不影响正确性。两条硬不变量必须成立：① `clobbers` 与
///    `callee_saved` 不交；② 参数寄存器与返回寄存器都在 `clobbers` 里（跨调用存活值不能
///    留在参数寄存器上——x86 显式选 `sysv64` 时谱里那份**漏了 RDI/RSI**，读 plan 正好修掉）。
macro_rules! convention_facts_probe {
    ($name:ident, $tm:ty, $conv:literal, $probe:expr, $equal_cs:expr) => {
        #[test]
        fn $name() {
            use forge_codegen::pipeline::abi_target::call_layout;
            use forge_isa_runtime::machine::call_layout::ArgPlace;

            let tm = <$tm>::new();
            let reg = builtin::registry().expect("内置注册表");
            let func = $probe();
            let plan = plan_for_function(&tm, &reg, $conv, &func).expect("plan");
            let layout = call_layout(&plan, &tm);
            let ri = tm.reg_info();
            let gpr = ri.default_gpr_class();
            let idx = |v: &[(forge_ir::RegClass, u32)]| -> Vec<u32> {
                v.iter()
                    .filter(|(c, _)| *c == gpr)
                    .map(|&(_, i)| i)
                    .collect()
            };

            // ① callee-saved：plan 的 GPR 类项 vs 谱里声明的表。
            let plan_cs = idx(&layout.callee_saved);
            let spec_cs = ri.callee_saved();
            // **机器事实必须覆盖约定**（v20 A6 收口的护栏）：plan 的每个 GPR callee-saved
            // 都要在 `[machine].callee_saved_gpr`（= 生成物 `callee_saved()`，帧件实际会保存的
            // 那组）里——否则"某份约定要求保住 R15、机器帧却不存它"就是静默破坏。
            // 只核 GPR：FPR（AAPCS64 的 V8-V15）由 `class = "fpr"` 的 callee_save 角色承担，
            // 其数量由 plan 决定的 min_frame 保证装得下。
            for r in &plan_cs {
                assert!(
                    spec_cs.contains(r),
                    "{}：约定要求保住寄存器 {r}，但机器帧（[machine].callee_saved_gpr）不保存它",
                    $conv
                );
            }
            if $equal_cs {
                assert_eq!(
                    plan_cs, spec_cs,
                    "{}：plan（绑定 cs_gpr）与机器事实 [machine].callee_saved_gpr 必须同值",
                    $conv
                );
            } else {
                // 另一份约定：plan 必须是谱那份的**子集**——spec 是 ISA 主约定的口径，
                // 多出来的是"在这份约定里由 caller 保存"的寄存器（sysv64 的 RDI/RSI）。
                for r in &plan_cs {
                    assert!(
                        spec_cs.contains(r),
                        "{}：plan 的 callee-saved {r} 不该是谱表之外的新寄存器",
                        $conv
                    );
                }
                assert!(
                    plan_cs.len() < spec_cs.len(),
                    "{}：这份约定的 callee-saved 必须少于 ISA 主约定的那份（否则说明绑定没生效）",
                    $conv
                );
            }

            // ② 帧填充：plan（规则） == 谱。
            assert_eq!(
                layout.frame_padding,
                tm.abi().frame_padding(),
                "{}：plan 的 frame_padding 与机器事实 [machine].frame_padding 必须同值",
                $conv
            );

            // ②b 位置计数规则：plan（约定规则 `position`） == 机器事实
            //     `[machine].arg_slot`（生成物 `TargetABI::arg_placement`）。
            //     只对"ISA 的主约定"成立——同一台机器的另一份约定（x86 的 sysv64）
            //     可以是 by_class，那正是 `$equal_cs = false` 的第二种情形。
            //     这条守卫保证"无 plan 的回退形态"与"约定数据"不会分叉。
            if $equal_cs {
                let want = match tm.abi().arg_placement() {
                    forge_isa_runtime::machine::abi::ArgPlacement::ByClass => {
                        forge_abi::PositionRule::ByClass
                    }
                    forge_isa_runtime::machine::abi::ArgPlacement::ByPosition => {
                        forge_abi::PositionRule::ByPosition
                    }
                };
                assert_eq!(
                    plan.position, want,
                    "{}：plan 的 position 与机器事实 [machine].arg_slot 必须同值",
                    $conv
                );
            }

            // ③ clobbers 的两条硬不变量。
            let clobber_idx = idx(&layout.clobbers);
            assert!(!clobber_idx.is_empty(), "{}：破坏集不该为空", $conv);
            for cs in &plan_cs {
                assert!(
                    !clobber_idx.contains(cs),
                    "{}：callee-saved 的 {cs} 不能同时出现在破坏集里",
                    $conv
                );
            }
            // 首个整数参数落点必须在破坏集里（跨调用存活值不能留在它上面）。
            let first = layout
                .args
                .iter()
                .find_map(|a| match &a.place {
                    ArgPlace::Reg { class, index, .. } => Some((*class, *index)),
                    _ => None,
                })
                .expect("至少有首参落在寄存器");
            assert_eq!(first.0, gpr, "首参应是整数类");
            assert!(
                clobber_idx.contains(&first.1),
                "{}：首个整数参数寄存器 {} 必须在破坏集里",
                $conv,
                first.1
            );
            // 返回寄存器也在破坏集里（按名核对，不假设它的号）。
            let ret_name = match &plan.ret {
                forge_abi::RetLoc::Reg { reg } => reg.name.clone(),
                other => panic!("{other:?}"),
            };
            assert!(
                layout
                    .clobbers
                    .iter()
                    .zip(plan.clobbers.iter())
                    .any(|((c, i), r)| *c == gpr && r.name == ret_name && *i == r.index),
                "{}：返回寄存器 {ret_name} 必须在破坏集里",
                $conv
            );

            // ③b **破坏集与签名无关**（v20 A5-3）：宿主在函数规划失败时用**空签名**
            //     问一次引擎就能拿到正确的 clobbers（谱里的 `[abi].call_clobbers`
            //     已删除，`LowerCtx::conv_clobbers` 走的就是这条路）。
            let empty_sig = forge_abi::Signature::new(Vec::new(), None);
            let empty_plan = forge_codegen::pipeline::abi_target::plan_for_signature(
                &tm, &reg, $conv, &empty_sig,
            )
            .expect("空签名的 plan 必须算得出来（约定级事实）");
            assert_eq!(
                plan.clobbers, empty_plan.clobbers,
                "{}：clobbers 必须与签名无关（无 plan 回退路径的前提）",
                $conv
            );
        }
    };
}

// x86 Win64：整数参数 4 个（RCX/RDX/R8/R9），callee-saved 7 个（含 RDI/RSI）。
convention_facts_probe!(
    x86_win64_facts_match_the_spec,
    forge_codegen::arch::x86_v12::TargetMachine,
    "win64",
    win64_stack_args_probe,
    true
);

// x86 **SysV64**：同一台机器换约定——callee-saved 少 RDI/RSI，破坏集必须多出它们。
//
// 这条正是"读 plan"的价值：谱里那份 `[abi]`（callee-saved 表与 clobbers）是 **win64
// 口径**，拿它去编 sysv64 的函数会漏掉 RDI/RSI 的破坏，跨调用存活值留在上面会被
// callee 静默覆盖。本测试钉住"plan 给的是 sysv64 的口径"。
convention_facts_probe!(
    x86_sysv64_facts_match_the_spec,
    forge_codegen::arch::x86_v12::TargetMachine,
    "sysv64",
    sysv64_probe,
    false
);

// riscv64 LP64D：callee-saved = X9 + X18..X27（11 个）。
convention_facts_probe!(
    riscv64_lp64d_facts_match_the_spec,
    forge_codegen::arch::riscv64_v12::TargetMachine,
    "lp64d",
    lp64d_probe,
    true
);

// arm64 AAPCS64：callee-saved = X19..X28（10 个 GPR；FPR 的 V8-V15 只进 plan，
// 发射侧按类分派保存是 A6 的事，所以消费者按 GPR 类比）。
convention_facts_probe!(
    arm64_aapcs64_facts_match_the_spec,
    forge_codegen::arch::arm64_v12::TargetMachine,
    "aapcs64",
    aapcs64_probe,
    true
);

/// **plan 的破坏集覆盖整个 FP 文件**（v20 A5-3 ④ 实测，2026-09-26）：x86 的 plan 破坏集
/// 在 FP 侧列**全部 16 个 XMM**，比谱里曾经声明的（`[abi].call_clobbers` 缺省兜底给的
/// 4 个参数 XMM + XMM0）宽——物理上 plan 那份才对（Win64 的 XMM0-15 全 volatile）。该键
/// 已在 2026-09-27 删除（发射只认 plan / 宿主的约定级破坏集）。
///
/// 这份更宽的破坏集曾经**打错值**：`test_jit_v128_byval_return_lane3`（lane3 的 7.5
/// 读成 0）与 `test_jit_v128_byval_mixed_int_pos` 变红——根因是单结果 IR 值一律按
/// **池宽**成类（V128 也拿 FPR(8)），而 spill/reload 的宽度取自类宽 ⇒ 只搬 8 字节。
/// 已修在 `pipeline/lowering.rs`（向量按真实字节数成类 VEC(16/32/64)），所以现在
/// 发射侧消费 **plan 的全类破坏集**（不再按类过滤）。本测试钉住两条事实：
/// ① plan 的 FP 破坏集比谱里声明的宽（若某天变窄，说明引擎的"可用池"口径改了，
/// 要同步 A6 记录与 `lowering.rs` 的注释）；② GPR 侧随约定变（win64 5 个 vs sysv64 7 个）。
#[test]
fn x86_plan_clobbers_cover_the_whole_fp_file() {
    use forge_codegen::pipeline::abi_target::call_layout;

    let cases: [(&str, fn() -> Function); 2] = [
        ("win64", win64_stack_args_probe as fn() -> Function),
        ("sysv64", sysv64_probe as fn() -> Function),
    ];
    for (conv, probe) in cases {
        let tm = TargetMachine::new();
        let reg = builtin::registry().expect("内置注册表");
        let plan = plan_for_function(&tm, &reg, conv, &probe()).expect("plan");
        let layout = call_layout(&plan, &tm);
        let gpr = tm.reg_info().addr_class();

        let plan_fp = layout.clobbers.iter().filter(|(c, _)| *c != gpr).count();
        assert!(
            plan_fp > 4,
            "{conv}：plan 的 FP 破坏集应覆盖全部 XMM（比谱里声明的 4 个宽），实测 {plan_fp}"
        );
        let plan_gpr: Vec<u32> = layout
            .clobbers
            .iter()
            .filter(|(c, _)| *c == gpr)
            .map(|&(_, i)| i)
            .collect();
        let expected = if conv == "win64" { 5 } else { 7 };
        assert_eq!(plan_gpr.len(), expected, "{conv}：GPR 破坏集个数");
    }
}

/// `fn f(i64) -> i64`，用 `sysv64` 约定（x86 上的另一份平台约定）。
fn sysv64_probe() -> Function {
    let ctx = TypeContext::new();
    let sig = FunctionSignature::new(&[(TypeId::I64, "a")], &[TypeId::I64])
        .with_calling_convention(CallConvId::builtin(ConvName::SysV64));
    let mut b = FunctionBuilder::new("sysv64_probe", ctx, sig);
    let (entry, params) = b.create_block_with_params(&[(TypeId::I64, "a")]);
    b.switch_to_block(entry);
    b.ret(&[params[0]]);
    b.finish().expect("build")
}

/// `fn f(i64, f64, i64, f64) -> f64`，用给定约定。
///
/// 这条签名是「调用点 plan」最有信息量的样本：**位置计数**（win64）与**类计数**（lp64d）
/// 在它上面必然分叉（win64：a→RCX、b→XMM1、c→RDX、d→XMM3；lp64d：a→X10、b→F10、c→X11、d→F11）。
fn mixed_probe(cc: CallConvId) -> Function {
    let ctx = TypeContext::new();
    let params = [
        (TypeId::I64, "a"),
        (TypeId::F64, "b"),
        (TypeId::I64, "c"),
        (TypeId::F64, "d"),
    ];
    let sig = FunctionSignature::new(&params, &[TypeId::F64]).with_calling_convention(cc);
    let mut b = FunctionBuilder::new("mixed_probe", ctx, sig);
    let (entry, p) = b.create_block_with_params(&params);
    b.switch_to_block(entry);
    b.ret(&[p[3]]);
    b.finish().expect("build")
}

/// **形状 → plan 必须等于 函数 → plan**（v20 A5-3「调用点 plan」的入场券）。
///
/// 调用点在 lowering 时只有实参的**形状**（大小/对齐/族/成员，见
/// `machine::call_layout::ArgShape`），没有被调方的 `Function`；而"往哪个寄存器/栈槽搬"
/// 只有引擎算得出来。两条入口必须在**每一份约定**上给出逐项相同的落点——差一项就是调用方
/// 往错的地方搬值（不崩，静默错值），矩阵未必抓得到。
///
/// 比的是**落点/栈/clobber/callee-saved/hidden**，不比参数名（形状侧叫 `a0`、函数侧叫
/// `a`/`b`，名字只用于诊断）。
#[test]
fn shape_plan_matches_the_function_plan() {
    use forge_codegen::pipeline::abi_target::plan_for_shapes;
    use forge_isa_runtime::machine::call_layout::ArgShape;
    use forge_isa_runtime::machine::call_plan::CallRequest;

    /// 取出"必须逐项相同"的那几块（名字无关）。
    #[derive(Debug, PartialEq)]
    struct Cmp {
        args: Vec<forge_abi::Placement>,
        ret: forge_abi::RetLoc,
        stack: forge_abi::StackLayout,
        callee_saved: forge_abi::CalleeSavedPlan,
        hidden: forge_abi::HiddenSlots,
        clobbers: Vec<forge_abi::plan::RegRef>,
    }
    fn cmp(p: &forge_abi::AbiPlan) -> Cmp {
        Cmp {
            args: p.args.iter().map(|a| a.place.clone()).collect(),
            ret: p.ret.clone(),
            stack: p.stack.clone(),
            callee_saved: p.callee_saved.clone(),
            hidden: p.hidden.clone(),
            clobbers: p.clobbers.clone(),
        }
    }

    let reg = builtin::registry().expect("内置注册表");

    // ① x86 win64：6 个 i64（前 4 进寄存器、后 2 走栈）与混合签名。
    {
        let tm = forge_codegen::arch::x86_v12::TargetMachine::new();
        let cases: Vec<(&str, Function, Vec<ArgShape>, Option<ArgShape>)> = vec![
            (
                "win64 六整数（含栈参数）",
                win64_stack_args_probe(),
                vec![ArgShape::int(8, 8); 6],
                Some(ArgShape::int(8, 8)),
            ),
            (
                "win64 混合（位置计数）",
                mixed_probe(CallConvId::builtin(ConvName::Win64)),
                vec![
                    ArgShape::int(8, 8),
                    ArgShape::float(8),
                    ArgShape::int(8, 8),
                    ArgShape::float(8),
                ],
                Some(ArgShape::float(8)),
            ),
        ];
        for (what, func, args, ret) in cases {
            let by_func = plan_for_function(&tm, &reg, "win64", &func).expect("函数 plan");
            let by_shape = plan_for_shapes(
                &tm,
                &reg,
                &CallRequest::new("win64").args(args).rets(ret_list(&ret)),
            )
            .expect("形状 plan");
            assert_eq!(
                cmp(&by_shape),
                cmp(&by_func),
                "{what}：形状 plan ≠ 函数 plan"
            );
        }
    }

    // ② riscv lp64d：按类计数——混合签名在这里与 win64 分叉（同一条签名、两种约定）。
    {
        let tm = forge_codegen::arch::riscv64_v12::TargetMachine::new();
        let func = mixed_probe(CallConvId::builtin(ConvName::Lp64d));
        let args = vec![
            ArgShape::int(8, 8),
            ArgShape::float(8),
            ArgShape::int(8, 8),
            ArgShape::float(8),
        ];
        let by_func = plan_for_function(&tm, &reg, "lp64d", &func).expect("函数 plan");
        let by_shape = plan_for_shapes(
            &tm,
            &reg,
            &CallRequest::new("lp64d")
                .args(args)
                .rets([ArgShape::float(8)]),
        )
        .expect("形状 plan");
        assert_eq!(
            cmp(&by_shape),
            cmp(&by_func),
            "lp64d：形状 plan ≠ 函数 plan"
        );
    }

    // ③ arm64 aapcs64：整型签名（浮点在 arm64 上缺 FPR 寄存器组——那条缺口另有用例钉住）。
    {
        let tm = forge_codegen::arch::arm64_v12::TargetMachine::new();
        let func = aapcs64_probe();
        let args = vec![ArgShape::int(8, 8), ArgShape::int(8, 8)];
        let by_func = plan_for_function(&tm, &reg, "aapcs64", &func).expect("函数 plan");
        let by_shape = plan_for_shapes(
            &tm,
            &reg,
            &CallRequest::new("aapcs64")
                .args(args)
                .rets([ArgShape::int(8, 8)]),
        )
        .expect("形状 plan");
        assert_eq!(
            cmp(&by_shape),
            cmp(&by_func),
            "aapcs64：形状 plan ≠ 函数 plan"
        );
    }
}

/// **注册表路径可用**（v20 A5-3）：宿主注册后，生成物在调用点按 `(ISA, 约定, 形状)`
/// 就能拿到与"直接算"逐项相同的布局；未注册的 ISA ⇒ `None`（fail-closed，不猜落点）。
///
/// 这条是"调用方改按被调方落点搬实参"的唯一查表入口的守卫。
#[test]
fn call_planner_registry_serves_the_shape_plan() {
    use forge_codegen::pipeline::abi_target::{call_layout, plan_for_shapes};
    use forge_isa_runtime::machine::call_layout::ArgShape;
    use forge_isa_runtime::machine::call_plan::CallRequest;
    use forge_isa_runtime::machine::call_plan::{has_call_planner, plan_call};

    forge_codegen::pipeline_hooks::ensure_registered();
    assert!(
        has_call_planner("x86_64_v12"),
        "宿主注册后应能查到调用点布局钩子"
    );

    let reg = builtin::registry().expect("内置注册表");
    let tm = TargetMachine::new();
    let args = vec![
        ArgShape::int(8, 8),
        ArgShape::float(8),
        ArgShape::int(8, 8),
        ArgShape::float(8),
    ];
    let ret = Some(ArgShape::float(8));

    let by_hook = plan_call(
        "x86_64_v12",
        &CallRequest::new("win64")
            .args(args.iter())
            .rets(ret_list(&ret)),
    )
    .expect("钩子应给出布局");
    let plan = plan_for_shapes(
        &tm,
        &reg,
        &CallRequest::new("win64")
            .args(args.iter())
            .rets(ret_list(&ret)),
    )
    .expect("直接算");
    assert_eq!(
        by_hook,
        call_layout(&plan, &tm),
        "钩子给出的布局必须与「形状 → plan → CallLayout」逐项相同"
    );

    // 未注册的 ISA ⇒ **点名原因**（`NoPlanner`），不再是笼统的 `None`：调用点据此能直接
    // 知道"宿主没接 planner"，而不是"某个签名算不出来"。
    assert_eq!(
        plan_call(
            "nope_isa",
            &CallRequest::new("win64").args(args).rets(ret_list(&ret))
        ),
        Err(
            forge_isa_runtime::machine::call_plan::CallPlanError::NoPlanner {
                isa: "nope_isa".into()
            }
        )
    );
}

/// **约定级整数返回槽**（v20 A5-3）：`plan_for_shapes(空参, 整数返回)` 给出的返回寄存器就是
/// `LowerCtx::conv_ret_gpr` 的来源，也正是 `[abi].ret_regs` 删除前谱里声明的那些值
/// （x86 `RAX`、riscv `X10` = a0、arm64 `X0`）——`Call` 读**被调方**返回值靠它。
#[test]
fn convention_level_int_return_slot() {
    use forge_codegen::pipeline::abi_target::plan_for_shapes;
    use forge_isa_runtime::machine::call_layout::ArgShape;
    use forge_isa_runtime::machine::call_plan::CallRequest;

    let reg = builtin::registry().expect("内置注册表");
    let ret_shape = [ArgShape::int(8, 8)];

    // x86 win64：RAX。
    {
        let tm = TargetMachine::new();
        let p = plan_for_shapes(&tm, &reg, &CallRequest::new("win64").rets(ret_shape.iter()))
            .expect("win64 plan");
        match p.ret {
            forge_abi::RetLoc::Reg { reg } => assert_eq!(reg.name, "RAX"),
            other => panic!("{other:?}"),
        }
    }
    // riscv lp64d：a0 = X10（**不是 index 0**——这正是谱里那份 `ret_regs` 存在的理由）。
    {
        let tm = forge_codegen::arch::riscv64_v12::TargetMachine::new();
        let p = plan_for_shapes(&tm, &reg, &CallRequest::new("lp64d").rets(ret_shape.iter()))
            .expect("lp64d plan");
        match p.ret {
            forge_abi::RetLoc::Reg { reg } => assert_eq!(reg.name, "X10"),
            other => panic!("{other:?}"),
        }
    }
    // arm64 aapcs64：x0。
    {
        let tm = forge_codegen::arch::arm64_v12::TargetMachine::new();
        let p = plan_for_shapes(
            &tm,
            &reg,
            &CallRequest::new("aapcs64").rets(ret_shape.iter()),
        )
        .expect("aapcs64 plan");
        match p.ret {
            forge_abi::RetLoc::Reg { reg } => assert_eq!(reg.name, "X0"),
            other => panic!("{other:?}"),
        }
    }
}

/// `fn f(i64, i64) -> i64`，用 `aapcs64` 约定（arm64 后端）。
fn aapcs64_probe() -> Function {
    let ctx = TypeContext::new();
    let sig = FunctionSignature::new(&[(TypeId::I64, "a"), (TypeId::I64, "b")], &[TypeId::I64])
        .with_calling_convention(CallConvId::builtin(ConvName::Aapcs64));
    let mut b = FunctionBuilder::new("aapcs64_probe", ctx, sig);
    let (entry, params) = b.create_block_with_params(&[(TypeId::I64, "a"), (TypeId::I64, "b")]);
    b.switch_to_block(entry);
    b.ret(&[params[0]]);
    b.finish().expect("build")
}

/// **A6 ① 缺口清点**（2026-09-29 实测）：三台真机 × 各自的主约定，用一组**代表性形状**
/// 跑一遍引擎——**只剩一条算不出 plan**：AAPCS64 的 4×f32 HFA 返回（≥3 槽返回的搬运，
/// 引擎按 `RetLoc` 只有单寄存器/双寄存器两种形态，A6 未实现）。
///
/// 其余曾经被列为"规划缺口"的形状（`Pair`（2 槽聚合）、**无指针的 `Indirect`**（by-ref
/// 指针本身溢出到栈）、按引用向量、参数溢出到栈、浮点溢出到栈、混合两套寄存器文件）
/// **都已有 plan**——所以 A6 ① 的"补齐每条发射路径"实际只剩 HFA 返回这一条。
///
/// 形状覆盖：标量混合、按值/按引用向量、HFA（2/4 成员）、2 槽聚合（RegPair）、≥3 槽返回、
/// 24 字节聚合（必然间接）、参数溢出到栈、by-ref 指针本身也在栈上（`Indirect { ptr: None }`）、
/// 浮点溢出到栈。断言是**精确**的：缺口集必须恰好等于下面这条，多一条少一条都要来改这里
/// （少了 = 缺口已关，顺带更新方案文档的 A6）。
#[test]
fn a6_gap_inventory() {
    use forge_codegen::arch::arm64_v12::TargetMachine as Arm64;
    use forge_codegen::arch::riscv64_v12::TargetMachine as Riscv64;
    use forge_codegen::pipeline::abi_target::plan_for_shapes;
    use forge_isa_runtime::machine::call_layout::{ArgShape, ShapeKind};
    use forge_isa_runtime::machine::call_plan::CallRequest;

    fn agg(size: u32, align: u32, members: Vec<ArgShape>) -> ArgShape {
        ArgShape {
            size,
            align,
            kind: ShapeKind::Aggregate { members },
        }
    }
    fn vec(size: u32, align: u32, lanes: u32, elem_is_float: bool) -> ArgShape {
        ArgShape {
            size,
            align,
            kind: ShapeKind::Vector {
                elem_is_float,
                lanes,
                // 元素宽度在这里不参与判定（`shape_to_ty` 用真实 size/align + 元素族），
                // 探针一律按 4 字节元素给。
                elem_bytes: 4,
            },
        }
    }

    let i64s = ArgShape::int(8, 8);
    let f64s = ArgShape::float(8);
    let f32s = ArgShape::float(4);
    let v128 = vec(16, 16, 4, true);
    let v256 = vec(32, 32, 8, true);
    let hfa2 = agg(16, 8, vec![f64s.clone(), f64s.clone()]);
    let hfa4 = agg(16, 4, vec![f32s.clone(); 4]);
    let pair = agg(16, 8, vec![i64s.clone(), i64s.clone()]);
    let agg24 = agg(24, 8, vec![i64s.clone(); 3]);

    let cases: Vec<(&str, Vec<ArgShape>, Option<ArgShape>)> = vec![
        (
            "scalars_mix",
            vec![i64s.clone(), f64s.clone(), f32s.clone()],
            Some(i64s.clone()),
        ),
        ("byval_vec16", vec![v128.clone()], Some(i64s.clone())),
        ("byref_vec32", vec![v256.clone()], Some(i64s.clone())),
        ("ret_vec16", vec![], Some(v128.clone())),
        ("ret_vec32", vec![], Some(v256.clone())),
        ("hfa2_arg", vec![hfa2.clone()], Some(f64s.clone())),
        ("hfa4_arg", vec![hfa4.clone()], Some(f32s.clone())),
        ("hfa4_ret", vec![], Some(hfa4.clone())),
        ("pair_arg", vec![pair.clone()], Some(i64s.clone())),
        ("pair_ret", vec![], Some(pair.clone())),
        ("agg24_arg", vec![agg24.clone()], Some(i64s.clone())),
        ("agg24_ret", vec![], Some(agg24.clone())),
        ("many_int_args", vec![i64s.clone(); 8], Some(i64s.clone())),
        (
            "byref_ptr_on_stack",
            vec![
                i64s.clone(),
                i64s.clone(),
                i64s.clone(),
                i64s.clone(),
                v256.clone(),
            ],
            Some(i64s.clone()),
        ),
        ("many_floats", vec![f64s.clone(); 9], Some(f64s.clone())),
        (
            "mixed_over_both_files",
            vec![
                i64s.clone(),
                f64s.clone(),
                i64s.clone(),
                f64s.clone(),
                i64s.clone(),
                f64s.clone(),
                i64s.clone(),
                f64s.clone(),
            ],
            Some(i64s.clone()),
        ),
    ];

    let reg = builtin::registry().expect("内置注册表");
    let mut gaps: Vec<String> = Vec::new();
    macro_rules! probe {
        ($isa:expr, $conv:expr, $tm:expr) => {
            for (name, args, ret) in &cases {
                match plan_for_shapes(
                    $tm,
                    &reg,
                    &CallRequest::new($conv).args(args).rets(ret_list(ret)),
                ) {
                    Ok(_) => {}
                    Err(e) => gaps.push(format!("{} / {} / {}: {}", $isa, $conv, name, e)),
                }
            }
        };
    }
    let x86 = TargetMachine::new();
    probe!("x86_64_v12", "win64", &x86);
    probe!("x86_64_v12", "sysv64", &x86);
    let rv = Riscv64::new();
    probe!("riscv64_v12", "lp64d", &rv);
    let a64 = Arm64::new();
    probe!("arm64_v12", "aapcs64", &a64);

    // 精确断言：缺口恰好是"arm64 的 4×f32 HFA 返回"这一条。
    let hfa4_only = gaps.len() == 1 && gaps[0].starts_with("arm64_v12 / aapcs64 / hfa4_ret:");
    assert!(
        hfa4_only,
        "A6 ① 的缺口面变了（{} 条）：\n{}\n——少了 = 缺口已关（顺带更新方案文档 A6），\
         多了 = 有新的规划缺口，先补引擎再谈 fail-closed",
        gaps.len(),
        gaps.join("\n")
    );
}

/// `Option<ArgShape>` → 返回形状切片（兼容旧写法的测试辅助）。
fn ret_list(
    ret: &Option<forge_isa_runtime::machine::call_layout::ArgShape>,
) -> Vec<forge_isa_runtime::machine::call_layout::ArgShape> {
    ret.iter().cloned().collect()
}
/// `fn f(i64) -> (i64, i64)`：**多值返回**（v20 A6）的 IR 形态，用 `win64` 约定。
///
/// 这条函数形态就是 forge-rustc 给 ScalarPair 生成的 IR（两个独立返回值）。
fn pair_return_probe() -> Function {
    let ctx = TypeContext::new();
    let sig = FunctionSignature::new(&[(TypeId::I64, "n")], &[TypeId::I64, TypeId::I64])
        .with_calling_convention(CallConvId::builtin(ConvName::Win64));
    let mut b = FunctionBuilder::new("pair_return_probe", ctx, sig);
    let (entry, params) = b.create_block_with_params(&[(TypeId::I64, "n")]);
    b.switch_to_block(entry);
    b.ret(&[params[0], params[0]]);
    b.finish().expect("build")
}

/// **多值返回落点由 plan 给**（v20 A6）：两个返回值 → `RAX:RDX`（`RegPair`），
/// 而不是旧实现的"第一个进默认返回槽 + 写死类内号 1"。
///
/// 写死类内号 1 在 x86 上碰巧对（RDX），在 riscv 上会落到 X1（= ra）——所以这条守卫的
/// 价值不在 x86 本身，而在于它把"第二个返回值在哪"从**发射侧常量**变成**引擎产物**。
#[test]
fn two_value_return_is_plan_driven() {
    use forge_codegen::FunctionCompiler;
    use forge_isa_runtime::machine::call_layout::RetPlace;

    let compiler = FunctionCompiler::new(TargetMachine::new());
    let func = pair_return_probe();
    let (_cf, alloc) = compiler
        .compile_with_alloc(&func)
        .expect("两个返回值的函数必须能编");
    let layout = alloc.call_layout.as_ref().expect("编译入口必须给布局");
    assert_eq!(
        layout.ret,
        Some(RetPlace::Pair {
            lo: (forge_ir::RegClass::GPR(8), 0),
            hi: (forge_ir::RegClass::GPR(8), 2),
        }),
        "两个 i64 返回值 → RAX:RDX（由绑定的 ret_int 池给，不写死类内号）"
    );
}

/// `fn f({f64, f64}) -> ()`：**HFA2 聚合实参**（AAPCS64 走 V0:V1，不是 X0:X1）。
///
/// 用来证明**调用点的形状投影必须摊开成员**：只报 size/align 时调用点会算成整数槽
/// （X0:X1），与被调方的函数级 plan（V0:V1）分叉——实参直接搬错寄存器。
fn hfa2_arg_probe() -> Function {
    let ctx = TypeContext::new();
    let st = ctx
        .borrow_mut()
        .struct_anon(vec![TypeId::F64, TypeId::F64], false);
    let sig = FunctionSignature::new(&[(st, "a")], &[])
        .with_calling_convention(CallConvId::builtin(ConvName::Aapcs64));
    let mut b = FunctionBuilder::new("hfa2_arg_probe", ctx, sig);
    let (entry, _params) = b.create_block_with_params(&[(st, "a")]);
    b.switch_to_block(entry);
    b.ret(&[]);
    b.finish().expect("build")
}

/// **HFA 聚合实参：调用点形状 == 被调方 plan**（v20 A6）。
///
/// 这条守的是"形状里必须有成员"：`ArgShape::from_ir_type` 递归摊开成员后，
/// `{f64,f64}` 才被判成 HFA 并落到**浮点池**；只报 size/align 的旧投影会落整数槽。
#[test]
fn hfa_aggregate_shape_matches_the_callee_plan() {
    use forge_abi::Placement;
    use forge_codegen::pipeline::abi_target::plan_for_shapes;
    use forge_isa_runtime::machine::call_layout::ArgShape;
    use forge_isa_runtime::machine::call_plan::CallRequest;

    let reg = builtin::registry().expect("内置注册表");
    let tm = forge_codegen::arch::arm64_v12::TargetMachine::new();
    let func = hfa2_arg_probe();
    let by_func = plan_for_function(&tm, &reg, "aapcs64", &func).expect("函数 plan");

    // 调用点视角：只有 IR 类型 ⇒ 用生成物走的同一条投影。
    let store = func.types.borrow();
    let sig = store.signature_opt(func.signature).expect("签名在类型表里");
    let st = sig.params[0].0;
    let shape = ArgShape::from_ir_type(&store, st);
    let (size, align) = (shape.size, shape.align);
    assert!(
        matches!(
            shape.kind,
            forge_isa_runtime::machine::call_layout::ShapeKind::Aggregate { ref members }
                if members.len() == 2
        ),
        "聚合形状必须带 2 个成员（HFA 判定靠它）：{shape:?}"
    );
    let by_shape =
        plan_for_shapes(&tm, &reg, &CallRequest::new("aapcs64").args([shape])).expect("形状 plan");
    assert_eq!(
        by_shape.args[0].place, by_func.args[0].place,
        "HFA 聚合实参：调用点 plan 必须与被调方 plan 逐项相同"
    );
    match &by_shape.args[0].place {
        Placement::RegPair { lo, hi } => assert_eq!(
            (lo.name.as_str(), hi.name.as_str()),
            ("V0", "V1"),
            "HFA 走浮点池"
        ),
        other => panic!("{other:?}"),
    }

    // **反证（这条守卫得有用）**：成员留空的形状——修复前的调用点投影——会落到整数槽，
    // 与被调方分叉。一旦有人把"摊成员"改回去，上面的断言会红，而这一条说明红的是真问题
    // （实参搬错寄存器），不是快照漂移。
    let blind = forge_isa_runtime::machine::call_layout::ArgShape {
        size,
        align,
        kind: forge_isa_runtime::machine::call_layout::ShapeKind::Aggregate {
            members: Vec::new(),
        },
    };
    let by_blind =
        plan_for_shapes(&tm, &reg, &CallRequest::new("aapcs64").args([blind])).expect("形状 plan");
    assert_ne!(
        by_blind.args[0].place, by_func.args[0].place,
        "成员留空的投影必须与真实 plan 分叉（这正是它错的地方）"
    );
}

/// **调用点的变参提示决定未命名实参的落点**（变参 D6，2026-09-30 落地）。
///
/// 背景：调用点只看得见实参**形状**，而 win64 的 `variadic_stack_only = true` 会把**未命名
/// 实参**从寄存器改判到栈——"哪几个实参是未命名的"只有知道被调方签名才判得出来。于是
/// `plan_call`/`plan_for_shapes` 多了一位 `variadic: Option<(bool, 命名个数)>`，由宿主查
/// **模块级签名表**（`LowerCtx::module_sigs`，JIT 在 `compile_module` 里填）得到。
///
/// 这条守卫钉三件事：① 不给提示 ⇒ 按非变参发（第 2/3 个进 RDX/R8）；② 给提示 ⇒ 未命名
/// 实参**走栈**；③ 被调方视角（IR 的 LLVM 形态：签名只列命名参数）确实算出 `va_area`。
#[test]
fn call_site_variadic_hint_decides_unnamed_argument_placement() {
    use forge_codegen::pipeline::abi_target::{plan_for_shapes, plan_for_signature};
    use forge_isa_runtime::machine::call_layout::{ArgPlace, ArgShape};
    use forge_isa_runtime::machine::call_plan::CallRequest;

    let reg = builtin::registry().expect("内置注册表");
    let tm = TargetMachine::new();
    let shapes = [
        ArgShape::int(8, 8),
        ArgShape::int(8, 8),
        ArgShape::int(8, 8),
    ];

    // ① 没有变参提示（单函数编译 / 被调方不是变参）：三个实参都进寄存器。
    let by_shape = plan_for_shapes(&tm, &reg, &CallRequest::new("win64").args(shapes.iter()))
        .expect("形状 plan");
    assert!(
        by_shape.args[1..]
            .iter()
            .all(|a| matches!(a.place, forge_abi::Placement::Reg { .. })),
        "非变参语义下第 2/3 个实参在寄存器：{:?}",
        by_shape.args
    );
    let layout = forge_codegen::pipeline::abi_target::call_layout(&by_shape, &tm);
    assert!(matches!(
        layout.arg(1).map(|a| &a.place),
        Some(ArgPlace::Reg { .. })
    ));

    // ② 给了变参提示（被调方是变参、命名 1 个）：第 2/3 个实参改判到**栈**。
    let hinted = plan_for_shapes(
        &tm,
        &reg,
        &CallRequest::new("win64").args(shapes.iter()).variadic(1),
    )
    .expect("变参形状 plan");
    assert!(
        hinted.args[1..]
            .iter()
            .all(|a| matches!(a.place, forge_abi::Placement::Stack { .. })),
        "win64 的未命名实参必须走栈：{:?}",
        hinted.args
    );
    let hinted_layout = forge_codegen::pipeline::abi_target::call_layout(&hinted, &tm);
    assert!(matches!(
        hinted_layout.arg(1).map(|a| &a.place),
        Some(ArgPlace::Stack { .. })
    ));

    // ③ 被调方视角：同一个三实参调用，签名路径（`.variadic(1)`）与 ② 一致 —— 说明**提示
    //    把调用点摆到了与被调方相同的语义上**（在此之前两条路径会分叉）。
    let sig = forge_abi::Signature::new(
        vec![
            ("a".into(), forge_abi::TyView::int(8, 8)),
            ("b".into(), forge_abi::TyView::int(8, 8)),
            ("c".into(), forge_abi::TyView::int(8, 8)),
        ],
        None,
    )
    .variadic(1);
    let by_sig = plan_for_signature(&tm, &reg, "win64", &sig).expect("变参 plan");
    assert_eq!(
        hinted.args[1].place, by_sig.args[1].place,
        "调用点（给了变参提示）与被调方必须给出同一落点"
    );
}

/// **失败必须带诊断**（人体工学）：`plan_call` 不再返回 `Option` 把原因丢掉，而是
/// [`CallPlanError`]——未注册的 ISA 是 `NoPlanner`（点名宿主该做什么），算了但失败是
/// `Failed`（正文来自宿主/引擎，能一直追到"池不够 / 缺绑定 / 能力缺口"）。
///
/// 这条守卫就是在钉"错误正文不许退化成一句话"：`Failed` 的 `why` 必须带引擎的原文特征。
#[test]
fn call_plan_errors_carry_the_reason() {
    use forge_codegen::pipeline::abi_target::register_isa_call_planner;
    use forge_isa_runtime::machine::call_layout::ArgShape;
    use forge_isa_runtime::machine::call_plan::{CallPlanError, CallRequest, plan_call};

    // ① 没注册 planner 的 ISA：点名"宿主没接"，不是"签名算不出来"。
    let e = plan_call("no_such_isa", &CallRequest::new("win64")).unwrap_err();
    assert_eq!(
        e,
        CallPlanError::NoPlanner {
            isa: "no_such_isa".into()
        }
    );
    let msg = e.to_string();
    assert!(
        msg.contains("register_call_planner") && msg.contains("ensure_registered"),
        "NoPlanner 要给两条出路：{msg}"
    );

    // ② 注册了 planner、但约定未注册 ⇒ `Failed`，正文带引擎的解释与自查命令。
    register_isa_call_planner("doc_probe_isa", TargetMachine::new());
    let e = plan_call("doc_probe_isa", &CallRequest::new("nope_conv")).unwrap_err();
    match &e {
        CallPlanError::Failed { conv, why } => {
            assert_eq!(conv, "nope_conv");
            assert!(
                why.contains("nope_conv") && why.contains("未注册"),
                "Failed 的正文必须来自引擎（点名约定 + 原因）：{why}"
            );
        }
        other => panic!("应为 Failed：{other:?}"),
    }
    let msg = e.to_string();
    assert!(
        msg.contains("forge-isa abi check") && msg.contains("calling-conventions.md"),
        "Failed 要给自查命令与文档路径：{msg}"
    );

    // ③ 同一份请求走通了：确认上面的失败来自"约定未注册"，不是 ISA 本身不可用。
    let ok = plan_call(
        "doc_probe_isa",
        &CallRequest::new("win64").rets([ArgShape::int(8, 8)]),
    );
    assert!(ok.is_ok(), "win64 在 x86 机器上应能规划：{ok:?}");
}

/// **变参信息进运行时镜像**（v20 变参 V2/V6）：引擎的 `VaArea`（形状数据）必须原样折到
/// `CallLayout.va`——被调方物化 `va_list` 与取值都按它跑**同一套算法**。
///
/// 三件事：① win64 是**地址式单游标**（没有基址、没有溢出区 ⇒ 代码里没有"栈式分支"，
/// 只有数据）；② sysv64 是**计数式游标 + 溢出区 + 保存区**；③ 非变参签名不带 `va`（不猜）。
#[test]
fn call_layout_mirrors_the_variadic_shape() {
    use forge_codegen::pipeline::abi_target::{call_layout, plan_for_signature};

    let reg = builtin::registry().expect("内置注册表");
    let tm = TargetMachine::new();
    let sig = forge_abi::Signature::new(
        vec![
            ("fmt".into(), forge_abi::TyView::ptr(8)),
            ("x".into(), forge_abi::TyView::float(8)),
        ],
        None,
    )
    .variadic(2); // 两个形参都是**命名**的（未命名实参由调用方多传 ⇒ 不在签名里）
    let plan = plan_for_signature(&tm, &reg, "win64", &sig).expect("win64 变参 plan");
    let va = call_layout(&plan, &tm).va.expect("变参必有 va 信息");
    assert_eq!(va.shape.as_deref(), Some("win64_stack"));
    assert_eq!((va.size, va.align), (8, 8));
    assert!(va.stack_only, "win64 的未命名实参只走栈");
    assert_eq!(
        (va.arg_rules.int.base, va.arg_rules.int.overflow),
        (None, None),
        "win64 的游标就是栈上实参的地址（没有基址、没有溢出区）"
    );
    assert_eq!(va.arg_rules.int.step, 8, "推进一个槽");
    // 字段布局与保存区也跟着镜像（v20 V3）：win64 对象只有 1 个游标字段、不需要保存区。
    assert_eq!(
        va.fields
            .iter()
            .map(|f| (f.offset, f.size))
            .collect::<Vec<_>>(),
        vec![(0, 8)],
        "win64 的 va_list 对象 = 1 个指针字段"
    );
    assert!(va.save.is_none(), "win64 栈式不需要寄存器保存区");
    assert_eq!(
        va.init.as_ref().map(|i| i.fields.clone()),
        Some(vec![
            forge_isa_runtime::machine::call_layout::VaInitVal::FrameOff(48)
        ]),
        "win64 的对象字段 0 = 未命名实参区地址（first_arg_offset 16 + shadow 32）"
    );

    // ② sysv64：计数式游标（上界 = 本类保存区字节数）+ 溢出区 + 保存区。
    let plan = plan_for_signature(&tm, &reg, "sysv64", &sig).expect("sysv64 变参 plan");
    let va = call_layout(&plan, &tm).va.expect("变参必有 va 信息");
    assert_eq!(va.shape.as_deref(), Some("sysv_reg_save"));
    assert_eq!((va.size, va.align), (24, 8));
    assert!(!va.stack_only, "SysV 的未命名实参继续用寄存器");
    assert_eq!(
        (
            va.arg_rules.int.cursor,
            va.arg_rules.int.base,
            va.arg_rules.int.overflow,
            va.arg_rules.int.limit,
        ),
        (0, Some(3), Some(2), 48),
        "整数游标 = gp_offset（上限 48 = 6 个 GP 槽），基址 = reg_save_area，溢出 = overflow_arg_area"
    );
    assert_eq!(
        (
            va.arg_rules.float.cursor,
            va.arg_rules.float.base,
            va.arg_rules.float.limit,
            va.arg_rules.float.step,
            va.arg_rules.float.cursor_origin,
        ),
        (1, Some(3), 176, 16, 48),
        "浮点游标 = fp_offset（从 GP 区之后 48 起、到保存区末尾 176），步长 = 16 字节槽"
    );
    // 字段布局（4 字段）与保存区槽表（6 GP ×8 + 8 XMM ×16 = 176、对齐 16）都要镜像过来：
    // `va_arg` 的游标与序言 spill 都按这些数字走，漏镜像 = 运行时算错偏移。
    assert_eq!(
        va.fields
            .iter()
            .map(|f| (f.offset, f.size))
            .collect::<Vec<_>>(),
        vec![(0, 4), (4, 4), (8, 8), (16, 8)],
        "sysv64 的 va_list = gp/fp offset + overflow_arg_area + reg_save_area"
    );
    let save = va.save.as_ref().expect("sysv64 需要寄存器保存区");
    assert_eq!((save.size, save.align), (176, 16));
    assert_eq!(save.slots.len(), 14, "6 GP + 8 XMM");
    // GP 块：0,8,…,40 各 8 字节；接着 FP 块：48,64,…,160 各 16 字节（psABI 的槽宽，
    // **不是**寄存器类宽）。索引只在类内做区分，这里断言"偏移/宽度"这条真正驱动
    // 序言 spill 与 `va_arg` 游标的数字。
    let got: Vec<(u32, u32)> = save.slots.iter().map(|s| (s.offset, s.size)).collect();
    assert_eq!(
        got,
        vec![
            (0, 8),
            (8, 8),
            (16, 8),
            (24, 8),
            (32, 8),
            (40, 8),
            (48, 16),
            (64, 16),
            (80, 16),
            (96, 16),
            (112, 16),
            (128, 16),
            (144, 16),
            (160, 16),
        ],
        "sysv64 保存区：6 GP（8 字节槽）+ 8 XMM（16 字节槽）"
    );

    // ③ 非变参签名：没有 va 信息。
    let plain = forge_abi::Signature::new(vec![("a".into(), forge_abi::TyView::int(8, 8))], None);
    let plan = plan_for_signature(&tm, &reg, "win64", &plain).expect("非变参 plan");
    assert!(call_layout(&plan, &tm).va.is_none(), "非变参不该带 va 信息");
}
