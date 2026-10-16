# 后端 v24 里程碑方案：合法化两相化 + 值描述单一来源 + 缺口/观测台账化

> **状态：[progress]**（2026-10-11 立项）。本文是**活账本**：每个里程碑落地时在 §6 追加
> 批次记录（含实测数字、命令、偏差），任何"说得出但没做到"的东西必须如实写在这里。
> 立项时的代码基线 = v23 收口提交 `8f39d966`；**行号随迭代漂移，以符号名为准**。
> 上一轮方案（v23，已收口归档）见 [`docs/archive/backend-v23-plan.md`](../archive/backend-v23-plan.md)。

前提纪律（沿用既有约定）：

- **一个任务一次提交**（不按批次拆 PR）；**里程碑未完成不提交**，工作累积在工作区。
- 每个里程碑以仓库全量门禁 + 该里程碑新增守卫为验收；**删除清单逐条清零**才算完成。
- 破坏性更新允许，但 **C ABI 互操作不可破坏**（见 §5 不做清单）。
- 守卫必须精确（白名单条目**必须被命中**）；预算只许按实测下调，不许为放过而上调。

## 1. 目标与成功判据

| 维度 | 目标 |
| --- | --- |
| 无冗余 | 聚合合法化只剩**一份**算法、**一张**表；`LowerCtx` 的值描述表由 4 张并为 1；能力缺口要么有实现、要么在数据层不可达（不留 fail-closed 死路） |
| 人体工学 | 生成物（DSL lowering）不再依赖宿主填充的旁表；诊断开关不可能"静默失效"；契约文档与代码用守卫对齐 |
| 通用性 | 管线内**零** ISA/约定名；新增一份约定 = 只改数据（TOML），不改管线代码；守卫继续钉 ISA 中立 |
| 可验证 | 每个里程碑的判据 = 全量门禁（workspace / 两 JIT 矩阵 / asm 三档棘轮 / clippy / fmt）+ 新增守卫 + 删除清单清零 |

**总成功判据**（收口时一次性核对）：

1. `cargo test --workspace --exclude forge-rustc` exit 0；`cargo test -p forge-codegen --lib --all-features` 全绿；
   `jit_matrix_x86` 197/3/0、`jit_matrix_riscv64` 136/64/0；asm 三档（`asm_parse` / `asm_encoding` / `asm_exec`）
   棘轮不变；`clippy -D warnings` 与 `fmt --check` 干净。
2. 删除清单逐条为零（每条给出 `rg` 判据）。
3. 新增守卫全绿，且每条都在注释里写清"为什么值钱"（防摆设守卫）。
4. 契约文档（[`docs/reference/backend-pipeline.md`](../reference/backend-pipeline.md)）与代码清单被守卫钉住一致。

## 2. 证据（立项时只读侦察得到的代码事实）

1. **聚合双态 + 侧表是整个后端的最大冗余面**：`AggSlots`（值 → (槽地址, 类型)）被 6 个文件、
   76 处引用；`rewrite_agg_value_uses` 由 4 个 pass + `va_expand` 调用；`move_to_front` 有
   **6 个调用点**，它存在的原因只是 `make_inst` 把新指令**追加到块尾**（`agg_expand.rs` 的
   `move_to_front` 定义处写明）。
2. 这一套机制在 v23 收尾阶段**直接产出四个缺陷 + 两处隐患**：
   ① 改写 `extractvalue` 时 **use-before-def**（缺 `move_to_front`，症状是"字段各自读对、
   一起用就错、每次运行值不同"）；② by-ref 分支 `return Ok(())` **跳出整个循环** ⇒ 同一聚合值的
   第 2+ 个使用点漏重写（漏掉的 `ExtractValue` 掉到 lowering）；③ 实参来源覆盖不全
   （`insertvalue` 链）；④ **pass 次序**脆弱：形参必须先于实参、"大聚合 load 值传播"必须后于实参
   （两向约束逼出一次 pass 拆分），而 ABI 槽排布又必须在聚合族**之后**（否则帧槽重叠）。
3. **值描述表有四份**：`LowerCtx::{xreg_types, vreg_classes, vreg_types, vreg_widths}`
   （`crates/foundation/forge-isa-runtime/src/ctx.rs`）。生成的 DSL lowering 在
   `crates/frontend/forge-isa-dsl/src/dsl/codegen/*.rs` 里 **~20 处**读 `ctx.xreg_types`；
   `crates/backend/forge-codegen/tests/entity_tables.rs` 专门守卫"这些表必须保持非密集下标语义"
   ——即把一份冗余固化成了契约。
4. **能力缺口散落在 65 处 `Unsupported`** 里（多数是"IR 结构损坏"的正常报错）。真·能力缺口示例：
   `legalize/agg_call.rs`（既非 sret 也非寄存器档的聚合返回）、`insertvalue` 链实参（v23 试做后
   按纪律回退）、`va_expand.rs` 的"约定未声明 `aggregate_arg`"（aapcs64 / lp64d 的聚合变参）、
   `legalize/common.rs` 的"约定把聚合判成标量栈落点"。
