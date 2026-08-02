//! CFG 分析 — 支配树、前驱/后继、后序遍历。
//!
//! Cooper-Harvey-Kennedy 迭代算法计算立即支配者。

use crate::entity::*;
use crate::function::Function;
use crate::terminator::Terminator;
use std::collections::{HashMap, HashSet};

// ============================================================
// CFG 辅助函数
// ============================================================

pub fn block_successors(terminator: &Terminator) -> Vec<Block> {
    terminator.successors()
}

pub fn block_successors_in_func(func: &Function, block: Block) -> Vec<Block> {
    func.dfg
        .block_terminator(block)
        .map(|t| t.successors())
        .unwrap_or_default()
}

pub fn block_predecessors(func: &Function, target: Block) -> Vec<Block> {
    let mut preds = Vec::new();
    for (block, bd) in func.dfg.blocks() {
        for succ in bd.terminator.successors() {
            if succ == target {
                preds.push(block);
            }
        }
    }
    preds
}

// ============================================================
// DominatorTree
// ============================================================

#[derive(Clone, Debug)]
pub struct DominatorTree {
    pub entry: Block,
    idom: HashMap<Block, Block>,
    children: HashMap<Block, Vec<Block>>,
    dom_sets: HashMap<Block, HashSet<Block>>,
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
        let dom_sets = compute_dom_sets(&idom, &children);

        Self {
            entry,
            idom,
            children,
            dom_sets,
            block_count,
        }
    }

    fn empty() -> Self {
        Self {
            entry: Block(0),
            idom: HashMap::new(),
            children: HashMap::new(),
            dom_sets: HashMap::new(),
            block_count: 0,
        }
    }

    pub fn dominates(&self, a: Block, b: Block) -> bool {
        if a == b {
            return true;
        }
        self.dom_sets.get(&a).is_some_and(|set| set.contains(&b))
    }

    pub fn strictly_dominates(&self, a: Block, b: Block) -> bool {
        a != b && self.dominates(a, b)
    }

    pub fn idom(&self, block: Block) -> Option<Block> {
        self.idom.get(&block).copied().filter(|&id| id != block)
    }

    pub fn children(&self, block: Block) -> &[Block] {
        self.children.get(&block).map_or(&[], |v| v.as_slice())
    }

    pub fn entry(&self) -> Block {
        self.entry
    }
    pub fn block_count(&self) -> usize {
        self.block_count
    }
}

fn compute_postorder(func: &Function, entry: Block) -> Vec<Block> {
    let mut result = Vec::new();
    let mut visited = HashSet::new();
    // Iterative DFS to avoid stack overflow on deep CFGs (>10k blocks).
    // Stack holds (block, children_processed): push (block, false) on first
    // visit, then (block, true) after pushing unvisited successors in reverse
    // order so that the left-most successor is popped first.
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
        let succs = block_successors_in_func(func, current);
        for succ in succs.into_iter().rev() {
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
    let mut idom: HashMap<Block, Block> = HashMap::new();
    idom.insert(entry, entry);

    let mut changed = true;
    while changed {
        changed = false;
        for &block in postorder.iter().rev() {
            if block == entry {
                continue;
            }
            let preds = block_predecessors(func, block);
            let processed: Vec<Block> =
                preds.into_iter().filter(|p| idom.contains_key(p)).collect();
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

fn compute_dom_sets(
    idom: &HashMap<Block, Block>,
    children: &HashMap<Block, Vec<Block>>,
) -> HashMap<Block, HashSet<Block>> {
    let mut sets: HashMap<Block, HashSet<Block>> = HashMap::new();
    let entry = idom.iter().find(|(k, v)| k == v).map(|(k, _)| *k);
    if let Some(entry) = entry {
        compute_dom_set_rec(entry, children, &mut sets);
    }
    sets
}

fn compute_dom_set_rec(
    node: Block,
    children: &HashMap<Block, Vec<Block>>,
    sets: &mut HashMap<Block, HashSet<Block>>,
) -> HashSet<Block> {
    let mut dom_set = HashSet::new();
    dom_set.insert(node);
    if let Some(kids) = children.get(&node) {
        for &child in kids {
            dom_set.extend(compute_dom_set_rec(child, children, sets));
        }
    }
    sets.insert(node, dom_set.clone());
    dom_set
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
            let mut b = fb.build(entry);
            let c = b.iconst_i32(1);
            b.branch(c, then_blk, &[], else_blk, &[]);
        }
        {
            let mut b = fb.build(then_blk);
            let v = b.iconst_i32(42);
            b.jump(merge_blk, &[v]);
        }
        {
            let mut b = fb.build(else_blk);
            let v = b.iconst_i32(0);
            b.jump(merge_blk, &[v]);
        }
        {
            let params = fb.func.dfg.block_param_values(merge_blk).to_vec();
            let mut b = fb.build(merge_blk);
            b.ret(&[params[0]]);
        }
        let func = fb.finish();
        let dt = DominatorTree::build(&func);
        assert!(dt.dominates(entry, merge_blk));
    }
}
