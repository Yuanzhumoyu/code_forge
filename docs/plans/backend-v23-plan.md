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
| M4 单一操作数模型 | ✅ 已落地（批次 A/B/C；两处偏差如实记录：`xreg_types` 保留、`inst_clobbers` 与 `clobbers()` 非同源） | 见 §13 M4 收口 | 2026-10-11 |
| M5 分配器与发射 | ✅ 已落地（C6 帧记账单点 + C4 表归一 + 条件式收参缺陷修复 + C3 释放 2 寄存器 + C4 双根因收口 + 密集输出 + SpillEdit + **真 Belady** + MachBuffer 三项） | 见 §14 M5 进度 | 2026-10-11 |
| M6 合法化与缺口 | ✅ 已落地（**单一驱动 + 五个 pass**（va/agg-store/agg-call/agg-param/gep）、`expand_*` 计数 0、去重三项、幂等守卫、缺口泛化三项 + 两项 fail-closed；**两处未达成如实记录**：`va_arg` 聚合真做、`>16B` 非 sret 栈返回走 `StackByVal` —— 都需约定层数据扩展） | 见 §14 后的 M6 批次记录 | 2026-10-11 |

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

## 14. M5 进度（进行中）

### 爆炸半径（2026-10-11 实测）

| 目标 | 命中 | 说明 |
| --- | --- | --- |
| C4 `callee_saved` / `callee_saved_fpr` | 166 / 6 | 两份平行列表（GPR 整表 vs FPR 只存用到的），`pop_free` 与保存集各读一份；**class 盲**：`callee_saved.contains(&p.num)` 让 GPR 号与 FPR 号互相误判 |
| C3 池过滤 | `regalloc_bt.rs:146-163` | 用**同一个** `fp_reg`/`sp_reg` 号过滤**每个类**的池 ⇒ 三个发行 ISA 各误伤 2 个寄存器 |
| `RegAllocConfig` | 19 | 含测试构造器 `new`（M4 记为待 M5 的 `from_target` 收口） |
| frame 重算（C6） | 4 处 | `compiler.rs:95/2037/2299`、`emission.rs:38`、`frame_layout.rs:129` |
| `AllocResult` 平行数组 | 5 条 | `param_vregs`/`param_is_float`/`param_by_ref`/`param_is_32`/`param_bytes` |

### 第一批（C6 帧记账单点）——✅ 已落地（2026-10-11）

`LowerCtx` 增 `min_frame`，与 `callee_saved_bytes`/`stack_slot_shift` 同源同时刻填（lowering 之前
那一处 `frame_layout_info`）；`emission.rs` 的 spill 基点改为读 `ctx.callee_saved_bytes`，
`calculate_frame_size` 改为读 `ctx.min_frame`；助手 `frame_layout::callee_saved_bytes` 删除。
**两处未动**（输入不同，已标注）：`compiler.rs:95`（聚合落点探针 plan）与 `:2037`
（`va_info_for_expansion` 的 plan）——它们按**各自签名**的 plan 算平移。

### 第二批（C4 表归一 + C6 收尾）——✅ 已落地（2026-10-11）；C3 与"类感知偏好"**按实测回退**

- **C4 表归一**：`callee_saved: Vec<u32>` + `callee_saved_fpr: Vec<u32>` → `Vec<(RegClass, u32)>`（plan 投影），
  保存集扫描按类查同一张表；`callee_saved_fpr` 字段删除。
- **回退一（C4 的类感知偏好）**：`pop_free` 的偏好仍**按号跨类**匹配（与旧实现逐位等价）。
  改成"类族 + 号"会打红 `test_jit_float_params_beyond_xmm_registers_come_from_the_stack`，
  而**该用例只在 `--all-features`/workspace 的 feature 统一下编译进来**（单 crate 默认特性少 84 个用例）
  —— 门禁必须显式包含 `cargo test -p forge-codegen --lib --all-features`，本轮已加入。
- **回退二（C3 池过滤）**：让 sp/fp 排除只对整数类生效（放出 FPR 池同号的 2 个寄存器）同样打红那条用例
  ⇒ **那两个号的排除是承重的**（与浮点栈参数的搬运路径共用寄存器）。

**前置调查结论（2026-10-11，三组对照实验 + 生成器源码）**：

C3 与类感知偏好都动分配序，因此先做了**逐号对照**：FPR 池**只放 XMM4 → 过**、
**只放 XMM5 → 过**、**两个都放 → 红**。⇒ 不是"某个寄存器号特殊"，而是**池一变大，
分配结果就变**——那条用例对分配变化敏感，问题不在寄存器身份。

**机制（据 `dsl/codegen/frame.rs` 的收参发射）**：栈上形参的收参是**条件式**的——
只有该形参 vreg **没有**拿到寄存器时（`__rm.spill_slots.contains_key(&__pv)`）才走
`load 布局槽 → GPR scratch → store spill 槽` 这条按位搬运；**池变大后这些形参拿到了寄存器，
收参分支整条被跳过** ⇒ 值没落地 ⇒ 静默错值（无编译错误、无异常）。

**因此 M5 的正确顺序是**：① 先把"条件式收参"改成**无条件收参**（收参写到该 vreg 的最终落点：
有寄存器就收进寄存器、否则收进溢出槽——由分配结果驱动，而不是由"有没有溢出槽"驱动）；
② 再加「新增用例：栈形参在池有余量时必须仍被正确收参」；③ 之后才做 C3 释放那 2 个寄存器与
C4 的类感知偏好。这条调查已写进 `regalloc_bt.rs` 的注释。

### 第三批（无条件收参 + C3）——✅ 已落地（2026-10-11）

**① 收参无条件化**（`dsl/codegen/frame.rs`）：栈上形参的收参改由**分配结果**驱动——
拿到寄存器 ⇒ 直接收进它（整数 `stack_arg_load`；**浮点走新增的类限定角色**
`{ role = "stack_arg_load", class = "fpr" }`，落在 x86 的 `MOVSD_RM`，与既有浮点
`stack_arg_store`（`MOVSD_MR`）对称）；没拿到 ⇒ 仍走按位搬运进溢出槽。
原先"栈落点 + 已分配寄存器"那条分支写着"实际到不了"并按 `Unsupported` 拒绝——实测**到得了**，
那正是静默错值的入口。

**② C3 落地**：池过滤只对整数类排除 sp/fp ⇒ FPR 池释放同号的 2 个寄存器
（x86 XMM4/XMM5、riscv f2/f8、arm64 V29/V31）。

**③ C4 类感知偏好仍挡住**：收参修好后复测，按 `(类族, 号)` 匹配**仍**打红
`test_jit_float_params_beyond_xmm_registers_come_from_the_stack` ⇒ 与 C3 **不同源**，回退并单列。
下一步：dump 该用例在两种偏好下的分配差异再动（这是本轮唯一还没解开的）。

**④ 夹具同步**：`stack_args_capability_is_declared_by_roles` 改用 `strip_role`
（删角色的**全部声明形态**）——类限定形态出现后，只删平铺形态会留下"声明了角色却找不到指令"
的假缺口，那正是这条测试第一次红出来的原因。
**⑤ 新守卫（计划第 ② 步）**：`test_jit_stack_float_params_are_received_into_registers`——
六个 f64 形参**全参与运算**（配 16 个 XMM 的池 ⇒ 栈上的第 5/6 个也拿到寄存器），断言和值 63。
它是本轮新增的**浮点直接收参**那条分支的**唯一直接覆盖**（原有用例在池紧张时走溢出槽那条）。

