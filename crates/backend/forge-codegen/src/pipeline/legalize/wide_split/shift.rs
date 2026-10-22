//! 移位：**常量移位量**的 O(n) 快路径。
//!
//! 运行期移位量（O(n²) select 链）在 P1.5 补；在此之前出现运行期量 ⇒ 整个函数回退给
//! `wide_mem`（见 `mod.rs` 的可行性判断），不会静默算错。
//!
//! 语义按 `docs/reference/ir-int-semantics.md`：`amt >= bits` ⇒ `shl/ushr = 0`、
//! `sshr` = 全符号位。

use super::arith::{normalize_top, top_valid_bits};
use super::*;

/// 全 0 或全符号位（`sshr` 越界时的填充）。
fn fill_limbs(
    e: &mut LimbEmitter<'_>,
    anchor: Inst,
    a: &Limbs,
    n: usize,
    w: u32,
    sign_fill: bool,
) -> Limbs {
    let mut out = Limbs::new();
    let v = if sign_fill {
        // 顶肢算术右移 W-1 位 = 全 1（负）或全 0（非负）
        let sh = e.iconst_in(anchor, (w - 1) as u64, w);
        let ty = e.limb_ty(w);
        e.emit_before(anchor, Opcode::Sshr, &[a[n - 1], sh], &[], ty)
    } else {
        e.iconst_in(anchor, 0, w)
    };
    for _ in 0..n {
        out.push(v);
    }
    out
}

/// 常量移位量 `amt` 的 `Ishl`/`Ushr`/`Sshr`。
pub(super) fn shift_const(
    e: &mut LimbEmitter<'_>,
    anchor: Inst,
    a: &Limbs,
    amt: u64,
    op: Opcode,
    w: u32,
    bits: u32,
) -> Limbs {
    let n = limb_count(bits, w);
    let ty = e.limb_ty(w);
    let is_shl = op == Opcode::Ishl;
    let is_sshr = op == Opcode::Sshr;

    if amt >= bits as u64 {
        return fill_limbs(e, anchor, a, n, w, is_sshr);
    }
    let k = (amt / w as u64) as usize; // 整肢位移
    let r = (amt % w as u64) as u32; // 肢内位移
    let zero = e.iconst_in(anchor, 0, w);

    let mut out = Limbs::new();
    for i in 0..n {
        let mut acc: Option<Value> = None;
        let mut push = |e: &mut LimbEmitter<'_>, v: Value| {
            acc = Some(match acc {
                None => v,
                Some(prev) => e.emit_before(anchor, Opcode::Bor, &[prev, v], &[], ty),
            });
        };

        if is_shl {
            // out[i] = a[i-k] << r | a[i-k-1] >> (W-r)
            if i >= k {
                let j = i - k;
                let main = if r == 0 {
                    a[j]
                } else {
                    let rs = e.iconst_in(anchor, r as u64, w);
                    e.emit_before(anchor, Opcode::Ishl, &[a[j], rs], &[], ty)
                };
                push(e, main);
                if r != 0 && j > 0 {
                    let wmr = e.iconst_in(anchor, (w - r) as u64, w);
                    let cross = e.emit_before(anchor, Opcode::Ushr, &[a[j - 1], wmr], &[], ty);
                    push(e, cross);
                }
            }
        } else {
            // out[i] = a[i+k] >> r | a[i+k+1] << (W-r)（sshr 越界侧补符号）
            if i + k < n {
                let j = i + k;
                let main = if r == 0 {
                    a[j]
                } else {
                    let rs = e.iconst_in(anchor, r as u64, w);
                    e.emit_before(anchor, Opcode::Ushr, &[a[j], rs], &[], ty)
                };
                push(e, main);
                if r != 0 {
                    let wmr = e.iconst_in(anchor, (w - r) as u64, w);
                    let cross = if j + 1 < n {
                        e.emit_before(anchor, Opcode::Ishl, &[a[j + 1], wmr], &[], ty)
                    } else if is_sshr {
                        // 符号填充：全 1/全 0 左移 (W-r) 位
                        let sh = e.iconst_in(anchor, (w - 1) as u64, w);
                        let sign = e.emit_before(anchor, Opcode::Sshr, &[a[n - 1], sh], &[], ty);
                        e.emit_before(anchor, Opcode::Ishl, &[sign, wmr], &[], ty)
                    } else {
                        zero
                    };
                    push(e, cross);
                }
            } else if is_sshr && r == 0 {
                // 整肢越界且无肢内位移：整肢都是符号位
                let sh = e.iconst_in(anchor, (w - 1) as u64, w);
                let sign = e.emit_before(anchor, Opcode::Sshr, &[a[n - 1], sh], &[], ty);
                push(e, sign);
            } else if is_sshr {
                // 越界但有肢内位移：值 = 符号位左移 (W-r) 位再补高位符号位
                let wmr = e.iconst_in(anchor, (w - r) as u64, w);
                let sh = e.iconst_in(anchor, (w - 1) as u64, w);
                let sign = e.emit_before(anchor, Opcode::Sshr, &[a[n - 1], sh], &[], ty);
                let cross = e.emit_before(anchor, Opcode::Ishl, &[sign, wmr], &[], ty);
                let hi = e.emit_before(anchor, Opcode::Sshr, &[sign, sh], &[], ty);
                push(e, hi);
                push(e, cross);
            }
        }
        out.push(acc.unwrap_or(zero));
    }

    // 左移可能把进位写进顶肢无效应位
    let valid = top_valid_bits(bits, w);
    if is_shl && valid < w {
        let top = out[n - 1];
        out[n - 1] = normalize_top(e, anchor, top, valid, w);
    }
    out
}

