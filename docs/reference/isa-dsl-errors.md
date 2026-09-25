# ISA-DSL 错误码目录（v18）

> 状态：[active]（2026-09-19 起；2026-09-20 补 `DSL-ENCODING` 与 §3.7 `[encoding]` 三态；
> 2026-09-21 补 §3.8 多文件组合与 `parts`）。对应实现：
> `crates/frontend/forge-isa-dsl/src/v12/diag.rs`（诊断收集与定位）、`validate.rs`（各节校验）、
> `model.rs`（模型级派生与 gate）、`src/loader.rs`（多文件组合的加载期错误）。
> 语法规范见 [`docs/reference/isa-dsl.md`](isa-dsl.md)，教程见
> [`docs/guides/isa-dsl-tutorial.md`](../guides/isa-dsl-tutorial.md)；
> 执行方案见 [`docs/archive/forge-dsl-v18-plan.md`](../archive/forge-dsl-v18-plan.md) §7。

## 1. 诊断长什么样

一条诊断 = `路径:行:列: 错误码: 消息`，**一行一条**，可直接点击跳到 ISA TOML 的出错处：

```text
<repo>/isa/x86_v12.toml:1832:1: DSL-INST: [[instructions.SUB_RM_R]]: form 'MRR_TYPO' is not declared in [[forms]]
<repo>/isa/x86_v12.toml:2101:9: DSL-LOWER: [[lowering.Isub]].when: 未知属性 'rd_width'（可用：rd/rs1_width/…）——未知属性恒为假，规则永不命中
```

