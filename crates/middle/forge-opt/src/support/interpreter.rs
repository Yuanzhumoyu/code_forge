//! IR interpreter — compile-time const function evaluation.
//!
//! Evaluates pure functions at compile time by interpreting the IR.
//! Used for constant propagation across function boundaries and
//! compile-time function evaluation (CTFE).

use crate::ConstValue;
use forge_ir::*;
use std::collections::HashMap;

pub struct IrInterpreter {
    values: HashMap<Value, InterpValue>,
}

#[derive(Clone, Debug)]
pub enum InterpValue {
    Int(Big, TypeId),
    Float(Big, TypeId),
    Bool(bool),
}

impl InterpValue {
    fn to_const_value(&self) -> ConstValue {
        match self {
            InterpValue::Int(v, t) => ConstValue::Int(v.clone(), *t),
            InterpValue::Float(v, t) => ConstValue::Float(v.clone(), *t),
            InterpValue::Bool(b) => ConstValue::Bool(*b),
        }
    }

    fn to_bool(&self) -> Option<bool> {
        match self {
            InterpValue::Bool(b) => Some(*b),
            InterpValue::Int(v, _) => Some(!v.is_zero()),
            InterpValue::Float(v, _) => Some(!v.is_zero()),
        }
    }
}

/// Fold an opcode with constant operands during interpretation.
/// Delegates to const_fold.
pub fn interp_fold_opcode(
    opcode: &Opcode,
    operands: &[ConstValue],
    ty: TypeId,
) -> Result<Option<ConstValue>, CompileError> {
    crate::scalar::const_fold::fold_opcode(opcode, operands, ty)
}

impl IrInterpreter {
    pub fn new() -> Self {
        Self {
            values: HashMap::new(),
        }
    }

    /// Evaluate a function with given arguments.
    ///
    /// Returns `Some(InterpValue)` if evaluation succeeds (pure function
    /// with no side effects). Returns `None` if the function contains
    /// non-deterministic operations (calls, memory access, etc.).
    pub fn evaluate(
        &mut self,
        func: &Function,
        args: &[InterpValue],
    ) -> Result<Option<InterpValue>, CompileError> {
        let entry = match func.entry_block {
            Some(b) => b,
            None => return Ok(None),
        };

        // Map entry block params → args
        let param_vals = func.dfg.block_param_values(entry);
        for (i, &pv) in param_vals.iter().enumerate() {
            if let Some(arg) = args.get(i) {
                self.values.insert(pv, arg.clone());
            }
        }

        // Interpret starting from entry block
        self.eval_block(func, entry)
    }

    /// Evaluate a single basic block, following control flow.
    fn eval_block(
        &mut self,
        func: &Function,
        block: Block,
    ) -> Result<Option<InterpValue>, CompileError> {
        let block_data = &func.dfg.blocks[block.0 as usize];

        // Execute instructions in order
        for &inst_id in &block_data.inst_order {
            let inst = &func.dfg.insts[inst_id.0 as usize];
            if matches!(inst.opcode, Opcode::Nop) {
                continue;
            }

            let result_val = inst.results.first().copied();
            let result_ty = result_val
                .map(|v| func.dfg.values[v.0 as usize].ty)
                .unwrap_or(TypeId::VOID);

            let value = self.eval_instruction(
                &inst.opcode,
                &inst.operands,
                &inst.immediates,
                result_ty,
                func,
            )?;

            if let (Some(v), Some(ival)) = (result_val, value) {
                self.values.insert(v, ival);
            }
        }

        // Follow terminator
        match &block_data.terminator {
            Terminator::Return { values } => {
                if values.is_empty() {
                    Ok(None)
                } else if let Some(ival) = self.values.get(&values[0]) {
                    Ok(Some(ival.clone()))
                } else {
                    Ok(None)
                }
            }
            Terminator::Jump { target, .. } => self.eval_block(func, *target),
            Terminator::Branch {
                cond,
                then_block,
                else_block,
                ..
            } => {
                match self.values.get(cond).and_then(|v| v.to_bool()) {
                    Some(true) => self.eval_block(func, *then_block),
                    Some(false) => self.eval_block(func, *else_block),
                    None => Ok(None), // Non-constant branch
                }
            }
            Terminator::Unreachable => Ok(None),
            Terminator::Switch { .. } => Ok(None), // Too complex for basic interpreter
        }
    }

