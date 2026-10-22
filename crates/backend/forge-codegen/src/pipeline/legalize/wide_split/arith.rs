//! 算术与位运算的肢级改写：`Iconst` / `Iadd` / `Isub` / 位运算 / `Select` /
//! `Ireduce` / `Sextend` / `Zextend`。
//!
//! 全部按"n 肢循环"写：`n = 2` 时即经典的两字（i128）形态。

use super::*;

/// 顶肢的有效位数（位宽不是字宽整数倍时 < `W`）。
pub(super) fn top_valid_bits(bits: u32, w: u32) -> u32 {
    let r = bits % w;
    if r == 0 { w } else { r }
}

/// 顶肢规范化：位宽不是字宽整数倍时，清掉顶肢的无效应位。
///
/// **为什么必要**：`Iadd`/`Ishl`/`Imul` 的进位或溢出会写进顶肢的高位，那些位不属于这个
/// 宽值；不清掉的话，后续比较/移位会把垃圾当真值。
pub(super) fn normalize_top(
    e: &mut LimbEmitter<'_>,
    anchor: Inst,
    v: Value,
    valid: u32,
    w: u32,
) -> Value {
    if valid >= w {
        return v;
    }
    let mask = e.iconst_in(anchor, mask_of(valid), w);
    e.emit_before(anchor, Opcode::Band, &[v, mask], &[], e.limb_ty(w))
}

/// 任意宽度常量 → n 肢。≤128 位走整数常量池；>128 位走 `Big`（反复 `udiv` 取肢）。
pub(super) fn const_limbs(
    e: &mut LimbEmitter<'_>,
    anchor: Inst,
    cid: ConstId,
    w: u32,
    n: usize,
) -> Limbs {
    let mut out = Limbs::new();
    if let Some(bv) = e.func.constants.bits(cid) {
        let mut cur = bv.bits;
        for _ in 0..n {
            out.push(e.iconst_in(anchor, (cur & mask_of(w) as u128) as u64, w));
            cur >>= w.min(127);
        }
        return out;
    }
    let Some(big) = e.func.constants.big(cid).cloned() else {
        // 常量池里既不是 int 也不是 big：交给上层 fail-closed（理论上不可达）
        for _ in 0..n {
            out.push(e.iconst_in(anchor, 0, w));
        }
        return out;
    };
    let two_w = pow2_big(w);
    let mut cur = big;
    for _ in 0..n {
        out.push(e.iconst_in(anchor, cur.trunc_to_u64(), w));
        cur = cur.udiv(&two_w).unwrap_or_else(|_| Big::from_u64(0));
    }
    out
}

/// `2^w` 的 `Big`（`w ≤ 64`）。
fn pow2_big(w: u32) -> Big {
    if w >= 64 {
        Big::from_i128(1i128 << 64)
    } else {
        Big::from_i64(1i64 << w)
    }
}

/// `Iadd` / `Isub`：逐肢相加 + 进位（借位）链。
///
/// 进位只需 `icmp Ult`（无符号比较），因此**不需要** "add with carry" 这种新 opcode。
pub(super) fn add_sub(
    e: &mut LimbEmitter<'_>,
    anchor: Inst,
    a: &Limbs,
    b: &Limbs,
    w: u32,
    bits: u32,
    add: bool,
) -> Limbs {
    let n = limb_count(bits, w);
    let op = if add { Opcode::Iadd } else { Opcode::Isub };
    let ty = e.limb_ty(w);
    let mut out = Limbs::new();
    let mut carry: Option<Value> = None; // None = 0
    for k in 0..n {
        let s = e.emit_before(anchor, op, &[a[k], b[k]], &[], ty);
        // add: 进位 = s < a[k]；sub: 借位 = a[k] < b[k]
        let (x, y) = if add { (s, a[k]) } else { (a[k], b[k]) };
        let c = e.emit_before(
            anchor,
            Opcode::Icmp,
            &[x, y],
            &[Immediate::IntCC(IntCC::Ult)],
            TypeId::BOOL,
        );
        let with_carry = match carry {
            None => s,
            Some(prev) => {
                let cz = e.emit_before(anchor, Opcode::Uextend, &[prev], &[], ty);
                e.emit_before(anchor, op, &[s, cz], &[], ty)
            }
        };
        out.push(with_carry);
        carry = Some(c);
    }
    // 顶肢规范化（进位可能落到无效应位）
    let valid = top_valid_bits(bits, w);
    if valid < w {
        let top = out[n - 1];
        out[n - 1] = normalize_top(e, anchor, top, valid, w);
    }
    out
}

