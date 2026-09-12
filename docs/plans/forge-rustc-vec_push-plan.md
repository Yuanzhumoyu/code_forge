# forge-rustc vec_push/vec_string 完整改进方案

> 对应 `docs/archive/roadmap-status.md` 剩余事项 2 与 `crates/tools/forge-rustc/WORKAROUNDS.md`
> [WA-11]。e2e 58 用例中 2 个预期失败（`vec_push` SEGV、`vec_string` len 错）
> 与派生现象（`vecwc2` 挂起 124）。十二轮深挖已收敛根因范围，本文给出
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
sret 返回（`String=Vec<u8>` 24B）+ ScalarPair &str 参数——`to_vec::<Global>`
sret 返回链为下一候选。

### 🔬 vec_string/vecfrom 定位（2026-09 续，sret_ptr 传 0）

**vecfrom 最小反例**：`Vec::from(&[1u8,2,3][..])` → exit=0（want 3）。
反汇编：mainCRTStartup 调 to_vec（sret 返回 Vec）时 **sret_ptr 传 0**
——`mov r14→rcx` 其中 r14=[-0x70] 的值（0），而非 `lea [-0x70(%rbp)]`
（槽地址）。

**候选**：sret_setup（lowering.rs 1052-1078）生成 LEA 槽地址 → MOV 到
RCX，但 **sret 地址 vreg 与参数搬移 vreg 被 regalloc 分配同寄存器 r14
→ 顺序覆盖**（与 vec_push 的 spilled 参数同类：sret 地址 vreg 的 live
range 跨搬移未保持）。修复方向：sret 地址 vreg 的 live range 修复或
move_args 前强制存活（与 #spilled_int_receive 同思路）。

### 🔬 vec_string 深挖（2026-09 续：Slice 常量 + PtrMetadata）

**strlen 最小反例**：`let s = "hi"; s.len()` → exit=0（want 2）。两步根因：

1. **Slice 常量未落盘**：`_2 = const "hi"`（`ConstValue::Slice{alloc, meta}`）
   是 &str 字面量——statement.rs 的聚合分支 `eval_const_bytes`（16B 字节
   展开）对 Slice 求值失败 → 0。**修复**：Slice 分支写槽
   `ptr@[base] = global_addr(alloc)`、`len@[base+8] = iconst(meta)`，并登记
   alloc 字节到 rodata（FuncRefTable::intern_promoted + backend.rs 落盘）。
   验证：strlen=2 ✓（连同 statement.rs 的 is_ref_const 引用守卫）。
2. **PtrMetadata 恒 0**：`str::len` 的 MIR 是 `_0 = PtrMetadata(_2)`——
   rvalue.rs 的 UnOp::PtrMetadata 恒返回 0（注释"thin 指针"）——fat
   pointer（&str）的 metadata 是 len。**修复**：`fat_ptr_metadata(place)`
   读槽 [base+8]。验证：strlen=2 ✓。

**⚠️ 回归教训（vec_push）**：Slice/promoted 落盘改动（func_ref.rs
intern_promoted + backend.rs 落盘 + rvalue.rs promoted 分支的
`GlobalAlloc::Memory` 拦截）**导致 vec_push 回归**（grow 链挂起，编译
>10min）——`GlobalAlloc::Memory` 分支过宽：拦截了**所有非 Static 的
const**（含 Layout 聚合），LAYOUT 被当 promoted 引用处理 → 值错。
需加**类型守卫**（仅 `TyKind::Ref/RawPtr` 走 static/promoted）并逐个
验证（vec_push 转正后回退，e2e 保 57/58；vec_string 修复留待专项，
改动需在 vec_push 用例上先行回归）。

### ✅ vec_string 转正（69afd24 后续，e2e 58/58）

**最终修复（三处，均为类型守卫 + 落盘）**：

1. **Slice 落盘（statement.rs）**：`_x = const "hi"`（`ConstValue::Slice`）
   → 写槽 `ptr@[base]=global_addr(alloc)`、`len@[base+8]=iconst(meta)`，
   并 `intern_promoted` 登记 alloc 字节到 rodata（backend.rs 落盘）。
2. **Slice 实参（mod.rs）**：`String::from("hi")` 的 &str 实参是 Slice
   → 拆 `global_addr(ptr)` + `iconst(len)` 两个标量传（eval_const_bytes
   对 Slice 返回 None，退化 0 会传空指针）。
3. **PtrMetadata（rvalue.rs）**：`str::len` 的 `_0 = PtrMetadata(_2)` →
   `fat_ptr_metadata(place)` 读 fat ptr 槽 [base+8]（&str/&[T] 的 len）。

**验证**：vec_string=2 ✓、vec_push=2 ✓（类型守卫避免 Layout 误判）、
strlen=2 ✓；jit 80 / forge-dsl 51 / forge-tests 36 / mini_c 162 全绿。

**vecfrom 残留**（`Vec::from(&[1u8,2,3][..])`，exit=0 want 3）：`[1,2,3]`
是 **promoted 数组引用**（`_3 = const mainCRTStartup::promoted[0]`，
&[u8;3] → Index → &[u8]）——Unevaluated 常量的 promoted 路径未走
（statement.rs 的 Slice 分支只匹配 Slice；promoted 引用是 Ptr/Indirect
且 Unevaluated 需先 eval）。下一轮：处理 Unevaluated 引用的
promoted/static 路径。

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

---

## 8. 2026-09-10 复核：E1 假设未复现 + 已落地加固（当前状态以此节为准）

**触发**：2026-09-06（`b56225d` + `b836a78`）把 5 个 vec/alloc 用例重新标为
`known_failure` 并列入 `FLAKY` 双向容忍名单（`crates/tools/forge-rustc/tests/e2e.rs`），
本文 §1–§7「已转正 / e2e 58–85 全绿」的表述随之失效。§1–§7 作为历史记录保留，
本节给出**当前实测状态**与结论。

### 8.1 本机实测（Windows x64，Alder Lake：**无 AVX-512F**）

| 验证项 | 命令/口径 | 结果 |
| --- | --- | --- |
| 全量 stage_a | `cargo test -p forge-rustc --test e2e -- e2e_stage_a_scalar_cases` × 2 轮（改动前） | **103/103 通过**（两轮一致）；5 个 FLAKY 用例全部以正确 exit 通过：`box_value`=42、`vec_push`=2、`vec_string`=2、`vec_from_slice`=3、`vec_iter_enumerate`=80 |
| 单用例 `vec_push` × 5 轮 | `FORGE_E2E_ONLY=vec_push` | 5/5 `FLAKY-PASS` exit=2（无 AV 无挂起） |
| spill 现场（决定性证据） | `FORGE_TRACE_ALLOC=1 FORGE_TRACE_SPILL=1`（19782 行 trace） | def-spill 36 处；与指令关联成功的 **32 处全部**是 `MovRegImm64 { dest: RAX }` / `MovRMem { dest: RAX, src: RAX }` / `XorRmR { dest: RAX }` 这类 **dest 占位为物理 RAX 但字段可被 `set_reg_field` 改写**的值字段（`map_reg_field` 绑定） |

**§E1 假设判定（写死物理寄存器 × def-spill → 静默写坏槽）**：在当前代码 + 本机上
**未复现**——32/32 关联 def-spill 都落在可改写字段上（emission 会把字段重设 scratch，
指令实际写入与 store-back 一致 ⇒ 值正确），且 5 用例 ×7 轮全绿。
⇒ 5 个 `known_failure` 标记**保持不动**（无修补证据不放水）；若 CI 仍偶发，按既有
做法导出 `target\tmp\logs_*` 取证比对（同类先例：`cli_tests` 的 alloc AV 最终以
artifact 双证判定为 CI runner 环境性，见提交 `c545aaa`/`80d552d`）。

### 8.2 已落地加固（把「假设中的静默面」永久收口）

即使假设未复现，其失败形态（静默写坏 spill 槽）属于 must-not-happen，已按
fail-closed 收口：

1. **`MachineInst::is_reg_field_settable`**（`machine/inst.rs`，新 trait 能力，
   默认 `true` 保持手写 machine 兼容）；DSL 生成器（`forge-dsl .../codegen/machine.rs`）
   按变体的 Reg 操作数表精确产出——固定物理字段（不参与 `map_reg_field`）返回 `false`。
2. **regalloc fail-closed 守卫**（`pipeline/regalloc_bt.rs`，标记 §1.9）：spilled def
   落在不可改写字段时返回 `IrError::RegAlloc`（消息含 WA-40 编号），不再让
   emission 命中 `_ => {}` 静默 no-op。
3. **回归测试**：`test_def_spill_on_settable_field_ok`（正向：合法 def-spill 仍允许）、
   `test_def_spill_on_fixed_field_errors`（守卫生效，断言错误文案）。
4. **`AllocResult.param_bytes`**（IR 类型字节数，分配后由 `CompileState` 填充）——
   供 by-ref 收参按真实字节宽分派（见 `docs/archive/ymm-abi-plan.md` D5）。

### 8.3 未关闭项

- **5 个 FLAKY 用例**：保持 `known_failure`，转正标准仍为各 `reason` 所写
  （3 轮 stage_a + parallel 全绿且 exit 正确）。**2026-09-10 已定性**：8 并发负载下复现
  的是**运行期瞬态超时**（宿主侧起进程/映像延迟）——失败轮产物语义正确、同产物复跑即
  通过，**非错码、非错编译**；5 例 `reason` 均已写入该证据（详见 §9.2）。
  例外记录：`vec_push` / `vec_iter_enumerate` 历史 reason 里的 **AV（0xC0000005）** 形态
  本轮**未复现**，不能据此归因（保持原描述）。
- **sret 地址 vreg live range**（§E3 第 2 项、「🔬 vec_string/vecfrom 定位」）：
  **未实施**；`vec_from_slice` 在本机通过（exit=3），故无复现证据，暂缓；一旦复现，
  按该节「move_args 前强制存活」方向实施。

### 8.4 用例计数口径（历史文档已多处漂移，以此为准）

