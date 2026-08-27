//! demo_v12 — 类型约束自动分发验证。
//!
//! 核心断言：同一助记符（`add`/`mov`）的 16/32/64 位寄存器版本由汇编器按
//! 操作数实际类型自动分发到不同 opcode；混宽拒绝；空白免疫；decode→encode
//! 字节往返；disassemble→assemble 往返。

use forge_codegen::demo_v12::{Inst, assemble, decode, disassemble, encode};

fn enc(asm: &str) -> Vec<u8> {
    let inst = assemble(asm).unwrap_or_else(|e| panic!("assemble `{asm}`: {e}"));
    encode(&inst).unwrap_or_else(|e| panic!("encode `{asm}`: {e}"))
}

/// 32 位小端字：opcode | rd<<8 | rs1<<11 | rs2<<14
fn word(b0: u8, rd: u32, rs1: u32, rs2: u32) -> Vec<u8> {
    let w = (b0 as u32) | (rd << 8) | (rs1 << 11) | (rs2 << 14);
    w.to_le_bytes().to_vec()
}

// ─────────────────── 类型约束自动分发（核心） ───────────────────

#[test]
fn same_mnemonic_dispatch_by_operand_width() {
    // add：`w`/`r`/`x` 前缀 = 16/32/64 位类 → 自动选 ADD16/ADD32/ADD64
    assert_eq!(enc("add w1, w2, w3"), word(0x10, 1, 2, 3));
    assert_eq!(enc("add r1, r2, r3"), word(0x11, 1, 2, 3));
    assert_eq!(enc("add x1, x2, x3"), word(0x12, 1, 2, 3));
    // mov：二操作数（rd<<8 | rs1<<11）
    assert_eq!(enc("mov w1, w2"), word(0x20, 1, 2, 0));
    assert_eq!(enc("mov r1, r2"), word(0x21, 1, 2, 0));
    assert_eq!(enc("mov x1, x2"), word(0x22, 1, 2, 0));
}

#[test]
fn dispatched_inst_variants_are_distinct() {
    // 分发结果必须是不同 Inst 变体（不同编码）
    let a = assemble("add w1, w2, w3").unwrap();
    let b = assemble("add r1, r2, r3").unwrap();
    let c = assemble("add x1, x2, x3").unwrap();
    assert!(a != b && b != c && a != c);
    assert!(matches!(a, Inst::Add16 { .. }));
    assert!(matches!(b, Inst::Add32 { .. }));
    assert!(matches!(c, Inst::Add64 { .. }));
}

#[test]
fn mixed_width_operands_rejected() {
    // 混宽操作数无 form 匹配 → 报错（不再静默按首操作数编码）
    assert!(assemble("add r1, x2, r3").is_err());
    assert!(assemble("mov w1, x2").is_err());
    assert!(assemble("add x1, x1, w2").is_err());
}

#[test]
fn specific_form_wins_over_polymorphic_declaration_order() {
    // 声明序 add16 → add32 → add64；`add r1, r2, r3` 必须命中 ADD32 而非
    // 提前命中的 ADD16（约束过滤）——类型签名分发而非首形状。
    let inst = assemble("add r1, r2, r3").unwrap();
    assert!(matches!(inst, Inst::Add32 { .. }));
}

// ─────────────────── token 化：空白免疫 ───────────────────

#[test]
fn whitespace_insensitive() {
    assert_eq!(enc("add  r1 ,  r2 , r3"), enc("add r1, r2, r3"));
    assert_eq!(enc("  mov   w0  ,  w7  "), enc("mov w0, w7"));
    assert_eq!(enc("ADD R1,R2,R3"), enc("add r1, r2, r3")); // 助记符大小写不敏感
}

// ─────────────────── 立即数/标签 ───────────────────

#[test]
fn label_numeric_and_sym_rejection() {
    // 数字标签
    assert_eq!(enc("brz r1, 42"), {
        let w = 0x30u32 | (1 << 11) | (42 << 16);
        w.to_le_bytes().to_vec()
    });
    // 符号标签在单条 assemble 中拒绝（需 parse_insts 两遍布局）
    assert!(assemble("brz r1, loop").is_err());
    // 标签槽收到寄存器名 → 拒绝
    assert!(assemble("brz r1, r2").is_err());
}

#[test]
fn immediate_range_check() {
    // lab 槽：signed 16 位 → [-32768, 32767]；越界报错（不再静默截断）
    assert!(assemble("brz r1, 32767").is_ok());
    assert!(assemble("brz r1, -32768").is_ok());
    assert!(assemble("brz r1, 32768").is_err());
    assert!(assemble("brz r1, -32769").is_err());
    // 符号引用不受范围限制（布局期回填 Block 索引）
    assert!(assemble("brz r1, big_label").is_err()); // 单条 API 拒绝符号
    // 负立即数（-0x10 十六进制）
    assert_eq!(enc("brz r1, -0x10"), {
        let w = 0x30u32 | (1 << 11) | ((-16i32) as u32 & 0xFFFF) << 16;
        w.to_le_bytes().to_vec()
    });
}

#[test]
fn roundtrip_bytes_and_text() {
    for asm in [
        "add w1, w2, w3",
        "add r4, r5, r6",
        "add x0, x7, x3",
        "mov w0, w7",
        "mov r3, r3",
        "mov x5, x1",
        "brz r1, 42",
        "brz r2, -4",
        "nop",
    ] {
        let b = enc(asm);
        let (d, n) = decode(&b).unwrap_or_else(|| panic!("decode {b:02x?} (`{asm}`)"));
        assert_eq!(n, 4, "`{asm}`: consumed != 4");
        let b2 = encode(&d).unwrap();
        assert_eq!(b2, b, "`{asm}`: decode→encode 字节不一致");
        // disassemble → reassemble 必须还原同一指令
        let text = disassemble(&d);
        let d2 = assemble(&text).unwrap_or_else(|e| panic!("reassemble `{text}`: {e}"));
        assert_eq!(d2, d, "`{asm}` → `{text}` 往返");
    }
}

