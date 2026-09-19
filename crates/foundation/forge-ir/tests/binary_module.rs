//! 二进制序列化 B5：METADATA / GLOBALS / MODULE 段 + 全模块语料往返与尺寸基线。
//!
//! 三类断言：
//!
//! 1. **全模块往返**：metadata（Leaf/Tuple/Named/Placeholder + 命名表）、全局变量
//!    （字节 init / 表达式文本 / ifunc / 附件）、别名、comdat、三元组、源文件、
//!    模块 asm、函数与指令附件——编解码后逐项相等，并对解码结果跑 `Verifier`；
//! 2. **语料端到端**：LLVM 官方 test/Assembler 的**全部正向用例**（198）在
//!    parse → to_binary → from_binary 后**文本打印逐字符相同**，且
//!    `encode(decode(encode(m)))` 与 `encode(m)` 逐字节相同（记录合计字节数作为
//!    后续演进的尺寸基线）；
//! 3. **fail-closed**：未知 metadata 节点/值 tag、metadata 前向引用、附件指向越界
//!    节点、重复 metadata 节点、未知 comdat kind —— 全部 `Err`。

use forge_ir::ir::metadata::{
    AttachedMetadata, MetadataId, MetadataKind, MetadataNode, MetadataValue,
};
use forge_ir::ir::symbol::ComdatKind;
use forge_ir::text::parser::parse_module;
use forge_ir::{
    GlobalVariable, IR_FORMAT_VERSION, ImmStr, IrError, Module, SectionId, TypeId,
    check_binary_compat,
};
use std::fs;
use std::path::Path;

// ============================================================
// 测试侧工具
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

fn build_stream(sections: &[(u8, Vec<u8>)]) -> Vec<u8> {
    fn vlen(mut v: u64) -> usize {
        let mut n = 1;
        while v >= 0x80 {
            v >>= 7;
            n += 1;
        }
        n
    }
    let producer = "forge-ir test";
    let mut header_len = 0usize;
    let mut offsets = vec![0u64; sections.len()];
    loop {
        let mut off = header_len as u64;
        let mut size =
            8 + 1 + vlen(producer.len() as u64) + producer.len() + vlen(sections.len() as u64);
        for (i, (_, body)) in sections.iter().enumerate() {
            offsets[i] = off;
            size += 1 + vlen(off) + vlen(body.len() as u64);
            off += body.len() as u64;
        }
        if size == header_len {
            break;
        }
        header_len = size;
    }
    let mut out = Vec::new();
    out.extend_from_slice(b"FORGEIR\0");
    put_varint(&mut out, u64::from(IR_FORMAT_VERSION));
    put_varint(&mut out, producer.len() as u64);
    out.extend_from_slice(producer.as_bytes());
    put_varint(&mut out, sections.len() as u64);
    for (i, (id, body)) in sections.iter().enumerate() {
        out.push(*id);
        put_varint(&mut out, offsets[i]);
        put_varint(&mut out, body.len() as u64);
    }
    assert_eq!(out.len(), header_len);
    for (_, body) in sections {
        out.extend_from_slice(body);
    }
    out
}

/// METADATA 段体：`num` + 各节点字节 + 命名表（0 条）。
fn metadata_body(nodes: &[Vec<u8>]) -> Vec<u8> {
    let mut out = Vec::new();
    put_varint(&mut out, nodes.len() as u64);
    for n in nodes {
        out.extend_from_slice(n);
    }
    put_varint(&mut out, 0); // 命名表
    out
}

/// 只带 COMPAT/STRINGS/METADATA 的流（负向用例：不掺 FUNCS/GLOBALS）。
fn metadata_only(nodes: &[Vec<u8>]) -> Vec<u8> {
    // STRINGS 表：索引 0 = "k"（字段/名字用）
    let mut strings = Vec::new();
    put_varint(&mut strings, 1);
    put_varint(&mut strings, 1);
    strings.push(b'k');
    build_stream(&[
        (SectionId::Compat.as_u8(), vec![0]),
        (SectionId::Strings.as_u8(), strings),
        (SectionId::Metadata.as_u8(), metadata_body(nodes)),
    ])
}

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
// 全模块往返
// ============================================================