`tests/e2e.rs` 的 `CASES` 现为 **103 项**（98 硬断言 + 5 个 `FLAKY` 容忍项）。
历史值 58（本文 §5）/81（e2e.rs 旧头注）/85（README 旧文）/96（WA-36 行）均已过时。

## 9. 修复方案（5 个 FLAKY 用例转正 + sret live range）

**现状定级（2026-09-12 收官）**：本计划的实现工作（Slice 落盘 / PtrMetadata / Unevaluated promoted /
`spilled_int_receive` / ScalarPair 双返回 / 栈参数 / `frame_padding` / 原生 Select / 宽向量常量）
**均已落地**；§9 的两件悬案也已终结——5 例的机型相关 AV **根因 = WA-42**（niche 枚举 tag
写入宽度），修复后**全部转正**（`FLAKY` 清空 + `known_failure=false`），另一个"未复现的假设
（sret live range）"**未触发即作废**（§9.4）。收尾 CI（run 93934196718）**11 个 job 全绿**。

### 9.1 步骤 1（前置，需要 CI 侧证据）：失败产物与 trace 对照

本机为**低频复现**（8 路并发负载下 ≈0.3%，见 §9.2）⇒ 终局证据仍应取自 CI 失败 run。
**原文此处假设"沿用既有机制：日志与产物 artifact"——2026-09-10 核查不成立**：
`forge-rustc-e2e` job 当时既无 `tee` 落盘、也无 `if: failure()` 上传（全仓库只有
clippy job 有上传），失败即随 runner 销毁。已补（`.github/workflows/ci.yml`）：e2e 步骤改为
`… | Tee-Object ${{ runner.temp }}/e2e.log`、设 `FORGE_E2E_KEEP=1`（失败轮工作目录不删），
并新增失败时上传 `e2e-evidence`（日志 + `forge_rustc_e2e_*` 工作目录）。

1. 取 CI 失败 run 的 `e2e-evidence` artifact（失败用例的 `.rs`/`.exe` 与日志里的
   `KNOWN <case> error: …` 文案）；
2. **比对判据**：把 CI 产物的 `.text`（或 forge 写出的 `.o`）与本机同源编译产物做
   逐字节比对（`llvm-objdump` 归一化指令序列 + 对象字节）：
   - **字节一致而 CI 崩** ⇒ 判为 **CI runner 环境性**（同类先例：`cli_tests` 的
     alloc AV 最终以 artifact 双证判定为环境性，提交 `c545aaa`/`80d552d`）→ 进入
     步骤 3；
   - **字节不同** ⇒ 编译行为在 CI 与本机有别（工具链/宿主差异）→ 进入步骤 4；
3. 复现失败 run 侧再取 `FORGE_TRACE_ALLOC=1 FORGE_TRACE_SPILL=1`（e2e 通过
   `FORGE_E2E_TRACE=1` 打印）与 `FORGE_TRACE_MIR/STMT/TERM`，按 §8.1 的关联法
   逐条核对 def-spill 站点（脚本口径：把 `[spill-def] vN at ProgPoint(P)` 与
   `[inst ProgPoint(P)] …` 配对，检查该字段是否 `MovRegImm64/MovRMem/XorRmR`
   这类**可改写**字段）。

### 9.2 步骤 2：本机加严复跑与取证（**已执行，2026-09-10**）

脚本：`crates/tools/forge-rustc/tests/e2e_flake_repro.ps1`（`-Mode hammer`：5 轮全量
stage_a + 并行变体 + 5 用例各单跑 ×10；`-Mode load`：N 个并发 worker 直接跑 e2e 测试
二进制、对单用例制造负载直到复现）。失败即写 `target/tmp/*_fail_*.txt`，并把 `KEEP`
保留的失败工作目录**复制回工作区**。脚本落仓库内（而非 `target/tmp`）是因为后者会随
会话轮换被清空——2026-09-10 19:45 实测：当时的 hammer/复现脚本全部丢失。

**实测结果（两轮 hammer，合计 13 轮 stage_a）**：

| 项目 | 结果 |
| --- | --- |
| 全量 stage_a | **v1 5 轮 + v2 5 轮 + 早前 3 轮 = 13 轮，轮轮 103/103**；5 个 FLAKY 用例每轮均以正确 exit 通过（42/2/2/3/80） |
| 并行变体 `e2e_parallel_pool_threads` | 3 次全 PASS（`T=1 == -Z threads=2`） |
| `vec_push` / `vec_string` / `vec_iter_enumerate` / `box_value` | 各 10/10（`exits=[2…] / [2…] / [80…] / [42…]`） |
| `vec_from_slice` | **v1 出现 1 次失败（1/10，连续重负载序列中）**；v2 10/10；随后定向复跑 30（带 trace）+ 40（无 trace）全过 ⇒ 该序列口径 **80 次单跑 1 次失败**（≈1/80；8 路并发负载口径见下文 ≈0.3%） |

**失败形态（已定位到"运行期挂起"，2026-09-10 事后取证）**：

1. v1 记录行是 `vec_from_slice : pass=9 fail=1  exits=[3,3,3,3,3,3,3,3,3]`——10 次里
   **只有 9 个退出码**。v1 脚本按 `$c\s+exit=` 抓取，若失败轮输出 `KNOWN vec_from_slice
   exit=N (want 3)` 必被计入 ⇒ 该轮输出的是 `KNOWN vec_from_slice error: …`，即 harness 的
   `Err` 分支，**不是错误退出码**。（文案当时未落盘——v1 脚本只写聚合行；v2 已修。）
2. **产物补证**：失败轮的工作目录 `%TEMP%\forge_rustc_e2e_37184`（时间戳 18:40:38，正落在
   v1 定向循环时段）**至今仍在**，且只有 `vec_from_slice.{rs,exe,pdb}` 三件。当时
   `FORGE_E2E_KEEP` **尚不存在**（harness 支持于 18:41:47 才写入源码、18:45:43 才提交
   `8a05d23`；hammer 脚本加该变量更晚）⇒ 目录能留下来只能是 `remove_dir_all` **失败**：
   只有**超时路径**会在 `kill()` 子进程后立刻清理（被强杀进程的 `.exe` 映像尚未释放 →
   Windows 删除遭 sharing violation，而 `let _ =` 吞掉错误）；编译失败无 exe、
   起进程失败无锁，二者都留不下三件套。
   对照实验佐证：**通过**的单用例轮不留目录（`failed_cases` 为空即删除，实测复现）。
3. **产物正确性**：那一轮的 `vec_from_slice.exe` 事后独立复跑 **5/5 `exit=3`**（正确值）
   ⇒ 该轮**编译成功且产物语义正确**，失败落在运行期（15 s 未退出），与本仓库 CI 记录的
   "偶发 timeout（挂起）"同族。

**负载复现（2026-09-10 20:08，已当场留证）**：`-Mode load -Workers 8 -Rounds 40`
（8 个并发 worker 直接跑 e2e 二进制、单用例 `vec_from_slice`、`FORGE_E2E_KEEP=1`）
**第 1 轮即复现**，首次拿到失败文案：

```text
FAIL worker2 run1 dt=17364ms :: KNOWN vec_from_slice error: timeout (挂起：可能 assert
失败进入 panic loop) [P6 alloc] :: keep=…\forge_rustc_e2e_15608
```

失败轮产物已复制到 `target/tmp/e2e_timeout_evidence/`（`vec_from_slice.{rs,exe,pdb}`，
exe SHA256 `d227066e…315c14`）。**对该产物独立复跑 20 次：20/20 `exit=3`，每次 91–368 ms**
⇒ 按 §9.1 判据落入"行为一致而失败"一支 = **运行期环境性**（宿主侧起进程/映像就绪延迟：
新写出的 `.exe` 在 8 路并发嵌套编译下未能在 15 s 内跑起来），**不是错码、也不是该用例的
错编译**。负载阈值旁证：同一脚本 4 并发 × 30 轮 = **120 次全绿**（单轮 avg 1.86 s、
max 2.56 s、无一 > 5 s）——即需要 8 路负载才触发。

**主库侧加固（2026-09-10，同轮落地）**：harness 在超时后用**同一产物复跑一次**
（`run_exe_with_timeout` + `TIMEOUT_MARKER`）：真挂起（产物确定性 panic loop）两次都会
超时、照常上报；宿主侧延迟则被这次复跑吸收，并打印 `RETRY <case> …` 留痕。
超时阈值可用 `FORGE_E2E_TIMEOUT_SECS` 覆盖（诊断用）。

**复现的分布（累计 5 个窗口，每窗口 8 worker × 40 轮 = 320 次单跑）**：

| 用例 | 观察到的瞬态超时 | 处理结果 |
| --- | --- | --- |
| `vec_from_slice` | 窗口 A（尚未加复跑）1 次硬失败；窗口 B 4 次 | A 用产物对照定性（20/20 `exit=3`）；B 全部由"同产物复跑"转为通过 |
| `box_value` | 1 个窗口内 4 次 | 全部由复跑转为通过 |
| `vec_push` | 3 个窗口共 1 次 | 复跑通过 |
| `vec_string` | 3 个窗口共 1 次 | 复跑通过 |
| `vec_iter_enumerate` | 3 个窗口共 1 次 | 复跑通过 |

⇒ **5 例全部**观察到同一瞬态超时（合计 ≈3.5k 次单跑内 11 次，量级 ≈0.3%），**没有一次
是错码**，且除开场那次未加复跑的硬失败外全部被"同产物复跑"吸收。超时整轮 16–18 s
（≈15 s 超时 + ≈2 s 嵌套编译）后复跑即通过，与"产物错码 / 确定性挂起"不符，指向
**宿主侧运行期延迟**（8 路并发下的起进程 / 映像就绪 / 扫描排队）。上文 4 并发对照
（120 次全绿、单轮 max 2.56 s）说明需要 8 路负载才容易触发。

**结论（2026-09-10）**：这 5 例的偶发失败是**运行期瞬态超时**（宿主侧），**不是错码、
也不是该类用例的错编译**——已用"失败轮产物 20/20 正确退出"与"同产物复跑即通过"两条
独立证据支持。

