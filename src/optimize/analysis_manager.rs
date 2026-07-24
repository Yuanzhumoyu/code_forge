//! 分析管理器 -- 缓存优化的分析结果，避免重复计算。
//!
//! 注意：这是优化管线的分析缓存管理器，用于缓存 [DominatorTree] / [LoopForest]
//! 等分析结果。原始的支配树算法实现在 [`crate::ir::analysis`]，
//! 循环检测实现在 [`crate::ir::loop_info`]。
//!
//! 当前各优化 pass 独立构建分析结果，导致相同分析被多次重复计算。
//! `AnalysisManager` 提供懒缓存 + 失效机制。

use crate::ir::{BlockId, DominatorTree, Function, LoopForest};
use std::collections::HashSet;

/// 分析结果的标识符。
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum AnalysisKind {
    /// 支配树 (DominatorTree)。
    DominatorTree,
    /// 原始支配关系（每个块的支配者集合）。
    Dominators,
    /// 前驱块列表。
    Predecessors,
    /// 循环森林 (LoopForest) -- 自然循环检测结果。
    Loops,
    /// 值使用链。
    UseLists,
}

/// 分析管理器 -- 缓存函数的分析结果。
///
/// Pass 通过此管理器请求分析结果，无需每次都重新计算。
/// 当 pass 修改 IR 后，调用 `invalidate` 标记失效的分析。
///
/// # Example
///
/// ```ignore
/// let mut am = AnalysisManager::new();
/// let dom_tree = am.get_dominator_tree(func);  // 首次计算，缓存
/// let lf = am.get_loop_forest(func, dom_tree);  // 使用缓存的 dom_tree
/// am.invalidate(AnalysisKind::DominatorTree); // 修改 CFG 后失效
/// ```
#[derive(Clone, Debug)]
pub struct AnalysisManager {
    /// 缓存的支配树。
    dominator_tree: Option<DominatorTree>,
    /// 缓存的原始支配关系。
    dominators: Option<Vec<HashSet<BlockId>>>,
    /// 缓存的前驱列表。
    predecessors: Option<Vec<Vec<BlockId>>>,
    /// 缓存的循环森林。
    loop_forest: Option<LoopForest>,
    /// 缓存的回边列表（用于向后兼容的 get_loops）。
    back_edges: Option<Vec<(BlockId, BlockId)>>,
}

impl AnalysisManager {
    pub fn new() -> Self {
        Self {
            dominator_tree: None,
            dominators: None,
            predecessors: None,
            loop_forest: None,
            back_edges: None,
        }
    }

    /// 获取支配树。
    /// 缓存有效时直接返回，否则重新计算。
    pub fn get_dominator_tree(&mut self, func: &Function) -> &DominatorTree {
        self.dominator_tree
            .get_or_insert_with(|| DominatorTree::build(func))
    }

    /// 获取循环森林。
    ///
    /// 需要支配树 — 如果已缓存则复用，否则每次独立构建。
    /// 注意：当 IR 被修改后，必须调用 `invalidate_cfg()` 失效所有 CFG 分析。
    pub fn get_loop_forest(&mut self, func: &Function) -> &LoopForest {
        let dt = self.get_dominator_tree(func).clone();
        self.loop_forest
            .get_or_insert_with(|| LoopForest::build(func, &dt))
    }

    /// 获取原始支配关系（每个块的支配者集合）。
    ///
    /// 注意：此方法构建临时的原始支配集合，与 `DominatorTree` 中的 `dominates()` 查询效率不同。
    /// 如需频繁查询支配关系，优先使用 `get_dominator_tree()` + `dominates()`。
    pub fn get_dominators(&mut self, func: &Function) -> &Vec<HashSet<BlockId>> {
        self.dominators.get_or_insert_with(|| {
            let n = func.blocks.len();
            if n == 0 {
                return Vec::new();
            }
            let preds = func.predecessors();
            let all_blocks: HashSet<BlockId> = (0..n as u32).map(BlockId).collect();
            let mut doms: Vec<HashSet<BlockId>> = vec![all_blocks; n];
            doms[0] = {
                let mut s = HashSet::new();
                s.insert(BlockId(0));
                s
            };
            let mut changed = true;
            while changed {
                changed = false;
                for b in 1..n {
                    let mut new_dom: HashSet<BlockId> = if preds[b].is_empty() {
                        HashSet::new()
                    } else {
                        let mut iter = preds[b].iter();
                        let first = *iter.next().unwrap();
                        let mut intersection = doms[first.0 as usize].clone();
                        for p in iter {
                            intersection = intersection
                                .intersection(&doms[p.0 as usize])
                                .copied()
                                .collect();
                        }
                        intersection
                    };
                    new_dom.insert(BlockId(b as u32));
                    if new_dom != doms[b] {
                        doms[b] = new_dom;
                        changed = true;
                    }
                }
            }
            doms
        })
    }

    /// 获取前驱块列表。
    pub fn get_predecessors(&mut self, func: &Function) -> &Vec<Vec<BlockId>> {
        self.predecessors
            .get_or_insert_with(|| func.predecessors())
    }

