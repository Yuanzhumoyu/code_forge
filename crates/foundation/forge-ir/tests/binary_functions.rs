//! 二进制序列化 B4：FUNCS 段（函数 + `dfg` + layout）的往返、确定性与负向对照。
//!
//! 覆盖的三类断言：
//!
//! 1. **往返**：含控制流（钻石 CFG + 块参数）、调用、immediate、metadata、`loc`、
//!    指令选择标签、值名/块名的函数，编解码后逐项相等，且解码结果过 `Verifier`；
//! 2. **确定性**：两次编码逐字节相同、`decode → encode` 幂等；use-lists 重算结果与
//!    原函数逐值相等（不落盘、靠重建）；
//! 3. **fail-closed**：未知 opcode 名 / 未知调用约定 / value kind 与密集索引不一致 /
//!    悬空操作数 / 未知 immediate tag / 越界值类型 —— 全部 `Err`（带偏移），不 panic。

use forge_ir::ir::metadata::{AttachedMetadata, MetadataKind, MetadataNode, MetadataValue};
use forge_ir::util::string_pool::InternedStr;
use forge_ir::{
    Block, FuncRef, FunctionAttributes, FunctionBuilder, FunctionSignature, IR_FORMAT_VERSION,
    ImmStr, Inst, IntCC, IrError, IselStrategy, Module, Opcode, SectionId, SourceLocation, TypeId,
    Value, check_binary_compat,
};

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

/// 段表条目 `(id, offset, len, raw_len)`（`raw_len > 0` = 段体是压缩体）。
fn sections_of(bytes: &[u8]) -> Vec<(u8, usize, usize, usize)> {
    let mut pos = 8;
    let _version = read_varint(bytes, &mut pos);
    let producer_len = read_varint(bytes, &mut pos) as usize;
    pos += producer_len;
    let count = read_varint(bytes, &mut pos) as usize;
    let mut out = Vec::new();
    for _ in 0..count {
        let id = bytes[pos];
        pos += 1;
        let offset = read_varint(bytes, &mut pos) as usize;
        let len = read_varint(bytes, &mut pos) as usize;
        let raw_len = read_varint(bytes, &mut pos) as usize;
        out.push((id, offset, len, raw_len));
    }
    out
}

/// 字符串池内容（用于在负向夹具里引用真实索引）。
///
/// 走**解码后的池**而不是 raw 段体：v2 起 STRINGS 段通常是压缩体，测试侧不该自带
/// 解压器。写侧先 `intern_pool` 整池登记，故池内容恰是 STRINGS 表的**前缀**且顺序
/// 一致 ⇒ 池内索引就是表索引（后续段追加的字符串排在池后）。
fn strings_of(bytes: &[u8]) -> Vec<String> {
    let m = Module::from_binary(bytes).expect("参考模块必须可解码");
    let store = m.types.borrow();
    (0..store.strings.len())
        .map(|i| store.strings.lookup(InternedStr(i as u32)).to_string())
        .collect()
}

