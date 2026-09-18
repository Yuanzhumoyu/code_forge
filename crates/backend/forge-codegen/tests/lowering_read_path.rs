//! lowering / 编译管线的**类型快照预算**守卫（v3 S3 余项①）。
//!
//! 背景：`LowerCtx` 曾经只带 `Option<TypeContext>`，生成的 lowering（`forge-dsl` 的
//! `quote!` 模板）在**每条指令**上 `tc.borrow()`（11 处），属性计算还各调一次
//! `is_vector`/`size_bytes`/`scalar_bits` 这类"一次性读封装"（每个又是一次取锁）；
//! 宿主的宽向量门（`compiler.rs`）也在"每指令 × 每类型"上各取一次锁。
//!
//! 现在：`LowerCtx.type_store: Option<TypeStoreRef>`（lowering 入口取**一次**快照，
//! lowering 期间类型表只读），模板与宿主门都改读这份快照。本文件把这条纪律钉成
//! **可测不变量**：编译一个函数的快照次数**不随指令数增长**。
//!
//! 度量用 `TypeContext::debug_read_count`（仅 `debug_assertions`）——本文件整块随
//! `cfg(debug_assertions)` 编译（release 下为空，与 forge-ir 的同名守卫一致）。
#![cfg(debug_assertions)]

mod common;

use common::demo8_v12::TargetMachine;
use forge_codegen::FunctionCompiler;
use forge_ir::TypeId;
use forge_ir::ir::builder::FunctionBuilder;
use forge_ir::ir::function::Function;
use forge_ir::ir::types::{FunctionSignature, TypeContext};

/// `fn f(a: i8) -> i8 { a + a + … }`（`extra` 条 `iadd`）。
///
/// 用 demo8（1 字节寄存器池）夹具：它是**库外**测试夹具，编译路径与发行后端一致。
fn build(ctx: &TypeContext, extra: usize) -> Function {
    let sig = FunctionSignature::new(&[(TypeId::I8, "a")], &[TypeId::I8]);
    let mut b = FunctionBuilder::new("snap", ctx.clone(), sig);
    let (entry, params) = b.create_block_with_params(&[(TypeId::I8, "a")]);
    b.switch_to_block(entry);
    let mut acc = params[0];
    for _ in 0..extra {
        acc = b.iadd(acc, params[0]);
    }
    b.ret(&[acc]);
    b.finish().expect("build")
}

/// 编译一次函数：返回 `(类型快照次数, COW 克隆次数)`。计数在**编译前**清零，
/// 因此只统计编译管线（建 IR 阶段的 intern 不计）。
fn compile_counts(extra: usize) -> (usize, usize) {
    let ctx = TypeContext::new();
    let func = build(&ctx, extra);
    let compiler = FunctionCompiler::new(TargetMachine::new());
    ctx.debug_reset_read_count();
    ctx.debug_reset_cow_clone_count();
    let cf = compiler.compile_raw(&func).expect("编译应成功");
    assert!(!cf.code.is_empty(), "生成代码非空");
    (ctx.debug_read_count(), ctx.debug_cow_clone_count())
}

/// 快照次数**不随指令数增长**（改前：1 条 15 次、80 条 252 次 ≈ 每条指令 3 次）。
///
/// 负向验证（本切片实测）：把 `compiler.rs` 的宽向量门改回"每指令 `types.borrow()`"
/// ⇒ 1 条 10 次 vs 80 条 252+ 次，本用例 FAILED。
#[test]
fn compile_type_snapshots_do_not_scale_with_ir_size() {
    let (small, _) = compile_counts(1);
    let (large, _) = compile_counts(80);
    assert_eq!(
        small, large,
        "编译的类型快照次数不得随 IR 规模增长（1 条指令 {small} 次 vs 80 条 {large} 次）"
    );
    assert!(
        small <= 16,
        "每次编译应是常数级快照（实测 {small} 次；改前 1 条指令就已 15 次、80 条 252 次）"
    );
}

/// 编译期间不得出现"快照跨 intern"（否则每次写都要克隆整表）。
#[test]
fn compile_never_holds_a_snapshot_across_interning() {
    let (_, cows) = compile_counts(40);
    assert_eq!(
        cows, 0,
        "编译管线不得持快照去 intern 类型（实测 {cows} 次整表克隆）"
    );
}

/// **源码级**守卫：lowering 生成器模板不得再出现 `type_ctx` / `.borrow()`
/// （v3 S3 余项①：模板改读 `LowerCtx::type_store` 快照）。
///
/// 运行时用例只能证明"次数不随规模增长"，这条把**做法**也钉住：任何人把
/// `ctx.type_ctx` 或逐指令取锁写回模板，这里立刻红。
#[test]
fn lowering_templates_stay_on_the_snapshot() {
    let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("..")
        .join("..")
        .join("..");
    for rel in [
        "crates/frontend/forge-dsl/src/v12/codegen/lowering.rs",
        "crates/frontend/forge-dsl/src/v12/codegen/integration.rs",
    ] {
        let path = root.join(rel);
        let text = std::fs::read_to_string(&path)
            .unwrap_or_else(|e| panic!("读不到 {}：{e}", path.display()));
        for bad in ["type_ctx", ".borrow()"] {
            assert!(
                !text.contains(bad),
                "{rel} 里仍有 `{bad}`——lowering 模板必须用 `ctx.type_store` 快照\
                 （lowering 入口取一次；见 v3 S3 余项①）"
            );
        }
        assert!(
            text.contains("ctx.type_store"),
            "{rel} 应经 `ctx.type_store` 读类型（实测没有出现）"
        );
    }
}
