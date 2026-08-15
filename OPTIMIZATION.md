# Optimization List

基于 `benches/compile_bench.rs` 全套基准（默认测量时间，2026-08-03，本机
12th Gen i9-12900H / Windows 11）得出的编译管线热点分析与优化清单。

运行基准：`cargo bench --bench compile_bench -- '^(ir_build|ir_parse|opt_|pipeline_breakdown|codegen|verify|module|throughput|code_size|comparison|e2e_compile_)'`
（`e2e_jit_execute` 已纳入默认运行——JIT 正确性修复后不再挂起，见 BENCHMARKS.md）。

时间均为 criterion median（µs，除注明 ms）。

---

## 0. 结论速览

| 优先级 | 项 | 现象（实测） | 状态 |
| --- | --- | --- | --- |
| **P0** | forge-grammar 解析 O(n³) 病态 | 2 条指令 13.0 ms → 8 条 100 ms → 300 条 ≈ 5.6 h | **已修复（2026-08-03）**：根因为 lexer O(n²)，零拷贝 + match_here 后 simple_add 13ms→317µs；再经 logos+lalrpop 重写 → 9.34µs（见 §9） |
| **P1** | SCCP 单 pass 最贵 | 72.5 µs @ many_ops（const_fold 的 2.5×） | **已部分实施**：小整数快路径（§9，-12%） |
| **P1** | codegen_mem 最贵 | 451.9 µs（x86），e2e 172.6 µs | 需分析 |
| **P1** | codegen_big_loop | 1.14 ms（x86），e2e 310.9 µs | 需分析 |
| **P1** | Nop tombstone 残留 | O2/O3 管线留下 Nop，拖入 codegen | **已实施（2026-08-03）**：O2/O3 末尾追加 DeadCodeElim，e2e_compile_loop -36% |
| **P2** | const_fold（O1 最贵） | 28.9 µs @ many_ops | 可实施 |
| **P2** | codegen_float | 251.7 µs；e2e 206.7 µs | 需分析 |
| **P2** | 测量环境不稳定 | 同输入跨组 2–4× 差异 | 文档/流程 |
| **P2** | 小函数 prologue 开销 | codegen_simple_add 6.9 µs vs ir_build 1.9 µs | 可实施（需谨慎） |
| **P3** | verify 成本 | verify_many_ops 24 µs | 观察 |
| **P3** | 字节→指令解码器缺失 | `TargetDecoder` 无实现（`TargetDisassembler` 为 DSL 生成的 Inst→文本格式化器） | 记录 |

---

## 1. P0 — forge-grammar 文本解析 O(n³) 病态复杂度

> **✅ 已解决（2026-08-03，见 §9 实施记录）**：根因实为 lexer O(n²)（非文法 O(n³)）——
> 零拷贝 + match_here 后 simple_add 13ms→317µs；随后 logos+lalrpop 重写 → 9.34µs。
> 本节为修复前分析，保留作历史记录。

**证据**（`ir_parse` 组，release，median）：

| 样本 | 指令数 | 块数 | 时间 |
| --- | ---: | ---: | ---: |
| `ir_parse_simple_add` | 2 | 1 | 13.0 ms |
| `ir_parse_mul_add` | 3 | 1 | 25.6 ms |
| `ir_parse_big_text_4` | 4 | 1 | 39.4 ms |
| `ir_parse_dot_product` | 8 | 4 | 61.7 ms |
| `ir_parse_loop_sum` | 8 | 4 | 101.7 ms |
| `ir_parse_complex` | 9 | 3 | 99.4 ms |
| `ir_parse_multi_func` | 4 函数 | — | 204.4 ms |

- 指令数 ×2 → 时间 ×3（2→4 条：13→39 ms）；×4 → ×7.7（2→8 条：13→100 ms）。
  实测拟合 ≈ **O(n³)**（debug 下 16 条 3.5 s、64 条 148 s；300 条预计 5.6 h，
  基准直接挂起）。
- 对比：程序化构建同规模 IR 只要 µs 级（`ir_build_many_ops` 13.6 µs），
  文本解析慢 **3 个数量级**。
