# forge-rustc v27 方案：从"能跑的部分后端"到**完整 rustc 后端**

> **状态：[progress]**（2026-10-11 立项）。本文是**活账本**：每个里程碑落地时在 §8 追加批次记录
> （含实测数字、命令、偏差），任何"说得出但没做到"的东西必须如实写在这里。
> 基线 = v26 收口提交 `9b2cc124`；**行号随迭代漂移，以符号名为准**。
> 上一轮方案（v26，已收口归档）见 [`docs/archive/project-v26-plan.md`](../archive/project-v26-plan.md)；
> 后端契约 [`reference/backend-pipeline.md`](../reference/backend-pipeline.md)、
> 能力缺口台账 [`reference/backend-gaps.md`](../reference/backend-gaps.md)。
>
> 用户给定的口径（本轮**放宽**于以往）：**允许破坏性更新、无需兼容旧版本、无冗余**。
> 因此本方案的原则是 **"绕法必须消失"**：工作区里每一条 `[WA-NN]` 要么被正确实现取代、
> 要么被明确判为"不做"并写清数据条件——不允许长期挂在"已知限制/容忍名单"里。

前提纪律（沿用）：**一个任务一次提交**；**里程碑未完成不提交**；每个里程碑以 `cargo forge gate`
的全量门禁 + 该里程碑**新增守卫**为验收；**删除清单逐条清零**；预算/棘轮只许按实测下调；
用户可见变更进 `CHANGELOG.md`；契约与账本随代码同步回写。

## 1. 立项证据（本次只读审查实测，2026-10-11）

| # | 证据 | 实测 |
| --- | --- | --- |
| E1 | **形态与体量** | `crates/tools/forge-rustc` = 21 个源文件 / **约 8.7k 行**：`dwarf.rs` 2021、`lower/` 3700+、`backend.rs` 982；`crate-type = ["dylib"]`（`-Zcodegen-backend` 后端）；`tests/e2e.rs` 2543 行、`tests/determinism.rs` 593 行 |
| E2 | **已有能力面**（README §支持矩阵，逐条可核） | 标量算术/控制流/直接调用/递归/Windows x64 调用约定（含 shadow space、sret、浮点 XMM0-3）/FFI/聚合/枚举（含 niche）/`__rust_alloc` 全家/static 段/Box/闭包/Drop/`dyn` 虚调用（vtable 表）/DWARF5 C1+C2（gdb 16.2 实证）/多 CGU 并行 + 每 CGU 独立对象 + `-C incremental` |
| E3 | **e2e 现状（代码为真）** | `CASES` **105 条**；`known_failure: true` = **1**、`false` = 106；`expect_compile_fail: true` = **2**；`overflow-checks=off` 仍出现在 **6** 处（含默认编译路径 `e2e.rs:1584`） |
| E4 | **文档与代码已经分叉** | README §支持矩阵 仍写「**103 用例：98 硬断言 + 5 个 `FLAKY` 容忍项**」，而 `WORKAROUNDS.md` 的 WA-11 记「2026-09-12 五例全部转正、`FLAKY` 清空（CI run 93927221004 `known=[]`）」⇒ 现状是 **1 个 `known_failure`**。**没有任何守卫**钉这两侧 |
| E5 | **正确性类开放绕法**（`WORKAROUNDS.md` —— ⚠️ **本条在 M1 逐条核实后被推翻，见 §8**） | `WA-05` 常规 `+`/`-`/`*` 的溢出 Assert 未覆盖（测试统一关 `overflow-checks`）；`WA-36` 窄 signed 值 32 位符号扩展缺口（`i8` load 零扩展 ⇒ -50 读成 +206）；`WA-40` def 落在**写死物理寄存器**的指令上被 spill ⇒ 静默写坏槽；`WA-41` 可确定性复现的错编（`Vec::new(); v.push(1)` 后读 `APTR[0]`）；`WA-46` FPR/向量溢出恒按 8 字节搬运 + 64B 值类宽只有 32 ⇒ **向量 spill 静默截断** |
| E6 | **对象/链接层绕法** | `WA-01` MSVC 链接器不应用 `.rodata` 的 ADDR64 重定位（实测全 0）⇒ vtable 指针表被迫放 `.data`；`WA-07` 靠 `/DEFAULTLIB:vcruntime.lib` 提供 `__CxxFrameHandler3` |
| E7 | **缺能力三类** | ① `global_asm` 无实现（`WA-03`，直接报错）；② **内联 `asm!` 完全没做**（全仓 `inline_asm` 命中 **0**）；③ `panic=unwind` **明确拒绝**（`backend.rs:73-79`；`eh_personality` 命中 0、`.xdata` 命中 0），TLS 实质没有（`tls`/`thread_local` 各 1 处提及） |
| E8 | **冗余** | `WA-02`：rustc 的 alloc shim 实例与手写的 `alloc_runtime.rs`（401 行）**重复定义**（LNK2005）⇒ 靠 `keep_item` 过滤掉 rustc 侧实例维持"一份" |
| E9 | **门禁覆盖缺口** | `cargo forge gate` 对 rustc 只有 `check -p forge-rustc`（`gate.rs` 无 e2e 步骤）；**105 用例的 e2e 只在 CI 作业 `forge-rustc-e2e` 里跑** ⇒ 本地"全套门禁通过"并不覆盖后端真值 |
| E10 | **工具链锚点** | `rust-toolchain.toml` = `nightly-2026-09-05`（CI 注释：更早的 1.99 编不过）；rustc 侧 API 漂移是持续成本（`WA-34`/`WA-38`/`WA-39` 全是"随 nightly 适配"类） |
| E11 | **目标面单一** | 只在 **x86_64-pc-windows-msvc 宿主**验证；`arch/{arm64,riscv64}` 只有指令覆盖，rustc 后端未验证 |

**一句话**：今天的 forge-rustc 是"**在一条被绕法撑起来的路线上跑通了 105 个用例**"；
要成为"彻底的后端"，缺的是四件事：**把绕法换成实现**（E3/E5/E6/E8）、**补三类语言能力**（E7）、
**让门禁真的覆盖后端**（E9）、**文档与代码不许再分叉**（E4）。

## 2. 里程碑

> **状态总表（2026-10-11 校准；事实以代码/e2e/台账为准，§8 是逐轮记录）**：
> 本表只回答"现在到哪了"，判据的细节在各期正文与 §8 里。e2e 用例数**不写死**——
> 以 `crates/tools/forge-rustc/README.md` 的机读计数为准（`rustc_doc_counts` 守卫钉住）。