5. **观测开关会静默失效**：`pipeline/mod.rs` 的 `trace_enabled` 用 `TRACE_KEYS` 白名单 +
   `OnceLock` 缓存；v23 实测 `FORGE_TRACE_AGG` **不在白名单里** ⇒ 相关 `eprintln` 全是死代码，
   排查时一度误导。
6. **契约文档漂移**：`docs/reference/backend-pipeline.md` 只写了 M1，§4 指向已归档的 v23 方案
   ——M2–M6 的契约变化在当前文档里无处可查。
7. 管线规模（行数）：`regalloc_bt.rs` 1453 / `compiler.rs` 1446 / `agg_expand.rs` 1126 /
   `va_expand.rs` 1033 / `legalize/*` 约 2000。

## 3. 里程碑

### M1 —— 合法化两相化：**计划（只读）+ 一次性物化**

**问题**：现在的合法化是"每个 pass 就地改 IR 并立即重写使用处"，于是"谁先跑"成了隐式契约
（证据 1/2 的四类缺陷）。

**改法（唯一新机制，其余是删除）**：

- 新增 `pipeline/legalize/plan.rs`：`enum Place { Segs(SmallVec<[Value; 2]>), Mem(Value /*地址*/), Void }`
  与 `PlaceTable = SecondaryMap<Value, Place>`（**取代 `AggSlots`**：键相同、语义更窄）。
- **两相**：
  - **Phase A（计划，只读）**：遍历函数，为**每条 ABI 边界**算出 `Place`（形参 / 返回 /
    call 实参 / call 结果 / `va_arg` 结果），写进 `PlaceTable`。这一步**不改 IR**、不建指令、
    不建槽；边界判定全部来自 `forge-abi` 的 `AbiPlan`（保持通用性）。
  - **Phase B（物化，一次）**：按块序、按指令序**单遍**走完函数，为每个用到的 `Place` 就地发出
    显式 `Alloca` / `Load` / `Store` / `Iadd`(GEP)，并把使用处改成标量值。只有这一遍在写 IR
    ⇒ "生成的指令被追加到块尾"不再构成问题 ⇒ **`move_to_front` 全部删除**。
- 槽的分配时机随之确定：**Phase B 之后**才排 ABI 槽，彻底消灭"合法化前扫深度"的隐患
  （v23 是用拆 pass 绕开的）。
- `LegalizeDriver` 保留（定序 + `ChangeSet` 报告 + 幂等），但 pass 收敛成三类：
  `plan`（申报边界）/ `materialize`（物化）/ `gep`（常量折叠）；`agg-leftover` 的存在理由
  （"必须在实参之后"）消失 ⇒ 删除。

**接口/破坏面**：`pipeline::agg_expand` 的公开面收窄到 `PlaceTable` + 物化入口；`legalize/*`
各 pass 的签名从"改 IR"变为"申报边界"。宿主 API（`CompileSession` / `JitLoader`）**不变**；
C ABI 语义**不变**。

**删除清单（判据）**：

- `rg 'rewrite_agg_value_uses' crates/backend` == 0。
- `rg 'fn move_to_front' crates/backend` == 0。
- `rg 'AggSlots' crates/` == 0。
- `agg_expand.rs` 行数比基线（1126）降 ≥ 40%；`agg-leftover` 从驱动清单消失，
  `driver_has_the_documented_pass_order` 改为钉 `plan → materialize → gep`。

**新增守卫**：`place_is_written_once`（同一值计划两次即红）；`materialization_is_idempotent`
（第二遍零改动，沿用 `legalization_is_idempotent` 口径）；`no_ir_write_before_materialize`
（Phase A 跑完后 `dfg.insts()` 条数不变——直接钉住"计划阶段只读"）。

**验收**：v23 已建的全部聚合/变参用例（`test_jit_agg_*`、`test_jit_va_arg_*`）保持绿；
为四类缺陷各留一条回归用例（use-before-def / 多使用点 / `insertvalue` 来源 / 次序无关）；
两矩阵计数不下降。

**风险**：中-高（重写合法化内核）。缓解：**绞杀者策略**——先按新结构实现并让行为等价
（用 `ChangeSet` 的规模差逐用例对拍），全绿后再删旧路径；M1 单独占一个窗口，不与其它里程碑混提。

### M2 —— 值描述单一来源：`xreg_types` 退场

**问题**：四张平行表（证据 3），且生成物在 lowering 期读宿主填充的 `ctx.xreg_types`
——通用性与人体工学双输（新增 ISA 或改类型系统要同时改生成器模板与宿主）。

**改法**：

