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
| M1 正确性清零（溢出/窄有符号/向量溢出/regalloc） | ✅ 落地 | §8 M1；e2e 硬断言、`known_failure` = 0 |
| M2 `global_asm!` | ✅ 落地 | §8 M2；e2e `global_asm_call`/`global_asm_sym_const` |
| M2 展开（`panic=unwind`/SEH） | ❌ **不做**（明确拒绝） | 台账 `UNWIND` |
| M2 TLS | ✅ 落地（动态 TLS） | §8 TLS；e2e `thread_local_roundtrip` |
| M2.5 S1 内联 asm 骨架 | ✅ 落地 | §8 S1；e2e `inline_asm_nop`/`inline_asm_ud2` |
| M2.5 S2a 寄存器约束（`reg`） | ✅ 落地 | e2e `inline_asm_reg_inout` |
| M2.5 S2b 显式寄存器名（含短名映射） | ✅ 落地 | e2e `inline_asm_named_reg`；`reg_name_table` 守卫 |
| M2.5 S3 `const` 占位符 | ✅ 落地 | e2e `inline_asm_const` |
| M2.5 S3 `sym` 占位符 | ❌ **受 ISA 限制**：本谱汇编模板无法表达符号操作数 ⇒ 即使做了也**无法验证**、且编不过 ⇒ 保持 fail-closed | §8 S3 记录 |
| M2.5 S4 `clobber_abi` | ✅ 落地（纯 clobber 槽） | e2e `inline_asm_clobber_abi` |
| M2.5 S4 `options(noreturn)` | ✅ 落地 | e2e `inline_asm_noreturn` |
| M2.5 S4 `label` 操作数 | ❌ **结构性阻塞**：标签必须落在指令流**中间**，当前"文本→一次汇编→一串指令"表达不了 | §8 的阻塞分析 |
| M3-① alloc 按需注入 | ✅ 落地 | §8；`WA-02` 关闭 |
| M3-② 真跑 `std` | ⏳ **卡在环境**：需要 `-Zbuild-std` + 网络（本仓沙箱无网络凭证） | §8；`cargo forge init --std` 已就位 |
| M4-T1 目标校验（不再静默回落） | ✅ 落地 | 台账 `TARGET`；§8 M4-T1 |
| M4-T2 约定按目标选 | ✅ **实测已成立**（双向守卫） | `abi_target_real` 的 `abstract_c_…` |
| M4-T3 目标相关对象层/链接层 | ❌ 未做（需交叉工具链验证） | §8 M4 施工图 |
| M4-T4 对象层根治（`WA-01`） | ✅ 首结论：**按设计关闭**；剩余项见 §8 | `WORKAROUNDS.md` `WA-01` |
| M5 契约与门禁 | ✅ 落地 | §8；`--subset rustc` 步骤表 |
| M6-① 清理 | 🔶 部分（`forge-rustc` 测试目标的 clippy 仍未纳入） | §8 |
| M6-② 账本归档 + 参考全扫 | ⏳ 待做（前置是各里程碑收口） | — |

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

### M2.5 内联 `asm!` 的真实现（**从 M2 拆出的专项**，S1–S4）

**为什么拆出来**：M2 的其余四项都已在"落地或明确降级"的口径下闭环（见 §8）；
内联 asm 与它们不同——它**不能降级**（`asm!` 在真实 crate 里随处可见，长期 fail-closed
等于把后端锁死在玩具规模），而真实现需要**跨 crate 的 IR 变更**（路线 A：
`forge-ir` 新增「原始文本 + 操作数约束」节点 + 发射期汇编），工作量与风险都独立成一级。
设计（三路线取舍 + S1–S4 分期 + 每期判据）见 §8 的「M2 剩余项的设计」一节。

### M3 `std` 可用性：让真实程序能编能跑

**M3-① 已落地（2026-10-11）：alloc 运行时改为按需注入**（`WA-02` 的正解，也是 std 的前置条件）

- 现状（落地前）：后端在**每个 crate 的首个 CGU 无条件注入**手写 alloc shim
  （`__rust_alloc`/`dealloc`/`realloc`/`alloc_zeroed`，基于 `VirtualAlloc`）；链 `std` 时
  std 的 rlib **自带同一族符号** ⇒ 必然 LNK2005 重复定义。
- 落地：用 `tcx.crates(())` 判定"是否链了 std"（与本文件既有的 `crate_name` 惯用法同源），
  **链 std ⇒ 不注入**；no_std ⇒ 照旧注入（那正是这个 shim 存在的理由）。
  原来的做法是反过来的——用 `keep_item` 过滤 rustc 侧的 alloc 实例，既绕、又挡不住 std 场景。
- 回归：全量 e2e（110 用例，全部 no_std ⇒ 仍注入）见 §8 的实测。
- **下一步（M3-②）**：真正跑一个 `std` 程序。已确认的可行路径与已知阻塞：
  1. `-Zbuild-std=std,panic_abort` 需要在 workspace 内建 std（`rust-src` 已装 ✔，不写 `~/.cargo`）；
  2. 若要让**用户 crate** 用 forge、**sysroot crate** 用 LLVM，需要 README 提到的
     `forge-rustc-wrapper`（`cargo forge init` 会写进 `.cargo/config.toml`）；
  3. 立项时的三项阻塞**已解除两项**（2026-10-11 校准）：TLS ✅ 已实现（动态 TLS），
     内联 `asm!` ✅ 已实现（S1/S2a/S2b/S3-const/S4）；剩下的未知是 std 的
     `lang_start` 入口链（`mainCRTStartup` → `std::rt::lang_start`）与 build-std 的网络依赖。
     跑法已固定成一条命令：`cargo forge init --std`。

### TLS 真实现 —— **动态模型已落地**（2026-10-11，工作区）

> 上一轮把 TLS 判成"不做（附条件）"，条件就是"推进 std 前必须做动态路"。本轮直接把
> 动态路做完了，**该决策已被实现取代**（保留下面这段历史记录，便于理解为什么选动态模型）。

- **为什么动态模型**：静态 TLS（`.tls$` + `gs:[58]`）需要 IR 支持**段前缀**，
  而 `MemRef` 只有 base/disp/index/scale；动态模型（`TlsAlloc`/`TlsGetValue`/`TlsSetValue`，
  kernel32 已链）**不需要新的 IR 能力**，代价是每次访问一次函数调用。
- **落地**（新增 `src/tls_runtime.rs`，约 140 行）：
  - 每个 TLS 静态发**两个隐藏数据符号**（命名确定 ⇒ 降级侧不查表）：
    `__forge_tls_init_<sym>`（初值模板）与 `__forge_tls_slot_<sym>`（索引槽，0 = 未分配）；
  - 运行时 `__forge_tls_addr(slot, size, align, init)`：槽里存 `TlsAlloc` **索引 + 1**（0 保留为
    "未分配"）⇒ 首次访问为**当前线程**分配 + 拷贝初值 + `TlsSetValue`，之后直接返回该线程块；
    `TlsAlloc` 失败（索引耗尽）⇒ `ExitProcess(3)`（与 `abort()` 同码，不做半吊子降级）；
  - 地址物化统一在两个入口：`lower::const_data_addr`（const/静态地址）与
    **`Rvalue::ThreadLocalRef`**（rustc 对 `&TLS` 用的专门 rvalue——**这是实测发现的**：
    第一版只改 `const_data_addr`，报 `unsupported rvalue: &/*tls*/ mut X`）；
  - `FuncRefTable` 新增 `intern_global_named`（按符号名取 GlobalId——隐藏符号不是 `AllocId`）。
- **实测踩坑**：`create_block_with_params` **会切换当前块**（它的 doc 却写"不切换"，
  实际调用的 `create_block_with_tys` 里有一行 `self.cur_block = block`）⇒ 运行时函数的入口块
  丢了终结符（`block 0 has no explicit terminator`）。处置：建完所有块后**显式切回入口块**。
- **边界（写清，不静默）**：每静态独占一个 TLS 索引（不共享槽）、无析构、
  索引耗尽即终止进程；静态 `.tls$` 模型（更快）留作将来优化。
- **e2e**：`thread_local_roundtrip`（`X: 0 初值写 7` + `Y: 5 初值 +2` ⇒ **exit=14**）——
  同时钉住"零初值可用""非零初值模板真的被拷贝""读回的是本线程块"三件事。
  负向用例 `neg_thread_local` 随之**删除**（能力已落地，它的断言不再成立）。

### TLS 的决策（2026-10-11 实测复核后定为 **不做**，附条件 —— **已被上面的实现取代，留作历史**）

v27 M2 曾把 TLS 记成"待做（先 fail-closed）"。本轮把两条实现路线都量到了具体代价，
按"宁可明确拒绝，不要半途的错语义"的口径**改判为 `不做`**，并把条件写清：

**路线 A：静态 TLS（`.tls$` 段 + TEB 相对寻址）**

- 需要的机器码形态是 `mov rax, gs:[58]`（TEB→ThreadLocalStoragePointer）+ `[rax + 8*_tls_index + off]`；
- **阻塞点**：我们的 `MemRef` 只有 base/disp/index/scale，**没有段前缀（`gs:`）**，IR 与谱都表达不了
  ⇒ 要先扩 IR（一个新地址形态）+ 谱模板 + 编码器，连锁面很大。

**路线 B：动态 TLS（`TlsAlloc`/`TlsGetValue`/`TlsSetValue`，kernel32 已链）**

- 语义正确、**不需要**新的 IR 能力（就是一个运行时函数 + 调用），代价是每次访问一次函数调用；
- 实测确认的改动面：① 生成运行时
  `__forge_tls_addr(slot, size, align, init) -> ptr`（首触 `TlsAlloc` + `TlsGetValue` 判空 +
  `__rust_alloc` + `memcpy` + `TlsSetValue`，约 8 个基本块，全用现有 builder 能力）；
  ② 每个 TLS 静态发**两个隐藏数据符号**（初值模板 `__forge_tls_init_<sym>` 与索引槽
  `__forge_tls_slot_<sym>`，命名确定 ⇒ 降级侧不必查表）；③ 把"静态地址物化"统一到一个 helper——
  但 `intern_const_data` 现有 **4 个调用点**（`rvalue.rs`、`mod.rs`、`statement.rs` ×2），
  且其中两处用它做 slice 字节/promoted 路径 ⇒ 返回类型要改成枚举、四处同步；
  ④ `FuncRefTable` 需要一个"按符号名取 GlobalId"的入口（现有 `intern_global` 按 `alloc_id` 键）。
- 估算：**约两轮**（含调试），且中途若停在半成品，`#[thread_local]` 会"能编过但语义可疑"——
  比现在**明确拒绝**更糟。

**结论**：在本专项里 **不做**，保持 `[cap:TLS]` fail-closed（诊断里现已写明"会静默丢线程私有语义"）。
**条件（什么时候重启）**：只要决定推进 `std` 可用性（M3-②），就必须先做路线 B——因为 `std` 的
`thread_local!` 是硬依赖；届时按上面 ①–④ 的顺序做，并把本期 4 个调用点当作既有清单。

### 收口待办（M6）

- **forge-rustc 的测试代码此前没有 lint 标准**：门禁的 clippy 只把 forge-rustc 当 **path 依赖**
  （只 lint lib），它的 `tests/*.rs` 从未被 `-D warnings` 覆盖。实测 `cargo clippy -p forge-rustc
  --all-targets -- -D warnings` 会报若干**既有** lint（`determinism.rs` 的 doc 列表缩进、
  `is_multiple_of`、冗余 cast 等，**不是 v27 引入的**）⇒ 收口时：先修这些既有 lint，再给门禁
  加一步 `clippy -p forge-rustc --all-targets`（让它与其余 crate 同标准）。
