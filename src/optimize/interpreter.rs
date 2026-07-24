//! IR 解释器 — 编译时求值 const 函数。
//!
//! 实现了 Zig 风格的核心洞察：编译时求值就是解释执行。
//! 同一个 Function 既可以被 FunctionCompiler 编译为机器码（运行时），
//! 也可以被 IrInterpreter 解释执行（编译时）。
//!
//! # 使用场景
//!
//! 当 lowering 遇到 `Call { func }` 指令，且：
//! 1. 被调用函数标记为 `is_const == true`
//! 2. 所有实参都是编译时已知的常量
//!
//! 则 IrInterpreter 可以直接求值该调用，生成 Iconst/Fconst 而非 Call 机器指令。

use crate::CompileError;
use crate::ir::*;
use crate::optimize::const_fold::{ConstValue, fold_opcode};
use crate::optimize::{OptimizationPass, PassResult};
use std::collections::HashMap;

/// 默认最大解释步数（防止 const 函数中的无限循环）。
pub const DEFAULT_MAX_STEPS: u64 = 10_000;

/// 解释执行结果。
#[derive(Clone, Debug)]
enum StepResult {
    /// 继续执行下一条指令。
    Continue,
    /// 函数返回。
    Return(Option<ConstValue>),
    /// 操作在编译时无法执行（回退到运行时调用）。
    Unsupported,
}

/// IR 解释器 — 在编译时执行 const 函数。
///
/// # 安全特性
///
/// - **步数限制**：防止 const 函数中的无限循环导致编译器挂起
/// - **优雅回退**：遇到无法编译时求值的操作时返回 `Ok(None)`，保留运行时调用
/// - **错误传播**：编译时除零等真正错误返回 `Err`
pub struct IrInterpreter {
    /// Value → ConstValue 映射（解释器的"寄存器"）。
    values: HashMap<Value, ConstValue>,
    /// 常量函数表（用于递归 const 调用）。
    const_functions: HashMap<FuncRef, Function>,
    /// 剩余步数（防止无限循环）。
    remaining_steps: u64,
}

impl IrInterpreter {
    /// 创建一个新的解释器实例。
    pub fn new(const_functions: HashMap<FuncRef, Function>) -> Self {
        Self {
            values: HashMap::new(),
            const_functions,
            remaining_steps: DEFAULT_MAX_STEPS,
        }
    }

    /// 设置最大步数限制。
    pub fn set_max_steps(&mut self, steps: u64) {
        self.remaining_steps = steps;
    }

    /// 以常量参数执行一个 const 函数，返回结果。
    ///
    /// # Returns
    ///
    /// * `Ok(Some(value))` — 编译时求值成功
    /// * `Ok(None)` — 函数无法编译时求值（不是 const，或包含不支持的操作）
    /// * `Err(...)` — 编译时错误（除零、无限循环等）
    pub fn evaluate(
        func: &Function,
        args: &[ConstValue],
        const_functions: &HashMap<FuncRef, Function>,
    ) -> Result<Option<ConstValue>, CompileError> {
        if !func.is_const {
            return Ok(None);
        }

        let mut interpreter = Self::new(const_functions.clone());

        // 绑定实参到入口块参数
        if let Some(entry) = func.entry_block() {
            for (i, (param_val, _)) in entry.params.iter().enumerate() {
                if i < args.len() {
                    interpreter.values.insert(*param_val, args[i].clone());
                }
            }
        }

        // 从 entry block 开始解释执行
        match interpreter.execute_block(func, BlockId(0)) {
            Ok(StepResult::Return(val)) => Ok(val),
            Ok(StepResult::Unsupported) => Ok(None),
            Ok(StepResult::Continue) => Ok(None), // 无返回语句
            Err(e) => Err(e),
        }
    }

    /// 消耗一个步数。返回 `Err` 如果超出限制。
    fn consume_step(&mut self) -> Result<(), CompileError> {
        if self.remaining_steps == 0 {
            return Err(CompileError::Internal(
                "const function exceeded maximum step limit (possible infinite loop)".into(),
            ));
        }
        self.remaining_steps -= 1;
        Ok(())
    }

