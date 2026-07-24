//! 控制流分析基础设施 -- 支配树 (Dominator Tree)。
//!
//! 提供基于 Cooper-Harvey-Kennedy 迭代算法的支配树计算，
//! 以及支配边界 (dominance frontier) 计算，作为所有优化 pass 的共享分析层。
//!
//! ## 算法
//!
//! - **支配树**: Cooper-Harvey-Kennedy 迭代算法，通过逆后序遍历实现 O(n log n) 典型复杂度。
//!   参考: Cooper, Harvey, Kennedy "A Simple, Fast Dominance Algorithm" (2001).
//! - **支配边界**: Cytron et al. "Efficiently Computing Static Single Assignment Form" (1991).

use super::function::*;
use super::instructions::Terminator;
use super::types::*;
use std::collections::{HashMap, HashSet};
use std::sync::LazyLock;

// ============================================================
// CFG helpers
// ============================================================

/// 获取终止指令的所有后继块。
pub fn block_successors(terminator: &Terminator) -> Vec<BlockId> {
    match terminator {
        Terminator::Branch {
            true_block,
            false_block,
            ..
        } => vec![*true_block, *false_block],
        Terminator::Jump { target, .. } => vec![*target],
        Terminator::Switch {
            default_block,
            cases,
            ..
        } => {
            let mut succs = vec![*default_block];
            for (_, target, _) in cases {
                if !succs.contains(target) {
                    succs.push(*target);
                }
            }
            succs
        }
        Terminator::Return { .. } | Terminator::Unreachable => Vec::new(),
    }
}

/// 获取指定 block 在函数中的所有后继。
pub fn block_successors_in_func(func: &Function, block: BlockId) -> Vec<BlockId> {
    func.block(block)
        .map_or(Vec::new(), |b| block_successors(&b.terminator))
}

/// 获取指定 block 的所有前驱。
pub fn block_predecessors(func: &Function, target: BlockId) -> Vec<BlockId> {
    let mut preds = Vec::new();
    for block in func.iter_blocks() {
        for succ in block_successors(&block.terminator) {
            if succ == target {
                preds.push(block.id);
            }
        }
    }
    preds
}

// ============================================================
// Postorder computation
// ============================================================

fn compute_postorder(func: &Function, entry: BlockId) -> Vec<BlockId> {
    let mut result = Vec::new();
    let mut visited = HashSet::new();
    dfs_postorder(func, entry, &mut visited, &mut result);
    result
}

fn dfs_postorder(
    func: &Function,
    current: BlockId,
    visited: &mut HashSet<BlockId>,
    result: &mut Vec<BlockId>,
) {
    visited.insert(current);
    if let Some(block) = func.block(current) {
        for succ in block_successors(&block.terminator) {
            if !visited.contains(&succ) {
                dfs_postorder(func, succ, visited, result);
            }
        }
    }
    result.push(current);
}

// ============================================================
// Dominator Tree
// ============================================================

/// 支配树 -- CFG 支配关系的紧凑表示。
///
/// 使用 Cooper-Harvey-Kennedy 迭代算法构建，
/// 通过逆后序 (reverse postorder) 遍历实现接近线性的典型性能。
#[derive(Clone, Debug)]
pub struct DominatorTree {
    pub entry: BlockId,
    /// 直接支配者 (immediate dominator): idom[b] = 直接支配 b 的最近块。
    /// 入口块的 idom 不包含在映射中（返回 None）。
    idom: HashMap<BlockId, BlockId>,
    /// 支配树的孩子关系: children[p] = p 直接支配的所有块。
    children: HashMap<BlockId, Vec<BlockId>>,
    /// 支配集合: dom_sets[a] = a 支配的所有块的集合。
    dom_sets: HashMap<BlockId, HashSet<BlockId>>,
    /// 支配边界: DF[b] = b 的支配边界集合。
    domin_frontiers: HashMap<BlockId, HashSet<BlockId>>,
    block_count: usize,
    #[allow(dead_code)]
    postorder: Vec<BlockId>,
    #[allow(dead_code)]
    postorder_rank: HashMap<BlockId, usize>,
}