- **`-Zshare-generics=yes`（`WA-04`）**：现在仍是"链接期才发现的 LNK2019"⇒ 收口时可改为
  **编译期明确拒绝**（带缺口 ID）或补齐"被引用但上游未生成"的实例。

### M5 缺陷台账（2026-10-11 审计）：窄类型 / 转换三处（**可复现**，含 LLVM 对照值）

继续用"探未覆盖语义族"的方法，这一轮探到三处缺陷。**都没进 e2e 套件**——套件是**零容忍**
（`known_failure` 必须 0），带缺陷的用例不能提交；以"复现体 + 对照值"记在这里，修好再转用例。

#### ① 窄有符号右移错值（`Sshr` 规则）—— 错值，最优先

- 复现：`let a: i8 = -128; a >> 7` ⇒ 后端 **1**，LLVM **-1**；`i16 >> 15` 同理（分位诊断掩码 9 = {bit1, bit8}）。
- 同一条里**无符号**是对的：`u8 >> 7` = 1、`u16 >> 15` = 1 ✔。
- 已排除"窄加载不符号扩展"：`let a: i8 = -128; (a as i64) + 200` ⇒ **72** ✔（加载正确）。
- 可疑点（未证实）：`isa/x86.toml` 的 `[[lower.Sshr]]` =
  `["movsxd {out}, {0}", "mov RCX, {1}", "SAR_RM_CL {out}"]` —— 序列里**硬写 RCX**；
  窄类型寄存器池更紧，`{out}` 更可能被分配到 RCX 而被 `mov RCX, {1}` 破坏。

#### ② 浮点→整数转换缺 lowering 规则

- 复现：`3.99f64 as u8`、`-2.5f32 as i8` 之类 ⇒ 编译期
  `[cap:SPEC_LOWERING_RULE_MISSING] DSL lowering: Fptosi no matching rule`。
- 事实：x86 只有 32/64 位目的宽度的 `cvttss2si`/`cvttsd2si` ⇒ 8/16 位目的必须"先转 32/64 位再截断"，
  属**谱侧规则序列**要补的（不是后端问题）。

#### ③ 窄有符号算术

与 ① 同源（同一条 `mov RCX` 破坏面）的可能性较大。

**修复顺序**：①（错值优先）→ ③（可能同源）→ ②（补谱规则）。修好后三条都转成 e2e 用例
（届时用例数 +3）。

### M5 补面之七（2026-10-11）：一次**被用例否掉**的过界改动（`map_type` 兜底）

- **动机**：上轮 `u128` 的根因是"兜底 `_ => PTR` 把未识别类型静默当指针"。想把兜底改成
  **白名单**（只有 `Ref/RawPtr/FnPtr/FnDef/Dynamic/Str` 走 PTR，其余拒绝）。
- **实测结果**：全量 e2e **12+ 条红**（`nested_struct`/`checked_tuple`/`write_bytes_loop`/
  `box_value`/`box_write`/`vec_push`/`vec_string`/`vec_from_slice`/`closure_capture`/
  `closure_fnmut`/`drop_glue`/`checked_destructure`）⇒ **兜底是 load-bearing 的**：
  聚合/闭包/分配器那些形态确实走 `map_type` 拿"指针形状"的表示，白名单把它们误杀了。
- **结论**：兜底**不是** bug；`u128` 的问题在于"**标量**落进了兜底"。精确的防线是
  **只钉标量**——已在兜底分支加 `debug_assert!`（标量 ⇒ 直接炸；debug 构建就是 e2e，
  release 不受影响），并保留上轮 `Int/Uint(I128)` 的显式拒绝。回退后全量 **122/122 绿**。
- **教训**：审计要**先测再改**；"看起来更严格的兜底"可能正在承载别的语义。这也是本轮
  唯一一次"改动被自己的用例否掉"——但结论比改动值钱：现在有断言把**这一类**（标量静默
  变指针）永久钉住。

### M5 补面之六（2026-10-11）：**`u128`/`i128` 静默错值**——审计抓到的最大一条

- **怎么发现的**：想给 128 位整数补个用例（此前**零覆盖**），写完真跑 ⇒ 返回 1 而不是 42。
  用位掩码定位 ⇒ 高/低半取值全错，但"移位往返"和"取模恒等式"却是对的。
  再单独取一半 ⇒ `(1u128 << 64) + 5` 的**低半是 6**（应为 5）。
- **根因**：`map_type` 里**没有** `I128/U128` 分支 ⇒ 落到兜底分支被当成 **`PTR`（64 位）**
  ⇒ `1u128 << 64` 按"64 位移位"的计数掩码变成 `1 << (64 & 63) = 1`，于是整条链静默错值。
  这是"**静默错值**"里最典型的一种：能编、能跑、答案错。
- **处置**：`map_type` 显式拒绝 `Int(I128)`/`Uint(U128)`，缺口 ID **`WIDE_INT`**
  （台账行：决策 `待做`）；e2e 换成负向 `neg_u128`（`expect_compile_err` 匹配 `WIDE_INT`）。
  真支持需要「两寄存器对」表示 + 全套运算 + libcall（`__multi3`/`__udivti3`/…）+ ABI —— 独立专项。
- **踩到的第二个坑**：缺口台账的 **ID 字符集只允许大写字母与下划线**（解析器会跳过含数字的
  ID，表现成"台账没有行"）⇒ ID 从 `I128` 改名 **`WIDE_INT`**。

### M5 补面之五（2026-10-11）：静对齐与"用户分配器"两处审计

- **静对齐**：`align` 取自 layout（`inner.align.bytes()`）⇒ `#[repr(align(N))]` 应当生效，
  但此前**没有任何用例**验证"符号地址真的对齐"。新增 e2e `static_align_64`：运行期检查
  `&LATE % 64 == 0`（不依赖对象格式）⇒ **exit=42** ✔（用例数 **121**）。
- **用户 `#[global_allocator]`**：我原以为"no_std 自带分配器 + 我们的 shim"会撞重复符号，
  实测**否**——`vec_push`/`vec_string` 等既有用例本来就自带 `#[global_allocator]` 且全绿
  ⇒ **不必改**（假设被实测否掉，比"先修再发现没问题"便宜）。

### M5 补面之四（2026-10-11）：`#[link_section]` 明确拒绝（不许静默放错段）

- **发现**：后端**从没读过** `tcx.codegen_fn_attrs(..).link_section` ⇒ 用户写
  `#[link_section = ".CRT$XCU"]` 之类会被**静默忽略**（照旧进 `.data`/`.rodata`）——
  "写了不生效"是最糟的一种错，比报错难查得多。
- **处置**：`backend.rs::reject_link_section`（**一个**缺口 ID `LINK_SECTION`，静态与函数两处
  调用同一助手）明确拒绝，诊断里点名**段名**并指向 M4-T4 的对象层"任意段"支持。
- **e2e**：`neg_link_section`（`#[unsafe(link_section = ".mydata")] static X` ⇒ 编译期拒绝，
  `expect_compile_err` 匹配 `LINK_SECTION`）⇒ 用例数 **120**。
- 顺带：`#[export_name]` **不需要**后端做什么——它由 rustc 的符号名解析（`symbol_name_for_instance`）
  负责，已由既有用例覆盖。

### M5 补面之三（2026-10-11）：DWARF 调试段从"手工 gdb"变成自动核对

- **缺口**：README 写着"DWARF5（gdb 实证）"——那条是**手工**验的，后续改动把它弄坏没人拦得住。
- **落地**：`tests/dwarf_sections.rs` 直接读**对象文件的 COFF 段表**（20 字节文件头 + 40 字节段头
  - 长名走字符串表，不引第三方库），断言：
  - `-C debuginfo=2` ⇒ `.debug_info` / `.debug_abbrev` / `.debug_line` / `.debug_line_str` /
    `.debug_aranges` / `.debug_frame` 都在；
  - `-C debuginfo=0` ⇒ **一个都不在**（负向对照，防止"永远为真"的空断言）；
  - `-C debuginfo=2` 编出来的程序答案仍是 42（调试信息不改变语义）。
- **实测纠了一个预期**：我原以为会有 `.debug_str`，实际用的是 DWARF5 的 **`.debug_line_str`**
  ⇒ 断言按**实测集合**写（不是照抄 LLVM 的段名）。
- **纳入门禁**：新步骤 `rustc-dwarf` + 进 `--subset rustc` ⇒ 子集 **15 步**。

### M5 补面之二（2026-10-11）：其它 codegen 旋钮

同一条测试文件里加了"旋钮要么支持、要么点名拒绝"的矩阵（`tests/opt_levels.rs`）：

| 旋钮 | 结果 |
| --- | --- |
| `-C debuginfo=0/1/2` | 各档都能编能跑，答案 42 ✔ |
| `-C codegen-units=1/8`、`-Z threads=4` | 同上 ✔ |
| `-C panic=unwind` | **干净拒绝**（见下） |

**`panic=unwind` 的实测口径（重要）**：**no_std** crate 会被 **rustc 前端**先拒
（"unwinding panics are not supported without std"），根本走不到后端的 `[cap:UNWIND]`——
那条诊断只对 **std** 场景有意义，而 std 场景本地跑不了（M3-② 阻塞）。
所以测试断言"拒绝 + 诊断提到 unwind"（前端或后端任一），**不假装**后端那条路被测到了。

### M5 补面（2026-10-11）：`-C opt-level` 全档回归

- **缺口**：e2e 夹具的用例结构里没有"rustc 旗标"这一维（`extra` 是**源码级**附加）⇒
  117+ 条用例**全部只跑 debug 档**，而"后端在 `-O` 各档也对"是"完整后端"的硬承诺。
- **落地**：新测试 `tests/opt_levels.rs`（独立文件，约 190 行）——同一个无 std 程序
  （循环 + 递归 + 乘法 + 窄化 + 溢出检查）在 **0/1/2/3/s/z** 六档各编一次、链接、**真执行**，
  断言都返回 **42**；另有"同档编两次"的**确定性**断言。
- **两个踩过的坑**（都是断言选错了对象）：
  ① 拿 `.exe` 做逐字节比较 ⇒ 里面含链接器时间戳/PDB 路径，测的是**链接器**不是后端；
  ② `--emit=obj=<路径>` 在本版 rustc 上不生效，且后端产出的对象名是**固定的**
     `forge_codegen_output.o` ⇒ 用 `--out-dir <每轮独立目录>` 定位再比。
- **实测**：六档全部 42 ✔；同档两次 object 逐字节相等 ✔（1.9s，够便宜）。
- **纳入门禁**：新步骤 `rustc-opt-levels`（`Scope::Windows`）+ 进 `--subset rustc` ⇒ 子集 **14 步**。

### S3-sym 结论（2026-10-11）：受**谱汇编器文法**限制，保持 fail-closed + 点名诊断

- **实测过程**：把 `sym` 的符号名替换写进去后，汇编器报
  `内联汇编文本无法被汇编器解析："mov RAX, _RNv…seven"` ⇒ **符号名替换是对的**，
  卡在**谱汇编器的操作数文法**（只接受寄存器与数值字面量）。
- **关键区别**：`global_asm!` 能用符号，是因为它**不走本谱汇编器**（包成 `global_asm!`
  交给 **LLVM** 汇编）；inline `asm!` 走的是本谱汇编器 ⇒ 两者能力不同，别互相类推。
- **处置**：按"不留未测代码"原则**回退**替换实现，改成 **fail-closed + 点名诊断**
  （说清"谱汇编器只吃寄存器/数值 + `global_asm!` 为什么能用 + 根治方向"），
  并用负向用例 `neg_inline_asm_sym` 钉住（`expect_compile_err` 匹配"操作数暂不可用"）。
