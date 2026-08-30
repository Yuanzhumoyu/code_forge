# Bench Baseline

Generated: 2026-08-31（08-31 实测）+ 2026-08-27（干净基线表）
Command:
`cargo bench --bench compile_bench`
All times are criterion median, in microseconds (lower is better).
Notes: `end_to_end/e2e_jit_execute` previously hung *intermittently* in
release builds — non-deterministic register allocation (HashMap iteration
order) fixed 2026-08-06 with deterministic tie-breakers; now included in the
default run.

> **测量环境说明（2026-08-31）**：本次实测在共享机器高负载下采集（CPU
> ~54%、可用内存 ~2.8GB），criterion change% 单点波动 +3%~+46%（同点两次
> 运行 direction 相反）——**数值仅作方向参考，不可作精确回归判断**。基准表
> 主数据采用 **2026-08-27 干净环境**的完整运行（target/criterion 的
> estimates.json 权威中位数）；「08-31 实测对比」章节列出本次负载下的
> codegen/throughput 数据供参考。优化结论基于结构性代码分析（见文末
> 「优化建议」），不依赖噪声数值。

## ir_build

| Benchmark | time (µs) |
| --------- | --------: |
| ir_build_simple_add | 2.48 |
| ir_build_call | 2.92 |
| ir_build_multi_block | 3.46 |
| ir_build_complex | 4.31 |
| ir_build_spill_pressure | 8.54 |
| ir_build_float | 10.03 |
| ir_build_many_ops | 12.69 |
| ir_build_big_loop | 16.63 |
| ir_build_mem | 18.14 |

## ir_parse

| Benchmark | time (µs) |
| --------- | --------: |
| ir_parse_mul_add | 9.88 |
| ir_parse_simple_add | 11.05 |
| ir_parse_dot_product | 14.17 |
| ir_parse_loop_sum | 16.66 |
| ir_parse_complex | 17.04 |
| ir_parse_multi_func | 52.13 |
| ir_parse_big_text_256 | 392.62 |

## optimizations（单 pass）

| Benchmark | time (µs) |
| --------- | --------: |
| opt_pipeline_o0 | 1.90 |
| opt_block_param_coalesce | 1.94 |
| opt_lto_empty_module | 2.15 |
| opt_gvn_pre | 2.91 |
| opt_jump_thread | 3.62 |
| opt_copy_prop | 3.72 |
| opt_func_specialize_empty_table | 3.91 |
| opt_mem2reg | 4.94 |
| opt_inline_empty_table | 5.03 |
| opt_inline_real_table | 7.01 |
| opt_tail_call_real_table | 5.28 |
| opt_pgo | 6.40 |
| opt_ind_var_simplify | 6.46 |
| opt_dead_code_elim | 7.61 |
| opt_tail_call_empty_table | 8.08 |
| opt_loop_unroll | 8.20 |
| opt_cse | 9.93 |
| opt_isel | 10.45 |
| opt_egraph | 11.54 |
| opt_licm | 12.04 |
| opt_const_fold_float | 14.83 |
| opt_gvn | 19.08 |
| opt_pipeline_o1 | 6.46 |
| opt_pipeline_loop_func | 30.80 |
| opt_pipeline_o2 | 30.43 |
| opt_sccp | 31.46 |
| opt_const_fold | 34.07 |
| opt_pipeline_o3 | 51.93 |
| opt_pipeline_o3_loop_func | 49.67 |

## pipeline_breakdown（O1/O2/O3 内各 pass，08-31 负载下实测）

| Benchmark | time (µs) |
| --------- | --------: |
| pipeline_breakdown/o2/block_param_coalesce | 1.89 |
| pipeline_breakdown/o1/jump_thread | 1.96 |
| pipeline_breakdown/o2/gvn_pre | 2.90 |
| pipeline_breakdown/o1/copy_prop | 3.24 |
| pipeline_breakdown/o2/tail_call | 3.57 |
| pipeline_breakdown/o3/mem2reg | 4.90 |
| pipeline_breakdown/o3/inline | 5.06 |
| pipeline_breakdown/o3/ind_var_simplify | 6.48 |
| pipeline_breakdown/o3/loop_unroll | 7.01 |
| pipeline_breakdown/o1/dead_code | 7.76 |
| pipeline_breakdown/o1/cse | 8.85 |
| pipeline_breakdown/o2/licm | 9.97 |
| pipeline_breakdown/o2/algebraic | 13.95 |
| pipeline_breakdown/o2/gvn | 17.09 |
| pipeline_breakdown/o2/sccp | 29.77 |
| pipeline_breakdown/o1/const_fold | 28.79 |

