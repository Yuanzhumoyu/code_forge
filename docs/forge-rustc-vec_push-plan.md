# forge-rustc vec_push/vec_string 完整改进方案

> 对应 `docs/roadmap-status.md` 剩余事项 2 与 `crates/tools/forge-rustc/WORKAROUNDS.md`
> [WA-11]。e2e 58 用例中 2 个预期失败（`vec_push` SEGV、`vec_string` len 错）
> + 派生现象（`vecwc2` 挂起 124）。十二轮深挖已收敛根因范围，本文给出
> **分步消元的完整技术路径**（每步独立可验证，不再盲调）。

## 1. 现象与已确认事实

| 事实 | 状态 | 依据 |
| --- | --- | --- |
| `Vec::new` 构造全对 | ✅ 已排除 | RawVec 新嵌套布局 cap@0/ptr@8/len@16 正确 |
| `with_capacity(2)+2 push`（不触发 grow）全对 | ✅ 已排除 | 无 grow 路径时正确 |
| `Ord::max/min` 本身降级正确 | ✅ 已排除 | maxmin 实验：`0.max(1)=1`、`0.min(1)=0`，maxmin=10 PASS |
| **`Vec::new`+push（触发 grow）错** | ❌ 错误现场 | cap=0（应 1）、len=5（应 1）——grow 链值错 |
| `vecwc2`（with_capacity(2)+2 push 触发 grow）**挂起 124** | ❌ 错误现场 | grow 链「值错与挂起」输入敏感交替——多路径错误 |
| 嵌套 niche 判别（`CF::Break(Err(5u8))` 应=7 现=1） | ✅ 已修复转正 | rvalue.rs 改 `codegen_get_discr` 算术公式（relative=raw-start；is_niche=relative ule max；discr=算术 select），nested_enum_break 用例转正 |
| 编译非确定性 | ⚠️ 诊断注意 | HashMap 顺序影响函数布局；reloc 用符号名不受影响 |

**结论**：嵌套 niche 修复后 vec_push **仍未转正** → 剩余问题不在判别读取，
而在 **grow 链的多路径实参值**（`grow_amortized` 的 new_cap 计算链）。

## 2. 根因收敛（十九轮定性后的候选）

`Vec::push` 触发 `RawVec::grow_amortized`，MIR 级链条：

```text
push → grow_one → grow_amortized(len, additional=1)
  → new_cap = cmp::max(2 * cap, required_cap)     ← 两个 max 调用（dest=12/16）
    → required_cap = len.checked_add(additional)   ← 候选 A
    → cap*2                                       ← 候选 B
  → alloc(new_cap) → finish_grow → copy_agg（sret：旧元素拷到新分配）
```

候选点（按优先级）：

1. **`[lower.Select]` 原生路径**（WA-17 已修规则层，x86/aarch64 已落库）：
   max/min 降级走 `icmp + select` 链。规则层修复后**待 regalloc 重叠检查
   验证**——若 Select 仍走无条件 true 臂的降级路径，max 的实参选择错。
   **最高嫌疑**（与「值错/挂起交替」的路径敏感性吻合：不同输入走不同
   select 分支）。
2. **grow 链 `finish_grow` 的 sret `copy_agg`**（e2e 注释记录）：高压 spill 下
   写 `[0]`——sret_ptr 值丢失——指向 **forge-codegen regalloc 的活区间/
   spill 决策 bug**（sret 指针 XReg 的 live range 跨 call 未保持）。
3. **`_17 = const 8/4/1` 候选**：grow 链中 size/align 常量实参错（非主流假设，
   与 `with_capacity` 路径正确矛盾）。

## 3. 技术路径（分步消元，每步独立验证）

### E1：最小复现指令级定位（前置，1 天）

README 限制区⑧ `probe_n n=5`（~40 行）最小复现。gdb 断点 + `FORGE_TRACE_LOWER/TERM/VCODE` 逐 MIR 语句比对：

- 在 `grow_amortized` 的两个 `max` 调用（dest=12/16）处断点，打印
  `cap`/`len`/`additional`/`required_cap`/`cap*2` 各实参的实际值；
- **确定错在哪条语句的哪个实参**（值错 vs 挂起两个现场各抓一次）；
- 产出：错值语句编号 + 实参来源（哪个 SSA 值）。

**出口标准**：定位到具体 MIR 语句 + 实参来源，E2/E3 据此定向。

### E2：`[lower.Select]` 原生化验证（1-2 天，最大嫌疑）

