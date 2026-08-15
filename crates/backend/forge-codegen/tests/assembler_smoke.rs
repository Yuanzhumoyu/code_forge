//! x86_64 汇编器冒烟测试：parse_insts / parse_lines / bind 消歧 / 标签解析。

use forge_codegen::assembler;
use forge_codegen::machine::assembler::{AsmError, TargetAssembler};
use forge_codegen::x86_64::{Assembler, Disassembler, Inst, Reg};

fn reg_index(r: &Reg) -> u32 {
    <Reg as forge_ir::PhysReg>::to_index(*r)
}

#[test]
fn parse_mov_and_add() {
    let asm = Assembler;
    let insts = asm
        .parse_insts("mov RAX, RBX\nadd RAX, 5\n")
        .expect("parse");
    assert_eq!(insts.len(), 2);
    match &insts[0] {
        Inst::MovRmR { dest, src, opsize } => {
            assert_eq!(reg_index(dest), 0, "RAX");
            assert_eq!(reg_index(src), 3, "RBX");
            assert_eq!(*opsize, 64);
        }
        other => panic!("expected MovRmR, got {other:?}"),
    }
    match &insts[1] {
        Inst::Add64RImm32 { dest, imm } => {
            assert_eq!(reg_index(dest), 0, "RAX");
            assert_eq!(*imm, 5);
        }
        other => panic!("expected Add64RImm32, got {other:?}"),
    }
}

#[test]
fn parse_hex_imm() {
    let asm = Assembler;
    let insts = asm.parse_insts("mov_imm RAX, 0xFF\n").expect("parse");
    match &insts[0] {
        Inst::MovRegImm64 { reg, imm } => {
            assert_eq!(reg_index(reg), 0);
            assert_eq!(*imm, 255);
        }
        other => panic!("expected MovRegImm64, got {other:?}"),
    }
}

#[test]
fn parse_lea_sib() {
    let asm = Assembler;
    let insts = asm.parse_insts("lea RAX, [RBX+RCX*4+8]\n").expect("parse");
    match &insts[0] {
        Inst::LeaR64Sib {
            dest,
            base,
            index,
            scale,
            disp,
        } => {
            assert_eq!(reg_index(dest), 0);
            assert_eq!(reg_index(base), 3, "RBX");
            assert_eq!(reg_index(index), 1, "RCX");
            assert_eq!(*scale, 4);
            assert_eq!(*disp, 8);
        }
        other => panic!("expected LeaR64Sib, got {other:?}"),
    }
}

#[test]
fn parse_setcc_expansion() {
    let asm = Assembler;
    // set{cond} 展开：sete/setne
    let insts = asm.parse_insts("sete AL\nsetne CL\n").expect("parse");
    match &insts[0] {
        Inst::SetccRm8 { dest, cond } => {
            assert_eq!(reg_index(dest), 0, "AL");
            assert_eq!(*cond, 0x84, "e");
        }
        other => panic!("expected SetccRm8, got {other:?}"),
    }
    match &insts[1] {
        Inst::SetccRm8 { cond, .. } => assert_eq!(*cond, 0x85, "ne"),
        other => panic!("expected SetccRm8, got {other:?}"),
    }
}

#[test]
fn parse_labels_two_pass() {
    let asm = Assembler;
    // 标签回填为**字节偏移**：je(6B: 0F 84+rel32) + mov(3B: 48 89 c3) → .L1 @ 9
    let src = "je .L1\nmov RAX, RBX\n.L1:\nnop\n";
    let insts = asm.parse_insts(src).expect("parse");
    assert_eq!(insts.len(), 3);
    match &insts[0] {
        Inst::JccRel32 { cond, rel } => {
            assert_eq!(*cond, 0x84, "e");
            assert_eq!(*rel, 9, "label .L1 字节偏移 6+3");
        }
        other => panic!("expected JccRel32, got {other:?}"),
    }
    match &insts[2] {
        Inst::Nop => {}
        other => panic!("expected Nop, got {other:?}"),
    }
}

#[test]
fn parse_jmp_rel32() {
    let asm = Assembler;
    // jmp(5B: E9+rel32) → .Lend @ 5
    let insts = asm.parse_insts("jmp .Lend\n.Lend:\n").expect("parse");
    match &insts[0] {
        Inst::JmpRel32 { rel } => assert_eq!(*rel, 5),
        other => panic!("expected JmpRel32, got {other:?}"),
    }
}

#[test]
fn numeric_rel_direct_offset() {
    let asm = Assembler;
    // 数字标签直接作为字节偏移（不进标签表）
    let insts = asm.parse_insts("jmp 8\n").expect("parse");
    match &insts[0] {
        Inst::JmpRel32 { rel } => assert_eq!(*rel, 8),
        other => panic!("expected JmpRel32, got {other:?}"),
    }
}