    /// 获取循环信息（后边列表）。
    ///
    /// 返回所有回边作为 `(header, tail)` 对。
    /// 优先使用 `get_loop_forest()` 获取完整的循环森林信息。
    #[deprecated(note = "Use get_loop_forest() instead for richer loop information")]
    pub fn get_loops(&mut self, func: &Function) -> &Vec<(BlockId, BlockId)> {
        self.back_edges.get_or_insert_with(|| {
            let dt = DominatorTree::build(func);
            let mut back_edges = Vec::new();
            for block in func.iter_blocks() {
                for succ in crate::ir::block_successors(&block.terminator) {
                    if dt.dominates(succ, block.id) {
                        back_edges.push((succ, block.id));
                    }
                }
            }
            back_edges
        })
    }

    /// 失效指定分析。
    /// 当 pass 修改了 IR（如添加/删除块、修改 CFG），
    /// 调用此方法使相关分析缓存失效。
    pub fn invalidate(&mut self, kind: AnalysisKind) {
        match kind {
            AnalysisKind::DominatorTree => self.dominator_tree = None,
            AnalysisKind::Dominators => self.dominators = None,
            AnalysisKind::Predecessors => self.predecessors = None,
            AnalysisKind::Loops => {
                self.loop_forest = None;
                self.back_edges = None;
            }
            AnalysisKind::UseLists => { /* 由 Function 自身管理 */ }
        }
    }

    /// 失效所有分析缓存。
    pub fn invalidate_all(&mut self) {
        self.dominator_tree = None;
        self.dominators = None;
        self.predecessors = None;
        self.loop_forest = None;
        self.back_edges = None;
    }

    /// 失效与 CFG 相关的所有分析（当控制流图改变时调用）。
    pub fn invalidate_cfg(&mut self) {
        self.dominator_tree = None;
        self.dominators = None;
        self.predecessors = None;
        self.loop_forest = None;
        self.back_edges = None;
    }
}

impl Default for AnalysisManager {
    fn default() -> Self {
        Self::new()
    }
}

// ============================================================
// 测试
// ============================================================

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ir::*;

    #[test]
    fn test_analysis_dominator_tree_cache() {
        let sig = Signature::new(&[], &[Type::I32]);
        let mut b = FunctionBuilder::new("test", sig);
        let entry = b.create_block();
        let then_block = b.create_block();
        let else_block = b.create_block();
        let merge = b.create_block();

        b.switch_to_block(entry);
        let c = b.iconst_i32(1);
        b.branch(c, then_block, else_block, &[], &[]);
        b.switch_to_block(then_block);
        b.jump(merge, &[]);
        b.switch_to_block(else_block);
        b.jump(merge, &[]);
        b.switch_to_block(merge);
        b.return_(&[]);

        let func = b.finish();
        let mut am = AnalysisManager::new();

        // First access: compute and cache
        let count = am.get_dominator_tree(&func).block_count();
        assert!(count > 0);

        // Second access: should hit cache
        let dt2 = am.get_dominator_tree(&func);
        assert_eq!(dt2.block_count(), count);
    }

    #[test]
    fn test_analysis_loop_forest_cache() {
        let sig = Signature::new(&[], &[Type::I32]);
        let mut b = FunctionBuilder::new("test_loop", sig);
        let entry = b.create_block();
        let loop_body = b.create_block();
        let exit = b.create_block();

        b.switch_to_block(entry);
        b.jump(loop_body, &[]);
        b.switch_to_block(loop_body);
        let c = b.iconst_i32(1);
        b.branch(c, loop_body, exit, &[], &[]);
        b.switch_to_block(exit);
        b.return_(&[]);

        let func = b.finish();
        let mut am = AnalysisManager::new();

        let lf = am.get_loop_forest(&func);
        assert!(!lf.is_empty(), "Expected at least one loop");
    }

    #[test]
    fn test_analysis_invalidate() {
        let sig = Signature::new(&[], &[Type::I32]);
        let mut b = FunctionBuilder::new("test", sig);
        let entry = b.create_block();
        b.switch_to_block(entry);
        b.return_(&[]);
        let func = b.finish();

        let mut am = AnalysisManager::new();

        // Cache predecessors
        let _ = am.get_predecessors(&func);
        assert!(am.predecessors.is_some());

        // Invalidate should clear
        am.invalidate(AnalysisKind::Predecessors);
        assert!(am.predecessors.is_none());
    }

    #[test]
    fn test_cfg_invalidation() {
        let mut am = AnalysisManager::new();
        let sig = Signature::new(&[], &[Type::I32]);
        let mut b = FunctionBuilder::new("test", sig);
        let entry = b.create_block();
        b.switch_to_block(entry);
        b.return_(&[]);
        let func = b.finish();

        let _ = am.get_dominator_tree(&func);
        let _ = am.get_loop_forest(&func);
        assert!(am.dominator_tree.is_some());
        assert!(am.loop_forest.is_some());

        am.invalidate_cfg();
        assert!(am.dominator_tree.is_none());
        assert!(am.loop_forest.is_none());
    }

    #[test]
    fn test_back_edges_analysis() {
        let sig = Signature::new(&[], &[Type::I32]);
        let mut b = FunctionBuilder::new("test_loop", sig);
        let entry = b.create_block();
        let loop_body = b.create_block();
        let exit = b.create_block();

        b.switch_to_block(entry);
        b.jump(loop_body, &[]);
        b.switch_to_block(loop_body);
        let c = b.iconst_i32(1);
        b.branch(c, loop_body, exit, &[], &[]);
        b.switch_to_block(exit);
        b.return_(&[]);

        let func = b.finish();
        let mut am = AnalysisManager::new();
        #[allow(deprecated)]
        let loops = am.get_loops(&func);
        assert!(!loops.is_empty(), "Expected at least one loop");
    }
}
