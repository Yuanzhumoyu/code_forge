# Codegen Stage Profile（2026-08 优化后）

用 `CF_CODEGEN_TIMING=1 cargo bench --bench compile_bench -- 'codegen_*' --measurement-time 1`
采集的 stage 分解（`compiler.rs` 的 `[codegen {name}] total/lower/regalloc/emit` 输出，稳态中位数）。

> **时效性（2026-08-31 第二轮实测）**：本表为 2026-08-06 采集；08-31
> 用 `CF_CODEGEN_TIMING` 重测 many_ops：**regalloc 56-60%、lowering 33%→
> 26%（SmallVec 后）、emit 8%**——regalloc 仍是第一瓶颈，lowering 曾是
> 意外高占比（第二轮新热点，已处理）。
>
> **P0 已实施（bce47a2）**：`next_use_after` 二分、`pop_free` 有序池、
> `compute_live_intervals` 预分配、`evict_and_assign` 驱逐候选预计算——
> codegen 组 8/11 点改善或持平。
>
> **第二轮已实施（944f0eb）**：`LowerCtx.current_immediates` Vec →
> SmallVec<[u64;4]>（lowering 主循环每指令重建，免堆分配）——many_ops
> lower 62.9µs→25.8µs、total 188µs→100µs；codegen multi_block/complex/
> with_o1 改善 12-20%、throughput 大函数 -5~-13%。剩余建议见
> `docs/performance/bench_baseline.md` 文末「优化建议」。
>
> **第三轮评估（08-31）**：ir_parse big_text_256 -9.4% 显著改善（既有
> lexer 零拷贝修复效果，无需再动）；verify 各 pass 用惰性缓存
> （dominator_tree/predecessors）结构健康；regalloc 剩余为数据流规模
> 本质成本——三轮后无低风险高收益点，进入测量/维护期。

## 各函数 stage 占比（2026-08-06 基线；08-31 重测见时效性说明）

| 函数 | total | lower | regalloc | emit | blocks+vreg+frame |
| --- | ---: | ---: | ---: | ---: | ---: |
| many_ops（60 inst） | ~180µs | ~35µs（20%） | ~113µs（**63%**） | ~20µs（11%） | <0.5% |
| complex（17 inst，4 块） | ~63µs | ~8.8µs（14%） | ~41µs（**65%**） | ~7.3µs（12%） | ~2% |
| sum_to_n（multi_block，10 inst） | ~51µs | ~7.3µs（14%） | ~34µs（**67%**） | ~5.5µs（11%） | ~4% |

（`codegen_big_loop`/`codegen_mem` 的精确 stage 未单独采集，二者 regalloc 压力更大——有
spill 路径；以多数函数 63-67% 的比例推断 regalloc 仍是第一瓶颈。）

## 结论（阶段优先级，2026-08-31 更新）

1. **regalloc 仍是 codegen 的第一瓶颈（~56-67%）**——已优化项：
   `next_use_after` 二分、`pop_free` 有序池、`intervals` 预分配、
   `evict_and_assign` 驱逐预计算（bce47a2）。剩余为数据流规模本质成本。
2. **lowering（20%→26%）**——已优化：`current_immediates` SmallVec
   （944f0eb）；`cloned().collect()` 已早期借用化。
3. **emit（~8-11%）**：无 `AllocResult` 深克隆（建议已过时），无需改动。

## 测量方法备注

`CF_CODEGEN_TIMING` 输出来自 `pipeline/compiler.rs` 的 `compile_with_alloc`（release
构建下同样打印）。warmup 阶段数字偏高（首次编译/冷缓存），取稳态中位数。
