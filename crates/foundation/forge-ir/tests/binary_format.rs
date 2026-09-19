//! 二进制序列化 B1：容器骨架（魔数/版本/producer/段表）+ STRINGS 段 + 负向对照。
//!
//! 断言分三类：
//!
//! 1. **往返**：`Module::new()` 与带字符串池的模块 ⇄ 字节流（含"表索引 0 恒为空串"
//!    这条规范化差异的精确断言）；
//! 2. **确定性**：两次编码逐字节相同；`decode → encode` 与首次编码逐字节相同；
//! 3. **fail-closed**：任意截断前缀、坏魔数、版本不符、未知段 id、未知 COMPAT
//!    flag 位、非法 UTF-8、重复字符串、段数炸弹 —— 全部 `Err`，且**不 panic**。
//!
//! 夹具的头部装配是**测试侧独立实现**（不调用 `writer::finish`），负向用例优先在真实
//! `to_binary` 产物上打补丁，保证测的是真编码器而不是夹具自己。

use forge_ir::util::string_pool::InternedStr;
use forge_ir::{IR_FORMAT_VERSION, ImmStr, IrError, Module, SectionId, check_binary_compat};

// ============================================================
// 测试侧工具（独立于被测实现）
// ============================================================

fn put_varint(out: &mut Vec<u8>, mut v: u64) {
    loop {
        let b = (v & 0x7f) as u8;
        v >>= 7;
        if v == 0 {
            out.push(b);
            return;
        }
        out.push(b | 0x80);
    }
}

fn varint_len(mut v: u64) -> usize {
    let mut n = 1;
    while v >= 0x80 {
        v >>= 7;
        n += 1;
    }
    n
}

fn read_varint(bytes: &[u8], pos: &mut usize) -> u64 {
    let mut result = 0u64;
    let mut shift = 0;
    loop {
        let b = bytes[*pos];
        *pos += 1;
        result |= u64::from(b & 0x7f) << shift;
        if b & 0x80 == 0 {
            return result;
        }
        shift += 7;
    }
}

/// 字符串池快照（按句柄顺序）。
fn pool_snapshot(m: &Module) -> Vec<String> {
    let store = m.types.borrow();
    (0..store.strings.len())
        .map(|i| store.strings.lookup(InternedStr(i as u32)).to_string())
        .collect()
}

/// 按内容查回字符串（解码后句柄重建，内容才是契约）。
fn lookup(m: &Module, text: &str) -> String {
    let mut store = m.types.borrow_mut();
    let handle = store.strings.intern(text);
    store.strings.lookup(handle).to_string()
}

/// 段表条目在字节流里的位置：`(id, id_pos, offset, len)`。
struct Entry {
    id: u8,
    id_pos: usize,
    offset: usize,
    len: usize,
}

/// 测试侧解析真实编码产物的头部 + 段表（用于"打补丁"式负向用例）。
fn parse_entries(bytes: &[u8]) -> Vec<Entry> {
    assert_eq!(&bytes[..8], b"FORGEIR\0", "魔数");
    let mut pos = 8;
    let _version = read_varint(bytes, &mut pos);
    let producer_len = read_varint(bytes, &mut pos) as usize;
    pos += producer_len;
    let count = read_varint(bytes, &mut pos) as usize;
    let mut entries = Vec::new();
    for _ in 0..count {
        let id_pos = pos;
        let id = bytes[pos];
        pos += 1;
        let offset = read_varint(bytes, &mut pos) as usize;
        let len = read_varint(bytes, &mut pos) as usize;
        entries.push(Entry {
            id,
            id_pos,
            offset,
            len,
        });
    }
    entries
}

