//! demo8_v12 — **1 字节寄存器 ISA** 回归测试（去「寄存器类型/宽度写死」）。
//!
//! 夹具（`isa/demo8_v12.toml`）只有唯一的 1 字节 GPR 组 `[reg.gpr1]`：历史实现
//! 在生成期锚定 `GPR(8).or(GPR(4))`、把地址类/值池/栈槽/帧开销写死 8 字节，会在
//! 这个 ISA 上静默退化（空名字表、构造不存在的类）。本文件断言三件事：
//!
//! 1. **元数据派生**：主 GPR 类/地址类/值池/槽单位/帧开销全部 = 1 字节；
//!    sp/fp/scratch/callee_saved 名字解析成功（不再是空表）；allocatable 正确排除。
//! 2. **汇编/编码/解码往返**在同一路径上仍然正确（1 字节寄存器不改变指令字宽）。
//! 3. **宿主编译**：i8-only 函数可编译出机器码；i64 等宽类型被值池门
//!    **编译期拒绝**（`Unsupported`），不再按 8 字节池生成不存在的寄存器类。

mod common;

use common::demo8_v12::{Inst, TargetMachine, assemble, decode, disassemble, encode};
use forge_codegen::FunctionCompiler;
use forge_codegen::TargetMachine as TargetMachineTrait;
use forge_ir::{FunctionBuilder, FunctionSignature, PhysReg, RegClass, TypeContext, TypeId};

fn enc(asm: &str) -> Vec<u8> {
    let inst = assemble(asm).unwrap_or_else(|e| panic!("assemble `{asm}`: {e}"));
    encode(&inst).unwrap_or_else(|e| panic!("encode `{asm}`: {e}"))
}

/// 32 位小端字：opcode | rd<<8 | rs1<<11 | rs2<<14（与 demo_v12 同布局）。
fn word(b0: u8, rd: u32, rs1: u32, rs2: u32) -> Vec<u8> {
    let w = (b0 as u32) | (rd << 8) | (rs1 << 11) | (rs2 << 14);
    w.to_le_bytes().to_vec()
}

/// 32 位小端字：opcode | rd<<8 | imm8<<16（RI2 形式）。
fn imm_word(b0: u8, rd: u32, imm: u32) -> Vec<u8> {
    let w = (b0 as u32) | (rd << 8) | (imm << 16);
    w.to_le_bytes().to_vec()
}

// ───────────────── 1. 宽度元数据派生（核心回归）─────────────────

/// 1 字节寄存器 ISA 的类/宽度全部由元数据派生——不再是 8 字节缺省。
#[test]
fn one_byte_register_metadata_is_derived() {
    let tm = TargetMachine::new();
    let ri = TargetMachineTrait::reg_info(&tm);
    assert_eq!(
        ri.default_gpr_class(),
        RegClass::GPR(1),
        "主 GPR 类 = 最宽已声明组（唯一 [reg.gpr1]）"
    );
    assert_eq!(ri.addr_class(), RegClass::GPR(1), "[meta].addr_width = 1");
    assert_eq!(
        ri.value_gpr_class(),
        RegClass::GPR(1),
        "[meta].value_gpr_width = 1（宿主值池）"
    );
    assert_eq!(ri.slot_bytes(), 1, "[meta].slot_bytes = 1（栈槽单位）");
    assert_eq!(
        ri.frame_pointer_overhead(),
        1,
        "帧指针保存槽 = [meta].fp_overhead_bytes = 1（历史写死 8）"
    );
    assert_eq!(ri.num_gp_regs(), 8, "唯一组的 8 个寄存器");
    // 名字解析：sp/fp/scratch 全部命中（历史：锚点 GPR(8)/GPR(4) 都不存在 →
    // 空名字表 → 这些全部静默丢弃/落回索引 0）。
    assert_eq!(
        ri.sp_reg().register_index(),
        Some(7),
        "[machine.frame].sp = A7"
    );
    assert_eq!(ri.fp_reg().map(|r| r.to_index()), Some(6), "fp = A6");
    assert_eq!(ri.scratch_regs(), vec![4, 5], "[abi].scratch = A4/A5");
    assert_eq!(
        ri.callee_saved(),
        Vec::<u32>::new(),
        "[abi.callee_saved] 空"
    );
    // allocatable = 0..8 排除 sp(7)/fp(6)/scratch(4,5)/reserved(6,7) → A0..A3
    assert_eq!(ri.allocatable_gp_order(), vec![0, 1, 2, 3]);
}