| 里程碑 / 切片 | 状态 | 证据落点 |
| --- | --- | --- |
| **P1 128 位 C-ABI 跨后端兼容**（2026-10-23 起） | ✅ **已落地（2026-10-24）**：定义侧/调用侧都问 rustc `FnAbi` 的 `PassMode`（按引用 ⇒ 16 字节对齐副本的指针；`Cast{Vector}` ⇒ `XMM0`；`Direct` ⇒ 两整数槽/`RAX:RDX`）；跨后端探针**双向** 42 | §2.8；`WA-52`（已修）；`tests/cross_abi.rs` |
| **P2 `INLINE_ASM` 的 `sym`/`label`** | ✅ **已完成（2026-10-24）**：`sym` 走"地址物化到输入寄存器"（M6-93），`label` 走"目标块 → 外部标签表 → 编码器 `use_label_at`"（M6-94，x86_64 变长谱；定宽谱明确拒绝）；正反两面的 e2e 都钉住 | §2.6 进度；台账 `INLINE_ASM` |
| **P3 M4-T3 第二目标** | ✅ **对象级 + 执行级都已落地（2026-10-24）**：对象级 = A64 编码逐字节对照 clang + 真实 ELF64/AArch64 对象核对（M6-92）；执行级 = arm64 `Call`/`CallIndirect` 角色（BL/BLR）+ `LDURX/STURX` 的 stack_arg 角色 + qemu 模块执行（M6-97），并据真机执行把 aapcs64 的 `first_offset_slots` 从 2 校正为 0。**rustc→aarch64 首光**（M6-98）：`--target aarch64-unknown-none` 的 no_core crate 编出 ELF64/AArch64 对象、clang crt0 + ld.lld 链接、QEMU 真执行（`pick(42,7)=42`）。**仍缺**：aarch64 的 **core/std**（仅 no_core 可用）与更大 opcode 面（Icmp/Select/移位/除法…） | §2.7；`tests/arm64_clang_oracle.rs`；`tests/arm64_exec.rs`；`exec/qemu_aarch64.rs`；`arm64-oracle` / `arm64-rustc-exec` 门禁步 |
| **P4 分配器事件化** | ✅ **已完成（2026-10-24）**：`SPILL-VALUE`（逐值）+ `SPILL-EDIT`（逐指令 kind/offset/scratch/width）+ `ALLOC-SUMMARY.peak_live`（寄存器压力峰值）+ `Field::I64`；`events_guard` 用**高压夹具**钉住三种粒度都真触发 | §8 M6-87/M6-90/M6-91；`events_guard` 三方双向 |
| M1 正确性清零（溢出 / 窄有符号 / 向量溢出 / regalloc） | ✅ 落地 | §8 M1；e2e 硬断言、`known_failure` = 0 |
| M2 `global_asm!` | ✅ 落地 | §8 M2；e2e `global_asm_call` / `global_asm_sym_const` |
| M2 展开（`panic=unwind` / SEH） | ❌ **不做**（明确拒绝） | 台账 `UNWIND` |
| M2 TLS | ✅ 落地（动态 TLS） | §8 TLS；e2e `thread_local_roundtrip` |
| M2.5 S1 内联 asm 骨架 | ✅ 落地 | §8 S1；e2e `inline_asm_nop` / `inline_asm_ud2` |
| M2.5 S2a 寄存器约束（`reg`） | ✅ 落地 | e2e `inline_asm_reg_inout` |
| M2.5 S2b 显式寄存器名（含短名映射） | ✅ 落地 | e2e `inline_asm_named_reg`；`reg_name_table` 守卫 |
| M2.5 S3 `const` 占位符 | ✅ 落地 | e2e `inline_asm_const` |
| M2.5 S3 `sym` 占位符 | ❌ **受 ISA 限制**：本谱汇编模板表达不了符号操作数 ⇒ 保持 fail-closed | §8 S3 记录 |
| M2.5 S4 `clobber_abi` | ✅ 落地（纯 clobber 槽） | e2e `inline_asm_clobber_abi` |
| M2.5 S4 `options(noreturn)` | ✅ 落地 | e2e `inline_asm_noreturn` |
| M2.5 S4 `label` 操作数 | ❌ **结构性阻塞**：标签必须落在指令流中间，当前"文本 → 一次汇编 → 一串指令"表达不了 | §8 的阻塞分析 |
| M3-① alloc 按需注入 | ✅ 落地 | §8；`WA-02` 关闭 |
| M3-② 真跑 `std` | ✅ **已跑通（2026-10-18，本机离线）**：`std` 由 LLVM 编、我们的 crate 由 forge 编；`println!` / `String` / `Vec` / `env::args` / `HashMap` 五档全绿 | `examples/forge-rustc-std-hello/run-offline.ps1` |
| M4-T1 目标校验（不再静默回落） | ✅ 落地 | 台账 `TARGET`；§8 M4-T1 |
| M4-T2 约定按目标选 | ✅ **实测已成立**（双向守卫） | `abi_target_real` 的 `abstract_c_…` |
| M4-T3 目标相关对象层 / 链接层 | ❌ 未做（需交叉工具链验证） | §8 M4 施工图 |
| M4-T4 对象层根治（`WA-01`） | ✅ 首结论：**按设计关闭**；剩余项见 §8 | `WORKAROUNDS.md` `WA-01` |
| M5 契约与门禁 | ✅ **已完成（2026-10-24）**：门禁/子集骨架、`i128`/`u128`、浮点比较语义（96 判定矩阵）、`sym`/`label` 全部闭环，`known_failure=0`；未覆盖形态一律用 `[cap:…]` 明确拒绝 | 下方清单 + 各里程碑记录 |
| M6-① 清理 | ✅ **已完成（2026-10-18）**：`forge-rustc` clippy 清零并进全量门禁 | `gate` 的 `rustc-clippy` 步骤 |
| M6-② 账本归档 + 参考全扫 | ✅ **已完成（2026-10-19）**：本文收缩成"状态表 + 未闭环清单" | `docs/archive/forge-rustc-v27-milestones.md` |
| M6-③ 代码生成选项 fail-closed | ✅ **已完成（2026-10-19）**：靠 LLVM 管线的 `-C` 选项统一拒绝 | 台账 `CODEGEN_OPTS` |
| M6-④ IR 优化管线接线 | ✅ **已完成（2026-10-19）**：按 `-C opt-level` 默认开启，O2 与 LLVM 持平 | 台账 `CODEGEN_OPTS`；archive M6-④ |
| M6-⑤ 契约面 fail-closed 扩展 | ✅ **已完成（2026-10-19）**：`CALL_CONV`（非 Win64 约定拒绝；此前 `sysv64` 被当 C 打包）、`VARARGS_DEF`（C 变参定义拒绝；此前会让 rustc ICE）、`EMIT_KINDS`（`asm`/`llvm-ir`/`llvm-bc` 拒绝；此前"退出码 0 但没产物"） | 台账三行 + e2e `neg_sysv64` / `neg_c_variadic` / `flag_emit_asm` |
| M6-⑥ 优化档 MIR 形态覆盖 | ✅ **已完成（2026-10-19）**：e2e 新增 `opt_*` 机制（注入 `-C opt-level=2`），一次抓出 6 个只在优化档出现的真缺陷并全部修复 | e2e 8 条 `opt_*`；archive M6-⑦/⑧ |
| M6-⑦ 活区间正确性 | ✅ **已完成（2026-10-19）**：两处区间**空洞**（同块定义→使用、跨块只标两点）修成正区间；调用点接入**约定级破坏集**（此前 `Call` 一条 clobber 都没有） | archive M6-⑫ |
| M6-⑧ 不变量守卫 | ✅ **已完成（2026-10-19）**：分配重叠自检、字段来源检查、溢出槽初始化探针 + 机器级 dump/隔离开关 | `regalloc_bt/verify.rs`、`pipeline/field_check.rs`、`pipeline/spill_init.rs` |
| M6-⑨ `-o` 与产物种类语义 | ✅ **已完成（2026-10-19）**：`--emit=obj -o <path>` 此前静默不生效 ⇒ 复制到 rustc 请求的路径；`--out-dir` 形态保留固定名 | `tests/object_output.rs`；archive M6-⑭/⑱ |
| M6-⑩ 已知后端缺陷（**已闭合**） | ✅ **已完成（2026-10-24，M6-95）**：两条同源症状的根因都定位到**帧记账**——`callee_saved_bytes` 把 fp 保存槽算进"rbp 之下"，spill 槽 0 落在 `rsp - 8`，`call` 的返回地址恰好压在那里；循环内一调用就把槽写成代码地址，reload 当基址用 ⇒ AV（"第一次迭代对、第二次起坏"）。修法：`callee_saved_bytes` 只算 rbp 之下的 push（fp 保存槽在 `[rbp]`），并经 `AllocResult.callee_saved_bytes` 让 emission 与 `@move_args` **同源**；`stack_slot_shift` 保持原口径。`StackAddr` 外提随之回归默认，`FORGE_LICM_STACKADDR` 门控删除；守卫 = `outside_spill_base_excludes_the_frame_pointer_slot`（帧账单测）+ `stack_addr_is_hoisted_by_default`（LICM 单测）+ e2e `opt_loop_call_inline`（O2、循环内调用、真执行 30） | §2.7；archive M6-95 |
| M6-⑪ 宽/窄整数溢出语义 | ✅ **已完成（2026-10-19）**：窄（8/16 位）加/减/乘的溢出标志不再依赖 x86 的 CF/OF——由 **legalize 的新 pass `narrow_overflow`** 统一改写成"扩宽算 + 比较区间 + 截回"（`forge-rustc` 里那份等价实现已删，语义只剩一处）。`-Coverflow-checks=on` 在**真 std 语料**上 O0/O2 都与 LLVM 逐行相同 | e2e 5 条 `oc_*`；JIT `jit_uadd_overflow_flag_is_correct`；archive M6-㊽…53 |
| M6-⑫ `()`/ZST 返回崩溃 | ✅ **已完成（2026-10-19）**：`TerminatorKind::Return` 对零大小返回类型此前发 `Load(void)` + 带 1 个操作数的 `Ret` ⇒ IR 校验 Internal error（`core::intrinsics::cold_path` 必崩，任何开溢出检查的 crate 都会撞上） | archive M6-㊴ |
| M6-⑬ 门禁自洽与真 std 语料 | ✅ **已完成（2026-10-19）**：`cli_tests` 夹具改用工作区 `CARGO_HOME` + `target/cli-tests`（沙箱可写）；`rustc-std` 步骤进全套门禁（forge 编 vs LLVM 编逐行对拍，默认 O0/O2）；全套 **51 步 0 失败** | archive M6-㊱/㊲ |
| M6-⑭ `cargo test` 能力 | ✅ **已实测（2026-10-19）**：`cargo test -Z panic-abort-tests` 在 forge 编的夹具工程上跑通（默认 `cargo test` 因 libtest 要 unwind 而与 `[cap:UNWIND]` 冲突，属明确拒绝而非缺陷） | README「用法」；archive M6-51 |

**M5 清单（2026-10-11 立项；2026-10-24 全部销项——原标注的「M5 未完成，不提交 PR」已失效）**：

- ~~NaN 比较缺陷~~ **已修复**（`Ne` → `FloatCC::Une`；`float_cmp_semantics` 钉住）；
- ~~同类仍可能有遗留：「下一步要在 `Fcmp` 发射路径上实测 `cond` 取值，别再靠枚举序推断」~~
  **已实测清零（2026-10-18）**：JIT 矩阵 `test_jit_fcmp_all_conditions_matrix` 覆盖 **16 个
  `FloatCC` × 6 种操作数组合（含 NaN 在左/右/两侧）= 96 个判定**，oracle 用 Rust 自身语义。
  这一测**真的抓到缺陷**：谱里 `Fcmp` 的"两个 setcc 组合"形态（`oeq/olt/ole` 与
  `ugt/uge/une` 六个条件）在 `SETCC_F` 之后直接 `and/or`，而 `SETCC` 只写低 8 位 ⇒ 组合用的
  临时寄存器 `{g}` 高位是垃圾，结果不再规范化（实测 `Ugt(1,2)` 得 `-118417152`）。
  修法：`isa/x86.toml` 的形状 B/C 在 `COMIS*` **之前**补 `xor {g}, {g}`（`xor` 清标志，
  必须放在比较之前）。e2e 另加严格数值用例 `float_cmp_normalized_bool`（把每个比较 `as i32`
  累加，truthy 判断会掩盖这一类缺陷）⇒ **e2e 165/165**；
- ~~`i128`/`u128` 仍是 `[cap:WIDE_INT]` 明确拒绝~~ **已闭环（2026-10-18）**：`map_type` 返回真 `i128`，
  后端 `legalize::wide_split` 用**多寄存器表示**（n 个原生宽 SSA 值）拆解；任意位宽（含 `i100`、
  `i256`）都有 JIT 验证，e2e 全量（`u128_*`/`i128_*` 在内）。未覆盖形态（宽值 phi、除/余）
  明确 `[cap:WIDE_INT_SHAPE]`；
- ~~`label`/`sym` 仍 fail-closed（前者受"指令流内标签"的结构限制，后者受谱汇编器文法限制）~~
  **已闭环（2026-10-24，M6-93/M6-94）**：`sym` 走"符号地址物化到输入寄存器 + abs64 重定位"，
  `label` 走"目标块 → 外部标签表 → 编码器 `use_label_at`"；两者都有正反两面的 e2e 守卫，
  字面符号写法仍显式 fail-closed（`inline_asm_sym_literal`）。
- **零容忍口径现在是"实测过的 0"，不是"挡在套件外的 0"**：`known_failure: true` 计数 = 0，
  且新加的浮点条件矩阵是**逐条件**断言（不是代表性抽样）。

⇒ **M5 清单已清零（2026-10-24）**：上列每一条都有"修复 + 守卫/用例"的落点，且全量门禁连续为绿。

## 2.6 P2 决策：`INLINE_ASM` 的 `sym` / `label`（2026-10-23）

