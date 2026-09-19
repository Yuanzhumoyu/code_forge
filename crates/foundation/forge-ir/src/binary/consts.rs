//! CONSTS 段：常量池五通道（int / float / big / vector / aggregate）。
//!
//! 段内顺序固定：**int → float → big → vector → aggregate**（聚合可引用标量，
//! 必须最后）。各通道按池内索引顺序逐条写，解码按同序 `insert_*` 重建 ⇒
//! `ConstId`/`AggId` 的密集索引逐位不变（句柄即索引）。
//!
//! fail-closed：未知通道判别位、越界 `ConstId`/`AggId` 子引用、`Big` 的未知变体
//! tag、长度越界一律 `IrError::BinaryDecode`；解码期若 `insert_*` 返回的索引与
//! 文件里的位置不符（= 文件含重复条目，合法池不可能出现）也报错。

use crate::entity::{AggId, ConstId, Endianness, TypeId};
use crate::error::IrError;
use crate::ir::constant::{AggChild, ConstantPool};
use crate::util::big::Big;

use super::format::SectionId;
use super::reader::Cursor;
use super::writer::{self, Writer};

// ============================================================
// 编码
// ============================================================

/// 写 CONSTS 段（段体缓冲由调用方装回 writer）。
pub(crate) fn encode_consts(pool: &ConstantPool, w: &mut Writer) {
    let mut body = Vec::new();
    encode_pool(pool, &mut body, w);
    w.assign_section(SectionId::Consts, body);
}

/// 写一个常量池的五通道体（**段级与函数级共用**：模块的 `CONSTS` 段、
/// 每个函数记录里的 `constants` 字段都走这里）。
pub(crate) fn encode_pool(pool: &ConstantPool, body: &mut Vec<u8>, _w: &mut Writer) {
    let (int_n, float_n, big_n, vec_n, agg_n) = pool.channel_counts();

    // --- int ---
    writer::put_varint(body, int_n as u64);
    for i in 0..int_n {
        let id = ConstId::pack(ConstId::TAG_INT, i as u32);
        let (value, bits) = pool.get_int(id).expect("int 通道逐条存在");
        writer::put_zigzag_i128(body, value);
        writer::put_varint(body, u64::from(bits));
    }

    // --- float ---
    writer::put_varint(body, float_n as u64);
    for i in 0..float_n {
        let id = ConstId::pack(ConstId::TAG_FLOAT, i as u32);
        let (bits, width) = pool.get_float_with_width(id).expect("float 通道逐条存在");
        writer::put_varint_u128(body, bits);
        writer::put_varint(body, u64::from(width));
    }

    // --- big ---
    writer::put_varint(body, big_n as u64);
    for i in 0..big_n {
        let id = ConstId::pack(ConstId::TAG_BIG, i as u32);
        let value = pool.get_big(id).expect("big 通道逐条存在");
        encode_big(body, value);
    }

    // --- vector（字节 + 端序）---
    writer::put_varint(body, vec_n as u64);
    for i in 0..vec_n {
        let id = ConstId::pack(ConstId::TAG_VEC, i as u32);
        let data = pool.get_vector(id).expect("vector 通道逐条存在");
        let endian = pool.get_vector_endian(id).expect("vector 端序与数据同生");
        writer::put_len_prefixed(body, data);
        writer::put_u8(
            body,
            match endian {
                Endianness::Little => 0,
                Endianness::Big => 1,
            },
        );
    }

    // --- aggregate（树形：标量子 + 嵌套聚合子）---
    writer::put_varint(body, agg_n as u64);
    for i in 0..agg_n {
        let agg = pool
            .get_aggregate(AggId::new(i as u32))
            .expect("aggregate 通道逐条存在");
        writer::put_varint(body, u64::from(agg.ty.0));
        writer::put_varint(body, agg.children.len() as u64);
        for child in &agg.children {
            match child {
                AggChild::Scalar(id) => {
                    writer::put_u8(body, 0);
                    writer::put_varint(body, u64::from(id.raw()));
                }
                AggChild::Agg(id) => {
                    writer::put_u8(body, 1);
                    writer::put_varint(body, u64::from(id.0));
                }
            }
        }
    }
}

