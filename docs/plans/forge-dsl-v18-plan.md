# forge-dsl 改进方案（ISA-DSL v18）

> 状态：[progress]（2026-09-19 撰写）。用户 2026-09-19 拍板口径：**允许破坏性更新、无需兼容旧版本、
> 可参考网络上的设计方案、需兼顾用户体验、需足够通用而非服务于个别指令集**。
>
> 现行实现：`crates/frontend/forge-dsl/src/v12/`（生成器内部仍叫 `v12`，schema 已迭代到 v17——
> 命名本身就是本方案要修的问题之一）。v18 语法规范落地后重写 `docs/reference/isa-dsl.md`，
> 旧语法（v12–v17）整篇归档 `docs/archive/isa-dsl-v12-v17.md`。
> 文中一切数字为本机实测（命令与日期随行标注），**以代码与测试为准**。
>
> **修订 2026-09-19（S2c）**：按评审意见「三个定义指令的模块只能留一个，否则是使用负担」，
> `[[families]]` 与 `[[aliases]]` 已**删除**，`[[templates]]`（`body` + `rows`）+ 指令属性
> `ref` 成为唯一机制；S2a 曾得出的"三者互补"结论已作废。设计、决策与实测见 §5.2 与 §7，
> 生成代码顺序差异的解释见 §12.8。

## 目录

