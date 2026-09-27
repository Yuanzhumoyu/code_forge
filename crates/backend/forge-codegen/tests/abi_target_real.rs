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

/// **栈参数的偏移核对**（v20 A3b-2b-2c 的入场券）：布局给的 `Stack { offset }` 必须与
/// **现有发射路径**（`@move_args` 的 `[abi.stack_args]` 算式）**同值**：
/// `first_offset_slots × slot + shadow + (pos − n_int) × stride × slot`。
///
/// 这是"把栈参数收参也切到布局"之前唯一能先做的正确性检查：**偏移差一**就是读错值
/// （不是崩，而是静默错值），矩阵未必抓得到，必须先钉住。
#[test]
fn stack_arg_offsets_agree_with_the_legacy_formula() {
    use forge_isa_runtime::machine::call_layout::ArgPlace;

    let tm = TargetMachine::new();
    let reg = builtin::registry().expect("内置注册表");
    let func = win64_stack_args_probe();
    let plan = plan_for_function(&tm, &reg, "win64", &func).expect("plan");
    let layout = forge_codegen::pipeline::abi_target::call_layout(&plan, &tm);

    // 现有路径的口径（x86 谱）：first_offset_slots = 2、shadow = 32、stride = 1、slot = 8。
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
                    "第 {i} 个参数的栈偏移：布局 {offset} ≠ 现有算式 {expect}"
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
/// 1. `callee_saved`：plan（绑定 `cs_gpr`）对谱 `[abi.callee_saved].gpr`。相等的那三份
///    逐项同值；x86 的 sysv64 少 `RDI/RSI`②（win64 里它们是 callee-saved，sysv64 里是
///    **参数寄存器**）。plan 侧可能多出 FPR（AAPCS64 的 `cs_fpr`），但发射侧还没按类分派
///    保存（A6）⇒ 消费者只取 GPR 类，这里也按 GPR 类比。
/// 2. `frame_padding`：plan（规则字段）与机器事实 `[machine].frame_padding` 必须相等
///    （同一台机器的帧机制给出同一个值；规则侧那份随约定而变，是 plan 的来源）。
/// 3. `clobbers`：plan = 可用池 − callee-saved。它与谱声明的 `[abi].call_clobbers`
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
                    "{}：plan（绑定 cs_gpr）与谱 [abi.callee_saved].gpr 必须同值",
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
/// 在 FP 侧列**全部 16 个 XMM**，比谱里声明的（`[abi].call_clobbers` 缺省兜底给的
/// 4 个参数 XMM + XMM0）宽——物理上 plan 那份才对（Win64 的 XMM0-15 全 volatile）。
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