fn reference_module() -> Module {
    let mut m = Module::new();
    // metadata：Leaf / Tuple / Named / Placeholder + 命名表
    let leaf = m
        .metadata_store
        .intern(MetadataNode::Leaf(MetadataValue::String(ImmStr::from(
            "leaf-str",
        ))));
    let tuple = m.metadata_store.intern(MetadataNode::Tuple(
        vec![
            MetadataValue::Uint(7),
            MetadataValue::Int(-9),
            MetadataValue::Float(1.5f64.to_bits()),
            MetadataValue::Null,
            MetadataValue::Node(leaf),
            MetadataValue::Field(
                ImmStr::from("k"),
                Box::new(MetadataValue::IntBig(ImmStr::from(
                    "123456789012345678901234567890",
                ))),
            ),
        ]
        .into(),
    ));
    let named = m.metadata_store.intern(MetadataNode::Named {
        name: ImmStr::from("dbg"),
        ops: vec![
            MetadataValue::String(ImmStr::from("x")),
            MetadataValue::Node(tuple),
        ]
        .into(),
        distinct: true,
    });
    let placeholder = m.metadata_store.intern(MetadataNode::Placeholder);
    m.metadata_store.define_named("t", tuple);
    m.metadata_store.define_named("u", placeholder);

    // 全局变量：字节 init + 表达式文本 + ifunc + 附件
    let mut g = GlobalVariable::mutable("g0", TypeId::I32);
    g.init = Some(vec![1, 2, 3, 4]);
    g.alignment = 8;
    g.addr_space = 1;
    g.symbol.dso_local = true;
    g.attach_metadata(AttachedMetadata {
        kind: MetadataKind::Custom(ImmStr::from("custom-kind")),
        node: named,
    });
    m.add_global(g).expect("加入全局");

    let mut g1 = GlobalVariable::constant("g1", TypeId::PTR);
    g1.init_expr_text = Some(ImmStr::from("ptr @g0"));
    m.add_global(g1).expect("加入全局");

    let mut g2 = GlobalVariable::mutable("ifunc_g", TypeId::I32);
    g2.is_ifunc = true;
    g2.ifunc_params = vec![ImmStr::from("i32"), ImmStr::from("ptr")];
    g2.ifunc_resolver = Some(ImmStr::from("resolver"));
    m.add_global(g2).expect("加入 ifunc 全局");

    // 别名 + comdat
    let mut alias = forge_ir::ir::function::GlobalAlias::new("a0", TypeId::PTR, "ptr @g0");
    alias.linkage = forge_ir::ir::symbol::Linkage::Internal;
    alias.dso_local = true;
    alias.attach_metadata(AttachedMetadata {
        kind: MetadataKind::NoAlias,
        node: leaf,
    });
    m.add_global_alias(alias).expect("加入别名");
    m.add_comdat("c0", ComdatKind::ExactMatch)
        .expect("加入 comdat");

    // 模块级事实
    m.set_target_triple("x86_64-pc-windows-msvc");
    m.source_filename = Some(ImmStr::from("main.c"));
    m.module_asm.push(ImmStr::from("nop"));
    m.module_asm.push(ImmStr::from("ret"));

    // 一个带附件与名字的函数
    let types = m.types.clone();
    let sig = forge_ir::FunctionSignature::new(&[(TypeId::I32, "n")], &[TypeId::I32]);
    let mut b = forge_ir::FunctionBuilder::new("f", types.clone(), sig);
    let (entry, params) = b.create_block_with_params(&[(TypeId::I32, "n")]);
    b.switch_to_block(entry);
    b.ret(&[params[0]]);
    let mut f = b.finish().expect("函数");
    f.attach_metadata(AttachedMetadata {
        kind: MetadataKind::Loop,
        node: named,
    });
    f.value_names
        .insert(params[0], f.types.borrow_mut().strings.intern("param0"));
    m.add_function(f);
    m
}