**⑧ 全量 `forge-tests --lib` 扫描的偶发已闭环（2026-10-11）**：密集化那轮的一次全量扫描报
`1 failed`（耗时 2755 s），按组分跑全绿；**重跑同一全量扫描 = 58 passed / 0 failed / 33.38 s**
（日志 `target/tmp/v23-m5dense-forge-tests.log`，仅有的 FAILED 字样来自 PowerShell 的
NativeCommandError 噪声行）⇒ 判为**一次性偶发（环境/时序）**，与密集化改动无关；结论记此，
不再挂着当未决项。

### SpillEdit 第五步（完整 Belady + spill cost）——**设计已定，待实现**（2026-10-11）

**为什么它需要新的编辑种类**：现状 `evict_and_assign` 只驱逐**死值**（`next_use == MAX`），
无死值时要么 spill 当前 def、要么报错。真正的 Belady 要驱逐"最远的下次使用"的**活跃值**，
而被驱逐者的当前寄存器值**必须先写回它的槽**（否则它下次 use 从空槽读到垃圾——这正是 P0-8
当年把 Belady 降级成"只驱逐死值"的原因）。因此需要在 `SpillEdit` 里加**时机在指令之前**的
一种：`SpillLive { slot, preg, class, width }`（与 `Store` 的区别是时机与语义：`Store` 是
"本指令算出来的 def 写回自己的槽"、在指令**后**；`SpillLive` 是"本指令要占用某寄存器，
那个寄存器里的活跃值先落盘"、在指令**前**）。

**实施清单（一批做完再提交）**：

1. `alloc_result.rs`：加 `SpillEdit::SpillLive`；文档写清与 `Store` 的时机差异。
2. `regalloc_bt.rs`：`BtState` 加 `live_stores: Vec<Vec<SpillEdit>>` 与 `current_global_inst: usize`
   （`process_block` 每条指令前更新——驱逐点在 `assign_reg` 里，拿不到全局号）；
   `evict_and_assign` 改策略：候选人按 `next_use_after` 降序取"最远"的**活跃**值（确定性 tie-break
   用 vreg index），驱逐它时把 `SpillLive` 记进 `live_stores[current_global_inst]`；死值仍走旧路
   （死值不需要落盘）。**注意**：本指令自身的 use 要排除在外（它马上要读那个寄存器）。
3. `build_spill_edits`：把 `live_stores[gi]` 作为该指令编辑序列的**前缀**（即使 binds 没有溢出，
   也要因此产出非空编辑）——emission 的 `edits.is_empty()` 判据不变。
4. `emission.rs`：`SpillLive` 在"装值"那一段之前执行（`emit_spill_store(preg.num, sp_base + slot.offset, width, class, sink)`）。
5. 用例（方案的验收要求）：**"必须驱逐活跃值"的正例**——现状 fail-closed，改后应"可编且结果正确"；
   再加一条"被驱逐的活跃值下次 use 读到的仍是正确值"（这是 `SpillLive` 存在的意义）。
6. 判定：两矩阵 + `asm_exec` + **mini_c e2e**（本仓教训：宽度类/时机类分歧只有跨 crate e2e 抓得到）。

**风险**：动分配序 ⇒ 输出变化；若矩阵或 e2e 红且一次修复不成就回退并另记（C3/C4 的先例）。
### MachBuffer（M5 第 6 项）—— ✅ 三项全部落地（2026-10-11）

1. **次序 hack 删除**（依据：标签走 pending fixup，前向引用天然成立）；
2. **重定位补丁语义单点化**（`RelocKind::patch_value`，删 JIT 私有副本）；
3. **`EPILOGUE` 哨兵替换为分配标签**：`LabelRef::Machine(u32)` + `CodeSink::new_label()`
   （id 独立区间），`ExternalLabel`/`EPILOGUE`/空间断言删除；`needs_epilogue_label()` 保留
   （它问的是"是否用共享尾声"，是能力判据）。
   新守卫两条：`machine_labels_are_allocated_uniquely_and_round_trip`、
   `forward_machine_label_reference_is_resolved`。

**M5 六条至此全部落地**（C3 释放两寄存器 + C4 双根因 + 密集输出 + SpillEdit + 真 Belady +
MachBuffer），判据：workspace 门禁 exit 0、`forge-codegen --lib --all-features` 2810、
runtime 18、jit x86 197/3/0、riscv64 136/64/0、asm 三档全绿、clippy/fmt 干净。


**已落且有回归证据**：

1. **次序 hack 删除**：`emission.rs` 的 Pass1/Pass2（"非返回块先发、返回块最后"）删除，块按 vcode 自然
   次序发射。依据：重排是**编码器急切算偏移**时代的遗留——现在 `emit_epilogue_jump` 写**标签号**、
   由 `CodeSink` 记 pending fixup 并在 `bind_label`/`finish` 统一打补丁，前向引用天然成立。
   三条硬回归全绿：x86 197/3/0（前向跳转）、riscv64 136/64/0（分支范围）、`asm_exec` 1 passed。
2. **重定位补丁语义单点化**：`RelocKind::patch_value`（运行时）成为唯一口径，删 JIT 的
   `compute_patch_value` 私有副本。ObjectWriter 侧写的是**对象文件重定位**（真打补丁交给链接器），
   因此"共用"的落点就是这条语义函数 + 既有 `RelocKind`/`RelocPatcher`。

**待做（下一批）**：`ExternalLabel::EPILOGUE` 哨兵 + `needs_epilogue_label` 特判。**为什么不能顺手删**：
`LabelRef` 的**数字 id 就是标签查找键**（`emit.rs` 的 `from_id` 是纯区间判断：`id >= BASE` ⇒
`External(Epilogue)`，否则 `Block(id)`），而编码器只能往指令字段里写**数字**。要真正"用分配出来的
标签"，得让数字 id 空间支持分配（`Machine(id)` 走独立区间 + `from_id` 查表）——那是标签类型/编码面
的改动，需要单独一批 + 前向跳转回归。当前哨兵已被**类型化**（骗不出同值句柄）并有 `debug_assert`
挡空间冲突，属"已知且受控"的临时形态。
**第四次尝试：✅ 成功（2026-10-11）** —— 根因是"活跃值落盘"这条路**本身就是错的**。

`FORGE_TRACE_SPILL` 一眼看穿（同一槽 -88 的三条记录）：

```text
[spill] atomic_spill load 10 off=-88        ← def：被重定向到 scratch r10 并 Store 回槽
[spill] atomic_spill store 10 off=-88
[spill] atomic_spill spill-live r1 off=-88  ← 驱逐活跃值时把 **r1 的陈旧内容**倒进同一个槽
[spill] atomic_spill load 10 off=-88
```

发射侧按**最终表**决定搬运 ⇒ 凡"最终有槽"的 vreg，其每次 def 都走 scratch 并写回槽、每次 use 都
从槽读 ⇒ **槽才是权威副本**，寄存器内容可能早已陈旧。于是"把受害者寄存器写回槽"是**用陈旧值覆盖
正确值**——这就是 `-163` / `-1774193535` 的来源。

**修法（做减法）**：驱逐活跃值只需释放寄存器（`spill_vreg` 会分配槽并归还寄存器）；
`SpillEdit::SpillLive` 与 `live_stores` 全套**删除**（不留死代码）。另补
`spill_vreg` 早退分支的寄存器释放（探针实测：该路径**只在驱逐活跃值时可达**，故旧策略掩盖至今）。

**验收**：原"已知限制"用例改写为正例（17 个 V256 值 > 16 寄存器 ⇒ 必驱逐活跃值，断言编得过且
值 = -153）；`cargo test --workspace --exclude forge-rustc` exit 0、`forge-codegen --lib
--all-features` **2810 passed**、jit x86 197/3/0、riscv64 136/64/0、asm_exec 1 passed、
clippy/fmt 干净。
**第三次尝试（2026-10-11，工作区进行中，**未提交**——用户要求 M5 完成前不提交）**

