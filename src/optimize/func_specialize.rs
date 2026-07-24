//! 函数特化 (Function Specialization) pass。
//!
//! 当 const 函数被调用且部分参数为编译时常量时，
//! 克隆该函数并用常量参数替换，然后内联到调用点。
//!
//! # 算法
//!
//! 1. 收集所有常量值和 const 调用点（不可变扫描）
//! 2. 对每个调用点：克隆被调用函数、替换常量参数、运行常量折叠
//! 3. 将特化版本内联到调用点（替换 Call 指令）

use crate::CompileError;
use crate::ir::*;
use crate::optimize::{ConstValue, OptimizationPass, PassResult};
use std::collections::HashMap;

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
        "Specializes const functions with constant arguments and inlines at call site"
    }

    fn run_on_function(&self, func: &mut Function) -> Result<PassResult, CompileError> {
        specialize_calls(func, &self.const_functions)
    }
}

/// 调用点信息（收集阶段，避免 borrow 冲突）。
struct CallSite {
    block_idx: usize,
    inst_idx: usize,
    callee_ref: FuncRef,
    const_args: Vec<Option<ConstValue>>,
}

pub fn specialize_calls(
    func: &mut Function,
    const_functions: &HashMap<FuncRef, Function>,
) -> Result<PassResult, CompileError> {
    let mut result = PassResult::default();

    // Phase 1: 收集已知常量和调用点（不可变扫描）
    let mut known: HashMap<Value, ConstValue> = HashMap::new();
    for block in func.blocks.iter() {
        for inst in &block.instructions {
            match &inst.opcode {
                Opcode::Iconst { index } => {
                    if let (Some(v), Some(big)) =
                        (inst.result, func.constant_pool.get(*index).cloned())
                    {
                        known.insert(v, ConstValue::Int(big, inst.ty));
                    }
                }
                Opcode::Fconst { index } => {
                    if let (Some(v), Some(big)) =
                        (inst.result, func.constant_pool.get(*index).cloned())
                    {
                        known.insert(v, ConstValue::Float(big, inst.ty));
                    }
                }
                _ => {}
            }
        }
    }

    let mut call_sites: Vec<CallSite> = Vec::new();
    for (bi, block) in func.blocks.iter().enumerate() {
        for (ii, inst) in block.instructions.iter().enumerate() {
            if let Opcode::Call { func: callee } = &inst.opcode
                && let Some(target_fn) = const_functions.get(callee)
                && target_fn.is_const
            {
                let const_args: Vec<Option<ConstValue>> = inst
                    .operands
                    .iter()
                    .map(|v| known.get(v).cloned())
                    .collect();
                call_sites.push(CallSite {
                    block_idx: bi,
                    inst_idx: ii,
                    callee_ref: *callee,
                    const_args,
                });
            }
        }
    }

    // Phase 2: 处理每个调用点
    for site in call_sites {
        let target_fn = match const_functions.get(&site.callee_ref) {
            Some(f) => f,
            None => continue,
        };

        let mut specialized = target_fn.clone();
        specialized.name = format!("{}_specialized", specialized.name);

        // 替换常量参数：在特化函数中，将常量参数对应的使用处直接替换
        let entry_params: Vec<(Value, Type)> = specialized
            .blocks
            .first()
            .map(|b| b.params.clone())
            .unwrap_or_default();
        for (pi, (param_val, param_ty)) in entry_params.iter().enumerate() {
            if let Some(Some(cv)) = site.const_args.get(pi) {
                let const_idx = match cv {
                    ConstValue::Int(big, _) | ConstValue::Float(big, _) => {
                        specialized.constant_pool.insert(big.clone())
                    }
                    _ => continue,
                };
                let new_const_val = specialized.create_value();
                let opcode = match cv {
                    ConstValue::Int(..) => Opcode::Iconst { index: const_idx },
                    ConstValue::Float(..) => Opcode::Fconst { index: const_idx },
                    _ => continue,
                };
                // 在块开头插入常量定义
                specialized.blocks[0].instructions.insert(
                    0,
                    Instruction::new(
                        opcode,
                        smallvec::smallvec![],
                        Some(new_const_val),
                        *param_ty,
                    ),
                );

                // 替换特化函数中所有对 param_val 的引用
                for blk in specialized.blocks.iter_mut() {
                    for i_inst in blk.instructions.iter_mut() {
                        for op in i_inst.operands.iter_mut() {
                            if *op == *param_val {
                                *op = new_const_val;
                            }
                        }
                    }
                    // 也更新 terminator 中对 param_val 的引用
                    match &mut blk.terminator {
                        Terminator::Branch {
                            cond,
                            true_args,
                            false_args,
                            ..
                        } => {
                            if *cond == *param_val {
                                *cond = new_const_val;
                            }
                            for v in true_args.iter_mut().chain(false_args.iter_mut()) {
                                if *v == *param_val {
                                    *v = new_const_val;
                                }
                            }
                        }
                        Terminator::Jump { args, .. } => {
                            for v in args.iter_mut() {
                                if *v == *param_val {
                                    *v = new_const_val;
                                }
                            }
                        }
                        Terminator::Return { values } => {
                            for v in values.iter_mut() {
                                if *v == *param_val {
                                    *v = new_const_val;
                                }
                            }
                        }
                        Terminator::Switch {
                            discriminant,
                            cases,
                            ..
                        } => {
                            if *discriminant == *param_val {
                                *discriminant = new_const_val;
                            }
                            for (_, _, args) in cases.iter_mut() {
                                for v in args.iter_mut() {
                                    if *v == *param_val {
                                        *v = new_const_val;
                                    }
                                }
                            }
                        }
                        Terminator::Unreachable => {}
                    }
                }
            }
        }

        // 对特化版本运行常量折叠
        crate::optimize::ConstFoldPass::new().run_on_function(&mut specialized)?;

        // 内联特化函数到调用点
        inline_at_call(func, site.block_idx, site.inst_idx, &specialized);
        result.changed = true;
        result.values_replaced += 1;
    }

    Ok(result)
}