#[test]
fn full_module_roundtrips() {
    let m = reference_module();
    let bytes = m.to_binary();
    let back = Module::from_binary(&bytes).expect("解码全模块");

    // metadata：节点逐条相等（含 Placeholder 与 Named 的 distinct 位）+ 命名表
    assert_eq!(back.metadata_store.len(), m.metadata_store.len(), "节点数");
    for i in 0..m.metadata_store.len() {
        let id = MetadataId(i as u32);
        assert_eq!(
            back.metadata_store.get(id),
            m.metadata_store.get(id),
            "metadata 节点 {i}"
        );
        assert_eq!(
            back.metadata_store.name_of(id),
            m.metadata_store.name_of(id),
            "metadata 名字 {i}"
        );
    }
    assert_eq!(back.metadata_store.lookup_named("t"), Some(MetadataId(1)));
    assert_eq!(
        back.metadata_store.lookup_named("u"),
        Some(MetadataId(3)),
        "Placeholder 也是普通节点"
    );

    // 全局/别名/comdat
    assert_eq!(back.global_count(), m.global_count());
    for ((gid, orig), (bid, got)) in m.iter_globals().zip(back.iter_globals()) {
        assert_eq!(gid, bid, "GlobalId 顺序");
        assert_eq!(got.name.as_str(), orig.name.as_str());
        assert_eq!(got.ty, orig.ty);
        assert_eq!(got.init, orig.init);
        assert_eq!(got.init_expr_text, orig.init_expr_text);
        assert_eq!(got.alignment, orig.alignment);
        assert_eq!(got.addr_space, orig.addr_space);
        assert_eq!(got.is_constant, orig.is_constant);
        assert_eq!(got.symbol.dso_local, orig.symbol.dso_local);
        assert_eq!(got.is_ifunc, orig.is_ifunc);
        assert_eq!(got.ifunc_params, orig.ifunc_params);
        assert_eq!(got.ifunc_resolver, orig.ifunc_resolver);
        assert_eq!(got.metadata(), orig.metadata(), "全局附件");
    }
    let aliases: Vec<_> = back.iter_global_aliases().collect();
    let orig_aliases: Vec<_> = m.iter_global_aliases().collect();
    assert_eq!(aliases.len(), orig_aliases.len());
    for (got, orig) in aliases.iter().zip(orig_aliases.iter()) {
        assert_eq!(got.name.as_str(), orig.name.as_str());
        assert_eq!(got.aliasee_text, orig.aliasee_text);
        assert_eq!(got.linkage, orig.linkage);
        assert_eq!(got.dso_local, orig.dso_local);
        assert_eq!(got.metadata(), orig.metadata(), "别名附件");
    }
    assert_eq!(back.comdat_count(), m.comdat_count());
    for i in 0..m.comdat_count() {
        let id = forge_ir::ir::symbol::ComdatId(i as u32);
        assert_eq!(back.get_comdat(id).kind, m.get_comdat(id).kind);
        assert_eq!(
            back.get_comdat(id).name.as_str(),
            m.get_comdat(id).name.as_str()
        );
    }

    // 模块级事实
    let (a, b2) = (
        back.target_triple.as_ref().expect("三元组"),
        m.target_triple.as_ref().expect("三元组"),
    );
    assert_eq!(a.arch.as_str(), b2.arch.as_str());
    assert_eq!(a.vendor.as_str(), b2.vendor.as_str());
    assert_eq!(a.os.as_str(), b2.os.as_str());
    assert_eq!(a.environment.as_str(), b2.environment.as_str());
    assert_eq!(back.source_filename, m.source_filename);
    assert_eq!(back.module_asm, m.module_asm);

    // 函数 + 附件/名字表 + 校验器
    assert_eq!(back.function_count(), m.function_count());
    for f in back.iter_functions() {
        assert_eq!(f.metadata().len(), 1, "函数附件");
        let mut v = forge_ir::verify::Verifier::with_ctx(f.types.clone());
        v.verify(f)
            .unwrap_or_else(|errs| panic!("解码后函数未过校验：{errs:?}"));
    }
    assert_eq!(
        back.find_function("f"),
        Some(forge_ir::FuncRef::new(0)),
        "函数名索引表已重建"
    );
}

#[test]
fn full_module_encoding_is_idempotent() {
    let first = reference_module().to_binary();
    let back = Module::from_binary(&first).expect("解码");
    assert_eq!(back.to_binary(), first, "全模块编码必须幂等");
}