现状（见 `docs/reference/backend-gaps.md` 的 `INLINE_ASM` 行）：S1–S4、`sym`、`label`
**已全部落地**（`sym` M6-93、`label` M6-94）；两者**没有**按当时预估去扩谱面操作数文法，
而是分别用"地址物化到输入寄存器"与"外部标签表 + 编码器既有 `use_label_at`"完成。
字面符号写法仍显式 fail-closed（`inline_asm_sym_literal`）。下面是当时的决策记录（保留）。

两条都**不是**"在降级层补一段字符串拼接"能解决的，它们要动的是**谱面汇编器**这一层：

| 操作数 | 缺什么 | 正解（按层） | 成本 |
| --- | --- | --- | --- |
| `sym` | 谱面汇编器的操作数文法只吃寄存器/数值字面量（`[operand.*]` 的 `kind = imm/mem/gpr/...`），符号没有对应的槽 | 新增操作数种类 `[operand.sym]`（`kind = "sym"`）+ 汇编器把标识符 token 绑到该槽 + 该槽带 `reloc = "abs64"`（x86 已有 `MOVABS_GLOBAL` 可承接）+ 降级层把 `InlineAsmOperand::Sym` 渲染成符号文本并登记重定位 | 中（DSL 模型/解析/生成 + 一处降级层 + e2e） |
| `label` | 标签必须落在**指令流中间**，而现在的形态是"文本 → 一次汇编 → 一串指令" | 汇编器支持**块内标签**：文本里 `L: ... jmp L` 在**同一次汇编**里解析成相对偏移（不需要跨 IR 块的 `LabelRef`）；rustc 的 `label` 操作数 `{0}` 只要绑定成"本块内的标签 id"即可 | 中（汇编器加标签表 + 分支绑定） |

**进度（2026-10-24，M6-93）——`sym` 用更小的落法完成**：谱面汇编器的操作数文法**不用动**。
降级层把符号**地址**物化成一个 IR 值（函数：`fn_addr_value`；静态：`static_addr_value`；
都走隐藏全局符号表 + `GlobalAddr` 的 `abs64` 重定位），并以普通**输入寄存器**（约束 `i,g`）
承载；模板占位符就是那个寄存器。因此：

- 覆盖 `mov {0}, {1}` / `call {0}` / `jmp {0}` / `[…{0}…]` 等**寄存器形态**；
- 把 `sym` 当**字面符号**用的写法（`[rip + {0}]`、`offset {0}`）会被汇编器拒绝
  ⇒ 编译期明确报错（fail-closed，不静默错码），由 e2e `inline_asm_sym_literal` 钉住；
- 同时修了 inline asm 定寄存器的一个真缺陷：**池选择必须避开所有显式名**（此前
  `sym` 可能先挑走 `RAX`，随后显式 `out("rax")` 被判成重复），且"纯 clobber 与值槽同名"
  改为合法（LLVM 的 `={rax},~{rax}` 同形）——`inline_asm_sym_call` 钉住。

`label` 的落法（2026-10-24，M6-94）——**不需要改谱面操作数文法，也不需要动发射器**：
rustc 给的是 `targets[target_index]`（目标 MIR 块），降级层把它换成"文本里唯一的标签名 +
外部标签表项 `(名字, IR 块下标)`"（作为 IR 立即数对跟着文本走）；汇编器 `parse_insts` 在
文本标签表里查不到该名字时回退到外部表，把**块下标**写进 label 槽；指令编码器本来就会对
label 槽 `use_label_at`（`LabelRef::from_id(rel)` → 相对位移），于是分支自然落到目标块。
连带修了两处真问题：① 可达性分析此前只看 `targets[0]`（fallthrough），label 目标块会被
判不可达 ⇒ 没有 IR 落点；现在 `targets` 全推入；② 外部标签的值是"块下标"，若谱的 label
槽带 unit（A64 的 4 字节单位）回填会乘 unit ⇒ 生成物对这类谱**明确拒绝**（x86_64 变长谱
unit = 1，正常支持）。正反两面 e2e：`inline_asm_label`（无条件命中）、
`inline_asm_label_cond`（`jcc e` 命中）、`inline_asm_label_cond_skip`（不命中 ⇒ 不误入）。

结论：两项都**可做**，但都需要先扩展谱面（`isa-dsl` 的 operand kind 与汇编器），
不是降级层的活。优先级排在 `WA-52`（128 位 C-ABI 兼容）之后——
`asm!` 带 `sym`/`label` 的代码在真实语料里比例很低，而 ABI 不兼容是**静默错值**级风险。

在那之前保持 **fail-closed**（现状）：给出明确诊断，不静默降级。

## 2.7 P3 结论：M4-T3「第二目标」的本机可行性（2026-10-23 实测）

探测结果（本机一次性测，命令见下）：

| 依赖 | 结果 |
| --- | --- |
| `rustup target list --installed` | 只有 `x86_64-pc-windows-msvc`、`riscv32imc-unknown-none-elf` —— **没有 aarch64 的 std** |
| `aarch64-linux-gnu-gcc` / `aarch64-linux-gnu-ld` / `aarch64-w64-mingw32-gcc` | **都不在** |
| `qemu-aarch64` / `qemu-aarch64-static` | **都不在** ⇒ 本机**无法执行** aarch64 产物 |
| `clang` / `ld.lld` / `llvm-objdump` | **在**（`D:\Program Files\LLVM\bin`），且 `clang --target=aarch64-unknown-linux-gnu` **能出 `.o`** |
| `rust-src` 组件 | **在**（`lib/rustlib/src/rust/library`） |

结论：M4-T3 必须拆成两档，别混为一谈——

- **对象/重定位级验证（本机可做）**：用 `clang --target=aarch64…` 做**独立 oracle**（同一段汇编/同一份 IR 的手写对应物），
  用 `llvm-objdump`/`clang -c` 出的字节对照我们的生成物，用 `ld.lld` 验证重定位可解析。
  这能证明"编码/重定位/ABI 布局"层面正确，**不能**证明运行期语义。
- **真机/模拟器执行级验证（本机不可做）**：需要 aarch64 执行环境（qemu 或真机），
  以及 `rustup target add aarch64-…`（要网络）或 `-Z build-std`（`rust-src` 已在 ⇒ 可行但慢）。

⇒ P3 的"执行级"部分是**环境依赖**，不是代码缺口：记在这里，等有换机/换环境时再开；
本机先把"对象级"那一档做起来（验收 = 与 clang 的字节/重定位对照表全绿）。

**进度（2026-10-24，M6-92）**：对象级的**编码 + 对象层**已落地——
`crates/backend/forge-codegen/tests/arm64_clang_oracle.rs` 用 clang 做独立 oracle：
24 条 A64 指令逐字节对照；`fn add(a,b)` 编成 `elf64-littleaarch64` 对象后 `.text`
与 `CompiledFunction.code` 逐字节相等，ELF 头/反汇编由 `llvm-readobj`/`llvm-objdump`
核对；门禁新增 `arm64-oracle`。

**进度（2026-10-24，M6-97）——执行级也落地了**：本机装有 `qemu-system-aarch64`
（`D:\Program Files\qemu`，system 模式 + semihosting 退出，`exec/qemu_aarch64.rs`
已有脚手架但此前只拼**入口函数**）。这一轮：

- `isa/arm64.toml` 给 `BL` 加 `roles = ["call"]`、`BLR` 加 `roles = ["call_indirect"]`，
  `LDURX/STURX` 加 `stack_arg_load/stack_arg_store`（±256 的 imm9 覆盖栈参数/溢出槽）；
- qemu 夹具改为**布局全部函数**并按 `RelocKind::patch_value` 回填 `@N` 的 BL imm26
  （只接 4 字节 PC 相对重定位，其余明确报错）；
- 真执行两条：`qemu_aarch64_exec_direct_call`（add(20,22)=42，寄存器实参/返回）、
  `qemu_aarch64_exec_call_with_stack_args`（sum9(1..9)=45，第 9 个实参走栈）。
- 真机执行纠正了一处**约定数据**：aapcs64 的 `first_offset_slots` 原写 2（照 x86 的
  "push fp 后 [rbp+16]"），但本实现 arm64 的帧基址 = **入口 sp**（A64 的 `bl` 不压返回
  地址，保存 fp/lr 在帧内 `[x29-16]/[x29-8]`）⇒ 首个栈实参在 `[x29]`，改为 0；
  变参对象的 `__stack` 初值随之 `FrameOff(0)`（`abi_target_real` 同步）。

**仍未覆盖**：`forge-rustc` 的 aarch64 目标本身（需要 aarch64 的 core/std 与 rustc
target 接线）——那是"用 rustc 编 aarch64 程序"的下一步，不是本节的代码生成层。

**进度（2026-10-24，M6-98）——rustc→aarch64 首光**：`--target aarch64-unknown-none`
的 **no_core** crate 已能编、链、跑：

- 后端：对象文件按 `--target` 建（曾 `TargetConfig::host()` ⇒ 静默写 x86_64 COFF）；
  裸机三元组 `*-none` 在 target_lexicon 里 binary format = `Unknown` ⇒ 按 ELF 处理；
  `ObjectWriter::new` 失败改为报错（原来 `.expect` 直接 ICE）；Windows VirtualAlloc 的
  alloc runtime 只在 Windows x86_64 注入（aarch64 不再被它拖住）。
- 对象层（**数据驱动，库里零 arch 分支**）：`Relocation` 带上 ISA 侧语义名
  （arm64 `BL` 声明 `reloc = "call26"`，`[asm.reloc.call26]` 为**仅命名形态**）；
  链接器重定位类型写在 `conventions/aapcs64-arm64.toml` 的 `[link.elf]`
  （`call26 = { type = 283, clear_mask = 0x03FF_FFFF }` = `R_AARCH64_CALL26` +
  清零 imm26 位段）；`forge-object` 只按 `(格式, 名字)` 查表落类型/清位段，
  未命中回退 `object` 的 Generic（x86 现状）。**新增指令集只加 TOML，不改
  `forge-object`/`forge-rustc` 代码**（首版曾把 `Architecture::Aarch64` 写进
  对象库，属越界，已按此设计重做）。
- arm64 谱面：新增 `[[lower.StackAddr]]`（`movn` 造负偏移 + `add fp`）、
  `Load`/`Store`、`Inttoptr`/`Ptrtoint` 规则；`[[lower.Store]]` 按 IR 操作数序
  `(value, addr)` 写成 `str {0}, [{1}]`（首版写反 ⇒ store 拿值当地址，QEMU 走异常向量）。
