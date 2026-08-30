# Codegen Stage Profile（2026-08 优化后）

用 `CF_CODEGEN_TIMING=1 cargo bench --bench compile_bench -- 'codegen_*' --measurement-time 1`
采集的 stage 分解（`compiler.rs` 的 `[codegen {name}] total/lower/regalloc/emit` 输出，稳态中位数）。

> **时效性（2026-08-31）**：本 profile 为 2026-08-06 采集。8/27 与 08-31
> 的 codegen 组实测（`docs/bench_baseline.md`）显示整体 ±16% 波动（负载
> 噪声），regalloc 仍是第一瓶颈的结论成立（多数函数 stage 占比 63-67%
> 未变；codegen 组是最大耗时组，throughput_500/codegen 2367µs ≈ optimize
> 的 1.6 倍）。具体优化建议见 `docs/bench_baseline.md` 文末「优化建议」
> （P0-1 next_use_after 二分、P0-2 pop_free 有序池、P0-3 intervals 预分配、
> P0-4 驱逐候选缓存、P1 lowering 借用化、P2 emit 视图化）。

## 各函数 stage 占比

| 函数 | total | lower | regalloc | emit | blocks+vreg+frame |
| --- | ---: | ---: | ---: | ---: | ---: |
| many_ops（60 inst） | ~180µs | ~35µs（20%） | ~113µs（**63%**） | ~20µs（11%） | <0.5% |
| complex（17 inst，4 块） | ~63µs | ~8.8µs（14%） | ~41µs（**65%**） | ~7.3µs（12%） | ~2% |
| sum_to_n（multi_block，10 inst） | ~51µs | ~7.3µs（14%） | ~34µs（**67%**） | ~5.5µs（11%） | ~4% |

（`codegen_big_loop`/`codegen_mem` 的精确 stage 未单独采集，二者 regalloc 压力更大——有
spill 路径；以多数函数 63-67% 的比例推断 regalloc 仍是第一瓶颈。）

## 结论（Phase 2 优先级）

1. **regalloc 仍是 codegen 的第一瓶颈（~63-67%）**——优先：
   - `liverange::compute_live_intervals`：`intervals` HashMap 预分配 + 阶段 1 每指令 `entry().or_insert_with` 去重
   - `evict_and_assign`：`next_use_after` 线性 `find` → 二分（`uses` 已按程序点有序）
   - `pop_free`/`remove_from_free`：池操作小优化
2. **lowering ~14-20%**：每块指令 `cloned().collect()` 借用化、`InstPacket` clobbers `repeat_n` 共享
3. **emit ~11%**：spill 指令 `AllocResult` 深克隆 → `AllocResultView`（overrides 映射）

## 测量方法备注

`CF_CODEGEN_TIMING` 输出来自 `pipeline/compiler.rs` 的 `compile_with_alloc`（release
构建下同样打印）。warmup 阶段数字偏高（首次编译/冷缓存），取稳态中位数。
