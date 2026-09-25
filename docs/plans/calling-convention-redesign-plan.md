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

- **宿主 `AbiTarget` 适配器**（`TargetRegInfo` → 引擎）：把 IR 签名真正跑成 `AbiPlan` 并
  在编译入口校验（本片只做"解析约定名 + 投影签名"；跑 plan 与按 plan 发射是 A3）。
- `forge-rustc` 的 `PassMode` → IR 属性落库（现在前端从不写 IR 的约定/属性，
  这也是旧设计"死值"的另一半原因）。

### A2b ✅ 声明属性真正改变规划（本片）

- **引擎吃声明属性**（`forge_abi::DeclAttrs`）：`Signature` 新增 `attrs`/`ret_attrs`，
  `plan_fn` 按属性改分类与落点——`byval(N)` → 栈上副本 + 指针（副本进 `byval_area_bytes`）、
  `sret` → 占约定声明的 hidden 槽（**逐约定不同**：x86 RCX/RDI、AAPCS64 x8、riscv a0）、
  `inreg` → 分类说要走栈时再试寄存器（池空仍走栈，不硬凑）、`zeroext`/`signext` → 落点的
  `Extension`（两个都写时 `signext` 胜）、`align(N)` → 栈落点对齐。
- **IR 侧接上**（`forge_codegen::pipeline::sig_view`）：`Function::param_attrs`/`ret_attrs`
  与 `FunctionSignature`、`TypeStore` → `forge_abi::Signature`。这些字段**以前只有文本层
  认识、没有任何代码读**（与旧 `CallConv` 一样是装饰）；类型投影只摊开不猜测（聚合体用
  TypeStore 的权威大小，可扩展向量落 `Other`）。
- 测试：`forge-abi/tests/invariants.rs` 新增两条（`declared_attributes_change_the_plan`：
  byval/sret/两个 ext/align 逐条断言产物；`inreg_overrides_a_stack_classification`：池够与
  池耗尽两种行为），以及 `forge-codegen/tests/conv_registry_read_path.rs` 的投影用例
  （含"带填充结构体不能被按成员求和"这条反例）。

### A3a ✅ 宿主适配器（真实后端 → 引擎）

- `TargetMachine::role_bits(role) -> Option<u16>`（`forge-isa-runtime` 的 trait 默认 `None`）：
  **ISA 申报能力**的正式出口；DSL 从 `[[instructions]].roles` 生成
  `match role { "gpr_mov" => Some(64), … , _ => None }`（能力名折算与 `forge-isa abi check`
  的静态视图**同源**，都走 `abi_view::role_capability`，所以两边不会漂移）。
- `forge_codegen::pipeline::abi_target`：`MachineAbiTarget`（`TargetRegInfo` → `AbiTarget`：
  索引空间 = GPR 区 + FP 区、名字用生成枚举的变体名、pinned = 不在可分配表、能力走
  `role_bits`）+ `plan_for_function`/`plan_for_signature`（IR 签名 → `AbiPlan`）。
- 交叉核对测试 `tests/abi_target_real.rs`：真机 `win64` 上同一个签名给出与 A1 合成目标
  **相同的落点**（RCX / XMM1 / 返回 RAX、shadow 32、callee-saved 来自真表），未注册约定
  在真机上同样 fail-closed。
- **还没做（A3b）**：按 plan 发射调用点/入口/序尾声（x86 优先，验收 = 生成物逐字节不变）；
  适配器的 `link_reg()` 目前返回 `None`（`TargetRegInfo`/`TargetABI` 都没暴露 ra/lr，
  A5 随 `[machine]` 补）。

### A3b-1 ✅ plan 接进编译入口（只算不用）

- `CompileState` 新增 `abi_plan`/`abi_plan_note`：编译入口用 A3a 的适配器把该函数的
  `AbiPlan` 算出来挂上；算不出来**不阻断编译**（发射还没用它），原因留档。
- `FORGE_TRACE_ABI=1` 打印计划（`AbiPlan::to_text()`）或失败原因——发射尚未切换期间，
  这是"计划长什么样"的唯一证据面。
- **前置核对**（`tests/abi_target_real.rs` 新增）：引擎的计划必须与**当前**发射路径的
  `AllocResult` 一致（sret 有无、逐参数 by-ref、栈参数区字节数）——这是把发射切到 plan
  之前唯一能先做的正确性检查，也是 A3b-2 的入场券。

### A3b-2a ✅ 中性调用布局（发射切换的地基）