- 工具链层：`validate_inst_lines` 现在接受**算术表达式占位符**（`{off - 4}` 之类；
  以前 codegen 支持、校验层误杀）；新增 `{off_f0}`..`{off_f3}`（栈槽偏移的 16 位分片，
  `movz+movk` 造任意偏移）。arm64 的 `Iconst` 改成按 **X 宽**材料化（i32 常量也写 X
  形式 = 符号扩展后的 64 位，与 x86 `MOV_REG_IMM64` 同约定）：W 形式会把 `-4` 高位清零，
  混合宽度使用（矩阵的 `stack_addr_iadd_offset_store_load`）会算出野地址死循环。
- 守卫：`crates/tools/forge-rustc/tests/arm64_exec.rs`（对象 `e_machine=183` 断言 +
  crt0/ld.lld/QEMU 真执行 42）、`forge-object` 的"不按架构分派"语义守卫
  （同一 RelocKind 在三种 `Architecture` 下必须给出同一 flags）、`link_table` 查表单测；
  门禁新增 `arm64-rustc-exec`。

**进度（2026-10-24，M6-99）——arm64 比较/选择/窄宽扩展落地，变参编得出**：
在 M6-98 的访存面上再补 `Icmp`（`cmp` + `csel 1/0`）、`Select`（i1 与 0 比 → `csel`）、
`Sextend`/`Uextend`（`add {out}, {zero}, {src}, sxtb/sxth/sxtw/uxtb/uxth/uxtw`；
**零操作数不能用 Rn=31**——扩展形式里 31 是 SP，实测把栈地址当结果）、`Ireduce`
（`mov w`），以及 `Band`/`Bor`/`Bxor` 的窄宽度兜底。结果：

- `jit_matrix_arm64`：**50 passed / 150 skipped / 0 failed**（M6-98 时 26 passed）；
  `variadic_va_arg_*` 用例在 QEMU 上真跑通过；
- `tm_aapcs64_varargs_*` 守卫从"必须 Unsupported"升级成"必须编得出"；
- 快照（`pins.txt`/`strict_overlap`）按实测更新：arm64 op 覆盖 13→18、真缺口 90→85、
  vary 候选 9→12、部分重叠 0→9（都是"窄宽度特化 + 兜底"的合法写法）。

**进度（2026-10-25，M6-100）——arm64 移位/除法/取余落地**：

- 新增编码（clang oracle 逐字节核对）：`LSLV`/`LSRV`/`ASRV` 的 X/W 六条（`DIV2A` 族，
  `op2d` = 8/9/10，与 `SDIV/UDIV` 同编码空间）。
- 降级规则：`Ishl/Ushr/Sshr` 直落 `LSLV/LSRV/ASRV`；`Sdiv/Udiv` 直落 `SDIV/UDIV`；
  `Srem/Urem` = `q = a/b; rem = a - q*b`（`SDIV/UDIV` + `MSUB`）。
- `jit_matrix_arm64`：**61 passed / 139 skipped / 0 failed**（M6-99 时 50 passed）。
- 快照：arm64 op 覆盖 18→25、真缺口 85→78、vary 候选 12→19、部分重叠 9→16
  （新增的 64/其余 成对规则，同样是"特化 + 兜底"）。

**进度（2026-10-25，M6-101）——arm64 标量浮点落地**：

- 新编码（全部经 clang oracle 逐字节核对）：`FADD/FSUB/FMUL/FDIV/FMAX/FMIN`（S/D）、
  `FNEG/FABS`、`FCMP`、`FCVT`（f32↔f64）、`SCVTF/UCVTF`（i32/i64 → f32/f64）、
  `FCVTZS/FCVTZU`（f32/f64 → i32/i64）、FP `ldr/str`（`LDRS/LDRD/STRS/STRD` 无位移形态）。
- **解码 trie 的硬约束**：同一节点上固定位段两两不相交 ⇒ 标量浮点全部统一成
  **11 位 opcode 前缀**（[31:21] = 0xF1/0xF3/0x4F1/0x4F3，与 `SDIV/UDIV` 同族），
  1 源指令再用不重叠的 `opc` 字段（[20:10]）区分；`FMOV` 也并入该前缀
  （原来 8 位 opcode 0x1E 会与 0xF1 在 [24,32) 判歧义）。
- 降级：算术/取负/绝对值/比较（`Fcmp` 的 14 个条件——有序条件直接 `csel`，
  NaN 相关条件用"先有序判定、再 OR `vs`"，`mi/vs/vc` 走 **flag 银行的 `CSEL_F`**
  转 `FlagCC::{S,O,No}`）、`Fpext/Fptrunc`（`FCVT`）、`Sitofp/Uitofp/Fptosi/Fptoui`
  按 (结果宽, 源宽) 四条、`Fload/Fstore`。
- **QEMU 夹具**：crt0 必须开 `CPACR_EL1.FPEN`（复位时 FP/SIMD 关闭，访问 d 寄存器会
  trap 到异常向量）；浮点实参按 AAPCS64 装 `d0..d7`（`movz/movk` → `fmov dN, x9`），
  `Executor::exec_f64_args` 已有 aarch64 实现。
- 结果：`jit_matrix_arm64` **63 passed / 137 skipped / 0 failed**；其余浮点用例
  （`Fma`/`Fceil`/`Ffloor`/`Fcopysign`、向量）仍按能力集 Skip。

**进度（2026-10-25，M6-102）——arm64 浮点常量 / Fma / 取整 + FPR↔GPR 桥**：

- `{fconst_f0}..{fconst_f3}` 占位符（浮点位模式的 16 位分片，与 `{iconst_f*}` 同序）+
  `FMOV_*_FROM_*`（x↔d、w↔s）⇒ `Fconst` 不再缺规则：f32/f64 常量经 GPR 材料化后
  `fmov` 进 FPR。
- 新增 `FMADD`（`Fma` 的融合乘加）、`FRINTP`/`FRINTM`（`Fceil`/`Ffloor`，S/D）。
- `Fcmp` 的谓词从 `elem` 改为 **`rs1_width`**：`elem` 取的是**结果**类型，而 `Fcmp` 的结果是
  bool（elem=5）⇒ 之前所有 `Fcmp` 规则都不命中（本轮补 `Fconst` 后才暴露）。
- 结果：`jit_matrix_arm64` **75 passed / 125 skipped / 0 failed**（M6-101 为 63）。
- 剩余：`Fcopysign`（需符号位掩码 + FPR/GPR 往返，单独一轮）尚未做，仍按能力集 Skip。

**进度（2026-10-25，M6-103）——arm64 `Fcopysign`**：

- A64 没有 `fcopysign` 单指令：降级 = `fabs` 取幅值 + 在 GPR 里拼符号位
  （`fmov` 取 b 的位型 → `and` 单 bit 掩码（`movz` 造 0x8000_0000 / 0x8000<<48）→
  `orr` 到幅值位型 → `fmov` 回 FPR），32/64 两套。
- 矩阵新增两条**可判定**用例（F64 直出型会被 i8 值域过滤跳过）：
  `fcopysign_neg_to_pos_i64`（-3.5 配 +2.0 → 3）与 `fcopysign_pos_to_neg_i64`
  （+3.5 配 −2.0 → −3），经 `Fptosi` 落进 i8 值域。
- 结果：`jit_matrix_arm64` **77 passed / 125 skipped / 0 failed**。

**进度（2026-10-25，M6-104）——窄宽度访存 / 掩码截断 / UDF 陷阱；aarch64 core 路线侦察**：

- `Load`/`Store` 补 **i1/i8/i16** 宽度（arm64：`ldrb/ldrh/strb/strh`；riscv64：`lbu/lhu/sb/sh`），
  `Ireduce` 补 1/8/16 的**显式掩码**（`and 1`/`and 0xFF`/`and 0xFFFF`）——A64 寄存器
  没有窄写回，不掩码会让高位垃圾进入后续 X 宽比较/加法。
- 新增 `UDF`（全零字）并标 `effect = ["Trap"]`：`Unreachable` terminator 现在按 DSL 的
  "无操作数陷阱"约定发 `udf`；新增守卫 `tm_unreachable_emits_udf`。
- 矩阵新增三条**真执行**用例（arm64 79 / riscv64 139，均 0 failed）：
  `narrow_i8_store_load`、`narrow_i16_store_load`、`bool_i1_store_load`。
- **aarch64 core 路线侦察结论**：rustup 无 `aarch64-unknown-none` 的 rust-std（离线装不上），
  但 rust-src 在本地 ⇒ 可直接 `rustc` 为 aarch64 编出 `libcore.rlib` 与
  `libcompiler_builtins.rlib`，再用 forge 后端编用户 `#![no_std]` crate（进度：
  `Store/Ireduce` 缺口已清，`UDF` 已补）。**仍阻塞**：大函数溢出用 `STURX`（imm9，
  ±256）——`swap_nonoverlapping_short` 需要 312 字节偏移，属帧/溢出**编码形态选择**
  的缺口（下一步：溢出改用 SP 基址 + 无符号 scaled imm12，或寄存器偏移形态）。

**进度（2026-10-25，M6-105）——aarch64 + `core` 首光**：

- **可运行闭环**：`#![no_std]` crate（`core::cmp::max/min`、`mem::size_of`）由 forge 后端编出
  aarch64 对象，与 **LLVM 编的 `libcore.rlib` + `libcompiler_builtins.rlib`** 一起经 `ld.lld`
  链接，在 QEMU 上真跑出 `12`。新增测试 `rustc_aarch64_core_probe_runs_under_qemu`
  （离线装不上 `rust-std-aarch64-unknown-none`，改从本地 `rust-src` 直接为目标编两个 rlib，
  产物缓存在 `target/tmp/arm64-core-probe/sysroot`）。
- **为它补的谱面规则**（arm64）：`SaddOverflow/UaddOverflow/SsubOverflow/UsubOverflow`
  （`ADDS/SUBS` + 标志 → `CSEL`/`CSEL_F`；debug 溢出检查真的走到了）、`Undef/Poison`
  （取零的合法特化）。**仍缺**：`SmulOverflow/UmulOverflow`（需 `SMULH`/`SMADDL` 组合）、
  `GlobalAddr`（静态数据地址；需 A64 `abs_g0..g3` 重定位序列）。
- **诊断改进**：谱里没有某 op 的规则时，lowering 兜底错误现在**点名** op（旧文本只有
  `DSL lowering`，盲猜过一轮）。
