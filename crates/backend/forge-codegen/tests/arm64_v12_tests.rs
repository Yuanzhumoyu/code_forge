//! arm64_v12 — A64 P1-A1 golden 测试。
//!
//! 所有 REF_* 常量来自本机 LLVM oracle：
//! `clang --target=aarch64-none-elf -c` + `llvm-objdump -d` 逐字实测
//! （该工具链严格实现 ARM 官方 A64 位段）。asm→字节 golden 对照 +
//! decode→encode 字节往返 + disassemble→assemble 往返。

use forge_codegen::arm64_v12::{Reg, assemble, decode, disassemble, encode};

#[test]
fn debug_reg_index() {
    use forge_ir::PhysReg;
    let w5 = Reg::W5.to_index();
    let x5 = Reg::X5.to_index();
    let w6 = Reg::W6.to_index();
    assert_eq!((w5, x5, w6), (5, 5, 6), "W/X 应共享物理编号");
}

fn enc(asm: &str) -> Vec<u8> {
    let inst = assemble(asm).unwrap_or_else(|e| panic!("assemble `{asm}`: {e}"));
    encode(&inst).unwrap_or_else(|e| panic!("encode `{asm}`: {e}"))
}

fn word_le(w: u32) -> Vec<u8> {
    w.to_le_bytes().to_vec()
}

// ─────────────────── asm → 字节 golden（clang oracle 常量） ───────────────────

#[test]
fn golden_encode_oracle_verified() {
    // 立即数 ALU：add/adds/sub/subs × X/W
    assert_eq!(enc("add x5, x6, #42"), word_le(0x9100A8C5));
    let i = assemble("add w5, w6, #42").unwrap();
    assert_eq!(encode(&i).unwrap(), word_le(0x1100A8C5), "Inst={i:?}");
    assert_eq!(enc("adds w5, w6, #42"), word_le(0x3100A8C5));
    assert_eq!(enc("adds x5, x6, #42"), word_le(0xB100A8C5));
    assert_eq!(enc("sub x5, x6, #42"), word_le(0xD100A8C5));
    assert_eq!(enc("subs x5, x6, #42"), word_le(0xF100A8C5));
    assert_eq!(enc("add x0, x1, #0"), word_le(0x91000020));
    // 寄存器 ALU（shifted-reg LSL#0 形式）
    assert_eq!(enc("add x5, x6, x2"), word_le(0x8B0200C5));
    assert_eq!(enc("add w5, w6, w2"), word_le(0x0B0200C5));
    assert_eq!(enc("sub x5, x6, x7"), word_le(0xCB0700C5));
    // 分支（偏移 0 = 自身）
    assert_eq!(enc("b 0"), word_le(0x14000000));
    assert_eq!(enc("bl 0"), word_le(0x94000000));
    // NOP
    assert_eq!(enc("nop"), word_le(0xD503201F));
}

#[test]
fn golden_register_encoding_pattern() {
    // rd/rn/imm 位段位置：add x5,x6,#0x2a → imm12=0x2A<<10、Rn=6<<5、Rd=5
    let w = 0x91000000u32 | (42 << 10) | (6 << 5) | 5;
    assert_eq!(enc("add x5, x6, #42"), word_le(w));
    // 寄存器形式：rm=2<<16
    let w2 = 0x8B000000u32 | (2 << 16) | (6 << 5) | 5;
    assert_eq!(enc("add x5, x6, x2"), word_le(w2));
}

// ─────────────────── decode → encode 字节往返 ───────────────────

#[test]
fn decode_roundtrip_bytes() {
    for asm in [
        "add x5, x6, #42",
        "add w0, w30, #4095",
        "adds x1, x2, #3",
        "sub w4, w5, w6",
        "subs x7, x8, #0",
        "add x9, x10, x11",
        "b 4",
        "bl -8",
        "nop",
    ] {
        let b = enc(asm);
        let (d, n) = decode(&b).unwrap_or_else(|| panic!("decode {b:02x?} (`{asm}`)"));
        assert_eq!(n, 4, "`{asm}`: 消费字节 != 4");
        let b2 = encode(&d).unwrap();
        assert_eq!(b2, b, "`{asm}`: decode→encode 字节不一致");
    }
}

// ─────────────────── disassemble → assemble 往返 ───────────────────

#[test]
fn assemble_disassemble_roundtrip() {
    for asm in ["add x5, x6, #42", "add w1, w2, w3", "sub x0, x0, x0", "nop"] {
        let inst = assemble(asm).unwrap();
        let text = disassemble(&inst);
        let inst2 = assemble(&text).unwrap_or_else(|e| panic!("reassemble `{text}`: {e}"));
        assert_eq!(inst2, inst, "`{asm}` → `{text}` 往返失败");
    }
}

// ─────────────────── 非法输入拒绝 ───────────────────

#[test]
fn invalid_asm_rejected() {
    assert!(assemble("frob x0, x1").is_err());
    assert!(assemble("add x0, x1").is_err());
    assert!(assemble("add x0, x1, #4096").is_err()); // imm12 上界 4095
    assert!(assemble("add x0, x1, x2, lsl #1").is_err()); // A1 未支持移位后缀
    assert!(assemble("").is_err());
    // 寄存器宽度混用拒绝
    assert!(assemble("add x0, x1, w2").is_err());
    assert!(assemble("add w0, x1, w2").is_err());
}
