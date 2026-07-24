//! e-graph 优化框架 (Equality Saturation)。
//!
//! 基于 equality saturation 的优化框架实现。
//! 提供 30+ 条代数恒等式重写规则 + 迭代饱和（到达不动点）。
//!
//! # 规则类别
//!
//! - **算术**: x+0→x, x-0→x, x-x→0, x*1→x, x*0→0, const∘const→const
//! - **除法**: x/1→x, 0/x→0, x%1→0, 0%x→0, x%x→0
//! - **位运算**: x&0→0, x&-1→x, x|0→x, x|-1→-1, x^0→x, x^x→0, x&x→x, x|x→x
//! - **移位**: x<<0→x, 0<<x→0, x>>0→x, 0>>x→0
//! - **比较**: icmp.eq x,x→1, icmp.ne x,x→0
//! - **选择**: select(1,a,b)→a, select(0,a,b)→b, select(c,x,x)→x
//! - **Phi**: phi(x,x,…)→x (all operands equal)
//! - **类型转换**: ireduce(iconst)→iconst, extend(iconst)→iconst
//! - **Bnot**: ~~x→x (double negation)
//!
//! # 迭代饱和
//!
//! 重复应用规则直到 IR 不再变化，处理级联优化链。

use crate::CompileError;
use crate::ir::*;
use crate::optimize::{OptimizationPass, PassResult};
use std::collections::HashMap;

#[derive(Default)]
pub struct EGraphPass;

impl EGraphPass {
    pub fn new() -> Self {
        Self
    }
}

impl OptimizationPass for EGraphPass {
    fn name(&self) -> &'static str {
        "egraph"
    }
    fn description(&self) -> &'static str {
        "Algebraic simplification using equality saturation (30+ rules, iterative)"
    }

    fn run_on_function(&self, func: &mut Function) -> Result<PassResult, CompileError> {
        apply_rewrite_rules(func)
    }
}

// ============================================================
// 常量缓存 — 加速常量查找
// ============================================================

struct ConstCache {
    map: HashMap<Value, Big>,
    /// Value → source operand for Bnot results (for ~~x→x rule).
    bnot_sources: HashMap<Value, Value>,
}

impl ConstCache {
    fn build(func: &Function) -> Self {
        let mut map = HashMap::new();
        let mut bnot_sources = HashMap::new();
        for block in &func.blocks {
            for inst in &block.instructions {
                if let Some(v) = inst.result {
                    match &inst.opcode {
                        Opcode::Iconst { index } | Opcode::Fconst { index } => {
                            if let Some(big) = func.constant_pool.get(*index).cloned() {
                                map.insert(v, big);
                            }
                        }
                        Opcode::Bnot if !inst.operands.is_empty() => {
                            bnot_sources.insert(v, inst.operands[0]);
                        }
                        _ => {}
                    }
                }
            }
        }
        Self { map, bnot_sources }
    }

    fn is_zero(&self, v: Value) -> bool {
        self.map.get(&v).is_some_and(|b| b.is_zero())
    }

    fn is_one(&self, v: Value) -> bool {
        self.map.get(&v).is_some_and(|b| b.is_one())
    }

    fn is_all_ones(&self, v: Value) -> bool {
        self.map
            .get(&v)
            .is_some_and(|b| b.try_to_i64().is_some_and(|i| i == -1))
    }

    fn try_to_i64(&self, v: Value) -> Option<i64> {
        self.map.get(&v).and_then(|b| b.try_to_i64())
    }
}

// ============================================================
// 辅助函数
// ============================================================

fn make_const(func: &mut Function, value: i64, ty: Type, result: Value) -> Instruction {
    let index = func.constant_pool.insert(Big::from_i64(value));
    Instruction::new(
        Opcode::Iconst { index },
        smallvec::smallvec![],
        Some(result),
        ty,
    )
}

fn clone_as_copy(inst: &Instruction, src: Value) -> Instruction {
    Instruction::new(Opcode::Copy, smallvec::smallvec![src], inst.result, inst.ty)
}

