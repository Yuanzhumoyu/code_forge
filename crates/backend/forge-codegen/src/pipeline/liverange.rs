//! LiveInterval — 活跃区间系统。
//!
//! 替代旧的 `HashMap<VReg, (usize, usize)>` 单区间表示，
//! 支持 lifetime holes、use positions 和 spill weight 计算。

use forge_ir::*;
use std::collections::{HashMap, HashSet};

/// 程序点 — VCode 中的精确位置。
///
/// 编码: 高 16 bit = block_index, 低 16 bit = instruction_index * 2.
/// 每个指令占两个程序点: 偶数 = use/输入, 奇数 = def/输出.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct ProgPoint(u32);

impl ProgPoint {
    /// 每个块的最大指令数 (2^16 / 2 = 32768).
    pub const BLOCK_STRIDE: u32 = 0x10000;

    /// 从 (block_index, instruction_index_in_block) 构造程序点。
    /// instruction_index 自动翻倍以预留 use/def 空间。
    pub fn new(block: u32, inst: u32) -> Self {
        ProgPoint(block * Self::BLOCK_STRIDE + inst * 2)
    }

    /// 程序点之前的位置。
    pub fn before(self) -> ProgPoint {
        ProgPoint(self.0.saturating_sub(1))
    }

    /// 程序点之后的位置。
    pub fn after(self) -> ProgPoint {
        ProgPoint(self.0.saturating_add(1))
    }

    /// 一条指令的起始程序点（use 位置）。
    pub fn inst_start(block: u32, inst: u32) -> ProgPoint {
        ProgPoint::new(block, inst)
    }

    /// 一条指令的结束程序点（def 位置）。
    pub fn inst_end(block: u32, inst: u32) -> ProgPoint {
        ProgPoint::new(block, inst).after()
    }

    /// 最大值（用于 "永远不被使用的下一个使用点"）。
    pub const MAX: ProgPoint = ProgPoint(u32::MAX);

    /// 获取原始值。
    pub fn raw(self) -> u32 {
        self.0
    }
}

/// 单个活跃段 [start, end) — start inclusive, end exclusive.
#[derive(Clone, Copy, Debug)]
pub struct LiveSegment {
    pub start: ProgPoint,
    pub end: ProgPoint,
}

impl LiveSegment {
    pub fn new(start: ProgPoint, end: ProgPoint) -> Self {
        debug_assert!(start <= end, "LiveSegment: start after end");
        Self { start, end }
    }

    /// 此段是否包含给定程序点。
    pub fn contains(&self, point: ProgPoint) -> bool {
        self.start <= point && point < self.end
    }

    /// 两段是否重叠。
    pub fn overlaps(&self, other: &LiveSegment) -> bool {
        self.start < other.end && other.start < self.end
    }
}

/// 含 lifetime holes 的活跃区间。
///
/// 每个 XReg 对应一个 LiveInterval。
/// segments 有序、不相交，表示 VReg 的各个活跃阶段。
#[derive(Clone, Debug)]
pub struct LiveInterval {
    pub vreg: XReg,
    pub reg_class: RegClass,
    /// 有序、不相交的活跃段
    pub segments: Vec<LiveSegment>,
    /// 使用点列表（按程序点排序），用于 spill 启发式
    pub uses: Vec<ProgPoint>,
    /// 定义点列表
    pub defs: Vec<ProgPoint>,
    /// spill 权重（循环深度加权）
    pub weight: f32,
}

impl LiveInterval {
    /// 创建一个新的活跃区间。
    pub fn new(vreg: XReg, reg_class: RegClass) -> Self {
        Self {
            vreg,
            reg_class,
            segments: Vec::new(),
            uses: Vec::new(),
            defs: Vec::new(),
            weight: 0.0,
        }
    }

    /// 添加一个定义点。
    pub fn add_def(&mut self, point: ProgPoint) {
        self.defs.push(point);
    }

    /// 添加一个使用点。
    pub fn add_use(&mut self, point: ProgPoint) {
        self.uses.push(point);
    }

    /// 扩展当前段或开始新段以覆盖 point。
    pub fn cover(&mut self, point: ProgPoint) {
        let point_after = point.after();
        if let Some(last) = self.segments.last_mut() {
            if last.end > point {
                // 已在范围内 (end > point, start <= point)
                return;
            }
            if last.end == point || last.end == point.before() {
                // 相邻或刚好在段末 — 合并扩展，消除 ProgPoint 的 1-point 微间隙
                last.end = point_after;
                return;
            }
        }
        // 开始新段
        self.segments.push(LiveSegment::new(point, point_after));
    }

    /// 区间是否包含给定程序点。
    pub fn contains(&self, point: ProgPoint) -> bool {
        self.segments.iter().any(|seg| seg.contains(point))
    }

    /// 在 point 之后的第一个使用点（用于 spill 启发式）。
    pub fn next_use_after(&self, point: ProgPoint) -> Option<ProgPoint> {
        self.uses.iter().copied().find(|&u| u > point)
    }