impl DominatorTree {
    /// 为函数构建支配树。
    ///
    /// 执行 Cooper-Harvey-Kennedy 算法：
    /// 1. 计算后序遍历 (postorder)
    /// 2. 以逆后序遍历迭代更新直接支配者
    /// 3. 构建支配集合和支配边界
    pub fn build(func: &Function) -> Self {
        let block_count = func.blocks.len();
        if block_count == 0 {
            return Self::empty();
        }

        let entry = func.blocks[0].id;

        let postorder = compute_postorder(func, entry);
        let postorder_rank: HashMap<BlockId, usize> = postorder
            .iter()
            .enumerate()
            .map(|(i, &b)| (b, i))
            .collect();

        let idom = compute_idom(func, entry, &postorder, &postorder_rank);
        let children = compute_dom_children(&idom);
        let dom_sets = compute_dom_sets(&idom, &children);
        let domin_frontiers = compute_dominance_frontiers(func, &idom, &dom_sets);

        Self {
            entry,
            idom,
            children,
            dom_sets,
            domin_frontiers,
            block_count,
            postorder,
            postorder_rank,
        }
    }

    fn empty() -> Self {
        Self {
            entry: BlockId(0),
            idom: HashMap::new(),
            children: HashMap::new(),
            dom_sets: HashMap::new(),
            domin_frontiers: HashMap::new(),
            block_count: 0,
            postorder: Vec::new(),
            postorder_rank: HashMap::new(),
        }
    }

    /// 判断 `a` 是否支配 `b`。
    ///
    /// a 支配 b 当且仅当从入口块到 b 的每条路径都经过 a。
    /// 每个块支配自己。
    pub fn dominates(&self, a: BlockId, b: BlockId) -> bool {
        if a == b {
            return true;
        }
        self.dom_sets
            .get(&a)
            .is_some_and(|set| set.contains(&b))
    }

    /// 判断 `a` 是否严格支配 `b`（等价于 a dominates b 且 a != b）。
    pub fn strictly_dominates(&self, a: BlockId, b: BlockId) -> bool {
        a != b && self.dominates(a, b)
    }

    /// 返回 `block` 的直接支配者 (immediate dominator)。
    ///
    /// 入口块没有直接支配者，返回 `None`。
    pub fn idom(&self, block: BlockId) -> Option<BlockId> {
        self.idom.get(&block).copied().filter(|&id| id != block)
    }

    /// 返回 `block` 在支配树中的所有直接子节点。
    pub fn children(&self, block: BlockId) -> &[BlockId] {
        self.children.get(&block).map_or(&[], |v| v.as_slice())
    }

    /// 返回 `block` 的支配边界 (dominance frontier)。
    ///
    /// 支配边界 DF[X] 是所有块 Y 的集合，使得 X 支配 Y 的某个前驱
    /// 但不严格支配 Y。这是插入 φ 节点所需的关键信息。
    ///
    /// 参考: Cytron et al. "Efficiently Computing SSA Form" (1991).
    pub fn dominance_frontier(&self, block: BlockId) -> Vec<BlockId> {
        static EMPTY_DF: LazyLock<HashSet<BlockId>> = LazyLock::new(HashSet::new);
        self.domin_frontiers
            .get(&block)
            .unwrap_or(&EMPTY_DF)
            .iter()
            .copied()
            .collect()
    }

    /// 返回所有块的支配边界映射的引用（用于需要遍历全部边界的场景）。
    pub fn all_dominance_frontiers(&self) -> &HashMap<BlockId, HashSet<BlockId>> {
        &self.domin_frontiers
    }

    /// 返回入口块 ID。
    pub fn entry(&self) -> BlockId {
        self.entry
    }

    /// 返回函数中的块总数。
    pub fn block_count(&self) -> usize {
        self.block_count
    }
}

/// 便捷函数：为函数构建支配树。
///
/// 等价于 `DominatorTree::build(func)`。
pub fn compute_dom_tree(func: &Function) -> DominatorTree {
    DominatorTree::build(func)
}

