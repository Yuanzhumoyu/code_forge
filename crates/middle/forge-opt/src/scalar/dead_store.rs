//! 死 store 消除（DSE）——别名驱动的冗余内存写消除。
//!
//! 块内顺序扫描：维护「位置 → 最近 store」映射；同一位置的连续两个
//! store 之间没有任何读取该位置的 load/调用/原子 → 前一个 store 的
//! 结果从未被读取（被后者覆盖），可安全删除。
//!
//! 依赖 P1-5 的最小别名分析（`MemoryLocation`/`AliasResult`）：
//! - 不同栈槽 / 不同全局 / 栈 vs 全局互不重叠（NoAlias）→ 不互相影响；
//! - 同基址（MayAlias）store 之间、store 与 load 之间保守互杀；
//! - Unknown 位置（实参/load 结果指针）可能写任意位置 → store 清空全表，
//!   且 Unknown store 本身不记录（两未知地址可能不同）。
//!
//! 保守边界：volatile store/load 不参与（可观察语义）；call/原子清空全表
//! （call 可写任意位置、原子是读改写）；Fence 仅排序不写数据，不清表。

use crate::{OptimizationPass, PassResult};
use forge_ir::IrError;
use forge_ir::*;
use std::collections::HashMap;

/// 死 store 消除 pass。
#[derive(Default)]
pub struct DeadStoreElimPass;

impl DeadStoreElimPass {
    pub fn new() -> Self {
        Self
    }
}

impl OptimizationPass for DeadStoreElimPass {
    fn name(&self) -> &'static str {
        "dead-store"
    }

    fn description(&self) -> &'static str {
        "Eliminate redundant stores whose value is overwritten before any read (alias-driven)"
    }

    fn run_on_function(&self, func: &mut Function) -> Result<PassResult, IrError> {
        eliminate_dead_stores(func)
    }
}

/// 对单个函数执行块内死 store 消除。
pub fn eliminate_dead_stores(func: &mut Function) -> Result<PassResult, IrError> {
    let mut result = PassResult::default();
    let alias = AliasAnalysis::new();
    // 待删除的冗余 store（循环后统一 kill，避免借用冲突）
    let mut to_kill: Vec<Inst> = Vec::new();

    let block_count = func.dfg.blocks.len();
    for bi in 0..block_count {
        // 位置 → 最近 store 指令
        let mut last_store: HashMap<MemoryLocation, Inst> = HashMap::new();
        let inst_ids = func.dfg.blocks[bi].inst_order.clone();

        for inst_id in inst_ids {
            let inst = &func.dfg.insts[inst_id.0 as usize];
            match inst.opcode {
                Opcode::Store | Opcode::Fstore => {
                    // volatile store 不可删（可观察语义），也不记录
                    if inst.mem_flags.is_volatile() {
                        continue;
                    }
                    let Some(loc) = alias.location_of_access(func, inst) else {
                        continue;
                    };
                    if loc == MemoryLocation::Unknown {
                        // 未知地址可能写任意位置 → 清空全表；自身不记录
                        last_store.clear();
                        continue;
                    }
                    // 同位置（MayAlias）已有 store → 前者从未被读，删除
                    if let Some(prev) = last_store.get(&loc) {
                        to_kill.push(*prev);
                        result.instructions_removed += 1;
                        result.changed = true;
                    }
                    last_store.insert(loc, inst_id);
                }
                Opcode::Load | Opcode::Fload => {
                    // 读取：MayAlias 的待删 store 不可删（其值被本 load 使用）
                    let Some(loc) = alias.location_of_access(func, inst) else {
                        continue;
                    };
                    last_store.retain(|&l, _| alias.alias(l, loc) == AliasResult::NoAlias);
                }
                // call 可写任意位置、原子读改写 → 清空全表
                Opcode::Call | Opcode::CallIndirect | Opcode::AtomicRmw | Opcode::Cmpxchg => {
                    last_store.clear();
                }
                _ => {}
            }
        }
    }

    for inst in to_kill {
        func.kill_inst(inst);
    }

    Ok(result)
}

// ============================================================
// 测试
// ============================================================

#[cfg(test)]
mod tests {
    use super::*;

    /// 统计函数内 Store 指令数。
    fn count_stores(func: &Function) -> usize {
        func.dfg
            .insts
            .iter()
            .filter(|i| matches!(i.opcode, Opcode::Store | Opcode::Fstore))
            .count()
    }

    /// 正向：同一槽连续两 store（中间无读）→ 第一个删除（2 → 1）。
    #[test]
    fn dse_same_slot_consecutive_stores() {
        let sig = FunctionSignature::new(&[], &[]);
        let mut b = FunctionBuilder::new("test", TypeContext::new(), sig);
        let entry = b.create_block();
        b.switch_to_block(entry);
        let p0 = b.stack_addr(0);
        let v1 = b.iconst_i32(1);
        let v2 = b.iconst_i32(2);
        b.store(v1, p0);
        b.store(v2, p0);
        b.ret(&[]);
        let mut func = b.finish().expect("build");

        let pass = DeadStoreElimPass::new();
        let r = pass.run_on_function(&mut func).unwrap();
        assert!(r.changed, "同槽连续 store 应消除第一个");
        assert_eq!(count_stores(&func), 1, "应只剩 1 条 store");
    }

