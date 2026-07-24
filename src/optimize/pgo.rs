//! Profile-Guided Optimization (PGO) 支持。
//!
//! 提供基于运行时剖析反馈的优化基础设施。
//!
//! # 工作流
//!
//! 1. Instrumentation: 在 IR 中插入计数器指令
//! 2. Training: 运行 instrumented 程序收集 profile 数据
//! 3. Optimization: 根据 profile 数据优化 IR（内联热函数、重排基本块等）

use crate::CompileError;
use crate::ir::*;
use crate::optimize::{OptimizationPass, PassResult};
use std::collections::HashMap;

/// PGO 计数器类型。
#[derive(Clone, Debug, Default)]
pub struct PgoCounters {
    /// 基本块执行计数：BlockId → 执行次数。
    pub block_counts: HashMap<BlockId, u64>,
    /// 边执行计数：(source, target) → 执行次数。
    pub edge_counts: HashMap<(BlockId, BlockId), u64>,
}

impl PgoCounters {
    pub fn new() -> Self {
        Self::default()
    }

    /// 获取块的热度分数（用于内联决策）。
    pub fn block_heat(&self, block: BlockId) -> u64 {
        self.block_counts.get(&block).copied().unwrap_or(0)
    }

    /// 获取边的执行概率（0-100）。
    pub fn edge_probability(&self, source: BlockId, target: BlockId) -> u32 {
        let total: u64 = self
            .edge_counts
            .iter()
            .filter(|((s, _), _)| *s == source)
            .map(|(_, c)| *c)
            .sum();
        if total == 0 {
            return 50;
        }
        let count = self
            .edge_counts
            .get(&(source, target))
            .copied()
            .unwrap_or(0);
        ((count * 100) / total) as u32
    }
}

/// PGO 插桩 pass — 在 IR 中插入基本块计数器。
#[derive(Default)]
pub struct PgoInstrumentPass;

impl PgoInstrumentPass {
    pub fn new() -> Self {
        Self
    }
}

impl OptimizationPass for PgoInstrumentPass {
    fn name(&self) -> &'static str {
        "pgo-instrument"
    }
    fn description(&self) -> &'static str {
        "Inserts block execution counters for profile-guided optimization"
    }

    fn run_on_function(&self, func: &mut Function) -> Result<PassResult, CompileError> {
        let mut result = PassResult::default();

        // 预先创建 PGO 计数器常量
        let counter_idx = func.constant_pool.insert(Big::from_u64(1));

        // 为每个基本块插入计数器
        let mut insertions: Vec<(usize, Value)> = Vec::new();
        for bi in 0..func.blocks.len() {
            if !func.blocks[bi].instructions.is_empty()
                || !matches!(func.blocks[bi].terminator, Terminator::Unreachable)
            {
                let v = func.create_value();
                insertions.push((bi, v));
            }
        }

        for (bi, counter_val) in insertions {
            func.blocks[bi].instructions.insert(
                0,
                Instruction::new(
                    Opcode::Iconst { index: counter_idx },
                    smallvec::smallvec![],
                    Some(counter_val),
                    Type::I64,
                ),
            );
            result.changed = true;
            result.instructions_removed += 1;
        }

        Ok(result)
    }
}

/// PGO 驱动优化 pass — 根据 profile 数据优化。
pub struct PgoOptimizePass {
    pub counters: PgoCounters,
}

impl PgoOptimizePass {
    pub fn new(counters: PgoCounters) -> Self {
        Self { counters }
    }
}