- 已定位到的次要点：`parse_operand_value_ast` 的 `LocalIdent` 查找是
  `value_names` 线性扫描（O(n²) 贡献）；主因疑似 forge-grammar `parser.rs`
  的 `Alt` 回溯 + `ZeroOrMore` 嵌套（`block ::= ... stmt* terminator` 中每条
  `inst` 的 `(results "=")? opcode operands immediates? flags? ";"?` 组合）。
  另：多函数单源解析的 ZeroOrMore 已知有 bug（BENCHMARKS.md），需分开解析。

**建议**：

1. 先用 profiling（`cargo bench --profile-time` 或 perf）定位热点函数；
2. 检查 `snapshot`/`restore` 是否反复拷贝/重建 CST 子树；
3. `value_names` 改为 `HashMap<InternedStr, Value>`（O(1) 查找）；
4. 给 `ZeroOrMore`/`Alt` 加 memoization（packrat）或在 `inst` 规则加前视
   消除回溯（`results` 模式先行匹配）；
5. 大文本基准 `ir_parse_big_text_4` 保持 n=4（避免挂起），修复后恢复 n 参数。

**验证**：`cargo bench --bench compile_bench -- 'ir_parse'`；修复后
`ir_parse_big_text_N` 应能提高到 ≥64 且时间随 n 近似线性。

---

## 2. P1 — SCCP 单 pass 最贵

**证据**：

| pass | 输入 | 时间 |
| --- | --- | --- |
| `opt_sccp` | many_ops | 72.5 µs |
| `opt_const_fold` | many_ops | 28.9 µs |
| `opt_gvn` | many_ops | 16.6 µs |
| `opt_egraph` | many_ops | 8.8 µs |
| `opt_cse` | many_ops | 8.7 µs |

- SCCP 是 many_ops（61 条 IR）上最贵的单 pass，约为 const_fold 的 2.5×。
- `pipeline_breakdown/o2/sccp` 测得 179.6 µs（环境噪声偏高，相对排名一致）。
- O2 管线（complex）28.0 µs vs O1（complex）6.5 µs——O2 增量 21.5 µs 中
  SCCP/GVN/egraph 是主要贡献者。

**建议**：分析 `scalar/sccp.rs` 的工作队列与 lattice 更新；检查是否重复
扫描 worklist、`value_names`/use-list 查找热点。SCCP 在编译器中通常可用
稀疏求值（仅处理 def-use 可达节点）显著加速。

**验证**：`cargo bench --bench compile_bench -- 'opt_sccp'`，目标 < 40 µs。

---

## 3. P1 — codegen_mem / codegen_big_loop 最贵（x86_64）

**证据**（codegen 组，x86_64；本组受环境噪声影响整体偏高 ~2–4×，
相对排序有效，`comparison` 组为低噪声复测）：

| 基准 | IR 指令 | codegen 组 | comparison 组 |
| --- | ---: | ---: | ---: |
| `codegen_mem` | 61 | 451.9 µs | — |
| `codegen_big_loop` | ~100 | 1.14 ms | — |
| `codegen_many_ops` | 61 | 389.7 µs | 92.7 µs |
| `codegen_float` | 41 | 251.7 µs | — |
| `codegen_complex` | 17 | 116.6 µs | 28.4 µs |
| `codegen_simple_add` | 3 | 19.6 µs | 6.9 µs |

- mem 函数（20 轮 store/load）与 big_loop（20-op 循环体）codegen 最贵；
  浮点链次之。e2e 组（O2+codegen）同趋势：`e2e_compile_mem` 172.6 µs、
  `e2e_compile_float` 206.7 µs、`e2e_compile_big_loop` 310.9 µs。
- 代码量：simple_add 44 B、many_ops 497 B、complex 156 B、multi_block 122 B
  （`code_size` 组）——many_ops 每 IR 指令 ~8 B，正常；耗时与字节不成比例，
  热点在 lowering/regalloc 而非 emit。

**建议**：对 `compile_raw` 做阶段细分（lowering / regalloc / frame / emit）
的 profile；重点看 mem 函数（store/load 序列）与 big_loop（多块+循环）在
寄存器分配/溢出上的开销。

**验证**：`cargo bench --bench compile_bench -- 'codegen_(mem|big_loop|float)'`。

---

## 4. P1 — Nop tombstone 残留（简单可落地）

