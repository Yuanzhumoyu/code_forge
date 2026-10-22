//! 回写自检：**指令字段里的物理寄存器必须来自"分配结果"或"溢出编辑"**。
//!
//! ## 治什么
//!
//! 发射侧 encode 只读指令字段。字段的合法来源只有两个：
//!
//! 1. `CompileState::run_regalloc` 的**回写**（`assignments[vreg]`）；
//! 2. 溢出编辑的 `SpillEdit::Bind`（把该字段改指 scratch）。
//!
//! 只要有第三个来源——比如"回写跳过了这个字段"留下的 lowering 期**占位寄存器**——
//! 机器码就会读到一个没有任何人写过的寄存器，表现为**静默错值**（v27 M6 反复出现
//! 的 `StoreMemR { src: RAX, src2: RAX }` 就是这类：地址字段停在占位 RAX 上）。
//!
//! ## 判据
//!
//! 对每条指令的每个绑定：字段当前寄存器 ∈ { 分配寄存器, 该字段的 scratch } ∪ 固定物理字段。
//! 都不满足 ⇒ 报错并点名指令序号 / 字段 / 值 / 字段里的寄存器。
//!
//! 注意：**固定物理字段**（`settable == false`，模板里写死的寄存器，如 `cqo` 的 RDX）
//! 不参与判定——它们本来就不由分配器写。

use crate::machine::inst::MachineInst;
use crate::pipeline::alloc_result::{AllocResult, SpillEdit};
use crate::prelude::IrError;

/// 校验一个函数的全部指令字段绑定来源合法。
pub(crate) fn verify_field_bindings<I: MachineInst>(
    fname: &str,
    vcode: &crate::VCode<I>,
    inst_ops: &[smallvec::SmallVec<[crate::machine::lowering::OperandBind; 2]>],
    alloc: &AllocResult,
) -> Result<(), IrError> {
    let mut gi = 0usize;
    for block in vcode.blocks() {
        for inst in &block.instructions {
            let binds = inst_ops.get(gi).map(|s| s.as_slice()).unwrap_or(&[]);
            // 字段号 → 该字段当前寄存器（来自指令自己的操作数表）。
            let operands = inst.operands();
            for b in binds {
                let Some(op) = operands
                    .iter()
                    .find(|o| usize::from(o.field) == usize::from(b.field) && o.role == b.role)
                else {
                    continue; // 形状表没有这一项：由 debug 守卫的"旁表对齐"检查负责
                };
                if !inst.is_reg_field_settable(usize::from(b.field)) {
                    continue; // 固定物理字段：不由分配器写
                }
                let Some(actual) = op.slot else {
                    continue; // 无寄存器槽（内存字段等）
                };
                // 合法来源 ①：分配结果
                let from_alloc = alloc.preg(b.vreg).map(|p| p.num) == Some(actual);
                // 合法来源 ②：本指令有溢出编辑**要绑这个字段**（scratch 号在发射时才写进
                // 字段，所以这里只判"有没有这条编辑"，不比较号）。
                let from_edit = alloc
                    .edits
                    .get(gi)
                    .map(|es| {
                        es.iter().any(|e| {
                            matches!(e, SpillEdit::Bind { field, .. }
                                if usize::from(*field) == usize::from(b.field))
                        })
                    })
                    .unwrap_or(false);
                if !from_alloc && !from_edit {
                    return Err(IrError::Internal(format!(
                        "'{fname}' 第 {gi} 条指令（{:?}）的字段 {}（{:?}）里是寄存器 {actual}，\
                         但它既不是值 v{} 的分配结果（{:?}），也不是本指令溢出编辑给的 scratch\
                         ——字段停在 lowering 期的占位寄存器上，机器码会读到没人写过的寄存器\
                         （该值有溢出槽：{}；本指令的编辑：{:?}）",
                        inst,
                        b.field,
                        b.role,
                        b.vreg.index(),
                        alloc.preg(b.vreg).map(|p| p.num),
                        alloc.is_spilled(b.vreg),
                        alloc.edits.get(gi).map(|e| e.len()).unwrap_or(0),
                    )));
                }
            }
            gi += 1;
        }
    }
    Ok(())
}
