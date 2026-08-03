//! Compilation pipeline performance benchmarks.
//!
//! Benchmarks covering the full codegen pipeline:
//! - IR construction (building complex functions programmatically)
//! - IR text parsing (LLVM IR syntax via logos+lalrpop: define/@/typed operands)
//! - Optimization pipeline (individual passes + O0/O1/O2/O3 pipelines)
//! - Pipeline pass breakdown (per-pass cost within O1/O2/O3)
//! - IR verification (forge_ir::verify::Verifier)
//! - Module-level compilation (cross-function call relocation) and IPA passes
//!   with a real function table
//! - Code generation (lowering + register allocation + emit for x86_64)
//! - End-to-end compilation (optimize + compile, and JIT execution)
//! - Throughput (parameterized scaling with function size)
//! - Code size metrics (compiled byte counts)
//! - Comparison report (structured opt/codegen/size data per test case)
//!
//! Run with: `cargo bench`
//! Run a single group: `cargo bench -- optimizations`
//!
//! Note: `end_to_end/e2e_jit_execute` currently hangs in release builds (JIT
//! correctness bug — see `cargo test --release --test jit_integration`); run
//! the rest with:
//!   cargo bench --bench compile_bench -- '^(ir_build|optimizations|pipeline_breakdown|codegen|verify|module|throughput|code_size|comparison|end_to_end/e2e_compile_)'
//!
//! Known frontend limitation surfaced by these benchmarks: parsing a large
//! single-block instruction stream was ~O(n³) in the forge-grammar lexer
//! (2 insts ≈ 6 ms, 16 ≈ 3.5 s, 300 ≈ 5.6 h). Fixed 2026-08-03 (zero-copy
//! lexer matching — near-linear now); `ir_parse_big_text_256` samples the
//! scaled-up input.

use code_forge::backend::x86_64::{TargetMachine, ensure_registered};
use code_forge::backend::{CompiledFunction, FunctionCompiler};
use code_forge::ir::*;
use code_forge::optimize::*;
use criterion::{BatchSize, Criterion, Throughput, criterion_group, criterion_main};
use std::hint::black_box;

// ============================================================
// Benchmark helpers — function builders
// ============================================================

fn build_simple_add() -> Function {
    let sig = FunctionSignature::new(&[(TypeId::I32, "a"), (TypeId::I32, "b")], &[TypeId::I32]);
    let mut b = FunctionBuilder::new("add", TypeContext::new(), sig);
    let (entry, params) = b.create_block_with_params(&[(TypeId::I32, "a"), (TypeId::I32, "b")]);
    b.switch_to_block(entry);
    let sum = b.iadd(params[0], params[1]);
    b.ret(&[sum]);
    b.finish()
}

fn build_many_ops() -> Function {
    let sig = FunctionSignature::new(&[], &[TypeId::I32]);
    let mut b = FunctionBuilder::new("many_ops", TypeContext::new(), sig);
    let entry = b.create_block();
    b.switch_to_block(entry);
    let v1 = b.iconst(1, TypeId::I32);
    let v2 = b.iconst(2, TypeId::I32);
    let mut acc = b.iadd(v1, v2);
    for _ in 0..19 {
        let x = b.iconst(3, TypeId::I32);
        acc = b.iadd(acc, x);
        acc = b.imul(acc, v1);
    }
    b.ret(&[acc]);
    b.finish()
}

/// Parameterized builder: `n` rounds of (iconst, iadd, imul) — used by
/// throughput benchmarks to measure scaling with IR size.
fn build_n_ops(n: usize) -> Function {
    let sig = FunctionSignature::new(&[], &[TypeId::I32]);
    let mut b = FunctionBuilder::new("n_ops", TypeContext::new(), sig);
    let entry = b.create_block();
    b.switch_to_block(entry);
    let v1 = b.iconst(1, TypeId::I32);
    let v2 = b.iconst(2, TypeId::I32);
    let mut acc = b.iadd(v1, v2);
    for _ in 0..n {
        let x = b.iconst(3, TypeId::I32);
        acc = b.iadd(acc, x);
        acc = b.imul(acc, v1);
    }
    b.ret(&[acc]);
    b.finish()
}

fn build_complex_function() -> Function {
    let sig = FunctionSignature::new(
        &[(TypeId::I32, "a"), (TypeId::F64, "b_val")],
        &[TypeId::F64],
    );
    let mut b = FunctionBuilder::new("complex", TypeContext::new(), sig);
    let (entry, params) = b.create_block_with_params(&[(TypeId::I32, "a"), (TypeId::F64, "b_val")]);
    let then_block = b.create_block();
    let else_block = b.create_block();
    let merge_block = b.create_block();

    b.switch_to_block(entry);
    let a = params[0];
    let bv = params[1];
    let x = b.imul(a, a);
    let f_b = b.fmul(bv, bv);
    let ten = b.iconst(10, TypeId::I32);
    let cond = b.icmp(IntCC::SignedGreaterThan, x, ten);
    b.branch(cond, then_block, &[], else_block, &[]);

    let one_bits = 1.0_f64.to_bits();
    b.switch_to_block(then_block);
    let one = b.fconst(one_bits, TypeId::F64);
    let ty = b.fadd(f_b, one);
    b.jump(merge_block, &[ty]);

    b.switch_to_block(else_block);
    let one2 = b.fconst(one_bits, TypeId::F64);
    let fy = b.fsub(f_b, one2);
    b.jump(merge_block, &[fy]);

    b.switch_to_block(merge_block);
    let result_param = b.iconst(0, TypeId::F64); // Simplified phi
    b.ret(&[result_param]);
    b.finish()
}

fn build_multi_block_function() -> Function {
    let sig = FunctionSignature::new(&[(TypeId::I32, "n")], &[TypeId::I32]);
    let mut b = FunctionBuilder::new("sum_to_n", TypeContext::new(), sig);
    let (entry, params) = b.create_block_with_params(&[(TypeId::I32, "n")]);
    let header = b.create_block();
    let body = b.create_block();
    let exit = b.create_block();

    b.switch_to_block(entry);
    let n = params[0];
    let zero = b.iconst(0, TypeId::I32);
    let one = b.iconst(1, TypeId::I32);
    b.jump(header, &[zero, zero]);

    b.switch_to_block(header);
    let i = b.iconst(0, TypeId::I32);
    let sum = b.iconst(0, TypeId::I32);
    let cond = b.icmp(IntCC::SignedLessThan, i, n);
    b.branch(cond, body, &[], exit, &[]);

    b.switch_to_block(body);
    let next_sum = b.iadd(sum, i);
    let next_i = b.iadd(i, one);
    b.jump(header, &[next_i, next_sum]);

    b.switch_to_block(exit);
    b.ret(&[sum]);
    b.finish()
}

