# 后端 v23 里程碑方案：数据驱动的通用后端

> **状态：[progress]**（2026-10-11 立项；M1–M6 见 §5，逐项回写进度、实测数字与命令）。
> 命名说明：本方案的 "v23" 是本里程碑自己的编号，与 ISA-DSL **语法版本 v22**
> （不写进 TOML、只靠文档/CHANGELOG 追踪）不是同一套编号——**不要把 v23 写进任何
> `isa/*.toml`**。证据行号取自立项时 HEAD `d5d66cdb`，随迭代漂移，**以符号名为准**。

## 1. 问题（全部有代码证据）

### 1.1 通用性：通用管线认识单台机器

| # | 事实 | 证据 |
| --- | --- | --- |
| G1 | 编译入口直接调宿主能力函数、写死宽向量档 | `compiler.rs:1942`、`:1990` 调 `crate::avx512_available()`；`:1996` 写死 `bytes != 32 && bytes != 64`；`:1935`、`:2310` 写死 `> 16` |
| G2 | 「聚合 > 8 字节」有第二事实源，且**早于** ABI 引擎被使用（引擎还没被问就分流了） | `compiler.rs:222/389/438/457/698/800/1063/1404`、`agg_expand.rs:31/45/64/79/91` 写死 `> 8`；段切分 `div_ceil(8)` 见 `compiler.rs:240`、`agg_expand.rs:217`；阈值本属引擎数据 `forge-abi/src/rules.rs:33-35`（`size_gt`/`size_le`/`IndirectVia`） |
| G3 | `agg_arg_place`/`agg_ret_is_sret` 是对引擎的**侧信道 RPC**：伪造 `ArgShape{members: Vec::new()}`、取全局锁、`format!` 造名字，只为读手里已有的 plan；`members` 为空 ⇒ HFA 永远分类不出 | `compiler.rs:75-151` |
| G4 | 能力钩子无人实现：`TargetPeephole`/`TargetSimulator` 零实现者、零谱路径，管线却留着调用点 | `machine/target.rs:61/109`、`machine/peephole.rs`、`machine/simulator.rs`、`pipeline/lowering.rs:793`、`compiler.rs:2173` |
| G5 | 栈帧保存集两套口径：fp-outside 按**谱面静态 push 表**算帧、fp-inside 按 **plan** 算（v20 A6 未收的口子） | `frame_layout.rs:44-58`（注释自陈「要一起换，归 A6」） |

### 1.2 冗余：同一事实两处表示

| # | 事实 | 证据 |
| --- | --- | --- |
| R1 | `AbiPlan`（forge-abi）↔ `CallLayout`（runtime）两套镜像 + 手写翻译；`VaArgRule` 12 字段抄两遍 | `machine/call_layout.rs:43-124/166-191` ↔ `forge-abi/src/plan.rs:61-138/303-333`；`abi_target.rs:275-583`、`:565-583` |
| R2 | 两份 IR 类型 → 形状投影，退化策略不一致（长数组一边折成 1 个 int、一边折成 0 个成员） | `sig_view.rs:45-96` ↔ `machine/call_layout.rs:384-439`；`TyKind::Vector.elem_bytes` 要求传入后被丢弃（`abi_target.rs:186-201`） |
| R3 | 分配后 class 回写两份**语义不同**的实现，一份绕过 ISA 的 `[types]` 映射 | `compiler.rs:2519`（走 `ctx.reg_class_for`）↔ `emission.rs:307-311`（走 `RegClass::from_type_id`） |
| R4 | 一条指令的操作数被描述两遍：trait 一套 + `xreg_map`/`inst_clobbers` 平行旁表一套；`use_constraints`/`def_constraints` 全仓**零消费者** | `machine/inst.rs:82-192`；`compiler.rs:2233/2236`；`lowering.rs:632/637/674/676` 四处必须同步 `extend` |
| R5 | 宿主面编译入口 5 条且语义不一致：`compile` 与 `compile_raw` 实现同一句调用（后者「绕过优化」的承诺不成立）；`JitCompiler::add_function` 不装模块签名表 ⇒ 变参调用点按非变参发 | `compiler.rs:1847/1889/1896`、`jit.rs:143/183`、`target.rs:121-129`、`object_writer.rs:136` |
| R6 | 合法化散成 8 个同构收集器 + 5 份段打包复制 + 4 处内联搬移（已有函数 `move_to_front` 没被调用） | `compiler.rs:200/380/423/669/774/1048/1434/1615`；段打包复制 `compiler.rs:487-492/845-849/1682-1693/1743-1745`；`agg_expand.rs:690` |
| R7 | 死机制/死 API：`AllocContext{}` 空结构；`TargetEncoder::encoded_size`/`encode_into` 无消费者；`RegAllocConfig::new` 写死 `GPR64/FPR64/width 8` | `alloc_config.rs:58/95`、`regalloc_bt.rs:54`、`encoder.rs:36-63` |

### 1.3 正确性与编译质量

| # | 事实 | 证据 |
| --- | --- | --- |
| C1 | **调用方与被调方对同一类型的分类会分歧**（AAPCS64 `{f32,f32,f32}` HFA：调用方探针给 `Registers`、被调方给 float 池）⇒ 静默错寄存器 | 调用方 `compiler.rs:75-90`（members 为空）↔ 被调方 `sig_view.rs:59-64` + `forge-abi/src/builtin.rs:134/142` |
| C2 | 7 个「是否需要聚合展开」判据**已经互相矛盾**（`has_agg_mem_access` 的 Load 支少了 `size_bytes > 8`）⇒ 漏判即**静默跳过** Stage 0.5，不是报错 | `agg_expand.rs:14-150`、`:103-111`（clone 门） |
| C3 | 寄存器池过滤**类无关**：拿 GPR 的 sp/fp 编号过滤 FPR/VEC 池 ⇒ x86 少 `XMM4/XMM5`、riscv64 少 `f2/f8`、arm64 少 `f29/f31`（三个发行 ISA 各丢 2 个寄存器） | `regalloc_bt.rs:150-155` |
| C4 | callee-saved 只认两个族且 `pop_free` 类无关 ⇒ **FPR 的跨调用保护实际不成立**（AAPCS64 真正要保 v8-v15，却按编号偏好 v19-v28） | `regalloc_bt.rs:862-896`、`:813-816`；`frame_layout.rs:157-160` 已断言这个分裂 |
| C5 | 文档与实现不符：①「Belady's MIN」未实现——只保留 `next_use == MAX`（死值）再按 vreg 编号取最大；② 约束机制（`Fixed`/`ReuseInput`/`Stack`/`Clobber`）不可达，所有调用点传 `Any`；③ `precolored` 建了从不读 | `regalloc_bt.rs:4/503-505` 对比 `:404-456`、`:90/275/293-313`、`compiler.rs:2394` |
| C6 | 帧记账四处累加、`frame_layout_info` 被重算 5 次 | `frame_layout.rs:100-113`；`compiler.rs:100/2047/2299`、`frame_layout.rs:118`、`emission.rs:38`；`frame_slots.rs:2-10` 记着上次漂移 |
| C7 | 发射缝合线：`emit_inst_with_spills` 13 个参数；`emit_spill_load/store` 把寄存器类压成 `is_fp: bool`（挡住 VEC/KReg 分派）；spill 基点公式二次推导 | `emission.rs:212-226/239-253/280`、`machine/frame.rs:55-62` |

### 1.4 必须保留（重构中不许打回）

- `check_value_pools` 的 fail-closed 值池门（`compiler.rs:1854`）；
- `Alloca(Type(I8), Uint(n))` 规范 immediate 形态（`frame_slots.rs` 只认它）；
- `trace_enabled` 的 `OnceLock` 缓存（`pipeline/mod.rs:28-52`，实测省 7–18%）；
- relocation 以 `RelocKind` 宽度为 ISA 数据、无白名单（`emit.rs:218-259`）；
- CFI 按 ISA 名注册（`cfi.rs:214`）；
- `frame_slots::scan_frontend_slots` 单一实现（`frame_slots.rs:43`）——M6 照这个模式做；
- 三类 fail-closed 文案（`conv_registry.rs:80-102`、`abi_setup.rs:66-79`、`call_plan.rs:101-108`）。

## 2. 成功判据（客观可验）

- **S1** `rg -n 'avx(2|512)?_available' crates/backend/forge-codegen/src/pipeline` → 0；
  `rg -n '> 8|> 16' crates/backend/forge-codegen/src/pipeline` → 0（注释与守卫白名单除外）。
- **S2** 聚合间接判据唯一：全部经 `AbiPlan` 查询；`rg 'size_bytes\(.*\) >' pipeline` → 0。
- **S3** `BacktrackingAllocator::allocate` 只接受 `&VCode<I>` + `&RegAllocConfig`；
  `CompileState` 无 `xreg_map`/`inst_clobbers`/`xreg_types`。
- **S4** 编译实现唯一：`CompileSession` 是唯一做编译的类型（`FunctionCompiler`/`compile_raw`
  全仓 0 命中）；`JitLoader::add_function`/`compile_module` 只是**薄委托**（每处 ≤ 5 行，
  自身不含管线调用）；`target.rs` 的擦除入口只查注册表。
