# forge-ir v3 改进方案（已批准，分期落地中）

> 状态：方案已批准（2026-09-14），**S0 已落地**，S1–S8 待开工。
> 本文是实施记录 + 决策依据；IR 语义规范见（待建）`docs/reference/ir.md`。
> 现状证据为 2026-09-14 对仓库的只读实测（grep 计数 + 逐文件阅读），口径见 §2 末尾。

## 1. 诊断与目标

**一句话诊断**：forge-ir 目前是**由 LLVM 文本往返目标反向塑造出来的 IR**——它的形状
（phi 语法糖、`UndefNamed`/`AggConst`、`extra_attrs: Vec<ImmStr>`、`%v{n}` 命名、
display 合成 phi）服务"能把 LLVM `.ll` 读进来再打回去"；而"编译器内部表示"真正需要
的东西（**指令元数据单一事实源**、**完整 use-def**、**类型所有权**、**pass 契约**、
**可增长性**）靠手工同步维持，已经出现结构性腐化。

### 成功标准（可度量）

| 维度 | 现状（实测） | 目标 |
| --- | --- | --- |
| 新增一个 IR opcode 要改的地方 | ≥ 8 处手写表（枚举、`result_count`、`expected_operand_count`、`mnemonic`、LLVM 正/反映射、builder 方法、verifier 白名单、display 分支） | **1 处声明**（+ 可选 1 条 lowering 规则） |
| 表/清单漂移 | 已漂移：测试辅助 `all_opcodes()` 漏 10 个变体；`llvm_mapping.rs` 自述 105（实际 109）；分节计数错 | **机器守卫**（S0 已落一半） |
| use-def 完整性 | 终结符用值**不在** use-lists；`replace_all_uses` 覆盖不到分支/`ret` 参数；注释指向不存在的 `Terminator::map_values` | 用值 100% 覆盖；`replace_all_uses` 后 `Verifier` 零错误 |
| 句柄辅表 | 109 处 `HashMap<Value\|Block\|Inst\|FuncRef, _>` | `PrimaryMap/SecondaryMap/EntitySet` |
| 类型查询 | 每次过 `Arc<RwLock<TypeStore>>`；锁中毒 `expect` panic；`TypeId` 常量靠"预填充顺序 + debug_assert" | 无锁、无 panic、所有权显式 |
| verifier | 无 `TypeContext` 时 8 类检查静默跳过；`check_gep_indices` 对坏输入 panic | 无静默跳过（S0 已修）；公开 API 不 panic（S0 已修） |
| pass 后校验 | 只 `log::warn`（坏 IR 继续流动） | S0 已改为策略化（默认 warn + `PassVerify::Error` 严格开关）；S6 修完 pass 后切默认严格 |
| 门禁 | workspace 1367 passed / 0 failed；x86 矩阵 195/3/0；riscv64 QEMU 131/67/0；LLVM 语料 452（198/254/0） | 不得低于上述基线 |

## 2. 现状证据（关键项，带出处）

1. **指令元数据多处手写**：`Opcode` **109** 变体；派生属性散在 `result_count`（带
   `_ => 1` 兜底）、`may_ub`、`has_side_effect`、`mnemonic`、`expected_operand_count`；
   LLVM 文本名另有正/反两张有损表（`callbr→Call`、`trunc→Ireduce`、`Fload` 复用 `load`）；
   verifier 另有 ~371 行 per-opcode 类型规则。全仓 `Opcode::` 引用 **743 处 / 30+ 文件**。
2. **指令是"opcode + 3 张并行表"**：`Instruction { opcode, results, operands, immediates,
   flags, mem_flags, param_attrs, fn_attrs, metadata, loc, isel_strategy }`——无属性系统；
   开放集合一律字符串；`Icmp{cond}`/`Fcmp{cond}` 又是带 payload 的变体（两种表示并存）。
3. **use-def 不完整且可被绕过**：`Use.user` 只能是 `Inst`；`Function.dfg` 与
   `DataFlowGraph.{values,insts,blocks}` 全 `pub`（下游可绕过 use-lists 直改操作数）。
4. **双份顺序 + 死字段 + 只增不减的墓碑**：`Instruction.block/pos` 与
   `BlockData.inst_order` 各存一份顺序（`pos` 只写不读，S0 已删）；删除是
   "`opcode=Nop` + 清表 + 结果值 VOID 化"，槽位永不复用。
5. **类型系统被锁与硬编码索引夹住**：`TypeContext(Arc<RwLock<TypeStore>>)`；`TypeId`
   常量与 `TypeStore` 预填充顺序两处约定（索引 9 是空洞）；`bits()` 硬编码 `PTR=64`、
   复合类型返回 0；`Module::set_data_layout` 整体替换 `self.types`。
6. **常量池缺陷（S0 已修）**：float 池无位宽 → f32/f64 位模式相同即合并，phi 打印按
   f64 还原产生错值；`Default` 绕过 bool 预置槽；`total_len/is_empty` 漏聚合池。
7. **verifier（S0 部分已修）**：63 条规则 / 13 阶段；无 ctx 时 8 类静默跳过；
   `check_gep_indices` 可 panic；缺 entry 参数 vs 签名、`Call` 实参 vs 被调签名、
   result 类型 == 操作数类型、向量算术同型等检查；错误无 severity。
8. **pass 框架（S0 部分已修）**：`UntilFixedPoint` 无上限；`PassResult` 计数丢失；
   分析统一整体失效；pass 后校验只 warn。
9. **形状约束**：`Block(0xFFFFFFFD)` 当 epilogue 外部标签哨兵；"entry = `Block(0)`" 约定
   硬编码 5 处；`pipeline/lowering.rs` 用"枚举序 == arena 下标"反推句柄；`.0 as usize` 260 处。
10. **下游耦合面**：直接依赖仅 5 个 crate；`forge-dsl` 生成代码含 **33 处字面
    `forge_ir::` 路径**；`forge-hir-macro` 有 **34 条硬编码 `Opcode::X` 映射**；ISA TOML 的
    `op = "Iadd"`/`match = "Fadd(...)"` 是 opcode 名契约（x86 100 个不同 op）；
    `forge-tests/src/coverage.rs` 的 60 臂 `match op` 有 `_` 兜底（新 opcode 静默走 fallback）。
    反面：`forge-object` 只依赖 `ImmStr`+`IrError`，`forge-plugin`/`forge-grammar`/`forge-mem` 零引用。
11. **文本层**：AST 无 span、错误只有字节偏移且不转行列、无错误恢复；`display_llvm.rs`
    89 个测试是"结构等价往返"（不覆盖 globals/metadata/alias）；crate 外仅 5 处使用解析器。

> 口径：行数 `[IO.File]::ReadAllLines().Length`；计数 `Select-String -Pattern <re> | .Count`
> （域 `crates/ examples/ src/ benches/ tests/`，排除 `target/`）。

## 3. 设计原则

1. **单一事实源**：能声明就别手写；声明式数据 → 生成代码/表，机器守卫防漂移。
2. **fail-closed**：不允许静默跳过、静默兜底、注释里的隐式契约、公开 API panic。
3. **不设白名单/上限**：种类/宽度/能力一律走数据；确需边界时点名它是"**值表示**上限"。
4. **不变量必须可机器检查**：verifier（运行期）+ 穷举 match/一致性测试（编译期/CI）。
5. **破坏性变更分期落地**：每期单独提交、CI 全绿、门禁数字不回退、可 `git revert`。

## 4. 改进项（子系统）

- **A 指令元数据单一事实源（S1）**：`ops.toml` + `build.rs` 生成 `Opcode`/`OpcodeInfo`/
  `ALL`/名字映射/属性/builder 断言/覆盖矩阵行；`Icmp/Fcmp` payload 归一。
- **B 终结符并入指令流（S4，正确性核心）**：`Br/BrIf/Switch/Return/Unreachable/Invoke/Resume`
  成为 opcode，块实参是操作数；删 `Terminator`/`has_terminator`；`Use` 带种类；
  `LabelRef` 取代 `Block(0xFFFFFFFD)`；`Function::entry()` 取代 `Block(0)` 约定。
- **C 实体容器与密集索引（S2）**：`PrimaryMap/SecondaryMap/EntitySet/ListPool/PackedOption`；
  替换 109 处按句柄的 `HashMap`；句柄字段私有化 + 访问器；墓碑语义显式化。
- **D 类型系统（S3）**：去 `RwLock`、`TypeStore` 显式传参、`bits()` 拆 `scalar_bits()`/
  `size_bytes(&DataLayout)`、`TypeId` 常量构造期建立、签名去重。
- **E 附件强类型化与可见性（S5）**：`isel_strategy` 类型化；开放集合划边界；
  metadata 单写；`dfg` 私有化 + 受限编辑 API。
- **F 常量池修正（S0 已落）**。
- **G 校验与 pass 契约（S6）**：verifier 补 4 类检查 + severity + 带实体位置的诊断 +
  "重算 CFG/支配树并比对"；pass 后校验默认切 `PassVerify::Error`；`AnalysisManager`。
- **H 文本层（S7）**：`features=["text"]` 门控（**不拆文件**）；span 贯穿；错误恢复；
  往返断言扩面；结构化 fuzz。LLVM 语料继续作为互操作硬门禁。
- **I 可选项（S8，默认不做）**：二进制序列化、MemorySSA-lite、crate 边界拆分。

## 5. 分期与进度