（上例是**示意**（把某条指令的 form 名写错、把谓词属性名写错后实测的输出形状）：路径与行号随
谱内容漂移，以符号名与消息文本为准。跑 `forge-isa validate <你的谱>` 得到的才是当前真实位置。）

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
| `DSL-META` | `[meta]` | ISA 名非法、`comment_char` 不是单字符、宽度键指向未声明的组、**变体参数未声明或取值越界**（`--params`/`params` 传了 `[meta].variants` 里没有的名字，或取值不在声明域内，v19 V5） |
| `DSL-ENCODING` | `[encoding]`（v18 S4） | `kind` 与键结构不匹配（`fixed` 写 `widths`/`max_len`、`prefix_scan` 写 `bits`、`mixed` 写 `max_len`）、`widths` 为空/重复/含 0、`bits = 0`、`default_opsize = 0`、逐指令 `width` 不在 `widths` 里或与 `bits` 不一致、`prefix_scan` 下写 `width` |
| `DSL-REG` | `[reg.*]` | 缺 GPR 组、`names`/`count` 不一致、组名宽度非法、生成式声明参数不全 |
| `DSL-STACK` | `[stack]` | `slot`/`align`/`fp_save` 为 0 或未指向已声明组 |
| `DSL-TYPES` | `[types]` | 类型名非法、目标寄存器组未声明、类宽 < 类型宽（会静默截断） |
| `DSL-CONV` | `[conventions.*]` | 位域越界/重叠、`modrm` 字段未声明、条件码表为空/`code` 超 4 位/`ir` 名非法或重复映射、`cond` 槽却没有表、用了 `{cc}` 却没映射全 10 个 IR 条件、`prefix_scan` 条目非法 |
| `DSL-SLOT` | `[[operand_slots]]` | 重名、`kind` 与字段不匹配、`imm` 宽为 0、`class`/`classes` 未声明、`byte_reg` 用错组 |
| `DSL-FORM` | `[[forms]]` | 位域未声明、既无 `opcode_field` 也无 `modrm`、`modrm` 引用不存在的操作数 |
| `DSL-INST` | `[[instructions]]` | 指令重名、`form` 未声明、**操作数槽未声明**、角色与槽 `roles` 不符、定宽字段未声明、操作数多于 `operand_fields`、缺少编码信息、`ref` 为空/与指令名冲突、`reloc` 名字未声明/绑定的槽不在操作数里、`asm` 里的 `{参数名}`（`[meta].variants`）**本次没传值**（v19 V5） |
| `DSL-RELOC` | `[[reloc]]`（v18 S3d） | 名字为空/重复、`slot` 未声明或不是 `imm` 槽、`semantics` 不是宿主已知语义（由 serde 在反序列化期拒绝） |
| `DSL-DERIVE` | `[[derive]]`（v18 S3f） | 名字为空/重复/与核心谓词属性重名（解析期）、`expr` 不是合法谓词、引用了未知属性或另一个派生（提示"派生不能引用派生"） |
| `DSL-PSEUDO` | `[[pseudo]]`（v18 S3e） | 名字为空/重复/与指令助记符重名、`params` 为空或重复、`emit` 为空或有空行、emit 行首词既不是指令助记符也不是别的伪指令、`{…}` 不是声明的参数、参数没被用到 |
| `DSL-TEMPLATE` | `[[templates]]` 展开出的实例 | 实例的 `form` 未声明、操作数槽未声明、缺少编码信息等——消息前缀是 `[[templates.X]]`（X = 模板名），模板行本身的错误同样归这里 |
| `DSL-LOWER` | `[[lowering]]` | 引用名未声明、占位符未知、**`when` 属性未知**（恒假 ⇒ 规则永不命中）、完全重复、死规则 |
| `DSL-OVERLAP` | `[[lowering]]`（v19 V6b，**仅 `validate --strict-overlap`**） | 同 op 两条规则的取值域**相交但互不包含**（部分重叠）：裁决序里前者先命中。**默认档不报**——真谱里"特化 + 兜底"遍地都是（实测三 ISA 共 61 条，全是合法写法）；该档是**评审清单**，不是错误判据 |
| `DSL-PATTERN` | `[[pattern]]` | 匹配树语法错、内部节点用 Fcmp/Icmp/Copy/Nop、叶变量重复、`when` 属性未知 |
| `DSL-ABI` | `[abi]` | 寄存器名未在 `[reg.*]` 声明（scratch/reserved/ret_regs/call_clobbers/call_ret_reg/callee_saved/arg_class） |
| `DSL-EMIT` | `[emit]` | 指令引用未声明、`@` 伪指令未知、占位符未知、块为空 |
| `DSL-SPILL` | `[spill.*]` | 指令引用未声明、占位符不是 `{N}`、`base` 未在 `[reg.*]` 声明 |
| `LINT-*` | `forge-isa lint`（**静态体检，不是校验错误**） | `LINT-UNUSED-SLOT`/`LINT-UNUSED-FORM`/`LINT-UNUSED-BITFIELD`（默认档）、`LINT-BITFIELD-OVERLAP`（默认档）、`LINT-OP-GAP`（`--ops`）、`LINT-REF-UNUSED`（`--refs`）、`LINT-UNASSIGNED-BITS`（`--bits`）、`LINT-VARY-CANDIDATE`（`--suggest`，只建议）——判据与档位见 `docs/reference/isa-dsl.md`「静态体检」 |
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
`@push_callee` / `@pop_callee` / `@frame_alloc` / `@frame_free`，
`[emit]` 占位符只认 `{frame_size}` / `{frame_size_neg}` / `{frame_size_mN}` / `{callee_saved_bytes}`，
`[spill]` 只认编号 `{N}`。**为什么硬报**：S0 基线实测这些位置**完全不校验**——把
`MOV64_RR` 写成 `MOV64_R` 要等到生成代码编译甚至运行时才暴露（见方案 §12.4）。

其中一个名字有**专门的迁移提示**：`@move_args` 已删除（v20 A3b-2b-2b）——收参属于
**调用约定**，由生成器按 forge-abi 的调用布局统一发射，谱里不再写它（把它从模板里
删掉即可；`[emit.prologue]` 因此可以为空/缺席，收参照样发射）。

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

### 3.7 指令宽度三态（`[encoding]`，v18 S4）

