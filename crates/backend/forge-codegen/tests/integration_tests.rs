//! ISA-DSL TargetMachine 集成冒烟测试（迭代 5）。
//! 验证集成层:TargetMachine 可组装、MachineInst 查询(uses/defs/reg_field/
//! set_reg_field/effects)、Encoder/Decoder/Disassembler/Assembler 组件可用。
use forge_codegen::machine::inst::MachineInst;
use forge_codegen::machine::lowering::OperandBind;
use forge_codegen::machine::target::TargetMachine as _;
use forge_codegen::x86::Reg;
use forge_ir::PhysReg;

#[test]
fn target_machine_assembles() {
    let tm = forge_codegen::x86::TargetMachine::new();
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
    // 可编码 GPR 数 = 32（APX 的 r16..r31 也能编）；**分配池**只有前 16 个
    // （`[reg.gpr64].alloc_count = 16`，见 `abi_target_real.rs`）。
    assert_eq!(ri.num_gp_regs(), 32, "x86 可编码 GPR 数（含 APX EGPR）");
    assert!(ri.num_fp_regs() >= 16, "x86 XMM 数");
}

#[test]
fn machine_inst_queries() {
    use forge_codegen::x86::{Inst, Reg};
    // movrr rax, rbx:op0=dst(out)=RAX, op1=src(in)=RBX, op2=opsize
    let inst = Inst::MovRRm {
        dst: Reg::from_index(0, forge_ir::RegClass::GPR64),
        src: Reg::from_index(3, forge_ir::RegClass::GPR64),
    };
    let uses: smallvec::SmallVec<[u32; 4]> = inst.uses();
    let defs: smallvec::SmallVec<[u32; 2]> = inst.defs();
    let want_uses: smallvec::SmallVec<[u32; 4]> = smallvec::smallvec![3u32];
    let want_defs: smallvec::SmallVec<[u32; 2]> = smallvec::smallvec![0u32];
    assert_eq!(uses, want_uses, "movrr uses = src");
    assert_eq!(defs, want_defs, "movrr defs = dst");
    // reg_field 顺序 = 操作数序(Reg 位置序):op0 → 0, op1 → 1
    assert_eq!(inst.reg_field(0), 0);
    assert_eq!(inst.reg_field(1), 3);
    // set_reg_field 回填(regalloc 模拟)：class 携带分配宽度
    let mut m = inst.clone();
    m.set_reg_field(0, 8, forge_ir::RegClass::GPR64);
    m.set_reg_field(1, 9, forge_ir::RegClass::GPR64);
    match m {
        Inst::MovRRm { dst, src, .. } => {
            assert_eq!(dst.to_index(), 8);
            assert_eq!(src.to_index(), 9);
        }
        _ => panic!("expected MovRRm"),
    }
    // PhysReg：谱内物理编号 = 组内索引（x86 gpr512 0..15；xmm 组内 0..15）
    assert_eq!(Reg::RAX.to_index(), 0);
    assert_eq!(Reg::R8.to_index(), 8);
    assert_eq!(Reg::XMM0.to_index(), 0);
    assert_eq!(Reg::XMM15.to_index(), 15);
}

#[test]
fn encoder_decoder_via_tm() {
    use forge_codegen::x86::Inst;
    let tm = forge_codegen::x86::TargetMachine::new();
    let inst = Inst::MovRRm {
        dst: Reg::from_index(0, forge_ir::RegClass::GPR64),
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
            dst: Reg::from_index(0, forge_ir::RegClass::GPR64),
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
            dst: Reg::from_index(0, forge_ir::RegClass::GPR64),
        }]
    );
    // "mov eax, ebx"（32 位）→ **MR 形态**（`89`）：与 64 位那条同一个方向（也与上游
    // LLVM 一致）。签名更具体 ⇒ 胜出；`8B` 方向那条（`MOV_R_RM`）仍在谱里**认领解码**。
    let insts = a.parse_insts("mov eax, ebx").expect("assemble 32 位");
    assert_eq!(
        insts,
        vec![Inst::MovRmR24 {
            src: Reg::from_index(3, forge_ir::RegClass::GPR(32)),
            dst: Reg::from_index(0, forge_ir::RegClass::GPR(32)),
        }]
    );
    // 8B 形态（别的汇编器会编出来）必须解得回来：两条合法编码收敛到同一段文本。
    let (back, n8) = tm
        .decoder()
        .expect("decoder")
        .decode(&[0x8B, 0xC3])
        .expect("decode 8B 形态");
    assert_eq!(n8, 2);
    assert!(
        matches!(back, Inst::MovRRm { .. }),
        "8B 形态应解成 MOV_R_RM：{back:?}"
    );
}

#[test]
fn riscv_target_machine_assembles() {
    let tm = forge_codegen::riscv64::TargetMachine::new();
    let _ = tm.isa_info();
    let _ = tm.reg_info();
    let ri = tm.reg_info();
    assert_eq!(ri.num_gp_regs(), 32, "riscv GPR 数");
    assert_eq!(ri.num_fp_regs(), 32, "riscv FPR 数");
}

// ─────────────────── lowering（[[lower.<op>]] 符号化操作数）───────────────────

#[test]
fn lowering_iadd_packet() {
    use forge_codegen::prelude::{LowerCtx, Opcode};
    use forge_codegen::x86::Inst;

    let tm = forge_codegen::x86::TargetMachine::new();
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
    // inst_ops：MovRRm(def=rd idx0, use=a idx1)；AddRmR(use=b idx0, def=rd idx1)
    // —— ADD_RM_R {1}, {out}：src: op0←{1}=b、dest: op1←{out}=rd
    assert_eq!(pack.inst_ops.len(), 2);
    let m0 = &pack.inst_ops[0];
    assert_eq!(m0.len(), 2);
    assert_eq!(m0[0], OperandBind::def(out, 0), "MovRRm field0 = out(def)");
    assert_eq!(m0[1], OperandBind::use_(a, 1), "MovRRm field1 = a(use)");
    let m1 = &pack.inst_ops[1];
    assert_eq!(m1[0], OperandBind::use_(b, 0), "AddRmR field0 = b(use)");
    assert_eq!(m1[1], OperandBind::def(out, 1), "AddRmR field1 = out(def)");
    // 占位字段 = from_index(0)（regalloc 前）
    match &pack.insts[0] {
        Inst::MovRRm { dst, src, .. } => {
            assert_eq!(dst.to_index(), 0);
            assert_eq!(src.to_index(), 0);
        }
        _ => unreachable!(),
    }
}