- runtime：`LowerCtx` 的四张表并为一张 `values: SecondaryMap<VReg, ValDesc>`，
  `ValDesc { class: RegClass, bits: u16, ty: TypeId }`；对外**只给访问器**
  （`desc(v)` / `is_fp(v)` / `bits(v)` / `class(v)` / `ty_of(v)`），表本身私有。
- 删除 `LowerCtx::xreg_types`。它的两个宿主消费点（`compiler.rs` 里"折进绑定表"与"分配后回填"）
  改读 `ValDesc` + `AllocResult` 绑定（v23 M4 批次 C 已把类型折进 `OperandBind::class`，绑定自足）。
- DSL codegen：`dsl/codegen/*.rs` 的 ~20 处 `ctx.xreg_types.get(&v)` 改为访问器；
  生成物**不再需要**任何"值→类型"旁表。谓词名（`rd_width` / `rd_vec` / …）与数据面不变
  ⇒ 三份发行谱的 TOML 零改动。
- 守卫：`tests/entity_tables.rs` 改写为 `only_one_value_desc_table`
  （`LowerCtx` 里键为 `VReg`/`XReg` 的平行表数 == 1）；`read_path_budget` 表按实测更新。

**验收**：`rg 'xreg_types' crates/` == 0；三 ISA 生成期自测 + 两矩阵 + `forge-tests` 全绿；
生成物体积不回潮（按 `docs/performance/` 既有口径复测并记录数字）。

**风险**：低（机械替换 + 一处字段合并），但触及生成器模板 ⇒ 必须三份谱一起跑。

### M3 —— 能力缺口台账化：**实现或数据层删除**

**问题**：能力缺口以 65 处 `Unsupported` 的形态散在代码里（证据 4），没人知道"这台机器 /
这份约定到底缺什么"，且失败信息只在触发时可见。

**改法**：

- 新增 [`docs/reference/backend-gaps.md`](../reference/backend-gaps.md)：**能力缺口台账**，
  每行 = 缺口 / 谁需要 / 决策（`已实现` / `不做` / `待做`）/ 钉住它的守卫或数据条件。
- 新增守卫 `capability_gaps_guard`（放 `crates/backend/forge-codegen/tests/`，形态照
  `read_path_budget`）：扫描"能力类" `Unsupported` 消息集合，与台账表**逐一相等**
  （多一条、少一条都红）；台账条目必须指向真实文件。
- 逐条决策（本方案拍定，实施时按此执行）：
  1. **aapcs64 / lp64d 的聚合变参**：先做一次只读测量（`agg_arg_place` + 各自 `classify` 数据），
     确认 `>16B` 聚合在变参位置的**调用方**落点；若是 `StackByVal`（与 sysv64 同形）⇒ 复用
     sysv64 做法（`aggregate_arg` + `limit = 0` + 内联档）并加真执行用例；若走寄存器/保存区
     ⇒ 台账记 `不做`，数据层**不声明** `aggregate_arg`（现状），失败信息保持点名它。
  2. **`insertvalue` 链作为聚合实参**：M1 的两相物化天然覆盖（方向是"按定义链求地址"）。
     M3 把 v23 留下的**契约断言**（`test_jit_va_arg_reads_unnamed_aggregate_arg` 里
     "必须报错否则红"那段）翻成真执行断言（`f2() == 11`）。
  3. **`Placement::Pair` / `RetLoc::RegGroup` / 无指针 `Indirect` 的发射落点**：逐条判定
     "实现"或"数据不可达"。**注意**：小聚合（sysv64 `≤16B` 直接 int 池、aapcs64 HFA
     `slots = "hfa"`）是 **C ABI 强制**的寄存器档，**不删**（见 §5）。
  4. **宽向量第 5+ GPR 槽**：维持"不做"（v20 已拍板），进台账并写理由。
  5. **sysv64 `>16B` 非 sret 栈返回**：v23 已证"前提有误、无消费者"（四份约定的大聚合返回全走
     `hidden_sret`），台账记 `不做` + 指向守卫 `wide_return_uses_each_conventions_own_sret_slot`。

**验收**：台账与代码**双向**一致（守卫强制）；"实现"类条目各自有真执行/生成级用例；
"不做"类条目在数据层不可达（`isa/*.toml` 与 `forge-abi/src/builtin.rs` 里没有对应规则）。

**风险**：低-中（以记录为主，每条决策都小而明确）。

### M4 —— 观测台账化：诊断开关不可能静默失效

**问题**：证据 5（`FORGE_TRACE_AGG` 死了几个月）。

**改法**：

- 新增 `pipeline/trace.rs`：`const KNOBS: &[(name, 说明)]` 单表 + `fn knob(name) -> bool`。
  语义：**未注册的名字在 debug 下 panic**（`debug_assert!`），release 下视为关（不 panic，
  但 `eprintln` 一次警告）。
