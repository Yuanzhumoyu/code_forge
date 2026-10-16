//! 从 `regalloc_bt.rs` 拆出（v25 M4：逐字搬移，不改行为）。

use crate::IrError;

use crate::pipeline::alloc_config::RegAllocConfig;
use crate::pipeline::alloc_result::{SpillEdit, SpillSlot};
use forge_ir::*;

/// **逐指令溢出编辑**（v23 M5 的 `SpillEdit`）：决策在分配器里做出、发射侧只按序执行。
///
/// 原来决策与执行混在发射侧（`emit_inst_with_spills` 现查最终表、现挑 scratch、现判
/// `settable`）——分配器真正的逐点状态反而用不上。搬到这里之后：① scratch 池不够 =
/// **分配期** fail-closed；② `settable` 预检也在这里（指令在手）；③ 将来要驱逐**活跃值**，
/// 只需多产出一条 `Store`。
///
/// 口径与旧发射实现逐条对齐（**输出等价**）：scratch 的**号**取自 `config.scratch_regs`、
/// **类**取值的类；每条指令的编辑顺序是 `Reload`（全部被溢出的值）→ `Bind`（每个被溢出的
/// 绑定）→ `Store`（被溢出的 def）。没被溢出的绑定**不**产生编辑：它的字段已由管线在发射
/// 前按 `inst_ops` 统一回写过（旧发射实现里那次重写是冗余的）。
pub(super) fn build_spill_edits<I: crate::machine::inst::MachineInst>(
    vcode: &crate::VCode<I>,
    inst_ops: &[smallvec::SmallVec<[crate::machine::lowering::OperandBind; 2]>],
    assignments: &[Option<PReg>],
    spill_slots: &[Option<SpillSlot>],
    config: &RegAllocConfig,
) -> Result<Vec<Vec<SpillEdit>>, IrError> {
    use crate::machine::inst::OperandRole;
    // **逐点判据**（取证 ②）：此刻真的在栈上 = 有槽 **且** 没有寄存器。只看"有槽"会把
    // "曾被驱逐、之后 reload 回寄存器"的值也当成栈上的——那会把字段重定向到 scratch 并
    // 从槽里读**陈旧值**。
    // **判据回到"有槽"**（取证 ② 被推翻）：发射前的统一回写用的是**最终**分配表，所以
    // 只要某 vreg 有槽，就一律"从槽经 scratch 读"——这样才与回写无关地自洽。反过来按
    // "此刻没有寄存器"判，会让**早期**指令的字段停在**最终**（可能更晚的）寄存器号上，
    // 读到那个时刻的垃圾（实测：单跑 atomic 用例直接 0xc0000005 访问违规）。
    let slot_of =
        |v: XReg| -> Option<SpillSlot> { spill_slots.get(v.index() as usize).copied().flatten() };
    let mut edits: Vec<Vec<SpillEdit>> = Vec::new();
    let mut gi = 0usize;
    for block in vcode.blocks() {
        for inst in block.instructions.iter() {
            let mut e: Vec<SpillEdit> = Vec::new();
            if let Some(binds) = inst_ops.get(gi) {
                // 去重收集被溢出的 vreg（同一 vreg 的 use/def 共用同一个 scratch）。
                let mut spilled: smallvec::SmallVec<[XReg; 8]> = smallvec::SmallVec::new();
                for b in binds.iter() {
                    if slot_of(b.vreg).is_some() && !spilled.contains(&b.vreg) {
                        spilled.push(b.vreg);
                    }
                }
                if !spilled.is_empty() {
                    if spilled.len() > config.scratch_regs.len() {
                        return Err(IrError::RegAlloc(format!(
                            "第 {gi} 条指令需要 {} 个 scratch 寄存器搬溢出值，\
                             但本 ISA 只声明了 {} 个",
                            spilled.len(),
                            config.scratch_regs.len()
                        )));
                    }
                    // scratch 绑定：号来自 scratch 池，**类取值的类**（与旧发射口径一致）。
                    let scratch_of =
                        |v: XReg, i: usize| PReg::new(config.scratch_regs[i].num, v.class());
                    for (i, &v) in spilled.iter().enumerate() {
                        let slot = slot_of(v).expect("上面刚判过该 vreg 有溢出槽");
                        e.push(SpillEdit::Reload {
                            slot,
                            scratch: scratch_of(v, i),
                            class: v.class(),
                            width: v.bytes(),
                        });
                    }
                    let mut stored: smallvec::SmallVec<[XReg; 8]> = smallvec::SmallVec::new();
                    for b in binds.iter() {
                        let Some(i) = spilled.iter().position(|&v| v == b.vreg) else {
                            continue;
                        };
                        if !inst.is_reg_field_settable(usize::from(b.field)) {
                            return Err(IrError::RegAlloc(format!(
                                "第 {gi} 条指令的字段 {} 不可改写（固定物理字段）\
                                 却是被溢出值的落点",
                                b.field
                            )));
                        }
                        e.push(SpillEdit::Bind {
                            field: b.field,
                            scratch: scratch_of(b.vreg, i),
                            // **与该绑定的折入类一致**（`bind.class` = 值宽类，多类槽靠它定
                            // opsize；退到 vreg 的池宽类是 WA-35 的老病）。
                            class: b.class.unwrap_or_else(|| b.vreg.class()),
                        });
                        if b.role == OperandRole::Def && !stored.contains(&b.vreg) {
                            stored.push(b.vreg);
                            let slot = slot_of(b.vreg).expect("上面刚判过该 vreg 有溢出槽");
                            e.push(SpillEdit::Store {
                                slot,
                                scratch: scratch_of(b.vreg, i),
                                class: b.vreg.class(),
                                width: b.vreg.bytes(),
                            });
                        }
                    }
                }
            }
            edits.push(e);
            gi += 1;
        }
    }
    let _ = assignments;
    Ok(edits)
}

// ============================================================
// Allocator State
// ============================================================