**5 例仍保持 `known_failure`（不放水）**：本轮拿到的是"超时=环境性"的证据，不是"用例
产出正确"的通用证明——`vec_push` / `vec_iter_enumerate` 历史上记录的 **AV（0xC0000005）**
形态本轮并未复现，不能据此归因。5 例的 `reason` 均已写入本轮证据；转正仍需"3 轮
stage_a + parallel 全绿且该用例 exit 正确"，且 CI 侧不再出现超时假败（错码仍会照常被
`FLAKY`/`KNOWN` 捕获）。

**CI 实证（2026-09-10）**：推送 `e50cc3e` 触发 run #21（<https://github.com/Yuanzhumoyu/code_forge/actions/runs/34479152950>）——
**11/11 job 全绿**；其中 `forge-rustc (e2e, Windows)` 的 "Run e2e (stage A + M4 parallel)"
步骤 **success**（3m34s），新增的 "Upload e2e evidence (on failure)" 按预期 **skipped**
（无失败）。即：超时同产物复跑 + `tee` 日志 + `FORGE_E2E_KEEP` 的取证链路已在 CI 生效，
本轮 CI 未出现超时。**单轮全绿 ≠ flake 消失**（转正标准见 §9.3）。

**下一步（未闭环）**：

- CI 侧观察：本轮已具备 `e2e-evidence` 上传（§9.1）+ 超时同产物复跑，下一次偶发应能
  直接给出 `RETRY`/`KNOWN` 文案与产物；
- 若出现**两次都超时**（真挂起）：按 §8.1 关联法查 def-spill 站点（转 §9.4）；
- 若仍只有单次超时：累计若干轮 CI 全绿后按 §9.3 转正；
- **门禁收紧候选（待定，需用户/CI 观察后再做）**：现状 `FLAKY` 对 5 例**任何**失败
  （含错码）都只打 `KNOWN` 不致命 ⇒ 这 5 例的**正确性其实没有门禁**。既然本轮已把
  "超时"定性为环境性，可考虑把容忍范围缩到**只容忍超时**（`Err(TIMEOUT_MARKER)`，
  且已有同产物复跑兜底）、**错码（`exit=N (want M)`）恢复为硬失败**——这样 5 例的
  正确性重新进门禁。风险：历史上 `vec_push`/`vec_iter_enumerate` 记录过 AV 形态，
  若 AV 也是宿主侧偶发，CI 会重新变红（那也正是需要的信号）。

**复跑前置（环境）**：2026-09-10 会话轮换清空了 `target/`，且钉版工具链缺 `rustc-dev`
（`cargo test -p forge-rustc` 报 13 个 `can't find crate for rustc_abi/…`），需
`rustup component add rustc-dev --toolchain nightly-2026-09-05`；`rust-src` 会与既有
`lib\rustlib\src\rust\library\.cargo\config.toml` 冲突，须单独安装。

### 9.3 步骤 3（若判定为环境性）

**本节判定已在 §9.2 成立（运行期瞬态超时，非错码/错编译）**，据此推进的部分：

- `reason` 改写（**已完成**）：5 例均已写入本轮证据——`vec_from_slice`（复现 → 产物
  20/20 正确 → 同产物复跑通过）、`box_value`（一个窗口内 4 次瞬态超时、全部复跑通过）、
  `vec_push` / `vec_string` / `vec_iter_enumerate`（各 1 次瞬态超时、复跑通过；其中
  `vec_push` / `vec_iter_enumerate` 的历史 **AV 形态本轮未复现**，已在 reason 中注明
  不能据此归因）；
- harness 侧加固（**已完成**，见 §9.2）：超时后同产物复跑一次 + `RETRY` 留痕 +
  `FORGE_E2E_TIMEOUT_SECS`；这是把"环境性超时"与"真挂起"分开的判据（两次都超时 =
  真挂起，照常上报）；
- CI 取证（**已完成**，见 §9.1）：`forge-rustc-e2e` 现在 `tee` 落盘 + 失败上传
  `e2e-evidence`（日志 + 保留的失败工作目录），把每次偶发都变成可对照的证据；
- **仍未做（等更多 CI 轮次）**：`crates/tools/forge-rustc/README.md` 支持矩阵节的
  环境性记录——本轮只拿到"CI 单轮全绿 + 取证链路生效"（run #21），样本不足以把
  "本机 8 路负载 ≈0.3%、宿主侧瞬态超时"外推成 CI 结论；待续若干轮 CI 全绿、
  或 CI 出现一次 `RETRY` 样例（新链路会直接留证）之后再写。
  **→ 2026-09-12 改口径**：AV 那半边已定性为**真缺陷（WA-42）**而非环境性（`.text`
  相同、差的是栈残留），故 README 支持矩阵**不再**为它留"环境性"记录；只有"宿主侧
  瞬态超时"是环境性的，已由 harness 的同产物复跑 + `RETRY` 留痕吸收（§9.2）。

### 9.4 步骤 4（若判定为编译行为差异）

**→ 2026-09-12：本步骤未触发、前提作废。** §9.1 第 2 条的判据没有走到"字节不同"分支
——CI 两台 runner 与本机产物 `.text` **逐字节相同**（`a91b9eef…63fb` / `5f421f08…c732`）；
根因是 **WA-42**（niche 枚举 `None` 的 tag 只写 4 字节 → 高 4 字节残留栈垃圾）——同一份
机器码在不同机器上因**栈残留内容**不同而表现不同（§9.7）。下列两条假设因此都未被证实，
保留作历史参考。

按 §8.1 的关联法定位后，优先怀疑并验证两条线（本计划历史结论的延续）：

1. **并行路径**：`FORGE_CODEGEN_THREADS>1` + `-Z threads>=2` 下函数任务粒度对
   regalloc 决策的影响（determinism 测试已覆盖**产物布局**，但**未**覆盖 regalloc
   决策路径本身）——做法：同源在 `T=1` 与 `T=4` 下导出 `FORGE_TRACE_ALLOC` 并 diff
   分配结果，若不一致即定位分配器的迭代序依赖（HashMap → 排序）；
2. **sret 地址 vreg live range**（§E3 第 2 项）：构造"跨 call 的 sret 指针 + 高压
   spill"最小用例（主库 JIT 级，可本机跑），复现后按"`move_args` 前强制存活 /
   显式 spill 保护"实施（`forge-dsl .../codegen/frame.rs` 收参 + regalloc 活区间）；
   若主库级最小用例无法复现，则问题在 forge-rustc 生成的 IR 形态，转为 IR 级对照。

### 9.5 步骤 5：转正判定（不放水）

**判据**（各 `reason` 所写）：每例单独判定——**本机 5 轮 + CI 3 轮 stage_a+parallel
全绿** ⇒ 翻转 `known_failure:false` 并从 `FLAKY` 摘除（一例一提交，便于回归定位）；
任一轮失败 ⇒ 保留 FLAKY 并回写 trace 证据到本节。

**转正的真正含义**：`known_failure + FLAKY` 意味着该用例**任何**失败（含错码）都只打
`KNOWN` 不致命 ⇒ **正确性没有门禁**；转正后错码与"双次超时"都会硬失败。所以转正不是
"宣布已修"，而是**把该用例重新纳入门禁**——§9.2 的门禁收紧候选即由此而来。

**就绪度证据（2026-09-10 实测）**：

| 维度 | 证据 |
| --- | --- |
| stage_a 全量 | 本机 15 轮 103/103（v1 5 + v2 5 + 早前 3 + 本轮 2）；CI run #20/#21/#22 各 1 轮全绿 |
| parallel 变体 | 本机 3 次 PASS；CI 每轮同一 job 内通过 |
| 单用例正确性 | 常规口径 ≈3.5k 次单跑 + **严格口径 1600 次**（`FORGE_E2E_STRICT_FLAKY=1`：5 例 × 8 并发 × 40 轮）⇒ **错码 0 次** |
| 失败模式 | 只有宿主侧瞬态 15 s 超时（常规 ≈0.3%、严格口径 5/1600），**每次同产物复跑即通过**；真挂起（双次超时）仍会照常上报 |
| 静默写坏槽假设（WA-40 E1） | 未复现；fail-closed 守卫 + 两条回归测试已落地（见 §9.6） |

**严格模式开关**：`FORGE_E2E_STRICT_FLAKY=1` 只改 FLAKY 用例的**错码**判定（超时仍容忍、
仍由同产物复跑兜底），**默认关闭 ⇒ CI 现有行为不变**。它把"这 5 例的正确性是否已可
进门禁"变成可执行问句：本轮 1600 次单跑无错码 ⇒ 就绪度证据充分。

两条已实测的使用注意：

- 两条分支都验过（把 `vec_push` 的 `expected` 临时改成 99）：容忍模式打印
  `KNOWN vec_push exit=2 (want 99)`（不致命），严格模式打印 `FAIL …` 并计入
  `unexpected failures` ⇒ 硬失败；
- 用 `FORGE_E2E_ONLY=<失败用例>` 单跑时，即便容忍模式套件也会红——失败来自末尾的
  健全性守卫 `assert!(passed > 0, "no cases passed — backend broken")`，与 FLAKY
  容忍无关。因此严格模式评估应跑**全量** stage_a（103 例），不要只看单例。

**建议的转正路径（需人批准，本轮未执行）**：

1. 先让 CI 跑一轮严格模式（把 `FORGE_E2E_STRICT_FLAKY=1` 加进 e2e 步骤，或手动触发）
   观察 1–2 轮：出现错码 ⇒ 直接得证"仍需修"，回 §9.4；
2. 严格模式 CI 连续 2 轮全绿 ⇒ 5 例 `known_failure: false`、从 `FLAKY` 摘除，并把
   "超时容忍"留在 harness 的复跑逻辑里（不再靠用例白名单）；
3. `reason` 改写为"已转正（日期）+ 转正依据"；保留 `e2e-evidence` 上传备复盘。

#### 严格模式 CI 试运行 → 抓到 CI 稳定 AV（2026-09-10/11）

- run #25（`f7caaaa`）、run #26（`605ce0a`）严格口径均红，失败步是同一处
  "Run e2e (stage A + M4 parallel)"；而非严格口径 run #20–#24 连续 5 轮"全绿"。
- **失败文案（run #26 日志，2026-09-11 取得）**：