    /// 执行一个基本块及其后继。
    fn execute_block(
        &mut self,
        func: &Function,
        start_block: BlockId,
    ) -> Result<StepResult, CompileError> {
        self.consume_step()?;

        let block = func
            .block(start_block)
            .ok_or_else(|| CompileError::Internal(format!("block {} not found", start_block)))?;

        // 执行块内指令
        for inst in &block.instructions {
            match self.execute_instruction(func, inst)? {
                StepResult::Continue => {}
                other => return Ok(other),
            }
        }

        // 处理终止指令
        self.execute_terminator(func, &block.terminator)
    }

    /// 执行单条指令。
    fn execute_instruction(
        &mut self,
        func: &Function,
        inst: &Instruction,
    ) -> Result<StepResult, CompileError> {
        // 收集 operands 的常量值
        let const_ops: Vec<ConstValue> = inst
            .operands
            .iter()
            .filter_map(|v| self.values.get(v).cloned())
            .collect();

        // 如果是 Call 到 const 函数，尝试递归解释执行
        if let Opcode::Call { func: callee_func } = &inst.opcode {
            if let Some(const_func) = self.const_functions.get(callee_func)
                && const_func.is_const
                && const_ops.len() == inst.operands.len()
            {
                match IrInterpreter::evaluate(const_func, &const_ops, &self.const_functions)? {
                    Some(result_val) => {
                        if let Some(v) = inst.result {
                            self.values.insert(v, result_val);
                        }
                        return Ok(StepResult::Continue);
                    }
                    None => {
                        return Ok(StepResult::Unsupported);
                    }
                }
            }
            // 非 const 函数 → 无法编译时求值
            return Ok(StepResult::Unsupported);
        }

        // 不可编译时求值的操作 → 回退
        match &inst.opcode {
            Opcode::Load
            | Opcode::Store
            | Opcode::CallIndirect
            | Opcode::StackLoad { .. }
            | Opcode::StackStore { .. }
            | Opcode::StackAddr { .. }
            | Opcode::GlobalAddr { .. } => {
                return Ok(StepResult::Unsupported);
            }
            _ => {}
        }

        // 如果所有 operands 都是常量，折叠求值
        if const_ops.len() == inst.operands.len() && !inst.operands.is_empty() {
            match fold_opcode(&inst.opcode, &const_ops, inst.ty)? {
                Some(folded) => {
                    if let Some(v) = inst.result {
                        self.values.insert(v, folded);
                    }
                    return Ok(StepResult::Continue);
                }
                None => {
                    // fold_opcode 返回 None 意味着操作码不可折叠（Nop, Iconst, Fconst 等）
                    // 继续后面的处理
                }
            }
        }

        // 处理特殊情况
        match &inst.opcode {
            Opcode::Iconst { index } => {
                if let Some(v) = inst.result {
                    let big = func
                        .constant_pool
                        .get(*index)
                        .cloned()
                        .unwrap_or(Big::S_ZERO);
                    self.values.insert(v, ConstValue::Int(big, inst.ty));
                }
            }
            Opcode::Fconst { index } => {
                if let Some(v) = inst.result {
                    let big = func.constant_pool.get(*index).cloned().unwrap_or_else(|| {
                        let fmt = inst.ty.float_format().unwrap_or(FloatFormat::F64);
                        Big::from_bits(0, fmt)
                    });
                    self.values.insert(v, ConstValue::Float(big, inst.ty));
                }
            }
            Opcode::Copy => {
                // 传播被复制的值
                if let Some(source) = inst.operands.first()
                    && let Some(val) = self.values.get(source).cloned()
                    && let Some(v) = inst.result
                {
                    self.values.insert(v, val);
                }
            }
            Opcode::Phi { .. } => {
                // Phi 节点：所有操作数的值应该相同（由 const 折叠保证）
                // 或在块参数绑定中已经设置
                if let Some(first_op) = inst.operands.first()
                    && let Some(val) = self.values.get(first_op).cloned()
                    && let Some(v) = inst.result
                {
                    self.values.insert(v, val);
                }
            }
            Opcode::Nop => {}
            _ => {
                // 遇到不确定的情况（部分 operands 不是常量）→ 回退
                if const_ops.len() < inst.operands.len() {
                    return Ok(StepResult::Unsupported);
                }
            }
        }

        Ok(StepResult::Continue)
    }