- **根治方向（记成 ISA-DSL 的活）**：给汇编器加「符号立即数 + `reloc abs64`」的解析形态；
  谱侧已有 `MOVABS_GLOBAL`（`reloc = "abs64"`）承接重定位，缺的只是**词法/文法**这一层。

### M5 补漏（2026-10-11）：后端子集看不见的两个守卫，红了

跑 `--subset quick`（= fmt + **全部**守卫 + 文档）才暴露的两件事——都是 v27 自己造成的：

1. **`error_taxonomy`（forge-codegen）**：仓库规定后端 `Unsupported(` 必须是能力缺口
   （`IrError::capability(ID, …)`），畸形 IR ⇒ `Internal`、数据/使用错误 ⇒ `Semantic`。
   而 v27 S1–S4 在 `pipeline/inline_asm.rs` 里写了 **26 处**裸 `IrError::Unsupported(`
   ⇒ 一直红，`--subset rustc` 却看不见（该守卫不在子集里）。处置：按分类学重判 ——
   约束协议/搬运形态这类"我们自己产物的形状" ⇒ `Internal`（7 处），
   用户文本/谱数据（汇编文本解析失败、显式寄存器不存在、没有汇编器等）⇒ `Semantic`（19 处）。
   **不新增能力 ID**（`capability_gaps` 要求每个 ID 恰好一处，而 `INLINE_ASM` 的失败点在
   `forge-rustc` 侧，不该被后端重复计入）。
2. **`lint_shipped`/`op_gap`（forge-isa-dsl）**：v27 给 `ops.toml` 加了 `InlineAsm`，
   而谱里**没有也不该有** `[[lower.InlineAsm]]`（它带文本与变长操作数，DSL 表达不了，
   由宿主管线展开）⇒ 被算成"真缺口"（x86 3 → 4）。处置：把 `InlineAsm` 归入
   `HOST_PIPELINE_OPS`（与 `VaStart` 同类），三谱 pin 的"宿主管线"8 → 9、真缺口不变；
   守卫的数组长度 8 → 9。

**把这两个守卫加进 `--subset rustc`**（各 0.2s / 0.6s）⇒ 后端子集 **13 步**。
教训与 v27 立项时那次同类：**"只跑后端子集"会漏掉跨 crate 的契约守卫**；
凡是"后端代码属于 workspace 契约一部分"的守卫，都应该在子集里。

### S4 第二刀（2026-10-11）：`options(noreturn)` 可用

- **此前**：`options.contains(NORETURN)` 参与 `ok` 判定 ⇒ 一票否决（`[cap:INLINE_ASM]`）。
- **现在**：`noreturn` **不改变汇编文本**、只改**落点**——没有 fallthrough（rustc 侧 `targets`
  为空）⇒ 当前块以 `unreachable()` 收尾（与"发散调用"用同一个终结符写法）。
  CFG 可达性那一步本来就按 `NORETURN` 不排后继（`lower/mod.rs` 的落点收集），所以两处口径一致。
- **e2e**：`inline_asm_noreturn`（`asm!("ud2", options(noreturn))` ⇒ **trap 0xC000001D**；
  此前连编译都过不去）⇒ 用例数 **118**、`expect_trap` 3 条。
- **口径提醒**：trap 用例的 `expected` 必须写**非零 NTSTATUS**（`-1073741795`），写 0 会被判
  "want 0" 红——本轮踩了一次。

### M4-T4 首结论（2026-10-11）：`WA-01` 按设计关闭（不是待根治项）

- **证据链**：① `add_data_with_relocs` 的文档写明**实测**——"MSVC 链接器对 `.rodata` 段的
  ADDR64 重定位不应用（链接后全 0），`.data` 段正常"；② 我们的对象写入器**故意**不给
  `add_rodata` 提供 relocs 变体 ⇒ "只读段 + 重定位"这种**链接后必坏**的形态**不可表达**；
  ③ 因此 vtable 指针表落 `.data` 是**在给定工具链下的正确实现**，不是技术债。
- **处置**：`WA-01` 判**已关闭（按设计）**；`add_rodata` 的文档把这条不变量写清
  （后来者不必再"试一次"）；`WORKAROUNDS.md` 的"根治方向"列不再写"需主库兼容"
  （那是**无法根治**的外部行为）。
- **残余代价（如实记录，不做粉饰）**：指针表在运行期**可写** ⇒ 丢掉只读页保护（W^X 层面
  的轻微退化）。若将来要恢复只读语义，得换链接器口径（lld/SYSV）或改用"加载时重定位"，
  两条都超出本后端的可控范围。

### M3-② 工作流收口（2026-10-11）：`cargo forge init --std`

- **问题**：M3-② 的探测工程原先要"先 `cargo forge init`（no_std 口径）再**手工编辑**
  `.cargo/config.toml` 把 `build-std` 改成 `["std","panic_abort"]`"——多一步手工、容易漏。
- **落地**：`init` 新增 `--std`（`build-std = ["std","panic_abort"]`），与 `--alloc` **互斥**
  （两者是不同的 build-std 口径）。**不加** `-Zshare-generics=yes`——本后端明确拒绝它
  （`[cap:SHARE_GENERICS]`，`WA-04`）。
- **实测**：在 `target/tmp` 的探针工程里跑 `cargo forge init --std` ⇒ 生成
  `build-std = ["std", "panic_abort"]` ✔；`--std --alloc` ⇒ clap 报
  "the argument '--std' cannot be used with '--alloc'" ✔；
  `examples/forge-rustc-std-hello/README.md` 的跑法同步改成一条命令。

### 门禁补面 + `label` 的阻塞分析（2026-10-11）

**① 确定性测试进门禁**：`determinism`（多 CGU 逐字节相等 / 并行==串行 / 增量）此前**既不在全量门禁
也不在后端子集**——只有 CI 作业跑。实测 3 个测试 16s ⇒ 新增步骤 `rustc-determinism`
（`Scope::Windows`，与 e2e 同口径），并进 `--subset rustc` 白名单（子集现 **11 步**）。
这条补的是"多 CGU/增量确定性"这个**对外承诺**的本地可复现性（`WA-38`/`WA-39`）。

**② `label` 操作数为什么还做不了（结构性，不是工作量问题）**：`asm!("jmp {l}", label { … })`
要求模板里出现**标签位置**，而标签必须落在指令流的某个**位置**上。我们现在的模型是
"文本 → 一次汇编 → 一串机器指令"，标签只能出现在这串指令的**两端**（开头/结尾）——
落在中间的标签会让它后面的指令地址全部偏移，而我们的发射是**按块线性拼接**的，
没有"在指令流中间插入一个位置"的表达。可行的解法只有两条，都要动结构：
(a) 让汇编器支持"分段"（一次汇编多段、每段之间留可回填的块边界）；
(b) 把 asm 拆成多条 IR op（每段一条），标签成为段边界。
两条都属 S4 的独立设计项，不在本轮的"止血"范围内。

### M4-T2 结论（2026-10-11）：**约定已按目标选**，并由双向测试钉住

实测（`abi_target_real::abstract_c_on_arm64_resolves_to_aapcs64`）：

- 同一个**抽象** `CallConvId::Builtin(C)`：在 **aarch64** 机器上 plan 出来是 **`aapcs64`**，
  在 **x86_64** 机器上是 **`win64`** ✔；
- 也就是说 v20 的"`c` 是抽象名，由规则的 `aliases` + 该机绑定共同代答"这条设计**已经在工作**，
  后端**没有**把 win64 写死 ⇒ M4-T2 在库层面**无需改动**，只需要把这件事钉住。
- 钉法：同一个探针函数跑两台真机，两条断言互为对照（一条只说 aapcs64 = 偶然默认也能过）。

⇒ T2 剩下的部分不在约定层，而在**目标相关的对象层/链接层**（`.debug_frame` 的 CFI、PE 异常表、
`global_asm` 的汇编器参数、链接器与入口），已并入 T3。

### M4-T1 落地（2026-10-11）：未知目标不再静默回落 x86_64

- **问题**：`isa_name_for_target` 的 `else` 分支返回 `"x86_64"` ⇒ 交叉编译到 riscv64 之类的
  目标会**静默编出 x86 机器码**（仓库明令禁止的静默降级）。这是 M4 施工图里危害最高的那一处。
- **落地**：函数改成返回 `Option<&'static str>`；`codegen_crate` 入口对 `None` 报
  **`[cap:TARGET]`** 并直接返回空模块。**目标校验只有一个失败点**（缺口守卫要求 ID 恰好一处）
  ⇒ `compile_fn_task` 里的推导写成**不变量**（`expect`，因为入口已校验过）。
- **实测可达性**：本仓沙箱里**无法端到端验证**（只有宿主 target 的 core 已安装，交叉目标在前端就失败）
  ⇒ 验收靠"单点 + 台账 + 守卫 + 代码审查"，如实记录（方案里不承诺本仓全绿）。
- **台账**：新增 `TARGET` 行（决策 `不做`）；`capability_gaps` 绿（ID 恰好一处）。

### M4 施工图（2026-10-11 实测后定稿）：第二目标 + 对象层根治

**实测结论先说**：第二目标比预想的小——**ISA 与约定层已经就绪**，后端里真正写死目标的地方只有 4 处
（全仓搜 `win64|x86_64|msvc|"x86"` 共 18 处命中，其余是注释/文档）。

| 已有 | 位置 |
| --- | --- |
| 目标三元组 → ISA 注册（x86_64 / aarch64 两条分支） | `compile.rs::auto_register_isa_for_target` |
| arm64 谱（aarch64 后端 + 编码/汇编器） | `isa/arm64.toml`、`forge-codegen/src/arch/arm64.rs` |
| aarch64 调用约定 `aapcs64` | `forge-abi` 的规则与绑定（v20 A1/A2） |

**真正的 4 处写死（按危害排序）**：

1. **未知目标静默回落到 x86_64**（`compile.rs` 的 `isa_name_for_target` 末尾 `else { "x86_64" }`）
   ⇒ 交叉编译到 riscv64/其他目标会**静默编出 x86 机器码**。这是最该先修的（违反"不静默降级"）。
   处置：返回 `Option`，`codegen_crate` 明确拒绝（新缺口 ID `TARGET`），**不再回落**。
   注：本仓沙箱里**无法端到端验证**（只有宿主 target 的 core 已安装，交叉目标在前端就失败）
   ⇒ 只能靠"单点 + 台账 + 代码审查"验收，如实记录。
2. **`.debug_frame`（CFI）只写了 x86_64**（`dwarf.rs`）：aarch64 的 CFA/寄存器编号规则完全不同
   （x29/x30、sp 对齐、`DW_CFA_def_cfa` 的寄存器号）⇒ 目标切换必须按目标生成，否则调试信息是错的。
3. **`global_asm.rs` 的 `is_x86` 分支**：汇编器调用的旗标/包装随目标变（aarch64 用 `-C target=…`
   与不同的 `bad_asm_style` 处理）。
4. **`WA-01`（vtable/指针表落 `.data`、`.rodata` 的 ADDR64 被 MSVC 链接器拒）**：属对象层根治，
   见下 §对象层。

**分期（每期都给验收口径）**：

- **T1 目标校验**：`isa_name_for_target` 去回落、`codegen_crate` 对未支持目标明确拒绝
  （`[cap:TARGET]`，台账一行、源码一处）。验收：代码审查 + 台账守卫（`capability_gaps`）。
- **T2 约定与 ABI**：`abi_setup` 里对 `win64` 的假定改成"按目标选约定"
  （x86_64-msvc → `win64`、aarch64-* → `aapcs64`）。验收：`abile/abi_target_real` 类的单测
  在两种约定下各跑一遍（合成函数，不需真实目标）。
