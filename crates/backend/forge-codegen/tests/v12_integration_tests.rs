//! v12 TargetMachine 集成冒烟测试（迭代 5）。
//! 验证集成层:TargetMachine 可组装、MachineInst 查询(uses/defs/reg_field/
//! set_reg_field/effects)、Encoder/Decoder/Disassembler/Assembler 组件可用。
use forge_codegen::machine::inst::MachineInst;
use forge_codegen::machine::target::TargetMachine as _;
use forge_codegen::x86_v12::Reg;
use forge_ir::PhysReg;

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
    // movrr rax, rbx:op0=dest(out)=RAX, op1=src(in)=RBX, op2=opsize
    let inst = Inst::MovRRm {
        dest: Reg::from_index(0, forge_ir::RegClass::GPR64),
        src: Reg::from_index(3, forge_ir::RegClass::GPR64),
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
        Inst::MovRRm { dest, src, .. } => {
            assert_eq!(dest.to_index(), 8);
            assert_eq!(src.to_index(), 9);
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
        dest: Reg::from_index(0, forge_ir::RegClass::GPR64),
        src: Reg::from_index(3, forge_ir::RegClass::GPR64),
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
            dest: Reg::from_index(0, forge_ir::RegClass::GPR64),
            src: Reg::from_index(3, forge_ir::RegClass::GPR64),
        }
    );
    // disassembler / assembler（v13：movrr 已合并为 mov）
    let d = tm.disassembler().expect("disasm");
    assert_eq!(d.disassemble(&inst), "mov RAX, RBX");
    let a = tm.assembler().expect("asm");
    // "mov rax, rbx"（64 位）→ 类型签名分发到 MOV_RM_R（0x89 MR 方向）
    let insts = a.parse_insts("mov rax, rbx").expect("assemble");
    assert_eq!(
        insts,
        vec![Inst::MovRmR {
            src: Reg::from_index(3, forge_ir::RegClass::GPR64),
            dest: Reg::from_index(0, forge_ir::RegClass::GPR64),
        }]
    );
    // "mov eax, ebx"（32 位）→ MOV_R_RM（0x8B 方向）
    let insts = a.parse_insts("mov eax, ebx").expect("assemble 32 位");
    assert_eq!(
        insts,
        vec![Inst::MovRRm {
            dest: Reg::from_index(0, forge_ir::RegClass::GPR(4)),
            src: Reg::from_index(3, forge_ir::RegClass::GPR(4)),
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
    // xreg_map：MovRRm(def=rd idx0, use=a idx1)；AddRmR(use=b idx0, def=rd idx1)
    // —— ADD_RM_R {1}, {out}：src: op0←{1}=b、dest: op1←{out}=rd
    assert_eq!(pack.xreg_map.len(), 2);
    let m0 = &pack.xreg_map[0];
    assert_eq!(m0.len(), 2);
    assert_eq!(m0[0], (out, 0u8, true), "MovRRm field0 = out(def)");
    assert_eq!(m0[1], (a, 1u8, false), "MovRRm field1 = a(use)");
    let m1 = &pack.xreg_map[1];
    assert_eq!(m1[0], (b, 0u8, false), "AddRmR field0 = b(use)");
    assert_eq!(m1[1], (out, 1u8, true), "AddRmR field1 = out(def)");
    // 占位字段 = from_index(0)（regalloc 前）
    match &pack.insts[0] {
        Inst::MovRRm { dest, src, .. } => {
            assert_eq!(dest.to_index(), 0);
            assert_eq!(src.to_index(), 0);
        }
        _ => unreachable!(),
    }
}

#[test]
fn lowering_unknown_op_unsupported() {
    use forge_codegen::prelude::{LowerCtx, Opcode};
    let tm = forge_codegen::x86_v12::TargetMachine::new();
    let mut ctx = LowerCtx::default();
    // GetElementPtr 尚未实现（Phase 5）——保持 Unsupported
    let r = tm
        .lowering()
        .lower_inst(&Opcode::GetElementPtr, &[], &[], &mut ctx);
    assert!(r.is_err(), "未声明的 op 应 Unsupported");
}

// ─────────────────── FrameLowering（[emit] prologue/epilogue + spill）───────────────────

#[test]
fn frame_prologue_bytes() {
    let tm = forge_codegen::x86_v12::TargetMachine::new();
    let fl = tm.frame_lowering();
    let rm = forge_codegen::AllocResult::new();
    let mut sink = forge_codegen::CodeSink::default();
    fl.emit_prologue(0, &rm, &mut sink).expect("prologue");
    // push rbp(55) + mov64rr rbp, rsp(48 89 e5) + push rbx(53) + push rdi(57)
    // + push rsi(56) + push r12(41 54) + push r13(41 55) + push r14(41 56)
    // + push r15(41 57) —— frame_size=0 时无 @frame_alloc
    let bytes = sink.bytes();
    assert_eq!(
        bytes,
        &[
            0x55, // push rbp
            0x48, 0x89, 0xE5, // mov64rr rbp, rsp
            0x53, // push rbx
            0x57, // push rdi
            0x56, // push rsi
            0x41, 0x54, // push r12
            0x41, 0x55, // push r13
            0x41, 0x56, // push r14
            0x41, 0x57, // push r15
        ],
        "prologue bytes (frame_size=0): {bytes:02x?}"
    );
}

#[test]
fn frame_prologue_with_alloc() {
    let tm = forge_codegen::x86_v12::TargetMachine::new();
    let fl = tm.frame_lowering();
    let rm = forge_codegen::AllocResult::new();
    let mut sink = forge_codegen::CodeSink::default();
    fl.emit_prologue(32, &rm, &mut sink).expect("prologue");
    let bytes = sink.bytes();
    // 末尾应有 @frame_alloc：sub rsp, 32 → 48 81 EC 20 00 00 00
    //（81 /5 = sub、reg=ext=5、rm=rsp=4 → modrm 0xEC；imm32）
    assert_eq!(
        &bytes[bytes.len() - 7..],
        &[0x48, 0x81, 0xEC, 0x20, 0x00, 0x00, 0x00],
        "frame_alloc (sub rsp, 32) tail: {:02x?}",
        &bytes[bytes.len() - 9..]
    );
}

#[test]
fn frame_epilogue_bytes() {
    let tm = forge_codegen::x86_v12::TargetMachine::new();
    let fl = tm.frame_lowering();
    let rm = forge_codegen::AllocResult::new();
    let mut sink = forge_codegen::CodeSink::default();
    fl.emit_epilogue(0, &rm, &mut sink).expect("epilogue");
    let bytes = sink.bytes();
    // mov64rr rsp, rbp(48 89 ec: reg=src: rbp=5、rm=dest: rsp=4 → 0xEC) +
    // sub rsp, 56(48 81 ec 38 00 00 00：rsp=rbp-56 到 callee-saved 区) +
    // pop r15..rbx + pop rbp(5d) + ret(c3)
    assert_eq!(&bytes[0..3], &[0x48, 0x89, 0xEC], "mov64rr rsp, rbp");
    assert_eq!(
        &bytes[3..10],
        &[0x48, 0x81, 0xEC, 0x38, 0x00, 0x00, 0x00],
        "sub rsp, 56"
    );
    assert_eq!(bytes[bytes.len() - 2], 0x5D, "pop rbp");
    assert_eq!(bytes[bytes.len() - 1], 0xC3, "ret");
}

#[test]
fn frame_spill_bytes() {
    let tm = forge_codegen::x86_v12::TargetMachine::new();
    let fl = tm.frame_lowering();
    let mut sink = forge_codegen::CodeSink::default();
    // spill store: mov64mr [rbp-8], rax → 48 89 45 F8
    fl.emit_spill_store(0, -8, 8, false, &mut sink)
        .expect("spill store");
    assert_eq!(
        sink.bytes(),
        &[0x48, 0x89, 0x45, 0xF8],
        "mov64mr [rbp-8], rax"
    );
    let mut sink2 = forge_codegen::CodeSink::default();
    // spill load: mov64rm rax, [rbp-8] → 48 8B 45 F8
    fl.emit_spill_load(0, -8, 8, false, &mut sink2)
        .expect("spill load");
    assert_eq!(
        sink2.bytes(),
        &[0x48, 0x8B, 0x45, 0xF8],
        "mov64rm rax, [rbp-8]"
    );
}

// ─────────────────── terminator lowering（Return/Jump）───────────────────

#[test]
fn terminator_return_packet() {
    use forge_codegen::prelude::{LowerCtx, Terminator, Value};
    use forge_codegen::x86_v12::Inst;
    use smallvec::smallvec;

    let tm = forge_codegen::x86_v12::TargetMachine::new();
    let mut ctx = LowerCtx::default();
    let ret_val = ctx.alloc_xreg(forge_ir::RegClass::GPR64);
    let mut vmap = std::collections::HashMap::new();
    vmap.insert(Value(7), ret_val);
    let term = Terminator::Return {
        values: smallvec![Value(7)],
        metadata: smallvec![],
    };
    let pack = tm
        .lowering()
        .lower_terminator(&term, &vmap, &std::collections::HashMap::new(), &mut ctx)
        .expect("lower Return");
    // MOV_RM8_R64(0x89: reg=src: val、rm=dest: RAX)——与 v11 一致：不生成 RET，
    // return block 经 epilogue_jump 跳到 epilogue 统一恢复。
    assert_eq!(
        pack.insts.len(),
        1,
        "Return → 1 条微指令（ret 在 epilogue）"
    );
    assert!(matches!(pack.insts[0], Inst::MovRm8R64 { .. }));
    // val → 字段 0(use)
    let m0 = &pack.xreg_map[0];
    assert_eq!(m0.len(), 1);
    assert_eq!(
        m0[0],
        (ret_val, 0u8, false),
        "MOV_RM8_R64 field0 = val(use)"
    );
}

#[test]
fn terminator_jump_packet() {
    use forge_codegen::prelude::{Block, LowerCtx, Terminator};
    use forge_codegen::x86_v12::Inst;
    use smallvec::smallvec;

    let tm = forge_codegen::x86_v12::TargetMachine::new();
    let mut ctx = LowerCtx::default();
    let term = Terminator::Jump {
        target: Block(3),
        args: smallvec![],
        metadata: smallvec![],
    };
    let pack = tm
        .lowering()
        .lower_terminator(
            &term,
            &std::collections::HashMap::new(),
            &std::collections::HashMap::new(),
            &mut ctx,
        )
        .expect("lower Jump");
    assert_eq!(pack.insts.len(), 1);
    match &pack.insts[0] {
        Inst::JmpRel32 { target } => assert_eq!(*target, 3, "jmp 目标块号"),
        other => panic!("expected JmpRel32, got {other:?}"),
    }
}
