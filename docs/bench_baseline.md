# Bench Baseline

Generated: 2026-08-06T14:15:00
Command:
`cargo bench --bench compile_bench`
All times are criterion median, in microseconds (lower is better).
Notes: `end_to_end/e2e_jit_execute` previously hung *intermittently* in
release builds — non-deterministic register allocation (HashMap iteration
order) fixed 2026-08-06 with deterministic tie-breakers; now included in the
default run. Shared machine: absolute values vary with system load;
optimization conclusions use same-configuration before/after comparison.

## ir_build

| Benchmark | time (µs) |
| --------- | --------: |
| ir_build_simple_add | 2.33 |
| ir_build_call | 2.37 |
| ir_build_multi_block | 3.66 |
| ir_build_complex | 4.37 |
| ir_build_spill_pressure | 5.97 |
| ir_build_float | 8.82 |
| ir_build_many_ops | 12.36 |
| ir_build_big_loop | 13.73 |
| ir_build_mem | 16.03 |

## ir_parse

| Benchmark | time (µs) |
| --------- | --------: |
| ir_parse_simple_add | 5.67 |
| ir_parse_mul_add | 7.68 |
| ir_parse_dot_product | 9.59 |
| ir_parse_loop_sum | 11.15 |
| ir_parse_complex | 12.57 |
| ir_parse_multi_func | 35.90 |
| ir_parse_big_text_256 | 311.63 |

## pipeline_breakdown

| Benchmark | time (µs) |
| --------- | --------: |
| pipeline_breakdown/o2_block_param_coalesce | 1.50 |
| pipeline_breakdown/o1_jump_thread | 1.67 |
| pipeline_breakdown/o2_gvn_pre | 2.97 |
| pipeline_breakdown/o2_tail_call | 3.16 |
| pipeline_breakdown/o1_copy_prop | 3.34 |
| pipeline_breakdown/o3_inline | 3.92 |
| pipeline_breakdown/o3_loop_unroll | 5.00 |
| pipeline_breakdown/o3_mem2reg | 5.05 |
| pipeline_breakdown/o3_ind_var_simplify | 5.10 |
| pipeline_breakdown/o1_dead_code | 6.14 |
| pipeline_breakdown/o2_licm | 7.41 |
| pipeline_breakdown/o2_egraph | 8.33 |
| pipeline_breakdown/o1_cse | 8.78 |
| pipeline_breakdown/o2_gvn | 13.27 |
| pipeline_breakdown/o2_sccp | 24.47 |
| pipeline_breakdown/o1_const_fold | 26.88 |

## codegen

| Benchmark | time (µs) |
| --------- | --------: |
| codegen_riscv64/simple_add | 10.44 |
| codegen_wasm32/simple_add | 10.65 |
| codegen_aarch64/simple_add | 11.12 |
| codegen_simple_add | 11.28 |
| codegen_call | 13.72 |
| codegen_aarch64/multi_block | 26.36 |
| codegen_aarch64/complex | 26.60 |
| codegen_multi_block | 31.34 |
| codegen_wasm32/multi_block | 32.04 |
| codegen_riscv64/multi_block | 32.63 |
| codegen_riscv64/complex | 32.69 |
| codegen_wasm32/complex | 33.20 |
| codegen_complex | 35.98 |
| codegen_spill_pressure | 47.69 |
| codegen_with_o2_complex | 64.40 |
| codegen_float | 79.77 |
| codegen_aarch64/many_ops | 81.74 |
| codegen_with_o2 | 85.39 |
| codegen_wasm32/many_ops | 93.72 |
| codegen_riscv64/many_ops | 99.58 |
| codegen_many_ops | 100.71 |
| codegen_mem | 108.75 |
| codegen_with_o1 | 119.26 |
| codegen_big_loop | 227.39 |

## verify

| Benchmark | time (µs) |
| --------- | --------: |
| verify_simple_add | 0.96 |
| verify_spill_pressure | 3.16 |
| verify_loop | 5.22 |
| verify_complex | 5.34 |
| verify_many_ops | 7.86 |
| verify_mem | 8.45 |

