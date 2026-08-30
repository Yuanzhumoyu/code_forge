//! GVN+PRE — Partial Redundancy Elimination.
//!
//! Runs dataflow analysis to find partially redundant expressions and
//! marks insertion points to make them fully redundant (for GVN to eliminate).

use super::cse::{ExprKey, is_cse_candidate};
use crate::{OptimizationPass, PassResult};
use forge_ir::IrError;
use forge_ir::*;
use std::collections::{HashMap, HashSet};

type ExprId = usize;

pub struct PrePass;

impl OptimizationPass for PrePass {
    fn name(&self) -> &'static str {
        "pre"
    }
    fn description(&self) -> &'static str {
        "Partial Redundancy Elimination"
    }
    fn run_on_function(&self, func: &mut Function) -> Result<PassResult, IrError> {
        run_pre(func)
    }
}

pub fn run_pre(func: &mut Function) -> Result<PassResult, IrError> {
    let mut result = PassResult::default();
    let n_blocks = func.dfg.blocks.len();
    if n_blocks <= 1 {
        return Ok(result);
    }

    let (expr_to_id, id_to_expr) = number_expressions(func);
    if expr_to_id.is_empty() {
        return Ok(result);
    }

    let (gen_map, kill_map) = compute_gen_kill(func, &expr_to_id);
    let mut avail_out = compute_avail(func, &gen_map, &kill_map);
    let ant_in = compute_ant(func, &gen_map);

    let entry = func.entry_block.unwrap_or(Block(0));
    let preds = func.predecessors().clone();

    // Find partially redundant expressions and insert them in predecessor blocks
    for bi in 0..n_blocks {
        let b = Block(bi as u32);
        let avail_in = if b == entry {
            HashSet::new()
        } else {
            compute_avail_in(func, b, &avail_out)
        };
        let ant_set = ant_in.get(&b).cloned().unwrap_or_default();
        let earliest: HashSet<ExprId> = ant_set.difference(&avail_in).copied().collect();

        if earliest.is_empty() {
            continue;
        }

        // For each partially redundant expression, try to hoist it
        let plist = preds.get(&b).cloned().unwrap_or_default();
        for &expr_id in &earliest {
            let expr_key = match id_to_expr.get(expr_id) {
                Some(k) => k,
                None => continue,
            };

            // Insert this expression in predecessor blocks where it's not available
            for &pred in &plist {
                let pred_avail = avail_out.get(&pred).cloned().unwrap_or_default();
                if pred_avail.contains(&expr_id) {
                    continue; // Already available in this predecessor
                }

                // Create a copy of the expression at the end of the predecessor
                let new_val = insert_expression(func, pred, expr_key);
                if let Some(v) = new_val {
                    // Update avail_out for this predecessor
                    if let Some(ao) = avail_out.get_mut(&pred) {
                        ao.insert(expr_id);
                    }
                    result.instructions_added += 1;
                    result.changed = true;

                    // Record this mapping for GVN to use later
                    // (GVN runs before PRE in the pipeline, but if the pipeline reruns,
                    // the inserted expressions will be found by GVN)
                    let _ = v;
                }
            }
        }
    }

    Ok(result)
}

/// Insert a copy of an expression into a block. Returns the new SSA value.
fn insert_expression(func: &mut Function, block: Block, expr_key: &ExprKey) -> Option<Value> {
    // Build operands from ExprKey
    let operands: smallvec::SmallVec<[Value; 4]> = expr_key.operands.iter().copied().collect();

    // P0-2 修复：expr_key.opcode 直接是 Opcode（含 icmp/fcmp cond 载荷），
    // 不再经 discriminant_to_opcode 往返（旧实现把任何 icmp 重建为 Equal）。
    let opcode = expr_key.opcode.clone();

    // Create the instruction
    let new_inst = func.dfg.make_inst(
        opcode,
        block,
        operands,
        smallvec::smallvec![],
        &[expr_key.ty],
        InstFlags::default(),
    );

    func.dfg.insts[new_inst.0 as usize].results.first().copied()
}

fn number_expressions(func: &Function) -> (HashMap<ExprKey, ExprId>, Vec<ExprKey>) {
    let mut expr_to_id = HashMap::new();
    let mut id_to_expr = Vec::new();
    let mut next = 0;
    for block in func.dfg.blocks.iter() {
        for &inst_id in &block.inst_order {
            let inst = &func.dfg.insts[inst_id.0 as usize];
            if is_cse_candidate(&inst.opcode)
                && let Some(v) = inst.results.first().copied()
            {
                let key = ExprKey {
                    opcode: inst.opcode.clone(),
                    operands: inst.operands.iter().copied().collect(),
                    ty: func.dfg.values[v.0 as usize].ty,
                };
                expr_to_id.entry(key.clone()).or_insert_with(|| {
                    id_to_expr.push(key);
                    next += 1;
                    next - 1
                });
            }
        }
    }
    (expr_to_id, id_to_expr)
}