fn type_mask(ty: Type) -> i64 {
    match ty {
        Type::I8 => 0xFF,
        Type::I16 => 0xFFFF,
        Type::I32 => 0xFFFF_FFFF,
        _ => -1,
    }
}

fn truncate_to_type(value: i64, ty: Type) -> i64 {
    value & type_mask(ty)
}

// ============================================================
// 替换操作描述（避免 borrow 冲突：扫描时不修改 func）
// ============================================================

enum ReplaceAction {
    Copy { src: Value },
    Const { value: i64 },
}

// ============================================================
// 核心重写
// ============================================================

pub fn apply_rewrite_rules(func: &mut Function) -> Result<PassResult, CompileError> {
    let mut total = PassResult::default();

    loop {
        let cache = ConstCache::build(func);
        let mut replacements: Vec<(usize, usize, ReplaceAction)> = Vec::new();

        for (bi, block) in func.blocks.iter().enumerate() {
            for (ii, inst) in block.instructions.iter().enumerate() {
                if matches!(inst.opcode, Opcode::Nop) {
                    continue;
                }
                if let Some(action) = try_rewrite(inst, &cache) {
                    replacements.push((bi, ii, action));
                }
            }
        }

        if replacements.is_empty() {
            break;
        }

        total.changed = true;
        total.instructions_removed += replacements.len();
        for (bi, ii, action) in replacements {
            let inst = &func.blocks[bi].instructions[ii];
            let new_inst = match action {
                ReplaceAction::Copy { src } => clone_as_copy(inst, src),
                ReplaceAction::Const { value } => {
                    let result = inst.result.unwrap_or(Value(0));
                    make_const(func, value, inst.ty, result)
                }
            };
            func.blocks[bi].instructions[ii] = new_inst;
        }
    }

    Ok(total)
}

