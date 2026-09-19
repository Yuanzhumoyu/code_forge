//! METADATA 段：`MetadataStore` 节点表 + 命名表。
//!
//! 段内顺序：**节点表（dense 顺序）→ 命名表（按名字节序排序）**。
//! 节点按 id 顺序 `intern` 回放 ⇒ `MetadataId` 逐位不变；节点内的 `Node(id)` 引用
//! **只允许指向更小的 id**（`intern` 的语义决定：构造时只能引用已存在的节点），
//! 违反即 `Err`（防环、防悬空）。
//!
//! `Placeholder`（解析期为显式 `!N` 补的空洞）与用户写的空 `Tuple`（`!0 = !{}`）
//! 是**两个不同 tag**——display 行为不同（前者无人引用时不打印），不能合并。

use crate::error::IrError;
use crate::ir::metadata::{MetadataId, MetadataNode, MetadataStore, MetadataValue};
use crate::util::imm_str::ImmStr;

use super::format::SectionId;
use super::reader::Cursor;
use super::writer::{self, Writer};

/// metadata 值的**嵌套深度上限**（`MetadataValue::Field` 递归）。
///
/// 解码递归必须在读**不可信输入**时给深度上界：否则一条
/// `Field(k, Field(k, Field(k, …)))` 就能把解码器的栈打爆（进程 abort，
/// 而不是可捕获的错误）。64 远超真实 DI/metadata 的嵌套（语料实测个位数）。
pub(crate) const MAX_METADATA_DEPTH: usize = 64;

fn err<T>(offset: usize, msg: impl Into<String>) -> Result<T, IrError> {
    Err(IrError::BinaryDecode {
        offset,
        msg: msg.into(),
    })
}

// ============================================================
// 编码
// ============================================================

pub(crate) fn encode_metadata(store: &MetadataStore, w: &mut Writer) {
    let mut body = Vec::new();
    writer::put_varint(&mut body, store.len() as u64);
    for i in 0..store.len() {
        let node = store
            .get(MetadataId(i as u32))
            .expect("metadata 节点按 id 连续存在");
        encode_node(&mut body, node, w);
    }
    let names = store.names_sorted();
    writer::put_varint(&mut body, names.len() as u64);
    for (name, id) in &names {
        let idx = w.intern(name);
        writer::put_varint(&mut body, u64::from(idx));
        writer::put_varint(&mut body, u64::from(id.0));
    }
    w.assign_section(SectionId::Metadata, body);
}

fn encode_node(out: &mut Vec<u8>, node: &MetadataNode, w: &mut Writer) {
    match node {
        MetadataNode::Leaf(v) => {
            writer::put_u8(out, 0);
            encode_value(out, v, w);
        }
        MetadataNode::Tuple(values) => {
            writer::put_u8(out, 1);
            encode_values(out, values, w);
        }
        MetadataNode::Named {
            name,
            ops,
            distinct,
        } => {
            writer::put_u8(out, 2);
            let idx = w.intern(name);
            writer::put_varint(out, u64::from(idx));
            encode_values(out, ops, w);
            writer::put_u8(out, u8::from(*distinct));
        }
        MetadataNode::Placeholder => writer::put_u8(out, 3),
    }
}

fn encode_values(out: &mut Vec<u8>, values: &[MetadataValue], w: &mut Writer) {
    writer::put_varint(out, values.len() as u64);
    for v in values {
        encode_value(out, v, w);
    }
}

fn encode_value(out: &mut Vec<u8>, value: &MetadataValue, w: &mut Writer) {
    match value {
        MetadataValue::String(s) => {
            writer::put_u8(out, 0);
            let idx = w.intern(s);
            writer::put_varint(out, u64::from(idx));
        }
        MetadataValue::Uint(v) => {
            writer::put_u8(out, 1);
            writer::put_varint(out, *v);
        }
        MetadataValue::Int(v) => {
            writer::put_u8(out, 2);
            writer::put_zigzag(out, *v);
        }
        MetadataValue::IntBig(s) => {
            writer::put_u8(out, 3);
            let idx = w.intern(s);
            writer::put_varint(out, u64::from(idx));
        }
        MetadataValue::Float(bits) => {
            writer::put_u8(out, 4);
            writer::put_varint(out, *bits);
        }
        MetadataValue::Null => writer::put_u8(out, 5),
        MetadataValue::Node(id) => {
            writer::put_u8(out, 6);
            writer::put_varint(out, u64::from(id.0));
        }
        MetadataValue::Field(key, inner) => {
            writer::put_u8(out, 7);
            let idx = w.intern(key);
            writer::put_varint(out, u64::from(idx));
            encode_value(out, inner, w);
        }
    }
}

// ============================================================
// 解码
// ============================================================

