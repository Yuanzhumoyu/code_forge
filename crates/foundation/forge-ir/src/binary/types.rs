//! TYPES 段：类型条目 + 命名类型表 + 函数签名表 + `DataLayout`。
//!
//! 段内编码顺序固定：**条目 → 命名类型 → 签名 → DataLayout**。
//! 一切"集合"都按确定性顺序写（`HashMap` 按 key 排序；`Vec` 按原序），
//! 因此同输入必同输出（B1 的确定性守卫在 `tests/binary_format.rs`）。
//!
//! 解码侧 fail-closed：未知条目 tag、越界字符串/类型索引、非法判别位、重复去重键、
//! 预填充固定索引错位、悬空类型引用——一律 `IrError::BinaryDecode`。

use crate::entity::{Endianness, TypeId};
use crate::error::IrError;
use crate::ir::data_layout::{DataLayout, Mangling};
use crate::ir::types::{CallConv, FunctionSignature, TypeEntry, TypeField, TypeStore};
use crate::util::imm_str::ImmStr;
use crate::util::string_pool::InternedStr;

use super::format::SectionId;
use super::reader::Cursor;
use super::writer::{self, Writer};

// ============================================================
// 编码
// ============================================================

/// 写 TYPES 段（段体缓冲由调用方装回 writer）。
pub(crate) fn encode_types(store: &TypeStore, w: &mut Writer) {
    let mut body = Vec::new();
    writer::put_varint(&mut body, store.entries().len() as u64);
    for entry in store.entries() {
        encode_entry(&mut body, entry, w);
    }
    let named = store.named_types_sorted();
    writer::put_varint(&mut body, named.len() as u64);
    for (name, id) in &named {
        let idx = w.intern(name);
        writer::put_varint(&mut body, u64::from(idx));
        writer::put_varint(&mut body, u64::from(id.0));
    }
    writer::put_varint(&mut body, store.signatures().len() as u64);
    for sig in store.signatures() {
        encode_signature(&mut body, sig, w);
    }
    encode_data_layout(&mut body, &store.data_layout);
    w.assign_section(SectionId::Types, body);
}

/// 池内字符串句柄 → 表索引（不自洽的 IR 在此 fail-fast 并说明原因）。
fn pool_str(w: &Writer, handle: InternedStr) -> u64 {
    match w.pool_string_index(handle) {
        Some(idx) => u64::from(idx),
        None => panic!(
            "InternedStr({}) 不在类型库字符串池内——IR 不自洽（句柄与模块的池必须同源）",
            handle.0
        ),
    }
}

fn encode_entry(out: &mut Vec<u8>, entry: &TypeEntry, w: &mut Writer) {
    match entry {
        TypeEntry::Int { bits } => {
            writer::put_u8(out, 0);
            writer::put_varint(out, u64::from(*bits));
        }
        TypeEntry::Float { bits } => {
            writer::put_u8(out, 1);
            writer::put_varint(out, u64::from(*bits));
        }
        TypeEntry::BFloat { bits } => {
            writer::put_u8(out, 2);
            writer::put_varint(out, u64::from(*bits));
        }
        TypeEntry::Vector { elem, len } => {
            writer::put_u8(out, 3);
            writer::put_varint(out, u64::from(elem.0));
            writer::put_varint(out, u64::from(*len));
        }
        TypeEntry::ScalableVector { elem, min_len } => {
            writer::put_u8(out, 4);
            writer::put_varint(out, u64::from(elem.0));
            writer::put_varint(out, u64::from(*min_len));
        }
        TypeEntry::Array { elem, len } => {
            writer::put_u8(out, 5);
            writer::put_varint(out, u64::from(elem.0));
            writer::put_varint(out, *len);
        }
        TypeEntry::Struct {
            name,
            fields,
            is_packed,
        } => {
            writer::put_u8(out, 6);
            writer::put_option_tag(out, name.is_some());
            if let Some(n) = name {
                writer::put_varint(out, pool_str(w, *n));
            }
            writer::put_varint(out, fields.len() as u64);
            for f in fields {
                writer::put_option_tag(out, f.name.is_some());
                if let Some(n) = f.name {
                    writer::put_varint(out, pool_str(w, n));
                }
                writer::put_varint(out, u64::from(f.ty.0));
            }
            writer::put_u8(out, u8::from(*is_packed));
        }
        TypeEntry::Pointer { addr_space } => {
            writer::put_u8(out, 7);
            writer::put_varint(out, u64::from(*addr_space));
        }
        TypeEntry::Function {
            params,
            rets,
            is_vararg,
        } => {
            writer::put_u8(out, 8);
            writer::put_varint(out, params.len() as u64);
            for p in params {
                writer::put_varint(out, u64::from(p.0));
            }
            writer::put_varint(out, rets.len() as u64);
            for r in rets {
                writer::put_varint(out, u64::from(r.0));
            }
            writer::put_u8(out, u8::from(*is_vararg));
        }
        TypeEntry::Token => writer::put_u8(out, 9),
        TypeEntry::Metadata => writer::put_u8(out, 10),
        TypeEntry::Opaque => writer::put_u8(out, 11),
    }
}