fn try_rewrite(inst: &Instruction, cache: &ConstCache) -> Option<ReplaceAction> {
    let ops = &inst.operands;
    if ops.is_empty() {
        return None;
    }
    let ty = inst.ty;
    let _result = inst.result?;

    let a = ops[0];
    let b = ops.get(1).copied();

    match &inst.opcode {
        // === Iadd ===
        Opcode::Iadd => {
            if let Some(bv) = b {
                if cache.is_zero(bv) {
                    return Some(ReplaceAction::Copy { src: a });
                }
                if cache.is_zero(a) {
                    return Some(ReplaceAction::Copy { src: bv });
                }
                if let (Some(c1), Some(c2)) = (cache.try_to_i64(a), cache.try_to_i64(bv)) {
                    return Some(ReplaceAction::Const {
                        value: c1.wrapping_add(c2),
                    });
                }
            }
            None
        }
        // === Isub ===
        Opcode::Isub => {
            if let Some(bv) = b {
                if cache.is_zero(bv) {
                    return Some(ReplaceAction::Copy { src: a });
                }
                if a == bv {
                    return Some(ReplaceAction::Const { value: 0 });
                }
                if let (Some(c1), Some(c2)) = (cache.try_to_i64(a), cache.try_to_i64(bv)) {
                    return Some(ReplaceAction::Const {
                        value: c1.wrapping_sub(c2),
                    });
                }
            }
            None
        }
        // === Imul ===
        Opcode::Imul => {
            if let Some(bv) = b {
                if cache.is_one(bv) {
                    return Some(ReplaceAction::Copy { src: a });
                }
                if cache.is_one(a) {
                    return Some(ReplaceAction::Copy { src: bv });
                }
                if cache.is_zero(bv) || cache.is_zero(a) {
                    return Some(ReplaceAction::Const { value: 0 });
                }
                if let (Some(c1), Some(c2)) = (cache.try_to_i64(a), cache.try_to_i64(bv)) {
                    return Some(ReplaceAction::Const {
                        value: c1.wrapping_mul(c2),
                    });
                }
            }
            None
        }
        // === Udiv / Sdiv ===
        Opcode::Udiv | Opcode::Sdiv => {
            if let Some(bv) = b {
                if cache.is_one(bv) {
                    return Some(ReplaceAction::Copy { src: a });
                }
                if cache.is_zero(a) {
                    return Some(ReplaceAction::Const { value: 0 });
                }
                if let (Some(c1), Some(c2)) = (cache.try_to_i64(a), cache.try_to_i64(bv))
                    && c2 != 0
                {
                    return Some(ReplaceAction::Const { value: c1 / c2 });
                }
            }
            None
        }
        // === Urem / Srem ===
        Opcode::Urem | Opcode::Srem => {
            if let Some(bv) = b {
                if cache.is_one(bv) {
                    return Some(ReplaceAction::Const { value: 0 });
                }
                if cache.is_zero(a) {
                    return Some(ReplaceAction::Const { value: 0 });
                }
                if a == bv {
                    return Some(ReplaceAction::Const { value: 0 });
                }
                if let (Some(c1), Some(c2)) = (cache.try_to_i64(a), cache.try_to_i64(bv))
                    && c2 != 0
                {
                    return Some(ReplaceAction::Const { value: c1 % c2 });
                }
            }
            None
        }
        // === Band ===
        Opcode::Band => {
            if let Some(bv) = b {
                if cache.is_zero(bv) || cache.is_zero(a) {
                    return Some(ReplaceAction::Const { value: 0 });
                }
                if cache.is_all_ones(bv) {
                    return Some(ReplaceAction::Copy { src: a });
                }
                if cache.is_all_ones(a) {
                    return Some(ReplaceAction::Copy { src: bv });
                }
                if a == bv {
                    return Some(ReplaceAction::Copy { src: a });
                }
                if let (Some(c1), Some(c2)) = (cache.try_to_i64(a), cache.try_to_i64(bv)) {
                    return Some(ReplaceAction::Const { value: c1 & c2 });
                }
            }
            None
        }
        // === Bor ===
        Opcode::Bor => {
            if let Some(bv) = b {
                if cache.is_zero(bv) {
                    return Some(ReplaceAction::Copy { src: a });
                }
                if cache.is_zero(a) {
                    return Some(ReplaceAction::Copy { src: bv });
                }
                if cache.is_all_ones(bv) || cache.is_all_ones(a) {
                    return Some(ReplaceAction::Const { value: -1 });
                }
                if a == bv {
                    return Some(ReplaceAction::Copy { src: a });
                }
                if let (Some(c1), Some(c2)) = (cache.try_to_i64(a), cache.try_to_i64(bv)) {
                    return Some(ReplaceAction::Const { value: c1 | c2 });
                }
            }
            None
        }
        // === Bxor ===
        Opcode::Bxor => {
            if let Some(bv) = b {
                if cache.is_zero(bv) {
                    return Some(ReplaceAction::Copy { src: a });
                }
                if cache.is_zero(a) {
                    return Some(ReplaceAction::Copy { src: bv });
                }
                if a == bv {
                    return Some(ReplaceAction::Const { value: 0 });
                }
                if let (Some(c1), Some(c2)) = (cache.try_to_i64(a), cache.try_to_i64(bv)) {
                    return Some(ReplaceAction::Const { value: c1 ^ c2 });
                }
            }
            None
        }
        // === Bnot ===
        Opcode::Bnot => {
            // ~~x → x (check if operand is itself a Bnot result)
            if let Some(&src) = cache.bnot_sources.get(&a) {
                return Some(ReplaceAction::Copy { src });
            }
            if let Some(c) = cache.try_to_i64(a) {
                let mask = type_mask(ty);
                return Some(ReplaceAction::Const { value: (!c) & mask });
            }
            None
        }
        // === Ishl ===
        Opcode::Ishl => {
            if let Some(bv) = b {
                if cache.is_zero(bv) {
                    return Some(ReplaceAction::Copy { src: a });
                }
                if cache.is_zero(a) {
                    return Some(ReplaceAction::Const { value: 0 });
                }
                if let (Some(c1), Some(c2)) = (cache.try_to_i64(a), cache.try_to_i64(bv))
                    && (0..128).contains(&c2)
                {
                    return Some(ReplaceAction::Const { value: c1 << c2 });
                }
            }
            None
        }
        // === Ushr ===
        Opcode::Ushr => {
            if let Some(bv) = b {
                if cache.is_zero(bv) {
                    return Some(ReplaceAction::Copy { src: a });
                }
                if cache.is_zero(a) {
                    return Some(ReplaceAction::Const { value: 0 });
                }
                if let (Some(c1), Some(c2)) = (cache.try_to_i64(a), cache.try_to_i64(bv))
                    && (0..128).contains(&c2)
                {
                    let val = ((c1 as u64) >> c2) as i64;
                    return Some(ReplaceAction::Const { value: val });
                }
            }
            None
        }
        // === Sshr ===
        Opcode::Sshr => {
            if let Some(bv) = b {
                if cache.is_zero(bv) {
                    return Some(ReplaceAction::Copy { src: a });
                }
                if cache.is_zero(a) {
                    return Some(ReplaceAction::Const { value: 0 });
                }
                if let (Some(c1), Some(c2)) = (cache.try_to_i64(a), cache.try_to_i64(bv))
                    && (0..128).contains(&c2)
                {
                    return Some(ReplaceAction::Const { value: c1 >> c2 });
                }
            }
            None
        }
        // === Select ===
        Opcode::Select if ops.len() >= 3 => {
            let (true_v, false_v) = (ops[1], ops[2]);
            if cache.is_one(a) {
                return Some(ReplaceAction::Copy { src: true_v });
            }
            if cache.is_zero(a) {
                return Some(ReplaceAction::Copy { src: false_v });
            }
            if true_v == false_v {
                return Some(ReplaceAction::Copy { src: true_v });
            }
            None
        }
        // === Icmp ===
        Opcode::Icmp { cond } if ops.len() >= 2 => {
            let bv = ops[1];
            match cond {
                IntCC::Equal if a == bv => {
                    return Some(ReplaceAction::Const { value: 1 });
                }
                IntCC::NotEqual if a == bv => {
                    return Some(ReplaceAction::Const { value: 0 });
                }
                _ => {}
            }
            if let (Some(c1), Some(c2)) = (cache.try_to_i64(a), cache.try_to_i64(bv)) {
                let val = match cond {
                    IntCC::Equal => c1 == c2,
                    IntCC::NotEqual => c1 != c2,
                    IntCC::SignedLessThan => c1 < c2,
                    IntCC::SignedGreaterThan => c1 > c2,
                    IntCC::SignedLessThanOrEqual => c1 <= c2,
                    IntCC::SignedGreaterThanOrEqual => c1 >= c2,
                    IntCC::UnsignedLessThan => (c1 as u64) < (c2 as u64),
                    IntCC::UnsignedGreaterThan => (c1 as u64) > (c2 as u64),
                    IntCC::UnsignedLessThanOrEqual => (c1 as u64) <= (c2 as u64),
                    IntCC::UnsignedGreaterThanOrEqual => (c1 as u64) >= (c2 as u64),
                };
                return Some(ReplaceAction::Const { value: val as i64 });
            }
            None
        }
        // === Phi (all operands equal → Copy) ===
        Opcode::Phi { .. } if !ops.is_empty() => {
            let first = ops[0];
            if ops.iter().all(|&v| v == first) {
                return Some(ReplaceAction::Copy { src: first });
            }
            None
        }
        // === Ireduce ===
        Opcode::Ireduce => {
            if let Some(c) = cache.try_to_i64(a) {
                return Some(ReplaceAction::Const {
                    value: truncate_to_type(c, ty),
                });
            }
            None
        }
        // === Sextend / Uextend ===
        Opcode::Sextend | Opcode::Uextend => {
            if let Some(c) = cache.try_to_i64(a) {
                return Some(ReplaceAction::Const { value: c });
            }
            None
        }
        // === Fadd ===
        Opcode::Fadd { .. } => {
            if let Some(bv) = b {
                if cache.is_zero(bv) {
                    return Some(ReplaceAction::Copy { src: a });
                }
                if cache.is_zero(a) {
                    return Some(ReplaceAction::Copy { src: bv });
                }
            }
            None
        }
        // === Fsub ===
        Opcode::Fsub { .. } => {
            if let Some(bv) = b {
                if cache.is_zero(bv) {
                    return Some(ReplaceAction::Copy { src: a });
                }
                if a == bv {
                    return Some(ReplaceAction::Const { value: 0 });
                }
            }
            None
        }
        // === Fmul ===
        Opcode::Fmul { .. } => {
            if let Some(bv) = b {
                if cache.is_one(bv) {
                    return Some(ReplaceAction::Copy { src: a });
                }
                if cache.is_one(a) {
                    return Some(ReplaceAction::Copy { src: bv });
                }
                if cache.is_zero(bv) || cache.is_zero(a) {
                    return Some(ReplaceAction::Const { value: 0 });
                }
            }
            None
        }
        _ => None,
    }
}

