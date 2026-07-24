//! Compilation pipeline performance benchmarks.
//!
//! Benchmarks covering the full codegen pipeline:
//! - IR construction (building complex functions programmatically)
//! - IR parsing (parsing .ll-like textual IR)
//! - Optimization pipeline (running O2 passes)
//! - Code generation (lowering + register allocation + emit for x86_64)
//! - End-to-end JIT compilation
//!
//! Run with: `cargo bench`
//! Run a single group: `cargo bench -- ir_parse`
//! Compare against baseline: `cargo bench -- --save-baseline base`

use codegen_lib::backend::x86_64::{X86Isa, ensure_registered};
use codegen_lib::backend::FunctionCompiler;
use codegen_lib::ir::*;
use codegen_lib::optimize::*;
use criterion::{BenchmarkId, Criterion, Throughput, criterion_group, criterion_main};
use std::hint::black_box;

// ============================================================
// Benchmark helpers -- function builders
// ============================================================

/// Build a simple two-argument add function: fn(a: i32, b: i32) -> i32 { a + b }
fn build_simple_add() -> Function {
    let sig = Signature::new(&[(Type::I32, "a"), (Type::I32, "b")], &[Type::I32]);
    let mut b = FunctionBuilder::new("add", sig);
    let (entry, params) = b.create_block_with_params(&[(Type::I32, "a"), (Type::I32, "b")]);
    b.switch_to_block(entry);
    let sum = b.iadd(params[0], params[1]);
    b.return_(&[sum]);
    b.finish()
}

/// Build a function with ~20 arithmetic instructions
fn build_many_ops() -> Function {
    let sig = Signature::new(&[], &[Type::I32]);
    let mut b = FunctionBuilder::new("many_ops", sig);
    let entry = b.create_block();
    b.switch_to_block(entry);
    let v1 = b.iconst(1, Type::I32);
    let v2 = b.iconst(2, Type::I32);
    let mut acc = b.iadd(v1, v2);
    for _ in 0..19 {
        let x = b.iconst(3, Type::I32);
        acc = b.iadd(acc, x);
        acc = b.imul(acc, v1);
    }
    b.return_(&[acc]);
    b.finish()
}

/// Build a moderately complex function with mixed integer/float ops and branches.
fn build_complex_function() -> Function {
    // fn complex(a: i32, b: f64) -> f64 {
    //     let x = a * a;
    //     let y = b * b;
    //     if x > 10 { y + 1.0 } else { y - 1.0 }
    // }
    let sig = Signature::new(&[(Type::I32, "a"), (Type::F64, "b")], &[Type::F64]);
    let mut b = FunctionBuilder::new("complex", sig);

    let (entry, params) =
        b.create_block_with_params(&[(Type::I32, "a"), (Type::F64, "b")]);
    let then_block = b.create_block();
    let else_block = b.create_block();
    let merge_block = b.create_block();

    b.switch_to_block(entry);
    let a = params[0];
    let b_val = params[1];
    let x = b.imul(a, a);
    let f_b = b.fmul(b_val, b_val);
    let ten = b.iconst(10, Type::I32);
    let cond = b.icmp(IntCC::SignedGreaterThan, x, ten);
    b.branch(cond, then_block, else_block, &[], &[]);

    let one_bits = 1.0_f64.to_bits();
    b.switch_to_block(then_block);
    let one = b.fconst(one_bits, Type::F64);
    let ty = b.fadd(f_b, one);
    b.jump(merge_block, &[ty]);

    b.switch_to_block(else_block);
    let one2 = b.fconst(one_bits, Type::F64);
    let fy = b.fsub(f_b, one2);
    b.jump(merge_block, &[fy]);

    b.switch_to_block(merge_block);
    b.return_(&[fy]); // NOTE: phi not constructed here; simplified for bench

    b.finish()
}

/// Build a function with multiple blocks to stress CFG-based optimizations.
fn build_multi_block_function() -> Function {
    // fn sum_to_n(n: i32) -> i32 {
    //     let mut i = 0;
    //     let mut sum = 0;
    //     loop {
    //         if i >= n { break; }
    //         sum += i;
    //         i += 1;
    //     }
    //     return sum;
    // }
    let sig = Signature::new(&[(Type::I32, "n")], &[Type::I32]);
    let mut b = FunctionBuilder::new("sum_to_n", sig);
    let (entry, params) = b.create_block_with_params(&[(Type::I32, "n")]);
    let header = b.create_block();
    let body = b.create_block();
    let exit = b.create_block();

    b.switch_to_block(entry);
    let n = params[0];
    let zero = b.iconst(0, Type::I32);
    let one = b.iconst(1, Type::I32);
    b.jump(header, &[zero, zero]);

    b.switch_to_block(header);
    // header block uses params: (i, sum)
    let i = b.iconst(0, Type::I32); // simplified: use immediate instead of phi
    let sum = b.iconst(0, Type::I32);
    let cond = b.icmp(IntCC::SignedLessThan, i, n);
    b.branch(cond, body, exit, &[], &[]);

    b.switch_to_block(body);
    let next_sum = b.iadd(sum, i);
    let next_i = b.iadd(i, one);
    b.jump(header, &[next_i, next_sum]);

    b.switch_to_block(exit);
    b.return_(&[sum]);

    b.finish()
}