- 删除 `TRACE_KEYS` 白名单 + `OnceLock<HashMap>` 缓存（改为按名 `OnceLock<bool>`，或直接读 env
  ——`trace_enabled` 不在热路径）。
- 新增守卫 `trace_knobs_are_bijective`：扫描全仓 `trace_enabled("…")` / `knob("…")` 字面量，
  与 `KNOBS` 表**双向相等**；并把表打印进 `reference/backend-pipeline.md` 的一节
  （文档由守卫钉住与代码一致）。

**验收**：`rg 'TRACE_KEYS' crates/` == 0；双向守卫绿；`FORGE_TRACE_AGG` 在新机制下是注册项
且被现有用例覆盖。

**风险**：低（纯诊断面）。

### M5 —— 契约回写 + 账本工程化

**问题**：证据 6（`backend-pipeline.md` 只写 M1）。

**改法**：

- `docs/reference/backend-pipeline.md` 补齐现行契约：阶段表（合法化两相 / 指令选择 / 分配 /
  帧 / 发射 / 装载）、聚合与变参的表示契约、诊断开关表、缺口台账指针；状态头 = `[active]` + 日期。
- 守卫 `pipeline_stages_match_doc`：代码里的阶段/pass 清单（`LegalizeDriver::pass_names()`
  与管线阶段常量）必须与文档表格**集合相等**（照 `driver_has_the_documented_pass_order` 与
  `schema_guard` 的口径）。
- 本账本进文档地图：`docs/README.md` 的 `plans/` 表与 `CLAUDE.md` 的目录树恢复指向
  `plans/backend-v24-plan.md`。

**验收**：契约文档与代码清单守卫绿；`docs/README.md` / `CLAUDE.md` 引用可解析（`git grep` 无死链）；
改动文档 `npx --yes markdownlint-cli2 <文件>` 0 issues。

**风险**：低（文档 + 一条守卫）。

## 4. 交付物

1. 本活账本（方案原文 + §6 逐里程碑回写）。
2. 代码：`pipeline/legalize/{plan,materialize}.rs`（M1）、`ctx.rs` + `dsl/codegen/*`（M2）、
   `docs/reference/backend-gaps.md` + 守卫（M3）、`pipeline/trace.rs`（M4）、
   `reference/backend-pipeline.md` 回写（M5）。
3. `CHANGELOG.md`：每个里程碑一条 `### Changed/Fixed (日期)` 条目（含实测数字与命令）。
4. 每里程碑一次提交（单一 subject 长标题，`git commit -F target/tmp/*.txt`）。

## 5. 假设与不做清单

**假设**：

- C ABI 互操作是硬需求（JIT 用例经 Rust `extern "C"`、forge-rustc e2e、asm_exec）
  ⇒ **小聚合的寄存器档（win64 / sysv64 `≤8B`/`≤16B`、aapcs64 HFA、lp64d `≤2×XLEN`）必须保留**。
  "所有聚合一律内存化"（Cranelift `StructArgument` 那种纯内存模型）**只作为 M1 的内部表示**，
  不改 ABI 语义。
- 门禁口径不变（`$env:CARGO_INCREMENTAL='0'`、`$env:CARGO_PROFILE_TEST_DEBUG='0'`）。
- 允许破坏 `forge-codegen` 内部 API 与 `LowerCtx` 公开字段（M2 会删字段）。

**不做**：① 宽向量第 5+ GPR 槽；② `>16B` 非 sret 栈返回（前提有误）；③ ISA-DSL 语法面改动
（v22 不动）；④ 生成物表化/体积优化（v23 已判定收益不足）；⑤ 新 ISA 接入。

**参考（外部设计，仅作结构参考；本方案的决策以本仓证据为准）**：

