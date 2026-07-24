//! Mem2Reg pass — 将栈分配的变量提升为 SSA 寄存器。
//!
//! 检测通过 StackLoad/StackStore 以相同 offset 访问的栈槽，
//! 将其替换为 SSA 值，在控制流汇合点插入 Phi 节点。
//!
//! # 算法（简化版）
//!
//! 1. 按 offset 分组 StackLoad/StackStore
//! 2. 对每对 (offset, type)，收集 Store 块和 Load 位置
//! 3. 在 Store 块支配边界插入 Phi 节点
//! 4. 用 SSA 值替换 Load，用 Copy 替换 Store
//! 5. 由后续 DCE 和 CopyProp 清理

use crate::CompileError;
use crate::ir::*;
use crate::optimize::{OptimizationPass, PassResult};
use std::collections::{HashMap, HashSet};

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
        "Promotes stack-allocated variables to SSA registers"
    }

    fn run_on_function(&self, func: &mut Function) -> Result<PassResult, CompileError> {
        promote_regs(func)
    }
}

/// Slot 信息：同一 offset + type 的所有访问。
struct SlotInfo {
    ty: Type,
    /// (block_idx, inst_idx, stored_value)
    stores: Vec<(usize, usize, Value)>,
    /// (block_idx, inst_idx) — loads to be replaced
    loads: Vec<(usize, usize)>,
    /// load result Values (to be replaced with SSA values)
    load_results: Vec<Value>,
}

#[allow(deprecated)]
pub fn promote_regs(func: &mut Function) -> Result<PassResult, CompileError> {
    let mut result = PassResult::default();
    if func.blocks.is_empty() {
        return Ok(result);
    }

    // 计算 CFG 信息
    let preds = func.predecessors();
    let doms = func.dominator_tree_raw();
    let _dom_tree = func.dominator_tree();

    // 1. 按 (offset, type) 分组所有 StackLoad/StackStore
    let slots = group_stack_accesses(func);
    if slots.is_empty() {
        return Ok(result);
    }

    for slot in &slots {
        if slot.stores.is_empty() || slot.loads.is_empty() {
            continue;
        }

        // 2. 收集 Store 所在块
        let mut store_blocks: HashSet<usize> = HashSet::new();
        for &(bi, _, _) in &slot.stores {
            store_blocks.insert(bi);
        }

        // 3. 计算需要 Phi 的块（store_blocks 的迭代支配边界）
        let mut phi_blocks: HashSet<usize> = HashSet::new();
        let mut worklist: Vec<usize> = store_blocks.iter().copied().collect();
        while let Some(bi) = worklist.pop() {
            let df = compute_df(bi, &doms, &preds);
            for df_bi in df {
                if phi_blocks.insert(df_bi) {
                    worklist.push(df_bi);
                }
            }
        }

        // 4. 如果不需要 Phi（单 store 块支配所有 load），直接替换
        if phi_blocks.is_empty() {
            // 所有 store 在同一条支配链上：直接用 store 值替换 load
            for &(bi, ii, stored_val) in &slot.stores {
                // 将 StackStore 替换为 Copy(stored_val)
                let new_val = func.create_value();
                func.blocks[bi].instructions[ii].opcode = Opcode::Copy;
                func.blocks[bi].instructions[ii].operands = smallvec::smallvec![stored_val];
                func.blocks[bi].instructions[ii].result = Some(new_val);
                func.blocks[bi].instructions[ii].ty = slot.ty;

                // 将所有 StackLoad 替换为 Copy(stored_val)
                for &(li_bi, li_ii) in &slot.loads {
                    func.blocks[li_bi].instructions[li_ii].opcode = Opcode::Copy;
                    func.blocks[li_bi].instructions[li_ii].operands =
                        smallvec::smallvec![stored_val];
                    // 保持 result 不变
                }

                result.changed = true;
                result.values_replaced += slot.loads.len();
                result.instructions_removed += 1;
            }
            continue;
        }

        // 5. 在 phi_blocks 中插入 Phi 指令
        let mut phi_values: HashMap<usize, Value> = HashMap::new();
        for &bi in &phi_blocks {
            let phi_val = func.create_value();
            phi_values.insert(bi, phi_val);
            // 插入 Phi 指令到块开头
            let phi_inst = Instruction::new(
                Opcode::Phi {
                    incoming: smallvec::smallvec![],
                },
                smallvec::smallvec![],
                Some(phi_val),
                slot.ty,
            );
            func.blocks[bi].instructions.insert(0, phi_inst);
        }

        // 6. 创建新的 SSA 值替换每个 store
        let mut store_defs: Vec<(usize, usize, Value)> = Vec::new(); // (bi, ii, new_ssa_val)
        for &(bi, ii, stored_val) in &slot.stores {
            let new_val = func.create_value();
            store_defs.push((bi, ii, new_val));
            // 将 Store 替换为 Copy(stored_val) → result = new_val
            func.blocks[bi].instructions[ii].opcode = Opcode::Copy;
            func.blocks[bi].instructions[ii].operands = smallvec::smallvec![stored_val];
            func.blocks[bi].instructions[ii].result = Some(new_val);
            func.blocks[bi].instructions[ii].ty = slot.ty;
        }

        // 7. 填充 Phi 节点操作数（基于前驱块中的定义）
        for (&bi, &phi_val) in &phi_values {
            let mut phi_operands: Vec<Value> = Vec::new();
            for &pred_bi in &preds[bi] {
                // 找到在此前驱块中活跃的值
                let pred_val =
                    find_reaching_def(BlockId(pred_bi.0), &store_defs, &phi_values, &doms);
                phi_operands.push(pred_val);
            }
            if let Some(phi_inst) = func.blocks[bi]
                .instructions
                .iter_mut()
                .find(|i| i.result == Some(phi_val))
            {
                phi_inst.operands = phi_operands.into_iter().collect();
            }
        }

        // 8. 替换 Load 为对应的 SSA 值
        for (load_idx, &(bi, ii)) in slot.loads.iter().enumerate() {
            if load_idx < slot.load_results.len() {
                let _load_result = slot.load_results[load_idx];
                // 找到到达此 load 的定义
                let reaching =
                    find_reaching_def(BlockId(bi as u32), &store_defs, &phi_values, &doms);
                // 将 Load 替换为 Copy(reaching)
                func.blocks[bi].instructions[ii].opcode = Opcode::Copy;
                func.blocks[bi].instructions[ii].operands = smallvec::smallvec![reaching];
                // 保持 result 不变（以便其他指令引用）
            }
        }

        result.changed = true;
        result.values_replaced += slot.loads.len();
    }

    Ok(result)
}