/// `Band` / `Bor` / `Bxor`：逐肢。
pub(super) fn bitwise(
    e: &mut LimbEmitter<'_>,
    anchor: Inst,
    a: &Limbs,
    b: &Limbs,
    w: u32,
    bits: u32,
    op: Opcode,
) -> Limbs {
    let n = limb_count(bits, w);
    let ty = e.limb_ty(w);
    let mut out = Limbs::new();
    for k in 0..n {
        out.push(e.emit_before(anchor, op, &[a[k], b[k]], &[], ty));
    }
    out
}

/// `Select`：逐肢复制同一个条件。
pub(super) fn select_wide(
    e: &mut LimbEmitter<'_>,
    anchor: Inst,
    cond: Value,
    a: &Limbs,
    b: &Limbs,
    w: u32,
    bits: u32,
) -> Limbs {
    let n = limb_count(bits, w);
    let ty = e.limb_ty(w);
    let mut out = Limbs::new();
    for k in 0..n {
        out.push(e.select(anchor, cond, a[k], b[k], ty));
    }
    out
}

/// 宽值 → 窄值：取低肢，再按目标宽度截断。
pub(super) fn reduce_narrow(
    e: &mut LimbEmitter<'_>,
    anchor: Inst,
    a: &Limbs,
    w: u32,
    to_ty: TypeId,
) -> Value {
    let to_bits = e.func.types.borrow().scalar_bits(to_ty).unwrap_or(w);
    if to_bits >= w {
        return a[0];
    }
    e.emit_before(anchor, Opcode::Ireduce, &[a[0]], &[], to_ty)
}

/// 窄值 → 宽值（`Sextend` / `Zextend`）：低肢扩展，其余肢填符号位或 0。
pub(super) fn ext_from_narrow(
    e: &mut LimbEmitter<'_>,
    anchor: Inst,
    x: Value,
    w: u32,
    n: usize,
    signed: bool,
    bits: u32,
) -> Limbs {
    let ty = e.limb_ty(w);
    let lo = e.emit_before(
        anchor,
        if signed {
            Opcode::Sextend
        } else {
            Opcode::Uextend
        },
        &[x],
        &[],
        ty,
    );
    let mut out = Limbs::new();
    out.push(lo);
    if n > 1 {
        let fill = if signed {
            // 算术右移 W-1 位 ⇒ 符号位全 1 或全 0
            let sh = e.iconst_in(anchor, (w - 1) as u64, w);
            e.emit_before(anchor, Opcode::Sshr, &[lo, sh], &[], ty)
        } else {
            e.iconst_in(anchor, 0, w)
        };
        for _ in 1..n {
            out.push(fill);
        }
    }
    // 符号扩展可能写进顶肢无效应位（只有 sext 会）——统一规范化
    let valid = top_valid_bits(bits, w);
    if signed && valid < w {
        let top = out[n - 1];
        out[n - 1] = normalize_top(e, anchor, top, valid, w);
    }
    out
}

/// `Bnot`：逐肢按位取反（`mod 2^bits` 下 `!x = -x-1`，逐肢取反即可）。
pub(super) fn not_limbs(
    e: &mut LimbEmitter<'_>,
    anchor: Inst,
    a: &Limbs,
    w: u32,
    bits: u32,
) -> Limbs {
    let n = limb_count(bits, w);
    let ty = e.limb_ty(w);
    let mut out = Limbs::new();
    for k in 0..n {
        out.push(e.emit_before(anchor, Opcode::Bnot, &[a[k]], &[], ty));
    }
    // 顶肢规范化（取反会把无效应位置 1）
    let valid = top_valid_bits(bits, w);
    if valid < w {
        let top = out[n - 1];
        out[n - 1] = normalize_top(e, anchor, top, valid, w);
    }
    out
}