- `forge-isa-runtime` 新增 `machine::call_layout`：**中性数据**（`CallLayout`/`CallArg`/
  `ArgPlace`/`RetPlace`/`Ext`）——一次调用的实参落点、返回、隐藏 sret、栈区尺寸、
  callee-saved、被叫方弹栈。寄存器存 **(类, 类内号)**（`RegClass` 是 forge-ir 的中性类型），
  生成物用 `Reg::from_index(i, class)` 还原；**运行时因此不依赖 forge-abi**。
- `forge-codegen::pipeline::abi_target::call_layout(plan, machine)`：`AbiPlan` → `CallLayout`
  （ABI 空间号折回类内号、`sret` 标记打到落点上）。
- 管线把它塞进 `LowerCtx::call_layout`（`None` = 没接上/算不出 ⇒ 走既有 `[abi]` 路径）。
- 测试 `abi_target_real.rs`：真机 `win64` 上逐项核对（RCX=(GPR(8),1)、XMM1=(FPR(16),1)、
  返回 RAX=(GPR(8),0)、callee-saved 含 RBX=(GPR(8),3)、`caller_offset` 算法、shadow=32）。
- **发射仍未切换**（生成物读的还是 `TargetABI`）⇒ 生成物逐字节不变；下一步才是把生成物的
  `move_args`/收参/序尾声改读 `ctx.call_layout`，并用"全 JIT 矩阵逐字节不变"验收。

### A3b-2b-1 ✅ 序/尾声侧的通路（AllocResult 带上布局）

结构事实先钉住：`TargetFrameLowering::emit_prologue/epilogue` 只拿到 `frame_size` +
`AllocResult`，**没有** `LowerCtx`；而 `@move_args`（收参）是生成 lowering 的一部分、有
`LowerCtx`。所以"生成物改读 plan"有两条通路，序/尾声这条必须先把布局**放进 `AllocResult`**：

- `AllocResult` 新增 `call_layout: Option<CallLayout>`（默认 `None`），管线在收尾处从
  `LowerCtx::call_layout` 填入；`None` = 没接上/算不出 ⇒ 帧件走既有 `[abi]` 路径。
- 测试 `abi_target_real.rs`：钉住"管线确实把它带到了帧件那里"（conv/shadow/首个实参
  RCX=(GPR(8),1)/callee-saved 非空）。
- 发射仍未切换 ⇒ 生成物逐字节不变。

### A3b-2b-2a ✅ 缺省约定 `c` 的绑定 + 收参（`@move_args`）改读布局

**先补的前提**：IR 的缺省约定是抽象名 `c`，而内置绑定只有 `win64`/`sysv64`/`aapcs64`/
`lp64d`——`c` 在真机上**没有绑定** ⇒ `call_layout` 永远是 `None`，生成物的布局路径
永远不生效（"接了但没生效"）。补法是把它变成**数据**：`AbiRules` 新增
`aliases = ["c"]`（"这台机器上的 C 约定就是本约定"），内置 `win64`/`aapcs64`/`lp64d`
声明它、`sysv64` 刻意不声明。

**必须是"整套代答"**（`AbiRegistry::resolve_conv`）：解析出的必须是同一份约定的
**规则 + 绑定**。踩过的坑（2026-09-25 实测）：先写成"只让**绑定**代答"——`c` 的通用规则
配上 Win64 的寄存器池，于是 by_position 变成 by_class、返回池从 RAX 变成 RCX——
`test_jit_mixed_int_float_args` / `test_jit_v128_byval_mixed_int_pos` /
`test_jit_sret_with_byref_arg` 三个用例当场红。纪律两条：同一机器上两份约定都声称
代答同一别名 ⇒ **报错**（不按注册序猜）；显式 `(ISA, "c")` 绑定**优先**于别名
（宿主覆写入口：连规则一起自己提供）。

**收参切换**（x86 优先，riscv/arm64 同时受益于同一份生成物）：

- `@move_args` 的入参来源改读 `AllocResult::call_layout`：`ArgPlace::Reg`（类 + 类内号，
  int 类走 `gpr_mov`、浮点/向量类按类宽分派 `vec_mov`/`fpr_mov`）与带指针的
  `ArgPlace::Indirect`（宽向量 by-ref，走 `wide_vec_load_32/64`）。
- **入场判定 `__layout_ok`**：只有**每个**形参都落在本片覆盖的落点时才启用；有一个
  不支持（`Pair`/`Group`/`Stack`/无指针的 `Indirect`）就**整函数**退回既有 `[abi]`
  路径——两条路径不混用（混用会让旧路径的 `__gi`/`__fi` 游标错位），也让"逐字节不变"
  这条验收可判。`call_layout = None`（没绑定/规划失败）同样退回。
