# forge-isa-dsl 改进方案（ISA-DSL v19：可独立接入 + 数据化验证 + 规范体检）

> 状态：[progress]（2026-09-23 撰写）。用户 2026-09-19 起的口径延续：**允许破坏性更新、无需兼容
> 旧版本、可参考网络上的设计方案、需兼顾用户体验、需足够通用而非服务于个别指令集**。
>
> 上一轮（v18）执行方案已退役归档：`docs/archive/forge-dsl-v18-plan.md`（S0–S10 全部落地或经
> 度量判定不做，其数字仍是本文 §2 的证据来源）。v18 之后的临时插曲 S10a–S10d 见该档 §7.2。
>
> 一切数字为本机实测（命令、日期随行标注），**以代码与测试为准**。

## 目录

- [1. 目标与成功判据](#1-目标与成功判据)
- [2. 现状证据（本仓实测）](#2-现状证据本仓实测)
- [3. 外部设计参考（借什么、不借什么）](#3-外部设计参考借什么不借什么)
- [4. v19 设计总览与破坏性清单](#4-v19-设计总览与破坏性清单)
- [5. 切片计划](#5-切片计划)
- [6. 迁移与文档落地](#6-迁移与文档落地)
- [7. 边缘情况与失败模式](#7-边缘情况与失败模式)
- [8. 显式假设](#8-显式假设)
- [9. 验收门禁](#9-验收门禁)
- [10. 附录：v19 S0 基线](#10-附录v19-s0-基线)

## 1. 目标与成功判据

| # | 目标 | 硬判据（可实测） |
| --- | --- | --- |
| G1 | **任意普通 crate 都能承载一份 ISA 谱**：生成物只依赖一个"运行时"crate，不需要 forge-codegen | 新增最小宿主 crate 只依赖 `forge-isa-runtime`（+ build-dep `forge-isa-dsl`）即编译并跑通 encode/decode/asm + `__spec_tests`；`cargo tree` 里无 forge-codegen；`isa_from_file!` 的 `krate` 参数被删除 |
| G2 | **测试向量数据化**：黄金字节/负向用例写进谱，随谱走 | 发行 ISA 的黄金向量迁进 `[[vectors]]`；手写 Rust 黄金测试行数下降 ≥50%（给出前后数字）；`forge-isa test <谱>` 能独立跑 |
| G3 | **规范体检工具**：写谱的人能自己发现问题 | `forge-isa lint` 在三个发行 ISA 上零误报（清单快照钉住），输出含"能力缺口/未用声明/可合并族/未指定位"，支持 `--json` |
| G4 | **一份源谱 → 多个位宽变体** | 从 `isa/riscv64_v12.toml` 生成 RV32 变体（只读投影 MVP）；`forge-isa explain` 显示参数生效点；变体纳入生成物命名与覆盖登记 |
| G5 | **单一事实源**：旧计划退役、文档不漂移 | `docs/plans/` 不再有 v18 计划；`git grep -n forge-dsl-v18-plan` 全量指向 `docs/archive/`；本方案是唯一现行入口 |

## 2. 现状证据（本仓实测）

| 事实 | 数值/位置 | 说明 |
| --- | --- | --- |
| 生成物运行面 = **约 20 个宿主路径** | `docs/reference/isa-dsl.md`「生成代码依赖的运行面」；`machine/*` 共 2,387 行 | 生成物引用 `crate::machine::{abi,assembler,decoder,disasm,encoder,frame,inst,isa_info,lowering,pattern,reg_info,reloc_patcher,target}` + 顶层 `AllocResult/CodeSink/CompiledFunction/EncodeError/IrError/RelocKind/Registry/impl_erased_target_machine!` |
| **运行面 ↔ 管线互相引用** | `machine/encoder.rs:3,37` 用 `crate::AllocResult`；`machine/frame.rs:43` 用 `crate::pipeline::emit::LabelRef`；`machine/target.rs:139` 宏里 new `$crate::pipeline::compiler::FunctionCompiler` | CLAUDE.md「Assembler↔JIT 环」的真实形态；拆 runtime 必须先解这三处 |
| 各部分代码量 | `pipeline` 6,849 / `runtime` 3,429 / `machine` 2,387 行 | 生成物只需要 machine + 少数数据型 + 注册表 ≈ 3.5k 行（整机 12.7k 的 28%） |
| **谱里 0 个测试向量** | `isa/*.toml` 里 `bytes`/`expect`/`vectors` 键出现 **0** 次 | 黄金字节全在手写 Rust：`arm64_v12_tests.rs` 284 行/109 断言、`x86_v12_tests.rs` 1,159 行、`riscv64_v12_tests.rs` 389 行 |
| 生成物大头仍是**每条规则的发射体** | x86 lowering impl 611,642 B 中 C 段 334,020 B（54.6%，944 臂）；riscv 202,324 B；arm64 12,165 B | S8b-2 判定"要通用解释器，未做"（`docs/performance/bench_baseline.md`） |
| 规则/指令规模 | x86 197 lowering / 142 指令 / 3,518 行；riscv64 106/48/1,816；arm64 19/24/936 | arm64 lowering 只有 19 条 → 矩阵大量 skip |
| JIT 覆盖 | x86 195 pass/3 skip；riscv64 131/67；**arm64 23/175** | 本机 2026-09-23 实测（`MATRIX-SUMMARY` 事件）；skip 原因 = capability（op 未覆盖）/ value-range |
| 生成器已无 ISA 字面量 | `crates/frontend/forge-isa-dsl/tests/generality_guard.rs` 的 `ALLOWED = &[]` | 剩下的"不通用"是**宿主耦合**，不是硬编码 |
| CLI 现状 | `validate`/`insts`/`explain`/`diff`/`schema`/`fmt` 6 个子命令 | 无 `test`、无 `lint`、无严格档 |
| 新键的三方针守卫 | `tests/schema_guard.rs`：schema 表 ↔ `v12/model.rs` ↔ 文档键速查表 ↔ 签入的 `isa-dsl.schema.json` | 任何新键（`[[vectors]]`/`params`/`only_variants`）四处同改 |
| 工作区成员是显式列表 | 根 `Cargo.toml` `members` | 新增 crate 要显式登记 |

## 3. 外部设计参考（借什么、不借什么）

| 设计 | 关键机制（已核证） | 本方案借用 |
| --- | --- | --- |
| **QEMU decodetree**（[规范](https://www.qemu.org/docs/master/devel/decodetree.html)、[decodetree.py](https://gitlab.com/qemu-project/qemu/-/raw/master/scripts/decodetree.py)） | `%field`/`&args`/`@fmt` 打包复用、format/args **自动推断**、`{ }` 允许重叠 vs `[ ]` 禁止重叠、`--test-for-error` + `tests/decode/*.decode` 做**诊断回归**、连续位掩码优化成 `switch ((insn>>sh)&k)`、变长是"每位宽一份 decoder" | V3 的负向向量与诊断快照；V4 的"未指定位/位域重叠"检查；V5 的 per-变体生成物 |
| **LLVM TableGen**（[ProgRef](https://llvm.org/docs/TableGen/ProgRef.html)、[DecoderEmitter.cpp](https://github.com/llvm/llvm-project/blob/main/llvm/utils/TableGen/DecoderEmitter.cpp)） | `class→multiclass→defm + let`；`!locs` 给"实例 + 参与定义的 multiclass"多级位置；`-dump-json`；`-warn-on-unused-template-args`；解码表 = 字节码 + 解释器 | V4 的"模板未使用键"告警；`explain` 的完整 provenance 链；`--json` 面向外部工具 |
| **Cranelift ISLE**（[设计回顾](https://cfallin.org/blog/2023/01-20/cranelift-isle/)、[语言参考](https://raw.githubusercontent.com/bytecodealliance/wasmtime/refs/heads/main/cranelift/isle/docs/language-reference.md)） | **重叠必须显式 priority**（PR #4906/#5011/#5322 真抓出被完全遮蔽的规则）；`stablemapset` 避免迭代序污染生成物 | V6 的 `--strict-overlap`/`--warn-unreachable`；生成物**确定性守卫** |
| **Ghidra SLEIGH**（[手册](https://ghidra.re/ghidra_docs/languages/html/sleigh.html)、[p-code](https://ghidra.re/ghidra_docs/languages/html/pcoderef.html)、[#9576](https://github.com/NationalSecurityAgency/ghidra/issues/9576)、[InSPECtor](https://arxiv.org/html/2608.13042v2)） | p-code = **数据 + 通用解释器**（为了让分析器 retarget）；歧义检查 **opt-in** ⇒ 学界差分实测出 125 个唯一 bug | 反面教训：检查默认关会真出 bug → V6 档位**先量化再定**；V3 的"全字节空间健壮性/扰动"测试 |
| **GCC .md**（[Mode Iterators](https://gcc.gnu.org/onlinedocs/gccint/Mode-Iterators.html)、[Parameterized Names](https://gcc.gnu.org/onlinedocs/gccint/Parameterized-Names.html)、[genrecog 重写说明](https://gcc.gnu.org/pipermail/gcc-patches/2015-April/417287.html)） | 迭代器**逐值带条件**：`[SI (DI "TARGET_64BIT")]`；`@name<...>` 生成 `code_for_/gen_` helper；**没有编码往返校验** | V5 的"带条件变体列"，并把 `vary` 从 lowering 下沉到 `[[templates]]`；保住我们已有的 encode∘decode 往返优势 |
| **Sail / ASL**（[Sail POPL'19](https://doi.org/10.1145/3290384)、[asl-interpreter](https://github.com/alastairreid/asl-interpreter/blob/master/README.md)、[IntelLabs isa-tools](https://github.com/IntelLabs/isa-tools)） | 一份规范配置化出 RV32/RV64；golden model + RISCOF 差分 | V5 的参数化（xlen 式）；差分沿用现有 QEMU 通道；**不做形式化** |

**明确不借**：ISLE 的术语重写语言本身（会逼 `[[lowering]]`+`insts`+`vary` 全量重写）；纯"数据表 +
解释器"式 lowering（研究结论：表+解释器只在"数据被多消费者复用"或"解释器本来就存在"时划算，
与本仓 S8b-2 实测一致）；Sail/ASL 形式化；LSP（单独立项）。

## 4. v19 设计总览与破坏性清单

```text
                    ┌──────────────────────────────┐
   谱（TOML）        │ formats/slots/instructions/   │  ← 数据：编码 + 指令 + 谓词 + 变体参数
   isa/*.toml        │ templates(+vary)/vectors      │
                    └───────────────┬──────────────┘
                                    │ forge-isa-dsl（build script 预生成，v18 S10d）
                                    ▼
                    ┌──────────────────────────────┐
   生成物（$OUT_DIR）│ forge_gen_<模块>_<参数哈希>.rs │
                    └───────────────┬──────────────┘
                                    │ include!
      ┌─────────────────────────────┼──────────────────────────────┐
      ▼                             ▼                              ▼
 forge-isa-runtime          （宿主自己的管线）              任意第三方 crate
 = 生成物唯一依赖面            通过 Pipeline trait 注入         只需 runtime + 谱
   （machine/* + 数据型）      （forge-codegen 注册自己的）      （V2 的实证对象）
```

四层职责：**编码层**（forms/位域名/操作数槽，含 v18 宽度三态）→ **指令层**（`[[instructions]]` +
`[[templates]]` 唯一复用机制）→ **语义层**（`[[lowering]]`/`[[pattern]]`）→ **验证层**
（`__spec_tests` + 新增 `[[vectors]]`）。

### 破坏性清单（明确删除，不设兼容层）

| 删除/变更 | 影响面 | 替代 |
| --- | --- | --- |
| `isa_from_file!(…, krate = <宿主>)` 与 `krate` 语义 | `ExpandOptions.krate`、`rewrite_path_roots`、14 处调用点（3 发行后端 + `tests/common/mod.rs` 8 + `tests/spec_tests_v12.rs` 3）、`parts_selection.rs`/`tutorial_spec.rs` 等测试 | 生成物一律写绝对路径 `forge_isa_runtime::…`（宿主只需依赖 runtime） |
| `forge-codegen` 作为"生成物运行面"的提供者 | `docs/reference/isa-dsl.md` 运行面一节、`tests/library_surface.rs` | 新 crate `forge-isa-runtime` |
| `machine/*` 位于 `forge-codegen` 下 | `crate::machine::…` → `forge_isa_runtime::…`；`spec_coverage_guard`/`no_hardcoded_widths` 的路径假设 | `forge-codegen` 保留内部转发（不承诺给第三方） |
| 手写 Rust 黄金字节测试（发行 ISA 部分） | `*_v12_tests.rs` 的黄金表 | 谱内 `[[vectors]]` + `forge-isa test` |
| `[[lowering]]` 的隐式裁决（同优先级重叠靠声明序） | 400+ 条规则 | 默认保留，CI 开严格档 |

## 5. 切片计划

> 每片沿用固定门禁（见 §9）。**MVP = V0–V4 + V6 的确定性部分**；V5/V7 为按度量/可选。

| 片 | 范围 | 关键产出 | 证据/验收 | 叫停价值 |
| --- | --- | --- | --- | --- |
| **V0** 基线取证 | 生成耗时/规模、编译时间分档、覆盖缺口、运行面清单、黄金测试规模 | 本文 §10 基线表；机器可读"生成物运行面表"（V1 的迁移清单）；`ops.toml` 有而谱里无 lowering 的**缺口清单** | 数字可复现（命令写进文档）；缺口清单与矩阵 skip 对得上 | 立刻可用（给 V1/V4 定范围） |
| **V1** 拆 `forge-isa-runtime`（**破坏性**） | 新 crate：`machine/*` + `runtime/{output_types,registry}` + `AllocResult`/`CodeSink`/`LabelRef`；解三处 `machine→pipeline` 引用：数据型上移，管线走 runtime 的 `Pipeline` trait，`impl_erased_target_machine!` 移入 runtime | 生成物只依赖 runtime；`forge-codegen` 变成 runtime 下游（保留 JIT/regalloc/emission/arch） | `cargo tree -p forge-isa-runtime` 只含 forge-ir；新守卫 `runtime_has_no_pipeline_dep.rs`；三后端 930 条规格用例 + 黄金 + 矩阵 195/3/0、131/67/0、23/175/0 逐数字不变 | 通用性的**前置条件** |
| **V2** 外部宿主实证 | 新 crate（如 `crates/tools/isa-host-demo`）：仅 `forge-isa-runtime` + build-dep `forge-isa-dsl` + 自带玩具谱（`parts = ["encode","decode","asm"]`） | `krate` 参数删除；教程"新 ISA 从这里开始"改指本 crate | 该 crate `cargo test` 通过（含 `__spec_tests` + 向量）；`cargo tree` 无 forge-codegen；参数表只剩 `spec_tests`/`name`/`parts`/`params` | G1 的**硬证据** |
| **V3** `[[vectors]]` 数据化测试 | `{ asm, bytes }`、`{ asm, error = "<码>" }`、`{ bytes, error = "DECODE", partial = N }`；生成进 `__spec_tests`；新增 `forge-isa test <谱> [--json]` | 三 ISA 迁移黄金字节；Rust 侧只留集成/ABI/JIT 断言 | 迁移前后**逐字节等价**（脚本 dump 前后 sha256 比对）；手写测试行数 −≥50%（前后数字入库）；负向向量钉错误码 | G2；作者体验立刻变好 |
| **V4** `forge-isa lint` | 未用 `[[operand_slots]]`/`[[forms]]`/位域/`ref`；模板行键未被 `body` 或 `{…}` 引用；可合并为 `vary` 的族（只建议）；位域重叠/未指定位；能力缺口；死规则 | `lint` 子命令 + 错误码 + 三 ISA 清单快照 | 零误报（人工核对后钉快照）；`--json`；退出码与 `validate` 一致（0/1/2） | G3；写谱门槛下降 |
| **V5** 参数化变体（**破坏性**，MVP 只做只读投影） | `[meta].variants = { xlen = [32,64] }` + `params = { xlen = 32 }` + 逐指令/模板 `only_variants` + 值条件列 `vary` 下沉到 `[[templates]]` | RV32 从同一 riscv64 谱生成（投影：encode/decode/asm，不注册）；`explain` 显示参数生效点 | 与独立 RV32 表或 QEMU 32 位用例对拍；矩阵/覆盖守卫显式登记变体期望 | G4；臂/扩展式复用的验证 |
| **V6** 诊断严格度 + 确定性 | `validate --strict-overlap` / `--warn-unreachable`；生成物确定性守卫 | 默认档 = 现状（先量化噪音），CI 开严格档；同谱重复生成逐字节相同 | 严格档在三 ISA 上的新诊断清单 + 误报评估（含 `or`/`not` 的 Opaque 边界），数字入库 | 借 ISLE 抓"被完全遮蔽的规则"；借 SLEIGH 教训避免"默认关" |
| **V7** 语义层表化（**可选，默认不做**） | 只借"属性视图 + 生成期可分析性"；把 S8b-2 的通用解释器当候选 | 先量：V1 之后 x86 C 段 334 KB 是否仍是编译时间主因、表化净收益是否 ≥15% token 且不增编译时间 | 结论入库（含实测数字），无论做与不做 | 与 S8b-2 一致：以度量决定 |

**顺序与理由**：V0（定范围）→ V1/V2（通用性地基）→ V3/V4（作者可见收益，可独立交付）→
V6（低风险、抓真问题）→ V5（参数化）→ V7（按度量）。

**进度**：

- ✅ **V0 已落地（2026-09-23）**：数字与口径见 §10。三条影响后续设计的关键发现：
  ① 生成不是瓶颈（三谱 687 ms；改被扫描源文件后整个 `cargo check` 2.82 s，随后 fresh）；
  ② **终结指令不走 `[[lowering]].op`**（生成的 `lower_terminator` 按 `TermKind` 分派，
  谱里根本没有 `op = "Jmp"/"Br"/"Ret"`）⇒ V4 的 lint 必须把"终结指令/宿主管线处理/真缺口"
  分三类报，否则一上手就是上百条误报；③ runtime 迁移清单已锁定（10 个 `machine` 子模块 +
  9 个顶层项），两处硬耦合是 `pipeline::emit::LabelRef` 与 `prelude`。
- 🚧 **V1a 已落地（2026-09-23）**：生成物运行面拆成 **`crates/foundation/forge-isa-runtime`**
  （3,669 行：`machine/*` 的 trait 与数据型、`LowerCtx`/`MemRef`、`AllocResult`/`CodeSink`/
  `LabelRef`/`CompiledFunction`/`RelocKind`、`Registry`、`VCode` 数据型、CPU 能力探测、生成物
  `prelude`）。`forge-codegen` 降到 10,037 行并保留同名 re-export（内部路径不变）。新守卫
  `runtime_surface.rs`（依赖面只允许 forge-ir/smallvec/thiserror；源码不得出现
  `forge_codegen`/`crate::pipeline`），「反宽度写死」守卫按归属一分为二（runtime 12 条 /
  codegen 4 条，失效条目仍强制删除）。证据：`cargo tree -p forge-isa-runtime` 不含
  forge-codegen；`cargo test -p forge-codegen` **27/27 二进制全绿**；clippy `-D warnings` 干净。
  **V1b 未做**：生成物改指绝对路径 `forge_isa_runtime::…` 并删 `krate`/`rewrite_path_roots`；
  `impl_erased_target_machine!` 带 `pipeline = <路径>` 参数搬进 runtime（现在留在 codegen，
  因此第三方宿主暂时不能用 `tm` 部件——G1 的实证要等 V1b/V2）。

**V1b 采用「runtime 注册管线」方案（2026-09-23 用户拍板）**：

- 生成物**不再包含任何宿主路径**：生成器恒发
  `forge_isa_runtime::impl_erased_target_machine!(TargetMachine);`（宏签名回到单参数）。
- `forge-isa-runtime` 新增管线注册表：`FunctionPipeline` trait（方法 `compile_raw`）加
  `register_pipeline(isa, factory)`；按 ISA 名查表，未注册就返回 `IrError::Unsupported`（fail-closed）。
  工厂签名是「任意机器引用 → 可选管线对象」，因此运行面不需要知道具体机器类型。
- 宏里的 `compile` 走 `forge_isa_runtime::compile_via_pipeline(name, self, func)`（内部查注册表）。
- **宿主**（forge-codegen）在自己那侧为每个发行后端注册一次：`register_pipeline(ISA 名 + 工厂闭包)`，
  闭包里把传入的机器引用 downcast 成该后端的 `TargetMachine` 再交给 `FunctionCompiler`；
  夹具与测试侧注册同一份闭包。

- 于是生成物与宿主彻底解耦：`krate`、`pipeline` 参数、占位路径替换**都不需要**。

**V1b 执行清单（已勘察完毕，逐步照做即可；改动是原子的，必须一次全做）**：

1. `gen_file.rs`：`RawArgs.krate` → `pipeline: Option<syn::Path>`（键名 `pipeline`，错误消息与
   `generated_file_name` 的参数哈希同步改；`krate` 从支持列表里删掉=破坏性）。
2. `lib.rs`：`ExpandOptions.krate` → `pipeline: Option<String>`；`expand_file`/`expand_source`/
   `expand_loaded` 的 `krate` 形参换成 `pipeline: Option<&TokenStream>`；
   **`rewrite_path_roots(ts, krate)` 换成固定根改写 `rewrite_runtime_roots(ts)`**：
   `crate` → `forge_isa_runtime`、`forge_ir` → `forge_isa_runtime::ir`；两条单测随之改名
   （`rewrite_*_rewrites_only_path_roots` / `rewrites_forge_ir`）。
3. `v12/codegen/mod.rs`：`generate_with_parts(model, spec_tests, parts)` 增第三个参数
   `pipeline: Option<&TokenStream>`，透传给 integration。
4. `v12/codegen/integration.rs:867`：`crate::impl_erased_target_machine!(TargetMachine);` →
   `forge_isa_runtime::impl_erased_target_machine!(TargetMachine, #pipeline);`
   （`parts` 含 `tm` 而 `pipeline` 缺失 ⇒ **生成期报错**，消息给出 `pipeline = <宿主管线路径>` 的写法；
   `parts` 不含 `tm` 时该参数不需要）。
5. 宏搬到 runtime：`crates/backend/forge-codegen/src/erased_macro.rs` →
   `crates/foundation/forge-isa-runtime/src/erased.rs`，宏签名改成
   `($tm:ty, $pipeline:path)`，体内 `<$pipeline>::new(self.clone()).compile_raw(func)`；
   codegen 侧如需保留旧名可 `pub use forge_isa_runtime::impl_erased_target_machine;`。
6. 14 处调用点加参数：3 个发行后端用 `pipeline = crate::pipeline::compiler::FunctionCompiler`；
   `tests/common/mod.rs`(8) 与 `tests/spec_tests_v12.rs`(3) 用
   `pipeline = forge_codegen::pipeline::compiler::FunctionCompiler`（`krate` 一律删除）；
   只生成 `encode/decode/asm` 的调用点（如 `include_v12_tests` 的编码器变体）不写该参数。
7. 文档：`docs/reference/isa-dsl.md`（参数表 + 「宿主接入」+「生成代码依赖的运行面」改指
   `forge_isa_runtime`）、`docs/guides/isa-dsl-tutorial.md`、`CLAUDE.md`（Architecture Rules 1/3/4
   与 Testing Notes）、`CHANGELOG.md`（破坏性：`krate` 删除、运行面迁移）、本计划进度。
8. 门禁：`cargo test -p forge-isa-dsl -p forge-isa -p forge-dsl`、`cargo test -p forge-codegen`
   （27 二进制）、三架构 JIT 矩阵、`clippy -D warnings`、`fmt`、`cargo doc`、workspace 测试。

## 6. 迁移与文档落地

**v18 退役（已完成于 2026-09-23，D0）**：`docs/plans/forge-dsl-v18-plan.md` →
`docs/archive/forge-dsl-v18-plan.md`（加 ARCHIVED 头 + 指向本文）；`git grep -n forge-dsl-v18-plan`
全量更新为归档路径（9 个文件 14 处）；`docs/README.md` 的 plans 表行移入 archive 表并新增本文行。

**每片必做的文档动作**：

1. **三方针守卫**（新增/改动键时）：`src/schema.rs` ↔ `v12/model.rs` ↔ `docs/reference/isa-dsl.md`
   键速查表（`TABLE_BEGIN/END` 区段）↔ 签入的 `isa-dsl.schema.json`
   （`cargo run -p forge-isa -- schema --out isa-dsl.schema.json`）。
2. `docs/reference/isa-dsl.md`：新键小节 + 与 v18 的差异；`docs/reference/isa-dsl-errors.md`：新错误码。
3. `CLAUDE.md`：**只写已落地的事实**——文档地图随本文移动；Architecture Rules 与 Testing Notes
   在对应切片真正落地后才改（避免文档领先于代码）。
4. `CHANGELOG.md`：用户可见/破坏性变更（`krate` 删除、runtime crate、向量迁移）写成
   `[Unreleased]` 条目，含迁移三步。
5. `docs/performance/bench_baseline.md`：V0 基线与每片 A/B 数字（含"未做/不做"的判定与理由）。

## 7. 边缘情况与失败模式

| 风险 | 处置 |
| --- | --- |
| 拆 runtime 漏项（生成物引用到未迁类型）→ 三后端同时炸 | 用 V0 的"运行面清单"驱动迁移；先把 `machine→pipeline` 三处改完再搬文件；每步跑全套门禁 |
| 守卫路径假设失效：`spec_coverage_guard`（钉总数 + 按 `lib.rs` 首个 `#[cfg(test)]` 截断）、`no_hardcoded_widths`、`library_surface` | 逐条重指；`library_surface` 改为钉"runtime crate 不含 demo 谱/不含 arch"；验证"条目必须被命中"仍有效 |
| 生成物改名/迁移留下 OUT_DIR 孤儿文件、RA 需重载一次（S10d 已知） | 写进 `docs/guides/rust-analyzer-notes.md`；不自动清理（无害） |
| `[[vectors]]` 迁移悄悄改语义 | 迁移脚本 dump 迁移前 Rust 黄金表 → 与谱内向量逐条 sha256 比对；负向向量必须带错误码 |
| V5 变体放大构建/守卫乘数（×2 变体 = 生成物/注册表/矩阵/覆盖登记翻倍） | MVP 只做"只读投影（不注册 tm）"；`params` 纳入生成物命名哈希；覆盖登记显式列期望值，禁止静默 skip |
| `--strict-overlap` 误报（判定域为闭区间近似，`or`/`not` 记 Opaque） | 先只统计不阻断；误报数字入库后再决定默认档 |
| 删除 `krate` 的影响面被低估 | 落地前 `git grep -n krate` 全量列清单逐处迁移；参数表同步 |
| 宿主环境抖动（`link.exe` 0xC0000409、RA 读到中间态） | 门禁串行 `-j 1`；失败单点重试并如实记录，不当作代码缺陷 |

## 8. 显式假设

1. **允许破坏性更新、不做兼容层**：`krate`、`rewrite_path_roots`、`forge-codegen` 作为生成物运行面
   的角色都可删。
2. **不引入新第三方依赖**：`lint`/`test` 续用手写参数解析与手写 JSON；LSP 单独立项。
3. **不做形式化**：只借"权威语义 + 差分对拍"；riscv 侧继续用 QEMU 真执行当 oracle，x86/arm64 只测
   不变量（不假装有 oracle）。
4. **`forge-codegen` 继续是整机管线**（JIT/regalloc/emission/三后端），只把生成物运行面拆出去；
   Assembler↔JIT 的环通过 runtime 的 `Pipeline` trait 钩子解开。
5. **发行 ISA + 6 个夹具是验收载体**：任何改动都要在 x86/riscv64/arm64 上全绿（含 930 条生成期规格
   用例与全部黄金字节）。
6. **只借机制、不借语言**（ISLE 术语重写、Sail/ASL 形式化、纯表解释式 lowering 均不做或按度量决定）。

## 9. 验收门禁

```bash
cargo fmt --all -- --check
cargo clippy --workspace --exclude forge-rustc --all-targets --all-features -- -D warnings
cargo test -p forge-isa-dsl -p forge-isa -p forge-dsl
cargo test -p forge-codegen -j 1 -- --test-threads=1
cargo test -p forge-tests --lib jit_matrix_x86_v12
cargo test -p forge-tests --lib jit_matrix_riscv64_v12 -- --test-threads=1 --nocapture
cargo test -p forge-tests --lib jit_matrix_arm64_v12 -- --test-threads=1 --nocapture
cargo check --workspace --exclude forge-rustc --release --all-targets
RUSTDOCFLAGS="-D warnings" cargo doc --no-deps --all-features
cargo test --workspace --exclude forge-rustc -j 1 -- --test-threads=1
npx markdownlint-cli2 <改动文档>
```

每片结束：提交（中文 conventional commit，写清实测数字）→ `git push origin-ssh HEAD` →
用 `/commits/<sha>/check-runs?filter=latest` 核验 CI；未跑完不当成功报。

## 10. 附录：v19 S0 基线

> 采集日期 **2026-09-23**，本机（Windows x86_64，nightly-2026-09-05）。脚本：`target/v0_metrics.py`
> （op 覆盖 + 测试规模 + 运行面）、`target/v0_op_gap.py`（缺口原始清单）、临时用例
> `cargo test -p forge-isa-dsl --test zz_v0_metrics -- --nocapture`（规模/耗时，跑完已删）。
> 口径：字节数 = `expand_file` 产出的 token 文本长度（= build script 落盘件正文，v18 S10d 起的紧凑打印）。

### 10.1 生成物规模与生成耗时

| ISA | 全部件 + `spec_tests` | 全部件 | 无 `tm`（enc+dec+asm） | enc+dec | 生成耗时（全部件+spec_tests） |
| --- | ---: | ---: | ---: | ---: | ---: |
| x86 | 1,336,624 | 1,009,125 | 576,413 | 423,292 | 371 ms |
| riscv64 | 722,311 | 508,202 | 237,771 | 127,905 | 184 ms |
| arm64 | 523,816 | 310,437 | 206,101 | 107,080 | 132 ms |

三个发行谱合计 **687 ms**；`forge-codegen` 全量预生成（14 处调用点含夹具变体）在 build script 里
完成，实测改一个被扫描源文件后整个 `cargo check` 只用 2.82 s（见 10.2）。

### 10.2 编译时间分档（`cargo check -p forge-codegen`）

| 场景 | 耗时 |
| --- | ---: |
| 冷（`cargo clean -p forge-codegen` 之后） | **57.0 s** |
| 增量、无改动 | 23.8 s |
| 改一个**被扫描的源文件**（build script 重跑 + 14 份重新生成 + 编 lib） | **2.82 s** |
| 紧随其后（应 fresh） | 0.45 s |

⇒ 生成（含落盘）不是瓶颈（<1 s），冷启动时间由 rustc 解析 1.3 MB 生成物 + 编译 crate 主导；
**无重编循环**（第 4 次 0.45 s 证明）。

### 10.3 能力缺口（116 个 op 分四类 —— 这是 V4 lint 的口径基础）

| ISA | 谱 `[[lowering]]`/`[[pattern]]` 覆盖 | 终结指令（生成的 `lower_terminator` 按 `TermKind` 分派） | 宿主管线处理 | 真缺口 |
| --- | ---: | ---: | ---: | ---: |
| x86 | 100 | 6 | 7 | **3**（`AddrSpaceCast` `Resume` `VaArg`） |
| riscv64 | 61 | 6 | 7 | **42**（浮点/向量/指针转换一整套） |
| arm64 | 8 | 6 | 7 | **95**（几乎全部整数/浮点/内存 op） |

**V0 的关键发现（纠正了"全体都缺 16 个 op"的粗口径）**：

1. **终结指令不走 `[[lowering]].op`**：生成器为每个 ISA 生成 `lower_terminator`，按 IR 的
   `TermKind`（Return/Jump/Branch/Switch/Invoke）分派；谱里因此根本**没有** `op = "Jmp"`/`"Br"`/
   `"Ret"`，`forge-codegen/src` 里也没有 `Opcode::Jmp`/`Opcode::Br`（零出现）。把
   `Br/Jmp/Ret/Switch/Unreachable/Invoke` 算成"缺口"是**误报**。
2. **7 个 op 由宿主管线直接处理**（`Bitcast`/`Call`/`CallIndirect`/`ExtractValue`/`InsertValue`/
   `GetElementPtr`/`LandingPad`，宿主里有 `Opcode::X` 直查）——也不是谱侧缺口。
3. 真正的缺口只有 **`AddrSpaceCast`/`Resume`/`VaArg`**（宿主与谱都没有）+ 各 ISA 自己没做的
   lowering（riscv 42 / arm64 95，属**谱侧工作量**而非 DSL 缺陷）。
4. 因此 **V4 的 lint 必须把三类分开报**（终结指令 / 宿主管线 / 真缺口），否则一上手就是
   上百条误报。

### 10.4 生成物的宿主依赖面（V1 的迁移清单）

从最大生成物（`forge_gen_x86_v12_28fd384c3571d4ee.rs`，1,336,872 B）机械抽取：**183 条**不同的
`crate::…` 路径，归约为

- **`machine` 子模块 10 个**（29 条具体路径）：`abi`（`FrameLayout`/`FrameLayoutKind`/`TargetABI`）、
  `assembler`（`AsmError`/`TargetAssembler`）、`decoder`（`DecodeError`/`TargetDecoder`）、
  `disasm`、`encoder`、`frame`、`inst`（`OperandConstraint`）、`isa_info`
  （`IsaCapabilities`/`IsaInfo`/`RegisterClassInfo`）、`lowering`、`pattern`
  （`PatPred`/`PatTerm`/`PatCmp`/`PatternSpec`）、`reg_info`（`TargetRegInfo`/`class_for_type_in_pool`）、
  `reloc_patcher`、`target`（`TargetMachine`）。
- **顶层 9 项**：`AllocResult`、`CodeSink`、`EncodeError`、`IrError`、`Registry`、`RelocKind`、
  `impl_erased_target_machine!`、`pipeline`、`prelude`。

**两处硬耦合（V1 必须先解）**：① 编码器里的重定位写回直接用
`crate::pipeline::emit::LabelRef`；② 大量类型经 `crate::prelude::…` 引用（`EffectKind` 等），
说明 runtime 必须自带 `prelude` 与 `LabelRef`/`CodeSink`/`AllocResult` 这几个数据型。

### 10.5 手写测试规模（V3 迁移对象）

`crates/backend/forge-codegen/tests/*.rs` 合计 **5,683 行 / 626 条断言**；承载黄金字节的主要是
`x86_v12_tests.rs`（1,160 行/22 断言，黄金表是数据）、`arm64_v12_tests.rs`（285/109）、
`riscv64_v12_tests.rs`（390/27）、`v12_integration_tests.rs`（353/49），以及 6 个夹具文件
（`demo*`/`include_v12_tests.rs`，合计 ≈1,120 行）。V3 的目标是"发行 ISA 的黄金字节进谱"，
夹具与集成/ABI/JIT 断言留在 Rust。
