# 后端能力缺口台账（v24 M3）

> 状态：`[active]`（2026-10-11 建立；随缺口增删同步更新）
> 守卫：[`capability_gaps_guard`](../../crates/backend/forge-codegen/tests/capability_gaps.rs) ——
> 本表与**源码里的缺口集合逐一相等**（多一行、少一行都红）；扫描面 = `forge-codegen/src`
> 与 `forge-isa-runtime/src`（v25 M2 起，后者也构造缺口错误）。
> 分类学守卫：[`error_taxonomy`](../../crates/backend/forge-codegen/tests/error_taxonomy.rs) ——
> 后端/运行时的 `Unsupported` **必须**是缺口（畸形 IR ⇒ `Internal`，使用/数据错误 ⇒ `Semantic`）。
> 相关：[`backend-pipeline.md`](backend-pipeline.md)（管线契约）、
> [`calling-conventions.md`](calling-conventions.md)（约定层）、
> v24 收口记录 [`backend-v24-plan.md`](../archive/backend-v24-plan.md) §M3（已归档）。

## 1. 这张表解决什么问题

v24 立项时的侦察结论：能力缺口以 60+ 处 `Unsupported(...)` 的形态**散在代码里**——
没人知道"这台机器 / 这份约定 / 这份谱到底缺什么"，失败信息也只在触发那一刻可见。
后果是同一类问题被反复重新发现，而"其实已经支持了、只是没人试"的缺口也长期没人动。

现在每个"做不了"的失败点都用 `IrError::capability(ID, msg)` 构造（v25 M2 起住在 `forge-ir`：
`forge-isa-runtime` 的 trait 默认实现也要能构造缺口错误），消息以
**`[cap:ID]`** 开头：

* **失败信息自带身份**：用户/CI 可以按 `[cap:…]` 搜索、归类、按 ID 豁免；
* **台账可机读**：守卫把源码里的 ID 集合与本表的 ID 列**逐一相等**；
* **决策有落点**：每行必须写清"已实现 / 不做 / 待做"与**钉住它的守卫或数据条件**。

**边界（不进来）**：畸形 IR 与内部不变量（字段越界、常量池缺项…）、使用错误
（约定没注册、JIT 装载 ISA 不匹配）、约定/形状数据自相矛盾——三者都不是"这台机器做不了"，
混进来会让台账失去意义。

## 2. 台账

决策口径：**已实现** = 现在就能做（有守卫或真执行用例）；**数据层** = 机器做得到，缺的是
约定/谱/宿主的声明（改数据即可，不改代码）；**不做** = 明确不计划支持（写明理由）；
**待做** = 计划支持但尚未实现（写明前置条件）。

