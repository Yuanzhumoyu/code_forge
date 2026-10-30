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
    let f64x2 = tc.vector_ty(forge_ir::TypeId::F64, 2);
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

    for (elem, vec_ty, want, word) in [
        (forge_ir::TypeId::F32, i32x4, "Dup4sW", 0x4E04_0C00u32),
        (forge_ir::TypeId::F64, f64x2, "Dup2dX", 0x4E08_0C00u32),
    ] {
        let src_cls = if elem == forge_ir::TypeId::F32 {
            forge_ir::RegClass::GPR(32)
        } else {
            forge_ir::RegClass::GPR(64)
        };
        let scalar = ctx.alloc_xreg(src_cls);
        ctx.set_value_desc(scalar, elem);
        let out = ctx.alloc_xreg(forge_ir::RegClass::VEC(128));
        ctx.set_value_desc(out, vec_ty);
        let pack = tm
            .lowering()
            .lower_inst(&Opcode::Vbroadcast, &[scalar], &[out], &mut ctx)
            .unwrap_or_else(|e| panic!("Vbroadcast {elem:?} 应可降级：{e:?}"));
        let got = format!("{:?}", pack.insts[0]);
        assert!(got.starts_with(want), "splat 应为 {want}，实际 {got}");
        let mut sink = forge_codegen::CodeSink::default();
        tm.encoder()
            .encode(
                &pack.insts[0],
                &forge_codegen::AllocResult::new(),
                &mut sink,
            )
            .expect("编码 splat");
        assert_eq!(sink.bytes(), &word.to_le_bytes(), "splat 编码不符 oracle");
    }
}