- 现状：WA-17 已修规则层（x86/aarch64），但 Select 可能仍走非原生路径；
- 动作：确认 `[lower.Select]` 生成原生 `icmp+select`（非无条件 true 臂）；
  若 regalloc 重叠检查已就绪，恢复原生 Select 路径；
- 重跑：`vec_push`/`vec_string`/`vecwc2`/`maxmin`/`nested_enum_break`。

**出口标准**：vec 三用例转正 → 收尾；仍错 → 进入 E3。

### E3：grow 链 sret copy_agg 的 spill 复现（2-3 天，regalloc 侧）

- 依据：e2e 注释「sret copy_agg 在高压 spill 下写 [0]——sret_ptr 值丢失」；
- 动作：构造高压 spill 小用例（大量存活值 + grow 链）复现 sret_ptr 丢失；
  检查 `regalloc_bt.rs` 的 sret 指针 XReg live range（跨 call 是否被
  clobber 覆盖、spill/reload 决策）；
- 修复：sret 指针的跨 call 保持（callee-saved 分配或显式 spill 保护）。

**出口标准**：高压用例下 sret_ptr 稳定；vec 三用例转正。

### E4：riscv 交叉验证（1 天，并行可做）

- 用 riscv 后端跑同一 grow 链用例（QEMU 执行）：
  - **riscv 也错** → lowering/regalloc 通用问题（E2/E3 修主库即可双端生效）；
  - **仅 x86 错** → x86 编码/regalloc 特有（检查 x86 的 sret/select 编码路径）。

### E5：转正与回归（0.5 天）

- `vec_push`/`vec_string`/`vecwc2`：`known_failure` → `false`（转硬断言）；
- 全量 e2e 58 用例 + 主库门禁（forge-codegen jit 全套、forge-tests、
  mini_c）——调用链改动必须全套守门。

## 4. 验收标准

- [ ] `cargo test -p forge-rustc --test e2e`：vec_push=2、vec_string=2、
      vecwc2 不挂起，3 用例转正（known_failure=false）
- [ ] `nested_enum_break` 保持绿（同源回归保护）
- [ ] 主库门禁全绿（forge-codegen jit / forge-tests 34+ / mini_c）
- [ ] 诊断环境变量说明更新（FORGE_TRACE_* 保留）

## 5. 风险与对策

| 风险 | 对策 |
| --- | --- |
| MIR 级调试投入不确定（历史 12+ 轮） | E1 限定 1 天产出「语句级定位」，超时改由 E2（规则层已修，验证成本低）推进 |
| 编译非确定性（HashMap 布局）干扰复现 | 诊断用固定函数布局（FORGE_TRACE 保留 + 单函数程序）；E1 的 probe_n 已最小化 |
| Select 原生化影响面大（全库 lowering） | E2 只验证路径选择（非全量重写），x86/aarch64 规则层已落库 |
| regalloc 改动波及 298 条 JIT 用例 | E3 修复后全量 JIT + forge-tests 矩阵回归 |

## 6. 一句话总结

**先 E1 定位（1 天）→ E2 验证 Select 原生化（最大嫌疑，1-2 天）→ 未转正则
E3 查 sret spill（regalloc 侧）→ E4 riscv 交叉定界 → E5 转正回归**。
E2 成功概率最高（嵌套 niche 修复后残留问题与 select 路径敏感性吻合）。

---

## 7. 诊断进展（2026-09 补充）

> e2e 环境已打通（`RUSTUP_HOME=target\rustup_home` junction + 
> `FORGE_E2E_NIGHTLY` 覆盖；`.rustup\tmp` 被 Defender 拦截的解法）。
> 工具：`FORGE_E2E_ONLY=用例名`（单用例）+ `FORGE_E2E_TRACE=1`（编译成功
> 也打印 stderr）+ `FORGE_TRACE_MIR/STMT/TERM/CALL/ABI/SLOT/STORE/LOAD/
> LOWER/CONST`（全链路降级 trace）。

### 已排除（主库全部验证正确，jit 79 绿）

| 路径 | 验证 |
| --- | --- |
| f64/fconst 常量实参 + XMM 槽 | test_jit_f64_const_arg_call ✓ |
| **i64/iconst 常量实参 + GPR 槽 + 跨函数 call** | test_jit_i64_const_arg_call ✓ |
| mixed int/float by-position 槽位 | test_jit_mixed_int_float_args ✓ |
| fcmp → select / branch / if-else | test_jit_fcmp_branch_if_else ✓ |
| f64 参数栈槽中转（Fstore/Fload） | test_jit_f64_param_via_stack_slot ✓ |
| 完整组合（fconst 实参 + branch callee） | test_jit_fconst_args_branch_callee ✓ |
| fib 递归 call / Select cmov（BOOL/I32 cond） | ✓ |
| Unreachable terminator（Trap 标签驱动） | test_jit_unreachable_terminator ✓ |

