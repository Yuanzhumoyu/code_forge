//! 常量折叠 pass。
//!
//! 采用 worklist-driven 算法：当一条指令的所有操作数都是编译时已知的常量时，
//! 在编译时求值该操作，将结果替换为 Iconst/Fconst，并传播到所有使用者。
//!
//! # Algorithm
//!
//! 1. SCAN: 遍历所有指令，记录 Iconst/Fconst 到 known map
//! 2. SEED: 将所有非常量指令的 result 加入 worklist
//! 3. LOOP: worklist 驱动折叠直到不动点
//! 4. BRANCH_FOLD: 常量条件分支 → 无条件跳转
//! 5. 返回 PassResult

use crate::CompileError;
use crate::ir::*;
use crate::ir::{Big, FloatFormat};
use crate::optimize::{OptimizationPass, PassResult};
use std::collections::HashMap;

// ============================================================
// ConstValue — 编译时已知值
// ============================================================

/// 编译时已知的值。
///
/// 保留类型信息以实现正确的截断、扩展和位宽处理。
/// 例如 `Iconst(0xFF, I8)` 和 `Iconst(0xFF, I32)` 是不同的常量。
#[derive(Clone, Debug, PartialEq)]
pub enum ConstValue {
    /// 整数值（任意精度）+ 类型。
    Int(Big, Type),
    /// 浮点值（任意精度）+ 类型。
    Float(Big, Type),
    /// 布尔值（整数比较的结果）。
    Bool(bool),
}

impl ConstValue {
    /// 如果此值是整数，返回其值和类型。
    pub fn as_int(&self) -> Option<(&Big, Type)> {
        match self {
            ConstValue::Int(v, t) => Some((v, *t)),
            _ => None,
        }
    }

    /// 如果此值是布尔值，返回它。
    pub fn as_bool(&self) -> Option<bool> {
        match self {
            ConstValue::Bool(b) => Some(*b),
            _ => None,
        }
    }

    /// 如果此值是浮点数，返回其值和类型。
    pub fn as_float(&self) -> Option<(&Big, Type)> {
        match self {
            ConstValue::Float(v, t) => Some((v, *t)),
            _ => None,
        }
    }

    /// 转换为布尔值（用于分支条件判断）。
    /// 整数 0 = false，非零 = true。
    pub fn to_bool(&self) -> Option<bool> {
        match self {
            ConstValue::Bool(b) => Some(*b),
            ConstValue::Int(v, _) => Some(!v.is_zero()),
            ConstValue::Float(v, _) => Some(!v.is_zero()),
        }
    }
}

// ============================================================
// 整数宽度辅助函数（基于 Big）
// ============================================================

/// 按类型宽度截断 Big 值（有符号语义）。
fn truncate_to_type(value: &Big, ty: Type) -> Big {
    let bits = ty.bits();
    if bits == 0 || (!ty.is_int() && !ty.is_ptr()) {
        return value.clone();
    }
    value.truncate_to_bits_signed(bits)
}

/// 将 Big 解释为固定位宽的无符号值。
fn as_unsigned_big(value: &Big, ty: Type) -> Big {
    let bits = ty.bits();
    if bits == 0 {
        return Big::U_ZERO;
    }
    value.truncate_to_bits(bits)
}

/// 按位宽计算全 1 掩码。
fn bit_mask(bits: u32) -> Big {
    if bits == 0 {
        return Big::U_ZERO;
    }
    if bits >= 128 {
        return Big::Unsigned(dashu::Natural::from(u128::MAX));
    }
    Big::Unsigned(dashu::Natural::from((1u128 << bits) - 1))
}

// ============================================================
// 折叠 — 核心求值函数
// ============================================================

