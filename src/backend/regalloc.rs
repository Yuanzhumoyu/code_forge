//! 图着色寄存器分配器 — Chaitin-Briggs 算法。
//!
//! 构建干涉图 (interference graph)，使用图着色算法分配物理寄存器。
//! 当度数 ≥ K 且无法着色时，溢出到栈。
//!
//! ## 算法
//!
//! 1. **构建**: 从 live ranges 构建干涉图
//! 2. **简化**: 度数 < K 的节点入栈（Briggs: 度数 ≥ K 但有低度数邻居的也入栈）
//! 3. **溢出**: 如果所有节点度数 ≥ K，选择溢出代价最小的节点
//! 4. **选择**: 弹出栈，分配与邻居不同的颜色
//! 5. **重写**: 为溢出节点插入 spill/reload 代码
//!
//! ## 与 regalloc_adapter 的关系
//!
//! `crate::regalloc_adapter` 提供 trait 抽象层，本模块提供具体实现。

use crate::ir::*;
use std::collections::{BTreeSet, HashMap, HashSet};

/// 图着色寄存器分配器。
pub struct GraphColorRegAlloc {
    /// 可用的通用寄存器数量。
    num_gprs: usize,
    /// 可用的浮点寄存器数量。
    num_fprs: usize,
}

impl GraphColorRegAlloc {
    pub fn new(num_gprs: usize, num_fprs: usize) -> Self {
        Self { num_gprs, num_fprs }
    }

    /// 对一组 VReg 执行图着色分配。
    ///
    /// 输入: live_ranges — VReg → (def_point, last_use_point) 的映射
    /// 输出: VReg → PReg 的分配结果，以及需要 spill 的 VReg 列表
    pub fn allocate(
        &self,
        live_ranges: &HashMap<VReg, (usize, usize)>,
        vreg_class: &HashMap<VReg, RegClass>,
    ) -> (HashMap<VReg, PReg>, Vec<VReg>) {
        let vregs: Vec<VReg> = live_ranges.keys().copied().collect();
        let n = vregs.len();
        if n == 0 {
            return (HashMap::new(), Vec::new());
        }

        // Step 1: 构建干涉图
        let interference = build_interference_graph(&vregs, live_ranges);

        // 将 VReg 映射到图索引
        let _vreg_to_idx: HashMap<VReg, usize> =
            vregs.iter().enumerate().map(|(i, &v)| (v, i)).collect();

        // Step 2-4: Chaitin-Briggs 迭代
        let mut assignment: HashMap<VReg, PReg> = HashMap::new();
        let mut spilled: Vec<VReg> = Vec::new();
        let mut remaining: HashSet<usize> = (0..n).collect();
        let mut stack: Vec<usize> = Vec::new();

        while !remaining.is_empty() {
            // 简化阶段
            let mut progress = true;
            while progress {
                progress = false;
                let to_remove: Vec<usize> = remaining
                    .iter()
                    .filter(|&&idx| {
                        let degree = interference[idx]
                            .iter()
                            .filter(|&nb| remaining.contains(nb))
                            .count();
                        let k = self.colors_for(vregs[idx], vreg_class);
                        degree < k
                    })
                    .copied()
                    .collect();

                for idx in &to_remove {
                    remaining.remove(idx);
                    stack.push(*idx);
                    progress = true;
                }
            }

            // 如果还有剩余，需要溢出
            if !remaining.is_empty() {
                // 选择溢出代价最大的节点（度数最高的）
                let spill_idx = remaining
                    .iter()
                    .max_by_key(|&&idx| {
                        interference[idx]
                            .iter()
                            .filter(|&nb| remaining.contains(nb))
                            .count()
                    })
                    .copied();

                if let Some(idx) = spill_idx {
                    remaining.remove(&idx);
                    spilled.push(vregs[idx]);
                }
            }
        }

        // Step 5: 着色 — 从栈中弹出并分配
        let mut colored: HashMap<usize, u8> = HashMap::new();
        while let Some(idx) = stack.pop() {
            let class = vreg_class.get(&vregs[idx]).copied().unwrap_or(RegClass::Int);
            let k = self.colors_for(vregs[idx], vreg_class);
            let mut used_colors: BTreeSet<u8> = BTreeSet::new();
            for &nb in &interference[idx] {
                if let Some(&c) = colored.get(&nb) {
                    used_colors.insert(c);
                }
            }
            // 找到第一个未使用的颜色
            let color = (0..k as u8).find(|c| !used_colors.contains(c));
            if let Some(c) = color {
                colored.insert(idx, c);
                assignment.insert(vregs[idx], PReg::new(c, class));
            } else {
                spilled.push(vregs[idx]);
            }
        }

        (assignment, spilled)
    }

