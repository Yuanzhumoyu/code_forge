//! 死代码消除 pass。
//!
//! 两阶段：
//! 1. 死指令消除 — 移除无使用者且无副作用的指令
//! 2. 死块消除 — 从入口块做 DFS，将不可达块清空

use crate::{OptimizationPass, PassResult};
use forge_ir::CompileError;
use forge_ir::*;
use std::collections::{HashMap, HashSet};

/// 死代码消除 pass。
#[derive(Default)]
pub struct DeadCodeElimPass;

impl DeadCodeElimPass {
    pub fn new() -> Self {
        Self
    }
}

impl OptimizationPass for DeadCodeElimPass {
    fn name(&self) -> &'static str {
        "dead-code-elim"
    }

    fn description(&self) -> &'static str {
        "Dead code elimination: removes unused instructions and unreachable blocks"
    }

    fn run_on_function(&self, func: &mut Function) -> Result<PassResult, CompileError> {
        eliminate_dead_code(func)
    }
}

/// 对单个函数执行死代码消除。
pub fn eliminate_dead_code(func: &mut Function) -> Result<PassResult, CompileError> {
    let mut result = PassResult::default();

    // 阶段 1: 死指令消除
    let inst_count = eliminate_dead_instructions(func);
    result.instructions_removed += inst_count;

    // 阶段 2: 死块消除
    let block_count = eliminate_dead_blocks(func);
    result.blocks_removed += block_count;

    result.changed = inst_count > 0 || block_count > 0;
    Ok(result)
}

/// 阶段 1: 消除无使用者的纯指令。
///
/// 算法：
/// 1. 构建 use-count 映射
/// 2. 标记无使用者的纯指令为 Nop
/// 3. 由于 SSA 连锁效应，迭代直到不动点
fn eliminate_dead_instructions(func: &mut Function) -> usize {
    let mut total_removed = 0;

    loop {
        let mut removed_this_round = 0;

        // 构建 use-count
        let use_counts = build_use_counts(func);

        // Collect inst IDs since we can't borrow func.dfg mutably while iterating
        let mut inst_ids: Vec<Inst> = Vec::new();
        for block in func.dfg.blocks.iter() {
            for &inst_id in &block.inst_order {
                inst_ids.push(inst_id);
            }
        }

        for inst_id in &inst_ids {
            let inst = &mut func.dfg.insts[inst_id.0 as usize];
            // 跳过已经是 Nop 的指令
            if matches!(inst.opcode, Opcode::Nop) {
                continue;
            }

            // 有副作用的指令不可消除
            if has_side_effects(&inst.opcode) {
                continue;
            }

            // 检查是否有使用者
            if let Some(result_val) = inst.results.first().copied() {
                let count = use_counts.get(&result_val).copied().unwrap_or(0);
                if count == 0 {
                    // 无使用者 → 替换为 Nop
                    inst.opcode = Opcode::Nop;
                    inst.operands.clear();
                    // Update result value type to VOID
                    func.dfg.values[result_val.0 as usize].ty = TypeId::VOID;
                    inst.results.clear();
                    removed_this_round += 1;
                }
            }
        }

        total_removed += removed_this_round;
        if removed_this_round == 0 {
            break;
        }
    }

    total_removed
}

/// 构建 Value → use-count 映射。
fn build_use_counts(func: &Function) -> HashMap<Value, usize> {
    let mut counts: HashMap<Value, usize> = HashMap::new();

    for block in func.dfg.blocks.iter() {
        for &inst_id in &block.inst_order {
            let inst = &func.dfg.insts[inst_id.0 as usize];
            if matches!(inst.opcode, Opcode::Nop) {
                continue;
            }
            for operand in &inst.operands {
                *counts.entry(*operand).or_insert(0) += 1;
            }
        }

        // Terminator 使用的值也计入
        match &block.terminator {
            Terminator::Branch {
                cond,
                then_args,
                else_args,
                ..
            } => {
                *counts.entry(*cond).or_insert(0) += 1;
                for v in then_args.iter().chain(else_args.iter()) {
                    *counts.entry(*v).or_insert(0) += 1;
                }
            }
            Terminator::Jump { args, .. } => {
                for v in args {
                    *counts.entry(*v).or_insert(0) += 1;
                }
            }
            Terminator::Return { values } => {
                for v in values {
                    *counts.entry(*v).or_insert(0) += 1;
                }
            }
            Terminator::Unreachable => {}
            Terminator::Switch {
                discriminant,
                cases,
                ..
            } => {
                *counts.entry(*discriminant).or_insert(0) += 1;
                for (_, _, args) in cases.iter() {
                    for v in args {
                        *counts.entry(*v).or_insert(0) += 1;
                    }
                }
            }
        }
    }

    counts
}