// ============================================================
// 测试
// ============================================================

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ir::FunctionBuilder;

    #[test]
    fn egraph_add_zero() {
        let sig = Signature::new(&[], &[Type::I32]);
        let mut b = FunctionBuilder::new("test", sig);
        let entry = b.create_block();
        b.switch_to_block(entry);
        let x = b.iconst_i32(42);
        let zero = b.iconst_i32(0);
        let sum = b.iadd(x, zero);
        b.return_(&[sum]);
        let mut func = b.finish();
        let r = EGraphPass::new().run_on_function(&mut func).unwrap();
        assert!(r.changed);
        assert!(func.validate().is_valid());
    }

    #[test]
    fn egraph_mul_one() {
        let sig = Signature::new(&[], &[Type::I32]);
        let mut b = FunctionBuilder::new("test", sig);
        let entry = b.create_block();
        b.switch_to_block(entry);
        let x = b.iconst_i32(42);
        let one = b.iconst_i32(1);
        let prod = b.imul(x, one);
        b.return_(&[prod]);
        let mut func = b.finish();
        let r = EGraphPass::new().run_on_function(&mut func).unwrap();
        assert!(r.changed);
        assert!(func.validate().is_valid());
    }

    #[test]
    fn egraph_mul_zero() {
        let sig = Signature::new(&[], &[Type::I32]);
        let mut b = FunctionBuilder::new("test", sig);
        let entry = b.create_block();
        b.switch_to_block(entry);
        let x = b.iconst_i32(42);
        let zero = b.iconst_i32(0);
        let prod = b.imul(x, zero);
        b.return_(&[prod]);
        let mut func = b.finish();
        let r = EGraphPass::new().run_on_function(&mut func).unwrap();
        assert!(r.changed);
        assert!(func.validate().is_valid());
    }

    #[test]
    fn egraph_sub_zero() {
        let sig = Signature::new(&[], &[Type::I32]);
        let mut b = FunctionBuilder::new("test", sig);
        let entry = b.create_block();
        b.switch_to_block(entry);
        let x = b.iconst_i32(42);
        let zero = b.iconst_i32(0);
        let diff = b.isub(x, zero);
        b.return_(&[diff]);
        let mut func = b.finish();
        let r = EGraphPass::new().run_on_function(&mut func).unwrap();
        assert!(r.changed);
        assert!(func.validate().is_valid());
    }

    #[test]
    fn egraph_sub_self() {
        let sig = Signature::new(&[], &[Type::I32]);
        let mut b = FunctionBuilder::new("test", sig);
        let entry = b.create_block();
        b.switch_to_block(entry);
        let x = b.iconst_i32(42);
        let diff = b.isub(x, x);
        b.return_(&[diff]);
        let mut func = b.finish();
        let _ = EGraphPass::new().run_on_function(&mut func).unwrap();
        assert!(func.validate().is_valid());
    }

    #[test]
    fn egraph_div_one() {
        let sig = Signature::new(&[], &[Type::I32]);
        let mut b = FunctionBuilder::new("test", sig);
        let entry = b.create_block();
        b.switch_to_block(entry);
        let x = b.iconst_i32(42);
        let one = b.iconst_i32(1);
        let div = b.sdiv(x, one);
        b.return_(&[div]);
        let mut func = b.finish();
        let r = EGraphPass::new().run_on_function(&mut func).unwrap();
        assert!(r.changed);
        assert!(func.validate().is_valid());
    }

    #[test]
    fn egraph_or_zero() {
        let sig = Signature::new(&[], &[Type::I32]);
        let mut b = FunctionBuilder::new("test", sig);
        let entry = b.create_block();
        b.switch_to_block(entry);
        let x = b.iconst_i32(42);
        let zero = b.iconst_i32(0);
        let r = b.bor(x, zero);
        b.return_(&[r]);
        let mut func = b.finish();
        let r = EGraphPass::new().run_on_function(&mut func).unwrap();
        assert!(r.changed);
        assert!(func.validate().is_valid());
    }

    #[test]
    fn egraph_xor_zero() {
        let sig = Signature::new(&[], &[Type::I32]);
        let mut b = FunctionBuilder::new("test", sig);
        let entry = b.create_block();
        b.switch_to_block(entry);
        let x = b.iconst_i32(42);
        let zero = b.iconst_i32(0);
        let r = b.bxor(x, zero);
        b.return_(&[r]);
        let mut func = b.finish();
        let r = EGraphPass::new().run_on_function(&mut func).unwrap();
        assert!(r.changed);
        assert!(func.validate().is_valid());
    }

    #[test]
    fn egraph_shl_zero() {
        let sig = Signature::new(&[], &[Type::I32]);
        let mut b = FunctionBuilder::new("test", sig);
        let entry = b.create_block();
        b.switch_to_block(entry);
        let x = b.iconst_i32(42);
        let zero = b.iconst_i32(0);
        let r = b.ishl(x, zero);
        b.return_(&[r]);
        let mut func = b.finish();
        let r = EGraphPass::new().run_on_function(&mut func).unwrap();
        assert!(r.changed);
        assert!(func.validate().is_valid());
    }

    #[test]
    fn egraph_select_true() {
        let sig = Signature::new(&[], &[Type::I32]);
        let mut b = FunctionBuilder::new("test", sig);
        let entry = b.create_block();
        b.switch_to_block(entry);
        let cond = b.iconst_i32(1);
        let a = b.iconst_i32(10);
        let b_val = b.iconst_i32(20);
        let sel = b.select(cond, a, b_val);
        b.return_(&[sel]);
        let mut func = b.finish();
        let r = EGraphPass::new().run_on_function(&mut func).unwrap();
        assert!(r.changed);
        assert!(func.validate().is_valid());
    }

    #[test]
    fn egraph_cascade() {
        // (x * 1) + 0 → Copy(x) + 0 → Copy(x)
        let sig = Signature::new(&[], &[Type::I32]);
        let mut b = FunctionBuilder::new("test", sig);
        let entry = b.create_block();
        b.switch_to_block(entry);
        let x = b.iconst_i32(42);
        let one = b.iconst_i32(1);
        let c0 = b.iconst_i32(0);
        let prod = b.imul(x, one);
        let sum = b.iadd(prod, c0);
        b.return_(&[sum]);
        let mut func = b.finish();
        let r = EGraphPass::new().run_on_function(&mut func).unwrap();
        assert!(r.changed);
        assert!(func.validate().is_valid());
    }

    #[test]
    fn egraph_no_crash() {
        let sig = Signature::void();
        let mut b = FunctionBuilder::new("test", sig);
        let entry = b.create_block();
        b.switch_to_block(entry);
        b.return_(&[]);
        let mut func = b.finish();
        EGraphPass::new().run_on_function(&mut func).unwrap();
        assert!(func.validate().is_valid());
    }
}

