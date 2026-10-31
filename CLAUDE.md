# CLAUDE.md

## Documentation Map（文档地图）

仓库文档按**职能**分目录存放（2026-09 重组）。每篇状态标记：
`[active]` 现行维护 · `[progress]` 进行中 · `[archive]` 历史归档（头部 ARCHIVED，
仅供参考、代码为准）。**先读 `docs/README.md`**（完整索引），再按主题进子目录。

```text
docs/
├── README.md              # 文档总索引 + 状态图例
├── reference/             # 现行规范/设计参考 [active]
│   ├── isa-dsl.md             # ISA-DSL 语法规范（v22 现行；改 isa/*.toml 先看它）
│   ├── isa-dsl-errors.md      # ISA-DSL 错误码目录（报错看不懂先看它）
│   ├── calling-conventions.md # 调用约定层 forge-abi（v20 A1：规则/绑定/计划 + forge-isa abi）
│   ├── aarch64-encoding-ref.md# A64 编码参考（arm64 后端/golden 依据）
│   ├── backend-pipeline.md    # 后端编译管线契约（v23 M1：CompileSession/JitLoader/ModuleSigs）
│   ├── backend-gaps.md        # 后端能力缺口台账（`[cap:ID]` + 决策 + 双向守卫；条数以台账为准）
│   ├── generality-audit.md    # **v28 通用性审计**：写死/未接线问题 GEN-01…08（占位符、ISA 接入、插件、宽度、动态产物、分配器）
│   ├── constants.md           # 常量层（v28：BitValue 比特值 / ConstId 四元枚举 / ConstantPool 四通道 / AsConst）
│   └── imm_str.md             # ImmStr 类型设计（forge-ir 代码注释引用）
├── plans/                 # 进行中专项方案（活账本）[progress]
│   └── forge-v28-generality-plan.md # **forge v28 通用性整治**（M0 审计 / M1 占位符数据化 /
│                                    #   M2 ISA 接入零本体改动 / M3 插件零胶水 / M4 宽度泛化 /
│                                    #   M5 动态产物 / M6 分配器缺陷 / M7 收口）
├── forge-ir/              # forge-ir 工作流 [active]
│   ├── README.md              # 组说明（历轮计划已归档、如何继续）
│   └── backlog.md             # 未关闭待办速览（出处指向 archive/forge-ir/）
├── performance/           # 基准与优化
│   ├── BENCHMARKS.md          # 基准运行框架
│   ├── OPTIMIZATION.md        # 优化清单
│   ├── bench_baseline.md      # 基线 + 历轮实测
│   ├── generated_compile_profile.md # 生成物编译画像（v19 V7：分段体积/编译时间归属 + 表化结论）
│   └── codegen_stage_profile.md # codegen stage 占比
├── guides/                # 工具与方法
│   ├── isa-dsl-tutorial.md    # ISA-DSL 30 分钟接入教程（新 ISA 从这里开始）
│   ├── coverage.md            # cargo-llvm-cov 覆盖率工作流（含下限棘轮）
│   ├── lint.md                # markdownlint 检查命令 + 存量基线（写文档后自查）
│   └── testing-notes.md       # **测试与夹具笔记**（v26 M5 从本文件搬出，约 300 行：
│                              #   JIT 矩阵/汇编三档/生成物落盘/谱内向量/工具链 lint 档/…）
└── archive/               # 历史归档（⚠️ 内容以其记录时点为准）
    ├── README.md              # 归档图例与清单
    ├── roadmap-status.md / isa-dsl-v12-roadmap.md / asm-dec-generic-design-v2.md
    ├── isa-dsl-v12-v17.md     # ISA-DSL v12–v17 语法史 + v18 删除/改名总表
    ├── ymm-abi-plan.md        # YMM ABI 方案（2026-09-10 核查完成 → 归档）
    ├── hir-shrink-plan.md     # HIR/mini_c 收缩（同日均告终结 → 归档）
    ├── clippy-fixes.md / coverage-history.md
    ├── isa-dsl-v21-redesign-plan.md / isa-dsl-v21-execution-checklist.md  # ISA-DSL v21 方案与执行清单（均已收口归档）
    ├── forge-ir-binary-serialization-plan.md / forge-ir-s8-design.md # forge-ir 二进制序列化（落地后归档）
    ├── varargs-plan.md / isa-dsl-variant-field-values.md # 变参（功能面已完成）/ 按变体给字段值（早已落地）
    ├── forge-ir-v3-plan.md      # forge-ir v3（S1–S8 全部收口；未关闭项移交 ../forge-ir/backlog.md）
    ├── forge-rustc-vec_push-plan.md # forge-rustc vec 族（2026-10-11 归档：验收 4/4 达成，e2e 103 条用例全绿）
    ├── project-v26-plan.md       # 项目 v26 里程碑 M1–M5（2026-10-11 归档：全部收口；现行入口 = reference/backend-pipeline.md + backend-gaps.md + guides/testing-notes.md + CHANGELOG）
    ├── backend-v23-plan.md     # 后端 v23 里程碑 M1–M6 + M7（2026-10-11 归档：全部收口；现行入口 = reference/backend-pipeline.md + CHANGELOG）
    ├── backend-v24-plan.md     # 后端 v24 里程碑 M1–M5（2026-10-11 归档：全部收口；现行入口 = reference/backend-pipeline.md + backend-gaps.md + CHANGELOG）
    ├── backend-v25-plan.md     # 后端 v25 里程碑 M1–M5（2026-10-11 归档：全部收口；现行入口 = reference/backend-pipeline.md（一次分类/错误分类学/规模棘轮/事件表）+ CHANGELOG）
    └── forge-ir/              # forge-ir 历轮计划/审计 7 篇（452 收敛基线，2026-08 停更）
```

仓库根与 crate 文档（不在 docs/ 下）：

- `README.md` 项目主页 · `CLAUDE.md` 本文件 · `CHANGELOG.md` 更新日志
- `crates/*/README.md` 与 `crates/tools/forge-rustc/WORKAROUNDS.md`（机读绕法清单
  `[WA-NN]`）——源码级文档，随 crate 走。

常见误解提示：

- **forge-ir 的轮次纪元不统一**（audit 第 47 轮 ≠ roadmap 第 31 轮）——都已在
  archive/forge-ir/，继续 forge-ir 迭代请按 `docs/forge-ir/README.md` 约定新开记录；
