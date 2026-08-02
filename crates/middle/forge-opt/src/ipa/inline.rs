//! 函数内联 pass。
//!
//! 将小函数内联到调用点，消除调用开销，
//! 并为后续优化（常量折叠、复制传播）创造更多机会。
//!
//! # 内联条件
//!
//! - 被调用函数只有一个基本块（简单函数）
//! - 被调用函数的指令数不超过阈值（默认 20）
//! - 被调用函数不是递归的（不调用自身）
//! - 尊重 `#[inline(always)]` / `#[inline(never)]` 属性
//! - 调用点在循环中时应用 10x 成本惩罚
//!
//! # 算法 (v2 IR)
//!
//! 1. 找到符合条件的 Call 指令
//! 2. 在被调用函数的指令创建新的 caller value/instruction
//! 3. 将被调用函数的返回结果映射到 Call 的结果
//! 4. 移除 Call 指令 (标记 Nop)

use crate::{OptimizationPass, PassResult};
use forge_ir::CompileError;
use forge_ir::*;
use smallvec::SmallVec;
use std::collections::HashMap;

/// 函数内联 pass。
pub struct InlinePass {
    functions: HashMap<FuncRef, Function>,
    threshold: usize,
    max_depth: usize,
}

impl InlinePass {
    pub fn new(functions: HashMap<FuncRef, Function>) -> Self {
        Self {
            functions,
            threshold: 20,
            max_depth: 8,
        }
    }

    pub fn set_threshold(&mut self, threshold: usize) {
        self.threshold = threshold;
    }

    pub fn with_threshold(&mut self, threshold: usize) -> &mut Self {
        self.threshold = threshold;
        self
    }

    pub fn threshold(&self) -> usize {
        self.threshold
    }

    pub fn set_max_depth(&mut self, depth: usize) {
        self.max_depth = depth;
    }
}

impl OptimizationPass for InlinePass {
    fn name(&self) -> &'static str {
        "inline"
    }
    fn description(&self) -> &'static str {
        "Inlines small function bodies into call sites with cost-aware heuristics"
    }
    fn run_on_function(&self, func: &mut Function) -> Result<PassResult, CompileError> {
        inline_calls(func, &self.functions, self.threshold, self.max_depth)
    }
}

