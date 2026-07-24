//! 公共子表达式消除 (CSE) pass。
//!
//! 在每个基本块内查找并消除重复的计算。
//! 如果两条纯指令有相同的操作码、相同的操作数、相同的类型，
//! 则它们产生相同的值，后一条可以替换为对前一条结果的引用。
//!
//! # 算法
//!
//! 1. 对每个基本块，维护一个表达式 → Value 的哈希表
//! 2. 遍历指令，跳过有副作用或不产生结果的指令
//! 3. 计算表达式的哈希键（opcode + operands + type）
//! 4. 如果键已存在，映射当前 result → 之前的 result，替换为 Nop
//! 5. 如果键不存在，记录映射
//! 6. 遍历完成后，更新所有 Value 引用

use crate::CompileError;
use crate::ir::*;
use crate::optimize::{OptimizationPass, PassResult};
use std::collections::HashMap;

/// 公共子表达式消除 pass（局部，块内）。
///
/// 消除同一基本块内的重复计算。
/// 对于全局 CSE（跨基本块），需要支配树分析，留待后续版本实现。
#[derive(Default)]
pub struct CsePass;

impl CsePass {
    pub fn new() -> Self {
        Self
    }
}

impl OptimizationPass for CsePass {
    fn name(&self) -> &'static str {
        "cse"
    }

    fn description(&self) -> &'static str {
        "Common subexpression elimination: removes duplicate computations within a block"
    }

    fn run_on_function(&self, func: &mut Function) -> Result<PassResult, CompileError> {
        eliminate_common_subexpressions(func)
    }
}

/// 计算指令的表达式的哈希键。
///
/// 键由操作码和操作数列表组成。
/// 类型不同的同操作码指令被视为不同的表达式。
/// 对于交换律操作（Iadd/Imul/Fadd/Fmul/Band/Bor/Bxor），操作数排序后生成统一键。
pub(crate) fn expr_key(opcode: &Opcode, operands: &[Value], ty: Type) -> ExprKey {
    let mut ops = operands.to_vec();
    if is_commutative(opcode) && ops.len() >= 2 {
        // 对交换律操作排序操作数，使 a+b 和 b+a 产生相同键
        ops.sort_by_key(|v| v.0);
    }
    ExprKey {
        opcode: opcode_discriminant(opcode),
        operands: ops,
        ty,
    }
}

/// 判断操作码是否满足交换律（a op b == b op a）。
fn is_commutative(opcode: &Opcode) -> bool {
    matches!(
        opcode,
        Opcode::Iadd
            | Opcode::Imul
            | Opcode::Fadd { .. }
            | Opcode::Fmul { .. }
            | Opcode::Band
            | Opcode::Bor
            | Opcode::Bxor
    )
}

/// 获取操作码的判别值（用于哈希）。
pub(crate) fn opcode_discriminant(opcode: &Opcode) -> u8 {
    match opcode {
        Opcode::Iadd => 1,
        Opcode::Isub => 2,
        Opcode::Imul => 3,
        Opcode::Udiv => 4,
        Opcode::Sdiv => 5,
        Opcode::Urem => 6,
        Opcode::Srem => 7,
        Opcode::Fadd { .. } => 8,
        Opcode::Fsub { .. } => 9,
        Opcode::Fmul { .. } => 10,
        Opcode::Fdiv { .. } => 11,
        Opcode::Freeze => 11,
        Opcode::Fneg { .. } => 12,
        Opcode::Fabs { .. } => 13,
        Opcode::Fsqrt { .. } => 14,
        Opcode::Band => 15,
        Opcode::Bor => 16,
        Opcode::Bxor => 17,
        Opcode::Bnot => 18,
        Opcode::Ishl => 19,
        Opcode::Ushr => 20,
        Opcode::Sshr => 21,
        Opcode::Icmp { .. } => 22,
        Opcode::Fcmp { .. } => 23,
        Opcode::Load => 24, // Load from same address = same value (conservatively treated as pure)
        Opcode::Sextend => 25,
        Opcode::Uextend => 26,
        Opcode::Ireduce => 27,
        Opcode::Bitcast => 28,
        Opcode::StackAddr { .. } => 29,
        Opcode::GlobalAddr { .. } => 30,
        Opcode::Select => 31,
        Opcode::Copy => 32,
        // Non-CSE-able
        Opcode::Store
        | Opcode::StackLoad { .. }
        | Opcode::StackStore { .. }
        | Opcode::Call { .. }
        | Opcode::CallIndirect
        | Opcode::Iconst { .. }
        | Opcode::Fconst { .. }
        | Opcode::Phi { .. }
        | Opcode::Nop
        | Opcode::Vadd
        | Opcode::Vsub
        | Opcode::Vmul
        | Opcode::Vextract { .. }
        | Opcode::Vinsert { .. }
        | Opcode::ShuffleVector { .. }
        | Opcode::AtomicRmw { .. }
        | Opcode::Cmpxchg { .. }
        | Opcode::Fence { .. }
        | Opcode::ExtractValue { .. }
        | Opcode::InsertValue { .. }
        | Opcode::Alloca { .. }
        | Opcode::GetElementPtr { .. } => 0,
    }
}