- **S5** `rg 'TargetPeephole|TargetSimulator|AllocContext|CallLayout|ArgPlace'` → 仅注释/守卫白名单。
- **S6** 门禁全绿：`cargo test --workspace --exclude forge-rustc`；
  `cargo test -p forge-tests --lib jit_matrix_x86 -- --nocapture`；
  `cargo test -p forge-tests --lib jit_matrix_riscv64 -- --test-threads=1 --nocapture`
  （QEMU 缺通道如实记 skip，不假绿）；`asm_parse`/`asm_encoding`/`asm_exec` +
  `asm/ratchet/<isa>.txt` 逐项相等；`cargo clippy --workspace --all-targets -- -D warnings`；
  `cargo fmt --all -- --check`；改动文档 `npx markdownlint-cli2 <文件>` 0 error。
- **S7** 通用性裁判（新用例）：`tests/isa/demo8.toml`（1 字节寄存器）与
  `tests/isa/demo_inst100.toml`（100 位指令字）各编一条带聚合传参的函数；
  一条合成约定（阈值 ≠ 8、保存集含第三族）能出 plan 并编过。
- **S8** 净减：`crates/backend/forge-codegen/src/pipeline/` 生产代码 −1800 行量级
  （立项时约 6800 行），无新增 `#[allow]`。
- **S9** 不劣化：`cargo bench --bench compile_bench -- --save-baseline before` /
  `--baseline before` 的 `comparison|codegen` 组劣化 ≤ 5%；`CF_CODEGEN_TIMING` 的
  regalloc 占比写进 `docs/performance/bench_baseline.md`；C3/C4 修好后「需要驱逐活跃值」
  的合成用例从 fail-closed 变为可编且结果正确。

## 3. 目标与非目标

### 目标

- 通用管线里**不再出现任何一台机器的专属事实**（能力由谱/约定申报，阈值只从引擎来）；
- 一条机器指令的操作数、一份 ABI 数据、一处帧记账、一条编译入口——**同一事实只有一处表示**；
- 宿主面的"学一个类型就能用"：`CompileSession` 覆盖 JIT / 对象文件 / 类型擦除三条出口；
- 分配器说明的行为与它真正做的事一致（池过滤、callee-saved、驱逐策略、spill 编辑）；
- 允许破坏性更新：无兼容层、无迁移工具、无 deprecated 别名。

### 非目标

Windows SEH / ARM EH codegen、statepoint、`SjLj`；优化 pass（`forge-opt`）；
前端与文法；新 ISA 接入；性能专项（只保留 S9 的「不劣化」约束）。

## 4. 数据流目标形态

```text
forge-ir ──► CompileSession ──► legalize（单一驱动，IR→IR）
                                   │
                                   ▼
                             lowering（InstPacket：只有 insts/inputs/outputs/xregs）
                                   │  每条指令自描述操作数（InstOperand：reg/field/class/constraint/settable）
                                   ▼
                        regalloc（输入：VCode + RegAllocConfig；输出：RegAllocOutput + SpillEdit）
                                   │
                                   ▼
                     MachBuffer + encoder（标签/分支松弛/常量池/重定位记录）
                                   │
                                   ▼
                   CompiledFunction ──► JitLoader / ObjectWriter / ErasedTargetMachine
```

ABI 侧只有一层数据：`forge-abi-model`（POD：`AbiPlan`/`Placement`/`RetLoc`/`ArgShape`/`TyView`/
`Signature`/`VaArgRule`），被 `forge-abi`（规则 + 引擎 + TOML）与 `forge-isa-runtime`（生成物运行面）
同时依赖；不再有 `CallLayout` 镜像。

## 5. 里程碑

### M1 编译入口收敛：`CompileSession`（人体工学；先做）

#### 目标

宿主只学一个类型；三条出口共用同一份编译产物与同一张模块签名表。

#### 改动

- 新增 `pipeline/session.rs`：`CompileSession<M: TargetMachine>`，`for_module(machine, &Module)`
  或 `new(machine)`；链式 `with_opt_level` / `with_parallel` / `with_alloc_config`；
  `compile_module(&Module) -> Result<CompiledModule, IrError>`；`add_function(name, sig, build_fn)`
  走**同一条内部路径** `compile_one(func, sigs)`（因此 `add_function` 也带模块签名表，修变参洞）。
- `CompiledModule { machine, functions: Vec<CompiledFunction>, symbols, data_segments }`：
  `function(&self, name)`、`iter_functions()`、`jit()/into_jit()`。
- 模块签名表类型化：`ModuleSigs(Vec<FuncSigEntry>)`（`variadic`/`fixed_args`/`param_count`），
  替代满仓的 `Vec<(bool, u32)>`（`compiler.rs:1818`、`abi_setup.rs:36-41`、`jit.rs:212`）。
- 删 `FunctionCompiler` 公开面（并入 session，保留内部实现）；`compile`/`compile_raw` 二者取一
  （优化级别用一个 `OptLevel` 表达）。
- `JitCompiler` → `JitLoader`（只负责可执行内存、符号解析、重定位应用、`get_fn`）；
  `jit.rs:213-269` 的并行/串行两条复制路径合成一条循环 + `parallel` 选项。
- `ErasedTargetMachine::compile(func)` → `compile_module(&Module)`（`target.rs:121-129`）。

#### 公开面变化（破坏性）

删 `FunctionCompiler`、`compile_raw`、`with_module_sigs(Vec<(bool,u32)>)`、
`JitCompiler::add_function`；`CHANGELOG` 点名替代入口。

#### 边界

`add_function` 单函数路径没有模块级名表 ⇒ 只装「本函数 + 已知 external」的签名，
跨函数变参调用仍按外部符号处理（文档写清）；并行选项默认关闭（保持「<8 函数串行」的实测结论）。

#### 验收

`tests/session_api.rs`（同一 Module 走 session / `JitLoader` / `ObjectWriter` 三条出口，
函数码字节相同）；变参 `add_function` 用例（当前会红）；JIT 两矩阵不变；S4。

### M2 单一 ABI 数据模型（去镜像 + 单次分类）

#### 目标

一份 ABI 数据、一次分类、两侧同源。

#### 改动

- 新增 leaf crate `crates/foundation/forge-abi-model`（零内部依赖）：`AbiPlan`、`Placement`、
  `RetLoc`、`ArgShape`、`TyView`、`Signature`、`DeclAttrs`、`VaArgRule`（**带** `VaField.name`）、
  `VaField`。`forge-abi` 依赖它并 `pub use`（保持 `forge_abi::AbiPlan` 路径）；
  `forge-isa-runtime` 依赖它。
- 删 `forge-isa-runtime/src/machine/call_layout.rs` 的整套镜像与 `abi_target.rs:275-583` 的翻译；
  `AllocResult.call_layout: Option<AbiPlan>`；生成物 `move_args` 读 POD。
- 单一 IR→形状投影：`fn type_to_shape(store, ty) -> TyView`（唯一实现，含深度/长数组策略），
  删 `sig_view.rs:45-96` 与 `call_layout.rs:384-439` 两份；删 `ShapeKind::Vector.elem_bytes`。
- 单次分类：`AbiPlan` 增 `arg_kind(&self, i) -> ArgKind` / `ret_kind(&self) -> RetKind`；
  删 `agg_arg_place`/`agg_ret_is_sret`（`compiler.rs:75-151`）。
- 别名口径统一：`LowerCtx.conv.name`（原始注册键）与 `CallLayout.conv`（别名解析后）合成一个字段。

#### 边界

`forge-abi` 的四份 `.plan.txt` 黄金除 HFA 修正外**逐字节不变**（回归门）；
`AbiRegistry` 单例与 `Box::leak` 不在本轮动。

#### 验收

S5/S6；R1/R2/R3 消除；新增 `hfa_caller_callee_agree`（同签名在
win64/sysv64/aapcs64/lp64d 下「调用方计划 == 被调方计划」）+ aapcs64 HFA 真跑（arm64 QEMU 通道，
缺通道显式 skip）。

### M3 能力数据化（去 x86 硬编码 + 单一合法性判据 + A6 收口）

#### 改动

- `TargetMachine::features(&self) -> FeatureSet`（ISA 申报 ∪ 宿主探测，名字是数据）；
  `TargetRegInfo` 增 `wide_vector_bytes(&self) -> &[u16]`、`vector_by_value_max(&self) -> u16`（缺省 16）。
  管线四处（`compiler.rs:1935/1942/1990/2310`）改问能力，删对 `avx512_available()` 的直接调用。
- 聚合阈值只从引擎来：`AbiPlan::agg_is_indirect(size, align)`（由 `rules.rs` 的
  `size_gt`/`size_le`/`IndirectVia` 派生）；G2 的 7+2 处 `> 8`/`> 16` 与两处 `div_ceil(8)` 全走它。
- 合法化判据合一：删 `agg_expand.rs:14-150` 的 7 个 `has_*`，改成「无条件进 pass，pass 自报
  `ChangeSet`」；仅保留一处 dry-scan 决定「是否需要 clone」（消除 C2 的静默漏判）。
