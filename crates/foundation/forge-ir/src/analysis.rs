//! CFG 分析 — 支配树、前驱/后继、后序遍历。
//!
//! Cooper-Harvey-Kennedy 迭代算法计算立即支配者。

use crate::entity::*;
use crate::entity_map::SecondaryMap;
use crate::function::Function;
use crate::loop_info::LoopForest;
use std::sync::{Arc, RwLock};

// ============================================================
// CFG 辅助函数
// ============================================================

pub fn block_successors_in_func(func: &Function, block: Block) -> Vec<Block> {
    func.dfg.block_successors(block)
}

// ============================================================
// AnalysisManager — 惰性分析缓存的显式生命周期（v3 S6）
// ============================================================

/// 分析修订号 = (DFG 结构修订号, 管理器失效计数)。
///
/// **正确性只取决于它**：每个缓存槽里记着"这份结果算于哪个修订号"，读者拿到的
/// 永远是当前修订号对应的结果。改完控制流**不需要**任何人记得失效缓存——
/// 修订号变了就自动重算（`invalidate()` 只是省下一次重算的显式优化）。
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct AnalysisRevision {
    /// [`crate::dfg::DataFlowGraph::cfg_revision`]：块增删、终结符写入、墓碑化的计数。
    pub cfg: u64,
    /// [`AnalysisManager::invalidations`]：显式 `invalidate()` 的次数。
    pub invalidations: u64,
}

/// 一个惰性分析缓存槽：按修订号判定新鲜，过期即重算；读者拿到 `Arc` 快照。
///
/// 用 `RwLock<Option<(修订号, Arc<T>)>>` 而不是 `OnceLock<T>`：`OnceLock` 一旦
/// 初始化就只能靠 `&mut self` 清空，"改了控制流但漏了失效"会静默返回旧结果；
/// 这里过期自检 + 重算，且 `&self` 即可（快照式读，读到的分析在该调用内恒定）。
#[derive(Debug)]
pub struct AnalysisSlot<T> {
    cached: RwLock<Option<(AnalysisRevision, Arc<T>)>>,
}

impl<T> Default for AnalysisSlot<T> {
    fn default() -> Self {
        Self {
            cached: RwLock::new(None),
        }
    }
}

impl<T> AnalysisSlot<T> {
    /// 取当前修订号对应的结果：修订号一致 ⇒ 复用快照；否则重算并覆盖。
    ///
    /// 并发：两个线程同时发现过期时会各算一次，覆盖后仍等价（分析是纯函数）。
    fn get(&self, rev: AnalysisRevision, compute: impl FnOnce() -> T) -> Arc<T> {
        if let Some((cached_rev, value)) = self.cached.read().expect("分析缓存锁中毒").as_ref()
            && *cached_rev == rev
        {
            return Arc::clone(value);
        }
        let value = Arc::new(compute());
        *self.cached.write().expect("分析缓存锁中毒") = Some((rev, Arc::clone(&value)));
        value
    }

    fn clear(&self) {
        *self.cached.write().expect("分析缓存锁中毒") = None;
    }
}

/// 惰性分析缓存管理器（[`Function::analysis`]）：显式生命周期 + 修订号自校验。
///
/// 生命周期契约：
///
/// - 每个槽按 [`AnalysisRevision`] 自校验；`invalidate()` 只做"提前释放"，
///   **正确性不依赖任何调用方记得失效**（这是本类型存在的理由）；
/// - 读者拿 `Arc` 快照：快照在本次调用内恒定，不受后续控制流改写影响——
///   "先读旧图、再改 CFG、再读"不会看到半个新图（此前的 `&T` 引用语义会让
///   第二次读拿到被就地改写后的内容）。
#[derive(Default)]
pub struct AnalysisManager {
    invalidations: u64,
    predecessors: AnalysisSlot<SecondaryMap<Block, Vec<Block>>>,
    successors: AnalysisSlot<SecondaryMap<Block, Vec<Block>>>,
    dominator_tree: AnalysisSlot<DominatorTree>,
    loop_forest: AnalysisSlot<LoopForest>,
}

impl AnalysisManager {
    pub fn new() -> Self {
        Self::default()
    }

    /// 显式失效：失效计数 +1 并释放全部槽位。
    ///
    /// **这是优化而不是正确性前提**（见类型级文档）：忘了调用只是白算一次。
    /// 幂等、O(1)；需要 `&mut self`，与其它线程的 `&self` 共享由借用检查互斥。
    pub fn invalidate(&mut self) {
        self.invalidations += 1;
        self.predecessors.clear();
        self.successors.clear();
        self.dominator_tree.clear();
        self.loop_forest.clear();
    }

    /// 显式失效次数（构成 [`AnalysisRevision`] 的一半）。
    pub fn invalidations(&self) -> u64 {
        self.invalidations
    }

    /// 组装当前修订号；`cfg` 来自 [`Function::cfg_revision`]。
    pub fn revision(&self, cfg: u64) -> AnalysisRevision {
        AnalysisRevision {
            cfg,
            invalidations: self.invalidations,
        }
    }