| 期 | 范围 | 状态 |
| --- | --- | --- |
| **S0** | 守卫与止血（12 项） | **已落地**（见 §6） |
| S1 | 指令元数据单一事实源 | **已落地**：`ops.toml` + `build.rs` 生成枚举/派生表/名字与 LLVM 文本名映射/逐指令类型规则族 + 全部查表 O(1)（§6 第一~四步） |
| S2 | 实体容器与密集索引 | **forge-ir 部分收口**：四个容器已实现；内部主表 + `predecessors()`/`successors()`（含下游调用点）+ 支配树字段已迁移，句柄键 `HashMap` 45 → 10 处；余项：句柄字段私有化、墓碑语义、`ListPool`、两个下游 crate 内部句柄表 |
| S3 | 类型系统去锁/所有权 | **部分落地**：`TypeId::bits()`/`try_bits()` 已删除（76 处调用点迁移，指针宽度改按 DataLayout）、锁中毒不再 panic、类型事实 API（`scalar_bits`/`builtin_*`）+ 守卫测试；余项：去 `RwLock` 与 `TypeStore` 显式传参 |
| S4 | 终结符归一 + 完整 use-def | **已落地（主体完成）**：终结符并入指令流——7 个终结符 opcode、块实参即操作数、`Terminator` 枚举与 `UseSite` 双双删除；前置八项（entry fail-closed / LabelRef / use-def 补全 / 字段私有化 / 显式未终止 / 写入口 / 投影访问器 / 读取面迁移）见 §6 |
| S5 | 附件强类型化与可见性 | **前三切片已落地**：`isel_strategy` 类型化（`IselStrategy`，无宿主标签清单）+ 字段私有化；开放集合划边界（删 `TargetTriple` 的架构名查表、IR 公开面字符串统一 `ImmStr`）；metadata 单写（5 个载体各收成一个写入口）（均见 §6 末）；余项：`dfg` 私有化 + 受限编辑 API |
| S6 | 校验与 pass 契约 | **部分落地**：S0 暴露的 pass 欠账已全部清偿，校验默认策略切到 `Error`（见 §6 末）；终结符诊断已点名真实指令句柄（见 §6 末）；校验器/契约的进一步强化待续 |
| S7 | 文本层诊断与往返 | 待开工 |
| S8 | 可选（二进制/MemorySSA/crate 边界） | 需拍板 |

### 每期固定门禁

`cargo fmt --all` → `cargo clippy --workspace --exclude forge-rustc --all-targets
--all-features -- -D warnings` → `cargo test --workspace --exclude forge-rustc
--exclude cargo-forge -j 2 -- --test-threads=1` → 两条 jit 矩阵（x86 195/3/0、
riscv64 QEMU 131/67/0）→ LLVM 语料 452（198/254/0）→ 文档（改 `.md` 必跑
markdownlint 到 0 error）→ push 后看 CI run 全绿。

## 6. S0 落地记录（2026-09-14）

**已完成（含实证）**：

1. `Opcode::ALL`（109 项）+ `name()`/`from_name()`/`from_mnemonic()`；`name()` 是
   **无 `_` 兜底臂的穷举 match**（新增变体不同步即编译失败）。
2. 新守卫测试 `forge-ir/tests/opcode_table.rs`（4 个）：`ALL` 与枚举等势、变体名唯一
   可逆、助记符唯一可逆、条件变体身份。
3. 删掉 `opcode.rs` 测试辅助里手写的第二份清单（漏 10 个变体）→ 改用 `Opcode::ALL`。
4. 常量池：float 位宽进去重键（f32/f64 不再合并）、`insert_float_typed`、
   `get_float_width`、`remap_from` 带位宽、`Default` == `new()`（bool 槽不再悬空）、
   `total_len/is_empty` 含聚合池、**phi 打印按值宽还原**（修 f32 错值）。
   新测试 `forge-ir/tests/constant_pool.rs`（7 个）。
5. `MetadataStore::insert_at` 与 `intern` 的去重表自洽；`get` 返回 `Option`（不再 panic）；
   解析器侧的悬空引用改成点名语义错误。
6. `Verifier` 无 `TypeContext` → `VerifyError::MissingTypeContext`（不再静默跳过 8 类检查）；
   `check_gep_indices` 越界报错而非 panic。
7. 删死字段 `Instruction.pos`；修正三处与代码不符的注释（`lib.rs`/`entity.rs` 的
   "已有 PrimaryMap/SecondaryMap"、`function.rs` 指向不存在的 `Terminator::map_values`）。
8. forge-opt：`PassResult` 计数正确累加；`UntilFixedPoint` 加 `MAX_FIXED_POINT_ROUNDS`
   上限（超限报错）；pass 后校验策略化为 `PassVerify{Off,Warn,Error}`（默认 `Warn`，
   `Error` 是 S6 门禁）；**修掉一处非法 IR 夹具**（`o2_pipeline_nop_residue` 的 `build_loop`
   建了 0 参数块却传 2 个实参）。
9. `forge-tests/src/coverage.rs`：34 个"矩阵未覆盖"的 opcode 逐条登记原因
   （`UNCOVERED_OPS`），新增完整性守卫断言 `COVERAGE_OPS ∪ UNCOVERED_OPS == Opcode::ALL`
   与名字 rot 检测（此前 `check_isa` 的 `_` 兜底让缺口完全不可见）。
10. 新增 crate 文档 `crates/foundation/forge-ir/README.md`；修正 `docs/forge-ir/backlog.md`
    的 #1 错误结论（`agg_expand` rewrite 族并未迁移）。

**S0 实测发现的新欠账（转 S6）**：打开"pass 后严格校验"后暴露：`inline`
（内联体操作数未登记 use-lists + 返回类型不匹配）、`gvn_pre`/`mem2reg`
（插入指令的操作数未登记 use-lists）会留下不一致 IR。由于一个 pass 弄脏后后续每个 pass
都会报同一处不一致，"按 pass 白名单放行"不可行 → S0 保留 `Warn` 默认 + 严格开关，
并用 `strict_verification_reports_known_debt` 钉住（该测试在 S6 修好后会失败，提示切换默认）。

**基线数字（S0 后）**：workspace **1367 passed / 0 failed / 18 ignored**（65 suites）。

### S6 先行清偿：pass 欠账清零（2026-09-14）

按"先把 S6 的两笔 pass 欠账清掉，再继续 S1"的要求，先清偿 S0 暴露的欠账，
**不清偿就不许把默认策略切到 `Error`**（严格校验一旦默认开启，任何脏 pass 都会
直接打断全部管线用例）：

1. **契约先立**：`Function::make_inst` / `make_inst_with_meta_and_loc` 包装
   `dfg.make_inst*` 并登记 use-lists；`Function::refresh_inst_uses` 供就地改写操作数后
   重登记。根因是 `UseLists::remove_inst` **按指令当前操作数**逐条删——在改写**之后**
   调用就删不掉旧记录，于是新增 `UseLists::forget_inst`（按 `user == inst` 清扫，
   与操作数内容无关）。现全仓 pass/前端建指令一律走 `Function::make_inst*`；
   回归守卫 `forge-ir/tests/use_lists.rs`（3 个用例，含"改完刷新后 use-lists 与 DFG 一致"）。
2. **三处欠账逐一修掉**（不是放行）：`inline`（内联体操作数未登记 + 调用结果
   **终结符操作数**没被 RAUW，曾让 `ret` 返回 `VOID` 值 ⇒ 改用 `Function::apply_replacements`）、
   `gvn_pre`（新建指令未登记；且 PRE 会在**不被操作数定义支配**的前驱插入 ⇒ 新增
   `operands_dominate` 守卫）、`mem2reg`/`pgo`/`lto`/`func_specialize`（同批收敛）。
   `ipa/tail_call.rs` 顺带收窄为**仅自递归**（跨函数尾调用改写会产生
   `BlockParamCountMismatch` 的非法 IR）并改为原子 `kill_inst` + `set_terminator`。
3. **codegen 侧补上同一道门**：`pipeline/compiler.rs` 的 47 处 `.dfg.make_inst*` 改走
   包装，13 处墓碑/Copy 块与 3 处操作数改写点改为 `refresh_inst_uses`，聚合展开后
   加 **debug-only** 断言 `use_lists.verify(&dfg)`。此处**刻意只查 use-lists、不跑完整
   `Verifier`**：`expand_geps` 会生成类型自洽性不足的 IR（`%p = add i64 %prev, %t`
   却声明 PTR 结果，源码原注释即"verify 不跑"），根因是 v12 尚无
   `Ptrtoint`/`Inttoptr` 降级 ⇒ **类型化指针算术归 S4/S5**，此处不静默容忍而是写明范围。
4. **默认策略切换**：`PassVerify::Error` 成为 `#[default]`（debug 构建；`Off`/`Warn`
   仍可经 `PassManager::set_verify_after_pass` 显式选择，release 不跑校验）。
   S0 的 `strict_verification_reports_known_debt` 按预期完成使命后撤销，
   换成 `strict_verification_passes_for_all_pipelines`（O1/O2/O3 × 多形态函数全绿）。

**基线数字（S6 欠账清偿后）**：workspace **1369 passed / 0 failed / 18 ignored**（65 suites，
`cargo test --workspace --exclude forge-rustc --exclude cargo-forge -j 2 -- --test-threads=1`）。

### S1（第一步）：`ops.toml` 成为指令元数据单一事实源（2026-09-14）

**做了什么**：crate 根新增 `ops.toml`（109 条 `[[op]]`：变体名、助记符、分组、
文档、值操作数个数、结果数、`may_ub`、`side_effect`、变体载荷），`build.rs` 扩容为
"lalrpop + 读 `ops.toml` 生成 `$OUT_DIR/opcode_gen.rs`"，`src/opcode.rs` 用 `include!`
接入。被替换掉的**六张手写表**：枚举本身、`ALL`、`name()`、`mnemonic()`、
`result_count()`、`expected_operand_count()`（外加 `may_ub()`/`has_side_effect()`），
一次性全部改由生成物投影：

