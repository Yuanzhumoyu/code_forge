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
fn golden_a2_logic_cmp_movw() {
    // 逻辑寄存器（clang oracle）
    assert_eq!(enc("and x0, x1, x2"), word_le(0x8A020020));
    assert_eq!(enc("and w3, w4, w5"), word_le(0x0A050083));
    assert_eq!(enc("bic x6, x7, x8"), word_le(0x8A2800E6));
    assert_eq!(enc("bic w9, w10, w11"), word_le(0x0A2B0149));
    assert_eq!(enc("orr x0, x1, x2"), word_le(0xAA020020));
    assert_eq!(enc("orr w3, w4, w5"), word_le(0x2A050083));
    assert_eq!(enc("orn x6, x7, x8"), word_le(0xAA2800E6));
    assert_eq!(enc("eor x0, x1, x2"), word_le(0xCA020020));
    assert_eq!(enc("eor w3, w4, w5"), word_le(0x4A050083));
    assert_eq!(enc("eon x6, x7, x8"), word_le(0xCA2800E6));
    // MOV 别名（ORR rd, xzr, rm）
    assert_eq!(enc("mov x9, x10"), word_le(0xAA0A03E9));
    assert_eq!(enc("mov w9, w10"), word_le(0x2A0A03E9));
    // CMP/CMN（SUBS/ADDS rd=31）
    assert_eq!(enc("cmp x0, #5"), word_le(0xF100141F));
    assert_eq!(enc("cmp w0, #5"), word_le(0x7100141F));
    assert_eq!(enc("cmn x0, #5"), word_le(0xB100141F));
    assert_eq!(enc("cmp x1, x2"), word_le(0xEB02003F));
    assert_eq!(enc("cmp w1, w2"), word_le(0x6B02003F));
    // MOVZ/MOVN/MOVK（hw=0）与 mov #imm 别名
    assert_eq!(enc("movz x0, #0"), word_le(0xD2800000));
    assert_eq!(enc("movz w0, #0"), word_le(0x52800000));
    assert_eq!(enc("movz x5, #0x1234"), word_le(0xD2824685));
    assert_eq!(enc("movn x0, #0"), word_le(0x92800000));
    assert_eq!(enc("movn w0, #0"), word_le(0x12800000));
    assert_eq!(enc("movk x0, #0"), word_le(0xF2800000));
    assert_eq!(enc("movk w5, #0xABCD"), word_le(0x729579A5));
    // adds/sub 寄存器形式（A2 补）
    assert_eq!(enc("adds x5, x6, x7"), word_le(0xAB0700C5));
    assert_eq!(enc("subs w5, w6, w7"), word_le(0x6B0700C5));
}

#[test]
fn a2_invalid_rejected() {
    assert!(assemble("movz x0, #0x10000").is_err()); // imm16 上界 0xFFFF
    assert!(assemble("movk w0, #0x10000").is_err());
    assert!(assemble("orr x0, w1, x2").is_err()); // 宽度混用
    assert!(assemble("cmp x0, w1").is_err());
    assert!(assemble("mov w0, x1").is_err());
}

#[test]
fn golden_a3_mul_div_br_cbz_csel() {
    // 乘法/除法（clang oracle）
    assert_eq!(enc("madd x0, x1, x2, x3"), word_le(0x9B020C20));
    assert_eq!(enc("madd w0, w1, w2, w3"), word_le(0x1B020C20));
    assert_eq!(enc("msub x4, x5, x6, x7"), word_le(0x9B069CA4));
    assert_eq!(enc("msub w4, w5, w6, w7"), word_le(0x1B069CA4));
    assert_eq!(enc("mul x0, x1, x2"), word_le(0x9B027C20));
    assert_eq!(enc("mul w0, w1, w2"), word_le(0x1B027C20));
    assert_eq!(enc("sdiv x0, x1, x2"), word_le(0x9AC20C20));
    assert_eq!(enc("sdiv w0, w1, w2"), word_le(0x1AC20C20));
    assert_eq!(enc("udiv x0, x1, x2"), word_le(0x9AC20820));
    assert_eq!(enc("udiv w0, w1, w2"), word_le(0x1AC20820));
    // 间接分支
    assert_eq!(enc("br x3"), word_le(0xD61F0060));
    assert_eq!(enc("blr x3"), word_le(0xD63F0060));
    assert_eq!(enc("ret"), word_le(0xD65F03C0));
    // CBZ/CBNZ
    assert_eq!(enc("cbz x0, 0"), word_le(0xB4000000));
    assert_eq!(enc("cbnz w1, 0"), word_le(0x35000001));
    // CSEL 族
    assert_eq!(enc("csel x0, x1, x2, #0"), word_le(0x9A820020)); // eq
    assert_eq!(enc("csinc x0, x1, x2, #0"), word_le(0x9A820420));
    assert_eq!(enc("csinv x4, x5, x6, #1"), word_le(0xDA8610A4)); // ne
    assert_eq!(enc("csneg w4, w5, w6, #12"), word_le(0x5A86C4A4)); // gt
}

#[test]
fn golden_a4_memory() {
    // 无符号 imm12 用缩放单元（imm12=1 → X 真实偏移 8 字节 = clang [x1,#8]）
    assert_eq!(enc("ldr x0, [x1, #0]"), word_le(0xF9400020));
    assert_eq!(enc("ldr x0, [x1, #1]"), word_le(0xF9400420)); // clang [x1,#8]
    assert_eq!(enc("str x0, [x1, #0]"), word_le(0xF9000020));
    assert_eq!(enc("str w2, [x3, #0]"), word_le(0xB9000062));
    assert_eq!(enc("ldr w2, [x3, #1]"), word_le(0xB9400462)); // clang [x3,#4]（scale=2）
    assert_eq!(enc("ldr w2, [x3, #0x3FF]"), word_le(0xB94FFC62)); // clang #0xffc
    // LDUR/STUR 未缩放真字节
    assert_eq!(enc("ldur x0, [x1, #0]"), word_le(0xF8400020));
    assert_eq!(enc("ldur w2, [x3, #-8]"), word_le(0xB85F8062));
    assert_eq!(enc("stur w0, [x1, #4]"), word_le(0xB8004020));
}

#[test]
fn golden_a5_pair_and_sp() {
    // 帧/SP（clang oracle）
    assert_eq!(enc("sub sp, sp, #16"), word_le(0xD10043FF));
    assert_eq!(enc("add sp, sp, #16"), word_le(0x910043FF));
    assert_eq!(enc("add x29, sp, #0"), word_le(0x910003FD)); // mov x29, sp 别名
    // 寄存器对（imm7=缩放单元：X 对 #2 = 16 字节）
    assert_eq!(enc("stp x29, x30, [sp, #2]"), word_le(0xA9017BFD));
    assert_eq!(enc("ldp x29, x30, [sp, #2]"), word_le(0xA9417BFD));
    assert_eq!(enc("stp x0, x1, [x2, #0]"), word_le(0xA9000440));
    assert_eq!(enc("ldp x0, x1, [x2, #0]"), word_le(0xA9400440));
    assert_eq!(enc("stp w0, w1, [x2, #0]"), word_le(0x29000440));
    // SP 基址访存（imm12 缩放：#1 → 8 字节）
    assert_eq!(enc("str x0, [sp, #1]"), word_le(0xF90007E0));
    assert_eq!(enc("ldr x1, [sp, #1]"), word_le(0xF94007E1));
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
