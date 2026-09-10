# Markdown lint 工作流（markdownlint-cli2）

仓库 Markdown 格式基准与硬规则见根目录 `CLAUDE.md` →「Markdown 文档规范」；
本文档给出**可执行的检查命令、仓库配置说明与当前存量基线**。

## 检查命令（写/改文档后必跑）

在仓库根目录运行（需 node；本机 npx 缓存受沙箱限制时先设
`npm_config_cache=<workspace>/target/tmp/npm-cache`）：

```bash
# 单个/数个文件（改动自查用，最快）
npx markdownlint-cli2 docs/reference/isa-dsl.md CLAUDE.md

# 全仓（*.md + docs/** + crates/**/README.md；读取根目录 .markdownlint.json）
npx markdownlint-cli2 "**/*.md" "!target"
```

通过 = `Summary: 0 error(s)` 且退出码 0。**未跑过上面任一命令前，
不要声称文档 lint 干净**（规则有版本/配置差异，凭肉眼判断不可靠）。

## 仓库配置（根目录 .markdownlint.json）

```json
{
  "default": true,
  "MD013": {
    "line_length": 120,
    "code_block_line_length": 160,
    "heading_line_length": 120,
    "tables": false
  },
  "MD024": { "allow_different_nesting": true, "siblings_only": true }
}
```

要点（与 CLAUDE.md 规范节保持一致，改任一处须同步另一处）：

- 行宽 120：中文正文按此折行；**表格行不计行长**（`tables: false`——
  表格行无法断行，长单元格合法，无需逐表豁免注释）；
- MD024 `siblings_only`：仅同一父标题下重名报错——多轮/多版本记录
  文档（bench 基线、changelog 结构）跨节复用小节标题不误报。

## 豁免形式（按 CLAUDE.md 规范，均须写明原因）

- **局部行**：`<!-- markdownlint-disable MD013 -->` … `<!-- markdownlint-enable MD013 -->`
- **单文件级**（放文件头部，注明原因；如 `CHANGELOG.md`、`docs/reference/
  aarch64-encoding-ref.md`）：
  `<!-- markdownlint-configure-file { "MD013": { "line_length": 300, ... } } -->`

## 存量基线（2026-09-05 实测，`Summary: 85 error(s)`）

全仓现存违规**仅限历史归档正文**（`docs/archive/forge-ir/` 5 篇长文档，
按归档纪律"内容以记录时点为准"未做格式重构）：

| 文件 | 处数 | 主要规则 |
| --- | --- | --- |
| `archive/forge-ir/code-quality-audit.md` | 37 | MD013/032/031（正文长行、列表与代码围栏空行） |
| `archive/forge-ir/iteration-roadmap.md` | 30 | MD031/032/013 |
| `archive/forge-ir/next-iterations.md` | 10 | MD032/031/013 |
| `archive/forge-ir/iteration-checklist.md` | 6 | MD013/031 |
| `archive/forge-ir/display-gaps.md` | 2 | MD013 |

处理纪律：**不得以"懒豁免"掩盖新违规**——新写/新改文档必须零警告；
上述冻结正文如需改写（如从 archive 恢复为 active），在改写同时顺手清理。
其余文件（根 README/CLAUDE/CHANGELOG、docs/ 全树现行文档、crates
README/WORKAROUNDS）当前全部零警告。

## 写后自查清单（与 CLAUDE.md 第 6 条配套）

1. 跑 `npx markdownlint-cli2 <改动的文件>` → 0 error；
2. 相对链接/锚点可解析（搬动文档后 `git grep` 清理引用）；
3. 状态头与日期已写；首行 H1、单换行结尾；
4. 未引入过时陈述（对照 `docs/README.md` 图例与 archive 头）。