    /// Evaluate a single instruction.
    fn eval_instruction(
        &mut self,
        opcode: &Opcode,
        operands: &[Value],
        immediates: &[Immediate],
        ty: TypeId,
        func: &Function,
    ) -> Result<Option<InterpValue>, CompileError> {
        match opcode {
            // Constants from immediates
            Opcode::Iconst => {
                if let Some(cid) = immediates.first().and_then(|i| i.as_const())
                    && let Some(big) = func.constants.resolve_big(cid)
                {
                    return Ok(Some(InterpValue::Int(big, ty)));
                }
                Ok(None)
            }
            Opcode::Fconst => {
                if let Some(cid) = immediates.first().and_then(|i| i.as_const())
                    && let Some(big) = func.constants.resolve_big(cid)
                {
                    return Ok(Some(InterpValue::Float(big, ty)));
                }
                Ok(None)
            }

            // Pure arithmetic: try const-fold if all operands are known
            Opcode::Iadd
            | Opcode::Isub
            | Opcode::Imul
            | Opcode::Udiv
            | Opcode::Sdiv
            | Opcode::Urem
            | Opcode::Srem
            | Opcode::Band
            | Opcode::Bor
            | Opcode::Bxor
            | Opcode::Bnot
            | Opcode::Ishl
            | Opcode::Ushr
            | Opcode::Sshr
            | Opcode::Fadd
            | Opcode::Fsub
            | Opcode::Fmul
            | Opcode::Fdiv
            | Opcode::Fneg
            | Opcode::Fabs
            | Opcode::Fsqrt
            | Opcode::Sextend
            | Opcode::Uextend
            | Opcode::Ireduce
            | Opcode::Bitcast
            | Opcode::Select
            | Opcode::Copy => {
                let const_ops: Vec<ConstValue> = operands
                    .iter()
                    .filter_map(|v| self.values.get(v).map(|iv| iv.to_const_value()))
                    .collect();
                if const_ops.len() == operands.len() {
                    match interp_fold_opcode(opcode, &const_ops, ty)? {
                        Some(cv) => match cv {
                            ConstValue::Int(v, t) => Ok(Some(InterpValue::Int(v, t))),
                            ConstValue::Float(v, t) => Ok(Some(InterpValue::Float(v, t))),
                            ConstValue::Bool(b) => Ok(Some(InterpValue::Bool(b))),
                        },
                        None => Ok(None),
                    }
                } else {
                    Ok(None) // Not all operands are known
                }
            }

            // Comparison
            Opcode::Icmp { .. } | Opcode::Fcmp { .. } => {
                let const_ops: Vec<ConstValue> = operands
                    .iter()
                    .filter_map(|v| self.values.get(v).map(|iv| iv.to_const_value()))
                    .collect();
                if const_ops.len() == operands.len() {
                    match interp_fold_opcode(opcode, &const_ops, ty)? {
                        Some(cv) => match cv {
                            ConstValue::Bool(b) => Ok(Some(InterpValue::Bool(b))),
                            ConstValue::Int(v, _) => Ok(Some(InterpValue::Bool(!v.is_zero()))),
                            _ => Ok(None),
                        },
                        None => Ok(None),
                    }
                } else {
                    Ok(None)
                }
            }

            // Pure ops that may fold via const_fold
            Opcode::Clz
            | Opcode::Ctz
            | Opcode::Popcnt
            | Opcode::Bitreverse
            | Opcode::Rotl
            | Opcode::Rotr
            | Opcode::Abs
            | Opcode::Smin
            | Opcode::Smax
            | Opcode::Umin
            | Opcode::Umax
            | Opcode::SaddSat
            | Opcode::SsubSat
            | Opcode::UaddSat
            | Opcode::UsubSat
            | Opcode::Bswap
            | Opcode::Fma
            | Opcode::Fmin
            | Opcode::Fmax
            | Opcode::Fcopysign
            | Opcode::Ffloor
            | Opcode::Fceil
            | Opcode::Ftrunc
            | Opcode::Fround => {
                let const_ops: Vec<ConstValue> = operands
                    .iter()
                    .filter_map(|v| self.values.get(v).map(|iv| iv.to_const_value()))
                    .collect();
                if const_ops.len() == operands.len() {
                    match interp_fold_opcode(opcode, &const_ops, ty)? {
                        Some(cv) => match cv {
                            ConstValue::Int(v, t) => Ok(Some(InterpValue::Int(v, t))),
                            ConstValue::Float(v, t) => Ok(Some(InterpValue::Float(v, t))),
                            ConstValue::Bool(b) => Ok(Some(InterpValue::Bool(b))),
                        },
                        None => Ok(None),
                    }
                } else {
                    Ok(None)
                }
            }

            // Pointer predicates — fold if operand is known
            Opcode::IsNull | Opcode::IsNotNull => {
                let const_ops: Vec<ConstValue> = operands
                    .iter()
                    .filter_map(|v| self.values.get(v).map(|iv| iv.to_const_value()))
                    .collect();
                if const_ops.len() == operands.len() {
                    match interp_fold_opcode(opcode, &const_ops, ty)? {
                        Some(ConstValue::Bool(b)) => Ok(Some(InterpValue::Bool(b))),
                        _ => Ok(None),
                    }
                } else {
                    Ok(None)
                }
            }

            // Non-deterministic / side-effecting: bail out
            Opcode::Load
            | Opcode::Store
            | Opcode::Call
            | Opcode::CallIndirect
            | Opcode::StackAddr
            | Opcode::GlobalAddr
            | Opcode::Alloca
            | Opcode::GetElementPtr
            | Opcode::AtomicRmw
            | Opcode::Cmpxchg
            | Opcode::Fence
            | Opcode::Freeze
            | Opcode::Vadd
            | Opcode::Vsub
            | Opcode::Vmul
            | Opcode::Vdiv
            | Opcode::Vneg
            | Opcode::Vabs
            | Opcode::Vextract
            | Opcode::Vinsert
            | Opcode::Vbitcast
            | Opcode::Vbroadcast
            | Opcode::ShuffleVector
            | Opcode::ExtractValue
            | Opcode::InsertValue
            | Opcode::Trap
            | Opcode::SaddOverflow
            | Opcode::UaddOverflow
            | Opcode::SsubOverflow
            | Opcode::UsubOverflow
            | Opcode::SmulOverflow
            | Opcode::UmulOverflow
            | Opcode::Poison
            | Opcode::Undef
            | Opcode::Nop => Ok(None),
        }
    }
}