| ID | 缺口（谁在什么条件下撞到） | 决策 | 钉住它的守卫 / 数据条件 |
| --- | --- | --- | --- |
| `VALUE_POOL_TYPE` | ISA 值池承载不了该 IR 类型（值池 GPR/FPR 比类型窄） | 数据层 | 谱声明 `[meta].value_gpr_width` / `addr_width` / `value_fpr_width`；`forge-isa abi check` |
| `SIMD_WIDE_VECTOR_TIER` | 宽向量（>16B）字节数不在本 ISA 的宽向量档位里 | 数据层 | 谱声明 `[machine].vector_tiers`；`wide_vector_bytes()` 读它 |
| `SIMD_VECTOR_BYREF` | 该约定没声明向量 by-ref（`>16B` 向量参数/返回无处落） | 数据层 | 约定规则 `by_ref`；win64/sysv64 已声明，`forge-isa abi check <谱>` 逐条报告 |
| `VECTOR_BYTES_LOWERING` | IR 向量字节数既不是 32 也不是 64（没有 lowering 规则） | 不做 | 仅 V256/V512 有规则；其它宽度是 IR 层构造错误（v20 拍板） |
| `VECTOR_TIER_UNSUPPORTED` | IR 层出现了**宿主能力表**里没有的宽向量字节数（管线无法为它选指令） | 数据层 | 宿主 `wide_vector_bytes()` / 谱 `[machine].vector_tiers`（分工：`SIMD_WIDE_VECTOR_TIER` 是**谱**声明的档位表，这条是**宿主**交给管线的能力表） |
| `EXCEPTION_LANDINGPAD` | 异常处理 `landingpad` 无机器指令映射 | 不做 | P1.1 仅文本层解析/展示（见 `docs/reference/isa-dsl.md`） |
| `EXCEPTION_INVOKE_RESUME` | 异常处理 `invoke`/`resume` 同上 | 不做 | 同上 |
| `VA_SHAPE_MISSING` | 函数用了 `VaStart`/`VaArg`，但它这份约定没声明 `hidden.va_list` | 数据层 | 约定规则 `hidden.va_list`（形状数据） |
| `VA_INIT_MISSING` | 宿主没给 `LowerCtx::conv.va.init`（对象字段初值） | 数据层 | 宿主注册表给的 plan；`abi_setup` 填它 |
| `VA_CURSOR_FIELD_WIDTH` | `va_list` 游标字段宽度不是 1/2/4/8 | 不做 | 形状数据只应写这四档（写别的属数据错） |
| `VA_ARG_AGG_RULE` | 约定没声明聚合取参规则（`aggregate_arg`） | **不做** | 见 §3.1 与 §3.5：声明会让**外部 C 调用方**静默错值（引擎给 aapcs64/lp64d 的聚合落点是 `ByRef`，而这两份 psABI 的聚合变参是**按值展平**）⇒ 保持 fail-closed，直到有与该 psABI 的**外部对照**证据 |
| `VA_ARG_VECTOR_MEM` | `va_arg` 结果是向量：缺「内存→向量寄存器」取值能力 | **不做** | 见 §3.5：仓内**没有任何**构造 `va_arg(向量)` 的 IR 生产者（无真实用例）；x86 形态数据已具备（`SIMD_VECTOR_BYREF` 声明 by-ref），缺的是需求 |
| `VA_ARG_FLOAT_WIDTH` | `va_arg` 的浮点结果不是 f64/f32 | 不做 | 更窄浮点在变参里按默认提升走（f32→f64） |
| `SPEC_LOWERING_RULE_MISSING` | 这份谱没有该 op/树型匹配的降低规则（生成物的默认实现报错） | 数据层 | 谱里补 `[[lower.<op>]]`；`forge-isa lint --ops <宿主 op 表>` 逐条报告缺哪条 |
| `FRAME_EMIT_EPILOGUE_JUMP` | 这份谱没覆盖 `emit_epilogue_jump`（帧降低的默认实现报错，v25 M2 从 `Unimplemented` 收编） | 数据层 | 谱/生成器补该帧操作；默认实现 fail-closed |
| `SPEC_MOVE_TABLE_GAP` | 这份谱没有覆盖某个 (目的形状 × 来源形状 × 位宽) 的搬运（生成物里按表 fail-closed） | 数据层 | 谱里补该指令并写 `data_width`；`moves.rs` 的 `MoveTable` 是唯一查表处（v26 M4：生成器 19 个发射点归并于此） |
| `SPEC_TERMINATOR_MISSING` | 这份谱没覆盖某个终结序列（`Jmp`/`Br`/`Ret`/trap/`Call`/`CallIndirect` 的指令缺失） | 数据层 | 谱面补 `[[lower.<终结>]]` 与对应指令（v21 W6：终结序列唯一来源 = 谱面；v26 M4 归并 10 个发射点） |
| `SPEC_ROLE_MISSING` | 这份谱没申报某个语义角色（收参/栈参数/浮点槽等），而结构派生不出它 | 数据层 | 谱面在指令上写 `roles = [ … ]`（v26 M4 归并 6 个发射点） |
| `SPEC_FRAME_OP_MISSING` | 这份谱没覆盖某个帧/溢出操作（溢出模板缺、某寄存器类的溢出、FPR 溢出宽度不支持） | 数据层 | 谱面补 `[machine.spill]`/帧指令（v26 M4 归并 5 个发射点） |
| `ABI_RET_SLOT_MISSING` | 调用布局里没有返回寄存器落点（`ret` 槽与约定级 `ret_gpr` 都没有），或宽返回缺 `hidden_sret` 槽 | 数据层 | 约定的 `ret_classify`/`hidden_sret` + 该机绑定（v26 M4 新增；4 个发射点） |
| `ABI_CLOBBERS_MISSING` | 调用点的破坏集既没有 plan、约定级破坏集也是空的 | 数据层 | 宿主注册该约定（`conv_registry`）或约定数据补 `clobbers`（v26 M4 新增） |
| `UNWIND` | `panic=unwind`：缺 SEH 展开表（`.pdata`/`.xdata`）、缺 `__rust_eh_personality`、缺 landing pad 降级 | **不做** | （v27 M2 决策）真做需要：CFI 描述每个函数的 prologue + 展开指令表 + personality 实现 + rustc 侧 `unwind` 表接线；当前 e2e 全部 `-C panic=abort`，拒绝点是 `codegen_crate` 的 panic 策略检查（带此 ID） |
| `TLS` | 线程局部静态（`#[thread_local]`）：曾把 static 发成普通 `.data`/`.rodata` ⇒ **线程私有语义静默丢失** | **已实现** | （2026-10-11，v27 **动态 TLS**）`src/tls_runtime.rs` 生成 `__forge_tls_addr(slot, size, align, init)`：槽里存 `TlsAlloc` 索引 + 1，首次访问为**当前线程**分配并拷贝初值模板（`TlsGetValue` 判空 + `__rust_alloc` + `memcpy` + `TlsSetValue`）；每个 TLS 静态发两个隐藏符号（`__forge_tls_slot_<sym>` / `__forge_tls_init_<sym>`）。地址物化统一在 `lower::const_data_addr`/`Rvalue::ThreadLocalRef` 走它。e2e `thread_local_roundtrip`（零初值 + 非零初值模板 + 读回）⇒ **exit=14**。**边界**：每静态独占一个 TLS 索引（不共享槽）、无析构、`TlsAlloc` 失败直接 `ExitProcess(3)`；静态 `.tls$` 模型（`gs:[58]`）仍需 IR 支持段前缀，未做 |
| `INLINE_ASM` | 内联 `asm!`：此前会静默降级成 `unreachable`（编译无声通过、运行退出 `0xC000001D`）⇒ 静默错值；v27 M2 先 fail-closed | 待做 | **S1 已落地**（2026-10-11）：无操作数的纯文本形态真实现（见下）；（2026-10-11，v27 M2.5/**S1**）**无操作数的纯文本形态已真实现**：`TerminatorKind::InlineAsm` → IR `Opcode::InlineAsm`（文本走 `Immediate::String`）→ 降级期本 ISA 汇编器 `parse_insts` 解析 → 机器指令就地插进函数体。e2e `inline_asm_nop`（exit=42）与 `inline_asm_ud2`（NTSTATUS `0xC000001D`，证明字节真的被执行）。**仍 fail-closed**：寄存器/`const`/`sym` 约束、标签、`options(noreturn)`、以及非本 ISA 的文本 ⇒ `[cap:INLINE_ASM]`（S2/S3/S4 见方案 §8） |
| `MIR_TERMINATOR` | MIR 终结符里后端尚未支持的那些（协程的 `FalseEdge`/`FalseUnwind`/`CoroutineDrop` 等） | **不做** | 兜底必须**明确拒绝**、不许静默 `unreachable`： 未知终结符 ⇒ 编译期报错点名 kind；静默 `unreachable` 会把「没实现」伪装成「程序自己不可达」——`InlineAsm` 就是这么被藏了很久（v27 M2） |
| `ABI_PLACEMENT_UNEMITTED` | 调用布局给了发射侧还没接的落点（`RegPair`/`RegGroup`/无指针 `Indirect`） | 数据层 | 约定改用已接的落点，或补齐发射侧（A6 余项；v26 M4 新增） |
| `FRAME_SPILL_LOAD` | 同上：没覆盖溢出回填的 load | 数据层 | 同上 |
| `FRAME_SPILL_STORE` | 同上：没覆盖溢出回填的 store | 数据层 | 同上 |
| `AGG_RET_PLACEMENT` | 聚合返回的约定落点既不是 sret 也不是寄存器档 | 数据层 | 约定 `ret_classify` 必须覆盖该尺寸（否则 fail-closed） |
| `AGG_RET_STACK_UNSUPPORTED` | 聚合返回走"栈返回"（非 sret） | 不做 | v23 已证**前提有误**（`RetLoc` 无栈变体、四份约定全走 `hidden_sret`）；守卫 `wide_return_uses_each_conventions_own_sret_slot` |
| `AGG_ARG_PLACEMENT` | 聚合实参的约定落点既不是寄存器档也不是按引用档 | 数据层 | 约定 `arg_classify` / `by_ref` |
| `AGG_ARG_INSERTVALUE_CHAIN` | `insertvalue`/`undef` 链搭出来的聚合作为实参 | **已实现** | 见 §3.2（物化相的"沿定义链求地址"共享助手 + 真执行断言 `f2() == 11`） |
| `AGG_LOAD_USE` | 大聚合 `load` 值用于 `store` 拷贝/`extractvalue` 之外的使用点 | **不做** | 见 §3.5：展开策略是**就地 Nop 化 + 使用处展开**（值本身不物化）⇒ `phi`/`select`/`insertvalue` 这类要在别处读整值的使用点没有落脚点；传参/返回另有路径覆盖。要做得先有「物化点」设计 + 真实用例 |
| `AGG_PLACE_PROBE` | 聚合实参落点探针：约定规划失败 | 数据层 | 约定规则/绑定；`forge-isa abi check` |
| `AGG_RET_PLACE_PROBE` | 聚合返回落点探针：约定规划失败 | 数据层 | 同上 |
| `ABI_PLAN_UNAVAILABLE` | 这台机器上的约定规划不出该函数的调用布局 | 数据层 | 约定数据 + 绑定；编译入口 fail-closed（无 plan 不继续） |
| `VCONST_NO_RULE` | 这份谱里没有 `Vconst` 的匹配规则 | 数据层 | 谱的 `[[lower.Vconst]]`（谓词 `rd_vec`/`elem` 等） |

## 3. 逐条决策的证据

### 3.1 aapcs64 / lp64d 的聚合变参（`VA_ARG_AGG_RULE`）

**只读测量**（不改任何数据、不改代码）用**生产同一探针** `legalize::common::agg_arg_place`
（M1 的计划相与合法化各 pass 都读它），夹具 = 24 字节聚合 `{i64,i64,i64}`：

| 约定 | 24B 聚合**形参**落点（实测） | `aggregate_arg` 现状 | 结论 |
| --- | --- | --- | --- |
| `win64` | `ByRef`（指针指向调用方副本） | 已声明（`indirect = true`，阈值 8） | 有真执行用例（`f() == 11`） |
| `sysv64` | `StackByVal { offset: 16, size: 24 }`（整块进传出区） | 已声明（`limit = 0`：memory class 不进寄存器保存区） | 有真执行用例（`== 22`） |
| `aapcs64` | **`ByRef`** | **不声明** | 见下 |
| `lp64d` | **`ByRef`** | **不声明** | 见下 |

测量命令（临时探针已删，做法留档：在 `legalize/plan.rs` 的测试里对四份 `TargetMachine`
各调一次 `agg_arg_place`，`cargo test … -- --nocapture` 读输出）。

**结论：`待做`，本轮不声明。** 两条理由：

1. `aapcs64`/`lp64d` 的落点是 **`ByRef`**（指针语义，与 win64 同形 ⇒ **可**按 win64 形状声明
   `aggregate_arg` + `indirect = true` + 阈值）；这与 `sysv64` 的 `StackByVal` 是**两种**形状，
   不能照抄；
2. 但这两份约定**都没有"聚合变参"的真执行通道**：`arm64` 没有 JIT 矩阵（只有 x86 原生与
   riscv64 QEMU 两条真执行通道），riscv64 侧也还没有 `va_arg` 的 QEMU 夹具。按本仓库的铁律
   （"宁可明确拒绝，也不要静默错值"）——**先不声明**，失败信息保持 `[cap:VA_ARG_AGG_RULE]`
   点名它。**前置条件**：先有该 ISA 的聚合变参真执行用例，再开数据声明。

### 3.2 `insertvalue` 链作为聚合实参（`AGG_ARG_INSERTVALUE_CHAIN`）——本轮**实现**

v23 收尾时这条路径**回退成 fail-closed**（当时的助手编出了 `72198297058681447` 而非 11）。
M3 把它做通了，过程里两处值得记住：

1. **M1 的两相物化并不自动覆盖它**（立项方案的假设有误，实测纠正）：拦住它的是**调用实参
   pass** 自己的"值来源"分派（`load` / 段值 / 其它）。修法是**共享一份实现**——物化相导出
   `ensure_value_addr`（沿定义链求地址），实参 pass 复用它，拿到槽地址后与 `load` 来源走
   **同一条**逐段 load 路径（`load_segs_from_addr`）。
2. **"沿定义链"必须是递归的**：`%t1 = insertvalue undef, 4, 0` → `%t2 = insertvalue %t1, 7, 1`
   里 `%t2` 的**基值自己就是链**。只看一步会把 `%t1` 的字段按 0 处理——这正是 v23 那次
   "能编能跑、值不对"的根因形状。
3. **生成的指令必须被捕获并一起搬到调用点之前**：修好递归后测试**编译通过、运行
   `STATUS_ACCESS_VIOLATION`**——因为物化出来的开槽/搬字节/写字段指令留在块尾，而用它们的
   逐段 `load` 被搬到调用点前 ⇒ **又是 use-before-def**（地址是垃圾）。捕获方式与物化相同：
   比较处理前后块序长度取新增段。

真执行断言（v23 契约断言的**翻转**）：

```text
test_jit_va_arg_reads_unnamed_aggregate_arg  → f2() == 11
```

### 3.5 三条 `待做` 的收口决定（v26 M4）

计划要求「逐条落地或明确降级为**不做**，并把决定写回台账——决策必须离开 `待做`」。
本轮三条都**离开**了 `待做`，理由都是可复核的代码/数据事实：

1. **`VA_ARG_AGG_RULE` → 不做**（最值得记的一条，因为「看起来可以照抄 win64」）：
   * 实测（只读探针）：aapcs64/lp64d 的 24B 聚合**形参**落点由引擎判为 `ByRef`（与 win64 同形）；
   * 但这两份 psABI 的**聚合变参是按值展平**的（不是传指针）⇒ 照 win64 形状声明 `aggregate_arg`
     后，我方**自洽**（我方调用方 ↔ 我方被调方）能过，而与**外部 C 调用方**互操作时会静默错值；
   * 自洽的 JIT 用例**只能证明自洽**，证不了互操作 ⇒ 按本仓铁律（宁可明确拒绝，不要静默错值）
     **不声明**，失败信息继续点名 `[cap:VA_ARG_AGG_RULE]`。
     **前置条件**：与该 psABI 的**外部对照**证据（真实 C 调用方编出的调用序列，或 psABI 原文逐条裁定）。
2. **`VA_ARG_VECTOR_MEM` → 不做**：仓内**没有任何**构造 `va_arg(向量)` 的 IR 生产者（即没有真实用例）；
   x86 侧形态数据其实已具备（`SIMD_VECTOR_BYREF` 已声明 by-ref、mem→vec 搬移也在），缺的是需求。
   要落地时的步骤：`va_expand` 查搬移表取 mem→vec 搬移 + 在真执行用例上钉住。
3. **`AGG_LOAD_USE` → 不做**：大聚合 `load` 的展开是**就地 Nop 化 + 每个使用处展开**
   （值本身不物化，见 `agg_param.rs::expand_large_aggs`）⇒ `phi`/`select`/`insertvalue` 这类
   「要在别处读整值」的使用点**没有落脚点**；传参/返回另有路径覆盖（实参 pass 的 `load` 来源 +
   返回的 sret 物化）。要做得先有「物化点」设计（把整值落进帧槽），那是另一个专项。

### 3.4 生成器侧的 66 个发射点（v26 M4 收口）

**立项证据**：`forge-isa-dsl/src/dsl/codegen/**` 里有 **66 个发射型 `Unsupported`**（写在 `quote!`
里 ⇒ 使用者看得见），**零个**带 `[cap:]` 标记；而 v25 M2 只把这个构造器下沉到了宿主侧
（`forge-isa-runtime`/`forge-codegen`）。后果：谱/约定"做不了"的报错无法按类统计、也不进台账。

**做法**：按 v25 M2 的四分类逐站点判定，把 66 个点**归并成 5 个 `SPEC_*` 类**（谱侧能力），
再并上 **1 个新 `ABI_*`**（布局数据）与既有的 `ABI_PLAN_UNAVAILABLE`；畸形 IR 的 9 处改
`IrError::Internal`：

| ID | 发射点 | 说明 |
| --- | ---: | --- |
| `SPEC_MOVE_TABLE_GAP` | 19 | 没有覆盖 (目的 × 来源 × 位宽) 的搬运 |
| `SPEC_TERMINATOR_MISSING` | 10 | 终结序列/指令缺失 |
| `ABI_PLAN_UNAVAILABLE`（已有行） | 9 + 宿主侧 1 | 调用布局不可得 |
| `SPEC_ROLE_MISSING` | 6 | 语义角色未申报 |
| `SPEC_FRAME_OP_MISSING` | 5 | 帧/溢出操作未覆盖 |
| `ABI_RET_SLOT_MISSING`（新） | 4 | 返回落点/`hidden_sret` 缺 |
| `SPEC_LOWERING_RULE_MISSING`（已有行） | 2 + 宿主侧 | 没有 `[[lower.<op>]]` |
| `ABI_CLOBBERS_MISSING`（新） | 1 | 破坏集数据缺 |
| `ABI_PLACEMENT_UNEMITTED`（新） | 1 | 布局给了发射侧未接的落点 |

**判据（可复核）**：`rg 'Unsupported\(' crates/frontend/forge-isa-dsl/src/dsl` == **0**；
`capability_gaps_guard` 的扫描面扩到生成器，且**类别型 ID（`SPEC_*`/`ABI_*`）允许多触发点**
（它们说的是"这份谱/约定有没有这个能力"这一个**类**），其余 ID 仍守 v24 M3 的"一个决定点"。
验证靠**真的编译生成物**：`cargo test -p forge-isa-dsl`（三份发行谱 + 全部夹具）与
`cargo test -p forge-codegen`（x86/arm64/riscv64 三个宿主的生成模块）。

### 3.3 其它三条（一句话）

* `VECTOR_BYTES_LOWERING` / `EXCEPTION_*`：**不做**，理由在上表末列（v20 与 P1.1 的既有拍板）。
* `AGG_RET_STACK_UNSUPPORTED`：**不做**，v23 已用三条独立证据证伪其前提
  （`RetLoc` 无栈变体 / 四份约定全走 `hidden_sret` / `engine.rs` 的 `Stack` 臂也走 sret）。
* `SIMD_WIDE_VECTOR_TIER` 的"宽向量第 5+ GPR 槽"：**不做**（v20 拍板；调用方/被调方/by-class
  三处 fail-closed，见 `crates/tools/forge-rustc/WORKAROUNDS.md` WA-37）。
