//! **预约**：把"模型说的活跃区间"变成"分配器真的占住寄存器"。
//!
//! 从 `state.rs` 拆出（那里撞规模棘轮，见 `tests/file_size_ratchet.rs`）。
//!
//! ## 治什么
//!
//! 分配器原来的分配时机是"**首次遇到该 vreg 的指令**"，而活跃区间模型允许区间
//! **早于**首次出现（块首 live-in、参数从函数入口就活着）。缺口期间 `pop_free`
//! 会把这个寄存器再分给别人 ⇒ 同一个物理寄存器上出现两个"模型认为同时活着"的值
//! （2026-11-01 实测：把 `cover_range` 修成真的填跨度后，x86 JIT 矩阵上自检批量报
//! `v2068 [0,14)` 与 `v1555 [7,11)` 同寄存器）。
//!
//! ## 做法
//!
//! `starts` 是 `(区间起点, vreg)` 的升序表，`next_start` 是只前进的游标；每条指令
//! 在 `expire_dead` 之后、按操作数分配之前调一次 [`BtState::reserve_starting`]，
//! 把"此刻已经活着、还没落位"的值先分到寄存器。总代价 O(区间数)。
//!
//! 只补**非 def 起点**的情况：def 的区间起点就是它自己的 def 点，原地分配即可。

use crate::IrError;
use crate::machine::inst::OperandConstraint;
use crate::pipeline::liverange::ProgPoint;

use super::state::BtState;

impl<'a> BtState<'a> {
    pub(super) fn reserve_starting(&mut self, point: ProgPoint) -> Result<(), IrError> {
        while let Some(&(start, vreg)) = self.starts.get(self.next_start) {
            if start > point {
                break;
            }
            self.next_start += 1;
            if self.assignments.contains_key(&vreg) || self.spill_slots.contains_key(&vreg) {
                continue;
            }
            let class = self.vreg_class(vreg);
            if crate::pipeline::trace::knob("FORGE_TRACE_ALLOC") {
                eprintln!("[reserve] v{} at {point:?} (start {start:?})", vreg.index());
            }
            let preg = self.assign_reg(vreg, OperandConstraint::Any, class, &[], true)?;
            self.active.insert(vreg, preg);
        }
        Ok(())
    }
}
