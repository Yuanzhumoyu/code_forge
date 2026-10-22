//! `int_width` 的单元测试（从主文件搬出，见 `file_size_ratchet` 的规模上限）。

use super::*;
use crate::pipeline::legalize::{LegalizeCtx, LegalizeDriver};
use forge_ir::ir::builder::FunctionBuilder;
use forge_ir::ir::types::{FunctionSignature, TypeContext};

fn run_pass(func: &mut Function) -> ChangeSet {
    let machine = crate::arch::x86::TargetMachine::new();
    let mut lctx = LegalizeCtx::new(&machine, "win64".to_string());
    // 只跑本 pass：`scalar()` 批里 `wide_mem` 在前，会先把宽值搬进内存槽，
    // 于是 R3 再也看不到宽值（那是生产路径的正确行为，这里要单独测 R3 本身）。
    let driver = LegalizeDriver {
        passes: vec![Box::new(super::IntWidthPass)],
    };
    driver
        .run(func, &mut lctx)
        .expect("整数位宽补位 pass 必须成功")
}

/// **R2 就近升位**：`i3` 加法 ⇒ `zext`×2 + `i8` 加法 + `ireduce → i3`；
/// 且原链路（ret 的操作数）拿到的是**截回后的 i3 值**。
#[test]
fn r2_widens_i3_add_and_truncates_back() {
    let tc = TypeContext::new();
    let i3 = tc.int_ty(3);
    let sig = FunctionSignature::new(&[], &[i3]);
    let tcheck = tc.clone();
    let mut fb = FunctionBuilder::new("w3_add", tc, sig);
    fb.create_block_here();
    let a = fb.const_int(5, i3);
    let b = fb.const_int(3, i3);
    let sum = fb.iadd(a, b);
    fb.ret(&[sum]);
    let mut func = fb.finish().expect("i3 加法应能建起来");

    let cs = run_pass(&mut func);
    assert!(cs.total() > 0, "非原生位宽必须让 pass 动手：{cs:?}");

    let ops: Vec<Opcode> = func.dfg.insts().map(|(_, d)| d.opcode).collect();
    assert_eq!(
        ops.iter().filter(|o| **o == Opcode::Uextend).count(),
        2,
        "两个 i3 操作数各升位一次：{ops:?}"
    );
    assert_eq!(
        ops.iter().filter(|o| **o == Opcode::Ireduce).count(),
        1,
        "结果截回一次：{ops:?}"
    );
    // 加法本体现在应是 i8（8 ∈ x86 的 int_widths）。
    let add = func
        .dfg
        .insts()
        .find(|(_, d)| d.opcode == Opcode::Iadd)
        .expect("加法还在");
    let r = add.1.results[0];
    let ty = func.dfg.value_type(r).expect("结果有类型");
    assert_eq!(
        tcheck.borrow().scalar_bits(ty),
        Some(8),
        "加法结果应升到 i8"
    );
    // ret 的操作数应是 ireduce 的结果（i3），而不是 i8 的加法结果。
    let ret_operand = func
        .dfg
        .insts()
        .find(|(_, d)| d.opcode == Opcode::Ret)
        .and_then(|(_, d)| d.operands.first().copied())
        .expect("ret 有操作数");
    let ret_ty = func.dfg.value_type(ret_operand).expect("ret 操作数有类型");
    assert_eq!(tcheck.borrow().scalar_bits(ret_ty), Some(3), "ret 仍拿 i3");
}

/// 有符号运算用 `sext`（`sdiv i3`），无符号用 `zext`。
#[test]
fn r2_uses_sext_for_signed_ops() {
    let tc = TypeContext::new();
    let i3 = tc.int_ty(3);
    let sig = FunctionSignature::new(&[], &[i3]);
    let mut fb = FunctionBuilder::new("w3_sdiv", tc, sig);
    fb.create_block_here();
    let a = fb.const_int(6, i3);
    let b = fb.const_int(2, i3);
    let q = fb.sdiv(a, b);
    fb.ret(&[q]);
    let mut func = fb.finish().expect("i3 sdiv 应能建起来");

    run_pass(&mut func);
    let ops: Vec<Opcode> = func.dfg.insts().map(|(_, d)| d.opcode).collect();
    assert!(
        ops.contains(&Opcode::Sextend),
        "有符号除法必须用 sext 升位：{ops:?}"
    );
    assert!(!ops.contains(&Opcode::Uextend), "不应混入 zext：{ops:?}");
}

/// **幂等**：跑第二遍不再产生任何改动（位宽已是原生、`ireduce` 不在可升位集合里）。
#[test]
fn int_width_pass_is_idempotent() {
    let tc = TypeContext::new();
    let i7 = tc.int_ty(7);
    let sig = FunctionSignature::new(&[], &[i7]);
    let mut fb = FunctionBuilder::new("w7_idem", tc, sig);
    fb.create_block_here();
    let a = fb.const_int(70, i7);
    let b = fb.const_int(9, i7);
    let s = fb.iadd(a, b);
    fb.ret(&[s]);
    let mut func = fb.finish().expect("i7 加法应能建起来");

    assert!(run_pass(&mut func).total() > 0, "第一遍必须动手");
    let second = run_pass(&mut func);
    assert_eq!(second.total(), 0, "第二遍不应再有改动：{second:?}");
}
