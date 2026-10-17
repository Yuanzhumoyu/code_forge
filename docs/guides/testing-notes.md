# 测试与夹具笔记（Testing Notes）

> 状态：**[active]**（2026-10-11 起）。**本文是 `CLAUDE.md` 原「Testing Notes」节的原地搬家**——
> 原节约 300 行 / 26 KB，把 `CLAUDE.md` 顶到**工作区指令预算（64 KB）之上被加载器截断**
> ⇒ 按文档纪律（长内容按主题进 `docs/guides/`，`CLAUDE.md` 留索引）搬到此处，
> `CLAUDE.md` 侧只留最常用条目的摘要与到本文的链接。
> 一切以代码与测试为准；文中的行号/数字**随迭代漂移**，以符号名为准。

## 条目

- Integration tests in `tests/` import ISA types from `code_forge::backend::<isa_module>::*`
- **黄金/快照文件必须先归一化换行再比**（v20 A1 实测）：Windows 上 git 按 CRLF 检出签入的
  LF 文件（CI 的 `Test (Windows)` 就这么红过一次，本地因"测试自己写过一遍"而全绿）。
  `str::lines()` 会**吃掉** `\r`，所以"只比 `lines()`"看不见差异、而"直接比字符串"又会红
  ——两者叠加的现象是**差异点报在文件末尾 + 期望 `<缺行>`**，极难排查。
  正确做法：比之前把两边都折成 `\n`（见 `forge-abi/tests/common/mod.rs::check_golden`，
  它现在按这个口径比，并有 CRLF 变异实测）。
- `forge-rustc` tests require nightly Rust with `rustc-dev` component
- **构建报 `os error 112`（磁盘满）**（2026-10-11 实测）：`target/debug/incremental`
  会涨到几十 GB（本机实测 **44.7 GB**，`target/` 共 73 GB），D: 被写满后
  `cargo build/test` 报 `failed to write query cache … 磁盘空间不足`。
  处置：删 `target/debug/incremental`（安全——只是缓存，代价是下次全量重建），
  并给后续构建设 `$env:CARGO_INCREMENTAL='0'`（不再生成这类增量缓存）。
  这也是"门禁中途红、报错却指着 query-cache"的根因，别误判成代码问题。
- **JIT 集成矩阵**（`forge-tests/src/jit_matrix.rs`）：架构无关、一次编写——
  用例**零 ISA 引用**，ISA 只存在于薄 runner（`isa/<name>/` 绑定机器 +
  能力集 `Capabilities`，riscv64 未来接入复用）。当前 **x86 197 passed /
  3 skipped**（本机 2026-10-01 实测 `cargo test -p forge-tests --lib
  jit_matrix_x86 -- --nocapture`）、**riscv64 136 passed /
  64 skipped，0 failed**（QEMU 通道；2026-10-01 本机实测
  `cargo test -p forge-tests --lib jit_matrix_riscv64 -- --test-threads=1
  --nocapture`，**需 `--nocapture` 才看得到计数**；浮点参数真执行靠
  `Executor::exec_f64_args` + crt0 的 `fmv.d.x/fmv.w.x` 装载）：整数/浮点/调用/
  向量/饱和/指针转换/undef/poison/GlobalAddr/原子（AtomicRmw/Cmpxchg）/GEP/Nop/
  混宽整数算术与比较；V256 用例在无 AVX 机器
  自动 Skip（**哪些用例 Skip 及原因**经 `FORGE_JIT_EVENTS` 的
  `MATRIX-SKIP`/`MATRIX-FAIL`/`MATRIX-SUMMARY` 事件可核对——libtest 吞掉通过
  测试的输出，否则 CI 上不可见）。`CaseKind::{I32/I64/F64/Bool/Block/Args/
  F64Args/Module/CompileOnly}`；`ops` 未覆盖 → Skip（不失败，实现后自动转绿）。
  **两条矩阵是两套能力集，改类表/值池/ABI 必须都跑**（2026-09-13 实测：x86 全绿
  而 riscv 的 7 个 fcmp 错值，正是 riscv 通道抓到的）。
  测试入口：`cargo test -p forge-tests jit_matrix_x86`。