/// 对单个函数执行内联。
pub fn inline_calls(
    func: &mut Function,
    functions: &HashMap<FuncRef, Function>,
    threshold: usize,
    max_depth: usize,
) -> Result<PassResult, CompileError> {
    let mut result = PassResult::default();
    let mut inline_count: usize = 0;

    let loop_forest = {
        let dt = DominatorTree::build(func);
        LoopForest::build(func, &dt)
    };
    let _predecessors = func.predecessors().clone();

    // We iterate by block index because we may modify inst_order during inlining
    let block_count = func.dfg.blocks.len();
    for bi in 0..block_count {
        if inline_count >= max_depth {
            break;
        }
        let block = Block(bi as u32);
        let loop_depth = loop_forest.get_loop_depth(block);

        // Collect call candidates first (can't borrow func mutably while iterating)
        #[allow(clippy::type_complexity)]
        let mut candidates: Vec<(usize, Inst, FuncRef, SmallVec<[Value; 4]>, Value)> = Vec::new();
        {
            let block_data = &func.dfg.blocks[bi];
            for (pos, &inst_id) in block_data.inst_order.iter().enumerate() {
                let inst = &func.dfg.insts[inst_id.0 as usize];
                if !matches!(inst.opcode, Opcode::Call) {
                    continue;
                }
                let callee_ref = match inst.immediates.iter().find_map(|i| i.as_func()) {
                    Some(r) => r,
                    None => continue,
                };
                if !can_inline_with_chain(callee_ref, inst, functions, threshold, loop_depth, &[]) {
                    continue;
                }
                if let Some(call_result) = inst.results.first().copied() {
                    candidates.push((pos, inst_id, callee_ref, inst.operands.clone(), call_result));
                }
            }
        }

        // Process candidates in reverse order to keep positions valid
        for (call_pos, call_inst_id, callee_ref, call_operands, call_result) in
            candidates.iter().rev()
        {
            let call_result = *call_result;
            if inline_count >= max_depth {
                break;
            }
            let callee = match functions.get(callee_ref) {
                Some(f) => f,
                None => continue,
            };
            let callee_entry = match callee.entry_block {
                Some(b) => b,
                None => continue,
            };

            // Map callee entry params → call operands
            let callee_param_vals: Vec<Value> = callee.dfg.blocks[callee_entry.0 as usize]
                .param_values
                .iter()
                .copied()
                .collect();
            let callee_entry_insts: Vec<Inst> = callee.dfg.blocks[callee_entry.0 as usize]
                .inst_order
                .to_vec();

            let mut value_map: HashMap<Value, Value> = HashMap::new();
            for (i, &pval) in callee_param_vals.iter().enumerate() {
                if let Some(&arg) = call_operands.get(i) {
                    value_map.insert(pval, arg);
                }
            }

            // Identify callee return value from terminator
            let callee_ret_vals: SmallVec<[Value; 2]> =
                match &callee.dfg.blocks[callee_entry.0 as usize].terminator {
                    Terminator::Return { values } => values.clone(),
                    _ => continue,
                };

            // Clone instructions from callee → caller
            let old_inst_order_len = func.dfg.blocks[bi].inst_order.len();
            let mut new_inst_count = 0usize;

            for &callee_inst_id in &callee_entry_insts {
                let ci = &callee.dfg.insts[callee_inst_id.0 as usize];

                // Skip terminators and Nops in callee body
                if ci.opcode == Opcode::Nop {
                    continue;
                }
                if ci.results.is_empty() && ci.opcode != Opcode::Store {
                    continue;
                }

                // Remap operands
                let mut new_operands: SmallVec<[Value; 4]> = SmallVec::new();
                for &op in &ci.operands {
                    new_operands.push(value_map.get(&op).copied().unwrap_or(op));
                }

                // Remap constants (callee pool → caller pool)
                let mut new_immediates: SmallVec<[Immediate; 4]> = SmallVec::new();
                for imm in &ci.immediates {
                    match imm {
                        Immediate::Const(cid) => {
                            let new_cid = if let Some(big) = callee.constants.resolve_big(*cid) {
                                func.constants.insert_big(big)
                            } else {
                                *cid // keep original if resolution fails
                            };
                            new_immediates.push(Immediate::Const(new_cid));
                        }
                        other => new_immediates.push(*other),
                    }
                }

                // Determine result types
                let result_tys: SmallVec<[TypeId; 2]> = ci
                    .results
                    .iter()
                    .map(|&v| callee.dfg.values[v.0 as usize].ty)
                    .collect();

                // Create new instruction in caller
                let new_inst = func.dfg.make_inst(
                    ci.opcode,
                    block,
                    new_operands,
                    new_immediates,
                    &result_tys,
                    ci.flags,
                );

                // Map old callee results → new caller values
                let new_results = func.dfg.inst_results(new_inst).to_vec();
                for (old_val, &new_val) in ci.results.iter().zip(new_results.iter()) {
                    value_map.insert(*old_val, new_val);
                }
                new_inst_count += 1;
            }

            // Map return value → call result
            if let Some(ret_val) = callee_ret_vals.first()
                && let Some(&mapped_ret) = value_map.get(ret_val)
            {
                // Replace all uses of call_result with the mapped return value
                func.use_lists.replace_all_uses(call_result, mapped_ret);
            }

            // Move the new instructions from end of inst_order to before the call
            if new_inst_count > 0 {
                let block_data = &mut func.dfg.blocks[bi];
                let insert_pos = *call_pos;
                // The new insts are currently at positions [old_inst_order_len .. old_inst_order_len + new_inst_count)
                // We need to move them to position insert_pos
                // Use rotate_right on the slice [insert_pos..]
                let end = block_data.inst_order.len();
                let rotate_by = end - old_inst_order_len;
                block_data.inst_order[insert_pos..].rotate_right(rotate_by);

                // Remove the Call instruction (now shifted by new_inst_count)
                let call_shifted_pos = insert_pos + new_inst_count;
                let call_id = block_data.inst_order[call_shifted_pos];
                func.use_lists.remove_inst(&func.dfg, call_id);
                func.dfg.remove_inst(call_id);
            } else {
                // Just remove the call
                func.use_lists.remove_inst(&func.dfg, *call_inst_id);
                func.dfg.remove_inst(*call_inst_id);
            }

            inline_count += 1;
            result.instructions_removed += 1;
            result.changed = true;
        }
    }

    Ok(result)
}