/// Produce a sample LLVM-IR-like text for parsing benchmarks.
fn sample_llvm_ir_text() -> &'static str {
    r#"
define i32 @add(i32 %a, i32 %b) {
entry:
  %sum = add i32 %a, %b
  ret i32 %sum
}

define i32 @mul_add(i32 %a, i32 %b, i32 %c) {
entry:
  %prod = mul i32 %a, %b
  %result = add i32 %prod, %c
  ret i32 %result
}

define double @dot_product(double %x1, double %y1, double %x2, double %y2) {
entry:
  %dx = fsub double %x2, %x1
  %dy = fsub double %y2, %y1
  %dx2 = fmul double %dx, %dx
  %dy2 = fmul double %dy, %dy
  %sum = fadd double %dx2, %dy2
  ret double %sum
}
"#
}

// ============================================================
// Group 1: IR Construction Benchmarks
// ============================================================

fn bench_ir_build_simple(c: &mut Criterion) {
    c.bench_function("ir_build_simple_add", |bencher| {
        bencher.iter(|| {
            let func = black_box(build_simple_add());
            black_box(func);
        });
    });
}

fn bench_ir_build_many_ops(c: &mut Criterion) {
    c.bench_function("ir_build_many_ops", |bencher| {
        bencher.iter(|| {
            let func = black_box(build_many_ops());
            black_box(func);
        });
    });
}

fn bench_ir_build_complex(c: &mut Criterion) {
    c.bench_function("ir_build_complex", |bencher| {
        bencher.iter(|| {
            let func = black_box(build_complex_function());
            black_box(func);
        });
    });
}

fn bench_ir_build_multi_block(c: &mut Criterion) {
    c.bench_function("ir_build_multi_block", |bencher| {
        bencher.iter(|| {
            let func = black_box(build_multi_block_function());
            black_box(func);
        });
    });
}

// ============================================================
// Group 2: IR Parsing Benchmarks
// ============================================================

fn bench_ir_parse_simple(c: &mut Criterion) {
    let text = "define i32 @add(i32 %a, i32 %b) {\nentry:\n  %sum = add i32 %a, %b\n  ret i32 %sum\n}\n";
    c.bench_function("ir_parse_simple_add", |bencher| {
        bencher.iter(|| {
            let mut parser = LlvmIrParser::new();
            let module = parser.parse(black_box(text)).unwrap();
            black_box(module);
        });
    });
}

fn bench_ir_parse_complex(c: &mut Criterion) {
    let text = sample_llvm_ir_text();
    c.bench_function("ir_parse_multi_func", |bencher| {
        bencher.iter(|| {
            let mut parser = LlvmIrParser::new();
            let module = parser.parse(black_box(text)).unwrap();
            black_box(module);
        });
    });
}

// ============================================================
// Group 3: Optimization Pipeline Benchmarks
// ============================================================

fn bench_opt_const_fold(c: &mut Criterion) {
    let func = build_many_ops();
    let pass = ConstFoldPass::new();
    c.bench_function("opt_const_fold", |bencher| {
        bencher.iter(|| {
            let mut f = black_box(func.clone());
            let result = pass.run_on_function(black_box(&mut f)).unwrap();
            black_box(result);
        });
    });
}

fn bench_opt_dce(c: &mut Criterion) {
    let func = build_many_ops();
    let pass = DeadCodeElimPass::new();
    c.bench_function("opt_dead_code_elim", |bencher| {
        bencher.iter(|| {
            let mut f = black_box(func.clone());
            let result = pass.run_on_function(black_box(&mut f)).unwrap();
            black_box(result);
        });
    });
}

fn bench_opt_copy_prop(c: &mut Criterion) {
    let func = build_many_ops();
    let pass = CopyPropPass::new();
    c.bench_function("opt_copy_prop", |bencher| {
        bencher.iter(|| {
            let mut f = black_box(func.clone());
            let result = pass.run_on_function(black_box(&mut f)).unwrap();
            black_box(result);
        });
    });
}