#[test]
fn unknown_mnemonic_and_bad_syntax() {
    assert!(assemble("frob r1, r2").is_err());
    assert!(assemble("add").is_err());
    assert!(assemble("add r1 r2 r3").is_err()); // 缺逗号
    assert!(assemble("add r1, r2, 0xZZ").is_err()); // 非法十六进制
    assert!(assemble("").is_err());
}

// ─────────────────── parse_insts 两遍布局（标签） ───────────────────

#[test]
fn parse_insts_two_pass_labels() {
    use forge_codegen::machine::assembler::TargetAssembler;
    let asm = forge_codegen::demo_v12::Assembler;
    // 前向标签 + 后向引用 + 空行/注释/尾随注释
    let insts = asm
        .parse_insts("brz r1, loop\nloop: nop\n# comment\n\nbrz r2, loop # trailing\n")
        .unwrap();
    assert_eq!(insts.len(), 3);
    // label 槽回填为 Block 索引（两处引用同一 loop = 块 1）
    match (&insts[0], &insts[2]) {
        (Inst::Brz { imm16: a, .. }, Inst::Brz { imm16: b, .. }) => {
            assert_eq!(*a, 1);
            assert_eq!(*b, 1);
        }
        other => panic!("unexpected {other:?}"),
    }
}

#[test]
fn parse_insts_undefined_label() {
    use forge_codegen::machine::assembler::{AsmError, TargetAssembler};
    let asm = forge_codegen::demo_v12::Assembler;
    let err = asm.parse_insts("brz r1, nowhere").unwrap_err();
    assert!(matches!(err, AsmError::UndefinedLabel(_)));
}

#[test]
fn parse_insts_labels_resolve_consistently() {
    use forge_codegen::demo_v12::{decode, encode};
    use forge_codegen::machine::assembler::TargetAssembler;
    let asm = forge_codegen::demo_v12::Assembler;
    // 标签展开后编码：两处 brz 引用 loop（块 1 = nop 所在块）
    let insts = asm
        .parse_insts("brz r1, loop\nloop: nop\nbrz r2, loop")
        .unwrap();
    let b0 = encode(&insts[0]).unwrap();
    let b2 = encode(&insts[2]).unwrap();
    assert_eq!(&b0[2..4], &[1, 0], "brz#0 标签回填 1");
    assert_eq!(&b2[2..4], &[1, 0], "brz#1 标签回填 1");
    // decode 往返
    let (d, n) = decode(&b0).unwrap();
    assert_eq!(n, 4);
    assert_eq!(encode(&d).unwrap(), b0);
}

// ─────────────────── 伪指令（.byte/.align/.global） ───────────────────

#[test]
fn parse_insts_directives() {
    use forge_codegen::demo_v12::{Inst, encode};
    use forge_codegen::machine::assembler::TargetAssembler;
    let asm = forge_codegen::demo_v12::Assembler;
    // .byte 原样字节；.align 4 在 offset=8 时无填充；标签仍按块索引
    let insts = asm
        .parse_insts(".byte 0xde, 0xad, 0xbe, 0xef\nmov r1, r2\n.align 4\nloop: nop\nbrz r1, loop")
        .unwrap();
    // [Raw(deadbeef), Mov32, Nop, Brz]（align 无填充）
    assert_eq!(insts.len(), 4);
    assert_eq!(insts[0], Inst::Raw(vec![0xde, 0xad, 0xbe, 0xef]));
    assert!(matches!(insts[1], Inst::Mov32 { .. }));
    // 标签 loop → 块 2（nop 所在）
    assert!(matches!(&insts[3], Inst::Brz { imm16: 2, .. }));
    // 总字节 = 4 + 4 + 4 + 4 = 16（align 对齐）
    let total: usize = insts.iter().map(|i| encode(i).unwrap().len()).sum();
    assert_eq!(total, 16);
}

#[test]
fn parse_insts_align_pads() {
    use forge_codegen::demo_v12::{Inst, encode};
    use forge_codegen::machine::assembler::TargetAssembler;
    let asm = forge_codegen::demo_v12::Assembler;
    // .byte 1 字节 → .align 4 → 填 3 字节 0x00 → nop
    let insts = asm.parse_insts(".byte 0x01\n.align 4\nnop").unwrap();
    assert_eq!(insts.len(), 3);
    assert!(matches!(&insts[1], Inst::Raw(v) if v == &vec![0u8; 3]));
    let total: usize = insts.iter().map(|i| encode(i).unwrap().len()).sum();
    assert_eq!(total, 8);
    // disassemble Raw 渲染
    assert_eq!(
        forge_codegen::demo_v12::disassemble(&insts[0]),
        ".byte 0x01"
    );
}

#[test]
fn parse_insts_directive_errors() {
    use forge_codegen::machine::assembler::{AsmError, TargetAssembler};
    let asm = forge_codegen::demo_v12::Assembler;
    assert!(matches!(
        asm.parse_insts(".frob 1").unwrap_err(),
        AsmError::Other(_)
    ));
    assert!(matches!(
        asm.parse_insts(".byte 0x100").unwrap_err(),
        AsmError::Other(_) // 越界字节
    ));
    // .global 接受（符号登记，无消费方）
    assert!(asm.parse_insts(".global foo\nnop").is_ok());
}