/// 对给定的操作码和常量操作数进行编译时求值。
///
/// 返回 `Some(ConstValue)` 如果所有操作数已知且可求值。
/// 返回 `None` 如果操作数不足或包含未知值。
pub fn fold_opcode(
    opcode: &Opcode,
    operands: &[ConstValue],
    ty: Type,
) -> Result<Option<ConstValue>, CompileError> {
    match opcode {
        // === 整数算术 ===
        Opcode::Iadd => fold_binary_int(operands, ty, |a, b| a + b),
        Opcode::Isub => fold_binary_int(operands, ty, |a, b| a - b),
        Opcode::Imul => fold_binary_int(operands, ty, |a, b| a * b),
        Opcode::Udiv => {
            let (a, b) = match get_two_bigs(operands) {
                Some(v) => v,
                None => return Ok(None),
            };
            match a.udiv(b) {
                Ok(result) => Ok(Some(ConstValue::Int(truncate_to_type(&result, ty), ty))),
                Err(_) => Err(CompileError::Internal(
                    "compile-time division by zero".into(),
                )),
            }
        }
        Opcode::Sdiv => {
            let (a, b) = match get_two_bigs(operands) {
                Some(v) => v,
                None => return Ok(None),
            };
            match a.sdiv(b) {
                Ok(result) => Ok(Some(ConstValue::Int(truncate_to_type(&result, ty), ty))),
                Err(_) => Err(CompileError::Internal(
                    "compile-time division by zero".into(),
                )),
            }
        }
        Opcode::Urem => {
            let (a, b) = match get_two_bigs(operands) {
                Some(v) => v,
                None => return Ok(None),
            };
            match a.urem(b) {
                Ok(result) => Ok(Some(ConstValue::Int(truncate_to_type(&result, ty), ty))),
                Err(_) => Err(CompileError::Internal(
                    "compile-time division by zero".into(),
                )),
            }
        }
        Opcode::Srem => {
            let (a, b) = match get_two_bigs(operands) {
                Some(v) => v,
                None => return Ok(None),
            };
            match a.srem(b) {
                Ok(result) => Ok(Some(ConstValue::Int(truncate_to_type(&result, ty), ty))),
                Err(_) => Err(CompileError::Internal(
                    "compile-time division by zero".into(),
                )),
            }
        }

        // === 位运算 ===
        Opcode::Band => fold_binary_int(operands, ty, |a, b| a & b),
        Opcode::Bor => fold_binary_int(operands, ty, |a, b| a | b),
        Opcode::Bxor => fold_binary_int(operands, ty, |a, b| a ^ b),
        Opcode::Bnot => {
            if operands.is_empty() {
                return Ok(None);
            }
            match &operands[0] {
                ConstValue::Int(v, _) => {
                    let bits = ty.bits();
                    if bits == 0 {
                        return Ok(Some(ConstValue::Int(Big::S_ZERO, ty)));
                    }
                    let mask = bit_mask(bits);
                    let unsigned_val = v.truncate_to_bits(bits);
                    let result = mask ^ unsigned_val;
                    Ok(Some(ConstValue::Int(truncate_to_type(&result, ty), ty)))
                }
                _ => Ok(None),
            }
        }
        Opcode::Ishl => {
            let (a, b) = match get_two_bigs(operands) {
                Some(v) => v,
                None => return Ok(None),
            };
            let shift = big_to_usize(b);
            let result = a.clone() << shift;
            Ok(Some(ConstValue::Int(truncate_to_type(&result, ty), ty)))
        }
        Opcode::Ushr => {
            let (a, b) = match get_two_bigs(operands) {
                Some(v) => v,
                None => return Ok(None),
            };
            let shift = big_to_usize(b);
            let ua = as_unsigned_big(a, ty);
            let result = ua >> shift;
            Ok(Some(ConstValue::Int(truncate_to_type(&result, ty), ty)))
        }
        Opcode::Sshr => {
            let (a, b) = match get_two_bigs(operands) {
                Some(v) => v,
                None => return Ok(None),
            };
            let shift = big_to_usize(b);
            // For signed shift, convert to signed by truncating to bits first
            let sa = a.truncate_to_bits_signed(ty.bits());
            let result = sa >> shift;
            Ok(Some(ConstValue::Int(truncate_to_type(&result, ty), ty)))
        }

        // === 整数比较 ===
        Opcode::Icmp { cond } => fold_icmp(operands, *cond, ty),

        // === 浮点比较 ===
        Opcode::Fcmp { cond, .. } => fold_fcmp(operands, *cond, ty),

        // === 浮点算术 ===
        Opcode::Fadd { .. } => fold_binary_float(operands, ty, |a, b| a + b),
        Opcode::Fsub { .. } => fold_binary_float(operands, ty, |a, b| a - b),
        Opcode::Fmul { .. } => fold_binary_float(operands, ty, |a, b| a * b),
        Opcode::Fdiv { .. } => fold_binary_float(operands, ty, |a, b| a / b),
        Opcode::Freeze => {
            // Freeze 阻止常量折叠 — 值不能跨 Freeze 被推断
            Ok(None)
        }
        Opcode::ShuffleVector { .. }
        | Opcode::AtomicRmw { .. }
        | Opcode::Cmpxchg { .. }
        | Opcode::Fence { .. }
        | Opcode::ExtractValue { .. }
        | Opcode::InsertValue { .. } => Ok(None),
        Opcode::Fneg { .. } => {
            if operands.is_empty() {
                return Ok(None);
            }
            match &operands[0] {
                ConstValue::Float(v, t) => Ok(Some(ConstValue::Float(-v.clone(), *t))),
                _ => Ok(None),
            }
        }
        Opcode::Fabs { .. } => {
            if operands.is_empty() {
                return Ok(None);
            }
            match &operands[0] {
                ConstValue::Float(v, t) => Ok(Some(ConstValue::Float(v.clone().abs(), *t))),
                _ => Ok(None),
            }
        }
        Opcode::Fsqrt { .. } => {
            if operands.is_empty() {
                return Ok(None);
            }
            match &operands[0] {
                ConstValue::Float(v, t) => {
                    let f = v.to_f64();
                    let result_f = if f < 0.0 { f64::NAN } else { f.sqrt() };
                    let result_big = Big::from_f64(result_f).unwrap_or(Big::F_ZERO);
                    Ok(Some(ConstValue::Float(result_big, *t)))
                }
                _ => Ok(None),
            }
        }

        // === 类型转换 ===
        Opcode::Sextend => fold_extend(operands, ty, true),
        Opcode::Uextend => fold_extend(operands, ty, false),
        Opcode::Ireduce => fold_ireduce(operands, ty),
        Opcode::Bitcast => fold_bitcast(operands, ty),

        // === 选择 ===
        Opcode::Select => {
            if operands.len() < 3 {
                return Ok(None);
            }
            match operands[0].to_bool() {
                Some(true) => Ok(Some(operands[1].clone())),
                Some(false) => Ok(Some(operands[2].clone())),
                None => Ok(None),
            }
        }

        // === Phi / Copy ===
        Opcode::Phi { .. } => {
            if operands.is_empty() {
                return Ok(None);
            }
            // 如果所有 phi 操作数相同，折叠为此值
            let first = &operands[0];
            if operands.iter().all(|op| op == first) {
                Ok(Some(first.clone()))
            } else {
                Ok(None)
            }
        }
        Opcode::Copy => {
            if operands.is_empty() {
                return Ok(None);
            }
            Ok(Some(operands[0].clone()))
        }

        // === 不可折叠的操作码 ===
        Opcode::Load
        | Opcode::Store
        | Opcode::StackLoad { .. }
        | Opcode::StackStore { .. }
        | Opcode::StackAddr { .. }
        | Opcode::GlobalAddr { .. }
        | Opcode::Call { .. }
        | Opcode::CallIndirect
        | Opcode::Vadd
        | Opcode::Vsub
        | Opcode::Vmul
        | Opcode::Vextract { .. }
        | Opcode::Vinsert { .. }
        | Opcode::Nop
        | Opcode::Alloca { .. }
        | Opcode::GetElementPtr { .. } => Ok(None),

        // Iconst/Fconst 在此处不应出现（已在 scan 阶段处理）
        Opcode::Iconst { .. } | Opcode::Fconst { .. } => Ok(None),
    }
}

