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
| KD-03 | LICM 默认关闭：`-O2` 的 `core::slice::sort` 在开启时错值（`[1,9,14,14]`） | `crates/middle/forge-opt/src/lib.rs`、`loops/licm.rs` | 运行时正确性无守卫；gate 只跑无 LICM 管线 |
| KD-05 | NEON 向量 / aarch64 DWARF-CFI 未覆盖 | `docs/plans/forge-rustc-v27-plan.md`、`docs/reference/backend-gaps.md` | 属能力缺口，fail-closed 或未接线 |

## KD-03：LICM 默认关闭

- **症状**：`-O2` 的 `core::slice::sort` 在 LICM 开启时错值（实测
  `[1,9,14,14]`；`FORGE_OPT_SKIP=licm` 恢复正常）。最窄复现是「只外提 Iconst」
  （`FORGE_LICM_ONLY=iconst`）。
- **2026-10-29 复核（M6-129 之后，枚举 ABI 已修）**：LICM 开启下 sort 守卫不再是
  “错值”而是**挂起**（>20 分钟不退出）；按指令类二分（`FORGE_LICM_ONLY`）：
  `stack_addr` 单独通过、`load` 单独通过、**`iconst` 单独挂起**。把常量从
  “外提集合”改为“只用于证明不变”（仍在 `inv_values`，不再进 `inv_insts`）后，
  全量 LICM 仍挂 ⇒ 不是常量本身，而是**链式外提的放置/支配性**（外提目标取
  `preheader.unwrap_or(header)`，未校验目标是否支配全部使用；嵌套循环/多出口
  时可能不成立；克隆顺序也只按块序、不看跨块定义顺序）。
- **2026-10-29 尝试（已回退，未提交代码）**：按上述方向实现四件事——
  ① 外提前过滤“目标块必须支配每个结果的全部使用块”；② 跨块链式外提按定义序
  拓扑排序；③ 克隆落点改为“块首 + 按拓扑序排列”（修掉 `InstOrderViolation`）；
  ④ 只允许有**真 preheader** 的循环外提（无 preheader 不外提）。结果：
  IR 校验全部通过（不再有 InstOrderViolation），但 `-O2` kitchen 探针**仍然挂起**；
  类二分仍是 `stack_addr`/`load` 单独通过、`iconst` 单独挂起，且“排除常量外提”
  后组合仍挂 ⇒ 剩余根因在**发射侧/语义**（外提到的值跨块使用与栈槽/帧处理），
  不是纯 LICM 放置问题。代码已回退，KD-03 继续 open，不建议在没有 IR/汇编
  差分前默认开启。
- **下一步**：在 `FORGE_ENABLE_LICM=1` 下对失败函数做“外提前后 IR + 汇编”差分，
  重点看外提 `StackAddr/Ptrtoint/Inttoptr/Iadd` 链与发射侧栈槽/帧基址；修好后
  再把 sort 守卫加 `FORGE_ENABLE_LICM=1` 变体并默认开启。
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