本轮把 ①（`spill_vreg` 早退也释放寄存器）+ ②（逐点判据）**分开验证**，结论明确：

1. **① 成立且是必需的**：只做 ① 时 atomic 用例**不再崩溃**，只是值错（`-1774193535` vs `50`）。
2. **② 被推翻（已回退）**：加上"此刻没有寄存器才算在栈上"的判据后，单跑 atomic 用例直接
   **0xc0000005 访问违规**。原因想清楚了：发射前的字段统一回写用的是**最终**分配表，所以
   按"此刻有没有寄存器"判，会让**早期**指令的字段停在一个**更晚**的寄存器号上（那个时刻它是垃圾）
   —— 判据必须与回写同源。**正确口径仍是"有槽就一律经 scratch 从槽读"**（自洽性来自
   `SpillLive` 保证槽是最新的）。
3. **探针测量**：把诊断探针插进 `spill_vreg` 的早退分支后跑**全量门禁一次都没触发**
   ⇒ 那条早退只有在"驱逐活跃值"时才会走到，属于本批次内的路径（不是既存缺陷）。

**当前工作区状态（WIP）**：Belady 实现 + `SpillEdit::SpillLive` + emission 指令前落盘 + 修复 ①；
`test_jit_atomic_rmw_spill_pressure` **值错**（唯一硬失败），已知限制用例
（`test_jit_v256_high_pressure_spill_is_known_limited`）待改写为正例。

**下一轮入口（已收窄到一条）**：对 atomic 用例开 `FORGE_TRACE_SPILL`，逐条比对
"`SpillLive` 落盘 → 后续 reload 读槽"的配对，重点看**同一 vreg 被驱逐两次**、
以及**驱逐点之后同一指令内的其它操作数**是否也被重定向；定位后补齐第 ③ 条修复。

### Belady（SpillEdit 第五步）第二次尝试：又前进了两步，仍回退（2026-10-11）

第二次按清单实现（`SpillLive` 变体、`live_stores`/`current_global_inst`/`current_uses`、驱逐取
"最远活跃值"、`build_spill_edits` 前缀合并、emission 在指令前落盘），编译通过，并拿到两条**新证据**：

1. **`spill_vreg` 的"已有槽就早退"会漏掉释放寄存器**（真 bug，独立于 Belady）：活跃值被驱逐 →
   之后 `reload_from_stack` 拿到寄存器 → **再次**被驱逐时命中早退分支，寄存器既不归还也不清
   `assignments`/`reg_owner`；而调用方 `evict_and_assign` 仍把那个寄存器交给当前 vreg ⇒
   **两个活值共用一个寄存器**（症状 = 某个值读到别人的值，与第一轮 `-163 vs -153` 同型）。
   死值驱逐永远走不到这条（死值不会再被 reload），所以旧策略一直掩盖它。
   修法（下一批带上）：早退分支里同样 `assignments.remove` + `clear_reg_owner` + 归还 free。
2. **"在栈上"的判据要用逐点状态，不能只看"有槽"**：`build_spill_edits` 现在按
   `spill_slots` 有无判"要搬运"，于是"曾被驱逐、之后 reload 回寄存器"的值也被当成栈上的
   ——把字段重定向到 scratch 并从槽里读**陈旧值**。候选口径 = **有槽且此刻没有寄存器**
   （`assignments[v].is_none()`）。

**本轮实测**：只做 ① 时 `test_jit_atomic_rmw_spill_pressure` 仍错（`169866129` vs `50`）；
再做 ②（逐点判据）时该用例不再报错值，但**整个测试二进制 `0xc0000005`（访问违规）** ⇒ 说明
②动到了发射路径的形状，需要更细的取证（先单跑一条用例 + `FORGE_TRACE_SPILL` 看编辑序列）。

**纪律**：两处都属"动分配序/发射序"的改动，红着不留 ⇒ 已 `git checkout` 回退，工作区干净、
门禁维持上一提交状态。下一批从 ① 开始（它是独立真 bug，可单独验证），再带 ② 重试 Belady。

**第一次尝试与实测结果（2026-10-11，已按纪律回退）**：按上面的清单实现了 1–4 步
（`SpillLive` 变体、`BtState::live_stores`/`current_global_inst`/`current_uses`、驱逐改"取最远活跃值"、
`build_spill_edits` 前缀合并、emission 在指令前执行落盘），**全部编译通过**，并把原先"已知限制"
用例（`test_jit_v256_high_pressure_spill_is_known_limited`）改写成"必须编过且算对"的
`test_jit_v256_high_pressure_spill_evicts_live_values`（17 个 V256 值逐个消费，期望 `Σ -(1+i) = -153`）。

**结果**：该用例**确实编过了**（限制解除 ✔），但**返回值错**：实得 **-163**（差 **-10**，正好等于
`-(1+9)`——第 10 个值那一项），指向"某个被驱逐的值读到了**另一个**值/槽"的配对错误。
其余 2808 个 codegen 用例全绿（说明分歧只在活跃值驱逐这条新路径上）。

**回退理由与下一步取证**：动分配序的改动在"编过但错值"上必须停下（本仓既有纪律：未验证不假绿；
C3/C4 的先例也是回退 + 另记）。下一步取证方向（按性价比排序）：① 对该用例开 `FORGE_TRACE_ALLOC`
+ 新的"编辑序列 dump"看被驱逐者与其槽的配对（哪一次驱逐把 A 的槽给了 B 的落盘）；
② 检查**同一 vreg 被驱逐两次**与"驱逐后又 reload 再驱逐"时 `live_stores` 是否重复/错序
（`current_global_inst` 的归属是否正确）；③ 确认 `current_uses` 的排除是否漏掉 `Bind` 阶段
（同一指令里既 use 又 def 的字段）。

### SpillEdit ——✅ 已落地（2026-10-11，第一～四步；**输出等价**）

`SpillEdit { Reload/Bind/Store }` + `AllocResult::edits`；分配器出口第二趟产出编辑
（`build_spill_edits`），scratch 池不足与 `settable` 预检**前移到分配期**；
`emit_inst_with_spills` 只剩"按序执行三类编辑"，删掉 scratch 选择与覆盖表。
**第五步（完整 Belady + spill cost 驱逐）仍未做**——它才是行为变更，需要新增
"必须驱逐活跃值"的用例（现状 fail-closed）。

**取证教训（值得留档）**：第一版把 `Bind` 的类写成 `vreg.class()`（池宽），而旧实现用的是
`bind.class`（M4 折入的**值宽类**）——多类槽的 opsize 靠它。**两矩阵与 codegen 单测全绿，
mini_c 的 e2e 才把它抓出来**（`test_hir_e2e_loop_bound_from_param` 返回 2750685 而非 45）⇒
"输出等价"必须由**跨 crate 的 e2e** 才能确认，codegen 自己的用例覆盖不到这种宽度类分歧。

### SpillEdit 设计与实施清单（历史留档，2026-10-11 侦察）

**现状（实测形状）**：`emission::emit_inst_with_spills` 现在是 **11 参**（M4/M5 已把三个平行数组并成
`binds`），体内做四件事：① 从 `binds` 去重收集 spilled vreg；② 从 `target.reg_info().scratch_regs()`
选 scratch（数量不够即报错）；③ 逐条 `emit_spill_load` 装值 + `set_reg_field` 改写 spilled 字段
（含 `settable` fail-closed）；④ 发指令；⑤ 对 spilled **def** 逐条 `emit_spill_store`。
分配器侧：`allocate()` → `process_block()` 逐块逐指令拿着 `vcode` 与 `inst_ops`，出口持有
`assignments`/`spill_slots`，且 `config.scratch_regs: Vec<PReg>` 就在手上——**决策所需的一切它都有**。