```text
FAIL  vec_push           exit=-1073741819 (want 2)
FAIL  vec_iter_enumerate exit=-1073741819 (want 80)
[keep] 失败用例 ["vec_push","vec_iter_enumerate"] —— 工作目录保留：
       C:\Users\RUNNER~1\AppData\Local\Temp\forge_rustc_e2e_7668
=== e2e 汇总 ===
passed: 101/103
test result: FAILED. 5 passed; 1 failed; … finished in 51.84s
```

- `-1073741819` = `0xC0000005` **ACCESS_VIOLATION**。同轮另外 3 个 FLAKY 用例
  （`vec_string` / `vec_from_slice` / `box_value`）均 **FLAKY-PASS**（exit 正确）。
- **门禁问题被证实**：容忍口径下这两条会打成 `KNOWN <case> exit=-1073741819`
  （非致命），套件照旧 `passed: 101/103` 且**退出码 0** ⇒ 历史那些"CI 全绿"里，
  这 2 例**一直在 AV**（各 `reason` 早已记录同一签名），**FLAKY 把它们的所有失败
  都吞掉了**。严格模式只是把既存事实变成红灯——这正是本轮加开关的目的。
- **本机对照基线**（同一 harness、同一钉版工具链构建；产物留 `target/tmp/local_ref/`）：
  `vec_push.exe` 94720 B、`.text` SHA256 `a91b9eef…63fb`，独立复跑 **exit=2**；
  `vec_iter_enumerate.exe` 103936 B、`.text` SHA256 `5f421f08…c732`，独立复跑
  **exit=80** ⇒ 同源在本机产物**正确**（与 §9.2 的结论一致：本机不是错编译）。
- **取证链路缺陷（本轮已修）**：`${{ runner.temp }}`（`D:\a\_temp`）≠ 测试进程的
  `%TEMP%`（`C:\Users\RUNNER~1\AppData\Local\Temp`）⇒ #25/#26 的 artifact 只收到
  `e2e.log`（4056 B），**保留的失败工作目录没上传**。现改为按 `$env:TEMP` 收集：
  打印每个失败产物的 `.text` SHA256（跨机可比指纹，`pe_text_hash.ps1`）并把
  `.rs/.exe` 复制到 `target/tmp/e2e_keep/` 一并上传。
- **本机 codegen 确定性对照（2026-09-11）**：这两个用例各**连续重建 3 次**，`.text`
  SHA256 三次全同（`vec_push` `a91b9eef…63fb` ×3、`vec_iter_enumerate` `5f421f08…c732`
  ×3）⇒ 本机这条路径逐字节确定：CI 指纹**相同**即可判环境性、**不同**即 CI 侧
  codegen 差异（转 §9.4）。
- **判定完成（2026-09-11，CI run #27 日志）**：CI 侧指纹与本机**逐字节相同**——

| 用例 | 大小（CI = 本机） | `.text` SHA256（CI = 本机） |
| --- | --- | --- |
| `vec_push` | 94720 B | `a91b9eef72b947006551c4bf1af44df3f03becc2602710caeb1c24c4939663fb` |
| `vec_iter_enumerate` | 103936 B | `5f421f08ef7457898a07481ad6d5cfc95bbd03520a97f2cfe2c612f1d10cc732` |

  ⇒ **同一份机器码在 CI 上间歇 AV**（run #25/#26/#27/#29 命中、run #28 未命中），
  在**本机稳定正确** ⇒ 判定 **CI runner 环境性 AV**（同 `cli_tests` alloc AV 的产物
  双证先例 `c545aaa`/`80d552d`），**不是 codegen 缺陷**；同时反证这两例的 codegen
  跨机器逐字节确定（本机 3 次重建亦全同）。

- **机制探针（2026-09-11 起每轮无条件运行）**：run #31 实测 ——
  `[probe] vec_push 20 runs: AVx20`、`[probe] vec_iter_enumerate 20 runs: AVx20`，
  两侧 `.text` 指纹均与本机一致；本机同两 exe 各 20 次为 `2x20` / `80x20`（**0 AV**）。
  ⇒ **同一 runner 实例内确定性崩溃**（20/20；ASLR 每轮都变却次次崩，故与地址随机化
  无关），而**跨 runner 轮次时有时无** ⇒ 指向 runner 池里**机型/镜像不同**（哪台撞上
  触发条件就在那台上稳定复现）。机制候选因此收窄为"机型/OS 版本固定的某项策略"
  （CET/CFG/加载器布局等），**不是随机时序**。
  本机对照一次性重述：严格口径单用例 ≈5.5k 次 + 全量套件多轮，**0 次 AV**。

- **两条机制取证路线（2026-09-11 起随每轮 CI 跑）**：

  1. **WER 崩溃记录**：探针打印 `Application Error` 事件的 exception code +
     fault offset（本机有逐字节相同的 `.text`，可反查崩在哪条指令）。
     ⚠️ run #32 实测该 runner **没有任何 WER 记录**（服务未记录/被禁用）⇒ 此路已死。
  2. **分配路径逐步裁剪**（`e2e_alloc_step_probe`，诊断测试、不做断言）：
     `probe_new_only`（只 new）/ `probe_one_push`（首次 push = 0→4 的 grow）/
     `probe_two_push` / `probe_six_push`（多次 grow）/ `probe_iter_enum`
     （= `vec_iter_enumerate`），外加 **`*_bump` 变体**（分配器每次返回**不同地址**）。
     本机基线：7 个变体全 `ok`（5.14 s）。**run #33 实测（失败机型）**：

     | 变体 | 该机型结果 |
     | --- | --- |
     | `probe_new_only`（不分配） | **ok exit=0** |
     | `probe_one_push` / `probe_two_push` / `probe_six_push` / `probe_iter_enum` | **AV** |
     | `probe_two_push_bump` / `probe_six_push_bump`（bump 分配器） | **AV** |

     ⇒ ①**崩溃必须有分配**，且**首次 push（空 Vec 的 grow-from-empty）就触发**；
     ②**bump 变体同样 AV** ⇒ "每次 alloc 返回同一地址"**不是**触发条件（上一轮假设
     被否）；③同机 `vec_string` / `vec_from_slice` / `box_value`（一次直接 alloc、
     不走 grow-from-empty）**全部通过** ⇒ 触发点精确落在 **`RawVec::grow_amortized`
     的空容量路径**上。
  3. **普通 rustc 对照（不可行，已撤）**：本想用"同一份源码由不带 `-Zcodegen-backend`
     的 rustc 编一遍"判定是否 forge 侧行为差异，但 rustc 对 `#![no_main]` 不把
     `#[no_mangle] mainCRTStartup` 当入口（只产出 ~1.5 KB 空桩、退出码恒 0）⇒ 对照组
     不成立，该路径已从 harness 移除（避免留下"假对照"）。
  4. **runner 指纹**（每轮打印）：OS/构建号、`PROCESSOR_IDENTIFIER`、核数 —— 用来把
     "机器相关的那个变量"落到纸面。**run #34 实测（失败机型）**：
     `Windows Server 2025 Datacenter / 26100.33296 (24H2) / AMD64 Family 25 Model 1
     (AuthenticAMD) / 4 核`；本机对照：`Windows 11 25H2 / 26200.9445 /
     Intel64 Family 6 Model 154 (GenuineIntel) / 20 核`，0 AV。
     ⇒ 失败侧集中在 **Server 2025 SKU + AMD 机型**，与本机（Win11 + Intel）不同；
     同一轮 `probe_one_push` 仍 AV、`probe_new_only` 仍 ok、bump 变体仍 AV，裁剪结论
     在第二个失败机型上复现。
  5. **单变量实验：e2e job 换 `windows-2022`**（2026-09-11，**已做毕、已回退**）：
     探针同时打印 `.text` 指纹——结果 **Server 2022 的 AMD 机型（20348.5499 /
     AMD64 Family 25 Model 17）同样 20/20 AV**，且指纹与本机仍逐字节相同
     ⇒ **变量是机型（实测均 AMD Family 25）而非 OS SKU**；钉版无益，`ci.yml` 已回退
     到 `windows-latest` 并把结论写进注释。
     附带发现：该机上 `vec_iter_enumerate` 的退出码是 **`-1073740791`（0xC0000409
     STATUS_STACK_BUFFER_OVERRUN）**而非 AV——同一 exe 的**症状随机器变化**，
     与"栈/缓冲区被写坏"的签名一致（对 WA-41 有参考价值）。

  **判读（当前）**：崩溃点已精确到"空 Vec 首次 push 的 grow 路径"，且与地址复用无关；
  同一份机器码在该机型 20/20 崩、在本机 5.5k+ 次 0 崩 ⇒ 仍是**机型/OS 侧变量**
  （候选：该 runner 的 OS 构建/CPU 相关的运行库分派路径、或该镜像的安全策略），
  **不是 forge 的 codegen 缺陷**。

