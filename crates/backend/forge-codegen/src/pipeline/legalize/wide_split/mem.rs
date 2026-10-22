//! 宽值的 `Load` / `Store`：n 次窄存取。
//!
//! 地址偏移常量必须是 **PTR 类型**（`i64` 常量与 `ptr` 相加会生成 `add ptr, i64`，
//! 实测地址算错）。

use super::*;

/// `%r = load i<bits>, ptr %p` → n 条 `load i<W>`。
pub(super) fn load_limbs(
    e: &mut LimbEmitter<'_>,
    anchor: Inst,
    ptr: Value,
    w: u32,
    bits: u32,
) -> Limbs {
    let n = limb_count(bits, w);
    let ty = e.limb_ty(w);
    let mut out = Limbs::new();
    for k in 0..n {
        let p = if k == 0 {
            ptr
        } else {
            let off = e.iconst_ptr_in(anchor, (k as u32 * (w / 8)) as u64, w);
            e.ptr_add(anchor, ptr, off)
        };
        out.push(e.emit_before(anchor, Opcode::Load, &[p], &[], ty));
    }
    out
}

/// `store i<bits> %v, ptr %p` → n 条 `store i<W>`。
///
/// 把**原指令就地改写**成"存最后一个肢"，其余肢的存插在它之前 —— IR 没有"删除指令"
/// 的语义，这样不需要移除任何指令。
pub(super) fn store_limbs(
    e: &mut LimbEmitter<'_>,
    anchor: Inst,
    v: &Limbs,
    ptr: Value,
    w: u32,
    bits: u32,
) {
    let n = limb_count(bits, w);
    let ty = e.limb_ty(w);
    let mut addrs = Limbs::new();
    for k in 0..n {
        let p = if k == 0 {
            ptr
        } else {
            let off = e.iconst_ptr_in(anchor, (k as u32 * (w / 8)) as u64, w);
            e.ptr_add(anchor, ptr, off)
        };
        addrs.push(p);
    }
    // 前 n-1 个肢：新插 store（锚点之前，顺序保持）
    for k in 0..n - 1 {
        let _ = e.emit_before_store(anchor, v[k], addrs[k]);
    }
    // 最后一个肢：就地改写原 store
    e.rewrite_in_place(anchor, Opcode::Store, &[v[n - 1], addrs[n - 1]]);
    let _ = ty;
}