## module

| Benchmark | time (µs) |
| --------- | --------: |
| module_compile_cross_call | 43.14 |

## throughput

| Benchmark | time (µs) |
| --------- | --------: |
| throughput_10/ir_build | 7.48 |
| throughput_10/optimize_o1 | 24.25 |
| throughput_50/ir_build | 25.29 |
| throughput_10/optimize_o2 | 38.21 |
| throughput_10/optimize_o3 | 42.55 |
| throughput_100/ir_build | 48.49 |
| throughput_10/codegen | 59.67 |
| throughput_200/ir_build | 89.78 |
| throughput_50/optimize_o1 | 94.28 |
| throughput_50/optimize_o2 | 146.67 |
| throughput_50/optimize_o3 | 151.45 |
| throughput_100/optimize_o1 | 187.31 |
| throughput_500/ir_build | 219.52 |
| throughput_50/codegen | 237.56 |
| throughput_100/optimize_o2 | 281.79 |
| throughput_100/optimize_o3 | 289.52 |
| throughput_200/optimize_o1 | 365.10 |
| throughput_100/codegen | 458.31 |
| throughput_200/optimize_o2 | 540.62 |
| throughput_200/optimize_o3 | 565.82 |
| throughput_500/optimize_o1 | 885.78 |
| throughput_200/codegen | 908.43 |
| throughput_500/optimize_o2 | 1353.49 |
| throughput_500/optimize_o3 | 1372.67 |
| throughput_500/codegen | 2363.65 |

## code_size

| Benchmark | time (µs) |
| --------- | --------: |
| code_size_simple_add/size | 11.97 |
| code_size_multi_block/size | 32.32 |
| code_size_complex/size | 35.86 |
| code_size_many_ops/size | 105.04 |

## comparison

| Benchmark | time (µs) |
| --------- | --------: |
| comparison/opt_o2_simple_add | 3.78 |
| comparison/codegen_simple_add | 11.64 |
| comparison/size_simple_add | 11.92 |
| comparison/opt_o2_complex | 25.10 |
| comparison/opt_o2_loop | 27.00 |
| comparison/codegen_loop | 32.05 |
| comparison/size_loop | 32.56 |
| comparison/size_complex | 35.66 |
| comparison/codegen_complex | 35.98 |
| comparison/opt_o2_many_ops | 66.45 |
| comparison/codegen_many_ops | 100.71 |
| comparison/size_many_ops | 103.53 |

## end_to_end

| Benchmark | time (µs) |
| --------- | --------: |
| e2e_compile_simple | 15.05 |
| e2e_compile_loop | 48.29 |
| e2e_compile_complex | 55.87 |
| e2e_compile_spill | 57.91 |
| e2e_compile_float | 110.77 |
| e2e_compile_mem | 241.64 |
| e2e_compile_big_loop | 316.27 |
| e2e_jit_execute | 17.72 |

---

## 2026-08-06 第二轮优化与问题修复说明

基于 `benches/compile_bench.rs` 基准驱动，全量测试
`cargo test --workspace --exclude forge-rustc` 1287 passed / 0 failed 兜底。

### 问题代码修复（本轮核心）

#### 1. 间歇性 JIT 死循环（非确定性寄存器分配）——重大修复

**现象**：`end_to_end/e2e_jit_execute` 与 mini_c 集成测试（如
`test_struct_in_loop`）在 release/debug 下**间歇性挂起**（同二进制跨进程
运行，~66% 失败率；单次运行全绿纯属运气）。

**根因**（非确定性，跨进程变化）：

- `regalloc_bt.rs evict_and_assign`：`active.iter().max_by_key(...)` 在多个
  victim 的 `next_use_after` 平局时，victim 选择取决于 HashMap 迭代顺序
  （RandomState 每进程随机）→ 偶发不同的 spill/驱逐决策 → 偶发错误分配 →
  生成机器码死循环。
