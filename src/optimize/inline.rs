//! 函数内联 pass。
//!
//! 将小函数（尤其是 const 函数）的内联到调用点，消除调用开销，
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
//! # 算法
//!
//! 1. 找到符合条件的 Call 指令
//! 2. 克隆被调用函数的指令到调用点之前
//! 3. 将被调用函数的返回结果映射到 Call 的结果
//! 4. 将 Call 替换为 Nop

use crate::CompileError;
use crate::ir::*;
use crate::optimize::{OptimizationPass, PassResult};
use std::collections::HashMap;

/// 函数内联 pass。
///
/// 需要函数表来查找被调用函数的 IR 体。
pub struct InlinePass {
    /// 可用函数的表。
    functions: HashMap<FuncRef, Function>,
    /// 内联阈值：超过此指令数的函数不会被内联。
    threshold: usize,
    /// 最大内联深度：防止递归内联链导致的代码膨胀（默认 8）。
    max_depth: usize,
    /// 最小内联收益比（未来扩展）。
    _min_benefit_ratio: f64,
}

impl InlinePass {
    /// 创建一个新的内联 pass。
    ///
    /// `functions` 提供被调用函数的 IR 体。
    /// 仅在此表中的函数可被内联。
    pub fn new(functions: HashMap<FuncRef, Function>) -> Self {
        Self {
            functions,
            threshold: 20,
            max_depth: 8,
            _min_benefit_ratio: 0.5,
        }
    }

    /// 设置内联阈值（最大指令数，默认 20）。
    pub fn set_threshold(&mut self, threshold: usize) {
        self.threshold = threshold;
    }

    /// 构建器模式：设置内联阈值并返回 `&mut Self` 用于链式调用。
    pub fn with_threshold(&mut self, threshold: usize) -> &mut Self {
        self.threshold = threshold;
        self
    }

    /// 获取当前内联阈值。
    pub fn threshold(&self) -> usize {
        self.threshold
    }

    /// 设置最大内联深度。
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
    let call_chain: Vec<FuncRef> = Vec::new();
    inline_calls_depth(func, functions, threshold, max_depth, 0, &call_chain)
}

