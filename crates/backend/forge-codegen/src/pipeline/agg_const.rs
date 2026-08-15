//! 聚合常量字节打包（第二十七轮从 compiler.rs 拆出——纯函数组,无编译器
//! 状态依赖;供 Stage 0.5 聚合展开阶段的 Store/Call 常量参数打包使用）。

use forge_ir::{AggChild, AggConst, ConstantPool, Function, IrError, TypeStore};

/// 把聚合常量打包为小端字节序列(按类型布局含 padding)。
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
                    out[off + k] = (raw >> (k * 8)) as u8; // 小端
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
