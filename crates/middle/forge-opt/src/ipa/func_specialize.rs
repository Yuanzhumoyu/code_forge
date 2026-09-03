//! 函数特化 (Function Specialization) pass。
//!
//! 识别 Call 指令中对 const 函数的常量参数调用，创建特化版本。
//! 特化版本中常量参数被替换为具体值，允许后续常量折叠消除冗余计算。
//!
//! # 算法 (v2)
//!
//! 1. 扫描 Call 指令，匹配 const_functions 中的被调用函数
//! 2. 检查 call arguments: 是否有至少一个操作数是常量
//! 3. 若满足条件: 将 callee body 克隆到 caller 的 call site
//! 4. 在克隆过程中，将对应 const 参数的 param 替换为 iconst
//! 5. 将 call results 替换为克隆体的 return values
//! 6. 移除 Call 指令

use crate::{OptimizationPass, PassResult};
use forge_ir::IrError;
use forge_ir::*;
use std::collections::HashMap;

#[doc(hidden)]
pub struct FuncSpecializePass {
    const_functions: HashMap<FuncRef, Function>,
}

impl FuncSpecializePass {
    pub fn new(const_functions: HashMap<FuncRef, Function>) -> Self {
        Self { const_functions }
    }
}

impl OptimizationPass for FuncSpecializePass {
    fn name(&self) -> &'static str {
        "func-specialize"
    }
    fn description(&self) -> &'static str {
        "Specializes const functions with constant arguments by cloning and substituting at call site"
    }
    fn run_on_function(&self, func: &mut Function) -> Result<PassResult, IrError> {
        specialize_calls(func, &self.const_functions)
    }
}

/// 对函数中的 Call 指令执行特化。
pub fn specialize_calls(
    func: &mut Function,
    const_functions: &HashMap<FuncRef, Function>,
) -> Result<PassResult, IrError> {
    let mut result = PassResult::default();

    // Collect all Call instructions
    #[allow(clippy::type_complexity)]
    let mut calls: Vec<(usize, Inst, FuncRef, Vec<(usize, Value)>)> = Vec::new();
    for bi in 0..func.dfg.blocks.len() {
        let block_data = &func.dfg.blocks[bi];
        for &inst_id in &block_data.inst_order {
            let inst = &func.dfg.insts[inst_id.0 as usize];
            if inst.opcode != Opcode::Call {
                continue;
            }
            // Find callee FuncRef
            let callee_ref = inst.immediates.iter().find_map(|i| i.as_func());
            let Some(callee) = callee_ref else { continue };
            let Some(_callee_func) = const_functions.get(&callee) else {
                continue;
            };

            // Check which arguments are constants
            let mut const_args: Vec<(usize, Value)> = Vec::new();
            for (i, &operand) in inst.operands.iter().enumerate() {
                let def = func.dfg.value_def(operand).copied();
                if let Some(ValueDef::Inst(def_inst, _)) = def {
                    let def_data = &func.dfg.insts[def_inst.0 as usize];
                    if matches!(def_data.opcode, Opcode::Iconst | Opcode::Fconst) {
                        const_args.push((i, operand));
                    }
                }
            }

            if !const_args.is_empty() {
                calls.push((bi, inst_id, callee, const_args));
            }
        }
    }

    // Process each call
    for (_block_idx, call_inst, callee_ref, _const_args) in calls {
        let Some(callee_func) = const_functions.get(&callee_ref) else {
            continue;
        };

        // Only specialize single-block functions for now
        let callee_body = &callee_func.dfg.blocks;
        if callee_body.len() != 1 && callee_body.is_empty() {
            continue;
        }

        let call_block = func.dfg.insts[call_inst.0 as usize].block;
        let call_results: Vec<Value> = func.dfg.insts[call_inst.0 as usize]
            .results
            .iter()
            .copied()
            .collect();
        let call_operands: Vec<Value> = func.dfg.insts[call_inst.0 as usize]
            .operands
            .iter()
            .copied()
            .collect();

        // Clone callee body into caller at call site
        let ret_vals = clone_callee_into_caller(func, callee_func, call_block, &call_operands)?;

        // Replace call results with return values（DFG + use-lists 双更新）
        for (i, &call_result) in call_results.iter().enumerate() {
            if let Some(&ret_val) = ret_vals.get(i) {
                func.replace_all_uses(call_result, ret_val);
                result.values_replaced += 1;
            }
        }

        // Remove the Call instruction（原子：use-lists + 墓碑化）
        func.kill_inst(call_inst);
        result.instructions_removed += 1;
        result.changed = true;
    }

    Ok(result)
}

