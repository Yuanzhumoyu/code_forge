//! CFG 分析 — 支配树、前驱/后继、后序遍历。
//!
//! Cooper-Harvey-Kennedy 迭代算法计算立即支配者。

use crate::entity::*;
use crate::function::Function;
use std::collections::HashMap;

// ============================================================
// CFG 辅助函数
// ============================================================

pub fn block_successors_in_func(func: &Function, block: Block) -> Vec<Block> {
    func.dfg
        .block_terminator(block)
        .map(|t| t.successors())
        .unwrap_or_default()
}

// ============================================================
// DominatorTree
// ============================================================

#[derive(Clone, Debug)]
pub struct DominatorTree {
    pub entry: Block,
    children: HashMap<Block, Vec<Block>>,
    /// DFS 进入/离开时间戳（支配树上的区间序）。
    /// a 支配 b ⟺ tin[a] ≤ tin[b] 且 tout[b] ≤ tout[a]——O(1) 查询、
    /// O(n) 空间（替代原 dom_sets 的每块全套支配者 HashSet，O(n²) 空间）。
    tin: HashMap<Block, u32>,
    tout: HashMap<Block, u32>,
    block_count: usize,
}

impl DominatorTree {
    pub fn build(func: &Function) -> Self {
        let block_count = func.dfg.block_count();
        if block_count == 0 {
            return Self::empty();
        }
        let entry = func.entry_block.unwrap_or(Block(0));

        let postorder = compute_postorder(func, entry);
        let postorder_rank: HashMap<Block, usize> =
            postorder.iter().enumerate().map(|(i, &b)| (b, i)).collect();

        let idom = compute_idom(func, entry, &postorder, &postorder_rank);
        let children = compute_children(&idom);
        let (tin, tout) = compute_intervals(entry, &children);

        Self {
            entry,
            children,
            tin,
            tout,
            block_count,
        }
    }

    fn empty() -> Self {
        Self {
            entry: Block(0),
            children: HashMap::new(),
            tin: HashMap::new(),
            tout: HashMap::new(),
            block_count: 0,
        }
    }

    pub fn dominates(&self, a: Block, b: Block) -> bool {
        if a == b {
            return true;
        }
        // 区间判定：a 支配 b ⟺ b 落在 a 的 DFS 子树区间内
        match (
            self.tin.get(&a),
            self.tout.get(&a),
            self.tin.get(&b),
            self.tout.get(&b),
        ) {
            (Some(&ta_in), Some(&ta_out), Some(&tb_in), Some(&tb_out)) => {
                ta_in <= tb_in && tb_out <= ta_out
            }
            _ => false,
        }
    }

    pub fn children(&self, block: Block) -> &[Block] {
        self.children.get(&block).map_or(&[], |v| v.as_slice())
    }

    pub fn block_count(&self) -> usize {
        self.block_count
    }
}
fn compute_postorder(func: &Function, entry: Block) -> Vec<Block> {
    let mut result = Vec::new();
    let mut visited = std::collections::HashSet::<Block>::new();
    let mut stack = vec![(entry, false)];
    while let Some((current, processed)) = stack.pop() {
        if processed {
            result.push(current);
            continue;
        }
        if !visited.insert(current) {
            continue;
        }
        stack.push((current, true));
        for succ in block_successors_in_func(func, current) {
            if !visited.contains(&succ) {
                stack.push((succ, false));
            }
        }
    }
    result
}

fn compute_idom(
    func: &Function,
    entry: Block,
    postorder: &[Block],
    postorder_rank: &HashMap<Block, usize>,
) -> HashMap<Block, Block> {
    // 一次构建前驱映射（替代每轮对每个 block 线性扫全函数，O(轮数×n²) → O(轮数×n)）。
    let mut preds_map: HashMap<Block, Vec<Block>> = HashMap::new();
    for (block, bd) in func.dfg.blocks() {
        for succ in bd.terminator.successors() {
            preds_map.entry(succ).or_default().push(block);
        }
    }

    let mut idom: HashMap<Block, Block> = HashMap::new();
    idom.insert(entry, entry);

    let mut changed = true;
    while changed {
        changed = false;
        for &block in postorder.iter().rev() {
            if block == entry {
                continue;
            }
            let preds = preds_map.get(&block).map(|v| v.as_slice()).unwrap_or(&[]);
            let processed: Vec<Block> = preds
                .iter()
                .copied()
                .filter(|p| idom.contains_key(p))
                .collect();
            if processed.is_empty() {
                continue;
            }
            let mut new_idom = processed[0];
            for &pred in &processed[1..] {
                new_idom = intersect(&idom, postorder_rank, new_idom, pred);
            }
            if idom.get(&block).copied() != Some(new_idom) {
                idom.insert(block, new_idom);
                changed = true;
            }
        }
    }
    idom
}