/// 值池门：i8 可承载；i16/i32/i64/指针（宽 > 1 字节）与**全部浮点/向量**
/// （本 ISA 未声明任何 FPR 组 ⇒ 寄存器文件不存在）一律按类型拒绝。
/// fail-closed 的关键：拒绝发生在编译期，而不是按 8 字节池生成不存在的类。
#[test]
fn one_byte_pool_rejects_wide_types() {
    let tm = TargetMachine::new();
    let ri = TargetMachineTrait::reg_info(&tm);
    assert_eq!(ri.class_for_type(TypeId::I8), Some(RegClass::GPR(1)));
    assert_eq!(ri.class_for_type(TypeId::I16), None, "无 GPR(2) 组");
    assert_eq!(ri.class_for_type(TypeId::I32), None, "无 GPR(4) 组");
    assert_eq!(ri.class_for_type(TypeId::I64), None, "无 GPR(8) 组");
    // 注：PTR 由夹具的 `[types] ptr = "gpr1"` 显式映射承载 —— 见
    // `class_table_has_only_declared_classes` 与 `explicit_type_map_*` 断言。
    // 无 FPR 组 → 浮点/向量寄存器文件不存在（值池门必须看**文件存在性**，
    // 只看宽度会让 f64 落到一个该 ISA 没有的 FPR(8) 类上）。
    assert_eq!(ri.class_for_type(TypeId::F32), None, "无浮点寄存器组");
    assert_eq!(ri.class_for_type(TypeId::F64), None, "无浮点寄存器组");
    assert_eq!(ri.class_for_type(TypeId::V64), None, "无向量寄存器组");
    assert_eq!(ri.class_for_type(TypeId::V128), None, "无向量寄存器组");
    assert_eq!(ri.class_for_type(TypeId::V256), None, "无向量寄存器组");
    assert_eq!(ri.class_for_type(TypeId::VOID), None, "void = 无寄存器");
    // `[types] ptr = "gpr1"`：**显式映射优先于通用规则**——本 ISA 地址宽 1 字节，
    // 所以指针可承载（通用规则会按 `TypeId::bits()` 的 8 字节把它拒掉）。
    assert_eq!(
        ri.class_for_type(TypeId::PTR),
        Some(RegClass::GPR(1)),
        "[types] 显式映射必须生效"
    );
    assert!(
        ri.type_map().contains(&(TypeId::PTR, RegClass::GPR(1))),
        "type_map 必须暴露给 lowering：{:?}",
        ri.type_map()
    );
    // 显式映射只覆盖列出的类型，其余仍走值池门。
    assert_eq!(ri.class_for_type(TypeId::I64), None, "i64 仍未声明 → 拒绝");
}

/// 类表 = ISA 声明：1 字节 ISA 只有 `GPR(1)`——不再有"编造的 fallback 类表"
/// （历史实现会凭空造出 GPR(2)/GPR(4)/FPR(4)/FPR(8)/VEC(16)… 并借同族池，
/// 使未声明类变成"可分配但不可编码"）。
#[test]
fn class_table_has_only_declared_classes() {
    let tm = TargetMachine::new();
    let classes = TargetMachineTrait::reg_info(&tm).register_classes();
    let list: Vec<RegClass> = classes.iter().map(|c| c.reg_class).collect();
    assert_eq!(
        list,
        vec![RegClass::GPR(1)],
        "唯一声明组 [reg.gpr1] ⇒ 类表只能有 GPR(1)：{list:?}"
    );
    assert_eq!(classes[0].width, 1, "类宽 = 1 字节");
    assert_eq!(classes[0].allocatable, vec![0, 1, 2, 3], "池 = A0..A3");
}

// ───────────────── 2. 汇编/编码/解码往返 ─────────────────

#[test]
fn encode_layout_is_metadata_free_of_x86_defaults() {
    // add：opcode + rd/rs1/rs2（3 位字段够 8 个寄存器）
    assert_eq!(enc("add a1, a2, a3"), word(0x10, 1, 2, 3));
    // mov 寄存器形态
    assert_eq!(enc("mov a1, a2"), word(0x20, 1, 2, 0));
    // mov 立即数形态（RI2：rd + imm8）
    assert_eq!(enc("mov a1, 7"), imm_word(0x21, 1, 7));
    // 加载/存储（RI：rd/rs1/imm8）
    let ri_word = |b0: u8, rd: u32, rs1: u32, imm: u32| {
        let w = (b0 as u32) | (rd << 8) | (rs1 << 11) | (imm << 16);
        w.to_le_bytes().to_vec()
    };
    assert_eq!(enc("ld a1, a2, 3"), ri_word(0x30, 1, 2, 3));
    assert_eq!(enc("st a1, a2, 3"), ri_word(0x31, 1, 2, 3));
    assert_eq!(enc("subi a7, a7, 2"), ri_word(0x40, 7, 7, 2));
    assert_eq!(enc("addi a7, a7, 2"), ri_word(0x41, 7, 7, 2));
    // 分支（B：rs1 + imm8）
    assert_eq!(enc("brz a1, -4"), {
        let w = 0x50u32 | (1 << 11) | (((-4i32) as u32 & 0xFF) << 16);
        w.to_le_bytes().to_vec()
    });
    // ret：全字位域
    assert_eq!(enc("ret"), 1u32.to_le_bytes().to_vec());
    assert_eq!(enc("nop"), 0u32.to_le_bytes().to_vec());
}

