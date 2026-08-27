//! demo_v12 — TargetMachine 接入验证（B3）。
//!
//! 验证 demo_v12 从"自包含演示模块"升级为完整 TargetMachine：
//! IR（FunctionBuilder）→ lowering → regalloc → frame → encode 全链路，
//! 且宽度分发在 lowering 层生效（i16/i32/i64 的 Iadd 分别命中
//! ADD16/ADD32/ADD64——同一条 lowering 模板 "add {out}, {0}, {1}"）。
//!
//! 注意：demo 的 [abi.frame].sp = X7、fp = X6 为演示占位（无 callee-saved、
//! frame_size=0 时不发射帧指令）；[emit].epilogue_label = false（无 JMP
//! 指令，return block 直接 fall-through 到尾声 RET）。

use forge_codegen::FunctionCompiler;
use forge_ir::{FunctionBuilder, FunctionSignature, TypeContext, TypeId};

/// 构建 `fn name(a, b) -> ret` 并编译出机器码。参数类型 = 返回类型
/// （i16/i32/i64 三宽度各自独立验证 lowering 宽度分发）。
fn compile_binop(
    name: &str,
    ret_ty: TypeId,
    build: impl FnOnce(&mut FunctionBuilder, &[forge_ir::Value]) -> forge_ir::Value,
) -> forge_codegen::CompiledFunction {
    let sig = FunctionSignature::new(&[(ret_ty, "a"), (ret_ty, "b")], &[ret_ty]);
    let mut b = FunctionBuilder::new(name, TypeContext::new(), sig);
    let (entry, params) = b.create_block_with_params(&[(ret_ty, "a"), (ret_ty, "b")]);
    b.switch_to_block(entry);
    let v = build(&mut b, &params);
    b.ret(&[v]);
    let func = b.finish().expect("build");
    let compiler = FunctionCompiler::new(forge_codegen::demo_v12::TargetMachine::new());
    compiler.compile_raw(&func).expect("compile")
}

/// 全字节中查找给定 32 位小端指令字（opcode + 3×3 位寄存器 + 16 位 imm）。
fn find_word(code: &[u8], w: u32) -> bool {
    code.windows(4)
        .any(|c| u32::from_le_bytes([c[0], c[1], c[2], c[3]]) == w)
}

#[test]
fn tm_compile_minimal() {
    // fn add64(a, b) -> i64 { a + b }——应含 ADD64 (0x12) 与 RET (0x01)
    let cf = compile_binop("add64", TypeId::I64, |b, p| b.iadd(p[0], p[1]));
    eprintln!("add64 code ({} bytes): {:02x?}", cf.code.len(), cf.code);
    assert!(!cf.code.is_empty(), "生成代码非空");
    // ADD64：opcode 0x12、rd=rs1=rs2=0（占位 0，regalloc 回填后可变化——
    // 这里只断言 opcode 低字节存在于某处）
    assert!(
        cf.code.contains(&0x12),
        "应含 ADD64 opcode 0x12: {:02x?}",
        cf.code
    );
    // RET：word 位域 = 0x01（小端第 1 字节 0x01、其余 0）
    assert!(
        find_word(&cf.code, 0x0000_0001),
        "应含 RET (word=1): {:02x?}",
        cf.code
    );
}

#[test]
fn tm_compile_width_dispatch() {
    // i16 add → ADD16 (0x10)；i32 add → ADD32 (0x11)；i64 add → ADD64 (0x12)
    let c16 = compile_binop("add16", TypeId::I16, |b, p| b.iadd(p[0], p[1]));
    let c32 = compile_binop("add32", TypeId::I32, |b, p| b.iadd(p[0], p[1]));
    let c64 = compile_binop("add64", TypeId::I64, |b, p| b.iadd(p[0], p[1]));
    assert!(
        c16.code.contains(&0x10),
        "i16 Iadd 应选 ADD16 (0x10): {:02x?}",
        c16.code
    );
    assert!(
        c32.code.contains(&0x11),
        "i32 Iadd 应选 ADD32 (0x11): {:02x?}",
        c32.code
    );
    assert!(
        c64.code.contains(&0x12),
        "i64 Iadd 应选 ADD64 (0x12): {:02x?}",
        c64.code
    );
}

#[test]
fn tm_decode_roundtrip() {
    // 对编译产物逐 4 字节 decode，decode(encode(x)) == x（硬契约）
    use forge_codegen::demo_v12::{decode, encode};
    for name in ["add16", "add32", "add64"] {
        let ty = match name {
            "add16" => TypeId::I16,
            "add32" => TypeId::I32,
            _ => TypeId::I64,
        };
        let cf = compile_binop(name, ty, |b, p| b.iadd(p[0], p[1]));
        let mut off = 0;
        let mut roundtripped = 0;
        while off + 4 <= cf.code.len() {
            let word = &cf.code[off..off + 4];
            let (d, n) = decode(word).unwrap_or_else(|| panic!("decode @{off}: {word:02x?}"));
            assert_eq!(n, 4, "@{off}: consumed != 4");
            assert_eq!(
                encode(&d).unwrap(),
                word,
                "@{off}: decode→encode 字节不一致 (fn {name})"
            );
            off += n;
            roundtripped += 1;
        }
        assert!(roundtripped >= 2, "{name}: 至少 2 条指令可回环");
    }
}
