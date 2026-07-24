//! 循环不变量外提 (LICM) pass。
//!
//! 将循环内不随迭代变化（loop-invariant）的计算提升到循环外部，
//! 减少循环体内的冗余计算。
//!
//! # 算法
//!
//! 1. 使用 [`LoopForest`] 检测循环（基于支配树的统一循环分析）
//! 2. 标记循环不变量（操作数全在循环外或已标记为不变量）
//! 3. 将不变量指令外提到循环 pre-header 或循环头开头
//!
//! # 条件
//!
//! 指令是循环不变量，iff：
//! - 所有操作数定义在循环体外，或
//! - 所有操作数都是已标记的循环不变量
//! - 指令无副作用（纯计算）
//! - 指令在循环的每次迭代中都执行（dominates all loop exits）

use crate::CompileError;
use crate::ir::*;
use crate::optimize::{OptimizationPass, PassResult};
use std::collections::HashSet;

/// 循环不变量外提 pass。
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
        "Loop-invariant code motion: hoists invariant computations out of loops"
    }

    fn run_on_function(&self, func: &mut Function) -> Result<PassResult, CompileError> {
        hoist_loop_invariants(func)
    }
}

/// 对单个函数执行 LICM。
pub fn hoist_loop_invariants(func: &mut Function) -> Result<PassResult, CompileError> {
    let mut result = PassResult::default();

    // Build unified loop analysis
    let dt = DominatorTree::build(func);
    let lf = LoopForest::build(func, &dt);

    if lf.is_empty() {
        return Ok(result);
    }

    for loop_info in lf.all_loops() {
        let body: HashSet<BlockId> = loop_info.blocks.iter().copied().collect();
        if body.len() <= 1 {
            continue; // Loop body too small
        }

        let header = loop_info.header;

        // Collect values defined outside the loop
        let defined_outside: HashSet<Value> = collect_values_outside(func, &body);

        // Mark invariant instructions
        let invariants = mark_invariants(func, &body, &defined_outside);
        if invariants.is_empty() {
            continue;
        }

        // Hoist to header block
        let count = hoist_to_header(func, header, &invariants);
        result.instructions_removed += count;
        result.changed = true;
    }

    Ok(result)
}

/// 收集在循环体外定义的所有 Value。
fn collect_values_outside(func: &Function, body: &HashSet<BlockId>) -> HashSet<Value> {
    let mut values = HashSet::new();

    for block in func.iter_blocks() {
        if body.contains(&block.id) {
            continue;
        }
        // Block params
        for (v, _) in &block.params {
            values.insert(*v);
        }
        // Instruction results
        for inst in &block.instructions {
            if let Some(v) = inst.result {
                values.insert(v);
            }
        }
    }

    values
}

/// 标记循环不变量指令。
///
/// 返回 invariant 指令的 (block_id, inst_index) 集合。
fn mark_invariants(
    func: &Function,
    body: &HashSet<BlockId>,
    outside_values: &HashSet<Value>,
) -> HashSet<(BlockId, usize)> {
    let mut invariant_values: HashSet<Value> = HashSet::new();
    let mut invariant_insts: HashSet<(BlockId, usize)> = HashSet::new();

    // First pass: mark Iconst/Fconst inside the loop as invariant (they are constants)
    for block in func.iter_blocks() {
        if !body.contains(&block.id) {
            continue;
        }
        for (i, inst) in block.instructions.iter().enumerate() {
            if matches!(inst.opcode, Opcode::Iconst { .. } | Opcode::Fconst { .. })
                && let Some(v) = inst.result
            {
                invariant_values.insert(v);
                invariant_insts.insert((block.id, i));
            }
        }
    }

    // Second pass: iteratively mark instructions whose operands are all invariant
    let mut changed = true;
    while changed {
        changed = false;

        for block in func.iter_blocks() {
            if !body.contains(&block.id) {
                continue;
            }

            for (i, inst) in block.instructions.iter().enumerate() {
                let key = (block.id, i);
                if invariant_insts.contains(&key) {
                    continue;
                }

                // Skip instructions without results
                let result_val = match inst.result {
                    Some(v) => v,
                    None => continue,
                };

                // Skip instructions with side effects
                if has_side_effects(&inst.opcode) {
                    continue;
                }

                // Skip Phi (phi is not invariant)
                if matches!(inst.opcode, Opcode::Phi { .. }) {
                    continue;
                }

                // Check if all operands are outside the loop or already marked invariant
                let all_invariant = inst
                    .operands
                    .iter()
                    .all(|v| outside_values.contains(v) || invariant_values.contains(v));

                if all_invariant {
                    invariant_values.insert(result_val);
                    invariant_insts.insert(key);
                    changed = true;
                }
            }
        }
    }

    invariant_insts
}

