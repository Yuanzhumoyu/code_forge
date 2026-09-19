//! FUNCS 段：函数表（签名/属性/符号 + 每函数常量池 + `dfg` + `layout`）。
//!
//! # 每个函数的记录布局
//!
//! ```text
//! name | signature | call_conv | attributes | extra_attrs
//! | symbol | is_const | personality | param_attrs | ret_attrs
//! | metadata | debug_info | value_names | block_names
//! | constants(五通道) | values | insts | blocks | layout | entry_block
//! ```
//!
//! # 为什么 `values` 要显式写 kind
//!
//! `DataFlowGraph` 里值的 dense 顺序 = "块参数先建、指令结果后建"。为了让"顺序即
//! 索引"在解码侧**可校验**而不是靠隐式假设，每个值都写一条显式 `ValueDef`
//! （`Inst(inst, k)` / `Param(block, k)` / `AggConst(AggId)` / `UndefNamed(str)`），
//! 解码后逐条回查：该值必须恰好是它所声称的那条指令的第 k 个结果 / 那个块的第 k 个
//! 参数（见 [`validate_dfg`]）。任何不一致 → `Err`，绝不"猜一个"。
//!
//! # use-lists
//!
//! **不落盘**（双写=两个真相）：解码后按"每条指令的操作数"重建
//! （`UseLists::record_inst`，终结符指令同样在其列）。

use crate::entity::{Block, ConstId, FuncRef, GlobalId, Inst, SigRef, TypeId, Value};
use crate::error::IrError;
use crate::ir::dfg::{BlockData, DataFlowGraph, Instruction, ValueData, ValueDef};
use crate::ir::function::Module;
use crate::ir::function::{Function, FunctionAttributes, ParamAttributes};
use crate::ir::immediate::Immediate;
use crate::ir::inst_flags::InstFlags;
use crate::ir::isel_strategy::IselStrategy;
use crate::ir::mem_flags::MemFlags;
use crate::ir::metadata::{AttachedMetadata, MetadataId, MetadataKind};
use crate::ir::symbol::{DllStorageClass, Linkage, SymbolInfo, TlsModel, Visibility};
use crate::util::imm_str::ImmStr;
use crate::util::string_pool::InternedStr;

use super::consts;
use super::format::SectionId;
use super::reader::Cursor;
use super::types::{decode_call_conv, encode_call_conv};
use super::writer::{self, Writer};

// ============================================================
// 小工具
// ============================================================

fn err<T>(offset: usize, msg: impl Into<String>) -> Result<T, IrError> {
    Err(IrError::BinaryDecode {
        offset,
        msg: msg.into(),
    })
}

/// v：句柄（`pub(crate)` 字段）→ varint。
fn put_handle(out: &mut Vec<u8>, v: u32) {
    writer::put_varint(out, u64::from(v));
}

/// Option<句柄>。
fn put_opt_handle(out: &mut Vec<u8>, v: Option<u32>) {
    writer::put_option_tag(out, v.is_some());
    if let Some(v) = v {
        writer::put_varint(out, u64::from(v));
    }
}

/// bool（1 字节）。
fn put_bool(out: &mut Vec<u8>, v: bool) {
    writer::put_u8(out, u8::from(v));
}

/// 字符串：登记进 STRINGS 段，写索引。
fn put_str(out: &mut Vec<u8>, w: &mut Writer, s: &ImmStr) {
    let idx = w.intern(s);
    writer::put_varint(out, u64::from(idx));
}

/// `Option<ImmStr>`。
fn put_opt_str(out: &mut Vec<u8>, w: &mut Writer, s: Option<&ImmStr>) {
    writer::put_option_tag(out, s.is_some());
    if let Some(s) = s {
        put_str(out, w, s);
    }
}

/// 读字符串表索引 → `ImmStr`。
fn read_str(c: &mut Cursor<'_>, strings: &[ImmStr]) -> Result<ImmStr, IrError> {
    let at = c.offset();
    let idx = c.read_usize()?;
    strings.get(idx).cloned().ok_or(IrError::BinaryDecode {
        offset: at,
        msg: format!("字符串索引 {idx} 越界（表长 {}）", strings.len()),
    })
}

/// 读字符串表索引 → 池句柄（解码期句柄 == 索引）。
fn read_interned(c: &mut Cursor<'_>, strings: &[ImmStr]) -> Result<InternedStr, IrError> {
    let at = c.offset();
    let idx = c.read_usize()?;
    if idx >= strings.len() {
        return err(
            at,
            format!("字符串索引 {idx} 越界（表长 {}）", strings.len()),
        );
    }
    Ok(InternedStr(idx as u32))
}