- **T3 目标相关对象层**：`.debug_frame` 按目标生成；`global_asm` 的汇编器调用按目标参数化；
  **PE 的异常表**（aarch64 的 `.pdata`/`.xdata` 与 x64 不同）——这是 `UNWIND` 从"不做"变"可做"的前提。
  验收：**必须在非沙箱环境**（需要对应 target 的 core 与链接器），本仓只能做单点与合成测试。
- **T4 对象层根治（与目标无关）**：`WA-01` 的 `.rodata` ADDR64 处置——先做**决策**（继续用 `.data`
  并写清代价，还是按段类型分流 + 与链接器行为对齐），再决定是否引入第二链接器路径。

**风险与诚实边界**：T2 的本机可验证性最高（合成 ABI 单测）；T3/T4 的核心验证要么需要交叉工具链，
要么需要非沙箱环境 ⇒ 方案里不承诺"本仓全绿"，只承诺"每步都有可机检的落点"（守卫/单测/台账）。

### S4 首刀（2026-10-11）：`clobber_abi` 可用（纯 clobber 槽）

- **发现**：这个 nightly 的 MIR `TerminatorKind::InlineAsm` **没有 `clobber_abi` 字段**——
  `clobber_abi("C")` 在 MIR 构建期就被展开成一批 `Out { reg, place: None }` 操作数。
  之前我们把它当成"输出寄存器"去解析（要 GPR 类）⇒ 撞上 `xmm0`/`k0` 直接 fail-closed
  （不静默错，但真实代码编不过）。
- **落地**：`Out`/`InOut` 的 `place: None` 视为**纯 clobber 槽**（约束 `c,<suffix>`）：
  不产物值、不搬数据，只把该寄存器记进 `inst_clobbers`。纯 clobber 的显式名**跨类解析**
  （`CompileState::reg_classes`），解析不到（本 ISA 没有的寄存器，如 AVX-512 mask `k0`）
  **跳过无害**——我们从不在那些寄存器里放值。
- **两处踩坑**：① 去重必须按 **(编号, 类)**——不同类的 `to_index` 会撞车（`rax`/`xmm0` 都可能为 0）；
  ② 手工验证一度"挂起"，其实是 shell 里 `Start-Job` 的 rustup/toolchain 解析问题，
  换到 e2e 夹具（自带超时与正确旗标）后 **0.77s 通过**——**别把工具链问题误判成后端 bug**。
- **e2e**：`inline_asm_clobber_abi`（`asm!("nop", clobber_abi("C"))` ⇒ **42**），用例数 **117**。

### M4-② 首刀（2026-10-11）：`-Zshare-generics=yes` 编译期明确拒绝（`WA-04` 关闭）

- **问题**：用户显式开 `-Zshare-generics=yes` 时，跨 crate 的泛型实例会被"共享"给上游 crate
  生成，而本后端不生成辅助实例 ⇒ **链接期**才缺符号（LNK2019），错误离现场很远。
- **落地**：`codegen_crate` 开头判 `tcx.sess.opts.unstable_opts.share_generics == Some(true)`
  ⇒ 编译期 `[cap:SHARE_GENERICS]` 明确拒绝。**只拒用户显式指定**：该选项默认是 `None`，
  由 rustc 按优化级别解析（debug/`-O0` 下默认 true）⇒ 默认路径不受影响 ✔。
- **实测**（手工，因为 e2e 的 `extra` 是源码级附加、不是 rustc flag）：
  带 flag ⇒ `error: Unsupported: [cap:SHARE_GENERICS] …`；不带 ⇒ 正常产出 11264 字节的 exe。
- **台账**：新增 `SHARE_GENERICS` 行（决策 `不做`）+ `WA-04` 改判**已关闭**。
  根治（补辅助实例）仍归 M4——它需要对象层的"实例导出/引用"模型。

### M3-② `std` 探测：工程与命令已固化，**库内前置阻塞已排除**（2026-10-11）

**已排除的前置阻塞**（本轮之前/本轮落地，均有 e2e 守护）：

- **TLS**：`std` 的 `thread_local!` 依赖线程局部静态 ⇒ 动态 TLS 已实现
  （`thread_local_roundtrip`，exit=14）；
- **`abort()`**：`std` 的 `abort_internal` 走 `core::intrinsics::abort()` ⇒ 已实现
  （`intrinsic_abort`，exit=3）；
- **alloc 运行时**：链 `std` 时不再注入手写 shim（`WA-02`），避免 LNK2005。

**新增探测工程**：`examples/forge-rustc-std-hello/`（`Cargo.toml` + `src/main.rs` +
`rust-toolchain.toml`（浮动 `nightly`）+ README 里的精确命令）。形态与 `forge-rustc-hello`
一致（独立 `[workspace]`，`panic=abort`），差别是 `build-std = ["std", "panic_abort"]`
且 `std` 由 LLVM 编译（`tools/forge-rustc-wrapper` 按 crate 名剥离 `-Zcodegen-backend`）。

**本仓 sandbox 内跑不通（环境限制，不是后端缺陷）**：`-Zbuild-std=std` 要解析 `std` 的
构建依赖（`cfg-if`/`getopts` 等），而 sandbox **无网络凭证**——实测原文：

```text
error: failed to get `cfg-if` as a dependency of package `std v0.0.0 (…/library/std)`
[35] SSL connect error (schannel: AcquireCredentialsHandle failed: SEC_E_NO_CREDENTIALS)
```

补充实测：把 `CARGO_HOME` 重定向到 workspace 内（`target/cargo-home`，并复制 `~/.cargo/config.toml`
的 ustc 镜像）能绕过"`~/.cargo` 不可写"，但**绕不过没有网络**这一条。⇒ 结论必须**在沙箱外
（或 CI）**取；命令已固化到该工程 README，一条命令即可。

**下一步（M3-② 的验收口径）**：在沙箱外/CI 跑通 `hello from std`；若失败，按后端原则它应当
以**明确的 `[cap:…]` 或编译错误**暴露缺口 ⇒ 逐条落成新的缺口 ID 或实现项。

### M3-③ `core::intrinsics::abort()` 落地（2026-10-11，工作区）

- 现状：`abort()` 落到 `lower_intrinsic` 的末尾 ⇒ `unsupported intrinsic`（编译失败）。
  任何走 `abort` 路径的真实 crate（`std` 的 `abort_internal`、部分 `panic=abort` 收尾）都编不过。
- 落地：`"abort"` 分支发 kernel32 `ExitProcess(3)`（MSVC `abort()` 的退出码；Rust 对 `abort()`
  的退出码**未作规定**）；其后的 `unreachable()` 由调用点（`Call` 终结符的 `target: None` 分支）发。
  后端没有内联 `asm!`（`[cap:INLINE_ASM]`）⇒ 不能发 `ud2`，用 FFI 终止进程是当前唯一正确出口。
- e2e：`intrinsic_abort`（`black_box(true)` 后调 `abort()`）⇒ **`PASS intrinsic_abort exit=3`**；
  用例总数 **111**（README 的机读计数标记同步更新——`rustc_doc_counts_guard` 会强制这件事）。

### M3 原始条目

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

### M5 门禁与守卫收口

**已落地（2026-10-11，见 §8）**：两个新守卫（`rustc_doc_counts`：README 计数 ↔ `tests/e2e.rs`
双向相等；`workarounds_guard`：`[WA-NN]` 编号唯一 + 代码引用可解析）+ 门禁新增两步
（`rustc-e2e` 110 用例真执行、`rustc-e2e-guards` 不变量，均限 Windows 口径）。
原 M5 里的"契约回写 + 账本归档"**另立 M6**（它只能在 M2.5/M3/M4 都收口之后做）。

### M6 契约回写与账本归档

**已拆为两步**：**M6-① 契约回写**（README 的能力面/路线图、`CLAUDE.md` 的 forge-rustc 速查、
`WORKAROUNDS.md` 状态纠偏——2026-10-11 已落地，见 §8）；**M6-② 账本归档**（`plans/` → `archive/`
并全量更新引用，只能在 M2.5/M3/M4 都收口后做）。

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

### M6-① 契约回写（2026-10-11，工作区）

**动机**：v27 前十个轮次的改动全落在代码/账本上，而**对外契约**还停在 v26：
`crates/tools/forge-rustc/README.md` 的「路线图」结尾写着 **"路线图无剩余开放项"**，
`CLAUDE.md` 里没有 forge-rustc 后端的现状章节 ⇒ 新读者会得到一份错的"当前能力面"。

**改动**：

- `crates/tools/forge-rustc/README.md`：
  ① 路线图结尾改为"v27 重新定义了'什么算完整后端'"，并指向方案账本；
  ② 新增 **「v27 能力与缺口面」** 一节：`global_asm!`（含 `const`/`sym`）的实现路线、
  e2e 的 trap 语义与 `known_failure` 清零、**四个缺口 ID 的决策表**
  （`INLINE_ASM` 待做 / `UNWIND` 不做 / `TLS` 待做 / `MIR_TERMINATOR` 不做）、
  门禁两步 + 两个守卫、以及 M2.5/TLS/M3/M4/M6 的顺序依赖。
- `CLAUDE.md`：新增 **「forge-rustc（rustc 代码生成后端）现状速查」** 一节（能力面 / e2e 与
  机读计数标记 / 四个缺口 ID / WORKAROUNDS 的"根治方向不是现状"教训 / 门禁两步 / 顺序依赖）。
- `WORKAROUNDS.md`：`WA-02` 改判**已关闭**（按需注入），并补回 `WA-26`/`WA-28` 两行
  （此前是悬空引用，由新守卫抓到）。

**自检**：两个守卫在改动后重跑均绿——`rustc_doc_counts_guard`
（`{cases: 110, hard: 110, flaky: 0, known: 0, expect_trap: 1}`，README 的机读标记与代码一致）、
`workarounds_guard`（43 编号唯一、引用全可解析）；文档 0 issues。

**同批提交的代码**：`M3-①`（alloc 运行时**按需注入**，见下节）——它是 std 的前置条件，
与本批文档同源，故一并提交；**M3 本身未完成**（M3-② 的真跑 `std` 需要 M2.5 与 TLS）。

### S2b 补丁（2026-10-11）：传统寄存器短名映射

上一轮记的待办（`out("rax")` 因 rustc 规范名是 `ax` 而不可用）已解：

- **谱**：`[reg.gpr64]` 加 `aliases = { ax = 0, cx = 1, … }`。上一轮没生效的原因是**插入位置**
  （落在 `names` 数组范围内侧、被当成注释/别的键）；正确位置是该表的顶层键（本谱的
  `alloc_count = 16` 之后）。
- **生成器**：`reg_by_name` 不再走生成的 `FromStr`——**跨组同名时它的先匹配规则不可靠**
  （`ax` 会落到 16 位组）。改成直接扫"主名 + 别名"两张表（`eq_ignore_ascii_case`、类必须一致），
  别名对与主名对**同源**（都在 `gen_reg_info` 里由 `group_names` + `aliases` 生成）。
- **守卫**：`reg_name_table` 新增断言——短名 `ax`/`AX` 必须解析到 `RAX` 的编号；
  并记下"两个类里编号恰好都是 0"这个事实（`to_index` 跨组冲突的由来，别再用编号区分类）。
- **e2e**：`inline_asm_named_reg` 改回最自然的写法 `asm!("xor rax, rax", out("rax") x)` ⇒ **exit=0**。

### S2b 落地（2026-10-11）：显式寄存器名