fn encode_signature(out: &mut Vec<u8>, sig: &FunctionSignature, w: &mut Writer) {
    writer::put_varint(out, sig.params.len() as u64);
    for (ty, name) in &sig.params {
        writer::put_varint(out, u64::from(ty.0));
        let idx = w.intern(name);
        writer::put_varint(out, u64::from(idx));
    }
    writer::put_varint(out, sig.returns.len() as u64);
    for r in &sig.returns {
        writer::put_varint(out, u64::from(r.0));
    }
    encode_call_conv(out, sig.calling_convention);
    writer::put_u8(out, u8::from(sig.variadic));
}

pub(crate) fn encode_call_conv(out: &mut Vec<u8>, cc: CallConv) {
    // 判别值固定；新增变体时这里的 match 会编译期失败（不会静默错位）。
    let tag: u8 = match cc {
        CallConv::Default => 0,
        CallConv::SystemV => 1,
        CallConv::WindowsX64 => 2,
        CallConv::Fast => 3,
        CallConv::CDecl => 4,
        CallConv::Internal => 5,
        CallConv::Custom(_) => 6,
        CallConv::Aapcs => 7,
        CallConv::AapcsVfp => 8,
        CallConv::RiscvIlp32 => 9,
        CallConv::RiscvLp64 => 10,
        CallConv::WasmBasic => 11,
        CallConv::StdCall => 12,
        CallConv::VectorCall => 13,
        CallConv::PreserveMost => 14,
        CallConv::PreserveAll => 15,
        CallConv::Cold => 16,
    };
    writer::put_u8(out, tag);
    if let CallConv::Custom(n) = cc {
        writer::put_varint(out, u64::from(n));
    }
}