fn read_opt<T>(
    c: &mut Cursor<'_>,
    f: impl FnOnce(&mut Cursor<'_>) -> Result<T, IrError>,
) -> Result<Option<T>, IrError> {
    let at = c.offset();
    match c.read_u8()? {
        0 => Ok(None),
        1 => Ok(Some(f(c)?)),
        other => err(at, format!("Option 判别位应为 0/1，实测 {other}")),
    }
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

fn read_u16(c: &mut Cursor<'_>, what: &str) -> Result<u16, IrError> {
    let at = c.offset();
    let v = c.read_usize()?;
    u16::try_from(v).map_err(|_| IrError::BinaryDecode {
        offset: at,
        msg: format!("{what} 超出 u16"),
    })
}

// ============================================================
// 编码
// ============================================================

/// 写 FUNCS 段（段体缓冲由调用方装回 writer）。
pub(crate) fn encode_funcs(module: &Module, w: &mut Writer) {
    let mut body = Vec::new();
    writer::put_varint(&mut body, module.function_count() as u64);
    for func in module.iter_functions() {
        encode_function(&mut body, func, w);
    }
    w.assign_section(SectionId::Funcs, body);
}

fn encode_function(out: &mut Vec<u8>, f: &Function, w: &mut Writer) {
    // 一次读快照：`InternedStr`（值名/块名/immediate 字符串）都要按内容落表。
    let store = f.types.borrow();
    put_str(out, w, &f.name);
    put_handle(out, f.signature.0);
    encode_call_conv(out, f.calling_convention);
    writer::put_varint(out, u64::from(f.attributes.bits()));
    writer::put_varint(out, f.extra_attrs.len() as u64);
    for a in &f.extra_attrs {
        put_str(out, w, a);
    }
    encode_symbol(out, w, &f.symbol);
    put_bool(out, f.is_const);
    put_opt_handle(out, f.personality.map(|r| r.0));
    encode_param_attr_list(out, w, &f.param_attrs);
    encode_param_attr_list(out, w, &f.ret_attrs);
    writer::put_varint(out, f.metadata().len() as u64);
    for m in f.metadata() {
        encode_metadata_kind(out, w, &m.kind);
        put_handle(out, m.node.0);
    }
    // debug_info
    writer::put_option_tag(out, f.debug_info.is_some());
    if let Some(dbg) = &f.debug_info {
        writer::put_varint(out, dbg.locations.iter().count() as u64);
        for (v, loc) in dbg.locations.iter() {
            put_handle(out, v.0);
            encode_source_location(out, w, loc);
        }
        put_opt_str(out, w, dbg.function_name.as_ref());
    }
    // 名字表
    writer::put_varint(out, f.value_names.iter().count() as u64);
    for (v, name) in f.value_names.iter() {
        put_handle(out, v.0);
        put_str(out, w, &ImmStr::from(store.lookup_str(*name)));
    }
    writer::put_varint(out, f.block_names.iter().count() as u64);
    for (b, name) in f.block_names.iter() {
        put_handle(out, b.0);
        put_str(out, w, &ImmStr::from(store.lookup_str(*name)));
    }
    // 每函数常量池
    consts::encode_pool(&f.constants, out, w);
    // dfg
    encode_dfg(out, &f.dfg, &store, w);
    // layout + entry
    writer::put_varint(out, f.layout.block_order.len() as u64);
    for b in &f.layout.block_order {
        put_handle(out, b.0);
    }
    put_opt_handle(out, f.entry_block.map(|b| b.0));
}

fn encode_symbol(out: &mut Vec<u8>, w: &mut Writer, s: &SymbolInfo) {
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
            Visibility::Default => 0,
            Visibility::Hidden => 1,
            Visibility::Protected => 2,
        },
    );
    writer::put_u8(
        out,
        match s.dll_storage_class {
            DllStorageClass::Default => 0,
            DllStorageClass::DllImport => 1,
            DllStorageClass::DllExport => 2,
        },
    );
    put_opt_str(out, w, s.section.as_ref());
    put_opt_handle(out, s.comdat.map(|c| c.0));
    writer::put_option_tag(out, s.tls_model.is_some());
    if let Some(t) = s.tls_model {
        writer::put_u8(
            out,
            match t {
                TlsModel::GeneralDynamic => 0,
                TlsModel::LocalDynamic => 1,
                TlsModel::InitialExec => 2,
                TlsModel::LocalExec => 3,
            },
        );
    }
    put_bool(out, s.unnamed_addr);
    put_bool(out, s.can_discard);
    put_bool(out, s.dso_local);
}