/// Memory-heavy function: 20 rounds of store → load → accumulate. Gives
/// Mem2Reg a real promotion workload (its old benchmark input had no
/// load/store at all, so the pass ran on an empty task).
fn build_mem_func() -> Function {
    let sig = FunctionSignature::new(&[(TypeId::PTR, "p")], &[TypeId::I32]);
    let mut b = FunctionBuilder::new("mem_ops", TypeContext::new(), sig);
    let (entry, params) = b.create_block_with_params(&[(TypeId::PTR, "p")]);
    b.switch_to_block(entry);
    let mut acc = b.iconst(0, TypeId::I32);
    for i in 0..20 {
        let v = b.iconst(i as i64, TypeId::I32);
        b.store(v, params[0]);
        let loaded = b.load(params[0], TypeId::I32);
        acc = b.iadd(acc, loaded);
    }
    b.ret(&[acc]);
    b.finish()
}

/// Floating-point-heavy function: F64 multiply-accumulate chain.
fn build_float_func() -> Function {
    let sig = FunctionSignature::new(&[(TypeId::F64, "x")], &[TypeId::F64]);
    let mut b = FunctionBuilder::new("float_chain", TypeContext::new(), sig);
    let (entry, params) = b.create_block_with_params(&[(TypeId::F64, "x")]);
    b.switch_to_block(entry);
    let one = b.fconst(1.0_f64.to_bits(), TypeId::F64);
    let mut acc = b.fmul(params[0], params[0]);
    for _ in 0..19 {
        acc = b.fadd(acc, one);
        acc = b.fmul(acc, params[0]);
    }
    b.ret(&[acc]);
    b.finish()
}

/// Function with an external call site (`call @0`). The callee is not part of
/// this function; passes with an empty table see a plain call site.
fn build_call_func() -> Function {
    let sig = FunctionSignature::new(&[(TypeId::I32, "a")], &[TypeId::I32]);
    let mut b = FunctionBuilder::new("caller", TypeContext::new(), sig);
    let (entry, params) = b.create_block_with_params(&[(TypeId::I32, "a")]);
    b.switch_to_block(entry);
    let rets = b.call(FuncRef(0), &[params[0]], &[TypeId::I32]);
    let r = b.iadd(params[0], rets[0]);
    b.ret(&[r]);
    b.finish()
}

/// Spill-pressure function: 12 live constants + a balanced add tree, so the
/// register allocator must spill several values (mirrors the old
/// test_spill_high_pressure scenario).
fn build_spill_pressure() -> Function {
    let sig = FunctionSignature::new(&[], &[TypeId::I32]);
    let mut b = FunctionBuilder::new("spill_pressure", TypeContext::new(), sig);
    let entry = b.create_block();
    b.switch_to_block(entry);
    let v1 = b.iconst(1, TypeId::I32);
    let v2 = b.iconst(2, TypeId::I32);
    let v3 = b.iconst(3, TypeId::I32);
    let v4 = b.iconst(4, TypeId::I32);
    let v5 = b.iconst(5, TypeId::I32);
    let v6 = b.iconst(6, TypeId::I32);
    let v7 = b.iconst(7, TypeId::I32);
    let v8 = b.iconst(8, TypeId::I32);
    let v9 = b.iconst(9, TypeId::I32);
    let v10 = b.iconst(10, TypeId::I32);
    let v11 = b.iconst(11, TypeId::I32);
    let v12 = b.iconst(12, TypeId::I32);
    let s1 = b.iadd(v1, v2);
    let s2 = b.iadd(v3, v4);
    let s3 = b.iadd(v5, v6);
    let s4 = b.iadd(v7, v8);
    let s5 = b.iadd(v9, v10);
    let s6 = b.iadd(v11, v12);
    let m1 = b.iadd(s1, s2);
    let m2 = b.iadd(s3, s4);
    let m3 = b.iadd(s5, s6);
    let t1 = b.iadd(m1, m2);
    let r = b.iadd(t1, m3);
    b.ret(&[r]);
    b.finish()
}

/// Loop function with a large body (20 arithmetic ops per iteration).
fn build_big_loop() -> Function {
    let sig = FunctionSignature::new(&[(TypeId::I32, "n")], &[TypeId::I32]);
    let mut b = FunctionBuilder::new("big_loop", TypeContext::new(), sig);
    let (entry, params) = b.create_block_with_params(&[(TypeId::I32, "n")]);
    let header = b.create_block();
    let body = b.create_block();
    let exit = b.create_block();

    b.switch_to_block(entry);
    let n = params[0];
    let zero = b.iconst(0, TypeId::I32);
    let one = b.iconst(1, TypeId::I32);
    b.jump(header, &[zero, zero]);

    b.switch_to_block(header);
    let i = b.iconst(0, TypeId::I32);
    let sum = b.iconst(0, TypeId::I32);
    let cond = b.icmp(IntCC::SignedLessThan, i, n);
    b.branch(cond, body, &[], exit, &[]);

    b.switch_to_block(body);
    let mut acc = b.iadd(sum, i);
    for k in 0..19 {
        let c = b.iconst(k + 2, TypeId::I32);
        acc = b.iadd(acc, c);
        acc = b.imul(acc, c);
    }
    let next_i = b.iadd(i, one);
    b.jump(header, &[next_i, acc]);

    b.switch_to_block(exit);
    b.ret(&[sum]);
    b.finish()
}

/// Count IR instructions in a function.
fn count_ir_insts(func: &Function) -> usize {
    func.dfg.blocks.iter().map(|b| b.inst_order.len()).sum()
}

// ============================================================
// IR text samples (for the ir_parse group)
//
// The forge-ir text parser currently handles one function per source
// (multi-function modules are not supported yet — see forge-ir#parse_module),
// so "multi-func" is measured by parsing several functions in sequence.
//
// These samples contain real instruction bodies (iadd/imul/fmul/icmp/br/jmp)
// so the benchmark exercises actual instruction parsing — the old samples were
// all `{ entry: ret i32 }` shells that parsed to an empty body.
// ============================================================

const IR_SIMPLE_ADD: &str = "define i32 @add(i32 %a, i32 %b) {\n  %entry:\n    %s = add i32 %a, i32 %b\n    ret i32 %s\n}\n";
const IR_MUL_ADD: &str = "define i32 @mul_add(i32 %a, i32 %b, i32 %c) {\n  %entry:\n    %m = mul i32 %a, i32 %b\n    %s = add i32 %m, i32 %c\n    ret i32 %s\n}\n";
const IR_DOT_PRODUCT: &str = "define i32 @dot_product(i32 %a, i32 %b) {\n  %entry:\n    br label %header\n  %header:\n    %cond = icmp eq i32 %a, i32 %b\n    br i1 %cond, label %body, label %exit\n  %body:\n    %p = mul i32 %a, i32 %b\n    br label %header\n  %exit:\n    ret i32 %a\n}\n";
const IR_LOOP_SUM: &str = "define i32 @loop_sum(i32 %n) {\n  %entry:\n    br label %header\n  %header:\n    %cond = icmp slt i32 0, i32 %n\n    br i1 %cond, label %body, label %exit\n  %body:\n    %s1 = add i32 0, i32 1\n    %i1 = add i32 0, i32 %n\n    br label %header\n  %exit:\n    ret i32 0\n}\n";
/// Float + branch sample (mirrors `build_complex_function`).
const IR_COMPLEX_TEXT: &str = "define double @complex(i32 %a, double %b) {\n  %entry:\n    %x = mul i32 %a, i32 %a\n    %f = fmul double %b, double %b\n    %cond = icmp eq i32 %x, i32 %a\n    br i1 %cond, label %then, label %els\n  %then:\n    %t = fadd double %f, double %f\n    ret double %t\n  %els:\n    %e = fsub double %f, double %f\n    ret double %e\n}\n";