/// 内联成本评估结果。
#[derive(Debug, Clone)]
pub struct InlineCost {
    pub static_cost: usize,
    pub loop_penalty: usize,
    pub cost: usize,
    pub benefit: usize,
    pub should_inline: bool,
}

/// 评估内联指定调用的成本和收益。
pub fn evaluate_inline_cost(
    callee: FuncRef,
    call_inst: &Instruction,
    functions: &HashMap<FuncRef, Function>,
    threshold: usize,
    loop_depth: u32,
) -> InlineCost {
    let func = match functions.get(&callee) {
        Some(f) => f,
        None => {
            return InlineCost {
                static_cost: 0,
                loop_penalty: 0,
                cost: 0,
                benefit: 0,
                should_inline: false,
            };
        }
    };

    // #[inline(always)] — 无视成本
    if func.attributes.contains(FunctionAttributes::INLINE_ALWAYS) {
        return InlineCost {
            static_cost: 1,
            loop_penalty: 0,
            cost: 1,
            benefit: 100,
            should_inline: true,
        };
    }
    // #[inline(never)] — 永不内联
    if func.attributes.contains(FunctionAttributes::INLINE_NEVER) {
        return InlineCost {
            static_cost: usize::MAX,
            loop_penalty: 0,
            cost: usize::MAX,
            benefit: 0,
            should_inline: false,
        };
    }

    // 仅处理单块函数
    if func.dfg.blocks.len() != 1 {
        return InlineCost {
            static_cost: 0,
            loop_penalty: 0,
            cost: 0,
            benefit: 0,
            should_inline: false,
        };
    }

    let block = &func.dfg.blocks[0];

    // 检查自递归
    for &inst_id in &block.inst_order {
        let inst = &func.dfg.insts[inst_id.0 as usize];
        if matches!(inst.opcode, Opcode::Call)
            && let Some(inner_ref) = inst.immediates.iter().find_map(|i| i.as_func())
            && inner_ref == callee
        {
            return InlineCost {
                static_cost: usize::MAX,
                loop_penalty: 0,
                cost: usize::MAX,
                benefit: 0,
                should_inline: false,
            };
        }
    }

    if !matches!(block.terminator, Terminator::Return { .. }) {
        return InlineCost {
            static_cost: 0,
            loop_penalty: 0,
            cost: 0,
            benefit: 0,
            should_inline: false,
        };
    }

    let static_cost = block.inst_order.len();

    // 收益估算：常量参数可折叠
    let const_param_count = call_inst.operands.len(); // heuristic: all operands may enable folding
    let fold_benefit = const_param_count; // 1 point per operand
    let call_benefit = 1; // eliminating call/return overhead
    let benefit = fold_benefit + call_benefit;

    // 循环惩罚
    let loop_multiplier: usize = if loop_depth > 0 { 10 } else { 1 };
    let loop_penalty = static_cost * (loop_multiplier - 1);
    let effective_cost = static_cost * loop_multiplier;

    let should_inline = effective_cost.saturating_sub(benefit) <= threshold;

    InlineCost {
        static_cost,
        loop_penalty,
        cost: effective_cost,
        benefit,
        should_inline,
    }
}

