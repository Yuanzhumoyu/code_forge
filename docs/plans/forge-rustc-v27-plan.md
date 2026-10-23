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
| **P2 `INLINE_ASM` 的 `sym`/`label`** | 📋 **结论已定、待实现**：需在谱面层做（`[operand.sym]` + 汇编器块内标签），排在 P1 之后；现状 fail-closed | §2.6；台账 `INLINE_ASM` |
| **P3 M4-T3 第二目标** | ⛔ **执行级受环境限制**（本机无 emulator / aarch64 std / 交叉 gcc-ld）；**对象级可做**（clang/ld.lld/llvm-objdump 齐备） | §2.7 实测表 |
| **P4 分配器事件化** | 🔶 **首块落地**：`SPILL-VALUE`（逐值 vreg/offset/size）+ `Field::I64`；下一步寄存器压力峰值与逐指令编辑 | §8 M6-87；`events_guard` 三方双向 |
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
| M5 契约与门禁 | 🔶 **进行中**：门禁 / 子集骨架已在；`i128`/`u128` 已闭环；浮点比较语义已实测清零；余项见下方清单 | 见下方"未闭环清单" |
| M6-① 清理 | ✅ **已完成（2026-10-18）**：`forge-rustc` clippy 清零并进全量门禁 | `gate` 的 `rustc-clippy` 步骤 |
| M6-② 账本归档 + 参考全扫 | ✅ **已完成（2026-10-19）**：本文收缩成"状态表 + 未闭环清单" | `docs/archive/forge-rustc-v27-milestones.md` |
| M6-③ 代码生成选项 fail-closed | ✅ **已完成（2026-10-19）**：靠 LLVM 管线的 `-C` 选项统一拒绝 | 台账 `CODEGEN_OPTS` |
| M6-④ IR 优化管线接线 | ✅ **已完成（2026-10-19）**：按 `-C opt-level` 默认开启，O2 与 LLVM 持平 | 台账 `CODEGEN_OPTS`；archive M6-④ |
| M6-⑤ 契约面 fail-closed 扩展 | ✅ **已完成（2026-10-19）**：`CALL_CONV`（非 Win64 约定拒绝；此前 `sysv64` 被当 C 打包）、`VARARGS_DEF`（C 变参定义拒绝；此前会让 rustc ICE）、`EMIT_KINDS`（`asm`/`llvm-ir`/`llvm-bc` 拒绝；此前"退出码 0 但没产物"） | 台账三行 + e2e `neg_sysv64` / `neg_c_variadic` / `flag_emit_asm` |
| M6-⑥ 优化档 MIR 形态覆盖 | ✅ **已完成（2026-10-19）**：e2e 新增 `opt_*` 机制（注入 `-C opt-level=2`），一次抓出 6 个只在优化档出现的真缺陷并全部修复 | e2e 8 条 `opt_*`；archive M6-⑦/⑧ |
| M6-⑦ 活区间正确性 | ✅ **已完成（2026-10-19）**：两处区间**空洞**（同块定义→使用、跨块只标两点）修成正区间；调用点接入**约定级破坏集**（此前 `Call` 一条 clobber 都没有） | archive M6-⑫ |
| M6-⑧ 不变量守卫 | ✅ **已完成（2026-10-19）**：分配重叠自检、字段来源检查、溢出槽初始化探针 + 机器级 dump/隔离开关 | `regalloc_bt/verify.rs`、`pipeline/field_check.rs`、`pipeline/spill_init.rs` |
| M6-⑨ `-o` 与产物种类语义 | ✅ **已完成（2026-10-19）**：`--emit=obj -o <path>` 此前静默不生效 ⇒ 复制到 rustc 请求的路径；`--out-dir` 形态保留固定名 | `tests/object_output.rs`；archive M6-⑭/⑱ |
| M6-⑩ 已知后端缺陷（**未闭合**） | 🔶 **两条同源**：`spill_vreg` 驱逐活值时不产生机器指令（槽可能是未初始化内容）；允许 LICM 外提 `StackAddr` 时某形状产出垃圾值 / AV。**当前以 `LICM` 不外提 `StackAddr` 规避（用户侧无暴露）**，正解是"分配器事件化" | archive M6-⑤…⑰ 的试改回退记录 |
| M6-⑪ 宽/窄整数溢出语义 | ✅ **已完成（2026-10-19）**：窄（8/16 位）加/减/乘的溢出标志不再依赖 x86 的 CF/OF——由 **legalize 的新 pass `narrow_overflow`** 统一改写成"扩宽算 + 比较区间 + 截回"（`forge-rustc` 里那份等价实现已删，语义只剩一处）。`-Coverflow-checks=on` 在**真 std 语料**上 O0/O2 都与 LLVM 逐行相同 | e2e 5 条 `oc_*`；JIT `jit_uadd_overflow_flag_is_correct`；archive M6-㊽…53 |
| M6-⑫ `()`/ZST 返回崩溃 | ✅ **已完成（2026-10-19）**：`TerminatorKind::Return` 对零大小返回类型此前发 `Load(void)` + 带 1 个操作数的 `Ret` ⇒ IR 校验 Internal error（`core::intrinsics::cold_path` 必崩，任何开溢出检查的 crate 都会撞上） | archive M6-㊴ |
| M6-⑬ 门禁自洽与真 std 语料 | ✅ **已完成（2026-10-19）**：`cli_tests` 夹具改用工作区 `CARGO_HOME` + `target/cli-tests`（沙箱可写）；`rustc-std` 步骤进全套门禁（forge 编 vs LLVM 编逐行对拍，默认 O0/O2）；全套 **51 步 0 失败** | archive M6-㊱/㊲ |
| M6-⑭ `cargo test` 能力 | ✅ **已实测（2026-10-19）**：`cargo test -Z panic-abort-tests` 在 forge 编的夹具工程上跑通（默认 `cargo test` 因 libtest 要 unwind 而与 `[cap:UNWIND]` 冲突，属明确拒绝而非缺陷） | README「用法」；archive M6-51 |