- **真实汇编语料测试台**（`crates/tools/forge-tests/asm/`，v20 V9）：三档测试二进制
  `--test asm_parse`（上游语料逐行喂 `parse_insts`）/`--test asm_encoding`（上游注释里的
  `encoding: [0x…]` 与我们 `encode()` 的字节逐条对拍）/`--test asm_exec`（`asm/exec/**`
  的真语法叶子函数小程序**真跑**：x86 原生、riscv64/aarch64 走 QEMU，缺通道则
  `ASM-EXEC-SKIP` 说明原因，不假绿）。解析档按四桶归因（`Parsed`/`NoPrefix`/
  `TailMismatch`/`CorpusOnly`）；**门禁 = `asm/ratchet/<isa>.txt` 计数棘轮**（逐项相等，
  两个方向都红；重刷 `$env:FORGE_ASM_WRITE_RATCHET = "1"`，刷完必须看 diff）。
  桶判定不信"首词 = 助记符"，只问生成物的**线性扫描探针** `could_be_instruction`
  （见 Key Architecture Rules §5）。事件走 `FORGE_ASM_EVENTS`、记分板落
  `target/asm-suite/<isa>.json`；语料出处/许可/摘要见 `asm/PROVENANCE.md`，
  边界与已知缺口见 `asm/README.md`。
- **rust-analyzer 假阳性（v18 S10d 已修）**：`isa_from_file!` 调用行上曾报成对的
  `expected expression` / `expected R_PAREN`（riscv64 15 对 / arm64 4 对 / 夹具 9、6 对，
  **x86 0**）——那是"整份生成物作为宏展开结果"撑出来的 RA 展开管线问题（生成物用 RA 自己的
  解析器解析零 `ERROR` 节点，`rustc`/`clippy`/门禁一直全绿）。修法 = **生成物落 `$OUT_DIR`
  文件 + 展开只剩一句 `include!`**，文件由宿主 build script 预生成（见下一条）。实测四类
  文件 **68 条 → 0**，且没有引入新的 unresolved import（这个反面指标必须一起看）。
  **改用 `include!` 后有两条实测约束，别再试别的路子**：① RA 只加载"分析开始前"就存在的
  文件——宏在展开期写出的文件 RA 报 `failed to load file`（文件确实存在），随后生成模块的项
  在 20+ 个文件里全变 unresolved；② `TokenStream::to_string()` **上下文相关**（proc 宏里是
  rustc 美化打印、普通二进制里是紧凑打印，x86 2.66 MB vs 1.34 MB），两侧都写会互相覆盖、
  每轮重编。别再试 span 卫生性（`Span::mixed_site()` 实测**无效**：诊断逐文件一模一样）。
  全部证据、二分方法与踩过的坑见
  [`docs/guides/rust-analyzer-notes.md`](docs/guides/rust-analyzer-notes.md)（含"字符串续行
  被判 `Invalid escape`"这一类已修项）。一切以 rustc 与门禁为准。
- **生成物落盘（v18 S10d）**：`isa_from_file!` 展开成
  `include!(concat!(env!("OUT_DIR"), "/forge_gen_<模块名>_<参数哈希>.rs"))`；文件由
  `forge_isa_dsl::pregenerate_host()`（宿主 build script，扫 `src/ tests/ benches/
  examples/` 里每一处调用）写。三条不变量：① 文件名**只由参数**决定（改谱后路径不变，
  同一份谱的不同变体落到不同文件）；② 文件里带 `#[allow(warnings, clippy::all)]`
  （生成物不再享受"宏展开不 lint"的豁免，否则 `-D warnings` 门禁多 223 条风格警告）；
  ③ **单一写者**（只有 build script 写，宏侧只发 `include!`，文件缺失 fail-closed 报
  接入两步）。`forge_isa_dsl::expand_file` 仍返回**完整 token 流**，CLI 与各生成物守卫
  测试继续用它；守卫 = `gen_file` 的 4 条单测（展开形状、文件名只由参数决定、落盘件是
  合法 Rust 文件、宿主扫描能找到调用）。