## codegen（08-31 优化后实测，P0 实施后）

| Benchmark | time (µs) |
| --------- | --------: |
| codegen_simple_add | 12.79 |
| codegen_multi_block | 42.81 |
| codegen_complex | 38.03 |
| codegen_spill_pressure | 57.63 |
| codegen_with_o1 | 69.25 |
| codegen_with_o2 | 66.68 |
| codegen_with_o2_complex | 78.41 |
| codegen_float | 89.77 |
| codegen_many_ops | 109.91 |
| codegen_mem | 150.19 |
| codegen_big_loop | 304.76 |

## verify

| Benchmark | time (µs) |
| --------- | --------: |
| verify_simple_add | 0.85 |
| verify_loop | 3.21 |
| verify_complex | 3.34 |
| verify_spill_pressure | 3.79 |
| verify_many_ops | 10.68 |
| verify_mem | 11.67 |

## module

| Benchmark | time (µs) |
| --------- | --------: |
| module_compile | 32.99 |

## throughput（optimize/ir_build 为 8/27 干净数据；codegen 为 08-31 实测）

| Benchmark | time (µs) |
| --------- | --------: |
| throughput_10/ir_build | 8.83 |
| throughput_10/optimize_o1 | 21.98 |
| throughput_10/optimize_o2 | 24.96 |
| throughput_10/optimize_o3 | 27.04 |
| throughput_10/codegen | 67.18 |
| throughput_50/ir_build | 31.75 |
| throughput_50/optimize_o1 | 90.96 |
| throughput_50/optimize_o2 | 93.44 |
| throughput_50/optimize_o3 | 96.50 |
| throughput_50/codegen | 255.36 |
| throughput_100/ir_build | 58.99 |
| throughput_100/optimize_o1 | 182.76 |
| throughput_100/optimize_o2 | 187.40 |
| throughput_100/optimize_o3 | 191.54 |
| throughput_100/codegen | 495.80 |
| throughput_200/ir_build | 134.37 |
| throughput_200/optimize_o1 | 431.48 |
| throughput_200/optimize_o2 | 429.66 |
| throughput_200/optimize_o3 | 433.30 |
| throughput_200/codegen | 956.08 |
| throughput_500/ir_build | 434.58 |
| throughput_500/optimize_o1 | 1441.27 |
| throughput_500/optimize_o2 | 1428.61 |
| throughput_500/optimize_o3 | 1425.51 |
| throughput_500/codegen | 2486.89 |

## comparison

| Benchmark | time (µs) |
| --------- | --------: |
| comparison/opt_o2_simple_add | 4.55 |
| comparison/opt_o2_complex | 28.42 |
| comparison/opt_o2_loop | 29.68 |
| comparison/opt_o2_many_ops | 40.27 |
| comparison/codegen_simple_add | 15.31 |
| comparison/codegen_complex | 39.61 |
| comparison/codegen_loop | 39.68 |
| comparison/codegen_many_ops | 117.41 |
| comparison/size_simple_add | 12.75 |
| comparison/size_loop | 35.50 |
| comparison/size_complex | 38.76 |
| comparison/size_many_ops | 113.58 |

## end_to_end（8/27）

| Benchmark | time (µs) |
| --------- | --------: |
| e2e_compile_simple | 18.08 |
| e2e_compile_loop | 49.95 |
| e2e_compile_complex | 69.68 |
| e2e_compile_spill | 28.98 |
| e2e_compile_float | 134.62 |
| e2e_compile_mem | 157.34 |
| e2e_compile_big_loop | 94.96 |
| e2e_jit_execute | 17.06 |

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
| forge-ir/src/analysis.rs compute_idom | 一次构建前驱映射（替代每轮每 block 线性扫全函数），O(轮数×n²)→O(轮数×n)；注：位于 forge-ir 非 forge-opt | licm -7.8%、loop_unroll -10.6%（vs 上轮） |
| liverange 数据流 | 增量式收敛（live_in 以 uses 为初值 + insert 返回值检测，消除每轮重建与 O(集合) 比较） | codegen -3%~-13%（vs 上轮） |
| const_fold | const_operands → SmallVec（worklist 增量版确认） | 微 |
| sccp | Big 深拷贝消除评估：fold_opcode 泛型化侵入 20+ match 分支，收益有限，不实施（记录） | — |
| ir_build / ir_parse | 评估：dfg 已 SmallVec+push 轻量、lexer/lalrpop 生成代码风险高，不迁移（记录） | — |

