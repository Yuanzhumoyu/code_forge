# forge-ir backlog（活跃待办速览）

> forge-ir LLVM 文本层工作流的历轮计划/记录文档已于 2026-09 归档至
> [`docs/archive/forge-ir/`](../archive/forge-ir/)（各档均带 ARCHIVED 头）。
> 下表为归档时（2026-08 基线：LLVM Assembler 452 全收敛、roundtrip 0 缺口、
> workspace 全绿）**仍未关闭的待办**摘录，每项标注出处档供开工前回读全文。
> ⚠️ 开工前请以代码现状复核（文档停更于 2026-08，仓库已继续演进）。

| # | 待办 | 状态提示 | 出处（archive/forge-ir/） |
| --- | --- | --- | --- |
| 1 | `agg_expand` 第三批（rewrite 族 4 函数 ~400 行）迁移 | **已完成（2026-10-08）**：`memoryize_from_segs` / `memoryize_from_addr` / `rewrite_agg_value_uses` / `move_to_front` 已从 `pipeline/compiler.rs` 迁入 `pipeline/agg_expand.rs`（含文档注释块，逐行切片搬移而非人工转写）；`compiler.rs` 2940 → 2544 行、`agg_expand.rs` 177 → 574 行；四个函数改 `pub(crate)`，调用点经 `use crate::pipeline::agg_expand::{…}` 引入；`agg_expand.rs` 里那条占位注释改成实况（仍留 `compiler.rs` 的是 `expand_*` 执行族，与 `CompileState` 时序耦合更紧）。**连带按纪律更新了读路径预算**：`read_path_budget` 的 `compiler.rs` 21 → 17，并新增 `agg_expand.rs` 的 13 条目（4 处 `.borrow()` 随搬移转移）。验收：整仓门禁 exit 0 | code-quality-audit.md（第 47 轮"剩余状态"） |
| 2 | 结构性重构：`make_inst` 三层包装收敛、egraph 改名、巨型文件拆分、forge-dsl 双语法统一、`isel_strategy` 标签接通、HexLit 超 64 位折叠对齐 | **2026-10-08 逐项复核（结论：只剩一项需决策）**：`HexLit 超 64 位折叠对齐` **已落地**（归档档第 217 行列在"本轮落地"表：lexer 现为 `HexLit(Big)`，不再静默折 0）；`make_inst` 层次是**有意设计**（`Dfg::make_inst` 为"不要直接用"的裸层，`Function::make_inst*` 是 S6 定的唯一入口，另加 `use_lists.record_inst`）；`巨型文件拆分`、`forge-dsl 双语法统一` 均是**用户已决定远期**（归档第 195/207 行）；`egraph 改名` 为命名清理（egraph 不在现行管线）。**仍需决策**：`isel_strategy` 标签只做了存取 + 二进制往返（`funcs.rs:332/333/744`），后端无消费者——"接通"要什么语义（选指策略？pass 开关？）属设计口径 | code-quality-audit.md 历轮"遗留/记录"节、remaining-tasks.md §3 |
| 3 | extractvalue/insertvalue 常量**深层**折叠的 builder API 缺口（`fb.extract_value` 仅单层、AggConst 递归 child 缺深层索引） | **折叠侧已修（2026-10-08）；builder 侧判定"不加"**：`const_fold.rs` 现沿 `extractvalue` 链上溯出（根部常量聚合 × 索引路径）并深层下钻 `AggConst` 树（新增 `agg_child_at` / `resolve_agg_chain`），嵌套链一次折到标量；复现/回归用例 `fold_nested_extractvalue_chain`（修前实测 `changed=false, removed=0`）。**builder 不加路径版**：这套 IR 里没有"独立聚合常量值"——聚合常量只作为 `extractvalue` 的 immediate 出现（`semantics.rs:2480`），路径助手没有根可起步，加了是投机 API。`insertvalue` 的常量折叠仍不折（要新造聚合常量，且无实测用例；与 emit 表化同类，需要时再评估） | remaining-tasks.md §1.2 |
| 4 | P2 长期：4.1 round-trip fuzz、4.2 负向全覆盖、4.4 端到端执行对照 | **三项均已落地（2026-10-08 复核）**：4.1 `forge-ir/tests/roundtrip_fuzz.rs`（`fuzz_roundtrip` + 默认/替换种子两条用例）与 `forge-codegen` 的 `encoder_fuzz_tests.rs::fuzz_roundtrip_decode_encode_decode`；4.2 `forge-ir/tests/verify_negative.rs:959` 的「错误码 × 回归测试对账守卫」（`VerifyError` 每个变体都必须有测试，否则点名）；4.4 `crates/tools/forge-tests/asm/exec` 的 20 个真语法小程序（x86 原生、riscv64/aarch64 走 QEMU，缺通道则显式 SKIP 不假绿）+ `src/exec/fuzz.rs` 的 37 条性质测试 | iteration-roadmap.md §4 |
| 5 | 性能候选：grammar 拆分、criterion 基准接入 | **criterion 已接入**（2026-10-08 复核：`crates/*/benches/` 下 `ir_binary.rs`/`ir_parse.rs` 等用 `criterion_group!`+`criterion_main!`，`docs/performance/BENCHMARKS.md` 是其入口）；`grammar 拆分`与"巨型文件拆分"同属**用户已决定远期**的口径 | iteration-roadmap.md §5 |
| 6 | P1 残余：SjLj 异常 codegen（长期）、va_arg ABI 布局语义、>16B 聚合栈传参（现 Unsupported） | **2026-10-08 复核**：`va_arg ABI 布局语义` **已随变参 V0–V5 落地**（四份约定的 `va_list` 形状齐备并由 `invariants.rs` 的 `va_shapes_match_the_documented_table`/`va_object_layout_matches_the_psabi_numbers` 钉住，SysV `%al` 写入 V7 已做，见 `docs/archive/varargs-plan.md`）；`SjLj` 仍**长期**。**>16B 聚合**是唯一实质缺口，且 ABI 计划侧已具备数据：逐形参 `DeclAttrs`（LLVM `byval`/`sret`/`inreg`/align，`engine.rs:98-102`）、byval 临时区 `StackAlloc`（`engine.rs:576-577`）、帧内地址计算与宽拷贝能力（`engine.rs:33/35`），`agg24`/`mixed_wide_ret` 在 abi 测试里有用例；缺口只在 IR 展开（`agg_expand` 段模型限 ≤16B，四个拒绝点在 `compiler.rs:63/179/321/398`）。**两种约定的 IR 形态不同**（win64＝调用方栈上副本+指针 `Indirect{on_stack:true}`；sysv64＝按值整块进传出区 `Placement::Stack{size}`）⇒ 要先决定**展开 pass 是否引入 ABI 计划依赖**（该 pass 现刻意只依赖 `forge_ir` 与 `agg_const`，原因是常量池克隆时序）——属设计决策 | iteration-roadmap.md §3.2/§2.5、next-iterations.md L2/L7 |
| 7 | Windows SEH/ARM EH codegen（随 ABI 选择）、statepoint（无近期计划）、M1 拆分监控（>15s 触发） | 长期/条件性；**2026-09-12 未复核** | next-iterations.md §0/§3 |
| 8 | forge-grammar semantic 层接入（产品决策）、forge-tests fuzz 归并、jit `register_external` 吞错改 Result 透传 | **2026-09-12 复核**：fuzz 归并 ✅ 已落地（`forge-tests/src/exec/fuzz.rs`，16 条性质测试）；`register_external` ✅ 已是 `Result<(), IrError>` 且透传 patch 错误；**仍开放**：semantic 层零外部调用（`forge-grammar/src/lib.rs` 注明"第二十九轮起 NameResolver/SymbolTable/TypeChecker 零外部调用，mini_c 用自研"） | remaining-tasks.md §3 |