/// Programmatically build a large linear IR text: `n` chained `iadd`
/// instructions in a single block (exercises parser scaling).
fn ir_text_many_ops(n: usize) -> String {
    let mut s = String::with_capacity(n * 24 + 64);
    s.push_str("define i32 @many_ops(i32 %a) {\n  %entry:\n");
    for i in 0..n {
        let prev = if i == 0 {
            "%a".to_string()
        } else {
            format!("%v{}", i - 1)
        };
        s.push_str(&format!("    %v{i} = add i32 {prev}, i32 %a\n"));
    }
    s.push_str(&format!("    ret i32 %v{}\n}}\n", n - 1));
    s
}

// ============================================================
// Group 1: IR Construction Benchmarks
// ============================================================

fn bench_ir_build_simple(c: &mut Criterion) {
    c.bench_function("ir_build_simple_add", |b| {
        b.iter(|| {
            let func = black_box(build_simple_add());
            black_box(func);
        });
    });
}

fn bench_ir_build_many_ops(c: &mut Criterion) {
    c.bench_function("ir_build_many_ops", |b| {
        b.iter(|| {
            let func = black_box(build_many_ops());
            black_box(func);
        });
    });
}

fn bench_ir_build_complex(c: &mut Criterion) {
    c.bench_function("ir_build_complex", |b| {
        b.iter(|| {
            let func = black_box(build_complex_function());
            black_box(func);
        });
    });
}

fn bench_ir_build_multi_block(c: &mut Criterion) {
    c.bench_function("ir_build_multi_block", |b| {
        b.iter(|| {
            let func = black_box(build_multi_block_function());
            black_box(func);
        });
    });
}

fn bench_ir_build_float(c: &mut Criterion) {
    c.bench_function("ir_build_float", |b| {
        b.iter(|| {
            let func = black_box(build_float_func());
            black_box(func);
        });
    });
}

fn bench_ir_build_mem(c: &mut Criterion) {
    c.bench_function("ir_build_mem", |b| {
        b.iter(|| {
            let func = black_box(build_mem_func());
            black_box(func);
        });
    });
}

fn bench_ir_build_call(c: &mut Criterion) {
    c.bench_function("ir_build_call", |b| {
        b.iter(|| {
            let func = black_box(build_call_func());
            black_box(func);
        });
    });
}

fn bench_ir_build_spill_pressure(c: &mut Criterion) {
    c.bench_function("ir_build_spill_pressure", |b| {
        b.iter(|| {
            let func = black_box(build_spill_pressure());
            black_box(func);
        });
    });
}

fn bench_ir_build_big_loop(c: &mut Criterion) {
    c.bench_function("ir_build_big_loop", |b| {
        b.iter(|| {
            let func = black_box(build_big_loop());
            black_box(func);
        });
    });
}

// ============================================================
// Group 2: IR Text Parsing Benchmarks
// ============================================================

fn bench_ir_parse_simple_add(c: &mut Criterion) {
    c.bench_function("ir_parse_simple_add", |b| {
        b.iter(|| {
            let f = forge_ir::ir_parser::parse_function(black_box(IR_SIMPLE_ADD))
                .unwrap_or_else(|e| panic!("ir_parse_simple_add: {e}"));
            black_box(f);
        });
    });
}

fn bench_ir_parse_mul_add(c: &mut Criterion) {
    c.bench_function("ir_parse_mul_add", |b| {
        b.iter(|| {
            let f = forge_ir::ir_parser::parse_function(black_box(IR_MUL_ADD))
                .unwrap_or_else(|e| panic!("ir_parse_mul_add: {e}"));
            black_box(f);
        });
    });
}

fn bench_ir_parse_dot_product(c: &mut Criterion) {
    c.bench_function("ir_parse_dot_product", |b| {
        b.iter(|| {
            let f = forge_ir::ir_parser::parse_function(black_box(IR_DOT_PRODUCT))
                .unwrap_or_else(|e| panic!("ir_parse_dot_product: {e}"));
            black_box(f);
        });
    });
}

fn bench_ir_parse_loop_sum(c: &mut Criterion) {
    c.bench_function("ir_parse_loop_sum", |b| {
        b.iter(|| {
            let f = forge_ir::ir_parser::parse_function(black_box(IR_LOOP_SUM))
                .unwrap_or_else(|e| panic!("ir_parse_loop_sum: {e}"));
            black_box(f);
        });
    });
}

fn bench_ir_parse_complex(c: &mut Criterion) {
    c.bench_function("ir_parse_complex", |b| {
        b.iter(|| {
            let f = forge_ir::ir_parser::parse_function(black_box(IR_COMPLEX_TEXT))
                .unwrap_or_else(|e| panic!("ir_parse_complex: {e}"));
            black_box(f);
        });
    });
}

fn bench_ir_parse_multi_func(c: &mut Criterion) {
    let srcs = [IR_SIMPLE_ADD, IR_MUL_ADD, IR_DOT_PRODUCT, IR_LOOP_SUM];
    c.bench_function("ir_parse_multi_func", |b| {
        b.iter(|| {
            for src in srcs {
                let f = forge_ir::ir_parser::parse_function(black_box(src))
                    .unwrap_or_else(|e| panic!("ir_parse_multi_func: {e}"));
                black_box(f);
            }
        });
    });
}

/// Large linear text (`IR_BIG_TEXT_N` chained iadds) — parser scaling. The text
/// itself is built once outside the timed loop (only parsing is measured).
///
/// NOTE: was capped at 4 because the forge-grammar lexer was O(n²) (each token
/// re-copied the remaining input and regex-matched longest-first prefixes),
/// making 300-instr text take ~5.6 h. Fixed 2026-08-03 (zero-copy `&[char]`
/// matching + `match_here` prefix length) — parsing is now near-linear
/// (~46 ms @ 128 instrs debug), so the sample is scaled up to 256.
const IR_BIG_TEXT_N: usize = 256;

fn bench_ir_parse_big_text(c: &mut Criterion) {
    let src = ir_text_many_ops(IR_BIG_TEXT_N);
    c.bench_function("ir_parse_big_text_256", |b| {
        b.iter(|| {
            let f = forge_ir::ir_parser::parse_function(black_box(&src))
                .unwrap_or_else(|e| panic!("ir_parse_big_text_256: {e}"));
            black_box(f);
        });
    });
}

// ============================================================
// Group 3: Optimization Pipeline Benchmarks
// ============================================================

