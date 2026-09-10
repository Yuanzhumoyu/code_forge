# v14-forge-ir-redesign 路线图状态（2026-08 交接文档）

## ⚠️ ARCHIVED（2026-09）

> v14-forge-ir-redesign 路线图交接文档（P0–P5 执行史，2026-08）。
> 2026-09 已自述大幅过期；剩余开放项已移交
> `crates/tools/forge-rustc/WORKAROUNDS.md`（WA-37/38/39）与其 README 路线图节。
> 本文为历史记录，仅供参考；代码现状以仓库代码与现行文档为准，不再维护。
> 本文档记录审查路线图（P0-P5）的执行状态与剩余事项的技术路径，供后续迭代/决策参考。
> 所有状态均以代码为准（非文档承诺），生成自实际提交历史与全量测试验证。
>
> **新工作线**：ISA-DSL v12（唯一语法，不兼容 v11）已完成迭代 1-6、Phase 1-7
> 与**第三轮破坏性重构**（effect 语义标签统一、ABI 指令键、占位符注册表
> `placeholder.rs`、编号操作数/临时动态化——见 `docs/reference/isa-dsl.md` 与
> `docs/archive/asm-dec-generic-design-v2.md` 第三轮记录）；**v11 语法层已物理删除**
> （forge-dsl v11 实现、v11 arch 后端 x86_64/aarch64/riscv64/wasm32/minimal_sd、
> forge-asm、exec-unicorn），详见 `docs/archive/isa-dsl-v12-roadmap.md` §22。
> 当前基线：forge-dsl 51 测试、forge-codegen jit 全套、forge-tests 34
> （riscv 矩阵 126 QEMU 真执行）、mini_c 全绿。

## 已完成的迭代（git 提交可追溯）

| 阶段 | 内容 | 验证 |
| --- | --- | --- |
| **P0 固化** | WIP 分块提交×5（forge-ir 语料/后端管线/前端/工具链/文档）；roundtrip 失败清单自愈（display_llvm 自删过期记录）；mini_c 残余回归测试（loops got 1 / params SEGV 确认已修复）；integration_tests 注释对齐 | 48 段全绿 |
| **P1 IR 层** | FloatCC 全 16 条件（补 false/true/ueq/ugt/uge/ult/ule/une）+ **x86/aarch64/riscv NaN 语义修正**（UCOMISS 对 NaN 置 ZF=PF=CF=1，原 oeq/olt/ole 实为 ueq/ult/ule）；Frem 独立 opcode（废弃 frem→Fdiv 映射）；fcmp 文法 CondName（false/true 词法歧义） | NaN JIT 测试 + 16 条件解析测试 |
| **P2 解码器** | DSL 生成 TargetDecoder 完整收官：Phase 1 定宽（riscv64/aarch64/minimal_sd）+ Phase 2/2b/2c/2d/2e x86 变长（@modrm/@op_rm/@cmovcc/push/pop/mov_imm64/setcc + SSE 族 7 + @modrm_mem 内存 + VEX 族 3 + LEA 族 3） | decoder_smoke 12/12 往返 |
| **P3 前端** | 删除语义层死代码（NameResolver/SymbolTable/TypeChecker，-2k 行）；静默降级改显式报错×3（maps_to→compile_error、SimpleRegex→panic、for_level 空表→告警）；AttrValue::Block 直 cast 消除（BlockId + block_map） | forge-hir/mini_c 全绿 |
| **P4 工具链** | jit feature 真实 gate（forge-codegen 门控 + 根转发 + mini_c/forge-tests 显式启用）；GDB JIT e_machine 按宿主架构；CI 覆盖 v14 分支；fmt/clippy/test/doc 门禁本地全绿（含恢复 4 个丢失 #[test] 与 f64::NAN 弃用修复） | 门禁全过 |
| **文档** | 25 份 md 全部核对更新：README/CLAUDE/BENCHMARKS/OPTIMIZATION/CHANGELOG、docs/reference/isa-dsl.md（v11 重写 1624 行，已随 v12 删除重写为 v12 规范）/encoding-guide.md（重写 488 行，已随 v11 语法层删除）/forge-ir 系列 6 份/mini_c/forge-rustc/forge-tests README、bench_baseline/coverage 计数刷新 | 以 4 份审计报告为准 |
| **回归增强** | mini_c dual_backend 历史排除项转正（循环内条件分支/真条件 while/do-while 双栈变量 +3 测试）；decoder_smoke 12 测试 | 全绿 |

