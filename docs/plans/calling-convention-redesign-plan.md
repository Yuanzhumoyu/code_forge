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

### A3b-2b-2c 栈参数收参改读 call_layout

**先做的核对（2c-1 ✅，2026-09-25）**：`forge-codegen/tests/abi_target_real.rs` 新增
`stack_arg_offsets_agree_with_the_legacy_formula`——把布局的 `Stack { offset }` 与现有发射
路径的 `[abi.stack_args]` 算式逐参数对照。**当场抓到模型 bug**：`Stack.offset` 漏算
`shadow_bytes`（win64 第 5 个参数算成 16，现有算式是 48）。修法 = `engine::stack_place`
加上 shadow，并把"被调方视角、已含 shadow"与 `Stack.offset(k) − caller_offset(k) =
first_arg_offset` 写进注释与参考文档；`win64` 黄金快照整体 +32（其余三份 shadow = 0，
逐字节不变），`invariants.rs` 按新口径断言。

**剩下（2c-2 ✅，2026-09-25）**：`ArgPlace::Stack` 已接进 `@move_args` 的布局路径
（`__layout_ok` 接受 `Stack`；从 `[frame_base + offset]` 收进分配寄存器，偏移全部来自
forge-abi），带栈参数的函数不再整函数回退；被 regalloc 强制 spill 的栈参数仍走既有 spill
块（数值同一套，已由对照测试钉住）。**浮点走栈**与 `Pair`/`Group`/无指针 `Indirect`
仍 fail-closed / 整函数回退（`Pair`/`Group` 的边界写在守卫测试里）。

**剩余面**：`Pair`/`Group`、无指针的 `Indirect`（byval 指针本身在栈上）。

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

### A4 ✅ 序/尾声由生成器生成（伪指令**全部**删除）

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

**落地实测（2026-09-25）**：`FGE_DEBUG_GEN=1` 取样 10 份生成模块，**x86 / riscv64 / arm64
与夹具 demo_inst`*` 全部 SHA256 相同**（序/尾声的每条指令、字段顺序、立即数形态逐字一致：
x86 是 push 机制、riscv/arm64 是帧内机制 + 动态 `callee_saved_to_save`）；workspace 全套
serially 绿（含两条 JIT 矩阵）。写伪指令/模板的谱被 `validate` 拒绝，反回潮守卫
`tests/call_layout_emission.rs` 扫三谱 + 夹具的 TOML。

**顺带要修的既有隐患**：x86 尾声的 `sub rsp, {callee_saved_bytes}` 用的是**静态**列表长度，
而 `@push_callee` 按 `callee_saved_to_save` **动态**保存——少保存时 rsp 落点会错位。
A4 让尾声与序言**同源**（都用动态计数），差异进测试。

**明确放弃的表达力**：谱再也无法给函数插入"任意序言步骤"（例如 vzeroupper、栈探测、
GOT 建立）。需要时**加角色**（上层能看见的能力），不回到自由模板——这正是"指令保持裸的、
调用平衡交给约定层"的代价与收益。

### A5 删谱里的 `[abi]` 约定节，加 `[machine]`（进行中：arm64 浮点子集 ✅）

**已完成（A5-1，2026-09-25）**：arm64 浮点能力——`[reg.fpr8]`（V0..V31）+ `fpr` 操作数槽 +
`FMOVR` 形式 + `FMOV_S`/`FMOV_D`（`fpr_mov` 32/64）+ `LDURD`/`STURD`/`LDURS`/`STURS`（与
`LDURX`/`STURX` 同形的 SIMD&FP 访存）+ `[spill.FPR]`；绑定补 `float = V0-V7`、
`ret_float = V0-V3` ⇒ `abi check --strict` 缺口 **6 → 1**（只剩 HFA4 返回那条 A6 限制），
`abi plan` 给出 `f64 → V0` / `ret → V0`。守卫同步：指令总数 110、派生条目 360、歧义名单 +6
（V 命名统一 ⇒ S/D 汇编文本相同，已知且刻意）、aapcs64 黄金快照重 bless。

发射侧按类分派保存指令（FPR → STURD/LDURD）需要 role 能带**类限定**，归 A6。

**仍待做（A5-3：`[abi]` → `[machine]`，2026-09-25 侦察已做，按下面清单机械执行即可）**：
`[abi]` 里的**机器事实**搬进 `[machine]`，只留约定数据在绑定/规则里。侦察结论（这就是全部
触点，已逐个核对过调用点）：