- `Opcode::info() -> &'static OpcodeInfo` 是**无 `_` 兜底臂**的生成 match（新增变体
  不同步更新即编译失败）；`name/mnemonic/category/doc/arity/result_count/may_ub/
  side_effect` 都是它的字段。
- 操作数元数建模为 `OperandArity::{Fixed(u8), Variadic}`：修掉了老表"`0` 既表示
  '无操作数' 又表示 '不检查'"的语义混淆（`expected_operand_count()` 保持历史契约
  `Variadic => 0`，要区分请读 `info().arity`）。
- 生成器 **fail-closed**：缺字段/类型不对/名字或助记符重复/未知载荷/载荷缺默认值
  一律 `panic!` 中断构建（构建期就拦住，而不是运行期查表取第一个）。
- `ops.toml` 的 `payload` 当时把 `Icmp{cond}`/`Fcmp{cond}` 记为**变体载荷**——
  已于同日第二步归一（见下）。

**迁移保真证据**（不靠"看起来一样"）：用一次性脚本把 **git HEAD 的手写表**与
**生成物**各自独立解析（老表按 Rust 源码分组解析、新表按生成行正则解析）后逐项比对：
`OLD_VARIANTS=109 / OLD_MNEMONICS=109 / NEW_INFOS=109`，`NEW_VARIADIC=6 /
NEW_SIDE_EFFECT=9 / NEW_MAY_UB=15`，**MISMATCHES=0（109 变体 × 6 属性）**。

**当时记录的 S1 余项**（verifier 逐指令类型规则与 builder 断言的声明化）已在
第三步与第四步处理，见下。

### S1（第二步）：比较条件归一（2026-09-14）

`Icmp { cond }`/`Fcmp { cond }` 的**变体载荷**删除，条件改走 instruction 的
immediate 通道：`Opcode::Icmp`/`Opcode::Fcmp` 成为纯身份变体（`Opcode` 至此
109 个变体全部无载荷），条件由 `Immediate::IntCC`/`FloatCC` 承载，
`ops.toml` 用 `cond = "IntCC"|"FloatCC"` 声明这一契约（生成物给出
`Opcode::cond_kind()`）。

- **数值表示唯一化**：`IntCC::code()`（1..=10）/`FloatCC::code()`（1..=16）+
  `from_code()` 成为条件的唯一数字映射。此前 forge-dsl 为**每个 ISA 模块**生成
  一张 `icmp_id`/`fcmp_id` 表，宿主 lowering 另有隐式约定；现在宿主
  `current_immediates` 直接填 `code()`，ISA TOML 的 `cond` 谓词读同一个数字，
  生成代码里那两张表被删除（x86 的 setcc 编码映射保留 ISA 侧，输入改为
  canonical code → x86 cc）。
- **fail-closed**：`Verifier` 新增 `MissingCondImmediate` / `WrongCondImmediate`
  （条件缺失或类型不对直接报错，不按"默认条件"继续）；display 对坏 IR 打印
  `icmp <cond?>` 而非静默省略条件；forge-codegen 的 immediate 折叠表删掉
  `_ => 0` 兜底臂，改为显式列出全部 `Immediate` 变体（新增变体必须重新表态）。
- **顺手修掉的两个真实缺陷**（都是本次归一暴露的）：
  ① `ExprKey`（CSE/GVN/GVN-PRE 的表达式的键）只含 `opcode + operands + ty`——
  条件在 opcode 载荷里时恰好"够用"，归一后 `icmp eq` 与 `icmp ne` 键相同会互相
  消除（**错值级**）；现在 immediate 进键，`p0_icmp_cond_distinct` 回归测试
  正是这样抓到的。
  ② `algebraic.rs` 的 `x cmp x → 1/0` 规则读的是 `Immediate::Int(cc)`，而当时
  条件在变体载荷上、immediates 恒空 ⇒ 该分支**从未命中**（死代码）；归一后真正生效。

**基线数字（S1 第二步后）**：见本节末门禁表（workspace 1374 passed / 0 failed /
18 ignored，66 suites；x86 矩阵 195/3/0；riscv64 131/67/0）。

### S1（第三步）：LLVM 文本名进 `ops.toml` + 查表一律 O(1)（2026-09-14）

**LLVM 文本名表进 `ops.toml`**：`ir_parser/llvm_mapping.rs` 里的**正/反两张手写表**
（105+ 条 `match`）删除，改为每个 opcode 声明
`llvm = "<文本名>"` + `llvm_parse = false`（仅 display 用）+ `llvm_alias = [...]`（旧名）：

- 有损对从"读者自己比对两张表"变成**声明**：`Fload`/`Fstore` 复用 `load`/`store`、
  `Iconst`/`Fconst`/`Vconst` 常量内联、`Vadd`/`Vsub`/`Vmul`/`Vneg`/`Vabs`/`Vbitcast`
  由标量名 + 向量类型精化、`Vsplit`/`Vconcat`/`Ftrunc` 解析器不接受、`CallIndirect`
  复用 `call`、`Icmp`/`Fcmp` 需要条件——全部 `llvm_parse = false`（17 条）。