`[encoding]` 是**独立的段**（`[meta]` 不再有 `default_inst_width`/`variable_length`/
`max_inst_len`/`default_opsize`）。三态的键结构性互斥，在**校验期**就报，不会留到生成期。

| 消息 | 修法 |
| --- | --- |
| `[encoding].bits 缺失：kind = "fixed" 必须声明指令字宽` | 只出现在**完全省略 `[encoding]`** 的骨架文档上，且由生成期（`inst_bytes()`）报——补上 `[encoding] bits = <位>` |
| `[encoding].widths 只适用于 kind = "mixed"` | `fixed` 只有一个字长（写 `bits`）；要混合字长把 `kind` 改成 `"mixed"` |
| `[encoding].max_len 只适用于 kind = "prefix_scan"` | `max_len` 是前缀扫描式变长的上限；定宽/混合不需要 |
| `[encoding].bits 只适用于 kind = "fixed"/"mixed"` | `prefix_scan` 是逐指令变长，删掉 `bits`（最长长度写 `max_len`） |
| `[encoding].widths 不能为空` / `widths 里的字长必须 > 0` / `widths 里有重复字长` | `mixed` 必须给出非空、无重复、每项 > 0 的字长集 |
| `[encoding].bits 必须也在 widths 里（它是 width 缺省值）` | `bits` 是逐指令 `width` 的缺省，必须属于 `widths` |
| `[[instructions.X]]: width N 不在 [encoding].widths (…) 里` | 逐指令字长必须是 `widths` 的成员（解码按字长分组，成员外无法解码） |
| `[[instructions.X]]: width N 与 [encoding].bits (M) 不一致` | `fixed` 全 ISA 一个字长；`width` 只是自解释的重复，写别的值就是错的 |
| `[[instructions.X]]: width = N 不能替代 [encoding].bits` | `fixed` 且没写 `bits`：请在 `[encoding].bits` 声明一次，而不是逐条写 `width` |
| `[[instructions.X]]: prefix_scan ISA 不得写 width` | 前缀扫描式的字长由前缀链决定，删掉 `width` |

### 3.8 多文件组合与部件选择（v18 S7d）

`include` / `[[override]]` 在**加载期**（`forge-isa-dsl::loader`）处理，所以这类错误不带
`路径:行:列`（还不是"某一行的语法/语义错"），但一定点名**具体文件或键**：

| 消息 | 修法 |
| --- | --- |
| `Cannot read ISA file 'nope.toml'` | `include` 里的路径相对**写这一行的文件**解析；检查拼写与相对位置 |
| `include 成环 / 同一文件被包含两次` | 公共片段只能被包含一次（A→B→A 也会被拒）；把公共部分再抽一层或改用 `[[override]]` |
| `同名标量冲突：<键> 在 <文件A> 与 <文件B> 都给了值` | 表节跨文件合并，但同一个**标量键**只能有一处给值；要覆盖就写 `[[override]]`（消息里 `<键>`/`<文件>` 是占位符，实际会填上键名与两个来源文件） |
| `[[override]] key = "…" 在任何文件里都没有对应的 \`… = …\` 行` | 覆盖只能改**已存在**的键（拼错了？还是要新增键？新增直接写在根文件里） |
| `[[override]] key = "…" 缺少 \`value\`` | 显式覆盖必须给新值 |
| `[[override]] key = "…"：目标表 [节名] 在合并结果里不存在` | 被包含文件里没有这一节（先确认节名，或把该节写进根文件） |
| `include 需要经多文件加载器展开`（或 `[[override]]` 版本） | 用 `isa_from_file!` / `forge-isa` CLI / `forge_isa_dsl::expand_file` 这些入口（它们自动处理 include 与 override），不要绕过去把裸文本交给解析器 |

`isa_from_file!` 的 `parts` 参数是**编译期**错误（不是诊断）：