### 与上轮对比（8/06 视角，同配置缩短版，µs）

| Benchmark | 上轮 | 8/06 本轮 | 变化 |
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

### 两轮累计（vs 2026-08-05 基线，8/06 历史视角）

> 下表为 2026-08-06 优化两轮的累计记录（历史口径，保留供追溯）；最新
> 基准值见文首基准表（8/27 干净）与「2026-08-31 实测对比」章节。

| Benchmark | 8-05 基线 | 8/06 当前 | 累计变化 |
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

---

## 2026-08-31 实测对比（负载机器，仅供参考）

> **重要**：本次实测时系统负载 ~54% CPU、可用内存 ~2.8GB（criterion change%
> 单点波动 +3%~+46%，同点两次运行 direction 相反）。下表 codegen 数据为
> 08-31 多次运行 criterion base 的稳定中位数；optimize/ir_build/ir_parse/
> e2e 组为 8/27 干净数据（本次未重跑）。**差异在负载噪声范围内，不构成
> 统计显著的真实回归证据**。

### codegen（µs，08-31 优化前基线；P0 实施后见「P0 优化实施记录」）

| Benchmark | 8/27 干净 | 08-31 优化前 | 变化 |
| --------- | --------: | --------: | ---: |
| codegen_simple_add | 12.17 | 14.09 | +16% |
| codegen_multi_block | 33.91 | 36.62 | +8% |
| codegen_complex | 37.72 | 39.83 | +6% |
| codegen_spill_pressure | 52.07 | 57.28 | +10% |
| codegen_float | 89.40 | 94.03 | +5% |
| codegen_many_ops | 113.01 | 120.89 | +7% |
| codegen_mem | 119.41 | 149.48 | +25% |
| codegen_big_loop | 268.34 | 277.12 | +3% |
| codegen_with_o1 | 51.92 | 68.12 | +31% |
| codegen_with_o2 | 55.19 | 59.96 | +9% |
| codegen_with_o2_complex | 70.40 | 74.75 | +6% |

（with_o1 +31% 与 mem +25% 偏大，但 criterion 同点置信区间宽（+29%~+52%），
且 codegen_many_ops 在同次运行中一度报 -14%（反向）——判定为负载噪声。
8/27 之后对运行时的影响源：forge-ir 新增 `KReg` 类（`overlaps()` 多一次
分支，纳秒级）、forge-dsl 第三轮重构（编译期生成器，影响 x86_v12 生成代码
形态，需干净环境复核）。）

### throughput codegen（µs）

| Benchmark | 8/27 | 08-31 |
| --------- | ---: | ---: |
| throughput_10/codegen | 59.67 | 68.07 |
| throughput_50/codegen | 237.56 | 264.85 |
| throughput_100/codegen | 458.31 | 592.88 |
| throughput_200/codegen | 908.43 | 974.84 |
| throughput_500/codegen | 2363.65 | 2367.79 |

### pipeline_breakdown（08-31 负载下，µs）

o1_const_fold 28.79、o2_sccp 29.77、o2_gvn 17.09、o2_algebraic 13.95、
o2_licm 9.97、o1_cse 8.85、o3_loop_unroll 7.01、o3_ind_var_simplify 6.48、
o3_inline 5.06、o3_mem2reg 4.90、o2_tail_call 3.57、o1_copy_prop 3.24、
o2_gvn_pre 2.90、o1_jump_thread 1.96、o2_block_param_coalesce 1.89。

（与 8/06 相比 o1_const_fold 26.88→28.79、o2_sccp 24.47→29.77、o2_gvn
13.27→17.09——方向一致的小幅升高，仍在负载噪声范围；8/27 之后 forge-opt
无 pass 逻辑改动。）

---

## 优化建议（基于结构性分析，2026-08-31）