/// Run a single function pass with construction excluded from measurement
/// (`iter_batched` — the setup closure's cost is not part of the timed
/// routine, so the pass itself is what is measured).
fn bench_function_pass(
    c: &mut Criterion,
    name: &str,
    build: fn() -> Function,
    pass: Box<dyn OptimizationPass>,
) {
    c.bench_function(name, |b| {
        b.iter_batched(
            build,
            |mut f| {
                pass.run_on_function(&mut f).unwrap();
                black_box(f);
            },
            BatchSize::SmallInput,
        );
    });
}

fn bench_opt_const_fold(c: &mut Criterion) {
    let pass = forge_opt::scalar::const_fold::ConstFoldPass::new();
    bench_function_pass(c, "opt_const_fold", build_many_ops, Box::new(pass));
}

/// Constant folding on the floating-point chain (F64 folds exercise the Big
/// float paths).
fn bench_opt_const_fold_float(c: &mut Criterion) {
    let pass = forge_opt::scalar::const_fold::ConstFoldPass::new();
    bench_function_pass(c, "opt_const_fold_float", build_float_func, Box::new(pass));
}

fn bench_opt_dce(c: &mut Criterion) {
    let pass = forge_opt::scalar::dead_code::DeadCodeElimPass::new();
    bench_function_pass(c, "opt_dead_code_elim", build_many_ops, Box::new(pass));
}

fn bench_opt_copy_prop(c: &mut Criterion) {
    let pass = forge_opt::scalar::copy_prop::CopyPropPass::new();
    bench_function_pass(c, "opt_copy_prop", build_many_ops, Box::new(pass));
}

fn bench_opt_cse(c: &mut Criterion) {
    let pass = forge_opt::scalar::cse::CsePass::new();
    bench_function_pass(c, "opt_cse", build_many_ops, Box::new(pass));
}

fn bench_opt_gvn(c: &mut Criterion) {
    let pass = forge_opt::scalar::gvn::GvnPass::new();
    bench_function_pass(c, "opt_gvn", build_many_ops, Box::new(pass));
}

fn bench_opt_sccp(c: &mut Criterion) {
    let pass = forge_opt::scalar::sccp::SccpPass::new();
    bench_function_pass(c, "opt_sccp", build_many_ops, Box::new(pass));
}

fn bench_opt_jump_thread(c: &mut Criterion) {
    let pass = forge_opt::scalar::jump_thread::JumpThreadPass::new();
    bench_function_pass(c, "opt_jump_thread", build_complex_function, Box::new(pass));
}

fn bench_opt_mem2reg(c: &mut Criterion) {
    let pass = forge_opt::scalar::mem2reg::Mem2RegPass::new();
    bench_function_pass(c, "opt_mem2reg", build_mem_func, Box::new(pass));
}

fn bench_opt_licm(c: &mut Criterion) {
    let pass = forge_opt::loops::licm::LicmPass::new();
    bench_function_pass(c, "opt_licm", build_multi_block_function, Box::new(pass));
}

fn bench_opt_block_param_coalesce(c: &mut Criterion) {
    let pass = forge_opt::scalar::block_param_coalesce::BlockParamCoalescePass::new();
    bench_function_pass(
        c,
        "opt_block_param_coalesce",
        build_multi_block_function,
        Box::new(pass),
    );
}

fn bench_opt_ind_var_simplify(c: &mut Criterion) {
    let pass = forge_opt::loops::ind_var_simplify::IndVarSimplifyPass::new();
    bench_function_pass(
        c,
        "opt_ind_var_simplify",
        build_multi_block_function,
        Box::new(pass),
    );
}

fn bench_opt_loop_unroll(c: &mut Criterion) {
    let pass = forge_opt::loops::loop_unroll::LoopUnrollPass::new();
    bench_function_pass(
        c,
        "opt_loop_unroll",
        build_multi_block_function,
        Box::new(pass),
    );
}

fn bench_opt_gvn_pre(c: &mut Criterion) {
    let pass = forge_opt::scalar::gvn_pre::PrePass;
    bench_function_pass(c, "opt_gvn_pre", build_many_ops, Box::new(pass));
}

fn bench_opt_egraph(c: &mut Criterion) {
    let pass = forge_opt::advanced::egraph::EGraphPass::new();
    bench_function_pass(c, "opt_egraph", build_many_ops, Box::new(pass));
}

fn bench_opt_pgo(c: &mut Criterion) {
    let pass = forge_opt::advanced::pgo::PgoInstrumentPass::new();
    bench_function_pass(c, "opt_pgo", build_many_ops, Box::new(pass));
}

fn bench_opt_isel(c: &mut Criterion) {
    let pass = forge_opt::advanced::egraph::ISelPass::new();
    bench_function_pass(c, "opt_isel", build_many_ops, Box::new(pass));
}

// ── IPA passes (empty function table) ──
// These passes take a cross-function table; with an empty table they run on
// the single benchmark function and mostly exercise the pass's own
// scan/dispatch overhead. Building a populated module-level benchmark would
// require a separate module framework.

fn bench_opt_inline(c: &mut Criterion) {
    let pass = forge_opt::ipa::inline::InlinePass::new(std::collections::HashMap::new());
    bench_function_pass(c, "opt_inline_empty_table", build_many_ops, Box::new(pass));
}

fn bench_opt_tail_call(c: &mut Criterion) {
    let pass = forge_opt::ipa::tail_call::TailCallPass::new(std::collections::HashMap::new());
    bench_function_pass(
        c,
        "opt_tail_call_empty_table",
        build_many_ops,
        Box::new(pass),
    );
}

fn bench_opt_func_specialize(c: &mut Criterion) {
    let pass =
        forge_opt::ipa::func_specialize::FuncSpecializePass::new(std::collections::HashMap::new());
    bench_function_pass(
        c,
        "opt_func_specialize_empty_table",
        build_many_ops,
        Box::new(pass),
    );
}

/// Run a module-level pass (e.g. LTO) on an empty module.
fn bench_module_pass(c: &mut Criterion, name: &str, pass: Box<dyn OptimizationPass>) {
    c.bench_function(name, |b| {
        b.iter(|| {
            let mut m = forge_ir::Module::new();
            pass.run_on_module(&mut m).unwrap();
            black_box(m);
        });
    });
}

fn bench_opt_lto(c: &mut Criterion) {
    let context = forge_opt::ipa::lto::LtoContext::new();
    let pass = forge_opt::ipa::lto::LtoPass::new(context);
    bench_module_pass(c, "opt_lto_empty_module", Box::new(pass));
}

/// Full pipeline benchmark: build excluded from measurement.
fn bench_pipeline(
    c: &mut Criterion,
    name: &str,
    level: OptimizationLevel,
    build: fn() -> Function,
) {
    let pm = PassManager::for_level(level);
    c.bench_function(name, |b| {
        b.iter_batched(
            build,
            |mut f| {
                pm.run_on_function(&mut f).unwrap();
                black_box(f);
            },
            BatchSize::SmallInput,
        );
    });
}

fn bench_opt_pipeline_o0(c: &mut Criterion) {
    bench_pipeline(
        c,
        "opt_pipeline_o0",
        OptimizationLevel::O0,
        build_complex_function,
    );
}

