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
| S3 | 类型系统去锁/所有权 | 待开工 |
| S4 | 终结符归一 + 完整 use-def | 待开工（依赖 S1） |
| S5 | 附件强类型化与可见性 | 待开工（依赖 S4） |
| S6 | 校验与 pass 契约 | **部分落地**：S0 暴露的 pass 欠账已全部清偿，校验默认策略切到 `Error`（见 §6 末）；校验器/契约的进一步强化待续 |
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