- **本机确定性异常（2026-09-11 新发现，与上面那条独立）**：诊断探针里有三个变体在
  **本机（Intel/Win11 —— 就是 CI 上 0 AV 的同一台机器）**也**稳定 AV**（3/3 轮）：

  | 变体 | 内容 | 本机 |
  | --- | --- | --- |
  | `probe_addr_calc` | `p = APTR[0]; sz = ASIZE[0]; (p as usize + sz) - base` 作返回值（**不读内存**） | **AV** |
  | `probe_read_at_ptr` | 读 `p.add(sz)` 一个字节并把值作返回值 | **AV** |
  | `probe_read_only` | 读同一字节、但值归一化成常量再返回 | ok exit=165 |
  | `probe_guard_k0` / `probe_ptr_roundtrip` | 用同样的指针算术做比较/减法后返回 | ok |

  **最小复现**（脱离 harness：`rustc -Zcodegen-backend=<forge_rustc.dll>` 直接编）：
  `Vec::new(); v.push(1);` 之后读 `APTR[0]`/`ASIZE[0]`，把 `(p as usize + sz) - base`
  当返回值 ⇒ **AV**（期望 16）；把同一个值改成"先比较再返回常量" ⇒ 正常。
  **形态敏感（Heisenbug）**：给该程序加上 VEH 基础设施
  （`AddVectoredExceptionHandler` 加函数指针再加 handler）后，同一表达式**正常返回
  16**——用带标记退出码（`0xEE0xxxxx`）验证 handler **没有**跑过，即确实"异常消失
  了"；而加一个无关空函数**仍 AV**。⇒ 症状随寄存器分配/栈布局变化。

  **判读（谨慎）**：这是**本仓可本地复现的候选 codegen 缺陷**（合法程序确定性 AV，
  且随无关代码形状出现/消失），与上面"CI 机型相关"可能同源（都表现为某些布局下
  指针/地址变野），也可能独立。当前证据只到"某个代码形态会崩"，**尚未定位到具体
  指令**；后续按 §9.4 关联法缩小到 IR/regalloc 决策：`FORGE_TRACE_VCODE` 与
  `llvm-objdump -d` 逐版对照，再做二分裁剪。已登记为 `WORKAROUNDS.md`
  **WA-41**（当时标注开放；2026-09-12 关闭，见下）。
  **→ 2026-09-12 已定位并根治（= WA-42，见 §9.7）**：与 CI 机型相关 AV **同源**——
  都是 niche 枚举 `None` 的 tag 只写 4 字节、判别读 8 字节，高 4 字节残留栈垃圾
  （本机该程序的残留恰为 `0x00007ffd00000000`，AMD runner 上恒非零 → 必然 AV）。

- **AMD 机型上的崩溃面比 Vec 更宽（run #36/#37 实测）**：两台 AMD Family 25 机型上，
  "分配类"探针**大面积 AV**，而且**两台之间还不一样**：

  | 变体 | 本机 Intel | Server2022 / AMD M17 | Server2025 / AMD M1 |
  | --- | --- | --- | --- |
  | `probe_new_only`（不分配） | ok | ok | ok |
  | `probe_one_push` / `two_push` / `six_push` / `iter_enum` | ok | **AV** | **AV** |
  | `probe_audit_one_i32`（读 `ASIZE[0]`=16） | ok 16 | **AV** | **AV** |
  | `probe_audit_one_u8`（读 `ASIZE[0]`=8） | ok 8 | ok | ok |
  | `probe_audit_grow2` / `audit_align4` / `oob_guard` / `ptr_roundtrip` | ok | **AV** | **AV** |
  | `probe_read_at_ptr` / `probe_read_only` | **AV** | **AV** | ok |
  | `probe_addr_calc` | **AV** | **AV** | **`0xC0000409`** |
  | `probe_guard_k0`（同为 16B 分配 + 读毒区） | ok | ok | ok |

  ⇒ ①触发面**不止 Vec/grow**：`audit_one_i32` / `ptr_roundtrip` 这类"分配 + 读自己的
  记账静态"同样崩；②**崩溃集合随机型变化**，同一份 exe 在两台 AMD 机给出不同集合与
  不同症状（AV vs `0xC0000409`）⇒ 不是"某程序的确定性错码"，而是**机器相关的运行时
  行为**；③`0xC0000409` 与审计分配器"越界即 `return null` → `handle_alloc_error`
  → abort"的路径吻合，是下一步隔离点。
  已加入**裸分配探针**（`probe_raw_alloc4/16/64`：`alloc::alloc::alloc` 后写一字节；
  `probe_heap_write`：不经分配器直接写 HEAP）——用来判定"连裸分配都崩"还是"Vec 机制
  才崩"。本机四项均 ok。

- **可见性缺口（已修，2026-09-11）**：libtest **捕获"通过"测试的 stdout**，而容忍后的
  AV/超时不会让测试失败 ⇒ 这些事件在 CI 日志里**原本看不见**（run #28 全绿，但无法
  判断当轮有没有踩到 AV）。现在 harness 在设 `FORGE_E2E_EVENTS=<路径>` 时把关键事件
  追加落盘（`RETRY` / `KNOWN-TIMEOUT` / `KNOWN-WRONGCODE` / `CI-ENV-AV` / `FAIL` +
  末尾 `SUMMARY passed=… known=…`），CI 用 `if: always()` 步骤打印并在失败时随
  artifact 上传 ⇒ **每轮都能直接看出"有没有环境性事件"**。

- **据此落地的策略（2026-09-11）**：严格模式收窄为**只容忍「超时」与「CI-ENV-AV
  签名（`0xC0000005` = `-1073741819`）」**，其余错码仍硬失败；被容忍的 AV 打印
  `KNOWN <case> exit=… [phase] CI-ENV-AV: …`（**显式、不静默**）；两例 `reason`
  已写入双证。效果：CI 恢复绿（`passed: 101/103` + 2 条 `CI-ENV-AV` 告警），而
  "非 AV 错码进门禁"的收益保留。
- **转正判据相应更新**：`vec_push` / `vec_iter_enumerate` 在 CI 上**间歇 AV**
  （#25–#27 命中、#28 未命中）——容忍后套件仍绿，因此**"CI 全绿"不再能证明这两例
  exit 正确**（要靠 `[SUMMARY]` 事件的 `known=[]`）。未消除该 AV 前**暂不转正**；
  `vec_string` / `vec_from_slice` / `box_value` 不受影响，仍按原判据（3 轮 stage_a +
  parallel 全绿且 exit 正确）评估。
  **→ 2026-09-12**：该 AV 已定位并根治（§9.7 / WA-42）。判据按"CI 侧
  `[SUMMARY] known=[]`"执行完成：run 93927221004 在同一 AMD 机型给出
  `passed=103/103 known=[]` + 探针 21/21 ok ⇒ **5 例已全部转正**（FLAKY 清空 +
  `known_failure=false`），详见 §9.7.1。

### 9.6 与 WA-40 的关系（已落地的收口，防止假设中的静默错码）

即使 E1 假设未被证实，其失败形态（spilled def 落在不可改写字段 → 静默写坏槽）已按
fail-closed 收口：`MachineInst::is_reg_field_settable` + regalloc 守卫（spilled def
撞不可改写字段 ⇒ `Err(IrError::RegAlloc)`，不允许静默写垃圾），并有两条回归测试
（`test_def_spill_on_settable_field_ok` / `test_def_spill_on_fixed_field_errors`）。
因此后续任何触发该形态的输入都会**显式报错**而非偶发 AV——这本身会让步骤 9.4 的
排查更快收敛（要么不触发，要么给出确定的编译错误）。

### 9.7 根治（2026-09-12）：niche tag 写入宽度 = WA-42

§9.5 里"CI 机型相关 AV"与"本机确定性 AV（WA-41）"**同源**，根因在降级层一行宽度推导，
与 regalloc 无关（WA-40 的假设至此可以彻底排除）。

**定位路径**（本机最小复现 `asm_bad`：`Vec::new(); v.push(1);` + 分配器记账回读）：

1. `llvm-objdump -d` 找到崩溃指令：`movq (%r11),%r10`（`RIP=…4B83`），`r11` 来自
   被拷贝的 `ptr`；`gdb` 实测 `ptr=0x00007ffd00000000`、`old_layout.size=0x7ffde8cf00d2`
   ——**低 4 字节为 0、高 4 字节为栈残留**，这是"只写了 4 字节"的指纹。
2. `FORGE_TRACE_ABI/CALL/ARGS` 确认调用链与实参布局：
   `finish_grow(0x6486) → <Global as Allocator>::grow(0x1518C) → grow_impl_runtime(0x2CA7)`，
   崩在 `grow_impl_runtime` 的 `copy_nonoverlapping(ptr, new_ptr, old_layout.size())`；
   `new_layout={16,4}` 全对，`ptr` / `old_layout` 是**栈残留**。
3. 关掉 PE `DYNAMIC_BASE`（`target/tmp/noaslr.py`）后按绝对 VA 在各入口打点，实测
   `finish_grow` 入口 `cap=4`、`elem_layout={4,4}` 正确，但 `current_memory` 返回的
   24 字节 `Some(...)` 三字段全是垃圾、且 sret 缓冲**调用前全 0** ⇒ `current_memory`
   **本应返回 `None`**（`self.cap==0`）却走了 `Some`。
4. `FORGE_TRACE_IR` 看到 IR 里 `current_memory` 的 None 分支是
   **`%b3: store i32 0, ptr %v64`（4 字节）**，而 `%b5` 收尾把**整个 24 字节**（3 次
   `load i64`）拷进 sret；调用方 `finish_grow` 按 8 字节判空 ⇒ 高 4 字节残留非零 ⇒
   误判 `Some(野指针)` ⇒ 解引用 AV。

**根因**：`lower/statement.rs` 的 niche 枚举构造把 tag 写入宽度按 `backend_repr`
两分支推导（`Scalar` / `ScalarPair`）+ `_ => 4` 兜底。`Option<(NonNull<u8>, Layout)>`
（24 字节，`BackendRepr::Memory`，niche = offset 0 的 8 字节指针字段）落进兜底 ⇒
`store i32 0`。WA-29 修的是 `Scalar`/`ScalarPair` 两形态，**Memory payload + 指针 niche
这条路径漏网**。本机 e2e 的 `vec_push` 为何没崩：该程序栈上高 4 字节残留恰为 0；
AMD runner 的残留恒非零 ⇒ 20/20 稳定 AV（`.text` 与本机逐字节相同也由此解释：
同一份机器码，只差栈残留内容）。

**修复**：宽度改取**枚举 tag 标量自身**——`Variants::Multiple { tag, .. }`（Niche 编码
下 rustc 给的 `tag` 就是 niche 字段的标量）→ `tag.primitive().size(&self.tcx).bytes()`；
≥8 字节写 I64（`store i64 0` = 8 字节完整清零），窄 tag（u8/u16）行为不变。

**验证（全部本机实测）**：

| 项 | 修复前 | 修复后 |
| --- | --- | --- |
| WA-41 最小复现 `asm_bad` 退出码 | `-1073741819`（AV） | **16**（期望值；关 ASLR 复跑同值） |
| `current_memory` 的 IR | 1 处 `store i32 0` / 18 处 `store i64 0` | **0 处** `store i32 0` / **19 处** `store i64 0` |
| 本机 e2e | `passed: 101/103`（5 例 FLAKY-PASS） | **`passed: 103/103 known=[]`** |
| 回归门 | — | `niche_wide_payload_none_tag_store_uses_tag_width`（IR 级，与栈布局无关） |

