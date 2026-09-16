# forge-ir

`forge-ir` 是 code-forge 的**目标无关中间表示**（SSA + 基本块 + 显式块参数），
被 `forge-opt`（优化）、`forge-codegen`（后端 lowering）、`forge-hir`（前端降级）、
`forge-object`（目标文件）与 `forge-rustc`（rustc 后端）消费。

```text
前端（HIR / mini_c / LLVM 文本）
        │  FunctionBuilder / ir_parser
        ▼
   forge-ir（本 crate）──► forge-opt（pass 流水线）──► forge-codegen（机器 IR + 编码）
        │                                                        │
        └────────────── forge-object（COFF/ELF/Mach-O）◄──────────┘
```

## 结构

| 模块 | 内容 |
| --- | --- |
| `entity` | 实体句柄：`Value`/`Inst`/`Block`/`TypeId`/`FuncRef`/`ConstId`/`GlobalId`/`SigRef`/`VReg`/`XReg`/`PReg`/`RegClass` |
| `entity_map` | 密集索引容器：`PrimaryMap`/`SecondaryMap`/`EntitySet`/`PackedOption` + `EntityRef`（S2） |
| `dfg` | `DataFlowGraph`（`values`/`insts`/`blocks` 三个 arena——`values` 已私有化，见「arena 访问收口」）、`Instruction`、`BlockData` |
| `function` | `Function`、`Module`、`Layout`、`GlobalVariable`/`GlobalAlias`（`analysis` 字段是 `AnalysisManager`） |
| `types` | `TypeStore`（interner）、`TypeContext`、`FunctionSignature` |
| `opcode` / `immediate` / `inst_flags` / `mem_flags` / `isel_strategy` | 指令操作码与附件（`opcode` 的枚举与派生表由 `ops.toml` 生成，见下） |
| `builder` | `FunctionBuilder`（构造 IR 的唯一推荐入口） |
| `use_list` / `analysis` / `loop_info` / `alias` | def-use 链、支配树与**惰性分析缓存管理器 `AnalysisManager`**（修订号自校验 + `Arc` 快照）、循环森林、最小别名分析 |
| `constant` / `big` / `imm_str` / `string_pool` | 常量池（int/float/big/vector/aggregate）、任意精度、SSO 字符串 |
| `verify` | `Verifier`（实测 31 个错误码 `VerifyError` / 18 个 `check_*` 检查函数 + 墓碑规范形态 `TombstoneNotCanonical`、严重级 `VerifySeverity`（`UnreachableBlock` 为唯一建议级）） |
| `display` / `ir_parser` | LLVM 文本输出（logos + lalrpop 解析） |
| `metadata` / `debug_info` / `symbol` / `data_layout` | 元数据、调试信息、符号、DataLayout 与 target triple（只作数据，不提供按架构名猜属性的查询——见「开放集合的边界」） |

crate 根的 **`ops.toml`** 是指令清单与派生属性的**单一事实源**：`build.rs` 读它生成
`$OUT_DIR/opcode_gen.rs`（`Opcode` 枚举、`ALL`/`INFOS`、名字与助记符映射、
`result_count`/`expected_operand_count`/`may_ub`/`has_side_effect`/`cond`/`llvm`/
`type_rule`）。**新增一个 opcode = 加一行 `[[op]]`**；生成物与枚举同源，不可能漂移
（`Opcode::info()` 是无兜底臂的穷举 match，查表都是生成的 `match` ⇒ O(1)）。

比较条件（`Icmp`/`Fcmp`）走 immediate 通道（`Immediate::IntCC`/`FloatCC`，
`ops.toml` 的 `cond` 声明该契约、`Opcode::cond_kind()` 暴露它）；条件的数字表示是
`IntCC::code()`（1..=10）/`FloatCC::code()`（1..=16），ISA TOML 的 `cond` 谓词与宿主
`LowerCtx.current_immediates` 共用这一份映射。`Verifier` 对缺失/类型不对的条件报
`MissingCondImmediate`/`WrongCondImmediate`。

