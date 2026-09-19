//! GLOBALS 段（全局变量/别名/comdat）与 MODULE 段（目标三元组/源文件/模块 asm）。
//!
//! - 全局/别名一律经 `Module::{add_global, add_global_alias, add_comdat}` 回放：
//!   它们会同步重建名字索引表（`func_names`/`global_names`/`comdat_names`），
//!   因此这三张表**不落盘**（落盘即双写源）；重名/重 comdat 由这些入口 fail-closed 拒绝。
//! - 全局的字符串载荷（`init_expr_text`、ifunc 参数与 resolver、段名等）都是 `ImmStr`，
//!   一律走 STRINGS 段按内容往返。
//! - MODULE 段只放"模块级事实"：`target_triple`（四段）、`source_filename`、`module_asm`。

use crate::entity::TypeId;
use crate::error::IrError;
use crate::ir::function::{GlobalAlias, GlobalVariable, Module};
use crate::ir::symbol::{ComdatKind, Linkage, SymbolInfo};
use crate::util::imm_str::ImmStr;

use super::format::SectionId;
use super::meta::{decode_attached, encode_attached};
use super::reader::Cursor;
use super::writer::{self, Writer};

fn err<T>(offset: usize, msg: impl Into<String>) -> Result<T, IrError> {
    Err(IrError::BinaryDecode {
        offset,
        msg: msg.into(),
    })
}

fn put_str(out: &mut Vec<u8>, w: &mut Writer, s: &ImmStr) {
    let idx = w.intern(s);
    writer::put_varint(out, u64::from(idx));
}