    fn colors_for(&self, _vreg: VReg, class: &HashMap<VReg, RegClass>) -> usize {
        match class.get(&_vreg) {
            Some(RegClass::Float) => self.num_fprs,
            _ => self.num_gprs,
        }
    }
}

/// 构建干涉图：如果两个 VReg 的 live range 重叠，则它们之间有边。
fn build_interference_graph(
    vregs: &[VReg],
    live_ranges: &HashMap<VReg, (usize, usize)>,
) -> Vec<HashSet<usize>> {
    let n = vregs.len();
    let mut graph = vec![HashSet::new(); n];

    for i in 0..n {
        let (i_def, i_last) = live_ranges[&vregs[i]];
        for j in (i + 1)..n {
            let (j_def, j_last) = live_ranges[&vregs[j]];
            // 重叠条件: i 在 j 开始前未结束，且 j 在 i 开始前未结束
            if i_def <= j_last && j_def <= i_last {
                graph[i].insert(j);
                graph[j].insert(i);
            }
        }
    }

    graph
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_empty_allocation() {
        let alloc = GraphColorRegAlloc::new(8, 8);
        let (map, spilled) = alloc.allocate(&HashMap::new(), &HashMap::new());
        assert!(map.is_empty());
        assert!(spilled.is_empty());
    }

    #[test]
    fn test_no_interference() {
        let mut ranges = HashMap::new();
        ranges.insert(VReg(0), (0, 1));
        ranges.insert(VReg(1), (2, 3));
        let mut class = HashMap::new();
        class.insert(VReg(0), RegClass::Int);
        class.insert(VReg(1), RegClass::Int);

        let alloc = GraphColorRegAlloc::new(8, 8);
        let (map, spilled) = alloc.allocate(&ranges, &class);
        assert_eq!(map.len(), 2);
        assert!(spilled.is_empty());
        // Should use different or same colors (same is fine since no interference)
    }

    #[test]
    fn test_interference_causes_spill_or_different_colors() {
        let mut ranges = HashMap::new();
        // All VRegs overlap — need more than K registers to avoid spill
        for i in 0..10 {
            ranges.insert(VReg(i), (0, 5));
        }
        let mut class = HashMap::new();
        for i in 0..10 {
            class.insert(VReg(i), RegClass::Int);
        }

        let alloc = GraphColorRegAlloc::new(4, 4); // Only 4 GPRs
        let (map, spilled) = alloc.allocate(&ranges, &class);
        // Some must spill
        assert!(!spilled.is_empty() || map.len() < 10);
    }

    #[test]
    fn test_interference_graph() {
        let vregs = vec![VReg(0), VReg(1), VReg(2)];
        let mut ranges = HashMap::new();
        ranges.insert(VReg(0), (0, 3)); // overlaps with 1 and 2
        ranges.insert(VReg(1), (1, 4)); // overlaps with 0 and 2
        ranges.insert(VReg(2), (2, 5)); // overlaps with 0 and 1

        let graph = build_interference_graph(&vregs, &ranges);
        assert!(graph[0].contains(&1));
        assert!(graph[0].contains(&2));
        assert!(graph[1].contains(&2));
    }
}
