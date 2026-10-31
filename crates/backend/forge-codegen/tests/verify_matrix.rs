//! v31：**三 ISA 的合法化后校验矩阵**——同一批 IR 在 x86/arm64/riscv64 上都要通过
//! 完整 `Verifier`（`CompileOptions::verify(On)`）。
//!
//! 覆盖三类会触发 legalize 的形态：
//! - 标量算术（基线）；
//! - 动态 GEP（`add ptr, i64`，`binop_ptr_off`）；
//! - i128 宽值（`wide_split` 拆肢）。

use forge_codegen::CompileOptions;
use forge_codegen::CompileSession;
use forge_codegen::VerifyLevel;
use forge_codegen::machine::target::TargetMachine;
use forge_ir::ir::builder::FunctionBuilder;
use forge_ir::ir::types::{FunctionSignature, TypeContext};
use forge_ir::{Function, TypeId};

fn options() -> CompileOptions {
    CompileOptions::off().with_verify(VerifyLevel::On)
}

fn scalar_fn() -> Function {
    let i64 = TypeId::I64;
    let sig = FunctionSignature::new(&[(i64, "a"), (i64, "b")], &[i64]);
    let mut b = FunctionBuilder::new("scalar", TypeContext::new(), sig);
    let (blk, p) = b.create_block_with_params(&[(i64, "a"), (i64, "b")]);
    b.switch_to_block(blk);
    let sum = b.iadd(p[0], p[1]);
    b.ret(&[sum]);
    b.finish().expect("build scalar")
}

fn gep_fn() -> Function {
    let i64 = TypeId::I64;
    let sig = FunctionSignature::new(&[(i64, "base"), (i64, "idx")], &[i64]);
    let mut b = FunctionBuilder::new("gep_dyn", TypeContext::new(), sig);
    let (blk, p) = b.create_block_with_params(&[(i64, "base"), (i64, "idx")]);
    b.switch_to_block(blk);
    let addr = b.inttoptr(p[0], TypeId::PTR);
    let slot = b.gep(addr, &[p[1]], i64);
    let v = b.load(slot, i64);
    b.ret(&[v]);
    b.finish().expect("build gep_dyn")
}

fn wide_fn() -> Function {
    // 宽值走**函数内部**（常量 + 加法 + 截断回 i64）：不碰 i128 入口参数/返回的
    // ABI 边界（那是另一条 `WIDE_INT_SHAPE`，不属于本矩阵要验证的 legalize 形态）。
    let i128 = TypeId::I128;
    let i64 = TypeId::I64;
    let sig = FunctionSignature::new(&[], &[i64]);
    let mut b = FunctionBuilder::new("wide_add", TypeContext::new(), sig);
    b.create_block_here();
    let a = b.const_int(1i128, i128);
    let c = b.const_int(2i128, i128);
    let sum = b.iadd(a, c);
    let narrowed = b.ireduce(sum, i64);
    b.ret(&[narrowed]);
    b.finish().expect("build wide_add")
}

fn check_all<M: TargetMachine + Clone>(tm: M, isa: &str) {
    for (name, func) in [
        ("scalar", scalar_fn()),
        ("gep", gep_fn()),
        ("wide", wide_fn()),
    ] {
        let cf = CompileSession::new(tm.clone())
            .with_options(options())
            .compile(&func)
            .unwrap_or_else(|e| panic!("[{isa}/{name}] 合法化后校验失败：{e}"));
        assert!(!cf.code.is_empty(), "[{isa}/{name}] 产物为空");
    }
}

#[test]
fn x86_legalize_verifies() {
    check_all(forge_codegen::x86::TargetMachine::new(), "x86_64");
}

#[test]
fn arm64_legalize_verifies() {
    check_all(forge_codegen::arm64::TargetMachine::new(), "aarch64");
}

#[test]
fn riscv64_legalize_verifies() {
    check_all(forge_codegen::riscv64::TargetMachine::new(), "riscv64");
}