**证据/已知问题**：BENCHMARKS.md 已记录——GVN/CSE/dead-code 将死指令改写为
`Opcode::Nop`，codegen 会跳过它们（已修复编译正确性），但 Nop 会一直携带到
codegen（污染 use-list/块布局遍历，增加后续 pass 与 codegen 的工作量），
仅当后续还有 DCE 时才被清理。

**建议**：在 O2/O3 管线**末尾追加一次 `DeadCodeElimPass(UntilFixedPoint)`**
（`forge-opt/src/lib.rs` 的 `for_level_with_table`，O2/O3 分支最后
`pm.add_pass(DeadCodeElim, UntilFixedPoint)`）。成本 ~7 µs（many_ops），
换来后续 pass/codegen 更干净。

**验证**：`cargo test -p forge-opt -p forge-codegen` + `opt_pipeline_o2/o3`
基准；断言管线后函数无 Nop（可在测试中加断言）。

---

## 5. P2 — const_fold（O1 最贵）

**证据**：`opt_const_fold` 28.9 µs（many_ops）、`opt_const_fold_float` 12.5 µs
（float）。O1 管线（complex）6.5 µs。BENCHMARKS.md 记录 const_fold 曾从
469 µs 降到 44 µs（many_ops），现已 28.9 µs——仍有下降空间但收益中等。

**建议**：复查 `scalar/const_fold.rs` 的固定点迭代（`UntilFixedPoint`）是否
每轮全量重扫；float 路径（dashu Big 浮点）是否可缓存。

**验证**：`cargo bench --bench compile_bench -- 'opt_const_fold'`。

---

## 6. P2 — 测量环境不稳定（基准方法）

**证据**：同输入跨基准组差异显著——

| 同工作 | 组 A | 组 B | 比值 |
| --- | ---: | ---: | ---: |
| compile many_ops | codegen 组 389.7 µs | comparison 组 92.7 µs | 4.2× |
| compile simple_add | codegen 组 19.6 µs | comparison 组 6.9 µs | 2.8× |
| SCCP on many_ops | opt_sccp 72.5 µs | pipeline_breakdown 179.6 µs | 2.5× |

**原因**：笔记本 CPU 频率/温度波动（ir_parse 组耗时长、跑在前面的组
CPU 状态差）。codegen 组紧跟慢速 ir_parse 后运行，数值系统性偏高；
`comparison`/`code_size` 组在最后、且单迭代更短，更接近真实值。

**建议**：

1. 跨运行对比一律用 `cargo bench -- --save-baseline X` / `--baseline X`
   （相对比值，不比较绝对数）；
2. 报告中标注"codegen 组受环境噪声影响，以 comparison 组为准"（已写入
   BENCHMARKS.md）；
3. 可选：`--measurement-time` 调大 + 锁频（电源计划高性能）。

---

## 7. P2 — 小函数 prologue 开销

**证据**：`codegen_simple_add` 6.9 µs（comparison）vs `ir_build_simple_add`
1.9 µs；`e2e_compile_simple` 13.0 µs。BENCHMARKS.md 记录 prologue 推送
Windows x64 全量 callee-saved（含 RDI/RSI），小函数开销占比大。

**建议**：prologue 按函数实际使用的 callee-saved 寄存器**按需推送**（liveness
扫描后只 push 用到的），可显著减小小函数 prologue/epilogue 的指令数与时间。
注意保持 ABI 正确（`jit_integration` 156 测试回归）。

**验证**：`cargo bench --bench compile_bench -- 'codegen_simple_add'` +
`cargo test -p forge-codegen --release`（JIT 回归）。

---

## 8. P3 — 其他观察（记录，暂不实施）

| 项 | 数据 | 备注 |
| --- | --- | --- |
| verify 成本 | verify_many_ops 24.0 µs、simple_add 2.9 µs、mem 8.4 µs | 相对管线成本可接受；若 PassManager 默认开启 verify，需评估开销 |
| module 级编译 | `module_compile_cross_call` 45.0 µs（2 函数含跨调用） | 正常；`opt_inline_real_table` 5.9 µs / `opt_tail_call_real_table` 5.5 µs（真实函数表 vs 空表 4.4/3.9 µs，成本接近） |
| 反汇编/解码 | 无任何后端实现 `TargetDecoder`/`TargetDisassembler` | 无法基准化；实现后补 `disasm` 基准组 |
| pipeline 增量 | O1 6.5 → O2 28.0 → O3 43.4 µs（complex） | O2 增量主要来自 SCCP/GVN/egraph；O3 增量来自 inline/mem2reg/loop 变换 |
| throughput 扩展性 | 50→500 ops：codegen 9.7×、O1 9.8×、O2 9.2×、O3 9.2×、ir_build 8.5× | **全部近似线性**（const_fold 修复后的成果），无需优化 |
| IPA 空表基准 | inline/tail_call/func_specialize 空表 3.6–4.4 µs | 已新增真实表版本对照 |