逐指令**类型规则族**同样声明在 `ops.toml`（`type_rule`，12 族）：`Verifier` 按族
分派（穷举 match），形状类规则的实现只在 `src/type_rules.rs` 一份（`check_shape`，
builder 侧作为 debug 工具保留）；`Convert` 族的事实表是
`convert = { src, dst, width }`。

## arena 访问收口

`DataFlowGraph` 的三个 arena 曾全 `pub`，下游能绕过 use-lists 直改操作数（S0 诊断
里的"use-def 可被绕过"）。v3 S5 的收口**按 arena 分切片**推进（每个 arena 一次
提交内完成迁移，不留双入口）：

| arena | 状态 | 读 | 写（唯一入口） |
| --- | --- | --- | --- |
| `values` | **已私有**（`pub(crate)`） | `value_data`（fail-closed）/ `value_data_opt`（容忍坏 IR）/ `value_def` / `value_type` / `values()` / `value_count` | `set_value_type`（pass 精化类型）；创建/墓碑化仍在 `dfg.rs` 内 |
| `insts` | **已私有**（`pub(crate)`） | `inst_data`（fail-closed）/ `inst_data_opt`（容忍坏 IR）/ `inst_opcode` / `inst_operands` / `inst_results` / `inst_block` / `insts()` / `inst_count` | `inst_mut` / `inst_mut_opt`（`(dfg, Inst)` 就地编辑口）；结构性增删只有 `make_inst*` / `remove_inst` |
| `blocks` | **已私有**（`pub(crate)`） | `block`（fail-closed）/ `block_opt`（容忍坏 IR）/ `block_data_iter` / `blocks()` / `block_count` / `block_params` / `block_param_values` / `block_terminator` / `block_inst_iter` | `block_mut`（就地编辑口）；结构性增删只有 `make_block*` / `remove_block` |

`*_data` 与 `BlockData::terminator` 同一契约：句柄不合法（越界/来自别的 DFG）
即 panic——坏 IR 只有校验器/display 这类容忍方能用 `*_opt` 读口。

`inst_mut` 的**契约**：安全字段是 `opcode`/`immediates`/`flags`/`mem_flags`/
`param_attrs`/`fn_attrs`/`metadata`/`loc`/`isel_strategy`；**`operands`/`results`
不在此列**——改操作数会绕过 use-lists，推荐路径是
`Function::{replace_all_uses, apply_replacements}`，或"就地改写 +
`Function::refresh_inst_uses` 重登记"（后者是本口的既有用法，
`tests/dfg_privatization.rs::inst_mut_operand_edit_needs_refresh` 钉住）。

`block_mut` 的**契约**：`inst_order` 是**块内指令顺序的唯一事实源**，只在
"指令插入/搬移"的实现里改（`make_inst*` 追加、`move_insts_to` 重排）；改
`params`/`param_values` 必须与 `Function::{add_block_param, remove_block_param}`
口径一致（后者同步值表与 use-lists）；`terminator` 字段已私有，写终结符走
`Function::{jump, branch, ret, …}`。

迁移面实测：`values` 31 处索引 + 7 处 `get(..)`（其中 3 处写）；`insts`
216 处索引 + 8 处裸 `dfg.insts[..]` + 11 处 `get(..)` + 3 处 `get_mut(..)` +
1 处 `iter()` + 25 处 `&mut …`，共 39 个文件；`blocks` 75 处索引 + 1 处裸
`dfg.blocks[..]` + 40 处 `len()` + 23 处 `iter()` + 8 处 `&mut …` + 1 处
`get(..)`（另含 `benches/` 1 处）。守卫 `tests/dfg_privatization.rs` 断言
`src/` 里 `dfg.values`/`dfg.insts`/`dfg.blocks` **字段**访问为 0（迭代访问器
`values()`/`insts()`/`blocks()`/`block_data_iter()` 与 `insts_iter_mut()` 不算；
已用负向探针验证会失败）。

## metadata 单写

附件 metadata（`AttachedMetadata` = `(kind, node)` 对）在 5 个载体上各只有**一个
写入口**，字段私有（v3 S5）：