**转正路径**：本机已全绿，但 CI 的 AMD runner 才是原始失败现场。先推修复（FLAKY 标记
**暂不动**）→ 看下轮 `[SUMMARY] known=[]` 且无 `[CI-ENV-AV]` ⇒ 再把 5 例移出 `FLAKY`
并翻 `known_failure=false`（§9.5 的判据不变：不以"容忍式全绿"当转正证据）。

**残余**：niche 位于聚合 payload **非 0 偏移**时 `tag_off` 仍走 `_ => 0` 兜底（本轮复现
形态 niche 在 offset 0，未取该路径）；如需支持应按 `tag_field` 取偏移并补用例。

**工作量**：步骤 2 本机 ~0.5 天；步骤 1/3/4 取决于 CI 何时复现（本地无法闭环）。

#### 9.7.1 CI 实证与 5 例转正（2026-09-12，收官）

修复推送后 **CI run `93927221004`**（e2e Windows job，runner = **AMD64 Family 25
Model 1 Stepping 1 / 4 cores** —— 正是修复前 20/20 稳定 AV 的同一机型）：

| 观察点 | 修复前（run #25–#39） | 本次 run 93927221004 |
| --- | --- | --- |
| `[SUMMARY] stage_a` | `passed=101/103 known=[vec_push, vec_iter_enumerate]`（+ `[CI-ENV-AV]`） | **`passed=103/103 known=[] unexpected=0`** |
| `vec_push` 20 次单跑 | `AVx20`（0xC0000005） | **`2x20`**（全部 exit=2） |
| `vec_iter_enumerate` 20 次单跑 | `AVx20` | **`80x20`** |
| alloc 逐步探针 21 变体 | 8 个变体 `AVx20`（`one_push`/`two_push`/`six_push`/`iter_enum`/`audit_one_i32`/`audit_grow2`/`audit_align4`/`oob_guard`/`ptr_roundtrip` 等） | **21/21 全 `ok`** |
| e2e 测试函数 | — | **8 passed / 0 failed**（含新增 IR 级回归门） |

⇒ 同机型、同探针二进制、从"稳定 AV"变"全部正确"，**根因与修复都得到跨机器实证**；
`.text` 逐字节相同而结论相反，正说明变量是**栈残留内容**（niche 高 4 字节）而非机器码。

依据此证据，**5 例按 §9.5 判据转正**（`tests/e2e.rs`）：

- `FLAKY` 名单清空（机制保留：将来再遇机器相关偶发时把用例名放回即可，见 const 文档）；
- 5 例 `known_failure: true → false`（错码/AV 从此**硬失败**，不再有任何容忍路径）；
- 各 case 的 `reason` 前置「2026-09-12 转正」+ 本 run 证据，历史叙述整段保留作回归对照；
- 本机复核：`e2e_stage_a_scalar_cases` `PASS box_value/vec_push/vec_string/
  vec_from_slice/vec_iter_enumerate`（exit=42/2/2/3/80）+ `passed: 103/103`，
  全 8 个测试 `--test-threads=1` 全绿；hammer（§9.2）stage_a 5 轮 103/103 + 各例 ×10 全过。

**收官 run 93934196718（2026-09-12，转正后 + WA-43 后）：11 个 job 全绿**（Format / Clippy /
Test Linux·macOS·Windows / forge-rustc check / e2e / forge-tests / Docs / Benchmarks / Coverage）：

- e2e 这次落在**另一台机器**（`Intel64 Family 6 Model 173`，与 §9.7.1 的 `AMD64 Family 25
  Model 1` 不同族）仍 `[SUMMARY] stage_a passed=103/103 known=[] unexpected=0` +
  `vec_push 2x20` / `vec_iter_enumerate 80x20` + 8 tests 0 failed ⇒ 修复跨**两个 runner 族**成立；
- `Test (Windows)`：`120 passed; 0 failed`，其中 `test_jit_v512_byref_param`（运行级 lane15，
  需 AVX-512F）、`test_v512_vconst_generates_four_evex_inserts`（生成级，任意机器）、
  `test_v512_byref_callee_load_is_64b` 均 `ok`；
- 该 run 同时修掉了我自己引入的 Clippy 红（测试里未用变量，见 CI 同款命令已纳入本地自查）。
  注：libtest 会捕获**通过**测试的输出来，所以从日志**无法区分**"V512 运行级用例真跑过"
  还是"该 runner 无 AVX-512F 提前 return"（两者都打印 `ok`）——运行级实证取自
  run 93927221004（同一用例在修复前于 AVX-512 runner 上因 `Unsupported` 失败 ⇒ 确未跳过）
  与其后修复通过的同一用例；**永久门**是那条任意机器可跑的生成级测试。

**未并入**：`e2e_parallel_pool_threads` 的用例名单仍不含 vec/alloc 与 V256 用例
——「par 编译偶发 timeout 挂起」形态从未定性（与本次 AV 无关），合并需先拿该形态的
独立证据（见该测试头部注释）。

**旁证（同 run 发现，与本修复无关 → 已顺手根治）**：`Test (Windows)` job 的
`test_jit_v512_byref_param` 曾失败 `Unsupported("v12 lowering: Vconst no matching rule")`
——该测试在**无 AVX-512F** 的机器上提前 return（本机即如此），只有 AVX-512F 的 runner
才真正跑到 `vconst(16×f32)`；而 `isa/x86_v12.toml` 的 `Vconst` 规则只覆盖
`rd = 64/128/256`，**缺 `rd = 512`**（历史上 2026-09-11 已有 3 次同类红）。
⇒ 预存缺口被机型差异暴露，非 WA-42 提交引入。
**2026-09-12 已实现（WA-43）**：新增 EVEX `VINSERTF32X4` + 4 个常量占位符
（`{vconst_lo_h2}`/`{vconst_hi_h2}`/`{vconst_lo_h3}`/`{vconst_hi_h3}`）+ 两条
`Vconst rd=512` 规则（32/64 位 lane），4 条插入按 imm=0..3 覆盖全部 lane；
并用**生成级**测试 `test_v512_vconst_generates_four_evex_inserts`（无宽向量参数/返回
⇒ 任何机器可跑）断言「恰 4 条 EVEX 插入且 imm={0,1,2,3}」，`objdump -b binary` 实测
解码为 `vinsertf32x4 zmm13, zmm13, xmm15, 0x0/1/2/3`、8 条 movabs 常量逐 lane 与源码
f32 位型一致。运行级 lane15=16 仍由原 V512 用例在有 AVX-512F 的 runner 上守护。

## 10. 剩余开放项修复记录（2026-09-12）

本节记录 §9.7.1 结尾列出的三项残余 + 一项可见性改进的修复过程。**每项独立提交、独立过门**；
共同门：`cargo fmt --all -- --check` / `cargo clippy --workspace --exclude forge-rustc
--all-targets --all-features -- -D warnings` / `cargo test --workspace --exclude forge-rustc
--exclude cargo-forge` / `cargo test -p forge-rustc --test e2e -- --test-threads=1`
（要求 `[SUMMARY] stage_a passed=N/N known=[]`）/ 改 `.md` 后 `markdownlint-cli2` 0 error。

### 10.1 W1：`stack_arg` 兜底名 → 角色 / 生成期 fail-closed（提交 `b675a80`）

**问题**：`forge-dsl` 通用生成器里还剩三处「按 x86 指令名兜底」——`frame.rs` 与
`lowering.rs` 的 `format_ident!("Mov64Rm")` / `("Mov64Mr")` + `mem/dest/src` 字段名，
只在 ISA 未声明 `[abi].stack_arg_shadow` 时取值。与 `docs/reference/isa-dsl.md` 角色章
「角色缺失 → 明确 Unsupported，不再静默去查一个别的 ISA 的指令名」相悖。

**定位证据**：兜底今天是死代码（x86 声明了 shadow + 两个角色 → 走标签路径；riscv 走
by-class 且其引用点被 `has_stack_arg` 生成期门控——由其生成测试二进制的字符串指纹佐证：
含 `v12 move_args`/`v12 float args`/`by-ref vector arg load missing`，但**不含**
`Mov64Rm`/`Mov64Mr` 与 `int/float/by-ref arg position out of range`）。但「by-position +
shadow 缺角色」的 ISA 会生成引用不存在变体的代码（模糊的生成码编译错）。

**改法**：按角色取指令（`inst_by_role(StackArgLoad/StackArgStore)` + `reg_mem_fids` 结构
派生字段 + `info.vn` 变体名）；缺角色时 shadow 已声明 → **生成期**点名角色的错误，
未声明 → `None`（不生成该分支 / 给出明确 Emit 错误）。生成器里不再出现任何字面指令名。

**验证**：新增 DSL 守卫 `stack_arg_shadow_requires_role_tags`（夹具 = 真实
`isa/x86_v12.toml` 字符串手术删 `roles` 行）：删 `stack_arg_load`/`stack_arg_store` 时由
frame.rs 报 `…[abi].stack_arg_shadow 已声明，但本 ISA 缺 roles = ["…"] 的指令（不按指令名兜底）`；
原样则全量 `generate()` 成功。回归：`forge-codegen` 0 failed、clippy/fmt 干净、
e2e 8/8 + `passed=103/103 known=[]`（含 `five_args_stack`）。

### 10.2 W2：niche tag 偏移一般化（WA-44）

**问题**：`statement.rs`（写 niche）与 `rvalue.rs`（读判别）共用同一套两分支启发式——
`ScalarPair` 且 `b` 是指针 → `b_offset`，**其余一律 0**。于是 niche 落在聚合 payload
非 0 偏移的枚举读写都在 offset 0 ⇒ 判别读到字段 0 的值；该值为 0 时 `Some` 被误判 `None`。

**先立门再修**：新增 e2e `niche_offset_some_zero_first`（`Option<(usize, usize, NonNull<u8>)>`
= 24 字节、niche 在第 3 字段 offset 16，`Some((0, 2, dangling))` 期望 12）——**修复前实测
exit=99** ✓ 复现；`niche_offset_none_roundtrip`（None → 99）作对侧守卫。