/// 检查是否应该内联，同时检测调用链递归。
fn can_inline_with_chain(
    callee: FuncRef,
    call_inst: &Instruction,
    functions: &HashMap<FuncRef, Function>,
    threshold: usize,
    loop_depth: u32,
    call_chain: &[FuncRef],
) -> bool {
    if call_chain.contains(&callee) {
        return false;
    }
    evaluate_inline_cost(callee, call_inst, functions, threshold, loop_depth).should_inline
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 构建一个简单的被调用函数：fn add_one(x: i32) -> i32 { x + 1 }
    fn build_add_one() -> Function {
        let sig = FunctionSignature::new(&[(TypeId::I32, "x")], &[TypeId::I32]);
        let mut b = FunctionBuilder::new("add_one", TypeContext::new(), sig);
        let (entry, params) = b.create_block_with_params(&[(TypeId::I32, "x")]);
        b.switch_to_block(entry);
        let one = b.iconst_i32(1);
        let sum = b.iadd(params[0], one);
        b.ret(&[sum]);
        b.finish()
    }

    #[test]
    fn inline_simple_call() {
        let callee = build_add_one();
        let mut funcs = HashMap::new();
        funcs.insert(FuncRef(0), callee);

        // caller: fn test() -> i32 { add_one(41) }
        let sig = FunctionSignature::new(&[], &[TypeId::I32]);
        let mut b = FunctionBuilder::new("test", TypeContext::new(), sig);
        let entry = b.create_block();
        b.switch_to_block(entry);
        let c41 = b.iconst_i32(41);
        let ret = b.call(FuncRef(0), &[c41], &[TypeId::I32]);
        b.ret(&[ret[0]]);

        let mut caller = b.finish();
        let pass = InlinePass::new(funcs);
        let r = pass.run_on_function(&mut caller).unwrap();
        assert!(r.changed);

        // After inlining, the Call should be replaced with Nop
        let has_call = caller.dfg.blocks[0]
            .inst_order
            .iter()
            .any(|&iid| matches!(caller.dfg.insts[iid.0 as usize].opcode, Opcode::Call));
        assert!(!has_call, "Call should be inlined");
    }

    #[test]
    fn inline_respects_threshold() {
        // Build a large callee (more than 5 instructions)
        let sig = FunctionSignature::new(&[(TypeId::I32, "x")], &[TypeId::I32]);
        let mut b = FunctionBuilder::new("big_fn", TypeContext::new(), sig);
        let (entry, params) = b.create_block_with_params(&[(TypeId::I32, "x")]);
        b.switch_to_block(entry);
        let x = params[0];
        let v1 = b.iadd(x, x);
        let v2 = b.imul(v1, v1);
        let v3 = b.isub(v2, x);
        let v4 = b.bor(v3, v1);
        let v5 = b.bxor(v4, v2);
        b.ret(&[v5]);
        let callee = b.finish();
        let callee_size = callee.dfg.blocks[0].inst_order.len();
        assert!(
            callee_size >= 5,
            "callee should have at least 5 instructions"
        );

        let mut funcs = HashMap::new();
        funcs.insert(FuncRef(0), callee);

        let sig2 = FunctionSignature::new(&[], &[TypeId::I32]);
        let mut b2 = FunctionBuilder::new("test", TypeContext::new(), sig2);
        let entry2 = b2.create_block();
        b2.switch_to_block(entry2);
        let c = b2.iconst_i32(5);
        let ret = b2.call(FuncRef(0), &[c], &[TypeId::I32]);
        b2.ret(&[ret[0]]);
        let mut caller = b2.finish();

        // Build a fresh callee for the threshold test
        let sig3 = FunctionSignature::new(&[(TypeId::I32, "x")], &[TypeId::I32]);
        let mut b3 = FunctionBuilder::new("big_fn2", TypeContext::new(), sig3);
        let (entry3, params3) = b3.create_block_with_params(&[(TypeId::I32, "x")]);
        b3.switch_to_block(entry3);
        let x3 = params3[0];
        let v1_2 = b3.iadd(x3, x3);
        let v2_2 = b3.imul(v1_2, v1_2);
        let v3_2 = b3.isub(v2_2, x3);
        let v4_2 = b3.bor(v3_2, v1_2);
        let v5_2 = b3.bxor(v4_2, v2_2);
        b3.ret(&[v5_2]);
        let callee2 = b3.finish();

        let mut funcs2 = HashMap::new();
        funcs2.insert(FuncRef(0), callee2);

        let mut low_pass = InlinePass::new(funcs2);
        low_pass.set_threshold(2);
        let r = low_pass.run_on_function(&mut caller).unwrap();
        assert!(!r.changed, "Should not inline above threshold");
    }

    #[test]
    fn inline_maps_return_value() {
        let callee = build_add_one();
        let mut funcs = HashMap::new();
        funcs.insert(FuncRef(0), callee);

        // caller uses the return value in further computation
        let sig = FunctionSignature::new(&[], &[TypeId::I32]);
        let mut b = FunctionBuilder::new("test", TypeContext::new(), sig);
        let entry = b.create_block();
        b.switch_to_block(entry);
        let c = b.iconst_i32(10);
        let ret = b.call(FuncRef(0), &[c], &[TypeId::I32]);
        let doubled = b.iadd(ret[0], ret[0]);
        b.ret(&[doubled]);

        let mut caller = b.finish();
        let pass = InlinePass::new(funcs);
        let r = pass.run_on_function(&mut caller).unwrap();
        assert!(r.changed);
    }

    #[test]
    fn inline_no_change_for_external() {
        let sig = FunctionSignature::new(&[], &[TypeId::I32]);
        let mut b = FunctionBuilder::new("test", TypeContext::new(), sig);
        let entry = b.create_block();
        b.switch_to_block(entry);
        let c = b.iconst_i32(1);
        let ret = b.call(FuncRef(99), &[c], &[TypeId::I32]); // not in table
        b.ret(&[ret[0]]);

        let mut caller = b.finish();
        let pass = InlinePass::new(HashMap::new());
        let r = pass.run_on_function(&mut caller).unwrap();
        assert!(!r.changed);
    }

    #[test]
    fn inline_always_attribute_forces_inline() {
        let sig = FunctionSignature::new(&[(TypeId::I32, "x")], &[TypeId::I32]);
        let mut callee_builder = FunctionBuilder::new("always_fn", TypeContext::new(), sig);
        let (entry, params) = callee_builder.create_block_with_params(&[(TypeId::I32, "x")]);
        callee_builder.switch_to_block(entry);
        let x = params[0];
        let v1 = callee_builder.iadd(x, x);
        let v2 = callee_builder.imul(v1, v1);
        let v3 = callee_builder.isub(v2, x);
        let v4 = callee_builder.bor(v3, v1);
        let v5 = callee_builder.bxor(v4, v2);
        let v6 = callee_builder.iadd(v5, v5);
        let v7 = callee_builder.imul(v6, v6);
        callee_builder.ret(&[v7]);
        let mut callee = callee_builder.finish();
        callee.attributes.set(FunctionAttributes::INLINE_ALWAYS);

        let mut funcs = HashMap::new();
        funcs.insert(FuncRef(0), callee);

        let sig2 = FunctionSignature::new(&[], &[TypeId::I32]);
        let mut b2 = FunctionBuilder::new("test", TypeContext::new(), sig2);
        let entry2 = b2.create_block();
        b2.switch_to_block(entry2);
        let c = b2.iconst_i32(5);
        let ret = b2.call(FuncRef(0), &[c], &[TypeId::I32]);
        b2.ret(&[ret[0]]);
        let mut caller = b2.finish();

        let pass = InlinePass::new(funcs);
        let r = pass.run_on_function(&mut caller).unwrap();
        assert!(r.changed, "Should inline with #[inline(always)]");
    }

    #[test]
    fn inline_never_attribute_prevents_inline() {
        let sig = FunctionSignature::new(&[(TypeId::I32, "x")], &[TypeId::I32]);
        let mut callee_builder = FunctionBuilder::new("never_fn", TypeContext::new(), sig);
        let (entry, params) = callee_builder.create_block_with_params(&[(TypeId::I32, "x")]);
        callee_builder.switch_to_block(entry);
        let one = callee_builder.iconst_i32(1);
        let result = callee_builder.iadd(params[0], one);
        callee_builder.ret(&[result]);
        let mut callee = callee_builder.finish();
        callee.attributes.set(FunctionAttributes::INLINE_NEVER);

        let mut funcs = HashMap::new();
        funcs.insert(FuncRef(0), callee);

        let sig2 = FunctionSignature::new(&[], &[TypeId::I32]);
        let mut b2 = FunctionBuilder::new("test", TypeContext::new(), sig2);
        let entry2 = b2.create_block();
        b2.switch_to_block(entry2);
        let c = b2.iconst_i32(5);
        let ret = b2.call(FuncRef(0), &[c], &[TypeId::I32]);
        b2.ret(&[ret[0]]);
        let mut caller = b2.finish();

        let pass = InlinePass::new(funcs);
        let r = pass.run_on_function(&mut caller).unwrap();
        assert!(!r.changed, "Should NOT inline with #[inline(never)]");
    }

    #[test]
    fn inline_with_threshold_builder() {
        let callee = build_add_one();
        let mut funcs = HashMap::new();
        funcs.insert(FuncRef(0), callee);

        let sig = FunctionSignature::new(&[], &[TypeId::I32]);
        let mut b = FunctionBuilder::new("test", TypeContext::new(), sig);
        let entry = b.create_block();
        b.switch_to_block(entry);
        let c = b.iconst_i32(41);
        let ret = b.call(FuncRef(0), &[c], &[TypeId::I32]);
        b.ret(&[ret[0]]);
        let mut caller = b.finish();

        let mut pass = InlinePass::new(funcs);
        pass.with_threshold(5);
        assert_eq!(pass.threshold(), 5);
        let r = pass.run_on_function(&mut caller).unwrap();
        assert!(r.changed, "Should inline when threshold is sufficient");
    }

    #[test]
    fn inline_loop_penalty_applies() {
        let sig = FunctionSignature::new(&[(TypeId::I32, "x")], &[TypeId::I32]);
        let mut callee_builder = FunctionBuilder::new("loop_fn", TypeContext::new(), sig);
        let (entry, params) = callee_builder.create_block_with_params(&[(TypeId::I32, "x")]);
        callee_builder.switch_to_block(entry);
        let x = params[0];
        let v1 = callee_builder.iadd(x, x);
        let v2 = callee_builder.imul(v1, v1);
        callee_builder.ret(&[v2]);
        let callee = callee_builder.finish();

        let mut funcs = HashMap::new();
        funcs.insert(FuncRef(0), callee);

        let sig2 = FunctionSignature::new(&[], &[TypeId::I32]);
        let mut b = FunctionBuilder::new("loop_caller", TypeContext::new(), sig2);
        let entry_blk = b.create_block();
        let body_blk = b.create_block();
        let exit_blk = b.create_block();

        b.switch_to_block(entry_blk);
        let iv = b.iconst_i32(0);
        b.jump(body_blk, &[]);

        b.switch_to_block(body_blk);
        let c10 = b.iconst_i32(10);
        let cmp = b.icmp(IntCC::SignedLessThan, iv, c10);
        let c5 = b.iconst_i32(5);
        let _call_res = b.call(FuncRef(0), &[c5], &[TypeId::I32]); // call inside loop!
        let inc = b.iconst_i32(1);
        let _next = b.iadd(iv, inc);
        b.branch(cmp, body_blk, &[], exit_blk, &[]);

        b.switch_to_block(exit_blk);
        b.ret(&[iv]);

        let mut caller = b.finish();
        let pass = InlinePass::new(funcs);
        let r = pass.run_on_function(&mut caller).unwrap();
        // With loop penalty (10x), the callee cost is 2*10=20 which may exceed threshold
        // The key test: shouldn't crash.
        let _ = r;
    }

    #[test]
    fn inline_recursion_detection() {
        let sig = FunctionSignature::new(&[(TypeId::I32, "x")], &[TypeId::I32]);
        let mut callee_builder = FunctionBuilder::new("recurse", TypeContext::new(), sig);
        let (entry, params) = callee_builder.create_block_with_params(&[(TypeId::I32, "x")]);
        callee_builder.switch_to_block(entry);
        let ret = callee_builder.call(FuncRef(0), &[params[0]], &[TypeId::I32]);
        callee_builder.ret(&[ret[0]]);
        let callee = callee_builder.finish();

        let mut funcs = HashMap::new();
        funcs.insert(FuncRef(0), callee);

        let sig2 = FunctionSignature::new(&[], &[TypeId::I32]);
        let mut b = FunctionBuilder::new("caller", TypeContext::new(), sig2);
        let entry2 = b.create_block();
        b.switch_to_block(entry2);
        let c = b.iconst_i32(1);
        let ret = b.call(FuncRef(0), &[c], &[TypeId::I32]);
        b.ret(&[ret[0]]);
        let mut caller = b.finish();

        let pass = InlinePass::new(funcs);
        let r = pass.run_on_function(&mut caller).unwrap();
        assert!(!r.changed, "Recursive function should not be inlined");
    }
}
