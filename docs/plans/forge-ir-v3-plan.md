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
| S1 | 指令元数据单一事实源 | 待开工 |
| S2 | 实体容器与密集索引 | 待开工 |
| S3 | 类型系统去锁/所有权 | 待开工 |
| S4 | 终结符归一 + 完整 use-def | 待开工（依赖 S1） |
| S5 | 附件强类型化与可见性 | 待开工（依赖 S4） |
| S6 | 校验与 pass 契约 | 待开工（**含 S0 暴露的 pass 欠账**） |
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
