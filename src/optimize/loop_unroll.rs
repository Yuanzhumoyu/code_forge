//! 循环展开 (Loop Unrolling) pass。
//!
//! 对迭代次数已知的简单循环进行展开，减少循环开销，
//! 为后续优化（常量折叠、GVN）创造更多连续基本块。
//!
//! # 条件
//!
//! - 循环有已知的编译时常量迭代次数
//! - 循环体足够小（默认 <= 16 条指令）
//! - 展开因子默认 4，最多不超过迭代次数
//!
//! # 算法
//!
//! 1. 使用 [`LoopForest`] 检测循环（基于支配树的统一循环分析）
//! 2. 分析归纳变量和边界条件
//! 3. 如果迭代次数为常量 N，展开 min(N, factor) 次
//! 4. 克隆循环体 N-1 次，调整 phi/分支

use crate::CompileError;
use crate::ir::*;
use crate::optimize::{OptimizationPass, PassResult};
use std::collections::HashMap;

#[derive(Default)]
pub struct LoopUnrollPass {
    /// 展开因子（默认 4）。
    pub factor: u32,
    /// 最大循环体指令数（超过则不展开）。
    pub max_body_size: usize,
}

impl LoopUnrollPass {
    pub fn new() -> Self {
        Self {
            factor: 4,
            max_body_size: 16,
        }
    }

    pub fn with_factor(mut self, factor: u32) -> Self {
        self.factor = factor;
        self
    }
}

impl OptimizationPass for LoopUnrollPass {
    fn name(&self) -> &'static str {
        "loop-unroll"
    }
    fn description(&self) -> &'static str {
        "Unrolls loops with known iteration counts"
    }

    fn run_on_function(&self, func: &mut Function) -> Result<PassResult, CompileError> {
        unroll_loops(func, self.factor, self.max_body_size)
    }
}

pub fn unroll_loops(
    func: &mut Function,
    factor: u32,
    max_body_size: usize,
) -> Result<PassResult, CompileError> {
    let mut result = PassResult::default();
    if factor < 2 {
        return Ok(result);
    }

    // Build unified loop analysis
    let dt = DominatorTree::build(func);
    let lf = LoopForest::build(func, &dt);

    let preds = func.predecessors();

    for loop_info in lf.all_loops() {
        let header = loop_info.header;

        // Collect body blocks
        let body_blocks: Vec<BlockId> = loop_info.blocks.clone();
        if body_blocks.is_empty() {
            continue;
        }

        // Compute total instruction count in the loop body
        let body_size: usize = body_blocks
            .iter()
            .map(|&b| func.blocks[b.0 as usize].instructions.len())
            .sum();
        if body_size > max_body_size {
            continue;
        }

        // Find the tail(s) of this loop (back-edge sources)
        let tails: Vec<BlockId> = find_backedge_tails(func, header, &dt);
        if tails.is_empty() {
            continue;
        }

        // Analyze trip count
        let trip_count = analyze_trip_count(func, header, &preds);
        let unroll_count = match trip_count {
            Some(n) if n > 1 => std::cmp::min(n as u32, factor),
            _ => continue,
        };
        if unroll_count < 2 {
            continue;
        }

        // Execute unroll: clone the loop body unroll_count-1 times
        // Use the first tail as the primary back-edge target
        let tail = tails[0];
        if unroll_loop_body(func, header, tail, &body_blocks, unroll_count, &preds) {
            result.changed = true;
            result.instructions_removed += 1;
        }
    }

    Ok(result)
}

/// Find blocks that have a back-edge to the given header.
fn find_backedge_tails(func: &Function, header: BlockId, dom_tree: &DominatorTree) -> Vec<BlockId> {
    let mut tails = Vec::new();
    for block in func.iter_blocks() {
        for succ in block_successors(&block.terminator) {
            if succ == header && dom_tree.dominates(header, block.id) {
                tails.push(block.id);
            }
        }
    }
    tails
}

/// 分析循环迭代次数。
/// 查找模式: header 中 phi(i, i+step) 然后在 tail 中 icmp i < N -> branch。
fn analyze_trip_count(func: &Function, header: BlockId, preds: &[Vec<BlockId>]) -> Option<u64> {
    let header_block = &func.blocks[header.0 as usize];

    // Find phi nodes in header (induction variables)
    for inst in &header_block.instructions {
        if !matches!(inst.opcode, Opcode::Phi { .. }) {
            continue;
        }
        let phi_val = inst.result?;

        // Look for icmp comparing this phi in predecessor blocks
        let tail_preds = &preds[header.0 as usize];
        for &pred in tail_preds {
            let pred_block = &func.blocks[pred.0 as usize];
            if let Terminator::Branch {
                cond,
                ..
            } = &pred_block.terminator
            {
                // Check if cond is an icmp comparing phi_val
                for inst in &pred_block.instructions {
                    if inst.result == Some(*cond)
                        && let Opcode::Icmp {
                            cond: IntCC::SignedLessThan,
                        } = &inst.opcode
                        && inst.operands.len() >= 2
                    {
                        let other = if inst.operands[0] == phi_val {
                            inst.operands[1]
                        } else if inst.operands[1] == phi_val {
                            inst.operands[0]
                        } else {
                            continue;
                        };
                        // Look for the constant definition
                        for inst2 in &func.blocks[header.0 as usize].instructions {
                            if inst2.result == Some(other)
                                && let Opcode::Iconst { index } = &inst2.opcode
                            {
                                return func.constant_pool.get(*index).and_then(|b| b.try_to_u64());
                            }
                        }
                    }
                }
            }
        }
    }
    None
}

