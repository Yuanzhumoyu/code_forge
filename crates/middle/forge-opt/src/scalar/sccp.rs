//! 稀疏条件常量传播 (SCCP) pass.
//!
//! Jointly propagates constant values and execution state (reachability).
//! Can discover constants missed by standard const-fold (e.g. through unreachable branches).

use crate::{ConstValue, OptimizationPass, PassResult};
use forge_ir::IrError;
use forge_ir::*;
use std::collections::{HashMap, HashSet};

#[derive(Default)]
pub struct SccpPass;

impl SccpPass {
    pub fn new() -> Self {
        Self
    }
}

impl OptimizationPass for SccpPass {
    fn name(&self) -> &'static str {
        "sccp"
    }
    fn description(&self) -> &'static str {
        "Sparse Conditional Constant Propagation"
    }
    fn run_on_function(&self, func: &mut Function) -> Result<PassResult, IrError> {
        sccp(func)
    }
}

#[derive(Clone, Debug, Default)]
enum LatticeValue {
    #[default]
    Undefined,
    Constant(ConstValue),
    Top,
}

pub fn sccp(func: &mut Function) -> Result<PassResult, IrError> {
    let mut result = PassResult::default();
    if func.dfg.block_count() == 0 {
        return Ok(result);
    }

    let mut lattice: HashMap<Value, LatticeValue> = HashMap::new();
    let mut reachable: HashSet<Block> = HashSet::new();
    let mut worklist: Vec<Block> = Vec::new();
    // 去重入队标记：同一块可能被多个用户/分支重复加入 worklist，
    // 重复 pop 会整块重扫（克隆 + 全指令 evaluate），是主要开销。
    let mut in_queue: HashSet<Block> = HashSet::new();
    let entry = func.entry();
    if reachable.insert(entry) && in_queue.insert(entry) {
        worklist.push(entry);
    }

    // Init Iconst/Fconst
    for block in func.dfg.block_data_iter() {
        for &inst_id in &block.inst_order {
            let inst = &func.dfg.inst_data(inst_id);
            if let Some(v) = inst.results.first().copied() {
                let ty = func.dfg.value_data(v).ty;
                match &inst.opcode {
                    Opcode::Iconst
                        if let Some(cid) = inst.immediates.first().and_then(|i| i.as_const())
                            && let Some(big) = func.constants.resolve_big(cid) =>
                    {
                        lattice.insert(v, LatticeValue::Constant(ConstValue::Int(big, ty)));
                    }
                    Opcode::Fconst
                        if let Some(cid) = inst.immediates.first().and_then(|i| i.as_const())
                            && let Some(big) = func.constants.resolve_big(cid) =>
                    {
                        lattice.insert(v, LatticeValue::Constant(ConstValue::Float(big, ty)));
                    }
                    _ => {}
                }
            }
        }
    }

    let uses_map = collect_all_uses(func);

    // Worklist propagation
    while let Some(block_id) = worklist.pop() {
        in_queue.remove(&block_id);
        let block = &func.dfg.block(block_id);
        let inst_ids = &block.inst_order;

        for inst_id in inst_ids {
            let inst = &func.dfg.inst_data(*inst_id);
            if let Some(v) = inst.results.first().copied() {
                let old = lattice.get(&v);
                // Skip instructions that have already been constant-folded or
                // that produce values from immediates rather than operands
                // (Iconst, Fconst, StackAddr, GlobalAddr, etc.)
                // Otherwise evaluate_lattice will return Top and overwrite the
                // correct constant lattice value.
                if matches!(old, Some(LatticeValue::Constant(_))) {
                    continue;
                }
                let ty = func.dfg.value_data(v).ty;
                let new =
                    evaluate_lattice(&inst.opcode, &inst.operands, &inst.immediates, ty, &lattice);
                if !lattice_eq(old, &new) {
                    lattice.insert(v, new);
                    if let Some(users) = uses_map.get(&v) {
                        for &user_block in users {
                            if reachable.contains(&user_block) && in_queue.insert(user_block) {
                                worklist.push(user_block);
                            }
                        }
                    }
                }
            }
        }

        // Propagate reachability（按投影读，语义与旧 match 逐条等价）
        macro_rules! push_reachable {
            ($target:expr) => {
                if reachable.insert($target) && in_queue.insert($target) {
                    worklist.push($target);
                }
            };
        }
        if let Some((cond, then_block, _, else_block, _)) = func.dfg.term_branch(block_id) {
            let known = match lattice.get(&cond) {
                Some(LatticeValue::Constant(cv)) => cv.to_bool(),
                _ => None,
            };
            match known {
                Some(true) => push_reachable!(then_block),
                Some(false) => push_reachable!(else_block),
                None => {
                    push_reachable!(then_block);
                    push_reachable!(else_block);
                }
            }
        } else if let Some((target, _)) = func.dfg.term_jump(block_id) {
            push_reachable!(target);
        } else if let Some(view) = func.dfg.term_switch(block_id) {
            push_reachable!(view.default_block);
            for case in &view.cases {
                push_reachable!(case.target);
            }
        }
    }

    // Replace constants and fold branch conditions
    let block_count = func.dfg.block_count();
    for bi in 0..block_count {
        let block_id = Block(bi as u32);
        if !reachable.contains(&block_id) {
            continue;
        }

        let inst_ids = &func.dfg.block(Block(bi as u32)).inst_order;
        // 收集常量改写（避免借用冲突），循环后统一应用并同步 use-lists
        let mut rewrites: Vec<(Inst, Opcode, crate::entity::ConstId)> = Vec::new();
        for inst_id in inst_ids {
            let inst = &func.dfg.inst_data(*inst_id);
            if let Some(v) = inst.results.first().copied()
                && let Some(LatticeValue::Constant(cv)) = lattice.get(&v)
            {
                match cv {
                    ConstValue::Int(big, _) | ConstValue::Float(big, _) => {
                        let cid = func.constants.insert_big(big.clone());
                        let new_op = match cv {
                            ConstValue::Int(..) => Opcode::Iconst,
                            _ => Opcode::Fconst,
                        };
                        rewrites.push((*inst_id, new_op, cid));
                    }
                    _ => {}
                }
            }
        }
        for (inst_id, new_op, cid) in rewrites {
            // 同步 use-lists：清掉旧 operands 的使用记录再清字段
            func.use_lists.remove_inst(&func.dfg, inst_id);
            let inst = func.dfg.inst_mut(inst_id);
            inst.opcode = new_op;
            inst.operands.clear();
            inst.immediates = smallvec::smallvec![Immediate::Const(cid)];
            result.instructions_removed += 1;
            result.changed = true;
        }

        // Fold constant branch conditions（先算目标，再经 Function 的按形式写入口
        // 写入以同步 use-lists）
        let mut folded: Option<Block> = None;
        if let Some((cond, then_block, _, else_block, _)) = func.dfg.term_branch(Block(bi as u32))
            && let Some(LatticeValue::Constant(cv)) = lattice.get(&cond)
        {
            if let Some(true) = cv.to_bool() {
                folded = Some(then_block);
            } else if let Some(false) = cv.to_bool() {
                folded = Some(else_block);
            }
        }
        if let Some(target) = folded {
            func.jump(Block(bi as u32), target, []);
            result.changed = true;
        }
    }

    // Clear unreachable blocks
    for bi in 0..func.dfg.block_count() {
        let block_id = Block(bi as u32);
        if !reachable.contains(&block_id) {
            func.dfg.block_mut(Block(bi as u32)).inst_order.clear();
            func.unreachable(block_id);
            result.blocks_removed += 1;
            result.changed = true;
        }
    }

    Ok(result)
}