- **谱内测试向量（`[[vectors]]`，v19 V3）**：五种形态——`{asm, bytes}` 正向（`assemble → encode`
  逐字节相等 + `decode` 吃满再编码一致）、`{asm, error = "<子串>"}` 汇编/编码负向、
  `{bytes, error = "DECODE"[, partial = N]}` 解码负向（`partial` 钉 `decode_partial` 的消费量，
  截断输入合法）、`{bytes}` 解码正向、`{asm}` **闭环**（只断言编解码/文本闭环稳定，不比字节）。
  形态在解析期校验（`validate_vectors`），用例发射成 `__spec_tests::spec_vector_<下标>`
  （条数进 `SPEC_VECTORS`）。**别再手抄 Rust 黄金字节表/往返清单**——迁进谱里
  （三份发行谱 2026-10-08 实测共 **505 条**：x86 154 / riscv64 152 / arm64 199 ——
  v19 V3 迁移当时是 363 条，之后随覆盖补齐增长；迁移前后字节集合的规范化 sha256 相同）。
  校验规则：同一条 `asm` 给出两种期望字节才算错，完全相同的重复允许。
  守卫 `crates/frontend/forge-isa-dsl/tests/vectors.rs` 与 `tests/determinism.rs`。
- **ISA-DSL 工具链**：`cargo run -p forge-isa -- validate|insts|explain|diff|schema|fmt|test|lint|abi <谱.toml>`——
  **`abi`（v20 A1）** 是**调用约定的静态体检**，与 `lint` 分工不同：`lint` 查"谱自己写了却不
  用/自相矛盾"，`abi` 查"谱的能力 × 使用者的约定数据"——`forge-isa abi list` 列内置约定/绑定、
  `abi check <谱>` 用 `forge_isa_dsl::abi_view` 的能力视图跑代表签名（**硬错**：名字/约定写错，
  退出 1；**缺口**：这台机器做不了，fail-closed，默认只报 `⚠ GAP`、`--strict` 才影响退出码）、
  `abi plan <谱> --conv <名> --sig "i64, f64 -> i64"` 打印一份 `AbiPlan`。
  实测（2026-09-24）：x86 `win64`/`sysv64` 与 riscv64 `lp64d` 各 15 条代表签名**全绿**；
  arm64 `aapcs64` **6 条缺口**（谱里没有 FPR 寄存器组 ⇒ `float`/`ret_float` 池缺，
  与矩阵 175 条 skip 同源）。参考文档 `docs/reference/calling-conventions.md`，
  方案与分期 `docs/archive/calling-convention-redesign-plan.md`。
  其中 `lint`（v19 V4a–V4c）是**静态体检**：报"写了却用不上 / 自相矛盾 / 宿主覆盖不到"的声明——
  未用的 `[operand.<名字>]` / `[form.<名字>]`（v21 W3/W10 的现名）、`[conventions.bitfields]`、
  **逐指令视图**里的位域重叠
  （`LINT-BITFIELD-OVERLAP`；按整张位域表判会把 riscv `shamt5`/`shamt6`、arm64 `op6`+`imm26`、
  全字常量 `word` 这类"同一批位的多种解释"全判成误报），退出码同 `validate`；**三份发行谱必须零结论**
  （守卫 `crates/frontend/forge-isa-dsl/tests/lint_shipped.rs`——真阳性请修谱，别改快照；V4c 就据此
  修掉 arm64 `idx3`×`op8` 的 bit24 重复声明）。两个可选档：`--ops <宿主 op 表.toml>` 报**能力缺口**
  （三类分开：终结指令/宿主管线直查不算缺口，只有真缺口报；口径 = 计划 §10.3，实测
  x86 `100/6/8/3`、riscv64 `67/6/8/36`、arm64 `8/6/8/95`——"宿主管线"8 条里含 `VaStart`
  （变参 V2，**生成器按能力角色发**，谱里同样没有 `[[lower.<op>]]`），宿主 op 表是宿主数据、
  DSL 不留第二份清单）、
  `--refs` 报"声明了却没被任何模板行首引用的 `ref`"（**opt-in**：实测 x86 5 / riscv64 19 /
  arm64 27 条，多为给未来 lowering 预留的多态名，属作者意图，不进默认档）、
  `--bits` 报"指令字里没有位域覆盖的位段"（**opt-in**，只对定宽 ISA；实测 x86 0 /
  riscv64 **13** / arm64 **262** —— 比 W0 的 3 / 67 长，原因是 v21 W2 之后 **form 声明但
  "本指令未绑定"的槽不再算该指令的覆盖**（口径变化，见执行清单 W2 的如实记录），
  逐条核对为保留位或固定位型 ⇒ 评审清单；依据它修掉 riscv AMO/LR/SC 的 `aq`/`rl` 两个字面 0 位）、
  `--suggest` 报"同一 op 发射形状相同、可合并为 `vary` 的族"（**opt-in 只建议**：
  x86 55 / riscv64 **31** / arm64 7 —— riscv 比 W5 前的 28 长，是族合并之后的余量）。
  三张清单都由 `crates/frontend/forge-isa-dsl/tests/lint_shipped.rs` 的快照钉住（全绿 = 与快照一致）；
  **数字随口径变化，改动前先看那个守卫的注释**。
  不接后端就能校验（全部诊断 + 行:列；`--strict-overlap` 另报 lowering 的**部分重叠**（`DSL-OVERLAP`）
  ——该档默认关：实测三 ISA 67 条全是合法的"特化 + 兜底"，它只是评审清单，清单快照在
  `crates/frontend/forge-isa-dsl/tests/strict_overlap.rs`）、看**展开后**的指令与生效编码键、查单条指令的
  模板 provenance（哪个模板哪一行）、两份谱的规格 diff（迁移前后对照）、打印/写出 JSON
  Schema、把多文件谱 `fmt` 折叠成单文件（v18 S7d）。`--json` 机读；退出码 0/1/2。
  **变体投影（v19 V5）**：`validate|insts --params xlen=32` 走只读投影——`[meta].variants`
  声明参数域、六处声明（`[inst.<名字>]`/族行/`[machine.text]`/`[machine.spill.<模板名>]`/
  `[asm.pseudo.<名>]`/树型匹配）可标
  `only_variants`，投影丢声明 + **连带丢引用不到指令的 `[[lower.<op>]]`** + `{参数名}` 替换；
  `insts` 第一行打印投影账目（`--json` 里 `projection`）。宏侧同参数进生成物文件名哈希。
  **默认档逐字节不变**（不传参数不过滤）；`[machine.spill.*]`/`[machine.text]` 等结构件引用被投影掉的指令
  却**没标** `only_variants` ⇒ 报错（结构件要作者显式变体化，见 `docs/reference/isa-dsl.md`
  「变体参数」；守卫 `tests/variants.rs`）。
  实现 = `forge-isa-dsl::report` + `::schema` 投影层 + CLI 薄层，复用
  `collect_inst_infos` 的"form 预设 ⊕ 指令覆盖"判定（不重复实现）。
  **三方针守卫必须改三处一起改**：schema 表（`src/schema.rs`）↔ `dsl/model.rs` 结构体字段
  ↔ `docs/reference/isa-dsl.md` 的「键总览（速查表）」区段 + 签入的 `isa-dsl.schema.json`
  （`cargo test -p forge-isa-dsl --test schema_guard` 全钉住；重新生成 schema 用
  `cargo run -p forge-isa -- schema --out isa-dsl.schema.json`）。
  三条要点：① schema 里的键必须是**用户在 TOML 里实际写的键**——字段名与 TOML 键不同时以
  `#[serde(rename = "…")]` 为准（`reference`/`ref` 曾因此让编辑器把 `isa/x86.toml` 的
  35 处 `ref` 全部标红）；② `schema_guard.rs::shipped_specs_only_use_schema_keys` 直接拿
  `isa/*.toml` + 夹具当输入，任何"schema 与真实谱不符"都会红；③ 自由表（`fields = {…}`、
  `[family.<名>].body`、`when = {…}`）在 schema 里没有子约束，守卫也**不**下钻它们。