| 载体 | 读 | 写（唯一入口） |
| --- | --- | --- |
| 指令 | `Instruction::metadata()` | `Instruction::attach_metadata()` |
| 终结符 | `DataFlowGraph::term_metadata()` | `DataFlowGraph::attach_term_metadata()`（返回 `TermMetadataAttach`） |
| 函数 | `Function::metadata()` | `Function::attach_metadata()` |
| 全局变量 | `GlobalVariable::metadata()` | `GlobalVariable::attach_metadata()` |
| 别名 | `GlobalAlias::metadata()` | `GlobalAlias::attach_metadata()` |

创建期的初始表仍走 `make_inst_with_meta_and_loc` 的构造参数（不是"事后补写"）。
追加语义统一：同一 kind 再次附加即多一条（与文本里多处 `!dbg !N` 一一对应；
重复 kind 的合并/拒绝不在本步范围）。终结符的 `unreachable` 不接受附件
（文本里没有可挂的位置），未终止则是坏 IR——两种"没写成"的原因由返回值
`TermMetadataAttach::{Attached, NoTerminator, Unreachable}` 分开报，调用方
（解析器）据此给精确诊断。反回潮守卫见 `tests/metadata_single_write.rs`
（含源码断言"写入只能出现在唯一写入口的实现体里"，已用负向探针验证会失败）。

## 开放集合的边界

`Instruction`/`Module` 上的字符串与"可扩展标签"按**三分**划边界（v3 S5）：

| 类别 | 取值来源 | 表示 | 例子 |
| --- | --- | --- | --- |
| 闭合集合 | 单一事实源闭死 | 生成枚举 / 结构体 | opcode（`ops.toml`）、`Icmp/Fcmp` 条件码、指令类别与效果、终结符种类、`MetadataKind` 的 well-known 项 |
| 目标/ISA 数据 | ISA 或目标数据声明 | `ImmStr` / 专用类型 | 寄存器类与宽度、栈槽与对齐、指令字宽、pattern 名、`IselStrategy`、`TargetTriple` 各段 |
| 用户程序数据 | 用户源码/输入决定 | `ImmStr` | 函数/块/值/全局/结构体/`section` 名、`Immediate::String`、`source_filename`、`module asm` |

规则一：**不得从后两类的字符串反推第一类或任何数值**——那是宿主白名单，表外
取值只会得到静默错误答案（历史实例：`TargetTriple::{is_32bit, is_64bit,
os_name}` 三个架构名/OS 名查表，表外架构两个都返回 `false`；已于 S5 第 2 项
删除）。数值一律来自 ISA 数据，例如指针宽度 = `DataLayout` 的 `p:<size>:<abi>`
（由 ISA `[meta] addr_width` 派生），与 `target triple` 的架构名无关。

规则二：后两类的字符串数据一律用 `ImmStr`（SSO 内联 + `Arc<str>` 共享，
`Clone` O(1)），IR 公开面不出现裸 `String` 字段（`src/ir_parser/**` 的解析期
AST 与诊断消息按设计例外）。两条规则由 `tests/open_set_boundary.rs` 钉住
（行为断言：未知架构名原样往返、宽度只跟布局数据走；源码断言：不得重现代码表、
公开面不得出现 `String` 字段，且已用负向探针验证守卫会失败）。

## 使用要点

- **构造 IR**：用 `FunctionBuilder`（`create_entry_block` → `switch_to_block` →
  `iadd`/`load`/`brif`/`ret` … → `finish()`）。`finish()` 只校验"每块有显式终结符"，
  完整一致性校验请用 `Verifier::with_ctx(func.types.clone())`。
- **类型上下文**：`Function` 自带 `TypeContext`；跨函数/模块展示或校验时，
  **函数与 `Module` 必须共享同一个 `TypeContext`**（否则类型 id 在对方的 store 里越界；
  历史实现会在 `StringPool::lookup` 处 panic）。`Verifier::new()`（无 ctx）现在会返回
  `VerifyError::MissingTypeContext`——8 类类型相关检查无法执行时**不静默放宽**。
- **修改 IR**：优先用原子原语 `Function::{replace_all_uses, kill_inst,
  replace_all_uses_and_kill, remove_block_param, apply_replacements}`——它们保持
  use-lists 新鲜。终结符**就是指令**（v3 S4 主体），所以 `replace_all_uses` 天然
  覆盖分支实参/`ret` 返回值等终结符用值。
