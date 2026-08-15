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

use crate::{OptimizationPass, PassResult};
use forge_ir::IrError;
use forge_ir::*;
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

    fn run_on_function(&self, func: &mut Function) -> Result<PassResult, IrError> {
        eliminate_common_subexpressions(func)
    }
}

/// 计算指令的表达式的哈希键。
///
/// 键由操作码和操作数列表组成。
/// 类型不同的同操作码指令被视为不同的表达式。
/// 对于交换律操作（Iadd/Imul/Fadd/Fmul/Band/Bor/Bxor），操作数排序后生成统一键。
pub(crate) fn expr_key(opcode: &Opcode, operands: &[Value], ty: TypeId) -> ExprKey {
    let mut ops: smallvec::SmallVec<[Value; 4]> = operands.iter().copied().collect();
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
            | Opcode::Fadd
            | Opcode::Fmul
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
        Opcode::Fadd => 8,
        Opcode::Fsub => 9,
        Opcode::Fmul => 10,
        Opcode::Fdiv => 11,
        Opcode::Frem => 90,
        Opcode::Freeze => 12,
        Opcode::Fneg => 13,
        Opcode::Fabs => 14,
        Opcode::Fsqrt => 15,
        Opcode::Band => 16,
        Opcode::Bor => 17,
        Opcode::Bxor => 18,
        Opcode::Bnot => 19,
        Opcode::Ishl => 20,
        Opcode::Ushr => 21,
        Opcode::Sshr => 22,
        Opcode::Icmp { .. } => 23,
        Opcode::Fcmp { .. } => 24,
        Opcode::Load => 25, // Load from same address = same value (conservatively treated as pure)
        Opcode::Fload => 25, // 浮点 load 同 Load 语义
        Opcode::Sextend => 26,
        Opcode::Uextend => 27,
        Opcode::Fptrunc => 71,
        Opcode::Fpext => 72,
        Opcode::Fptosi => 73,
        Opcode::Sitofp => 74,
        Opcode::Fptoui => 75,
        Opcode::Uitofp => 76,
        Opcode::Ptrtoint => 77,
        Opcode::Inttoptr => 78,
        Opcode::Ireduce => 28,
        Opcode::Bitcast => 29,
        Opcode::StackAddr => 30,
        Opcode::GlobalAddr => 31,
        Opcode::Select => 32,
        Opcode::Copy => 33,
        // Bit manipulation (CSE-able)
        Opcode::Clz => 34,
        Opcode::Ctz => 35,
        Opcode::Popcnt => 36,
        Opcode::Bitreverse => 37,
        Opcode::Rotl => 38,
        Opcode::Rotr => 39,
        // Integer extended (CSE-able)
        Opcode::Abs => 40,
        Opcode::Smin => 41,
        Opcode::Smax => 42,
        Opcode::Umin => 43,
        Opcode::Umax => 44,
        Opcode::SaddSat => 45,
        Opcode::SsubSat => 46,
        Opcode::UaddSat => 47,
        Opcode::UsubSat => 48,
        Opcode::Bswap => 49,
        // Float extended (CSE-able)
        Opcode::Fma => 50,
        Opcode::Fmin => 51,
        Opcode::Fmax => 52,
        Opcode::Fcopysign => 53,
        Opcode::Ffloor => 54,
        Opcode::Fceil => 55,
        Opcode::Ftrunc => 56,
        Opcode::Fround => 57,
        // Pointer predicates (CSE-able)
        Opcode::IsNull => 58,
        Opcode::IsNotNull => 59,
        // Overflow arithmetic (CSE-able but produce 2 results)
        Opcode::SaddOverflow => 60,
        Opcode::UaddOverflow => 61,
        Opcode::SsubOverflow => 62,
        Opcode::UsubOverflow => 63,
        Opcode::SmulOverflow => 64,
        Opcode::UmulOverflow => 65,
        // SIMD extended (CSE-able)
        Opcode::Vdiv => 66,
        Opcode::Vneg => 67,
        Opcode::Vabs => 68,
        Opcode::Vbitcast => 69,
        Opcode::Vbroadcast => 70,
        // Non-CSE-able
        Opcode::Store
        | Opcode::Fstore
        | Opcode::Call
        | Opcode::CallIndirect
        | Opcode::Iconst
        | Opcode::Fconst
        | Opcode::Vconst
        | Opcode::Nop
        | Opcode::Vadd
        | Opcode::Vsub
        | Opcode::Vmul
        | Opcode::Vextract
        | Opcode::AddrSpaceCast
        | Opcode::VaArg
        | Opcode::Vinsert
        | Opcode::Vsplit
        | Opcode::Vconcat
        | Opcode::ShuffleVector
        | Opcode::AtomicRmw
        | Opcode::Cmpxchg
        | Opcode::Fence
        | Opcode::ExtractValue
        | Opcode::InsertValue
        | Opcode::Alloca
        | Opcode::GetElementPtr
        | Opcode::Poison
        | Opcode::Undef
        | Opcode::Trap
        | Opcode::LandingPad => 0,
    }
}