fn bench_opt_pipeline_o1(c: &mut Criterion) {
    bench_pipeline(
        c,
        "opt_pipeline_o1",
        OptimizationLevel::O1,
        build_complex_function,
    );
}

fn bench_opt_pipeline_o2(c: &mut Criterion) {
    bench_pipeline(
        c,
        "opt_pipeline_o2",
        OptimizationLevel::O2,
        build_complex_function,
    );
}

fn bench_opt_pipeline_o3(c: &mut Criterion) {
    bench_pipeline(
        c,
        "opt_pipeline_o3",
        OptimizationLevel::O3,
        build_complex_function,
    );
}

fn bench_opt_pipeline_loop_func(c: &mut Criterion) {
    bench_pipeline(
        c,
        "opt_pipeline_loop_func",
        OptimizationLevel::O2,
        build_multi_block_function,
    );
}

fn bench_opt_pipeline_o1_loop_func(c: &mut Criterion) {
    bench_pipeline(
        c,
        "opt_pipeline_o1_loop_func",
        OptimizationLevel::O1,
        build_multi_block_function,
    );
}

fn bench_opt_pipeline_o3_loop_func(c: &mut Criterion) {
    bench_pipeline(
        c,
        "opt_pipeline_o3_loop_func",
        OptimizationLevel::O3,
        build_multi_block_function,
    );
}

// ============================================================
// Group 3b: Pipeline Pass Breakdown
// ============================================================

/// Per-pass cost within the O1/O2/O3 pipelines, measured on a function shape
/// that exercises each pass (mirrors the pass lists in
/// `PassManager::for_level`). Summing a level's rows approximates its
/// pipeline cost; the rows identify which pass dominates a level.
fn bench_pipeline_breakdown(c: &mut Criterion) {
    let mut group = c.benchmark_group("pipeline_breakdown");
    let passes: Vec<(&str, fn() -> Box<dyn OptimizationPass>, fn() -> Function)> = vec![
        // O1
        (
            "o1/const_fold",
            || Box::new(forge_opt::scalar::const_fold::ConstFoldPass::new()),
            build_many_ops,
        ),
        (
            "o1/copy_prop",
            || Box::new(forge_opt::scalar::copy_prop::CopyPropPass::new()),
            build_many_ops,
        ),
        (
            "o1/cse",
            || Box::new(forge_opt::scalar::cse::CsePass::new()),
            build_many_ops,
        ),
        (
            "o1/dead_code",
            || Box::new(forge_opt::scalar::dead_code::DeadCodeElimPass::new()),
            build_many_ops,
        ),
        (
            "o1/jump_thread",
            || Box::new(forge_opt::scalar::jump_thread::JumpThreadPass::new()),
            build_complex_function,
        ),
        // O2 (on top of O1)
        (
            "o2/gvn",
            || Box::new(forge_opt::scalar::gvn::GvnPass::new()),
            build_many_ops,
        ),
        (
            "o2/gvn_pre",
            || Box::new(forge_opt::scalar::gvn_pre::PrePass),
            build_many_ops,
        ),
        (
            "o2/sccp",
            || Box::new(forge_opt::scalar::sccp::SccpPass::new()),
            build_many_ops,
        ),
        (
            "o2/block_param_coalesce",
            || Box::new(forge_opt::scalar::block_param_coalesce::BlockParamCoalescePass::new()),
            build_multi_block_function,
        ),
        (
            "o2/licm",
            || Box::new(forge_opt::loops::licm::LicmPass::new()),
            build_multi_block_function,
        ),
        (
            "o2/tail_call",
            || {
                Box::new(forge_opt::ipa::tail_call::TailCallPass::new(
                    std::collections::HashMap::new(),
                ))
            },
            build_many_ops,
        ),
        (
            "o2/egraph",
            || Box::new(forge_opt::advanced::egraph::EGraphPass::new()),
            build_many_ops,
        ),
        // O3 (on top of O2)
        (
            "o3/inline",
            || {
                Box::new(forge_opt::ipa::inline::InlinePass::new(
                    std::collections::HashMap::new(),
                ))
            },
            build_many_ops,
        ),
        (
            "o3/mem2reg",
            || Box::new(forge_opt::scalar::mem2reg::Mem2RegPass::new()),
            build_mem_func,
        ),
        (
            "o3/ind_var_simplify",
            || Box::new(forge_opt::loops::ind_var_simplify::IndVarSimplifyPass::new()),
            build_multi_block_function,
        ),
        (
            "o3/loop_unroll",
            || Box::new(forge_opt::loops::loop_unroll::LoopUnrollPass::new()),
            build_multi_block_function,
        ),
    ];

    for (name, pass_builder, build) in passes {
        group.bench_function(name, |b| {
            b.iter_batched(
                build,
                |mut f| {
                    let pass = pass_builder();
                    pass.run_on_function(&mut f).unwrap();
                    black_box(f);
                },
                BatchSize::SmallInput,
            );
        });
    }
    group.finish();
}

// ============================================================
// Group 4: Code Generation Benchmarks (x86_64)
// ============================================================

fn compile_fn(func: &Function) -> CompiledFunction {
    FunctionCompiler::new(TargetMachine::new())
        .compile_raw(func)
        .unwrap()
}

fn bench_codegen(c: &mut Criterion, name: &str, func: &Function) {
    c.bench_function(name, |b| {
        b.iter(|| {
            let compiled = black_box(compile_fn(func));
            black_box(compiled);
        });
    });
}

fn bench_codegen_simple_add(c: &mut Criterion) {
    ensure_registered();
    let func = build_simple_add();
    bench_codegen(c, "codegen_simple_add", &func);
}

fn bench_codegen_many_ops(c: &mut Criterion) {
    ensure_registered();
    let func = build_many_ops();
    bench_codegen(c, "codegen_many_ops", &func);
}

fn bench_codegen_complex(c: &mut Criterion) {
    ensure_registered();
    let func = build_complex_function();
    bench_codegen(c, "codegen_complex", &func);
}

fn bench_codegen_multi_block(c: &mut Criterion) {
    ensure_registered();
    let func = build_multi_block_function();
    bench_codegen(c, "codegen_multi_block", &func);
}

fn bench_codegen_float(c: &mut Criterion) {
    ensure_registered();
    let func = build_float_func();
    bench_codegen(c, "codegen_float", &func);
}

fn bench_codegen_mem(c: &mut Criterion) {
    ensure_registered();
    let func = build_mem_func();
    bench_codegen(c, "codegen_mem", &func);
}

// Direct call compiles now: x86 lowers Call to CALL rel32 with a placeholder
// displacement (0) — cross-function targets are not resolved in a
// single-function compile, so the emitted code is a stub. JIT execution of a
// call function is NOT supported.
fn bench_codegen_call(c: &mut Criterion) {
    ensure_registered();
    let func = build_call_func();
    bench_codegen(c, "codegen_call", &func);
}

fn bench_codegen_spill_pressure(c: &mut Criterion) {
    ensure_registered();
    let func = build_spill_pressure();
    bench_codegen(c, "codegen_spill_pressure", &func);
}