#[test]
fn type_disambiguation_reg_vs_imm() {
    let asm = Assembler;
    // 同 mnemonic 双候选：add Reg,Reg vs add Reg,Imm —— 按操作数类型语法层消歧
    let insts = asm
        .parse_insts("add RAX, RBX\nadd RAX, 5\n")
        .expect("parse");
    match &insts[0] {
        Inst::AddRmR {
            dest,
            src,
            opsize: _,
        } => {
            assert_eq!(reg_index(dest), 0, "RAX");
            assert_eq!(reg_index(src), 3, "RBX");
        }
        other => panic!("add reg,reg 应匹配寄存器候选，got {other:?}"),
    }
    match &insts[1] {
        Inst::Add64RImm32 { imm, .. } => assert_eq!(*imm, 5),
        other => panic!("add reg,imm 应匹配立即数候选，got {other:?}"),
    }
}

#[test]
fn undefined_label_error() {
    let asm = Assembler;
    let err = asm.parse_insts("jmp .Lmissing\n").unwrap_err();
    assert!(matches!(err, AsmError::UndefinedLabel(_)), "got {err:?}");
}

#[test]
fn type_mismatch_error() {
    let asm = Assembler;
    // mov 只接受寄存器操作数；`mov RAX, 5` 在语法层即被拒绝
    // （Reg 与 Imm 是不同非终结符——类型系统语法层消歧），
    // 或在 bind 层报 TypeMismatch。
    let err = asm.parse_insts("mov RAX, 5\n").unwrap_err();
    match &err {
        AsmError::ParseError(_) => {}
        AsmError::TypeMismatch { .. } => {}
        other => panic!("expected parse/type error, got {other:?}"),
    }
}

#[test]
fn parse_lines_keeps_pseudo_operands() {
    let asm = Assembler;
    // 指令包（lower/emit）语法：虚拟操作数 rd/rs1、临时寄存器 %t 在中间表示中保留
    let lines = asm
        .parse_lines("mov rd, rs1\nmov %t, rs2\n")
        .expect("parse_lines");
    assert_eq!(lines.len(), 2);
    let raw = lines[0].inst.as_ref().expect("inst");
    assert_eq!(raw.mnemonic, "mov");
    assert_eq!(raw.operands.len(), 2);
    assert_eq!(
        raw.operands[0],
        crate::assembler::OperandValue::Ident("rd".into())
    );
    assert_eq!(
        raw.operands[1],
        crate::assembler::OperandValue::Ident("rs1".into())
    );
    let raw2 = lines[1].inst.as_ref().expect("inst");
    assert_eq!(
        raw2.operands[0],
        crate::assembler::OperandValue::TmpReg("%t".into())
    );
    // parse_insts 对虚拟操作数报类型错误（无法物理化）
    let err = asm.parse_insts("mov rd, rs1\n").unwrap_err();
    assert!(matches!(err, AsmError::TypeMismatch { .. }), "got {err:?}");
}

#[test]
fn round_trip_disassemble() {
    use forge_codegen::machine::disasm::TargetDisassembler;
    let asm = Assembler;
    let dis = Disassembler;
    let insts = asm.parse_insts("mov RAX, RBX\n").expect("parse");
    let s = dis.disassemble(&insts[0]);
    assert!(
        s.to_lowercase().contains("mov"),
        "disasm `{s}` should contain mov"
    );
    assert!(s.contains("RAX"), "disasm `{s}` should contain RAX");
}

#[test]
fn mem_negative_offset() {
    let asm = Assembler;
    // [base-disp] 负偏移（spill 模板形态）：`mov64rm {dest}, {mem}`
    let insts = asm.parse_insts("mov64rm RAX, [RBP-8]\n").expect("parse");
    match &insts[0] {
        Inst::Mov64Rm { mem, .. } => {
            assert_eq!(mem.base, 5, "RBP");
            assert_eq!(mem.offset, -8);
        }
        other => panic!("expected Mov64Rm, got {other:?}"),
    }
}

#[test]
fn high_byte_register_alias() {
    let asm = Assembler;
    // gpr8h 视图寄存器（AH 物理编码 4）：named 变体直接绑定，不经过 from_index
    let insts = asm.parse_insts("sete AH\n").expect("parse");
    match &insts[0] {
        Inst::SetccRm8 { dest, .. } => {
            assert_eq!(reg_index(dest), 4, "AH 物理编号应为 4");
        }
        other => panic!("expected SetccRm8, got {other:?}"),
    }
}

#[test]
fn hex_imm_overflow_wraps() {
    let asm = Assembler;
    // 0xFFFFFFFFFFFFFFFF = -1（u64 位模式）
    let insts = asm
        .parse_insts("mov_imm RAX, 0xFFFFFFFFFFFFFFFF\n")
        .expect("parse");
    match &insts[0] {
        Inst::MovRegImm64 { imm, .. } => assert_eq!(*imm, -1),
        other => panic!("expected MovRegImm64, got {other:?}"),
    }
}
