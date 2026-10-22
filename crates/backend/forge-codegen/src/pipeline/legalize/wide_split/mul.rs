//! 宽值乘法：**子肢梳状**（任意肢数 n）。
//!
//! 关键点：取子肢宽度 `s = W/2`，两个 `s` 位子肢之积 < `2^(2s) = 2^W`，所以一条 `imul`
//! 的结果寄存器里**同时**装着低半与高半（`lo = p & mask_s`、`hi = p >> s`）——
//! 不需要"乘法高位"指令，也不需要 libcall。
//!
//! 复杂度 `O(n²)`：把 a、b 各展开成 `2n` 个子肢，按子肢下标做卷积，逐输出子肢累加
//! （进位用 `acc >> s` 逐次 flush），最后把相邻两个子肢拼回一个 `W` 位肢。

use super::arith::{normalize_top, top_valid_bits};
use super::*;

/// `Imul`：`a * b`，wrapping 到 `bits` 位。
pub(super) fn mul_limbs(
    e: &mut LimbEmitter<'_>,
    anchor: Inst,
    a: &Limbs,
    b: &Limbs,
    w: u32,
    bits: u32,
) -> Limbs {
    let n = limb_count(bits, w);
    let ty = e.limb_ty(w);
    let s = w / 2; // 子肢宽度
    let m_s = mask_of(s);
    let m_c = e.iconst_in(anchor, m_s, w);
    let sh_s = e.iconst_in(anchor, s as u64, w);
    let zero = e.iconst_in(anchor, 0, w);

    // 每个肢拆成 (低子肢, 高子肢)，拼成 2n 项的子肢数组
    let mut sa: smallvec::SmallVec<[Value; 8]> = smallvec::SmallVec::new();
    let mut sb: smallvec::SmallVec<[Value; 8]> = smallvec::SmallVec::new();
    for k in 0..n {
        let (alo, ahi) = (
            e.emit_before(anchor, Opcode::Band, &[a[k], m_c], &[], ty),
            e.emit_before(anchor, Opcode::Ushr, &[a[k], sh_s], &[], ty),
        );
        sa.push(alo);
        sa.push(ahi);
        let (blo, bhi) = (
            e.emit_before(anchor, Opcode::Band, &[b[k], m_c], &[], ty),
            e.emit_before(anchor, Opcode::Ushr, &[b[k], sh_s], &[], ty),
        );
        sb.push(blo);
        sb.push(bhi);
    }

    // 逐输出子肢累加（只保留低 2n 个子肢 = 结果的 n 个肢）
    let sn = 2 * n;
    let mut carry = zero;
    let mut out_subs: smallvec::SmallVec<[Value; 8]> = smallvec::SmallVec::new();
    for t in 0..sn {
        let mut acc = carry;
        for i in 0..sn {
            // p_lo: i + j == t；p_hi: i + j + 1 == t
            for (j, take_hi) in [
                (t.checked_sub(i), false),
                (t.checked_sub(i).and_then(|x| x.checked_sub(1)), true),
            ] {
                let Some(j) = j else { continue };
                if j >= sn {
                    continue;
                }
                let p = e.emit_before(anchor, Opcode::Imul, &[sa[i], sb[j]], &[], ty);
                let term = if take_hi {
                    e.emit_before(anchor, Opcode::Ushr, &[p, sh_s], &[], ty)
                } else {
                    e.emit_before(anchor, Opcode::Band, &[p, m_c], &[], ty)
                };
                acc = e.emit_before(anchor, Opcode::Iadd, &[acc, term], &[], ty);
            }
        }
        // c_s[t] = acc & mask_s；进位 = acc >> s
        out_subs.push(e.emit_before(anchor, Opcode::Band, &[acc, m_c], &[], ty));
        carry = e.emit_before(anchor, Opcode::Ushr, &[acc, sh_s], &[], ty);
    }

    // 相邻两个子肢拼回一个 W 位肢
    let mut out = Limbs::new();
    for k in 0..n {
        let hi = e.emit_before(anchor, Opcode::Ishl, &[out_subs[2 * k + 1], sh_s], &[], ty);
        out.push(e.emit_before(anchor, Opcode::Bor, &[out_subs[2 * k], hi], &[], ty));
    }

    // 顶肢规范化（乘积可能写进顶肢无效应位）
    let valid = top_valid_bits(bits, w);
    if valid < w {
        let top = out[n - 1];
        out[n - 1] = normalize_top(e, anchor, top, valid, w);
    }
    out
}