- **多文件谱（v18 S7d）**：`include = ["…"]` + `[[override]]` 由 `forge-isa-dsl::loader`
  消费（按块合并；数组节按 include 序追加、重复 `[表头]` 视为续写、同名标量冲突报错）；
  它是所有公开入口（`isa_from_file!`/`expand_file`/`validate_file`/CLI）的必经之路，
  诊断按来源文件映射，生成物对**每个来源**登记 `include_bytes!`。`include`/`[[override]]`
  是组合键（合并后不存在），裸文本入口带它们会明确报错。
- **生成字段名 = 操作数声明的名字（v22：声明就写在 `asm` 里）**：
  `asm = "add {dst:r:out}, {src:r}"` ⇒ `Inst::Iadd { dst, src }`（更早的 `ops`/`bind` 两个键
  效果相同，v22 起**已删除**：写了报未知键并给出"你是不是想写 X"的建议）；
  `[form.<名字>].fields` 的字段名（`rd`/`rs1`）
  与变长 ISA 的语义名（`dest`/`cond`）**只是内部编码键**（字段的编码落点由 `fields` 展开派生，
  不是用户面键；另有 modrm 角色/立即数
  表），不得再泄漏成字段名。关键字 → `r#type`、数字开头 → `_8bit`（最小归一，不做语义改名）；
  守卫 `crates/frontend/forge-isa-dsl/tests/field_names.rs`。