#[test]
fn lowering_unknown_op_unsupported() {
    use forge_codegen::prelude::{LowerCtx, Opcode};
    let tm = forge_codegen::x86::TargetMachine::new();
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
    let tm = forge_codegen::x86::TargetMachine::new();
    let fl = tm.frame_lowering();
    let rm = forge_codegen::AllocResult::new();
    let mut sink = forge_codegen::CodeSink::default();
    fl.emit_prologue(0, &rm, &mut sink).expect("prologue");
    // push rbp(55) + mov64rr rbp, rsp(48 89 e5)。**v20 A6 起不再静态发谱面表**：
    // 逐个 `push` 的列表来自 `AllocResult::callee_saved_to_save`（约定数据），
    // 这里用的是**空** AllocResult（`AllocResult::new()`）⇒ 没有要保存的寄存器，
    // 于是只有建帧的两条。真正的管线还会把这几个"没推的槽"补进 `frame_alloc`
    // 的字节数（见 `pipeline/emission.rs` 的 `cs_skipped`），所以直接调
    // `emit_prologue(frame_size, …)` 时看到的 `sub rsp` 仍是 `frame_size`。
    let bytes = sink.bytes();
    assert_eq!(
        bytes,
        &[
            0x55, // push rbp
            0x48, 0x89, 0xE5, // mov64rr rbp, rsp
        ],
        "prologue bytes (frame_size=0, 空 callee_saved_to_save): {bytes:02x?}"
    );
}

#[test]
fn frame_prologue_with_alloc() {
    let tm = forge_codegen::x86::TargetMachine::new();
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
    let tm = forge_codegen::x86::TargetMachine::new();
    let fl = tm.frame_lowering();
    let rm = forge_codegen::AllocResult::new();
    let mut sink = forge_codegen::CodeSink::default();
    fl.emit_epilogue(0, &rm, &mut sink).expect("epilogue");
    let bytes = sink.bytes();
    // mov64rr rsp, rbp(48 89 ec: reg=src: rbp=5、rm=dst: rsp=4 → 0xEC) +
    // **sub rsp, 实际保存数×槽**（v20 A6：长度取 `callee_saved_to_save.len()` 的
    // 运行时值；空 AllocResult ⇒ 0）+
    // （有要恢复的寄存器时）逆序 pop + pop rbp(5d) + ret(c3)
    assert_eq!(&bytes[0..3], &[0x48, 0x89, 0xEC], "mov64rr rsp, rbp");
    assert_eq!(
        &bytes[3..10],
        &[0x48, 0x81, 0xEC, 0x00, 0x00, 0x00, 0x00],
        "sub rsp, 0（空 callee_saved_to_save）"
    );
    assert_eq!(bytes[bytes.len() - 2], 0x5D, "pop rbp");
    assert_eq!(bytes[bytes.len() - 1], 0xC3, "ret");
}

#[test]
fn frame_spill_bytes() {
    let tm = forge_codegen::x86::TargetMachine::new();
    let fl = tm.frame_lowering();
    let mut sink = forge_codegen::CodeSink::default();
    // spill store: mov [rbp-8], rax → 48 89 45 F8
    fl.emit_spill_store(0, -8, 8, forge_ir::RegClass::GPR64, &mut sink)
        .expect("spill store");
    assert_eq!(sink.bytes(), &[0x48, 0x89, 0x45, 0xF8], "mov [rbp-8], rax");
    let mut sink2 = forge_codegen::CodeSink::default();
    // spill load: mov rax, [rbp-8] → 48 8B 45 F8
    fl.emit_spill_load(0, -8, 8, forge_ir::RegClass::GPR64, &mut sink2)
        .expect("spill load");
    assert_eq!(sink2.bytes(), &[0x48, 0x8B, 0x45, 0xF8], "mov rax, [rbp-8]");
}

// ─────────────────── terminator lowering（Return/Jump）───────────────────

#[test]
fn terminator_return_packet() {
    use forge_codegen::ir::{FunctionBuilder, FunctionSignature, TypeContext, TypeId};
    use forge_codegen::prelude::LowerCtx;
    use forge_codegen::x86::Inst;

    let tm = forge_codegen::x86::TargetMachine::new();
    let mut ctx = LowerCtx::default();
    // 直连 lowering（不经过编译管线）时，**约定级数据要自己填**：v20 A5-3 起
    // 返回槽不再来自谱里的 `[abi].ret_regs`（已删除），而是 plan 或这份约定级返回槽。
    // 管线（`compiler.rs`）在真实编译路径上会填好；这里按 x86 win64 的口径填 RAX（类内 0 号）。
    ctx.conv.ret_gpr = Some((
        <forge_codegen::x86::Reg as forge_ir::PhysReg>::from_index(
            0,
            tm.reg_info().value_gpr_class(),
        )
        .to_index(),
        tm.reg_info().value_gpr_class(),
    ));
    let ret_val = ctx.alloc_xreg(forge_ir::RegClass::GPR64);

    // 终结符是一条指令（S4 主体）：经公开写入口建好 IR，再交给 lowering。
    let mut fb = FunctionBuilder::new(
        "t",
        TypeContext::new(),
        FunctionSignature::new(&[], &[TypeId::I32]),
    );
    let (entry, _) = fb.create_entry_block();
    let v = fb.constant(7i32);
    fb.ret(&[v]);
    let func = fb.finish().expect("build");
    let ret_value = func.dfg.term_return_values(entry).expect("ret 投影")[0];
    let mut vmap = forge_ir::entity::map::SecondaryMap::new();
    vmap.insert(ret_value, ret_val);

    let pack = tm
        .lowering()
        .lower_terminator(
            &func.dfg,
            entry,
            &vmap,
            &forge_ir::entity::map::SecondaryMap::new(),
            &mut ctx,
        )
        .expect("lower Return");
    // MOV_RM8_R64(0x89: reg=src: val、rm=dst: RAX)——与 v11 一致：不生成 RET，
    // return block 经 epilogue_jump 跳到 epilogue 统一恢复。
    assert_eq!(
        pack.insts.len(),
        1,
        "Return → 1 条微指令（ret 在 epilogue）"
    );
    assert!(matches!(pack.insts[0], Inst::MovRm8R64 { .. }));
    // val → 字段 0(use)
    let m0 = &pack.inst_ops[0];
    assert_eq!(m0.len(), 1);
    assert_eq!(
        m0[0],
        OperandBind::use_(ret_val, 0),
        "MOV_RM8_R64 field0 = val(use)"
    );
}

#[test]
fn terminator_jump_packet() {
    use forge_codegen::ir::{FunctionBuilder, FunctionSignature, TypeContext, TypeId};
    use forge_codegen::prelude::LowerCtx;
    use forge_codegen::x86::Inst;

    let tm = forge_codegen::x86::TargetMachine::new();
    let mut ctx = LowerCtx::default();
    let mut fb = FunctionBuilder::new(
        "t",
        TypeContext::new(),
        FunctionSignature::new(&[], &[TypeId::I32]),
    );
    let b0 = fb.create_block();
    let b1 = fb.create_block();
    fb.switch_to_block(b0);
    fb.jump(b1, &[]);
    fb.switch_to_block(b1);
    let v = fb.constant(0i32);
    fb.ret(&[v]);
    let func = fb.finish().expect("build");

    let pack = tm
        .lowering()
        .lower_terminator(
            &func.dfg,
            b0,
            &forge_ir::entity::map::SecondaryMap::new(),
            &forge_ir::entity::map::SecondaryMap::new(),
            &mut ctx,
        )
        .expect("lower Jump");
    assert_eq!(pack.insts.len(), 1);
    match &pack.insts[0] {
        Inst::JmpRel32 { target } => assert_eq!(*target, 1, "jmp 目标块号"),
        other => panic!("expected JmpRel32, got {other:?}"),
    }
}

