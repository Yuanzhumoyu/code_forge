# v14-forge-ir-redesign 路线图状态（2026-08 交接文档）

> 本文档记录审查路线图（P0-P5）的执行状态与剩余事项的技术路径，供后续迭代/决策参考。
> 所有状态均以代码为准（非文档承诺），生成自实际提交历史与全量测试验证。
>
> **新工作线**：ISA-DSL v12（唯一语法，不兼容 v11）已完成迭代 1-6（含
> mini_c v12 全特性端到端 25/25）；**v11 语法层已物理删除**（forge-dsl v11
> 实现、v11 arch 后端 x86_64/aarch64/riscv64/wasm32/minimal_sd、forge-asm、
> exec-unicorn），详见 `docs/isa-dsl-v12-roadmap.md` §22。

## 已完成的迭代（git 提交可追溯）

| 阶段 | 内容 | 验证 |
| --- | --- | --- |
| **P0 固化** | WIP 分块提交×5（forge-ir 语料/后端管线/前端/工具链/文档）；roundtrip 失败清单自愈（display_llvm 自删过期记录）；mini_c 残余回归测试（loops got 1 / params SEGV 确认已修复）；integration_tests 注释对齐 | 48 段全绿 |
| **P1 IR 层** | FloatCC 全 16 条件（补 false/true/ueq/ugt/uge/ult/ule/une）+ **x86/aarch64/riscv NaN 语义修正**（UCOMISS 对 NaN 置 ZF=PF=CF=1，原 oeq/olt/ole 实为 ueq/ult/ule）；Frem 独立 opcode（废弃 frem→Fdiv 映射）；fcmp 文法 CondName（false/true 词法歧义） | NaN JIT 测试 + 16 条件解析测试 |
| **P2 解码器** | DSL 生成 TargetDecoder 完整收官：Phase 1 定宽（riscv64/aarch64/minimal_sd）+ Phase 2/2b/2c/2d/2e x86 变长（@modrm/@op_rm/@cmovcc/push/pop/mov_imm64/setcc + SSE 族 7 + @modrm_mem 内存 + VEX 族 3 + LEA 族 3） | decoder_smoke 12/12 往返 |
| **P3 前端** | 删除语义层死代码（NameResolver/SymbolTable/TypeChecker，-2k 行）；静默降级改显式报错×3（maps_to→compile_error、SimpleRegex→panic、for_level 空表→告警）；AttrValue::Block 直 cast 消除（BlockId + block_map） | forge-hir/mini_c 全绿 |
| **P4 工具链** | jit feature 真实 gate（forge-codegen 门控 + 根转发 + mini_c/forge-tests 显式启用）；GDB JIT e_machine 按宿主架构；CI 覆盖 v14 分支；fmt/clippy/test/doc 门禁本地全绿（含恢复 4 个丢失 #[test] 与 f64::NAN 弃用修复） | 门禁全过 |
| **文档** | 25 份 md 全部核对更新：README/CLAUDE/BENCHMARKS/OPTIMIZATION/CHANGELOG、docs/isa-dsl.md（v11 重写 1624 行，已随 v12 删除重写为 v12 规范）/encoding-guide.md（重写 488 行，已随 v11 语法层删除）/forge-ir 系列 6 份/mini_c/forge-rustc/forge-tests README、bench_baseline/coverage 计数刷新 | 以 4 份审计报告为准 |
| **回归增强** | mini_c dual_backend 历史排除项转正（循环内条件分支/真条件 while/do-while 双栈变量 +3 测试）；decoder_smoke 12 测试 | 全绿 |

**当前基线**：`cargo test --workspace --exclude forge-rustc` 全绿（v12 唯一
后端；mini_c v12 25/25、dual_backend 19/19、x86_v12 14/14、riscv64_v12 9/9、
集成 12/12、forge-codegen 100/100）。

## 剩余事项（需投入决策，技术路径如下）

### 1. YMM ABI（>128 位向量传参）

- **现状**：`pipeline/compiler.rs:1673-1691` 编译入口守卫——向量参数/返回 >16 字节直接 `IrError::Unsupported`（避免静默只传低 128 位）。
- **实现路径**（Windows x64 ABI）：>128 位向量**按引用传参**——调用方在栈上分配副本、传指针（GPR）；被调方入口经 `[abi.call].entry_*` 从 `[ptr]` 加载；返回走 sret 隐藏指针参数。需改动：① DSL 的 ABI 参数分类（TOML `[[abi.arg_class]]` 的 `strategy = "by-ref"` + `limit` 已就绪，见 `isa/x86_v12.toml` [abi]）；② `@move_args`/收参代码生成；③ call lowering 的栈拷贝；④ 返回路径。
- **风险**：ABI 正确性跨调用链，历史多轮稳定化过——需在改动后跑全部 298 条 JIT 用例 + forge-rustc e2e。

### 2. forge-rustc vec_push/vec_string（known_failure）

- **现状**：e2e 58 用例中 2 个预期失败（`vec_push` SEGV / `vec_string` len 错），WA-11 记录十二轮深挖。
- **根因假设**（WA-11 收敛）：Vec grow 链（`grow_amortized` 的 new_cap 计算/实参错——两个 max 调用 + `_17=const 8/4/1` 候选）+ 嵌套 niche 传播（rustc 布局，`CF::Break(Err(5u8))` 应=7 现=1 同源）。
- **技术路径**：`cargo test -p forge-rustc --test e2e` + `FORGE_TRACE_TERM/LOWER` 追踪 grow 路径 MIR→forge-ir 降级；主库侧 `[lower.Select]` 恢复原生（WA-17 已修规则层，regalloc 重叠检查后验证）。
- **风险**：MIR 级调试，投入不确定（项目历史 12+ 轮），需用户决定投入预期。

### 3. 解码器 Phase 2e 之后（低优先）

- `@lea_rip_rel` 已实现（global 字段语义为符号引用，解码给 raw disp32——字节往返一致）。
- `@leb128`（wasm32 专属）与 `@call_reloc*`/`@abs_reloc`（重定位占位——分支/调用目标需 CFG/链接上下文）**解码正确排除**（返回 `DecodeError::Other`），非缺口。

## 环境性限制（非代码问题）

- forge-rustc e2e 需 nightly + rustc-dev（本机可用，第 2 轮验证 56 过 + 2 预期失败）。
- `exec-unicorn`（vendored unicorn 跨架构模拟）已随 v11 后端删除——aarch64/riscv64
  无 v12 对应后端。