/// `Big` 三个变体：有符号整数 / 无符号整数 / 任意精度实数。
///
/// 整数用 `dashu` 的小端补码字节（规范形式、无冗余符号字节）；实数用
/// `repr().into_parts()` 的 `(significand, exponent)` 逐项写。
fn encode_big(out: &mut Vec<u8>, value: &Big) {
    match value {
        Big::Signed(i) => {
            writer::put_u8(out, 0);
            writer::put_len_prefixed(out, &i.to_le_bytes());
        }
        Big::Unsigned(n) => {
            writer::put_u8(out, 1);
            writer::put_len_prefixed(out, &n.to_le_bytes());
        }
        Big::Float(r) => {
            writer::put_u8(out, 2);
            // 有效数按 `Repr` 的**归一化**形式取（`Repr::new` 去尾零）；
            // 精度（`Context::precision`，0 = 无限）单独写——它是 FBig 的一部分，
            // 不落盘会让 `from_parts` 把它重置为"有效数位数"（1.5 的 53 → 2）。
            let repr = r.repr();
            writer::put_len_prefixed(out, &repr.significand().to_le_bytes());
            writer::put_zigzag(out, repr.exponent() as i64);
            writer::put_varint(out, r.precision() as u64);
        }
    }
}

// ============================================================
// 解码
// ============================================================

fn err<T>(offset: usize, msg: impl Into<String>) -> Result<T, IrError> {
    Err(IrError::BinaryDecode {
        offset,
        msg: msg.into(),
    })
}

/// 读 CONSTS 段并**填充**常量池（池由调用方以 `ConstantPool::new()` 起手：
/// 预置的两个 bool 槽会被文件里同序的前两条 int 条目经去重命中）。
pub(crate) fn decode_consts(pool: &mut ConstantPool, c: Cursor<'_>) -> Result<(), IrError> {
    decode_pool(pool, &mut c.clone())
}

