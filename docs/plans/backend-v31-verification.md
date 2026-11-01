# 后端 v31：合法化后全量 IR 校验转正

> 状态：**[progress]**（2026-11-01 立项；活账本）
>
> 目标：把"合法化后只跑 use-lists"升级为**完整 `Verifier`**，并修掉它暴露的
> IR 不变量违例。不变量：**生成码语义不变**（金标 + JIT 矩阵），校验失败一律
> fail-closed（`IrError::Verify`，带函数名与错误明细）。
>
> 相关：[`reference/backend-pipeline.md`](../reference/backend-pipeline.md) §3/§16、
> [`archive/forge-ir/backlog.md`](../archive/forge-ir/backlog.md)（GEP 类型债已还）。

## 1. 入口与开关

- `CompileOptions::verify(VerifyLevel::{Auto,Off,On})`（v31）：
  `Auto` = debug 构建开、release 关；`On` 给 CI/探针强制开；`Off` 显式跳过。
- `legalize::schedule::run(..., verify)` 在**所有合法化 pass 之后**跑
  `forge_ir::Verifier::with_ctx`；失败时 debug 构建打印合法化后的 IR 正文，
  返回 `IrError::Verify`（`CompileError::stage() == Legalize`）。

## 2. 修掉的违例（首次开启时 25 条 → 0）

| 类别 | 根因 | 修法 |
| --- | --- | --- |
| `ValueDefMismatch`（宽值） | `LimbEmitter::kill` 手写 `opcode=Nop` + 清 results，没置 `tombstone` 标志 | 改用 `Function::tombstone_inst`（规范形态：标志 + 清附件 + use-lists） |
| `TombstoneNotCanonical`（va 聚合） | 聚合档先 `tombstone_inst(ii)`，随后又走"原指令改 Copy"分支 | 聚合档处理完**提前返回** |
| `LoadAddrNotPointer`（win64 va） | 无保存区基址时把 cursor 的 i64 值直接当 Load 地址 | 显式 `Inttoptr(cur)` 再作地址 |
| `SelectCondNotBool` / `TypeMismatch` | va 的 `Icmp` 结果类型写成 `I8` | 改成 `BOOL` |
| `ConversionBitWidthMismatch`（wide_div） | `ext_from_narrow` 对同宽源仍发 `sext/uext` | 源位宽 ≥ limb 宽时复用源值 |
| `ValueDefMismatch`（call 清结果） | 聚合返回把 call 的 results 清空，旧结果值成为死残值 | Verifier 改为"仅仍被使用才报"（与墓碑/块参数同构） |
| sret 返回漏写缓冲 | 返回值被 `agg_param` 改写成 `ptr` 后按"类型 >8B"筛会漏掉 ⇒ `ret ptr` 留在 void 函数里；`sret` 档应写缓冲 | sret 时收**所有**返回值；大小查 `shadow_agg` / 计划的 `Indirect.size`；ptr 值本身就是地址（events_guard 夹具实测暴露） |
| riscv64 GEP | 谱里没有 `Ptrtoint`/`Inttoptr` 规则 | 补 `MOV_RM8_R64` 直拷规则（x86/arm64 已有） |

## 3. 守卫

- [`tests/verify_after_legalize.rs`](../../crates/backend/forge-codegen/tests/verify_after_legalize.rs)：
  GEP 展开必须通过完整校验；坏类型必须报 `IrError::Verify` 且点名函数。
- [`tests/verify_matrix.rs`](../../crates/backend/forge-codegen/tests/verify_matrix.rs)：
  x86/arm64/riscv64 × 标量/GEP/i128 三形态，全部 `VerifyLevel::On`；**并覆盖 O1/O2/O3**
  （优化产物进入 legalize 的路径）。
- 默认 `CompileOptions::verify` = `Auto`（debug 开）；全量 JIT 矩阵即回归网。
- release 探针（手动，见 §4）：`cargo test --release -p forge-codegen --test verify_matrix`
  ——release 下 `forge-opt` 的逐 pass 校验关闭，末尾合法化校验是最后一道闸。

## 4. 进度

- **2026-11-01 M1**：`VerifyLevel` + 合法化后 Verifier 落地；上表 8 类违例全部修复；
  `--all-features` lib 3104 条、forge-ir 全量、三 ISA 校验矩阵绿；**全量 `cargo forge gate`
  54/54 通过**（含 rustc e2e / jit-x86 / jit-riscv64 / docs）。
- **2026-11-01 M2**：校验矩阵扩到 **O1/O2/O3 × 三 ISA × 三形态**：debug（逐 pass 校验开）
  与 release（逐 pass 校验关，末尾合法化校验兜底）均 3/3 绿；未发现新的 opt/legalize 配合违例。
