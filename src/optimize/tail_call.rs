//! 尾调用优化 (TCO) pass。
//!
//! 检测尾调用模式 `v = call f(args); return v` 并将其转换为
//! 跳转到被调用函数入口块的 Jump，消除 call/ret 开销。
//!
//! # 算法
//!
//! 1. 扫描每个基本块的终止指令
//! 2. 如果终止指令是 Return，检查返回值是否来自该块最后一条 Call 指令
//! 3. 如果被调用函数在当前模块中（同模块尾调用），替换为 Jump
//! 4. 如果调用的是自身（尾递归），特别标记为自尾递归
//!
//! # 限制
//!
//! - 仅优化同模块内的调用
//! - 不修改调用约定（保留参数传递方式）
//! - 被调用函数必须与调用者有兼容的签名

use crate::CompileError;
use crate::ir::*;
use crate::optimize::{OptimizationPass, PassResult};
use std::collections::HashMap;

/// 尾调用优化 pass。
pub struct TailCallPass {
    /// 模块内可用的函数表 (FuncRef → Function)。
    function_table: HashMap<FuncRef, Function>,
}

impl TailCallPass {
    pub fn new(function_table: HashMap<FuncRef, Function>) -> Self {
        Self { function_table }
    }
}

impl OptimizationPass for TailCallPass {
    fn name(&self) -> &'static str {
        "tail-call"
    }

    fn description(&self) -> &'static str {
        "Tail call optimization: converts tail calls into jumps to eliminate call/ret overhead"
    }

    fn run_on_function(&self, func: &mut Function) -> Result<PassResult, CompileError> {
        optimize_tail_calls(func, &self.function_table)
    }
}

/// 对单个函数执行尾调用优化。
pub fn optimize_tail_calls(
    func: &mut Function,
    function_table: &HashMap<FuncRef, Function>,
) -> Result<PassResult, CompileError> {
    let mut result = PassResult::default();

    for block_idx in 0..func.blocks.len() {
        let block = &func.blocks[block_idx];
        let _block_id = block.id;

        // 检查块是否以 Return 结束
        let return_values = match &block.terminator {
            Terminator::Return { values } => values.clone(),
            _ => continue,
        };

        if return_values.is_empty() {
            continue;
        }

        // 查找该块最后一条 Call 指令
        let (call_idx, call_func, call_args, call_result) =
            match find_tail_call(&block.instructions) {
                Some(v) => v,
                None => continue,
            };

        // 检查 Return 的值是否是 Call 的结果
        let call_result_val = match call_result {
            Some(v) => v,
            None => continue,
        };

        if return_values.len() != 1 || return_values[0] != call_result_val {
            continue;
        }

        // 查找被调用函数
        let callee = match function_table.get(&call_func) {
            Some(f) => f.clone(),
            None => continue, // 外部函数，无法尾调用优化
        };

        // 验证签名兼容性（参数数量相同）
        if call_args.len() != callee.signature.params.len() {
            continue;
        }

        // 执行尾调用转换：
        // 1. 移除 Call 指令（替换为 Nop）
        // 2. 将 Return 替换为 Jump 到被调用函数的入口块
        let entry_block_id = callee.blocks[0].id;

        // 注意：被调用函数的 entry block 参数需要匹配
        // 对于自递归，Jump 到自己的 entry block
        let block = &mut func.blocks[block_idx];
        block.instructions[call_idx] =
            Instruction::new(Opcode::Nop, smallvec::smallvec![], None, Type::Void);
        let jump_args: smallvec::SmallVec<[Value; 2]> = call_args.into_iter().collect();
        block.terminator = Terminator::Jump {
            target: entry_block_id,
            args: jump_args,
        };

        result.instructions_removed += 1;
        result.changed = true;
    }

    Ok(result)
}

/// 查找块中可作为尾调用的 Call 指令。
///
/// 返回 (指令索引, 被调用 FuncRef, 调用参数, 调用结果 Value)。
fn find_tail_call(
    instructions: &[Instruction],
) -> Option<(
    usize,
    FuncRef,
    smallvec::SmallVec<[Value; 4]>,
    Option<Value>,
)> {
    // 从后向前查找 Call 指令（跳过 Nop）
    for (i, inst) in instructions.iter().enumerate().rev() {
        if matches!(inst.opcode, Opcode::Nop) {
            continue;
        }
        if let Opcode::Call { func } = &inst.opcode {
            return Some((i, *func, inst.operands.clone(), inst.result));
        }
        // 如果最后一条非 Nop 指令不是 Call，则没有尾调用
        break;
    }
    None
}

// ============================================================
// 测试
// ============================================================

#[cfg(test)]
mod tests {
    use super::*;
    use crate::optimize::OptimizationPass;

    fn make_func_table(funcs: &[Function]) -> HashMap<FuncRef, Function> {
        funcs
            .iter()
            .enumerate()
            .map(|(i, f)| (FuncRef(i as u32), f.clone()))
            .collect()
    }