    /// 执行终止指令，决定下一个要执行的块。
    fn execute_terminator(
        &mut self,
        func: &Function,
        term: &Terminator,
    ) -> Result<StepResult, CompileError> {
        match term {
            Terminator::Return { values } => {
                if values.is_empty() {
                    return Ok(StepResult::Return(None));
                }
                // 返回第一个值
                let val = self.values.get(&values[0]).cloned();
                Ok(StepResult::Return(val))
            }
            Terminator::Jump { target, args } => {
                self.bind_block_args(func, *target, args.as_slice())?;
                self.execute_block(func, *target)
            }
            Terminator::Branch {
                cond,
                true_block,
                false_block,
                true_args,
                false_args,
            } => {
                let cond_val = match self.values.get(cond) {
                    Some(v) => v,
                    None => return Ok(StepResult::Unsupported),
                };

                let is_true = match cond_val.to_bool() {
                    Some(b) => b,
                    None => return Ok(StepResult::Unsupported),
                };

                let (target, args) = if is_true {
                    (*true_block, true_args.as_slice())
                } else {
                    (*false_block, false_args.as_slice())
                };

                self.bind_block_args(func, target, args)?;
                self.execute_block(func, target)
            }
            Terminator::Unreachable => Err(CompileError::Internal(
                "reached unreachable block in const function".into(),
            )),
            Terminator::Switch {
                discriminant,
                default_block,
                cases,
            } => {
                let disc_val = match self.values.get(discriminant) {
                    Some(ConstValue::Int(v, _)) => v.clone(),
                    _ => return Ok(StepResult::Unsupported),
                };

                // 查找匹配的 case
                let (target, args) = cases
                    .iter()
                    .find(|(case_val, _, _)| Big::from_i64(*case_val) == disc_val)
                    .map(|(_, target, args)| (*target, args.as_slice()))
                    .unwrap_or((*default_block, &[][..]));

                self.bind_block_args(func, target, args)?;
                self.execute_block(func, target)
            }
        }
    }

    /// 将跳转参数绑定到目标块的参数。
    fn bind_block_args(
        &mut self,
        func: &Function,
        target: BlockId,
        args: &[Value],
    ) -> Result<(), CompileError> {
        if let Some(target_block) = func.block(target) {
            for (i, (param_val, _)) in target_block.params.iter().enumerate() {
                if i < args.len()
                    && let Some(val) = self.values.get(&args[i]).cloned()
                {
                    self.values.insert(*param_val, val);
                }
            }
        }
        Ok(())
    }
}

// ============================================================
// ConstFnEvalPass — 在编译时求值 const 函数调用
// ============================================================

/// 常量函数求值 pass。
///
/// 迭代扫描 IR 中的 `Call { func }` 指令，如果：
/// 1. 被调用函数是 const（`is_const == true`）
/// 2. 所有实参是编译时已知常量
///
/// 则使用 IrInterpreter 编译时求值，将 Call 替换为 Iconst/Fconst。
/// 迭代直到不动点（前一次求值可能使后续调用变为常量化）。
///
/// # 使用
///
/// ```ignore
/// let mut pass = ConstFnEvalPass::new(const_functions);
/// pass.run_on_function(&mut func)?;
/// ```
pub struct ConstFnEvalPass {
    const_functions: HashMap<FuncRef, Function>,
}

impl ConstFnEvalPass {
    /// 创建一个新的 const fn 求值 pass。
    pub fn new(const_functions: HashMap<FuncRef, Function>) -> Self {
        Self { const_functions }
    }