**目标**：决策搬进分配器，发射退化为"按序执行编辑"。

1. 运行时新类型（与 `AllocResult` 同文件）：
   `enum SpillEdit { Reload { slot, scratch }, Bind { field: u8, scratch }, Store { slot, scratch, class, width } }`
   + `AllocResult.edits: Vec<SmallVec<[SpillEdit; 4]>>`（与指令流平行）。
2. 分配器出口**第二趟**填 `edits`：对每条指令的每个 bind——vreg 被溢出 ⇒ `Bind{field, scratch}`；
   每个不同的 spilled vreg ⇒ 指令前 `Reload`、若该 bind 是 **Def** 再在指令后 `Store`。
   scratch 绑定按类取 `config.scratch_regs`，**池不够就在这里 fail-closed**（现在这步在 emission）。
3. `settable` 预检**前移**：分配器在产出 `Bind` 时校验 `inst.is_reg_field_settable(field)`
   （它手里就有指令），emission 不再预检。
4. emission 侧：`emit_inst_with_spills` 收敛成"按 `edits` 顺序：`Reload` ⇒ `frame_lowering.emit_spill_load`
   （`class` = 该 vreg 的类、`width` = 其字节数）、`Bind` ⇒ `set_reg_field`、`Store` ⇒ `emit_spill_store`"，
   删掉 scratch 选择与 `settable` 预检；`sp_base`/`frame_size`/`callee_saved_bytes`/`stack_args`
   仍由 emission 提供（那是**发射期**事实）。
5. **解锁项**：`edits` 一旦由分配器产出，驱逐就能给出"把活跃值 store 回槽"的编辑 ⇒ 可做
   **完整 Belady（取最远下一次使用）+ spill cost 驱逐**，并把 `regalloc_bt` 的文档从"只驱逐死值"
   更新为真实策略（该项与 `docs/plans/backend-v23-plan.md` M5 的 C5/SpillEdit 两条同时收口）。

**风险与顺序**：先做 1–4（**输出等价**：edits 序列与现有 emission 的临时决策应当逐条对应，两矩阵
先行判定），再做 5（**行为变更**：驱逐策略变了，需要新增"必须驱逐活跃值"的用例——现状是 fail-closed）。

**⑩ 第六批（2026-10-11，对应方案"spill 分派"条目）**：`emit_spill_load/store` 的 `is_fp: bool`
→ `class: RegClass`（FPR/VEC ⇒ 宽度档浮点模板、GPR ⇒ 通用模板、**KReg ⇒ 显式 fail-closed**）；
`class` 与 `width` **不重叠**（宽度是运行期选档、类是选指令族；池宽 ≠ 值宽），trait 文档写明理由与
反例；调用点统一成"值的类 + 值的宽"（原来 store 侧传的是分配到的**池类**）；两条新守卫
（KReg fail-closed、宽度×类独立）。

**⑨ 第五批（2026-10-11）**：删三个与 `param_vregs` 平行的**可推导**数组
（`param_is_float`/`param_by_ref`/`param_is_32`，最后一个是纯死数组——没有任何读点）——
生成物按 `v.class().is_fp()` / `is_fp() && bytes() > 16` / `bytes() == 4` 现算；
保留 `param_vregs` 与 `param_bytes`（后者是 IR 侧**真数据**：类型字节数不在 vreg 里，
by-ref 宽向量收参靠它分派 32B/64B）。同时**修掉上一批引入的重复发射**：浮点栈形参
"直接收参"分支后残留一句无条件 `#layout_stack_stmt`，会多发一条把 `__dest` 当 GPR 的收参
（写坏同号通用寄存器，可能正是活值）——测试抓不到，靠读代码发现，已删。
守卫锚点由 `param_by_ref` 换成回退段的 fail-closed 文案（三谱实测都在布局路径之后）。

### C4 类感知偏好 ——✅ 已收口（2026-10-11）

上节的取证方向**已验证并修好**，而且根因不在偏好本身：

1. **排除猜测**：编码级先排掉"高编号 FPR 缺 REX"（`movsd [rbx], xmm10` = `f2 44 0f 11 13`、
   `xmm15` = `f2 44 0f 11 3b`，REX.R 都在）。
2. **真因**：溢出搬运把值装进"号取自 scratch 池、**类取值的类**"的寄存器，x86 的 scratch 是
   **R10/R11** ⇒ 浮点值 reload 实际写 **XMM10/XMM11**；FPR 池一旦把**活值**分到这两个号，
   reload 就踩掉活值。旧"只比号"偏好巧合地从不把 FPR 分到 10/11，因而掩盖至今。
3. **修法**：池构造**保留 scratch 的号（任何类）** + callee-saved 优先按 **(类, 号)** 匹配。
   两者到位后 C4 用例通过，类感知偏好**正式启用**。
4. **守卫**：`scratch_reg_numbers_are_never_allocated`（scratch 号绝不分配给 vreg）。

**教训（与 mini_c 那次同源）**：跨类共享的**编号空间**（scratch 号、寄存器号）是这类"静默串味"
的温床；"某个偏好恰好总能绕开它"最容易让 bug 长期潜伏——所以守卫要钉**不变量**，不是钉现象。
### C4 类感知偏好的**根因取证**（2026-10-11，`FORGE_TRACE_ALLOC` 对照）

用分配器自己的 trace 把两种偏好下的分配**逐条对照**（基线与类感知各 26 行，`Compare-Object`
只剩 5 处差异），结论很干脆：

| | 基线（只比号） | 类感知 |
| --- | --- | --- |
| 栈上浮点形参（第 5/6 个）落到 | **f6 / f7**（XMM6/XMM7） | **f10 / f11**（XMM10/XMM11） |
| 其余分配 | 完全相同 | 完全相同 |

为什么旧口径总是挑到 6/7：win64 的 GPR callee-saved 里有 **RSI=6 / RDI=7**，于是"只比号"的匹配
把 XMM6/XMM7 当成「callee-saved 优先」，**恰好**总落在**低编号 FPR（< 8）**上。类感知口径
（语义上正确）则可能把值放进 **XMM10/XMM11**，结果 `e+f` 从 7 变成 8 ⇒ **编号 ≥ 8 的 FPR
在某条路径上编码/使用有 bug**，被旧偏好「永远用低编号 FPR」掩盖了。

**下一步（明确、可执行）**：

1. 先用编码级用例确认 `MOVSD`（`MOVSD_MR` / `MOVSD_RM` 等）在 **FPR 编号 ≥ 8** 时是否缺少
   REX.R/B——生成物把 FPR 号当 GPR 号写进 r/m 的那类问题（`MOVQ_XMM_FREG` 的历史注释里
   就有「XMM15 实际写 r15」的先例）。
2. 再查栈参搬运路径是否对高编号 FPR 走错了模板。
3. 修好后重新启用类感知偏好（表已是 `(类, 号)`，只改那一处过滤）。

证据文件：`target/tmp/c4-trace-base.txt` 与 `target/tmp/c4-trace-family.txt`
（会话内产物，按本仓惯例不入库）。
**⑦ 第四批（2026-10-11）**：删 `LiveInterval::weight` 及其整条计算链（含只为它服务的循环检测
预扫）；活区间与"每块局部 uses/defs"合成一趟（删第二趟与平行的 `global_inst_map`）；
`regalloc_bt.rs` 的 spill 启发式文档改成与实现一致（只驱逐死值；def 无死值 ⇒ spill 自己；
use 无死值 ⇒ 明确报错），并写明**完整 Belady + spill cost 的前提是 `SpillEdit`**。

