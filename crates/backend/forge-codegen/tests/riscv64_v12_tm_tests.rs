//! riscv64_v12 — TargetMachine 接入验证（B2 第一部分：编译链路）。
//!
//! IR（FunctionBuilder）→ lowering → regalloc → frame → encode 全链路，
//! 不执行（执行经 QEMU，见 forge-tests 的 riscv64 runner）。
//! 验证：整数运算/常量/加载存储/帧序言的机器码生成 + decode 回环契约。

use forge_codegen::FunctionCompiler;
use forge_ir::{FunctionBuilder, FunctionSignature, TypeContext, TypeId};

/// 编译 `fn name(a, b) -> ret`（i64 参数）为机器码。
fn compile_binop(
    name: &str,
    build: impl FnOnce(&mut FunctionBuilder, &[forge_ir::Value]) -> forge_ir::Value,
) -> forge_codegen::CompiledFunction {
    let sig = FunctionSignature::new(&[(TypeId::I64, "a"), (TypeId::I64, "b")], &[TypeId::I64]);
    let mut b = FunctionBuilder::new(name, TypeContext::new(), sig);
    let (entry, params) = b.create_block_with_params(&[(TypeId::I64, "a"), (TypeId::I64, "b")]);
    b.switch_to_block(entry);
    let v = build(&mut b, &params);
    b.ret(&[v]);
    let func = b.finish().expect("build");
    let compiler = FunctionCompiler::new(forge_codegen::riscv64_v12::TargetMachine::new());
    compiler.compile_raw(&func).expect("compile")
}

/// 编译无参常量函数 `fn name() -> i64`。
fn compile_const(name: &str, v: i64) -> forge_codegen::CompiledFunction {
    let sig = FunctionSignature::new(&[], &[TypeId::I64]);
    let mut b = FunctionBuilder::new(name, TypeContext::new(), sig);
    b.create_block_here();
    let c = b.iconst_i64(v);
    b.ret(&[c]);
    let func = b.finish().expect("build");
    let compiler = FunctionCompiler::new(forge_codegen::riscv64_v12::TargetMachine::new());
    compiler.compile_raw(&func).expect("compile")
}

/// 32 位指令字查找：全字节中某 4 字节 == 给定小端字。
fn find_word(code: &[u8], w: u32) -> bool {
    code.windows(4)
        .any(|c| u32::from_le_bytes([c[0], c[1], c[2], c[3]]) == w)
}

/// 查找含指定 opcode（低 7 位）的指令字。
fn find_opcode(code: &[u8], opcode: u32) -> bool {
    code.windows(4)
        .any(|c| u32::from_le_bytes([c[0], c[1], c[2], c[3]]) & 0x7F == opcode)
}

#[test]
fn tm_compile_add() {
    // add a, b → ADD（opcode 0x33, funct3=0, funct7=0；指令字低 7 位 = 0x33）
    let cf = compile_binop("add", |b, p| b.iadd(p[0], p[1]));
    eprintln!("add code ({} bytes): {:02x?}", cf.code.len(), cf.code);
    assert!(!cf.code.is_empty());
    // prologue: addi sp, sp, -112（min_frame_bytes=104 覆盖 callee_saved
    // 保存区 + @push_callee 按需保存）→ 指令字 0xF9010113
    assert!(
        find_word(&cf.code, 0xF901_0113),
        "prologue addi sp,sp,-112: {:02x?}",
        cf.code
    );
    // **按需保存**（阶段 G）：callee_saved 只保存实际分配到的 s 系——
    // add(a,b) 参数被 regalloc 分配到 s 系（跨 epilogue j 跳转存活）→
    // 保存 ra/fp + 3 个 s 系（s9/s10/s11，视分配而定）共 ≤5 个 SD。
    // 全量 11 个 s 系时代的 13 个 SD（+ra/fp）断言已失效；此处校验
    // **SD 数量显著少于全量**（按需保存生效）。
    let sd_count = cf
        .code
        .windows(4)
        .filter(|c| u32::from_le_bytes([c[0], c[1], c[2], c[3]]) & 0x7F == 0x23)
        .count();
    assert!(
        (4..=8).contains(&sd_count),
        "按需保存：SD 应在 4-8 之间（ra/fp + 少量 s 系，< 全量 13），实际 {sd_count}: {:02x?}",
        cf.code
    );
    // ADD 指令（opcode 0x33）应存在
    assert!(
        find_opcode(&cf.code, 0x33),
        "应含 ADD opcode 0x33: {:02x?}",
        cf.code
    );
    // epilogue: ret = jalr x0, 0(x1) = 0x0000_8067（小端 67 80 00 00）
    assert!(
        find_word(&cf.code, 0x0000_8067),
        "应含 RET (0x8067): {:02x?}",
        cf.code
    );
}