#[test]
fn assemble_encode_decode_roundtrip() {
    for asm in [
        "add a0, a1, a2",
        "sub a7, a0, a1",
        "mov a3, a4",
        "mov a5, 42",
        "ld a1, a2, 5",
        "st a1, a2, 5",
        "subi a7, a7, 2",
        "addi a7, a7, 2",
        "nop",
        "ret",
    ] {
        let inst = assemble(asm).unwrap_or_else(|e| panic!("assemble `{asm}`: {e}"));
        let bytes = encode(&inst).unwrap_or_else(|e| panic!("encode `{asm}`: {e}"));
        assert_eq!(bytes.len(), 4, "定宽 4 字节指令：`{asm}`");
        let (back, used) = decode(&bytes).unwrap_or_else(|| panic!("decode `{asm}` 失败"));
        assert_eq!(used, 4, "decode 消费整条指令：`{asm}`");
        assert_eq!(
            back, inst,
            "decode(encode(x)) == x：`{asm}`（原始 {:02x?}）",
            bytes
        );
        // 反汇编文本可再汇编（asm 模板往返）
        let text = disassemble(&back);
        let re =
            assemble(&text).unwrap_or_else(|e| panic!("re-assemble `{text}`（来自 `{asm}`）: {e}"));
        assert_eq!(encode(&re).unwrap(), bytes, "文本往返：`{asm}` → `{text}`");
    }
}

#[test]
fn polymorphic_mov_dispatch_by_operand_kind() {
    // 同一助记符 mov：寄存器形态 → MOV8；立即数形态 → MOV8_R_IMM8
    let r = assemble("mov a1, a2").unwrap();
    let i = assemble("mov a1, 9").unwrap();
    assert!(matches!(r, Inst::Mov8 { .. }), "寄存器形态：{r:?}");
    assert!(matches!(i, Inst::Mov8RImm8 { .. }), "立即数形态：{i:?}");
    // 越界立即数（8 位有符号 → [-128, 127]）必须报错，不静默截断
    assert!(assemble("mov a1, 128").is_err());
    assert!(assemble("mov a1, -129").is_err());
}

// ───────────────── 3. 宿主编译（i8-only 函数）─────────────────

/// 编译 `fn f(a: i8, b: i8) -> i8 { a + b }`（1 字节寄存器池）。
#[test]
fn compile_i8_function_on_one_byte_pool() {
    let ctx = TypeContext::new();
    let sig = FunctionSignature::new(&[(TypeId::I8, "a"), (TypeId::I8, "b")], &[TypeId::I8]);
    let mut b = FunctionBuilder::new("add8_fn", ctx, sig);
    let (entry, params) = b.create_block_with_params(&[(TypeId::I8, "a"), (TypeId::I8, "b")]);
    b.switch_to_block(entry);
    let v = b.iadd(params[0], params[1]);
    b.ret(&[v]);
    let func = b.finish().expect("build");

    let compiler = FunctionCompiler::new(TargetMachine::new());
    let cf = compiler.compile_raw(&func).expect("i8 函数必须可编译");
    eprintln!(
        "demo8 add8 code ({} bytes): {:02x?}",
        cf.code.len(),
        cf.code
    );
    assert!(!cf.code.is_empty(), "生成代码非空");
    assert_eq!(cf.code.len() % 4, 0, "定宽 4 字节指令序列");

    // 机器码必须能被本 ISA 解码回合法指令序列，且含 ADD8 与 RET
    let mut off = 0usize;
    let mut asm: Vec<String> = Vec::new();
    while off < cf.code.len() {
        let (inst, used) = decode(&cf.code[off..])
            .unwrap_or_else(|| panic!("机器码 offset {off} 处无法解码：{:02x?}", cf.code));
        asm.push(disassemble(&inst));
        off += used;
    }
    eprintln!("disasm: {asm:?}");
    let joined = asm.join("\n");
    assert!(joined.contains("add "), "应含 add：{joined}");
    assert!(joined.contains("ret"), "应含 ret：{joined}");
    // 收参/返回值必须经寄存器搬运（@move_args + ret_regs = A0）——只断言
    // "含 add/ret" 会让"参数未搬运/返回值未回写"这类回归溜过去。
    assert!(
        asm.iter().filter(|s| s.starts_with("mov ")).count() >= 2,
        "应含 ≥2 条 mov（收参 + 返回值回写）：{joined}"
    );
    assert!(
        asm.iter().any(|s| s.starts_with("mov A0")),
        "返回值必须回写到 ret_regs[0] = A0：{joined}"
    );
    // 全部寄存器操作数必须是 A0..A7（唯一 1 字节组）——出现别的寄存器名说明
    // 某处仍按写死的类/别名构造寄存器（如 x86 的 RBP/RAX）。
    for tok in joined.split(|c: char| !c.is_ascii_alphanumeric()) {
        let looks_like_reg = tok.len() >= 2
            && tok.chars().next().is_some_and(|c| c.is_ascii_uppercase())
            && tok[1..].chars().all(|c| c.is_ascii_digit());
        if looks_like_reg {
            assert!(tok.starts_with('A'), "非本 ISA 的寄存器名 {tok}：{joined}");
        }
    }
}

