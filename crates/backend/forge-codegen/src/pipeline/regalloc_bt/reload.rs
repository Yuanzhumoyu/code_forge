//! **溢出值的重装**（从 `state.rs` 拆出：那里已撞规模棘轮，见 `tests/file_size_ratchet.rs`）。
//!
//! 归属：重装是**分配器自己的记账**——它决定"这个 spilled vreg 这次用哪个物理
//! 寄存器承载"，与发射侧的 `Reload` 编辑配合。拆出来不改行为（逐字搬移），只把
//! "溢出 → 归还 → 重装"这套三段式放在一处，别再让 `assignments` / `reg_owner` /
//! `free_regs` 三张表在多个文件里各写一份。

use crate::IrError;
use crate::machine::inst::OperandConstraint;
use forge_ir::*;

use super::state::BtState;

impl<'a> BtState<'a> {
    /// 归还 `vreg` 当前占用的物理寄存器：清 owner、回空闲池、从 `assignments` 删掉。
    ///
    /// **一处实现**（2026-11-01）：`spill_vreg` 与 `reload_from_stack` 过去各写一遍，
    /// 而且后者**漏写**了——重新分配直接覆盖 `assignments`，旧 preg 既不回池、
    /// `reg_owner` 又仍记着本 vreg ⇒ 那个寄存器此后谁也拿不到（池被吃干净）。
    pub(super) fn release_assignment(&mut self, vreg: XReg) -> Option<PReg> {
        let preg = self.assignments.remove(&vreg)?;
        self.clear_reg_owner(preg);
        self.free_regs.entry(preg.class).or_default().push(preg);
        Some(preg)
    }

    pub(super) fn reload_from_stack(
        &mut self,
        vreg: XReg,
        constraint: OperandConstraint,
    ) -> Result<(), IrError> {
        // 为 spilled XReg 分配一个寄存器（实际 load 在 emit 阶段完成）。
        // 尊重约束：Fixed 约束必须使用指定寄存器。
        let class = self.vreg_class(vreg);
        let trace = crate::pipeline::trace::knob("FORGE_TRACE_ALLOC")
            || crate::pipeline::trace::knob("FORGE_TRACE_SPILL");
        // **先归还旧分配**（2026-11-01 修）：带槽的 vreg 可能既在槽里又在
        // `assignments` 里（`evict_and_assign` 的 is_def 分支 spill 之后又从空闲池
        // 拿到了寄存器）。下面会覆盖 `assignments`，不先归还就等于把这个物理寄存器
        // **永久漏掉**（既不回空闲池、`reg_owner` 又仍记着本 vreg）——小寄存器文件上
        // 直接表现为"压力更大、更多溢出"。旧值是陈旧副本（槽才是权威），归还无代价。
        self.release_assignment(vreg);
        match constraint {
            OperandConstraint::Fixed(required_preg) => {
                // 如果目标寄存器被占用（含跨宽度类重叠，如 GPR(4) 与 GPR(8) 同编号），
                // 驱逐占用者
                if let Some(occupant) = self.phys_owner(required_preg)
                    && occupant != vreg
                {
                    self.spill_vreg(occupant)?;
                }
                if trace {
                    eprintln!(
                        "[reload] v{} at {:?} -> fixed {}",
                        vreg.index(),
                        self.current_point,
                        required_preg
                    );
                }
                self.assignments.insert(vreg, required_preg);
                self.set_reg_owner(required_preg, vreg);
            }
            _ => {
                // 任意空闲寄存器即可。
                //
                // 拿不到寄存器**不是错误**：这个 vreg 有溢出槽 ⇒ 发射侧对它的每次
                // use 都从槽 load 到 scratch（`build_spill_edits` 的 Reload+Bind），
                // 寄存器里那份只是陈旧副本、没人读。所以允许"只归还、不再分配"，
                // 但**绝不保留过期 assignment**——那才会让字段回写落到别人的寄存器上
                // （`release_assignment` 已在上面统一归还）。
                if let Some(preg) = self.pop_free(class) {
                    if trace {
                        super::verify::trace_reload(vreg, preg, self.current_point);
                    }
                    self.assignments.insert(vreg, preg);
                    self.set_reg_owner(preg, vreg);
                } else if trace {
                    eprintln!(
                        "[reload] v{} at {:?} 无空闲寄存器——走槽（槽是权威副本）",
                        vreg.index(),
                        self.current_point
                    );
                }
            }
        }
        Ok(())
    }
}