**主库 ABI 真 bug 已修**：Windows x64 by-position 槽位（int/float 共享
位置计数——`[abi].arg_slot = "by-position"`，588708a）。

### 当前定位

e2e 剩余 26 个值错（fib_recursive/multi_call_chain/nested_calls/float_args/
i64_wrapping_add/checked_tuple/alloc_* 等）+ vec_push compile failed——
**MIR/lowering 全链路 trace 显示降级形态正常**（fadd/fcmp/switchInt→branch/
iconst 常量实参/Fstore 栈槽中转全对），主库等价 IR 全部执行正确 → **值错
收敛为 forge-rustc 与 rustc-2026-08-07 的 monomorphized core 函数
（wrapping_add/alloc 等）符号/链接交互**（rustc 内部漂移，非主库——
主库 80 测试全绿，全部调用/常量/分支/栈槽路径排除）。

### 已排除清单（主库 80 测试全绿）

| 路径 | 验证 |
| --- | --- |
| i64/iconst 常量实参 + GPR 槽 + 跨函数 call | test_jit_i64_const_arg_call ✓ |
| **i64 参数 Store/Load 栈槽中转** | test_jit_i64_param_via_stack_slot ✓ |
| f64 参数栈槽中转（Fstore/Fload） | test_jit_f64_param_via_stack_slot ✓ |
| fconst 实参 / mixed by-position / fcmp→branch / 组合 / fib / select | ✓ |

### ✅ vec_push 转正（54399c9，2026-09）——e2e 57/58

**根因（E3 定位）**：ScalarPair 拆分 + 高压 spill 下，entry vreg 无 preg
（被 spill）时 move_args 跳过收参（`!assignments.contains_key → continue`）
——但 mod.rs 210-234 仍用 entry_params 写 locals 槽 → 槽读垃圾（grow
链 new_cap/指针值错 → copy dst=0 崩溃）。

**修复**：move_args 对 spilled 的寄存器参数（位置 < n，int 类）显式收参
到 spill 槽（load ABI 寄存器 → scratch → store 槽，`#spilled_int_receive`，
与栈参数中转同构）；`__pos` 移循环开头统一计算（sret 偏移一致）。

**验证**：vec_push（Vec::new+2 push 触发 grow）exit=2 转正（known_failure
移除）；jit 80 / forge-dsl 51 / forge-tests 36 / mini_c 162 全绿。
**vec_string 仍 known**（exit=0，len 读错）：`String::from(&str)` 走
sret 返回（String=Vec<u8> 24B）+ ScalarPair &str 参数——to_vec::<Global>
sret 返回链为下一候选。

### ✅ 已修复（2026-09 reloc/对齐/双返回三连击，e2e 30→51/58）
| 根因 | 修复 | 提交 |
| --- | --- | --- |
| **COFF reloc 隐式 addend**：编码器占位 -(f+1)/-(g+1) 作为隐式 addend 残留 → call 目标偏 -1（0x10d0 vs wrapping_add 0x10d1）、GlobalAddr 符号地址偏 -1（movabs 0x2fff vs .rodata 0x3000） | object_writer.rs：REL32/ADDR64/ADDR32 在 add_relocation 前清零被重定位字段；REL32 保持 addend-4 补偿（coff_adjust_addend +4 净 0 不覆盖） | 16bec80 |
| **Windows x64 栈 16 字节对齐**：prologue push rbp+7 callee-saved 后 rsp%16==8，sub rsp 需 ≡8(mod16)；缺省 padding 0 → 系统 DLL 内 movdqa 未对齐 SEGV（ffi_exit_process 0xC0000005） | `[abi].frame_padding = 8`（模型+生成器+TOML 声明） | 16bec80 |
| **ScalarPair 双返回**：Return/Call 只处理 results.first()（RAX），RDX 从未 mov → overflowing_add 的 (i32,bool) bool 读垃圾 → checked_destructure 返回 0 | lowering.rs：Return values[1]→RDX、Call results[1]←RDX（GPR64 index 1） | 16bec80 |

**e2e 现状 56/58**：栈参数实现（5dba34b）后 five_args_stack/eight_args_stack/
mixed_args 全部转正。剩余 2 known：vec_push（exit=-1073741819，运行期崩溃
——**阻塞线打通：从 compile failed 变为真实执行到 grow 链崩溃点**，E1 可
直接指令级定位）+ vec_string（exit=0，同源）。

