# Benchmarks & Comparison Framework

Performance benchmarks for the code-forge compilation pipeline.

## Running Benchmarks

```bash
# Run all benchmarks
cargo bench --bench compile_bench

# Run a specific group
cargo bench --bench compile_bench -- ir_build       # IR construction
cargo bench --bench compile_bench -- ir_parse       # IR text parsing
cargo bench --bench compile_bench -- optimizations  # Optimization passes & pipelines
cargo bench --bench compile_bench -- pipeline_breakdown  # Per-pass pipeline cost
cargo bench --bench compile_bench -- codegen        # Code generation (x86_64 + other ISAs)
cargo bench --bench compile_bench -- verify         # IR verification
cargo bench --bench compile_bench -- module         # Module-level compile + IPA with real table
cargo bench --bench compile_bench -- end_to_end     # Full compilation pipeline
cargo bench --bench compile_bench -- throughput     # Parameterized throughput benchmarks
cargo bench --bench compile_bench -- code_size      # Code size metrics
cargo bench --bench compile_bench -- comparison     # Comparison framework

# Run everything except the JIT-execution benchmark (if you want to skip it)
cargo bench --bench compile_bench -- '^(ir_build|ir_parse|optimizations|codegen|throughput|code_size|comparison|end_to_end/e2e_compile_)'

# Run the whole suite including JIT execution (works since the JIT correctness fix)
cargo bench --bench compile_bench -- '^(ir_build|ir_parse|optimizations|codegen|throughput|code_size|comparison|end_to_end)'

# Save a baseline
cargo bench --bench compile_bench -- --save-baseline main

# Compare against baseline
cargo bench --bench compile_bench -- --baseline main

# Generate HTML report (requires criterion html_reports feature)
cargo bench --bench compile_bench -- --plotting-backend gnuplot
# Report at: target/criterion/report/index.html
```

## Benchmark Groups

### 1. IR Build (`ir_build`)

Measures the cost of constructing IR functions programmatically via `FunctionBuilder`.

| Benchmark | Description |
| --------- | ----------- |
| `ir_build_simple_add` | Build `fn(a:i32,b:i32)->i32 { a+b }` |
| `ir_build_many_ops` | Build function with 60 arithmetic ops |
| `ir_build_complex` | Build function with mixed int/float and branches |
| `ir_build_multi_block` | Build multi-block loop function |
| `ir_build_float` | Build F64 multiply-accumulate chain |
| `ir_build_mem` | Build store/load-heavy function (20 rounds) |
| `ir_build_call` | Build function with an external `call @0` site |
| `ir_build_spill_pressure` | Build 12 live constants + balanced add tree |
| `ir_build_big_loop` | Build loop with a 20-op body |

### 2. IR Parse (`ir_parse`)

Measures forge-ir text-format parsing — **LLVM IR syntax** (logos lexer +
lalrpop parser, `define`/`@`/typed operands/`;` comments). The samples are
real LLVM IR functions (`define i32 @add(i32 %a, i32 %b) { %entry: %s =
add i32 %a, i32 %b; ret i32 %s }`); `multi_func` parses several in sequence.

| Benchmark | Description |
| --------- | ----------- |
| `ir_parse_simple_add` | Parse 2-instr `add` LLVM function |
| `ir_parse_mul_add` | Parse 3-instr `mul_add` |
| `ir_parse_dot_product` | Parse 4-block branching function |
| `ir_parse_loop_sum` | Parse multi-block loop with icmp/br |
| `ir_parse_complex` | Parse float + branch function |
| `ir_parse_multi_func` | Parse 4 functions in sequence |
| `ir_parse_big_text_256` | Parse generated 256-instr single-block text |

**Scaling**: the logos+lalrpop rewrite (2026-08-03) parses near-linearly;
256-instr text takes ~370 µs (old forge-grammar lexer was O(n²), 300 insts
would have taken ~5.6 h).

### 3. Optimizations (`optimizations`)

Measures individual pass performance and full pipeline timings. Pass benchmarks
use `iter_batched` so IR construction time is excluded from measurement.

| Benchmark | Description |
| --------- | ----------- |
| `opt_const_fold` | Constant folding on ~60-op function |
| `opt_const_fold_float` | Constant folding on F64 chain (Big float paths) |
| `opt_dead_code_elim` | Dead code elimination |
| `opt_copy_prop` | Copy propagation |
| `opt_cse` | Common subexpression elimination |
| `opt_gvn` | Global value numbering |
| `opt_gvn_pre` | GVN-PRE (partial redundancy elimination) |
| `opt_sccp` | Sparse conditional constant propagation |
| `opt_jump_thread` | Jump threading on branching function |
| `opt_mem2reg` | Memory to register promotion on store/load function |
| `opt_licm` | Loop invariant code motion on loop function |
| `opt_ind_var_simplify` | Induction variable simplification on loop function |
| `opt_loop_unroll` | Loop unrolling on loop function |
| `opt_block_param_coalesce` | Block parameter coalescing on loop function |
| `opt_egraph` | E-graph equality saturation |
| `opt_pgo` | PGO instrumentation pass |
| `opt_isel` | E-graph instruction selection pass |
| `opt_inline_empty_table` | Inlining pass with empty function table |
| `opt_tail_call_empty_table` | Tail-call pass with empty function table |
| `opt_func_specialize_empty_table` | Function specialization with empty table |
| `opt_lto_empty_module` | LTO pass on an empty module |
| `opt_pipeline_o0` | Full O0 pipeline (empty pass list) |
| `opt_pipeline_o1` | Full O1 pipeline (const-fold, copy-prop, CSE, DCE, jump-thread) |
| `opt_pipeline_o2` | Full O2 pipeline (O1 + GVN, GVN-PRE, SCCP, LICM, tail-call, egraph, block-param coalesce) |
| `opt_pipeline_o3` | Full O3 pipeline (O2 + inline, mem2reg, ind-var simplify, loop unroll) |
| `opt_pipeline_loop_func` | O2 pipeline on a loop-based function |
| `opt_pipeline_o1_loop_func` | O1 pipeline on a loop-based function |
| `opt_pipeline_o3_loop_func` | O3 pipeline on a loop-based function |

### 3b. Pipeline Pass Breakdown (`pipeline_breakdown`)

Per-pass cost within each optimization level, measured on a function shape that
exercises the pass (mirrors `PassManager::for_level` pass lists). Summing a
level's rows approximates its pipeline cost; the rows identify which pass
dominates.