fn encode_data_layout(out: &mut Vec<u8>, dl: &DataLayout) {
    writer::put_u8(
        out,
        match dl.endianness {
            Endianness::Little => 0,
            Endianness::Big => 1,
        },
    );
    writer::put_u8(
        out,
        match dl.mangling {
            Mangling::Elf => 0,
            Mangling::MachO => 1,
            Mangling::WindowsCoff => 2,
        },
    );

    // 三张对齐表 + 指针表：按 key 排序（HashMap 迭代序不确定）。
    let mut ptrs: Vec<(u32, (u32, u32))> =
        dl.pointer_layout.iter().map(|(k, v)| (*k, *v)).collect();
    ptrs.sort_unstable_by_key(|(k, _)| *k);
    writer::put_varint(out, ptrs.len() as u64);
    for (space, (size, align)) in ptrs {
        writer::put_varint(out, u64::from(space));
        writer::put_varint(out, u64::from(size));
        writer::put_varint(out, u64::from(align));
    }

    let mut ints: Vec<(u32, u32)> = dl
        .integer_alignments
        .iter()
        .map(|(k, v)| (*k, *v))
        .collect();
    ints.sort_unstable_by_key(|(k, _)| *k);
    writer::put_varint(out, ints.len() as u64);
    for (bits, align) in ints {
        writer::put_varint(out, u64::from(bits));
        writer::put_varint(out, u64::from(align));
    }

    let mut floats: Vec<(u16, u32)> = dl.float_alignments.iter().map(|(k, v)| (*k, *v)).collect();
    floats.sort_unstable_by_key(|(k, _)| *k);
    writer::put_varint(out, floats.len() as u64);
    for (bits, align) in floats {
        writer::put_varint(out, u64::from(bits));
        writer::put_varint(out, u64::from(align));
    }

    let mut vecs: Vec<(u32, u32)> = dl.vector_alignments.iter().map(|(k, v)| (*k, *v)).collect();
    vecs.sort_unstable_by_key(|(k, _)| *k);
    writer::put_varint(out, vecs.len() as u64);
    for (bits, align) in vecs {
        writer::put_varint(out, u64::from(bits));
        writer::put_varint(out, u64::from(align));
    }

    writer::put_varint(out, u64::from(dl.aggregate_align));
    writer::put_varint(out, u64::from(dl.max_alignment));
    writer::put_varint(out, dl.native_integer_widths.len() as u64);
    for wdt in &dl.native_integer_widths {
        writer::put_varint(out, u64::from(*wdt));
    }
    writer::put_varint(out, dl.native_vector_widths.len() as u64);
    for wdt in &dl.native_vector_widths {
        writer::put_varint(out, u64::from(*wdt));
    }
    writer::put_varint(out, u64::from(dl.stack_align));
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

/// 表索引 → 字符串（越界即错）。
fn string_at(strings: &[ImmStr], idx: u64, at: usize) -> Result<&ImmStr, IrError> {
    let i = usize::try_from(idx).map_err(|_| IrError::BinaryDecode {
        offset: at,
        msg: format!("字符串索引 {idx} 超出 usize"),
    })?;
    strings.get(i).ok_or(IrError::BinaryDecode {
        offset: at,
        msg: format!("字符串索引 {idx} 越界（表长 {}）", strings.len()),
    })
}

/// 表索引 → 池句柄。解码期两者**恒等**（`decode_strings` 保证无重复、按序重建池）。
fn interned_at(strings: &[ImmStr], idx: u64, at: usize) -> Result<InternedStr, IrError> {
    string_at(strings, idx, at)?;
    Ok(InternedStr(idx as u32))
}

/// 读 TYPES 段并**替换** `store` 的条目/命名类型/签名/布局。
pub(crate) fn decode_types(
    store: &mut TypeStore,
    mut c: Cursor<'_>,
    strings: &[ImmStr],
) -> Result<(), IrError> {
    store.reset_for_binary_decode();

    let count_at = c.offset();
    let count = c.read_usize()?;
    if count > c.remaining() {
        // 每条目至少 1 字节 tag ⇒ 条数不得超过剩余字节数（不做不可信分配）。
        return err(
            count_at,
            format!("类型条目声明 {count} 条，但剩余字节只有 {}", c.remaining()),
        );
    }
    for _ in 0..count {
        let at = c.offset();
        let tag = c.read_u8()?;
        let entry = decode_entry(tag, at, &mut c, strings)?;
        let _ = store.insert_verbatim(entry);
    }

    let named_at = c.offset();
    let named_count = c.read_usize()?;
    if named_count > c.remaining() / 2 {
        return err(
            named_at,
            format!(
                "命名类型声明 {named_count} 条，但剩余字节只有 {}",
                c.remaining()
            ),
        );
    }
    for _ in 0..named_count {
        let at = c.offset();
        let name_idx = c.read_varint()?;
        let name = string_at(strings, name_idx, at)?.clone();
        let ty_at = c.offset();
        let ty = c.read_usize()?;
        if ty >= count {
            return err(
                ty_at,
                format!("命名类型 {name:?} 指向越界 TypeId {ty}（条目数 {count}）"),
            );
        }
        // `named_types` 的键是 `ImmStr`；同名的两条记录说明文件被改过。
        if store.lookup_named(name.as_str()).is_some() {
            return err(at, format!("命名类型 {name:?} 重复出现"));
        }
        store.define_named(name.as_str(), TypeId(ty as u32));
    }

    let sig_at = c.offset();
    let sig_count = c.read_usize()?;
    if sig_count > c.remaining() {
        return err(
            sig_at,
            format!(
                "签名表声明 {sig_count} 条，但剩余字节只有 {}",
                c.remaining()
            ),
        );
    }
    for _ in 0..sig_count {
        let sig = decode_signature(&mut c, strings, count)?;
        store.register_signature(sig);
    }

    let layout = decode_data_layout(&mut c)?;
    store.set_data_layout(layout);
    store
        .finish_binary_decode()
        .map_err(|msg| IrError::BinaryDecode {
            offset: c.offset(),
            msg,
        })?;
    Ok(())
}

fn decode_entry(
    tag: u8,
    at: usize,
    c: &mut Cursor<'_>,
    strings: &[ImmStr],
) -> Result<TypeEntry, IrError> {
    let ty = |c: &mut Cursor<'_>| -> Result<TypeId, IrError> {
        let at = c.offset();
        let raw = c.read_usize()?;
        Ok(TypeId(u32::try_from(raw).map_err(|_| {
            IrError::BinaryDecode {
                offset: at,
                msg: format!("TypeId {raw} 超出 u32"),
            }
        })?))
    };
    Ok(match tag {
        0 => TypeEntry::Int {
            bits: u32::try_from(c.read_varint()?).map_err(|_| IrError::BinaryDecode {
                offset: at,
                msg: "Int 位宽超出 u32".to_string(),
            })?,
        },
        1 => TypeEntry::Float {
            bits: u16::try_from(c.read_varint()?).map_err(|_| IrError::BinaryDecode {
                offset: at,
                msg: "Float 位宽超出 u16".to_string(),
            })?,
        },
        2 => TypeEntry::BFloat {
            bits: u16::try_from(c.read_varint()?).map_err(|_| IrError::BinaryDecode {
                offset: at,
                msg: "BFloat 位宽超出 u16".to_string(),
            })?,
        },
        3 => {
            let elem = ty(c)?;
            let len = u32::try_from(c.read_varint()?).map_err(|_| IrError::BinaryDecode {
                offset: at,
                msg: "Vector 长度超出 u32".to_string(),
            })?;
            TypeEntry::Vector { elem, len }
        }
        4 => {
            let elem = ty(c)?;
            let min_len = u32::try_from(c.read_varint()?).map_err(|_| IrError::BinaryDecode {
                offset: at,
                msg: "ScalableVector 长度超出 u32".to_string(),
            })?;
            TypeEntry::ScalableVector { elem, min_len }
        }
        5 => {
            let elem = ty(c)?;
            let len = c.read_varint()?;
            TypeEntry::Array { elem, len }
        }
        6 => {
            let name = match read_option_tag(c)? {
                false => None,
                true => {
                    let idx_at = c.offset();
                    let idx = c.read_varint()?;
                    Some(interned_at(strings, idx, idx_at)?)
                }
            };
            let len_at = c.offset();
            let field_count = c.read_usize()?;
            if field_count > c.remaining() {
                return err(
                    len_at,
                    format!(
                        "结构体声明 {field_count} 字段，但剩余字节只有 {}",
                        c.remaining()
                    ),
                );
            }
            let mut fields = Vec::with_capacity(field_count);
            for _ in 0..field_count {
                let fname = match read_option_tag(c)? {
                    false => None,
                    true => {
                        let idx_at = c.offset();
                        let idx = c.read_varint()?;
                        Some(interned_at(strings, idx, idx_at)?)
                    }
                };
                let fty = ty(c)?;
                fields.push(TypeField {
                    name: fname,
                    ty: fty,
                });
            }
            let is_packed = read_bool(c, at)?;
            TypeEntry::Struct {
                name,
                fields,
                is_packed,
            }
        }
        7 => TypeEntry::Pointer {
            addr_space: u32::try_from(c.read_varint()?).map_err(|_| IrError::BinaryDecode {
                offset: at,
                msg: "Pointer 地址空间超出 u32".to_string(),
            })?,
        },
        8 => {
            let p_at = c.offset();
            let p_count = c.read_usize()?;
            if p_count > c.remaining() {
                return err(
                    p_at,
                    format!(
                        "函数类型声明 {p_count} 参数，但剩余字节只有 {}",
                        c.remaining()
                    ),
                );
            }
            let mut params = Vec::with_capacity(p_count);
            for _ in 0..p_count {
                params.push(ty(c)?);
            }
            let r_at = c.offset();
            let r_count = c.read_usize()?;
            if r_count > c.remaining() {
                return err(
                    r_at,
                    format!(
                        "函数类型声明 {r_count} 返回值，但剩余字节只有 {}",
                        c.remaining()
                    ),
                );
            }
            let mut rets = Vec::with_capacity(r_count);
            for _ in 0..r_count {
                rets.push(ty(c)?);
            }
            let is_vararg = read_bool(c, at)?;
            TypeEntry::Function {
                params,
                rets,
                is_vararg,
            }
        }
        9 => TypeEntry::Token,
        10 => TypeEntry::Metadata,
        11 => TypeEntry::Opaque,
        other => return err(at, format!("未知类型条目 tag {other}")),
    })
}