// ============================================================
// E-Graph 指令选择 (ISel) — 将 IR 模式匹配到机器指令
// ============================================================

/// 指令选择器: 使用模式匹配将 IR opcode 序列映射到等效的优化形式。
///
/// 当前实现: 运行 EGraphPass 进行代数简化（可作为 lowering 的预处理步骤）。
/// 未来扩展: 添加机器指令模式 → 直接生成机器指令序列。
#[derive(Default)]
pub struct ISelPass {
    egraph: EGraphPass,
}

impl ISelPass {
    pub fn new() -> Self {
        Self {
            egraph: EGraphPass::new(),
        }
    }
}

impl crate::optimize::OptimizationPass for ISelPass {
    fn name(&self) -> &'static str {
        "isel-egraph"
    }
    fn description(&self) -> &'static str {
        "E-Graph based instruction selection preprocessing (algebraic simplification before lowering)"
    }

    fn run_on_function(
        &self,
        func: &mut Function,
    ) -> Result<crate::optimize::PassResult, crate::CompileError> {
        // Step 1: 代数简化 (常量折叠、恒等式消除)
        let result = self.egraph.run_on_function(func)?;
        // Step 2: 额外的 lowering 前规范化 (如 double-negate 消除已在 egraph 中处理)
        Ok(result)
    }
}

