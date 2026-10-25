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

**仍未覆盖**：aarch64 core 的完整表面（见上）、NEON 向量、原子/宽乘除、DWARF/CFI。

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
