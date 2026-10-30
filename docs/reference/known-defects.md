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
| KD-05 | NEON 向量未全覆盖（f32x4 算术已接线） | `docs/plans/forge-rustc-v27-plan.md`、`docs/reference/backend-gaps.md` | 属能力缺口，IR 层 fail-closed |

## KD-05：NEON 向量未全覆盖（f32x4 算术已接线）

- **已接线（2026-10-30，M6-134/135）**：`V128` 的 **f32x4** 与 **i32x4** 逐元素
  加/减/乘（`Vadd`/`Vsub`/`Vmul` → `fadd.4s`/`fsub.4s`/`fmul.4s`、`add.4s`/`sub.4s`/
  `mul.4s`，Q 视图 `fprq`）、**splat**（`Vbroadcast` → `dup.4s w` / `dup.2d x`）、
  Q 宽无位移 `ldr q`/`str q`（`Load`/`Store` rd/rs1_width=128）、Q 搬运
  （`VORR16B` `data_width=128` 作 move 别名）。编码全部用 clang oracle 逐字核对；
  守卫 = `lowering_arm64_neon_f32x4`、`lowering_arm64_neon_i32x4_splat`。
- **仅落编码、未接 lowering**：`f64x2` 的 `fadd.2d`/`fsub.2d`/`fmul.2d`
  （谱里已有 inst，运行时仍 fail-closed；elem 谓词在 f64 向量上的取值待排查）。
- **仍未接线**：其它排列/元素（f64x2、i32x4 整数 add/sub/mul、i8/i16…）、
  `Vbroadcast`（splat）、`Vextract`/`Vinsert`、向量比较/位运算的 lowering；
  这些 IR 形态仍 fail-closed。
- **端到端 SIMD 算术还缺一层（不在本缺陷范围）**：rustc 的 `core::simd::Simd`
  是 `#[repr(simd)]` 聚合，`simd_add` 等 intrinsic 需要先有「Simd 类型 → IR V128
  类型」的降级与本后端 intrinsic 映射；x86 目前同样没有（e2e 的 simd 用例只做
  from_array + 取 lane，不做算术）。
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
