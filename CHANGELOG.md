# CHANGELOG

<!-- markdownlint-configure-file { "MD013": { "line_length": 512, "code_block_line_length": 512, "heading_line_length": 512 }, "MD024": false } -->
<!-- 文件级豁免原因：历史条目按单行记录（最长 ~440 列），且多年条目同置 [Unreleased] 下，
     版本子节用 `### Added (日期)`，同层 Fixed/Changed 标题按惯例重复——均为 CHANGELOG
     结构与单行惯例，不按正文 120 列规则折行；重构留待 changelog 整顿时处理。 -->

All notable changes to the `code-forge` project will be documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.0.0/).

## [Unreleased]

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

- **终结符写入口按形式化 + 读取面投影化（forge-ir v3 S4 子项 d）**：`Function` 新增按形式命名的写入口 `jump`/`branch`/`ret`/`switch`/`unreachable`/`invoke`/`resume` 与 `set_return_values`/`retarget_terminator`/`replace_terminator_args`；`set_terminator(Terminator)` 与 `rewrite_terminator` 收为 `pub(crate)`，**crate 外已无法构造或就地改写 `Terminator`**（实测残留写点 = 0）。DFG 新增 `TermKind` 判别与投影访问器（`term_branch`/`term_jump`/`term_return_values`/`term_switch`/`term_invoke`/`term_resume_value`/`term_is_unreachable`/`term_args_to`/`term_used_values`/`for_each_term_value`）。
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