## 2026-09-14 复核记录（forge-ir v3 方案 S0）

新增待办（S0 实测发现的 pass 不变量欠账，归属 v3 方案 **S6**）：

- `inline` / `gvn_pre` / `mem2reg` 等 pass 运行后会留下 use-list 不一致或返回类型
  不匹配的 IR——此前 pass 后校验只 `log::warn`，欠账被掩盖；S0 把校验策略化
  （`forge_opt::PassVerify`，默认 `Warn`、严格模式 `Error`）并用
  `strict_verification_reports_known_debt` 钉住。
  **✅ 同日已清偿**：三处 pass 欠账修掉（`inline` 补终结符 RAUW、`gvn_pre` 补登记 +
  支配守卫、`mem2reg`/`pgo`/`lto`/`func_specialize` 同批），`Function::make_inst*`
  成为建指令唯一入口（配套 `UseLists::forget_inst` + `Function::refresh_inst_uses`），
  `PassVerify::Error` 转为 `#[default]`，守卫测试换成
  `strict_verification_passes_for_all_pipelines`；详见
  [`docs/archive/forge-ir-v3-plan.md`](../archive/forge-ir-v3-plan.md) §5/§6。
- 覆盖矩阵（`forge-tests/src/coverage.rs`）此前用 `match op { ... _ => ... }` 兜底，
  34 个 opcode 静默无覆盖；S0 改为显式 `UNCOVERED_OPS`（逐条原因）+ 完整性守卫断言
  `COVERAGE_OPS ∪ UNCOVERED_OPS == Opcode::ALL`。
- 方案与全部已落地项见 [`docs/archive/forge-ir-v3-plan.md`](../archive/forge-ir-v3-plan.md)。

**仍开放（S6 相关的类型模型欠账，归 S4/S5）**：`expand_geps` 会生成类型自洽性不足的
IR（`%p = add i64 %prev, %t` 却声明 PTR 结果，源码原注释即"verify 不跑"）——根因是
ISA-DSL 尚无 `Ptrtoint`/`Inttoptr` 降级；codegen 侧的 use-list 断言因此**刻意不跑完整
`Verifier`**（范围已在 `compiler.rs` 注释写明，不是静默容忍）。

## 2026-09-12 复核记录

本表停更于 2026-08，仓库此后经历 v14/v15 大改；本轮以**当前代码**（`rg` + 阅读源码）复核，
只改有证据的状态列，未复核项如实标注"未复核"（不臆断）：

- 复核命令：`rg` 全 crate 搜索（`agg_expand`/`rewrite` → 0 命中；`extract_value`、
  `NameResolver`/`SymbolTable`、`fuzz` 的命中位置见上表）；
- 结论：**#3、#6（聚合部分）、#8（semantic 层）仍开放**；**#1 前提失效**；
  **#8 的 fuzz 归并与 `register_external` 已落地**；#2/#4/#5/#7 未复核；
- 纪律：本表只作入口索引——开工前**必须**按上表逐项回读源码，勿据表内旧陈述直接动手。

## 工作流约定（源自归档档，仍适用）

- 新开工一轮：从本表挑项 → 完成后成果回写（建议新开轮次记录，勿回写已归档档）→
  更新本表状态列；
- 验收纪律与命令速查见 archive/forge-ir/ 各档（iteration-checklist/remaining-* §4）与
  `docs/archive/forge-ir/README.md` 组说明。