// ─────────────── NEON 向量 lowering（v27 M6-134）───────────────

/// arm64 的 V128 f32x4 加/减/乘必须落到 FADD4S/FSUB4S/FMUL4S（Q 视图 fprq），
/// 且编码与 clang oracle 一致：`fadd v0.4s, v0.4s, v0.4s` = 0x4E20D400。
///
/// 钉住三件事：① `[[lower.V*]]` 的 rd=128+elem=1 谓词命中；② 操作数是 fprq
/// （fpr128）而非 D 视图；③ 编码器的 match 字段来自真实 oracle 解码。
#[test]
fn lowering_arm64_neon_f32x4() {
    use forge_codegen::arm64::Inst;
    use forge_codegen::prelude::{LowerCtx, Opcode};
    use forge_ir::ir::types::TypeContext;

    let tc = TypeContext::new();
    let v128 = tc.vector_ty(forge_ir::TypeId::F32, 4);
    let tm = forge_codegen::arm64::TargetMachine::new();
    let mut ctx = LowerCtx::default();
    ctx.type_store = Some(tc.borrow());

    let mk = |ctx: &mut LowerCtx| {
        let x = ctx.alloc_xreg(forge_ir::RegClass::VEC(128));
        ctx.set_value_desc(x, v128);
        x
    };
    let a = mk(&mut ctx);
    let b = mk(&mut ctx);
    let out = mk(&mut ctx);

    for (op, want_word) in [
        (Opcode::Vadd, 0x4E20_D400u32),
        (Opcode::Vsub, 0x4EA0_D400u32),
        (Opcode::Vmul, 0x6E20_DC00u32),
    ] {
        let pack = tm
            .lowering()
            .lower_inst(&op, &[a, b], &[out], &mut ctx)
            .unwrap_or_else(|e| panic!("{op:?} 应可降级为 NEON 指令：{e:?}"));
        assert_eq!(pack.insts.len(), 1, "{op:?} → 单条微指令");
        match &pack.insts[0] {
            Inst::Fadd4s { .. } => assert_eq!(op, Opcode::Vadd),
            Inst::Fsub4s { .. } => assert_eq!(op, Opcode::Vsub),
            Inst::Fmul4s { .. } => assert_eq!(op, Opcode::Vmul),
            other => panic!("{op:?} 应落到 NEON 浮点指令，实际 {other:?}"),
        }
        let mut sink = forge_codegen::CodeSink::default();
        tm.encoder()
            .encode(
                &pack.insts[0],
                &forge_codegen::AllocResult::new(),
                &mut sink,
            )
            .expect("编码 NEON 指令");
        assert_eq!(
            sink.bytes(),
            &want_word.to_le_bytes(),
            "{op:?} 编码须与 clang oracle 一致（v0.4s, v0.4s, v0.4s）"
        );
    }
}

/// **NEON 扩展守卫**（v27 M6-135）：i32x4 的加/减/乘与 splat。
/// 编码全部来自 clang oracle（全 v0 操作数形态）：
/// add.4s=0x4EA08400、sub.4s=0x6EA08400、mul.4s=0x4EA09C00、
/// dup.4s,w0=0x4E040C00、dup.2d,x0=0x4E080C00。
#[test]
fn lowering_arm64_neon_i32x4_splat() {
    use forge_codegen::prelude::{LowerCtx, Opcode};
    use forge_ir::ir::types::TypeContext;

    let tc = TypeContext::new();
    let i32x4 = tc.vector_ty(forge_ir::TypeId::I32, 4);
    let f32x4 = tc.vector_ty(forge_ir::TypeId::F32, 4);
    let f64x2 = tc.vector_ty(forge_ir::TypeId::F64, 2);
    let i64x2 = tc.vector_ty(forge_ir::TypeId::I64, 2);

    let tm = forge_codegen::arm64::TargetMachine::new();
    let mut ctx = LowerCtx::default();
    ctx.type_store = Some(tc.borrow());

    for (op, want, word) in [
        (Opcode::Vadd, "Vadd4s", 0x4EA0_8400u32),
        (Opcode::Vsub, "Isub4sQ", 0x6EA0_8400u32),
        (Opcode::Vmul, "Imul4sQ", 0x4EA0_9C00u32),
    ] {
        let a = ctx.alloc_xreg(forge_ir::RegClass::VEC(128));
        let b = ctx.alloc_xreg(forge_ir::RegClass::VEC(128));
        let out = ctx.alloc_xreg(forge_ir::RegClass::VEC(128));
        for x in [a, b, out] {
            ctx.set_value_desc(x, i32x4);
        }
        let pack = tm
            .lowering()
            .lower_inst(&op, &[a, b], &[out], &mut ctx)
            .unwrap_or_else(|e| panic!("{op:?} 应可降级：{e:?}"));
        let got = format!("{:?}", pack.insts[0]);
        assert!(got.starts_with(want), "{op:?} 应为 {want}，实际 {got}");
        let mut sink = forge_codegen::CodeSink::default();
        tm.encoder()
            .encode(
                &pack.insts[0],
                &forge_codegen::AllocResult::new(),
                &mut sink,
            )
            .expect("编码");
        assert_eq!(sink.bytes(), &word.to_le_bytes(), "{op:?} 编码不符 oracle");
    }

    for (op, want, word) in [
        (Opcode::Vadd, "Fadd2d", 0x4E60_D400u32),
        (Opcode::Vsub, "Fsub2d", 0x4EE0_D400u32),
        (Opcode::Vmul, "Fmul2d", 0x6E60_DC00u32),
    ] {
        let a = ctx.alloc_xreg(forge_ir::RegClass::VEC(128));
        let b = ctx.alloc_xreg(forge_ir::RegClass::VEC(128));
        let out = ctx.alloc_xreg(forge_ir::RegClass::VEC(128));
        for x in [a, b, out] {
            ctx.set_value_desc(x, f64x2);
        }
        let pack = tm
            .lowering()
            .lower_inst(&op, &[a, b], &[out], &mut ctx)
            .unwrap_or_else(|e| panic!("f64x2 {op:?} 应可降级：{e:?}"));
        let got = format!("{:?}", pack.insts[0]);
        assert!(
            got.starts_with(want),
            "f64x2 {op:?} 应为 {want}，实际 {got}"
        );
        let mut sink = forge_codegen::CodeSink::default();
        tm.encoder()
            .encode(
                &pack.insts[0],
                &forge_codegen::AllocResult::new(),
                &mut sink,
            )
            .expect("编码 f64x2");
        assert_eq!(
            sink.bytes(),
            &word.to_le_bytes(),
            "f64x2 {op:?} 编码不符 oracle"
        );
    }
    // splat 的**源寄存器堆**按值类分派（v27 KD-05 修正）：本谱把标量 f32/f64 映射到
    // FPR 值类（`reg_class_for`），所以浮点 splat 先 `fmov w/x, s/d` 把位型搬进 GPR，
    // 再 DUP (general)；i32/i64 本来就在 GPR ⇒ 直接 DUP (general)。
    // 旧规则把浮点源也当 GPR 送进 `dup …, wN` ⇒ 端到端 splat 静默全零
    // （见 isa/arm64.toml 的 inst.DUP4S_W 注释）。
    // oracle：fmov w0,s0=0x1E260000、fmov x0,d0=0x9E660000、
    //         dup.4s v0,w0=0x4E040C00、dup.2d v0,x0=0x4E080C00。
    for (elem, vec_ty, src_cls, want, words) in [
        (
            forge_ir::TypeId::F32,
            f32x4,
            forge_ir::RegClass::FPR(32),
            "FmovWFromS",
            &[0x1E26_0000u32, 0x4E04_0C00u32][..],
        ),
        (
            forge_ir::TypeId::I32,
            i32x4,
            forge_ir::RegClass::GPR(32),
            "Dup4sW",
            &[0x4E04_0C00u32][..],
        ),
        (
            forge_ir::TypeId::F64,
            f64x2,
            forge_ir::RegClass::FPR(64),
            "FmovXFromD",
            &[0x9E66_0000u32, 0x4E08_0C00u32][..],
        ),
        (
            forge_ir::TypeId::I64,
            i64x2,
            forge_ir::RegClass::GPR(64),
            "Dup2dX",
            &[0x4E08_0C00u32][..],
        ),
    ] {
        let scalar = ctx.alloc_xreg(src_cls);
        ctx.set_value_desc(scalar, elem);
        let out = ctx.alloc_xreg(forge_ir::RegClass::VEC(128));
        ctx.set_value_desc(out, vec_ty);
        let pack = tm
            .lowering()
            .lower_inst(&Opcode::Vbroadcast, &[scalar], &[out], &mut ctx)
            .unwrap_or_else(|e| panic!("Vbroadcast {elem:?} 应可降级：{e:?}"));
        assert_eq!(
            pack.insts.len(),
            words.len(),
            "splat {elem:?} 应为 {} 条指令",
            words.len()
        );
        let got = format!("{:?}", pack.insts[0]);
        assert!(got.starts_with(want), "splat 应为 {want}，实际 {got}");
        for (i, word) in words.iter().enumerate() {
            let mut sink = forge_codegen::CodeSink::default();
            tm.encoder()
                .encode(
                    &pack.insts[i],
                    &forge_codegen::AllocResult::new(),
                    &mut sink,
                )
                .expect("编码 splat");
            assert_eq!(
                sink.bytes(),
                &word.to_le_bytes(),
                "splat {elem:?} 第 {i} 条编码不符 oracle"
            );
        }
    }
}