    /// 获取函数表的引用。
    pub fn functions(&self) -> &HashMap<FuncRef, Function> {
        &self.const_functions
    }
}

impl OptimizationPass for ConstFnEvalPass {
    fn name(&self) -> &'static str {
        "const-fn-eval"
    }

    fn description(&self) -> &'static str {
        "Evaluates calls to const functions with constant arguments at compile time"
    }

    fn run_on_function(&self, func: &mut Function) -> Result<PassResult, CompileError> {
        evaluate_const_calls(func, &self.const_functions)
    }
}

/// 扫描并求值函数中的所有 const 调用（迭代到不动点）。
pub fn evaluate_const_calls(
    func: &mut Function,
    const_functions: &HashMap<FuncRef, Function>,
) -> Result<PassResult, CompileError> {
    let mut result = PassResult::default();
    let mut changed = true;

    while changed {
        changed = false;

        // 扫描已知常量（包括之前迭代中新产生的常量）
        let mut known: HashMap<Value, ConstValue> = HashMap::new();
        for block in func.blocks.iter() {
            for inst in &block.instructions {
                match &inst.opcode {
                    Opcode::Iconst { index } => {
                        if let Some(v) = inst.result {
                            let big = func
                                .constant_pool
                                .get(*index)
                                .cloned()
                                .unwrap_or(Big::S_ZERO);
                            known.insert(v, ConstValue::Int(big, inst.ty));
                        }
                    }
                    Opcode::Fconst { index } => {
                        if let Some(v) = inst.result {
                            let big =
                                func.constant_pool.get(*index).cloned().unwrap_or_else(|| {
                                    let fmt = inst.ty.float_format().unwrap_or(FloatFormat::F64);
                                    Big::from_bits(0, fmt)
                                });
                            known.insert(v, ConstValue::Float(big, inst.ty));
                        }
                    }
                    _ => {}
                }
            }
        }

        // 遍历所有 Call 指令
        for block in func.blocks.iter_mut() {
            for inst in block.instructions.iter_mut() {
                if let Opcode::Call { func: callee } = inst.opcode
                    && let Some(const_func) = const_functions.get(&callee)
                    && const_func.is_const
                {
                    let const_args: Vec<ConstValue> = inst
                        .operands
                        .iter()
                        .filter_map(|v| known.get(v).cloned())
                        .collect();

                    if const_args.len() == inst.operands.len() {
                        match IrInterpreter::evaluate(const_func, &const_args, const_functions)? {
                            Some(eval_result) => {
                                // 替换 Call 为常量（插入常量池）
                                let (new_opcode, new_ty) = match &eval_result {
                                    ConstValue::Int(v, t) => {
                                        let index = func.constant_pool.insert(v.clone());
                                        (Opcode::Iconst { index }, *t)
                                    }
                                    ConstValue::Float(v, t) => {
                                        let index = func.constant_pool.insert(v.clone());
                                        (Opcode::Fconst { index }, *t)
                                    }
                                    ConstValue::Bool(b) => {
                                        let big = Big::from_i64(if *b { 1 } else { 0 });
                                        let index = func.constant_pool.insert(big);
                                        (Opcode::Iconst { index }, Type::I32)
                                    }
                                };
                                inst.opcode = new_opcode;
                                inst.ty = new_ty;
                                inst.operands.clear();

                                if let Some(v) = inst.result {
                                    known.insert(v, eval_result);
                                }

                                result.instructions_removed += 1;
                                result.changed = true;
                                changed = true;
                            }
                            None => {
                                // 无法编译时求值 → 保留 Call
                            }
                        }
                    }
                }
            }
        }
    }

    Ok(result)
}

// ============================================================
// 测试
// ============================================================

#[cfg(test)]
mod tests {
    use super::*;

    /// 辅助：创建设置了 is_const 标志的简单函数。
    fn make_const_func(name: &str, sig: Signature) -> FunctionBuilder {
        let mut b = FunctionBuilder::new(name, sig);
        b.set_const(true);
        b
    }