// ============================================================
// 辅助：从 operands 中提取整数/浮点
// ============================================================

fn get_two_bigs(operands: &[ConstValue]) -> Option<(&Big, &Big)> {
    if operands.len() < 2 {
        return None;
    }
    match (&operands[0], &operands[1]) {
        (ConstValue::Int(a, _), ConstValue::Int(b, _)) => Some((a, b)),
        _ => None,
    }
}

fn big_to_usize(b: &Big) -> usize {
    match b {
        Big::Unsigned(u) => usize::try_from(u).unwrap_or(0),
        Big::Signed(i) => usize::try_from(i).unwrap_or(0),
        Big::Float(f) => f64::try_from(f.clone()).unwrap_or(0.0) as usize,
    }
}

fn fold_binary_int(
    operands: &[ConstValue],
    ty: Type,
    op: fn(Big, Big) -> Big,
) -> Result<Option<ConstValue>, CompileError> {
    let (a, b) = match get_two_bigs(operands) {
        Some(v) => v,
        None => return Ok(None),
    };
    let result = op(a.clone(), b.clone());
    Ok(Some(ConstValue::Int(truncate_to_type(&result, ty), ty)))
}

fn fold_binary_float(
    operands: &[ConstValue],
    ty: Type,
    op: fn(Big, Big) -> Big,
) -> Result<Option<ConstValue>, CompileError> {
    if operands.len() < 2 {
        return Ok(None);
    }
    match (&operands[0], &operands[1]) {
        (ConstValue::Float(a, _), ConstValue::Float(b, _)) => {
            let result_f = op(a.clone(), b.clone());
            Ok(Some(ConstValue::Float(result_f, ty)))
        }
        _ => Ok(None),
    }
}

fn fold_icmp(
    operands: &[ConstValue],
    cond: IntCC,
    ty: Type,
) -> Result<Option<ConstValue>, CompileError> {
    let (a, b) = match get_two_bigs(operands) {
        Some(v) => v,
        None => return Ok(None),
    };
    let result = match cond {
        IntCC::Equal => a == b,
        IntCC::NotEqual => a != b,
        IntCC::SignedLessThan => a < b,
        IntCC::SignedGreaterThan => a > b,
        IntCC::SignedLessThanOrEqual => a <= b,
        IntCC::SignedGreaterThanOrEqual => a >= b,
        IntCC::UnsignedLessThan => {
            let ua = as_unsigned_big(a, ty);
            let ub = as_unsigned_big(b, ty);
            ua < ub
        }
        IntCC::UnsignedGreaterThan => {
            let ua = as_unsigned_big(a, ty);
            let ub = as_unsigned_big(b, ty);
            ua > ub
        }
        IntCC::UnsignedLessThanOrEqual => {
            let ua = as_unsigned_big(a, ty);
            let ub = as_unsigned_big(b, ty);
            ua <= ub
        }
        IntCC::UnsignedGreaterThanOrEqual => {
            let ua = as_unsigned_big(a, ty);
            let ub = as_unsigned_big(b, ty);
            ua >= ub
        }
    };
    // 比较结果：true = 1, false = 0 (I32)
    Ok(Some(ConstValue::Bool(result)))
}

fn fold_fcmp(
    operands: &[ConstValue],
    cond: FloatCC,
    _ty: Type,
) -> Result<Option<ConstValue>, CompileError> {
    if operands.len() < 2 {
        return Ok(None);
    }
    match (&operands[0], &operands[1]) {
        (ConstValue::Float(a, _), ConstValue::Float(b, _)) => {
            let a_f = a.to_f64();
            let b_f = b.to_f64();

            let a_is_nan = a_f.is_nan();
            let b_is_nan = b_f.is_nan();

            let result = match cond {
                FloatCC::Ordered => !a_is_nan && !b_is_nan,
                FloatCC::Unordered => a_is_nan || b_is_nan,
                FloatCC::Equal => a_f == b_f,
                FloatCC::NotEqual => a_f != b_f,
                FloatCC::LessThan => a_f < b_f,
                FloatCC::LessThanOrEqual => a_f <= b_f,
                FloatCC::GreaterThan => a_f > b_f,
                FloatCC::GreaterThanOrEqual => a_f >= b_f,
            };
            Ok(Some(ConstValue::Bool(result)))
        }
        _ => Ok(None),
    }
}

fn fold_extend(
    operands: &[ConstValue],
    to_ty: Type,
    signed: bool,
) -> Result<Option<ConstValue>, CompileError> {
    if operands.is_empty() {
        return Ok(None);
    }
    match &operands[0] {
        ConstValue::Int(v, from_ty) => {
            let from_bits = from_ty.bits();
            if from_bits == 0 {
                return Ok(Some(ConstValue::Int(Big::S_ZERO, to_ty)));
            }
            let extended = if signed {
                v.truncate_to_bits_signed(from_bits)
            } else {
                v.truncate_to_bits(from_bits)
            };
            Ok(Some(ConstValue::Int(extended, to_ty)))
        }
        _ => Ok(None),
    }
}