| 消息 | 修法 |
| --- | --- |
| `parts = [encode] 时不能生成生成期自测（\`__spec_tests\` 需要 encode/decode/asm 全部）——请显式写 \`spec_tests = false\`，或去掉 parts` | 关掉自测或放开部件 |
| `未知部件 \`encoder\`（可用：encode / decode / asm / tm）` | 部件名只有四个 |

### 3.9 `[[lowering]].op` 名单（v18 S5）

`op` 也可以写成一组同类 op（`op = ["Copy", "Uextend", "Freeze"]`）；名单在**解析期**展开成
逐 op 的规则（顺序 = 名单序），因此下面两条是解析期错误：

| 消息 | 修法 |
| --- | --- |
| `[[lowering.X]].op: op 名单不能为空` | `op = []` 无意义：删掉这条规则，或写具体 op |
| `[[lowering.X]].op: op 名不能为空` | 名单里混进了空串 |
| `[[lowering.X]].op: op 'Y' 在名单里重复` | 同一个 op 在一条规则里列了两次（展开后会撞车） |

### 3.10 `[[pattern]]` 死模式（v18 S5c）

| 消息 | 修法 |
| --- | --- |
| `[[pattern]] #N 是死模式——裁决序里靠前的 [[pattern]] #M（同一匹配树，priority 降 / Op 节点数降 / when 叶子数降 / 声明序升）已覆盖它的全部取值域` | 两个模式**匹配树相同**、前者的 `when` 覆盖后者：删掉后者，或给它更高的 `priority`（与 `[[lowering]]` 的死规则同语义） |

不同匹配树之间的覆盖关系不做推断——若两个**不同**的树其实覆盖同一批输入，需要作者自己确认。

### 3.11 变体投影（`--params` / `only_variants`，v19 V5）

| 消息 | 修法 |
| --- | --- |
| `[meta].variants: 传了参数 \`x=1\`，但谱里没有声明它（已声明：…）` | 在 `[meta].variants` 里声明该参数（`variants = { xlen = [32, 64] }`），或检查参数名拼写；谱完全没有变体机制时消息会写"没有声明任何变体参数" |
| `[meta].variants: 参数 \`xlen = 128\` 不在声明域 [32, 64] 内` | 取值超出声明域——改参数值，或（确实需要）扩声明域 |
| `[[instructions.X]].asm: 占位符 '{width}' 是**变体参数**（\`[meta].variants\`）但本次没有传值` | 参数化模板必须显式传参：CLI `--params width=32`、宏 `params = { width = 32 }`。**默认档不许留参数占位符**——留着会让生成的汇编打印出字面 `{width}` |
| `[emit.prologue].insts[i]: 未知指令引用 'SD'`（投影后才出现） | 该块引用了被投影掉的指令：给这个块补 `only_variants = { xlen = [64] }`（或为变体写一份自己的块）。**不给静默通道**是故意的——RV32 的帧件确实与 RV64 不同 |
| 投影没丢东西（账目里 `-0`） | 检查 `only_variants` 是否写在了**被模板展开的**声明上（`[[templates]].body` / `rows` 都行），以及 `params` 是否真的传了 |

投影的"丢了什么"永远打印在 `forge-isa insts --params …` 的第一行（`--json` 里是
`projection` 字段）——**看不到账目就说明没投影**。

### 3.12 位置看起来不对？

- 诊断指向**声明行**（`name = …` / `op = …` / 节头）是正常的；
- 若消息里用引号点名了出错的值（`'MRR_TYPO'`、`'rd_width'`、`'{bogus}'`），定位会进一步
  收到**该值所在的行**（限制在同一个声明块内，不会跨声明乱指）；
- **多文件谱**：诊断里的 `路径:行:列` 是**来源文件**里的位置（合并前的行号），不是"合并文本的
  第 N 行"——直接点开就能看到出错的那一行；
- 完全抽不出节/名字时退化为 `1:1`——这种情况属于应当补节前缀的消息，欢迎报 bug。
