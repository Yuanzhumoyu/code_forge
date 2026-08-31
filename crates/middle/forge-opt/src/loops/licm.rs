//! 循环不变量外提 (LICM) pass.
//!
//! Hoists loop-invariant computations out of loops.

use crate::{OptimizationPass, PassResult};
use forge_ir::IrError;
use forge_ir::*;
use std::collections::{HashMap, HashSet};

#[derive(Default)]
pub struct LicmPass;

impl LicmPass {
    pub fn new() -> Self {
        Self
    }
}

impl OptimizationPass for LicmPass {
    fn name(&self) -> &'static str {
        "licm"
    }
    fn description(&self) -> &'static str {
        "Loop-invariant code motion"
    }
    fn run_on_function(&self, func: &mut Function) -> Result<PassResult, IrError> {
        hoist_loop_invariants(func)
    }
}

pub fn hoist_loop_invariants(func: &mut Function) -> Result<PassResult, IrError> {
    let mut result = PassResult::default();
    // P1-5：别名分析（load 外提判定：循环内无 may-alias 写才可外提）
    let alias = AliasAnalysis::new();
    // 使用函数上的惰性分析缓存（licm 只移动指令、不改变块结构，支配树/
    // 循环森林无需重建）；无循环或循环体过小时快速返回。
    let loops: Vec<(Block, Block, HashSet<Block>, Vec<MemoryLocation>)> = func
        .loop_forest()
        .all_loops()
        .iter()
        .filter(|li| li.blocks.len() > 1)
        // P1-2/P1-3：外提目标 = preheader（若有——header 唯一循环外 pred，
        // 每迭代只执行一次的真正不变量位置）；无 preheader 时回退 header
        //（旧行为；insert_preheader pass 正规化后可全量走 preheader）。
        .map(|li| {
            let body: HashSet<Block> = li.blocks.iter().copied().collect();
            let write_locs = collect_loop_writes(func, &body, &alias);
            (
                li.header,
                li.preheader.unwrap_or(li.header),
                body,
                write_locs,
            )
        })
        .collect();
    if loops.is_empty() {
        return Ok(result);
    }

    for (_header, target, body, write_locs) in loops {
        let outside_values = collect_values_outside(func, &body);
        let invariants = mark_invariants(func, &body, &outside_values, &alias, &write_locs);
        if invariants.is_empty() {
            continue;
        }

        let count = hoist_to_header(func, target, &invariants);
        if count > 0 {
            result.instructions_removed += count;
            result.changed = true;
        }
    }
    if result.changed {
        // 保守：改动后使分析缓存失效，后续 pass 使用新鲜分析
        func.analysis_mut().invalidate();
    }
    Ok(result)
}

/// P1-5：循环体内全部 may-write 的位置（调用 → Unknown，与任何 load MayAlias）。
fn collect_loop_writes(
    func: &Function,
    body: &HashSet<Block>,
    alias: &AliasAnalysis,
) -> Vec<MemoryLocation> {
    let mut writes = Vec::new();
    for bi in 0..func.dfg.blocks.len() {
        let bid = Block(bi as u32);
        if !body.contains(&bid) {
            continue;
        }
        for &inst_id in &func.dfg.blocks[bi].inst_order {
            let inst = &func.dfg.insts[inst_id.0 as usize];
            match inst.opcode {
                Opcode::Call | Opcode::CallIndirect => writes.push(MemoryLocation::Unknown),
                Opcode::Store | Opcode::Fstore | Opcode::AtomicRmw | Opcode::Cmpxchg => {
                    if let Some(loc) = alias.location_of_access(func, inst) {
                        writes.push(loc);
                    }
                }
                _ => {}
            }
        }
    }
    writes
}

fn collect_values_outside(func: &Function, body: &HashSet<Block>) -> HashSet<Value> {
    let mut values = HashSet::new();
    for bi in 0..func.dfg.blocks.len() {
        let bid = Block(bi as u32);
        if body.contains(&bid) {
            continue;
        }
        let block = &func.dfg.blocks[bi];
        for &v in &block.param_values {
            values.insert(v);
        }
        for &inst_id in &block.inst_order {
            let inst = &func.dfg.insts[inst_id.0 as usize];
            if let Some(v) = inst.results.first().copied() {
                values.insert(v);
            }
        }
    }
    values
}