    // === IrInterpreter 测试 ===

    #[test]
    fn interpret_simple_const_add() {
        // const fn add(a: i32, b: i32) -> i32 { a + b }
        let sig = Signature::new(&[(Type::I32, "a"), (Type::I32, "b")], &[Type::I32]);
        let mut b = make_const_func("add", sig);
        let (entry, params) = b.create_block_with_params(&[(Type::I32, "a"), (Type::I32, "b")]);
        b.switch_to_block(entry);
        let sum = b.iadd(params[0], params[1]);
        b.return_(&[sum]);
        let func = b.finish();

        let result = IrInterpreter::evaluate(
            &func,
            &[
                ConstValue::Int(Big::from_i64(3), Type::I32),
                ConstValue::Int(Big::from_i64(5), Type::I32),
            ],
            &HashMap::new(),
        )
        .unwrap();
        assert_eq!(result, Some(ConstValue::Int(Big::from_i64(8), Type::I32)));
    }

    #[test]
    fn interpret_const_fn_with_branch() {
        // const fn max(a: i32, b: i32) -> i32 { if a > b { a } else { b } }
        let sig = Signature::new(&[(Type::I32, "a"), (Type::I32, "b")], &[Type::I32]);
        let mut b = make_const_func("max", sig);
        let (entry, params) = b.create_block_with_params(&[(Type::I32, "a"), (Type::I32, "b")]);
        let then_block = b.create_block();
        let else_block = b.create_block();

        b.switch_to_block(entry);
        let cond = b.icmp(IntCC::SignedGreaterThan, params[0], params[1]);
        b.branch(cond, then_block, else_block, &[params[0]], &[params[1]]);

        b.switch_to_block(then_block);
        b.return_(&[params[0]]);

        b.switch_to_block(else_block);
        b.return_(&[params[1]]);

        let func = b.finish();

        // max(10, 3) = 10
        let result = IrInterpreter::evaluate(
            &func,
            &[
                ConstValue::Int(Big::from_i64(10), Type::I32),
                ConstValue::Int(Big::from_i64(3), Type::I32),
            ],
            &HashMap::new(),
        )
        .unwrap();
        assert_eq!(result, Some(ConstValue::Int(Big::from_i64(10), Type::I32)));

        // max(3, 10) = 10
        let result = IrInterpreter::evaluate(
            &func,
            &[
                ConstValue::Int(Big::from_i64(3), Type::I32),
                ConstValue::Int(Big::from_i64(10), Type::I32),
            ],
            &HashMap::new(),
        )
        .unwrap();
        assert_eq!(result, Some(ConstValue::Int(Big::from_i64(10), Type::I32)));
    }

    #[test]
    fn interpret_const_fn_with_loop_limited() {
        // const fn that loops: while true { ... } → should hit step limit
        let sig = Signature::new(&[], &[Type::I32]);
        let mut b = make_const_func("loop_forever", sig);
        let loop_block = b.create_block();

        b.switch_to_block(loop_block);
        // 无条件自循环：Jump 到自身
        b.jump(loop_block, &[]); // no terminator needed, jump suffices

        // 换一种方式：用恒真分支循环
        let func = {
            let sig = Signature::new(&[], &[Type::I32]);
            let mut b = FunctionBuilder::new("loop_test", sig);
            b.set_const(true);
            let entry = b.create_block();
            b.switch_to_block(entry);
            let one = b.iconst_i32(1);
            let cmp = b.icmp(IntCC::Equal, one, one); // always true
            b.branch(cmp, entry, entry, &[], &[]); // infinite loop
            b.finish()
        };

        // 设置很小的步数限制
        let mut interpreter = IrInterpreter::new(HashMap::new());
        interpreter.set_max_steps(100);

        // 绑定实参后直接执行
        let result = interpreter.execute_block(&func, BlockId(0));
        assert!(result.is_err());
        assert!(result.unwrap_err().to_string().contains("step limit"));
    }

