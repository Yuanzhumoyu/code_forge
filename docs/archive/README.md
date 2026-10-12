# docs/archive — 历史归档

> 本目录存放**已完成 / 已过时 / 纯历史**的文档。每篇头部带统一的
> `⚠️ ARCHIVED（2026-09）` 块：注明归档原因与现行替代入口。

**读前注意**：归档内容描述的是其记录时点的状态（多数停更于 2026-08），
可能与当前代码不符——**代码现状以仓库代码与 `docs/` 根 README 所列现行文档为准**。

## 归档清单

| 文件 | 内容 | 现行替代 |
| --- | --- | --- |
| `roadmap-status.md` | v14-forge-ir-redesign 路线图交接（P0-P5 执行史） | `crates/tools/forge-rustc/WORKAROUNDS.md`（WA-37/38/39）+ README |
| `isa-dsl-v21-redesign-plan.md` | ISA-DSL v21 重设计（**2026-10-08 归档**：v21 已收口 —— W1–W11 + W6 + W6+ 全部落地；只记设计取舍与切片史） | `isa-dsl-v21-execution-checklist.md`（同期归档的执行清单）+ `../reference/isa-dsl.md`（现行规范） |
| `isa-dsl-v21-execution-checklist.md` | ISA-DSL v21 执行清单（**2026-10-11 归档**：W1–W11 + W6+ 与 §14 最终验收全部完成；只作历史证据） | `../reference/isa-dsl.md`（现行语法 v22） |
| `isa-dsl-variant-field-values.md` | 按变体给字段值（**2026-10-08 归档**：工作**早已落地** —— 键名是 `fields_variant`，`rev8` 正在用；本文留设计取舍与"文档头不可信、先 grep 键名"的教训） | `../reference/isa-dsl.md`（键表 + 变体参数节） |
| `varargs-plan.md` | 变参（varargs）方案（**2026-10-08 归档**：功能面已完成 —— 四份 `va_list` 形状齐备、SysV `%al` 写入 V7 已做；前端是明写的非目标） | `../reference/calling-conventions.md`（现行参考） |
| `forge-ir-v3-plan.md` | forge-ir v3 改进方案（**2026-10-08 归档**：S1–S8 **全部收口** —— S3 三个余项已落地、**S7 余项清零**、S8 经拍板只做二进制序列化且已落地；未关闭项移交 `../forge-ir/backlog.md`） | `../forge-ir/backlog.md`（活跃待办）+ `../reference/binary-format.md`（S8 产物规范） |
| `calling-convention-redesign-plan.md` | 调用约定重设计 v20 A1–A8（**2026-10-08 归档**：A1–A8 全部落地） | `../reference/calling-conventions.md`（现行参考） |
| `forge-isa-dsl-v19-plan.md` | ISA-DSL v19 改进方案（**2026-10-08 归档**：独立接入 / 谱内向量 / lint / 参数化变体 按代码事实核对落地） | `../reference/isa-dsl.md` + `CLAUDE.md`「v21 现行面速查」 |
| `forge-ir-binary-serialization-plan.md` | forge-ir 二进制序列化执行方案 B1–B5（**2026-10-08 归档**：实现已在） | `../reference/binary-format.md`（格式 v1 规范文本） |
| `forge-ir-s8-design.md` | forge-ir S8 可选项设计（**2026-10-08 归档**：只做二进制序列化，另两个候选经拍板不做） | `forge-ir-binary-serialization-plan.md`（同目录执行方案）+ `../forge-ir/backlog.md` |
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
