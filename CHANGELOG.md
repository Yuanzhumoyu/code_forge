# CHANGELOG

<!-- markdownlint-configure-file { "MD013": { "line_length": 512, "code_block_line_length": 512, "heading_line_length": 512 }, "MD024": false } -->
<!-- 文件级豁免原因：历史条目按单行记录（最长 ~440 列），且多年条目同置 [Unreleased] 下，
     版本子节用 `### Added (日期)`，同层 Fixed/Changed 标题按惯例重复——均为 CHANGELOG
     结构与单行惯例，不按正文 120 列规则折行；重构留待 changelog 整顿时处理。 -->

All notable changes to the `code-forge` project will be documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.0.0/).

## [Unreleased]

### Fixed (2026-09-21)

- **`#:schema` 编辑器补全：指令的 `ref` 键被 schema 写成了 `reference`（v18 S7e 修）**。模型的字段名是 `reference` + `#[serde(rename = "ref")]`，而 JSON Schema 发射器按字段名发射，于是**所有用 `ref` 的谱都被编辑器标红**（`isa/x86_v12.toml` 35 处，Taplo + `#:schema`）。
  现在 schema 发的是 TOML 里实际写的键 `ref`；三方守卫按 `#[serde(rename = "…")]` 取键名，并新增 `schema_guard.rs::shipped_specs_only_use_schema_keys`——**直接拿 `isa/*.toml` 与全部夹具当输入**，任何"schema 与真实谱不符"都会红（自由表 `fields = {…}`/`[[templates]].body`/`when = {…}` 在 schema 里本无子约束，守卫也不下钻）。`docs/reference/isa-dsl.md` 的键速查表与签入的 `isa-dsl.schema.json` 同步重新生成。
- 顺带修 `forge-isa insts` 表头把 `[meta].version` 标成 `schema`：它是 ISA 自己的版本串，与 DSL 语法版本无关，
  现在标 `version`（`[meta].version` 缺省时显示 `-`）。

### Added (2026-09-21)

- **ISA-DSL 文档重写（v18 S7e）**：`docs/reference/isa-dsl.md` 去掉 "v15" 历史标题与 v15 迭代总览段（移入归档），改为「版本与现状（v18）」——一张"唯一机制 ↔ 取代了什么"对照表 + 五条承诺；
  新增 **[`docs/guides/isa-dsl-tutorial.md`](docs/guides/isa-dsl-tutorial.md)**（30 分钟接入玩具 ISA TOY16：骨架 → 操作数槽 → 指令 → `[[templates]]` → 生成期自测 → CLI 自查，示例谱本机实跑 `validate`/`insts`/`explain` 通过）；
  新增归档 **[`docs/archive/isa-dsl-v12-v17.md`](docs/archive/isa-dsl-v12-v17.md)**（v12–v17 语法史 + v18 删除/改名总表 + 方案里提过但未采纳的改名）；
  `isa-dsl-errors.md` 补「多文件组合与部件选择」错误表（缺 include / 成环 / 同名标量冲突 / `[[override]]` 目标不存在 / `parts` + `spec_tests` 冲突）与多文件诊断定位说明；
  `docs/README.md`、`CLAUDE.md`（文档地图 + 三版本号辨析 + 三方守卫三条要点）同步；`bench_baseline.md` 的 `ir_dsl` 段补 v18 落地后的规格规模与**生成代码采集口径**提醒（S8 对照必须固定同一命令）。

- **ISA-DSL 多文件组合 `include` / `[[override]]` + 部件选择 `parts` + CLI `fmt`（v18 S7d）**。新增 `forge-isa-dsl::loader`（递归 `include`，深度上限 8、重复/成环报错、**按块合并**：数组节按 include 序追加、重复 `[表头]` 视为同节续写、同名标量冲突报错并提示改用 `[[override]]`），`[[override]] key = "点分路径" value = …` 显式覆盖被包含文件里的键（目标不存在即报错，拼错不静默）；诊断带**来源文件映射**——`路径:行:列` 指向真正写那一行的那份文件；生成物对**每个来源文件**都登记 `include_bytes!`（改任一片段都触发重编译）。
  `isa_from_file!` 参数扩到四个：`krate` / `spec_tests` / **`name = "…"`**（模块名覆盖，同一份谱展开多次必需）/ **`parts = ["encode", "decode", "asm", "tm"]`**（部件选择：`Inst`/`Reg` 枚举与寄存器表是任何部件的公共前提，恒定生成；受限时必须 `spec_tests = false`，否则编译期明确报错）。CLI 新增 `fmt [--out <file>]`：把多文件谱折叠成一份单文件 TOML（可继续编辑、可单文件分发、可独立校验、幂等）。
  `include`/`[[override]]` 同时进 `V12Model` + JSON Schema + 文档键表（三方针守卫覆盖）；绕过加载器把**裸文本**交给解析器时，带这两个组合键会明确报错而不是静默忽略。修掉 `report::validate_file` 把加载器消息改写成「读不到文件：<根路径>」的问题（缺 include / 覆盖键不存在时现在是点名文件与键的可诊断错误）。
  文档：`docs/reference/isa-dsl.md` 新增「多文件组合（`include` / `[[override]]`，v18 S7d）」节 + `isa_from_file!` 四参数表 + `fmt` 命令；方案 §5.8 改写为落地形态；夹具 README 增列 `include_root_v12` / `include_base_v12`。
  用例：`forge-isa-dsl/tests/parts_selection.rs`（7）、`forge-codegen/tests/include_v12_tests.rs`（7，含多文件黄金字节 + 只开 `encode` 的模块真编译）、`forge-isa/tests/cli_tests.rs` 增至 16（多文件 validate/insts、`fmt` 折叠与幂等、诊断指向片段文件、缺 include / 覆盖键不存在必须点名）。

### Changed (2026-09-21)

- **生成 `Inst` 变体的字段名 = `ops` 里声明的操作数名（v18 S7d 修正）**。此前字段名另取一套：定宽 ISA 取位域名（`[forms].operand_fields` 的 `rd`/`rs1`），变长 ISA 取语义角色名（`dest`/`cond`/`mem`/`imm`/`target`）——`ops` 里作者写的名字**只用于 asm 模板**，于是 `ops = ["dst:r:out", "src:r"]` 生成出 `Inst::Iadd { rd, rs1 }`：作者声明的名字在用户面没有任何意义，位域名还泄漏成了 API。
  现在**字段名就是声明名**（`Inst::Iadd { dst, src }`、x86 `MovRmR { src, dst }`、条件码 `JccRel32 { cc, target }`），位域名/语义角色名退回纯内部**编码键**（只用于查 `[conventions.bitfields]`、modrm 角色与立即数编码表）。名字不能直接作标识符时按最小规则归一：Rust 关键字 → 原始标识符（`type` → `r#type`）、数字开头 → 前缀 `_`（`8bit` → `_8bit`），**不做语义改名**。
  **编码行为零变化**：`demo_inst12_v12` 生成物 token 级对照（v18 S7a 基线 vs 现在）只有 `rd→dst`(32)/`rs1→src`(32)/`rs2→src2`(16) 共 80 处标识符改名，未触碰任何编码 token（证据 `target/s7d_field_rename_evidence.txt`）；三个 ISA 的黄金字节测试、417 条指令的生成期自测、三条 JIT 矩阵全绿。
  守卫与迁移：新增 `forge-isa-dsl/tests/field_names.rs`（定宽用声明名而非位域名、关键字原始化、数字开头加前缀、变长 ISA 按声明名）；`OperandUse` 的死字段 `field` 删除，改为携带**声明名** `name`；`crates/backend/forge-codegen/tests/*` 的引用同步改名（`dest`→`dst`、`rd`/`rs1`/`rs2`→`dst`/`src`/`src2`、`imm*`→`imm`、label 域→`target`、`cond`→`cc`）。文档：`docs/reference/isa-dsl.md` 的「命名操作数」「`[[forms]]`」「代码生成输出」三节改写为"声明名 = 字段名，位域名 = 编码位置"。

### Added (2026-09-20)

- **ISA-DSL 的 JSON Schema + `#:schema` 编辑器补全（v18 S7c），三方一致由守卫钉住**。`forge-isa-dsl::schema`（手写发射器，不引 `schemars`）把谱的 TOML 结构发射成 JSON Schema（draft 2020-12，32 个节定义 + 18 个根键，含必填/可选/编码键与节级说明）；`forge-isa schema [--out <file>]` 打印或写出，仓库根的 `isa-dsl.schema.json` 由它生成并签入；3 个发行 ISA + 6 个夹具的 TOML 顶部加 `#:schema <相对路径>` 注释，Taplo 等语言服务据此补全。
  **三方守卫**（`crates/frontend/forge-isa-dsl/tests/schema_guard.rs`，5 条）：① schema 每节的键集与 `v12/model.rs` 对应结构体的 `pub` 字段**逐键相等**（`#[serde(skip)]` 内部字段须在 `INTERNAL_FIELDS` 登记；`#[serde(flatten)]` 字段须在 `FLATTEN_FIELDS` 登记）；② 内部字段表不得过时；③ `docs/reference/isa-dsl.md` 新增的「键总览（速查表）」区段与 `schema::markdown_table()` **逐字相同**——文档里的键表不再手抄；④ 签入的 `isa-dsl.schema.json` 与发射器逐字相同；⑤ `--nocapture` 打印可粘贴的表格。守卫在落地时就抓到三处漂移（`enc` 被误当键、`Pattern` 的 `r#match`/TOML `match` 漏写、`[[pattern]].when` 未登记）。
  文档：`docs/reference/isa-dsl.md` 新增「键总览（速查表，v18 S7c）」节（机器校验的键表）+ 工具链节的 `schema` 与 `#:schema` 说明 + TOC；方案 §7「S7 进度」补 S7c。

### Added (2026-09-20)

- **`forge-isa` CLI（v18 S7b）：写 TOML 时不必接后端就能校验、看展开结果、做规格 diff**。新增 `crates/tools/forge-isa`（bin，唯一依赖 `forge-isa-dsl`；手写参数解析与 JSON 发射器，不引 `clap`/`serde_json`）：

  | 命令 | 作用 |
  | --- | --- |
  | `validate <谱.toml>…` | 解析 + 校验，打印**全部**诊断（`路径:行:列: 码: 消息` + 附注），有错退出 1 |
  | `insts <谱.toml>` | 展开后的指令与生效规格：字长（位/字节）、form、opcode、ops、编码键、`ref`、`reloc`、来源模板与行号、asm |
  | `explain <谱.toml> <指令名>` | 单条指令的完整来源：哪个模板的哪一行 + 该行与 `body` 的键 + 生效规格逐字段 |
  | `diff <a> <b>` | 两份谱的**规格 diff**（增/删/改字段，`字段: A → B`）——迁移前后"展开后有效规格"对照 |

  `--json` 给机读输出；退出码 `0` 成功 / `1` 诊断或失败 / `2` 用法错误。示例：`cargo run -p forge-isa -- explain isa/arm64_v12.toml ADDREGW` → `来源：[[templates.ADDREG]] 第 2 行` + 生效编码键。
  实现：新增 `forge-isa-dsl::report` 投影层（`IsaSummary`/`InstRow`/`Explain`/`SpecDiff`），**复用**编译器的 `collect_inst_infos`（form 预设 ⊕ 指令级覆盖的同一份判定），编码键清单由 `EncKeys` 的 serde 折出——不维护第二份键名表，新增编码键自动出现在三个子命令里。
  证据：`forge-isa` 10 条集成测试（跑真实二进制）+ `report` 6 条单测；`forge-isa-dsl` 182 单测；三份发行 ISA `validate` 全 OK；`insts isa/riscv64_v12.toml` = `116 条指令 / 19 条模板 / 110 条 lowering`；混合字长夹具 JSON 给出 `CADD16/CMOV16 = 16 位（2 字节）`、`LNOP32/LADD32 = 32 位（4 字节）`。
  文档：`docs/reference/isa-dsl.md` 新增「工具链：`forge-isa` CLI」节 + TOC；方案 §7「S7 进度」补 S7b 已落地。

### Changed (2026-09-20)

- **ISA-DSL 拆成两个 crate（v18 S7a）：`forge-isa-dsl`（编译器本体）+ `forge-dsl`（薄 proc-macro）**。原 `forge-dsl`（proc-macro）里的模型/解析/校验/诊断/代码生成整体搬进新的普通 lib **`forge-isa-dsl`**（`crates/frontend/forge-isa-dsl`，含 `v12/` 与 `assembler/`，以及两个反回潮守卫测试）；`forge-dsl` 只剩 `isa_from_file!` 的参数解析并调 `forge_isa_dsl::expand_file`。宏的使用方式与**生成代码逐字节不变**。
  动机：proc-macro crate 不能导出非宏项，而 ISA-DSL 的校验/`explain`/JSON Schema/`insts`/`diff`/CLI 都要在宏之外可用（S7b/S7c 的前提）。
  新公开面（`forge-isa-dsl`）：`expand_file(path, &ExpandOptions)`、`expand_str(source, mod_name, path)`、`validate_source`/`validate_file`（返回渲染好的诊断行）、`read_isa_file`、`dump_generated`、`ExpandOptions { krate, spec_tests }`。
  验证：临时 worktree 检出 S6 末状态（`113209c`）与本片各 dump 一次生成代码，路径前缀归一化后 x86/riscv64/arm64 + 6 个夹具共 9 个模块 **token 序列完全相同**（原始 dump 的行折叠差异只来自 rustc token 打印器对路径长度的换行启发式）；`forge-isa-dsl` 176 单测 + 2 `generality_guard` + 1 `no_hardcoded_widths` 全绿，`forge-dsl` 1 条参数解析单测；`forge-codegen` 全部 target、workspace 99 个 target、三架构 JIT 矩阵 195/3/0、131/67/0、23/175/0 不变；clippy（拆分后的三种组合）、release check（`--exclude forge-rustc`）、rustdoc、markdownlint 全干净。

### Added (2026-09-20)

- **ISA-DSL 生成期自测 `__spec_tests`（v18 S6）：每条指令自动进回归网**。生成器在 ISA 模块里再吐一个 `#[cfg(test)] mod __spec_tests`——写 TOML 的人不必再手抄"这条指令编出来是不是这几个字节"：

  | 断言 | 抓什么 |
  | --- | --- |
  | `encode` 成功 ∧ 长度 = 该指令字长（`prefix_scan` 除外） | 字长声明与编码不一致 |
  | `decode(bytes)` 成功 ∧ 消费 `bytes.len()` | 解码少读/多读 |
  | `encode∘decode` 与 `decode∘encode` 字节稳定 | 编码/解码不对称 |
  | 解码字段**值**原样（立即数按位域语义、条件码、寄存器索引、内存 base/disp） | 对称的位序/槽位错位（reg/rm 互换、扩展位丢失） |
  | `disassemble → assemble` 成功 ∧ 文本幂等 ∧ 再编码稳定；文本唯一时还要求字节相等 | 汇编/反汇编不对称、操作数序错、内存模板不闭合 |
  | 立即数 `min`/`max` 原样、`min−1`/`max+1` 在 `encode` 处**报错** | 静默截断/掩码（判据与编码器共享 `imm_encode_checked`） |

  覆盖维度 = **宽度视图**（多类槽逐宽度：x86 `gprx` 的 16/32/64 位分别走 66 前缀 / 无 REX.W / REX.W）× **高编号寄存器视图**（每组最高几个索引：REX.R/B/X、8 位寄存器的 REX 强制、EVEX 的 ZMM16-31）。断言的是**闭环不变式**，"字节对不对"仍由各 ISA 的编码参考文档与既有黄金值测试守着。
  生成模块导出 `SPEC_TOTAL`/`SPEC_COVERED`/`SPEC_CASES`/`SPEC_SKIPPED`（名字+原因，S6 判据：空）/`SPEC_TEXT_AMBIGUOUS`（同名同形、编码不同 ⇒ 文本分不清）；外部守卫 `src/spec_coverage_guard.rs` 钉死指令总数（x86 197 / riscv64 116 / arm64 104）、零跳过与歧义名单（31/4/0）。`isa_from_file!` 新增第三参数 `spec_tests = <bool>`（缺省 true）；夹具谱（`tests/common/mod.rs`，同一份谱被多个测试二进制包含）显式关掉，由 `tests/spec_tests_v12.rs` 打开三个极端形状夹具（1 字节寄存器 / 12 位字 / 混合字长）。
  覆盖结果：**417 条指令全部覆盖、零跳过**（用例数 x86 427 / riscv64 179 / arm64 258）；`cargo test -p forge-codegen` 26 个 target、workspace 97 个 target、三架构 JIT 矩阵 195/3/0、131/67/0、23/175/0 全部不变。

- **S6 当场抓到的两处真缺陷（已修）**：① riscv `SLLW/SRLW/SRAW` 的移位量共用了 6 位 `shamt` 槽而字段只有 5 位 ⇒ `sllw rd, rs1, 32..63` 被**静默编成 `n−32`**（不报错）；修法是 ISA 数据里加 `shamt_w`（5 位）槽并让三条 W 指令用它，越界现在在 `encode` 处报错。② x86 EVEX 寄存器直寻址的 `ModRM.rm` 是 `EVEX.X':B':rm[2:0]`（内存形式下 X' 才是 SIB index bit3），编码/解码两侧都只用了 4 位 ⇒ **ZMM16-31 当 rm 时静默编成 ZMM0-15**（`vaddps zmm16, zmm17, zmm18` 旧输出 `62 E1 74 40 58 C2` 实际是 `rm=zmm2`）；修法是非内存形态用 rm bit4 生成/还原 X'，黄金值更正为 `62 A1 74 40 58 C2`，并新增独立公开参照例 `vaddps zmm15, zmm24, zmm3` → `62 71 3C 40 58 FB`。
  文档：`docs/reference/isa-dsl.md` 新增「生成期自测（`__spec_tests`）」节（断言表 + 覆盖维度 + 常量 + `spec_tests` 参数）、方案 §5.9 与 §7「S6 进度」、arm64 指令数由 89 更正为 104（S3c 的 `b.cond` 16 行）。

### Added (2026-09-20)

- **ISA-DSL 指令宽度三态 `[encoding]`（v18 S4）：拆掉"一个 ISA 一个字长"的假设，打开 RVC/Thumb 类混合字长 ISA**。`[meta]` 的四个宽度散键（`default_inst_width` / `variable_length` / `max_inst_len` / `default_opsize`）删掉，收敛成独立的一段，**逐指令** `width` 由指令（或模板行）自己写：

  ```toml
  [encoding]
  kind = "fixed"          # fixed | mixed | prefix_scan
  bits = 32               # fixed：字长（位，任意 ≥ 1）

  # kind = "mixed"（16 位短编码 + 32 位长编码共存）
  # bits = 16             # 可选：逐指令 width 的缺省
  # widths = [16, 32]     # 必填：允许的字长集（解码按升序分组尝试）

  # kind = "prefix_scan"（x86：长度由前缀链决定）
  # max_len = 15          # 可选：最长指令字节数（缺省 15）

  default_opsize = 32     # 可选（原 [meta].default_opsize 归位到本段）
  ```

  - `fixed`：全 ISA 一个字长（= 旧 `default_inst_width` 语义），编码/解码路径与生成物**逐字节不变**；三发行 ISA 与 5 个夹具全部迁移到本段。
  - `mixed`（**新能力**）：编码按该指令字长发字节；解码**按 `widths` 升序分组**、每组一棵位级 trie、"首个完整匹配即停"（短编码优先，RVC/Thumb 同构）。新夹具 `crates/backend/forge-codegen/tests/isa/demo_mixed16_32_v12.toml`（低 2 位判别短/长编码：`sel = 0` vs `3`）+ `tests/demo_mixed16_32_v12_tests.rs` 5 条用例（黄金字节、按宽度分组解码与消费字节数、编解码往返、`decode_partial` 截断阈值 = 最短字长、能力集）。
  - `prefix_scan`：x86 走原有 `vlen.rs` 变长路径，只把"最长长度"换成 `max_len`；分派从"变长/定宽二分"改为按 `is_prefix_scan()` 三态分派（`fixed`/`mixed` 共用定宽位域编解码）。
  - **校验期结构性互斥**（错误码 `DSL-ENCODING`）：`fixed` 写 `widths`/`max_len`、`prefix_scan` 写 `bits`、`mixed` 写 `max_len`、逐指令 `width` 不在 `widths` 里或与 `bits` 不一致 → 编译期报错；**省略整个 `[encoding]`** 仍是合法骨架文档（= `fixed` 且无 `bits`），生成期 `inst_bytes()` 报"bits 缺失"——省略整段不会被静默当成定宽 32，逐指令 `width` 也不能替代 `bits`。
  - 能力集（`IsaCapabilities`）由三态派生：`fixed` → `fixed_inst_size` = `min` = `max` = `bits/8`（定宽 ISA 以前报 `0` = 未知，现在是真实字长）；`mixed` → `fixed_inst_size = 0`、min/max = 最窄/最宽、`variable_length = true`；`prefix_scan` → min = 1、max = `max_len`。
  - 未做（如实记录）：草案里的 `[encoding].prefixes` 前缀效果表——x86 前缀语义已在 `[conventions.prefix_scan]` + `vlen.rs`，再造一张表是同一事实两处声明。`mixed` 的短/长判别位由 ISA 自己保证互斥（校验器无法通用地证明，不做假保证），文档与夹具注释写明。
  - 证据：`cargo test -p forge-dsl` 175 + 2 + 1 全绿；workspace 测试 96 个 target 全绿；三架构 JIT 矩阵 **x86 195/3/0、riscv64 131/67/0、arm64 23/175/0**（与 S3 完全相同）；生成代码对拍（8 个模块）差异仅"文档字符串 `[meta].default_inst_width` → `[encoding].bits`"与上述能力集字长；clippy 两道、release check（`--exclude forge-rustc`，见下）、rustdoc、markdownlint 全干净。
  - 顺带发现（未修，超出本片范围）：`cargo check --workspace --release --all-targets` 在 `forge-rustc` 上失败——`types.rs::assert_assignable` 是 `#[cfg(debug_assertions)]` 而 `lower/place.rs` 无条件调用（两文件最后改动 2026-09-04，与本片无关；CI 只跑 debug 的 `cargo check -p forge-rustc`）。

### Added (2026-09-19)