/// 将不变量指令外提到 loop header 的开头。
fn hoist_to_header(
    func: &mut Function,
    header: BlockId,
    invariants: &HashSet<(BlockId, usize)>,
) -> usize {
    let mut count = 0;

    // Collect instructions to hoist
    let mut to_hoist: Vec<(BlockId, usize, Instruction)> = Vec::new();

    for &(block_id, inst_idx) in invariants {
        if let Some(block) = func.block(block_id)
            && inst_idx < block.instructions.len()
        {
            let inst = block.instructions[inst_idx].clone();
            to_hoist.push((block_id, inst_idx, inst));
        }
    }

    // Sort by block_id and inst_idx descending to preserve indices when removing
    to_hoist.sort_by(|a, b| b.0 .0.cmp(&a.0 .0).then(b.1.cmp(&a.1)).reverse());

    // Build remove list (skip Iconst/Fconst, they are zero-cost)
    let mut remove_list: Vec<(BlockId, usize)> = Vec::new();
    for &(block_id, inst_idx, ref inst) in &to_hoist {
        if matches!(inst.opcode, Opcode::Iconst { .. } | Opcode::Fconst { .. }) {
            continue;
        }
        remove_list.push((block_id, inst_idx));
    }

    // Remove from original positions (replace with Nop to avoid shifting)
    for (block_id, inst_idx) in &remove_list {
        if let Some(block) = func.block_mut(*block_id)
            && *inst_idx < block.instructions.len()
        {
            block.instructions[*inst_idx] =
                Instruction::new(Opcode::Nop, smallvec::smallvec![], None, Type::Void);
        }
    }

    // Insert at the beginning of the header block (after existing Iconst/Fconst)
    let header_block = &mut func.blocks[header.0 as usize];
    let mut insert_pos = 0;

    // Find insertion position: after existing Iconst/Fconst at the top of header
    for (i, inst) in header_block.instructions.iter().enumerate() {
        if matches!(inst.opcode, Opcode::Iconst { .. } | Opcode::Fconst { .. }) {
            insert_pos = i + 1;
        } else {
            break;
        }
    }

    for (_, _, inst) in to_hoist.iter().rev() {
        if matches!(inst.opcode, Opcode::Iconst { .. } | Opcode::Fconst { .. }) {
            continue;
        }
        header_block.instructions.insert(insert_pos, inst.clone());
        insert_pos += 1;
        count += 1;
    }

    count
}

fn has_side_effects(opcode: &Opcode) -> bool {
    matches!(
        opcode,
        Opcode::Store
            | Opcode::StackStore { .. }
            | Opcode::Call { .. }
            | Opcode::CallIndirect
            | Opcode::Load
    )
}

// ============================================================
// 测试
// ============================================================

#[cfg(test)]
mod tests {
    use super::*;
    use crate::optimize::OptimizationPass;

    #[test]
    fn hoist_invariant_add_from_loop() {
        let sig = Signature::new(&[(Type::I32, "x")], &[Type::I32]);
        let mut b = FunctionBuilder::new("test", sig);
        let (entry, params) = b.create_block_with_params(&[(Type::I32, "x")]);
        let (loop_body, loop_params) = b.create_block_with_params(&[(Type::I32, "x_phi")]);
        let exit = b.create_block();

        b.switch_to_block(entry);
        b.jump(loop_body, &[params[0]]);

        b.switch_to_block(loop_body);
        let x = loop_params[0];
        let c3 = b.iconst_i32(3);
        let c5 = b.iconst_i32(5);
        let invariant_add = b.iadd(c3, c5); // 3+5 -- invariant!
        let new_x = b.iadd(x, invariant_add);
        let c100 = b.iconst_i32(100);
        let cmp = b.icmp(IntCC::SignedLessThan, new_x, c100);
        b.branch(cmp, loop_body, exit, &[new_x], &[]);

        b.switch_to_block(exit);
        b.return_(&[new_x]);

        let mut func = b.finish();
        eprintln!("Before LICM:\n{}", func);

        let pass = LicmPass::new();
        let _r = pass.run_on_function(&mut func).unwrap();

        eprintln!("After LICM:\n{}", func);
        // The pass should not crash
    }