- A6 收口：fp-outside 的 callee-saved 改为**运行时按 `plan.callee_saved` 循环**发射（fp-inside 已是），
  删谱面静态 push 表与 `frame_layout.rs:44-58` 的「按谱表数兜底」分支；无 plan ⇒ 明确报错。
- 删死钩子与死 API（G4/R7）：`TargetPeephole`、`TargetSimulator`、`AllocContext`、
  `TargetEncoder::encoded_size`/`encode_into`、`AllocResult::dummy_for_sizing`、
  `RegAllocConfig::new`（改 `from_target(machine)` 唯一构造）。
- 修 `agg_const.rs:38-41` 的小端写死（随常量池段端序）。

#### 边界

`FORGE_ASSUME_AVX512` 仍是**生成期**开关（语义不变），只是不再被通用管线直接读；
`features` 名集合的 owner 是 ISA 谱/后端，未知名字必须 fail-closed 而非默认 false。

#### 验收

S1/S2/S5/S7；`frame_layout.rs` 单测改按 plan；三 ISA 真执行矩阵不变；
新增守卫：通用管线不得出现任何 `avx*` 与 `size_bytes(..) > 字面量`。

### M4 单一操作数模型（删平行旁表 + 清死约束接口）

#### 改动

- `forge-isa-runtime`：`pub struct InstOperand { reg: XReg, field: u8, class: RegClass,
  constraint: OperandConstraint, role: Use|Def, settable: bool }`；`MachineInst` 只保留
  `operands(&self) -> SmallVec<[InstOperand; 4]>`、`set_operand_reg(&mut self, i, preg, class) -> bool`，
  以及 `effects`/`is_branch`/`branch_targets`/`is_call`/`is_ret`/`memory_access`/`is_move`/`clobbers`；
  删 `uses`/`defs`/`use_constraints`/`def_constraints`/`reg_field`/`set_reg_field`/
  `is_reg_field_settable`。
- 生成器（**机械同步**，`crates/frontend/forge-isa-dsl/src/dsl/codegen/machine.rs`）：`__Shape` 的
  `uses`/`defs`/`def_reuse` 合并成「按字段序」的约束表，加发 `operands()`/`set_operand_reg()`，
  删 10 个方法；`gen_lowering` 每 arm 的 `map_reg_field` 调用全部删除。
- `InstPacket` 只剩 `insts`/`inputs`/`outputs`/`xregs`：删 `xreg_map`、`map_reg_field`、
  `remap_internal`（XReg 重编号这一整类 bug 面消失）。
- 管线：`CompileState` 只留 `inst_ops: Vec<SmallVec<[InstOperand; 4]>>`，删 `xreg_map`/
  `inst_clobbers`/`ctx.xreg_types`；liverange/regalloc/emission 统一读它；class 回写只留一份（修 R3）。

#### 边界

约束表必须让 `ReuseInput` 真正可达（C5 的约束机制要么接通、要么在 M5 明确删除——
**不留半接通**）；`determinism` 按新契约更新后仍须「同 TOML 同产物」。

#### 验收

`machine_shape_table.rs` 改钉 operands 表；`no_hardcoded_widths.rs`/`library_surface.rs`/
`isa_roundtrip_guard`/`spec_coverage_guard` 全绿；JIT 两矩阵不变；S3。

### M5 分配器与发射（正确性 + 热点；regalloc 占 codegen 58–76%）

#### 改动

- 修 C3：池过滤按**该类自己的** sp/fp（或生成侧直接剔净）。三个发行 ISA 各释放 2 个寄存器。
- 修 C4：`RegAllocConfig.callee_saved: Vec<(RegClass, u32)>` 一张表（plan 给），`pop_free` 按类查；
  删 `callee_saved`/`callee_saved_fpr` 两份平行列表；新增「FPR callee-saved 真的被保住」用例。
- 清 C5：约束四臂由 `operands()` 的 constraint 驱动（否则删）；`precolored` 接通或删；
  驱逐按 `next_use_after` 真正取「最远的下一次使用」（Belady），并允许无死值时按 spill cost
  选活跃值——**文档与实现必须一致**，选「实现」。
- 活区间单次遍历（`liverange.rs` + `regalloc_bt.rs:193-378`）：三趟合成一趟；`weight` 要么参与
  驱逐、要么删。
- 输出密集化：`RegAllocOutput { assign: Vec<Option<PReg>>, spills: Vec<Option<SpillSlot>> }`；
  `AllocResult` 拆成 `RegAllocOutput` + `FrameLayoutResult` + `ParamPlaces(Vec<ParamPlace>)`
  （消 `param_vregs`/`param_is_float`/`param_by_ref`/`param_is_32`/`param_bytes` 五条平行数组）；
  分配器不再构造它不知道的字段（`sret`/`stack_arg_bytes`/`va_*` 由管线填）。
- spill 显式编辑：分配器产出 `SpillEdit` 序列（store / reload / scratch 绑定），emission 退化为
  「按序发射」；删 13 参 `emit_inst_with_spills`、scratch 池分配与 `settable` 预检；
  `emit_spill_load/store` 的 `is_fp: bool` 改 `RegClass`（解开 VEC/KReg 分派）。
- 帧记账单点：`FramePlan` 一次算好放进 `LowerCtx`/`AllocResult`，删 `compiler.rs:100/2047/2299`、
  `frame_layout.rs:118`、`emission.rs:38` 的重算（C6）。
- `MachBuffer`（新 `forge-isa-runtime/src/machine/machbuf.rs`）：标签引用/绑定、分支松弛固定点、
  常量池岛、重定位记录——替掉 `emission.rs:51-150` 的「非返回块先发、返回块最后」次序 hack、
  `ExternalLabel::EPILOGUE` 哨兵（`:156-161`）与 `needs_epilogue_label` 特判；JIT（`jit.rs:325-369`）
  与 `ObjectWriter` 共用同一份重定位语义，删 JIT 自己的 `compute_patch_value` 复制。

#### 边界

先做「**输出等价**改写」（不改指令序列与字节），单独一步再改 spill 策略，保证每步都能
用矩阵判定；x86 前向跳转 / riscv 分支范围 / QEMU 通道是 MachBuffer 的三条硬回归。

#### 验收

spill 压力与「固定字段 spill」用例按新语义改写；新增「必须驱逐活跃值」用例
（现状 fail-closed → 可编且结果正确）；JIT 两矩阵 + `asm_exec` + forge-rustc e2e 全绿；S9。

### M6 合法化单一驱动 + 残余缺口泛化

#### 改动

- 新 `pipeline/legalize/`：`trait LegalizePass { fn run(&self, func: &mut Function, model: &dyn AbiModel,
  ctx: &mut LegalizeCtx) -> Result<ChangeSet, IrError>; }` 与 `LegalizeDriver`（定序、定点、变化报告、
  幂等守卫四个职责）。8 个 `expand_*` 收成 5 个 pass：`AggStore`、`AggCall`（结果/参数/返回/sret
  合一）、`AggParam`、`Gep`、`Va`。
- 去重（R6）：`fn pack_segment(bytes, off) -> u64`、`fn place_addr_of_slot(...)`（`Alloca(Type(I8),
  Uint(n))` 形态的唯一出口）、统一调用 `agg_expand::move_to_front`（删 4 处内联）。
- 缺口泛化（按依赖序）：`va_arg` 聚合分支（现在静默按标量 load，`va_expand.rs:628-632`）走同一份
  分类；`>16B` 非 sret 栈返回（`compiler.rs:199/469-474/702-706`）用 `StackByVal` 返回；
  未对齐嵌套字段（`agg_expand.rs:211-215`）改逐字节拷贝；聚合值其余算子（`agg_expand.rs:645-650`）
  改值内存化。
- `va_expand.rs:625-627` 的 C 默认提升（f32→f64→Fptrunc）移进 `AbiRules` 数据；
  `int_ty()` 的「非 1/2/4/8 宽度静默折 I64」改 fail-closed。

#### 验收

`test_jit_agg_*` 全族 + 新增 `va_arg` 聚合真跑 + 未对齐字段用例；合法化幂等
（第二次 `ChangeSet::empty`）；e2e 全绿；`rg 'fn expand_' pipeline/compiler.rs` 命中 ≤ 3；S8。

## 6. 横切纪律

1. **破坏性更新**：删掉的类型**不做** deprecated 别名与兼容层；`CHANGELOG.md` 每里程碑一条
   `### Changed (日期)`，点名 deleted / replaced-by。
2. **每条被删机制配一条「禁回潮」守卫**：新增
   `crates/backend/forge-codegen/tests/backend_contract.rs`——通用管线无 x86 能力名、无
   `size_bytes > 字面量`、无平行旁表、无多条入口、无 `CallLayout`、无 `TargetPeephole`/
   `TargetSimulator`。守卫条目必须在真实文件里被命中（沿用本仓「白名单条目必须被命中」的做法）。
3. **文档三方针**：新增/改动键必须同时改 `docs/reference/backend-pipeline.md` + `CLAUDE.md`
   架构段 + `docs/README.md` 索引，并留一份钉住文档与代码的守卫。
4. **每里程碑都跑**：S6 全套 + 本里程碑新增守卫；本文件 §7 回写进度、实测数字、命令与代码基线。
5. **未验证不假绿**：QEMU/AVX-512 通道缺失时按既有做法写 `SKIP` 事件（`FORGE_JIT_EVENTS`），
   并在本文件如实记录。

