//! demo_inst100_v12 — **100 位指令字 ISA** 回归测试（字宽无上限）。
//!
//! 夹具（`tests/isa/demo_inst100_v12.toml`）声明 `default_inst_width = 100`：
//! 超出任何机器字（u64/u128）、且不是 8 的倍数。本文件断言：
//!
//! 1. 100 位字被**接受**（无 {32,64,128} 之类白名单），存储为 **13 字节**
//!    （`ceil(100/8)`）；encode/decode 与它一致（`consumed = 13`）。
//! 2. 机器字之外的位域（`op` 在 bit 92..100 → 第 11/12 字节）正确读写——
//!    生成代码用**字节数组字**而不是 u64 累加器。
//! 3. 字外填充位（bit 100..104，即第 13 字节高 4 位）**必须为 0**：置 1 的字
//!    不匹配任何指令 → `None`（fail-closed）。

mod common;

use common::demo_inst100_v12::{Inst, Reg, assemble, decode, encode};

/// 100 位字的位偏移（与 TOML 的 bitfields 一致）。
const OP: u32 = 92;
const RD: u32 = 76;
const RS1: u32 = 64;
const RS2: u32 = 4;

/// 按位宽写一个域到 13 字节的字节数组（LE 位序：bit 0 = 第 0 字节 LSB）。
fn put(word: &mut [u8; 13], value: u64, off: u32, width: u32) {
    for i in 0..width {
        if (value >> i) & 1 == 0 {
            continue;
        }
        let bit = off + i;
        word[(bit / 8) as usize] |= 1u8 << (bit % 8);
    }
}

fn word(op: u64, rd: u64, rs1: u64, rs2: u64) -> Vec<u8> {
    let mut w = [0u8; 13];
    put(&mut w, op, OP, 8);
    put(&mut w, rd, RD, 4);
    put(&mut w, rs1, RS1, 4);
    put(&mut w, rs2, RS2, 4);
    w.to_vec()
}

fn enc(asm: &str) -> Vec<u8> {
    let inst = assemble(asm).unwrap_or_else(|e| panic!("assemble `{asm}`: {e}"));
    encode(&inst).unwrap_or_else(|e| panic!("encode `{asm}`: {e}"))
}

/// 100 位字 = 13 字节；位域落在机器字之外（bit 92..100）仍正确。
#[test]
fn hundred_bit_word_is_thirteen_bytes() {
    let expect = word(1, 1, 2, 3);
    assert_eq!(expect.len(), 13, "ceil(100/8) = 13 字节");
    // 逐字节定位：op 在 bit 92 → 第 11 字节 bit 4；rd 在 bit 76 → 第 9 字节 bit 4
    assert_eq!(expect[0], 0x30, "rs2 = 3 落在 bit 4..8");
    assert_eq!(expect[8], 0x02, "rs1 = 2 落在 bit 64..68（第 8 字节）");
    assert_eq!(expect[9], 0x10, "rd = 1 落在 bit 76..80（第 9 字节）");
    assert_eq!(expect[11], 0x10, "op = 1 落在 bit 92..100（第 11 字节）");
    assert_eq!(expect[12], 0x00, "bit 96..100 是 op 高位 = 0");

    assert_eq!(enc("add a1, a2, a3"), expect);
    let (inst, n) = decode(&expect).expect("decode 100 位字");
    assert_eq!(n, 13, "消费字节数 = 13");
    assert_eq!(
        inst,
        Inst::Add100 {
            rd: Reg::A1,
            rs1: Reg::A2,
            rs2: Reg::A3,
        }
    );
    // 高位寄存器（4 位域）也要能往返
    let wide = enc("mov a15, a9");
    assert_eq!(wide, word(2, 15, 9, 0));
    assert_eq!(
        decode(&wide).map(|(i, _)| i),
        Some(Inst::Mov100 {
            rd: Reg::A15,
            rs1: Reg::A9,
        })
    );
}

/// 全字 0 = nop（op = 0 且"补集零 guard"要求其余位为 0）。
#[test]
fn zero_word_is_nop() {
    let nop = enc("nop");
    assert_eq!(nop, vec![0u8; 13]);
    assert_eq!(decode(&nop), Some((Inst::Nop100, 13)));
}

/// 字外填充位（bit 100..104）非 0 → 无匹配（不静默按低 100 位解码）。
#[test]
fn padding_bits_must_be_zero() {
    let mut dirty = enc("nop");
    dirty[12] |= 0x10; // bit 100（第 13 字节 bit 4）
    assert!(
        decode(&dirty).is_none(),
        "填充位被置 1 的字必须被拒绝（补集零 guard）"
    );
    // 同一条字：把填充位清掉又可解码
    dirty[12] &= !0x10;
    assert!(decode(&dirty).is_some());
    // 不足 13 字节 → None
    assert!(decode(&dirty[..12]).is_none(), "不足一个字 → None");
}