    #[test]
    fn tail_recursion_self() {
        // fn countdown(n: i32) -> i32 {
        //     if n <= 0 { return 0; }
        //     return countdown(n - 1);  // tail call!
        // }
        let sig = Signature::new(&[(Type::I32, "n")], &[Type::I32]);
        let mut b = FunctionBuilder::new("countdown", sig);
        let (entry, params) = b.create_block_with_params(&[(Type::I32, "n")]);
        let (recursive, rec_params) = b.create_block_with_params(&[(Type::I32, "n")]);
        let ret_zero = b.create_block();

        b.switch_to_block(entry);
        b.jump(recursive, &[params[0]]);

        b.switch_to_block(recursive);
        let n = rec_params[0];
        let zero = b.iconst_i32(0);
        let cmp = b.icmp(IntCC::SignedGreaterThan, n, zero);
        b.branch(cmp, entry, ret_zero, &[], &[]); // placeholder, will modify

        b.switch_to_block(ret_zero);
        b.return_(&[zero]);

        let mut func = b.finish();

        // Rebuild: make the recursive block do a tail call
        let recursive_idx = 1;
        func.blocks[recursive_idx].instructions.clear();
        let n = func.blocks[recursive_idx].params[0].0;
        let one = func.create_value();
        let one_idx = func.constant_pool.insert(crate::ir::Big::from_i64(1));
        func.blocks[recursive_idx]
            .instructions
            .push(Instruction::new(
                Opcode::Iconst { index: one_idx },
                smallvec::smallvec![],
                Some(one),
                Type::I32,
            ));
        let n_minus_1 = func.create_value();
        func.blocks[recursive_idx]
            .instructions
            .push(Instruction::new(
                Opcode::Isub,
                smallvec::smallvec![n, one],
                Some(n_minus_1),
                Type::I32,
            ));
        let call_result = func.create_value();
        func.blocks[recursive_idx]
            .instructions
            .push(Instruction::new(
                Opcode::Call { func: FuncRef(0) },
                smallvec::smallvec![n_minus_1],
                Some(call_result),
                Type::I32,
            ));
        func.blocks[recursive_idx].terminator = Terminator::Return {
            values: smallvec::smallvec![call_result],
        };

        let table = make_func_table(&[func.clone()]);
        let pass = TailCallPass::new(table);
        let mut opt_func = func.clone();
        let r = pass.run_on_function(&mut opt_func).unwrap();

        assert!(r.changed);
        // Now the recursive block should end with Jump, not Return
        let recursive_block = &opt_func.blocks[1];
        assert!(
            matches!(recursive_block.terminator, Terminator::Jump { .. }),
            "Expected Jump terminator after TCO, got {:?}",
            recursive_block.terminator
        );
    }

    #[test]
    fn non_tail_call_unchanged() {
        // fn foo(x: i32) -> i32 {
        //     let v = bar(x);
        //     return v + 1;  // NOT a tail call (additional operation)
        // }
        let sig = Signature::new(&[(Type::I32, "x")], &[Type::I32]);
        let mut b = FunctionBuilder::new("foo", sig);
        let (entry, params) = b.create_block_with_params(&[(Type::I32, "x")]);
        b.switch_to_block(entry);
        let call_results = b.call(FuncRef(1), &[params[0]], &[Type::I32]);
        let one = b.iconst_i32(1);
        let sum = b.iadd(call_results[0], one);
        b.return_(&[sum]);

        let func = b.finish();
        let callee_sig = Signature::new(&[(Type::I32, "x")], &[Type::I32]);
        let callee = Function::new("bar", callee_sig);
        let table = make_func_table(&[func.clone(), callee]);

        let pass = TailCallPass::new(table);
        let mut opt_func = func.clone();
        let r = pass.run_on_function(&mut opt_func).unwrap();

        assert!(!r.changed); // Should not change non-tail calls
    }

    #[test]
    fn tail_call_to_other_function() {
        // fn wrapper(x: i32) -> i32 {
        //     return helper(x);  // tail call to other function
        // }
        let sig = Signature::new(&[(Type::I32, "x")], &[Type::I32]);
        let mut b = FunctionBuilder::new("wrapper", sig);
        let (entry, params) = b.create_block_with_params(&[(Type::I32, "x")]);
        b.switch_to_block(entry);
        let call_result = b.call(FuncRef(1), &[params[0]], &[Type::I32]);
        b.return_(&call_result);

        let func = b.finish();
        let callee_sig = Signature::new(&[(Type::I32, "x")], &[Type::I32]);
        let mut callee_builder = FunctionBuilder::new("helper", callee_sig);
        let (callee_entry, callee_params) =
            callee_builder.create_block_with_params(&[(Type::I32, "x")]);
        callee_builder.switch_to_block(callee_entry);
        callee_builder.return_(&[callee_params[0]]);
        let callee = callee_builder.finish();

        let table = make_func_table(&[func.clone(), callee.clone()]);
        let pass = TailCallPass::new(table);
        let mut opt_func = func.clone();
        let r = pass.run_on_function(&mut opt_func).unwrap();

        assert!(r.changed);
        // Should jump to helper's entry block
        let entry_block = &opt_func.blocks[0];
        assert!(matches!(entry_block.terminator, Terminator::Jump { .. }));
    }
}
