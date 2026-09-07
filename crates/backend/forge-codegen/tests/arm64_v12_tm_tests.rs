//! arm64_v12 — TargetMachine 接入验证（B2：编译链路，A64）。
//!
//! IR（FunctionBuilder）→ lowering → regalloc → frame → encode 全链路。
//! 本机无 A64 执行环境（无 QEMU runner）——验证机器码**结构**：
//! 帧序言（sub sp / stur x30,x29 / fp 建立）、尾声（ldur 恢复 + ret）、
//! 宽度分派（ADDREGX vs ADDREGW）、常量（MOVZX）以及全字节 decode→encode
//! 回环契约。golden 参考：LLVM clang --target=aarch64-none-elf oracle 常量
//!（tests/arm64_v12_tests.rs）。
//!
//! 帧布局镜像 riscv64_v12 的 fp-inside：sub sp,sp,#96（min_frame=96）→
//! stur x30,[sp,#88] / stur x29,[sp,#80] → add x29,sp,#96 → （按需保存
//! callee-saved）→ move_args → body → 尾声 fall-through（epilogue_label=
//! false——P1 仅单 return block 函数）。

use forge_codegen::FunctionCompiler;
use forge_ir::{FunctionBuilder, FunctionSignature, TypeContext, TypeId};

/// 编译 `fn name(a, b) -> i64` 为 A64 机器码。
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
    let compiler = FunctionCompiler::new(forge_codegen::arm64_v12::TargetMachine::new());
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
    let compiler = FunctionCompiler::new(forge_codegen::arm64_v12::TargetMachine::new());
    compiler.compile_raw(&func).expect("compile")
}

/// 找最后一个 4 字节字 == 给定小端字（尾声 RET 应在函数末尾）。
fn last_word(code: &[u8]) -> u32 {
    assert!(code.len() >= 4, "code too short: {code:02x?}");
    let c = &code[code.len() - 4..];
    u32::from_le_bytes([c[0], c[1], c[2], c[3]])
}

/// 任意指令字满足顶层 8 位 == top8（A64 顶层 [31:24] 对大多数族恒定量）。
fn has_top8(code: &[u8], top8: u32) -> bool {
    code.windows(4)
        .any(|c| (u32::from_le_bytes([c[0], c[1], c[2], c[3]]) & 0xFF00_0000) == top8 << 24)
}

#[test]
fn tm_compile_add_prologue_epilogue() {
    let cf = compile_binop("add", |b, p| b.iadd(p[0], p[1]));
    eprintln!("add code ({} bytes): {:02x?}", cf.code.len(), cf.code);
    assert!(!cf.code.is_empty());
    // 序言：sub sp, sp, #N（rd=31、rn=31、顶层 0xD1）——N ≥ 96 的 16 倍数
    assert!(
        cf.code.windows(4).any(|c| {
            let w = u32::from_le_bytes([c[0], c[1], c[2], c[3]]);
            (w & 0xFF00_0000) == 0xD100_0000 && (w & 0x1F) == 31 && ((w >> 5) & 0x1F) == 31
        }),
        "应含 sub sp,sp,#N: {:02x?}",
        cf.code
    );
    // 帧顶保存：stur x30/x29 到 [sp + frame - 8/16]（顶层 0xF8、opc2=0 store）
    let stur_count = cf
        .code
        .windows(4)
        .filter(|c| {
            let w = u32::from_le_bytes([c[0], c[1], c[2], c[3]]);
            (w & 0xFF00_0000) == 0xF800_0000 && ((w >> 22) & 3) == 0
        })
        .count();
    assert!(
        (2..=14).contains(&stur_count),
        "序言/尾声应含 stur 保存 x30/x29（按需保存 callee-saved 会追加）: {stur_count} {:02x?}",
        cf.code
    );
    // fp 建立：add x29, sp, #N（顶层 0x91、rd=29、rn=31）
    assert!(
        cf.code.windows(4).any(|c| {
            let w = u32::from_le_bytes([c[0], c[1], c[2], c[3]]);
            (w & 0xFF00_0000) == 0x9100_0000 && (w & 0x1F) == 29 && ((w >> 5) & 0x1F) == 31
        }),
        "应含 add x29,sp,#N: {:02x?}",
        cf.code
    );
    // ADDREGX（add x_, x_, x_：顶层 0x8B）应存在
    assert!(
        has_top8(&cf.code, 0x8B),
        "应含 add(reg) X 形式: {:02x?}",
        cf.code
    );
    // 尾声最后一个字 = RET（D65F03C0）——return block 直接 fall-through
    assert_eq!(
        last_word(&cf.code),
        0xD65F_03C0,
        "尾声应以 RET 结尾: {:02x?}",
        cf.code
    );
}