fn read_str(c: &mut Cursor<'_>, strings: &[ImmStr]) -> Result<ImmStr, IrError> {
    let at = c.offset();
    let idx = c.read_usize()?;
    strings.get(idx).cloned().ok_or(IrError::BinaryDecode {
        offset: at,
        msg: format!("字符串索引 {idx} 越界（表长 {}）", strings.len()),
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

fn read_u32(c: &mut Cursor<'_>, what: &str) -> Result<u32, IrError> {
    let at = c.offset();
    let v = c.read_usize()?;
    u32::try_from(v).map_err(|_| IrError::BinaryDecode {
        offset: at,
        msg: format!("{what} 超出 u32"),
    })
}

// ============================================================
// 符号信息（与 FUNCS 共用：GLOBALS 里也要写）
// ============================================================

pub(crate) fn encode_symbol(out: &mut Vec<u8>, w: &mut Writer, s: &SymbolInfo) {
    writer::put_u8(
        out,
        match s.linkage {
            Linkage::External => 0,
            Linkage::AvailableExternally => 1,
            Linkage::LinkOnceAny => 2,
            Linkage::LinkOnceODR => 3,
            Linkage::WeakAny => 4,
            Linkage::WeakODR => 5,
            Linkage::Appending => 6,
            Linkage::Internal => 7,
            Linkage::Private => 8,
            Linkage::ExternalWeak => 9,
            Linkage::Common => 10,
        },
    );
    writer::put_u8(
        out,
        match s.visibility {
            crate::ir::symbol::Visibility::Default => 0,
            crate::ir::symbol::Visibility::Hidden => 1,
            crate::ir::symbol::Visibility::Protected => 2,
        },
    );
    writer::put_u8(
        out,
        match s.dll_storage_class {
            crate::ir::symbol::DllStorageClass::Default => 0,
            crate::ir::symbol::DllStorageClass::DllImport => 1,
            crate::ir::symbol::DllStorageClass::DllExport => 2,
        },
    );
    writer::put_option_tag(out, s.section.is_some());
    if let Some(sec) = &s.section {
        put_str(out, w, sec);
    }
    writer::put_option_tag(out, s.comdat.is_some());
    if let Some(c) = s.comdat {
        writer::put_varint(out, u64::from(c.0));
    }
    writer::put_option_tag(out, s.tls_model.is_some());
    if let Some(t) = s.tls_model {
        writer::put_u8(
            out,
            match t {
                crate::ir::symbol::TlsModel::GeneralDynamic => 0,
                crate::ir::symbol::TlsModel::LocalDynamic => 1,
                crate::ir::symbol::TlsModel::InitialExec => 2,
                crate::ir::symbol::TlsModel::LocalExec => 3,
            },
        );
    }
    writer::put_u8(out, u8::from(s.unnamed_addr));
    writer::put_u8(out, u8::from(s.can_discard));
    writer::put_u8(out, u8::from(s.dso_local));
}

pub(crate) fn decode_symbol(c: &mut Cursor<'_>, strings: &[ImmStr]) -> Result<SymbolInfo, IrError> {
    use crate::ir::symbol::{DllStorageClass, TlsModel, Visibility};
    let at = c.offset();
    let linkage = match c.read_u8()? {
        0 => Linkage::External,
        1 => Linkage::AvailableExternally,
        2 => Linkage::LinkOnceAny,
        3 => Linkage::LinkOnceODR,
        4 => Linkage::WeakAny,
        5 => Linkage::WeakODR,
        6 => Linkage::Appending,
        7 => Linkage::Internal,
        8 => Linkage::Private,
        9 => Linkage::ExternalWeak,
        10 => Linkage::Common,
        other => return err(at, format!("未知 linkage tag {other}")),
    };
    let at = c.offset();
    let visibility = match c.read_u8()? {
        0 => Visibility::Default,
        1 => Visibility::Hidden,
        2 => Visibility::Protected,
        other => return err(at, format!("未知 visibility tag {other}")),
    };
    let at = c.offset();
    let dll_storage_class = match c.read_u8()? {
        0 => DllStorageClass::Default,
        1 => DllStorageClass::DllImport,
        2 => DllStorageClass::DllExport,
        other => return err(at, format!("未知 dll 存储类 tag {other}")),
    };
    let section = match read_bool(c)? {
        true => Some(read_str(c, strings)?),
        false => None,
    };
    let comdat = match read_bool(c)? {
        true => Some(crate::ir::symbol::ComdatId(read_u32(c, "comdat 引用")?)),
        false => None,
    };
    let tls_model = match read_bool(c)? {
        false => None,
        true => {
            let at = c.offset();
            Some(match c.read_u8()? {
                0 => TlsModel::GeneralDynamic,
                1 => TlsModel::LocalDynamic,
                2 => TlsModel::InitialExec,
                3 => TlsModel::LocalExec,
                other => return err(at, format!("未知 TLS 模型 tag {other}")),
            })
        }
    };
    Ok(SymbolInfo {
        linkage,
        visibility,
        dll_storage_class,
        section,
        comdat,
        tls_model,
        unnamed_addr: read_bool(c)?,
        can_discard: read_bool(c)?,
        dso_local: read_bool(c)?,
    })
}

// ============================================================
// GLOBALS
// ============================================================

pub(crate) fn encode_globals(module: &Module, w: &mut Writer) {
    let mut body = Vec::new();
    writer::put_varint(&mut body, module.global_count() as u64);
    for (_, g) in module.iter_globals() {
        put_str(&mut body, w, &g.name);
        writer::put_varint(&mut body, u64::from(g.ty.0));
        writer::put_option_tag(&mut body, g.init.is_some());
        if let Some(init) = &g.init {
            writer::put_len_prefixed(&mut body, init);
        }
        writer::put_option_tag(&mut body, g.init_expr_text.is_some());
        if let Some(t) = &g.init_expr_text {
            put_str(&mut body, w, t);
        }
        encode_symbol(&mut body, w, &g.symbol);
        writer::put_u8(&mut body, u8::from(g.is_constant));
        writer::put_varint(&mut body, u64::from(g.alignment));
        writer::put_varint(&mut body, u64::from(g.addr_space));
        encode_attached(&mut body, w, g.metadata());
        writer::put_u8(&mut body, u8::from(g.is_ifunc));
        writer::put_varint(&mut body, g.ifunc_params.len() as u64);
        for p in &g.ifunc_params {
            put_str(&mut body, w, p);
        }
        writer::put_option_tag(&mut body, g.ifunc_resolver.is_some());
        if let Some(r) = &g.ifunc_resolver {
            put_str(&mut body, w, r);
        }
    }

    writer::put_varint(&mut body, module.iter_global_aliases().count() as u64);
    for a in module.iter_global_aliases() {
        put_str(&mut body, w, &a.name);
        writer::put_varint(&mut body, u64::from(a.ty.0));
        writer::put_u8(
            &mut body,
            match a.linkage {
                Linkage::External => 0,
                Linkage::AvailableExternally => 1,
                Linkage::LinkOnceAny => 2,
                Linkage::LinkOnceODR => 3,
                Linkage::WeakAny => 4,
                Linkage::WeakODR => 5,
                Linkage::Appending => 6,
                Linkage::Internal => 7,
                Linkage::Private => 8,
                Linkage::ExternalWeak => 9,
                Linkage::Common => 10,
            },
        );
        writer::put_u8(&mut body, u8::from(a.dso_local));
        writer::put_u8(&mut body, u8::from(a.unnamed_addr));
        put_str(&mut body, w, &a.aliasee_text);
        encode_attached(&mut body, w, a.metadata());
    }

    writer::put_varint(&mut body, module.comdat_count() as u64);
    for i in 0..module.comdat_count() {
        let c = module.get_comdat(crate::ir::symbol::ComdatId(i as u32));
        put_str(&mut body, w, &c.name);
        writer::put_u8(
            &mut body,
            match c.kind {
                ComdatKind::Any => 0,
                ComdatKind::ExactMatch => 1,
                ComdatKind::Largest => 2,
                ComdatKind::NoDuplicates => 3,
                ComdatKind::SameSize => 4,
            },
        );
    }
    w.assign_section(SectionId::Globals, body);
}

pub(crate) fn decode_globals(
    module: &mut Module,
    mut c: Cursor<'_>,
    strings: &[ImmStr],
) -> Result<(), IrError> {
    let n_at = c.offset();
    let n = c.read_usize()?;
    if n > c.remaining() {
        return err(n_at, format!("全局变量声明 {n} 条，剩余字节不足"));
    }
    for _ in 0..n {
        let mut g = GlobalVariable::mutable("", TypeId::I32);
        g.name = read_str(&mut c, strings)?;
        let ty_at = c.offset();
        g.ty = TypeId(read_u32(&mut c, "全局类型")?);
        g.init = match read_bool(&mut c)? {
            true => Some(c.read_len_prefixed()?.to_vec()),
            false => None,
        };
        g.init_expr_text = match read_bool(&mut c)? {
            true => Some(read_str(&mut c, strings)?),
            false => None,
        };
        g.symbol = decode_symbol(&mut c, strings)?;
        g.is_constant = read_bool(&mut c)?;
        g.alignment = read_u32(&mut c, "全局对齐")?;
        g.addr_space = read_u32(&mut c, "全局地址空间")?;
        for m in decode_attached(&mut c, strings)? {
            g.attach_metadata(m);
        }
        g.is_ifunc = read_bool(&mut c)?;
        let p_at = c.offset();
        let pn = c.read_usize()?;
        if pn > c.remaining() {
            return err(p_at, format!("ifunc 参数声明 {pn} 条，剩余字节不足"));
        }
        for _ in 0..pn {
            g.ifunc_params.push(read_str(&mut c, strings)?);
        }
        g.ifunc_resolver = match read_bool(&mut c)? {
            true => Some(read_str(&mut c, strings)?),
            false => None,
        };
        let _ = ty_at;
        module.add_global(g).map_err(|e| IrError::BinaryDecode {
            offset: c.offset(),
            msg: format!("全局变量回放失败：{e}"),
        })?;
    }

    let n_at = c.offset();
    let n = c.read_usize()?;
    if n > c.remaining() {
        return err(n_at, format!("别名声明 {n} 条，剩余字节不足"));
    }
    for _ in 0..n {
        let mut a = GlobalAlias {
            name: ImmStr::default(),
            ty: TypeId::I32,
            linkage: Linkage::External,
            dso_local: false,
            unnamed_addr: false,
            aliasee_text: ImmStr::default(),
            metadata: Vec::new(),
        };
        a.name = read_str(&mut c, strings)?;
        a.ty = TypeId(read_u32(&mut c, "别名类型")?);
        let at = c.offset();
        a.linkage = match c.read_u8()? {
            0 => Linkage::External,
            1 => Linkage::AvailableExternally,
            2 => Linkage::LinkOnceAny,
            3 => Linkage::LinkOnceODR,
            4 => Linkage::WeakAny,
            5 => Linkage::WeakODR,
            6 => Linkage::Appending,
            7 => Linkage::Internal,
            8 => Linkage::Private,
            9 => Linkage::ExternalWeak,
            10 => Linkage::Common,
            other => return err(at, format!("未知别名 linkage tag {other}")),
        };
        a.dso_local = read_bool(&mut c)?;
        a.unnamed_addr = read_bool(&mut c)?;
        a.aliasee_text = read_str(&mut c, strings)?;
        for m in decode_attached(&mut c, strings)? {
            a.attach_metadata(m);
        }
        module
            .add_global_alias(a)
            .map_err(|e| IrError::BinaryDecode {
                offset: c.offset(),
                msg: format!("别名回放失败：{e}"),
            })?;
    }

    let n_at = c.offset();
    let n = c.read_usize()?;
    if n > c.remaining() {
        return err(n_at, format!("comdat 声明 {n} 条，剩余字节不足"));
    }
    for _ in 0..n {
        let name = read_str(&mut c, strings)?;
        let at = c.offset();
        let kind = match c.read_u8()? {
            0 => ComdatKind::Any,
            1 => ComdatKind::ExactMatch,
            2 => ComdatKind::Largest,
            3 => ComdatKind::NoDuplicates,
            4 => ComdatKind::SameSize,
            other => return err(at, format!("未知 comdat kind tag {other}")),
        };
        module
            .add_comdat(name.as_str(), kind)
            .map_err(|e| IrError::BinaryDecode {
                offset: c.offset(),
                msg: format!("comdat 回放失败：{e}"),
            })?;
    }
    Ok(())
}

// ============================================================
// MODULE
// ============================================================

pub(crate) fn encode_module_section(module: &Module, w: &mut Writer) {
    let mut body = Vec::new();
    writer::put_option_tag(&mut body, module.target_triple.is_some());
    if let Some(t) = &module.target_triple {
        put_str(&mut body, w, &t.arch);
        put_str(&mut body, w, &t.vendor);
        put_str(&mut body, w, &t.os);
        put_str(&mut body, w, &t.environment);
    }
    writer::put_option_tag(&mut body, module.source_filename.is_some());
    if let Some(s) = &module.source_filename {
        put_str(&mut body, w, s);
    }
    writer::put_varint(&mut body, module.module_asm.len() as u64);
    for asm in &module.module_asm {
        put_str(&mut body, w, asm);
    }
    w.assign_section(SectionId::Module, body);
}

pub(crate) fn decode_module_section(
    module: &mut Module,
    mut c: Cursor<'_>,
    strings: &[ImmStr],
) -> Result<(), IrError> {
    module.target_triple = match read_bool(&mut c)? {
        false => None,
        true => Some(crate::ir::data_layout::TargetTriple {
            arch: read_str(&mut c, strings)?,
            vendor: read_str(&mut c, strings)?,
            os: read_str(&mut c, strings)?,
            environment: read_str(&mut c, strings)?,
        }),
    };
    module.source_filename = match read_bool(&mut c)? {
        true => Some(read_str(&mut c, strings)?),
        false => None,
    };
    let n_at = c.offset();
    let n = c.read_usize()?;
    if n > c.remaining() {
        return err(n_at, format!("模块 asm 声明 {n} 条，剩余字节不足"));
    }
    for _ in 0..n {
        module.module_asm.push(read_str(&mut c, strings)?);
    }
    Ok(())
}