fn read_option_tag(c: &mut Cursor<'_>) -> Result<bool, IrError> {
    let at = c.offset();
    match c.read_u8()? {
        0 => Ok(false),
        1 => Ok(true),
        other => err(at, format!("Option 判别位应为 0/1，实测 {other}")),
    }
}

fn read_bool(c: &mut Cursor<'_>, _at: usize) -> Result<bool, IrError> {
    let at = c.offset();
    match c.read_u8()? {
        0 => Ok(false),
        1 => Ok(true),
        other => err(at, format!("bool 判别位应为 0/1，实测 {other}")),
    }
}

fn decode_signature(
    c: &mut Cursor<'_>,
    strings: &[ImmStr],
    type_count: usize,
) -> Result<FunctionSignature, IrError> {
    let p_at = c.offset();
    let p_count = c.read_usize()?;
    if p_count > c.remaining() {
        return err(
            p_at,
            format!("签名声明 {p_count} 参数，但剩余字节只有 {}", c.remaining()),
        );
    }
    let mut params = Vec::with_capacity(p_count);
    for _ in 0..p_count {
        let ty_at = c.offset();
        let raw = c.read_usize()?;
        if raw >= type_count {
            return err(
                ty_at,
                format!("签名参数指向越界 TypeId {raw}（条目数 {type_count}）"),
            );
        }
        let s_at = c.offset();
        let idx = c.read_varint()?;
        let name = string_at(strings, idx, s_at)?.clone();
        params.push((TypeId(raw as u32), name));
    }
    let r_at = c.offset();
    let r_count = c.read_usize()?;
    if r_count > c.remaining() {
        return err(
            r_at,
            format!(
                "签名声明 {r_count} 返回值，但剩余字节只有 {}",
                c.remaining()
            ),
        );
    }
    let mut returns = Vec::with_capacity(r_count);
    for _ in 0..r_count {
        let ty_at = c.offset();
        let raw = c.read_usize()?;
        if raw >= type_count {
            return err(
                ty_at,
                format!("签名返回值指向越界 TypeId {raw}（条目数 {type_count}）"),
            );
        }
        returns.push(TypeId(raw as u32));
    }
    let cc = decode_call_conv(c)?;
    let variadic = read_bool(c, c.offset())?;
    Ok(FunctionSignature {
        params,
        returns,
        calling_convention: cc,
        variadic,
    })
}