impl Default for IrInterpreter {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use FunctionBuilder;

    /// Build and evaluate: fn add(x: i32, y: i32) -> i32 { x + y }
    #[test]
    fn interp_eval_simple_add() {
        let sig = FunctionSignature::new(&[(TypeId::I32, "x"), (TypeId::I32, "y")], &[TypeId::I32]);
        let mut b = FunctionBuilder::new("add", TypeContext::new(), sig);
        let (entry, params) = b.create_block_with_params(&[(TypeId::I32, "x"), (TypeId::I32, "y")]);
        b.switch_to_block(entry);
        let sum = b.iadd(params[0], params[1]);
        b.ret(&[sum]);
        let func = b.finish();

        let mut interp = IrInterpreter::new();
        let result = interp
            .evaluate(
                &func,
                &[
                    InterpValue::Int(Big::from_i64(3), TypeId::I32),
                    InterpValue::Int(Big::from_i64(5), TypeId::I32),
                ],
            )
            .unwrap();

        match result {
            Some(InterpValue::Int(v, _)) => assert_eq!(v, Big::from_i64(8), "3+5 should be 8"),
            other => panic!("Expected Int(8), got {:?}", other),
        }
    }

    /// Evaluate: fn const_fn() -> i32 { 42 }
    #[test]
    fn interp_eval_const_fn() {
        let sig = FunctionSignature::new(&[], &[TypeId::I32]);
        let mut b = FunctionBuilder::new("const_fn", TypeContext::new(), sig);
        let entry = b.create_block();
        b.switch_to_block(entry);
        let v = b.iconst_i32(42);
        b.ret(&[v]);
        let func = b.finish();

        let mut interp = IrInterpreter::new();
        let result = interp.evaluate(&func, &[]).unwrap();

        match result {
            Some(InterpValue::Int(v, _)) => assert_eq!(v, Big::from_i64(42)),
            other => panic!("Expected Int(42), got {:?}", other),
        }
    }