- **能力**：`asm!("xor r8, r8", out("r8") x)` ⇒ x 从 5 变 **0**（e2e `inline_asm_named_reg`）。
  强断言：若实现忽略显式名（改挑别的寄存器），x 会留在 5 ⇒ 必红。
- **机制**：约束后缀从 `g`（池）扩展成"`g` 或**显式拼写**"；后端经新加的
  `TargetRegInfo::reg_by_name(name, class)` 反查编号（生成器用 `FromStr` + `PhysReg::class` 回验，
  与汇编器同源）；重复使用同一寄存器会被拒绝。
- **rustc 前端的两条实测规则**：
  ① 模板里**不能**用 `{0}` 引用显式寄存器操作数（"explicit register arguments cannot be used
  in the asm template"）⇒ 必须在文本里直接写寄存器名；
  ② `InlineAsmReg::name()` 给的是 rustc 的**规范短名**——x86 传统寄存器是 `ax`/`cx`/…（不是
  `rax`/`rcx`），与 x86 谱的 `RAX`/`RCX` 不同名 ⇒ **传统寄存器的显式名暂不可用**；
  `r8`/`r9`… 两侧一致 ⇒ 已可用。给谱加 `aliases` 试过但**没进生成物**（表形状另有要求，
  已回退、不留死数据），记为待办。

### S3 前半落地（2026-10-11）：`const` 占位符

- **设计**：`const`/`sym` 在 **rustc 侧就地换成字面文本**，只有寄存器占位符进入约束表
  （`{槽序号}`）——后端完全不用改，`const`/`sym` 也不需要寄存器分配。
- **无冗余**：常量格式化抽成 `global_asm::scalar_text`（纯函数）+ `const_text`（吃 DefId），
  `global_asm!` 的 HIR 路径与内联 `asm!` 的 MIR 路径**共用这一处**
  （本 nightly 的两侧枚举是不同类型：`rustc_hir::InlineAsmOperand` 与
  `rustc_middle::mir::InlineAsmOperand{ Const { value: Box<ConstOperand> } }`，
  `value.const_.eval(...)` 求值）。
- **实测**：`inline_asm_const`（`x = 35`、`asm!("add {0}, {1}", inout(reg) x, const 7)`
  ⇒ **exit=42**）。
- **`sym` 仍是 fail-closed**（MIR 侧 `SymFn { value }` 是 `ExprId`/`ConstOperand` 形态，
  取符号名要另做一层；S3 的下一半）。

### S2a 落地（2026-10-11）：寄存器约束端到端可用

**上一轮定位的字段问题的解法**：搬运指令的操作数字段**按角色挑**，不写死 0/1。

- 依据 `InstOperand { field, slot, class, constraint, role, settable }`：
  操作数表的顺序契约是"**Use 在前、Def 在后**"，且模板里写死的字段 `settable == false`
  ⇒ "`role == Def && settable`"唯一确定 dst、"`role == Use && settable && field != dst`"
  唯一确定 src（`inline_asm.rs::mov_fields`）。
  误按 0/1 绑定时，管线自己的 v23 M4 一致性检查会当场拒（
  "字段被登记为 Def 但操作数表里没有这一项"）——**守卫替我们抓了一次**。

**现在的能力**：`in(reg)`/`out(reg)`/`inout(reg)` 全部可用（只接 64 位整数；
显式寄存器名 = S2b、`const`/`sym` = S3、`label`/`clobber_abi` = S4 仍 fail-closed）。

**e2e 新用例**：`inline_asm_reg_inout`
（`let mut x = 5; let y = 37; asm!("add {0}, {1}", inout(reg) x, in(reg) y)` ⇒ **exit=42**；
不带 asm 时是 5 ⇒ 强断言）——它同时验证了：挑寄存器、名字替换、搬入/搬出、
`inst_clobbers`、以及结果写回 `store_place`。

### S2a 第二轮（2026-10-11）：results 变参 + 两侧接线完成，**接线暂挂**

**本轮做完的**：

- `ops.toml` 的 `results` 支持 **`"variadic"`**（`build.rs` 新增 `ResArity::{Fixed, Variadic}`
  与 `Display`；生成物里的 `result_count` 仍是 `u8`，变参记 0——它只是"声明形状"，不参与校验）；
  `InlineAsm` 因此变成 `operands = "variadic"` + `results = "variadic"`。
- `builder::inline_asm_with(constraints, text, in_operands, out_tys) -> Vec<Value>`。
- `pipeline/inline_asm.rs` 的 S2a 改用**操作数/结果双游标**（`i` 取操作数、`o` 取结果、
  `io` 两边各取一个）。
- `forge-rustc` 侧：模板重建（`Placeholder{operand_idx}` → 文本里的 `{idx}`）、
  `In`/`Out`/`InOut` 收集成"约束 + 输入值 + 输出类型 + 输出落点"、结果经 `store_place` 写回。
  两处实测细节：MIR 侧的 `InlineAsmRegOrRegClass` 来自 **`rustc_target`**（与 `rustc_ast`
  同名但不同类型）；`Out`/`InOut` 的 `place` 是 **`Option<Place>`**（`out(reg) _` 为 None）。

**为什么还挂着（诚实记录）**：接线跑起来后，管线自己的**一致性检查**当场拒绝：

```text
codegen worker panic: 第 22 条指令的字段 0 被登记为 Def（vreg 10323），
但操作数表里没有这一项：[InstOperand { field: 0, slot: Some(0), … }]
```

根因：**搬运指令的操作数字段索引取决于汇编器为 `mov rax, rax` 选中的编码形态**
（`MOV_R_RM` / `MOV_RM_R` 等 form 的 dst/src 字段序号不同），按 `(0=dst, 1=src)` 硬绑定是错的。
⇒ 正确做法是**按角色挑字段**（看 `MachineInst::operands()` 里每个 `InstOperand` 的
`field`/`slot`/`role`），或者更稳：在管线里加一个"造一条 bound 操作数的 mov"的**辅助构造器**，
把"哪个字段是 dst"这件事收敛到一处。

**当前状态**：`forge-rustc` 侧把 S2a 接线**暂挂回 fail-closed**（只有 `operands.is_empty()`
才走 S1 文本路径），带说明的 e2e 用例已删除 ⇒ 行为与 S1 一致、e2e 仍 113/113。
基础设施（IR 变参、builder、降级体、两侧收集逻辑）全部保留在树里，下一轮只需解字段问题。

### S2a 进度（2026-10-11，工作区）：**基础设施已落地并编译，接线还差两处**

**已完成**（代码在，行为未变——还没有东西发约束）：

- `ops.toml` 的 `InlineAsm` 从"0 操作数"改成 **`operands = "variadic"`**（`results` 仍是 0）。
- `builder` 新增 `inline_asm_with(constraints, text, operands)`（`inline_asm(text)` 保留 = S1 形态，
  实现转调它）；`immediates = [约束, 文本]`。
- `CompileState` 新增两个 `'static` 快照（`Arc::clone(machine.reg_info())`——借 `&M` 活不过
  `CompileState::new`）：`reg_name_of: Fn(u32, RegClass) -> Option<&'static str>` 与
  `gp_alloc_order: Vec<u32>`。
- `pipeline/inline_asm.rs` 的 **S2a 全流程**（已编译通过）：解析约束 → 从
  `gp_alloc_order` 挑互不重复的寄存器 → `reg_name` 换成名字 → **占位符替换**
  （`{}`/`{n}`，其它修饰一律明确拒绝）→ 汇编 → `in`/`inout` 的**搬入**（dst 侧
  `set_reg_field` 写死、src 侧 `OperandBind{vreg, field:1, role: Use}`）
  → asm 本体（字面量寄存器）→ `out`/`inout` 的**搬出**（dst 侧绑 `Def`）
  → 每条相关指令带 `inst_clobbers = 选中的寄存器 + 类`。

**还差两处（下一轮）**：

1. **`results` 变参**：`out`/`inout` 的值必须作为 IR **结果**产出。原因实测：本后端的局部变量
   住在**栈槽**里（`load_place` → `place_addr` → `load`），所以 asm 写完寄存器后必须
   `store_place(place, v)` 才可见，而 `store_place` 要一个 IR `Value` ⇒ 需要结果。
   `ops.toml` 的 `results` 目前只吃整数（`build.rs` 的 `results: u8`）⇒ 要按 `operands`
   的模式加 `"variadic"`，并同步校验器/表生成处。
2. **rustc 侧接线**：把 `InlineAsmOperand::{In, Out, InOut}` 变成"约束文本 + 操作数值"，
   并**重建带 `{n}` 的文本**（模板里的 `Placeholder{operand_idx}` 直接写成 `{idx}`）；
   显式寄存器名（`Reg(_)`）保持 fail-closed（S2b），`const`/`sym`/`label` 同（S3/S4）。

**验收用例已定**：`let x = 5; let y = 37; asm!("add {0}, {1}", inout(reg) x, in(reg) y); x`
⇒ **exit 42**（不带 asm 时是 5）。

### S2 施工图：寄存器约束（侦察完成，2026-10-11）

**结论先说**：S2 **不需要**给分配器加新能力。管线里已有的三件东西足以表达
"值 ↔ 物理寄存器"：

| 机制 | 是什么 | 位置 |
| --- | --- | --- |
| `OperandBind { vreg, field, role, class }` | 把一条机器指令的**寄存器字段**绑定到某个 vreg（分配器回填） | `machine/lowering.rs`、`CompileState.inst_ops` |
| `MachineInst::set_reg_field(i, preg, class)` | 把字段**写死**成物理寄存器（分配器不管它） | 序言/尾声与 `state.rs:403` 的先例 |
| `inst_clobbers: Vec<Vec<(u32, RegClass)>>` | 该指令**破坏**的物理寄存器集（分配器据此不把别的值放那儿） | `state.rs:19`、传给 `allocate(...)` |

且 `is_reg_field_settable` 明确：**模板里写死的寄存器字段不可改写**（DSL 不为它生成
`set_reg_field` 分支）——这正好是我们要的两种语义的分界线。

#### ① IR 表达（设计决定）

`InlineAsm` 从"0 操作数"扩成：**操作数 = 输入/输出值**，外加**两个** `Immediate::String`：
`[0]` = 约束描述、`[1]` = 汇编文本。约束描述用紧凑文本（如 `"i,g;o,g;io,g"`，
`i`=in / `o`=out / `io`=inout，第二段是寄存器类或显式寄存器名）。

理由：① 不引入新 IR 类型（S3 的 `const`/`sym` 与 S4 的 `clobber_abi`/`label` 都能往同一段
文本里加字段）；② IR dump 里人可读；③ 顺序即占位符序号（`{0}`/`{1}` 直接对应操作数）。

#### ② 降级（`pipeline/inline_asm.rs`）

1. **选寄存器**：显式名 ⇒ 用它；`reg` 类 ⇒ 从**调用者保存**池里贪心挑，避开已占用与该
   指令的 clobber 集；
2. **替换文本**：把 `{}`/`{n}` 换成选中的**物理寄存器名**（需要 `RegInfo` 的寄存器名表——
   **待验证**：拿到名字的入口名与形状）；
3. **汇编**：替换后的文本经 `asm_parse` ⇒ 机器指令（这些指令里的寄存器是**字面量**，
   不带 `OperandBind`）；
4. **输入搬运**：每条 `in`/`inout` 前插一条 `mov dst=<选中 preg>, src=<vreg>`——
   `dst` 用 `set_reg_field` 写死、`src` 用 `OperandBind{ role: Read }` 绑定；
5. **输出搬运**：每条 `out`/`inout` 后插 `mov dst=<vreg>, src=<选中 preg>`
   （`src` 写死、`dst` 绑定 `role: Write`）；
