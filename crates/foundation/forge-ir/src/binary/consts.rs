//! CONSTS 段：常量池（v28 带 tag 单流）。
//!
//! 布局（**无兼容读取**，`IR_FORMAT_VERSION` 6 起）：
//!
//! ```text
//! CONSTS := varint n, n × Entry
//! Entry  := u8 tag, payload
//!   tag 0 (Bits)   : zigzag varint(位模式) + varint(位宽)
//!   tag 1 (Big)    : 既有 Big 三变体编码
//!   tag 2 (Vector) : len-prefixed 字节 + u8 端序
//!   tag 3 (Agg)    : varint(类型 TypeId) + varint(子数) + 子 × varint(ConstId::raw)
//! ```
//!
//! 每条目按**通道内插入顺序**写出，解码按同序 `intern_*` 重建 ⇒ 句柄（索引 = 插入序号）
//! 逐位不变。解码期校验"第 i 条同 tag 条目拿到的索引必须等于 i"——重复条目（合法池不可能
//! 出现）即报错。聚合子可以引用**任意通道已解码**的句柄；引用自身通道时必须是**前序**索引
//! （保证无环）。
//!
//! fail-closed：未知 tag、越界索引、长度越界、`Big` 未知变体一律 `IrError::BinaryDecode`。

use crate::entity::{ConstId, Endianness, TypeId};
use crate::error::IrError;
use crate::ir::constant::ConstantPool;
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

/// 写一个常量池的条目体（**段级与函数级共用**：模块的 `CONSTS` 段、
/// 每个函数记录里的 `constants` 字段都走这里）。
pub(crate) fn encode_pool(pool: &ConstantPool, body: &mut Vec<u8>, _w: &mut Writer) {
    let counts = pool.counts();
    writer::put_varint(
        body,
        (counts.bits + counts.big + counts.vector + counts.agg) as u64,
    );

    for v in pool.bits_iter() {
        writer::put_u8(body, TAG_BITS);
        writer::put_zigzag_i128(body, v.bits as i128);
        writer::put_varint(body, u64::from(v.width));
    }
    for b in pool.big_iter() {
        writer::put_u8(body, TAG_BIG);
        encode_big(body, b);
    }
    for (data, endian) in pool.vector_iter() {
        writer::put_u8(body, TAG_VECTOR);
        writer::put_len_prefixed(body, data);
        writer::put_u8(
            body,
            match endian {
                Endianness::Little => 0,
                Endianness::Big => 1,
            },
        );
    }
    for a in pool.agg_iter() {
        writer::put_u8(body, TAG_AGG);
        writer::put_varint(body, u64::from(a.ty.0));
        writer::put_varint(body, a.children.len() as u64);
        for c in &a.children {
            writer::put_varint(body, u64::from(c.raw()));
        }
    }
}

const TAG_BITS: u8 = 0;
const TAG_BIG: u8 = 1;
const TAG_VECTOR: u8 = 2;
const TAG_AGG: u8 = 3;

/// `Big` 三个变体：有符号整数 / 无符号整数 / 任意精度实数。
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

/// 读 CONSTS 段并填充常量池（池由调用方以 `ConstantPool::new()` 起手）。
pub(crate) fn decode_consts(pool: &mut ConstantPool, c: Cursor<'_>) -> Result<(), IrError> {
    decode_pool(pool, &mut c.clone())
}

/// 解码一个常量池体（与 [`encode_pool`] 对称）。
pub(crate) fn decode_pool(pool: &mut ConstantPool, c: &mut Cursor<'_>) -> Result<(), IrError> {
    let n_at = c.offset();
    let n = c.read_usize()?;
    if n > c.remaining() {
        return err(
            n_at,
            format!("常量段声明 {n} 条，但剩余字节只有 {}", c.remaining()),
        );
    }

    // 每个通道已解码条数 = 下一条目应有的索引
    let mut seen = [0usize; 4];
    for _ in 0..n {
        let at = c.offset();
        let tag = c.read_u8()?;
        let id = match tag {
            TAG_BITS => {
                let bits = c.read_zigzag_i128()? as u128;
                let w_at = c.offset();
                let width = u32::try_from(c.read_varint()?).map_err(|_| IrError::BinaryDecode {
                    offset: w_at,
                    msg: "位值宽度超出 u32".to_string(),
                })?;
                if !(1..=128).contains(&width) {
                    return err(w_at, format!("位值宽度必须在 1..=128，实测 {width}"));
                }
                pool.intern_bits(crate::ir::constant::BitValue { bits, width })
            }
            TAG_BIG => pool.intern_big(decode_big(c)?),
            TAG_VECTOR => {
                let data = c.read_len_prefixed()?;
                let e_at = c.offset();
                let endian = match c.read_u8()? {
                    0 => Endianness::Little,
                    1 => Endianness::Big,
                    other => return err(e_at, format!("未知端序 tag {other}")),
                };
                pool.intern_vector(data, endian)
            }
            TAG_AGG => {
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
                    let raw_at = c.offset();
                    let raw =
                        u32::try_from(c.read_varint()?).map_err(|_| IrError::BinaryDecode {
                            offset: raw_at,
                            msg: "聚合子句柄超出 u32".to_string(),
                        })?;
                    let child = ConstId::from_raw(raw);
                    // 越界/未解码的句柄：按 tag 查已解码条数（Agg 必须指向**前序**聚合）
                    let limit = match child.tag() {
                        ConstId::TAG_BITS => seen[0],
                        ConstId::TAG_BIG => seen[1],
                        ConstId::TAG_VECTOR => seen[2],
                        _ => seen[3],
                    };
                    if child.index() as usize >= limit {
                        return err(k_at, format!("聚合子指向越界/未解码句柄 {child:?}"));
                    }
                    children.push(child);
                }
                pool.intern_agg(TypeId(ty), children)
            }
            other => return err(at, format!("未知常量 tag {other}")),
        };
        let slot = &mut seen[id.tag() as usize];
        if id.index() as usize != *slot {
            return err(
                at,
                format!(
                    "第 {} 通道的第 {slot} 条与既有条目重复（返回句柄 {id:?}）",
                    id.tag()
                ),
            );
        }
        *slot += 1;
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

fn err<T>(at: usize, msg: impl Into<String>) -> Result<T, IrError> {
    Err(IrError::BinaryDecode {
        offset: at,
        msg: msg.into(),
    })
}
