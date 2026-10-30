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
| KD-11 | 浮点形参（FPR）在函数内有调用时被 spill 后**收参丢失**（读成 0） | `forge-isa-dsl` 的 `gen_arg_receive`：spilled 寄存器参数收参只处理 GPR | 已修（M6-143）；守卫见 e2e/arm64，但自然程序难以强制 FPR spill |

## KD-11：浮点形参在函数内有调用时收参丢失（读成 0）

- **症状（2026-10-31 实测，x86_64 与 aarch64 都有）**：
  `#[inline(never)] fn f(x: f32) -> f32 { let y = helper(); x + y }`（`helper()` 非内联）
  在 `f(1.0)` 时返回 **9.0** 而不是 10.0 ⇒ `x` 在调用点之后被读成 0。
  同一形态若函数体内没有调用（x 不需要跨调用存活）则正确。
- **根因**：`forge-isa-dsl` 的 `dsl/codegen/frame.rs` `gen_arg_receive` 里，
  “**spilled 的寄存器形参**”收参分支（ABI 寄存器 → scratch → spill 槽）被
  `!__pv.class().is_fp()` 门控 ⇒ **浮点/向量形参被 regalloc spill 后没有收参写槽**，
  槽里是未初始化值；后续 reload 得到 0。真正按栈传递的浮点形参走的是另一条
  （`layout_stack_fpr_stmt`），所以只有“寄存器形参被 spill”这一格漏掉。
- **复现**：`target/tmp/simd-arm64/x86_f32_live.rs`（x86 原生，forge 后端，
  `mainCRTStartup` 返回 `f(1.0) as i32`，期望 10、实测 9）。
- **关闭条件**：在 `gen_arg_receive` 补 FPR spilled 形参收参（统一 spill 机制按类写槽），
  并加 x86 e2e + arm64 QEMU 真执行守卫。**已修（M6-143）**；该条目保留是因为自然程序
  很难强制 FPR 形参 spill，守卫更多是防回归，后续可补手搭 `AllocResult` 的 codegen 单测。

## 其它后端事实（非缺陷）

- **aarch64 DWARF CFI 已接线（2026-10-30，M6-133）**：`scan_arm64_prologue` 扫描
  `sub sp`/存 x30·x29/`add x29` 序言并产出 CFA 行（CFA=x29、RA@CFA-8、x29@CFA-16、
  x19..x28 依次下移）；`.debug_frame` 由 `dwarf.rs` 写入对象。守卫 =
  `rustc_aarch64_debug_frame_cfi_present`（对象里 `.debug_frame` 非空且含
  `DW_CFA_def_cfa_register(29)`）+ `cfi.rs` 的 arm64 扫描器单测。
- 权威清单见 [`backend-gaps.md`](backend-gaps.md)；v27 的进度与验收门槛见
  [`docs/archive/forge-rustc-v27-plan.md`](../archive/forge-rustc-v27-plan.md)（已收口）。

## 维护约定

1. 新增 `#[ignore]` 必须同时：写清 `#[ignore = "…（见 KD-xx）"]`、在本页建条目。
2. 关闭条目时删除本页对应段，并在 `CHANGELOG.md` 记一条（用户可见行为变化）。
3. 本页只放**已知且未关闭**的缺陷；修好后移入 CHANGELOG 历史，不留"已修复"段落。