### ✅ 栈参数（P2 ABI，5dba34b）——vec_push 阻塞线打通

| 组件 | 实现 |
| --- | --- |
| TOML | `[abi].stack_arg_shadow = 32` + MOV64_RM/MR 加 `stack_arg_load/store` 标签 |
| 调用方 | 第 5+ 参数 store [rsp+shadow+(k-n)*8]（标签驱动，不用指令名） |
| 被调方 | 从 [rbp+16+shadow+(k-n)*8] load 到 spill 槽（无条件，防共享寄存器批量覆盖） |
| 帧布局 | frame_size 并入栈参数区；spill 槽起始上移 stack_args |
| regalloc | 栈参数强制 spill（param_reg_count）；def 无死值可驱逐时 spill 自己（emission scratch 写入） |
| ArgClassKind | ArgClass.class String → 枚举（int/float/vector/other，serde lowercase） |

**vec_push 当前崩溃点（E1 起点）**：grow 链写 [0x10]（r11 从深栈槽读出
=0x10，空指针偏移）——grow_amortized 的 LAYOUT 聚合常量实参（ScalarPair
16B）传参路径为下一候选（`Alignment` 新 nightly 类型交互）。

### ✅ E1 增量（2026-09 栈参数后）

- **参数形态已排除**：f(i32, Layout, i32)=7 ✓、f(i64, i64, Layout, Layout,
  bool)=9 ✓——ScalarPair Layout 参数 + 栈参数组合完全正确（grow_impl_
  runtime 的 5 参数形态等价验证通过）。
- **崩溃收敛到函数体内**：grow_impl_runtime 调 __rust_realloc 后写
  [r11]，r11 从深栈槽 [-0x500(%rbp)] 读出为 0/0x10（垃圾值，运行间不同）
  ——**栈槽被写坏或读未初始化**。bt 显示返回地址栈被破坏。
- **下一候选**：regalloc **def-spill 占位 PReg(0)**（RAX）——emission 对
  spilled def 用 scratch 写入本应安全，但占位返回值与 active/assignment
  交互可能产生错位（需 FORGE_TRACE_SPILL 逐 spill 点核对槽地址）；或
  grow_impl_runtime 内 **Result<NonNull, AllocError> sret 返回**（16B
  聚合 + 栈参数混合场景）。

### 🔬 E1 深挖（gdb 崩溃现场，2026-09 续）

**崩溃现场**（vecpush.exe，ASLR 基址 0x7ff71bff0000）：
- 崩溃指令：`mov %r10,(%r11)`，r11=0（写 [0]）——**copy 目标指针为 0**
- 寄存器：r13=0x540000（**HEAP 静态数组基址**）、r14=0x15f008（栈）、
  rdi=4（Alignment=4）、rcx=0x10、r12=0x15f0c0（栈）、r8=0x540000
- 崩溃函数：0x5a5b 起的巨型函数（~0x2DA2 字节，含 grow_impl_runtime
  及其内联的 copy_nonoverlapping/write_bytes/UB 检查）
- 反汇编显示：`mov -0x6e0(%rbp),%r10; mov -0x500(%rbp),%r11; mov %r10,(%r11)`
  ——r11 从深栈槽读出=0 → **dst 指针槽值错**（应为新分配 0x540000+）

**推断**：grow_impl_runtime 内 copy_nonoverlapping 的 **dst = 新分配
指针**（0x540000 附近），但读出 0——new_ptr 未正确传播。候选：
① `__rust_realloc` 返回值（新 ptr）经 sret/双返回接收错位；
② Layout 参数（ScalarPair lo/hi）在函数体内 field 投影偏移错；
③ def-spill 的槽地址计算（sp_base 含 stack_args 后与 emission 不一致）。

**E1 验证已做**：参数形态等价用例（f(i32,Layout,i32)=7、f(5参数双Layout
+bool)=9）全对 → **参数传递层排除**，bug 在 grow_impl_runtime 函数体内
的指针/返回值传播逻辑。

### 🎯 E1 根因锁定（2026-09，写死 RAX × def-spill 交互）

**FORGE_TRACE_ALLOC 决定性证据**：grow 链指令大量形如
`LeaR64Sib { dest: RAX, mem: [RBP-256] }`、`MovRMem { dest: RAX }`、
`StoreMemR { src: RAX }`——**dest/src 写死物理 RAX**（clobber），且
xregs/defs 绑定普通 vreg（v154313 等）。regalloc 每指令驱逐 RAX 占用者
（victim next_use=None）→ **def vreg 反复 spill 到深槽**（slot 1144 等）。

