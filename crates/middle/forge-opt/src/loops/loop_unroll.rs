//! 循环展开 (Loop Unrolling) pass。
//!
//! v2 重新设计: v1 通过 Phi 检测 trip count。
//! v2 使用 block-param-based 归纳变量检测来估算 trip count。
//!
//! # 算法 (v2)
//!
//! 1. 检测循环 + 分析 trip count（使用 block-param 归纳变量检测）
//! 2. 若 trip count 已知且 ≤ factor (默认 4)，展开循环体
//! 3. 对每次展开克隆 loop body blocks，重映射 values
//! 4. 重新连接: peeled_body → cloned_body → ... → original_header

use crate::{OptimizationPass, PassResult};
use forge_ir::CompileError;
use forge_ir::*;
use std::collections::{HashMap, HashSet};

pub struct LoopUnrollPass {
    factor: usize,
    max_body_size: usize,
}

impl LoopUnrollPass {
    pub fn new() -> Self {
        Self {
            factor: 4,
            max_body_size: 16,
        }
    }

    pub fn with_factor(mut self, factor: usize) -> Self {
        self.factor = factor;
        self
    }
}

impl Default for LoopUnrollPass {
    fn default() -> Self {
        Self::new()
    }
}

impl OptimizationPass for LoopUnrollPass {
    fn name(&self) -> &'static str {
        "loop-unroll"
    }
    fn description(&self) -> &'static str {
        "Unrolls small counted loops using block-param-based trip count analysis"
    }
    fn run_on_function(&self, func: &mut Function) -> Result<PassResult, CompileError> {
        unroll_loops(func, self.factor, self.max_body_size)
    }
}

/// 对函数中的所有循环执行展开。
pub fn unroll_loops(
    func: &mut Function,
    factor: usize,
    max_body_size: usize,
) -> Result<PassResult, CompileError> {
    let mut result = PassResult::default();
    let dt = DominatorTree::build(func);
    let lf = LoopForest::build(func, &dt);

    for loop_info in lf.all_loops() {
        let body_size: usize = loop_info
            .blocks
            .iter()
            .map(|&b| func.dfg.blocks[b.0 as usize].inst_order.len())
            .sum();

        if body_size > max_body_size {
            continue;
        }

        let trip_count = estimate_trip_count(func, loop_info);
        if trip_count < 2 || trip_count > factor as u64 {
            continue;
        }

        let unroll_count = trip_count.min(factor as u64);
        let r = perform_unroll(func, loop_info, unroll_count);
        if r.changed {
            result.changed = true;
            result.instructions_added += r.instructions_added;
            result.instructions_removed += r.instructions_removed;
        }
    }

    Ok(result)
}

/// 执行循环展开: 对每个展开迭代克隆 body blocks 并重新连接。
fn perform_unroll(
    func: &mut Function,
    loop_info: &forge_ir::LoopInfo,
    unroll_count: u64,
) -> PassResult {
    let mut result = PassResult::default();
    let header = loop_info.header;
    let body_set: HashSet<Block> = loop_info.blocks.iter().copied().collect();
    let preds = func.predecessors().clone();

    // Find latch (back-edge predecessor of header)
    let header_preds = preds.get(&header).cloned().unwrap_or_default();
    let latch = header_preds
        .iter()
        .find(|&&p| body_set.contains(&p))
        .copied();

    // Find the first body block (the loop-body target of header's branch)
    let first_body = match find_first_body_block(func, header, &body_set) {
        Some(b) => b,
        None => return result,
    };

    // Collect body blocks in order (excluding header)
    let body_blocks = collect_body_chain(func, first_body, &body_set, latch);

    for _ in 0..unroll_count - 1 {
        let (block_remap, _val_remap) = clone_body_chain(func, &body_blocks, header, &body_set);

        if block_remap.is_empty() {
            break;
        }

        // Rewire: redirect header's branch to point to the first cloned body block
        let cloned_first = block_remap.get(&first_body).copied();
        if let Some(cf) = cloned_first {
            redirect_branch_target(func, header, first_body, cf);
        }

        result.instructions_added += body_blocks.len();
        result.changed = true;
    }

    result
}

/// Find the first body block: the target of header's branch that is inside the loop.
fn find_first_body_block(
    func: &Function,
    header: Block,
    body_set: &HashSet<Block>,
) -> Option<Block> {
    let term = &func.dfg.blocks[header.0 as usize].terminator;
    match term {
        Terminator::Branch {
            then_block,
            else_block,
            ..
        } => {
            if body_set.contains(then_block) {
                Some(*then_block)
            } else if body_set.contains(else_block) {
                Some(*else_block)
            } else {
                None
            }
        }
        _ => None,
    }
}