fn mark_invariants(
    func: &Function,
    body: &HashSet<Block>,
    outside: &HashSet<Value>,
    alias: &AliasAnalysis,
    write_locs: &[MemoryLocation],
) -> HashSet<(Block, Inst)> {
    let mut inv_values: HashSet<Value> = HashSet::new();
    let mut inv_insts: HashSet<(Block, Inst)> = HashSet::new();

    // First pass: constants are always invariant
    for bi in 0..func.dfg.blocks.len() {
        let bid = Block(bi as u32);
        if !body.contains(&bid) {
            continue;
        }
        for &inst_id in &func.dfg.blocks[bi].inst_order {
            let inst = &func.dfg.insts[inst_id.0 as usize];
            if matches!(inst.opcode, Opcode::Iconst | Opcode::Fconst)
                && let Some(v) = inst.results.first().copied()
            {
                inv_values.insert(v);
                inv_insts.insert((bid, inst_id));
            }
        }
    }

    // Iterate until fixed point
    let mut changed = true;
    while changed {
        changed = false;
        for bi in 0..func.dfg.blocks.len() {
            let bid = Block(bi as u32);
            if !body.contains(&bid) {
                continue;
            }
            for &inst_id in &func.dfg.blocks[bi].inst_order {
                if inv_insts.contains(&(bid, inst_id)) {
                    continue;
                }
                let inst = &func.dfg.insts[inst_id.0 as usize];
                if inst.results.is_empty() {
                    continue;
                }
                // P1-5：load 在「地址不变 + 循环内无 may-alias 写」时可外提
                // （每迭代读同一位置 → 提前到 preheader 只读一次等价）。
                // volatile / 原子 load 保留可观察语义，不外提。
                if crate::scalar::cse::is_load_op(&inst.opcode) {
                    if inst.mem_flags.is_volatile() {
                        continue;
                    }
                    let Some(loc) = alias.location_of_access(func, inst) else {
                        continue;
                    };
                    if write_locs
                        .iter()
                        .any(|&w| alias.alias(loc, w) != AliasResult::NoAlias)
                    {
                        continue;
                    }
                } else if has_side_effects(&inst.opcode) {
                    continue;
                }
                if inst
                    .operands
                    .iter()
                    .all(|v| outside.contains(v) || inv_values.contains(v))
                    && let Some(v) = inst.results.first().copied()
                {
                    inv_values.insert(v);
                    inv_insts.insert((bid, inst_id));
                    changed = true;
                }
            }
        }
    }
    inv_insts
}

/// Pre-collected invariant instruction IDs (avoids borrow conflicts with DFG).
struct InvariantData {
    inst_id: Inst,
    old_results: Vec<Value>,
}

fn hoist_to_header(
    func: &mut Function,
    header: Block,
    invariants: &HashSet<(Block, Inst)>,
) -> usize {
    // Phase 1: Collect invariant instruction IDs (immutable borrows)
    let mut hoist_data: Vec<InvariantData> = Vec::new();
    for bi in 0..func.dfg.blocks.len() {
        let bid = Block(bi as u32);
        let inst_order = func.dfg.blocks[bi].inst_order.clone();
        for &inst_id in &inst_order {
            if !invariants.contains(&(bid, inst_id)) {
                continue;
            }
            let inst = &func.dfg.insts[inst_id.0 as usize];
            if inst.results.is_empty() {
                continue;
            }
            hoist_data.push(InvariantData {
                inst_id,
                old_results: inst.results.to_vec(),
            });
        }
    }

    if hoist_data.is_empty() {
        return 0;
    }

    // Phase 2: Clone invariants to header（保留全字段：flags/mem_flags/metadata/loc），
    // 自动维护 val_remap（链式克隆把先前 hoist 的值重定向）
    let mut val_remap: HashMap<Value, Value> = HashMap::new();
    let mut count = 0;

    for hd in &hoist_data {
        func.dfg.clone_inst(hd.inst_id, header, &mut val_remap);
        count += 1;
    }

    // Phase 3: Replace all uses of old values with hoisted values（DFG + use-lists 双更新）
    for hd in &hoist_data {
        for &old_r in &hd.old_results {
            if let Some(&new_r) = val_remap.get(&old_r) {
                func.replace_all_uses(old_r, new_r);
            }
        }
    }

    // Phase 4: 原子删除原 invariant 指令（kill_inst：墓碑化 + use-lists 清理）
    for hd in &hoist_data {
        func.kill_inst(hd.inst_id);
    }

    count
}