/// 判断指令是否有副作用（不可消除）。
fn has_side_effects(opcode: &Opcode) -> bool {
    matches!(opcode, Opcode::Store | Opcode::Call | Opcode::CallIndirect)
}

/// 阶段 2: 消除不可达基本块。
///
/// 从入口块做 DFS 标记所有可达块。
/// 不可达块的指令被清空，terminator 设为 Unreachable。
pub(crate) fn eliminate_dead_blocks(func: &mut Function) -> usize {
    if func.dfg.blocks.is_empty() {
        return 0;
    }

    // DFS 标记可达块 (use entry_block if set, otherwise Block(0))
    let entry = func.entry_block.unwrap_or(Block(0));
    let mut reachable = HashSet::new();
    let mut stack = vec![entry];

    while let Some(block_id) = stack.pop() {
        if !reachable.insert(block_id) {
            continue;
        }

        let block = func.block(block_id);
        match &block.terminator {
            Terminator::Branch {
                then_block,
                else_block,
                ..
            } => {
                stack.push(*then_block);
                stack.push(*else_block);
            }
            Terminator::Jump { target, .. } => {
                stack.push(*target);
            }
            Terminator::Return { .. } | Terminator::Unreachable => {}
            Terminator::Switch {
                default_block,
                cases,
                ..
            } => {
                stack.push(*default_block);
                for (_, target, _) in cases.iter() {
                    stack.push(*target);
                }
            }
        }
    }

    let mut removed = 0;

    // 清空不可达块
    for (i, block) in func.dfg.blocks.iter_mut().enumerate() {
        let block_id = Block(i as u32);
        if !reachable.contains(&block_id)
            && (!block.inst_order.is_empty()
                || !matches!(block.terminator, Terminator::Unreachable))
        {
            removed += 1;
            block.inst_order.clear();
            block.terminator = Terminator::Unreachable;
        }
    }

    removed
}

// ============================================================
// 测试
// ============================================================

// ============================================================

#[cfg(test)]
mod tests {
    use super::*;
    use crate::OptimizationPass;

    #[test]
    fn eliminate_unused_pure_instruction() {
        let sig = FunctionSignature::new(&[], &[TypeId::I32]);
        let mut b = FunctionBuilder::new("test", TypeContext::new(), sig);
        let entry = b.create_block();
        b.switch_to_block(entry);
        let _c = b.iconst_i32(42); // unused -> dead
        let ret_val = b.iconst_i32(0); // used by return -> live
        b.ret(&[ret_val]);

        let mut func = b.finish();
        let pass = DeadCodeElimPass::new();
        let r = pass.run_on_function(&mut func).unwrap();

        assert!(r.changed);
        assert!(r.instructions_removed >= 1);
        // c should become Nop
        let inst_id = func.dfg.blocks[0].inst_order[0];
        let affected_inst = &func.dfg.insts[inst_id.0 as usize];
        assert!(matches!(affected_inst.opcode, Opcode::Nop));
    }

    #[test]
    fn preserve_side_effect_instructions() {
        let sig = FunctionSignature::new(&[(TypeId::PTR, "p")], &[]);
        let mut b = FunctionBuilder::new("test", TypeContext::new(), sig);
        let (entry, params) = b.create_block_with_params(&[(TypeId::PTR, "p")]);
        b.switch_to_block(entry);
        let p = params[0];
        let val = b.iconst_i32(99);
        b.store(val, p); // Store has side effects, cannot eliminate
        b.ret(&[]);

        let mut func = b.finish();
        let pass = DeadCodeElimPass::new();
        let _r = pass.run_on_function(&mut func).unwrap();

        // Store should be preserved (has side effects)
        let store_id = func.dfg.blocks[0].inst_order[1];
        let store_inst = &func.dfg.insts[store_id.0 as usize];
        assert!(matches!(store_inst.opcode, Opcode::Store));
    }

    #[test]
    fn eliminate_unreachable_block() {
        let sig = FunctionSignature::new(&[], &[TypeId::I32]);
        let mut b = FunctionBuilder::new("test", TypeContext::new(), sig);
        let entry = b.create_block();
        let dead_block = b.create_block();
        let merge = b.create_block();

        b.switch_to_block(entry);
        let one = b.iconst_i32(1);
        b.jump(merge, &[one]);

        // dead_block is unreachable (entry jumps directly to merge)
        b.switch_to_block(dead_block);
        let two = b.iconst_i32(2);
        b.jump(merge, &[two]);

        b.switch_to_block(merge);
        b.ret(&[]);

        let mut func = b.finish();
        let pass = DeadCodeElimPass::new();
        let r = pass.run_on_function(&mut func).unwrap();

        assert!(r.blocks_removed >= 1);
        let dead = &func.dfg.blocks[1];
        assert!(dead.inst_order.is_empty() || matches!(dead.terminator, Terminator::Unreachable));
    }
}