/// 执行循环展开：克隆循环体 `unroll_count - 1` 次并链接。
fn unroll_loop_body(
    func: &mut Function,
    header: BlockId,
    tail: BlockId,
    body_blocks: &[BlockId],
    unroll_count: u32,
    _preds: &[Vec<BlockId>],
) -> bool {
    let old_block_count = func.blocks.len();

    let mut prev_clone_header: Option<BlockId> = None;
    let mut all_new_blocks: Vec<Block> = Vec::new();
    let mut global_value_map: HashMap<Value, Value> = HashMap::new();

    for clone_idx in 0..(unroll_count - 1) {
        let _ = clone_idx;
        let mut value_map: HashMap<Value, Value> = HashMap::new();
        // Inherit previous round's value mapping (chain clones together)
        for (orig, mapped) in &global_value_map {
            value_map.insert(*orig, *mapped);
        }

        // Clone and save pool constant information
        let mut cloned_data: Vec<(
            Vec<(Value, Type)>,
            Vec<Instruction>,
            Terminator,
            Vec<Option<Big>>,
        )> = Vec::new();
        for &body_id in body_blocks {
            let block = &func.blocks[body_id.0 as usize];
            let bigs: Vec<Option<Big>> = block
                .instructions
                .iter()
                .map(|inst| match &inst.opcode {
                    Opcode::Iconst { index } => func.constant_pool.get(*index).cloned(),
                    Opcode::Fconst { index } => func.constant_pool.get(*index).cloned(),
                    _ => None,
                })
                .collect();
            cloned_data.push((
                block.params.clone(),
                block.instructions.clone(),
                block.terminator.clone(),
                bigs,
            ));
        }

        let mut new_block_ids: Vec<BlockId> = Vec::new();
        for (params, insts, term, bigs) in cloned_data.iter() {
            let new_id = func.create_block_id();
            new_block_ids.push(new_id);
            let mut new_block = Block::new(new_id);

            for (param_val, param_ty) in params {
                let new_val = func.create_value();
                value_map.insert(*param_val, new_val);
                global_value_map.insert(*param_val, new_val);
                new_block.params.push((new_val, *param_ty));
            }

            for (ii, inst) in insts.iter().enumerate() {
                let mut new_inst = inst.clone();
                if let Some(Some(big)) = bigs.get(ii) {
                    let new_idx = func.constant_pool.insert(big.clone());
                    match &new_inst.opcode {
                        Opcode::Iconst { .. } => {
                            new_inst.opcode = Opcode::Iconst { index: new_idx }
                        }
                        Opcode::Fconst { .. } => {
                            new_inst.opcode = Opcode::Fconst { index: new_idx }
                        }
                        _ => {}
                    }
                }
                for op in new_inst.operands.iter_mut() {
                    if let Some(mapped) = value_map.get(op) {
                        *op = *mapped;
                    }
                }
                if let Some(r) = new_inst.result {
                    let new_val = func.create_value();
                    value_map.insert(r, new_val);
                    global_value_map.insert(r, new_val);
                    new_inst.result = Some(new_val);
                }
                new_block.instructions.push(new_inst);
            }

            new_block.terminator = clone_terminator(term, &value_map);
            all_new_blocks.push(new_block);
        }

        // Link: previous clone's tail back-edge points to this clone's header
        if let Some(prev_header) = prev_clone_header {
            let cur_header = new_block_ids[0];
            for block in all_new_blocks.iter_mut().rev() {
                if block.terminator_targets_header(prev_header) {
                    redirect_terminator_target(&mut block.terminator, prev_header, cur_header);
                    break;
                }
            }
        }
        prev_clone_header = Some(new_block_ids[0]);
    }

    // Redirect original tail's back-edge to first clone's header
    if let Some(first_clone_header) = prev_clone_header.and(all_new_blocks.first().map(|b| b.id)) {
        let tail_block = &mut func.blocks[tail.0 as usize];
        redirect_terminator_target(&mut tail_block.terminator, header, first_clone_header);
    }

    // Add all clone blocks to the function
    for block in all_new_blocks {
        func.blocks.push(block);
    }

    func.blocks.len() > old_block_count
}