/// 按 offset + type 分组 StackLoad/StackStore。
///
/// StackStore 使用 `Type::Void`，因此无法直接与 StackLoad 的类型匹配。
/// 解决方案：用 offset 作为初始 key，StackLoad 的类型优先覆盖 Void 类型。
fn group_stack_accesses(func: &Function) -> Vec<SlotInfo> {
    // Phase 1: 用 offset 创建临时分组
    let mut slots_by_offset: HashMap<i32, SlotInfo> = HashMap::new();

    for (bi, block) in func.blocks.iter().enumerate() {
        for (ii, inst) in block.instructions.iter().enumerate() {
            match &inst.opcode {
                Opcode::StackStore { offset } => {
                    let entry = slots_by_offset.entry(*offset).or_insert_with(|| SlotInfo {
                        ty: Type::Void, // will be refined by StackLoad
                        stores: Vec::new(),
                        loads: Vec::new(),
                        load_results: Vec::new(),
                    });
                    if !inst.operands.is_empty() {
                        entry.stores.push((bi, ii, inst.operands[0]));
                    }
                }
                Opcode::StackLoad { offset } => {
                    let entry = slots_by_offset.entry(*offset).or_insert_with(|| SlotInfo {
                        ty: inst.ty,
                        stores: Vec::new(),
                        loads: Vec::new(),
                        load_results: Vec::new(),
                    });
                    // StackLoad 的类型优先（StackStore 类型是 Void）
                    if entry.ty == Type::Void {
                        entry.ty = inst.ty;
                    }
                    entry.loads.push((bi, ii));
                    if let Some(v) = inst.result {
                        entry.load_results.push(v);
                    }
                }
                _ => {}
            }
        }
    }

    slots_by_offset.into_values().collect()
}