| 机器事实 | 现状读者（要一并改） |
| --- | --- |
| `[abi].scratch`（→ `[machine].spill_scratch`） | `abi_view.rs:221`（静态视图 + notes）、`codegen/integration.rs:581-594`（`RegInfo::scratch_regs`）、`codegen/frame.rs:986`（栈参数收参 scratch，含错误消息）、`validate.rs:862`、`v12/tests.rs:920-933`（负向用例） |
| `[abi].reserved`（→ `[machine].fixed_regs`） | `abi_view.rs:215`、`integration.rs:582-584`、`validate.rs:863` |
| `[abi].call_ret_reg`（→ `[machine].link_reg`） | `abi_view.rs:228-235`、`codegen/lowering.rs:1470`（call 返回值槽）、`lowering.rs:1531`（call_indirect）、`frame.rs:713`（帧件保存 link）、`validate.rs:869`、`v12/tests.rs:937-952` |
| `[abi.frame].sp`/`fp`/`fp_push_bytes`/`alloc_neg`/`layout` | **已搬 `[machine.frame]`**（②b）：`abi_view.rs`、`integration.rs`（sp/fp 解析）、`frame.rs`（layout / fp_push_bytes / spill 缺省 base / 栈参数基址）、`lowering.rs`（`frame_base_toks`/`sp_base_toks`）、`validate.rs` 全部改走 `machine_frame()` |
| `[abi].arg_class.regs`、`ret_regs`、`arg_slot`、`stack_args`、`callee_saved`、`call_clobbers`、`frame_padding` | **约定事实**：改由 `AbiRules`/`AbiBinding` 给（`int/float/vector` 池、`ret_int/ret_float`、`position`、`stack.*`、`cs_gpr/cs_fpr`、`frame_padding`）；谱里删掉后，管线侧 `pipeline/compiler.rs`（`int_arg_slot_count`/`max_stack_arg_bytes`/`sret`/clobbers）与发射侧 `frame.rs` 的 `[abi]` 回退路径要改读 plan（`AllocResult::call_layout`），demo 夹具（无绑定）得在测试里注册自己的规则/绑定 |

**已落地（A5-3 ①+②，2026-09-26）**：`[machine]` 三个键（`fixed_regs` / `spill_scratch` /
`link_reg`）进场，**三份发行谱全部迁完**（x86/riscv64/arm64 的 `[abi].scratch`/`reserved`/
`call_ret_reg` 已删，值原样进 `[machine]`）。读取侧一律经
`V12Model::machine_scratch()/machine_reserved()/machine_link_reg()`——`[machine]` 优先、
回退 `[abi]` 同名旧键，所以**迁移可以逐谱进行、不必一次切完**；`validate` 两处都校验
（登记名须在 `[reg.*]` 里），`abi_view` 的静态视图与生成期同源。三条 fail-closed 错误消息
改点名新键（`[machine].spill_scratch` / `[machine].link_reg`），两条负向用例同步。
三方守卫（model ↔ JSON Schema ↔ 文档键表）同批更新并重新生成 `isa-dsl.schema.json`。
验收：workspace 全套 serially 绿（clippy `-D warnings` 0）、两条 JIT 矩阵与迁移前**逐条同值**
（x86 195 passed / 3 skipped、riscv64 131 passed / 67 skipped，0 failed）。
**已落地（A5-3 ②b，2026-09-26）**：`[machine.frame]` 帧形状（`sp`/`fp`/`layout`/
`fp_push_bytes`/`alloc_neg`）进场，读侧统一 `V12Model::machine_frame()`（新表优先、回退
`[abi.frame]`）；`isa/{x86,riscv64,arm64}_v12.toml` 与两份 demo 夹具、两条 DSL 内联反向用例
全部迁到新键（夹具因此真的走新路径）。`schema`/文档键表/`isa-dsl.schema.json` 三方同改。
验收：workspace 130 个测试二进制 serially 全绿、clippy 0、CI run 213 十项绿（唯一红 =
慢性 `forge-rustc (e2e, Windows)`）。

仍待做：③ 约定事实（`arg_class`/`ret_regs`/`arg_slot`/`stack_args`/`callee_saved`/
`call_clobbers`/`frame_padding`）移入 `AbiRules`/`AbiBinding` + demo 夹具补测试本地绑定，
④ 发射/管线改读 plan，然后删掉 `[abi]` 的约定键与机器事实回退路径。

**逐键迁移进度（2026-09-27 实测记录，每键独立提交 + 门禁）**：