- **仍阻塞**：大函数溢出槽偏移超 `STURX` imm9（±256，实测 312/344）——帧/溢出编码形态选择；
  以及 `core` 里遇得到的 `GlobalAddr`、乘溢出。
- 结果：arm64 op 覆盖 47→53、真缺口 56→50；`jit_matrix_arm64` 79 passed / 0 failed 不变。

**进度（2026-10-26，M6-106）——大帧溢出（imm9 限制解除）**：

- **溢出槽**：`[machine.spill.*]` 新增**数据键** `offset_base = "sp"`。发射侧据此把偏移换成
  「相对 sp」口径（fp-inside 下 = `frame + fp_offset`，恒非负、按槽宽对齐），模板用
  **无符号 scaled imm12**（A64 `LDRX/STRX`、`LDRD_ND/STRD_ND`、`LDRQ_FP/STRQ_FP`）——
  溢出从 ±256B 扩到 32KB 量级。
- **帧件（callee-save / 栈形参）**：`callee_save/callee_load` 角色从 `STURX/LDURX`
  （imm9）换到 `STRX/LDRX`（scaled imm12）；栈形参的**负**偏移仍留在 imm9 族
  （scaled 只收无符号）——角色按形态拆分。
- **帧尺寸记账修正**（fp-inside）：局部槽地址是 `fp - stack_slot_shift - depth`，而
  帧只按 depth 记账 ⇒ 槽深到帧底时最深的槽落到 `sp` 之下（新矩阵用例 40 个槽实测
  data abort 循环）。`calculate_frame_size` 对 `FrameLayoutKind::Inside` 补加
  `stack_slot_shift`；fp-outside（x86，shift 是已 push 的字节数）不加。
- 新矩阵用例 `deep_frame_many_slots`（40 个局部槽 + 读最深两槽，真执行）⇒
  `jit_matrix_arm64` **89 passed / 0 failed**；riscv64/x86 矩阵同步回归绿（riscv 也是
  fp-inside，帧记账修正对它同样生效）。
- **收益**：`core::ptr::swap_nonoverlapping_bytes` 一类大帧函数不再因 `STURX imm9`
  编不出（`swap_nonoverlapping_short` 已能编过）；aarch64 core 探针剩余阻塞收敛到
  `GlobalAddr` 与 `UmulOverflow` 两项。

**进度（2026-10-26，M6-107）——scaled 溢出真执行守卫 + core swap 阻塞点收敛**：

- 新增 **QEMU 真执行守卫** `rustc_aarch64_scaled_spill_probe_runs_under_qemu`：40 个跨调用活跃的
  标量把寄存器压力拉满，断言（a）机器码里 **没有** unscaled `stur/ldur`（顶层 0xF8）、
  （b）**有**至少一条 `str/ldr [sp, #imm>255]`（scaled imm12 ≥ 32）、（c）真跑出 `39`。
  实测该探针帧 ~0x6c0、24 条 >255B 的 scaled 访存——这是"换了 scaled 形态"而非"偏移碰巧还小"
  的直接证据（矩阵用例的帧太浅，分辨不出）。
- **core swap 侦查（未通过，记录）**：`core::mem::swap` 探针在 **O0**（debug-assertions 开）仍会
  卡住——触发的是 `swap_nonoverlapping_short` 之外的地址计算路径；关掉 debug-assertions 后能编过，
  但运行期仍 fault（LLVM 编的 `mem::swap` 与 forge 编的调用方交互处拿到坏指针），待用 IR/机器码
  对拍定位。
- **O2 暴露的两个后续缺口**（`cargo`-侧真跑时踩到）：
  1. `stack_arg_store/load` 的偏移是**负**的（`[sp + caller_offset]`），仍受 imm9 ±256 限制
     （实测 `Sturx: -352`）——stack-arg 超过 32 槽的大调用需要寄存器偏移/地址材料化形态；
  2. O2 下出现 `Iadd no matching rule`（兜底诊断已能点名 op）——待查该实例的属性组合。

**进度（2026-10-26，M6-108）——栈实参写槽统一、缺失值描述补齐、诊断带属性快照**：

- **栈实参写回槽走统一 spill 机制**：`move_args` 里"被 spill 的寄存器形参 → spill 槽"原先用
  `stack_arg_store` 角色指令直发（imm9 ±256），大函数实测 `Sturx: -352` 编不出；现在改调
  `emit_spill_store`（同一套 fp/sp 偏移口径 + 按类分派）——`stack_arg_store` 只保留"调用方
  写传出区"那层。
- **缺失值描述补齐**：谓词属性 `rs1_width`/`rd`/`elem` 读 `desc_of`；legalize 改写/复制/常量
  转发后某些值没有描述 ⇒ 按宽度分派的规则整批不命中（O2 实测
  `Iadd no matching rule（attrs: rs1_width=None, descs=[None, Some((i64,GPR64))]）`）。
  现在 `lower_inst` 前用 **IR 类型**补齐**缺失**的描述（已有不动）。
- **诊断带属性快照**：无规则错误现在同时打印 8 个谓词属性 + 每个实参的描述（`None` 会直接
  暴露"值描述缺失"这类根因）——这轮正是靠它定位到 `Iadd` 的 `rs1_width=None`。
- **core swap**：O2 下已能编过（上面两项），但运行期仍在 `forge_entry` 出错，机器码形如
  `mov w13,#1 … strb w14,[x13]`（`csel` 的临时寄存器被当成 store 地址）——**疑似**指令包内
  虚拟寄存器编号/字段映射串位，下一步从 `Icmp`+`store i1` 的 packet 对照查起。

**进度（2026-10-27，M6-109）——core swap 根因定位：循环不动量的活跃区间丢失**：

- 对照实验：同一探针用 **纯 LLVM** 编（`crt0` + `memcmp/memset/memcpy` stub）**跑出 8**（正确值），
  说明 `crt0`、链接、`libcore`、退出码链路都没问题 ⇒ 坏码来自 forge 自己的 codegen。
- 定位到 `core::mem::swap<i32>`（泛型实例在探针 crate 里由 forge 编）的循环：

  ```text
  0x400002b4  mov x24,#0        ; i
  0x400002c0  mov x21,#1        ; 循环上界（icmp 的常量 1）
  0x400002ec  cmp x24,x21       ; i < 1
  0x400002fc  mul x28,x24,x22   ; i*8
  0x40000304  add x21,x25,x28   ; !!! x21 被"指针 y + 偏移"覆盖 —— 上界被冲掉
  0x40000318  add x24,x24,x23   ; i += 1
  0x4000031c  b   loop
  ```

  上界寄存器 `x21` 在循环体里被**另一个活跃值（指针 y）复用**：此后 `cmp i, x21` 永远成立，
  指针按 `i*8` 一路越界 → `ldr` fault。结论：**跨循环活跃的值没有把活跃区间算进去**，
  分配器把已被占用的物理寄存器再分配出去（`evict` 未触发）。

- O2 的 `USE-LOST`（基址 vreg 在定义块之前使用）另有一条独立线索：vcode 里出现
  "block 1 使用、block 5 才定义"的 xreg——同一函数内 xreg 编号复用，待与活跃区间问题一并查。
- **诊断已加**：`FORGE_TRACE_LIVE=1` 现在同时打印每块的 `uses/defs/live_in/live_out`
  （此前只有 succ 表与区间，看不出"哪一块的 uses 漏了"）——下一步用它逐块核对
  "preheader 常量 → 循环头使用"这条链。另注意：分配后自检 `verify_no_overlap` 通过，
  说明**区间之间不重叠**（不是"两个活区间抢同一寄存器"，而是**区间少覆盖了循环体**）。

- **下一步**（明确）：查 `regalloc_bt` 的活跃区间构建——特别是
  **phi/循环携带值**与"定义在 preheader、使用在 loop body"的常量（本轮 x21 就是这类）；
  修好后用 core swap 探针（O0/O2 两档）做验收，再补 IR 级守卫。

**进度（2026-10-27，M6-111）——core swap 修好：缺的是控制流 effect（CFG 无边）**：

- **根因**：生成物的 `is_branch`/`branch_targets` **只从 `effect` 派生**，而 arm64 的
  `B`/`B_*`/`CBZ`/`CBNZ` 当时**没写 effect**（`B` 只有 `roles`）⇒ 所有分支边都不进
  liveness 的 CFG ⇒ 循环回边丢失 ⇒ "preheader 定义、循环头使用"的值被当成已死，寄存器
  在循环体里被复用（`core::mem::swap` 的上界 `x21` 被"指针 y + i*8"覆盖 → `cmp` 永真、
  指针越界 fault）。
- **修法（纯数据）**：arm64 补 `effect`：`B`/`BR` = Jump、`B_*`/`CBZ`/`CBNZ` = Branch、
  `BL`/`BLR` = Call；riscv 同缺口一并补（`JAL` = Call+Jump、`B 型` = Branch）。
- **连带**：CFG 修好后溢出压力真实上升，实测"一条指令要 3 个 scratch"⇒ arm64 谱面
  `[machine].scratch` 从 2 个补到 3 个（X16/X17/X18）。
- **验收**：`core::mem::swap` + 循环探针在 **O0 与 O2 两档都真跑出 8**（此前两档都挂）；
  新守卫 `rustc_aarch64_core_swap_probe_runs_under_qemu`（两档都跑、断言 8）钉住。
- 附：`docs/reference/isa-dsl.md` 的 effect 小节加了"控制流指令必须写 effect"的红线说明。
- 附带两处工程修正：① 新增 `FORGE_TRACE_LIVE_FN`（函数名过滤，已进 KNOBS 台账与契约文档表——
  否则 `trace_knobs` 三方守卫红）；② `arm64_exec` 的三条 core 探针测试改为**各自独立子目录 +
  `current_dir`**：它们原先共享同一 `target/tmp/arm64-core-probe`，并行跑时 rustc 的
  `-o` 复制会撞上 `forge_codegen_output*` 临时产物 → `无法把对象写到 …（os error 2）`。

**进度（2026-10-27，M6-112）——`GlobalAddr` 与 `UmulOverflow` 补齐，debug-assertions 探针真跑通**：