fn intersect(
    idom: &HashMap<Block, Block>,
    postorder_rank: &HashMap<Block, usize>,
    mut finger1: Block,
    mut finger2: Block,
) -> Block {
    // Cooper-Harvey-Kennedy algorithm: in postorder numbering,
    // higher rank = closer to entry. The deeper node has LOWER rank
    // and must be advanced up the idom chain.
    while finger1 != finger2 {
        let rank1 = postorder_rank.get(&finger1).copied().unwrap_or(0);
        let rank2 = postorder_rank.get(&finger2).copied().unwrap_or(0);
        if rank1 < rank2 {
            // finger1 is deeper (lower postorder rank) → advance it
            finger1 = *idom.get(&finger1).unwrap_or(&finger1);
        } else {
            // finger2 is deeper (or equal rank) → advance it
            finger2 = *idom.get(&finger2).unwrap_or(&finger2);
        }
    }
    finger1
}

fn compute_children(idom: &HashMap<Block, Block>) -> HashMap<Block, Vec<Block>> {
    let mut children: HashMap<Block, Vec<Block>> = HashMap::new();
    for (&child, &parent) in idom {
        if child != parent {
            children.entry(parent).or_default().push(child);
        }
    }
    children
}

/// 支配树上的 DFS 进入/离开时间戳（区间序，迭代实现防深树栈溢出）。
/// a 支配 b ⟺ tin[a] ≤ tin[b] 且 tout[b] ≤ tout[a]。
fn compute_intervals(
    entry: Block,
    children: &HashMap<Block, Vec<Block>>,
) -> (HashMap<Block, u32>, HashMap<Block, u32>) {
    let mut tin: HashMap<Block, u32> = HashMap::new();
    let mut tout: HashMap<Block, u32> = HashMap::new();
    let mut timer: u32 = 0;
    // Iterative DFS：stack 存 (node, exiting)；先推 (node, false) 入点，
    // 出点 (node, true) 记录 tout。
    let mut stack = vec![(entry, false)];
    while let Some((node, exiting)) = stack.pop() {
        if exiting {
            timer += 1;
            tout.insert(node, timer);
            continue;
        }
        if tin.contains_key(&node) {
            continue;
        }
        timer += 1;
        tin.insert(node, timer);
        stack.push((node, true));
        if let Some(kids) = children.get(&node) {
            for &kid in kids.iter().rev() {
                stack.push((kid, false));
            }
        }
    }
    (tin, tout)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::builder::FunctionBuilder;
    use crate::types::{FunctionSignature, TypeContext};

    #[test]
    fn test_dominator_tree_diamond() {
        let ctx = TypeContext::new();
        let sig = FunctionSignature::new(&[], &[ctx.i32_ty()]);
        let mut fb = FunctionBuilder::new("test", ctx.clone(), sig);
        let (entry, _) = fb.create_entry_block();
        let then_blk = fb.create_block();
        let else_blk = fb.create_block();
        let (merge_blk, _) = fb.create_block_with_params(&[(TypeId::I32, "r")]);
        {
            fb.switch_to_block(entry);
            let c = fb.iconst_i32(1);
            fb.branch(c, then_blk, &[], else_blk, &[]);
        }
        {
            fb.switch_to_block(then_blk);
            let v = fb.iconst_i32(42);
            fb.jump(merge_blk, &[v]);
        }
        {
            fb.switch_to_block(else_blk);
            let v = fb.iconst_i32(0);
            fb.jump(merge_blk, &[v]);
        }
        {
            let params = fb.func.dfg.block_param_values(merge_blk).to_vec();
            fb.switch_to_block(merge_blk);
            fb.ret(&[params[0]]);
        }
        let func = fb.finish().expect("build");
        let dt = DominatorTree::build(&func);
        assert!(dt.dominates(entry, merge_blk));
    }
}