fn fold_ireduce(operands: &[ConstValue], to_ty: Type) -> Result<Option<ConstValue>, CompileError> {
    if operands.is_empty() {
        return Ok(None);
    }
    match &operands[0] {
        ConstValue::Int(v, _) => Ok(Some(ConstValue::Int(truncate_to_type(v, to_ty), to_ty))),
        _ => Ok(None),
    }
}

fn fold_bitcast(operands: &[ConstValue], to_ty: Type) -> Result<Option<ConstValue>, CompileError> {
    if operands.is_empty() {
        return Ok(None);
    }
    match &operands[0] {
        ConstValue::Int(v, _) => {
            // 整数 → 浮点位模式
            if to_ty.is_float() {
                let bits = match to_ty.float_format() {
                    Some(fmt) => v.to_bits_trunc(fmt),
                    None => 0u64,
                };
                let float_big =
                    Big::from_bits(bits, to_ty.float_format().unwrap_or(FloatFormat::F64));
                Ok(Some(ConstValue::Float(float_big, to_ty)))
            } else {
                Ok(Some(ConstValue::Int(truncate_to_type(v, to_ty), to_ty)))
            }
        }
        ConstValue::Float(v, _) => {
            // 浮点 → 整数位模式
            if to_ty.is_int() {
                let fmt = FloatFormat::F64; // default
                let bits = v.to_bits_trunc(fmt);
                Ok(Some(ConstValue::Int(
                    Big::from_u64(bits).truncate_to_bits(to_ty.bits()),
                    to_ty,
                )))
            } else {
                // 浮点到浮点 (f32 <-> f64) — 保持位模式然后截断
                let target_fmt = to_ty.float_format().unwrap_or(FloatFormat::F64);
                let source_fmt = FloatFormat::F64; // default
                let bits = v.to_bits_trunc(source_fmt);
                let result = Big::from_bits(bits, target_fmt);
                Ok(Some(ConstValue::Float(result, to_ty)))
            }
        }
        ConstValue::Bool(b) => Ok(Some(ConstValue::Int(
            Big::from_i64(if *b { 1 } else { 0 }),
            to_ty,
        ))),
    }
}

// ============================================================
// Use-def 分析辅助
// ============================================================

/// 收集所有使用指定 Value 的指令的位置（(block_idx, inst_idx)）。
fn collect_uses(func: &Function) -> HashMap<Value, Vec<(usize, usize)>> {
    let mut uses: HashMap<Value, Vec<(usize, usize)>> = HashMap::new();

    for (bi, block) in func.blocks.iter().enumerate() {
        for (ii, inst) in block.instructions.iter().enumerate() {
            for operand in &inst.operands {
                uses.entry(*operand).or_default().push((bi, ii));
            }
        }
        // Terminator 中的值使用
        match &block.terminator {
            Terminator::Branch {
                cond,
                true_args,
                false_args,
                ..
            } => {
                uses.entry(*cond).or_default().push((bi, usize::MAX)); // MAX 表示 terminator
                for v in true_args.iter().chain(false_args.iter()) {
                    uses.entry(*v).or_default().push((bi, usize::MAX));
                }
            }
            Terminator::Jump { args, .. } => {
                for v in args {
                    uses.entry(*v).or_default().push((bi, usize::MAX));
                }
            }
            Terminator::Return { values } => {
                for v in values {
                    uses.entry(*v).or_default().push((bi, usize::MAX));
                }
            }
            Terminator::Unreachable => {}
            Terminator::Switch {
                discriminant,
                cases,
                ..
            } => {
                uses.entry(*discriminant)
                    .or_default()
                    .push((bi, usize::MAX));
                for (_, _, args) in cases.iter() {
                    for v in args {
                        uses.entry(*v).or_default().push((bi, usize::MAX));
                    }
                }
            }
        }
    }

    uses
}

// ============================================================
// ConstFoldPass
// ============================================================

/// 常量折叠优化 pass。
///
/// 使用 worklist-driven 算法：
/// 1. 扫描所有 Iconst/Fconst，建立已知常量映射
/// 2. 遍历所有指令，检查 operands 是否全是已知常量
/// 3. 如果可折叠，替换指令并传播到使用者
/// 4. 折叠常量条件分支
#[derive(Default)]
pub struct ConstFoldPass;

impl ConstFoldPass {
    pub fn new() -> Self {
        Self
    }
}

impl OptimizationPass for ConstFoldPass {
    fn name(&self) -> &'static str {
        "const-fold"
    }

    fn description(&self) -> &'static str {
        "Constant folding: evaluates operations with known operands at compile time"
    }

    fn run_on_function(&self, func: &mut Function) -> Result<PassResult, CompileError> {
        fold_function(func)
    }
}