- 结构守卫 `crates/frontend/forge-isa-dsl/tests/call_layout_emission.rs`：三份发行谱都
  发射了布局路径，且它排在旧路径之前（"接了却没生效"这类退步只有文字级守卫抓得到）。
- 实测（2026-09-25，本机）：x86 `c` 已能规划（首参 RCX=(GPR(8),1)，
  `abi_target_real.rs::the_default_convention_plans_and_reaches_the_frame_lowering`）；
  **x86 矩阵 195 passed / 3 skipped / 0 failed**、**riscv64 矩阵 131 passed / 67 skipped /
  0 failed**、`forge-codegen` 全套绿 ⇒ 收参换来源后行为不变。
- **仍未切**：栈参数的 load/store、`byval` 副本、序尾声（`@push_callee`/`@frame_alloc`/
  `{callee_saved_bytes}`）仍读 `[abi]`——A3b-2b-2c 与 A4。

### A3b-2b-2b ✅ 收参从谱面撤出（`@move_args` 退役，生成器插入）

**为什么**：`@move_args` 是**调用约定**的内容，谱不该写它——"什么时候收参"由生成器决定
（callee-saved 保存**之后**），"收到哪"由 forge-abi 的布局决定（上一片已切）。

- 伪指令白名单里**删掉 `move_args`**；谱里再写它会被**明确拒绝**并给出迁移提示
  （`validate.rs` 单列一条诊断，不是含糊的"未知伪指令"）。
- 生成器在序言里**自动插入**收参：位置 = 最后一个 `@push_callee` 之后；模板里没有
  `@push_callee`（手工保存的夹具）则放最后——两者都在所有保存之后。**顺序不是风格问题**：
  保存若晚于收参，存下来的是实参值而不是调用者的寄存器值，尾声恢复会毁掉调用者的寄存器。
- `[emit.prologue]` 因此**可以缺席**（`gen_emit_block` 不再因模板缺失而整体早退）：
  缺席 = 空模板 + 收参。demo 夹具的整段 `@move_args` 模板随之删除。
- 三份发行谱 + 两份夹具的模板里删掉 `@move_args`（其余四个伪指令仍在，A4 才轮到它们）。
- 验收：**生成物逐字节不变**——`FGE_DEBUG_GEN=1` 对 10 份生成模块（三份发行谱 + 七份
  夹具/多文件谱）取样，改动前后 **SHA256 全部相同**（`target/gen-baseline` 对照）；
  workspace 全套 serially 绿。
- 守卫：`call_layout_emission.rs` 新增三条（写 `@move_args` 必报错且提示指路；缺席模板
  的谱仍发射收参；收参位置在保存之后、帧分配之前）。

### A3b-2b-2c 帧件与栈参数改读 call_layout（未开始）

**剩余面**：`ArgPlace::Stack`（栈参数 load/store + spill 槽坐标）、`byval` 副本
（`Indirect { reg: None, on_stack: true }` + `byval_area_bytes`）、`Pair`/`Group`，
以及序尾声的 `@push_callee`/`@pop_callee`/`@frame_alloc`/`{callee_saved_bytes}`
（后者的 `callee_saved` 与 `stack_align`/`frame_padding` 已在 `CallLayout` 里）。

**切换前必须先关掉的能力缺口**（2026-09-25 用 A3b-1 的核对测出来的真实现状，已钉成测试
`abi_target_real.rs::riscv64_float_gap_is_engine_ok_but_emission_closed`）：

| 机器 | 引擎（plan） | 现状发射 | 结论 |
| --- | --- | --- | --- |
| x86_64 / `win64` | 能算（RCX/XMM1/RAX…） | 能编，且与 plan 三项一致 | 可直接切 |
| riscv64 / `lp64d` | 能算（int 槽 X10 + 浮点槽 F10） | **fail-closed**：`v12 float args (MOVSD/MOVSS missing)`（谱里没有 `fpr_mov` 角色） | 先补浮点搬运角色/指令 |
| arm64 / `aapcs64` | 浮点/HFA 直接报缺池（没有 FPR 寄存器组） | 同样做不到 | A5 补 `[reg.fpr8]` + 角色 |

也就是：**寄存器参数的收参可以切**（A3b-2b-2a 已切），栈参数/`byval`/序尾声要等各自的能力
补齐（与 A1 静态体检的缺口清单同一批）。