fn bench_codegen_big_loop(c: &mut Criterion) {
    ensure_registered();
    let func = build_big_loop();
    bench_codegen(c, "codegen_big_loop", &func);
}

/// Codegen benchmarks for the other backends (aarch64 / riscv64 / wasm32),
/// compiled with the same representative functions as x86_64.
fn bench_codegen_other_isa(c: &mut Criterion) {
    // aarch64 now lowers scalar float (Fadd/Fsub/Fmul/Fconst via NEON + FMOV),
    // so it covers the float `complex` case too. riscv64/wasm32: riscv64 lacks
    // F-extension instructions still, so complex stays x86_64/aarch64/wasm32.
    let aarch64_funcs: Vec<(&str, Function)> = vec![
        ("simple_add", build_simple_add()),
        ("many_ops", build_many_ops()),
        ("multi_block", build_multi_block_function()),
        ("complex", build_complex_function()),
    ];
    // riscv64 now has the F extension (Fadd/Fsub/Fmul/Fdiv/Fconst), so it
    // covers complex too.
    let riscv64_funcs: Vec<(&str, Function)> = vec![
        ("simple_add", build_simple_add()),
        ("many_ops", build_many_ops()),
        ("multi_block", build_multi_block_function()),
        ("complex", build_complex_function()),
    ];
    let common: Vec<(&str, Function)> = vec![
        ("simple_add", build_simple_add()),
        ("many_ops", build_many_ops()),
        ("multi_block", build_multi_block_function()),
    ];
    let _ = &common;
    // Function is not Clone, so build the wasm list separately.
    let wasm_funcs: Vec<(&str, Function)> = vec![
        ("simple_add", build_simple_add()),
        ("many_ops", build_many_ops()),
        ("multi_block", build_multi_block_function()),
        ("complex", build_complex_function()),
    ];

    code_forge::backend::aarch64::ensure_registered();
    codegen_isa_group(c, "aarch64", &aarch64_funcs, || {
        code_forge::backend::aarch64::TargetMachine::new()
    });
    code_forge::backend::riscv64::ensure_registered();
    codegen_isa_group(c, "riscv64", &riscv64_funcs, || {
        code_forge::backend::riscv64::TargetMachine::new()
    });
    code_forge::backend::wasm32::ensure_registered();
    codegen_isa_group(c, "wasm32", &wasm_funcs, || {
        code_forge::backend::wasm32::TargetMachine::new()
    });
}

/// Compile `funcs` with a generic ISA backend into a `codegen/{isa}/*` group.
fn codegen_isa_group<M: code_forge::backend::TargetMachine>(
    c: &mut Criterion,
    isa: &str,
    funcs: &[(&str, Function)],
    machine: impl Fn() -> M,
) {
    for (name, func) in funcs {
        let mut g = c.benchmark_group(format!("codegen/{isa}"));
        g.bench_function(*name, |b| {
            b.iter(|| {
                let compiled = black_box(
                    FunctionCompiler::new(machine())
                        .compile_raw(func)
                        .unwrap_or_else(|e| panic!("{isa} {}: {e}", name)),
                );
                black_box(compiled);
            });
        });
        g.finish();
    }
}

// ============================================================
// Group 4b: IR Verification Benchmarks
// ============================================================

/// `forge_ir::verify::Verifier` on the standard benchmark functions. IR
/// verification is ISA-independent — one group covers all representative
/// function shapes (including the spill-heavy one, which exercises the
/// use-list / operand-count checks hardest).
fn bench_verify(c: &mut Criterion) {
    let cases: Vec<(&str, Function)> = vec![
        ("simple_add", build_simple_add()),
        ("many_ops", build_many_ops()),
        ("complex", build_complex_function()),
        ("loop", build_multi_block_function()),
        ("mem", build_mem_func()),
        ("spill_pressure", build_spill_pressure()),
    ];
    for (name, func) in &cases {
        c.bench_function(&format!("verify/{name}"), |b| {
            b.iter(|| {
                let mut v = forge_ir::verify::Verifier::new();
                let ok = v.verify(black_box(func)).is_ok();
                black_box(ok);
            });
        });
    }
}

// ============================================================
// Group 4c: Module-Level Benchmarks
// ============================================================

/// Callee for module tests: `fn callee_double(a: i32) -> i32 { a * 2 }`.
fn build_callee() -> Function {
    let sig = FunctionSignature::new(&[(TypeId::I32, "a")], &[TypeId::I32]);
    let mut b = FunctionBuilder::new("callee_double", TypeContext::new(), sig);
    let (entry, params) = b.create_block_with_params(&[(TypeId::I32, "a")]);
    b.switch_to_block(entry);
    let two = b.iconst(2, TypeId::I32);
    let r = b.imul(params[0], two);
    b.ret(&[r]);
    b.finish()
}

/// Small module: `callee_double` (FuncRef 0) then `caller` (FuncRef 1, whose
/// body is `call @0; iadd`). `Module::add_function` assigns FuncRef in
/// insertion order, so `build_call_func`'s hardcoded `call FuncRef(0)` targets
/// the callee during module-level compilation.
fn build_test_module() -> forge_ir::Module {
    let mut m = forge_ir::Module::new();
    m.add_function(build_callee());
    m.add_function(build_call_func());
    m
}

/// JIT module compile: two functions, one with a cross-function `call @0` —
/// exercises module-level relocation resolution (unlike single-function
/// `compile_raw`, which emits a placeholder displacement).
fn bench_module_compile(c: &mut Criterion) {
    ensure_registered();
    c.bench_function("module_compile_cross_call", |b| {
        b.iter_batched(
            build_test_module,
            |m| {
                let mut jit = code_forge::jit::JitCompiler::new(TargetMachine::new());
                jit.compile_module(&m).unwrap();
                black_box(jit);
            },
            BatchSize::SmallInput,
        );
    });
}

/// IPA passes with a real (non-empty) function table — the old benchmarks ran
/// inline/tail-call with an empty HashMap, so they measured only dispatch
/// overhead. Here the table holds the callee, so the pass has real work.
fn bench_opt_ipa_real_table(c: &mut Criterion) {
    let table_src = || {
        let mut t: std::collections::HashMap<FuncRef, Function> = std::collections::HashMap::new();
        t.insert(FuncRef(0), build_callee());
        t.insert(FuncRef(1), build_call_func());
        t
    };

    c.bench_function("opt_inline_real_table", |b| {
        b.iter_batched(
            table_src,
            |table| {
                let pass = forge_opt::ipa::inline::InlinePass::new(table);
                let mut f = build_call_func();
                pass.run_on_function(&mut f).unwrap();
                black_box(f);
            },
            BatchSize::SmallInput,
        );
    });

    c.bench_function("opt_tail_call_real_table", |b| {
        b.iter_batched(
            table_src,
            |table| {
                let pass = forge_opt::ipa::tail_call::TailCallPass::new(table);
                let mut f = build_call_func();
                pass.run_on_function(&mut f).unwrap();
                black_box(f);
            },
            BatchSize::SmallInput,
        );
    });
}