- [1. 目标与成功判据](#1-目标与成功判据)
- [2. 现状证据](#2-现状证据)
- [3. 设计原则](#3-设计原则)
- [4. 参考设计](#4-参考设计)
- [5. v18 语法（逐节变更）](#5-v18-语法逐节变更)
- [6. 删除与改名总表](#6-删除与改名总表)
- [7. 切片计划](#7-切片计划)
- [8. 迁移](#8-迁移)
- [9. 边缘情况与失败模式](#9-边缘情况与失败模式)
- [10. 显式假设](#10-显式假设)
- [11. 文档落地清单](#11-文档落地清单)
- [12. 附录：S0 基线实测](#12-附录s0-基线实测)

## 1. 目标与成功判据

**目标**：把 forge-dsl 从"能表达三个 ISA 的 TOML 生成器"提升为**通用 ISA 规格语言**——数据驱动
（ISA 形状全在数据里，生成器不含任何 ISA 常量）、可参数化（族式重复由模板实例化消除）、
诊断可用（一次列全部错误 + 精确行列 + 可操作建议）、能力完备（条件码/重定位/伪指令/混合宽度/
生成自测都是数据）。

| 判据 | 当前基线（实测） | 目标 |
| --- | --- | --- |
| 生成器里的 ISA 常量 | `lowering.rs:110-138` 硬编码 x86 setcc 码；`asm.rs:396` x86 cond 缺省；`[conventions.cond]` 只有 x86 声明 | **0 处**，守卫测试钉死，全部从 ISA 数据取 |
| 族式重复 | arm64 38 对 X/W（76/89 = 85%）、riscv 16 对 S/D（32/78 = 41%）、x86 宽度变体与 SSE 族 | 模板实例化后重复声明消失；arm64 TOML 行数 **−30%**、x86/riscv **−10%** |
| 条件码能力 | arm64 只能编码 `b.eq` 一条（`BCOND` + `fields={cond=0}`）；x86 用 `cond` 槽 | arm64 `b.cond` **16 个条件一条声明**；两 ISA 共用同一机制 |
| 手工别名清单 | x86 25 条 / arm64 29 条 `[[aliases]]`，已见漂移（`vaddps` 漏 `VADDPS_ZMM_MASKZ`） | 别名由模板 `ref` 自动派生，清单消失 |
| 诊断 | fail-fast 单条错误；定位靠 `source.find("name = ...")` 启发式（`diag.rs:60-72`） | 一次列全部（≤32 + 计数）；精确 span；错误码 + did-you-mean |
| 校验缺口 | `[emit]`/`[spill]` 只查非空（`validate.rs:1124-1152`） | 与 lowering 同等严格（引用名/占位符/槽绑定全覆盖） |
| 指令级自测 | 生成面手写测试 2,952 行（含 `decoder_smoke.rs` 253 行覆盖十余条指令） | 生成 `__spec_tests`：全指令 encode↔decode↔disasm↔asm + 立即数边界 |
| 指令宽度 | `default_inst_width: Option<u32>` 单一全局值（`model.rs:361`） | `[encoding]` 三态 + 逐指令/模板 `width` ⇒ RVC/Thumb 可表达 |
| 规格书一致性 | 文档停在 v15/S1–S6，代码注释已到 v16/v17；`aliases`、`[conventions.mem]`、指令级 `when`、`prefix_scan.range`、EVEX `b/z/disp_scale` 未文档化；文档 L522 说 lowering 用"助记符"而模型 `model.rs:56-58` 说用"别名/指令名" | schema ↔ 文档 ↔ JSON Schema 三方针由守卫测试强制一致 |
| 门禁 | matrix x86 195/3/0、riscv64 131/67/0、arm64 23/175/0；语料与编码黄金值全绿 | 全部**不回归**；arm64 矩阵通过数上升（具体数落地实测回填） |

**非目标（写死）**：不做 LLVM bitcode/MC 互操作；不做从外部 ISA 描述（SLEIGH/Sail/riscv-opcodes）
自动导入；不做指令选择的形式化验证/SMT；不引入新依赖（如确需 `clap` 或 JSON-Schema 生成器另开请示）；
不动 forge-ir/forge-opt/forge-codegen 的运行期语义（本方案点名的 reloc 宿主契约除外）。

## 2. 现状证据

### 2.1 语法已迭代到 v16/v17，文档停在 v15

- `model.rs:56` `[[aliases]]（S10e）`、`model.rs:684` `[conventions.mem]（v16）`、
  `model.rs:1386` `asm 必填（v17…）`；文档正文只讲 v15 的 S1–S6。
- `docs/reference/isa-dsl.md` 里 `aliases` **0 次出现**；`[conventions.cond/mem]` 现语义、
  `prefix_scan.range`、EVEX `b/z/disp_scale`、指令级 `when`（`Instruction.when`）均无记载。
- 文档 L522：`insts` 用**助记符**引用；`model.rs:56-58` 与 `validate.rs:817-839`：用**别名名或指令名**引用。
  实际 x86 写 `mov`/`add`（别名）、riscv 写 `ADDW`（指令名）——文档这条是错的。

### 2.2 参数化缺失 ⇒ 三个 ISA 都在手抄族式重复

| ISA | 重复形态 | 实测 |
| --- | --- | --- |
| arm64 | X/W 对（sf 位、寄存器槽、opcode 三处不同，其余全同） | **38 对 / 76 条 = 89 条的 85%**：`ADDIMMX`(0x91, r64) 与 `ADDIMMW`(0x11, r32) 仅三处不同 |
| riscv | `_S`/`_W` ↔ `_D` 对（fpr4/fpr8、opcode） | **16 对 / 32 条 = 41%** |
| x86 | 宽度变体（`ADD_R_IMM32`/`ADD64_R_IMM32`、`MOV64_*`/`MOV_R8_*`…）与 SSE SD/SS 族 | 146 指令 + 51 族变体、19 forms；仅 6 组 12 条能被"同 form+asm+ops"归并，其余因变体无法覆盖 `ops`/`form`/enc 键而只能整条重写 |

`[[families]]` 的变体只能覆盖 `name/opcode/fields/asm/roles/when`（见 `model.rs` 的 `FamilyVariant`），
**不能覆盖 `ops`/`form`/其他 enc 键** ⇒ arm64 的 X/W 对写不进 family，只能整条复制。

### 2.3 别名是手工清单，且已漂移

x86 `[[aliases]]` 25 条、arm64 29 条（几乎全是 X/W 对或 1:1 改名），riscv 0 条。实测漂移：
`VADDPS_ZMM_MASKZ` 被 1 条 lowering 直接按指令名引用，却**不在** `vaddps` 别名里。
清单式成员登记 = 新增变体时忘了登记就静默失去"按引用名分派"的能力。

### 2.4 条件码不是数据（最典型的"服务个别指令集"）

- `lowering.rs:110-138`：在**通用** lowering 生成器里硬编码 `IntCC → x86 setcc 码`
  （`Equal => 4`、`SignedLessThan => 12`…），注释自述"x86 的 setcc 编码"。
- `asm.rs:396` `cond_default()`：`[conventions.cond]` 缺省 = x86 的 `o/no/b/ae/e/ne/...`；riscv/arm64 均未声明该表。
- 后果（实测）：arm64 只有 `BCOND{fields={cond=0}, asm="b.eq {target}"}` —— 只能编 `b.eq`；
  其余 15 个条件既无法声明也无法编码。同一件事在 x86 是"cond 槽 + 表"，在 arm64 只能是字面量。

### 2.5 `[emit]`/`[spill]` 是未校验的名字引用面

`validate_emit`/`validate_spill` 只查非空；`insts = ["PUSH RBP", "MOV64_RR RSP, RBP",
"SUB64_R_IMM32 RSP, {callee_saved_bytes}"]` 里的指令名、占位符没有任何编译期校验
（对比：lowering 已有引用名/占位符/属性三重校验，`validate.rs:875-924`）。
另有 5 个固定伪指令（`@push_callee`/`@pop_callee`/`@frame_alloc`/`@frame_dealloc`/`@move_args`）
与 41 条占位符分支硬编码在 Rust 里（`frame.rs`/`placeholder.rs`）。

### 2.6 指令宽度只有全局单值

`model.rs:361` `default_inst_width: Option<u32>` + `variable_length: bool`：定宽 ISA 全局一个值，
变长 = x86 式前缀扫描。**16/32 混合宽度（RVC、Thumb、MIPS16）无法表达**；`Instruction` 无 `width` 字段。

### 2.7 诊断：fail-fast + 启发式定位

`validate.rs:9-23` 顺序调用 14 个校验器，每个 `Result<(), String>` ⇒ **一次只报第一条**；
`diag.rs:60-72` 用 `source.find("name = \"X\"")` 回找行号（同名或同名前缀会指错），
抽不出名字就退化到 `1:1`。没有错误码、没有多错误、did-you-mean 只在个别消息里手写。

### 2.8 生成器里仍有 x86 缺省

`asm.rs:396`（cond 表）、`[conventions.prefix_scan]` 缺省 = x86 扫描集、`[abi]` 多项缺省 = x86 形态
（含 `call_ret_reg` 缺省 `"X1"` = riscv 的 ra）、`value_fpr_width` 缺省 8、`vector_tiers` 缺省 `[16,32,64]`。
这些缺省让非 x86 ISA 在**不报错**的情况下拿到别家形状。

### 2.9 工程形态

`forge-dsl` 是 `proc-macro = true` 的 crate，装着 model（1,781 行）+ validate（1,107 行）+
codegen（约 10.7k 行）+ 单测 `tests.rs`（2,713 行）：**无法出 bin/example**（proc-macro crate 限制），
单测无法作为普通 lib 被工具复用。`isa_from_file!` 只有 `krate` 一个参数（无模块名覆盖、无部件选择、
无多文件组合），没有 JSON Schema/编辑器补全，`FGE_DEBUG_GEN` dump 的是未格式化 token 串。

## 3. 设计原则

1. **数据 vs 宿主**：凡"某个 ISA 的形状"（条件码、重定位语义、伪指令、宽度、寄存器角色）都是数据；
   凡"编译器基础设施语义"（IR 算子、ABI 参数类、regalloc 契约）才是宿主。§2.4/§2.5/§2.6 三处越界改回数据。
2. **一族一写法**：同一概念在所有 ISA、所有节里用同一写法。`lowering.vary` 的"等长参数表 + 下标 zip +
   自动谓词"已经存在，就把它提升为**指令模板**的同一机制，而不是再造一套。
3. **单一事实源 + 守卫**：schema（serde 模型）是唯一事实源；文档、JSON Schema、编辑器补全、
   "生成器不得含 ISA 常量"都由会失败的测试钉住（沿用 `tests/no_hardcoded_widths.rs` 的
   "白名单 + 理由 + 防腐烂"机制）。
4. **fail-closed 与可读性优先于省行**：宁可报错也不静默默认；错误消息必须能直接改数据（带位置、
   带建议、一次给全）。数据文件里的历史迁移注释（"v13 起…""已删除…"）一律移出，进归档文档。

## 4. 参考设计

| 参考 | 借用点 |
| --- | --- |
| LLVM TableGen（[语言参考](https://llvm.org/docs/TableGen/ProgRef.html)、[FOSDEM 讲稿](https://archive.fosdem.org/2019/schedule/event/llvm_tablegen/attachments/slides/3304/export/events/attachments/llvm_tablegen/slides/3304/tablegen.pdf)） | `multiclass`/`defm` 参数化实例化、`let` 逐实例覆盖、`include` 组合、`InstAlias` 与伪指令、"只给汇编器用"的标记 |
| Cranelift ISLE（[crate](https://crates.io/crates/cranelift-isle)） | 选择规则写成**规则语言**（模式 + if-let + 优先级 + 可复用规则），规则与匹配器分离 |
| QEMU decodetree（[文档](https://www.qemu.org/docs/master/devel/decodetree.html)、`scripts/decodetree.py`） | **按宽度分组**的定宽解码（多宽度并存的现实写法）、字段抽取、伪指令 |
| Ghidra SLEIGH（本仓库已借鉴其位域风格） | 上下文/变长解码的字段-上下文分离（只借"宽度分组 + 上下文变量"的组织方式） |

Cranelift ISLE 与 decodetree 的页面本次抓取超时（网络受限），以上按已知设计要点引用；落地时以官方文档核对措辞。

## 5. v18 语法（逐节变更）

### 5.1 文件头与版本

```toml
schema = 18                              # 必填；不匹配即报错并给出迁移指引
name = "arm64_v12"                       # 顶层（原 [meta].name）
include = ["common/rv_base.toml"]        # 可选：多文件组合（见 5.7）
```

`[meta].version` 改为 **ISA 自己的版本串**（自由字符串，与 schema 无关）；生成器模块 `src/v12/` 改名 `src/schema/`。

### 5.2 `[[templates]]` — 参数化指令（唯一复用机制）

> **状态**：S2 初版按"参数域等长按下标 zip"（`params`）实现，**S2c 改为 `body` + `rows`
> 并删除 `[[families]]`/`[[aliases]]`**——三个定义指令的模块并存是使用负担，只保留一个；
> 原 `[[families]]` 变体块折成 `rows` 的一行，原 `[[aliases]]` 变成指令属性 `ref`。
> 决策过程与实测见 §7 的"S2c 设计修正"。

```toml
# arm64：X/W 对 → 一条模板两行
[[templates]]
name = "ADDIMM"
body = { ref = "add", form = "ALUIMM", opcode = "{opcode}", \
         ops = ["dst:{slot}:out", "src:{slot}", "imm:imm12u"], \
         asm = "add {dst}, {src}, #{imm}" }
rows = [
  { inst = "ADDIMMX", slot = "r64", opcode = 0x91, roles = ["frame_free"] },
  { inst = "ADDIMMW", slot = "r32", opcode = 0x11 },
]
```

```toml
# riscv：S/D 对（助记符后缀由 {p.lower} 派生，不再需要并排小写列）
[[templates]]
name = "FADD"
body = { form = "R", opcode = 0x53, fields = { funct3 = 0, funct7 = "{funct7}" }, \
         ops = ["dst:fpr:out", "src:fpr", "src2:fpr"], \
         asm = "fadd.{p.lower} {dst}, {src}, {src2}" }
rows = [
  { inst = "FADD_S", p = "S", funct7 = 0x00 },
  { inst = "FADD_D", p = "D", funct7 = 0x01 },
]
```

```toml
# x86：一族不同助记符（原先 [[families]]）
[[templates]]
name = "SD_BIN"
body = { form = "SSE_RR", modrm = { reg = "dst", rm = "src" },
         fields = { prefix = 0xF2, w = 0 }, ops = ["dst:fpr:out", "src:fpr"],
         asm = "{inst.lower} {dst}, {src}" }
rows = [
  { inst = "MOVSD", ref = "movsd", opcode = 0x10, roles = ["fpr_mov_f64"] },
  { inst = "MINSD", opcode = 0x5D },
  { inst = "MAXSD", opcode = 0x5F },
]
```

**规则（写死、可校验）**：

- `rows` 每行一条指令，`inst` 必填且全 ISA 唯一；`body` 可省略（= 一组各自独立的指令）。
- 行键三类，**只由 `body` 决定**：body 里有同名键 ⇒ 覆盖（表递归合并）；body 没有但被
  body 字符串用 `{键}` 引用 ⇒ 纯参数（只插值、不进指令字段）；其余 ⇒ 指令字段（拼错由
  `Instruction` 的反序列化点名拒绝）。
- 插值：`{键}`（该行取值）、`{键.lower}`、`{inst}`（实例名）；**整串恰为占位符 ⇒ 保留类型**
  （`opcode = "{opcode}"` 仍是整数）；不是本行键的 `{…}` 原样留着（内层 asm 占位符）。
- `ref` 是普通指令字段：写在 `body` = 全模板共用（多态分派），写在行 = 该行 1:1 引用名。
  取代 `[[aliases]]`——多条指令共用同一个 `ref` 就是多态引用。
- 展开顺序 = 模板声明序 + 行序 ⇒ 生成的 `Inst` 枚举与编码臂顺序稳定可复现。
- 完整性校验：`rows` 非空、`inst` 非空唯一、`ref` 非空且不与指令名冲突、指令至少要有一个
  编码来源（`opcode`/`fields`/`opcode_reg`/`modrm`/`vex`/`evex`/`imm`）。
- 两种等价书写都支持：`rows = [ { … }, … ]`（推荐，一行一条）与 `[[templates.rows]]`。

### 5.3 条件码数据化（删除 x86 硬编码）— **已落地（S3b）**

```toml
# [conventions.cond] 的键 = 本 ISA 汇编/反汇编可见的条件名；每条给编码 + 它实现哪个 IR 条件
[conventions.cond]
b   = { code = 2, ir = "ult" }
c   = { code = 2 }              # 同码别名（渲染取同码字母序最小名）
e   = { code = 4, ir = "eq" }
z   = { code = 4 }
eq  = 4                         # 简写 = { code = 4 }，ir 取键名（键名恰是 IR 条件名时）
```

- 一张表服务三处：汇编解析（cond 槽按名）、反汇编渲染（码 → 字母序最小名）、
  lowering 的 `{cc}`（**按 `ir` 字段**查，不再按名猜）；
- `lowering.rs` 的 x86 setcc 硬编码表与 `asm.rs` 的 `cond_default()`（x86 16 项缺省）
  已删除；宿主只留"IR 条件码 → 条件名"（`forge_ir::intcc_name`），生成代码里不出现
  `IntCC`（通用性守卫的 2 条欠账随之删除，**白名单清空**）；
- 校验：`code ≤ 15`、`ir` 必须是 10 个规范名之一且无重复映射、`cond` 槽需要表、
  **用了 `{cc}` 就必须映射全 10 个 IR 条件**（否则运行期静默退化成 0）；
- arm64 由此获得全 16 条件（S3c 落地 `b.cond`）。

### 5.4 `[[reloc]]` — 重定位数据化（取代 `global_reloc` 枚举）— **已落地（S3d）**

```toml
[[reloc]]
name = "abs64"                  # 指令引用：reloc = "abs64"
semantics = "absolute"          # 宿主语义（有限、ISA 无关）：absolute | pc_relative
slot = "imm64"                  # 绑定到哪个操作数槽（须是 imm 槽）
addend = 0                      # 可选：重定位值的链接期加减
```

- 删除 `GlobalReloc{Abs8,PcrelHi,PcrelLo}` 与 `Instruction.global_reloc`，改
  `Instruction.reloc: Option<String>`；指令只写引用名；
- **宿主语义集合就是 `RelocKind` 的两态**：`absolute` → `Absolute(ceil(槽宽/8))`
  （fixup = 指令末尾该槽的字节区间）、`pc_relative` → `Relative(指令字长, 0)`
  （fixup = 指令起始）。计划里列的 `hi20`/`lo12`/`got`/`tls_*` 属于"新增语义才需要
  宿主代码"的那一侧——等真有 ISA 用到再加，届时也是往这个枚举里加一个变体，
  而不是把 ISA 特有形状塞进生成器；
- ISA 特有的**位段写入**仍在该 ISA 的 reloc patcher 里（arm64 写 imm26/imm19、
  riscv 按 opcode 0x17/0x13 分写 hi20/lo12）——这是"数据之外唯一的宿主代码"。
- 校验：名字非空唯一、`slot` 已声明且是 imm 槽、指令引用的名字必须在表里、
  该指令确实有那个槽的操作数；错误码 `DSL-RELOC`。

### 5.5 `[[pseudo]]` — 汇编器伪指令（v18 S3e）— **已落地**

```toml
[[pseudo]]
name = "li"                     # 汇编可见的助记符（不得与指令助记符重名）
params = ["rd", "imm"]          # 位置实参名（emit 里用 `{名字}` 引用）
emit = [                        # 至少一行；每行都是普通汇编文本
  "lui {rd}, ({imm} + 0x800)",
  "addi {rd}, {rd}, ((({imm} + 0x800) & 0xfff) - 0x800)",
]
```

- 展开是**汇编器层**行为：`parse_insts` 遇到以伪指令名开头的行 → 按**顶层逗号**
  切分实参（`()`/`[]` 内不算）→ 逐行把 `{参数}` 换成实参文本 → 交给同一套汇编器
  装配；emit 行可以是**别的伪指令**（递归展开，深度上限 16）；
- emit 里的算术由**既有表达式求值器**求值（`+ - * / % << >> & | ^ ~`、括号、
  `.equ`）——不引入第二套表达式语言；实参是文本，参与算术时要自己加括号；
- 单条 `assemble()` 只接受展开成 1 条的伪指令，多条要用 `parse_insts`（错误消息指引）；
- 校验：名字非空/唯一/不得与指令助记符重名；`params` 非空唯一；emit 行首词必须是
  指令助记符或别的伪指令名；`{…}` 必须是声明的参数且每个参数都用到；
- **未实现**（如实记录，不做半成品）：按谓词分派同名多条（汇编期没有 IR 属性可判）、
  `pseudo_fold`（反汇编折叠回伪指令——那是指令级模式识别，与 `[[pattern]]` 同类问题）。

### 5.6 `[[derive]]` — 派生谓词属性（v18 S3f）— **已落地**

```toml
[[derive]]
name = "is_64"
expr = { eq = ["rs1_width", 64] }

[[lowering]]
op = "Iadd"
when = { eq = ["is_64", 1] }
insts = ["ADD64 {out}, {0}, {1}"]
```

- `expr` 复用结构化谓词（同一份解析与校验）；派生值 = 1/0，用 `eq`/`ne`/`in` 引用；
- 展开在**解析期**：生成期的 `__attr` 只多一个派生臂（判定走核心属性表），
  谓词判定逻辑一行未改；**没有 `[[derive]]` 时生成的属性表与引入前逐字相同**；
- 名字不得与核心属性重名；可出现在 `vary` 里（与核心属性同待遇）；
- **派生不能引用派生**：属性名出现在值位置（`eq = ["a", 0]`）的代换语义不唯一，
  与其发明规则不如拒绝并提示；
- 计划原列的"数值派生"（`expr = { sub = ["imm0", 4] }`）**未实现**：`Pred` 里没有
  算术节点，加它要同时动解析器与两条求值路径（`pred::eval` 与
  `compile_pred_guard`）——留待真有 ISA 需要时按同一份 AST 扩展。

### 5.7 指令宽度三态（新增能力，S4 已落地）

```toml
[encoding]
kind = "fixed"        # fixed | mixed | prefix_scan
bits = 32
# kind = "mixed" 时：widths = [16, 32]（解码按宽度分组尝试）
# kind = "prefix_scan" 时：max_len = 15，prefixes = [ { byte = 0x66, effects = ["opsize16"] }, … ]
```

`Instruction`/`[[templates]]` 可写 `width = 16`（缺省 = `[encoding].bits`；`mixed` 时必填或按 form 给）。
定宽解码把现有位级 trie 扩展为**按宽度分组 + 组内 trie**；`mixed` 按 `widths` 顺序尝试、首个完整匹配即停。
新增夹具 `demo_mixed16_32_v12.toml` 证明通用性。

**落地时的取舍（与本节草案的差异）**：

- `prefixes` 前缀效果表**未做**：x86 的前缀语义已在 `[conventions.prefix_scan]` +
  `vlen.rs` 里，S4 只需把"最长长度"换成 `max_len`；再造一张 `[encoding].prefixes`
  是同一事实两处声明（真要引入应作为独立的 S 片，届时删掉旧表）。
- `mixed` 的短/长判别位是 **ISA 自己的责任**：`decode` 按字长升序尝试、首个完整
  匹配即停，若短编码不把长编码的低位排除掉，长指令会被误判成短的（夹具用低 2 位
  `sel` 判别；文档与夹具注释都写明）。校验器无法通用地证明互斥，故不做假保证。
- `decode_partial` 的截断阈值：`fixed` = 字长、`mixed` = **最短**字长，
  否则"长度够但无匹配"会被当成截断（那是非法字节流）。
- 能力集：`fixed` → `fixed_inst_size = bits/8 = min = max`；`mixed` →
  `fixed_inst_size = 0`、min/max = 最窄/最宽、`variable_length = true`；
  `prefix_scan` → min = 1、max = `max_len`。
- 省略整个 `[encoding]` 保持"骨架文档合法、生成期报缺字长"（旧
  `default_inst_width` 缺省时的行为），不静默当 32。

### 5.8 组合与部件（新增）

```toml
include = ["common/rv_base.toml"]      # 深合并：数组按 include 序追加；同名标量冲突报错
[[override]]                            # 显式覆盖（取代"后出现的赢"这种隐式规则）
key = "meta.endian"
value = "big"
```

多文件诊断带**来源文件名**。`isa_from_file!` 参数扩展：`name = "..."`（模块名覆盖）、
`parts = ["encode", "decode", "asm", "tm"]`（部件选择）、`krate = ...`（保留）。

## 6. 删除与改名总表

| 旧 | 新 | 说明 |
| --- | --- | --- |
| `[meta].name` / `[meta].version` / `[meta].mode` | 顶层 `name` / `schema` | `version` 退为 ISA 自己的版本串；`mode` 删除（无消费者） |
| `[meta].default_inst_width` / `variable_length` / `max_inst_len` | `[encoding].kind/bits/widths/max_len` | 三态 |
| `[meta].default_opsize` | `[encoding].default_opsize` | 归位 |
| `[[families]]` | `[[templates]]` 的 `body` + `rows` | 变体 = 一行，可覆盖任意指令字段（含 `ops`/`form`/enc 键） |
| `[[aliases]]` | 指令属性 `ref`（多条共用 = 多态） | 与指令名同池 + 冲突校验 |
| `Instruction.global_reloc` | `Instruction.reloc = "<[[reloc]].name>"` | 数据化 |
| `[conventions.cond]`（x86 名 → 码） | 同名节，键 = IR 条件名 | 删除 Rust 侧 x86 表与缺省 |
| `Instruction.when`（三份谱 0 处使用） | 删除 | 死键 |
| `[conventions.mem]`（v16，未文档化） | 保留并文档化 | 已是数据 |
| 生成器内 x86 缺省（cond / prefix_scan / `[abi]` / `value_fpr_width` / `vector_tiers`） | 必填或 fail-closed 报错 | 不再静默给别家形状 |

## 7. 切片计划

> 门禁沿用 `docs/plans/forge-ir-binary-serialization-plan.md` §5 的固定命令（fmt / clippy 两道 /
> workspace 测试 / 语料 / release check / doc / markdownlint），**外加**每片必跑：三架构 JIT 矩阵、
> arm64 与 riscv 编码黄金值逐字节不变、以及本片新增守卫。每片结束提交，同批推送。

| 片 | 范围 | 关键产出 | 证据/验收 | 叫停价值 |
| --- | --- | --- | --- | --- |
| **S0** 基线 | 不改语法 | 采集生成代码规模/编译时间/TOML 与测试行数/坏 spec 首错行为；新增 `generality_guard.rs`（扫 `src/**` 里的 ISA 名、寄存器名、cond 码；白名单 + 理由 + 防腐烂） | 数字进 `docs/performance/bench_baseline.md` §ir_dsl；守卫先红后绿（红：§2.4 的 x86 setcc 表） | — |
| **S1** 诊断与校验 | 不换语法 | `Diags`（code + span + msg + notes + suggestions，收集式，≤32 + 计数）；**声明索引**（一次预扫建"节 + 名字 → 精确 span"，替代 `source.find` 启发式）；`[emit]`/`[spill]` 引用名与占位符校验；错误码目录 | 坏 spec 矩阵 ≥30 例断言条数/位置/建议；把 `MOV64_RR` 写成 `MOV64_R` 必须**编译期**报并点名行号 | 立刻可用 |
| **S2** 参数化模板 | `[[templates]]` 取代 families + aliases | 迁移 arm64（38 对）、riscv（16 对）、x86（SSE 族与宽度变体） | 全指令编码逐字节不变；arm64 TOML −30%、riscv/x86 −10% 行数；新增"实例不可区分/名冲突/域不等长"负向用例 | 核心收益 |
| **S3** 数据化 | cond / reloc / pseudo / derive | 删 x86 硬编码；arm64 得 `b.cond` 全 16 条件；`[[reloc]]`、`[[pseudo]]`、`[[derive]]` | arm64 16 条件编码对照 `docs/reference/aarch64-encoding-ref.md` 黄金值 + 反汇编往返；x86 `cond` 语义不变；`generality_guard` 白名单清空 | 通用性实质提升 |
| **S4** 宽度三态 ✅ 已落地 | `[encoding]` + 逐指令 `width` | mixed 解码（按宽度分组 trie）+ 新夹具 `demo_mixed16_32_v12.toml` | 新夹具黄金字节全绿；x86/riscv/arm64 不回归 | 打开 RVC/Thumb 类 ISA |
| **S5** 降低语言升级 | 结构化 `insts` + `[[sequences]]` + 属性补齐 + pattern 统一 | `lowering.emit` 表形式（`inst`/`let`/`select`/`switch`）；共享序列；pattern 与 lowering 同一裁决序与死规则检测 | x86 220 条 lowering 下降 ≥15%；黄金值不变 | 中收益、风险最高（可延后） |
| **S6** 生成自测 | 生成 `__spec_tests`（`cfg(test)`） | 每指令 encode↔decode↔encode、disassemble↔assemble↔encode、立即数边界（min/max/min−1/max+1）、全宽度视图 | 覆盖 x86 197 / riscv 116 / arm64 89（100%）；命中缺陷即修并记录；样板测试可删减 | 每条新指令自动进回归网 |
| **S7** 工具链与文档 | 拆 crate + CLI + 文档 | `forge-isa-dsl`（普通 lib）+ `forge-dsl`（薄 proc-macro）；`forge-isa` CLI：`validate`/`explain`/`schema`/`fmt`/`diff`/`insts`；JSON Schema + `#:schema`；文档重写 | schema ↔ 文档 ↔ JSON Schema 三方针守卫；CLI 集成测试；`isa_from_file!` 新参数用例 | UX 与可维护性长期收益 |
| **S8** 生成物减薄（可选） | 静态逻辑下沉 `forge-codegen::runtime`，生成物只留表 + 分派 | 生成代码 token 数 −≥40%、`cargo check -p forge-codegen` −≥20% | 以 S0 基线对比；黄金值与矩阵不变 | 按度量决定 |

**建议顺序**：S0 → S1 → S2 → S3 → S4（以上均已落地）→ S6 →（S7）→ S5 →（S8）。
S4 提前到 S6 之前：宽度三态是**语法/生成期**的破坏性改动，越早定下来，S6 生成的
`__spec_tests` 与 S7 的 JSON Schema 才不用二次改写。

**S3 进度**：

- **S3a 已落地（2026-09-19）**：三处"别家常量兜底"改 fail-closed——`frame.rs` 的 `R10`
  scratch 缺省与 `MOV_RM8_R64` 指令名探测（改按 `roles = ["gpr_mov"]`）、`lowering.rs`
  两处 `X1` 返回地址寄存器缺省；缺声明一律生成期报错，且只在真的需要时要求
  （无栈参数 / call 指令没有返回槽的 ISA 不受影响）。生成代码**逐字节不变**（8 个模块
  dump 对照），新增两条负向用例（用真实 x86/riscv 谱删声明构造）。
- **S3b 已落地（2026-09-19）**：条件码数据化——`[conventions.cond]` 变成
  `名 → { code, ir? }`（含整数简写）；一张表服务汇编解析、反汇编渲染与 lowering 的
  `{cc}`；删掉 `lowering.rs` 的 x86 setcc 表与 `asm.rs` 的 `cond_default()`（x86 16 项
  缺省）；宿主新增 `forge_ir::intcc_name`（IR 码 → 条件名），**生成代码不再出现
  `IntCC`**，`generality_guard` 的 `ALLOWED` **清空**。新增 8 条条件码用例；
  x86 生成代码的 `{cc}` 映射与旧硬编码表逐条相同（`eq→4 / ne→5 / slt→12 / sle→14 /
  sgt→15 / sge→13 / ult→2 / ule→6 / ugt→7 / uge→3`），x86 JIT 矩阵 195/3/0（含比较
  用例，端到端跑过 `{cc}`）不变。
- **S3c 已落地（2026-09-19）**：arm64 条件码符号化 + `b.cond`——`[conventions.cond]`
  给出 A64 的 16 个条件名（+`hs`/`lo` 别名，无 `ir` 映射），CSEL 族的 `cond4` 槽改成
  `kind = "cond"`（汇编/反汇编写符号名，不再是 `#0`）；新增 `B.cond` 模板 16 行
  （助记符 `b.{cname}` 由行键插值、条件码是 `[3:0]` 的固定位域 `bcond`），14 个 A64
  合法条件逐个对照 `docs/reference/aarch64-encoding-ref.md` 的条件码表验证字节 + 汇编↔
  反汇编往返 + 别名同码；顺带**删除**原先那条错的 `BCOND` 存根（目标塞进 `rt=[4:0]`、
  条件恒 0 且落在 `[15:12]`、imm19 恒 0——不是合法 B.cond，也无人使用）。固定宽解码器
  补上 `cond` 槽的 `u8` 绑定（定宽 ISA 的第一个 cond 槽）。
- **S3d 已落地（2026-09-19）**：重定位数据化——`[[reloc]]`（`name`/`semantics`/`slot`/`addend`）
  取代 `GlobalReloc` 枚举与 `Instruction.global_reloc`，指令只写 `reloc = "<名>"`；
  宿主语义集合 = `RelocKind` 的两态（`absolute` → `Absolute(ceil(槽宽/8))`、
  `pc_relative` → `Relative(指令字长, 0)`），ISA 特有的位段写入仍在各 ISA 的
  reloc patcher；x86 `MOVABS_GLOBAL` → `reloc = "abs64"`、riscv `AUIPC_GLOBAL`/
  `ADDI_GLOBAL` → `pcrel_hi`/`pcrel_lo`。生成代码只在重定位臂上从 `ABS8`（= `Absolute(8)`）
  变成 `Absolute(8)`、addend 字面量 `0` → `0i64`——**语义等价**；三架构 JIT 矩阵
  （含 x86/riscv 的 GlobalAddr 用例）不变。新增 7 条 reloc 用例（解析、宽度来自槽宽、
  未声明名/坏槽/重名/槽不在操作数里/语义名非法）。
- **S3f 已落地（2026-09-19）**：派生谓词属性 `[[derive]]`——`expr` 复用结构化谓词，
  属性名可在 `when`（lowering/pattern）与 `vary` 里当谓词属性用（值 1/0），解析期展开、
  生成期只在 `__attr` 多一个臂；名字不得与核心属性重名、不得引用另一个派生（代换语义
  不唯一）。**没有派生时 8 个生成模块与 S3d 逐字节相同**（能力零成本）；新增 6 条用例。
- **S3e 已落地（2026-09-19）**：汇编器伪指令 `[[pseudo]]`——`params` + `emit` 文本级
  展开（顶层逗号切分实参、`{参数}` 代入、emit 可嵌套别的伪指令、深度上限 16）；
  算术复用既有表达式求值器，故**没有新增表达式语言**；riscv 的 `li` 是实际用例
  （`li x10, 0x1234` → `lui` 0x00001537 + `addi` 0x23450513，与 lowering 的
  `{iconst_hi20}`/`{iconst_lo12}` 同一套算术）；新增 6 条声明侧用例 + 2 条
  riscv 行为用例（含 0/-1/0x800/表达式实参/混排/参数个数错/单条 API 指引）；
  **没有伪指令的 ISA 不生成任何展开器代码**（x86/arm64 与 5 个夹具的 dump 只差
  `assemble()` 那 2 行新增文档注释，riscv 多 97 行展开器）。
- 至此 **S3 全部落地**（S3a–S3f）。

**S4 进度（宽度三态，2026-09-20 全部落地）**：

- `[encoding]` 三态取代 `[meta]` 的 4 个宽度散键（`default_inst_width` /
  `variable_length` / `max_inst_len` / `default_opsize`），并新增**逐指令 `width`**
  （`[[instructions]]` 与 `[[templates]]` 的行都能写）。
- `kind = "fixed"`：全 ISA 一个字长（= 旧 `default_inst_width` 语义）。三个发行 ISA
  与 5 个夹具的生成代码**逐字节不变**，只有两处预期差异：能力集 `fixed_inst_size`
  由 `0` 变成真实字长（`4u32`，以前定宽 ISA 报 0 = "未知"）、文档字符串里的
  `[meta].default_inst_width` 改名 `[encoding].bits`。
- `kind = "mixed"`（**新能力**）：按 `widths` **升序**分组——每个字长一棵 trie，
  "首个完整匹配即停"（短编码优先，RVC/Thumb 同构）。新夹具
  `demo_mixed16_32_v12.toml`（低 2 位判别短/长编码，`sel = 0` vs `3`）+ 5 条用例
  （黄金字节、分组解码与消费字节数、编解码往返、`decode_partial` 截断阈值 =
  最短字长、能力集 = `variable_length` + min/max = 最窄/最宽）。
- `kind = "prefix_scan"`：x86 走原有 `vlen.rs` 变长路径（`max_len`，缺省 15），
  生成代码不变。分派从"`variable_length` 二分"改成按 `is_prefix_scan()` 三态分派
  （`fixed`/`mixed` 共用定长位域编解码）。
- 校验期**结构性互斥**：`fixed` 写 `widths`/`max_len`、`prefix_scan` 写 `bits`、
  `mixed` 写 `max_len`、逐指令 `width` 不在 `widths` 里或与 `bits` 不一致——全部
  编译期报错；诊断新增 `DSL-ENCODING`。**省略整个 `[encoding]`** = `fixed` 且无
  `bits` 的骨架文档（解析/校验通过、生成期 `inst_bytes()` 报"bits 缺失"）——
  省略整段不会被静默当成定宽 32；此时逐指令 `width` 也不能替代 `bits`。
- 迁移面：`isa/{x86,riscv64,arm64}_v12.toml` + 5 个 demo 夹具 + `forge-dsl` 测试内
  30 处内联 TOML。
- 证据：`cargo test -p forge-dsl` 175 + 2 + 1 全绿；workspace 测试 96 个 target
  全绿（`-j 1`）；三架构 JIT 矩阵 **x86 195/3/0、riscv64 131/67/0、arm64 23/175/0**
  （与 S3 完全相同）；生成代码对拍（`target/s4before_forge_gen_*.rs` ← S3 末
  状态、`target/s4after_forge_gen_*.rs` ← 本片）8 个模块差异仅上述两处。
- **顺带发现（未修，超出本片范围）**：`cargo check --workspace --release
  --all-targets` 在 `forge-rustc` 上报 `cannot find function assert_assignable`
  ——`types.rs` 里该函数是 `#[cfg(debug_assertions)]`，而 `lower/place.rs` 无条件
  调用；两文件最后改动是 2026-09-04 `ac612d1`，与本片无关（CI 只跑 debug 的
  `cargo check -p forge-rustc`）。故本片的 release 门禁按
  `--exclude forge-rustc` 跑。

**最小可用子集**：S0 + S1 + S2 + S3；**可在 S3 后叫停**并保留全部价值。

**S2 进度（S2a–S2c 全部落地，2026-09-19）**：机制 + 校验 + 测试已落地，三个发行 ISA 与全部夹具
已迁移到**唯一机制** `[[templates]]`（`body` + `rows`）+ 指令属性 `ref`。

| ISA | TOML 行数 | 迁移内容 | 等价证据 |
| --- | ---: | --- | --- |
| arm64 | 871 → **891（+2.3%）** | 32 条模板（64 行实例）；无 families/aliases 可删 | 生成代码**逐字节相同**；黄金 12+6；矩阵 23/175/0 |
| riscv64 | 1,899 → **1,802（−5.1%）** | 4 条 `[[families]]`（38 变体）折成 rows；15 条模板按 rows 重写（顺带删掉 `pl` 冗余列） | 生成代码每函数 token 多重集相同（只差顺序）；黄金 12+4；矩阵 131/67/0 |
| x86 | 3,887 → **3,678（−5.4%）** | 10 条 families（62 变体）折成 rows；23 条 `[[aliases]]` → 成员指令的 `ref`；1 条模板按 rows 重写 | 生成代码每函数 token 多重集相同（只差顺序）；黄金 61 例；矩阵 195/3/0 |

**等价自检**（`target/unify.py`，迁移脚本自带）：用 Python 复刻旧展开（instructions → 族变体 →
模板实例）与新展开（instructions + templates），逐指令比较 `form`/`opcode`/`fields`/`ops`/`asm`/
`roles`/`when`/编码键，并比较**引用名 → 成员有序列表**——三 ISA 全部相等（197/116/89 条指令）。
生成代码层面：arm64 与 5 个夹具**逐字节相同**；riscv/x86 每个函数的 token 多重集完全相同
（差异只在 `Inst` 枚举/编码臂/解码 trie 的顺序，原因见 §12.8）。

> **S2c 设计修正（评审意见 + 实测）**：`[[templates]]`/`[[families]]`/`[[aliases]]` 三个
> "定义指令的模块"并存是**使用负担**（同一件事三种写法、三套校验、三种诊断前缀），
> 因此三者合并为 `[[templates]]` 一个机制：
>
> | 旧机制 | 现在的表达 | 为什么能合并 |
> | --- | --- | --- |
> | `[[families]]` + variants | `body`（族级字段）+ `rows`（每个变体一行）；助记符继承写 `asm = "{inst.lower} …"`，带点助记符（`fadd.s`）用 `{p.lower}` 插值 | 行的键空间 = 指令字段空间，"变体只能覆盖 `fields`/`opcode`/`roles`"的限制消失 |
> | `[[aliases]]`（`name` + `insts`） | 成员指令上写 `ref = "名字"`；多条共用同一 `ref` = 多态 | `ref` 与指令名同池，引用表由"名字 ∪ `ref`"构建，与旧别名表**逐项有序相同** |
> | `[[templates]]` + `params` + `overrides` | 同一张 `rows` 表：参数取值写成行键，逐行补丁直接写进那一行 | 平行数组按列 zip 虽紧凑，但要逐行对齐、加一条变体改 3 个数组——正是"使用负担"的来源 |
>
> 代价与收益如实记：arm64 **+2.3%** 行数（2 行指令原先用平行数组写得极紧凑，现在一行一条
> 自解释，换来"不需要逐列对齐"）；riscv **−5.1%**、x86 **−5.4%**（族变体块每个 2–3 行 → 一行）。
> x86 剩余的同 asm/同 ops 组仍差在**编码键**（`form` 预设有无、`opsize`、内联 vs preset），
> 那是**不同编码**而非可参数化的取值，统一它们属于语义重构（风险大于收益，不做）。

## 8. 迁移

1. **迁移面**：`isa/{x86_v12,riscv64_v12,arm64_v12}.toml`（实测 6,235 行）+ 5 个夹具 +
   生成器 15,976 行（不含单测）+ 生成面手写测试 2,952 行（仅删样板部分）。
2. **顺序**：先落 v18 模型与校验（新语法可解析）→ 逐 ISA 迁移（每 ISA 一次提交，黄金值比对）→
   删旧语法代码与旧夹具 → 文档重写 + 旧语法归档。
3. **等价判据**（防"迁移顺手改语义"）：
   - 每 ISA 的**全指令编码**与迁移前逐字节一致：用 S0 采集的全指令样本 dump（指令 + 固定操作数样本 → sha256）迁移后重跑比对；
   - arm64/riscv/x86 既有黄金测试与 JIT 矩阵全绿（除 S3/S4 明确新增的能力）；
   - `forge-isa explain` 输出的"展开后有效规格"在迁移前后 diff 为空（除有意删除的死键与缺省）。
4. **数据清理**（同批）：TOML 里的历史迁移注释（"v13 起…""已删除…""历史行为"）全部移出，进归档文档。
5. **风险**：x86 单文件最大（3,356 行、146 + 51 条指令）最易出错；对策 = 分节迁移 + 每节跑全指令字节比对；
   x86 的 lowering（220 条）在 S5 之前不动。

## 9. 边缘情况与失败模式

| 场景 | 期望 |
| --- | --- |
| `rows` 为空 / `inst` 为空 / 实例名与手写指令或别的实例重名 | 编译期错误，指向模板声明（块内精准到该行/该 `inst`） |
| 行键既不在 `body` 里、也没被 `{键}` 引用 ⇒ 被当指令字段；拼错 | 编译期错误，消息带模板名 + 实例名 + 未知字段名 |
| `ref` 为空 / 与指令名冲突 | 编译期错误（`validate_references`） |
| 指令没有任何编码来源（`opcode`/`fields`/`opcode_reg`/`modrm`/`vex`/`evex`/`imm`） | 编译期错误（原只查族变体，S2c 起对所有指令生效） |
| `[conventions.cond]` 缺项，`{cc}` 遇到未覆盖条件 | 编译期错误点名缺失条件（不再静默 0） |
| `[emit]`/`[spill]` 引用不存在的指令/引用名/占位符 | 编译期错误 + 行号（S1） |
| `[encoding].kind = "mixed"` 但某指令无 `width` 且无法从 form 推导 | 编译期错误 |
| mixed 解码：短宽度前缀与长宽度指令前缀冲突 | 按 `widths` 顺序尝试，且校验器报"解码歧义" |
| `include` 循环 / 同名标量冲突 / 缺文件 | 编译期错误，消息含来源文件与 include 链 |
| 伪指令展开递归（`li` → `li`） | 深度上限 + 明确错误 |
| reloc 语义未知（宿主不支持） | 编译期错误列出可用语义集 |
| 坏 spec 有多条错误 | 一次列全（≤32），末尾"还有 N 条" |

## 10. 显式假设

1. **允许重写三个 ISA 的 TOML**（实测 6,235 行）——用户已明确"允许破坏性更新、无需兼容旧版本"。
2. **`forge-codegen` 的运行期面可对齐**：删除 `GlobalReloc`、新增 reloc 语义集、S8 的生成物减薄需要改
   `crates/backend/forge-codegen/src/machine/*`；这些改动不改变既有指令编码（黄金值守护）。
3. **补齐条件分支能力会改变 arm64 测试计数**（更多用例从 skip 转绿）；方案只承诺"不回归 + 上升"，
   具体数字落地实测回填。
4. **不新增依赖**：JSON Schema 由手写发射器 + 三方一致性守卫实现（不引 `schemars`）；CLI 手写参数解析
   （不引 `clap`）。若用户愿意放宽依赖策略，S7 可换成熟库简化。
5. 参照设计（TableGen/ISLE/decodetree）本次按已知要点引用（抓取超时），落地时以官方文档核对。
6. 归档文档只在头部声明现行替代入口，不参与持续维护。

## 11. 文档落地清单

- 本方案 `docs/plans/forge-dsl-v18-plan.md`（已落地）
- 重写 `docs/reference/isa-dsl.md`（v18 语法：一节一概念 + 迁移对照 + 生成代码契约）
- 新增 `docs/guides/isa-dsl-tutorial.md`（30 分钟接入一个小 ISA：寄存器组 → 位域 → 模板 → asm → 自测）
- 新增 `docs/reference/isa-dsl-errors.md`（错误码目录：码、含义、典型修法）
- 归档 `docs/archive/isa-dsl-v12-v17.md`（旧语法 + v15 的 S1–S6 + v16/v17 增量）
- 更新 `CLAUDE.md` 的 ISA-DSL 节（去掉 v12/v15 混述）、`CHANGELOG.md`、`docs/README.md` 索引
- `docs/performance/bench_baseline.md` 增 DSL 段（S0 基线：生成代码规模、编译时间、诊断响应）

## 12. 附录：S0 基线实测

命令与日期：本机 2026-09-19；`cargo clean -p forge-codegen` 后 `FGE_DEBUG_GEN=1 cargo check -p forge-codegen`。

### 12.1 规格规模（`isa/*.toml`）

| 文件 | 行数 | `[[instructions]]` | families 变体 | `[[aliases]]` | `[[lowering]]` | `[[pattern]]` | slots | forms |
| --- | ---: | ---: | ---: | ---: | ---: | ---: | ---: | ---: |
| `x86_v12.toml` | 3,356 | 146 | 51 | 25 | 220 | 2 | 15 | 19 |
| `riscv64_v12.toml` | 1,754 | 78 | 38 | 0 | 110 | 0 | 7 | 11 |
| `arm64_v12.toml` | 1,125 | 89 | 0 | 29 | 19 | 0 | 9 | 19 |
| 合计 | **6,235** | 313 | 89 | 54 | 349 | 2 | 31 | 49 |

夹具：`demo_v12` 232 行、`demo8_v12` 233 行、`demo_inst8/12/100_v12` 107/81/82 行。

### 12.2 生成代码规模（`FGE_DEBUG_GEN=1` dump，未格式化 token 串）

| 生成模块 | 字节 | 行 |
| --- | ---: | ---: |
| `forge_gen_x86_v12.rs` | 2,986,514 | 40,317 |
| `forge_gen_riscv64_v12.rs` | 1,263,470 | 21,638 |
| `forge_gen_arm64_v12.rs` | 644,018 | 9,241 |
| 合计 | **4,894,002** | **71,196** |

### 12.3 编译时间与手写测试规模

| 指标 | 实测 |
| --- | --- |
| `cargo clean -p forge-codegen` 后 `cargo check -p forge-codegen`（宏重跑 + 生成 4.9 MB 代码） | **36.7 s / 37.9 s**（两次） |
| 同上，随后空跑 `cargo check` | **0.4 s**（三次 0.81 / 0.40 / 0.41 s；紧跟 cold 的第一次 2.6 s） |
| 生成器 + 模型（`src/**` 除 `tests.rs`） | 15,976 行 |
| `src/v12/tests.rs`（crate 内单测） | 2,713 行 |
| 生成面手写测试（`*_v12_tests.rs`/`decoder_smoke`/`encoder_fuzz`/`asm_*`/`decoder_enhance`/`v12_integration`） | 2,952 行 |

> 备注：首轮测量曾得到 96.9 s / 70.6 s 的偏大值（与后台任务重叠）；上表是单独重跑的稳定值。

### 12.4 诊断行为基线（S0 取证）

命令：`cargo test -p forge-dsl --lib s0_baseline -- --nocapture`。夹具见
`crates/frontend/forge-dsl/src/v12/s0_baseline_tests.rs`（该文件**故意断言现状**，
S1/S3 落地时必须改写——它是"基线钉"）。

| 坏 spec | 现状输出 | 结论 |
| --- | --- | --- |
| 三处**互相独立**错误（未知 form + 指令名重复 + 未知 `when` 属性） | 只报第一条（`28:1: [[instructions.ADD]]: form 'NOPE' is not declared`） | **fail-fast**：一次只报一条（§2.7） |
| 无名节的错误（`[stack].align = 0`） | `1:1: v12 validation: [stack].align must be > 0` | 抽不出声明名 ⇒ **退化为 1:1**，丢失跳转（§2.7） |
| `[emit.prologue].insts = ["NO_SUCH_INST …", "@nope", "{bogus}"]` + `[spill.GPR]` 引用不存在的指令 | `<ok>`（**通过校验**） | `[emit]`/`[spill]` 名字与占位符**未校验**（§2.5） |
| 未知 form / 未知 `when` 属性 | 均带 `行:列` 与声明名，消息点名了出错的名字 | 已有基础可用，S1 补齐 span 精确性与 did-you-mean |

### 12.5 通用性守卫（S0 新增）

`crates/frontend/forge-dsl/tests/generality_guard.rs`：从 `isa/*.toml` **读出** 397 个指令名与
225 个寄存器名，再扫 `src/**`（除 `tests.rs` 与 `cfg(test)`）里的字符串字面量。

- **首次运行（空白名单）= 失败 12 处**，其中 5 处为真欠账、7 处为抽取器误报；
- 修正抽取器（`prefix` 只在 `[reg.*]` 块内取、剔除 IR 类型常量名 `F16/F32/...` 与类名撞名）后，
  真欠账 5 行（含 `lowering.rs` 的 `IntCC` 两行与 `"X1"` 缺省、`frame.rs` 的 `R10` 回退与
  `"MOV64_RM8_R64"` 回退）进白名单并注明"由 S3 删除"；
- 白名单防腐烂：每条必须被命中，S3 收尾时必须清空。

### 12.7 S2c 统一迁移实测（arm64 / riscv / x86）

日期：2026-09-19。迁移脚本 `target/unify.py`（`check` 模式先做模型级等价自检，`apply` 才写文件）。

| 指标 | arm64 | riscv64 | x86 |
| --- | ---: | ---: | ---: |
| TOML 行数（前 → 后） | 871 → **891**（+2.3%） | 1,899 → **1,802**（−5.1%） | 3,887 → **3,678**（−5.4%） |
| `[[instructions]]` 块（前 → 后） | 25 → 25 | 48 → 48 | 142 → 142 |
| `[[templates]]` 条 | 32 | 19 | 11 |
| 模板行（实例数） | 64 | 68 | 55 |
| `[[families]]` 条（前 → 后） | 0 → 0 | 4 → **0** | 10 → **0** |
| families 变体数（前 → 后） | 0 → 0 | 38 → **0**（= rows） | 62 → **0**（= rows） |
| `[[aliases]]` 条（前 → 后） | 0 → 0 | 0 → 0 | 23 → **0**（`ref` 注入 35 处） |
| 指令总数（等价自检） | 89 | 116 | 197 |
| 生成代码 | **逐字节相同** | 每函数 token 多重集相同 | 每函数 token 多重集相同 |
| 黄金测试 | 12 + 6 通过 | 12 + 4 通过 | 13 + 11 + 20 + 3 + 14 通过 |
| JIT 矩阵 | 23/175/0 | 131/67/0 | 195/3/0 |

等价判据分三层（越靠前越强）：

1. **模型级**（`target/unify.py` 自检）：旧展开 vs 新展开逐指令比较 `form`/`opcode`/`fields`/
   `ops`/`asm`/`roles`/`when`/编码键，并比较**引用名 → 成员有序列表**——三 ISA 全等；
2. **生成代码级**：迁移前生成物来自 **HEAD 工作树**（先 `git stash` 掉 DSL 改动再 dump，
   否则基线是脏的——第一版基线就踩了这个坑）；arm64 与 5 个夹具逐字节相同，riscv/x86 用
   "按函数比较 token 多重集"（顺序差异不影响多重集，内容差异必然暴露）——全部相同；
3. **行为级**：黄金测试 + 三架构 JIT 矩阵 + 语料（`llvm_assembler_compat` 11 /
   `corpus_roundtrip` 90 / `display_llvm` 1）。

注：`[[templates]]` 展开在**解析期**（与 `lowering.vary` 同一层），因此下游（校验/生成）
只看到普通指令——`codegen` 一行未改，这是"零行为变化"的结构性原因。

### 12.8 迁移后生成代码的顺序差异（为什么不是逐字节相同）

旧实现的展开顺序是 **手写指令 → 模板实例 → 族实例**（`expand_templates` 先把模板实例
`extend` 进 `instructions`，`collect_inst_infos` 再追加族变体）；新实现是 **手写指令 →
按文件序的模板（族已折成模板）**。riscv 的族在文件前部（第 175 行）、浮点模板在后部
（第 1,826 行），于是两边的 `Inst` 枚举、编码臂、解码 trie 中"族实例 vs 模板实例"的
相对次序互换 ⇒ 生成代码文本不同。

这不改变语义，理由有三条，都可复核：

- **集合相同**：`Inst` 名字集合逐 ISA 相同（89/116/197，与 §12.7 的行数一致）；
- **每函数 token 多重集相同**：说明只有语句顺序/换行变化，没有增删（见 §12.7 第 2 层）；
- **顺序敏感的两处都验过**：(a) 引用名 → 成员有序列表逐项相同（多态分派候选序）；
  (b) 解码/编码的 `match` 分支互斥性由各指令自己的编码键保证（如 x86 `MOVSX` 要求
  `!F2 && !F3`，`MINSD` 要求 `F2`，字节 opcode 也不同），黄金测试与 JIT 矩阵覆盖。

arm64 与 5 个夹具因为"模板本来就在手写指令之后"而没有这个差异，生成代码逐字节相同。

### 12.9 S1 落地后的诊断行为（与 §12.4 对照）

| 场景 | S0（改造前） | S1（现在） |
| 三处独立错误（未知 form + 重名 + 未知 `when` 属性） | 只报 1 条 | **3 条一次给全**，各带 `路径:行:列: 码` |
| 重复声明 | 只报"重名"，位置落在首处声明 | 指向**后出现**的那一处 + 附注 `同名声明也出现在 行:列` |
| 未知 `when` 属性 | 位置到 `[[lowering.OP]]` 声明行 | 精确到**出错的 `when = …` 键行** |
| `[emit]`/`[spill]` 引用不存在的指令/伪指令/占位符 | **通过校验**（只查非空） | 编译期错误 + 键行位置（错误码 `DSL-EMIT`/`DSL-SPILL`） |
| 无名节的错误（`[stack].align = 0`） | 退化为 `1:1` | 落在该节块内（节头 / 键行） |
| 错误条数上限 | 无（一次一条） | 32 条 + `（另有 N 条错误未列出）` |

交付物：`v12/diag.rs`（`Diag`/`Diags`/`DeclIndex`：节级错误码、块内精准定位、附注、上限）、
`validate_all`（收集式驱动 + 指令/lowering 逐条收集）、`validate_emit_all`/`validate_spill_all`
（引用名 + 伪指令 + 占位符 + `base` 寄存器全校验）、`src/v12/diag_matrix_tests.rs`（**30 例**矩阵 +
多错并列 + 附注 + 上限 + 头条精确位置）、`docs/reference/isa-dsl-errors.md`（错误码目录）。
门禁：fmt/clippy 两道/`cargo test -p forge-dsl`（135 + 2 + 1）/`cargo clean -p forge-codegen`
后 `cargo check -p forge-codegen`（**三份发行谱通过新校验**，34.7 s）/三架构矩阵/语料/doc/markdownlint。