/// 读 METADATA 段并**填充** `store`（由调用方以 `MetadataStore::new()` 起手）。
pub(crate) fn decode_metadata(
    store: &mut MetadataStore,
    mut c: Cursor<'_>,
    strings: &[ImmStr],
) -> Result<(), IrError> {
    let n_at = c.offset();
    let n = c.read_usize()?;
    if n > c.remaining() {
        return err(
            n_at,
            format!("metadata 节点声明 {n} 条，但剩余字节只有 {}", c.remaining()),
        );
    }
    let _ = n;
    for _ in 0..n {
        let node = decode_node(&mut c, strings, n)?;
        store.push_verbatim(node);
    }
    let n_at = c.offset();
    let n = c.read_usize()?;
    if n > c.remaining() / 2 {
        return err(n_at, format!("metadata 命名表声明 {n} 条，剩余字节不足"));
    }
    // 该表是 `name → id` 的**多对一**映射（同一节点可有多个别名），
    // 因此只拒绝"同一个名字出现两次"，不要求"一个 id 只有一个名字"。
    let mut seen_names: std::collections::HashSet<ImmStr> = std::collections::HashSet::new();
    for _ in 0..n {
        let at = c.offset();
        let name_idx = c.read_usize()?;
        let name = strings
            .get(name_idx)
            .cloned()
            .ok_or(IrError::BinaryDecode {
                offset: at,
                msg: format!("字符串索引 {name_idx} 越界（表长 {}）", strings.len()),
            })?;
        let id_at = c.offset();
        let id = c.read_usize()?;
        if id as u32 as usize >= store.len() {
            return err(
                id_at,
                format!(
                    "命名 metadata {name:?} 指向越界 id {id}（节点数 {}）",
                    store.len()
                ),
            );
        }
        if seen_names.contains(name.as_str()) {
            return err(at, format!("命名 metadata {name:?} 重复出现"));
        }
        seen_names.insert(name.clone());
        store.define_named(name.as_str(), MetadataId(id as u32));
    }
    Ok(())
}

fn decode_node(
    c: &mut Cursor<'_>,
    strings: &[ImmStr],
    total: usize,
) -> Result<MetadataNode, IrError> {
    let at = c.offset();
    Ok(match c.read_u8()? {
        0 => MetadataNode::Leaf(decode_value(c, strings, total, 0)?),
        1 => MetadataNode::Tuple(decode_values(c, strings, total, 0)?.into()),
        2 => {
            let at = c.offset();
            let name_idx = c.read_usize()?;
            let name = strings
                .get(name_idx)
                .cloned()
                .ok_or(IrError::BinaryDecode {
                    offset: at,
                    msg: format!("字符串索引 {name_idx} 越界（表长 {}）", strings.len()),
                })?;
            let ops = decode_values(c, strings, total, 0)?.into();
            let distinct = read_bool(c)?;
            MetadataNode::Named {
                name,
                ops,
                distinct,
            }
        }
        3 => MetadataNode::Placeholder,
        other => return err(at, format!("未知 metadata 节点 tag {other}")),
    })
}

fn decode_values(
    c: &mut Cursor<'_>,
    strings: &[ImmStr],
    total: usize,
    depth: usize,
) -> Result<Vec<MetadataValue>, IrError> {
    let n_at = c.offset();
    let n = c.read_usize()?;
    if n > c.remaining() {
        return err(n_at, format!("metadata 值声明 {n} 条，剩余字节不足"));
    }
    let mut out = Vec::with_capacity(n);
    for _ in 0..n {
        out.push(decode_value(c, strings, total, depth)?);
    }
    Ok(out)
}

fn decode_value(
    c: &mut Cursor<'_>,
    strings: &[ImmStr],
    total: usize,
    depth: usize,
) -> Result<MetadataValue, IrError> {
    if depth > MAX_METADATA_DEPTH {
        return err(
            c.offset(),
            format!("metadata 嵌套超过 {MAX_METADATA_DEPTH} 层（拒绝：防解码器栈溢出）"),
        );
    }
    let at = c.offset();
    Ok(match c.read_u8()? {
        0 => {
            let s_at = c.offset();
            let idx = c.read_usize()?;
            MetadataValue::String(strings.get(idx).cloned().ok_or(IrError::BinaryDecode {
                offset: s_at,
                msg: format!("字符串索引 {idx} 越界（表长 {}）", strings.len()),
            })?)
        }
        1 => MetadataValue::Uint(c.read_varint()?),
        2 => MetadataValue::Int(c.read_zigzag()?),
        3 => {
            let s_at = c.offset();
            let idx = c.read_usize()?;
            MetadataValue::IntBig(strings.get(idx).cloned().ok_or(IrError::BinaryDecode {
                offset: s_at,
                msg: format!("字符串索引 {idx} 越界（表长 {}）", strings.len()),
            })?)
        }
        4 => MetadataValue::Float(c.read_varint()?),
        5 => MetadataValue::Null,
        6 => {
            let id_at = c.offset();
            let raw = c.read_usize()?;
            // 只能引用**更小的 id**：`intern` 时该节点已存在，反之即损坏/成环
            if raw >= total {
                return err(
                    id_at,
                    format!("metadata 节点引用了越界 id {raw}（节点数 {total}）"),
                );
            }
            MetadataValue::Node(MetadataId(raw as u32))
        }
        7 => {
            let k_at = c.offset();
            let idx = c.read_usize()?;
            let key = strings.get(idx).cloned().ok_or(IrError::BinaryDecode {
                offset: k_at,
                msg: format!("字符串索引 {idx} 越界（表长 {}）", strings.len()),
            })?;
            MetadataValue::Field(key, Box::new(decode_value(c, strings, total, depth + 1)?))
        }
        other => return err(at, format!("未知 metadata 值 tag {other}")),
    })
}