    #[test]
    fn interpret_non_const_fn_returns_none() {
        let sig = Signature::new(&[], &[Type::I32]);
        let mut b = FunctionBuilder::new("not_const", sig);
        // is_const = false (default)
        let entry = b.create_block();
        b.switch_to_block(entry);
        let v = b.iconst_i32(1);
        b.return_(&[v]);
        let func = b.finish();

        assert!(!func.is_const);
        let result = IrInterpreter::evaluate(&func, &[], &HashMap::new()).unwrap();
        assert_eq!(result, None); // 不是 const 函数，返回 None
    }

    #[test]
    fn interpret_unsupported_operation_returns_none() {
        // const fn that tries to Load — should return None
        let sig = Signature::new(&[(Type::Ptr, "p")], &[Type::I32]);
        let mut b = make_const_func("load_test", sig);
        let (entry, params) = b.create_block_with_params(&[(Type::Ptr, "p")]);
        b.switch_to_block(entry);
        let v = b.load(params[0], Type::I32); // Load 在编译时无法执行
        b.return_(&[v]);
        let func = b.finish();

        let result = IrInterpreter::evaluate(
            &func,
            &[ConstValue::Int(Big::from_i64(0), Type::Ptr)], // 模拟指针
            &HashMap::new(),
        )
        .unwrap();
        assert_eq!(result, None); // 应该回退
    }

    #[test]
    fn interpret_division_by_zero_error() {
        let sig = Signature::new(&[], &[Type::I32]);
        let mut b = make_const_func("div_zero", sig);
        let entry = b.create_block();
        b.switch_to_block(entry);
        let a = b.iconst_i32(10);
        let zero = b.iconst_i32(0);
        let result = b.sdiv(a, zero);
        b.return_(&[result]);
        let func = b.finish();

        let result = IrInterpreter::evaluate(&func, &[], &HashMap::new());
        assert!(result.is_err()); // 除零是编译错误
    }

    #[test]
    fn interpret_copy_handling() {
        // const fn f() -> i32 { let x = 42; let y = x; return y; }
        let sig = Signature::new(&[], &[Type::I32]);
        let mut b = make_const_func("copy_test", sig);
        let entry = b.create_block();
        b.switch_to_block(entry);
        let x = b.iconst_i32(42);
        let y = b.copy(x);
        b.return_(&[y]);
        let func = b.finish();

        let result = IrInterpreter::evaluate(&func, &[], &HashMap::new()).unwrap();
        assert_eq!(result, Some(ConstValue::Int(Big::from_i64(42), Type::I32)));
    }

    // === ConstFnEvalPass 测试 ===

    #[test]
    fn eval_pass_evaluates_const_call() {
        // const fn answer() -> i32 { 42 }
        let sig = Signature::new(&[], &[Type::I32]);
        let mut b = FunctionBuilder::new("answer", sig);
        b.set_const(true);
        let entry = b.create_block();
        b.switch_to_block(entry);
        let v = b.iconst_i32(42);
        b.return_(&[v]);
        let answer_fn = b.finish();

        // fn caller() -> i32 { answer() + 1 }
        let mut b2 = FunctionBuilder::new("caller", Signature::new(&[], &[Type::I32]));
        let entry2 = b2.create_block();
        b2.switch_to_block(entry2);
        let call_result = b2.call(FuncRef(0), &[], &[Type::I32]);
        let one = b2.iconst_i32(1);
        let result = b2.iadd(call_result[0], one);
        b2.return_(&[result]);
        let mut caller_fn = b2.finish();

        let mut func_table = HashMap::new();
        func_table.insert(FuncRef(0), answer_fn);

        let pass = ConstFnEvalPass::new(func_table);
        let r = pass.run_on_function(&mut caller_fn).unwrap();
        assert!(r.changed);

        // Call 应该被替换为 Iconst(42)
        let call_inst = &caller_fn.blocks[0].instructions[0];
        if let Opcode::Iconst { index } = call_inst.opcode {
            assert_eq!(
                caller_fn
                    .constant_pool
                    .get(index)
                    .unwrap()
                    .try_to_i64()
                    .unwrap(),
                42
            );
        } else {
            panic!("Expected Iconst, got {:?}", call_inst.opcode);
        }
    }