- **v22 现行面速查（2026-10 落地，照此写谱）**：`[reg.<名>]`（宽度只在名字里）、
  `[operand.<名字>]`、`[enum.<表名>]`、`[form.<名>]`（表头即形式名，字段就地 `fields`）、
  `[inst.<名>]`（表头即指令名）、`[family.<名>]`（唯一复用机制）、
  `[pred.<名>]`（命名谓词）、`[asm.pseudo.<名>]` / `[asm.immfn.*]` /
  `[asm.reloc.<名>]`、`[machine.stack|text|classes|spill.<模板名>]`、`[lower.<名>]`。
  **操作数声明与字段绑定就地写在 `asm` 里**（v22 核心人体工学改造）：`{名字:槽[:角色]}` 声明
  一个操作数、`{@字段:槽[:角色]}` 额外把它绑到字段（`@[字段]` = 内存形态）；
  打印序 ≠ 编码序时用 `{名字:槽#序号}`（一条指令里**全标或全不标**）。
  旧键 `ops`/`bind` 与 v21 的占位修饰（`{n?}`/`{n:affix}`）**已删**：前者仍在解析器里兼容
  （`#[serde(skip)]` 内部载体，`INTERNAL_FIELDS` 守卫钉住"不进 schema"），后者连解析一起删了；
  `modrm` 保留——扩展码（`/4` 这类**整数** `reg`）只能由它表达。
  **仍是数组节的**：`[[lower.<op>]]`（同名可多块）、`[[vectors]]`、`[[override]]`。**重名由 TOML 自身拒绝**
  （表头即名字的表）；未知键一律 `deny_unknown_fields` 报错（并给出"你是不是想写 X"的建议），
  没有兼容层。
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
  `spec_tests = false` 关掉，另开一个用例二进制打开（见 `tests/spec_tests.rs`）。
  **谱内派生枚举器（v19 V3d）**：同一模块还导出 `all_insts() -> Vec<(名字, Inst)>` 与
  `SPEC_INSTS`（每条指令 × 宽度视图 + 每个 imm 槽的 `lo`/`hi` + 每个 mem 槽的
  `disp=8`/`disp=-8`/`index+scale=4`；取值与生成期自测同源 `sample_operands`）——
  宿主侧"全指令编解码往返"**不要再手抄 `all_insts()`**（x86 曾手抄 650 行且谱加指令时不
  自动跟上）；由 `crates/backend/forge-codegen/src/isa_roundtrip_guard.rs` 遍历三谱的
  1263/341/360 条跑字节闭环 + 覆盖清点。`decode(encode(x)) == x` 不在这条守卫里（别名撞车
  按声明序首匹配，本就不成立），仍由各 ISA 测试的"规范指令"小清单守。
  覆盖守卫 = `crates/backend/forge-codegen/src/spec_coverage_guard.rs`（钉死指令总数
  x86 265 / riscv64 119 / arm64 110、零跳过、文本歧义名单；v20 V10 起歧义键 = 字面段 +
  **按模板占位符序、不带操作数序号**的打印描述——只有占位符顺序相反的 `add [mem], r`
  对 `add r, [mem]`，以及"序号不同但渲染文本确实一样"的 `MOV_R_RM` 对 `MOV64_RR`，
  各修掉一类误判；**它是 `#[cfg(test)]` 项，
  必须放在 `lib.rs` 末尾**——写死宽度守卫按第一个 `#[cfg(test)]` 截断扫描）。