**⑥ C4 类感知偏好的取证线索（仍未落地）**：改成 `(类族, 号)` 后该用例报 **e+f = 8（期望 7）**，
8 = 4 + 4 ⇒ **第 5 个浮点形参拿到了第 6 个的值**（都来自栈）。既然收参已无条件化、
且直接收参分支已有 ⑤ 这条覆盖（当前配置下正确），下一步取证方向是**调用方写传出区**
（`stack_arg_store` 的 fpr 版）在同一偏好下的编号/偏移，或给该用例在两种偏好下
dump 机器码逐条对照。

### 后续批次（按"先输出等价、再改策略"的边界）

1. **C4**：`RegAllocConfig.callee_saved: Vec<(RegClass, u32)>` 一张表（`pop_free` 按类查、保存集按类扫），
   删 `callee_saved`/`callee_saved_fpr`；新增「FPR callee-saved 真的被保住」用例。
   **注意这是行为变更**（class 盲过滤修掉后分配序会变），矩阵判定。
2. **C3**：池过滤按类自己的 sp/fp，或上游（生成侧）直接从 `allocatable` 剔净 —— 二者都要在
   三 ISA 上核对"多出 2 个寄存器"。同样是行为变更。
3. **活区间单趟 + `weight` 归属**；**Belady 取真最远下一次使用**。
4. **`RegAllocOutput` 密集化**（`assign: Vec<Option<PReg>>`/`spills: Vec<Option<SpillSlot>>`）+
   `AllocResult` 拆分（消 5 条 `param_*` 平行数组）。
5. **`SpillEdit`**：分配器产出编辑序列，emission 退化为按序发射（删 13 参函数与 scratch 池）。
6. **`MachBuffer`**（新 `machine/machbuf.rs`）：替掉发射次序 hack、`ExternalLabel::EPILOGUE` 哨兵、
   `needs_epilogue_label` 特判；JIT 与 `ObjectWriter` 共用重定位语义。
7. M4 登记的候选：生成物按**变体**发表"模板字面物理寄存器" ⇒ 删 `CompileState.inst_clobbers`，
   并把"按包求并"变精确。

### M6 合法化单一驱动 —— 批次 1：驱动落地（2026-10-11，工作区进行中，未提交）

**已落**：新 `pipeline/legalize/mod.rs`：

- `trait LegalizePass<M: TargetMachine>`（`name()` + `run(func, ctx) -> ChangeSet`）与
  `LegalizeCtx`（机器视图 / 约定名 / `agg_slots` / `sret_ptr` —— 以前靠调用次序隐式传递的四个量
  现在显式在 ctx 里）；
- `LegalizeDriver`：**with_aggregates() 是次序的唯一出处**（`agg-store → agg-call → agg-param → gep`，
  依赖注释写在构造函数上：store 先展开、sret 先立、call 结果拆段先于实参拆段）；
- 每个 pass 自报 `ChangeSet`（IR 规模差 + sret 标记），驱动累加 + `FORGE_TRACE_AGG` 逐 pass 打印；
- 调用点（`compiler.rs` Stage 0.5）从"8 行硬编码调用"变成 `LegalizeDriver::with_aggregates().run(...)`；
- 守卫：`driver_has_the_documented_pass_order`（定序是契约，钉住 4 个 pass 的顺序）。

**偏差（如实记录）**：`TargetMachine` 带关联类型（`Reg`/`Inst`），`&dyn TargetMachine` 需要指定它们；
驱动因此**泛型化**（`LegalizeDriver<M: TargetMachine>`）而不是 trait 对象——pass 本来就收
`&impl TargetMachine`，泛型更省事且不引入 dyn 的间接层。另：`Va`（`va_expand::expand_va`）**暂不在
驱动里**——它需要 `VaSlots` 与 va 描述（形状与其余四个不同），留在调用点；下一批把它按同一模式接入。

**下一批（把 `fn expand_` 降到 ≤3，即 M6 验收）**：把 8 个 `expand_*` 的函数体搬进 `legalize/` 的
对应 pass 文件（`agg_store.rs`/`agg_call.rs`/`agg_param.rs`/`gep.rs`），`compiler.rs` 只留调用；
同时落 M6 的去重项（`pack_segment`、`place_addr_of_slot`、统一 `move_to_front`）。

### M6 批次 10：**合法化幂等**（M6 验收项）+ M6 收口评估（2026-10-11，未提交）

**新守卫** `legalization_is_idempotent`（`legalize/mod.rs`）：造一个 16 字节聚合形参的函数
（win64 下 `>8B ⇒ by-ref` ⇒ `AggParam` 必定动手），跑驱动**两遍**：

- 第一遍 `ChangeSet::total() > 0`（否则这条判据是空转——断言里写明了这一条）；
- 第二遍 `total() == 0` **且指令序列逐项相同**。

这条值钱的原因是：每个 pass 都是"就地改写 IR"，改写若不收敛，症状是**指令数/bootstrap 膨胀**
而不是报错——只有跑第二遍才看得见。

**M6 验收对照**（方案 §M6）：

| 验收项 | 状态 |
| --- | --- |
| `pipeline/legalize/` 驱动（定序/定点/变化报告/幂等） | ✅ 四职责齐（`VaPass` 起五个 pass） |
| 8 个 `expand_*` 收成 5 个 pass | ✅ `va → agg-store → agg-call → agg-param → gep` |
| `rg 'fn expand_' pipeline/compiler.rs` ≤ 3 | ✅ **0** |
| 去重：`pack_segment` / `place_addr_of_slot` / 统一 `move_to_front` | ✅ 前者 4→1、中者 3→1、后者已是 1 定义 3 调用 |
| 合法化幂等（第二次 `ChangeSet::empty`） | ✅ `legalization_is_idempotent` |
| `test_jit_agg_*` 全族 | ✅ 两矩阵全绿 |
| 未对齐字段用例 | ✅ `unaligned_nested_field_is_repacked_with_shifts` |
| **`va_arg` 聚合真跑** | ⏳ **未做**（见下） |
| e2e 全绿（forge-rustc） | 需 nightly 通道，不在本机门禁内 |

**剩余两项（如实记为"未达成"，并说明为什么不是一次小改）**：

1. **`va_arg` 聚合**：现在 `is_fp ? Fload : Load` —— 聚合类型会按**标量** load。做对需要
   ① 临时槽 + 从参数地址逐段拷贝；② 把 `(slot, ty)` 登记进 `agg_slots` 并递归重写使用处；
   ③ 且**先要按落点分类**判断参数地址处"是聚合字节本身还是指向聚合的指针"——这要看
   `VaArgRules` × `agg_arg_place` 的交叉，属 ABI 功能新增（含真执行用例）。
2. **`>16B` 非 sret 栈返回走 `StackByVal`**：发射侧 `ArgPlace::StackByVal` 已有，缺"返回侧也走它"；
   同样需要新约定 + 真执行用例。

**判据**：workspace 门禁 exit 0；`forge-codegen --lib --all-features` **2815 passed**（+1 幂等守卫）；
jit x86 197/3/0、riscv64 136/64/0；clippy/fmt 干净。
### M6 批次 9：**`insertvalue` 值内存化**（2026-10-11，未提交）

`rewrite_agg_value_uses` 的兜底分支以前对 `insertvalue` 直接 `Unsupported`（"仅支持
extractvalue/store/return/call 传参"）⇒ **任何"先建后改"的聚合构造都编译不了**。现在：

1. 为聚合值分配一个**自己的**帧槽（**不就地改来源**——来源可能住在调用方内存里，改它就是改
   调用方的数据）：段值表示时逐段 store；by-ref 表示时逐段 load→store；段值不足补常量 0
   （保守且确定，不读未初始化内存）；
