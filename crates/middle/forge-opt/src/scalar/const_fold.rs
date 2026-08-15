// 临时注释修正占位
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

use crate::{ConstValue, OptimizationPass, PassResult};
use forge_ir::IrError;
use forge_ir::*;
use forge_ir::{Big, FloatFormat};
use std::collections::HashMap;

// ============================================================
// TypeId 辅助函数 (forge-ir v2: TypeId 无 is_int/is_float 方法)
// ============================================================

/// 检查 TypeId 是否为整数类型（预设索引 2-5, 10）。
fn ty_is_int(ty: TypeId) -> bool {
    matches!(ty.0, 2 | 3 | 4 | 5 | 10)
}

/// 检查 TypeId 是否为浮点类型（预设索引 6-7, 11-12）。
fn ty_is_float(ty: TypeId) -> bool {
    matches!(ty.0, 6 | 7 | 11 | 12)
}

/// 检查 TypeId 是否为指针类型（预设索引 8）。
fn ty_is_ptr(ty: TypeId) -> bool {
    ty.0 == 8
}

/// 获取浮点格式。仅对预设浮点类型有效。
fn ty_float_format(ty: TypeId) -> Option<FloatFormat> {
    match ty.0 {
        6 => Some(FloatFormat::F32),   // F32
        7 => Some(FloatFormat::F64),   // F64
        11 => Some(FloatFormat::F16),  // F16
        12 => Some(FloatFormat::F128), // F128
        _ => None,
    }
}

// ============================================================
// 整数宽度辅助函数（基于 Big）
// ============================================================

/// 按类型宽度截断 Big 值（有符号语义）。
fn truncate_to_type(value: &Big, ty: TypeId) -> Big {
    let bits = ty.bits();
    if bits == 0 || (!ty_is_int(ty) && !ty_is_ptr(ty)) {
        return value.clone();
    }
    value.truncate_to_bits_signed(bits)
}

