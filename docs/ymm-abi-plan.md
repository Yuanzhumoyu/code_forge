# YMM ABI 完整改进方案（>128 位向量传参）

> 对应 `docs/roadmap-status.md` 剩余事项 1。现状：**被调方 by-ref 收参已
> 实现**（`move_args` by-ref 分支 + jit B1 测试），**调用方栈拷贝与返回
> （sret）路径未实现**——IR Call 传宽向量实参目前编译期拒绝。

## 1. 现状盘点（代码为准）

| 环节 | 状态 | 位置 |
| --- | --- | --- |
| ABI 声明 `vector by-ref limit` | ✅ 已声明 | `isa/x86_v12.toml` `[abi.arg_class] vector strategy="by-ref" limit=128` |
| 入口守卫（>16 字节向量检查） | ✅ 已实现 | `pipeline/compiler.rs:1706-1742`（`vector_by_ref_limit()` 查询；超限 → Unsupported） |
| 被调方收参（`move_args` by-ref 分支：从 `[ptr]` load 到 YMM） | ✅ 已实现 | `frame.rs` gen_frame_lowering；验证 `runtime/jit.rs:872 test_jit_v256_byref_param`（extern "C" fn(*const f32)） |
| 调用方 IR Call 宽向量实参（>16 字节） | ❌ 编译期拒绝 | `runtime/jit.rs:915 test_jit_wide_vector_call_arg_rejected`（显式拒绝防静默截断） |
| 宽向量返回值（sret） | ❌ 未实现 | 无路径；`expand_large_agg_ret` 仅覆盖 ≤16 字节聚合 |
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
