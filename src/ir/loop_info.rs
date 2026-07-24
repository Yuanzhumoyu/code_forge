//! 自然循环信息 (Natural Loop Info) -- 循环森林 (Loop Forest)。
//!
//! 基于支配关系的回边检测识别自然循环，构建循环嵌套森林。
//! 与 [`DominatorTree`](super::analysis::DominatorTree) 配合使用。
//!
//! ## 算法
//!
//! - **回边检测**: CFG 边 A → B，其中 B dominates A（B 是循环头）。
//! - **循环体收集**: 从 A 反向可达的所有块（不经过 B），加上 B 自身。
//! - **嵌套**: 通过块包含关系确定父子循环。
//!
//! 参考: Aho, Lam, Sethi, Ullman "Compilers: Principles, Techniques, and Tools" (2006).

use super::analysis::*;
use super::function::*;
use super::types::*;
use std::collections::{HashMap, HashSet};

// ============================================================
// LoopInfo -- 单个自然循环的信息
// ============================================================

/// 单个自然循环的信息。
///
/// 自然循环 (natural loop) 由一条回边定义：
/// tail → header，其中 header 支配 tail。
/// 循环体包含所有可以从 tail 反向到达（不经过 header）的块，加上 header。
#[derive(Clone, Debug)]
pub struct LoopInfo {
    /// 循环头 (loop header) -- 支配回边源的基本块。
    pub header: BlockId,
    /// 循环中的所有基本块（包括 header）。
    pub blocks: Vec<BlockId>,
    /// 循环嵌套深度 (1 = 最外层循环)。
    pub depth: u32,
    /// 父循环在 `LoopForest` 中的索引。
    pub parent_loop: Option<usize>,
    /// 循环出口块 -- 循环体内有边指向循环外的块。
    pub exit_blocks: Vec<BlockId>,
}

// ============================================================
// LoopForest -- 函数中所有自然循环的集合
// ============================================================

/// 循环森林 -- 函数中所有自然循环的集合。
///
/// 通过回边检测识别所有循环，计算嵌套关系和深度。
/// 提供高效的循环查询接口。
#[derive(Clone, Debug)]
pub struct LoopForest {
    /// 所有循环（按发现顺序排列）。
    loops: Vec<LoopInfo>,
    /// 循环头到循环索引的映射。
    header_to_loop: HashMap<BlockId, usize>,
    /// 块到其最内层循环索引的映射。
    block_to_loop: HashMap<BlockId, usize>,
    /// 每个块的循环深度。
    depths: HashMap<BlockId, u32>,
}

