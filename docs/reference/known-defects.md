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
| KD-05 | NEON 向量（V64/V128/V256 语义）未覆盖 | `docs/plans/forge-rustc-v27-plan.md`、`docs/reference/backend-gaps.md` | 属能力缺口，IR 层 fail-closed |

## KD-05：NEON 向量未覆盖（aarch64 CFI 已接线）

- 向量（V64/V128/V256 的 NEON 语义）仍未接线；IR 层遇到未支持形态会 fail-closed
  （`[cap:VECTOR]` 等），不是静默错值。
- **aarch64 DWARF CFI 已接线（2026-10-30，M6-133）**：`scan_arm64_prologue` 扫描
  `sub sp`/存 x30·x29/`add x29` 序言并产出 CFA 行（CFA=x29、RA@CFA-8、x29@CFA-16、
  x19..x28 依次下移）；`.debug_frame` 由 `dwarf.rs` 写入对象。守卫 =
  `rustc_aarch64_debug_frame_cfi_present`（对象里 `.debug_frame` 非空且含
  `DW_CFA_def_cfa_register(29)`）+ `cfi.rs` 的 arm64 扫描器单测。
- 权威清单见 [`backend-gaps.md`](backend-gaps.md)；进度与验收门槛见
  `docs/plans/forge-rustc-v27-plan.md` 的「仍未覆盖」段。

## 维护约定

1. 新增 `#[ignore]` 必须同时：写清 `#[ignore = "…（见 KD-xx）"]`、在本页建条目。
2. 关闭条目时删除本页对应段，并在 `CHANGELOG.md` 记一条（用户可见行为变化）。
3. 本页只放**已知且未关闭**的缺陷；修好后移入 CHANGELOG 历史，不留"已修复"段落。