| Benchmark | Description |
| --------- | ----------- |
| `pipeline_breakdown/o1/{const_fold,copy_prop,cse,dead_code,jump_thread}` | O1 passes |
| `pipeline_breakdown/o2/{gvn,gvn_pre,sccp,block_param_coalesce,licm,tail_call,egraph}` | O2 passes |
| `pipeline_breakdown/o3/{inline,mem2reg,ind_var_simplify,loop_unroll}` | O3 passes |

Measurement caveat: this group runs right after the slow `ir_parse` group, so
its absolute numbers skew high on this machine (up to ~2.5×); relative ranking
within the group is reliable.

### 4. Code Generation (`codegen`)

Measures lowering, register allocation, and machine code emission.

| Benchmark | Description |
| --------- | ----------- |
| `codegen_simple_add` | Compile small `add` function (x86_64) |
| `codegen_many_ops` | Compile 60-op function (x86_64) |
| `codegen_complex` | Compile branching float function (x86_64) |
| `codegen_multi_block` | Compile loop function (x86_64) |
| `codegen_float` | Compile F64 chain (x86_64) |
| `codegen_mem` | Compile store/load function (x86_64) |
| `codegen_spill_pressure` | Compile spill-heavy function (x86_64) |
| `codegen_big_loop` | Compile big-loop function (x86_64) |
| `codegen_with_o1` | O1 optimize then compile (x86_64) |
| `codegen_with_o2` | O2 optimize then compile (x86_64) |
| `codegen_with_o2_complex` | O2 optimize then compile complex (x86_64) |
| `codegen/aarch64/{case}` | Compile simple_add/many_ops/multi_block/complex with aarch64 |
| `codegen/riscv64/{case}` | Compile simple_add/many_ops/multi_block/complex with riscv64 |
| `codegen/wasm32/{case}` | Compile simple_add/many_ops/multi_block/complex with wasm32 |

Measurement caveat: the `codegen_*` group runs right after the slow `ir_parse`
group and its absolute numbers skew high on this machine (up to ~4× for small
functions). Use the `comparison`/`code_size` groups for low-noise absolute
numbers; use `--save-baseline`/`--baseline` for cross-run comparisons.

### 4b. IR Verification (`verify`)

`forge_ir::verify::Verifier` on representative function shapes (ISA-independent).

| Benchmark | Description |
| --------- | ----------- |
| `verify/{simple_add,many_ops,complex,loop,mem,spill_pressure}` | Verify each shape |

### 4c. Module Level (`module`)

Cross-function compilation and IPA passes with a real function table (the old
IPA benchmarks ran inline/tail-call with an empty HashMap).

| Benchmark | Description |
| --------- | ----------- |
| `module_compile_cross_call` | `JitCompiler::compile_module` on a 2-fn module with a cross-function `call @0` (exercises relocation resolution) |
| `opt_inline_real_table` | `InlinePass` with callee+caller in the function table |
| `opt_tail_call_real_table` | `TailCallPass` with callee+caller in the function table |

ISA capability notes: no `codegen_call` benchmark — the x86_64 lowering rules
have no `[lower.Call]` (`Unsupported: lower Call`); `ir_build_call` still
covers call-heavy IR construction. aarch64/riscv64 lack `Fmul` lowering, so
the float `complex` case is only benchmarked for x86_64 and wasm32.

### 5. End-to-End (`end_to_end`)

Full compilation from IR through code emission.

| Benchmark | Description |
| --------- | ----------- |
| `e2e_compile_simple` | O2 + compile simple add |
| `e2e_compile_complex` | O2 + compile complex function |
| `e2e_compile_loop` | O2 + compile loop function |
| `e2e_compile_float` | O2 + compile F64 chain |
| `e2e_compile_mem` | O2 + compile store/load function |
| `e2e_compile_spill` | O2 + compile spill-heavy function |
| `e2e_compile_big_loop` | O2 + compile big-loop function |
| `e2e_jit_execute` | JIT compile and execute `add(3, 4)` |

### 6. Throughput (`throughput`)

Parameterized benchmarks measuring scalability (N = rounds of arithmetic ops;
`Throughput::Elements` reports IR instruction count). One group per N so the
reported throughput matches each N's instruction count.

| Benchmark | Description |
| --------- | ----------- |
| `throughput/{n}/codegen` | Codegen time for functions with N ops |
| `throughput/{n}/optimize_o1` | O1 pipeline time for functions with N ops |
| `throughput/{n}/optimize_o2` | O2 pipeline time for functions with N ops |
| `throughput/{n}/optimize_o3` | O3 pipeline time for functions with N ops |
| `throughput/{n}/ir_build` | IR construction time for functions with N ops |

### 7. Code Size (`code_size`)

Compiles each test function inside the iteration and reports the emitted byte
count via `Throughput::Bytes` (so the report shows both time and bytes/s).

| Benchmark | Description |
| --------- | ----------- |
| `code_size/{case}/size` | Compiled size of each test function (one group per case) |

### 8. Comparison (`comparison`)

Structured report comparing optimization time, codegen time, and code size
across test cases. Use this to compare code-forge against other backends
(LLVM, Cranelift, etc.).

---

## Reference Results (this machine)

Environment: 12th Gen Intel(R) Core(TM) i9-12900H, 32 GB RAM,
Windows 11 Home China 10.0.26200, Rust 1.96.0-nightly, code-forge v0.2.0.
All times are criterion median wall-clock time. `cargo bench --bench compile_bench -- '^(ir_build|...|end_to_end/e2e_compile_)' --measurement-time 3`

### Compilation Time (median, lower is better)

| Test Case | IR Build | IR Parse | Opt O1 | Opt O2 | Codegen | Total* |
|---------:|---------:|-------:|-------:|--------:|-------:|
| simple_add | 2.0 µs | **9.34 µs** (was 5.97 ms, 640×) | 8.3 µs | 3.4 µs | 6.7 µs | ~24 µs |
| many_ops | 14.5 µs | — | ~44 µs† | ~107 µs | 84.2 µs | ~210 µs |
| complex | 4.2 µs | — | 8.3 µs | 28.5 µs | 24.1 µs | ~67 µs |
| loop | 3.5 µs | — | 8.3 µs | 30.7 µs | 20.1 µs | ~64 µs |

\* Total ≈ IR build + Opt O2 + Codegen.

### IR Parse — logos+lalrpop rewrite (2026-08-03, median)

