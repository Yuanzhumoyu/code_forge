//! 汇编器增强验证（D）：.equ 符号常量、立即数表达式、数据伪指令
//! （.word/.hword/.dword/.ascii/.asciz/.zero）、.macro/.endm、行号错误。
//!
//! 载体：表达式/.equ → riscv64（`addi x1, x0, expr`，imm12 有符号）；
//! 数据伪指令/宏/行号 → demo（定宽 32 位）。

mod common;

use forge_codegen::machine::assembler::TargetAssembler;

// ── riscv：表达式 / .equ ──

fn rv_enc(asm: &str) -> u32 {
    let inst =
        forge_codegen::riscv64::assemble(asm).unwrap_or_else(|e| panic!("assemble `{asm}`: {e}"));
    let bytes =
        forge_codegen::riscv64::encode(&inst).unwrap_or_else(|e| panic!("encode `{asm}`: {e}"));
    u32::from_le_bytes([bytes[0], bytes[1], bytes[2], bytes[3]])
}

fn rv_parse(src: &str) -> Vec<forge_codegen::riscv64::Inst> {
    let asm = forge_codegen::riscv64::Assembler;
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
        let bytes = forge_codegen::riscv64::encode(&insts[0]).unwrap();
        u32::from_le_bytes([bytes[0], bytes[1], bytes[2], bytes[3]])
    };
    assert_eq!(addi_imm(w), 11, "imm 应为 A+B*2 = 11");
}