---

## 9. 实施记录

### 2026-08-03 — P1: Nop tombstone 残留（forge-opt）

**改动**：`crates/middle/forge-opt/src/lib.rs` 的 `for_level_with_table` 在
O2 末尾（egraph 后）与 O3 末尾（loop_unroll 后）各追加一次
`DeadCodeElimPass(UntilFixedPoint)`。

**复现**：新增测试 `o2_pipeline_nop_residue` 统计管线后 Nop 残留——
many_ops 输入 0 个（各级别），loop 输入 O2/O3 后 **5 个 Nop + 间接死代码**
（GVN/CSE/SCCP/egraph/loop 变换在 O1 的 DCE 位置之后产生）。

**回归**：

- `cargo test -p forge-opt`：91 通过；`cargo test -p forge-codegen`：107 通过
- 基准（默认测量时间）：`e2e_compile_loop` **67.4 → 43.0 µs（-36%）**
  （loop 是 Nop 残留输入）；`opt_pipeline_o3` 43.4 → 41.5 µs。
  注：环境噪声大，绝对值参考，方向性收益确认。

**结论**：有效。loop 型输入的 O2/O3 优化结果显著变干净，codegen 遍历成本
下降。

### 2026-08-03 — P1: SCCP 单 pass 优化（尝试后回滚）

**尝试**：`scalar/sccp.rs` 去掉传播/apply 阶段的 `inst_order.clone()`（改直接
借用迭代），期望减少块级重扫的分配。

**结果**：同轮 baseline 对比（`--save-baseline sccp_before` / `--baseline`）：
opt_sccp **58.2 → 64.9 µs（+9.6%）**——反而变慢（借用方式影响编译器优化或
测量噪声）。**已回滚**，`sccp.rs` 无净改动。

**结论**：SCCP 热点在 `evaluate_lattice` 的 `Vec<ConstValue>` 分配与 dashu Big
运算（块级 worklist 已是粗粒度），去 clone 不是正确方向。进一步优化需指令级
worklist 或小整数快路径，标为后续工作。

### 2026-08-03 — P2: const_fold / prologue / codegen_mem / ir_parse 评估结论

| 项 | 评估 | 决定 |
| --- | --- | --- |
| const_fold | 1279 行核心 pass，已有优化历史（469→44→28.9 µs）；进一步优化需 dashu 快路径 | 暂不实施（高风险低收益），记录 |
| 小函数 prologue 按需 push | 需 DSL 静态模板条件化（`@push_callee`/`@pop_callee` 是静态 insts 列表、epilogue 硬编码 `sub RSP, 56`）+ 栈布局/对齐/spill 偏移联动 | 需专项任务（DSL 模板机制扩展），不贸然实施 |
| codegen_mem/big_loop | 热点需 profile（Windows 无 perf，需阶段细分埋点） | 需专项分析 |
| ir_parse O(n³) | forge-grammar 组合器回溯为根因，属语法前端重构；value_names O(n²) 优化依赖此前置 | 需专项任务（P0） |

### 2026-08-03 — mini_c JIT SEGV / 循环错误：5 个根因（forge-codegen / forge-dsl / x86 TOML）

**背景**：`cargo test -p mini_c` 崩 STATUS_ACCESS_VIOLATION（integration_tests 第
一个测试就崩）、`test_hir_e2e_loops` 返回 1（应 55）、`test_hir_e2e_params`
SEGV。用 objdump 逐字节反汇编定位到 **5 个确定性 bug**（非 BENCHMARKS.md 曾
怀疑的 DSL 非确定性——多次 clean rebuild 均确定性复现）：

