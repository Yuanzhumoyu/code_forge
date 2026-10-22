//! 聚合常量字节打包（第二十七轮从 compiler.rs 拆出——纯函数组,无编译器
//! 状态依赖;供 Stage 0.5 聚合展开阶段的 Store/Call 常量参数打包使用）。

use forge_ir::Endianness;
use forge_ir::{AggConst, ConstantPool, Function, IrError, TypeStore};

/// 把 `bytes[off .. off+len]` 按**小端**打包成一个整数（`len ≤ 8`）。
///
/// v23 M6 去重：段值/`≤8` 字节聚合常量参数的打包原来在 4 处各写一遍（`agg_call.rs` 两份、
/// `agg_store.rs` 两份），其中一半用 `get(..).unwrap_or(0)` 越界补零、一半直接下标——
/// **口径只有这一份**：越界按 0 补（下标那种在 `len` 不越界时结果相同，是它的特例）。
pub fn pack_segment(bytes: &[u8], off: usize, len: usize) -> u64 {
    debug_assert!(len <= 8, "段值最多 8 字节（{len}）");
    let mut v: u64 = 0;
    for k in 0..len {
        v |= (bytes.get(off + k).copied().unwrap_or(0) as u64) << (k * 8);
    }
    v
}

/// 把聚合常量打包为**目标端序**的字节序列(按类型布局含 padding)。
///
/// 端序取自 `TypeStore` 的 `DataLayout`（v23 M3 修：此前这里**写死小端**，
/// 大端目标上聚合常量的标量字段会整字节反序）。
pub fn pack_agg_bytes(func: &Function, agg: &AggConst) -> Result<Vec<u8>, IrError> {
    let ts = func.types.borrow();
    let size = ts.size_bytes(agg.ty) as usize;
    let mut out = vec![0u8; size];
    pack_agg_into_bytes(&func.constants, &ts, agg, 0, &mut out)?;
    Ok(out)
}

/// 递归打包(offset 定位到目标缓冲区)。
pub fn pack_agg_into_bytes(
    cp: &ConstantPool,
    ts: &TypeStore,
    agg: &AggConst,
    base: usize,
    out: &mut [u8],
) -> Result<(), IrError> {
    let big = matches!(ts.data_layout.endianness, Endianness::Big);
    for (i, child) in agg.children.iter().enumerate() {
        let fty = ts.aggregate_elem_type(agg.ty, i as u32).ok_or_else(|| {
            IrError::Internal(format!("聚合常量打包：字段 {i} 越界（type {:?}）", agg.ty))
        })?;
        let off = base + ts.field_offset(agg.ty, i as u32).unwrap_or(0) as usize;
        // 子节点是**统一句柄**：按 tag 自行分派（标量打包字节，嵌套聚合递归）。
        let cid = *child;
        if let Some(sub) = cp.agg(cid) {
            pack_agg_into_bytes(cp, ts, sub, off, out)?;
        } else {
            let raw = cp
                .bits(cid)
                .map(|bv| bv.bits)
                .ok_or_else(|| IrError::Internal("聚合常量打包：标量子缺失".into()))?;
            let n = ts.size_bytes(fty) as usize;
            for k in 0..n {
                // `k` 是**字节内序号**：小端 = 低位在前；大端 = 低位在后（整字节反序）。
                let at = if big { off + n - 1 - k } else { off + k };
                out[at] = (raw >> (k * 8)) as u8;
            }
        }
    }
    Ok(())
}