    pub fn predecessors(
        &self,
        rev: AnalysisRevision,
        compute: impl FnOnce() -> SecondaryMap<Block, Vec<Block>>,
    ) -> Arc<SecondaryMap<Block, Vec<Block>>> {
        self.predecessors.get(rev, compute)
    }

    pub fn successors(
        &self,
        rev: AnalysisRevision,
        compute: impl FnOnce() -> SecondaryMap<Block, Vec<Block>>,
    ) -> Arc<SecondaryMap<Block, Vec<Block>>> {
        self.successors.get(rev, compute)
    }

    pub fn dominator_tree(
        &self,
        rev: AnalysisRevision,
        compute: impl FnOnce() -> DominatorTree,
    ) -> Arc<DominatorTree> {
        self.dominator_tree.get(rev, compute)
    }

    pub fn loop_forest(
        &self,
        rev: AnalysisRevision,
        compute: impl FnOnce() -> LoopForest,
    ) -> Arc<LoopForest> {
        self.loop_forest.get(rev, compute)
    }
}

// ============================================================
// DominatorTree
// ============================================================

#[derive(Clone, Debug)]
pub struct DominatorTree {
    pub entry: Block,
    children: SecondaryMap<Block, Vec<Block>>,
    /// DFS 进入/离开时间戳（支配树上的区间序）。
    /// a 支配 b ⟺ tin[a] ≤ tin[b] 且 tout[b] ≤ tout[a]——O(1) 查询、
    /// O(n) 空间（替代原 dom_sets 的每块全套支配者 HashSet，O(n²) 空间）。
    tin: SecondaryMap<Block, u32>,
    tout: SecondaryMap<Block, u32>,
    /// 立即支配者（entry 为自身；P1-10 新增——循环分析/块参数 coalesce/
    /// SCEV 需要 idom/depth/NCD 查询）。
    idom: SecondaryMap<Block, Block>,
    /// 支配深度（entry=0，子块=父+1）。
    depth: SecondaryMap<Block, u32>,
    block_count: usize,
}

impl DominatorTree {
    pub fn build(func: &Function) -> Self {
        let block_count = func.dfg.block_count();
        if block_count == 0 {
            return Self::empty();
        }
        let entry = func.entry();

        let postorder = compute_postorder(func, entry);
        let postorder_rank: SecondaryMap<Block, usize> =
            postorder.iter().enumerate().map(|(i, &b)| (b, i)).collect();

        let idom = compute_idom(func, entry, &postorder, &postorder_rank);
        let children = compute_children(&idom);
        let (tin, tout) = compute_intervals(entry, &children);
        // 从 idom 推导深度：entry=0，沿 idom 链累加（O(n) 每块沿链到根，
        // 总 O(n·depth) —— 对典型 CFG 足够；深链退化可改 BFS 优化）。
        let mut depth: SecondaryMap<Block, u32> = SecondaryMap::new();
        depth.insert(entry, 0);
        for &b in &postorder {
            if b == entry {
                continue;
            }
            if let Some(&parent) = idom.get(b) {
                let d = depth.get(parent).copied().unwrap_or(0) + 1;
                depth.insert(b, d);
            }
        }

        Self {
            entry,
            children,
            tin,
            tout,
            idom,
            depth,
            block_count,
        }
    }

    fn empty() -> Self {
        Self {
            // 空函数（无块）的退化值：没有任何查询会用到它（block_count == 0
            // 时所有查询短路），故这里不是"入口约定"，只是占位。
            entry: Block(0),
            children: SecondaryMap::new(),
            tin: SecondaryMap::new(),
            tout: SecondaryMap::new(),
            idom: SecondaryMap::new(),
            depth: SecondaryMap::new(),
            block_count: 0,
        }
    }

    pub fn dominates(&self, a: Block, b: Block) -> bool {
        if a == b {
            return true;
        }
        // 区间判定：a 支配 b ⟺ b 落在 a 的 DFS 子树区间内
        match (
            self.tin.get(a),
            self.tout.get(a),
            self.tin.get(b),
            self.tout.get(b),
        ) {
            (Some(&ta_in), Some(&ta_out), Some(&tb_in), Some(&tb_out)) => {
                ta_in <= tb_in && tb_out <= ta_out
            }
            _ => false,
        }
    }

    pub fn children(&self, block: Block) -> &[Block] {
        self.children.get(block).map_or(&[], |v| v.as_slice())
    }

    /// 立即支配者（P1-10）：entry 的 idom 是自身。
    pub fn idom(&self, block: Block) -> Option<Block> {
        self.idom.get(block).copied()
    }

    /// 支配深度（P1-10）：entry=0。
    pub fn depth(&self, block: Block) -> u32 {
        self.depth.get(block).copied().unwrap_or(0)
    }

