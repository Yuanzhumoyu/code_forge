//! 全 ISA 汇编器集成测试：每个 ISA 的典型模板 parse + 类型消歧。
//!
//! 覆盖语法变体：#imm（aarch64）、offset(base)（riscv）、[rn,#imm]!（aarch64
//! 前索引）、b.{cond}（aarch64）、点号 mnemonic（riscv/wasm）、cc 展开、
//! 同 mnemonic 类型消歧（aarch64 mov 5 候选）。

use forge_codegen::machine::assembler::{AsmError, TargetAssembler};

// ── minimal_sd：prefix 寄存器 R0-R7 ──

#[test]
fn minimal_sd_mov_and_imm() {
    let asm = <forge_codegen::minimal_sd_test::minimal_sd::Assembler>::default_asm();
    // mov Reg,Reg vs mov Reg,Imm 语法层消歧
    let insts = asm.parse_insts("mov R0, R1\nmov R2, 5\n").expect("parse");
    assert_eq!(insts.len(), 2);
    let d = format!("{:?}", insts[0]);
    assert!(d.contains("dest") && d.contains("src"), "got {d}");
    let d2 = format!("{:?}", insts[1]);
    assert!(d2.contains("imm"), "mov Reg,Imm 应匹配立即数候选: {d2}");
}

#[test]
fn minimal_sd_fmov_bits_dot_mnemonic() {
    let asm = <forge_codegen::minimal_sd_test::minimal_sd::Assembler>::default_asm();
    let insts = asm.parse_insts("fmov.bits R0, R1\n").expect("parse");
    assert_eq!(insts.len(), 1);
}

// ── riscv64：named 寄存器 X0-X31 / F0-F31、offset(base) 内存 ──

#[test]
fn riscv_addi_and_flw() {
    let asm = <forge_codegen::riscv64::Assembler>::default_asm();
    // addi {dest}, {src1}, {imm}
    let insts = asm.parse_insts("addi X0, X1, 5\n").expect("parse");
    assert_eq!(insts.len(), 1);
    // flw {dest}, {offset}({base}) —— FPR dest + offset(base) 内存语法
    let insts = asm.parse_insts("flw F0, 8(X1)\n").expect("parse");
    assert_eq!(insts.len(), 1);
    let d = format!("{:?}", insts[0]);
    assert!(d.contains("F0") || d.contains("base"), "got {d}");
}

#[test]
fn riscv_fadd_dot_mnemonic() {
    let asm = <forge_codegen::riscv64::Assembler>::default_asm();
    let insts = asm.parse_insts("fadd.s F0, F1, F2\n").expect("parse");
    assert_eq!(insts.len(), 1);
}

// ── aarch64：named 寄存器、#{imm}、[rn,#imm]!、b.{cond}、mov 消歧组 ──

#[test]
fn aarch64_mov_type_disambiguation() {
    let asm = <forge_codegen::aarch64::Assembler>::default_asm();
    // mov 有 5 个候选（Ireg/GprReg 组合）——物理 GPR 对 → GprReg,GprReg 候选（权重最高）
    let insts = asm.parse_insts("mov X0, X1\n").expect("parse");
    assert_eq!(insts.len(), 1);
    let d = format!("{:?}", insts[0]);
    // 断言选择了 GprReg 变体（SD_MOV_RR），而非 Ireg 变体
    assert!(
        d.starts_with("SdMovRr") || d.contains("GprReg"),
        "应选 GprReg 候选: {d}"
    );
}

#[test]
fn aarch64_hash_imm_and_brk() {
    let asm = <forge_codegen::aarch64::Assembler>::default_asm();
    // mov {dest}, #{imm} —— HashImm 前缀
    let insts = asm.parse_insts("mov X0, #42\n").expect("parse");
    assert_eq!(insts.len(), 1);
    // brk #0 —— 字面数字
    let insts = asm.parse_insts("brk #0\n").expect("parse");
    assert_eq!(insts.len(), 1);
}