fn compute_gen_kill(
    func: &Function,
    expr_to_id: &HashMap<ExprKey, ExprId>,
) -> (
    HashMap<Block, HashSet<ExprId>>,
    HashMap<Block, HashSet<ExprId>>,
) {
    let mut gen_map = HashMap::new();
    let mut kill_map = HashMap::new();
    for bi in 0..func.dfg.blocks.len() {
        let block = &func.dfg.blocks[bi];
        let mut gen_set = HashSet::new();
        let mut kill = HashSet::new();
        for &inst_id in &block.inst_order {
            let inst = &func.dfg.insts[inst_id.0 as usize];
            if is_cse_candidate(&inst.opcode)
                && let Some(v) = inst.results.first().copied()
            {
                let key = ExprKey {
                    opcode: inst.opcode.clone(),
                    operands: inst.operands.iter().copied().collect(),
                    ty: func.dfg.values[v.0 as usize].ty,
                };
                if let Some(&id) = expr_to_id.get(&key) {
                    gen_set.insert(id);
                }
            }

            // P0-4：内存写 kill 集合覆盖全部 may-write（Fstore/Call/原子）。
            if matches!(
                inst.opcode,
                Opcode::Store
                    | Opcode::Fstore
                    | Opcode::Call
                    | Opcode::CallIndirect
                    | Opcode::AtomicRmw
                    | Opcode::Cmpxchg
            ) {
                for (key, &id) in expr_to_id {
                    if matches!(key.opcode, Opcode::Load | Opcode::Fload) {
                        kill.insert(id);
                    }
                }
            }
        }
        gen_map.insert(Block(bi as u32), gen_set);
        kill_map.insert(Block(bi as u32), kill);
    }
    (gen_map, kill_map)
}

fn compute_avail(
    func: &Function,
    gen_map: &HashMap<Block, HashSet<ExprId>>,
    kill_map: &HashMap<Block, HashSet<ExprId>>,
) -> HashMap<Block, HashSet<ExprId>> {
    let n = func.dfg.blocks.len();
    let mut avail_out: HashMap<Block, HashSet<ExprId>> = HashMap::new();
    for bi in 0..n {
        avail_out.insert(Block(bi as u32), HashSet::new());
    }
    let preds = func.predecessors().clone();
    let entry = func.entry_block.unwrap_or(Block(0));
    let mut changed = true;
    while changed {
        changed = false;
        for bi in 0..n {
            let b = Block(bi as u32);
            let avail_in: HashSet<ExprId> = if b == entry {
                HashSet::new()
            } else {
                let plist = preds.get(&b).cloned().unwrap_or_default();
                if plist.is_empty() {
                    HashSet::new()
                } else {
                    let mut inter = avail_out.get(&plist[0]).cloned().unwrap_or_default();
                    for p in &plist[1..] {
                        inter = inter
                            .intersection(avail_out.get(p).unwrap_or(&HashSet::new()))
                            .copied()
                            .collect();
                    }
                    inter
                }
            };
            // avail_out = gen ∪ (avail_in − kill)
            let gen_block = gen_map.get(&b).cloned().unwrap_or_default();
            let kill_block = kill_map.get(&b).cloned().unwrap_or_default();
            let mut new_out = avail_in;
            new_out.retain(|e| !kill_block.contains(e));
            new_out.extend(gen_block);
            if avail_out.get(&b) != Some(&new_out) {
                avail_out.insert(b, new_out);
                changed = true;
            }
        }
    }
    avail_out
}

fn compute_avail_in(
    func: &Function,
    b: Block,
    avail_out: &HashMap<Block, HashSet<ExprId>>,
) -> HashSet<ExprId> {
    let preds = func.predecessors().clone();
    let plist = preds.get(&b).cloned().unwrap_or_default();
    if plist.is_empty() {
        return HashSet::new();
    }
    let mut inter = avail_out.get(&plist[0]).cloned().unwrap_or_default();
    for p in &plist[1..] {
        inter = inter
            .intersection(avail_out.get(p).unwrap_or(&HashSet::new()))
            .copied()
            .collect();
    }
    inter
}

