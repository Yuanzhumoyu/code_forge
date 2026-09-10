# YMM ABI 完整改进方案（>128 位向量传参）

## ⚠️ ARCHIVED（2026-09）

> 本方案**已完成**（2026-09-10 逐项核查后归档）：S1–S5、D2、D3、D4、D5、D6 全部
> 落地并有测试守护——详细核查结论见下文「状态更新」与「§5 复核结论」。
> 现行信息入口：`crates/tools/forge-rustc/WORKAROUNDS.md`（WA-37 残余清单）与
> 仓库根 `CLAUDE.md` 的 SIMD 支持矩阵（ABI 行）。
> 归档时点的**已知残余**（均非本方案在办项）：宽向量第 5+ GPR 槽显式 Unsupported
> （两侧对称，决定不做）；IR 层大于 16B 的向量 Load/Store 无 lowering 规则
> （ISA 类模型缺 YMM 槽类 → 编译期显式拒绝，属未来特性）；V512 运行级 lane 断言受
> AVX-512F 硬件门控（生成级测试已守门）。
> 本文为历史记录，代码现状以仓库代码与现行文档为准。
> 对应 `docs/archive/roadmap-status.md` 剩余事项 1。
>
> **状态更新（2026-09-10 复核，以此为准）**：主库 S1–S5 **已实现**（2026-08-31 起
> ed43103 S1 / e9e869a S2 / 33df6fa 语义标签重构 / 4959474 S4-S5）——>128 位向量
> by-ref 传参（调用方 temp 槽 store + GPR 指针）+ sret 返回 + 混合槽位全链路；
> **D2/D3 已关闭**（B3 门控撤除；≤16B 向量全宽 MOVAPS，机制是指令角色
> `roles = ["vec_mov"]` 而非 `[abi].vec_mov_inst`——v15-S4 已删除全部 `*_inst`
> 名指针）。**本轮（2026-09-10）落地的真实缺口**：
> **D5 已修**（V512 被调方收参按**参数 IR 类型字节数**分派 64B load，见 §5.1）、
> **D4 补齐**（by-class 分支补显式 `Unsupported`，不再静默丢参）、
> **D6 补测试**（CallIndirect 宽向量实参 / sret 返回两例，均绿）。
> 另发现并收口：IR 层 **>16B 向量 Load/Store** 无 lowering 规则（旧行为落默认
> 8 字节 MOV 静默截断）→ 现编译期显式拒绝（§5.4）。

## 1. 现状盘点（代码为准）

| 环节 | 状态 | 位置 |
| --- | --- | --- |
| ABI 声明 `vector by-ref limit` | ✅ 已声明 | `isa/x86_v12.toml` `[abi.arg_class] vector strategy="by-ref" limit=128` |
| 入口守卫（>16 字节向量检查） | ✅ 已实现 | `pipeline/compiler.rs:1706-1742`（`vector_by_ref_limit()` 查询；超限 → Unsupported） |
| 被调方收参（`move_args` by-ref 分支：从 `[ptr]` load 到 YMM） | ✅ 已实现 | `frame.rs` gen_frame_lowering；验证 `runtime/jit.rs:872 test_jit_v256_byref_param`（extern "C" fn(*const f32)） |
| 调用方 IR Call 宽向量实参（>16 字节） | ✅ 已实现（2026-08-31 S1） | 帧内 RBP 相对槽（间距 64B）store + 指针进 GPR 槽；验证 `test_jit_wide_vector_call_byref`（旧「编译期拒绝」测试已删除并反转） |
| 宽向量返回值（sret） | ✅ 已实现（S2） | 被调方 store 到 `[RCX]` + 调用方回读；向量不经 `expand_large_agg_ret`（后者只管 `is_aggregate`）；验证 `test_jit_v256_byref_return` / `test_jit_wide_vector_call_sret` |
| regalloc 的 by-ref 标记 | ✅ 已预留 | `pipeline/alloc_result.rs:25 param_by_ref: Vec<bool>`（`regalloc_bt.rs:792` 填充） |

**结论**：缺口集中在**调用方**（call lowering 的实参栈拷贝）与**返回路径**（sret
隐藏指针参数）。被调方收参逻辑是现成参考实现。

## 2. 设计

### 2.1 调用方：宽向量实参 → 栈拷贝 + GPR 指针传参

IR `call f(v256, i32, v256)` 降级为：