/// 对单个函数执行常量折叠（公共入口，也可被 IrInterpreter 使用）。
pub fn fold_function(func: &mut Function) -> Result<PassResult, CompileError> {
    let mut result = PassResult::default();
    let mut changed = true;

    while changed {
        changed = false;

        // 1. 扫描已知常量
        let mut known: HashMap<Value, ConstValue> = HashMap::new();
        for block in func.blocks.iter() {
            for inst in &block.instructions {
                match &inst.opcode {
                    Opcode::Iconst { index } => {
                        if let Some(v) = inst.result
                            && let Some(big) = func.constant_pool.get(*index).cloned()
                        {
                            known.insert(v, ConstValue::Int(big, inst.ty));
                        }
                    }
                    Opcode::Fconst { index } => {
                        if let Some(v) = inst.result
                            && let Some(big) = func.constant_pool.get(*index).cloned()
                        {
                            known.insert(v, ConstValue::Float(big, inst.ty));
                        }
                    }
                    _ => {}
                }
            }
        }

        // 2. 收集使用关系
        let uses = collect_uses(func);

        // 3. Worklist: 所有非常量指令的结果
        let mut worklist: Vec<Value> = Vec::new();
        for block in func.blocks.iter() {
            for inst in &block.instructions {
                if let Some(v) = inst.result
                    && !known.contains_key(&v)
                {
                    worklist.push(v);
                }
            }
        }

        // 4. Worklist 循环
        let mut processed: std::collections::HashSet<Value> = std::collections::HashSet::new();

        while let Some(value) = worklist.pop() {
            if processed.contains(&value) {
                continue;
            }
            processed.insert(value);

            // 找到定义此值的指令
            let (inst_info, bi, ii) = match find_def_inst(func, value) {
                Some(v) => v,
                None => continue,
            };

            // 收集 operands 的常量值
            let const_operands: Vec<ConstValue> = inst_info
                .operands
                .iter()
                .filter_map(|v| known.get(v).cloned())
                .collect();

            // 如果不是所有 operands 都是常量，跳过
            if const_operands.len() != inst_info.operands.len() {
                continue;
            }

            // 尝试折叠
            if let Some(folded) = fold_opcode(&inst_info.opcode, &const_operands, inst_info.ty)? {
                // 替换指令为 Iconst/Fconst (插入常量池)
                let new_opcode = match &folded {
                    ConstValue::Int(v, _) => {
                        let index = func.constant_pool.insert(v.clone());
                        Opcode::Iconst { index }
                    }
                    ConstValue::Float(v, _) => {
                        let index = func.constant_pool.insert(v.clone());
                        Opcode::Fconst { index }
                    }
                    ConstValue::Bool(b) => {
                        let big = Big::from_i64(if *b { 1 } else { 0 });
                        let index = func.constant_pool.insert(big);
                        Opcode::Iconst { index }
                    }
                };

                func.blocks[bi].instructions[ii].opcode = new_opcode;
                // 更新类型：如果是 Bool 折叠，结果类型应该是 I32（比较结果类型）
                if matches!(folded, ConstValue::Bool(_)) {
                    func.blocks[bi].instructions[ii].ty = Type::I32;
                }

                known.insert(value, folded);
                changed = true;
                result.instructions_removed += 1;

                // 将所有使用者加入 worklist
                if let Some(users) = uses.get(&value) {
                    for &(_, ui) in users {
                        if ui == usize::MAX {
                            // Terminator 使用者 — 不需要加入 worklist
                            // （由 branch_fold 处理）
                            continue;
                        }
                        if let Some(user_inst) = func.blocks[bi].instructions.get(ui)
                            && let Some(uv) = user_inst.result
                            && !known.contains_key(&uv)
                        {
                            worklist.push(uv);
                        }
                    }
                }
            }
        }

        // 5. 分支折叠
        if fold_branches(func, &known) {
            changed = true;
            result.blocks_removed += 1;

            // 分支折叠后自动清除不可达块
            let dc_removed = crate::optimize::dead_code::eliminate_dead_blocks(func);
            result.blocks_removed += dc_removed;
        }
    }

    result.changed = result.instructions_removed > 0 || result.blocks_removed > 0;
    Ok(result)
}

/// 在基本块中查找定义指定 Value 的指令。
fn find_def_inst(func: &Function, value: Value) -> Option<(&Instruction, usize, usize)> {
    for (bi, block) in func.blocks.iter().enumerate() {
        for (ii, inst) in block.instructions.iter().enumerate() {
            if inst.result == Some(value) {
                return Some((inst, bi, ii));
            }
        }
    }
    None
}

/// 折叠常量条件分支。
fn fold_branches(func: &mut Function, known: &HashMap<Value, ConstValue>) -> bool {
    let mut changed = false;

    for block in func.blocks.iter_mut() {
        if let Terminator::Branch {
            cond,
            true_block,
            false_block,
            true_args,
            false_args,
        } = &block.terminator
            && let Some(const_val) = known.get(cond)
            && let Some(is_true) = const_val.to_bool()
        {
            // 替换为无条件跳转
            let (target, args) = if is_true {
                (*true_block, true_args.clone())
            } else {
                (*false_block, false_args.clone())
            };
            block.terminator = Terminator::Jump { target, args };
            changed = true;
        }
    }

    changed
}

// ============================================================
// 测试
// ============================================================

#[cfg(test)]
mod tests {
    use super::*;
    use crate::optimize::OptimizationPass;

    /// 查找指令的 opcode（按 result Value）。
    fn find_opcode(func: &Function, value: Value) -> Option<&Opcode> {
        for block in func.blocks.iter() {
            for inst in &block.instructions {
                if inst.result == Some(value) {
                    return Some(&inst.opcode);
                }
            }
        }
        None
    }

    /// 从 Iconst 指令读取 i64 值（通过常量池）。
    fn get_iconst_value(func: &Function, inst: &Instruction) -> Option<i64> {
        if let Opcode::Iconst { index } = &inst.opcode {
            func.constant_pool.get(*index).and_then(|b| b.try_to_i64())
        } else {
            None
        }
    }

    /// 按 Value 查找对应的 Iconst 指令并返回其 i64 值。
    fn get_iconst_value_for(func: &Function, value: Value) -> Option<i64> {
        for block in &func.blocks {
            for inst in &block.instructions {
                if inst.result == Some(value) {
                    return get_iconst_value(func, inst);
                }
            }
        }
        None
    }

    /// 按 Value 查找对应的 Fconst 指令并返回其 f64 值。
    fn get_fconst_value_for(func: &Function, value: Value) -> Option<f64> {
        for block in &func.blocks {
            for inst in &block.instructions {
                if inst.result == Some(value)
                    && let Opcode::Fconst { index } = &inst.opcode
                {
                    return func.constant_pool.get(*index).map(|b| b.to_f64());
                }
            }
        }
        None
    }