/// 测试侧装配一个完整流（用于构造结构性损坏的字节流）。
fn build_stream(version: u64, producer: &str, sections: &[(u8, Vec<u8>)]) -> Vec<u8> {
    let mut header_len = 0usize;
    let mut offsets = vec![0u64; sections.len()];
    loop {
        let mut off = header_len as u64;
        let mut size = 8
            + varint_len(version)
            + varint_len(producer.len() as u64)
            + producer.len()
            + varint_len(sections.len() as u64);
        for (i, (_, body)) in sections.iter().enumerate() {
            offsets[i] = off;
            size += 1 + varint_len(off) + varint_len(body.len() as u64);
            off += body.len() as u64;
        }
        if size == header_len {
            break;
        }
        header_len = size;
    }
    let mut out = Vec::new();
    out.extend_from_slice(b"FORGEIR\0");
    put_varint(&mut out, version);
    put_varint(&mut out, producer.len() as u64);
    out.extend_from_slice(producer.as_bytes());
    put_varint(&mut out, sections.len() as u64);
    for (i, (id, body)) in sections.iter().enumerate() {
        out.push(*id);
        put_varint(&mut out, offsets[i]);
        put_varint(&mut out, body.len() as u64);
    }
    assert_eq!(out.len(), header_len, "夹具头部长度");
    for (_, body) in sections {
        out.extend_from_slice(body);
    }
    out
}

/// 字符串段体：`num` + 长度表 + 字节拼接。
fn strings_body(items: &[&str]) -> Vec<u8> {
    let mut body = Vec::new();
    put_varint(&mut body, items.len() as u64);
    for s in items {
        put_varint(&mut body, s.len() as u64);
    }
    for s in items {
        body.extend_from_slice(s.as_bytes());
    }
    body
}

/// 期望解码失败（成功即 panic）。
fn decode_err(bytes: &[u8]) -> IrError {
    match Module::from_binary(bytes) {
        Ok(_) => panic!("损坏的字节流必须被拒绝，实际解码成功"),
        Err(e) => e,
    }
}

fn msg_of(e: &IrError) -> String {
    match e {
        IrError::BinaryDecode { msg, .. } => msg.clone(),
        other => panic!("期望 BinaryDecode，实际 {other:?}"),
    }
}

// ============================================================
// 往返与确定性
// ============================================================

#[test]
fn empty_module_roundtrips() {
    let m = Module::new();
    let bytes = m.to_binary();
    let back = Module::from_binary(&bytes).expect("解码空模块");

    assert_eq!(back.function_count(), 0);
    assert_eq!(back.global_count(), 0);
    assert!(back.target_triple.is_none());
    assert!(back.source_filename.is_none());
    assert!(back.module_asm.is_empty(), "模块 asm 应为空");
    // 池逐条往返（空模块 ⇒ 空池；表不预留空串槽）。
    assert_eq!(pool_snapshot(&back), Vec::<String>::new());
    // DataLayout 暂不在 B1 落盘（B2 的 TYPES 段），故只比语义查询而非 Debug 文本
    // ——`DataLayout` 内部有 HashMap，`{:?}` 的键序随实例随机种子变化，不可作断言。
    let (before, after) = (m.data_layout(), back.data_layout());
    assert_eq!(after.pointer_size(0), before.pointer_size(0));
    assert_eq!(after.pointer_align(0), before.pointer_align(0));
    assert_eq!(after.integer_align(32), before.integer_align(32));
}

#[test]
fn string_pool_roundtrips_verbatim() {
    let m = Module::new();
    // 内联（SSO）与长串（Arc 共享）两条存储路径都覆盖；不含空串。
    let long_text = "a-very-long-type-name-exceeding-inline-capacity";
    m.types.borrow_mut().strings.intern("alpha");
    m.types.borrow_mut().strings.intern(long_text);
    assert_eq!(
        pool_snapshot(&m),
        vec!["alpha".to_string(), long_text.to_string()]
    );

    let bytes = m.to_binary();
    let back = Module::from_binary(&bytes).expect("解码");
    assert_eq!(
        pool_snapshot(&back),
        vec!["alpha".to_string(), long_text.to_string()],
        "解码后池与编码前逐条相同（顺序保持、无多余条目）"
    );
    // 内容可查回（句柄在解码期重建）
    assert_eq!(lookup(&back, "alpha"), "alpha");
    assert_eq!(lookup(&back, long_text), long_text);
}