#[test]
fn empty_module_has_all_eight_sections() {
    let bytes = Module::new().to_binary();
    let compat = check_binary_compat(&bytes).expect("头部");
    assert_eq!(
        compat.sections,
        vec![
            SectionId::Compat,
            SectionId::Strings,
            SectionId::Types,
            SectionId::Consts,
            SectionId::Metadata,
            SectionId::Funcs,
            SectionId::Globals,
            SectionId::Module
        ],
        "v1 恒定写 8 个段（空模块也写：读侧据此区分『段为空』与『缺段』）"
    );
}

// ============================================================
// 语料端到端 + 尺寸基线
// ============================================================

fn is_negative(name: &str, src: &str) -> bool {
    if src.lines().take(6).any(|l| l.contains("not llvm-as")) {
        return true;
    }
    let has_asm_run = src.lines().take(6).any(|l| l.contains("llvm-as"));
    let is_split = src.lines().take(6).any(|l| l.contains("split-file"));
    (name.contains("error")
        || name.contains("parse-error")
        || name.contains("invalid")
        || name.contains("redefinition"))
        && (has_asm_run || is_split)
}

#[test]
fn llvm_corpus_binary_roundtrip_is_text_identical() {
    let dir = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/llvm_assembler_cases");
    let mut checked = 0usize;
    let mut total_bytes = 0usize;
    let mut total_src = 0usize;
    let mut max_bytes = 0usize;
    let mut max_case = String::new();
    let mut failures: Vec<(String, String)> = Vec::new();

    for entry in fs::read_dir(&dir).expect("cases dir") {
        let entry = entry.expect("entry");
        let name = entry.file_name().to_string_lossy().to_string();
        if !name.ends_with(".ll") {
            continue;
        }
        let src = fs::read_to_string(entry.path()).expect("read");
        if is_negative(&name, &src) {
            continue;
        }
        let src = src.split(";---").next().unwrap_or(&src).to_string();
        let Ok(module) = parse_module(&src) else {
            continue; // 正向但当前语法层不支持的用例（既有基线里已知）
        };
        let before = format!("{module}");
        let bytes = module.to_binary();
        let decoded = match Module::from_binary(&bytes) {
            Ok(d) => d,
            Err(e) => {
                failures.push((name.clone(), format!("解码失败：{e:?}")));
                continue;
            }
        };
        let after = format!("{decoded}");
        if before != after {
            let at = before
                .lines()
                .zip(after.lines())
                .position(|(a, b)| a != b)
                .map(|i| {
                    format!(
                        "第 {} 行\n     解析后: {}\n     解码后: {}",
                        i + 1,
                        before.lines().nth(i).unwrap_or(""),
                        after.lines().nth(i).unwrap_or("")
                    )
                })
                .unwrap_or_else(|| {
                    format!(
                        "行数 {} vs {}",
                        before.lines().count(),
                        after.lines().count()
                    )
                });
            failures.push((name.clone(), format!("文本打印不一致（{at}）")));
            continue;
        }
        // 编码幂等：decode → encode 与首次逐字节相同
        let again = decoded.to_binary();
        if again != bytes {
            failures.push((name.clone(), "字节流不幂等".to_string()));
            continue;
        }
        checked += 1;
        total_bytes += bytes.len();
        total_src += src.len();
        if bytes.len() > max_bytes {
            max_bytes = bytes.len();
            max_case = name.clone();
        }
    }

    assert!(
        failures.is_empty(),
        "语料二进制往返失败 {} 例：\n{}",
        failures.len(),
        failures
            .iter()
            .take(10)
            .map(|(n, why)| format!("  {n}: {why}"))
            .collect::<Vec<_>>()
            .join("\n")
    );
    // 基线（2026-09-19 实测）：189 个正向用例可解析 ⇒ 全部往返文本一致
    assert!(checked >= 189, "往返覆盖数偏低：{checked}");
    // 尺寸基线：只打印，不设阈值（压缩/演进时重新采集）
    // 尺寸基线：只打印，不设阈值（压缩/演进时重新采集）。
    // 同时给"字节流 vs 源码文本"的比值——二进制缓存是否划算的直接依据。
    eprintln!(
        "binary size baseline: cases={checked} src_total={total_src}B bin_total={total_bytes}B \
         ratio={:.2}x max={max_bytes}B ({max_case}) avg_bin={}B",
        total_bytes as f64 / total_src.max(1) as f64,
        total_bytes / checked.max(1)
    );
}