```text
// 每个宽向量实参：
tmp  = alloca 32B（帧内 temp 槽，32 字节对齐）     ← lowering 的 temp 槽
store tmp[0..16],  v.low                       // vmovups 或逐 16 字节
store tmp[16..32], v.high
// 传参：by-ref 参数占 1 个 GPR 槽（RCX/RDX/…），其余参数正常
mov  RCX, tmp_ptr        // 宽向量参数 1 → 指针
mov  RDX, i32_param      // 标量参数照旧
mov  R8,  tmp2_ptr       // 宽向量参数 2 → 指针
call f
```

要点：

- **槽位规则**：by-ref 参数占整数寄存器序列的 1 个槽（Windows x64 ABI 的
  YMM 传参惯例即如此——按引用传递）。标量 int/float 参数与 by-ref 参数
  在 GPR/XMM 槽位序列中交错（by-ref 只占 GPR 槽）。
- **对齐**：temp 槽按 32 字节对齐（vmovups 非对齐也可，但对齐更稳）。
- **chunk 拷贝**：≤32 字节向量用 1-2 条 16 字节 movups；>32 字节（V512
  等）按 16 字节 chunk 循环，且需 AVX-512 检测门控（见 2.3）。
- **数据流**：调用前 store 向量值到 temp 槽；`call` 后 temp 槽是死的
  （regalloc 生命周期由 lowering 显式表达：store 用掉向量 XReg、call
  clobbers 后无 load——XReg 活性自然终止）。

### 2.2 返回路径：sret 隐藏指针参数

被调方返回宽向量 `ret v256`：

```text
// 签名展开：ret v256 → 隐藏参数 sret_ptr（GPR 槽尾或首？Windows x64：
// sret 是首个参数槽 RCX——与调用方约定一致）
// 调用方：
mov  RCX, ret_slot_ptr   // 隐藏 sret 指针（首 GPR 槽）
call f
v = load ret_slot_ptr[0..32]   // 结果 load 回 XReg

// 被调方：
store [ret_slot_ptr],  result.low
store [ret_slot_ptr+16], result.high
```

- 与 `expand_large_agg_ret`（≤16 字节拆段）区分：>16 字节向量走 sret，
  不拆段（保持 YMM 值语义）。
- sret 指针槽位：Windows x64 的 sret 约定是第一个参数槽（RCX），
  与 struct-return ABI 一致；方案按 x86 TOML 的 `arg_class` 顺序把
  sret 插入 int 类槽首。
- 结果回读：`call` 后从 `[ret_slot]` load 到结果 XReg（XMM 路径已有
  mov 回读模式可复用）。

### 2.3 V512/AVX-512 门控

- 现状 `avx_available()` 检测 AVX/AVX2（jit_matrix 的 "AVX" 伪能力）。
- 超 32 字节向量（V512）需 AVX-512（`avx512f`）——新增检测函数 +
  编译入口守卫（无 AVX-512 → Unsupported，防静默截断）。32 字节
  （V256）只依赖 AVX（已有检测）。

### 2.4 与被调方现有实现的衔接

- `move_args` by-ref 分支已实现「从 [ptr] load 到 YMM」——调用方改动
  后同一分支直接可用（`param_by_ref` 标记已由 regalloc 填充）。
- `alloc_result.rs` 的 `param_by_ref` 已存在——call lowering 与
  move_args 共用该标记判定 per-参数 by-ref。

## 3. 实施步骤（每步独立可验证）

| 步骤 | 内容 | 验证 |
| --- | --- | --- |
| **S1** | 调用方宽向量实参栈拷贝 + GPR 指针传参（call lowering 扩展） | `test_jit_wide_vector_call_arg_rejected` 反转：IR Call 传 V256 → 编译通过 + JIT 执行 lane0 值正确；forge-tests 新增 v256 调用用例 |
| **S2** | 返回路径 sret（被调方 + 调用方） | 新增 jit 测试：函数返回 V256（vconst 返回 + vextract 验证） |
| **S3** | 混合参数（标量 + 宽向量交错）与多宽向量参数 | GPR 槽位序列测试（如 `f(i32, v256, f64, v256)` 的槽位断言） |
| **S4** | V512/AVX-512 门控 | 无 AVX-512 机器编译 V512 传参 → Unsupported（不崩溃） |
| **S5** | mini_c / forge-tests 矩阵回归 + 文档 | 全门禁绿；CLAUDE.md SIMD 矩阵更新 |