| 键 | 处置 | 结果 |
| --- | --- | --- |
| `[abi].callee_saved` | 删；机器事实 `[machine].callee_saved_gpr` + `callee_save_slots` 接管（计数与**名字**必须同源，否则半迁移的树会崩——见 CHANGELOG 更正） | ✅ 键已删，三谱绿 |
| `[abi].call_clobbers` | 删（riscv64/arm64；x86 本就没写）；破坏集全面由 plan 的 `clobbers` 给 | ✅ 键已删，三谱绿 |
| `[abi].frame_padding` | 改读 `[machine].frame_padding`（机器事实）；约定侧 `AbiRules::frame_padding` 进 plan，管线优先 | ✅ 键已迁走 |
| `[abi.stack_args]` | 删；调用点改读 plan（`CallLayout::caller_offset(k)` + `shadow_bytes`/`slot_bytes`，基址恒 `[machine.frame].sp`），被调方 spill 收参改读 `ArgPlace::Stack { offset }`；能力申报改由 `roles = ["stack_arg_load"/"stack_arg_store"]` 说话（有 load 缺 store 仍生成期报错；两者皆无 = 不支持栈参数）；规则侧补 `shadow_bytes` 必须是槽单位倍数的校验 | ✅ 键已删 |
| `[abi].ret_regs` | 删；`Return` 读 `CallLayout.ret`（否则宿主的 `conv_ret_gpr`）、`Call` 读 `conv_ret_gpr`（空参合成签名问引擎），两者皆无 ⇒ fail-closed；`TargetABI::ret_regs()` 收敛为空表缺省（无消费者）；守卫 `convention_level_int_return_slot` 钉住 RAX/X10/X0 | ✅ 键已删 |
| `[abi].arg_class` | 消费者逐个摘：① **被调方 spill 收参** ✅ 已改按 `ArgPlace::Reg`（fail-closed）；② 剩下**调用方**的参数搬运（`gen_call_lowering`/`arg_move_loop` 改成按 `plan_call` 的 `ArgPlace` 逐实参发射：`Reg`/`Indirect{reg}`/`Stack` 直接发，`Pair`/`Group`/无指针 `Indirect` fail-closed）、③ `gen_abi` 的 `arg_regs()`/`int_arg_slot_count` 与宿主 `param_reg_count` 改由 plan/绑定给 | ⏳（① ✅） |
| `[abi].call_clobbers` | 删；破坏集是**签名无关**的约定级事实 ⇒ 管线在无 plan 时用空签名问一次引擎，塞进 `LowerCtx::conv_clobbers`，生成物"两者皆无 ⇒ fail-closed"；守卫 ③b 钉住"空签名 plan == 真实函数 plan 的 clobbers" | ✅ 键已删 |
| `[abi].arg_slot` | 删；改读机器事实 `[machine].arg_slot`（`V12Model::machine_arg_slot()` / 生成物 `TargetABI::arg_placement()`）；约定侧新增 `AbiPlan::position`（进黄金快照），守卫 ②b 钉住"机器事实 == 主约定的 plan.position" | ✅ 键已迁走 |
| `[abi].scratch` / `reserved` / `call_ret_reg` / `frame` / `callee_saved`（**迁移期回退键**） | 删；机器事实只留 `[machine]`（夹具与内联谱全部迁完，`CalleeSaved` 结构体与 schema 两节一并删） | ✅ 键已删 |
| demo 夹具（`tests/isa/*.toml`） | ✅ **已补测试本地约定**：`tests/common/mod.rs::ensure_demo_conventions()`（两份规则 + 绑定，`aliases = ["c"]`）⇒ 夹具函数真的有 plan（两条守卫钉住 `call_layout` 存在与落点） | ✅ |

步骤建议（每步都可独立跑门禁）：
① 加 `[machine]`（model + schema + `docs/reference/isa-dsl.md` 键表三处同改，`schema_guard` 钉住）；✅
② 上表"机器事实"逐个改读 `[machine]`（含错误消息与负向用例）；x86 先迁，跑门禁；✅（三谱同批迁完；`[machine.frame]` 见 ②b）
③ riscv/arm64 迁移 + demo 夹具补测试本地绑定 ⇒ 删除 `[abi]` 的约定键；
④ 发射/管线改读 plan（这一步才动 `move_args` 的 `[abi]` 回退路径，可用"生成物逐字节不变"
   与三条 ISA 矩阵验收）。

- 新增 `[machine] { fixed_regs, spill_scratch, link_reg }`（只留**机器事实**）；
  参数池/sret/callee-saved/栈参数布局全部移到绑定与规则里。