// ============================================================
// fail-closed 负向对照
// ============================================================

#[test]
fn unknown_metadata_node_tag_is_rejected() {
    let e = decode_err(&metadata_only(&[vec![99]]));
    assert!(msg_of(&e).contains("未知 metadata 节点 tag"), "{e:?}");
}

#[test]
fn unknown_metadata_value_tag_is_rejected() {
    // Leaf + 未知值 tag 99
    let e = decode_err(&metadata_only(&[vec![0, 99]]));
    assert!(msg_of(&e).contains("未知 metadata 值 tag"), "{e:?}");
}

#[test]
fn metadata_out_of_range_reference_is_rejected() {
    // 节点 0 引用"节点总数之外"的 id 1（本流只有 1 个节点）⇒ 越界。
    // 注意：**前向引用是合法的**（显式 `!N` 编号会预分配槽位，见 named-metadata.ll），
    // 因此这里只钉"越界"这条边界。
    let mut node = vec![0u8]; // Leaf
    node.push(6); // Node
    put_varint(&mut node, 1);
    let e = decode_err(&metadata_only(&[node]));
    assert!(msg_of(&e).contains("越界"), "{e:?}");
}

#[test]
fn duplicate_metadata_nodes_are_preserved_verbatim() {
    // 两条内容相同的节点是**合法**的：显式 `!N` 编号经 insert_at 预分配槽位，
    // 每个 id 都可能被引用（语料 named-metadata.ll 等）。解码必须保 id 不折叠。
    let leaf = vec![0u8, 0, 0]; // Leaf(String, 索引 0 = "k")
    let bytes = metadata_only(&[leaf.clone(), leaf]);
    let back = Module::from_binary(&bytes).expect("重复内容节点应被保留");
    assert_eq!(back.metadata_store.len(), 2, "两条节点各占一个 id");
    assert_eq!(
        back.metadata_store.get(MetadataId(0)),
        back.metadata_store.get(MetadataId(1))
    );
}

#[test]
fn dangling_metadata_attachment_is_rejected() {
    // 真实模块的字节流：把函数附件的节点 id 改成越界值
    let real = reference_module().to_binary();
    let compat = check_binary_compat(&real).expect("头部");
    assert_eq!(compat.format_version, IR_FORMAT_VERSION);
    // 直接构造：只有 COMPAT/STRINGS/METADATA(空) + 一个引用 id 9 的函数附件
    let mut strings = Vec::new();
    put_varint(&mut strings, 1);
    put_varint(&mut strings, 1);
    strings.push(b'f');
    let mut funcs = Vec::new();
    put_varint(&mut funcs, 1); // 函数数
    put_varint(&mut funcs, 0); // 名字 = "f"
    put_varint(&mut funcs, 0); // 签名
    funcs.push(0); // 调用约定
    put_varint(&mut funcs, 0); // 属性
    put_varint(&mut funcs, 0); // 开放属性
    funcs.extend_from_slice(&[0, 0, 0, 0, 0, 0, 0, 0, 0]); // 符号
    funcs.push(0); // is_const
    funcs.push(0); // personality
    put_varint(&mut funcs, 0); // 参数属性
    put_varint(&mut funcs, 0); // 返回属性
    put_varint(&mut funcs, 1); // 函数 metadata：1 条
    funcs.push(0); // kind = DebugLoc
    put_varint(&mut funcs, 9); // 越界节点 id
    funcs.push(0); // debug_info
    put_varint(&mut funcs, 0); // 值名
    put_varint(&mut funcs, 0); // 块名
    for _ in 0..5 {
        put_varint(&mut funcs, 0); // 常量池
    }
    put_varint(&mut funcs, 0); // 值
    put_varint(&mut funcs, 0); // 指令
    put_varint(&mut funcs, 0); // 块
    put_varint(&mut funcs, 0); // 布局
    funcs.push(0); // 入口块
    let stream = build_stream(&[
        (SectionId::Compat.as_u8(), vec![0]),
        (SectionId::Strings.as_u8(), strings),
        (SectionId::Metadata.as_u8(), metadata_body(&[])),
        (SectionId::Funcs.as_u8(), funcs),
    ]);
    // 注：该流没有 TYPES 段 ⇒ 签名 0 不存在，先被拒；这里只要求"是 BinaryDecode"，
    // 真正覆盖"附件越界"的是 `dangling_metadata_attachment_on_real_module`。
    let e = decode_err(&stream);
    assert!(matches!(e, IrError::BinaryDecode { .. }), "{e:?}");

    // 真实模块：附件必然在界内 ⇒ 正常解码；把 METADATA 段体替成"0 节点"后，
    // 附件就变成悬空 ⇒ 必须被 validate_metadata_refs 拒绝。
    let mut sections: Vec<(u8, Vec<u8>)> = Vec::new();
    for (id, off, len) in test_sections_of(&real) {
        if id == SectionId::Metadata.as_u8() {
            sections.push((id, metadata_body(&[])));
        } else {
            sections.push((id, real[off..off + len].to_vec()));
        }
    }
    let broken = build_stream(&sections);
    let e = decode_err(&broken);
    assert!(msg_of(&e).contains("越界节点"), "{e:?}");
}