| Benchmark | before (forge-grammar) | after (logos+lalrpop) | speedup |
| ----------- | ----------------------: | ----------------------: | --------: |
| `ir_parse_simple_add` | 317 µs | **9.34 µs** | 34× |
| `ir_parse_mul_add` | 460 µs | **8.60 µs** | 53× |
| `ir_parse_dot_product` | 1.87 ms | **10.2 µs** | 183× |
| `ir_parse_loop_sum` | 2.93 ms | **12.8 µs** | 229× |
| `ir_parse_complex` | 2.84 ms | **14.2 µs** | 200× |
| `ir_parse_multi_func` | 7.4 ms | **39.2 µs** | 189× |
| `ir_parse_big_text_256` | 24.4 ms | **370 µs** | 66× |

Deterministic LR parsing (logos + lalrpop) is 1–2 orders of magnitude faster
than the previous PEG frontend; 256-instr text now parses in 370 µs.
† O1 pipeline is dominated by `opt_const_fold` (44 µs on the 60-op function after the const-fold rework; was 469 µs).

### Optimization Pass Detail (median µs)

| Pass | simple_add | many_ops | complex | loop |
| ------: | ---------: | ---------: | --------: | ---------: |
| ConstFold only | 1.7* | 44.2 | 4.7* | 8.9* |
| DCE only | — | 17.3 | — | — |
| CopyProp only | — | 14.0 | — | — |
| CSE only | — | 11.6 | — | — |
| GVN only | — | 18.4 | — | — |
| SCCP only | — | 65.4 | — | — |
| JumpThread only | — | 1.6 | — | — |
| Mem2Reg only | — | — | — | 3.6 |
| LICM only | — | — | — | 8.1 |
| BlockParamCoalesce only | — | — | — | 1.4 |
| O0 pipeline | 4.6 | — | — | — |
| O1 pipeline | 8.3 | — | — | — |
| O2 pipeline | 32.6 | — | — | — |
| O3 pipeline | 42.0 | — | — | — |
| O2 pipeline (loop) | — | — | — | 30.5 |

\* Estimated from pipeline timings; single-pass runs were only measured for the function shapes listed in `optimizations`.

### Codegen Detail (median µs)

| Benchmark | Time | Benchmark | Time |
| ---------: | ---------: | ---------: | ---------: |
| codegen_simple_add | ~17 | codegen_with_o1 | ~229 |
| codegen_many_ops | ~178 | codegen_with_o2 | ~278 |
| codegen_complex | ~61 | codegen_with_o2_complex | ~137 |
| codegen_multi_block | ~47 | e2e_compile_simple | ~12 |
| e2e_compile_complex | ~61 | e2e_compile_loop | ~59 |
| e2e_jit_execute | 12.4 | | |

Note: small-function codegen times roughly doubled versus the pre-fix
baseline because the prologue now pushes the full Windows-x64 callee-saved
set (RDI/RSI included); large functions are barely affected (~+4% at 500 ops).
The `code_size/*` benchmarks (identical compile work) show ~+10% on
`simple_add`, so the `codegen_*` group numbers above are noisy on this
machine.

### Throughput Scaling

| Operation Count | IR Instructions | Codegen Time (µs) | O1 Opt Time (µs) |
| ---------: | ---------: | ---------: | ---------: |
| 10 | 34 | 47.6 | 28.4 |
| 50 | 154 | 194.9 | 111.4 |
| 100 | 304 | 378.4 | 217.8 |
| 200 | 604 | 734.5 | 438.0 |
| 500 | 1504 | 1812 | 1071.4 |

Codegen scales ~linearly with IR size. The O1 pipeline is now near-linear
too (10× ops ≈ 9.6× time) after the `opt_const_fold` rework (was ~O(n²):
500 ops took 576 ms; now 1.07 ms).

### New-group Reference Numbers (2026-08-03, median)

`verify` (µs): simple_add 2.9, many_ops 24.0, complex 5.3, loop 5.2, mem 8.4,
spill_pressure 3.0.

`module` (µs): `module_compile_cross_call` 45.0 (2-fn module, cross-function
`call @0`); `opt_inline_real_table` 5.9; `opt_tail_call_real_table` 5.5
(real-table IPA vs 3.6–4.4 µs empty-table baselines).

`pipeline_breakdown` (µs, relative ranking reliable; absolute values skew high
up to ~2.5× because the group runs right after the slow `ir_parse`):
O1 — const_fold 28.1, dead_code 21.4, cse 8.1, jump_thread 4.7, copy_prop 2.5;
O2 — **sccp 179.6 (dominant)**, egraph 26.2, licm 24.4, gvn 14.9, tail_call 5.5,
gvn_pre 3.6, block_param_coalesce 2.8; O3 — ind_var_simplify 25.3,
loop_unroll 13.9, mem2reg 10.8, inline 8.5. Cross-check with the low-noise
`opt_*` single-pass group: sccp 72.5, const_fold 28.9, gvn 16.6, cse 8.7,
egraph 8.8 (µs @ many_ops).

Pipeline (µs, complex input): O0 1.0, O1 6.5, O2 28.0, O3 43.4 (loop input:
O1 7.0 / O2 30.2 / O3 40.9).

e2e (µs): simple 13.0, complex 75.6, loop 67.4 → 43.0 (after trailing-DCE fix),
float 206.7, mem 172.6, spill 64.5, big_loop 310.9.

---

## Known Issues

1. **`ir_parse` was O(n³) — FIXED (2026-08-03).** The pathological scaling
   (2 insts ≈ 13 ms, 8 ≈ 100 ms, 300 ≈ 5.6 h) was actually an **O(n²) lexer**:
   each token re-copied the remaining input and regex-matched longest-first
   prefixes. Zero-copy `&[char]` matching + `match_here` prefix-length made it
   near-linear: `ir_parse_simple_add` 13 ms → **317 µs (41×)**, multi_func
   204 → 7.4 ms, `ir_parse_big_text_256` 24 ms (300 insts used to hang).
   The parser itself was linear all along (~500 µs); see OPTIMIZATION.md §9.
2. **`mini_c` JIT crashes — FIXED (2026-08-03).** Five deterministic codegen
   bugs caused `cargo test -p mini_c` to SEGV (STATUS_ACCESS_VIOLATION):
   `$modrm_mem_rr` RIP-relative encoding for rbp/r13 bases + missing SIB
   syntax, `callee_saved_bytes` omitting the frame-pointer slot, i32
   load/store fixed at 64-bit (adjacent slot overlap), frame size omitting
   the stack_addr local area, and missing `ctx.default_opsize` in lowering.
   All fixed (see OPTIMIZATION.md §9); `cargo test -p mini_c` is fully green.