/// 表达式的哈希键。
/// operands 用 SmallVec（≤4 操作数 inline，无堆分配）——
/// 每指令一次 key 构造是 CSE/GVN/GVN-PRE 的每指令固定开销。
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub(crate) struct ExprKey {
    pub(crate) opcode: u8,
    pub(crate) operands: smallvec::SmallVec<[Value; 4]>,
    pub(crate) ty: TypeId,
}

/// 判断指令是否可以被 CSE（纯指令，无副作用，产生结果）。
pub(crate) fn is_cse_candidate(opcode: &Opcode) -> bool {
    opcode_discriminant(opcode) != 0
}

/// 对单个函数执行局部 CSE。
pub fn eliminate_common_subexpressions(func: &mut Function) -> Result<PassResult, IrError> {
    let mut result = PassResult::default();
    // Value → Value 替换映射（被消除的 result → 保留的 result）
    let mut replacements: HashMap<Value, Value> = HashMap::new();
    // 待删除的重复表达式指令（循环后统一 kill，避免与借用冲突）
    let mut to_kill: Vec<Inst> = Vec::new();

    let block_count = func.dfg.blocks.len();
    for bi in 0..block_count {
        // 每个块独立维护表达式表
        let mut expr_table: HashMap<ExprKey, Value> = HashMap::new();

        // 借用 inst_order（blocks 与 insts 为 dfg 不同字段，可拆分借用）
        let inst_ids = &func.dfg.blocks[bi].inst_order;

        for inst_id in inst_ids {
            let inst = &mut func.dfg.insts[inst_id.0 as usize];

            // 跳过无结果的指令
            let inst_result = match inst.results.first().copied() {
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

            let ty = func.dfg.values[inst_result.0 as usize].ty;
            let key = expr_key(&inst.opcode, &mapped_operands, ty);

            if let Some(&existing_result) = expr_table.get(&key) {
                // 找到重复表达式！消除当前指令（延迟到循环后统一 kill）
                replacements.insert(inst_result, existing_result);
                to_kill.push(*inst_id);
                result.instructions_removed += 1;
                result.changed = true;
            } else {
                // 首次出现，记录
                expr_table.insert(key, inst_result);
            }
        }
    }

    // 应用替换（同步 use-lists），再原子删除重复表达式指令
    if !replacements.is_empty() {
        func.apply_replacements(&replacements);
        result.values_replaced = replacements.len();
    }
    for inst in to_kill {
        func.kill_inst(inst);
    }

    Ok(result)
}

// ============================================================
// 测试
// ============================================================

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{OptimizationPass, PassManager, PassRunMode};

    #[test]
    fn eliminate_duplicate_add() {
        // x + y computed twice → second should be eliminated
        let sig = FunctionSignature::new(&[(TypeId::I32, "x"), (TypeId::I32, "y")], &[TypeId::I32]);
        let mut b = FunctionBuilder::new("test", TypeContext::new(), sig);
        let (entry, params) = b.create_block_with_params(&[(TypeId::I32, "x"), (TypeId::I32, "y")]);
        b.switch_to_block(entry);
        let x = params[0];
        let y = params[1];
        let sum1 = b.iadd(x, y);
        let sum2 = b.iadd(x, y); // duplicate!
        let total = b.iadd(sum1, sum2);
        b.ret(&[total]);

        let mut func = b.finish().expect("build");
        let pass = CsePass::new();
        let r = pass.run_on_function(&mut func).unwrap();

        assert!(r.changed);
        assert!(r.instructions_removed >= 1);
        // sum2 应被原子删除（kill_inst 从 inst_order 移除）：
        // total 的两个操作数都变为 sum1，sum2 值 VOID
        let ValueDef::Inst(total_inst, _) = func.dfg.value_def(total).unwrap() else {
            panic!("total 应是指令定义")
        };
        let ops = &func.dfg.insts[total_inst.0 as usize].operands;
        assert_eq!(ops.as_slice(), &[sum1, sum1], "total 应引用 sum1 两次");
        assert_eq!(
            func.dfg.value_type(sum2),
            Some(TypeId::VOID),
            "sum2 已被删除"
        );
    }

    #[test]
    fn no_eliminate_different_operands() {
        // x + y and x + z are different → both stay
        let sig = FunctionSignature::new(
            &[(TypeId::I32, "x"), (TypeId::I32, "y"), (TypeId::I32, "z")],
            &[TypeId::I32],
        );
        let mut b = FunctionBuilder::new("test", TypeContext::new(), sig);
        let (entry, params) = b.create_block_with_params(&[
            (TypeId::I32, "x"),
            (TypeId::I32, "y"),
            (TypeId::I32, "z"),
        ]);
        b.switch_to_block(entry);
        let sum1 = b.iadd(params[0], params[1]);
        let sum2 = b.iadd(params[0], params[2]); // different operand
        let total = b.iadd(sum1, sum2);
        b.ret(&[total]);

        let mut func = b.finish().expect("build");
        let pass = CsePass::new();
        let r = pass.run_on_function(&mut func).unwrap();

        assert!(!r.changed); // no CSE should happen
    }

    #[test]
    fn eliminate_duplicate_bitwise() {
        // (a & b) computed twice
        let sig = FunctionSignature::new(&[(TypeId::I32, "a"), (TypeId::I32, "b")], &[TypeId::I32]);
        let mut b = FunctionBuilder::new("test", TypeContext::new(), sig);
        let (entry, params) = b.create_block_with_params(&[(TypeId::I32, "a"), (TypeId::I32, "b")]);
        b.switch_to_block(entry);
        let and1 = b.band(params[0], params[1]);
        let and2 = b.band(params[0], params[1]); // duplicate
        let result = b.bor(and1, and2);
        b.ret(&[result]);

        let mut func = b.finish().expect("build");
        let pass = CsePass::new();
        let r = pass.run_on_function(&mut func).unwrap();
        assert!(r.changed);
    }

    #[test]
    fn no_cse_on_store() {
        // Store instructions should not be CSE'd
        let sig = FunctionSignature::new(&[(TypeId::PTR, "p"), (TypeId::I32, "v")], &[]);
        let mut b = FunctionBuilder::new("test", TypeContext::new(), sig);
        let (entry, params) = b.create_block_with_params(&[(TypeId::PTR, "p"), (TypeId::I32, "v")]);
        b.switch_to_block(entry);
        b.store(params[1], params[0]); // first store
        b.store(params[1], params[0]); // second store — NOT a CSE candidate
        b.ret(&[]);

        let mut func = b.finish().expect("build");
        let pass = CsePass::new();
        let r = pass.run_on_function(&mut func).unwrap();
        assert!(!r.changed); // stores should not be affected
    }

    #[test]
    fn eliminate_duplicate_constant_expr() {
        let sig = FunctionSignature::new(&[(TypeId::I32, "x")], &[TypeId::I32]);
        let mut b = FunctionBuilder::new("test", TypeContext::new(), sig);
        let (entry, params) = b.create_block_with_params(&[(TypeId::I32, "x")]);
        b.switch_to_block(entry);
        let x = params[0];
        let c3 = b.iconst_i32(3);
        let _c5 = b.iconst_i32(5);
        let expr1 = b.imul(x, c3);
        let expr2 = b.imul(x, c3); // x * 3 computed twice
        let result = b.iadd(expr1, expr2);
        b.ret(&[result]);

        let mut func = b.finish().expect("build");
        let pass = CsePass::new();
        let r = pass.run_on_function(&mut func).unwrap();
        assert!(r.changed);
        assert!(r.instructions_removed >= 1);
    }

    #[test]
    fn cse_chain() {
        // Multiple duplicates in a chain
        let sig = FunctionSignature::new(&[(TypeId::I32, "a"), (TypeId::I32, "b")], &[TypeId::I32]);
        let mut b = FunctionBuilder::new("test", TypeContext::new(), sig);
        let (entry, params) = b.create_block_with_params(&[(TypeId::I32, "a"), (TypeId::I32, "b")]);
        b.switch_to_block(entry);
        let a = params[0];
        let b_val = params[1];

        let sum1 = b.iadd(a, b_val);
        let _sum2 = b.iadd(a, b_val); // duplicate of sum1
        let prod1 = b.imul(sum1, sum1);
        let _prod2 = b.imul(sum1, sum1); // duplicate of prod1

        b.ret(&[prod1, prod1]); // use prod1 twice, prod2 should be dead
        let mut func = b.finish().expect("build");
        let pass = CsePass::new();
        let r = pass.run_on_function(&mut func).unwrap();
        assert!(r.changed);
    }

    #[test]
    fn pass_manager_with_cse() {
        let sig = FunctionSignature::new(&[(TypeId::I32, "x")], &[TypeId::I32]);
        let mut b = FunctionBuilder::new("test", TypeContext::new(), sig);
        let (entry, params) = b.create_block_with_params(&[(TypeId::I32, "x")]);
        b.switch_to_block(entry);
        let x = params[0];
        let c = b.iconst_i32(2);
        let e1 = b.imul(x, c);
        let e2 = b.imul(x, c); // duplicate
        let r = b.iadd(e1, e2);
        b.ret(&[r]);

        let mut func = b.finish().expect("build");

        let mut pm = PassManager::new();
        pm.add_pass(Box::new(CsePass::new()), PassRunMode::Once);
        let result = pm.run_on_function(&mut func).unwrap();
        assert!(result.changed);
    }
}
