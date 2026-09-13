//! demo_inst12_v12 — **12 位指令字 ISA** 回归测试（字宽无白名单）。
//!
//! 夹具（`tests/isa/demo_inst12_v12.toml`）声明 `default_inst_width = 12`：
//! 既不是 8 的倍数、也不是 16/32/64 中的任何一个。本文件断言：
//!
//! 1. 12 位字被**接受**（不存在 {8,16,32,64} 白名单），存储为 **2 字节**
//!    （`ceil(12/8)`），encode/decode 与它一致（`consumed = 2`）。
//! 2. 字外填充位（bits 12..16，即第二个字节的高 4 位）**必须为 0**：置 1 的字
//!    不匹配任何指令 → `None`（fail-closed，不静默按低位解码）。
//! 3. 汇编/反汇编往返在同一字宽上成立。

mod common;

use common::demo_inst12_v12::{Inst, Reg, assemble, decode, disassemble, encode};

/// 12 位字（bits 0..12）：`op(3) | rd(3) | rs1(3) | rs2(3)`。
fn word12(op: u16, rd: u16, rs1: u16, rs2: u16) -> u16 {
    (op << 9) | (rd << 6) | (rs1 << 3) | rs2
}

/// 12 位字的 2 字节小端表示（高 4 位填充 0）。
fn bytes12(w: u16) -> Vec<u8> {
    w.to_le_bytes()[..2].to_vec()
}

fn enc(asm: &str) -> Vec<u8> {
    let inst = assemble(asm).unwrap_or_else(|e| panic!("assemble `{asm}`: {e}"));
    encode(&inst).unwrap_or_else(|e| panic!("encode `{asm}`: {e}"))
}

/// 12 位字 = 2 字节；decode 消费 2 字节（不是 1、也不是 4）。
#[test]
fn twelve_bit_word_is_two_bytes() {
    assert_eq!(enc("nop"), vec![0x00, 0x00], "12 位全 0 字 = 2 字节");
    // add a1, a2, a3 → op=1 | rd=1 | rs1=2 | rs2=3
    let expect = word12(1, 1, 2, 3);
    assert_eq!(enc("add a1, a2, a3"), bytes12(expect));
    assert_eq!(
        expect, 0x253,
        "0b001_001_010_011 = 0x253（bits 9..12 = op）"
    );

    let (inst, n) = decode(&bytes12(expect)).expect("decode 12 位字");
    assert_eq!(n, 2, "消费字节数 = ceil(12/8) = 2");
    assert_eq!(
        inst,
        Inst::Add12 {
            rd: Reg::A1,
            rs1: Reg::A2,
            rs2: Reg::A3,
        }
    );
}

/// 字外填充位（第二个字节高 4 位）非 0 → 不匹配任何指令（不静默按低位解码）。
#[test]
fn padding_bits_must_be_zero() {
    let valid = bytes12(word12(1, 1, 2, 3));
    assert!(decode(&valid).is_some(), "合法字可解码");
    // 仅把填充位置 1：bits 12..16 = 0b0001
    let dirty = vec![valid[0], valid[1] | 0x10];
    assert!(
        decode(&dirty).is_none(),
        "填充位被置 1 的字必须被拒绝（补集零 guard），不能只按低 12 位解码"
    );
    // nop（全字常量 0）同理
    assert!(decode(&[0x00, 0x00]).is_some());
    assert!(decode(&[0x00, 0x01]).is_none(), "0x0100 ≠ 0（12 位）");
}

/// 汇编 → 编码 → 解码 → 反汇编往返（12 位字）。
#[test]
fn roundtrip_and_text() {
    for (asm, text) in [
        ("nop", "nop"),
        ("add a3, a1, a2", "add A3, A1, A2"),
        ("mov a0, a7", "mov A0, A7"),
    ] {
        let bytes = enc(asm);
        assert_eq!(bytes.len(), 2, "`{asm}` 是 12 位字（2 字节）");
        let (inst, n) = decode(&bytes).unwrap_or_else(|| panic!("decode `{asm}`"));
        assert_eq!(n, 2);
        assert_eq!(encode(&inst).expect("re-encode"), bytes, "`{asm}` 往返字节");
        assert_eq!(disassemble(&inst), text);
    }
}