- 三份发行谱 + 四份绑定同步迁移；arm64 补 `[reg.fpr8]`（`V0..V31`）与浮点搬运角色
  ⇒ `abi check` 的 6 条缺口清零。
- 验收：`forge-isa validate` 三谱零诊断、`abi check --strict` 三谱零缺口、
  矩阵 skip 数下降。

### A6 变参 / HFA 部分在寄存器 / ≥3 槽返回 / 罕见的弹栈约定

**A5-3 ④ 带出来的前置缺口**（2026-09-26 实测，都已在代码里注明并留了守卫）：

- ~~**向量值跨调用存活会读回错值**~~ **已修**（2026-09-26）：把 plan 的完整破坏集
  （x86：全部 16 个 XMM）交给发射后暴露的不是破坏集的问题，而是**单结果 IR 值按池宽
  成类**——V128 也拿 `FPR(8)`，而 `XReg::width()` 就是类宽，spill/reload 只搬 8 字节
  （`test_jit_v128_byval_return_lane3` 返回 0）。修在 `pipeline/lowering.rs`：向量按真实
  字节数成类（`VEC(16/32/64)`）。此后**全类破坏集可直接用**（`lowering.rs` 不再按类过滤），
  守卫 `x86_plan_clobbers_cover_the_whole_fp_file` 钉住"plan 的 FP 破坏集覆盖全部 XMM"。
- **callee-saved 的类分派保存**（2026-09-26：**已落地并接上**）：角色声明能带寄存器类限定
  （`{ role = "callee_save", class = "fpr" }`），arm64 的 `STURD`/`LDURD` 按此申报，
  `callee_saved_loop` 在生成物里按 `__preg.class.is_fp()` 分派；regalloc 侧
  `RegAllocConfig::callee_saved_fpr` 单独承载 FPR 集（`X8`/`V8` 同号，不能与 GPR 集合并），
  `callee_saved_to_save` 也扫主 FPR 类表，fp-inside 的 `min_frame` 按 **plan** 的整张表算
  （AAPCS64 = 10 GPR + 8 FPR = 18 条 ⇒ 16 + 18×8 = 160）。守卫
  `arm64_fp_inside_frame_counts_the_plan_callee_saved_table` 钉住这三件事。
  **仍未覆盖**：如果某个 ISA 的 FPR 保存槽宽度不是"一个槽单位"（如 16 字节的 Q 寄存器），
  偏移公式 `(k+1)*__SLOT_BYTES` 需要按类给宽度——今天的 arm64 存 D（8 字节）正好等于槽宽。
- **帧上方 callee-saved 字节数**（fp-outside 一路）：**已解决（2026-09-27，`531b9e2`）**。
  x86 的序言/尾声现在按 `alloc_result.callee_saved_to_save` **运行时**发射（逐个 `push`；
  尾声 `sp -= n*槽` 取同一列表的运行时长度并逆序 `pop`），列表来源是约定数据（有 plan 用
  plan：显式选 sysv64 就只有 5 个；无 plan 才退谱面表）。
  **绕开第二层耦合的办法（这是关键设计，别忘）**：帧内所有偏移（`sp_base`、栈槽平移、
  栈参数区）都是**编译期常量**、按谱面表长算的，少推 Δ 个槽会让 rsp 抬高 Δ×槽而整体错位。
  不要去把这些常量改成运行时值（那会牵动 `emit_spill_load/store` 的签名），而是让**管线把
  Δ×槽补进序言实际分配的字节数**（`pipeline/emission.rs` 的 `cs_skipped`，仅 fp-outside）
  ⇒ rsp 落点与"静态表全长"时代**逐字节相同**，所有编译期常量与栈对齐（`frame_padding`）
  继续成立。fp-inside 的保存是帧内槽、不动 rsp ⇒ `cs_skipped = 0`。
  **中途踩过的两个坑（留档）**：① 只改发射、不动帧字节数 ⇒ 跨调用用例读垃圾值 +
  `0xC0000005`；② 只补 `frame_padding`（`padding = (rule + Δ×槽) mod align`）救不了——
  因为 `__cs_bytes`/`sp_base` 是**寻址基点**而非仅尺寸，必须靠"帧补齐"让 rsp 原地不动。
  遗留：护栏 `x86_static_push_table_covers_the_plan_gpr_callee_saved` 的立论（"静态 push
  表"）已过时，应换成钉住 `cs_skipped` 的守卫；`[abi].callee_saved` 现在只剩"无 plan 时的
  兜底"作用，可以连同 A5-3 ③ 一起删——**但删之前必须先做下面这件事**。