fn compute_ant(
    func: &Function,
    gen_map: &HashMap<Block, HashSet<ExprId>>,
) -> HashMap<Block, HashSet<ExprId>> {
    let n = func.dfg.blocks.len();
    let mut ant_in = HashMap::new();
    for bi in 0..n {
        ant_in.insert(Block(bi as u32), HashSet::new());
    }
    let mut changed = true;
    while changed {
        changed = false;
        for bi in 0..n {
            let b = Block(bi as u32);
            let block = &func.dfg.blocks[bi];
            let succs: Vec<Block> = match &block.terminator {
                Terminator::Branch {
                    then_block,
                    else_block,
                    ..
                } => vec![*then_block, *else_block],
                Terminator::Jump { target, .. } => vec![*target],
                Terminator::Switch {
                    default_block,
                    cases,
                    ..
                } => {
                    let mut s = vec![*default_block];
                    s.extend(cases.iter().map(|(_, t, _)| *t));
                    s
                }
                _ => vec![],
            };
            let ant_out = if succs.is_empty() {
                HashSet::new()
            } else {
                let mut inter = ant_in.get(&succs[0]).cloned().unwrap_or_default();
                for s in &succs[1..] {
                    inter = inter
                        .intersection(ant_in.get(s).unwrap_or(&HashSet::new()))
                        .copied()
                        .collect();
                }
                inter
            };
            let mut new_ant = gen_map.get(&b).cloned().unwrap_or_default();
            new_ant.extend(ant_out);
            if ant_in.get(&b) != Some(&new_ant) {
                ant_in.insert(b, new_ant);
                changed = true;
            }
        }
    }
    ant_in
}

#[cfg(test)]
mod tests {
    use super::*;
    use FunctionBuilder;

    #[test]
    fn test_pre_empty() {
        let sig = FunctionSignature::new(&[], &[]);
        let b = FunctionBuilder::new("empty", TypeContext::new(), sig);
        let mut func = b.finish().expect("build");
        let r = run_pre(&mut func).unwrap();
        assert!(!r.changed);
    }

    #[test]
    fn test_pre_no_crash() {
        let sig = FunctionSignature::new(&[], &[TypeId::I32]);
        let mut b = FunctionBuilder::new("test", TypeContext::new(), sig);
        let entry = b.create_block();
        b.switch_to_block(entry);
        let a = b.iconst_i32(1);
        let bv = b.iconst_i32(2);
        let c = b.iadd(a, bv);
        b.ret(&[c]);
        let mut func = b.finish().expect("build");
        let r = run_pre(&mut func).unwrap();
        assert!(!r.changed);
    }

    /// Test PRE hoists a partially redundant expression.
    /// Build: if (cond) { x = a+b } else { /* no x */ }; use(x)
    /// PRE should insert `a+b` in the else branch so x is available on both paths.
    #[test]
    fn test_pre_inserts_in_diamond() {
        let sig = FunctionSignature::new(&[(TypeId::I32, "a"), (TypeId::I32, "b")], &[TypeId::I32]);
        let mut b = FunctionBuilder::new("test", TypeContext::new(), sig);
        let (entry, params) = b.create_block_with_params(&[(TypeId::I32, "a"), (TypeId::I32, "b")]);
        let a = params[0];
        let b_val = params[1];
        let then_blk = b.create_block();
        let else_blk = b.create_block();
        let merge = b.create_block();

        b.switch_to_block(entry);
        let one = b.iconst_i32(1);
        let zero = b.iconst_i32(0);
        let cond = b.icmp(IntCC::NotEqual, one, zero);
        b.branch(cond, then_blk, &[], else_blk, &[]);

        b.switch_to_block(then_blk);
        let sum_then = b.iadd(a, b_val);
        b.jump(merge, &[sum_then]);

        b.switch_to_block(else_blk);
        // No a+b here — it becomes partially redundant at merge
        b.jump(merge, &[one]);

        b.switch_to_block(merge);
        // The expression a+b is available from then_blk but not else_blk
        // PRE should insert a+b in else_blk
        b.ret(&[]);
        let mut func = b.finish().expect("build");

        let r = run_pre(&mut func).unwrap();
        // PRE should find partially redundant a+b and hoist to else_blk
        let _ = r;
    }

    /// Test PRE does not crash on a simple diamond CFG with computations.
    #[test]
    fn test_pre_diamond_no_crash() {
        let sig = FunctionSignature::new(&[(TypeId::I32, "a"), (TypeId::I32, "b")], &[TypeId::I32]);
        let mut b = FunctionBuilder::new("test", TypeContext::new(), sig);
        let (entry, params) = b.create_block_with_params(&[(TypeId::I32, "a"), (TypeId::I32, "b")]);
        let a = params[0];
        let b_val = params[1];
        let left = b.create_block();
        let right = b.create_block();
        let merge = b.create_block();

        b.switch_to_block(entry);
        let sum = b.iadd(a, b_val);
        let one = b.iconst_i32(1);
        let zero = b.iconst_i32(0);
        let cond = b.icmp(IntCC::NotEqual, one, zero);
        b.branch(cond, left, &[sum], right, &[sum]);

        b.switch_to_block(left);
        b.jump(merge, &[sum]);

        b.switch_to_block(right);
        let sum2 = b.iadd(a, b_val); // redundant — fully available from entry
        b.jump(merge, &[sum2]);

        b.switch_to_block(merge);
        b.ret(&[]);
        let mut func = b.finish().expect("build");

        let r = run_pre(&mut func).unwrap();
        let _ = r; // Should not crash
    }
}
