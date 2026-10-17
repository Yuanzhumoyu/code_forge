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

### M1 正确性清零：把绕法换成实现

**问题**：E3/E5/E6——测试默认关溢出检查，窄类型/向量溢出/regalloc 四处有静默错值风险。

- **溢出检查（`WA-05`）**：实现常规 `+`/`-`/`*` 的 `Assert` 语义（`OverflowOp`/`AssertKind` 分支
  要真的发射"溢出则 panic/abort"路径），然后**从 e2e 默认路径删掉 `-C overflow-checks=off`**。
- **窄有符号（`WA-36`）**：`i8`/`i16` load 后的符号扩展按**载体类型与源类型**共同决定，
  不是一律零扩展；补 e2e 负值用例（-50 参与比较/除法/转型）。
- **向量溢出（`WA-46`）**：FPR/向量 spill 宽度与类宽按**值宽度**（含 64B），不许恒 8 字节。
- **regalloc def-spill（`WA-40`）+ 错编（`WA-41`）**：`def` 落在写死物理寄存器的指令上时，
  要么禁止该 def 参与溢出、要么在溢出后修复写回；`WA-41` 的最小复现进 e2e 作为回归探针。
- **删除清单**：`overflow-checks=off`（默认路径 1 处；其余特例必须在注释里写明理由）、
  e2e 里剩下的 `known_failure: true`（1 条）。
**判据**：`rg -c 'overflow-checks=off' crates/tools/forge-rustc/tests` ==（特例数，且每条有理由注释）；
`rg -c 'known_failure:\s*true'` == 0；e2e 全绿（**硬断言**，无容忍名单）。

### M2 缺能力闭环：`global_asm` / 内联 `asm!` / 展开 / TLS

**问题**：E7——真实 crate 一定会碰这四样（`core`/`libc`/`std` 都碰）。