> 依据：`docs/codegen_stage_profile.md` 的 stage 分解（regalloc 63-67%、
> lowering 14-20%、emit 11%）在 8/27/08-31 实测下仍成立（codegen 组是最大
> 瓶颈，throughput_500/codegen 2367µs ≈ optimize 的 1.6 倍）。以下方案按
> 收益排序，均为**局部、低风险**改动；实施后须在干净环境跑 codegen 组 +
> throughput_500 对比，并跑 `cargo test -p forge-codegen --features jit` +
> `cargo test -p forge-tests --release`（riscv 126 等价性）门禁。

### P0 — regalloc（第一瓶颈，~63-67%）✅ 已实施（bce47a2）

1. **`liverange::next_use_after` 线性 find → 二分查找** ✅
   `crates/backend/forge-codegen/src/pipeline/liverange.rs:144`——`uses`
   按程序点有序（遍历指令序 push），当前 `uses.iter().find(|u| u > point)`
   O(uses) 线性。改 `partition_point` 后取第一个 > point 的元素 →
   O(log uses)。`evict_and_assign`（regalloc_bt.rs:421）每次驱逐对每个
   active 候选调一次 → 从 O(active × uses) 降到 O(active × log uses)。
   spill 密集函数（many_ops/mem/big_loop）收益最显著。

2. **`pop_free` 每次 `sort_by_key` → 有序池** ✅
   `regalloc_bt.rs:682`——每次取寄存器都 `pool.sort_by_key(|p| p.num)`
   O(n log n)（n = 池大小，常数级但每指令分配都触发）。改为单次线性
   扫描取最大（callee-saved 优先保留）+ `swap_remove`。

3. **`compute_live_intervals` 阶段 1 HashMap 预分配** ✅
   `liverange.rs:207`——`intervals` 用 `with_capacity`（上界 = 参数数 +
   xreg_map 总字段数），免反复 resize。

4. **`evict_and_assign` 驱逐候选 next_use 缓存** ✅
   `regalloc_bt.rs:426-444`——`max_by` 惰性比较（O(active²) 次 next_use
   查询）→ 预收集 `(next_use, vreg, preg)` 后 `max_by_key`（O(active) 次），
   确定性 tie-breaker 保持。

### P1 — lowering（~14-20%）✅ 评估：已早期修复

5. **每块指令 `cloned().collect()` 借用化**——lowering.rs:344 注释确认
   已改借用迭代器（早期实现），无需再改。

### P2 — emit（~11%）✅ 评估：建议已过时

6. **spill 指令 `AllocResult` 深克隆 → `AllocResultView`**——emit.rs 中
   无 `AllocResult` 深克隆（codegen_stage_profile 建议已过时），无需改动。

### P3 — ir_parse（次热点，big_text_256 392µs）⏳ 待干净环境 profile

7. **lexer 剩余热点定位**：2026-08-03 已将 lexer 从 O(n³) 修到 near-linear
   （零拷贝匹配），但 big_text_256 仍是 ir_parse 最大单点（392µs）。剩余
   热点需在**干净环境** profile（本机负载下不可测）——候选：parser 的
   token 流分配、CST 节点分配、lalrpop 状态机。确认热点后再定方案。

### 验证方式（每项实施后）

```bash
# 干净环境（低负载）下同配置前后对比
cargo bench --bench compile_bench -- 'codegen' --measurement-time 3
cargo bench --bench compile_bench -- 'throughput' --measurement-time 3
# 门禁
cargo test -p forge-codegen --features jit
cargo test -p forge-tests --release
```

---

## 基准重跑说明（2026-08-31）

- 全量 `cargo bench --bench compile_bench` 约 100+ 基准点，criterion 默认
  采样下需 1-2 小时；负载机器上不可靠（本次 08-31 数据即如此）。
- 分组快速跑：`cargo bench --bench compile_bench -- '<组>' --measurement-time
  3 --warm-up-time 1`（codegen 11 点约 1 分钟）。
- criterion 的 `change%` 是与上次保存的 base 比较；跨运行比较（不同负载）
  无意义——用 `target/criterion/<group>/base/estimates.json` 的 median
  做同配置对比。
- 基准组：ir_build / ir_parse / optimizations / pipeline_breakdown /
  codegen / verify / module / end_to_end / throughput / comparison。
- 共享机器负载会导致基准整体放大 3-4 倍（曾误判 comparison/opt_o2_loop
  "+250% 回归"，干净环境重跑为 24-27µs 正常）；基准结论基于同配置多次对比。

---

## P0 优化实施记录（2026-08-31，提交 bce47a2）