/// 读一个常量池的五通道体（**段级与函数级共用**，见 [`encode_pool`]）。
pub(crate) fn decode_pool(pool: &mut ConstantPool, c: &mut Cursor<'_>) -> Result<(), IrError> {
    // --- int ---
    let n_at = c.offset();
    let n = c.read_usize()?;
    if n > c.remaining() {
        return err(
            n_at,
            format!("int 通道声明 {n} 条，但剩余字节只有 {}", c.remaining()),
        );
    }
    for i in 0..n {
        let at = c.offset();
        let value = c.read_zigzag_i128()?;
        let bits_at = c.offset();
        let bits = u32::try_from(c.read_varint()?).map_err(|_| IrError::BinaryDecode {
            offset: bits_at,
            msg: "整数位宽超出 u32".to_string(),
        })?;
        let id = pool.insert_int(value, bits);
        if id.index() as usize != i {
            return err(
                at,
                format!("int 通道第 {i} 条与既有条目重复（返回索引 {}）", id.index()),
            );
        }
    }

    // --- float ---
    let n_at = c.offset();
    let n = c.read_usize()?;
    if n > c.remaining() {
        return err(
            n_at,
            format!("float 通道声明 {n} 条，但剩余字节只有 {}", c.remaining()),
        );
    }
    for i in 0..n {
        let at = c.offset();
        let bits = c.read_varint_u128()?;
        let w_at = c.offset();
        let width = u16::try_from(c.read_varint()?).map_err(|_| IrError::BinaryDecode {
            offset: w_at,
            msg: "浮点值宽超出 u16".to_string(),
        })?;
        if !(1..=128).contains(&width) {
            return err(w_at, format!("浮点值宽必须在 1..=128，实测 {width}"));
        }
        let id = pool.insert_float_typed(bits, width);
        if id.index() as usize != i {
            return err(
                at,
                format!(
                    "float 通道第 {i} 条与既有条目重复（返回索引 {}）",
                    id.index()
                ),
            );
        }
    }

    // --- big ---
    let n_at = c.offset();
    let n = c.read_usize()?;
    if n > c.remaining() {
        return err(
            n_at,
            format!("big 通道声明 {n} 条，但剩余字节只有 {}", c.remaining()),
        );
    }
    for i in 0..n {
        let at = c.offset();
        let value = decode_big(c)?;
        let id = pool.insert_big(value);
        if id.index() as usize != i {
            return err(
                at,
                format!("big 通道第 {i} 条与既有条目重复（返回索引 {}）", id.index()),
            );
        }
    }

    // --- vector ---
    let n_at = c.offset();
    let n = c.read_usize()?;
    if n > c.remaining() {
        return err(
            n_at,
            format!("vector 通道声明 {n} 条，但剩余字节只有 {}", c.remaining()),
        );
    }
    for i in 0..n {
        let at = c.offset();
        let data = c.read_len_prefixed()?;
        let e_at = c.offset();
        let endian = match c.read_u8()? {
            0 => Endianness::Little,
            1 => Endianness::Big,
            other => return err(e_at, format!("未知端序 tag {other}")),
        };
        let id = pool.insert_vector_with_endian(data, endian);
        if id.index() as usize != i {
            return err(
                at,
                format!(
                    "vector 通道第 {i} 条与既有条目重复（返回索引 {}）",
                    id.index()
                ),
            );
        }
    }

    // --- aggregate ---
    let n_at = c.offset();
    let n = c.read_usize()?;
    if n > c.remaining() {
        return err(
            n_at,
            format!(
                "aggregate 通道声明 {n} 条，但剩余字节只有 {}",
                c.remaining()
            ),
        );
    }
    let (int_n, float_n, big_n, vec_n, _) = pool.channel_counts();
    for i in 0..n {
        let at = c.offset();
        let ty_at = c.offset();
        let ty = u32::try_from(c.read_varint()?).map_err(|_| IrError::BinaryDecode {
            offset: ty_at,
            msg: "聚合类型 TypeId 超出 u32".to_string(),
        })?;
        let c_at = c.offset();
        let child_n = c.read_usize()?;
        if child_n > c.remaining() {
            return err(
                c_at,
                format!(
                    "聚合声明 {child_n} 个子节点，但剩余字节只有 {}",
                    c.remaining()
                ),
            );
        }
        let mut children = Vec::with_capacity(child_n);
        for _ in 0..child_n {
            let k_at = c.offset();
            let tag = c.read_u8()?;
            let idx_at = c.offset();
            let idx = c.read_usize()?;
            match tag {
                0 => {
                    let id = ConstId::from_raw(u32::try_from(idx).map_err(|_| {
                        IrError::BinaryDecode {
                            offset: idx_at,
                            msg: "聚合标量子超出 u32".to_string(),
                        }
                    })?);
                    // 悬空标量：按 tag 在对应通道里查一次（越界即错）
                    let ok = match id.tag() {
                        ConstId::TAG_INT => (id.index() as usize) < int_n,
                        ConstId::TAG_FLOAT => (id.index() as usize) < float_n,
                        ConstId::TAG_BIG => (id.index() as usize) < big_n,
                        ConstId::TAG_VEC => (id.index() as usize) < vec_n,
                        _ => false,
                    };
                    if !ok {
                        return err(k_at, format!("聚合标量子指向越界 ConstId {}", id.raw()));
                    }
                    children.push(AggChild::Scalar(id));
                }
                1 => {
                    let agg = u32::try_from(idx).map_err(|_| IrError::BinaryDecode {
                        offset: idx_at,
                        msg: "聚合子索引超出 u32".to_string(),
                    })?;
                    // 只允许引用**已解码**的聚合（前序索引）⇒ 树形、无环
                    if agg as usize >= i {
                        return err(
                            k_at,
                            format!("聚合子指向未解码/越界 AggId {agg}（当前 {i}）"),
                        );
                    }
                    children.push(AggChild::Agg(AggId::new(agg)));
                }
                other => return err(k_at, format!("未知聚合子 tag {other}")),
            }
        }
        let id = pool.insert_aggregate(TypeId(ty), children);
        if id.0 as usize != i {
            return err(
                at,
                format!("aggregate 通道第 {i} 条与既有条目重复（返回索引 {}）", id.0),
            );
        }
    }
    Ok(())
}

fn decode_big(c: &mut Cursor<'_>) -> Result<Big, IrError> {
    let at = c.offset();
    Ok(match c.read_u8()? {
        0 => Big::Signed(dashu::Integer::from_le_bytes(c.read_len_prefixed()?)),
        1 => Big::Unsigned(dashu::Natural::from_le_bytes(c.read_len_prefixed()?)),
        2 => {
            let significand = dashu::Integer::from_le_bytes(c.read_len_prefixed()?);
            let exponent_at = c.offset();
            let exponent = c.read_zigzag()?;
            let exp = isize::try_from(exponent).map_err(|_| IrError::BinaryDecode {
                offset: exponent_at,
                msg: format!("实数指数 {exponent} 超出本平台 isize"),
            })?;
            let p_at = c.offset();
            let precision = c.read_usize().map_err(|_| IrError::BinaryDecode {
                offset: p_at,
                msg: "实数精度超出 usize".to_string(),
            })?;
            let repr = dashu::float::Repr::new(significand, exp);
            let context = dashu::float::Context::new(precision);
            Big::Float(dashu::Real::from_repr(repr, context))
        }
        other => return err(at, format!("未知 Big 变体 tag {other}")),
    })
}