    /// 最后的活跃程序点。
    pub fn end_point(&self) -> ProgPoint {
        self.segments.last().map(|s| s.end).unwrap_or(ProgPoint(0))
    }

    /// 起始活跃程序点。
    pub fn start_point(&self) -> ProgPoint {
        self.segments
            .first()
            .map(|s| s.start)
            .unwrap_or(ProgPoint(0))
    }
}

// ============================================================
// Liveness 分析
// ============================================================

/// 为 VCode 中所有 XReg 计算 LiveInterval。
///
/// 使用迭代数据流分析（非 SSA 依赖）计算带 holes 的活跃区间。
/// Spill weight 使用循环深度加权（循环内 ×10）。
pub fn compute_live_intervals<I: crate::MachineInst>(
    vcode: &crate::VCode<I>,
    xreg_map: &[smallvec::SmallVec<[XReg; 2]>],
    param_xregs: &[XReg],
) -> HashMap<XReg, LiveInterval> {
    // ── 阶段 0: 循环检测 ──
    // 收集所有块（迭代器不暴露 len）
    let blocks: Vec<_> = vcode.blocks().collect();
    let num_blocks = blocks.len();

    // 构建 IR Block → VCode block index 的映射
    let mut ir_block_to_vblock: HashMap<Block, usize> = HashMap::new();
    for (idx, block) in blocks.iter().enumerate() {
        ir_block_to_vblock.insert(block.ir_block, idx);
    }

    // 检测每个 VCode 块是否在循环体内（有回边指向 ≤ 当前索引的块）
    let mut in_loop: Vec<bool> = vec![false; num_blocks];
    for (block_idx, block) in blocks.iter().enumerate() {
        for inst in &block.instructions {
            if inst.is_branch() {
                for target_ir in inst.branch_targets().iter() {
                    if let Some(&target_idx) = ir_block_to_vblock.get(target_ir) {
                        // 回边：跳转到 ≤ 当前索引的块
                        if target_idx <= block_idx {
                            // 标记循环体内所有块（从 target 到 branch）
                            for v in in_loop[target_idx..=block_idx].iter_mut() {
                                *v = true;
                            }
                        }
                    }
                }
            }
        }
    }

    // ── 阶段 1: 计算 LiveInterval（带循环权重）──
    let mut intervals: HashMap<XReg, LiveInterval> = HashMap::new();

    const LOOP_WEIGHT: f32 = 10.0;
    const NORMAL_WEIGHT: f32 = 1.0;

    let mut global_inst = 0usize;
    for (block_idx, block) in blocks.iter().enumerate() {
        let block_u32 = block_idx as u32;
        let w = if in_loop[block_idx] {
            LOOP_WEIGHT
        } else {
            NORMAL_WEIGHT
        };

        for (inst_idx, _inst) in block.instructions.iter().enumerate() {
            let inst_u32 = inst_idx as u32;
            let use_point = ProgPoint::inst_start(block_u32, inst_u32);

            // 该指令的寄存器字段对应的 XReg（由指令包 xreg_map 聚合提供）
            if let Some(slot) = xreg_map.get(global_inst) {
                for &xreg in slot.iter() {
                    let interval = intervals
                        .entry(xreg)
                        .or_insert_with(|| LiveInterval::new(xreg, xreg.class()));
                    interval.weight += w;
                    interval.add_use(use_point);
                    interval.cover(use_point);
                }
            }
            global_inst += 1;
        }
    }

    // ── 阶段 1.5: 参数 XReg 活跃区间扩展 ──
    // 参数 XReg 的值在函数入口处就已存在（通过 ABI 传入），
    // 因此必须将其活跃区间起点扩展到函数开始位置 ProgPoint(0,0)。
    // 如果不扩展，BT 分配器可能在参数首次被使用前就将其寄存器分配给其他 XReg。
    let func_start = ProgPoint::new(0, 0);
    for &param_xreg in param_xregs {
        if let Some(interval) = intervals.get_mut(&param_xreg) {
            interval.cover(func_start);
        }
    }

    let mut global_inst_map = 0usize;
    // ── 阶段 2: 跨块数据流分析 ──
    // 构建 per-block 后继列表（branch targets + fallthrough）
    let mut block_succs: Vec<Vec<usize>> = vec![Vec::new(); num_blocks];
    for (block_idx, block) in blocks.iter().enumerate() {
        // 收集显式 branch targets
        for inst in &block.instructions {
            if inst.is_branch() {
                for target_ir in inst.branch_targets().iter() {
                    if let Some(&target_idx) = ir_block_to_vblock.get(target_ir) {
                        block_succs[block_idx].push(target_idx);
                    }
                }
            }
        }

        // 如果不是返回块且不是最后一块，添加 fallthrough 以建立完整 CFG
        // 这对于跨块 liveness 数据流至块重要
        if !block.is_return_block && block_idx + 1 < num_blocks {
            block_succs[block_idx].push(block_idx + 1);
        }
        // 去重
        block_succs[block_idx].sort();
        block_succs[block_idx].dedup();
    }

    // 构建 per-block local uses/defs
    let mut block_uses: Vec<HashSet<XReg>> = vec![HashSet::new(); num_blocks];
    let mut block_defs: Vec<HashSet<XReg>> = vec![HashSet::new(); num_blocks];
    for (block_idx, block) in blocks.iter().enumerate() {
        for (inst_idx, _inst) in block.instructions.iter().enumerate() {
            if let Some(slot) = xreg_map.get(global_inst_map) {
                for &xreg in slot.iter() {
                    block_uses[block_idx].insert(xreg);
                    block_defs[block_idx].insert(xreg);
                }
            }
            global_inst_map += 1;
        }
    }

    // 迭代数据流: live_out[B] = ∪ live_in[succ], live_in[B] = uses[B] ∪ (live_out[B] - defs[B])
    let mut live_in: Vec<HashSet<XReg>> = vec![HashSet::new(); num_blocks];
    let mut live_out: Vec<HashSet<XReg>> = vec![HashSet::new(); num_blocks];

    loop {
        let mut changed = false;
        // 逆序处理（从出口向后传播）
        for block_idx in (0..num_blocks).rev() {
            // live_out[B] = ∪ live_in[succ]
            let mut new_out: HashSet<XReg> = HashSet::new();
            for &succ in &block_succs[block_idx] {
                new_out.extend(&live_in[succ]);
            }
            if new_out != live_out[block_idx] {
                changed = true;
                live_out[block_idx] = new_out;
            }
            // live_in[B] = uses[B] ∪ (live_out[B] - defs[B])
            let mut new_in = block_uses[block_idx].clone();
            for vreg in live_out[block_idx].difference(&block_defs[block_idx]) {
                new_in.insert(*vreg);
            }
            if new_in != live_in[block_idx] {
                changed = true;
                live_in[block_idx] = new_in;
            }
        }
        if !changed {
            break;
        }
    }

    // 扩展 interval 以覆盖跨块活跃范围
    // 仅当 XReg 来自前驱（在 block 外部定义）时扩展到 block_start
    // 仅当 XReg 到达后继时扩展到 block_end
    for (vreg, interval) in intervals.iter_mut() {
        for block_idx in 0..num_blocks {
            let block_u32 = block_idx as u32;
            let last_inst = blocks[block_idx].instructions.len().saturating_sub(1) as u32;

            // 扩展到 block_end：VReg 活跃并传递到后继块
            if live_out[block_idx].contains(vreg) {
                let block_end = ProgPoint::inst_end(block_u32, last_inst);
                interval.cover(block_end);
            }

            // 扩展到 block_start：VReg 来自前驱块（未在本块定义）
            if live_in[block_idx].contains(vreg) && !block_defs[block_idx].contains(vreg) {
                let block_start = ProgPoint::new(block_u32, 0);
                interval.cover(block_start);
            }
        }
    }

    intervals
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 构造第 n 号 GPR 临时寄存器（测试辅助）。
    fn xgpr(n: u32) -> XReg {
        let mut xa = XRegAllocator::new();
        for _ in 0..n {
            xa.alloc_default(RegClass::GPR);
        }
        xa.alloc_default(RegClass::GPR)
    }

    #[test]
    fn test_prog_point_ordering() {
        let p0 = ProgPoint::new(0, 0);
        let p1 = ProgPoint::inst_end(0, 0);
        assert!(p0 < p1);

        let p2 = ProgPoint::new(0, 1);
        assert!(p1 < p2);
    }

    #[test]
    fn test_live_segment_contains() {
        let seg = LiveSegment::new(ProgPoint(10), ProgPoint(20));
        assert!(seg.contains(ProgPoint(10)));
        assert!(seg.contains(ProgPoint(15)));
        assert!(!seg.contains(ProgPoint(20))); // end exclusive
        assert!(!seg.contains(ProgPoint(9)));
    }

    #[test]
    fn test_live_segment_overlaps() {
        let a = LiveSegment::new(ProgPoint(0), ProgPoint(10));
        let b = LiveSegment::new(ProgPoint(5), ProgPoint(15));
        let c = LiveSegment::new(ProgPoint(10), ProgPoint(20));
        assert!(a.overlaps(&b));
        assert!(!a.overlaps(&c)); // a.end == c.start, not overlap
    }

    #[test]
    fn test_live_interval_cover() {
        let mut interval = LiveInterval::new(xgpr(0), RegClass::GPR);
        interval.cover(ProgPoint(5));
        interval.cover(ProgPoint(6)); // extends

        assert_eq!(interval.segments.len(), 1);
        assert!(interval.contains(ProgPoint(5)));
        assert!(interval.contains(ProgPoint(6)));

        // hole
        interval.cover(ProgPoint(10));
        assert_eq!(interval.segments.len(), 2);
        assert!(!interval.contains(ProgPoint(8))); // in the hole
        assert!(interval.contains(ProgPoint(10)));
    }
}