fn bench_codegen_with_o1(c: &mut Criterion) {
    ensure_registered();
    let pm = PassManager::for_level(OptimizationLevel::O1);
    c.bench_function("codegen_with_o1", |b| {
        b.iter_batched(
            build_many_ops,
            |mut func| {
                pm.run_on_function(&mut func).unwrap();
                let compiled = compile_fn(&func);
                black_box(compiled);
            },
            BatchSize::SmallInput,
        );
    });
}

fn bench_codegen_with_o2(c: &mut Criterion) {
    ensure_registered();
    let pm = PassManager::for_level(OptimizationLevel::O2);
    c.bench_function("codegen_with_o2", |b| {
        b.iter_batched(
            build_many_ops,
            |mut func| {
                pm.run_on_function(&mut func).unwrap();
                let compiled = compile_fn(&func);
                black_box(compiled);
            },
            BatchSize::SmallInput,
        );
    });
}

fn bench_codegen_with_o2_complex(c: &mut Criterion) {
    ensure_registered();
    let pm = PassManager::for_level(OptimizationLevel::O2);
    c.bench_function("codegen_with_o2_complex", |b| {
        b.iter_batched(
            build_complex_function,
            |mut func| {
                pm.run_on_function(&mut func).unwrap();
                let compiled = compile_fn(&func);
                black_box(compiled);
            },
            BatchSize::SmallInput,
        );
    });
}

// ============================================================
// Group 5: End-to-End Compilation Pipeline
// ============================================================

/// Optimize (O2) then compile — no JIT execution.
fn bench_e2e_compile(c: &mut Criterion, name: &str, build: fn() -> Function) {
    ensure_registered();
    let pm = PassManager::for_level(OptimizationLevel::O2);
    c.bench_function(name, |b| {
        b.iter_batched(
            build,
            |mut func| {
                pm.run_on_function(&mut func).unwrap();
                let compiled = compile_fn(&func);
                black_box(compiled);
            },
            BatchSize::SmallInput,
        );
    });
}

fn bench_e2e_compile_simple(c: &mut Criterion) {
    bench_e2e_compile(c, "e2e_compile_simple", build_simple_add);
}

fn bench_e2e_compile_complex(c: &mut Criterion) {
    bench_e2e_compile(c, "e2e_compile_complex", build_complex_function);
}

fn bench_e2e_compile_loop(c: &mut Criterion) {
    bench_e2e_compile(c, "e2e_compile_loop", build_multi_block_function);
}

fn bench_e2e_compile_float(c: &mut Criterion) {
    bench_e2e_compile(c, "e2e_compile_float", build_float_func);
}

fn bench_e2e_compile_mem(c: &mut Criterion) {
    bench_e2e_compile(c, "e2e_compile_mem", build_mem_func);
}

fn bench_e2e_compile_spill(c: &mut Criterion) {
    bench_e2e_compile(c, "e2e_compile_spill", build_spill_pressure);
}

fn bench_e2e_compile_big_loop(c: &mut Criterion) {
    bench_e2e_compile(c, "e2e_compile_big_loop", build_big_loop);
}

fn bench_e2e_jit_execute(c: &mut Criterion) {
    use code_forge::mem::ExecutableMemory;
    ensure_registered();
    let func = build_simple_add();
    c.bench_function("e2e_jit_execute", |b| {
        b.iter(|| {
            let compiled = compile_fn(&func);
            let mem = ExecutableMemory::new(&compiled.code).unwrap();
            let f: extern "C" fn(i32, i32) -> i32 = unsafe { mem.get_fn(0).unwrap() };
            let result = black_box(f(3, 4));
            assert_eq!(result, 7);
        });
    });
}

// ============================================================
// Group 6: Throughput Benchmarks (parameterized by function size)
// ============================================================

fn bench_throughput(c: &mut Criterion) {
    ensure_registered();
    // One group per N: criterion's group-level `throughput` would otherwise be
    // overwritten by the last N and reported against every benchmark.
    for n in [10usize, 50, 100, 200, 500] {
        let func = build_n_ops(n);
        let insts = count_ir_insts(&func);
        let mut group = c.benchmark_group(format!("throughput/{n}"));
        group.throughput(Throughput::Elements(insts as u64));

        // Codegen throughput at N ops.
        group.bench_function("codegen", |b| {
            b.iter(|| {
                let compiled = black_box(compile_fn(&func));
                black_box(compiled);
            });
        });

        // O1 pipeline throughput at N ops (construction excluded).
        let pm = PassManager::for_level(OptimizationLevel::O1);
        group.bench_function("optimize_o1", |b| {
            b.iter_batched(
                || build_n_ops(n),
                |mut f| {
                    pm.run_on_function(&mut f).unwrap();
                    black_box(f);
                },
                BatchSize::SmallInput,
            );
        });

        // O2/O3 pipeline throughput at N ops (construction excluded).
        let pm2 = PassManager::for_level(OptimizationLevel::O2);
        group.bench_function("optimize_o2", |b| {
            b.iter_batched(
                || build_n_ops(n),
                |mut f| {
                    pm2.run_on_function(&mut f).unwrap();
                    black_box(f);
                },
                BatchSize::SmallInput,
            );
        });
        let pm3 = PassManager::for_level(OptimizationLevel::O3);
        group.bench_function("optimize_o3", |b| {
            b.iter_batched(
                || build_n_ops(n),
                |mut f| {
                    pm3.run_on_function(&mut f).unwrap();
                    black_box(f);
                },
                BatchSize::SmallInput,
            );
        });

        // IR construction throughput at N ops.
        group.bench_function("ir_build", |b| {
            b.iter(|| {
                let f = black_box(build_n_ops(n));
                black_box(f);
            });
        });
        group.finish();
    }
}

// ============================================================
// Group 7: Code Size Metrics
// ============================================================

fn bench_code_size_metrics(c: &mut Criterion) {
    ensure_registered();
    let funcs: Vec<(&str, Function)> = vec![
        ("simple_add", build_simple_add()),
        ("many_ops", build_many_ops()),
        ("complex", build_complex_function()),
        ("multi_block", build_multi_block_function()),
    ];

    // One group per case: group-level `throughput` must match the case's own
    // byte count, so it cannot be shared across cases with different sizes.
    for (name, func) in &funcs {
        let code_bytes = compile_fn(func).code.len();
        let ir_insts = count_ir_insts(func);
        let mut group = c.benchmark_group(format!("code_size/{name}"));
        // Report the compiled size as throughput so the report shows bytes/s.
        group.throughput(Throughput::Bytes(code_bytes as u64));
        group.bench_function("size", |b| {
            b.iter(|| {
                let compiled = black_box(compile_fn(func));
                black_box(compiled.code.len());
                black_box(ir_insts);
            });
        });
        group.finish();
    }
}

// ============================================================
// Group 8: Comparison Report
// ============================================================

