# CHANGELOG

<!-- markdownlint-configure-file { "MD013": { "line_length": 512, "code_block_line_length": 512, "heading_line_length": 512 }, "MD024": false } -->
<!-- 文件级豁免原因：历史条目按单行记录（最长 ~440 列），且多年条目同置 [Unreleased] 下，
     版本子节用 `### Added (日期)`，同层 Fixed/Changed 标题按惯例重复——均为 CHANGELOG
     结构与单行惯例，不按正文 120 列规则折行；重构留待 changelog 整顿时处理。 -->

All notable changes to the `code-forge` project will be documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.0.0/).

## [Unreleased]

### Fixed (2026-10-07) — `unit` 在**位切片**编码路径被跳过（分段立即数 + 单位缩放会写错字段）

`[operand_slots].unit`（源值单位）原先只在**单字段**编码路径生效：`encode = "slice"` 的
**多字段**分支直接 `let __mv = *#fid as u64;`，把**源值**当字段值切片。于是任何"分段立即数
+ 单位缩放"的形态都会写错——实测 ADR/ADRP 家族里 `adrp x0, #4096`（页单位）编成
`[0,128,0,144]`，上游是 `[0,0,0,176]`（差一整个字节）。

修法（与单字段路径**对称**，两行概念）：

**编码侧**：切片前先把源值除以单位（`imm` 槽查整除并报错；`label` 槽只移位不查——它塞的是
  标签 id 占位、随后由 reloc patcher 改写，查整除只会误报，口径与单字段路径一致）；
**解码侧**：拼回各段后乘回单位（缺这一步则"编码除了、解码没乘回来"的往返字节不稳）。

同时把**生成期采样**对齐到单位（`spec.rs`）：`imm` 槽的 `lo`/`hi` 采样值必须取单位的整数倍
（朝零对齐，仍落在原值域内），否则边界自测会构造出"编码期整除检查必然拒绝"的用例。

**证据**：应用本修复后，ADR/ADRP 的 4 条上游字节向量（`adr x0, #0`=`0x10`、`adr x0, #1`=`0x30`、
`adrp x0, #0`=`0x90`、`adrp x0, #4096`=`0xb0`）**逐字节全过**，往返稳定；`cargo test -p
forge-codegen --test spec_tests` 与全工作区门禁（`cargo test --workspace --exclude forge-rustc`）
均 **exit 0**。**未随本提交带夹具用例**：给 `demo_inst12` 加"切片 + unit"夹具时，该夹具的 imm
文本形态与模板不合（`ldw r0, #12` 报 no matching），因此夹具已撤，本条只落"修复 + 采样对齐"，
夹具与 ADR/ADRP 的谱侧落地留待下一步（详见 `crates/tools/forge-tests/asm/README.md` 的勘察记录）。

### Added (2026-10-07) — aarch64：`mov` 立即数的**符号/表达式**形态（8 条）+ 同型指令改用模板（谱减 ~99 行）

**形态**：`mov x0, #:abs_g0_s:sym`、`mov x0, 1b - 0b` 这类写法。**零新增概念**：槽复用 movz/movk/movn 重定位那批已有的 `imm16g0..g3`（按 hw 分组、各带 `imm_fns` 过滤 + `require_symbol`），编码沿用 `MOVXIMM`/`MOVWIMM`（`mov <Rd>, #imm` = `movz hw=0`）的 form/opcode，模板**不带 `#`**（修饰文本/符号文本自带）以与数值形态保持形状隔离。效果：`parsed` 1922 → **1924**、红 1376 → **1369**；**`mov-expr-as-immediate.s` 与 `mov-unsupported-expr-as-immediate.s` 两个文件转绿** ⇒ 全绿集 **123 → 125**（PROVENANCE 127 行）。

**同型指令改用 `[[templates]]` + `rows`**（按仓库既有风格，紧凑且更省体积）：把最近三批**34 条**逐条 `[[instructions]]` 压成 **21 个模板**（`mov` 立即数 1 个；符号/表达式形态按 hw 8 个；`movz`/`movk`/`movn` 的 `abs_g*` 按 (助记符, hw) 12 个），`isa/arm64.toml` 减少约 **99 行**。**展开后的指令名与编码完全等价**——证据就是两处计数钉死值（arm64 **454**、枚举器 **2213**）与文本歧义名单（38 条）在重构前后**一字未变**、codegen 全绿。同时删掉三个一次性生成器（`gen-aarch64-mov-{reloc,imm,imm-sym}.mjs`）：谱里已是紧凑模板形式，留着只会重新吐出冗长版本。

### Added (2026-10-07) — 汇编器：数字局部标签 `1b`/`1f`（GNU as / LLVM MC 语义，ISA 无关）

设计取"通用 + 人体工学、零冗余"：

- **词法器**把 `数字 + b/f` 吐成一个普通 `Ident`（判据：其后不再接标识符字符或 `.`）——于是下游的**符号引用收集 + 立即数修饰**原样可用（`mov x0, #:abs_g0_s:1b` 里的 `1b` 就是一个普通符号名），**不需要新的操作数种类，也不需要新的槽键**；
- 两遍布局里数字标签**单独收集**成"编号 → 定义位置列表"（同一编号可反复定义），回填时 `1b` 取**最近的上一个**（允许与分支同址：`1: b 1b`），`1f` 取**下一个**；找不到 → `UndefinedLabel`（准确诊断 ⇒ 语料侧归入"上下文不足"桶而不是缺陷）；
- 与 NEON 元素后缀**互不干扰**：`v0.16b` 那类以 `.` 开头、由既有的 dot-ident 规则吃掉，不会走到这条分支。

效果：**全绿集 117 → 123（+6，六个都是 riscv64 文件）**——能力坐在共享汇编器里、与 ISA 无关，所以 riscv64 一起受益（"通用"的直接回报）；PROVENANCE 125 行；riscv64 棘轮随之刷新。aarch64 侧那 5 个含局部标签的文件仍被**别的缺口**挡着（`arm64-adr.s` 缺 ADR/ADRP 指令本身；`mov-expr-as-immediate.s` 缺 `mov` 立即数的**符号/表达式**形态）——是下一轮的靶子，本轮的能力是它们的前置件。

### Added (2026-10-07) — aarch64：`mov <Rd>, #imm` 立即数别名（2 条）

`mov x0, #0` / `mov w0, #imm`——上游把它展开成 `movz #imm, lsl #0`（`mov x0, #0` = `0xD2800000`）。与寄存器形态 `mov {dst}, {src}` **形状不同**（带 `#`）⇒ 共存不遮蔽，正是前几轮反复验证过的"同关键字、异形状"路子。生成器入库 `crates/tools/forge-tests/asm/gen-aarch64-mov-imm.mjs`。守卫同步：`spec_coverage_guard` arm64 444 → **446**、`isa_roundtrip_guard` arm64 枚举器 2153 → **2165**；全工作区门禁 exit 0。

### Added (2026-10-07) — aarch64：`movz`/`movk`/`movn` 的 `abs_g*` 类重定位 24 条

`movz x2, #:abs_g0:sym`、`movk x3, #:abs_g1:foo` 这类写法。关键点是 **`hw` 由修饰名决定**（`abs_g0..g3` ⇒ `hw` 0..3），所以四个**按 hw 分组的符号槽**各自用 `imm_fns`（上一轮落的新键）过滤修饰表——过滤表列到 20/20/18/6 个名字（带 `#` 与不带 `#` 两种写法都列）；不这么分就会把 `movz … #:abs_g2:foo` 编成 `hw = 0`、**静默写错字段**。24 条 = 3 助记符 × 4 hw × {X, W}，opcode 照 `[[templates]]` 的 rows 抄（`MOVZX`=0x1A5 / `MOVZW`=0xA5 / `MOVNX`=0x125 / `MOVNW`=0x25 / `MOVKX`=0x1E5 / `MOVKW`=0xE5）；模板**不带 `#`**（修饰文本自带）以与数值形态保持"形状隔离"。生成器入库 `crates/tools/forge-tests/asm/gen-aarch64-mov-reloc.mjs`。

守卫同步：`spec_coverage_guard` arm64 420 → **444**、文本歧义名单 6 → **30** 条（12 组 X/W 同形、靠寄存器类分派，与既有 `LDRX`/`LDRW` 同一模式）、`isa_roundtrip_guard` arm64 枚举器 2009 → **2153**。本批是**纯解析**能力（新变体同 opcode、且 `require_symbol` 把关），既有字节对拍不受影响。

### Fixed (2026-10-07) — `[meta].comment_char` 是单字符 ⇒ A64 的 `/` 会把立即数里的除法截断

`strip_comment` 按**单个字符**切注释，而 A64/GAS 的注释是 `//`：`isa/arm64.toml` 写 `comment_char = "/"` 就使 `movz x0, #(32 / 2)` 从 `/` 处被截成 `movz x0, #(32` ⇒ 报"没有匹配的指令"。上游为此专门写了 `single-slash.s`，所以这条是可复现的语料证据而非猜测。修法 = 注释标记改成 **1–4 个字符的字符串**（`#` / `//` / `;`），生成物的 `strip_comment` 按**整串**匹配，并把 `isa/arm64.toml` 改成 `comment_char = "//"`。校验放宽到 1..=4 字符（原为"必须恰好一个字符"），文档同步。

效果：aarch64 `parsed` 1917 → **1918**、红 1429 → **1428**；`single-slash.s` 整文件转绿 ⇒ 全绿集 **115 → 116**（PROVENANCE 118 行）、淘汰 618 → **617**。

### Fixed (2026-10-07) — aarch64 FP 成对的前/后索引用不了：imm7 槽是无符号的

`stp d8, d9, [sp, #-64]!` 这类写法原先借用了**无符号**的 `imm12fp*` 槽 ⇒ 负偏移直接越界、报"没有匹配的指令"（`stp d10, d11, [sp, #16]` 这类正偏移则正常，所以只有前/后索引的成对形态受影响）。修法 = 镜像 GPR 成对的 `imm7x`/`imm7w`，补 `imm7fp4`/`imm7fp8`/`imm7fp16`（**有符号**、width 7、单位 = 元素尺寸）并替换 18 条 FP 成对的 imm 槽。

效果：aarch64 `parsed` 1909 → **1917**、红 1437 → **1429**；**`seh-optimize.s`、`seh.s` 整文件转绿** ⇒ 全绿集 **113 → 115**（PROVENANCE 117 行）、淘汰 620 → **618**；aarch64 棘轮随之刷新。

### Fixed (2026-10-07) — aarch64 FP unscaled 访存的 op8 用错了族（`LDURD`/`STURD`/`LDURS`/`STURS`）

这四条写的是 `0xFD`/`0xBD`——那是**无符号偏移族**（imm12）；unscaled 族是 `0xFC`/`0xBC`（`ldur d8, [sp, #8]` 上游 = `e8 83 40 fc`）。潜伏至今的原因是 FP 槽用 V 名、语料写 `d8`/`s7`，那批行从来没解析过 ⇒ 字节对拍档看不见它。**影响不止编码**：`LDURD`/`STURD` 带 `callee_load`/`callee_save` 角色、`[spill.FPR]` 也引用它们 ⇒ arm64 后端的 FPR 保存/恢复与 FP 溢出一直在按错族发偏移。修后用 4 条谱内向量钉住（字节取自语料的上游期望值）。

### Added (2026-10-07) — aarch64：FP/NEON 宽度视图 + SIMD&FP 访存全族（60 条）

**宽度必须由寄存器组承载**：FP 访存的助记符都是 `ldr`/`str`/`ldp`/`stp`，宽度只在寄存器名上；若 B/H/S/D/Q 同属一个组（都算 FPR(8)），`ldr s0,…` 与 `ldr d0,…` 的**文本形状完全相同**，分派会提交到先声明的那条（实测 S 被编成 D、Q 被编成 B）。做法：既有 `[reg.fpr8]`（V0..V31）作 **D 视图** + `D0..D31` 别名，另开 `fpr1`/`fpr2`/`fpr4`/`fpr16`（B/H/S/Q）；**`[meta].default_fpr_width = 8` 必须显式写**（否则"主 FPR 类"按最宽组取到 Q，ABI 找 `V0` 报 `UnresolvedReg`）；新增 `[spill.FPR16]`。

指令：单寄存器 B/H/S/D/Q × {无符号偏移, unscaled, 前索引, 后索引} × {ldr, str} + S/D/Q 成对 LDP/STP × {有符号偏移, 前索引, 后索引}（D 视图的无位移形态沿用既有 `LDURD`/`STURD`，避免同形重复）。**本批也抓到我自己的一处编码错**：Q（128 位）单寄存器的 opc 是 **11 载 / 10 存**（不是 D 的 01/00），照 D 写会让 `ldr q9, …` 变成 `ldr B9, …`——被新解析出来的语料行（26 条 FP 形态差异）当场抓出，改完 FP 形态差异归零。

效果：aarch64 `parsed` 1640 → **1909**、红 1706 → **1437**、编码 `checked` 785 → **945**、`known` 53 → **57**（+4 为新解析出的 `sp` 别名写法暴露的既有编码口径差异，非 FP）；全绿集 **112 → 113**（PROVENANCE 115 行）。守卫同步：`spec_coverage_guard` arm64 364 → **420**、`isa_roundtrip_guard` arm64 枚举器 1673 → **2009**。

### Added (2026-10-07) — aarch64：移位立即数 16 条（`add w3, w4, #1024, lsl #12`）

A64 的 add/subtract (immediate) 用 **bit22 = sh** 表示"左移 12 位"，且文本里的立即数**始终是未移位的值**（`#1024, lsl #12` ⇒ 字段 1024，槽的 `unit` 不受影响）。16 条 = 数值形态 8 条（`add`/`sub`/`adds`/`subs` × X/W，槽 `imm12u`）+ **重定位 + 移位** 8 条（槽 `imm12sym`，覆盖 `add x2, x3, #:lo12:sym, lsl #12`）。效果：aarch64 红行 **1734 → 1706**、`parsed` 1618 → **1640**、`corpus_only` +6。守卫同步：`spec_coverage_guard` arm64 348 → **364**、`isa_roundtrip_guard` arm64 枚举器 1577 → **1673**。

一处**语料侧**的 `known` +1（不是我们的回归，已登记进 `asm/README.md`）：`basic-a64-instructions.s:312` 的 `subs xzr, sp, #20, lsl #12` 我们编出 64 位形态（与文本里的 `xzr`/`sp` 一致），而那一行的 CHECK 期望是 32 位形态——语料抽取把 CHECK 行配对错了（该文件另有 `subs wzr, wsp, …`）。

### Fixed (2026-10-07) — 汇编器两遍布局把「第一个冒号之前的一切」当标签（A64 重定位族 382 行一直落红的根因）

生成的 `parse_insts` 按 `name:` 识别标签定义，但判据是"**见到冒号就切**"：`add x8, x8, :lo12:sizes` 因此被切成"标签 `add x8, x8,` + 指令 `lo12:sizes`"，报 `no matching instruction`。语料抽取侧本就有"名字不含空白"的守卫，生成侧没有——**这是 A64 重定位修饰族（382 行、47 个修饰名、十几个文件）从来没解析过的真正原因**，而不是"缺指令"。修法 = 同一口径的判据：名字非空且不含空白/逗号/括号/`#` 才算标签。修后 `add x8, x8, :lo12:sizes` 装配为 `imm: 0`（与上游未重定位字段 = 0 一致），未定义符号报 `UndefinedLabel`（准确诊断 ⇒ 归入"上下文不足"桶而非缺陷）。

### Added (2026-10-07) — ISA-DSL：`[[operand_slots]].require_symbol`（只收符号/带修饰的写法）

重定位修饰形态的符号位置必须开在 `unit == 1` 的槽上（校验硬要求），而立即数槽的候选**特异性按接受域**裁决——unit-1 槽比 unit-8 窄 ⇒ 优先命中，会把**数值写法**接走（实测 `ldr x0, [x1, #8]` 被编成字段 8 而不是 1，静默错编码）。`require_symbol = true` 让该槽在"没记到符号/修饰"时**直接不匹配**，数值写法自然落回数值槽；要求 `kind = "imm"` 且同时声明 `symbols = true`。判据在 `__imm` **内部**判（它成功时会提交并清空 `__SYMREF`，外层测长度增量恒为 0）。四处同步：`model.rs` 字段、`validate.rs` 校验、`schema.rs` + 重新生成的 `isa-dsl.schema.json`、`docs/reference/isa-dsl.md`（键表 + 专节）+ 单测 `require_symbol_validates`（四种情形）。另外：含 `require_symbol` 槽的指令是**仅装配**形态（渲染出的无修饰文本由数值兄弟接手），生成期自测对它们**跳过文本闭环**、只测编解码闭环。

### Added (2026-10-07) — aarch64：重定位修饰族 12 条（`add x0, x0, #:lo12:sym`）

ALU 立即数 8 条（`add`/`sub`/`adds`/`subs` × X/W）+ LDR/STR 4 条，槽是 `imm12sym`（unit 1 + `symbols` + `require_symbol`）。修饰名做成全局数据 `[[conventions.imm_fn]]` **98 条**（49 个名字 × 带 `#`/不带 `#`）——**一个变体收全部修饰**；`text` 里带 `#`、指令模板**不带**，使重定位变体的**文本形状**与数值变体不同（同一形状的多候选里分派会提交到排第一的那条，不按操作数失败回退；形状相同会互相遮蔽，实测 `add W0, W1, #0` 因此装配不出来）。

效果：aarch64 `parsed` 1572 → **1618**、红 1877 → **1734**、`corpus_only` +97、编码 `known` 仍 **53**（无字节回归）；**4 个文件整文件转绿**（`arm64-ilp32.s`、`basic-pic.s`、`jump-table.s`、`tls-add-shift.s`）⇒ 全绿集 **109 → 112**（PROVENANCE 114 行）、淘汰 624 → **621**；aarch64 棘轮随之刷新。守卫同步：`spec_coverage_guard` arm64 336 → **348**、`isa_roundtrip_guard` arm64 枚举器 1505 → **1577**。仍缺（留在红桶）：`#:lo12:sym, lsl #12`、`str q0, …`（FP 视图）、`movz/movk/movn` 的 `abs_g*`、`adrp/adr` 的 `:got:`、`ldr x0, =sym`。

### Added (2026-10-07) — aarch64：字节/半字 GPR 访存 18 条（`ldrb`/`ldrh`/`ldrsb`/`ldrsh`/`ldrsw`/`strb`/`strh`）

9 组助记符/类别 × {带位移, 无位移} = **18 条**（语料 117 行）。编码与既有 LDR/STR 同族：`size 111 V 00 opc imm12 Rn Rt`——`opc` 00 = store / 01 = 无符号载 / 10 = 符号载到 X / 11 = 符号载到 W；imm12 的**单位 = 访问宽度**（byte 1 / halfword 2 / word 4，按单位分成 `imm12b`/`imm12h`/`imm12w` 三个槽）。

**一处真错由字节对拍档当场抓到**：无符号偏移族（imm12）的 `[25:24] = 01` ⇒ op8 是 `0x39`/`0x79`/`0xB9`，我最初按 unscaled 族的 `0x38`/`0x78`/`0xB8` 写（`ldrb w4, [x3]` 我们 `0x…38` ≠ 上游 `0x…39`）；修正后该族 `known` 清零。顺带：`0xB9` 让 `ldrsw` 的无位移形态不再与既有 `LDURSW`（`0xB8`、`mode` 占 bit10 起 2 位）在**解码位 trie** 里重叠——生成期本会把这种结构冲突直接喊出来。谱内向量 5 条（生成器从语料 CHECK 行自动挑具体字节样本：`ldrb`/`ldrsh`/`strb`/`ldrsw`/`strh`），codegen 全过。

效果：aarch64 `parsed` 1547 → **1572**、`no_prefix` 14558 → **14322**、编码 `known` 78 → **53**；新认出的 236 行里 25 条整条通过、211 条**按归因落红**（地址形态/寄存器族尚未做，非回归）。取舍账本（留 109 / 砍 624）与两个棘轮逐字不变。守卫同步：`spec_coverage_guard` arm64 318 → **336**、`isa_roundtrip_guard` arm64 枚举器 1433 → **1505**。

### Added (2026-10-07) — aarch64：扩展寄存器操作数（48 条，300 行那一族）——关键字做成「命名立即数操作数」

`add x8, x8, w5, sxtw #2`、`cmp w0, w1, uxtb` 这类"第三个操作数带扩展关键字、可再带量"的写法此前整族落红。**不新增 DSL 能力**：关键字做成**命名立即数操作数**（`{ext}` → `option` [15:13]），量做成 `imm3` 操作数（[12:10]）——于是每 (助记符, 宽度) 只需 **4 条**（W 扩展 / X 扩展 × 带量 / 省略量），
6 助记符（`add`/`sub`/`adds`/`subs`/`cmp`/`cmn`）× 2 宽度 × 4 = **48 条**（若按"每个文本组合一条指令"要 192 条，靠把关键字与量做成操作数压到 4 条）。
编码与移位寄存器族**共用 `op8`**（0x8B/0x0B/0xAB/0x2B/0xCB/0x4B/0xEB/0x6B），靠 **bit21 = 1** 区分；`cmp`/`cmn` 用无 `rd` 的两条形式 + `fields = { rd = 31 }`。

两张命名表按**操作数类别**分：W 表收全部 8 个（`add w1, w2, w3, uxtx` 也合法——32 位视图下 uxtx 是恒等，这是语料暴露出来的）、X 表只收 `uxtx`/`sxtx`。
**字节证据**：8 个 option 的上游字节在语料里排成一组（`add w1, w2, w3, <ext>` 从 `41 00 23 0b` 逐项递增到 `41 e0 23 0b`，正好是 option 0..7），谱内一次加 **10 条向量**（含 X 形式 `add x1, x2, w3, uxtb` = `41 00 23 8b`）——**首跑全过**。

效果：aarch64 语料红行 **1872 → 1666**、`parsed` 1341 → **1547**，**`arm64-leaf-compact-unwind.s` 整文件转绿**（它最后一条正是 `add x8, x8, w5, sxtw #2`）——全绿集 **109 → 110**（aarch64 语料 30 → **31** 份）、淘汰 624（= `parsed = 0` 557 + 出红 55 + 字节差异 12）、aarch64 棘轮 `parsed` 267 → **356**。守卫同步：`spec_coverage_guard` arm64 270 → **318**、`isa_roundtrip_guard` arm64 枚举器 1049 → **1433**。

### Added (2026-10-07) — aarch64：寄存器偏移寻址（32 条）+ ADD/SUB 移位寄存器带量（8 条）

① **寄存器偏移**（`ldr x9, [x27, x6]`、`ldr w0, [x0, x0, lsl #2]`、`str w14, [x26, w6, uxtw]`、`str w13, [x27, x5, sxtx #2]`）：编码是 `size 111 V 00 opc **1** Rm option S **10** Rn Rt`——与 imm9 家族**共用 `op8 = 0xF8`(X)/`0xB8`(W)**，只靠 **bit21 = 1** 与 **[11:10] = 10** 区分（这两条固定位是 **19 条上游期望字节逐条核对**出来的）。`option`/`S` 与文本关键字的对应同样是**量**出来的：LSL = 3 / UXTW = 2 / SXTW = 6 / SXTX = 7，且 **S=1 ⇔ 文本带 `#N`，N 恒 = log2(访问宽度)**（X 形式 3、W 形式 2）
⇒ 每种组合都是一条**完整的文本模板**，不需要"可选片段"机制。索引寄存器的类由 option 决定（LSL/SXTX 收 X、UXTW/SXTW 收 W；`xzr`/`wzr` 走别名）。语料里出现的 8 种组合全做，× 2 宽度 × ldr/str = **32 条**。

② **ADD/SUB(S) 移位寄存器带量**（`add w12, w13, w14, lsl #12`）：与既有的逻辑族（`ALUSH6`/`ALUSH5`）同格局——`shift` 是逐指令常量（LSL = 0），移位量是**操作数**（X 用 `imm6`、W 用 `imm5`），4 助记符 × 2 宽度 = **8 条**。

③ **字节证据**：谱内一次性加 12 条向量（8 条寄存器偏移 + 4 条 ADD/SUB 带量），字节全部取自语料 CHECK 行的**上游期望值**（`00 68 60 b8` = `ldr w0, [x0, x0]`、`69 6b 26 f8` = `str x9, [x27, x6]`、`ac 31 0e 8b` = `add x12, x13, x14, lsl #12`…）——**首跑全过**。

④ 效果：aarch64 语料红行 **1945 → 1872**、`parsed` 1268 → **1341**；与第二十七批末（aarch64 红 2122）相比 `arm64-leaf-compact-unwind.s` 16 → **1**（只差最后一条）、`arm64-memory.s` 121 → 100、`arm64-arithmetic-encoding.s` 122 → 110、`seh-packed-unwind.s` 111 → 48。本轮**没有文件跨过"整文件无红"的门槛**（近门的几个都在 1–4 条），所以取舍账本（109 / 625）与两个棘轮**逐字不变**。守卫同步：`spec_coverage_guard` arm64 230 → **270**、`isa_roundtrip_guard` arm64 枚举器 937 → **1049**。

### Added (2026-10-07) — aarch64：寄存器惯用名 `fp`/`ip0`/`ip1`；16 条前后索引（writeback）形态

① **别名表漏了 `FP`**：真实序言/尾声写的是惯用名而不是编号（`mov x29, fp`、`stp x29, lr, [sp, #-16]!`），而我们 `[reg.gpr8].aliases` 只有 `XZR`/`LR`——**注释里写着 FP、表里没写**，一行之差就让 `cfi.s`/`seh-*.s` 这些整文件落红。补 `FP = 29`，顺带 `IP0 = 16`/`IP1 = 17`（链接器 scratch 的惯用名）。

② **前后索引（writeback）16 条**：单寄存器 4 宽 × 前/后索引（`ldr x0, [x1, #16]!` / `ldr x0, [x1], #16`）——与 unscaled 家族共用 `op8 = 0xF8`(X)/`0xB8`(W)，靠新位域 `mode`（[11:10]：`11` 前索引 / `01` 后索引）区分；pair 4 宽 × 前/后索引——[25:23]：`011` 前 / `001` 后 ⇒ `op8 = 0xA9`/`0xA8`（W 形式 `0x29`/`0x28`）、`idx2 = 2`/`3`。imm 一律真字节且有符号（pair 的 imm7 三种模式下都按元素尺寸缩放，沿用上一批的 `imm7x`/`imm7w`）。

③ **字节证据**：仓库自己的 A64 编码基准 [`docs/reference/aarch64-encoding-ref.md`](docs/reference/aarch64-encoding-ref.md) 第 121/124 行给出权威值——`ldr x0,[x1],#16` = `F8410420`、`ldr x0,[x1,#16]!` = `F8410C20`、`stp x29,x30,[sp,#-16]!` = `A9BF7BFD`、`ldp x29,x30,[sp],#16` = `A8C17BFD`，外加无位移 pair 的 `A9000440`/`A9400440`。6 条谱内向量**一次通过**，说明新形态的编码与模式位都对得上。

④ 效果：aarch64 语料红行 **2122 → 1945**、红文件 76 → **72**、`parsed` 1091 → 1268；**4 个文件整文件转绿**（`cfi.s`、`seh-multi-epilog.s`、`seh-large-func.s`、`seh-large-func-multi-epilog.s`——它们此前只差这条序言/尾声写法），全绿集 104 → **109**（aarch64 26 → **30** 份）、aarch64 棘轮 `parsed` 197 → **267**；`seh-packed-unwind.s` 111 → 48、`basic-a64-instructions.s` 482 → 442。守卫同步：`spec_coverage_guard` arm64 214 → **230**、`isa_roundtrip_guard` arm64 枚举器 841 → **937**。

### Fixed (2026-10-07) — aarch64 访存立即数的「单位」：文本是字节、字段按访问宽度缩放（+ 8 条无位移形态）

A64 的 `LDR`/`STR`/`LDP`/`STP` 立即数在**文本里是字节**、在**字段里是"字节 ÷ 访问尺寸"**：`ldr x2, [sp, #32]` ⇒ `imm12 = 4`、`ldr w5, [x4, #20]` ⇒ `imm12 = 5`、`stp x29, x30, [sp, #-16]` ⇒ `imm7 = -2`（这些值直接来自语料 CHECK 行里的上游期望字节）。我们谱里 `imm12u`/`imm7u` 一直按**字段单位**收文本（`ldr x0, [x1, #1]` ⇒ 字段 1 = 8 字节），于是真实语料里**每一条带非零字节偏移**的访存都会编出错字节——只是 vendored 集里一个这样的文件都没有，字节对拍档一直看不见它。

**修法**：按访问宽度分槽（`imm12x` `unit = 8` / `imm12w` `unit = 4`、`imm7x` `unit = 8` / `imm7w` `unit = 4`；pair 的 `imm7` 同时改成**有符号**，`stp …, #-16` 才收得下），而 `imm12u` **留在原地**——ALU/CMP/CMN 的立即数**不缩放**（`add x0, x1, #5` 就是 5），同一段位的两种解释，所以分成两个槽（"单位"不是全局约定，而是每条指令的语义）。

**顺带补 8 条无位移形态**（`ldr x4, [x3]`、`str w9, [x8]`、`stp x0, x1, [x2]`…）：编码上就是"位移 = 0"那一格，与 `DCPS1B`（裸 `dcps1` = `dcps1 #0`）同款——声明序放在带位移的指令**之后**，解码仍走带位移那条（渲染出 `, #0`），新形态只承担"省略写法能装配"。

**证据**：语料 `arm64-memory.s` 的 `; CHECK: … ; encoding:` 行给出上游字节，谱内新增 6 条向量（`ldr x4, [x3]` = `64 00 40 f9`、`str x2, [sp, #32]` = `e2 13 00 f9`、`ldr w5, [x4, #20]` = `85 14 40 b9`…）；该文件里我们认得的 23 条**逐字节全等**（`known` 0），且改动前后**没有新增任何访存类字节差异**。原有 7 条访问向量只是**文本**从字段单位换成字节（`ldr x0, [x1, #1]` → `#8`、`stp x29, x30, [sp, #2]` → `#16`），**字节一字未动**——正说明改的是"解释"而不是"编码"。

效果：aarch64 语料红行 **2183 → 2122**、`parsed` 1030 → 1091（`arm64-leaf-compact-unwind.s` 34 → 16、`seh-large-func-multi-epilog.s` 37 → 8、`basic-a64-instructions.s` 494 → 482）；vendored 集与两个棘轮**逐字不变**（新增能力只影响此前落红的文件，取舍账本没动）。守卫同步：`spec_coverage_guard` arm64 206 → **214**、`isa_roundtrip_guard` arm64 枚举器 825 → **841**。

### Added (2026-10-07) — riscv64 浮点 `rm` 成为操作数；省略写法的缺省值按**字节证据**定（算术 `dyn`、转换 `rne`）

① **`rm` 是操作数**：F/D 的 `funct3` 那 3 位就是舍入模式，谱里原先把它写死（每条指令一个固定值，或干脆没有 rm），于是真实语料里**带 rm 的写法**（`fadd.s f1, f2, f3, dyn`、`fcvt.w.s a0, fs5, rne`）一条都装配不了。现在用既有机制表达：`[conventions.imm_names.rm]`（`rne`/`rtz`/`rdn`/`rup`/`rmm`/`dyn`——就是上一批那张**命名立即数表**的第二张消费者）+ 两条新 form（`R_RM` 三操作数、`R_RM2` 两操作数：`funct3` 当操作数位域），浮点算术 5 族与 `fcvt` 全 18 条各加"显式 rm"形态。

② **省略 rm 的缺省值不靠猜**：真实语料两种写法都写，所以每族另有"省略 rm"的并列声明。缺省值是从**语料自带的上游期望字节里量出来的**：**算术族（`fadd`/`fsub`/`fmul`/`fdiv`/`fsqrt`）省略 rm = `dyn`(7)**，**转换族（`fcvt.*`）省略 rm = `rne`(0)**——11 条独立样本，横跨 `rv32d-valid.s`、`rv32zdinx-valid.s`、`rv32zfhmin-valid.s`、`rv64zhinxmin-valid.s`（两个上游文件对同一件事的**反汇编**写法还给过相反的暗示，最终以**字节**为准）。我们原先两侧都写死 `0` ⇒ 算术族是**字节错**，现在两侧都由上游字节守住。

③ **顺带修掉 `FCVT.S.D` 的 `rs2`**：规范是 funct7 `0100000` + **rs2 = 1**，我们原来与 `fcvt.d.s` 同形（rs2 = 0）——字节对拍直接抓到（`fcvt.s.d fs5, fs6, dyn` 少一位）。同时删掉 6 条固定 `rtz` 的旧 `FCVT_*_RTZ` 与 2 条把省略写法写死 `rne` 的声明（已被 rm 操作数覆盖），lowering 改成显式传 `1`（= rtz）：lowering 模板要过"操作数签名"校验，**裸名字不在它的分类里**，写数字才过。

④ **效果**：`rv32f-valid.s` / `rv32d-valid.s` / `rv64d-valid.s` / `rv64f-valid.s` / `fp-default-rounding-mode.s` 与两个 aliases 文件共 **7 个进全绿层**（全绿集 97 → **104**；riscv64 语料 68 → **75** 份、`parsed` 1616 → **1741**）；riscv64 红行 465 → **446**、红文件 58 → 55；编码对拍 `cases` 1139 → **1251**、`checked` 1021 → **1111**、**`known` 仍 0**——这几个 FP 文件的每条上游期望字节**逐字节相等**（`rv32d-valid.s` 49 条期望 / 38 条对拍上 / 0 差异），`rm` 的两套缺省值因此有了直接的字节证据。
   附带一处**归因迁移**（不是回归）：Zfinx/Zdinx 那些"浮点算在整数寄存器上"的写法（`fadd.s x26, x27, x28`）原先落 `no_prefix`（连助记符都不认识），补上 FP 形态后落进红桶（认识助记符了、操作数对不上）——`no_prefix` 182 → 224、红行净减 19。

⑤ **工具修正**：`fetch.mjs` 的打分步骤加 `--no-fail-fast`。cargo 默认在**第一个失败的测试目标**就停，而两个档的棘轮在换谱/换语料时几乎必然同时不一致 ⇒ 排在后面的 `asm_parse` 根本不跑，解析档记分板停在上一轮内容上，于是"这一轮才下载、还没打过分"的候选被记成 `not-scored` **砍掉**（判据被静默绕过）。2026-10-07 实测踩到（364 个候选被误记、淘汰账错位），已修。

### Added (2026-10-07) — 命名立即数表（`[conventions.imm_names.<表名>]` + 槽 `names`）；riscv64 命名 CSR 全量落地

① **新能力：一个名字 = 一个值**。与既有的 `kind = "bits"`（名字**拼接**、编码取按位或，`fence iorw`）相反，真实 ISA 里另一类是**单名单值**：RISC-V 的 CSR 名（`mstatus` = 0x300）、浮点舍入模式（`rtz` = 1、`dyn` = 7）。谱里写一张表、槽上写 `names = "<表名>"` 即可：

```toml
[conventions.imm_names.csr]
mstatus = 0x300
mvendorid = 0xF11

[[operand_slots]]
name = "csr12"
kind = "imm"
signed = false
width = 12
names = "csr"
```

运行时**双向**：解析**既认名字也认字面量**（真实语料两种都写：`csrrs t1, mvendorid, zero` 与 `csrrs t2, 0xF11, zero` 是同一批测试里的相邻两行），名字按大小写不敏感匹配（与寄存器名同口径）、没命中就落回原来的字面量/表达式路径；渲染时**值在表里就写名字**，所以 `disassemble → assemble` 照旧闭合，**同值多名取字典序最小者**（上游有 `stval`/`sbadaddr`、`satp`/`sptbr`、`mtval`/`mbadaddr`、`dscratch`/`dscratch0` 这类别名对）。校验期 fail-closed：表必须已声明且非空、名字必须是 ident、**每个值要落在槽的接受值域内**；`names` 只对 `kind = "imm"` 有意义，变长（`prefix_scan`）ISA 写它直接拒。表是**数据**——DSL 不认识任何 CSR 名/舍入模式名。

② **riscv64：CSR 名表 432 条**，从上游 `llvm/lib/Target/RISCV/RISCVSystemOperands.td`（ref `llvmorg-19.1.0`）机械生成（含 `foreach` 区间 `hpmcounter3..31`/`pmpaddr0..63` 与 `AltName`/`DeprecatedName` 别名），并**用语料自带的期望字节逐条对账**：语料里出现的 429 个名字全部命中、值**零不符**。此前 `csr12` 槽只收数字，于是 `machine-csr-names.s`、`user-`/`supervisor-`/`hypervisor-csr-names.s` 与其 rv32 变体**整文件**落红桶。

③ 效果（`fetch.mjs` 重跑取舍 + 刷棘轮）：riscv64 **红行 1056 → 465**、红文件 72 → 58；**13 个文件自动进全绿层**（全绿集 84 → **97**、PROVENANCE 98 份 LLVM + 1 份手工；riscv64 语料 55 → **68** 份）。编码对拍档同时大涨，且**逐字节全等**：riscv64 `cases` 339 → **1139**、`checked` 221 → **1021**、`known` 仍 **0**（新进来的 CSR 文件自带 ~800 条上游期望字节，全部对上——这是 432 条表值正确性的直接证据）。生成物自测（`forge-codegen --lib` 1863 条）与三方针守卫全绿。

> 新进集的文件里有 `machine-csr-names-invalid.s`（上游"故意写错"的文件）：它那些行在 rv32 目标上被 LLVM 拒（"requires an option to be enabled"），但在 **riscv64** 目标上名字 → 12 位编码本身完全合法（上游 `rv64-machine-csr-names.s` 的注释也这么说），不是假绿。

### Added (2026-10-07) — riscv64 浮点寄存器 ABI 别名（`ft0`/`fa0`/`fs0`…）；逐文件记分板带上红桶原文

① **纯谱数据修掉一整类红桶**：`[reg.gpr8.aliases]`（`a0`/`s0`/`fp`…）早就有，`[reg.fpr4]` 却只有 `F0`…`F31` 数字名——而**真实语料里浮点操作数几乎全写 ABI 名**（实测全集 `ft*`/`fa*`/`fs*` 出现 **1840** 次，数字名只有 **47** 次），于是整批 FP 用例文件（`rv32f-valid.s`、`rv32d-valid.s`、`numeric-reg-names-*.s`、`fp-*-default-rounding-mode.s`…）**整文件**落进红桶。补上 psABI 那套映射（0–7 = `ft0`–`ft7`、8–9 = `fs0`–`fs1`、10–17 = `fa0`–`fa7`、18–27 = `fs2`–`fs11`、28–31 = `ft8`–`ft11`），口径与 GPR 别名**完全一致**：**解析认、渲染仍出规范名**（`F`），不新增任何 DSL 键、不动生成器。

② 实测（`asm/fetch.mjs` 重跑取舍 + 刷棘轮）：riscv64 红行 **1240 → 1056**、红文件 76 → 72；`numeric-reg-names-f.s` / `numeric-reg-names-d.s` **整文件解析通过**并自动进全绿层（全绿集 83 → **84** 份、PROVENANCE 85 份 LLVM + 1 份手工；riscv64 语料 53 → **55** 份、`parsed` 526 → **654**）；同族文件红行同步下降（`rv32f-valid.s` 26 → 16、`rv32d-valid.s` 27 → 18、`fp-default-rounding-mode.s` 16 → 8、`rvf-aliases-valid.s` 10 → 5）。三架构**编码对拍 `known` 仍全 0**（棘轮只动 riscv64 解析档），x86/aarch64 计数逐字不变。

③ **工具：逐文件记分板带红桶原文**（`target/asm-suite/<isa>.json` 的 `file_rows[].samples`）：每条是 `文件:行号 ⇥ 原文 ⇥ 错误`（每文件最多 8 条），此前只进棘轮的总样例（每套 12 条）——按文件归因时反面看不到"错在哪一行"。JSON 里的控制字符（制表符）现在**转义**（`esc`），否则消费方解析整份记分板会失败；`fetch.mjs` 不读该字段，**取舍判据不变**。

### Added (2026-10-06) — 真实汇编语料从"三份小样"扩到**上游全集里全绿的那一层**（取舍自动化 + 出处守卫）

① **取舍不再手抄**：新工具 `crates/tools/forge-tests/asm/fetch.mjs`（Node 自带根证书，替掉需要系统 TLS 的 `fetch.ps1`）一条命令做三件事——按钉死的 ref（`llvmorg-19.1.0`）拉上游 LLVM MC 三套目录的**顶层** `.s`（x86 只取 Intel 语法的文件名那批），跑解析档 + 编码对拍档拿**逐文件**记分板，再按「**整条解析得住（`parsed >= 1`）+ 不产生红桶（`tail_mismatch` ≤ 已登记缺口）+ **编码对拍没有字节差异**」淘汰，最后**重写** `asm/PROVENANCE.md`（逐文件 字节/行/sha256）。两个档因此新增机读产物：`target/asm-suite/<isa>.json` 的 `file_rows` 与 `encoding-files.json`（逐文件记分板），淘汰清单落 `target/asm-dropped.json`——判据在脚本里，不在人脑里：谱长本事后重跑即自动补回。

② **评了 733 个候选、收 83 个**（riscv64 339 / aarch64 337 / x86 57 顶层；淘汰 651 = 576「本谱一个候选都不认识」+ 66「出红」+ 9「有字节差异」）。成果：`parsed` riscv64 136 → **526**、aarch64 153 → **197**、x86 llvm-mc 111 → **116**；编码对拍 riscv64 134 → **221**、aarch64 119 → **125**、x86 110 不变，**三架构 `known` 仍然全 0**（每一条对拍上的期望字节都逐字节相等）；红桶只有 riscv64 那 1 条已登记的 `jal a0, a0`。淘汰面（SVE/SME/AVX-512/AMX/RVV/RVC 压缩编码/带符号 CSR 名/重定位表达式/移位与扩展寄存器操作数…）逐类记在 `asm/README.md`。

③ **语料抓出一处真缺陷并修掉**：riscv64 `clz`/`ctz`/`cpop`/`rev8` 原先写成 **R 型**（`opcode = 0x33` + `funct7`），实际是 **OP-IMM**（`opcode = 0x13` + 立即数 `0x600`/`0x601`/`0x602`/`0x6b8`）——原编码与 `rol` 撞车（同 `funct3=1`/`funct7=0x30`，只差 `rs2`），**我们自己的解码器把 `clz X5, X6` 的字节解成 `rol X5, X6, X0`**，即这三条编出来解不回自己。上游 `rv64zbb-valid.s` 的期望字节 `0x93,0x52,0x83,0x6b` 一比就露（`isa/riscv64.toml` 换 `form = "I"` + 固定 `imm12`，谱内 4 条向量同步改）；修完 `rv64zbb-valid.s`/`rv64zbkb-valid.s` 由"有字节差异"转为**逐字节全等**并进了语料集。

④ **新守卫 `crates/tools/forge-tests/tests/asm_provenance.rs`**：`asm/PROVENANCE.md`（生成物）与磁盘语料**双向一致**——磁盘有表里没有 / 表里有磁盘没有 / 字节数或行数对不上，三者都红（sha256 由 `fetch.mjs` 拉取时校验，守卫只比字节数与行数以免引入哈希依赖）。`gnu-gas-intel` 那份节选同时**钉到 commit** `19cc5b7efb3e…`（按内容寻址，任一镜像同 sha），并核出它其实是上游**前 64 行逐字未改**（原先 PROVENANCE 里"空白已归一化"的说法不成立，已删）。

### Added (2026-10-05) — x86 `movzx`/`movsx` 的内存源形态（最后一个已登记的汇编器缺口）

`movzx eax, byte ptr [rbx]` 的**源宽度只能由尺寸关键字给**（`0F B6` 与 `0F B7` 的差别在操作码，没有任何寄存器能表达源宽），而 `{size}` 按设计只是"给人读的提示"、不进类型签名 ⇒ 原来那对 `MOVZX_R{8,16}_MEM`（`[src]` 简写、asm 里**没有关键字**）文本一模一样，只能挂在歧义名单上；再照搬 16/32 位目的地的变体只会让 `movzx eax, word ptr [rbx]` 静默编成 byte 那条。

**修法用既有机制**：把尺寸关键字写成**模板字面量**（`asm = "movzx {dst}, byte ptr {mem}"`，与 `NOT_MEM32` 一族同款）——两条写法文本天然区分、候选各自命中，**渲染也带关键字** ⇒ `disassemble → assemble` 闭合。补 4 条完整内存模板形态（`MOVZX/MOVSX_MEM{8,16}`；目的宽度仍由寄存器驱动：16 位目的发 66、64 位目的发 REX.W）。

顺带修掉简写那对的两处问题：① `MOVZX_R16_MEM` 把 `66` **写死**（那是"16 位目的"的编码），配 64 位目的会编出 `66 48 0F B7`——现在宽度一律由目的寄存器驱动（`48 0F B7` ✓）；② 基址槽收窄成单一 64 位地址类（多类槽会被**生成期采样器**强推 2/4 字节**地址**，是无意义的组合——地址宽度不是数据宽度）。

效果：`spec_coverage_guard` 的文本歧义名单**少了 `MOVZX_R8_MEM`/`MOVZX_R16_MEM` 两条**（文本不再撞车 ⇒ 恢复强断言）；落地 4 条谱内向量；文档补「源宽度只有尺寸关键字能表达时，把关键字写成模板字面量」。

### Added (2026-10-05) — 字节对拍三架构全清：长编码拆行纳入对拍 + 十六进制立即数是位模式

① **上游把长编码写成多条只有字节的注释**（`acquire lock add …` 是 `[0xf2]` + `[0xf0,0x48,0x01,0x00]`；`pushf`/`popf` 是"两条 CHECK 后面跟两条指令"），抽取器原先按"run ≥ 2 就整段丢掉"保守处理。现在按**声明的侧**绑定、且**只在块内相邻**：注释在指令**后** ⇒ 按序**拼接**成一份期望；注释在指令**前** ⇒ 按序 **1:1** 配给紧随其后的同样多条指令；绑不上就整块丢掉、后续不错位（三条单测钉住；`intel-syntax-encoding.s` 补上逐段声明）。

② **十六进制立即量是位模式**：词法器原先按 `i64::from_str_radix` 读，64 位槽写不出 `#0xfffffffffffffff0`（= -16）——aarch64 的 `and sp, x5, #0xfffffffffffffff0` 报 "bad hex immediate"。改成按 `u64` 读、再按位重解释（二进制同理；**十进制仍是值**，超 i64 报错）。

效果：编码对拍 x86 `cases` 104 → **110**、`dropped` 8 → **0**；aarch64 `unparsed` 1 → **0**、`checked` → **119** —— **三架构都是 `cases = checked`、`known` 全 0**（语料里每一条期望字节都逐字节对上）。

### Added (2026-10-05) — x86 `prefix` 收列表（`acquire/release lock`）；x86 语料全部归因完毕

`acquire lock add [mem], r` = `F2` + `F0`、`release lock add …` = `F3` + `F0`：x86 的锁 + 内存序提示是**两条前缀**，而 `EncKeys.prefix` 只收一个字节。现在 `prefix` 收**列表**（`prefix = ["0xF2", "0xF0"]`）：编码按书写序发（66 → 前缀 → REX → opcode），解码逐个按**前缀扫描标志**判（顺序无关）；校验期拒掉列表里的 `"opsize"`（混进列表会让"发不发"含糊）与第二个 `"field"`。

改这里踩到一个**必须保留的旧语义**：单条前缀值为 `0` 时生成的"**没有前缀**"断言（`!__p66 && !__pF0 && !__pF2 && !__pF3`）不能省——SSE 的 `66` 变体（`ADDPD`）与无前缀变体（`ADDPS`）同 opcode，少了它后者会先命中前者的字节（全量套件实测 `spec_addpd` 解成 `Addps`）。

效果：x86 `llvm-mc` 解析档 `parsed` 109 → **111**、`no_prefix` 2 → **0** —— 该档 447 行（111 parsed + 336 corpus_only）**全部归因完毕**；两条锁前缀的字节人工核对 `f2 f0 48 01 00` / `f3 f0 48 01 00` 与上游一致（它们不在编码档的 104 条里：上游把长编码拆成两条 `CHECK: encoding:` 注释，抽取器按"run ≥ 2 就整段丢掉"保守处理——已记入 `asm/README.md`）。

### Added (2026-10-05) — x86 助记符条件后缀（`sete`/`cmovl`）→ 语料带期望字节的用例全部逐字节对上

上游写 `sete r16b` / `cmovl eax, r16d`（助记符**自带条件**、没有 `cc` 操作数），我们只有通用形态（`setcc {dst}, {cc}` / `cmovcc {dst}, {src}, {cc}`，供 lowering 用）。两条"做成指令"的路都被**解码树**挡住（原因留在 `isa/x86.toml` 注释里）：`{cc}` 嵌进助记符要扫描器让"紧贴操作数的字面段"按 ident 前缀匹配（`sete` 是一个 token）；按条件码建**精确** opcode（`0F 94`）会与通用形态的**掩码**边（`0F 9x`）重叠——那是**真**歧义（同一批字节两种 `Inst` 形状），`check_dec_trie_overlaps` 直接拒。

**解法是既有机制**：`[[pseudo]]` 文本展开（分派键是**整词** `sete`/`cmovl`，不产生解码 arm；SSE 的八个比较别名就是先例）。配套补三条**同掩码**的兄弟指令（`SETCC_RM8_B` 8 位名、`SETCC_R_MEM` 内存、`CMOVCC_R_MEM`；同掩码 ⇒ 归到同一解码节点按声明序试，靠 mod 守卫区分），并把 `CMOVCC_R_RM` 从固定 64 位放宽到 16/32/64。

顺带修掉同族一处**恒假守卫**：`opsize = 8`（`setcc byte ptr`）生成 `__opsize == 1` 的解码条件，而 `__opsize` 只可能取 2/4/8 ⇒ arm 永远解不出来（8 位没有 opsize 前缀可查——与 v20 V10 加 8 位 ALU 族时实测过的那条同源，这次补在 `Opsize::Reg` 分支上）。

效果：x86 `llvm-mc` `parsed` 95 → **109**、`no_prefix` 16 → **2**（只剩 `acquire/release lock`）；编码对拍 **checked 104 / unparsed 0 / known 0**——**语料里带期望字节的用例全部逐字节对上**。落地 3 条新谱内向量（`sete r16b`、`sete byte ptr [r16 + r17]`、`cmovl r17d, r16d`）。

### Added (2026-10-05) — x86 补 APX 语料用到的真实形态；`llvm-mc` 解析档红桶清零

承接 APX/REX2：把 `apx-rex2-format-intel.s` 里**谱里根本没有的形态**补上（14 条指令），x86 `llvm-mc` 的 `tail_mismatch` 从 39 降到 **0**：

- **一元族的内存形态** 8 条（`NOT/NEG/INC/DEC/MUL/IMUL/DIV/IDIV_MEM32`，`FF /0./1`、`F7 /2../7`）。宽度没有寄存器可驱动，所以模板里**写死字面 `dword ptr`**——`not byte ptr [rax]` 不会命中（真实汇编器同样要求一元内存形态给尺寸提示），不会静默按 32 位编出来。
- 一元 **`MUL_RM`/`IMUL_RM`**（`F7 /4`、`F7 /5`）；拓宽 `NOT_RM`/`DIV_RM`/`IDIV_RM` 到 16/32/64 位（原来固定 64）。
- 两操作数 **`IMUL_R_MEM`**（`0F AF` + 内存源）、**`NOP_RM`/`NOP_MEM32`**（`0F 1F /0`，Intel 的多字节填充形态）。
- **`MOVSXD_R_RM32`**：真实汇编写 32 位源名（`movsxd rax, r16d`），与 lowering 用的 64 位源那条**同编码**（`MRR_FIX64`）——两条同编码指令是既有格局（`MOV_R_RM`/`MOV_RM_R`）。

顺带修掉一处**解码树漏洞**：固定扩展码（`F7 /2` 的 2）的守卫只加在寄存器分支、**内存分支漏了** ⇒ `neg dword ptr [rax]` 会解成**声明在前**的 `NotMem32`。一元内存形态是第一批"固定 ext + 内存"指令，所以现在才暴露。

效果：x86 `llvm-mc` `parsed` 36 → **95**、红桶 55 → **0**、`no_prefix` 20 → 16；字节对拍 `checked` 31 → **90 条逐字节全等**、`known` 仍 **0**。余下 16 条 `no_prefix` 已逐条登记（14 条助记符条件后缀 `cmovl`/`sete`、2 条 `acquire/release lock`），各自的实现路线写在 `asm/README.md` 与 `isa/x86.toml` 注释里。

### Added (2026-10-05) — x86 APX：REX2 前缀与 EGPR（`r16`..`r31`）编码/解码；16/32 位 `mov` 的 MR 形态

**能力由谱数据声明，不新增开关**：x86 的四个 GPR 宽度组扩到 32 项（`r16`..`r31` 四个视图 = APX 的 EGPR），编码器看到任一 GPR 字段的索引 ≥ 16 就改发 **REX2**（`0xD5` + payload `M0 R4 X4 B4 W R3 X3 B3`），第 4 位进 payload、`M0` **取代 `0F` 字节**；解码侧在 `M0=1` 时先补一个虚拟 `0F` 再走原有派发树（消费长度减一），因此派发树不用写第二份。`[[conventions.prefix_scan]]` 的效果从 `Vec<String>` 改成**枚举** `PrefixEffect`（`addr32`/`addr16` 合并成有值变体 `AddrSize(32|16)`），编码器/校验器/schema 不再各处比字符串。

新增键 `[reg.<组>].alloc_count`（**分配池 ≠ 寄存器文件**）：`[reg.gpr8]` 现在 `names` 32 项 + `alloc_count = 16`——EGPR **能编码**（`mov r16d, eax` 这类写法成立），但**不进分配池**。这是安全阀：分配器一用 r16+，JIT 产物在没有 APX 的机器上就是**非法指令**（实测 `STATUS_ILLEGAL_INSTRUCTION`，全量套件当场抓到）。

两条 fail-closed：VEX/EVEX 与 0F38/0F3A 表达不了 EGPR（本实现里 GPR 字段只有 4 位）——寄存器槽装得下 EGPR 的谱在**生成期**报错，内存地址是 EGPR 的在**编码期**报错；高编号寄存器视图按同一能力封顶（取到 15）。

顺带补上 x86 **16/32 位 `mov` 的 MR 形态**（`89`，`MOV_RM_R_24`）：上游 LLVM 对 reg-reg 用 MR，我们原先只给 64 位建了它 ⇒ 那类上游编码我们**解不回来**。字节变化（16/32 位 reg-reg `mov` 由 `8B` 变 `89`，与上游一致）属破坏性更新：谱内向量与相关测试已同步。

效果（语料 `apx-rex2-format-intel.s`）：x86 llvm-mc 档 `parsed` 36 → **52**、红桶 55 → **39**；编码对拍 `checked` 31 → **47**、`known` 保持 **0**（16 条 APX 行与上游 CHECK 逐字节一致）。落地 5 条 APX 谱内向量。

### Fixed (2026-10-05) — 谱统一落在严格 TOML 1.0 子集（`isa/riscv64.toml` 的多行内联表）

`isa/riscv64.toml` 的寄存器别名原先写成跨 5 行的内联表（`aliases = { zero = 0, … ,` … `t6 = 31 }`）——那是 **TOML 1.1** 才允许的写法（多行内联表 + 尾逗号）：本仓库的 `toml` 依赖是 spec 1.1 实现（`toml-1.1.x+spec-1.1.0`），所以构建全绿；而编辑器、Python `tomllib` 等 1.0 解析器会报语法错。改成标准子表（写法等价）：

```toml
[reg.gpr8.aliases]
zero = 0
```

生成物与语料计数**零变化**，并补一条窄守卫 `crates/frontend/forge-isa-dsl/tests/schema_guard.rs::shipped_specs_are_strict_toml_1_0`（只钉"内联表内不许有裸换行"这一条，扫描器口径另有自测）与规范说明（`docs/reference/isa-dsl.md`「谱的书写约束：严格 TOML 1.0 子集」）。

### Added (2026-10-04) — `[[operand_slots]].symbols`：立即数位置上的符号引用（`%hi(foo)`）

真实汇编里立即数常常是**符号**：`lui a0, %hi(foo)`、`addi ra, sp, %lo(foo)`、
`auipc a0, %pcrel_hi(foo)`、`jal rd, .Lpcrel_hi0`。此前这类行一律报"没有这条指令"——**诊断是错的**
（那是合法写法，只是符号要等重定位）。

新增槽属性 `symbols = true`（只对 `imm`/`label` 有意义，且要求 `unit == 1`）：

- **解析**：未定义的 ident（`.equ` 常量优先）被记成**符号引用 + 当前立即数修饰**，当 0 参与算术；
  整条命中后随 `__lsyms` 一起提交。未声明 `symbols` 的槽照旧"未知 ident 即不匹配"——**能力是
  声明的，不是隐含的**；`.equ`/`.set` 的表达式也不受影响（那里不武装）。
- **回填**（两遍布局的第二遍）：符号解析成"该标签的**块下标**"（`.` = 本条指令自身的下标），
  再**过一遍修饰**（复用 `[[conventions.imm_fn]]` 生成的 `__imm_fn<i>`）——`%hi(foo)` 就是
  `hi(foo 的块下标)`（= 3，与 `foo` 在 3 号指令处一致）；**未定义**的符号报 `UndefinedLabel`，
  这正是想要的准确诊断。
- riscv64 落地：`imm12`/`imm20` 打开 `symbols`；补 `%pcrel_hi`/`%pcrel_lo` 两个修饰（值与
  `%hi`/`%lo` 同义——重定位**种类**是 linker/patcher 的事，本仓库记在 `[[reloc]]`/`reloc` 上）。
- 顺带修掉一处**词法缺陷**：`.` 开头的局部标签名（`.Lp`、`.Lpcrel_hi0`——真实语料里遍地都是）
  原先被切成 `Dot` + `Ident`，既当不了标签也解析不了。`__lex` 现在只在 `.` 后面**不跟字母**时
  才当位置符号（`jal zero, .` 照旧）。

效果：riscv64 语料 `parsed` 135 → **136**、红桶 7 → **1**（只剩 `jal a0, a0`——上游把第二操作数当
**未定义符号 a0**（`jal rd, symbol` 只收符号），我们拒了"寄存器样子的 ident"当符号以免 `jmp rax`
这类走错候选，已登记为设计取舍），`corpus_only` 361 → **366**（5 条未定义符号行归因到"符号在别处"，
不再误报成"缺指令"）。x86/aarch64 计数不变（词法改动无回归）。

### Added (2026-10-04) — `kind = "bits"`：命名位集合操作数（`fence pred, succ`）+ riscv 收尾三条写法

**命名位集合**：有些操作数在文本里是**若干名字的拼接**——RISC-V `fence` 的 pred/succ 就是
`i`/`o`/`r`/`w` 的组合（`fence iorw, iorw`、`fence w, ir`、`fence r,w`）。表是**数据**：

```toml
[conventions.bitsets.fence]
i = 8
o = 4
r = 2
w = 1

[[operand_slots]]
name = "fence_set"
kind = "bits"      # 新 kind
table = "fence"
width = 4
```

- 解析：取一个 ident，按表里的名字**贪心最长匹配**逐段吃掉，编码值 = 各位的**按位或**；整串必须
  吃干净（剩余字符不是名字 ⇒ 这条写法不匹配，不静默当空集）；表在**解析期**摊平进槽
  （`resolve_bitset_tables`，`#[serde(skip)]` 字段），生成器不再回头查表；
- 渲染：按名字**字典序**把置位的名字拼起来（确定性，与 `[conventions.cond]` 的"同码取字母序
  最小名"同一口径）；
- 校验：表必须已声明且非空、`width` 必填、位必须落在槽宽内、名字必须是 ident 片段；变长
  （`prefix_scan`）ISA 写它 → 校验期 fail-closed。三方针同步：`schema.rs` 新增
  `[conventions.bitsets.<table>]` 段与 `[[operand_slots]].table` 键、`docs/reference/isa-dsl.md`
  的键表与专节、schema_guard 的 `INTERNAL_FIELDS` 登记（`table_entries`）。

**riscv64 收尾三条写法**（都是纯数据/小改）：

- `jalr rd, rs1, imm`（三操作数，手册里的规范形；与两操作数写法**同编码**，只是文本不同）；
- 位置符号 `.`：`__label` 把它记成**自引用**（空名），两遍回填时换成"当前指令自身的块下标"，
  与"标签回填块下标"同一口径（`jal zero, .`）；
- `fence`/`fence.i`/`fence.tso` 从**整字常量**改成字段形——整字常量会与 `fence pred, succ` 的
  opcode 边在 [0,7) 上判成同一条 trie 边（生成期直接报 ambiguous）；顺带删掉因此失效的
  `W32` 表单与 `word` 位域（lint 的"写了却用不上"当场抓到）。

效果：riscv64 语料 `parsed` 129 → **135**、红桶 13 → **7**，字节对拍 **129 → 134 条逐字节全等
（`unparsed` 5 → 0）**——"有期望字节的用例全部对上"（`known` 仍 0）。riscv64 指令 137 → **139**、
派生枚举器 435 → **442**、未指定位评审清单 3 → **7**（fence 族字段形的固定 0 段）。
余下 7 条红桶是**一类**：立即数位置上的未定义符号（`%hi(foo)` 等，需要"imm 槽符号引用 +
回填后才算得出 hi/lo"这一设计项）与 `jal a0, a0`（上游当未定义符号 `a0`，我们拒了寄存器样子的
ident 当符号——已登记不改）。详见 `crates/tools/forge-tests/asm/README.md`。

### Added (2026-10-04) — `encode = "logical_imm"`：位掩码立即数（值 → N/immr/imms）+ aarch64 语料清零

**逻辑（位掩码）立即数**：ARM 系逻辑运算的立即数不是任意值——它必须是"一段连续 1 循环填充
整个宽度"的位模式，编码成三分量 `N`(1 位) / `immr`(6 位) / `imms`(6 位)。这是**非线性**映射
（由值的位结构反算分量），位切片表达不了，于是 `encode` 增加第二个方案：

```toml
[[operand_slots]]
name = "logicimm64"
kind = "imm"
signed = false
width = 64                     # 元素宽度（32 或 64）
encode = "logical_imm"
fields = ["nbit_l", "immr", "imms"]   # 依序 = N、immr、imms（校验期钉死宽度 1/6/6）
```

- 实现（`__encode_logical_imm`/`__decode_logical_imm`，只在用到它的谱里发射）：① 找最小的
  重复元素尺寸；② 把那段 1 旋到最低位并确认它**连续**（可环绕）；③ 反算 `immr` 与 `imms`。
  不可编码的值（全 0、全 1、非连续段）在编码期报错、可编码集不是区间 ⇒ 生成期自测不按
  lo/hi 采样边界（代表值由方案给）。
- 方案由**谱按名字选择**、实现由 DSL 提供（与 `kind = "cond"`/`wrap`/`unit` 同一类）。

aarch64 落地（+8 条指令：`and`/`orr`/`eor`/`ands` × X/W）：

- 顶层用 **op9**（[31:23]）而不是 op8——`MOVZ/MOVN/MOVK` 的 op9 只在 bit23 上与逻辑立即数
  不同（0x1A5 vs 0x1A4），用 op8 会被位 trie 判成同一条边（生成期直接报 ambiguous）；
- W 形式 `wrap = true, min = -2^32`：收得下 `#~15`、`#~(0xfe<<24)` 这类按位取反写出来的负数，
  规范化到无符号位模式；
- 字节与上游注释逐条相同（`and w0, w0, #1` = `12 00 00 00`、`and sp, x5, #~15` 的
  immr=60/imms=59、`eor x1, x2, #0x8000` 的 immr=49 都对上）。

**aarch64 语料清零**（余下三条写法，全是纯数据）：`ret xN`（第二条 RET，rn 成操作数，解码靠
"叶 arm 先试 + 平铺 if" 回退）、`b.al`/`b.nv`（A64 里是保留码，上游汇编器照样收）、裸
`dcps1/2/3`（= `dcpsN #0`，声明成 imm16==0 的叶 arm；先试过 `[[pseudo]]`，但伪指令的前导
字面与指令重名会被校验期正确拒绝）。另加 `[reg.*].aliases` 的 `XZR`/`WZR`（31 号在"零寄存器"
语境下）与 `LR`——`orr w8, wzr, #0x1`、`ret lr` 才解析得动。

效果：aarch64 语料解析档 `parsed` 130 → **153**、**`no_prefix` 4 → 0、红桶 19 → 0**（语料里
能认出来的写法全部通过），字节对拍 `checked` 100 → **118 条逐字节全等**（`known` 仍 0）。
arm64 指令 200 → **206**、派生枚举器 818 → **825**。

### Added (2026-10-04) — `[[operand_slots]]` 的 `encode` + `fields`：一个值摊到多个字段

有些操作数在编码里不是"一段连续的位"：A64 的 `tbz`/`tbnz` 位序号（0..63）被拆到**两个
不相邻**的位域——低 5 位进 `b40`[23:19]、第 5 位进 `b5`[31]。DSL 此前只能声明"位域 ← 常量
或操作数"，没有"一个值 → 多个字段"这一环，于是 `tbz`/`tbnz` 整族写不出来。

新增两个槽属性（成对出现）：

```toml
[[operand_slots]]
name = "bitpos64"              # 位序号 0..63
kind = "imm"
signed = false
width = 6
encode = "slice"               # 位切片：按 fields 声明序从值的最低位切起
fields = ["b40", "b5"]         # b40(5 位)[23:19] + b5(1 位)[31]
```

- **`slice`**：第 i 段 = 值 `>>`(前面各段宽度之和)，长度 = 该位域宽度；解码反向拼回
  （编码、解码、反汇编、生成期自测四处同源）。校验期要求 **Σ字段宽度 == 槽的 `width`**；
- 表单 `operand_fields` 里这一项仍写**首字段**（`fields[0]`），其余字段由槽声明
  （写别的 → 校验期拒绝）；
- 方案由**谱按名字选择**、实现由 DSL 提供（与 `kind = "cond"`/`wrap`/`unit` 同一类：
  能力是声明的，不是隐含的；没声明的谱生成物逐字不变）；只实现于定宽（含 `mixed`）
  编码/解码，变长（`prefix_scan`）ISA 写它 → 校验期 fail-closed；
- 解码的**补集零 guard 要把槽的每个字段都算作"操作数位"**：只算 `operand_fields` 那一个
  会把 `b5`[31] 当成保留位、要求它为 0——实测 `TBZX[bit=63]` 编得出但解不回。

aarch64 落地：新增 `op7`/`b5`/`b40`/`imm14` 位域与 `off14`（label，`unit = 4`）、
`bitpos64`（slice 槽）、`bitpos32` 槽，表单 `TBZ` 与 `TBZ`/`TBNZ` 模板 × X/W = 4 条指令
（arm64 指令总数 188 → **192**，派生枚举器 778 → **802**）。3 条谱内 `[[vectors]]`
（含 `tbz w3, #5, #32764` 与 `tbnz x3, #8, #-32768` 两条上游注释里的真实字节）。

效果：aarch64 语料解析档 `parsed` 123 → **130**、**`no_prefix` 11 → 4**、红桶 19 不变，
字节对拍 `checked` 98 → **100 条逐字节全等**（`known` 仍 0）。

### Added (2026-10-04) — `[[operand_slots]].unit`：源值单位（分支偏移是字节）

真实汇编器里的分支偏移是**字节**：A64 的 `b #28` 编码成 `imm26 = 28 / 4 = 7`（`cbz` 的
imm19、`tbz` 的 imm14 同理），x86 的 `jmp rel32` 本身就是字节。而 DSL 的 imm/label 槽原先
把源文本的值**原样**写进字段——于是 `b #134217724` 被判越界（字段最大 33554431）、
`b 8` 编成 imm26 = 8 而 GAS/LLVM 会编成 2。

新增槽属性 `unit`（2 的幂，缺省 1；只对 `imm`/`label` 有意义）：

1. **值域按源单位**给（`imm_range` = 字段值域 × unit），越界报错；
2. 源值不是 unit 的整数倍 ⇒ 该条写法**不匹配**（与越界同处理，不静默取整）；
3. 编码写 `源值 >> log2(unit)`，**解码乘回来** ⇒ `Inst` 字段与 `disassemble` 渲染始终是
   源单位，`disassemble → assemble` 照旧闭合；
4. **符号标签**回填"块下标 × unit"（该块的字节偏移），与数字写法同单位。

这与宿主的 `RelocPatcher` 是同一口径（`Arm64RelocPatcher` 就是 `imm = offset >> 2`），
于是"汇编器里的数字写法"与"编译器打补丁"不再各说各话。**只实现于定宽（含 `mixed`）
编码/解码**：变长（`prefix_scan`）ISA 写它会**校验期 fail-closed**（明确报错，不静默当 1）。
编码期的"非整数倍"检查只对 **imm 槽**发——label 槽在编译器路径上塞的是**标签 id 占位**
（`LabelRef::EPILOGUE.id()`，不是字节偏移，随后由 reloc patcher 整体改写），查对齐只会误报；
文本路径的对齐检查在 `__imm`/`__label` 里，两条路都覆盖到了。

aarch64 落地：`off26`/`off19` 声明 `unit = 4`；另补 `[meta].imm_prefix = "#"`（模板虽都写
`#{imm}`，但**分支目标槽前没有字面 `#`**，`b #28` 的前缀得由这条声明来吃）。
生成期自测同步：立即数边界按字段单位算完再乘回源单位；越界探针按 unit 步进（否则
"非整数倍"会先把探针拦下，值域检查永远测不到）。

效果：aarch64 语料解析档 `parsed` 115 → **123**、红桶 27 → **19**，字节对拍 93 → **98 条
逐字节全等**（`known` 仍 0——含上游注释里 `cbz w20, #1048572` 这类期望字节）。

### Changed (2026-10-04) — `[[operand_slots]].roles` 由列表改为**一个** `OperandRole`（破坏性）

槽的角色声明从 `roles = ["in", "out"]` 改成 `roles = "inout"`——**一个 `OperandRole` 就够**：
`inout`（读改写）在语义上已经涵盖 `in` 与 `out`，旧的三种拼法
（`["in"]` / `["in", "out"]` / `["in", "out", "inout"]`）里后两种是同一件事，而且
`["in", "out"]` 还**接不住**读改写操作数（必须在集合里再补一个 `inout`）——那正是这次要
消掉的冗余。

- 判定口径：声明 `inout` 的槽接受 `in`/`out`/`inout` 三种操作数声明，声明 `in` 的槽只接受
  `in`，声明 `out` 的槽只接受 `out`；**不写 = 不限制**（旧的 `roles` 缺省语义在代码里本就是
  "不校验"，文档写"缺省 `["in"]`"是漂移，一并改正）。
- 连带删掉两条只对集合有意义的校验（"roles 不能为空"/"角色不能重复"）。
- 迁移：三份发行谱 + 7 份夹具 + `forge-isa-dsl` 内的 22 处字符串夹具，合计 45 处
  （`docs/archive/**` 是历史记录，按仓库约定不动）。
- 守卫照旧：`forge-isa-dsl` 全绿（含 `inout_role_parses` 改成断言 `Some(OperandRole::InOut)`）、
  `schema_guard` 绿（schema 里该键只有描述、无类型，`isa-dsl.schema.json` 无需重生成）。

### Changed (2026-10-04) — 生成物落盘件改写成 prettyplease 规范形（可读 + 编辑器能解析）

`isa_from_file!` 的落盘件（`$OUT_DIR/forge_gen_*.rs`）此前是**紧凑 token 文本**（x86 单行
1.4 MB）——人排查生成物时读不动，编辑器/rust-analyzer 对这种单行文件也给不出有意义的语法树。

现在正文过一遍 `syn::parse2` + `prettyplease::unparse`（rustfmt 风格），并加了两条不变量
（守卫 `gen_file::tests::generated_file_is_a_faithful_rust_file`）：

- **规范形**：`unparse(parse(落盘件))` 逐字节等于落盘件 ⇒ 同一份谱每轮生成的字节稳定
  （否则每轮重写，rustc 每轮重编这个生成模块）；
- **忠实**：解析回来的程序与生成 token 的规范形逐字节相等。忠实性**不比 token 文本**——
  prettyplease 会把 `#[doc = r"…"]` 渲染成 `/// …`（解析回来是 `#[doc = "…"]`），是同一个
  字符串值的两种合法拼写；比 token 文本会误报这处差异。

成本（dev 档实测）：三份发行谱合计 ≈ 0.8 s（x86 parse 0.58 s + unparse 0.20 s，
arm64/riscv64 各 ≈ 0.3 s），夹具谱可忽略；落盘件体积约为紧凑形的 3 倍。生成物的**对外面**
不变（`isa-host-demo` 的 `host_surface` 文本级守卫按空白归一化后照旧通过——它按
`# [doc = r"…"]` 属性形剥散文，规范形把文档渲染成 `/// …` 行注释，两形都已兼容）。

顺带修掉三处**过期快照**（前几批补指令后没同步，与本批功能无关）：`lint_shipped` 的
arm64 未指定位评审清单 71 → 123；`variants` 的 riscv64 原生/RV32 投影指令数
119/105 → 137/120（被投影掉的 RV64 专属指令 14 → 17）；`isa/arm64.toml` 的
`ands`/`bics` 无后缀形态去掉两个没人引用的 `ref`。

### Fixed (2026-10-04) — 定宽解码位 trie 的 `else if` 链把后一条边变成死代码

**症状**：`bics x0, x1, x2`（以及 `bics` 的移位形态、`bics w0, w1, w2`）**编得出、解不回**——
`encode` 给出正确字节 `20 00 22 ea`，`decode` 却返回 `None`。同族的 `ands` 一切正常。

**根因**（`forge-isa-dsl` 的 `dsl/codegen/mod.rs::emit_bit_trie`）：定宽解码的位级 trie 把同一
节点上的边展开成 **`else if` 链**。但同一节点上两条边的位区间**可以不相交、却同时命中同一个
字**——`bics` 的 N 位在 bit21、逻辑族的移位在 bits[22,24)，`0xEA220020` 两条都命中。`else if`
只在**前一条边的条件为假**时才试下一条，于是"条件为真、但子树的叶子 guard 没过而返回不了"
会**直接跳出整条链**：`ands` 的移位边先接住这个字、guard 不过，`bics` 那条边永远到不了。

**修法**：边**平铺成互不嵌套的 `if`**（试一条、没返回就试下一条），与定宽 decode 的既有语义
（叶 arm 优先 + 声明序）一致。变长（字节）decode 的 `emit_dec_trie` 不受影响——那里同一节点的
掩码由 `check_dec_trie_overlaps` 保证互斥，`else if` 既安全又完整（该处注释已写明这个区别）。

**回归**：`crates/backend/forge-codegen/tests/arm64_tests.rs::decode_bit_trie_disjoint_edges_fall_through`
（5 条真实写法编解码往返）、`isa/arm64.toml` 的 `bics x0, x1, x2` 谱内向量，以及生成期自测
（`spec_bics*` 全部由红转绿）。这条缺陷此前不显形，是因为**没有任何谱在同一 opcode 节点上同时
用到"位区间不相交却不是同一位域"的两条边**；补 aarch64 逻辑族（移位后缀 + N 位）才第一次踩到。

### Added (2026-10-04) — aarch64：补逻辑（移位寄存器）族（移位后缀 + `ands`/`bics`）

语料 `arm64-logical-encoding.s` 里 68 行卡在逻辑族的**移位后缀**与 `ands`/`bics` 的无后缀形态上，
这批补齐（**纯谱数据，生成器零改动**）：

- 位域新增 `shift`（[22,24)，2 位常量）、`imm6`（[10,16)）、`imm5`（[10,16) 的 5 位读法）；
  表单 `ALUSH6`/`ALUSH5`（`rd/rn/rm` + 移位量）；操作数槽/模板各若干。
- 指令 64 条 = `and`/`ands`/`bic`/`bics`/`orr`/`orn`/`eor`/`eon` × `lsl`/`lsr`/`asr`/`ror` × X/W，
  另加 `ands`/`bics` 的无后缀形态 4 条（此前整族未声明）→ arm64 指令总数 120 → **188**。
- **为什么移位种类是"逐指令常量"而不是新语法**：移位量是操作数（6 位/5 位槽），移位种类是 2 位
  常量——`[[templates]]` 的行正好是"同一形状、逐行给常量"的载体，故 8 条模板 × 8 行即可，
  DSL 不需要"值 → 多字段"这类新能力（位掩码立即数与 `tbz`/`tbnz` 仍需要，见
  `crates/tools/forge-tests/asm/README.md` 的缺口清单）。
- **W 形式用 5 位槽**：A64 规定 32 位形式的 `imm6<32`；用 6 位槽会静默编出未分配的字。
- 4 条谱内 `[[vectors]]`（`and … ror #2`、`bics x0, x1, x2`、`bics … asr #3`、`orn w … lsl #7`）。

效果：aarch64 解析档 `parsed` 47 → **115**、`no_prefix` 35 → **11**、红桶 71 → **27**；
字节对拍 `checked` 25 → **93 条逐字节全等**（`known` 仍 0）——新族的每个字节都与 LLVM 上游注释相同。

### Fixed (2026-10-04) — aarch64：两处把 `#` 当注释的缺陷（立即数全被截断）+ 系统/异常族

补 aarch64 系统/异常族时撞出**两处独立缺陷**，根因都是"`#` 被当成注释"——AArch64 的 `#` 是**立即数前缀**（注释是 `//`）：

1. **语料档**（`crates/tools/forge-tests/src/asm/corpus.rs`）：aarch64 suite 的 `comments` 列了 `#`，于是 `tbz x1, #3, foo` 被截成 `tbz x1,`、`cbz w1, #28` 截成 `cbz w1,`，这些行被误判成"本 ISA 没有这条指令"。实测这批文件里**没有以 `#` 开头的注释行**（LLVM 的 AArch64 用例用 `;` 与 `//`），`#` 只出现在立即数位置 ⇒ 从注释集里去掉。棘轮样本随即可见真实文本（`b #28`、`cbz w1, #28`、`and w0, w0, #1`）。
2. **谱**（`isa/arm64.toml`）：`comment_char = "#"` ——生成物对**每一行**按 `#` 截断，`add x0, x1, #5`、`ldr x0, [x1, #8]` 全被砍成 `add x0, x1,` 而解析失败。这解释了 aarch64 语料 `parsed` 长期只有 37/420。改成 `comment_char = "/"`（A64 的注释是 `//`，取单字符后正好是它）。

顺带修的（同一批暴露的）：

- **`[meta].imm_prefix` 只实现了 `$`**（声明 `#` 静默不生效）：现在按声明吃对应 token（词法只有 `Dollar`/`Hash`），并在声明期**拒掉**其它字符（"写了却不起作用"改成"写不出来"）。
- **`SYSEXC` form 的位域重叠**：系统/异常的顶层常量我一开始写成 `b16`（[31:16]），与 `imm16`（[20:5]）在 bit16-20 重叠 ⇒ imm16 取大值时把顶层常量写坏（**编得出、解不回**）。派生日志守卫实测 `SVC[imm16=65535]` 解码失败当场抓到；改成 11 位顶层（[31:21]，复用 `mtop` 段）后 32 位三段互不重叠。

**补的族**：`SVC`/`HVC`/`SMC`/`BRK`/`HLT`/`DCPS1..3`（`SYSEXC`：11 位顶层 + `imm16[20:5]` + `op5[4:0]`）与整字常量 `ERET`（0xD69F03E0）/`DRPS`（0xD6BF03E0）。模板里 `#` 与其它 A64 指令一致写成 `#{imm}` 字面量。

效果：aarch64 解析档 `parsed` 37 → **47**、`no_prefix` 45 → **35**，字节对拍 `checked` 15 → **25 条逐字节全等**（`known` 仍 0）；另加 4 条谱内 `[[vectors]]`（`brk #1`、`svc #8`、`brk #65535`（钉住重叠那处修复）、`eret`）。arm64 指令总数 110 → **120**、派生枚举器 360 → **386**（人工复核后就地更新）；x86 275 / riscv64 137 不变。

剩余 aarch64 缺口已按根因登记：位掩码立即数编码（值 → N/immr/imms）、移位后缀操作数、`tbz`/`tbnz`、分支立即数目标、`dcpsn` 的省略立即数（需要"可选操作数"）、`b.al`、`ret lr`。

### Added (2026-10-04) — riscv64：补完 Zicsr/`fence.i`/`fence.tso`/`BGEU` 与别名 `unimp`（`no_prefix` 桶清零）

语料的 `no_prefix` 桶（"没有任何候选的首段能对上" = 缺指令）在上一批之后还剩 14 条，这批清到 **0**：

- **`BGEU`**：`BB` 分支模板只少了这一行（`funct3 = 7`）。
- **Zicsr 六条**：`CSRRW`/`CSRRS`/`CSRRC`/`CSRRWI`/`CSRRSI`/`CSRRCI`（opcode 0x73，`funct3` 1/2/3/5/6/7）。
  文本里的操作数序（rd, csr, 源）与字段绑定序（rd, rs1, imm12 = I 型 form）**不必一致**——`asm` 用命名引用，绑定由 `operand_fields` 决定（v15-S3c 的命名操作数设计，这里正好用上）。
- **`FENCE_I`**（0x0000100F）与 **`FENCE_TSO`**（0x8330000F = `fence rw, rw` + `fm=8`）：整字常量，与 `FENCE` 一样用 `W32` 形态。
- **`unimp`**：LLVM/GAS 把它定义为 `csrrw x0, cycle, x0`（0xC0001073）——它是**别名**不是新编码，声明成整字指令会与 `CSRRW` 撞车（解码器按"编码不相交"建 trie，直接 `DSL-OTHER ... overlap ... ambiguous` 报错），所以用 `[[pseudo]]` 展开一条。

两个值得记的坑：

1. **CSR 号是 12 位无符号**（`0xfff` = 4095），拿有符号的 `imm12` 槽（load/store 位移用）会让 `csrrw t0, 0xfff, t1` 报越界 ⇒ 新增 `csr12` 槽（unsigned，12 位），三条 `*I` 的 zimm 另用 `zimm5`（unsigned，5 位，与 `shamt_w` 同宽但语义不同，单列）。
2. `shamt_w` 的注释顺带改正：用它的是 0x1B 的 `slliw`/`srliw`/`sraiw`（0x3B 那三条是寄存器移位）。

结果（`asm/ratchet/`）：riscv64 解析档 `parsed` 115 → **129**、**`no_prefix` 14 → 0**、红桶 13 不变；字节对拍 `checked` 115 → **129 条逐字节全等**，`known` 仍 0。另加 5 条谱内 `[[vectors]]`（`bgeu`/`csrrw t0, 0xfff, t1`/`csrrsi`/`fence.tso`/`unimp`）。riscv64 指令总数 128 → **137**、派生枚举器 383 → **435**（人工复核后就地更新）；x86 275 / arm64 110 不变。

剩余 13 条红桶已按**根因**登记进 `crates/tools/forge-tests/asm/README.md`：① 需要重定位/符号地址（5 条，缺"未定义符号 → 重定位记录"）；② 位置符号 `.`（2 条）；③ **`fence` 的字母集合操作数**（4 条）——需要一类**新能力**「命名位集合」操作数（表 + `kind = "bitset"` 槽：按表里名字贪心拼接解析、取位或编码、按声明序拼接渲染），不是 `fence` 的特例，先登记不做；另加两寄存器形式的 `jal`。

### Added (2026-10-04) — riscv64：字节/半字访存、W 立即数移位；修掉 W 寄存器移位的编码错误

从语料的 `no_prefix` 桶（"没有任何候选的首段能对上"= 缺指令）按首词归并出来的清单里，补上最基础的一族，并顺带修掉一处**真编码错误**：

- **补** `LB`/`LH`/`LBU`/`LHU`/`SB`/`SH`（与 `LW`/`SW` 同形，只差 `funct3`）与 `SLLIW`/`SRLIW`/`SRAIW`（opcode 0x1B + `shamt_w`，5 位 shamt 独立于 6 位槽）。
- **修**：`SLLW`/`SRLW`/`SRAW` 原先被声明成**立即数**移位（0x3B + shamt 槽）——ISA 里它们是 **R 型寄存器**移位（rs2 = 移位量），立即数那条是 0x1B 的 `slliw`/`srliw`/`sraiw`。三条改成 `WW` 模板的行（与 `ADDW`…`REMUW` 同形）；`shamt_w` 槽由新的 `*IW` 三条接着用，不留死声明。
- **求值器补一元 `!` = 逻辑非**（`!x` = x==0 ? 1 : 0）：上游期望字节实证 `lh t1, !1(zero)` 的位移是 **0**（`03 13 00 00`）而不是按位取反的 -2（那条写的是 `~2047`）。这是**表达式运算符**（与 `~` 同类，任何 ISA 都可能出现在立即数里），不是 ISA 知识。

效果（`asm/ratchet/`）：riscv64 解析档 `parsed` 89 → **115**、红桶 15 → **13**、`no_prefix` 38 → **14**；字节对拍 `checked` 89 → **115 条逐字节全等**，`known` 仍 0——两族新指令的每个字节都与上游注释相同（不是我手算的）。另加 4 条谱内 `[[vectors]]`（`lb`/`sh`/`slliw`/`lh t1, !1(zero)`）把语义钉在谱里；x86 指令总数 275 不变，riscv64 119 → **128**、派生枚举器 341 → **383**（人工复核后就地更新）。

`crates/tools/forge-tests/asm/README.md` 的缺口清单同步收窄：riscv64 红桶只剩三类——需要重定位/符号地址（`%pcrel_hi(foo)` 等）、位置符号（`.`）、寄存器形式的 `jal`/`jalr`；`no_prefix` 只剩 Zicsr、`fence` 族与 `unimp`。

### Added (2026-10-04) — `[[conventions.imm_fn]]`：立即数修饰改成**谱声明的数据**（DSL 里没有修饰名）

各家的汇编器都有一批**立即数修饰写法**：GAS/RISC-V 的 `%hi(x)` / `%lo(x)`、GAS/ARM 的 `:lower16:x` / `:upper16:x`、MIPS 的 `hi(x)` / `lo(x)`——拼写不同、值语义也不同（带不带进位补偿、掩码几位、按不按有符号读数）。把其中某一种写进求值器，等于让通用 DSL 变成"为那个 ISA 设计"的系统；所以新能力是一张**声明表**，每条两行数据：

```toml
[[conventions.imm_fn]]
name = "hi"                  # 名字（唯一诊断名；不参与文本匹配）
text = "%hi({0})"            # 源文本形态：`{0}` 处是内层表达式，其余按字面 token 匹配
expr = "({0} + 0x800) >> 12" # 值语义：`{0}` 处是内层表达式的值
```

- **匹配**：`text` 的两段是字面 token（`mnemonic_case = insensitive` 时 Ident 豁免大小写），中间的 `{0}` 递归解析成一个表达式；前缀/后缀/两侧都有字面都行（`{0}:lo` 合法）。
- **求值**：`expr` 用**既有表达式语言**——生成期把 `expr` 词法化，运行期把 `{0}` 换成数值 token 再走 `__expr`，因此任意算术与任意嵌套（`%hi(%lo(x))`、`expr` 里再用别的修饰）天然成立，**不引入第二套表达式语言**。
- **缺省=没有**：没声明 `imm_fn` 的谱生成物里根本没有修饰机制（与引入前逐字相同）；没声明的拼写（`%hi` 在 x86 上）就是"这条写法不匹配"，不猜语义、不当常量。
- **校验**：`name` 非空唯一；`text`/`expr` 各含**恰好一个** `{0}`；`text` 必须含至少一个字面（光一个 `{0}` 是通配，会吞掉任意表达式）。
- **三方针同步**：`dsl/model.rs`（`ImmFnDef`）/`src/schema.rs` + 重新生成的 `isa-dsl.schema.json` / `docs/reference/isa-dsl.md`（键速查表 + 「立即数修饰」节，含"为什么是数据而不是内置"的说明）。

**通用性的正面证据**：同一机制、零代码改动，`isa/riscv64.toml` 声明 `%hi(x)`/`%lo(x)`（带括号、进位补偿 + 低 12 位有符号读数），夹具 `crates/backend/forge-codegen/tests/isa/demo.toml` 声明 `:lower16:x`/`:upper16:x`（**无括号、纯掩码**），`demo_tests::imm_fn_is_spec_data_not_isa_knowledge` 真跑字节并验证嵌套与"未声明拼写报错"。

效果：riscv64 语料解析档 `parsed` 82 → **89**（红桶 22 → **15**）、字节对拍 `checked` 82 → **89 条逐字节全等**（`lui a0, %hi(2)` = `37 05 00 00`、`jalr a0, %lo(2048)(a1)` = `67 85 05 80`、`lw a0, %lo(97)(a2)` = `03 25 16 06`…，`known` 仍 0），另加 4 条谱内 `[[vectors]]` 把语义钉在谱里。**未实现**（如实登记）：需要重定位的修饰（`%pcrel_hi(foo)` 且 `foo` 未定义）——那不是值语义能表达的，要有重定位记录。

### Changed (2026-10-04) — `[[pseudo]]` 改 `asm` 声明形态：参数表从写法里长出来（破坏性，无兼容层）

`[[pseudo]]` 的 `params` 列表**删除**，改成两个键：`name` 是这条声明的**唯一标识**（与写法无关）、`asm` 是**这条伪指令的书写规范**（分派按它的前导字面），`asm` 里的 `{名字}` **就是**参数——`emit` 里引用的是同一批名字，没有第二份清单可以漂移：

```toml
[[pseudo]]
name = "li"                     # 声明标识（唯一即可，不必等于写法首词）
asm  = "li {rd}, {imm}"         # 书写规范：`{名字}` 即参数（按首次出现序）
emit = ["lui {rd}, (({imm} + 0x800) >> 12)", "addi {rd}, {rd}, ((({imm} + 0x800) & 0xfff) - 0x800)"]
```

- **`name` 与写法解耦**：分派看 `asm` 首段的首词（同一分派键下可有多条写法，按声明序逐个试），`name = "load_imm"` + `asm = "li {rd}, {imm}"` 合法；遮蔽检查也跟着走**写法**而不是名字（`name = "alias"` + `asm = "mov …"` 会被拦下）。顺带把文档里一直写着的"emit 行可以嵌套别的伪指令"**真正实现**（展开行递归再判），并让 ci ISA 的首词匹配大小写不敏感。
- **切参按模板的字面段走**（不再"按逗号位置猜"）：`,` 是模板写死的分隔符，`li x10,0x1234`（无空格）照样认；缺分隔符 / 多一段 / 尾部多东西都**当场报错并回显写法**（`伪指令 'li' 的写法是 'li {rd}, {imm}'：缺 ','`）。
- **展开行装配失败时点名是哪一行展开出来的**：`li x10, 0x1234 junk` 以前只会得到 "no matching instruction"，现在是 `伪指令展开行 'lui x10, ((0x1234 junk + 0x800) >> 12)' 汇编失败：…`。
- 校验同步收紧：`asm` 首字面必须**就是 `name`**（伪指令按整词分派）、占位符之间必须有字面分隔、`emit` 里的 `{…}` 必须是 `asm` 声明过的参数（报错时回显 `asm`）、`asm` 声明的参数必须都被用到；零操作数伪指令合法（参数表就是空的）；同一名字在 `asm` 里出现两次表示**前后必须写成一样**。
- 三个同步点一起改：`dsl/model.rs` 结构体、`src/schema.rs` 键表 + 重新生成的 `isa-dsl.schema.json`、`docs/reference/isa-dsl.md` 的键速查表与 `[[pseudo]]` 小节（`schema_guard` 三方针全绿）。

### Added (2026-10-04) — x86 SSE 比较谓词族 + 八个经典别名（`cmpltps` 等）

上游 `intel-syntax-encoding.s` 的 `cmpltps XMM2, XMM1` 是谱里登记的缺口：x86 从来没有 SSE 谓词比较指令。补 `CMPPS`/`CMPPD`/`CMPSS`/`CMPSD_SCALAR`（`0F C2 /r ib`，`ps/pd/ss/sd` 靠强制前缀区分，谓词是立即数第 3 操作数），八个经典 packed 别名（`cmpeqps`/`cmpltps`/`cmpleps`/`cmpunordps`/`cmpneqps`/`cmpnltps`/`cmpnleps`/`cmpordps` 及 `pd` 同族）用 `[[pseudo]]` **文本展开**表达——一行别名换一行 `cmpps {a}, {b}, <谓词>`，不新增 32 条指令、也不引入第二套分派（标量别名刻意不做：那会盖掉 `cmpss xmm, xmm, imm8` 的三操作数写法）。

证据：`cmpltps XMM2, XMM1` → `0F C2 D1 01` 与上游注释逐字节相同，解析档 `llvm-mc` x86 `parsed` 35 → **36**（`no_prefix` 21 → 20），字节对拍 x86 `checked` 30 → **31**（`known` 仍 0）；四条指令进生成期自测与派生枚举器（x86 指令总数 271 → **275**、枚举器 1327 → **1351**，歧义名单未变）。

### Added (2026-10-04) — x86 内存形式的 `mov` 族（8/16/32 位）+ `movsxd`/`xchg` 的内存源形式

真实汇编里最常见的 `mov eax, [rbx+8]`、`mov [rbx+8], eax` 在谱里**一条候选都没有**：已有的 `MOV_R_MEM` 系列 rm 引用的是 `reg` 槽（`[{src}]` 简写——那是给 IR 降级用的 **vreg 基址**，没有位移/索引/尺寸前缀），而 `MOV64_RM`/`MOV64_MR` 固定 `opsize = 64`（且带 `stack_arg_*` 角色，不能动）。补 6 条内存形式指令：

- `MOV_R_MEM_AUTO`(8B) / `STORE_MEM_R_AUTO`(89)：16/32 位 load/store，走**完整内存模板**（位移、`index*scale`、尺寸前缀、无基址），66 前缀由数据寄存器宽度驱动；
- `MOV_R_MEM_8_AUTO`(8A) / `STORE_MEM_R_8_AUTO`(88)：8 位是独立操作码，同样走完整模板（改前连 `mov al, [rbx+2]` 都解不出来）；
- `MOVSXD_R_MEM`(63)：`movsxd rax, dword ptr [mem]`（原来只有寄存器源）；
- `XCHG_MEM_R_AUTO`(87)：带位移的 `xchg [mem], r`（原来的 `XCHG_MEM_R` 是简写、表示不了位移）。

新槽 `gpr24`（16/32 位）刻意**不含 64 位**：64 位的内存 mov 已由 `MOV64_RM`/`MOV64_MR` 覆盖，再收 64 位就是同一批字节的第二条候选（无冗余）。六条都**不写 `ref`**（与 ALU 内存族同纪律）：IR 降级路径不变，候选集与运行期行为逐字节不动，JIT 矩阵不受影响。

证据：字节逐条钉住（`mov ax, [rbx+2]` = `66 8B 43 02`、`mov r8w, [rbx+2]` = `66 44 8B 43 02`、`mov [rbx+2], r8d` = `44 89 43 02`、`movsxd rax, dword ptr [rbx]` = `48 63 03`、无基址 `mov eax, [0x12345678]` = `8B 04 25 …`）+ 编解码闭环测试；并且 `asm/exec/x86/` 新增 `mem_mov32.s` / `mem_mov16.s` 两个小程序**真执行**（`ASM-EXEC-SUMMARY ran=8 → 10 skipped=0`）。守卫同步：x86 指令总数 265 → **271**、派生枚举器 1263 → **1327**（歧义名单未变）。语料棘轮不动（vendored 语料里没有这些写法，全量语料里遍地都是）。

顺带登记一个**设计级**缺口（不在这批修）：`movzx`/`movsx` 的内存源形式卡在"文本分不出源宽度"——`byte`/`word` 是 `{size}` 组件、按设计不携带宽度，两条指令的文本与类型签名完全相同（已在歧义名单登记），照搬一套 16/32 位目的地 variant 只会让 `movzx eax, word ptr [rbx]` 静默编成 byte 那条；要修得先决定 `{size}` 是否携带宽度，或把尺寸关键字写成模板字面量。

### Added (2026-10-04) — 汇编器：`.set`/`.equ` 符号常量、位移与立即数共用值语法、地址尺寸覆盖前缀 `0x67`

三条都是**汇编器 / DSL 语义**的用户可见变化（破坏性，无兼容层）：

- **符号常量**：`.equ name, expr` 与 `.set name, expr` 是同一个伪指令的两种拼法（GAS 两个名字等价，LLVM 用例写的是 `.set`）；**顺序求值**（前向引用失败、同名后定义覆盖前定义）。引用它的地方与字面量走**同一套值语法**——**立即数与内存位移都能写**（`cmp eax, FOO`、`mov rax, FOO[rbx]`、`[rbx+FOO*2]`）。位移以前只认字面量（`__raw_int` / `__raw_signed_int` 两个 helper + 四段分支），现在与立即数共用递归下降求值器 `__expr`——**两个 helper 与那些分支整体删除**（无冗余）；一元 `+` 变恒等（上游语料 `lwu x2, +4(x3)` 就是这种写法）。
- **地址尺寸覆盖前缀**（新数据：`[[conventions.prefix_scan]].effects = ["addr32"]` / `["addr16"]`，x86 = `0x67`）：内存 base/index 寄存器宽度 ≠ `[meta].addr_width` 时（64 位模式下的 `[eax]`）**编码器自动发**这条前缀，**解码器**看到它就把 base/index 建成覆盖宽度的地址类（`[eax]` 与 `[rax]` 是不同地址，不能都建成 64 位）；`[base]` 简写（`reg` 槽当基址）的槽也是地址，同样按地址类解。**修掉的缺陷**：改前 `add eax, [eax]` 静默编成 `03 00`（真值 `67 03 00`）——地址尺寸错、字节却认得出，`gnu-gas-intel` 语料里 32 条 32 位地址行的字节据此由错转对。覆盖宽度必须指向本谱**已声明**的寄存器组；一个 ISA 只支持一个覆盖宽度；缺省扫描集**不含**该效果（是谱自己声明的数据，不隐式强加给别的 `prefix_scan` 谱）。
- **语料台**（`crates/tools/forge-tests`）：`.equ`/`.set` 定义**按行号**喂进行上下文——只喂本行**之前**的定义（`rv32i-valid.s` 里 `CONST` 先 30 后 16，整篇一起喂会把两个引用解成同一值，字节对拍会假红）；`encoding:` 注释的位置改成**带起始行的逐段声明**（`intel-syntax-encoding.s` 前 94 行是"注释在指令前"、尾部两条是"注释在指令后"，整篇一个风格会把 `[0x83,0xf8,0x02]` 配给后一条指令、凭空造出一条字节差异）；字节对拍档开始与解析档共用同一份上下文。

效果（数值以 `crates/tools/forge-tests/asm/ratchet/` 为准）：解析档 x86 `llvm-mc` 33 → **35**（`cmp eax, FOO` 与 `cmp eax, FOO[eax]` 都从红桶进 `parsed`）、riscv64 72 → **82**、x86 `gnu-gas-intel` 50 条不变；**字节对拍** x86 28 → **30 条逐字节全等**、riscv64 72 → **82 条逐字节全等**，`known` 与 `variants` 全 0。`gnu-gas-intel` 里那 32 条 32 位地址行现在编出的是**带 `0x67` 的正确字节**（不是"以前也算 parsed"）。

### Changed (2026-10-03) — `MemRef.base` 改 `Option<Reg>`：无基址（绝对）寻址（破坏性，无兼容层）

`MemRef.base` 从 `Reg` 改成 `Option<Reg>`，**基址的有无由模板形状声明**：模板里**写了 `{base}`** 就是"这条写法必须有基址"（缺了整条不匹配）；**没写 `{base}`** 就是"这条写法**没有基址**"（`base = None`），此时其余组件一律按**必需**处理（没有基址时位移/索引就是地址本身，否则 `[0]` 会渲染成 `]`）。校验：模板最多一个 `{base}`；无基址模板必须含 `{disp}` 或 `{index}`。这样"要不要基址"只有一处声明，不需要新键，也没有第二个开关可与它矛盾。

- x86 用 `mod=00` + `rm=100` + `SIB.base=101` + disp32 表示无基址（ModRM 的 `mod=00`/`rm=101` 是 RIP 相对，本 DSL 仍明确拒绝）；编解码两侧对称。
- 顺带**合并两份重复的内存编解码实现**（"无冗余"）：编码侧普通 ModRM 与 VEX 两条**逐字节相同**的 ModRM/SIB/位移发射合成一个 `gen_mem_modrm`（EVEX 的 disp8 要按 `disp_scale` 压缩，另存一份）；解码侧三条（普通/VEX/EVEX）的 SIB+位移解码合成一个 `gen_mem_decode`（用 `memref` / `disp8_scale` 两个参数表达差异）。
- 只写 `[base]` 的**简写**（`rm = "[槽名]"`，`reg` 槽当基址）表示不了无基址 ⇒ 解码时**显式拒绝**：不拒就会把绝对地址当成 `[RBP]` 解出来、还少读 4 字节 disp32，把真正的 MemRef 形式挤掉（本轮实测 `48 8B 04 25 …` 一度被 `MOV_R_MEM` 抢走，`spec_vector_83` 当场抓到）。
- x86 谱声明第三条模板 `{size}[{disp}]`，并加一条 `[[vectors]]` 钉住它（`mov RAX, [-8]` → `48 8B 04 25 F8 FF FF FF`）。效果：`llvm-mc` 解析档 32 → **33**；x86 字节对拍 27 → **28 条逐字节全等、0 条差异**——`movsd XMM5, QWORD PTR [-8]` 编出的 `f2 0f 10 2c 25 f8 ff ff ff` 与上游注释里的期望字节一致。"无基址寻址"据此从 `asm/README.md` 的缺口清单移除。

### Added (2026-10-03) — x86 一元 `inc`/`dec`（`FF /0`、`FF /1`、`FE /0`、`FE /1`）

与 `not`/`neg` 同族（`MRR_EXT_OP`：reg = 固定扩展码、rm = 目的操作数）：16/32/64 位走 `FF /0`、`FF /1`，8 位走 `FE /0`、`FE /1`——`inc`/`dec` 的 16 位短编码（`40+r`..`4F`）在长模式里已经是 REX 前缀，所以只能用 `FF` 这一组。四条指令写成两条 `[[templates]]`（各 2 行）；**不写 `ref`**：IR 的 `+1`/`-1` 走 ADD/SUB 的立即数形式，降级不发射它们，所以候选集与运行期行为不变（JIT 矩阵逐条一致）。

效果：`gnu-gas-intel` 解析档 `parsed` 48 → **50**、`no_prefix` 6 → **4**，64 条里只剩 7 条红桶（全是 32 位模式专有的段寄存器 push/pop）。

**归因口径变化（不是回归）**：补上 `inc`/`dec` 后，`apx-rex2-format-intel.s` 里 8 行 APX 形态的 `inc`/`dec`（`inc r16d`、`dec dword ptr [rax + r16]`）从 `no_prefix`（"没有候选的首段能对上"）挪进了 `tail_mismatch`（"首段对得上、后面没对上"）——以前连 `inc` 都不认识，现在知道助记符对、错在 APX 操作数。`parsed` 不变，红桶数字变化仅此一项：`llvm-mc` 的 `no_prefix` 29 → **21**、`tail_mismatch` 50 → **58**。

### Added (2026-10-03) — 立即数槽 `wrap`：字面量按 W 位位模式读（两种读数都收）

立即数字段是 W 位**位模式**——`0x90909090` 与 `-1869574000` 是同一批字节，各家汇编器两种写法都收。DSL 原先只收 `signed` 决定的那一种读数，于是 x86 的 `add eax, 0x90909090`（`imm32` 是**有符号** 32 位，放不下 `0x90909090`）汇编不了，`gnu-gas-intel` 有 8 行卡在这里。

新槽键 `wrap = true`（声明侧校验：仅 `kind = "imm"` 且 `width < 64`）：

- **接受域**放宽到两种读数的并集（有符号槽多收无符号写法、无符号槽多收负数写法）；
- 解析后**规范化**回 `signed` 的读数再进编码（`v > 规范上界 ⇒ v - 2^W`、`v < 规范下界 ⇒ v + 2^W`）；
- **规范值域 `imm_range()` 不变**：解码的符号扩展、反汇编渲染、生成期自测的 `min`/`max` 边界检查照旧——所以 `add rax, -12` 的文本与字节逐字节不变；超出 W 位**仍然报错**（不静默截断/掩码）；
- 候选特异性（`form_specificity`）与类型签名（`type_signature`）改按**接受**域计（口径是"能接受多少输入"），规范域只影响解码侧。

x86 谱据此给 `imm32` 打开它，并加了一条 `[[vectors]]`（`add EAX, 0x90909090` → `81 C0 90 90 90 90`）：`gnu-gas-intel` 解析档 `parsed` 40 → **48**，红桶只剩 7 条**段寄存器 push/pop**——那些是 **32 位模式专有语法**（长模式下只有 `fs`/`gs` 可 push/pop，`daa`/`das`/`aaa`/`aas` 也已移除），照抄等于给 x86-64 编非法指令，已在 `asm/README.md` 登记为"边界"而非"缺口"。规范见 `docs/reference/isa-dsl.md` 的「立即数字面量的两种读数」。

### Added (2026-10-03) — x86 8 位 ALU 族（`*_MR_8` / `*_R_MEM_8` / `*_RM8_IMM8` 各 8 条）

x86 的 8 位算术是**独立操作码**（不是同一操作码换宽度前缀）：`00 /r`（r/m8 ← r/m8 op r8）、`02 /r`（r8 ← r8 op r/m8）、`80 /digit ib`（r/m8 ← r/m8 op imm8）——ADD/OR/ADC/SBB/AND/SUB/XOR/CMP 各一条，共 24 条，一条 `[[templates]]` 各 8 行。寄存器槽用 `gpr1b`（class `gpr1` + `byte_reg = true`），并给它补上 `inout`（8 位 ALU 是读改写）。与 16/32/64 位族一样**不写 `ref`**，IR 降级不变（JIT 矩阵 x86 197/3/0、riscv64 136/64/0、arm64 23/177/0 与改动前逐条一致）。

顺带给操作数槽 `gpr8` 补了说明注释：它的 `class` 是 **`gpr8`（64 位名字）**，这是**有意的**——`SETCC_RM8 {out}, …` 的降低文本里必须是承接布尔结果的 64 位 vreg 名（`xor {out}, {out}` 先清零，故低字节即结果），把 class 改成 `gpr1` 会让所有 Cmp/Fcmp 降级当场汇编失败。真正的 8 位名字槽是 `gpr1b`。

效果：`gnu-gas-intel` 解析档 `parsed` 16 → **40**、`tail_mismatch` 39 → **15**（只剩「无符号 imm32」与「段寄存器 push/pop」两类）；字节对拍不变（27 条逐字节全等、0 条差异）。

### Fixed (2026-10-03) — 1 字节操作数的 `opsize` 解码守卫恒为假，8 位指令解不出来

`opsize = "s0"`（或操作数名）指向**单类 1 字节槽**时，生成物会写下 `__opsize == 1` 的解码守卫；而 `__opsize` 只可能取 **2/4/8**（由 66 前缀与 REX.W 扫描决定）——8 位操作数**没有** opsize 前缀可查，守卫恒假 ⇒ 该指令**解码永远失败**（实测 `00 00`（`add [rax], al`）解不出来，`isa_roundtrip_guard` 当场红）。以前没暴露是因为缺省分支的 `find(|w| *w == 2 || *w == 4 || *w == 8)` 恰好把 1 字节槽过滤掉了。现在宽度 ∉ {2,4,8} ⇒ **不发守卫**（与缺省分支同一口径）；编码侧不受影响（1 字节本来就不出 66/REX.W）。

### Changed (2026-10-03) — `[conventions.mem]` 改成模板**列表** + `{size}` 尺寸前缀（破坏性，无兼容层）

声明从 `template = "…"` 变成 `templates = ["…", …]`：**第 0 条 = 反汇编渲染形态**（渲染必须唯一，否则 `disassemble` 的输出会随解析尝试顺序漂移），其余是**解析专用备选**——按列表序逐条试，第一条整条走通的赢。同一份 `MemRef` 的几种合法写法（x86 的 Intel `[base+disp]` 与 GAS `disp[base]`）各写一条，不必硬塞进同一条模板；空列表报错。

- 新组件 `{size}`：吃掉 `size_keywords` 里的任一关键字（大小写按 `[meta].mnemonic_case`），**值不进 `MemRef`**——宽度由操作数槽/`opsize` 决定，尺寸前缀只是给人读的冗余提示（`mov QWORD PTR [rsp-16], rax` 与 `mov [rsp-16], rax` 编出同样的字节）。渲染输出空串，故 `disassemble → assemble` 照旧闭合；模板里用了 `{size}` 却没声明 `size_keywords` ⇒ 生成期报错。紧邻 `{size}` 的字面量**不是**它的条件前缀（否则 `qword ptr [rax]` 的 `[` 会随尺寸前缀一起消失）。
- `docs/reference/isa-dsl.md` 新增「`[conventions.mem]` — 内存操作数的文本形态」小节；schema 表 / 签入的 `isa-dsl.schema.json` / 键总览速查表三处同步（`schema_guard` 钉住）。
- x86 谱据此声明两条模板（`{size}[{base}+{index}*{scale}+{disp}]`、`{size}{disp}[{base}]`）与五个尺寸关键字（`byte/word/dword/qword/xmmword ptr`）：`mov QWORD PTR [RSP - 16], RAX` 这类写法开始能解析。

### Added (2026-10-03) — x86 内存形式的 ALU 族（`*_MR` / `*_R_MEM` 各 8 条）

与寄存器版**共用操作码**，区别只在 ModRM 的 mod（=11 走寄存器形式、≠11 走内存）——`ADD_RM_R`(0x01) 这类只覆盖前者，现在补上后者：目的在内存的 `ADD/OR/ADC/SBB/AND/SUB/XOR/CMP_MR`（0x01/09/11/19/21/29/31/39）与源在内存的 `*_R_MEM`（0x03/0B/13/1B/23/2B/33/3B），一条 `[[templates]]` 各 8 行；新编码形式 `MRR_MEMREF_AUTO`（`opsize = "s0"`：内存操作数自己不携带宽度，宽度由旁边的寄存器驱动）。

全部**不写 `ref`** ⇒ IR 降级不发射它们（目前只有汇编器入口需要），候选集不变、字节与运行期行为逐字节不动（JIT 矩阵 x86 197/3/0、riscv64 136/64/0 与改动前逐条一致）。效果：`gnu-gas-intel` 解析档 `parsed` 0 → **16**、`llvm-mc` 31 → **32**；x86 编码对拍 26 → **27 条逐字节对上、0 条差异**。

### Fixed (2026-10-03) — 生成期自测的文本歧义键漏了「打印出来是什么」

`text_key` 原先按**操作数声明序 + 带操作数序号**建键，两类指令因此被判错：① 只有占位符顺序相反（`add [mem], r` 是 `add {1}, {0}`、`add r, [mem]` 是 `add {0}, {1}`）——文本明显不同却被当同形，白白放弃强断言（本轮的 16 条新 ALU 内存形式全被误报）；② 序号不同但**渲染文本确实一样**（`MOV_R_RM` 目的在 reg 字段、`MOV64_RR` 目的在 rm 字段，都打印 `mov A, B`）——真歧义却被拆开，强断言落到文本分不清的指令上而变红（`MOV_R8_RM64[hi]`）。

现在键 = 字面段序列 + **按模板占位符序、不带序号**的「打印描述」：前者恢复强断言（`disasm → asm → encode` 必须回到同一字节），后者正确留在歧义名单里。x86 文本歧义名单 43 → **37**（`MOV64_MR`/`MOV64_RM`/`MOVSD_MR`/`MOVSD_RM`/`MOVUPS_MR`/`MOVUPS_RM` 实测全部通过强断言），riscv64 / arm64 名单不变。

### Fixed (2026-10-03) — 解码侧「无基址」内存形态少读 4 字节 disp32

`mod=00` + `SIB.base=101` 是 x86 的**无基址**形态（后面跟着 disp32）。解码器原先只按 SIB 取基址、**不读那 4 字节**，于是返回一个假的 `[rbp+disp]` 并少消费字节。`MemRef.base` 是必需字段、表示不了这种形态 ⇒ 现在 fail-closed 拒绝。缺陷一直在，只是没有指令占那条 opcode（0x2B）时 `decode` 直接 `Err`、看不见；补完 ALU 内存族后 `encoder_fuzz_tests` 的 `sub rax, [0x12345678]` 当场抓到。

### Added (2026-10-03) — `[reg.*].aliases`：寄存器别名（解析认、渲染出主名）

真实语料里 riscv 几乎全用 ABI 名（`lwu sp, 4(gp)`、`addw a2, a3, a4`），而我们反汇编/黄金输出
一直用 `x` 名。以前没有别名的位置，只能"再声明一份寄存器组"或改 asm 模板拼名字——都是冗余。
现在 `[reg.<name>]` 多一个**别名表**：

- `aliases = { a0 = 10, fp = 8, s0 = 8 }`——`别名 = 组内下标`，与主名**位置无关**，
  同一寄存器允许多个别名（riscv `x8` 既是 `s0` 也是 `fp`）；
- **解析认别名**（`Reg::from_str`，随 `[meta].case_insensitive_regs` 折叠大小写），
  **渲染仍用主名**（反汇编输出 `x10`，不是 `a0`）；
- 校验在 `validate_regs`：下标越界 / 空名 / 与本组主名撞车 / **跨组重名**都报错
  （跨组重名会让 `Reg::from_str` 静默取第一个）；三方守卫照旧（`schema.rs` ↔ `model.rs` ↔
  `docs/reference/isa-dsl.md` 键表 + `isa-dsl.schema.json`）。

riscv64 谱据此补上 32 个 ABI 别名（`zero`/`ra`/`sp`/`gp`/`tp`/`t0`–`t6`/`s0`–`s11`/`a0`–`a7`/`fp`）：
解析档 `parsed` 12 → **72**，编码对拍 12 → **72 条逐字节全等**。

### Fixed (2026-10-03) — `lui`/`auipc` 的立即数口径改成 GAS/LLVM 的字段值（破坏性，无兼容层）

`imm20` 原先是**预移位**位域（`pieces = [{ offset = 12, width = 20, shift = 12 }]`），
即"立即数写绝对值、编码器负责 `>>12`"。于是 `lui a0, 2` 在我们这里编成 imm20=0
（上游/规范是 `37 25 00 00` → rd = 8192），`lui t0, 1048575` 更是被截成 255——
**riscv 语料的字节对拍一接通就抓到 7 条**。现在按标准口径改（`shift = 0`，
**立即数就是 imm20 字段值**）：

- `codegen/placeholder.rs` 的五个 `*_hi20` 占位符（`{iconst_hi20}`、`{iconst_hi32_hi20}`、
  `{iconst_lo32_hi20}`、`{fconst_hi32_hi20}`、`{fconst_lo32_hi20}`）改为
  `((v + 0x800) >> 12) & 0xFFFFF`——原来由编码器的 `shift` 干的活挪到这里，
  **低位进位/掩码语义逐字节不变**（掩码还顺带把 32 位常量的回绕写清楚）；
- riscv64 谱：`imm20` 的 `shift = 12` → `0`（于是 `Lui` 的立即数**重新受 0..1048575 值域检查**，
  以前因为"预移位"整条豁免）；`li` 伪指令的 `lui` 行补 `>> 12`；
  lowering 里的字面 LUI 立即数（`0x55555000`/`0x33333000`/`0xf0f1000`/`0x0ff0000`/`0x80000`/`0x10000`）
  改成字段值（`0x55555`/`0x33333`/`0xf0f1`/`0xff0`/`0x80`/`0x10`，行尾注释留绝对值）；
  谱内 3 条 `lui X1, 4096` 向量改成 `lui X1, 1`（字节不变）；
- **riscv64 矩阵 136 passed / 64 skipped / 0 failed 不变**（编码字节与运行时行为逐条保持），
  riscv64 编码对拍 **7 条差异 → 0**。

### Added (2026-10-03) — riscv64 补 RV64I 的 `lwu` / `addiw`

两条纯 ISA 补全（`only_variants = { xlen = [64] }`，不参与 lowering）：解析档 riscv 的
`NoPrefix` 里少了它们，`insts` 也从 117 → **119** 条。守卫同步：`spec_coverage_guard`
（riscv 119）、`isa_roundtrip_guard`（riscv 派生条目 329 → **341**）、`variants.rs`
（RV32 投影丢 14 条、默认档 119）、`forge-isa` CLI 的 `insts` 计数。

### Fixed (2026-10-03) — 字节 oracle 抓到的 x86 缺陷：ALU 立即数短形式（`83 /n ib`）与 16/32 位目的地

补上真实语料当天，编码对拍档就报出 5 条差异 + 一批"解析不动"，全部是**谱缺形状**而不是语料问题：

- **`83 /n ib`（符号扩展 imm8）整族缺失**：`xor rax, 12` 以前编成 7 字节的 imm32 形式
  （`48 81 f0 0c 00 00 00`），规范/上游用 4 字节的 `48 83 f0 0c`；更糟的是**解码器也不认**
  `48 83 /6 ib`（往返守卫注释里的"别名撞车"掩盖不了这一点）。现在补 `ADD/OR/AND/SUB/XOR/CMP`
  的 imm8s 形式 + `ADC/SBB` 的 reg-reg 与 imm 形式（共 16 条），并新增槽
  `imm8s`（**有符号 8 位**——CPU 会符号扩展到操作数宽度，与给位域用的无符号 `imm8` 不能混用）。
- **16/32 位目的地**：`xor eax, 12` / `add ax, -12` 完全不解析（原先的 imm32/imm8 形式都只挂
  `dst:gpr` = 64 位）。imm32 形式的目的/源槽改成多宽度 `gprx`，于是 `66`/无前缀/REX.W
  按操作数宽度自动选。
- **同文本两条编码的分派**：imm8s 与 imm32 汇编文本相同，两处得改——
  ① `type_signature`（asm 扫描的去重键）带上立即数**值域**，否则窄值域那条会把宽值域那条
  去重掉、放不进 imm8 的立即数就没候选了；② `form_specificity`（候选排序）也把立即数计入
  "能接受多少输入"，否则 `ADD64_R_IMM32`（单类 `gpr`）会排在 `ADD_R_IMM8S`（多类）前面，
  `add rax, -12` 又被编成 imm32（这正是修完第一版后**剩下的那一条**差异，靠字节 oracle 的
  逐条对拍 + `FORGE_ASM_ENCODING_CASES=1` 才定位到）。
- **顺带删掉两条重复指令**：`ADD64_R_IMM32` / `SUB64_R_IMM32` 与多宽度的 `ADD_R_IMM32` /
  `SUB_R_IMM32` 只差 `dst:gpr`、asm 文本完全相同（前者专门承载 `frame_free`/`frame_alloc`
  角色）。角色改挂到多宽度那条上（帧调整传进来的恒是 64 位栈指针 vreg，**编码逐字节不变**，
  x86 矩阵 197/3/0 与 riscv 136/64/0 全绿可证），两条重复删除。
- 其余补齐（都是真实语料里的常见写法）：移位立即数 `SHL/SHR/SAR_RM_ONE`（`D1 /n`，`shl edi, 1`）
  与 `*_RM_IMM8`（`C1 /n ib`）、`MOV_R_IMM32`（`B8+r id`，`mov eax, 0x1234`）、
  `RET_IMM16`/`RETF`/`RETF_IMM16`、`PUSHF`/`POPF`/`PUSHFW`/`POPFW`（16 位变体走
  `prefix = "field"` + `fields = { prefix = 0x66 }`——`opsize = 16` 对**无操作数**指令发不出
  前缀；两条 `*W` 声明在裸形式**之前**，否则 `66 9c` 会先被 `PUSHF` 解掉、往返不字节稳定）。
  新增槽 `imm16` 与 32 位目的地槽 `gpr4`。
- **守卫同步**：`spec_coverage_guard` 指令总数 197 → **221**、文本歧义名单 +13（imm8s/imm32 成对、
  去掉被删的两条）；`isa_roundtrip_guard` 派生条目 602 → **819**；`[[vectors]]` 里 7 条 ALU
  立即数期望更新为新的规范短形式；`call_layout_emission.rs` 的帧分配断言跟着改名。
- **实测**：编码对拍 x86 从「6 条对上 / 5 条已知差异」变成「**26 条对上 / 0 条差异**」，
  riscv64 12 条、aarch64 15 条逐字节全等不变；解析档 x86 `llvm-mc` 的 `parsed` 7 → **31**、
  GAS 那份 `no_prefix` 18 → 6（`add/adc/sbb` 等族现在认了，剩下的是 `byte ptr`/`disp[base]`
  这类写法）；`forge-codegen --lib` 1374 全绿、forge-codegen 30 个集成档全绿、
  `forge-isa-dsl` 全绿（224 + 17 档）、x86 矩阵 **197/3/0**、riscv64 矩阵 **136/64/0**
  （编译器行为不变）、`forge-abi` 51、`forge-isa` 33、`forge-tests --lib` 50。

### Added (2026-10-03) — 语料补齐：x86/aarch64 的字节 oracle 接通，编码对拍也上计数棘轮

按 `asm/PROVENANCE.md` 的清单继续补上游语料（本机 shell 无 TLS，走 `web_fetch` 一份份取回），**六份** LLVM MC 用例进仓库，全部逐字落盘并在 `PROVENANCE.md` 记了 URL/ref/许可/sha256：

- x86：`intel-syntax-encoding.s`、`apx-rex2-format-intel.s`（Intel 语法 + `encoding:` 期望字节——x86 从此有了**字节 oracle**）；
- aarch64：`arm64-logical-encoding.s`、`arm64-branch-encoding.s`；
- riscv64：`rv32i-valid.s`、`rv64m-valid.s`。
- `SUITES` 新增 `x86/llvm-mc` 一套（key = `llvm-mc`），解析档棘轮随之重刷（x86 两套：`0/18/43/3` + `7/41/63/336`）。

编码对拍档（`tests/asm_encoding.rs`）随之加固，**外部 oracle 真的开始报东西了**：

- **期望字节的抽取支持四种上游写法**（同行尾部 / 注释自带汇编文本 / 注释在指令**后** / 注释在指令**前**）；3、4 两种**逐文件声明**（`Suite::encoding_sides`）——同一目录两种风格都有，任何"看第一条推断整篇"的启发式都会在"首条指令没有期望注释"的文件上整体错位一条（实测 `rv32i-valid.s:15` 的 `.Lpcrel_hi0: auipc …`）；前缀被拆开的期望（`acquire lock add` 的 `[0xf2]` + `[0xf0,0x48,0x01,0x00]`）与重定位形式（`[0xeb,A]`）分别"整段丢掉（计入 `dropped`）"与"占位不产 case"，都不猜。
- **三种结果分开记**：一致（逐字节相等）、`variants`（字节不同但两边喂给**我们自己的解码器**后反汇编文本相同 = 同一指令的另一种合法编码）、`known`（字节不同且不等价）。
- **`known` 挂进棘轮** `asm/ratchet/encoding.txt`（与解析档同一套"两个方向都红"：新增一条红、少一条也红），避免门禁永久红或静默放过。
- 实测（2026-10-03）：riscv64 **12 条逐字节全等**、aarch64 **15 条逐字节全等**、x86 6 条对拍上；x86 的 **5 条 `known`** 是真实缺口——`xor rax, 12` 我们出 imm32 形式（7 字节 `48 81 f0 0c 00 00 00`）而上游/规范是符号扩展 imm8（4 字节 `48 83 f0 0c`），且**我们的解码器连 `48 83 /6 ib` 都解不出**；同族 `xor/or/cmp/add/adc/sbb` 的 **16/32 位目的地**形式（`xor eax, 12`、`add ax, -12`）则完全不解析（`README.md` 的「已知缺口」已逐条登记）。

### Fixed (2026-10-03) — 伪指令校验不再按"首词"猜助记符（与扫描器同口径）

`validate_pseudos`（错误码 `DSL-PSEUDO`）原来把每条指令 `asm` 的**首个空白分隔词**当成助记符集，用它查两件事：伪指令名是否与指令重名、`emit` 行首词是不是一条指令。这正是"首词 = 助记符"那套被废弃的假设，对**操作数前置**的模板（`asm = "{dst} = {src}"`）会把合法的 `emit` 行判成拼错（首词是 `{rd}` 这类占位符）。

- 改为比**前导字面的 token**：用扫描器那份词法（`assembler::tokenize`）取每条模板第一个 `{…}` 之前的前导字面，`emit` 行同样取前导字面，**重叠部分逐 token 相容**（任一方是对方的前缀）即算"接得上"；模板以操作数开头（前导为空）不构成约束。
- 遮蔽判定的口径同步修正：伪指令展开**先于**指令扫描，所以"重名"只发生在**模板前导字面的首个 token** 就是该名字时（多 token 前导如 `lock cmpxchg [` 只认 `lock`）；大小写按 `[meta].mnemonic_case` 走（不敏感档 `MOV` 与 `mov` 撞车）。旧错误消息"汇编器会先匹配到指令，伪指令永不生效"把顺序写反了，一并改对。
- `emit` 行首的伪操作判定改用 `[meta].directive_prefix`（不再硬编码 `.`）。
- 顺带把"按空白切词查助记符"的措辞从 `docs/reference/isa-dsl.md`、`docs/reference/isa-dsl-errors.md` 与 `model.rs` 的 `[[pseudo]]` 文档例里清掉；`docs/archive/**` 与 CHANGELOG 旧条目保持原样。
- 守卫：`dsl/tests.rs::pseudo_checks_do_not_assume_first_word_is_mnemonic`（操作数前置 + 行首是参数的 emit 必须通过；多 token 前导的 `set if` 通过而 `sett` 报错；大小写不敏感/敏感两档的遮蔽判定）。实测 `forge-isa-dsl --lib` 224、三份发行谱 `validate`/`lint` 零结论、其余计数不变。

### Added (2026-10-03) — 真实汇编语料测试台（x86 / riscv64 / arm64）+ 线性扫描探针

拿**上游真实汇编语料**（GNU as / LLVM MC 的测试文件）检验生成物的汇编器：能不能读真实写法、编出的字节对不对、编出来的程序真跑不跑得起来。三档各一个测试二进制，全部在 `crates/tools/forge-tests/`：

- `tests/asm_parse.rs`：逐行喂 `Assembler::parse_insts`，按四桶归因（`Parsed` / `NoPrefix` / `TailMismatch` / `CorpusOnly`），**门禁 = 计数棘轮** `asm/ratchet/<isa>.txt`（逐项相等，多一个少一个都红；重刷 `$env:FORGE_ASM_WRITE_RATCHET=1`）。实测（2026-10-03）x86 `0/18/43/3`、riscv64 `7/14/10/76`、aarch64 `2/0/1/17`，红桶样例写在棘轮文件里。
- `tests/asm_encoding.rs`：把上游注释里的 `# CHECK-ASM: encoding: [0x…]` 抽成 `(指令, 上游字节)` 对拍——**外部编码 oracle**（不依赖我们自建黄金表）。riscv64 语料 31 条期望里 7 条能解析，**逐字节全等**；x86/aarch64 的语料暂未带字节期望（P2 补 NASM/XED、LLVM MC X86/AArch64），该档打印 `ASM-ENCODING-SKIP` 说明原因，不假绿。
- `tests/asm_exec.rs`：`asm/exec/<isa>/*.s` 的**真语法叶函数小程序**汇编 → 装进 `CompiledFunction` → **真跑**（x86 原生、riscv64/aarch64 走 QEMU semihosting），断言返回值；缺通道则 `ASM-EXEC-SKIP` 说明原因。实测 `ran=8 skipped=0`（三架构都真跑通）。
- 新增生成物公开面 **`could_be_instruction(text) -> bool`**（线性扫描探针）：只跑整模板扫描的**第一步**（首段匹配），是必要性判断（`false` ⇒ 一定不是本 ISA 的指令）。之所以要它：桶判定必须区分"上游有我们没实现的指令"与"有指令但语法对不上"，而**生成期切词得到的"助记符表"不可用**——v17 起 `asm` 模板可以操作数前置（首"词"是操作数）、首段可以是多 token 字面（`lock cmpxchg [`）、空白分词与扫描器的 token 切分也不是一回事（`amoadd.w.aqrl` 是一个 Ident）。探针与 `assemble`/`parse_insts` 共用同一份候选、同一个首段匹配器、同一个 lexer。生成器守卫 = `dsl/tests.rs::asm_scan_probe_replaces_mnemonic_table`（钉"有探针、无助记符表"）。
- 语料 **vendor 进仓库**（`asm/parse/**` 逐字保留上游原文），出处/ref/许可/摘要见 `asm/PROVENANCE.md`（LLVM MC = `Apache-2.0 WITH LLVM-exception`，GAS = `GPL-3.0-or-later` 且独立目录）；`asm/README.md` 写清边界（只接 Intel 语法、不吃 C 预处理器的 `riscv-tests` 族、不做随机指令执行 smoke）与已跑出的真实缺口（riscv `inc`/`lwu`/`addiw` 未声明、ABI 别名寄存器不认、`%lo(2048)(x7)`；aarch64 `ret lr`；x86 `disp[base]`/`byte ptr`/无符号 `imm32`/`mov eax, imm32`）。`asm/fetch.ps1` 供有 TLS 的机器补齐全量语料（本机 `curl`/`git` 无 TLS，故 P1 只 vendored 三份小样）。
- **验证**：`forge-isa-dsl` 223（+1 守卫）、`forge-abi` 51、`forge-isa` 33、`forge-isa-runtime` 31、`forge-tests --lib` 44、`isa-host-demo` 9+5、forge-codegen 21 个集成档计数不变（`spec_tests` 36、`demo_tests` 15、`include_tests` 7、`integration_tests` 12、`x86_tests` 5、`riscv64_tests` 7、`arm64_tests` 7、`library_surface` 3 …）；clippy `-D warnings` 0、fmt 0、markdownlint 0。

### Changed (2026-10-03) — 清除 `v12` 历史命名（破坏性，无兼容层）

文件名与标识符里的 `v12` 是 v12–v17 语法时代的残留，读起来像"当前版本是 v12"（实际现行语法
是 v18），且与 `V128`/`MOV12`/AArch64 的 `V12` 寄存器等无关名字容易混。一次清干净：

- **ISA 谱与生成模块**：`isa/{x86,riscv64,arm64}_v12.toml` → `isa/{x86,riscv64,arm64}.toml`
  （生成模块名 = 文件 stem ⇒ `forge_codegen::{x86,riscv64,arm64}`）；`[meta].name` →
  `x86_64`/`riscv64`/`arm64`，**同步** `forge-abi/conventions/*.toml` 的 `isa =` 键与宿主
  注册表/`plan_call` 的 ISA 名键。
- **后端包装**：`src/arch/{x86,riscv64,arm64}_v12.rs` → `arch/*.rs`（`pub use arch::*` 随之）。
- **测试与夹具**：14 个 `tests/*_v12_tests.rs` → 去后缀；7 个 `tests/isa/*_v12.toml` → 去后缀；
  `spec_tests_v12.rs`/`v12_integration_tests.rs`/`include_v12_tests.rs` 等同步；
  `jit_matrix_*_v12` → `jit_matrix_*`（CI 命令同步）。
- **DSL 内部**：`forge-isa-dsl/src/v12/` → `src/dsl/`（`crate::v12::` → `crate::dsl::`）；
  `V12Model` → `IsaModel`、`V12Error` → `DslError`、`V12_LOWERING_OPS` → `LOWERING_OPS`。
- **文档**：现行文档里的路径/命令全量更新；`docs/archive/**` **保持原样**（归档内容以记录
  时点为准，`isa-dsl-v12-*` 文件名就是那段历史的标识）。
- **验证（改名不改行为，逐数字不变）**：`forge-codegen --lib` 1285、`spec_coverage_guard`
  197/117/110、矩阵 x86 **197/3/0**、riscv **136/64/0**、`forge-isa-dsl` 222 + 集成档全绿、
  `forge-abi` 51、`forge-isa` 33、`forge-tests --lib` 44、三谱 `validate`/`lint` 零结论、
  clippy `-D warnings` 0、fmt 0、markdownlint 0。

### Changed (2026-10-03) — 搬运族派生：删掉九个手写角色，ISA 只写 `data_width`

搬运指令的四个事实里，**方向 / 寄存器族 / 立即数还是寄存器**本来就在操作数结构里
（槽角色 `out|inout|in`、槽的 `class`/`classes`、槽的 `kind`），只有**数据宽度**不在。
于是：

- **删除** `gpr_mov` / `gpr_mov_imm` / `ret_mov` / `fpr_mov` / `vec_mov` /
  `fpr_to_gpr_mov` / `gpr_to_fpr_mov` / `wide_vec_load` / `wide_vec_store` 九个角色与
  `RoleDecl.bits`（"角色带宽度"整条机制随之消失）；相应删掉 `FpMovWidths` /
  `BankMovWidths` / `inst_by_role_for` 与三个守卫（`role_widths.rs`、
  `fpr_mov_widths.rs`、`bank_mov_roles.rs`）。
- **新增指令键 `data_width = <位>`**（**数据宽度**，与 `width` = 指令字长是两件事）：
  写了它就是"一条搬这么多位的搬运"。选指令 = 按 **(目的形状 × 来源形状 × 位宽)** 查
  派生表（`v12/codegen/moves.rs::MoveTable`，唯一实现，生成器四处发射点与
  `abi_view`/生成物 `role_bits` 同源）：寄存器/立即数来源取**最窄覆盖者**，涉及内存取
  **精确**宽度；同形状同宽度多条候选 ⇒ **生成期报错并列出候选**；一条都没有 ⇒
  生成物里 fail-closed。三操作数形态（riscv `fsgnj.d rd, rs, rs`）的第三槽自动填成源，
  store（`mem:mem:out`）同样成立。
- **顺带清掉两处硬编码宽度**：by-ref/sret 的宽向量栈拷贝不再按 `wide_vec_load_32/64`
  的固定 256/512 位取指令（改按运行期字节宽查表）；`f32/f64` 的 32/64 不再出现在
  生成器里。
- **实测（逐字节/行为不变）**：x86 矩阵 **197/3/0**、riscv64 矩阵 **136/64/0**（QEMU）、
  `forge-codegen --lib` 1285、`forge-isa-dsl` 全绿（新增守卫 `tests/move_derive.rs` 6 条）、
  `forge-abi` 28 invariants、`forge-isa` 全绿、`forge-tests --lib` 44；clippy `-D warnings`
  0、fmt 0、markdownlint 0。

### Changed (2026-10-01) — lp64d 的变参改成 psABI 定本口径：未命名实参走整数寄存器，保存区紧贴入口 `sp`

RISC-V 上"变参实参只走栈"这条**已知偏差**（与 GCC/Clang 编译的变参函数互调会错）按定本
原文四件一起改完，QEMU 真执行验收：

- **① `variadic_stack_only = false`**：未命名实参走 a0-a7（溢出才上栈）。
- **② `variadic_classify` 按整数约定分类**：定本 `riscv-cc.adoc` 的浮点调用约定一节写着
  *"The remainder of this section applies only to **named** arguments. **Variadic arguments are
  passed according to the integer calling convention.**"* ⇒ 变参的浮点也走整数寄存器
  （`double` 的位模式进 a0-a7）。调用点为此需要**类间位搬移**（上一片已落地的
  `fpr_to_gpr_mov`/`gpr_to_fpr_mov` 角色）——这正是那条通用能力的第一位真实用户。
- **③ 保存区只装整数参数寄存器**：`save = { int_slot = 8, float_slot = 0 }`（定本："integer
  argument registers"；浮点槽会让线性游标在"整数寄存器之后、栈实参之前"读到不该有的槽）。
- **④ 保存区紧贴入口 `sp`、与栈实参连续**：新增形状属性 `VaSaveDecl::contiguous` ⇒ 帧布局把
  保存区放**帧顶** `[入口 sp - save_size, 入口 sp)`，ra/fp、callee-saved 的保存槽（生成物读
  `AllocResult.va_top`）与局部槽（`stack_slot_shift`）整体下移一个 `save_size`；`area` 初值 =
  保存区起点 + **已用整数寄存器数 × 槽宽**，于是单指线性游标先走完寄存器里的变参、再接着走
  栈上的变参。`contiguous = false` 时布局与历史**逐字节一致**（其余三份约定的黄金快照未变）。
- **连带**：`forge-isa abi check` 的"游标就是栈地址却允许未命名实参进寄存器"硬错**加了例外**
  （`contiguous` 时不再矛盾），变参状态行也如实描述为"保存区（与栈实参连续）+ 线性游标"；
  `lp64d.plan.txt` 黄金重刷（`arg` 落点从栈变寄存器、`va_save` 128→64、新增
  `va_save_contiguous true`）。
- **实测**：riscv 矩阵 **136/64/0**（`variadic_va_arg_int_only` = 47、`variadic_va_arg_int_and_float`
  = 49 真跑，走的是新的寄存器路径）、x86 矩阵 **197/3/0**、`forge-abi` 28 invariants 全绿、
  `forge-codegen --lib` 1285、`forge-isa-dsl` 全绿、`forge-isa` 全绿、`forge-tests --lib` 44；
  `clippy -D warnings` 0、`fmt --check` 0、markdownlint 0。
- **踩到并修掉的一个真 bug**：保存区基址的 `StackAddr` 立即数第一版写成 `shift - save_size`，而
  调用方已经把 `stack_slot_shift` **加上**了 `va_top` ⇒ 地址少了 `2×save_size`；QEMU 用例当场
  读到 4 而不是 47（失败信息直接指出了症状）。修成 `shift`（`fp + shift - (shift + save_size)`）
  后两条用例全绿——这也是"每片都用真执行验收"的价值。

### Added (2026-10-01) — 变参数据面两件：`variadic_classify`（未命名实参专属分类）与"保存区只收一个类"

为 lp64d 的变参修正铺数据面（发射侧的两块能力已就位：fp 位宽表、类间位搬移）。两件都**只加
能力、不改现有行为**（四份内置约定都没用它们，黄金与矩阵逐字节不变）：

- **`AbiRules::variadic_classify`**（新规则键）：**未命名（变参）实参**专属分类，空 = 与命名
  实参同一套。psABI 允许"变参按另一套约定传"——RISC-V 的 `riscv-cc.adoc` 在浮点调用约定一节
  写着 *"The remainder of this section applies only to named arguments. **Variadic arguments are
  passed according to the integer calling convention.**"* ⇒ LP64D 上变参的浮点也走整数寄存器。
  用一张分类表表达，而不是在引擎里给某个 ISA 开洞（与既有的 `ret_classify` 同一形状；
  `merge_parent`/`validate` 一并接上）。
- **保存区槽宽 `0` = 该类不进保存区**（`VaSaveDecl`）：`save = { int_slot = 8, float_slot = 0 }`
  ⇒ 保存区只装整数参数寄存器。定本要求保存区装的是 *"**integer argument registers** not used
  for named arguments"*——浮点池占槽会让线性游标在"整数寄存器之后、栈实参之前"读到一批本不该
  存在的槽。
- 守卫：`invariants::variadic_classify_switches_only_the_unnamed_arguments`（命名实参仍走
  `classify`、未命名走专属表；去掉表 ⇒ 回到普通分类——证明变化来自数据）、
  `invariants::zero_slot_width_keeps_a_class_out_of_the_save_area`（槽表与 size 逐格核对，
  并对照 `float_slot = 16` 的旧行为）。
- 验证：`forge-abi` 27 invariants（+2）全绿、黄金快照未变；x86 矩阵 197/3/0、riscv 矩阵
  136/64/0、`forge-codegen --lib` 1285、`forge-isa-dsl` 全绿、`forge-isa` 全绿；
  `clippy -D warnings` 0、`fmt --check` 0、markdownlint 0。
- **仍未做**（文献与顺序见 `docs/plans/varargs-plan.md` §5）：保存区放**帧顶**（与调用方栈实参
  连续，需新增一条"要求连续"的形状数据属性 + 帧布局整体下移一个 save_size）、`area` 初值改成
  "保存区起点 + 已用整数寄存器数 × 槽宽"，然后才能翻 `lp64d.variadic_stack_only = false`。

### Added (2026-10-01) — 类间位搬移角色：浮点值落在整数寄存器时不再**静默错值**

值的寄存器类与 ABI 落点的类**不同**时，旧实现只看**落点的类**决定用哪条搬移指令：浮点值落在
整数寄存器（`classify` 把浮点判给 `int` 池）时会走 `gpr_mov`，把 FPR 的号当 GPR 号用
（`Reg::from_index(10, GPR)` = `x10`）——**静默错值**。这个形状不是假想的：psABI 的**整数约定
收浮点**就是它（RISC-V 变参一律按整数约定传、Zfinx/软浮点约定），而且 lp64d 的变参修正正要
走到这里。

- **两个新角色**（宽度写在声明里，任意 N ≥ 1，与 `fpr_mov` 同一套位宽表机制）：
  `fpr_to_gpr_mov`（FPR 的位模式 → GPR）、`gpr_to_fpr_mov`（反向）。方向在角色名里，
  字段名与 Reg 槽下标按**槽的寄存器类**取（不按操作数位置猜）。
- **发射侧四路分派**：调用点（`arg_move_loop`）与被调方（`frame.rs` 的收参）都按
  **落点的类 × 值所在的池**分派；不一致时走类间位搬移，**缺该角色 ⇒ 生成物里明确
  `Unsupported`**（不退化、不猜）。被调方与调用点的方向**相反**（数据流向相反）。
- **谱面申报**（指令本来就有，只加角色）：riscv `FMV_X_W`/`FMV_X_D`（f→x 32/64）、
  `FMV_W_X`/`FMV_D_X`（x→f 32/64）；x86 `MOVD_IREG_FREG`/`MOVD_FREG_IREG`（32）、
  `MOVQ_R64_XMM`/`MOVQ_XMM_R64`（64）。arm64 暂不申报（AAPCS64 不需要，缺了会 fail-closed）。
- **验证**：新增 `forge-codegen/tests/bank_mov.rs` 用一个**合成约定**（"浮点判给 int 池"）
  走真实管线：被调方收到 `fmv.d.x`、调用点发出 `fmv.x.d`，而正常分类（lp64d）仍走同类的
  `fsgnj.d`（两路不串）；`forge-isa-dsl/tests/bank_mov_roles.rs` 钉住"发行谱申报了四个档"与
  "把角色删掉后生成物里出现 fail-closed 分支"。回归：x86 矩阵 197/3/0、riscv 136/64/0、
  `forge-codegen --lib` 1285、`forge-isa-dsl` 全绿、`clippy -D warnings` 0、`fmt --check` 0。

### Fixed (2026-10-01) — lp64d 的 `va_list` 对象布局对齐定本（`sizeof(va_list)` = 指针宽度），并纠正 §5 的修复配方

按 RISC-V psABI 定本原文重核变参一节时，发现**数据**与**下一步的做法**各错一处：

- **数据**：`riscv_save_area` 的 `size` 由 **24 改成 8**。定本 `riscv-cc.adoc`（issue #412 之后的
  "Expand va_list description" 版本）第一句就是 *"The `va_list` type has the same representation as
  `void*`"* ⇒ LP64D 上 `sizeof(va_list)` = 8；原来的 24 是从 sysv64 抄来的余量（那 16 字节既没有
  字段也没有读者），它让 `va_copy`／把 `va_list` 交给 `vprintf` 一类**跨编译器**用法在对象布局上
  就对不上。黄金快照 `lp64d.plan.txt` 随之重刷（只动 `va_area … size=` 两行），§2 文档表同步，
  `invariants::va_shapes_match_the_documented_table`（逐格比对）与
  `va_object_layout_matches_the_psabi_numbers`（字段布局）继续钉住。
- **做法**：`docs/plans/varargs-plan.md` §5 与 `forge-abi/src/builtin.rs` 的注释此前写着"把 lp64d
  换成 `gp_offset`/`fp_offset`/`reg_save_area`/`stack_arg_area` 四字段 + 两个游标"——那是
  **SysV AMD64 的 `va_list`**；RISC-V 的 `va_list` 就是一个 `void*`（单线性游标）。同一处还写着
  "调用方按正常分类传未命名实参"——定本的浮点调用约定一节写着 *"The remainder of this section
  applies only to **named** arguments. **Variadic arguments are passed according to the integer
  calling convention.**"* ⇒ LP64D 上**变参的浮点也走整数寄存器**（`double` 的位模式进 a0-a7），
  "正常分类"会把 `f64` 放进 `fa0`，是另一个错。两处已按定本原文改正，并把完整修复写成
  **四件一起**（`stack_only = false` / 保存区放帧顶与栈实参连续 / `area` = 保存区起点 +
  已用整数寄存器数 × 槽宽 / 变参按整数约定分类），其中第 ④ 件在发射侧还缺一块**通用**能力：
  **类间位搬移**（FPR 的位模式搬进 GPR：riscv `fmv.x.d`、x86 `MOVQ`、arm64 `FMOV`）——角色系统
  现在只有同类内的 `gpr_mov`/`fpr_mov`。
- 守卫：`invariants::lp64d_variadic_stack_only_is_a_documented_deviation` 增加**正向**断言
  （`size`/`align` = 8、字段只有一个 = `void*`），偏差部分继续钉住并写清四件一起的条件。
- 验证：`forge-abi` 全绿（黄金重刷后）、riscv 矩阵 **136/64/0**（未受影响）、`abi_target_real` 25、
  `forge-isa` 全绿（`abi check` 仍报 lp64d 的"可疑组合"，对象尺寸已显示 8 字节）。
- **仍未做**（本条的真正剩余）：把 lp64d 的变参切到寄存器路径——见上面"四件一起"，其中
  类间位搬移是一块独立的通用能力。

### Changed (2026-10-01) — 浮点搬移改成**按位宽查表**：不再写死 32/64，riscv 的浮点参数/返回真跑起来

上一片留下的"钥匙"（`fpr_mov` 的三操作数形态）这一片装上了，而且**没有按 ISA 打补丁**：

- **生成器：位宽表取代硬编码两档**。`roles = [{ role = "fpr_mov", bits = N }]`（**任意 N ≥ 1**）
  由 ISA 申报，生成器把所有声明收成一张表（`FpMovWidths`），发射点只说"这次要搬多少位"，
  由表生成 N 路分派；表里没有该位宽 ⇒ 生成物里明确 `Unsupported` 并**列出已声明档**（不猜、不回退）。
  三操作数搬移（riscv `fsgnj.d rd, rs, rs`）由表里的第三槽自动填源。删掉了
  `role_name_for(role, bits)`——它的调用点必须**写死一个宽度常量**，正是这个问题的来源。
  守卫：`crates/frontend/forge-isa-dsl/tests/fpr_mov_widths.rs`（把 x86 的两档改成 16/128 后，
  生成物里只该出现这两档、写死的 32/64 一个不留）。
- **两处"绑定无关能力"的真开洞**（都是通用性缺陷，不是某个 ISA 的洞）：
  ① 调用点浮点实参搬运要求 `fpr_mov && vec_mov` ⇒ 没有向量寄存器的 ISA 连 `f64` 实参都发不出去；
  现在两条能力**互不依赖**，只有"按值向量"那条路会在运行期按 IR 类型 fail-closed。
  ② riscv 谱里没有 `FSGNJ_D`（只有单精度 `fsgnj.s`）⇒ f64 的搬移无处申报；已补为同一 `fpr_mov`
  的 64 位档。
- **riscv 谱补浮点算术**：`Fadd`/`Fsub`/`Fmul`/`Fdiv`（单/双精度各一条，谱里 `FADD_S/D` 等指令早就有）。
  此前这些 op 全是**真缺口**，矩阵里所有浮点算术用例被整条 Skip 掩盖。
- **测试通道补浮点参数执行**：`jit_matrix` 此前对 `CaseKind::F64Args` + 注入执行器直接报
  "not supported"；现在 `Executor::exec_f64_args` + riscv crt0 的 `li a0; fmv.d.x fa{i}, a0`
  （f32 用 `fmv.w.x`）让 f64/f32 参数真进 `fa0..fa3`，并新增独立冒烟用例
  `qemu_exec_f64_arg`（42.0 → 42、7.0f32 → 7）钉住这条通道。
- **实测**：riscv 矩阵 **136 passed / 64 skipped / 0 failed**（原 133/67/0；
  `float_args_two`、`float_args_four_xmm3`、`call_float_roundtrip` 从 Skip 转**真跑**），
  x86 **197/3/0**；`forge-codegen --lib` 1285、`abi_target_real` 25（
  `riscv64_float_gap_is_engine_ok_but_emission_closed` 改名
  `riscv64_float_arg_is_open_on_both_engine_and_emission`：从"钉差异"改成"钉已闭合"）、
  `forge-isa-dsl` 全绿、`forge-isa` 24、`forge-abi` 全绿；`clippy -D warnings` 0、`fmt --check` 0。
- 快照同步（riscv 谱的预期变更）：指令数 116 → 117、枚举器条目 327 → 329、RV32 投影指令
  104 → 105、lowering（RV32 视角）100 → 108 与默认视角 114 → 122、能力覆盖 63 → 67
  （真缺口 40 → 36）、vary 建议 24 → 28、部分重叠 23 → 27。
- 仍未做：`lp64d` 的 `va_list` 形状切换（`stack_only = false` + 四字段 + 两条游标规则）、
  `va_meta`（SysV `%al`）、前端产出 `variadic`。

### Fixed (2026-10-01) — aapcs64 的变参数据修正：未命名实参**走寄存器**（`variadic_stack_only` → false）

上一片的"可疑组合"告警立刻见效：它把 **aapcs64** 也点了出来——`aapcs64` 与 lp64d 是**同一个模式**
（声明了寄存器保存区，却把未命名实参设成只走栈）。这一片修正它：

- **判据（不需要定本原文）**：AAPCS64 的 `va_list` 是 `{__stack, __gr_top, __vr_top, __gr_offs,
  __vr_offs}` 的**计数式**结构——被调方把自己用过的通用/向量寄存器存进保存区，`va_arg` 按
  `__gr_offs`/`__vr_offs`（从负值数到 0）从保存区取，数到 0 再落到 `__stack`。这套结构只在
  "未命名实参**确实进寄存器**"时才讲得通；"只走栈"与保存区语义自相矛盾。
- 改动 = **纯数据**（`builtin.rs` 一行 + 注释），黄金快照按 `FORGE_ABI_BLESS=1` 重刷；文档表
  `varargs-plan.md` §2 的 aapcs64 行从"只走栈"改为"继续用寄存器"（守卫
  `va_shapes_match_the_documented_table` 正是先报出这处不一致）。
- **验证口径如实说明**：本机**没有 arm64 执行通道**，这次只在**计划面**验证——
  `forge-abi` 25 条 invariants（含文档表守卫）、`abi_target_real` 25（含
  `aapcs64_variadic_shape_is_pure_data`，其断言同步改成"未命名实参走寄存器"）、`forge-isa` 24、
  `forge-isa-dsl` 全绿；x86 矩阵 197/3/0、riscv 矩阵 133/67/0（未受影响）；`clippy -D warnings` 0、
  `fmt --check` 0。**这不是执行验证**，写进方案 §5。
- 收益：`abi check` 的"可疑组合"告警此后只剩 lp64d 一处（它的修正需要先把生成器的三操作数 fp
  搬移泛化，见下一条），aapcs64 的数据先与 psABI 模型对齐。

### Added (2026-10-01) — `abi check` 报出"可疑组合"：声明了保存区却让未命名实参只走栈（数据驱动的一致性检查）

lp64d 的 psABI 偏差（未命名实参应走寄存器）此前只写在文档与守卫里，用户在 `forge-isa abi check`
里看不到。这一片把它变成**工具可见的一致性检查**——而且**不点名任何约定**，按形状数据判：

- 判据：`va_list` 形状**声明了寄存器保存区**（`save.is_some()`）却把 `variadic_stack_only` 设成
  **true**。保存区的存在本身就意味着"未命名实参可以进寄存器、由被调方存下来"（AAPCS64 的
  `__gr_offs`/`__vr_offs`、RISC-V 的 vararg save area 都是这个模型）⇒ 这个组合**自洽但可能不符合
  psABI 定本**：自己的调用方/被调方按同一份约定对齐能跑，与外部编译器互调会错。
- 落点：`ℹ 变参 …` 那一行追加 `⚠ …` 提示（**不改退出码**——它是"待核"而不是"这台机器做不了"；
  `--strict` 目前不覆盖它，如实写在消息里）。
- 收益：`abi check riscv64.toml` 现在会提示 lp64d 的组合可疑；**aapcs64 同组合也一并被点名**
  （此前没人注意它和 lp64d 是同一个模式）。
- 守卫：`cli_tests::abi_check_reports_the_variadic_state` 增加断言（riscv 输出必须含
  "可能不符合 psABI 定本"）。
- 验证：`forge-isa` 9 + 24 全绿；`clippy -D warnings` 0、`fmt --check` 0。

### Changed (2026-10-01) — 补强证据：riscv 缺的 `fpr_mov` 是"浮点全面落地"的同一把钥匙（实验后回退）

接着上一片的 lp64d psABI 议题往下验证：把 riscv 谱里**早就有指令、却一直没有 lowering** 的三条
浮点算术（`Fadd`/`Fsub`/`Fmul`，各单/双精度两条）补上，看矩阵有什么变化——结果暴露了**同一个**
根因：

- 补上浮点算术后，原先被"缺 `Fadd`"掩盖的三条浮点用例立刻从 **Skip 变成 Fail**，且原因全指向
  `fpr_mov`：`float_args_two` / `float_args_four_xmm3`（`v12 float args (MOVSD/MOVSS missing)`）、
  `call_float_roundtrip`（`v12 float return: 未声明 roles = ["fpr_mov_f64"]/["fpr_mov_f32"]`）。
- 也就是说 **`fpr_mov`（含 RISC-V 的三操作数 `fsgnj.d rd, rs, rs` 形态）是 riscv 浮点落地的
  同一把钥匙**：补它一次 → 同时解掉① 浮点变参（lp64d 与 psABI 一致的前提）、② 浮点参数/返回、
  ③ 上面三条用例的真跑。而"只补浮点算术"会把 Skip 变成 Fail（更差），所以这次实验**已回退**
  （riscv 矩阵仍是 **133/67/0**），把结论与顺序写进 `docs/plans/varargs-plan.md` §5。
- 验证（回退后）：riscv 矩阵 133/67/0、x86 197/3/0、`forge-abi` 全绿、markdownlint 0。

### Changed (2026-10-01) — lp64d 的 psABI 修正**试做后按实测回退**：整数支能过，浮点支卡在"三操作数 fp 搬移"

上一片核出 lp64d 与 RISC-V psABI 定本的偏差（未命名实参应走寄存器）。这一片动手改，并用 QEMU 矩阵
逐步验证，结论是**必须按顺序做、现在停在第 ② 步之前**：

- **试做的改法**（**不需要**动帧布局）：把 lp64d 换成"两个游标（`gp_offset`/`fp_offset`）+ 保存区
  指针 + 栈实参指针"的形状 + `variadic_stack_only = false`。理由是定本要求"保存区与栈实参连续"
  是为了让**单个线性游标**能从保存区走进栈实参；我们的 `va_list` 自带两个指针，因此不必连续。
- **实测 ①（整数支）**：`variadic_va_arg_int_only` 在 QEMU 上**通过**——未命名实参进 `a1`/`a2`，
  被调方 spill 进保存区、`va_arg` 按 `gp_offset` 读回 ⇒ 数据驱动的模型**能**表达定本语义。
- **实测 ②（浮点支）**：`variadic_va_arg_int_and_float` **编译失败**——
  `v12 call: 浮点/向量实参搬运缺 MOVSS/MOVSD/MOVAPS 角色`：riscv 谱里没有 f64 的 fp→fp 搬移
  （只有 `FSGNJ_S`，没有 `FSGNJ_D`），且规范化的写法是三操作数 `fsgnj.d rd, rs, rs`；生成器的
  `fpr_mov` 路径假设两操作数（8 处发射点都在构造 `Inst::<mov>{ dst, src }`）⇒ 直接给三操作数
  指令申报角色**连生成物都编不过**。
- **因此回退**（保持绿）：只做形状切换会让 riscv 上任何"传浮点变参"的调用点从"自洽能跑"退化成
  **编译错误**，比现状更糟。修复顺序写进方案：① 补 `FSGNJ_D` + 角色；② 生成器的 `fpr_mov`
  发射**按形状**泛化（三操作数时 `src2` 也填源寄存器，与 `StackMemShape` 同一套做法）；
  ③ 再改 lp64d 的形状。
- **验证**：回退后 `forge-abi` 7 个 target 全绿（25 条 invariants，含钉住偏差的那条）、
  `clippy -D warnings` 0、`fmt --check` 0、markdownlint 0；riscv 矩阵仍是 **133/67/0**。

### Fixed (2026-10-01) — riscv 浮点支打通：`Fload`/`Fstore` lowering + `FSD` 的浮点栈参数角色（lp64d 变参两条用例都真跑）

上一片让 lp64d 的**整数**变参在 QEMU 上真跑；这一片把**浮点**支也接通——发现两处与变参无关的
谱面缺口：

- **riscv 谱有 `FLD`/`FSD` 指令，却没有任何 `Fload`/`Fstore` 的 `[[lowering]]`** ⇒ `va_arg(f64)`
  展开出的 `Fload` 编不出来（矩阵报 `Unsupported("v12 lowering")`）。补上单/双精度各一条
  （`FLW`/`FLD`、`FSW`/`FSD`）。
- **浮点栈实参写不出去**：调用方写传出区用 `stack_arg_store`，此前只给整数 `SD` 申报了角色；
  给 `FSD` 补 `{ role = "stack_arg_store", class = "fpr" }`（S 形式，生成器按形状填 sp + 偏移）。
- **矩阵用例的 `ops` 多写了一个 `Fadd`**（用例本身不做浮点算术：只在调用点传 `2.5`、被调方
  `va_arg(f64)` 后 `fptosi`）——riscv 谱没有 `Fadd`，这条**用不到的**能力把用例误判成"能力不足"
  而 Skip；删掉它，用例两台机器都真跑。
- **验收**：riscv QEMU 矩阵 **133/67/0**（`variadic_va_arg_int_only` = 47、
  `variadic_va_arg_int_and_float` = 49，**两条都真跑、无 variadic Skip**），x86 **197/3/0**。
- **同步的快照**（补 lowering 改变了"能力缺口"口径，按纪律人工复核后同步）：
  `lint_shipped::op_gap_matches_section_10_3` riscv 覆盖 61 → 63、真缺口 42 → 40；
  `lint_shipped::vary_candidate_inventory` riscv 22 → 24（新增的两条同形状规则是"可合并"建议，
  不强行 `vary`）。
- **验证**：`forge-isa-dsl` 全绿（222 + 各守卫）、`forge-tests` lib 43、矩阵两套、`forge-codegen`
  lib 1362 + `riscv64_tests` 7 + `riscv64_tm_tests` 5 + `integration_tests` 12、
  `forge-abi`、`forge-ir` 281、`forge-isa` 24；`clippy -D warnings` 0、`fmt --check` 0。

### Changed (2026-10-01) — aapcs64 的缺口量到位：arm64 后端**只有 8 个 op 的 lowering**（守卫钉住覆盖度）

上一片说"aapcs64 被 arm64 缺的 lowering 挡住"，这一片把"缺多少"量清楚并钉住——结论是**这不是
变参的问题，是 arm64 后端还没成型**：

- 实测：arm64 谱的 `[[lowering]]` 只覆盖 **8 个 op**（`Band`/`Bor`/`Bxor`/`Copy`/`Iadd`/`Iconst`/
  `Imul`/`Isub`，全是算术）；变参展开要用的 `StackAddr`/`Store`/`Load`/`Icmp`/`Select`/`Sextend`/
  `Ireduce` **一个都没有**（x86/riscv 有 ⇒ win64/sysv64/lp64d 真跑）。
- 守卫 `arm64_tm_tests::tm_aapcs64_varargs_are_blocked_by_missing_arm64_lowering` 现在**两件事一起钉**：
  ① 变参函数编到 arm64 必须是**明确 Unsupported**（不许静默编错）；② `arm64::SUPPORTED_OPS`
  里**没有**这些 op——arm64 补上任一条时守卫会红，提醒把 aapcs64 从"数据面"升级成"编得出 + 真跑"。
- 方案文档（`docs/plans/varargs-plan.md` §5）据此把 aapcs64 一条改成"先补 arm64 后端（独立且大得多的
  工作）+ 本机无 arm64 执行通道"，不再把它算作变参侧的缺口。
- 验证：`arm64_tm_tests` 7、`forge-codegen` lib 1362；`clippy -D warnings` 0、`fmt --check` 0。

### Fixed (2026-10-01) — riscv 栈参数打通：访存指令按**形状**取 + 修掉 `lp64d` 的 `first_offset_slots`（变参在 QEMU 上真跑）

上一片把 lp64d 的缺口钉在"调用方写不出传出区"。这一片补齐，顺带挖出一个与变参无关的真 bug：

- **栈参数访存指令按形状取，不按 ISA 取**（新 `StackMemShape`，生成器 + `move_args` 共用一份
  `stack_mem_shape` 判据）：`Reg+Mem`（x86 `MOV64_MR`/`MOV64_RM`，Mem 槽填
  `MemRef{base, disp}`）或 **`值Reg+基址Reg+位移Imm`**（RISC-V S 形式 `SD {src}, {imm}({src2})`——
  没有 Mem 槽的定宽 ISA 用这种，基址填机器的帧/栈基址、位移填偏移）。riscv 的 `SD` 因此可以申报
  `roles = ["stack_arg_store"]`；`move_args` 的保存区 spill / 收参路径一并按形状发。
- **真 bug（被这条新路径暴露）**：`lp64d` 的 `first_offset_slots` 写的是 **2**（照搬 x86 的"序言
  总是 push fp"），而 RISC-V 的帧基址 `X8` = **入口 sp**（不 push 返回地址）⇒ 被调方的 `va_list`
  比调用方写的槽**高 16 字节**。实测：QEMU 通道的 `variadic_va_arg_int_only` 读到 **0**（应 47）；
  改成 **0** 后对齐（`lp64d.plan.txt` 黄金快照 `first_arg_off` 16 → 0，`FORGE_ABI_BLESS=1` 重刷）。
  **影响面**：riscv 上任何栈实参（第 9+ 个命名实参、变参未命名实参）此前都按错位置取——没有用例
  走到过。
- **矩阵**：`variadic_va_arg_int_only` 现在是**两台机器真跑**（不再需要伪能力门控 `va_stack_args`，
  该门控与其注释一并删除）：x86 **197/3/0**、riscv **132/68/0**（浮点那条因 riscv 谱缺 `Fadd`
  仍按能力门控 Skip）。
- **验证**：`forge-isa-dsl` 全绿（222 + 各守卫）、`forge-codegen` lib 1362 + `abi_target_real` 25 +
  `arm64_tm_tests` 7 + `riscv64_tm_tests` 5、`forge-tests` lib 43、`forge-abi`、
  `forge-ir` 281、`forge-isa` 全绿；`clippy -D warnings` 0、`fmt --check` 0。

### Added (2026-10-01) — 变参：aapcs64/lp64d 的两条**确定性守卫** + 缺口精确到"差什么、怎么接"

V6 之后四份约定的形状/规则都是数据，但 **aapcs64/lp64d 还没有真跑**。这一片把"差什么"实测清楚并
用守卫钉住，避免后来的人以为它们已经能用：

- **lp64d 的被调方编得出**（新守卫 `riscv64_tm_tests::tm_compiles_riscv_varargs_callee_side`）：
  `va_start` + 两次 `va_arg(i64)` 在真 riscv 后端上走完 lowering → regalloc → frame → encode，
  且帧 ≥ 保存区 128 字节。⇒ **riscv 只差调用方一侧**。
- **lp64d 的调用方接不上（缺口修好前不许静默）**：写传出区要 `stack_arg_store` 角色，而生成器要求
  它是 **Reg+Mem 形状**；riscv 谱的 `SD` 是 `base+disp` 模板形状、`stack_arg_load`/`stack_arg_store`
  一个都没有 ⇒ 实测 `v12 call: 本 ISA 缺 roles = ["stack_arg_store"] 的指令（栈参数写不出去）`。
  **与变参无关**（riscv 上第 9+ 个命名栈实参同样走不到）；矩阵按伪能力 `va_stack_args` 门控 Skip。
- **aapcs64 的数据面逐格钉住**（新守卫 `abi_target_real::aapcs64_variadic_shape_is_pure_data`）：
  5 个字段（`__stack`/`__gr_top`/`__vr_top`/`__gr_offs`/`__vr_offs`）、取参规则（**有符号**计数、
  基准 = 区域顶端、溢出 = `__stack`、步长 8/16）、逐字段初值（`FrameOff(16)` / `SaveOff(64)` /
  `SaveOff(192)` / `Imm(−8)` / `Imm(0)`）全部逐格断言。
- **aapcs64 的编译面被 arm64 缺的 lowering 挡住**（新守卫
  `arm64_tm_tests::tm_aapcs64_varargs_are_blocked_by_missing_arm64_lowering`）：实测报
  `Unsupported("v12 lowering")`——变参展开用的 IR 词汇（`Icmp`/`Select`/`StackAddr`/`Sextend`/
  `Ireduce`）在 arm64 谱里一条 lowering 都没有（x86/riscv 有）。守卫钉"必须是明确 Unsupported"，
  补齐时会红，提醒改成"编得出 + 真跑"。
- **验证**：`forge-codegen` lib 1362、`abi_target_real` 25、`arm64_tm_tests` 7、
  `riscv64_tm_tests` 5、矩阵 x86 197/3/0 与 riscv 131/69/0、`forge-tests` lib 43；
  `clippy -D warnings` 0、`fmt --check` 0、markdownlint 0。

### Changed (2026-10-01) — 变参 V6：`va_list` 形状与取参规则做成**约定数据**，管线只剩一套算法（破坏性）

前一片之后管线里还剩两处"为某个 ABI 开洞"：`va_expand` 按 `(有没有保存区, 字段数)` 判"族"
（4 字段 = SysV 一族、1 字段 = win64 一族），宿主 `abi_target` 按**字段名**（`fields[0].name ==
"cursor"`）认形态。这一片把形状整体数据化，两处判定一起消失：

- **`VaListKind` 枚举删除**（破坏性）：`hidden.va_list` 现在是**形状数据**——预置名
  （`"win64_stack"`/`"sysv_reg_save"`/`"aapcs64_struct"`/`"riscv_save_area"`）**或**一张显式形状表
  `{ size, align, fields, save, int_arg, float_arg }`。加约定/加机器 = 加数据，不改代码；
  `va_list_size`/`va_list_align` 键并入形状（写了会报未知键）。引擎侧 `va_area_from_shape` 只做
  解析：字段**名**→下标、上限/步长/零点由保存区槽表推（显式键优先），形状写错就明确报错。
- **取参规则 = 形状的一部分**（`int_arg`/`float_arg`）：`cursor`（游标字段）/`base`（偏移基准，
  缺省 = 游标即地址）/`overflow`（溢出区，缺省 = 没有溢出支）/`signed_limit`/`limit`/`step`/
  `overflow_step`。解析结果进 `AbiPlan::va_area.arg_rules`，黄金快照新增 `va_rule` 行钉住
  （四份内置约定：SysV = 无符号偏移游标 + 溢出区、Win64 = 地址式单游标、AAPCS64 = 有符号计数
  （基准 = 区域顶端）+ 溢出区、LP64D = 地址式游标 + 保存区）。
- **管线只剩一套算法**（`pipeline/va_expand.rs`）：读游标 →（有基址就加基址）→（有溢出区就按
  上限 select、两条游标各自推进；没有就是直线）→ 取值（`f32` 先取 `f64` 再 `Fptrunc`）→ 写回。
  "族"的判定与按字段名认形态的代码都删了。
- **生成器的变参臂删除**：`gen_va_start_lowering`/`gen_va_arg_lowering` 不再进生成物；
  谱里也不再申报 `ptr_load`/`ptr_store`/`add_imm`/`fpr_narrow` 四个角色（`Role` 变体一并删）。
  变参对 ISA 的**要求降到零**（只用既有 IR op 的降级）——win64 因此也走 IR 展开，三条原本跑
  生成器臂的用例（含 `f32` 默认提升）在无生成器臂的情况下继续真跑绿。
- **`abi check` 的变参一行改成按形状数据说**（形状名 + 是"栈式游标"还是"保存区 + 溢出区" +
  字段表），"自相矛盾"硬错的判据也数据化（游标即地址 ⇒ 必须 `variadic_stack_only`）。
- **矩阵：`VaStart`/`VaArg` 升为"宿主管线 op"**（`Capabilities::HOST_PIPELINE_OPS`，每台机器无条件
  支持）——不再由各 runner 的 `CAPS_EXTRA` 逐台声明（那等于把"变参能不能跑"绑回 ISA）。新增
  架构无关用例 `variadic_va_arg_int_only`（`4*10+7 = 47`）：x86 真跑（矩阵 **197/3/0**），
  riscv 因**调用方一侧**的已知缺口 Skip——实测报 `v12 call: 本 ISA 缺 roles =
  ["stack_arg_store"] 的指令`：生成器要求该角色是 **Reg+Mem 形状**，而 riscv 谱的 `SD` 是
  `base+disp` 模板形状（与变参无关的通用缺口：riscv 上第 9+ 个命名栈实参同样走不到）。门控用
  伪能力 `va_stack_args`，缺口与证据记进方案。
- **验证**：`forge-codegen` 1362（8 条变参 JIT 全绿）+ `abi_target_real` 24、`forge-tests`
  矩阵 x86 **197/3/0** 与 riscv **131/69/0**（新用例的门控 Skip 可见）、lib 43、`forge-abi`、
  `forge-ir` 281、`forge-isa-dsl` 222、`forge-isa`（含 `abi check` 端到端）全绿；
  `clippy --workspace --exclude forge-rustc --all-targets --all-features -j 1 -- -D warnings` 0、
  `cargo fmt --all -- --check` 0、markdownlint 0。黄金快照按 `FORGE_ABI_BLESS=1` 重刷
  （`va_area` 行记形状名 + 新增 `va_rule` 行）。

### Changed (2026-10-01) — 前端局部槽深度扫描抽成唯一实现（`pipeline/frame_slots.rs`）

变参的 ABI 槽（寄存器保存区 + `va_list` 对象）必须排在前端局部槽**之下**，而"前端局部槽占多深"
在两个地方需要同一个答案：`pipeline/lowering.rs`（给 `Alloca` 分槽、算帧尺寸）与
`pipeline/va_expand.rs`（展开前算 ABI 槽起点）。上一片在展开侧照着 lowering 的算式**镜像了一份**
——两份实现要对齐就等于迟早漂移，且漂移的后果是静默错值（保存区被局部变量覆盖）。这一片抽掉镜像：

- **唯一实现** `pipeline/frame_slots.rs::scan_frontend_slots(func, slot_bytes, size_of)`：一次扫完
  三处统计（`StackAddr` 的立即数偏移、`stack_addr(0)+iadd(iconst(-N))` 模式、`Alloca` 区），
  返回 `FrontendSlots { stackaddr_depth, allocas }`；`depth_bytes(slot_bytes)` 给出"这些槽覆盖到的最深
  字节"。`lowering` 与 `va_expand` 都调它，`size_of`（类型尺寸来源）由调用方给——lowering 用类型快照、
  展开用函数自己的 `types`。
- **行为不变（逐处核对）**：立即数那部分 lowering 仍在逐指令通路里并进 `max_stack_bytes`（与预扫描
  结果取 max，幂等）；`Iadd` 模式那部分只有预扫描能看到，继续在这里并进去；`Alloca` 区仍按"StackAddr
  区底 + 一槽"往下排，槽偏移的分配留在 lowering（展开只需要深度，不需要具体槽位）。
- **顺手删掉死代码**：`FrontendSlots::alloca_base` 抽出后无人调用（lowering 自己算 `slot` 起点）⇒ 删。
- 验证：`forge-codegen`（1362，含 8 条变参用例 + `va_expand::tests::abi_slots_sit_below_frontend_locals`）、
  `forge-tests`（矩阵 196/3/0，lib 43）、`forge-abi`、`forge-ir`（281）、`forge-isa-dsl` 全绿；
  `clippy --workspace --exclude forge-rustc --all-targets --all-features -j 1 -- -D warnings` 0、
  `cargo fmt --all -- --check` 0。

### Changed (2026-10-01) — 变参：`va_start` 物化也搬进 IR 展开，拆掉两处 x86 专属（打包 + `gpr_imm`）

评审指出上一片"为某一个开洞"：保存区物化留在生成器里，于是有两处只为 x86 成立的东西——
① `VaInit.offsets` 把 `gp_offset|fp_offset` **打包**成一个 u64（因为生成器只有 8 字节的帧相对
store），② 为写常量而新增的角色 `gpr_imm`。这一片把物化也搬进 IR 展开，两者都消失：

- **对象初值改成"逐字段的数据"**：`VaInit.fields: Vec<VaInitVal>`（与 `va.fields` 同序），值有三种
  来源——`Imm`（常量，按**字段自己的宽度**截断后写）/ `FrameOff`（帧内地址）/ `SaveOff`（保存区
  基址）。IR 展开按 `va.fields` 的偏移与宽度逐字段 `Store` ⇒ **不再需要打包、也不需要立即数
  能力**（常量用 `Iconst`、地址用现成的 `StackAddr`）；`gpr_imm` 角色与 `MOV_REG_IMM64` 上的声明
  一并删除。
- **修掉一个真缺陷（同一片里发现的）**：ABI 槽（保存区 + 对象）原先在**编译入口**按"当时的
  `max_stack_bytes`"（= 0）预留，而前端 `stack_addr(-N)` 要到 lowering 才统计 ⇒ **两者会重叠**
  （sysv64 那条用例恰好没有局部槽，所以没暴露）。现在 `plan_abi_slots` 在**展开前**扫一遍 IR 算
  前端局部槽深度（三处统计：`StackAddr` 立即数、`stack_addr(0)+iadd(iconst(-N))` 模式、`Alloca` 区；
  这一片先按 lowering 的算式镜像了一份，随即抽成**唯一实现** `pipeline/frame_slots.rs`——见下一条），
  ABI 槽从它之后排，并把结果喂给 `CompileState`（帧尺寸/序言偏移）。
- **守卫（确定性、已验证会咬）**：`va_expand::tests::abi_slots_sit_below_frontend_locals`——直接断言
  "保存区/对象排在前端局部槽之下"；把深度扫描短路后它报
  `保存区必须在前端局部槽之下（front=32, save_depth=176）` ✗，复原后通过 ✓。
  另加一条 JIT 用例 `test_jit_va_arg_sysv64_coexists_with_frontend_locals`（变参 + 前端局部槽一起用）。
  **诚实说明**：这条 JIT 用例**不能**替代上面的单测——实测把深度短路后它仍然绿（局部槽落在保存区
  的 XMM 段，而用例读的是 GPR 参数），所以放置正确性由那条单测钉住。
- 验证：`forge-codegen`（1362，含 8 条变参用例）、`forge-abi`、`forge-tests`（矩阵 196/3/0）、
  `forge-ir`、`forge-isa-dsl`、`forge-isa` 全绿；`clippy -D warnings` 0、`cargo fmt --check` 0。

### Added (2026-10-01) — 变参 V3 第四片：保存区形态的 `va_arg` 走 IR 展开（sysv64 两条支真跑）

保存区形态的取值是**有条件**的（游标未超上限取保存区、否则取溢出区，各自推进），这一片把它做成
**管线里的 IR 展开**（`pipeline/va_expand.rs`），而不是生成器序列：

- **为什么在 IR 层**：条件取值在 IR 只需 `Icmp`/`Select`/`Iadd`/`Uextend`/`Ireduce`/`Load`/`Store`/
  `Fload`/`Fptrunc`——**全都有现成降级**；生成器要发同样的事得新增 `cmp`/`cmov`/`add_rr`/`and`
  四个能力角色，且别的 ISA 还得各来一遍。
- **布局与宽度一律来自数据（评审点，已按此实现）**：字段偏移与宽度取自 plan 的 `va.fields`
  （**不再写死 sysv64 的 0/4/8/16**），游标按**它自己的宽度**读写（`u32` 就 4 字节、`u64` 就 8 字节），
  槽宽取 `va.save.slots[0].size`、上限/步长取 `va.init`（宿主从保存区槽表算，不写死 psABI 数字）。
  换机器/换约定只要还是"主游标 + 次游标 + 溢出指针 + 保存区指针"这一族就不改代码；不是这一族
  （aapcs64 的 gr/vr 计数、riscv 的分界）**明确报错**"该形态不在本族"，不按 sysv64 的偏移瞎算。
- **验收（真跑，两条支都覆盖）**：`test_jit_va_arg_sysv64_reads_the_register_save_area`（两个未命名
  实参都在寄存器 ⇒ `4*10+7 = 47`）与 `test_jit_va_arg_sysv64_overflows_to_the_stack_arg_area`
  （第 6 个超出 GP 池 ⇒ 必须改取溢出区并推进溢出游标 ⇒ `42`）。
- **仍然存在的 x86 专属（= 下一步要拆的洞，已记进方案）**：`va_start` 物化仍在生成器里，于是
  ① `VaInit.offsets` 把 `gp|fp` 打包成一个 u64（生成器只有 8 字节帧相对 store），② 为此新增的
  能力角色 `gpr_imm`。通用做法 = 把 `va_start` 也搬进同一个 IR 展开（字段按自己的宽度 store、
  帧内地址用现成 `StackAddr`），两者都会消失。
- 验证：`forge-codegen`（1360，含两条新 JIT 用例）、`forge-abi`、`forge-tests`（矩阵 196/3/0）、
  `forge-ir`、`forge-isa-dsl`、`forge-isa` 全绿；`clippy -D warnings` 0、`cargo fmt --check` 0、
  markdownlint 0。

### Fixed (2026-10-01) — sysv64 的栈实参落点少了一槽（收到的是返回地址）

`sysv64` 内置规则的 `first_offset_slots` 写的是 **1**（只算返回地址），而本实现的被调方**总是
`push` 帧指针** ⇒ 从 `rbp` 看第一个栈实参在 `[rbp + 16]`（返回地址 + 保存的 fp 各占一槽）。
win64/aapcs64/lp64d 本来就都写 2，只有 sysv64 这一份漏了。

- **实测证据**：新用例 `test_jit_sysv64_seventh_integer_arg_comes_from_the_stack`（7 个整数形参，
  第 7 个走栈）在修前返回 `2001632886933`（**返回地址**），修后返回 `7`。
- **影响面**：任何走栈的 sysv64 形参（第 7 个整数起、超 FP 池的浮点）此前都读错。与变参无关——
  这条路径此前**没有用例**，是做变参保存区时顺手撞见的。
- 黄金快照 `sysv64.plan.txt` 随之更新：`first_arg_off` 8 → 16、栈实参偏移整体 +8（逐行核对过；
  其余三份约定不变，因为它们本来就是 2）。

### Added (2026-10-01) — 变参 V3 发射（sysv64）：序言 spill 参数寄存器 + `va_start` 物化 `va_list` 对象

上一片把保存区的**计划面数据**做对了（槽表 + 字段布局），这一片把它接上发射：

- **管线预留保存区**：`CallLayout.va.save` 有区就按 `align`/`size` 在 locals 区之后开一段帧字节，
  偏移落在 `LowerCtx::va_save_off` / `AllocResult.va_save_off`（与 `Opcode::VaStart` 的对象槽
  同一条通路 ⇒ 两者不重叠）。实测（sysv64）：`size=176 align=16 depth=176 → 帧基址 -240`。
- **序言 spill**：`move_args` 在收参**之前**按槽表把参数寄存器写进保存区，**按槽的寄存器类分派**
  （GP 用 `stack_arg_store`、FP 用 `{ role = "stack_arg_store", class = "fpr" }`——两个角色本来就有）。
- **`va_start` 物化对象**（sysv64 一族）：`lea` 对象地址 + `lea` 溢出区 + `lea` 保存区，用**新角色
  `gpr_imm`**（x86 = `MOV_REG_IMM64`）把宿主算好的 `gp_offset|fp_offset` 打包值装进寄存器，
  再三条帧相对 store 写字段。"已用掉几个参数寄存器 / 溢出区从哪开始"由**宿主**按 plan 预先算好
  （`CallLayout.va.init: VaInit`）——生成物是各约定通用的，算不出来。
- **验收（真跑）**：`test_jit_va_start_materializes_the_sysv64_register_save_area`——两侧都用 sysv64，
  被调方用 **IR 算术**读出 `gp_offset`（= 8）与 `reg_save_area`，从 `[reg_save+8]`/`[+16]` 取回
  `4`/`7` 算出 **47**。这条路一次性验证了：序言 spill 的槽序与偏移、`gp_offset` 初值、
  `reg_save_area` 指向保存区。
- **仍是 ⬜**：保存区形态的 `va_arg`（分支式取值——游标超界要改取溢出区）⇒ sysv64 的 `va_arg`
  继续 fail-closed；aapcs64/riscv 的 `init` 未算 ⇒ 它们的 `va_start` 也继续 fail-closed（消息点名）。
- 验证：`forge-codegen`（1358，含两条新 JIT 用例）、`forge-abi`、`forge-tests`（矩阵 196/3/0）、
  `forge-ir`、`forge-isa-dsl`、`forge-isa` 等全绿；`clippy -D warnings` 0、`cargo fmt --check` 0、
  markdownlint 0。

### Added (2026-10-01) — 变参 V3 计划面：`va_list` 字段布局 + 寄存器保存区槽表

寄存器保存区（`sysv64`/`lp64d`/`aapcs64`）是变参最后一块功能缺口。先把**风险最大的东西**做对：
槽表与字段布局的数字——`va_arg` 的游标与序言 spill 都按它们走，算错就是**静默错值**。

- **计划面新增两样数据**（`AbiPlan::va_area`）：`fields`（`va_list` 对象字段：名/偏移/宽）与
  `save`（寄存器保存区：大小/对齐/槽表 `{寄存器, 区内偏移, 字节数}`）；`CallLayout.va` 有对应的
  **运行时中立镜像**（只带偏移/大小/类+号，不带名字）。
- **四份内置约定的数字**（由形态定的 psABI 事实，不是使用者选项）：
  win64 `cursor@0+8`（无保存区）；sysv64 4 字段 + 保存区 `176/16` = 6 GP（8 字节槽）+ 8 XMM
  （16 字节槽）；aapcs64 5 字段 + 保存区 `192/16` = 8 X + 8 V；lp64d `area@0+8` + 保存区 `128/8`
  = 8 X + 8 F。
- **踩到并修掉一个真问题**：浮点槽宽**不能取"寄存器类宽"**。psABI 规定的是**保存区槽宽**——
  按绑定宽度算会得到 arm64 `128`（应 `192`：V 槽恒 16 字节）、riscv `96`（应 `128`：FP 槽 =
  XLEN）。现在槽宽按**形态**给（与字段布局同一类事实）：`sysv_reg_save`/`aapcs64_struct` → 16、
  `riscv_save_area` → `slot_bytes`。
- **守卫**：`forge-abi` 的 `va_object_layout_matches_the_psabi_numbers`（四份约定的字段/槽表逐条
  对 psABI）+ `abi_target_real::call_layout_mirrors_the_variadic_shape` 扩展到断言镜像后的字段与
  槽表；黄金快照 `tests/golden/*.plan.txt` 重新生成（diff 只有新增的 `va_field`/`va_save*` 行）。
- **诚实边界**：发射侧（序言按槽表 spill、给保存区与对象开帧槽、`va_start` 写字段、`va_arg`
  分支取值）仍未做 ⇒ `va_start` 对保存区形态继续 **fail-closed**；**已知偏差**：arm64 的绑定把 V
  建模成 64 位 ⇒ 序言只会写低 64 位（向量/HFA 变参要 128 位存取，arm64 落地时处理）。
- 验证：`forge-abi`（含新守卫）、`forge-codegen`（1356 + `abi_target_real` 24）、`forge-tests`
  （矩阵 196/3/0）、`forge-ir`、`forge-isa-dsl` 全绿；`clippy -D warnings` 0、`cargo fmt --check` 0、
  markdownlint 0。

### Added (2026-10-01) — 变参进架构无关的 JIT 矩阵（`variadic_va_arg_int_and_float`）

变参此前只有 `forge-codegen` 里的 JIT 用例（单机、单 ISA），**架构无关的 `jit_matrix` 里
一个变参用例都没有**——等于跨 ISA 证据为零，而这正是矩阵存在的理由。这一片把它补上：

- **新用例 `variadic_va_arg_int_and_float`**（`CaseKind::Module`）：被调方变参 `callee(fmt, ...)`
  用 `va_start` 物化对象、读两次 `va_arg(i64)` 与一次 `va_arg(f64)`，算
  `a*10 + b + (i64)d`；调用方（非变参 `main`）多传三个实参 ⇒ 未命名实参按被调方语义进传出
  栈区。**`4*10 + 7 + 2 = 49`**（x86 矩阵实测：**196 passed / 3 skipped / 0 failed**，195 → 196）。
- **能力门控**：`VaStart`/`VaArg` 是**生成器专用臂**（谱里没有也不该有 `[[lowering]]`），所以
  把它们加进 x86 runner 的 `CAPS_EXTRA`（"非 lowering 路径"清单，与 `Call`/`GetElementPtr`
  同类）；其它 ISA 没申报 ⇒ 该用例按 `Skip("capability")` 诚实跳过。P1-16 的
  "`CAPS_EXTRA` 与 TOML 生成的 `SUPPORTED_OPS` 不相交"守卫继续成立。
- **顺带成为模块签名表接线的跨 ISA 守卫**：这条用例的调用方只有靠
  `JitCompiler::compile_module` 装表才会把未命名实参发到栈上（V1 那个缺口的现场）——
  矩阵里从此有一条真跑用例盯着它。
- `check_isa` 的 `UNCOVERED_OPS` 里 `VaStart`/`VaArg` 两条**理由改写**：它们说的是"逐 op 最小
  构造器造不出（要变参上下文）"，而不是"矩阵没覆盖"——矩阵现在覆盖了。
- 验证：`forge-tests --lib` 43（含矩阵 196/3/0）、`forge-codegen --lib --all-features`（1356）、
  `forge-ir`、`forge-isa-dsl`、`forge-abi` 全绿；`clippy -D warnings` 0、`cargo fmt --check` 0、
  markdownlint 0。

### Added (2026-10-01) — 第 5+ 个浮点形参的收参路径：补两条真跑用例 + 更正一处误导性注释

查"栈上的浮点形参收参"时，先按上一片的对称性假设它是缺口（调用方已按类分派、被调方还在用整数
load）——**实测证伪**，结论与文档/注释都按实测更正：

- **实际机制**：ABI 落在栈上的形参没有入场寄存器 ⇒ 不在 `assignments` 里 ⇒ 由 `move_args` 的
  "先收进 spill 槽"中转处理：`load 布局槽 → scratch(GPR) → store spill 槽`，**按位搬运**——
  所以**浮点参数本来就正确**（位型不变），FPR 溢出槽的 load 再按类还原。
- **`ArgPlace::Stack` 那一支里的"浮点直接收进寄存器"路径当前到不了**（未分配的形参在
  `#stack_arg_receive` 之后就 `continue` 了）。它是**防御性**分支：万一将来"栈落点 + 已分配
  寄存器"同时成立，把 FPR 当整数搬会静默错值，所以保持 fail-closed，并在注释里写清它需要
  `{ role = "stack_arg_load", class = "fpr" }` 才能接。原先那句"栈上的浮点参数收参尚未接进
  发射"是**误导**（听起来像功能缺失），已改写。
- **新增两条真跑用例（这条路径此前零覆盖）**：
  `test_jit_float_params_beyond_xmm_registers_come_from_the_stack`（6 个 f64 形参，只有前 4 个
  进 XMM0-3，取第 5/6 个算 `e + f = 7`）与
  `test_jit_stack_float_param_is_received_into_a_register`（只用第 5 个：`9.0 → 9`）——
  钉的是**端到端的值**（收参装错寄存器类会静默给错值，只看"编译通过"抓不到）。
- 文档：`docs/reference/calling-conventions.md` 的浮点栈实参两段按实测改写，守卫索引加一行。
- 验证：`forge-codegen --lib --all-features`（1356）、`forge-tests`（x86 矩阵 195/3/0）、
  `forge-ir`、`forge-isa-dsl`、`forge-abi` 全绿；`clippy -D warnings` 0、`cargo fmt --check` 0、
  markdownlint 0。

### Added (2026-10-01) — 变参 V4 第三片：`f32` 的默认实参提升（`va_arg` 取 f64 再窄回）

`f32` 走进来：按 **C/LLVM 的默认实参提升**，`float` 在变参调用里被传成 `double`，所以
`va_arg(ap, f32)` 必须**取 `f64` 再窄回**——只按 f32 读 4 字节会读到 promoted double 的**低半**
（垃圾位型，静默错值）。

- **新能力角色** `{ role = "fpr_narrow", bits = 32 }`（x86 = `CVTSD2SS`）：把默认提升后的浮点值
  窄回目标宽度（`bits` 用 S9 的宽度语义）。缺它 ⇒ `va_arg(ap, f32)` **明确 Unsupported**
  （`f64` 不受影响）。
- **D5 裁定同日更正**：提升规则**不放约定数据**，而是放在 `va_arg` 的语义里——C 的默认实参
  提升是**语言层**规则、不是 ABI 事实，LLVM 就是这么分工的（`va_arg` 指令语义规定"小于
  `double` 的浮点按 `double` 读再截断"，clang 在调用点做提升）。原先写的"进 `AbiRules` 的
  `va_promote`"是**过度数据化**：四份内置约定都是 double，加一个没人会改的键不合本仓库
  "没有消费者不加键"的纪律。**触发条件**：若某份约定的变参浮点提升在 psABI 层面不是 double，
  再加数据键。
- 发射序列（win64）：`cur = [ap]` → `tmp = [cur]`（fpr 取值，f64）→ `val = cvtsd2ss(tmp)` →
  `cur += slot_bytes` → `[ap] = cur`。
- **验收（真跑）**：`test_jit_va_arg_narrows_promoted_f32`——调用方传 `2.5` / `3.25`（f64），
  被调方两次 `va_arg(ap, f32)` 窄回后相加 = `5.75`，`fptosi` 得 **5**。
- 仍是 ⬜：寄存器保存区（`sysv64`/`lp64d`/`aapcs64` 的序言物化）、更窄浮点与向量的取值能力。
- 验证：`forge-codegen --lib --all-features`（1354）、`forge-tests`（x86 矩阵 195/3/0）、
  `forge-ir`、`forge-isa-dsl`、`forge-abi` 全绿；`clippy -D warnings` 0、`cargo fmt --check` 0、
  markdownlint 0。

### Added (2026-10-01) — 变参 V4 第二片：浮点（f64）未命名实参两端打通

整数类之后补上浮点：**调用方**把 f64 栈实参按类写进传出区、**被调方**用 `va_arg(ap, f64)` 读回。

- **两条按寄存器类限定的能力角色**（v18 S9 的 `class` 限定，与 `callee_save` 同类用法）：
  - `{ role = "stack_arg_store", class = "fpr" }`（x86 = `MOVSD_MR`）——调用方写浮点栈实参；
  - `{ role = "ptr_load", class = "fpr" }`（x86 = `MOVSD_R_MEM`）——被调方按 fpr 取值。
- **只做被调方那一半是不够的**：调用方若拿整数 store（`MOV64_MR`）搬 XMM，值会被静默写坏。
  所以 `arg_move_loop` 的栈参数分支改成**按实参的 IR 类型选 store**：浮点走 fpr 版；本 ISA
  没申报 fpr 版 ⇒ **明确 Unsupported**（不再静默错值）。同一路径也让**第 5+ 个命名浮点实参**
  归位（此前无用例覆盖）。
- 被调方 `va_arg` 的值取值同样按结果类分派：整数/指针走无类限定的 `ptr_load`（x86 = `MOV_R_MEM`，
  宽度随 IR 类型自动），标量浮点走 fpr 版。
- **覆盖范围（诚实清单）**：整数类 + **f64** 已通；`f32`/更窄要**提升规则**（D5：调用方按 C 的
  默认提升传 f64，读取方读 f64 再截断），向量要"mem → 向量寄存器"的取值能力——两者都在
  **运行期按结果类型 fail-closed**。
- **验收（真跑）**：`test_jit_va_arg_reads_unnamed_float_args`——变参 `callee(fmt)` 两次
  `va_arg(ap, f64)` 取回 `4.5` / `7.0`，相加后 `fptosi` 得 `11`。
- 验证：`forge-codegen --lib --all-features`（1353）、`forge-tests`（x86 矩阵 195/3/0）、
  `forge-ir`、`forge-isa-dsl`、`forge-abi` 全绿；`clippy -D warnings` 0、`cargo fmt --check` 0、
  markdownlint 0。

### Added (2026-10-01) — 变参 V4 第一片：`va_arg` 取值 + 原地推进（整数类，win64 端到端）

`va_list` 对象之上补上"取值"这一半：**`va_arg(ap, ty)`**（IR op 本来就有——`ops.toml`、文本层
`va_arg <ptrty> <ptr>, <ty>`、二进制格式往返；这一片补上 `FunctionBuilder::va_arg` 与发射）。

- **发射按三个能力角色取**（`ptr_load`/`ptr_store`/`add_imm`），序列 =
  `cur = [ap]` → `val = [cur]` → `cur += slot_bytes` → `[ap] = cur`；x86 谱把这三个角色打在
  `MOV_R_MEM`/`STORE_MEM_R`/`ADD_R_IMM32` 上（指令本来就有，只是没有角色——值是**宽度自动**
  的 `gprx` 槽，所以 i32/i64 都走同一条发射路径）。缺角色 ⇒ 整条臂明确 `Unsupported`。
  这三个角色**不是 ABI 能力**（`abi_view::role_capability` 返回 `None`）：`forge-isa abi check`
  不看它们，缺了由生成器点名。
- **只覆盖整数类结果**：浮点/向量要「mem → FPR」的取值能力（x86 谱今天没有，`fpr_mov` 是
  寄存器间搬运），所以**按结果类型 fail-closed**——不把 FPR 塞进 GPR 指令里静默编错值。
  提升规则（D5）随之留给"有浮点取值能力"的那一片：整数类今天靠"调用方按槽写、读取方按结果
  宽度截断"已经自洽。
- **验收（真跑）**：`test_jit_va_arg_reads_unnamed_stack_args`（V3 那条改用 `va_arg`）——
  变参 `callee(fmt)` 两次 `va_arg` 取回 `4` / `7`，算 `4*10+7 = 47`。
- 仍是 ⬜：寄存器保存区（`sysv64`/`lp64d`/`aapcs64` 的序言物化）、浮点取值能力、提升规则数据。
- 验证：`forge-codegen --lib --all-features`（1352）、`forge-isa-dsl`、`forge-ir`、`forge-abi`、
  `forge-tests`（x86 矩阵 195 passed / 3 skipped / 0 failed）全绿；`clippy -D warnings` 0、
  `cargo fmt --check` 0、markdownlint 0。

### Changed (2026-10-01) — 变参 V3 第一片：`va_start` 物化 `va_list` 对象（契约收紧，破坏性）

V2 把 `va_start` 定义成"取未命名实参区的地址"——那只对 **win64 栈式**成立，往寄存器保存区
（`sysv64`/`lp64d`/`aapcs64`）走时不通用（那些约定的 `va_list` 是多字段对象）。按
`docs/plans/varargs-plan.md` 的裁定先把契约收紧，这一片落地其中的**对象物化**：

- **`va_start` 现在物化 `va_list` 对象并返回对象地址**（破坏性：win64 多一个帧槽 + 一次 store，
  读未命名实参要先从对象里取出游标）。win64 的对象 = 1 个指针字段（偏移 0）= 未命名实参区地址。
- **对象槽由管线分配**（不新增"ABI 临时区"机制）：`Opcode::VaStart` 与 `StackAddr` 走同一条
  通路——管线按 `va_area.size/align` 选偏移、设 `ctx.current_offset`、抬 `max_stack_bytes`，
  生成器只读 `ctx.current_offset`，不猜偏移。为什么不让前端 `alloca`：`va_list` 的大小/对齐是
  **约定的数据**（win64 = 8、sysv64 = 24），前端不该知道；而 `max_stack_bytes` 本来就是管线在算。
- **发射**：`gen_va_start_lowering` = `lea` 对象地址 + `lea` 未命名区地址 + 帧相对 store 写字段 0，
  结果 = 对象地址。全部按角色取（`frame_addr` + `stack_arg_store`，谱里**没有** `VaStart` 的
  `[[lowering]]` 规则——与 `frame_set`/`frame_alloc` 同族：谱只申报能力，序列由生成器发）。
- **验收（真跑）**：`test_jit_va_start_reads_unnamed_stack_args` 改成**穿过对象**读回两个未命名
  实参（`load(ap)` → `inttoptr` → `load` / `gep` / `load`），仍是 `4*10+7 = 47`。
- **仍是 ⬜（诚实清单）**：寄存器保存区（`sysv64`/`lp64d`/`aapcs64` 的序言物化）、`VaArg` op 与
  提升规则；V4 还需要给"按 vreg 基址读写"与"寄存器 += 常量"申报三个能力角色
  （`ptr_load`/`ptr_store`/`add_imm`）——今天 x86 谱里那三条指令没有角色，生成器按纪律不按指令名探测。
- 验证：`forge-codegen --lib --all-features`（1352）、`forge-tests`（x86 矩阵 195/3/0）全绿；
  `clippy -D warnings` 0、`cargo fmt --check` 0、markdownlint 0。

### Added (2026-10-01) — 变参 V2 发射：被调方用 `va_start` 读回未命名实参（win64 端到端）

上一片只铺了数据面（IR 有 `VaStart`、`CallLayout.va` 有形态，但**没有任何发射路径**）。这一片把
被调方这条腿接通，并顺手补上它暴露出来的一个真缺口：

- **发射：`VaStart` 由生成器按能力角色发**（`lowering.rs::gen_va_start_lowering`）——`roles = ["frame_addr"]`
  （x86 = `LEA_RBP_OFF`）发一条 `lea dst, [fp + 未命名区起点]`，其余（`load`/`gep`/算术）全走
  既有规则。与 `frame_set`/`frame_alloc`/`callee_save` 同族：**谱只申报能力，序列由生成器发**
  （`isa/x86.toml` 里**没有也不该有** `VaStart` 的 `[[lowering]]` 规则）。寄存器保存区形态
  （`sysv64`/`lp64d`/`aapcs64`）与该约定不支持变参时**明确 fail-closed**，消息分别点名 V3 与
  "`va_list` 未声明"。
- **新 op 的落点是"未命名区起点"**：`max(命名栈实参 offset + size)`；没有命名栈参数时是
  **`first_arg_offset + shadow_bytes`**（win64 = 16 + 32 = 48）——**两个都要加**。引擎侧不变量是
  `Stack.offset(k) = first_arg_offset + shadow_bytes + k×slot`：`first_arg_offset` 是"帧基址 → 调用方
  sp"的距离，而调用方把第 0 个栈槽写在 `sp + shadow_bytes` 处。只取 `first_arg_offset` 会读到
  shadow 区里的垃圾（实测算出 1.4e15 量级的地址值）。
- **修掉一个真缺口：`JitCompiler::compile_module` 从未装模块签名表**。`with_module_sigs` 的唯一
  调用者是 V1 的测试本身，**整模块编译的宿主全都没接** ⇒ 变参调用点按非变参发（三个实参全进
  RCX/RDX/R8）、被调方 `va_start` 读到 shadow 区。修法是**让模块自述这张表**，而不是在每个宿主里
  再抄一份：`forge_ir::Module::signature_table()`（下标 = `FuncRef::index()`；读**每个函数自己的**
  类型上下文——`Module::add_function` 只做入表，不把签名并进模块 store，读 `module.types` 会全部
  落空）+ `FunctionCompiler::for_module(machine, &module)`（宿主侧默认写法）。
  `JitCompiler::compile_module`（串行 + 并行两条路径共用一份表）与 `forge-tests` 的矩阵/QEMU
  逐函数编译路径都改用它。
- **验收（真跑，不是断言落点）**：`jit.rs::test_jit_va_start_reads_unnamed_stack_args`——
  变参 `callee(fmt)` 用 `va_start` → `load` → `gep` → `load` 读两个未命名实参算 `a*10+b`，
  调用方 `main` 传 `(0, 4, 7)`，**返回值 = 47**。
- 连带：`forge-ir` 新增单测 `signature_table_mirrors_function_order`（普通/变参/void 三个函数
  的对齐与取值）。
- **仍缺（诚实状态）**：寄存器保存区（V3）、`%al`/`va_meta` 写入、`va_arg` 与提升规则（V4）、
  以及**任何前端产出 `variadic` 签名**；单函数编译的宿主（forge-rustc 逐函数后端）拿不到签名表，
  变参调用点按非变参处理——两条都记进 `docs/reference/calling-conventions.md` 的已知缺口表。
- 验证：`forge-ir --lib`（281）、`forge-codegen --lib --all-features`（1352）、`forge-tests --lib`（43）
  全绿；`clippy -D warnings` 0、`cargo fmt --check` 0；markdownlint 0。

### Fixed (2026-10-01) — 上一片留下的红门禁：`VaStart` 没进 lint 的能力缺口口径

`cargo test -p forge-isa-dsl --test lint_shipped` 在 HEAD 上就是红的——`f57979e` 把 `VaStart`
加进宿主 op 表（`forge-ir/ops.toml`）时没同步"能力缺口口径"快照，
`op_gap_matches_section_10_3` 于是把三谱的**真缺口**各多算一条（x86 3 → 4，列出的第 4 条正是
`VaStart`）。上一片的 CHANGELOG 验证清单里没列 `forge-isa-dsl`，所以漏掉了。

- **修法不是改快照数字，而是归对桶**：`VaStart` 与 `Bitcast`/`Call`/`CallIndirect`/`GetElementPtr`
  等一样属于"谱里**没有** `[[lowering]]`、但**不是缺口**"——只是成因不同：它由**生成器按能力
  角色**发（`roles = ["frame_addr"]` → `lea`，与 `frame_set`/`frame_alloc` 一族同写法）。
  因此 `lint::HOST_PIPELINE_OPS` 由 7 条变 8 条（并写明两类成因的差别），三谱的**覆盖数与真
  缺口数不变**（`100/6/8/3`、`61/6/8/42`、`8/6/8/95`）。
- 同步位置：`crates/frontend/forge-isa-dsl/tests/lint_shipped.rs` 的快照、
  `docs/plans/forge-isa-dsl-v19-plan.md`（§10.3 表 + §10.4 上方的机读化说明）、`CLAUDE.md` 的
  口径行。历史 CHANGELOG 条目（记 7 条的那段）**不改**——它记录的是当时的实测。
- 教训（写给下一片）：**新增宿主 op 必须跑一次 `forge-isa lint <谱> --ops …`**，
  `lint_shipped` 那条守卫就是为这件事存在的。

### Added (2026-09-30) — 变参 V2 数据面：IR 新 op `VaStart` + `CallLayout.va` 镜像

变参被调方要能"读到未命名实参"，先得有两样东西：**IR 里表达"取未命名实参区的地址"**，以及
**运行时读得到的 va 形态**。这一片把这两半落地（发射侧另起一片）：

- **IR：新 op `VaStart`**（`ops.toml` 一行：0 操作数 → 指针；`builder::va_start()`），语义是
  "取未命名实参区的地址"——它是**约定相关**的取值（win64 就是帧内那段栈地址；sysv64/riscv/arm64
  需要寄存器保存区）。`ops.toml` 追加在末尾 ⇒ 既有 opcode 编号不变（**不升格式版本**）。
  连带更新：`opcode_table` 的穷举 match + `type_rule = "none"` 计数基线（55 → 56）、
  `forge-opt` 的常量折叠（地址不是常量 ⇒ 不折叠）、`forge-tests` 覆盖矩阵的缺口清单
  （`("VaStart", "可变参数：需要 va_start ABI 夹具（方案 V2）")`）。
- **计划镜像：`CallLayout.va: Option<VaInfo>`**（运行时中立：`VaKind`/`size`/`align`/`stack_only`），
  由宿主的 `AbiPlan::va_area` **逐条 match** 折过来（加变体即编译失败）。
  `VaKind::supports_frame_addr_va_start()` 是"能不能直接用帧内栈地址实现 `va_start`"的判据——
  只有 `win64_stack` 为真；其余形态（寄存器保存区）留给方案 V3。
- **诚实状态**：`isa/x86.toml` 里**还没有** `VaStart` 的 lowering 规则，所以今天任何函数用
  `va_start` 都会因"没有规则"而 fail-closed——这一片只铺数据面，**不改任何现有行为**
  （新增的守卫是集成测试 `abi_target_real`，lib 计数仍是 1351）。
- 守卫：`abi_target_real::call_layout_mirrors_the_variadic_shape`（win64 栈式 8/8、
  sysv64 保存区 24/8、非变参不带 `va`）+ `forge-ir` 的 opcode 表守卫。
  文档：方案 V2 标 🚧（数据面 ✅ / 发射待做）、参考文档的 `CallLayout` 表与守卫索引各加一行。
- 验证：`forge-ir`（280+ 全绿）、`forge-opt`、`forge-abi`、`forge-codegen`（1352）、
  `forge-tests`（43）、`forge-hir` 全绿；`clippy -D warnings` 0、`cargo fmt --check` 0；
  markdownlint 0。

### Changed (2026-09-30) — 抽出 `pipeline/abi_setup.rs`：调用约定装配从 `CompileState::new` 里独立出来

- **三条约定层规则集中到一处**（`forge-codegen/src/pipeline/abi_setup.rs::setup_conv`）：① 约定名解析（未注册 fail-closed）；② **约定级整数返回槽**探针（空参 + 整数返回问一次引擎）；③ **函数级 plan 的 fail-closed**（错误消息含三步修法与自查命令）+ 约定级破坏集兜底 + 模块签名表落位。返回该函数的 `AbiPlan`（调用方存进 `CompileState` 供 `FORGE_TRACE_ABI=1` 打印）。
- 为什么：这三件事原先埋在 `CompileState::new`（约 300 行的构造过程）中间，**既读不出来也单独测不了**——只能"编译一个函数再看结果"。抽出来后调用点只剩一行，规则与错误消息在一个文件里，`#[cfg(test)] mod tests` 的 4 条单测直接断言"返回槽 = RAX / 布局 = win64 / 破坏集非空 / 未注册约定点名修法 / 模块签名表进出 `variadic_of`"。
- 顺便清掉 `compiler.rs` 里那 70 余行的内联块与三处重复的 `crate::pipeline::abi_target::…` 长路径。
- 验证：`cargo test -p forge-codegen --lib --all-features` **1351 passed**（含 4 条新单测）、`-p forge-codegen`（全 target，exit 0）、`-p forge-tests`、`-p forge-abi` 全绿；`clippy -D warnings` 0、`cargo fmt --check` 0。

### Added (2026-09-30) — 变参 V5：`forge-isa abi check` 上报变参状态（三种自相矛盾算硬错）

- **新增一行 `ℹ 变参 …`**（每份约定一条）：把"这台机器 × 这份约定的变参处于什么状态"说清楚——
  `win64` 栈式（`va_list` = 栈指针，调用方一侧可用）；`sysv64`/`lp64d`/`aapcs64` 需要**寄存器
  保存区** ⇒ 如实报"发射侧尚未物化（方案 V2/V3）"。这是**已知状态而非缺口**，只进 `ℹ`、
  不影响退出码（缺口语义保持"这台机器做不了"，否则 `--strict` 会把"发射尚未实现"混进机器能力账）。
- **三种自相矛盾升级为硬错**（影响退出码）：① `va_list = "win64_stack"` 却
  `variadic_stack_only = false`（未命名实参可能进寄存器，而 va_list 只指向栈）；
  ② 声明了形态却没给合法的 `va_list_size`/`va_list_align`（要 > 0 且 size 是 align 的整数倍）；
  ③ `va_meta_pool`/`va_len_pool` 点到的池在这台机器上解析不动。
- 守卫：`cli_tests::abi_check_reports_the_variadic_state`（三份发行谱的 `ℹ` 行 + "不加缺口"）与
  `abi.rs` 的 5 条 `variadic_report_*` 单测（矛盾 / 尺寸 / 池名 / 保存区 / 不支持变参）。
  方案文档的 V5 标 ✅，参考文档的守卫索引加一行。
- 验证：`cargo test -p forge-isa` 全绿（含 6 条 `abi` CLI 用例 + 5 条单测）。

### Changed (2026-09-30) — `LowerCtx` 的调用约定字段收进 `CallConvCtx`（破坏性重构）

- **6 个散落字段 → 一个结构体**：`LowerCtx::{call_conv, call_conv_name, call_layout,
  conv_clobbers, conv_ret_gpr, module_sigs}` 合并为 `LowerCtx::conv: CallConvCtx`，字段名
  自解释：`id`（IR 声明的 `CallConvId`）/ `name`（解析后的注册表键）/ `layout`（本函数布局）/
  `clobbers`（约定级破坏集）/ `ret_gpr`（约定级整数返回槽）/ `module_sigs`（模块级签名表）。
  理由：原来看不出哪些是"IR 声明的"、哪些是"宿主算好的"、哪些是"本函数的"。
- **两个把逻辑收进来的助手**：`conv.layout()`（替代到处写的 `.as_ref()`）与
  `conv.variadic_of(func_ref)`（把"按 `FuncRef` 查变参信息 + 越界兜底"从生成物搬回运行时，
  生成物里 4 行塌成 1 行，且宿主侧测试可直接覆盖）。参考文档与 `CLAUDE.md` 同步改名。
- 验证：`forge-isa-runtime` 4、`forge-abi` 7、`forge-isa-dsl` 18、`forge-codegen`（29 个测试
  二进制，exit 0）、`forge-tests` 2、`forge-opt`/`forge-mem`/`forge-grammar`/`forge-hir`/
  `forge-object`/`forge-plugin`/`forge-isa` 全绿；`clippy -D warnings` 0、`cargo fmt --check` 0。

### Changed (2026-09-30) — 调用点 API 的人体工学：`CallRequest` 结构体 + `CallPlanError` 带诊断（破坏性）

两个问题都是"用错编译器不会拦、出错看不出来"，按人体工学一次改掉（不留兼容层）：

- **一次调用 = 一个结构体**：`plan_call(isa, &CallRequest)` / `CallPlanner::plan_call(&req)` 取代
  `plan_call(isa, conv, args, rets, variadic)` 这种 4 个位置参数（`args`/`rets` 顺序与 `variadic`
  的含义都容易搞错）。builder 让调用点自解释：`CallRequest::new(conv).args(a).rets(r).variadic(2)`；
  `args`/`rets` 收 `IntoIterator<Item: Into<ArgShape>>`，数组、`Vec`、`&Vec`、`iter()` 都能直接传
  （为此加了 `impl From<&ArgShape> for ArgShape`）。宿主侧的 `plan_for_shapes(machine, reg, &req)`
  同步收敛成 3 个参数。
- **失败带诊断**：`plan_call` 由 `Option<CallLayout>` 改为 `Result<CallLayout, CallPlanError>`——
  `NoPlanner { isa }`（宿主没接 planner，消息给 `register_call_planner` / `ensure_registered` 两条
  出路）与 `Failed { conv, why }`（`why` 是**引擎原文**：池不够 / 缺绑定 / 能力缺口，消息带
  `forge-isa abi check` 与参考文档路径）。旧版把原因丢掉，调用点只能报"布局不可得"。
- **生成物**：调用点把 `CallPlanError` 的原文原样带进 `Unsupported`，不再自己编一句泛化文案。
- 守卫 `abi_target_real::call_plan_errors_carry_the_reason`（未注册 ISA ⇒ `NoPlanner`；注册了但约定未注册 ⇒ `Failed` 且正文点名约定与原因）；`call_planner_registry_serves_the_shape_plan` 的"未注册 ⇒ None"断言同步改为 `NoPlanner`。文档：参考文档「调用点 plan」补两条人体工学纪律，守卫索引加一行。
- 验证：`forge-isa-runtime` 4、`forge-abi` 7、`forge-isa-dsl` 18、`forge-codegen` 29、`forge-tests` 2、`forge-opt`/`forge-mem`/`forge-grammar`/`forge-hir`/`forge-object`/`forge-plugin`/`forge-isa` 全部绿；`clippy -D warnings` 0、`cargo fmt --check` 0。
  （**环境提示**：本机磁盘 97% / 常驻 node 进程 6 GB，`cargo test --workspace` 一次性跑会在编译
  `forge-ir` 的**测试二进制**时 OOM；逐包跑可过。`forge-ir` 本片未改动。）

### Docs (2026-09-30) — 调用约定层收口：`现状总览与守卫索引` + 文档守卫（索引不许指向不存在的测试）

- **`docs/reference/calling-conventions.md` 新增「现状总览与守卫索引」**：一张表看完"这一层做到哪一步、由哪条守卫钉住"——约定数据→计划、IR 读路径、适配器交叉核对、调用点按形状算落点、约定级返回槽、无 plan = 编译错误、多值返回、调用点形状摊成员、自定义约定、异域钩子、变参（规划 ✅ / 调用方 ✅ / 被调方 ⬜）、`[abi]` 删键的文档一致性、以及三条"评估后不做"（按成员拆、`callee_pop`、红区/尾调用）。
- **新守卫** `forge-abi/tests/doc_guard.rs::guard_index_names_exist`：表里"守卫"列的每个名字都必须在 `crates/**/*.rs` 里找到 `fn <名字>`——文档不许引用不存在的守卫（测试改名/删掉时会红）。同 `schema_guard` 对键表、`varargs-plan` 对 `va_list` 形状表的做法。
- 验证：`cargo test -p forge-abi --test doc_guard` 绿；改动的文档 markdownlint 0。

### Added (2026-09-30) — 变参 V0+V1：调用点按**被调方的变参语义**发未命名实参（win64 端到端）

- **`CallPlanner::plan_call` / `call_plan::plan_call` / `abi_target::plan_for_shapes` 多一位 `variadic: Option<(bool, u32)>`** = "被调方是不是变参、命名了几个"：调用点只看得见实参形状，而"未命名实参在**只走栈**的约定（win64/aapcs64/lp64d）里要改判到栈"只有知道被调方签名才判得出来。
- **模块级签名表**：`JitCompiler::compile_module` 按 `FuncRef` 建 `(variadic, 命名个数)` 表 → `FunctionCompiler::with_module_sigs` → `LowerCtx::module_sigs` → 生成物在 `Call` 处查表后传给 `plan_call`。IR **未改动**（沿用 LLVM 形态：变参签名只列**命名**参数，`Call` 可以多传实参；`Call` 本来就不校验个数）。单函数编译没有这张表 ⇒ 按非变参处理，与旧行为一致。
- **验收（可观察的发射决策，不是"算出来的值"）**：`abi_target_real::call_site_variadic_hint_decides_unnamed_argument_placement`（不给提示 → `Reg(RDX)/Reg(R8)`；给提示 → `Stack/Stack`；并与被调方视角落点一致）+ `jit.rs::test_jit_variadic_unnamed_args_go_to_stack`（装表 ⇒ `stack_arg_bytes == shadow(32) + 2×8 = 48`，不装 ⇒ 0；并 JIT 真跑一遍不崩、返回命名参数值）。
- **更正一处我自己的误判**：上一轮把"IR 表达不了未命名实参"记成必须先动 IR（D7）。实测证明**不需要改 IR**——那次是**测试把被调方建模错了**（3 个形参全列进签名又标 `variadic` ⇒ `fixed_count == 实参个数` ⇒ 变参提示无操作、用例假绿）。方案文档的 D6/D7 与实测经过已按事实改写（含"错/对两种建模"的对照表）。
- **仍缺**：`va_meta`（SysV 的 `%al`）写入、被调方 `va_area` 物化（V2/V3）、`va_arg`（V4）——都需要 `va_start` 的 IR 形态，且今天没有前端产出变参签名。
- 验证：`cargo test --workspace --exclude forge-rustc` 全绿、`clippy -D warnings` 0、`cargo fmt --check` 0、改动文档 markdownlint 0。

### Docs (2026-09-30) — 变参：实测"只接调用点信息"是**假绿**，补 D7（IR 表达未命名实参）

- **先试后写**：按 D6 选项②把整条"模块签名表 → `LowerCtx` → 生成物查表 → `plan_call`"接通，
  并加了一个 win64 的 JIT 端到端用例——**用例通过，但它是假绿的**：管线投影出来的
  `fixed_count` **恒等于** IR 形参个数（`sig_view` 只有 `variadic: bool` 可用），而 `Call`
  每个形参一个实参 ⇒ `unnamed = i >= fixed_count` 永远为假，变参提示传下去也是无操作，
  未命名实参根本不会出现。**已整片回滚**（不留无效果的机器），结论写进方案。
- **新决策点 D7**（IR/管线怎么表达"未命名实参"）：建议照 LLVM 形态——变参**签名只列命名参数**，
  `Call` 允许带比形参更多的实参（`declare @printf(ptr, ...)`）；它是 D6 的前置，两者建议一起做。
- **守卫加强**：`abi_target_real::call_site_shapes_cannot_express_variadic_placement` 补第 ④ 段——
  断言 `signature_view(变参函数).fixed_count == params.len()`（未命名实参在今天的 IR 里无法表达）
  且变参函数的 plan 有 `va_area`（被调方那一半是完整的）。
- 方案文档同步：§4 新增 D7、D6/D7 证据小节改写、§5 的 **V0 改为"两件一起做"**并记录这次
  "假绿 + 回滚"的过程与原因、V1 注明"验收要等 V2/V4"。
- 验证：`cargo test -p forge-codegen --test abi_target_real call_site_shapes` 绿；改动文档 markdownlint 0。

### Docs (2026-09-30) — 变参方案补 D6：**V1 的真正阻塞点是"调用点不知道被调方是变参"**

- **新决策点 D6**（调用点怎么知道"被调方是变参、命名了几个"）：`plan_call(isa, conv, args, rets)` 只吃**形状**，没有变参信息；而 win64 的 `variadic_stack_only = true` 会把未命名实参从寄存器改判到栈。同一个三 i64 签名，**形状路径给 `Reg(RDX)/Reg(R8)`、签名路径给 `Stack/Stack`** —— 所以 V1 缺的不是发射代码，而是这条信息通路（建议：管线编译模块时已有每个 `FuncRef` 的签名，把 `FuncRef → (variadic, fixed_count)` 表放进 `LowerCtx`，生成物在调用点查表后传给 `plan_call`；不动 IR）。
- **新守卫** `abi_target_real::call_site_shapes_cannot_express_variadic_placement`：把上面两条路径的落点差异钉成**可执行证据**，并断言二者不同——哪天一致了就会红，提醒复查 D6 与 V1 验收。
- 方案文档同步：§1 补"调用点拿不到变参信息"、§3 把 V1 的前置阻塞点写在发射代码之前、§4 新增 D6（含证据小节）、§5 新增 **V0（前置裁定）** 并把 V1 移到 V0 之后。
- 验证：`cargo test -p forge-codegen --test abi_target_real variadic` 绿；改动文档 markdownlint 0。

### Docs (2026-09-30) — 变参方案（`docs/plans/varargs-plan.md`）+ 形状表由引擎输出钉住

- **新方案文档**：变参（varargs）**规划已完成、发射与前端为空**的现状索引——分层列清"模型/引擎/黄金快照/CLI 有，发射与前端零消费"（并把证据落到符号名），四份内置约定的 `va_list` 形态表，五个决策点（先服务哪种场景 / `va_arg` 怎么表达 / 以哪份 psABI 定本 / 先做哪台机器 / 提升规则放哪），五期切片（V1 调用方 → V2 被调方 → V3 寄存器保存区 → V4 `va_arg` → V5 体检），以及**为什么现在不做 + 触发条件**（§6）。
- **新守卫** `invariants.rs::va_shapes_match_the_documented_table`：方案 §2 那张表（`VaListKind`/size/align/未命名实参去向/`va_meta` 寄存器）必须与引擎对四份内置约定实际算出的 `AbiPlan` **逐格相同**——这张表是"变参做到哪一步"的索引，写歪了会把后来的人引错。
- 索引与交叉引用：`docs/README.md` 的 plans 表加一行、并把调用约定方案一行更新到 A1–A8；`docs/reference/calling-conventions.md` 的缺口表新增"变参整体"一行指向该方案；v20 方案的 A6 剩余面改为指向新方案。
- 验证：`cargo test -p forge-abi --test invariants va_shapes` 绿；改动文档 markdownlint 0。

### Added (2026-09-29) — v20 A8：自定义约定的**端到端**证据（JIT 真跑）+ 缺口报错给下一步

- **新守卫 `test_jit_custom_convention_drives_argument_registers`**（`forge-codegen/src/runtime/jit.rs`）：运行期注册一份**自定义约定**（`int = ["R8","R9"]`、`ret_int = ["RAX"]`），编译 `callee(a,b) -> a-b` 与 `main() -> callee(20,7)` 并 **JIT 真执行**（得 13）；同时断言实参落点是 `(GPR(8), 8)`/`(GPR(8), 9)`——**故意不是** C 的 `RCX`/`RDX`。
  为什么这条最有价值：内置四份约定恰好都是 C 家族，"某个寄存器被写死"在它们身上看不出来（旧实现真写过"第二个返回值 = 类内号 1 = RDX"）；换自定义约定后写死就一定错值。这是"约定是使用者的数据、引擎不服务于个别指令集"的可执行证明。
- **缺口报错给下一步**：`AbiError::Unsupported`（单个聚合要 ≥3 个寄存器槽）的正文从"≥3 的返回搬运见 A6"改为**说清两条路**——"按成员拆（暂无按值聚合的产出者）"与"多个**独立**返回值请用多值签名"；守卫 `errors.rs::three_slot_return_is_explicitly_unsupported` 同步断言这两个关键词。
- 文档：`docs/reference/calling-conventions.md` 的「加自己的约定」补端到端证据一段（含为什么内置四份证明不了这件事）。
- 验证：`forge-abi` + `forge-isa-dsl` 24 个测试二进制全绿、`forge-isa --test cli_tests abi` 5 绿。

### Docs (2026-09-29) — v20 A8：文档一致性收口（删键后的死引用 + 去处必须存在）

- **改掉仍把 `[abi]` 当活键的句子**：`docs/reference/calling-conventions.md`（"整函数退回 `[abi]` 路径"→ 现在**没有旧路径可退**，落点没接就是明确 `Unsupported`；"`abi_view` 读 `[reg.*]`/`[abi]`"→ 改读 `[reg.*]`/`[machine]`；Win64 的 XMM 条改为 `[machine].callee_saved_gpr`）、`docs/reference/isa-dsl.md`（push 机制保存的是**约定数据**，无 plan 才退机器事实）。
- **补齐 `callee_pop` 的"为什么没消费"**（诚实清单新增一行）：计划里算得出来（`AbiPlan::callee_pop_bytes`），但本实现的传出参数区在**调用方帧内**（不是"push 上去"），被调方 `ret N` 会把调用方 sp 抬高 N ⇒ 只改尾声必然错位，必须同时定**调用点契约**才谈得上正确；今天没有这样的 ABI 宿主。
- **新守卫** `schema_guard::deleted_abi_keys_stay_deleted_and_their_destinations_exist`：① schema 键表里不得再出现 `[abi]` 节；②「原键 → 去处」对照表的去处列必须写成显式 `` `[machine].<键>` ``，且这些键**真的存在于 schema 键表**——文档不许把读者引到不存在的键上（为此把对照表那一行改写为显式路径）。
- 验证：`cargo test -p forge-isa-dsl --test schema_guard` 7 绿；改动文档 markdownlint 0。

### Changed (2026-09-29) — v20 A6：接入体验收口（fail-closed 报错带修法）+「按成员赋值」评估结论

- **报错带得动修法**：编译入口"规划不出调用布局"的消息改为**并列两种原因 + 三步修法 + 自查命令 + 文档路径**（`register_rules_toml`/`register_binding_toml`、`forge-isa abi check`/`abi plan`、`docs/reference/calling-conventions.md`、`FORGE_TRACE_ABI=1`）；生成物里调用点的"布局不可得"同样点名两种原因（宿主没注册 planner / 该签名规划不出来）。守卫 `conv_registry_read_path` 现在逐项断言这些关键词都在——报错不允许退化成死胡同。
- **教程补一节**：`docs/guides/isa-dsl-tutorial.md` 新增「6. 要编译（`parts` 含 `tm`）就必须接调用约定」——为什么（约定是使用者的数据、没注册就 fail-closed）、可抄的三步（规则 TOML → 绑定 TOML → IR 里 `CallConvId`）、以及三条自查命令；原 §6/§7 顺延为 §7/§8，并补指向调用约定参考文档。
- **「按成员赋值」评估结论：暂不做（有依据）**。做它（单个聚合要 ≥3 寄存器 / HFA 部分在寄存器）的前提是"聚合按值放在寄存器里"，而**今天的 IR 没有这种形态的产出者**：`forge-rustc` 的 `AbiKind::{Scalar, Pair, Indirect}` 把 ScalarPair 拆成两个标量、内存聚合走 sret/间接；`examples/mini_c` 没有结构体类型；全仓非测试代码构造 `Struct`/`Array` 类型 **0 处**；后端的 `ExtractValue`/`InsertValue` 本就是"在聚合的 64 位域里做字段操作"（≤8 字节聚合 = 位打包标量，本来就只占 1 个寄存器）。落点侧已就绪（`RetLoc::RegGroup`/`ArgPlace::Group`），触发条件写进方案 A6 ⑤。
- 验证：workspace 131 个测试二进制全绿、clippy `-D warnings` 0、`cargo fmt --check` 0、改动文档 markdownlint 0。

### Fixed (2026-09-29) — v20 A6：调用点形状必须**摊开成员**（HFA 聚合实参曾静默搬错寄存器）

- **问题**：调用点的实参/返回形状是**就地**拼的，聚合只报 `Aggregate { members: [] }`、向量恒报 `elem_is_float: false, lanes: 0`。而 **HFA/HVA 判定靠成员**：AAPCS64 上的 `{f64,f64}` 该走浮点池（`V0:V1`），成员留空则退化成"≤16B 聚合按两个整数槽"（`X0:X1`）——于是**调用点**与**被调方**（函数级 plan，成员齐全）分叉，实参静默落到错寄存器。
- **修法**：`forge-isa-runtime` 新增 `ArgShape::from_ir_type(store, ty)`——从 IR 类型递归摊开成员与元素（`Struct`/`Array`/`Vector`；深度 4、成员 16 封顶，超限退化成"没有成员"交给规则兜底）；生成物的调用点改用它投影实参与**全部**返回值形状。
- 连带效果：单寄存器 HFA（如 `{f32}`）在调用点从"算成 2 个整数槽 ⇒ fail-closed"变成**真的能发**（`Reg(V0)`）；多寄存器 HFA 仍是 fail-closed（按成员拆属"按成员赋值"那条 A6 活），但报错点从"落点不对"变成"落点对、发射未接"。
- 守卫 `abi_target_real::hfa_aggregate_shape_matches_the_callee_plan`：`{f64,f64}` 参数在 aapcs64 上，**调用点 plan == 被调方 plan**（`V0:V1`），并**反证**"成员留空的形状必然与真实 plan 分叉"。
- 验证：workspace 131 个测试二进制全绿、clippy `-D warnings` 0、`cargo fmt --check` 0。

### Changed (2026-09-29) — v20 A6：**多值返回**（≥3 槽返回的搬运）——落点全部来自绑定

- **`Signature.ret` → `rets: Vec<TyView>`**：IR 的 `FunctionSignature.returns` 本来就允许多个返回值（ScalarPair 拆出来的两个标量、`(i64,i64,i64)` 这类），而 `sig_view` 以前只取 `first()`——"第二个返回值在哪个寄存器"于是只能由发射侧猜。
- **引擎**：`rets.len() > 1` 时**逐个分类、逐个占一个返回寄存器**，产出 `RetLoc::RegPair`（2 个）/ `RetLoc::RegGroup`（≥3 个，新增变体）；每个分量必须是 `direct { slots = 1 }`，否则明确 `Unsupported`（聚合走单值路径）；池不够 ⇒ `PoolExhausted`。
- **修掉一处 ISA 特定硬编码**：发射侧原本写死"第二个返回值进类内号 1"（x86 恰好是 RDX）——riscv 的类内号 1 是 X1（= ra），返回槽其实在 X10/X11。现在 win64 的 `ret_int` 补为 `["RAX","RDX"]`（`ret_float` 补 `["XMM0","XMM1"]`），落点一律来自绑定池；`Return` 与调用点按 `RetPlace::Pair`/`Group` **逐值按类分派**（整数 → `gpr_mov`、浮点 → `fpr_mov32/64`）。
- **调用点看全结果**：`call_plan::plan_call(isa, conv, args, rets)` 与 `abi_target::plan_for_shapes` 收**全部**结果形状（此前只传第一个）；`RetPlace::Group` 带 `regs()` 助手。
- 守卫：`forge-abi/tests/invariants.rs` 两条（win64 `RAX:RDX`、lp64d `X10:X11`、aapcs64 四个 f32 → `V0..V3`、混合类 `RAX:XMM1`；以及池不够/分量是聚合两条 fail-closed）、`abi_target_real::two_value_return_is_plan_driven`、**JIT 端到端** `test_jit_multi_value_return_is_read_back`（`callee(5) -> (6,7)`，调用方算 `6*10+7 = 67`）。
- 仍 fail-closed（诚实清单）：单个聚合要 ≥3 个寄存器的返回（AAPCS64 的 4×f32 HFA，需按成员拆，属"按成员赋值"规则语言）；`a6_gap_inventory` 的那条缺口集不变。
- 验证：workspace 131 个测试二进制全绿。

### Added (2026-09-29) — v20 A7：Swift / Go 的 `AbiHooks` 示例（异域约定的正规出口）

- **新示例**：`crates/foundation/forge-abi/tests/exotic_hooks.rs`（`cargo test -p forge-abi --test exotic_hooks`，3 个用例）——Swift 的 aapcs64 方言与 Go 的 amd64 内部 ABI 各一份钩子 + 规则/绑定 TOML + 断言。证明"数据表达不了的部分"有正规出口，而不是去改引擎或往谱里塞特例。
- 三个事实各演示一个：① **隐式上下文寄存器**（Swift `self` = X20、Go `g` = R14）只能由钩子填进 `AbiPlan::hidden.context`（数据侧没有写入者，参数列表也表达不了"隐式参数"）；② **语言专属类型**（Swift 的 errortype，LLVM 靠 `swifterror` 属性标识）由 `classify` 改判到 `swift_error` 池；③ **"保留"与"保存"不同**——X21 是 error **出参**，绑定的 `cs_gpr` 池只能说"保存这组"，钩子把它从 `callee_saved.regs` 摘掉并加进 `clobbers`（与 LLVM 的 `CSR_AArch64_AAPCS_SwiftError = CSR_AArch64_AAPCS − X21` 同义）。
- 数值有出处：LLVM `AArch64CallingConvention.td` 的 `CCIfSwiftSelf → X20` / `CCIfSwiftError → X21`；Go 内部 ABI 的 `g` 在 amd64 的 R14。文档入口：`docs/reference/calling-conventions.md` 的「数据表达不了的：AbiHooks」新增对照表。

### Changed (2026-09-29) — v20 A6：**无 plan 不再继续编译**（编译入口 fail-closed）+ A6 缺口清点

- **fail-closed 切换**（`pipeline/compiler.rs`）：`plan_for_function` 失败不再"留个 note、退回约定级数据继续编译"，而是直接在**编译入口**报 `IrError::Unsupported`——消息点名约定名、引擎给的原因、以及 fail-closed 口径。`CompileState::abi_plan` 由 `Option` 收敛为定值，`abi_plan_note` 字段删除（`FORGE_TRACE_ABI=1` 也随之只打印计划）。理由是旧行为把错误推给发射侧的各个 fail-closed 点（调用点/收参/栈参数/帧布局），消息离现场远，而且真实的引擎缺口会被误当成"这条签名能编"。
- **先量后切**：切换前用 `FORGE_TRACE_ABI=1 --nocapture` 跑遍 `forge-codegen --lib --all-features`（1344 例）与 `forge-tests --lib`（43 例，含三台机器的 JIT 矩阵）——`[abi-plan]` **零命中**，即既有语料里没有一条路径靠"无 plan 回退"跑通。切换后 workspace 130 个测试二进制全绿。
- **A6 缺口清点**（守卫 `abi_target_real::a6_gap_inventory`）：16 组代表性形状 × 三台真机 × 四份约定，**精确断言**缺口集恰好一条——`arm64 / aapcs64` 的 **4×f32 HFA 返回**（要 4 个连续寄存器槽，`RetLoc` 今天只有单/双寄存器形态）。此前列为缺口的 `Pair`（2 槽聚合）、**无指针的 `Indirect`**（by-ref 指针本身溢出到栈）、按引用向量、参数溢出到栈、浮点溢出到栈、混合两套寄存器文件的形状**都已有 plan**。
- **唯一需要改的既有用例**：`conv_registry_read_path::rules_without_binding_fail_closed_at_the_compile_entry`（原 `…_on_the_return_slot`）——只有规则没有绑定时，错误现在发生在编译入口并点名"这台 ISA 没有为约定 X 注册寄存器绑定"，比原先"发射侧返回槽报错"更早、更具体。
- 验证：`cargo fmt --check` 0、`clippy --workspace --exclude forge-rustc --all-targets --all-features -D warnings` 0、workspace 串行全套 130 个测试二进制绿。

### Changed (2026-09-27) — v20 A5-3 收口：`[abi].arg_class` 删除，**`[abi]` 一节整体下线**

- **调用方按被调方落点搬实参**（`gen_call_lowering`/`arg_move_loop` 重写）：调用点把实参 IR 类型摊成 `ArgShape`（size/align/族）→ `machine::call_plan::plan_call(ISA, 约定, 形状, 返回形状)` → 逐实参按 `ArgPlace` 发射——`Reg { class, index }`（int → `gpr_mov`；fp → 按字节宽 `fpr_mov32/64`；向量 → `vec_mov`）、`Indirect { reg }`（`byref` 副本 + 指针 mov）、`Stack`（`caller_offset(k)`）、`Ignore` 跳过；`Pair`/`Group`/无指针 `Indirect` ⇒ **fail-closed**（A6 缺口，旧实现会把它们静默塞进整数寄存器）。`sret` 隐藏槽也改用 `CallLayout.hidden_sret`（不再假设"首 int 参数槽"）。
- **被调方收参只走布局路径**：谱面的 `[abi].arg_class` 删掉后，`move_args` 的"无布局回退"整段改为 **fail-closed**（旧实现按谱面顺序塞寄存器 = 静默错值）。
- **宿主注册调用点 planner**：`pipeline_hooks::ensure_registered()` 为三个发行后端注册 `register_isa_call_planner`（`Box::leak` 持有机器），并由 `FunctionCompiler::new` 幂等触发——生成的 `ensure_registered()` 不能直接调宿主（生成物里的 `crate::pipeline` 会被改写成运行时路径）。
- **能力缺口改由角色说话**：没有 `roles = ["gpr_mov"]` 的谱（只做编码试点的夹具）在 Call/收参处整条降级 `Unsupported`，而不是生成期报"mov 形状不符"。
- **`[abi]` 删除**：`Abi`/`ArgClass`/`ArgClassKind`/`ArgStrategy` 类型、`IsaModel::abi` 字段、schema 的 `[abi]`/`[abi.arg_class]` 两节、文档键表与 `isa-dsl.md` 的 `[abi]` 章（改写为"已删除 + 去处对照表"）全部下线；三份发行谱与两份夹具的 `[abi]` 段删除。
- **两处生成期近似一并归位**：向量 by-ref 阈值 → 机器事实 `[machine].vector_by_ref_bytes`（x86 = 16）；`RegAllocConfig.param_reg_count` → 由 plan 数"落在寄存器的形参个数"（不再用 `TargetABI::int_arg_slot_count`）。
- 验证：`forge-tests --lib` **43 绿**（含 x86/riscv/arm64 三条 JIT 矩阵与 QEMU 跨函数调用）、`forge-codegen --lib` 1278 绿。

### Changed (2026-09-27) — v20 A5-3：被调方 spill 收参改按 layout（不再读谱面 `[abi.arg_class]` 的顺序）

- **`move_args` 的 spilled 寄存器参数**（`crates/frontend/forge-isa-dsl/src/v12/codegen/frame.rs`）：来源寄存器不再按"谱面 `int_regs[__pos]`"取，而是读**被调方布局** `ArgPlace::Reg { class, index }`——按位置/按类计数、`sret` 占不占首槽都由引擎算好；拿不到布局或落点不是寄存器 ⇒ **fail-closed**（不按谱面顺序猜）。
- 这是 `[abi].arg_class` 的消费者之一被摘掉；剩下的消费者是**调用方**的参数搬运（`gen_call_lowering`/`arg_move_loop`）与 `gen_abi` 的 `arg_regs()`/`int_arg_slot_count`（宿主 `param_reg_count`）——它们是下一步（见计划文档）。
- 验证：workspace 串行全套绿、`forge-tests --lib` 43 绿（含 x86/riscv/arm64 三条 JIT 矩阵）、clippy `-D warnings` 0、`cargo fmt --check` 0。

### Changed (2026-09-27) — v20 A5-3：`[abi].ret_regs` 删除（返回槽由 plan / 约定级数据给）

- **谱面删键**：`[abi].ret_regs`（`Abi` 字段 / schema / 文档键表 / riscv64 与 arm64 谱里的声明、`demo8` 夹具的声明一并去掉）。它一直是"返回槽放哪"的**生成期近似**——`x86` 靠 index 0 兜底、`riscv` 声明 `X10`，而 `arm64` 的 `X0` 恰好也是 0，这种"每台机器各写一遍"的近似正是 A5-3 要收掉的东西。
- **`Return`**：优先读**本函数的 plan**（`CallLayout.ret` 的 `RetPlace::Reg`），否则读宿主的**约定级整数返回槽** `LowerCtx::conv_ret_gpr`；两者都没有 ⇒ **fail-closed**（不再按 `index 0` 猜——riscv 的返回槽是 `X10`）。
- **`Call`**（读被调方返回值）：读 `LowerCtx::conv_ret_gpr`（调用点看不到被调方的签名，而"标量整数返回放哪个寄存器"是约定级事实）。
- **宿主的约定级数据**（`pipeline/compiler.rs`）：用"**空参 + 整数返回**"的合成签名问一次引擎（`plan_for_shapes`）填 `conv_ret_gpr`——与签名无关，与 `conv_clobbers` 同一套路。
- **trait 收敛**：`TargetABI::ret_regs()` 现在**没有消费者**（生成物改读 plan / LowerCtx），DSL 不再从谱里发射它，运行时 trait 给它一个空表缺省实现（手工后端不破）。
- 守卫：`abi_target_real::convention_level_int_return_slot` —— `plan_for_shapes(空参, i64 返回)` 必须给出**谱里原来那些值**（win64 `RAX`、lp64d `X10`、aapcs64 `X0`）。
- 直连 lowering 的测试（`integration_tests::terminator_return_packet`）改为自己填 `ctx.conv_ret_gpr`——真实编译路径上由管线填好。
- 验证：workspace 串行全套绿、clippy `-D warnings` 0、`cargo fmt --check` 0、三条 JIT 矩阵同值。

### Added (2026-09-27) — v20 A5-3：demo 夹具补**测试本地约定**（夹具从此真的有 plan）

**背景**：夹具谱（`tests/isa/demo.toml`、`demo8.toml`）没有约定数据，`Builtin(C)` 解析到内置 `c` 又没有 `(demo, c)` 绑定 ⇒ `AbiPlan` 算不出来，夹具函数一直走"**无 plan 回退**"。而 A5-3 要删的 `[abi].arg_class`/`ret_regs` 正是那条回退路径的输入——夹具不先有 plan，删键就会连带炸掉它们。

- **测试本地约定**（`crates/backend/forge-codegen/tests/common/mod.rs`）：`ensure_demo_conventions()` 幂等注册两份规则 + 绑定（`register_rules_toml`/`register_binding_toml`，v20 A2 就有的公开 API）。数据照夹具谱口径写：`demo` = int 池 `X0-X3` / `ret_int = X0` / `stack 8,8` / `aliases = ["c"]`；`demo8` = int 池 `A0-A3` / `ret_int = A0` / `stack 1,1` / `aliases = ["c"]`；两者都无 callee-saved（与 `[machine].callee_saved_gpr = []` 一致）。
- **四个夹具测试接线**（`demo8_tests.rs`、`demo_tm_tests.rs`、`lowering_read_path.rs`；`conv_registry_read_path.rs` 用自己的 `probe_conv`，不受影响）。
- **两条新守卫**：`demo8_fixture_now_plans_via_the_local_convention` / `demo_fixture_now_plans_via_the_local_convention`——编译产物必须带 `call_layout`（`conv` = `demo8`/`demo`、参数落 A0/A1），**证明夹具真的走在 plan 路径上**而不是继续静默回退。
- 验证：三个夹具测试二进制（9 + 5 + 3 用例）与全量 workspace 串行套件全绿、clippy `-D warnings` 0、`cargo fmt --check` 0。

### Added (2026-09-27) — v20 A5-3：「调用点 plan」基础设施（形状 → 被调方布局）

**为什么要有**：调用点在 lowering 时只有实参的**形状**（大小/对齐/族/成员），没有被调方的 `Function`；而"第 i 个实参进哪个寄存器/栈槽"只有引擎算得出来（规则 + 绑定 = 使用者的数据）。谱里那份 `[abi.arg_class]`/`ret_regs` 只是这份数据的**生成期近似**——这才是它们删不掉的真正原因。

- **中性形状**（`forge-isa-runtime::machine::call_layout::ArgShape`/`ShapeKind`）：`Int`/`Ptr`/`Float`/`Vector{elem_is_float, lanes, elem_bytes}`/`Aggregate{members}`/`Other`。运行时 crate 仍**不依赖 forge-abi**。
- **桥**（`forge-codegen::pipeline::abi_target::plan_for_shapes`）：形状 → `forge_abi::TyView`（聚合成员递归，HFA 判定靠它）→ `Signature` → `AbiRegistry::plan` → `AbiPlan`。
- **查询注册表**（`forge-isa-runtime::machine::call_plan`）：`CallPlanner` trait（闭包 blanket impl）+ `register_call_planner(isa, …)` / `plan_call(isa, conv, args, ret)`；未注册 ⇒ `None`（生成物 fail-closed，不猜落点）。宿主在 `forge_codegen::pipeline_hooks::ensure_registered()` 里为三个发行后端各注册一个闭包（`Box::leak` 持有自己的 machine）。
- **两条守卫**（`crates/backend/forge-codegen/tests/abi_target_real.rs`）：① `shape_plan_matches_the_function_plan`——**形状算出来的 plan 必须与函数算出来的逐项相同**（win64 六整数含栈参数 / win64 混合按位置计数 / lp64d 混合按类计数 / aapcs64 整型；比落点/栈/callee-saved/hidden/clobbers，不比参数名）；② `call_planner_registry_serves_the_shape_plan`——注册表路径与直接算逐项相同，未注册 ISA 返回 `None`。
- 本片**只建能力不改发射**（调用方仍按谱面搬实参），因此三份发行谱零行为变化；下一片切发射后即可删 `[abi].arg_class`/`ret_regs`。
- 验证：`abi_target_real` 17 绿、workspace 串行全套绿、clippy `-D warnings` 0、`cargo fmt --check` 0。

### Changed (2026-09-27) — v20 A5-3：`[abi].call_clobbers` 删除（破坏集改由宿主的约定级数据给）

- **谱面删键**：`[abi].call_clobbers`（`Abi` 字段 / schema / 文档键表一并去掉）。它一直是"无 plan 时按参数寄存器 + 返回寄存器猜"的兜底——**猜漏 callee 破坏的临时寄存器就是静默错值**（riscv 递归 fib 死循环的根因）。
- **正确数据其实与签名无关**：`AbiPlan::clobbers` = 可用池 − callee-saved − pinned，只取决于 `(ISA, 约定)`。于是管线在**本函数规划失败**时用**空签名**问一次引擎（`plan_for_signature` + `Signature::new(vec![], None)`），把结果塞进新的 `LowerCtx::conv_clobbers`（`(类内号, 类)`）。
- **生成物**：`lowering.rs` 的调用点破坏集改成「有 plan 用 plan 的 `clobbers`；否则用 `ctx.conv_clobbers`；两者都没有 ⇒ **fail-closed**」（不再按谱面猜）。
- **守卫 ③b**：`abi_target_real.rs` 的四约定交叉核对新增「空签名的 plan 与真实函数 plan 的 `clobbers` 逐项相同」——这条不变式正是回退路径的正确性前提。
- 验证：workspace 串行全套绿（forge-codegen lib 1344、abi_target_real 15）、clippy `--all-targets --all-features -D warnings` 0、`cargo fmt --check` 0、markdownlint 三份文档 0 issue；三条 JIT 矩阵不变（本改动只在"无 plan"路径上生效，三份发行谱都有 plan）。

### Changed (2026-09-27) — v20 A5-3：`[abi].arg_slot` 迁到 `[machine]`，`AbiPlan` 带上 `position`

- **`[abi].arg_slot` → `[machine].arg_slot`（机器事实）**：位置计数规则（`by-class` = int/float 各自独立推进；`by-position` = 共享位置游标，Windows x64）是**这台机器的谱面缺省约定怎么数位置**，写给**无 plan 的回退形态**；约定侧的正式位置是 `AbiRules::position`，现在随 plan 一起给出（新增 `AbiPlan::position`，`to_text()` 的 `variadic` 行带上 `position <值>`，四份黄金快照同步）。
- 读取侧单点化：`IsaModel::machine_arg_slot()`；生成物新增 `TargetABI::arg_placement()`（运行时侧的 `ArgPlacement::{ByClass, ByPosition}`，缺省 `ByClass`，手工后端无需改）；`codegen/{frame,lowering}.rs` 的 `by_position` 标志改读机器事实。
- **新增守卫（②b）**：`abi_target_real.rs` 的四约定交叉核对新增一条——「plan 的 `position` == 机器事实 `[machine].arg_slot`（生成物的 `arg_placement()`）」；只对 **ISA 主约定**成立（x86 的 sysv64 规则本就是 `by_class`，属 `$equal_cs = false` 的第二种情形）。
- 三方同改：`schema.rs` 键位移 + 重生成的 `isa-dsl.schema.json` + `docs/reference/isa-dsl.md` 键速查表（`[abi]` 只剩 `arg_class`/`ret_regs`/`call_clobbers`）。
- 为什么这条能安全先迁：`position` 是**约定级而非签名级**事实，机器事实与规则值必须同值（②b 守卫钉住）；真正需要"调用点 plan"的是 `arg_class`/`ret_regs`（调用方要按**被调方**的落点搬实参）。
- 验证：workspace 串行全套绿、clippy `-D warnings` 0、四份 `forge-abi` 黄金快照按 `FORGE_ABI_BLESS=1` 重生成后逐行核对（win64 = `by_position`，sysv64/lp64d/aapcs64 = `by_class`）。

### Changed (2026-09-27) — v20 A5-3：机器事实回退键下线（`[abi].{scratch,reserved,call_ret_reg,frame,callee_saved}` 删除）

- **谱面**：`[abi]` 只剩真正的**约定键**（`arg_class`/`ret_regs`/`call_clobbers`/`arg_slot`）；机器事实一律只在 `[machine]`（`spill_scratch`/`fixed_regs`/`link_reg`/`frame`/`callee_saved_gpr` + `callee_save_slots`）。旧同义键**删除**（`Abi` 字段、`CalleeSaved` 结构体、schema 的 `[abi.frame]`/`[abi.callee_saved]` 两节与键表一并去掉）：写旧键现在报未知键，"两处都写、谁生效"的含糊彻底消失。
- **读侧单点化**：`machine_scratch`/`machine_reserved`/`machine_link_reg`/`machine_frame`/`machine_callee_saved`/`machine_callee_save_slots` 全部改成**只读 `[machine]`**（`callee_save_slots` 缺省 0，不再由名单长度兜底）。名字解析校验随之只挂在 `[machine]` 键上（原先 `[abi]`/`[machine]` 双份）。
- **夹具迁移**：`tests/isa/demo8.toml`（`scratch`/`reserved`/`[abi.callee_saved]` → `[machine] spill_scratch`/`fixed_regs`/`callee_saved_gpr`）、`tests/isa/demo.toml`（`[abi.callee_saved]` → `[machine] callee_saved_gpr = []`）、`diag_matrix_tests.rs` 的内联谱（`scratch` → `[machine] spill_scratch`；"未知寄存器名"用例改用仍在 `[abi]` 的 `ret_regs`，保住 `DSL-ABI` 码）。
- 验证：workspace 串行全套绿、clippy `-D warnings` 0、三条 JIT 矩阵与迁移前同值。

### Changed (2026-09-27) — v20 A5-3：`[abi.stack_args]` 删除（栈参数布局全面由 plan 驱动）

- **谱面删键**：`[abi.stack_args]`（`callee_base`/`caller_base`/`first_offset_slots`/`stride_slots`/`shadow_bytes`）整节删除，`Abi` 结构体 / schema / `isa-dsl.schema.json` / 文档键表同步。这五个键是**约定事实**，`AbiRules` 里本来就有对应（`shadow_bytes` + `stack.slot_bytes`/`stack.first_offset_slots`），谱里那份只是生成期烘死的常量。
- **调用方**（`arg_move_loop`）：栈参数 store 改成读 plan——偏移 `CallLayout::caller_offset(k)`（= `shadow + k×槽`）、帧需求用 `shadow_bytes`/`slot_bytes`；门控从"谱里声明过 shadow"换成"**本函数有调用布局**（`ctx.call_layout`）"，没有布局就明确 `Unsupported`（不再按谱面常量猜偏移）。基址恒为 `[machine.frame].sp`（`caller_base` 键随之下线）。
- **被调方**（`move_args`）：spill 槽收参那条路从谱面算式（`first_offset_slots×slot + shadow + (pos−n)×stride×slot`）改成读 **`ArgPlace::Stack { offset }`**；非 spill 的栈参数本来就走布局路径。
- **能力申报改由角色说话**：`has_stack_arg` 从"谱里声明了 `shadow_bytes`"改成"声明了 `roles = ["stack_arg_load"]`"。于是"有 load 缺 store"仍是**生成期**点名角色的错误，而"两个角色都没有"只是**不支持栈参数**（生成照旧成功，收参路径运行期 `Unsupported`）；`[machine].spill_scratch` 的要求随之挂在角色上。
- **规则侧补一条校验**（原先在谱面侧）：`shadow_bytes` 必须是槽单位的整数倍——键搬走时这条不变量不能丢。
- 守卫：`abi_target_real.rs` 的栈参偏移核对改名为 `stack_arg_offsets_match_the_x86_convention_numbers`（对照 `AbiRules` 的 2/32/8，不再是"对照谱面算式"）；`v12/tests.rs` 的角色用例改成 `stack_args_capability_is_declared_by_roles`（含"删掉 load 角色仍能生成"的反向对照）。
- **已知边界**（写进 `docs/reference/isa-dsl.md`）：调用方按"每个栈参数一个槽"计数，>8 字节的栈参数今天也不支持（float/vector 超出寄存器数时明确 `Unsupported`）。

### Changed (2026-09-27) — v20 A5-3：`[abi].call_clobbers` 与 `[abi].frame_padding` 两个约定键迁走

- **`[abi].call_clobbers` 删除**（riscv64/arm64 两谱；x86 本就没写）：调用点的破坏集已全面由 plan 驱动（`AbiPlan.clobbers` = 可用池 − callee-saved，经 `CallLayout.clobbers` 到 regalloc）。谱里那份降级为缺省空、已无人读。
- **`[abi].frame_padding` → `[machine].frame_padding`（机器事实）**：它由**这台机器的帧机制**决定——x86 是 `push fp` + 7 次 callee-saved 推入 = 8 次推入（偶数）而入口 `rsp ≡ 8 (mod 16)`，故须补 8；riscv/arm64 的 fp-inside 是显式 `sub sp`，故 0。它与 `[stack].align` 同源，归属机器事实。**约定侧**的同名字段仍在 `AbiRules::frame_padding`（win64/sysv64 都 = 8，进 plan）：管线优先读 plan，无 plan 才回退机器事实（与 `callee_save_slots` 同一套路）。
- 读取侧：`IsaModel::machine_frame_padding()`；生成物 `TargetABI::frame_padding()` 改读 `[machine]`；`Abi` 的 `frame_padding` 字段与 schema 的 `[abi]` 键一并删除 ⇒ 写旧键现在**明确报错**（不再有"两处都写、谁生效"）。
- 守卫：`abi_target_real.rs` 的交叉核对改成「plan 的 `frame_padding` == 机器事实 `[machine].frame_padding`」（4 份约定逐项核）。
- 验证：clippy `-D warnings` 0、workspace 串行全套绿、三条 JIT 矩阵不变。

### Changed (2026-09-27) — v20 A6/A5-3：[abi].callee_saved 迁移到 [machine]（键已删）

- **机器事实两组**：`[machine].callee_saved_gpr = [...]`（帧件会保存的那组 GPR；x86 = RBX/RDI/RSI/R12-R15、riscv64 = 11 个 s 系、arm64 = X19-X28）与 `[machine].callee_save_slots`（推入槽数，可由名单派生）。语义是"**这台机器的帧件会保存这组寄存器**"，与"某份约定**要求**保住哪些"（绑定/plan）分开。
- **读取侧单点化**：`IsaModel::machine_callee_saved()`（优先 `[machine].callee_saved_gpr`、回退 `[abi].callee_saved.gpr`）与 `machine_callee_save_slots()`；生成物 `TargetRegInfo::callee_saved()` / `callee_save_slots()` 都读机器事实，运行时 trait 的 `callee_save_slots()` 带缺省实现（手工后端无需改）。
- **删键**：三份发行谱的 `[abi].callee_saved` 段删除。**这一条曾两次实测崩溃**（`test_jit_call_indirect_wide_vector_byref`，`0xC0000005`）——**不是**"无 plan 回退缺名字"（那个说法已作废）：崩溃都发生在**半迁移的树**上，计数（`callee_save_slots` / 生成期 `__cs_bytes` = 7/64）已切到机器事实、名字（`TargetRegInfo::callee_saved()`）却还是空表 ⇒ 帧按 7 个槽布局、regalloc 却以为一个都没被保住，"计数 / 名字 / 消费者"三者不同源。名字与计数**同源**（都读 `[machine]`）之后删键即成立；全套跑完 `FORGE_TRACE_ABI=1` 打印 **0 条** `[abi-plan]`，即测试里没有一次 plan 失败、回退路径根本没被走到。
- 验证：clippy `--all-targets --all-features -D warnings` 0、workspace 串行全套绿、三条 JIT 矩阵与迁移前同值（x86 195/3、riscv64 131/67、arm64 23/175）。

### Changed (2026-09-27) — v20 A6：x86 序言/尾声改按运行时 callee-saved 列表发射

- **push 机制运行时化**：序言按 `alloc_result.callee_saved_to_save` 逐个 `push`，尾声 `sp -= n*槽` 取同一列表的运行时长度并逆序 `pop`。列表来自**约定数据**（有 plan 用 plan：显式选 `sysv64` 就只有 5 个，不是谱里那份 win64 的 7 个），无 plan 的夹具退回谱面表。
- **让位设计（这次能成的原因）**：帧内偏移都是编译期常量、按谱面表长算的，少推 Δ 个槽会让 rsp 抬高 Δ×槽而整体错位（上一轮实测：跨调用读垃圾值 + 访问违例，"只修 `frame_padding`"救不了）。这次由**管线把 Δ×槽补进序言实际分配的字节数**（`pipeline/emission.rs` 的 `cs_skipped`，仅 fp-outside）⇒ rsp 落点与静态表全长时代**逐字节相同**，所有常量与栈对齐继续成立。fp-inside 的保存是帧内槽、不动 rsp（`cs_skipped = 0`）。
- **两处字节级黄金同步**（`tests/integration_tests.rs`）：空 `AllocResult` 下新语义是"只有 `push rbp` + `mov rbp,rsp`"与尾声 `sub rsp, 0`；注释写清"管线会补 `cs_skipped`，直接调帧件时不会"。
- 验证：workspace 串行全套绿、clippy `-D warnings` 0、三条 JIT 矩阵不变。

### Added (2026-09-26) — v20 A6：FPR callee-saved 打通（AAPCS64 的 v8-v15 真的会被保存）

承接上一片的"能力"，这一片把它**接上**：

- **regalloc 两类都喂**：`RegAllocConfig` 拆出 `callee_saved_fpr`（主 FPR 类的编号空间），与 `callee_saved`（GPR 空间）分开——`X8` 与 `V8` 同号，合成一个集合会互相误判。有 plan 时两条都取自 plan（GPR = X19-X28、FPR = V8-V15），无 plan 的夹具退回谱面 GPR 表 + 空 FPR 表。
- **`callee_saved_to_save` 也扫主 FPR 类表**：只把**实际分配到**的 FPR 加进保存列表（与 GPR 侧同一口径），并按 `p.class.is_fp()` 区分同号寄存器。
- **fp-inside 的帧字节数按 plan 算**：帧顶槽位公式 `frame - fp_push - (k+1)*slot` 要求帧装得下**整张表**，而这张表随约定而变（AAPCS64 = 10 GPR + 8 FPR = 18 条）。plan 的表 ⊇ 实际保存集，所以按 plan 算必然够用；只按谱面的 GPR 表算会少 64 字节，多存的 8 个 FPR 会写到帧外。**fp-outside（x86）不动**（push 机制仍静态发谱面列表，理由见 `frame_layout.rs` 的注释）；fp-inside 的 `callee_saved_bytes = 0`，所以 spill 寻址不受影响。
- 守卫：`pipeline/frame_layout.rs` 新增单测——aapcs64 的 plan 必须是 10 GPR + 8 FPR，且 `min_frame = 16 + 18×8 = 160`（只按谱面表算会明显更小）；同时钉住 fp-inside 的 `callee_saved_bytes == 0`、`stack_slot_shift` 不随这条切换而变。
- 验证：workspace serially 绿（forge-codegen lib **1344** 用例）、clippy `-D warnings` 0、三条 JIT 矩阵不变（x86 195/3、riscv64 131/67、arm64 23/175，0 failed）。

### Added (2026-09-26) — v20 A6（前置）：角色声明带**寄存器类**限定，帧内保存按类分派

- **角色声明新增类轴**：`roles = [{ role = "callee_save", class = "fpr" }]`（`class = "gpr" | "fpr"`）。同一个"存到帧"能力在 GPR 与 FPR 上要用**不同指令**时分开申报；两个轴（`bits`/`class`）正交，裸声明与该类的声明**可以共存**——裸的那条是另一类的兜底（只有"同一个类声明两条"才是冲突，编译期报错）。唯一性键由 (角色, 位宽) 扩成 (角色, 位宽, 类)。
- **生成器按类解析**：新增 `role_name_for_class(role, class)`——类限定命中的声明优先，否则回退裸声明；`role_name(role)` 只看裸声明（写了限定的能力必须按轴解析，否则明确报错而不是静默取第一条）。
- **帧内保存/恢复循环按运行时寄存器类分派**：`callee_saved_loop` 在生成期解析出 GPR/FPR 两条指令，生成物在循环里按 `__preg.class.is_fp()` 选一条。两类解析到同一条指令（或只有一类申报）时**照旧发单条语句**——x86/riscv 与全部夹具的生成物**逐字节不变**（已用 `FGE_DEBUG_GEN` 与改动前的基线逐文件比对：只有 arm64 变了，+456 字节 = 多出的 STURD/LDURD 两支）。
- **arm64 谱**：`STURD`/`LDURD` 声明 `{ role = "callee_save"/"callee_load", class = "fpr" }`（`STURX`/`LDURX` 保持裸声明管 GPR）。AAPCS64 要求 v8-v15 低 64 位由被调方保存，而 `STURX` 只认 GPR——这一步把"能力"补上。
- **本片是前置能力，尚未激活**：regalloc 的 callee-saved 仍只喂 GPR（`callee_saved_to_save` 只扫主 GPR 类），所以 FPR 那支今天不会被执行；激活需要"喂 FPR + 帧最小字节数按实际保存数算"（下一步，见计划文档 A6）。
- 守卫：`crates/frontend/forge-isa-dsl/tests/role_widths.rs` 新增两条——"裸声明 + 类限定声明合法（用 arm64 真谱）"与"同一个类声明两条必须报错"。
- 验证：workspace serially 绿（clippy `-D warnings` 0）、三条 JIT 矩阵不变。

### Fixed (2026-09-26) — v20 A5-3（④-1）：约定事实改读 plan，顺带修掉两处约定口径错

- **`CallLayout` 补两个约定事实**：`clobbers`（可用池 − callee-saved）与 `frame_padding`，由 `abi_target::call_layout` 从 `AbiPlan.clobbers` / `AbiPlan.stack.frame_padding` 填。三处消费者改**优先读 plan**，谱里那份降级为无 plan（未注册约定/合成夹具）时的兜底：
  - **regalloc 的 callee-saved 集**（`compiler.rs`）：改用 plan 的 GPR 类项——同一台机器换约定，保存集必须跟着换。**只取 GPR 类**：AAPCS64 的 `cs_fpr`（V8-V15）已在 plan 里，但发射侧还没有按类分派的保存序列（`callee_save` 角色目前只指向 STURX），把 FPR 喂给 regalloc 会让它把跨调用值放进不被保存的寄存器——**类分派保存归 A6**，这条边界写在代码注释与计划里。
  - **调用点的破坏集**（`v12/codegen/lowering.rs` 的 Call/CallIndirect）：`ctx.current_clobbers` 优先取 plan 的**全类** `clobbers`（GPR + FP），无 plan 才用谱里声明的名单。
  - **帧填充**（`frame_layout.rs`）：`frame_padding` 优先取 plan（它是规则的字段、随约定而变）。
- **修掉一条向量宽度缺陷（第四条消费者切换时暴露，根因在另一处）**：把 plan 的**完整**破坏集交给发射后，x86 的 FP 侧从谱里声明的 4 个 XMM 变成全部 16 个（物理上正确——Win64 的 XMM0-15 全 volatile），`test_jit_v128_byval_return_lane3`（lane3 的 7.5 读成 **0**）与 `test_jit_v128_byval_mixed_int_pos` 随即变红。
  - 根因**不是**破坏集本身，而是**单结果 IR 值一律按"池宽"成类**（`pipeline/lowering.rs`）：V128 也拿 `FPR(8)`，而 `XReg::width()` 就是类宽、spill/reload 与 ABI 回读的搬运宽度都取自它 ⇒ 这个值一旦被 spill 就只搬 8 字节，lane2/3 静默丢。此前靠"幸运合并"（fixup 的 dst 与 src 同为 XMM0，MOVAPS 退化成空操作）掩盖。
  - **修法**：向量值按**真实字节数**成类（同一份 `reg_class_for` 的档位表 → `VEC(16/32/64)`），标量仍走池宽。修完后 plan 的**全类**破坏集可以直接消费（不再按类过滤），守卫 `x86_plan_clobbers_cover_the_whole_fp_file` 钉住"plan 的 FP 破坏集覆盖全部 XMM"这一事实。
- **顺带修掉的真错（两处，均由新增的交叉核对测试暴露）**：
  1. **`sysv64` 规则缺 `frame_padding`**（缺省 0）。SysV 的入口 `rsp ≡ 8 (mod 16)`，而本实现的序言是 `push fp` + 5 个 callee-saved = **偶数次 push** ⇒ `sub rsp` 前 `rsp ≡ 8`，必须补 8 字节才能在 call 点回到 16 对齐（与 Win64 同值、同理由）。规则补 `frame_padding = 8`，黄金快照 `sysv64.plan.txt` 的 `frame_pad 0 → 8`。
  2. **x86 显式选 `sysv64` 时破坏集漏 RDI/RSI**：谱里的 `[abi]` 是 **win64 口径**（RDI/RSI 是 callee-saved），拿它编 sysv64 的函数时跨调用存活值留在 RDI/RSI 上会被 callee 静默覆盖。改读 plan 后破坏集随约定切换（sysv64 = 7 个含 RDI/RSI，win64 = 5 个）。
- **刻意保留（写进注释与计划）**：帧上方 **callee-saved 字节数**仍按**谱面**列表数——x86 的 push 机制是静态发射（谱面列表逐个 `push`），改按 plan 计数会在"plan 比谱表短"（sysv64：5 vs 7）时少算 16 字节，让局部/spill 槽与 push 槽重叠。要一起换须先把 push 机制改成运行时按 `alloc_result.callee_saved_to_save` 循环（帧字节数也随之变运行时值）——归 A6。
- **新增交叉核对测试**（`crates/backend/forge-codegen/tests/abi_target_real.rs`，4 份约定）：`x86_win64`/`x86_sysv64`/`riscv64_lp64d`/`arm64_aapcs64` 逐项核对「plan 的 callee-saved vs 谱表」「plan 的 frame_padding vs 谱」「clobbers 的两条硬不变量（与 callee-saved 不交；参数寄存器与返回寄存器都在里面）」。相等的那三份断言**逐项同值**，sysv64 断言的是**预期差异**（plan ⊂ 谱表且严格更小）——这是"把三处消费者切到 plan"的入场券。
- 验证：workspace 全套 serially 绿、clippy `--all-targets --all-features -D warnings` 0。

### Added (2026-09-26) — v20 A5-3（②b）：`[machine.frame]` 帧形状，三份发行谱迁完

- **`[machine]` 增 `frame` 子表**（`sp`/`fp`/`layout`/`fp_push_bytes`/`alloc_neg`，键与旧 `[abi.frame]` 同名同义）：这五项是"这台机器怎么建帧"的**机器事实**，不是约定内容（保存谁、栈参数怎么排才是约定）。读侧统一走 `IsaModel::machine_frame()`——`[machine.frame]` 优先、回退 `[abi.frame]`，**所有**读者（含只取 `fp` 或只取 `fp_push_bytes` 的）都经它，避免迁移期"一半读新表、一半读旧表"的分裂。
- **读者改写**（全部改为 `machine_frame()`）：`abi_view` 的 pinned 视图、`integration.rs` 的 sp/fp 解析 + 帧是否声明、`frame.rs` 的 layout/fp_push_bytes/spill 缺省 base/栈参数收参基址、`lowering.rs` 的 `frame_base_toks`/`sp_base_toks`（sret/by-ref/栈参数 store 的基址）、`validate.rs` 的 sp 非空与寄存器名检查。错误消息与文档字符串同步改点名 `[machine.frame]`。
- **谱**：`isa/{x86,riscv64,arm64}_v12.toml` 的 `[abi.frame]` 段移入 `[machine.frame]`（值原样）；两份夹具 `crates/backend/forge-codegen/tests/isa/{demo,demo8}.toml` 与 DSL 内联夹具（`frame_sp_*` 两条反向用例）一并迁到新键——它们现在**真的走新路径**，回退路径只服务外部旧谱。
- **三方守卫**：`schema.rs` 增 `[machine.frame]` 节（`sp` 必填）与 `[machine].frame` 键、`docs/reference/isa-dsl.md` 键表与 `## [machine.frame]` 一节的迁移说明、重新生成的 `isa-dsl.schema.json`；`docs/reference/calling-conventions.md` 的能力视图来源改述。（`[abi.frame]` 仍在 schema/校验里，作为迁移期回退路径。）
- 验证：workspace 全套 serially 绿（130 个测试二进制 0 失败）、clippy `--all-targets --all-features -D warnings` 0；本地与 CI run 213（`983b8ee`）一致——CI 11 项里 10 项绿，唯一红的是既有慢性项 `forge-rustc (e2e, Windows)`。

### Added (2026-09-26) — v20 A5-3（①+②）：`[machine]` 机器事实层，三份发行谱迁完

- **新增 `[machine]` 段**（只描述"这台机器是什么样"）：`fixed_regs`（regalloc 不可分配，原 `[abi].reserved`）、`spill_scratch`（溢出与栈参数收参的临时寄存器，原 `[abi].scratch`）、`link_reg`（call 写返回地址的寄存器，原 `[abi].call_ret_reg`）。键名**故意与旧键不同名**——两处都写时"谁生效"必须无歧义；`[machine]` 优先、回退 `[abi]` 旧键，所以迁移期两种写法都认，可以逐谱迁移。
- **读取侧单点化**：所有读者改走 `IsaModel::machine_scratch()/machine_reserved()/machine_link_reg()`——`abi_view` 的静态能力视图、`integration.rs` 的 `RegInfo` scratch/reserved、`frame.rs` 的栈参数收参 scratch + 帧件保存 link、`lowering.rs` 的 `call`/`call_indirect` 返回地址槽。`validate` 对 `[machine]` 与 `[abi]` 两处都校验寄存器名（必须落在某个 `[reg.*]` 组内）。
- **三条 fail-closed 错误消息改点名新键**（`[machine].spill_scratch` 未声明却要走栈参数收参、`[machine].link_reg` 未声明却有 Out/InOut Reg 槽），两条负向用例（`codegen_requires_scratch_for_stack_args` / `codegen_requires_call_ret_reg_when_call_has_ret_slot`）同步改成新键——它们仍钉着"不按某个 ISA 的寄存器名兜底"。
- **三份发行谱迁移完毕**（`isa/{x86,riscv64,arm64}_v12.toml`）：`[abi].scratch`/`reserved`/`call_ret_reg` 删除，值**原样**进 `[machine]`（x86 `R10/R11`、riscv `X5/X6` + `X0/X1/X3/X4` + `X1`、arm64 `X16/X17` + `X18/X30` + `X30`）。
- **三方一致守卫同步**：`schema.rs`（根键 + `[machine]` 节）、`docs/reference/isa-dsl.md` 键速查表与新增 `[machine]` 一节、重新生成的 `isa-dsl.schema.json`（`schema_guard` 6 项全绿）。
- 验证：workspace 全套 serially 绿（clippy `--all-targets --all-features -D warnings` 0）；两条 JIT 矩阵与迁移前**逐条同值**——x86 **195 passed / 3 skipped**、riscv64 **131 passed / 67 skipped**，0 failed（scratch/reserved/link 三条路径都被矩阵跨调用用例实际走到）。
- 仍待做（A5-3 ③/④）：约定事实（`arg_class`/`ret_regs`/`arg_slot`/`stack_args`/`callee_saved`/`call_clobbers`/`frame_padding`）移入 `AbiRules`/`AbiBinding`、demo 夹具补测试本地绑定、发射/管线改读 `AbiPlan`，然后删除 `[abi]` 的约定键与本次保留的回退路径。

### Added (2026-09-25) — v20 A5（续）：AAPCS64 的 v8-v15 进 callee-saved（引擎侧）

- **规则/绑定**：`aapcs64` 的 `callee_saved.pools` 加 `cs_fpr`，绑定补 `cs_fpr = ["V8".."V15"]`。
  效果：`abi plan` 的 `callee_saved` 现在含 `V8..V15`，`clobbers` **不再**把 v8-v15 列为被破坏
  （AAPCS64 要求被调方保它们低 64 位）。黄金快照 `aapcs64.plan.txt` 随之更新。
- **如实标注的边界**：这一步只在**引擎/计划侧**成立。**发射侧还没按寄存器类分派保存指令**
  ——`callee_save` 角色目前唯一指向 `STURX`（GPR 视图），而 regalloc 的 callee-saved 列表来自
  谱里的 `[abi.callee_saved]`（不含 FPR），所以**行为不变、且偏保守**（regalloc 仍把 v8-v15
  当跨调用不安全）。把帧件的保存/恢复按类分派（FPR 走 STURD/LDURD）需要 role 能带类限定，
  归 A6；缺口已写进绑定注释与计划文档，不留含糊。

### Added (2026-09-25) — v20 A5（子集）：arm64 浮点能力——FPR 组 + 浮点搬运角色 + 绑定补池

- **谱**（`isa/arm64.toml`）：新增 `[reg.fpr8]`（`V0..V31`）、`fpr` 操作数槽、`FMOVR` 形式，以及 6 条指令——`FMOV_S`/`FMOV_D`（`fpr_mov` 32/64）、`LDURD`/`STURD`/`LDURS`/`STURS`（SIMD&FP 的 unscaled 访存，编码与 `LDURX`/`STURX` 同形：`0xFD…`/`0xBD…` + `opc2`），外加 `[spill.FPR]`（D 寄存器溢出）。
- **绑定**（`conventions/aapcs64-arm64.toml`）：补 `float = ["V0".."V7"]`、`ret_float = ["V0".."V3"]`（HFA ≤4 返回落连续 v0-v3），并删掉"arm64 没有 FPR 组"的缺口说明。
- **效果**：`forge-isa abi check --strict isa/arm64.toml` 的缺口 **6 → 1**（只剩 HFA4 *返回*搬运那条"≥3 槽见 A6"的引擎侧限制）；`abi plan` 现在给出 `f64 → V0`、`i64 → X0`、`ret → V0`（此前是 `MissingPool`）。arm64 的浮点/HFA **参数**与 ≤2 槽返回自此有寄存器可落，发射侧也拿得到 `fpr_mov` 角色（收参走 `FMOV_S`/`FMOV_D`）。
- **刻意保留的取舍/缺口**（如实登记）：① FP 寄存器统一命名 `V0..V31`（不像 x86 那样名字自带宽度），因此 `fmov v0, v1` / `ldur v0, [x29,#8]` 的 S/D 两种编码**汇编文本相同** ⇒ 反汇编按声明序取 S；这条已知歧义写进了 `spec_coverage_guard.rs` 的钉死名单；② 暂无 `cs_fpr` 池 ⇒ `clobbers` 保守地把 v0-v31 全列为被破坏（安全方向）；③ arm64 的浮点**算术**尚未接线（矩阵里的 F64 用例仍按 ops 未覆盖 skip）。
- **守卫同步**：`spec_coverage_guard`（arm64 指令总数 104 → **110**、歧义名单 +6）、`isa_roundtrip_guard`（arm64 派生条目 332 → **360**）、`forge-abi` 黄金快照 `aapcs64.plan.txt`（clobbers 含 V0-V31）；合成目标 `common::arm64()` 随之带上 FPR 组，`missing_pool_is_explicit_not_silent` 改为**自备缺池绑定**（测"缺池的报法"，不依赖 arm64 有没有池）。

### Added (2026-09-25) — v20 A3b-2b-2c-2：栈落点接进布局驱动的收参

- `@move_args` 的布局路径新增 `ArgPlace::Stack { offset }` 分支：栈参数从
  `[frame_base + offset]` 收进分配的寄存器（指令仍按角色 `stack_arg_load` 取），
  **偏移完全来自 forge-abi**（`first_offset_slots + shadow + k×slot`，上一片刚修正）。
  `__layout_ok` 的入场判定也随之接受 `Stack` ⇒ 带栈参数的函数（第 5+ 个参数）不再因为
  "有一个栈落点"而整函数退回 `[abi]` 位置算式——它们的寄存器参数也走布局。
- 被 regalloc 强制 spill 的栈参数仍由既有的 spill 收参块处理（同一套数值；其"布局偏移 ==
  旧算式"由 `abi_target_real::stack_arg_offsets_agree_with_the_legacy_formula` 钉住）。
- **浮点参数走栈**的收参**明确拒绝**（`Unsupported`）而不是当整数搬进 GPR：旧路径同样只走
  GPR 搬运，这是这条路上的旧缺口，接它属于后续（需要按宽度分派的 FPR load）。
- 仍未接：`Pair`/`Group`/无指针的 `Indirect`（整函数回退，边界在守卫测试里）。
- 验证：workspace 全套 serially 绿（含两条 JIT 矩阵；`Args` 用例覆盖 5+ 参数）。

### Fixed (2026-09-25) — v20 A3b-2b-2c-1：`Placement::Stack.offset` 漏算 shadow（被调方读栈参数会读错）

- **模型 bug**：被调方视角的栈参数偏移只算了 `first_offset_slots × slot` + 参数区位置，**漏了 `shadow_bytes`**。调用方把第 k 个栈参数写在 `[sp + shadow + k×slot]`，被调方读同一实参时要加上"返回地址 + 保存的帧指针"**以及** shadow——win64 第 5 个参数因此被算成 `off=16`，而现有发射算式是 `48`（差的就是那 32 字节 shadow）。
- **发现方式**：`forge-codegen/tests/abi_target_real.rs::stack_arg_offsets_agree_with_the_legacy_formula`（新增）——把布局的 `Stack { offset }` 与**现有发射路径**的 `[abi.stack_args]` 算式逐参数对照。这条测试现在是"把栈参数收参切到布局"的入场券：偏移差一不是崩而是**静默错值**，矩阵未必抓得到。
- **修法**：`engine::stack_place` 的落点偏移加上 `rules.shadow_bytes`，并把 `Stack.offset` 的语义（被调方视角、已含 shadow）与两个视角的固定关系 `Stack.offset(k) − caller_offset(k) = first_arg_offset` 写进参考文档与代码注释。
- **影响面**：只有 `shadow_bytes != 0` 的约定受影响——`win64` 黄金快照的栈偏移整体 +32（`sysv64`/`aapcs64`/`lp64d` 的 shadow 为 0，**逐字节不变**）；`invariants.rs` 的"参数区装得下最后一个字节"随之按新口径断言；workspace 全套 serially 绿。
- 发射侧**尚未消费** `Stack` 落点（栈参数收参仍走 `[abi.stack_args]` 算式）⇒ 运行时行为不变，`Stack` 接进布局路径是 A3b-2b-2c-2。

### Changed (2026-09-25) — v20 A4：序/尾声由生成器生成，伪指令全部删除（谱只剩裸指令）

- **设计裁定（用户）**：谱只描述**裸指令**（形状 + 编码 + 能力角色），函数调用平衡（保存谁、帧多大、怎么建立帧指针）交给**调用约定**层；角色是"这条指令能充当什么"的**能力声明**，不是给指令加的副作用。
- **删除**：`[emit.prologue]`/`[emit.epilogue]` 与四个伪指令（`@push_callee`/`@pop_callee`/`@frame_alloc`/`@frame_free`；`@move_args` 更早已删）。`[emit]` 只剩 `align_pad`/`epilogue_label` 两个机器事实。写回去会被明确拒绝（`DSL-TOML`：新键不存在），并有反回潮守卫 `crates/frontend/forge-isa-dsl/tests/call_layout_emission.rs`（三谱 + 夹具里不许再出现这些键/伪指令）。
- **新增三个能力角色**：`frame_set`（`in`=源、`out`=目标，可带 imm：x86 `mov rbp, rsp` / `mov rsp, rbp`、riscv `addi x8, x2, frame`、arm64 `addimmx x29, sp, frame`）、`callee_save` / `callee_load`（`Reg[0]`=值、`Reg[1]`=基址、`Imm[0]`=偏移：riscv `SD`/`LD`、arm64 `STURX`/`LDURX`）。复用 `push`/`pop`/`frame_alloc`/`frame_free`/`ret`。
- **生成器的规范序列**（顺序由**机制**决定）：push 机制（x86）= `push fp` → `frame_set(fp←sp)` → 逐个 `push` callee-saved → **收参** → `frame_alloc`；尾声 = `frame_set(sp←fp)` → `sp -= callee-saved 区` → 逆序 `pop` → `pop fp` → `ret`。帧内机制（riscv/arm64）= `frame_alloc` → 存 link（`[sp+frame-8]`）→ 存 fp（`[sp+frame-fp_push]`）→ `frame_set(fp←sp+frame)` → 逐个 `callee_save`（`[sp+frame-fp_push-(k+1)*slot]`）→ **收参**；尾声镜像 + `frame_free` + `ret`。三条不变量：保存早于收参、帧内保存槽只在分配之后写、尾声与序言同源（逆序恢复）。
- **缺角色的步不发射**（角色 = 能力申报）；`ret` 例外：尾声**必须**有 `roles = ["ret"]`，否则明确报错。
- **验收 = 三份发行谱的生成物逐字节不变**：`FGE_DEBUG_GEN=1` 对照改动前后，x86/riscv64/arm64 生成模块 **SHA256 全同**（序/尾声的每一条指令、字段顺序、立即数形态都对得上）；workspace 全套 serially 绿（含两条 JIT 矩阵）。
- **两处有意为之的行为变化**：① demo 夹具声明了 `frame_alloc`/`frame_free` 却从没分配过帧 —— 现在按声明发射（`frame_size == 0` 时守卫不发，故测试行为不变）；② x86 尾声的 `sp -= callee-saved 区` 仍是静态表长（与 push 侧同源），"少保存时错位"的隐患留在 A3b-2b-2c/后续统一。
- **代价**：谱再也无法给函数插入任意序言步骤（vzeroupper/栈探测/GOT 建立）——需要时**加角色**（上层可见的能力），不回到自由模板。

### Changed (2026-09-25) — v20 A3b-2b-2b：`@move_args` 退役（收参由生成器发射，谱不再定义调用约定）

- **伪指令白名单里删掉 `move_args`**：谱里再写 `@move_args` 会被**明确拒绝**并给出迁移提示（单列一条诊断，不是含糊的"未知伪指令"）——它是**调用约定**的内容，谱不该写它。
- **生成器自动插入收参**：位置 = 最后一个 `@push_callee` **之后**（模板里没有 `@push_callee` 则放最后），两者都在所有 callee-saved 保存之后。**顺序不是风格问题**：保存若晚于收参，存下来的是实参值而不是调用者的寄存器值，尾声恢复会毁掉调用者的寄存器。
- `[emit.prologue]` 因此**可以缺席**（`gen_emit_block` 不再因模板缺失而整体早退）：缺席 = 空模板 + 收参；demo 夹具的整段 `@move_args` 模板随之删除。
- 三份发行谱（x86/riscv64/arm64）与两份夹具的模板里删掉 `@move_args`；其余四个伪指令（`@push_callee`/`@pop_callee`/`@frame_alloc`/`@frame_free`）仍在，A4 才轮到它们。
- **验收 = 生成物逐字节不变**：`FGE_DEBUG_GEN=1` 对 10 份生成模块（三份发行谱 + 七份夹具/多文件谱）取样，改动前后 **SHA256 全部相同**；workspace 全套（serially）绿。
- 守卫 `forge-isa-dsl/tests/call_layout_emission.rs` 新增三条：写 `@move_args` 必报错且提示指路、缺席序言模板的谱仍发射收参、收参位置在保存之后且帧分配之前。

### Added (2026-09-25) — v20 A3b-2b-2a：缺省约定 `c` 的绑定 + 收参改读调用布局

- **前提缺口**：IR 的缺省约定是抽象名 `c`，而内置绑定只有 `win64`/`sysv64`/`aapcs64`/`lp64d` ⇒ `c` 在真机上没有绑定 ⇒ `call_layout` 恒为 `None`、生成物的布局路径永远不生效。补法是把它变成**数据**：`AbiRules` 新增 `aliases = ["c"]`（"这台机器的 C 就是本约定"），内置 `win64`/`aapcs64`/`lp64d` 声明、`sysv64` 刻意不声明。**关键在"整套代答"**：解析出的必须是同一份约定的**规则 + 绑定**（`AbiRegistry::resolve_conv`）。实测教训——只让**绑定**代答（拿 `c` 自己的通用规则配 Win64 的寄存器）会得到 by_class 的槽位与 RCX 返回：混合 int/float 参数读错、`sret` + by-ref 槽位错位，三个 JIT 用例当场红。纪律：同一机器上两份约定都声称代答 ⇒ 报错（不按注册序猜）；显式 `(ISA, "c")` 绑定/规则优先于别名（宿主覆写入口）。
- `@move_args`（被调方收参）改读 `AllocResult::call_layout`：来源寄存器取自布局（`ArgPlace::Reg` 的 **(类, 类内号)**——int 类走 `gpr_mov`、浮点/向量类按类宽分派 `vec_mov`/`fpr_mov`），宽向量 by-ref 走 `ArgPlace::Indirect`。生成器不再自己数"第几个 int 槽"、不再自己算 `sret` 偏移（这两件事现在全由绑定/规则决定）。
- **入场判定 `__layout_ok`**：只有**每个**形参都落在本片覆盖的落点时才启用；有一个不支持（`Pair`/`Group`/`Stack`/无指针的 `Indirect`）就**整函数**退回既有 `[abi]` 路径（两条路径不混用——混用会让旧路径的 `__gi`/`__fi` 游标错位）。`call_layout = None`（无绑定/规划失败）同样退回。
- 守卫与实测：新增 `forge-isa-dsl/tests/call_layout_emission.rs`（三份发行谱都发射布局路径、且排在旧路径之前、只按寄存器**类**分派）；`abi_target_real.rs` 新增"缺省 `c` 也能规划并到达帧件"；**x86 矩阵 195 passed / 3 skipped / 0 failed**、**riscv64 矩阵 131 passed / 67 skipped / 0 failed**、`forge-codegen` 全套绿 ⇒ 收参换来源后行为不变。
- 仍未切：栈参数 load/store、`byval` 副本、序尾声（`@push_callee`/`@frame_alloc`/`{callee_saved_bytes}`）仍读 `[abi]`（A3b-2b-2b / A4）。

### Added (2026-09-25) — v20 A3b-2b-1：序/尾声侧拿到调用布局（`AllocResult.call_layout`）

- **结构事实**：`TargetFrameLowering::emit_prologue/epilogue` 只拿到 `frame_size` + `AllocResult`、拿不到 `LowerCtx`，而 `@move_args`（收参）是生成 lowering 的一部分、有 `LowerCtx`。所以"生成物改读 plan"分两条通路，序/尾声这条必须先把布局放进 `AllocResult`。
- `AllocResult` 新增 `call_layout: Option<CallLayout>`（默认 `None`），管线在收尾处从 `LowerCtx::call_layout` 填入；`None` = 没接上/算不出 ⇒ 帧件走既有 `[abi]` 路径。
- 测试（`tests/abi_target_real.rs`）：钉住管线确实把布局带到帧件（conv/shadow/首个实参 `RCX=(GPR(8),1)`/callee-saved 非空）。
- 发射仍未切换 ⇒ 生成物逐字节不变。

### Added (2026-09-25) — v20 A3b-2a：中性调用布局 `machine::call_layout`（发射切换的地基）

- `forge-isa-runtime` 新增 `machine::call_layout`：把"一次调用长什么样"变成**中性数据**（`CallLayout`：逐参落点 / 返回 / 隐藏 sret / 栈区尺寸 / callee-saved / 被叫方弹栈 / 红区 / 扩展位数）。寄存器用 **(类, 类内号)** 表示（`RegClass` 是 forge-ir 的中性类型），生成物 `Reg::from_index(i, class)` 即可还原——**运行时因此不需要依赖 forge-abi**。
- `forge_codegen::pipeline::abi_target::call_layout(plan, machine)`：`AbiPlan` → `CallLayout`（ABI 空间号折回类内号、`sret` 标记打到落点上）；管线把它塞进 `LowerCtx::call_layout`。
- 测试（`tests/abi_target_real.rs`，真机 x86）：RCX=(GPR(8),1)、XMM1=(FPR(16),1)、返回 RAX=(GPR(8),0)、callee-saved 含 RBX=(GPR(8),3)、`caller_offset(k)=shadow+k*slot` 全部核对。
- **发射尚未切换**（生成物读的仍是 `TargetABI`）⇒ 生成物逐字节不变；这一步是"生成物改读 plan"的地基。

### Added (2026-09-25) — v20 A3b-1：调用计划接进编译入口（只算不用，行为不变）

- `CompileState` 在编译入口用 A3a 的宿主适配器把该函数的 `AbiPlan` 算出来挂上（`abi_plan`/`abi_plan_note`）；算不出来**不阻断编译**（发射还没用它），原因留档。`FORGE_TRACE_ABI=1` 打印计划的确定性文本或失败原因——发射尚未切换期间这是"计划长什么样"的唯一证据面。
- **前置核对**（`tests/abi_target_real.rs` +1）：引擎的计划必须与**当前**发射路径的 `AllocResult` 一致——sret 有无、**逐参数** by-ref 判定、栈参数区字节数（`arg_area − shadow`）。这条一致性检查是把发射切到 plan 上的入场券：不一致就说明"适配器/数据/现有路径"有一边错了。
- 本片**不改发射** ⇒ 生成物逐字节不变（`cargo test --workspace` 全绿、clippy `-D warnings` 干净）。

### Added (2026-09-25) — v20 A3a：宿主适配器（真实后端 → 引擎），ISA 正式申报能力

- **`TargetMachine::role_bits(role) -> Option<u16>`**（`forge-isa-runtime` trait，默认 `None`）：ISA 向宿主**申报能力**的出口。DSL 从谱里的 `[[instructions]].roles` 生成 `match role { "gpr_mov" => Some(64), … , _ => None }`（按能力名折算，与 `forge-isa abi check` 的静态视图**同源**——都走 `abi_view::role_capability`，因此"谱里写了什么"与"宿主申报了什么"不会漂移；生成物里不留名字表 + 线性查找）。
- **`forge_codegen::pipeline::abi_target`**：`MachineAbiTarget` 把真实 `TargetRegInfo` 喂给 `forge-abi` 引擎（索引空间 GPR 区 + FP 区、名字用生成枚举的变体名、`pinned` = 不在可分配表、能力走 `role_bits`），并提供 `plan_for_function`/`plan_for_signature`（IR 签名 + 约定数据 → 真机 `AbiPlan`）。
- **交叉核对**（`tests/abi_target_real.rs`，3 条）：真机 x86 上 `win64` 的同一签名给出与 A1 合成目标**相同的落点**（第 1 个整数 RCX、第 2 个浮点 XMM1、标量返回 RAX、shadow 32、callee-saved/clobber 来自真表），未注册约定在真机上同样 fail-closed——即"适配器 + 内置绑定 + 引擎"三方对得上。
- 说明：本片**只算不算用**（`AbiPlan` 供校验与诊断），发射仍走老路 ⇒ 现有编译行为不变；按 plan 发射调用点/入口/序尾声是 A3b（验收：x86 生成物逐字节不变）。

### Added (2026-09-24) — v20 A2b：声明属性（`byval`/`sret`/`inreg`/`zeroext`/`signext`/`align`）真的改变规划

- **引擎吃声明属性**：`forge_abi::Signature` 新增 `attrs`/`ret_attrs`（`DeclAttrs`），`plan_fn` 按属性改分类与落点——`byval(N)` → 该形参变"调用方栈上 N 字节副本 + 指针"（副本进 `StackLayout::byval_area_bytes`）、`sret` → 该形参占约定声明的 **hidden sret 槽**（x86 RCX/RDI、AAPCS64 **x8**、riscv a0；不再按普通参数分类）、`inreg` → 分类说要走栈时再试寄存器（池空仍走栈，不硬凑）、`zeroext`/`signext` → 落点带 `Extension`（都写时 `signext` 胜，与 LLVM 一致）、`align(N)` → 栈落点对齐抬到 N。
- **IR 侧接上**：新增 `forge_codegen::pipeline::sig_view`——`Function::param_attrs`/`ret_attrs` + `FunctionSignature` + `TypeStore` → `forge_abi::Signature`。`ParamAttributes`（`byval`/`sret`/`inreg`/`zeroext`/`signext`/`align`…）此前**只有文本层认识、没有任何代码读**（与刚删掉的旧 `CallConv` 一样是装饰），现在它是规划的输入。类型投影只摊开不猜测：大小/对齐取 `TypeStore` 的权威值（**带填充的结构体不能按成员求和**），摊不开的类型（可扩展向量）落 `TyKind::Other` 交给规则兜底。
- 测试：`forge-abi/tests/invariants.rs` +2（`declared_attributes_change_the_plan` 逐条断言产物：`byval` 变间接落点且副本区按 N 算、`sret` 在两个约定上各就各位且不与后续参数撞号、两个 ext、`align` 抬对齐；`inreg_overrides_a_stack_classification` 覆盖"池够"与"池耗尽"两种行为）；`forge-codegen/tests/conv_registry_read_path.rs` 的投影用例（含"带填充结构体大小"这条反例）。`AbiPlan::to_text()` 只打印**产物**（`ext=` / `on_stack=` / `align=`），所以"属性生效没有"在快照里一眼可见。
- 下一步（A3）：宿主 `AbiTarget` 适配器（`TargetRegInfo` → 引擎）把签名真正跑成 `AbiPlan`，并按 plan 发射调用点/入口/序尾声（x86 优先、生成物逐字节不变为验收）。

### Changed (2026-09-24) — v20 A2：IR 里的调用约定从"死枚举"变成"使用者命名的标识 + 宿主注册表"（破坏性）

- **删掉 16 变体的 `CallConv`，换成 `CallConvId`**（`forge-ir`）：`Builtin(ConvName)`（`c`/`sysv64`/`win64`/`aapcs64`/`lp64d`）、`Named(ImmStr)`（**使用者自己注册的名字**，开放集合）、`Index(u32)`（LLVM `cc N`）。默认 = `Builtin(C)`，文本层对 `c` 不写关键字（与 LLVM 一致）。旧枚举里 `SystemV`/`Fast`/`Cold`/`PreserveAll` 那些变体既不是公共约定、也没有任何代码读——那是"用枚举假装支持一切"。
- **未知约定不再被折成不可还原的占位值**：文本层原先把 `amdgpu_cs_chain` 这类名字编码成 `Custom(len ^ 0x8000_0000)`（既打印不回原样、也没人读），现在 `Named(ImmStr)` 原样保留；`to_text`/`from_text` **共用一张表**（`win64cc`/`fastcc`/`cc 42` 等 LLVM 拼写 = 同一份数据），往返不会漂移。
- **新增真实读路径**（旧实现 `ctx.call_conv` 只写不读，等于文档）：`forge-codegen::pipeline::conv_registry` 是宿主注册表（内置五份 + `register_rules_toml`/`register_binding_toml` 加自己的），编译入口 `CompileState::new` 把 `CallConvId` 解析成注册表键（`Index(n)` → `"cc{n}"`）落在 `LowerCtx::call_conv_name`；**未注册一律 fail-closed**（报 `Unsupported` 并列出已注册的名字，**不**退回缺省约定——`CallConv::Default` 就是这么变成死值的）。A3 起同一个名字用来查 `AbiRules`/`AbiBinding` 发射调用点。
- **二进制格式升到 `IR_FORMAT_VERSION = 3`**：签名体里那 1 字节判别值改成 tag（`0..=4` 内置 / `5` 命名（字符串表下标）/ `6` 数值（varint））；`c` 仍是 1 字节，因此字节偏移类的手工解码测试不受影响。**无兼容读取**（设计前提就是破坏性更新）：v2 字节流在头部即报版本不符；文档 `docs/reference/binary-format.md` 同步（标题 v3 + 版本历史表）。
- 测试：`crates/backend/forge-codegen/tests/conv_registry_read_path.rs`（内置五份可解析；未注册的 `Named`/`Index` 都 fail-closed 且消息能指路；**端到端**"未注册时编译失败 → 注册后同一份 IR 编译通过"）。`forge-ir` 的文本/二进制/往返测试全部因共用表而继续绿（`fastcc`/`win64cc`/`cc 10` 的原样往返）。
- 说明：`forge-rustc` 从不引用这个类型（前端也从没填过它）——这正是"死值"的另一半原因；A2b 会让前端把 `PassMode` 落到 IR 属性上。

### Added (2026-09-24) — 调用约定层 `forge-abi`（v20 A1：约定是使用者的数据，ISA 只申报能力）

- **新 crate `crates/foundation/forge-abi`**（无内部依赖）：把调用约定拆成三层数据 + 一个通用引擎——`AbiRules`（约定，平台无关的 TOML，可继承）→ `AbiBinding`（`(ISA, 约定)` 的寄存器绑定）→ `AbiPlan`（引擎产物，**调用方与被调方共用**：每个实参/形参的落点、返回值、栈布局、callee-saved、hidden 槽、clobber）。内置约定 5 份（`c` 抽象基类 + `win64`/`sysv64`/`aapcs64`/`lp64d`）与参考绑定 4 份（`crates/foundation/forge-abi/conventions/*.toml`，含"为什么这么绑"的注释）。引擎只认注册表里注册过的约定名，未注册 ⇒ 明确报错（**不**退回缺省——旧设计里 IR 的 `CallConv::Default` 就是这样变成死值的）。
- **修掉四处会写错值的模型缺口**（都是"内置数据跑起来"才暴露的，`cargo check` 看不见）：① 同质浮点聚合（HFA）判定不分整数/浮点 ⇒ `struct{i64,i64}` 会进浮点寄存器（RISC-V 应走 a0:a1）；② HFA 槽数写死 ⇒ `struct{f32}` 白吃 4 个寄存器、把后面的参数挤到栈上，改为 `slots = "hfa"`（按类型取成员数）；③ 参数位与返回位共用分类规则 ⇒ AAPCS64 的 >16B 聚合"当参数是 byval、当返回是 **x8** sret"被混成一条，新增 `ret_classify`（非空即独占）；④ 返回寄存器与参数寄存器共用池（x86 返回在 RAX、参数从 RCX 起）⇒ 拆成 `ret_int`/`ret_float` 两套池 + 两个独立游标（按位置计数时一个 2 槽返回不再把第一个参数挤走）。
- **另修三处**：byval 副本改在**独立的调用方临时区**（`StackLayout::byval_area_bytes`；混进传出参数区会与栈参数抢内存）、`ClassRule` 的 TOML 键 `do` 缺 `#[serde(rename)]`（会让 5 份内置约定整片解析失败，`cargo check` 发现不了）、新增 `Placement::RegGroup`（AAPCS64 的 4×f32 HFA 需要 4 个连续槽；返回位本片明确 `Unsupported`）。
- **ISA 侧能力视图 `forge_isa_dsl::abi_view`（新公开模块）**：从谱读"这台机器有什么"——寄存器表（GPR 区 + FP 区编号与 `TargetRegInfo::num_gp_regs`/`num_fp_regs` 对齐，别名解析到同一物理号）、固定用途寄存器（`[abi].reserved` + sp/fp）、链接寄存器（`[abi].call_ret_reg`）、角色→能力。**不读调用约定**（那是使用者的数据），也不暴露整个模型。
- **CLI `forge-isa abi list|check|plan`**：新增 `abi` 子命令与 `forge-abi` 依赖。`abi check <谱>` 用能力视图 × 内置绑定跑 15 条代表签名，**硬错**（`UnresolvedReg`/`BadRules` = 名字或约定写错）退出 1、**缺口**（`MissingPool`/`Unsupported`/`CapabilityGap` = 这台机器做不了，fail-closed）默认只报 `⚠ GAP`、`--strict` 才影响退出码；`abi plan` 打印一份 `AbiPlan`（确定性文本）。**实测**：x86 `win64`/`sysv64` 与 riscv64 `lp64d` 各 15 条**全绿**；arm64 `aapcs64` **6 条缺口**（谱里没有 FPR 寄存器组 ⇒ `float`/`ret_float` 池缺 ⇒ 浮点/HFA 无寄存器可落，与矩阵 175 条 skip 同源，A5 关闭）。
- 测试：黄金快照 4 份（四约定 × 21 条语料，`FORGE_ABI_BLESS=1` 重新生成）+ 不变量 18 条（位置计数/按类计数、整数聚合不是 HFA、单成员 HFA 只占一槽、4 成员 HFA 用 `RegGroup`、寄存器不重复记账、栈参数不重叠、sret 逐约定各就各位、变参未命名实参、钩子覆盖分类）+ fail-closed 目录 14 条（未注册约定/缺绑定/名字写错/空池/非 2 的幂/未知族名/无 `va_list`/能力缺口/池耗尽/≥3 槽返回）+ CLI 端到端 5 条。文档：`docs/reference/calling-conventions.md`（现行参考）+ `docs/plans/calling-convention-redesign-plan.md`（A1–A7 分期）。
- **顺带修一个跨平台测试坑**（CI 的 `Test (Windows)` 抓到）：黄金快照比对现在**先归一化换行**再比。Windows 上 git 会按 CRLF 检出签入的 LF 文件，而 `str::lines()` 会吃掉 `\r` —— "直接比字符串"会红、"只比 `lines()`"看不见，叠加出来的现象是**差异点报在文件末尾 + 期望 `<缺行>`**（本地因测试自己写过一遍文件而全绿）。已有 CRLF 变异实测（把黄金文件改成 CRLF 后测试仍绿）。
- **本片只新增**（没有任何消费者）：谱里的 `[abi]` 节、管线、IR 都还没切换，因此**现有编译行为逐字节不变**；A2–A5 分步切换（IR `CallConvId` → 管线按 plan 发射 → 删谱里 `[abi]` 换 `[machine]`）。

### Added (2026-09-24) — ISA-DSL v19 V4d（lint 收尾：未指定位 / 可合并为 `vary` 的族）

- **`LINT-UNASSIGNED-BITS`（opt-in `lint --bits`）**：指令字里**没有任何位域覆盖**的位段（按缺省 0 发射）。只对 `kind = "fixed"` 的 ISA 判——变长 ISA 的前缀/REX/ModRM/VEX 由编码器发射、不在位域表里建模，按表判必成误报；全字常量自动无缺口。**实测 + 人工核对**：x86 0 / riscv64 3（`NOP`/`ECALL`/`EBREAK` 的固定位型，故意只声明 `opcode`）/ arm64 67（全是保留位：`BR`/`RET`/`B.cond` 的 `[4,5)`、`[0,5)`+`[10,16)`、`LDUR` 族的 `[10,12)`+`[21,24)` 抽查都对得上参考编码）⇒ 保持评审清单，不进默认档。
- **顺带修谱（真阳性，逐字节不变）**：riscv64 的 AMO/LR/SC 四条模板体原本把 RV64A 的 `aq`/`rl` 两位留在"未指定位"里（谱注释里提过、谱里没声明）⇒ 显式声明 `aq`/`rl` 位域并在四条模板体写 0：`--bits` 的 riscv 条目 11 → 3，`cargo test -p forge-codegen --lib` 仍 **1265 passed**，将来支持 acquire/release 只改 lowering 取值、不动编码定义。
- **`LINT-VARY-CANDIDATE`（opt-in `lint --suggest`，只建议）**：同一 op 的多条 `[[lowering]]` 规则**发射形状相同**（逐行归一：助记符→`_`，数字/寄存器/占位符→通配）⇒ 可用一条 `vary = { attr = [...], name = [...] }` 合并。实测同形状组：x86 55 / riscv64 22 / arm64 7。**默认关**：合并与否是风格取舍（有的谱故意写开、便于各自演进），建议不进门槛。
- 两条规则的清单都由 `crates/frontend/forge-isa-dsl/tests/lint_shipped.rs` 钉住（`unassigned_bits_inventory` / `vary_candidate_inventory`，并断言**默认档仍零结论**）；CLI 增 `--bits` / `--suggest`。

### Added (2026-09-24) — ISA-DSL v19 V3d（谱内派生枚举器：宿主不再手抄 `all_insts()`）

- **生成物新增 `__spec_tests::all_insts()`**（`#[cfg(test)]`，宿主成品零成本）：按谱派生"代表实例"——每条指令 × 每个宽度视图一条，另有每个 imm 槽的 `lo`/`hi` 边界与每个 mem 槽的 `disp=8`/`disp=-8`/`index+scale=4` 风味。取值规则与生成期自测**同源**（新抽出的 `sample_operands`，两处不再各写一遍）。
- **宿主侧全指令往返改由它驱动**：新增 `crates/backend/forge-codegen/src/isa_roundtrip_guard.rs`——三份发行谱的**全部**派生实例（实测 x86 **602** / riscv64 **327** / arm64 **332** 条，远多于指令总数 197/116/104）跑 `encode → decode → encode` 字节闭环 + 解码吃满整条 + `SPEC_INSTS` 覆盖清点（谱里哪条指令没进枚举器就报哪条）。
- **x86 手抄的 650 行 `all_insts()` 删除**（连同 `decode_encode_byte_roundtrip_all`）：手抄清单在谱加指令时不会自动跟上（漏测且无人发现），这一版据实记录。三个 ISA 测试文件合计 **1225 → 557 行**（相对 V3 迁移前的 1832 行 = **−69.6%**，原目标 −≥50% 达成）；`cargo test -p forge-codegen --lib` **1264 → 1265 passed**。
- 保留两处**显式小清单**（语义上不能派生）：各 ISA 的"规范指令 `decode(encode(x)) == x`"（别名撞车时本就不成立）与类表/ABI 类断言。

### Added (2026-09-24) — ISA-DSL v19 V4c（lint 三条新规则：位域重叠 / 能力缺口 / 未引用 ref）

- **`LINT-BITFIELD-OVERLAP`（默认档）**：同一条指令的字段视图里两个位域抢同一批位（视图 = form 预设 ⊕ 指令覆盖后的编码键 + 指令 `fields`）。**必须按逐指令视图判**：按整张 `[conventions.bitfields]` 表判会把 riscv 的 `shamt5`/`shamt6`、`funct5`/`funct6`/`funct7`、arm64 的 `op6`+`imm26`、两边的 `word`（全字常量）这类"同一批位的多种解释"全判成错。**首次落地报出 4 条真阳性**：arm64 的 STP/LDP X/W 四条的 `idx3`（bit24 与 `op8` 常量重复写同一位，取值恰好一致所以黄金字节没暴露）⇒ 已修谱为 `idx2`（`op8` 常量给 bit24），`cargo test -p forge-codegen --lib` 仍 **1264 passed**（89 条 arm64 向量逐字节不变）。
- **`LINT-OP-GAP`（`lint --ops <宿主 op 表>`）**：能力缺口按计划 §10.3 的**三类分开报**——终结指令（`Ret/Jmp/Br/Switch/Unreachable/Invoke`，谱里不该有 `[[lowering]]`）与宿主管线直查的 7 条都不算缺口，只有真缺口报结论，另打印一行覆盖率口径。宿主 op 表是**宿主自己的数据**（`crates/foundation/forge-ir/ops.toml`，DSL 侧不留第二份会漂移的清单）。实测与 §10.3 逐数字一致：x86 `覆盖 100 / 终结 6 / 宿主管线 7 / 真缺口 3`（`AddrSpaceCast`/`Resume`/`VaArg`）、riscv64 `61/6/7/42`、arm64 `8/6/7/95`——守卫 `tests/lint_shipped.rs::op_gap_matches_section_10_3` 把它钉住。
- **`LINT-REF-UNUSED`（opt-in `lint --refs`）**：指令声明了 `ref` 却没有任何 lowering/pattern/emit/pseudo/spill 模板行首引用它。**默认关**：`ref` 有"给还没写的 lowering 预留多态名"的合法用法（实测三谱 28 条——arm64 27 条预留 / x86 1 条 `vmovups` 疑似残留），这是作者意图，只有作者能判；写新谱时打开它抓"名字拼错 ⇒ 多态分派永不命中"最有用。清单快照 `tests/lint_shipped.rs::unreferenced_ref_inventory`。
- CLI：`forge-isa lint <谱>... [--json] [--ops <宿主 op 表.toml>] [--refs]`；退出码口径不变（默认档零结论仍为 0）。

### Added (2026-09-24) — ISA-DSL v19 V5（参数化变体：`[meta].variants` / `only_variants` / `--params`）

- **一份源谱可以投影出多个变体**：`[meta].variants = { xlen = [32, 64] }` 声明"参数名 → 取值域"，指令/模板行/`[emit.prologue|epilogue]`/`[spill.*]`/`[[pseudo]]`/`[[pattern]]` 六处都能标 `only_variants = { xlen = [64] }`（"本声明只在这些取值下存在"）。**默认档逐字节不变**（不传参数 ⇒ 不过滤、不替换）。
- **`forge-isa validate|insts --params xlen=32`**（逗号分隔多个）：`insts` 第一行打印**投影账目**（丢了哪些指令、逐节丢了什么、投影后剩多少），`--json` 里是 `projection` 字段。参数名未声明 / 取值越界一律 `DSL-META` 报错——拼错参数名不会静默"什么都没发生"。
- **`isa_from_file!` 新增 `params = { xlen = 32 }`**：生成期投影，参数进生成物**文件名哈希**（同一份谱的两个变体落到不同 `$OUT_DIR` 文件，互不覆盖）。
- **连带规则**：引用了被投影掉指令/`ref` 的 `[[lowering]]` 规则自动随之丢掉（可推断依赖），账目单列条数；`asm`/`[[lowering]].insts` 里的 `{参数名}` 替换成取值（宽度是数据）。**其余引用 fail-closed**：`[spill]`/`[emit]`/`[[pseudo]]`/`[[pattern]]` 引用了被投影掉的声明却没标 `only_variants` ⇒ 校验期报"未知指令引用"（结构件必须由作者显式变体化，自动猜是错的）；模板里留着参数占位符却没传值 ⇒ 报错并点名传法（否则生成的汇编会打印出字面 `{width}`）。
- **实测账目**（`forge-isa insts --params xlen=32 isa/riscv64.toml`）：`116 → 104` 条指令（丢 `LD`/`SD` 与 10 条 W 族）、`110 → 96` 条 lowering（连带丢 14 条）、逐节丢弃 `[emit.prologue]`/`[emit.epilogue]`/`[spill.GPR]` 各 1 项。守卫 `crates/frontend/forge-isa-dsl/tests/variants.rs`（6 条：默认档零投影、未声明/越界报错、RV32 账目快照、级联不多丢、只对传了的参数生效、替换语义）。
- **MVP 边界**：只读投影（不注册后端、不生成变体专属运行期表）。RV32 投影的帧件是空的（本谱没写 LW/SW 版本），因此它不是一份可运行的后端谱——用途是**看见变体依赖面**与让生成物正确分文件。

### Changed (2026-09-24)

- **`report::validate_file_opts` / `validate_opts` 的第二参数由 `bool` 改为 `&RunOpts`**（V5 的一部分）：`strict_overlap` 变成 `RunOpts` 的一个字段，与新的 `params`（变体投影）放在同一处；调用点（CLI、`tests/strict_overlap.rs` 等）随之同步。**更正**：本轮一度把 `tests/strict_overlap.rs` 的编译错误记成"V6b 遗留的坏测试"，核对提交 `68e3a92` 后确认当时的签名是 `(path, strict_overlap: bool)`、该测试**可编译**——错误是本轮重构引入并当场修掉的，V6b 无此问题。

### Added (2026-09-23) — ISA-DSL v19 V6（确定性守卫 + `validate --strict-overlap`）

- **`validate --strict-overlap`**（新诊断码 `DSL-OVERLAP`）：报"同一 op 的两条 lowering 规则取值域**相交但互不包含**"（裁决序里前者先命中）——用来抓"`when` 写窄导致大部分取值掉进兜底"或"靠 `priority` 硬分胜负"这两类可疑形状。**默认档不变**（实测三份发行谱共 61 条、抽查全是故意的"特化 + 兜底"，因此它只作为评审清单，CI 不开这个档；清单条数由 `crates/frontend/forge-isa-dsl/tests/strict_overlap.rs` 钉成快照，变了要人工复核）。
- **生成物确定性守卫**（`crates/frontend/forge-isa-dsl/tests/determinism.rs`）：同一份谱同参数展开两次，token 文本必须**逐字节相同**、文件名恒定（`spec_tests` 变体不同名）——防止生成器内部换成 `HashMap` 迭代序/引入时间戳后"缓存失效 + 本机能编 CI 编不过"而 `cargo test` 仍全绿。

### Added (2026-09-23) — ISA-DSL v19 V4a（`forge-isa lint` 静态体检）

- **新子命令 `forge-isa lint <谱>... [--json]`**：不执行、不编译，只报"写了却用不上"的声明——`LINT-UNUSED-SLOT`（`[[operand_slots]]` 没被任何 `ops` 引用）、`LINT-UNUSED-FORM`（`[[forms]]` 没被任何指令/模板引用）、`LINT-UNUSED-BITFIELD`（`[conventions.bitfields]` 的位域名只出现在声明处，按标识符边界计数，`imm1` 不会命中 `imm12`），结论锚到 TOML 行列；退出码与 `validate` 一致（0/1/2）。三份发行谱首次跑出 2 条真阳性（x86 的 `gpr32` 槽、arm64 的 `p6` 位域，都没人引用）→ 均已删掉（零行为变化：`cargo test -p forge-codegen --lib` 仍 1150 passed），现在三谱全部干净。守卫 `crates/frontend/forge-isa-dsl/tests/lint_shipped.rs` 把"零误报"钉成快照。

### Added (2026-09-23) — ISA-DSL v19 V3a（谱内测试向量 + `forge-isa test`）

- **谱里可以直接写测试向量（`[[vectors]]`）**：`{asm, bytes}`（`assemble → encode` 逐字节相等 + `decode` 吃满再编码一致）、`{asm, error = "<子串>"}`（汇编/编码必须失败）、`{bytes, error = "DECODE"[, partial = N]}`（解码必须失败，`partial` 钉 `decode_partial` 的消费量）、`{bytes}`（解码正向）。生成期把它们翻成 `__spec_tests::spec_vector_<下标>` 用例，**不再需要手抄 Rust 黄金字节表**。形态在解析期校验（缺 `asm`/`bytes`、正负同给、定宽字长不符、字节越界、`partial` 用错、同一文本两种期望字节、空子串均报错）。riscv64 已迁移 67 条（原先 Rust 里的三张 oracle 表；迁移前后字节集合的规范化 sha256 相同，`cargo test -p forge-codegen --lib` 903 → 970 passed）。
- **新 CLI 子命令 `forge-isa test <谱> [--json]`**：给任意谱现搭一个**零宿主** crate（只依赖 `forge-isa-runtime` + build-dependency `forge-isa-dsl`，`parts = ["encode","decode","asm"]`）并 `cargo test --offline`，直接跑谱里的向量与每条指令的闭环用例——不需要 forge-codegen，也不需要作者先写宿主。退出码与其它子命令一致（0/1/2）。

### Added (2026-09-23) — ISA-DSL v19 V3b（三份发行谱的黄金字节全部进谱）

- **x86（102 条）与 arm64（78 条）的 golden 字节表也迁进 `[[vectors]]`**：x86 的 7 张 oracle 表（GPR/R 型/控制流/SSE/内存/家族/VEX，含 4 条多行写法条目）与 arm64 的 78 条 `enc("…") == word_le(0x…)` 断言。三份发行谱现在合计 **247 条向量**（riscv64 67 + x86 102 + arm64 78），全部由生成物 `__spec_tests::spec_vector_*` 执行：`cargo test -p forge-codegen --lib` **1150 passed**（= 903 基线 + 247），`forge-isa test` 逐谱 `failed:0`。迁移前后字节集合的规范化 sha256 均相同；三个测试文件合计 **1832 → 1436 行（−396）**。
- **向量冲突判据**：同一段 `asm` 给出两种期望字节 → 编译期报错；完全相同的重复**允许**（x86 谱里有一条历史记录：`mov RAX, RBX` 两种编码合并后逐字节相同）。
- **`[[vectors]]` 增第 5 种形态：`{asm}` 闭环向量**（v19 V3c）：只断言 `assemble → encode → decode → encode` 字节稳定与 `disassemble` 幂等，**不比黄金字节**（给"文本合法、字节由别处守"的清单用，文本歧义的指令也安全）。据此把三个 ISA 里剩余的**往返清单**也迁进谱（riscv64 67 条、x86 36 条、arm64 11 条）：三份发行谱现在共 **361 条向量**（x86 138 / riscv64 134 / arm64 89），`cargo test -p forge-codegen --lib` **1150 → 1264 passed**（+114，与迁移条数逐条对上），三个测试文件合计 **1832 → 1225 行（−607，−33.1%）**。

### Changed (2026-09-23)

- **`parts` 不含 `tm` 时补发伪指令展开助手**：`parts` 只开 `asm`、不开 `tm` 的生成物里，`assemble` 引用的 `__pseudo_expand` 原先只在 `tm` 部件的 `gen_assembler` 里发射，导致带 `[[pseudo]]` 的谱（如 riscv 的 `li`）在 asm-only 宿主上编译不过（`E0425`）。现在 `asm && !tm` 时在模块里补发（`tm` 在时不发，避免重复定义）。由 `forge-isa test` 在 riscv64 上实测发现。

### Added (2026-09-23) — ISA-DSL v19 V2（外部宿主实证）

- **新增示例 `examples/isa-host-demo`：证明一份 ISA 谱可以独立接入**。它只有 `[dependencies] forge-isa-runtime` 与 `[build-dependencies] forge-isa-dsl`——连 proc-macro crate 都不依赖（`build.rs` 直接调 `pregenerate`、`src/lib.rs` 只写一句 `include!`），自带玩具谱 `isa/toy16.toml` 与依赖面/生成物守卫。`cargo test -p isa-host-demo` 跑出 9 条生成期自测 + 5 条宿主用例；`cargo tree -p isa-host-demo --edges normal,build` 里没有 forge-codegen。这是 v19 目标 G1（任意普通 crate 都能承载一份谱）的硬证据，也是教程"新 ISA 从这里开始"的可抄模板。

### Changed (2026-09-23)

- **`spec_tests` 不再要求 `tm` 部件**：生成期自测（`__spec_tests`）只需要 encode/decode/asm，此前 `parts = ["encode","decode","asm"]`（不含 `tm`）会报"必须写 `spec_tests = false`"——但自测根本不碰 TargetMachine 集成层（ABI/lowering/帧布局），这条限制挡住了"只做编解码 + 汇编"的宿主（例如 `examples/isa-host-demo`）。判据收敛为 `Parts::supports_spec_tests()`；缺 encode/decode/asm 仍在编译期明确报错。

### Changed (2026-09-23) — ISA-DSL v19 V1（破坏性）

- **生成物运行面拆成 `forge-isa-runtime`；`isa_from_file!` 删除 `krate`**。生成物不再依赖宿主 crate 的公开面，一律写绝对路径 `forge_isa_runtime::…`（`forge_ir::…` → `forge_isa_runtime::ir::…`），因此**任意普通 crate 只要依赖 `forge-isa-runtime` + build script 预生成**就能承载一份 ISA 谱；`krate` 参数与"按宿主改写路径根"的机制一并删除。
- **编译管线改为运行时注册**：`parts` 含 `tm` 时，宿主要用 `forge_isa_runtime::register_pipeline(ISA 名, 工厂)` 注册自己的管线（forge-codegen 已在 `pipeline_hooks::ensure_registered()`（JIT 入口自动调用）接好三个发行后端）；未注册 ⇒ 明确 `IrError::Unsupported`，不 panic。`impl_erased_target_machine!` 随之只按 ISA 名查表。
- 新增守卫 `crates/foundation/forge-isa-runtime/tests/runtime_surface.rs`：运行面依赖面只允许 `forge-ir`/`smallvec`/`thiserror`，源码不得出现 `forge_codegen`/`crate::pipeline`。

### Fixed (2026-09-23)

- **编辑器里 `isa_from_file!` 那几行的假阳性语法错全部消失（v18 S10d）**。rust-analyzer 一直在 `crates/backend/forge-codegen/src/arch/*.rs` 与 4 个测试夹具文件的 `isa_from_file!(…)` 行上报成对的 `expected expression` / `expected R_PAREN`（实测 riscv64 15 对 / arm64 4 对 / 夹具 9、6 对，x86 0），而 `cargo build/check/test/clippy` 一直全绿。
  根因是"整份生成物（x86 全部件 1.0 MB token）作为**宏展开结果**"撑出来的 RA 展开管线问题；修法是让生成物走**文件**这条路：落到 `$OUT_DIR/forge_gen_<模块名>_<参数哈希>.rs`，`isa_from_file!` 只展开成一句 `include!(concat!(env!("OUT_DIR"), "/…"))`。实测四类文件 **68 条 → 0**，且没有引入新的 unresolved import。
  两条踩过的坑（`docs/guides/rust-analyzer-notes.md` §1 有完整证据）：① RA 只加载"分析开始前"就存在的文件——宏在展开期写出的文件它会报 `failed to load file`，随后生成模块的项在 20+ 个文件里全变 unresolved（**比原来更糟**），所以生成物改由**宿主 build script** 预生成；② `TokenStream::to_string()` 在 proc 宏里（rustc 美化打印）与普通二进制里（紧凑打印）**文本不同**，两侧都写会互相覆盖、每轮重编，因此只允许 build script 一个写者。反面也试过：给生成物 token 整体换 `Span::mixed_site()` **完全无效**（诊断逐文件一模一样）。

### Changed (2026-09-23)

- **宿主 crate 需要 build script 预生成 ISA 生成物（v18 S10d，破坏性 / 两处三行）**。`Cargo.toml` 加 `[build-dependencies] forge-isa-dsl = { path = … }`，`build.rs` 里 `forge_isa_dsl::pregenerate_host().expect(…)`
  ——它扫 `src/ tests/ benches/ examples/` 里每一处 `isa_from_file!`，按**同一份**参数解析生成到 `$OUT_DIR`，并登记 `cargo:rerun-if-changed`（谱的全部来源文件 + 被扫描的源文件与目录）。
  没接就**编译期**报错并点名这两步（fail-closed，而不是悄悄让生成模块变空）。`forge-codegen` 已就位；落盘件里带 `#[allow(warnings, clippy::all)]`（生成物不再享受"宏展开不 lint"的豁免，实测不加会多 223 条风格类警告）。
  `forge_isa_dsl::expand_file`（完整 token 流入口，CLI 与各守卫测试用）语义不变。

### Changed (2026-09-22)

- **角色去宽度化：`Role` 不再把位宽编进名字（v18 S9，破坏性 / 只需改 TOML 6 行）**。原先 `roles` 里是 `fpr_mov_f32` / `fpr_mov_f64` / `wide_vec_store_32` / `_64` / `wide_vec_load_32` / `_64`——位宽被写死进角色名，别的位宽的 ISA 接不进来（等于把 x86 的 32/64 当成 ISA 通用事实）。现在角色名去掉宽度后缀（`fpr_mov` / `wide_vec_store` / `wide_vec_load`），宽度**写在角色声明里**：`roles = [{ role = "fpr_mov", bits = 32 }]`（单位是**位**，与 `opsize`/`[encoding].bits` 一致；无宽度语义的角色照旧写 `"gpr_mov"`）。
  生成器按 **(角色, 位宽)** 解析（`role_name_for`），选不到时错误消息给出**请求位宽 + 已声明位宽集合**；校验同样按 (角色, 位宽) 唯一——同角色同宽度声明两次、或同一角色一处写 `bits` 一处不写，都在**编译期**报错并点出两条指令名。`isa/x86.toml` 改了 6 行（`MOVSS`/`MOVSD`/`VMOVUPS_256_*`/`VMOVUPS_512_*`），**生成的机器码逐字节不变**（黄金字节 + 三架构 JIT 矩阵与基线逐数字相同；另有两例守卫：任意位宽 16/24 合法、同 (角色,位宽) 冲突必报）。设计动机、证据与"为什么宽度不能从槽派生"（`MOVSS`/`MOVSD` 共用 `fpr` 槽）见 `docs/archive/forge-dsl-v18-plan.md` §7.1。

### Changed (2026-09-21)

- **`[[pattern]]` 与 `[[lowering]]` 统一裁决序 + 死模式检测（v18 S5c）**。`[[pattern]]` 新增 `priority`（与 `[[lowering]].priority` 同语义：大者先试），裁决序统一为 (`priority` 降, 匹配树 Op 节点数降, `when` 谓词叶子数降, 声明序升)；
  此前 pattern 的排序只写在 codegen 里（没有 `priority`），现在抽成 `IsaModel::pattern_order()`，**codegen 与校验器共读一份**。新增**死模式检测**：匹配树结构相同、且裁决序在前者的 `when` 完全覆盖后者 → 编译期报错并点出两个 `[[pattern]]` 下标（不同树之间不做覆盖推断——保守，宁可漏报不误报；与 lowering 的死规则检测同口径）。x86 现有两条 pattern 都没有 `priority`、新旧排序键等价 ⇒ **生成物不变**（黄金值与 JIT 矩阵照旧）。`isa-dsl.md` 的 `[[pattern]]` 节、`isa-dsl-errors.md` §3.10 同步；键速查表与 `isa-dsl.schema.json` 因新增 `priority` 键重新生成。

- **`[[lowering]].op` 支持名单：一条规则服务多个同类 op（v18 S5）**。`op = ["Copy", "Uextend", "Freeze", "Ptrtoint", "Inttoptr"]` 等价于把这五条逐条写开，但**只维护一份发射序列**（改一处不会漏另外四处）。名单在解析期展开成逐 op 的规则，名单序 = 展开序，因此每个 op 内部的裁决序与逐条写开**完全一致**；空名/重名报错。
  随迁移落地（数字为**声明数**；`forge-isa insts` 报的是 `vary`/`op` 展开后的执行规则数）：x86 同 op 冗余 `when` 折叠 13 组 + 跨 op 归并 6 组（**220 → 197 条声明（−10.5%）**，展开后执行规则 299 → 286；TOML 3,693 → 3,518 行）；riscv 3 组（**110 → 106 条声明（−3.6%）**，展开后仍 110）；arm64 不变。
  **等价证据**：逐 op 在**同一属性空间**上比较迁移前后的判定函数（属性取值 ∪ {v±1, 0, 哨兵}，按 (`priority` 降, 叶子数降, 声明序升) 取首条命中）——x86 100 op / 2,155 格、riscv 61/352、arm64 8/61 **逐格相同**；`cargo test -p forge-codegen`（27 个测试二进制，含全部黄金字节与 417 条指令的规格自测）与三架构 JIT 矩阵（195/3/0、131/67/0、23/175/0）全绿。
  **按实测收窄**（不做，附数字）：① `lowering.emit` 结构化表（`inst`/`let`/`select`/`switch`）——族式 op 在多个属性上同时分派，表格化的规则数收益 ≤5%，而 `vary` 已覆盖其绝大多数形态；② `[[sequences]]`——跨 op 的长序列复用实测为 **0**（x86 88 条长序列、77 个不同序列，无一跨 op 重复）。两条都会引入第二个参数化机制（与 S2c「三机制合一」的结论相悖）却只值 ≤5%/0，故不做；方案里"x86 ≥15%"的目标实测不可达（真实冗余 10.5%），已按实测修正。
  文档：`docs/reference/isa-dsl.md` 的 `[[lowering]]` 节补 `op` 名单与"为什么不加 emit/sequences"（含数字）；`isa-dsl-errors.md` 补 §3.9；方案 §7 记 S5 进度与 S5c 待办。

### Fixed (2026-09-21)

- **`#:schema` 编辑器补全：指令的 `ref` 键被 schema 写成了 `reference`（v18 S7e 修）**。模型的字段名是 `reference` + `#[serde(rename = "ref")]`，而 JSON Schema 发射器按字段名发射，于是**所有用 `ref` 的谱都被编辑器标红**（`isa/x86.toml` 35 处，Taplo + `#:schema`）。
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
  `include`/`[[override]]` 同时进 `IsaModel` + JSON Schema + 文档键表（三方针守卫覆盖）；绕过加载器把**裸文本**交给解析器时，带这两个组合键会明确报错而不是静默忽略。修掉 `report::validate_file` 把加载器消息改写成「读不到文件：<根路径>」的问题（缺 include / 覆盖键不存在时现在是点名文件与键的可诊断错误）。
  文档：`docs/reference/isa-dsl.md` 新增「多文件组合（`include` / `[[override]]`，v18 S7d）」节 + `isa_from_file!` 四参数表 + `fmt` 命令；方案 §5.8 改写为落地形态；夹具 README 增列 `include_root` / `include_base`。
  用例：`forge-isa-dsl/tests/parts_selection.rs`（7）、`forge-codegen/tests/include_v12_tests.rs`（7，含多文件黄金字节 + 只开 `encode` 的模块真编译）、`forge-isa/tests/cli_tests.rs` 增至 16（多文件 validate/insts、`fmt` 折叠与幂等、诊断指向片段文件、缺 include / 覆盖键不存在必须点名）。

### Changed (2026-09-21)

- **生成 `Inst` 变体的字段名 = `ops` 里声明的操作数名（v18 S7d 修正）**。此前字段名另取一套：定宽 ISA 取位域名（`[forms].operand_fields` 的 `rd`/`rs1`），变长 ISA 取语义角色名（`dest`/`cond`/`mem`/`imm`/`target`）——`ops` 里作者写的名字**只用于 asm 模板**，于是 `ops = ["dst:r:out", "src:r"]` 生成出 `Inst::Iadd { rd, rs1 }`：作者声明的名字在用户面没有任何意义，位域名还泄漏成了 API。
  现在**字段名就是声明名**（`Inst::Iadd { dst, src }`、x86 `MovRmR { src, dst }`、条件码 `JccRel32 { cc, target }`），位域名/语义角色名退回纯内部**编码键**（只用于查 `[conventions.bitfields]`、modrm 角色与立即数编码表）。名字不能直接作标识符时按最小规则归一：Rust 关键字 → 原始标识符（`type` → `r#type`）、数字开头 → 前缀 `_`（`8bit` → `_8bit`），**不做语义改名**。
  **编码行为零变化**：`demo_inst12` 生成物 token 级对照（v18 S7a 基线 vs 现在）只有 `rd→dst`(32)/`rs1→src`(32)/`rs2→src2`(16) 共 80 处标识符改名，未触碰任何编码 token（证据 `target/s7d_field_rename_evidence.txt`）；三个 ISA 的黄金字节测试、417 条指令的生成期自测、三条 JIT 矩阵全绿。
  守卫与迁移：新增 `forge-isa-dsl/tests/field_names.rs`（定宽用声明名而非位域名、关键字原始化、数字开头加前缀、变长 ISA 按声明名）；`OperandUse` 的死字段 `field` 删除，改为携带**声明名** `name`；`crates/backend/forge-codegen/tests/*` 的引用同步改名（`dest`→`dst`、`rd`/`rs1`/`rs2`→`dst`/`src`/`src2`、`imm*`→`imm`、label 域→`target`、`cond`→`cc`）。文档：`docs/reference/isa-dsl.md` 的「命名操作数」「`[[forms]]`」「代码生成输出」三节改写为"声明名 = 字段名，位域名 = 编码位置"。

### Added (2026-09-20)

- **ISA-DSL 的 JSON Schema + `#:schema` 编辑器补全（v18 S7c），三方一致由守卫钉住**。`forge-isa-dsl::schema`（手写发射器，不引 `schemars`）把谱的 TOML 结构发射成 JSON Schema（draft 2020-12，32 个节定义 + 18 个根键，含必填/可选/编码键与节级说明）；`forge-isa schema [--out <file>]` 打印或写出，仓库根的 `isa-dsl.schema.json` 由它生成并签入；3 个发行 ISA + 6 个夹具的 TOML 顶部加 `#:schema <相对路径>` 注释，Taplo 等语言服务据此补全。
  **三方守卫**（`crates/frontend/forge-isa-dsl/tests/schema_guard.rs`，5 条）：① schema 每节的键集与 `dsl/model.rs` 对应结构体的 `pub` 字段**逐键相等**（`#[serde(skip)]` 内部字段须在 `INTERNAL_FIELDS` 登记；`#[serde(flatten)]` 字段须在 `FLATTEN_FIELDS` 登记）；② 内部字段表不得过时；③ `docs/reference/isa-dsl.md` 新增的「键总览（速查表）」区段与 `schema::markdown_table()` **逐字相同**——文档里的键表不再手抄；④ 签入的 `isa-dsl.schema.json` 与发射器逐字相同；⑤ `--nocapture` 打印可粘贴的表格。守卫在落地时就抓到三处漂移（`enc` 被误当键、`Pattern` 的 `r#match`/TOML `match` 漏写、`[[pattern]].when` 未登记）。
  文档：`docs/reference/isa-dsl.md` 新增「键总览（速查表，v18 S7c）」节（机器校验的键表）+ 工具链节的 `schema` 与 `#:schema` 说明 + TOC；方案 §7「S7 进度」补 S7c。

### Added (2026-09-20)

- **`forge-isa` CLI（v18 S7b）：写 TOML 时不必接后端就能校验、看展开结果、做规格 diff**。新增 `crates/tools/forge-isa`（bin，唯一依赖 `forge-isa-dsl`；手写参数解析与 JSON 发射器，不引 `clap`/`serde_json`）：

  | 命令 | 作用 |
  | --- | --- |
  | `validate <谱.toml>…` | 解析 + 校验，打印**全部**诊断（`路径:行:列: 码: 消息` + 附注），有错退出 1 |
  | `insts <谱.toml>` | 展开后的指令与生效规格：字长（位/字节）、form、opcode、ops、编码键、`ref`、`reloc`、来源模板与行号、asm |
  | `explain <谱.toml> <指令名>` | 单条指令的完整来源：哪个模板的哪一行 + 该行与 `body` 的键 + 生效规格逐字段 |
  | `diff <a> <b>` | 两份谱的**规格 diff**（增/删/改字段，`字段: A → B`）——迁移前后"展开后有效规格"对照 |

  `--json` 给机读输出；退出码 `0` 成功 / `1` 诊断或失败 / `2` 用法错误。示例：`cargo run -p forge-isa -- explain isa/arm64.toml ADDREGW` → `来源：[[templates.ADDREG]] 第 2 行` + 生效编码键。
  实现：新增 `forge-isa-dsl::report` 投影层（`IsaSummary`/`InstRow`/`Explain`/`SpecDiff`），**复用**编译器的 `collect_inst_infos`（form 预设 ⊕ 指令级覆盖的同一份判定），编码键清单由 `EncKeys` 的 serde 折出——不维护第二份键名表，新增编码键自动出现在三个子命令里。
  证据：`forge-isa` 10 条集成测试（跑真实二进制）+ `report` 6 条单测；`forge-isa-dsl` 182 单测；三份发行 ISA `validate` 全 OK；`insts isa/riscv64.toml` = `116 条指令 / 19 条模板 / 110 条 lowering`；混合字长夹具 JSON 给出 `CADD16/CMOV16 = 16 位（2 字节）`、`LNOP32/LADD32 = 32 位（4 字节）`。
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
  生成模块导出 `SPEC_TOTAL`/`SPEC_COVERED`/`SPEC_CASES`/`SPEC_SKIPPED`（名字+原因，S6 判据：空）/`SPEC_TEXT_AMBIGUOUS`（同名同形、编码不同 ⇒ 文本分不清）；外部守卫 `src/spec_coverage_guard.rs` 钉死指令总数（x86 197 / riscv64 116 / arm64 104）、零跳过与歧义名单（31/4/0）。`isa_from_file!` 新增第三参数 `spec_tests = <bool>`（缺省 true）；夹具谱（`tests/common/mod.rs`，同一份谱被多个测试二进制包含）显式关掉，由 `tests/spec_tests.rs` 打开三个极端形状夹具（1 字节寄存器 / 12 位字 / 混合字长）。
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
  - `mixed`（**新能力**）：编码按该指令字长发字节；解码**按 `widths` 升序分组**、每组一棵位级 trie、"首个完整匹配即停"（短编码优先，RVC/Thumb 同构）。新夹具 `crates/backend/forge-codegen/tests/isa/demo_mixed16_32.toml`（低 2 位判别短/长编码：`sel = 0` vs `3`）+ `tests/demo_mixed16_32_tests.rs` 5 条用例（黄金字节、按宽度分组解码与消费字节数、编解码往返、`decode_partial` 截断阈值 = 最短字长、能力集）。
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

- **ISA-DSL arm64 条件码符号化 + `b.cond` 全条件（v18 S3c）**：`isa/arm64.toml` 新增 `[conventions.cond]`（A64 的 16 个条件名 `eq/ne/cs/cc/mi/pl/vs/vc/hi/ls/ge/lt/gt/le/al/nv` + `hs`/`lo` 同码别名；**不写 `ir`**——arm64 的 lowering 目前不用 `{cc}`，将来加 Icmp lowering 时 validate 会强制补全 10 个），CSEL 族的 `cond4` 槽从 `kind = "imm"`（写成 `#0`）改为 `kind = "cond"`：汇编/反汇编现在用**符号名**（`csel x0, x1, x2, eq`，反汇编渲染同码首选名 `hs`→`cs`）。
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

- **ISA-DSL 参数化模板 `[[templates]]`（v18 S2a，**首版机制形态**：`params` 平行数组**，已被同日 S2c 改为 `body` + `rows`；本条目保留首版迁移的实测数字）**：一处声明 → N 条指令，自动派生别名。动机是实测出来的重复：`isa/arm64.toml` 有 **38 对**指令只差 sf 位/寄存器槽/opcode（76/89 = 85%）、`isa/riscv64.toml` 有 **16 对** S/D（41%），而旧 `[[families]]` 的变体**无法覆盖 `ops`/`form`/enc 键**，只能整条复制；与之配套的 `[[aliases]]` 还是手写清单（x86 25 条 / arm64 29 条，且已见漂移：`vaddps` 漏了 `VADDPS_ZMM_MASKZ`）。
  新机制：`params` 等长列表按下标 zip（与 `[[lowering]].vary` 同语义）；字符串字段里 `{参数}` 做文本替换、**整串就是 `{参数}`** 时保留参数类型（`opcode = "{opcode}"` 仍是整数）；`name`/`names` 给实例名；`ref` 自动派生别名（单值 = 多态共用，含 `{参数}` = 逐实例）；`[[templates.overrides]]` 逐行打补丁（表递归合并）。展开在**解析期**完成 ⇒ 下游（校验/代码生成）只看到普通指令与别名，`codegen` 一行未改；展开出的实例若非法，诊断前缀改回 `[[templates.X]]`，位置落在模板声明行。
  **arm64 首例迁移**：`isa/arm64.toml` **1,125 → 731 行（−35%）**，32 条模板取代 64 个手写指令块、**29 条手写 `[[aliases]]` 全删**；黄金测试（`arm64_tests` 12 例 + `arm64_tm_tests` 6 例）通过、arm64 JIT 矩阵 **23 passed / 175 skipped / 0 failed** 不变、生成代码里的 `Inst::` 名字集合 **90 个前后 diff 为空**、arm64 生成代码 644,018 → 636,986 B。
  证据与文档：`src/v12/template_tests.rs`（10 例：展开/类型保留/逐实例 ref/补丁合并/五类失败路径/lowering 引用实例名）、`v12/model.rs::Template` 文档、`docs/reference/isa-dsl.md` 新增 `[[templates]]` 节、`docs/reference/isa-dsl-errors.md` 增模板错误表、方案 §7/§12.7 记录实测、通用性守卫扩展为也识别模板实例名（arm64 迁移后 `ADDIMMX` 不再是 `[[instructions]]` 字面量）。

- **ISA-DSL S2 续：riscv 与 x86 定向迁移（三 ISA 全覆盖；**数字为 S2a 形态**）**。riscv `1,754 → 1,606 行（−8.4%）`：15 条模板取代 30 个 `_S`/`_W` ↔ `_D` 指令块（`funct7`/`funct3` 逐行给、助记符后缀 `.{pl}` 插值）——顺带证明 **`[[families]]` 表达不了这类对**（族模板 `{name}` 只能派生"变体名小写"，得不到 `fadd.s` 这种带点助记符，这正是它此前只能手写两份的原因）。x86 `3,356 → 3,324 行（−1%）`：`movzx`/`movsx` 的 R8/R16 四条合成 1 条模板（`ref = "{mn}"` 顺带取代 2 条手写 `[[aliases]]`），四条指令的**生成编码臂文本 SHA-256 逐字节相同**。
  **当轮的设计结论已被 S2c 推翻（保留作过程记录）**：S2a 曾得出"`[[templates]]`/`[[families]]`/`[[aliases]]` 三者**互补而非取代**"，于是选择保留三者分工；S2c 按评审意见把它们合并为 `[[templates]]` 一个机制（见本节第一条）——"互补"成立的是**表达力**判断，不成立的是**该不该并存**：`rows` 的键空间就是指令字段空间，族变体只能覆盖 `fields`/`opcode`/`roles` 的限制消失，别名折成 `ref` 后引用表逐项相同。x86 剩余的同 asm/同 ops 组仍差在**编码键**（`form` 预设有无、`opsize`、内联 vs preset）——那是不同编码而非可参数化取值，统一它们属于语义重构，**不做**（此结论 S2c 沿用）。
  验证：三 ISA 的 `Inst::` 名字集合前后 diff 为空（90 / 117 / 198）、黄金测试（arm64 12+6、riscv 12+4、x86 61 例）与三架构 JIT 矩阵（195/3/0、131/67/0、23/175/0）全不变。

### Changed (2026-09-19)

- **ISA-DSL 诊断升级（v18 S1）：一次列全 + 精确到键行 + 错误码；`[emit]`/`[spill]` 从"完全不校验"变为编译期校验**。执行方案 `docs/archive/forge-dsl-v18-plan.md`（用户口径：允许破坏性更新、无需兼容旧版本、兼顾体验、足够通用），本条目是其中的 S1 切片。
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
  新夹具：`tests/isa/demo_inst8.toml`（8 位字 + 字内 label 域，注册 2 位域 `RelocPatcher`）、`demo_inst12.toml`（12 位字，非 8 倍数 + 填充位必须为 0）、`demo_inst100.toml`（100 位字 = 13 字节，位域在 bit 92..100）；用例在 `tests/demo_inst{8,12,100}_v12_tests.rs`。
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
  夹具演示：`tests/isa/demo8.toml`（`ptr = "gpr1"`）、`tests/isa/demo.toml`（`f32/f64 = "gpr8"` 软浮点）；
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

- **demo/示例 ISA 迁出库本体，`isa_from_file!` 支持宿主 crate 路径**：ISA-DSL 的示例谱（`demo`、`demo8`）此前是 `forge-codegen` 的 `src/arch/` 模块 + 仓库根 `isa/` 谱，与 x86_64/arm64/riscv64 这些**真实后端**并列，容易误读为"发行 ISA"。现在：
  - `isa_from_file!` 新增可选第二参数 `krate = <路径>`：生成物里的 `crate::…` 改写为 `<路径>::…`、`forge_ir::…` 改写为 `<路径>::ir::…`（新增 `forge_codegen::ir` re-export），因此生成代码只依赖宿主的公开面；**缺省参数生成物逐字节不变**（已用 5 个 ISA 的 `FGE_DEBUG_GEN` dump 逐字节比对）。
  - `isa_from_file!` 参数解析与路径改写有单测（`crate` 改写只作用于路径位置，`pub(crate)` 可见性标记与字符串字面量不受影响）。
  - 夹具谱移到 `crates/backend/forge-codegen/tests/isa/{demo,demo8}.toml`（附 `README.md`），由 `tests/common/mod.rs` 用 `krate = forge_codegen` 宿住；删除 `src/arch/demo.rs`、`src/arch/demo8.rs` 与 `src/lib.rs` 的 re-export；6 个测试文件改经 `common::demo*`。
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
  - 新夹具 **`isa/demo8.toml`**（1 字节寄存器 ISA：唯一 `[reg.gpr1]` 组，`addr_width`/`slot_bytes`/`value_gpr_width`/`fp_overhead_bytes` = 1，`default_opsize = 8`）+ `crates/backend/forge-codegen/tests/demo8_tests.rs`：断言元数据派生（`GPR(1)`、1 字节槽、sp/fp/scratch 名字解析成功、`allocatable = A0..A3`）、汇编→编码→解码→反汇编往返、宿主编译 i8 函数（20 字节机器码反汇编回 `mov A3, A0`/`add A1, A3, A2`/`ret`）与 i64 的编译期拒绝。
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

- **V256/V512 向量 IR 层 Load/Store（ISA 规则 + 编译入口能力门）**：`isa/x86.toml` 的 `Load`/`Store` 规则原先只覆盖
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

- **向量溢出（spill）宽度静默截断（WA-46）**：`isa/x86.toml` 的
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