    // === 常量折叠测试 ===

    #[test]
    fn fold_iadd_constants() {
        let mut b = FunctionBuilder::new("test", Signature::void());
        let entry = b.create_block();
        b.switch_to_block(entry);
        let a = b.iconst_i32(3);
        let b_val = b.iconst_i32(5);
        let sum = b.iadd(a, b_val);
        b.return_(&[sum]);

        let mut func = b.finish();
        let pass = ConstFoldPass::new();
        let result = pass.run_on_function(&mut func).unwrap();

        assert!(result.changed);
        assert!(result.instructions_removed >= 1);
        // sum 应该变为 Iconst(8)
        assert_eq!(get_iconst_value_for(&func, sum), Some(8));
    }

    #[test]
    fn fold_isub_constants() {
        let mut b = FunctionBuilder::new("test", Signature::void());
        let entry = b.create_block();
        b.switch_to_block(entry);
        let a = b.iconst_i32(10);
        let b_val = b.iconst_i32(3);
        let result = b.isub(a, b_val);
        b.return_(&[result]);

        let mut func = b.finish();
        let pass = ConstFoldPass::new();
        let r = pass.run_on_function(&mut func).unwrap();
        assert!(r.changed);
        assert_eq!(get_iconst_value_for(&func, result), Some(7));
    }

    #[test]
    fn fold_imul_constants() {
        let mut b = FunctionBuilder::new("test", Signature::void());
        let entry = b.create_block();
        b.switch_to_block(entry);
        let a = b.iconst_i32(6);
        let b_val = b.iconst_i32(7);
        let result = b.imul(a, b_val);
        b.return_(&[result]);

        let mut func = b.finish();
        let pass = ConstFoldPass::new();
        let r = pass.run_on_function(&mut func).unwrap();
        assert!(r.changed);
        assert_eq!(get_iconst_value_for(&func, result), Some(42));
    }

    #[test]
    fn fold_chain() {
        // (2 + 3) * 4 = 20
        let mut b = FunctionBuilder::new("test", Signature::void());
        let entry = b.create_block();
        b.switch_to_block(entry);
        let v2 = b.iconst_i32(2);
        let v3 = b.iconst_i32(3);
        let v4 = b.iconst_i32(4);
        let sum = b.iadd(v2, v3);
        let prod = b.imul(sum, v4);
        b.return_(&[prod]);

        let mut func = b.finish();
        let pass = ConstFoldPass::new();
        let r = pass.run_on_function(&mut func).unwrap();
        assert!(r.changed);
        // 最终结果应该是 Iconst(20)
        assert_eq!(get_iconst_value_for(&func, prod), Some(20));
    }

    #[test]
    fn fold_band_constants() {
        let mut b = FunctionBuilder::new("test", Signature::void());
        let entry = b.create_block();
        b.switch_to_block(entry);
        let a = b.iconst_i32(0xFF);
        let b_val = b.iconst_i32(0x0F);
        let result = b.band(a, b_val);
        b.return_(&[result]);

        let mut func = b.finish();
        let pass = ConstFoldPass::new();
        let r = pass.run_on_function(&mut func).unwrap();
        assert!(r.changed);
        assert_eq!(get_iconst_value_for(&func, result), Some(0x0F));
    }

    #[test]
    fn fold_bor_constants() {
        let mut b = FunctionBuilder::new("test", Signature::void());
        let entry = b.create_block();
        b.switch_to_block(entry);
        let a = b.iconst_i32(0xF0);
        let b_val = b.iconst_i32(0x0F);
        let result = b.bor(a, b_val);
        b.return_(&[result]);

        let mut func = b.finish();
        let pass = ConstFoldPass::new();
        let r = pass.run_on_function(&mut func).unwrap();
        assert!(r.changed);
        assert_eq!(get_iconst_value_for(&func, result), Some(0xFF));
    }

    #[test]
    fn fold_bxor_constants() {
        let mut b = FunctionBuilder::new("test", Signature::void());
        let entry = b.create_block();
        b.switch_to_block(entry);
        let a = b.iconst_i32(0xFF);
        let b_val = b.iconst_i32(0x0F);
        let result = b.bxor(a, b_val);
        b.return_(&[result]);

        let mut func = b.finish();
        let pass = ConstFoldPass::new();
        let r = pass.run_on_function(&mut func).unwrap();
        assert!(r.changed);
        assert_eq!(get_iconst_value_for(&func, result), Some(0xF0));
    }

    #[test]
    fn fold_bnot_constant() {
        let mut b = FunctionBuilder::new("test", Signature::void());
        let entry = b.create_block();
        b.switch_to_block(entry);
        // bnot 0x0F (I32) = 0xFFFF_FFF0
        let a = b.iconst_i32(0x0F);
        let result = b.bnot(a);
        b.return_(&[result]);

        let mut func = b.finish();
        let pass = ConstFoldPass::new();
        let r = pass.run_on_function(&mut func).unwrap();
        assert!(r.changed);
        assert_eq!(get_iconst_value_for(&func, result), Some(-16)); // 0xFFFFFFF0 as signed i32
    }

    #[test]
    fn fold_ishl_constant() {
        let mut b = FunctionBuilder::new("test", Signature::void());
        let entry = b.create_block();
        b.switch_to_block(entry);
        let a = b.iconst_i32(1);
        let shift = b.iconst_i32(3);
        let result = b.ishl(a, shift);
        b.return_(&[result]);

        let mut func = b.finish();
        let pass = ConstFoldPass::new();
        let r = pass.run_on_function(&mut func).unwrap();
        assert!(r.changed);
        assert_eq!(get_iconst_value_for(&func, result), Some(8));
    }

