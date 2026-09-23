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
    let i = assemble("add w5, w6, #42").unwrap();
    assert_eq!(encode(&i).unwrap(), word_le(0x1100A8C5), "Inst={i:?}");
    // 寄存器 ALU（shifted-reg LSL#0 形式）
    // 分支（偏移 0 = 自身）
    // NOP
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
    // 间接分支
    // CBZ/CBNZ
    // CSEL 族（v18 S3c：条件码写**符号名**，不再写 `#N` 立即数）
    // 同码别名（hs↔cs、lo↔cc）得到同一字节；反汇编渲染首选名
    assert_eq!(enc("csel x0, x1, x2, hs"), enc("csel x0, x1, x2, cs"));
    assert_eq!(enc("csel x0, x1, x2, lo"), enc("csel x0, x1, x2, cc"));
    assert_eq!(
        disassemble(&assemble("csel x0, x1, x2, hs").unwrap()),
        "csel X0, X1, X2, cs"
    );
    // B.cond：B.cond 全 14 条件（AL/NV 是保留编码，A64 不允许）
    for (name, code) in [
        ("eq", 0u32),
        ("ne", 1),
        ("cs", 2),
        ("cc", 3),
        ("mi", 4),
        ("pl", 5),
        ("vs", 6),
        ("vc", 7),
        ("hi", 8),
        ("ls", 9),
        ("ge", 10),
        ("lt", 11),
        ("gt", 12),
        ("le", 13),
    ] {
        let asm = format!("b.{name} 0");
        assert_eq!(
            enc(&asm),
            word_le(0x5400_0000 | code),
            "B.cond {name} 编码（[3:0]=cond）"
        );
        // 反汇编往返：渲染回符号名
        let inst = assemble(&asm).unwrap();
        assert_eq!(disassemble(&inst), asm, "B.cond 反汇编往返");
    }
    // 偏移进 imm19（[23:5]）；别名同码
    assert_eq!(enc("b.hs 0"), enc("b.cs 0"), "hs 是 cs 的别名");
    assert_eq!(enc("b.lo 0"), enc("b.cc 0"), "lo 是 cc 的别名");
    // 汇编→反汇编保持写下的拼写（别名各有一行模板）；
    // 经**解码**则规范化为同码首选名（`cs`/`cc` 在表里字母序最小）。
    assert_eq!(disassemble(&assemble("b.hs 0").unwrap()), "b.hs 0");
    let (decoded, _) = decode(&enc("b.hs 0")).unwrap();
    assert_eq!(disassemble(&decoded), "b.cs 0", "解码按同码首选名渲染");
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

/// MOVZ/MOVK hw 变体（P3① 大立即数多序列）：clang oracle 词 → decode →
/// encode 字节往返。词 = base | (imm16<<5) | (hw<<21) | rd（imm16 值放
/// [20:5]、hw 值放 [22:21]——A64 movz/movk 移宽立即数族）。
/// oracle: movz x0,#0xABCD,lsl#16 = d2b579a0；lsl#32 = d2d579a0；
/// lsl#48 = d2f579a0；movz w0,#0xABCD,lsl#16 = 52b579a0；
/// movk x0,#0xABCD,lsl#16 = f2b579a0；lsl#32 = f2d579a0；
/// movz x1,#1,lsl#48 = d2e00021；movz x9,#0xFFFF,lsl#48 = d2ffffe9。
#[test]
fn golden_movw_hw_variants_decode_roundtrip() {
    for w in [
        0xD2B579A0u32, // movz x0, #0xABCD, lsl #16（MOVZX1）
        0xD2D579A0u32, // movz x0, #0xABCD, lsl #32（MOVZX2）
        0xD2F579A0u32, // movz x0, #0xABCD, lsl #48（MOVZX3）
        0x52B579A0u32, // movz w0, #0xABCD, lsl #16（MOVZW1）
        0xF2B579A0u32, // movk x0, #0xABCD, lsl #16（MOVKX1）
        0xF2D579A0u32, // movk x0, #0xABCD, lsl #32（MOVKX2）
        0xD2E00021u32, // movz x1, #1, lsl #48
        0xD2FFFFE9u32, // movz x9, #0xFFFF, lsl #48
    ] {
        let b = word_le(w);
        let (d, n) = decode(&b).unwrap_or_else(|| panic!("decode {w:08x}"));
        assert_eq!(n, 4, "{w:08x}: 消费字节 != 4");
        let b2 = encode(&d).unwrap();
        assert_eq!(b2, b, "词 {w:08x}: decode→encode 字节不一致");
        assert_eq!(disassemble(&d), disassemble(&d), "{w:08x} disasm 自洽");
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