/// **NEON div/neg/abs 守卫**（v27 M6-137）。编码全 v 形式（oracle 见 isa/arm64.toml）：
/// fdiv.4s=0x6E20FC00、fdiv.2d=0x6E60FC00、fneg.4s=0x6EA0F800、fneg.2d=0x6EE0F800、
/// fabs.4s=0x4EA0F800、fabs.2d=0x4EE0F800、neg.4s=0x6EA0B800、abs.4s=0x4EA0B800。
#[test]
fn lowering_arm64_neon_div_neg_abs() {
    use forge_codegen::prelude::{LowerCtx, Opcode};
    use forge_ir::ir::types::TypeContext;

    let tc = TypeContext::new();
    let f32x4 = tc.vector_ty(forge_ir::TypeId::F32, 4);
    let f64x2 = tc.vector_ty(forge_ir::TypeId::F64, 2);
    let i32x4 = tc.vector_ty(forge_ir::TypeId::I32, 4);
    let tm = forge_codegen::arm64::TargetMachine::new();
    let mut ctx = LowerCtx::default();
    ctx.type_store = Some(tc.borrow());

    // (op, ty, 操作数个数, 期望变体, 编码)
    for (op, ty, arity, want, word) in [
        (Opcode::Vdiv, f32x4, 2, "Fdiv4s", 0x6E20_FC00u32),
        (Opcode::Vdiv, f64x2, 2, "Fdiv2d", 0x6E60_FC00u32),
        (Opcode::Vneg, f32x4, 1, "Fneg4s", 0x6EA0_F800u32),
        (Opcode::Vneg, f64x2, 1, "Fneg2d", 0x6EE0_F800u32),
        (Opcode::Vneg, i32x4, 1, "Ineg4s", 0x6EA0_B800u32),
        (Opcode::Vabs, f32x4, 1, "Fabs4s", 0x4EA0_F800u32),
        (Opcode::Vabs, f64x2, 1, "Fabs2d", 0x4EE0_F800u32),
        (Opcode::Vabs, i32x4, 1, "Iabs4s", 0x4EA0_B800u32),
    ] {
        let a = ctx.alloc_xreg(forge_ir::RegClass::VEC(128));
        let b = ctx.alloc_xreg(forge_ir::RegClass::VEC(128));
        let out = ctx.alloc_xreg(forge_ir::RegClass::VEC(128));
        for x in [a, b, out] {
            ctx.set_value_desc(x, ty);
        }
        let args: &[forge_ir::XReg] = if arity == 2 { &[a, b] } else { &[a] };
        let pack = tm
            .lowering()
            .lower_inst(&op, args, &[out], &mut ctx)
            .unwrap_or_else(|e| panic!("{op:?} 应可降级：{e:?}"));
        let got = format!("{:?}", pack.insts[0]);
        assert!(got.starts_with(want), "{op:?} 应为 {want}，实际 {got}");
        let mut sink = forge_codegen::CodeSink::default();
        tm.encoder()
            .encode(
                &pack.insts[0],
                &forge_codegen::AllocResult::new(),
                &mut sink,
            )
            .expect("编码");
        assert_eq!(sink.bytes(), &word.to_le_bytes(), "{op:?} 编码不符 oracle");
    }

    // i32x4 的 Vdiv 没有 NEON 整数除法 ⇒ 必须 fail-closed（不是静默错值）。
    let a = ctx.alloc_xreg(forge_ir::RegClass::VEC(128));
    let b = ctx.alloc_xreg(forge_ir::RegClass::VEC(128));
    let out = ctx.alloc_xreg(forge_ir::RegClass::VEC(128));
    for x in [a, b, out] {
        ctx.set_value_desc(x, i32x4);
    }
    assert!(
        tm.lowering()
            .lower_inst(&Opcode::Vdiv, &[a, b], &[out], &mut ctx)
            .is_err(),
        "i32x4 Vdiv 应 fail-closed"
    );
}