**M5 未闭环清单（2026-10-11 状态修正；**M5 未完成**，不提交 PR）**：

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
- `label`/`sym` 仍 fail-closed（前者受"指令流内标签"的结构限制，后者受谱汇编器文法限制）。
- **零容忍口径现在是"实测过的 0"，不是"挡在套件外的 0"**：`known_failure: true` 计数 = 0，
  且新加的浮点条件矩阵是**逐条件**断言（不是代表性抽样）。

## 2.6 P2 决策：`INLINE_ASM` 的 `sym` / `label`（2026-10-23）

现状（见 `docs/reference/backend-gaps.md` 的 `INLINE_ASM` 行）：S1–S4 的可用面已落地
（纯文本、`in`/`out`/`inout`+`reg`、`const`、`clobber_abi`、`options(noreturn)`），
**`sym` 与 `label` 仍 fail-closed**（`[cap:INLINE_ASM]`，明确报错而非静默错值）。

两条都**不是**"在降级层补一段字符串拼接"能解决的，它们要动的是**谱面汇编器**这一层：

| 操作数 | 缺什么 | 正解（按层） | 成本 |
| --- | --- | --- | --- |
| `sym` | 谱面汇编器的操作数文法只吃寄存器/数值字面量（`[operand.*]` 的 `kind = imm/mem/gpr/...`），符号没有对应的槽 | 新增操作数种类 `[operand.sym]`（`kind = "sym"`）+ 汇编器把标识符 token 绑到该槽 + 该槽带 `reloc = "abs64"`（x86 已有 `MOVABS_GLOBAL` 可承接）+ 降级层把 `InlineAsmOperand::Sym` 渲染成符号文本并登记重定位 | 中（DSL 模型/解析/生成 + 一处降级层 + e2e） |
| `label` | 标签必须落在**指令流中间**，而现在的形态是"文本 → 一次汇编 → 一串指令" | 汇编器支持**块内标签**：文本里 `L: ... jmp L` 在**同一次汇编**里解析成相对偏移（不需要跨 IR 块的 `LabelRef`）；rustc 的 `label` 操作数 `{0}` 只要绑定成"本块内的标签 id"即可 | 中（汇编器加标签表 + 分支绑定） |

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
