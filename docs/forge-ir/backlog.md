# forge-ir backlog（活跃待办速览）

> forge-ir LLVM 文本层工作流的历轮计划/记录文档已于 2026-09 归档至
> [`docs/archive/forge-ir/`](../archive/forge-ir/)（各档均带 ARCHIVED 头）。
> 下表为归档时（2026-08 基线：LLVM Assembler 452 全收敛、roundtrip 0 缺口、
> workspace 全绿）**仍未关闭的待办**摘录，每项标注出处档供开工前回读全文。
> ⚠️ 开工前请以代码现状复核（文档停更于 2026-08，仓库已继续演进）。

| # | 待办 | 状态提示 | 出处（archive/forge-ir/） |
| --- | --- | --- | --- |
| 1 | `agg_expand` 第三批（rewrite 族 4 函数 ~400 行）迁移 | 需专用工具/步骤，未实施 | code-quality-audit.md（第 47 轮"剩余状态"） |
| 2 | 结构性重构：`make_inst` 三层包装收敛、egraph 改名、巨型文件拆分、forge-dsl 双语法统一、`isel_strategy` 标签接通、HexLit 超 64 位折叠对齐 | 历轮累积未关闭（低风险/风险面大混合） | code-quality-audit.md 历轮"遗留/记录"节、remaining-tasks.md §3 |
| 3 | extractvalue/insertvalue 常量**深层**折叠的 builder API 缺口（`fb.extract_value` 仅单层、AggConst 递归 child 缺深层索引） | 未实现（工作量小-中 + 验收） | remaining-tasks.md §1.2 |
| 4 | P2 长期：4.1 round-trip fuzz、4.2 负向全覆盖、4.4 端到端执行对照 | 无完成标注，仍开放 | iteration-roadmap.md §4 |
| 5 | 性能候选：grammar 拆分、criterion 基准接入 | 待评估 | iteration-roadmap.md §5 |
| 6 | P1 残余：SjLj 异常 codegen（长期）、va_arg ABI 布局语义、>16B 聚合栈传参（现 Unsupported） | 前置条件门控 | iteration-roadmap.md §3.2/§2.5、next-iterations.md L2/L7 |
| 7 | Windows SEH/ARM EH codegen（随 ABI 选择）、statepoint（无近期计划）、M1 拆分监控（>15s 触发） | 长期/条件性 | next-iterations.md §0/§3 |
| 8 | forge-grammar semantic 层接入（产品决策）、forge-tests fuzz 归并、jit `register_external` 吞错改 Result 透传 | ⚠️ 记录待处理 | remaining-tasks.md §3 |

## 工作流约定（源自归档档，仍适用）

- 新开工一轮：从本表挑项 → 完成后成果回写（建议新开轮次记录，勿回写已归档档）→
  更新本表状态列；
- 验收纪律与命令速查见 archive/forge-ir/ 各档（iteration-checklist/remaining-* §4）与
  `docs/archive/forge-ir/README.md` 组说明。
