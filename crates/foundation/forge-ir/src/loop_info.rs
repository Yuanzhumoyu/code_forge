//! 循环森林 — 自然循环检测与分析。

use crate::analysis::{DominatorTree, block_predecessors, block_successors_in_func};
use crate::entity::*;
use crate::function::Function;
use std::collections::{HashMap, HashSet};

#[derive(Clone, Debug)]
pub struct LoopInfo {
    pub header: Block,
    pub blocks: Vec<Block>,
    pub depth: u32,
    pub parent_loop: Option<usize>,
    pub exit_blocks: Vec<Block>,
}

#[derive(Clone, Debug)]
pub struct LoopForest {
    loops: Vec<LoopInfo>,
    header_to_loop: HashMap<Block, usize>,
    depths: HashMap<Block, u32>,
}

impl LoopForest {
    pub fn build(func: &Function, dom_tree: &DominatorTree) -> Self {
        let mut loops: Vec<LoopInfo> = Vec::new();
        let mut header_to_loop: HashMap<Block, usize> = HashMap::new();

        // Detect back-edges
        for (pred, bd) in func.dfg.blocks() {
            for succ in bd.terminator.successors() {
                if dom_tree.dominates(succ, pred) {
                    let header = succ;
                    if let Some(&idx) = header_to_loop.get(&header) {
                        collect_loop_body(func, pred, header, &mut loops[idx].blocks);
                    } else {
                        let mut blocks = vec![header];
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

        // Compute exit blocks
        for l in &mut loops {
            let block_set: HashSet<Block> = l.blocks.iter().copied().collect();
            for &b in &l.blocks {
                for succ in block_successors_in_func(func, b) {
                    if !block_set.contains(&succ) && !l.exit_blocks.contains(&succ) {
                        l.exit_blocks.push(succ);
                    }
                }
            }
        }

        // Nesting
        let loop_sets: Vec<HashSet<Block>> = loops
            .iter()
            .map(|l| l.blocks.iter().copied().collect())
            .collect();
        for i in 0..loops.len() {
            for j in 0..loops.len() {
                if i == j {
                    continue;
                }
                if loop_sets[i].is_superset(&loop_sets[j])
                    && loop_sets[i].len() > loop_sets[j].len()
                {
                    let tighter = match loops[j].parent_loop {
                        None => true,
                        Some(ex) => loop_sets[i].len() < loop_sets[ex].len(),
                    };
                    if tighter {
                        loops[j].parent_loop = Some(i);
                    }
                }
            }
        }

        let depths: Vec<u32> = (0..loops.len())
            .map(|idx| compute_depth(&loops, idx))
            .collect();
        for (idx, l) in loops.iter_mut().enumerate() {
            l.depth = depths[idx];
        }

        let mut depth_map: HashMap<Block, u32> = HashMap::new();
        for l in &loops {
            for &b in &l.blocks {
                let e = depth_map.entry(b).or_insert(0);
                *e = (*e).max(l.depth);
            }
        }
        for (block, _) in func.dfg.blocks() {
            depth_map.entry(block).or_insert(0);
        }

        Self {
            loops,
            header_to_loop,
            depths: depth_map,
        }
    }

    pub fn is_loop_header(&self, block: Block) -> bool {
        self.header_to_loop.contains_key(&block)
    }
    pub fn get_loop_depth(&self, block: Block) -> u32 {
        self.depths.get(&block).copied().unwrap_or(0)
    }
    pub fn top_level_loops(&self) -> Vec<&LoopInfo> {
        self.loops
            .iter()
            .filter(|l| l.parent_loop.is_none())
            .collect()
    }
    pub fn all_loops(&self) -> &[LoopInfo] {
        &self.loops
    }
    pub fn len(&self) -> usize {
        self.loops.len()
    }
    pub fn is_empty(&self) -> bool {
        self.loops.is_empty()
    }
}

fn collect_loop_body(func: &Function, start: Block, header: Block, body: &mut Vec<Block>) {
    let mut body_set: HashSet<Block> = body.iter().copied().collect();
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

fn compute_depth(loops: &[LoopInfo], idx: usize) -> u32 {
    loops[idx]
        .parent_loop
        .map_or(1, |p| compute_depth(loops, p) + 1)
}

// ============================================================
// 测试
// ============================================================

#[cfg(test)]
mod tests {
    use super::*;
    use crate::analysis::DominatorTree;
    use crate::builder::FunctionBuilder;
    use crate::types::{FunctionSignature, TypeContext};

    /// Build a function with a single loop:
    ///   entry → header → body → (back-edge to header or exit)
    fn build_single_loop() -> (Function, LoopForest) {
        let ctx = TypeContext::new();
        let sig = FunctionSignature::new(&[], &[ctx.i32_ty()]);
        let mut fb = FunctionBuilder::new("test", ctx.clone(), sig);
        let entry = fb.create_block();
        let header = fb.create_block();
        let body = fb.create_block();
        let exit_blk = fb.create_block();

        fb.switch_to_block(entry);
        fb.jump(header, &[]);

        fb.switch_to_block(header);
        let cond = fb.iconst_i32(1);
        fb.branch(cond, body, &[], exit_blk, &[]);

        fb.switch_to_block(body);
        fb.jump(header, &[]); // back-edge: body → header

        fb.switch_to_block(exit_blk);
        let r = fb.iconst_i32(0);
        fb.ret(&[r]);

        let func = fb.finish();
        let dt = DominatorTree::build(&func);
        let lf = LoopForest::build(&func, &dt);
        (func, lf)
    }

    /// Build a function with nested loops:
    ///   entry → outer_hdr → inner_hdr → inner_body → (back to inner_hdr or fall to outer_body → back to outer_hdr or exit)
    fn build_nested_loops() -> (Function, LoopForest) {
        let ctx = TypeContext::new();
        let sig = FunctionSignature::new(&[], &[ctx.i32_ty()]);
        let mut fb = FunctionBuilder::new("test", ctx.clone(), sig);
        let entry = fb.create_block();
        let outer_header = fb.create_block();
        let inner_header = fb.create_block();
        let inner_body = fb.create_block();
        let outer_body = fb.create_block();
        let exit_blk = fb.create_block();

        fb.switch_to_block(entry);
        fb.jump(outer_header, &[]);

        fb.switch_to_block(outer_header);
        let c1 = fb.iconst_i32(1);
        fb.branch(c1, inner_header, &[], exit_blk, &[]);

        fb.switch_to_block(inner_header);
        let c2 = fb.iconst_i32(1);
        fb.branch(c2, inner_body, &[], outer_body, &[]);

        fb.switch_to_block(inner_body);
        fb.jump(inner_header, &[]); // inner back-edge

        fb.switch_to_block(outer_body);
        fb.jump(outer_header, &[]); // outer back-edge

        fb.switch_to_block(exit_blk);
        let r = fb.iconst_i32(0);
        fb.ret(&[r]);

        let func = fb.finish();
        let dt = DominatorTree::build(&func);
        let lf = LoopForest::build(&func, &dt);
        (func, lf)
    }

    #[test]
    fn no_loops_in_linear_function() {
        let ctx = TypeContext::new();
        let sig = FunctionSignature::new(&[], &[ctx.i32_ty()]);
        let mut fb = FunctionBuilder::new("test", ctx.clone(), sig);
        let entry = fb.create_block();
        fb.switch_to_block(entry);
        let v = fb.iconst_i32(42);
        fb.ret(&[v]);
        let func = fb.finish();
        let dt = DominatorTree::build(&func);
        let lf = LoopForest::build(&func, &dt);
        assert!(lf.is_empty());
        assert_eq!(lf.len(), 0);
    }

    #[test]
    fn single_loop_detected() {
        let (_func, lf) = build_single_loop();
        assert!(!lf.is_empty(), "expected at least 1 loop, got {}", lf.len());
    }

    #[test]
    fn single_loop_header_identified() {
        let (_func, lf) = build_single_loop();
        let top = lf.top_level_loops();
        assert_eq!(top.len(), 1, "expected 1 top-level loop");
        let the_loop = top[0];
        // Header should be recognized as loop header
        assert!(lf.is_loop_header(the_loop.header));
    }

    #[test]
    fn single_loop_depth() {
        let (_func, lf) = build_single_loop();
        let top = lf.top_level_loops();
        let the_loop = top[0];
        assert_eq!(the_loop.depth, 1);
        assert!(lf.get_loop_depth(the_loop.header) > 0);
    }

    #[test]
    fn single_loop_has_exit() {
        let (_func, lf) = build_single_loop();
        let top = lf.top_level_loops();
        let the_loop = top[0];
        assert!(
            !the_loop.exit_blocks.is_empty(),
            "loop should have at least 1 exit block"
        );
    }

    #[test]
    fn nested_loops_detected() {
        let (_func, lf) = build_nested_loops();
        // Should have at least 2 loops (inner + outer)
        assert!(lf.len() >= 2, "expected >= 2 loops, got {}", lf.len());
    }

    #[test]
    fn nested_loops_have_parent_child() {
        let (_func, lf) = build_nested_loops();
        let all = lf.all_loops();
        // Find inner loop (the one with parent_loop set)
        let inner = all
            .iter()
            .find(|l| l.parent_loop.is_some())
            .expect("should have an inner loop with parent");
        let outer = &all[inner.parent_loop.unwrap()];
        // Inner loop should be nested inside outer
        assert!(
            outer.blocks.len() >= inner.blocks.len(),
            "outer loop should contain inner loop's blocks"
        );
    }

    #[test]
    fn nested_loops_depth() {
        let (_func, lf) = build_nested_loops();
        let all = lf.all_loops();
        let inner = all
            .iter()
            .find(|l| l.parent_loop.is_some())
            .expect("should have an inner loop with parent");
        let outer = &all[inner.parent_loop.unwrap()];
        // Inner should be deeper than outer
        assert!(inner.depth > outer.depth, "inner depth should exceed outer");
    }

    #[test]
    fn diamond_cfg_no_loop() {
        let ctx = TypeContext::new();
        let sig = FunctionSignature::new(&[], &[ctx.i32_ty()]);
        let mut fb = FunctionBuilder::new("test", ctx.clone(), sig);
        let (entry, _) = fb.create_entry_block();
        let then_blk = fb.create_block();
        let else_blk = fb.create_block();
        let (merge_blk, merge_params) = fb.create_block_with_params(&[(TypeId::I32, "r")]);
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
            fb.ret(&[merge_params[0]]);
        }
        let func = fb.finish();
        let dt = DominatorTree::build(&func);
        let lf = LoopForest::build(&func, &dt);
        assert!(lf.is_empty(), "diamond CFG should have no loops");
    }
}