**机制**：emission 对 spilled def 用 scratch 覆盖指令的 dest 字段
（set_reg_field），但**写死 RAX 的指令 encode 忽略字段、恒用 RAX** →
store 用 scratch（垃圾）→ **spill 槽写入垃圾**（崩溃槽 [-0x500]=1144
值 0 = v312692 def-spill）。Call 结果/地址计算链全部受影响。

**修复方向（下一轮）**：
1. **写死物理寄存器的指令不应把 dest 当普通 vreg def**——clobber_map
   已声明 RAX clobber，但 dest 字段的 vreg 绑定导致 spill 覆盖失效；
   lowering 层对 `Reg::from_index(0)` 绑定的 dest 应显式 precolored
   （vreg→RAX 固定，regalloc 不 spill）；
2. 或 emission 对**含写死物理字段的指令**禁用 scratch 覆盖（detect
   物理字段 + spilled vreg → 直接报错而非写垃圾）；
3. 或 def-spill 返回占位后，Call/LEA 等固定寄存器指令强制 result 在
   寄存器（不 spill）。

### 🔬 E2 反例（2026-09 续，locals × 栈参数交互）

**five_wb 反例**：`f(g: i64, p: i64, l1: Layout, l2: Layout, z: bool)` +
函数体局部数组 `buf: [u8; 16]` + write_bytes(buf, 0xAB, l1.size()) +
返回 buf[0] + l2.size() → **实测 171（want 179）**：l1.size()=8 对
（write_bytes 写 8 字节 ✓）、**l2.size()=0 错**（+8 缺失）。

**对照**：f=9 用例（同签名、无 locals）→ l2.size()=4 正确 ✓。

**结论**：**被调方有 locals（StackAddr）时，栈参数（位置 ≥ n）的
ScalarPair 值错**（l2 是第二个 Layout，位置 5/6 走栈）——locals 与
栈参数收参/帧布局交互（候选：move_args 的栈参数 load 地址、或
locals 槽与栈参数区在 frame 内重叠）。这与 vec_push 的
grow_impl_runtime（大量 locals + 双 Layout 栈参数）崩溃同源。

**下一步**：用 five_wb 最小反例（已可复现 171≠179）逐指令对比
l2.size() 的读取链（反汇编 f 的 prologue 栈参数收参 + l2 投影）。

### 🔬 E3 定位（2026-09 续，move_args 收参不完整）

**f 的反汇编**（five_wb，0x108d 起）：prologue 只收 2 个寄存器参数
（`mov rcx→r15; mov rdx→r14`）——**r8/r9（l1_lo/l1_hi）和栈参数
（l2_lo/l2_hi/z）完全未收**。f 有 7 个位置参数（ScalarPair 拆分），
但 move_args 只处理了前 2 个。

**原因链**：
1. `param_vregs`（entry XReg，ScalarPair 拆分后 7 项）与
   `param_is_float`/`param_by_ref`（按 param_xregs 构建，7 项）**长度
   一致** ✓；
2. 但 move_args 外层循环 `if !assignments.contains_key(&__pv) → continue`
   ——**entry vreg 无 preg（被 spill 或 dead）时跳过收参**；
3. f 内 l1_lo/l1_hi 的 entry vreg 被 spill（寄存器压力）→ 跳过 →
   **但 mod.rs 210-234 仍用 entry_params 写槽** → 槽读垃圾；
4. **l2（位置 4/5 栈参数）依赖 move_args 的栈 load**——若该 vreg 也无
   preg → 栈 load 分支（要求 spill_slots 有槽）可能走或不走。

**待解问题**：为何 l1（寄存器，r8/r9）值对而 l2（栈）值错？以及
spill 的 entry vreg 如何正确收参（move_args 需把 ABI 值写入 spill 槽，
而非跳过）。

### 下一步候选（按优先级）

1. **vec_push E1 启动**（阻塞线已通）：跨函数 call reloc 已修，grow 链
   的 monomorphized core 函数（RawVec::grow_amortized 等）现在能正确链接
   执行——按 §3 E1 最小复现指令级定位 grow 链值错/挂起；
2. **Windows x64 栈参数**（P2 ABI）：five_args_stack/eight_args_stack/
   mixed_args 转正（第 5+ 整数参数走栈 + shadow space）；
3. **box_value/box_write**：#[global_allocator] 语义（后端注入 __rust_alloc
   无法替代前端要求）。

E1-E5（grow 链定位/Select 恢复/sret spill/转正）在此线之后执行。
