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
  3. 已知会遇到：std 的 `thread_local!`（后端现对 `#[thread_local]` 是 `[cap:TLS]` fail-closed）、
     std 的 `lang_start` 入口链（`mainCRTStartup` → `std::rt::lang_start`）、
     以及内联 `asm!`（`[cap:INLINE_ASM]`，若用户 crate 或平台胶水层用到）。
     ⇒ **M3 的真实顺序依赖 M2.5（内联 asm）与 TLS**，这一点本轮已用代码事实确认，不是猜测。

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