pub(crate) fn decode_call_conv(c: &mut Cursor<'_>) -> Result<CallConv, IrError> {
    let at = c.offset();
    Ok(match c.read_u8()? {
        0 => CallConv::Default,
        1 => CallConv::SystemV,
        2 => CallConv::WindowsX64,
        3 => CallConv::Fast,
        4 => CallConv::CDecl,
        5 => CallConv::Internal,
        6 => CallConv::Custom(u32::try_from(c.read_varint()?).map_err(|_| {
            IrError::BinaryDecode {
                offset: at,
                msg: "Custom 调用约定编号超出 u32".to_string(),
            }
        })?),
        7 => CallConv::Aapcs,
        8 => CallConv::AapcsVfp,
        9 => CallConv::RiscvIlp32,
        10 => CallConv::RiscvLp64,
        11 => CallConv::WasmBasic,
        12 => CallConv::StdCall,
        13 => CallConv::VectorCall,
        14 => CallConv::PreserveMost,
        15 => CallConv::PreserveAll,
        16 => CallConv::Cold,
        other => return err(at, format!("未知调用约定 tag {other}")),
    })
}

fn decode_data_layout(c: &mut Cursor<'_>) -> Result<DataLayout, IrError> {
    let at = c.offset();
    let endianness = match c.read_u8()? {
        0 => Endianness::Little,
        1 => Endianness::Big,
        other => return err(at, format!("未知端序 tag {other}")),
    };
    let at = c.offset();
    let mangling = match c.read_u8()? {
        0 => Mangling::Elf,
        1 => Mangling::MachO,
        2 => Mangling::WindowsCoff,
        other => return err(at, format!("未知 mangling tag {other}")),
    };

    let n_at = c.offset();
    let n = c.read_usize()?;
    if n > c.remaining() / 2 {
        return err(n_at, format!("指针布局表声明 {n} 项，剩余字节不足"));
    }
    let mut pointer_layout = std::collections::HashMap::with_capacity(n);
    for _ in 0..n {
        let k_at = c.offset();
        let space = c.read_usize()?;
        let size = c.read_usize()?;
        let align = c.read_usize()?;
        let key = u32::try_from(space).map_err(|_| IrError::BinaryDecode {
            offset: k_at,
            msg: "地址空间超出 u32".to_string(),
        })?;
        let value = (
            u32::try_from(size).map_err(|_| IrError::BinaryDecode {
                offset: k_at,
                msg: "指针大小超出 u32".to_string(),
            })?,
            u32::try_from(align).map_err(|_| IrError::BinaryDecode {
                offset: k_at,
                msg: "指针对齐超出 u32".to_string(),
            })?,
        );
        if pointer_layout.insert(key, value).is_some() {
            return err(k_at, format!("指针布局表 key {key} 重复"));
        }
    }

    let mut integer_alignments = std::collections::HashMap::new();
    let n_at = c.offset();
    let n = c.read_usize()?;
    if n > c.remaining() / 2 {
        return err(n_at, format!("整数对齐表声明 {n} 项，剩余字节不足"));
    }
    for _ in 0..n {
        let k_at = c.offset();
        let bits = c.read_usize()?;
        let align = c.read_usize()?;
        let key = u32::try_from(bits).map_err(|_| IrError::BinaryDecode {
            offset: k_at,
            msg: "整数位宽超出 u32".to_string(),
        })?;
        let value = u32::try_from(align).map_err(|_| IrError::BinaryDecode {
            offset: k_at,
            msg: "整数对齐超出 u32".to_string(),
        })?;
        if integer_alignments.insert(key, value).is_some() {
            return err(k_at, format!("整数对齐表 key {key} 重复"));
        }
    }

    let mut float_alignments = std::collections::HashMap::new();
    let n_at = c.offset();
    let n = c.read_usize()?;
    if n > c.remaining() / 2 {
        return err(n_at, format!("浮点对齐表声明 {n} 项，剩余字节不足"));
    }
    for _ in 0..n {
        let k_at = c.offset();
        let bits = c.read_usize()?;
        let align = c.read_usize()?;
        let key = u16::try_from(bits).map_err(|_| IrError::BinaryDecode {
            offset: k_at,
            msg: "浮点位宽超出 u16".to_string(),
        })?;
        let value = u32::try_from(align).map_err(|_| IrError::BinaryDecode {
            offset: k_at,
            msg: "浮点对齐超出 u32".to_string(),
        })?;
        if float_alignments.insert(key, value).is_some() {
            return err(k_at, format!("浮点对齐表 key {key} 重复"));
        }
    }

    let mut vector_alignments = std::collections::HashMap::new();
    let n_at = c.offset();
    let n = c.read_usize()?;
    if n > c.remaining() / 2 {
        return err(n_at, format!("向量对齐表声明 {n} 项，剩余字节不足"));
    }
    for _ in 0..n {
        let k_at = c.offset();
        let bits = c.read_usize()?;
        let align = c.read_usize()?;
        let key = u32::try_from(bits).map_err(|_| IrError::BinaryDecode {
            offset: k_at,
            msg: "向量位宽超出 u32".to_string(),
        })?;
        let value = u32::try_from(align).map_err(|_| IrError::BinaryDecode {
            offset: k_at,
            msg: "向量对齐超出 u32".to_string(),
        })?;
        if vector_alignments.insert(key, value).is_some() {
            return err(k_at, format!("向量对齐表 key {key} 重复"));
        }
    }

    let at = c.offset();
    let aggregate_align = u32::try_from(c.read_varint()?).map_err(|_| IrError::BinaryDecode {
        offset: at,
        msg: "聚合对齐超出 u32".to_string(),
    })?;
    let at = c.offset();
    let max_alignment = u32::try_from(c.read_varint()?).map_err(|_| IrError::BinaryDecode {
        offset: at,
        msg: "最大对齐超出 u32".to_string(),
    })?;

    let n_at = c.offset();
    let n = c.read_usize()?;
    if n > c.remaining() {
        return err(n_at, format!("原生整数宽度表声明 {n} 项，剩余字节不足"));
    }
    let mut native_integer_widths = Vec::with_capacity(n);
    for _ in 0..n {
        let at = c.offset();
        native_integer_widths.push(u32::try_from(c.read_varint()?).map_err(|_| {
            IrError::BinaryDecode {
                offset: at,
                msg: "原生整数宽度超出 u32".to_string(),
            }
        })?);
    }
    let n_at = c.offset();
    let n = c.read_usize()?;
    if n > c.remaining() {
        return err(n_at, format!("原生向量宽度表声明 {n} 项，剩余字节不足"));
    }
    let mut native_vector_widths = Vec::with_capacity(n);
    for _ in 0..n {
        let at = c.offset();
        native_vector_widths.push(u32::try_from(c.read_varint()?).map_err(|_| {
            IrError::BinaryDecode {
                offset: at,
                msg: "原生向量宽度超出 u32".to_string(),
            }
        })?);
    }

    let at = c.offset();
    let stack_align = u32::try_from(c.read_varint()?).map_err(|_| IrError::BinaryDecode {
        offset: at,
        msg: "栈对齐超出 u32".to_string(),
    })?;

    Ok(DataLayout {
        endianness,
        mangling,
        pointer_layout,
        integer_alignments,
        float_alignments,
        vector_alignments,
        aggregate_align,
        max_alignment,
        native_integer_widths,
        native_vector_widths,
        stack_align,
    })
}
