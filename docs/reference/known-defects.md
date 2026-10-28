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
| KD-04 | aarch64 `&slice[from..to]`（Subslice）长度未重算：`from_str` 死循环 | ✅ **死循环已修**（M6-127，`Rvalue::Ref` Subslice 分支） | 探针 O0/O1/O2 不再挂；但**解析值仍错**（见 KD-08） |
| KD-05 | NEON 向量 / aarch64 DWARF-CFI 未覆盖 | `docs/plans/forge-rustc-v27-plan.md`、`docs/reference/backend-gaps.md` | 属能力缺口，fail-closed 或未接线 |
| KD-06 | 大函数里先跑迭代器代码，再调 `str::parse`：结果错/挂起（独立小函数正确） | `crates/tools/forge-rustc/src/lower/`（串槽/ABI 方向待查） | 无守卫；已知可复现组合 |
| KD-08 | **`i32::from_str` 返回值高位丢失**：`parse::<i32>("12345")` 高位为 0（只留低 8 位） | ① **O2 聚合常量**已修（M6-128：纯数据 ≥8B 聚合常量按字节物化；含指针聚合仍走重定位路径）；② **O0 仍错**：core `from_str` 载体路径待修 | ① 守卫 `rustc_aarch64_sort_probe`（`Result<i32,i32>` 常量 == 断言，O0/O1/O2）；② 独立最小复现：`((parse(...) >> 8) & 0xFF) == 0` |

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

## KD-04：aarch64 kitchen 组合探针挂起

- **症状**：单函数里同时用 `i32::from_str`、`sort_unstable`、`binary_search`、
  `write!` 的探针在 QEMU 下挂起（-O0/-O1/-O2 都挂）；但分片探针（`parse`、
  `even`、`map/filter`、`sort`、`binary_search`）各自正确。
- **证据**：`docs/plans/forge-rustc-v27-plan.md` 的 M6-126/127 进度段；分片探针脚本
  位于 `target/tmp`（不入库，可重建）。
- **2026-10-29 更新（M6-127 后）**：**挂起根因已修**——`from_ascii_bytes_radix_impl`
  的 `s = &s[1..]` 走 `Rvalue::Ref` 的 WA-25 分支，整值复制 fat pointer、忽略
  `Subslice`，len 不缩短 ⇒ 死循环；新增 Subslice 分支后 O0/O1/O2 不再挂。
- **新暴露（KD-08）**：解析**值**仍错——独立探针 `((parse::<i32>("12345") >> 8) & 0xFF)`
  在 O0/O2 都是 0（正确应为 `0x30`）。IR 观测：`from_ascii_bytes_radix_impl` 的
  `Result<i32, ParseIntError>` 返回被降成 `define ptr`（8 字节载体用 PTR 类型），
  Ok 路径把 tag@0 / value@4 写进返回槽后 `load ptr` 返回内容；调用方按 Result 读回时
  值与标签的 4 字节位置/载体语义不一致，导致高位丢失。另有 Err 路径
  `store ptr 1` 把 1 字节 `IntErrorKind` 当 8 字节写（越界 4 字节）。
- **钉住方式**：无守卫（组合/载体形态不稳定，先定位再固化）。
- **复现**：以 `crates/tools/forge-rustc/tests/arm64_exec.rs` 的 sort/iterator
  探针为模板，把四段合并进 `forge_entry`，按 O0/O1/O2 编译并在 QEMU 跑。
- **关闭条件**：定位并修复后，把 kitchen 探针提升为 `arm64_exec` 守卫。

## KD-05：NEON / aarch64 DWARF-CFI 未覆盖

- 向量（V64/V128/V256 的 NEON 语义）与 aarch64 的 `.eh_frame`/CFI 仍未接线；
  IR 层遇到未支持形态会 fail-closed（`[cap:VECTOR]` 等），不是静默错值。
- 权威清单见 [`backend-gaps.md`](backend-gaps.md)；进度与验收门槛见
  `docs/plans/forge-rustc-v27-plan.md` 的「仍未覆盖」段。

## 维护约定

1. 新增 `#[ignore]` 必须同时：写清 `#[ignore = "…（见 KD-xx）"]`、在本页建条目。
2. 关闭条目时删除本页对应段，并在 `CHANGELOG.md` 记一条（用户可见行为变化）。
3. 本页只放**已知且未关闭**的缺陷；修好后移入 CHANGELOG 历史，不留"已修复"段落。
