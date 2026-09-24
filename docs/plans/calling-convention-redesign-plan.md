# 调用约定重设计（v20，A1–A7）

> **状态：[progress]**（A1 已落地，2026-09-24）。现行参考见
> [`docs/reference/calling-conventions.md`](../reference/calling-conventions.md)；
> **代码与测试为准**，本文只记设计取舍、分期与进度。

## 1. 问题（四条代码级证据）

原设计把调用约定写在 ISA 谱的 `[abi]` 节里。四条硬伤都有代码证据（行号随迭代漂移，
以符号名为准）：

1. **IR 里的约定是死值**：`forge-ir` 的 `CallConv`（16 个变体 + `Custom(u32)`）全仓只有
   一处**写**（`crates/backend/forge-codegen/src/pipeline/compiler.rs` 设 `ctx.call_conv`），
   **没有任何地方读**——它既不影响布局也不影响发射，等于文档。
2. **同一份约定在每个 ISA 里各写一遍、各写各的**：x86 谱声明了
   `push`/`pop`/`stack_arg_load`/`stack_arg_store`/`frame_addr`/`wide_vec_store|load` 等角色，
   arm64 谱只有 `gpr_mov`/`frame_alloc`/`frame_free`/`jump`/`epilogue_jump`/`ret`/`branch`。
3. **换 ISA 就错**：间接结果指针（sret）被硬编码成"**首 int 参数槽**"——那是 Windows x64
   的 RCX 语义；AAPCS64 用 **x8**、RISC-V 用 **a0**。同理"栈参数偏移""callee-saved 序列"
   各写各的，靠人工对齐。
4. **变参无处可写**：`Signature.variadic` 与 `Opcode::VaArg` 都在，但没有"未命名实参放哪、
   `va_list` 什么形状、SysV 的 `%al` 谁填"的模型（覆盖率里长期挂着这条）。

## 2. 目标与非目标

**目标**（对应"足够通用，而非服务于个别指令集"）：

- 约定是**使用者的数据**（TOML/构造器 + 钩子），不是编译器里的 if-else；
- **同一台机器挂多份约定**（x86 同时挂 `win64` 与 `sysv64`）、**同一份约定挂多台机器**
  （换 ISA 只换寄存器绑定）；
- ISA 只申报**能力**；能力缺口在**规划期**报，不拖到生成/运行期；
- **fail-closed**：表达不了的形态一律明确报错，绝不"差不多能用"；
- 允许破坏性更新（无兼容层、无迁移工具）。

**非目标**：本层不发射机器码、不管寄存器分配策略、不做语言前端（这些分别是 A3+ 与
各前端的事）。

## 3. 分层与数据

```text
L0 forge-ir         声明"用哪份约定"（CallConvId）+ 每条实参/形参的属性（A2）
L1 forge-abi        数据（AbiRules / AbiBinding）+ 通用引擎（A1 ✅）
L2 ISA 谱           只申报能力：寄存器/宽度/角色（A5）；[abi] 约定节删除
L3 管线             按 AbiPlan 发射调用点/入口/序尾声（A3–A4）
L4 使用者           提供约定与绑定（rustc 前端 / HIR / mini_c / 你自己的语言）
```

三份数据（细节见参考文档）：`AbiRules`（约定，平台无关）→ `AbiBinding`（(ISA, 约定)
的寄存器绑定）→ `AbiPlan`（纯数据产物，调用方与被调方共用）。

## 4. 三个需要拍板的点（已定）

| 议题 | 选择 | 理由 |
| --- | --- | --- |
| 约定的表达媒介 | **数据（TOML + 构造器）为主，`AbiHooks` 兜底** | 数据能覆盖 C 家族全部四份约定；Swift `self`/Go context 这类语言专属约定用钩子，不污染引擎 |
| `[emit.prologue\|epilogue]` 的存废 | **删除**，由引擎按能力生成 | 手写发射序列正是"约定写两遍"的来源；能力（`sp_adjust`/`push`/`frame_addr`…）足以生成 |
| IR 里的约定标识 | `CallConvId::{Builtin, Named(ImmStr), Index(u32)}` + 宿主注册表，未注册 ⇒ fail-closed | `CallConv::Custom(u32)` 解码得出却无表可查，等于假的支持 |

## 5. 分期与进度

### A1 ✅ 数据 + 引擎 + 参考数据 + CLI

#### 产物

- 新 crate `crates/foundation/forge-abi`：`AbiRules` / `AbiBinding` / `AbiTarget` /
  `plan_fn` → `AbiPlan` / `AbiRegistry` + `AbiHooks`。
- 内置约定 5 份：`c`（抽象基类）+ `win64` / `sysv64` / `aapcs64` / `lp64d`。
- 参考绑定 4 份：`crates/foundation/forge-abi/conventions/*.toml`（含"为什么这么绑"的注释）。
- `forge_isa_dsl::abi_view`：**从谱读能力视图**（寄存器表/固定用途/链接寄存器/角色→能力），
  编号与 `TargetRegInfo::num_gp_regs`/`num_fp_regs` 对齐。