3. **`Nop` tombstones from optimization passes — mitigated.** GVN/CSE/SCCP/egraph
   rewrite dead instructions to `Opcode::Nop` after the O1 DCE position; codegen
   skips them (fixed). The O2/O3 pipelines now end with a trailing
   `DeadCodeElimPass(UntilFixedPoint)` (2026-08-03), collapsing indirect dead
   code: `e2e_compile_loop` 67.4 → 43.0 µs (-36%). Residual `Nop` *placeholders*
   remain in inst_order (DCE marks, does not physically remove) — physical Nop
   removal is a possible further step.
4. **`Opt O2` on `many_ops` remains SCCP-dominated.** SCCP is the most
   expensive single pass (72.5 µs @ many_ops; const_fold 28.9 µs second); the
   O2 pipeline (GVN/SCCP/egraph on top of O1) carries it. Throughput scaling is
   otherwise near-linear (see Throughput Scaling).
5. **ISA lowering coverage: ZERO gaps.** `crates/tools/forge-tests/src/coverage.rs`
   prints the full 75-opcode × 3-ISA matrix, and `coverage_all_isa_zero_gaps`
   asserts all three backends (x86_64/aarch64/riscv64) lower **0/75**
   ops as unsupported（wasm32 无覆盖矩阵——仅字节码输出）。The ISA-gap pass added: aarch64 single-source int
   (CLZ/RBIT/REV64/ABS/BSWAP), rotate/min-max/CSEL, NEON float ops + FCMP/CSET,
   saturating ops; riscv64 Zbb bit ops (CLZ/CTZ/CPOP/REV8/ROL/ROR/MIN/MAX),
   software sequences (Abs/saturating/Fcopysign), LD-based load, AMOADD/FENCE;
   wasm32 stack-machine opcodes (0x67-0x98 bytecodes) + full [lower.*] mapping.
   Known limitations retained: cross-function `Call` compiles with a
   placeholder displacement in single-function compilation (JIT resolves
   via `compile_module`); aarch64/riscv64 float rounding uses FRINT/fsgnjx
   approximations for Ffloor/Fceil/Ftrunc/Fround; x86 clobbers() generation is
   disabled (arg_regs spill timing — mechanism retained).
6. **Benchmark measurement noise (Windows laptop).** Same work measured in
   different groups varies 2–4× (codegen group after slow `ir_parse` vs
   `comparison` group; e.g. compile many_ops 389.7 vs 92.7 µs; SCCP 72.5 vs
   179.6 µs). Use `comparison`/`code_size` for low-noise absolute numbers and
   `--save-baseline`/`--baseline` for cross-run comparisons. No
   字节→指令解码（`TargetDecoder`）实现存在——`TargetDisassembler` 为 DSL 生成的
   Inst→文本格式化器（非字节解码），故无 disasm 基准组（tracked in OPTIMIZATION.md §8).

## ISA lowering coverage matrix

From `cargo test -p forge-tests --features "isa-x86_64,isa-aarch64,isa-riscv64" -- --nocapture`
(75 representative opcodes; "ok" = compile succeeds):

| Opcode | x86_64 | aarch64 | riscv64 | wasm32 |
| ------ | :----: | :-----: | :-----: | :----: |
| Iadd/Isub/Imul/Udiv/Sdiv/Urem/Srem | ok | Urem/Srem gap | ok | ok |
| Band/Bor/Bxor/Bnot/Ishl/Ushr/Sshr | ok | ok | ok | ok |
| Rotl/Rotr/Smin/Smax/Umin/Umax | gap | gap | gap | gap |
| Clz/Ctz/Popcnt/Bitreverse/Abs/Bswap | gap | gap | gap | gap |
| Fadd/Fsub/Fmul | ok | ok (fixed) | ok (fixed) | ok |
| Fdiv/Fsqrt/Fconst | ok | Fdiv/Fsqrt gap, Fconst ok | ok | ok |
| Fneg/Fabs/Fmin/Fmax/Fma/Fcopysign | partial | gap | gap | partial |
| Icmp/Fcmp | ok | Fcmp gap | Fcmp gap | ok |
| Load/Store/StackAddr/Alloca/GEP | ok | ok | partial | ok |
| Call/CallIndirect | ok (fixed) | ok (fixed) | CallIndirect ok, Call gap | ok |
| Copy/Select/Nop | ok | ok | ok | ok |
| Sextend/Uextend/Ireduce/Bitcast | ok | ok | ok | ok |
| Overflow ops (SaddOverflow etc.) | gap | gap | gap | gap |
| Trap/Freeze/IsNull/AtomicRmw/Fence | gap | gap | gap | gap |

## Fixes included in this benchmark pass