    /// 最近公共支配者（P1-10）：沿 idom 链求交（浅者优先）。
    /// 用深度对齐后同步上升——O(depth) 查询。
    pub fn ncd(&self, a: Block, b: Block) -> Option<Block> {
        let mut x = a;
        let mut y = b;
        loop {
            let dx = self.depth(x);
            let dy = self.depth(y);
            if dx > dy {
                x = self.idom(x)?;
            } else if dy > dx {
                y = self.idom(y)?;
            } else if x == y {
                return Some(x);
            } else {
                x = self.idom(x)?;
                y = self.idom(y)?;
            }
        }
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
    postorder_rank: &SecondaryMap<Block, usize>,
) -> SecondaryMap<Block, Block> {
    // 一次构建前驱映射（替代每轮对每个 block 线性扫全函数，O(轮数×n²) → O(轮数×n)）。
    let mut preds_map: SecondaryMap<Block, Vec<Block>> = SecondaryMap::new();
    for (block, _bd) in func.dfg.blocks() {
        // CFG 构造容忍未终止的块（校验器会在坏 IR 上跑）：
        // "没有终结符" ⇒ 没有后继边，这是结构事实，不是静默回退。
        for succ in func.dfg.block_successors(block) {
            preds_map.get_mut_or_default(succ).push(block);
        }
    }

    let mut idom: SecondaryMap<Block, Block> = SecondaryMap::new();
    idom.insert(entry, entry);

    let mut changed = true;
    while changed {
        changed = false;
        for &block in postorder.iter().rev() {
            if block == entry {
                continue;
            }
            let preds = preds_map.get(block).map(|v| v.as_slice()).unwrap_or(&[]);
            let processed: Vec<Block> = preds
                .iter()
                .copied()
                .filter(|p| idom.contains_key(*p))
                .collect();
            if processed.is_empty() {
                continue;
            }
            let mut new_idom = processed[0];
            for &pred in &processed[1..] {
                new_idom = intersect(&idom, postorder_rank, new_idom, pred);
            }
            if idom.get(block).copied() != Some(new_idom) {
                idom.insert(block, new_idom);
                changed = true;
            }
        }
    }
    idom
}

fn intersect(
    idom: &SecondaryMap<Block, Block>,
    postorder_rank: &SecondaryMap<Block, usize>,
    mut finger1: Block,
    mut finger2: Block,
) -> Block {
    // Cooper-Harvey-Kennedy algorithm: in postorder numbering,
    // higher rank = closer to entry. The deeper node has LOWER rank
    // and must be advanced up the idom chain.
    while finger1 != finger2 {
        let rank1 = postorder_rank.get(finger1).copied().unwrap_or(0);
        let rank2 = postorder_rank.get(finger2).copied().unwrap_or(0);
        if rank1 < rank2 {
            // finger1 is deeper (lower postorder rank) → advance it
            finger1 = idom.get(finger1).copied().unwrap_or(finger1);
        } else {
            // finger2 is deeper (or equal rank) → advance it
            finger2 = idom.get(finger2).copied().unwrap_or(finger2);
        }
    }
    finger1
}

fn compute_children(idom: &SecondaryMap<Block, Block>) -> SecondaryMap<Block, Vec<Block>> {
    let mut children: SecondaryMap<Block, Vec<Block>> = SecondaryMap::new();
    for (child, parent) in idom.iter() {
        if child != *parent {
            children.get_mut_or_default(*parent).push(child);
        }
    }
    children
}

/// 支配树上的 DFS 进入/离开时间戳（区间序，迭代实现防深树栈溢出）。
/// a 支配 b ⟺ tin[a] ≤ tin[b] 且 tout[b] ≤ tout[a]。
fn compute_intervals(
    entry: Block,
    children: &SecondaryMap<Block, Vec<Block>>,
) -> (SecondaryMap<Block, u32>, SecondaryMap<Block, u32>) {
    let mut tin: SecondaryMap<Block, u32> = SecondaryMap::new();
    let mut tout: SecondaryMap<Block, u32> = SecondaryMap::new();
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
        if tin.contains_key(node) {
            continue;
        }
        timer += 1;
        tin.insert(node, timer);
        stack.push((node, true));
        if let Some(kids) = children.get(node) {
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

    /// P1-10：idom/depth/ncd 查询（if-else 汇合：entry idom 自身、then/else
    /// 的 idom 是 entry、merge 的 idom 是 entry、ncd(then, else)=entry）。
    #[test]
    fn test_idom_depth_ncd() {
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
            fb.switch_to_block(merge_blk);
            let params = fb.func.dfg.block_param_values(merge_blk).to_vec();
            fb.ret(&[params[0]]);
        }
        let func = fb.finish().expect("build");
        let dt = DominatorTree::build(&func);

        // idom：entry 自身
        assert_eq!(dt.idom(entry), Some(entry));
        // then/else 的 idom 是 entry
        assert_eq!(dt.idom(then_blk), Some(entry));
        assert_eq!(dt.idom(else_blk), Some(entry));
        // depth：entry=0、then/else=1、merge=1
        assert_eq!(dt.depth(entry), 0);
        assert_eq!(dt.depth(then_blk), 1);
        assert_eq!(dt.depth(else_blk), 1);
        assert_eq!(dt.depth(merge_blk), 1);
        // ncd(then, else) = entry
        assert_eq!(dt.ncd(then_blk, else_blk), Some(entry));
        // ncd 自反
        assert_eq!(dt.ncd(then_blk, then_blk), Some(then_blk));
    }
}
