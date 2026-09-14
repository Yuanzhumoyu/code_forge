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
| `dfg` | `DataFlowGraph`（`values`/`insts`/`blocks` 三个 arena）、`Instruction`、`BlockData` |
| `function` | `Function`、`Module`、`Layout`、`GlobalVariable`/`GlobalAlias`、`AnalysisCache` |
| `types` | `TypeStore`（interner）、`TypeContext`、`FunctionSignature` |
| `opcode` / `immediate` / `inst_flags` / `mem_flags` | 指令操作码与附件（`opcode` 的枚举与派生表由 `ops.toml` 生成，见下） |
| `builder` | `FunctionBuilder`（构造 IR 的唯一推荐入口） |
| `use_list` / `analysis` / `loop_info` / `alias` | def-use 链、支配树、循环森林、最小别名分析 |
| `constant` / `big` / `imm_str` / `string_pool` | 常量池（int/float/big/vector/aggregate）、任意精度、SSO 字符串 |
| `verify` | `Verifier`（63 条规则 / 13 阶段） |
| `display` / `ir_parser` | LLVM 文本输出（logos + lalrpop 解析） |
| `metadata` / `debug_info` / `symbol` / `data_layout` | 元数据、调试信息、符号、DataLayout |

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
  use-lists 新鲜。**注意 `replace_all_uses` 只覆盖指令操作数**，终结符里的用值
  （分支实参/`ret` 返回值）要用 `apply_replacements` 或 `Terminator::args_to`/`retarget`。
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
（`ops.toml` + 生成枚举/派生表/名字与 LLVM 文本名映射/逐指令类型规则族，查表全 O(1)），
下一步按方案进入 S2（实体容器 `PrimaryMap`/`SecondaryMap`）。