- 管线按 `AbiPlan` 走：实参搬运（寄存器面已切）、返回值搬运、栈参数 store/load、sret 指针。
- 验收：**x86 的生成物逐字节不变**（`forge-codegen` 全套测试 + 三 ISA 矩阵）；
  再开 riscv64/arm64。
- 这一步会删掉"首 int 参数槽"那类硬编码（A3b-2b-2a 已删掉收参侧的那几处）。

### A4 序/尾声由生成器生成（伪指令**全部**删除）

**目标（用户 2026-09-25 的设计裁定）**：谱里只留**裸指令**（形状 + 编码 + 能力角色），
序/尾声（函数调用平衡：保存谁、帧多大、怎么建立帧指针）**完全由生成器**按"机器事实 +
角色"生成。`[emit.prologue]`/`[emit.epilogue]` 连同 `@push_callee`/`@pop_callee`/
`@frame_alloc`/`@frame_free` 一起删除；`[emit]` 只剩 `align_pad`/`epilogue_label` 这类
**机器事实**。角色是**能力声明**（这条指令能充当某件事），不是给指令加副作用。

**机器事实（谱里已有；A5 会整节搬进 `[machine]`/绑定）**：
`[abi.frame]` 的 `sp`/`fp`/`fp_push_bytes`/`alloc_neg`、`[abi].call_ret_reg`（**链接寄存器
已存在**，就是它）、`[abi.callee_saved].gpr`（静态列表；A5 改由绑定的 `cs_gpr` 给出）、
`[stack].slot`。

**新增三个角色**（同一指令可挂多个角色；缺角色 ⇒ 明确 `Unsupported`，不猜名字）：

| 角色 | 形状约定 | 用途 |
| --- | --- | --- |
| `frame_set` | `in` 槽 = 源、`out` 槽 = 目标，可带 imm | 建立/恢复帧指针（x86 `mov rbp, rsp` / `mov rsp, rbp`；riscv `addi x8, x2, frame`；arm64 `addimmx x29, sp, frame`） |
| `callee_save` | Reg 槽[0]=值、Reg 槽[1]=基址、Imm 槽[0]=偏移 | 保存寄存器到帧（riscv `SD`、arm64 `STURX`） |
| `callee_load` | 同上（load 方向） | 从帧恢复（`LD`/`LDURX`） |

复用已有角色：`push`/`pop`（push 机制的保存/恢复）、`frame_alloc`/`frame_free`（sp 调整，
符号由 `alloc_neg` 定）、`ret`。

**生成器的序列**（顺序 = 引擎决定；机制今天按"谱里有没有 `push`+`pop` 角色"判定，
A5 后由 `CallLayout::save_mechanism` 给）：

- **push 机制**（x86）序言：`push fp` → `frame_set(fp←sp)` → 逐个 `push` 静态 callee-saved
  → **收参** → `frame_alloc`（`frame_size != 0` 守卫）。
  尾声：`frame_set(sp←fp)` → `frame_alloc(imm = cs 字节)` → 逆序 `pop` → `pop fp` → `ret`。
- **store_to_frame**（riscv/arm64）序言：`frame_alloc` → `callee_save(link,[sp+frame-8])`
  → `callee_save(fp,[sp+frame-16])` → `frame_set(fp←sp+frame)` → 动态 `callee_save`
  （`[sp+frame-fp_push-(k+1)*slot]`）→ **收参**。
  尾声：逆序动态 `callee_load` → `callee_load(fp,-16)` → `callee_load(link,-8)`
  → `frame_free` → `ret`。

**验收**：三 ISA 的序/尾声**机器码逐字节不变**——先把当前 `emit_prologue`/`emit_epilogue`
（固定 `frame_size` + 固定 callee-saved 集合）的字节固化成 golden，再切生成器，golden 必须
不动；随后全套测试 + 两条矩阵。

**顺带要修的既有隐患**：x86 尾声的 `sub rsp, {callee_saved_bytes}` 用的是**静态**列表长度，
而 `@push_callee` 按 `callee_saved_to_save` **动态**保存——少保存时 rsp 落点会错位。
A4 让尾声与序言**同源**（都用动态计数），差异进测试。

**明确放弃的表达力**：谱再也无法给函数插入"任意序言步骤"（例如 vzeroupper、栈探测、
GOT 建立）。需要时**加角色**（上层能看见的能力），不回到自由模板——这正是"指令保持裸的、
调用平衡交给约定层"的代价与收益。

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