/// **NEON lane 提取守卫**（v27 M6-138）：i32x4 的 `vextract(v, i)` → `umov w, v.s[i]`。
/// 编码（oracle）全 v0 形态：lane0..3 = 0x0E043C00 / 0x0E0C3C00 / 0x0E143C00 / 0x0E1C3C00。
#[test]
fn lowering_arm64_neon_vextract_i32x4() {
    use forge_codegen::prelude::{LowerCtx, Opcode};
    use forge_ir::ir::types::TypeContext;

    let tc = TypeContext::new();
    let i32x4 = tc.vector_ty(forge_ir::TypeId::I32, 4);
    let tm = forge_codegen::arm64::TargetMachine::new();
    let mut ctx = LowerCtx::default();
    ctx.type_store = Some(tc.borrow());

    for (lane, word) in [
        (0u64, 0x0E04_3C00u32),
        (1, 0x0E0C_3C00),
        (2, 0x0E14_3C00),
        (3, 0x0E1C_3C00),
    ] {
        let vec = ctx.alloc_xreg(forge_ir::RegClass::VEC(128));
        ctx.set_value_desc(vec, i32x4);
        let idx = ctx.alloc_xreg(forge_ir::RegClass::GPR(32));
        ctx.set_value_desc(idx, forge_ir::TypeId::I32);
        let out = ctx.alloc_xreg(forge_ir::RegClass::GPR(32));
        ctx.set_value_desc(out, forge_ir::TypeId::I32);
        ctx.current_immediates = smallvec::smallvec![lane];
        let pack = tm
            .lowering()
            .lower_inst(&Opcode::Vextract, &[vec, idx], &[out], &mut ctx)
            .unwrap_or_else(|e| panic!("Vextract lane {lane} 应可降级：{e:?}"));
        let got = format!("{:?}", pack.insts[0]);
        assert!(
            got.starts_with("Umov4sL"),
            "lane {lane} 应为 UMOV，实际 {got}"
        );
        let mut sink = forge_codegen::CodeSink::default();
        tm.encoder()
            .encode(
                &pack.insts[0],
                &forge_codegen::AllocResult::new(),
                &mut sink,
            )
            .expect("编码");
        assert_eq!(
            sink.bytes(),
            &word.to_le_bytes(),
            "lane {lane} 编码不符 oracle"
        );
    }
}

// ─────────── NEON 覆盖面补齐（v27 KD-05：整数算术 / 位运算 / 位重解释 /
// 浮点 lane 提取 / lane 插入）───────────
//
// 这些守卫都钉三件事：① 规则的 `when` 谓词命中（rd=128 + elem/imm0）；
// ② 落到**哪条**指令（名字前缀）；③ 编码与 clang oracle 逐字一致。
// 期望字全部是"寄存器下标全 0"形态（`AllocResult::new()` 下未分配 vreg 编 0）。

/// **A. V128 整数加/减/乘**（i8x16 / i16x8 / i64x2）。
/// oracle（全 v0 形态）：add.16b=0x4E208400、add.8h=0x4E608400、add.2d=0x4EE08400、
/// sub.16b=0x6E208400、sub.8h=0x6E608400、sub.2d=0x6EE08400、
/// mul.16b=0x4E209C00、mul.8h=0x4E609C00。
/// i64x2 的 `Vmul` 没有 NEON 指令（无 64 位 lane 整数乘）⇒ 必须 fail-closed。
#[test]
fn lowering_arm64_neon_int_v128() {
    use forge_codegen::prelude::{LowerCtx, Opcode};
    use forge_ir::ir::types::TypeContext;

    let tc = TypeContext::new();
    let i8x16 = tc.vector_ty(forge_ir::TypeId::I8, 16);
    let i16x8 = tc.vector_ty(forge_ir::TypeId::I16, 8);
    let i64x2 = tc.vector_ty(forge_ir::TypeId::I64, 2);
    let tm = forge_codegen::arm64::TargetMachine::new();
    let mut ctx = LowerCtx::default();
    ctx.type_store = Some(tc.borrow());

    for (op, ty, want, word) in [
        (Opcode::Vadd, i8x16, "Vadd16b", 0x4E20_8400u32),
        (Opcode::Vadd, i16x8, "Vadd8h", 0x4E60_8400u32),
        (Opcode::Vadd, i64x2, "Vadd2d", 0x4EE0_8400u32),
        (Opcode::Vsub, i8x16, "Vsub16b", 0x6E20_8400u32),
        (Opcode::Vsub, i16x8, "Vsub8h", 0x6E60_8400u32),
        (Opcode::Vsub, i64x2, "Vsub2d", 0x6EE0_8400u32),
        (Opcode::Vmul, i8x16, "Vmul16b", 0x4E20_9C00u32),
        (Opcode::Vmul, i16x8, "Vmul8h", 0x4E60_9C00u32),
    ] {
        let a = ctx.alloc_xreg(forge_ir::RegClass::VEC(128));
        let b = ctx.alloc_xreg(forge_ir::RegClass::VEC(128));
        let out = ctx.alloc_xreg(forge_ir::RegClass::VEC(128));
        for x in [a, b, out] {
            ctx.set_value_desc(x, ty);
        }
        let pack = tm
            .lowering()
            .lower_inst(&op, &[a, b], &[out], &mut ctx)
            .unwrap_or_else(|e| panic!("{op:?} 应可降级：{e:?}"));
        assert_eq!(pack.insts.len(), 1, "{op:?} → 单条微指令");
        let got = format!("{:?}", pack.insts[0]);
        assert!(got.starts_with(want), "{op:?} 应为 {want}，实际 {got}");
        let mut sink = forge_codegen::CodeSink::default();
        tm.encoder()
            .encode(
                &pack.insts[0],
                &forge_codegen::AllocResult::new(),
                &mut sink,
            )
            .expect("编码");
        assert_eq!(sink.bytes(), &word.to_le_bytes(), "{op:?} 编码不符 oracle");
    }

    // i64x2 的 Vmul：NEON 无 64 位 lane 整数乘 ⇒ fail-closed（不是静默错值）。
    let a = ctx.alloc_xreg(forge_ir::RegClass::VEC(128));
    let b = ctx.alloc_xreg(forge_ir::RegClass::VEC(128));
    let out = ctx.alloc_xreg(forge_ir::RegClass::VEC(128));
    for x in [a, b, out] {
        ctx.set_value_desc(x, i64x2);
    }
    assert!(
        tm.lowering()
            .lower_inst(&Opcode::Vmul, &[a, b], &[out], &mut ctx)
            .is_err(),
        "i64x2 Vmul 应 fail-closed（NEON 没有 64 位 lane 整数乘）"
    );
}

