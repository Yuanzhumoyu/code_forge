//! v12 TargetMachine 集成冒烟测试（迭代 5）。
//! 验证集成层:TargetMachine 可组装、MachineInst 查询(uses/defs/reg_field/
//! set_reg_field/effects)、Encoder/Decoder/Disassembler/Assembler 组件可用。
use forge_codegen::machine::inst::MachineInst;
use forge_codegen::machine::target::TargetMachine as _;

#[test]
fn target_machine_assembles() {
    let tm = forge_codegen::x86_v12::TargetMachine::new();
    let _ = tm.isa_info();
    let _ = tm.reg_info();
    let _ = tm.abi();
    let _ = tm.encoder();
    let _ = tm.frame_lowering();
    assert!(tm.disassembler().is_some());
    assert!(tm.assembler().is_some());
    assert!(tm.decoder().is_some());
    // RegInfo 基本查询
    let ri = tm.reg_info();
    assert_eq!(ri.num_gp_regs(), 16, "x86 GPR 数");
    assert!(ri.num_fp_regs() >= 16, "x86 XMM 数");
}

#[test]
fn machine_inst_queries() {
    use forge_codegen::x86_v12::{Inst, Reg};
    use forge_ir::PhysReg;
    // movrr rax, rbx:op0=dest(out)=RAX, op1=src(in)=RBX, op2=opsize
    let inst = Inst::MovRRm {
        op0: 0,
        op1: 3,
        op2: 64,
    };
    let uses: smallvec::SmallVec<[u32; 4]> = inst.uses();
    let defs: smallvec::SmallVec<[u32; 2]> = inst.defs();
    let want_uses: smallvec::SmallVec<[u32; 4]> = smallvec::smallvec![3u32];
    let want_defs: smallvec::SmallVec<[u32; 2]> = smallvec::smallvec![0u32];
    assert_eq!(uses, want_uses, "movrr uses = src");
    assert_eq!(defs, want_defs, "movrr defs = dest");
    // reg_field 顺序 = 操作数序(Reg 位置序):op0 → 0, op1 → 1
    assert_eq!(inst.reg_field(0), 0);
    assert_eq!(inst.reg_field(1), 3);
    // set_reg_field 回填(regalloc 模拟)
    let mut m = inst.clone();
    m.set_reg_field(0, 8);
    m.set_reg_field(1, 9);
    match m {
        Inst::MovRRm { op0, op1, .. } => {
            assert_eq!(op0, 8);
            assert_eq!(op1, 9);
        }
        _ => panic!("expected MovRRm"),
    }
    // PhysReg：v12 物理编号 = 组内索引（x86 gpr64 0..15；xmm 组内 0..15）
    assert_eq!(Reg::RAX.to_index(), 0);
    assert_eq!(Reg::R8.to_index(), 8);
    assert_eq!(Reg::XMM0.to_index(), 0);
    assert_eq!(Reg::XMM15.to_index(), 15);
}

#[test]
fn encoder_decoder_via_tm() {
    use forge_codegen::x86_v12::Inst;
    let tm = forge_codegen::x86_v12::TargetMachine::new();
    let inst = Inst::MovRRm {
        op0: 0,
        op1: 3,
        op2: 64,
    };
    let mut sink = forge_codegen::CodeSink::default();
    tm.encoder()
        .encode(&inst, &forge_codegen::AllocResult::new(), &mut sink)
        .expect("encode via TM");
    assert_eq!(sink.bytes(), &[0x48, 0x8B, 0xC3], "movrr rax, rbx");
    // decoder
    let (dec, n) = tm
        .decoder()
        .expect("decoder")
        .decode(&[0x48, 0x8B, 0xC3])
        .expect("decode");
    assert_eq!(n, 3);
    assert_eq!(
        dec,
        Inst::MovRRm {
            op0: 0,
            op1: 3,
            op2: 64
        }
    );
    // disassembler / assembler
    let d = tm.disassembler().expect("disasm");
    assert_eq!(d.disassemble(&inst), "movrr RAX, RBX");
    let a = tm.assembler().expect("asm");
    let insts = a.parse_insts("movrr rax, rbx").expect("assemble");
    assert_eq!(
        insts,
        vec![Inst::MovRRm {
            op0: 0,
            op1: 3,
            op2: 64
        }]
    );
}

#[test]
fn riscv_target_machine_assembles() {
    let tm = forge_codegen::riscv64_v12::TargetMachine::new();
    let _ = tm.isa_info();
    let _ = tm.reg_info();
    let ri = tm.reg_info();
    assert_eq!(ri.num_gp_regs(), 32, "riscv GPR 数");
    assert_eq!(ri.num_fp_regs(), 32, "riscv FPR 数");
}

// ─────────────────── lowering（[[lowering]] 符号化操作数）───────────────────

#[test]
fn lowering_iadd_packet() {
    use forge_codegen::prelude::{LowerCtx, Opcode};
    use forge_codegen::x86_v12::Inst;

    let tm = forge_codegen::x86_v12::TargetMachine::new();
    let lowering = tm.lowering();

    // 构造 LowerCtx：最小调用面（alloc_xreg 用于缺省参数）
    let mut ctx = LowerCtx::default();
    let a = ctx.alloc_xreg(forge_ir::RegClass::GPR64);
    let b = ctx.alloc_xreg(forge_ir::RegClass::GPR64);
    let out = ctx.alloc_xreg(forge_ir::RegClass::GPR64);
    let pack = lowering
        .lower_inst(&Opcode::Iadd, &[a, b], &[out], &mut ctx)
        .expect("lower Iadd");
    // 两条微指令：MovRRm + AddRmR
    assert_eq!(pack.insts.len(), 2, "Iadd → 2 条微指令");
    assert!(matches!(pack.insts[0], Inst::MovRRm { .. }));
    assert!(matches!(pack.insts[1], Inst::AddRmR { .. }));
    // xreg_map：MovRRm(def=rd idx0, use=rs1 idx1)；AddRmR(use=rs1, def=rd)
    assert_eq!(pack.xreg_map.len(), 2);
    let m0 = &pack.xreg_map[0];
    assert_eq!(m0.len(), 2);
    assert_eq!(m0[0], (out, 0u8, true), "MovRRm field0 = out(def)");
    assert_eq!(m0[1], (a, 1u8, false), "MovRRm field1 = a(use)");
    let m1 = &pack.xreg_map[1];
    assert_eq!(m1[0], (a, 0u8, false), "AddRmR field0 = a(use)");
    assert_eq!(m1[1], (out, 1u8, true), "AddRmR field1 = out(def)");
    // 占位字段 = 0（regalloc 前）
    match &pack.insts[0] {
        Inst::MovRRm { op0, op1, op2 } => {
            assert_eq!(*op0, 0);
            assert_eq!(*op1, 0);
            assert_eq!(*op2, 64, "opsize 缺省 64");
        }
        _ => unreachable!(),
    }
}

#[test]
fn lowering_unknown_op_unsupported() {
    use forge_codegen::prelude::{LowerCtx, Opcode};
    let tm = forge_codegen::x86_v12::TargetMachine::new();
    let mut ctx = LowerCtx::default();
    let r = tm.lowering().lower_inst(&Opcode::Fadd, &[], &[], &mut ctx);
    assert!(r.is_err(), "未声明的 op 应 Unsupported");
}