/// Clone a callee function's body into the caller at a specific block.
/// Returns the values that correspond to the callee's Return.
fn clone_callee_into_caller(
    caller: &mut Function,
    callee: &Function,
    target_block: Block,
    call_args: &[Value],
) -> Result<Vec<Value>, IrError> {
    let mut val_remap: HashMap<Value, Value> = HashMap::new();

    // Map callee params to call arguments
    for callee_block in callee.dfg.blocks.iter() {
        for (pv, arg) in callee_block.param_values.iter().zip(call_args.iter()) {
            val_remap.insert(*pv, *arg);
        }
    }

    // Clone instructions from callee into caller
    let mut ret_vals: Vec<Value> = Vec::new();
    for bi in 0..callee.dfg.blocks.len() {
        let callee_block = &callee.dfg.blocks[bi];

        for &inst_id in &callee_block.inst_order {
            let inst = &callee.dfg.insts[inst_id.0 as usize];

            // Remap operands
            let new_operands: smallvec::SmallVec<[Value; 4]> = inst
                .operands
                .iter()
                .map(|v| val_remap.get(v).copied().unwrap_or(*v))
                .collect();

            // Clone immediates（remap_from 按 tag 全池重建，替代手写三级 fallback）
            let new_immediates: smallvec::SmallVec<[Immediate; 4]> = inst
                .immediates
                .iter()
                .map(|im| match im {
                    Immediate::Const(cid) => {
                        Immediate::Const(caller.constants.remap_from(&callee.constants, *cid))
                    }
                    _ => *im,
                })
                .collect();

            // Get result types
            let result_tys: Vec<TypeId> = inst
                .results
                .iter()
                .map(|&v| callee.dfg.values[v.0 as usize].ty)
                .collect();

            // Skip terminators (Return handled separately)
            if matches!(
                inst.opcode,
                Opcode::Iconst | Opcode::Fconst | Opcode::StackAddr
            ) {
                // Let these pass through to create new values
            }

            // 保留全字段（flags/mem_flags/metadata/loc/isel_strategy）
            let new_inst = caller.dfg.make_inst_with_meta_and_loc(
                inst.opcode,
                target_block,
                new_operands,
                new_immediates,
                &result_tys,
                inst.flags,
                inst.mem_flags,
                inst.metadata.clone(),
                inst.loc.clone(),
            );
            if let Some(strategy) = inst.isel_strategy {
                caller.dfg.insts[new_inst.0 as usize].isel_strategy = Some(strategy);
            }

            // Map old results to new results
            let new_results = &caller.dfg.insts[new_inst.0 as usize].results;
            for (i, &old_r) in inst.results.iter().enumerate() {
                if i < new_results.len() {
                    val_remap.insert(old_r, new_results[i]);
                }
            }
        }

        // Handle callee's Return terminator
        if let Terminator::Return { values, .. } = &callee_block.terminator {
            for &v in values.iter() {
                ret_vals.push(val_remap.get(&v).copied().unwrap_or(v));
            }
        }
    }

    Ok(ret_vals)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn func_specialize_no_crash() {
        let sig = FunctionSignature::new(&[(TypeId::I32, "x")], &[TypeId::I32]);
        let mut b = FunctionBuilder::new("mul2", TypeContext::new(), sig);
        let (entry, params) = b.create_block_with_params(&[(TypeId::I32, "x")]);
        b.switch_to_block(entry);
        let two = b.iconst_i32(2);
        let prod = b.imul(params[0], two);
        b.ret(&[prod]);
        let mut cf = b.finish().expect("build");
        cf.is_const = true;

        let mut func_table = HashMap::new();
        func_table.insert(FuncRef(0), cf);

        let sig2 = FunctionSignature::new(&[], &[]);
        let mut b2 = FunctionBuilder::new("caller", TypeContext::new(), sig2);
        let entry2 = b2.create_block();
        b2.switch_to_block(entry2);
        b2.ret(&[]);
        let mut caller = b2.finish().expect("build");

        let pass = FuncSpecializePass::new(func_table);
        let r = pass.run_on_function(&mut caller);
        assert!(r.is_ok());
    }

    #[test]
    fn func_specialize_const_arg_call() {
        // Create a const function: mul2(x) = x * 2
        let sig = FunctionSignature::new(&[(TypeId::I32, "x")], &[TypeId::I32]);
        let mut b = FunctionBuilder::new("mul2", TypeContext::new(), sig);
        let (entry, params) = b.create_block_with_params(&[(TypeId::I32, "x")]);
        b.switch_to_block(entry);
        let two = b.iconst_i32(2);
        let prod = b.imul(params[0], two);
        b.ret(&[prod]);
        let cf = b.finish().expect("build");
        let callee_ref = FuncRef(0);

        let mut func_table = HashMap::new();
        func_table.insert(callee_ref, cf);

        // Create caller: call mul2(5)
        let sig2 = FunctionSignature::new(&[], &[TypeId::I32]);
        let mut b2 = FunctionBuilder::new("caller", TypeContext::new(), sig2);
        let entry2 = b2.create_block();
        b2.switch_to_block(entry2);
        let five = b2.iconst_i32(5);
        let result = b2.call(callee_ref, &[five], &[TypeId::I32]);
        b2.ret(&[result[0]]);
        let mut caller = b2.finish().expect("build");

        let pass = FuncSpecializePass::new(func_table);
        let r = pass.run_on_function(&mut caller).unwrap();
        assert!(r.changed, "Should specialize const arg call");
    }
}