    #[test]
    fn eval_pass_non_const_args_preserves_call() {
        // const fn add(a: i32, b: i32) -> i32 { a + b }
        let sig = Signature::new(&[(Type::I32, "a"), (Type::I32, "b")], &[Type::I32]);
        let mut b = FunctionBuilder::new("add", sig);
        b.set_const(true);
        let (entry, params) = b.create_block_with_params(&[(Type::I32, "a"), (Type::I32, "b")]);
        b.switch_to_block(entry);
        let sum = b.iadd(params[0], params[1]);
        b.return_(&[sum]);
        let add_fn = b.finish();

        // fn caller(x: i32) -> i32 { add(x, 5) } — x is runtime
        let sig2 = Signature::new(&[(Type::I32, "x")], &[Type::I32]);
        let mut b2 = FunctionBuilder::new("caller", sig2);
        let (entry2, params2) = b2.create_block_with_params(&[(Type::I32, "x")]);
        b2.switch_to_block(entry2);
        let x = params2[0];
        let c5 = b2.iconst_i32(5);
        let call_result = b2.call(FuncRef(0), &[x, c5], &[Type::I32]);
        b2.return_(&[call_result[0]]);
        let mut caller_fn = b2.finish();

        let mut func_table = HashMap::new();
        func_table.insert(FuncRef(0), add_fn);

        let pass = ConstFnEvalPass::new(func_table);
        let r = pass.run_on_function(&mut caller_fn).unwrap();
        assert!(!r.changed); // x is not const, so call should stay
        // Call is instruction at index 1 (after iconst(5))
        assert!(matches!(
            caller_fn.blocks[0].instructions[1].opcode,
            Opcode::Call { .. }
        ));
    }

    #[test]
    fn eval_pass_recursive_const_fn_base_case() {
        // Test factorial(1) — base case, no recursion needed
        let sig = Signature::new(&[(Type::I32, "n")], &[Type::I32]);
        let mut b = FunctionBuilder::new("factorial", sig);
        b.set_const(true);
        let (entry, params) = b.create_block_with_params(&[(Type::I32, "n")]);
        let then_block = b.create_block();
        let else_block = b.create_block();

        b.switch_to_block(entry);
        let n = params[0];
        let one = b.iconst_i32(1);
        let cmp = b.icmp(IntCC::SignedLessThanOrEqual, n, one);
        b.branch(cmp, then_block, else_block, &[], &[]);

        b.switch_to_block(then_block);
        b.return_(&[one]);

        b.switch_to_block(else_block);
        let n_minus_1 = b.isub(n, one);
        let _recurse = b.call(FuncRef(0), &[n_minus_1], &[Type::I32]);
        let test_val = b.iconst_i32(0);
        b.return_(&[test_val]);

        let factorial_fn = b.finish();

        let mut b2 = FunctionBuilder::new("caller", Signature::new(&[], &[Type::I32]));
        let entry2 = b2.create_block();
        b2.switch_to_block(entry2);
        let c1 = b2.iconst_i32(1);
        let call_result = b2.call(FuncRef(0), &[c1], &[Type::I32]);
        b2.return_(&[call_result[0]]);
        let mut caller_fn = b2.finish();

        let mut func_table = HashMap::new();
        func_table.insert(FuncRef(0), factorial_fn);

        let pass = ConstFnEvalPass::new(func_table);
        let r = pass.run_on_function(&mut caller_fn).unwrap();
        assert!(r.changed);

        // factorial(1) should evaluate to 1 (base case in then_block)
        let call_inst = &caller_fn.blocks[0].instructions[0];
        if let Opcode::Iconst { index } = call_inst.opcode {
            assert_eq!(
                caller_fn
                    .constant_pool
                    .get(index)
                    .unwrap()
                    .try_to_i64()
                    .unwrap(),
                1
            );
        } else {
            panic!("Expected Iconst, got {:?}", call_inst.opcode);
        }
    }