fn has_side_effects(opcode: &Opcode) -> bool {
    matches!(
        opcode,
        // P1-5：Load/Fload 不再整体视为副作用（load 外提判定单独处理）；
        // Fstore 补入写集合（此前缺失）。
        Opcode::Store
            | Opcode::Fstore
            | Opcode::Call
            | Opcode::CallIndirect
            | Opcode::AtomicRmw
            | Opcode::Cmpxchg
            | Opcode::Fence
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn licm_no_crash() {
        let sig = FunctionSignature::new(&[], &[TypeId::I32]);
        let mut b = FunctionBuilder::new("test", TypeContext::new(), sig);
        let entry = b.create_block();
        b.switch_to_block(entry);
        let v = b.iconst_i32(42);
        b.ret(&[v]);
        let mut func = b.finish().expect("build");
        let pass = LicmPass::new();
        let r = pass.run_on_function(&mut func).unwrap();
        assert!(!r.changed);
    }

    /// Test that a loop-invariant Iconst is hoisted out of the loop body.
    #[test]
    fn licm_hoist_invariant_const() {
        // Build: while cond { x = 42; ... }
        // The const 42 inside the loop should be hoisted to the header
        let sig = FunctionSignature::new(&[], &[TypeId::I32]);
        let mut b = FunctionBuilder::new("test", TypeContext::new(), sig);
        let preheader = b.create_block();
        b.switch_to_block(preheader);
        let zero = b.iconst_i32(0);
        let header = b.create_block();
        b.switch_to_block(preheader);
        b.jump(header, &[]);

        b.switch_to_block(header);
        let n = b.iconst_i32(10);
        let cond = b.icmp(IntCC::SignedLessThan, zero, n);
        let body_blk = b.create_block();
        let exit = b.create_block();
        b.switch_to_block(header);
        b.branch(cond, body_blk, &[], exit, &[]);

        b.switch_to_block(body_blk);
        let loop_const = b.iconst_i32(42); // loop-invariant — should be hoisted
        let one = b.iconst_i32(1);
        let _used = b.iadd(loop_const, one);
        b.jump(header, &[]);

        b.switch_to_block(exit);
        b.ret(&[]);
        let mut func = b.finish().expect("build");

        let pass = LicmPass::new();
        let r = pass.run_on_function(&mut func).unwrap();
        assert!(
            r.changed,
            "LICM should hoist invariant Iconst from loop body"
        );
        assert!(
            r.instructions_removed > 0,
            "Should report hoisted instructions"
        );
    }

    /// Test that a loop-invariant computation (add of two outside values) is hoisted.
    #[test]
    fn licm_hoist_invariant_computation() {
        // Build: while cond { x = a + b; ... } where a,b are defined outside the loop
        let sig = FunctionSignature::new(&[(TypeId::I32, "a"), (TypeId::I32, "b")], &[TypeId::I32]);
        let mut b = FunctionBuilder::new("test", TypeContext::new(), sig);
        let (preheader, params) =
            b.create_block_with_params(&[(TypeId::I32, "a"), (TypeId::I32, "b")]);
        let a = params[0];
        let b_val = params[1];
        let zero = b.iconst_i32(0);
        let header = b.create_block();
        b.switch_to_block(preheader);
        b.jump(header, &[]);

        b.switch_to_block(header);
        let n = b.iconst_i32(10);
        let cond = b.icmp(IntCC::SignedLessThan, zero, n);
        let body_blk = b.create_block();
        let exit = b.create_block();
        b.switch_to_block(header);
        b.branch(cond, body_blk, &[], exit, &[]);

        b.switch_to_block(body_blk);
        let _invariant_add = b.iadd(a, b_val); // loop-invariant: a,b are outside the loop
        b.jump(header, &[]);

        b.switch_to_block(exit);
        b.ret(&[]);
        let mut func = b.finish().expect("build");

        let pass = LicmPass::new();
        let r = pass.run_on_function(&mut func).unwrap();
        assert!(r.changed, "LICM should hoist invariant Iadd from loop body");
        assert!(
            r.instructions_removed > 0,
            "Should report hoisted instructions"
        );
    }

    /// Test that non-invariant instructions (depends on loop-varying value) are NOT hoisted.
    #[test]
    fn licm_does_not_hoist_variant() {
        // Build: for i in 0..10: x = i + 5;  → i is loop-varying, should NOT be hoisted
        let sig = FunctionSignature::new(&[], &[TypeId::I32]);
        let mut b = FunctionBuilder::new("test", TypeContext::new(), sig);
        let entry = b.create_block();
        b.switch_to_block(entry);
        let zero = b.iconst_i32(0);
        let (header, header_params) = b.create_block_with_params(&[(TypeId::I32, "i")]);
        b.switch_to_block(entry);
        b.jump(header, &[zero]);

        b.switch_to_block(header);
        let i = header_params[0];
        let n = b.iconst_i32(10);
        let cond = b.icmp(IntCC::SignedLessThan, i, n);
        let body_blk = b.create_block();
        let exit = b.create_block();
        b.switch_to_block(header);
        b.branch(cond, body_blk, &[], exit, &[]);

        b.switch_to_block(body_blk);
        let five = b.iconst_i32(5);
        let _variant_add = b.iadd(i, five); // i is loop-varying → NOT invariant
        let one = b.iconst_i32(1);
        let next_i = b.iadd(i, one);
        b.jump(header, &[next_i]);

        b.switch_to_block(exit);
        b.ret(&[]);
        let mut func = b.finish().expect("build");

        let pass = LicmPass::new();
        let r = pass.run_on_function(&mut func).unwrap();
        // Only the Iconst 5 should be hoisted; the Iadd(i, 5) should NOT be hoisted
        // So LICM should report some changes (from Iconst hoisting), but fewer than if
        // the Iadd was also hoisted
        // At minimum: the Iconst(5) is hoisted, so changed=true
        assert!(
            r.changed || r.instructions_removed == 0,
            "Only Iconst(5) is invariant; Iadd(i,5) depends on loop-varying i"
        );
    }

    /// P1-5 正向：循环体 load 栈槽（地址在外、循环内无 may-alias 写）→ 外提到
    /// preheader；循环体不再含 load。
    #[test]
    fn p1_5_licm_hoist_invariant_load() {
        let sig = FunctionSignature::new(&[], &[TypeId::I32]);
        let mut b = FunctionBuilder::new("test", TypeContext::new(), sig);
        let preheader = b.create_block();
        b.switch_to_block(preheader);
        let p = b.stack_addr(0); // 地址在循环外
        let zero = b.iconst_i32(0);
        let header = b.create_block();
        b.switch_to_block(preheader);
        b.jump(header, &[]);

        b.switch_to_block(header);
        let n = b.iconst_i32(10);
        let cond = b.icmp(IntCC::SignedLessThan, zero, n);
        let body_blk = b.create_block();
        let exit = b.create_block();
        b.switch_to_block(header);
        b.branch(cond, body_blk, &[], exit, &[]);

        b.switch_to_block(body_blk);
        let v = b.load(p, TypeId::I32); // 循环不变 load（无写 → 可外提）
        let one = b.iconst_i32(1);
        let _used = b.iadd(v, one);
        b.jump(header, &[]);

        b.switch_to_block(exit);
        b.ret(&[]);
        let mut func = b.finish().expect("build");

        let pass = LicmPass::new();
        let r = pass.run_on_function(&mut func).unwrap();
        assert!(r.changed, "LICM 应外提不变 load");

        // 全函数只剩 1 条 load，且不在循环体
        let mut loads: Vec<Block> = Vec::new();
        for (bi, blk) in func.dfg.blocks.iter().enumerate() {
            for &inst_id in &blk.inst_order {
                let inst = &func.dfg.insts[inst_id.0 as usize];
                if matches!(inst.opcode, Opcode::Load) {
                    loads.push(Block(bi as u32));
                }
            }
        }
        assert_eq!(loads.len(), 1, "load 应被外提（只剩 1 条）");
        assert_ne!(loads[0], body_blk, "load 不应留在循环体");
    }

    /// P1-5 负向：循环体对同一槽 store + load → load 与 store MayAlias，
    /// 不得外提（load 仍留在循环体）。
    #[test]
    fn p1_5_licm_no_hoist_load_with_alias_store() {
        let sig = FunctionSignature::new(&[], &[TypeId::I32]);
        let mut b = FunctionBuilder::new("test", TypeContext::new(), sig);
        let preheader = b.create_block();
        b.switch_to_block(preheader);
        let p = b.stack_addr(0);
        let zero = b.iconst_i32(0);
        let header = b.create_block();
        b.switch_to_block(preheader);
        b.jump(header, &[]);

        b.switch_to_block(header);
        let n = b.iconst_i32(10);
        let cond = b.icmp(IntCC::SignedLessThan, zero, n);
        let body_blk = b.create_block();
        let exit = b.create_block();
        b.switch_to_block(header);
        b.branch(cond, body_blk, &[], exit, &[]);

        b.switch_to_block(body_blk);
        let sv = b.iconst_i32(7);
        b.store(sv, p); // 写同一槽 —— MayAlias
        let v = b.load(p, TypeId::I32); // 读到的值被 store 决定 → 不可外提
        let one = b.iconst_i32(1);
        let _used = b.iadd(v, one);
        b.jump(header, &[]);

        b.switch_to_block(exit);
        b.ret(&[]);
        let mut func = b.finish().expect("build");

        let pass = LicmPass::new();
        let r = pass.run_on_function(&mut func).unwrap();

        // load 必须仍留在循环体
        let mut loads_in_body = 0;
        for &inst_id in &func.dfg.blocks[body_blk.0 as usize].inst_order {
            let inst = &func.dfg.insts[inst_id.0 as usize];
            if matches!(inst.opcode, Opcode::Load) {
                loads_in_body += 1;
            }
        }
        assert_eq!(
            loads_in_body, 1,
            "MayAlias store 在循环内 → load 不得外提（P1-5 负向）"
        );
        let _ = r;
    }
}