fn encode_param_attr_list(out: &mut Vec<u8>, w: &mut Writer, list: &[ParamAttributes]) {
    writer::put_varint(out, list.len() as u64);
    for a in list {
        put_bool(out, a.zeroext);
        put_bool(out, a.signext);
        put_bool(out, a.noalias);
        put_bool(out, a.readonly);
        put_bool(out, a.writeonly);
        put_opt_handle(out, a.byval.map(|t| t.0));
        put_opt_handle(out, a.sret.map(|t| t.0));
        put_bool(out, a.inreg);
        put_bool(out, a.nocapture);
        put_bool(out, a.nonnull);
        writer::put_varint(out, u64::from(a.align));
        put_bool(out, a.noundef);
        writer::put_varint(out, a.extra.len() as u64);
        for e in &a.extra {
            put_str(out, w, e);
        }
    }
}

fn encode_metadata_kind(out: &mut Vec<u8>, w: &mut Writer, k: &MetadataKind) {
    writer::put_u8(
        out,
        match k {
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
    if let MetadataKind::Custom(name) = k {
        put_str(out, w, name);
    }
}

fn encode_source_location(
    out: &mut Vec<u8>,
    w: &mut Writer,
    loc: &crate::analysis::debug_info::SourceLocation,
) {
    put_opt_str(out, w, loc.file.as_ref());
    writer::put_varint(out, u64::from(loc.line.unwrap_or(u32::MAX)));
    writer::put_varint(out, u64::from(loc.column.unwrap_or(u32::MAX)));
}

fn encode_dfg(
    out: &mut Vec<u8>,
    dfg: &DataFlowGraph,
    store: &crate::ir::types::TypeStore,
    w: &mut Writer,
) {
    // --- values ---
    writer::put_varint(out, dfg.value_count() as u64);
    for (_, v) in dfg.values() {
        match v.def {
            ValueDef::Inst(i, k) => {
                writer::put_u8(out, 0);
                put_handle(out, i.0);
                writer::put_u8(out, k);
            }
            ValueDef::Param(b, k) => {
                writer::put_u8(out, 1);
                put_handle(out, b.0);
                writer::put_varint(out, u64::from(k));
            }
            ValueDef::AggConst(a) => {
                writer::put_u8(out, 2);
                put_handle(out, a.0);
            }
            ValueDef::UndefNamed(name) => {
                writer::put_u8(out, 3);
                put_str(out, w, &ImmStr::from(store.lookup_str(name)));
            }
        }
        put_handle(out, v.ty.0);
    }
    // --- insts ---
    writer::put_varint(out, dfg.inst_count() as u64);
    for (_, inst) in dfg.all_insts() {
        encode_inst(out, inst, store, w);
    }
    // --- blocks ---
    writer::put_varint(out, dfg.block_count() as u64);
    for (_, b) in dfg.blocks() {
        writer::put_varint(out, b.params.len() as u64);
        for t in &b.params {
            put_handle(out, t.0);
        }
        writer::put_varint(out, b.param_values.len() as u64);
        for v in &b.param_values {
            put_handle(out, v.0);
        }
        writer::put_varint(out, b.inst_order.len() as u64);
        for i in &b.inst_order {
            put_handle(out, i.0);
        }
        put_opt_handle(out, b.terminator_opt().map(|i| i.0));
    }
}

fn encode_inst(
    out: &mut Vec<u8>,
    inst: &Instruction,
    store: &crate::ir::types::TypeStore,
    w: &mut Writer,
) {
    // opcode 用**生成的名字**（ops.toml 是单一事实源；读侧 from_name 解析，
    // 未知名字即错 ⇒ 变体改名/新增都会显式报错，不会静默错位）。
    put_str(out, w, &ImmStr::from(inst.opcode.name()));
    put_handle(out, inst.block.0);
    writer::put_varint(out, inst.results.len() as u64);
    for r in &inst.results {
        put_handle(out, r.0);
    }
    writer::put_varint(out, inst.operands.len() as u64);
    for o in &inst.operands {
        put_handle(out, o.0);
    }
    writer::put_varint(out, inst.immediates.len() as u64);
    for im in &inst.immediates {
        encode_immediate(out, im, store, w);
    }
    writer::put_varint(out, u64::from(inst.flags.bits()));
    writer::put_varint(out, u64::from(inst.mem_flags.bits()));
    encode_param_attr_list(out, w, &inst.param_attrs);
    writer::put_varint(out, u64::from(inst.fn_attrs.bits()));
    writer::put_varint(out, inst.metadata().len() as u64);
    for m in inst.metadata() {
        encode_metadata_kind(out, w, &m.kind);
        put_handle(out, m.node.0);
    }
    writer::put_option_tag(out, inst.loc.is_some());
    if let Some(loc) = &inst.loc {
        encode_source_location(out, w, loc);
    }
    writer::put_option_tag(out, inst.isel_strategy.is_some());
    if let Some(s) = &inst.isel_strategy {
        put_str(out, w, &ImmStr::from(s.name()));
    }
    put_bool(out, inst.tombstone);
}

fn encode_immediate(
    out: &mut Vec<u8>,
    im: &Immediate,
    store: &crate::ir::types::TypeStore,
    w: &mut Writer,
) {
    match im {
        Immediate::Int(v) => {
            writer::put_u8(out, 0);
            writer::put_zigzag(out, *v);
        }
        Immediate::Uint(v) => {
            writer::put_u8(out, 1);
            writer::put_varint(out, *v);
        }
        Immediate::Const(c) => {
            writer::put_u8(out, 2);
            put_handle(out, c.raw());
        }
        Immediate::Block(b) => {
            writer::put_u8(out, 3);
            put_handle(out, b.0);
        }
        Immediate::Func(f) => {
            writer::put_u8(out, 4);
            put_handle(out, f.0);
        }
        Immediate::Global(g) => {
            writer::put_u8(out, 5);
            put_handle(out, g.0);
        }
        Immediate::Type(t) => {
            writer::put_u8(out, 6);
            put_handle(out, t.0);
        }
        Immediate::String(s) => {
            writer::put_u8(out, 7);
            put_str(out, w, &ImmStr::from(store.lookup_str(*s)));
        }
        Immediate::Agg(a) => {
            writer::put_u8(out, 8);
            put_handle(out, a.0);
        }
        Immediate::IntCC(cc) => {
            writer::put_u8(out, 9);
            writer::put_u8(out, cc.code());
        }
        Immediate::FloatCC(cc) => {
            writer::put_u8(out, 10);
            writer::put_u8(out, cc.code());
        }
    }
}

// ============================================================
// 解码
// ============================================================

/// 读 FUNCS 段并把函数加进模块。
pub(crate) fn decode_funcs(
    module: &mut Module,
    mut c: Cursor<'_>,
    strings: &[ImmStr],
    type_count: usize,
) -> Result<(), IrError> {
    let count_at = c.offset();
    let count = c.read_usize()?;
    if count > c.remaining() {
        return err(
            count_at,
            format!(
                "函数表声明 {count} 个函数，但剩余字节只有 {}",
                c.remaining()
            ),
        );
    }
    for _ in 0..count {
        let f = decode_function(&mut c, strings, type_count, module)?;
        module.add_function(f);
    }
    Ok(())
}

fn decode_function(
    c: &mut Cursor<'_>,
    strings: &[ImmStr],
    type_count: usize,
    module: &Module,
) -> Result<Function, IrError> {
    let name = read_str(c, strings)?;
    let sig_at = c.offset();
    let signature = SigRef(read_u32(c, "签名引用")?);
    if module.types.borrow().signature_opt(signature).is_none() {
        return err(
            sig_at,
            format!("函数 {name:?} 引用越界签名 SigRef({})", signature.0),
        );
    }
    let cc = decode_call_conv(c)?;
    let mut f = Function::new(name, module.types.clone(), signature, cc);

    f.attributes = FunctionAttributes::from_bits(read_u32(c, "函数属性")?);
    let n_at = c.offset();
    let n = c.read_usize()?;
    if n > c.remaining() {
        return err(n_at, format!("函数附加属性声明 {n} 条，剩余字节不足"));
    }
    for _ in 0..n {
        f.extra_attrs.push(read_str(c, strings)?);
    }
    f.symbol = decode_symbol(c, strings)?;
    f.is_const = read_bool(c)?;
    f.personality = read_opt(c, |c| Ok(FuncRef(read_u32(c, "personality 引用")?)))?;
    f.param_attrs = decode_param_attr_list(c, strings)?;
    f.ret_attrs = decode_param_attr_list(c, strings)?;
    let n_at = c.offset();
    let n = c.read_usize()?;
    if n > c.remaining() {
        return err(n_at, format!("函数 metadata 声明 {n} 条，剩余字节不足"));
    }
    for _ in 0..n {
        let kind = decode_metadata_kind(c, strings)?;
        let node = MetadataId(read_u32(c, "metadata 节点")?);
        f.attach_metadata(AttachedMetadata { kind, node });
    }
    // debug_info
    f.debug_info = read_opt(c, |c| {
        let n_at = c.offset();
        let n = c.read_usize()?;
        if n > c.remaining() {
            return err(n_at, format!("调试位置声明 {n} 条，剩余字节不足"));
        }
        let mut dbg = crate::analysis::debug_info::DebugInfo::new();
        for _ in 0..n {
            let at = c.offset();
            let v = Value(read_u32(c, "调试位置的 Value")?);
            if v.0 as usize >= u32::MAX as usize {
                return err(at, "调试位置 Value 越界");
            }
            let loc = decode_source_location(c, strings)?;
            dbg.locations.insert(v, loc);
        }
        dbg.function_name = read_opt(c, |c| read_str(c, strings))?;
        Ok(dbg)
    })?;
    // 名字表
    let n_at = c.offset();
    let n = c.read_usize()?;
    if n > c.remaining() {
        return err(n_at, format!("值名表声明 {n} 条，剩余字节不足"));
    }
    for _ in 0..n {
        let v = Value(read_u32(c, "值名表的 Value")?);
        let s = read_interned(c, strings)?;
        f.value_names.insert(v, s);
    }
    let n_at = c.offset();
    let n = c.read_usize()?;
    if n > c.remaining() {
        return err(n_at, format!("块名表声明 {n} 条，剩余字节不足"));
    }
    for _ in 0..n {
        let b = Block(read_u32(c, "块名表的 Block")?);
        let s = read_interned(c, strings)?;
        f.block_names.insert(b, s);
    }
    // 每函数常量池
    consts::decode_pool(&mut f.constants, c)?;
    // dfg
    decode_dfg(c, strings, &mut f.dfg, type_count)?;
    // layout + entry
    let n_at = c.offset();
    let n = c.read_usize()?;
    if n > c.remaining() {
        return err(n_at, format!("块布局声明 {n} 条，剩余字节不足"));
    }
    for _ in 0..n {
        f.layout.push(Block(read_u32(c, "布局里的 Block")?));
    }
    f.entry_block = read_opt(c, |c| Ok(Block(read_u32(c, "入口块")?)))?;

    // 结构校验（悬空句柄 / value kind 一致性）+ use-lists 重建
    let type_count_u32 = u32::try_from(type_count).unwrap_or(u32::MAX);
    validate_dfg(&f.dfg, type_count_u32, &f.layout.block_order, f.entry_block).map_err(|msg| {
        IrError::BinaryDecode {
            offset: c.offset(),
            msg,
        }
    })?;
    rebuild_use_lists(&mut f);
    Ok(f)
}

/// 重建 use-lists（终结符指令也在 `insts` 里 ⇒ 一并登记）。
fn rebuild_use_lists(f: &mut Function) {
    f.use_lists = crate::analysis::use_list::UseLists::new();
    for (inst, data) in f.dfg.all_insts() {
        let operands = data.operands.clone();
        f.use_lists.record_inst(inst, &operands);
    }
}

fn decode_symbol(c: &mut Cursor<'_>, strings: &[ImmStr]) -> Result<SymbolInfo, IrError> {
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
    let section = read_opt(c, |c| read_str(c, strings))?;
    let comdat = read_opt(c, |c| {
        Ok(crate::ir::symbol::ComdatId(read_u32(c, "comdat 引用")?))
    })?;
    let tls_model = read_opt(c, |c| {
        let at = c.offset();
        Ok(match c.read_u8()? {
            0 => TlsModel::GeneralDynamic,
            1 => TlsModel::LocalDynamic,
            2 => TlsModel::InitialExec,
            3 => TlsModel::LocalExec,
            other => return err(at, format!("未知 TLS 模型 tag {other}")),
        })
    })?;
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

fn decode_param_attr_list(
    c: &mut Cursor<'_>,
    strings: &[ImmStr],
) -> Result<Vec<ParamAttributes>, IrError> {
    let n_at = c.offset();
    let n = c.read_usize()?;
    if n > c.remaining() {
        return err(n_at, format!("参数属性声明 {n} 条，剩余字节不足"));
    }
    let mut out = Vec::with_capacity(n);
    for _ in 0..n {
        let mut a = ParamAttributes::new();
        a.zeroext = read_bool(c)?;
        a.signext = read_bool(c)?;
        a.noalias = read_bool(c)?;
        a.readonly = read_bool(c)?;
        a.writeonly = read_bool(c)?;
        a.byval = read_opt(c, |c| Ok(TypeId(read_u32(c, "byval 类型")?)))?;
        a.sret = read_opt(c, |c| Ok(TypeId(read_u32(c, "sret 类型")?)))?;
        a.inreg = read_bool(c)?;
        a.nocapture = read_bool(c)?;
        a.nonnull = read_bool(c)?;
        a.align = read_u32(c, "参数对齐")?;
        a.noundef = read_bool(c)?;
        let n_at = c.offset();
        let n = c.read_usize()?;
        if n > c.remaining() {
            return err(n_at, format!("参数附加属性声明 {n} 条，剩余字节不足"));
        }
        for _ in 0..n {
            a.extra.push(read_str(c, strings)?);
        }
        out.push(a);
    }
    Ok(out)
}

fn decode_metadata_kind(c: &mut Cursor<'_>, strings: &[ImmStr]) -> Result<MetadataKind, IrError> {
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
        13 => MetadataKind::Custom(read_str(c, strings)?),
        other => return err(at, format!("未知 metadata kind tag {other}")),
    })
}

fn decode_source_location(
    c: &mut Cursor<'_>,
    strings: &[ImmStr],
) -> Result<crate::analysis::debug_info::SourceLocation, IrError> {
    let file = read_opt(c, |c| read_str(c, strings))?;
    let line = read_u32(c, "源位置行号")?;
    let column = read_u32(c, "源位置列号")?;
    Ok(crate::analysis::debug_info::SourceLocation {
        file,
        line: if line == u32::MAX { None } else { Some(line) },
        column: if column == u32::MAX {
            None
        } else {
            Some(column)
        },
    })
}

fn decode_dfg(
    c: &mut Cursor<'_>,
    strings: &[ImmStr],
    dfg: &mut DataFlowGraph,
    type_count: usize,
) -> Result<(), IrError> {
    // --- values ---
    let n_at = c.offset();
    let n = c.read_usize()?;
    if n > c.remaining() {
        return err(n_at, format!("值表声明 {n} 条，剩余字节不足"));
    }
    for _ in 0..n {
        let at = c.offset();
        let tag = c.read_u8()?;
        let def = match tag {
            0 => {
                let i = Inst(read_u32(c, "值定义指令")?);
                let k_at = c.offset();
                let k = c.read_u8()?;
                if k >= 2 {
                    return err(
                        k_at,
                        format!("指令结果下标 {k} 越界（每条指令至多 2 个结果）"),
                    );
                }
                ValueDef::Inst(i, k)
            }
            1 => {
                let b = Block(read_u32(c, "值定义块")?);
                let k = read_u16(c, "块参数下标")?;
                ValueDef::Param(b, k)
            }
            2 => ValueDef::AggConst(crate::AggId(read_u32(c, "聚合常量")?)),
            3 => ValueDef::UndefNamed(read_interned(c, strings)?),
            other => return err(at, format!("未知值定义 tag {other}")),
        };
        let ty_at = c.offset();
        let ty = TypeId(read_u32(c, "值类型")?);
        if ty.0 as usize >= type_count {
            return err(
                ty_at,
                format!("值类型 TypeId({}) 越界（类型数 {type_count}）", ty.0),
            );
        }
        dfg.push_value_verbatim(ValueData { def, ty });
    }
    // --- insts ---
    let n_at = c.offset();
    let n = c.read_usize()?;
    if n > c.remaining() {
        return err(n_at, format!("指令表声明 {n} 条，剩余字节不足"));
    }
    for _ in 0..n {
        let inst = decode_inst(c, strings)?;
        dfg.push_inst_verbatim(inst);
    }
    // --- blocks ---
    let n_at = c.offset();
    let n = c.read_usize()?;
    if n > c.remaining() {
        return err(n_at, format!("块表声明 {n} 条，剩余字节不足"));
    }
    for _ in 0..n {
        let mut b = BlockData {
            params: Default::default(),
            param_values: Default::default(),
            inst_order: Vec::new(),
            terminator: None,
        };
        let n_at = c.offset();
        let pn = c.read_usize()?;
        if pn > c.remaining() {
            return err(n_at, format!("块参数声明 {pn} 条，剩余字节不足"));
        }
        for _ in 0..pn {
            b.params.push(TypeId(read_u32(c, "块参数类型")?));
        }
        let vn_at = c.offset();
        let vn = c.read_usize()?;
        if vn != pn {
            return err(vn_at, format!("块参数值数 {vn} 与类型数 {pn} 不一致"));
        }
        for _ in 0..vn {
            b.param_values.push(Value(read_u32(c, "块参数值")?));
        }
        let n_at = c.offset();
        let ino = c.read_usize()?;
        if ino > c.remaining() {
            return err(n_at, format!("块内指令声明 {ino} 条，剩余字节不足"));
        }
        for _ in 0..ino {
            b.inst_order.push(Inst(read_u32(c, "块内指令")?));
        }
        b.terminator = read_opt(c, |c| Ok(Inst(read_u32(c, "终结符指令")?)))?;
        dfg.push_block_verbatim(b);
    }
    Ok(())
}

fn decode_inst(c: &mut Cursor<'_>, strings: &[ImmStr]) -> Result<Instruction, IrError> {
    let op_at = c.offset();
    let opcode_name = read_str(c, strings)?;
    let opcode = crate::Opcode::from_name(opcode_name.as_str()).ok_or(IrError::BinaryDecode {
        offset: op_at,
        msg: format!("未知 opcode 名 {:?}", opcode_name.as_str()),
    })?;
    let block = Block(read_u32(c, "指令所属块")?);
    let mut inst = Instruction {
        opcode,
        block,
        results: Default::default(),
        operands: Default::default(),
        immediates: Default::default(),
        flags: InstFlags::empty(),
        mem_flags: MemFlags::empty(),
        param_attrs: Default::default(),
        fn_attrs: FunctionAttributes::NONE,
        metadata: Default::default(),
        loc: None,
        isel_strategy: None,
        tombstone: false,
    };
    let n_at = c.offset();
    let rn = c.read_usize()?;
    if rn > c.remaining() {
        return err(n_at, format!("指令结果声明 {rn} 条，剩余字节不足"));
    }
    for _ in 0..rn {
        inst.results.push(Value(read_u32(c, "指令结果")?));
    }
    let n_at = c.offset();
    let on = c.read_usize()?;
    if on > c.remaining() {
        return err(n_at, format!("指令操作数声明 {on} 条，剩余字节不足"));
    }
    for _ in 0..on {
        inst.operands.push(Value(read_u32(c, "指令操作数")?));
    }
    let n_at = c.offset();
    let imn = c.read_usize()?;
    if imn > c.remaining() {
        return err(n_at, format!("指令 immediate 声明 {imn} 条，剩余字节不足"));
    }
    for _ in 0..imn {
        inst.immediates.push(decode_immediate(c, strings)?);
    }
    inst.flags = InstFlags::from_bits_retain(read_u32(c, "指令 flags")?);
    inst.mem_flags = MemFlags::from_bits_retain(read_u16(c, "内存 flags")?);
    inst.param_attrs = decode_param_attr_list(c, strings)?.into();
    inst.fn_attrs = FunctionAttributes::from_bits(read_u32(c, "调用点函数属性")?);
    let n_at = c.offset();
    let mn = c.read_usize()?;
    if mn > c.remaining() {
        return err(n_at, format!("指令 metadata 声明 {mn} 条，剩余字节不足"));
    }
    for _ in 0..mn {
        let kind = decode_metadata_kind(c, strings)?;
        let node = MetadataId(read_u32(c, "metadata 节点")?);
        inst.attach_metadata(AttachedMetadata { kind, node });
    }
    inst.loc = read_opt(c, |c| decode_source_location(c, strings))?;
    inst.isel_strategy = read_opt(c, |c| {
        let at = c.offset();
        let name = read_str(c, strings)?;
        if name.is_empty() {
            return err(at, "指令选择标签不得为空");
        }
        Ok(IselStrategy::new(name))
    })?;
    inst.tombstone = read_bool(c)?;
    Ok(inst)
}

fn decode_immediate(c: &mut Cursor<'_>, strings: &[ImmStr]) -> Result<Immediate, IrError> {
    let at = c.offset();
    Ok(match c.read_u8()? {
        0 => Immediate::Int(c.read_zigzag()?),
        1 => Immediate::Uint(c.read_varint()?),
        2 => Immediate::Const(ConstId::from_raw(read_u32(c, "常量引用")?)),
        3 => Immediate::Block(Block(read_u32(c, "块引用")?)),
        4 => Immediate::Func(FuncRef(read_u32(c, "函数引用")?)),
        5 => Immediate::Global(GlobalId(read_u32(c, "全局引用")?)),
        6 => Immediate::Type(TypeId(read_u32(c, "类型引用")?)),
        7 => Immediate::String(read_interned(c, strings)?),
        8 => Immediate::Agg(crate::AggId(read_u32(c, "聚合引用")?)),
        9 => {
            let at = c.offset();
            let code = c.read_u8()?;
            Immediate::IntCC(crate::IntCC::from_code(code).ok_or(IrError::BinaryDecode {
                offset: at,
                msg: format!("未知整数条件码 {code}"),
            })?)
        }
        10 => {
            let at = c.offset();
            let code = c.read_u8()?;
            Immediate::FloatCC(
                crate::FloatCC::from_code(code).ok_or(IrError::BinaryDecode {
                    offset: at,
                    msg: format!("未知浮点条件码 {code}"),
                })?,
            )
        }
        other => return err(at, format!("未知 immediate tag {other}")),
    })
}

// ============================================================
// 结构校验
// ============================================================

/// 解码后的 `dfg` 结构校验：悬空句柄 + **value kind 与密集索引一致**。
///
/// 这是"顺序即索引"这条不变量在解码侧的显式检查点：每条 `ValueDef` 必须回指到
/// 真正拥有它的那条指令结果 / 那个块参数，且每条指令结果也必须回指到自己的值。
pub(crate) fn validate_dfg(
    dfg: &DataFlowGraph,
    type_count: u32,
    layout: &[Block],
    entry: Option<Block>,
) -> Result<(), String> {
    let value_count = dfg.value_count();
    let inst_count = dfg.inst_count();
    let block_count = dfg.block_count();
    if block_count > u32::MAX as usize
        || inst_count > u32::MAX as usize
        || value_count > u32::MAX as usize
    {
        return Err("dfg 规模超出 u32 句柄空间".to_string());
    }
    for vi in 0..value_count {
        let v = Value::new(vi as u32);
        let ty = dfg
            .value_type(v)
            .ok_or_else(|| format!("值 {vi} 无类型（arena 与计数不一致）"))?;
        if ty.0 >= type_count {
            return Err(format!(
                "值 {vi} 的类型 TypeId({}) 越界（类型数 {type_count}）",
                ty.0
            ));
        }
        let def = *dfg
            .value_def(v)
            .ok_or_else(|| format!("值 {vi} 无定义（arena 与计数不一致）"))?;
        match def {
            ValueDef::Inst(i, k) => {
                let (i, k) = (i.0 as usize, usize::from(k));
                let data = dfg.inst_data_opt(Inst::new(i as u32)).ok_or_else(|| {
                    format!("值 {vi} 声称由指令 {i} 定义，但指令只有 {inst_count} 条")
                })?;
                match data.results.get(k) {
                    Some(r) if r.0 as usize == vi => {}
                    other => {
                        return Err(format!(
                            "值 {vi} 声称是指令 {i} 的第 {k} 个结果，实际 {:?}",
                            other.map(|r| r.0)
                        ));
                    }
                }
            }
            ValueDef::Param(b, k) => {
                let (b, k) = (b.0 as usize, usize::from(k));
                let data = dfg.block_opt(Block::new(b as u32)).ok_or_else(|| {
                    format!("值 {vi} 声称是块 {b} 的参数，但块只有 {block_count} 个")
                })?;
                match data.param_values.get(k) {
                    Some(p) if p.0 as usize == vi => {}
                    other => {
                        return Err(format!(
                            "值 {vi} 声称是块 {b} 的第 {k} 个参数，实际 {:?}",
                            other.map(|p| p.0)
                        ));
                    }
                }
            }
            ValueDef::AggConst(_) | ValueDef::UndefNamed(_) => {}
        }
    }
    for (inst, data) in dfg.all_insts() {
        let ii = inst.0 as usize;
        if data.block.0 as usize >= block_count {
            return Err(format!("指令 {ii} 属于越界块 {}", data.block.0));
        }
        for r in &data.results {
            if r.0 as usize >= value_count {
                return Err(format!(
                    "指令 {ii} 的结果 {} 越界（值数 {value_count}）",
                    r.0
                ));
            }
            match dfg.value_def(*r) {
                Some(ValueDef::Inst(owner, k))
                    if owner.0 as usize == ii && data.results.get(*k as usize) == Some(r) => {}
                other => {
                    return Err(format!(
                        "指令 {ii} 的结果 {} 的定义回指不一致：{other:?}",
                        r.0
                    ));
                }
            }
        }
        for o in &data.operands {
            if o.0 as usize >= value_count {
                return Err(format!(
                    "指令 {ii} 的操作数 {} 越界（值数 {value_count}）",
                    o.0
                ));
            }
        }
    }
    for (blk, b) in dfg.blocks() {
        let bi = blk.0 as usize;
        if b.params.len() != b.param_values.len() {
            return Err(format!(
                "块 {bi} 的类型数 {} 与参数值数 {} 不一致",
                b.params.len(),
                b.param_values.len()
            ));
        }
        for t in &b.params {
            if t.0 >= type_count {
                return Err(format!("块 {bi} 的参数类型 TypeId({}) 越界", t.0));
            }
        }
        for p in &b.param_values {
            if p.0 as usize >= value_count {
                return Err(format!("块 {bi} 的参数值 {} 越界", p.0));
            }
        }
        for i in &b.inst_order {
            if i.0 as usize >= inst_count {
                return Err(format!("块 {bi} 的顺序表含越界指令 {}", i.0));
            }
        }
        if let Some(t) = b.terminator_opt()
            && t.0 as usize >= inst_count
        {
            return Err(format!("块 {bi} 的终结符 {} 越界", t.0));
        }
    }
    for b in layout {
        if b.0 as usize >= block_count {
            return Err(format!("块布局含越界块 {}", b.0));
        }
    }
    if let Some(e) = entry
        && e.0 as usize >= block_count
    {
        return Err(format!("入口块 {} 越界", e.0));
    }
    Ok(())
}