    #[test]
    fn fold_ushr_constant() {
        let mut b = FunctionBuilder::new("test", Signature::void());
        let entry = b.create_block();
        b.switch_to_block(entry);
        let a = b.iconst_i32(-8i32); // 0xFFFFFFF8
        let shift = b.iconst_i32(2);
        let result = b.ushr(a, shift);
        b.return_(&[result]);

        let mut func = b.finish();
        let pass = ConstFoldPass::new();
        pass.run_on_function(&mut func).unwrap();
        // 逻辑右移后，高位填零
        let val = get_iconst_value_for(&func, result).unwrap();
        // 0xFFFFFFF8 >> 2 (logical) = 0x3FFFFFFE
        assert_eq!(val as u32, 0x3FFFFFFE_u32);
    }

    #[test]
    fn fold_sshr_constant() {
        let mut b = FunctionBuilder::new("test", Signature::void());
        let entry = b.create_block();
        b.switch_to_block(entry);
        let a = b.iconst_i32(-8i32); // 0xFFFFFFF8
        let shift = b.iconst_i32(2);
        let result = b.sshr(a, shift);
        b.return_(&[result]);

        let mut func = b.finish();
        let pass = ConstFoldPass::new();
        pass.run_on_function(&mut func).unwrap();
        // 算术右移后，符号扩展
        assert_eq!(get_iconst_value_for(&func, result), Some(-2)); // -8 >> 2 = -2
    }

    #[test]
    fn fold_icmp_equal() {
        let mut b = FunctionBuilder::new("test", Signature::void());
        let entry = b.create_block();
        b.switch_to_block(entry);
        let a = b.iconst_i32(5);
        let b_val = b.iconst_i32(5);
        let result = b.icmp(IntCC::Equal, a, b_val);
        b.return_(&[result]);

        let mut func = b.finish();
        let pass = ConstFoldPass::new();
        let r = pass.run_on_function(&mut func).unwrap();
        assert!(r.changed);
        assert_eq!(get_iconst_value_for(&func, result), Some(1)); // true = 1
    }

    #[test]
    fn fold_icmp_not_equal() {
        let mut b = FunctionBuilder::new("test", Signature::void());
        let entry = b.create_block();
        b.switch_to_block(entry);
        let a = b.iconst_i32(5);
        let b_val = b.iconst_i32(3);
        let result = b.icmp(IntCC::NotEqual, a, b_val);
        b.return_(&[result]);

        let mut func = b.finish();
        let pass = ConstFoldPass::new();
        pass.run_on_function(&mut func).unwrap();
        assert_eq!(get_iconst_value_for(&func, result), Some(1));
    }

    #[test]
    fn fold_icmp_signed_less_than() {
        let mut b = FunctionBuilder::new("test", Signature::void());
        let entry = b.create_block();
        b.switch_to_block(entry);
        let a = b.iconst_i32(-5);
        let b_val = b.iconst_i32(3);
        let result = b.icmp(IntCC::SignedLessThan, a, b_val);
        b.return_(&[result]);

        let mut func = b.finish();
        let pass = ConstFoldPass::new();
        pass.run_on_function(&mut func).unwrap();
        assert_eq!(get_iconst_value_for(&func, result), Some(1));
    }

    #[test]
    fn fold_icmp_unsigned_greater_than() {
        let mut b = FunctionBuilder::new("test", Signature::void());
        let entry = b.create_block();
        b.switch_to_block(entry);
        // -1 as unsigned = 0xFFFFFFFF, which is > 5
        let a = b.iconst_i32(-1i32);
        let b_val = b.iconst_i32(5);
        let result = b.icmp(IntCC::UnsignedGreaterThan, a, b_val);
        b.return_(&[result]);

        let mut func = b.finish();
        let pass = ConstFoldPass::new();
        pass.run_on_function(&mut func).unwrap();
        assert_eq!(get_iconst_value_for(&func, result), Some(1));
    }

    #[test]
    fn fold_select_constant_cond() {
        let mut b = FunctionBuilder::new("test", Signature::void());
        let entry = b.create_block();
        b.switch_to_block(entry);
        let cond = b.iconst_i32(1); // non-zero = true
        let a = b.iconst_i32(10);
        let b_val = b.iconst_i32(20);
        let result = b.select(cond, a, b_val);
        b.return_(&[result]);

        let mut func = b.finish();
        let pass = ConstFoldPass::new();
        let r = pass.run_on_function(&mut func).unwrap();
        assert!(r.changed);
        assert_eq!(get_iconst_value_for(&func, result), Some(10));
    }

    #[test]
    fn no_fold_mixed_constants() {
        // 常量 + 运行时值 → 不应该折叠
        let sig = Signature::new(&[(Type::I32, "x")], &[Type::I32]);
        let mut b = FunctionBuilder::new("test", sig);
        let (entry, params) = b.create_block_with_params(&[(Type::I32, "x")]);
        b.switch_to_block(entry);
        let x = params[0];
        let c = b.iconst_i32(5);
        let sum = b.iadd(x, c);
        b.return_(&[sum]);

        let mut func = b.finish();
        let pass = ConstFoldPass::new();
        let r = pass.run_on_function(&mut func).unwrap();
        // 不应该折叠（x 不是常量）
        assert!(!r.changed);
        assert!(matches!(find_opcode(&func, sum), Some(Opcode::Iadd)));
    }