6. **clobber**：把选中的寄存器 + 声明的 clobber 集写进**每条**相关指令的 `inst_clobbers`。

#### ③ 分配器交互：**零改动**

`inst_clobbers` 让分配器把选中寄存器视为"此处被破坏"，于是不会把别的值放在那儿；
搬运指令的 vreg 操作数走正常分配。**不需要** pinned live range、不需要改 regalloc——
这是本设计最重要的收敛点。

#### ④ 分期与验收

- **S2a**：`in`/`out`/`inout` + `reg` 类（调用者保存池）。
  验收：`let x = 5; let y = 37; asm!("add {0}, {1}", inout(reg) x, in(reg) y); x`
  ⇒ **exit 42**（不带 asm 时是 5 ⇒ 强断言；用 `add` 是因为本谱的 `mov` 只有 reg-reg/reg-mem，
  **没有** `mov reg, imm` 形态）。
- **S2b**：显式寄存器名（`in("rcx") x`）+ `lateout`/`inlateout` 的次序语义。
- **S2c**：`const`/`sym` 占位符（= S3；S2 落地后它们才有可验证的用法）、`clobber_abi`、
  `options(...)`、`label`（= S4）。

#### ⑤ 清单：两条已实测确认，一条仍是开工第一件事

- **已确认**：`OperandBind.role` 的枚举是 `OperandRole::{Use, Def}`（不是 Read/Write）——
  输入绑 `Use`、输出绑 `Def`。
- **已确认**：`MachineInst::set_reg_field(i, preg, class)` 只对**可改写**字段生效；模板里写死的
  寄存器（DSL 不为它生成该分支）正是"字面量寄存器"那一侧，语义边界天然清楚。
- **已解决（2026-10-11，S2 开工第一件事）**：**寄存器名表**。`TargetRegInfo::reg_name(reg, class)`
  已落地——生成器从**与 `FromStr`/`From<Reg> for &'static str` 同源**的 `group_names` 发出
  反向表，经 `TargetRegInfo` 暴露给管线。**踩到一个真缺陷（被新守卫当场抓到）**：
  `PhysReg::to_index` 在"多宽度视图"ISA 上**跨组冲突**（x86 的 `AL` 与 `RAX` 同为编号 0），
  第一版只按编号查 ⇒ 返回 `"AL"` 而不是 `"RAX"`。修正：接口**必须带 `RegClass`**，
  反向表按 `(编号, 类)` 匹配。守卫 `crates/backend/forge-codegen/tests/reg_name_table.rs`
  同时钉住两件事：拿得到名字，且**汇编器认这个名字**（`add {name}, {name}` 往返解析）。
- **不变式**：`push_inst_line` 与 `inst_ops`/`inst_clobbers` 必须**逐条**平行（漏一条就错位）；
  `inline_asm.rs` 现在每次 push 都配一条，扩成多指令后要保持。

### S1 收口：规模棘轮回退修复 + 后端管线契约回写（2026-10-11）

**上一轮留下的真实回退（本轮修好）**：S1 往 `pipeline/lowering.rs` 里塞了 ~36 行，
把该文件撑到 **871 行 > 规模棘轮上限 835** ⇒ `file_size_ratchet` 红。上一轮只跑了
`cargo forge gate --subset rustc`，而那个子集**不含**这条守卫 ⇒ "验收通过"是**不完整的**。

处置（按仓库纪律，**切文件而不是抬上限**）：

1. 内联汇编的展开体抽成新模块 `pipeline/inline_asm.rs`
   （`CompileState::expand_inline_asm(&Instruction) -> Result<bool, IrError>`，
   调用点缩成 `if self.expand_inline_asm(inst)? { continue; }`）；
2. 顺手把两处冗长注释压缩（Nop 墓碑 4→1 行、Load/Store 宽度 4→2 行）⇒ 836 行，
   棘轮恢复绿；
3. **把 `file_size_ratchet` 加进 `--subset rustc` 白名单**——它是"改后端"最该跑到的守卫之一，
   上一轮正是漏了它。规矩记下：**给后端子集加步骤时，先问"这一步会不会因为我改的文件而红"**。

**契约回写**：`docs/reference/backend-pipeline.md` 新增 **§13「v27 增量」**（原 §13 顺延为 §14）：
13.1 内联汇编（IR 形态 / 展开时机 / 文本来源 / fail-closed / S1 边界）、
13.2 运行时注入（alloc 与 TLS 的"何时注入 + 关键契约"表）、13.3 `abort()` 的出口。
这三处都是**宿主侧**能力（不新增机器能力），写进管线契约是为了下次改它们时不必再摸一遍。

### S1 落地（2026-10-11，工作区）——**无操作数的内联 `asm!` 真实现**

按施工图做完，实测结论与预期一致，另有两处"施工时才发现"的细节：

1. **汇编器**：`CompileState` 新增 `asm_parse: Option<Arc<dyn Fn(&str) -> Option<Vec<I>> + Send + Sync>>`，
   在 `CompileState::new` 里由 `machine.assembler()` 构造（`Arc::clone`，`TargetAssembler: Send + Sync`）。
   `new` 的 where 子句补 `M: TargetMachine<Inst = I>`（原来只写 `M: TargetMachine`）。
2. **字符串表**：**不需要新字段**——`CompileState::new` 早已把 `TypeStore` 快照放进
   `ctx.type_store`（v3 S3 的读路径优化），`lower_block` 直接 `lookup_str` 取文本。
   这是施工图里"要穿两样东西"被**推翻一半**的地方：施工前必须实测，别照图纸闷头做。
3. **IR 侧**：`ops.toml` 新增 `InlineAsm`（`operands = 0`、`results = 0`、`side_effect = true`、
   `type_rule = "none"`）；`builder::inline_asm(&str)` 以 `Immediate::String` 携带文本；
   `lowering.rs` 在逐指令循环里特判该 op ⇒ 解析文本、把每条 `Inst` 连同平行的
   `inst_ops`/`inst_clobbers` 推进当前虚拟块。
4. **施工时才发现的两处**：
   - `forge-opt` 的 `const_fold` 对 `Opcode` 是**穷尽 match** ⇒ 新 op 必须显式加一条
     （`InlineAsm => Ok(None)`，语义正确：不透明文本永不折叠）；
   - `TerminatorKind::InlineAsm` 的 `template` 字段在本 nightly 上**直接 `.iter()` 即可**
     （`.as_ref()` 会触发 `AsRef` 歧义：`&[T]` 有多个 `AsRef` 实现）。
5. **rustc 侧放行条件（S1 口径）**：模板全是 `String` 片段（无 `{}` 占位符）且非
   `options(noreturn)` ⇒ 发 IR op + `jump(targets[0])`；其余形态保持 `[cap:INLINE_ASM]`。

**实测**：`inline_asm_nop` **exit=42**（编过、跑通、后续代码照常执行）；
`inline_asm_ud2` **exit=-1073741795**（NTSTATUS #UD ⇒ 插进去的字节**真的被 CPU 执行**）；
用例总数 **113**。

### S1 施工图（2026-10-11 二轮侦察：**要穿两样东西进管线**）

早先担心的"给生成的 `Inst` 加 raw 变体"**不需要**——正确路径是：降级期用本 ISA 的
`TargetAssembler::parse_insts(&str) -> Result<Vec<Inst>, AsmError>` 解析文本，把得到的
`Inst` 就地推进当前虚拟块（`vcode.push_inst_line` + 平行的 `inst_ops`/`inst_clobbers`）。

真正的阻塞是管线里**拿不到这两样**（均为实测）：

| # | 缺什么 | 现状 | 施工 |
| --- | --- | --- | --- |
| 1 | **汇编器** | `lower_block` 只有 `&dyn TargetLowering`；`CompileState` 字段里没有 machine/assembler | `CompileState` 加 `asm_parse: Option<Arc<dyn Fn(&str) -> Option<Vec<I>> + Send + Sync>>`，在 `CompileState::new(&self.machine, …)`（`pipeline/compiler/mod.rs` 的调用点，**那里有 machine**）里由 `machine.assembler()` 构造（`TargetAssembler: Send + Sync + 'static`，可直接 `Arc::clone`） |
| 2 | **字符串表** | `Immediate::String(InternedStr)` 目前**只有二进制序列化器**（`funcs.rs` 的 `store.lookup_str`）会解析；降级期**无人读** | 同 `new` 里把 `lookup_str` 闭包一并传入（`CompileState` 持 `Arc`） |
| 3 | IR op | `ops.toml` 无内联 asm 的 op | 加 `InlineAsm`（`operands = 0`、`results = 0`、`side_effect = true`、`type_rule = "none"`），文本走 `Immediate::String` |
| 4 | builder 入口 | 无 | `pub fn inline_asm(&mut self, text: &str)` ⇒ `self.emit(Opcode::InlineAsm, vec![], vec![Immediate::String(interned)], &[], InstFlags::SIDE_EFFECT)` |
| 5 | rustc 侧 | `TerminatorKind::InlineAsm` → `[cap:INLINE_ASM]` | S1 只放行**纯 String 模板**（无 `{}` 占位、无 `label` 操作数）；`const`/`sym`/寄存器留 S2–S4 |

**S1 验收**：e2e `inline_asm_nop`（`asm!("nop")` 后返回 42 ⇒ exit 42）与 `inline_asm_ud2`
（`asm!("ud2")` ⇒ NTSTATUS trap `0xC000001D`）——后者能证明"字节真的被执行"（不只是编译通过）。

### M5-② `--subset rustc`：后端工作流一条命令（2026-10-11）

- 动机：全量门禁 44 步（约 10 分钟）里后端只占 2 步；改后端的人要么等全量、要么手工挑命令，
  容易漏守卫。
- 口径：`cargo forge gate --subset rustc` = `fmt` + `rustc-e2e` + `rustc-e2e-guards` +
  `capability_gaps` + `rustc_doc_counts` + `workarounds_guard` + `docs`（**7 步**，3–4 分钟）。
- 实现：**显式白名单**（`RUSTC_STEPS`）而不是给 `Scope` 加变体——后端步骤横跨 fmt/守卫/e2e
  三类，语义上不属于同一个 Scope，而白名单本身就是这个子集的文档。
- `rustc-e2e` 步显式 `--skip e2e_cargo_template_workflow`：该用例要 rustup **下载固定的
  dated 通道**（本机/沙箱里因写 `~/.rustup` 或没网络而失败），由 CI 的 `forge-rustc-e2e`
  作业覆盖——门禁跑"能跑的"，不装作跑过。
- 顺带修三处**存量漂移**（同类问题：写死会变的数字）：门禁步描述里的"110 用例"、`CLAUDE.md`
  的"28 条守卫"与"110 用例"、`CLAUDE.md` 的 TLS 状态行（已实现，但文档还写"待做"）。改法统一为
  **去掉具体数字/指向单一事实源**。

### M5 门禁与守卫收口 —— **已落地**（2026-10-11，工作区）

治的病（立项证据 E4/E9）：

- **E4 文档与代码分叉**：README 的「支持矩阵」写着「**103** 用例：98 硬断言 + 5 个 `FLAKY`
  容忍项」，而 `tests/e2e.rs` 实测是 **110 用例 / `known_failure: true` = 0 / `FLAKY` 为空**
  （那 5 例已随 `WA-42` 在 2026-09-12 全部转正）。两边谁都没错——只是**没有任何东西钉住它们**。
- **E9 门禁不覆盖后端**：`cargo forge gate` 对 rustc 只有 `check -p forge-rustc`，
  110 条"编译→链接→真执行"用例**只活在 CI 作业里** ⇒ 本地"全套门禁通过"并不覆盖后端真值。