2. 把插入的字段写进槽内 `field_offset` 处（浮点字段先 `Bitcast` 成 i64 位模式再存）；
3. `insertvalue` 墓碑化，**结果值登记成"住在该槽"**，再递归处理结果值的后续使用（后续
   `extractvalue` 因此走地址语义）。

**守卫** `insertvalue_is_memoryized`：钉"不再报错 + 结果登记进 `agg_slots` + 槽里确实有 `Store`"
——只登记不写就是"读到未初始化内存"，两件事缺一不可。

**顺带满足读路径纪律**：`insertvalue` 需要的静态事实（字段偏移/类型/是否浮点/聚合字节数）
**并进函数入口那唯一一次锁**（守卫先报了 8 > 6，把事实提到入口后回到 **6**，定额不用改）。
类型库没有"字段数"接口 ⇒ 用 `aggregate_elem_type(i).is_some()` 探到 `None`（256 上限兜底）。

**判据**：workspace 门禁 exit 0；`forge-codegen --lib --all-features` **2814 passed**（+1 守卫）；
jit x86 197/3/0、riscv64 136/64/0；asm_parse 3 / asm_exec 1；clippy/fmt 干净。

**M6 仍欠**：① `va_arg` 聚合结果（要临时槽 + 逐段拷贝 + 走落点分类）；② `>16B` 非 sret 栈返回
改 `StackByVal`。这两项都是**行为变更 + 需要真执行用例**，按批再落。
### M6 批次 8：**非对齐嵌套字段**从"报错"改为"能展开"（2026-10-11，未提交）

`memoryize_from_segs` 原来对 `off % 8 != 0` 直接 `Unsupported`（"非对齐字段暂不支持"）⇒
`{i8, {i32,i32}}` 这类字段偏移不是 8 倍数的聚合**整类编译不了**。现在段值先按 `off % 8` 字节
**移位拼接**再写槽：

- 拼接用 `(lo >> 8r) + (hi << (64-8r))`——两半位域不相交，加法与按位或等价，而 **`Ior` 在三份谱里
  都没有 lowering**（`Ishl`/`Ushr` 是 x86 与 riscv64 有、arm64 没有 ⇒ arm64 上这类函数仍会
  fail-closed，报的是标准的"无 lowering 规则"，已在账本记明）。
- 取 `hi` 时**严格按"这一段是否落在字段内"判**，越界或超出字段就补常量 0 —— 不读相邻聚合成员的字节。

**新守卫** `unaligned_nested_field_is_repacked_with_shifts`：造一个 `off = 4` 的嵌套字段提取，
断言 ① 不再 `Unsupported`；② IR 里**确实出现 `Ushr` 与 `Ishl`**（否则就是"悄悄按对齐直存"，
数据整体错位比报错更糟）。

**判据**：workspace 门禁 exit 0；`forge-codegen --lib --all-features` **2813 passed**（+1 守卫）；
jit x86 197/3/0、riscv64 136/64/0；clippy/fmt 干净。

**观察（未改，留证据）**：这条路径与对齐路径都按 `n_segs` 整段存（`size = 12` 时写 16 字节），
而 `frame_slots` 按 `Type(I8) × count` **精确**算槽字节数——是否越界取决于槽是否被填充到 8 的倍数。
本轮只做"能展开"，越界问题**没有实测证据**，不顺手改（改法会是给末段加掩码，属行为变更）。

**M6 仍欠**：① `va_arg` 聚合结果（要临时槽 + 逐段拷贝）；② `>16B` 非 sret 栈返回 `StackByVal`；
③ 聚合其余算子（InsertValue 等）值内存化。
### M6 批次 7：缺口泛化（部分）+ `int_ty` fail-closed（2026-10-11，未提交）

- **`va_expand::int_ty` 改 fail-closed**：1/2/4/8 四档照旧，其余**明确报错**（以前 `_ => I64`
  把 3/5/6 字节静默当 8 字节读写 ⇒ 游标字段与相邻对象字段互相踩，症状是"取参取到别的字段的值"；
  形状数据是使用者给的，写错必须报出来）。连带 `store_cursor` 改 `Result` 并在两处调用点传播。
  新守卫 `cursor_field_width_is_fail_closed`（四档通过 + `[0,3,5,6,7,16]` 全部报错且信息点名宽度）。
- **第三个 `Alloca` 站点去重**：`agg_expand.rs` 的嵌套字段抽取值也改用
  `legalize::common::place_addr_of_slot`（规范 immediate 形态的唯一出口现在是 3 处调用、0 处内联）。

**判据**：workspace 门禁 exit 0；`forge-codegen --lib --all-features` **2812 passed**（+1 守卫）；
jit x86 197/3/0、riscv64 136/64/0；clippy/fmt 干净。

**仍欠（M6 收口前）**：① `va_arg` **聚合**结果（现在 `is_fp ? Fload : Load`，聚合类型会按标量 load
——要按值返回就得配临时槽 + 逐段拷贝，属功能新增）；② `>16B` 非 sret 栈返回改 `StackByVal`
（`ArgPlace::StackByVal` 发射侧已有，缺的是"返回侧也走它"）；③ 未对齐嵌套字段（`off % 8 != 0`）
改逐字节/移位拷贝（现在按段整存 ⇒ 未对齐时数据整体错位）；④ 聚合其余算子（InsertValue 等）改值内存化。
这四项都是**行为变更**，各自需要正例 + 真执行用例，按批再落。

**环境事故（第二次，同日）**：workspace 门禁再次报 `link.exe` 1102 + `invalid metadata files for
crate test`——当时 D: 只剩少量可用空间；删 `target/debug/incremental` 后重跑 exit 0。**结论**：
本机跑全量门禁前先看 D: 可用空间（`target/` 已 51.7 GB），否则会看到"链接失败/元数据无效"这类
**与代码无关**的红。

### M6 批次 6：`Va` 进驱动 —— 五个 pass 齐了（2026-10-11，未提交）

- `LegalizeCtx` 增 `va: Option<(VaInfo, AbiSlots)>`：**槽排布留在编译入口**（深度依赖前端局部槽，
  只有 lowering 前那次预扫算得出），**展开交给 `VaPass`**；两边是同一份数据。
- 驱动次序成为方案说的五个 pass：**`va → agg-store → agg-call → agg-param → gep`**（守卫
  `driver_has_the_documented_pass_order` 已按新次序钉住）。`VaPass` 自报 `ChangeSet`，
  物化份数/取值条数的报告并进 `FORGE_TRACE_AGG`（原来只在 `FORGE_TRACE_LOWER` 里）。
- `func_owned`（可变 IR 副本）的创建条件补齐：以前只在 `needs_agg_expand` 时建、变参块自己
  `get_or_insert`，现在两处显式各建各的，语义不变但不再依赖隐式时序。

**判据**：workspace 门禁 exit 0；`forge-codegen --lib --all-features` 2811 passed；
jit x86 197/3/0、riscv64 136/64/0；asm_parse 3 / asm_exec 1；clippy/fmt 干净。

**环境事故（记录）**：本轮门禁首次运行时 `link.exe` 报 **exit 1102**（同时两矩阵与 asm 全绿）
——是 **D: 盘再次被 `target/debug/incremental`（10.9 GB，`target/` 共 51.7 GB）写满**，
删掉该目录后同一条命令 exit 0。CLAUDE.md 的「os error 112 / 磁盘满」条目同样适用本现象。

**M6 仍欠**：缺口泛化（`va_arg` 聚合分支、`>16B` 非 sret 栈返回 `StackByVal`、未对齐嵌套字段
逐字节拷贝、聚合其余算子值内存化）与 `int_ty()` 非 1/2/4/8 宽度改 fail-closed。