/// 将 Big 解释为固定位宽的无符号值。
fn as_unsigned_big(value: &Big, ty: TypeId) -> Big {
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
/// 折叠 `extractvalue` 聚合字面量（3.1：immediates = [idx, Agg(id), Type]）。
/// 标量提取 → 标量 ConstValue；嵌套聚合结果不折叠（值语义）。
fn fold_extract_value(
    pool: &forge_ir::constant::ConstantPool,
    immediates: &[Immediate],
    result_ty: TypeId,
) -> Result<Option<ConstValue>, IrError> {
    use forge_ir::constant::AggChild;
    let idx = match immediates.first() {
        Some(Immediate::Uint(i)) => *i as usize,
        _ => return Ok(None),
    };
    let agg = match immediates.get(1) {
        Some(Immediate::Agg(id)) => *id,
        _ => return Ok(None),
    };
    let agg_c = match pool.get_aggregate(agg) {
        Some(a) => a,
        None => return Ok(None),
    };
    let child = match agg_c.children.get(idx) {
        Some(c) => c,
        None => return Ok(None),
    };
    Ok(match child {
        AggChild::Scalar(cid) => {
            if let Some((v, _bits)) = pool.get_int(*cid) {
                Some(ConstValue::Int(Big::from_i128(v), result_ty))
            } else {
                pool.get_float128(*cid).map(|bits| {
                    ConstValue::Float(
                        Big::from_f64(f64::from_bits(bits as u64)).unwrap_or(Big::F_ZERO),
                        result_ty,
                    )
                })
            }
        }
        // 嵌套聚合结果：不做标量折叠（值语义；codegen 仅折叠时读取）
        AggChild::Agg(_) => None,
    })
}

/// 返回 `None` 如果操作数不足或包含未知值。
pub fn fold_opcode(
    opcode: &Opcode,
    operands: &[ConstValue],
    ty: TypeId,
) -> Result<Option<ConstValue>, IrError> {
    match opcode {
        // === 整数算术 ===
        Opcode::Iadd => fold_binary_int(operands, ty, |a, b| a + b, |a, b| a.checked_add(b)),
        Opcode::Isub => fold_binary_int(operands, ty, |a, b| a - b, |a, b| a.checked_sub(b)),
        Opcode::Imul => fold_binary_int(operands, ty, |a, b| a * b, |a, b| a.checked_mul(b)),
        Opcode::Udiv => {
            let (a, b) = match get_two_bigs(operands) {
                Some(v) => v,
                None => return Ok(None),
            };
            match a.udiv(b) {
                Ok(result) => Ok(Some(ConstValue::Int(truncate_to_type(&result, ty), ty))),
                Err(_) => Err(IrError::Internal("compile-time division by zero".into())),
            }
        }
        Opcode::Sdiv => {
            let (a, b) = match get_two_bigs(operands) {
                Some(v) => v,
                None => return Ok(None),
            };
            match a.sdiv(b) {
                Ok(result) => Ok(Some(ConstValue::Int(truncate_to_type(&result, ty), ty))),
                Err(_) => Err(IrError::Internal("compile-time division by zero".into())),
            }
        }
        Opcode::Urem => {
            let (a, b) = match get_two_bigs(operands) {
                Some(v) => v,
                None => return Ok(None),
            };
            match a.urem(b) {
                Ok(result) => Ok(Some(ConstValue::Int(truncate_to_type(&result, ty), ty))),
                Err(_) => Err(IrError::Internal("compile-time division by zero".into())),
            }
        }
        Opcode::Srem => {
            let (a, b) = match get_two_bigs(operands) {
                Some(v) => v,
                None => return Ok(None),
            };
            match a.srem(b) {
                Ok(result) => Ok(Some(ConstValue::Int(truncate_to_type(&result, ty), ty))),
                Err(_) => Err(IrError::Internal("compile-time division by zero".into())),
            }
        }

        // === 位运算 ===
        Opcode::Band => fold_binary_int(operands, ty, |a, b| a & b, |a, b| Some(a & b)),
        Opcode::Bor => fold_binary_int(operands, ty, |a, b| a | b, |a, b| Some(a | b)),
        Opcode::Bxor => fold_binary_int(operands, ty, |a, b| a ^ b, |a, b| Some(a ^ b)),
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
        Opcode::Fadd => fold_binary_float(operands, ty, |a, b| a + b),
        Opcode::Fsub => fold_binary_float(operands, ty, |a, b| a - b),
        Opcode::Fmul => fold_binary_float(operands, ty, |a, b| a * b),
        Opcode::Fdiv => fold_binary_float(operands, ty, |a, b| a / b),
        Opcode::Freeze => {
            // Freeze 阻止常量折叠 — 值不能跨 Freeze 被推断
            Ok(None)
        }
        Opcode::ShuffleVector
        | Opcode::AtomicRmw
        | Opcode::Cmpxchg
        | Opcode::Fence
        | Opcode::ExtractValue
        | Opcode::InsertValue => Ok(None),
        Opcode::Fneg => {
            if operands.is_empty() {
                return Ok(None);
            }
            match &operands[0] {
                ConstValue::Float(v, t) => Ok(Some(ConstValue::Float(-v.clone(), *t))),
                _ => Ok(None),
            }
        }
        Opcode::Fabs => {
            if operands.is_empty() {
                return Ok(None);
            }
            match &operands[0] {
                ConstValue::Float(v, t) => Ok(Some(ConstValue::Float(v.clone().abs(), *t))),
                _ => Ok(None),
            }
        }
        Opcode::Fsqrt => {
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
        // 新增 LLVM 转换：暂不常量折叠（安全跳过，后续按需补语义）
        Opcode::Fptrunc
        | Opcode::Fpext
        | Opcode::Fptosi
        | Opcode::Sitofp
        | Opcode::Fptoui
        | Opcode::Uitofp
        | Opcode::Ptrtoint
        | Opcode::Inttoptr => Ok(None),

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

        // === Copy ===
        Opcode::Copy => {
            if operands.is_empty() {
                return Ok(None);
            }
            Ok(Some(operands[0].clone()))
        }

        // === 不可折叠的操作码 ===
        Opcode::Load
        | Opcode::Fload
        | Opcode::Store
        | Opcode::Fstore
        | Opcode::StackAddr
        | Opcode::GlobalAddr
        | Opcode::Call
        | Opcode::CallIndirect
        | Opcode::Vadd
        | Opcode::Vsub
        | Opcode::Vmul
        | Opcode::Vextract
        | Opcode::Vinsert
        | Opcode::Vsplit
        | Opcode::Vconcat
        | Opcode::Nop
        | Opcode::Alloca
        | Opcode::GetElementPtr
        | Opcode::AddrSpaceCast
        | Opcode::VaArg => Ok(None),

        // Iconst/Fconst/Vconst 在此处不应出现（已在 scan 阶段处理）
        Opcode::Iconst | Opcode::Fconst | Opcode::Vconst => Ok(None),

        // === 位操作 (6) ===
        Opcode::Clz => {
            let a = match get_one_big(operands) {
                Some(v) => v,
                None => return Ok(None),
            };
            let u = a.trunc_to_u64();
            let result = u.leading_zeros() as u64;
            Ok(Some(ConstValue::Int(
                Big::from_u64(result).truncate_to_bits(ty.bits()),
                ty,
            )))
        }
        Opcode::Ctz => {
            let a = match get_one_big(operands) {
                Some(v) => v,
                None => return Ok(None),
            };
            let u = a.trunc_to_u64();
            let result = u.trailing_zeros() as u64;
            Ok(Some(ConstValue::Int(
                Big::from_u64(result).truncate_to_bits(ty.bits()),
                ty,
            )))
        }
        Opcode::Popcnt => {
            let a = match get_one_big(operands) {
                Some(v) => v,
                None => return Ok(None),
            };
            let u = a.trunc_to_u64();
            let result = u.count_ones() as u64;
            Ok(Some(ConstValue::Int(
                Big::from_u64(result).truncate_to_bits(ty.bits()),
                ty,
            )))
        }
        Opcode::Bitreverse => {
            let a = match get_one_big(operands) {
                Some(v) => v,
                None => return Ok(None),
            };
            let u = a.trunc_to_u64();
            let result = u.reverse_bits();
            Ok(Some(ConstValue::Int(
                Big::from_u64(result).truncate_to_bits(ty.bits()),
                ty,
            )))
        }
        Opcode::Rotl => {
            let (a, b) = match get_two_bigs(operands) {
                Some(v) => v,
                None => return Ok(None),
            };
            let val = a.trunc_to_u64();
            let shift = (big_to_usize(b) as u32) % (ty.bits().max(1));
            let result = val.rotate_left(shift);
            Ok(Some(ConstValue::Int(
                Big::from_u64(result).truncate_to_bits(ty.bits()),
                ty,
            )))
        }
        Opcode::Rotr => {
            let (a, b) = match get_two_bigs(operands) {
                Some(v) => v,
                None => return Ok(None),
            };
            let val = a.trunc_to_u64();
            let shift = (big_to_usize(b) as u32) % (ty.bits().max(1));
            let result = val.rotate_right(shift);
            Ok(Some(ConstValue::Int(
                Big::from_u64(result).truncate_to_bits(ty.bits()),
                ty,
            )))
        }

        // === 整数扩展 (10) ===
        Opcode::Abs => {
            let a = match get_one_big(operands) {
                Some(v) => v,
                None => return Ok(None),
            };
            Ok(Some(ConstValue::Int(truncate_to_type(&a.abs(), ty), ty)))
        }
        Opcode::Smin => {
            let (a, b) = match get_two_bigs(operands) {
                Some(v) => v,
                None => return Ok(None),
            };
            let result = if *a <= *b { a.clone() } else { b.clone() };
            Ok(Some(ConstValue::Int(truncate_to_type(&result, ty), ty)))
        }
        Opcode::Smax => {
            let (a, b) = match get_two_bigs(operands) {
                Some(v) => v,
                None => return Ok(None),
            };
            let result = if *a >= *b { a.clone() } else { b.clone() };
            Ok(Some(ConstValue::Int(truncate_to_type(&result, ty), ty)))
        }
        Opcode::Umin => {
            let (a, b) = match get_two_bigs(operands) {
                Some(v) => v,
                None => return Ok(None),
            };
            let ua = as_unsigned_big(a, ty);
            let ub = as_unsigned_big(b, ty);
            let result = if ua <= ub { ua } else { ub };
            Ok(Some(ConstValue::Int(truncate_to_type(&result, ty), ty)))
        }
        Opcode::Umax => {
            let (a, b) = match get_two_bigs(operands) {
                Some(v) => v,
                None => return Ok(None),
            };
            let ua = as_unsigned_big(a, ty);
            let ub = as_unsigned_big(b, ty);
            let result = if ua >= ub { ua } else { ub };
            Ok(Some(ConstValue::Int(truncate_to_type(&result, ty), ty)))
        }
        Opcode::SaddSat => {
            let (a, b) = match get_two_bigs(operands) {
                Some(v) => v,
                None => return Ok(None),
            };
            let bits = ty.bits();
            if bits == 0 {
                return Ok(Some(ConstValue::Int(Big::S_ZERO, ty)));
            }
            let a_i64 = a.try_to_i64();
            let b_i64 = b.try_to_i64();
            if let (Some(av), Some(bv)) = (a_i64, b_i64) {
                let result = av.saturating_add(bv);
                Ok(Some(ConstValue::Int(
                    Big::from_i64(result).truncate_to_bits_signed(bits),
                    ty,
                )))
            } else {
                Ok(None)
            }
        }
        Opcode::SsubSat => {
            let (a, b) = match get_two_bigs(operands) {
                Some(v) => v,
                None => return Ok(None),
            };
            let bits = ty.bits();
            if bits == 0 {
                return Ok(Some(ConstValue::Int(Big::S_ZERO, ty)));
            }
            let a_i64 = a.try_to_i64();
            let b_i64 = b.try_to_i64();
            if let (Some(av), Some(bv)) = (a_i64, b_i64) {
                let result = av.saturating_sub(bv);
                Ok(Some(ConstValue::Int(
                    Big::from_i64(result).truncate_to_bits_signed(bits),
                    ty,
                )))
            } else {
                Ok(None)
            }
        }
        Opcode::UaddSat => {
            let (a, b) = match get_two_bigs(operands) {
                Some(v) => v,
                None => return Ok(None),
            };
            let bits = ty.bits();
            if bits == 0 {
                return Ok(Some(ConstValue::Int(Big::U_ZERO, ty)));
            }
            let a_u64 = a.trunc_to_u64();
            let b_u64 = b.trunc_to_u64();
            let result = a_u64.saturating_add(b_u64);
            Ok(Some(ConstValue::Int(
                Big::from_u64(result).truncate_to_bits(bits),
                ty,
            )))
        }
        Opcode::UsubSat => {
            let (a, b) = match get_two_bigs(operands) {
                Some(v) => v,
                None => return Ok(None),
            };
            let bits = ty.bits();
            if bits == 0 {
                return Ok(Some(ConstValue::Int(Big::U_ZERO, ty)));
            }
            let a_u64 = a.trunc_to_u64();
            let b_u64 = b.trunc_to_u64();
            let result = a_u64.saturating_sub(b_u64);
            Ok(Some(ConstValue::Int(
                Big::from_u64(result).truncate_to_bits(bits),
                ty,
            )))
        }
        Opcode::Bswap => {
            let a = match get_one_big(operands) {
                Some(v) => v,
                None => return Ok(None),
            };
            let bits = ty.bits();
            if bits == 0 {
                return Ok(Some(ConstValue::Int(Big::S_ZERO, ty)));
            }
            let u = a.trunc_to_u64();
            let result = u.swap_bytes();
            Ok(Some(ConstValue::Int(
                Big::from_u64(result).truncate_to_bits(bits),
                ty,
            )))
        }

        // === 浮点扩展 (8) ===
        Opcode::Fma => {
            if operands.len() < 3 {
                return Ok(None);
            }
            match (&operands[0], &operands[1], &operands[2]) {
                (ConstValue::Float(a, _), ConstValue::Float(b, _), ConstValue::Float(c, _)) => {
                    let a_f = a.to_f64();
                    let b_f = b.to_f64();
                    let c_f = c.to_f64();
                    let result = a_f.mul_add(b_f, c_f);
                    let result_big = Big::from_f64(result).unwrap_or(Big::F_ZERO);
                    Ok(Some(ConstValue::Float(result_big, ty)))
                }
                _ => Ok(None),
            }
        }
        Opcode::Fmin => {
            if operands.len() < 2 {
                return Ok(None);
            }
            match (&operands[0], &operands[1]) {
                (ConstValue::Float(a, _), ConstValue::Float(b, _)) => {
                    let result = a.to_f64().min(b.to_f64());
                    let result_big = Big::from_f64(result).unwrap_or(Big::F_ZERO);
                    Ok(Some(ConstValue::Float(result_big, ty)))
                }
                _ => Ok(None),
            }
        }
        Opcode::Fmax => {
            if operands.len() < 2 {
                return Ok(None);
            }
            match (&operands[0], &operands[1]) {
                (ConstValue::Float(a, _), ConstValue::Float(b, _)) => {
                    let result = a.to_f64().max(b.to_f64());
                    let result_big = Big::from_f64(result).unwrap_or(Big::F_ZERO);
                    Ok(Some(ConstValue::Float(result_big, ty)))
                }
                _ => Ok(None),
            }
        }
        Opcode::Fcopysign => {
            if operands.len() < 2 {
                return Ok(None);
            }
            match (&operands[0], &operands[1]) {
                (ConstValue::Float(a, _), ConstValue::Float(b, _)) => {
                    let result = a.to_f64().copysign(b.to_f64());
                    let result_big = Big::from_f64(result).unwrap_or(Big::F_ZERO);
                    Ok(Some(ConstValue::Float(result_big, ty)))
                }
                _ => Ok(None),
            }
        }
        Opcode::Ffloor => {
            if operands.is_empty() {
                return Ok(None);
            }
            match &operands[0] {
                ConstValue::Float(v, _) => {
                    let result = v.to_f64().floor();
                    let result_big = Big::from_f64(result).unwrap_or(Big::F_ZERO);
                    Ok(Some(ConstValue::Float(result_big, ty)))
                }
                _ => Ok(None),
            }
        }
        Opcode::Fceil => {
            if operands.is_empty() {
                return Ok(None);
            }
            match &operands[0] {
                ConstValue::Float(v, _) => {
                    let result = v.to_f64().ceil();
                    let result_big = Big::from_f64(result).unwrap_or(Big::F_ZERO);
                    Ok(Some(ConstValue::Float(result_big, ty)))
                }
                _ => Ok(None),
            }
        }
        Opcode::Ftrunc => {
            if operands.is_empty() {
                return Ok(None);
            }
            match &operands[0] {
                ConstValue::Float(v, _) => {
                    let result = v.to_f64().trunc();
                    let result_big = Big::from_f64(result).unwrap_or(Big::F_ZERO);
                    Ok(Some(ConstValue::Float(result_big, ty)))
                }
                _ => Ok(None),
            }
        }
        Opcode::Fround => {
            if operands.is_empty() {
                return Ok(None);
            }
            match &operands[0] {
                ConstValue::Float(v, _) => {
                    let result = v.to_f64().round();
                    let result_big = Big::from_f64(result).unwrap_or(Big::F_ZERO);
                    Ok(Some(ConstValue::Float(result_big, ty)))
                }
                _ => Ok(None),
            }
        }

        // === 溢出算术 (6) — 跳过: 双结果指令不适合当前单结果折叠框架 ===
        Opcode::SaddOverflow
        | Opcode::UaddOverflow
        | Opcode::SsubOverflow
        | Opcode::UsubOverflow
        | Opcode::SmulOverflow
        | Opcode::UmulOverflow => Ok(None),

        // === 值语义 (2) — 不可折叠 ===
        Opcode::Poison | Opcode::Undef => Ok(None),

        // === 指针谓词 (2) ===
        Opcode::IsNull => {
            let a = match get_one_big(operands) {
                Some(v) => v,
                None => return Ok(None),
            };
            Ok(Some(ConstValue::Bool(a.is_zero())))
        }
        Opcode::IsNotNull => {
            let a = match get_one_big(operands) {
                Some(v) => v,
                None => return Ok(None),
            };
            Ok(Some(ConstValue::Bool(!a.is_zero())))
        }

        // === SIMD (5) — 跳过: 无向量常量支持 ===
        Opcode::Vdiv | Opcode::Vneg | Opcode::Vabs | Opcode::Vbitcast | Opcode::Vbroadcast => {
            Ok(None)
        }

        // === Trap (1) — 副作用, 不可折叠 ===
        Opcode::Trap => Ok(None),
        // === 异常（P1.1）— 副作用, 不可折叠 ===
        Opcode::LandingPad => Ok(None),
    }
}

// ============================================================
// 辅助：从 operands 中提取整数/浮点
// ============================================================

fn get_one_big(operands: &[ConstValue]) -> Option<&Big> {
    if operands.is_empty() {
        return None;
    }
    match &operands[0] {
        ConstValue::Int(v, _) => Some(v),
        _ => None,
    }
}

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
    ty: TypeId,
    op: fn(Big, Big) -> Big,
    op_small: fn(i64, i64) -> Option<i64>,
) -> Result<Option<ConstValue>, IrError> {
    let (a, b) = match get_two_bigs(operands) {
        Some(v) => v,
        None => return Ok(None),
    };
    // 小整数快路径：两个操作数都能放进 i64 且结果不溢出时用原生 i64 运算
    // （避免 dashu Big 的堆分配 + clone——const_fold/SCCP 整数常量折叠热点）。
    // checked 运算返回 None（溢出）时回退任意精度 Big。
    if let (Some(ai), Some(bi)) = (a.try_to_i64(), b.try_to_i64())
        && let Some(r) = op_small(ai, bi)
    {
        let rbig = Big::from_i64(r);
        return Ok(Some(ConstValue::Int(truncate_to_type(&rbig, ty), ty)));
    }
    let result = op(a.clone(), b.clone());
    Ok(Some(ConstValue::Int(truncate_to_type(&result, ty), ty)))
}

fn fold_binary_float(
    operands: &[ConstValue],
    ty: TypeId,
    op: fn(Big, Big) -> Big,
) -> Result<Option<ConstValue>, IrError> {
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
    ty: TypeId,
) -> Result<Option<ConstValue>, IrError> {
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
    _ty: TypeId,
) -> Result<Option<ConstValue>, IrError> {
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
    to_ty: TypeId,
    signed: bool,
) -> Result<Option<ConstValue>, IrError> {
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

fn fold_ireduce(operands: &[ConstValue], to_ty: TypeId) -> Result<Option<ConstValue>, IrError> {
    if operands.is_empty() {
        return Ok(None);
    }
    match &operands[0] {
        ConstValue::Int(v, _) => Ok(Some(ConstValue::Int(truncate_to_type(v, to_ty), to_ty))),
        _ => Ok(None),
    }
}

fn fold_bitcast(operands: &[ConstValue], to_ty: TypeId) -> Result<Option<ConstValue>, IrError> {
    if operands.is_empty() {
        return Ok(None);
    }
    match &operands[0] {
        ConstValue::Int(v, _) => {
            // 整数 → 浮点位模式
            if ty_is_float(to_ty) {
                let bits = match ty_float_format(to_ty) {
                    Some(fmt) => v.to_bits_trunc(fmt),
                    None => 0u64,
                };
                let float_big =
                    Big::from_bits(bits, ty_float_format(to_ty).unwrap_or(FloatFormat::F64));
                Ok(Some(ConstValue::Float(float_big, to_ty)))
            } else {
                Ok(Some(ConstValue::Int(truncate_to_type(v, to_ty), to_ty)))
            }
        }
        ConstValue::Float(v, _) => {
            // 浮点 → 整数位模式
            if ty_is_int(to_ty) {
                let fmt = FloatFormat::F64; // default
                let bits = v.to_bits_trunc(fmt);
                Ok(Some(ConstValue::Int(
                    Big::from_u64(bits).truncate_to_bits(to_ty.bits()),
                    to_ty,
                )))
            } else {
                // 浮点到浮点 (f32 <-> f64) — 保持位模式然后截断
                let target_fmt = ty_float_format(to_ty).unwrap_or(FloatFormat::F64);
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

    for (bi, block) in func.dfg.blocks.iter().enumerate() {
        for &inst_id in &block.inst_order {
            let inst = &func.dfg.insts[inst_id.0 as usize];
            for operand in &inst.operands {
                uses.entry(*operand)
                    .or_default()
                    .push((bi, inst_id.0 as usize));
            }
        }
        // Terminator 中的值使用
        match &block.terminator {
            Terminator::Branch {
                cond,
                then_args,
                else_args,
                ..
            } => {
                uses.entry(*cond).or_default().push((bi, usize::MAX)); // MAX 表示 terminator
                for v in then_args.iter().chain(else_args.iter()) {
                    uses.entry(*v).or_default().push((bi, usize::MAX));
                }
            }
            Terminator::Jump { args, .. } => {
                for v in args {
                    uses.entry(*v).or_default().push((bi, usize::MAX));
                }
            }
            Terminator::Return { values, .. } => {
                for v in values {
                    uses.entry(*v).or_default().push((bi, usize::MAX));
                }
            }
            Terminator::Unreachable => {}
            Terminator::Invoke {
                args,
                normal_args,
                unwind_args,
                ..
            } => {
                for v in args
                    .iter()
                    .chain(normal_args.iter())
                    .chain(unwind_args.iter())
                {
                    uses.entry(*v).or_default().push((bi, usize::MAX));
                }
            }
            Terminator::Resume { value, .. } => {
                uses.entry(*value).or_default().push((bi, usize::MAX));
            }
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

    fn run_on_function(&self, func: &mut Function) -> Result<PassResult, IrError> {
        fold_function(func)
    }
}

/// 对单个函数执行常量折叠（公共入口，也可被 IrInterpreter 使用）。
pub fn fold_function(func: &mut Function) -> Result<PassResult, IrError> {
    let mut result = PassResult::default();

    // ---- 一次性扫描：value → def 指令映射、已知常量、初始 worklist ----
    // (旧实现每轮重建 known/uses/worklist，并对 worklist 每个 value 做全函数
    // 线性扫描 find_def_inst，导致 O(n²)/O(n³) 的固定开销。这里只扫一遍，
    // 之后增量更新 known，worklist 单遍即可完成传播。)
    let mut def_map: HashMap<Value, Inst> = HashMap::new();
    let mut known: HashMap<Value, ConstValue> = HashMap::new();
    let mut worklist: Vec<Value> = Vec::new();

    for block in func.dfg.blocks.iter() {
        for &inst_id in &block.inst_order {
            let inst = &func.dfg.insts[inst_id.0 as usize];
            let Some(v) = inst.results.first().copied() else {
                continue;
            };
            def_map.insert(v, inst_id);

            match &inst.opcode {
                Opcode::Iconst | Opcode::Fconst | Opcode::Vconst => {
                    if let Some(cid) = inst.immediates.first().and_then(|i| i.as_const())
                        && let Some(big) = func.constants.resolve_big(cid)
                    {
                        let ty = func.dfg.value_type(v).unwrap_or(TypeId::VOID);
                        let cv = if matches!(inst.opcode, Opcode::Iconst) {
                            ConstValue::Int(big, ty)
                        } else {
                            ConstValue::Float(big, ty)
                        };
                        known.insert(v, cv);
                    }
                }
                _ => worklist.push(v),
            }
        }
    }

    // 使用关系（一次构建；折叠不修改 operands，因此无需重建）
    let uses = collect_uses(func);

    // ---- Worklist 循环（单遍，增量传播）----
    let mut processed: std::collections::HashSet<Value> = std::collections::HashSet::new();

    while let Some(value) = worklist.pop() {
        if processed.contains(&value) {
            continue;
        }

        // 通过 def_map O(1) 定位定义指令（旧实现为全函数线性扫描）
        let def_inst_id = match def_map.get(&value) {
            Some(&id) => id,
            None => continue,
        };
        let inst_info = &func.dfg.insts[def_inst_id.0 as usize];

        // 收集 operands 的常量值（SmallVec：多数指令 1-3 个操作数，避免堆分配）
        let const_operands: smallvec::SmallVec<[ConstValue; 4]> = inst_info
            .operands
            .iter()
            .filter_map(|v| known.get(v).cloned())
            .collect();

        // 如果不是所有 operands 都是常量，跳过且不标记 processed：
        // 该值可能在后续折叠中变成可折叠（operand 传播后），
        // 需要保留再次处理的机会。仅在所有 operands 已知时才标记，
        // 保证 worklist 单遍收敛（否则 UntilFixedPoint 要跑数十轮）。
        if const_operands.len() != inst_info.operands.len() {
            continue;
        }
        processed.insert(value);

        // 获取此指令结果值的类型
        let result_ty = func.dfg.value_type(value).unwrap_or(TypeId::VOID);

        // 尝试折叠（ExtractValue 聚合字面量走专用路径——需要常量池）
        if let Some(folded) = if matches!(inst_info.opcode, Opcode::ExtractValue) {
            fold_extract_value(&func.constants, &inst_info.immediates, result_ty)?
        } else {
            fold_opcode(&inst_info.opcode, &const_operands, result_ty)?
        } {
            // 替换指令为 Iconst/Fconst (插入常量池)
            let new_const_id = match &folded {
                ConstValue::Int(v, _) => func.constants.insert_big(v.clone()),
                ConstValue::Float(v, _) => func.constants.insert_big(v.clone()),
                ConstValue::Bool(b) => {
                    let big = Big::from_i64(if *b { 1 } else { 0 });
                    func.constants.insert_big(big)
                }
            };

            let new_opcode = match &folded {
                ConstValue::Int(..) => Opcode::Iconst,
                ConstValue::Float(..) => Opcode::Fconst,
                ConstValue::Bool(_) => Opcode::Iconst, // bool → Iconst(0/1)
            };

            // Update the instruction in-place（同步 use-lists：清掉旧 operands）
            if !func.dfg.insts[def_inst_id.0 as usize].operands.is_empty() {
                func.use_lists.remove_inst(&func.dfg, def_inst_id);
            }
            let inst = &mut func.dfg.insts[def_inst_id.0 as usize];
            inst.opcode = new_opcode;
            inst.operands.clear();
            inst.immediates = smallvec::smallvec![Immediate::Const(new_const_id)];
            // Update result value type if bool was folded
            if matches!(folded, ConstValue::Bool(_))
                && let Some(result_val) = inst.results.first().copied()
            {
                func.dfg.values[result_val.0 as usize].ty = TypeId::I32;
            }

            known.insert(value, folded);
            result.instructions_removed += 1;

            // 将所有使用者加入 worklist
            if let Some(users) = uses.get(&value) {
                for &(_user_bi, ui) in users {
                    if ui == usize::MAX {
                        // Terminator 使用者 — 不需要加入 worklist
                        // （由 fold_branches 处理）
                        continue;
                    }
                    let user_inst = &func.dfg.insts[ui];
                    if let Some(uv) = user_inst.results.first().copied()
                        && !known.contains_key(&uv)
                    {
                        worklist.push(uv);
                    }
                }
            }
        }
    }

    // 分支折叠（常量条件分支 → 无条件跳转）。
    // 注意：常量折叠在 worklist 单遍内已完成传播，fold_branches 不再产生
    // 新的指令级折叠机会，因此不需要旧实现的外层 while changed 循环。
    if fold_branches(func, &known) {
        // 分支折叠后另一分支目标不可达，立即消除死块，保持 CFG 干净。
        let dead_blocks = crate::scalar::dead_code::eliminate_dead_blocks(func);
        result.blocks_removed += dead_blocks;
    }

    result.changed = result.instructions_removed > 0 || result.blocks_removed > 0;
    if result.changed {
        // 指令/分支/死块已修改：失效分析缓存，避免后续 pass 复用 stale 数据。
        func.analysis_mut().invalidate();
    }
    Ok(result)
}

/// 折叠常量条件分支。
fn fold_branches(func: &mut Function, known: &HashMap<Value, ConstValue>) -> bool {
    let mut changed = false;

    for block in func.dfg.blocks.iter_mut() {
        if let Terminator::Branch {
            cond,
            then_block,
            else_block,
            then_args,
            else_args,
            ..
        } = &block.terminator
            && let Some(const_val) = known.get(cond)
            && let Some(is_true) = const_val.to_bool()
        {
            // 替换为无条件跳转
            let (target, args) = if is_true {
                (*then_block, then_args.clone())
            } else {
                (*else_block, else_args.clone())
            };
            block.terminator = Terminator::Jump {
                target,
                args,
                metadata: smallvec::smallvec![],
            };
            changed = true;
        }
    }

    changed
}

// ============================================================
// 测试 (v2 API — minimal: port remaining tests from v1 later)
// ============================================================

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{OptimizationPass, PassManager, PassRunMode};

    /// Helper: read i64 from an Iconst instruction's constant pool entry.
    fn get_iconst_i64(func: &Function, inst_id: Inst) -> Option<i64> {
        let inst = &func.dfg.insts[inst_id.0 as usize];
        if matches!(inst.opcode, Opcode::Iconst) {
            inst.immediates
                .first()
                .and_then(|i| i.as_const())
                .and_then(|cid| func.constants.get_big(cid))
                .and_then(|b| b.try_to_i64())
        } else {
            None
        }
    }

    /// Find an instruction by its result Value and read its constant value.
    fn get_iconst_value_for(func: &Function, value: Value) -> Option<i64> {
        for block in func.dfg.blocks.iter() {
            for &inst_id in &block.inst_order {
                let inst = &func.dfg.insts[inst_id.0 as usize];
                if inst.results.first().copied() == Some(value) {
                    return get_iconst_i64(func, inst_id);
                }
            }
        }
        None
    }

    #[test]
    fn fold_extractvalue_agg_literal() {
        // 3.1：extractvalue 聚合字面量常量折叠——标量提取折叠为常量
        //（通过 parse 构建：semantics 把聚合字面量写入 ConstantPool）
        let src = "define i32 @f() {
  %e:
    %c = extractvalue [2 x i32] [i32 1, i32 2], 1
    ret i32 %c
}";
        let module = crate::ir_parser::parse_module(src).expect("parse");
        let mut func = module.iter_functions().next().unwrap().clone();
        let pass = ConstFoldPass::new();
        let result = pass.run_on_function(&mut func).unwrap();
        assert!(result.changed, "聚合字面量 extractvalue 应折叠（%c = 2）");
        // %c 应为常量 2
        let def = func.dfg.value_def(Value(2)).cloned();
        if let Some(forge_ir::dfg::ValueDef::Inst(iid, _)) = def {
            let inst = &func.dfg.insts[iid.0 as usize];
            assert!(
                matches!(inst.opcode, Opcode::Iconst),
                "折叠后应为 Iconst，got {:?}",
                inst.opcode
            );
        }
    }

    #[test]
    fn fold_iadd_constants() {
        let sig = FunctionSignature::new(&[], &[TypeId::I32]);
        let mut b = FunctionBuilder::new("test", TypeContext::new(), sig);
        let entry = b.create_block();
        b.switch_to_block(entry);
        let a = b.iconst_i32(3);
        let b_val = b.iconst_i32(5);
        let sum = b.iadd(a, b_val);
        b.ret(&[sum]);

        let mut func = b.finish().expect("build");
        let pass = ConstFoldPass::new();
        let result = pass.run_on_function(&mut func).unwrap();

        assert!(result.changed);
        assert!(result.instructions_removed >= 1);
        assert_eq!(get_iconst_value_for(&func, sum), Some(8));
    }

    #[test]
    fn fold_in_pass_manager() {
        let sig = FunctionSignature::new(&[], &[TypeId::I32]);
        let mut b = FunctionBuilder::new("test", TypeContext::new(), sig);
        let entry = b.create_block();
        b.switch_to_block(entry);
        let a = b.iconst_i32(100);
        let b_val = b.iconst_i32(200);
        let sum = b.iadd(a, b_val);
        b.ret(&[sum]);

        let mut func = b.finish().expect("build");
        let mut pm = PassManager::new();
        pm.add_pass(Box::new(ConstFoldPass::new()), PassRunMode::Once);
        let result = pm.run_on_function(&mut func).unwrap();

        assert!(result.changed);
        assert_eq!(get_iconst_value_for(&func, sum), Some(300));
    }

    #[test]
    fn no_fold_mixed_constants() {
        let sig = FunctionSignature::new(&[(TypeId::I32, "x")], &[TypeId::I32]);
        let mut b = FunctionBuilder::new("test", TypeContext::new(), sig);
        let (entry, params) = b.create_block_with_params(&[(TypeId::I32, "x")]);
        b.switch_to_block(entry);
        let x = params[0];
        let c = b.iconst_i32(5);
        let sum = b.iadd(x, c);
        b.ret(&[sum]);

        let mut func = b.finish().expect("build");
        let pass = ConstFoldPass::new();
        let r = pass.run_on_function(&mut func).unwrap();
        // 不应该折叠（x 不是常量）
        assert!(!r.changed);
    }

    #[test]
    fn fold_branch_eliminates_dead_block() {
        // Build: if (true) { ret 42 } else { ret 99 }
        // After const_fold, the else block should be eliminated as unreachable.
        let sig = FunctionSignature::new(&[], &[TypeId::I32]);
        let mut b = FunctionBuilder::new("test", TypeContext::new(), sig);
        let entry = b.create_block();
        let then_blk = b.create_block();
        let else_blk = b.create_block();

        b.switch_to_block(entry);
        let one = b.iconst_i32(1); // always true (non-zero)
        b.branch(one, then_blk, &[], else_blk, &[]);

        b.switch_to_block(then_blk);
        let v42 = b.iconst_i32(42);
        b.ret(&[v42]);

        b.switch_to_block(else_blk);
        let v99 = b.iconst_i32(99);
        b.ret(&[v99]);

        let mut func = b.finish().expect("build");
        let pass = ConstFoldPass::new();
        let r = pass.run_on_function(&mut func).unwrap();

        assert!(
            r.changed,
            "Should fold constant branch and eliminate dead block"
        );
        assert!(r.blocks_removed >= 1, "Dead block should be eliminated");

        // The else block should be unreachable now
        let else_block = &func.dfg.blocks[else_blk.0 as usize];
        assert!(
            else_block.inst_order.is_empty()
                || matches!(else_block.terminator, Terminator::Unreachable),
            "Else block should be cleared (dead)"
        );
    }
}