落地的两件东西：

1. **两个新守卫**（都放在 `crates/backend/forge-codegen/tests/`，与既有的 `capability_gaps`
   同处——它们是仓库级守卫，不需要 rustc_private）：
   - `rustc_doc_counts`（`rustc_doc_counts_guard`）：README 里新增一段**机读计数标记**
     （`cases=… hard=… flaky=… known=… expect_trap=…`），守卫把它与 `tests/e2e.rs` 的实测
     计数**逐项双向比对**；README 缺标记 ⇒ 红（否则守卫"看不见"文档）。
     同时改正 README 的陈旧段落与 `Vec/String` 行。
   - `workarounds_guard`：`[WA-NN]` 编号唯一（口径 = **裸编号行唯一**，该表允许
     `WA-33 增补（M2）` 这类共用基础编号的续记行）+ 代码里的引用**都能在表里找到**。
     反向（表里有、代码没回指）只**报告不失败**——有些行是约定/工具链要求，本就没有代码站点。
2. **门禁两步**（`Scope::Windows`；跨目标见 M4）：`rustc-e2e`（110 用例真执行，
   `--test-threads=1`）+ `rustc-e2e-guards`（trap 语义/零容忍名单/模板契约不变量）。

守卫第一次运行就抓到两处真漂移（这正是它的价值）：

1. **`WA-26`/`WA-28` 是悬空引用**：代码注释（`lower/mod.rs`、`lower/rvalue.rs`）引用这两个编号，
   而表里**没有这两行**（某轮清理时删了行、注释没跟上）⇒ 读者照号去查会扑空。
   处置：补回两行并标明「**已并入**」（结论现由 `WA-44`/`WA-42` 承载），不删注释里的历史引用。
2. `WA-33` 有三行（原行 + `增补（M2）`/`（M10）`）⇒ 守卫口径修正为"裸编号行唯一"。

实测：`rustc_doc_counts_guard` → `{cases: 110, hard: 110, flaky: 0, known: 0, expect_trap: 1}`；
`workarounds_guard` → 表内 **43** 编号、唯一、代码引用全部可解析（12 个无回指，按设计只报告）；
`cargo forge gate --list` → 共 **44** 步（含上述两步与两个守卫）。

**门禁全量结果（同轮实测）**：`cargo forge gate` ⇒ **42/44 步通过**，两步失败且**都是沙箱外的写权限**，
与本次改动无关（同一会话里第 6 轮已记录过同类）：

| 步骤 | 失败原因（原文） |
| --- | --- |
| `workspace` | `cli_cargo_mode_run_mini_project` / `cli_init_then_bare_cargo_build` 失败：`failed to unpack package getopts v0.2.24` → `failed to create directory C:\Users\13523\.cargo\registry\src\…`（`workspace-write` 沙箱不允许写 `~/.cargo`） |
| `rustc-e2e` | 9 个测试函数里只有 `e2e_cargo_template_workflow` 红：`syncing channel updates…` → `could not create temp file C:\Users\13523\.rustup\tmp\…: 拒绝访问 (os error 5)`（同第 6 轮记录的 rustup 写权限） |

**实质验证**（剥掉环境因素）：e2e 的 **110/110 用例**与不变量守卫全绿（`rustc-e2e-guards` 步 `ok`；
单独跑 `e2e_stage_a_scalar_cases` 亦 110/110）；`workspace` 步的失败仅限那两个需要 cargo 解包
registry 的 cli 测试，其余套件全绿（含 2834 条 forge-codegen lib 测试）。
⇒ **M5 的交付物本身已验证**；那两步在无沙箱环境（CI）不受此限。

### M2-① `global_asm!` —— **已落地并实测**（2026-10-11，工作区）

**设计（先落纸后动手，照 §7 的参考实现）**：`global_asm!` 的输入是**汇编文本**（指令 + 标签 +
一串伪指令）。用我们自己的 ISA 汇编器（`TargetAssembler::parse_insts`）做等于把 LLVM 的 asm
parser 重写一遍（它既不懂伪指令也没有符号表）——**冗余**。真实后端（cranelift / gcc 后端）的
做法一致：**把文本交给系统上的汇编器**。本实现走 cranelift 那条**可移植**的路：另起一个
`rustc` 子进程（默认后端 = LLVM）把文本包进 `global_asm!(r###"…"###)` 汇编成对象。

**接入点（本仓已有基础设施，无需新造）**：`build_plans` 本来就**保留** `MonoItem::GlobalAsm`
（注释写着"GlobalAsm 不去重"），每个 CGU 的循环里有 `writer` 与 `outdir`；产物以
`CompiledModule::global_asm_object` 报给 rustc（`global_asm_object: Option<PathBuf>` 本来就是
rustc API 的字段，之前恒为 `None`）——rustc 侧 `rustc_codegen_ssa/back/link.rs:565` 会把每个
模块的这个对象加进链接命令。

**支持面**：字面文本（指令/标签/伪指令）**全覆盖**（因为就是 LLVM 的 asm parser）；
**`const` 占位符**（按常量类型的符号性格式化成十进制：有符号 ⇒ i128 负值可读、无符号 ⇒ u128）、
**`sym` 占位符**（复用 crate 内**同一份** `symbol_name_for_instance`——与函数/调用点符号同源，
名字不会两处漂移）均已支持；寄存器操作数与 `label` → `[cap:GLOBAL_ASM_OPERAND]`
（`global_asm!` 本不该有）。

`sym`/`const` 是**同一里程碑内**从 fail-closed 转正的（原先两个分支直接报缺口 ID）。
实测用例 `global_asm_sym_const`：`lea rax, [rip + {f}]`（`f = sym forge_sym_target`——
**名字写错会在链接期报未定义符号**，所以这条真的在测符号解析）+ `mov eax, {c}`
（`c = const 42`）⇒ **`PASS global_asm_sym_const exit=42`**。

踩到的 API 细节（本 nightly，写进代码注释）：`TyKind::FnDef` 的第二字段是
**`Binder<GenericArgsRef>`**（要先 `skip_binder()`，与 `lower/mod.rs` 直接调用路径同源）；
`ConstValue` 在 `rustc_middle::mir::ConstValue`（不在 `mir::interpret`）；
`InlineAsmOperand::SymStatic` 还有 `path` 字段（用 `..` 忽略）。

**实测**：e2e 新用例 `global_asm_call`（`global_asm!` 定义 `forge_gas_add`，Rust 侧
`unsafe extern "C"` 调用，20+22）⇒ **`PASS global_asm_call exit=42`**；产物链可见：
`forge_codegen_output.o` + **`forge_global_asm.o`（361 字节）**同目录产出。

**连带删除（"允许破坏性更新、无冗余"）**：原有负向用例 **`neg_global_asm` 删除**——它断言
"`global_asm!` 必须编译失败"（`expect_compile_err = "global_asm"`），被本项落地**正当地推翻**；
不支持的三类形态改由生成期的 `[cap:GLOBAL_ASM_*]` **定向 fail-closed**，不再需要笼统的负向用例。
全量 e2e 复跑 ⇒ **`passed: 106/106`**（用例数净不变：+`global_asm_call`、−`neg_global_asm`），
零 `KNOWN`。

踩坑两条（写给后续）：

1. **用了没登记的观测开关 ⇒ 硬 panic**：第一版写 `trace_enabled("GLOBAL_ASM")`，而 v26 M1 的
   开关台账守卫对未登记名字是**panic**（不是警告）⇒ 编译直接 101。改用**已登记**且语义匹配的
   `FORGE_TRACE_GLOBAL`（"全局与重定位符号解析"）——**不新增登记项**（无冗余）。
2. **子 rustc 必须清 `RUSTFLAGS`/`CARGO_ENCODED_RUSTFLAGS`**：否则子进程会再次加载本后端
   （`-Zcodegen-backend=<本 dylib>`）⇒ 递归。已在 `assemble()` 里 `env_remove` 并写进文档注释。

### M2-② 内联 `asm!` —— **明确拒绝（fail-closed）** + 堵住静默兜底（2026-10-11，工作区）

**取证（先量再改）**：`TerminatorKind::InlineAsm` 在 `lower/mod.rs` **没有分支** ⇒ 落进终结符
match 的兜底 `_ => { self.builder.unreachable(); }`。实测（`asm!("mov {0}, 7", out(reg) x)`
后返回 `x`）：**编译无声通过**，运行退出 **`0xC000001D`**（就是后端自己的 `unreachable` 兜底）
⇒ **静默错值**，违反仓库铁律。

**落地**：

1. `TerminatorKind::InlineAsm` 显式分支 ⇒ `IrError::capability("INLINE_ASM", …)`，诊断带
   **模板首片**（如 `"mov {…}, 7"`），并说明它为什么不能照搬 `global_asm!`：global_asm 是
   **整段独立**文本（交汇编器进独立对象即可），而内联 asm 要**嵌在函数体里**、还要绑定
   **寄存器约束** ⇒ 需要 IR 层能表达「原始文本 + 操作数寄存器约束」。
2. **兜底不再静默**：`_ => unreachable()` ⇒ `IrError::capability("MIR_TERMINATOR", …)` 并点名
   终结符 kind。静默 `unreachable` 会把「没实现」伪装成「程序自己不可达」——`InlineAsm`
   就被这么藏了很久。
3. CFG 可达性预扫补上 `InlineAsm` 的落点（本 nightly 字段名是 `targets: Box<[BasicBlock]>`，
   不是 `destination`；首元素 = 落点，除非 `options(noreturn)`/`naked_asm!`）。
4. **守卫扩面**：`capability_gaps_guard` 扫描面加 `crates/tools/forge-rustc/src` ⇒ 后端自己的
   缺口 ID 也进台账；台账加 `INLINE_ASM`（**待做**：需要 IR 层新节点 + 约束降级）与
   `MIR_TERMINATOR`（**不做**：兜底必须明确拒绝）两行，守卫双向绿。
5. **e2e**：新增负向用例 `neg_inline_asm`（`expect_compile_fail` + 断言 stderr 含
   `cap:INLINE_ASM`）。

**连带破坏性变更（M1 ↔ M2 的真实冲突）**：M1 的 panic 出口 `asm!("ud2")` 在 M2 下编不过
⇒ 改为 FFI `ExitProcess(101)`；M1 的守卫同步改判（模板必须用确定性 abort、且**不许出现内联 asm**）。

**实测**：全量 e2e ⇒ **`passed: 107/107`**、零 `KNOWN`；`e2e_trap_semantics_invariants` 绿；
`capability_gaps` 守卫绿。

**踩坑（自我命中第三次）**：新守卫「模板不许用内联 asm」的**针写在守卫自己那行** ⇒ 恒真失败。
改成**只扫模板切片**（`make_source` 起、到下一个顶层函数前）——既不自我命中，也比扫全文件精确。

### M2-③④ `panic=unwind` 决策 + TLS fail-closed（2026-10-11，工作区）

两项都做了**"先量现状 → 明确决策 → 进台账 → 负向用例钉住"**（M2 的"落地或明确降级"口径）。

**③ `panic=unwind` → 明确 `不做`（带缺口 ID）**

- 现状：拒绝点是 `codegen_crate` 里的 panic 策略检查，**原来只是一个裸字符串**——既不在
  缺口台账里、也没有守卫覆盖 ⇒ 读者从"缺口全貌"看不到它。
- 落地：改用 `IrError::capability("UNWIND", …)`（⇒ 被 `capability_gaps_guard` 双向钉住），
  台账加行、决策 **不做**，条件写清（真做需要：每函数 prologue 的 CFI 描述 + 展开指令表 +
  personality 实现 + rustc 侧 `unwind` 表接线）。
