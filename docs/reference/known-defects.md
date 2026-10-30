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
| KD-09 | aarch64 端到端 `Simd<f32,4>` 在“向量值跨多次 lane 读取存活”时错值 | `crates/tools/forge-rustc` 的向量降级 / arm64 spill 路径 | **不可见**：arm64 JIT 矩阵的向量用例已能跑，但 rustc 端到端仍无守卫 |
| KD-11 | 浮点形参（FPR）在函数内有调用时被 spill 后**收参丢失**（读成 0） | `forge-isa-dsl` 的 `gen_arg_receive`：spilled 寄存器参数收参只处理 GPR | 已修（M6-143）；守卫见 e2e/arm64，但自然程序难以强制 FPR spill |

## KD-09：aarch64 端到端 `Simd<f32,4>` 向量值跨 lane 读取存活时错值

- **已修（2026-10-31，M6-142）**：`Simd::<f32,4>::splat` 全零 lane。根因是 SIMD/FP 与
  通用寄存器是**两个寄存器堆**（W13 ≠ S13）：模板 `dup vD.4s, wN` 的源槽是 GPR，而值
  在 FPR；旧规则直接发 `DUP (general)` 读到 0。修法：`fmov w/x{g}, s/d{src}` 先把位型
  搬进 GPR 再 `dup`（谱面规则，不改降级层）。
- **仍错（2026-10-31 实测）**：`target/tmp/simd-arm64/probe.rs` 的完整链路
  （splat/add/mul/sub/div + `let r = -q` + 四次 `q[i]` 求和 + `r[0]`）返回 150
  （`r[0]` 不是 −10）；而孤立 neg、全链后 neg、常量 q + 多次 lane 读取三个差分都正确。
  共同点：**一个向量值在更多活跃标量/lane 读取期间要存活** ⇒ 疑似向量 spill 槽与
  后续临时槽复用或对齐问题，不是 NEON 指令编码。
- **门禁可见性（Vconst 已补后）**：arm64 JIT 矩阵的向量用例已从 Skip 转 Pass
  （`jit_matrix_arm64` 105 passed / 101 skipped），并在 `forge-tests` 有两条 QEMU
  IR 级守卫（splat+算术+多 lane、两向量同时活跃）。但 **rustc 端到端**仍无守卫。
- **关闭条件**：修好上述存活形态并新增 qemu 真执行守卫（完整 splat→算术→lane 读取的
  数值断言），守卫进门禁。

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
- 权威清单见 [`backend-gaps.md`](backend-gaps.md)；进度与验收门槛见
  `docs/plans/forge-rustc-v27-plan.md` 的「仍未覆盖」段。

## 维护约定

1. 新增 `#[ignore]` 必须同时：写清 `#[ignore = "…（见 KD-xx）"]`、在本页建条目。
2. 关闭条目时删除本页对应段，并在 `CHANGELOG.md` 记一条（用户可见行为变化）。
3. 本页只放**已知且未关闭**的缺陷；修好后移入 CHANGELOG 历史，不留"已修复"段落。
