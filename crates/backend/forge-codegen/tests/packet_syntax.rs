//! 指令包（lower/emit insts）语法支持测试。
//!
//! 验证 parse_lines（中间表示 API）对指令包语法的完整支持：
//! 虚拟操作数（rd/rs1/func…）、临时寄存器 %t、VReg(N)、{const N}、
//! @ 伪指令、MemRef `[rs1]` 形式。

use forge_codegen::assembler::OperandValue;
use forge_codegen::machine::assembler::TargetAssembler;
use forge_codegen::x86_64::Assembler;

fn ops_of(asm: &Assembler, src: &str, line: usize) -> Vec<OperandValue> {
    let lines = asm.parse_lines(src).expect("parse_lines");
    lines[line]
        .inst
        .as_ref()
        .unwrap_or_else(|| panic!("line {line} has no inst"))
        .operands
        .clone()
}

#[test]
fn pseudo_operands_ident() {
    let asm = Assembler;
    // 虚拟操作数 rd/rs1/rs2 作为 Ident 保留（mov 的 Reg,Reg 模板）
    let ops = ops_of(&asm, "mov rd, rs1\n", 0);
    assert_eq!(
        ops,
        vec![
            OperandValue::Ident("rd".into()),
            OperandValue::Ident("rs1".into()),
        ]
    );
}

#[test]
fn tmp_reg_percent() {
    let asm = Assembler;
    let ops = ops_of(&asm, "mov %t0, rs2\n", 0);
    assert_eq!(ops[0], OperandValue::TmpReg("%t0".into()));
}

#[test]
fn vreg_literal() {
    let asm = Assembler;
    // VReg(N) 字面量（lower 指令包的过渡语法）
    let ops = ops_of(&asm, "mov VReg(96), rs1\n", 0);
    assert_eq!(ops[0], OperandValue::VReg(96));
    let ops2 = ops_of(&asm, "mov vreg(3), rs1\n", 0);
    assert_eq!(ops2[0], OperandValue::VReg(3));
}

#[test]
fn const_pool_inline() {
    let asm = Assembler;
    // {const N} 常量池内联（imm 字段）
    let ops = ops_of(&asm, "mov_imm RAX, {const 42}\n", 0);
    assert_eq!(ops[1], OperandValue::Const(42));
    // bind 层也可用（parse_insts：Const 视作 Imm）
    let insts = asm.parse_insts("mov_imm RAX, {const 42}\n").expect("parse");
    match &insts[0] {
        forge_codegen::x86_64::Inst::MovRegImm64 { imm, .. } => assert_eq!(*imm, 42),
        other => panic!("expected MovRegImm64, got {other:?}"),
    }
}

#[test]
fn at_label_pseudo_inst() {
    let asm = Assembler;
    // @pop_callee 等 emit 宏作为伪指令行（inst_idx = usize::MAX 哨兵）
    let lines = asm
        .parse_lines("@pop_callee\nmov RAX, RBX\n")
        .expect("parse_lines");
    let raw = lines[0].inst.as_ref().expect("inst");
    assert_eq!(raw.inst_idx, usize::MAX);
    assert_eq!(raw.mnemonic, "@pop_callee");
    assert!(raw.operands.is_empty());
    assert!(lines[1].inst.is_some());
    // parse_insts 对伪指令报错（无独立编码）
    let err = asm.parse_insts("@pop_callee\n").unwrap_err();
    assert!(err.to_string().contains("pseudo-op"), "got {err:?}");
}

#[test]
fn mem_ref_virtual_base() {
    let asm = Assembler;
    // MemRef 字段 `[rs1]`：base 是虚拟操作数，中间表示保留
    let ops = ops_of(&asm, "mov64rm rd, [rs1]\n", 0);
    assert_eq!(
        ops[1],
        OperandValue::Mem {
            base: Some("rs1".into()),
            disp: 0
        }
    );
    // 物理基址可绑定
    let insts = asm.parse_insts("mov64rm RAX, [RBP+8]\n").expect("parse");
    match &insts[0] {
        forge_codegen::x86_64::Inst::Mov64Rm { mem, .. } => {
            assert_eq!(mem.base, 5, "RBP");
            assert_eq!(mem.offset, 8);
        }
        other => panic!("expected Mov64Rm, got {other:?}"),
    }
}

#[test]
fn call_indirect_pseudo_operand() {
    let asm = Assembler;
    // x86 间接调用语法 `call *{target}`：物理寄存器可绑定
    let insts = asm.parse_insts("call *RAX\n").expect("parse");
    assert_eq!(insts.len(), 1);
    // 指令包 `CALL_RM rs1`（虚拟操作数）→ 中间表示保留
    let lines = asm.parse_lines("call *rs1\n").expect("parse_lines");
    let raw = lines[0].inst.as_ref().expect("inst");
    assert_eq!(raw.mnemonic, "call");
    assert_eq!(raw.operands[0], OperandValue::Ident("rs1".into()));
    // 直接 `call func`（无 `*`）是 CALL_REL32（target=i32）语法，符号不匹配 → 报错
    let err = asm.parse_lines("call func\n").unwrap_err();
    assert!(
        matches!(
            err,
            forge_codegen::machine::assembler::AsmError::ParseError(_)
        ),
        "got {err:?}"
    );
}

#[test]
fn icmp_packet_style() {
    let asm = Assembler;
    // 典型 x86 lowering 指令包行（物理 + 虚拟混合），中间表示完整保留
    let src = "xor RDX, RDX\nmov %t, rs2\nmovsxd RAX, rs1\ncqo\nidiv %t\nmov rd, RAX\n";
    let lines = asm.parse_lines(src).expect("parse_lines");
    assert_eq!(lines.len(), 6);
    assert_eq!(
        lines[1].inst.as_ref().unwrap().operands[0],
        OperandValue::TmpReg("%t".into())
    );
    assert_eq!(
        lines[1].inst.as_ref().unwrap().operands[1],
        OperandValue::Ident("rs2".into())
    );
    assert_eq!(lines[2].inst.as_ref().unwrap().mnemonic, "movsxd");
}