- **JIT correctness (release):** three bugs fixed in x86_64 codegen —
  (a) spill offsets did not account for the callee-saved register area
  (spilled slots could land below `rsp` and hit an unmapped stack page);
  (b) `RDI`/`RSI` were missing from `[abi.callee_saved]` (generated code
  clobbered the caller's RDI/RSI); (c) `@move_args` ran before `@push_callee`
  in the prologue, so argument moves into callee-saved registers overwrote the
  caller's values before they were saved. After the fixes,
  `cargo test -p forge-tests`（`isa/x86_64/jit.rs`，298 个 JIT 测试）全绿，
  `e2e_jit_execute` runs at ~12 µs (it previously hung/corrupted the heap).
- **`Nop` lowering:** the codegen pipeline now skips `Opcode::Nop` instead of
  failing with `Unsupported("lower Nop")`, so O2-optimized IR compiles.
- **`opt_const_fold` rework:** one-shot `value → def` map instead of a
  per-item full-function scan, single-pass worklist (fixed an over-eager
  `processed` mark that forced dozens of fixed-point iterations), removing the
  outer rebuild loop. O1 pipeline went from super-linear (~O(n²), 576 ms at
  500 ops) to near-linear (1.07 ms at 500 ops).
- **aarch64 scalar float:** added FPR register class, FMOV (FPR↔GPR), and
  `[lower.Fadd/Fsub/Fmul/Fconst]` (NEON 4S encodings) — `complex` now compiles
  on aarch64 (was `Unsupported: lower Fmul`).
- **riscv64 F extension:** added `[reg.xmm]` F0–F31, FADD.S/FSUB.S/FMUL.S/
  FDIV.S/FSQRT.S/FSGNJ.S/FMIN.S/FMAX.S/FMV.W.X/FMV.X.W/FLW/FSW, FPR spill and
  `[lower.Fadd/Fsub/Fmul/Fdiv/Fsqrt/Fconst]` — core float now compiles on
  riscv64.
- **Direct call lowering:** x86 `CALL rel32`, aarch64 `BL`, and
  CallIndirect (`CALL r/m64`, `BLR`, `JALR`) now compile (`codegen_call`
  re-enabled, ~7 µs). Cross-function targets emit a placeholder displacement —
  resolution requires module-level linking; JIT execution of call functions
  is not supported. Template positional operands may now drive Freg
  instructions (`Ireg → Freg` type compatibility in the asm resolver).

## LLVM vs. code-forge Comparison Template

Use the table below to record performance comparisons. All tests are run on
the same machine, same input functions, at equivalent optimization levels.

### Compilation Time (microseconds, lower is better)

Run with: `cargo bench -- comparison`

| Test Case | IR Instructions | code-forge Opt | code-forge Codegen | code-forge Total | LLVM Total | Ratio |
| ---------: | ---------: | ---------: | ---------: | ---------: | --------- | --------- |
| simple_add | 3 | `3.4` | `6.7` | `11.9` | `____` | `____` |
| many_ops | 61 | `531.8` | `84.2` | `631` | `____` | `____` |
| complex | 17 | `28.5` | `24.1` | `67` | `____` | `____` |
| loop | 10 | `30.7` | `20.1` | `64` | `____` | `____` |

### Code Size (bytes, lower is better)

| Test Case | IR Instructions | code-forge Size | LLVM -O0 | LLVM -O2 | Notes |
| ---------: | ---------: | ---------: | ---------: | ---------: | --------- |
| simple_add | 3 | 44 | `____` | `____` | |
| many_ops | 61 | 497 | `____` | `____` | |
| complex | 17 | 156 | `____` | `____` | |
| loop | 10 | 122 | `____` | `____` | |

### Notes

- LLVM measurements use `llc -O2` (or `clang -O2 -c`) on equivalent C/Rust input.
- code-forge measurements from `cargo bench` Criterion reports in `target/criterion/`.
- All times are median wall-clock time in microseconds.
- code-forge total = IR build + optimization (O2) + lowering + register allocation + emit.

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

## JIT cross-function call & generic relocation framework

Implemented in the ISA-gap pass:

- **Generic relocation model (LLVM-MC style).** `RelocKind` is now
  `Absolute(u8) | Relative(u8, i8)` — no ISA-specific variants. ISA encoding
  (x86 rel32, AArch64 BL imm26, RISC-V B/J immediate reordering) lives in
  per-backend `RelocPatcher` implementations, registered by ISA name and used
  by `CodeSink::finish` (in-function label fixups) and `JitCompiler`
  (cross-function relocations). `fixup_to_reloc` maps all branch fixups to
  `REL4`.
- **Cross-function JIT calls work.** `[lower.Call]` expands `func` (the IR
  FuncRef number from `Immediate::Func`) into a call instruction whose
  `@call_reloc` / `@call_reloc32` primitive emits the placeholder and records a
  relocation against the `@N` symbol. `compile_module` registers both
  `func.name` and `@N`, `apply_relocations_to` carries the caller name (was
  dropped before, silently skipping patches), and `resolve_pending` patches
  through `ExecutableMemory::modify` (temporary RW) using the backend's
  `RelocPatcher`. Verified by `test_jit_cross_function_call`
  (caller→callee executes and returns 42).
- **Local/native function linking.** `JitCompiler::lookup_symbol` resolves
  through a chain: JIT symbol table → user `set_symbol_resolver` callback →
  host process dynamic symbols (`GetProcAddress` for the exe and kernel32 /
  `dlsym` on Unix). `register_external` still works for explicit registration.
- **Param/return data passing** (complete ABI: registers + stack + float) is
  the next phase; `@move_args` currently handles GPR register params only.

- **Cross-function argument passing (Phase 2 MVP).** `[lower.Call]` now moves
  up to N integer args into ABI registers (`arg0`-`arg7` lowering variables):
  x86_64 `RCX/RDX/R8/R9` (+ Windows x64 ABI), aarch64 `X0-X7`, riscv64
  `X10-X17` (A0-A7); a single integer return value is moved back from
  `RAX`/`X0`/`X10`. Verified by `test_jit_cross_function_call_with_args`
  (caller→callee2(21) returns 42). Recorded limitations: float args (XMM/FP
  arg regs), stack args (>N args), multiple return values, and
  XMM/stack-arg handling in `@move_args` are not yet lowered — completing
  these needs per-type argument classification in the lowering generator.

- **Type-classified call lowering (x86_64).** `[abi.call]` configures the mov
  instructions; the DSL now generates a dedicated `Call` lowering arm that
  classifies each argument by IR type — integer args move into `gpr[k]`
  (`RCX/RDX/R8/R9`), float args into `xmm[m]` (`XMM0-XMM3`), and the return
  value moves back from `RAX`/`XMM0` by type (new `MOVQ_XMM_FREG` /
  `MOVQ_FREG_XMM`). ABI float arg regs configured for x86 (`XMM0-3`), aarch64
  (`V0-V7`) and riscv64 (`F10-F17`). Recorded limitations: callee-side
  float/stack parameter receipt (`@move_args` type classification), stack
  overflow args (>register count), and aarch64/riscv64 `[abi.call]` config.

- **Local-function linkage end-to-end.** `test_jit_call_external_function`
  proves the JIT can actually invoke an external Rust function via
  `call_indirect` (side-effect verification through a static atomic).
  Known limitation: when `call_indirect` has a return value, the regalloc
  result-copy direction is wrong (`mov eax, r15d` instead of `mov r15d, eax`)
  — direct `Call` return values are unaffected.

- **Type-classified parameter receipt (`@move_args`, x86).** `gen_move_args` now
  classifies params by `param_is_float` (filled from `ctx.vreg_classes` during
  regalloc): integer params move from `gpr[k]` (`MovRm8R64`), float params from
  `xmm[m]` (`MOVQ_FREG_XMM` via `[abi.call].ret_mov_f`). Remaining: float
  return path (`@move_ret` float + compiler ret-block float mov), multi-return,
  x64 shadow space.

- **x86 lowering gap closure (26/39 done).** New instructions + lowering:
  `BSWAP_R`, `LZCNT_R`/`TZCNT_R`/`POPCNT_R` (F3 0F BD/BC/B8), `ROL_RM_CL`/
  `ROR_RM_CL` (@shift_cl /0,/1), `MINSD`/`MAXSD`, `MFENCE`, `MOVQ_XMM_FREG`/
  `MOVQ_FREG_XMM`; lowering for Trap, Freeze, Fence, Clz, Ctz, Popcnt, Bswap,
  Fmin, Fmax, Fma, Ftrunc, Rotl, Rotr, Smin, Smax, Umin, Umax, Abs, IsNull,
  IsNotNull, and the six `*Overflow` ops (result half only; flag half and
  saturating ops recorded). Remaining (13): SaddSat/SsubSat/UaddSat/UsubSat,
  Bitreverse, Fcopysign, Ffloor/Fceil/Fround (need ROUNDSD), Alloca/
  GetElementPtr/GlobalAddr (LEA GEP), AtomicRmw (LOCK XCHG).

- **x86 lowering gap closure (30/39).** Additional: `ROUNDSD_I` ($sse_rr_3a
  macro, 66 0F 3A 0B /r ib) with Ffloor/Fceil/Fround (+ Ftrunc via
  cvtsd2si/cvtsi2sd), GetElementPtr via `lea [base+index*4]`. Remaining (9):
  sadd_sat/sub_sat/uadd_sat/usub_sat (need overflow-detect + clamp with
  IMM64 constants), Bitreverse (bit-reversal sequence), Fcopysign (AND/OR
  sign-mask), Alloca (stack-frame slot), GlobalAddr (global relocation),
  AtomicRmw (LOCK-prefixed xadd — no F0 prefix precedent in the DSL yet).

- **x86 lowering gap closure (34/39).** Added cc_names `s/ns/o/no` and the
  saturating family via precolored scratch (R11-R14, MOV_REG_IMM64 constants,
  seto/setb + cmovns/cmovs/cmovne): SaddSat, UaddSat (carry→MAX), SsubSat,
  UsubSat (borrow→0). Remaining (5): Bitreverse (needs immediate-shift or
  ~20-instruction bit-twiddle sequence), Fcopysign (XMM sign-mask constant),
  Alloca (stack-frame slot), GlobalAddr (global relocation), AtomicRmw (LOCK
  prefix — no F0-prefix macro precedent in the DSL).

- **x86 lowering gap closure (39/39 — COMPLETE).** Final batch: `AtomicRmw`
  (LOCK XADD via new `$lock_modrm_mem_rr` + `$modrm_mem_rr` macros, old value
  loaded pre-op), `Bitreverse` (Hacker's Delight 3-step + bswap), `Fcopysign`
  (GPR bit round-trip via movq + AND/OR sign mask), `GlobalAddr` (placeholder
  zero — module-level global relocation still required), `Alloca`
  (compile-time placeholder `lea [rs1+rs2*1+0]` — real stack-slot allocation
  needs compiler-level frame support). Coverage matrix: **x86_64 0/75 ops not
  lowering** (was 39). Semantic caveats (compile-verified, execution may need
  the above infrastructure): Alloca/GlobalAddr placeholders, precolored
  scratch R11-R15 conflicts in long lowering sequences, AtomicRmw Add-only.

- **aarch64 lowering gap closure (11 done, 35 remaining).** Reusing SD_*
  instructions: Trap (SD_UD2), Freeze (SD_MOV), Urem/Srem (udiv/sdiv + mul +
  sub via reserved X9 scratch), the six `*Overflow` result halves, Fma
  (SD_FMUL + SD_FADD). Coverage: aarch64 46→35. Remaining need new AArch64
  instructions (CLZ/RBIT/REV/FDIV/FMIN/FMAX/FNEG/FABS/FSQRT/FCMP/FRINT —
  encoding must be verified) or sequences (rotate, saturate, IsNull cset).

- **KNOWN ISSUE — DSL generation is not deterministic across clean builds.**
  Generated code iterates model HashMap (RandomState seed is per-compilation),
  so a clean rebuild can flip template binding order (e.g. MOV_RM8_R64
  dest/src) and produce machine code that crashes JIT execution
  (STATUS_ACCESS_VIOLATION in test_add). Recompiling usually lands on a good
  seed (forge-tests jit.rs 298 pass). Fix: make the model deterministic
  (BTreeMap/ordered iteration in forge-dsl) — tracked as a separate task.

- **GlobalAddr now real (x86).** `[lower.GlobalAddr]` emits `movabs rd, <g>`
  with a new `@abs_reloc` encoding primitive (8-byte absolute placeholder +
  `RelocKind::Absolute(8)` against the `"G{id}"` symbol); `JitCompiler`
  allocates a per-global data segment (`data_segments`) and registers both the
  global name and `"G{id}"`. `test_jit_global_addr` verifies the JIT function
  reads the global's address and its init value (42). Alloca still needs the
  two-phase stack-slot design (recorded).

- **x86 overflow flags + AtomicRmw op dispatch.** Overflow ops now lower the
  flag half via `setcc r2` (seto/setb) — `test_jit_overflow_flag` verifies
  MAX+1 sets the flag. `AtomicRmw` is dispatched by op (Icmp-style
  `AtomicRmw.<Op>` sub-rules + `ctx.current_atomic_op` from
  `Immediate::Uint`): Add (XADD), Xchg (XCHG), Sub/And/Or/Xor (LOCK-prefixed)
  all compile; Nand/Max/Min/Umax/Umin need a cmpxchg loop (recorded).
  Recorded issue: executing the memory-operand atomic hits a regalloc
  cross-instruction VReg split (iconst/global_addr result used later lands in
  a different physical register) — compile-verified, execution pending.

- **riscv64 lowering gap closure (13 done, 32 remaining).** Trap (ECALL), the
  six `*Overflow` result halves, Fma (FMUL_S+FADD_S), Fmin/Fmax, Fneg/Fabs
  (FSGNJN_S/FSGNJX_S — new, funct3 0x01/0x02), Fcmp condition sub-rules
  (FEQ_S/FLT_S/FLE_S — new, funct7 0x20; Equal/LessThan/LE/GreaterThan/GE).
  Coverage: riscv64 45→32. Recorded: riscv64 `i64` immediate fields (LD_R/
  SD_R offset, SLTIU/SLLI imm) hit a DSL old-path field-binding mismatch
  (immediate receives a VReg) — Load/Store/Freeze/NotEqual deferred; needs a
  forge-dsl binding fix.

- **riscv64 immediate-field binding bug (systemic).** `i64` immediate fields
  (ADDI imm, SLTIU imm, SLLI/SRAI shamt) bind the *previous* operand when the
  template uses explicit VRegs — `SRAI VReg(31), VReg(30), 63` fails with
  E0308 (shamt receives VReg). Existing `Iconst` (`ADDI rd, iconst, VReg(0)`)
  compiles because iconst/VReg are positional workarounds. Root cause is in
  the forge-dsl old-path field binding (cst_codegen.rs) — blocks riscv64
  immediate-heavy lowering (Smin/Smax sequence, Freeze, IsNull, Fcmp.NotEqual,
  Load/Store offsets). Tracked for a forge-dsl fix before the remaining
  riscv64 items.

- **FIXED — forge-dsl old-path field binding.** cst_codegen.rs now binds
  positional operands by the asm template's placeholder order (TOML
  definition order) instead of `inst_def.fields` (which serde alphabetizes) —
  this corrupted immediate fields (riscv64 ADDI/SLTIU/SRAI imm/shamt,
  aarch64 SD_SETCC cond). Adapted the workaround templates (riscv64
  ADDI/XORI/SLTIU order, aarch64 SD_SETCC `rd, cond`, riscv64 Bnot,
  JALR/BNE/Return term templates, SD_R/LD_R restored). Verification:
  `test_generate_is_deterministic` + forge-codegen 107 tests + jit 298 +
  workspace 51 all pass. riscv64 back to 34 not-lowering (was 32; Fconst
  and CallIndirect deferred — lui+addi / JALR VReg variant recorded).

## 测试三级标注与跨架构执行验证

**测试级别标注**（每项测试按其验证深度归类）：

| 级别 | 含义 | 载体 |
| --- | --- | --- |
| `compile-only` | 只验证 lowering 编译通过（不执行） | forge-tests `coverage!`/`coverage_all_isa_zero_gaps`（覆盖矩阵，3 ISA × 75 ops：x86_64/aarch64/riscv64） |
| `encode-golden` | 验证指令编码字节精确匹配 | forge-tests `encode_golden!`（x86_64 31）+ `aarch64`（32）+ `riscv64`（37）= 100 断言 |
| `exec-required` | 实际执行 JIT 编译产物并断言返回值 | forge-tests `isa/x86_64/jit.rs`（298 测试，宿主 x86_64 Windows）+ `text_to_exec.rs`（1068 行） |

**跨架构执行验证**（问题 1：宿主为 Intel，非宿主 ISA 无本机执行验证）：

- **aarch64 / riscv64**：经 WSL 安装 `qemu-user`（`wsl apt install qemu-user`）后，将 JIT 编译产物导出为二进制，用 `qemu-aarch64`/`qemu-riscv64` user-mode 执行 + 返回值断言。WSL 发行版未安装时（需 `wsl --install`），以 `encode-golden` 编码断言为执行验证的替代。
- **wasm32**：wasm 字节码产物可用 wasmtime 执行（安装后）。
- **已知限制**：`call_indirect` 有返回值时的结果 copy 方向 bug；x86 浮点返回值链路（@move_ret 多返回）；x64 shadow space 栈参数。

## forge-tests 通用测试框架（unicorn 执行引擎）

**框架结构**：`crates/tools/forge-tests` 提供统一 ISA 测试框架，按 Cargo features 开关：

| feature | 作用 |
| --- | --- |
| `isa-x86_64` / `isa-aarch64` / `isa-riscv64` | ISA 开关 |
| `test-int` / `test-float` / `test-io` / `test-control` | 指令类型分组（默认全开） |
| `exec-unicorn` | vendored unicorn（cmake 编译静态库）+ 手写 FFI 跨架构模拟执行（aarch64/riscv64） |
| `nightly` | forge-rustc 后端测试 |

**测试类型**（三级标注）：

- `coverage!`（compile-only 覆盖矩阵）— 用户自定义 ISA 经 backend_tests! 家族宏接入
- `encode_golden!`（编码字节断言）
- `exec!`（执行测试：本机 x86_64 NativeExecutor + unicorn 跨架构 UnicornExecutor）

**跨架构执行**：vendored unicorn（`crates/tools/forge-tests/vendor/unicorn`，build.rs 用
cmake 编译静态库）execute-from-buffer 方式（mem_map 代码页+栈页 → mem_write 裸机器码 →
emu_start → reg_read 返回值寄存器 RAX/X0/X10），替代 WSL/QEMU 路径。构建前提：只需
cmake + C 编译器（VS Build Tools / gcc / clang）——**无需 libclang / pkg-config /
系统 unicorn / 预编译 DLL**。wasm32 不在本框架范围（无执行测试，仅 forge-codegen 内嵌
compile-only 覆盖）。

---

## 性能优化记录（2026-08，compile_bench 驱动）

基于 `benches/compile_bench.rs` 基准热点，对 codegen/regalloc 链、优化 pass 层、
IR 构建与解析公共热点做了一轮优化。方法：先跑全量基准存 `--save-baseline main`，
每阶段改完后用 `--baseline main` 对比（criterion change，p<0.05 视为显著）。

### 优化内容

| 模块 | 改动 |
| --- | --- |
| regalloc | `reg_owner` HashMap 全表扫描 → 按物理寄存器编号索引数组（O(1) 冲突检测）；`active` Vec retain+push → HashMap（O(1) 插入/移除）；process_block 消除 4 处每指令克隆（slot/clobbers/inst_uses/二次 clone） |
| liverange | 阶段 0（循环检测）与阶段 2（CFG 构建）重复扫描合并为一遍；跨块区间扩展 O(V×B) → 按块 Σ live_out |
| emission | `emit_code` 改 `&mut self`，用块引用替代整 VCode clone；emit 闭包去 self 捕获（emit_inst 系列改静态方法）；每指令 slot 借用化 |
| compiler | `func.clone()` 条件化——仅 pattern isel 启用的 ISA 才克隆整个 Function（x86_64 零克隆） |
| pass 层 | sccp worklist 加 in-queue 去重 + 借用替代克隆；copy_prop 单轮收敛（链式 Copy 已解析）；改 CFG 的 pass（dead_code/jump_thread/const_fold）调用 `analysis_mut().invalidate()` 失效分析缓存 |
| IR 构建 | builder `emit` 每指令 `env::var_os("FORGE_TRACE_IR")` → OnceLock 缓存；`ops.clone()` 双克隆 → 单次拷贝；dfg Instruction 构造 `results.clone()` → move |
| IR 解析 | semantics `value_map`/`block_map` String key → 借用 `&str`（消除每指令 String 克隆）；grammar 5 处右递归列表（ParamList/TypeOps/GepIdxList/CallArgList/StructList）O(n²) reducer → 左递归累积 |

### 基准结果（criterion change vs main baseline，中位数）

| 基准组 | 变化范围 | 代表项 |
| --- | --- | --- |
| ir_build | **-8.8% ~ -46.8%** | many_ops 20.4→11.5µs（-44.7%）、mem -46.8%、float -43.4%、big_loop -44.7%、spill_pressure -39.7% |
| ir_parse | **-11.4% ~ -24.1%** | big_text_256 372→322µs（-14.1%）、loop_sum -24.1%、mul_add -21.3%、multi_func -18.9% |
| optimizations | sccp **-62.6%**（67.5→23.9µs）、copy_prop -19.5%、dead_code_elim -15.8%、cse -10.1%、const_fold_float -6.1% | pipeline O2 -7.4%、O3 -3.5%、loop_func -5.6% |
| codegen | **-3.9% ~ -19.0%** | mem -19.0%、multi_block -15.5%、big_loop -11.5%、float -10.3%、simple_add -9.5%、many_ops -6.7%、spill_pressure -6.3%、with_o1 -6.6%、with_o2 -5.8% |
| throughput | codegen -3.9~-5.2%；ir_build -42~-52%（500 ops 457→219µs） | optimize_o1 -5.1%、o2 -2.6% |
| e2e | float **-34.7%**（274→179µs，func.clone 移除）、mem -14.9%、complex -6.7% | simple -4.4%、big_loop -3.5% |

无显著回退（codegen_float 首次对比 +6% 经重跑确认是顺序噪声，实际 -10.3%）。

### 验证

- `cargo test --workspace --exclude forge-rustc --all-features`：全部通过（forge-ir 239、forge-opt 91、forge-codegen 116+12+15+8+12、forge-tests 140 含 298 条 JIT 执行用例迁移集等）。
- `cargo check -p forge-ir -p forge-opt -p forge-codegen`：无警告。

---

## 性能优化记录（2026-08 第二轮：codegen 链残余 + 框架级）

基于 `docs/codegen_stage_profile.md` 的 stage 占比实测（**regalloc 仍占 codegen
63-67%**、lowering 14-20%、emit ~11%）与已识别未实施的优化点，完成第二轮优化。
对比基准同为 `--save-baseline main`（第一轮优化前）；下表为**两轮累计**变化
（criterion change vs main baseline，中位数）。

### 优化内容(2026-08-06)

| 模块 | 改动 |
| --- | --- |
| regalloc | `compute_live_intervals` intervals HashMap 按指令数预分配（免 rehash）；`LiveInterval::next_use_after` 线性 find → `partition_point` 二分（spill 决策 O(log n)） |
| emission | **spill 指令的 `AllocResult` 深克隆完全消除**：以 `SmallVec<[(XReg,PReg);2]>` 稀疏 overrides + 闭包查询替代整表克隆（覆盖优先、base 回退）。经核实 encode 从指令字段读物理寄存器（`set_reg_field` 已写入），不依赖 reg_map 覆盖——比计划中的 AllocResultView 方案更优：零接口改动、改动面 6+ 文件收敛为 1 个 |
| lowering | `lower_block` 每块 `cloned().collect()` 深克隆整块指令 → 直接借用迭代（`block_inst_iter` 返回 `&Instruction`） |
| pass 层 | `ExprKey.operands` Vec → `SmallVec<[Value;4]>`（cse/gvn/gvn_pre 三处构造，每指令省一次堆分配）；cse/gvn 块 `inst_order.clone()` → 借用（dfg 字段级拆分借用） |
| IR 构建 | `UseLists` HashMap 预分配 `with_capacity(8)`（微函数不受损，中大型函数少 rehash） |
| JIT 并行化 | `compile_module` 自适应并行：函数数 ≥8 时 `std::thread::scope` 并行编译（每函数独立 `compile_raw`），串行按 func_ref 序注册符号；`Function` 加 `unsafe impl Sync`（文档约束：编译只读路径安全，惰性分析并发由调用方约束）；新增 `test_jit_parallel_module_compile`（8 函数 + 跨函数 call） |
| 修复 | **既有 bug**：`apply_relocations_to` 的 `pending_relocs` 覆盖式赋值改追加——8+ 函数模块中中间函数的跨函数 call relocation 会丢失 patch（2 函数时侥幸正常），执行返回垃圾值 |

### 基准结果（vs main baseline，两轮累计）

| 基准组 | 变化范围 | 代表项 |
| --- | --- | --- |
| codegen | **-0.8% ~ -29.2%** | big_loop -29.2%（369→299µs）、mem -23.0%、multi_block -14.0%、many_ops -11.4%、with_o2 -10.0%、spill_pressure -9.8%、with_o1 -9.7%、complex -8.0%；aarch64/riscv64/wasm32 各函数 -2.8~-15.1% |
| ir_build | **-10.6% ~ -50.3%** | mem -50.3%、float -47.8%、many_ops -43.9%、big_loop -43.7%、spill_pressure -42.1% |
| ir_parse | **-5.3% ~ -21.3%** | big_text_256 -21.3%（372→292µs）、loop_sum -20.8%、mul_add -17.9%、multi_func -17.4% |
| optimizations | sccp **-63.1%**、cse -20.1%、gvn_pre -5.3%、gvn -4.7% | pipeline O2 -13.7%、O1 -9.4%、O3 -9.1% |
| module | module_compile_cross_call -2.4%（串行路径，无回退） | |

无显著回退（`ir_build_simple_add` 曾 +39.5%，系 UseLists 过度预分配所致，改为
`with_capacity(8)` 后恢复 -10.6%；`codegen_float` +1.5% 属运行噪声范围）。

### 验证(2026-08-06)

- `cargo test --workspace --exclude forge-rustc --all-features`：全部通过（forge-ir 201、
  forge-opt 91、forge-codegen 117 含新并行测试、forge-tests 140 含 JIT 跨函数回归）。
- 新增测试覆盖：`test_jit_parallel_module_compile`（8 函数并行编译 + 跨函数 call 执行）。
- `unsafe impl Sync for Function` 附文档约束（编译只读路径安全；惰性分析并发由调用方保证）。