fn evaluate_lattice(
    opcode: &Opcode,
    operands: &[Value],
    immediates: &[forge_ir::Immediate],
    ty: TypeId,
    lattice: &HashMap<Value, LatticeValue>,
) -> LatticeValue {
    let const_ops: smallvec::SmallVec<[ConstValue; 4]> = operands
        .iter()
        .filter_map(|v| match lattice.get(v)? {
            LatticeValue::Constant(cv) => Some(cv.clone()),
            _ => None,
        })
        .collect();
    if const_ops.len() != operands.len() {
        return LatticeValue::Top;
    }
    match super::const_fold::fold_opcode(opcode, &const_ops, immediates, ty) {
        Ok(Some(cv)) => LatticeValue::Constant(cv),
        _ => LatticeValue::Top,
    }
}

fn lattice_eq(a: Option<&LatticeValue>, b: &LatticeValue) -> bool {
    match (a, b) {
        (None, _) => false,
        (Some(LatticeValue::Undefined), LatticeValue::Undefined) => true,
        (Some(LatticeValue::Constant(a)), LatticeValue::Constant(b)) => a == b,
        (Some(LatticeValue::Top), LatticeValue::Top) => true,
        _ => false,
    }
}

fn collect_all_uses(func: &Function) -> HashMap<Value, HashSet<Block>> {
    let mut uses: HashMap<Value, HashSet<Block>> = HashMap::new();
    for bi in 0..func.dfg.block_count() {
        let block = &func.dfg.block(Block(bi as u32));
        for &inst_id in &block.inst_order {
            let inst = &func.dfg.inst_data(inst_id);
            for operand in &inst.operands {
                uses.entry(*operand).or_default().insert(Block(bi as u32));
            }
        }
    }
    uses
}

#[cfg(test)]
mod tests {
    use super::*;
    use FunctionBuilder;

    #[test]
    fn sccp_folds_constants() {
        let sig = FunctionSignature::new(&[], &[TypeId::I32]);
        let mut b = FunctionBuilder::new("test", TypeContext::new(), sig);
        let entry = b.create_block();
        b.switch_to_block(entry);
        let c1 = b.iconst_i32(3);
        let c2 = b.iconst_i32(5);
        let sum = b.iadd(c1, c2);
        b.ret(&[sum]);
        let mut func = b.finish().expect("build");
        let pass = SccpPass::new();
        let r = pass.run_on_function(&mut func).unwrap();

        // SCCP should fold 3+5=8
        assert!(r.changed, "SCCP should fold 3+5=8");
        assert!(r.instructions_removed > 0);

        // Verify Iadd was replaced with Iconst
        let has_iadd = func
            .dfg
            .block(Block(0))
            .inst_order
            .iter()
            .any(|&iid| matches!(func.dfg.inst_data(iid).opcode, Opcode::Iadd));
        assert!(!has_iadd, "Iadd should have been replaced with Iconst");
    }

    #[test]
    fn sccp_no_crash_non_const() {
        let sig = FunctionSignature::new(&[(TypeId::I32, "x")], &[TypeId::I32]);
        let mut b = FunctionBuilder::new("test", TypeContext::new(), sig);
        let (entry, params) = b.create_block_with_params(&[(TypeId::I32, "x")]);
        b.switch_to_block(entry);
        let one = b.iconst_i32(1);
        let sum = b.iadd(params[0], one);
        b.ret(&[sum]);
        let mut func = b.finish().expect("build");
        let pass = SccpPass::new();
        let r = pass.run_on_function(&mut func);
        assert!(r.is_ok());
    }
}