/// Block helper: check if the terminator targets a specific block.
impl Block {
    fn terminator_targets_header(&self, target: BlockId) -> bool {
        match &self.terminator {
            Terminator::Branch {
                true_block,
                false_block,
                ..
            } => *true_block == target || *false_block == target,
            Terminator::Jump { target: t, .. } => *t == target,
            Terminator::Switch {
                default_block,
                cases,
                ..
            } => *default_block == target || cases.iter().any(|(_, b, _)| *b == target),
            _ => false,
        }
    }
}

/// Redirect a target block reference in a terminator.
fn redirect_terminator_target(term: &mut Terminator, from: BlockId, to: BlockId) {
    match term {
        Terminator::Branch {
            true_block,
            false_block,
            ..
        } => {
            if *true_block == from {
                *true_block = to;
            }
            if *false_block == from {
                *false_block = to;
            }
        }
        Terminator::Jump { target, .. }
            if *target == from => {
                *target = to;
            }
        Terminator::Switch {
            default_block,
            cases,
            ..
        } => {
            if *default_block == from {
                *default_block = to;
            }
            for (_, b, _) in cases.iter_mut() {
                if *b == from {
                    *b = to;
                }
            }
        }
        _ => {}
    }
}

fn clone_terminator(term: &Terminator, value_map: &HashMap<Value, Value>) -> Terminator {
    let map_val = |v: &Value| value_map.get(v).copied().unwrap_or(*v);
    match term {
        Terminator::Branch {
            cond,
            true_block,
            false_block,
            true_args,
            false_args,
        } => Terminator::Branch {
            cond: map_val(cond),
            true_block: *true_block,
            false_block: *false_block,
            true_args: true_args.iter().map(map_val).collect(),
            false_args: false_args.iter().map(map_val).collect(),
        },
        Terminator::Jump { target, args } => Terminator::Jump {
            target: *target,
            args: args.iter().map(map_val).collect(),
        },
        Terminator::Return { values } => Terminator::Return {
            values: values.iter().map(map_val).collect(),
        },
        Terminator::Unreachable => Terminator::Unreachable,
        Terminator::Switch {
            discriminant,
            default_block,
            cases,
        } => Terminator::Switch {
            discriminant: map_val(discriminant),
            default_block: *default_block,
            cases: cases
                .iter()
                .map(|(v, b, args)| (*v, *b, args.iter().map(map_val).collect()))
                .collect(),
        },
    }
}

// ============================================================
// 测试
// ============================================================

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ir::FunctionBuilder;

    #[test]
    fn loop_unroll_no_crash_on_simple_loop() {
        let sig = Signature::new(&[], &[Type::I32]);
        let mut b = FunctionBuilder::new("test", sig);
        let entry = b.create_block();
        let loop_hdr = b.create_block();
        let loop_body = b.create_block();
        let exit = b.create_block();

        b.switch_to_block(entry);
        let _zero = b.iconst_i32(0);
        b.jump(loop_hdr, &[]);

        b.switch_to_block(loop_hdr);
        b.switch_to_block(loop_body);
        let _c = b.iconst_i32(42);
        b.jump(exit, &[]);

        b.switch_to_block(exit);
        let c2 = b.iconst_i32(42);
        b.return_(&[c2]);

        let mut func = b.finish();
        let pass = LoopUnrollPass::new();
        pass.run_on_function(&mut func).unwrap();
        assert!(func.validate().is_valid());
    }

    #[test]
    fn loop_unroll_preserves_validation() {
        let sig = Signature::void();
        let mut b = FunctionBuilder::new("test", sig);
        let entry = b.create_block();
        let loop_blk = b.create_block();
        let exit = b.create_block();

        b.switch_to_block(entry);
        b.jump(loop_blk, &[]);

        b.switch_to_block(loop_blk);
        let cond = b.iconst_i32(1);
        b.branch(cond, loop_blk, exit, &[], &[]); // back-edge to loop_blk

        b.switch_to_block(exit);
        b.return_(&[]);

        let mut func = b.finish();
        let pass = LoopUnrollPass::new();
        pass.run_on_function(&mut func).unwrap();
        assert!(func.validate().is_valid());
    }

    #[test]
    fn loop_unroll_handles_no_loops() {
        let sig = Signature::new(&[], &[Type::I32]);
        let mut b = FunctionBuilder::new("no_loop", sig);
        let entry = b.create_block();
        b.switch_to_block(entry);
        let v = b.iconst_i32(42);
        b.return_(&[v]);
        let mut func = b.finish();

        let pass = LoopUnrollPass::new();
        let r = pass.run_on_function(&mut func).unwrap();
        assert!(!r.changed);
        assert!(func.validate().is_valid());
    }
}