- **终结符**：载体是一条终结符指令（`Opcode::{Ret, Jmp, Br, Switch, Unreachable,
  Invoke, Resume}`，存 `insts`、不进 `inst_order`，由 `BlockData.terminator`
  引用）；读取经投影访问器 `DataFlowGraph::{term_kind, term_branch, term_jump,
  term_return_values, term_switch, term_invoke, term_args_to, block_successors, …}`，
  写入经按形式写入口 `Function::{jump, branch, ret, switch, unreachable, invoke,
  resume, set_return_values, retarget_terminator, replace_terminator_args}`。
  编码约定见 `ops.toml` 的「终结符」节（与 `dfg.rs` 的解码一一对应）。
- **指令选择标签**（`Instruction.isel_strategy: Option<IselStrategy>`，S5）：
  模式匹配/融合阶段标注"这条指令是怎么被选出来的"。标签名**由目标 ISA 数据
  决定**——forge-ir 不解析、不认识任何具体名字（无枚举/白名单/长度限制），
  名字里的参数（如 `"lea_sib:4"`）也原样保留。字段**私有**，读写走
  `Instruction::{isel_strategy, set_isel_strategy, clear_isel_strategy}`；
  `IselStrategy` 是**值**（`ImmStr` 承载：短名内联零分配、长名 `Arc<str>` 共享、
  `Clone` O(1)），因此可跨 DFG 搬运（inline/lto/func_specialize 把被调方的标签
  搬到调用方）；空名 fail-closed。**注意**：手写 pattern-isel 生产者已随
  ISA-DSL v15 删除，标签与 `[[pattern]]` 命名体系的对接仍属 backlog（会改变
  指令序列，需专项验证）。
- **指令清单**：`ops.toml` → 生成的 `Opcode::ALL` 是全部变体的单一事实源（覆盖率
  矩阵、一致性守卫、名字查找都用它）；`Opcode::name()`/`from_name()` 用于 ISA TOML
  的 `op = "..."` 契约，`Opcode::info()` 给出类别/元数/结果数/UB/副作用/条件通道。

## 已知欠账

本 crate 的表示层缺口与分期修复计划见
[`docs/plans/forge-ir-v3-plan.md`](../../../docs/plans/forge-ir-v3-plan.md)：
指令元数据单一事实源（S1）、实体容器 `PrimaryMap/SecondaryMap`（S2）、类型上下文
去锁与所有权（S3）、终结符并入指令流与完整 use-def（S4）、附件强类型化与可见性收紧
（S5）、校验与 pass 契约（S6）、文本层诊断（S7）。

S0（2026-09-14）已落地的止血项与 S6 先行清偿见该文档 §6；**S1 已全部落地**
（`ops.toml` + 生成枚举/派生表/名字与 LLVM 文本名映射/逐指令类型规则族，查表全 O(1)）；
**S2 第一切片已落地**（`entity_map` 四个容器 + forge-ir 内部句柄键表迁移，
句柄键 `HashMap` 45 → 31 处）；**S4 主体已落地**（终结符并入指令流）；
**S5 第一切片已落地**（`isel_strategy` 类型化 + 字段私有化，2026-09-15）；
**S5 第二切片已落地**（开放集合划边界：删掉 `TargetTriple` 的架构名查表、
IR 公开面字符串统一 `ImmStr`，2026-09-15）；
**S5 第三切片已落地**（metadata 单写：5 个载体的附件各收成一个写入口 + 字段
私有化，2026-09-15）；
**S5 第四切片已落地**（`dfg` 私有化第一步：`values` arena 收口 + `set_value_type`
受限写入口，2026-09-15）；
**S5 第五切片已落地**（`dfg` 私有化第二步：`insts` arena 收口 +
`inst_mut`/`inst_mut_opt` 就地编辑口，2026-09-15）；
**S5 第六切片已落地**（`dfg` 私有化第三步收尾：`blocks` arena 收口 +
`block_mut`/`block_data_iter`，**三个 arena 全部私有**，2026-09-15）。
余项见该文档 §6 的 S2/S5 记录。