- Cranelift 把"结构体实参/返回"提升为 ABI 层的一等概念、按**字节数**标识
  （`ArgumentPurpose::StructArgument(u32)` / `StructReturn`），而不是散在指令选择里 ——
  [cranelift-codegen::ir::ArgumentPurpose](https://docs.rs/cranelift-codegen/latest/cranelift_codegen/ir/enum.ArgumentPurpose.html)；
  其 ABI 层实现见
  [`cranelift/codegen/src/machinst/abi.rs`](https://github.com/bytecodealliance/wasmtime/blob/main/cranelift/codegen/src/machinst/abi.rs)。
- LLVM 侧同构：形参/实参落点由 `TargetLowering::LowerFormalArguments` / `CCState` 一处决定，
  `byval` 聚合同样是"ABI 层显式拷贝/指针"语义 ——
  [llvm/TargetLowering.h](https://android.googlesource.com/platform/external/llvm/+/14a129a3cf8e609b9deb8546267e509088bf7dcd/include/llvm/Target/TargetLowering.h)、
  [llvm-dev: making a copy of a byval aggregate on the callee's frame](https://marc.info/?l=llvm-dev&m=137297343130987&w=2)。

## 6. 批次记录（逐里程碑回写）

### M2 值描述单一来源 —— **一次落地**（2026-10-11，工作区）

**做了什么**：

- 新增 `forge_isa_runtime::ctx::ValDesc { class, bits, ty }`：**值的完整描述**，只有两个
  产生点——`LowerCtx::set_value_desc`（唯一写入口，类与位宽都从 IR 类型推导）与访问器
  （`desc_of`/`is_fp_value`/`is_vector_value`/`value_bytes`/`type_bits_of`）。
- `LowerCtx` 的**四张平行表并为一张**：`values: HashMap<XReg, ValDesc>`（**私有字段**，
  对外只给访问器）。表名 `xreg_types` 与三张 `VReg` 键的表（`vreg_classes`/`vreg_types`/
  `vreg_widths`）**整族删除**。
- **实测的额外收获（无冗余）**：三张 `VReg` 表是**孑遗**——`vreg_classes`/`vreg_widths`
  只写不读，`vreg_types` 只被 `is_float_vreg` 读、而后者**没有任何调用者**；
  `xreg_width` 同样无调用者。⇒ 连同 `next_vreg`/`temp_vregs`/`zero_vreg` 与
  `alloc_vreg*` 一族**一起删除**（整个 VReg 机制在管线里是死的）。
- 消费侧迁移（**语义逐字不变**，只换"类型的来源"）：
  - 生成物模板 16 处（`dsl/codegen/lowering.rs`）→ `desc_of` / `is_vector_value` /
    `value_bytes`；其中 8 处原本问 `type_store.size_bytes`，改用 **同源**访问器
    `value_bytes`（**不**用 `ValDesc::bytes()`——后者由位宽取整，对"存储尺寸 ≠ 标量位宽"
    的类型如 80 位扩展浮点会给出不同答案）；
  - 谓词属性 6 处（`dsl/codegen/integration.rs`：`rd`/`rs1_width`/`rs2_width`/`elem`/
    `rd_vec`/`rs1_vec`）→ `desc_of`，闭包内 `*t` → `d.ty`；
  - 宿主 9 处（`pipeline/lowering.rs` 的 7 个写点 → `set_value_desc`；
    `compiler.rs` 的"折进绑定"与"参数字节宽"两个读点 → `desc_of`）。
- **一处口径分叉被真正消除**（M4 遗留）：以前 `compiler.rs` 的绑定折入要
  `reg_class_for(ty)` 自己再推一次类，而 `emission.rs` 的 spill 回填推法不同；
  现在类的**唯一推导处**是 `set_value_desc`，折入直接 `bind.class = Some(d.class)`。
- 守卫改写：`tests/entity_tables.rs` 的"三张 VReg 表必须密集"（表已不存在）换成
  **`only_one_value_desc_table`**——`LowerCtx` 里值描述表**恰好一张**、值类型是 `ValDesc`、
  **表不得 `pub`**（否则又会长出第二套读法）、且四个旧名在**全仓生产代码里零出现**
  （守卫文件自身的检查名单自跳过，并在注释里写明这一点）。

**验收**（`cargo clean` 后**整树重建**再跑一遍；数字即该次重跑结果）：

| 判据 | 结果 |
| --- | --- |
| `rg 'xreg_types' crates/` | **0**（生产代码；唯一提及在守卫的检查名单里，它自跳过） |
| `forge-codegen --lib --all-features` | **2826 passed / 0 failed** |
| `forge-codegen --test entity_tables`（新守卫） | **4 passed** |
| `forge-ir --test read_path_budget` | **2 passed**（M2 未增删取锁 ⇒ 预算不变） |
| `cargo test --workspace --exclude forge-rustc` | **exit 0** |
| `clippy --workspace --all-targets --all-features -- -D warnings` / `fmt --check` | 干净 |
| 两矩阵 / asm 三档 | **197/3/0** · **136/64/0** · **3/1/1** |
| **生成物体积不回潮**（按时间切分 A/B：迁移前 06:57 的产物 vs 迁移后 07:16 的产物） | x86 6,355,380 → **6,348,453**（−6,927，−0.109%）；riscv64 2,513,978 → **2,510,701**（−3,277，−0.130%）；arm64 6,745,567 → **6,744,803**（−764，−0.011%）——**三份谱全部变小** |

**过程中的两处环境事故（如实记录，都不改结论）**：

1. 有一次 `cargo clippy` 报 `E0786 invalid metadata files`——根因是**门禁作业还在编译时我又改了
   源文件**（两个 cargo 进程重叠写同一个 target）。处置：`cargo clean` 后**全量重建**并把整条
   门禁重跑一遍（上表数字即重跑结果）。
2. `Get-PSDrive D` 曾报"可用 0 GB"，`fsutil`/WMI 又被拒——用 `DriveInfo` 复核实际可用
   **38.2 GB**、写 1 MB 探测成功 ⇒ **不是磁盘满**，别按"磁盘满"误处置。

### M1 合法化两相化 —— 批次 5：**旧改写族删除（M1 收口）**（2026-10-11，工作区）

**做了什么**：

- 物化相补齐最后两类使用：**`insertvalue`**（把来源字节搬进自己的帧槽 + 写字段 + 结果登记；
  后续使用由本相在这条指令**之后**自然处理，**无需递归**）与**终结符 `ret`**
  （段值档就地展开成多返回值；内存档不动——由 `expand_large_agg_ret` 按约定写 sret）。
- **删除旧改写族**（M1 的删除清单）：按值驱动的就地改写函数、按地址内存化的助手、
  "生成后搬到使用点前"的助手，以及它们在三个 pass 里的 **6 个调用点**、
  只为搬运而存在的 `UsePos.pos` 字段、无人构造的 `Place::Void` 变体。
- 计划相**改成完整描述**：不再只装"要动的"边界——不可动的也以 `Keep` 入计划，于是
  "计划 = 这个函数全部 ABI 边界"是可枚举的（也顺带让所有计划变体都有构造点，
  摘掉了两个临时 `allow(dead_code)`；`rg 'TODO\(M1'` == 0）。
- 引用纪律：已删除的符号名**不在代码里留引用**（含注释），历史说明留在本账本。
- 读路径预算按**实测**下调：`agg_expand.rs` 6 → 2；新增 `materialize.rs`(1) 与 `plan.rs`(1)
  两条"整个 pass 只取一次读锁"的预算（守卫 `read_path_budget` 精确等式，绿）。

**M1 删除清单（清零证据）**：

| 项 | 结果 |
| --- | --- |
| `AggSlots` | **0** |
| 旧改写函数 `rewrite_agg_value_uses` | **0**（含注释） |
| `move_to_front` | **0**（含注释） |
| `memoryize_from_addr` | **0**（含注释） |
| `Place::Void` / `rg 'TODO\(M1'` | **0** / **0** |

**代码规模**：`agg_expand.rs` 1242 → **506** 行（旧改写族整体退场）；新增
`materialize.rs` 599 行 + `plan.rs` 401 行（**两相取代一族**，且物化相是唯一的改写者）。

**实测**：`forge-codegen --lib --all-features` **2826 passed / 0 failed**；
`read_path_budget` 2 passed；`clippy -D warnings` / `fmt --check` 干净；
全量门禁（workspace + JIT 两矩阵 + asm 三档）见下（同批实测）。

**期间的一处判断修正（如实记录）**：`va_expand` 的两条单测曾断言"load 的**精确条数**"
与"最后一次段 store 之后**恰好 1 次** load"。使用处改写搬进物化相后，这些**形状专属**的
计数随"谁在哪一步发射哪条"漂移（探针确认**没有重复物化**：同一条 `extractvalue` 只被处理
一次；值由 JIT 真执行用例守）。两条断言改成**结构判据**（"字段读在段 store 之后"的下界、
"间接档至少 5 次 load"），并写明为什么不钉精确值。

### M1 合法化两相化 —— 批次 4：**物化相接管 `extractvalue` / `store` / `call`**（2026-10-11，工作区进行中）

**做了什么**：

- 物化相的形态定稿：**捕获"块尾新追加的那一段"**——`make_inst` 一律追加到块尾，所以比较处理一条
  指令前后的块序长度，就知道它生成了哪些（这样复用既有助手如 `memoryize_from_segs` **不必改签名**）。
- 新增三类使用改写（内存档 + 段值档都管）：
  - **`extractvalue`**：内存档 ⇒ `Iadd`+`Load` / 嵌套直接登记子地址；段值档 ⇒ `segs[off/8]` 按
    `off%8` `Ushr` 取出（浮点再 `Bitcast`）/ 嵌套则开槽写入（复用 `memoryize_from_segs`）；
  - **`store`**：内存档 ⇒ 逐 8 字节 `load`/`store` 搬运；段值档 ⇒ 逐段 `store` 到 `dst+8k`；
  - **`call`/`call_indirect`**：`Mem` ⇒ 实参换成指针；`Segs` ⇒ 就地展开成多个段实参
    （`by-ref` 的"调用方副本 + `byval`"仍归 `expand_agg_call_args`——它跑得更早且自带那段形状逻辑）。
- **搬迁闸门按"值在不在位置表里"判**（不是 opcode）——这是批次 3 的教训固化：
  `extractvalue`/`store`（操作数在表里）与 `call`（任一实参在表里）让渡给物化相；
  `ret`/`insertvalue` 仍在 `rewrite_agg_value_uses` 里（下一批）。
- `agg-param` 把**段值档**也登记进位置表（`Place::Segs`）——这是"段值也能被物化相看见"的前提。
- 新增守卫 `segment_resident_extract_is_materialized`（段值档的 `extractvalue` 必须被物化，
  且 `off%8 != 0` 的字段必须走移位取出）。

**实测**：`forge-codegen --lib --all-features` **2827 passed / 0 failed**。

**删除清单进度**：

| 项 | 状态 |
| --- | --- |
| `rg 'AggSlots' crates/` | **0** ✅ |
| `rg 'rewrite_agg_value_uses' crates/backend` | 仍在（只剩 `ret`/`insertvalue` 两条使用路径） |
| `rg 'fn move_to_front' crates/backend` | 仍在 6 个调用点（旧函数内部）——物化相已不需要它 |

**下一批（批次 5）**：把 `ret`/`insertvalue` 搬进物化相 ⇒ 删 `rewrite_agg_value_uses` 与
`move_to_front`（M1 删除清单清零）⇒ 跑全量门禁 + 收口提交（CHANGELOG + 账本）。

### M1 合法化两相化 —— 批次 3：**物化相接走内存档 `ExtractValue`**（2026-10-11，工作区进行中）

**做了什么**：

- 新增 `legalize/materialize.rs`：按**指令序单遍**处理，逐块读出快照、为每条指令算出要插入的
  生成指令、拼成新的 `inst_order` **一次写回** ⇒ **生成指令天生排在消费者之前**，
  不需要"生成到块尾再搬"（`move_to_front` 的那一步在这里不存在了）。
  - 标量字段：`Iconst(foff)`+`Iadd`+`Load(fty)`，原 `extractvalue` 改 `Copy`；
  - 嵌套聚合字段：**直接登记子地址**（父地址 `+ foff`）并把原指令墓碑化——旧实现在这里会新开
    一个槽再把字节搬一遍，属纯冗余。
- **搬迁闸门用谓词而不是 opcode**（这是本批最重要的一处修正）：`rewrite_agg_value_uses` 里
  `extractvalue` 只在"操作数**已在位置表里**（内存档）"时让渡给物化相；**段值档仍归旧函数**
  ——它需要 `segs` 才能算字段（寄存器里的值没有地址可加）。一开始我按 opcode 让渡，
  结果寄存器档的提取没人管（真执行用例直接红）。
- **事实交接 `shadow_agg`**（生产者在重映射销毁信息前把它交出来）：by-ref 聚合形参会**重建签名**
  并把旧形参值全局替换成 `ptr`（`replace_all_uses`）⇒"这个 `ptr` 代表 T 型聚合"此后**不在 IR
  类型里**，而物化相要按 T 算字段偏移。`agg-param` 记 `shadow_agg[ptr] = T`（并同时给 `ptr`
  自己登记 `Place::Mem(ptr)`，因为它本身就是地址）。缺这一步的实测症状：残留的 `extractvalue`
  因查不到位置/类型一路掉到 lowering 报 `Unsupported("DSL lowering")`。
- 驱动分三批（守卫同步）：**聚合族**（`plan` + 聚合各族）→ **va**（要排完 ABI 槽才跑）
  → **收尾**（`materialize` + `gep`）。收尾放最后的理由：`Va` 也会登记位置（它把聚合实参
  拷进自己的帧槽），物化必须等它。
- 两条新守卫（在 `materialize.rs` 的测试里）：**`materialized_load_precedes_its_consumer`**
  （逐块核对"每条 `Copy` 的操作数定义排在它之前"——正是 v23 真凶的形状）与
  **`materialize_is_idempotent`**（第二遍零改动）。
- 单测同步改成分工后的契约：`va_expand` 的聚合取参用例现在断言"**交接**（结果登记成内存档）"
  再跑一遍物化相断言"字段从槽里 load"。

**实测**：`forge-codegen --lib --all-features` **2826 passed / 0 failed**；
`clippy -p forge-codegen --all-targets --all-features -- -D warnings` exit 0。

**删除清单进度**：

| 项 | 状态 |
| --- | --- |
| `rg 'AggSlots' crates/` | **0** ✅ |
| `rg 'rewrite_agg_value_uses' crates/backend` | 仍在（接管了内存档以外：段值档 / store / insert / call / ret） |
| `rg 'fn move_to_front' crates/backend` | 仍在 6 个调用点（旧函数内部用）——物化相已不需要它 |

**下一批（批次 4）**：把 `Store` / `Call` / `Ret` / `InsertValue` 的使用也搬到物化相
（需要一并搬"调用点形状与属性"的处理），之后删 `rewrite_agg_value_uses` 与 `move_to_front`。

### M1 合法化两相化 —— 批次 2：**位置表换掉（`AggSlots` 退场）**（2026-10-11，工作区进行中）

**做了什么**：把"值 → (地址, **槽类型**)"的旧表换成 `Places`（值 → `Place`：`Mem(addr)` /
`Segs(n 个 i64)` / `Void`），并把旧表名与类型别名一并删除。

- **关键观察（这就是"冗余"的具体形态）**：旧表存的 `ty` 字段是**纯冗余**——三处登记
  （聚合形参 / call 结果 / 嵌套字段）传的都是**该值自己的 IR 类型**，而这从
  `func.dfg.value_type(v)` 随时可取。删掉它之后表里只剩"位置"一件事。
- 表语义随之变窄并可被守卫：**写一次**（同值二次登记 debug 下 panic）；
  `addr(v)` 是"取内存地址"的**唯一访问器**（替代原先散在各处的
  `agg_slots.get(v).map(|&(a, _)| a)`）。
- 冗余清理：删掉 `Places::new`（`Default` 已派生，两处实现同一件事）。

**删除清单进度**（M1 的判据）：

| 项 | 状态 |
| --- | --- |
| `rg 'AggSlots' crates/` | **0** ✅（别名与 24 处引用已改名/删除） |
| `rg 'rewrite_agg_value_uses' crates/backend` | 待物化相接管（批次 3+） |
| `rg 'fn move_to_front' crates/backend` | 同上（6 个调用点） |

**实测**：`forge-codegen --lib --all-features` **2824 passed / 0 failed**；
`clippy -p forge-codegen --all-targets --all-features -- -D warnings` exit 0。

**临时偏差**：`plan.rs` 模块级仍有一个 `TODO(M1 …)` 的 `allow(dead_code)`（计划相的
`Segs`/`Void` 等变体要等物化相构造）；**M1 收口判据含 `rg 'TODO\(M1'` == 0**。

**下一批（批次 3）**：物化相接走**内存档 `ExtractValue`**——单遍、按指令序、插入位置一次到位
（取代"生成后搬到使用点前"的 `move_to_front`）；同批把 `rewrite_agg_value_uses` 的
`ExtractValue` 臂交给它。

### M1 合法化两相化 —— 批次 1：**计划相落地**（2026-10-11，工作区进行中）

**做了什么**（只读的一相，尚未改任何既有语义）：

- 新增 `pipeline/legalize/plan.rs`：`Place`（`Segs`/`Mem`/`Void`）+ `Places`（**写一次**的位置表，
  第二次登记 debug 下 panic）+ 五类落点计划（`ParamPlan`/`RetPlan`/`ArgPlan`/`CallResultPlan`/
  `VaArgPlan`）+ `AbiEdges`（整函数的 ABI 边界计划）+ `plan_abi_edges()`。
- **决策来源唯一**：落点一律问 `forge-abi` 的既有探针（`agg_arg_place` / `agg_ret_is_sret` /
  变参形状的 `arg_rules`），管线里没有任何约定名分支。**签名改写也被建模成计划**
  （`ParamPlan::AsPtr`/`AsSegs`/`AsStackMem`）——因为"重建签名"本身就是 IR 写，
  所以它属于物化相，不属于计划相。
- `LegalizeCtx` 增 `edges: Option<AbiEdges>`；新增 `PlanPass` 并排在驱动**首位**
  （`plan` 进 pass 清单，定序守卫同步）；`FORGE_TRACE_AGG` 打印计划摘要（可动边界数）。
- 两条新守卫：`plan_is_read_only`（跑完计划相后 IR 的指令/值/常量数**逐项相等**）、
  `place_is_written_once`（同值二次登记必须 panic，不得静默覆盖）。

**实测**：`cargo test -p forge-codegen --lib --all-features` **2824 passed / 0 failed**；
`clippy -p forge-codegen --all-targets --all-features -- -D warnings` exit 0。

**临时偏差（M1 收口前必须清零）**：物化相落地前，计划相的数据只有测试与 trace 消费 ⇒
`plan.rs` 模块级与 `Places` 上各有一个 `#![allow(dead_code)]` / `#[allow(dead_code)]`，
都带 `TODO(M1 …)` 注释。**M1 的验收判据含 `rg 'TODO\(M1' crates/` == 0**。

**下一批**：物化相（Phase B）——单遍改写、按计划发显式 IR，替换 `agg-*` 各 pass 的就地改写；
之后删 `AggSlots`/`rewrite_agg_value_uses`/`move_to_front`（删除清单见 §3）。

### M1 合法化两相化 —— 立项（2026-10-11）

先落盘账本（本文），实现自下一轮开始。开工前的只读侦察结论见 §2；
M1 的删除清单、守卫与验收判据见 §3。

<!-- 每个批次在这里追加：批次号 / 日期 / 做了什么 / 实测数字 / 命令 / 偏差 -->

## 7. 里程碑顺序与窗口

`M1`（最大风险，单独窗口）→ `M2` → `M3` → `M4` → `M5`。

M1 成功判据不达标（含"删除清单未清零"）即**停止推进**：如实记录偏差并重估，
不得为了推进里程碑而放宽判据或上调守卫预算。