/// **B/C. 向量位运算 + 位重解释**（V128）。
/// oracle：and.16b=0x4E201C00、orr.16b=0x4EA01C00、eor.16b=0x6E201C00；
/// 位重解释用 `orr vD.16b, vN.16b, vN.16b`（128 位 move 别名）⇒ 同 orr.16b 编码。
#[test]
fn lowering_arm64_neon_bitwise_and_bitcast() {
    use forge_codegen::prelude::{LowerCtx, Opcode};
    use forge_ir::ir::types::TypeContext;

    let tc = TypeContext::new();
    let i32x4 = tc.vector_ty(forge_ir::TypeId::I32, 4);
    let f32x4 = tc.vector_ty(forge_ir::TypeId::F32, 4);
    let tm = forge_codegen::arm64::TargetMachine::new();
    let mut ctx = LowerCtx::default();
    ctx.type_store = Some(tc.borrow());

    for (op, want, word) in [
        (Opcode::Band, "Vand16b", 0x4E20_1C00u32),
        (Opcode::Bor, "Vorr16b", 0x4EA0_1C00u32),
        (Opcode::Bxor, "Veor16b", 0x6E20_1C00u32),
    ] {
        let a = ctx.alloc_xreg(forge_ir::RegClass::VEC(128));
        let b = ctx.alloc_xreg(forge_ir::RegClass::VEC(128));
        let out = ctx.alloc_xreg(forge_ir::RegClass::VEC(128));
        for x in [a, b, out] {
            ctx.set_value_desc(x, i32x4);
        }
        let pack = tm
            .lowering()
            .lower_inst(&op, &[a, b], &[out], &mut ctx)
            .unwrap_or_else(|e| panic!("{op:?} 应可降级：{e:?}"));
        assert_eq!(pack.insts.len(), 1, "{op:?} → 单条微指令");
        let got = format!("{:?}", pack.insts[0]);
        assert!(got.starts_with(want), "{op:?} 应为 {want}，实际 {got}");
        let mut sink = forge_codegen::CodeSink::default();
        tm.encoder()
            .encode(
                &pack.insts[0],
                &forge_codegen::AllocResult::new(),
                &mut sink,
            )
            .expect("编码");
        assert_eq!(sink.bytes(), &word.to_le_bytes(), "{op:?} 编码不符 oracle");
    }

    // f32x4 的位运算同样走 and.16b（按位语义与元素类型无关）。
    let a = ctx.alloc_xreg(forge_ir::RegClass::VEC(128));
    let b = ctx.alloc_xreg(forge_ir::RegClass::VEC(128));
    let out = ctx.alloc_xreg(forge_ir::RegClass::VEC(128));
    for x in [a, b, out] {
        ctx.set_value_desc(x, f32x4);
    }
    let pack = tm
        .lowering()
        .lower_inst(&Opcode::Band, &[a, b], &[out], &mut ctx)
        .expect("f32x4 Band 应可降级");
    assert!(format!("{:?}", pack.insts[0]).starts_with("Vand16b"));

    // C. Vbitcast：`orr vD.16b, vN.16b, vN.16b`（{0} 用两次 ⇒ rn=rm）。
    let v = ctx.alloc_xreg(forge_ir::RegClass::VEC(128));
    let out = ctx.alloc_xreg(forge_ir::RegClass::VEC(128));
    for x in [v, out] {
        ctx.set_value_desc(x, i32x4);
    }
    let pack = tm
        .lowering()
        .lower_inst(&Opcode::Vbitcast, &[v], &[out], &mut ctx)
        .expect("Vbitcast 应可降级");
    assert_eq!(pack.insts.len(), 1, "Vbitcast → 单条 Q 搬运");
    let got = format!("{:?}", pack.insts[0]);
    assert!(
        got.starts_with("Vorr16b"),
        "Vbitcast 应为 Vorr16b，实际 {got}"
    );
    let mut sink = forge_codegen::CodeSink::default();
    tm.encoder()
        .encode(
            &pack.insts[0],
            &forge_codegen::AllocResult::new(),
            &mut sink,
        )
        .expect("编码 Vbitcast");
    assert_eq!(
        sink.bytes(),
        &0x4EA0_1C00u32.to_le_bytes(),
        "Vbitcast 编码不符 oracle（orr v0.16b,v0.16b,v0.16b）"
    );
}

/// **D. 浮点 lane 提取**：`umov w/x, v.s/d[i]` 取位型 → `fmov s/d, w/x` 进标量 FPR。
/// oracle：umov w0,v0.s[0..3]=0x0E043C00/0x0E0C3C00/0x0E143C00/0x0E1C3C00、
/// umov x0,v0.d[0..1]=0x4E083C00/0x4E183C00、fmov s0,w0=0x1E270000、
/// fmov d0,x0=0x9E670000。
#[test]
fn lowering_arm64_neon_vextract_f32_f64() {
    use forge_codegen::prelude::{LowerCtx, Opcode};
    use forge_ir::ir::types::TypeContext;

    let tc = TypeContext::new();
    let f32x4 = tc.vector_ty(forge_ir::TypeId::F32, 4);
    let f64x2 = tc.vector_ty(forge_ir::TypeId::F64, 2);
    let tm = forge_codegen::arm64::TargetMachine::new();
    let mut ctx = LowerCtx::default();
    ctx.type_store = Some(tc.borrow());

    // (lane, 向量类型, 结果类型, 期望两条指令前缀, umov 编码, fmov 编码)
    for (lane, vec_ty, out_ty, umov_prefix, umov_word, fmov_word) in [
        (
            0u64,
            f32x4,
            forge_ir::TypeId::F32,
            "Umov4sL0",
            0x0E04_3C00u32,
            0x1E27_0000u32,
        ),
        (
            1,
            f32x4,
            forge_ir::TypeId::F32,
            "Umov4sL1",
            0x0E0C_3C00,
            0x1E27_0000,
        ),
        (
            2,
            f32x4,
            forge_ir::TypeId::F32,
            "Umov4sL2",
            0x0E14_3C00,
            0x1E27_0000,
        ),
        (
            3,
            f32x4,
            forge_ir::TypeId::F32,
            "Umov4sL3",
            0x0E1C_3C00,
            0x1E27_0000,
        ),
        (
            0,
            f64x2,
            forge_ir::TypeId::F64,
            "Umov2dL0",
            0x4E08_3C00,
            0x9E67_0000,
        ),
        (
            1,
            f64x2,
            forge_ir::TypeId::F64,
            "Umov2dL1",
            0x4E18_3C00,
            0x9E67_0000,
        ),
    ] {
        let vec = ctx.alloc_xreg(forge_ir::RegClass::VEC(128));
        ctx.set_value_desc(vec, vec_ty);
        let idx = ctx.alloc_xreg(forge_ir::RegClass::GPR(32));
        ctx.set_value_desc(idx, forge_ir::TypeId::I32);
        // 浮点结果在 FPR 值类里（本谱 reg_class_for(f32)=FPR(32)、f64=FPR(64)）。
        let out_cls = if out_ty == forge_ir::TypeId::F32 {
            forge_ir::RegClass::FPR(32)
        } else {
            forge_ir::RegClass::FPR(64)
        };
        let out = ctx.alloc_xreg(out_cls);
        ctx.set_value_desc(out, out_ty);
        ctx.current_immediates = smallvec::smallvec![lane];
        let pack = tm
            .lowering()
            .lower_inst(&Opcode::Vextract, &[vec, idx], &[out], &mut ctx)
            .unwrap_or_else(|e| panic!("Vextract {out_ty:?} lane {lane} 应可降级：{e:?}"));
        assert_eq!(pack.insts.len(), 2, "{out_ty:?} lane {lane} → umov + fmov");
        let got0 = format!("{:?}", pack.insts[0]);
        assert!(
            got0.starts_with(umov_prefix),
            "{out_ty:?} lane {lane} 应为 {umov_prefix}，实际 {got0}"
        );
        let mut sink = forge_codegen::CodeSink::default();
        tm.encoder()
            .encode(
                &pack.insts[0],
                &forge_codegen::AllocResult::new(),
                &mut sink,
            )
            .expect("编码 umov");
        assert_eq!(
            sink.bytes(),
            &umov_word.to_le_bytes(),
            "{out_ty:?} lane {lane} umov 编码不符 oracle"
        );
        let mut sink2 = forge_codegen::CodeSink::default();
        tm.encoder()
            .encode(
                &pack.insts[1],
                &forge_codegen::AllocResult::new(),
                &mut sink2,
            )
            .expect("编码 fmov");
        assert_eq!(
            sink2.bytes(),
            &fmov_word.to_le_bytes(),
            "{out_ty:?} lane {lane} fmov 编码不符 oracle"
        );
    }
}