- **`global_asm!`（`WA-03`）**：把 `MonoItem::GlobalAsm` 的原文交给**汇编器**（生成对象文件的
  `.text` 段），参考 cranelift 后端的做法（[`global_asm.rs`](https://github.com/rust-lang/rust/blob/4b0c9d76/compiler/rustc_codegen_cranelift/src/global_asm.rs)）。
- **内联 `asm!`**：实现 `InlineAsm` 的发射（约束解析 → 操作数绑定 → 交给汇编器），
  先覆盖 x86_64 的 `reg`/`reg_class`/`const`/`sym`/`memory`/`options(nostack, nomem)`；
  参考 [`inline_asm.rs`](https://rust.googlesource.com/rust/+/d14c1c75ab284d382bd1e9c499596c274f1abe58/compiler/rustc_codegen_cranelift/src/inline_asm.rs)。
- **展开（`panic=unwind`）**：二选一，**必须明确**——(a) 发射 SEH 表（`.pdata`/`.xdata` +
  `__rust_eh_personality`）让 `panic=unwind` 可用；(b) 明确"不做"，并把 `backend.rs` 的拒绝
  信息写成**可诊断的缺口**（进 `backend-gaps.md` 台账 + `[cap:…]`）而不是内联字符串。
- **TLS（`#[thread_local]`）**：静态 TLS 变量 → 段与重定位（先 x86_64-msvc 的 `.tls$` 形态）。
**判据**：四类各有 e2e/真执行用例（`global_asm` 小函数、`asm!("nop")`/`asm!("add {0}, 1", inout(reg) x)`、
unwrap 触发 `catch_unwind` 或用例级 SEH 决策文档、`thread_local!` 读回值）。

### M3 `std` 可用性：让真实程序能编能跑

**问题**：今天只能 `no_std` + `no_main` + `#[no_mangle] extern "C" fn main`（README §支持矩阵），
`std` 程序（E2/E7 的 runtime 符号链）编不了 ⇒ "后端"用不起来。

- 打通 `std`：入口（`mainCRTStartup` → Rust `main`）、`panic` runtime、`alloc` runtime（与 `WA-02`
  的取舍合并）、`unwind`（依赖 M2 的决策）、`std::io` 最小闭环。
- 判据：`examples/forge-rustc-hello` 用 **`std`** 编 + 跑 + 校验输出；再加 **2 个真实 crate**
  （如 `serde`+`smallvec` 的 `-Zbuild-std` 路径，E5/`WA-19`）能编过并跑通。
- **删除清单**：`WA-02` 的手写 alloc shim（401 行）——若 rustc 侧实例可用就删手写那份，
  否则反过来删"过滤逻辑"并把取舍写进台账（**无冗余**：同一份 shim 不许存在两份）。

### M4 第二目标 + 对象/链接层根治

**问题**：E6/E11——`.rodata` 重定位绕法、单一目标面。

- **`.rodata` ADDR64（`WA-01`）**：在对象写者侧修正重定位（或文档化 + 由 lld-link 验证），
  让 vtable 表回到只读段；随之删掉 `add_data_with_relocs` 的"被迫写 `.data`"理由。
- **`share-generics`（`WA-04`）**：补"被引用但上游未生成"的实例，或明确不支持并让
  `-Zshare-generics=yes` **编译期报错**（不许 LNK2019 这种链接期才发现）。
- **第二目标**：把 rustc 后端接到 **aarch64 或 riscv64**（两者都有 QEMU 真执行通道），
  至少覆盖 M1 的 e2e 标量/调用子集；ABI 面复用 `forge-abi` 的四份约定。
**判据**：`.rodata` 用例（vtable）在新位置通过；第二目标的 e2e 子集在 QEMU 真跑绿。

### M5 契约与门禁收口

- **门禁**：`cargo forge gate` 加 `--subset rustc`（或步骤 `rustc-e2e`）——把 E9 的缺口补上，
  CI 复用同一命令。
- **文档守卫（治 E4 的根）**：README §支持矩阵的**用例计数**（`CASES` 条数、`known_failure`
  条数、`FLAKY` 名单）与代码**双向相等**，由新守卫 `rustc_doc_counts_guard` 钉住；
  `WORKAROUNDS.md` 的 `[WA-NN]` 编号唯一 + 代码里引用的编号必须存在（反向也查）。
- **台账化**：M2 里"不做"的项（若选 (b)）进 `reference/backend-gaps.md` 并带 `[cap:…]`；
  `WORKAROUNDS.md` 的 open 条目**清零或全部有明确条件**。
- 契约回写（README/CLAUDE.md/`docs/reference/*`）+ 账本归档（同 v23–v26 做法，单独 docs 提交）。

## 3. 顺序与窗口

`M1`（正确性，**先做**：它是"后端可信"的前提）→ `M2`（缺能力）→ `M3`（std 可用性，依赖 M2 的展开决策）
→ `M4`（第二目标，独立窗口）→ `M5`（收口）。M2 的展开决策必须**先落纸**再动手（否则会返工）。

## 4. 交付物与提交

1. 活账本 `docs/plans/forge-rustc-v27-plan.md`（首次提交）。
2. 代码：M1 四处正确性实现、M2 四类能力、M3 std 打通、M4 对象层 + 第二目标。
3. 守卫：`rustc_doc_counts_guard`（M5）、`workarounds_guard`（编号唯一 + 引用可解析）、
   以及各里程碑的 e2e 真值用例。
4. 每个里程碑一次提交（单 subject 长标题，`git commit -F target/tmp/*.txt`）。

## 5. 成功判据（整体）

- **硬指标**：`overflow-checks=off` 只出现在有理由的特例；`known_failure: true` == 0；
  `WORKAROUNDS.md` 的 open 条目 == 0 或全部带"数据条件"；README 的计数与代码双向相等。
- **可用性**：`std` 程序（含至少 2 个真实 crate）能编、能跑、输出正确。
- **目标面**：第二个架构的 rustc 后端在 QEMU 上跑通 M1 的 e2e 子集。
- **门禁**：`cargo forge gate --subset rustc` 一条命令覆盖后端真值测试。

## 6. 假设与不做清单

- **不做**（除非 M2 明确选 (a)）：Windows 之外的**展开**实现（ARM EH / SjLj）——先只做决策与台账。
- **不做**：为旧版 rustc 兼容（用户口径"无需兼容"）——只跟当前 `rust-toolchain.toml` 的 nightly。
- **假设**：`-Zcodegen-backend` 的 dylib 形态不变（rustc 侧接口随 nightly 漂移，属持续成本）。
- **风险**：M1 的 regalloc 修复落在**主库**（`forge-codegen`）——那是共享代码，改动要跑全套门禁，
  可能连带影响 JIT 矩阵与 asm 三档（这正是门禁存在的意义）。

## 7. 外部参考

- rustc 后端 trait 面：`rustc_codegen_ssa::traits::backend`
  （[`backend.rs`](https://rust.googlesource.com/rust/+/36f18f2d3a2df4a2ce0925f438631b615e2ab4dc/src/librustc_codegen_ssa/traits/backend.rs)
  ／[`traits/mod.rs`](https://rust.googlesource.com/rust/+/b3b32b74bcc767aae694c473e87d10cee9d0aff3/src/librustc_codegen_ssa/traits/mod.rs)）：
  完整后端要实现/接管的全部方法（含 `global_asm`、`inline_asm`、`link`、`join_codegen` 等）。
- Cranelift 后端的两个对照实现：
  [`global_asm.rs`](https://github.com/rust-lang/rust/blob/4b0c9d76/compiler/rustc_codegen_cranelift/src/global_asm.rs)
  与
  [`inline_asm.rs`](https://rust.googlesource.com/rust/+/d14c1c75ab284d382bd1e9c499596c274f1abe58/compiler/rustc_codegen_cranelift/src/inline_asm.rs)
  ——本方案 M2 的两项直接照它的形态做（"把原文/约束交给汇编器"）。
- 展开边表（unwind side tables）与 SEH 的关系：
  [LIACS 论文（2023–2024）](https://theses.liacs.nl/pdf/2023-2024-BaronBRBjrnRoy.pdf)
  ——M2 的 `panic=unwind` 决策要用（`.pdata`/`.xdata` 与 personality 的职责边界）。

## 8. 批次记录（逐里程碑回写）

### M1 正确性清零 —— **已收口（2026-10-11，工作区）**

**结论先说**：立项时按 `WORKAROUNDS.md` 的字面描述列的四个 M1 项（`WA-05`/`WA-36`/`WA-46`/
`WA-40`+`WA-41`）**逐条对代码与测试核实后，三项早已被历史修复关闭、一项只剩无法在本机复现的
CI 偶发**。M1 真正的缺口是**测量过程本身暴露的那个**：套件**无法表达"必须 trap"**——断言失败边
确实会分流（不是静默回绕），但 e2e 的 panic handler 是 `loop {}`、而 `run_exe_with_timeout` 把
"异常终止"与"超时"混在一个分支 ⇒ 溢出被拦**看起来像产物挂起**，没有任何硬断言。

逐项核实表（判据 = 代码/测试，不是文档）：

| 项 | 立项描述 | 实测结论 | 证据 |
| --- | --- | --- | --- |
| `WA-05` 溢出 Assert | "未覆盖、测试统一关 checks" | **早已实现**，描述过时 | `lower/mod.rs:847-899` 按 `expected` 分流 + 失败边调 `panic_impl`；默认路径**不传** `overflow-checks=off`（只有 `overflow_off*` 显式关）；`overflow_checks_on`/`_mul` 两例本就跑在 checks on 下 |
| `WA-36` 窄有符号 | "符号扩展未实现（i8 max 实证）" | **v14 已关闭**（修复链 ①–⑤ 在代码里） | `lower/rvalue.rs:47-75`：无符号窄源 mask、有符号窄源 `sextend`；`WORKAROUNDS.md` 该行标"已关闭（v14）" |
| `WA-46` 向量溢出宽度 | "恒按 8 字节搬运、64B 值类宽只有 32" | **生成级已修**，残余是硬件相关偶发 | `isa/x86.toml` 有 `[machine.spill.fpr16/32/64]` 三档 + `gen_fpr_spill_dispatch` 按 `width` 分派（未声明宽度 ⇒ 生成期 fail-closed）+ `reg_class_for` 的 `VEC(64)` 档；残余 `test_jit_v512_byref_param` 只在**有 AVX-512F 的 runner** 上真跑 ⇒ 本机无法复现，转入 M4 窗口 |
| `WA-40`/`WA-41` regalloc def-spill / 错编 | "def 被 spill 静默写坏槽 / 本机可复现错编" | **已关闭**（根因 = `WA-42` niche tag 写入宽度） | vec 族 5 例全部转正（`known_failure: false`，reason 记 CI run 93927221004）；本机全量 e2e 106/106 |

M1 实际交付（本轮）：

1. **trap 语义**（`tests/e2e.rs`，破坏性）：panic handler `loop {}` → `ud2`；`Case` 新增
   `expect_trap`；`run_exe_with_timeout` 把三态（正常/异常终止/超时）分开；新增 `is_trap_code`
   按 **NTSTATUS severity** 判异常；新用例 `overflow_runtime_trap` 把 `expected` 钉到
   **`0xC000_001D`**（`ud2` 的 NTSTATUS）。
2. **不变量守卫** `e2e_trap_semantics_invariants`：钉住三件"改坏了不会红"的事——handler 必须
   是 `ud2`、代码里不许有 `known_failure: true`、每个 `expect_trap` 用例必须钉一个真实 trap 码
   （≥ `0xC0000000`）。**它自己踩了两次自我命中**（守卫扫描自己文件时命中自己写的字面量）——
   与 v26 事件守卫同源，改用 `concat!` 在编译期拼针解决。
3. **台账纠偏**：`WORKAROUNDS.md` 的 `WA-05` 改判为已关闭（附判据）、`WA-32` 状态词显式化、
   `WA-46` 补上"生成级已就位 + 残余归 M4"的复核结论；本方案 §1 的 E5 加"已被 M1 推翻"的标注。

判据达成：全量 e2e **`passed: 106/106`**、零 `KNOWN`、95 s；代码里 `known_failure: true` = **0**；
新守卫绿。删除清单：`expect_trap` 之前无表达力（0 → 1 个硬断言用例）、`loop {}` handler（1 处）、
`WA-05` 的陈旧描述（1 行）。

教训（写给后续里程碑）：`WORKAROUNDS.md` 的"根治方向"列**不是现状**——它记录的是**写那一行时**
的判断；下游方案若照抄它做计划，会把早已关闭的项当缺口（本轮 E5/E6 就是这样）。**M5 的
`workarounds_guard` 必须同时检查"状态词与代码引用"**，并给每行加**机器可读的状态列**。

### M1 侦察（2026-10-11，工作区，未提交）

**目的**：动手前先把"绕法"的现状量准（避免照抄 WORKAROUNDS 的旧描述）。

实测发现（三条，都推翻了立项时引用的文档口径）：

1. **`WA-05` 的描述已过时**：`TerminatorKind::Assert` **已经实现**——
   `lower/mod.rs:847-899` 会按 `expected` 分流（`branch(cond, succ, fail)`），失败边**调
   `panic_impl`（`#[panic_handler]`）后 `unreachable`**；`lower/mod.rs:219` 的 CFG 预建也走
   success target。e2e 里还有两条**在 overflow-checks 打开（默认）下运行**的用例：
   `overflow_checks_on`（`black_box(1000i32) + black_box(2000i32)` == 3000）与
   `overflow_checks_on_mul`（7×6 == 42），均 `known_failure: false`。
   ⇒ **真正的残余不是"Assert 没实现"**，而是两件事：
   (a) **溢出发生时的路径从未被测试**（现用例只走不溢出路径；注释自己写明"常量溢出在
   const-eval 拦截"⇒ 运行期溢出触发 `panic_const_*` 的那条路没有真值用例）；
   (b) 套件的默认编译参数仍带 `-C overflow-checks=off`（`e2e.rs:1584` 等 6 处）
   ⇒ 默认口径下"溢出必须被拦"这件事**没有门禁**。
2. **e2e 计数以代码为真**：`CASES` = **105** 条、`known_failure: true` = **1**、
   `expect_compile_fail: true` = **2**；README §支持矩阵仍写"**103 用例：98 硬断言 + 5 个
   `FLAKY` 容忍项**"⇒ **文档与代码已经分叉**（M5 的 `rustc_doc_counts_guard` 正是治它）。
3. **`WA-36` 的落点已定位**：`lower/rvalue.rs:56` 的注释"符号扩展依赖 ireduce 后的使用未实现
   （i8 max 实证）"——窄有符号（`i8`/`i16`）符号扩展就是 M1 的第四项。

下一步（M1 的四个动作，按可测性排序）：

| 动作 | 站点 | 判据 |
| --- | --- | --- |
| 运行期溢出必须被拦 | `lower/mod.rs:847` + e2e | 新增负向用例：运行期溢出 + `#[panic_handler]` ⇒ 必须 abort/panic 而非静默回绕；随后**默认路径去掉 `overflow-checks=off`** |
| 窄有符号符号扩展 | `lower/rvalue.rs:56` + `i8`/`i16` load 路径 | 新增用例：`-50i8` 参与比较/除法/转型读回负值 |
| 向量/FPR 溢出宽度 | `WA-46`（主库 `[spill.FPR]` + 类宽） | 溢出 32B/64B 向量后读回逐 lane 相等 |
| regalloc def-spill + 错编 | `WA-40` / `WA-41`（主库 `regalloc_bt.rs`） | `WA-41` 的最小复现进 e2e；`Vec::new(); push` 读回正确 |

> 说明：四条里前三条的站点在 **forge-rustc**（后端侧），第四条在**主库** `forge-codegen`
> （共享代码 ⇒ 改完必须跑全套门禁 + 两条 JIT 矩阵，见 §6 风险）。

M1 第一项已测得结论（同轮，探针实测）：

新增探针用例 `overflow_runtime_probe`（`black_box(i32::MAX) + black_box(1)`，
`FORGE_E2E_ONLY=overflow_runtime_probe` 单跑）实测：

```text
RETRY overflow_runtime_probe 首次运行超时（疑宿主侧起进程/映像延迟），同产物复跑一次
KNOWN overflow_runtime_probe error: timeout（挂起：20 s 内未退出——可能 assert 失败进入 panic loop）
```

⇒ **判定：`Assert` 失败边真的分流出去了**（**不是**静默回绕——回绕会退出 `-2147483648`）。
`WA-05` 的"Assert 分支未覆盖"确实是过时描述。真正的残余是**两件可落地的事**：

1. **trap 语义没有表达力**：e2e 的 `#[panic_handler]` 是 `loop {}` ⇒ "溢出被拦"表现为
   **挂起**，harness 只能记 timeout；要把它变成**硬断言**，得给 `Case` 加
   `expect_trap`（异常终止 / 平台 abort 码即算过）+ 让 panic 路径 `abort`
   （`core::intrinsics::abort()` 在 no_std nightly 可用）。这是**破坏性**改动（允许）。
2. **默认口径仍关检查**：6 处 `-C overflow-checks=off`（含默认编译路径）⇒ 溢出拦截在默认
   门禁里**不可见**；trap 用例落地后要删掉默认路径那一处。

附带踩坑（写给下一轮）：`FORGE_E2E_ONLY=<已知失败用例>` 单跑会**报 `no cases passed —
backend broken` 硬失败**（harness 要求至少一条通过）——探针用例要单跑，必须同时带上一条通过的
用例名，或先把它转成硬断言用例。本轮因此多花了一次 20 s×2 的超时。

M1-① trap 语义：已落地并实测（同轮）

改动（`crates/tools/forge-rustc/tests/e2e.rs`，破坏性）：① 模板的 `#[panic_handler]` 从
`loop {}` 改成 `unsafe { core::arch::asm!("ud2", options(noreturn)) }`（真异常终止）；
② `Case` 加 **`expect_trap: bool`**（106 个用例字面量 + 数组外的诊断探针全部补字段）；
③ `run_case_with` 把退出结局分三态并按 `expect_trap` 归一，**保留真实 NTSTATUS**；
④ 新增 `is_trap_code`（NTSTATUS severity：`0xC0000000` 及以上算异常终止）。

实测两坑（都已写进代码注释，别再踩）：：

1. **Windows 上 `ud2` 是"有码"退出**：`status.code()` = `Some(-1073741795)` = `0xC000001D`
   （STATUS_ILLEGAL_INSTRUCTION），**不是** `None` ⇒ "`code() == None` 即 trap" 的假设
   **第一次跑就被证伪**；判定必须按 NTSTATUS severity。
2. **哨兵值会与合法退出码撞车**：最初把 trap 归一成 `TRAP_EXIT = i32::MIN`，而
   `i32::MAX + 1` 的**回绕值恰好就是 `i32::MIN`** ⇒ "溢出 trap"与"静默回绕"在判定上无法区分
   （该用例形同虚设）。现在：归一保留**真实 NTSTATUS**、用例直接钉 `expected: 0xC000_001D`；
   `TRAP_EXIT` 只作"拿不到码（宿主强行结束）"的兜底，取值落在 trap 区间内（`0xC000_0000`）。

判据达成：：`PASS overflow_runtime_trap exit=-1073741795`（`FORGE_E2E_ONLY` 单跑，1/106）；
静默回绕会正常退出 `-2147483648`（≠ 任何 trap 码）⇒ 必红；断言失败边不触发（挂起）⇒ 也必红。

**全量验收（同轮实测）**：`cargo test -p forge-rustc --test e2e -- --nocapture
--test-threads=1 e2e_stage_a_scalar_cases` ⇒ **`passed: 106/106`**、`test result: ok`、
**零 `KNOWN`**、95 s（单机、串行）⇒ ① 改造对既有 105 例无回归；② 代码里
`known_failure: true` **已归零**（M1 的一条硬判据达成——最后一个是本里程碑的溢出探针，
现已转正为硬断言；`rg` 计数时注意排除文档注释里那处 `known_failure:true` 的历史说明）。

遗留（下一轮）：：`run_exe_with_timeout` 的 `Some(None)`（宿主强行结束）现在归成 `TRAP_EXIT`；
`CI_ENV_AV_CODE`（`0xC0000005`）这条历史容忍码在新语义下走 trap 分支——`FLAKY` 名单为空且只在
`FORGE_E2E_STRICT_FLAKY=1` 时有意义，故当前无影响，但 **M5 的守卫要把它钉住**（否则将来把 AV
当"环境性"容忍会重新走进这条路）。
