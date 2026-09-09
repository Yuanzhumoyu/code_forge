# docs/archive — 历史归档

> 本目录存放**已完成 / 已过时 / 纯历史**的文档。每篇头部带统一的
> `⚠️ ARCHIVED（2026-09）` 块：注明归档原因与现行替代入口。

**读前注意**：归档内容描述的是其记录时点的状态（多数停更于 2026-08），
可能与当前代码不符——**代码现状以仓库代码与 `docs/` 根 README 所列现行文档为准**。

## 归档清单

| 文件 | 内容 | 现行替代 |
| --- | --- | --- |
| `roadmap-status.md` | v14-forge-ir-redesign 路线图交接（P0-P5 执行史） | `crates/tools/forge-rustc/WORKAROUNDS.md`（WA-37/38/39）+ README |
| `isa-dsl-v12-roadmap.md` | ISA-DSL v12 方案与迭代史 | `../reference/isa-dsl.md`（v15 规范） |
| `asm-dec-generic-design-v2.md` | 汇编器/解码器 v2 设计蓝本（v13 落地） | `../reference/isa-dsl.md` + forge-dsl 代码 |
| `clippy-fixes.md` | 2026-07 clippy 全量清零记录 | —（已完成） |
| `coverage-history.md` | Windows 覆盖率接入诊断 + 补测交付 | `../guides/coverage.md`（CI llvm-cov 现行） |
| `forge-ir/` | forge-ir 历轮计划/backlog/审计 7 篇 | `../forge-ir/backlog.md`（未关闭待办）+ 代码 |

## 何时归档

- 文档宣称的功能已全部实现/关闭/修复；
- 文档描述的方案已被新方案取代（如 v12 → v15）；
- 文档是单次事件记录或时点快照，之后不再有维护价值。

归档不删除内容；如某项"复活"（重新成为待办/规范），从归档档摘出并注明出处即可。