/// 表达式的哈希键。
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub(crate) struct ExprKey {
    pub(crate) opcode: u8,
    pub(crate) operands: Vec<Value>,
    pub(crate) ty: Type,
}

/// 判断指令是否可以被 CSE（纯指令，无副作用，产生结果）。
pub(crate) fn is_cse_candidate(opcode: &Opcode) -> bool {
    opcode_discriminant(opcode) != 0
}

/// 对单个函数执行局部 CSE。
pub fn eliminate_common_subexpressions(func: &mut Function) -> Result<PassResult, CompileError> {
    let mut result = PassResult::default();
    // Value → Value 替换映射（被消除的 result → 保留的 result）
    let mut replacements: HashMap<Value, Value> = HashMap::new();

    for block in func.blocks.iter_mut() {
        // 每个块独立维护表达式表
        let mut expr_table: HashMap<ExprKey, Value> = HashMap::new();

        for inst in block.instructions.iter_mut() {
            // 跳过无结果的指令
            let inst_result = match inst.result {
                Some(v) => v,
                None => continue,
            };

            // 跳过不可 CSE 的指令
            if !is_cse_candidate(&inst.opcode) {
                continue;
            }

            // 计算表达式键（操作数已映射到替换后的值）
            let mapped_operands: Vec<Value> = inst
                .operands
                .iter()
                .map(|v| replacements.get(v).copied().unwrap_or(*v))
                .collect();

            let key = expr_key(&inst.opcode, &mapped_operands, inst.ty);

            if let Some(&existing_result) = expr_table.get(&key) {
                // 找到重复表达式！消除当前指令
                replacements.insert(inst_result, existing_result);
                inst.opcode = Opcode::Nop;
                inst.operands.clear();
                inst.ty = Type::Void;
                inst.result = None;
                result.instructions_removed += 1;
                result.changed = true;
            } else {
                // 首次出现，记录
                expr_table.insert(key, inst_result);
            }
        }
    }

    // 在所有指令中应用替换
    if !replacements.is_empty() {
        apply_replacements(func, &replacements);
        result.values_replaced = replacements.len();
    }

    Ok(result)
}

/// 在函数的所有指令和终止指令中应用 Value 替换。
pub(crate) fn apply_replacements(func: &mut Function, replacements: &HashMap<Value, Value>) {
    for block in func.blocks.iter_mut() {
        for inst in block.instructions.iter_mut() {
            if matches!(inst.opcode, Opcode::Nop) {
                continue;
            }
            for operand in inst.operands.iter_mut() {
                if let Some(&replacement) = replacements.get(operand) {
                    *operand = replacement;
                }
            }
        }

        // 更新终止指令
        match &mut block.terminator {
            Terminator::Branch {
                cond,
                true_args,
                false_args,
                ..
            } => {
                if let Some(&r) = replacements.get(cond) {
                    *cond = r;
                }
                for v in true_args.iter_mut().chain(false_args.iter_mut()) {
                    if let Some(&r) = replacements.get(v) {
                        *v = r;
                    }
                }
            }
            Terminator::Jump { args, .. } => {
                for v in args.iter_mut() {
                    if let Some(&r) = replacements.get(v) {
                        *v = r;
                    }
                }
            }
            Terminator::Return { values } => {
                for v in values.iter_mut() {
                    if let Some(&r) = replacements.get(v) {
                        *v = r;
                    }
                }
            }
            Terminator::Switch {
                discriminant,
                cases,
                ..
            } => {
                if let Some(&r) = replacements.get(discriminant) {
                    *discriminant = r;
                }
                for (_, _, args) in cases.iter_mut() {
                    for v in args.iter_mut() {
                        if let Some(&r) = replacements.get(v) {
                            *v = r;
                        }
                    }
                }
            }
            Terminator::Unreachable => {}
        }
    }
}

// ============================================================
// 测试
// ============================================================

#[cfg(test)]
mod tests {
    use super::*;
    use crate::optimize::OptimizationPass;

    #[test]
    fn eliminate_duplicate_add() {
        // x + y computed twice → second should be eliminated
        let sig = Signature::new(&[(Type::I32, "x"), (Type::I32, "y")], &[Type::I32]);
        let mut b = FunctionBuilder::new("test", sig);
        let (entry, params) = b.create_block_with_params(&[(Type::I32, "x"), (Type::I32, "y")]);
        b.switch_to_block(entry);
        let x = params[0];
        let y = params[1];
        let sum1 = b.iadd(x, y);
        let sum2 = b.iadd(x, y); // duplicate!
        let total = b.iadd(sum1, sum2);
        b.return_(&[total]);

        let mut func = b.finish();
        let pass = CsePass::new();
        let r = pass.run_on_function(&mut func).unwrap();

        assert!(r.changed);
        assert!(r.instructions_removed >= 1);
        // sum2 should be Nop, and total should use sum1 twice
        let mut found_nop = false;
        for inst in &func.blocks[0].instructions {
            if matches!(inst.opcode, Opcode::Nop) {
                found_nop = true;
            }
        }
        assert!(found_nop, "Expected a Nop from eliminated duplicate");
    }