/// 测试侧装配一个完整流（`raw_len = 0` 表示段体原样存放）。
fn build_stream(version: u64, producer: &str, sections: &[(u8, Vec<u8>, usize)]) -> Vec<u8> {
    fn vlen(mut v: u64) -> usize {
        let mut n = 1;
        while v >= 0x80 {
            v >>= 7;
            n += 1;
        }
        n
    }
    let mut header_len = 0usize;
    let mut offsets = vec![0u64; sections.len()];
    loop {
        let mut off = header_len as u64;
        let mut size = 8
            + vlen(version)
            + vlen(producer.len() as u64)
            + producer.len()
            + vlen(sections.len() as u64);
        for (i, (_, body, raw_len)) in sections.iter().enumerate() {
            offsets[i] = off;
            // 条目 = id(1) + offset + len + raw_len
            size += 1 + vlen(off) + vlen(body.len() as u64) + vlen(*raw_len as u64);
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
    for (i, (id, body, raw_len)) in sections.iter().enumerate() {
        out.push(*id);
        put_varint(&mut out, offsets[i]);
        put_varint(&mut out, body.len() as u64);
        put_varint(&mut out, *raw_len as u64);
    }
    assert_eq!(out.len(), header_len);
    for (_, body, _) in sections {
        out.extend_from_slice(body);
    }
    out
}

/// 用给定的 FUNCS 段体替换一份真实编码里的 FUNCS 段（其余段**连同 `raw_len`
/// 原样搬运**，压缩段照旧是压缩段）。`funcs_raw_len = 0` = 新段体未压缩。
fn with_funcs_body_raw(real: &[u8], funcs_body: Vec<u8>, funcs_raw_len: usize) -> Vec<u8> {
    let producer = check_binary_compat(real)
        .expect("头部")
        .producer
        .to_string();
    let mut sections: Vec<(u8, Vec<u8>, usize)> = Vec::new();
    for (id, off, len, raw_len) in sections_of(real) {
        if id == SectionId::Funcs.as_u8() {
            continue;
        }
        sections.push((id, real[off..off + len].to_vec(), raw_len));
    }
    sections.push((SectionId::Funcs.as_u8(), funcs_body, funcs_raw_len));
    build_stream(u64::from(IR_FORMAT_VERSION), &producer, &sections)
}

/// 同上，但新 FUNCS 段体按**未压缩**存放（负向夹具一律走这条）。
fn with_funcs_body(real: &[u8], funcs_body: Vec<u8>) -> Vec<u8> {
    with_funcs_body_raw(real, funcs_body, 0)
}

/// 最小函数记录夹具：固定头部 + 调用方给的 `values`/`insts`/`blocks`/`layout`。
///
/// 头部字段全取默认值（sig = 0、cc = 0、无属性/符号/名字表、常量池五通道计数 0），
/// 因此夹具只测"目标字段"的负向路径，不掺入其它字段的合法性。
fn mini_func(name_idx: u64, values: &[u8], insts: &[u8], blocks: &[u8], layout: &[u8]) -> Vec<u8> {
    mini_func_with_cc(name_idx, 0, values, insts, blocks, layout)
}

/// 同上，但可指定调用约定字节（未知 cc 的负向用例用）。
fn mini_func_with_cc(
    name_idx: u64,
    cc: u8,
    values: &[u8],
    insts: &[u8],
    blocks: &[u8],
    layout: &[u8],
) -> Vec<u8> {
    let mut out = Vec::new();
    put_varint(&mut out, 1); // 函数个数
    put_varint(&mut out, name_idx);
    put_varint(&mut out, 0); // signature
    out.push(cc);
    out.extend_from_slice(&mini_func_tail(values, insts, blocks, layout));
    out
}

/// `mini_func` 的公共尾部（属性 → layout → entry）。
fn mini_func_tail(values: &[u8], insts: &[u8], blocks: &[u8], layout: &[u8]) -> Vec<u8> {
    let mut out = Vec::new();
    put_varint(&mut out, 0); // attributes
    put_varint(&mut out, 0); // extra_attrs
    // symbol：linkage/visibility/dll 各 1 字节 + section/comdat/tls 各 1 字节 none + 3 bool
    out.extend_from_slice(&[0, 0, 0, 0, 0, 0, 0, 0, 0]);
    out.push(0); // is_const
    out.push(0); // personality none
    put_varint(&mut out, 0); // param_attrs
    put_varint(&mut out, 0); // ret_attrs
    put_varint(&mut out, 0); // metadata
    out.push(0); // debug_info none
    put_varint(&mut out, 0); // value_names
    put_varint(&mut out, 0); // block_names
    // 每函数常量池：五通道计数全 0
    for _ in 0..5 {
        put_varint(&mut out, 0);
    }
    out.extend_from_slice(values);
    out.extend_from_slice(insts);
    out.extend_from_slice(blocks);
    out.extend_from_slice(layout);
    out.push(0); // entry_block none
    out
}

/// **指令段**夹具：`1` 条指令（含段内条数）。所有字段取默认值，opcode 名索引由
/// 调用方给（未知 opcode / 未知 immediate 等负向用例用）。
fn mini_inst(
    opcode_idx: u64,
    block: u64,
    results: &[u64],
    operands: &[u64],
    immediates: &[u8],
) -> Vec<u8> {
    let mut out = Vec::new();
    put_varint(&mut out, 1); // 指令条数（段内计数）
    put_varint(&mut out, opcode_idx);
    put_varint(&mut out, block);
    put_varint(&mut out, results.len() as u64);
    for r in results {
        put_varint(&mut out, *r);
    }
    put_varint(&mut out, operands.len() as u64);
    for o in operands {
        put_varint(&mut out, *o);
    }
    put_varint(&mut out, 1); // immediates 条数（由 `immediates` 给字节）
    out.extend_from_slice(immediates);
    put_varint(&mut out, 0); // flags
    put_varint(&mut out, 0); // mem_flags
    put_varint(&mut out, 0); // param_attrs
    put_varint(&mut out, 0); // fn_attrs
    put_varint(&mut out, 0); // metadata
    out.push(0); // loc none
    out.push(0); // isel none
    out.push(0); // tombstone
    out
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
// 参考模块：含控制流/调用/immediate/元数据/位置/标签/名字
// ============================================================

fn reference_module() -> Module {
    let mut m = Module::new();
    let types = m.types.clone();
    let sig = FunctionSignature::new(&[(TypeId::I32, "n")], &[TypeId::I32]);

    // callee：fn callee(x: i32) -> i32 { ret x }
    let callee_sig = FunctionSignature::new(&[(TypeId::I32, "x")], &[TypeId::I32]);
    let mut cb = FunctionBuilder::new("callee", types.clone(), callee_sig);
    let (c_entry, c_params) = cb.create_block_with_params(&[(TypeId::I32, "x")]);
    cb.switch_to_block(c_entry);
    cb.ret(&[c_params[0]]);
    let callee = cb.finish().expect("callee");
    m.add_function(callee);

    // 主函数：钻石 CFG（块参数）+ 调用 + icmp + 常量 + 名字
    let mut b = FunctionBuilder::new("main", types.clone(), sig);
    let (entry, params) = b.create_block_with_params(&[(TypeId::I32, "n")]);
    let then_blk = b.create_block();
    let else_blk = b.create_block();
    let (merge, merge_params) = b.create_block_with_params(&[(TypeId::I32, "r")]);
    b.switch_to_block(entry);
    let zero = b.iconst_i32(0);
    let cond = b.icmp(IntCC::SignedGreaterThan, params[0], zero);
    b.branch(cond, then_blk, &[], else_blk, &[]);
    b.switch_to_block(then_blk);
    let called = b.call(FuncRef::new(0), &[params[0]], &[TypeId::I32]);
    b.jump(merge, &[called[0]]);
    b.switch_to_block(else_blk);
    b.jump(merge, &[zero]);
    b.switch_to_block(merge);
    b.ret(&[merge_params[0]]);
    let mut func = b.finish().expect("main");

    // 附件：metadata（真实节点）、loc、指令选择标签、值名
    let node = m
        .metadata_store
        .intern(MetadataNode::Leaf(MetadataValue::String(ImmStr::from(
            "noscope",
        ))));
    let target = func
        .dfg
        .block(entry)
        .inst_order
        .first()
        .copied()
        .expect("entry 有指令");
    {
        let inst = func.dfg.inst_mut(target);
        inst.attach_metadata(AttachedMetadata {
            kind: MetadataKind::NoAlias,
            node,
        });
        inst.loc = Some(SourceLocation::new("main.c", 12, 7));
        inst.set_isel_strategy(IselStrategy::new("lea_sib:4"));
    }
    func.value_names
        .insert(zero, func.types.borrow_mut().strings.intern("zero"));
    func.block_names
        .insert(merge, func.types.borrow_mut().strings.intern("merge"));
    func.attributes = FunctionAttributes::NONE;
    func.extra_attrs.push(ImmStr::from("nounwind"));
    func.symbol.dso_local = true;
    func.is_const = false;
    m.add_function(func);
    m
}

// ============================================================
// 往返 / 确定性
// ============================================================

#[test]
fn function_roundtrips_with_control_flow() {
    let m = reference_module();
    let bytes = m.to_binary();
    let back = Module::from_binary(&bytes).expect("解码函数模块");

    assert_eq!(back.function_count(), m.function_count());
    for (fr, orig) in m.iter_functions().enumerate() {
        let got = back.get_function(FuncRef::new(fr as u32));
        assert_eq!(got.name.as_str(), orig.name.as_str(), "函数名");
        assert_eq!(got.signature, orig.signature);
        assert_eq!(got.calling_convention, orig.calling_convention);
        assert_eq!(got.attributes, orig.attributes);
        assert_eq!(got.extra_attrs.len(), orig.extra_attrs.len());
        assert_eq!(got.is_const, orig.is_const);
        assert_eq!(got.symbol.dso_local, orig.symbol.dso_local);
        assert_eq!(got.entry_block, orig.entry_block);
        assert_eq!(got.layout.block_order, orig.layout.block_order, "块布局");
        assert_eq!(
            got.dfg.value_count(),
            orig.dfg.value_count(),
            "值数量（dense 索引必须逐位一致）"
        );
        assert_eq!(got.dfg.inst_count(), orig.dfg.inst_count(), "指令数量");
        assert_eq!(got.dfg.block_count(), orig.dfg.block_count(), "块数量");

        for i in 0..orig.dfg.value_count() {
            let v = Value::new(i as u32);
            assert_eq!(got.dfg.value_data(v).ty, orig.dfg.value_data(v).ty);
            assert_eq!(got.dfg.value_def(v), orig.dfg.value_def(v), "值 {i} 的定义");
        }
        for i in 0..orig.dfg.inst_count() {
            let inst = Inst::new(i as u32);
            assert_eq!(
                got.dfg.inst_opcode(inst),
                orig.dfg.inst_opcode(inst),
                "指令 {i} opcode"
            );
            assert_eq!(got.dfg.inst_operands(inst), orig.dfg.inst_operands(inst));
            assert_eq!(got.dfg.inst_results(inst), orig.dfg.inst_results(inst));
            assert_eq!(
                got.dfg.inst_data(inst).immediates,
                orig.dfg.inst_data(inst).immediates
            );
            assert_eq!(
                got.dfg.inst_data(inst).flags,
                orig.dfg.inst_data(inst).flags
            );
            assert_eq!(
                got.dfg.inst_data(inst).mem_flags,
                orig.dfg.inst_data(inst).mem_flags
            );
            assert_eq!(
                got.dfg.inst_data(inst).metadata(),
                orig.dfg.inst_data(inst).metadata()
            );
            assert_eq!(got.dfg.inst_data(inst).loc, orig.dfg.inst_data(inst).loc);
            assert_eq!(
                got.dfg.inst_data(inst).isel_strategy(),
                orig.dfg.inst_data(inst).isel_strategy()
            );
            assert_eq!(
                got.dfg.inst_data(inst).is_tombstone(),
                orig.dfg.inst_data(inst).is_tombstone()
            );
        }
        for b in 0..orig.dfg.block_count() {
            let blk = Block::new(b as u32);
            assert_eq!(got.dfg.block(blk).params, orig.dfg.block(blk).params);
            assert_eq!(
                got.dfg.block(blk).param_values,
                orig.dfg.block(blk).param_values
            );
            assert_eq!(
                got.dfg.block(blk).inst_order,
                orig.dfg.block(blk).inst_order
            );
            assert_eq!(
                got.dfg.block(blk).terminator_opt(),
                orig.dfg.block(blk).terminator_opt()
            );
        }
        // use-lists 不落盘、靠重建：逐值使用计数必须相等
        for i in 0..orig.dfg.value_count() {
            let v = Value::new(i as u32);
            assert_eq!(
                got.use_lists.use_count(v),
                orig.use_lists.use_count(v),
                "值 {i} 的使用计数"
            );
        }
        // 名字表与函数级 metadata
        assert_eq!(
            got.value_names.iter().count(),
            orig.value_names.iter().count()
        );
        assert_eq!(
            got.block_names.iter().count(),
            orig.block_names.iter().count()
        );
        assert_eq!(got.metadata().len(), orig.metadata().len());
    }
}

#[test]
fn decoded_function_passes_verifier() {
    let m = reference_module();
    let back = Module::from_binary(&m.to_binary()).expect("解码");
    for f in back.iter_functions() {
        let mut v = forge_ir::verify::Verifier::with_ctx(f.types.clone());
        v.verify(f)
            .unwrap_or_else(|errs| panic!("解码后的函数未过校验：{errs:?}"));
    }
}

#[test]
fn function_encoding_is_idempotent() {
    let first = reference_module().to_binary();
    let back = Module::from_binary(&first).expect("解码");
    assert_eq!(back.to_binary(), first, "FUNCS 段同样必须编码幂等");
}

// ============================================================
// fail-closed 负向对照
// ============================================================

#[test]
fn unknown_opcode_name_is_rejected() {
    let real = reference_module().to_binary();
    let strings = strings_of(&real);
    // 表里某个**不是 opcode 名**的字符串（函数名/参数名等）⇒ from_name 必为 None
    let bogus_idx = strings
        .iter()
        .position(|s| Opcode::from_name(s).is_none())
        .expect("表里有非 opcode 串") as u64;
    let mut values = Vec::new();
    put_varint(&mut values, 1); // 值个数
    values.extend_from_slice(&[1]); // Param(block 0, k 0)
    put_varint(&mut values, 0); // block
    put_varint(&mut values, 0); // k
    put_varint(&mut values, TypeId::I32.index() as u64);
    let inst = mini_inst(bogus_idx, 0, &[], &[], &[0, 0]);
    let bytes = with_funcs_body(
        &real,
        mini_func(
            0,
            &values,
            &inst,
            &block_body(1, &[0], None),
            &layout_body(&[0]),
        ),
    );
    let e = decode_err(&bytes);
    assert!(msg_of(&e).contains("未知 opcode"), "{e:?}");
}

#[test]
fn unknown_call_conv_is_rejected() {
    let real = reference_module().to_binary();
    let bytes = with_funcs_body(
        &real,
        mini_func_with_cc(
            0,
            99, // 未知调用约定
            &zero_values(),
            &no_insts(),
            &no_blocks(),
            &no_layout(),
        ),
    );
    let e = decode_err(&bytes);
    assert!(msg_of(&e).contains("调用约定"), "{e:?}");
}

#[test]
fn value_kind_mismatch_is_rejected() {
    let real = reference_module().to_binary();
    // 值 0 声称是"指令 0 的第 0 个结果"，但该指令没有结果
    let mut values = Vec::new();
    put_varint(&mut values, 1);
    values.extend_from_slice(&[0]); // Inst
    put_varint(&mut values, 0); // inst 0
    values.push(0); // result idx 0
    put_varint(&mut values, TypeId::I32.index() as u64);
    let op_at = strings_of(&real)
        .iter()
        .position(|s| Opcode::from_name(s).is_some())
        .expect("表里有 opcode 名") as u64;
    let inst = mini_inst(op_at, 0, &[], &[], &[0, 0]);
    let bytes = with_funcs_body(
        &real,
        mini_func(
            0,
            &values,
            &inst,
            &block_body(1, &[0], None),
            &layout_body(&[0]),
        ),
    );
    let e = decode_err(&bytes);
    let msg = msg_of(&e);
    assert!(msg.contains("声称"), "{e:?}");
}

#[test]
fn dangling_operand_is_rejected() {
    let real = reference_module().to_binary();
    let op_at = strings_of(&real)
        .iter()
        .position(|s| Opcode::from_name(s).is_some())
        .expect("表里有 opcode 名") as u64;
    let inst = mini_inst(op_at, 0, &[], &[7], &[0, 0]); // 操作数 7 不存在
    let bytes = with_funcs_body(
        &real,
        mini_func(
            0,
            &zero_values(),
            &inst,
            &block_body(1, &[0], None),
            &layout_body(&[0]),
        ),
    );
    let e = decode_err(&bytes);
    assert!(msg_of(&e).contains("操作数"), "{e:?}");
}

#[test]
fn unknown_immediate_tag_is_rejected() {
    let real = reference_module().to_binary();
    let op_at = strings_of(&real)
        .iter()
        .position(|s| Opcode::from_name(s).is_some())
        .expect("表里有 opcode 名") as u64;
    let inst = mini_inst(op_at, 0, &[], &[], &[99]); // 未知 immediate tag
    let bytes = with_funcs_body(
        &real,
        mini_func(
            0,
            &zero_values(),
            &inst,
            &block_body(1, &[0], None),
            &layout_body(&[0]),
        ),
    );
    let e = decode_err(&bytes);
    assert!(msg_of(&e).contains("未知 immediate"), "{e:?}");
}

#[test]
fn value_type_out_of_range_is_rejected() {
    let real = reference_module().to_binary();
    let mut values = Vec::new();
    put_varint(&mut values, 1);
    values.extend_from_slice(&[1]); // Param(block 0, k 0)
    put_varint(&mut values, 0); // block
    put_varint(&mut values, 0); // k
    put_varint(&mut values, 9999); // 越界类型
    let bytes = with_funcs_body(
        &real,
        mini_func(
            0,
            &values,
            &no_insts(),
            &block_body(1, &[0], None),
            &layout_body(&[0]),
        ),
    );
    let e = decode_err(&bytes);
    assert!(msg_of(&e).contains("类型"), "{e:?}");
}

#[test]
fn truncated_funcs_body_is_rejected() {
    let real = reference_module().to_binary();
    let (_, off, len, raw_len) = sections_of(&real)
        .into_iter()
        .find(|(id, _, _, _)| *id == SectionId::Funcs.as_u8())
        .expect("FUNCS 段");
    let body = real[off..off + len].to_vec();
    for cut in 1..body.len() {
        // 保留原始 `raw_len`：FUNCS 段是压缩体时，走的是"解压失败即 Err"这条路径
        // （而不是把压缩字节当段体解析）。
        let bytes = with_funcs_body_raw(&real, body[..cut].to_vec(), raw_len);
        let e = decode_err(&bytes);
        assert!(
            matches!(e, IrError::BinaryDecode { .. }),
            "FUNCS 截断到 {cut} 字节应报 BinaryDecode，实际 {e:?}"
        );
    }
}

#[test]
fn empty_module_still_writes_funcs_section() {
    let bytes = Module::new().to_binary();
    let compat = check_binary_compat(&bytes).expect("头部");
    assert!(compat.sections.contains(&SectionId::Funcs));
    let back = Module::from_binary(&bytes).expect("解码空模块");
    assert_eq!(back.function_count(), 0);
}

// ============================================================
// 夹具片段
// ============================================================

fn zero_values() -> Vec<u8> {
    let mut out = Vec::new();
    put_varint(&mut out, 0);
    out
}

fn no_insts() -> Vec<u8> {
    let mut out = Vec::new();
    put_varint(&mut out, 0);
    out
}

fn no_blocks() -> Vec<u8> {
    let mut out = Vec::new();
    put_varint(&mut out, 0);
    out
}

fn no_layout() -> Vec<u8> {
    let mut out = Vec::new();
    put_varint(&mut out, 0);
    out
}

/// 一个块：参数类型数 + 类型 + 参数值个数 + 值 + 顺序表 + 终结符。
fn block_body(params: u64, param_values: &[u64], terminator: Option<u64>) -> Vec<u8> {
    let mut out = Vec::new();
    put_varint(&mut out, 1); // 块数
    put_varint(&mut out, params);
    for _ in 0..params {
        put_varint(&mut out, TypeId::I32.index() as u64);
    }
    put_varint(&mut out, param_values.len() as u64);
    for v in param_values {
        put_varint(&mut out, *v);
    }
    put_varint(&mut out, 0); // inst_order
    match terminator {
        Some(t) => {
            out.push(1);
            put_varint(&mut out, t);
        }
        None => out.push(0),
    }
    out
}

fn layout_body(blocks: &[u64]) -> Vec<u8> {
    let mut out = Vec::new();
    put_varint(&mut out, blocks.len() as u64);
    for b in blocks {
        put_varint(&mut out, *b);
    }
    out
}