#[test]
fn pool_containing_empty_string_still_roundtrips() {
    let m = Module::new();
    m.types.borrow_mut().strings.intern("alpha");
    m.types.borrow_mut().strings.intern(""); // 池里已有空串（指向表索引 0）
    let bytes = m.to_binary();
    let back = Module::from_binary(&bytes).expect("解码");
    // 池里有空串时也只是普通一条：内容 = {"alpha", ""}
    let mut snapshot = pool_snapshot(&back);
    snapshot.sort();
    assert_eq!(snapshot, vec![String::new(), "alpha".to_string()]);
    assert_eq!(lookup(&back, ""), "");
    assert_eq!(lookup(&back, "alpha"), "alpha");
}

#[test]
fn encoding_is_deterministic_and_idempotent() {
    let m = Module::new();
    m.types.borrow_mut().strings.intern("beta");
    m.types.borrow_mut().strings.intern("alpha");

    let first = m.to_binary();
    let second = m.to_binary();
    assert_eq!(first, second, "两次编码必须逐字节相同");

    let back = Module::from_binary(&first).expect("解码");
    assert_eq!(
        back.to_binary(),
        first,
        "decode → encode 必须与首次编码逐字节相同（编码幂等）"
    );
}

#[test]
fn to_binary_into_appends_without_touching_prefix() {
    let m = Module::new();
    let mut buf = b"PREFIX".to_vec();
    m.to_binary_into(&mut buf);
    assert_eq!(&buf[..6], b"PREFIX", "已有内容必须保留（追加语义）");
    assert_eq!(&buf[6..], m.to_binary().as_slice(), "追加的正是完整编码");
}

#[test]
fn check_binary_compat_reports_header() {
    let bytes = Module::new().to_binary();
    let compat = check_binary_compat(&bytes).expect("兼容检查");
    assert_eq!(compat.format_version, IR_FORMAT_VERSION);
    assert!(
        compat.producer.starts_with("forge-ir "),
        "producer 应带 crate 名：{}",
        compat.producer
    );
    assert_eq!(
        compat.sections,
        vec![
            SectionId::Compat,
            SectionId::Strings,
            SectionId::Types,
            SectionId::Consts,
            SectionId::Funcs
        ],
        "B1–B4 写 COMPAT + STRINGS + TYPES + CONSTS + FUNCS"
    );
}

// ============================================================
// B2：TYPES 段（条目 / 命名类型 / 签名 / DataLayout）
// ============================================================

/// 建一个"类型丰富"的模块：标量/向量/可扩展向量/数组/指针/命名与匿名 struct
/// （含字段名）/bfloat/token/metadata/opaque + 命名类型映射 + 签名表 +
/// 非默认 DataLayout。
fn rich_module() -> Module {
    use forge_ir::ir::types::{CallConv, FunctionSignature, TypeField};

    let mut m = Module::new();
    m.set_data_layout(forge_ir::ir::data_layout::DataLayout::x86_32_linux());
    {
        let mut store = m.types.borrow_mut();
        let i24 = store.int_ty(24);
        let f16 = store.float_ty(16);
        let bf16 = store.bfloat_ty(16);
        let _vec = store.vector_ty(i24, 3);
        let _svec = store.scalable_vector_ty(f16, 4);
        let _arr = store.array_ty(bf16, 7);
        let _ptr1 = store.pointer_ty(1);
        let _token = store.token_ty();
        let _meta = store.metadata_ty();
        let _opaque = store.opaque_ty();
        let field_name = store.intern_str("field0");
        let s = store.struct_named(
            "MyStruct",
            vec![TypeField::named(field_name, i24), TypeField::new(f16)],
            true,
        );
        store.define_named("MyStruct", s);
        let anon = store.struct_anon(vec![i24, f16], false);
        store.define_named("Anon", anon);
        let sig = FunctionSignature {
            params: vec![(i24, ImmStr::from("x")), (f16, ImmStr::from("y"))],
            returns: vec![i24, f16],
            calling_convention: CallConv::Custom(42),
            variadic: true,
        };
        let sr = store.register_signature(sig);
        assert_eq!(sr.index(), 0, "签名单一");
    }
    m
}