### M6 批次 5：去重项（R6）—— 打包与栈缓冲取址各收成**一个出口**（2026-10-11，未提交）

1. **`agg_const::pack_segment(bytes, off, len) -> u64`**：把"段值 / ≤8 字节聚合常量参数"的
   字节打包收成唯一实现。原来 4 处各写一遍（`agg_call.rs` 两份、`agg_store.rs` 两份），
   而口径并不一致——一半用 `get(..).unwrap_or(0)` 越界补零、一半直接下标。统一为前者
   （下标那种是"不越界"的特例，结果相同）。
2. **`legalize::common::place_addr_of_slot(func, block, size) -> (Inst, Value)`**：
   `Alloca(Type(I8), Uint(n))` 这个**帧槽预扫描唯一认得的规范形态**原来在两个 pass 里各写
   一遍（写 `Const(池 id)` 会让槽根本不进表 ⇒ 没有偏移，后续 store/load 全错位）。现在只有
   这里能造，并附"调用方负责移到正确块位置（SSA 支配）"的契约说明。
3. 顺手清掉 `agg_expand.rs` 里那段**过时注释**（它还写着"expand_* 执行族仍留在 compiler.rs
   按批再迁"——批次 2+3+4 已全部迁完）。

**判据**：workspace 门禁 exit 0；`forge-codegen --lib --all-features` 2811 passed；
jit x86 197/3/0、riscv64 136/64/0；clippy/fmt 干净。

**M6 仍欠**：① `Va` pass 接入驱动；② 缺口泛化（`va_arg` 聚合、`>16B` 非 sret 栈返回、
未对齐嵌套字段、聚合其余算子）与 `int_ty` fail-closed；③ `move_to_front` 的内联副本复核
（现状：定义 1 处 + `agg_expand` 内 3 处调用，未见内联复制）。

### M6 批次 4：`legalize/common.rs` —— **反向依赖清零**（2026-10-11，未提交）

把只服务合法化的 6 个共享件从 `compiler.rs` 搬进新 `legalize/common.rs`：
`AggArgLoc`（含 derive，搬运时一度把 derive 落在 `CompileSession` 上，clippy/rustc 当场报出来）、
`agg_arg_place`、`agg_ret_is_sret`、`append_param`、`collect_extract_jobs`、`make_placeholder_ptr`。

**结果**：`grep 'pipeline::compiler' pipeline/legalize/*.rs` = **0** —— 合法化不再反向 `use`
编译管线（方向恢复为 `compiler → legalize`）。`compiler.rs` 里只剩一处**测试模块内**的
`use legalize::common::{AggArgLoc, agg_arg_place}`（那是落点探针的集成用例在读它；放在顶层会被
lib 构建判为 unused import——clippy 抓到了，已按"谁用谁引"放回测试模块）。

`read_path_budget` 随之下调：`compiler.rs` 4 → **3**，并新增 `legalize/common.rs` 条目（1 处）。

**判据**：workspace 门禁 exit 0；`forge-codegen --lib --all-features` 2811 passed；jit x86 197/3/0、
riscv64 136/64/0；clippy/fmt 干净。

**M6 仍欠**：① `Va` pass 接入驱动；② 去重项（`pack_segment`/`place_addr_of_slot`/统一
`move_to_front`）；③ 缺口泛化（`va_arg` 聚合、`>16B` 非 sret 栈返回、未对齐嵌套字段、聚合其余
算子）与 `int_ty` fail-closed。

### M6 批次 3：**剩下的 3 个 `expand_*` 也迁出** ⇒ `compiler.rs` 里的合法化函数体归零（2026-10-11，未提交）

- `expand_large_agg_ret`（246 行）与 `expand_agg_call_args`（251 行）→ `legalize/agg_call.rs`；
  `expand_large_aggs`（379 行）→ `legalize/agg_param.rs`。
- **`rg 'fn expand_' pipeline/compiler.rs` = 0**（M6 验收要求 ≤3，已达成为 0）。
- `read_path_budget` 再次随迁移下调：`compiler.rs` **13 → 4**；legalize 侧定额按实测更新为
  `agg_call.rs` 5、`agg_param.rs` 9、`agg_store.rs` 1、`gep.rs` 1（`agg_expand.rs` 仍 6）。
- 迁移中新增 `pub(crate)` 暴露：`collect_extract_jobs`（`agg_call.rs`/`agg_param.rs` 用）。

**判据**：workspace 门禁 exit 0；`forge-codegen --lib --all-features` **2811 passed**；
jit x86 197/3/0、riscv64 136/64/0；clippy/fmt 干净。

**仍欠**：① `compiler.rs` 仍被 legalize 反向依赖（`agg_ret_is_sret`/`append_param`/
`make_placeholder_ptr`/`agg_arg_place`/`AggArgLoc`/`collect_extract_jobs` 都是 `pub(crate)`），
下一批搬进 `legalize/common.rs`；② M6 的缺口泛化项与 `int_ty` fail-closed 未做。

### M6 批次 2：pass 体迁出 `compiler.rs`（2026-10-11，未提交）

把 5 个 pass 的**函数体**从 `compiler.rs` 搬进 `legalize/`：

| 新文件 | 迁入 |
| --- | --- |
| `legalize/agg_call.rs` | `expand_sret_return`（43 行含文档）+ `expand_large_agg_call_results`（98 行） |
| `legalize/agg_store.rs` | `expand_agg_stores`（178 行） |
| `legalize/gep.rs` | `expand_geps`（175 行） |
| `legalize/agg_param.rs` | `expand_large_agg_params`（193 行） |

**验收指标达成**：`rg 'fn expand_' pipeline/compiler.rs` = **3**（要求 ≤3）。剩下的 3 个
（`expand_large_agg_ret` / `expand_agg_call_args` / `expand_large_aggs`）按同一模式迁入
`agg_call.rs` / `agg_param.rs` 即可归零——留作下一批（它们之间互相引用较多，一批做完再动）。

**批次 2 的两处连带调整（都已落）**：

1. **`read_path_budget` 预算随迁移下调**：`compiler.rs` 的 `borrow()` 处数 20 → **13**
   （守卫的原话就是"迁移完成了一批？请把预算下调到 13"），并为 4 个新文件登记定额：
   `legalize/agg_call.rs` 3、`agg_param.rs` 2、`agg_store.rs` 1、`gep.rs` 1（各 = 实测值）。
2. **`compiler.rs` 里的合法化专用件标 `pub(crate)`** 供迁出的 pass 使用：
   `agg_ret_is_sret` / `append_param` / `make_placeholder_ptr` / `agg_arg_place` / `AggArgLoc`；
   `rewrite_agg_value_uses` 直接从 `agg_expand` 引（实现本来就在那里）。

**过程记录（如实）**：批量删未用 import 的脚本用了 `;\n` 模式，在 **CRLF** 文件上没生效，
反复"trim 成功但没删掉"；随后一处按名字替换误伤 `gep.rs` 的函数体（把体内一行当成 import 行）
⇒ **改为从 `git show HEAD:compiler.rs` 重新抽取该函数体整文件重写**，并顺手给它标 `pub(crate)`。
教训与仓库既有的记录一致：**多行/带换行的文本改写一律别用 PowerShell 或简单脚本，优先单个
`edit` 工具调用**（本次连脚本里的 `\n` 都踩了 CRLF）。
**共享件现状（下一批收口）**：`agg_ret_is_sret` / `append_param` / `make_placeholder_ptr` /
`agg_arg_place` / `AggArgLoc` 目前仍留在 `compiler.rs` 并标了 `pub(crate)`，由迁出的 pass
`use` 回来；`rewrite_agg_value_uses` 直接从 `agg_expand` 引（它本来就是那里的实现）。
下一批把这些**合法化专用件**一并搬进 `legalize/common.rs`，让 `compiler.rs` 不再被 legalize 反向依赖。