**S1 为最高优先级**（解除当前显式拒绝，补上调用侧缺口）。

## 4. 风险与对策

- **ABI 槽位序列错误**（跨调用链参数错位）：S3 用槽位断言测试兜底；
  被调方 move_args 已有参数槽计数逻辑可复用。
- **对齐/未对齐 load**：vmovups（非对齐）先落地，规避对齐假设；
  S1 验证 32 字节 temp 槽对齐。
- **跨调用链 ABI 一致性**：S1/S2 改动后全量 298 条 JIT 用例 + forge-rustc
  e2e（58 用例）回归——调用链改动最危险，必须全套守门。
- **返回值经栈回读的额外拷贝**：先正确后优化（sret 是标准做法，接受
  一次栈往返；后续可做返回值优化 RVO 评估）。

## 5. 2026-09-10 复核结论（D4/D5/D6 + 新发现）

### 5.1 D5 V512 收参宽度（**已修**）

**根因（静态定位 + 生成级验证）**：被调方 by-ref 收参的 load 变体按
`__pv.width()`（参数 XReg 的**寄存器类宽**）分派，而 `reg_class_for` 对 >128 位
向量一律给 `VEC(32)`（`lib.rs`）→ **64B（ZMM）分支永不可达**：调用方按 IR 类型
`size_bytes` 写满 64B，被调方只 load 32B（VEX.256 还会把 ZMM 高 256 位清零 →
lane8..15 丢失）。旧测试只断言 lane0，故长期未暴露。

**修法**：新增 `AllocResult.param_bytes`（每参数 IR 类型字节数，分配后由
`CompileState` 按 `xreg_types` 填充，镜像 `sret`/`stack_arg_bytes` 的既有模式）；
`forge-dsl .../codegen/frame.rs` 的 by-ref load 改为按 `__rm.param_bytes` 分派：
64 → ZMM（EVEX）、≤32 → VEX.256、其它 >32B → 显式 `Unsupported`（不静默截断）。

**验证**：新增生成级测试 `test_v512_byref_callee_load_is_64b`（断言 prologue 含
EVEX zmm load 且**不含** VEX.256 ymm load）——本机无 AVX-512F 也能跑（只编译不执行）。
运行级断言（lane15 = 16.0）仍需硬件；本机 Alder Lake 无 AVX-512F，
`test_jit_v512_byref_param` 在本机**直接 return 跳过**（实测确认）。

### 5.2 D4 第 5+ GPR 槽（**已补齐**）

调用方与 by-position 分支本就有显式 `Unsupported`；**by-class 分支缺 else**
（`frame.rs`）→ 静默不收参。本轮补 else → `Unsupported`（与 by-position 同款
fail-closed）。x86 走 by-position，故对本仓库 x86 主路径无行为变化。

### 5.3 D6 CallIndirect 宽向量（**已补测试**）

路径与直接 `Call` 共用 `gen_call_lowering`（仅跳过 `args[0]`），此前**零测试**。
新增两例（`runtime/jit.rs`，本机可跑）：
`test_jit_call_indirect_wide_vector_byref`（V256 by-ref 实参 → lane0=1）、
`test_jit_call_indirect_wide_vector_sret_return`（V256 sret 返回 → lane0=3）——均绿。

### 5.4 新发现：IR 层 >16B 向量 Load/Store（**已 fail-closed**）

`isa/x86_v12.toml` 的 Load/Store 规则只覆盖 `rd_vec/rs1_vec = 8/16`；>16B 向量会
落到底部默认 **8 字节** `MOV_R_MEM`/`STORE_MEM_R` → **静默截断**（槽往返只搬 8B）。
该类规则无法直接补：ISA 类模型只有 XMM（`fpr16`）/ZMM（`fpr32`）槽类、**缺
YMM(32B) 槽类**（实测：`VMOVUPS_RM` 的 `dst:fpr` 与 `VMOVUPS_ZMM_MEM` 的
`dst:fpr32` 均被 DSL 报“操作数签名不符”，规则无法表达）。
处理：在 `pipeline/compiler.rs` 增加守卫——**>16B 向量 Load/Store 显式拒绝**
（`Unsupported`，消息说明缺 YMM 槽类），测试
`test_wide_vector_slot_load_store_is_rejected`（32B/64B 两档断言）。
forge-rustc 的向量值走内存建模（`copy_agg` / `CopyNonOverlapping`）不经该路径，
故无回归；如需支持需先在类模型引入 YMM 槽类。