#[test]
fn rich_type_store_roundtrips() {
    let m = rich_module();
    let bytes = m.to_binary();
    let back = Module::from_binary(&bytes).expect("解码类型丰富的模块");

    let before = m.types.borrow();
    let after = back.types.borrow();
    assert_eq!(after.type_count(), before.type_count(), "条目数");
    for i in 0..before.type_count() {
        let id = forge_ir::TypeId::new(i as u32);
        assert_eq!(after.get(id), before.get(id), "条目 {i} 往返应逐字段相等");
    }
    // 命名类型映射（含"名字指向非 struct 条目"的情形）
    for name in ["MyStruct", "Anon"] {
        assert_eq!(
            after.lookup_named(name),
            before.lookup_named(name),
            "{name}"
        );
    }
    // 签名表
    assert!(
        after.signature_opt(forge_ir::SigRef::new(1)).is_none(),
        "签名表只应有一条"
    );
    let sig = after.get_signature(forge_ir::SigRef::new(0));
    assert_eq!(sig.params.len(), 2);
    assert_eq!(sig.params[0].1.as_str(), "x");
    assert_eq!(sig.params[1].1.as_str(), "y");
    assert_eq!(sig.returns.len(), 2);
    assert_eq!(
        sig.calling_convention,
        forge_ir::ir::types::CallConv::Custom(42)
    );
    assert!(sig.variadic);
    // DataLayout（逐字段比，避免 HashMap Debug 顺序造成的假失败）
    let (a, b) = (after.data_layout.clone(), before.data_layout.clone());
    assert_eq!(a.endianness, b.endianness);
    assert_eq!(a.mangling, b.mangling);
    assert_eq!(a.pointer_layout, b.pointer_layout);
    assert_eq!(a.integer_alignments, b.integer_alignments);
    assert_eq!(a.float_alignments, b.float_alignments);
    assert_eq!(a.vector_alignments, b.vector_alignments);
    assert_eq!(a.aggregate_align, b.aggregate_align);
    assert_eq!(a.max_alignment, b.max_alignment);
    assert_eq!(a.native_integer_widths, b.native_integer_widths);
    assert_eq!(a.native_vector_widths, b.native_vector_widths);
    assert_eq!(a.stack_align, b.stack_align);
}

#[test]
fn rich_module_encoding_is_idempotent() {
    let first = rich_module().to_binary();
    let back = Module::from_binary(&first).expect("解码");
    assert_eq!(
        back.to_binary(),
        first,
        "含类型/签名/布局的字节流同样必须编码幂等"
    );
}

#[test]
fn unknown_type_entry_tag_is_rejected() {
    // TYPES 段体：1 条目 + 未知 tag 99。
    let mut types = Vec::new();
    put_varint(&mut types, 1);
    types.push(99);
    let stream = build_stream(
        u64::from(IR_FORMAT_VERSION),
        "test 0.0.0",
        &[
            (SectionId::Compat.as_u8(), vec![0x00]),
            (SectionId::Strings.as_u8(), strings_body(&[""])),
            (SectionId::Types.as_u8(), types),
        ],
    );
    let e = decode_err(&stream);
    assert!(msg_of(&e).contains("未知类型条目"), "{e:?}");
}

#[test]
fn types_body_without_tail_is_rejected() {
    // 手工构造的 TYPES 段体只有"条目数 + 一条 Int"（缺命名表/签名表/DataLayout）
    // ⇒ 读完之前必须报错（fail-closed：不 panic、不"读到哪算哪"）。
    let mut types = Vec::new();
    put_varint(&mut types, 1);
    types.push(0); // Int
    put_varint(&mut types, 8);
    let stream = build_stream(
        u64::from(IR_FORMAT_VERSION),
        "test 0.0.0",
        &[
            (SectionId::Compat.as_u8(), vec![0x00]),
            (SectionId::Strings.as_u8(), strings_body(&[""])),
            (SectionId::Types.as_u8(), types),
        ],
    );
    let e = decode_err(&stream);
    assert!(matches!(e, IrError::BinaryDecode { .. }), "{e:?}");
}