| # | 根因 | 现象 | 修复 |
| --- | --- | --- | --- |
| 1 | `$modrm_mem_rr`/`$lock_modrm_mem_rr` 宏 mod=00+rm=101（rbp/r13）被硬件解读为 RIP-relative 且缺 disp32 | store/load 变成 `mov [rip+垃圾], r` → SEGV | 宏对 rm==5/13 退化 mod=01+disp8=0（isa/x86_v10.toml） |
| 2 | `callee_saved_bytes` 漏计帧指针 8 字节 | 局部变量槽落在 callee-saved push 槽内（覆盖保存值） | 新增 `TargetRegInfo::frame_pointer_overhead()`（TOML `[abi].fp_push_bytes`：x86=8/aarch64=16/riscv=16/wasm=0），compiler.rs 两处计入 |
| 3 | Opsize 字段默认 64（`ctx.default_opsize` 未用于 lowering） | i32 load/store 用 8 字节访问，相邻 4 字节槽重叠 → 循环变量互相污染（返回 1） | DSL lowering 的 Opsize 默认改 `ctx.default_opsize`（cst_codegen.rs 两处 + codegen/mod.rs）；compiler.rs Store 无结果时用 operands[0] 类型 |
| 4 | `calculate_frame_size` 不含局部变量区（stack_addr 槽） | 局部槽落在 sub rsp 分配区之下（Windows 无 red zone）→ 参数内联场景 SEGV | `LowerCtx.max_stack_bytes` 跟踪最大槽深，frame_size = max(spill, locals) + 对齐 |
| 5 | SIB 语法：`?cond {modrm} {sib}` 的第二个 segment 无 `?` 前缀 | SIB 字节无条件输出且顺序错位（modrm 前）→ 所有 store 多 1-2 字节 | SIB segment 独立加条件前缀（两处宏） |

**回归**：`cargo test -p mini_c` 全绿（lib 19+16、integration_tests 90、
dual_backend 2）；`cargo test --workspace --exclude forge-rustc` 39 组全过
（含 jit_integration 跨函数 Call）。

**顺带修复**：`[cc_names]` 的 o/no 从 SETcc 值（0x90/0x91）修为 Jcc 值
（0x80/0x81，jo/jno）；`[lower.Call]` 补 Windows x64 shadow space
（`SUB64_R_IMM32 RSP, 32` / `ADD64_R_IMM32 RSP, 32`）；mini_c README 的过时
Known Limitations（分支 bug、No Call）已修正。

### 2026-08-03 — P0: ir_parse O(n³) — 实为 lexer O(n²)（forge-grammar）

**定位**：CF_PARSE_STATS 阶段计时（探针）——`parse_expr` 全量仅 3-7ms，**lexer
占 99%**（n=4 203ms → n=16 3.4s，O(n²)）；parser 本身近线性（310-508µs）。
两处 O(n²) 来源：①`next_token`/`skip_whitespace_and_comments` 每 token 复制
剩余输入（`remaining: String`）；②`match_regex_prefix` 从最长前缀递减尝试
（每次 `chars.collect()` + `candidate` 复制）——每 token 的每个 regex 规则
O(剩余长度²)。

**修复**（crates/frontend/forge-grammar/src/lexer.rs）：

- `remaining` 改 `&self.chars[self.pos..]`（`&[char]` slice，零复制）
- `match_regex_prefix` 直接用 `SimpleRegex::match_here(chars)` 返回前缀匹配
  长度（O(匹配长度)），删掉最长前缀递减尝试
- `matches_pattern`/`match_here` 改 `&[char]` 签名
- 附带：ir_parser.rs `parse_operand_value_ast` 的 value_names 线性查找改
  `name_map: HashMap<String, Value>`（O(1)）

**效果**（release，criterion）：ir_parse_simple_add 13ms → **317µs（41×）**、
multi_func 204ms → 7.4ms（28×）、dot_product 61.7 → 1.87ms（33×）、
loop_sum 101.7 → 2.93ms（35×）；**big_text_256 24.4ms**（旧 300 条预计
5.6h 挂死）。debug 探针 n=128 从 55s → 46ms（1180×），近线性。

### 2026-08-03 — P1: SCCP / const_fold 小整数快路径（fold_binary_int）