/// Collect body blocks in topological order starting from first_body.
fn collect_body_chain(
    func: &Function,
    first_body: Block,
    body_set: &HashSet<Block>,
    _latch: Option<Block>,
) -> Vec<Block> {
    let mut chain = Vec::new();
    let mut visited = HashSet::new();
    let mut current = first_body;
    loop {
        if !body_set.contains(&current) || visited.contains(&current) {
            break;
        }
        visited.insert(current);
        chain.push(current);

        // Follow terminator to next body block
        let term = &func.dfg.blocks[current.0 as usize].terminator;
        match term {
            Terminator::Jump { target, .. } if body_set.contains(target) => {
                current = *target;
            }
            Terminator::Branch {
                then_block,
                else_block,
                ..
            } => {
                // Follow the path that stays in the loop
                if body_set.contains(then_block) && !visited.contains(then_block) {
                    current = *then_block;
                } else if body_set.contains(else_block) && !visited.contains(else_block) {
                    current = *else_block;
                } else {
                    break;
                }
            }
            _ => break,
        }
    }
    chain
}

/// Pre-collected data for cloning an instruction (avoids borrow conflicts).
struct InstTemplate {
    /// Original instruction id — used to look up original results for value remapping.
    inst_id: Inst,
    opcode: Opcode,
    operands: smallvec::SmallVec<[Value; 4]>,
    immediates: smallvec::SmallVec<[Immediate; 4]>,
    result_tys: Vec<TypeId>,
    flags: InstFlags,
}

/// Clone a chain of body blocks. Returns (block_remap, value_remap).
fn clone_body_chain(
    func: &mut Function,
    body_blocks: &[Block],
    _header: Block,
    _body_set: &HashSet<Block>,
) -> (HashMap<Block, Block>, HashMap<Value, Value>) {
    let mut block_remap: HashMap<Block, Block> = HashMap::new();
    let mut val_remap: HashMap<Value, Value> = HashMap::new();

    // Phase 1: collect all data with immutable borrows
    struct BlockTemplate {
        block: Block,
        param_tys: Vec<TypeId>,
        param_values: Vec<Value>,
        insts: Vec<InstTemplate>,
        terminator: Terminator,
    }

    let mut templates: Vec<BlockTemplate> = Vec::new();
    for &block in body_blocks {
        let orig = &func.dfg.blocks[block.0 as usize];
        let param_tys: Vec<TypeId> = orig.params.iter().copied().collect();
        let param_values: Vec<Value> = orig.param_values.iter().copied().collect();

        let orig_insts: Vec<Inst> = orig.inst_order.clone();
        let mut inst_templates = Vec::new();
        for &inst_id in &orig_insts {
            let inst = &func.dfg.insts[inst_id.0 as usize];
            let result_tys: Vec<TypeId> = inst
                .results
                .iter()
                .map(|&v| func.dfg.values[v.0 as usize].ty)
                .collect();
            inst_templates.push(InstTemplate {
                inst_id,
                opcode: inst.opcode,
                operands: inst.operands.clone(),
                immediates: inst.immediates.clone(),
                result_tys,
                flags: inst.flags,
            });
        }

        templates.push(BlockTemplate {
            block,
            param_tys,
            param_values,
            insts: inst_templates,
            terminator: orig.terminator.clone(),
        });
    }

    // Phase 2: create new blocks and instructions with mutable borrows
    for tmpl in &templates {
        let (new_block, new_params) = if tmpl.param_tys.is_empty() {
            let b = func.dfg.make_block();
            (b, Vec::new())
        } else {
            func.dfg.make_block_with_params(&tmpl.param_tys)
        };

        block_remap.insert(tmpl.block, new_block);

        // Map original param values to new param values
        for (i, &orig_pv) in tmpl.param_values.iter().enumerate() {
            if i < new_params.len() {
                val_remap.insert(orig_pv, new_params[i]);
            }
        }
    }

    // Phase 3: clone instructions (all blocks created, val_remap has params)
    for tmpl in &templates {
        let new_block = block_remap[&tmpl.block];

        for inst_tmpl in &tmpl.insts {
            // Remap operands
            let new_operands: smallvec::SmallVec<[Value; 4]> = inst_tmpl
                .operands
                .iter()
                .map(|v| val_remap.get(v).copied().unwrap_or(*v))
                .collect();

            // Remap immediates: FuncRef targets stay the same, Consts stay the same
            let new_immediates: smallvec::SmallVec<[Immediate; 4]> = inst_tmpl.immediates.clone();

            let new_inst = func.dfg.make_inst(
                inst_tmpl.opcode,
                new_block,
                new_operands,
                new_immediates,
                &inst_tmpl.result_tys,
                inst_tmpl.flags,
            );

            // Map old results to new results using stored inst_id
            let orig_inst = &func.dfg.insts[inst_tmpl.inst_id.0 as usize];
            let new_results = &func.dfg.insts[new_inst.0 as usize].results;
            for (i, &old_r) in orig_inst.results.iter().enumerate() {
                if i < new_results.len() {
                    val_remap.insert(old_r, new_results[i]);
                }
            }
        }

        // Clone terminator
        let new_term = clone_terminator(&tmpl.terminator, &val_remap, &block_remap);
        func.dfg.set_terminator(new_block, new_term);
    }

    (block_remap, val_remap)
}

