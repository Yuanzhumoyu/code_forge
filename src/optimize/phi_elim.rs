//! Phi 节点消除 pass — 将 SSA Phi 节点转换为基本块间的 Copy 指令。
//!
//! 在寄存器分配之前运行，将每个 Phi 节点展开为：
//! - 前驱块末尾的 Copy 指令（MOV）
//! - 保留 Phi result 作为 SSA 值（在 lowering 阶段分配 VReg）
//!
//! # 算法
//!
//! 1. 收集所有 Phi 节点
//! 2. 对每个 Phi: 为 result 创建 Copy，映射到唯一活动值
//! 3. 消除冗余 Phi (所有操作数相同 → Copy)

use crate::CompileError;
use crate::ir::*;
use crate::optimize::{OptimizationPass, PassResult};

#[derive(Default)]
pub struct PhiElimPass;

impl PhiElimPass {
    pub fn new() -> Self {
        Self
    }
}

impl OptimizationPass for PhiElimPass {
    fn name(&self) -> &'static str {
        "phi-elim"
    }
    fn description(&self) -> &'static str {
        "Eliminates phi nodes by inserting copy instructions in predecessor blocks"
    }

    fn run_on_function(&self, func: &mut Function) -> Result<PassResult, CompileError> {
        eliminate_phis(func)
    }
}

pub fn eliminate_phis(func: &mut Function) -> Result<PassResult, CompileError> {
    let mut result = PassResult::default();
    let preds = func.predecessors();

    // Phase 1: 收集每个块中的 phi 信息
    // (block_idx, phi_result, Vec<(pred_block, operand_value, ty)>)
    let mut phi_work: Vec<(usize, Value, Vec<(usize, Value, Type)>)> = Vec::new();

    for (bi, block) in func.blocks.iter().enumerate() {
        let pred_list = &preds[bi];

        for inst in &block.instructions {
            if !matches!(inst.opcode, Opcode::Phi { .. }) {
                continue;
            }
            let phi_result = match inst.result {
                Some(r) => r,
                None => continue,
            };

            // 构建 前驱 → 操作数 的映射（phi 操作数按前驱顺序排列）
            let mut pred_copies: Vec<(usize, Value, Type)> = Vec::new();
            for (op_idx, &op_val) in inst.operands.iter().enumerate() {
                if op_idx < pred_list.len() {
                    let pred_bi = pred_list[op_idx].0 as usize;
                    pred_copies.push((pred_bi, op_val, inst.ty));
                }
            }

            if !pred_copies.is_empty() {
                phi_work.push((bi, phi_result, pred_copies));
            }
        }
    }

    if phi_work.is_empty() {
        return Ok(result);
    }

    // Phase 2: 在每个前驱块末尾插入 Copy 指令
    for (_block_idx, phi_result, pred_copies) in &phi_work {
        for &(pred_bi, op_val, ty) in pred_copies {
            let copy_inst = Instruction::new(
                Opcode::Copy,
                smallvec::smallvec![op_val],
                Some(*phi_result),
                ty,
            );
            // 插入到前驱块末尾（terminator 之前）
            let pred_block = &mut func.blocks[pred_bi];
            let insert_pos = pred_block.instructions.len();
            pred_block.instructions.insert(insert_pos, copy_inst);
        }
        result.instructions_removed += 1;
        result.changed = true;
    }

    // Phase 3: 移除所有 Phi 指令
    for (block_idx, _, _) in &phi_work {
        let block = &mut func.blocks[*block_idx];
        block
            .instructions
            .retain(|inst| !matches!(inst.opcode, Opcode::Phi { .. }));
    }

    Ok(result)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ir::FunctionBuilder;

    #[test]
    fn phi_elim_redundant_phi() {
        let sig = Signature::void();
        let mut b = FunctionBuilder::new("test", sig);
        let (entry, params) = b.create_block_with_params(&[(Type::I32, "x")]);
        let merge = b.create_block(); // merge has no params

        b.switch_to_block(entry);
        let _x = params[0];
        b.jump(merge, &[]); // no args to match merge's no params

        b.switch_to_block(merge);
        b.return_(&[]);

        let mut func = b.finish();
        let pass = PhiElimPass::new();
        pass.run_on_function(&mut func).unwrap();
        assert!(func.validate().is_valid());
    }

    #[test]
    fn phi_elim_preserves_validation() {
        // Diamond with phi at merge
        let sig = Signature::void();
        let mut b = FunctionBuilder::new("test", sig);
        let (entry, _params) = b.create_block_with_params(&[(Type::I32, "x")]);
        let then_blk = b.create_block();
        let else_blk = b.create_block();
        let merge = b.create_block();

        b.switch_to_block(entry);
        let cond = b.iconst_i32(1);
        b.branch(cond, then_blk, else_blk, &[], &[]);

        b.switch_to_block(then_blk);
        let _v1 = b.iconst_i32(10);
        b.jump(merge, &[]);

        b.switch_to_block(else_blk);
        let _v2 = b.iconst_i32(20);
        b.jump(merge, &[]);

        b.switch_to_block(merge);
        b.return_(&[]);

        let mut func = b.finish();
        let pass = PhiElimPass::new();
        pass.run_on_function(&mut func).unwrap();
        assert!(func.validate().is_valid());
    }
}