- **内存操作数的文本形态（v20 V10）**：`[conventions.mem] templates` 是**列表**——
  **第 0 条 = 反汇编渲染形态**（渲染必须唯一，否则输出会随解析尝试顺序漂移），其余是
  解析专用备选（按列表序试，第一条整条走通的赢）。组件 `{base}`（写了就必须有基址）`{index}`
  `{scale}` `{disp}` `{size}`；`{size}` 吃 `size_keywords` 里的任一关键字（大小写按
  `[meta].mnemonic_case`），**值不进 `MemRef`**——宽度由槽/`opsize` 决定，尺寸前缀只是
  给人读的冗余提示（`mov QWORD PTR [rsp-16], rax` 与 `mov [rsp-16], rax` 编出同样的
  字节），渲染输出空串所以 `disassemble → assemble` 照旧闭合。**紧邻 `{size}` 的字面量
  不是它的条件前缀**（否则 `qword ptr [rax]` 的 `[` 会随尺寸消失）。**`MemRef.base` 是 `Option<Reg>`**
  （v20 V10）：**写了 `{base}` = 这条写法必须有基址**（缺了整条不匹配），**没写 `{base}`
  = 这条写法没有基址**（`base = None`，绝对地址），此时其余组件按**必需**处理。x86 的
  无基址是 `mod=00` + `SIB.base=101` + disp32，编解码两侧对称；只写 `[base]` 的简写
  表示不了无基址 ⇒ 解码显式拒绝（不拒会把绝对地址当 `[RBP]` 解出来还少读 4 字节
  disp32，把真正的 MemRef 形式挤掉）。`sub rax, [0x12345678]` 的往返由
  `encoder_fuzz_tests` 覆盖。规范见 `docs/reference/isa-dsl.md` 的
  「`[conventions.mem]` — 内存操作数的文本形态」。
- **`opsize` 指向 1 字节槽 ⇒ 解码不发守卫（v20 V10）**：`__opsize` 只可能取 2/4/8
  （由 66 前缀与 REX.W 扫描决定），8 位操作数没有 opsize 前缀可查——显式
  `opsize = "s0"` 指向单类 1 字节槽时会生成 `__opsize == 1` 的解码守卫，恒假 ⇒ 解码
  永远失败（缺省分支的 `find(|w| *w == 2 || *w == 4 || *w == 8)` 恰好把这类过滤掉了，
  所以以前没暴露）。实测 `00 00`（`add [rax], al`）由 `isa_roundtrip_guard` 抓到。
- **立即数字面量的两种读数（`wrap`，v20 V10）**：立即数字段是 W 位**位模式**，
  `0x90909090` 与 `-1869574000` 是同一批字节。槽上写 `wrap = true`（仅 `kind = "imm"`
  且 `width < 64`，声明期校验）后，汇编器额外接受另一种读数并**规范化**回 `signed` 的
  读数（`OperandSlot::imm_accept_range` / `imm_wrap_modulus`）；**规范值域 `imm_range()`
  不变**——解码符号扩展、反汇编渲染、生成期自测的 min/max 边界都照旧，所以
  `add rax, -12` 的文本与字节逐字节不变。超出 W 位仍然报错。候选特异性与类型签名按
  **接受**域计（口径 = "能接受多少输入"）。x86 `imm32` 打开它收下 `add eax, 0x90909090`。
- **生成物形状表（v18 S8a）**：`impl MachineInst for Inst` 的 8 个查询方法
  （`uses`/`defs`/`use_constraints`/`def_constraints`/`effects`/`reg_field`/`set_reg_field`/
  `is_reg_field_settable`）在生成物里**不许再逐指令展开**——它们读每变体一行的
  `__SHAPES`（+ `__SLOT_CLASSES`/`__EFFECT_SETS` 两张去重表）与 3 个访问器
  （`__shape`/`__reg_slot`/`__set_reg_slot`，见 `dsl/codegen/machine.rs`）。
  守卫 = `crates/frontend/forge-isa-dsl/tests/machine_shape_table.rs`（钉"8 个方法体零
  `Inst::` 臂" + "形状表与两个访问器覆盖同一批变体、行数 = 变体数"）；实测数字见
  `docs/performance/bench_baseline.md` 的「S8a 落地度量」（`tm` −12~26%，整模块 −7.5~12.9%）。