- **删 `[abi].callee_saved` 的前置条件（2026-09-27 实测得出，别照直觉直接删）**：该键还有
  **一个生成期消费者**——`v12/codegen/frame.rs` 的 `gen_arg_receive` 用它算生成物里的
  编译期常量 `__cs_bytes = fp_push + 谱面 gpr 数 × 槽`（x86 = 8 + 7×8 = **64**；
  riscv = 16 + 11×8 = **104**，正是 `min_frame` 那个数），它参与
  `sp_base = -(frame) - __cs_bytes + stack_arg_bytes`（收**溢出到栈的栈参数**时的地址基点）。
  **实测**：只删 x86 的 `[abi.callee_saved]` ⇒ `__cs_bytes` 变 8（少 56）⇒
  `forge-codegen --lib --all-features` 直接 `0xC0000005`（栈参数 spill 基点偏了）；已回滚。
  **正确拆法**：它是**机器事实**而非约定事实——"这台机器的帧为 callee-saved 预留几个推入
  槽"与"哪些寄存器必须由被调方保住"（约定，在 plan/绑定里）是两件事：
  ① `[machine]` 增 `callee_save_slots`（x86 = 7、riscv = 11、arm64 = 10；与
  `[abi.callee_saved]` 同义但换名，避免"两处都写谁生效"）；
  ② 生成期 `__cs_bytes`、`frame_layout_info` 的无 plan 回退、`emission.rs` 的 `cs_skipped`
  **三处都改读它**（`cs_skipped` 要用机器槽数而不是 plan 数，才不会把"约定比机器少几个"
  也算成"没推"）；
  ③ 然后才能删三份谱的 `[abi].callee_saved`（regalloc 的集合仍来自 plan；无 plan 的夹具
  本来就只有空表）。

  **2026-09-27 二次实测（这次定位到具体测试）**：机器事实链路接上后再删 x86 的
  `[abi].callee_saved`，`forge-codegen --lib --all-features` 仍 `0xC0000005`，崩在
  `runtime::jit::tests::test_jit_call_indirect_wide_vector_byref`（逐个用例串行跑，前一条
  `test_jit_boundary_constants` 还是 ok）。**关键**：`compiler.rs` 的 plan 计算是**允许失败**的
  （"算不出来不阻断编译，退回谱里声明的约定数据"，结果留在 `abi_plan_note`），于是**宽向量 by-ref /
  call_indirect 这类引擎暂时规划不了的签名**会走 `call_layout == None` 的回退路径——那条路的
  regalloc callee-saved 集合就是 `ri.callee_saved()`。删掉谱面键 ⇒ 该集合为空 ⇒ 被调方
  什么都不保存、而调用方以为 RBX/RDI/… 被保住 ⇒ 破坏调用者状态（错值，极端即访问违例）。
  **结论**：`[abi].callee_saved` 不能只靠"发射侧不再读它"来删——必须先让**每条发射路径都有 plan**（= 补 A6 里那些规划缺口：Pair/Group/无指针 Indirect/宽向量 by-r
  ef 等），或让无 plan 的回退
  不再需要**寄存器名**（例如保守地"除 scratch 外全部可用 GPR 都算 callee-saved"）。这条与 A6 的
  **⚠ 2026-09-27 更正（重要）**：上面的"真正原因"**未被证实**。补齐 trace 后实测：① 在
  `forge-codegen --lib` 全量跑里（键仍在）**没有任何一次规划失败**（`FORGE_TRACE_ABI=1` 一条 `[abi-plan]`
  都没打）——即"无 plan 的回退"在那套测试里根本没被走到；② 把键删掉后，单独跑那个"崩掉的"用例
  `test_jit_call_indirect_wide_vector_byref` **是通过的**（1343 filtered / 1 passed）。所以那次 `0xC0000005` 是
  **整套串行跑时的顺序/状态相关崩溃**，触发点尚未定位；`[machine].callee_saved_gpr` 那次修复确实让整套转绿，但
  它与崩溃之间的**因果链没有被证明**。
  **对裁定的影响**：用户选的 fail-closed 路线（见下）建立在"缺口导致回退、回退需要名字"这个解释上；在
  把因果查清之前，**不要**据此大改引擎（补 Pair/Group/… 规划缺口）。正确的下一步是：先用删键 +
  整套跑（保留 `FORGE_TRACE_ABI`/`FORGE_JIT_EVENTS`）把那次崩溃**复现并定位到具体机制**，再决定 fail-closed 还是别的路线。

  **2026-09-27 最终更正（这次查到底了）**：所谓"删键就崩"其实是**迁移中途状态**造成的，与"无 plan
  回退"无关：① 键在 `3361236` 里**已经删掉**了，所以本轮再"删一次"是**空操作**，整套自然绿
  （1344 passed，已实测两次）；② 全程 `FORGE_TRACE_ABI=1` 下**没有任何一次规划失败**（`[abi-plan]` 一条不打印）
  ⇒"无 plan 回退"在这套测试里根本没被走到；③ 早先那两次 `0xC0000005` 发生在**只切了一半**的树上：
  `callee_save_slots`（计数）与 `__cs_bytes`/帧布局已经改读机器事实（7 槽 / 64 字节），而**名字**
  （`TargetRegInfo::callee_saved()`）还是空的 ⇒ 帧按 7 槽布置、regalloc 却认为没有寄存器需要保
  ⇒ 内部不自洽。补上 `[machine].callee_saved_gpr`（名字）之后计数与名字**同源**，一切自洽。
  **结论**：`[abi].callee_saved` → `[machine]` 的迁移**已完成**；六份约定键的迁移**不需要**走
  fail-closed 那条弯路（那条裁定建立在一个错误解释上）——只要求每步都保证"计数 / 名字 / 消费者三者同源"。
  （= 补 A6 里那些规划缺口：Pair/Group/无指针 Indirect/宽向量 by-ref 等），或让无 plan 的回退
  不再需要**寄存器名**（例如保守地"除 scratch 外全部可用 GPR 都算 callee-saved"）。这条与 A6 的
  规划缺口是同一件事，别分开做。

  **2026-09-27 设计结论（下一步照此实现）**：无 plan 的回退**必须有寄存器名**，而"名字"不能凭空
  发明，所以把"帧可能保存的那组 GPR"也升格为机器事实，与计数一起放在 `[machine]`：
  ① `[machine].callee_saved_gpr = ["RBX", "RDI", …]`（x86 用 win64∪sysv64 = 现有 7 个；riscv = 11 个 s 系；
  arm64 = X19-X28）——语义是"这台机器的帧件会保存这组寄存器"，与"某份约定**要求**保住哪些"
  （绑定）分开；② 生成的 `TargetRegInfo::callee_saved()` 改读它（`[abi].callee_saved` 才可删）；
  ③ **加一条 validate/守卫：plan 的 callee-saved ⊆ 机器那组**（否则"约定要求保 R15、机器帧不存它"这种
  静默破坏必须编译期报错）；④ `callee_save_slots` 若与名单并存，取名单长度（计数可由名单派生，
  避免"两个都写、谁生效"）。
  另一条可选路线是把"无 plan + 需要 callee-saved"直接 **fail-closed**（明确报错而不是猜），

  **★ 裁定（用户，2026-09-27）：走 fail-closed 这条路线**——先把 A6 的规划缺口补齐（让**每条发射
  路径都能规划**），再把"无 plan + 需要约定数据"改成**明确报错**，然后才删六份 `[abi]` 约定键
  （`arg_class`/`ret_regs`/`arg_slot`/`stack_args`/`call_clobbers`/`frame_padding`）并给 demo 夹具补测试本地绑定。
  **执行顺序（一批做完）**：① `AbiRules`/引擎补规划缺口——Pair（`RegPair`）/ `RegGroup` / 无指针的 `Indirect`
  （`Placement::Indirect{ptr:None}`）/ 宽向量 by-ref（`Indirect{on_stack}`）在代表性签名上都能出 plan（`forge-abi` 黄金 +
  `abi_target_real.rs` 的交叉核对同步）；② `compiler.rs` 的 `None` 分支改 fail-closed（报错点名缺口 + 提示看
  `abi_plan_note`），随之修掉靠回退跑通的用例（`test_jit_call_indirect_wide_vector_byref` 等）；
  ③ 六份键逐个迁移（约定数据进 `AbiRules`/`AbiBinding`，管线/发射改读 plan）、谱里删键；④ demo 夹具在测试内注册本地
  绑定；⑤ 全量门禁 + 三条矩阵 + 中文提交 + 推送 + 抓 CI。
  但那会让今天靠回退跑通的 wide-vector-byref 用例变红，须先补 A6 的规划缺口——优先级低于上面①–④。

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