- **`GlobalAddr`**：A64 用 `MOVZ/MOVK` 四片（G3→G2→G1→G0）承载绝对地址，每片一个链接器
  重定位。ELF 的 `R_AARCH64_MOVW_UABS_G*` 是**指令字补丁**（16 位立即数嵌在 4 字节指令里），
  于是给 `[asm.reloc.<名>]` 加了两个**数据键**：`width`（补丁宽度，缺省 = 槽宽）与
  `fixup`（`slot` 缺省 / `inst`）——宿主仍无 ISA 分支；链接器类型号写进
  `conventions/aapcs64-arm64.toml` 的 `[link.elf]`（G3=269、G2_NC=268、G1_NC=266、G0_NC=264）。
  编码期的负占位（`-(g+1)`）在无符号 imm 槽会被 `encode` 拒绝 ⇒ 带 `fixup="inst"` 的臂
  **先按 0 编码**、再发重定位（占位只用来判"这是符号引用"）。
- **`UmulOverflow`**（无符号乘溢出，64/32 两档）：`a*b > MAX ⟺ b≠0 ∧ a > MAX/b`，
  用 `MUL` + `MOVN`(全 1) + `UDIV` + 两次 `CMP`/`CSEL` + `AND` 分支自由地算；A64 的
  `UDIV` 除零返回 0（不陷入），所以 b=0 用 `b != 0` 掩掉。（有符号 `SmulOverflow` 还缺
  `SMULH`，另案。）
- **验收**：core swap 探针在 **O0 / O2 / O0+debug-assertions** 三档都真跑出 **13**
  （含静态数据 `MAGIC=5` —— GlobalAddr 真正被解析并读回；debug 档会编出 core 的
  precondition/overflow 检查，UmulOverflow 被真实执行）。守卫测试扩成三档。
- 附带：`core` 的 debug 档会拉进 `escape_ascii`/`dec2flt` 等"只编不跑"的代码 ⇒ 链接需要
  `memcmp/memset/memcpy/__udivti3` 兜底，守卫测试里加 `CORE_STUBS_S`（不参与被测路径）。
- **矩阵解锁**：`GlobalAddr`/`UmulOverflow` 补齐后 arm64 多跑 4 条 `CaseKind::Module` 用例
  （`globaladdr_load`/`globaladdr_store_load` 等）⇒ `jit_matrix_arm64` **93 passed / 0 failed**。
  为此补了 `QemuAarch64Executor::exec_module`（多函数布局 + `@N`/`G{id}` 就地补丁 + 全局数据段）：
  **全局地址必须用数据段页对齐的 `p_vaddr`**，拿文件偏移当地址会"看起来对、一读就 fault"
  （实测挂起，已修）。

**进度（2026-10-27，M6-113）——`SmulOverflow`（含新 `SMULH` 指令）**：

- 新指令 `SMULH`（有符号 64×64 高半）：复用 **MUL3** 形状（`MULADD` 的四字段需要
  `#0..#3` 全排列，3 操作数写不出来——validate 要求 0..2 不重复；MUL3 的 `ra` 由
  `match = { ra = 31 }` 固定，正好）。
- `SmulOverflow`：64 位档 = `SMULH` + `MUL` + `ASRV #63` 与高半比较（不相等即溢出）；
  32 位档先把两操作数 `sxtw` 到 64 位（乘积精确），再用 `p>>63 != p>>31` 判
  `p == sext32(low)`。**教训**：`ADD_X_EXTW_NA` 的最后一个字段是 `[enum.extend_w]` 的
  **枚举值**（`sxtw = 6`，不是 `4`）——写错时"无溢出"被判成溢出。
- 验收：arm64 矩阵解锁 2 条 `smul_overflow_*` 用例 ⇒ **95 passed / 0 failed**；
  pins：arm64 `op_gap = 56,6,9,47`、`unassigned_bits = 272`、`spec_totals` 610、
  `roundtrip.derived` +2、编码 digest 更新（全部经官方写入器/实测值刷新）。

**进度（2026-10-27，M6-114）——arm64 原子：LSE 单指令（seqcst）**：

- 新形状 `[form.LSE3]` + 六个族（`SWPAL`/`LDADDAL`/`LDCLRAL`/`LDSETAL`/`LDEORAL`/`CASAL`，
  各 X/W 两行）。编码：`opcode` = [31:24]（含 size 位）、`[23:21]` = A/R/1 固定 1（AL =
  seqcst 档）、`[15:12]` opc2、**`[11:10]` fix2**（LD* = 0、CAS = 3——CAS 的 `[15:10]`
  是 `111111`，只写 4 位会编出非法字，实测 QEMU 直接 `<unknown>` 挂起）。
- `AtomicRmw`：Xchg=SWPAL、Add=LDADDAL、Sub=LDADDAL(取负)、And=LDCLRAL(取反)、
  Or=LDSETAL、Xor=LDEORAL；`Cmpxchg` = MOV+CASAL+MOV，成功标志取 CAS 的 Z（CSELW）。
  32 位档同族换 W 视图。**IR 的内存序是第二个立即数、谓词看不到** ⇒ 一律用 AL
  （对所有 ordering 都正确，略慢，不静默放宽）。
- 验收：arm64 矩阵解锁 5 条原子用例（add/sub/xchg/cmpxchg 成败）⇒ **100 passed / 0 failed**。
- 测试口径：QEMU 默认 CPU 改为 `max`（LSE 需要 ARMv8.1-A；老机型 `cortex-a57` 只能跑
  非原子路径），`FORGE_A64_CPU` 可覆盖。
- pins：arm64 `op_gap = 58,6,9,45`、`spec_totals` 622、`roundtrip` +24、encoding digest 更新。

**进度（2026-10-28，M6-115）——真实 core 程序的三处阻塞**：

- **`select_unpredictable`**：降级层新增 intrinsic 映射（语义就是 `Select`，分支预测提示后端
  无法表达）——缺了它 core 的 sort 实现整条编不过。
- **`Clz`**：新 `[form.DP1BIT]` + `CLZ`/`RBIT`（X/W 两档，clang 逐字节核对）；窄宽度的
  `32-w` 修正在 legalize 层，谱只覆盖 32/64。arm64 `op_gap` 因此 59/44。
- **大帧**：`sub sp, sp, #imm12` 只到 4095；`[u64; 600]`（帧 5936B）实测直接失败
  （`Addimmx: immediate 5936 out of range`）。`sp_adjust` 与 **`frame_set`（fp ← sp + frame）**
  现在都按**槽的 `min/max`** 分块（宿主不写死上限），落点仍是 frame_size；守卫
  `rustc_aarch64_big_frame_probe_runs_under_qemu` 在 O0/O2 都真执行出 1799。
- **已知阻塞（另案）**：`store i1 <常量>` 会拿到一个**已被墓碑化**的 `Iconst` 结果
  （`ValueData.ty = VOID`），desc 变成 `bits: 0` ⇒ `Store no matching rule`。触发者是
  DCE/某 pass 删了仍被 Store 引用的 Iconst（use-list 刷新漏点）。真实 fmt/sort 程序卡
  在这里；修法必须钉在 IR 侧（删除必须连带重导向使用点），不是给 lowering 猜宽度。

**进度（2026-10-28，M6-116）——ZST 字段/值的 Store**：

- 真实 fmt/sort 程序在 O0 的硬阻塞：聚合构造按字段 `lower_operand`+`Store`；`()` 字段的
  `map_type` 是 **VOID** ⇒ 造出 `Iconst ty=VOID`（desc `bits: 0`）⇒ `Store no matching rule`
  （用 `FORGE_TRACE_LOWER` 的 `VOID-def op=Iconst imm=Const(Bits(6))` + backtrace 定位到
  `lower/statement.rs` 的字段写入）。
- 修法：**ZST 没有内存表示**——聚合字段循环里 `f_sz == 0` 跳过；`store_ty` 对
  `ty == VOID` 直接返回（防御性，避免任何路径把 VOID 值写成 Store）。**不是**给 lowering
  猜宽度。
- 守卫 `rustc_aarch64_zst_aggregate_probe_runs_under_qemu`：`struct S { a: i32, b: () }`
  的构造 + 元组含 `()`，O0 真执行 **7**（修前编译期就 `Store no matching rule`）。
- 已知未闭环：fmt+sort 全量探针现在**能编过**（O0），但 QEMU 里挂起（panic 循环或异常
  循环，待定位）；O2 在 `const-fold` 后 IR 校验失败（`Udiv`/`Ushr` 操作数类型不一致，
  优化档另案）。

**进度（2026-10-28，M6-117）——迭代器跑通（ZST 字段写越界）**：

- 现象：`v.iter()` 单独就 Data Abort（`x29` 在 `Iter::new` 返回后变 0）。QEMU gdbstub
  写观察点（`Z2,<fp-16>,8`）抓到两次写：prologue 正确写 fp，随后 `Iter::new` 内一条
  **间接 store** 写 0 到同一槽。
- 根因：结构体逐字段构造循环（`lower/statement.rs` 的 Adt/Tuple/Array 分支）**没有跳过
  ZST 字段**——`Iter` 的 `_marker: PhantomData`（0 字节）被当成 8 字节写到 `slot+16`；
  `Iter` 槽只有 16 字节，于是越界 8 字节正好落在被调方 `x29` 保存槽 ⇒ 返回后 caller
  fp=0 ⇒ 下一条 fp 相对 store 直接 Data Abort。（M6-116 只补了**枚举**分支。）
- 修法：该分支同样 `if f_sz == 0 { continue; }`。
- 验收：新守卫 `rustc_aarch64_iterator_probe_runs_under_qemu`：`while let Some(x)=it.next()`
  与 `iter().copied().sum()` 两路求和，O0 真执行 **20**。
- **仍未闭环**：`core::fmt` 整数格式化探针 O0 运行到 **`Undefined Instruction` @ `ELR=0x0`**
  （跳到地址 0：空函数指针/坏返回地址，疑似 `&dyn Display` vtable 或函数指针收参）；
  O1/O2 另有 `const-fold` 后 IR 校验失败（`StoreAddrNotPointer` / `Ushr` 类型不一致）。

**进度（2026-10-28，M6-118）——fp-inside 栈槽平移漏了 callee-saved 区**：

- 现象：`write!(buf, "{}", a)`（带格式化实参）跳到地址 0；而 `write!(buf, "x")`
  与直接 `write_str` 都正常。
- 定位：QEMU gdbstub 写观察点打在 `Buf::write_str` 的 **x24 保存槽**（`fp-0x40`）上，
  命中序列 = prologue 存 x24 → 零初始化 `str x25,[x24]` 写同一地址（局部槽）→ …
  ⇒ 局部槽落在 callee-saved 保存区里；返回时 x24 被局部值覆盖 ⇒ 内层
  `core::fmt::write` 的 `blr x24` 跳到 0。