/// **E. lane 插入**（Vinsert，rd=128）：先 Q 搬运整向量到 out，再 INS 单 lane。
/// oracle：mov v0.s[0..3],w0 = 0x4E041C00/0x4E0C1C00/0x4E141C00/0x4E1C1C00、
/// mov v0.d[0..1],x0 = 0x4E081C00/0x4E181C00；搬运 = orr v0.16b,v0.16b,v0.16b=0x4EA01C00；
/// 浮点源先 `fmov w0,s0`=0x1E260000 / `fmov x0,d0`=0x9E660000。
#[test]
fn lowering_arm64_neon_vinsert() {
    use forge_codegen::prelude::{LowerCtx, Opcode};
    use forge_ir::ir::types::TypeContext;

    let tc = TypeContext::new();
    let f32x4 = tc.vector_ty(forge_ir::TypeId::F32, 4);
    let f64x2 = tc.vector_ty(forge_ir::TypeId::F64, 2);
    let i32x4 = tc.vector_ty(forge_ir::TypeId::I32, 4);
    let i64x2 = tc.vector_ty(forge_ir::TypeId::I64, 2);
    let tm = forge_codegen::arm64::TargetMachine::new();
    let mut ctx = LowerCtx::default();
    ctx.type_store = Some(tc.borrow());

    // (lane, 向量类型, 标量类型/寄存器类, 期望指令序列前缀, INS 编码)
    for (lane, vec_ty, elem_ty, elem_cls, want, ins_word) in [
        (
            0u64,
            f32x4,
            forge_ir::TypeId::F32,
            forge_ir::RegClass::FPR(32),
            vec!["Vorr16b", "FmovWFromS", "Vins4sL0"],
            0x4E04_1C00u32,
        ),
        (
            1,
            f32x4,
            forge_ir::TypeId::F32,
            forge_ir::RegClass::FPR(32),
            vec!["Vorr16b", "FmovWFromS", "Vins4sL1"],
            0x4E0C_1C00,
        ),
        (
            2,
            f32x4,
            forge_ir::TypeId::F32,
            forge_ir::RegClass::FPR(32),
            vec!["Vorr16b", "FmovWFromS", "Vins4sL2"],
            0x4E14_1C00,
        ),
        (
            3,
            f32x4,
            forge_ir::TypeId::F32,
            forge_ir::RegClass::FPR(32),
            vec!["Vorr16b", "FmovWFromS", "Vins4sL3"],
            0x4E1C_1C00,
        ),
        (
            0,
            f64x2,
            forge_ir::TypeId::F64,
            forge_ir::RegClass::FPR(64),
            vec!["Vorr16b", "FmovXFromD", "Vins2dL0"],
            0x4E08_1C00,
        ),
        (
            1,
            f64x2,
            forge_ir::TypeId::F64,
            forge_ir::RegClass::FPR(64),
            vec!["Vorr16b", "FmovXFromD", "Vins2dL1"],
            0x4E18_1C00,
        ),
        (
            0,
            i32x4,
            forge_ir::TypeId::I32,
            forge_ir::RegClass::GPR(32),
            vec!["Vorr16b", "Vins4sL0"],
            0x4E04_1C00,
        ),
        (
            3,
            i32x4,
            forge_ir::TypeId::I32,
            forge_ir::RegClass::GPR(32),
            vec!["Vorr16b", "Vins4sL3"],
            0x4E1C_1C00,
        ),
        (
            0,
            i64x2,
            forge_ir::TypeId::I64,
            forge_ir::RegClass::GPR(64),
            vec!["Vorr16b", "Vins2dL0"],
            0x4E08_1C00,
        ),
        (
            1,
            i64x2,
            forge_ir::TypeId::I64,
            forge_ir::RegClass::GPR(64),
            vec!["Vorr16b", "Vins2dL1"],
            0x4E18_1C00,
        ),
    ] {
        let vec = ctx.alloc_xreg(forge_ir::RegClass::VEC(128));
        ctx.set_value_desc(vec, vec_ty);
        let elem = ctx.alloc_xreg(elem_cls);
        ctx.set_value_desc(elem, elem_ty);
        let idx = ctx.alloc_xreg(forge_ir::RegClass::GPR(32));
        ctx.set_value_desc(idx, forge_ir::TypeId::I32);
        let out = ctx.alloc_xreg(forge_ir::RegClass::VEC(128));
        ctx.set_value_desc(out, vec_ty);
        ctx.current_immediates = smallvec::smallvec![lane];
        let pack = tm
            .lowering()
            .lower_inst(&Opcode::Vinsert, &[vec, elem, idx], &[out], &mut ctx)
            .unwrap_or_else(|e| panic!("Vinsert {vec_ty:?} lane {lane} 应可降级：{e:?}"));
        assert_eq!(
            pack.insts.len(),
            want.len(),
            "{vec_ty:?} lane {lane} 指令条数"
        );
        for (i, w) in want.iter().enumerate() {
            let got = format!("{:?}", pack.insts[i]);
            assert!(
                got.starts_with(w),
                "{vec_ty:?} lane {lane} 第 {i} 条应为 {w}，实际 {got}"
            );
        }
        // 搬运（第 0 条）与 INS（最后一条）的编码都对照 oracle。
        let mut sink = forge_codegen::CodeSink::default();
        tm.encoder()
            .encode(
                &pack.insts[0],
                &forge_codegen::AllocResult::new(),
                &mut sink,
            )
            .expect("编码搬运");
        assert_eq!(
            sink.bytes(),
            &0x4EA0_1C00u32.to_le_bytes(),
            "Vinsert 的 Q 搬运应为 orr v0.16b,v0.16b,v0.16b"
        );
        let last = pack.insts.len() - 1;
        let mut sink2 = forge_codegen::CodeSink::default();
        tm.encoder()
            .encode(
                &pack.insts[last],
                &forge_codegen::AllocResult::new(),
                &mut sink2,
            )
            .expect("编码 INS");
        assert_eq!(
            sink2.bytes(),
            &ins_word.to_le_bytes(),
            "{vec_ty:?} lane {lane} INS 编码不符 oracle"
        );
    }
}