/// Clone a terminator with value/block remapping.
fn clone_terminator(
    term: &Terminator,
    val_remap: &HashMap<Value, Value>,
    block_remap: &HashMap<Block, Block>,
) -> Terminator {
    match term {
        Terminator::Jump { target, args } => {
            let new_target = block_remap.get(target).copied().unwrap_or(*target);
            let new_args: smallvec::SmallVec<[Value; 2]> = args
                .iter()
                .map(|v| val_remap.get(v).copied().unwrap_or(*v))
                .collect();
            Terminator::Jump {
                target: new_target,
                args: new_args,
            }
        }
        Terminator::Branch {
            cond,
            then_block,
            then_args,
            else_block,
            else_args,
        } => {
            let new_cond = val_remap.get(cond).copied().unwrap_or(*cond);
            let new_then = block_remap.get(then_block).copied().unwrap_or(*then_block);
            let new_else = block_remap.get(else_block).copied().unwrap_or(*else_block);
            let new_then_args: smallvec::SmallVec<[Value; 2]> = then_args
                .iter()
                .map(|v| val_remap.get(v).copied().unwrap_or(*v))
                .collect();
            let new_else_args: smallvec::SmallVec<[Value; 2]> = else_args
                .iter()
                .map(|v| val_remap.get(v).copied().unwrap_or(*v))
                .collect();
            Terminator::Branch {
                cond: new_cond,
                then_block: new_then,
                then_args: new_then_args,
                else_block: new_else,
                else_args: new_else_args,
            }
        }
        Terminator::Return { values } => {
            let new_vals: smallvec::SmallVec<[Value; 2]> = values
                .iter()
                .map(|v| val_remap.get(v).copied().unwrap_or(*v))
                .collect();
            Terminator::Return { values: new_vals }
        }
        _ => term.clone(),
    }
}

/// Redirect the branch target in block's terminator from old_target to new_target.
fn redirect_branch_target(func: &mut Function, block: Block, old_target: Block, new_target: Block) {
    let bd = &mut func.dfg.blocks[block.0 as usize];
    let new_term = match &bd.terminator {
        Terminator::Branch {
            cond,
            then_block,
            then_args,
            else_block,
            else_args,
        } => {
            let new_then = if *then_block == old_target {
                new_target
            } else {
                *then_block
            };
            let new_else = if *else_block == old_target {
                new_target
            } else {
                *else_block
            };
            Terminator::Branch {
                cond: *cond,
                then_block: new_then,
                then_args: then_args.clone(),
                else_block: new_else,
                else_args: else_args.clone(),
            }
        }
        _ => return,
    };
    func.dfg.set_terminator(block, new_term);
}