#[test]
fn tm_compile_addw_width_dispatch() {
    // i32 add → ADDW（opcode 0x3B）；i64 add → ADD（opcode 0x33）
    let sig = FunctionSignature::new(&[(TypeId::I32, "a"), (TypeId::I32, "b")], &[TypeId::I32]);
    let mut b = FunctionBuilder::new("addw", TypeContext::new(), sig);
    let (entry, params) = b.create_block_with_params(&[(TypeId::I32, "a"), (TypeId::I32, "b")]);
    b.switch_to_block(entry);
    let v = b.iadd(params[0], params[1]);
    b.ret(&[v]);
    let func = b.finish().expect("build");
    let compiler = FunctionCompiler::new(forge_codegen::riscv64_v12::TargetMachine::new());
    let cf = compiler.compile_raw(&func).expect("compile addw");
    assert!(
        find_opcode(&cf.code, 0x3B),
        "i32 Iadd 应选 ADDW (opcode 0x3B): {:02x?}",
        cf.code
    );
    // i64 对比：ADD opcode 0x33
    let cf64 = compile_binop("add64", |b, p| b.iadd(p[0], p[1]));
    assert!(
        find_opcode(&cf64.code, 0x33),
        "i64 Iadd 应选 ADD (0x33): {:02x?}",
        cf64.code
    );
}

#[test]
fn tm_compile_const_lui_addi() {
    // 常量 42 → lui hi20(0) + addi lo12(42)；42 在 addi 范围内
    let cf = compile_const("c42", 42);
    eprintln!("const42 code ({} bytes): {:02x?}", cf.code.len(), cf.code);
    assert!(
        find_opcode(&cf.code, 0x37),
        "应含 LUI (opcode 0x37): {:02x?}",
        cf.code
    );
    assert!(
        find_opcode(&cf.code, 0x13),
        "应含 ADDI (opcode 0x13): {:02x?}",
        cf.code
    );
    // 大常量：0x12345678 → lui 0x12345 + addi 0x678（低 12 位 < 0x800 无需修正）
    let cfb = compile_const("big", 0x1234_5678);
    assert!(
        find_opcode(&cfb.code, 0x37),
        "big: 应含 LUI: {:02x?}",
        cfb.code
    );
    // 负常量 -1：lui 0xFFFFF + addi -1（低 12 位 0xFFF >= 0x800 → 修正为 -1）
    let cfn = compile_const("neg", -1);
    assert!(
        find_opcode(&cfn.code, 0x37),
        "neg: 应含 LUI: {:02x?}",
        cfn.code
    );
}

#[test]
fn tm_decode_roundtrip() {
    use forge_codegen::riscv64_v12::{decode, encode};
    // 对编译产物逐 4 字节 decode，decode(encode(x)) == x（硬契约）
    for (name, cf) in [
        ("add", compile_binop("add", |b, p| b.iadd(p[0], p[1]))),
        ("const42", compile_const("c42", 42)),
        ("constbig", compile_const("big", 0x1234_5678)),
    ] {
        let mut off = 0;
        let mut roundtripped = 0;
        while off + 4 <= cf.code.len() {
            let word = &cf.code[off..off + 4];
            let (d, n) =
                decode(word).unwrap_or_else(|| panic!("{name} decode @{off}: {word:02x?}"));
            assert_eq!(n, 4, "{name} @{off}: consumed != 4");
            assert_eq!(
                encode(&d).unwrap(),
                word,
                "{name} @{off}: decode→encode 字节不一致"
            );
            off += n;
            roundtripped += 1;
        }
        assert!(roundtripped >= 3, "{name}: 至少 3 条指令可回环");
    }
}
