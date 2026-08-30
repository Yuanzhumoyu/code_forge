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

README 限制区⑧ `probe_n n=5`（~40 行）最小复现。gdb 断点 + 
`FORGE_TRACE_LOWER/TERM/VCODE` 逐 MIR 语句比对：

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

### 下一步候选（按优先级）

1. **monomorphized core 函数符号/链接**：`core::num::wrapping_add` 实例的
   符号生成/解析与链接（值错 1164775489 疑为未初始化/错符号调用——反汇编
   e2e 产物验证 call 目标）；
2. **rustc 常量位模式**：`rvalue.rs:298` 的 `scalar.to_bits(s.size())`——
   CONST trace 已加（wrapping_add 常量 1000/2000 正确，暂排除）；
3. **FnAbi 参数布局**：`FORGE_TRACE_ABI` 对比 rustc FnAbi 与打包
   （wrapping_add 2×i64 Direct 正确，暂排除）。

E1-E5（grow 链定位/Select 恢复/sret spill/转正）在此线之后执行。
