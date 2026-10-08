# ISA-DSL v21 重设计方案

> [progress] 2026-10-08 设计提案（待评审；实施另开）。
> 现行语法 = v18（`docs/reference/isa-dsl.md`）；v19/v20 是 v18 之上的增量，本方案是**下一次破坏性重设计**。
> 本文所有现状断言取自本轮只读审计（`crates/frontend/forge-isa-dsl/src`、`isa/*.toml`、`docs/reference/isa-dsl.md`），
> 引用的行号**随迭代漂移，以符号名为准**。**本文只是设计，不改任何源码**。

## 目录

- [1. 范围与判据](#1-范围与判据)
- [2. 现状证据](#2-现状证据)
- [3. 外部参考](#3-外部参考)
- [4. 设计公理](#4-设计公理)
- [5. 表面结构（节表）](#5-表面结构节表)
- [6. 逐节语法](#6-逐节语法)
- [7. 后门清除表](#7-后门清除表)
- [8. 破坏性清单（键级）](#8-破坏性清单键级)
- [9. 覆盖论证](#9-覆盖论证)
- [10. 迁移映射与工作量](#10-迁移映射与工作量)
- [11. 实施切片路线](#11-实施切片路线)
- [12. 边缘情况与失败模式](#12-边缘情况与失败模式)
- [13. 假设与可评审决策](#13-假设与可评审决策)
- [14. 附录：审计取证](#14-附录审计取证)

---

## 1. 范围与判据

### 1.1 本方案回答什么

一次**不兼容旧版本的** ISA-DSL 表面重设计：删掉"为当前这三份谱开的缺省与特例"，
把编码/文本/选择三层改成"一处声明、类型驱动、无位置隐含"，并让**新增第四个 ISA 不再需要改编译器**。
载体仍是 TOML（已拍板）；不引入自研文本语法。

### 1.2 成功判据

| # | 目标 | 硬判据（可核对） |
| --- | --- | --- |
| D1 | 可读性 | 顶层节 21 → 13；并存机制（`forms` 预设 / `templates` 参数化 / `ref` 多态 / `[conventions.*]` 七子节 / 变体门六处）各收敛为 1；"靠数组位置决定语义"的位置 10 → 0；字段就地可读，无往返查找 |
| D2 | 通用性 | 审计 §7.2 的 16 处 DSL 改动点 → 0（或标注为唯一受控扩展点）；宿主侧按 ISA 名分发的重定位补丁器改为生成 |
| D3 | 无冗余 | 三谱实测重复各有唯一落点（模板版/手抄版并存、`lint.vary_candidates`、`imm_fn` 影子条目、位域当常量） |
| D4 | 后门归零 | 审计的 9 个真 ISA 特化 + 12 个 x86 口味缺省 + 1 处活兼容分支，逐条给出落点 |
| D5 | 可表达性 | v18 三谱每一类写法都有 v21 写法，或明确"不支持 + 理由 + 替代" |
| D6 | 可评审 | 决策点显式（见 §13），待冻结项给默认方案与备选 |

---

## 2. 现状证据

### 2.1 表面规模

| 事实 | 数值 / 位置 |
| --- | --- |
| 规范规模 | `docs/reference/isa-dsl.md` 2155 行；键总览表由 `src/schema.rs` 生成、`tests/schema_guard.rs` 逐字钉住 |
| 顶层节 | 21（`meta`/`include`/`override`/`encoding`/`reg`/`conventions`/`types`/`stack`/`operand_slots`/`forms`/`instructions`/`templates`/`reloc`/`derive`/`pseudo`/`lowering`/`pattern`/`machine`/`emit`/`spill`/`vectors`） |
| 可写键 | 约 150 |
| 字符串魔法族 | 约 20 类（`ops` 三元组、asm 占位、mem 模板组件、13 个编码键取值、谓词属性、`vary`、模板插值、spill `{N}`…） |

### 2.2 字段声明今天要在三处之间往返

| 环节 | 今天写在哪 |
| --- | --- |
| 位域定义 | `[conventions.bitfields.<名>]`（全局表，`offset`/`width` 或 `pieces[].shift`） |
| 主操作码位域 | `[[forms]].opcode_field = "opcode"` |
| 操作数 → 位域绑定 | `[[forms]].operand_fields = ["rd", "rs1", "rs2"]`，**第 i 个操作数 → 第 i 个位域名** |
| 固定字段值 | `[[instructions]].fields = { funct3 = 0 }`（按位域名引用） |
| 一值摊多段 | `[[operand_slots]].encode = "slice"` + `fields = [...]`（宽度和必须等于槽宽） |

⇒ 读一条指令的编码要跳四处；同段多解（`shamt5`/`shamt6`、`op6`/`imm26`）只能靠并存位域名 + lint 例外处理。

### 2.3 寄存器名与真值错位

组名数字 = **字节**：`[reg.gpr8]` = 64 位（RAX…）、`[reg.fpr16]` = 128 位（XMM0…）、`[reg.gpr1]` = 8 位（AL…）。
`RegClass` 内部也按字节承载（`GPR(8)` = 64 位）。`[meta]` 的五个宽度键同样是**字节**
（`default_gpr_width`/`default_fpr_width`/`addr_width`/`value_gpr_width`/`value_fpr_width`），
与 `[encoding].bits`（位）和 `opsize`（位）三套口径并存；`value_fpr_width` 缺省写死 `8`、
`default_fpr_width` 让 `fpr16` 优先，都是 x86 的 ZMM 组把基准带偏后的补丁。

### 2.4 三谱规模与重复（只读审计实测）

| 事实 | x86 | riscv64 | arm64 |
| --- | --- | --- | --- |
| 物理行数 | 5102 | 3631 | 5813 |
| 指令（显式 + 模板行） | 200 + 99 | 92 + 82 | 348 + 196 |
| 同签名指令块占比 | 122/200（61%） | 79/92（85.9%） | 328/348（94.3%） |
| 最大同签名组 | — | — | 54 条（abs/neg） |
| `lint.vary_candidates` | 55 | 28 组 | 7 |
| `lint.unassigned_bits` | 0 | 7 | 262 |
| 文本歧义名单 | 36 | 4 | 42 |
| 注释密度 | 11.2% | 10.1% | 5.7%（含 25 行 mojibake） |

x86 专项：`modrm` 内联 116/200、`fields` 内联 84/200、写了 `form` 又覆盖编码键 128/200；
20 个 form 里只有 22 条指令是纯预设。

### 2.5 后门（活代码）

| # | 位置 | 事实 |
| --- | --- | --- |
| 1 | `dsl/codegen/lowering.rs:555-625` | 分支降级按"字段个数"猜三种 ISA；`else if` 三臂 + 字段名兜底（不匹配就装到别的字段，不报错） |
| 2 | `dsl/codegen/lowering.rs:202` | `let has_jal = has_jmp && !model.is_prefix_scan();` —— "JAL 语义" = "非 x86" |
| 3 | `dsl/codegen/lowering.rs:579` | 硬编码条件码 `4u8`（x86 `je`），**绕过 `[conventions.cond]` 这份数据** |
| 4 | `dsl/codegen/lowering.rs:527-554`、`frame.rs:172-208` | 跳转与尾声跳转同款三形态；riscv 臂硬编码 `Reg::from_index(0, …)` 当零寄存器 |
| 5 | `dsl/model.rs:1287-1305` | `default_prefix_scan()` 逐字节就是 x86 前缀集；与同文件对 `addr32` 的 fail-closed 态度自相矛盾 |
| 6 | `dsl/model.rs:1177-1221` | `PrefixEffect` 是 x86 闭集；`extra_bytes()` 把"只有 Rex2 吃一字节"当唯一特例 |
| 7 | `dsl/codegen/vlen.rs:2496-2506`、`:1837` | 解码前缀条件表与"哪些前缀进 key"按 5 个字节枚举 |
| 8 | `dsl/codegen/vlen.rs:2439-2494` | REX2 归一化 = 硬写的字节手术（插虚拟 `0F`） |
| 9 | `dsl/codegen/integration.rs:920-946` | `{off}`/`{alloca}` 第二套内存语法：`index` 恒 `None`、`scale` 恒 1，与 `[conventions.mem]` 无关 |
| 10 | `dsl/codegen/frame.rs:234,1538-1557` | `[spill.GPR]`/`[spill.FPR<bytes>]` 是代码合同；起别的名**静默 no-op** |
| 11 | `dsl/validate.rs:1069-1073`、`dsl/model.rs:1346-1349` | `force_disp_base` 元素 `< 16` 硬编码（"REX.X-extended rm"） |
| 12 | `dsl/codegen/moves.rs:311` | `if a.width >= 128 { "vec_mov" } else { "fpr_mov" }` 魔数 |
| 13 | `dsl/codegen/{machine,integration,frame}.rs` | `RegClass::FPR(8)` 回退、`["rsp","sp"]`/`["rbp","fp"]` 惯例名回退 |
| 14 | `crates/foundation/forge-isa-runtime/src/machine/reloc_patcher.rs:77-84` | 按 `[meta].name` 分发 `"x86_64"/"riscv64"/"arm64"`，未知名 `_ => return` 静默 `None`；散布位段重排必须宿主手写 |
| 15 | `dsl/lint.rs:161` | `["op", "ops"]` 双拼兼容（本轮唯一活兼容分支） |

`validate.rs` 33 条校验规则中：通用约 14、半通用约 12、ISA 特化 3（`modrm` 位域名、`force_disp_base`、`prefix_scan`），
另有 2 处"变长 ISA 不支持"缺口（`names`/`bits` 槽在 `prefix_scan` 下 fail-closed）。
`lint` 8 个码：通用 5 / 半通用 3。

### 2.6 文档级缺口

`[[instructions]].when` 是**死键**（schema 与文档列着，`validate` 与 codegen 从不读）；
`[[operand_slots]].kind` 的枚举漏 `bits`；`zr31`/`float`/`arrangement` 只在键总览出现、正文零解释；
占位符注册表代码里 38 条而文档表只覆盖约 15 条（V512 的 `_h2`/`_h3` 与全部 `iconst`/`fconst` 分片缺失）。

---

## 3. 外部参考

沿用本仓 `docs/plans/forge-isa-dsl-v19-plan.md` §3 已核证的来源，并并入本轮新增调研。

| 设计 | 借 | 不借 |
| --- | --- | --- |
| QEMU [decodetree](https://www.qemu.org/docs/master/devel/decodetree.html) | `%field`（位段命名一次）+ `&args`（操作数集）+ `@format`（布局）三者分离与组合；重叠显式分组；诊断回归 | 变长前导的"每位宽一份 decoder" |
| LLVM [TableGen](https://llvm.org/docs/TableGen/ProgRef.html) / [GlobalISel 模式](https://llvm.org/docs/GlobalISel/MIRPatterns.html) | `class → multiclass → let` 的继承 + 行展开；显式根；同名操作数二次出现 = 相等约束 | 元编程层（会逼全量重写） |
| Cranelift [ISLE](https://cfallin.org/blog/2023/01-20/cranelift-isle/) / [语言参考](https://github.com/bytecodealliance/wasmtime/blob/main/cranelift/isle/docs/language-reference.md) | 重叠必须显式选赢家（实测抓出大量"永不触发"的规则）⇒ `--strict-overlap` 升为可选硬档；类型承载不变量 | 术语重写语言（v19 已否决）；argument polarity（ISLE 自己已删） |
| Ghidra [SLEIGH token](https://ghidra.re/ghidra_docs/languages/html/sleigh_tokens.html) / [constructors](https://ghidra.re/ghidra_docs/languages/html/sleigh_constructors.html) | `define token` 的位段命名；`attach variables` 的"映射失败即非法编码"；table 子表（一族共用 reg/imm/[reg]） | p-code 式"数据 + 通用解释器"（v19 V7 已按度量否决） |
| GCC [Mode Iterators](https://gcc.gnu.org/onlinedocs/gccint/Mode-Iterators.html) | 迭代器逐值带条件（对应 `vary` + `when`） | 无往返校验 |
| [Sail 手册](https://www.cl.cam.ac.uk/~pes20/sail/manual.pdf) / [Sail 语言](https://raw.githubusercontent.com/rems-project/sail/refs/heads/sail2/doc/asciidoc/language.adoc) / [ASL 笔记](https://alastairreid.github.io/RelatedWork/notes/asl/) / [Sail POPL'19](https://doi.org/10.1145/3290384) | 位向量长度是类型级（不许隐式位宽伸缩）；`mapping` 类型 = 双向编解码关系；`bitfield` 可重叠不连续 | 形式化与通用编程语言化（strings/list 不进语义） |

### 3.1 本轮新增的四条硬结论

1. **QEMU 的 x86 完全不用 decodetree**——位图模型表达不了前缀扫描。
   ⇒ `bits` 与 `stream` 必须是**两套机制、两套词汇**，只共用 `form` 这个入口名，**绝不合并**。
2. Sleigh 的 `token → field → table`、decodetree 的 `@format + &args`、ARM 的 class + slot
   **三家独立收敛到同一结构**：**编码 = slots × patterns**。本方案的 `form.fields`（slots）+
   `inst.ops`/`match`（patterns）就是这一层。
3. **位段必须命名、禁止位置回查**（Sleigh/decodetree 一致）⇒ `operand_fields` 位置数组判为反模式，
   字段在 form 里就地声明并带接口名。
4. **映射失败必须等于非法编码**（Sleigh `attach`）⇒ 类型不匹配、寄存器组放不下、位宽不符，
   一律**声明期报错**，不退回整数解释。

### 3.2 参考局限（如实记录）

- ARM 官方 slotted-instruction/Encoding 伪代码正文未取到（入口 302 到 support.arm.com，PDF 不可解析），
  只给可核验入口，未转述细节。
- 未找到名为 "A Survey of ISA Specification Languages" 的单一综述，改以 Alastair Reid 的
  ISA 规范文献库作等价替代。

---

## 4. 设计公理

1. **一概念一处**：每个语义只有一个键、一种写法（没有"预设 / 内联 / 别名"三套并存）。
2. **宽度只在名字里**：`[reg.gpr64]` 表头即"64 位整数寄存器"；不写 `class`、不写 `bits`。
3. **字段就地声明**：字段的类型、位区间、接口名、默认编码全部写在用到它的 `form` 里；
   不再有全局位域表，也不再有按位置的绑定数组。
4. **结构进结构，文本只放文本**：类型/绑定/取值/变体条件一律用 TOML 键；
   字符串只用于 asm/print 文本、名字引用与两种记法（位区间、立即数修饰文本模式）。
5. **不猜**：删除全部缺省推断（`opsize` 从"唯一 out"推、`modrm.reg/rm` 位置缺省、
   `imm` = 最后一个操作数、`FPR(8)` 兜底、惯例名回退）。可省略的只有两条：
   **同名即绑定**与**纯值域派生**。
6. **ISA 特殊机制只在一处且有名**：`form.kind = "bits" | "stream"`；`stream` 是
   "前缀链 + 操作码 + 操作数编码"这一族变长编码的**唯一受控扩展点**，段词汇是闭集且写在谱里。

---

## 5. 表面结构（节表）

顶层节 21 → 13：

| v21 节 | 取代 | 内容 |
| --- | --- | --- |
| `[meta]` + `[meta.text]` | `[meta]` | 名字/端序/模式/变体参数/文本约定 |
| `[reg.<名>]` | `[reg.<名>]` | 寄存器组：**表头即类 + 位宽** |
| `[enum.<名>]` | `[conventions.cond]` + `[conventions.bitsets.*]` + `[conventions.imm_names.*]` | 命名取值表（`value` / `bits`） |
| `[operand.<名>]` | `[[operand_slots]]` | 可复用的取值类型（imm / label / mem / cond / bits） |
| `[encode.<名>]` | `operand.encode` + `fields` + `arrangement` | 非线性值 ↔ 多字段映射（`slice` / `logical_imm` / `values`） |
| `[form.<名>]` | `[[forms]]` + `[encoding]` + `[conventions.bitfields]` | **唯一编码载体**：`bits`（就地 `fields`）/ `stream`（有序段，每段同样就地 `fields`） |
| `[inst.<名>]` | `[[instructions]]` | 指令：`form` + `ops` + `match` + `bind` + `asm` + `effect`/`roles` |
| `[family.<名>]` | `[[templates]]` + `ref` | 唯一复用机制（body + rows + vary）；族名同时是多态引用名 |
| `[pred.<名>]` | `[[derive]]` | 命名谓词 |
| `[lower.<Op>]` | `[[lowering]]` + `[[pattern]]` | 选择规则（`when`/`tree`/`vary`/`emit`/`temps`），含分支与跳转 |
| `[machine]`（+ `frame`/`stack`/`text`/`classes`/`spill.<组>`） | `[machine]`+`[machine.frame]`+`[stack]`+`[emit]`+`[types]`+`[spill.*]` | 机器事实唯一处 |
| `[asm]`（+ `pseudo.<名>`/`reloc.<名>`/`immfn.<名>`） | `[[pseudo]]` + `[[reloc]]` + `[[conventions.imm_fn]]` | 汇编器侧数据 |
| `[[vectors]]`、`include`、`[[override]]` | 同名 | 不变（回归网） |

**整节删除**：`[encoding]`、`[conventions]`、`[stack]`、`[emit]`、`[types]`、`[[forms]]`（改 `[form.*]`）、
`[[derive]]`（改 `[pred.*]`）、`[[lowering]]` 与 `[[pattern]]`（改 `[lower.*]`）、
`[[pseudo]]`/`[[reloc]]`/`[[conventions.imm_fn]]`（改 `[asm.*]`）、`[spill.*]`（改 `[machine.spill.*]`）。

---

## 6. 逐节语法

### 6.1 `[reg.*]` — 宽度只在名字里

```toml
[reg.gpr64]                    # 表头即"64 位整数寄存器"：族 = gpr，宽度 = 64 位
prefix = "R"
count = 32
index_base = 0                 # 原 base_index
alloc = 16                     # 原 alloc_count：能编码不能分配（APX EGPR）
rex_required = [4, 5, 6, 7]    # ISA 语义（非宽度）：这些组内编号必须发 REX，原 byte_reg

[reg.gpr64.aliases]            # 别名 = 组内下标（解析认别名、渲染只出主名）
ra = 0

[reg.fpr128]                   # 取代今天的 [reg.fpr16]（16 字节 = 128 位）
prefix = "XMM"
count = 16
```

规则：表头 = `族前缀 + 位宽`，族前缀 ∈ `gpr`/`fpr`/`vec`/`kreg`（决定寄存器族），位宽单位 = **位**。

**三谱改名对照**（一次性原子重命名，必须 grep 全仓）：

| 旧名（= 字节） | 真值 | 新名（= 位） |
| --- | --- | --- |
| `gpr8` / `gpr4` / `gpr2` / `gpr1` | 64 / 32 / 16 / 8 位 | `gpr64` / `gpr32` / `gpr16` / `gpr8` |
| `fpr8` / `fpr16` / `fpr32` | 64 / 128 / 256 位 | `fpr64` / `fpr128` / `fpr256` |
| `fpr4` / `fpr2` / `fpr1` | 32 / 16 / 8 位 | `fpr32` / `fpr16` / `fpr8` |
| `kreg8` | 8 位 | `kreg8`（不变） |

> **迁移风险**：`gpr8` 今天是 64 位、改后是 8 位——旧名与新名**语义相反**，漏改一处即静默编错字宽。
> 迁移必须一次改完，并靠字节级回归（`isa_roundtrip_guard`）兜住。

**连带**：`[meta]` 的五个宽度键（`default_gpr_width` 等，均字节）删除；
"哪一组是主类"改由 `[machine]` 指组名（`gpr = "gpr64"`、`fpr = "fpr128"`），
因为 x86 最宽的 FPR 组是 ZMM（512 位），不能按"最宽"推。见 §13 决策 D-2。

### 6.2 字段语法（完整规范）

#### 6.2.1 语法

```text
fields      = "[" field ("," field)* ","? "]"
field       = type-expr ( "|" type-expr )* "[" range ("," range)* "]" [ ":" name ] [ "=" value ]
type-expr   = "u" digits | "i" digits | <寄存器组名> | <operand 名>
range       = bit | bit ":" bit
value       = "0x" hex | "0b" bin | decimal
```

#### 6.2.2 四段含义

| 段 | 含义 | 必填 |
| --- | --- | --- |
| `type \| type …` | 该槽可被哪种数据填充；联合 = 符合其中任一类型即可 | 必填 |
| `[range, …]` | 位区间；多段时**按值的低位 → 高位依序放置**（与 v18 `encode = "slice"` 同语义，但顺序显式写出） | 必填 |
| `: name` | 接口名：`ops` 引用它、生成的 `Inst` 字段名是它、`match` 覆盖它、asm 用 `{name}`；不写 = 匿名槽 | 可省 |
| `= value` | 该槽的**默认编码**；不写 = 全 0 | 可省 |

#### 6.2.3 槽的两态（唯一判定规则）

- **指令的 `ops` 里列出了这个槽名** ⇒ **操作数槽**：解码捕获（不参与匹配）、编码由操作数填；
  `= value` 只在"可选操作数省略时"生效。
- **没列出** ⇒ **常量槽**：解码要求等于 `match[name]`（缺省 = 字段的 `= value`，再缺省 = 0）；
  编码写该值。

判定引入的唯一隐含是"是不是操作数由 `ops` 说"，不再有"位置""唯一 out"这类推断。

#### 6.2.4 类型词汇

| 写法 | 含义 | 校验（声明期，失败即非法） |
| --- | --- | --- |
| `u<N>` / `i<N>` | 内联整数数据（无符号 / 有符号），N 位 | N **必须等于**字段总位宽 |
| 寄存器组名（`gpr64`、`fpr128`） | 该组的一个寄存器 | 组内编号必须放得下字段位宽（32 成员 ⇒ ≥ 5 位） |
| `[operand.*]` 名（`imm12i`、`off26`、`mem`、`cc`、`fence`） | 可复用取值类型 | 其 `bits` 必须等于字段总位宽（或由 `[encode.*]` 声明多段映射） |

联合（`gpr64|gpr32`）表示"该槽可被任一类型填充"；**多候选同时成立 ⇒ 报错并列出候选**（不靠声明序消歧）。
寄存器组名与 `[operand.*]` 名同池，不得重名。

#### 6.2.5 位段相交

同一 form 内两个字段的位区间**允许相交**，只要没有**同一条指令**同时绑定相交的两段。
这条把今天 `LINT-BITFIELD-OVERLAP` 的逐指令口径写进规范：riscv 的 `shamt5`/`shamt6`、
arm64 的 `op6`/`imm26`、`word` 因此都是正常写法，不需要 lint 例外。

#### 6.2.6 指令级覆盖

`match = { name = value }`：

- 覆盖**常量槽** ⇒ 换固定值（riscv 的 `funct3`/`funct7`、x86 的 `opcode`）；
- 覆盖**操作数槽** ⇒ 该槽退化为常量（arm64 的 `rd = 31`，今天写成 `fields = { rd = 31 }`）。

`bind = { 槽名 = "操作数名" }` 是**唯一**的重命名机制（无位置语义），仅在槽名与操作数名不同的时候写。

#### 6.2.7 三谱示例

riscv R 型（`opcode` 在 form 里给默认值，`rd`/`rs1`/`rs2` 由 `ops` 列出 ⇒ 操作数槽）：

```toml
[form.R]
kind = "bits"
words = [32]
fields = [
  "u7[6:0]:opcode=0x33",
  "gpr64[11:7]:rd",
  "u3[14:12]:funct3",
  "gpr64[19:15]:rs1",
  "gpr64[24:20]:rs2",
  "u7[31:25]:funct7",
]

[inst.ADD]
form = "R"
ops = ["rd:out", "rs1", "rs2"]        # funct3/funct7 缺省 0，无需 match

[inst.SLL]
form = "R"
ops = ["rd:out", "rs1", "rs2"]
match = { funct3 = 1 }
```

riscv B 型（散射位段，值的低位 → 高位）：

```toml
[form.B]
kind = "bits"
words = [32]
fields = [
  "u7[6:0]:opcode=0x63",
  "off13[7,11:8,30:25,31]:imm",
  "u3[14:12]:funct3",
  "gpr64[19:15]:rs1",
  "gpr64[24:20]:rs2",
]
```

arm64 TBZ（一个操作数摊到两段不相邻的位，今天靠 `encode = "slice"` + 两个位域名 + 指令再列一遍）：

```toml
[form.TBZ]
kind = "bits"
words = [32]
fields = [
  "u6[23:19,31]:bitpos",              # = 今天的 b40(5 位) + b5(1 位)，一个槽
  "u6[30:25]:op=0b011011",
  "off14[18:5]:imm",                  # unit = 4，见 [operand.off14]
  "gpr64[4:0]:rt",
]
```

x86（`stream`：每段用同一套字段语法声明自己的字节布局；段 kind 只表达"这堆字节扮演什么角色"）：

```toml
[form.x86_mov_r_rm]
kind = "stream"
max_len = 15
segments = ["prefix", "rex", "opcode", "modrm"]      # 顺序即发射序

[form.x86_mov_r_rm.prefix]
kind = "prefix"
bytes = { "0x66" = "opsize", "0xF0" = "lock", "0xF2" = "repne", "0xF3" = "repe" }

[form.x86_mov_r_rm.rex]
kind = "rex"
fields = ["u1[7]:w", "u1[6]:r", "u1[5]:x", "u1[4]:b", "u4[3:0]=0x4"]

[form.x86_mov_r_rm.opcode]
kind = "opcode"
fields = ["u8[7:0]:opcode"]

[form.x86_mov_r_rm.modrm]
kind = "modrm"
fields = ["u2[7:6]:mod", "u3[5:3]:reg", "u3[2:0]:rm"]

[inst.MOV_R_RM]
form = "x86_mov_r_rm"
match = { opcode = 0x8B }
ops = ["dst:out", "src"]
bind = { reg = "dst", rm = "src" }
asm = "movrr {dst}, {src}"
```

#### 6.2.8 由此删除

`[conventions.bitfields]`（整节）、`form.opcode_field`、`form.operand_fields`、
指令 `fields = {…}` 映射、`[operand.*].encode = "slice"` 与 `[operand.*].fields`
（slice 由多段位区间表达）。

#### 6.2.9 声明期校验（全部 fail-closed）

位区间越界，或同一条指令同时绑定相交的两段；`uN/iN` 宽度 ≠ 字段总位宽；
寄存器组放不下；`operand` 类型不存在或宽度不符；联合类型多候选同时成立；
`ops`/`match`/`bind` 引用了该 form 里不存在的槽名；`bind` 的目标不是操作数槽。
未被任何字段覆盖的位 = wildcard（不参与匹配），由 lint 的 `--bits` 档报出（不默认报错）。

### 6.3 `[enum.*]` — 命名取值表

```toml
[enum.cond]
kind = "value"                 # value | bits
o = { code = 0 }
b = { code = 2, ir = "ult" }
c = { code = 2 }               # 同码别名（反汇编取同码里字母序最小者）
z = 4                          # 简写：ir 取键名（键名恰是 IR 条件名时）

[enum.fence]
kind = "bits"                  # 名字拼接、按位或
i = 8
o = 4
r = 2
w = 1

[enum.csr]
mstatus = 0x300
mvendorid = 0xF11
```

`[enum.cond]` 仍是"一张表三处用"：汇编按名解析 / 反汇编取同码首选名 / lowering 的 `{cc}` 按 `ir` 查。
`enum` 取代 `[conventions.bitsets.*]` 与 `[conventions.imm_names.*]` 两张表。

### 6.4 `[operand.*]` 与 `[encode.*]`

只有当字段需要额外语义（单位、符号、枚举、文本形态、后缀）时才需要声明 `[operand.*]`；
常见情形字段直接用寄存器组名或 `uN`/`iN`。

```toml
[operand.imm8s]
kind = "imm"
bits = 8
signed = true

[operand.imm32]
kind = "imm"
bits = 32
signed = true
literal = "bits"               # 原 wrap：十六进制字面量按位模式读

[operand.off13]
kind = "label"
bits = 13
unit = 2                       # 源单位（原 unit），语义不变

[operand.csr12]
kind = "imm"
bits = 12
enum = "csr"                   # 原 names = "csr"

[operand.fence]
kind = "bits"
enum = "fence"                 # 原 kind="bits" + table = "fence"

[operand.cc]
kind = "cond"
enum = "cond"

[operand.sym12]
kind = "imm"
bits = 12
symbol = { allow = true, require = true, modifiers = ["abs_g0", "abs_g1"] }
#         ^ 合并原 symbols / require_symbol / imm_fns 三个键

[operand.mem]
kind = "mem"
text = ["[{base}+{index}*{scale}+{disp}]",     # 第 0 条 = 渲染形态（规则不变）
        "{disp}[{base}]", "[{disp}]"]          # 其余仅解析，按列表序试
size_words = ["byte ptr", "word ptr", "dword ptr", "qword ptr"]
#            ^ 原 [conventions.mem].size_keywords，挂到槽上

[operand.arr4]
kind = "imm"
bits = 4
suffix = { encode = "arr" }    # 原 arrangement（NEON v0.16b 这类后缀）
```

```toml
[encode.logimm]                # 唯一的非线性映射（A64 位掩码立即数）
kind = "logical_imm"
fields = ["n", "immr", "imms"]  # 这三名的位区间仍由所在 form 的 fields 声明

[encode.arr]                   # 后缀名 → 字段值（取代 arrangement 的内联表）
kind = "values"
v0_16b = { vq = 0, vsize = 1 }
```

删除的开关：`float`（→ `kind = "imm"` + `value = "float"`）、`wrap`（→ `literal`）、
`min`/`max`（→ `range = [lo, hi]`，缺省派生）、`table`/`names`（→ `enum`）、
`class` 单数糖（字段写联合即可）、`arrangement`（→ `suffix`）。

### 6.5 `[form.*]` — 唯一编码载体

- `kind = "bits"`（缺省）：`words = [32]`；单值 = 定宽，多值 = 原 `mixed`；字段见 §6.2。
- `kind = "stream"`：`max_len`；`segments = [...]` 有序段；每段一个子表，段 kind ∈
  `prefix` | `escape` | `rex` | `opcode` | `opcode_reg` | `modrm` | `sib` | `disp` | `imm` | `vex` | `evex`
  （闭集，唯一受控扩展点），每段用**同一套字段语法**声明字节布局。
- `[encoding]` 整节消失：定宽 / 混合 / 前缀扫描三态由 `kind` + `words`/`max_len` 直接表达，
  三态键互斥校验随之消失。
- 前缀表**必须显式声明**（删除 `default_prefix_scan()` 的 x86 字节表）；
  前缀效果改为参数化词汇（`opsize` / `addrsize` / `lock` / `rep` / `reg_ext` / `map` / `opaque(len)`），
  `reg_ext` 与 `map` 用数据表达 REX2 式"扩展位 + 映射替代字节"，不再有字节手术。
- `force_disp_base` 的上界由 modrm 段 `rm` 字段位宽 + `reg_ext.bits` 推出，删掉硬编码 16。
- **待原型冻结**：`stream` 段子表的最终键名（默认如 §6.2.7 例；备选是段内统一 `slots = {}`）。

### 6.6 `[inst.*]` / `[family.*]` / 分组

```toml
[inst.MOV_R_RM]                # 见 §6.2.7
[inst.ADD]                     # 见 §6.2.7

[family.rev]                   # 唯一复用机制（body + rows + vary）；族名 = 多态引用名
form = "vec2r"
asm = "rev{bits}.{arr} {dst}, {src}"
[[family.rev.rows]]
inst = "REV164H"
bits = 16
arr = "4h"
match = { opcode = 0x0E, vq = 0, vsize = 1, rm = 0 }
```

- `ops` 变短：`["dst:out", "src"]`（名 + 角色；类型/位域来自 form 的字段），取代 `"dst:gprx:out"`。
- `ref` → `group`；删除"form 预设 + 指令内联编码键"两套并存：编码一律在 form，指令只写 `match`/`bind`。
- 删除死键 `[inst].when`；`only_variants` + `fields_variant` + 六处变体门 → 统一
  条目级 `when = { xlen = 64 }` + 值级字段分派（吸收 `docs/plans/isa-dsl-variant-field-values.md`）：
  `match = { imm12 = { xlen = { 32 = 0x698 }, default = 0x6b8 } }`。
- `data_width` → `data_bits`（单位统一）。
- 新增 `family.vary`（与 `[lower.*].vary` 同一套 zip 语义，一处机制两处用）消同签名重复；
  新增 `reserved = [...]` 作为"保留位"的显式别名（字段默认值语义已能表达大部分）。

### 6.7 `[pred.*]`

```toml
[pred.is_64]
expr = { eq = ["rs1_bits", 64] }
```

`expr` 就是结构化谓词（`and`/`or`/`not`/`eq`/`ne`/`lt`/`le`/`gt`/`ge`/`in`）；
派生属性取 1/0，只能引用核心属性。

### 6.8 `[lower.<Op>]` — 含分支与跳转

```toml
[[lower.Iadd]]
emit = ["mov {out}, {0}", "add {1}, {out}"]

[[lower.Load]]
vary = { w = [32, 64], m = ["mov32", "mov64"] }
emit = ["{m} {out}, [{0}]"]

[[lower.Br]]                   # 分支降级由谱写，编译器不再猜 ISA
emit = ["test {0}, {0}", "jcc {cc_inv}, {false}", "jmp {true}"]

[[lower.Fadd_Mul]]             # 原 [[pattern]]，合并进同一节
tree = "Fadd(Fmul(a, b), c)"
when = { elem = 1 }
emit = ["movss {out}, {a}", "mulss {out}, {b}", "addss {out}, {c}"]
```

- 删除编译器里的三处"按字段个数猜 ISA"：x86 = 上面的 `test` + `jcc` + `jmp`；
  riscv = `beq {0}, x0, {false}` + `jal x0, {true}`；arm64 = `cbz {0}, {false}` + `b {true}`。
- 新增通用符号值 `{true}` / `{false}` / `{target}` / `{cc}` / `{cc_inv}`
  （`{cc_inv}` 反查 `[enum.cond]`，取代硬编码 `4u8`）。
- 占位符去动物园：新增**表达式**取代 20 余个派生 token，例如 `{imm0 - 4}`、
  `{iconst >> 12 & 0xfffff}`、`{(iconst >> 32) & 0xfffff}`（V512/iconst/fconst/shufps 分片全部由此表达）。
- 临时寄存器显式声明：`temps = ["g0:gpr", "f1:fpr"]`；删除 `{g}`/`{gN}`/`{f}`/`{fN}` 的隐式声明。
- 待度量项：`[snippet.<名>]` 局部子序列复用（riscv"物化 64 位掩码"4 行惯用法出现 14 次是真实复用证据；
  v19 曾按 x86 数据判定"跨 op 复用为 0"，需按新证据重测）。**默认不做。**

### 6.9 `[asm]`

```toml
[asm.reloc.abs64]
semantics = "absolute"
operand = "imm64"              # 原 slot
addend = 0

[asm.pseudo.li]
asm = "li {rd}, {imm}"
emit = ["lui {rd}, (({imm} + 0x800) >> 12)",
        "addi {rd}, {rd}, ((({imm} + 0x800) & 0xfff) - 0x800)"]

[asm.immfn.lo12]
texts = ["#:lo12:{0}", ":lo12:{0}"]     # 一个名字多种拼写，消掉 arm64 的 49 组影子条目
expr = "{0} & 0xfff"
```

- **重定位改为可生成**：`[asm.reloc.*]` 用 `operand` 指向槽，散布位段重排复用该槽所在字段的位区间
  ⇒ 删除宿主侧按 ISA 名的 `RelocPatcher` 分发与两个手写补丁器。
- 文本层两条规则取代现状的一堆特例：① `asm` 是**可接受写法列表**，第 0 条 = 渲染形态
  （与 `mem.text` 同口径）；② 占位修饰 `{name?}`（可选，连同紧邻字面词缀一起省略）、
  `{name:affix}`（词缀随组件出现，`+` 对已带 `-` 的数值抑制）。
  尺寸关键字属于 `[operand.mem].size_words`，"紧邻 `{size}` 的字面量不是条件前缀"这个例外消失。
- `[lower.*].emit` 里的内存操作数改用 `[operand.mem].text` 的同一套组件解析
  ⇒ 删除 `integration.rs:920-946` 的私有内存语法。

### 6.10 `[machine]`

```toml
[machine]
gpr = "gpr64"                  # 主类 = 指组名（宽度在名字里），取代 default_gpr_width（字节）
fpr = "fpr128"                 # 显式，取代 "fpr16 优先" 补丁
addr = "gpr64"                 # 取代 addr_width
fixed = ["X0", "X1"]           # 原 fixed_regs
scratch = ["X5", "X6"]         # 原 spill_scratch
link = "X1"                    # 原 link_reg
arg_place = "position"         # 原 arg_slot: by-position
padding = 8                    # 原 frame_padding
byref_bytes = 16               # 原 vector_by_ref_bytes
vector_tiers = [128, 256, 512] # 位

[machine.frame]
sp = "X2"
fp = "X8"
layout = "inside"              # outside | inside
fp_push = 16
alloc_neg = true

[machine.stack]
slot = 64                      # 位（原字节）
align = 128
fp_save = 64

[machine.text]
align_pad = 0x90               # 原 [emit].align_pad
epilogue_label = true

[machine.classes]              # 原 [types]：IR 类型 → 寄存器组
i32 = "gpr32"
f64 = "fpr64"

[machine.spill.gpr]            # 键 = 声明的寄存器组名（不再是 GPR/FPR<bytes> 命名合同）
class = "gpr64"
load = "MOV64_RM {0}, {1}"
store = "MOV64_MR {0}, {1}"
```

- 删除 `[stack]`/`[emit]`/`[types]` 三节；删除 `callee_saved_gpr`/`callee_save_slots`
  （v20 A6 起已无"无 plan 继续编译"路径）。
- spill 按组名分派 ⇒ 写错键名不再静默 no-op。

### 6.11 变体、测试向量、多文件

`[[vectors]]` / `include` / `[[override]]` 名字与语义不变（最机械的一块，
也是唯一能证明"字节没变"的回归网）；变体统一为条目级 `when` + 值级字段分派。

### 6.12 `[meta]`

```toml
[meta]
name = "x86_64"
endian = "little"
mode = 64
variants = { xlen = [32, 64] }

[meta.text]
comment = "#"                  # 原 comment_char
label_suffix = ":"
directive = "."                # 原 directive_prefix
imm_prefix = "$"
mnemonic_case = "insensitive"
reg_case = "insensitive"       # 原 case_insensitive_regs
```

---

## 7. 后门清除表

| # | 后门（位置） | v21 落点 |
| --- | --- | --- |
| 1 | 分支降级按字段个数猜 ISA；`has_jal = has_jmp && !is_prefix_scan()`；硬编码 cond `4u8` | **删除**：`[lower.Br]` 由谱写；新增 `{true}`/`{false}`/`{cc_inv}` |
| 2 | 跳转降级三形态 + 硬编码 x0 | **删除**：`[lower.Jmp]` 由谱写 |
| 3 | 尾声跳转三形态（`frame.rs:172-208`） | **删除**：`[lower.*]` + `roles` 数据 |
| 4 | `default_prefix_scan()` 的 x86 字节表 | **删除缺省**；前缀段 `bytes` 必须显式 |
| 5 | `PrefixEffect` x86 闭集 + `extra_bytes` 特例 | 参数化效果词汇，写在前缀段 |
| 6 | `prefix_cond_ts` 与"哪些前缀进 key"按字节枚举 | **删除**：解码条件由前缀段 `bytes` 集合生成 |
| 7 | REX2 字节手术 + 虚拟 `0F` | 段数据：`map` + `reg_ext.bits` + 位序 |
| 8 | `{off}`/`{alloca}` 第二套内存语法 | 统一走 `[operand.mem].text` |
| 9 | `[spill.FPR<bytes>]` 命名合同（写错静默 no-op） | `[machine.spill.<组名>]`，按组分派 |
| 10 | `reloc_patcher.rs:77-84` 按 ISA 名分发 + 手写补丁器 | 由 `[asm.reloc.*]` + 字段位区间**生成** |
| 11 | `force_disp_base ≤ 15` 魔数 | 上界由 rm 字段位宽 + `reg_ext.bits` 推出 |
| 12 | `moves.rs:311` 的 `width >= 128 → vec_mov` | 由所选指令的寄存器组派生 |
| 13 | `FPR(8)` / `["rsp","sp"]` / `["rbp","fp"]` 惯例回退 | **fail-closed**，一律来自 `[machine]`/`[reg.*]` |
| 14 | `opsize` 的 `"s0"/"s1"/"out"/"max"` 四态与位置语义 | form 的 `word_size` 规则 + 显式绑定；`"sN"` 消失 |
| 15 | `modrm` 的 `reg`/`rm` 位置缺省、`imm` = 最后一个操作数 | 段字段名 + `bind`；无位置缺省 |
| 16 | `kind`/`class` 隐式推导与 `class`/`classes` 双形 | `kind` + 字段类型（组名或联合）单一形式 |
| 17 | `[conventions.bitfields]` + `opcode_field` + `operand_fields` 三处往返 | **合并为 form 内就地 `fields`**（本方案核心） |
| 18 | `wrap`/`float`/`arrangement`/`symbols`/`require_symbol`/`imm_fns`/`zr31`/`byte_reg` 八个一次性开关 | `literal` / `value` / `suffix` / `symbol{}` / `operand.zero,sp` / `reg.rex_required` |
| 19 | `lint.rs:161` 的 `["op","ops"]` 双拼兼容 | 唯一拼法 |
| 20 | `[[instructions]].when` 死键；`kind` 枚举漏 `bits`；占位符表漏 23 条 | 死键删除；键表由 schema 生成，缺一即守卫红 |

**保留但写明是"受控扩展点 / 宿主合同"**（不算后门）：
`stream` 的段 kind 闭集；`roles` 的 17 个能力名（跨 crate 与 `forge-abi` 的合同）；
`effect` 的 9 个语义标签。

---

## 8. 破坏性清单（键级）

### 8.1 整节

| v18 节 | v21 处理 |
| --- | --- |
| `[encoding]` | 删除 → `form.kind` + `words`/`max_len` |
| `[conventions]` | 删除 → `form.fields`（位域）、`enum`（cond/bitsets/imm_names）、`asm.immfn`、`operand.mem` |
| `[stack]` | 删除 → `[machine.stack]` |
| `[emit]` | 删除 → `[machine.text]` |
| `[types]` | 删除 → `[machine.classes]` |
| `[[forms]]` | 改 `[form.<名>]`（名字即表头） |
| `[[instructions]]` | 改 `[inst.<名>]` |
| `[[templates]]` | 改 `[family.<名>]`，`ref` → `group` |
| `[[derive]]` | 改 `[pred.<名>]` |
| `[[lowering]]` + `[[pattern]]` | 合并为 `[lower.<Op>]`（`tree` 承载原 pattern） |
| `[[pseudo]]` / `[[reloc]]` / `[[conventions.imm_fn]]` | 改 `[asm.pseudo.*]` / `[asm.reloc.*]` / `[asm.immfn.*]` |
| `[spill.<名>]` | 改 `[machine.spill.<组名>]` |
| `[[vectors]]` / `include` / `[[override]]` | 不变 |

### 8.2 键（按 v18 节）

| v18 键 | v21 处理 |
| --- | --- |
| root `meta` | 保留（键改动见下） |
| root 其余节名 | 见 §8.1 |
| `[meta].name` / `version` / `variants` / `endian` / `mode` | 保留（`version` 仍是自由串） |
| `[meta].case_insensitive_regs` / `comment_char` / `label_suffix` / `directive_prefix` / `imm_prefix` / `mnemonic_case` | 改名 + 移入 `[meta.text]` |
| `[meta].default_gpr_width` / `default_fpr_width` / `addr_width` / `value_gpr_width` / `value_fpr_width` | **删除**（宽度在组名里；主类由 `[machine].gpr/fpr/addr` 指组） |
| `[meta].vector_tiers` | 移 `[machine].vector_tiers`，单位改位 |
| `[encoding].kind` / `bits` / `widths` / `max_len` / `default_opsize` | 删除 → `form.kind` / `words` / `max_len` / `form.default_word_size` |
| `[reg.*].names` / `prefix` / `count` | 保留 |
| `[reg.*].base_index` | 改名 `index_base` |
| `[reg.*].alloc_count` | 改名 `alloc` |
| `[reg.*].aliases` | 保留 |
| （组名的数字） | **语义改位**，全部重命名（§6.1 表） |
| `[stack].slot` / `align` / `fp_save` | 移 `[machine.stack]`，单位改位 |
| `[types].<类型名>` | 移 `[machine.classes].<类型名>` |
| `[conventions.bitfields.<名>].offset` / `width` | **删除** → 字段就地位区间 |
| `[[conventions.bitfields.<名>.pieces]].offset` / `width` / `shift` | **删除** → 多段位区间（值序） |
| `[conventions.modrm].reg_field` / `rm_field` | 删除（段字段自带名） |
| `[conventions.modrm].force_disp_base` | 移入 modrm 段，上界由数据推出 |
| `[conventions.cond].<名>.code` / `ir` | 移 `[enum.cond]` |
| `[[conventions.prefix_scan]].byte` / `range` / `effects` | 移 `form.*.prefix.bytes`，效果参数化 |
| `[conventions.bitsets.<表>]` | 移 `[enum.<表>]`（`kind = "bits"`） |
| `[conventions.imm_names.<表>]` | 移 `[enum.<表>]` |
| `[[conventions.imm_fn]].name` / `text` / `expr` | 改 `[asm.immfn.<名>].texts` / `expr`（`name` → 表头） |
| `[conventions.mem].templates` / `size_keywords` | 移 `[operand.mem].text` / `size_words` |
| `[[operand_slots]].name` | 改表头 |
| `[[operand_slots]].kind` | 保留 |
| `[[operand_slots]].class` / `classes` | 删除（字段类型写组名或联合） |
| `[[operand_slots]].zr31` | 改 `zero` + `sp` 两个布尔 |
| `[[operand_slots]].byte_reg` | 移 `[reg.*].rex_required` |
| `[[operand_slots]].width` | 保留（改键名 `bits`，单位位） |
| `[[operand_slots]].signed` | 保留 |
| `[[operand_slots]].float` | 改 `value = "float"` |
| `[[operand_slots]].min` / `max` | 合一为 `range = [lo, hi]` |
| `[[operand_slots]].wrap` | 改 `literal = "bits"` |
| `[[operand_slots]].unit` | 保留 |
| `[[operand_slots]].roles` | 改 `role`（单值） |
| `[[operand_slots]].encode` / `fields` | `slice` → 多段位区间；`logical_imm` → `[encode.*]` |
| `[[operand_slots]].table` / `names` | 合一为 `enum = "<表名>"` |
| `[[operand_slots]].symbols` / `require_symbol` / `imm_fns` | 合一为 `symbol = { allow, require, modifiers }` |
| `[[operand_slots]].arrangement` | 改 `suffix = { encode = "<名>" }` |
| `[[forms]].name` | 改表头 |
| `[[forms]].modrm` / `modrm_fixed` | 删除 → modrm 段字段 + `bind` + `match` |
| `[[forms]].rex` | 删除 → `rex` 段 |
| `[[forms]].vex.{map,pp,w,l}` / `evex.{…}` | 移 `vex` / `evex` 段 |
| `[[forms]].prefix` / `opsize` / `rex_w` | 移前缀段 / `word_size` 规则 |
| `[[forms]].opcode_reg` | 移 `opcode_reg` 段的字段 |
| `[[forms]].imm` | 移 `imm` 段 |
| `[[forms]].escape` | 移 `escape` 段 |
| `[[forms]].opcode_field` | **删除** → 字段接口名 + form 默认值 |
| `[[forms]].operand_fields` | **删除** → 字段接口名 + 同名绑定 |
| `[[instructions]].name` | 改表头 |
| `[[instructions]].asm` | 保留（扩展为列表 + 修饰符） |
| `[[instructions]].form` | 保留（现在必填） |
| `[[instructions]].opcode` | 合并进 `match` |
| `[[instructions]].fields` | **删除** → form 字段默认值 + `match` |
| `[[instructions]].ops` | 保留（简化为 `名:角色`） |
| `[[instructions]].when` | **删除**（死键） |
| `[[instructions]].effect` / `roles` / `implicit_regs` / `reloc` | 保留 |
| `[[instructions]].data_width` | 改名 `data_bits`（位） |
| `[[instructions]].width` | 改 `word_bits`（位；缺省由 form 的 `words` 定） |
| `[[instructions]].only_variants` / `fields_variant` | 合并为 `when` + 值级分派 |
| `[[instructions]].ref` | 改 `group` |
| 13 个内联编码键 | 见上（form 段/`match`/`bind`） |
| `[[templates]].rows` / `name` / `body` | 改 `[family.<名>]` 的 `rows` / 表头 / `body` |
| `[[templates.rows]].inst` 与其余键 | 保留（加 `vary`） |
| `[[reloc]].name` | 改表头 |
| `[[reloc]].semantics` / `slot` / `addend` | 保留（`slot` 改名 `operand`） |
| `[[derive]].name` | 改表头 |
| `[[derive]].expr` | 保留 |
| `[[pseudo]].name` | 改表头 |
| `[[pseudo]].asm` / `emit` / `only_variants` | 保留（`only_variants` → `when`） |
| `[[lowering]].op` | 改表头 `[lower.<Op>]` |
| `[[lowering]].insts` | 改名 `emit` |
| `[[lowering]].when` / `priority` | 保留 |
| `[[lowering]].vary` | 保留（并下沉到 `family`） |
| `[[pattern]].insts` | 合并进 `[lower.<Op>].emit` |
| `[[pattern]].match` | 改名 `tree` |
| `[[pattern]].when` / `priority` / `only_variants` | 保留（`only_variants` → `when`） |
| `[machine].fixed_regs` / `spill_scratch` / `link_reg` | 改名 `fixed` / `scratch` / `link` |
| `[machine].frame` | 保留（子表） |
| `[machine].callee_saved_gpr` / `callee_save_slots` | **删除**（走 ABI 层） |
| `[machine].frame_padding` / `arg_slot` / `vector_by_ref_bytes` | 改名 `padding` / `arg_place` / `byref_bytes` |
| `[machine.frame].sp` / `fp` | 保留 |
| `[machine.frame].layout` | 值改 `outside` / `inside` |
| `[machine.frame].fp_push_bytes` | 改名 `fp_push` |
| `[machine.frame].alloc_neg` | 保留 |
| `[emit].align_pad` / `epilogue_label` | 移 `[machine.text]` |
| `[spill.<名>].load` / `store` / `base` / `only_variants` | 移 `[machine.spill.<组名>]`，加 `class`（`only_variants` → `when`） |
| `[[vectors]].asm` / `bytes` / `error` / `partial` / `comment` | 保留 |
| `[[override]].key` / `value` | 保留 |
| 宏参数 `spec_tests` / `name` / `parts` / `params` | 保留 |

### 8.3 键合一与新增

| 类型 | 条目 |
| --- | --- |
| 合一 | `symbols`+`require_symbol`+`imm_fns` → `symbol{}`；`table`+`names` → `enum`；`wrap` → `literal`；`float` → `value`；`arrangement` → `suffix`；`min`+`max` → `range`；`only_variants`+`fields_variant`+`when` → `when`+值分派 |
| 新增 | `{true}`/`{false}`/`{target}`/`{cc_inv}`；emit 表达式；`temps`；`family.vary`；`reserved`；`[asm.immfn.*].texts`；文本占位修饰 `{n?}`/`{n:affix}`；字段语法的 `bind` |
| 明确不做 | 自研文本语法；`[snippet]`（先度量）；表化 lowering 解释器（v19 V7 已否决）；任何 v18 键的兼容层 |

---

## 9. 覆盖论证

| 现状写法（代表） | v21 |
| --- | --- |
| riscv `pieces` 散布位段（S/B/J） | 多段位区间 `off13[7,11:8,30:25,31]:imm` |
| riscv `opcode_field` + `operand_fields` 位置绑定 | `form.fields` 就地声明 + `ops` 列名（同名即绑定） |
| riscv `bitfields` + 指令 `fields = {…}` | 字段默认值 `= bits` + 指令 `match` |
| riscv `bitsets` / `imm_names` + `kind = "bits"` / `names` 槽 | `[enum.*]` + `enum = "<表名>"` |
| riscv `%hi`/`%lo` + `symbols`/`require_symbol`/`imm_fns` | `[asm.immfn.*]` + `operand.symbol{}` |
| riscv `shamt5`/`shamt6` 同段多解 | 两条 form 各自的字段（位段可相交，写进规范） |
| riscv `only_variants` | 条目级 `when` |
| x86 `modrm = { reg = …, rm = … }` | modrm 段字段名 + 指令 `bind` |
| x86 `prefix = ["0xF2","0xF0"]` | 前缀段 `bytes` + `match.prefix` |
| x86 `wrap` / `imm8` 与 `imm8s` 并存 | `literal = "bits"` / 两个 `[operand.*]` |
| x86 VEX/EVEX 三操作数 + `disp_scale` | `vex` / `evex` 段（槽与固定值全在数据里） |
| x86 8 个常量池占位符 | emit 表达式（`{iconst >> 12 & 0xfffff}` 等） |
| x86 `[[pattern]]` 树匹配 | `[lower.*].tree` |
| arm64 `operand_fields = ["rt","rn","rm","vq","vq","vq"]` 凑位置 | 槽有名字，多绑定由 `bind` 表达，无需凑位置 |
| arm64 TBZ 一个操作数摊两段 | 一个多段字段 `u6[23:19,31]:bitpos` |
| arm64 `encode = "slice"` + `fields` | 多段位区间（同一机制） |
| arm64 位域当常量（`one21 = 1` ×80） | 字段默认值 / `= bits` / `reserved` |
| arm64 `zr31` / `unit` / `#` 前缀 + `//` 注释 | `operand.zero,sp` / `unit`（不变）/ `[meta.text]` |
| arm64 98 个 `imm_fn`（49 组 `#`/无 `#` 影子） | `[asm.immfn.*].texts` 多拼写合一 |
| 三谱同编码多形态（歧义名单 82 条） | 保留两条声明但显式（`group` + `match` 差异化）；文档给消歧判据，不靠声明序截胡 |
| 三谱 `[stack]`/`[emit]`/`[types]`/`[spill.*]` | `[machine]` 子表 |

任何"装不下"的项必须写"不支持 + 理由 + 替代做法"并进入待评审决策；截至本稿未发现此类项。

---

## 10. 迁移映射与工作量

### 10.1 按区块

| 区块 | x86 | riscv64 | arm64 | 性质 |
| --- | --- | --- | --- | --- |
| `[[vectors]]` + `[[templates]]` + 文件头 | 15.9% | 20.6% | 23.9% | 100% 机械 |
| `conventions.*`（bitfields 搬进 form） | 1.9% | 16.6% | 10.5% | 约 90% 机械（arm64 49 组影子需人判） |
| `[[instructions]]` | 42.9% | 24.4% | 51.0% | 混合（arm64 字段默认值与保留位需人判） |
| `[[lowering]]` | 29.6% | 28.3% | 1.8% | 混合 |
| 其余结构件 | 约 10% | 约 10% | 约 11% | 约 70% 机械 |

综合：**机械约 60% / 人工约 40%**。

### 10.2 风险排序（迁移时先人工审）

1. arm64 数百个固定字段值（bit 级正确性；25 行注释已被 mojibake 毁掉，不能靠注释回溯）。
2. x86 36 条文本歧义 + **寄存器重定基的"旧新名语义相反"**。
3. riscv 同段多解与投影连带丢引用。
4. 三谱 `[machine]` 机器事实（`padding`/`arg_place`/`byref_bytes`/`link`/`fixed` 最易被误当编码键）。
5. `[[vectors]]` 505 条（唯一能证明"字节没变"的回归网）。

---

## 11. 实施切片路线

> 本节只是路线；本轮不执行。每片三谱同迁，保证仓库始终绿。

| 片 | 范围 | 门禁 |
| --- | --- | --- |
| W0 | 基线取证（本稿 §2 已取） | 数字可复现，命令入档 |
| W1 | `[reg.*]` 单位重定基 + 名字即类/宽（**最危险的原子改名**） | 三谱字节级不变；`RegClass` 内部由字节改位；grep 无旧名残留 |
| W2 | 字段语法：`form.fields` 就地声明 + `ops`/`match`/`bind`（**删除 `bitfields`/`opcode_field`/`operand_fields`**） | `isa_roundtrip_guard` 计数不变；`spec_coverage_guard` 299/174/544 不变 |
| W3 | `[operand.*]` + `[enum.*]` + `[encode.*]` 收敛（8 个开关 → 5 个正交键） | `validate` 三谱零诊断；`schema_guard` 四方同步 |
| W4 | `form.kind = "stream"` 段模型（x86；删除 20 个 form 名与 37 处内联组合） | x86 全量字节闭环 |
| W5 | `family`/`group`/`vary` + 重载解析（删 `ref` 与宽度 `when` 分派） | 字节不变；歧义名单只减不增 |
| W6 | `[lower.*]`：分支/跳转/返回下放 + emit 表达式 + `temps` | 三矩阵计数不变（x86 197/3、riscv 136/64、arm64 23/175）；加第四形态夹具 |
| W7 | 文本层：`asm` 列表 + 修饰符 + `[operand.mem].text` | 反汇编 → 汇编闭合；四档 asm 测试台计数不变 |
| W8 | `[asm.reloc.*]` + 生成补丁器（删宿主按 ISA 名分发） | 新增 ISA 不再需要手写补丁器 |
| W9 | `[machine]` 合并 + `[machine.spill.*]` 按组分派 | 溢出路径守卫 + 矩阵 |
| W10 | 删除 v18 代码路径/文档/守卫；`docs/reference/isa-dsl.md` 换版；索引更新 | `git grep` 无 v18 残留；教程重写 |
| W11 | 通用性实证：新增一个"非 x86/riscv/arm64"夹具 ISA，零编译器改动跑通 | 新守卫：新增 ISA 的 PR 只改 TOML |

---

## 12. 边缘情况与失败模式

| 情况 | v21 规定 |
| --- | --- |
| 同 form 内位段相交 | 允许；同一条指令同时绑定两者才报错 |
| 字段类型放不下寄存器组编号 | 声明期报错（映射失败即非法编码） |
| `uN`/`iN` 宽度 ≠ 字段总位宽 | 声明期报错 |
| 联合类型的多候选同时成立 | 声明期报错并列出候选 |
| `ops`/`match`/`bind` 引用不存在的槽名 | 声明期报错，列出该 form 的槽名 |
| 匿名槽未给 `= bits` | 默认 0（保留位口径） |
| 位段未被任何字段覆盖 | wildcard，不参与匹配；lint `--bits` 报出（不默认报错） |
| 重载歧义（同形状同宽度多条） | 生成期报错并列出候选（不设 pin） |
| 重载无候选 | 生成物内 `Unsupported`（fail-closed） |
| 前缀表缺失 / 用了未声明的段 kind | 报错并列出闭集 |
| `when` 引用未声明的变体参数 / 缺 `default` | 声明期报错（复用 v19 V5 的域校验） |
| 文本两条写法都能匹配 | 按列表序首命中；生产期由 lint 报歧义 |
| `unit` 不是 2 的幂 / 越界 | 声明期报错（沿用现有口径） |
| `asm` 渲染与解析不闭合 | 生成期自测 + `[[vectors]]` 闭环形态守 |

---

## 13. 假设与可评审决策

### 13.1 假设

1. TOML 是唯一载体（已拍板）。
2. 三谱字节级等价是硬验收（迁移不许动编码）。
3. 接受"分支/跳转/返回降级下放到谱"（编译器从"猜形状"改为"提供标签与补丁点"）。
4. 接受删除 `callee_saved_gpr`/`callee_save_slots`（手写后端改走 ABI 层）。

### 13.2 可评审决策

**已冻结（2026-10-08，用户指示开始执行）**：D-1 … D-6 按本表口径执行。
若要改口径，先改本表并在同一提交里同步 [isa-dsl-v21-execution-checklist.md](isa-dsl-v21-execution-checklist.md) §1.1。

| # | 决策 | 本稿口径 | 备选 |
| --- | --- | --- | --- |
| D-1 | "是不是操作数"如何判定 | 由指令的 `ops` 是否列出该槽决定（字段只声明类型/位段/默认值） | 字段上写 `operand = true`（多一个键） |
| D-2 | `[meta]` 五个宽度键 | 删除，改由 `[machine] gpr/fpr/addr = "<组名>"` 指组 | 保留但改按位（仍与组名重复） |
| D-3 | 操作数槽缺 `= bits` 的语义 | 视作 wildcard（不参与匹配）；只有常量槽缺省才是"全 0" | 字面执行"所有字段缺省全 0"（会让寄存器操作数只能匹配 r0） |
| D-4 | 改名机制 | 保留 `bind = { 槽名 = "操作数名" }`（无位置语义） | 在 `ops` 里就地重命名（接近已删的 v14 内联声明） |
| D-5 | 散布位段方向 | `[7,11:8,30:25,31]` 按值低位 → 高位 | 按高位 → 低位（与 v18 `pieces.shift` 换算方向相反） |
| D-6 | 未被字段覆盖的位 | wildcard + lint 报出 | 一律强制 0（会大面积改变 arm64 解码接受集，262 位） |

### 13.3 待原型冻结

**已冻结（2026-10-08）**：冻结项 1 的键名以 W4 定稿回写本节；冻结项 2 保持"不做、只登记为待度量项"；
冻结项 3 允许 `form` 一层 `base` 继承。

1. `stream` 段的最终键名与嵌套（默认见 §6.2.7；备选：段内统一 `slots = {}`）。
2. `[snippet]` 是否引入（默认不做；判据 = riscv 14 处复用按新口径重测的净收益）。
3. `form` 是否允许 `base` 继承（默认允许一层，与 `family` 共用同一套 `base` 语义）。

---

## 14. 附录：审计取证

### 14.1 取证方式

- 读 `docs/reference/isa-dsl.md` 全文（2155 行）并与 `src/schema.rs`（`SECTIONS`/`ENC_KEYS`）、
  `src/dsl/model.rs`（键的类型与 serde 默认）、`isa-dsl.schema.json` 交叉核对；
- 读 `src/dsl/codegen/placeholder.rs`（占位符注册表 38 条）、`src/dsl/pred.rs`（谓词属性）、
  `src/lint.rs`、`src/validate.rs`；
- 全文 grep 字符串字面量比较、`strip_prefix`、`match` 臂，定位 ISA 特化；
- 用脚本统计 `isa/*.toml` 的节结构、同签名指令块、逐字重复行序列、编码键内联次数；
- 与 `crates/tools/forge-tests/asm/ratchet/pins.txt` 的权威计数
  （`spec_totals` = x86 299 / riscv64 174 / arm64 544）对拍。

### 14.2 关键定量结论

- `validate.rs` 校验规则：通用约 14 / 半通用约 12 / ISA 特化 3（`modrm` 位域名、`force_disp_base`、`prefix_scan`），
  另有 2 处"变长 ISA 不支持"缺口。
- `lint` 8 码：`LINT-UNUSED-SLOT`/`FORM`/`BITFIELD`/`BITFIELD-OVERLAP`（默认档）+
  `--ops`/`--refs`/`--bits`/`--suggest`（opt-in）。
- 生成器里**没有**任何 ISA 名或指令助记符字面量（`tests/generality_guard.rs` 的白名单为空）——
  今天的"不通用"不是名字硬编码，而是 §2.5 那 15 处**结构特化**。
- 三谱之间**零重复**的 `conventions.*` 内容；重复出现在 **ISA 与代码缺省之间**
  （x86 的 `prefix_scan` 6/7 条与 `default_prefix_scan()` 逐字节相同 ⇒ 删掉行为不变）。

### 14.3 取证产物去向

本轮子代理在仓库根留下的临时件 `ISA-DSL-后门审计报告.md`（549 行）已按本文档地图纪律删除，
其要点（§2.5 的 15 条特化、§7 的清除表、§14.2 的定量结论）已折入本方案。
