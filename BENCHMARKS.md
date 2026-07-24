# Benchmarks & Comparison Framework

Performance benchmarks for the codegen-lib compilation pipeline.

## Running Benchmarks

```bash
# Run all benchmarks
cargo bench

# Run a specific group
cargo bench -- ir_build       # IR construction
cargo bench -- ir_parse       # IR parsing
cargo bench -- optimizations  # Individual optimization passes
cargo bench -- codegen        # Code generation (x86_64)
cargo bench -- end_to_end     # Full compilation pipeline
cargo bench -- throughput     # Parameterized throughput benchmarks
cargo bench -- comparison     # Comparison framework

# Save a baseline
cargo bench -- --save-baseline main

# Compare against baseline
cargo bench -- --baseline main

# Generate HTML report (requires criterion html_reports feature)
cargo bench -- --plotting-backend gnuplot
# Report at: target/criterion/report/index.html
```

## Benchmark Groups

### 1. IR Build (`ir_build`)

Measures the cost of constructing IR functions programmatically via `FunctionBuilder`.

| Benchmark | Description |
|-----------|-------------|
| `ir_build_simple_add` | Build `fn(a:i32,b:i32)->i32 { a+b }` |
| `ir_build_many_ops` | Build function with 20 arithmetic ops |
| `ir_build_complex` | Build function with mixed int/float and branches |
| `ir_build_multi_block` | Build multi-block loop function |

### 2. IR Parse (`ir_parse`)

Measures LLVM-IR text parsing speed for interoperability.

| Benchmark | Description |
|-----------|-------------|
| `ir_parse_simple_add` | Parse a single `add` function |
| `ir_parse_multi_func` | Parse 3 functions (add, mul_add, dot_product) |

### 3. Optimizations (`optimizations`)

Measures individual pass performance and full pipeline timings.

| Benchmark | Description |
|-----------|-------------|
| `opt_const_fold` | Constant folding on ~20-op function |
| `opt_dead_code_elim` | Dead code elimination |
| `opt_copy_prop` | Copy propagation |
| `opt_gvn` | Global value numbering (cross-block CSE subset) |
| `opt_jump_thread` | Jump threading on branching function |
| `opt_pipeline_o0` | Full O0 pipeline (copy-prop only) |
| `opt_pipeline_o1` | Full O1 pipeline (const-fold + DCE + copy-prop) |
| `opt_pipeline_o2` | Full O2 pipeline (const-fold + GVN + LICM + jump-thread + DCE + copy-prop) |
| `opt_pipeline_loop_func` | O2 pipeline on a loop-based function |

### 4. Code Generation (`codegen`)

Measures lowering, register allocation, and x86_64 machine code emission.

| Benchmark | Description |
|-----------|-------------|
| `codegen_simple_add` | Compile small `add` function |
| `codegen_many_ops` | Compile 20-op function |
| `codegen_complex` | Compile branching float function |
| `codegen_multi_block` | Compile loop function |
| `codegen_with_o2` | O2 optimize then compile |
| `codegen_with_o2_complex` | O2 optimize then compile complex function |

### 5. End-to-End (`end_to_end`)

Full compilation from IR through code emission, including JIT execution.

| Benchmark | Description |
|-----------|-------------|
| `e2e_compile_simple` | O2 + compile simple add |
| `e2e_compile_complex` | O2 + compile complex function |
| `e2e_compile_loop` | O2 + compile loop function |
| `e2e_jit_execute` | JIT compile and execute `add(3, 4)` |

### 6. Throughput (`throughput`)

Parameterized benchmarks measuring scalability.

| Benchmark | Description |
|-----------|-------------|
| `throughput_codegen/N` | Codegen time for functions with N ops |
| `throughput_optimize/N` | Optimization time for functions with N ops |
| `code_size/*` | Code size metrics for each test function |

### 7. Comparison (`comparison`)

Structured report comparing optimization time, codegen time, and code size across test cases. Use this to compare codegen-lib against other backends (LLVM, Cranelift, etc.).

---

## LLVM vs. codegen-lib Comparison Template

Use the table below to record performance comparisons. All tests are run on
the same machine, same input functions, at equivalent optimization levels.

### Environment

| Property | Value |
|----------|-------|
| CPU | `______` |
| RAM | `______` |
| OS | `______` |
| Rust version | `______` |
| LLVM version | `______` |
| codegen-lib commit | `______` |

### Compilation Time (microseconds, lower is better)

Run with: `cargo bench -- comparison`

| Test Case | IR Instructions | codegen-lib Opt | codegen-lib Codegen | codegen-lib Total | LLVM Total | Ratio |
|-----------|---------------:|----------------:|--------------------:|------------------:|-----------:|------:|
| simple_add | `____` | `____` | `____` | `____` | `____` | `____` |
| many_ops | `____` | `____` | `____` | `____` | `____` | `____` |
| complex | `____` | `____` | `____` | `____` | `____` | `____` |
| loop | `____` | `____` | `____` | `____` | `____` | `____` |

### Code Size (bytes, lower is better)

| Test Case | IR Instructions | codegen-lib Size | LLVM -O0 | LLVM -O2 | Notes |
|-----------|---------------:|-----------------:|---------:|---------:|-------|
| simple_add | `____` | `____` | `____` | `____` | |
| many_ops | `____` | `____` | `____` | `____` | |
| complex | `____` | `____` | `____` | `____` | |
| loop | `____` | `____` | `____` | `____` | |

### Optimization Pass Detail (microseconds)

| Pass | simple_add | many_ops | complex | loop |
|------|-----------:|---------:|--------:|-----:|
| O0 pipeline | `____` | `____` | `____` | `____` |
| O1 pipeline | `____` | `____` | `____` | `____` |
| O2 pipeline | `____` | `____` | `____` | `____` |
| ConstFold only | `____` | `____` | `____` | `____` |
| GVN only | `____` | `____` | `____` | `____` |
| DCE only | `____` | `____` | `____` | `____` |
| CopyProp only | `____` | `____` | `____` | `____` |

### Throughput Scaling

| Operation Count | Codegen Time (us) | Opt Time (us) | Code Size (bytes) |
|----------------:|------------------:|--------------:|------------------:|
| 10 | `____` | `____` | `____` |
| 50 | `____` | `____` | `____` |
| 100 | `____` | `____` | `____` |
| 200 | `____` | `____` | `____` |

### Notes

- LLVM measurements use `llc -O2` (or `clang -O2 -c`) on equivalent C/Rust input.
- codegen-lib measurements from `cargo bench` Criterion reports in `target/criterion/`.
- All times are median wall-clock time in microseconds.
- LLVM total = IR parse + optimization + codegen (assembly emission).
- codegen-lib total = IR build + optimization + lowering + register allocation + emit.

---

## Getting LLVM Baseline Numbers

```bash
# Create equivalent C file and compile
cat > /tmp/test.c << 'EOF'
// simple_add equivalent
int add(int a, int b) { return a + b; }
EOF

# Measure with hyperfine
hyperfine --warmup 10 'clang -O2 -c /tmp/test.c -o /dev/null'

# Or use llc directly with LLVM IR
echo '
define i32 @add(i32 %a, i32 %b) {
  %sum = add i32 %a, %b
  ret i32 %sum
}
' > /tmp/test.ll
hyperfine --warmup 10 'llc -O2 /tmp/test.ll -o /dev/null'
```
