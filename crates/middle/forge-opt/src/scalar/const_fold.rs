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
use dashu::base::Abs;
use dashu::base::BitTest;
use dashu::integer::UBig;
use forge_ir::IrError;
use forge_ir::entity::map::SecondaryMap;
use forge_ir::*;
use forge_ir::{Big, FloatFormat};

// ============================================================
// 整数宽度辅助函数（基于 Big）
// ============================================================

/// 按类型宽度截断 Big 值（有符号语义）。
fn truncate_to_type(value: &Big, ty: TypeId) -> Big {
    let bits = scalar_bits(ty);
    if bits == 0 || (!ty.is_int()) {
        return value.clone();
    }
    value.truncate_to_bits_signed(bits)
}

/// 将 Big 解释为固定位宽的无符号值。
fn as_unsigned_big(value: &Big, ty: TypeId) -> Big {
    let bits = scalar_bits(ty);
    if bits == 0 {
        return Big::U_ZERO;
    }
    value.truncate_to_bits(bits)
}

// ============================================================
// 折叠 — 核心求值函数
// ============================================================

/// 沿 `path` 在常量聚合树里**深层下钻**（backlog #3 ✗）：`path` 是逐级字段索引 ✗。
///
/// 返回最终落点的子节点 ✗（标量或更深的聚合 ✗）；路径越界 / 中途落到标量 ⇒ `None` ✗。
fn agg_child_at(
    pool: &forge_ir::ir::constant::ConstantPool,
    agg: forge_ir::ConstId,
    path: &[usize],
) -> Option<forge_ir::ConstId> {
    let (first, rest) = path.split_first()?;
    let mut node: forge_ir::ConstId = *pool.agg(agg)?.children.get(*first)?;
    for idx in rest {
        // 子节点是聚合才能继续下钻；标量子节点 ⇒ 路径越界
        pool.agg(node)?;
        node = *pool.agg(node)?.children.get(*idx)?;
    }
    Some(node)
}

/// 解析"在聚合值 `value` 上**再**取下标 `extra`"⇒ `(根部常量聚合, 完整索引路径 ✗)` ✗。
///
/// `extractvalue` 的聚合源有两种形状 ✗：**immediate**（`[idx, Agg(id), Type]` ✗）或
/// **值**（`extractvalue {t} %a, i` ✗）。后者在 SSA 里必由前一条 `extractvalue`
/// 定义 ✗ ⇒ 顺着定义链把各层下标拼起来 ✗，就能直接从常量池的聚合树里一次折到底 ✗
/// （这是 backlog #3「`AggConst` 递归 child 缺深层索引」的正解 ✗）。
///
/// 返回 `None` ⇒ 链的根部不是常量聚合 ✗（按"折不了"处理 ✗，不是错误 ✗）。
fn resolve_agg_chain(
    func: &forge_ir::Function,
    value: Value,
    extra: usize,
) -> Option<(forge_ir::ConstId, Vec<usize>)> {
    use forge_ir::ir::dfg::ValueDef;
    let inst_id = match func.dfg.value_def(value)? {
        ValueDef::Inst(i, _) => *i,
        _ => return None,
    };
    let inst = func.dfg.inst_data(inst_id);
    if !matches!(inst.opcode, Opcode::ExtractValue) {
        return None;
    }
    let own = match inst.immediates.first() {
        Some(Immediate::Uint(i)) => *i as usize,
        _ => return None,
    };
    match inst.immediates.get(1) {
        // 链的根部：常量聚合（`Const` 里的 Agg tag）⇒ 路径 = [本层 idx, 追加的 extra]
        Some(Immediate::Const(root)) if root.tag() == forge_ir::ConstId::TAG_AGG => {
            Some((*root, vec![own, extra]))
        }
        _ => {
            let src = *inst.operands.first()?;
            let (root, mut path) = resolve_agg_chain(func, src, own)?;
            path.push(extra);
            Some((root, path))
        }
    }
}

/// 对给定的操作码和常量操作数进行编译时求值。
///
/// 返回 `Some(ConstValue)` 如果所有操作数已知且可求值。
/// 折叠 `extractvalue` 聚合字面量（3.1：immediates = [idx, Agg(id), Type]）。
/// 标量提取 → 标量 ConstValue；嵌套聚合结果本身不折叠（值语义 ✗），
/// 但**链上的下一层**会带着完整路径再折一次（见 [`resolve_agg_chain`] ✗）。
fn fold_extract_value(
    pool: &forge_ir::ir::constant::ConstantPool,
    agg: forge_ir::ConstId,
    path: &[usize],
    result_ty: TypeId,
) -> Result<Option<ConstValue>, IrError> {
    let Some(cid) = agg_child_at(pool, agg, path) else {
        return Ok(None);
    };
    // 嵌套聚合结果：不做标量折叠（值语义；codegen 仅折叠时读取）
    if pool.agg(cid).is_some() {
        return Ok(None);
    }
    Ok(match pool.get(cid) {
        Some(forge_ir::ir::constant::ConstEntry::Bits(bv)) => {
            Some(ConstValue::Int(Big::from_i128(bv.bits as i128), result_ty))
        }
        Some(forge_ir::ir::constant::ConstEntry::Big(b)) => {
            Some(ConstValue::Int(b.clone(), result_ty))
        }
        _ => None,
    })
}

/// 返回 `None` 如果操作数不足或包含未知值。
///
/// `immediates` 是**指令的 immediate 通道**（v3 S1 起比较条件经
/// `Immediate::IntCC`/`FloatCC` 传递，不再是 `Opcode` 载荷）；
/// 只有比较类折叠会读它，其余分支忽略。
/// 常量折叠的标量位宽（**内建标量表**；调用点已由 `fold_opcode` 的入口守卫保证
/// 是内建标量，`debug_assert` 兜住违反）。
///
/// 这是 v3 S3 去 `TypeId::bits()` 的结果：不再"猜"位宽——动态位宽类型
/// （`i24` 等由 `TypeStore::int_ty` 内部化）在本模块（无 store）**不折叠**。
fn scalar_bits(ty: TypeId) -> u32 {
    let bits = ty.builtin_scalar_bits();
    debug_assert!(bits.is_some(), "常量折叠只应处理内建标量，实际 {ty:?}");
    bits.unwrap_or(0)
}

