//! 死代码消除 pass。
//!
//! 两阶段：
//! 1. 死指令消除 — 移除无使用者且无副作用的指令
//! 2. 死块消除 — 从入口块做 DFS，将不可达块清空

use crate::{OptimizationPass, PassResult};
use forge_ir::IrError;
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

    fn run_on_function(&self, func: &mut Function) -> Result<PassResult, IrError> {
        eliminate_dead_code(func)
    }
}

/// 对单个函数执行死代码消除。
pub fn eliminate_dead_code(func: &mut Function) -> Result<PassResult, IrError> {
    let mut result = PassResult::default();

    // 阶段 1: 死指令消除
    let inst_count = eliminate_dead_instructions(func);
    result.instructions_removed += inst_count;

    // 阶段 2: 死块消除
    let block_count = eliminate_dead_blocks(func);
    result.blocks_removed += block_count;

    result.changed = inst_count > 0 || block_count > 0;
    if result.changed {
        // 死块已删除：失效分析缓存（前驱/后继/支配树），
        // 避免后续 pass 复用 stale 的 CFG 分析。
        func.analysis_mut().invalidate();
    }
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

        // 构建 use-count（官方 use-lists + 终结符参数补充）
        let use_counts = build_use_counts(func);

        // Collect inst IDs since we can't borrow func.dfg mutably while iterating
        let mut inst_ids: Vec<Inst> = Vec::new();
        for block in func.dfg.blocks.iter() {
            for &inst_id in &block.inst_order {
                inst_ids.push(inst_id);
            }
        }

        // 无使用者的纯指令 → 延迟到循环后统一原子删除（kill_inst 同步 use-lists）
        let mut to_kill: Vec<Inst> = Vec::new();
        for inst_id in &inst_ids {
            let inst = &func.dfg.insts[inst_id.0 as usize];
            // 跳过已经是 Nop 的指令
            if matches!(inst.opcode, Opcode::Nop) {
                continue;
            }

            // 有副作用的指令不可消除
            if has_side_effects(&inst.opcode) {
                continue;
            }

            // P0-3/P0-5：volatile load 不可消除（可观察语义——读可能改变
            // 外部状态，如 MMIO）。
            if matches!(inst.opcode, Opcode::Load | Opcode::Fload)
                && inst.mem_flags.contains(forge_ir::mem_flags::MemFlags::VOLATILE)
            {
                continue;
            }

            // 检查是否有使用者
            if let Some(result_val) = inst.results.first().copied() {
                let count = use_counts.get(&result_val).copied().unwrap_or(0);
                if count == 0 {
                    to_kill.push(*inst_id);
                    removed_this_round += 1;
                }
            }
        }
        for inst in to_kill {
            func.kill_inst(inst);
        }

        total_removed += removed_this_round;
        if removed_this_round == 0 {
            break;
        }
    }

    total_removed
}

/// 构建 Value → use-count 映射。
///
/// 指令操作数部分来自官方 `UseLists`（各 pass 经 kill/RAUW/apply_replacements
/// 维护的新鲜 def-use 链）；终结符参数不在 use-lists 记录范围，需补充计数。
fn build_use_counts(func: &Function) -> HashMap<Value, usize> {
    let mut counts: HashMap<Value, usize> = HashMap::new();

    for (value, _) in func.dfg.values() {
        let n = func.use_lists.use_count(value);
        if n > 0 {
            counts.insert(value, n);
        }
    }

    // Terminator 使用的值也计入
    for (_, block_data) in func.dfg.blocks() {
        for v in block_data.terminator.used_values() {
            *counts.entry(v).or_insert(0) += 1;
        }
    }

    counts
}

/// 判断指令是否有副作用（不可消除）。
///
/// P0-5 修复：覆盖全部 may-write/may-fault 指令——原实现仅
/// Store/Call/CallIndirect，漏了 AtomicRmw/Cmpxchg（有结果）→ 结果未用
/// 即被删除，**原子副作用丢失**；也漏了可 fault 的 load（改变崩溃语义）。
fn has_side_effects(opcode: &Opcode) -> bool {
    matches!(
        opcode,
        Opcode::Store
            | Opcode::Fstore
            | Opcode::Call
            | Opcode::CallIndirect
            | Opcode::AtomicRmw
            | Opcode::Cmpxchg
            // Load 恒视为潜在副作用（可 fault）——保守：只有 volatile 显式
            // 处理（见主循环），普通 load 无 use 时删除安全（值未用、不写
            // 内存），但 NOTRAP 保证外的 load 保守保留。
            | Opcode::Load
            | Opcode::Fload
    )
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
            Terminator::Invoke {
                normal_block,
                unwind_block,
                ..
            } => {
                stack.push(*normal_block);
                stack.push(*unwind_block);
            }
            Terminator::Resume { .. } => {}
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

        let mut func = b.finish().expect("build");
        let pass = DeadCodeElimPass::new();
        let r = pass.run_on_function(&mut func).unwrap();

        assert!(r.changed);
        assert!(r.instructions_removed >= 1);
        // c 应被原子删除（kill_inst 从 inst_order 移除墓碑）：值类型 VOID
        let ValueDef::Inst(c_inst, _) = func.dfg.value_def(_c).unwrap() else {
            panic!("c 应是指令定义")
        };
        assert!(
            matches!(func.dfg.insts[c_inst.0 as usize].opcode, Opcode::Nop),
            "c 指令应已墓碑化为 Nop"
        );
        assert_eq!(func.dfg.value_type(_c), Some(TypeId::VOID), "c 值应为 VOID");
        // ret_val 仍活跃
        assert_eq!(func.dfg.value_type(ret_val), Some(TypeId::I32));
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

        let mut func = b.finish().expect("build");
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

        let mut func = b.finish().expect("build");
        let pass = DeadCodeElimPass::new();
        let r = pass.run_on_function(&mut func).unwrap();

        assert!(r.blocks_removed >= 1);
        let dead = &func.dfg.blocks[1];
        assert!(dead.inst_order.is_empty() || matches!(dead.terminator, Terminator::Unreachable));
    }

    /// P0-5 负向：原子指令（AtomicRmw）结果未用也**不可**删除——
    /// 旧 has_side_effects 仅 Store/Call/CallIndirect，原子副作用丢失。
    #[test]
    fn p0_atomic_not_dead() {
        let sig = FunctionSignature::new(&[(TypeId::PTR, "p")], &[TypeId::I32]);
        let mut b = FunctionBuilder::new("test", TypeContext::new(), sig);
        let (entry, params) = b.create_block_with_params(&[(TypeId::PTR, "p")]);
        b.switch_to_block(entry);
        let p = params[0];
        let one = b.iconst_i32(1);
        // AtomicRmw 结果未用（旧值丢弃）——但原子写副作用必须保留
        let _rmw = b.atomic_rmw(forge_ir::AtomicRmwOp::Add, p, one, forge_ir::Ordering::SequentiallyConsistent);
        b.ret(&[]);

        let mut func = b.finish().expect("build");
        let pass = DeadCodeElimPass::new();
        let r = pass.run_on_function(&mut func).unwrap();

        // 原子指令必须保留（有副作用）——DCE 不应移除任何东西
        assert_eq!(
            r.instructions_removed, 0,
            "原子指令结果未用也不可删除（P0-5 回归：原子副作用丢失）"
        );
        let atomic_present = func
            .dfg
            .insts
            .iter()
            .any(|i| matches!(i.opcode, Opcode::AtomicRmw));
        assert!(atomic_present, "原子指令应保留");
    }
}