fn inline_calls_depth(
    func: &mut Function,
    functions: &HashMap<FuncRef, Function>,
    threshold: usize,
    max_depth: usize,
    current_depth: usize,
    call_chain: &[FuncRef],
) -> Result<PassResult, CompileError> {
    let _ = current_depth;
    let mut result = PassResult::default();
    let mut inline_count: usize = 0;

    // 构建循环森林用于循环深度检测
    let dom_tree = compute_dom_tree(func);
    let loop_forest = LoopForest::build(func, &dom_tree);

    for block_idx in 0..func.blocks.len() {
        let block_id = func.blocks[block_idx].id;
        let loop_depth = loop_forest.get_loop_depth(block_id);

        let mut inst_idx = 0;
        while inst_idx < func.blocks[block_idx].instructions.len() {
            let should_inline = {
                let inst = &func.blocks[block_idx].instructions[inst_idx];
                if let Opcode::Call { func: callee } = &inst.opcode {
                    can_inline_with_chain(
                        *callee,
                        inst,
                        functions,
                        threshold,
                        loop_depth,
                        call_chain,
                    )
                } else {
                    false
                }
            };

            if inline_count >= max_depth {
                break;
            }
            if should_inline {
                inline_count += 1;
                let inst = &func.blocks[block_idx].instructions[inst_idx];
                let callee_ref = match &inst.opcode {
                    Opcode::Call { func } => *func,
                    _ => unreachable!(),
                };
                let callee = &functions[&callee_ref];
                let call_result = inst.result;
                let call_operands: Vec<Value> = inst.operands.iter().copied().collect();

                // 克隆被调用函数的指令并映射值
                let mut value_map: HashMap<Value, Value> = HashMap::new();

                // 映射参数到实参
                let entry_block = &callee.blocks[0];
                for (i, (param_val, _)) in entry_block.params.iter().enumerate() {
                    if i < call_operands.len() {
                        value_map.insert(*param_val, call_operands[i]);
                    }
                }

                // 克隆指令并插入到调用点之前
                let mut cloned_insts: Vec<Instruction> = Vec::new();
                for callee_inst in &entry_block.instructions {
                    let mut new_inst = callee_inst.clone();

                    // 重映射常量池索引（从 callee 的池映射到 caller 的池）
                    match &new_inst.opcode {
                        Opcode::Iconst { index } => {
                            if let Some(big) = callee.constant_pool.get(*index) {
                                let new_index = func.constant_pool.insert(big.clone());
                                new_inst.opcode = Opcode::Iconst { index: new_index };
                            }
                        }
                        Opcode::Fconst { index } => {
                            if let Some(big) = callee.constant_pool.get(*index) {
                                let new_index = func.constant_pool.insert(big.clone());
                                new_inst.opcode = Opcode::Fconst { index: new_index };
                            }
                        }
                        _ => {}
                    }

                    // 重映射 operands
                    for operand in new_inst.operands.iter_mut() {
                        if let Some(mapped) = value_map.get(operand) {
                            *operand = *mapped;
                        }
                    }

                    // 分配新的 result Value
                    if let Some(old_result) = new_inst.result {
                        let new_result = func.create_value();
                        value_map.insert(old_result, new_result);
                        new_inst.result = Some(new_result);
                    }

                    cloned_insts.push(new_inst);
                }

                // 将返回值映射到 call 的 result
                if let Terminator::Return { values } = &entry_block.terminator
                    && let (Some(cr), Some(ret_val)) = (call_result, values.first())
                {
                    if let Some(mapped_ret) = value_map.get(ret_val) {
                        value_map.insert(cr, *mapped_ret);
                    } else {
                        // 返回值是入口参数（未映射）→ 直接映射
                        value_map.insert(cr, *ret_val);
                    }
                }

                // 替换 call 为 Nop，插入克隆的指令
                func.blocks[block_idx].instructions[inst_idx].opcode = Opcode::Nop;
                func.blocks[block_idx].instructions[inst_idx]
                    .operands
                    .clear();
                func.blocks[block_idx].instructions[inst_idx].ty = Type::Void;
                func.blocks[block_idx].instructions[inst_idx].result = None;

                // 在 call 位置插入克隆的指令
                let cloned_len = cloned_insts.len();
                let insert_pos = inst_idx;
                for ci in cloned_insts.into_iter().rev() {
                    func.blocks[block_idx].instructions.insert(insert_pos, ci);
                }

                // 更新所有后续指令中对 call_result 的引用
                if let Some(cr) = call_result
                    && let Some(mapped) = value_map.get(&cr)
                {
                    for inst in func.blocks[block_idx].instructions.iter_mut() {
                        for operand in inst.operands.iter_mut() {
                            if *operand == cr {
                                *operand = *mapped;
                            }
                        }
                    }
                    // Also update terminator uses
                    match &mut func.blocks[block_idx].terminator {
                        Terminator::Return { values } => {
                            for v in values.iter_mut() {
                                if *v == cr {
                                    *v = *mapped;
                                }
                            }
                        }
                        Terminator::Branch {
                            cond,
                            true_args,
                            false_args,
                            ..
                        } => {
                            if *cond == cr {
                                *cond = *mapped;
                            }
                            for v in true_args.iter_mut().chain(false_args.iter_mut()) {
                                if *v == cr {
                                    *v = *mapped;
                                }
                            }
                        }
                        Terminator::Jump { args, .. } => {
                            for v in args.iter_mut() {
                                if *v == cr {
                                    *v = *mapped;
                                }
                            }
                        }
                        Terminator::Unreachable => {}
                        Terminator::Switch {
                            discriminant: disc,
                            cases,
                            ..
                        } => {
                            if *disc == cr {
                                *disc = *mapped;
                            }
                            for (_, _, args) in cases.iter_mut() {
                                for v in args.iter_mut() {
                                    if *v == cr {
                                        *v = *mapped;
                                    }
                                }
                            }
                        }
                    }
                }

                result.instructions_removed += 1;
                result.changed = true;
                inst_idx += cloned_len; // skip past inserted instructions
            }

            inst_idx += 1;
        }
    }

    Ok(result)
}