/// **运行期移位量**的 `Ishl`/`Ushr`/`Sshr`：整肢量 `k` + 肢内量 `r` 都在运行时算，
/// 每个输出肢对每个源肢用 `select(k == |i-j|, term, 0)` 串成 `or` 链（O(n²)）。
///
/// `amt >= bits` 的定值行为：`shl/ushr ⇒ 0`、`sshr ⇒ 全符号位`。
pub(super) fn shift_runtime(
    e: &mut LimbEmitter<'_>,
    anchor: Inst,
    a: &Limbs,
    amt: &Limbs,
    op: Opcode,
    w: u32,
    bits: u32,
) -> Limbs {
    let n = limb_count(bits, w);
    let ty = e.limb_ty(w);
    let is_shl = op == Opcode::Ishl;
    let is_sshr = op == Opcode::Sshr;
    let zero = e.iconst_in(anchor, 0, w);
    let wc = e.iconst_in(anchor, w as u64, w);
    let bits_c = e.iconst_in(anchor, bits as u64, w);
    let bm1 = e.iconst_in(anchor, (bits - 1) as u64, w);

    // ① 高位肢非 0 ⇒ 移位量 ≥ 2^W > bits（直接判溢出）
    let mut hi_nz: Option<Value> = None;
    for k in 1..amt.len() {
        let nz = e.emit_before(
            anchor,
            Opcode::Icmp,
            &[amt[k], zero],
            &[Immediate::IntCC(IntCC::Ne)],
            TypeId::BOOL,
        );
        hi_nz = Some(match hi_nz {
            None => nz,
            Some(prev) => {
                // 布尔 OR 走 i64 通道，最后再比 0
                let (p64, n64) = (e.zext_bool(anchor, prev, w), e.zext_bool(anchor, nz, w));
                let both = e.emit_before(anchor, Opcode::Bor, &[p64, n64], &[], ty);
                e.emit_before(
                    anchor,
                    Opcode::Icmp,
                    &[both, zero],
                    &[Immediate::IntCC(IntCC::Ne)],
                    TypeId::BOOL,
                )
            }
        });
    }
    let a_lo = amt[0];
    let ge = e.emit_before(
        anchor,
        Opcode::Icmp,
        &[a_lo, bits_c],
        &[Immediate::IntCC(IntCC::Uge)],
        TypeId::BOOL,
    );
    let ovf = match hi_nz {
        None => ge,
        Some(h) => {
            let (h64, g64) = (e.zext_bool(anchor, h, w), e.zext_bool(anchor, ge, w));
            let both = e.emit_before(anchor, Opcode::Bor, &[h64, g64], &[], ty);
            e.emit_before(
                anchor,
                Opcode::Icmp,
                &[both, zero],
                &[Immediate::IntCC(IntCC::Ne)],
                TypeId::BOOL,
            )
        }
    };

    // ② 夹到 bits-1 后分解成整肢量 k 与肢内量 r
    let small = e.emit_before(
        anchor,
        Opcode::Icmp,
        &[a_lo, bits_c],
        &[Immediate::IntCC(IntCC::Ult)],
        TypeId::BOOL,
    );
    let amt_c = e.select(anchor, small, a_lo, bm1, ty);
    let k = e.emit_before(anchor, Opcode::Udiv, &[amt_c, wc], &[], ty);
    let r = e.emit_before(anchor, Opcode::Urem, &[amt_c, wc], &[], ty);
    let wmr = e.emit_before(anchor, Opcode::Isub, &[wc, r], &[], ty);
    let r_is0 = e.emit_before(
        anchor,
        Opcode::Icmp,
        &[r, zero],
        &[Immediate::IntCC(IntCC::Eq)],
        TypeId::BOOL,
    );

    // ③ 符号填充值（sshr 用）：顶肢算术右移 W-1 位
    let sign_fill = {
        let sh = e.iconst_in(anchor, (w - 1) as u64, w);
        e.emit_before(anchor, Opcode::Sshr, &[a[n - 1], sh], &[], ty)
    };

    let mut out = Limbs::new();
    for i in 0..n {
        let mut acc: Option<Value> = None;
        for j in 0..n {
            // shl：k == i-j；shr：k == j-i
            let d: Option<usize> = if is_shl {
                i.checked_sub(j)
            } else {
                j.checked_sub(i)
            };
            let Some(d) = d else { continue };
            let d_c = e.iconst_in(anchor, d as u64, w);
            let eqk = e.emit_before(
                anchor,
                Opcode::Icmp,
                &[k, d_c],
                &[Immediate::IntCC(IntCC::Eq)],
                TypeId::BOOL,
            );
            // 主项：a[j] << r / a[j] >> r（r == 0 时就是原值，移位不会错）
            let main = if is_shl {
                e.emit_before(anchor, Opcode::Ishl, &[a[j], r], &[], ty)
            } else {
                e.emit_before(anchor, Opcode::Ushr, &[a[j], r], &[], ty)
            };
            // 跨肢项：shl 取 a[j-1] >> (W-r)；shr 取 a[j+1] << (W-r)（越界侧 sshr 补符号）
            let cross_src: Option<Value> = if is_shl {
                if j == 0 { None } else { Some(a[j - 1]) }
            } else if j + 1 < n {
                Some(a[j + 1])
            } else if is_sshr {
                Some(sign_fill)
            } else {
                None
            };
            let cross = match cross_src {
                None => zero,
                Some(src) => {
                    let raw = if is_shl {
                        e.emit_before(anchor, Opcode::Ushr, &[src, wmr], &[], ty)
                    } else {
                        e.emit_before(anchor, Opcode::Ishl, &[src, wmr], &[], ty)
                    };
                    // r == 0 时跨肢项必须是 0（否则会发出"移位 W 位"）
                    e.select(anchor, r_is0, zero, raw, ty)
                }
            };
            let term = e.emit_before(anchor, Opcode::Bor, &[main, cross], &[], ty);
            let picked = e.select(anchor, eqk, term, zero, ty);
            acc = Some(match acc {
                None => picked,
                Some(prev) => e.emit_before(anchor, Opcode::Bor, &[prev, picked], &[], ty),
            });
        }
        let v = acc.unwrap_or(zero);
        // 溢出兜底：shl/ushr ⇒ 0；sshr ⇒ 全符号位
        let fill = if is_sshr { sign_fill } else { zero };
        out.push(e.select(anchor, ovf, fill, v, ty));
    }

    // 左移可能把进位写进顶肢无效应位
    let valid = top_valid_bits(bits, w);
    if is_shl && valid < w {
        let top = out[n - 1];
        out[n - 1] = normalize_top(e, anchor, top, valid, w);
    }
    out
}