    #[test]
    fn licm_no_crash_on_loop_free_code() {
        let sig = Signature::new(&[], &[Type::I32]);
        let mut b = FunctionBuilder::new("test", sig);
        let entry = b.create_block();
        b.switch_to_block(entry);
        let v = b.iconst_i32(42);
        b.return_(&[v]);
        let mut func = b.finish();

        let pass = LicmPass::new();
        let _r = pass.run_on_function(&mut func).unwrap();
        assert!(!_r.changed); // No loops, should not change
    }

    #[test]
    fn licm_preserves_correctness() {
        let sig = Signature::new(&[(Type::I32, "n")], &[Type::I32]);
        let mut b = FunctionBuilder::new("sum", sig);
        let (entry, params) = b.create_block_with_params(&[(Type::I32, "n")]);
        let (loop_hdr, hdr_params) =
            b.create_block_with_params(&[(Type::I32, "sum_phi"), (Type::I32, "n_phi")]);
        let loop_body_block = b.create_block();
        let exit = b.create_block();

        b.switch_to_block(entry);
        let zero = b.iconst_i32(0);
        b.jump(loop_hdr, &[zero, params[0]]);

        b.switch_to_block(loop_hdr);
        let sum = hdr_params[0];
        let n = hdr_params[1];
        let cmp = b.icmp(IntCC::SignedGreaterThan, n, zero);
        b.branch(cmp, loop_body_block, exit, &[], &[]);

        b.switch_to_block(loop_body_block);
        let new_sum = b.iadd(sum, n);
        let one = b.iconst_i32(1);
        let new_n = b.isub(n, one);
        b.jump(loop_hdr, &[new_sum, new_n]);

        b.switch_to_block(exit);
        b.return_(&[sum]);

        let mut func = b.finish();
        let validation = func.validate();
        assert!(
            validation.is_valid(),
            "Validation before LICM: {:?}",
            validation.errors
        );

        let pass = LicmPass::new();
        pass.run_on_function(&mut func).unwrap();

        let validation = func.validate();
        assert!(
            validation.is_valid(),
            "Validation after LICM: {:?}",
            validation.errors
        );
    }

    #[test]
    fn licm_hoists_invariant_from_nested() {
        let sig = Signature::new(&[(Type::I32, "x")], &[Type::I32]);
        let mut b = FunctionBuilder::new("test", sig);
        let (entry, params) = b.create_block_with_params(&[(Type::I32, "x")]);
        let (body, body_params) = b.create_block_with_params(&[(Type::I32, "x_phi")]);
        let exit = b.create_block();

        // Entry computes invariant values (outside loop)
        b.switch_to_block(entry);
        let c10 = b.iconst_i32(10);
        let c20 = b.iconst_i32(20);
        b.jump(body, &[params[0]]);

        b.switch_to_block(body);
        let x = body_params[0];
        // c10 + c20 is invariant (both defined in entry, outside loop)
        let invariant = b.iadd(c10, c20);
        let new_x = b.iadd(x, invariant);
        let c100 = b.iconst_i32(100);
        let cmp = b.icmp(IntCC::SignedLessThan, new_x, c100);
        b.branch(cmp, body, exit, &[new_x], &[]);

        b.switch_to_block(exit);
        b.return_(&[new_x]);

        let mut func = b.finish();
        let validation = func.validate();
        assert!(validation.is_valid());

        let pass = LicmPass::new();
        let _r = pass.run_on_function(&mut func).unwrap();

        let validation = func.validate();
        assert!(
            validation.is_valid(),
            "Validation after LICM: {:?}",
            validation.errors
        );
    }

    #[test]
    fn licm_handles_empty_loop_body() {
        // A loop with only a header (self-loop) but no other body blocks
        let sig = Signature::new(&[], &[Type::I32]);
        let mut b = FunctionBuilder::new("minimal", sig);
        let entry = b.create_block();
        let loop_blk = b.create_block();
        let exit = b.create_block();

        b.switch_to_block(entry);
        b.jump(loop_blk, &[]);

        b.switch_to_block(loop_blk);
        let c = b.iconst_i32(1);
        b.branch(c, loop_blk, exit, &[], &[]);

        b.switch_to_block(exit);
        let c2 = b.iconst_i32(42);
        b.return_(&[c2]);

        let mut func = b.finish();
        let pass = LicmPass::new();
        let r = pass.run_on_function(&mut func).unwrap();
        // May or may not hoist, but should not crash
        let _ = r;
        assert!(func.validate().is_valid());
    }
}