- CLI：`forge-isa abi list|check|plan`。
- 测试：黄金快照 4 份（四约定 × 21 条语料）+ 不变量 18 条 + fail-closed 目录 14 条、
  CLI 端到端 5 条。

#### 实现中发现并修掉的模型缺口

这些都不是风格问题，是会写错值的：

1. **整数聚合被误当 HFA**：同质判定不分整数/浮点 ⇒ `{i64,i64}` 会进浮点寄存器
   （RISC-V 应走 a0:a1）。现改为只认**浮点**成员。
2. **HFA 槽数写死**：`slots = 4` 让 `{f32}` 白吃 3 个寄存器并把后面的参数挤到栈上。
   现加 `slots = "hfa"`（按类型取成员数）。
3. **返回位与参数位共用规则**：AAPCS64 的 >16B 聚合当参数是 byval、当返回是 x8 sret；
   共用一份规则会把"返回值就是这个指针"当成事实。现加 `ret_classify`（非空即独占）。
4. **返回寄存器与参数寄存器共用池**：Win64 标量返回在 RAX、参数从 RCX 起；共用池
   会让返回值与参数抢寄存器，且按位置计数时一个 2 槽返回会把第一个参数挤走。
   现分 `ret_int`/`ret_float` 池 + 两个独立游标。
5. **byval 副本被分配在传出参数区**：副本是调用方帧内临时量，混进参数区会与后面的
   栈参数抢内存。现分 `byval_area_bytes`（`Placement::Indirect.at` 的坐标系）。
6. **`ClassRule.do_` 缺 `#[serde(rename = "do")]`**：TOML 里写 `do` 直接解析失败
   （把 5 份内置约定的解析整片打挂，`cargo check` 发现不了——只有跑起来才知道）。
7. **≥3 连续槽**：AAPCS64 的 4 成员 HFA 需要 4 个连续浮点寄存器，原模型只能表达 1/2
   ⇒ 加 `Placement::RegGroup`（参数位）；返回位本片明确 `Unsupported`（A6）。

#### 实测结论（2026-09-24 本机）

| 检查 | 结果 |
| --- | --- |
| `cargo test -p forge-abi` | 快照 2 + 不变量 18 + 错误目录 14 全绿 |
| `forge-isa abi check isa/x86_v12.toml` | `win64` / `sysv64` 各 15 条代表签名**全部可规划** |
| `forge-isa abi check isa/riscv64_v12.toml` | `lp64d` 15 条全绿 |
| `forge-isa abi check isa/arm64_v12.toml` | `aapcs64` **6 条缺口**（无 FPR 组 ⇒ `float`/`ret_float` 池缺）——与矩阵 175 条 skip 同源 |

#### 边界

本片**只新增**（没有消费者），因此现有编译行为**逐字节不变**：`[abi]` 节仍在谱里、
管线仍按老路走，直到 A3–A5 切换。

#### CI 上抓到的两件事

1. `Test (Windows)` 红了一次：黄金快照比对没归一化换行——Windows 上 git 按 CRLF 检出
   签入的 LF 文件，`str::lines()` 又会吃掉 `\r`，于是"直接比字符串会红、只比 `lines()`
   看不见"，现象是**差异点报在文件末尾 + 期望 `<缺行>`**。已修（比之前两边都折成 `\n`）
   并做了 CRLF 变异实测。
2. `forge-rustc (e2e, Windows)` 仍是**历史性红**（与本片无关，见
   [`forge-rustc-vec_push-plan.md`](forge-rustc-vec_push-plan.md)）。

### A2 ✅ IR：`CallConvId` + 宿主注册表读路径（本片）

#### 产物

- `forge-ir` 删掉 16 变体的 `CallConv`，换成
  `CallConvId::{Builtin(ConvName), Named(ImmStr), Index(u32)}` + `ConvName`
  （`c`/`sysv64`/`win64`/`aapcs64`/`lp64d`）。默认 = `Builtin(C)`，文本层对 `c` 不写关键字
  （与 LLVM 一致）。
- **文本表只有一份**（`to_text`/`from_text` 互为逆）：`win64cc`/`fastcc`/`cc N` 等 LLVM 拼写
  与规范名共用一张表；未知标识符 → `Named`（原样保留）。旧实现把未知名字折成
  `Custom(len ^ 0x8000_0000)`——既还原不出来，也没有任何代码读。
- **二进制格式升到 v3**（`IR_FORMAT_VERSION = 3`）：签名体里那 1 字节判别值改成 tag
  （`0..=4` 内置 / `5` 命名（字符串表下标）/ `6` 数值（varint））。`c` 仍是 1 字节
  ⇒ 字节偏移类的手工测试不受影响。无兼容读取（设计前提就是破坏性更新）。
