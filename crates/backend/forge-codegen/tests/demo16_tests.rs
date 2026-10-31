//! demo16 — 2 字节（16 位）寄存器 ISA 回归（v28 M4）。
//!
//! 与 demo8 同一套宽度机制，只把寄存器/栈槽单位抬到 2 字节；证明 16 位值能
//! 走通 DSL 生成 + 宿主流水线，而超过值池宽度的 i32 被编译期拒绝。

mod common;

use common::demo16::{TargetMachine, decode, disassemble};
use forge_codegen::CompileSession;
use forge_codegen::TargetMachine as TargetMachineTrait;
use forge_ir::{FunctionBuilder, FunctionSignature, RegClass, TypeContext, TypeId};

#[test]
fn metadata_uses_two_byte_class_and_slot() {
    let tm = TargetMachine::new();
    assert_eq!(
        tm.reg_info().class_for_type(TypeId::PTR),
        Some(RegClass::GPR(16)),
        "地址类必须来自 gpr16（类宽度按位），而不是写死的 64 位"
    );
    assert_eq!(
        tm.reg_info().class_for_type(TypeId::I16),
        Some(RegClass::GPR(16))
    );
}

#[test]
fn compile_i16_function_on_two_byte_pool() {
    let ctx = TypeContext::new();
    let sig = FunctionSignature::new(&[(TypeId::I16, "a"), (TypeId::I16, "b")], &[TypeId::I16]);
    let mut b = FunctionBuilder::new("add16_fn", ctx, sig);
    let (entry, params) = b.create_block_with_params(&[(TypeId::I16, "a"), (TypeId::I16, "b")]);
    b.switch_to_block(entry);
    let v = b.iadd(params[0], params[1]);
    b.ret(&[v]);
    let func = b.finish().expect("build");

    common::ensure_demo_conventions();
    let compiler = CompileSession::new(TargetMachine::new());
    let cf = compiler.compile(&func).expect("i16 函数必须可编译");
    assert!(!cf.code.is_empty(), "生成代码非空");
    assert_eq!(cf.code.len() % 4, 0, "定宽 4 字节指令序列");

    let mut off = 0usize;
    let mut asm = Vec::new();
    while off < cf.code.len() {
        let (inst, used) = decode(&cf.code[off..])
            .unwrap_or_else(|| panic!("机器码 offset {off} 处无法解码：{:02x?}", cf.code));
        asm.push(disassemble(&inst));
        off += used;
    }
    let joined = asm.join("\n");
    assert!(joined.contains("add "), "应含 add：{joined}");
    assert!(joined.contains("ret"), "应含 ret：{joined}");
    assert!(
        asm.iter().filter(|s| s.starts_with("mov ")).count() >= 2,
        "应含 ≥2 条 mov（收参 + 返回值回写）：{joined}"
    );
}

#[test]
fn compile_i32_function_is_rejected_by_value_pool() {
    let ctx = TypeContext::new();
    let sig = FunctionSignature::new(&[(TypeId::I32, "a"), (TypeId::I32, "b")], &[TypeId::I32]);
    let mut b = FunctionBuilder::new("add32_fn", ctx, sig);
    let (entry, params) = b.create_block_with_params(&[(TypeId::I32, "a"), (TypeId::I32, "b")]);
    b.switch_to_block(entry);
    let v = b.iadd(params[0], params[1]);
    b.ret(&[v]);
    let func = b.finish().expect("build");

    common::ensure_demo_conventions();
    let err = CompileSession::new(TargetMachine::new())
        .compile(&func)
        .expect_err("i32 超出 2 字节值池，必须 fail-closed");
    let msg = format!("{err}");
    assert!(
        msg.contains("Unsupported") || msg.contains("GPR") || msg.contains("class"),
        "拒绝原因应指向值池/寄存器类：{msg}"
    );
}
