//! 汇编器增强验证（D）：.equ 符号常量、立即数表达式、数据伪指令
//! （.word/.hword/.dword/.ascii/.asciz/.zero）、.macro/.endm、行号错误。
//!
//! 载体：表达式/.equ → riscv64_v12（`addi x1, x0, expr`，imm12 有符号）；
//! 数据伪指令/宏/行号 → demo_v12（定宽 32 位）。

use forge_codegen::machine::assembler::TargetAssembler;

// ── riscv：表达式 / .equ ──

fn rv_enc(asm: &str) -> u32 {
    let inst = forge_codegen::riscv64_v12::assemble(asm)
        .unwrap_or_else(|e| panic!("assemble `{asm}`: {e}"));
    let bytes =
        forge_codegen::riscv64_v12::encode(&inst).unwrap_or_else(|e| panic!("encode `{asm}`: {e}"));
    u32::from_le_bytes([bytes[0], bytes[1], bytes[2], bytes[3]])
}

fn rv_parse(src: &str) -> Vec<forge_codegen::riscv64_v12::Inst> {
    let asm = forge_codegen::riscv64_v12::Assembler;
    asm.parse_insts(src)
        .unwrap_or_else(|e| panic!("parse_insts: {e}"))
}

/// 提取 addi x1, x0, imm 的 imm12（signed 12 位，bit20-31）
fn addi_imm(w: u32) -> i64 {
    let raw = ((w >> 20) & 0xFFF) as i64;
    if raw >= 0x800 { raw - 0x1000 } else { raw }
}

// ─────────────────── .equ 符号常量 ───────────────────

#[test]
fn equ_symbol_in_immediate() {
    let insts = rv_parse(concat!(
        ".equ A, 5\n",
        ".equ B, 3\n",
        "addi x1, x0, A+B*2\n",
    ));
    assert_eq!(insts.len(), 1);
    // A+B*2 = 5+6 = 11
    let w = {
        let bytes = forge_codegen::riscv64_v12::encode(&insts[0]).unwrap();
        u32::from_le_bytes([bytes[0], bytes[1], bytes[2], bytes[3]])
    };
    assert_eq!(addi_imm(w), 11, "imm 应为 A+B*2 = 11");
}

#[test]
fn equ_forward_reference() {
    // .equ 顺序求值：前向引用应失败（顺序语义）
    let asm = forge_codegen::riscv64_v12::Assembler;
    let err = asm.parse_insts(concat!(".equ X, Y\n", ".equ Y, 5\n", "addi x1, x0, X\n"));
    assert!(err.is_err(), "前向 .equ 引用应失败: {err:?}");
}

// ─────────────────── 立即数表达式 ───────────────────

#[test]
fn immediate_expr_arithmetic() {
    // (4+3)*2-1 = 13
    assert_eq!(addi_imm(rv_enc("addi x1, x0, (4+3)*2-1")), 13);
    // 1<<4 | 1 = 17
    assert_eq!(addi_imm(rv_enc("addi x1, x0, 1<<4 | 1")), 17);
    // -(2+3) = -5
    assert_eq!(addi_imm(rv_enc("addi x1, x0, -(2+3)")), -5);
    // 17/5 = 3、17%5 = 2
    assert_eq!(addi_imm(rv_enc("addi x1, x0, 17/5")), 3);
    assert_eq!(addi_imm(rv_enc("addi x1, x0, 17%5")), 2);
    // 括号嵌套 + 位与： (0x30 & 0x1F) = 0x10
    assert_eq!(addi_imm(rv_enc("addi x1, x0, (0x30 & 0x1F)")), 0x10);
    // 按位非：~5 = -6
    assert_eq!(addi_imm(rv_enc("addi x1, x0, ~5")), -6);
}

#[test]
fn immediate_expr_in_branch() {
    // 标签槽表达式：beq x1, x2, 40+2 → off_b = 42
    let inst = forge_codegen::riscv64_v12::assemble("beq x1, x2, 40+2")
        .unwrap_or_else(|e| panic!("assemble beq: {e}"));
    let bytes = forge_codegen::riscv64_v12::encode(&inst).unwrap();
    // B 型 imm13：bit[12|10:5|4:1|11]（相对指令地址；asm 数字标签 = 偏移）
    let w = u32::from_le_bytes([bytes[0], bytes[1], bytes[2], bytes[3]]);
    // 提取 B 型偏移（与 encode 的 imm_b pieces 一致）：
    let v = ((w >> 31 & 1) << 12)
        | ((w >> 25 & 0x3F) << 5)
        | ((w >> 8 & 0xF) << 1)
        | ((w >> 7 & 1) << 11);
    let v = (v as i64) << 51 >> 51; // 13 位符号扩展
    assert_eq!(v, 42, "beq 标签表达式 40+2 = 42");
}

// ─────────────────── 数据伪指令（demo_v12）───────────────────