- 别名 3 条：`callbr → Call`、`ptrtoaddr → Ptrtoint`、`vextractelement → Vextract`。
- 生成期断言：变体名/助记符/**解析名集合**三者唯一（解析名冲突会让
  `from_llvm_name` 变成"查第一个"的隐式优先级）。
- **迁移保真证据**：一次性脚本独立解析 git HEAD 的两张表与生成物后逐项比对
  → `PARSE_NAMES=95 / OLD_PARSE_PAIRS=95 / OLD_SHOW_PAIRS=108`，**MISMATCHES=0**。

**查表一律 O(1)**（`Opcode::from_name`/`from_mnemonic`/`from_llvm_name`/
`from_cond_llvm_name`/`info()` 都是**生成的 `match`**，不是 `ALL.iter().find`）：

| 查找 | 线性扫 `ALL` | 生成 `match` | 提升 |
| --- | --- | --- | --- |
| `from_llvm_name` | 2770.5 ns | 203.5 ns | 13.6× |
| `from_mnemonic` | 2532.2 ns | 300.8 ns | 8.4× |
| `from_name` | 2806.0 ns | 551.2 ns | 5.1× |

（本机 debug 构建，200k 轮 × 28 个名字 = 560 万次；计时工具留在
`tests/llvm_name_lookup_perf.rs`，默认 `#[ignore]`，跑法见文件头。）

**生成器自身的构建期查表也改了**：唯一性检查从 O(n²) 双重循环 + `seen.iter().find`
改为 `HashMap` O(1) 探测（冲突信息直接点名两条），分节条数从 O(n·c) 改为预聚合
O(n)。**证据：生成物 `opcode_gen.rs` 前后 SHA256 完全一致**
（`5A53A0F5B566C4EEA0CD6CAADBDE9ECA2E54E408E0D35C0C712D2FC0130B062A`）。
另外把 `semantics.rs` 的 DWARF 表达式操作码表（`OPS.iter().find`）与
`forge-tests/coverage.rs` 的清单腐烂检查（`Opcode::ALL.iter().any`）也换成
`match`/生成的 `from_name`。

### S1（第四步）：verifier 类型规则声明化（2026-09-14）

`verify.rs` 的 `check_operand_types` 原本按 opcode 手写分组（`matches!` 大名单：
binop 37 个、浮点 binop 8 个、Fma、Select、Icmp/Fcmp，再加上 13 个转换指令的
逐 opcode 分支）。现在**族名声明在 `ops.toml`**，verifier 按族分派：

- `type_rule` 12 个族的**封闭词汇表**：`none`(48) / `binop_same`(37) /
  `convert`(13) / `load`(2) / `store`(2) / `same3` / `cmp_int` / `cmp_float` /
  `select` / `cmpxchg_pair` / `call` / `call_indirect`。写成未实现的族名 →
  **构建期报错**；新增族名 → `verify.rs` 的穷举 match（无 `_` 臂）**编译失败**
  ——双向 fail-closed。
- 转换指令的 13 条分支收敛成一张**事实表**：`convert = { src, dst, width }`
  （`int/float/ptr/any` × `widen/narrow/any/equal_bytes/equal_total_bits`），
  规则本体只剩一份（含诊断文本由 `conversion_expectation` 从事实表拼）。
- 形状类规则（`binop_same`/`same3`/`cmp_*`/`select`）抽到新模块
  `src/type_rules.rs` 的 `check_shape`：**verifier 与 builder 共用同一实现**
  （builder 侧保留为 debug 工具函数，未挂在发射路径上——见下），并有自己的 4 个单测。
- `none` 的两种含义在 `ops.toml` 注释里点名（确实无规则 / 检查在 immediate 阶段），
  由守卫测试 `type_rule_classification_is_complete` 断言**每个 opcode 都有分类**
  且各族计数钉住（新增 opcode 默认掉进 `none` 会被拦下）。

**门禁（S1 收尾后）**：workspace **1380 passed / 0 failed / 19 ignored**（67 suites）；
x86 矩阵 195/3/0；riscv64 131/67/0；fmt / clippy `-D warnings` 干净。

**builder 侧断言不做声明化（实证否决）**：把同一个 `check_shape` 挂到`FunctionBuilder::emit_with_mem` 的 debug 断言后，**5 个既有 builder 测试失败**
（`test_int_binary_upcast_result_ty`、`test_float_binary_upcast_result_ty`、
`test_bool_arith_normalized`、`test_bool_special_handling`、
`test_shift_result_ty_keeps_lhs`）——builder **刻意允许**"混合宽度操作数 + 结果类型
upcast"（`iadd(i8, i64) → i64`），而 verifier 的 `BinopSame` 要求两个操作数同类型。
二者职责不同：builder 是宽松构造层（类别维度由各方法的 `assert!(t.is_int())` 把关，
**比 verifier 更严**——verifier 对 binop 并不查类别），verifier 是严格校验层。
强行统一会破坏既有语义，故保留 `debug_check_shape` 为可选工具并写明原因。

### S2（第一切片）：实体容器 + forge-ir 内部主表迁移（2026-09-14）

**容器本体**（新模块 `src/entity_map.rs`，无新依赖，8 个单测）：

- `EntityRef` trait（句柄 ↔ 密集下标，`as_u32`/`from_u32`）+ 宏
  `entity_ref_impls!`，已为 `Value`/`Inst`/`Block`/`TypeId`/`FuncRef`/`ConstId`/
  `GlobalId`/`SigRef`/`AggId`/`VReg` 实现。
- `PrimaryMap<K, V>`：主存，`push` 分配句柄、下标即句柄、**只增不删**（删除会让
  句柄失效，而句柄遍布 IR——这一"刻意不支持删除"与 Cranelift 的取舍一致）。
- `SecondaryMap<K, V>`：辅存，`Vec<Option<V>>`，"未设置"与"空值"可区分；
  `get_mut_or_default`/`get_mut_or_insert_with` 等价于 `HashMap` 的 `entry().or_*`。
- `EntitySet<K>`：密集位图（`O(1)` 插入/查询/删除）。
- `PackedOption<K>`：句柄的可空压缩，**4 字节**（`Option<Value>` 是 8 字节——
  `Value` 没有 niche），`u32::MAX` 为空哨兵。
- **`ListPool` 不做**（本仓库的列表用途都是短生命周期局部量，引入只增加一层间接）。

**已迁移的句柄键表**（`HashMap` → `SecondaryMap`，逐个是"句柄即下标"的天然密集表）：

| 位置 | 表 | 说明 |
| --- | --- | --- |
| `use_list.rs` | `uses: Value → SmallVec<[Use;4]>` | 最热路径：每建/删/改指令都碰 |
| `verify.rs` | `defined: Value → TypeId` | 每次 `verify()` 重建、每操作数查询 |
| `function.rs` | `value_names`/`block_names` | 显示名绑定 |
| `display.rs` | `NameResolver::{values, blocks}` | `docs/reference/imm_str.md` 点名的热路径 |
| `alias.rs` | `memo: Value → MemoryLocation` | 惰性别名查询缓存 |
| `debug_info.rs` | `locations: Value → SourceLocation` | 调试位置 |
| `loop_info.rs` | `depths: Block → u32` | 循环深度 |

**计量证据**：`crates/foundation/forge-ir/src` 里"句柄键 `HashMap`"从 **45 处降到
31 处**（`git grep` 对比 HEAD；全仓基线 129 处 / 36 文件）。容器与迁移共 **8 个
容器单测**，workspace 1380 → 1388 passed。

**S2 余项（未做，明确记录）**：① 句柄字段私有化 + 访问器（`Value(pub u32)` →
`index()`，全仓 `.0` 约 260 处，需按 crate 分期）；② 墓碑语义显式化（`Layout` 的
删除/复用策略）；③ `ListPool`（按需）；④ `forge-opt`/`forge-codegen` 内部的句柄键表
（regalloc 的 `XReg→PReg`、`Block→VBlockId` 等）。

### S2（第二切片）：`predecessors()`/`successors()` 迁到密集索引（2026-09-14）

`Function::predecessors()`/`successors()` 是**跨 crate 公开 API**（`forge-opt` 的
循环/预处理 pass、`forge-codegen` 的 lowering 共 10+ 调用点）。两处
`OnceLock<HashMap<Block, Vec<Block>>>` 改为
`OnceLock<SecondaryMap<Block, Vec<Block>>>`，`preds.entry(succ).or_default()` 换成
`get_mut_or_default`；调用方由 `.get(&block)` 改为 `.get(block)`（11 处，横跨
`display.rs`/`function.rs`/`semantics.rs`/`verify.rs`/`loop_info.rs` 与
`forge-opt` 的 6 个 pass 文件、`forge-codegen` 的 lowering）。

语义差异（写进 doc）：`predecessors()` 里**无前驱的块不出现**（entry），调用方用
`get(b).cloned().unwrap_or_default()`；`successors()` 则**每个块都有条目**（无后继者
为空 `vec![]`）——与迁移前逐块 `insert` 的行为一致。

**计量**（同一 `git grep` 口径，`forge-ir/src` 全树）：第一切片后 32 处 → 本切片后 **25 处**（本轮还改掉
`loop_info.rs` 的 `collect_loop_body` 形参类型）。workspace 1388 passed 不变
（本轮无新增测试，改动是等价替换；全仓编译 + 全部测试 + 两条矩阵是证据）。

### S2（第三切片）：支配树字段密集化（2026-09-14）

`analysis.rs` 的 `DominatorTree` 五个字段（`children`/`tin`/`tout`/`idom`/`depth`）
由 `HashMap<Block, _>` 改为 `SecondaryMap`（含 `empty()`、C-H-K 迭代的 `idom` 局部表、
`compute_children`/`compute_intervals` 的签名与返回类型）。支配树是
`dominates`/`idom`/`depth`/`ncd`/`children` 的底座，`loop_info`/`licm`/`gvn` 都在用；
查询从"哈希 + 探测"变为一次 `Vec` 索引（`dominates` 一次查 4 张表）。

顺带修掉一处 `SecondaryMap` 的 API 缺口用法：`for (child, &parent) in idom` 需要
`IntoIterator for &SecondaryMap`（本容器暂未提供），改为 `idom.iter()`
（产出 `(K, &V)`）。`IntoIterator for &SecondaryMap/PrimaryMap/EntitySet` 作为
待补的易用性缺口记录在此。

**计量**（同一 `git grep` 口径，`forge-ir/src` 全树）：句柄键 `HashMap`
S2 前 45 处 → 第二切片后 25 处 → **本切片后 10 处**（`SecondaryMap` 使用点 74 处）。
workspace 1388 passed 不变（等价替换；全仓编译 + 全部测试 + 两条矩阵为证据）。

### S3（第二切片）：删除 `TypeId::bits()`/`try_bits()`（2026-09-14）

按"无需兼容旧版本结构"的直接要求，**删除**了会撒谎的位宽视图并迁移全部调用点
（不做兼容层）：

- **`TypeId::bits()` / `try_bits()` 删除**。它有两条撒谎的默认值：`PTR` 恒 64
  （不查 `DataLayout`）、复合类型返回 0（与 void 不可区分）。替代物：
  `TypeStore::scalar_bits`（含指针，按 DataLayout）与
  `TypeId::builtin_scalar_bits`（内建标量）+ 新增
  `TypeId::builtin_vector_bits`（`V64/V128/V256` 的静态总位宽——类型名即事实）
  与 `TypeContext::scalar_bits` 转发方法。
- **迁移 76 处调用点**（`.bits()`/`.try_bits()`；原先统计的 80 处里有 4 处在
  `forge-rustc`，逐条核对后确认那是 **rustc 自己的 API**
  （`data_layout.pointer_size().bits()`），与本次无关）：
  `forge-ir`（verify 转换宽度规则改问 store、builder 的 `iconst` 常量位宽问
  store、entity 测试改测 `builtin_scalar_bits`）、`forge-opt`
  （`const_fold` 19 处 + `algebraic` 1 处）、`forge-dsl`
  （5 处 **生成代码**：新增 `LowerCtx::type_bits_of`，生成的 lowering 改调它）、
  `forge-codegen`（`opsize_from_type`/`mem_opsize_from_type` 改为实例方法并问
  store、`reg_info` 的 VEC 档位用内建向量位宽、`pattern.rs`/`compiler.rs` 同理）。
- **两处行为修正**（这正是删掉视图的目的）：
  ① `opsize_from_type`/`mem_opsize_from_type` 现在按 `DataLayout` 算指针宽度
  ——32 位目标的指针 opsize 从 64 修正为 32（x86/riscv64/arm64 默认仍是 64，
  故两条矩阵不变）；
  ② `const_fold` 对**动态位宽**标量（`i24` 等内部化类型）改为**不折叠**
  （fail-closed 守卫 + `debug_assert`）：本模块无 store，宁可不优化也不按错误
  位宽算值。要折叠动态位宽需把 `TypeStore` 传进该模块（S3 后续项）。
- 验证：残留 `bits(`/`try_bits` 调用点 0（`forge-rustc` 那两处是 rustc API）；
  fmt/clippy `-D warnings` 干净；workspace **1383 passed / 0 failed / 19 ignored**
  （68 suites）；x86 矩阵 195/3/0；riscv64 131/67/0。

**S3 余项**：① `TypeContext(Arc<RwLock<TypeStore>>)` 去锁 + `TypeStore` 显式传参
（本轮只去掉锁中毒 panic；所有权显式化会牵动数百处 `ctx.borrow()`）；
② `TypeId` 常量的"预填充顺序 + 索引 9 空洞"契约目前由 `debug_assert` + 本轮新增
的 `tests/type_facts.rs` 守卫（release 下靠测试而非断言）；
③ `Module::set_data_layout` 整体替换 `self.types` 的语义待收敛。

### S4 前置清理：入口约定 fail-closed + 尾声哨兵具名（2026-09-14）

S4（终结符并入指令流）的两处"形状约定"先清掉，避免它们跟着大改一起漂：

1. **`Function::entry()`（fail-closed）**：此前 7 处 pass/分析写
   `func.entry_block.unwrap_or(Block(0))`——静默回退会把"根本没设入口"伪装成
   "入口是 0 号块"，支配树/循环分析/CFG 遍历据此算出**看似合理但错误**的结果。
   现在统一走 `Function::entry()`：缺失即 panic（编程错误），要"可能没有入口"
   的语义就直接读 `entry_block` 字段。涉及 `analysis.rs`、`forge-opt` 的
   `dead_code`/`gvn`/`gvn_pre`×2/`jump_thread`/`sccp`，以及
   `forge-codegen/pipeline/compiler.rs` 的 entry 参数重建（原来注着
   "entry 块约定为索引 0"）。`DominatorTree::empty()` 的 `entry: Block(0)`
   保留但注明是"空函数退化值、没有任何查询会用到"，不是入口约定。
   `make_placeholder_ptr` 的 `Block(0)` 同样注明是 `ValueDef::Param` 的语法占位。

2. **`EPILOGUE_LABEL`（具名哨兵）**：统一尾声不是 IR 块，但机器层 label 复用
   `Block` 句柄，此前用字面量 `Block(0xFFFFFFFD)`（emission.rs 两处 + 注释里
   提到这个魔数）。现在 `crate::pipeline::emit::EPILOGUE_LABEL` 具名 + 文档
   （为什么取 u32 空间最高的 3 个值、为什么不能改——定宽 ISA 把块号写进
   label 位域，reloc patcher 依赖它只占低位），并在绑定前加 debug 断言
   （块数不得逼近哨兵值）。

**验证**：workspace 1383 passed / 0 failed / 19 ignored（68 suites）；
x86 矩阵 195/3/0；riscv64 131/67/0；fmt/clippy `-D warnings` 干净。

### S4（子项）：`LabelRef` 取代哨兵 `Block`（2026-09-15）

机器层 label 复用 IR 的 `Block` 句柄，但"统一尾声"不是 IR 块——历史实现用魔数
`Block(0xFFFFFFFD)` 表示（上一轮已具名为 `EPILOGUE_LABEL`，本轮再进一步）：

- 新类型 `pipeline::emit::LabelRef { Block(Block), External(ExternalLabel) }`
  以及 `ExternalLabel::{BASE, id()}`：把"真实块"与"机器层自造标签"写进类型，
  外部标签 id 独占 u32 空间最高 3 个值。
- **唯一的数字 ↔ 标签互转边界**：`LabelRef::id()` / `LabelRef::from_id()`
  ——定宽 ISA 要把 id 塞进 label 位域、变长走 reloc，编码器/patcher 仍按数字
  工作，但机器层 API 不再暴露"可能是假块的 Block"。
- `CodeSink::{bind_label, use_label_at}` 收 `impl Into<LabelRef>`（块标签零改动
  调用）；`TargetFrameLowering::emit_epilogue_jump` 形参改为 `LabelRef`；
  DSL 生成器同步（`epilogue_block.id() as i64`、生成的 machine.rs 用
  `LabelRef::from_id(rel as u32)` 从编码 id 还原）。
- 编码 id 值不变（尾声仍 0xFFFF_FFFD），故 `reloc_patcher` 的位段重排语义与
  测试完全不受影响；`emission.rs` 的 debug 断言改为对 `ExternalLabel::BASE`
  校验块数。

**验证**：workspace 1383 passed / 0 failed / 19 ignored（68 suites）；
x86 矩阵 195/3/0；riscv64 131/67/0；fmt/clippy `-D warnings` 干净。

### S4（子项 a）：终结符用值进入 use-def（2026-09-15）

S4 主体（终结符并入指令流）之前，先把**正确性缺口**独立补上：`Use.user` 只能是
`Inst`，因此终结符里的用值——分支条件与 `then/else` 实参、`jump` 实参、`ret`
返回值、`switch` 判别值与 case 实参、`invoke`/`resume` 用值——**完全不在
use-def 中**。后果是 `Function::replace_all_uses` 名不副实（pass 对分支实参做
RAUW 会留下悬空实参），而 `Verifier` 的 use-list 检查**看不见**这一类（它也只
校验指令操作数）。

- `Terminator::for_each_value` / `for_each_value_mut`：终结符用值的**唯一遍历序**
  （`Branch: cond, then_args, else_args`；`Switch: discriminant, default_args,
  cases…`；…）。两份遍历由同一个 `macro_rules!` 模板展开 ⇒ 记录用的下标序与
  改写用的下标序**结构性一致**，不可能各自漂移；`used_values()` 也改由它实现。
  单槽变体（`Resume`）的末次自增由 `let _ = idx;` 显式读一次（`unused_assignments`）。
- `use_list.rs`：`Use { value, user: Inst, operand_idx: u8 }` →
  `Use { value, site: UseSite, operand_idx: u32 }`，`UseSite::{Inst(Inst),
  Term(Block)}`。新增 `record_terminator`（写终结符后）、`remove_terminator`
  （按**旧**用值精确摘除，`O(用值数)`）、`forget_terminator`（按宿主清扫，
  顺序不敏感）、`has_use_at`、`user_blocks`；`user_insts`/`user_blocks` 去重保序。
  `verify()` 改为**双向**：终结符用值必须在 use-lists 中，且每条 `Term` 记录
  必须在 DFG 终结符的同一平坦下标上取到同值。`Use` 从 12 字节增至 16 字节
  （`SecondaryMap<Value, SmallVec<[Use; 4]>>` 内联容量 64 字节），换取终结符覆盖。
- `Function::set_terminator(block, term)`：写终结符的**唯一公开入口**——`mem::replace`
  取出旧终结符（owned，无克隆）→ 精确摘除旧 use 项 → 写新终结符 → 登记新项。
  `DataFlowGraph::set_terminator` 降为 `pub(crate)`（低层原语，不动 use-lists）。
  新增 `refresh_terminator_uses(block)` 供 `retarget`/`replace_args`/`remove_arg`
  这类就地改写后重登记（`retarget` 只改目标块、不动用值，可跳过）。
- `Function::replace_all_uses` 现在同时写指令操作数与终结符槽位（按 `UseSite` 分派）；
  `apply_replacements` 删掉手写的 7 变体终结符 match，改用规范序 `for_each_value_mut`
  并按块刷新（净减约 60 行且有唯一事实源）；`remove_block_param` 删参数后刷新前驱。
- 全部 15 处终结符写入点收口：builder 的 7 个终结符方法改走 `func.set_terminator`；
  forge-opt 的 8 处直接写字段（`sccp` 3、`jump_thread` 4、`dead_code` 1、`const_fold` 1）
  改为 `func.set_terminator`（`sccp`/`const_fold` 先算出新终结符再写，避免与
  `lattice`/`known` 的借用冲突）；`tail_call`/`insert_preheader`/`loop_unroll`（2）
  由 `dfg.set_terminator` 换成 `Function::set_terminator`；解析器 phi 回填的
  `replace_args` 后加 `refresh_terminator_uses`。唯一外部 `Use` 字段读者
  `mem2reg`（4 处）改为 `u.site.as_inst()`（终结符宿主不算 Load/Store ⇒ 不提升）。

**验证**：workspace 1392 passed / 0 failed / 19 ignored（71 suites；较 S4 子项
1383 增 9 = `terminator.rs` 3 + `use_list.rs` 2 + `tests/use_lists.rs` 4）；
x86 矩阵 195/3/0；riscv64 131/67/0；fmt/clippy `-D warnings` 干净。
新增守卫：`UseLists` 双向校验（终结符缺项/陈旧项都报 `UseListInconsistency`）、
`terminator.rs` 的平坦序规格测试与"共享/可变遍历一致"测试。

### S4（子项 b）：终结符写入面收口（2026-09-15）

S4-a 把终结符用值纳入 use-def 之后，"谁能写终结符"本身就成了不变量的一部分。
本子项把**写面收口从约定变成编译期强制**：

- `BlockData::{terminator, has_terminator}` 降为 `pub(crate)`，新增只读访问器
  `BlockData::terminator()` / `BlockData::has_terminator()`；crate 外
  （forge-opt / forge-codegen / forge-rustc / forge-ir 集成测试共 22 个文件）
  的读取全部改为访问器。**crate 外已无法直接写终结符字段**，唯一路径是
  `Function::set_terminator`。`DataFlowGraph::block_terminator_mut` 同步降为
  `pub(crate)`（补一个只读的 `block_has_terminator`）。
- 新增 `Function::rewrite_terminator(block, f)`：就地改写终结符（改实参、增删用值）
  后**自动重登记 use 项**——"不想重建整个终结符"时的入口。
- **暴露并修掉三处真实缺陷**：`forge-codegen/src/pipeline/compiler.rs` 里的 `ret`
  值就地改写（大聚合返回值展开、段值替换、`ret` 内 RAUW）从不刷新 use-def；S4-a
  之后它们会留下陈旧 use 项。三处现在都走 `rewrite_terminator`；
  `insert_preheader.rs` 的 `pred.terminator.retarget(...)` 一并改走同一 API
  （`retarget` 只动目标块不动用值，重登记是恒等操作，但契约不破）。
- **可达性取证（不是推断）**：在 `expand_large_agg_ret` 的调用点临时插 panic 探针，
  跑 `cargo test -p forge-codegen`（21 suites 全绿）与
  `cargo test -p forge-tests --lib`（43 用例，含 x86/riscv JIT 矩阵）**均未触发**
  ⇒ 该路径在本机不可达，只由 forge-rustc e2e 覆盖（本机无法编译 `forge-rustc`，
  缺 `rustc-dev`）。因此把该 API 的契约用
  `tests/use_lists.rs::rewrite_terminator_keeps_use_def_fresh` 钉在本地。

**验证**：workspace 1393 passed / 0 failed / 19 ignored（71 suites，较 S4-a +1）；
x86 矩阵 195/3/0；riscv64 131/67/0；fmt/clippy `-D warnings` 干净。

### S4（子项 c）：块级表示收口——"未终止"成为显式状态（2026-09-15）

S4 主体（终结符并入指令流、块实参变操作数）必须**一次提交内完成**：只要还有
一个变体留在 `Terminator` 里，块就有"指令终结符 / 枚举终结符"两种形态，读写方
就得同时处理两者——那正是用户明令禁止的双表示。在动主体之前，先把它最后的结构
歧义清掉：

- `BlockData` 的 `terminator: Terminator` + `has_terminator: bool` 两个字段
  合并成 **`terminator: Option<Terminator>`**。此前默认值 `Terminator::Unreachable`
  同时充当"未终止"占位与"显式 unreachable"，只能靠布尔位区分；任何按 `Default`
  构造出来的块都会把"漏写终结符"静默伪装成"显式 unreachable"。现在两件事在类型上
  分开：`None` = 尚未终止。
- 读取口按"是否需要容忍坏 IR"分成两个，**不存在静默回退**：
  - `BlockData::terminator()` —— **fail-closed**：未终止即 panic（与
    `Function::entry()` 同一套契约）。所有"假设 IR 已成形"的读取点用它，
    于是"漏写终结符"会**响亮失败**而不是被当成 unreachable 继续算 CFG。
  - `BlockData::terminator_opt()` —— 给必须容忍坏 IR 的调用方：**校验器**、
    display、解析器的元数据校验。
  - **CFG 构造**（`Function::{predecessors, successors}`、`DominatorTree::build`、
    `LoopForest::build`）走 `terminator_opt()`：未终止块"没有出边"是结构事实，
    不是回退——校验器本来就要在坏 IR 上跑（`Function::predecessors()` 在
    `check_entry` 里就被调用）。
- 行为保持不变的两处独立兜底：`check_terminators` 仍报 `MissingTerminator`；
  `check_path_termination` 对 `None` 仍报 `PathWithoutReturn`（原实现正是靠
  `has_terminator` 区分这两者的）。`FunctionBuilder::finish` 的防御改为
  `terminator.is_none()`，报错文案不变。
- 新增守卫 `tests/verify_negative.rs::unterminated_block_is_explicit_state`：
  ①新块 `terminator_opt()` 为 `None`；②CFG 构造/支配树容忍且不 panic；
  ③经 `terminator()` 读取**必须 panic**（`catch_unwind` 验证"响亮失败"）。
  `dfg::tests::test_make_block` 同步从"新块终结符 = Unreachable"改为"新块未终止"。

**验证**：workspace 1394 passed / 0 failed / 19 ignored（71 suites，较 S4-b +1）；
x86 矩阵 195/3/0；riscv64 131/67/0；fmt/clippy `-D warnings` 干净。

### S4（子项 d）：写入口按形式化 + 读取面投影化（2026-09-15）

主体必须一次提交内完成（见上），所以先把"表示相关的调用点"压到少数几个函数里，
使那次原子切换只需要重写这些函数的**实现体**：

- **写侧**：`Function` 新增按形式命名的写入口——`jump` / `branch` / `ret` /
  `switch` / `unreachable` / `invoke` / `resume`，外加 `set_return_values`
  （改 `ret` 实参）、`retarget_terminator`、`replace_terminator_args`。
  调用方不再拼 `Terminator`；`Function::set_terminator` 与 `rewrite_terminator`
  收为 `pub(crate)` ⇒ **crate 外已无法构造或就地改写 `Terminator`**（实测写点残留 = 0）。
  迁移：builder 的 7 个终结符方法委托给新入口；forge-opt 的 7 个文件全部改走新入口；
  codegen 三处 `ret` 值改写改走 `set_return_values`，其中一处的 RAUW 改用
  `replace_all_uses`——顺带修掉"手写逐块改 `inst.operands` 却不刷新 use-lists"
  的又一处置空；`loop_unroll` 的 `clone_terminator` 换成 `emit_cloned_terminator`
  （按投影读源块 + 按形式写目标块，不再 match 变体）；解析器 phi 回填走
  `replace_terminator_args`。
- **读侧**：新增 `TermKind`（无载荷判别）与 DFG 投影访问器——`term_kind` /
  `term_branch` / `term_jump` / `term_return_values` / `term_switch` /
  `term_invoke` / `term_resume_value` / `term_is_unreachable` / `term_args_to` /
  `term_used_values` / `for_each_term_value`。
- **新守卫** `tests/terminator_api.rs`（6 例）：7 种形式**写进去能按同形式投影读回**
  （种类/目标/实参/用值/后继），`TermKind` 判别与实际变体一致，写入口与就地改写
  都让 use-def 保持新鲜。
- **残余表示相关面（下一轮原子切换要处理的全部内容）**：forge-opt 的约 35 处读取
  match（const_fold 8 / dead_code 7 / loop_unroll 5 / jump_thread 4 / sccp 3 /
  gvn_pre 3 / tail_call 3 / insert_preheader 3 / ind_var_simplify 2 /
  block_param_coalesce 2 / inline 2 / lto+func_specialize+copy_prop 各 1）+
  三处**天生表示感知**的边界（display 打印、codegen lowering、文本解析）+
  校验器内部的变体 match + 访问器/写入口自身的实现体。

**验证**：workspace 1400 passed / 0 failed / 19 ignored（72 suites，较 S4-c +6）；
x86 矩阵 195/3/0；riscv64 131/67/0；fmt/clippy `-D warnings` 干净。

### S4（子项 e）：读取面全部改走投影（forge-opt 的终结符依赖清零）（2026-09-15）

S4-d 建好了投影访问器与按形式写入口，本步把消费者全部搬过去：

- forge-opt 的全部读取 match 迁移：`const_fold`（`collect_uses` 的 7 变体 match
  换成一行 `for_each_term_value`，另 2 处断言）、`dead_code`（DFS 后继 + 2 处断言）、
  `jump_thread`（4）、`sccp`（可达性传播）、`gvn_pre`（后继）、`tail_call`（3）、
  `inline`（2）、`lto` / `func_specialize` / `copy_prop` / `ind_var_simplify` /
  `block_param_coalesce` / `insert_preheader` / `loop_unroll`（含 `find_first_body_block`
  与 `collect_body_chain` 的后继跟踪）。
- codegen 的 3 处 `ret` 读取与 1 处 invoke/resume 判别、forge-rustc
  `compile.rs` 的诊断打印也改走投影。
- **结果：`crates/middle/forge-opt` 已 0 处引用 `Terminator`**（`git grep -c` 实测），
  该 crate 不再依赖终结符表示。
- **顺带修掉一处遍历口径缺陷**：`const_fold::collect_uses` 的 `Switch` 分支漏了
  `default_args`（只收 discriminant + case args），改走 `for_each_term_value` 后
  default 实参也计入"被使用"——这正是"手写逐变体遍历"与规范序唯一事实源分叉的产物。
- **残余（全部属于原子切换本身）**：forge-ir 内部（访问器/写入口实现、`use_list`、
  `verify`、`ir_parser`、`dfg`/`function`）+ 三处天生表示感知的边界（display 打印、
  codegen lowering 及其 DSL 生成器、文本解析）+ `tests/terminator_api.rs` 的
  变体一致性断言。

**验证**：workspace 1400 passed / 0 failed / 19 ignored（72 suites，用例数不变——
纯迁移）；x86 矩阵 195/3/0；riscv64 131/67/0；fmt/clippy `-D warnings` 干净。

### S4（子项 f）：crate 内的读取面也全部走投影（2026-09-15）

S4-e 之后外部 crate 的表示依赖已清零；本步把 **forge-ir 自己**的读取点也搬过去，
使 S4 主体只剩"访问器/写入口的实现体 + `Terminator` 定义本身"要动：

- `use_list`：`record_terminator`/`remove_terminator` 不再收 `&Terminator`，改为
  `(block, &DataFlowGraph)` 经 `for_each_term_value` 读取；`verify` 的终结符两条
  检查同样改走访问器 ⇒ use-list 层完全不感知终结符的存储形态。
- `verify`：全部变体 match 改走投影——`check_terminators` 用 `block_successors`
  收集目标（并删掉逐变体重复的目标校验），`check_block_params` 用
  `term_branch`/`term_jump`/`term_switch`，`check_dominance` 用 `term_invoke`，
  `check_path_termination` 用 `term_kind`，两处可达性 BFS 用 `block_successors`。
- `display`：`TerminatorDisplay` 从"持有 `&Terminator` 并 match 变体"改为
  "持有 `block` + 按 `term_kind` 分派 + 投影取载荷"——打印这个边界不再依赖存储形态。
- `ir_parser/semantics.rs`：终结符元数据的校验与附着改走新增的
  `DataFlowGraph::{term_metadata, term_metadata_mut}`（解析 AST `ParsedTerminator`
  与本项无关，原样保留）。
- 有意的小行为变化：`check_terminators` 现在对**每个**非法跳转目标各报一条
  `InvalidTerminatorTarget`（旧代码对 `Switch` 只报第一条 case 就 `break`）——
  错误信息更完整，测试全绿。

**统计**：forge-ir 内 `Terminator` 引用 241 → 205，其中 60 处于定义/测试、
40 处于解析 AST（`ParsedTerminator`），真正剩余的"存储形态相关"只有
`dfg.rs`（访问器实现 33）+ `function.rs`（写入口实现 19）+ `terminator.rs` 定义本身。

**验证**：workspace 1400 passed / 0 failed / 19 ignored（72 suites，用例数不变）；
x86 矩阵 195/3/0；riscv64 131/67/0；fmt/clippy `-D warnings` 干净。

### S4（主体）：终结符并入指令流（2026-09-15）

八个前置子项（§6 上）把表示相关的调用点收进少数函数之后，主体在**一次提交**内完成：

- **表示**：终结符不再是枚举——它就是一条指令（`Opcode::{Ret, Jmp, Br, Switch,
  Unreachable, Invoke, Resume}`），存 `DataFlowGraph::insts`，由
  `BlockData.terminator: Option<Inst>` 引用。**不进 `inst_order`**：块内指令列表
  仍只含非终结符指令，因此"遍历块内指令"的全部既有语义与约 40 处迭代点不变
  （这是把 349 处引用的迁移面压到可控范围的关键取舍）。
- **编码**（`ops.toml` 的「终结符」节与 `dfg.rs` 解码一一对应，且解码处自校验）：
  `Ret` operands=返回值；`Jmp` operands=实参 + `[Block(target)]`；`Br`
  operands=`[cond, then_args…, else_args…]` + `[Block(then), Block(else),
  Uint(then_argc), Uint(else_argc)]`；`Switch` operands=`[disc, default_args…,
  case_args…]` + `[Block(default), Uint(default_argc), Uint(case_count),
  (Int, Block, Uint(argc))…]`；`Invoke` 同形带 `Func`/`Type(ret_ty)`；`Resume`
  operands=`[value]`。**不新增 immediate 变体、不新增 arena**——`Immediate::Block`/
  `Type` 早已存在，case 表内联进 immediates。
- **读**：`TermKind` 由 opcode 派生（`terminator::term_kind_of`，并有守卫与
  `ops.toml` 的 `category = "terminator"` 对账）；DFG 的投影访问器全部改为从
  终结符指令解码；`term_switch` 返回结构化 `SwitchView`（case 表来自 immediates）。
- **写**：按形式写入口直接编码指令；`set_terminator` 负责"墓碑化旧终结符指令 +
  发射新指令 + 同步 use-lists"；`retarget`/`replace_args`/`remove_arg` 改为
  "解码 → 改 → 按形式写回"（避免手写操作数 splice）。
- **use-def 归一**：`UseSite` 与 `record_terminator`/`forget_terminator` 等一并删除
  ——终结符用值就是那条指令的 operands，`UseLists` 只剩"指令操作数"一条路径，
  `verify` 的终结符专项检查也随之消失（被"每条指令的操作数都登记"覆盖）。
  `apply_replacements` 的终结符专项分支同样删除。
- **边界**：`TargetLowering::lower_terminator` 形参从 `&Terminator` 改为
  `(dfg, block)`，DSL 生成器改为按 `TermKind` 分派 + 投影取载荷 ⇒ **ISA TOML 与
  `[[lowering]]` 规则一字未动**；打印（`TerminatorDisplay`）解析器、校验器在 S4-f
  已经只依赖访问器，本次未改。
- **顺带**：`ops.toml` 新增 7 条（`category = "terminator"`，variadic + results=0 +
  type_rule none）；`opcode_table` 的穷举表与分类计数基线同步；forge-tests 的
  `UNCOVERED_OPS` 登记 7 条（终结符由 `lower_terminator` 路径与 JIT 控制流用例覆盖）；
  `display_llvm` 的往返比较从"比终结符值"改为"比语义形态"（两侧指令句柄必然不同）。

**验证**：workspace 1394 passed / 0 failed / 19 ignored（72 suites）；
x86 矩阵 195/3/0；riscv64 131/67/0；fmt/clippy `-D warnings` 干净。
新增守卫 `tests/terminator_api.rs::terminator_is_an_inst_outside_inst_order`
（终结符是 `insts` 里的指令、**不在** `inst_order`、其操作数在 use-def 里、
重写后旧指令墓碑化且 use 项摘除）。

### S6（子项）：终结符诊断点名真实指令句柄（2026-09-15）

S4 主体让终结符成为指令之后，校验器里与终结符相关的 5 个错误变体仍只报块号或带
伪造句柄，属于"表示已换、诊断没跟上"的残留：

- **问题**：`BlockParamCountMismatch` / `ReturnTypeMismatch` /
  `ReturnValueTypeMismatch` / `InvalidTerminatorTarget` /
  `TerminatorDominanceViolation` 这 5 类错误**全都精确归属于一条终结符指令**，
  但诊断里要么只有块号，要么用 `Inst(u32::MAX)` 占位（源码注释原文：
  "终结符无独立指令句柄可见性"）——消费者（display / forge-rustc 诊断）无法把
  错误定位到具体那条指令，也无法在块内有多个候选时区分。
- **做法**：5 个变体各加 `inst: Inst` 字段并在 `Display` 里打印
  （`block {}: terminator inst {} …` 等）；构造点全部改为取**真实句柄**——
  `check_uses` 的终结符用值循环绑定 `term_inst`、`check_block_params` 绑定
  前驱块的终结符指令、`switch` case 重复与非法跳转目标各自取
  `block_terminator(block)`。两处 `Inst(u32::MAX)` 伪造随之删除；这些
  `expect` 都挂在"投影已命中"的分支上，坏 IR 上不会因缺终结符而 panic。
- **守卫**：`tests/verify_negative.rs::terminator_diagnostics_carry_real_inst`
  ——`ret` 值数量不符时 `ReturnTypeMismatch.inst` 必须等于该块的终结符指令；
  用公开写入口 `Function::jump` 把跳转目标改到不存在的块后，
  `InvalidTerminatorTarget.inst` 必须等于**新**终结符指令、且不等于被墓碑化的
  旧句柄（同时钉住"重写终结符 = 墓碑 + 新发指令"）。

**验证**：workspace 1395 passed / 0 failed / 19 ignored（56 个测试二进制 +
13 组 doc-test，较 S4 主体 +1）；x86 矩阵 195/3/0；riscv64 131/67/0；
fmt/clippy `-D warnings` 干净。

### S5（第 1 项）：`isel_strategy` 类型化 + 字段私有化（2026-09-15）

S4 主体收口了终结符表示之后，指令上最后一个"开放集合裸字符串"附件就是
`Instruction.isel_strategy: Option<&'static str>`。本步把它类型化并把字段私有化，
**不改变任何指令序列**（该通道当前既无生产者也无消费者，见下）。

- **类型**：新增 `IselStrategy`（新模块 `src/isel_strategy.rs`），**刻意不是枚举**
  ——标签集合由目标 ISA 数据决定（例 `"lea_sib:4"` = `Iadd(Imul(idx,4), base)` 走
  LEA 的 SIB 形式、倍率 4），forge-ir 不解析、不认识任何具体名字：无枚举、无白名单、
  无字符/长度限制，名字内嵌的参数原样保留。修掉两个真实缺陷：
  ① **`'static` 逼生产者泄漏**——名字来自运行期数据（TOML/匹配表）时必须
  `Box::leak` 才能变成 `&'static str`（审计记录过那次泄漏修复）；现在
  `IselStrategy::new` 收任意 `&str`/`String`，字面量走 `from_static` 零拷贝；
  ② **裸字符串让错配静默**——手写标签 `"lea_sib"` 与 DSL 侧
  `"lea-merge-iadd-imul-4"` 比较为假时编译期毫无提示（审计记录的断点）；类型化后
  要比较必须先构造 `IselStrategy`，且**刻意不实现** `PartialEq<str>` /
  `Deref<Target = str>` / `Default`（空名 fail-closed；`None` 是"无标签"的唯一编码，
  与 S4-c 对终结符消灭"同一件事两份编码"同一条理由）。
- **承载选型**：值语义用 `ImmStr`（≤22 B 内联零分配、长名 `Arc<str>` 共享、
  `Clone` O(1)），**不用 `InternedStr`**（`StringPool` 池内 id）——inline / lto /
  func_specialize 要把标签从**被调方的 DFG** 搬到**调用方的 DFG**，池内 id 跨池失真。
- **可见性**：字段降为 `pub(crate)`，读 `Instruction::isel_strategy()`、写
  `set_isel_strategy` / `clear_isel_strategy`；5 处"保留全字段"复制点
  （`dfg::clone_inst`、`lto`、`inline`、`func_specialize`、`function.rs` 的克隆测试）
  全部改走写入口 ⇒ crate 外无法再直写附件（与 `BlockData.terminator` 同一约定）。
- **现状（必须记清，避免误读为"已接通"）**：手写 pattern-isel 生产者
  （`ext/pattern_isel.rs`，`lea_sib`/`cmovcc` 标签）已随 ISA-DSL v15 的"删死模块"
  删除，`[[pattern]]`/`lower_pattern` 侧用的是 pattern 名——两条命名体系的对接仍是
  backlog #2 的功能开发项（会改变指令序列，需专项验证）。本步只做**类型化与可见性**，
  不改变 pass / 后端的任何行为。

**验证**：workspace 1406 passed / 0 failed / 19 ignored（57 个测试二进制 +
13 组 doc-test，较 S6 子项 +11）；x86 矩阵 195/3/0；riscv64 131/67/0；
fmt/clippy `-D warnings` 干净。新增守卫 `tests/isel_strategy.rs`（5 例：运行期名字
免泄漏、`clone_inst` 保留、跨 DFG 搬运、单值覆盖写、空名 panic）+ 模块内 6 例
（短名内联、长名共享、字面量借用三种承载 + 空名两条入口 + 值语义）。
首轮整包跑出 2 处**我自己测试里的断言错误**（20 B 名字其实走内联、派生 `Debug`
打印 `IselStrategy("…")`），已修正——说明"类型承载变体"的断言必须实测而非按印象写。

### S5（第 2 项）：开放集合划边界（2026-09-15）

S5 第 1 项把 `isel_strategy` 定量类型化之后，本步给出**整类"开放集合"的边界**
并清掉越界的那一处。边界三分：

- **(a) 闭合集合**：由单一事实源闭死（opcode = `ops.toml` 生成、`Icmp/Fcmp` 条件码、
  指令类别与效果、终结符种类、`MetadataKind` 的 well-known 项）；
- **(b) 目标/ISA 数据**：开放，但值由 ISA/目标数据声明（寄存器类与宽度、栈槽与
  对齐、指令字宽、pattern 名、`IselStrategy`、`TargetTriple` 各段）；
- **(c) 用户程序数据**：开放（函数/块/值/全局/结构体/`section` 名、metadata 自定义
  kind、`Immediate::String`、`source_filename`、`module asm`）。

**规则**：不得从 (b)/(c) 的字符串反推 (a) 或**任何数值**——那就是宿主白名单，
表外取值只会得到静默错误答案；数值一律来自 ISA 数据（指针宽度 = `DataLayout` 的
`p:<size>:<abi>`，由 ISA `[meta] addr_width` 派生）。另：(b)/(c) 的字符串数据一律
用 `ImmStr`，IR 公开面不出现裸 `String` 字段。

**本步实做**：

1. **删掉越界的那处**：`TargetTriple::{is_32bit, is_64bit, os_name}` 是把架构名/OS 名
   写成宿主查表的三个查询（`"x86_64" | "aarch64" | …`），表外架构（`loongarch64`、
   用户自定 ISA 名）**两个都返回 `false`**——"既非 32 位也非 64 位"的静默错误答案，
   而且与 ISA 自己声明的 `addr_width` 可能矛盾。实测**零生产调用点**（只有它自己的
   单测），故按"无需兼容旧版本结构"直接删除。`TargetTriple` 只留 `parse`/字段/`Display`：
   四个分量原样保留、原样往返，无归一化表、无"未知 → 默认"改写。
2. **统一 (c) 类的承载**：`Module.source_filename: Option<String>` →
   `Option<ImmStr>`、`module_asm: Vec<String>` → `Vec<ImmStr>`（SSO + `Arc<str>` 共享、
   `Clone` O(1)；迁移解析器 2 处写入点，`Display` 处靠 deref 无需改）。IR 公开面
   至此没有裸 `String` 字段。
3. **守卫** `tests/open_set_boundary.rs`（4 例）：行为断言——未知架构名经
   parse → display → parse 原样往返；指针宽度只跟布局字符串走（同一 `p:32:32` 挂在
   x86_64 与 loongarch64 两个 triple 下都是 4 字节）。源码断言——`src/` 不得出现
   带引号的架构名字面量或 `fn is_32bit/is_64bit/os_name`；IR 公开面不得出现
   `pub … : String` 字段（`src/ir_parser/**` 的解析期 AST 按设计例外，白名单必须被命中）。
   **守卫已用负向探针实测会失败**（临时放入 `src/zz_guard_probe.rs` 含
   `pub probe_field: String` + `"x86_64"` 后两条源码断言双双 FAILED，删除后恢复绿）
   ——否则"永远绿的守卫"等于没有守卫。

**验证**：workspace 1410 passed / 0 failed / 19 ignored（58 个测试二进制 +
13 组 doc-test，较 S5 第 1 项 +4）；x86 矩阵 195/3/0；riscv64 131/67/0；
fmt/clippy `-D warnings` 干净。首轮整包又抓到 1 处**我自己新写断言的错误**
（三段式 `riscv64-unknown-elf` 的第三段是 OS 而非 environment，已按实测修正）
——"目标三元组的段语义"同样只能实测，不能按印象写。

### S5（第 3 项）：metadata 单写（2026-09-15）

附件 metadata（`AttachedMetadata` = `(kind, node)` 对）此前有**多条写路径**，
而且语义还不一致：指令可经构造参数或直接 `inst.metadata.push(..)`，终结符要经
`term_metadata_mut` 返回的 `&mut SmallVec` **逃逸可变引用**（可以整表替换、清空、
乱序，附件约定无处安放），函数是 `func.metadata.push(..)`，全局变量则是
`gv.metadata = attached`（整表替换）——与函数的追加语义不一致。

本步把 5 个载体的写入各收成**一个入口**并把字段私有化：

| 载体 | 读 | 写（唯一入口） |
| --- | --- | --- |
| 指令 | `Instruction::metadata()` | `Instruction::attach_metadata()` |
| 终结符 | `DataFlowGraph::term_metadata()` | `DataFlowGraph::attach_term_metadata()` → `TermMetadataAttach` |
| 函数 | `Function::metadata()` | `Function::attach_metadata()` |
| 全局变量 | `GlobalVariable::metadata()` | `GlobalVariable::attach_metadata()` |
| 别名 | `GlobalAlias::metadata()` | `GlobalAlias::attach_metadata()` |

- **终结符**：删掉 `term_metadata_mut`（`pub(crate)` 的 `&mut SmallVec` 逃逸），
  换成返回 `TermMetadataAttach::{Attached, NoTerminator, Unreachable}` 的单写入口
  ——此前解析器用两段检查（`term_kind().is_none()` + `term_metadata_mut().is_none()`）
  区分两种失败原因，现在由返回值显式分类，解析器的两条诊断一字未改。
- **创建期的初始表**仍走 `make_inst_with_meta_and_loc` 的构造参数（构造参数不是
  "事后补写"）；`clone_inst` 保持"复制全字段"。
- **追加语义统一**：全局变量从"整表替换"改为逐条追加（解析期该表必为空，
  行为等价）；同一 kind 多次附加即多一条（与文本里多处 `!dbg !N` 一一对应；
  重复 kind 的合并/拒绝不在本步范围，属 S6 校验契约）。
- **跨 crate 读取面**迁移：`forge-opt` 的 inline / lto / func_specialize 三处
  "保留全字段"从 `inst.metadata.clone()` 改为 `inst.metadata().iter().cloned().collect()`
  （`SmallVec::from_slice` 要求 `Copy`，`AttachedMetadata` 非 `Copy`——实测编译报错后改）。

**验证**：workspace 1416 passed / 0 failed / 19 ignored（59 个测试二进制 +
13 组 doc-test，较 S5 第 2 项 +6）；x86 矩阵 195/3/0；riscv64 131/67/0；
fmt/clippy `-D warnings` 干净。新增守卫 `tests/metadata_single_write.rs`（6 例：
指令追加序、终结符三种返回值（含 `unreachable` 拒绝）+ 读口恒空、函数/全局追加、
**文本层四载体端到端落位**、builder 产物可挂附件、源码断言"写入只许出现在唯一写
入口实现体里"）。源码断言同样用**负向探针**实测会失败（`src/zz_guard_probe.rs`
里放 `inst.metadata.push(..)` ⇒ FAILED，删除后恢复绿）。

## 7. 参考设计（外部）

- Cranelift：two-map 实体容器与"刻意不支持删除"
  <https://docs.rs/cranelift-entity/latest/cranelift_entity/>；`PackedOption`
  <https://docs.rs/cranelift-entity/latest/cranelift_entity/packed_option/index.html>；
  verifier"重算 CFG/支配树再比对 + 带指令正文诊断"
  <https://github.com/bytecodealliance/wasmtime/blob/main/cranelift/codegen/src/verifier/mod.rs>
- MLIR：属性/类型由 context 拥有并 uniqued
  <https://mlir.llvm.org/docs/DefiningDialects/AttributesAndTypes/>；trait/interface 分层
  <https://mlir.llvm.org/docs/Interfaces/>
- LLVM：opaque pointer 迁移方法论 <https://llvm.org/docs/OpaquePointers.html>；
  bitcode 自描述与 `Upgrade*` 层 <https://llvm.org/docs/BitCodeFormat.html>；
  verifier 的自我定位 <https://github.com/llvm/llvm-project/blob/main/llvm/include/llvm/IR/Verifier.h>
- 其他：QBE <https://c9x.me/compile/doc/il.html>；rustc MIR
  <https://rustc-dev-guide.rust-lang.org/mir/index.html>；Go SSA
  <https://go.googlesource.com/go/+/HEAD/src/cmd/compile/internal/ssa/README.md>；
  V8 退回 CFG 的实测 <https://v8.dev/blog/leaving-the-sea-of-nodes>；SSA 构造
  <https://pp.ipd.kit.edu/publication.php?id=braun13cc>；Rust 容器取舍
  <https://docs.rs/slotmap/>、<https://docs.rs/index_vec/>；韧性 LL 解析
  <https://matklad.github.io/2023/05/21/resilient-ll-parsing-tutorial.html>
- 反面教训（明确不抄）：完整 dialect/trait/interface 体系、未实测的 e-graph 多表达、
  Sea-of-Nodes、对文本 IR 做字节级 fuzz、把 LALR 生成解析器作为长期方案。

## 8. 非目标（明确不提议）

- **不拆巨型文件**、不统一 forge-dsl 双语法（用户既有决策）；
- catchpad/cleanuppad 全族、metadata kind 白名单校验、L1 DI 验证器、vscale 的
  TypeId/codegen 扩展、inline asm 编码器集成、statepoint、裸全局引用 init、
  autoupgrade、多值 `ret`（均已否决，出处见 `docs/archive/forge-ir/`）；
- 不改 IR 为 Sea-of-Nodes；不引入完整 dialect 体系；
- 不动 `forge-grammar` semantic 层；`grammars/ir.lx` 是 forge-grammar 的测试夹具，
  不是 forge-ir 的语法文件。