pub fn fold_opcode(
    opcode: &Opcode,
    operands: &[ConstValue],
    immediates: &[forge_ir::Immediate],
    ty: TypeId,
) -> Result<Option<ConstValue>, IrError> {
    // fail-closed 守卫：位宽运算只对有静态位宽的标量成立。动态位宽类型（i24…）、
    // 向量、复合类型一律**不折叠**——宁可不优化，也不按错误位宽算出错值。
    // 要折叠动态位宽需要把 `TypeStore` 传进本模块（S3 后续项，见方案 §6）。
    if ty.builtin_scalar_bits().is_none() {
        return Ok(None);
    }
    match opcode {
        // v27 M2.5：内联汇编是**副作用 + 不透明文本**，永远不折叠（也不许被
        // DCE/CSE 动到——`side_effect = true` 的那一侧由 pass 各自判）。
        Opcode::InlineAsm => Ok(None),
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
                    let bits = scalar_bits(ty);
                    if bits == 0 {
                        return Ok(Some(ConstValue::Int(Big::S_ZERO, ty)));
                    }
                    let mask = Big::bit_mask(bits);
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
            let Some(shift) = shift_mod(b, scalar_bits(ty)) else {
                return Ok(None);
            };
            let result = a.clone() << shift as usize;
            Ok(Some(ConstValue::Int(truncate_to_type(&result, ty), ty)))
        }
        Opcode::Ushr => {
            let (a, b) = match get_two_bigs(operands) {
                Some(v) => v,
                None => return Ok(None),
            };
            let Some(shift) = shift_mod(b, scalar_bits(ty)) else {
                return Ok(None);
            };
            let ua = as_unsigned_big(a, ty);
            let result = ua >> shift as usize;
            Ok(Some(ConstValue::Int(truncate_to_type(&result, ty), ty)))
        }
        Opcode::Sshr => {
            let (a, b) = match get_two_bigs(operands) {
                Some(v) => v,
                None => return Ok(None),
            };
            let Some(shift) = shift_mod(b, scalar_bits(ty)) else {
                return Ok(None);
            };
            let sa = a.truncate_to_bits_signed(scalar_bits(ty));
            let result = sa >> shift as usize;
            Ok(Some(ConstValue::Int(truncate_to_type(&result, ty), ty)))
        }
        // === 整数比较 ===
        Opcode::Icmp => match immediates.iter().find_map(|im| im.as_int_cc()) {
            Some(cond) => fold_icmp(operands, cond, ty),
            // 条件缺失 = 坏 IR（`Verifier` 的 MissingCondImmediate 会先报）；
            // 折叠层不猜条件，直接不折叠。
            None => Ok(None),
        },

        // === 浮点比较 ===
        Opcode::Fcmp => match immediates.iter().find_map(|im| im.as_float_cc()) {
            Some(cond) => fold_fcmp(operands, cond, ty),
            None => Ok(None),
        },

        // === 浮点算术 ===
        Opcode::Fadd => fold_binary_float(operands, ty, |a, b| a + b),
        Opcode::Fsub => fold_binary_float(operands, ty, |a, b| a - b),
        Opcode::Fmul => fold_binary_float(operands, ty, |a, b| a * b),
        Opcode::Fdiv => fold_fdiv(operands, ty),
        Opcode::Frem => fold_frem(operands, ty),
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
            let Some(fmt) = ty.try_float_format() else {
                return Ok(None);
            };
            match &operands[0] {
                ConstValue::Float(v, _) => {
                    // 负数的平方根 = NaN（Big::Float 无法表示）⇒ 唯一不折叠的情形。
                    if v.is_neg() {
                        return Ok(None);
                    }
                    // 目标 p 位 + 守卫位：`sqrt` 在提升后的精度上求值，最终一次舍入。
                    let guard = fmt.sig_bits + 8;
                    let r = match v.with_precision_at_least(guard) {
                        Big::Float(r) => r,
                        _ => return Ok(None),
                    };
                    Ok(Some(ConstValue::Float(Big::Float(r.sqrt()), ty)))
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

        // === 终结符指令（S4 主体：终结符就是指令）===
        // 常量折叠只处理值生产者；终结符不产值（`fold_opcode` 的前提是"有结果值"），
        // 分支折叠走 `fold_branches`，这里一律不折叠。
        Opcode::Ret
        | Opcode::Jmp
        | Opcode::Br
        | Opcode::Switch
        | Opcode::Unreachable
        | Opcode::Invoke
        | Opcode::Resume => Ok(None),

        // 变参取参（v20 变参 V2）：结果是指向未命名实参区的**地址**（由约定给），
        // 不是常量 ⇒ 不折叠。
        Opcode::VaStart => Ok(None),

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
        //
        // 全部走 dashu 原生位运算（v27 M6-121）：`UBig::bit_len`/`bit`/`count_ones`/
        // `trailing_zeros`/`split_bits` + Big 的移位/掩码。旧实现先 `trunc_to_u64()`
        // 再算，i128 的高 64 位会被丢掉。
        Opcode::Clz => {
            let a = match get_one_big(operands) {
                Some(v) => v,
                None => return Ok(None),
            };
            let bits = scalar_bits(ty);
            if bits == 0 {
                return Ok(Some(ConstValue::Int(Big::U_ZERO, ty)));
            }
            let u = unsigned_bits_of(a, bits);
            let result = bits.saturating_sub(u.bit_len() as u32);
            Ok(Some(ConstValue::Int(
                Big::from_u64(u64::from(result)).truncate_to_bits(bits),
                ty,
            )))
        }
        Opcode::Ctz => {
            let a = match get_one_big(operands) {
                Some(v) => v,
                None => return Ok(None),
            };
            let bits = scalar_bits(ty);
            if bits == 0 {
                return Ok(Some(ConstValue::Int(Big::U_ZERO, ty)));
            }
            let u = unsigned_bits_of(a, bits);
            let result = u.trailing_zeros().map_or(bits, |z| (z as u32).min(bits));
            Ok(Some(ConstValue::Int(
                Big::from_u64(u64::from(result)).truncate_to_bits(bits),
                ty,
            )))
        }
        Opcode::Popcnt => {
            let a = match get_one_big(operands) {
                Some(v) => v,
                None => return Ok(None),
            };
            let bits = scalar_bits(ty);
            if bits == 0 {
                return Ok(Some(ConstValue::Int(Big::U_ZERO, ty)));
            }
            let u = unsigned_bits_of(a, bits);
            Ok(Some(ConstValue::Int(
                Big::from_u64(u.count_ones() as u64).truncate_to_bits(bits),
                ty,
            )))
        }
        Opcode::Bitreverse => {
            let a = match get_one_big(operands) {
                Some(v) => v,
                None => return Ok(None),
            };
            let bits = scalar_bits(ty);
            if bits == 0 {
                return Ok(Some(ConstValue::Int(Big::U_ZERO, ty)));
            }
            let u = unsigned_bits_of(a, bits);
            let mut r = UBig::ZERO;
            for i in 0..bits as usize {
                if u.bit(i) {
                    r.set_bit(bits as usize - 1 - i);
                }
            }
            Ok(Some(ConstValue::Int(
                Big::Unsigned(r).truncate_to_bits(bits),
                ty,
            )))
        }
        Opcode::Rotl => {
            let (a, b) = match get_two_bigs(operands) {
                Some(v) => v,
                None => return Ok(None),
            };
            let bits = scalar_bits(ty);
            if bits == 0 {
                return Ok(Some(ConstValue::Int(Big::U_ZERO, ty)));
            }
            let Some(shift) = shift_mod(b, bits) else {
                return Ok(None);
            };
            let x = Big::Unsigned(unsigned_bits_of(a, bits));
            let result = if shift == 0 {
                x
            } else {
                ((x.clone() << shift as usize) | (x >> (bits - shift) as usize))
                    & unsigned_max(bits)
            };
            Ok(Some(ConstValue::Int(result.truncate_to_bits(bits), ty)))
        }
        Opcode::Rotr => {
            let (a, b) = match get_two_bigs(operands) {
                Some(v) => v,
                None => return Ok(None),
            };
            let bits = scalar_bits(ty);
            if bits == 0 {
                return Ok(Some(ConstValue::Int(Big::U_ZERO, ty)));
            }
            let Some(shift) = shift_mod(b, bits) else {
                return Ok(None);
            };
            let x = Big::Unsigned(unsigned_bits_of(a, bits));
            let result = if shift == 0 {
                x
            } else {
                ((x.clone() >> shift as usize) | (x << (bits - shift) as usize))
                    & unsigned_max(bits)
            };
            Ok(Some(ConstValue::Int(result.truncate_to_bits(bits), ty)))
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
            let bits = scalar_bits(ty);
            if bits == 0 {
                return Ok(Some(ConstValue::Int(Big::S_ZERO, ty)));
            }
            // 全在 Big 上做（v27 M6-121）：旧实现 `try_to_i64()` 对 i128 直接放弃、
            // 且饱和边界写死 i64。现按 bits 精确算上下界并 clamp。
            let sa = a.truncate_to_bits_signed(bits);
            let sb = b.truncate_to_bits_signed(bits);
            let (min, max) = signed_bounds(bits);
            let sum = sa + sb;
            let result = if sum > max {
                max
            } else if sum < min {
                min
            } else {
                sum
            };
            Ok(Some(ConstValue::Int(truncate_to_type(&result, ty), ty)))
        }
        Opcode::SsubSat => {
            let (a, b) = match get_two_bigs(operands) {
                Some(v) => v,
                None => return Ok(None),
            };
            let bits = scalar_bits(ty);
            if bits == 0 {
                return Ok(Some(ConstValue::Int(Big::S_ZERO, ty)));
            }
            let sa = a.truncate_to_bits_signed(bits);
            let sb = b.truncate_to_bits_signed(bits);
            let (min, max) = signed_bounds(bits);
            let diff = sa - sb;
            let result = if diff > max {
                max
            } else if diff < min {
                min
            } else {
                diff
            };
            Ok(Some(ConstValue::Int(truncate_to_type(&result, ty), ty)))
        }
        Opcode::UaddSat => {
            let (a, b) = match get_two_bigs(operands) {
                Some(v) => v,
                None => return Ok(None),
            };
            let bits = scalar_bits(ty);
            if bits == 0 {
                return Ok(Some(ConstValue::Int(Big::U_ZERO, ty)));
            }
            let ua = a.truncate_to_bits(bits);
            let ub = b.truncate_to_bits(bits);
            let max = unsigned_max(bits);
            let sum = ua + ub;
            let result = if sum > max { max } else { sum };
            Ok(Some(ConstValue::Int(truncate_to_type(&result, ty), ty)))
        }
        Opcode::UsubSat => {
            let (a, b) = match get_two_bigs(operands) {
                Some(v) => v,
                None => return Ok(None),
            };
            let bits = scalar_bits(ty);
            if bits == 0 {
                return Ok(Some(ConstValue::Int(Big::U_ZERO, ty)));
            }
            let ua = a.truncate_to_bits(bits);
            let ub = b.truncate_to_bits(bits);
            let result = if ua < ub { Big::U_ZERO } else { ua - ub };
            Ok(Some(ConstValue::Int(truncate_to_type(&result, ty), ty)))
        }
        Opcode::Bswap => {
            let a = match get_one_big(operands) {
                Some(v) => v,
                None => return Ok(None),
            };
            let bits = scalar_bits(ty);
            if bits == 0 || !bits.is_multiple_of(8) {
                // bswap 只对整字节位宽有定义（其余位宽不折叠）。
                return Ok(None);
            }
            let bytes = (bits / 8) as usize;
            // dashu 原生 `split_bits(8)` 逐字节搬（不手写掩码）。
            let mut x = unsigned_bits_of(a, bits);
            let mut r = UBig::ZERO;
            for _ in 0..bytes {
                let (lo, hi) = x.split_bits(8);
                r = (r << 8usize) | lo;
                x = hi;
            }
            Ok(Some(ConstValue::Int(
                Big::Unsigned(r).truncate_to_bits(bits),
                ty,
            )))
        }
        // === 浮点扩展 (8) ===
        //
        // **全格式折叠**（v27 M6-121）：f16/f32/f64/f128 都支持。中间态按目标精度
        // + 守卫位用 Big/Real 任意精度算，落到常量池时一次舍入；只有"结果是 NaN"
        // （0/0、负数开方、fmod 0）不折叠——`Big::Float` 是精确实数，没有 NaN。
        Opcode::Fma => {
            if operands.len() < 3 {
                return Ok(None);
            }
            let Some(fmt) = ty.try_float_format() else {
                return Ok(None);
            };
            match (&operands[0], &operands[1], &operands[2]) {
                (ConstValue::Float(a, _), ConstValue::Float(b, _), ConstValue::Float(c, _)) => {
                    // a*b 需要 2p 位；再加 c ⇒ 留 2p+16 守卫位，最终一次舍入。
                    let guard = fmt.sig_bits * 2 + 16;
                    let aa = a.with_precision_at_least(guard);
                    let bb = b.with_precision_at_least(guard);
                    let cc = c.with_precision_at_least(guard);
                    Ok(Some(ConstValue::Float(aa * bb + cc, ty)))
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
                    // 原生 `Ord` 选小者（不经 f64：f128/超大值不丢精度）；相等时按
                    // IEEE-754 的零符号规则取负零（`-0.0 < +0.0` 的语义在 min 里）。
                    let ord = a.cmp(b);
                    let pick_b = ord == std::cmp::Ordering::Greater
                        || (ord == std::cmp::Ordering::Equal && !a.is_neg() && b.is_neg());
                    let picked = if pick_b { b } else { a };
                    Ok(Some(ConstValue::Float(picked.clone(), ty)))
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
                    // 同上；相等时取正零（max 方向）。
                    let ord = a.cmp(b);
                    let pick_b = ord == std::cmp::Ordering::Less
                        || (ord == std::cmp::Ordering::Equal && a.is_neg() && !b.is_neg());
                    let picked = if pick_b { b } else { a };
                    Ok(Some(ConstValue::Float(picked.clone(), ty)))
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
                    // 只复制**符号位**（量值原样保留，不经 f64——f128 转 f64 会变 Inf）。
                    let mag = match a.abs() {
                        Big::Float(r) => r,
                        _ => return Ok(None),
                    };
                    let out = if b.is_neg() { -mag } else { mag };
                    Ok(Some(ConstValue::Float(Big::Float(out), ty)))
                }
                _ => Ok(None),
            }
        }
        Opcode::Ffloor => {
            if operands.is_empty() {
                return Ok(None);
            }
            match &operands[0] {
                // floor/ceil/trunc/round 在精确实数上是**精确运算**（不引入舍入），
                // 任意格式都直接做（整型变体本身就是整数 ⇒ 原样）。
                ConstValue::Float(v, _) => {
                    let out = match v {
                        Big::Float(r) => Big::Float(r.floor()),
                        other => other.clone(),
                    };
                    Ok(Some(ConstValue::Float(out, ty)))
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
                    let out = match v {
                        Big::Float(r) => Big::Float(r.ceil()),
                        other => other.clone(),
                    };
                    Ok(Some(ConstValue::Float(out, ty)))
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
                    let out = match v {
                        Big::Float(r) => Big::Float(r.trunc()),
                        other => other.clone(),
                    };
                    Ok(Some(ConstValue::Float(out, ty)))
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
                    let out = match v {
                        Big::Float(r) => Big::Float(r.round()),
                        other => other.clone(),
                    };
                    Ok(Some(ConstValue::Float(out, ty)))
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

/// 移位量按位宽**取模**（与后端 lowering 的硬件语义一致：x86 的 `SHR CL`/arm64 的
/// `LSRV` 都只使用低 log2(位宽) 位）。任意大的 Big 移位量都能精确取模。
fn shift_mod(b: &Big, bits: u32) -> Option<u32> {
    if bits == 0 {
        return None;
    }
    let v = unsigned_bits_of(b, 128);
    let rem = v % UBig::from(bits);
    Some(u32::try_from(&rem).unwrap_or(0))
}

/// 值的**低 `bits` 位**裸位模式（无符号视角；dashu 原生 `truncate_to_bits` 的结果
/// 恒是 `Unsigned`）。
fn unsigned_bits_of(a: &Big, bits: u32) -> UBig {
    match a.truncate_to_bits(bits) {
        Big::Unsigned(u) => u,
        Big::Signed(i) => UBig::try_from(i.abs()).unwrap_or(UBig::ZERO),
        Big::Float(_) => UBig::ZERO,
    }
}

/// `bits` 位有符号整数的 `(min, max)`（任意精度构造，128 位也正确）。
fn signed_bounds(bits: u32) -> (Big, Big) {
    let half = Big::U_ONE << (bits as usize - 1);
    let half_i = match &half {
        Big::Unsigned(u) => dashu::Integer::from(u.clone()),
        Big::Signed(i) => i.clone(),
        _ => dashu::Integer::from(0u8),
    };
    (
        Big::Signed(-half_i.clone()),
        Big::Signed(half_i - dashu::Integer::from(1u8)),
    )
}

/// `bits` 位无符号整数的最大值 `2^bits - 1`。
fn unsigned_max(bits: u32) -> Big {
    (Big::U_ONE << bits as usize) - Big::U_ONE
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

/// 浮点二元折叠（`+`/`-`/`*`）：目标格式 p 位，中间态提升到 `p + 8` 守卫位再做
/// 运算，最终由落到常量池时的 `to_float_bits` **按目标格式一次舍入** ⇒ f16/f32/f64/
/// f128 全部精确折叠（RNE 下 ≥2 位守卫即无双重舍入误差；v27 M6-121）。
fn fold_binary_float(
    operands: &[ConstValue],
    ty: TypeId,
    op: fn(Big, Big) -> Big,
) -> Result<Option<ConstValue>, IrError> {
    if operands.len() < 2 {
        return Ok(None);
    }
    let Some(fmt) = ty.try_float_format() else {
        return Ok(None);
    };
    let guard = fmt.sig_bits + 8;
    match (&operands[0], &operands[1]) {
        (ConstValue::Float(a, _), ConstValue::Float(b, _)) => {
            let aa = a.with_precision_at_least(guard);
            let bb = b.with_precision_at_least(guard);
            Ok(Some(ConstValue::Float(op(aa, bb), ty)))
        }
        _ => Ok(None),
    }
}

/// 浮点除法折叠：`x / 0` 按 IEEE 得 ±inf（Big 能表示）；`0 / 0` = NaN
/// （Big::Float 无 NaN ⇒ 唯一不折叠的情形）。
fn fold_fdiv(operands: &[ConstValue], ty: TypeId) -> Result<Option<ConstValue>, IrError> {
    if operands.len() < 2 {
        return Ok(None);
    }
    let Some(fmt) = ty.try_float_format() else {
        return Ok(None);
    };
    match (&operands[0], &operands[1]) {
        (ConstValue::Float(a, _), ConstValue::Float(b, _)) => {
            if b.is_zero() {
                if a.is_zero() {
                    return Ok(None); // NaN
                }
                let neg = a.is_neg() != b.is_neg();
                let inf = if neg {
                    Big::Float(dashu::Real::NEG_INFINITY)
                } else {
                    Big::Float(dashu::Real::INFINITY)
                };
                return Ok(Some(ConstValue::Float(inf, ty)));
            }
            let guard = fmt.sig_bits + 8;
            let aa = a.with_precision_at_least(guard);
            let bb = b.with_precision_at_least(guard);
            Ok(Some(ConstValue::Float(aa / bb, ty)))
        }
        _ => Ok(None),
    }
}

/// 浮点取余折叠：`fmod(a, 0)` = NaN ⇒ 不折叠；其余按精确值取余（Big 原生 `%`）。
fn fold_frem(operands: &[ConstValue], ty: TypeId) -> Result<Option<ConstValue>, IrError> {
    if operands.len() < 2 {
        return Ok(None);
    }
    let Some(fmt) = ty.try_float_format() else {
        return Ok(None);
    };
    match (&operands[0], &operands[1]) {
        (ConstValue::Float(a, _), ConstValue::Float(b, _)) => {
            if b.is_zero() {
                return Ok(None); // NaN
            }
            let guard = fmt.sig_bits + 8;
            let aa = a.with_precision_at_least(guard);
            let bb = b.with_precision_at_least(guard);
            Ok(Some(ConstValue::Float(aa % bb, ty)))
        }
        _ => Ok(None),
    }
}

fn fold_icmp(
    operands: &[ConstValue],
    cond: IntCC,
    _ty: TypeId,
) -> Result<Option<ConstValue>, IrError> {
    // **必须按操作数自己的位宽截断，不能用结果类型**（v27 M6-120）：`Icmp` 的结果
    // 类型是 `i1`（BOOL），若拿结果类型去截断操作数，`icmp ult i64 0, i64 4` 会把
    // 4 截成 1 位 = 0 ⇒ 0 < 0 = false。实测后果：`-O1` 下 `ub_checks` 的
    // `0 < pointee_size` 被折叠成 false（`store i1 0`），`ExactSizeIterator::len`
    // 一进就 panic。
    let (ConstValue::Int(a, a_ty), ConstValue::Int(b, b_ty)) = (&operands[0], &operands[1]) else {
        return Ok(None);
    };
    let (a_ty, b_ty) = (*a_ty, *b_ty);
    let sa = truncate_to_type(a, a_ty);
    let sb = truncate_to_type(b, b_ty);
    let result = match cond {
        IntCC::Eq => sa == sb,
        IntCC::Ne => sa != sb,
        IntCC::Slt => sa < sb,
        IntCC::Sgt => sa > sb,
        IntCC::Sle => sa <= sb,
        IntCC::Sge => sa >= sb,
        IntCC::Ult => as_unsigned_big(a, a_ty) < as_unsigned_big(b, b_ty),
        IntCC::Ugt => as_unsigned_big(a, a_ty) > as_unsigned_big(b, b_ty),
        IntCC::Ule => as_unsigned_big(a, a_ty) <= as_unsigned_big(b, b_ty),
        IntCC::Uge => as_unsigned_big(a, a_ty) >= as_unsigned_big(b, b_ty),
    };
    // 比较结果：true = 1, false = 0（折叠层统一用 `ConstValue::Bool`，类型由调用方设为 BOOL）
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
            // **用 Big 的原生 `Ord`**（任意精度实数），不要先转 f64（v27 M6-120）：
            // f64 会把 f128/超大值截断；`Big::Float` 由 `from_f64` 构造时已拒绝 NaN
            // （NaN 常量不进 Big 通道 ⇒ 这里不可能出现 NaN），所以有序/无序两族
            // 判据在折叠层恒分别成立：`Ord` 恒真、`Uno` 恒假，`U*` 与 `O*` 同值。
            let ord = a.cmp(b);
            let eq = ord == std::cmp::Ordering::Equal;
            let lt = ord == std::cmp::Ordering::Less;
            let gt = ord == std::cmp::Ordering::Greater;
            let result = match cond {
                FloatCC::False => false,
                FloatCC::True => true,
                FloatCC::Ord => true,
                FloatCC::Uno => false,
                FloatCC::Oeq | FloatCC::Ueq => eq,
                FloatCC::One | FloatCC::Une => !eq,
                FloatCC::Olt | FloatCC::Ult => lt,
                FloatCC::Ole | FloatCC::Ule => lt || eq,
                FloatCC::Ogt | FloatCC::Ugt => gt,
                FloatCC::Oge | FloatCC::Uge => gt || eq,
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
            // 源类型必须是内建标量（动态位宽 → 不折叠）
            let Some(from_bits) = from_ty.builtin_scalar_bits() else {
                return Ok(None);
            };
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
            // 整数 → 浮点：把**低 total_bits 位**当 IEEE 位模式（Big 上做，不经 u64）。
            if let Some(fmt) = to_ty.try_float_format() {
                let total = fmt.total_bits();
                let bits = v.truncate_to_bits(total).trunc_to_u128();
                match Big::from_float_bits(bits, fmt) {
                    Some(float_big) => Ok(Some(ConstValue::Float(float_big, to_ty))),
                    // NaN：Big 表示不了 ⇒ 不折叠（不是错值）
                    None => Ok(None),
                }
            } else {
                Ok(Some(ConstValue::Int(truncate_to_type(v, to_ty), to_ty)))
            }
        }
        ConstValue::Float(v, src_ty) => {
            let src_fmt = src_ty.try_float_format().unwrap_or(FloatFormat::F64);
            // 浮点 → 整数：按**源格式**的位模式，截到目标整数位宽。
            if to_ty.is_int() {
                let Some(to_bits) = to_ty.builtin_scalar_bits() else {
                    return Ok(None);
                };
                let Some(bits) = v.to_float_bits(src_fmt) else {
                    return Ok(None);
                };
                Ok(Some(ConstValue::Int(
                    Big::from_u128(bits).truncate_to_bits(to_bits),
                    to_ty,
                )))
            } else if let Some(dst_fmt) = to_ty.try_float_format() {
                // 位转换只对**同宽**格式有定义；不同宽（f32↔f64）不是 bitcast 语义 ⇒ 不折叠。
                if src_fmt.total_bits() != dst_fmt.total_bits() {
                    return Ok(None);
                }
                let Some(bits) = v.to_float_bits(src_fmt) else {
                    return Ok(None);
                };
                match Big::from_float_bits(bits, dst_fmt) {
                    Some(result) => Ok(Some(ConstValue::Float(result, to_ty))),
                    None => Ok(None),
                }
            } else {
                Ok(None)
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
fn collect_uses(func: &Function) -> SecondaryMap<Value, Vec<(usize, usize)>> {
    let mut uses: SecondaryMap<Value, Vec<(usize, usize)>> = SecondaryMap::new();

    for bi in 0..func.dfg.block_count() {
        let block = &func.dfg.block(Block::new(bi as u32));
        for &inst_id in &block.inst_order {
            let inst = &func.dfg.inst_data(inst_id);
            for operand in &inst.operands {
                uses.get_mut_or_default(*operand)
                    .push((bi, inst_id.index() as usize));
            }
        }
        // 终结符中的值使用（规范序遍历，唯一事实源）
        func.dfg.for_each_term_value(Block::new(bi as u32), |_, v| {
            uses.get_mut_or_default(v).push((bi, usize::MAX));
        });
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
    let mut def_map: SecondaryMap<Value, Inst> = SecondaryMap::new();
    let mut known: SecondaryMap<Value, ConstValue> = SecondaryMap::new();
    let mut worklist: Vec<Value> = Vec::new();

    for block in func.dfg.block_data_iter() {
        for &inst_id in &block.inst_order {
            let inst = &func.dfg.inst_data(inst_id);
            let Some(v) = inst.results.first().copied() else {
                continue;
            };
            def_map.insert(v, inst_id);

            match &inst.opcode {
                Opcode::Iconst | Opcode::Fconst | Opcode::Vconst => {
                    if let Some(cid) = inst.immediates.first().and_then(|i| i.as_const()) {
                        let ty = func.dfg.value_type(v).unwrap_or(TypeId::VOID);
                        match inst.opcode {
                            Opcode::Iconst => {
                                if let Some(big) = func.constants.as_big(cid) {
                                    known.insert(v, ConstValue::Int(big, ty));
                                }
                            }
                            Opcode::Fconst => {
                                // **浮点常量在池里是 Bits 位模式，不是整数**（v27 M6-121）：
                                // `as_big` 会把位模式当补码整数解释 ⇒ 折叠层拿到的是
                                // `4607182418800017408` 而不是 `1.0`。这里按目标浮点格式
                                // 精确解码（f16/32/64/128；NaN 不进 known ⇒ 不折叠）。
                                if let Some(fmt) = ty.try_float_format()
                                    && let Some(bv) = func.constants.bits(cid)
                                    && let Some(big) = Big::from_float_bits(bv.bits, fmt)
                                {
                                    known.insert(v, ConstValue::Float(big, ty));
                                }
                            }
                            _ => {}
                        }
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
        let def_inst_id = match def_map.get(value) {
            Some(&id) => id,
            None => continue,
        };
        let inst_info = &func.dfg.inst_data(def_inst_id);

        // 收集 operands 的常量值（SmallVec：多数指令 1-3 个操作数，避免堆分配）
        let const_operands: smallvec::SmallVec<[ConstValue; 4]> = inst_info
            .operands
            .iter()
            .filter_map(|v| known.get(*v).cloned())
            .collect();

        // 如果不是所有 operands 都是常量，跳过且不标记 processed：
        // 该值可能在后续折叠中变成可折叠（operand 传播后），
        // 需要保留再次处理的机会。仅在所有 operands 已知时才标记，
        // 保证 worklist 单遍收敛（否则 UntilFixedPoint 要跑数十轮）。
        // backlog #3：`extractvalue` 的聚合源可以是**值**（嵌套聚合链 ✗）——这种指令的操作数
        // 不是常量 ✗，但只要能沿定义链上溯到**常量聚合** ✗，仍可一次折到底 ✗。
        let extract_path: Option<(forge_ir::ConstId, Vec<usize>)> =
            if matches!(inst_info.opcode, Opcode::ExtractValue) {
                match inst_info.immediates.first() {
                    Some(Immediate::Uint(i)) => {
                        let idx = *i as usize;
                        match inst_info.immediates.get(1) {
                            Some(Immediate::Const(id))
                                if id.tag() == forge_ir::ConstId::TAG_AGG =>
                            {
                                Some((*id, vec![idx]))
                            }
                            _ => inst_info
                                .operands
                                .first()
                                .copied()
                                .and_then(|src| resolve_agg_chain(func, src, idx)),
                        }
                    }
                    _ => None,
                }
            } else {
                None
            };

        if extract_path.is_none() && const_operands.len() != inst_info.operands.len() {
            continue;
        }
        processed.insert(value);

        // 获取此指令结果值的类型
        let result_ty = func.dfg.value_type(value).unwrap_or(TypeId::VOID);

        // 尝试折叠（ExtractValue 聚合字面量走专用路径——需要常量池）
        if let Some(folded) = if matches!(inst_info.opcode, Opcode::ExtractValue) {
            match &extract_path {
                Some((agg, path)) => fold_extract_value(&func.constants, *agg, path, result_ty)?,
                None => None,
            }
        } else {
            fold_opcode(
                &inst_info.opcode,
                &const_operands,
                &inst_info.immediates,
                result_ty,
            )?
        } {
            // 替换指令为 Iconst/Fconst (插入常量池)
            let new_const_id = match &folded {
                ConstValue::Int(v, _) => func.constants.intern_big(v.clone()),
                ConstValue::Float(v, ty) => {
                    // **浮点结果按目标格式编码回 Bits 通道**（v27 M6-121）：此前走
                    // `intern_big`（任意精度通道），后端 `as_f64_bits` 会把它按 f64
                    // 位模式取——f32 结果因此变成 f64 模式的低 32 位（错值）。
                    // `to_float_bits` 按目标格式**一次正确舍入**（f128 也是真 128 位）。
                    let Some(fmt) = ty.try_float_format() else {
                        continue;
                    };
                    let Some(bits) = v.to_float_bits(fmt) else {
                        // NaN（Big 表示不了）：不折叠，保持原指令
                        continue;
                    };
                    func.constants.intern_float(bits, fmt.total_bits() as u16)
                }
                ConstValue::Bool(b) => {
                    let big = Big::from_i64(if *b { 1 } else { 0 });
                    func.constants.intern_big(big)
                }
            };
            let new_opcode = match &folded {
                ConstValue::Int(..) => Opcode::Iconst,
                ConstValue::Float(..) => Opcode::Fconst,
                ConstValue::Bool(_) => Opcode::Iconst, // bool → Iconst(0/1)
            };

            // Update the instruction in-place（同步 use-lists：清掉旧 operands）
            if !func.dfg.inst_data(def_inst_id).operands.is_empty() {
                func.use_lists.remove_inst(&func.dfg, def_inst_id);
            }
            let inst = func.dfg.inst_mut(def_inst_id);
            inst.opcode = new_opcode;
            inst.operands.clear();
            inst.immediates = smallvec::smallvec![Immediate::Const(new_const_id)];
            // Update result value type if bool was folded
            //
            // **必须是 `BOOL`（i1），不是 `I32`**：本 IR 里比较结果与 `br` 条件恒为 i1
            // （verifier 的 "icmp/fcmp results are Bool" 规则要求）。设成 i32 会让下游
            // 第二条比较一边是 i32、另一边仍是 i8/i64 ⇒ 折叠后 IR 非法。
            // 2026-10-19 实测：`wsprintfA("%d", 42)` + `-O` 触发
            // `TypeMismatch { Icmp, expected: t2, found: t4 }`（i8 vs i32）。
            if matches!(folded, ConstValue::Bool(_))
                && let Some(result_val) = inst.results.first().copied()
            {
                func.dfg.set_value_type(result_val, TypeId::BOOL);
            }

            known.insert(value, folded);
            result.instructions_removed += 1;

            // 将所有使用者加入 worklist
            if let Some(users) = uses.get(value) {
                for &(_user_bi, ui) in users {
                    if ui == usize::MAX {
                        // Terminator 使用者 — 不需要加入 worklist
                        // （由 fold_branches 处理）
                        continue;
                    }
                    let user_inst = &func.dfg.inst_data(Inst::new(ui as u32));
                    if let Some(uv) = user_inst.results.first().copied()
                        && !known.contains_key(uv)
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
fn fold_branches(func: &mut Function, known: &SecondaryMap<Value, ConstValue>) -> bool {
    let mut changed = false;

    for bi in 0..func.dfg.block_count() {
        let block_id = Block::new(bi as u32);
        // 先算出去向与实参（借用结束），再经 Function 的按形式写入口写入以同步 use-lists
        let folded = if let Some((cond, then_block, then_args, else_block, else_args)) =
            func.dfg.term_branch(block_id)
            && let Some(const_val) = known.get(cond)
            && let Some(is_true) = const_val.to_bool()
        {
            // 替换为无条件跳转
            if is_true {
                Some((then_block, then_args.to_vec()))
            } else {
                Some((else_block, else_args.to_vec()))
            }
        } else {
            None
        };
        if let Some((target, args)) = folded {
            func.jump(block_id, target, &args);
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
        let inst = &func.dfg.inst_data(inst_id);
        if matches!(inst.opcode, Opcode::Iconst) {
            inst.immediates
                .first()
                .and_then(|i| i.as_const())
                .and_then(|cid| func.constants.big(cid))
                .and_then(|b| b.try_to_i64())
        } else {
            None
        }
    }

    /// Find an instruction by its result Value and read its constant value.
    fn get_iconst_value_for(func: &Function, value: Value) -> Option<i64> {
        for block in func.dfg.block_data_iter() {
            for &inst_id in &block.inst_order {
                let inst = &func.dfg.inst_data(inst_id);
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
        let module = forge_ir::text::parse_module(src).expect("parse");
        let mut func = module.iter_functions().next().unwrap().clone();
        let pass = ConstFoldPass::new();
        let result = pass.run_on_function(&mut func).unwrap();
        assert!(result.changed, "聚合字面量 extractvalue 应折叠（%c = 2）");
        // %c 应为常量 2
        let def = func.dfg.value_def(Value::new(2)).cloned();
        if let Some(forge_ir::ir::dfg::ValueDef::Inst(iid, _)) = def {
            let inst = &func.dfg.inst_data(iid);
            assert!(
                matches!(inst.opcode, Opcode::Iconst),
                "折叠后应为 Iconst，got {:?}",
                inst.opcode
            );
        }
    }

    #[test]
    fn fold_nested_extractvalue_chain() {
        // backlog #3 复现：**嵌套**聚合常量上的 extractvalue 链 ✗。
        // 实测（2026-10-08 ✗）：第一层提取的结果是"聚合值"✗，fold 直接 `None` ✗
        // ⇒ 第二层看不到常量 ⇒ **整条链不折叠** ✗，`%c` 仍是 ExtractValue 指令 ✗。
        let src = "define i32 @f() {
  %e:
    %a = extractvalue {i32, {i32, i32}} {i32 1, {i32 2, i32 3}}, 1
    %c = extractvalue {i32, i32} %a, 1
    ret i32 %c
}";
        let module = forge_ir::text::parse_module(src).expect("parse");
        let mut func = module.iter_functions().next().unwrap().clone();
        let pass = ConstFoldPass::new();
        let result = pass.run_on_function(&mut func).unwrap();
        assert!(
            result.changed && result.instructions_removed >= 1,
            "嵌套 extractvalue 链应折叠（backlog #3 ✗）：changed={}, removed={}",
            result.changed,
            result.instructions_removed
        );
    }

    #[test]
    fn fold_iadd_constants() {
        let sig = FunctionSignature::new(&[], &[TypeId::I32]);
        let mut b = FunctionBuilder::new("test", TypeContext::new(), sig);
        let entry = b.create_block();
        b.switch_to_block(entry);
        let a = b.constant(3i32);
        let b_val = b.constant(5i32);
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
        let a = b.constant(100i32);
        let b_val = b.constant(200i32);
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
        let c = b.constant(5i32);
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
        let one = b.constant(1i32); // always true (non-zero)
        b.branch(one, then_blk, &[], else_blk, &[]);

        b.switch_to_block(then_blk);
        let v42 = b.constant(42i32);
        b.ret(&[v42]);

        b.switch_to_block(else_blk);
        let v99 = b.constant(99i32);
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
        let else_block = &func.dfg.block(else_blk);
        assert!(
            else_block.inst_order.is_empty() || func.dfg.term_is_unreachable(else_blk),
            "Else block should be cleared (dead)"
        );
    }
    /// 位操作必须保留 i128 的高 64 位（旧实现先 `trunc_to_u64` ⇒ 全丢）。
    #[test]
    fn fold_bitops_keep_i128_high_bits() {
        let v = Big::from_u128((1u128 << 127) | (1u128 << 63) | 1);
        let one = |op: Opcode| {
            fold_opcode(
                &op,
                &[ConstValue::Int(v.clone(), TypeId::I128)],
                &[],
                TypeId::I128,
            )
            .unwrap()
            .unwrap()
            .as_int()
            .unwrap()
            .0
            .trunc_to_u128()
        };
        assert_eq!(one(Opcode::Popcnt), 3, "popcnt 应数到 bit127/63/0");
        assert_eq!(one(Opcode::Clz), 0, "最高位已置位 ⇒ clz = 0");
        assert_eq!(one(Opcode::Ctz), 0, "最低位已置位 ⇒ ctz = 0");
        assert_eq!(
            one(Opcode::Bitreverse),
            (1u128 << 127) | (1u128 << 64) | 1,
            "128 位逐位反转"
        );
        // i128 的 bswap：低字节 1 → 最高字节
        let one_val = |op: Opcode| {
            fold_opcode(
                &op,
                &[ConstValue::Int(Big::from_u64(1), TypeId::I128)],
                &[],
                TypeId::I128,
            )
            .unwrap()
            .unwrap()
            .as_int()
            .unwrap()
            .0
            .trunc_to_u128()
        };
        assert_eq!(one_val(Opcode::Bswap), 1u128 << 120, "16 字节反转");
    }

    /// 饱和算术按**目标位宽**的边界饱和（i128 也正确，旧实现写死 i64）。
    #[test]
    fn fold_saturating_keeps_i128_bounds() {
        let fold2 = |op: Opcode, a: Big, b: Big| {
            fold_opcode(
                &op,
                &[
                    ConstValue::Int(a, TypeId::I128),
                    ConstValue::Int(b, TypeId::I128),
                ],
                &[],
                TypeId::I128,
            )
            .unwrap()
            .unwrap()
            .as_int()
            .unwrap()
            .0
            .trunc_to_u128()
        };
        let one = Big::from_u64(1);
        assert_eq!(
            fold2(Opcode::UaddSat, Big::from_u128(u128::MAX), one.clone()),
            u128::MAX,
            "u128 加法饱和到 2^128-1（不是 u64::MAX）"
        );
        assert_eq!(
            fold2(Opcode::UsubSat, Big::from_u64(0), one.clone()),
            0,
            "无符号减法下溢饱和到 0"
        );
        assert_eq!(
            fold2(Opcode::SaddSat, Big::from_i128(i128::MAX), one.clone()),
            i128::MAX as u128,
            "有符号加法饱和到 i128::MAX"
        );
        assert_eq!(
            fold2(Opcode::SsubSat, Big::from_i128(i128::MIN), one.clone()),
            i128::MIN as u128,
            "有符号减法饱和到 i128::MIN"
        );
    }

    /// 移位量按位宽取模（与后端硬件语义一致），**任意大的常量都要折叠**。
    #[test]
    fn fold_shift_amount_is_masked_like_backend() {
        let one = |amount: Big| {
            fold_opcode(
                &Opcode::Ishl,
                &[
                    ConstValue::Int(Big::from_u64(1), TypeId::I64),
                    ConstValue::Int(amount, TypeId::I64),
                ],
                &[],
                TypeId::I64,
            )
            .unwrap()
            .unwrap()
            .as_int()
            .unwrap()
            .0
            .trunc_to_u128()
        };
        assert_eq!(one(Big::from_u64(64)), 1, "1 << (64 % 64) = 1");
        assert_eq!(one(Big::from_u64(65)), 2, "1 << (65 % 64) = 2");
        // 超大移位量也要能精确取模（旧实现装不下 u64 就放弃折叠）
        let huge = Big::from_u128(u128::MAX);
        // u128::MAX % 64 == 63 ⇒ 1 << 63（i64 视角 = i64::MIN，128 位二补数视角如下）
        assert_eq!(
            one(huge),
            (-9223372036854775808i128) as u128,
            "超大移位量取模后 = 63"
        );
    }

    /// 常量池里的浮点是 **Bits 位模式**：known 表必须按浮点解码；
    /// 折叠结果也必须是 Bits（f32 不能变成 f64 位模式的低 32 位）。
    #[test]
    fn folded_float_constants_use_target_format_bits() {
        let src = "define float @f() {
  %e:
    %r = fadd float 1.5, float 2.5
    ret float %r
}";
        let module = forge_ir::text::parse_module(src).expect("parse");
        let mut func = module.iter_functions().next().unwrap().clone();
        let pass = ConstFoldPass::new();
        pass.run_on_function(&mut func).expect("fold");
        // 找出 Fconst 的定义（折叠后 %r 应为常量 4.0f32）
        let mut found = None;
        for block in func.dfg.block_data_iter() {
            for &inst_id in &block.inst_order {
                let inst = func.dfg.inst_data(inst_id);
                if inst.opcode == Opcode::Fconst
                    && let Some(cid) = inst.immediates.first().and_then(|i| i.as_const())
                {
                    found = func.constants.bits(cid);
                }
            }
        }
        let bv = found.expect("应折叠出 Fconst");
        assert_eq!(bv.width, 32, "float 常量位宽必须是 32（Bits 通道）");
        assert_eq!(
            bv.bits,
            u128::from(4.0f32.to_bits()),
            "1.5f32 + 2.5f32 应折叠成 4.0f32 的位模式"
        );
    }
}
