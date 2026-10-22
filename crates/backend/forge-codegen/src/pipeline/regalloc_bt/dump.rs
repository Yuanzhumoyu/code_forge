//! 分配结果诊断 dump（`FORGE_DUMP_ALLOC=1`）。
//!
//! 为什么单独一处：`FORGE_DUMP_VCODE` 打的是**发射前**的机器码（寄存器字段已经是
//! 回填后的物理号），看不出"这个字段当初是哪个 vreg、它的活区间覆盖到哪里"。
//! 分配出错时要回答的恰恰是那三个问题，所以这里按"指令 → 绑定 → vreg → 区间"的
//! 直线关系逐行打出来。
//!
//! 与 [`super::verify::verify_no_overlap`] 的分工：那个是**不变量检查**（错了就报错），
//! 这个是**取证视图**（人看）。

use super::liverange::ProgPoint;
use super::state::BtState;
use crate::machine::inst::MachineInst;

/// 打印一个函数的分配明细。`FORGE_DUMP_ALLOC_FILTER` 可按函数名子串过滤。
pub(super) fn dump_alloc<I: MachineInst>(
    func_name: &str,
    vcode: &crate::VCode<I>,
    inst_ops: &[smallvec::SmallVec<[crate::machine::lowering::OperandBind; 2]>],
    state: &BtState<'_>,
) {
    if let Ok(filter) = std::env::var("FORGE_DUMP_ALLOC_FILTER")
        && !filter.is_empty()
        && !func_name.contains(&filter)
    {
        return;
    }
    eprintln!("[alloc] === {func_name} ===");
    let mut gi = 0usize;
    for (block_idx, block) in vcode.blocks().enumerate() {
        eprintln!("[alloc] block {block_idx} (ir {:?})", block.ir_block);
        for (inst_idx, inst) in block.instructions.iter().enumerate() {
            let pp = ProgPoint::new(block_idx as u32, inst_idx as u32);
            eprintln!("[alloc]   #{inst_idx} pp={} {inst:?}", pp.raw());
            let Some(binds) = inst_ops.get(gi) else {
                gi += 1;
                continue;
            };
            for b in binds.iter() {
                let vreg = b.vreg;
                let preg = state.assignments.get(&vreg).map(|p| p.num);
                let slot = state.spill_slots.get(&vreg).map(|s| s.offset);
                let iv = state.intervals.get(&vreg);
                let segs = iv
                    .map(|iv| {
                        iv.segments
                            .iter()
                            .map(|s| format!("[{},{})", s.start.raw(), s.end.raw()))
                            .collect::<Vec<_>>()
                            .join(" ")
                    })
                    .unwrap_or_default();
                let covers = iv
                    .map(|iv| iv.segments.iter().any(|s| s.contains(pp)))
                    .unwrap_or(false);
                let uses = iv
                    .map(|iv| {
                        iv.uses
                            .iter()
                            .map(|p| p.raw().to_string())
                            .collect::<Vec<_>>()
                            .join(",")
                    })
                    .unwrap_or_default();
                eprintln!(
                    "[alloc]      v{} {:?} field={} preg={:?} spill={:?} 区间={} 覆盖本点={} uses=[{}]",
                    vreg.index(),
                    b.role,
                    b.field,
                    preg,
                    slot,
                    segs,
                    covers,
                    uses
                );
            }
            gi += 1;
        }
    }
}