fn read_bool(c: &mut Cursor<'_>) -> Result<bool, IrError> {
    let at = c.offset();
    match c.read_u8()? {
        0 => Ok(false),
        1 => Ok(true),
        other => err(at, format!("bool 判别位应为 0/1，实测 {other}")),
    }
}

/// 读 u32（附件里的 `MetadataId`）。
fn read_u32(c: &mut Cursor<'_>, what: &str) -> Result<u32, IrError> {
    let at = c.offset();
    let v = c.read_usize()?;
    u32::try_from(v).map_err(|_| IrError::BinaryDecode {
        offset: at,
        msg: format!("{what} 超出 u32"),
    })
}

// ============================================================
// 附件（`(kind, node)` 对）：FUNCS / GLOBALS 共用
// ============================================================

/// 写一层 metadata 附件（函数头 / 指令 / 全局 / 别名共用同一编码）。
pub(crate) fn encode_attached(
    out: &mut Vec<u8>,
    w: &mut Writer,
    list: &[crate::ir::metadata::AttachedMetadata],
) {
    writer::put_varint(out, list.len() as u64);
    for m in list {
        encode_kind(out, w, &m.kind);
        writer::put_varint(out, u64::from(m.node.0));
    }
}

/// 读一层 metadata 附件。
pub(crate) fn decode_attached(
    c: &mut Cursor<'_>,
    strings: &[ImmStr],
) -> Result<Vec<crate::ir::metadata::AttachedMetadata>, IrError> {
    use crate::ir::metadata::{AttachedMetadata, MetadataId};
    let n_at = c.offset();
    let n = c.read_usize()?;
    if n > c.remaining() {
        return err(n_at, format!("metadata 附件声明 {n} 条，剩余字节不足"));
    }
    let mut out = Vec::with_capacity(n);
    for _ in 0..n {
        let kind = decode_kind(c, strings)?;
        out.push(AttachedMetadata {
            kind,
            node: MetadataId(read_u32(c, "metadata 节点")?),
        });
    }
    Ok(out)
}

/// 写 `MetadataKind`（闭合集合 + `Custom` 开放名）。
pub(crate) fn encode_kind(
    out: &mut Vec<u8>,
    w: &mut Writer,
    kind: &crate::ir::metadata::MetadataKind,
) {
    use crate::ir::metadata::MetadataKind;
    writer::put_u8(
        out,
        match kind {
            MetadataKind::DebugLoc => 0,
            MetadataKind::TBAA => 1,
            MetadataKind::TBAAStruct => 2,
            MetadataKind::AliasScope => 3,
            MetadataKind::NoAlias => 4,
            MetadataKind::Range => 5,
            MetadataKind::NonNull => 6,
            MetadataKind::Align => 7,
            MetadataKind::Dereferenceable => 8,
            MetadataKind::NoUndef => 9,
            MetadataKind::Loop => 10,
            MetadataKind::Prof => 11,
            MetadataKind::FpMath => 12,
            MetadataKind::Custom(_) => 13,
        },
    );
    if let MetadataKind::Custom(name) = kind {
        let idx = w.intern(name);
        writer::put_varint(out, u64::from(idx));
    }
}

/// 读 `MetadataKind`。
pub(crate) fn decode_kind(
    c: &mut Cursor<'_>,
    strings: &[ImmStr],
) -> Result<crate::ir::metadata::MetadataKind, IrError> {
    use crate::ir::metadata::MetadataKind;
    let at = c.offset();
    Ok(match c.read_u8()? {
        0 => MetadataKind::DebugLoc,
        1 => MetadataKind::TBAA,
        2 => MetadataKind::TBAAStruct,
        3 => MetadataKind::AliasScope,
        4 => MetadataKind::NoAlias,
        5 => MetadataKind::Range,
        6 => MetadataKind::NonNull,
        7 => MetadataKind::Align,
        8 => MetadataKind::Dereferenceable,
        9 => MetadataKind::NoUndef,
        10 => MetadataKind::Loop,
        11 => MetadataKind::Prof,
        12 => MetadataKind::FpMath,
        13 => {
            let s_at = c.offset();
            let idx = c.read_usize()?;
            MetadataKind::Custom(strings.get(idx).cloned().ok_or(IrError::BinaryDecode {
                offset: s_at,
                msg: format!("字符串索引 {idx} 越界（表长 {}）", strings.len()),
            })?)
        }
        other => return err(at, format!("未知 metadata kind tag {other}")),
    })
}