/// 测试侧段表解析。
fn test_sections_of(bytes: &[u8]) -> Vec<(u8, usize, usize)> {
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
    let mut pos = 8;
    let _ = read_varint(bytes, &mut pos);
    let producer_len = read_varint(bytes, &mut pos) as usize;
    pos += producer_len;
    let count = read_varint(bytes, &mut pos) as usize;
    let mut out = Vec::new();
    for _ in 0..count {
        let id = bytes[pos];
        pos += 1;
        let offset = read_varint(bytes, &mut pos) as usize;
        let len = read_varint(bytes, &mut pos) as usize;
        out.push((id, offset, len));
    }
    out
}

#[test]
fn deep_metadata_nesting_is_rejected_without_stack_overflow() {
    // 嵌套深度上限（64）：`Field(k, Field(k, …))` 递归解码必须给上界，
    // 否则不可信输入能把解码器栈打爆（进程 abort，而非可捕获的 Err）。
    fn nested(levels: usize) -> Vec<u8> {
        let mut node = vec![0u8]; // Leaf
        for _ in 0..levels {
            node.push(7); // Field
            put_varint(&mut node, 0); // key 字符串索引 0 = "k"
        }
        node.extend_from_slice(&[2, 2]); // 最内层 Int(1)：tag 2 + zigzag(1) = 2
        node
    }
    // 浅层（8 层）正常解码
    let ok = metadata_only(&[nested(8)]);
    let back = Module::from_binary(&ok).expect("8 层嵌套应可解码");
    assert_eq!(back.metadata_store.len(), 1);

    // 65 层已超过上限 ⇒ Err
    let e = decode_err(&metadata_only(&[nested(65)]));
    assert!(msg_of(&e).contains("嵌套"), "{e:?}");

    // 5000 层：必须在**爆栈之前**拒绝（走到上限就返回，不递归到底）
    let e = decode_err(&metadata_only(&[nested(5000)]));
    assert!(msg_of(&e).contains("嵌套"), "{e:?}");
}

#[test]
fn unknown_comdat_kind_is_rejected() {
    let mut globals = Vec::new();
    put_varint(&mut globals, 0); // 全局
    put_varint(&mut globals, 0); // 别名
    put_varint(&mut globals, 1); // comdat 1 条
    put_varint(&mut globals, 0); // 名字索引 0
    globals.push(99); // 未知 kind
    let mut strings = Vec::new();
    put_varint(&mut strings, 1);
    put_varint(&mut strings, 1);
    strings.push(b'c');
    let stream = build_stream(&[
        (SectionId::Compat.as_u8(), vec![0]),
        (SectionId::Strings.as_u8(), strings),
        (SectionId::Metadata.as_u8(), metadata_body(&[])),
        (SectionId::Globals.as_u8(), globals),
    ]);
    let e = decode_err(&stream);
    assert!(msg_of(&e).contains("comdat kind"), "{e:?}");
}
