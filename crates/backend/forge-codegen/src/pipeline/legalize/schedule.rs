//! 合法化批次编排（v30 M2：从 `compiler::compile_stages` 外提；**只搬不改次序**）。
//!
//! 批次次序与理由仍在下面逐条注释里（v23 收尾 M7 修正的 ABI 槽时机）；
//! `compiler` 只负责"什么时候调用它"，不再持有批次细节。
//!
//! 返回：需要继续编译的函数副本（没改写 ⇒ `None`，调用方用原函数）与 ABI 槽。

use std::sync::Arc;

use crate::IrError;
use crate::machine::target::TargetMachine;
use crate::pipeline::va_expand::AbiSlots;
use forge_ir::{Function, ModuleSigs};

/// 合法化产物。
pub(crate) struct Output {
    /// 就地改写后的副本（`None` = 没有任何 pass 需要改写，调用方继续用原函数）。
    pub(crate) func: Option<Function>,
    /// 变参 ABI 槽（排布在聚合族之后；`None` = 本函数没有变参）。
    pub(crate) abi_slots: Option<AbiSlots>,
}

/// 跑完整合法化批次。
///
/// `verify` = 是否在合法化之后跑**完整** [`forge_ir::Verifier`]（v31：取代旧的
/// "只跑 use-lists"——GEP 展开的 `add ptr, i64` 由 `type_rule = "binop_ptr_off"`
/// 正式承认，不再需要"verify 不跑"的例外）。
pub(crate) fn run<M: TargetMachine>(
    machine: &M,
    func: &Function,
    module_sigs: Option<Arc<ModuleSigs>>,
    verify: bool,
) -> Result<Output, IrError> {
    // 可变的 IR 副本：聚合 store/参数展开（agg_expand）需要 &mut Function。
    // 展开后读取 func_ref（Stage 4+ 全部只读原函数；S6 的树型 树型匹配
    // 匹配发生在 lowering 驱动内逐块预扫，不需要在编译期克隆整个 Function）。
    let needs_agg_expand = crate::pipeline::agg_expand::needs_agg_expand(func);
    let mut func_owned: Option<Function> = needs_agg_expand.then(|| func.clone());

    // **变参 IR 展开**（v20 变参 V3/V6）：`VaStart`/`VaArg` 在这里展开成显式 IR——按 plan 的
    // 形状数据（字段布局 + 保存区 + 取参规则）跑同一套算法，理由见 `pipeline/va_expand.rs`。
    //
    // 时机：与聚合展开同一处（**必须**在 `CompileState::new` 之前——它克隆常量池；展开会
    // 往池里插常量），也在 lowering 之前。
    //
    // **ABI 槽（保存区 + `va_list` 对象）的排布时机（v23 收尾 M7 修正）**：它必须排在
    // 前端局部槽之下，深度靠扫 IR 得到。**扫的必须是合法化之后的 IR**——聚合族 pass 会
    // 新增 `Alloca`（聚合槽），若在它们之前排定，lowering 看到的局部区更大 ⇒ 新槽可能
    // 落进 ABI 槽区（症状：字段各自读对、一起用就错、每次运行还不同）。所以：
    //
    // ```text
    //   ① 聚合族 pass（不含 va）  →  ② plan_abi_slots（此时含新建的 Alloca）
    //                             →  ③ Va pass（用排好的槽）
    // ```
    //
    // 这里先把 `va` 信息（**不含槽**）算出来，槽在 ② 之后补上。
    let mut abi_slots: Option<AbiSlots> = None;
    let mut va_pending: Option<(
        forge_isa_runtime::machine::call_layout::VaInfo,
        i64, // stack_slot_shift
    )> = None;
    if crate::pipeline::va_expand::has_va_op(func) {
        match crate::pipeline::va_expand::va_info_for_expansion(machine, func, module_sigs)? {
            Some((va, layout)) => {
                let fl = crate::pipeline::frame_layout::frame_layout_info(machine, Some(&layout));
                va_pending = Some((va, i64::from(fl.stack_slot_shift)));
                // 变参也要就地改写 IR ⇒ 需要有可变的副本（与聚合展开同一份）。
                func_owned.get_or_insert_with(|| func.clone());
            }
            None => {
                return Err(IrError::capability(
                    "VA_SHAPE_MISSING",
                    format!(
                        "变参：函数 `{}` 用了 VaStart/VaArg，但它的约定没声明 `hidden.va_list`\
                         （`va_list` 的形状）——变参需要形状数据，生成器不再有专用臂",
                        func.name
                    ),
                ));
            }
        }
    }

    // Stage 0.5: 聚合 store 展开（聚合字面量 → 打包 i64 常量 store；
    // >8 字节聚合显式 Unsupported）。必须在 CompileState 创建**之前**
    // 执行：CompileState 克隆 func 的常量池，若先建 state 再展开，新常量
    // 只进 func_owned 副本的池，lowering 读 state 里旧池（idx 错位 → 0）。
    crate::pipeline::legalize::ensure_scalar_owned(machine, func, &mut func_owned);
    if let Some(f) = func_owned.as_mut() {
        // **合法化单一驱动**（v23 M6）：定序、依赖传递与"改了什么"都收在驱动里。
        // 聚合按值传参的落点判据来自**约定数据**（只读单参探针）：win64 `>8B ⇒ 调用方
        // 副本 + 指针`、sysv64 `>16B ⇒ 传出参数区（未接）`。约定名解析失败 ⇒ fail-closed
        //（与 `abi_setup::setup_conv` 同口径）。
        let agg_conv = crate::pipeline::conv_registry::resolve(&f.calling_convention)?;
        let mut lctx = crate::pipeline::legalize::LegalizeCtx::new(machine, agg_conv);
        let mut changes = crate::pipeline::legalize::LegalizeDriver::scalar().run(f, &mut lctx)?;
        // ① 聚合族先跑（不含 va）：它新增 `Alloca` ⇒ 必须在 ABI 槽排布前落定。
        changes.merge(
            crate::pipeline::legalize::LegalizeDriver::with_aggregates_except_va()
                .run(f, &mut lctx)?,
        );
        // **①.5 宽值批**：必须在**聚合批之后**（跨界宽值已由计划相/聚合批改写成
        // 按引用指针、sret 缓冲或向量槽），在**收尾批之前**（物化相要看到已拆肢的
        // load/store）。见 `LegalizeDriver::wide` 的说明与 `WA-52`。
        changes.merge(crate::pipeline::legalize::LegalizeDriver::wide().run(f, &mut lctx)?);
        // **② 排 ABI 槽**：此时才扫 IR——深度里已含聚合族新建的槽（见上方时机注释）。
        if let Some((va, shift)) = va_pending.take() {
            let slots = crate::pipeline::va_expand::plan_abi_slots(
                f,
                &va,
                crate::machine::target::TargetMachine::reg_info(machine).slot_bytes() as u32,
                shift,
            );
            abi_slots = Some(slots);
            lctx.va = Some((va, slots));
            // **③ 再跑 va**（用排好的槽）。
            changes.merge(crate::pipeline::legalize::LegalizeDriver::va_only().run(f, &mut lctx)?);
        }
        // ④ 收尾（物化相 + GEP）：物化须等 va 登记完位置。
        changes.merge(crate::pipeline::legalize::LegalizeDriver::finalize().run(f, &mut lctx)?);
        if crate::pipeline::trace::knob("FORGE_TRACE_AGG") {
            eprintln!(
                "[forge] 聚合展开 {}：共 {} 项（+{} 指令 / +{} 值 / +{} 常量 / sret={}）",
                f.name,
                changes.total(),
                changes.new_insts,
                changes.new_values,
                changes.new_consts,
                changes.sret
            );
        }
        // 聚合展开是**就地改写 IR**（墓碑化/改 Copy/重写操作数）——debug 构建下
        // 先校验 **use-def 一致性**（2026-09-14 S6：这些站点此前只改字段、
        // 不更新 use-lists，留下陈旧 use 项）；`verify` 打开时再跑**完整 Verifier**。
        #[cfg(debug_assertions)]
        {
            if let Err(errors) = f.use_lists.verify(&f.dfg) {
                return Err(IrError::Internal(format!(
                    "聚合展开后 use-lists 不一致（fn {}）：{errors:?}",
                    f.name
                )));
            }
        }
        // **完整 IR 校验**（v31）：合法化是唯一会就地改写 IR 的阶段。
        // 旧注释说"GEP 的 `add ptr, i64` 类型不自洽 ⇒ verify 不跑"——那笔欠账已由
        // `Iadd` 迁到 `binop_ptr_off` 族（`add ptr, i64` 是正式形状）偿清；这里跑
        // 全量校验，任何 pass 把类型改坏都当场 fail-closed。
        if verify {
            let mut verifier = forge_ir::Verifier::with_ctx(f.types.clone());
            if let Err(errors) = verifier.verify(f) {
                // 失败时把**合法化后的 IR** 打出来（只在 debug 构建、只在真失败时）——
                // 这类不变量违例没有 IR 正文几乎无法定位（类型号/指令号是内部句柄）。
                #[cfg(debug_assertions)]
                eprintln!(
                    "[forge] 合法化后 IR 校验失败：fn {}（{} 条）\n{}",
                    f.name,
                    errors.len(),
                    forge_ir::text::display::function_to_string(f)
                );
                let detail = errors
                    .iter()
                    .map(|e| format!("{e:?}"))
                    .collect::<Vec<_>>()
                    .join("; ");
                return Err(IrError::Verify(format!(
                    "合法化后 IR 校验失败（fn {}，{} 条）：{detail}",
                    f.name,
                    errors.len()
                )));
            }
        }
    }

    Ok(Output {
        func: func_owned,
        abi_slots,
    })
}
