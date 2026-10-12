//! 聚合常量字节打包（第二十七轮从 compiler.rs 拆出——纯函数组,无编译器
//! 状态依赖;供 Stage 0.5 聚合展开阶段的 Store/Call 常量参数打包使用）。

use forge_ir::Endianness;
use forge_ir::{AggChild, AggConst, ConstantPool, Function, IrError, TypeStore};

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
            IrError::Unsupported(format!("聚合常量打包：字段 {i} 越界（type {:?}）", agg.ty))
        })?;
        let off = base + ts.field_offset(agg.ty, i as u32).unwrap_or(0) as usize;
        match child {
            AggChild::Scalar(c) => {
                let raw = if ts.is_float(fty) {
                    cp.get_float128(*c)
                        .ok_or_else(|| IrError::Unsupported("聚合常量打包：浮点标量缺失".into()))?
                } else {
                    cp.get_int(*c)
                        .ok_or_else(|| IrError::Unsupported("聚合常量打包：整数标量缺失".into()))?
                        .0 as u128
                };
                let n = ts.size_bytes(fty) as usize;
                for k in 0..n {
                    // `k` 是**字节内序号**：小端 = 低位在前；大端 = 低位在后（整字节反序）。
                    let at = if big { off + n - 1 - k } else { off + k };
                    out[at] = (raw >> (k * 8)) as u8;
                }
            }
            AggChild::Agg(aid) => {
                let sub = cp
                    .get_aggregate(*aid)
                    .ok_or_else(|| IrError::Unsupported("聚合常量打包：嵌套聚合缺失".into()))?;
                pack_agg_into_bytes(cp, ts, sub, off, out)?;
            }
        }
    }
    Ok(())
}