impl LoopForest {
    /// 为函数构建循环森林。
    ///
    /// 需要预先构建的 [`DominatorTree`]。
    ///
    /// # 算法
    ///
    /// 1. 扫描所有 CFG 边，检测回边（successor dominates predecessor）
    /// 2. 对每对 (header, tail)，反向收集循环体
    /// 3. 计算出口块
    /// 4. 通过块包含关系确定嵌套（最内层优先映射）
    pub fn build(func: &Function, dom_tree: &DominatorTree) -> Self {
        let mut loops: Vec<LoopInfo> = Vec::new();
        let mut header_to_loop: HashMap<BlockId, usize> = HashMap::new();

        // Step 1: Detect back-edges and build initial loops
        for block in func.iter_blocks() {
            let pred = block.id;
            for succ in block_successors(&block.terminator) {
                // Back-edge: succ dominates pred
                if dom_tree.dominates(succ, pred) {
                    let header = succ;
                    if let Some(&loop_idx) = header_to_loop.get(&header) {
                        // Existing loop with this header: expand body with new back-edge source
                        let existing = &mut loops[loop_idx];
                        collect_loop_body(func, pred, header, &mut existing.blocks);
                    } else {
                        let mut blocks: Vec<BlockId> = Vec::new();
                        blocks.push(header);
                        collect_loop_body(func, pred, header, &mut blocks);

                        let idx = loops.len();
                        loops.push(LoopInfo {
                            header,
                            blocks,
                            depth: 1,
                            parent_loop: None,
                            exit_blocks: Vec::new(),
                        });
                        header_to_loop.insert(header, idx);
                    }
                }
            }
        }

        // Step 2: Compute exit blocks for each loop
        for l in &mut loops {
            let block_set: HashSet<BlockId> = l.blocks.iter().copied().collect();
            for &b in &l.blocks {
                if let Some(block) = func.block(b) {
                    for succ in block_successors(&block.terminator) {
                        if !block_set.contains(&succ)
                            && !l.exit_blocks.contains(&b) {
                                l.exit_blocks.push(b);
                            }
                    }
                }
            }
        }

        // Build block sets for nesting computation
        let loop_sets: Vec<HashSet<BlockId>> = loops
            .iter()
            .map(|l| l.blocks.iter().copied().collect())
            .collect();

        // Step 3: Compute nesting via set containment
        // If loop_i strictly contains loop_j, loop_i is a candidate parent of loop_j.
        // The smallest container is the direct parent.
        for i in 0..loops.len() {
            for j in 0..loops.len() {
                if i == j {
                    continue;
                }
                if loop_sets[i].is_superset(&loop_sets[j])
                    && loop_sets[i].len() > loop_sets[j].len()
                {
                    // Check if i is the "tightest" parent so far for j
                    let is_tighter = match loops[j].parent_loop {
                        None => true,
                        Some(existing_parent) => loop_sets[i].len() < loop_sets[existing_parent].len(),
                    };
                    if is_tighter {
                        loops[j].parent_loop = Some(i);
                    }
                }
            }
        }

        // Step 4: Compute depths recursively
        let loop_depths: Vec<u32> = (0..loops.len())
            .map(|idx| compute_depth(&loops, idx))
            .collect();

        for (idx, l) in loops.iter_mut().enumerate() {
            l.depth = loop_depths[idx];
        }

        // Step 5: Build per-block mappings
        let mut depths: HashMap<BlockId, u32> = HashMap::new();
        let mut block_to_loop: HashMap<BlockId, usize> = HashMap::new();

        for (loop_idx, l) in loops.iter().enumerate() {
            for &b in &l.blocks {
                // Update depth to max (innermost)
                let e = depths.entry(b).or_insert(0);
                *e = (*e).max(l.depth);

                // Map block to its innermost loop (highest depth)
                let existing = block_to_loop.get(&b).copied();
                let update = match existing {
                    None => true,
                    Some(ex_idx) => loops[loop_idx].depth > loops[ex_idx].depth,
                };
                if update {
                    block_to_loop.insert(b, loop_idx);
                }
            }
        }

        // All blocks default to depth 0 (not in any loop)
        for block in func.iter_blocks() {
            depths.entry(block.id).or_insert(0);
        }

        Self {
            loops,
            header_to_loop,
            block_to_loop,
            depths,
        }
    }

    /// 检查指定块是否为循环头。
    pub fn is_loop_header(&self, block: BlockId) -> bool {
        self.header_to_loop.contains_key(&block)
    }

    /// 获取指定块的循环嵌套深度。
    ///
    /// 0 表示不在任何循环中，1 表示最外层循环，依此类推。
    pub fn get_loop_depth(&self, block: BlockId) -> u32 {
        self.depths.get(&block).copied().unwrap_or(0)
    }

    /// 获取包含指定块的循环（如果有的话）。
    ///
    /// 如果块在多个嵌套循环中，返回最内层循环。
    pub fn get_loop_for(&self, block: BlockId) -> Option<&LoopInfo> {
        self.block_to_loop
            .get(&block)
            .and_then(|&idx| self.loops.get(idx))
    }

    /// 根据循环头获取循环信息。
    pub fn loop_for_header(&self, header: BlockId) -> Option<&LoopInfo> {
        self.header_to_loop
            .get(&header)
            .and_then(|&idx| self.loops.get(idx))
    }

    /// 返回所有顶层循环（没有父循环的循环）。
    pub fn top_level_loops(&self) -> Vec<&LoopInfo> {
        self.loops.iter().filter(|l| l.parent_loop.is_none()).collect()
    }

    /// 返回所有循环的切片。
    pub fn all_loops(&self) -> &[LoopInfo] {
        &self.loops
    }

    /// 返回所有循环的可变切片。
    pub fn all_loops_mut(&mut self) -> &mut [LoopInfo] {
        &mut self.loops
    }

    /// 返回循环总数。
    pub fn len(&self) -> usize {
        self.loops.len()
    }

    /// 是否没有发现任何循环。
    pub fn is_empty(&self) -> bool {
        self.loops.is_empty()
    }
}

// ============================================================
// Internal helpers
// ============================================================

