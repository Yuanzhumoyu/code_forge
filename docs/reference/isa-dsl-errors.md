# ISA-DSL 错误码目录（v18 S1）

> 状态：[active]（2026-09-19 起）。对应实现：`crates/frontend/forge-dsl/src/v12/diag.rs`
> （诊断收集与定位）、`validate.rs`（各节校验）、`model.rs`（模型级派生与 gate）。
> 执行方案见 `docs/plans/forge-dsl-v18-plan.md` §7「S1 诊断与校验」。

## 1. 诊断长什么样

一条诊断 = `路径:行:列: 错误码: 消息`，**一行一条**，可直接点击跳到 ISA TOML 的出错处：

```text
D:/repo/isa/x86_v12.toml:1832:1: DSL-INST: [[instructions.SUB_RM_R]]: form 'MRR_TYPO' is not declared in [[forms]]
D:/repo/isa/x86_v12.toml:2101:9: DSL-LOWER: [[lowering.Isub]].when: 未知属性 'rd_width'（可用：rd/rs1_width/…）——未知属性恒为假，规则永不命中
```

- **行:列**：由**声明索引**给出（一次预扫建立"节 + 名字 → 块行范围"），不会像旧实现那样
  用 `source.find` 全局搜名字而指到别处；块内还能进一步精确定位到**出错的那个键行**
  （消息里用引号点名了出错的值，如 `'MRR_TYPO'`）。
- **一次列全**：校验不再 fail-fast，按"节 + 逐条声明"收集，最多 32 条，超出时末尾追加
  `（另有 N 条错误未列出；上限 32 条）`。
- **附注**：同一处声明被重复定义时，会额外给 `= 注：同名声明也出现在 行:列`。

## 2. 错误码 = 出错所在节

错误码是**节级的**（稳定、可用于过滤/统计），细粒度信息在消息里。

| 错误码 | 对应节 | 典型问题 |
| --- | --- | --- |
| `DSL-TOML` | TOML 语法/结构 | 键名打错（`deny_unknown_fields`）、表重复、缺必填字段 |
| `DSL-META` | `[meta]` | ISA 名非法、`default_inst_width` 与 `variable_length` 冲突、`comment_char` 不是单字符 |
| `DSL-REG` | `[reg.*]` | 缺 GPR 组、`names`/`count` 不一致、组名宽度非法、生成式声明参数不全 |
| `DSL-STACK` | `[stack]` | `slot`/`align`/`fp_save` 为 0 或未指向已声明组 |
| `DSL-TYPES` | `[types]` | 类型名非法、目标寄存器组未声明、类宽 < 类型宽（会静默截断） |
| `DSL-CONV` | `[conventions.*]` | 位域越界/重叠、`modrm` 字段未声明、条件码表为空/`code` 超 4 位/`ir` 名非法或重复映射、`cond` 槽却没有表、用了 `{cc}` 却没映射全 10 个 IR 条件、`prefix_scan` 条目非法 |
| `DSL-SLOT` | `[[operand_slots]]` | 重名、`kind` 与字段不匹配、`imm` 宽为 0、`class`/`classes` 未声明、`byte_reg` 用错组 |
| `DSL-FORM` | `[[forms]]` | 位域未声明、既无 `opcode_field` 也无 `modrm`、`modrm` 引用不存在的操作数 |
| `DSL-INST` | `[[instructions]]` | 指令重名、`form` 未声明、**操作数槽未声明**、角色与槽 `roles` 不符、定宽字段未声明、操作数多于 `operand_fields`、缺少编码信息、`ref` 为空/与指令名冲突、`reloc` 名字未声明/绑定的槽不在操作数里 |
| `DSL-RELOC` | `[[reloc]]`（v18 S3d） | 名字为空/重复、`slot` 未声明或不是 `imm` 槽、`semantics` 不是宿主已知语义（由 serde 在反序列化期拒绝） |
| `DSL-DERIVE` | `[[derive]]`（v18 S3f） | 名字为空/重复/与核心谓词属性重名（解析期）、`expr` 不是合法谓词、引用了未知属性或另一个派生（提示"派生不能引用派生"） |
| `DSL-TEMPLATE` | `[[templates]]` 展开出的实例 | 实例的 `form` 未声明、操作数槽未声明、缺少编码信息等——消息前缀是 `[[templates.X]]`（X = 模板名），模板行本身的错误同样归这里 |
| `DSL-LOWER` | `[[lowering]]` | 引用名未声明、占位符未知、**`when` 属性未知**（恒假 ⇒ 规则永不命中）、完全重复、死规则 |
| `DSL-PATTERN` | `[[pattern]]` | 匹配树语法错、内部节点用 Fcmp/Icmp/Copy/Nop、叶变量重复、`when` 属性未知 |
| `DSL-ABI` | `[abi]` | 寄存器名未在 `[reg.*]` 声明（scratch/reserved/ret_regs/call_clobbers/call_ret_reg/callee_saved/arg_class） |
| `DSL-EMIT` | `[emit]` | 指令引用未声明、`@` 伪指令未知、占位符未知、块为空 |
| `DSL-SPILL` | `[spill.*]` | 指令引用未声明、占位符不是 `{N}`、`base` 未在 `[reg.*]` 声明 |
| `DSL-INCLUDE` | `include`（v18 S7） | 文件缺失、循环 include、同名标量冲突 |
| `DSL-OTHER` | 无节可归 | 模型级错误（如缺 `[reg.*]` 顶层派生失败）——**出现即说明该消息缺节前缀，应报 bug** |

## 3. 常见修法与"为什么这样报"