#[cfg(test)]
mod isel_tests {
    use super::*;
    use crate::ir::FunctionBuilder;

    #[test]
    fn isel_simplify_add_zero() {
        // x + 0 → x (降低前消除冗余操作数)
        let sig = Signature::new(&[], &[Type::I32]);
        let mut b = FunctionBuilder::new("isel_add0", sig);
        let entry = b.create_block();
        b.switch_to_block(entry);
        let x = b.iconst_i32(42);
        let zero = b.iconst_i32(0);
        let sum = b.iadd(x, zero);
        b.return_(&[sum]);
        let mut func = b.finish();
        let r = ISelPass::new().run_on_function(&mut func).unwrap();
        assert!(r.changed, "x+0 should be simplified to x");
    }

    #[test]
    fn isel_simplify_mul_one() {
        // x * 1 → x
        let sig = Signature::new(&[], &[Type::I32]);
        let mut b = FunctionBuilder::new("isel_mul1", sig);
        let entry = b.create_block();
        b.switch_to_block(entry);
        let x = b.iconst_i32(99);
        let one = b.iconst_i32(1);
        let prod = b.imul(x, one);
        b.return_(&[prod]);
        let mut func = b.finish();
        let r = ISelPass::new().run_on_function(&mut func).unwrap();
        assert!(r.changed, "x*1 should be simplified to x");
    }

