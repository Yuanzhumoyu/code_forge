//! 宽值的位计数：`Clz` / `Ctz`（`Ctpop` 在 IR 里是 SWAR 序列，不走这里）。
//!
//! 语义按 `docs/reference/ir-int-semantics.md`：**按值的位宽计数**（例如 `i100` 的
//! `clz` 数的是 100 位里的前导零）。计数值本身必然装得进一个肢（≤ bits ≤ n·W）。
//!
//! 结果宽度由指令的结果类型决定：窄结果（如内建返回的 `u32`）⇒ `Ireduce`；
//! 宽结果（`Clz i128 → i128`）⇒ 由调用方用 [`count_limbs`] 拼成 n 肢。

use super::*;

/// 在肢上算出计数值（类型 = 肢宽 `i<W>`）。
pub(super) fn count_acc(
    e: &mut LimbEmitter<'_>,
    anchor: Inst,
    a: &Limbs,
    w: u32,
    bits: u32,
    leading: bool,
) -> Value {
    let n = limb_count(bits, w);
    let ty = e.limb_ty(w);
    let count_op = if leading { Opcode::Clz } else { Opcode::Ctz };

    let mut acc = if leading {
        e.emit_before(anchor, count_op, &[a[n - 1]], &[], ty)
    } else {
        e.emit_before(anchor, count_op, &[a[0]], &[], ty)
    };

    for k in 1..n {
        let base = (k as u64) * (w as u64);
        let base_v = e.iconst_in(anchor, base, w);
        // 已跨过的 k 个肢是否全零：`acc == k*W` 正好表示这件事。
        let all_zero = e.emit_before(
            anchor,
            Opcode::Icmp,
            &[acc, base_v],
            &[Immediate::IntCC(IntCC::Eq)],
            TypeId::BOOL,
        );
        let idx = if leading { n - 1 - k } else { k };
        let ck = e.emit_before(anchor, count_op, &[a[idx]], &[], ty);
        let ck_plus = e.emit_before(anchor, Opcode::Iadd, &[ck, base_v], &[], ty);
        acc = e.select(anchor, all_zero, ck_plus, acc, ty);
    }
    acc
}

/// 窄结果：计数 → （必要时）截到结果宽度。
pub(super) fn count_scalar(
    e: &mut LimbEmitter<'_>,
    anchor: Inst,
    a: &Limbs,
    w: u32,
    bits: u32,
    leading: bool,
    rt: TypeId,
) -> Value {
    let acc = count_acc(e, anchor, a, w, bits, leading);
    let rt_bits = e.func.types.borrow().scalar_bits(rt).unwrap_or(w);
    if rt_bits >= w {
        acc
    } else {
        e.emit_before(anchor, Opcode::Ireduce, &[acc], &[], rt)
    }
}

/// 宽结果：`[count, 0, 0, …]`（计数值只占最低肢）。
pub(super) fn count_limbs(
    e: &mut LimbEmitter<'_>,
    anchor: Inst,
    a: &Limbs,
    w: u32,
    bits: u32,
    leading: bool,
) -> Limbs {
    let acc = count_acc(e, anchor, a, w, bits, leading);
    let mut out = Limbs::new();
    out.push(acc);
    for _ in 1..limb_count(bits, w) {
        out.push(e.iconst_in(anchor, 0, w));
    }
    out
}