- **读路径**（本片的关键）：`forge-codegen::pipeline::conv_registry` —— 宿主注册表
  （内置五份 + `register_rules_toml`/`register_binding_toml` 加自己的），
  `resolve()` 把标识解析成注册表键（`Index(n)` → `"cc{n}"`）并落在
  `LowerCtx::call_conv_name`。**未注册 ⇒ `CompileState::new` 直接报错并列出已注册的名字**。
  旧实现 `ctx.call_conv = …` 只写不读，全仓无人读——现在这条链路是活的。
- 测试：`crates/backend/forge-codegen/tests/conv_registry_read_path.rs`（3 条）——
  内置五份可解析；未注册的 `Named`/`Index` 都 fail-closed 且消息列已知名字；
  **端到端**："同名 IR 未注册时编译失败 → 注册后编译通过"（⇒ 改约定名确实改变规划走的数据）。

#### 还没做（下一步）

- **签名级参数属性**：`ParamAttributes`（`byval`/`sret`/`inreg`/`zeroext`/`signext`/`align`…）
  目前挂在 `Function` 上（按参数索引），**调用点**要用的 `FunctionSignature` 还没有；
  A2b 把它变成签名的一部分（单一事实源），并让文本/二进制、verifier 一起走。
- **属性 → `AbiPlan` 的投影**：`TypeId + TypeStore + DataLayout + ParamAttributes → TyView`
  与按属性覆盖分类（`byval(N)` → `Indirect{CallerStackCopy}`、`sret` → `Indirect{HiddenSret}`…）。
  这一块与 A3 的 `AbiTarget` 适配器（`TargetRegInfo` → 引擎）同批做，避免两套投影。
- `forge-rustc` 的 `PassMode` → IR 属性的落库（现在前端从不写 IR 的约定/属性，
  这也是旧设计"死值"的另一半原因）。

### A3 调用点/入口/返回值按 plan 发射（x86 优先）

- 管线按 `AbiPlan` 走：实参搬运、返回值搬运、栈参数 store/load、sret 指针。
- 验收：**x86 的生成物逐字节不变**（`forge-codegen` 全套测试 + 三 ISA 矩阵）；
  再开 riscv64/arm64。
- 这一步会删掉"首 int 参数槽"那类硬编码。

### A4 序/尾声/帧按 plan 生成

- 删 `[abi.frame]` 的帧填充与 `[emit.prologue|epilogue]`；由 `callee_saved` 机制 +
  能力（`sp_adjust`/`push`/`frame_addr`）生成。
- 验收：三 ISA 的帧布局与保存序列逐字节不变（`fp-outside`/`fp-inside` 两种模式各有快照）。

### A5 删谱里的 `[abi]` 约定节，加 `[machine]`

- 新增 `[machine] { fixed_regs, spill_scratch, link_reg }`（只留**机器事实**）；
  参数池/sret/callee-saved/栈参数布局全部移到绑定与规则里。
- 三份发行谱 + 四份绑定同步迁移；arm64 补 `[reg.fpr8]`（`V0..V31`）与浮点搬运角色
  ⇒ `abi check` 的 6 条缺口清零。
- 验收：`forge-isa validate` 三谱零诊断、`abi check --strict` 三谱零缺口、
  矩阵 skip 数下降。

### A6 变参 / HFA 部分在寄存器 / ≥3 槽返回 / 罕见的弹栈约定

- va_list 取用（SysV 寄存器保存区 / Win64 栈指针 / AAPCS64 结构 / riscv 保存区）；
  变参元信息寄存器（`%al`；`LEN` 以官方 psABI 定本为准）。
- HFA/HVA 的"寄存器不够时部分在寄存器"（需要按成员赋值的规则语言）。
- ≥3 槽返回的搬运；`stdcall`/`thiscall` 的 `callee_pop`（模型已有，待管线消费）。
- 红区（SysV 128 字节）与尾调用约束（`tail_calls.must_match_stack`）。

### A7 可选的异域钩子示例

- 给 Swift（`self`/`error`）、Go（context/GC 安全点）各写一个 `AbiHooks` 示例，
  证明"数据表达不了的部分"有正规出口（而不是改引擎）。

## 6. 风险与对策

| 风险 | 对策 |
| --- | --- |
| A3/A4 切换时改变既有生成物（回归面大） | 以"逐字节不变"为验收：先 x86，再 riscv64/arm64；每一步都跑三 ISA 矩阵 |
| 约定数据与谱的事实漂移（拼错寄存器名） | `abi check` 的硬错档（`UnresolvedReg`）+ `builtin_catalog_is_self_consistent` 守卫 |
| 缺口被当成"绿" | `abi check` 默认把缺口与硬错分开报，`--strict` 才让缺口影响退出码；缺口清单在参考文档里逐条列出关闭时机 |
| 参考文档与代码漂移 | 参考文档只描述已落地的东西；快照/清单类断言都在测试里（本文不复制数值） |