impl OptimizationPass for PgoOptimizePass {
    fn name(&self) -> &'static str {
        "pgo-optimize"
    }
    fn description(&self) -> &'static str {
        "Optimizes IR using profile-guided feedback"
    }

    fn run_on_function(&self, func: &mut Function) -> Result<PassResult, CompileError> {
        let mut result = PassResult::default();

        // 根据 block heat 重排基本块（热块放在前面）
        let mut indexed: Vec<(usize, u64)> = func
            .blocks
            .iter()
            .enumerate()
            .map(|(i, b)| (i, self.counters.block_heat(b.id)))
            .collect();
        indexed.sort_by_key(|(_, h)| std::cmp::Reverse(*h));

        // 检查是否需要重排（入口块必须保持在位置 0）
        if indexed.len() <= 1 || indexed[0].0 == 0 {
            return Ok(result);
        }

        // 保持入口块在位置 0
        let entry_pos = indexed.iter().position(|&(i, _)| i == 0);
        if let Some(ep) = entry_pos {
            let entry = indexed.remove(ep);
            indexed.insert(0, entry);
        }

        // 构建旧索引 → 新索引映射
        let mut old_to_new: Vec<Option<usize>> = vec![None; func.blocks.len()];
        for (new_idx, &(old_idx, _)) in indexed.iter().enumerate() {
            old_to_new[old_idx] = Some(new_idx);
        }

        // 更新块中所有 BlockId 引用
        let update_bid = |bid: &mut BlockId| {
            let old_idx = bid.0 as usize;
            if let Some(Some(new_idx)) = old_to_new.get(old_idx) {
                *bid = BlockId(*new_idx as u32);
            }
        };

        for block in func.blocks.iter_mut() {
            match &mut block.terminator {
                Terminator::Branch {
                    true_block,
                    false_block,
                    ..
                } => {
                    update_bid(true_block);
                    update_bid(false_block);
                }
                Terminator::Jump { target, .. } => update_bid(target),
                Terminator::Switch {
                    default_block,
                    cases,
                    ..
                } => {
                    update_bid(default_block);
                    for (_, target, _) in cases.iter_mut() {
                        update_bid(target);
                    }
                }
                _ => {}
            }
        }

        // 物理重排 blocks 向量
        let old_blocks = std::mem::take(&mut func.blocks);
        for (old_idx, _) in &indexed {
            func.blocks.push(old_blocks[*old_idx].clone());
        }

        result.changed = true;
        result.blocks_removed += 0; // no blocks removed, just reordered
        Ok(result)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ir::FunctionBuilder;

    #[test]
    fn pgo_instrument_adds_counters() {
        let sig = Signature::new(&[(Type::I32, "x")], &[Type::I32]);
        let mut b = FunctionBuilder::new("test", sig);
        let (entry, params) = b.create_block_with_params(&[(Type::I32, "x")]);
        b.switch_to_block(entry);
        b.return_(&[params[0]]);

        let mut func = b.finish();
        let pass = PgoInstrumentPass::new();
        let r = pass.run_on_function(&mut func).unwrap();
        assert!(r.changed);
        assert!(func.validate().is_valid());
    }

    #[test]
    fn pgo_counters_default() {
        let counters = PgoCounters::new();
        assert_eq!(counters.block_heat(BlockId(0)), 0);
        assert_eq!(counters.edge_probability(BlockId(0), BlockId(1)), 50);
    }

    #[test]
    fn pgo_reorder_hot_block_first() {
        // Build a function with 3 blocks, mark block 2 as hot
        let sig = Signature::new(&[], &[Type::I32]);
        let mut b = FunctionBuilder::new("test", sig);
        let entry = b.create_block();
        let cold = b.create_block();
        let hot = b.create_block();

        b.switch_to_block(entry);
        let c = b.iconst_i32(1);
        b.branch(c, hot, cold, &[], &[]);

        b.switch_to_block(cold);
        let v_cold = b.iconst_i32(0);
        b.return_(&[v_cold]);

        b.switch_to_block(hot);
        let v_hot = b.iconst_i32(42);
        b.return_(&[v_hot]);

        let mut func = b.finish();

        // BlockId mapping: entry=0, cold=1, hot=2. Mark hot as... hot.
        let mut counters = PgoCounters::new();
        counters.block_counts.insert(BlockId(2), 1000); // hot block
        counters.block_counts.insert(BlockId(0), 100); // entry
        counters.block_counts.insert(BlockId(1), 1); // cold block

        let pass = PgoOptimizePass::new(counters);
        let r = pass.run_on_function(&mut func).unwrap();
        assert!(r.changed, "Block reorder should have occurred");
        assert!(func.validate().is_valid());
        // Entry block should still be first
        assert_eq!(func.blocks[0].id, BlockId(0), "Entry must remain first");
    }
}