### 3.1 `[[instructions.X]]: operand slot 'Y' is not declared in [[operand_slots]]`

`asm` 里的 `{Y}` 或 `ops = ["…:Y…"]` 指向了不存在的槽。修法：在 `[[operand_slots]]` 里声明
`name = "Y"`，或改 `asm`/`ops` 的引用。**为什么硬报**：v13 之前这里会静默生成一个空/零宽度
操作数，产出能编译但语义错的编码。

### 3.2 `[[lowering.OP]].when: 未知属性 'A'（可用：…）`

谓词属性拼错（`rd_width` vs `rd`）。**为什么硬报**：未知属性在求值时恒为假 ⇒ 规则**永不命中**，
既不报错也不生效，是历史上最难查的一类缺陷。v18 起可用 `[[derive]]` 声明派生属性，
不必改 Rust（S3）。

### 3.3 `[[lowering.OP]]: 第 N 条规则是死规则`

按裁决序（`priority` 降 / 谓词叶子数降 / 声明序升）前面的规则已完全覆盖它 ⇒ 它永不生效。
修法：删掉，或给它更高的 `priority`；若本意是"更具体的先匹配"，把它写成更窄的谓词。

### 3.4 `[emit.prologue].insts[i]: 未知指令引用 'X'` / `未知伪指令 '@x'` / `未知占位符 '{x}'`

`[emit]` 与 `[spill.*]` 模板只能引用**已声明指令名或引用名（指令的 `ref`）**，`@` 伪指令只认
`@push_callee` / `@pop_callee` / `@frame_alloc` / `@frame_free` / `@move_args`，
`[emit]` 占位符只认 `{frame_size}` / `{frame_size_neg}` / `{frame_size_mN}` / `{callee_saved_bytes}`，
`[spill]` 只认编号 `{N}`。**为什么硬报**：S0 基线实测这些位置**完全不校验**——把
`MOV64_RR` 写成 `MOV64_R` 要等到生成代码编译甚至运行时才暴露（见方案 §12.4）。

### 3.5 `[[templates.X]]: …`（v18 S2c）

模板展开发生在**解析期**，因此模板自身的错误是 `DSL-TOML`（解析阶段，带 `[[templates.X]]`
前缀与行号），展开出的指令若非法则报 `DSL-TEMPLATE`（消息前缀是 `[[templates.X]]`，X = 模板名）。
无论哪种，位置都落在**模板声明**上（块内还会精准到出错的那一行 / 那个 `inst`），不会指向源里
不存在的 `[[instructions.实例名]]`。

常见消息与修法：

| 消息 | 修法 |
| --- | --- |
| `rows 不能为空（模板至少要一行）` | 模板至少要写一行 `{ inst = "…" }` |
| `第 N 行的 inst 不能为空` | 补上 `inst`（= 指令名，全 ISA 唯一） |
| `行 'NAME' 的字段非法：unknown field 'k'` | 行键写错了：`body` 里有同名键 = 覆盖，body 用 `{k}` 引用的 = 纯参数，其余都会被 `Instruction` 当字段解析 |
| `指令缺少编码信息——至少要给 opcode/fields，或 opcode_reg/modrm/vex/evex/imm 之一` | 该指令没有任何编码来源（`form` 只给 `opcode_field` 之类的修饰不算） |
| `ref 不能为空` / `ref 'X' 与指令名冲突（引用名与指令名同池）` | `ref` 要么省略，要么给一个不与任何指令名重名的非空名字 |
| `duplicate instruction name 'X'` | 模板实例名与别的指令/实例重名（模板之间、模板与手写指令之间同池） |
| `body 必须是内联表（指令字段的集合）` | `body = { … }`，不能是字符串/数组 |

### 3.6 条件码（`[conventions.cond]`，v18 S3b）

键 = 本 ISA 汇编/反汇编可见的条件名，`code` 是编码，`ir` 是它实现的 IR 整数条件。

| 消息 | 修法 |
| --- | --- |
| `条件码表不能为空（要么整节不写，要么至少一条）` | 删掉空的 `[conventions.cond]`，或补上条目 |
| `code M 超出条件字段宽度（4 位，0..=15）` | 条件码占 opcode 低 4 位（定宽 ISA 的 cond 位域同理），编码值改到 0..=15 |
| `ir 'X' 不是 IR 整数条件名（可用：eq / ne / …）` | `ir` 只认那 10 个名字；纯汇编别名请省略 `ir`（键名不是 IR 条件名时自动视为别名） |
| `IR 条件 'eq' 被 'e' 与 'e2' 重复映射` | 一个 IR 条件只能映射到一个编码（否则 `{cc}` 取谁没有答案） |
| `操作数槽 'X' 的 kind = "cond" 需要条件码表` | 声明 `[conventions.cond]`（v18 S3b 起不再回退 x86 的 16 项表） |
| `用了 {cc} 但 [conventions.cond] 没把所有 IR 整数条件映射全——缺 slt / uge` | 给缺的条件各找一条汇编名加 `ir = "<条件名>"`（漏映射会在运行期静默退化成 0 = 溢出条件） |

### 3.7 位置看起来不对？

- 诊断指向**声明行**（`name = …` / `op = …` / 节头）是正常的；
- 若消息里用引号点名了出错的值（`'MRR_TYPO'`、`'rd_width'`、`'{bogus}'`），定位会进一步
  收到**该值所在的行**（限制在同一个声明块内，不会跨声明乱指）；
- 完全抽不出节/名字时退化为 `1:1`——这种情况属于应当补节前缀的消息，欢迎报 bug。
