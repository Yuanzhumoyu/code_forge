//! 死代码消除 pass。
//!
//! 两阶段：
//! 1. 死指令消除 — 移除无使用者且无副作用的指令
//! 2. 死块消除 — 从入口块做 DFS，将不可达块清空

use crate::CompileError;
use crate::ir::*;
use crate::optimize::{OptimizationPass, PassResult};
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

        for block in func.blocks.iter_mut() {
            for inst in block.instructions.iter_mut() {
                // 跳过已经是 Nop 的指令
                if matches!(inst.opcode, Opcode::Nop) {
                    continue;
                }

                // 有副作用的指令不可消除
                if has_side_effects(&inst.opcode) {
                    continue;
                }

                // 检查是否有使用者
                if let Some(result_val) = inst.result {
                    let count = use_counts.get(&result_val).copied().unwrap_or(0);
                    if count == 0 {
                        // 无使用者 → 替换为 Nop
                        inst.opcode = Opcode::Nop;
                        inst.operands.clear();
                        inst.ty = Type::Void;
                        inst.result = None;
                        removed_this_round += 1;
                    }
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

    for block in func.blocks.iter() {
        for inst in &block.instructions {
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
                true_args,
                false_args,
                ..
            } => {
                *counts.entry(*cond).or_insert(0) += 1;
                for v in true_args.iter().chain(false_args.iter()) {
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
    matches!(
        opcode,
        Opcode::Store | Opcode::StackStore { .. } | Opcode::Call { .. } | Opcode::CallIndirect
    )
}

/// 阶段 2: 消除不可达基本块。
///
/// 从入口块做 DFS 标记所有可达块。
/// 不可达块的指令被清空，terminator 设为 Unreachable。
pub(crate) fn eliminate_dead_blocks(func: &mut Function) -> usize {
    if func.blocks.is_empty() {
        return 0;
    }

    // DFS 标记可达块
    let mut reachable = HashSet::new();
    let mut stack = vec![BlockId(0)]; // entry block index

    while let Some(block_id) = stack.pop() {
        if !reachable.insert(block_id) {
            continue;
        }

        if let Some(block) = func.block(block_id) {
            match &block.terminator {
                Terminator::Branch {
                    true_block,
                    false_block,
                    ..
                } => {
                    stack.push(*true_block);
                    stack.push(*false_block);
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
    }

    let mut removed = 0;

    // 清空不可达块
    for i in 0..func.blocks.len() {
        let block_id = BlockId(i as u32);
        if !reachable.contains(&block_id) {
            let block = &mut func.blocks[i];
            if !block.instructions.is_empty()
                || !matches!(block.terminator, Terminator::Unreachable)
            {
                removed += 1;
                block.instructions.clear();
                block.terminator = Terminator::Unreachable;
            }
        }
    }

    removed
}

// ============================================================
// 测试
// ============================================================

#[cfg(test)]
mod tests {
    use super::*;
    use crate::optimize::OptimizationPass;

    #[test]
    fn eliminate_unused_pure_instruction() {
        let sig = Signature::new(&[], &[Type::I32]);
        let mut b = FunctionBuilder::new("test", sig);
        let entry = b.create_block();
        b.switch_to_block(entry);
        let _c = b.iconst_i32(42); // 无使用者 → 死代码
        let ret_val = b.iconst_i32(0); // 被 return 使用 → 保留
        b.return_(&[ret_val]);

        let mut func = b.finish();
        let pass = DeadCodeElimPass::new();
        let r = pass.run_on_function(&mut func).unwrap();

        assert!(r.changed);
        assert!(r.instructions_removed >= 1);
        // c 应该变为 Nop（无使用者）
        let affected_inst = &func.blocks[0].instructions[0];
        assert!(matches!(affected_inst.opcode, Opcode::Nop));
        // ret_val 指令应该保留（被 return 使用）
        let ret_inst = &func.blocks[0].instructions[1];
        assert!(matches!(ret_inst.opcode, Opcode::Iconst { .. }));
    }

    #[test]
    fn preserve_side_effect_instructions() {
        let sig = Signature::new(&[(Type::Ptr, "p")], &[]);
        let mut b = FunctionBuilder::new("test", sig);
        let (entry, params) = b.create_block_with_params(&[(Type::Ptr, "p")]);
        b.switch_to_block(entry);
        let p = params[0];
        let val = b.iconst_i32(99);
        b.store(val, p); // Store 有副作用，不可消除
        b.return_(&[]);

        let mut func = b.finish();
        let pass = DeadCodeElimPass::new();
        let _r = pass.run_on_function(&mut func).unwrap();

        // Store 应该保留（有副作用）
        let store_inst = &func.blocks[0].instructions[1];
        assert!(matches!(store_inst.opcode, Opcode::Store));
        // val (Iconst) is used by Store, so it should also be preserved
    }

    #[test]
    fn eliminate_unreachable_block() {
        // 创建一个有不可达块的函数
        let sig = Signature::new(&[], &[Type::I32]);
        let mut b = FunctionBuilder::new("test", sig);
        let entry = b.create_block();
        let dead_block = b.create_block();
        let merge = b.create_block();

        b.switch_to_block(entry);
        let one = b.iconst_i32(1);
        b.jump(merge, &[one]);

        // dead_block 不可达（entry 直接 jump 到 merge，没有到 dead_block 的路径）
        b.switch_to_block(dead_block);
        let two = b.iconst_i32(2);
        b.jump(merge, &[two]);

        b.switch_to_block(merge);
        b.return_(&[]);

        let mut func = b.finish();
        let pass = DeadCodeElimPass::new();
        let r = pass.run_on_function(&mut func).unwrap();

        assert!(r.blocks_removed >= 1);
        // dead_block 应该被清空
        let dead = &func.blocks[1];
        assert!(dead.instructions.is_empty() || matches!(dead.terminator, Terminator::Unreachable));
    }

    #[test]
    fn eliminate_nop_chain() {
        let sig = Signature::new(&[], &[Type::I32]);
        let mut b = FunctionBuilder::new("test", sig);
        let entry = b.create_block();
        b.switch_to_block(entry);
        let _v1 = b.iconst_i32(10);
        let v2 = b.iconst_i32(20);
        // v1 无使用者 → 死代码
        b.return_(&[v2]);

        let mut func = b.finish();
        let pass = DeadCodeElimPass::new();
        let r = pass.run_on_function(&mut func).unwrap();
        assert!(r.changed);
        assert!(matches!(func.blocks[0].instructions[0].opcode, Opcode::Nop));
    }

    #[test]
    fn cascading_dead_code() {
        // v1 = Iconst(10)  -- dead
        // v2 = Iadd(v1, v1) -- depends on v1  -- dead
        // v3 = Iconst(0)   -- used by return  -- live
        let sig = Signature::new(&[], &[Type::I32]);
        let mut b = FunctionBuilder::new("test", sig);
        let entry = b.create_block();
        b.switch_to_block(entry);
        let _v1 = b.iconst_i32(10);
        let _v2 = b.iadd(_v1, _v1); // 依赖 v1，两者都无使用者
        let v3 = b.iconst_i32(0);
        b.return_(&[v3]);

        let mut func = b.finish();
        let pass = DeadCodeElimPass::new();
        let r = pass.run_on_function(&mut func).unwrap();
        assert!(r.changed);
        // v1 和 v2 都应该是 Nop
        assert!(matches!(func.blocks[0].instructions[0].opcode, Opcode::Nop));
        assert!(matches!(func.blocks[0].instructions[1].opcode, Opcode::Nop));
        // v3 保留
        assert!(matches!(
            func.blocks[0].instructions[2].opcode,
            Opcode::Iconst { .. }
        ));
    }

    #[test]
    fn pass_manager_dce_after_const_fold() {
        // const-folding should create dead code, then DCE should clean it
        let sig = Signature::new(&[(Type::I32, "x")], &[Type::I32]);
        let mut b = FunctionBuilder::new("test", sig);
        let (entry, params) = b.create_block_with_params(&[(Type::I32, "x")]);
        b.switch_to_block(entry);
        let x = params[0];
        let c1 = b.iconst_i32(3);
        let c2 = b.iconst_i32(5);
        let prod = b.imul(c1, c2); // folded → Iconst(15), c1/c2 become dead
        let result = b.iadd(x, prod);
        b.return_(&[result]);

        let mut func = b.finish();

        // Run full default pipeline (const-fold → dce)
        let pm = crate::optimize::PassManager::default();
        let r = pm.run_on_function(&mut func).unwrap();
        assert!(r.changed);
        // prod should be Iconst(15), c1 and c2 should be Nop (DCE'd)
        // x + 15 should remain (x is runtime)
    }
}
