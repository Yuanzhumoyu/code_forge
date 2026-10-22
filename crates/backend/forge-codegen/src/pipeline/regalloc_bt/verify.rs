//! 分配结果自检（独立文件：`state.rs` 撞规模棘轮，且这条不变量值得单独一处说清）。
//!
//! **不变量**：同一物理寄存器上不得出现重叠的活跃区间。
//!
//! 一旦被破坏，机器码里就会出现"两个同时活着的值共用一个寄存器"——表现是**静默错值**
//! 或 AV（v27 M6 实测：LICM 外提 `StackAddr` 后，循环头块发出
//! `MovRMem { dst: RAX, … }` 紧接 `StoreMemR { src: RAX, src2: RAX }`，
//! store 的地址寄存器被分成了上一条 load 的目标）。分配器出错必须**响亮**，
//! 不能交给运行期去踩内存。
//!
//! 归属：这是**分配器**的自检（它自己的不变量），所以住在 `regalloc_bt/` 而不是别处。

use std::collections::HashMap;

use forge_ir::{IrError, PReg, RegClass, XReg};

use super::liverange;
use crate::pipeline::alloc_result::SpillSlot;

/// 校验 `assignments` 里每个物理寄存器上的活跃段两两不重叠。
///
/// - 半开区间 `[start, end)`：端点相接不算重叠（一个值死在某点、另一个生在同一个点
///   是允许的——这正是线性扫描的复用机会）；
/// - 有 spill 槽的 vreg 跳过（它没有物理寄存器）；
/// - 同一 `(num, class)` 才算同一物理寄存器（同号的 GPR 与 FPR 是不同寄存器）。
pub(super) fn verify_no_overlap(
    assignments: &HashMap<XReg, PReg>,
    spill_slots: &HashMap<XReg, SpillSlot>,
    intervals: &HashMap<XReg, liverange::LiveInterval>,
) -> Result<(), IrError> {
    // (preg, class) → [(start, end, vreg_idx)]
    // 位置用 `u64`（`ProgPoint` 现为 u64：块步长 2^32，见 `liverange.rs` 的说明）。
    let mut by_reg: HashMap<(u32, RegClass), Vec<(u64, u64, u32)>> = HashMap::new();
    for (vreg, preg) in assignments {
        if spill_slots.contains_key(vreg) {
            continue;
        }
        let Some(iv) = intervals.get(vreg) else {
            continue;
        };
        for seg in &iv.segments {
            by_reg.entry((preg.num, preg.class)).or_default().push((
                seg.start.raw(),
                seg.end.raw(),
                vreg.index(),
            ));
        }
    }

    if std::env::var_os("FORGE_VERIFY_REGALLOC_TRACE").is_some() {
        let total: usize = by_reg.values().map(|v| v.len()).sum();
        eprintln!(
            "[regalloc-selfcheck] 已分配 vreg={} 寄存器组={} 活跃段={}",
            assignments.len(),
            by_reg.len(),
            total
        );
    }

    for ((preg, class), mut segs) in by_reg {
        segs.sort();
        for i in 0..segs.len() {
            for j in (i + 1)..segs.len() {
                let (s1, e1, v1) = segs[i];
                let (s2, e2, v2) = segs[j];
                if s1 < e2 && s2 < e1 {
                    return Err(IrError::Internal(format!(
                        "寄存器分配自检失败：物理寄存器 {preg}（{class:?}）上 v{v1} [{s1},{e1}) 与 \
                         v{v2} [{s2},{e2}) 的活跃区间重叠——两个同时活着的值被分到同一寄存器，\
                         机器码会静默错值（见 regalloc_bt/verify.rs 的说明）"
                    )));
                }
            }
        }
    }
    Ok(())
}

/// **诊断探针**（`FORGE_VERIFY_SPILL_LIVE=1`）：报告"**活值被驱逐**"。
/// 活值被驱逐时，寄存器里的值与槽里的值可能**分叉**（槽是权威副本：def 走 scratch
/// 直接写槽，寄存器里往往是陈旧值）。这条只在**跨调用/跨块**的压力形状下出现，
/// 是"循环携带值 × 调用"那类红点的取证入口——**不要**据此改回"驱逐时落槽"：
/// 2026-10-19 实测该改法会让 `jit_stack_params_with_call` 等多台原本绿的用例立刻红
/// （陈旧值覆盖了槽里的正确值）。要放到 `spill_vreg` 的调用点，保持 `state.rs` 的规模。
pub(super) fn report_live_eviction(
    vreg: XReg,
    point: super::liverange::ProgPoint,
    preg: Option<PReg>,
    has_slot: bool,
) {
    eprintln!(
        "[spill-live] v{} 在 {point:?} 被驱逐但仍有后续使用（preg={preg:?}，已落槽={has_slot}）\
         ——寄存器里的当前值与槽里的权威副本可能分叉",
        vreg.index()
    );
}

/// `FORGE_TRACE_ALLOC`：值从空闲池拿到寄存器。
pub(super) fn trace_assign_free(vreg: XReg, preg: PReg, point: super::liverange::ProgPoint) {
    if crate::pipeline::trace::knob("FORGE_TRACE_ALLOC") {
        eprintln!("[alloc] v{} -> {preg} at {point:?} (free)", vreg.index());
    }
}

/// `FORGE_TRACE_ALLOC`：无空闲寄存器，进入驱逐路径。
pub(super) fn trace_evict(vreg: XReg, point: super::liverange::ProgPoint) {
    if crate::pipeline::trace::knob("FORGE_TRACE_ALLOC") {
        eprintln!("[alloc] v{} evict at {point:?}", vreg.index());
    }
}

/// `FORGE_TRACE_ALLOC/SPILL`：溢出值 reload 拿到寄存器。
pub(super) fn trace_reload(vreg: XReg, preg: PReg, point: super::liverange::ProgPoint) {
    eprintln!("[reload] v{} at {point:?} -> {preg}", vreg.index());
}
