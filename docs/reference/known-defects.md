# 已知缺陷索引（Known Defects）

> 状态：active（2026-10-28 建）· 维护：新增/关闭 `#[ignore]`、默认关闭的门禁豁免、
> 无守卫的探针缺口时，**同一次提交里**更新本页。

本页收口那些**不会被 `cargo forge gate` 报红**的已知失败项：`#[ignore]` 测试、
默认关闭的优化 pass、以及只在临时探针里复现的缺口。目的是让"全绿"不再把永久
隐藏项一起盖住：每一条都写清 **症状 / 证据 / 钉住方式 / 复现命令 / 关闭条件**。

另见：能力缺口（fail-closed 的 `[cap:ID]`，不是静默缺陷）在
[`backend-gaps.md`](backend-gaps.md)；forge-rustc 的临时绕法在
`crates/tools/forge-rustc/WORKAROUNDS.md`。

## 汇总

| 编号 | 症状 | 位置 | 门禁可见性 |
| --- | --- | --- | --- |
| KD-01 | JIT 栈形参 + 循环 + 调用：`n >= 1` 时产物 AV 崩溃（分配器缺陷，M6-㉑/㉔） | `crates/backend/forge-codegen/src/runtime/jit.rs`（`test_jit_spill_pressure_loop_with_call`） | `#[ignore]`，lib 测试不计红 |
| KD-02 | JIT「栈形参上界 + 循环 + 调用」纯组合即挂起（死循环，M6-㉝） | 同上（`jit_stack_param_loop_call_only_step`） | `#[ignore]`，lib 测试不计红 |
| KD-03 | LICM 默认关闭：`-O2` 的 `core::slice::sort` 在开启时错值（`[1,9,14,14]`） | `crates/middle/forge-opt/src/lib.rs`、`loops/licm.rs` | 运行时正确性无守卫；gate 只跑无 LICM 管线 |
| KD-05 | NEON 向量 / aarch64 DWARF-CFI 未覆盖 | `docs/plans/forge-rustc-v27-plan.md`、`docs/reference/backend-gaps.md` | 属能力缺口，fail-closed 或未接线 |

## KD-01 / KD-02：JIT 分配器 AV 与死循环

- **KD-01（M6-㉑/㉔）**：`test_jit_spill_pressure_loop_with_call` 在 `n = 0` 通过、
  `n >= 1` 时 `STATUS_ACCESS_VIOLATION`。机器级证据（`FORGE_TRACE_SPILL=1`）：
  第 9 个形参 `n` 分配在 `RDI`，循环头用 `CmpRmR { src: R10, src2: RDI }` 读它，
  而循环体把 `RDI` 当累加器写 —— 寄存器分配/溢出槽冲突。
- **KD-02（M6-㉝）**：`jit_stack_param_loop_call_only_step` 实测挂起（死循环）
  ——纯「栈形参上界 + 循环 + 调用」就坏，与形参参与累加无关。
- **钉住方式**：两条 `#[ignore = "已知失败：…"]`（`crates/backend/forge-codegen/src/runtime/jit.rs`）。
  `cargo test --workspace` / CI 的 Test (Windows) 会显示 `ignored`，**不计红**。
- **复现**：

```bash
cargo test -p forge-codegen --lib --all-features \
  test_jit_spill_pressure_loop_with_call -- --ignored --nocapture
cargo test -p forge-codegen --lib --all-features \
  jit_stack_param_loop_call_only_step -- --ignored --nocapture
```

- **关闭条件**：分配器修好后去掉 `#[ignore]`（注释要求二者当回归用）；`cargo test
  -p forge-codegen --lib --all-features` 全绿且 CI Test (Windows) 覆盖。

## KD-03：LICM 默认关闭

- **症状**：`-O2` 的 `core::slice::sort` 在 LICM 开启时错值（实测
  `[1,9,14,14]`；`FORGE_OPT_SKIP=licm` 恢复正常）。最窄复现是「只外提 Iconst」
  （`FORGE_LICM_ONLY=iconst`）。
- **现状**：`lib.rs` 的 O2 管线只在 `FORGE_ENABLE_LICM=1` 时注册 LICM；默认关闭。
- **钉住方式**：无运行时守卫（默认路径不启用该 pass）；aarch64 sort 守卫覆盖的是
  无 LICM 管线（O0/O1/O2 真执行 15）。
- **复现**：

```bash
FORGE_ENABLE_LICM=1 cargo test -p forge-rustc --test arm64_exec \
  rustc_aarch64_sort_probe_runs_under_qemu -- --nocapture
```

- **关闭条件**：补全外提条件（支配性 / 插入位置 / 链依赖）后默认开启，并给 sort
  守卫加一条「LICM 开启」变体；详见方案 M6-123 段与 `loops/licm.rs` 头注释。

## KD-05：NEON / aarch64 DWARF-CFI 未覆盖

- 向量（V64/V128/V256 的 NEON 语义）与 aarch64 的 `.eh_frame`/CFI 仍未接线；
  IR 层遇到未支持形态会 fail-closed（`[cap:VECTOR]` 等），不是静默错值。
- 权威清单见 [`backend-gaps.md`](backend-gaps.md)；进度与验收门槛见
  `docs/plans/forge-rustc-v27-plan.md` 的「仍未覆盖」段。

## 维护约定

1. 新增 `#[ignore]` 必须同时：写清 `#[ignore = "…（见 KD-xx）"]`、在本页建条目。
2. 关闭条目时删除本页对应段，并在 `CHANGELOG.md` 记一条（用户可见行为变化）。
3. 本页只放**已知且未关闭**的缺陷；修好后移入 CHANGELOG 历史，不留"已修复"段落。
