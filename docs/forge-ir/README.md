# forge-ir 文档组说明

forge-ir（及依赖方 forge-opt/forge-codegen/forge-hir）的 **LLVM IR 文本层对齐**
工作流在 2026-08 达成本文基线后，其历轮计划/backlog/执行/审计文档已整体归档至
[`docs/archive/forge-ir/`](../archive/forge-ir/)（每篇带 `⚠️ ARCHIVED` 头）。

**基线（归档时）**：LLVM Assembler 官方语料 **452 全收敛**（198 正向 / 254 正确
拒绝 / 0 误接受）、display→reparse roundtrip 0 缺口、workspace 全绿。

## 本组文件

| 文件 | 内容 | 状态 |
| --- | --- | --- |
| `README.md` | 本说明 | active |
| `backlog.md` | 归档时仍未关闭的待办速览（逐项标出处） | active（维护中） |
| `../archive/forge-ir/` | 历轮计划与记录全集（roadmap/checklist/next/remaining×2/audit/display-gaps） | archive |

## 阅读路径（避免迷宫）

- 想了解 forge-ir 曾做过什么/为何这么设计 → `../archive/forge-ir/iteration-roadmap.md`
  （历轮成果）与 `code-quality-audit.md`（质量审计，轮次编号与其不同纪元，勿混比）；
- 想找未完成的事 → 本组 `backlog.md`（来源已标，开工回原档读全文）；
- 想核对当前语法事实 → `../reference/isa-dsl.md`（这是 DSL 规范，非 forge-ir 文档）。

## 迭代约定（续开新轮时）

- 新轮成果请记录在**新文档**（或 `backlog.md` 状态列），勿回写已归档文件；
- 轮次编号建议延续 `iteration-roadmap` 纪元并在文首注明，避免再次出现多纪元混乱。