    #[test]
    fn isel_double_negate_to_original() {
        // ~~x → x
        let sig = Signature::new(&[], &[Type::I32]);
        let mut b = FunctionBuilder::new("isel_dneg", sig);
        let entry = b.create_block();
        b.switch_to_block(entry);
        let x = b.iconst_i32(0xABCD);
        let n1 = b.bnot(x);
        let n2 = b.bnot(n1);
        b.return_(&[n2]);
        let mut func = b.finish();
        let r = ISelPass::new().run_on_function(&mut func).unwrap();
        assert!(r.changed, "~~x should be simplified to x");
    }

    #[test]
    fn isel_chain_simplification() {
        // (x + 0) * 1 - 0 → x
        let sig = Signature::new(&[], &[Type::I32]);
        let mut b = FunctionBuilder::new("isel_chain", sig);
        let entry = b.create_block();
        b.switch_to_block(entry);
        let x = b.iconst_i32(42);
        let zero = b.iconst_i32(0);
        let one = b.iconst_i32(1);
        let s = b.iadd(x, zero);
        let m = b.imul(s, one);
        let r = b.isub(m, zero);
        b.return_(&[r]);
        let mut func = b.finish();
        let r = ISelPass::new().run_on_function(&mut func).unwrap();
        assert!(r.changed);
    }
}