**改法（范围收窄）**：新增唯一助手 `LowerCtxt::niche_tag_offset`（两侧共用）：
①`ScalarPair` 分支**逐字保留** WA-26/WA-28/vl3 的经验判据；②**只新增**「非 ScalarPair」
分支 → `Variants::Multiple { tag_field, .. }` + `fields().offset(tag_field)`
（rustc_abi 文档：Niche 的 niche 位于该枚举的 `tag_field` 字段）；③其余 0；另加
fail-closed 尺寸守卫。

**范围教训（本次最有价值的一条）**：第一版把 `ScalarPair` 分支"一般化"为
「tag 与 `b` 同类（primitive 相同）就用 `b_offset`」——grow 链三例
（`vec_push`/`vec_iter_enumerate`/`string_concat_len`）立刻 `exit=-1073741795`
（`0xC000001D`）；gdb 定位到 `rip` 处即 **`ud2`（`0F 0B`）**，即误判判别值后走进了
`unreachable_unchecked` 通路。⇒ 这些枚举的 niche 判据**不能按 tag 标量类推**，
保留原判据、只补聚合分支。

**验证**：两例 e2e 修复后 12/99；全量 e2e `passed=105/105 known=[]`；
hammer（§9.2）5 轮 `105/105 KNOWN=[]` + parallel PASS + 5 例各 ×10 全过；clippy/fmt 干净。

### 10.3 W3：V256/V512 向量 IR Load/Store（ISA 规则 + 编解码能力补齐，2026-09-12）

**问题**：`Load`/`Store` 规则只覆盖 `rd_vec`/`rs1_vec` = 8/16；>16B 由 `compiler.rs` 的 IR 前置门
fail-closed 拒绝（注释归因"ISA 类模型缺 YMM(32B) 槽类"）。真正让"补规则"行不通的是两条 **DSL 能力缺口**
（先探明、再动手——否则会写成一条永不命中的规则）：

1. lowering 模板**只能绑定寄存器操作数**（`parse_mem_template` 只认物理寄存器名与 `{off}`/`{alloca}`），
   无法现场构造 MemRef ⇒ 宽向量内存指令若用 `mem` 槽（带 base/disp）就写不出规则；
2. VEX/EVEX **解码臂**拒绝「内存形式 + reg 槽」（生成期直接 `Err`），尽管 v15 ModRM 明确允许
   `modrm = { rm = "[reg]" }`（仅基址 `[base]`、disp 恒 0，`docs/reference/isa-dsl.md` 已写为语言能力）
   且**编码侧早已实现**该风味 ⇒ 这类指令根本加不进 ISA。

**改法**：

- ISA：新增 4 条 **reg 基址**指令 `VMOVUPS_256_R_MEM`/`VMOVUPS_256_MEM_R`（VEX.256，`vex_l=1`）、
  `VMOVUPS_512_R_MEM`/`VMOVUPS_512_MEM_R`（EVEX.512，`evex_l=2`）——与原 MemRef 形式**同编码键**
  （同一机器码的两种操作数建模：MemRef 形式继续供 ABI by-ref 的 `[RSP+off]`）；`Load`/`Store` 规则
  补 `rd_vec`/`rs1_vec` = 32/64 两档；
- forge-dsl `vlen.rs`：VEX/EVEX 解码臂对 reg 类 rm 槽按 v15 语义取 `ModRM.rm + B`（SIB 在场按 `SIB.base`）；
- `compiler.rs`：IR 前置门改口径——32B（V256）放行（与既有 V256 算术路径一致）、**>32B 需
  `avx512_available()`**（与宽向量 ABI 守卫同一判据）、非 32/64 的 >16B 宽度仍显式拒绝
  （三条路径都不产出静默错码，也不发射本机无法执行的 EVEX）；
- 顺带修 `lib.rs`：新增 **`avx512_hardware_available()`**（纯 cpuid、不读 env）——运行级 EVEX 用例必须按
  **硬件**判 skip；否则别的测试留下的 `FORGE_ASSUME_AVX512` 会让它在本机**真跑 EVEX**（实测
  `cargo test -p forge-codegen --lib --all-features` 崩于 `test_jit_v512_byref_param`，`0xC000001D`）。
  生成级用例的 env 开关收敛为 panic 安全的 RAII 守卫（`AssumeAvx512`，Drop 时清除）。

**验证**（本机 2026-09-12）：

- 生成级：`test_v256_slot_load_store_is_lowered`（VEX `C4 .. 7C 10/11`，且不得退回 `movsd`——
  反向守卫按真实的 3 字节 VEX 形态匹配）；`test_v512_slot_load_store_requires_avx512`（无 AVX-512F 时
  **必须编译期拒绝** + `FORGE_ASSUME_AVX512` 下 EVEX `62 .. 10/11`，P2 的 L'L=10）；
- 运行级：`test_jit_v256_slot_roundtrip` **真执行** VEX.256 栈槽往返，lane7 = 16.5 → 16
  （旧 8 字节默认规则会得垃圾/0）；
- 编码：`test_jit_wide_vec_reg_base_mem_roundtrip_bytes`（asm→encode→decode→encode 字节往返）+
  `objdump -D -b binary -m i386:x86-64 -M intel` 实证
  `c4 e1 7c 10 00` = `vmovups ymm0, YMMWORD PTR [rax]`、`c4 e1 7c 11 00` = store、
  `62 f1 7c 48 10 00` = `vmovups zmm0, ZMMWORD PTR [rax]`、`62 f1 7c 48 11 00` = store；
- 门禁：`cargo test --workspace --exclude forge-rustc --exclude cargo-forge` 0 failed（含
  `-p forge-codegen --lib --all-features` 123 passed）、e2e 8/8 + stage_a
  `[SUMMARY] stage_a passed=105/105 known=[] unexpected=0`、clippy `-D warnings` 与 fmt 干净。

**残余**：V512 的 Load/Store 在本机只有编译级/生成级证据（无 AVX-512F 硬件）——运行级由 W4 的
"是否真跑过"可见化 + 有 AVX-512 runner 上的 `test_jit_v512_byref_param` 守护；同一机器码存在两种
`Inst` 变体（汇编按操作数形状分发、解码先命中 MemRef 形式，打印与编码字节等价，见 WORKAROUNDS WA-45）。

### 10.4 W4：AVX-512 门控用例"是否真跑过"的可见化（2026-09-12）

**问题**：CI 上无法判断 V512 用例是否真跑过——libtest 捕获**通过**测试的 stdout/stderr，
`skip`（无 AVX-512F 提前 return）与真执行都只打印 `... ok`。历史上因此只能靠 runner 机型
推断"V512 缺口是否暴露 / 修复后是否真验证过"。

**改法**：

- 新增 `forge-codegen::jit_event(kind, case)`：`FORGE_JIT_EVENTS=<文件>` 时追加一行
  `<事件> <用例>`（写失败静默；与 e2e 的 `FORGE_E2E_EVENTS` 同模式、同理：libtest 吞输出）；
- 事件点：`AVX512-RUN` / `AVX512-SKIP`（运行级 `test_jit_v512_byref_param`，按**硬件**判）、
  `AVX512-HW=0|1`（生成级 `test_v512_slot_load_store_requires_avx512` 的能力检测结果）、
  `V512-GEN`（两条 env 放开的生成级用例——证明 EVEX 生成路径当轮真跑过）；
- CI `Test (Windows)` job：cargo test 步骤加 `FORGE_JIT_EVENTS` env，并加 `if: always()` 步骤
  打印（去重排序）。
- **取证通道（实测修正）**：本仓库无法读 job 日志——`/actions/jobs/<id>/logs` 对公共仓库也返回
  403（`Must have admin rights`）。`$GITHUB_STEP_SUMMARY` 只是 UI 的 job summary，**不会**出现在
  check-run 的 `output.summary`（#51 的诊断步骤成功且写入后，该字段仍为 `null`）⇒ 不作为通道。
  真正可读的是 **workflow 命令 `::error::` 产生的 annotations**（`/check-runs/<id>/annotations`
  无需 admin，实测可读）：故新增 `if: always()` 步骤 `Surface failing test names as annotations`——
  从 `target/tmp/windows-test.log`（cargo test 已用 `Tee-Object` 落盘）提取
  `test … FAILED` / `thread … panicked` / `test result: FAILED` 行并以 `::error::` 重发。
  同步骤也让 `Test (Windows)` 的失败**用例名**从此可自查（此前只能看到 "exit code 1"）。

**本机实证**（2026-09-12，`cargo test -p forge-codegen --lib --all-features`，本机无 AVX-512F）：

```text
V512-GEN test_v512_byref_callee_load_is_64b
AVX512-HW=0 test_v512_slot_load_store_requires_avx512
V512-GEN test_v512_vconst_generates_four_evex_inserts
AVX512-SKIP test_jit_v512_byref_param
```

⇒ 生成级 EVEX 路径真跑、运行级按硬件 skip，且判定依据可见。有 AVX-512F 的 runner 上应出现
`AVX512-HW=1` 与 `AVX512-RUN`——**待该版本 CI run 核对后回填**（未核对前不声称"CI 上运行级
V512 已被真跑覆盖"）。