fn bench_comparison(c: &mut Criterion) {
    ensure_registered();
    let mut group = c.benchmark_group("comparison");

    let cases: Vec<(&str, Function)> = vec![
        ("simple_add", build_simple_add()),
        ("many_ops", build_many_ops()),
        ("complex", build_complex_function()),
        ("loop", build_multi_block_function()),
    ];

    for (name, func) in &cases {
        let ir_insts = count_ir_insts(func);

        // Optimization time (O2) for this case (construction excluded).
        let pm = PassManager::for_level(OptimizationLevel::O2);
        group.bench_function(format!("opt_o2/{name}"), |b| {
            b.iter_batched(
                || build_from_case(name),
                |mut f| {
                    pm.run_on_function(&mut f).unwrap();
                    black_box(f);
                },
                BatchSize::SmallInput,
            );
        });

        // Codegen time for this case (no optimization).
        group.bench_function(format!("codegen/{name}"), |b| {
            b.iter(|| {
                let compiled = black_box(compile_fn(func));
                black_box(compiled);
            });
        });

        // Code size for this case: compile inside the iteration and report
        // the emitted byte count (also measures real compile time).
        // No group-level throughput here: opt/codegen/size measure different
        // quantities and a shared throughput value would be misleading.
        let _code_bytes = compile_fn(func).code.len();
        group.bench_function(format!("size/{name}"), |b| {
            b.iter(|| {
                let compiled = black_box(compile_fn(func));
                black_box(compiled.code.len());
                black_box(ir_insts);
            });
        });
    }
    group.finish();
}

/// Rebuild a case function by name (used where the pass needs a fresh &mut Function).
fn build_from_case(name: &str) -> Function {
    match name {
        "simple_add" => build_simple_add(),
        "many_ops" => build_many_ops(),
        "complex" => build_complex_function(),
        "loop" => build_multi_block_function(),
        _ => unreachable!("unknown case: {name}"),
    }
}

// ============================================================
// Criterion group definitions
// ============================================================

criterion_group!(
    name = ir_build;
    config = Criterion::default();
    targets =
        bench_ir_build_simple,
        bench_ir_build_many_ops,
        bench_ir_build_complex,
        bench_ir_build_multi_block,
        bench_ir_build_float,
        bench_ir_build_mem,
        bench_ir_build_call,
        bench_ir_build_spill_pressure,
        bench_ir_build_big_loop,
);

criterion_group!(
    name = ir_parse;
    config = Criterion::default();
    targets =
        bench_ir_parse_simple_add,
        bench_ir_parse_mul_add,
        bench_ir_parse_dot_product,
        bench_ir_parse_loop_sum,
        bench_ir_parse_complex,
        bench_ir_parse_multi_func,
        bench_ir_parse_big_text,
);

criterion_group!(
    name = optimizations;
    config = Criterion::default();
    targets =
        bench_opt_const_fold,
        bench_opt_const_fold_float,
        bench_opt_dce,
        bench_opt_copy_prop,
        bench_opt_cse,
        bench_opt_gvn,
        bench_opt_gvn_pre,
        bench_opt_sccp,
        bench_opt_jump_thread,
        bench_opt_mem2reg,
        bench_opt_licm,
        bench_opt_ind_var_simplify,
        bench_opt_loop_unroll,
        bench_opt_block_param_coalesce,
        bench_opt_egraph,
        bench_opt_pgo,
        bench_opt_isel,
        bench_opt_inline,
        bench_opt_tail_call,
        bench_opt_func_specialize,
        bench_opt_lto,
        bench_opt_pipeline_o0,
        bench_opt_pipeline_o1,
        bench_opt_pipeline_o2,
        bench_opt_pipeline_o3,
        bench_opt_pipeline_loop_func,
        bench_opt_pipeline_o1_loop_func,
        bench_opt_pipeline_o3_loop_func,
);

criterion_group!(
    name = pipeline_breakdown;
    config = Criterion::default();
    targets = bench_pipeline_breakdown,
);

criterion_group!(
    name = codegen;
    config = Criterion::default();
    targets =
        bench_codegen_simple_add,
        bench_codegen_many_ops,
        bench_codegen_complex,
        bench_codegen_multi_block,
        bench_codegen_float,
        bench_codegen_mem,
        bench_codegen_call,
        bench_codegen_spill_pressure,
        bench_codegen_big_loop,
        bench_codegen_with_o1,
        bench_codegen_with_o2,
        bench_codegen_with_o2_complex,
        bench_codegen_other_isa,
);

criterion_group!(
    name = verify;
    config = Criterion::default();
    targets = bench_verify,
);

criterion_group!(
    name = module;
    config = Criterion::default();
    targets =
        bench_module_compile,
        bench_opt_ipa_real_table,
);

criterion_group!(
    name = end_to_end;
    config = Criterion::default();
    targets =
        bench_e2e_compile_simple,
        bench_e2e_compile_complex,
        bench_e2e_compile_loop,
        bench_e2e_compile_float,
        bench_e2e_compile_mem,
        bench_e2e_compile_spill,
        bench_e2e_compile_big_loop,
        bench_e2e_jit_execute,
);

criterion_group!(
    name = throughput;
    config = Criterion::default();
    targets = bench_throughput,
);

criterion_group!(
    name = code_size;
    config = Criterion::default();
    targets = bench_code_size_metrics,
);

criterion_group!(
    name = comparison;
    config = Criterion::default();
    targets = bench_comparison,
);

criterion_main!(
    ir_build,
    ir_parse,
    optimizations,
    pipeline_breakdown,
    codegen,
    verify,
    module,
    end_to_end,
    throughput,
    code_size,
    comparison
);

// ============================================================
// Sample validation tests (run via `cargo test --bench compile_bench`)
//
// Guard the IR text samples and any generated sources used by benchmarks:
// every sample must parse and must contain real instructions (the old
// `{ entry: ret i32 }` shells parsed to empty bodies and measured nothing).
// ============================================================

#[cfg(test)]
mod tests {
    #[test]
    fn ir_text_samples_parse_with_instructions() {
        for (name, src) in [
            ("IR_SIMPLE_ADD", super::IR_SIMPLE_ADD),
            ("IR_MUL_ADD", super::IR_MUL_ADD),
            ("IR_DOT_PRODUCT", super::IR_DOT_PRODUCT),
            ("IR_LOOP_SUM", super::IR_LOOP_SUM),
            ("IR_COMPLEX_TEXT", super::IR_COMPLEX_TEXT),
        ] {
            let f = forge_ir::ir_parser::parse_function(src)
                .unwrap_or_else(|e| panic!("{name} failed to parse: {e}"));
            assert!(
                super::count_ir_insts(&f) >= 2,
                "{name}: expected real instructions, got {}",
                super::count_ir_insts(&f)
            );
        }
    }

    #[test]
    fn ir_text_many_ops_generated_scales() {
        let src = super::ir_text_many_ops(super::IR_BIG_TEXT_N);
        let f = forge_ir::ir_parser::parse_function(&src)
            .unwrap_or_else(|e| panic!("big text failed to parse: {e}"));
        assert!(
            super::count_ir_insts(&f) >= super::IR_BIG_TEXT_N,
            "expected >= {} insts, got {}",
            super::IR_BIG_TEXT_N,
            super::count_ir_insts(&f)
        );
    }
}