- 负向用例 `neg_panic_unwind`：**踩了一个真坑**——`no_std` + `panic=unwind` 下 rustc
  **先**要求 `eh_personality` lang item（缺它时分析期就报错，根本走不到后端的检查）⇒ 用例
  自带一个哑 `#[lang = "eh_personality"]` 才能推进到 codegen，从而真正断言那条拒绝。

**④ TLS → 先 fail-closed（`待做`）**

- 现状（本轮新发现）：静态数据路径**完全没有** thread-local 处理 ⇒ `#[thread_local] static`
  会走普通 static 分支、发射成 `.data`/`.rodata` ⇒ **线程私有语义静默丢失**（所有线程共享
  一份）——与 `InlineAsm` 同一类静默错值，只是没人碰过。
- 落地：`MonoItem::Static` 分支先查 `tcx.is_thread_local_static(def_id)` ⇒
  `IrError::capability("TLS", …)`（诊断带静态名）；台账加行、决策 **待做**，条件写清
  （`.tls$` 段 + TLS 回调目录/`__tls_index`（Windows）或 `%fs:` 相对寻址 + 访问模型降级；
  判据 = **双线程读写互不干扰**的真执行用例）。
- 负向用例 `neg_thread_local`（`#![feature(thread_local)]` + `#[thread_local] static mut`）。

**连带的 harness 变更**：新增**用例级 panic 策略开关**（用例名含 `panic_unwind` 时不加
`-C panic=abort`）——否则那条拒绝路径永远走不到。

**实测**：`neg_panic_unwind` / `neg_thread_local` 均 `PASS … compile-fail (gated)`；
`capability_gaps` 守卫绿（现在 forge-rustc 侧有 3 个 ID：`INLINE_ASM`/`UNWIND`/`TLS` +
兜底 `MIR_TERMINATOR`）。

### M2 缺能力闭环 —— **已收口**（2026-10-11，工作区）

| 项 | 状态 | 证据 |
| --- | --- | --- |
| `global_asm!` | **已实现**（含 `const`/`sym` 占位符） | `src/global_asm.rs`（新增）+ e2e `global_asm_call`（exit=42）/`global_asm_sym_const`（exit=42，`sym` 写错名字会在链接期报未定义符号）；`WA-03` 改判已关闭 |
| 内联 `asm!` | **fail-closed**（原来是静默错值）→ 真实现**另立 M2.5** | `[cap:INLINE_ASM]` + e2e `neg_inline_asm`；设计见下一节 |
| `panic=unwind` | **明确 `不做`**（带缺口 ID） | `[cap:UNWIND]` + 台账行 + e2e `neg_panic_unwind` |
| TLS | **fail-closed**（原来是静默丢语义），决策改为 **`不做`**（条件见下节：推进 std 时必须先做路线 B） | `[cap:TLS]` + 台账行 + e2e `neg_thread_local` |

**顺带堵住的静默黑洞**：MIR 终结符 match 的兜底 `_ => unreachable()` 改成
`[cap:MIR_TERMINATOR]` 明确拒绝——静默 `unreachable` 会把「没实现」伪装成「程序自己不可达」
（内联 asm 就是这么被藏了很久）。

**守卫**：`capability_gaps_guard` 的扫描面加 `crates/tools/forge-rustc/src` ⇒ 后端侧 4 个 ID
（`INLINE_ASM`/`UNWIND`/`TLS`/`MIR_TERMINATOR`）与台账**双向相等**；e2e 目标三个"不变量"
（trap 语义 / 零容忍名单 / 模板不许用内联 asm）由 `e2e_trap_semantics_invariants` 钉住。

**实测**：全量 e2e **`passed: 110/110`**、零 `KNOWN`、`known_failure: true` = 0；
`capability_gaps` + `e2e_trap_semantics_invariants` 均绿；文档 0 issues。

### M2 剩余项的设计（内联 `asm!` 真实现，先落纸）

**为什么不能照抄 `global_asm!` 那条路**：global_asm 是**整段独立文本**（交系统汇编器产独立对象、
报 `global_asm_object` 即可）；内联 asm 要**嵌在函数体里**、并且要绑定**操作数寄存器约束**
（`in(reg)`/`out("rax")`/`inout`…），它影响的是**本函数的指令流与寄存器分配**。

**三条候选路线与取舍**：

| 路线 | 做法 | 判断 |
| --- | --- | --- |
| **A. IR 层新增「原始文本 + 约束」节点**（推荐） | `forge-ir` 加一个带文本与操作数槽的节点（`IR_FORMAT_VERSION` 4→5）；约束在**降级期**绑定到 IR vreg（`in(reg)` = 任选 GPR、`in("rax")` = 定死物理寄存器）；**发射期**（regalloc 之后、物理寄存器已知）把文本按占位符替换成真实寄存器名并交给汇编器编码进本函数代码流 | 唯一语义正确的路线；跨 crate（foundation + backend），工作量最大 |
| B. 每处 asm 合成一个**本模块函数**再 `call` | 复用 global_asm 的"交汇编器"机制 | ✗ **语义错误**：多一层帧、破坏 `nostack`/clobber 期望，`asm!("mov rax,[rsp+8]")` 这类会读错栈 |
| C. 只用我们自己 ISA 汇编器解析文本成 IR 指令 | 无外部工具链 | ✗ 方向不对：ISA 汇编器产出 `Inst`（ISA 层），而 forge-rustc 造的是 forge-ir；且它不懂伪指令/符号表 |

**选 A，分四阶段**（每阶段都可独立验收）：

- **S1 无操作数、无标签、无 clobber 的文本**：文本按行用 A 的发射期汇编器编成字节，直接进本函数
  代码流（`asm!("sfence")`/`asm!("ud2")`/`asm!("pause")` 这类）。判据：e2e 用例真跑。
- **S2 `in(reg)`/`out(reg)`/`inout(reg)`（任选寄存器）**：占位符 `{0}` 交给发射期替换；
  操作数用 IR vreg 表达，寄存器类别由 `reg_class` 决定（GPR/FPR）。
- **S3 定死物理寄存器与显式约束**（`in("rax")`、`out("xmm0")`、`const`、`sym`）：需要"预着色"
  （IR vreg 钉到物理寄存器）——主线已有"写死物理寄存器"的指令槽机制可复用（见 `WA-40` 的上下文）。
- **S4 `clobber_abi` / `options(nostack, nomem, pure, noreturn)` / 标签**：语义收尾。

**在那之前，现状是 fail-closed**（`[cap:INLINE_ASM]` + `neg_inline_asm` 负向用例）——
比落地前的"静默降级成 `unreachable`"严格更好。

S1 的接入点侦察（2026-10-11，只读实测——比上面的路线 A 更具体）：

- **op 是数据**：宿主 op 表是 `ops.toml`（`VaStart` 就是 2026-09-30 进表、
  2026-10-01 归入 `HOST_PIPELINE_OPS` 的先例）⇒ 新增 `InlineAsm` op 走同一条路，
  **不需要**在 forge-ir 里手写枚举分支。
- **载体已有**：`forge_ir::util::imm_str::ImmStr` 是 IR 里的字符串载体（`CallConvId::Named`/
  debug info 都用它）⇒ 原始汇编文本可以原样作为 op 的 payload。
- **发射期能追加原始字节**：`pipeline::emit::CodeSink` 有 `bytes()`/追加接口（
  `alloc_integration_tests.rs` 直接用 `sink.bytes()` 做断言）⇒ "把汇编文本编成的字节
  拼进本函数代码流"这一步在发射层是现成的。
- **扩展 pass 的先例**：`pipeline/va_expand/` 正是"把宿主伪 op 展开成显式指令"的现成形态
  ⇒ S1 可以照它写成 `inline_asm_expand`（把无操作数文本用 ISA 汇编器解析成机器指令、
  就地插入当前块），S2+ 再引入操作数绑定。
- **结论**：S1 的改动面比预估小（ops.toml + 一个扩展 pass + forge-rustc 侧改为发该 op），
  `IR_FORMAT_VERSION` 是否要升取决于 op 表是否进二进制格式签名段——动手前先确认这一点。

### M2 剩余（下一轮起）

- **内联 `asm!` 的真实现**（③④ 的决策已在前两节落地）（`RUSTFLAGS` 那条同源；`InlineAsm` 的寄存器操作数解析 + 发射）；参考
  cranelift 的 [`inline_asm.rs`](https://rust.googlesource.com/rust/+/d14c1c75ab284d382bd1e9c499596c274f1abe58/compiler/rustc_codegen_cranelift/src/inline_asm.rs)。
- **`panic=unwind` 二选一决策**（发射 SEH 表 / 明确不做并进缺口台账）与 **TLS**。

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

**e2e 目标整体验收（同轮实测）**：`cargo test -p forge-rustc --test e2e -- --test-threads=1`
⇒ **8 passed / 1 failed**。唯一失败是 `e2e_cargo_template_workflow`，且是**环境性的**：

```text
info: syncing channel updates for nightly-2026-09-05-x86_64-pc-windows-msvc
error: could not create temp file C:\Users\13523\.rustup\tmp\…: 拒绝访问。 (os error 5)
```

它在 **rustup 同步阶段**就失败（早于任何编译/后端行为）：本会话的文件沙箱是 `workspace-write`，
而 rustup 要写 workspace 之外的 `~/.rustup/tmp` ⇒ **与 M1 改动无关**（该用例走
`cargo +nightly …` 子进程，不经过被改的 `run_case`/`run_exe_with_timeout` 路径）。
⇒ M1 的验收口径取：**106/106 用例 + 新守卫 + 另外 8 个 e2e 测试全绿**；此例在沙箱内不可跑
（CI 上不受此限）。

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

判据达成：`PASS overflow_runtime_trap`（`FORGE_E2E_ONLY` 单跑）；静默回绕会正常退出
`-2147483648`（≠ 期望码）⇒ 必红；断言失败边不触发（挂起）⇒ 也必红。

> ⚠️ **M2 更正**：本项第一版的 panic 出口是 `asm!("ud2")`（期望码 `0xC000_001D`）。M2 把内联
> `asm!` 改成 **fail-closed** 后，模板自己用 `asm!` 会让**全套用例编不过**（实测）⇒ 改为
> **FFI `ExitProcess(101)`**（`#[link_name]` 别名，避免与既有 `ffi_exit_process` 撞名——
> 这个撞名也是实测撞出来的，全量套件当场 106/107）⇒ 期望码 = **101**（`TRAP_EXIT`）。
> 确定性更强（不依赖硬件异常码）；`is_trap_code` 仍用于识别**未预期**的 NTSTATUS 崩溃。

**全量验收（同轮实测）**：`cargo test -p forge-rustc --test e2e -- --nocapture
--test-threads=1 e2e_stage_a_scalar_cases` ⇒ **`passed: 106/106`**、`test result: ok`、
**零 `KNOWN`**、95 s（单机、串行）⇒ ① 改造对既有 105 例无回归；② 代码里
`known_failure: true` **已归零**（M1 的一条硬判据达成——最后一个是本里程碑的溢出探针，
现已转正为硬断言；`rg` 计数时注意排除文档注释里那处 `known_failure:true` 的历史说明）。

遗留（下一轮）：`run_exe_with_timeout` 的 `Some(None)`（宿主强行结束）现在归成 `TRAP_EXIT`；
`CI_ENV_AV_CODE`（`0xC0000005`）这条历史容忍码在新语义下走 trap 分支——`FLAKY` 名单为空且只在
`FORGE_E2E_STRICT_FLAKY=1` 时有意义，故当前无影响，但 **M5 的守卫要把它钉住**（否则将来把 AV
当"环境性"容忍会重新走进这条路）。