// ============================================================
// Cooper-Harvey-Kennedy immediate dominator computation
// ============================================================

fn compute_idom(
    func: &Function,
    entry: BlockId,
    postorder: &[BlockId],
    postorder_rank: &HashMap<BlockId, usize>,
) -> HashMap<BlockId, BlockId> {
    let mut idom: HashMap<BlockId, BlockId> = HashMap::new();
    idom.insert(entry, entry);

    let mut changed = true;
    while changed {
        changed = false;
        // Iterate in reverse postorder (≈ topological order of the dominator tree)
        for &block in postorder.iter().rev() {
            if block == entry {
                continue;
            }
            let preds = block_predecessors(func, block);
            let processed_preds: Vec<BlockId> =
                preds.into_iter().filter(|p| idom.contains_key(p)).collect();
            if processed_preds.is_empty() {
                continue;
            }
            let mut new_idom = processed_preds[0];
            for &pred in &processed_preds[1..] {
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

/// 在支配树中查找两个节点的最近公共祖先 (LCA)。
///
/// 使用后序排名 (postorder rank) 来决定哪个 "finger" 更靠近入口块，
/// 从而高效地向上移动。
fn intersect(
    idom: &HashMap<BlockId, BlockId>,
    postorder_rank: &HashMap<BlockId, usize>,
    mut finger1: BlockId,
    mut finger2: BlockId,
) -> BlockId {
    while finger1 != finger2 {
        let rank1 = postorder_rank.get(&finger1).copied().unwrap_or(0);
        let rank2 = postorder_rank.get(&finger2).copied().unwrap_or(0);
        if rank1 > rank2 {
            finger1 = *idom.get(&finger1).unwrap_or(&finger1);
        } else {
            finger2 = *idom.get(&finger2).unwrap_or(&finger2);
        }
    }
    finger1
}

fn compute_dom_children(idom: &HashMap<BlockId, BlockId>) -> HashMap<BlockId, Vec<BlockId>> {
    let mut children: HashMap<BlockId, Vec<BlockId>> = HashMap::new();
    for (&child, &parent) in idom {
        if child != parent {
            children.entry(parent).or_default().push(child);
        }
    }
    children
}

/// 递归计算支配集合：dom_set[parent] = {parent} ∪ all dom_sets[children].
fn compute_dom_sets(
    idom: &HashMap<BlockId, BlockId>,
    children: &HashMap<BlockId, Vec<BlockId>>,
) -> HashMap<BlockId, HashSet<BlockId>> {
    let mut sets: HashMap<BlockId, HashSet<BlockId>> = HashMap::new();
    let entry = idom.iter().find(|(k, v)| k == v).map(|(k, _)| *k);
    if let Some(entry) = entry {
        compute_dom_set_rec(entry, children, &mut sets);
    }
    sets
}

fn compute_dom_set_rec(
    node: BlockId,
    children: &HashMap<BlockId, Vec<BlockId>>,
    sets: &mut HashMap<BlockId, HashSet<BlockId>>,
) -> HashSet<BlockId> {
    let mut dom_set = HashSet::new();
    dom_set.insert(node);
    if let Some(kids) = children.get(&node) {
        for &child in kids {
            let child_set = compute_dom_set_rec(child, children, sets);
            dom_set.extend(child_set);
        }
    }
    sets.insert(node, dom_set.clone());
    dom_set
}

/// 计算支配边界：DF[X] = 所有 Y 使得 X 支配 Y 的某个前驱但 X 不严格支配 Y。
///
/// 算法 (Cytron et al. 1991):
/// 对于 CFG 中的每条边 A → B，沿支配树向上遍历，
/// 找到第一个严格支配 B 的节点为止，将其间的所有节点加入 DF 中。
fn compute_dominance_frontiers(
    func: &Function,
    idom: &HashMap<BlockId, BlockId>,
    dom_sets: &HashMap<BlockId, HashSet<BlockId>>,
) -> HashMap<BlockId, HashSet<BlockId>> {
    let mut df: HashMap<BlockId, HashSet<BlockId>> = HashMap::new();
    // Initialize empty frontiers for all blocks
    for block in func.iter_blocks() {
        df.entry(block.id).or_default();
    }
    for block in func.iter_blocks() {
        let b = block.id;
        let preds = block_predecessors(func, b);
        for a in preds {
            // Walk up the dominator tree: for each node X that strictly
            // dominates a but not b, add b to DF[X]. Stop when reaching idom(b).
            let mut x = a;
            loop {
                // Stop when x strictly dominates b (past the frontier)
                if x != b
                    && dom_sets
                        .get(&x)
                        .is_some_and(|s| s.contains(&b))
                {
                    break;
                }
                // If x does not dominate b (and x != b), this is a frontier add
                if x != b {
                    df.entry(x).or_default().insert(b);
                }
                // Move up to idom
                if let Some(&next) = idom.get(&x) {
                    if next == x {
                        break;
                    }
                    x = next;
                } else {
                    break;
                }
            }
        }
    }
    df
}

// ============================================================
// Tests
// ============================================================

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ir::builder::FunctionBuilder;
    use crate::ir::types::{IntCC, Signature};

    fn build_diamond_func() -> Function {
        let mut b = FunctionBuilder::new("diamond", Signature::new(&[], &[Type::I32]));
        let entry = b.create_block();
        b.switch_to_block(entry);
        let cond = b.iconst_i32(1);
        let t_block = b.create_block();
        b.switch_to_block(t_block);
        let tv = b.iconst_i32(42);
        let f_block = b.create_block();
        b.switch_to_block(f_block);
        let fv = b.iconst_i32(0);
        let merge = b.create_block();
        b.switch_to_block(merge);
        let phi = b.phi(&[(tv, t_block), (fv, f_block)], Type::I32);
        b.return_(&[phi]);
        b.switch_to_block(entry);
        b.branch(cond, t_block, f_block, &[], &[]);
        b.switch_to_block(t_block);
        b.jump(merge, &[]);
        b.switch_to_block(f_block);
        b.jump(merge, &[]);
        b.finish()
    }

    fn build_loop_func() -> Function {
        let mut b = FunctionBuilder::new("loop_fn", Signature::new(&[], &[Type::I32]));
        let entry = b.create_block();
        b.switch_to_block(entry);
        let header = b.create_block();
        b.jump(header, &[]);

        b.switch_to_block(header);
        let iv = b.iconst_i32(0);
        let body = b.create_block();
        b.switch_to_block(body);
        let _next = b.iconst_i32(1);
        let exit = b.create_block();
        b.switch_to_block(exit);
        b.return_(&[iv]);

        b.switch_to_block(entry);
        b.jump(header, &[]);
        b.switch_to_block(header);
        let c10 = b.iconst_i32(10);
        let cmp = b.icmp(IntCC::SignedLessThan, iv, c10);
        b.branch(cmp, body, exit, &[], &[]);
        b.switch_to_block(body);
        b.jump(header, &[]);
        b.finish()
    }

    // ============================================================
    // Dominator tree tests
    // ============================================================

    #[test]
    fn test_dominator_tree_diamond() {
        let func = build_diamond_func();
        let dt = DominatorTree::build(&func);
        let entry = func.blocks[0].id;

        // Entry should dominate all blocks
        for blk in &func.blocks {
            assert!(
                dt.dominates(entry, blk.id),
                "entry should dominate {}",
                blk.id
            );
        }

        // Entry has no immediate dominator
        assert_eq!(dt.idom(entry), None);
    }

    #[test]
    fn test_compute_dom_tree() {
        let func = build_diamond_func();
        let dt = compute_dom_tree(&func);
        assert_eq!(dt.block_count(), 4);
    }

    #[test]
    fn test_postorder() {
        let func = build_diamond_func();
        let entry = func.blocks[0].id;
        let postorder = compute_postorder(&func, entry);
        assert_eq!(*postorder.last().unwrap(), entry);
        assert_eq!(postorder.len(), 4);
    }

    #[test]
    fn test_block_successors() {
        let func = build_diamond_func();
        let entry = func.blocks[0].id;
        let succs = block_successors(&func.block(entry).unwrap().terminator);
        assert_eq!(succs.len(), 2);
    }

    #[test]
    fn test_block_successors_in_func() {
        let func = build_diamond_func();
        let entry = func.blocks[0].id;
        let succs = block_successors_in_func(&func, entry);
        assert_eq!(succs.len(), 2);
    }

    #[test]
    fn test_block_predecessors() {
        let func = build_diamond_func();
        let merge = func.blocks[3].id;
        let preds = block_predecessors(&func, merge);
        assert_eq!(preds.len(), 2);
    }

    #[test]
    fn test_strictly_dominates() {
        let func = build_diamond_func();
        let dt = DominatorTree::build(&func);
        let entry = func.blocks[0].id;
        let merge = func.blocks[3].id;
        assert!(dt.strictly_dominates(entry, merge));
        assert!(!dt.strictly_dominates(merge, merge));
        assert!(!dt.strictly_dominates(merge, entry));
    }

    #[test]
    fn test_dominance_frontier() {
        let func = build_diamond_func();
        let dt = DominatorTree::build(&func);
        for blk in &func.blocks {
            let df = dt.dominance_frontier(blk.id);
            assert!(df.len() <= func.blocks.len());
        }
    }

    #[test]
    fn test_children() {
        let func = build_diamond_func();
        let dt = DominatorTree::build(&func);
        let entry = func.blocks[0].id;
        let kids = dt.children(entry);
        // In a diamond, entry dominates all blocks and has a subset as direct children
        assert!(!kids.is_empty(), "entry should have at least one child in dom tree");
    }

    #[test]
    fn test_empty_function() {
        let func = Function::new("empty", Signature::void());
        let dt = DominatorTree::build(&func);
        assert_eq!(dt.block_count(), 0);
        let dt2 = compute_dom_tree(&func);
        assert_eq!(dt2.block_count(), 0);
    }

    #[test]
    fn test_single_block_function() {
        let sig = Signature::new(&[], &[Type::I32]);
        let mut b = FunctionBuilder::new("single", sig);
        let entry = b.create_block();
        b.switch_to_block(entry);
        let v = b.iconst_i32(42);
        b.return_(&[v]);
        let func = b.finish();

        let dt = DominatorTree::build(&func);
        let entry = func.blocks[0].id;
        assert_eq!(dt.block_count(), 1);
        assert!(dt.dominates(entry, entry));
        assert!(!dt.strictly_dominates(entry, entry));
        assert_eq!(dt.idom(entry), None);
        assert!(dt.children(entry).is_empty());
    }

    #[test]
    fn test_loop_dominates() {
        let func = build_loop_func();
        let dt = DominatorTree::build(&func);
        let header = func.blocks[1].id; // header block
        let body = func.blocks[2].id;   // body block

        // header should dominate body (header is the loop header)
        assert!(dt.dominates(header, body));
    }

    #[test]
    fn test_dominates_reflexive() {
        let func = build_diamond_func();
        let dt = DominatorTree::build(&func);
        for blk in &func.blocks {
            assert!(dt.dominates(blk.id, blk.id), "block should dominate itself");
        }
    }

    #[test]
    fn test_idom_non_entry() {
        let func = build_diamond_func();
        let dt = DominatorTree::build(&func);
        let entry = func.blocks[0].id;
        // All non-entry blocks should have an idom
        for blk in &func.blocks {
            if blk.id != entry {
                let idom = dt.idom(blk.id).expect("non-entry block should have idom");
                assert!(dt.strictly_dominates(idom, blk.id));
            }
        }
    }

    #[test]
    fn test_all_dominance_frontiers() {
        let func = build_diamond_func();
        let dt = DominatorTree::build(&func);
        let all_df = dt.all_dominance_frontiers();
        assert_eq!(all_df.len(), func.blocks.len());
    }
}