fn dm_parse(src: &str) -> Vec<forge_codegen::demo_v12::Inst> {
    let asm = forge_codegen::demo_v12::Assembler;
    asm.parse_insts(src)
        .unwrap_or_else(|e| panic!("parse_insts: {e}"))
}

#[test]
fn data_word_hword_dword() {
    use forge_codegen::demo_v12::Inst;
    let insts = dm_parse(concat!(
        ".word 0x11223344\n",
        ".hword 0x5566\n",
        ".dword 0x1122334455667788\n",
    ));
    assert_eq!(insts.len(), 3);
    assert!(matches!(&insts[0], Inst::Raw(b) if b == &[0x44, 0x33, 0x22, 0x11]));
    assert!(matches!(&insts[1], Inst::Raw(b) if b == &[0x66, 0x55]));
    assert!(
        matches!(&insts[2], Inst::Raw(b) if b == &[0x88, 0x77, 0x66, 0x55, 0x44, 0x33, 0x22, 0x11])
    );
}

#[test]
fn data_ascii_asciz_zero() {
    use forge_codegen::demo_v12::Inst;
    let insts = dm_parse(concat!(".ascii \"hi\"\n", ".asciz \"!\"\n", ".zero 3\n"));
    assert_eq!(insts.len(), 3);
    assert!(matches!(&insts[0], Inst::Raw(b) if b == b"hi"));
    assert!(matches!(&insts[1], Inst::Raw(b) if b == &[b'!', 0]));
    assert!(matches!(&insts[2], Inst::Raw(b) if b == &[0, 0, 0]));
}

#[test]
fn data_escape_sequences() {
    use forge_codegen::demo_v12::Inst;
    // 源文本含 \n \t \" 转义（rust 字符串里用 \\n 等表达源反斜杠）
    let src = ".ascii \"a\\nb\\t\\\"\"\n";
    let insts = dm_parse(src);
    let expect: Vec<u8> = vec![b'a', b'\n', b'b', b'\t', 0x22];
    assert!(
        matches!(&insts[0], Inst::Raw(b) if b == &expect),
        "{:?}",
        insts[0]
    );
}

// ─────────────────── .macro/.endm（demo_v12）───────────────────

#[test]
fn macro_expansion() {
    use forge_codegen::demo_v12::Inst;
    let insts = dm_parse(concat!(
        ".macro LD2 reg, imm\n",
        "mov %reg, w0\n",
        "addi %reg, %reg, %imm\n",
        ".endm\n",
        "LD2 w1, 7\n",
    ));
    assert_eq!(insts.len(), 2, "insts={:?}", insts);
    assert!(matches!(&insts[0], Inst::Mov16 { .. }));
    assert!(matches!(&insts[1], Inst::Addi16 { imm16: 7, .. }));
}

#[test]
fn macro_nested_expansion() {
    use forge_codegen::demo_v12::Inst;
    let insts = dm_parse(concat!(
        ".macro SETZ r\n",
        "mov %r, w0\n",
        ".endm\n",
        ".macro SETIMM r, v\n",
        "SETZ %r\n",
        "addi %r, %r, %v\n",
        ".endm\n",
        "SETIMM w2, 9\n",
    ));
    assert_eq!(insts.len(), 2, "insts={:?}", insts);
    assert!(matches!(&insts[0], Inst::Mov16 { .. }));
    assert!(matches!(&insts[1], Inst::Addi16 { imm16: 9, .. }));
}

#[test]
fn macro_error_missing_endm() {
    let asm = forge_codegen::demo_v12::Assembler;
    assert!(asm.parse_insts(concat!(".macro FOO\n", "nop\n")).is_err());
}

#[test]
fn macro_error_arg_count() {
    let asm = forge_codegen::demo_v12::Assembler;
    assert!(
        asm.parse_insts(concat!(".macro ONE a\n", "nop\n", ".endm\n", "ONE 1, 2\n"))
            .is_err()
    );
}

// ─────────────────── 结构化错误带行号 ───────────────────

#[test]
fn error_carries_line_number() {
    // demo：第 3 行语法错误
    let asm = forge_codegen::demo_v12::Assembler;
    let err = asm
        .parse_insts(concat!("nop\n", "nop\n", "frob r1, r2\n"))
        .unwrap_err();
    let msg = format!("{err}");
    assert!(msg.contains("line 3"), "语法错误应带行号: {msg}");
    // riscv：第 5 行未定义标签
    let asm = forge_codegen::riscv64_v12::Assembler;
    let err = asm
        .parse_insts(concat!(
            "nop\n",
            "nop\n",
            "nop\n",
            "nop\n",
            "beq x1, x2, nowhere\n"
        ))
        .unwrap_err();
    let msg = format!("{err}");
    assert!(msg.contains("line 5"), "未定义标签应带行号: {msg}");
}

#[test]
fn error_equ_bad_expr() {
    let asm = forge_codegen::riscv64_v12::Assembler;
    let err = asm.parse_insts(".equ X, 1+\n").unwrap_err();
    assert!(format!("{err}").contains("line 1"), "err: {err:?}");
}