    #[test]
    fn fold_branch_true_condition() {
        let mut b = FunctionBuilder::new("test", Signature::void());
        let entry = b.create_block();
        let then_block = b.create_block();
        let else_block = b.create_block();
        let merge = b.create_block();

        b.switch_to_block(entry);
        let one = b.iconst_i32(1);
        let zero = b.iconst_i32(0);
        let cmp = b.icmp(IntCC::Equal, one, one); // always true
        b.branch(cmp, then_block, else_block, &[], &[]);

        b.switch_to_block(then_block);
        b.jump(merge, &[one]);

        b.switch_to_block(else_block);
        b.jump(merge, &[zero]);

        b.switch_to_block(merge);
        b.return_(&[]);

        let mut func = b.finish();
        let pass = ConstFoldPass::new();
        let r = pass.run_on_function(&mut func).unwrap();
        assert!(r.changed);
        // entry block 的 terminator 应该变为 Jump
        assert!(matches!(func.blocks[0].terminator, Terminator::Jump { .. }));
    }

    #[test]
    fn fold_extend_and_reduce() {
        // sextend i8(-1) → i32 → ireduce → i8 = -1
        let mut b = FunctionBuilder::new("test", Signature::void());
        let entry = b.create_block();
        b.switch_to_block(entry);
        let v = b.iconst_i8(-1i8);
        let ext = b.sextend(v, Type::I32);
        let red = b.ireduce(ext, Type::I8);
        b.return_(&[red]);

        let mut func = b.finish();
        let pass = ConstFoldPass::new();
        let r = pass.run_on_function(&mut func).unwrap();
        assert!(r.changed);
        // 结果应为 Iconst(-1, I8) → truncated = -1 as i8 as i64 = -1
        assert_eq!(get_iconst_value_for(&func, red), Some(-1));
    }

    #[test]
    fn fold_copy_propagates_constant() {
        let mut b = FunctionBuilder::new("test", Signature::void());
        let entry = b.create_block();
        b.switch_to_block(entry);
        let c = b.iconst_i32(42);
        let copied = b.copy(c);
        b.return_(&[copied]);

        let mut func = b.finish();
        let pass = ConstFoldPass::new();
        let r = pass.run_on_function(&mut func).unwrap();
        assert!(r.changed);
        assert_eq!(get_iconst_value_for(&func, copied), Some(42));
    }

    #[test]
    fn fold_constant_division_by_zero_error() {
        let mut b = FunctionBuilder::new("test", Signature::void());
        let entry = b.create_block();
        b.switch_to_block(entry);
        let a = b.iconst_i32(10);
        let zero = b.iconst_i32(0);
        let result = b.sdiv(a, zero);
        b.return_(&[result]);

        let mut func = b.finish();
        let pass = ConstFoldPass::new();
        let r = pass.run_on_function(&mut func);
        assert!(r.is_err());
    }

    #[test]
    fn fold_div_constants() {
        let mut b = FunctionBuilder::new("test", Signature::void());
        let entry = b.create_block();
        b.switch_to_block(entry);
        let a = b.iconst_i32(20);
        let b_val = b.iconst_i32(4);
        let result = b.sdiv(a, b_val);
        b.return_(&[result]);

        let mut func = b.finish();
        let pass = ConstFoldPass::new();
        let r = pass.run_on_function(&mut func).unwrap();
        assert!(r.changed);
        assert_eq!(get_iconst_value_for(&func, result), Some(5));
    }

    #[test]
    fn fold_rem_constants() {
        let mut b = FunctionBuilder::new("test", Signature::void());
        let entry = b.create_block();
        b.switch_to_block(entry);
        let a = b.iconst_i32(20);
        let b_val = b.iconst_i32(7);
        let result = b.srem(a, b_val);
        b.return_(&[result]);

        let mut func = b.finish();
        let pass = ConstFoldPass::new();
        let r = pass.run_on_function(&mut func).unwrap();
        assert!(r.changed);
        assert_eq!(get_iconst_value_for(&func, result), Some(6));
    }

    #[test]
    fn fold_float_add() {
        let mut b = FunctionBuilder::new("test", Signature::void());
        let entry = b.create_block();
        b.switch_to_block(entry);
        let a = b.fconst_f64(1.5_f64);
        let b_val = b.fconst_f64(2.5_f64);
        let result = b.fadd(a, b_val);
        b.return_(&[result]);

        let mut func = b.finish();
        let pass = ConstFoldPass::new();
        let r = pass.run_on_function(&mut func).unwrap();
        assert!(r.changed);
        if let Some(f) = get_fconst_value_for(&func, result) {
            assert!((f - 4.0).abs() < 0.001);
        } else {
            panic!("expected Fconst");
        }
    }

    #[test]
    fn fold_float_neg() {
        let mut b = FunctionBuilder::new("test", Signature::void());
        let entry = b.create_block();
        b.switch_to_block(entry);
        let a = b.fconst_f64(3.0_f64);
        let result = b.fneg(a);
        b.return_(&[result]);

        let mut func = b.finish();
        let pass = ConstFoldPass::new();
        let r = pass.run_on_function(&mut func).unwrap();
        assert!(r.changed);
        if let Some(f) = get_fconst_value_for(&func, result) {
            assert!((f - (-3.0)).abs() < 0.001);
        } else {
            panic!("expected Fconst");
        }
    }

    #[test]
    fn pass_manager_with_const_fold() {
        // 集成测试：通过 PassManager 运行 ConstFoldPass
        let mut b = FunctionBuilder::new("test", Signature::void());
        let entry = b.create_block();
        b.switch_to_block(entry);
        let a = b.iconst_i32(100);
        let b_val = b.iconst_i32(200);
        let sum = b.iadd(a, b_val);
        b.return_(&[sum]);

        let mut func = b.finish();

        let mut pm = crate::optimize::PassManager::new();
        pm.add_pass(ConstFoldPass::new(), crate::optimize::PassRunMode::Once);
        let result = pm.run_on_function(&mut func).unwrap();

        assert!(result.changed);
        assert_eq!(result.instructions_removed, 1);
        assert_eq!(get_iconst_value_for(&func, sum), Some(300));
    }
}