/// **F. Vconst 材料化**（v27 KD-05）：128 位常量 → `movz/movk` 拼低 64 位 → `fmov d`
/// → 再拼高 64 位 → `INS d[1]`。钉住三件事：
/// ① 10 条指令的**顺序**（先低半再高半——写 D 视图会清零 Q 的高半）；
/// ② 分片立即数取**连续** 64 位半（不是 x86 `PUNPCK` 交错序）；
/// ③ 每条的编码字节（clang oracle，x0/v0 形态）。
///
/// 两个常量：第一个是 [1.0f32, 2.0, 3.0, 4.0]（lo = 0x40000000_3F800000、
/// hi = 0x40800000_40400000）；第二个**四片各不相同**
/// （lo = 0x12345678_9ABCDEF0 → f3=0x1234/f2=0x5678/f1=0x9ABC/f0=0xDEF0、
/// hi = 0xFEDCBA98_76543210 → f3=0xFEDC/f2=0xBA98/f1=0x7654/f0=0x3210）——
/// 分片错位（f2 与 f1/f3 互换）在第一个常量上会被 0 掩盖，第二个不会。
#[test]
fn lowering_arm64_neon_vconst() {
    use forge_codegen::prelude::{LowerCtx, Opcode};
    use forge_ir::ir::types::TypeContext;

    let tc = TypeContext::new();
    let f32x4 = tc.vector_ty(forge_ir::TypeId::F32, 4);
    let tm = forge_codegen::arm64::TargetMachine::new();
    let mut ctx = LowerCtx::default();
    ctx.type_store = Some(tc.borrow());

    // (常量字节, 期望的 10 条（前缀, 编码）)
    let case_a: Vec<u8> = [1.0f32, 2.0, 3.0, 4.0]
        .iter()
        .flat_map(|v| v.to_bits().to_le_bytes())
        .collect();
    let case_b: Vec<u8> = vec![
        0xF0, 0xDE, 0xBC, 0x9A, 0x78, 0x56, 0x34, 0x12, // lo = 0x12345678_9ABCDEF0
        0x10, 0x32, 0x54, 0x76, 0x98, 0xBA, 0xDC, 0xFE, // hi = 0xFEDCBA98_76543210
    ];
    let cases: [(&[u8], [(&str, u32); 10]); 2] = [
        (
            case_a.as_slice(),
            [
                ("Movzx3", 0xD2E8_0000),
                ("Movkx2", 0xF2C0_0000),
                ("Movkx1", 0xF2A7_F000),
                ("Movkx", 0xF280_0000),
                ("FmovDFromX", 0x9E67_0000),
                ("Movzx3", 0xD2E8_1000),
                ("Movkx2", 0xF2C0_0000),
                ("Movkx1", 0xF2A8_0800),
                ("Movkx", 0xF280_0000),
                ("Vins2dL1", 0x4E18_1C00),
            ],
        ),
        (
            case_b.as_slice(),
            [
                ("Movzx3", 0xD2E2_4680),
                ("Movkx2", 0xF2CA_CF00),
                ("Movkx1", 0xF2B3_5780),
                ("Movkx", 0xF29B_DE00),
                ("FmovDFromX", 0x9E67_0000),
                ("Movzx3", 0xD2FF_DB80),
                ("Movkx2", 0xF2D7_5300),
                ("Movkx1", 0xF2AE_CA80),
                ("Movkx", 0xF286_4200),
                ("Vins2dL1", 0x4E18_1C00),
            ],
        ),
    ];

    for (bytes, want) in cases {
        let mut pool = forge_ir::ConstantPool::new();
        let cid = pool.intern_vector(bytes, forge_ir::Endianness::Little);
        ctx.constant_pool = Some(pool);
        ctx.current_const_index = cid.raw();

        let out = ctx.alloc_xreg(forge_ir::RegClass::VEC(128));
        ctx.set_value_desc(out, f32x4);
        let pack = tm
            .lowering()
            .lower_inst(&Opcode::Vconst, &[], &[out], &mut ctx)
            .unwrap_or_else(|e| panic!("Vconst(V128) 应可降级：{e:?}"));
        assert_eq!(
            pack.insts.len(),
            10,
            "Vconst → movz/movk×4 + fmov + ×4 + ins"
        );
        for (i, (prefix, word)) in want.iter().enumerate() {
            let got = format!("{:?}", pack.insts[i]);
            assert!(
                got.starts_with(prefix),
                "Vconst 第 {i} 条应为 {prefix}，实际 {got}"
            );
            let mut sink = forge_codegen::CodeSink::default();
            tm.encoder()
                .encode(
                    &pack.insts[i],
                    &forge_codegen::AllocResult::new(),
                    &mut sink,
                )
                .expect("编码 Vconst");
            assert_eq!(
                sink.bytes(),
                &word.to_le_bytes(),
                "Vconst 第 {i} 条编码不符 oracle（{prefix}）"
            );
        }
    }

    // 反向守卫：只接了 V128 ⇒ V64 的 Vconst 仍 fail-closed。
    let out64 = ctx.alloc_xreg(forge_ir::RegClass::VEC(64));
    let f32x2 = tc.vector_ty(forge_ir::TypeId::F32, 2);
    ctx.set_value_desc(out64, f32x2);
    assert!(
        tm.lowering()
            .lower_inst(&Opcode::Vconst, &[], &[out64], &mut ctx)
            .is_err(),
        "V64 的 Vconst 应 fail-closed"
    );
}