// ============================================================
// B3：CONSTS 段（五通道常量）
// ============================================================

/// 常量池各通道的快照（只用公开 API：逐索引取到 `None` 为止）。
fn pool_snapshot_consts(pool: &forge_ir::ConstantPool) -> Vec<String> {
    use forge_ir::{AggId, ConstId};
    let mut out = Vec::new();
    let mut i = 0u32;
    while let Some((v, bits)) = pool.get_int(ConstId::pack(ConstId::TAG_INT, i)) {
        out.push(format!("int({v},{bits})"));
        i += 1;
    }
    let mut i = 0u32;
    while let Some((bits, w)) = pool.get_float_with_width(ConstId::pack(ConstId::TAG_FLOAT, i)) {
        out.push(format!("float({bits:#x},{w})"));
        i += 1;
    }
    let mut i = 0u32;
    while let Some(b) = pool.get_big(ConstId::pack(ConstId::TAG_BIG, i)) {
        out.push(format!("big({b:?})"));
        i += 1;
    }
    let mut i = 0u32;
    while let Some(data) = pool.get_vector(ConstId::pack(ConstId::TAG_VEC, i)) {
        let endian = pool
            .get_vector_endian(ConstId::pack(ConstId::TAG_VEC, i))
            .expect("端序与数据同生");
        out.push(format!("vec({data:?},{endian:?})"));
        i += 1;
    }
    let mut i = 0u32;
    while let Some(agg) = pool.get_aggregate(AggId::new(i)) {
        out.push(format!("agg({:?},{:?})", agg.ty, agg.children));
        i += 1;
    }
    out
}

/// 建一个"常量丰富"的模块：五通道都非空（含负 i128、f32/f64 同位模式、NaN 载荷、
/// 两种端序的向量、嵌套聚合）。
fn rich_consts_module() -> Module {
    use forge_ir::{AggChild, Big, ConstId, Endianness};

    let mut m = Module::new();
    let f32_bits = 0x3FC0_0000u128; // f32 1.5
    let nested_scalar;
    let agg_id;
    {
        let pool = &mut m.constants;
        // 预置 (0,1)/(1,1) 已在 index 0/1；再加边界值与负数。
        pool.insert_int(-42, 32);
        pool.insert_int(i128::MIN, 128);
        pool.insert_int(i128::MAX, 128);
        // 同一 u64 位模式在 f32/f64 下必须保持两条（值宽进 key）
        let a = pool.insert_float_typed(f32_bits, 32);
        let b = pool.insert_float_typed(f32_bits, 64);
        assert_ne!(a, b, "同一位模式不同值宽不得去重");
        pool.insert_float_typed(f64::NAN.to_bits() as u128, 64);
        pool.insert_float(1.5f64.to_bits());
        pool.insert_big(Big::from_i128(-12345678901234567890));
        pool.insert_big(Big::U_ZERO);
        pool.insert_big(Big::from_f64(1.5).expect("1.5 是有限实数"));
        pool.insert_vector(&[1, 2, 3, 4]);
        pool.insert_vector_with_endian(&[9, 9], Endianness::Big);
        nested_scalar = ConstId::pack(ConstId::TAG_INT, 0);
        let nested =
            pool.insert_aggregate(forge_ir::TypeId::I32, vec![AggChild::Scalar(nested_scalar)]);
        agg_id = pool.insert_aggregate(
            forge_ir::TypeId::I32,
            vec![AggChild::Scalar(nested_scalar), AggChild::Agg(nested)],
        );
    }
    // 让构造出的 id 一定被用到（避免"没插入"的假象）
    assert_eq!(agg_id.index(), 1, "两个聚合：嵌套子在前");
    let _ = nested_scalar;
    m
}