    /// Function with a known branch condition → follows correct path.
    /// b = 0 (false) → else branch → returns 2
    #[test]
    fn interp_known_branch_follows_path() {
        let sig = FunctionSignature::new(&[(TypeId::I32, "x")], &[TypeId::I32]);
        let mut b = FunctionBuilder::new("test", TypeContext::new(), sig);
        let (entry, params) = b.create_block_with_params(&[(TypeId::I32, "x")]);
        let then_blk = b.create_block();
        let else_blk = b.create_block();

        b.switch_to_block(entry);
        b.branch(params[0], then_blk, &[], else_blk, &[]);

        b.switch_to_block(then_blk);
        let v1 = b.iconst_i32(1);
        b.ret(&[v1]);

        b.switch_to_block(else_blk);
        let v2 = b.iconst_i32(2);
        b.ret(&[v2]);
        let func = b.finish();

        let mut interp = IrInterpreter::new();
        // x = 0 (false) → else branch → returns 2
        let result = interp
            .evaluate(&func, &[InterpValue::Int(Big::from_i64(0), TypeId::I32)])
            .unwrap();
        match result {
            Some(InterpValue::Int(v, _)) => assert_eq!(v, Big::from_i64(2), "0 means else → 2"),
            other => panic!("Expected Int(2), got {:?}", other),
        }
    }

    /// Function with memory access → interpreter bails out.
    #[test]
    fn interp_memory_access_returns_none() {
        let sig = FunctionSignature::new(&[], &[TypeId::I32]);
        let mut b = FunctionBuilder::new("test", TypeContext::new(), sig);
        let entry = b.create_block();
        b.switch_to_block(entry);
        let addr = b.stack_addr(0);
        let v = b.load(addr, TypeId::I32); // Memory access → can't interpret
        b.ret(&[v]);
        let func = b.finish();

        let mut interp = IrInterpreter::new();
        let result = interp.evaluate(&func, &[]).unwrap();
        assert!(result.is_none(), "Should bail on memory access");
    }

    /// Evaluate: fn choose(b: i32) -> i32 { if b { 10 } else { 20 } }
    #[test]
    fn interp_eval_branch() {
        let sig = FunctionSignature::new(&[(TypeId::I32, "b")], &[TypeId::I32]);
        let mut b = FunctionBuilder::new("choose", TypeContext::new(), sig);
        let (entry, params) = b.create_block_with_params(&[(TypeId::I32, "b")]);
        let then_blk = b.create_block();
        let else_blk = b.create_block();

        b.switch_to_block(entry);
        b.branch(params[0], then_blk, &[], else_blk, &[]);

        b.switch_to_block(then_blk);
        let ten = b.iconst_i32(10);
        b.ret(&[ten]);

        b.switch_to_block(else_blk);
        let twenty = b.iconst_i32(20);
        b.ret(&[twenty]);
        let func = b.finish();

        let mut interp = IrInterpreter::new();

        // b = 1 (true) → should take then branch → return 10
        let result = interp
            .evaluate(&func, &[InterpValue::Int(Big::from_i64(1), TypeId::I32)])
            .unwrap();
        match result {
            Some(InterpValue::Int(v, _)) => assert_eq!(v, Big::from_i64(10), "true → then → 10"),
            other => panic!("Expected Int(10), got {:?}", other),
        }
    }
}