fn bench_opt_gvn(c: &mut Criterion) {
    let func = build_many_ops();
    let pass = GvnPass::new();
    c.bench_function("opt_gvn", |bencher| {
        bencher.iter(|| {
            let mut f = black_box(func.clone());
            let result = pass.run_on_function(black_box(&mut f)).unwrap();
            black_box(result);
        });
    });
}

fn bench_opt_jump_thread(c: &mut Criterion) {
    let func = build_complex_function();
    let pass = JumpThreadPass::new();
    c.bench_function("opt_jump_thread", |bencher| {
        bencher.iter(|| {
            let mut f = black_box(func.clone());
            let result = pass.run_on_function(black_box(&mut f)).unwrap();
            black_box(result);
        });
    });
}

fn bench_opt_pipeline_o0(c: &mut Criterion) {
    let func = build_complex_function();
    let pm = PassManager::for_level(OptimizationLevel::O0);
    c.bench_function("opt_pipeline_o0", |bencher| {
        bencher.iter(|| {
            let mut f = black_box(func.clone());
            let result = pm.run_on_function(black_box(&mut f)).unwrap();
            black_box(result);
        });
    });
}

fn bench_opt_pipeline_o1(c: &mut Criterion) {
    let func = build_complex_function();
    let pm = PassManager::for_level(OptimizationLevel::O1);
    c.bench_function("opt_pipeline_o1", |bencher| {
        bencher.iter(|| {
            let mut f = black_box(func.clone());
            let result = pm.run_on_function(black_box(&mut f)).unwrap();
            black_box(result);
        });
    });
}

fn bench_opt_pipeline_o2(c: &mut Criterion) {
    let func = build_complex_function();
    let pm = PassManager::for_level(OptimizationLevel::O2);
    c.bench_function("opt_pipeline_o2", |bencher| {
        bencher.iter(|| {
            let mut f = black_box(func.clone());
            let result = pm.run_on_function(black_box(&mut f)).unwrap();
            black_box(result);
        });
    });
}

fn bench_opt_pipeline_loop(c: &mut Criterion) {
    let func = build_multi_block_function();
    let pm = PassManager::for_level(OptimizationLevel::O2);
    c.bench_function("opt_pipeline_loop_func", |bencher| {
        bencher.iter(|| {
            let mut f = black_box(func.clone());
            let result = pm.run_on_function(black_box(&mut f)).unwrap();
            black_box(result);
        });
    });
}

// ============================================================
// Group 4: Code Generation Benchmarks (x86_64)
// ============================================================

fn bench_codegen_simple_add(c: &mut Criterion) {
    ensure_registered();
    let func = build_simple_add();
    c.bench_function("codegen_simple_add", |bencher| {
        bencher.iter(|| {
            let compiled = FunctionCompiler::<X86Isa>::compile_raw(black_box(&func)).unwrap();
            black_box(compiled);
        });
    });
}

fn bench_codegen_many_ops(c: &mut Criterion) {
    ensure_registered();
    let func = build_many_ops();
    c.bench_function("codegen_many_ops", |bencher| {
        bencher.iter(|| {
            let compiled = FunctionCompiler::<X86Isa>::compile_raw(black_box(&func)).unwrap();
            black_box(compiled);
        });
    });
}

fn bench_codegen_complex(c: &mut Criterion) {
    ensure_registered();
    let func = build_complex_function();
    c.bench_function("codegen_complex", |bencher| {
        bencher.iter(|| {
            let compiled = FunctionCompiler::<X86Isa>::compile_raw(black_box(&func)).unwrap();
            black_box(compiled);
        });
    });
}

fn bench_codegen_multi_block(c: &mut Criterion) {
    ensure_registered();
    let func = build_multi_block_function();
    c.bench_function("codegen_multi_block", |bencher| {
        bencher.iter(|| {
            let compiled = FunctionCompiler::<X86Isa>::compile_raw(black_box(&func)).unwrap();
            black_box(compiled);
        });
    });
}

fn bench_codegen_with_o2(c: &mut Criterion) {
    ensure_registered();
    let func = build_many_ops();
    let pm = PassManager::for_level(OptimizationLevel::O2);
    c.bench_function("codegen_with_o2", |bencher| {
        bencher.iter(|| {
            let compiled = FunctionCompiler::<X86Isa>::compile_with_passes(
                black_box(&func), black_box(&pm),
            )
            .unwrap();
            black_box(compiled);
        });
    });
}