- **archive 内"已实现/待办"不代表代码现状**——改代码/加测试前以源码与 test 为准；
- ISA-DSL 现行语法以 `reference/isa-dsl.md` 为准，**v21 重设计（2026-10）已收口**：
  表头即名字的三张表（`[form.<名>]`/`[inst.<名>]`/`[pred.<名>]`）、`[operand.<名字>]`、
  `[enum.<表名>]`、`[family.<名>]`、`[asm.*]`、`[machine.*]` 都已就位；**v21 的进度与验收记录**
  已归档（`archive/isa-dsl-v21-execution-checklist.md`，只作历史证据），
  **方案本身**也已归档（`archive/isa-dsl-v21-redesign-plan.md`）。
  注意区分两种"版本号"：DSL 语法版本（**v22，现行**；靠文档/CHANGELOG 追踪，**不写进 TOML**）
  与 `[meta].version`（ISA 自己的自由字符串）。**文件名不带语法版本**——旧的 `*_v12`
  命名（`isa/*_v12.toml`、`src/v12/`、`arch/*_v12.rs`、`tests/*_v12_tests.rs`）已于
  2026-10 清除，别再加回来。

## Markdown 文档规范（格式基准：markdownlint v0.41.1）

本仓库所有 `.md` 的**格式基准** = [markdownlint v0.41.1](https://github.com/DavidAnson/markdownlint/tree/v0.41.1)
（CommonMark + GFM；全部规则默认启用）。新写/改动文档应先满足下列硬规则；
确需豁免时用行内注释（见下）并在注释旁写原因。

### 格式硬规则（不豁免）

- **标题**：文件首行必须是唯一 `# H1`（MD041/MD025）；用 atx 风格（MD003）且
  `#` 后留空格（MD018）；层级每次只递增一级（MD001）；标题前后空行
  （MD022）、不尾随标点（MD026，中文标题的 `。`/`：` 亦算）、不重复同文标题
  （MD024）；不加粗文本冒充标题（MD036）。
- **空白**：无尾随空格（MD009）、无硬 Tab（MD010，缩进用空格）、连续空行
  至多一行（MD012）、文件以单个换行结尾（MD047）。
- **列表/引用**：列表符号风格一致且同层对齐（MD004/005/007/032）、列表项
  前后空行、`>` 后一个空格且引用块内不夹空行（MD027/028）、有序号前缀一致
  （MD029）。
- **代码块**：一律围栏式（MD046）、围栏与正文间空行（MD031）、**必须标注语言**
  （MD040，如 `rust`/`bash`/`toml`/`text`）、围栏风格一致用三个反引号（MD048）。
- **链接**：相对链接可解析、不用反向/空链接（MD011/042）、裸 URL 一律包进
  `<...>` 或链接文本（MD034）、锚点片段有效（MD051）、引用式链接须有定义
  （MD052/053）。链接文本应描述内容（MD059，避免"点这里/详见此处"）。
- **表格**：表头用管道行分隔（MD055）、各行列数一致（MD056）、表格前后空行
  （MD058）。
- **图片**：一律有 alt 文本（MD045）。
- **行内元素**：强调/代码/链接标记内侧无空格（MD037/038/039）。

### 仓库配置（行宽适配中文/表格）

仓库**中文正文与长表格占多**，根目录 `.markdownlint.json` 配置如下
（**实际生效配置以此为准**，md 内豁免注释见下节）：

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

- 行宽基准 = **120 列**（中文正文/正文行按此折行；默认 80 会让中文行频报）；
- `tables: false`：**表格行不计入 MD013**——表格行无法断行，长单元格合法，
  无需逐表加豁免注释；
- `MD024.siblings_only`：仅同一父标题下重名才报——多轮/多版本记录文档
  （bench 基线、changelog 结构）跨节复用小节标题不误报；
- 将来接入 CI lint 时命令示例：
  `npx markdownlint-cli2 "**/*.md" "!target"`（读取同一 `.markdownlint.json`）。
- 完整命令与存量基线见 `docs/guides/lint.md`（**写/改文档后跑文件级命令自查**）。

### 局部豁免写法（必须写明原因）

```markdown
<!-- markdownlint-disable MD013 -->
这一行超长且有理由（长 URL/长公式/不可断行的代码路径）。
<!-- markdownlint-enable MD013 -->
```

### 编写注意事项（约束编写者，超出 lint 的语义要求）

1. **归类与边界**：先想清文档职能再落笔——现行规范/参考 → `docs/reference/`；
   未完成专项 → `docs/plans/`；基准度量 → `docs/performance/`；工具用法 →
   `docs/guides/`；forge-ir 工作流 → `docs/forge-ir/`；**已完成/已过时/单次
   记录 → `docs/archive/`**（加 `> ⚠️ ARCHIVED（日期）` 头并注明现行替代入口）。
   用户可见变更只进 `CHANGELOG.md`（Keep a Changelog）；内部迭代过程记录不进
   CHANGELOG；crate 专属内容放 crate 内 README/WORKAROUNDS，勿堆进 docs/。
2. **状态与时效（本仓库最重要的教训）**：文档头标 `[active]/[progress]/
   [archive]` + 日期；**不要写"持续更新"除非你真的每轮迭代都回写**——停更就
   如实标停更日期；数值/基线（bench、覆盖数、用例收敛数）必须记采集日期、
   命令与代码基线；**一切状态以代码与测试为准**，文档只是索引与记录。
3. **引用纪律**：交叉引用写仓库相对路径（如 `docs/reference/isa-dsl.md`）；
   搬动/改名文档后 `git grep` 全量更新引用并清除死链；禁止引用已删除文件；
   引用代码行号/提交号时注明"随迭代漂移，以符号名为准"。
4. **结构与篇幅**：一篇文档只讲一个主题（混职能则拆段，历史段挪 archive）；
   长文（>500 行）开篇加目录；重复内容用链接引用代替复制粘贴（单一事实源）。
5. **语言与术语**：正文语言跟随文件既有语言（不中英混杂叙述）；代码/标识符/
   TOML 键/路径原样 code font；术语大小写一致（forge-ir、ISA-DSL、markdownlint
   等）；涉及轮次编号的迭代文档先声明所用纪元（历史上 forge-ir audit 与
   roadmap 纪元曾不一致）。
6. **提交前自查清单**：对**改动的每个文件**实跑
   `npx markdownlint-cli2 <文件>`（读仓库 `.markdownlint.json`）到 0 error
   （豁免须带注释；命令/豁免形式/存量基线见 `docs/guides/lint.md`）；相对
   链接/锚点全可解析；状态头与日期已写；首行 H1 且单换行结尾；未引入过时
   陈述（对照 `docs/README.md` 图例与 archive 头）。

## 编辑纪律（改代码怎么动手）

**默认：改代码用 `edit`/`write` 的精确文本替换，一次一处、改完立刻验证；脚本只做机械操作。**

这条规则要治的具体毛病：批量脚本"成功退出"了，但改动其实落在错的地方、或漏了某些文件——
树里留下一堆看起来对、跑起来错的东西，而且没法 review。手改的成本是几行 diff，
脚本出错的成本是一次**假绿**。

### 允许用脚本（机械操作，穷举）

- **同名批量改名**：同一标识符跨多文件的字面量替换；
- **同一处字面量在 N 个文件里重复替换**（如同一个 API 调用换签名）；
- **生成/格式化**：表驱动生成物、`cargo fmt`、统一行尾/编码；
- **只读统计**：数出现次数、列文件清单、比对两棵树（**不写文件**）。

### 禁止用脚本（必须手改）

- 任何**语义变更**：加/删控制流、改条件、换算法、改错误处理路径；
- **修编译错误**：一条条读完再改，不许用正则"猜"改法；
- **新写代码**：新函数、新分支、新测试用例的正文；
- **按模式推断的改写**（启发式正则 / AST 重写）：改错不会报错。

### 真要用脚本，四条硬要求（缺一即违规）

1. **精确字面量替换**，不做"聪明"匹配；需要上下文就把上下文写进 `old` 串；
2. **断言命中数**：每处替换前断言 `count == 期望`，命中 0 或超出立即退出——宁可红，
   不许静默；
3. **改完看 diff**：`git diff --stat` + 抽查改动文件（改名类必须逐处看）；
4. **改完跑验证**：`cargo fmt --all` + 受影响 crate 的 `cargo check` / 对应测试；
   **跨 crate 的 API 迁移还要跑一次全量 clippy**（子集门禁 `quick`/`rustc` 都**不含**
   全量 lint，这是红点漏网的常见路径）。

只碰任务点名的文件：脚本不遍历全仓乱改，改完的文件清单要能逐条说出理由。

### 本仓踩过的实例（为什么写这条）

- v28 删 `iconst_*`/`fconst_*` 糖时用脚本做跨 crate 迁移，**漏了
  `benches/compile_bench.rs` 的 37 处**——脚本正常退出，红点到几轮后跑全量 clippy 才暴露；
- 同一次迁移里十六进制/下划线字面量缺类型后缀，脚本按 `git diff` 配对补完后仍要
  **手工修** `runtime/jit.rs`、`tls_runtime.rs` 等 20 处；
- PowerShell 写脚本还要当心：`$home` 是只读自动变量；`.NET` 静态调用用的是**进程 CWD**
  而不是 `Set-Location` 之后的路径（两条都实际踩过，报错还很误导）。

## 生成物落盘（别弄脏工作区）

**规矩：任何"跑出来的东西"一律写进 `target/`。** 工作区里出现的每个文件都要能说出
"它为什么属于仓库"；说不出的就该在 `target/` 下（`/target` 已在 `.gitignore`）。

**"被 gitignore" ≠ "目录干净"**：`.gitignore` 只让 `git status` 好看，目录里照样堆垃圾
（本项目实测：仓库根 14 个 `rustc-ice-*.txt`、`.npm-cache/`，示例工程里 `.cargo/` +
`target-o*/target-llvm-o*/target/`）。检查要用 `Get-ChildItem -Force`，不能只看 `git status`。

| 生成物 | 落到 | 做法 |
| --- | --- | --- |
| npm/npx 缓存 | `target/.npm-cache` | `$env:npm_config_cache = "<repo>/target/.npm-cache"` |
| cargo 包缓存/解包 | `target/cargo-home` | `$env:CARGO_HOME = "<repo>/target/cargo-home"`（沙箱下也是必需，默认 `~/.cargo` 会 `os error 5`） |
| 探测/示例工程的构建 | `target/<工程名>/*` | `cargo build --target-dir <repo>/target/…`；**集成配置**放 `target/…/cwd/.cargo/config.toml`，用 `--manifest-path` 指回工程（cargo 从 **CWD 及父目录**读配置），工程目录里什么都不留 |
| 一次性脚本/抓取物/IR dump | `target/tmp/` | 别写在仓库根 |
| rustc ICE 转储 | 不出现在工作区 | `$env:RUSTC_ICE = "0"`；或在 `target/` 下的目录里跑 rustc（它写 CWD） |
| 基准/实验的多档产物 | `target/<名字>-o<档>` | 与正式 `target/` 分开，随时可整目录删 |

**两条实测教训**：

- `run-offline.ps1` 曾用 `RUSTFLAGS` 环境变量代替 `.cargo/config.toml` —— **不等价**：
  `RUSTFLAGS` 连 host 侧 `std_detect` 也一起塞进 forge 后端，ABI 断言当场炸
  （`rustc FnAbi expects 0, forge packs 1`）。集成必须走 config；
- wrapper 只认 **`RUSTC_WRAPPER`**（`CARGO_BUILD_RUSTC_WRAPPER` 无效）：漏了就会用 forge 去编
  `core`/`std`，报一堆 `Unsupported: SIMD type` / intrinsic 错误，看起来像"后端缺能力"，
  其实是配置没生效。

## Build Commands

**v26 M3 起，全套门禁只有一条命令**（`cargo forge gate`，逐项报告 + 非零退出；
CI 的 Linux/Windows 作业也复用它的子集）：

```bash
# 全套（本机默认口径；约 6–10 分钟）
cargo run -p cargo-forge -- gate          # 或把 target/debug 加进 PATH 后：cargo forge gate

# 常用变体
cargo forge gate --guards                 # 只跑守卫注册表（条数以 GUARDS 表为准，约 1 分钟）
cargo forge gate --subset quick           # fmt + 守卫 + 文档
cargo forge gate --subset linux|windows   # CI 的两套口径（Linux 不含 forge-codegen 的 JIT 测试）
cargo forge gate --subset rustc           # **只跑后端**（fmt + e2e + 三个后端守卫 + 文档，约 3–4 分钟）
cargo forge gate --list                   # 只列出将要跑的步骤
cargo forge gate --json                   # 机器可读汇总

# 单个命令（gate 跑的就是这些；需要单点排查时用）
cargo fmt --all -- --check
cargo clippy --workspace --exclude forge-rustc --all-targets --all-features -- -D warnings
cargo clippy -p forge-ir --no-default-features --lib -- -D warnings   # 文本层门
cargo test --workspace --exclude forge-rustc
cargo test -p forge-codegen --lib --all-features   # `jit` + `parallel`（**不要**对 workspace 开 --all-features：
                                                   # forge-tests 的 nightly feature 会拉 rustc_private）
cargo check -p forge-rustc                          # 需要 nightly + rustc-dev
```

门禁纪律（沿用）：**一个任务一次提交**、里程碑未完成不提交、每个里程碑以全量门禁 + 新增守卫验收、
删除清单清零、守卫白名单条目必须被命中、预算/棘轮只许按实测下调。

## Project Architecture

`code-forge` is a specification-driven compiler infrastructure. Users write:

1. **ISA TOML files** (`isa/*.toml`) defining instruction encodings, register banks, and lowering rules
2. **Grammar files** (`.lx` EBNF format) defining assembler syntax

The `forge-dsl` proc-macro (`isa_from_file!`) compiles TOML → Rust code at build time, generating the
complete ISA module (instructions, encoder, disassembler, assembler, lowering).

### Crate Dependency Graph

```text
code-forge (root umbrella)
├── forge-ir          (no internal deps)
├── forge-isa-runtime  (no internal deps；生成物的 trait/数据型 + 主机探测 `cpu` + 诊断出口
│                       `diagnostics`（事件通道 + 观测开关台账）+ `mem`（可执行内存，v26 M2 从
│                       已删除的 forge-mem 折叠进来）)
├── forge-abi-model   (leaf：AbiPlan/Placement/RetLoc/TyView + 两个规则枚举 + 唯一的 IR→TyView 投影；依赖 serde+forge-ir)
├── forge-abi         → forge-abi-model, forge-ir（规则/绑定/引擎；约定由使用者提供）
├── forge-opt         → forge-ir
├── forge-codegen     → forge-ir, forge-opt, forge-isa-runtime, forge-dsl
├── forge-dsl         (proc-macro 薄层：解析 isa_from_file! 参数 → forge-isa-dsl)
├── forge-isa-dsl     (普通 lib：ISA-DSL 编译器本体——模型/解析/校验/诊断/代码生成)
├── forge-grammar     (no internal deps；CST/AST/visit + ir.lx 测试。**v26 M2 起它在下游只剩示例**)
│                     （`forge-hir` 一个消费者，见下条与「前端线已降级为示例」）
# ── 前端线（forge-hir / forge-hir-macro / mini_c）：**v26 M2 起降级为示例** ──
# 三个目录已移到 `examples/hir-demo/`（hir/、hir-macro/、mini_c 内容）并从工作区 `exclude`：
# 不再被 `cargo test --workspace` 与 CI 构建。手动检查：
#   cargo check --manifest-path examples/hir-demo/Cargo.toml
├── forge-object      → forge-ir, forge-codegen
├── forge-plugin      → forge-codegen
├── forge-rustc       → code-forge (强制 object-file/plugins features)
└── forge-tests       → code-forge, forge-rustc(optional, nightly feature)
```

### Key Architecture Rules

1. **`isa_from_file!`（现行 ISA-DSL v22 语法）生成的代码只依赖运行时 crate** — v19 V1b 起
   路径恒为绝对路径，宿主是哪个 crate 不再影响生成物内容。
   生成模块名 = 文件 stem（可用 `name = "…"` 覆盖）。**没有 `krate` 参数**：生成物
   一律写绝对路径 `forge_isa_runtime::…`（`forge_ir::…` → `forge_isa_runtime::ir::…`），
   因此**任何** crate 只要依赖 `forge-isa-runtime`（+ build script 预生成）就能承载一份谱——
   demo 夹具正是这样住在 `tests/isa/*.toml` + `tests/common/mod.rs` 而不进库本体，
   最小可抄的**外部宿主** = `examples/isa-host-demo`（运行期只依赖 `forge-isa-runtime`，
   build-dependency 只有 `forge-isa-dsl`，`parts` 只取 encode/decode/asm 而仍开着生成期自测；
   依赖面与生成物文本级守卫在它的 `tests/host_surface.rs`，是 v19 V2 的 G1 证据）。
   `parts` 含 `tm` 时宿主须注册编译管线（`forge_isa_runtime::register_pipeline`，
   forge-codegen 的 `pipeline_hooks::ensure_registered` 已接好）。
   库本体只有真实后端：`arch/{x86,arm64,riscv64}.rs`。

2. **Assembler/JIT coupling 已解**（v19 V1）——`forge-isa-runtime` 持有生成物的全部 trait
   与数据型（`machine/*`、`LowerCtx`/`MemRef`、`AllocResult`/`CodeSink`/`CompiledFunction`），
   编译管线（JIT/regalloc/发射）留在 forge-codegen 并通过
   `forge_isa_runtime::register_pipeline(ISA 名, 工厂)` **注册**给运行时；
   `impl_erased_target_machine!` 只查注册表（未注册 ⇒ `IrError::Unsupported`）。

3. **Proc-macro limitation** — `forge-dsl` 是 proc-macro crate；Rust 禁止它导出非宏项。
   因此**编译器本体在 `forge-isa-dsl`**（普通 lib：模型/解析/校验/诊断/代码生成 +
   `expand_file`/`validate_file` 入口），`forge-dsl` 只是**纯转发**（v18 S10d 起连参数
   解析都在 `forge-isa-dsl::gen_file::parse_macro_args`，因为 build script 预生成要走
   同一份解析）。同理 `MemRef` 等生成物要用的类型定义在 forge-codegen，不在 DSL crate。

4. **生成物由 build script 生成，不是宏**（v18 S10d）——`forge-codegen/build.rs` 调
   `forge_isa_dsl::pregenerate_host()`；生成物落在 `$OUT_DIR`，宏只发 `include!`。
   两条硬约束（RA 只能加载分析开始前存在的文件；`TokenStream::to_string()` 上下文相关）
   见 `docs/guides/rust-analyzer-notes.md` §1 与 [`docs/guides/testing-notes.md`](docs/guides/testing-notes.md)。

5. **自包含 asm** — 生成模块内联实现 assemble（v17 起**整条 `asm` 模板左→右 token 扫描**，
   同助记符多 form 按类型签名自动分发；**没有 `match 助记符` 前置分派**），不再经
   lalrpop 语法与 forge-asm 运行时（v11 时代已随语法层删除）。`TargetAssembler` trait
   （`crate::machine::assembler`）仅要求 `parse_insts`。同一份候选还导出一个**诊断探针**
   `could_be_instruction(text) -> bool`（只跑扫描的第一步 = 首段匹配；`false` ⇒ 一定不是
   本 ISA 的指令）——**不要**用"取 `asm` 首词当助记符表"这类生成期切词来判断"本 ISA 认不认
   这条指令"：模板可以操作数前置、首段可以多 token、空白分词 ≠ 扫描器的 token 切分。

6. **调用约定是使用者的数据，ISA 只申报能力**（v20 A1/A2）——`forge-abi` 把这件事拆成
   三层：`AbiRules`（约定，平台无关的 TOML）→ `AbiBinding`（(ISA, 约定) 的寄存器绑定）→
   `AbiPlan`（引擎产物，调用方与被调方共用）。**IR 只声明"用哪份"**：
   `CallConvId::{Builtin(ConvName), Named(ImmStr), Index(u32)}`（旧的 16 变体 `CallConv`
   已删——它既不是公共约定、也没人读）；宿主注册表
   `forge_codegen::pipeline::conv_registry` 在编译入口解析成注册表键（未注册 ⇒ **fail-closed**，
   结果落在 `LowerCtx::conv.name`——调用约定相关的一切都在 `LowerCtx::conv: CallConvCtx`
   里（`id`/`name`/`layout`/`clobbers`/`ret_gpr`/`module_sigs`），A3 起用它查规则/绑定）。
   谱侧只提供 `AbiTarget`（有哪些寄存器/宽度/能力），没有宿主后端时由
   `forge_isa_dsl::abi_view` 从谱本身建出这份视图。三条连带纪律：① **谱侧只有两件事**：
   能力申报（`roles`）与机器事实（`[machine]`）——`[abi]` 已**整节删除**（A5-3），写它会报未知键；
   ② 返回寄存器
   与参数寄存器是**两套池**（x86 返回在 RAX、参数从 RCX 起），HFA 的槽数按类型取
   （`slots = "hfa"`），宽返回的 sret 落点**每份约定不同**（x86=RCX/RDI、AAPCS64=x8、
   riscv=a0）——这三处正是旧实现写错值的地方；③ 改 `CallConvId` 的编码要**同时**升
   `IR_FORMAT_VERSION`（**当前 4**：v26 M2 删 `isel_strategy` 时升过；v3 是 `CallConvId`）并改
`docs/reference/binary-format.md` 的版本历史表。
   ④ **`c` 是抽象名，靠约定的 `aliases` 落地**（A3b-2b-2a）——IR 的缺省
   `CallConvId::Builtin(C)` 不带平台信息，"这台机器上的 C 是哪一份"写在**规则**里
   （`AbiRules::aliases`：内置 `win64`/`aapcs64`/`lp64d` 声明 `aliases = ["c"]`，
   `sysv64` 不声明；**整套代答** = 规则 + 该机绑定同源，只让绑定代答会得到 by_class
   的槽位与 RCX 返回，实测三个 JIT 用例红；同一机两份声称 ⇒ 报错）。**收参与序/尾声都由
   生成器发射**：`@move_args`（A3b-2b-2b）与 `[emit.prologue|epilogue]` + 四个伪指令（A4）
   **都已从谱面撤出**（写了会被明确拒绝），生成器按 `AllocResult::call_layout` 收参、
   按机器事实 + 角色生成序/尾声（`frame_alloc`/`frame_free`/`frame_set`/`push`/`pop`/
   `callee_save`/`callee_load`/`ret`）；落在发射侧还没接的落点（`Pair`/`Group`/无指针
   `Indirect`）⇒ **明确 `Unsupported`**（A5-3 起没有旧路径可退）；**没有 plan 是编译错误**
   （A6 的 fail-closed）。调用点 API 的形状面是 `CallRequest`（`conv`/`args`/`rets`/
   `variadic` 一行 builder），失败返回带引擎原文的 `CallPlanError`。
   参考 `docs/reference/calling-conventions.md`，分期 `docs/archive/calling-convention-redesign-plan.md`。

### ISA Backend Pattern

```rust
// 发行后端：crates/backend/forge-codegen/src/arch/my_isa.rs
forge_dsl::isa_from_file!("isa/my_isa.toml");
pub use self::my_isa::*; // 生成 TargetMachine / Inst / Reg 等全套组件
// 注册由 DSL 生成的 ensure_registered() 完成（OnceLock 注册 Registry + reloc patcher），
// 无需手写——见 arch/x86.rs 的实际形态。

// 测试夹具（不发行）：crates/backend/forge-codegen/tests/common/mod.rs
forge_dsl::isa_from_file!("tests/isa/demo.toml");
```

> **宿主必须接 build script（v18 S10d）**：生成物是 `$OUT_DIR` 下的**文件**
> （`forge_gen_<模块名>_<参数哈希>.rs`），`isa_from_file!` 只展开成一句 `include!`；
> 文件由 build script 预生成——`build.rs` 里 `forge_isa_dsl::pregenerate_host()`，
> `Cargo.toml` 里 `[build-dependencies] forge-isa-dsl = { path = … }`（`forge-codegen`
> 两者都已就位）。没接就**编译期**报错并点名这两步。原因（RA 只能加载分析开始前存在的
> 文件 + `TokenStream::to_string()` 上下文相关）见
> [`docs/guides/rust-analyzer-notes.md`](docs/guides/rust-analyzer-notes.md) §1，
> 规范见 `docs/reference/isa-dsl.md`「宿主接入」。
> 注：生成模块导出的是 `TargetMachine`（组合 IsaInfo/RegInfo/ABI/Lowering/Encoder/
> FrameLowering/Disassembler/Assembler/**Decoder**），**没有 `Isa` 类型**；
> 无 `register_backend!` 宏。v11 后端（x86_64/aarch64/riscv64/wasm32/minimal_sd）
> 已随 v11 语法层删除——现为 `arch/x86.rs`、`arch/arm64.rs`、
> `arch/riscv64.rs`（三者均已接 TargetMachine；riscv 定宽试点有 QEMU 真执行
> 矩阵）。示例/夹具谱（`demo`、`demo8`、`demo_inst{8,12,100}`）**不在库里**，见
> `crates/backend/forge-codegen/tests/isa/README.md`。

### Frontend Pipeline (forge-grammar v21)

> **v26 M2 起：这一线只服务示例**。`forge-grammar` 仍是工作区成员（它自带完整测试），
> 但它的唯一下游 `forge-hir` 与示例 `mini_c` 已移到 `examples/hir-demo/` 并脱离工作区
> ⇒ **不进门禁、会静默失效**（见该目录 README 与依赖图下方注释）。

```text
Grammar (.lx)  ──►  Lexer + CST Parser  ──►  AST Lowering (schema-driven)  ──►  Semantic Analysis
     (runtime)        (existing, unchanged)         (NEW: AstSchema)                (NEW: SymbolTable, etc.)
```

Key types for consuming the AST:

- `AstSchema` — runtime definition of AST node shapes (mirrors `Grammar` pattern)
- `TypedAst` / `AstRef` — type-safe AST access (replaces raw `CstNode` walking)
- `AstVisitor` / `AstTransform` — read-only traversal and bottom-up rewriting
- `SymbolTable` / `NameResolver` — semantic analysis passes
- `Parser::parse_to_ast()` — one-step lex→parse→lower

Example: migrating from CST to AST:

```rust
// OLD: CST manual walking
let name = cst.flatten_child("IDENT").ok_or(...)?.text().to_string();

// NEW: Schema-driven AST
let schema = ir_schema(); // lazy_static
let ast = parser.parse_to_ast(source, &schema)?;
let name = node.get_text("name")?;
```

## HIR Lowering (forge-hir)

> **v26 M2 起：`forge-hir` / `forge-hir-macro` 是示例（`examples/hir-demo/hir{,-macro}/`），
> 不是产品线**——它们已脱离工作区，本节描述的是那份示例的内部约定（改动前请先把它移回
> `crates/` 或给它一条真实产品路径）。

`forge-hir` 是手写 AST→IR 降级层（HIR = 高层 IR 图，lowering 后进 forge-ir）：

1. **op 目录**：`define_lowering!`（forge-hir-macro）只声明 atom → 后端 Opcode
   映射。每个 atom 生成 `xxx_tag()`、类型安全 `build_xxx(&mut IrGraph, 属性先,
   操作数后) -> Result<…, HirError>`（仿 Cranelift InstBuilder）、`register_atoms()`。
   `rule`/`struct` 关键字已移除——AST 降级必须手写。
2. **手写 lowering**：所有降级函数统一签名 `fn(…, ctx: &mut HirCtx, node: AstRef)
   -> Result<…, HirError>`。`HirCtx` 是唯一可变借用对象（graph + syms + loops +
   next_offset + return_slot/return_block），禁止字段拆分/闭包适配器。
3. **入口**：`lower_into_module(&graph, &registry, module, name, sig)` 一步把
   IrGraph 降级为 forge-ir Function（entry 块参数会映射为函数实参）。
4. 示例：`examples/mini_c/src/codegen_hir.rs`（与 `codegen.rs` 双后端并存，
   `compiler::Backend::{Direct, Hir}` 切换，`tests/dual_backend_tests.rs` 回归对比）。

语义约定：`alloc_slot` 发真正的 `mem.stack_addr` 节点；`load` 结果类型取自
`ty` 属性；`AttrValue::BlockId` 存 IrGraph 内部块句柄（lowering 经 block_map
转换，勿直接 cast 成 forge_ir::Block）。

## Testing Notes

**内容已搬到 [`docs/guides/testing-notes.md`](docs/guides/testing-notes.md)**（约 300 行：
JIT 矩阵 / 汇编语料三档 / 生成物落盘 / 谱内向量 / ISA-DSL 工具链 lint 档 / 宽度元数据 /
搬运族派生 / 短名折叠 / forge-rustc e2e 开关 …——按主题查那里）。

留在本文件的**四条最容易踩的**（细节与实测证据见上文链接）：

- **黄金/快照必须先归一化换行再比**：git 在 Windows 按 CRLF 检出签入的 LF 文件 ⇒ 只比
  `lines()` 看不见差异、直接比字符串又会红（差异点报在文件末尾 + 期望"缺行"）。比之前两边都折成 `\n`。
- **`os error 112`（磁盘满）**：`target/debug/incremental` 会涨到几十 GB（实测 44.7 GB）⇒
  删它，并给后续构建设 `CARGO_INCREMENTAL=0`（门禁"中途红、报错指着 query cache"的根因）。
- **门禁只有一条命令**：`cargo forge gate`（见上文 Build Commands）；改了守卫用 `--guards`。
- **两条 JIT 矩阵是两套能力集**：改类表/值池/ABI **必须都跑**（x86 全绿而 riscv 错值的先例）。

## SIMD 支持矩阵（x86_64，isa/x86.toml）

| 维度 | 支持 | 说明 |
| --- | --- | --- |
| 长度 | V64（2×f32）、V128（4×f32）、V256（8×f32，AVX）/（8×i32、4×i64，AVX2）、V512（16×f32/8×f64，EVEX；算术/常量/by-ref ABI/Load-Store 均已支持，>32B 需 AVX-512F） | 动态 `vector_ty(elem, len)` 与内建去重；`<3 x f32>` 等非 2 幂长度用 128 位指令低 lane 语义（未用 lane 无定义）；V256/V512 测试在无 AVX/AVX-512 机器自动 skip |
| 元素 | f32/f64/i32/i64 | vadd/vsub/vneg 全元素（f32→addps、f64→addpd、i32→paddd、i64→paddq/psubq；V256 整数走 AVX2 vpaddd/vpsubd/vpaddq/vpsubq）；vmul 浮点 + i32（PMULLD/VPMULLD）；i64 vmul/vdiv 与整数 vdiv 无 SIMD 指令 → 编译期 Unsupported；vabs 用按位掩码（andps 0x7FFFFFFF×4）对 f64/i64 亦正确 |
| 运算 | const_vector/const_vector_array/vadd/vsub/vmul/vdiv/vneg/vabs/vbitcast/vextract（全 lane）/vinsert/vbroadcast/vsplit/vconcat/shuffle_vector | `const_vector<T: Vector>(Vec<T>)` 泛型值语义（动态数组，ty 由 T+长度推导）；`const_vector_array([T; N])` 静态数组；`const_vector_bytes(Vec<u8>, ty)` 底层字节 API（元素 LE 字节序，用户自定义 `Vector::lane_bytes` 即可接入）；shuffle_vector：V128 单 shufps、V256 拆半双 shufps（mask 组内语义）；vbroadcast 64 位元素用 vbroadcastsd。**vextract 的 V256 规则按 `rs1_width`（向量操作数）判定**——结果类型是标量，用 `rd` 会永不命中（2026-09-10 修复：V256 lane≥4 曾取到低半区值，`test_jit_v256_byref_high_lane` 守护） |
| 常量 | 向量常量走池的 `Vector` 通道（字节 + 端序，端序进去重键） | 构造器（v28 起统一为 `const_vector*` 家族）：`const_vector<T: Vector>(Vec<T>)`（ty 由 T+len 推导，默认 Little）、`const_vector_array([T; N])`、`const_vector_bytes(Vec<u8>, ty)`；**端序**：`Vector::lane_bytes(endian)`（Little→to_le、Big→to_be，u8..u128/f32/f64 全位宽，u128 16 字节不截断）、`const_vector_endian`/`const_vector_bytes_endian` 显式端序（大端框架数据）、`ConstantPool::vector_endian` 查询；DSL 按常量端序还原（LE→from_le、BE→from_be，32 位元素逐元素 BE 读）；**V512（64 字节）常量**按 4×128 位段各自装好后用 4 条 EVEX `VINSERTF32X4`（imm=0..3，覆盖全部 128 位 lane）拼成——占位符 `{vconst_lo_h2}`/`{vconst_hi_h2}`/`{vconst_lo_h3}`/`{vconst_hi_h3}`，生成级测试 `test_v512_vconst_generates_four_evex_inserts`（无需 AVX-512 硬件）守护，见 WORKAROUNDS WA-43；rodata 数据段加载为长期优化 |
| ABI | **≤16B（V64/V128）按值 XMM 全宽 + >16B（V256）by-ref/sret 全线支持**（2026-09 D3 补齐 VEC(16) 全宽） | Windows x64 无 YMM 参数寄存器——>8B 非标量按引用传指针（占 GPR 槽）、返回 >64 位走首参 RCX 隐藏 sret；≤16B 向量（VEC(16) 类）按值进 XMM{pos}（by-position 槽）：收参/实参/返回用**全宽 128 位 MOVAPS**（按值向量按**整寄存器槽**请求 128 位 → 取到全宽搬移那条（x86 = MOVAPS）；MOVSD/MOVSS 只移 8/4B 会静默截断高半，WA-37 D3 修复）。Load/Store 谓词加 `rd_vec`/`rs1_vec`（向量类型字节数）→ V128 走 MOVUPS_128（0F 10/11 无前缀 16 字节）、V64 走 movsd 8B。jit 12+ 测试绿（v128/v64 byval param/return、mixed、wide byref/sret、v512）；forge-rustc B3 门控已撤、simd_v128/v64/v256 e2e 全绿。**残余**：宽向量第 5+ GPR 槽显式 Unsupported（调用方 + 被调方 by-position/by-class 三处均 fail-closed；设计取舍，建议不做）。CallIndirect 宽参/返回**已测**（`test_jit_call_indirect_wide_vector_byref` / `test_jit_call_indirect_wide_vector_sret_return`，2026-09-12 逐条核查 WA-37 残留清单时确认）。IR 层 >16B 向量 Load/Store 已于 2026-09-12 支持（32B 恒可、>32B 需 AVX-512F，`rd_vec`/`rs1_vec` = 32/64 → `VMOVUPS_256_*`/`VMOVUPS_512_*` 的 **reg 基址**形式；ABI by-ref 仍走 MemRef 形式）。V512 被调方收参已按**参数 IR 字节数**分派 64B load（EVEX；生成级测试 `test_v512_byref_callee_load_is_64b` 守护，运行级 lane15 断言需 AVX-512F 硬件）。见 forge-rustc WORKAROUNDS.md WA-37 |
| 编码 | SSE（0F/0F38 前缀族）+ AVX（VEX C4 语义键）+ AVX2（VEX 族） | 生成器内联实现 ModRM/REX/VEX 发射（`dsl/codegen/mod.rs` 的 VlenCtx）；VEX 三操作数 r/m=src2、vvvv=~src1；无源指令 vvvv 编码 1111；vextractf128 的 dest 在 r/m、src 在 reg；vzeroupper 无需（Windows x64 ABI 允许破坏 YMM 高半） |

结构化谓词：属性表 = `dsl/pred.rs` 的 `PRED_ATTRS`（`rd`/`rs1_width`/
`rs2_width`/`rd_vec`/`rs1_vec`/`elem`/`cond`/`imm0`，与生成器 `__attr` 分派表
单点同步；写错属性名编译期报错——未知属性恒为假会让规则永不命中）+
`and/or/not/in/eq/ne/lt/le/gt/ge` 组合（纯 TOML 数据，无字符串）。
`imm0` = `current_immediates[0]`（Vextract/Vinsert lane 索引、AtomicRmw op
判别值 Xchg=0/Add=1/Sub=2）。

`op` 由**表头名**给出（`[[lower.Iadd]]` ⇒ op = `Iadd`）。同一序列要覆盖多条同形 op ⇒
**各写一块**：x86 就这样给 Copy/Uextend/Freeze/Ptrtoint/Inttoptr、Sitofp/Uitofp、Fptosi/Fptoui、
Undef/Poison 等 6 组去重（共 197 条 `[[lower.<op>]]` 声明）。**不加**规则的表形式与共享序列机制：
实测表格化的规则数收益 ≤5%、长序列跨 op 复用为 0，而 `vary` 已覆盖同一模板按行代入的形态。

`[[lower.<op>]]` 的两个消重/去序键：

- `vary = { attr = [...], name = [...] }`：各列表**等长**，按下标 zip 成行展开。
  键在 `PRED_ATTRS` 里 → 该行自动追加 `eq = [键, 值]` 到 `when`；否则是模板里
  `{键}` 的纯替换变量。x86 `Fcmp` 32 条 → 8 条、`Vadd`/`Vsub` 各 8 → 3。
- 规则**不依赖声明序**：裁决序 = (`priority` 降, 谓词叶子数降, 声明序升)，
  见 `IsaModel::lowering_by_op`。被前序规则完全覆盖的规则 → 编译期报"死规则"
  （判定域 = 每属性闭区间集合；含 `or`/`not` 记为 Opaque 跳过）。`priority`
  只在"故意让更宽的规则赢"时用（x86 `Vextract` 的 lane 0 快路径）。

编码键（v15-S3，v21 W10 现名）：`[form.<名字>]` 与 `[inst.<名字>]` **共用同一组语义键**
（`EncKeys`：modrm/modrm_fixed/rex/vex/evex/prefix/opsize/rex_w/opcode_reg/imm/
escape）。form 退化为**可选的预设混入**，指令可逐键
覆盖（`EncKeys::over`，指令优先），`form` 本身可省略——组合不再需要预先命名。
x86 forms 47 → 19，37 条指令直接内联编码键（不再有 `MRR_0F_NOOS_MEM` 与
`MRR_MEM_0F_NOOS` 这种把 4 个事实编进名字、且只差 `rex_w` 的并存命名）。
`opsize` 固定宽度写**位宽整数**（`opsize = 64`）——v14 的 `"r8"` 是字节单位、
与 `[conventions]` 的位单位矛盾，旧写法现在报错并给出迁移提示；
**位置的 `"s<N>"` 已删除**（v21 W4/D4 #14）⇒ 改用 `"out"`（唯一 out/inout 角色的 Reg 操作数）、
操作数**名字**，或共享 form 上的 `"auto"`（第一个 Reg 操作数驱动）；`"max"`（全部 Reg
操作数的最大宽度）不变。

命名操作数（v22：**声明就地写在 `asm` 里**）：`asm = "add {dst:r:out}, {src:r}"`
（`{@字段:槽[:角色]}` 额外绑定字段、`@[字段]` = 内存形态、`#序号` 钉编码序）。
更早的 `ops` 数组 + `bind` 键**已删除**（写了报未知键）。在现行写法里，
**声明名同时就是生成的 `Inst` 字段名**（声明 `dst` ⇒ `Inst::Iadd { dst, src }`；
位域名/语义角色名只是内部编码键）。`opsize` 因此能写 `opsize = "dst"`（自解释），
取代 `"s1"` 这种"读者无法判断指哪个"的位置引用（v14 那个把 64 位指针截成 32 位的
bug 就出在 `s0` 恰好是**源**）。v14 的 asm 内联声明 `{i:[槽:角色]}`、v21 的占位修饰
（`{n?}` / `{n:affix}`）**都已删除**，无兼容层。`collect_inst_infos` 把命名占位符规范化成
`{序号}` 后交给下游，生成器（asm/machine/decode）只认索引、不感知命名。

`modrm` 改显式映射（v15-S3d）：`modrm = { reg = "src", rm = "dst" }`——哪个命名
操作数进 `reg` 字段、哪个进 `rm` 字段直接写出来。`reg = <整数>` = 固定扩展码
（取代 `"ext"` + `fields.ext` 两处声明）；`rm = "[名]"` = 内存形式（mod≠11，与
asm 里 `[{base}]` 同形），两种内存风味由 `rm` 引用的槽 kind 区分（`mem` 槽带
base/disp/index/scale、`reg` 槽仅 `[base]`）。取代 v14 的六个魔法串
（`rr`/`rr_rev`/`rr_src2`/`ext`/`rm_mem`/`rm_memref`）——它们是**位置隐含**的：
同一个 `"rr"` 在 ADD_RM_R 里 reg=源、在 MOV_R_RM 里 reg=目的，读者必须回查生成器
才知道。生成器侧编码与解码原先各有一张 `(Kind, 操作数序号) → 字段` 位置表，现在
统一收敛成三条索引规则（`i == reg` / `i == rm` / 其余 → VEX.vvvv）。
`modrm` 因此从 form 下移到指令/家族（form 只留非位置性的键）。

## forge-rustc（rustc 代码生成后端）现状速查

`crates/tools/forge-rustc` 是**用 code-forge 当 rustc 后端的 dylib**（`-Zcodegen-backend=…`）。
现状与边界**以代码与 e2e 为准**，方案见
[`docs/archive/forge-rustc-v27-plan.md`](docs/archive/forge-rustc-v27-plan.md)（2026-10-31 收口归档）：

### 分层红线（2026-10-22 明确，改降级层前必读）

**降级层不得写死"这台机器上才成立"的东西**——具体三类，都踩过：

1. **运行时符号**：不要在 lowering 里直接 `intern("__udivti3")` 之类的 libcall 名字
   （MSVC 有 `__udivti3`／没有 `__sdivti3`；别的目标连名字都不同）；
2. **寄存器/向量类型**：不要假定存在 16 字节向量寄存器（曾经用"返回类型声明成 `TypeId::V128`
   借 `xmm0`"来实现 128 位除法——ISA 没有 V128 就全崩）；
3. **原生位宽**：不要假定 64 位肢（`wide_split` 的 `w` 来自 `machine.int_widths()`，
   8 位 ISA 上是 8）。

⇒ 这类"语义补全"（宽值除法、溢出、位宽提升……）**必须落在 IR/legalize 层**，用
`Iadd/Isub/Ishl/Ushr/Sshr/Icmp/Select` 这类 ISA 无关的 IR 表达，并对**任意原生位宽**成立。
降级层只负责"把 MIR 映射成 IR"，不做寄存器/符号层面的猜测。

- **能力面**：标量/控制流/调用/Windows x64 约定/FFI/聚合/枚举（含 niche）/alloc 运行时/
  static/Box/闭包/Drop/`dyn` 虚调用/DWARF5（gdb 实证）/多 CGU 并行 + 每 CGU 独立对象 +
  `-C incremental`；v27 新增 **`global_asm!`**（含 `const`/`sym`）。
- **e2e**：`cargo test -p forge-rustc --test e2e -- --test-threads=1`（真执行；
  `known_failure` = 0）。**用例级并行已默认开启**（每个 worker 独立编译+运行，产物按用例名
  分子目录隔离；结果按 `CASES` 顺序回收后再分类，输出与串行一致）——并发度由
  `FORGE_E2E_JOBS` 覆盖（默认 = 可用核数，`1` = 串行）。**用例数见本 crate README 的机读计数标记**——由守卫
  `rustc_doc_counts` 与 `tests/e2e.rs` 双向钉住（改用例数必须同步它；本文不写具体数字，
  免得再漂）。
- **能力缺口**：权威清单 = 台账 `docs/reference/backend-gaps.md`（每条一个 `[cap:ID]`，
  由 `capability_gaps` 守卫与源码**双向相等**钉住，别在这里抄一遍）。几条重要的：
  `INLINE_ASM`（待做：S1 施工图见归档账本，需把汇编器与字符串表穿进管线）、`UNWIND`
  （不做，缺 SEH 表）、`TLS`（**已实现**：动态 TLS，`thread_local_roundtrip` 守护）、
  `MIR_TERMINATOR`（不做，兜底必须明确拒绝）、`CODEGEN_OPTS`（不做：`-C lto`/覆盖率/
  sanitizer/PGO/CFG 防护等**靠 LLVM 管线**的选项统一**明确拒绝**——静默忽略等于安全特性没开）。
- **绕法登记**：`crates/tools/forge-rustc/WORKAROUNDS.md`（`[WA-NN]`，机读；由
  `workarounds_guard` 钉住编号唯一与引用可解析）。**注意：该表的"根治方向"列是写那一行时的
  判断，不是现状**——v27 立项时据此误判了四项，实际早已关闭（见方案 §8 的教训）。
- **门禁**：`cargo forge gate --subset rustc` = 只跑后端（fmt + e2e + 三个后端守卫 + clippy + 文档）。
- **真 `std` 已跑通（2026-10-18）**：`examples/forge-rustc-std-hello/` 的
  `run-offline.ps1` 一条命令构建并运行 `println!`/`String`/`Vec`/`env::args`/`HashMap`
  五个用例（`std` 由 LLVM 编、我们的 crate 由 forge 编）。沙箱里的关键是
  **`CARGO_HOME` 指到工作区**（默认 `~/.cargo` 解包会 `os error 5`），与网络无关。

## Code Conventions

- Edition 2024 throughout
- `forge-ir` 的类型经 `forge-isa-runtime::ir` 暴露给生成物（`forge_ir::` 的固定改写目标；
  `forge-codegen::ir` 只是同源 re-export）
- 生成物路径**恒为** `forge_isa_runtime::…`（v19 V1b 起没有 `krate`）：任意 crate 只要
  依赖它 + build script 预生成就能承载谱（最小例子 `examples/isa-host-demo`）
- Root `src/lib.rs` is a thin facade — all real code in `crates/`
- **改代码默认用 `edit`/`write` 的精确文本替换，脚本只做机械操作**——完整规则（允许/禁止清单、
  用脚本的四条硬要求、本仓踩过的实例）见上文「编辑纪律（改代码怎么动手）」
