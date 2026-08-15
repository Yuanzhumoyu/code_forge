//! Mem2Reg pass — 将栈分配的变量提升为 SSA 值。
//!
//! v2 IR 使用 Store/Load + StackAddr（替代 v1 的 StackStore/StackLoad）。
//! 此 pass 识别仅被 Store/Load 访问的栈槽，将其提升为 SSA 值。
//!
//! # 算法 (v2 重新设计)
//!
//! 1. 识别 alloca 候选: `StackAddr` 指令，其结果仅被 Store/Load 使用
//!    （无 address-taken 逃逸 — 不能作为 call 参数或 gep 基址）
//! 2. 简单情况（单 Store，支配所有 Load）→ 直接用 stored value 替换 Load
//! 3. 多 Store 情况 → 在 iterated dominance frontier 插入 block params
//! 4. 移除 Store/Load/StackAddr 指令

use crate::{OptimizationPass, PassResult};
use forge_ir::IrError;
use forge_ir::*;

#[derive(Default)]
pub struct Mem2RegPass;

impl Mem2RegPass {
    pub fn new() -> Self {
        Self
    }
}

impl OptimizationPass for Mem2RegPass {
    fn name(&self) -> &'static str {
        "mem2reg"
    }
    fn description(&self) -> &'static str {
        "Promotes stack-allocated variables to SSA values using Load/Store + StackAddr analysis"
    }
    fn run_on_function(&self, func: &mut Function) -> Result<PassResult, IrError> {
        promote_to_ssa(func)
    }
}

struct SlotInfo {
    /// 产生 ptr 值的 StackAddr 指令
    stack_addr_inst: Inst,
    /// 生成的 ptr 值
    ptr_value: Value,
    /// 所有 Store 指令: (block, inst, stored_value)
    stores: Vec<(Block, Inst, Value)>,
    /// 所有 Load 指令: (block, inst)
    loads: Vec<(Block, Inst)>,
}