**#50 的教训（诊断步骤自身红了）**：首版 `shell: pwsh` 步骤用了**行尾 `|` 续行**的跨行管道，
在 CI 上以退出码 1 失败（`Test (Windows)` 因此整 job 红；被测的 `cargo test` 步骤本身成功）。
本地以 Windows PowerShell 5.1 复现同类解析错误（`The string is missing the terminator`）——
诊断脚本必须只用版本无关构造。已改为：全部单行语句、围栏字符串用**单引号** `'```text'`
（双引号里的反引号是转义字符，`` `t `` = TAB）、`$env:GITHUB_STEP_SUMMARY` 空值守卫；
本地对"有事件文件/无事件文件"两分支均 0 退出。

### 10.5 提交与 CI 证据（2026-09-12）

| run | commit | 内容 | 结果 |
| --- | --- | --- | --- |
| 34703244549（#56） | `49323a1` | §10.5 补"#51 候选已排除"（docs） | **11 job 全绿**（含 **Test (Windows)**）；自 #52 起该 job 已连续 #52/#56 绿 |
| （#55） | `e3a055c` | `docs/forge-ir/backlog.md` 复核（docs） | 未逐条核对（docs-only，其后 #56 同树全绿） |
| 34702946018（#54） | `3a76c99` | 残留清单核查 + 矩阵跳过/失败可见化（代码） | **11 job 全绿**（逐 job 核对：Format / Clippy / forge-rustc check / forge-tests / Docs / Test (Windows) / Benchmarks / e2e / Test (macOS) / Test (Linux) / Coverage） |
| 34684847549（#53） | `fb22296` | §10.5 记录（docs） | **11 job 全绿** |
| 34683492547（#52） | `ecd9646` | Windows job 失败用例名转 annotations + 测试输出落盘 | **11 job 全绿**（含 **Test (Windows)**） |
| 34682470331（#51） | `c214390` | 诊断步骤改版本无关写法 | ❌ 仅 **Test (Windows)** 红，且**这次是 `cargo test --workspace --exclude forge-rustc` 步骤本身失败**（诊断步骤绿）。代码与全绿的 #49 完全相同 ⇒ **抖动**；失败用例名未知（当时还没有 annotations 通道），已由 #52 的可见化机制兜住 |
| 34682141756（#50） | `3756d2c` | §10.4/§10.5 记录 + 事件写 step summary | ❌ 仅 **Test (Windows)** 红——**失败点是新增诊断步骤自身**（见 §10.4「#50 的教训」）；被测的 `cargo test --workspace --exclude forge-rustc` 步骤成功 |
| 34681840003（#49） | `ed24119` | W3 + W4（宽向量 Load/Store + 门控可见化） | **11 job 全绿**（Format / Clippy / forge-rustc check / forge-tests / Docs / **Test (Windows)** / Benchmarks / e2e / Test (macOS) / Test (Linux) / Coverage） |
| 34680068995（#48） | `ca55a75` | W2（WA-44：niche tag 偏移按 `tag_field`） | **11 job 全绿** |
| 34677125033（#45） | `3ab1bfc` | `frame.rs` by-ref 收参角色化（W1 前序提交） | ❌ 仅 **Test (Windows)** 红 |

**#51 的未决项（已由 #57 的 annotations 解释）**：#51 与 #49 代码完全相同、#49 与 #52 的同一 job 全绿，
当时判为**抖动**且拿不到失败用例名。**#57 复现同一现象并由 annotations 自证**：失败用例 =
`test_jit_v512_byref_param`、断言在 `runtime/jit.rs:1457`（lane15）、`122 passed; 1 failed`——
即同一用例同一断言 ⇒ #51 的未决项**不是新问题**，根因是 §10.7 的向量溢出宽度缺陷（WA-46）。
本机侧已做 15 轮 `forge-codegen --lib --all-features` 压测（0 失败）
与 x86 JIT 矩阵（195 passed / 3 skipped / 0 failed），未见复现（本机无 AVX-512F，该用例按硬件 skip）。
**已排除一个候选**：`cargo-forge` 的 `cli_tests`（重、带 `-Z build-std` 冷启动）在 CI 上会 **SKIP**——
`tools/forge-rustc-wrapper` **不是 workspace 成员**（根 `Cargo.toml` members 不含它），
故该 job 不产 `target/debug/forge_rustc.dll`，而 cli_tests 以"dll 缺失 → 打印 SKIP 并放行"
为前置约定（2026-09-12 复核 `Cargo.toml` + `cli_tests.rs` 头部注释）。

**#45 的 `Test (Windows)` 红与今日修的 env 泄漏同源（推断，非日志直证）**：该 job 跑
`cargo test --workspace --exclude forge-rustc`，其 `forge-codegen --lib` 二进制里同时存在
①设 `FORGE_ASSUME_AVX512` 的生成级 V512 用例（D5/Vconst）与 ②要**执行** EVEX 的运行级
`test_jit_v512_byref_param`；当时②用 `avx512_available()`（读 env）判 skip ⇒ 可能被①留下的
env 带进真执行。Windows runner 是消费级 Intel（无 AVX-512F）⇒ EVEX 触发
`STATUS_ILLEGAL_INSTRUCTION`。佐证：**本机以同一命令稳定复现同一崩溃**
（`cargo test -p forge-codegen --lib --all-features` → `0xc000001d` @ `test_jit_v512_byref_param`；
见 §10.3），且 #48/#49 同代码全绿而 #45 红，符合"并行测试 env 时序"的 race 特征。
**未取到 job 日志**：`/actions/jobs/<id>/logs` 对公共仓库亦返回 403
（`Must have admin rights to Repository`），故本条为 job 级证据 + 本机复现的推断。
`ed24119` 改为按**硬件**判 skip（`avx512_hardware_available()`）后，#49 同一 job 转绿。

### 10.6 残留清单核查 + 矩阵跳过可见化（2026-09-12）

**背景**：§10.1–§10.5 落地后，`WORKAROUNDS.md` / `CLAUDE.md` 里仍有几条"残余"是**本轮之前**
写的，可能已被后续提交关掉——文档滞后于代码是本仓库反复出现的教训。逐条对**代码/测试**核查：

| 出处 | 原残余 | 核查结论（依据） |
| --- | --- | --- |
| WA-37 ③ | V512 测试仅断言 lane0（盲区） | **已关闭**——`test_jit_v512_byref_param` 断言 **lane15**（D5 缺陷正是"被调方只 load 32B"，lane0 对它是盲区） |
| WA-37 ⑤ | CallIndirect 宽参/返回缺测试 | **已关闭**——D6①/② 的 `test_jit_call_indirect_wide_vector_byref` 与 `test_jit_call_indirect_wide_vector_sret_return` 已在 `runtime/jit.rs` |
| WA-37 ④ | 宽向量第 5+ GPR 槽显式 Unsupported | **保留**（设计取舍，两侧 fail-closed，建议不做） |
| WA-42 | niche 非 0 偏移时 `tag_off` 走 `_ => 0` 兜底 | **已关闭**——WA-44（§10.2） |
| WA-43 | V512 的 Load/Store 仍 fail-closed | **已关闭**——W3（§10.3）/WA-45 |

**矩阵跳过/失败可见化**（§10.4 同一问题的另一个面）：libtest 吞掉通过测试输出，于是
"跳过了哪些用例、为什么"在 CI 上不可见。`jit_matrix::emit_events` 现把结果写成
`FORGE_JIT_EVENTS` 事件 `MATRIX-SKIP` / `MATRIX-FAIL` / `MATRIX-SUMMARY`（x86 与 riscv 两个
runner 都接）。本机实测（2026-09-12，`cargo test -p forge-tests --lib jit_matrix_x86_v12`）：

```text
MATRIX-SKIP x86_v12 sdiv_var_module (capability)
MATRIX-SKIP x86_v12 call_recursive_fib (capability)
MATRIX-SKIP x86_v12 call_recursive_fib_slot (capability)
MATRIX-SUMMARY x86_v12 pass=195 skip=3 fail=0
```

⇒ 3 条 Skip 全是"所需 op 不在能力集"的能力性跳过（不是硬件/环境）；顺带纠正 `CLAUDE.md` 里
写死的旧值 193 → 195（含采集日期与命令）。

### 10.7 由 CI annotations 定位并修复：向量溢出宽度（WA-46，2026-09-12）

**触发**：run #57（`8649229`，docs-only 提交）的 `Test (Windows)` 红。该 job 的 check-run
`annotations`（`::error::` 通道，§10.4 落地）**直接给出**：

```text
test runtime::jit::tests::test_jit_v512_byref_param ... FAILED
---- runtime::jit::tests::test_jit_v512_byref_param stdout ----
thread 'runtime::jit::tests::test_jit_v512_byref_param' (8540) panicked at crates\backend\forge-codegen\src\runtime\jit.rs:1457:9:
test result: FAILED. 122 passed; 1 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.06s
```

**这一步的信息量**（此前完全不可得，job 日志 403、annotations 通道是唯一读法）：

1. 该 runner **有 AVX-512F**——否则 `test_jit_v512_byref_param` 会按硬件 skip（`AVX512-SKIP`）；
2. 失败断言是 `jit.rs:1457` 的 `assert_eq!(got, 16)`（lane15），**不是**编译错误/超时；
3. 与 #51 的"未知抖动"**同用例同断言** ⇒ 那个未决项到此解释清楚；
4. docs-only 提交也会红 ⇒ 与本次改动无关，是**既有缺陷**在特定条件下暴露。

**定位**（本机无法复现：无 AVX-512F，EVEX 不能执行）走到确定性证据链：

- 溢出宽度：`[spill.FPR]` 只有 8 字节 `MOVSD_RM/MR`，而生成的 `emit_spill_load/store`
  **显式丢弃 width**（`let _ = width;`）⇒ 任何 FPR 类溢出只搬低 8 字节；
- 值宽：`reg_class_for` 把 >128 位向量一律归 `VEC(32)`，类表里 `reg_width = 32`
  ⇒ spill 槽 32 字节、`xreg.width()` 报 32（64B 的 V512 也被当 32B）；
- 二者叠加 ⇒ 溢出后**高半区既不写也不读**，读到的是**栈残留** ⇒ **偶发**（值随栈内容变化），
  与"同代码多数 run 全绿、少数 run 红"的观测完全一致（#49/#52/#54/#56 绿、#51/#57 红）。

**修复与守卫**：见 `WORKAROUNDS.md` WA-46 与 `CHANGELOG.md`；守卫是**确定性**的
`test_fpr_spill_width_dispatch`（直接驱动生成的 `FrameLowering`，逐宽度断言机器码形状 +
未声明宽度必须报错），不依赖分配器压力（高压力路径另有 scratch 限制，已单独记录）。

**复核点**：下一次含本修复的 CI run，`Test (Windows)` 的 `test_jit_v512_byref_param`
应确定性通过（本机无 AVX-512F，该硬件路径只能在 CI 上验证）。