    /// 正向：异槽 store 互不影响（都保留）。
    #[test]
    fn dse_distinct_slots_both_kept() {
        let sig = FunctionSignature::new(&[], &[]);
        let mut b = FunctionBuilder::new("test", TypeContext::new(), sig);
        let entry = b.create_block();
        b.switch_to_block(entry);
        let p0 = b.stack_addr(0);
        let p1 = b.stack_addr(1);
        let v1 = b.iconst_i32(1);
        let v2 = b.iconst_i32(2);
        b.store(v1, p0);
        b.store(v2, p1);
        b.ret(&[]);
        let mut func = b.finish().expect("build");

        let pass = DeadStoreElimPass::new();
        let r = pass.run_on_function(&mut func).unwrap();
        assert!(!r.changed, "异槽 store 互不干扰，不应消除");
        assert_eq!(count_stores(&func), 2);
    }

    /// 负向：中间 load 读取同槽 → 前一个 store 不可删。
    #[test]
    fn dse_load_between_stores_blocks() {
        let sig = FunctionSignature::new(&[], &[]);
        let mut b = FunctionBuilder::new("test", TypeContext::new(), sig);
        let entry = b.create_block();
        b.switch_to_block(entry);
        let p0 = b.stack_addr(0);
        let v1 = b.iconst_i32(1);
        let v2 = b.iconst_i32(2);
        b.store(v1, p0);
        let _l = b.load(p0, TypeId::I32); // 读同槽 → 第一个 store 不可删
        b.store(v2, p0);
        b.ret(&[]);
        let mut func = b.finish().expect("build");

        let pass = DeadStoreElimPass::new();
        let r = pass.run_on_function(&mut func).unwrap();
        assert!(!r.changed, "load 读取后 store 不可删");
        assert_eq!(count_stores(&func), 2);
    }

    /// 负向：volatile store 不参与。
    #[test]
    fn dse_volatile_store_kept() {
        let sig = FunctionSignature::new(&[], &[]);
        let mut b = FunctionBuilder::new("test", TypeContext::new(), sig);
        let entry = b.create_block();
        b.switch_to_block(entry);
        let p0 = b.stack_addr(0);
        let v1 = b.iconst_i32(1);
        let v2 = b.iconst_i32(2);
        let flags = forge_ir::mem_flags::MemFlags::VOLATILE;
        b.store_with_flags(v1, p0, flags);
        b.store_with_flags(v2, p0, flags);
        b.ret(&[]);
        let mut func = b.finish().expect("build");

        let pass = DeadStoreElimPass::new();
        let r = pass.run_on_function(&mut func).unwrap();
        assert!(!r.changed, "volatile store 不可消除");
        assert_eq!(count_stores(&func), 2);
    }

    /// 负向：原子读改写（AtomicRmw）在中间 → 前一个 store 不可删
    /// （原子指令可能读/写同位置，保守清表）。
    #[test]
    fn dse_atomic_between_stores_blocks() {
        use forge_ir::opcode::{AtomicRmwOp, Ordering};
        let sig = FunctionSignature::new(&[], &[]);
        let mut b = FunctionBuilder::new("test", TypeContext::new(), sig);
        let entry = b.create_block();
        b.switch_to_block(entry);
        let p0 = b.stack_addr(0);
        let v1 = b.iconst_i32(1);
        let v2 = b.iconst_i32(2);
        b.store(v1, p0);
        b.atomic_rmw(AtomicRmwOp::Add, p0, v1, Ordering::SequentiallyConsistent);
        b.store(v2, p0);
        b.ret(&[]);
        let mut func = b.finish().expect("build");

        let pass = DeadStoreElimPass::new();
        let r = pass.run_on_function(&mut func).unwrap();
        assert!(!r.changed, "原子读改写后 store 不可删");
        assert_eq!(count_stores(&func), 2);
    }

    /// Unknown 位置 store（未知地址指针）→ 清空全表且不记录：
    /// `store v1, p0; store v3, p_unknown; store v2, p0` → 全部保留
    /// （中间未知 store 可能写 p0）。
    #[test]
    fn dse_unknown_addr_store_clears() {
        let sig = FunctionSignature::new(&[(TypeId::PTR, "p")], &[]);
        let mut b = FunctionBuilder::new("test", TypeContext::new(), sig);
        let (entry, params) = b.create_block_with_params(&[(TypeId::PTR, "p")]);
        b.switch_to_block(entry);
        let p0 = b.stack_addr(0);
        let v1 = b.iconst_i32(1);
        let v2 = b.iconst_i32(2);
        let v3 = b.iconst_i32(3);
        b.store(v1, p0);
        b.store(v3, params[0]); // 未知地址（实参指针）——保守清表
        b.store(v2, p0);
        b.ret(&[]);
        let mut func = b.finish().expect("build");

        let pass = DeadStoreElimPass::new();
        let r = pass.run_on_function(&mut func).unwrap();
        assert!(!r.changed, "Unknown store 可能写任意位置 → 不得消除");
        assert_eq!(count_stores(&func), 3);
    }
}