#[test]
fn constant_pool_roundtrips_all_channels() {
    let m = rich_consts_module();
    let bytes = m.to_binary();
    let back = Module::from_binary(&bytes).expect("解码常量模块");

    let before = pool_snapshot_consts(&m.constants);
    let after = pool_snapshot_consts(&back.constants);
    assert!(!before.is_empty());
    assert_eq!(after, before, "五通道逐条（含索引顺序）必须一致");
    assert_eq!(back.constants.total_len(), m.constants.total_len());
    assert_eq!(back.constants.vec_len(), m.constants.vec_len());
}

#[test]
fn consts_encoding_is_idempotent() {
    let first = rich_consts_module().to_binary();
    let back = Module::from_binary(&first).expect("解码");
    assert_eq!(back.to_binary(), first, "常量段同样必须编码幂等");
}

#[test]
fn empty_constants_pool_still_roundtrips() {
    // 空池仍带预置 bool 槽 ⇒ 往返后槽位不变（bool_const 的 id 仍有效）
    let m = Module::new();
    let back = Module::from_binary(&m.to_binary()).expect("解码");
    assert_eq!(
        pool_snapshot_consts(&back.constants),
        pool_snapshot_consts(&m.constants),
        "预置槽位必须原样保留"
    );
    assert_eq!(
        back.constants.bool_const(false),
        m.constants.bool_const(false)
    );
    assert_eq!(
        back.constants.bool_const(true),
        m.constants.bool_const(true)
    );
}

/// 测试侧装配 CONSTS 段体（各通道按格式顺序）。
struct ConstsFixture {
    ints: Vec<(i128, u32)>,
    floats: Vec<(u128, u16)>,
    bigs: Vec<Vec<u8>>,
    vecs: Vec<(Vec<u8>, u8)>,
    aggs: Vec<(u32, Vec<(u8, u64)>)>,
}

impl Default for ConstsFixture {
    fn default() -> Self {
        Self {
            ints: vec![(0, 1), (1, 1)],
            floats: Vec::new(),
            bigs: Vec::new(),
            vecs: Vec::new(),
            aggs: Vec::new(),
        }
    }
}

impl ConstsFixture {
    fn body(&self) -> Vec<u8> {
        let mut out = Vec::new();
        let put_i128 = |out: &mut Vec<u8>, v: i128| {
            let zz = ((v << 1) ^ (v >> 127)) as u128;
            let mut v = zz;
            loop {
                let b = (v & 0x7f) as u8;
                v >>= 7;
                if v == 0 {
                    out.push(b);
                    break;
                }
                out.push(b | 0x80);
            }
        };
        let put_u128 = |out: &mut Vec<u8>, mut v: u128| loop {
            let b = (v & 0x7f) as u8;
            v >>= 7;
            if v == 0 {
                out.push(b);
                break;
            }
            out.push(b | 0x80);
        };
        put_varint(&mut out, self.ints.len() as u64);
        for (v, bits) in &self.ints {
            put_i128(&mut out, *v);
            put_varint(&mut out, u64::from(*bits));
        }
        put_varint(&mut out, self.floats.len() as u64);
        for (bits, w) in &self.floats {
            put_u128(&mut out, *bits);
            put_varint(&mut out, u64::from(*w));
        }
        put_varint(&mut out, self.bigs.len() as u64);
        for b in &self.bigs {
            out.extend_from_slice(b);
        }
        put_varint(&mut out, self.vecs.len() as u64);
        for (data, endian) in &self.vecs {
            put_varint(&mut out, data.len() as u64);
            out.extend_from_slice(data);
            out.push(*endian);
        }
        put_varint(&mut out, self.aggs.len() as u64);
        for (ty, children) in &self.aggs {
            put_varint(&mut out, u64::from(*ty));
            put_varint(&mut out, children.len() as u64);
            for (tag, idx) in children {
                out.push(*tag);
                put_varint(&mut out, *idx);
            }
        }
        out
    }

    fn stream(&self) -> Vec<u8> {
        build_stream(
            u64::from(IR_FORMAT_VERSION),
            "test 0.0.0",
            &[
                (SectionId::Compat.as_u8(), vec![0x00]),
                (SectionId::Strings.as_u8(), strings_body(&[])),
                (SectionId::Consts.as_u8(), self.body()),
            ],
        )
    }
}