#[test]
fn aarch64_preindex_mem() {
    let asm = <forge_codegen::aarch64::Assembler>::default_asm();
    // stp {rt1}, {rt2}, [{rn}, #{imm}]! —— 前索引 + 写回
    let insts = asm.parse_insts("stp X0, X1, [SP, #16]!\n").expect("parse");
    assert_eq!(insts.len(), 1);
}

#[test]
fn aarch64_bcond_expansion() {
    let asm = <forge_codegen::aarch64::Assembler>::default_asm();
    // b.{cond} 展开：b.eq / b.ne
    let insts = asm
        .parse_insts("b.eq .L1\nb.ne .L2\n.L1:\nnop\n.L2:\nnop\n")
        .expect("parse");
    assert_eq!(insts.len(), 4);
    let d = format!("{:?}", insts[0]);
    assert!(d.contains("rel"), "b.eq 应带 rel: {d}");
}

#[test]
fn aarch64_undefined_label() {
    let asm = <forge_codegen::aarch64::Assembler>::default_asm();
    let err = asm.parse_insts("b.eq .Lmissing\n").unwrap_err();
    assert!(matches!(err, AsmError::UndefinedLabel(_)), "got {err:?}");
}

// ── wasm32：prefix 寄存器 L0-L31、;; 注释模板 ──

#[test]
fn wasm32_local_get_and_binop() {
    let asm = <forge_codegen::wasm32::Assembler>::default_asm();
    // local.get {dest}（模板 ;; 注释被裁剪）
    let insts = asm.parse_insts("local.get L0\n").expect("parse");
    assert_eq!(insts.len(), 1);
    // i32.clz {dest}, {src} —— 点号 mnemonic
    let insts = asm.parse_insts("i32.clz L0, L1\n").expect("parse");
    assert_eq!(insts.len(), 1);
    // 未定义指令 → 语法层拒绝
    let err = asm.parse_lines("i32.add L0, L1\n").unwrap_err();
    assert!(matches!(err, AsmError::ParseError(_)), "got {err:?}");
}

/// 生成代码的 `Assembler` 是零大小结构体——统一构造入口。
trait DefaultAsm: Sized {
    fn default_asm() -> Self;
}
impl DefaultAsm for forge_codegen::minimal_sd_test::minimal_sd::Assembler {
    fn default_asm() -> Self {
        forge_codegen::minimal_sd_test::minimal_sd::Assembler
    }
}
impl DefaultAsm for forge_codegen::riscv64::Assembler {
    fn default_asm() -> Self {
        forge_codegen::riscv64::Assembler
    }
}
impl DefaultAsm for forge_codegen::aarch64::Assembler {
    fn default_asm() -> Self {
        forge_codegen::aarch64::Assembler
    }
}
impl DefaultAsm for forge_codegen::wasm32::Assembler {
    fn default_asm() -> Self {
        forge_codegen::wasm32::Assembler
    }
}

#[test]
fn unknown_mnemonic_rejected() {
    let asm = forge_codegen::x86_64::Assembler;
    // 未知 mnemonic → 语法层拒绝（保留字集合外，且非 Ident 指令形态）
    let err = asm.parse_lines("frobnicate RAX, RBX\n").unwrap_err();
    assert!(matches!(err, AsmError::ParseError(_)), "got {err:?}");
}

#[test]
fn x86_round_trip_smoke() {
    use forge_codegen::machine::disasm::TargetDisassembler;
    let asm = forge_codegen::x86_64::Assembler;
    let dis = forge_codegen::x86_64::Disassembler;
    let insts = asm
        .parse_insts("mov_imm RAX, 0x10\nlea RAX, [RBX+RCX*4+8]\nlea_off RAX, [RBP+8]\n")
        .expect("parse");
    assert_eq!(insts.len(), 3);
    let d0 = dis.disassemble(&insts[0]);
    assert!(d0.contains("0x10") || d0.contains("16"), "got {d0}");
}