/// 计算块 bi 的支配边界。
fn compute_df(bi: usize, doms: &[HashSet<BlockId>], preds: &[Vec<BlockId>]) -> Vec<usize> {
    let mut df = Vec::new();
    let block_id = BlockId(bi as u32);

    for (b, b_preds) in preds.iter().enumerate() {
        let b_id = BlockId(b as u32);
        for &p in b_preds {
            if doms[p.0 as usize].contains(&block_id) && !strictly_dominates(block_id, b_id, doms) {
                df.push(b);
                break;
            }
        }
    }
    df
}

fn strictly_dominates(a: BlockId, b: BlockId, doms: &[HashSet<BlockId>]) -> bool {
    a != b && doms[b.0 as usize].contains(&a)
}

/// 找到在 block 开头处活跃的 SSA 定义。
/// 查找支配 block 的最近定义。
fn find_reaching_def(
    block: BlockId,
    store_defs: &[(usize, usize, Value)],
    phi_values: &HashMap<usize, Value>,
    doms: &[HashSet<BlockId>],
) -> Value {
    // 优先：在 block 中的 phi
    if let Some(&phi_val) = phi_values.get(&(block.0 as usize)) {
        return phi_val;
    }

    // 找到在支配 block 的块中最近的定义
    let mut best: Option<(usize, Value)> = None; // (dom_depth, value)
    for &(bi, _, val) in store_defs {
        let def_block = BlockId(bi as u32);
        if doms[block.0 as usize].contains(&def_block) {
            let depth = doms[def_block.0 as usize].len();
            if best.is_none_or(|(d, _)| depth > d) {
                best = Some((depth, val));
            }
        }
    }

    // 查找在 phi 块中的定义（也在支配块中）
    for (&bi, &val) in phi_values {
        let phi_block = BlockId(bi as u32);
        if doms[block.0 as usize].contains(&phi_block) {
            let depth = doms[phi_block.0 as usize].len();
            if best.is_none_or(|(d, _)| depth > d) {
                best = Some((depth, val));
            }
        }
    }

    best.map(|(_, v)| v).unwrap_or(Value(0))
}

// ============================================================
// 测试
// ============================================================

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ir::FunctionBuilder;

    /// 简单的单块 mem2reg：stack_store + stack_load → Copy
    #[test]
    fn promote_simple_load_store() {
        // fn test() -> i32 {
        //   stack_store 42, fp+0
        //   v = stack_load fp+0, i32
        //   ret v
        // }
        // After: v = copy(42), ret v
        let sig = Signature::new(&[], &[Type::I32]);
        let mut b = FunctionBuilder::new("test", sig);
        let entry = b.create_block();
        b.switch_to_block(entry);
        let c42 = b.iconst_i32(42);
        b.stack_store(0, c42);
        let v = b.stack_load(0, Type::I32);
        b.return_(&[v]);

        let mut func = b.finish();
        // Run constfold first to make Iconst visible
        crate::optimize::ConstFoldPass::new()
            .run_on_function(&mut func)
            .unwrap();
        let pass = Mem2RegPass::new();
        let r = pass.run_on_function(&mut func).unwrap();
        // Simple single-block case should now be promoted (no longer skipped)
        assert!(r.changed, "Simple stack load/store should be promoted");
        assert!(func.validate().is_valid());
    }

    #[test]
    fn mem2reg_preserves_validation() {
        let sig = Signature::new(&[(Type::I32, "x")], &[Type::I32]);
        let mut b = FunctionBuilder::new("test", sig);
        let (entry, params) = b.create_block_with_params(&[(Type::I32, "x")]);
        b.switch_to_block(entry);
        let x = params[0];
        b.stack_store(8, x);
        let v = b.stack_load(8, Type::I32);
        b.return_(&[v]);

        let mut func = b.finish();
        let pass = Mem2RegPass::new();
        pass.run_on_function(&mut func).unwrap();
        assert!(func.validate().is_valid());
    }
}