/// 收集循环体：从 `start` 反向可达的所有块（不通过 `header`）。
///
/// 使用工作列表算法，反向遍历前驱：
/// - 遇到 `header` 时停止（不将其重新加入工作列表）
/// - 遇到已访问的块时跳过
/// - 其他所有前驱加入工作列表和 body
fn collect_loop_body(func: &Function, start: BlockId, header: BlockId, body: &mut Vec<BlockId>) {
    let mut body_set: HashSet<BlockId> = body.iter().copied().collect();
    let mut worklist = vec![start];
    if !body_set.contains(&start) {
        body.push(start);
        body_set.insert(start);
    }

    while let Some(current) = worklist.pop() {
        if current == header {
            continue;
        }
        for pred in block_predecessors(func, current) {
            if !body_set.contains(&pred) && pred != header {
                body.push(pred);
                body_set.insert(pred);
                worklist.push(pred);
            } else if pred == header && !body_set.contains(&header) {
                body.push(header);
                body_set.insert(header);
            }
        }
    }
}

/// 递归计算循环深度。
fn compute_depth(loops: &[LoopInfo], idx: usize) -> u32 {
    let l = &loops[idx];
    l.parent_loop.map_or(1, |p| compute_depth(loops, p) + 1)
}

// ============================================================
// Tests
// ============================================================

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ir::builder::FunctionBuilder;
    use crate::ir::types::{IntCC, Signature};

    fn build_simple_loop_func() -> Function {
        // entry -> header <-> body
        //           |
        //           v
        //          exit
        let mut b = FunctionBuilder::new("simple_loop", Signature::new(&[], &[Type::I32]));
        let entry = b.create_block();
        b.switch_to_block(entry);
        let header = b.create_block();
        b.jump(header, &[]);

        b.switch_to_block(header);
        let iv = b.iconst_i32(0);
        let body = b.create_block();
        b.switch_to_block(body);
        let one = b.iconst_i32(1);
        let _next = b.iadd(iv, one);
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

    fn build_diamond_func() -> Function {
        // entry -> T_block -> merge
        //      \-> F_block -/
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

    fn build_nested_loop_func() -> Function {
        // entry -> outer_hdr -> inner_hdr -> inner_body
        //            |             |--> exit
        //            v
        //           outer_body
        let mut b = FunctionBuilder::new("nested", Signature::new(&[], &[Type::I32]));
        let entry = b.create_block();
        let outer_hdr = b.create_block();
        let inner_hdr = b.create_block();
        let inner_body = b.create_block();
        let outer_body = b.create_block();
        let exit = b.create_block();

        b.switch_to_block(entry);
        b.jump(outer_hdr, &[]);

        b.switch_to_block(outer_hdr);
        let oiv = b.iconst_i32(0);
        let c10 = b.iconst_i32(10);
        let cmp_o = b.icmp(IntCC::SignedLessThan, oiv, c10);
        b.branch(cmp_o, inner_hdr, exit, &[], &[]);

        b.switch_to_block(inner_hdr);
        let iiv = b.iconst_i32(0);
        let c5 = b.iconst_i32(5);
        let cmp_i = b.icmp(IntCC::SignedLessThan, iiv, c5);
        b.branch(cmp_i, inner_body, outer_body, &[], &[]);

        b.switch_to_block(inner_body);
        let one = b.iconst_i32(1);
        let _niiv = b.iadd(iiv, one);
        b.jump(inner_hdr, &[]);

        b.switch_to_block(outer_body);
        let one2 = b.iconst_i32(1);
        let _noiv = b.iadd(oiv, one2);
        b.jump(outer_hdr, &[]);

        b.switch_to_block(exit);
        let c42 = b.iconst_i32(42);
        b.return_(&[c42]);
        b.finish()
    }

    // ============================================================
    // Loop detection tests
    // ============================================================

    #[test]
    fn test_loop_forest_build() {
        let func = build_simple_loop_func();
        let dt = DominatorTree::build(&func);
        let lf = LoopForest::build(&func, &dt);

        assert!(!lf.is_empty(), "should detect at least one loop");
        assert_eq!(lf.len(), 1);
    }

    #[test]
    fn test_no_loops_in_diamond() {
        let func = build_diamond_func();
        let dt = DominatorTree::build(&func);
        let lf = LoopForest::build(&func, &dt);

        assert!(lf.is_empty(), "diamond has no loops");
    }

    #[test]
    fn test_is_loop_header() {
        let func = build_simple_loop_func();
        let dt = DominatorTree::build(&func);
        let lf = LoopForest::build(&func, &dt);

        // header block (index 1) should be a loop header
        let header = func.blocks[1].id;
        assert!(lf.is_loop_header(header));

        // entry block should not be a loop header
        let entry = func.blocks[0].id;
        assert!(!lf.is_loop_header(entry));
    }

    #[test]
    fn test_get_loop_depth() {
        let func = build_simple_loop_func();
        let dt = DominatorTree::build(&func);
        let lf = LoopForest::build(&func, &dt);

        let entry = func.blocks[0].id;
        assert_eq!(lf.get_loop_depth(entry), 0, "entry is not in any loop");

        let header = func.blocks[1].id;
        assert!(lf.get_loop_depth(header) >= 1, "header is in a loop");
    }

    #[test]
    fn test_get_loop_for() {
        let func = build_simple_loop_func();
        let dt = DominatorTree::build(&func);
        let lf = LoopForest::build(&func, &dt);

        let header = func.blocks[1].id;
        let loop_info = lf.get_loop_for(header);
        assert!(loop_info.is_some(), "header should be in a loop");
        assert_eq!(loop_info.unwrap().header, header);

        let entry = func.blocks[0].id;
        assert!(lf.get_loop_for(entry).is_none(), "entry is not in any loop");
    }

    #[test]
    fn test_loop_for_header() {
        let func = build_simple_loop_func();
        let dt = DominatorTree::build(&func);
        let lf = LoopForest::build(&func, &dt);

        let header = func.blocks[1].id;
        let loop_info = lf.loop_for_header(header);
        assert!(loop_info.is_some());
        assert_eq!(loop_info.unwrap().header, header);
    }

    #[test]
    fn test_loop_blocks_contain_header_and_body() {
        let func = build_simple_loop_func();
        let dt = DominatorTree::build(&func);
        let lf = LoopForest::build(&func, &dt);

        let top = lf.top_level_loops();
        assert_eq!(top.len(), 1);
        let l = top[0];

        let header = func.blocks[1].id;
        let body = func.blocks[2].id;

        assert!(l.blocks.contains(&header), "loop should contain its header");
        assert!(l.blocks.contains(&body), "loop should contain body block");
    }

    #[test]
    fn test_exit_blocks_non_empty() {
        let func = build_simple_loop_func();
        let dt = DominatorTree::build(&func);
        let lf = LoopForest::build(&func, &dt);

        let top = lf.top_level_loops();
        let l = top[0];
        assert!(!l.exit_blocks.is_empty(), "loop should have exit blocks");
    }

    #[test]
    fn test_nested_loops() {
        let func = build_nested_loop_func();
        let dt = DominatorTree::build(&func);
        let lf = LoopForest::build(&func, &dt);

        assert_eq!(lf.len(), 2, "should detect two loops (outer + inner)");

        let top = lf.top_level_loops();
        assert_eq!(top.len(), 1, "one top-level loop");

        // Check nesting depths
        for loop_info in lf.all_loops() {
            assert!(loop_info.depth >= 1 && loop_info.depth <= 2);
        }

        // Inner blocks should have depth >= 2
        let inner_hdr = func.blocks[2].id; // inner_hdr
        assert!(
            lf.get_loop_depth(inner_hdr) >= 1,
            "inner header should be in at least one loop"
        );
    }

    #[test]
    fn test_empty_function() {
        let func = Function::new("empty", Signature::void());
        let dt = DominatorTree::build(&func);
        let lf = LoopForest::build(&func, &dt);

        assert!(lf.is_empty());
        assert_eq!(lf.len(), 0);
    }

    #[test]
    fn test_all_loops_returns_all() {
        let func = build_nested_loop_func();
        let dt = DominatorTree::build(&func);
        let lf = LoopForest::build(&func, &dt);

        let all = lf.all_loops();
        assert_eq!(all.len(), 2);
    }

    #[test]
    fn test_is_empty_on_loop_free() {
        let func = build_diamond_func();
        let dt = DominatorTree::build(&func);
        let lf = LoopForest::build(&func, &dt);

        assert!(lf.is_empty());
    }

    #[test]
    fn test_direct_backedge_loop() {
        // A block that branches to itself (simple self-loop)
        let sig = Signature::new(&[], &[Type::I32]);
        let mut b = FunctionBuilder::new("self_loop", sig);
        let entry = b.create_block();
        let loop_blk = b.create_block();
        let exit = b.create_block();

        b.switch_to_block(entry);
        b.jump(loop_blk, &[]);

        b.switch_to_block(loop_blk);
        let c = b.iconst_i32(1);
        b.branch(c, loop_blk, exit, &[], &[]); // back-edge to self

        b.switch_to_block(exit);
        let c2 = b.iconst_i32(42);
        b.return_(&[c2]);

        let func = b.finish();
        let dt = DominatorTree::build(&func);
        let lf = LoopForest::build(&func, &dt);

        assert_eq!(lf.len(), 1, "should detect the self-loop");
        let l = &lf.all_loops()[0];
        assert!(l.blocks.contains(&loop_blk));
        assert!(lf.is_loop_header(loop_blk));
    }
}
