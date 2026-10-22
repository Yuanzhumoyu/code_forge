//! `Icmp` 的 n 肢字典序比较。
//!
//! **标志一律走 i64 通道**：本 isel 对 `i1` 上的 `and/or` 与 `i1` 的 `Select` 不可靠
//!（IR 形状正确、JIT 值错，实测），所以每个布尔都 `zext` 成 `i64` 再组合，末尾用一条
//! `icmp ne 0` 还原成 `i1`。

use super::*;
use forge_ir::IntCC;

/// 有符号条件 → 其无符号等价（低位肢是量值，只有顶肢才带符号语义）。
pub(super) fn unsigned_of(cc: IntCC) -> IntCC {
    use IntCC::*;
    match cc {
        Slt => Ult,
        Sle => Ule,
        Sgt => Ugt,
        Sge => Uge,
        other => other,
    }
}

/// 字典序比较：`Eq/Ne` 全部肢相等；`< <= > >=` 从顶肢往下定胜负。
pub(super) fn compare(
    e: &mut LimbEmitter<'_>,
    anchor: Inst,
    a: &Limbs,
    b: &Limbs,
    cc: IntCC,
    w: u32,
    bits: u32,
) -> Value {
    let n = limb_count(bits, w);
    let t = e.limb_ty(w);
    let zero = e.iconst_in(anchor, 0, w);
    let eq_of = |e: &mut LimbEmitter<'_>, k: usize| -> Value {
        let eq = e.emit_before(
            anchor,
            Opcode::Icmp,
            &[a[k], b[k]],
            &[Immediate::IntCC(IntCC::Eq)],
            TypeId::BOOL,
        );
        e.zext_bool(anchor, eq, w)
    };

    if matches!(cc, IntCC::Eq | IntCC::Ne) {
        let mut acc = eq_of(e, 0);
        for k in 1..n {
            let f = eq_of(e, k);
            acc = e.emit_before(anchor, Opcode::Band, &[acc, f], &[], t);
        }
        let fin = if cc == IntCC::Eq {
            IntCC::Ne
        } else {
            IntCC::Eq
        };
        return e.emit_before(
            anchor,
            Opcode::Icmp,
            &[acc, zero],
            &[Immediate::IntCC(fin)],
            TypeId::BOOL,
        );
    }

    // **严格大于**（多肢）：`OR_k (eq_higher(k) & (a_k > b_k))`。
    //
    // v27 M6-75 修：这里原来把**带等号的条件码**（`Uge`/`Sge`…）直接用在顶肢上，
    // 于是 `5u128 >= 7u128` 被算成真（顶肢 0 >= 0 成立就"决定"了结果）——典型的多肢比较
    // 错误：**相等不能决定大小**。现在每肢一律用**严格**比较，需要 `>=`/`<=` 时再与
    // "全肢相等"组合，`<`/`<=` 用 `!(>)`/`!(>=)` 取反（全序，安全）。
    // **规范关系一律取"严格大于"**（符号性跟随原条件码），再由它派生其余四种：
    //   `>` ⇒ gt ｜ `>=` ⇒ gt|eq ｜ `<` ⇒ !(gt|eq) ｜ `<=` ⇒ !gt
    // 不能"取请求方向的严格版"——那会让 `<` 走成 `!(a<b 或 相等)` 恒假（实测第 0 个用例红）。
    let signed = matches!(cc, IntCC::Sgt | IntCC::Sge | IntCC::Slt | IntCC::Sle);
    let strict = if signed { IntCC::Sgt } else { IntCC::Ugt };
    let mut gt: Option<Value> = None; // 严格大于（i64 标志 0/1）
    let mut alleq: Option<Value> = None; // 更高肢是否全相等
    for k in (0..n).rev() {
        // 顶肢用原符号性（严格条件），其余肢用无符号等价
        let limb_cc = if k == n - 1 {
            strict
        } else {
            unsigned_of(strict)
        };
        let ck = e.emit_before(
            anchor,
            Opcode::Icmp,
            &[a[k], b[k]],
            &[Immediate::IntCC(limb_cc)],
            TypeId::BOOL,
        );
        let fk = e.zext_bool(anchor, ck, w);
        let feq = eq_of(e, k);
        gt = Some(match (gt, alleq) {
            (None, None) => fk,
            (Some(r), Some(q)) => {
                let term = e.emit_before(anchor, Opcode::Band, &[q, fk], &[], t);
                e.emit_before(anchor, Opcode::Bor, &[r, term], &[], t)
            }
            _ => unreachable!(),
        });
        alleq = Some(match alleq {
            None => feq,
            Some(q) => e.emit_before(anchor, Opcode::Band, &[q, feq], &[], t),
        });
    }
    let gt_v = gt.expect("n ≥ 1");
    let eq_all = alleq.expect("n ≥ 1");
    let out = match cc {
        IntCC::Sgt | IntCC::Ugt => gt_v,
        IntCC::Sge | IntCC::Uge => e.emit_before(anchor, Opcode::Bor, &[gt_v, eq_all], &[], t),
        IntCC::Slt | IntCC::Ult => {
            // a < b ⟺ !(a >= b)
            let o = e.emit_before(anchor, Opcode::Bor, &[gt_v, eq_all], &[], t);
            let nz = e.emit_before(
                anchor,
                Opcode::Icmp,
                &[o, zero],
                &[Immediate::IntCC(IntCC::Eq)],
                TypeId::BOOL,
            );
            return nz;
        }
        IntCC::Sle | IntCC::Ule => {
            // a <= b ⟺ !(a > b)
            let nz = e.emit_before(
                anchor,
                Opcode::Icmp,
                &[gt_v, zero],
                &[Immediate::IntCC(IntCC::Eq)],
                TypeId::BOOL,
            );
            return nz;
        }
        _ => unreachable!("Eq/Ne 已在上面处理"),
    };
    e.emit_before(
        anchor,
        Opcode::Icmp,
        &[out, zero],
        &[Immediate::IntCC(IntCC::Ne)],
        TypeId::BOOL,
    )
}
