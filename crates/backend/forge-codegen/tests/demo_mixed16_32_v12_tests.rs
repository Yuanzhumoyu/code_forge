//! 混合字长夹具（`tests/isa/demo_mixed16_32_v12.toml`）验证：v18 S4 宽度三态的
//! `kind = "mixed"` ——逐指令 `width`、按 `widths` 升序分组的解码、以及编解码往返。
//!
//! 位布局（见夹具头注释）：
//! - 16 位短编码：`[1:0]=00`、`[4:2]=rd`、`[7:5]=rs1`、`[15:13]=cop`
//! - 32 位长编码：`[1:0]=11`、`[4:2]=rd`、`[7:5]=rs1`、`[24:13]=imm12`、`[31:25]=lop`

mod common;

use common::demo_mixed16_32_v12::{Inst, assemble, decode, decode_partial, disassemble, encode};

fn enc(asm: &str) -> Vec<u8> {
    let inst = assemble(asm).unwrap_or_else(|e| panic!("assemble `{asm}`: {e}"));
    encode(&inst).unwrap_or_else(|e| panic!("encode `{asm}`: {e}"))
}

fn word_le(w: u32) -> Vec<u8> {
    w.to_le_bytes().to_vec()
}

/// 字长由**逐指令 `width`** 决定：短编码 2 字节、长编码 4 字节。
#[test]
fn mixed_widths_encode_to_their_own_length() {
    // cadd R1, R2 → cop=1<<13、sel=0、rd=1<<2、rs1=2<<5
    assert_eq!(enc("cadd R1, R2"), word_le(0x2044)[..2].to_vec());
    // cmov R3, R4 → cop=2<<13、rd=3<<2、rs1=4<<5
    assert_eq!(enc("cmov R3, R4"), vec![0x8C, 0x40]);
    // lnop → lop=1<<25、sel=3
    assert_eq!(enc("lnop"), word_le(0x0200_0003));
    // ladd R1, R2, 5 → lop=2<<25、imm12=5<<13、rd=1<<2、rs1=2<<5、sel=3
    assert_eq!(enc("ladd R1, R2, 5"), word_le(0x0400_A047));
    // 负立即数按槽宽符号扩展（signed = true）→ imm12 = 0xFFF
    assert_eq!(enc("ladd R0, R0, -1"), word_le(0x05FF_E003));
}

/// 解码按 `widths` 升序分组：短编码优先，返回**该指令**的消费字节数。
#[test]
fn mixed_decode_picks_the_matching_width_group() {
    let (i16, n16) = decode(&enc("cadd R1, R2")).expect("16 位应能解码");
    assert_eq!(n16, 2, "短编码消费 2 字节");
    assert!(matches!(i16, Inst::Cadd16 { .. }), "{i16:?}");

    let (i32, n32) = decode(&enc("ladd R1, R2, 5")).expect("32 位应能解码");
    assert_eq!(n32, 4, "长编码消费 4 字节");
    assert!(matches!(i32, Inst::Ladd32 { .. }), "{i32:?}");

    // 长编码的低 2 位是 11（sel=3）⇒ 低 16 位不匹配任何短编码：截成 2 字节也不误判
    let long = enc("ladd R1, R2, 5");
    assert!(decode(&long[..2]).is_none(), "长编码前缀不得被当成短编码");
}

/// 编解码往返 + 反汇编往返（两种字长都覆盖）。
#[test]
fn mixed_roundtrips() {
    for asm in [
        "cadd R1, R2",
        "cmov R3, R4",
        "lnop",
        "ladd R1, R2, 5",
        "ladd R7, R0, -1",
    ] {
        let bytes = enc(asm);
        let (inst, n) = decode(&bytes).unwrap_or_else(|| panic!("decode `{asm}`"));
        assert_eq!(n, bytes.len(), "`{asm}` 消费字节数 = 字长");
        assert_eq!(encode(&inst).unwrap(), bytes, "`{asm}` 再编码应逐字节相同");
        let text = disassemble(&inst);
        let again = assemble(&text).unwrap_or_else(|e| panic!("re-assemble `{text}`: {e}"));
        assert_eq!(encode(&again).unwrap(), bytes, "`{asm}` 反汇编→汇编往返");
    }
}

/// `decode_partial`：短于**最短**字长 ⇒ 截断（Err(len)）；否则 0（无匹配）。
#[test]
fn mixed_decode_partial_reports_truncation() {
    assert_eq!(decode_partial(&[0x44]), Err(1), "1 字节 < 最短字长 2");
    assert_eq!(decode_partial(&[]), Err(0));
    // 2 字节但不匹配任何短编码（低 2 位 = 11）⇒ 没有完整匹配，非截断
    assert_eq!(decode_partial(&[0x03, 0x00]), Err(0));
}

/// 能力集按三态派生：mixed ⇒ variable_length、min/max = 最窄/最宽字长。
#[test]
fn mixed_capabilities_report_width_range() {
    use forge_codegen::machine::target::TargetMachine as _;
    let tm = common::demo_mixed16_32_v12::TargetMachine::new();
    let caps = tm.isa_info().capabilities();
    assert!(caps.variable_length, "混合字长属于变长编码");
    assert_eq!(caps.fixed_inst_size, 0, "没有单一字长");
    assert_eq!(caps.min_inst_len, 2);
    assert_eq!(caps.max_inst_len, 4);
}