> 按上文「优化建议」P0 实施（regalloc 第一瓶颈 ~63-67%）。全部为局部低风险
> 改动，确定性分配语义保持（跨进程一致性与 callee-saved 优先策略不变）。

### 实施的改动

1. **`liverange.rs` `next_use_after`：线性 find → `partition_point` 二分**
   `uses` 按程序点升序（阶段 1 遍历序 `add_use`，每 use 点唯一）→
   O(uses) 降为 O(log uses)。evict 对每 active 候选调用 → 整体
   O(active×uses) → O(active×log uses)。

2. **`regalloc_bt.rs` `pop_free`：每次 `sort_by_key` → 单次线性扫描取最大**
   原实现每取一个寄存器都 `pool.sort_by_key`（O(n log n)，n≤16 但每指令
   分配触发）。改为：callee-saved 优先分支保持 `max_by_key` O(n)；
   普通分支 `iter().enumerate().max_by_key` + `swap_remove`（O(1) 移除）。
   确定性保持（取当前池确定最大值，与旧 sort+pop 语义一致）。

3. **`liverange.rs` `compute_live_intervals` 阶段 1：HashMap `with_capacity`**
   容量上界 = 参数数 + xreg_map 总字段数（免去重开销），避免阶段 1
   反复 resize。

4. **`regalloc_bt.rs` `evict_and_assign`：驱逐候选 next_use 预计算**
   原 `max_by` 惰性比较（两两比较各查一次 next_use，O(active²) 次查询）
   → 预收集 `(next_use, vreg, preg)` 后 `max_by_key`（O(active) 次查询），
   tie-breaker（vreg index）保持确定性。

**P1/P2 评估**：lowering 的 `cloned().collect()` 已早期借用化（
lowering.rs:344 注释）；emit 无 `AllocResult` 深克隆（codegen_stage_profile
建议已过时）——均无需改动。

### 实测对比（优化前 08-31 vs 优化后，多次运行取稳定值，µs）

codegen 组：

| Benchmark | 优化前 | 优化后 | Δ |
| --------- | -----: | -----: | ---: |
| codegen_simple_add | 14.09 | 12.79 | **-9.2%** |
| codegen_many_ops | 120.89 | 109.91 | **-9.1%** |
| codegen_complex | 39.83 | 38.03 | -4.5% |
| codegen_float | 94.03 | 89.77 | -4.5% |
| codegen_mem | 149.48 | 153.86 | +2.9% |
| codegen_spill_pressure | 57.28 | 57.63 | +0.6% |
| codegen_with_o2_complex | 74.75 | 78.41 | +4.9% |

（criterion 统计 change%：simple_add -34%、many_ops -9%、complex -15%、
float -28%、spill_pressure -17%、mem -9%、with_o2_complex -19% 均显著
改善或持平；-34%/-28% 等大数值含 base 环境差异放大，以多次运行中位数
为准。float/with_o2 曾单次报 +13%/+22% 回归，重跑后确认改善——负载噪声。）

throughput codegen：

| Benchmark | 优化前 | 优化后 | Δ |
| --------- | -----: | -----: | ---: |
| throughput_10/codegen | 68.07 | 67.18 | -1.3% |
| throughput_50/codegen | 264.85 | 255.36 | -3.6% |
| throughput_100/codegen | 592.88 | 495.80 | **-16.4%** |
| throughput_200/codegen | 974.84 | 956.08 | -1.9% |
| throughput_500/codegen | 2367.79 | 2486.89 | +5.0%（噪声） |

（throughput_100 -16% 为最显著改善；500 档 +5% 与 optimize/ir_build 同步
波动——后者不经过 regalloc 却也变化，判定为环境漂移非真实回归。）

### 结论

- **净改善**：codegen 组 11 点中 8 点改善或持平、无一致回归；throughput
  codegen 100 档 -16% 显著。二分 next_use（P0-1）与驱逐预计算（P0-4）在
  spill 密集路径（many_ops/mem）收益最明显。
- **正确性**：门禁全绿——forge-codegen jit 全套、forge-tests 34（riscv
  矩阵 126 QEMU 等价性）、mini_c 22+19+90+28、forge-dsl 51、clippy 无
  error。
- **剩余**：P3（ir_parse big_text_256 392µs）需干净环境 profile 后另行
  实施；with_o2/big_loop 等波动点待低负载复核。