- 根因：`frame_layout_info` 的 **fp-inside** 分支 `stack_slot_shift = fp_push_bytes`
  （16），但保存槽占了 `[fp - fp_push - cs_bytes, fp)`（x19..x28 最多 80B）。
  修法：`stack_slot_shift = fp_push_bytes + cs_bytes`（Outside 不变）。
- 验收：`write!(buf, "{}", a)` 真执行（f3 探针输出 `"1"` ⇒ exit 1）；新守卫
  `rustc_aarch64_fmt_probe_runs_under_qemu`（`write!("{}:{}:{}")` + 手写排序 +
  `count_ones` + Collatz，O0 真执行 **65**）。
- 连带修正（同一缺陷面）：栈槽整体下移后，`aapcs64_struct` 的保存区（原先排在局部槽
  之下）偏移超出 `STURX` 的 imm9 范围（实测 `Sturx: immediate -368`）。AAPCS64 定本要求
  `__stack` 指向**入口 sp**、保存区紧贴其下 ⇒ 改成 `contiguous = true`（帧顶保存区），
  与 riscv 同一条路径；arm64/riscv/x86 三个 JIT 矩阵（含变参用例）全绿。

**进度（2026-10-28，M6-119）——优化档（O1/O2）IR 合法性整批修复**：

- `-C opt-level=1/2` 下此前连编都编不过（verifier 在 pass 之后拒绝），逐条修好：
  1. `copy_agg` 的地址统一 `inttoptr` 成 PTR（sret 指针/间接聚合形参在签名里是 i64 槽）
     ⇒ 消除 `StoreAddrNotPointer`；
  2. `CastKind::PointerExposeProvenance` / `PointerWithExposedProvenance` 显式
     `ptrtoint`/`inttoptr`（旧实现落进 `_ => Ok(val)`，指针值当整数用）；
  3. `ptr_offset_from` / `ptr_offset_from_unsigned` 先把两个指针 `ptrtoint` 再做
     差值/除法（旧实现产出 `udiv ptr %x, i64 4`）；
  4. GVN-PRE 的 `insert_expression` 复制键里的 **immediates**（v22 起 `IntCC`/`FloatCC`
     在立即数通道，空立即数重建的 `icmp` 没有条件 ⇒ `MissingCondImmediate`）；
  5. 溢出槽最小 8 字节、8 对齐（vreg 类可能只有 4 字节宽，写槽却是 `STRX`（8B scaled）；
     旧实现给出 off=4 ⇒ `Strx: immediate 4 不是 8 的整数倍`）。
- 验收：fmt 守卫升格为 **O0/O1/O2 全档**真执行 65（`write!` 整数格式化 + 插入排序 +
  `count_ones` + Collatz）；`-O1/-O2` 的 `forge_entry`/`Iter::len` 也能编过并跑。
- **已由 M6-120 闭环**：迭代器探针在 O1 的 panic 是 `const-fold` 的 `fold_icmp`
  按**结果位宽**（i1）截断操作数（`0 < 4` 折成 false）——见下一节。

**进度（2026-10-28，M6-120）——`const-fold` 的 `icmp` 按结果位宽截断（O1 误编译）**：

- 现象：迭代器探针 `-O1` 运行期 panic（`ub_checks` 的
  `0 < pointee_size && pointee_size <= isize::MAX`），`-O0`/`-O2` 正常；
  `FORGE_OPT_SKIP=const-fold` 可绕过。
- 根因：`fold_icmp` 用传入的 `ty`（**结果类型 i1**）做 `as_unsigned_big` 截断 ⇒
  `icmp ult i64 0, i64 4` 把 4 截成 1 位 = 0 ⇒ `0 < 0` = false ⇒ `store i1 0`
  ⇒ 断言分支走 panic（机器码实测：`mov x25,#0; strb w25,[..]`）。
- 修法：按**操作数各自的类型**（`ConstValue::Int(_, ty)`）截断；有符号比较先
  `truncate_to_type` 再比，无符号比较各自 `as_unsigned_big`。
- 验收：两个真程序守卫都升到 **O0/O1/O2 全档**——迭代器守卫 20、fmt 守卫 65，
  三档全绿；arm64_exec 8/8、三 JIT 矩阵、forge-opt 101、clippy 0。
- **连带（同一批）**：`Big` 的比较全面回到**原生任意精度 `Ord`**（不再先转 `f64`/`i64`）：
  `PartialOrd::partial_cmp` 直接复用 `Ord`（`Big::Float` 是任意精度实数、构造时拒绝 NaN，
  三族之间是全序）；跨变体经 `Integer`/`Real` 精确转换；`fold_fcmp` 用 `Ord` 判
  eq/lt/gt（`U*` 与 `O*` 在折叠层同值），`Fmin`/`Fmax` 直接选 Big（零按 IEEE 符号规则
  取 `-0`/`+0`），`Fcopysign` 只搬符号位（f128 量值不再经 f64 变 `Inf`）。
  同时删掉语义含糊、与 `Ord` 冲突的私有 `cmp_total`。

**进度（2026-10-28，M6-121）——常量折叠全宽度/全格式，不再有 `Big→固定类型→Big` 往返**：

- 浮点 I/O：`Big::from_float_bits(u128, fmt)` / `to_float_bits(fmt) -> u128` 用 dashu 原生
  `Real` + `split_bits`/`bit_len`/`bit` 精确编解码 **f16/f32/f64/f128**（不走 `f64`；
  f128 的 112 位尾数逐位保留；NaN 返回 `None`）。
- 折叠层：`Fconst` 的已知值按**浮点**解码（此前把位模式当整数）；浮点结果回写 **Bits
  通道**（f32 不再落成 f64 位模式的低 32 位）；`+ - * / fmod fma sqrt floor ceil trunc
  round` 全格式折叠（中间态 `p+8`/`2p+16` 守卫位，落到常量池时一次正确舍入；
  `x/0 = ±inf` 折叠，NaN 结果除外——`Big::Float` 是精确实数、没有 NaN）。
- 整数：`popcnt/clz/ctz/bitreverse/bswap` 按目标位宽用 dashu 原生
  `bit_len/bit/count_ones/trailing_zeros/split_bits` 算（**i128 高 64 位不再丢**）；
  移位量按位宽取模（与后端硬件语义一致，任意大常量都能折）；`sadd_sat/ssub_sat/
  uadd_sat/usub_sat` 用 Big 的任意精度边界（i128 正确饱和，旧实现写死 i64）。
- 测试：`float_bits_roundtrip_all_formats`、`f128_bits_keep_full_mantissa`、
  `nan_bits_are_rejected`、`inf_and_zero_sign_roundtrip`、`trunc_to_u128_keeps_low_bits`、
  `fold_bitops_keep_i128_high_bits`、`fold_saturating_keeps_i128_bounds`、
  `fold_shift_amount_is_masked_like_backend`、`folded_float_constants_use_target_format_bits`。

**进度（2026-10-28，M6-122）——`core::slice::sort` 真跑（连带 4 类后端缺陷）**：

- 新增守卫 `rustc_aarch64_sort_probe_runs_under_qemu`：16 元乱序数组 `sort_unstable()`
  后校验连续 0..15（返回值 15）。修前 `[2,1].sort_unstable()` 得 `[2,2]`/Data Abort。
- **`ptr::copy_nonoverlapping` 逐元素宽度**：旧实现每元素固定搬 `step_by(8)` 个 8 字节块
  ⇒ i32 元素越界 4 字节 + 4 对齐地址上 `str x` 触发 arm64 对齐异常；`ptr::copy` 还缺
  **memmove 反向分支**（dst>src 重叠时把首元素一路复制，实测排序成 `[2,2,4,3]`）。
  现按元素 `size`+`align` 用 8/4/2/1 阶梯精确覆盖；`copy` 在 dst>src 时反向复制。
- **小聚合的按值宽度**：`map_type` 把聚合兜底成 `PTR`，`MaybeUninit<[u8;4]>` 这类
  **整数 ABI 标量**被 8 字节读写（`swap_chunk` 越界 + 对齐异常）。`place()` 现在按
  `backend_repr` 选宽度：整数 Scalar / 自然对齐的 Memory 小聚合 → I8/I16/I32/I64，
  指针 Scalar（`Option<&T>` 等）保持 PTR。
- **`TerminatorKind::Drop` 真调 drop glue**（`Instance::resolve_drop_glue`）：旧实现只
  连边 ⇒ `CopyOnDrop` 的析构从不执行、gap 里暂存的元素从不写回（`[2,2]` 的直接原因）。
- **`size_of_val_raw::<[T]>`（slice/str）动态求长**：unsized layout 旧实现得 0 ⇒
  `swap_nonoverlapping` 认为 bytes=0 整个跳过；现从胖指针元数据取 len（slice:
  len×elem_size，str: len）。
- **函数项调用**（`&mut T::lt` 这类 `FnDef` place）：旧实现按 8 字节 load 零大小函数项
  ⇒ 拿到 0 后 `blr 0`；现识别 `FnDef` place 直接取实例符号地址。
- **arm64 补 `[[lower.Trap]]`**（UDF）——IR 层显式 `Opcode::Trap` 不再报
  `[cap:SPEC_LOWERING_RULE_MISSING]`；`abort` intrinsic 按目标分派：Windows 保持
  `ExitProcess(3)`，非 Windows 落 `Trap`（freestanding 目标不再因 `ExitProcess`
  链接失败——`core::slice::sort` 内部 `abort()` 调用点实测）。
- pins：arm64 `op_gap = 60,6,9,43`（补 `[[lower.Trap]]` 后覆盖 59→60、真缺口 44→43）。
- **仍未闭环（另案）**：`-O1+` 时 rustc 的 MIR inliner 把 `#[inline]` 实例标成
  available-externally 且不进 CGU 分区，而 forge 优化器**没有内联 pass** ⇒ 直接调用点
  留未定义符号（sort 的 `CopyOnDrop::drop`/`revswap`）。当前 sort 守卫只跑 `-O0`；
  真修需要后端"按需发射缺失实例"或自实现内联。

**进度（2026-10-28，M6-123）——`-O1+` 缺失实例按需补编；sort 三档真跑**：