fn bench_codegen_with_o2_complex(c: &mut Criterion) {
    ensure_registered();
    let func = build_complex_function();
    let pm = PassManager::for_level(OptimizationLevel::O2);
    c.bench_function("codegen_with_o2_complex", |bencher| {
        bencher.iter(|| {
            let compiled = FunctionCompiler::<X86Isa>::compile_with_passes(
                black_box(&func), black_box(&pm),
            )
            .unwrap();
            black_box(compiled);
        });
    });
}

// ============================================================
// Group 5: End-to-End Compilation Pipeline
// ============================================================

fn bench_e2e_compile_simple(c: &mut Criterion) {
    ensure_registered();
    let func = build_simple_add();
    let pm = PassManager::for_level(OptimizationLevel::O2);
    c.bench_function("e2e_compile_simple", |bencher| {
        bencher.iter(|| {
            let compiled = FunctionCompiler::<X86Isa>::compile_with_passes(
                black_box(&func), black_box(&pm),
            )
            .unwrap();
            black_box(compiled);
        });
    });
}

fn bench_e2e_compile_complex(c: &mut Criterion) {
    ensure_registered();
    let func = build_complex_function();
    let pm = PassManager::for_level(OptimizationLevel::O2);
    c.bench_function("e2e_compile_complex", |bencher| {
        bencher.iter(|| {
            let compiled = FunctionCompiler::<X86Isa>::compile_with_passes(
                black_box(&func), black_box(&pm),
            )
            .unwrap();
            black_box(compiled);
        });
    });
}

fn bench_e2e_compile_loop(c: &mut Criterion) {
    ensure_registered();
    let func = build_multi_block_function();
    let pm = PassManager::for_level(OptimizationLevel::O2);
    c.bench_function("e2e_compile_loop", |bencher| {
        bencher.iter(|| {
            let compiled = FunctionCompiler::<X86Isa>::compile_with_passes(
                black_box(&func), black_box(&pm),
            )
            .unwrap();
            black_box(compiled);
        });
    });
}

fn bench_e2e_jit_execute(c: &mut Criterion) {
    use codegen_lib::executable_memory::ExecutableMemory;
    ensure_registered();
    let func = build_simple_add();
    c.bench_function("e2e_jit_execute", |bencher| {
        bencher.iter(|| {
            let compiled = FunctionCompiler::<X86Isa>::compile_raw(black_box(&func)).unwrap();
            let mem = ExecutableMemory::new(&compiled.code).unwrap();
            let f: extern "C" fn(i32, i32) -> i32 = unsafe { mem.get_fn(0).unwrap() };
            let result = black_box(f(3, 4));
            assert_eq!(result, 7);
        });
    });
}

// ============================================================
// Group 6: Throughput / Size Benchmarking (parameterized)
// ============================================================

/// Build a function with `n` arithmetic instructions.
fn build_scale_ops(n: usize) -> Function {
    let sig = Signature::new(&[], &[Type::I32]);
    let mut b = FunctionBuilder::new("scale_ops", sig);
    let entry = b.create_block();
    b.switch_to_block(entry);
    let v = b.iconst(1, Type::I32);
    let mut acc = v;
    let one = b.iconst(1, Type::I32);
    for _ in 0..n {
        acc = b.iadd(acc, one);
        acc = b.imul(acc, v);
        acc = b.iadd(acc, one);
    }
    b.return_(&[acc]);
    b.finish()
}

fn bench_throughput_codegen(c: &mut Criterion) {
    ensure_registered();
    let mut group = c.benchmark_group("throughput_codegen");
    for &n in &[10usize, 50, 100, 200] {
        let func = build_scale_ops(n);
        let inst_count = func.blocks.iter().map(|b| b.instructions.len()).sum::<usize>();
        group.throughput(Throughput::Elements(inst_count as u64));
        group.bench_with_input(
            BenchmarkId::from_parameter(format!("{}ops_{}insts", n, inst_count)),
            &func,
            |bencher, func| {
                bencher.iter(|| {
                    let compiled =
                        FunctionCompiler::<X86Isa>::compile_raw(black_box(func)).unwrap();
                    black_box(compiled);
                });
            },
        );
    }
    group.finish();
}

fn bench_throughput_optimize(c: &mut Criterion) {
    let mut group = c.benchmark_group("throughput_optimize");
    for &n in &[10usize, 50, 100, 200] {
        let func = build_scale_ops(n);
        let inst_count = func.blocks.iter().map(|b| b.instructions.len()).sum::<usize>();
        let pm = PassManager::for_level(OptimizationLevel::O2);
        group.throughput(Throughput::Elements(inst_count as u64));
        group.bench_with_input(
            BenchmarkId::from_parameter(format!("{}ops_{}insts", n, inst_count)),
            &(func, pm),
            |bencher, (func, pm)| {
                bencher.iter(|| {
                    let mut f = black_box(func.clone());
                    let result = pm.run_on_function(black_box(&mut f)).unwrap();
                    black_box(result);
                });
            },
        );
    }
    group.finish();
}

