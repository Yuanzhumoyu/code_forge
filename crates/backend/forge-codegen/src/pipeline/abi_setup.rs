//! **编译入口的调用约定装配**（v20）：把"这台机器 × 这份约定 × 这个函数"算成
//! [`LowerCtx::conv`](forge_isa_runtime::ctx::CallConvCtx) 上的一组数据。
//!
//! ## 为什么单独成模块
//!
//! 这里的三件事都是**约定层的规则**，原先埋在 `CompileState::new`（一个 300 行的构造过程）
//! 中间，既读不出来也单独测不了：
//!
//! 1. **约定级整数返回槽**（`conv.ret_gpr`）：谱里不再声明 `[abi].ret_regs`，"标量整数返回放
//!    哪个寄存器"是约定级事实（x86 RAX / riscv a0=X10 / arm64 x0）——用"空参 + 整数返回"的
//!    合成请求问一次引擎即可（与具体签名无关）。
//! 2. **函数级 plan 与 fail-closed**（`conv.layout`）：算不出调用布局就是**编译错误**
//!    （v20 A6 的裁定），错误消息带三步修法与自查命令。
//! 3. **约定级破坏集**（`conv.clobbers`）：`AbiPlan::clobbers` 是**签名无关**的事实，用空签名
//!    问一次即可；生成物在调用点拿不到本函数 plan 时用它兜底。
//!
//! 抽出来后调用点只有一行（`setup_conv(...)`），规则集中在本文件，`#[cfg(test)] mod tests`
//! 能直接对三条规则断言——不再需要"编译一个函数再看结果"才能覆盖。

use forge_abi::{AbiPlan, Signature};
use forge_ir::ir::function::Function;
use forge_isa_runtime::ctx::LowerCtx;
use forge_isa_runtime::machine::call_layout::{ArgShape, RetPlace};
use forge_isa_runtime::machine::target::TargetMachine;

/// 装配 [`LowerCtx::conv`](forge_isa_runtime::ctx::CallConvCtx)，并返回该函数的 [`AbiPlan`]
/// （调用方存进 `CompileState`，供 `FORGE_TRACE_ABI=1` 打印）。
///
/// `module_sigs` = 模块级签名表（`FuncRef::index()` → `(是否变参, 命名个数)`）；单函数编译传
/// `None`（调用点按非变参处理）。**要求**：调用前 `ctx.value_gpr_class` 已就位（返回槽探针
/// 用它取宽度）。
pub(crate) fn setup_conv<M: TargetMachine>(
    ctx: &mut LowerCtx,
    machine: &M,
    func: &Function,
    module_sigs: Option<&[(bool, u32)]>,
) -> Result<AbiPlan, forge_ir::IrError> {
    // ① IR 声明的那份标识 + 解析成注册表键（未注册 ⇒ fail-closed，消息里列出已注册的名字）。
    ctx.conv.id = func.calling_convention.clone();
    ctx.conv.name = crate::pipeline::conv_registry::resolve(&func.calling_convention)?;
    ctx.conv.module_sigs = module_sigs.map(|s| s.to_vec());

    let reg = crate::pipeline::conv_registry::registry()
        .read()
        .expect("约定注册表被投毒");

    // ② 约定级整数返回槽：空参 + 整数返回的合成请求。
    let w = ctx.value_gpr_class.width() as u32;
    let ret_probe = forge_isa_runtime::machine::call_plan::CallRequest::new(ctx.conv.name.clone())
        .rets([ArgShape::int(w.max(1), w.max(1))]);
    if let Ok(p) = crate::pipeline::abi_target::plan_for_shapes(machine, &reg, &ret_probe)
        && let Some(RetPlace::Reg { class, index, .. }) =
            crate::pipeline::abi_target::call_layout(&p, machine).ret
    {
        ctx.conv.ret_gpr = Some((index, class));
    }

    // ③ 函数级 plan：**fail-closed**——算不出来就是编译错误（错误消息带修法与自查命令）。
    let plan =
        match crate::pipeline::abi_target::plan_for_function(machine, &reg, &ctx.conv.name, func) {
            Ok(p) => p,
            Err(e) => {
                if crate::pipeline::trace_enabled("FORGE_TRACE_ABI") {
                    eprintln!("[abi-plan] {e}");
                }
                return Err(forge_ir::IrError::Unsupported(format!(
                    "v12 编译入口：这台机器上的约定 `{}` 规划不出该函数的调用布局（{e}）——\
                 无 plan 不再继续编译（fail-closed）。\n\
                 \x20 怎么修：① 约定数据缺/写错 ⇒ 用 \
                 forge_codegen::pipeline::conv_registry::register_rules_toml / \
                 register_binding_toml 注册（或改用已注册的约定）；\
                 ② 静态自查缺口 ⇒ `forge-isa abi check <谱>`（`--strict` 让缺口影响\
                 退出码）、`forge-isa abi plan <谱> --conv <名> --sig \"…\"`；\
                 ③ 口径与缺口清单 ⇒ docs/reference/calling-conventions.md、\
                 docs/plans/calling-convention-redesign-plan.md 的 A6。\n\
                 \x20 提示：设 FORGE_TRACE_ABI=1 可看到该函数的 plan 或失败原因。",
                    ctx.conv.name
                )));
            }
        };
    ctx.conv.layout = Some(crate::pipeline::abi_target::call_layout(&plan, machine));

    // ④ 约定级破坏集（签名无关）：空签名问一次；拿不到就保持空 ⇒ 生成物在调用点 fail-closed。
    if let Ok(p) = crate::pipeline::abi_target::plan_for_signature(
        machine,
        &reg,
        &ctx.conv.name,
        &Signature::new(Vec::new(), None),
    ) {
        // `CallLayout::clobbers` 是 (类, 类内号)，`LowerCtx` 的 `current_clobbers` 用 (号, 类)。
        ctx.conv.clobbers = crate::pipeline::abi_target::call_layout(&p, machine)
            .clobbers
            .into_iter()
            .map(|(c, i)| (i, c))
            .collect();
    }
    Ok(plan)
}