- **搬运族派生（v20 V8，取代 v18 S9 的"角色带宽度"与 v20 V7 的类间搬移角色）**：
  **搬运指令不再写角色**——`gpr_mov`/`gpr_mov_imm`/`ret_mov`/`fpr_mov`/`vec_mov`/
  `fpr_to_gpr_mov`/`gpr_to_fpr_mov`/`wide_vec_load`/`wide_vec_store` 九个角色与
  `RoleDecl.bits` **已整条删除**。ISA 只在指令上写 `data_width = <位>`（**数据宽度**，
  与 `width` = 指令字长是两件事），其余全部由操作数结构派生：
  ① 方向 = 槽角色（`out`/`inout` = 目的、`in` = 来源）；② 寄存器族 = 槽的
  `class`（`RegClass` 自带 `GPR`/`FPR`/`VEC`/`KReg`）；③ 立即数/寄存器/内存 =
  槽的 `kind`。选指令 = 按 **(目的形状 × 来源形状 × 位宽)** 查表（`dsl/codegen/moves.rs`
  的 `MoveTable`，**唯一实现**，生成器四处发射点与 `abi_view`/生成物 `role_bits` 同源）：
  寄存器/立即数来源取**最窄覆盖者**（整寄存器搬移对更窄的值同样正确），涉及内存取**精确**
  宽度；同形状同宽度多条候选 ⇒ **生成期报错并列出候选**（不设"钉选"注解）；一条都没有 ⇒
  生成物里 fail-closed。三操作数形态（riscv `fsgnj.d rd, rs, rs`）的**第三槽自动填成源**，
  store（目的在内存，`mem:mem:out`）同样成立。**为什么宽度仍要人写**：`MOVSS`/`MOVSD`
  共用 `fpr16` 槽、`FSGNJ_S`/`FSGNJ_D` 共用 `fpr4` 槽——槽分不出 32/64，宽度不在结构里。
  **为什么 `stack_arg_*`/`callee_save|load` 仍是角色**：它们与彼此形状一模一样，只有用途
  不同（用途不在操作数结构里）。守卫 `crates/frontend/forge-isa-dsl/tests/move_derive.rs`
  （位宽是数据 / 第三槽 / 缺搬运 fail-closed / 歧义报错 / 坏形状报错 / 发行谱零搬运角色）与
  `crates/backend/forge-codegen/tests/bank_mov.rs`（合成约定"浮点判给 int 池"走真实管线）；
  规范见 `docs/reference/isa-dsl.md` 的「`roles` 语义角色」。
- **谓词属性：按需 + 无名字分派（v18 S8b-1 / S8d）**：`gen_lowering_attrs` 生成的属性源
  在生成物里**只发射一次**（放在 `lower_inst` 的 `match op` 之前；跟着 op 臂走 = 每个
  op 重复一份 2.5 KB，x86 曾 100 份 = 243 KB），并且是
  `__AC<'x>`（`done` 位图 + `v: [Option<i64>; 9]` + `op`/`args`/`results` 共享借用）
  加每个核心属性一个 `#[inline] fn <名>(&mut self, ctx: &LowerCtx) -> Option<i64>`
  （**没算过才算**，因此"每次 `lower_inst` 每属性最多算一次"仍然成立，但**不需要的属性
  一次都不算**——旧实现每次都把 9 个全算一遍）。谓词名在**生成期**解析成
  `__ac . <属性> (&*ctx)`（见 `compile_pred_guard`/`attr_expr`），**不得**再引入
  `__attr(name)` 之类的运行时字符串分派；`[pred.<名字>]` 派生属性也在生成期展开。
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
  `[machine.stack]`：`slot`/`align`/`fp_save`（v21 W9：从 `[stack]` 迁入，**单位是位**）；
  栈参数布局（`callee_base`/`caller_base`/`first_offset_slots`/`stride_slots`/
  `shadow_bytes`）**已随 `[abi]` 整节删除**（v20 A5-3），归宿主的 `AbiRules`；优先级 显式键 > 派生 > **报错**）。定宽指令字在生成代码里是
  **字节数组**（`[u8; ceil(位/8)]` + `__place`/`__bits`），位域可落在机器字之外
  （100 位字夹具 = 13 字节）；唯一边界是**单个位域 ≤ 64 位**（值承载在 u64/i64）。
  生成期用 `__DEFAULT_GPR_CLASS`/`__ADDR_CLASS`/`__SLOT_BYTES` 等常量，宿主用
  `TargetRegInfo::{addr_class, value_gpr_class,
  value_fpr_class, slot_bytes, vector_tiers, class_for_type}`——**不要**再写
  `RegClass::GPR64`/8 字节缺省。1 字节寄存器 ISA 夹具 =
  `crates/backend/forge-codegen/tests/isa/demo8.toml`；指令字宽夹具 =
  `tests/isa/demo_inst{8,12,100}.toml`（由
  `tests/common/mod.rs` 用 `isa_from_file!(…)` 宿住，
  **不进库本体**；用例在 `tests/demo8_tests.rs`）；反回潮守卫 =
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
  `docs/archive/forge-rustc-vec_push-plan.md` §9.1/§9.2（失败轮产物须与本机产物做
  行为/字节对照：一致 ⇒ 宿主环境性；错码 ⇒ 转 regalloc 关联法）。