**当前基线**：`cargo test --workspace --exclude forge-rustc` 全绿（v12 唯一
后端；forge-dsl 51、forge-codegen jit 全套（63+13+10+5+20+15+3+9+4+3+12+13）、
forge-tests 34（含 riscv 矩阵 126 QEMU 真执行）、mini_c 22+19+90+28、
demo_v12 3）。

## 剩余事项（需投入决策，技术路径如下）

> **2026-09 更新**：本清单已大幅过期——YMM ABI 主库侧已实现（下 §1）；
> vec_push/vec_string 已关闭（WA-11，e2e 全绿）；**并行 CGU 已全链路完成并
> 关闭**（M3 task 化 → M4 真并行根治 WA-38（函数任务 par_map 提交 rustc
> 查询池，`-Z threads>=2` + FORGE_CODEGEN_THREADS>1 启用）→ M5 内容稳定键
> → M6 Stage B（每 CGU 独立对象 + 多 WorkProduct）→ B-v2（debuginfo
> per-CGU CU）→ `-C incremental` 多 CGU 端到端；提交 `b43ce8d`…`42bf37e`）。
> 剩余开放项与 2026-09 方案的完整路径见
> `crates/tools/forge-rustc/WORKAROUNDS.md`（WA-37/WA-38/WA-39）与
> `crates/tools/forge-rustc/README.md` 路线图节。

### 1. YMM ABI（>128 位向量传参）——✅ 主库已实现（2026-08-31）

- **现状**：S1-S5 已实现（提交 ed43103/e9e869a/33df6fa/4959474，早于 HEAD）——
  >128 位向量 by-ref 传参（调用方 temp 槽 store + GPR 指针）+ sret 返回 +
  混合槽位全链路；JIT 测试 7 个全绿（jit.rs v256/v512 byref param、
  wide_vector_call byref/sret、mixed/sret_with_byref）。入口守卫
  `compiler.rs` 由一律拒绝改为 by-ref 能力选择（≤16B 寄存器 / >16B by-ref /
  >32B 需 avx512）。
- **剩余缺口（D2-D6，见 `docs/archive/ymm-abi-plan.md` 状态块与 WA-37——该档 2026-09-10 已由 plans/ 归档，D2-D6 亦均已落地）**：forge-rustc B3
  门控分级解除（只解 V256，前置向量 local 全宽 load/store 基建）；≤16B 向量
  按值 XMM 全宽移动（V64/V128 静默截断，B3 保护中）；V512 全 lane 验证；
  第 5+ GPR 槽（建议不做）；CallIndirect 测试。主库 compiler/lowering/frame/
  regalloc **无需再改**（现状即终态）。

### 2. forge-rustc vec_push/vec_string —— ✅ 已关闭（WA-11）

- **2026-09 状态**：grow 链（Result/ControlFlow/TryReserveError 嵌套 niche 错误
  传播）累积修复收官——`vec_push`/`vec_string`/`vec_from_slice`/
  `string_concat_len` 全部转正（e2e 全绿）；本段 2026-08 记录的"2 预期失败"
  已不成立。

### 3. 解码器 Phase 2e 之后（低优先）

- `@lea_rip_rel` 已实现（global 字段语义为符号引用，解码给 raw disp32——字节往返一致）。
- `@leb128`（wasm32 专属）与 `@call_reloc*`/`@abs_reloc`（重定位占位——分支/调用目标需 CFG/链接上下文）**解码正确排除**（返回 `DecodeError::Other`），非缺口。

## 环境性限制（非代码问题）

- forge-rustc e2e 需 nightly + rustc-dev（本机可用，第 2 轮验证 56 过 + 2 预期失败）。
- `exec-unicorn`（vendored unicorn 跨架构模拟）已随 v11 后端删除——aarch64/riscv64
  无 v12 对应后端。