    #[test]
    fn eval_pass_recursive_const_fn() {
        // const fn factorial(n: i32) -> i32 {
        //     if n <= 1 { return 1; }
        //     else { return n * factorial(n - 1); }
        // }
        let sig = Signature::new(&[(Type::I32, "n")], &[Type::I32]);
        let mut b = FunctionBuilder::new("factorial", sig);
        b.set_const(true);
        let (entry, params) = b.create_block_with_params(&[(Type::I32, "n")]);
        let then_block = b.create_block();
        let else_block = b.create_block();

        b.switch_to_block(entry);
        let n = params[0];
        let one = b.iconst_i32(1);
        let cmp = b.icmp(IntCC::SignedLessThanOrEqual, n, one);
        b.branch(cmp, then_block, else_block, &[], &[]);

        b.switch_to_block(then_block);
        b.return_(&[one]);

        b.switch_to_block(else_block);
        let n_minus_1 = b.isub(n, one);
        let recurse = b.call(FuncRef(0), &[n_minus_1], &[Type::I32]); // 递归
        let prod = b.imul(n, recurse[0]);
        b.return_(&[prod]);

        let factorial_fn = b.finish();

        // fn caller() -> i32 { factorial(5) }
        let mut b2 = FunctionBuilder::new("caller", Signature::new(&[], &[Type::I32]));
        let entry2 = b2.create_block();
        b2.switch_to_block(entry2);
        let c5 = b2.iconst_i32(5);
        let call_result = b2.call(FuncRef(0), &[c5], &[Type::I32]);
        b2.return_(&[call_result[0]]);
        let mut caller_fn = b2.finish();

        let mut func_table = HashMap::new();
        func_table.insert(FuncRef(0), factorial_fn);

        let pass = ConstFnEvalPass::new(func_table);
        let r = pass.run_on_function(&mut caller_fn).unwrap();
        assert!(r.changed);

        // factorial(5) = 120 (Call is at index 1, after Iconst(5) at index 0)
        let call_inst = &caller_fn.blocks[0].instructions[1];
        if let Opcode::Iconst { index } = call_inst.opcode {
            assert_eq!(
                caller_fn
                    .constant_pool
                    .get(index)
                    .unwrap()
                    .try_to_i64()
                    .unwrap(),
                120
            );
        } else {
            panic!("Expected Iconst, got {:?}", call_inst.opcode);
        }
    }

    #[test]
    fn eval_pass_unsupported_fn_preserves_call() {
        // const fn that loads from pointer → can't evaluate
        let sig = Signature::new(&[(Type::Ptr, "p")], &[Type::I32]);
        let mut b = FunctionBuilder::new("load_const", sig);
        b.set_const(true);
        let (entry, params) = b.create_block_with_params(&[(Type::Ptr, "p")]);
        b.switch_to_block(entry);
        let v = b.load(params[0], Type::I32);
        b.return_(&[v]);
        let load_fn = b.finish();

        // caller with const arg
        let mut b2 = FunctionBuilder::new("caller", Signature::new(&[], &[Type::I32]));
        let entry2 = b2.create_block();
        b2.switch_to_block(entry2);
        let ptr_val = b2.iconst(0x1000, Type::Ptr);
        let call_result = b2.call(FuncRef(0), &[ptr_val], &[Type::I32]);
        b2.return_(&[call_result[0]]);
        let mut caller_fn = b2.finish();

        let mut func_table = HashMap::new();
        func_table.insert(FuncRef(0), load_fn);

        let pass = ConstFnEvalPass::new(func_table);
        let r = pass.run_on_function(&mut caller_fn).unwrap();
        assert!(!r.changed); // Should not be able to evaluate Load at compile time
    }
}