#[test]
fn equ_forward_reference() {
    // .equ 顺序求值：前向引用应失败（顺序语义）
    let asm = forge_codegen::riscv64::Assembler;
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
    let inst = forge_codegen::riscv64::assemble("beq x1, x2, 40+2")
        .unwrap_or_else(|e| panic!("assemble beq: {e}"));
    let bytes = forge_codegen::riscv64::encode(&inst).unwrap();
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

// ─────────────────── 数据伪指令（demo）───────────────────

fn dm_parse(src: &str) -> Vec<common::demo::Inst> {
    let asm = common::demo::Assembler;
    asm.parse_insts(src)
        .unwrap_or_else(|e| panic!("parse_insts: {e}"))
}

#[test]
fn data_word_hword_dword() {
    use common::demo::Inst;
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
    use common::demo::Inst;
    let insts = dm_parse(concat!(".ascii \"hi\"\n", ".asciz \"!\"\n", ".zero 3\n"));
    assert_eq!(insts.len(), 3);
    assert!(matches!(&insts[0], Inst::Raw(b) if b == b"hi"));
    assert!(matches!(&insts[1], Inst::Raw(b) if b == &[b'!', 0]));
    assert!(matches!(&insts[2], Inst::Raw(b) if b == &[0, 0, 0]));
}

#[test]
fn data_escape_sequences() {
    use common::demo::Inst;
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

// ─────────────────── .macro/.endm（demo）───────────────────

#[test]
fn macro_expansion() {
    use common::demo::Inst;
    let insts = dm_parse(concat!(
        ".macro LD2 reg, imm\n",
        "mov %reg, w0\n",
        "addi %reg, %reg, %imm\n",
        ".endm\n",
        "LD2 w1, 7\n",
    ));
    assert_eq!(insts.len(), 2, "insts={:?}", insts);
    assert!(matches!(&insts[0], Inst::Mov16 { .. }));
    assert!(matches!(&insts[1], Inst::Addi16 { imm: 7, .. }));
}

#[test]
fn macro_nested_expansion() {
    use common::demo::Inst;
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
    assert!(matches!(&insts[1], Inst::Addi16 { imm: 9, .. }));
}

#[test]
fn macro_error_missing_endm() {
    let asm = common::demo::Assembler;
    assert!(asm.parse_insts(concat!(".macro FOO\n", "nop\n")).is_err());
}

#[test]
fn macro_error_arg_count() {
    let asm = common::demo::Assembler;
    assert!(
        asm.parse_insts(concat!(".macro ONE a\n", "nop\n", ".endm\n", "ONE 1, 2\n"))
            .is_err()
    );
}

// ─────────────────── 结构化错误带行号 ───────────────────

#[test]
fn error_carries_line_number() {
    // demo：第 3 行语法错误
    let asm = common::demo::Assembler;
    let err = asm
        .parse_insts(concat!("nop\n", "nop\n", "frob r1, r2\n"))
        .unwrap_err();
    let msg = format!("{err}");
    assert!(msg.contains("line 3"), "语法错误应带行号: {msg}");
    // riscv：第 5 行未定义标签
    let asm = forge_codegen::riscv64::Assembler;
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
    let asm = forge_codegen::riscv64::Assembler;
    let err = asm.parse_insts(".equ X, 1+\n").unwrap_err();
    assert!(format!("{err}").contains("line 1"), "err: {err:?}");
}

// ─────────────────── 符号常量进位移 / 一元 `+` ───────────────────

fn x86_bytes(src: &str) -> Vec<u8> {
    let asm = forge_codegen::x86::Assembler;
    let insts = asm
        .parse_insts(src)
        .unwrap_or_else(|e| panic!("parse_insts `{src}`: {e}"));
    let mut out = Vec::new();
    for i in &insts {
        out.extend(forge_codegen::x86::encode(i).unwrap_or_else(|e| panic!("encode `{src}`: {e}")));
    }
    out
}

/// `.set` 与 `.equ` 是同一个伪指令的两种拼法（上游 llvm-mc 用例用 `.set`），
/// 符号常量在**立即数**里可用（x86 `imm32` 槽，上游期望字节 `83 /7 ib`）。
#[test]
fn symbol_constant_in_immediate() {
    // `asm/parse/x86/llvm-mc/intel-syntax-encoding.s`：.set FOO, 2 / cmp eax, FOO
    assert_eq!(
        x86_bytes(".set FOO, 2\ncmp eax, FOO\n"),
        vec![0x83, 0xf8, 0x02]
    );
    assert_eq!(
        x86_bytes(".equ FOO, 2\ncmp eax, FOO\n"),
        vec![0x83, 0xf8, 0x02]
    );
    // 符号参与算术（值语法与字面量完全同一套）
    assert_eq!(
        x86_bytes(".set FOO, 1\ncmp eax, FOO+1\n"),
        vec![0x83, 0xf8, 0x02]
    );
}

/// 符号常量在**位移**里同样可用：位移与立即数共用 `__expr`（这条正是旧实现缺的
/// 第二套读数——位移只认字面量）。GAS Intel 的 `disp[base]` 与 `[base+disp]` 等价。
#[test]
fn symbol_constant_in_displacement() {
    assert_eq!(
        x86_bytes(".set FOO, 2\nmov rax, FOO[rbx]\n"),
        vec![0x48, 0x8b, 0x43, 0x02]
    );
    assert_eq!(
        x86_bytes("mov rax, [rbx+2]\n"),
        vec![0x48, 0x8b, 0x43, 0x02]
    );
    // 位移表达式（符号 + 算术）：FOO*2 = 4
    assert_eq!(
        x86_bytes(".set FOO, 2\nmov rax, [rbx+FOO*2]\n"),
        vec![0x48, 0x8b, 0x43, 0x04]
    );
    // 无基址的绝对寻址里同样是值语法：`[FOO]` = `[2]`（SIB 无基址 + disp32）
    assert_eq!(
        x86_bytes(".set FOO, 2\nmov rax, [FOO]\n"),
        x86_bytes("mov rax, [2]\n")
    );
}

/// `.set`/`.equ` 是**顺序**语义：后面的引用取新值（语料 `rv32i-valid.s` 里
/// `CONST` 先 30 后 16 就靠这个——整篇一起求值会把两个引用解成同一个值）。
#[test]
fn set_redefinition_is_sequential() {
    assert_eq!(
        rv_words(".set C, 30\nlui a0, C\n.set C, 16\nlui a0, C\n"),
        rv_words("lui a0, 30\nlui a0, 16\n")
    );
}

/// 一元 `+` 是恒等（上游语料 `lwu x2, +4(x3)`、GAS `[+8]` 都这么写）。
#[test]
fn unary_plus_is_identity() {
    assert_eq!(rv_enc("addi x1, x0, +5"), rv_enc("addi x1, x0, 5"));
    assert_eq!(rv_enc("addi x1, x0, -(+5)"), rv_enc("addi x1, x0, -5"));
    assert_eq!(x86_bytes("mov rax, [+8]\n"), x86_bytes("mov rax, [8]\n"));
    assert_eq!(x86_bytes("mov rax, [-8]\n"), x86_bytes("mov rax, [0-8]\n"));
}

/// **地址尺寸覆盖（0x67）**：64 位模式下 32 位地址寄存器（`[eax]`）要发 `0x67`，
/// 否则地址尺寸错（改前实测 `add eax, [eax]` 编成 `03 00`，真值 `67 03 00`）。
#[test]
fn address_size_override_prefix() {
    assert_eq!(x86_bytes("add eax, [eax]\n"), vec![0x67, 0x03, 0x00]);
    assert_eq!(x86_bytes("mov rax, [eax]\n"), vec![0x67, 0x48, 0x8b, 0x00]);
    // SIB 索引也参与地址尺寸（`[rbx+eax*4]`）
    assert_eq!(
        x86_bytes("add rax, [rbx+eax*4]\n"),
        vec![0x67, 0x48, 0x03, 0x04, 0x83]
    );
    // 上游 llvm-mc 用例的期望字节：`.set FOO, 2` + `cmp eax, FOO[eax]`
    assert_eq!(
        x86_bytes(".set FOO, 2\ncmp eax, FOO[eax]\n"),
        vec![0x67, 0x3b, 0x40, 0x02]
    );
    // 64 位地址（缺省）**不发**前缀
    assert_eq!(x86_bytes("add eax, [rax]\n"), vec![0x03, 0x00]);
    assert_eq!(
        x86_bytes("add rax, [rbx+rcx*4]\n"),
        vec![0x48, 0x03, 0x04, 0x8b]
    );
}

/// 地址尺寸覆盖是**编解码对称**的：`67 03 00` 解出来是 `[EAX]`（不是 `[RAX]`），
/// 重新编码逐字节相同——否则 `[eax]` 与 `[rax]` 会被当成同一条。
#[test]
fn address_size_override_round_trips() {
    use forge_codegen::x86::{decode, disassemble, encode};
    for (asm, want) in [
        ("add eax, [eax]", vec![0x67, 0x03, 0x00]),
        ("add rax, [rbx+eax*4]", vec![0x67, 0x48, 0x03, 0x04, 0x83]),
    ] {
        let inst = forge_codegen::x86::assemble(asm).unwrap_or_else(|e| panic!("{asm}: {e}"));
        assert_eq!(encode(&inst).unwrap(), want, "{asm}");
        let (back, used) = decode(&want).unwrap_or_else(|| panic!("decode `{asm}` 失败"));
        assert_eq!(used, want.len(), "{asm} 解码未吃满");
        assert_eq!(encode(&back).unwrap(), want, "{asm} 往返字节变了");
        let text = disassemble(&back);
        assert!(text.contains("EAX"), "{asm} 反汇编应保留 32 位地址：{text}");
    }
}

/// 非地址类寄存器当基址/索引仍被拒（`[al]`/`[xmm0]` 不是地址）。
#[test]
fn non_address_class_base_is_rejected() {
    let asm = forge_codegen::x86::Assembler;
    for src in ["mov rax, [al]", "add eax, [xmm0]", "add rax, [rbx+xmm1*4]"] {
        assert!(asm.parse_insts(src).is_err(), "`{src}` 应当被拒");
    }
}

// ─────────────────── 内存形式的 mov 族（8/16/32 位）───────────────────

/// 真实汇编里最常见的 `mov r8/16/32, [mem]` 与 `mov [mem], r8/16/32`：**完整内存模板**
/// （位移 / 索引 / 尺寸前缀 / 无基址）都要能解。改前只有两条窄路：
/// `MOV_R_MEM` 的 `[{reg}]` 简写（没有位移，是给 IR 的 vreg 基址用的）与 `opsize = 64`
/// 固定的 `MOV64_RM`/`MOV64_MR` ⇒ `mov eax, [rbx+8]` **一条候选都没有**。
#[test]
fn memory_mov_family_8_16_32() {
    // 8 位是独立操作码（8A/88）
    assert_eq!(x86_bytes("mov al, [rbx+2]\n"), vec![0x8a, 0x43, 0x02]);
    assert_eq!(x86_bytes("mov [rbx+2], al\n"), vec![0x88, 0x43, 0x02]);
    // 16 位：66 由数据寄存器宽度驱动
    assert_eq!(x86_bytes("mov ax, [rbx+2]\n"), vec![0x66, 0x8b, 0x43, 0x02]);
    assert_eq!(x86_bytes("mov [rbx+2], ax\n"), vec![0x66, 0x89, 0x43, 0x02]);
    // 32 位
    assert_eq!(x86_bytes("mov eax, [rbx+2]\n"), vec![0x8b, 0x43, 0x02]);
    assert_eq!(x86_bytes("mov [rbx+2], eax\n"), vec![0x89, 0x43, 0x02]);
    // 高编号寄存器（REX.R/B）
    assert_eq!(
        x86_bytes("mov r8d, [rbx+2]\n"),
        vec![0x44, 0x8b, 0x43, 0x02]
    );
    assert_eq!(
        x86_bytes("mov [rbx+2], r8d\n"),
        vec![0x44, 0x89, 0x43, 0x02]
    );
    assert_eq!(
        x86_bytes("mov r8w, [rbx+2]\n"),
        vec![0x66, 0x44, 0x8b, 0x43, 0x02]
    );
    // 尺寸前缀只是给人读的提示（不进字节）；索引与无基址风味照旧
    assert_eq!(
        x86_bytes("mov al, byte ptr [rbx+2]\n"),
        x86_bytes("mov al, [rbx+2]\n")
    );
    assert_eq!(
        x86_bytes("mov word ptr [rbx+2], ax\n"),
        x86_bytes("mov [rbx+2], ax\n")
    );
    assert_eq!(
        x86_bytes("mov eax, [rbx+rcx*4+8]\n"),
        vec![0x8b, 0x44, 0x8b, 0x08]
    );
    assert_eq!(
        x86_bytes("mov eax, [0x12345678]\n"),
        vec![0x8b, 0x04, 0x25, 0x78, 0x56, 0x34, 0x12]
    );
}

/// `movsxd r64, dword ptr [mem]`（原来只有寄存器源）与带位移的 `xchg [mem], r`
/// （原来的 `XCHG_MEM_R` 是 `[{reg}]` 简写，表示不了位移）。
#[test]
fn memory_movsxd_and_xchg() {
    assert_eq!(
        x86_bytes("movsxd rax, dword ptr [rbx]\n"),
        vec![0x48, 0x63, 0x03]
    );
    assert_eq!(
        x86_bytes("movsxd rax, [rbx+4]\n"),
        vec![0x48, 0x63, 0x43, 0x04]
    );
    assert_eq!(
        x86_bytes("xchg [rbx+8], rax\n"),
        vec![0x48, 0x87, 0x43, 0x08]
    );
    assert_eq!(x86_bytes("xchg [rbx+8], eax\n"), vec![0x87, 0x43, 0x08]);
    // 无位移的写法仍由声明在前的 `XCHG_MEM_R` 命中（同字节）
    assert_eq!(x86_bytes("xchg [rbx], rax\n"), vec![0x48, 0x87, 0x03]);
}

/// 新增内存 mov 族的**编解码闭环**（解码吃满、重编码逐字节相同）。
#[test]
fn memory_mov_family_round_trips() {
    use forge_codegen::x86::{decode, encode};
    for (asm, want) in [
        ("mov eax, [rbx+2]", vec![0x8b, 0x43, 0x02]),
        ("mov ax, [rbx+2]", vec![0x66, 0x8b, 0x43, 0x02]),
        ("mov [rbx+2], eax", vec![0x89, 0x43, 0x02]),
        ("mov al, [rbx+2]", vec![0x8a, 0x43, 0x02]),
        ("movsxd rax, dword ptr [rbx]", vec![0x48, 0x63, 0x03]),
        ("xchg [rbx+8], rax", vec![0x48, 0x87, 0x43, 0x08]),
    ] {
        let inst = forge_codegen::x86::assemble(asm).unwrap_or_else(|e| panic!("{asm}: {e}"));
        assert_eq!(encode(&inst).unwrap(), want, "{asm}");
        let (back, used) = decode(&want).unwrap_or_else(|| panic!("decode `{asm}` 失败"));
        assert_eq!(used, want.len(), "{asm} 解码未吃满");
        assert_eq!(encode(&back).unwrap(), want, "{asm} 往返字节变了");
    }
}

// ─────────────────── SSE 比较谓词（`0F C2 /r ib`）与经典别名 ───────────────────

/// 谓词立即数本体（`ps`/`pd`/`ss`/`sd` 靠**强制前缀**区分）与经典八谓词的 packed 别名
/// （`[[pseudo]]` 文本展开成一条 `cmpps/cmppd`，不新增指令、不加第二套分派）。
#[test]
fn sse_compare_predicates() {
    // 本体：0F C2 /r ib
    assert_eq!(
        x86_bytes("cmpps xmm2, xmm1, 1\n"),
        vec![0x0f, 0xc2, 0xd1, 0x01]
    );
    assert_eq!(
        x86_bytes("cmppd xmm2, xmm1, 1\n"),
        vec![0x66, 0x0f, 0xc2, 0xd1, 0x01]
    );
    assert_eq!(
        x86_bytes("cmpss xmm2, xmm1, 1\n"),
        vec![0xf3, 0x0f, 0xc2, 0xd1, 0x01]
    );
    assert_eq!(
        x86_bytes("cmpsd xmm2, xmm1, 1\n"),
        vec![0xf2, 0x0f, 0xc2, 0xd1, 0x01]
    );
    // 别名就是同一条指令——`asm/parse/x86/llvm-mc/intel-syntax-encoding.s` 的
    // `cmpltps XMM2, XMM1` 期望字节正是 `0F C2 D1 01`
    assert_eq!(
        x86_bytes("cmpltps XMM2, XMM1\n"),
        vec![0x0f, 0xc2, 0xd1, 0x01]
    );
    // 八谓词各编出各自的立即数
    let mut seen = std::collections::BTreeSet::new();
    for (name, pred) in [
        ("cmpeqps", 0u8),
        ("cmpltps", 1),
        ("cmpleps", 2),
        ("cmpunordps", 3),
        ("cmpneqps", 4),
        ("cmpnltps", 5),
        ("cmpnleps", 6),
        ("cmpordps", 7),
    ] {
        let got = x86_bytes(&format!("{name} xmm2, xmm1\n"));
        assert_eq!(got, vec![0x0f, 0xc2, 0xd1, pred], "{name}");
        assert!(seen.insert(got), "{name} 的字节与别的别名撞了");
    }
    assert_eq!(
        x86_bytes("cmpordpd xmm2, xmm1\n"),
        vec![0x66, 0x0f, 0xc2, 0xd1, 0x07]
    );
    // 别名不会吃掉 `cmp`（整词命中）：`cmp eax, ebx` 仍是 `39 D8`
    assert_eq!(x86_bytes("cmp eax, ebx\n"), vec![0x39, 0xd8]);
}

/// 谓词指令的编解码闭环：解码吃满、重编码逐字节相同、反汇编回**本体**写法。
#[test]
fn sse_compare_round_trips() {
    use forge_codegen::x86::{decode, disassemble, encode};
    for (asm, want) in [
        ("cmpltps XMM2, XMM1", vec![0x0f, 0xc2, 0xd1, 0x01]),
        ("cmppd xmm2, xmm1, 7", vec![0x66, 0x0f, 0xc2, 0xd1, 0x07]),
        ("cmpss xmm3, xmm4, 3", vec![0xf3, 0x0f, 0xc2, 0xdc, 0x03]),
    ] {
        let inst = forge_codegen::x86::assemble(asm).unwrap_or_else(|e| panic!("{asm}: {e}"));
        assert_eq!(encode(&inst).unwrap(), want, "{asm}");
        let (back, used) = decode(&want).unwrap_or_else(|| panic!("decode `{asm}` 失败"));
        assert_eq!(used, want.len(), "{asm} 解码未吃满");
        assert_eq!(encode(&back).unwrap(), want, "{asm} 往返字节变了");
        let text = disassemble(&back);
        assert!(
            text.starts_with("cmpps")
                || text.starts_with("cmppd")
                || text.starts_with("cmpss")
                || text.starts_with("cmpsd"),
            "{asm} 反汇编应给本体写法：{text}"
        );
    }
}

// ─────────────────── [[pseudo]] 伪指令展开（S3e）───────────────────

fn rv_words(src: &str) -> Vec<u32> {
    rv_parse(src)
        .iter()
        .map(|i| {
            let b = forge_codegen::riscv64::encode(i).unwrap();
            u32::from_le_bytes([b[0], b[1], b[2], b[3]])
        })
        .collect()
}

/// `li rd, imm` → `lui` + `addi`：逐值对照 RISC-V 规范分解
/// （hi20 = (v + 0x800) >> 12、lo12 = 符号扩展的低 12 位）。
#[test]
fn pseudo_li_expands_to_lui_addi() {
    // lui x10, 0x1；addi a0, a0, 0x234
    assert_eq!(rv_words("li x10, 0x1234\n"), vec![0x0000_1537, 0x2345_0513]);
    // 小值：lui x10, 0 + addi x10, x10, 0
    assert_eq!(rv_words("li x10, 0\n"), vec![0x0000_0537, 0x0005_0513]);
    // -1：lui a0, 0 + addi a0, a0, -1（imm12 = 0xFFF）
    assert_eq!(rv_words("li x10, -1\n"), vec![0x0000_0537, 0xFFF5_0513]);
    // 0x800：hi 进位 + lo = -2048（imm12 = 0x800）
    assert_eq!(rv_words("li x10, 0x800\n"), vec![0x0000_1537, 0x8005_0513]);
    // 表达式实参走既有表达式求值器（与字面量结果一致）
    assert_eq!(
        rv_words("li x10, (1 << 12) + 0x234\n"),
        rv_words("li x10, 0x1234\n")
    );
    // 多条：伪指令与普通指令混排，偏移不错位
    assert_eq!(rv_words("li x10, 0x1234\nnop\n").len(), 3);
}

/// 新版 `[[pseudo]]`（`asm` 声明形态）：切参按**模板字面**走，写法不符/参数不对
/// ⇒ 报错**回显写法**；展开行自己装配失败时点名是哪一行展开出来的。
#[test]
fn pseudo_asm_template_is_matched_and_reported() {
    let asm = forge_codegen::riscv64::Assembler;
    // 合法写法：逗号两侧空格随意（`li x10,0x1234` 也认），表达式实参保原样
    for src in ["li x10, 0x1234", "li x10 , 0x1234", "li x10,0x1234"] {
        let n = asm
            .parse_insts(src)
            .unwrap_or_else(|e| panic!("{src}: {e}"));
        assert_eq!(n.len(), 2, "{src} 应展开成两条");
    }
    // 缺分隔符 ⇒ 报错并给出写法
    let msg = format!("{}", asm.parse_insts("li a0").unwrap_err());
    assert!(msg.contains("伪指令写法 'li {rd}, {imm}'"), "msg: {msg}");
    assert!(msg.contains("缺 ','"), "msg: {msg}");
    // 多一个参数（最后一个实参里多逗号）
    let msg = format!("{}", asm.parse_insts("li x10, 1, 2").unwrap_err());
    assert!(msg.contains("需要 2 个参数"), "msg: {msg}");
    assert!(msg.contains("多了逗号"), "msg: {msg}");
    // 展开行自己装配失败 ⇒ 点名展开后的那一行（否则只有 "no matching instruction"）
    let msg = format!("{}", asm.parse_insts("li x10, 0x1234 junk").unwrap_err());
    assert!(msg.contains("伪指令展开行"), "msg: {msg}");
    assert!(msg.contains("lui x10"), "msg: {msg}");
    // 单条 API 装不下多条展开 ⇒ 指引改用 parse_insts
    let err = forge_codegen::riscv64::assemble("li x10, 0x1234").unwrap_err();
    assert!(
        err.contains("parse_insts"),
        "单条 API 应指引改用 parse_insts：{err}"
    );
}