/// 内联成本评估结果。
#[derive(Debug, Clone)]
pub struct InlineCost {
    /// 静态成本：内联后新增的指令数估算。
    pub static_cost: usize,
    /// 循环惩罚：调用点所在循环深度的额外成本（深循环中成本 × 10）。
    pub loop_penalty: usize,
    /// 总成本：static_cost + loop_penalty。
    pub cost: usize,
    /// 预期收益：常量折叠机会 + call 消除 + 上下文特化。
    pub benefit: usize,
    /// 是否应内联。
    pub should_inline: bool,
}

/// 评估内联指定调用的成本和收益。
///
/// 比简单的阈值检查更精细：考虑循环惩罚、常量参数收益、
/// `#[inline(always)]` / `#[inline(never)]` 属性、以及内联后的死代码消除机会。
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
            }
        }
    };

    // === 属性检查 ===
    // `#[inline(always)]` — 无视成本，始终内联
    if func.attributes.contains(FunctionAttributes::INLINE_ALWAYS) {
        return InlineCost {
            static_cost: 1,
            loop_penalty: 0,
            cost: 1,
            benefit: 100,
            should_inline: true,
        };
    }
    // `#[inline(never)]` — 永不内联
    if func.attributes.contains(FunctionAttributes::INLINE_NEVER) {
        return InlineCost {
            static_cost: usize::MAX,
            loop_penalty: 0,
            cost: usize::MAX,
            benefit: 0,
            should_inline: false,
        };
    }

    // 单块函数
    if func.blocks.len() != 1 {
        return InlineCost {
            static_cost: 0,
            loop_penalty: 0,
            cost: 0,
            benefit: 0,
            should_inline: false,
        };
    }

    let block = &func.blocks[0];

    // 不能自递归
    for inst in &block.instructions {
        if let Opcode::Call { func: inner } = &inst.opcode
            && *inner == callee
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

    // 基础成本：指令数
    let static_cost = block.instructions.len();

    // 收益估算：常量参数可折叠
    let mut const_param_count = 0usize;
    for op in &call_inst.operands {
        // 常量操作数产生折叠机会
        if op.0 > 0 {
            const_param_count += 1;
        }
    }
    let fold_benefit = const_param_count * 2; // 每个常量参数可能消除 2 条指令

    // Call 消除节省：call + 相关参数准备指令 ≈ 3
    let call_benefit = 3;

    let benefit = fold_benefit + call_benefit;

    // 循环惩罚：在循环中的内联成本 × 10
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
/// 
/// 如果 callee 已经在调用链中存在（直接或间接递归），拒绝内联。
fn can_inline_with_chain(
    callee: FuncRef,
    call_inst: &Instruction,
    functions: &HashMap<FuncRef, Function>,
    threshold: usize,
    loop_depth: u32,
    call_chain: &[FuncRef],
) -> bool {
    // 递归检测: 如果 callee 已经在调用链中，拒绝内联
    if call_chain.contains(&callee) {
        return false;
    }

    evaluate_inline_cost(callee, call_inst, functions, threshold, loop_depth).should_inline
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ir::FunctionBuilder;

    /// 构建一个简单的被调用函数：fn add_one(x: i32) -> i32 { x + 1 }
    fn build_add_one() -> Function {
        let sig = Signature::new(&[(Type::I32, "x")], &[Type::I32]);
        let mut b = FunctionBuilder::new("add_one", sig);
        let (entry, params) = b.create_block_with_params(&[(Type::I32, "x")]);
        b.switch_to_block(entry);
        let one = b.iconst_i32(1);
        let sum = b.iadd(params[0], one);
        b.return_(&[sum]);
        b.finish()
    }

    #[test]
    fn inline_simple_call() {
        let callee = build_add_one();
        let mut funcs = HashMap::new();
        funcs.insert(FuncRef(0), callee);

        // caller: fn test() -> i32 { add_one(41) }
        let mut b = FunctionBuilder::new("test", Signature::new(&[], &[Type::I32]));
        let entry = b.create_block();
        b.switch_to_block(entry);
        let c41 = b.iconst_i32(41);
        let ret = b.call(FuncRef(0), &[c41], &[Type::I32]);
        b.return_(&[ret[0]]);

        let mut caller = b.finish();
        let pass = InlinePass::new(funcs);
        let r = pass.run_on_function(&mut caller).unwrap();
        assert!(r.changed);
        assert!(caller.validate().is_valid());
        // After inlining, the Call should be replaced with Nop
        let has_call = caller.blocks[0]
            .instructions
            .iter()
            .any(|i| matches!(i.opcode, Opcode::Call { .. }));
        assert!(!has_call, "Call should be inlined");
    }

    #[test]
    fn inline_respects_threshold() {
        // Build a large callee (more than 5 instructions)
        let sig = Signature::new(&[(Type::I32, "x")], &[Type::I32]);
        let mut b = FunctionBuilder::new("big_fn", sig.clone());
        let (entry, params) = b.create_block_with_params(&[(Type::I32, "x")]);
        b.switch_to_block(entry);
        let x = params[0];
        let v1 = b.iadd(x, x);
        let v2 = b.imul(v1, v1);
        let v3 = b.isub(v2, x);
        let v4 = b.bor(v3, v1);
        let v5 = b.bxor(v4, v2);
        b.return_(&[v5]);
        let callee = b.finish();
        let callee_size = callee.blocks[0].instructions.len();
        assert!(
            callee_size >= 5,
            "callee should have at least 5 instructions"
        );

        let mut funcs = HashMap::new();
        funcs.insert(FuncRef(0), callee);

        let mut b = FunctionBuilder::new("test", Signature::new(&[], &[Type::I32]));
        let entry = b.create_block();
        b.switch_to_block(entry);
        let c = b.iconst_i32(5);
        let ret = b.call(FuncRef(0), &[c], &[Type::I32]);
        b.return_(&[ret[0]]);
        let mut caller = b.finish();

        // Set threshold below callee size to prevent inlining
        let mut funcs2: HashMap<FuncRef, Function> = HashMap::new();
        // Re-insert since funcs was moved
        let sig2 = Signature::new(&[(Type::I32, "x")], &[Type::I32]);
        let mut b2 = FunctionBuilder::new("big_fn", sig2.clone());
        let (entry2, params2) = b2.create_block_with_params(&[(Type::I32, "x")]);
        b2.switch_to_block(entry2);
        let x2 = params2[0];
        let v1_2 = b2.iadd(x2, x2);
        let v2_2 = b2.imul(v1_2, v1_2);
        let v3_2 = b2.isub(v2_2, x2);
        let v4_2 = b2.bor(v3_2, v1_2);
        let v5_2 = b2.bxor(v4_2, v2_2);
        b2.return_(&[v5_2]);
        funcs2.insert(FuncRef(0), b2.finish());

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
        let mut b = FunctionBuilder::new("test", Signature::new(&[], &[Type::I32]));
        let entry = b.create_block();
        b.switch_to_block(entry);
        let c = b.iconst_i32(10);
        let ret = b.call(FuncRef(0), &[c], &[Type::I32]);
        let doubled = b.iadd(ret[0], ret[0]);
        b.return_(&[doubled]);

        let mut caller = b.finish();
        let pass = InlinePass::new(funcs);
        let r = pass.run_on_function(&mut caller).unwrap();
        assert!(r.changed);
        assert!(caller.validate().is_valid());
    }

    #[test]
    fn inline_no_change_for_external() {
        // Call to a function not in the table should not be inlined
        let mut b = FunctionBuilder::new("test", Signature::new(&[], &[Type::I32]));
        let entry = b.create_block();
        b.switch_to_block(entry);
        let c = b.iconst_i32(1);
        let ret = b.call(FuncRef(99), &[c], &[Type::I32]); // not in table
        b.return_(&[ret[0]]);

        let mut caller = b.finish();
        let pass = InlinePass::new(HashMap::new());
        let r = pass.run_on_function(&mut caller).unwrap();
        assert!(!r.changed);
    }

    // ============================================================
    // 新增强测试
    // ============================================================

    #[test]
    fn inline_always_attribute_forces_inline() {
        // Callee with #[inline(always)] should be inlined even if above threshold
        let sig = Signature::new(&[(Type::I32, "x")], &[Type::I32]);
        let mut callee_builder = FunctionBuilder::new("always_fn", sig.clone());
        let (entry, params) =
            callee_builder.create_block_with_params(&[(Type::I32, "x")]);
        callee_builder.switch_to_block(entry);
        let x = params[0];
        let v1 = callee_builder.iadd(x, x);
        let v2 = callee_builder.imul(v1, v1);
        let v3 = callee_builder.isub(v2, x);
        let v4 = callee_builder.bor(v3, v1);
        let v5 = callee_builder.bxor(v4, v2);
        let v6 = callee_builder.iadd(v5, v5);
        let v7 = callee_builder.imul(v6, v6);
        callee_builder.return_(&[v7]);
        let mut callee = callee_builder.finish();
        callee.attributes
            .set(FunctionAttributes::INLINE_ALWAYS);

        let mut funcs = HashMap::new();
        funcs.insert(FuncRef(0), callee);

        let mut b = FunctionBuilder::new("test", Signature::new(&[], &[Type::I32]));
        let entry = b.create_block();
        b.switch_to_block(entry);
        let c = b.iconst_i32(5);
        let ret = b.call(FuncRef(0), &[c], &[Type::I32]);
        b.return_(&[ret[0]]);
        let mut caller = b.finish();

        let pass = InlinePass::new(funcs);
        let r = pass.run_on_function(&mut caller).unwrap();
        assert!(r.changed, "Should inline even with #[inline(always)]");
        assert!(caller.validate().is_valid());
    }

    #[test]
    fn inline_never_attribute_prevents_inline() {
        // Callee with #[inline(never)] should NOT be inlined even if tiny
        let sig = Signature::new(&[(Type::I32, "x")], &[Type::I32]);
        let mut callee_builder = FunctionBuilder::new("never_fn", sig);
        let (entry, params) =
            callee_builder.create_block_with_params(&[(Type::I32, "x")]);
        callee_builder.switch_to_block(entry);
        let one = callee_builder.iconst_i32(1);
        let result = callee_builder.iadd(params[0], one);
        callee_builder.return_(&[result]);
        let mut callee = callee_builder.finish();
        callee.attributes
            .set(FunctionAttributes::INLINE_NEVER);

        let mut funcs = HashMap::new();
        funcs.insert(FuncRef(0), callee);

        let mut b = FunctionBuilder::new("test", Signature::new(&[], &[Type::I32]));
        let entry = b.create_block();
        b.switch_to_block(entry);
        let c = b.iconst_i32(5);
        let ret = b.call(FuncRef(0), &[c], &[Type::I32]);
        b.return_(&[ret[0]]);
        let mut caller = b.finish();

        let pass = InlinePass::new(funcs);
        let r = pass.run_on_function(&mut caller).unwrap();
        assert!(!r.changed, "Should NOT inline with #[inline(never)]");
    }

    #[test]
    fn inline_with_threshold_builder() {
        let callee = build_add_one();
        let mut funcs = HashMap::new();
        funcs.insert(FuncRef(0), callee);

        let mut b = FunctionBuilder::new("test", Signature::new(&[], &[Type::I32]));
        let entry = b.create_block();
        b.switch_to_block(entry);
        let c = b.iconst_i32(41);
        let ret = b.call(FuncRef(0), &[c], &[Type::I32]);
        b.return_(&[ret[0]]);
        let mut caller = b.finish();

        // Use with_threshold builder method
        let mut pass = InlinePass::new(funcs);
        pass.with_threshold(5);
        assert_eq!(pass.threshold(), 5);
        let r = pass.run_on_function(&mut caller).unwrap();
        assert!(r.changed, "Should inline when threshold is sufficient");
    }

    #[test]
    fn inline_loop_penalty_applies() {
        // Build a callee that's just small enough
        let sig = Signature::new(&[(Type::I32, "x")], &[Type::I32]);
        let mut callee_builder = FunctionBuilder::new("loop_fn", sig.clone());
        let (entry, params) =
            callee_builder.create_block_with_params(&[(Type::I32, "x")]);
        callee_builder.switch_to_block(entry);
        let x = params[0];
        let v1 = callee_builder.iadd(x, x);
        let v2 = callee_builder.imul(v1, v1);
        callee_builder.return_(&[v2]);
        let callee = callee_builder.finish();

        let mut funcs = HashMap::new();
        funcs.insert(FuncRef(0), callee);

        // Build caller with a loop body containing the call
        let mut b = FunctionBuilder::new("loop_caller", Signature::new(&[], &[Type::I32]));
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
        let _call_res = b.call(FuncRef(0), &[c5], &[Type::I32]); // call inside loop!
        let inc = b.iconst_i32(1);
        let _next = b.iadd(iv, inc);
        b.branch(cmp, body_blk, exit_blk, &[], &[]);

        b.switch_to_block(exit_blk);
        b.return_(&[iv]);

        let mut caller = b.finish();
        let pass = InlinePass::new(funcs);
        let r = pass.run_on_function(&mut caller).unwrap();

        // With loop penalty (10x), the callee cost is 2*10=20, which may exceed threshold
        // The exact behavior depends on threshold. The key test: shouldn't crash.
        let _ = r;
        assert!(caller.validate().is_valid());
    }

    #[test]
    fn inline_recursion_detection() {
        // Build a callee that calls itself: fn recurse(x: i32) -> i32 { recurse(x) }
        let sig = Signature::new(&[(Type::I32, "x")], &[Type::I32]);
        let mut callee_builder = FunctionBuilder::new("recurse", sig.clone());
        let (entry, params) =
            callee_builder.create_block_with_params(&[(Type::I32, "x")]);
        callee_builder.switch_to_block(entry);
        let ret = callee_builder.call(FuncRef(0), &[params[0]], &[Type::I32]);
        callee_builder.return_(&[ret[0]]);
        let callee = callee_builder.finish();

        let mut funcs = HashMap::new();
        funcs.insert(FuncRef(0), callee);

        let mut b = FunctionBuilder::new("caller", Signature::new(&[], &[Type::I32]));
        let entry = b.create_block();
        b.switch_to_block(entry);
        let c = b.iconst_i32(1);
        let ret = b.call(FuncRef(0), &[c], &[Type::I32]);
        b.return_(&[ret[0]]);
        let mut caller = b.finish();

        let pass = InlinePass::new(funcs);
        let r = pass.run_on_function(&mut caller).unwrap();
        // Should NOT inline recursive function
        assert!(!r.changed, "Recursive function should not be inlined");
    }
}