fn bench_code_size_metrics(c: &mut Criterion) {
    ensure_registered();
    let mut group = c.benchmark_group("code_size");

    let funcs: Vec<(&str, Function)> = vec![
        ("simple_add", build_simple_add()),
        ("many_ops", build_many_ops()),
        ("complex", build_complex_function()),
        ("multi_block", build_multi_block_function()),
    ];

    for (name, func) in &funcs {
        let compiled =
            FunctionCompiler::<X86Isa>::compile_raw(func).expect("compile failed");
        let code_bytes = compiled.code.len();
        let ir_insts = func.blocks.iter().map(|b| b.instructions.len()).sum::<usize>();
        group.bench_function(
            format!("code_size_{}", name),
            |bencher| {
                bencher.iter(|| {
                    black_box(code_bytes);
                    black_box(ir_insts);
                });
            },
        );
    }
    group.finish();
}

// ============================================================
// Group 7: Comparison Framework -- structured output
// ============================================================

/// Runs all comparison benchmarks and prints a structured report.
/// This is a single combined benchmark that reports results in a
/// table format suitable for comparison with other backends (LLVM, etc.).
fn bench_comparison_report(c: &mut Criterion) {
    ensure_registered();
    let mut group = c.benchmark_group("comparison");

    let test_cases: Vec<(&str, Function)> = vec![
        ("add", build_simple_add()),
        ("many_ops", build_many_ops()),
        ("complex", build_complex_function()),
        ("loop", build_multi_block_function()),
    ];

    for (name, func) in &test_cases {
        let pm = PassManager::for_level(OptimizationLevel::O2);

        // Optimization time
        group.bench_function(format!("comparison_opt_{}", name), |bencher| {
            bencher.iter(|| {
                let mut f = black_box(func.clone());
                let result = pm.run_on_function(black_box(&mut f)).unwrap();
                black_box(result);
            });
        });

        // Codegen time (after optimization)
        group.bench_function(format!("comparison_codegen_{}", name), |bencher| {
            let mut f = func.clone();
            let _ = pm.run_on_function(&mut f);
            bencher.iter(|| {
                let compiled =
                    FunctionCompiler::<X86Isa>::compile_raw(black_box(&f)).unwrap();
                black_box(compiled);
            });
        });

        // Code size
        let mut f = func.clone();
        let _ = pm.run_on_function(&mut f);
        let compiled = FunctionCompiler::<X86Isa>::compile_raw(&f).unwrap();
        let code_size = compiled.code.len();
        group.bench_function(format!("comparison_size_{}", name), |bencher| {
            bencher.iter(|| black_box(code_size));
        });
    }

    group.finish();
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
);

criterion_group!(
    name = ir_parse;
    config = Criterion::default();
    targets =
        bench_ir_parse_simple,
        bench_ir_parse_complex,
);

criterion_group!(
    name = optimizations;
    config = Criterion::default();
    targets =
        bench_opt_const_fold,
        bench_opt_dce,
        bench_opt_copy_prop,
        bench_opt_gvn,
        bench_opt_jump_thread,
        bench_opt_pipeline_o0,
        bench_opt_pipeline_o1,
        bench_opt_pipeline_o2,
        bench_opt_pipeline_loop,
);

criterion_group!(
    name = codegen;
    config = Criterion::default();
    targets =
        bench_codegen_simple_add,
        bench_codegen_many_ops,
        bench_codegen_complex,
        bench_codegen_multi_block,
        bench_codegen_with_o2,
        bench_codegen_with_o2_complex,
);

criterion_group!(
    name = end_to_end;
    config = Criterion::default();
    targets =
        bench_e2e_compile_simple,
        bench_e2e_compile_complex,
        bench_e2e_compile_loop,
        bench_e2e_jit_execute,
);

criterion_group!(
    name = throughput;
    config = Criterion::default();
    targets =
        bench_throughput_codegen,
        bench_throughput_optimize,
        bench_code_size_metrics,
);

criterion_group!(
    name = comparison;
    config = Criterion::default();
    targets =
        bench_comparison_report,
);

// Run all benchmarks by default
criterion_main!(
    ir_build,
    ir_parse,
    optimizations,
    codegen,
    end_to_end,
    throughput,
    comparison,
);