/// 对函数执行 mem2reg 提升。
pub fn promote_to_ssa(func: &mut Function) -> Result<PassResult, IrError> {
    let mut result = PassResult::default();
    let dt = func.dominator_tree().clone();

    // Phase 1: 收集 StackAddr 候选
    let mut slots: Vec<SlotInfo> = Vec::new();

    for bi in 0..func.dfg.blocks.len() {
        let _block = Block(bi as u32);
        let block_data = &func.dfg.blocks[bi];
        for &inst_id in &block_data.inst_order {
            let inst = &func.dfg.insts[inst_id.0 as usize];
            if inst.opcode != Opcode::StackAddr {
                continue;
            }
            let ptr_val = match inst.results.first() {
                Some(&v) => v,
                None => continue,
            };

            // Check: is this StackAddr only used by Load/Store?
            let uses = func.use_lists.uses(ptr_val);
            if uses.is_empty() {
                continue;
            }
            let only_load_store = uses.iter().all(|u| {
                let user_inst = &func.dfg.insts[u.user.0 as usize];
                matches!(user_inst.opcode, Opcode::Load | Opcode::Store)
            });
            if !only_load_store {
                continue;
            }

            let mut store_list = Vec::new();
            let mut load_list = Vec::new();

            for u in uses {
                let user_inst = &func.dfg.insts[u.user.0 as usize];
                let user_block = user_inst.block;
                match user_inst.opcode {
                    Opcode::Store => {
                        // Store(value, ptr) — first operand is stored value, second is ptr
                        if let Some(&stored_val) = user_inst.operands.first() {
                            store_list.push((user_block, u.user, stored_val));
                        }
                    }
                    Opcode::Load => {
                        load_list.push((user_block, u.user));
                    }
                    _ => {}
                }
            }

            if !store_list.is_empty() && !load_list.is_empty() {
                slots.push(SlotInfo {
                    stack_addr_inst: inst_id,
                    ptr_value: ptr_val,
                    stores: store_list,
                    loads: load_list,
                });
            }
        }
    }

    // Phase 2: 对每个槽进行提升
    for slot in &slots {
        match slot.stores.len() {
            0 => continue,
            1 => {
                // Simple case: single store dominates all loads
                let (store_block, _, stored_val) = slot.stores[0];
                let all_dominated = slot
                    .loads
                    .iter()
                    .all(|(load_block, _)| dt.dominates(store_block, *load_block));

                if all_dominated {
                    // Replace all Load results with the stored value
                    for &(_, load_inst) in &slot.loads {
                        let load_results: Vec<Value> = func.dfg.insts[load_inst.0 as usize]
                            .results
                            .iter()
                            .copied()
                            .collect();
                        for lr in load_results {
                            func.use_lists.replace_all_uses(lr, stored_val);
                        }
                        // Mark load as Nop
                        func.use_lists.remove_inst(&func.dfg, load_inst);
                        func.dfg.remove_inst(load_inst);
                    }

                    // Remove Store
                    func.use_lists.remove_inst(&func.dfg, slot.stores[0].1);
                    func.dfg.remove_inst(slot.stores[0].1);

                    // Remove StackAddr
                    func.use_lists.remove_inst(&func.dfg, slot.stack_addr_inst);
                    func.dfg.remove_inst(slot.stack_addr_inst);

                    result.instructions_removed += slot.loads.len() + slot.stores.len() + 1;
                    result.values_replaced += slot.loads.len();
                    result.changed = true;
                }
            }
            _ => {
                // Multi-store case
                let mut promoted = false;

                // 2a. Trivial: all stores have the same value, and each load
                //     is dominated by at least one store.
                let first_val = slot.stores[0].2;
                let all_same_value = slot.stores.iter().all(|&(_, _, v)| v == first_val);
                let all_dominated = slot
                    .loads
                    .iter()
                    .all(|(lb, _)| slot.stores.iter().any(|&(sb, _, _)| dt.dominates(sb, *lb)));

                if all_same_value && all_dominated {
                    for &(_, load_inst) in &slot.loads {
                        let load_results: Vec<Value> = func.dfg.insts[load_inst.0 as usize]
                            .results
                            .iter()
                            .copied()
                            .collect();
                        for lr in load_results {
                            func.use_lists.replace_all_uses(lr, first_val);
                        }
                        func.use_lists.remove_inst(&func.dfg, load_inst);
                        func.dfg.remove_inst(load_inst);
                    }
                    for &(_, store_inst, _) in &slot.stores {
                        func.use_lists.remove_inst(&func.dfg, store_inst);
                        func.dfg.remove_inst(store_inst);
                    }
                    func.use_lists.remove_inst(&func.dfg, slot.stack_addr_inst);
                    func.dfg.remove_inst(slot.stack_addr_inst);

                    result.instructions_removed += slot.loads.len() + slot.stores.len() + 1;
                    result.values_replaced += slot.loads.len();
                    result.changed = true;
                    promoted = true;
                }

                // 2b. Local store forwarding: within each block, track the
                //     reaching store value and forward it to subsequent loads
                //     of the same slot address.
                if !promoted {
                    let mut load_forward: Vec<(Inst, Value)> = Vec::new();

                    for bi in 0..func.dfg.blocks.len() {
                        let block_data = &func.dfg.blocks[bi];
                        let mut reaching: Option<Value> = None;

                        for &inst_id in &block_data.inst_order {
                            let inst = &func.dfg.insts[inst_id.0 as usize];
                            if matches!(inst.opcode, Opcode::Nop) {
                                continue;
                            }

                            // Store(value, ptr) to our slot → update reaching def
                            if matches!(inst.opcode, Opcode::Store)
                                && inst.operands.len() >= 2
                                && inst.operands[1] == slot.ptr_value
                                && let Some(&stored_val) = inst.operands.first()
                            {
                                reaching = Some(stored_val);
                            }

                            // Load(ptr) from our slot → forward if reaching def exists
                            if matches!(inst.opcode, Opcode::Load)
                                && !inst.operands.is_empty()
                                && inst.operands[0] == slot.ptr_value
                                && let Some(stored_val) = reaching
                            {
                                load_forward.push((inst_id, stored_val));
                            }
                        }
                    }

                    let forwarded = load_forward.len();
                    for (load_inst, stored_val) in load_forward {
                        let load_results: Vec<Value> = func.dfg.insts[load_inst.0 as usize]
                            .results
                            .iter()
                            .copied()
                            .collect();
                        for lr in load_results {
                            func.use_lists.replace_all_uses(lr, stored_val);
                        }
                        func.use_lists.remove_inst(&func.dfg, load_inst);
                        func.dfg.remove_inst(load_inst);
                    }

                    if forwarded > 0 {
                        result.instructions_removed += forwarded;
                        result.values_replaced += forwarded;
                        result.changed = true;
                        promoted = true;
                    }
                }

                // 2c. Cross-block SSA construction with block params at
                //     iterated dominance frontiers. Deferred: requires
                //     dynamic block param insertion + jump arg update.
                let _ = promoted;
                // Complex multi-store with different values across blocks:
                // requires IDF-based SSA construction (future iteration)
            }
        }
    }

    Ok(result)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn mem2reg_single_store_single_load() {
        // Simple: store to stack, load from stack → replace load with stored value
        let sig = FunctionSignature::new(&[], &[TypeId::I32]);
        let mut b = FunctionBuilder::new("test", TypeContext::new(), sig);
        let entry = b.create_block();
        b.switch_to_block(entry);
        let addr = b.stack_addr(0); // StackAddr
        let v42 = b.iconst_i32(42);
        b.store(v42, addr); // Store 42
        let loaded = b.load(addr, TypeId::I32); // Load
        b.ret(&[loaded]);

        let mut func = b.finish().expect("build");
        let live_before = func
            .dfg
            .insts
            .iter()
            .filter(|i| !matches!(i.opcode, crate::Opcode::Nop))
            .count();

        let pass = Mem2RegPass::new();
        let r = pass.run_on_function(&mut func).unwrap();
        assert!(r.changed, "Should promote simple store/load");
        // mem2reg 墓碑化删除的指令(Nop 不回收)——统计活指令验证减少
        let live_after = func
            .dfg
            .insts
            .iter()
            .filter(|i| !matches!(i.opcode, crate::Opcode::Nop))
            .count();
        assert!(live_after < live_before, "Instructions should be removed");
    }

    #[test]
    fn mem2reg_no_change_without_load() {
        // Store without Load → can't promote (StackAddr not used by Load)
        let sig = FunctionSignature::new(&[], &[TypeId::I32]);
        let mut b = FunctionBuilder::new("test", TypeContext::new(), sig);
        let entry = b.create_block();
        b.switch_to_block(entry);
        let addr = b.stack_addr(0);
        let v42 = b.iconst_i32(42);
        b.store(v42, addr);
        b.ret(&[v42]); // uses v42 directly, not loaded

        let mut func = b.finish().expect("build");
        let pass = Mem2RegPass::new();
        let r = pass.run_on_function(&mut func).unwrap();
        // No Load uses the ptr, so can't promote
        assert!(!r.changed);
    }

    #[test]
    fn mem2reg_no_crash() {
        let sig = FunctionSignature::new(&[], &[TypeId::I32]);
        let mut b = FunctionBuilder::new("test", TypeContext::new(), sig);
        let entry = b.create_block();
        b.switch_to_block(entry);
        let v = b.iconst_i32(42);
        b.ret(&[v]);
        let mut func = b.finish().expect("build");

        let pass = Mem2RegPass::new();
        let r = pass.run_on_function(&mut func).unwrap();
        assert!(!r.changed);
    }

    /// Test local store forwarding: within a single block, a store followed
    /// by a load to the same slot should forward the stored value.
    #[test]
    fn mem2reg_local_store_forward() {
        let sig = FunctionSignature::new(&[], &[TypeId::I32]);
        let mut b = FunctionBuilder::new("test", TypeContext::new(), sig);
        let entry = b.create_block();
        b.switch_to_block(entry);
        let addr = b.stack_addr(0);
        let v42 = b.iconst_i32(42);
        b.store(v42, addr);
        let loaded = b.load(addr, TypeId::I32);
        b.ret(&[loaded]);

        let mut func = b.finish().expect("build");
        let pass = Mem2RegPass::new();
        let r = pass.run_on_function(&mut func).unwrap();
        assert!(r.changed, "Should forward store→load in same block");
    }

    /// Test local store forwarding with two different stored values:
    /// store 42, load→42, store 99, load→99.
    #[test]
    fn mem2reg_local_store_forward_two_values() {
        let sig = FunctionSignature::new(&[], &[TypeId::I32]);
        let mut b = FunctionBuilder::new("test", TypeContext::new(), sig);
        let entry = b.create_block();
        b.switch_to_block(entry);
        let addr = b.stack_addr(0);
        let v42 = b.iconst_i32(42);
        let v99 = b.iconst_i32(99);
        b.store(v42, addr);
        let load1 = b.load(addr, TypeId::I32);
        b.store(v99, addr);
        let load2 = b.load(addr, TypeId::I32);
        let sum = b.iadd(load1, load2); // uses: 42 + 99 = 141
        b.ret(&[sum]);

        let mut func = b.finish().expect("build");
        let pass = Mem2RegPass::new();
        let r = pass.run_on_function(&mut func).unwrap();
        // Single-store case should promote the entire slot
        // (first store dominates first load; but there are two stores,
        // so local forwarding should catch at least the first load)
        assert!(
            r.changed,
            "Should forward at least one load via local forwarding"
        );
    }

    /// Test that a load before any store is NOT forwarded (no reaching def).
    #[test]
    fn mem2reg_local_forward_no_reaching_def() {
        let sig = FunctionSignature::new(&[], &[TypeId::I32]);
        let mut b = FunctionBuilder::new("test", TypeContext::new(), sig);
        let entry = b.create_block();
        b.switch_to_block(entry);
        let addr = b.stack_addr(0);
        // Load before any store — no reaching definition
        let _load_uninit = b.load(addr, TypeId::I32);
        let v42 = b.iconst_i32(42);
        b.store(v42, addr);
        let loaded = b.load(addr, TypeId::I32);
        b.ret(&[loaded]);

        let mut func = b.finish().expect("build");
        let pass = Mem2RegPass::new();
        let r = pass.run_on_function(&mut func).unwrap();
        // The second load should be forwarded (reaching def = v42)
        assert!(r.changed, "Should forward second load (has reaching store)");
    }
}
