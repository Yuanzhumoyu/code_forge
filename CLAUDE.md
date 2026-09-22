# CLAUDE.md

## Documentation Map（文档地图）

仓库文档按**职能**分目录存放（2026-09 重组）。每篇状态标记：
`[active]` 现行维护 · `[progress]` 进行中 · `[archive]` 历史归档（头部 ARCHIVED，
仅供参考、代码为准）。**先读 `docs/README.md`**（完整索引），再按主题进子目录。

```text
docs/
├── README.md              # 文档总索引 + 状态图例
├── reference/             # 现行规范/设计参考 [active]
│   ├── isa-dsl.md             # ISA-DSL v18 唯一语法规范（改 isa/*.toml 先看它）
│   ├── isa-dsl-errors.md      # ISA-DSL 错误码目录（报错看不懂先看它）
│   ├── aarch64-encoding-ref.md# A64 编码参考（arm64_v12 后端/golden 依据）
│   └── imm_str.md             # ImmStr 类型设计（forge-ir 代码注释引用）
├── forge-ir/              # forge-ir 工作流 [active]
│   ├── README.md              # 组说明（历轮计划已归档、如何继续）
│   └── backlog.md             # 未关闭待办速览（出处指向 archive/forge-ir/）
├── plans/                 # 有未完成工作的专项方案 [progress]
│   └── forge-rustc-vec_push-plan.md # vec 族（5 用例 FLAKY，见 e2e.rs）
├── performance/           # 基准与优化
│   ├── BENCHMARKS.md          # 基准运行框架
│   ├── OPTIMIZATION.md        # 优化清单
│   ├── bench_baseline.md      # 基线 + 历轮实测
│   └── codegen_stage_profile.md # codegen stage 占比
├── guides/                # 工具与方法
│   ├── isa-dsl-tutorial.md    # ISA-DSL 30 分钟接入教程（新 ISA 从这里开始）
│   ├── coverage.md            # cargo-llvm-cov 覆盖率工作流
│   └── lint.md                # markdownlint 检查命令 + 存量基线（写文档后自查）
└── archive/               # 历史归档（⚠️ 内容以其记录时点为准）
    ├── README.md              # 归档图例与清单
    ├── roadmap-status.md / isa-dsl-v12-roadmap.md / asm-dec-generic-design-v2.md
    ├── isa-dsl-v12-v17.md     # ISA-DSL v12–v17 语法史 + v18 删除/改名总表
    ├── ymm-abi-plan.md        # YMM ABI 方案（2026-09-10 核查完成 → 归档）
    ├── hir-shrink-plan.md     # HIR/mini_c 收缩（同日均告终结 → 归档）
    ├── clippy-fixes.md / coverage-history.md
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
- ISA-DSL 现行语法是 **v18**（`reference/isa-dsl.md`）；v12–v17 的语法与删除总表在
  `archive/isa-dsl-v12-v17.md`，v12 路线图在 `archive/isa-dsl-v12-roadmap.md`。
  注意区分三种"版本号"：DSL 语法版本（v18，靠文档/CHANGELOG 追踪，**不写进 TOML**）、
  `[meta].version`（ISA 自己的自由字符串）、以及 `isa/*_v12.toml`/`src/v12/` 这类**历史命名**
  （生成器与谱的文件名，稳定不动）。

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

## Build Commands

```bash
# Check all crates (requires nightly)
cargo check --workspace

# Run all tests (exclude forge-rustc which needs special rustc sysroot)
cargo test --workspace --exclude forge-rustc

# Run tests with all features
cargo test --workspace --exclude forge-rustc --all-features

# Run specific crate tests
cargo test -p forge-ir
cargo test -p forge-opt
cargo test -p forge-codegen
cargo test -p forge-grammar

# Check forge-rustc (requires nightly + rustc dev components)
cargo check -p forge-rustc
```

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
├── forge-mem         (no internal deps)
├── forge-opt         → forge-ir
├── forge-codegen     → forge-ir, forge-opt, forge-mem, forge-dsl
├── forge-dsl         (proc-macro 薄层：解析 isa_from_file! 参数 → forge-isa-dsl)
├── forge-isa-dsl     (普通 lib：ISA-DSL 编译器本体——模型/解析/校验/诊断/代码生成)
├── forge-grammar     (no internal deps)
├── forge-hir-macro   (no internal deps)
├── forge-hir         → forge-ir, forge-grammar, forge-hir-macro
├── forge-object      → forge-ir, forge-codegen
├── forge-plugin      → forge-codegen
├── forge-rustc       → code-forge (强制 object-file/plugins features)
└── forge-tests       → code-forge, forge-rustc(optional, nightly feature)
```

### Key Architecture Rules

1. **`isa_from_file!`（现行 ISA-DSL v18 语法）默认生成 `crate::` 路径** — 即"生成在哪个
   crate 里就属于哪个 crate"（`crate::prelude::*`、`crate::machine::*` 在该 crate 内解析）。
   生成模块名 = 文件 stem（可用 `name = "…"` 覆盖）。**在别的 crate（含 `tests/`）里生成**时给
   第二个参数 `krate = <宿主路径>`：生成物里的 `crate::…` 改写为 `<宿主>::…`、`forge_ir::…`
   改写为 `<宿主>::ir::…`（forge-codegen 提供 `pub use forge_ir as ir;`），因此
   生成代码只依赖宿主的**公开面**——demo 夹具正是这样住在
   `tests/isa/*.toml` + `tests/common/mod.rs`（`krate = forge_codegen`）而不进库本体。
   库本体只有真实后端：`arch/{x86,arm64,riscv64}_v12.rs`（文件名里的 v12 是历史命名）。

2. **Assembler/JIT coupling** — `Assembler` trait ↔ `JitCompiler` are circularly coupled.
   Both live in forge-codegen. Cannot split into separate crates without first refactoring
   to remove the cycle.

3. **Proc-macro limitation** — `forge-dsl` 是 proc-macro crate；Rust 禁止它导出非宏项。
   因此**编译器本体在 `forge-isa-dsl`**（普通 lib：模型/解析/校验/诊断/代码生成 +
   `expand_file`/`validate_file` 入口），`forge-dsl` 只解析 `isa_from_file!` 参数并调它
   （v18 S7a）。同理 `MemRef` 等生成物要用的类型定义在 forge-codegen，不在 DSL crate。

4. **v12 自包含 asm** — v12 生成模块内联实现 assemble（表驱动，首词=mnemonic），不再经
   lalrpop 语法与 forge-asm 运行时（v11 时代已随语法层删除）。`TargetAssembler` trait
   （`crate::machine::assembler`）仅要求 `parse_insts`。

### ISA Backend Pattern

```rust
// 发行后端：crates/backend/forge-codegen/src/arch/my_isa.rs
forge_dsl::isa_from_file!("isa/my_isa.toml");
pub use self::my_isa::*; // 生成 TargetMachine / Inst / Reg 等全套组件
// 注册由 DSL 生成的 ensure_registered() 完成（OnceLock 注册 Registry + reloc patcher），
// 无需手写——见 arch/x86_v12.rs 的实际形态。

// 测试夹具（不发行）：crates/backend/forge-codegen/tests/common/mod.rs
forge_dsl::isa_from_file!("tests/isa/demo_v12.toml", krate = forge_codegen);
```

> 注：生成模块导出的是 `TargetMachine`（组合 IsaInfo/RegInfo/ABI/Lowering/Encoder/
> FrameLowering/Disassembler/Assembler/**Decoder**），**没有 `Isa` 类型**；
> 无 `register_backend!` 宏。v11 后端（x86_64/aarch64/riscv64/wasm32/minimal_sd）
> 已随 v11 语法层删除——现为 `arch/x86_v12.rs`、`arch/arm64_v12.rs`、
> `arch/riscv64_v12.rs`（三者均已接 TargetMachine；riscv 定宽试点有 QEMU 真执行
> 矩阵）。示例/夹具谱（`demo_v12`、`demo8_v12`、`demo_inst{8,12,100}_v12`）**不在库里**，见
> `crates/backend/forge-codegen/tests/isa/README.md`。

### Frontend Pipeline (forge-grammar v21)

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

- Integration tests in `tests/` import ISA types from `code_forge::backend::<isa_module>::*`
- `forge-rustc` tests require nightly Rust with `rustc-dev` component
- **JIT 集成矩阵**（`forge-tests/src/jit_matrix.rs`）：架构无关、一次编写——
  用例**零 ISA 引用**，ISA 只存在于薄 runner（`isa/<name>/` 绑定机器 +
  能力集 `Capabilities`，riscv64 未来接入复用）。当前 **x86_v12 195 passed /
  3 skipped**（本机 2026-09-13 实测 `cargo test -p forge-tests --lib
  jit_matrix_x86_v12`；旧记录的 193 已过时）、**riscv64_v12 131 passed /
  67 skipped，0 failed**（QEMU 通道；2026-09-13 本机实测
  `cargo test -p forge-tests --lib jit_matrix_riscv64_v12 -- --test-threads=1
  --nocapture`，**需 `--nocapture` 才看得到计数**）：整数/浮点/调用/
  向量/饱和/指针转换/undef/poison/GlobalAddr/原子（AtomicRmw/Cmpxchg）/GEP/Nop/
  混宽整数算术与比较；V256 用例在无 AVX 机器
  自动 Skip（**哪些用例 Skip 及原因**经 `FORGE_JIT_EVENTS` 的
  `MATRIX-SKIP`/`MATRIX-FAIL`/`MATRIX-SUMMARY` 事件可核对——libtest 吞掉通过
  测试的输出，否则 CI 上不可见）。`CaseKind::{I32/I64/F64/Bool/Block/Args/
  F64Args/Module/CompileOnly}`；`ops` 未覆盖 → Skip（不失败，实现后自动转绿）。
  **两条矩阵是两套能力集，改类表/值池/ABI 必须都跑**（2026-09-13 实测：x86 全绿
  而 riscv 的 7 个 fcmp 错值，正是 riscv 通道抓到的）。
  测试入口：`cargo test -p forge-tests jit_matrix_x86_v12`。
- **rust-analyzer 假阳性**：`isa_from_file!` 调用行上的 `expected expression` /
  `expected R_PAREN`（成对，riscv64 15 / arm64 4 / 夹具 9、6，**x86 0**）是 **RA 侧
  展开管线的问题**——生成物用 RA 自己的解析器解析零 `ERROR` 节点，`rustc`/`clippy`/
  门禁全绿。二分已收敛到"`encode` 与至少一个别的部件同时生成"且只在定宽/混合字长 ISA
  上出现；**别再从头排查**，方法与全部证据见
  [`docs/guides/rust-analyzer-notes.md`](docs/guides/rust-analyzer-notes.md)
  （含"字符串续行被判 `Invalid escape`"这一类已修项）。一切以 rustc 与门禁为准。
- **ISA-DSL 工具链**：`cargo run -p forge-isa -- validate|insts|explain|diff|schema|fmt <谱.toml>`——
  不接后端就能校验（全部诊断 + 行:列）、看**展开后**的指令与生效编码键、查单条指令的
  模板 provenance（哪个模板哪一行）、两份谱的规格 diff（迁移前后对照）、打印/写出 JSON
  Schema、把多文件谱 `fmt` 折叠成单文件（v18 S7d）。`--json` 机读；退出码 0/1/2。
  实现 = `forge-isa-dsl::report` + `::schema` 投影层 + CLI 薄层，复用
  `collect_inst_infos` 的"form 预设 ⊕ 指令覆盖"判定（不重复实现）。
  **三方针守卫必须改三处一起改**：schema 表（`src/schema.rs`）↔ `v12/model.rs` 结构体字段
  ↔ `docs/reference/isa-dsl.md` 的「键总览（速查表）」区段 + 签入的 `isa-dsl.schema.json`
  （`cargo test -p forge-isa-dsl --test schema_guard` 全钉住；重新生成 schema 用
  `cargo run -p forge-isa -- schema --out isa-dsl.schema.json`）。
  三条要点：① schema 里的键必须是**用户在 TOML 里实际写的键**——字段名与 TOML 键不同时以
  `#[serde(rename = "…")]` 为准（`reference`/`ref` 曾因此让编辑器把 `isa/x86_v12.toml` 的
  35 处 `ref` 全部标红）；② `schema_guard.rs::shipped_specs_only_use_schema_keys` 直接拿
  `isa/*.toml` + 夹具当输入，任何"schema 与真实谱不符"都会红；③ 自由表（`fields = {…}`、
  `[[templates]].body`、`when = {…}`）在 schema 里没有子约束，守卫也**不**下钻它们。
- **多文件谱（v18 S7d）**：`include = ["…"]` + `[[override]]` 由 `forge-isa-dsl::loader`
  消费（按块合并；数组节按 include 序追加、重复 `[表头]` 视为续写、同名标量冲突报错）；
  它是所有公开入口（`isa_from_file!`/`expand_file`/`validate_file`/CLI）的必经之路，
  诊断按来源文件映射，生成物对**每个来源**登记 `include_bytes!`。`include`/`[[override]]`
  是组合键（合并后不存在），裸文本入口带它们会明确报错。
- **生成字段名 = `ops` 声明的名字（v18 S7d 修正）**：`ops = ["dst:r:out", "src:r"]`
  ⇒ `Inst::Iadd { dst, src }`；`[forms].operand_fields` 的名字（`rd`/`rs1`）与变长 ISA 的
  语义名（`dest`/`cond`）**只是内部编码键**（查 `[conventions.bitfields]`/modrm 角色/立即数
  表），不得再泄漏成字段名。关键字 → `r#type`、数字开头 → `_8bit`（最小归一，不做语义改名）；
  守卫 `crates/frontend/forge-isa-dsl/tests/field_names.rs`。
- **TOML 改动**：改 `isa/*.toml` 直接触发重编译——生成模块内嵌
  `include_bytes!(<TOML 绝对路径>)`，rustc 据此登记编译依赖（不再需要手动 touch
  `arch/<isa>.rs`）。`FGE_DEBUG_GEN=1` 可 dump 生成代码到 `%TEMP%\forge_gen_*.rs`。
- **生成期自测（v18 S6）**：`isa_from_file!` 默认在生成模块里带
  `#[cfg(test)] mod __spec_tests`（每指令一条规格用例：`encode∘decode`/`decode∘encode`
  字节稳定、`decode_partial` 一致、解码字段值原样、`disassemble→assemble` 文本幂等、
  立即数边界「min/max 可编码 + min−1/max+1 必报错」；覆盖维度 = 宽度视图 × 高编号
  寄存器视图）。用例随 TOML 自动更新，**不要手抄这类样板测试**；"字节对不对"仍由
  `docs/reference/aarch64-encoding-ref.md` 一类参考文档 + 各 ISA 黄金值测试守。
  同一份谱要被多个测试二进制包含时（`tests/common/mod.rs`）用
  `spec_tests = false` 关掉，另开一个用例二进制打开（见 `tests/spec_tests_v12.rs`）。
  覆盖守卫 = `crates/backend/forge-codegen/src/spec_coverage_guard.rs`（钉死指令总数
  x86 197 / riscv64 116 / arm64 104、零跳过、文本歧义名单；**它是 `#[cfg(test)]` 项，
  必须放在 `lib.rs` 末尾**——写死宽度守卫按第一个 `#[cfg(test)]` 截断扫描）。
- **生成物形状表（v18 S8a）**：`impl MachineInst for Inst` 的 8 个查询方法
  （`uses`/`defs`/`use_constraints`/`def_constraints`/`effects`/`reg_field`/`set_reg_field`/
  `is_reg_field_settable`）在生成物里**不许再逐指令展开**——它们读每变体一行的
  `__SHAPES`（+ `__SLOT_CLASSES`/`__EFFECT_SETS` 两张去重表）与 3 个访问器
  （`__shape`/`__reg_slot`/`__set_reg_slot`，见 `v12/codegen/machine.rs`）。
  守卫 = `crates/frontend/forge-isa-dsl/tests/machine_shape_table.rs`（钉"8 个方法体零
  `Inst::` 臂" + "形状表与两个访问器覆盖同一批变体、行数 = 变体数"）；实测数字见
  `docs/performance/bench_baseline.md` 的「S8a 落地度量」（`tm` −12~26%，整模块 −7.5~12.9%）。
- **角色声明带宽度（v18 S9）**：`[[instructions]].roles` 的条目有两种写法——`"gpr_mov"`
  （无宽度语义，全 ISA 唯一）与 `{ role = "fpr_mov", bits = 32 }`（**有宽度语义**：同角色
  可多条，按 (角色, 位宽) 唯一，单位是**位**）。生成器按宽度查：`role_name_for(role, bits)`
  / `inst_by_role_for(…)`，选不到就明确 `Unsupported`（消息带请求位宽 + 已声明的位宽集合）；
  同角色同宽度重复、或同一角色一处写 `bits` 一处不写 ⇒ `validate` 编译期报错。
  **不要把位宽编进角色名**（S9 之前是 `fpr_mov_f32`/`wide_vec_load_64` 这类，别的位宽的
  ISA 无法接入；`MOVSS`/`MOVSD` 共用 `fpr` 槽，槽也分不出 32/64——宽度必须在声明里）。
  守卫 `crates/frontend/forge-isa-dsl/tests/role_widths.rs`（真实谱变异：16/24 位合法、
  同 (角色,位宽) 冲突必报）；角色表见 `docs/reference/isa-dsl.md`。
- **谓词属性：按需 + 无名字分派（v18 S8b-1 / S8d）**：`gen_lowering_attrs` 生成的属性源
  在生成物里**只发射一次**（放在 `lower_inst` 的 `match op` 之前；跟着 op 臂走 = 每个
  op 重复一份 2.5 KB，x86 曾 100 份 = 243 KB），并且是
  `__AC<'x>`（`done` 位图 + `v: [Option<i64>; 9]` + `op`/`args`/`results` 共享借用）
  加每个核心属性一个 `#[inline] fn <名>(&mut self, ctx: &LowerCtx) -> Option<i64>`
  （**没算过才算**，因此"每次 `lower_inst` 每属性最多算一次"仍然成立，但**不需要的属性
  一次都不算**——旧实现每次都把 9 个全算一遍）。谓词名在**生成期**解析成
  `__ac . <属性> (&*ctx)`（见 `compile_pred_guard`/`attr_expr`），**不得**再引入
  `__attr(name)` 之类的运行时字符串分派；`[[derive]]` 派生属性也在生成期展开。
  另外：不用 `{cc}` 的降低规则**不发射** `let __cc: u8 = 0;`（死代码，S8d 修）。
  守卫 = `crates/frontend/forge-isa-dsl/tests/lowering_attrs_once.rs`。剩余的大头是
  每条规则各自的发射序列（x86 C 段 334 KB），表化需通用解释器，度量后判定不做。
- **生成物短名折叠（v18 S8c）**：生成器最后一道 token 后处理
  （`forge-isa-dsl/src/lib.rs::fold_short_forms`，跑在 `rewrite_path_roots` **之前**）
  把三种长形折成短名：`Reg::from_index(…)` / `<Reg as forge_ir::PhysReg>::from_index(…)`
  → `__ph(…)`、`…::to_index(…)` → `__ti(…)`、`forge_ir::RegClass` → `__RC`。
  短名定义（`type __RC = …` + `fn __ph/__ti`）发射在模块开头、折叠**之后**，因此自己走
  正常路径改写——**在生成器里新写这三处长形会被 `tests/short_form_fold.rs` 抓住**
  （生成主体里必须一处不剩）。实测三步累计：x86 −29.6% / riscv −35.8% / arm64 −22.1%。
- **宽度元数据（去「宽度写死」）**：寄存器类/宽度/栈槽/栈参数布局/指令字宽一律由
  TOML 派生（`[meta]`：`default_gpr_width`/`default_fpr_width`/`addr_width`/
  `value_gpr_width`/`value_fpr_width`/`vector_tiers`；`[encoding]`：**宽度三态**
  `kind = "fixed"|"mixed"|"prefix_scan"` + `bits`/`widths`/`max_len`/`default_opsize`
  （v18 S4，**任意 ≥ 1 位，无白名单/上限**，逐指令 `width` 可覆盖 `bits`）；
  `[stack]`：`slot`/`align`/`fp_save`；
  `[abi.stack_args]`：`callee_base`/`caller_base`/`first_offset_slots`/`stride_slots`/
  `shadow_bytes`；优先级 显式键 > 派生 > **报错**）。定宽指令字在生成代码里是
  **字节数组**（`[u8; ceil(位/8)]` + `__place`/`__bits`），位域可落在机器字之外
  （100 位字夹具 = 13 字节）；唯一边界是**单个位域 ≤ 64 位**（值承载在 u64/i64）。
  生成期用 `__DEFAULT_GPR_CLASS`/`__ADDR_CLASS`/`__SLOT_BYTES` 等常量，宿主用
  `TargetRegInfo::{addr_class, value_gpr_class,
  value_fpr_class, slot_bytes, vector_tiers, class_for_type}`——**不要**再写
  `RegClass::GPR64`/8 字节缺省。1 字节寄存器 ISA 夹具 =
  `crates/backend/forge-codegen/tests/isa/demo8_v12.toml`；指令字宽夹具 =
  `tests/isa/demo_inst{8,12,100}_v12.toml`（由
  `tests/common/mod.rs` 用 `isa_from_file!(…, krate = forge_codegen)` 宿住，
  **不进库本体**；用例在 `tests/demo8_v12_tests.rs`）；反回潮守卫 =
  `crates/{frontend/forge-dsl,backend/forge-codegen}/tests/no_hardcoded_widths.rs`
  （白名单带理由，且条目必须被命中）+ `tests/library_surface.rs`（demo 谱不得
  回到 `src/` 或仓库根 `isa/`）。规范细节见 `docs/reference/isa-dsl.md`
  的「宽度元数据」节。
- **forge-rustc e2e 环境开关**（`crates/tools/forge-rustc/tests/e2e.rs`）：
  `FORGE_E2E_ONLY=<case>` 只跑单用例、`FORGE_E2E_KEEP=1` 失败轮保留工作目录
  （证据在 `%TEMP%\forge_rustc_e2e_<pid>\`，**该目录随会话轮换被清理**，须当场复制）、
  `FORGE_E2E_TRACE=1` 打印 `FORGE_TRACE_*` stderr、`FORGE_E2E_NIGHTLY` 覆盖本机
  工具链（如 `nightly`）、`FORGE_E2E_TIMEOUT_SECS` 覆盖产物运行超时（默认 15 s）、
  `FORGE_E2E_STRICT_FLAKY=1` 让 FLAKY 用例**只容忍超时与 CI 环境 AV 签名（0xC0000005）、
  其余错码硬失败**（默认关闭，用于评估 5 例正确性是否已可进门禁，见计划 §9.5）、
  `FORGE_E2E_EVENTS=<路径>` 把关键事件（`RETRY`/`KNOWN-*`/`CI-ENV-AV`/`FAIL` +
  `SUMMARY passed=… known=…`）追加落盘——libtest 会捕获**通过**测试的 stdout，没有它
  就无法从 CI 日志判断当轮是否踩到被容忍的环境性事件（CI 用 `if: always()` 步骤打印）。
  超时后用**同一产物复跑一次**并打印 `RETRY <case>`——真挂起（两次都超时）仍上报，
  宿主侧起进程延迟造成的假败被吸收。
  FLAKY 用例取证脚本：`crates/tools/forge-rustc/tests/e2e_flake_repro.ps1`
  （`-Mode hammer` 5 轮全量 stage_a + 并行变体 + 5 用例各单跑 ×10；`-Mode load`
  N 并发 worker 直接跑 e2e 二进制制造负载直到复现）。判定标准见
  `docs/plans/forge-rustc-vec_push-plan.md` §9.1/§9.2（失败轮产物须与本机产物做
  行为/字节对照：一致 ⇒ 宿主环境性；错码 ⇒ 转 regalloc 关联法）。

## SIMD 支持矩阵（x86_64，isa/x86_v12.toml）

| 维度 | 支持 | 说明 |
| --- | --- | --- |
| 长度 | V64（2×f32）、V128（4×f32）、V256（8×f32，AVX）/（8×i32、4×i64，AVX2）、V512（16×f32/8×f64，EVEX；算术/常量/by-ref ABI/Load-Store 均已支持，>32B 需 AVX-512F） | 动态 `vector_ty(elem, len)` 与内建去重；`<3 x f32>` 等非 2 幂长度用 128 位指令低 lane 语义（未用 lane 无定义）；V256/V512 测试在无 AVX/AVX-512 机器自动 skip |
| 元素 | f32/f64/i32/i64 | vadd/vsub/vneg 全元素（f32→addps、f64→addpd、i32→paddd、i64→paddq/psubq；V256 整数走 AVX2 vpaddd/vpsubd/vpaddq/vpsubq）；vmul 浮点 + i32（PMULLD/VPMULLD）；i64 vmul/vdiv 与整数 vdiv 无 SIMD 指令 → 编译期 Unsupported；vabs 用按位掩码（andps 0x7FFFFFFF×4）对 f64/i64 亦正确 |
| 运算 | vconst/vconst_array/vadd/vsub/vmul/vdiv/vneg/vabs/vbitcast/vextract（全 lane）/vinsert/vbroadcast/vsplit/vconcat/shuffle_vector | `vconst<T: Vector>(Vec<T>)` 泛型值语义（动态数组，ty 由 T+长度推导）；`vconst_array([T; N])` 静态数组；`vconst_bytes(Vec<u8>, ty)` 底层字节 API（元素 LE 字节序，用户自定义 `Vector::lane_bytes` 即可接入）；shuffle_vector：V128 单 shufps、V256 拆半双 shufps（mask 组内语义）；vbroadcast 64 位元素用 vbroadcastsd。**vextract 的 V256 规则按 `rs1_width`（向量操作数）判定**——结果类型是标量，用 `rd` 会永不命中（2026-09-10 修复：V256 lane≥4 曾取到低半区值，`test_jit_v256_byref_high_lane` 守护） |
| 常量 | 扁平字节池（`Vec<u8>` + offset 表 + 每段端序 `vec_endian`） | `vconst<T: Vector>(Vec<T>)` 泛型值语义（ty 由 T+len 推导，默认 Little）、`vconst_array([T; N])` 静态数组、`vconst_bytes` 底层字节；**端序**：`Vector::lane_bytes(endian)`（Little→to_le、Big→to_be，u8..u128/f32/f64 全位宽，u128 16 字节不截断）、`vconst_with_endian`/`vconst_bytes_with_endian` 显式端序（大端框架数据）、`ConstantPool::get_vector_endian` 查询；DSL 按常量端序还原（LE→from_le、BE→from_be，32 位元素逐元素 BE 读）；**V512（64 字节）常量**按 4×128 位段各自装好后用 4 条 EVEX `VINSERTF32X4`（imm=0..3，覆盖全部 128 位 lane）拼成——占位符 `{vconst_lo_h2}`/`{vconst_hi_h2}`/`{vconst_lo_h3}`/`{vconst_hi_h3}`，生成级测试 `test_v512_vconst_generates_four_evex_inserts`（无需 AVX-512 硬件）守护，见 WORKAROUNDS WA-43；rodata 数据段加载为长期优化 |
| ABI | **≤16B（V64/V128）按值 XMM 全宽 + >16B（V256）by-ref/sret 全线支持**（2026-09 D3 补齐 VEC(16) 全宽） | Windows x64 无 YMM 参数寄存器——>8B 非标量按引用传指针（占 GPR 槽）、返回 >64 位走首参 RCX 隐藏 sret；≤16B 向量（VEC(16) 类）按值进 XMM{pos}（by-position 槽）：收参/实参/返回用**全宽 128 位 MOVAPS**（指令角色 `roles = ["vec_mov"]`（缺省 MOVAPS）——MOVSD/MOVSS 只移 8/4B 会静默截断高半，WA-37 D3 修复）。Load/Store 谓词加 `rd_vec`/`rs1_vec`（向量类型字节数）→ V128 走 MOVUPS_128（0F 10/11 无前缀 16 字节）、V64 走 movsd 8B。jit 12+ 测试绿（v128/v64 byval param/return、mixed、wide byref/sret、v512）；forge-rustc B3 门控已撤、simd_v128/v64/v256 e2e 全绿。**残余**：宽向量第 5+ GPR 槽显式 Unsupported（调用方 + 被调方 by-position/by-class 三处均 fail-closed；设计取舍，建议不做）。CallIndirect 宽参/返回**已测**（`test_jit_call_indirect_wide_vector_byref` / `test_jit_call_indirect_wide_vector_sret_return`，2026-09-12 逐条核查 WA-37 残留清单时确认）。IR 层 >16B 向量 Load/Store 已于 2026-09-12 支持（32B 恒可、>32B 需 AVX-512F，`rd_vec`/`rs1_vec` = 32/64 → `VMOVUPS_256_*`/`VMOVUPS_512_*` 的 **reg 基址**形式；ABI by-ref 仍走 MemRef 形式）。V512 被调方收参已按**参数 IR 字节数**分派 64B load（EVEX；生成级测试 `test_v512_byref_callee_load_is_64b` 守护，运行级 lane15 断言需 AVX-512F 硬件）。见 forge-rustc WORKAROUNDS.md WA-37 |
| 编码 | SSE（0F/0F38 前缀族）+ AVX（VEX C4 语义键）+ AVX2（VEX 族） | v12 生成器内联实现 ModRM/REX/VEX 发射（`v12/codegen/mod.rs` 的 VlenCtx）；VEX 三操作数 r/m=src2、vvvv=~src1；无源指令 vvvv 编码 1111；vextractf128 的 dest 在 r/m、src 在 reg；vzeroupper 无需（Windows x64 ABI 允许破坏 YMM 高半） |

v12 结构化谓词：属性表 = `v12/pred.rs` 的 `PRED_ATTRS`（`rd`/`rs1_width`/
`rs2_width`/`rd_vec`/`rs1_vec`/`elem`/`cond`/`imm0`，与生成器 `__attr` 分派表
单点同步；写错属性名编译期报错——未知属性恒为假会让规则永不命中）+
`and/or/not/in/eq/ne/lt/le/gt/ge` 组合（纯 TOML 数据，无字符串）。
`imm0` = `current_immediates[0]`（Vextract/Vinsert lane 索引、AtomicRmw op
判别值 Xchg=0/Add=1/Sub=2）。

`[[lowering]].op` **可以是名单**（v18 S5）：`op = ["Copy", "Uextend", "Freeze"]` = 这几条 op
的 lowering 完全一样，只维护一份序列（解析期展开成逐 op 规则，名单序 = 展开序，裁决结果不变）。
x86 用它把 Copy/Uextend/Freeze/Ptrtoint/Inttoptr、Sitofp/Uitofp、Fptosi/Fptoui、Undef/Poison
等 6 组去重（220 → 197 条声明）。**不加** `lowering.emit`/`[[sequences]]`：实测表格化的规则数
收益 ≤5%、长序列跨 op 复用为 0，而 `vary` 已覆盖同一模板按行代入的形态（详见方案 §7 S5 进度）。

`[[lowering]]` 的两个消重/去序键（v15-S2）：

- `vary = { attr = [...], name = [...] }`：各列表**等长**，按下标 zip 成行展开。
  键在 `PRED_ATTRS` 里 → 该行自动追加 `eq = [键, 值]` 到 `when`；否则是模板里
  `{键}` 的纯替换变量。x86 `Fcmp` 32 条 → 8 条、`Vadd`/`Vsub` 各 8 → 3。
- 规则**不依赖声明序**：裁决序 = (`priority` 降, 谓词叶子数降, 声明序升)，
  见 `V12Model::lowering_by_op`。被前序规则完全覆盖的规则 → 编译期报"死规则"
  （判定域 = 每属性闭区间集合；含 `or`/`not` 记为 Opaque 跳过）。`priority`
  只在"故意让更宽的规则赢"时用（x86 `Vextract` 的 lane 0 快路径）。

编码键（v15-S3）：`[[forms]]` 与 `[[instructions]]` **共用同一组语义键**
（`EncKeys`：modrm/modrm_fixed/rex/vex/evex/prefix/opsize/rex_w/opcode_reg/imm/
escape/opcode_field/operand_fields）。form 退化为**可选的预设混入**，指令可逐键
覆盖（`EncKeys::over`，指令优先），`form` 本身可省略——组合不再需要预先命名。
x86 forms 47 → 19，37 条指令直接内联编码键（不再有 `MRR_0F_NOOS_MEM` 与
`MRR_MEM_0F_NOOS` 这种把 4 个事实编进名字、且只差 `rex_w` 的并存命名）。
`opsize` 固定宽度写**位宽整数**（`opsize = 64`）——v14 的 `"r8"` 是字节单位、
与 `[conventions]` 的位单位矛盾，旧写法现在报错并给出迁移提示；
`"s<N>"`（取第 N 个操作数的宽度）与 `"max"`（取全部 Reg 操作数的最大宽度）不变。

命名操作数（v15-S3c）：`ops = ["src:gprx", "dst:gprx:inout"]` **数组序 = 编码序**
（modrm reg/rm、定宽位域绑定都按这个序；角色缺省 `in`），`asm` 只用 `{名字}`
**引用**——声明与打印彻底分离。**声明名同时就是生成的 `Inst` 字段名**（v18 S7d：
`ops = ["dst:r:out", "src:r"]` ⇒ `Inst::Iadd { dst, src }`；位域名/语义角色名只是
内部编码键）。`opsize` 因此能写 `opsize = "dst"`（自解释），
取代 `"s1"` 这种"读者无法判断指哪个"的位置引用（v14 那个把 64 位指针截成 32 位的
bug 就出在 `s0` 恰好是**源**）。v14 的 asm 内联声明 `{i:[槽:角色]}` **已删除**，
无兼容层。`collect_inst_infos` 把 `{名字}` 规范化成 `{序号}` 后交给下游，
生成器（asm/machine/decode）只认索引、不感知命名。

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

## Code Conventions

- Edition 2024 throughout
- `forge-ir` types are re-exported in `forge-codegen::prelude` for DSL-generated code；
  `forge-codegen::ir` 是 `forge_ir` 的 re-export（`krate = …` 生成物的 `forge_ir::` 目标）
- Generated code paths use `crate::` **when generated inside forge-codegen**（发行后端）；
  在别的 crate 里生成时用 `isa_from_file!(…, krate = <宿主>)`，生成物只落到该宿主的公开面
- Root `src/lib.rs` is a thin facade — all real code in `crates/`
