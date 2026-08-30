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
        opcode: opcode.clone(),
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

/// 是否为内存读指令（Load/Fload）。
///
/// P1-5：load 在满足「无中间 may-alias 写」的前提下可参与 CSE/GVN 消重
/// （消重键 = opcode + 地址操作数 + 类型），volatile/原子访问由调用方排除。
/// 不并入 `is_cse_candidate` 白名单——GVN-PRE 的 block-level kill 模型与
/// 无 mem_flags 的 ExprKey 不适合 load PRE，保持 PRE 不编号 load。
pub(crate) fn is_load_op(opcode: &Opcode) -> bool {
    matches!(opcode, Opcode::Load | Opcode::Fload)
}

/// P1-5：load 表项是否被写位置 `wloc` kill（位置 MayAlias）。
///
/// 用于 CSE/GVN 的内存写 kill 精化：store 只 kill may-alias 的 load
/// （不同栈槽 / 不同全局 / 栈 vs 全局互不干扰）。无法判定写位置时
/// （`None`，防御性）保守返回 true。
pub(crate) fn killed_by_write(
    key: &ExprKey,
    wloc: Option<MemoryLocation>,
    alias: &AliasAnalysis,
    func: &Function,
) -> bool {
    if !is_load_op(&key.opcode) {
        return false;
    }
    let Some(wloc) = wloc else {
        return true;
    };
    let lloc = key
        .operands
        .first()
        .map(|&a| alias.location_of_addr(func, a))
        .unwrap_or(MemoryLocation::Unknown);
    alias.alias(lloc, wloc) != AliasResult::NoAlias
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
///
/// `opcode` 直接存 `Opcode`（含 Icmp/Fcmp 的 cond 载荷）而非判别 u8——
/// 修复 P0-2：`icmp eq a,b` 与 `icmp ne a,b` 此前共用判别值 23 被错误
/// 互相替换。Opcode 已实现 Eq+Hash（cond 参与哈希）。
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub(crate) struct ExprKey {
    pub(crate) opcode: Opcode,
    pub(crate) operands: smallvec::SmallVec<[Value; 4]>,
    pub(crate) ty: TypeId,
}

/// 判断指令是否可以被 CSE（纯指令，无副作用，产生结果）。
///
/// 显式纯指令白名单（不用判别值 != 0）：多结果指令（SaddOverflow 等
/// overflow 系）不进消重表——CSE 只映射 results[0]，results[1]（flag）
/// 会悬空指向被 kill 的指令（P0-6）；volatile/原子/副作用指令不参与
/// （P0-3/P0-5 语义）。
pub(crate) fn is_cse_candidate(opcode: &Opcode) -> bool {
    // 纯算术/逻辑/转换（无内存、无副作用、单结果）——白名单显式列出，
    // 避免判别值漏项或混入 Load/原子。
    matches!(
        opcode,
        Opcode::Iadd
            | Opcode::Isub
            | Opcode::Imul
            | Opcode::Udiv
            | Opcode::Sdiv
            | Opcode::Urem
            | Opcode::Srem
            | Opcode::Fadd
            | Opcode::Fsub
            | Opcode::Fmul
            | Opcode::Fdiv
            | Opcode::Frem
            | Opcode::Freeze
            | Opcode::Fneg
            | Opcode::Fabs
            | Opcode::Fsqrt
            | Opcode::Band
            | Opcode::Bor
            | Opcode::Bxor
            | Opcode::Bnot
            | Opcode::Ishl
            | Opcode::Ushr
            | Opcode::Sshr
            | Opcode::Icmp { .. }
            | Opcode::Fcmp { .. }
            | Opcode::Sextend
            | Opcode::Uextend
            | Opcode::Fptrunc
            | Opcode::Fpext
            | Opcode::Fptosi
            | Opcode::Sitofp
            | Opcode::Fptoui
            | Opcode::Uitofp
            | Opcode::Ptrtoint
            | Opcode::Inttoptr
            | Opcode::Ireduce
            | Opcode::Bitcast
            | Opcode::Select
            | Opcode::Copy
            | Opcode::Clz
            | Opcode::Ctz
            | Opcode::Popcnt
            | Opcode::Bswap
            | Opcode::Abs
    )
}

/// 对单个函数执行局部 CSE。
pub fn eliminate_common_subexpressions(func: &mut Function) -> Result<PassResult, IrError> {
    let mut result = PassResult::default();
    // Value → Value 替换映射（被消除的 result → 保留的 result）
    let mut replacements: HashMap<Value, Value> = HashMap::new();
    // 待删除的重复表达式指令（循环后统一 kill，避免与借用冲突）
    let mut to_kill: Vec<Inst> = Vec::new();
    // P1-5：别名分析（load 消重的 kill 精度）
    let alias = AliasAnalysis::new();

    let block_count = func.dfg.blocks.len();
    for bi in 0..block_count {
        // 每个块独立维护表达式表
        let mut expr_table: HashMap<ExprKey, Value> = HashMap::new();

        // 借用 inst_order（blocks 与 insts 为 dfg 不同字段，可拆分借用）
        let inst_ids = &func.dfg.blocks[bi].inst_order;

        for inst_id in inst_ids {
            // 只读借用（CSE 不改指令；别名查询需要 &func，不可持 &mut）
            let inst = &func.dfg.insts[inst_id.0 as usize];

            // P0-3：内存写指令 kill load 表达式——否则 `load p; store v,p;
            // load p` 第二个 load 被错误消除。
            // P0-4：kill 集合覆盖全部 may-write（Fstore/Call/CallIndirect/
            // AtomicRmw/Cmpxchg），不只 Store。
            // P1-5：store/原子按别名精化——只 kill may-alias 的 load
            // （不同栈槽 / 全局互不干扰）；调用可能写任意内存 → kill 全部。
            if matches!(
                inst.opcode,
                Opcode::Store | Opcode::Fstore | Opcode::AtomicRmw | Opcode::Cmpxchg
            ) {
                let wloc = alias.location_of_access(func, inst);
                expr_table.retain(|key, _| !killed_by_write(key, wloc, &alias, func));
                continue; // 这些指令本身不参与消重（有副作用/内存）
            }
            if matches!(inst.opcode, Opcode::Call | Opcode::CallIndirect) {
                expr_table.retain(|key, _| !is_load_op(&key.opcode));
                continue;
            }

            // 跳过无结果的指令
            let inst_result = match inst.results.first().copied() {
                Some(v) => v,
                None => continue,
            };

            // P0-6：多结果指令（overflow 系）不进消重表——CSE 只映射
            // results[0]，results[1]（flag）会悬空指向被 kill 的指令。
            if inst.results.len() > 1 {
                continue;
            }

            // P0-3：volatile load 不参与 CSE（可观察语义）。
            if is_load_op(&inst.opcode)
                && inst.mem_flags.contains(forge_ir::mem_flags::MemFlags::VOLATILE)
            {
                continue;
            }

            // 跳过不可 CSE 的指令（P1-5：load 在 kill 精化下可消重）
            if !is_cse_candidate(&inst.opcode) && !is_load_op(&inst.opcode) {
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

    /// P0-2 负向：`icmp eq a,b` 与 `icmp ne a,b` 必须**不**互相消除
    /// （旧 opcode_discriminant 把 Icmp 统一映射 23，二者被错误合并 → 错值）。
    #[test]
    fn p0_icmp_cond_distinct() {
        let sig = FunctionSignature::new(&[(TypeId::I32, "a"), (TypeId::I32, "b")], &[TypeId::BOOL]);
        let mut b = FunctionBuilder::new("test", TypeContext::new(), sig);
        let (entry, params) = b.create_block_with_params(&[(TypeId::I32, "a"), (TypeId::I32, "b")]);
        b.switch_to_block(entry);
        let a = params[0];
        let bv = params[1];
        let eq = b.icmp(forge_ir::IntCC::Equal, a, bv);
        let ne = b.icmp(forge_ir::IntCC::NotEqual, a, bv);
        let and = b.band(eq, ne);
        b.ret(&[and]);

        let mut func = b.finish().expect("build");
        let pass = CsePass::new();
        let r = pass.run_on_function(&mut func).unwrap();
        // eq 与 ne 是不同的表达式 → CSE 必须不消除任何一条
        assert!(
            !r.changed,
            "icmp eq/ne 不应互相消除（P0-2 回归：eq 与 ne 必须区分）"
        );
    }

    /// P0-3 负向：`load p; store v,p; load p`——第二个 load 必须保留
    /// （旧 CSE 块内 expr_table 全程保留，store 不 kill load → 错值）。
    #[test]
    fn p0_store_kills_load() {
        let sig = FunctionSignature::new(&[(TypeId::I32, "p"), (TypeId::I32, "v")], &[TypeId::I32]);
        let mut b = FunctionBuilder::new("test", TypeContext::new(), sig);
        let (entry, params) = b.create_block_with_params(&[(TypeId::I32, "p"), (TypeId::I32, "v")]);
        b.switch_to_block(entry);
        let p = params[0];
        let v = params[1];
        let l1 = b.load(p, TypeId::I32);
        b.store(v, p); // 写 p——必须 kill 前面的 load 表达式
        let l2 = b.load(p, TypeId::I32);
        let sum = b.iadd(l1, l2);
        b.ret(&[sum]);

        let mut func = b.finish().expect("build");
        let pass = CsePass::new();
        let r = pass.run_on_function(&mut func).unwrap();
        // store 在 l1 与 l2 之间 → l2 依赖 store 后的新值，不可消除
        assert!(
            !r.changed,
            "store 必须 kill load 表达式（P0-3 回归：load 不能被错误消除）"
        );
    }

    /// P0-6 负向：多结果指令（SaddOverflow）不参与 CSE——results[1]
    /// （overflow flag）若悬空指向被 kill 的指令会错值。
    #[test]
    fn p0_multi_result_not_cse() {
        // 单返回（无签名约束）：只验证两条 sadd_overflow 都不被 CSE。
        let sig = FunctionSignature::new(
            &[(TypeId::I32, "a"), (TypeId::I32, "b")],
            &[TypeId::I32],
        );
        let mut b = FunctionBuilder::new("test", TypeContext::new(), sig);
        let (entry, params) = b.create_block_with_params(&[(TypeId::I32, "a"), (TypeId::I32, "b")]);
        b.switch_to_block(entry);
        let a = params[0];
        let bv = params[1];
        let (s1, _of1) = b.sadd_overflow(a, bv);
        let (s2, _of2) = b.sadd_overflow(a, bv); // 重复——但多结果，不可消
        let sum = b.iadd(s1, s2);
        b.ret(&[sum]);

        let mut func = b.finish().expect("build");
        let pass = CsePass::new();
        let r = pass.run_on_function(&mut func).unwrap();
        assert!(
            !r.changed,
            "多结果指令不应被 CSE（P0-6 回归：overflow flag 悬空）"
        );
    }

    /// P1-5 正向：`load p0; store v,p1; load p0`（p0/p1 为不同栈槽）——
    /// store 写 p1 不 kill p0 的 load，第二个 load 被消除。
    /// （旧行为：任何 store kill 全部 load → 无法消重。）
    #[test]
    fn p1_5_cse_loads_across_non_alias_store() {
        let sig = FunctionSignature::new(&[], &[TypeId::I32]);
        let mut b = FunctionBuilder::new("test", TypeContext::new(), sig);
        let entry = b.create_block();
        b.switch_to_block(entry);
        let p0 = b.stack_addr(0);
        let p1 = b.stack_addr(1);
        let l1 = b.load(p0, TypeId::I32);
        let sv = b.iconst_i32(7);
        b.store(sv, p1); // 写 slot1 —— 与 slot0 NoAlias
        let l2 = b.load(p0, TypeId::I32); // 可复用 l1
        let sum = b.iadd(l1, l2);
        b.ret(&[sum]);

        let mut func = b.finish().expect("build");
        let pass = CsePass::new();
        let r = pass.run_on_function(&mut func).unwrap();
        assert!(
            r.changed,
            "不同栈槽的 store 不应 kill load（P1-5：别名精化 kill）"
        );
        // sum 的两个操作数都应指向 l1（l2 被消除）
        let ValueDef::Inst(sum_inst, _) = func.dfg.value_def(sum).unwrap() else {
            panic!("sum 应是指令定义")
        };
        let ops = &func.dfg.insts[sum_inst.0 as usize].operands;
        assert_eq!(ops.as_slice(), &[l1, l1], "l2 应被消除并复用 l1");
    }

    /// P1-5 负向：`load p0; store v,p0; load p0`（同栈槽）——store 与 load
    /// MayAlias → 第二个 load 必须保留。
    #[test]
    fn p1_5_cse_load_killed_by_same_slot_store() {
        let sig = FunctionSignature::new(&[], &[TypeId::I32]);
        let mut b = FunctionBuilder::new("test", TypeContext::new(), sig);
        let entry = b.create_block();
        b.switch_to_block(entry);
        let p0 = b.stack_addr(0);
        let l1 = b.load(p0, TypeId::I32);
        let sv = b.iconst_i32(7);
        b.store(sv, p0); // 写同一槽 —— MayAlias
        let l2 = b.load(p0, TypeId::I32);
        let sum = b.iadd(l1, l2);
        b.ret(&[sum]);

        let mut func = b.finish().expect("build");
        let pass = CsePass::new();
        let r = pass.run_on_function(&mut func).unwrap();
        assert!(
            !r.changed,
            "同槽 store 必须 kill load（P1-5 负向：MayAlias 不消重）"
        );
    }
}