- `compiler.rs` TargetMachine 类配置继承：`classes.keys().max_by_key(...)`
  平局时继承的 allocatable 随机。

**修复**：两处 `max_by_key` → `max_by` + 确定性 tie-breaker
（evict：`next_use_after` 平局按 vreg index 递增；类配置：按（变体序, 宽度））。
修复后 mini_c integration_tests 90 项跨进程多次运行全过，e2e_jit_execute
纳入默认基准稳定运行。

#### 2. regalloc 寄存器池泄漏深挖

审计全部入池/出池路径（初始化 / spill_vreg / expire_dead / assign_reg::Fixed /
precolored / evict_and_assign）：set_reg_owner 6 处与 clear 对称、入池均先清
owner——**无实际泄漏**。`pop_free` 丢弃冲突 preg 为防御性清理（占用者释放时
自然回池，非永久丢失），注释固化分析结论。

#### 3. JIT 基准注释更新

`benches/compile_bench.rs` 头部注释更新：删除过时 "hangs in release" 说明，
标注间歇性非确定性根因与修复，`e2e_jit_execute` 已纳入 `end_to_end` 组。

### 实施的优化（本轮）

| 项 | 内容 | 收益 |
| --- | --- | --- |
| analysis.rs compute_idom | 一次构建前驱映射（替代每轮每 block 线性扫全函数），O(轮数×n²)→O(轮数×n) | licm -7.8%、loop_unroll -10.6%（vs 上轮） |
| liverange 数据流 | 增量式收敛（live_in 以 uses 为初值 + insert 返回值检测，消除每轮重建与 O(集合) 比较） | codegen -3%~-13%（vs 上轮） |
| const_fold | const_operands → SmallVec（worklist 增量版确认） | 微 |
| sccp | Big 深拷贝消除评估：fold_opcode 泛型化侵入 20+ match 分支，收益有限，不实施（记录） | — |
| ir_build / ir_parse | 评估：dfg 已 SmallVec+push 轻量、lexer/lalrpop 生成代码风险高，不迁移（记录） | — |

### 与上轮对比（同配置缩短版，µs）

| Benchmark | 上轮 | 本轮 | 变化 |
| --------- | ----: | ----: | ---: |
| codegen_float | 92.0 | 79.8 | -13% |
| codegen_many_ops | 104.4 | 100.7 | -4% |
| codegen_mem | 112.4 | 108.8 | -3% |
| codegen_big_loop | 238.8 | 227.4 | -5% |
| throughput_500/codegen | 2433 | 2364 | -3% |
| pipeline_breakdown/o2_licm | 8.04 | 7.41 | -8% |
| pipeline_breakdown/o3_loop_unroll | 5.59 | 5.00 | -11% |
| throughput_500/optimize_o2 | 1378 | 1353 | -2% |
| comparison/opt_o2_loop | 24.8 | 27.0 | +9%（噪声级波动） |
| e2e_jit_execute | 17.5 | 17.7 | 持平（稳定无挂起） |

### 两轮累计（vs 2026-08-05 基线，同配置口径）

| Benchmark | 8-05 基线 | 当前 | 累计变化 |
| --------- | --------: | ----: | ------: |
| codegen_many_ops | 184.1 | 100.7 | **-45%** |
| codegen_float | 161.6 | 79.8 | **-51%** |
| codegen_mem | 221.9 | 108.8 | **-51%** |
| codegen_big_loop | 351.1 | 227.4 | **-35%** |
| throughput_500/codegen | 4372 | 2364 | **-46%** |
| throughput_500/optimize_o2 | 1528 | 1353 | -11% |

### 排查记录

- `ir_parse_big_text_4`（39 ms）为历史 criterion 残留目录（基准代码仅定义
  `_256`），忽略。
- 共享机器负载会导致基准整体放大 3-4 倍（曾误判 comparison/opt_o2_loop
  "+250% 回归"，干净环境重跑为 24-27µs 正常）；基准结论基于同配置多次对比。