- **按需补编 available-externally 实例**：lowering 期间记录每个直接调用目标的
  `(符号名, Instance)`（`LowerCtxt::referenced`，含 drop glue / 函数项地址）；
  `codegen_crate` 在 CGU 分区编译后做 fixpoint：把"被引用但未发射"且
  `upstream_monomorphization(tcx).is_none()` 的实例补编进合成 `<on-demand>` CGU。
  修前 `-O1+` 的 sort 因 `CopyOnDrop::drop`/`revswap` 未进分区而缺符号。
- **LICM 默认关闭**（`FORGE_ENABLE_LICM=1` 可打开）：`-O2` 的
  `core::slice::sort` 在开启 LICM 时错值（`[1,9,14,14]`；`FORGE_OPT_SKIP=licm`
  正常），最窄复现是"只外提 Iconst"（`FORGE_LICM_ONLY=iconst`）。外提的支配性/
  插入位置/链依赖条件尚未证明，先以正确性优先；常量/地址计算留在原地成本极低。
  补全条件后转正（loops/licm.rs）。
- 守卫 `rustc_aarch64_sort_probe_runs_under_qemu` 升格为 **-O0/O1/O2 全档**（16 元
  乱序 sort_unstable 后校验连续，返回值 15）；arm64_exec 9/9。

**进度（2026-10-28，M6-124）——`Ord::cmp`/`binary_search`、负判别 switchInt**：

- 新增 `BinOp::Cmp` 降级（rustc 2026 的三路比较，`Ord::cmp` 直接用）：按符号性
  发 `icmp slt/ult`+`icmp sgt/ugt`，在 **i32** 里 `select` 出 -1/0/1 再 `ireduce` 到
  i8（arm64 的 `CSEL` 只有 32/64 位形态，i8 会落 `[cap:SPEC_LOWERING_RULE_MISSING]`）。
  浮点 `Cmp` fail-closed。
- **窄的有符号判别 switchInt**：`matches!(3.cmp(&7), Ordering::Less)` 在 -O0 恒假
  （exit 5 vs 7）——i8 的 -1 是 0xFF，而 A64 `CMPIMMW` 是 32 位比较（常量 -1 ⇒
  0xFFFF_FFFF）。判别类型窄且有符号时统一 `sextend` 到 i64、case 常量按低位符号
  扩展后再比。
- **按需补编收口**：缺失实例追加到 **CGU0**（不新增 CGU/对象）；补编队列要求
  `upstream_monomorphization == None` **且** `is_mir_available`（`extern` 声明 /
  上游非 inline 项不能补编，否则 `instance_mir` ICE）；空 outcome 不入计划。
- sort 守卫的探针源追加 `Ord::cmp` 三路比较断言（Greater/Less/Equal 都验，
  不通过返回 100+m）；三档（O0/O1/O2）仍应真执行 15。
- **新发现的待闭环缺陷**（kitchen 探针，尚未修）：
  - `-O0`：`v.iter().copied().filter(|x| …).sum()` **挂起**（`filter` 闭包在
    `next()` 里经间接调用，疑似闭包/ZST 调用路径在 -O0 出错）；
  - `-O2`：`parse_u32`（`&str` 逐字节循环）返回 0（-O0 正确）——`-O2` 的 MIR
    形态或某个优化档 pass 误编译，待二分（`FORGE_OPT_SKIP`）。

**进度（2026-10-28，M6-125）——胖指针常量 transmute（`str::as_bytes`）**：

- `-O2` 把 `str::as_bytes` 内联成 `_3 = copy _1 as &[u8] (Transmute)`，常量是
  `ConstValue::Slice { alloc_id, meta }`（胖指针：data 分配 + len）。旧实现的
  `lower_operand` 只认 `Indirect`/`Scalar(Ptr)`，Slice 落到标量兜底 ⇒ 写 `null`，
  `PtrMetadata` 读 len=0（实测 `"abcd".as_bytes().len()` 在 -O2 得 0、`parse_u32`
  恒返回 0）。
- 修法：赋值语句的 `Cast` 分支识别 `Operand::Constant(ConstValue::Slice)`，按
  ScalarPair 直接写目标槽（ptr@0 = `const_data_addr(alloc_id)`、len@8 = meta）——
  常量不在栈槽里，不能走 place 复制。
- sort 守卫探针追加 `"abcd".as_bytes().len()==4` 与首字节 `== b'a'` 断言
  （O0/O1/O2 都验）。
- **仍未修**：`-O0` 的 `iter().copied().filter(..).sum()` 挂起（闭包经间接调用，
  疑似 -O0 闭包/ZST 路径），已登记待办。

**仍未覆盖**：NEON 向量、DWARF/CFI、LICM 的错误外提（默认关闭、待修）、
`-O0` 的 `filter()` 闭包挂起。

## 2.8 P1：128 位 C-ABI 跨后端兼容（✅ 2026-10-24 收口）

目标：让 `forge` 与 **LLVM 编的外部函数**在 128 位标量上**双向互操作**
（验收 = 跨后端探针两个方向都真跑出 42）。

### 结论：形态问 `FnAbi` 的 `PassMode`，不问约定名

`rustc` 的 `FnAbi` 就是目标 ABI 在本机上的权威投影（实测，x86_64-pc-windows-msvc）：

| 边界 | Rust ABI（默认） | `extern "C"`（Win64） |
| --- | --- | --- |
| 实参 | `PassMode::Direct`（≡ 两个整数槽 lo/hi） | `PassMode::Indirect { pointee_size: 16 }`（按引用） |
| 返回 | `PassMode::Direct`（≡ `RAX:RDX`） | `PassMode::Cast { unit: Vector { size: 16 } }`（`XMM0`） |

⇒ 降级层**不写**"如果是 C 就按引用"这类判断：`abi::wide_int_abi` 读 `PassMode`，
`Indirect` ⇒ 传指针、`Cast{Vector}` ⇒ 按 `V128` 返回、`Direct` ⇒ 两整数槽。

### 实现落点（四处同源）

1. **形态查询**：`crates/tools/forge-rustc/src/abi.rs` 的 `wide_int_abi(tcx, instance)` 与
   `callee_wide_int_abi(tcx, func, body)`：先按被调方**实例**问，实例解析不出时退回
   **操作数类型上的 `FnSig`**（`wide_int_abi_of_callee_ty` → `fn_abi_of_fn_ptr`）——
   fn 指针与 `dyn Trait` 方法因此可用（M6-89）；两条都拿不到才 fail-closed。
2. **定义侧**（`lower/mod.rs` 的 `build_signature`）：128 位形参按形态落成"一个指针"或
   "两个 `i64`"；返回落成 `V128`（向量槽）或两个 `i64`。入口收参：指针形态用
   `copy_agg` 复制 16 字节进本地槽，返回用 `load_ty(base, V128)` 读一个 16 字节向量。
3. **调用侧**：实参 `Indirect` ⇒ 先把值复制到**16 字节对齐的临时槽**、传地址（即使实参
   本来是 place 也复制：LLVM 的 `Indirect` 带 `pointee_align: 16`，而对齐不足的字段
   ——`#[repr(packed)]`/相邻槽——直接用 `movdqa` 会 AV）；结果是向量槽 ⇒ 调用按
   `ret_tys = [V128]` 取回，直接 `store` 进 destination 槽（位模式语义），整数对时仍是
   `pack_sp`。临时槽在 `LowerCtxt::new` 按"一次调用最多的按引用宽实参个数"预分配。
4. **合法化层**：ABI 边界编组按**大小**（v27 M6-83/84），但**显式排除向量**——≤16B 向量
   的按值/按引用由发射侧的 `bank = "vec"` 负责，>16B 向量在 IR 里已经是 sret/指针；
   把向量当"宽聚合"会拆成两个 `i64` 结果（静默读 `RAX:RDX`）。

### 验收（代码为真）

- `crates/tools/forge-rustc/tests/cross_abi.rs`：**双向**跨后端真执行——
  forge 编调用方 + LLVM 编被调方；LLVM 编调用方 + forge 编被调方。三个函数覆盖
  "按引用实参 / 向量返回 / 两者同时"，期望 42。已进门禁（`rustc-abi-cross`）。
- e2e `u128_abi_param` / `u128_abi_return` 钉住 **Rust ABI 仍是 `RAX:RDX` 两槽**（不回归）。
- 实测（2026-10-24）：`cross_abi` 2/2、e2e 全量（`e2e_stage_a_scalar_cases`）、
  `forge-codegen --lib` 2864/2864、`forge-rustc` clippy 干净。
- **为什么系统 e2e 测不出这一类**：自编自调时两边用同一套形态（哪怕错的）也自洽；
  `WA-52` 之前 105 条用例全绿而混链必 AV，正是这个原因——跨后端门禁是唯一真值。

### 踩坑记录（过程细节见 archive M6-84…M6-88）

- 128 位按引用实参**不能复用聚合的拆段路径**（`AGG_ARG_INSERTVALUE_CHAIN`：宽标量值
  既不是 `load` 也不是可物化的聚合链）⇒ 需要自己的"临时槽 + 传地址"路径；
- 一次替换 17 处 `is_scalar_pair_abi` 曾把辅助函数自己的函数体也换掉 ⇒ 无限递归、
  rustc 线程栈溢出（不是 `abi_is_rust` 的问题）；**改判据时先加辅助函数、再逐个站点接**；
- 别用**调用方**的约定判被调方的形态（调用方 `extern "C"`、被调方 Rust ABI 时就错）。

### 纪律

- 边界形态只来自**目标 ABI 的数据**（`FnAbi` 的 `PassMode`）；寄存器/重定位仍只来自
  `forge-abi` 的规则数据（`Classifier`/`CallPlan`）。降级层不写约定名判断。
- 解析不到被调方就**明确失败**，不静默按某一个约定继续（那正是 `WA-52` 的成因）。

## 3. 历史记录（已归档）

2026-10-11 → 2026-10-19 的**逐轮记录、旧施工图、事故复盘**已归档到
[`docs/archive/forge-rustc-v27-milestones.md`](../archive/forge-rustc-v27-milestones.md)
（本文只留"现在到哪了"与"还差什么"）。

各里程碑的**能力决策**（做什么/不做什么/为什么）不在本文，在
[`docs/reference/backend-gaps.md`](../reference/backend-gaps.md) 的 `[cap:ID]` 台账里（条数以那份文档为准，别在此抄数字）
——那份由 `capability_gaps` 守卫与源码双向钉住。