#[test]
fn duplicate_int_constant_is_rejected() {
    // 第三条与第一条相同 ⇒ insert_int 去重返回索引 0 ≠ 2 ⇒ Err
    let fixture = ConstsFixture {
        ints: vec![(0, 1), (1, 1), (0, 1)],
        ..Default::default()
    };
    let e = decode_err(&fixture.stream());
    assert!(msg_of(&e).contains("重复"), "{e:?}");
}

#[test]
fn unknown_big_variant_is_rejected() {
    let fixture = ConstsFixture {
        bigs: vec![vec![7u8]], // tag 7 未知
        ..Default::default()
    };
    let e = decode_err(&fixture.stream());
    assert!(msg_of(&e).contains("未知 Big 变体"), "{e:?}");
}

#[test]
fn aggregate_scalar_out_of_range_is_rejected() {
    // 标量子指向 big 通道 index 5（池里没有 big）
    let raw = forge_ir::ConstId::pack(forge_ir::ConstId::TAG_BIG, 5).raw();
    let fixture = ConstsFixture {
        aggs: vec![(4, vec![(0, u64::from(raw))])],
        ..Default::default()
    };
    let e = decode_err(&fixture.stream());
    assert!(msg_of(&e).contains("越界"), "{e:?}");
}

#[test]
fn aggregate_forward_reference_is_rejected() {
    // 聚合 0 引用聚合 0（自身）⇒ 未解码引用，必须错（防环）
    let fixture = ConstsFixture {
        aggs: vec![(4, vec![(1, 0)])],
        ..Default::default()
    };
    let e = decode_err(&fixture.stream());
    assert!(
        msg_of(&e).contains("未解码") || msg_of(&e).contains("越界"),
        "{e:?}"
    );
}

#[test]
fn unknown_vector_endian_is_rejected() {
    let fixture = ConstsFixture {
        vecs: vec![(vec![1, 2], 9)],
        ..Default::default()
    };
    let e = decode_err(&fixture.stream());
    assert!(msg_of(&e).contains("端序"), "{e:?}");
}

// ============================================================
// fail-closed 负向对照
// ============================================================

#[test]
fn every_truncated_prefix_is_rejected_without_panic() {
    let bytes = Module::new().to_binary();
    assert!(bytes.len() > 12, "样本太短，截断面不足");
    for cut in 0..bytes.len() {
        let e = decode_err(&bytes[..cut]);
        assert!(
            matches!(e, IrError::BinaryDecode { .. }),
            "截断到 {cut} 字节应报 BinaryDecode，实际 {e:?}"
        );
        assert!(
            check_binary_compat(&bytes[..cut]).is_err(),
            "截断到 {cut} 字节的兼容检查应失败"
        );
    }
}

#[test]
fn wrong_magic_is_rejected() {
    let mut bytes = Module::new().to_binary();
    bytes[0] ^= 0xff;
    let e = decode_err(&bytes);
    assert!(msg_of(&e).contains("魔数"), "{e:?}");
    match e {
        IrError::BinaryDecode { offset, .. } => assert_eq!(offset, 0, "魔数错误应指向偏移 0"),
        other => panic!("{other:?}"),
    }
}

#[test]
fn version_mismatch_is_rejected() {
    let mut bytes = Module::new().to_binary();
    // 版本 varint 紧跟在 8 字节魔数之后；v1 编码为单字节 0x01。
    assert_eq!(bytes[8], IR_FORMAT_VERSION as u8, "版本字节位置");
    bytes[8] = IR_FORMAT_VERSION as u8 + 1;
    let e = decode_err(&bytes);
    assert!(msg_of(&e).contains("格式版本"), "{e:?}");
    assert!(
        check_binary_compat(&bytes).is_err(),
        "版本不符时兼容检查也必须失败"
    );
}