#[test]
fn tm_compile_addw_width_dispatch() {
    // i32 add → ADDREGW（顶层 0x0B）；i64 add → ADDREGX（顶层 0x8B）
    let sig = FunctionSignature::new(&[(TypeId::I32, "a"), (TypeId::I32, "b")], &[TypeId::I32]);
    let mut b = FunctionBuilder::new("addw", TypeContext::new(), sig);
    let (entry, params) = b.create_block_with_params(&[(TypeId::I32, "a"), (TypeId::I32, "b")]);
    b.switch_to_block(entry);
    let v = b.iadd(params[0], params[1]);
    b.ret(&[v]);
    let func = b.finish().expect("build");
    let compiler = FunctionCompiler::new(forge_codegen::arm64_v12::TargetMachine::new());
    let cf = compiler.compile_raw(&func).expect("compile addw");
    eprintln!("addw code ({} bytes): {:02x?}", cf.code.len(), cf.code);
    assert!(
        has_top8(&cf.code, 0x0B),
        "i32 Iadd 应选 ADDREGW (顶层 0x0B): {:02x?}",
        cf.code
    );
    // i64 对比：ADDREGX 顶层 0x8B
    let cf64 = compile_binop("add64", |b, p| b.iadd(p[0], p[1]));
    assert!(
        has_top8(&cf64.code, 0x8B),
        "i64 Iadd 应选 ADDREGX (0x8B): {:02x?}",
        cf64.code
    );
}

#[test]
fn tm_compile_const_movz() {
    // 常量 42（0..0xFFFF 单 MOVZ）：movz x?, #42（顶层 0xD2）
    let cf = compile_const("c42", 42);
    eprintln!("const42 code ({} bytes): {:02x?}", cf.code.len(), cf.code);
    assert!(
        has_top8(&cf.code, 0xD2),
        "应含 MOVZ (顶层 0xD2): {:02x?}",
        cf.code
    );
    // 0 常量同样走 MOVZ #0
    let cf0 = compile_const("c0", 0);
    assert!(
        has_top8(&cf0.code, 0xD2),
        "const0: 应含 MOVZ: {:02x?}",
        cf0.code
    );
    // 上界 0xFFFF（imm16u 满值）
    let cfm = compile_const("cmax", 0xFFFF);
    assert!(
        has_top8(&cfm.code, 0xD2),
        "const0xFFFF: 应含 MOVZ: {:02x?}",
        cfm.code
    );
}

#[test]
fn tm_compile_sub_mul() {
    // sub a,b → SUBREGX（顶层 0xCB）；mul a,b → MULX（顶层 0x9B）
    let cfs = compile_binop("sub", |b, p| b.isub(p[0], p[1]));
    assert!(
        has_top8(&cfs.code, 0xCB),
        "Isub 应选 SUBREGX (0xCB): {:02x?}",
        cfs.code
    );
    assert_eq!(last_word(&cfs.code), 0xD65F_03C0, "sub: RET 结尾");
    let cfm = compile_binop("mul", |b, p| b.imul(p[0], p[1]));
    assert!(
        has_top8(&cfm.code, 0x9B),
        "Imul 应选 MULX (0x9B): {:02x?}",
        cfm.code
    );
}

#[test]
fn tm_compile_const_large_seq() {
    // P3① 大立即数：|v| ≥ 0x10000 → 全片序列（X 恒 4 条）：
    // movz x, #f3, lsl#48（顶层 0xD2、hw=3）+ movk x, #f2/f1/f0
    // （顶层 0xF2、hw=2/1/0）——movz 高片清零其它，movk 依 hw 降序覆写，
    // 任意 64 位位型（含 i64 负大值的两补码位型）可构造。
    let cf = compile_const("cbig", -1_000_000_007i64);
    eprintln!("const -1e9-7 code: {:02x?}", cf.code);
    let count_top8 = |top: u32| {
        cf.code
            .windows(4)
            .filter(|c| {
                (u32::from_le_bytes([c[0], c[1], c[2], c[3]]) & 0xFF00_0000) == top << 24
            })
            .count()
    };
    // -1_000_000_007 = 0xFFFF_FFFF_C465_35F9 → f3=0xFFFF f2=0xFFFF
    // f1=0xC465 f0=0x35F9：movz×1（hw3）+ movk×3
    assert_eq!(count_top8(0xD2), 1, "应恰一条 movz(hw3 高片): {:02x?}", cf.code);
    assert_eq!(count_top8(0xF2), 3, "应恰三条 movk(hw2/hw1/hw0): {:02x?}", cf.code);
    assert_eq!(count_top8(0x92), 0, "大值负常量不应走 movn 单条: {:02x?}", cf.code);
    // 上界内负值 -42 仍走 movn 单条（顶层 0x92）
    let cfn = compile_const("cneg42", -42);
    assert!(
        has_top8(&cfn.code, 0x92),
        "-42 应走 movn 单条 (顶层 0x92): {:02x?}",
        cfn.code
    );
}

#[test]
fn tm_decode_roundtrip() {
    use forge_codegen::arm64_v12::{decode, encode};
    // 对编译产物逐 4 字节 decode，decode(encode(x)) == x（硬契约）
    for (name, cf) in [
        ("add", compile_binop("add", |b, p| b.iadd(p[0], p[1]))),
        ("sub", compile_binop("sub", |b, p| b.isub(p[0], p[1]))),
        ("mul", compile_binop("mul", |b, p| b.imul(p[0], p[1]))),
        ("const42", compile_const("c42", 42)),
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
        assert!(roundtripped >= 6, "{name}: 至少 6 条指令可回环");
    }
}