## 7. 进度账本

| 里程碑 | 状态 | 证据（命令 / 数字 / 守卫） | 日期 |
| --- | --- | --- | --- |
| 立项（本文） | ✅ 落盘 | `docs/plans/backend-v23-plan.md` + `docs/README.md`/`CLAUDE.md` 指针 | 2026-10-11 |
| M1 编译入口收敛 | ✅ 完成（含一条记录在案的偏差） | 见 §10 M1 落地记录 | 2026-10-11 |
| M2 单一 ABI 数据模型 | ✅ 已落地（C1 修复 + leaf crate + 形状/变参/寄存器三面去镜像 + **删运行时镜像**） | 见 §11 M2 进度 | 2026-10-11 |
| M3 能力数据化（去 x86 硬编码 + 单一合法性判据 + A6 收口） | ✅ 已落地（死钩子/死 API 删除；端序修正；宽向量能力数据化 + ISA 中立守卫；A6 收口；触发判据合一 + `ChangeSet`） | 见 §12 M3 进度 | 2026-10-11 |
| M4 单一操作数模型 | 🚧 进行中（爆炸半径已量清 ≈350 处；批次 A/B/C 已定） | 见 §13 M4 进度 | 2026-10-11 |
| M5 分配器与发射 | ⏳ 待开工 | — | — |
| M6 合法化与缺口 | ⏳ 待开工 | — | — |

## 8. 顺序、风险与假设

- **顺序**：M1 → M2 → M3 → M4 → M5 → M6。依赖：M2 是 M3/M6 的前置（阈值/形状只有一份才谈得上
  「能力数据化」）；M4 是 M5 的前置（spill 编辑按 operands 生成）；M3 的「删死钩子」（G4/R7）
  体量小、无依赖，可提前到 M1 顺手做。
- **风险**：M4/M5 动生成物契约 → 由 `determinism`、`machine_shape_table`、`no_hardcoded_widths`、
  `isa_roundtrip_guard`、`spec_coverage_guard` 五条守卫托底；M5 的 spill 编辑动真执行路径 →
  分两步（先等价改写，再改策略）；C3/C4 修好后**寄存器分配结果变化**（可能改变生成字节），
  矩阵按「值正确」判定，字节级黄金若有需同步重刷并在本文件写明原因。
- **假设**：三份发行谱与 10 份夹具可随生成器契约同步改；QEMU 通道可判定（否则 skip 如实记录）；
  M2 后 `forge-abi` 四份 `.plan.txt` 黄金除 HFA 修正外不变；`forge-abi-model` 引入后
  `forge-isa-runtime` 不再受「不能依赖 ABI 数据」约束（这是本方案刻意打破的旧约束）；
  取 leaf crate 以免把 serde/toml 拖进运行时。

## 9. 参考设计（外部，作为取舍依据）