/// 将特化函数内联到调用点，替换 Call 指令为 callee 函数体。
fn inline_at_call(caller: &mut Function, block_idx: usize, inst_idx: usize, callee: &Function) {
    if callee.blocks.is_empty() {
        return;
    }
    let callee_entry = &callee.blocks[0];

    // 获取 call 指令的操作数（实参）
    let call_operands: Vec<Value> = caller.blocks[block_idx].instructions[inst_idx]
        .operands
        .iter()
        .copied()
        .collect();
    let call_result = caller.blocks[block_idx].instructions[inst_idx].result;

    // 构建参数映射：callee 形参 → caller 实参
    let mut param_map: HashMap<Value, Value> = HashMap::new();
    if let Some(callee_first) = callee.blocks.first() {
        for (i, (param_val, _)) in callee_first.params.iter().enumerate() {
            if let Some(&arg) = call_operands.get(i) {
                param_map.insert(*param_val, arg);
            }
        }
    }

    // 构建值映射（形参 → 新 SSA 值）
    let mut value_map: HashMap<Value, Value> = HashMap::new();
    for (param, &arg) in &param_map {
        value_map.insert(*param, arg);
    }

    // 克隆 callee 所有指令（除了 return），映射值和常量池
    let mut new_insts: Vec<Instruction> = Vec::new();
    for inst in &callee_entry.instructions {
        let mut ni = inst.clone();

        // 重映射常量池索引
        match &inst.opcode {
            Opcode::Iconst { index } => {
                if let Some(big) = callee.constant_pool.get(*index).cloned() {
                    let new_idx = caller.constant_pool.insert(big);
                    ni.opcode = Opcode::Iconst { index: new_idx };
                }
            }
            Opcode::Fconst { index } => {
                if let Some(big) = callee.constant_pool.get(*index).cloned() {
                    let new_idx = caller.constant_pool.insert(big);
                    ni.opcode = Opcode::Fconst { index: new_idx };
                }
            }
            _ => {}
        }

        // 重映射操作数
        for op in ni.operands.iter_mut() {
            if let Some(&mapped) = value_map.get(op) {
                *op = mapped;
            }
        }
        // 重映射结果
        if let Some(r) = ni.result {
            let new_v = caller.create_value();
            value_map.insert(r, new_v);
            ni.result = Some(new_v);
        }

        new_insts.push(ni);
    }

    // 映射 callee 返回值到 call result
    if let Terminator::Return { values } = &callee_entry.terminator
        && let Some(cr) = call_result
        && let Some(&ret_val) = values.first()
        && let Some(&mapped_ret) = value_map.get(&ret_val)
    {
        // 在所有新指令的末尾添加 copy(call_result, mapped_ret)
        let copy_inst = Instruction::new(
            Opcode::Copy,
            smallvec::smallvec![mapped_ret],
            Some(cr),
            callee_entry
                .instructions
                .first()
                .map(|i| i.ty)
                .unwrap_or(Type::Void),
        );
        new_insts.push(copy_inst);
    }

    // 替换 Call 指令为内联的指令序列
    let block = &mut caller.blocks[block_idx];
    block.instructions.remove(inst_idx);
    for (offset, ni) in new_insts.into_iter().enumerate() {
        block.instructions.insert(inst_idx + offset, ni);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ir::FunctionBuilder;

    #[test]
    fn func_specialize_no_crash() {
        let sig = Signature::new(&[(Type::I32, "x")], &[Type::I32]);
        let mut callee = FunctionBuilder::new("mul2", sig.clone());
        callee.set_const(true);
        let (entry, params) = callee.create_block_with_params(&[(Type::I32, "x")]);
        callee.switch_to_block(entry);
        let two = callee.iconst_i32(2);
        let prod = callee.imul(params[0], two);
        callee.return_(&[prod]);
        let cf = callee.finish();

        let mut func_table = HashMap::new();
        func_table.insert(FuncRef(0), cf);

        let mut b = FunctionBuilder::new("caller", Signature::void());
        let entry = b.create_block();
        b.switch_to_block(entry);
        let c3 = b.iconst_i32(3);
        b.call(FuncRef(0), &[c3], &[Type::I32]);
        b.return_(&[]);

        let mut caller = b.finish();
        let pass = FuncSpecializePass::new(func_table);
        pass.run_on_function(&mut caller).unwrap();
        assert!(caller.validate().is_valid());
    }

    #[test]
    fn func_specialize_const_param_inlined() {
        // const fn square(x: i32) -> i32 { x * x }
        let sig = Signature::new(&[(Type::I32, "x")], &[Type::I32]);
        let mut callee = FunctionBuilder::new("square", sig);
        callee.set_const(true);
        let (entry, params) = callee.create_block_with_params(&[(Type::I32, "x")]);
        callee.switch_to_block(entry);
        let x = params[0];
        let sq = callee.imul(x, x);
        callee.return_(&[sq]);
        let cf = callee.finish();

        let mut func_table = HashMap::new();
        func_table.insert(FuncRef(0), cf);

        // caller calls square(5)
        let mut b = FunctionBuilder::new("caller", Signature::new(&[], &[Type::I32]));
        let entry = b.create_block();
        b.switch_to_block(entry);
        let c5 = b.iconst_i32(5);
        let call_res = b.call(FuncRef(0), &[c5], &[Type::I32]);
        b.return_(&[call_res[0]]);

        let mut caller = b.finish();
        let pass = FuncSpecializePass::new(func_table);
        pass.run_on_function(&mut caller).unwrap();
        assert!(caller.validate().is_valid());
        // After specialization + inline, the call should be replaced
        let has_call = caller.blocks[0]
            .instructions
            .iter()
            .any(|i| matches!(i.opcode, Opcode::Call { .. }));
        assert!(!has_call, "Call should be inlined after specialization");
    }
}
