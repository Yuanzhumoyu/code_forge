//! 从 `va_expand.rs` 拆出（v25 M4：逐字搬移，不改行为）。

use super::AbiSlots;
use forge_ir::*;
use forge_isa_runtime::machine::call_layout::VaInfo;

/// 前端局部槽的深度——**直接用唯一实现**（`pipeline/frame_slots.rs`），不再自己镜像一份：
/// ABI 槽必须排在这些槽之下，否则保存区/`va_list` 对象会被局部变量覆盖（`va_arg` 读出垃圾）。
pub(super) fn frontend_locals_depth(func: &Function, slot_bytes: u32) -> u32 {
    let types = func.types.clone();
    let slots = crate::pipeline::frame_slots::scan_frontend_slots(func, slot_bytes, |t| {
        types.borrow().size_bytes(t).max(1)
    });
    slots.depth_bytes(slot_bytes)
}

/// 算 ABI 槽（保存区 → 对象，依次排在前端局部槽之下）。
///
/// **例外（v20 V7）**：形状要求"保存区与栈实参连续"时（`va.save_contiguous`）保存区放
/// **帧顶** `[入口 sp - save.size, 入口 sp)`——于是它**不**占局部槽之下的深度，`va_list`
/// 对象仍排在局部槽之下；`va_top` 告诉调用方把其余东西（ra/fp、callee-saved、局部槽）
/// 整体下移一个 `save.size`。这样"单一线性游标"走到保存区末尾时，下一个地址正好是调用方
/// 写在 `[入口 sp + …)` 的栈实参。
pub(crate) fn plan_abi_slots(
    func: &Function,
    va: &VaInfo,
    slot_bytes: u32,
    shift: i64,
) -> AbiSlots {
    let align_up = |v: u32, a: u32| {
        let a = a.max(1);
        v.div_ceil(a) * a
    };
    let mut depth = frontend_locals_depth(func, slot_bytes);
    if let Some(save) = va.save.as_ref().filter(|_| va.save_contiguous) {
        // 帧顶：只记 va_top 与"相对帧基址"的偏移，不占深度。
        depth = align_up(depth, va.align) + va.size;
        return AbiSlots {
            save_depth: save.size,
            obj_depth: depth,
            save_off: -(i64::from(save.size)),
            // `save_base_v` 是 `StackAddr` 的立即数，而 `StackAddr(v)` 的地址 = `fp + v - shift`；
            // 调用方会把 `stack_slot_shift` **加上** `va_top`（局部槽下移），所以这里的立即数要用
            // **加上之前**的 shift：`fp + shift - (shift + save.size) = fp - save.size` ✓。
            // （第一版写成 `shift - save.size` ⇒ 地址少了 2×save.size，QEMU 用例当场读出错值。）
            save_base_v: shift,
            va_top: save.size,
            shift,
            // 帧要装得下：帧顶保存区 + 下面那一整串（局部槽/对象槽的深度已含后者）。
            max_depth: depth + save.size,
        };
    }
    let mut save_depth = depth;
    if let Some(save) = va.save.as_ref() {
        depth = align_up(depth, save.align) + save.size;
        save_depth = depth;
    }
    depth = align_up(depth, va.align) + va.size;
    AbiSlots {
        save_depth,
        obj_depth: depth,
        save_off: -(save_depth as i64) - shift,
        save_base_v: -(save_depth as i64),
        va_top: 0,
        shift,
        max_depth: depth,
    }
}

/// 函数里有没有 `VaArg` / `VaStart`（决定要不要准备可变副本 / 跑展开）。
pub(crate) fn has_va_op(func: &Function) -> bool {
    for (_, bd) in func.dfg.blocks() {
        for &ii in &bd.inst_order {
            if matches!(
                func.dfg.inst_data(ii).opcode,
                Opcode::VaArg | Opcode::VaStart
            ) {
                return true;
            }
        }
    }
    false
}

/// **变参信息**（形状 + 保存区 + 逐字段初值）：没有 ⇒ 调用方给出"该约定不支持变参"的明确错误。
///
/// 实现 = 拿一份 `LowerCtx` 走 `abi_setup::setup_conv`（与编译入口同一份装配逻辑与
/// fail-closed 消息），只取其中的 `va` 镜像。
pub(crate) fn va_info_for_expansion<M: crate::machine::target::TargetMachine>(
    machine: &M,
    func: &Function,
    module_sigs: Option<std::sync::Arc<forge_ir::ModuleSigs>>,
) -> Result<Option<(VaInfo, forge_isa_runtime::machine::abi_model::AbiPlan)>, IrError> {
    use crate::machine::target::TargetMachine;
    crate::pipeline_hooks::ensure_registered();
    let mut ctx = forge_isa_runtime::ctx::LowerCtx::new();
    ctx.value_gpr_class = TargetMachine::reg_info(machine).value_gpr_class();
    crate::pipeline::abi_setup::setup_conv(&mut ctx, machine, func, module_sigs)?;
    // 只要求"有 `va_list` 信息 + 有 plan"：形状/初值齐不齐由 `expand_va` 报错（消息更近现场）。
    // 变参外壳现在挂在 `CallConvCtx::va`（v23 M2：plan 本身直接给发射端读，外壳只剩
    // "宿主算出来的初值"这一件事）。
    Ok(ctx
        .conv
        .layout()
        .and_then(|cl| ctx.conv.va.clone().map(|va| (va, cl.clone()))))
}

/// 字节宽 → IR 整数类型（游标字段的宽度由 plan 给：sysv64 是 u32，别的形态可能更宽）。
///
/// **fail-closed**（v23 M6）：以前把"非 1/2/4/8"静默折成 `I64`——形状数据给一个 3/5/6 字节
/// 的游标字段时，读写会按 8 字节进行，游标与相邻对象字段互相踩。宽度只有四档是有理由的
/// （IR 整数类型就到 64 位），所以不是"折中"而是**明确报错**。
pub(super) fn int_ty(bytes: u32) -> Result<TypeId, IrError> {
    match bytes {
        1 => Ok(TypeId::I8),
        2 => Ok(TypeId::I16),
        4 => Ok(TypeId::I32),
        8 => Ok(TypeId::I64),
        other => Err(crate::IrError::capability(
            "VA_CURSOR_FIELD_WIDTH",
            format!(
                "变参游标字段宽度 {other} 字节：只支持 1/2/4/8（形状数据给的宽度必须落在这四档）——             以前会静默按 8 字节读写，与相邻字段互相踩"
            ),
        )),
    }
}