- **ISA-DSL 汇编器伪指令 `[[pseudo]]`（v18 S3e）：汇编期的文本级多指令展开**。`.equ`/`.macro` 的结构化兄弟——不碰编码器、不碰 lowering：`parse_insts` 遇到以伪指令名开头的行，就按 `params` 位置切分实参、逐行把 `{参数}` 换成实参文本，再让**同一套汇编器**装配展开出的每一行。

  ```toml
  [[pseudo]]
  name = "li"                     # 汇编可见的助记符（不得与指令助记符重名）
  params = ["rd", "imm"]          # 位置实参名（emit 里用 `{名字}` 引用）
  emit = [
    "lui {rd}, ({imm} + 0x800)",
    "addi {rd}, {rd}, ((({imm} + 0x800) & 0xfff) - 0x800)",
  ]
  ```

  实参按**顶层逗号**切分（`()`/`[]` 内的逗号不算，故 `li x1, (a + b)` 与内存操作数 `[x2, #4]` 都能写）；emit 行里的算术由**既有表达式求值器**求值（`+ - * / % << >> & | ^ ~`、括号、`.equ` 符号）——**不引入第二套表达式语言**；emit 行可以是**别的伪指令**（递归展开，深度上限 16 防自引用成环）；单条 `assemble()` API 只接受展开成 1 条的伪指令，多条要用 `TargetAssembler::parse_insts`（错误消息指引）。
  riscv 的 `li` 是实际用例：`li x10, 0x1234` → `lui x10, 1`（0x00001537）+ `addi x10, x10, 0x234`（0x23450513），与 lowering 里 `Iconst` 用的 `{iconst_hi20}`/`{iconst_lo12}` 完全同一套算术（逐值对照 0/-1/0x800/表达式实参/与普通指令混排）。
  新增校验 `validate_pseudos`（错误码 `DSL-PSEUDO`）：名字非空/唯一/**不得与指令助记符重名**（重名会让汇编器永远匹配不到它）；`params` 非空唯一；`emit` 非空、行首词必须是指令助记符或别的伪指令名；`{…}` 必须是声明的参数且**每个参数都得用到**。**没有 `[[derive]]`/`[[pseudo]]` 的 ISA 不生成任何展开器代码**（x86/arm64 与 5 个夹具的生成物只差 `assemble()` 那 2 行新增文档注释）。
  未实现（如实记录，不做半成品）：按谓词分派同名多条（汇编期没有 IR 属性可判）、`pseudo_fold`（反汇编折叠回伪指令——那是指令级模式识别，与 `[[pattern]]` 同类问题）。
  证据：forge-dsl 175 + 2 + 1（新增 6 条声明侧用例）、`forge-codegen` 全部套件（`asm_enhance_tests` 15 条，新增 2 条 riscv 行为用例）、三架构 JIT 矩阵（195/3/0、131/67/0、23/175/0）、clippy、release、rustdoc、markdownlint 全干净。

- **ISA-DSL 派生谓词属性 `[[derive]]`（v18 S3f）：给重复出现的 `when` 条件起名字**。同一条件在多条 lowering 规则里重复时，一次声明、多处引用：

  ```toml
  [[derive]]
  name = "is_64"
  expr = { eq = ["rs1_width", 64] }

  [[lowering]]
  op = "Iadd"
  when = { eq = ["is_64", 1] }
  insts = ["ADD64 {out}, {0}, {1}"]
  ```

  `expr` 复用结构化谓词（同一份解析与校验），派生属性取值 = 1（真）/0（假），用 `eq`/`ne`/`in` 引用；展开在**解析期**——生成期的 `__attr` 只多一个派生臂（判定走核心属性表 `__attr_core`），谓词判定逻辑一行未改。名字不得与核心属性重名（否则静默遮蔽）、不得引用另一个派生（属性名出现在值位置的代换语义不唯一，如实拒绝并提示"派生不能引用派生"）；派生名可出现在 `vary` 里，与核心属性同待遇（自动追加 `eq = [名字, 行值]`）。错误码 `DSL-DERIVE`。
  **零成本**：没有 `[[derive]]` 时生成的属性表与引入前**逐字相同**（8 个模块 dump 逐个 `identical=True`）——三发行 ISA 与夹具都没用它，所以三架构 JIT 矩阵与全部黄金值不变。
  证据：新增 6 条用例（`when` 引用派生 + 生成的派生臂、无派生时不新增一层、`vary` 用派生名、核心属性重名、重名、`expr` 非法/类型错、派生引用派生）；文档 `docs/reference/isa-dsl.md` 结构化谓词节补 `[[derive]]` 小节与 `iconst` 行、`isa-dsl-errors.md` 增 `DSL-DERIVE`、方案 §5.6 改记为已落地并说明"数值派生（`sub` 等算术节点）未实现"的理由。

- **ISA-DSL 重定位数据化（v18 S3d）：`[[reloc]]` 取代 `GlobalReloc` 枚举**。指令侧只写引用名（`reloc = "abs64"`），语义与绑定槽在表里：

  ```toml
  [[reloc]]
  name = "abs64"            # 指令引用：reloc = "abs64"
  semantics = "absolute"    # 宿主语义（有限、ISA 无关）：absolute / pc_relative
  slot = "imm64"            # 绑定到哪个操作数槽（须是 imm 槽）
  addend = 0                # 可选：重定位值的链接期加减
  ```

  两种语义的 fixup 落点由**数据**决定：`absolute` → 指令末尾该槽的字节区间（补丁宽度 = `ceil(槽宽/8)`，x86 `MOVABS_GLOBAL` 的 imm64 → `Absolute(8)`）；`pc_relative` → 指令起始（补丁宽度 = 指令字长，riscv `AUIPC_GLOBAL`/`ADDI_GLOBAL` → `Relative(4, 0)`）。**宿主语义集合就是 `RelocKind` 的两态**——计划里列的 `hi20`/`lo12`/`got`/`tls_*` 属于"新增语义才需要宿主代码"那一侧，等真有 ISA 用到再加；ISA 特有的**位段写入**（arm64 写 imm26/imm19、riscv 按 opcode 0x17/0x13 分写 hi20/lo12）仍留在各 ISA 的 reloc patcher，这是数据之外唯一的宿主代码。
  删除 `GlobalReloc{Abs8,PcrelHi,PcrelLo}` 与 `Instruction.global_reloc`（无兼容层）；新增 `validate_relocs`（名字非空唯一、`slot` 已声明且是 imm 槽、指令引用的名字必须在表里、该指令确实有那个槽的操作数）与错误码 `DSL-RELOC`。
  证据：生成代码只在重定位臂上从 `ABS8`（定义即 `Absolute(8)`）变成 `Absolute(8)`、addend 字面量 `0` → `0i64` —— **语义等价**（x86 6 行、riscv 4 行差异，其余模块逐字节相同）；三架构 JIT 矩阵不变（x86 195/3/0、riscv 131/67/0、arm64 23/175/0；x86/riscv 矩阵含 `GlobalAddr` 用例，端到端跑过重定位）；新增 7 条 reloc 用例（解析 + 生成 `Relative(4,0)`/`Absolute(ceil(槽宽/8))`、未声明名、坏槽、重名、槽不在操作数里、语义名非法）。
  文档：`docs/reference/isa-dsl.md` 新增 `[[reloc]]` 节（含两语义的 fixup 表）并更新 `reloc` 字段、`isa-dsl-errors.md` 增 `DSL-RELOC`、方案 §5.4/§7。

- **ISA-DSL arm64 条件码符号化 + `b.cond` 全条件（v18 S3c）**：`isa/arm64_v12.toml` 新增 `[conventions.cond]`（A64 的 16 个条件名 `eq/ne/cs/cc/mi/pl/vs/vc/hi/ls/ge/lt/gt/le/al/nv` + `hs`/`lo` 同码别名；**不写 `ir`**——arm64 的 lowering 目前不用 `{cc}`，将来加 Icmp lowering 时 validate 会强制补全 10 个），CSEL 族的 `cond4` 槽从 `kind = "imm"`（写成 `#0`）改为 `kind = "cond"`：汇编/反汇编现在用**符号名**（`csel x0, x1, x2, eq`，反汇编渲染同码首选名 `hs`→`cs`）。
  新增 `B.cond`：一条 `[[templates]]` 16 行——`asm = "b.{cname} {target}"`（助记符用行键插值，这是"条件在助记符里"的通用写法）+ 条件码做成固定位域 `bcond = [3:0]`（opcode `0x54` 进 `[31:24]`、imm19 在 `[23:5]`、bit4 恒 0）。**14 个 A64 合法条件**（`[3:0]=111x` 保留）逐个对照 `docs/reference/aarch64-encoding-ref.md` §4 的条件码表验证字节：`b.eq 0`=0x54000000、`b.ne 0`=0x54000001、`b.hs 0`=0x54000002、`b.gt 0`=0x5400000C、`b.le 0`=0x5400000D、`b.eq 2`=0x54000040（偏移进 imm19），外加汇编↔反汇编往返与 `b.hs`≡`b.cs`、`b.lo`≡`b.cc` 同码断言。
  顺带**删除**原先那条错的 `BCOND` 存根（`form = "CBZF"` + `fields = { cond = 0 }`：目标被放进 `rt=[4:0]`、条件恒 0 且落在 `[15:12]`、imm19 恒 0——从来不是合法 B.cond，也无人使用；`b.eq` 之前被它抢先匹配）。
  生成器侧补一个缺口：**定宽解码器**原先对非 Reg 槽一律产出 `i64`，cond 槽在 `Inst` 里是 `u8` ⇒ 补上 `OperandKind::Cond => raw as u8`（arm64 是定宽 ISA 的第一个 cond 槽用例）。
  文档：`docs/reference/isa-dsl.md` 的条件码节补"条件当操作数 / 条件在助记符里"两种写法、`aarch64-encoding-ref.md` 记 S3c 进展并更新"待做"、方案 §7 记 S3c。

- **ISA-DSL 条件码数据化（v18 S3b）：`[conventions.cond]` 一张表服务三处，删掉 x86 硬编码表与缺省；通用性守卫白名单清空**。表从 `名 = 编码` 变成 `名 = { code, ir? }`（也接受整数简写 `eq = 4`，此时 `ir` 取键名）：键 = 本 ISA 汇编/反汇编可见的条件名，`code` ∈ 0..=15，`ir` = 它实现哪个 IR 整数条件（`eq/ne/slt/sle/sgt/sge/ult/ule/ugt/uge`）。三处用途：**汇编**按名解析 `cond` 槽、**反汇编**按码取同码里字母序最小的名（渲染与 S3b 前逐字节一致）、**lowering 的 `{cc}`** 按 `ir` 字段查本 ISA 编码。
  删掉两处 x86 硬编码：`lowering.rs` 的 `IntCC → setcc 编码` match（生成代码里不再出现 `IntCC`）与 `asm.rs` 的 `cond_default()`（未声明表时的 x86 16 项缺省）——现在"没声明就没有条件码能力，用到即报错"。宿主新增 `forge_ir::intcc_name(码) → 条件名`，把"IR 条件"这一侧的键空间收敛到一处（`INTCC_NAMES`/`IntCC::mnemonic`）。
  新增校验（`validate_cond`）：表非空、`code ≤ 15`（4 位条件字段）、`ir` 必须是 10 个规范名之一、**一个 IR 条件只能被映射一次**、`cond` 槽需要表、**用了 `{cc}` 就必须把 10 个条件映射全**（否则运行期静默退化成 0 = 溢出条件，是最难查的一类错）。
  证据：x86 生成代码的 `{cc}` 映射与新表逐条相同（`eq→4 / ne→5 / slt→12 / sle→14 / sgt→15 / sge→13 / ult→2 / ule→6 / ugt→7 / uge→3`），x86/riscv/arm64 其余生成物只在"未使用的 `__cc` 绑定"上变小；x86 黄金/汇编/解码 78 例 + 三架构 JIT 矩阵（195/3/0、131/67/0）不变；`tests/generality_guard.rs` 的 `ALLOWED` **清空**（生成期代码里已无任何 ISA 常量：指令名、寄存器名、条件码全来自 `isa/*.toml`）。文档：`docs/reference/isa-dsl.md` 新增"条件码表（一张表，三处用）"、`isa-dsl-errors.md` 增条件码错误表、方案 §5.3/§7。

- **ISA-DSL 三处"别家常量兜底"改 fail-closed（v18 S3a）：缺失的 ABI 声明不再回退到某个 ISA 的寄存器名/指令名**。①`frame.rs` 的栈参数收参 scratch 寄存器原先在 `[abi].scratch` 缺失时回退 x86 的 `R10`（生成的代码引用 `Reg::R10`——非 x86 ISA 直接编译不过）；②spilled 寄存器参数收参需要 MOV 时原先用 `inst_exists(infos, "MOV_RM8_R64")` **按 x86 指令名**探测，而同一函数里 `move_inst` 早已由 `roles = ["gpr_mov"]` 派生；③`lowering.rs` 的 Call/CallIndirect 两处在 `[abi].call_ret_reg` 缺失时回退 riscv 的 `X1`。现在三处都**生成期明确报错**，且**只在真的需要时才要求**（无栈参数的 ISA 不需要 scratch；call 指令没有 Out/InOut Reg 槽的 x86 不需要 `call_ret_reg`）。
  证据：三发行 ISA + 5 个夹具的 `FGE_DEBUG_GEN` dump 与改前**逐字节相同**（对现网谱是纯收紧），新增两条负向用例——真实 x86 谱删掉 `scratch` → 报错点名 `[abi].scratch`；真实 riscv 谱删掉 `call_ret_reg` → 报错点名 `call_ret_reg`。

- **ISA-DSL 定义指令的机制统一为**一个**（v18 S2c）：`[[templates]]` + `rows` + 指令属性 `ref`，`[[families]]`/`[[aliases]]` 全删**。起因是评审意见——同一件事有 `[[templates]]`/`[[families]]`/`[[aliases]]` 三个模块、三套校验、三种诊断前缀，是使用负担（"只能保留一个"）。
  合并规则只有两条：**多条指令共用一份声明** → 模板的 `body`（共享字段，可省略）+ `rows`（每行一条指令：`inst` 必填，其余键是取值/字段）；**一个引用名指向多条指令** → 指令上的 `ref`（多条共用同一 `ref` = 多态分派，取代 `[[aliases]]`）。
  行键分三类且**只由 `body` 决定**：body 里有同名键 ⇒ 覆盖（表递归合并）；body 没有但被 `{键}` 引用 ⇒ 纯参数（只插值）；其余 ⇒ 指令字段（拼错由 `Instruction` 反序列化点名拒绝）。`{inst}`/`{键.lower}` 派生实例名与助记符，整串恰为占位符时保留类型。`rows` 是 TOML 数组，`rows = [ { … }, … ]`（一行一条）与 `[[templates.rows]]` 是同一结构的两种写法。
  三 ISA 与全部夹具迁移：arm64 `871 → 891 行（+2.3%）`、riscv `1,899 → 1,802（−5.1%）`、x86 `3,887 → 3,678（−5.4%）`，`[[families]]` 14 条（100 变体）与 `[[aliases]]` 23 条全部消失，指令总数不变（89/116/197）。arm64 略增是换来的：原先 2 行指令用平行数组按列 zip 写得极紧凑，但要逐行对齐、加一条变体改 3 个数组。
  等价证据分三层：①**模型级**（迁移脚本 `target/unify.py` 自带自检）旧展开 vs 新展开逐指令比较 form/opcode/fields/ops/asm/roles/when/编码键 + **引用名 → 成员有序列表**，三 ISA 全等；②**生成代码级** arm64 与 5 个夹具**逐字节相同**，riscv/x86 每个函数的 token 多重集完全相同（差异只是 `Inst` 枚举/编码臂/解码 trie 的顺序——旧顺序是"手写指令 → 模板 → 族"，新顺序是"手写指令 → 按文件序的模板"，见方案 §12.8）；③**行为级** 黄金测试（arm64 12+6、riscv 12+4、x86 61 例）与三架构 JIT 矩阵（23/175/0、131/67/0、195/3/0）全不变、语料三支通过。顺带补回一条被族校验覆盖过的通用校验：**指令必须有编码来源**（`opcode`/`fields`/`opcode_reg`/`modrm`/`vex`/`evex`/`imm` 至少一个），现在对所有指令生效。
  证据与文档：`src/v12/model.rs`（`Template{name, body, rows}` + `TemplateRow{inst, fields}` + `deep_merge`/`subst`/`referenced_keys`）、`src/v12/template_tests.rs`（24 例）、`src/v12/diag_matrix_tests.rs`（30 例矩阵改成模板/`ref` 用例）、`docs/reference/isa-dsl.md` 的 `[[templates]]` 节重写为**唯一机制**（含旧→新对照表与实测收益）、`docs/reference/isa-dsl-errors.md`（`DSL-FAMILY`/`DSL-ALIAS` 删除，新增 `DSL-TEMPLATE` 与模板错误表）、方案 §5.2/§6/§7/§12.7/§12.8。

- **ISA-DSL 参数化模板 `[[templates]]`（v18 S2a，**首版机制形态**：`params` 平行数组**，已被同日 S2c 改为 `body` + `rows`；本条目保留首版迁移的实测数字）**：一处声明 → N 条指令，自动派生别名。动机是实测出来的重复：`isa/arm64_v12.toml` 有 **38 对**指令只差 sf 位/寄存器槽/opcode（76/89 = 85%）、`isa/riscv64_v12.toml` 有 **16 对** S/D（41%），而旧 `[[families]]` 的变体**无法覆盖 `ops`/`form`/enc 键**，只能整条复制；与之配套的 `[[aliases]]` 还是手写清单（x86 25 条 / arm64 29 条，且已见漂移：`vaddps` 漏了 `VADDPS_ZMM_MASKZ`）。
  新机制：`params` 等长列表按下标 zip（与 `[[lowering]].vary` 同语义）；字符串字段里 `{参数}` 做文本替换、**整串就是 `{参数}`** 时保留参数类型（`opcode = "{opcode}"` 仍是整数）；`name`/`names` 给实例名；`ref` 自动派生别名（单值 = 多态共用，含 `{参数}` = 逐实例）；`[[templates.overrides]]` 逐行打补丁（表递归合并）。展开在**解析期**完成 ⇒ 下游（校验/代码生成）只看到普通指令与别名，`codegen` 一行未改；展开出的实例若非法，诊断前缀改回 `[[templates.X]]`，位置落在模板声明行。
  **arm64 首例迁移**：`isa/arm64_v12.toml` **1,125 → 731 行（−35%）**，32 条模板取代 64 个手写指令块、**29 条手写 `[[aliases]]` 全删**；黄金测试（`arm64_v12_tests` 12 例 + `arm64_v12_tm_tests` 6 例）通过、arm64 JIT 矩阵 **23 passed / 175 skipped / 0 failed** 不变、生成代码里的 `Inst::` 名字集合 **90 个前后 diff 为空**、arm64 生成代码 644,018 → 636,986 B。
  证据与文档：`src/v12/template_tests.rs`（10 例：展开/类型保留/逐实例 ref/补丁合并/五类失败路径/lowering 引用实例名）、`v12/model.rs::Template` 文档、`docs/reference/isa-dsl.md` 新增 `[[templates]]` 节、`docs/reference/isa-dsl-errors.md` 增模板错误表、方案 §7/§12.7 记录实测、通用性守卫扩展为也识别模板实例名（arm64 迁移后 `ADDIMMX` 不再是 `[[instructions]]` 字面量）。

- **ISA-DSL S2 续：riscv 与 x86 定向迁移（三 ISA 全覆盖；**数字为 S2a 形态**）**。riscv `1,754 → 1,606 行（−8.4%）`：15 条模板取代 30 个 `_S`/`_W` ↔ `_D` 指令块（`funct7`/`funct3` 逐行给、助记符后缀 `.{pl}` 插值）——顺带证明 **`[[families]]` 表达不了这类对**（族模板 `{name}` 只能派生"变体名小写"，得不到 `fadd.s` 这种带点助记符，这正是它此前只能手写两份的原因）。x86 `3,356 → 3,324 行（−1%）`：`movzx`/`movsx` 的 R8/R16 四条合成 1 条模板（`ref = "{mn}"` 顺带取代 2 条手写 `[[aliases]]`），四条指令的**生成编码臂文本 SHA-256 逐字节相同**。
  **当轮的设计结论已被 S2c 推翻（保留作过程记录）**：S2a 曾得出"`[[templates]]`/`[[families]]`/`[[aliases]]` 三者**互补而非取代**"，于是选择保留三者分工；S2c 按评审意见把它们合并为 `[[templates]]` 一个机制（见本节第一条）——"互补"成立的是**表达力**判断，不成立的是**该不该并存**：`rows` 的键空间就是指令字段空间，族变体只能覆盖 `fields`/`opcode`/`roles` 的限制消失，别名折成 `ref` 后引用表逐项相同。x86 剩余的同 asm/同 ops 组仍差在**编码键**（`form` 预设有无、`opsize`、内联 vs preset）——那是不同编码而非可参数化取值，统一它们属于语义重构，**不做**（此结论 S2c 沿用）。
  验证：三 ISA 的 `Inst::` 名字集合前后 diff 为空（90 / 117 / 198）、黄金测试（arm64 12+6、riscv 12+4、x86 61 例）与三架构 JIT 矩阵（195/3/0、131/67/0、23/175/0）全不变。

### Changed (2026-09-19)

- **ISA-DSL 诊断升级（v18 S1）：一次列全 + 精确到键行 + 错误码；`[emit]`/`[spill]` 从"完全不校验"变为编译期校验**。执行方案 `docs/plans/forge-dsl-v18-plan.md`（用户口径：允许破坏性更新、无需兼容旧版本、兼顾体验、足够通用），本条目是其中的 S1 切片。
  诊断侧：`validate` 从"14 个校验器 fail-fast、一次只报第一条"改为**收集式**（节级 + 逐条声明，最多 32 条，超出追加"另有 N 条"）；定位从"`source.find("name = …")` 全局启发式（同名会指错、抽不出就退化 `1:1`）"改为**声明索引**（一次预扫建"节 + 名字 → 块行范围"，块内再用消息里引号点名的值精确定位到**出错的键行**）；重复声明指向**后出现**的那一处并附注另一处位置；错误码 `DSL-<节>` 稳定可过滤，目录见 `docs/reference/isa-dsl-errors.md`。
  校验侧补上两个"未校验的名字引用面"：`[emit.prologue/epilogue].insts` 与 `[spill.*].load/store` 现在校验指令引用名、`@` 伪指令（`@push_callee`/`@pop_callee`/`@frame_alloc`/`@frame_free`/`@move_args`）、占位符（emit：`{frame_size}`/`{frame_size_neg}`/`{frame_size_mN}`/`{callee_saved_bytes}`；spill：编号 `{N}`）与 `base` 寄存器名。**这会拒绝此前静默通过的 spec**（S0 基线实测：`[emit]` 里写 `NO_SUCH_INST`、`@nope`、`{bogus}` 全部 `<ok>`），三份发行谱已实测通过新校验。
  证据：`src/v12/diag_matrix_tests.rs`（30 例错误码/位置矩阵 + 多错并列 + 附注 + 上限 + 头条精确位置）、`v12/diag.rs` 单测（索引/精准定位/渲染/上限）、S0→S1 对照表在方案 §12.4/§12.6；`cargo clean -p forge-codegen` 后 `cargo check -p forge-codegen` 通过（34.7 s）。
  同批修掉两处 doc/code 漂移：`docs/README.md` 里 `reference/isa-dsl.md` 标注"文档 v15 / 代码 v16–v17"与 `reference/binary-format.md` 更新为 v2。

### Fixed (2026-09-19)

- **文本打印不再丢"多别名 metadata"的名字**（二进制语料往返测试顺带暴露的老问题）：一个 metadata 节点被多个名字指向时（`!foo` 与 `!\23pragma` 内容相同 ⇒ `intern`/`insert_at` 去重成同一节点），display 原先每节点只打印**一个**名字（且 `MetadataStore::name_of` 用 `HashMap::iter().find(...)`，打哪个随实例随机种子变化）⇒ **丢名字 + 输出不确定**。现在 display 对每个别名各打印一行（名字按字节序 ⇒ 同输入同输出），新增回归用例 `display_llvm.rs::named_metadata_prints_every_alias`（两行都在 + 再解析再打印逐字节相同；改前必失败）；参考文档"已知限制"一节相应移除该项（现 §12）。

- **补上二进制解码的 metadata 嵌套深度上限（执行方案 §2.6 承诺项）**：`MetadataValue::Field` 是递归结构，解码器原先会一直递归到输入末尾——一条 `Field(k, Field(k, Field(k, …)))` 就能把**解码器的栈打爆**（进程 abort，而不是可捕获的 `Err`）。现在 `MAX_METADATA_DEPTH = 64`，超过即 `IrError::BinaryDecode`（带偏移）；负向用例含 **5000 层**嵌套（必须在到达上限时返回，而不是递归到底），另有 8 层正常解码的正向对照。同步把"编码侧假定自洽 IR、不自洽时**带原因 panic**；解码侧任何输入都不 panic"写进 `Module::{to_binary_into, from_binary}` 的 API 文档，参考文档 §5 的原语表补上该上限。
  实测（本机 2026-09-19）：workspace **1690 passed / 0 failed / 19 ignored**；语料 198 正向 / 254 正确拒绝 / 0 误收（198 例二进制往返仍全部文本一致 + 字节幂等）；fmt/clippy 两道门/release/doc/markdownlint 全干净。

### Added (2026-09-19)

- **文件级 IR 缓存 `IrCache`（消费方收益②）+ 它的盈亏实测**：新增 `binary/cache.rs`，公开面 `forge_ir::{IrCache, CacheKey}`——按**内容键**（源码字节 + 格式版本 + producer 的 128 位指纹）把"源码文本 → `Module`"这一步落盘，`load`/`store`/`get_or_insert_with`/`entry_path`/`entry_count`；**自愈**（读失败/损坏/版本不符一律当 miss，miss 时重算并覆盖）、**原子落盘**（临时文件 + `rename`，不读到半截条目）、**失败不缓存**（`build` 返回 `None` 不落盘）。定位是"可随时删除的加速层，不是事实源"（128 位指纹非密码学哈希，已在模块文档交代）。
  接线与实测（本机 2026-09-19，`cargo bench -p forge-ir --bench ir_binary -- ir_binary_cache`）：基准新增 `ir_binary_cache`（884 B 文本）与 `ir_binary_cache_large`（43 KB 文本）两组，各自拆出 `parse_text` / `cache_hit` / `read_entry` / `decode_entry`。
  **结论按尺度分**（每项三次运行）：884 B 的中等模块 `cache_hit` 114–195 µs vs `parse_text` 77–94 µs（**亏 1.2–2.3×**——命中要付文件读取的固定成本 75–204 µs）；43 KB 的大模块 `cache_hit` 723–1146 µs vs `parse_text` 1835–2320 µs（**赚 2.0–2.5×**，每个模块省 ~0.9–1.4 ms）。
  示例新增 `--cache <dir>`（打印 HIT/MISS）；在 452 个 `.ll` 的夹具目录上端到端对照：不缓存 272/155 ms、缓存 179/194 ms ⇒ **无可测收益**（332 个文件本就解析失败，可解析的 119 个都太小）。**用法约定**：机制保留，但**不进正确性测试**（缓存会短路"文本 → 模块"，让测试少测解析器一层）——完整表在 `docs/performance/bench_baseline.md`，规范与约定在 `docs/reference/binary-format.md` §11。
  **顺带给出"收益③（rustc e2e 的 IR 级缓存）"的评估结论：不做**——`crates/tools/forge-rustc/` 里 `parse_module`/`from_binary`/`to_binary` 零命中（它是 rustc 的 codegen 后端，IR 直接从 MIR 在进程内降低，从不读文本 IR），且整支 198 文件语料的解析合计 ~0.9 s 而单个 e2e 用例是秒级工作；理由与触发条件记在 `docs/plans/forge-ir-binary-serialization-plan.md` §10.2。

- **二进制格式 v2：段体压缩（体积减半，零依赖、确定性）**。`IR_FORMAT_VERSION` 1 → **2**，段表条目加第 4 字段 `raw_len`（`0` = 段体原样存放、`> 0` = 段体是压缩体、值 = 解压后长度）；新增 `binary/pack.rs`（自研 LZ77 变体：token = 偶数字面量段 / 奇数回引段 + 距离 varint，窗口 64 KiB、最小匹配 4、单 token ≤ 64 KiB、贪心最长匹配、候选链长上限 64、哈希表长随输入自适应且**跨段复用**的 `Packer`）。写侧**只在压缩体严格更小时**才压（无旋钮、无开关 ⇒ 同输入同输出；由 `packed_entries_are_always_strictly_smaller` 钉住"永不越压越大"）；读侧解压后的段体与 v1 逐字节相同，段解码路径**一字未改**。
  实测体积（198 个 LLVM `test/Assembler` 正向语料）：字节流 241,440 → **121,257 B（×0.50）**，对源码文本 0.59× → **0.30×**，最大单例 31,038 → **8,482 B**，平均 1,219 → **612 B**。实测吞吐（`--bench ir_binary`，中等模块）：encode 21.9 → **22.5 µs**（三次运行 22.1/30.5/22.5 的中位数 ⇒ 压缩的成本落在计时噪声内；中途"每段新建 256 KiB 哈希表"版本实测 184.1 µs，改 scratch 复用 + 表长自适应后回到基线量级）、decode 77.2 → **42.9 µs**（三次运行 43.5/42.9/32.0，字节少 28% ⇒ 读侧游标/切片/校验都少）。**本机同点波动可达 ~1.7×**（同一版代码曾测得 encode 45.2 µs 与 26.0 µs），故文档一律按"方向 + 多次运行"记录、不当单点回归判据。
  解压安全（全部在**进入解压循环之前**或恰在当时 fail-closed）：单段 256 MiB 上限、压缩比 65536×+4096 上限、距离必须落在已产出字节内、token 长度不得超过"还差多少到 `raw_len`"、结束时必须恰好产出 `raw_len` 且输入恰好耗尽；负向用例覆盖坏段体首字节（空字面量 token）、`raw_len` 翻转（产出与声明不符）、压缩体截断、两路解压炸弹。格式规范见 `docs/reference/binary-format.md` §8（含"为什么自研"与"将来按段换成 zstd 不需要改格式"），基线对照表见 `docs/performance/bench_baseline.md`。
  门禁（提交 ecd2528）：fmt/clippy 两道门/`check --release --all-targets`/`doc -D warnings`/`bench --no-run`/语料三支/三架构 JIT 矩阵/markdownlint 全干净；workspace 测试 `-j 1 --test-threads=1` **1604 passed / 0 failed / 19 ignored**（92 个测试二进制）；**GitHub Actions CI run #131 全绿**（Linux/Windows/macOS 测试、Benchmarks check、Clippy、Format、Docs、Coverage、forge-rustc check + e2e、forge-tests nightly 共 11 个作业）。

- **二进制格式有了可运行的消费者 + 吞吐/尺寸基线**：新增示例 `crates/foundation/forge-ir/examples/ir_binary.rs`（只用公开 API —— 示例是独立 crate，因此同时是"公开面够不够用"的编译期检验）：默认模式做 parse → `to_binary` → `from_binary` → 逐函数 `Verifier` → 文本一致 → 字节幂等，并打印每文件的源码/字节流尺寸与 parse/encode/decode 耗时；`--check` 只读头部报 `check_binary_compat`（版本/producer/段表）；`--write <dir>` 落盘 `.fir`。新增 `benches/ir_binary.rs`（`cargo bench -p forge-ir --bench ir_binary`）量编解码吞吐。
  首次采集（本机 2026-09-19）：中等模块（884 B 文本 → 1,335 B 字节流）encode **21.9 µs / ~58 MiB·s⁻¹**、decode **77.2 µs / ~16.5 MiB·s⁻¹**（解码慢 ~3.5×：每次读都查剩余长度、重建三个 arena、回查 value kind 并重建 use-lists）；198 个正向语料的**源码文本 408,810 B → 字节流 241,440 B（0.59×，未压缩）**；v2 压缩后的对照数字见上一条。数字记入 `docs/performance/bench_baseline.md`，命令与口径见 `docs/reference/binary-format.md` §10/§11。

- **二进制往返的 fuzz 扩面：同一批随机模块走二进制（执行方案 §2.5 验收项）**：`tests/roundtrip_fuzz.rs` 的生成器与 `fuzz_roundtrip` 扩展为"文本往返 + 二进制往返"同批断言——默认 **10_000** 个随机模块（含第二固定种子 500 个）每个都额外做 `to_binary` → `from_binary`：①结构等价（模块/块/指令/终结符/常量/全局初始化字节，复用既有 `assert_modules_eq`/`assert_globals_eq`）；②解码后**文本打印与首次一致**；③`decode → encode` 与首次编码**逐字节相同**。本机实测 2 passed / 0 failed，整支 ~29 s（可 `FORGE_FUZZ_ITERS`/`FORGE_FUZZ_SEED` 覆盖复现）。

- **forge-ir 二进制序列化 B5（METADATA/GLOBALS/MODULE 段 + 语料端到端 + 格式规范文档）**：`binary/meta.rs`（metadata 节点表按 id 顺序 + 命名表按名排序）、`binary/globals.rs`（全局/别名/comdat + 模块三元组·源文件·模块 asm）。回放纪律：metadata 节点**按原样落位**（不用 `intern`——显式 `!N` 经 `insert_at` 预分配槽位、arena 允许同内容两条，`intern` 会折叠导致 id 整体错位；节点内 `Node(id)` 只校验 `< 节点总数`，**前向引用合法**）；全局/别名/comdat 一律经 `Module::{add_global, add_global_alias, add_comdat}` 回放，名字索引表随之重建、重名 fail-closed；附件 `MetadataId` 在解码末尾做悬空引用校验。
  **语料端到端**（`tests/binary_module.rs`）：LLVM `test/Assembler` 全部正向用例 parse → `to_binary` → `from_binary`，断言文本打印**逐字符一致** + `decode → encode` 逐字节幂等 —— **198/198 通过**；尺寸基线：合计 **241,440 B**、最大 **31,038 B**（`auto_upgrade_nvvm_intrinsics.ll`）、平均 **1,219 B**（作为后续压缩/演进的对照）。
  **顺带修掉一处既有不确定性**（语料往返测试抓到）：`MetadataStore::name_of` 原用 `HashMap::iter().find(...)` 反查，一个 id 被多个名字指向时（`!foo` 与 `!\23pragma` 内容相同 ⇒ 去重成一个节点）返回值随实例随机种子变化，display 输出随之不确定 → 改为取**字节序最小**者，并新增 `names_of` 返回全部别名；打印器"每节点只打一个名字"（多别名时丢名字）是**既有限制**，已记录在参考文档（当时的 §10，现 §12），当轮不动。
  规范落地：新增 `docs/reference/binary-format.md`（容器/段/原语/各段细节/确定性/版本/验证基线/已知限制），`docs/README.md` 索引；负向对照补：未知 metadata 节点/值 tag、越界节点引用、附件悬空、未知 comdat kind。
  实测（本机 2026-09-19）：workspace **1689 passed / 0 failed / 19 ignored**（B5 新增 10 例）；语料 198 正向 / 254 正确拒绝 / 0 误收；矩阵 x86 195/3/0、riscv64 131/67/0、arm64 23/175/0；fmt/clippy 两道门/release/doc/markdownlint 全干净。
- **forge-ir 二进制序列化 B4（FUNCS 段：函数 + `dfg` + layout）**：`binary/funcs.rs` 每个函数写名字/签名/调用约定/属性位/开放属性/符号（linkage·visibility·dll·section·comdat·TLS 模型·三个布尔）/`is_const`/personality/参数与返回属性/函数级 metadata/`debug_info`（`locations` + 函数名）/值名与块名/**每函数常量池**（与模块 `CONSTS` 共用同一份五通道编码器）/`dfg`/`layout`/入口块。
  `dfg` 侧：每个值写**显式 `ValueDef`**（`Inst(i,k)`/`Param(b,k)`/`AggConst`/`UndefNamed`）+ 类型；指令写 opcode **名字**（`ops.toml` 是单一事实源，读侧 `from_name` 解析，未知名即错——不受枚举声明顺序影响）、块、结果、操作数、11 种 immediates、`InstFlags`/`MemFlags`、调用点属性、metadata、`loc`、`isel_strategy`、墓碑位；块写参数类型/参数值/块内顺序/终结符。**use-lists 不落盘**，解码后按指令操作数重建（终结符指令在 `insts` 里，同样登记）。
  **解码后结构校验 `validate_dfg`**（"顺序即索引"不靠隐式假设）：值类型在界内、每条 `ValueDef` 回指一致（指令第 k 个结果 / 块第 k 个参数）、指令结果反向回指、操作数/结果/块参数类型/块顺序表/终结符/布局/入口块全部在界内——任一处不一致即 `Err`（带偏移）。解码写 arena 走新增的 `DataFlowGraph::{push_value_verbatim,push_inst_verbatim,push_block_verbatim}`（保 dense 索引、不触发 `make_*` 的附带登记），metadata 走既有唯一写入口 `attach_metadata`——两处都满足既有守卫（`dfg_privatization.rs` 的"arena 只能经受限 API 访问"、`metadata_single_write.rs` 的"附件只能经唯一写入口"），**未给守卫加白名单**。
  负向对照（全部实测为 `Err`）：未知 opcode 名、未知调用约定、value kind 与密集索引不一致、悬空操作数、未知 immediate tag、越界值类型、FUNCS 段体任意截断前缀（逐字节扫描不 panic）。
  实测（本机 2026-09-19）：workspace **1676 passed / 0 failed / 19 ignored**（B4 新增 11 例：往返/幂等/校验器/6 项负向/截断扫描/空模块段存在性）；往返用例同时断言**解码后函数过 `Verifier`**；语料 198 正向 / 254 正确拒绝 / 0 误收；矩阵 x86 195/3/0、riscv64 131/67/0、arm64 23/175/0（架构无关，未受影响）；fmt/clippy 两道门/release/doc 全干净。
- **forge-ir 二进制序列化 B3（CONSTS 段：五通道常量）**：`binary/consts.rs` 逐条按池内索引写 int/float/big/vector/aggregate 五通道，解码按**同序** `insert_*` 重建 ⇒ `ConstId`/`AggId` 的密集索引逐位不变（`bool_const` 的固定槽位也因此原样保留）。`Big` 三变体全覆盖：`Signed`/`Unsigned` 用 dashu 的小端补码字节，`Float` 写 `repr()` 的归一化有效数 + 指数 + **`Context::precision`**。
  **实现坑（负向用例抓出并修掉）**：`Big::Float` 只写 `(significand, exponent)` 会**丢精度上下文**——`Repr::into_parts()` 会把有效数归一化，`from_parts` 又把精度重置为"有效数位数"，于是 1.5 的 `prec: 53` 变成 `prec: 2`（值相同、精度不同，Debug 逐字符对比当场暴露）；改为写 `precision()` 并用 `Repr::new` + `Context::new` + `Real::from_repr` 重建后逐字符一致。
  fail-closed 负向对照（全部实测为 `Err`）：重复常量（`insert_*` 返回索引与位置不符）、未知 `Big` 变体 tag、未知向量端序 tag、聚合标量子越界、聚合**前向/自引用**（防环）、`i128`/`u128` varint 溢出（第 19 字节越界位、20 字节续位）。
  实测（本机 2026-09-19）：workspace **1668 passed / 0 failed / 19 ignored**；语料 198 正向 / 254 正确拒绝 / 0 误收；fmt `--check`、clippy 两道门、`cargo check --release --all-targets`、`cargo doc -D warnings` 全干净。

- **forge-ir 二进制序列化 B1+B2（IR bitcode v1：容器骨架 + 字符串表 + 类型段）**：新模块 `crates/foundation/forge-ir/src/binary/{mod,format,writer,reader,types}.rs`；公开 API `Module::{to_binary, to_binary_into, from_binary}` 与 `forge_ir::{IR_FORMAT_VERSION, SectionId, BinaryCompat, check_binary_compat}`，新错误变体 `IrError::BinaryDecode { offset, msg }`（`Display` 带偏移）。**不加 feature 门控**（零依赖、不依赖文本层 —— 对 `docs/plans/forge-ir-s8-design.md` §2.4 的修订）。
  格式 v1：`magic "FORGEIR\0"`(8B) + varint 版本 + producer（varint 长度 + UTF-8，**头部自包含**）+ varint 段数 + 段表 `[u8 id | varint 绝对偏移 | varint 长度]` + 段体（按 id 升序，无对齐无填充）。已落段：`0x00 COMPAT`（flags=0，未知位置位即错）、`0x01 STRINGS`（`num` + 长度表 + 字节拼接，**逐条往返、不预留空串槽**）、`0x02 TYPES`（12 种类型 tag 全覆盖 + 命名类型表（按名排序）+ 签名表 + `DataLayout`（三张对齐表与指针表按 key 排序））。
  fail-closed 纪律：`Cursor` 每次读先查剩余长度；长度字段先与剩余字节比对再分配（拒绝"声明 100 万段 / 4G 长度"）；varint 截断与溢出、未知段 id / 类型 tag / 调用约定 / mangling、段越界与重叠、重复段、重复字符串、非法 UTF-8、悬空 `TypeId`、预填充固定索引错位一律 `Err`（带偏移），**绝不 panic、绝不静默跳过**；`num = 0` 空池合法。
  **负向对照（真实发生并修掉，两条都记在提交信息里）**：① `BinaryCompat.producer` 原写 `String` ⇒ `open_set_boundary.rs::no_plain_string_fields_on_public_ir_surface` FAILED 并点名该行（v3 S5 的"公开面不得有裸 `String`"守卫当场生效）→ 改 `ImmStr`；② `to_binary_into` 追加语义下 `finish()` 的头部长度断言按绝对长度比较会误报 → 改成按增量比较。
  实测（本机 2026-09-19）：workspace **1658 passed / 0 failed / 19 ignored**（92 个测试目标；本片新增 42 例：`binary` 单测 16、类型库解码不变量单测 5、`tests/binary_format.rs` 21）；LLVM 语料 **198 正向 / 254 正确拒绝 / 0 误收**、语料往返幂等 189/189；JIT 矩阵 x86 **195/3/0**、riscv64 **131/67/0**、arm64 **23/175/0**；fmt `--check`、`clippy --workspace --exclude forge-rustc --all-targets --all-features -D warnings`、`clippy -p forge-ir --no-default-features --lib -D warnings`、`cargo check --release --all-targets`、`cargo doc -D warnings` 全干净。切片计划与逐片证据见
  `docs/plans/forge-ir-binary-serialization-plan.md`。

### Changed (2026-09-19)

- **forge-ir `src/` 目录归类（A0：A0a 文本层 + A0b 其余；可维护性重构）**：`src/` 顶层原来平铺 30 个 `.rs`（最大单文件 `text/parser/semantics.rs` 4,510 行），现按职能分组，顶层只剩 `lib.rs`/`error.rs`/`verify.rs`：`entity/{mod.rs←entity.rs, map.rs←entity_map.rs}`、`ir/{types,dfg,function,opcode,constant,immediate,terminator,builder,metadata,symbol,data_layout,type_rules,inst_flags,mem_flags,isel_strategy}.rs`、`analysis/{mod.rs←analysis.rs, alias, loop_info, use_list, debug_info}.rs`、`util/{big, imm_str, string_pool}.rs`、`text/{display.rs, parser/*}`。
  **公开面不变**：类型仍全部从 crate 根扁平导出（`forge_ir::Function`/`TypeId`/…），并**新增** `forge_ir::{EntityRef, EntitySet, PackedOption, PrimaryMap, SecondaryMap}`（此前只有 `forge_ir::entity_map::SecondaryMap`）；文本层规范路径 `forge_ir::text::{parse_module, parse_function, function_to_string}`。
  **无兼容层**（按"无需兼容旧版本结构"）：旧模块路径直接删除，不留别名/双入口；全仓 71 个 `.rs` + `grammar.lalrpop` 同提交改写（`crate::types::`→`crate::ir::types::` 等），`lalrpop_mod!` 生成路径改 `/text/parser/grammar.rs`，语法文件里 135 处 `crate::ir_parser::` 与 2 处 `crate::big::` 一并改（否则下次重建回退）。
  两处真实坑（详见 `docs/plans/forge-ir-v3-plan.md` §6 的 A0 条目）：① `lalrpop_mod!` 的 include 串与 `grammar.lalrpop` 的类型路径是两处独立事实；② 群组移动后文件内 `super::` 语义变化，按"模块→全路径"统一改 `crate::…`。组名取 `util` 而非 `support`（`forge_opt::support` 已占用，两者经 `code_forge::prelude` 的 glob 重导出会 `ambiguous glob re-exports`，实测改名后清零）。
  守卫同步（都是硬编码路径的静态守卫）：`read_path_budget.rs`（`src/ir/types.rs`）、`metadata_single_write.rs` 白名单（`ir/dfg.rs`/`ir/function.rs`）、`tombstone_semantics.rs`（`ir/dfg.rs`）、`forge-opt/tests/entity_tables.rs`（`src/ir/{function,dfg}.rs`）、`entity_privatization.rs`（`src/entity/mod.rs`）、`text_feature_gate.rs`（核心集合 = `src/**` 去掉 `src/text/**`，并新增"`src/display.rs`/`src/ir_parser.rs` 不得复活"断言）。**负向对照**：不改 `metadata_single_write.rs` 白名单 ⇒ 该用例 FAILED 并点名 `ir/dfg.rs` 三行；不改 `tombstone_semantics.rs` ⇒ FAILED 点名
  `ir/dfg.rs`；恢复后全绿（两次失败证明这些守卫真的在扫路径，而不是"没扫到所以绿"）。
  实测：workspace **1515 passed / 0 failed / 19 ignored**（与归类前同数）；LLVM 语料 **198 正向 / 254 正确拒绝 / 0 误收**、语料往返幂等 **189/189**（用例内 `assert_eq!` 精确断言）；三套 JIT 矩阵 x86 **195/3/0**、riscv64 **131/67/0**、arm64 **23/175/0**（均与归类前一致）；fmt `--check`、`clippy --workspace --exclude forge-rustc --all-targets --all-features -D warnings`、`clippy -p forge-ir --no-default-features --lib -D warnings`、`cargo check --release --all-targets`、`cargo doc -D warnings` 全干净。

- **S8 拍板：只做二进制序列化；新增执行方案文档**：`docs/plans/forge-ir-s8-design.md` 记录拍板结果（A 二进制**做**、B MemorySSA-lite 与 C crate 拆分**不做**，触发条件保留），并修订 §2.4 的 `binary` feature 决策（**不加 feature 门控**：零依赖零耦合，门控只制造"只有开 feature 才编到"的验证盲区）。新增 `docs/plans/forge-ir-binary-serialization-plan.md`：格式 v1 的**字节级规范**（magic + varint 版本 + producer + 段表，`0x00 COMPAT`…`0x07 MODULE` 八段；LEB128/zigzag、字符串表、句柄 dense index、枚举显式判别值 + 穷举 match、`Big` 规范形式、函数段 `value_kinds` 两遍解码、`Cursor` fail-closed 与嵌套深度上限、`check_binary_compat` 版本策略、确定性约束），
  B1–B5 切片表（范围/产出/负向对照/估行），每片门禁命令与基线数字，风险对策表，外部参考（MLIR bytecode / LLVM BitCode / wasmtime 序列化），以及否决 `postcard`/`bincode`/`rkyv`/`serde` 的理由（零新依赖、偏移级 fail-closed、禁止 `HashMap` 遍历的确定性都是验收项）。

### Changed (2026-09-16)

- **读写交错函数的"先写后读"：读数段合并（forge-ir v3 S3 余项②）**：`operand_to_value` 原来每个 arm 各取一次类型锁（metadata 判定、位模式判定、浮点位宽、向量元素类型、zeroinit 尺寸、const-expr 类型/尺寸，共 9 处）；现在在 `to_type`（**写**）之后取**一次**快照，把各 arm 需要的类型事实读进 `#[derive(Clone, Copy)] OperandTyFacts`，快照**不跨越**会 `strings.intern` 的 arm（跨越会触发整表 COW 克隆）。顺带合并 `build_inst` 的 `extractvalue` 索引链、`vconst` 的 `element_type`+`size_bytes`、`agg_const_from_operands` 的重复取锁。
  确定性计数实测（新守卫 `type_store_read_path.rs::operand_reads_share_one_snapshot`）：32 条指令的浮点位模式 161 → **129**、向量字面量 224 → **192**（各 −1 次/指令），公共路径 129 不变（无回归）；静态预算 `ir_parser/semantics.rs` 24 → **15**；`write_path_never_clones_in_real_workload` 仍 0 次整表克隆。负向验证：`UInt` arm 改回逐操作数取锁 ⇒ 用例 FAILED（161 vs 129）。
  实测：workspace 1515 passed / 0 failed / 19 ignored；x86 195/3/0、riscv64 131/67/0、arm64 23/175/0。

- **新增 S8 可选项设计方案（待拍板）**：`docs/plans/forge-ir-s8-design.md` 逐个给出二进制序列化 / MemorySSA-lite / crate 边界拆分的现状基线（forge-ir 生产代码 25,245 行、文本层占 9,995 行≈40%、10 类实体句柄、6 类常量通道…）、数据模型与格式/表示设计、分期与验证方案、成本与触发条件；建议 A（二进制）可做、B/C 不做，并列出需要拍板的四个问题。`docs/README.md` 索引与 v3 计划 S8 行已指向该文。

- **密集句柄表收口：全仓零 `HashMap<Value|Block|Inst>`（forge-ir v3 S2 余项④ 收官）**：把上批登记的 32 处余量全部迁完（`forge-opt` 的 `const_fold`/`sccp`/`gvn`/`gvn_pre`/`loop_unroll`/`licm`/`algebraic`/`lto`/`func_specialize`/`inline`），并清掉 `forge-ir`（`analysis` 的 `postorder_rank`/`preds_map`、`loop_info`、`ir_parser/semantics::per_pred`）与 `forge-codegen`（`agg_expand::AggSlots`、`compiler::rewrite`、`liverange`、`lowering::roots`）的同类表。
  为此给 `SecondaryMap` 补 `FromIterator<(K, V)>`（与 `HashMap::collect()` 同形），`Function::apply_replacements` 与 `DataFlowGraph::clone_inst` 的 `value_remap` 改收 `&mut SecondaryMap`。
  守卫由"预算表"升级为**零容忍扫描**：`forge-opt/tests/entity_tables.rs::no_dense_handle_hashmaps_in_forge_opt` + `forge-codegen/tests/entity_tables.rs::no_dense_handle_hashmaps_repo_wide`（扫 `crates/**` 与根 `src/`，跳过注释行、精确匹配键名以免误伤 forge-hir 的 `HashMap<BlockId, _>`）。负向验证：在 `loops/licm.rs` 插一处 `HashMap<Value, u64>` ⇒ 两个用例各 FAILED 并点名 `licm.rs:247: [Value]`。
  **唯一豁免**：`XReg`（`Eq`/`Hash` 含 class，按 index 密化会把同一 index 的不同类合并——语义变化），理由由 `xreg_is_index_plus_class_so_not_dense` 钉住。本切片未做 opt 侧 A/B（不宣称提速）。
  实测：workspace 1514 passed / 0 failed / 19 ignored；x86 195/3/0、riscv64 131/67/0、arm64 23/175/0。

- **优化 pass 侧密集句柄表 → `SecondaryMap`（forge-ir v3 S2 余项④ 第二批）**：`Value`/`Inst`/`Block` 都是 forge-ir 的密集句柄，以它们为键的表不该走 `HashMap`。本批迁移 `scalar/dead_code.rs::build_use_counts`、`scalar/copy_prop.rs::build_copy_map`、`scalar/cse.rs`/`scalar/gvn.rs` 的 `replacements`（含 `gvn_dfs` 传参）、`ipa/inline.rs::repl`，并把**核心 API** `Function::apply_replacements` 的参数由 `&HashMap<Value, Value>` 改为 `&SecondaryMap<Value, Value>`（4 处调用点同步）。
  剩余 32 处逐文件登记进新增的预算表 `crates/middle/forge-opt/tests/entity_tables.rs`（精确相等 + 每条写原因；迁移一批下调一批，新增一处即红），另有 `apply_replacements_takes_a_dense_map` 守卫核心 API 不回头。负向验证：在 `loops/licm.rs` 插一处 `HashMap<Value, u64>` ⇒ 预算用例 FAILED（实测 2 vs 预算 1）。
  本切片**未做** opt 侧 A/B 计时（不宣称 pass 提速），宣称的是结构一致性与"剩余量可数"。实测：workspace 1512 passed / 0 failed / 19 ignored；x86 195/3/0、riscv64 131/67/0、arm64 23/175/0。

- **代码生成侧密集句柄表 → `SecondaryMap`（forge-ir v3 S2 余项④ 第一批）**：`LowerCtx::{vreg_classes,vreg_types,vreg_widths}`（VReg 键）与 `CompileState::{value_to_xreg,block_map,alloca_offsets}`（Value/Block/Inst 键）由 `HashMap` 换成 forge-ir 的 `SecondaryMap`（下标即句柄，不再逐次 SipHash）；`TargetLowering::lower_terminator` 的映射参数、`forge-dsl` 模板签名与 `prelude` 同步（值语义键 `.get(&x)` → `.get(x)`，并去掉模板里为借用而写的 `let cond_val = &cond_val;`）。
  A/B 实测（demo8 夹具 `compile_raw`，release，500 次 × 7 轮取中位数）：64 条指令 **116.7µs → 103.3µs（−11.5%，区间不重叠）**；256 条指令 412.7µs → 400.5µs（−3.0%，尾部与噪声重叠）。首轮单次计时曾给出反向 +12%，重复测量后判定为噪声——文件头记下"单次计时不算证据"。
  **`XReg` 键表未迁（如实记录）**：`XReg { index, class }` 的 `Eq`/`Hash` 含 class，同一 index 不同类是不同键；按 index 密化会合并条目（语义变化），故 `xreg_types`/`precolored`/`assignments`/`spill_slots`/`intervals`/`active` 保持 `HashMap`，待拍板"class 是否属于键"。新增守卫 3 例（`entity_tables.rs`：编译期 VReg 表密集性、源码级 `CompileState` 表密集性、XReg 键含 class 的可执行断言）。
  实测：workspace 1510 passed / 0 failed / 19 ignored；x86 195/3/0、riscv64 131/67/0、arm64 23/175/0。

- **`LowerCtx` 带类型快照：生成物侧不再逐指令取锁（forge-ir v3 S3 余项①）**：`LowerCtx.type_ctx: Option<TypeContext>` → **`type_store: Option<TypeStoreRef>`**（lowering 入口取一次快照，lowering 期间类型表只读；`TypeStoreRef` 是 owned，解除了"守卫借着 `func`"的借用约束）。宿主的 `reg_class_for`/`mem_opsize_for`/`type_bits_of`/`type_bits_or_default` 与 `machine/pattern.rs` 的三个属性助手（参数改 `Option<&TypeStore>`）全部改读快照；`forge-dsl` 的 `quote!` 模板（`lowering.rs` 11 处 `tc.borrow()` + `integration.rs` 的逐属性一次性读封装）改读 `ctx.type_store`；`compiler.rs` 的宽向量可行性门由"每指令 × 每类型取锁"收成函数入口一次。
  取证：临时给 `TypeContext::borrow` 加 `#[track_caller]` 后按 `文件:行` 聚合——最大头是 `compiler.rs:1845`（每指令取锁）；改前编译一个函数取锁 **15 次（1 条指令）→ 252 次（80 条指令）**，改后 **恒为 10**。
  新增守卫 3 例（`crates/backend/forge-codegen/tests/lowering_read_path.rs`）：快照次数不随 IR 规模增长且 ≤16、编译期 0 次整表克隆、源码级"模板不得再出现 `type_ctx`/`.borrow()`"。`read_path_budget.rs` 预算：`forge-dsl/.../lowering.rs` 11 → 0（回潮守卫）、`pipeline/compiler.rs` 23 → 21。负向验证：宽向量门改回每指令取锁 ⇒ 规模用例 FAILED（14 vs 251）。
  实测：workspace 1507 passed / 0 failed / 19 ignored；x86 195/3/0、riscv64 131/67/0、arm64 23/175/0；LLVM 语料 198/254/0。

- **`TypeContext` 锁 → 快照：读路径不再持锁（forge-ir v3 S3 切片）**：`store: Arc<RwLock<TypeStore>>` → `Arc<RwLock<Arc<TypeStore>>>`——读写锁只护住那个 `Arc` 指针：`borrow()` 取锁只为克隆 `Arc` 后立刻放锁，返回新的 `TypeStoreRef`（`Deref<Target = TypeStore>`），读作用域不再持锁；`borrow_mut()` 返回 `TypeStoreMut`，`DerefMut` 走 `Arc::make_mut`（无快照存活时原地改，有快照存活时克隆整表）。同时**删除 `impl Deref for TypeContext { type Target = RwLock<TypeStore> }` 逃逸口**（把原始锁暴露给调用方，与"入口显式化"相反；全仓无使用者）。
  语义变化如实记录：读快照是不可变视图（拿快照后 intern，旧快照看不到新类型）；改前"读锁存活期间 `borrow_mut()`"会自锁死，现在合法并触发一次整表克隆——因此"读快照不跨 intern"从**死锁**强制变为**性能纪律**，由新增的 `debug_cow_clone_count`（仅 debug）钉住。
  新增守卫 2 例：`snapshot_is_isolated_from_later_interning`（快照存活时写入 ⇒ 恰好 1 次整表克隆 + 新旧快照可见性；该用例本身即"读快照与写可并存"的证据）、`write_path_never_clones_in_real_workload`（真实负载 0 次克隆）。负向验证：去掉 COW 计数 / 在 `int_ty` 里故意持快照跨 intern ⇒ 各 FAILED。**不宣称吞吐提升**（未做 A/B 基准），宣称的是结构性质与"真实负载 0 克隆"这一可测后果。
  实测：workspace 1504 passed / 0 failed / 19 ignored；x86 矩阵 195/3/0；riscv64 131/67/0；LLVM 语料 198/254/0、往返幂等 189/189。

- **splat 逐 lane 广播 + 向量元素类型名不再靠猜（forge-ir v3 S7 收官切片）**：①`ConstExpr::Splat` 在共享求值口 `const_expr_bytes` 里返回宽松 0（动它会让指令级常量池变序），故 `@s = global <4 x i32> splat (i32 7)` 的 lane 全是 0——现在**只在全局初值路径**广播（新增 `splat_init_bytes`：按向量元素类型逐 lane 重复内层标量；内层类型与元素类型不符即报错，非标量内层按既有 P1 占位零策略）。
  ②顺带挖出**向量元素类型名靠首字母猜**的静默损坏：`VecTy` 的 lexer 动作只做 `strip_prefix('i')/('b')/('f')`、其余落 `Ptr` ⇒ `float`→`f64`、`half`/`double`→`ptr`、`bfloat`→`i32`、`fp128`→`f64`（往返测试只比对 m1/m2、两边错得一样所以没暴露）。修：抽出唯一映射 `elem_ty_from_text`（`half`/`bfloat`→f16、`float`→f32、`double`→f64、`fp128`/`x86_fp80`/`ppc_fp128`→f128、`ptr`、`iN`/`bN`）。如实记录：`bfloat` 向量元素与 `half` 同为 `Float(16)`（标量 bfloat 另有 `BFloat`），`x86_fp80` 沿用既有约定映射 `f128`。
  新增守卫 2 例（`fidelity::splat_global_broadcasts_lane_value`、`fidelity::vector_element_type_names_are_not_guessed`，九种元素写法按打印形态钉住）；fuzz 生成器扩到 `double`/`half` 元素写法。负向验证：回退元素映射 / 回退 splat 广播 ⇒ 各 FAILED。
  实测：workspace 1502 passed / 0 failed / 19 ignored；LLVM 语料 198/254/0、往返幂等 189/189、结构化往返 198/0、10k fuzz 0 失败——与基线一致。**S7 全部切片落地。**

- **解析错误恢复：一次尽量报多处（forge-ir v3 S7 切片）**：改前解析失败只报**第一处**。现在首条诊断**原样**输出（格式与既有断言不变），随后从出错偏移起把**该行剩余内容删掉**（保留换行 ⇒ 行号不变）重新解析，新错误若在更后面就再报一条并标注"（跳过第 N 行出错处后继续检查）"，上限 **3 条**；剩余部分已能解析 / 位置不再前进 / 出错处就在行尾 / 已到输入末尾即停。因为只删"出错行剩余内容"，后续诊断回显的源码行与行号都仍是用户文件里的那一行。
  **恢复只用于报告**：任一条诊断存在就仍是 `Err`——LLVM 语料"误接受 0"判据不变（`recovery_never_accepts_invalid_source` 钉住）。**边界**：只覆盖语法层，语义阶段（未知 opcode/SSA 违规）仍是报第一处（`semantic_errors_still_report_first_only` 钉住现状）。
  新增守卫 5 例（`tests/parse_error_diagnostics.rs`）：多错误、上限、恢复不放行、无进展/EOF 单条、语义边界。负向验证：停用恢复循环 ⇒ 2 例 FAILED；上限改 8 ⇒ 上限用例 FAILED。
  实测：workspace 1500 passed / 0 failed / 19 ignored；LLVM 语料 198/254/0、往返幂等 189/189、结构化往返 198/0。

- **结构化 fuzz 扩面（forge-ir v3 S7 切片）**：`roundtrip_fuzz` 的全局生成器从 `global i32/i64 0|1|42` 扩到**保真矩阵**（i1/i5/i7/i24/i33/i128 十进制与十六进制、float/double/half/bfloat 的十进制与 `0x…` 位模式、`f0x…`、`zeroinitializer`、数组/结构体/`c"…"` 字符串聚合、向量字面量与 `splat`、常量表达式 init），断言侧新增**全局初始化字节对比**与**文本幂等**（`text1 == text2`）。10k 随机模块 0 失败。
  扩面立刻挖出三处"值悄悄丢"（往返只比对 m1/m2 两边时看不见）并修掉：①**向量字面量全局初值根本无法解析**（lexer 把 `<4 x i32> <i32 3, …>` 整段当一个 token，`TypeAndInit` 只拆 `zeroinitializer`）——新增 `GlobalInitVal::Vector` + `VecConstLit` 分支 + `vec_init_bytes`（逐 lane 按元素类型打包，lane 数/类别不符一律报错）；②**`parse_vec_lanes` 把每个 lane 都读成 0**（把整段 `i32 3` 喂给 `parse::<i64>()`，前缀必然失败）——改为先拆元素类型前缀、类别由前缀决定，lane 文本前缀也取自元素类型（`<4 x i16>` 不再打 `i32`）；③**`c"…"` 转义的收尾引号被吃掉**（`trim_end_matches('"')` 复数剥引号）：`c"T\22"` = `[84,34]` 打印成 `c"T\""` 后回读只剩 `[84]`——改为各剥一层
  `strip_prefix`/`strip_suffix`。
  回归钉子：`fidelity::vector_literal_global_keeps_lane_values`、`fidelity::malformed_vector_initializer_is_rejected`、`fidelity::escaped_trailing_quote_in_c_string_survives`。负向验证：回退 lane 前缀剥离 / 回退 `decode_c_string` ⇒ 2 例 FAILED，注掉 grammar 分支（强制重建生成物）⇒ 向量保真用例 FAILED。
  实测：workspace 1495 passed / 0 failed / 19 ignored；LLVM 语料 198/254/0、语料往返幂等 189/189、`display_llvm` 结构化往返 198/0 均与基线一致。

- **`features = ["text"]` 文本层门控（forge-ir v3 S7 切片）**：门控前 `--no-default-features` **根本编不过**——两个核心实体字段直接存解析层 AST（`GlobalVariable::init_expr: Option<ConstExpr>`、`GlobalAlias::{aliasee_ty, aliasee}`）。
  ①这两个字段的唯一读点是 `display`（原样还原文本）、唯一写点是语义层，故改为**不透明文本载荷**：语义层构建时渲染（`e.to_llvm_string()`；别名拼 `fmt_parsed_type(ty) + " " + expr`，`Void` 占位不带前缀），核心存 `init_expr_text: Option<ImmStr>` / `aliasee_text: ImmStr`（与既有 `ifunc_params` 同一约定），display 改为原样输出文本。
  ②`default = ["text"]`、`text = ["dep:logos", "dep:lalrpop-util"]`（两者 `optional = true`）；`lalrpop` 是 build-dependency 不可选 ⇒ `build.rs` 读 `CARGO_FEATURE_TEXT` 决定是否生成 LALRPOP 表，`ops.toml` 的指令元数据生成**不随 feature 关**（核心也读 `Opcode` 表）。
  ③`lib.rs` 的 `display`/`ir_parser` 两个模块加 `#[cfg(feature = "text")]`。
  ④新增 `tests/text_feature_gate.rs`（4 例源码级边界守卫：核心文件含注释在内不得出现 `ir_parser`/`crate::display`、模块声明恰好一次且紧跟 cfg、manifest 的 `dep:`+optional、build.rs 只门控 LALRPOP）；CI Clippy job 增 `cargo clippy -p forge-ir --no-default-features --lib -- -D warnings`（`--lib` 必须——`tests/*.rs` 本来就需要 `text`）。
  负向验证：核心文件引入 `ir_parser` / 去掉 cfg 属性 / `default = []` / 把元数据生成挪进门控块 ⇒ 四条守卫各自 FAILED，恢复后全绿。行为零变化：LLVM 语料 198/254/0、语料往返幂等 189/189、`display_llvm` 结构化往返全绿。
  实测：workspace 1492 passed / 0 failed / 19 ignored；无 feature 的 check/clippy（debug + release）0 错 0 警告。

- **IR 侧 span 贯穿：指令位置落进 `Instruction::loc`（forge-ir v3 S7 切片）**：改前实测整条解析链路上 `Instruction::loc` **恒为 `None`**——语法层不留字节区间（`ParsedBlock` 只有 `insts`），`FunctionBuilder::set_current_loc`/`emit1` 早已会把位置写进指令，却**没有任何调用者**，诊断只能指到函数级。
  现在三层各加一处真相：①语法层新增 `SpannedInst = <l: @L> <i: Inst> <r: @R>`（`grammar.lalrpop`），两个 `Block` 产生式改吃 `(<SpannedInst>)*`，由 `ast_items::split_spanned_insts` 拆出与 `insts` 平行的 `inst_spans`；
  ②`parse_to_ast` 用 `locate` 把字节偏移换算成 **1-based `(行, 列)`**（`ParsedBlock::inst_line_cols`；长度不等时视为无位置，不猜）；③`build_function` 发射每条指令前 `set_current_loc(...)`、循环尾复位，使指令内物化的常量随所属指令、循环后为终结符物化的合成常量**不继承**上一条位置。
  边界如实记录：`phi` 绑块参数、不发射指令，终结符走 `build_terminator`（不经 builder）⇒ 两者暂无位置；位置不进文本层（打印 → 重解析 → 再打印逐字节相同）。新增 `tests/source_span.rs`（4 例，含"列号必须来自源码"的非固定缩进夹具）；负向验证：删循环尾复位 ⇒ 2 例 FAILED，删发射前落位 ⇒ 3 例 FAILED，恢复后全绿。
  实测：workspace 1488 passed / 0 failed / 19 ignored；LLVM 语料 198/254/0、语料往返幂等 189/189 不变。

- **half/bfloat 位模式保真 + splat 文本往返：语料往返漂移 2 → 0（forge-ir v3 S7 切片）**：收掉最后两条漂移，189 个正向用例**全部幂等**（`KNOWN_DRIFT` 清空）。
  ① `float-literals.ll` 的 **f16/bfloat 值静默丢失**：`@2 = global half -qnan` 打印成 `0x7fc00000`（4 字节、无 `H` 前缀），回读按整数截断成 `0x0000`；根因是 `float_init_bytes` 没有 f16/bfloat 分支（落 `(f as f32)` 存 4 字节）。现在 `float_init_bytes` 增 f16（f32 → IEEE binary16，RN-even，含 denormal/Inf/NaN）与 bfloat（高 16 位 RN-even）；
  `fmt_global_init` 增 `0xH{:04x}`/`0xR{:04x}` 打印臂；grammar 的 `FloatHexLit`（`0xH...`/`f0x...`）由折成 `Float(0.0)` 改为 `Int(位模式)`；lexer 同时剥离 `f0x` 前缀（`llvm-dis` 的 half 输出形式）并**真正解码 C99 十六进制浮点**（`0x1.e3p-16`，此前恒 0.0）。实测 `+0x1.e3p-16` 打印 `0xH01e3`，与 LLVM 期望的 `f0x01e3` 一致。
  ② `constant-splat.ll` 的 **splat 常量折成空向量**（既丢值又多一个 `<1 x i32>` 类型前缀）：新增 `ConstExpr::Splat(ParsedType, Box<ConstExpr>)`，语法保留表达式、打印回 `splat (i32 7)`（必须带内层类型，否则自己解析不回来），并去掉向量常量打印里重复的类型前缀。**逐 lane 广播值仍未落地**——实测"取内层值"会让指令级常量池变序（`display_llvm` 结构化往返在 `constant-splat.ll` 上失败，`Iconst` ConstId 2 vs 0），故保持宽松 0、文本层原样往返。
  结果：语料往返漂移 **2 → 0**（幂等 189/189）；LLVM 语料正向 198/452、负向正确拒绝 254、误接受 0 不变；`display_llvm` 结构化往返全绿。新增回归用例 `fidelity::half_and_bfloat_globals_roundtrip`（5 组）与 `fidelity::splat_constant_roundtrips_without_type_prefix`；负向验证（回退位模式映射 + splat 内层类型 + 强制重建）⇒ 3 例 FAILED，恢复后全绿。
  实测：workspace 1484 passed / 0 failed / 19 ignored；x86 矩阵 195/3/0；riscv64 131/67/0。

- **聚合常量往返幂等：语料漂移 3 → 2（forge-ir v3 S7 切片）**：收掉 `unnamed.ll` 那条漂移，逐例打差异后定位两处根因，都在"聚合常量子元素"上：①浮点子元素被打印成整数字面量——`fmt_agg_scalar` 用 `format!("{}", f32)`，Rust 对 `4.0` 打 `"4"`，文本 `float 4` 回读成了 **i32** 常量（类型漂移）；现在用 `fmt_f32_literal`/`fmt_f64_literal` 保证带小数点或指数（`4` → `4.0`），非有限值给 hex 位模式。②聚合元素的零值被压成 i8 标量——`agg_const_from_operands` 对 `zeroinitializer`/`undef`/`poison`/`null` 元素一律 `insert_int(0, 8)`，元素类型是结构体时（`%1 zeroinitializer`）文本成了 `i8 0`，与聚合类型不符；现在新增 `zero_agg_child`：标量 → 零标量，**结构体/数组 → 递归零聚合**（`%1
  { i32 0 }`）。③顺带修整数子元素：用**元素类型**位宽打印，而不是常量池里记的宽度。
  结果：`unnamed.ll` 转幂等，往返漂移 **3 → 2**（余：`constant-splat.ll` 的 splat 展开与类型前缀、`float-literals.ll` 的 f16 hex 形态），幂等 **187/189**；LLVM 语料正向 **198/452**、负向正确拒绝 254、误接受 0 不变。新增回归用例 `fidelity::nested_zero_aggregate_roundtrips`；负向验证：临时关掉 `zero_agg_child` 的结构体分支 ⇒ 该用例 FAILED 且守卫报 `unnamed.ll` 为新漂移。
  实测：workspace 1482 passed / 0 failed / 19 ignored；x86 矩阵 195/3/0；riscv64 131/67/0。

- **命名元数据往返幂等：语料往返漂移 8 → 3（forge-ir v3 S7 切片）**：上一片留下的最大一类往返漂移（5 个 DI/`!named` 用例）定位到根因并修掉。逐例打差异后发现真相与最初猜测不同——命名元数据的**名字与内容都没丢**（`lookup_named` 两次都在），漂移来自 **id 空洞**与**文本顺序**：解析器为显式 `!N` 预留槽位时会把中间空洞补成空 tuple（文本里有 `!15` 与 `!19` ⇒ 16..18 成为 `!{}`），这些空洞被打印后二次解析成了"显式定义"，命名节点 id 整体后移；同时打印顺序把命名节点按 store id 内联在数字节点之间，于是文本与 id 绑定。
  现在：①`MetadataNode` 增加 **`Placeholder`** 变体，`MetadataStore::insert_at` 的空洞填充改用它——与用户写明的空 tuple（`!0 = !{}`）区分开；②display **先输出命名 metadata**（LLVM 风格，文本与 id 无关），再按 id 输出数字节点并**跳过无人引用的 `Placeholder`**（被引用的仍照打，否则 reparse 会引用未定义的 `!N`）；③`!tbaa` 形状检查对 `Placeholder` 保持宽松（引用未定义节点时无从校验；实测不放宽会让语料正向收敛数 198 → 197）。
  结果：往返幂等 **186/189**（此前 181），漂移 **8 → 3**；LLVM 语料正向 **198/452**、负向正确拒绝 254、误接受 0——均与基线一致。`tests/corpus_roundtrip.rs` 的 `KNOWN_DRIFT` 收窄到 3 条、计数同步更新；负向验证：关掉"跳过空洞占位" ⇒ 5 个 DI 用例重新漂移、守卫 FAILED；删掉 `KNOWN_DRIFT` 一条 ⇒ 守卫点名"新漂移" FAILED。
  实测：workspace 1481 passed / 0 failed / 19 ignored；x86 矩阵 195/3/0；riscv64 131/67/0。

- **语料往返断言扩面 + 两类保真缺陷修复（forge-ir v3 S7 切片）**：把"往返断言"落到真实语料——LLVM 官方 test/Assembler 的 189 个正向用例必须 `parse → print → parse → print` **幂等**（打印机做规范化，首轮不必等于原文，但规范化必须收敛）。实测：189 个 reparse 全部成功，但 9 个不幂等，逐例打差异后定位三类根因并修掉两类：
  ① **非字节位宽整数常量**：`@g = global i5 7` 打成 `0x07000000`、reparse 后又变 `0x00000007`——`int_init_bytes` 对非 8/16/32/64 位宽落 `(v as u32)` 存了 4 字节，打印端又把 LE 字节当十六进制原样输出；现在按 `ceil(bits/8)` 字节存储、打印端新增非字节位宽臂（LE 解码后十进制，`i5 7`）。
  ② **浮点类型上的整数字面量 = 位模式**：`global double 0x7FF0000000000000` 本应是 `+inf`，实测静默变成 `0.0`（`0x7FEFFFFFFFFFFFFF` 变 `0xffffffff`）——同一个 `_ => 32 位` 兜底把 64 位位模式截断；现在浮点类型按位模式编码（16/32/64），并把 f32/f64 的**大整数**打印改走 `0x` 位模式（`f64::MAX` 原先打 100+ 位十进制，解析端按整数读溢出 ⇒ 往返变全 1 位模式）。
  漂移 9 → **8**，幂等 180 → **181**。新增 `tests/corpus_roundtrip.rs`（3 例）：reparse 必须全成功、漂移必须恰好等于 `KNOWN_DRIFT`（8 条逐条记原因、必须被命中）、正向可 parse 数（189）与幂等数（181）精确相等；另含两类修复的回归用例。负向验证：删掉 `KNOWN_DRIFT` 一条 ⇒ 守卫点名"新漂移" FAILED；关掉 64 位浮点位模式分支 ⇒ 回归用例 FAILED。
  实测：workspace 1481 passed / 0 failed / 19 ignored；x86 矩阵 195/3/0；riscv64 131/67/0。仍在 `KNOWN_DRIFT` 的三类（命名元数据回读丢名字/列表、splat 向量全局多打类型前缀、聚合常量元素类型丢失）已有实测记录，留待后续切片。

- **解析错误诊断可用（forge-ir v3 S7 文本层切片）**：`ir_parser::parse_to_ast` 此前把 lalrpop 的原始错误 `format!("{:?}", e)` 直接当消息，实测输出是 `UnrecognizedToken { token: (17, Ident("entry"), 22), expected: ["Target", "VoidTy", … 共 88 项 …] }`——**没有行列、没有出错处的源码行**，还把 88 个文法内部记号名（`VconstOp`/`UselistorderBbKw` 这类）倒给用户。
  现在 `parse_module` 的语法错误是：`解析错误 2:1：非预期 entry` + 回显该行源码 + 插入符 `^` + 期望集合收敛到 8 项（`…（共 N 个）`），内部记号名做用户化映射（`RBrace`→`}`、`IntTy`→`iN`、`VecTy`→`<N x T>`、`*Kw` 去后缀小写、`IntLit`→`整数常量`、`LocalId`→`%局部名`）；EOF 报"输入在结构未结束时结束"，非法字符报"出现无法识别的字符"；坏偏移/非字符边界一律回退（诊断路径不 panic）。顺带把 `LexError` 改成 `BadChar { offset }` / `Rejected` 两态——词法错误此前丢掉了 logos 给出的位置。
  新增 `tests/parse_error_diagnostics.rs`（4 例）；负向验证（改回 `{:?}`）⇒ 4 例全 FAILED，恢复后全绿。实测：workspace 1478 passed / 0 failed / 19 ignored；x86 矩阵 195/3/0；riscv64 131/67/0；LLVM 语料正/负向断言不变。

- **读路径纪律批量落地 + 静态预算守卫（forge-ir v3 S3 切片）**：把"入口取一次读锁、把 `&TypeStore` 显式传下去"铺到剩余的纯读面。先按"函数体内是否有 `borrow_mut()` / 是否显式 `drop(guard)`"分类——`RwLock` 不可重入，读写交错的函数持有长读锁会自锁死，而这些函数当初写 `drop(ts)` 正是为了在读段之间放锁，属"已正确但不能改成长锁"，因此只迁移纯读函数：
  `pipeline/compiler.rs` `borrow()` **30 → 23**（7 个 `expand_*`/`memoryize_from_segs` 入口各一次）；`ir_parser/semantics.rs` **32 → 24**（6 个纯读助手 `pack_*_init`/`int_init_bytes`/`float_init_bytes`/`agg_const_from_operands`/`encode_lanes_to_bytes` 改 `&TypeStore`）；`types.rs` 13 处是 `TypeContext` 一次性查询封装（保留，新增用例钉住"每次调用恰好 1 次读锁、intern 不取读锁"）。
  `func: &mut Function` 的函数里不能用 `func.types.borrow()` 提升（守卫借着 `func` ⇒ 后续 `&mut func` 报 E0502），改用 `let types_ctx = func.types.clone();`（Arc，O(1)）再取锁。
  **实测发现**：`forge-dsl/src/v12/codegen/lowering.rs` 那 11 处 `borrow()` **不是普通代码，而是 `quote! { ... }` 模板文本**（生成的 lowering 在 codegen 期 `tc.borrow()`）——迁移它要同时改生成物与 `LowerCtx` 形态，单列为余项，本切片只用预算锁住。
  新增 `tests/read_path_budget.rs`（2 例）：逐文件把 `borrow()` 处数钉成**精确相等**的预算（每条写清"为什么还剩这些"，并断言文件存在防腐烂）；负向验证（往 `compiler.rs` 插一行 `borrow()`）实测 24 ≠ 23 ⇒ FAILED。
  实测：workspace 1474 passed / 0 failed / 19 ignored；x86 矩阵 195/3/0；riscv64 131/67/0。

- **读路径纪律落到校验器（forge-ir v3 S3 切片）**：`verify.rs` 此前有 13 处 `borrow()`——`check_conversion`/`check_immediates`/`check_gep_indices`/`check_operand_types` 等在**每条指令上各取一次读锁**（`check_conversion` 里 `scalar_bits`/`vector_bits` 两个闭包还各自取锁），取锁次数与指令数成正比。
  现在 `verify()` 取**一次**读锁（`ctx` 先 `clone`（Arc，O(1)）再 `borrow()`，守卫借用局部而非 `self`，因此仍可调 `&mut self` 检查函数），`&TypeStore` 贯穿 `check_types` → `check_operand_types` → `check_conversion`、`check_type_refs`、`check_immediates` → `check_gep_indices`；`is_pointer_ty`/`scalar_bits`/`class_of`/`class_matches` 改为接收 `Option<&TypeStore>`。实测 `verify.rs` 的 `borrow()` **13 → 1**。
  新增守卫 `verifier_read_locks_do_not_scale_with_ir_size`：1 条指令与 80 条指令的函数取锁次数必须相等（实测均 3 = 入口 1 + `Function` 内部查询 2）；负向验证（逐指令循环里插一行 `clone`+`borrow()`）实测 5 vs 84 ⇒ FAILED，删除后全绿。顺带实测到：这类退化**多数情况下连编译都过不了**——守卫借着 `self.ctx` 时不能再对 `self` 取 `&mut`（E0502），借用检查器已经封死"守卫跨 `&mut self` 调用"。
  实测：workspace 1471 passed / 0 failed / 19 ignored；x86 矩阵 195/3/0；riscv64 131/67/0。

- **类型读路径纪律：`&TypeStore` 显式传参（forge-ir v3 S3 切片，display 落地）**：`TypeContext::borrow()` 每次都是一次 `RwLock` 读锁获取，而 `RwLock` **不可重入**（读锁存活期间再 `borrow_mut()` intern 新类型 = 自锁死）；`display.rs` 此前有 **51 处** `borrow()`（`value_as_literal` 每值一次、`NameResolver::new` 每块/值各一次），打印一个模块的取锁次数与函数体大小成正比。
  现在 display 全量改为"**入口取一次锁、把 `&TypeStore` 显式传下去**"：`FunctionDisplay`/`BlockDisplay`/`InstDisplay`/`TerminatorDisplay`/`NameResolver`/`value_as_literal`/`fmt_phi_value` 的参数与字段改为 `&TypeStore`，`Display for Module` 与 `function_to_string` 各取一次（布局也读已取到的 store，避免 `Module::data_layout()` 再取一次）——非测试路径 `borrow()` **51 → 2**（这 2 处就是入口本身）。
  `TypeContext` 新增**仅 `debug_assertions`** 的读锁计数（`debug_read_count`/`debug_reset_read_count`；release 下字段与方法都不存在，零开销），并新增 `tests/type_store_read_path.rs`（3 例）钉住"打印一个模块/一个函数各只取 1 次读锁、重复打印 3 次 = 3 次"。负向验证：在 `Module::fmt` 临时加一行 `borrow()` ⇒ 计数 2、守卫 FAILED；删掉后全绿。
  实测：workspace 1470 passed / 0 failed / 19 ignored；x86 矩阵 195/3/0；riscv64 131/67/0。

- **坏 IR 的越界 `TypeId`/`SigRef` 变成诊断，不再 panic（forge-ir v3 S3 切片）**：`TypeStore::get`/`get_signature` 是 fail-closed 索引（`entries[id]`/`signatures[sr]`），而 IR 里的类型句柄是**数据**。临时探针实测两条崩溃路径：越界 `SigRef` 走校验器 ⇒ `signatures[9999]` `index out of bounds`；越界 `TypeId` 走 **display** ⇒ `entries[9999]` 越界（"把坏 IR 打印出来给人看"这个最需要诊断的时刻反而崩了）。
  现在：`TypeStore` 增加容忍坏 IR 的读取口 `entry_opt`/`signature_opt`/`contains_type`（`get`/`get_signature` 保持 fail-closed 并注明是良好 IR 的快路径）；校验器新增前置自检 `check_type_refs`（扫签名句柄 + 签名参数/返回类型 + 全部值类型 + 块参数类型），越界即报新错误码 `BadTypeId`/`BadSigRef` 并跳过后续类型相关检查（结构类检查照常跑完一起上报；错误码 31 → 33）；display 的 11 处类型查询改走 `entry_opt`，`fmt_llvm_type` 对越界 id 打印 `<bad-type:N>` 占位符，5 处 `size_bytes`/`alignment` 收进容忍助手（存储自身查询仍 fail-closed）。
  新增 `tests/bad_type_id_robustness.rs`（4 例）：越界值类型 / 越界签名返回类型 / 越界 `SigRef` 均返回诊断，坏 IR 打印出占位符；其中两条在改前实测 panic。实测：workspace 1467 passed / 0 failed / 19 ignored；x86 矩阵 195/3/0；riscv64 131/67/0。

- **`DataLayout` 单一数据源（forge-ir v3 S3 切片）**：`Module::set_data_layout` 此前是 `self.types = TypeContext::with_data_layout(..)`——**整体替换类型存储**。实测两条后果：①改布局前 intern 的签名/类型全部丢失（`get_signature` 直接在 fail-closed 分支 panic，文档里"必须在 `add_function` 之前调用（重建会清空已注册签名）"就是这条缺陷的自述）；②改布局前构建的 `Function` 持有旧存储 ⇒ 同一模块内模块侧与函数侧的指针宽度/大小/对齐各算各的（实测 `size_bytes(ptr)` 一侧 4、一侧 8）。
  现在：新增 `TypeStore::set_data_layout`（原地写那一个字段，无需失效任何缓存——布局派生结果都是查询期现算，`TypeKey` 不含宽度/对齐）；`Module::set_data_layout` 改为原地更新共享存储，**删除 `Module.data_layout` 副本字段**（唯一副本在存储里）并新增 `Module::data_layout()`，"必须在 `add_function` 之前调用"的约束消失；删除 `TypeContext::with_data_layout`（无调用方）；迁移 3 处字段读取与解析器注释。新增 `tests/data_layout_single_source.rs`（4 例，其中 2 例在实现前实测 FAILED：签名丢失 panic、指针宽度 8 ≠ 4）。
  实测：workspace 1463 passed / 0 failed / 19 ignored；x86 矩阵 195/3/0；riscv64 131/67/0。

- **`AnalysisManager`：惰性分析缓存改为"修订号自校验 + `Arc` 快照"（forge-ir v3 S6 切片，S6 最后一项余项）**：`Function` 上的 `OnceLock` 缓存（前驱/后继/支配树/循环森林）换成 `AnalysisSlot<T>`（`RwLock<Option<(修订号, Arc<T>)>>`），由新类型 `AnalysisManager` 拥有。修订号 = `(DFG 结构修订号, 显式失效次数)`；结构修订号（`DataFlowGraph::cfg_revision`）在**三条改 CFG 的路上**自动前进——块增删、终结符写入、**终结符被墓碑化**。读到过期修订即重算并返回当前快照 ⇒ **陈旧分析结果不可能被读到**，`invalidate_analysis()` 降级为"少算一次"的优化，正确性不再依赖调用方记得失效。
  实测出的两条旧漏洞：①`tombstone_inst` 不失效缓存，而块内顺序表不含终结符 ⇒ "就地墓碑化终结符"是绕过 `set_terminator` 的改图路，读者会拿到改图前的后继；②`func.dfg.make_block()`/`dfg.remove_block()` 直接改结构，没有 `&mut Function` 可用来失效。
  访问器返回 `Arc` 快照而非 `&T`："取一份分析 → 改 CFG → 再用"不再出现同一次使用期内前后不一致（旧引用语义会指向被就地改写的缓存）。**`forge-opt`/`forge-codegen` 零改动**（`Arc` 的 Deref 让既有调用与 `&SecondaryMap` 形参原样可用；13 处 `.clone()` 语义不变，从深拷贝变成 `Arc` 克隆）。
  校验器的 `AnalysisCacheStale` 检查与错误码随旧语义一并删除（错误码 32 → 31，无过渡层）；`tests/analysis_cache.rs` 重写为 8 例（写入口后必须看到新图、快照在改图后恒定、墓碑化终结符、绕过 `Function` 直接建块、`kill_inst` 终结符、`retarget_terminator`、反面"不改控制流的写入必须复用缓存"（`Arc::ptr_eq`）、显式失效仍可用）；两条负向探针各验一次（关掉对应 bump ⇒ 对应测试 FAILED，恢复后全绿）。
  实测：workspace 1459 passed / 0 failed / 19 ignored；x86 矩阵 195/3/0；riscv64 131/67/0。

- **错误码 × 回归测试对账守卫（forge-ir v3 S6 切片）**：`verify.rs` 的 `VerifyError` 现为 **32 个变体**，而 `tests/verify_negative.rs` 的文件头一直写着"27 个错误码全部有回归测试"——错误码 30 → 32 的两片都没人回写，属"文档声称 vs 实测"漂移。
  逐变体实测引用数后补齐 4 例：`OperandCountMismatch`（操作数个数不符）、`MultipleEntryBlocks`（多入口块，入口自带参数）、`DominanceViolation`（定义不支配使用）、`MissingTypeContext`（`TypeContext` 查不到类型）——这 4 个此前**既不在 `tests/*.rs` 出现、源码内 `VerifyError::<名>` 引用也不足 3 次**（crate 内单测同样没覆盖）。
  新增守卫 `every_verify_error_variant_has_a_regression_test`：解析 `src/verify.rs` 的枚举变体，要求每个变体在 `tests/*.rs` 里以 `VerifyError::<名>` 出现，或在同源码内出现 ≥3 次（构造点 + `Display` 臂 + crate 内单测），否则必须进本测试的 `ALLOWED`（当前为空；白名单条目必须被命中，否则报"条目已失效"）。守卫已用**负向探针**验证：临时插一个只有定义 + `Display` 臂的 `ProbeVariantNoTest` ⇒ 守卫 FAILED 并点名该变体，恢复后 green（改用测试侧改名做负向是无效的——只会得到 E0599 编译错，反而掩盖守卫是否生效）。文件头改为"对账交给守卫"，不再写会漂的计数。
  实测：`verify_negative` 44 例；workspace 1455 passed / 0 failed / 19 ignored；x86 矩阵 195/3/0；riscv64 131/67/0。

- **校验器 severity 分级 + 健壮性钉住（forge-ir v3 S6 切片）**：新增 `VerifySeverity::{Warning, Error}` 与 `VerifyError::{severity, is_error}`——分界线是"合法 IR 上的可疑现象"vs"不变量被破坏"，`UnreachableBlock`（不可达死块，LLVM 与本仓注释都视为合法，`check_reachability` 本就对它做了死 merge 豁免）降为唯一的建议级，其余保持违规级；`forge-opt` 的 `PassVerify::Error` 改为只在违规级失败、建议级记 `log::warn!`。
  新增 `tests/verifier_robustness.rs`（8 例）：越界跳转目标、越界操作数句柄、被跳转的未终止块、`remove_block` 后被指向的块、截断的 `switch` case 表、死块建议级、结构违规违规级、合法 IR 对照组——**实测 6 个坏 IR 夹具全部返回错误、无 panic**（含前两片新增的 `expect("投影命中…")` 路径）。顺带删掉 `forge-opt` 中一段自相矛盾的过时注释（上半段说严格校验"必然失败"，下半段已写"两处都已修…全绿"）。

- **校验器重算 CFG/支配树并比对 + 墓碑规范形态（forge-ir v3 S6 切片）**：新增 `VerifyError::{AnalysisCacheStale, TombstoneNotCanonical}`（错误码 30 → 32）。`Function::{predecessors, successors, dominator_tree}` 是 `OnceLock` 惰性缓存，此前**所有改控制流的写入口都不失效缓存**（全靠 `forge-opt` 8 处 pass 各自记得 `invalidate()`），改完 CFG 读到旧分析结果是静默错误且没有任何测试会失败。
  现在：①新增 `Function::invalidate_analysis()`（幂等 O(1)），`set_terminator`（所有按形式写入口的公共底层）/`retarget_terminator`/`kill_inst` 内部调用 ⇒ 写完自动失效；②校验器新增 `check_analysis_cache`：现场重算 successors/predecessors/支配树与**已初始化**缓存逐块比对，不一致报 `AnalysisCacheStale { what, block, cached, fresh }`（兜住绕过 `Function` 直改 `dfg` 的漏网）；③新增 `check_tombstones`：`is_tombstone()` 为真者不得仍带 operands/immediates/metadata/param_attrs/isel_strategy，否则报 `TombstoneNotCanonical`。实现中发现 `dfg.insts()` **会跳过墓碑**，校验器看不到它们——补 crate 内 `all_insts()`（原始 arena 迭代）。
  守卫 `tests/analysis_cache.rs`（4 例）+ `verify.rs` 内单测（伪墓碑上报；crate 外造不出伪墓碑，字段私有）。

- **墓碑语义显式化：`Instruction::is_tombstone()` 成为唯一判据（forge-ir v3 S2 切片）**：删除是"标墓碑"而非回收槽位，但"是不是墓碑"此前靠 `opcode == Nop` 猜——而 `Opcode::Nop` **是合法指令**（`FunctionBuilder::nop()` 发一条进 `inst_order`）。全仓实测三种答案（12 处 `matches!(opcode, Nop)`、4 处 `Nop && results.is_empty()`、1 处 `opcode == Nop`）：合法 Nop 被当成墓碑跳过，而带 results 的就地墓碑反被当成活指令。
  现在唯一事实源是 `Instruction.tombstone: bool`（私有）+ `is_tombstone()`，唯一实现是 `DataFlowGraph::tombstone_inst_low`（标标志 + Nop + 清 operands/immediates + **清附件** metadata/param_attrs/fn_attrs/isel_strategy，此前附件留在墓碑上）。两档语义共用它：**删除**（`remove_inst`/`kill_inst`，额外摘 `inst_order` 条目 + 清 results + 值 VOID）与**就地**（新增公开入口 `Function::tombstone_inst`，保留顺序表条目与 results，附 use-lists 重登记）——`forge-codegen` 8 处手写墓碑块全部改走它；4 处复合判据与 2 处"数墓碑"改 `is_tombstone()`，lowering 边界的 `matches!(opcode, Nop)` 按原意保留（任何 Nop 都不产生机器码）。
  守卫 `tests/tombstone_semantics.rs`（4 例：删除语义规范终态、**合法 `nop()` 不是墓碑**、就地墓碑化保留 results 但清附件、源码断言"`opcode = Opcode::Nop` 只许出现在 `dfg.rs`"，已用负向探针验证会失败）。

- **句柄字段私有化：10 个裸 u32 句柄不再能凭空构造（forge-ir v3 S2 切片）**：`Value`/`Inst`/`Block`/`TypeId`/`FuncRef`/`GlobalId`/`SigRef`/`AggId`/`VReg` 的字段由 `pub u32` 降为 `pub(crate)`，统一出入口 `::new(u32)` + `.index()`；`ConstId` 为 `::from_raw(u32)` + `.raw()`（打包值）+ 既有 `.index()`（低 30 位池内索引）+ `.tag()`。此前 crate 外可 `Value(999)` 造句柄（坏句柄从构造点泄漏到下游），也可 `v.0` 直读索引（句柄表示成了公开契约）。
  迁移面：本仓 178 + 45 处编译错误（按 `--message-format=json` 的 byte span 打补丁，4 轮收敛）；**DSL 生成器模板 32 处**（`forge-dsl/src/v12/codegen/{lowering,placeholder,machine,integration}.rs` 的 `quote!` 文本——生成物里的 `Block(...)`/`ConstId(...)`/`.0` 必须改生成器源）；`forge-rustc`（本机不可编译）走文本审计后定点修补。
  **踩坑**：机械规则"private field → `.index()`"对 `ConstId` 是错的——`ConstId::index()` 是低 30 位池内索引，而 `.0` 是含 tag 的打包值；整包测试立刻抓到 20 个 JIT 用例错值（float/vector 常量全错），改 `.raw()` 后恢复。守卫 `tests/entity_privatization.rs`（4 例：往返/Default/Display、`ConstId` 三者语义区分、源码断言句柄字段必须 `pub(crate)`，已用负向探针验证会失败）。

### Changed (2026-09-15)

- **`dfg` 私有化第三步：`blocks` arena 收口——三个 arena 至此全部私有（forge-ir v3 S5 第 6 项）**：`DataFlowGraph.blocks` 由 `pub` 降为 `pub(crate)`——读走 `block`（fail-closed：句柄不合法即 panic，与 `value_data`/`inst_data` 同契约）/ `block_opt`（容忍坏 IR）/ 新增 `block_data_iter`（无句柄迭代）/ 既有 `block_count`/`block_params`/`block_param_values`/`block_terminator`/`block_inst_iter`；写走 `block_mut`（就地编辑口），结构性增删仍只有 `make_block*`/`remove_block`。
  **契约**：`inst_order` 是块内指令顺序的唯一事实源（只在"插入/搬移"实现里改）；`params`/`param_values` 的改动须与 `Function::{add_block_param, remove_block_param}` 口径一致；写终结符走 `Function::{jump, branch, ret, …}`。新增 `block_data_iter` 的理由：原代码大量用 `blocks.iter().enumerate()` 取块序下标，而 `blocks()` 产出 `(Block, &BlockData)` 元组——无句柄迭代口让 23 处成为纯文本替换、语义零变化。
  迁移面实测：75 处 `dfg.blocks[..]` + 1 处裸 `dfg.blocks[..]` + 40 处 `len()` + 23 处 `iter()` + 8 处 `&mut …` + 1 处 `get(..)` + 2 处 `is_empty()` + 1 处整体借用，另含 `benches/compile_bench.rs`（首次把 benches 纳入迁移面）。顺带修 8 处 `&mut …inst_order` 前缀被吞、6 处多行 `.dfg\n.blocks\n.iter()` 链、2 处经读口 `inst_order.clear()`。守卫 `tests/dfg_privatization.rs` 扩到 13 例（block/block_opt/block_data_iter/blocks()/block_count 口径一致、越界 fail-closed、block_mut 就地编辑），源码断言扩成 `dfg.values`+`dfg.insts`+`dfg.blocks` 三字段（负向探针验证会失败）。

### Changed (2026-09-15)

- **`dfg` 私有化第二步：`insts` arena 收口 + `inst_mut` 就地编辑口（forge-ir v3 S5 第 5 项）**：`DataFlowGraph.insts` 由 `pub` 降为 `pub(crate)`——读走 `inst_data`（fail-closed：句柄不合法即 panic，与 `value_data`/`BlockData::terminator` 同契约）/ `inst_data_opt`（容忍坏 IR）/ 既有 `inst_opcode`/`inst_operands`/`inst_results`/`inst_block`/`insts()`/`inst_count`；写走 `inst_mut` / `inst_mut_opt`（`(dfg, Inst)` 寻址的就地编辑口），结构性增删仍只有 `make_inst*`/`remove_inst`。
  **契约**：安全字段是 `opcode`/`immediates`/`flags`/`mem_flags`/`param_attrs`/`fn_attrs`/`metadata`/`loc`/`isel_strategy`；`operands`/`results` 不在此列——改操作数走 `replace_all_uses`/`apply_replacements`，或"就地改写 + `refresh_inst_uses` 重登记"。另加 crate 内 `insts_iter_mut()`（`Function::apply_replacements` 用），使 `dfg.insts` 字段语法在 `src/` 里归零。
  迁移面实测 39 个文件：216 处索引 + 8 处裸 `dfg.insts[..]` + 11 处 `get(..)` + 3 处 `get_mut(..)` + 1 处 `iter()` + 25 处 `&mut …`；顺带修 8 处"经读口做写操作"（`set_isel_strategy`/`attach_metadata`/`flags |=`/`results.push`）、约 20 处 `&Inst` 接收者、4 处整数/`usize` 索引、6 处 `iter()`→`insts()` 的闭包解构、4 处多行 `.dfg\n.insts` 链。守卫 `tests/dfg_privatization.rs` 扩到 10 例（含"`inst_mut` 改操作数后必须 `refresh_inst_uses`，use 计数与 verifier 双重校验"），源码断言扩成 `dfg.values`+`dfg.insts` 双字段（负向探针验证会失败）。`blocks` arena 的收口是后续切片。

### Changed (2026-09-15)

- **`dfg` 私有化第一步：`values` arena 收口 + `set_value_type` 受限写入口（forge-ir v3 S5 第 4 项）**：`DataFlowGraph.values` 由 `pub` 降为 `pub(crate)`——读走 `value_data`（fail-closed：句柄不合法即 panic，与 `BlockData::terminator` 同契约）/ `value_data_opt`（容忍坏 IR）/ 既有 `value_def`/`value_type`/`values()`/`value_count`；写走唯一入口 `set_value_type(v, ty) -> bool`（越界不写返回 `false`），创建与墓碑化仍只在 `dfg.rs` 内。
  迁移面实测 38 处（31 处 `dfg.values[..]` + 7 处 `dfg.values.get(..)`，含 3 处写：`const_fold` 常量定宽、`gvn` 折叠类型、`algebraic` 结果类型），跨 forge-ir / forge-codegen / forge-opt 共 14 个文件；顺带修 4 处借用冲突——`value_data` 等借用整个 `dfg`，破坏了原先靠 `dfg.insts[..]` 与 `dfg.values[..]` 字段级不相交才成立的借用（gvn 的 `inst_ids` 改复制、algebraic/gvn 把读类型提到取 `&mut inst` 之前）。守卫 `tests/dfg_privatization.rs`（6 例，含"`src/` 里 `dfg.values` 字段访问为 0"的源码断言，已用负向探针验证会失败）。`insts`/`blocks` 两个 arena 的收口是后续切片。

### Changed (2026-09-15)

- **metadata 单写：5 个载体的附件各收成一个写入口，字段私有化（forge-ir v3 S5 第 3 项）**：附件 metadata（`(kind, node)` 对）此前有多条写路径且语义不一致——指令可经构造参数或 `inst.metadata.push(..)`、终结符要经 `term_metadata_mut` 逃逸出的 `&mut SmallVec`、函数是 `func.metadata.push(..)`、全局变量是 `gv.metadata = attached`（整表替换）。
  现统一为唯一入口：`Instruction::{metadata, attach_metadata}`、`DataFlowGraph::{term_metadata, attach_term_metadata}`（返回 `TermMetadataAttach::{Attached, NoTerminator, Unreachable}`，取代 `term_metadata_mut` 的逃逸可变引用）、`Function::{metadata, attach_metadata}`、`GlobalVariable::{metadata, attach_metadata}`、`GlobalAlias::{metadata, attach_metadata}`；创建期初始表仍走 `make_inst_with_meta_and_loc` 构造参数。
  追加语义统一（全局由"整表替换"改为逐条追加，解析期该表必为空 ⇒ 行为等价）；`unreachable` 不接受附件、未终止是坏 IR，两种失败原因由返回值分开报（解析器诊断文本未变）。跨 crate 读取面 `forge-opt` 的 inline/lto/func_specialize 三处改走 `inst.metadata()`。守卫 `tests/metadata_single_write.rs`（6 例，含文本层四载体端到端与"写入只许出现在唯一写入口实现体里"的源码断言，已用负向探针验证会失败）。

### Changed (2026-09-15)

- **开放集合划边界：删掉按架构名/OS 名查表的宿主查询，IR 公开面字符串统一 `ImmStr`（forge-ir v3 S5 第 2 项）**：`TargetTriple::{is_32bit, is_64bit, os_name}` **删除**——它们是"把写死的常量换成合法取值集合"的典型越界：架构名写进宿主白名单（`"x86_64" | "aarch64" | …`），表外架构（`loongarch64`、用户自定 ISA 名）两个都返回 `false`（既非 32 位也非 64 位的静默错误答案），且可能与 ISA 自己声明的 `addr_width` 矛盾；实测零生产调用点（只有其自身单测），故直接删除、不留兼容层。`TargetTriple` 只留 `parse`/字段/`Display`：四个分量原样保留原样往返，无归一化表。
  边界三分与规则（写进 crate README）：闭合集合（opcode/条件码/类别/效果/终结符种类，由 `ops.toml` 等单点闭死）、目标/ISA 数据（寄存器类与宽度、栈槽、指令字宽、pattern 名、isel 标签、triple 各段）、用户程序数据（名字、`section`、metadata 自定义 kind、`Immediate::String`）；**不得从后两类字符串反推第一类或任何数值**，数值一律来自 ISA 数据（指针宽度 = `DataLayout` 的 `p:<size>:<abi>`）。
  承载统一：`Module.source_filename: Option<String>` → `Option<ImmStr>`、`module_asm: Vec<String>` → `Vec<ImmStr>`（SSO + `Arc<str>` 共享，Clone O(1)），IR 公开面不再有裸 `String` 字段。守卫 `tests/open_set_boundary.rs`（4 例：未知架构名往返、宽度只跟布局数据走、`src/` 不得重现代码表、公开面不得有 `String` 字段），**已用负向探针验证守卫会失败**。

### Changed (2026-09-15)

- **`isel_strategy` 类型化 + 字段私有化（forge-ir v3 S5 第 1 项）**：`Instruction.isel_strategy` 从 `Option<&'static str>` 变为 `Option<IselStrategy>`（新模块 `src/isel_strategy.rs`）——**不是枚举**：标签名由目标 ISA 数据决定，forge-ir 不解析、不认识任何具体名字（无枚举/白名单/长度限制，名字内嵌的参数如 `"lea_sib:4"` 原样保留）。
  修掉两个真实缺陷：① `&'static str` 逼生产者把运行期名字 `Box::leak`（审计记录过那次泄漏修复），`IselStrategy::new` 现收任意 `&str`/`String`（短名内联零分配、长名 `Arc<str>` 共享、字面量 `from_static` 零拷贝）；② 裸字符串让"两套命名体系错配"（`lea_sib` vs `lea-merge-iadd-imul-4`）静默，类型化后比较必须先构造 `IselStrategy`，且**刻意不实现** `PartialEq<str>`/`Deref<Target = str>`/`Default`（空名 fail-closed，`None` 是"无标签"的唯一编码）。
  字段降为 `pub(crate)`，读写走 `Instruction::{isel_strategy, set_isel_strategy, clear_isel_strategy}`；5 处"保留全字段"复制点（`clone_inst`/lto/inline/func_specialize）改走写入口。**行为不变**：该通道当前无生产者（手写 `ext/pattern_isel.rs` 已随 ISA-DSL v15 删除），与 `[[pattern]]` 命名体系对接仍属 backlog #2。守卫 `tests/isel_strategy.rs`（5 例）。

### Changed (2026-09-15)

- **终结符诊断点名真实指令句柄（forge-ir v3 S6 子项，S4 主体后续）**：S4 让终结符成为指令之后，校验器中 5 个与终结符相关的错误变体仍只报块号或带伪造句柄——`BlockParamCountMismatch`/`ReturnTypeMismatch`/`ReturnValueTypeMismatch`/`InvalidTerminatorTarget`/`TerminatorDominanceViolation` 现各带 `inst: Inst` 并在 `Display` 里打印（`block {}: terminator inst {} …`）。
  构造点全部改取真实句柄（终结符用值循环绑定 `term_inst`、块参数检查绑定前驱块的终结符指令、`switch` case 重复与非法跳转目标各取 `block_terminator(block)`），两处 `Inst(u32::MAX)` 伪造占位删除。
  守卫 `tests/verify_negative.rs::terminator_diagnostics_carry_real_inst`（`ret` 计数不符点名真实 `ret` 指令；用公开写入口 `Function::jump` 改到不存在的块后，`InvalidTerminatorTarget.inst` 等于新终结符指令且不等于被墓碑化的旧句柄）。

### Changed (2026-09-15)

- **终结符并入指令流（forge-ir v3 S4 主体，破坏性）**：`Terminator` 枚举与 `UseSite` **删除**——终结符现在就是一条指令（新增 opcode `Ret`/`Jmp`/`Br`/`Switch`/`Unreachable`/`Invoke`/`Resume`，`category = "terminator"`），存 `DataFlowGraph::insts`，由 `BlockData.terminator: Option<Inst>` 引用，**不进 `inst_order`**（块内指令列表语义不变，约 40 处迭代点零改动）。块实参即这条指令的 operands、目标块与 `switch` case 表在它的 immediates 里（`Immediate::Block`/`Type` 早已存在，未新增 immediate 变体、未新增 arena；编码约定见 `ops.toml`「终结符」节，解码处自校验）。
  读经投影访问器（`term_kind`/`term_branch`/`term_jump`/`term_return_values`/`term_switch`（改为结构化 `SwitchView`）/`term_invoke`/`term_resume_value`/`term_is_unreachable`/`term_args_to`/`term_used_values`/`for_each_term_value`/`block_successors`），写经按形式写入口（`jump`/`branch`/`ret`/`switch`/`unreachable`/`invoke`/`resume`/`set_return_values`/`retarget_terminator`/`replace_terminator_args`）。
  **use-def 随之归一**：终结符用值就是普通操作数，`UseLists` 只剩"指令操作数"一条路径，`record_terminator`/`forget_terminator`/终结符专项校验与 `apply_replacements` 的终结符分支一并消失；`Function::replace_all_uses` 天然覆盖分支实参/`ret` 返回值。
  边界：`TargetLowering::lower_terminator` 形参由 `&Terminator` 改为 `(dfg, block)`，DSL 生成器按 `TermKind` 分派 —— **ISA TOML 与 `[[lowering]]` 规则未改动**。守卫：`terminator_is_an_inst_outside_inst_order`；`display_llvm` 往返比较改为比语义形态。

### Changed (2026-09-15)

- **crate 内读取面也全部走终结符投影（forge-ir v3 S4 子项 f）**：`use_list` 的 `record_terminator`/`remove_terminator` 不再收 `&Terminator`（改为按块 + `&DataFlowGraph` 经 `for_each_term_value` 读取）；
  `verify` 的全部变体 match 改走投影（`block_successors`/`term_branch`/`term_jump`/`term_switch`/`term_invoke`/`term_return_values`/`term_kind`）；`display` 的 `TerminatorDisplay` 改为"持有块 + 按 `term_kind` 分派 + 投影取载荷"，打印边界不再依赖存储形态；解析器的终结符元数据校验/附着改走新增的 `DataFlowGraph::{term_metadata, term_metadata_mut}`。有意的小行为变化：非法跳转目标现在**每个**各报一条 `InvalidTerminatorTarget`（旧代码对 `Switch` 只报第一条 case）。
  目的：S4 主体（终结符并入指令流、删 `Terminator`）只剩访问器/写入口的实现体与 `Terminator` 定义本身要改。forge-ir 内 `Terminator` 引用 241 → 205（其中 60 在定义/测试、40 在解析 AST）。

### Changed (2026-09-15)

- **读取面全部改走投影访问器，forge-opt 的终结符依赖清零（forge-ir v3 S4 子项 e）**：`crates/middle/forge-opt` 中所有 `match Terminator` 读取点（const_fold / dead_code / jump_thread / sccp / gvn_pre / tail_call / inline / lto / func_specialize / copy_prop / ind_var_simplify / block_param_coalesce / insert_preheader / loop_unroll）改为使用投影访问器（`term_branch`/`term_jump`/`term_return_values`/`term_switch`/`term_invoke`/`term_args_to`/`term_is_unreachable`/`for_each_term_value`/`block_successors`）；
  codegen 的 `ret` 读取与 invoke/resume 判别、forge-rustc 的诊断打印同步迁移。**该 crate 现已 0 处引用 `Terminator`**（`git grep -c` 实测）——为 S4 主体（终结符并入指令流、删 `Terminator`）把表示相关调用点收进访问器实现体。
  顺带修掉一处**遍历口径缺陷**：`const_fold::collect_uses` 的 `Switch` 分支漏算 `default_args`（旧手写 match 只收 discriminant + case args），改用规范序 `for_each_term_value` 后 default 实参也计入使用。

### Changed (2026-09-15)

- **终结符写入口按形式化 + 读取面投影化（forge-ir v3 S4 子项 d）**：`Function` 新增按形式命名的写入口 `jump`/`branch`/`ret`/`switch`/`unreachable`/`invoke`/`resume` 与 `set_return_values`/`retarget_terminator`/`replace_terminator_args`；`set_terminator(Terminator)` 与 `rewrite_terminator` 收为 `pub(crate)`，**crate 外已无法构造或就地改写 `Terminator`**（实测残留写点 = 0）。DFG 新增 `TermKind`
  判别与投影访问器（`term_branch`/`term_jump`/`term_return_values`/`term_switch`/`term_invoke`/`term_resume_value`/`term_is_unreachable`/`term_args_to`/`term_used_values`/`for_each_term_value`）。
  迁移 20 处构造点（builder 7 个方法委托、forge-opt 7 文件、codegen 3 处、解析器 phi 回填、loop_unroll 的 `clone_terminator` 改为按投影读+按形式写）；顺带修掉 codegen 中"手写逐块改 `inst.operands` 却不刷新 use-lists"的又一处 use-def 置空（改用 `replace_all_uses`）。
  目的：S4 主体（终结符并入指令流、删 `Terminator`）必须一次提交内完成，本步把表示相关的调用点收进少数访问器/写入口的实现体，使那次切换只需重写实现体。守卫 `tests/terminator_api.rs` 6 例（7 种形式的写↔投影读回、`TermKind` 判别一致、use-def 新鲜）。

### Changed (2026-09-15)

- **块级表示收口："未终止"成为显式状态（forge-ir v3 S4 子项 c）**：`BlockData` 的 `terminator: Terminator` + `has_terminator: bool` 合并为 `terminator: Option<Terminator>`——默认值 `Unreachable` 同时充当"未终止"占位与"显式 unreachable"、只能靠布尔位区分的歧义消失（按 `Default` 构造的块曾会把"漏写终结符"静默伪装成"显式 unreachable"）。
  读取口一分为二且**无静默回退**：`BlockData::terminator()` fail-closed（未终止即 panic，与 `Function::entry()` 同一契约），`BlockData::terminator_opt()` 供必须容忍坏 IR 的调用方（校验器/display/解析器元数据校验）；**CFG 构造**（`predecessors`/`successors`/支配树/循环森林）用 `terminator_opt()`——"未终止 ⇒ 无出边"是结构事实，且校验器本就要在坏 IR 上跑。
  两处兜底行为不变（`MissingTerminator` 与 `PathWithoutReturn` 仍分别上报）；新增守卫 `tests/verify_negative.rs::unterminated_block_is_explicit_state`（新块为 `None`、CFG 构造容忍不 panic、`terminator()` 读取必须 panic）。

### Changed (2026-09-15)

- **终结符写入面收口（forge-ir v3 S4 子项 b）**：`BlockData.{terminator,has_terminator}` 降为 `pub(crate)` 并新增只读访问器，crate 外（forge-opt / forge-codegen / forge-rustc / 集成测试，22 个文件）的读取统一改走访问器——**跨 crate 直接写终结符字段已不可能**，唯一写路径是 `Function::set_terminator`；`DataFlowGraph::block_terminator_mut` 同步降为 `pub(crate)`。
  新增 `Function::rewrite_terminator(block, f)`：就地改写终结符后自动重登记 use 项。
  **顺带修掉三处真实缺陷**：`forge-codegen` 的 `ret` 值就地改写（大聚合返回值展开、段值替换、`ret` 内 RAUW）此前从不刷新 use-def——在终结符进入 use-def 之后会留下陈旧 use 项，现全部改走 `rewrite_terminator`（`insert_preheader` 的 `retarget` 亦然）。
  可达性用探针实测：该路径在 `cargo test -p forge-codegen`（21 suites）与 `cargo test -p forge-tests --lib`（含 x86/riscv JIT 矩阵）下均不可达（仅 forge-rustc e2e 覆盖），故以 `tests/use_lists.rs::rewrite_terminator_keeps_use_def_fresh` 在本地钉住 API 契约。

### Fixed (2026-09-15)

- **终结符用值进入 use-def（forge-ir v3 S4 子项 a）**：
  `Use.user` 只能是 `Inst`，于是分支条件与 `then/else` 实参、`jump` 实参、`ret` 返回值、`switch` 判别值与 case 实参、`invoke`/`resume` 用值**完全不在 use-def 中**——`Function::replace_all_uses` 名不副实（pass 对分支实参做 RAUW 会留下悬空实参），而 `Verifier` 的 use-list 检查也看不见这一类（它同样只校验指令操作数）。
  现在 `Use { value, site: UseSite, operand_idx: u32 }`，`UseSite::{Inst(Inst), Term(Block)}`；`Terminator::for_each_value`/`for_each_value_mut` 是终结符用值的唯一遍历序（两份遍历由同一宏模板展开 ⇒ 记录序与改写序结构性一致，`used_values()` 亦由它实现）。
  `Function::set_terminator` 成为写终结符的唯一公开入口（精确摘除旧 use 项再登记新项；`DataFlowGraph::set_terminator` 降为 `pub(crate)`），新增 `refresh_terminator_uses` 供就地改写（`replace_args`/`remove_arg`）后重登记；15 处终结符写入点全部收口（builder 7、forge-opt 8），`apply_replacements` 删掉手写的 7 变体终结符 match 改用规范序遍历。
  `UseLists::verify` 改为双向（终结符用值必须在 use-lists 中；每条 `Term` 记录必须在 DFG 同一下标取到同值）⇒ 绕过 `set_terminator` 会立刻被 `PassVerify::Error` 抓到。守卫：`tests/use_lists.rs` 新增 4 例（RAUW 覆盖终结符实参、`set_terminator` 精确摘除、陈旧 use 项必报 `UseListInconsistency`）、`terminator.rs` 平坦序规格测试。

### Fixed (2026-09-14)

- **forge-ir S0 止血（12 项，来自全仓只读审计的实证缺陷）**：
  ① 常量池浮点只有位模式不记值宽 → f32 `1.5`（`0x3FC0_0000`）与位模式相同的 f64 **去重成同一个 `ConstId`**，而 phi 打印路径恒按 `f64::from_bits` 还原 → 输出错误的十进制值；现在位宽进去重键（`insert_float_typed`/`get_float_width`/`remap_from` 带位宽）且 phi 打印按值宽还原。
  ② `ConstantPool::default()` 绕过 `new()` 的 bool 预置槽而 `bool_const` 直接构造索引 → `Default` 现在是 `new()` 的等价实现。
  ③ `total_len()/is_empty()` 漏算聚合池。
  ④ `MetadataStore::insert_at` 与 `intern` 两条写入路径互不更新（同内容双 id、去重表指向被覆盖节点）+ `get` 越界 panic → 去重表自洽、`get` 返回 `Option`、解析器侧悬空引用点名报错。
  ⑤ `Verifier` 无 `TypeContext` 时 **8 类类型检查静默跳过**、`check_gep_indices` 对坏输入 panic → 新增 `VerifyError::MissingTypeContext`（fail-closed）与越界报错。
  ⑥ `Instruction.pos` 是只写不读的死字段（删/移指令后陈旧）→ 删除，块内顺序唯一事实源 = `BlockData.inst_order`。
  ⑦ 三处与代码不符的注释（`lib.rs`/`entity.rs` 声称已有 `PrimaryMap/SecondaryMap`、`function.rs` 指向不存在的 `Terminator::map_values`）。
  ⑧ 测试辅助 `all_opcodes()` 漏 10 个 opcode 变体（"系统性覆盖测试"并不系统）→ 改用新的 `Opcode::ALL`。
  ⑨ 覆盖矩阵用 `match op { … _ => … }` 兜底，34 个 opcode 静默无覆盖 → 逐条登记 `UNCOVERED_OPS`（带原因）+ 完整性守卫。
  ⑩ forge-opt `PassResult` 的删除/新增计数在汇总时**全部丢失**（只累加 `changed`）。
  ⑪ `UntilFixedPoint` 无迭代上限（pass 的 `changed` 恒真会挂死流水线）→ `MAX_FIXED_POINT_ROUNDS = 256` + 未收敛报错。
  ⑫ pass 后 IR 校验此前只 `log::warn`（坏 IR 继续流动）→ 策略化为 `PassVerify{Off,Warn,Error}`（S0 时默认 `Warn`；同日清偿 S6 欠账后 **`Error` 已是 `#[default]`**），并**修掉一处非法 IR 测试夹具**（`o2_pipeline_nop_residue` 的 `build_loop` 建 0 参数块却传 2 个实参）。
- **pass 后严格校验暴露的两笔欠账 → 同批清偿（v3 方案 S6 先行项，2026-09-14）**：
  `inline` 曾留下 use-list 不一致 + 返回类型不匹配（内联体建指令未登记，且**终结符操作数**没被 RAUW，`ret` 会返回 `VOID` 值 ⇒ 改用 `Function::apply_replacements`）；
  `gvn_pre`/`mem2reg` 插入指令的操作数未登记 use-lists，且 PRE 会在**不被操作数定义支配**的前驱插入（⇒ 新增 `operands_dominate` 守卫）。
  根因是 `UseLists::remove_inst` **按指令当前操作数**逐条删，改写**之后**调用就删不掉旧记录 → 新增 `UseLists::forget_inst`（按 `user == inst` 清扫）。
  现建指令一律走 `Function::make_inst*`（自动登记），就地改写走 `Function::refresh_inst_uses`；`PassVerify::Error` 成为 `#[default]`，守卫测试换成 `strict_verification_passes_for_all_pipelines`（O1/O2/O3 全绿）。详见 `docs/plans/forge-ir-v3-plan.md` §6 末。

### Changed (2026-09-14)

- **比较条件从 `Opcode` 变体载荷归一到 immediate 通道（forge-ir v3 S1 第二步）**：
  `Opcode::Icmp { cond }` / `Opcode::Fcmp { cond }` 的载荷删除，`Opcode` 至此 109 个变体全部无载荷；条件由 `Immediate::IntCC`/`FloatCC` 承载，`ops.toml` 用 `cond = "IntCC"|"FloatCC"` 声明该契约（生成物给出 `Opcode::cond_kind()`）。
  数值表示唯一化：`IntCC::code()`（1..=10）/`FloatCC::code()`（1..=16）+`from_code()`；宿主 lowering 把它们填进 `LowerCtx.current_immediates`，ISA TOML 的 `cond` 谓词读同一个数字——**forge-dsl 此前为每个 ISA 模块各生成一张 `icmp_id`/`fcmp_id` 数字映射表，现已删除**（x86 setcc 编码映射留在 ISA 侧，输入改为 canonical code）。
  fail-closed：`Verifier` 新增 `MissingCondImmediate`/`WrongCondImmediate`（条件缺失或类型不对直接报错，不按默认条件继续）；display 对坏 IR 打印 `icmp <cond?>` 而非静默省略；forge-codegen 的 immediate 折叠表删掉 `_ => 0` 兜底臂改为显式列出全部 `Immediate` 变体。
  **归一暴露并修掉两个真实缺陷**：① `ExprKey`（CSE/GVN/GVN-PRE 表达式的键）只含 `opcode + operands + ty`——条件在 opcode 载荷里时恰好够用，归一后 `icmp eq` 与 `icmp ne` 键相同会互相消除（错值级），现在 immediate 进键（回归测试 `p0_icmp_cond_distinct` 抓到的）；② `algebraic.rs` 的 `x cmp x → 1/0` 规则读的是 `Immediate::Int(cc)`，而条件当时在变体载荷上、immediates 恒空 ⇒ 该分支从未命中（死代码），归一后真正生效。

### Changed (2026-09-14)

- **LLVM 文本名表进 `ops.toml`；全部指令名查表改为生成 `match`（O(1)）**：
  `ir_parser/llvm_mapping.rs` 的正/反两张手写表（105+ 条 `match`）删除，改为每个 opcode 在 `ops.toml` 声明 `llvm = "<文本名>"`、`llvm_parse = false`（仅 display 用）、`llvm_alias = [...]`（旧名）。有损对从"读者自己比对两张表"变成**声明**：`Fload`/`Fstore` 复用 `load`/`store`、`Iconst`/`Fconst`/`Vconst` 常量内联、`Vadd`/`Vsub`/`Vmul`/`Vneg`/`Vabs`/`Vbitcast` 由标量名 + 向量类型精化、`Vsplit`/`Vconcat`/`Ftrunc` 解析器不接受、`CallIndirect` 复用 `call`、`Icmp`/`Fcmp` 需要条件（共 17 条 display-only）；别名 3 条（`callbr`/`ptrtoaddr`/`vextractelement`）。生成期断言变体名/助记符/**解析名集合**三者唯一。
  **查表一律 O(1)**：`from_name`/`from_mnemonic`/`from_llvm_name`/`from_cond_llvm_name`/`info()` 都是生成的 `match`（此前是 `ALL.iter().find` 线性扫 109 条）。
  本机 debug 实测（200k 轮 × 28 名 = 560 万次）：`from_llvm_name` 2770.5 → **201.2 ns**（13.8×）、`from_mnemonic` 2532.2 → **275.0 ns**（9.2×）、`from_name` 2806.0 → **509.1 ns**（5.5×）；计时工具 `forge-ir/tests/llvm_name_lookup_perf.rs` 默认 `#[ignore]` 留在仓库。
  生成器自身的构建期查表同步去掉 O(n²)/O(n·c)（改 `HashMap` O(1) 探测），**生成物 SHA256 前后完全一致**（`5A53A0F5…`）证明改写不改变行为；`semantics.rs` 的 DWARF 操作码表与 `forge-tests` 的清单腐烂检查也从线性扫描改为 `match`/`from_name`。
  **迁移保真证据**：独立脚本解析 git HEAD 两张表与生成物逐项比对 → `PARSE_NAMES=95 / OLD_PARSE_PAIRS=95 / OLD_SHOW_PAIRS=108`，**MISMATCHES=0**。

### Changed (2026-09-14)

- **verifier 的逐指令类型规则声明化（forge-ir v3 S1 收尾）**：
  `verify.rs` 的 `check_operand_types` 原本按 opcode 手写分组（`matches!` 大名单：binop 37 个、浮点 binop 8 个、`Fma`、`Select`、`Icmp`/`Fcmp`，再加 13 个转换指令的逐 opcode 分支）。
  现在**族名声明在 `ops.toml` 的 `type_rule`**，verifier 按族分派：12 个封闭族（`none`(48)/`binop_same`(37)/`convert`(13)/`load`(2)/`store`(2)/`same3`/`cmp_int`/`cmp_float`/`select`/`cmpxchg_pair`/`call`/`call_indirect`）。
  写成未实现的族名 → **构建期报错**；新增族名 → `verify.rs` 的穷举 match（无 `_` 臂）**编译失败**（双向 fail-closed）。
  13 条转换分支收敛成一张事实表 `convert = { src, dst, width }`（`int/float/ptr/any` × `widen/narrow/any/equal_bytes/equal_total_bits`），诊断文本由事实表拼出；形状类规则抽到新模块 `src/type_rules.rs` 的 `check_shape`（verifier 与 builder 共用一份实现，自带 4 个单测）。
  守卫测试 `type_rule_classification_is_complete` 断言每个 opcode 都有分类且各族计数钉住。
  **builder 侧不做声明化（实证否决）**：把同一函数挂到 `FunctionBuilder::emit_with_mem` 后 **5 个既有 builder 测试失败**——builder 刻意允许"混合宽度操作数 + 结果类型 upcast"（`iadd(i8, i64) → i64`），而 verifier 的 `BinopSame` 要求两操作数同类型；builder 是宽松构造层（类别维度由方法内 `assert!(t.is_int())` 把关，比 verifier 更严），verifier 是严格校验层，强行统一会破坏既有语义，故保留为可选工具函数并写明原因。

### Changed (2026-09-14)

- **`predecessors()`/`successors()` 迁到密集索引（forge-ir v3 方案 S2 第二切片）**：
  `Function` 的两处 `OnceLock<HashMap<Block, Vec<Block>>>` 改为 `OnceLock<SecondaryMap<Block, Vec<Block>>>`（`preds.entry(succ).or_default()` → `get_mut_or_default`）。
  这是**跨 crate 公开 API**：`forge-opt` 的 6 个 pass 文件（`gvn_pre`/`loop_unroll`/`insert_preheader`/`ind_var_simplify`/`block_param_coalesce`/`jump_thread`）、`forge-codegen` 的 lowering，以及 forge-ir 内的 `display.rs`/`function.rs`/`semantics.rs`/`verify.rs`/`loop_info.rs` 共 11 处调用点由 `.get(&block)` 改为 `.get(block)`。
  语义差异写进 doc：`predecessors()` 里**无前驱的块不出现**（entry），`successors()` **每个块都有条目**（无后继者空 vec）——与迁移前逐块 `insert` 行为一致。
  计量（同一 `git grep` 口径，`forge-ir/src` 全树）：S2 之前 **45 处** → 第一切片后 32 处 → 本切片后 **25 处**。门禁：clippy `-D warnings` 干净、workspace 1388 passed / 0 failed / 19 ignored（67 suites）、x86 矩阵 195/3/0、riscv64 131/67/0。

- **支配树字段密集化（forge-ir v3 方案 S2 第三切片）**：
  `analysis.rs` 的 `DominatorTree` 五个字段（`children`/`tin`/`tout`/`idom`/`depth`）由 `HashMap<Block, _>` 改为 `SecondaryMap`（含 `empty()`、C-H-K 迭代的 `idom` 局部表、`compute_children`/`compute_intervals` 的签名与返回类型）。
  支配树是 `dominates`/`idom`/`depth`/`ncd`/`children` 的底座（`loop_info`/`licm`/`gvn` 都在用），查询从"哈希 + 探测"变为一次 `Vec` 索引（`dominates` 一次查 4 张表）。
  计量（同一 `git grep` 口径，`forge-ir/src` 全树）：句柄键 `HashMap` S2 前 45 → 第二切片后 25 → **本切片后 10 处**（`SecondaryMap` 使用点 74 处）。
  顺带记录一个待补的易用性缺口：`for (child, &parent) in &secondary_map` 需要 `IntoIterator for &SecondaryMap`（暂未提供，本次改用 `iter()`）；门禁：clippy `-D warnings` 干净、workspace 1388 passed / 0 failed / 19 ignored（67 suites）、x86 矩阵 195/3/0、riscv64 131/67/0。

- **删除 `TypeId::bits()`/`try_bits()`，位宽改问类型事实 API（forge-ir v3 方案 S3 第二切片）**：
  旧视图有两条撒谎的默认值——`PTR` 恒 64（不查 `DataLayout`）、复合类型返回 0（与 void 不可区分）。
  按"无需兼容旧版本结构"直接删除，迁移 **76 处调用点**：`forge-ir`（verify 转换宽度规则、builder 的 `iconst` 常量位宽、entity 测试）、`forge-opt`（`const_fold` 19 处 + `algebraic`）、
  `forge-dsl`（5 处 **生成代码** —— 新增 `LowerCtx::type_bits_of`）、`forge-codegen`（`opsize_from_type`/`mem_opsize_from_type` 改实例方法并问 store、`reg_info` VEC 档位、`pattern.rs`/`compiler.rs`）。
  新增 `TypeStore::scalar_bits`（指针按 DataLayout）、`TypeContext::scalar_bits`、`TypeId::builtin_scalar_bits`/`builtin_vector_bits`。
  **两处行为修正**：32 位目标的指针 opsize 从 64 修正为 32；`const_fold` 对动态位宽标量（`i24` 等）改为不折叠（fail-closed 守卫，本模块无 store）。
  另：`TypeContext::borrow/borrow_mut` 锁中毒不再 panic（取回内部值），新增守卫 `tests/type_facts.rs`。

  实测：残留 `bits(`/`try_bits` 调用 0；workspace 1383 passed / 0 failed / 19 ignored（68 suites）；x86 矩阵 195/3/0；riscv64 131/67/0。

- **S4 前置清理：入口约定 fail-closed + 尾声哨兵具名**：
  ① 7 处 pass/分析（`analysis.rs`、`dead_code`/`gvn`/`gvn_pre`×2/`jump_thread`/`sccp`）与 `forge-codegen/pipeline/compiler.rs` 的 entry 参数重建此前写 `entry_block.unwrap_or(Block(0))` / "entry 块约定为索引 0"——静默回退会把"没设入口"伪装成"入口是 0 号块"，支配树/循环分析会据此算出看似合理但错误的结果；现在统一走新增的 `Function::entry()`（缺失即 panic，fail-closed），要"可能没有入口"语义的调用方直接读 `entry_block` 字段。
  ② 统一尾声标签此前是字面量 `Block(0xFFFFFFFD)`（`emission.rs` 两处 + reloc patcher 注释里的魔数）→ 具名为 `pipeline::emit::EPILOGUE_LABEL` 并写明"为什么是这个值、为什么不能改（定宽 ISA 把块号写进 label 位域，reloc patcher 依赖其只占低位）"，绑定前加 debug 断言（块数不得逼近哨兵）。

  实测：workspace 1383 passed / 0 failed / 19 ignored（68 suites）；x86 矩阵 195/3/0；riscv64 131/67/0；clippy `-D warnings` 干净。

- **`LabelRef` 取代机器层哨兵 `Block`（v3 方案 S4 子项）**：
  机器层 label 复用 IR 的 `Block` 句柄，但"统一尾声"不是 IR 块——此前用魔数 `Block(0xFFFFFFFD)` 表示（上一提交具名为 `EPILOGUE_LABEL`）。
  本轮引入 `pipeline::emit::LabelRef { Block(Block), External(ExternalLabel) }` 与 `ExternalLabel::{BASE, id()}`，把"真实块"与"机器层自造标签"写进类型；
  `LabelRef::id()`/`from_id()` 是**唯一的数字 ↔ 标签互转边界**（定宽 ISA 把 id 塞进 label 位域、变长走 reloc，编码器/patcher 仍按数字工作）。
  `CodeSink::{bind_label, use_label_at}` 收 `impl Into<LabelRef>`（块标签调用点零改动），`TargetFrameLowering::emit_epilogue_jump` 形参改为 `LabelRef`，
  DSL 生成器同步（`epilogue_block.id() as i64`、生成的 machine.rs 用 `LabelRef::from_id(rel as u32)` 还原）。
  编码 id 不变（尾声仍 `0xFFFF_FFFD`），`reloc_patcher` 位段重排语义与测试不受影响。

  实测：workspace 1383 passed / 0 failed / 19 ignored（68 suites）；x86 矩阵 195/3/0；riscv64 131/67/0；clippy `-D warnings` 干净。

### Added (2026-09-14)

- **实体容器与密集索引（forge-ir v3 方案 S2 第一切片）**：新模块 `src/entity_map.rs`（无新依赖，8 个单测）提供 `PrimaryMap`（`push` 分配句柄、下标即句柄、**刻意不支持删除**）、`SecondaryMap`（`Vec<Option<_>>`，"未设置"与"空值"可区分，`get_mut_or_default` 等价 `entry().or_default()`）、`EntitySet`（密集位图，O(1) 增删查）、`PackedOption`（句柄可空压缩：`Option<Value>` 8 字节 → **4 字节**，`u32::MAX` 为空哨兵），以及 `EntityRef` trait + `entity_ref_impls!` 宏（已为 10 个句柄类型实现：句柄 ↔ 密集下标）。`ListPool` 明确不做（本仓库列表用途都是短生命周期局部量）。
  同批把 forge-ir 内部的**句柄键表**迁到密集索引（`HashMap` → `SecondaryMap`）：`use_list.rs` 的 `uses`（最热路径：每建/删/改指令都碰）、`verify.rs` 的 `defined`（每次 `verify()` 重建）、`function.rs` 的 `value_names`/`block_names`、`display.rs` 的 `NameResolver`（`imm_str.md` 点名的热路径）、`alias.rs` 的 `memo`、`debug_info.rs` 的 `locations`、`loop_info.rs` 的 `depths`。
  **计量**：`forge-ir/src` 的句柄键 `HashMap` **45 → 31 处**（`git grep` 对比 HEAD；全仓基线 129 处 / 36 文件）；workspace 测试 1380 → **1388 passed**。
  S2 余项记在方案 §6：`predecessors()`/`successors()` 与支配树（公开 API，牵动 forge-opt/forge-codegen 15+ 调用点）、句柄字段私有化（`.0` 约 260 处）、墓碑语义显式化、两个下游 crate 内部的句柄键表。

### Changed (2026-09-14)

- **`ops.toml`：指令元数据单一事实源（forge-ir v3 方案 S1 第一步）**：
  crate 根新增 `crates/foundation/forge-ir/ops.toml`（109 条 `[[op]]`：变体名、助记符、分组、文档、值操作数个数、结果数、`may_ub`、`side_effect`、变体载荷），
  `build.rs` 扩容为"lalrpop + 读 `ops.toml` 生成 `$OUT_DIR/opcode_gen.rs`"，`src/opcode.rs` 以 `include!` 接入——**新增一个 opcode 从改 6 张手写表变成加一行 `[[op]]`**
  （枚举、`ALL`、`name()`、`mnemonic()`、`result_count()`、`expected_operand_count()`、`may_ub()`、`has_side_effect()` 全部由生成物投影；`Opcode::info()` 是无 `_` 兜底臂的生成 match，变体与表同源不可能漂移）。
  操作数元数建模为 `OperandArity::{Fixed(u8), Variadic}`，修掉老表"`0` 既表示无操作数又表示不检查"的语义混淆（`expected_operand_count()` 保持历史契约 `Variadic => 0`）。
  生成器 **fail-closed**：缺字段/类型不对/名字或助记符重复/未知载荷/载荷缺默认值一律构建期 `panic!`。
  **迁移保真证据**：一次性脚本把 git HEAD 的手写表与生成物**各自独立解析**后逐项比对 → `109 变体 × 6 属性，MISMATCHES=0`（`NEW_VARIADIC=6 / NEW_SIDE_EFFECT=9 / NEW_MAY_UB=15`）。门禁：workspace 1372 passed / 0 failed / 18 ignored（66 suites）、clippy `-D warnings` 干净。S1 余项（`Icmp`/`Fcmp` 载荷归一、LLVM 文本名表、verifier 规则与 builder 断言声明化）记在方案 §6 末。

- **`Function::make_inst` / `make_inst_with_meta_and_loc` / `refresh_inst_uses`（use-list 契约的公开入口）**：建指令即登记 use-lists；`refresh_inst_uses` 在就地改写操作数后重登记（对调用顺序不敏感）。配套 `UseLists::forget_inst(inst)`。回归守卫 `crates/foundation/forge-ir/tests/use_lists.rs`（3 个用例：改写后刷新一致、`forget_inst` 全删、`make_inst` 自动登记）。
- **codegen 侧 use-list 门禁**：`pipeline/compiler.rs` 的 47 处 `.dfg.make_inst*` 改走包装、13 处墓碑/Copy 块与 3 处操作数改写点改 `refresh_inst_uses`，聚合展开后加 **debug-only** 断言 `use_lists.verify(&dfg)`。此处**刻意只查 use-lists 而非完整 `Verifier`**：`expand_geps` 会生成类型自洽性不足的 IR（`%p = add i64 %prev, %t` 却声明 PTR 结果，源码原注释即"verify 不跑"），根因是 v12 尚无 `Ptrtoint`/`Inttoptr` 降级 ⇒ 类型化指针算术归 v3 的 S4/S5（范围写在注释里，不是静默容忍）。

- **`Opcode::ALL` / `Opcode::name()` / `from_name()` / `from_mnemonic()`（指令清单单一事实源的第一步）**：`Opcode::ALL` 是全部 **109** 个变体的权威清单；`name()` 是**无 `_` 兜底臂的穷举 match**（新增变体不同步更新即编译失败）。新增守卫 `crates/foundation/forge-ir/tests/opcode_table.rs`：清单与枚举等势、变体名唯一可逆、助记符唯一可逆、条件变体（`Icmp`/`Fcmp`）按变体身份。ISA TOML 的 `op = "Iadd"` / `pattern.match` 名字契约从此可机器校验。
- **`crates/foundation/forge-ir/README.md`**（此前该 crate 无 README）：结构表、`FunctionBuilder`/`TypeContext`/原子修改原语/`Opcode::ALL` 的使用要点与已知欠账入口。
- **`docs/plans/forge-ir-v3-plan.md`**：forge-ir v3 改进方案（诊断、证据、设计原则、子系统方案、S0–S8 分期与门禁、外部参考、非目标）与 S0 落地记录。

### Fixed (2026-09-13)

- **剩余的 x86 形态写死（残余 R8–R10）**：sret / 宽向量 by-ref / 栈参数三条路径的生成代码里仍有字面量：`Reg::RBP`（6 处）、`Reg::RSP`（1 处）、by-ref/sret 的 64 字节向量槽步长、`max(72)` 帧需求、`-8` sret 槽；`[abi].stack_align` 缺省还是 x86 的 16。现在：基址寄存器从 `[abi.frame].fp/.sp` 派生（未声明时回退主 GPR 组 0 号占位，而这些路径只在声明了对应角色的 ISA 上生成）；向量槽步长由 `[meta].vector_tiers` 最大档派生、帧需求 = 槽步长 + `[meta].slot_bytes`、sret 槽 = `slot_bytes`、栈参数偏移按 `slot_bytes`；`stack_align` 缺省 = `slot_bytes`。x86 生成物对这几处逐 token 等价（64/72/8 均由元数据算出同值），行为由 x86 矩阵 195/3/0 与 riscv64 矩阵 131/67/0 守住。
  （其中 `[abi].stack_align`、`[meta].slot_bytes` 等键随后归入 `[stack]`/`[abi.stack_args]`，见下条 B3。）

### Changed (2026-09-13)

- **指令字宽 = ISA 数据，且无白名单/上限（接口通用化 B6）**：
  定宽 ISA 此前被写死为"32 位字、4 字节读字"（`codegen/mod.rs` 的 `!= Some(32)` 门槛、`u32::from_le_bytes` 读字、`machine.rs` 的 `RelocKind::Relative(4, 0)`）。
  现在 `[meta].default_inst_width` 可写**任意 ≥ 1 位**，生成代码把指令字表示为**字节数组**（`[u8; ceil(位/8)]`，LE 位序）+ `__place`/`__bits` 助手：位域可跨字节、非字节对齐、落在机器字之外（100 位字夹具的 op 在 bit 92..100）。
  同步去限制的还有：位域的**字侧偏移无上限**（只保留"单个位域 ≤ 64 位"这一**值表示**上限——位域值承载在 u64 常量键与 i64 操作数上，越界明确报错）、定宽 label/global fixup 宽度由字长派生、`RelocKind::{Absolute,Relative}` 宽度 `u8 → u32` 且通用写入路径按宽度**符号/零扩展**补位（不再只认 1/4/8，也不再对其它宽度静默不补或 panic）。
  过程中修掉一个被新夹具暴露的生成器缺陷：**只有寄存器操作数的 ISA**（无 imm/label）生成出引用未定义 helper（`__expr`/`__set_label_operand`）的模块——两者现在无条件生成（未用入口加 `#[allow(dead_code)]`）。
  新夹具：`tests/isa/demo_inst8_v12.toml`（8 位字 + 字内 label 域，注册 2 位域 `RelocPatcher`）、`demo_inst12_v12.toml`（12 位字，非 8 倍数 + 填充位必须为 0）、`demo_inst100_v12.toml`（100 位字 = 13 字节，位域在 bit 92..100）；用例在 `tests/demo_inst{8,12,100}_v12_tests.rs`。
  验证（2026-09-13 本机）：workspace **1355 passed / 0 failed**（63 suites）、x86 jit 矩阵 **195/3/0**、riscv64（QEMU 真执行）**131/67/0**、fmt/clippy `-D warnings` 干净。
  32 位定宽 ISA（riscv64/arm64/demo）的生成物**代码形态**因此改变（不再用 `u32::from_le_bytes`/`u64 __w`），行为由上述测试与 riscv QEMU 真执行守住；x86（变长路径）生成物逐字节不变。

- **栈/传参键归类 `[stack]` 与 `[abi.stack_args]`（接口通用化 B3）**：
  栈槽单位、栈对齐、帧指针保存宽度此前散在 `[meta].slot_bytes`/`[meta].fp_overhead_bytes`/`[abi].stack_align`；
  Windows x64 的栈参数布局（基址寄存器、首个栈参槽位、槽步长、shadow space）散在 `[abi].stack_arg_shadow` 与生成器字面量（2/1/32、`Reg::RBP`/`Reg::RSP`）里。
  现在收敛为两个新表：`[stack] { slot, align, fp_save }`（缺省 = `addr_width` / `slot` / `addr_width`）与
  `[abi.stack_args] { callee_base, caller_base, first_offset_slots, stride_slots, shadow_bytes }`（x86 缺省 = `fp` / `sp` / 2 / 1）。
  `validate` 增加值域与枚举校验（基址只能 `fp`/`sp`、槽数与步长 > 0、shadow 为栈槽单位的正整数倍），frame/lowering 一律从新键取值。
  **行为不变证据**：`FGE_DEBUG_GEN=1` 的 5 个 ISA（x86/riscv64/arm64 + 两个夹具）dump 与重构前**逐字节一致**
  （`stride_slots == 1` 时不发射 `* 1`、字面量不带 `u32` 后缀）；workspace 1340 passed / 0 failed，x86 矩阵 195/3/0，riscv64 矩阵 131/67/0。
  规范同步 `docs/reference/isa-dsl.md`（`[meta]` 示例、键表、`[abi]` 示例与 `[abi.stack_args]` 说明）与 `CLAUDE.md`（宽度元数据条目）。

- **`[types]` 类型→类映射（接口通用化 B2）**：
  值池门与 lowering 的"类型 → 寄存器类"推导现在可被 ISA 覆盖——软浮点（`f64 = "gpr8"`）、1 字节地址（`ptr = "gpr1"`）、
  显式拒绝（`i64 = "unsupported"`）都成为 TOML 数据（新增 `TargetRegInfo::type_map()` / 生成的 `__TYPE_MAP` /
  `LowerCtx.type_map`，`class_for_type` 与 `reg_class_for` 读同一份数据）。
  校验：类型名白名单、目标必须已声明、类宽 ≥ 类型字节宽（`ptr` 按 `[meta].addr_width`，否则报"会静默截断"）、`void` 只能 unsupported。
  夹具演示：`tests/isa/demo8_v12.toml`（`ptr = "gpr1"`）、`tests/isa/demo_v12.toml`（`f32/f64 = "gpr8"` 软浮点）；
  新增 4 个 DSL 单测 + 3 个宿主/夹具断言。实测：workspace 1340 passed / 0 failed，x86 矩阵 195/3/0，riscv64 矩阵 131/67/0。

- **分配器类表改为 ISA 声明（接口通用化 B1，关闭审计遗留 R1）**：
  宿主的寄存器类表此前是"编译期编造"——`run_regalloc` 用硬编码清单（GPR 1/2/4、FPR 4/8/16、值池/地址类、tier）
  给**每个 ISA** 造类并让未声明类继承同族最宽类的池；demo8 这种只声明 `[reg.gpr1]` 的 ISA 也会得到
  GPR(2)/GPR(4)/FPR(8)/VEC(16…) 等"可分配但不可编码"的类。
  现在：DSL 生成 `TargetRegInfo::register_classes()`（**类表唯一来源**）——GPR 用类型系统整数宽度 {1,2,4,8}（≤ 主 GPR 宽）
  ∪ 已声明组宽 ∪ 地址/值池；FPR 用已声明组宽 ∪ **有效的宿主浮点值池类**（`value_fpr_class()` 缺省 FPR(8)）；
  VEC 用 `[meta].vector_tiers`；池分别取主 GPR / 浮点文件的分配序。宿主删除 `fallback_classes` 编造循环。
  新增守卫：x86 类表断言（含 GPR(1..8)/FPR(8,16,32)/VEC(16,32,64)，不得含未声明的 FPR(4)）、demo8 类表断言（恰好 `[GPR(1)]`）。
  **过程中 riscv64 QEMU 矩阵抓到真实回归**（首版未登记"有效浮点值池类"→ riscv 的 7 个 fcmp 错值），已修；
  修后 x86 195/3/0、riscv64 131/67/0。

- **demo/示例 ISA 迁出库本体，`isa_from_file!` 支持宿主 crate 路径**：ISA-DSL 的示例谱（`demo_v12`、`demo8_v12`）此前是 `forge-codegen` 的 `src/arch/` 模块 + 仓库根 `isa/` 谱，与 x86_64/arm64/riscv64 这些**真实后端**并列，容易误读为"发行 ISA"。现在：
  - `isa_from_file!` 新增可选第二参数 `krate = <路径>`：生成物里的 `crate::…` 改写为 `<路径>::…`、`forge_ir::…` 改写为 `<路径>::ir::…`（新增 `forge_codegen::ir` re-export），因此生成代码只依赖宿主的公开面；**缺省参数生成物逐字节不变**（已用 5 个 ISA 的 `FGE_DEBUG_GEN` dump 逐字节比对）。
  - `isa_from_file!` 参数解析与路径改写有单测（`crate` 改写只作用于路径位置，`pub(crate)` 可见性标记与字符串字面量不受影响）。
  - 夹具谱移到 `crates/backend/forge-codegen/tests/isa/{demo_v12,demo8_v12}.toml`（附 `README.md`），由 `tests/common/mod.rs` 用 `krate = forge_codegen` 宿住；删除 `src/arch/demo_v12.rs`、`src/arch/demo8_v12.rs` 与 `src/lib.rs` 的 re-export；6 个测试文件改经 `common::demo*`。
  - 新增库表面守卫 `tests/library_surface.rs`：`src/**` 不得引用 demo 谱、`src/arch/mod.rs` 只登记真实后端、仓库根 `isa/` 只剩 3 个发行谱、夹具谱必须在 `tests/isa/`。
  - 生成代码运行面所需的公开项补登：`pub use forge_ir::IrError`（此前是私有 `use`，是"生成物只能活在库内部"的最后一处硬依赖）、`pub use forge_ir as ir`；`impl_erased_target_machine!` 宏体改 `$crate::ir::…`（不再要求调用方有裸 `forge_ir` 在作用域）。
  - 文档：`CLAUDE.md`（Key Architecture Rules 第 1 条、ISA Backend Pattern、Code Conventions、Testing Notes）与 `docs/reference/isa-dsl.md`（快速开始、新增「生成代码依赖的运行面」、`已有 ISA 谱` 拆成发行后端/测试夹具）同步。

### Added (2026-09-12)

- **宽度元数据（去「寄存器类型/宽度写死」，DSL + 宿主）**：DSL 语法层早已支持任意寄存器宽度（`RegClass` payload = 字节），但生成期与宿主流水线把主 GPR 组锚定在 `GPR(8).or(GPR(4))`、地址类/值池/栈槽单位/帧开销内置 x86 的 8 字节缺省——只声明 1 字节寄存器组的 ISA 会**生成成功但语义错误**（名字表为空 ⇒ `sp`/`fp`/`scratch`/`callee_saved`/物理 clobber 静默丢弃，或落回 `from_index(0, GPR64)` 构造该 ISA 根本不存在的类）。现在：
  - `[meta]` 新增可选键（单位字节，优先级 **显式键 > 派生 > 报错**）：`default_gpr_width`（缺省 = 最宽已声明 GPR 组）、`default_fpr_width`（`fpr16` 基准优先）、`addr_width`、`value_gpr_width`、`value_fpr_width`（缺省 8 = f64 值池）、`slot_bytes`、`fp_overhead_bytes`、`vector_tiers`（缺省 `[16,32,64]`）。显式键必须指向已声明组，否则 `validate` 报错。
  - 生成模块新增常量 `__ADDR_CLASS`/`__VALUE_GPR_CLASS`/`__VALUE_FPR_CLASS`/`__SLOT_BYTES`/`__FP_OVERHEAD_BYTES`/`__VECTOR_TIERS`；宿主 `TargetRegInfo` 新增 `addr_class`/`value_gpr_class`/`value_fpr_class`/`slot_bytes`/`vector_tiers`/`class_for_type`（全部带**等于历史值**的缺省实现）。
  - 生成期去写死：主 GPR/FPR 组派生、`name_to_idx` 空表兜底删除、sp/fp 不再回退 `from_index(0, GPR64)`、`frame_pointer_overhead()` 由常量 8 改元数据、组别名兜底 `"RAX"` 删除、MemRef base/index 用地址类、spill 基址不再回退字面量 `"RBP"`、FPR spill 宽度档改为**已声明模板键**派生、by-value 向量阈值用 `[abi.arg_class].limit`、栈槽对齐/槽深/by-ref 向量槽用 `__SLOT_BYTES`。
  - 宿主去写死：`LowerCtx` 新增 `value_gpr_class`/`value_fpr_class`/`addr_class`/`slot_bytes`/`vector_tiers`（`CompileState::new` 注入）、值 XReg/零值/临时 vreg/phi-copy 类、spill scratch 类、类表 fallback 清单、`reg_class_for` 向量档位全部元数据化。
  - **fail-closed**：`sp`/`fp`/`scratch`/`reserved`/`callee_saved`/`ret_regs`/`call_ret_reg`/`call_clobbers`/`arg_class.regs`/`implicit_regs`/`[spill.*].base` 名字必须解析到已声明组（生成期报错）；`[abi].stack_align`/`stack_arg_shadow` 的"8 的倍数"校验改为按栈槽单位（这两个键 2026-09-13 归入 `[stack].align` 与 `[abi.stack_args].shadow_bytes`）；函数内值类型必须被 `class_for_type` 承载，否则**编译期** `Unsupported`（点名类型与值池宽度）。
  - 新夹具 **`isa/demo8_v12.toml`**（1 字节寄存器 ISA：唯一 `[reg.gpr1]` 组，`addr_width`/`slot_bytes`/`value_gpr_width`/`fp_overhead_bytes` = 1，`default_opsize = 8`）+ `crates/backend/forge-codegen/tests/demo8_v12_tests.rs`：断言元数据派生（`GPR(1)`、1 字节槽、sp/fp/scratch 名字解析成功、`allocatable = A0..A3`）、汇编→编码→解码→反汇编往返、宿主编译 i8 函数（20 字节机器码反汇编回 `mov A3, A0`/`add A1, A3, A2`/`ret`）与 i64 的编译期拒绝。
  - 反回潮守卫：`crates/frontend/forge-dsl/tests/no_hardcoded_widths.rs` 与 `crates/backend/forge-codegen/tests/no_hardcoded_widths.rs`（白名单带理由且条目必须被命中）。
  - 行为不变证据：`cargo test -p forge-dsl --lib` 112 passed；`cargo test -p forge-codegen --all-features` 266 passed / 0 failed；x86 jit matrix `pass=195 skip=3 fail=0`（`FORGE_JIT_EVENTS` 事件核对）。规范见 `docs/reference/isa-dsl.md` 的「宽度元数据」节。
  - **独立审计后的加固（同日）**：
    ①值池门从"只比类宽"改为"**池宽 + 寄存器文件存在性**"——无 `[reg.fpr*]` 的 ISA 上 `f32/f64/v64/v128/v256` 一律编译期拒绝
    （此前会放行一个该 ISA 不存在的 `FPR(8)`/`VEC(16)` 类，失败推迟到 regalloc）；
    ②`@push_callee`/栈参数收参与帧开销的**槽步长**从写死 `8`/`16` 改为 `__SLOT_BYTES`/地址类宽度；
    ③`[abi].stack_arg_shadow` 路径的内存基址从字面量 `Reg::RBP` 改为 `[abi.frame].fp` 派生（fp 缺失即生成期报错）；
    ④`TargetRegInfo::frame_pointer_overhead` 的 trait 缺省由常量 `8` 改为地址类宽度；
    ⑤`check_value_pools` 跳过被 DCE 墓碑化的 `TypeId::VOID` 值（否则 1 字节值池 ISA 上任何含死值的函数都会被误拒；新增 O1 回归测试）；
    ⑥demo8 测试补 `F32/F64/V64/V128/V256 → None` 断言、机器码反汇编的寄存器名/收参/返回回写断言，错误信息断言收紧到本门文案；
    ⑦两个反回潮守卫的 `FORBIDDEN` 扩到全部类字面量变体，并在文档里写明"裸数字宽度不在守卫范围"这一已知边界。

- **V256/V512 向量 IR 层 Load/Store（ISA 规则 + 编译入口能力门）**：`isa/x86_v12.toml` 的 `Load`/`Store` 规则原先只覆盖
  `rd_vec`/`rs1_vec` = 8/16（V64/V128），>16B 由编译入口 **fail-closed 拒绝**（"ISA 类模型缺 YMM 槽类"）。现有：
  - 规则补齐 32/64 两档 → `VMOVUPS_256_R_MEM`/`VMOVUPS_256_MEM_R`（VEX.256，`vex_l=1`）、`VMOVUPS_512_R_MEM`/`VMOVUPS_512_MEM_R`（EVEX.512，`evex_l=2`）；
  - 编译入口的字节门改为：32B（V256）放行（与既有 V256 算术路径一致）、**>32B（V512/EVEX）需 AVX-512F**（与宽向量 ABI 守卫同一判据）、
    其余非 32/64 的 >16B 宽度仍显式拒绝——四条路径都不产出静默错码；
  - 新增 **reg 基址**内存形式（`modrm = { rm = "[reg]" }`，仅基址 `[base]`、disp 恒 0）：IR Load/Store 的地址是 lowering 的**寄存器操作数**，
    而 lowering 模板只能绑定寄存器、无法现场构造 MemRef（原有的 `vs_memref` 形式继续供 ABI by-ref 路径的 `[RSP+off]`）。
  - 验证：生成级 `test_v256_slot_load_store_is_lowered`（VEX `C4 .. 7C 10/11`，且不得退回 `movsd`）、
    `test_v512_slot_load_store_requires_avx512`（无能力必须编译期拒绝 + `FORGE_ASSUME_AVX512` 下 EVEX `62 .. 10/11`，P2 L'L=10）、
    runtime `test_jit_v256_slot_roundtrip`（真执行 VEX.256 槽往返 lane7 = 16.5 → 16）、
    `test_jit_wide_vec_reg_base_mem_roundtrip_bytes`（asm→encode→decode→encode 字节往返 + objdump 实证
    `c4 e1 7c 10 00` = `vmovups ymm0, YMMWORD PTR [rax]`、`62 f1 7c 48 10 00` = `vmovups zmm0, ZMMWORD PTR [rax]`）。

- **V512 向量常量 `Vconst rd=512`（WA-43）**：ISA 模型的 `Vconst` 规则原只覆盖 `rd = 64/128/256`，64 字节常量落到「no matching rule」→
  `Unsupported`。该缺口只在**有 AVX-512F 的机器**暴露（运行级 `test_jit_v512_byref_param` 无 AVX-512F 时提前 return，
  本机即如此）⇒ 历史上按 CI runner 分配偶发红（`test result: FAILED. 118 passed; 1 failed`）。
  - 实现：新增 EVEX 指令 `VINSERTF32X4`（`EVEX.512.66.0F3A.W0 18 /r ib`）、常量池占位符
    `{vconst_lo_h2}` / `{vconst_hi_h2}` / `{vconst_lo_h3}` / `{vconst_hi_h3}`（`__vconst_half` 本已按 half 索引泛化），
    以及两条 `Vconst` 规则（32 位 lane：F32/I32 用 `PUNPCKLDQ`；64 位 lane：F64/I64 用 `PUNPCKLQDQ`）：
    4 个 128 位段各自装好后按 imm=0/1/2/3 插入；4 条插入覆盖全部 lane（`{out}` 自身当累加器，**初始值不影响结果**）。
  - 验证：新增**生成级**测试 `test_v512_vconst_generates_four_evex_inserts`（无需 AVX-512 硬件——无宽向量参数/返回，
    不触发宽向量 ABI 的 AVX-512 门控；断言恰有 4 条 `62 … 18 /r ib` 且 imm = {0,1,2,3}）；
    `objdump -D -b binary -m i386:x86-64` 解码实测为 `vinsertf32x4 zmm13, zmm13, xmm15, 0x0/1/2/3`，
    且 8 条 `movabs` 常量逐 lane 与源码 f32 位型一致（`0x404000003fc00000` … `0x4180000041600000`）；
    运行级 lane15=16 断言仍由 `test_jit_v512_byref_param` 在有 AVX-512F 的 runner 上守护。

### Fixed (2026-09-12)

- **V512（64B）向量按 lane 提取取错 lane（WA-47，CI run #51/#57/#61 的真实根因）**：`Vextract` 规则集里
  V256 有 `rs1_width = 256` 专档，而 **V512（`rs1_width = 512`）没有任何规则** ⇒ 落到 128 位通用回退
  `PSHUFD {f1}, {0}, {imm0}`；而 `PSHUFD` 的 imm8 **只用低 2 位**选 dword ⇒ lane L 实际取到 **lane(L%4)**
  （lane0..3 恰好正确、lane4..15 全错）。症状：`Test (Windows)` 上 `test_jit_v512_byref_param` 断言
  lane15 = 16 实测得 **4 = lane3**——该用例只在**有 AVX-512F 的 runner** 上真跑，因此长期伪装成
  "偶发/机型相关"（3 红 / 11 次 run）。
  - 修法：①新增 EVEX 指令 `VEXTRACTF32X4`（`EVEX.512.66.0F3A.W0 19 /r ib`，dest 在 r/m、src 在 reg）；
    ②新增 6 条 V512 `Vextract` 规则（`vary` 压缩）：先用 `VEXTRACTF32X4` 取 `seg = lane/4` 的 128 位段，
    再段内 `PSHUFD` 取 dword（f32/i32 用 `(lane%4)*0x55`；f64/i64 偶 lane 免 shuffle、奇 lane `PSHUFD 78`）；
    lane0 由既有 `priority = 1` 快路径覆盖。
  - 验证：生成级守卫断言 lane15 必须含 `EVEX 62 … 19 … imm=3` + `PSHUFD 0xFF` 且不得出现回退形态
    `PSHUFD …, 15`（objdump 实证 `vextractf32x4 xmm14, zmm15, 0x3` + `pshufd xmm14, xmm14, 0xff`）；
    workspace tests 0 failed；e2e 8/8 + `stage_a passed=105/105 known=[]`；clippy `-D warnings`/fmt 干净。

- **向量溢出（spill）宽度静默截断（WA-46）**：`isa/x86_v12.toml` 的
  `[spill.FPR]` 只有一份 **8 字节 `MOVSD`** 模板，而生成的 `emit_spill_load/store` **忽略 `width` 参数**；
  同时 `reg_class_for` 把 **64 字节向量也归入 `VEC(32)`**（该类的 `reg_width = 32`）⇒ 任何 FPR 类溢出只搬低
  8 字节、V512 的 spill 槽只有 32 字节，**未被搬运的高半区是栈残留**。
  症状：`Test (Windows)` 上 `test_jit_v512_byref_param` **偶发** `lane15 != 16`（`runtime/jit.rs:1457` 断言失败）
  ——该用例只在**有 AVX-512F 的 runner** 上真跑，本机无此硬件 ⇒ 长期只表现为 CI 抖动（#51 的"未知抖动"即此）。
  - 修法：①溢出模板**按值宽分档** `[spill.FPR16/32/64]`（`MOVUPS_RM/MR` 16B、`VMOVUPS_RM/MR` VEX.256、
    `VMOVUPS_ZMM_MEM/MR` EVEX.512）+ 新增 16B **MemRef 形式**指令 `MOVUPS_RM/MOVUPS_MR`（原 reg 基址形式
    disp 恒 0，表达不了 `[RBP-off]`）；②forge-dsl 生成器按 `width` 分派，**未声明宽度 → 编译期
    `Unsupported`**（fail-closed，绝不退回窄搬运；ISA 无 FPR 溢出模板时维持原 no-op）；③`reg_class_for`
    增加 `VEC(64)` 档（>32 字节向量）并在类表登记（`reg_width = 64`，槽宽与搬运宽度都按值宽）。
  - 验证：新增 `test_fpr_spill_width_dispatch`（直接驱动 `FrameLowering`，逐宽度断言机器码：8B `F2 0F 11/10`、
    16B `0F 11/10`、32B `C4..7C 11/10`、64B `62..11/10`（L'L=10），**24B/48B 必须报错**）；
    workspace tests 0 failed；e2e 8/8 + `stage_a passed=105/105 known=[]`；clippy `-D warnings`/fmt 干净。
  - 已知残余：向量**高压力** spill（>16 个同时活跃向量值）仍受 `[abi].scratch` 只有 2 个的限制
    （`instruction needs 3 scratch regs …`），由 `test_jit_v256_high_pressure_spill_is_known_limited` 断言记录。

- **niche 枚举 tag 偏移一般化（WA-44）**：`lower/mod.rs` 新增唯一助手 `niche_tag_offset`（`statement.rs` 写侧与 `rvalue.rs` 判别读侧共用）——
  旧实现只认「`ScalarPair` 且第二标量是指针 → `b_offset`」，**其余一律 0**；于是 niche 落在聚合 payload **非 0 偏移**的枚举（如 24 字节 `Memory` repr、
  niche = 第 3 个字段 offset 16）读写都在 offset 0 ⇒ 判别读到字段 0 的值，该值为 0 时 `Some` 被误判成 `None`。
  - 修法（范围收窄）：①`ScalarPair` 分支**逐字保留** WA-26/WA-28/vl3 的经验判据（`b` 是指针类才用 `b_offset`，否则 0）；
    ②**只新增**「非 ScalarPair」分支 → `Variants::Multiple { tag_field }` + `fields().offset(tag_field)`（rustc_abi 文档明示 Niche 的 niche 在该 `tag_field` 字段）；
    ③其余仍 0；另加 fail-closed 尺寸守卫（`偏移 + tag 宽度 > 枚举尺寸` → 编译错误）。
  - 验证：新增 e2e `niche_offset_some_zero_first`（期望 12；修复前 exit=99）与 `niche_offset_none_roundtrip`（99）；
    e2e 全量 `passed=105/105 known=[]`；hammer（计划 §9.2）5 轮 `105/105 KNOWN=[]` + 5 例各 ×10 全过。
  - 范围教训：曾把 `ScalarPair` 分支"一般化"为「tag 与 `b` 同类就用 `b_offset`」——grow 链三例（`vec_push`/`vec_iter_enumerate`/`string_concat_len`）
    立即 `exit=0xC000001D`（gdb：`ud2`/`unreachable_unchecked`）⇒ 那些枚举的判据不能按 tag 标量类推，故保留原判据、只补聚合分支。

- **宽聚合 payload 的 niche 枚举 `None` 写入宽度（WA-42，关闭 WA-41）**：`lower/statement.rs` 的 niche 构造把 tag 宽度按 `backend_repr`
  两分支（`Scalar`/`ScalarPair`）+ `_ => 4` 兜底推导——24 字节 `Memory` payload + offset 0 的 8 字节指针 niche（如 `Option<(NonNull<u8>, Layout)>`，
  即 `RawVecInner::current_memory` 的返回类型）落进兜底 → 发 `store i32 0`（只写低 4 字节），**高 4 字节残留栈上旧值** → 调用方按 8 字节判空失败
  → `finish_grow` 误取 `Some(野指针)` + 栈残留 `old_layout` → `grow_impl_runtime` 的 `copy_nonoverlapping` 解引用 → AV（`Vec::new(); v.push(1)` 即崩）。
  - 修法：宽度改取**枚举 tag 标量自身**（`Variants::Multiple { tag, .. }`，Niche 编码下 rustc 的 `tag` 即 niche 字段的标量）→ `tag.primitive().size()`；
    ≥8 字节写 I64（完整清零），窄 tag（u8/u16）行为不变。
  - 验证：本机 WA-41 最小复现由 `exit=-1073741819` 变 **16**；`FORGE_TRACE_IR` 对照 `store i32 0` 1 处 → 0 处；
    e2e `passed: 103/103 known=[]`（新增 IR 级回归门 `niche_wide_payload_none_tag_store_uses_tag_width`）。
  - 同一缺陷即 CI 上 `vec_push`/`vec_iter_enumerate` 的机型相关 AV（同一 grow 链、同一误判路径；AMD runner 栈残留高位恒非零、本机多数布局恰为 0）。
    CI 跨机器实证（run 93927221004，runner = AMD64 Family 25 Model 1，即修复前 20/20 AV 的同机型）：`[SUMMARY] stage_a passed=103/103 known=[]`、
    两例各 20 次单跑 `2x20`/`80x20`、alloc 逐步探针 21/21 全 ok。
- **VEX/EVEX 解码臂对「内存形式 + reg 槽」直接报错（forge-dsl `vlen.rs`）**：v15 的 ModRM 模型允许
  `modrm = { rm = "[名字]" }` 指向 `reg` 槽（= 仅基址 `[base]`、disp 恒 0，见 `docs/reference/isa-dsl.md`），
  编码侧也已实现该风味，但 **VEX/EVEX 解码侧**仍保留旧的「内存形式 rm 必须是 mem 槽」守卫（生成期 `Err`）——
  于是任何 `[reg]` 形式的 VEX/EVEX 指令都无法加入 ISA。现已对齐：rm 槽为 reg 类时 base 取 `ModRM.rm + B`（SIB 在场按 `SIB.base`）。
  回归守卫 `test_jit_wide_vec_reg_base_mem_roundtrip_bytes`（字节往返 + objdump 实证）。
- **`FORGE_ASSUME_AVX512` 泄漏到运行级 EVEX 用例 → 非法指令（`STATUS_ILLEGAL_INSTRUCTION` 0xC000001D）**：
  该 env 只应放开**生成期**可行性门，但 `test_jit_v512_byref_param` 用 `avx512_available()`（读 env）判断是否跳过——
  另一个测试留下的 env 会让它在无 AVX-512F 的 CPU 上**真的执行** EVEX。新增 `avx512_hardware_available()`
  （纯 cpuid、不读 env），运行级用例改用它判 skip；生成级用例的 env 开关收敛到 panic 安全的 RAII 守卫
  （`AssumeAvx512`，Drop 时清除）。实测：`cargo test -p forge-codegen --lib --all-features` 修复前 `0xc000001d` 崩在
  `test_jit_v512_byref_param`，修复后 `123 passed; 0 failed`。
- **宽向量守卫里一条空断言**：`test_v512_byref_callee_load_is_64b` 的负向断言按 2 字节 VEX（`C5 FC 10`）匹配，
  而本编码器**恒发 3 字节 VEX**（`C4`）⇒ 该断言恒真（空守卫）。改为 `C4 .. 7C 10` 形态。
- **e2e 门禁转正收官**：`vec_push` / `vec_string` / `vec_from_slice` / `vec_iter_enumerate` / `box_value` 移出 `FLAKY` 名单并翻 `known_failure=false`
  —— 5 例的错码/AV 从此**硬失败**（不再有 `CI-ENV-AV`/超时容忍路径；`FORGE_E2E_STRICT_FLAKY` 机制保留但名单为空即等价全量门禁）。
  判据按计划 §9.5 执行：CI run 93927221004 在同一 AMD 机型给出 `known=[]` + 探针 21/21 ok（§9.7.1）。

### Added (2026-08-08)

- **forge-rustc 模块化重构（P1）**：`lib.rs` 2143 行 → 61 行（薄 facade），拆分为 9 个模块（`prelude`/`backend`/`func_ref`/`alloc_runtime`/`layout`/`types`/`abi`/`compile`/`rustc_compat` + `lower/` 6 文件）；对齐 CGCL 架构（mod prelude + codegen backend 模式）。
- **rustc 1.99 nightly API 漂移适配（37 处）**：`CodegenBackend::codegen_crate`/`join_codegen` 签名变化、`CompiledModule.global_asm_object`、`BackendRepr::ScalarPair {..}`、`VariantLayout.field_offsets`、`EarlyBinder::bind(tcx, ..)`、`LangItem::DropGlue`、`substs.skip_binder()`、`Instance::resolve_drop_glue` 等——全部收敛进 `rustc_compat.rs` + `abi.rs`。
- **ABI 层收敛（P4.1）**：`abi_kind_of_ty`（PassMode 投影：16 字节 Scalar → Indirect、SimdVector → Direct）成为 `is_agg_mem`/`is_scalar_pair_abi` 统一内核；`pad_call_args` 按 FnAbi 补齐 track_caller 隐藏 `&Location` 参数（修复 panic 路径参数错位）；sret 计数修正（rustc FnAbi.args 不含 sret 指针但调用方须传）；Ignore/ZST 参数跳过（Global 等零大小类型不占参数槽）。
- **块参数传参一致性修复（WA-14）**：`map_terminator_args_to_params` 复用 arg 已有寄存器 + `pre_allocate_block_param_xregs` 顺序调整——write_bytes 内联循环从"完全不执行"变为执行（count=1 变体返回 0xAB 正确）；新增最小复现回归测试 `test_loop_block_param_write_bytes_style`。
- **测试体系统一（P4.5）+ CI 纳入（P4.6）**：`stage_a.rs`/`run_tests.sh`/`test_runner.sh` 并入 `tests/e2e.rs`（58 用例，known_failure+reason 回归探针）；`.github/workflows/ci.yml` 新增 `forge-rustc-check` job。
- **WORKAROUNDS.md**：14 条机读绕法清单（`[WA-NN]` 编号 + 代码注释引用）。

### Fixed

- **write_bytes_loop 转正（e2e 56/58，WA-14 关闭）**：三层根因全修——①块参数传参两层（map_terminator 复用 arg 映射 + pre_allocate 顺序）；②**窄类型宽度（真根因）**——`opsize_from_type(u8)=32` 导致主库 Load/Store 越界 4 字节读写（u8 元素读 0xABABABAB 垃圾、write_bytes 循环写 32 字节覆盖相邻槽）+ `ireduce(mov)` 不扩展导致 cast 后高 24 位残留（wb1 返回 0xFFFFFFAB）。修复：新增 `LowerCtx::mem_opsize_from_type`（Load/Store 用真实内存宽度，**不枚举不截断**——u8→8、u16→16、自定义非常规宽度如 12 字节 GPR→96 原样传递）+ forge-rustc IntToInt cast 对 u8/u16 无符号源零扩展 mask。
- **vec_push/vec_string 根因最终定性（十二轮深挖，仍 known_failure）**：**嵌套 niche 传播**（rustc 的 niche 布局传播——外层枚举判别与内层 payload 判别共享/嵌入字节，LLVM 级特性）：grow 链（Result/ControlFlow/TryReserveError 错误传播）与最小复现 cf5（`CF::Break(Err(5u8))` 应=7 现=1）同源——rvalue.rs/statement.rs 的 Niche 单层实现需扩展为嵌套传播（以 cf5 为驱动用例），WA-11 记录。
- **field_offset Primitive 防护（落库）**：`lower/mod.rs` 的 `field_offset` 对非 enum 类型无条件调 `fields().offset()`——标量（`FieldsShape::Primitive`）无字段触发 "Primitive has no fields" 编译 ICE（嵌套枚举投影如 `CF::Break(Err(1u8))` 的 `.0` 对标量）——修复后嵌套 `ControlFlow<Result>` 从编译 ICE → 正确运行。
- **诊断基础设施（落库，无行为影响）**：`FORGE_TRACE_TERM`（每块 terminator 打印）+ vcode dump 加 fn 名前缀（62 个函数的 vcode 此前无法区分——十二轮深挖的关键工具）+ regalloc_bt 的 spill/reload/evict trace。
- **regalloc_bt 诊断 trace 补齐**：`spill_vreg`/`reload_from_stack`/`evict_and_assign` 增加 `[spill]`/`[reload]`/`[evict]` 输出（此前完全静默，无法定位跨块寄存器问题）。
- **forge-rustc 3 个 known_failure 的编译层问题**：`vec_push`/`vec_string` 从 compile failed(101 ICE) 变为编译通过（sret 计数 + Ignore/ZST 参数跳过）。
- **forge-codegen liverange 测试编译修复**：`RegClass::GPR` → `RegClass::GPR(8)`（多宽度化重构后测试代码未跟上）。
- **forge-dsl 删除未使用 `stack_scratch` 变量**（`[abi.call]` 必需性校验块残留）。

### Added (2026-08-05)

- **RegClass 宽度化重构（多宽度寄存器类）**：
  - `forge-ir::RegClass` 由 7 个硬编码变体改为 `GPR(u16)/FPR(u16)/VEC(u16)` 三变体，payload = 字节宽度，可表达任意 ISA 非常规宽度（如 12 字节 GPR）；`GPR64/GPR32/GPR8/FPR64/VEC128/VEC256/Int/Float` 等均为便捷常量，语义不变。
  - 物理寄存器索引全链 `u8 → u32`（`PhysReg::to_index/from_index`、`PReg.num`、`FrameAccess::register_index`、DSL 生成的 `uses/defs/reg_field/set_reg_field/clobbers`、`TargetRegInfo` 各列表、`ClassConfig.allocatable/sp_reg/fp_reg`）——支持 >255 寄存器的 ISA（如 JVM 类）。
  - `TargetRegInfo::register_classes()` 暴露 `[reg_classes.*]` 全部多宽度类（`RegisterClassInfo` 新增 `allocatable`）；`reg_class_width` 全类支持（TOML 优先，未定义类回退 payload）。
  - lowering 按类型分派：I32 结果/参数/块参数 → `GPR(4)` 池（32 位指令语义），F32/F64 → `FPR(8)`；未定义类回退（GPR 族继承 GPR64 池、FPR/VEC 继承 FPR64 池）。
  - 分配器新增跨宽度类物理重叠检测（`phys_conflicts`/`phys_owner`）：GPR(4) 与 GPR(8) 同编号视为同一物理寄存器，杜绝两个 XReg 分到同一物理寄存器。
  - `isa_from_file!` 路径解析支持向上查找（`CARGO_MANIFEST_DIR/../../..`），非 workspace 根 cwd 下编译也可定位 `isa/*.toml`。

### Added (2026-07-26)

- **Directory restructuring**: `forge-codegen` (37 files → 5 subdirectories: `arch/`, `traits/`, `pipeline/`, `runtime/`, `ext/`) and `forge-opt` (24 files → 5 subdirectories: `scalar/`, `loops/`, `ipa/`, `advanced/`, `support/`). All backward-compatible `pub use` re-exports preserved.
- **Multi-segment conditional merge (E1)**: Consecutive `?cond` segments with the same condition now share one `if` block in generated code instead of generating separate `if` blocks.
- **Multi-block CFG JIT tests (F1)**: JIT tests for if-else branching, multi-parameter branching, loop countdown, stack frame with many locals, and boundary constants (i32::MAX/MIN, i64::MAX).
- **AArch64 Icmp lowering (B1)**: All 10 integer comparison conditions activated in `isa/aarch64_v10.toml` (EQ/NE/LT/GT/LE/GE/LO/HI/LS/HS via SD_CMP + SD_SETCC).
- **AArch64 + RISC-V compile tests (C2/C3)**: 5 new compile tests verifying add/mul, Icmp, and branch lowering across AArch64 and RISC-V backends.
- **Constant pool inline syntax (C1)**: `{const 42}` / `{const 0xFF}` / `{const 3.14}` in lowering operands emits immediate values without constant pool lookup.
- **Name-based field indexing**: V10 lowering path uses `ParsedTemplate` field names for operand-to-field mapping. `[lower.*]` operands must match BTreeMap alphabetical field order（v10 语法文档已随 v11 删除——现行唯一 DSL 语法见 `docs/reference/isa-dsl.md`）。

### Fixed

- **Display round-trip 两个真 bug**（forge-ir）：`CallIndirect` 丢失函数指针（现输出 `call <retty> %ptr(...)`）；`StackAddr`/`GlobalAddr`/`Alloca` 丢失立即数（现输出 offset/大小，如 `stack_addr -4`）——经扩展指令 round-trip 测试暴露。
- **审查驱动补测试（+7）**：forge 扩展指令 round-trip（stack_addr/copy/call_indirect/fconst 内联）、语义错误路径（undefined block/function）、`DataLayout::is_default`。见 `docs/archive/coverage-history.md`。
- **覆盖率工具链诊断记录**：cargo-llvm-cov 在 Windows msvc + rustc 1.96 无法产出可靠报告（profraw 与二进制 counter 错位，全 0%），四种方案验证记录见 `docs/archive/coverage-history.md`。

### Fixed

- **WASM32 `end` opcode**: Added `needs_epilogue_label()` trait method to `InstructionSet`, guarding x86-specific JMP emission. WASM functions now correctly terminate with `end` opcode (0x0B).
- **`forge-plugin` missing `log` dependency**: Added `log = "0.4"` to Cargo.toml — `--all-features` compilation now succeeds.
- **LEA constant pool FIXME**: Changed `constants: None` to `_constants_clone.as_deref()` in `lowering.rs`, enabling scale-value detection for LEA merge optimization.
- **CLAUDE.md cleanup**: Removed outdated `.rs.bak` file references.

### Added (2026-07-24)

- **Width-aware instruction model**: `FieldType::Opsize` + `DynType` in ISA model. Every GPR instruction now supports 16/32/64-bit operands via a unified encoding macro (`$modrm_rr`), automatically emitting 0x66 prefix (16-bit), default encoding (32-bit), or REX.W (64-bit) based on the opsize field.
- **Opsize propagation from IR types**: `LowerCtx::default_opsize` is set from `Type::size_bytes()` before instruction lowering. `default_for_type()` for Opsize reads `ctx.default_opsize`, making all instructions width-aware automatically.
- **17 new x86-64 instructions**: MOVZX (R8/R16), MOVSX (R8/R16), ADD/SUB/AND/OR/XOR/CMP r,imm32, CMPXCHG, XADD, BT/BTS/BTR/BTC, CMOVcc.
- **3 new encoding macros**: `$modrm_r_imm32` (width-aware r,imm32), `$cmovcc_rr` (conditional move with embedded condition code).
- **64-bit boundary fuzz tests**: 5 new tests exercising i64::MAX, i64::MIN, large multiply, power-of-two shift, and NOT operations.
- **DynType validation**: `IsaModel::validate()` now checks that kind is a supported type and default is in values list.

### Fixed

- **MOV64_RR encoding**: Changed from 0x8B to 0x89 (correct data direction: MOV r/m64, r64 → dest←src).
- **MOVQ_R64_XMM mnemonic**: Changed from `movq.to_gpr` (dot breaks IDENT lexer) to `movq_to_gpr`.
- **MOV_REG_IMM64 mnemonic disambiguation**: Changed to `mov_imm` to avoid AsmResolver collisions with MOV variants.
- **@shift_cl primitive**: Added missing REX.W prefix for 64-bit shift operations (was emitting 32-bit shift with 0x41 instead of 0x49/0x48).
- **Sshr lowering**: Uses `movsxd` (sign-extend 32→64) instead of zero-extending `mov` for arithmetic right shift.
- **MOVSXD_R_RM**: Hardcoded to always emit REX.W (always sign-extends to 64-bit), removed opsize field.
- **Prologue param copy TODO**: Resolved — `@move_args` already copies ABI arg regs → vregs via `Reg` type operands.
- **LEA constant pool**: Added cloning pattern to avoid borrow conflicts (scale validation disabled pending PatternMatcher vreg allocation fix).
- **Dead code warning**: Eliminated for `DynType.kind` and `DynType.values` (now used in validation).

### Changed

- **AArch64 TODO updated**: Prologue/epilogue require STP/LDP/MOV_SP/SUB_SP with Reg-type operands.
- **RISC-V TODO updated**: ADDI/SD/LD/JALR already defined; prologue needs Reg-typed variants.
- **Backend TODOs cleared**: AArch64 and RISC-V prologue requirements accurately documented.

### Added (2026-07)

- **Type::Bool**: New `Bool` type for comparison results (icmp/fcmp). Replaces `Type::I32` for boolean values, improving type safety and semantic clarity.
- **Opcode::Freeze**: New IR instruction to prevent undefined behavior propagation. Optimization passes treat `Freeze` as a barrier for constant folding and value inference.
- **SROA pass** (`src/optimize/sroa.rs`): Scalar Replacement of Aggregates optimization. Splits struct/array allocas into scalar allocas for mem2reg promotion.
- **AArch64 backend** (`examples/isa/aarch64_v10.toml`): New ISA backend targeting 64-bit ARM (AAPCS64 calling convention). Supports GPRs (X0-X30), FPRs (V0-V31), and standard instruction set.
- **CI configuration** (`.github/workflows/ci.yml`): Automated formatting, clippy, test, and docs checks across Linux/Windows/macOS.
- **Encoding DSL enhancements**: Declarative encoding format support for x86 and RISC-V instruction patterns.
- **PE/COFF relocation**: Format-aware relocation mapping for PE COFF (IMAGE_REL_AMD64_*) and Mach-O (X86_64_RELOC_*).
- **MIR extensions**: Expanded rustc MIR rvalue/terminator coverage (Repeat, Aggregate, CastKind variants, Assert, Yield).
- **LTO integration**: `Module::optimize_with_lto()` for cross-module optimization (inlining + dead function elimination).
- **E-Graph ISel**: `ISelPass` for algebraic simplification before instruction selection.
- **Performance benchmarks**: Criterion-based compilation pipeline benchmarks in `benches/compile_bench.rs`.
- **JIT multi-return**: Support for two-value returns (RAX + RDX) in the x86-64 JIT backend.
- **Register spill/reload**: Full spill handling for high register pressure scenarios using R10/R11 scratch registers.
- **Extended JIT test suite**: 129 integration tests covering parameters, returns, stack balance, register pressure, multi-return.

### Changed

- **Comparison result type**: `icmp` and `fcmp` now produce `Type::Bool` instead of `Type::I32`.
- **Copy instruction**: Now infers result type from source operand instead of hardcoding `Type::I32`.
- **Clippy clean**: All clippy warnings resolved in the main library and DSL codegen.
- **Rustc backend**: MonoItem path updated for latest nightly; Bool type mapping fixed to `Type::Bool`.

### Fixed

- Register allocator spill offset calculation (RBP-relative negative offsets).
- PE/COFF relocation flag mapping for x86-64 Windows targets.
- Mach-O relocation field naming (r_type, r_pcrel, r_length).
- Collapsible `str::replace` calls in DSL codegen (clippy).

---

## Version Policy

- **0.x.y**: API may change without notice. No stability guarantees.
- **1.0.0** (future): Public API frozen. Requires: rustc backend passes core/alloc tests, AArch64 backend functional, CI all-green, CHANGELOG maintained.
