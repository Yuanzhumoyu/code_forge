# forge-ir backlog（活跃待办速览）

> forge-ir LLVM 文本层工作流的历轮计划/记录文档已于 2026-09 归档至
> [`docs/archive/forge-ir/`](../archive/forge-ir/)（各档均带 ARCHIVED 头）。
> 下表为归档时（2026-08 基线：LLVM Assembler 452 全收敛、roundtrip 0 缺口、
> workspace 全绿）**仍未关闭的待办**摘录，每项标注出处档供开工前回读全文。
> ⚠️ 开工前请以代码现状复核（文档停更于 2026-08，仓库已继续演进）。

| # | 待办 | 状态提示 | 出处（archive/forge-ir/） |
| --- | --- | --- | --- |
| 1 | `agg_expand` 第三批（rewrite 族 4 函数 ~400 行）迁移 | ⚠️ **前提已失效（2026-09-12 复核）**：`forge-ir` 内已无 `agg_expand` / `rewrite` 族函数（全 crate `rg` 0 命中）——需回读归档档确认它对应到现在的哪块代码，或直接关闭 | code-quality-audit.md（第 47 轮"剩余状态"） |
| 2 | 结构性重构：`make_inst` 三层包装收敛、egraph 改名、巨型文件拆分、forge-dsl 双语法统一、`isel_strategy` 标签接通、HexLit 超 64 位折叠对齐 | 历轮累积未关闭（低风险/风险面大混合）；**2026-09-12 未逐项复核** | code-quality-audit.md 历轮"遗留/记录"节、remaining-tasks.md §3 |
| 3 | extractvalue/insertvalue 常量**深层**折叠的 builder API 缺口（`fb.extract_value` 仅单层、AggConst 递归 child 缺深层索引） | **仍开放（2026-09-12 复核）**：`fold_extract_value` 只折叠单层（嵌套子聚合走 `AggChild::Agg(_) => None`，见 `crates/middle/forge-opt/src/scalar/const_fold.rs`），`builder.extract_value` 亦单层 | remaining-tasks.md §1.2 |
| 4 | P2 长期：4.1 round-trip fuzz、4.2 负向全覆盖、4.4 端到端执行对照 | 无完成标注，仍开放；**2026-09-12 未复核** | iteration-roadmap.md §4 |
| 5 | 性能候选：grammar 拆分、criterion 基准接入 | 待评估；**2026-09-12 未复核**（注：`docs/performance/` 已有基准框架文档，接入状态需另查） | iteration-roadmap.md §5 |
| 6 | P1 残余：SjLj 异常 codegen（长期）、va_arg ABI 布局语义、>16B 聚合栈传参（现 Unsupported） | 前置条件门控；**2026-09-12 部分复核**：`crates/backend/forge-codegen/src/pipeline/agg_expand.rs` 仍标注「段值拆分 ≤16 字节：寄存器路径；>16 字节 Unsupported」⇒ **聚合**该项仍开放（>16B **向量**已另行走通：by-ref ABI + Load/Store，见 `WORKAROUNDS.md` WA-45） | iteration-roadmap.md §3.2/§2.5、next-iterations.md L2/L7 |
| 7 | Windows SEH/ARM EH codegen（随 ABI 选择）、statepoint（无近期计划）、M1 拆分监控（>15s 触发） | 长期/条件性；**2026-09-12 未复核** | next-iterations.md §0/§3 |
| 8 | forge-grammar semantic 层接入（产品决策）、forge-tests fuzz 归并、jit `register_external` 吞错改 Result 透传 | **2026-09-12 复核**：fuzz 归并 ✅ 已落地（`forge-tests/src/exec/fuzz.rs`，16 条性质测试）；`register_external` ✅ 已是 `Result<(), IrError>` 且透传 patch 错误；**仍开放**：semantic 层零外部调用（`forge-grammar/src/lib.rs` 注明"第二十九轮起 NameResolver/SymbolTable/TypeChecker 零外部调用，mini_c 用自研"） | remaining-tasks.md §3 |

## 2026-09-12 复核记录

本表停更于 2026-08，仓库此后经历 v14/v15 大改；本轮以**当前代码**（`rg` + 阅读源码）复核，
只改有证据的状态列，未复核项如实标注"未复核"（不臆断）：

- 复核命令：`rg` 全 crate 搜索（`agg_expand`/`rewrite` → 0 命中；`extract_value`、
  `NameResolver`/`SymbolTable`、`fuzz` 的命中位置见上表）；
- 结论：**#3、#6（聚合部分）、#8（semantic 层）仍开放**；**#1 前提失效**；
  **#8 的 fuzz 归并与 `register_external` 已落地**；#2/#4/#5/#7 未复核；
- 纪律：本表只作入口索引——开工前**必须**按上表逐项回读源码，勿据表内旧陈述直接动手。

## 工作流约定（源自归档档，仍适用）

- 新开工一轮：从本表挑项 → 完成后成果回写（建议新开轮次记录，勿回写已归档档）→
  更新本表状态列；
- 验收纪律与命令速查见 archive/forge-ir/ 各档（iteration-checklist/remaining-* §4）与
  `docs/archive/forge-ir/README.md` 组说明。
