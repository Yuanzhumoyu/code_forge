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

# 门禁/CI 用的**同一口径**（全部纳入版本管理的 *.md，**归档除外**）
npx --yes markdownlint-cli2 $(git ls-files '*.md' | grep -v '^docs/archive/')
```

**v26 M3 起这条口径进了门禁**：`cargo forge gate` 的最后一步与 CI 的 `docs` 作业都跑它
（归档文档冻结、明示豁免，见下节基线；`npx` 缺失时 gate 打印 `SKIP` 而不是失败）。

通过 = `Linting: N files` 且 `Summary: N issues in 0 files`、退出码 0。**读摘要要小心**：
`Summary: N issues in M files` 里的 **M = 有问题的文件数**（不是被 lint 的文件数，后者由
`Linting: N files` 那行给出）——只看摘要会把"干净的 N 个文件"误读成"一个都没跑"
（2026-10-07 实测踩过：`Linting: 2 files` + `Summary: 0 issues in 0 files` = 两文件都干净）。
**未跑过上面任一命令前，不要声称文档 lint 干净**（规则有版本/配置差异，凭肉眼判断不可靠）。
另：子目录里的单个文件请用**文档化命令的相对路径形式**（`npx markdownlint-cli2 docs/… CLAUDE.md`
会打印 `Finding:`/`Linting:` 两行，据此确认真的跑到了它）。

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

## 存量基线（2026-10-11 实测，`Summary: 114 issues in 6 files`，**全部在 `docs/archive/`**）

命令：`npx --yes markdownlint-cli2 $(git ls-files '*.md' | grep -v '^docs/archive/')`
（仓库根、markdownlint v0.41.1）⇒ **0 issues**；整树 `"**/*.md" "!target"` 的 114 条**全部**
落在历史归档正文（归档纪律：内容以记录时点为准，未做格式重构）：

| 文件 | 处数 | 主要规则 |
| --- | --- | --- |
| `docs/archive/forge-ir/code-quality-audit.md` | 41 | MD013/032/031/060 |
| `docs/archive/forge-ir/iteration-roadmap.md` | 42 | MD060/032/013/031 |
| `docs/archive/forge-ir/next-iterations.md` | 22 | MD031/032/013/060 |
| `docs/archive/forge-ir/iteration-checklist.md` | 6 | MD013/031/060 |
| `docs/archive/forge-ir/display-gaps.md` | 2 | MD013 |
| `docs/archive/isa-dsl-v12-roadmap.md` | 1 | MD033 |

（逐条总数以摘要为准。）
与 2026-10-07 的 115 条/7 篇相比：**v26 M3 修掉两颗现行文档的钉子**
（`docs/reference/isa-dsl.md` 的连续空行 MD012、`docs/reference/aarch64-encoding-ref.md` 的
行内 `<op>` MD033，后者原本靠文件级豁免掩盖）；归档侧数字变化来自 MD060 表格风格规则的
逐条展开计数（同一批正文，未改内容）。现在**归档之外全树零警告**，且这条口径进了门禁与 CI。

处理纪律：**不得以"懒豁免"掩盖新违规**——新写/新改文档必须零警告；
上述冻结正文如需改写（如从 archive 恢复为 active），在改写同时顺手清理。
其余文件（根 README/CLAUDE/CHANGELOG、docs/ 全树现行文档、crates
README/WORKAROUNDS）当前全部零警告。

## 写后自查清单（与 CLAUDE.md 第 6 条配套）

1. 跑 `npx markdownlint-cli2 <改动的文件>` → 0 error；
2. 相对链接/锚点可解析（搬动文档后 `git grep` 清理引用）；
3. 状态头与日期已写；首行 H1、单换行结尾；
4. 未引入过时陈述（对照 `docs/README.md` 图例与 archive 头）。