#[cfg(test)]
mod tests {
    use super::*;
    use forge_ir::ir::builder::FunctionBuilder;
    use forge_ir::ir::types::{FunctionSignature, TypeContext};
    use forge_ir::{CallConvId, ConvName, TypeId};

    fn probe(cc: CallConvId) -> Function {
        let ctx = TypeContext::new();
        let sig = FunctionSignature::new(&[(TypeId::I64, "n")], &[TypeId::I64])
            .with_calling_convention(cc);
        let mut b = FunctionBuilder::new("probe", ctx, sig);
        let (entry, params) = b.create_block_with_params(&[(TypeId::I64, "n")]);
        b.switch_to_block(entry);
        b.ret(&[params[0]]);
        b.finish().expect("build")
    }

    fn setup(func: &Function) -> (LowerCtx, AbiPlan) {
        crate::pipeline_hooks::ensure_registered();
        let tm = crate::arch::x86_v12::TargetMachine::new();
        let mut ctx = LowerCtx::new();
        ctx.value_gpr_class =
            crate::machine::target::TargetMachine::reg_info(&tm).value_gpr_class();
        let plan = setup_conv(&mut ctx, &tm, func, None).expect("装配成功");
        (ctx, plan)
    }

    /// 三条约定层规则一次钉住：返回槽探针（RAX）、函数级布局、约定级破坏集。
    #[test]
    fn setup_fills_ret_gpr_layout_and_clobbers() {
        let (ctx, plan) = setup(&probe(CallConvId::builtin(ConvName::Win64)));
        assert_eq!(
            ctx.conv.name, "win64",
            "IR 的 `Builtin(Win64)` 解析成注册表键"
        );
        let (index, class) = ctx.conv.ret_gpr.expect("约定级返回槽");
        assert_eq!(
            (index, class),
            (0, forge_ir::RegClass::GPR(8)),
            "win64 的标量整数返回在 RAX（类内号 0）"
        );
        let layout = ctx.conv.layout().expect("函数级布局");
        assert_eq!(layout.conv, "win64");
        assert!(
            matches!(layout.ret, Some(RetPlace::Reg { .. })),
            "布局的返回位是寄存器：{:?}",
            layout.ret
        );
        assert!(
            !ctx.conv.clobbers.is_empty(),
            "约定级破坏集不该为空（空 = 调用点 fail-closed）"
        );
        assert_eq!(plan.conv, "win64", "返回的 plan 与 ctx 同源");
    }

    /// `None` = 单函数编译 ⇒ 模块签名表缺席（调用点按非变参处理）。
    #[test]
    fn setup_records_absent_module_sigs() {
        let (ctx, _) = setup(&probe(CallConvId::builtin(ConvName::Win64)));
        assert!(ctx.conv.module_sigs.is_none());
        assert_eq!(
            ctx.conv.variadic_of(forge_ir::FuncRef::new(0)),
            None,
            "没有表 ⇒ 查不到变参信息（不猜）"
        );
    }

    /// 未注册的约定在**编译入口** fail-closed，且消息带修法（不是一句"规划不出来"）。
    #[test]
    fn setup_fail_closes_on_unregistered_convention() {
        crate::pipeline_hooks::ensure_registered();
        let tm = crate::arch::x86_v12::TargetMachine::new();
        let mut ctx = LowerCtx::new();
        ctx.value_gpr_class =
            crate::machine::target::TargetMachine::reg_info(&tm).value_gpr_class();
        let err = setup_conv(
            &mut ctx,
            &tm,
            &probe(CallConvId::named("no_such_conv")),
            None,
        )
        .expect_err("未注册 ⇒ 必须报错");
        let msg = err.to_string();
        assert!(
            msg.contains("no_such_conv") && msg.contains("register_rules_toml"),
            "要点名约定 + 给修法：{msg}"
        );
    }

    /// 模块签名表随装配进 `ctx.conv`，`variadic_of` 因此可查（变参 D6 的读口子）。
    #[test]
    fn setup_records_module_sigs() {
        crate::pipeline_hooks::ensure_registered();
        let tm = crate::arch::x86_v12::TargetMachine::new();
        let mut ctx = LowerCtx::new();
        ctx.value_gpr_class =
            crate::machine::target::TargetMachine::reg_info(&tm).value_gpr_class();
        let sigs = vec![(true, 1u32), (false, 0u32)];
        setup_conv(
            &mut ctx,
            &tm,
            &probe(CallConvId::builtin(ConvName::Win64)),
            Some(&sigs),
        )
        .expect("装配成功");
        assert_eq!(
            ctx.conv.variadic_of(forge_ir::FuncRef::new(0)),
            Some((true, 1))
        );
        assert_eq!(
            ctx.conv.variadic_of(forge_ir::FuncRef::new(9)),
            None,
            "越界 ⇒ None（不 panic）"
        );
    }
}