`fold_binary_int`（scalar/const_fold.rs）加 `op_small: fn(i64,i64)->Option<i64>`：
操作数 `Big::try_to_i64()` 成功且 `checked_*` 不溢出时用原生 i64 运算（避免
dashu Big 堆分配 + clone）；溢出回退任意精度 Big。SCCP 的 `evaluate_lattice`
复用 `fold_opcode` 同步受益。**效果**：opt_sccp 72.5 → 63.5µs（-12%）、
const_fold_float 12.5 → 9.9µs（-21%）；opt_const_fold 整数不变（28.9µs——
热点在 `fold_function` 传播循环而非 fold_opcode，后续可做增量 worklist）。

### 2026-08-03 — P2: prologue 按需 push — 评估为需架构改动，未实施

regalloc 已产出 `callee_saved_to_save`（实际使用的 callee-saved），但
`StackAddr` 的 lea 偏移在 lowering（Stage 4-5）定死，实际 push 数在
regalloc（Stage 6）后才知——按需 push 会使 rsp 上移，lea/spill 偏移落到
rsp 之下（Windows 无 red zone → SEGV）。需 StackAddr 偏移动态化（emit 时
重算）或预扫描 callee-saved。收益（小函数 codegen ~1-2µs）与风险不成比例，
**列为专项任务**（含 `CF_CODEGEN_TIMING` 诊断埋点，env 控制）。

### 2026-08-03 — P1: codegen 热点阶段分析

`compile_raw` 加 `CF_CODEGEN_TIMING` env 诊断（保留，env 控制零开销）。实测
（release 无——debug 相对值）：**regalloc 占 58-76%**（mem_ops 749µs/1.29ms、
many_ops 659µs/1.01ms、complex 193µs/308µs、sum_to_n 196µs/257µs），
lowering 22-27%，emit ~10%。**regalloc（BacktrackingAllocator）是 codegen
最大热点**——后续优化方向：回溯/驱逐策略复杂度、liveness 构建。

### 2026-08-03 — ir_parser 重写（logos + lalrpop，LLVM IR 语法）

**背景**：旧解析器基于 forge-grammar（PEG），语法不完整（多函数受限、操作数
无类型、条件固定 Equal）。按用户要求用 **logos（词法）+ lalrpop（LR 语法，
build.rs 生成）** 重写，**严格 LLVM IR 语法**，**全 105 opcode**，**彻底替换**
（删旧 ir_parser.rs 与 forge-grammar 依赖）。

**架构**（crates/foundation/forge-ir/src/ir_parser/）：

- `lexer.rs`：logos token（关键字/类型/`@`/`%`/字面量/标点/`;` 注释）；
  Token 值类型化（IntTy(u16)/FloatTy(u16)/VecTy(VecElem 枚举)/IntLit(i64)/
  FloatLit(f64)，标识符保留 String——用户 steer 减少字符串）
- `grammar.lalrpop`：module（target/define/declare 多函数）、function、
  block（label: inst* terminator）、指令（类型化操作数；icmp/fcmp 带条件；
  call/alloca/load/store/gep/转换 to 规则；通用 Ident 查映射表）、
  terminator（ret/br/switch/unreachable）、type（ValueType 无 void、
  向量/数组/结构）、value
- `llvm_mapping.rs`：全 105 opcode LLVM→forge 映射 + IntCC 全 10 +
  FloatCC 8（其余报语义错误）+ vector_op（add <4 x i32>→Vadd）
- `semantics.rs`：AST→forge IR（名称解析/SSA 唯一性/类型检查/opcode 映射/
  trunc 按类型分发/terminator/跨函数 call + bind_name）

**集成测试**（tests/ir_parser_llvm.rs，20 个）：标量/浮点/位/饱和/icmp 全
10/fcmp/内存/控制流/转换/调用/常量/undef/向量/扩展/模块级/错误路径。
**bench 样本**迁移 LLVM 格式（7 个 Success）。**workspace 40 组回归通过**。

**基准加速**（criterion，默认测量时间）：确定性 LR 解析比旧 PEG 前端快
1-2 个数量级——ir_parse_simple_add 317µs→**9.34µs（34×）**、dot_product
1.87ms→10.2µs（183×）、loop_sum 2.93ms→12.8µs（229×）、multi_func
7.4ms→39.2µs（189×）、big_text_256 24.4ms→370µs（66×）。

（继续追加：后续专项任务的改动与回归。）