/// 开启 IR 优化（O1）后编译 i8 函数：DCE 会把死值墓碑化（`ValueData.ty` =
/// `TypeId::VOID`，见 `dfg.rs`）——值池门必须跳过这类不承载寄存器的值，
/// 否则 1 字节值池的 ISA 上**任何**含死值的函数都会被误拒。
#[test]
fn compile_i8_function_with_opt_level_skips_tombstoned_values() {
    let ctx = TypeContext::new();
    let sig = FunctionSignature::new(&[(TypeId::I8, "a"), (TypeId::I8, "b")], &[TypeId::I8]);
    let mut b = FunctionBuilder::new("opt8_fn", ctx, sig);
    let (entry, params) = b.create_block_with_params(&[(TypeId::I8, "a"), (TypeId::I8, "b")]);
    b.switch_to_block(entry);
    let dead = b.isub(params[0], params[1]); // 结果未使用 → O1 DCE 墓碑化
    let live = b.iadd(params[0], params[1]);
    b.ret(&[live]);
    let _ = dead;
    let func = b.finish().expect("build");

    let compiler = FunctionCompiler::new(TargetMachine::new())
        .with_opt_level(forge_opt::OptimizationLevel::O1);
    let cf = compiler
        .compile_raw(&func)
        .expect("开启 O1 后 i8 函数必须仍可编译（墓碑值不得触发值池门）");
    eprintln!("demo8 O1 code ({} bytes): {:02x?}", cf.code.len(), cf.code);
    assert!(!cf.code.is_empty(), "生成代码非空");
    assert_eq!(cf.code.len() % 4, 0, "定宽 4 字节指令序列");
}

/// 宽类型（i64）在 1 字节值池上 → 编译期 `Unsupported`（点名值池宽度）。
#[test]
fn compile_i64_function_is_rejected_with_clear_error() {
    let ctx = TypeContext::new();
    let sig = FunctionSignature::new(&[(TypeId::I64, "a"), (TypeId::I64, "b")], &[TypeId::I64]);
    let mut b = FunctionBuilder::new("add64_fn", ctx, sig);
    let (entry, params) = b.create_block_with_params(&[(TypeId::I64, "a"), (TypeId::I64, "b")]);
    b.switch_to_block(entry);
    let v = b.iadd(params[0], params[1]);
    b.ret(&[v]);
    let func = b.finish().expect("build");

    let compiler = FunctionCompiler::new(TargetMachine::new());
    let err = compiler
        .compile_raw(&func)
        .expect_err("1 字节值池不得静默承载 i64");
    let msg = format!("{err}");
    eprintln!("i64 拒绝信息：{msg}");
    // 必须命中值池门的**具体**文案（`Unsupported` 单独出现不足以证明是本门
    // 拒绝——别的 Unsupported 也会满足）。
    assert!(msg.contains("值池无法承载"), "必须是值池门拒绝：{msg}");
    assert!(msg.contains("i64"), "必须点名类型：{msg}");
    assert!(
        msg.contains("GPR") || msg.contains("值池"),
        "必须点名值池：{msg}"
    );
}