    #[test]
    fn no_eliminate_different_operands() {
        // x + y and x + z are different → both stay
        let sig = Signature::new(
            &[(Type::I32, "x"), (Type::I32, "y"), (Type::I32, "z")],
            &[Type::I32],
        );
        let mut b = FunctionBuilder::new("test", sig);
        let (entry, params) =
            b.create_block_with_params(&[(Type::I32, "x"), (Type::I32, "y"), (Type::I32, "z")]);
        b.switch_to_block(entry);
        let sum1 = b.iadd(params[0], params[1]);
        let sum2 = b.iadd(params[0], params[2]); // different operand
        let total = b.iadd(sum1, sum2);
        b.return_(&[total]);

        let mut func = b.finish();
        let pass = CsePass::new();
        let r = pass.run_on_function(&mut func).unwrap();

        assert!(!r.changed); // no CSE should happen
    }

    #[test]
    fn eliminate_duplicate_bitwise() {
        // (a & b) computed twice
        let sig = Signature::new(&[(Type::I32, "a"), (Type::I32, "b")], &[Type::I32]);
        let mut b = FunctionBuilder::new("test", sig);
        let (entry, params) = b.create_block_with_params(&[(Type::I32, "a"), (Type::I32, "b")]);
        b.switch_to_block(entry);
        let and1 = b.band(params[0], params[1]);
        let and2 = b.band(params[0], params[1]); // duplicate
        let result = b.bor(and1, and2);
        b.return_(&[result]);

        let mut func = b.finish();
        let pass = CsePass::new();
        let r = pass.run_on_function(&mut func).unwrap();
        assert!(r.changed);
    }

    #[test]
    fn no_cse_on_store() {
        // Store instructions should not be CSE'd
        let sig = Signature::new(&[(Type::Ptr, "p"), (Type::I32, "v")], &[]);
        let mut b = FunctionBuilder::new("test", sig);
        let (entry, params) = b.create_block_with_params(&[(Type::Ptr, "p"), (Type::I32, "v")]);
        b.switch_to_block(entry);
        b.store(params[1], params[0]); // first store
        b.store(params[1], params[0]); // second store — NOT a CSE candidate
        b.return_(&[]);

        let mut func = b.finish();
        let pass = CsePass::new();
        let r = pass.run_on_function(&mut func).unwrap();
        assert!(!r.changed); // stores should not be affected
    }

    #[test]
    fn eliminate_duplicate_constant_expr() {
        // (3 + 5) computed twice — should become CSE
        // Actually this would already be folded by const-fold, so let's use params
        let sig = Signature::new(&[(Type::I32, "x")], &[Type::I32]);
        let mut b = FunctionBuilder::new("test", sig);
        let (entry, params) = b.create_block_with_params(&[(Type::I32, "x")]);
        b.switch_to_block(entry);
        let x = params[0];
        let c3 = b.iconst_i32(3);
        let _c5 = b.iconst_i32(5);
        let expr1 = b.imul(x, c3);
        let expr2 = b.imul(x, c3); // x * 3 computed twice
        let result = b.iadd(expr1, expr2);
        b.return_(&[result]);

        let mut func = b.finish();
        let pass = CsePass::new();
        let r = pass.run_on_function(&mut func).unwrap();
        assert!(r.changed);
        assert!(r.instructions_removed >= 1);
    }

    #[test]
    fn cse_chain() {
        // Multiple duplicates in a chain
        let sig = Signature::new(&[(Type::I32, "a"), (Type::I32, "b")], &[Type::I32]);
        let mut b = FunctionBuilder::new("test", sig);
        let (entry, params) = b.create_block_with_params(&[(Type::I32, "a"), (Type::I32, "b")]);
        b.switch_to_block(entry);
        let a = params[0];
        let b_val = params[1];

        let sum1 = b.iadd(a, b_val);
        let _sum2 = b.iadd(a, b_val); // duplicate of sum1
        let prod1 = b.imul(sum1, sum1);
        let _prod2 = b.imul(sum1, sum1); // duplicate of prod1

        b.return_(&[prod1, prod1]); // use prod1 twice, prod2 should be dead
        let mut func = b.finish();
        let pass = CsePass::new();
        let r = pass.run_on_function(&mut func).unwrap();
        assert!(r.changed);
    }

    #[test]
    fn pass_manager_with_cse() {
        let sig = Signature::new(&[(Type::I32, "x")], &[Type::I32]);
        let mut b = FunctionBuilder::new("test", sig);
        let (entry, params) = b.create_block_with_params(&[(Type::I32, "x")]);
        b.switch_to_block(entry);
        let x = params[0];
        let c = b.iconst_i32(2);
        let e1 = b.imul(x, c);
        let e2 = b.imul(x, c); // duplicate
        let r = b.iadd(e1, e2);
        b.return_(&[r]);

        let mut func = b.finish();

        let mut pm = crate::optimize::PassManager::new();
        pm.add_pass(CsePass::new(), crate::optimize::PassRunMode::Once);
        let result = pm.run_on_function(&mut func).unwrap();
        assert!(result.changed);
    }
}