#[test]
fn unknown_section_id_is_rejected() {
    let mut bytes = Module::new().to_binary();
    let entries = parse_entries(&bytes);
    let first = &entries[0];
    assert_eq!(first.id, SectionId::Compat.as_u8(), "首段应为 COMPAT");
    bytes[first.id_pos] = 0x7f; // 未知段 id
    let e = decode_err(&bytes);
    assert!(msg_of(&e).contains("未知段 id"), "{e:?}");
}

#[test]
fn unknown_compat_flag_bit_is_rejected() {
    let mut bytes = Module::new().to_binary();
    let entries = parse_entries(&bytes);
    let compat = entries
        .iter()
        .find(|e| e.id == SectionId::Compat.as_u8())
        .expect("COMPAT 段");
    assert_eq!(compat.len, 1, "v1 的 COMPAT 段体只有 flags 一字节");
    bytes[compat.offset] = 0x02; // 保留位被置位
    let e = decode_err(&bytes);
    assert!(msg_of(&e).contains("flag"), "{e:?}");
}

#[test]
fn duplicate_strings_are_rejected() {
    let stream = build_stream(
        u64::from(IR_FORMAT_VERSION),
        "test 0.0.0",
        &[
            (SectionId::Compat.as_u8(), vec![0x00]),
            (
                SectionId::Strings.as_u8(),
                strings_body(&["", "dup", "dup"]),
            ),
        ],
    );
    let e = decode_err(&stream);
    assert!(msg_of(&e).contains("重复"), "{e:?}");
}

#[test]
fn empty_string_table_is_accepted() {
    // num = 0（空池）合法：表不预留空串槽。
    let mut body = Vec::new();
    put_varint(&mut body, 0);
    let stream = build_stream(
        u64::from(IR_FORMAT_VERSION),
        "test 0.0.0",
        &[
            (SectionId::Compat.as_u8(), vec![0x00]),
            (SectionId::Strings.as_u8(), body),
        ],
    );
    let back = Module::from_binary(&stream).expect("空字符串表应被接受");
    assert!(back.types.borrow().strings.is_empty());
}

#[test]
fn invalid_utf8_in_string_table_is_rejected() {
    let mut body = Vec::new();
    put_varint(&mut body, 2); // num = 2
    put_varint(&mut body, 0); // 第 0 项 = 空串
    put_varint(&mut body, 2); // 第 1 项 2 字节
    body.extend_from_slice(&[0xff, 0xfe]);
    let stream = build_stream(
        u64::from(IR_FORMAT_VERSION),
        "test 0.0.0",
        &[
            (SectionId::Compat.as_u8(), vec![0x00]),
            (SectionId::Strings.as_u8(), body),
        ],
    );
    let e = decode_err(&stream);
    assert!(msg_of(&e).contains("UTF-8"), "{e:?}");
}

#[test]
fn missing_strings_section_is_rejected() {
    let stream = build_stream(
        u64::from(IR_FORMAT_VERSION),
        "test 0.0.0",
        &[(SectionId::Compat.as_u8(), vec![0x00])],
    );
    let e = decode_err(&stream);
    assert!(msg_of(&e).contains("STRINGS"), "{e:?}");
}

#[test]
fn section_count_bomb_is_rejected() {
    // 段数声明 100 万，但文件只剩几个字节 ⇒ 必须在分配前拒绝。
    let mut stream = Vec::new();
    stream.extend_from_slice(b"FORGEIR\0");
    put_varint(&mut stream, u64::from(IR_FORMAT_VERSION));
    put_varint(&mut stream, 5);
    stream.extend_from_slice(b"short");
    put_varint(&mut stream, 1_000_000);
    let e = decode_err(&stream);
    let msg = msg_of(&e);
    assert!(msg.contains("段表声明"), "{e:?}");
}

#[test]
fn binary_decode_error_display_carries_offset() {
    let mut bytes = Module::new().to_binary();
    bytes[0] ^= 0xff;
    let e = decode_err(&bytes);
    let text = e.to_string();
    assert!(text.contains("offset 0"), "{text}");
    assert!(text.contains("Binary decode error"), "{text}");
}