- Cranelift 的机器指令/VCode 模型——操作数（`Reg`/`VReg`/`InsnInput`/`InsnOutput`）挂在指令上、
  不另立平行旁表：[cranelift_codegen::machinst](https://docs.wasmtime.dev/api/src/cranelift_codegen/machinst/mod.rs.html)、
  [machinst/vcode.rs](https://raw.githubusercontent.com/bytecodealliance/wasmtime/0f4c0d4a358187f99f23897d306c8b2a3c94287e/cranelift/codegen/src/machinst/vcode.rs#1)
  ——对应 M4/M5。
- ISLE：把「匹配 + 发射」变成数据/规则而不是手写分支链
  （[cranelift-isle README](https://android.googlesource.com/toolchain/rustc/+/87880447cc5437c45a3562596a9766d9797e7318/vendor/cranelift-isle/)）
  ——对应 M3 的「单一判据来源」结论（不引入脚本引擎）。
- LLVM GlobalISel 的 TableGen 发射器：ISA 描述一次、多消费者同源
  （[GlobalISelEmitter.cpp](https://android.git.googlesource.com/toolchain/llvm/+/e0714cfc0fecd4a47349ffc676f1d91c03eb4e1c/utils/TableGen/GlobalISelEmitter.cpp)）
  ——对应 M2/M4 的「单一事实源」。
- 本仓内部可照抄的先例：`frame_slots::scan_frontend_slots`（单一实现）、
  `dsl/codegen/moves.rs` 的 `MoveTable`（唯一查表实现）、`fold_short_forms`（生成器后处理唯一入口）。

## 10. M1 落地记录（2026-10-11）

### 改动

- `pipeline/session.rs`（新）：`CompiledModule`（**纯数据且类型无关**：函数按 `FuncRef` 序、
  全局双符号、数据段、`isa` 标签；`into_parts` 供装载）、`CompileSession::compile_module`
  （整模块唯一实现：≥8 函数走 `std::thread::scope` 并行，串行/并行共用同一个 `compile_one`）、
  `CompileSession::add_function`（单函数便捷入口）。
- `pipeline/compiler.rs`：`FunctionCompiler` → `CompileSession`（`machine()` 降为 `pub(crate)`），
  删 `compile_raw`（`compile` 与它本来就是同一句调用），字段 `module_sigs` 改
  `Option<Arc<ModuleSigs>>`。
- `foundation/forge-ir`：新增 `ModuleSigs`/`FuncSigEntry`，`Module::signature_table()` 改返回它。
- `foundation/forge-isa-runtime`：`CallConvCtx::module_sigs: Option<Arc<ModuleSigs>>`；
  `variadic_of` 改走 `variadic_at`；`FunctionPipeline`/`ErasedTargetMachine` 的方法名
  `compile_raw` → `compile`。
- `runtime/jit.rs`：`JitCompiler` → `JitLoader`；`add_function`/`compile_module` 改**委托**
  `CompileSession`，装载收敛成 `install(CompiledModule)` + `load_function`（含 ISA 核对）；
  新增 `sigs: ModuleSigs` 按序累积（修单函数入口丢签名表）。
- 全仓机械收尾：`FunctionCompiler`/`JitCompiler`/`compile_raw` 在 `*.rs` 里 0 命中。

### 验收证据

| 判据 | 命令 | 结果 |
| --- | --- | --- |
| S4 | `rg 'FunctionCompiler\|JitCompiler\|compile_raw'`（`*.rs`） | 0 命中 |
| S6（门禁） | `cargo test --workspace --exclude forge-rustc` | `CARGO_EXIT=0`（含 forge-codegen 2802 条 lib 测试、~150 组 test result 全绿；`asm_parse`/`asm_encoding`/`asm_exec` 三档与计数棘轮同在这一跑里通过） |
| S6（编译面） | `cargo check --workspace --exclude forge-rustc --all-targets` | 0 error |
| S6（clippy） | `cargo clippy --workspace --exclude forge-rustc --all-targets --all-features -- -D warnings` | `exit 0` |
| S6（fmt） | `cargo fmt --all -- --check` | `exit 0` |
| S6（JIT 矩阵） | `cargo test -p forge-tests --lib jit_matrix_x86 -- --nocapture` | **197 passed / 3 skipped / 0 failed**（与立项基线逐数相同） |
| S6（JIT 矩阵） | `cargo test -p forge-tests --lib jit_matrix_riscv64 -- --test-threads=1 --nocapture` | **136 passed / 64 skipped / 0 failed**（与立项基线逐数相同） |
| 新增守卫 | `cargo test -p forge-codegen --test session_api` | 4 passed（`FuncRef` 序 / 两条路径字节相同 / ≥8 并行同源 / `add_function` 认签名表） |
| 装载核对 | `cargo test -p forge-codegen --all-features --lib jit::tests::install_rejects` | 1 passed（x86 产物装进 riscv 装载器 ⇒ 明确拒绝、消息点名两侧 ISA） |
| 变参洞（本条修复） | 同上第 4 条 | 装表与不装表的 `caller` 字节**不同**（不装表 = 修前的坏行为） |

> 上表在**冻结状态**上重跑过一遍（`cargo fmt --all` 之后）：clippy / 门禁 / 两条矩阵
> 四项同一次扫里全绿；`cargo check -p forge-codegen --all-features --all-targets` 亦 0 error。

**顺带清掉的三处存量 lint 债**（`clippy::if_same_then_else`，与本里程碑无关，只在本机浮动
nightly 上被点出——CI 钉版 nightly-2026-09-05 未报）：`dsl/model.rs` 的 `FamilyCarrier::expand`、
`dsl/validate.rs` 的族锚定、`forge-isa/src/main.rs` 的 `explain` 来源行——三处都是"两个分支产出
同一字符串"的历史残留（v21 统一族节名后分支未删），现各自收敛成一句 `format!("[family.{…}]")`
并留注释说明为何两种来源同节。

### 偏差（记录在案，不是遗漏）

1. **`JitLoader::add_function` / `JitLoader::compile_module` 保留为薄委托**（各 3–5 行，自身不含
   管线调用）。原方案要求删掉它们；实测这两条 API 有 **263 处测试调用点**（`runtime/jit.rs` 的
   测试模块），删除属纯机械但体量大的 sweep，且不改变"编译实现唯一"这一实质（S4 已按实质改写）。
   后续若要做这次 sweep，只需把 263 处改写成"先 `CompileSession`、再 `JitLoader::install`"。
2. **`FuncSigEntry` 只有 `variadic` + `fixed_args`**，未按方案原文加 `param_count`：IR 里
   "变参函数的未命名实参个数"不是签名事实（调用点各次不同），加了没有消费者 ⇒ 冗余。
3. **擦除入口仍是 `compile(&Function)`**（方法名已改，未改成 `compile_module(&Module)`）：
   擦除产品的类型（`CompiledModule<M>` 对擦除面不可命名）要与 M5 的装载/重定位统一一起定，
   现在改会先造一个只能用一次的中间类型。

## 11. M2 进度（进行中）

### 已落地（2026-10-11）：C1 —— 聚合落点探针必须带**成员形状**

`compiler.rs` 的两个侧信道探针 `agg_arg_place` / `agg_ret_is_sret` 原先伪造
`Aggregate { members: [] }`（只给 size/align）。后果：AAPCS64/LP64D 的 HFA
（`{f64,f64,f64}` 这类走浮点寄存器池）在**调用方一侧**被当成"普通聚合"⇒ 返回
`ByRef`，调用方按"本地副本 + 传指针"改写两端，而被调方按浮点寄存器收参 ⇒ **静默错值**。

- 现在两个探针取 `(&TypeStore, TypeId)`，形状一律走 `ArgShape::from_ir_type`
  （与调用点 plan、被调方 plan **同一份投影**，递归摊开成员/lane）。
- 三个调用点（`expand_large_agg_params` / `expand_sret_return` / `expand_agg_call_args`）
  改为传类型；**读锁处数不变**（判定与探针合并进同一段读锁，`read_path_budget` 仍为 20）。
- 守卫：`compiler.rs::alloc_integration_tests::aggregate_place_probe_carries_member_shapes`
  —— 用**同尺寸、不同成员**的一对类型证明"成员真的进了探针"：
  `{f64,f64,f64}`（24 B，HFA）⇒ `Registers`；`{i64,i64,i64}`（24 B，非 HFA）⇒ `ByRef`。
  只比 size 的旧实现必然给两者同一答案 ⇒ 这条守卫就是 C1 的判据。

### 已落地（2026-10-11，第六批）：**删掉运行时镜像——M2 收口**

- **`CallLayout`/`CallArg`/`ArgPlace`/`RetPlace`/`Ext` 整条删除**：`AllocResult::call_layout` 与
  `CallConvCtx::layout` 的类型**就是** `AbiPlan`；`abi_target::call_layout`（~260 行翻译）消失，
  调用点钩子 `plan_call` 直接返回 `AbiPlan`。
- **落点访问器**（`Placement::{as_reg, indirect_ptr, stack_offset, is_stack_byval, reg_refs}`、
  `RetLoc::{as_reg, reg_refs}`、`AbiPlan::{arg, caller_offset, slot_bytes, shadow_bytes,
  hidden_sret_slot, va_meta_slot}`）：生成物 12 处 + 调用点 5 臂改走访问器，
  **不再穷举枚举**（"引擎加变体、发射静默走错分支"结构上不可能）。
- `RetLoc` 不再 `Option`（`Void` 表"无返回"）；栈区字段统一从 `plan.stack` 读。
- 运行时 `machine/call_layout.rs`：255 行 → 78 行（只剩 `va_list` 的宿主侧外壳 `VaInfo`/
  `VaInit`/`VaInitVal` + 模型变参类型的再导出）。
- 守卫同步（`call_layout_emission.rs` 判据 = 落点访问器）；生成物解析失败时打印**谱名** +
  落盘 token 原文（本轮排查用上，留在生成器里）。

### 已落地（2026-10-11，第五批）：变参类型只剩一份（删 5 个手抄镜像类型）

- 运行时的 `VaArgRule`/`VaArgRules`/`VaField`/`VaSaveArea`(=`VaSave`)/`VaSaveSlot`
  **五个手抄类型整条删除** → `pub use forge_abi_model::plan::{…}`。此前 `VaArgRule` 的
  9 个字段在计划面与镜像各写一遍，`VaField.name` 还在翻译时被**静默丢弃**（现在回来了）。
- 翻译层删掉 `va_arg_rules`/`va_arg_rule_one`（20 行逐字段搬家）与字段/槽表重建（~15 行）：
  `VaInfo` 现在只剩"宿主算出来的 `VaInit`"要填（那个**不是**镜像——它依赖"已用掉几个
  参数寄存器"，只有编译入口知道）。
- 模型侧 `VaArgRule`/`VaArgRules` 补 `Copy`（纯小记录）。
- 生成物的寄存器保存区 store 序列改读 `__slot.reg.slot`/`__slot.reg.class`
  （`dsl/codegen/frame.rs` 的 4 个 emitted 站点）——生成物里也只剩一种寄存器表示。

### 已落地（2026-10-11，第四批）：`RegRef` 带类型化类与类内号（翻译层收敛）

- `RegRef { index /*ABI 空间号*/, slot /*类内号*/, class: forge_ir::RegClass, name }`：
  两个坐标各有其主（引擎按 `index` 记账、发射按 `(class, slot)` 构造寄存器），
  文档写明**不是重复表示**。
- `AbiTarget` 增 `reg_class(index) -> RegClass` 与 `reg_slot(index) -> u32`——
  "整数区有几个"是**机器事实**，由宿主答一次，下游不再回查。
- **翻译层的寄存器映射**从 20 行"按名字回查 + 判断 + 退化"收敛成
  `let reg = |r| (r.class, r.slot);`（`abi_target::call_layout`）。
- 5 处 `AbiTarget` 实现补齐（真实后端 / `forge-isa` 的 `ViewTarget` / `StubTarget` /
  三处测试替身）；新增 `forge_ir::RegClass::from_name`（`Debug` 形态的逆，供谱侧类名串还原）。
- `AbiPlan` 一族的 serde 派生全部去掉（实测无消费者；只有 TOML 面的 `CalleeSaveMechanism` 留）。

### 已落地（2026-10-11，第三批）：形状只剩 `TyView`（删 `ArgShape`/`ShapeKind` + 两份投影合一）

- **删掉运行时侧的中性形状类型**：`ArgShape`/`ShapeKind`（`machine/call_layout.rs` 里 19 处定义、
  `call_plan.rs` 8 处）整条删除；`CallRequest { args: Vec<TyView>, rets: Vec<TyView> }`。
  `abi_target::shape_to_ty` 那座"运行时形状 → 引擎视图"的桥（match 30 行）随之消失。
- **IR → 视图的投影只剩一份**：`TyView::from_ir_type`（`forge-abi-model/src/ir_view.rs`，
  深度上限 4、成员上限 16）。此前两份且**退化策略不一致**：运行时的 `ArgShape::from_ir_type`
  与代码生成的 `sig_view::ty_view`——后者**没有深度上限**（递归类型会栈溢出）且长数组折成
  "一个整块 int"，正是"调用点与被调方对同一类型分类分叉"的温床（C1 的同类问题）。
  `sig_view::{ty_view, element_kind, expand_members}` 全删（`signature_view` 直接用唯一投影）。
- 生成物路径：`crate::machine::call_layout::ArgShape` → `crate::machine::abi_model::TyView`
  （运行时 `machine/mod.rs` 对 leaf crate 做**再导出**，路径因此稳定、M2 收口时不必再改）。
- 依赖面守卫（`runtime_surface.rs`）的白名单加入 `forge-abi-model` 并**写明理由**
  （leaf 数据 crate：只有 serde+forge-ir，不含引擎/规则加载层/管线）。
- 影响 `ty.rs` 的 `TyKind::Vector` 只剩 `{elem, lanes}`——旧 `elem_bytes` 字段（审计标注
  "要求传入后被丢弃"）不再存在；向量的 size/align 一律取 `TypeStore` 的权威值。

### 已落地（2026-10-11，第二批）：leaf crate `forge-abi-model` + 删掉 `RegRef` 的死字段

**① 产物抽成 leaf crate**（M2 的"删镜像"前置）：

- 新 crate `crates/foundation/forge-abi-model`：`plan.rs`（`AbiPlan`/`Placement`/`RetLoc`/
  `ArgLoc`/`StackLayout`/`HiddenSlots`/`VaArea`/`VaArgRule(s)`/`VaField`/`VaSave*`/`DeclAttrs`
  与 `to_text`）、`ty.rs`（`TyView`/`TyKind`/`Elem`）、两个**规则侧小枚举**
  （`PositionRule`/`CalleePop` + `resolve_callee_pop`，它们同时是 `AbiRules` 与 `AbiPlan`
  的字段类型）。依赖只有 `serde`（`AbiPlan` 的 serde 派生实测**无消费者**，本可去掉，
  但两个规则枚举是 TOML 面类型，先留着——`toml` 不进运行时）。
- `forge-abi` 变成"规则/绑定/引擎/注册表"并 `pub use forge_abi_model::{plan, ty, rules_pod}`
  ——`forge_abi::plan::RegRef`、`forge_abi::AbiPlan`、`forge_abi::TyView` 等路径**全部照旧**，
  因此全仓 0 处调用改动（只有 `engine.rs` 一处 `AbiPlan::resolve_callee_pop` 改指模型）。
- 依赖图与 `CLAUDE.md` 同步（`forge-abi → forge-abi-model`）。

**② 删掉 `RegRef` 的**死**字段** `class: String`：

- 实测**全仓没有一个读者**（发射侧要的"类 + 类内号"由 ISA 的目标视图给出，不由这份数据携带；
  代码里 `.class` 的命中全是 `XReg::class()`/`PReg.class`/`LiveInterval.reg_class`）。
- 连带删掉只喂养它的 `AbiTarget::reg_class_name`（trait 方法 + 6 处实现：真实后端的
  `MachineAbiTarget`、`forge-isa` 的 `ViewTarget`/`StubTarget`、forge-abi 的三处测试替身）
  与 `ViewTarget.classes` 字段；`exotic_hooks.rs` 的 `reg()` 助手同步。
- `RegRef` 当时收敛成 `{ index: ABI 空间号, name: 谱里的寄存器名 }`——**先删死字段，
  不留两种表示**；有类型的 `RegClass` 与类内号已在**第四批**以有类型形态回来
  （`{index, slot, class, name}`）。

### 待落地（设计已定）

1. **leaf crate `forge-abi-model`**：把 `forge-abi` 的 POD 搬过去
   （`plan.rs` 全部 + `ty.rs` + `engine.rs` 的 `Signature`），`forge-abi` 只做
   `pub use forge_abi_model::*` 保持 `forge_abi::AbiPlan` 等路径不变。
   实测 `AbiPlan` 的 serde 派生**无消费者**（无 `to_string(&plan)`/`from_str::<AbiPlan>`），
   可整批去掉 ⇒ 模型 crate 零依赖（只多一个 `forge-ir`，见下条）。
2. **`RegRef.class` 由 `String` 改 `RegClass`**：这是"删镜像"的关键一步。现状
   `call_layout(plan, machine)` 之所以需要 300 行翻译，正是因为 `AbiPlan` 里的寄存器是
   **ABI 空间号 + 类名字符串**，而生成物要 `(RegClass, 类内号)`；把这一步挪进**引擎**：
   `AbiTarget` 增 `fn reg_class(&self, index: u32) -> RegClass`（宿主才知道自己的类），
   引擎在造 `RegRef` 时一并填 `class`。`name` **保留**（`to_text` 的黄金快照按它渲染）。
   于是 `forge-abi-model` 依赖 `forge-ir`（leaf，无内部依赖），`forge-abi` 与
   `forge-isa-runtime` 都依赖它，`CallLayout`/`ArgPlace`/`RetPlace`/`VaInfo` 镜像与翻译层
   整段删除，生成物直接读 `AbiPlan`。
3. **单一 IR→形状投影**：`sig_view::ty_view`（被调方）与 `ArgShape::from_ir_type`（调用方）
   合并成一份（保留 `from_ir_type` 的深度/成员上限——`ty_view` 目前**没有深度上限**，
   递归类型会栈溢出，合并时一并修掉）。
4. 预计触碰面：`CallLayout` 42 处 / `ArgPlace` 86 / `RetPlace` 25 / `ArgShape` 81 /
   `ShapeKind` 24 / `VaInfo` 17，分布在 20 个文件（含生成器模板与 20+ 测试）。

### 验收证据

#### 第一批（C1 修复）

| 判据 | 命令 | 结果 |
| --- | --- | --- |
| C1 守卫 | `cargo test -p forge-codegen --all-features --lib aggregate_place_probe` | 1 passed（同尺寸 HFA ⇒ `Registers` / 非 HFA ⇒ `ByRef`） |
| 读路径预算 | `cargo test -p forge-ir --test read_path_budget` | 2 passed（`compiler.rs` 仍为 20——探针与判定合进同一段读锁） |
| S6（门禁） | `cargo test --workspace --exclude forge-rustc` | `gate_exit=0` |
| S6（clippy/fmt） | `cargo clippy --workspace --exclude forge-rustc --all-targets --all-features -- -D warnings`；`cargo fmt --all -- --check` | 均 `exit 0` |
| S6（矩阵） | x86 / riscv64 两条 JIT 矩阵 | **197/3/0** 与 **136/64/0**（与基线逐数相同） |

#### 第六批（删运行时镜像——M2 收口）

| 判据 | 命令 | 结果 |
| --- | --- | --- |
| S5（去镜像） | `rg 'CallLayout\|ArgPlace'`（`*.rs`，非注释行） | 0 处（只剩历史叙述的注释） |
| 翻译层 | `abi_target::call_layout` | 已删除（规划入口直接返回 `AbiPlan`） |
| 布局路径守卫 | `cargo test -p forge-isa-dsl --test call_layout_emission` | 7 passed（判据 = 落点访问器） |
| S6（门禁/clippy/fmt/矩阵） | 门禁 + clippy + fmt + 两条 JIT 矩阵 | 全绿（gate `exit 0`；clippy `exit 0`；fmt `exit 0`；矩阵 **197/3/0** 与 **136/64/0**） |

#### 第五批（变参类型只剩一份）

| 判据 | 命令 | 结果 |
| --- | --- | --- |
| 类型去重 | `rg 'pub struct VaArgRule\|pub struct VaField\|pub struct VaSaveSlot'` | 只剩模型里的一份（运行时是 re-export） |
| 翻译层 | `abi_target` 的 va 段 | 55 行缩减为 11 行（只剩 `init: va_init(a)`） |
| S6（门禁/clippy/fmt/矩阵） | 门禁 + clippy + fmt + 两条 JIT 矩阵 | 全绿（gate `exit 0`；矩阵 **197/3/0** 与 **136/64/0**） |

#### 第四批（`RegRef` 类型化 + 翻译层收敛）

| 判据 | 命令 | 结果 |
| --- | --- | --- |
| 翻译层 | `abi_target::call_layout` 的寄存器映射 | 20 行闭包 → 1 行 `(r.class, r.slot)` |
| S6（门禁/clippy/fmt/矩阵） | 门禁 + clippy + fmt + 两条 JIT 矩阵 | 全绿（gate `exit 0`；矩阵 **197/3/0** 与 **136/64/0**） |

#### 第三批（形状只剩 `TyView`）

| 判据 | 命令 | 结果 |
| --- | --- | --- |
| 类型去重 | `rg 'ArgShape\|ShapeKind\|shape_to_ty'`（`*.rs`） | 仅历史注释 0 处代码命中 |
| S6（门禁/clippy/fmt/矩阵） | 门禁 + clippy + fmt + 两条 JIT 矩阵 | 全绿（gate `exit 0`；矩阵 **197/3/0** 与 **136/64/0**，与基线逐数相同） |
| 依赖面守卫 | `cargo test -p forge-isa-runtime --test runtime_surface` | 3 passed（白名单加入 `forge-abi-model` 并写明理由） |

#### 第二批（leaf crate + 删死字段）

| 判据 | 命令 | 结果 |
| --- | --- | --- |
| 依赖面 | `cargo tree -p forge-abi-model --depth 1` = `serde` 一项；`cargo tree -p forge-abi --depth 1` = `forge-abi-model` + serde/thiserror/toml | ✔（toml/引擎不进模型 crate） |
| 路径兼容 | `cargo check --workspace --exclude forge-rustc --all-targets` | 0 error（`forge_abi::plan::*`/`AbiPlan`/`TyView` 路径未变，全仓 0 处调用改动） |
| S6（门禁/clippy/fmt/矩阵） | `cargo test --workspace --exclude forge-rustc`；`cargo clippy … -D warnings`；`cargo fmt --all -- --check`；两条 JIT 矩阵 | 全绿：gate `exit 0`、clippy `exit 0`、fmt `exit 0`、矩阵 **197/3/0** 与 **136/64/0** |

## 12. M3 进度（进行中）

### 已落地（2026-10-11，第一批）：删死钩子与死 API + 聚合常量端序

| 项 | 处置 | 证据 |
| --- | --- | --- |
| `TargetPeephole` | 删 trait + `run_peephole` + 编译器 Stage 5 + `TargetMachine::peephole()` + 模块 | 三份发行谱零实现；调用点恒 `None`（`rg 'peephole'` 只剩历史注释） |
| `TargetSimulator`/`SimulationState`/`SimError` | 删 trait/类型 + 访问器 + 再导出 + 模块 | 全仓零实现、零调用 |
| `TargetEncoder::{encoded_size, encode_into}` | 删两个默认方法 | 调用方一律用 `encode()` + `CodeSink`；`encoded_size` 要靠伪造分配记录才能跑 |
| `AllocResult::dummy_for_sizing` | 删（含其单测） | 只为 `encoded_size` 存在 |
| `AllocContext` | 删空结构 + `allocate()` 第 3 参 + 9 处测试调用点 | `let _ = ctx; // 预留（将来约束上下文）` —— 从未被读 |
| `agg_const.rs` 端序 | 小端写死 → 随 `TypeStore::data_layout.endianness` | 三份发行谱皆小端 ⇒ 字节不变；大端目标不再整字节反序 |
| 宽度守卫白名单 | 删 4 条随 API 消失的条目 | 守卫要求"每条必须被命中"，删 API 必须同步删条目 |

**验收**：门禁 `exit 0`、clippy `exit 0`、fmt `exit 0`；矩阵 **197/3/0** 与 **136/64/0**；
asm 解析档 3 passed（棘轮计数未变）；两条 `no_hardcoded_widths` 守卫 1 passed。

### 已落地（2026-10-11，第二批）：宽向量能力数据化（通用管线不再认识 `avx*`）

- `TargetRegInfo::wide_vector_bytes`（ISA 后端答：谱申报档位 ∩ 宿主上界；缺省 `&[]` ⇒ fail-closed）、
  `cpu::host_wide_vector_limit()`（宿主事实）、`reg_info::wide_vectors_upto()`（取前缀，`&'static`）三件套。
- 管线两处 `crate::avx512_available()` 调用删除 → 问 `wide_vector_bytes()`；错误文案改 ISA 中立
  （断言该文案的 W3 用例同步）。
- 新守卫 `tests/generic_pipeline_is_isa_neutral.rs`（口径：`src/pipeline/**` 代码行、**标识符级**匹配
  ——`classes` 里的 `sse` 不算、注释与 `#[cfg(test)]` 豁免、白名单须命中）；**负例实测**过
  （加回 `avx512_available()` 调用立刻红，撤掉即绿）。

**验收**：门禁/clippy/fmt 全绿；矩阵 **197/3/0** 与 **136/64/0**；守卫 1 passed（负例转红）。

### 已落地（2026-10-11，第三批）：A6 收口（帧账目只认 plan，无 plan 明确报错）

- `frame_layout_info()` 的 callee-saved 计数**只从 plan 来**，删掉 `unwrap_or(0)` 这条
  "无 plan 就按零算"的缺省（少算帧字节 ⇒ 局部/spill 槽与 push 槽重叠 ⇒ 覆盖调用者保存值，
  归入静默错值那一类）。无 plan ⇒ **panic**（编译入口本就 fail-closed，走到这里说明有路径漏接）。
- 纠正该函数里**过期的注释**：它写着"fp-outside 仍按谱里声明的表数（x86 的 push 是静态发射）"，
  但 v20 A4/v21 W9 之后谱面的静态 push 表与 `[emit.prologue]` 模板都已删除，
  `gen_push_mechanism` 序言按 `alloc_result.callee_saved_to_save` **运行时循环** push、
  `cs_imm`/`sp_base` 也取运行时长度——两条路（fp-inside/fp-outside）现在都是
  "运行时按 plan 的整张表"。
- 单测改钉新契约：`catch_unwind(frame_layout_info(&tm, None)).is_err()`（无 plan 必须 fail-closed）。

**验收**：`forge-codegen` 单测 2719 passed；门禁/clippy/fmt 全绿；矩阵 **197/3/0** 与 **136/64/0**。

### 已落地（2026-10-11，第四批）：触发判据只剩一处（7 个 `has_*` → 单一保守判据 + `ChangeSet`）

- **删掉 7 个各扫一种模式的 `has_*`**（`has_large_agg_ret`/`has_large_agg_call_result`/
  `has_large_agg_call_arg`/`has_large_agg_param`/`has_large_agg_load`/`has_agg_mem_access`/
  `has_any_agg`）→ 一处 `needs_agg_expand` = 含 GEP **或** 提到过聚合类型/聚合字面量。
  新判据**过近似**：多克隆一份 `Function` 只是慢；漏判则是"那个 pass 静默不跑"（C2 的失效模式）。
- 新增 `agg_expand::{ChangeSet, IrSize}`：展开后按 **IR 规模差**（指令/值/常量）+ sret 标记
  自报"改动了什么"，写进 `FORGE_TRACE_AGG`。选变化量而非 8 个 pass 各自计数，是因为它**完整**：
  将来新增 pass 自动被覆盖，不会再出现"新加一类模式却忘了加判据"。
- 守卫 `agg_expand::tests::{needs_agg_expand_is_conservative, change_set_counts_only_growth}`。

**验收**：`forge-codegen` 单测 2721 passed；clippy/fmt 全绿；门禁与两矩阵见下（S6）。

### 待落地（设计已定）

0. **`TargetMachine::features(&self) -> FeatureSet` 不再需要**（记录为**偏差**）：方案这一项是为
   "管线四处改问能力"服务的，而那四处真正需要的判据是**"能不能编/跑这么宽的定宽向量"**——
   已由 `TargetRegInfo::wide_vector_bytes` 表达。此刻再引入一个**没有消费者**的 `FeatureSet`
   正是本轮在删的那类死 API；将来出现"按名字问能力"的真实需求时，连同
   `vector_by_value_max`（按值传参向量上界，目前由约定的 `by_ref_limit` 表达）一起做。
1. `TargetRegInfo::vector_by_value_max`（见上条，与 `features` 一并延后）。
2. 聚合阈值单一来源：`AbiPlan::agg_is_indirect(size, align)`（由 `size_gt`/`size_le`/`IndirectVia` 派生），
   `agg_expand.rs` 5 处 + `compiler.rs` 5 处的 `> 8`/`> 16` 与两处 `div_ceil(8)` 全走它。
   **注意**：`agg_expand` 的 `> 8` 是"按 i64 段拆"的 IR 表示决策（步长来自槽宽），与 ABI 的
   `> 16 ⇒ 内存` 是两件事——迁移时要把这两类分开，别把 IR 拆段也塞进合法化判据。
3. 合法化判据合一：删 `agg_expand.rs` 的 7 个 `has_*`，改成「无条件进 pass，pass 自报 `ChangeSet`」，
   只留一处 dry-scan 决定是否 clone。
4. A6 收口：fp-outside 的 callee-saved 改为运行时按 `plan.callee_saved` 循环；删静态 push 表与
   `frame_layout.rs` 的"按谱表数兜底"分支。
5. 新守卫：通用管线不得出现任何 `avx*`，也不得出现 `size_bytes(..) > 字面量`。
6. `RegAllocConfig::new`：本轮**未删**（记录为偏差）——它只在 5 处测试里用，生产路径是
   `compiler.rs` 的显式结构字面量 + plan。方案的"`from_target(machine)` 唯一构造"需要
   M5 把 `callee_saved` 表并进 config，届时一并收口；现在删会把 5 处测试改写成需要机器实例，
   收益与风险不成比例。

## 13. M4 进度（进行中）

### 爆炸半径（2026-10-11 实测，`rg` 计数）

| 符号 | 命中 | 主要落点 |
| --- | --- | --- |
| `reg_field` | 121 | 生成器 `dsl/codegen/lowering.rs` 48、`machine.rs` 17、`regalloc_bt.rs` 9、`integration_tests.rs` 8 |
| `xreg_map` | 63 | 生成器 19、`regalloc_bt.rs` 17、`compiler.rs` 7、`emission.rs` 7 |
| `map_reg_field` | 54 | 生成器 42（每 arm 一处）、`placeholder.rs` 6、`move_derive.rs` 3 |
| `xreg_types` | 49 | 生成器 26、`emission.rs` 6、`integration.rs` 6、`ctx.rs` 3 |
| `set_reg_field` | 25 | `regalloc_bt.rs` 5、生成器 4、`integration_tests.rs` 4、`inst.rs` 3 |
| `uses`/`defs` | 9 | `inst.rs` 4、`machine.rs` 2、`regalloc_bt.rs` 2、`use_list.rs` 1 |
| `use_constraints`/`def_constraints` | 8 | `machine_shape_table.rs` 4、`inst.rs` 2、`machine.rs` 2 |
| `remap_internal` | 4 | 生成器 `lowering.rs` 4 |
| `inst_clobbers` | 6 | `compiler.rs` 3、生成器 2、`ctx.rs` 1 |

合计 ≈ 350 处、跨生成器与管线两侧——**一次改完必然中途破树**，因此按"契约 → 消费端 → 删旧"三批走，
每批都要求门禁全绿：

### 批次 A（契约先行，管线不动）——✅ 已落地（2026-10-11）

**落地内容**：`MachineInst::operands() -> SmallVec<[InstOperand; 4]>`，每项
`{ field, slot, class, constraint, role, settable }`；顺序契约 = `Use` 在前、`Def` 在后。
`uses`/`defs`/`use_constraints`/`def_constraints`/`reg_field` 全部改成**由它派生的缺省实现**
（生成物暂仍显式发射，正好当对拍的另一边）；新增回写入口
`set_operand_reg(i, preg, class) -> bool`（`settable == false` ⇒ `false`，供 regalloc 对
spilled def fail-closed）；手写实现（`DummyInst`/`FixedFieldInst`）改为只实现 `operands()`。

**守卫**：`src/operand_model_guard.rs`（`#[cfg(test)]` 模块，**必须放文件末尾**——本仓
"写死寄存器类"守卫按第一个 `#[cfg(test)]` 截断扫描）逐条对拍三份发行谱**全部指令变体**的
七个方法与操作数表，**含 `def_reuse → ReuseInput` 映射**。它是批次 B/C"删旧"的前提证明。

**过程中被守卫拦下的真错**：`set_operand_reg` 最初用 `RegClass::GPR(64)` 兜底缺省类——
按本仓纪律**改实现**（要求调用方给类，与旧 `set_reg_field` 同签名），而不是加白名单。

**关于 `reg: XReg`（方案原写法）的更正**：微指令字段里放的是**占位物理寄存器**（`Reg`），
不是虚拟寄存器——vreg → 字段的对应目前只存在于 `InstPacket::xreg_map`（由 lowering 的
`map_reg_field` 逐 arm 填）。因此 `InstOperand` 这一批先只描述**形状**
（`slot: Option<u32>` = 占位槽物理号）。**批次 C 的前置条件**随之明确：
**让字段占位直接携带 vreg 号**（`Reg::from_index(vreg, slot_class)`），`xreg_map` 才可能删除；
否则只能保留旁表（那正是 M4 要消灭的东西）。

### 批次 A（原始设计，留档）

1. `forge-isa-runtime/src/machine/inst.rs`：新增
   `pub struct InstOperand { reg: XReg, field: u8, class: RegClass, constraint: OperandConstraint,
   role: OperandRole, settable: bool }`、`enum OperandRole { Use, Def }`，
   以及 `MachineInst::{operands(), set_operand_reg(i, preg, class) -> bool}`。
   两者**带缺省实现**，由既有的 `uses()`/`defs()`/`reg_field()`/`*_constraints()` 拼出
   （顺序 = `uses` 在前 `defs` 在后；regalloc 的"use 在 inst_start、def 在 inst_end"语义必须由
   顺序保证）。
2. 消费端逐个切到 `operands()`（`regalloc_bt.rs`/`emission.rs`/`use_list.rs`），生成器**暂不动**。
3. 验收：门禁全绿（这一步不该有任何行为变化）。

### 批次 B（生成器改发 operands 表）——✅ 已落地（2026-10-11）

**落地内容**：生成物**删掉** `uses`/`defs`/`use_constraints`/`def_constraints` 四个方法体
（批次 A 已证明它们与 `operands()` 在全部变体上等价），改由缺省实现派生。
`machine_shape_table.rs` 的 `TABLE_DRIVEN` 缩到 5 个，并新增回潮守卫
`derived_methods_are_not_emitted_any_more`（生成物里再出现那 4 个方法体即红）。

**守卫升级**：`operand_model_guard` 从"双边对拍"改为"派生 + 两条独立判据"——
① **顺序契约**（`Use` 必须在 `Def` 之前：regalloc 的 use@start / def@end 靠它）；
② **`is_move()` 交叉判据**（`is_move` 由另一张表 `move_arms` 驱动，与操作数表无关，
说搬移就必须恰好 1 个 `Def` 且 ≥1 个 `Use`）。实测覆盖
**x86 1550 / riscv64 624 / arm64 2581 = 4755 个指令变体**。

**仍未动的两处**（归批次 C，原因见下）：`reg_field`/`set_reg_field`/`is_reg_field_settable`
三个方法的发射保留——它们是**回写路径**（`compiler.rs`/`emission.rs`/`regalloc_bt.rs` 按
**字段号**调用），改成按**操作数号**调用要与 `xreg_map` 的删除同步做。

### 批次 B（原始设计，留档）

1. `dsl/codegen/machine.rs`：`__Shape` 的 `uses`/`defs`/`def_reuse` 合并成**按字段序**的
   `__OPERANDS` 表；发 `operands()`/`set_operand_reg()`，删 10 个方法（含 `reg_field`/
   `set_reg_field`/`is_reg_field_settable`）。
2. `gen_lowering` 每 arm 的 `map_reg_field` 调用全部删除（54 处里生成器的 42 处）。
3. `machine_shape_table.rs` 改钉 operands 表；`determinism` 必须仍"同 TOML 同产物"。
4. 风险：`__SLOT_CLASSES` 去重表与 `def_reuse` 的语义合并要**逐条对齐**（漏一条即 regalloc 拿到
   错的 use/def 顺序 ⇒ 活区间错 ⇒ 静默错值）。

### 批次 C 的前置验证——✅ 已落地（2026-10-11）

**加了两条对拍守卫（只加守卫、不删代码）**：

1. **发射点断言**（`InstPacket::map_reg_field`，`cfg(debug_assertions)`）：登记的
   `(field, is_def)` 必须能在该指令的 `operands()` 里找到同字段同角色的项——找不到即 panic
   并点出是第几条指令、哪个字段。**它证明 `is_def` 可由操作数表派生**。
2. **编译尾部全量对拍**（`CompileState::compile` 调 regalloc 之前）：按 vcode 块序逐条指令
   核对 `xreg_map` 与 `operands()` 的 `(field, role)`。注意只对拍形状面——旁表里是**虚拟
   寄存器**、字段里是**占位物理寄存器**（这正是前置条件要改的东西）。

**判据**：门禁 + 两矩阵 + 全部 codegen 单测（2722）全绿 ⇒ `is_def` 可删、旁表可从
`(vreg, field, is_def)` 缩到 `(vreg, field)`。

**剩下的机械活**（下一批，纯删除）：① 删 `map_reg_field` 的 `is_def` 形参（54 处调用点里的
42 处在生成器模板，改法是生成期就按形状表定角色）；② 让生成物的字段占位携带 vreg 号
（42 个 `Reg::from_index(0, cls)` → `Reg::from_index(<vreg>, cls)`），此后 `xreg_map` 才真正冗余；
③ 删 `xreg_map`/`inst_clobbers`/`ctx.xreg_types` → `CompileState.inst_ops`，`set_reg_field`
调用点改按操作数号走 `set_operand_reg`。

### 批次 C 的关键发现（2026-10-11，动手前必须量清）

**`is_def` 不是纯冗余**：生成期对每个 Reg 槽**只发一条** `map_reg_field`，角色取
`is_def = role ∈ {Out, InOut}`（`lowering.rs` 的 generic rule 发射路径）。也就是说
**InOut 槽在旁表里只登记成 `Def`（一条）**，但在形状表里该字段同时进 `uses` 与 `defs`
（InOut = 既读又写）。于是：

- 若"删掉 `is_def`、一律由 `operands()` 派生"，InOut 槽会从"仅 def"变成"use + def 各一条"
  —— 这是**行为变化**（对 x86 的 `add dst, src` 这类两操作数指令，regalloc 的活区间口径会变；
  方向上更像"修好"，但必须**先测量再改**，不能当纯重构顺手做）。
- 本轮的守卫只断言"旁表登记项在操作数表里有同字段同角色的对应项"（子集关系），因此
  它对上述差异**是盲的**——这正是它作为"能删"判据的**边界**，已写在这里以免误读。

**2026-10-11 实测修正（把上一段的推断纠正到位）**：加了"InOut 字段旁表条数"的哨兵断言后，
全量单测立刻把它打红——**InOut 字段的旁表条数可以是 0**：`e2e_compile_add` 里那条指令的
旁表是 `[(XReg(514), 0, false)]`，只登记了字段 0（use），字段 1（InOut 的 dst）**一条都没有**。
原因：`map_reg_field` 只登记**绑定到 vreg** 的槽，物理寄存器占位（生成期字面量 0）不参与映射。

因此结论要更硬一格：**旁表角色不是从形状表推导出来的**——同一 InOut 字段可能是 0 条
（物理槽）或 1 条 `Def`（vreg 槽），而形状表里它同时是 use 与 def。于是：

- **"删掉 `is_def`、一律按表派生角色"这条路关闭**（它会凭空给这些槽加出 use 项 ⇒ 改
  regalloc 活区间口径 = 行为变更）；
- 守卫保留两条：**子集关系**（旁表登记项必须在形状表里有同字段同角色的对应项）+ **哨兵**
  （InOut 字段旁表条数 ≤ 1，实测契约是 0 或 1）。哨兵的作用是"将来谁改旁表语义，这里会响"。

**修订后的批次 C 目标**（仍是方案要的"`CompileState` 只留 `inst_ops`"，但换一条可达路径）：
把**三张表**并成一张——① 旁表 `xreg_map`（vreg → 字段 + 角色，**角色来自 lowering，
保留 `is_def`/`OperandRole`**）；② `ctx.xreg_types`（vreg → IR 类型 ⇒ 回填用的宽度类，
实测写入点 ~26 处在 `pipeline/lowering.rs`、读取点 ~6 处在 `emission.rs`）；
③ 指令形状（字段/约束/`settable`/声明类，来自 `operands()`）。
`is_def: bool` 顺手升格为 `role: OperandRole`（类型化），语义不变。

### 批次 C（原始设计，留档）

1. `MachineInst` 删 `uses`/`defs`/`use_constraints`/`def_constraints`/`reg_field`/`set_reg_field`/
   `is_reg_field_settable`；`InstPacket` 只剩 `insts`/`inputs`/`outputs`/`xregs`（删 `xreg_map`/
   `map_reg_field`/`remap_internal`）。
2. `CompileState` 只留 `inst_ops: Vec<SmallVec<[InstOperand; 4]>>`（删 `xreg_map`/`inst_clobbers`/
   `ctx.xreg_types`）；liverange/regalloc/emission 统一读它，class 回写只留一份（修 R3）。
3. 约束表必须让 `ReuseInput` 真正可达（C5：要么接通、要么在 M5 明确删除——**不留半接通**）。