/// 估算循环的 trip count。
///
/// 检查 header block 的 icmp 比较和 block params 来估算迭代次数。
/// 支持常见的 `for i in 0..N` 模式 (start=0, step=1, icmp SLT/ULT with bound=N)。
/// 返回估算的 trip count，若不确定则返回 0。
fn estimate_trip_count(func: &Function, loop_info: &forge_ir::LoopInfo) -> u64 {
    let header = loop_info.header;
    let header_block = &func.dfg.blocks[header.0 as usize];
    let preds = func.predecessors();

    // Need at least 2 predecessors: one init (outside loop), one back edge
    let pred_list = match preds.get(&header) {
        Some(p) => p,
        None => return 0,
    };
    if pred_list.len() < 2 {
        return 0;
    }

    let body_set: std::collections::HashSet<Block> = loop_info.blocks.iter().copied().collect();

    // Find initial values from outside-loop predecessor
    let init_pred = pred_list.iter().find(|&&p| !body_set.contains(&p)).copied();

    // Check if there's an icmp in the header comparing the IV to a bound
    let mut iv_param_idx: Option<usize> = None;
    let mut init_value: Option<i64> = None;
    let mut bound_value: Option<i64> = None;
    let mut cmp_cc: Option<IntCC> = None;

    for (pi, &_param_val) in header_block.param_values.iter().enumerate() {
        let init_arg = init_pred.and_then(|p| {
            let term = &func.dfg.blocks[p.0 as usize].terminator;
            match term {
                Terminator::Jump { target, args } if *target == header => args.get(pi).copied(),
                Terminator::Branch {
                    then_block,
                    then_args,
                    else_block,
                    else_args,
                    ..
                } => {
                    if *then_block == header {
                        then_args.get(pi).copied()
                    } else if *else_block == header {
                        else_args.get(pi).copied()
                    } else {
                        None
                    }
                }
                _ => None,
            }
        });

        // Check if init arg is a constant
        if let Some(init_v) = init_arg {
            let init_def = func.dfg.value_def(init_v).copied();
            if let Some(ValueDef::Inst(inst, _)) = init_def {
                let inst_data = &func.dfg.insts[inst.0 as usize];
                if inst_data.opcode == Opcode::Iconst
                    && let Some(cid) = inst_data.immediates.iter().find_map(|i| i.as_const())
                    && let Some(big) = func.constants.resolve_int(cid)
                {
                    init_value = Some(big);
                    iv_param_idx = Some(pi);
                }
            }
        }
    }

    // Find icmp in header comparing IV to bound
    if let Some(iv_idx) = iv_param_idx {
        let iv_param_val = header_block.param_values[iv_idx];
        for &inst_id in &header_block.inst_order {
            let inst = &func.dfg.insts[inst_id.0 as usize];
            if let Opcode::Icmp { cond } = &inst.opcode {
                let operands = &inst.operands;
                // Check if one operand is the IV
                let other = if operands.first().copied() == Some(iv_param_val) {
                    cmp_cc = Some(*cond);
                    operands.get(1).copied()
                } else if operands.get(1).copied() == Some(iv_param_val) {
                    cmp_cc = Some(*cond);
                    operands.first().copied()
                } else {
                    continue;
                };

                if let Some(other_v) = other {
                    let other_def = func.dfg.value_def(other_v).copied();
                    if let Some(ValueDef::Inst(other_inst, _)) = other_def {
                        let other_data = &func.dfg.insts[other_inst.0 as usize];
                        if other_data.opcode == Opcode::Iconst
                            && let Some(cid) =
                                other_data.immediates.iter().find_map(|i| i.as_const())
                        {
                            bound_value = func.constants.resolve_int(cid);
                        }
                    }
                }
            }
        }
    }

    // Compute trip count from start, bound, and comparison condition.
    // For the common case (start=0, step=1):
    //   icmp SLT/ULT iv, N  → trip_count = N
    //   icmp SLE/ULE iv, N  → trip_count = N + 1
    // Negative bound with start=0 → loop never executes (trip_count = 0)
    if let (Some(bound), Some(init_val)) = (bound_value, init_value) {
        // Bound must be reachable from init_val with step=1
        if bound <= init_val {
            return 0; // Loop would never execute or count down (unsupported)
        }
        let base_trip = (bound - init_val) as u64;
        if base_trip > 100 {
            return 0;
        }
        // Check if comparison is "less than or equal" → +1 iteration
        if let Some(cc) = &cmp_cc {
            match cc {
                IntCC::SignedLessThanOrEqual | IntCC::UnsignedLessThanOrEqual => {
                    return base_trip + 1;
                }
                IntCC::SignedLessThan | IntCC::UnsignedLessThan => {
                    return base_trip;
                }
                _ => return 0, // Unsupported comparison for this heuristic
            }
        }
        // No IntCC found — return base trip count as fallback
        return base_trip;
    }

    0
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn loop_unroll_no_crash() {
        let sig = FunctionSignature::new(&[], &[TypeId::I32]);
        let mut b = FunctionBuilder::new("test", TypeContext::new(), sig);
        let entry = b.create_block();
        b.switch_to_block(entry);
        let v = b.iconst_i32(42);
        b.ret(&[v]);
        let mut func = b.finish();

        let pass = LoopUnrollPass::new();
        let r = pass.run_on_function(&mut func).unwrap();
        assert!(!r.changed);
    }

    /// Test that clone_body_chain correctly remaps values when a block has
    /// two instructions of the same opcode. This verifies the fix for the
    /// (opcode, result_count) matching bug.
    #[test]
    fn clone_body_chain_same_opcode_remap() {
        let sig = FunctionSignature::new(&[], &[]);
        let mut b = FunctionBuilder::new("test", TypeContext::new(), sig);
        let entry = b.create_block();
        b.switch_to_block(entry);
        let v1 = b.iconst_i32(1);
        let v2 = b.iconst_i32(2);
        // Two iadd instructions in the same block — same opcode, same result count
        let v3 = b.iadd(v1, v2); // 1 + 2 = 3
        let v4 = b.iadd(v3, v1); // 3 + 1 = 4
        b.ret(&[]);
        let mut func = b.finish();

        // Clone the entry block's body
        let (block_remap, val_remap) =
            clone_body_chain(&mut func, &[entry], entry, &HashSet::new());

        assert!(!block_remap.is_empty(), "Should create cloned block");
        // Both v3 and v4 should be remapped to distinct new values
        assert!(val_remap.contains_key(&v3), "v3 should be remapped");
        assert!(val_remap.contains_key(&v4), "v4 should be remapped");
        assert_ne!(
            val_remap.get(&v3),
            val_remap.get(&v4),
            "Different results should map to different values"
        );
    }

    /// Test estimate_trip_count with a positive bound (start=0, bound=4).
    #[test]
    fn trip_count_positive_bound() {
        // Build: for i in 0..4: body
        let sig = FunctionSignature::new(&[], &[TypeId::I32]);
        let mut b = FunctionBuilder::new("test", TypeContext::new(), sig);
        let entry = b.create_block();
        b.switch_to_block(entry);
        let zero = b.iconst_i32(0);
        let (header, header_params) = b.create_block_with_params(&[(TypeId::I32, "i")]);
        b.switch_to_block(entry);
        b.jump(header, &[zero]);

        b.switch_to_block(header);
        let four = b.iconst_i32(4);
        let cond = b.icmp(IntCC::SignedLessThan, header_params[0], four);
        let body_block = b.create_block();
        let exit_block = b.create_block();
        b.switch_to_block(header);
        b.branch(cond, body_block, &[], exit_block, &[]);

        b.switch_to_block(body_block);
        let one = b.iconst_i32(1);
        let next_i = b.iadd(header_params[0], one);
        b.jump(header, &[next_i]);

        b.switch_to_block(exit_block);
        b.ret(&[]);
        let func = b.finish();

        let dt = DominatorTree::build(&func);
        let lf = LoopForest::build(&func, &dt);
        let loops: Vec<_> = lf.all_loops().iter().collect();
        assert_eq!(loops.len(), 1, "Should find one loop");

        let tc = estimate_trip_count(&func, loops[0]);
        assert_eq!(tc, 4, "SLT with bound=4, start=0 → trip_count=4");
    }

    /// Test estimate_trip_count returns 0 for a negative bound (loop never executes).
    #[test]
    fn trip_count_negative_bound() {
        let sig = FunctionSignature::new(&[], &[TypeId::I32]);
        let mut b = FunctionBuilder::new("test", TypeContext::new(), sig);
        let entry = b.create_block();
        b.switch_to_block(entry);
        let zero = b.iconst_i32(0);
        let (header, header_params) = b.create_block_with_params(&[(TypeId::I32, "i")]);
        b.switch_to_block(entry);
        b.jump(header, &[zero]);

        b.switch_to_block(header);
        let neg_five = b.iconst_i32(-5);
        let cond = b.icmp(IntCC::SignedLessThan, header_params[0], neg_five);
        let body_block = b.create_block();
        let exit_block = b.create_block();
        b.switch_to_block(header);
        b.branch(cond, body_block, &[], exit_block, &[]);

        b.switch_to_block(body_block);
        let one = b.iconst_i32(1);
        let next_i = b.iadd(header_params[0], one);
        b.jump(header, &[next_i]);

        b.switch_to_block(exit_block);
        b.ret(&[]);
        let func = b.finish();

        let dt = DominatorTree::build(&func);
        let lf = LoopForest::build(&func, &dt);
        let loops: Vec<_> = lf.all_loops().iter().collect();
        assert_eq!(loops.len(), 1, "Should find one loop");

        let tc = estimate_trip_count(&func, loops[0]);
        // start=0, bound=-5, step=1: loop never enters (0 is never < -5)
        assert_eq!(tc, 0, "Negative bound with start=0 → trip_count=0");
    }
}