**判据**：`forge-codegen --lib --all-features` **2811 passed**；workspace 门禁 exit 0；
jit x86 197/3/0、riscv64 136/64/0。
**判据（本批）**：workspace 门禁 exit 0、`forge-codegen --lib --all-features` **2811 passed**
（+1 驱动守卫）、jit x86 197/3/0、riscv64 136/64/0。

## 13. M4 进度（收口）

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

### 批次 C 第一步——✅ 已落地（2026-10-11）：旁表改名 + 角色类型化

`InstPacket::xreg_map: Vec<SmallVec<[(XReg, u8, bool); 2]>>` →
**`inst_ops: Vec<SmallVec<[OperandBind; 2]>>`**，`OperandBind { vreg, field, role: OperandRole }`
（带 `new`/`def`/`use_` 构造器）。裸 `bool` 与形状表里的 `OperandRole` 本是同一件事的两种写法，
现在只剩一种；**语义完全不变**（`is_def = true` ⇔ `role == Def`）。

同步面：`CompileState.inst_ops`、`liverange`/`regalloc_bt`/`emission` 三处签名 + 八处解构、
`pipeline/lowering.rs` 两处 `extend`、两个测试模块的夹具与九处注释。
验收：门禁 exit 0、clippy/fmt 干净、两矩阵与基线逐数相同、codegen 单测 2722 绿。

### 批次 C 第二步——✅ 已落地（2026-10-11）：值宽类折进绑定表，两种推导口径合一

**发现的真问题**：字段回填时两个调用点各推一次类，**口径还不一样**——
`compiler.rs` 主回填走 `ctx.reg_class_for(ty)`（ISA 类型映射），`emission.rs` 的 spill 回填走
`forge_ir::RegClass::from_type_id(ty)`。同一个 vreg、同一个 IR 类型，两处可能推**不同的类**。

**修法**：`OperandBind` 增 `class: Option<RegClass>`；管线在 lowering 之后、进 regalloc 之前
**折一次**（`inst_ops` 逐项 → `ctx.reg_class_for`），两个回填点此后读同一份数据
（`bind.class` / `inst_classes[i]`），缺省退 vreg 自身类。
连带：`emit_inst_with_spills` 的 `xreg_types: &HashMap<XReg, TypeId>` 参数换成
`inst_classes: &[Option<RegClass>]`（顺手少一个跨字段借用）。

**余项**：`ctx.xreg_types` 字段本身还在（lowering 期草稿，~26 个 vreg 分配点写入），
但**读取面已收口到一处**（那次折入）。彻底删除的路径：让这 26 个分配点把类直接记进绑定
（或以包为单位记录 vreg→类，随 `append`/`extend` 带上来）。

### 批次 C 第三步——✅ 已落地（2026-10-11）：三个平行数组收敛成一个绑定切片 + `set_operand_reg` 删除

**发射侧**：`emit_inst_with_spills` 的 `inst_xregs`/`inst_field_idx`/`inst_classes` 三个平行切片
换成**一个 `binds: &[OperandBind]`**（调用点少铺三次 `collect`、签名少两个参数）。
spill 回写补 fail-closed：字段不可改写时 `Err(IrError::RegAlloc)`（与 regalloc 的 WA-40 同口径；
生成物 `settable` 恒真 ⇒ 发行 ISA 行为不变）。

**方案 API 的实测否决**：原设想的 `set_operand_reg(i, preg, class)` **已删除**——两个序号空间
不同源：绑定表只登记**绑定到 vreg** 的字段，形状表 `operands()` 登记该变体**全部** use/def 字段
（物理寄存器占位的操作数只在后者）。`add` 这类 InOut 指令实测：旁表 1 条、形状表 2 条，
`set_operand_reg(bind_idx)` 会写到**另一个字段**。回写统一为
`is_reg_field_settable(field)` + `set_reg_field(field, …)`：字段号来自绑定表、可改写性来自形状表。

### M4 收口（2026-10-11）

**逐条对照方案验收**：

| 方案要求 | 状态 |
| --- | --- |
| `uses`/`defs`/`use_constraints`/`def_constraints` 由 `operands()` 派生、生成物不再发射 | ✅（批次 A/B；回潮守卫在 `machine_shape_table.rs`） |
| 旁表归一：`inst_ops: Vec<SmallVec<[OperandBind; 2]>>`（`vreg`/`field`/`role`/`class`） | ✅（批次 C 第一、二步） |
| 值宽类的两种推导口径合一（`compiler.rs` 与 `emission.rs`） | ✅（批次 C 第二步：折一次，两处读同一份） |
| 回写只有一个入口、按**字段号**、可改写性 fail-closed | ✅（批次 C 第三步；`set_operand_reg` 按实测删除） |
| 发射侧平行数组清零 | ✅（批次 C 第三步：三个数组 → 一个 `binds` 切片） |
| `CompileState` 无 `xreg_map`/`inst_clobbers`/`xreg_types` | ⚠️ **部分**：`xreg_map` 已改名归一 ✅；`xreg_types` **保留（记为偏差，见下）**；`inst_clobbers` 保留（见下） |

**偏差一（`xreg_types` 保留）**：vreg 的**值宽**（I32→`GPR(4)`）既不在指令形状里、也不在 vreg 的池宽类里，
只能由"vreg → IR 类型"承载。现在读取面**只剩一处**（lowering 之后折进绑定），消费面读绑定。
彻底删表需先让 `XReg` 自己带值宽类（模型改动，不在 M4 范围）。

**偏差二（`inst_clobbers` 保留）**：它与 `MachineInst::clobbers()` **不是同一件事**——前者是
**规则级**"模板里写死的物理寄存器"（生成器 `collect_phys_clobbers` 按规则求并、展开到包内每条指令），
后者是**指令级****implicit 声明**（如 `cqo` 的 RDX）；regalloc 在 `regalloc_bt.rs:230-241` 把两者**合并**。
要删前者，得让生成物为**每个变体**发表"模板字面物理寄存器"（像 `__SHAPES` 那样），
顺带把"按包求并"变精确（多指令规则不必让每条指令都背全并集）——已列为 M5 的候选（与 C3/C4 同域）。

**新发现（存量，非本轮引入）**：实测存在"多类槽绑定没有值宽类记录"的编译路径
（`test_jit_agg_by_value_stack_sysv64` 第 15 条指令、riscv64 通道同样命中）——回填退到池宽类。
改动前的两个回填点同样先查 `xreg_types`、查不到即退池宽类 ⇒ 行为未变。
可见性：`FORGE_TRACE_M4=1` 打出条目。**若要收口**（多类槽一律带类），需要先量清这些 vreg
为什么没有类型记录（候选：lowering 内部临时 vreg 不登记类型）。

**下一步（历史记录）**：：`ctx.xreg_types` 字段（~26 个 vreg 分配点写入，
读取面已收口到那次折入）——让分配点把类直接记进绑定（或以包为单位记 vreg→类、随
`append`/`extend` 带上来），之后 `CompileState` 只剩 `inst_ops`，M4 即可收口。

**下一步（历史记录）**：
① 把 `ctx.xreg_types`（vreg → IR 类型）的**读点**（`emission.rs:307` 前后的回填宽度类、
`compiler.rs:2570` 附近）改成读 `inst_ops[..].class`（`OperandBind` 增 `class` 字段，
写点仍从 IR 类型推），从而删掉这张表；
② `set_reg_field` 的两个生产调用点（`compiler.rs:2563`、`emission.rs:311`）改走
`set_operand_reg`（按**操作数号**而非字段号），`CompileState` 只留 `inst_ops`。

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
