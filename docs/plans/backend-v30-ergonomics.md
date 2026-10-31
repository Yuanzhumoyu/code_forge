# 后端 v30：人体工学重构（破坏性 · 无兼容层）

> 状态：**[progress]**（2026-11-01 立项；本文件是活账本，随里程碑推进回写）
>
> 口径：**只改"怎么用"与"怎么组织"，不改"生成什么码"**。全部验收以
> "重构前后机器码逐字节相同"为硬判据（M0 的金标守卫钉住）。
>
> 相关：[`reference/backend-pipeline.md`](../reference/backend-pipeline.md)（现行管线契约）、
> [`reference/backend-gaps.md`](../reference/backend-gaps.md)（能力缺口台账）、
> [`reference/known-defects.md`](../reference/known-defects.md)（已知缺陷索引）。

## 1. 为什么做

v23–v29 把后端的**正确性**收得很紧（唯一编译入口、两相合法化、错误四分类、缺口台账），
但**使用面**与**组织面**留下了四类成本（2026-11-01 侦察，行号随迭代漂移）：

| 事实 | 位置 | 成本 |
| --- | --- | --- |
| 单函数管线 ~470 行、11 个阶段内联、`_t1.._t7` 手工计时 + 死变量 `_t2b` | `pipeline/compiler/mod.rs` | 加/调阶段要读改一大段 |
| `runtime/jit.rs` 5121 行，`#[cfg(test)]` 在第 466 行；棘轮只扫 `src/pipeline/**` | `runtime/jit.rs` | 生产 9% / 测试 91%，`runtime/**` 无上限 |
| `get_fn<F>` 是安全函数、对任意 `F` 直接 transmute | `runtime/jit.rs` | 签名写错 = UB，无检查 |
| `CompiledModule` 不带函数签名；`M: TargetMachine + Clone` 外溢 | `pipeline/session.rs` | JIT 无法校验；机器类型必须可 Clone |
| 用法集中：`CompileSession`/`JitLoader`/`get_fn` 在 forge-codegen 112/118/126 处、forge-tests 46/4/27 处 | 全仓 grep | 破坏性迁移主要在测试面，可控 |

## 2. 不变量（非目标）

- 不改指令选择、合法化策略、ABI 落点、寄存器分配、帧布局、优化 pass；同一 IR 必须产出同一字节。
- 不新增 ISA、不扩 ABI、不改 `IR_FORMAT_VERSION`、不改事件字段名与缺口 ID 文本。
- 不改 `forge-ir` / `forge-opt` / `forge-abi` / `forge-isa-dsl` 语义。
- 不做兼容层/`#[deprecated]` 垫片：旧符号直接删除，调用点全量迁移。

## 3. 目标公开面（决策）

```rust
// 选项：构造后不可变；Default 只读进程级默认
#[non_exhaustive]
pub struct CompileOptions { /* opt_level: Option<OptimizationLevel>, parallel: Parallelism */ }
impl CompileOptions {
    pub fn off() -> Self;                                  // 明确不跑 IR 优化
    pub fn speed(level: OptimizationLevel) -> Self;
    pub fn with_opt_level(self, level: OptimizationLevel) -> Self;
    pub fn with_parallel(self, p: Parallelism) -> Self;
}

// 阶段：文档表/计时/事件都从它派生
#[non_exhaustive]
pub enum Stage { Legalize, ISel, RegAlloc, Frame, Emit, Jit }

// 宿主错误：分层，不替换 IrError
pub struct CompileError { /* stage, function, source: IrError */ }
impl CompileError {
    pub fn stage(&self) -> Stage;
    pub fn function(&self) -> Option<&str>;
    pub fn gap_id(&self) -> Option<&str>;
    pub fn into_ir(self) -> IrError;
}
impl From<CompileError> for IrError;   // 仅 erased/插件边界

// JIT：签名随产物走，装载期校验；逃生口显式 unsafe
pub struct JitSignatureInfo { pub params: Vec<TypeId>, pub returns: Vec<TypeId>,
                              pub conv: CallConvId, pub variadic: bool }
pub unsafe trait JitSignature: Copy + 'static {
    fn params() -> Vec<TypeId>;
    fn ret() -> Option<TypeId>;
}
impl<M: TargetMachine> JitLoader<M> {
    pub fn get<F: JitSignature>(&self, name: &str) -> Result<F, IrError>;
    pub unsafe fn get_unchecked<F>(&self, name: &str) -> Result<F, IrError>;
    pub fn get_ptr(&self, name: &str) -> Option<*const u8>;
}
```

`JitSignature` 只覆盖 `extern "C" fn` 0–6 个参数，参数/返回限
`u8/i8/u16/i16/u32/i32/u64/i64/f32/f64` 与 `*const T`/`*mut T`，`()` 作返回；
变参、`i128`、向量、聚合一律走 `get_unchecked`。

## 4. 里程碑

| 里程碑 | 内容 | 状态 |
| --- | --- | --- |
| M0 | 金标（golden bytes）+ 活账本 | 完成 |
| M1 | `CompileOptions` / `CompileError` / `Arc<M>`，删除 `with_opt_level` | **进行中** |
| M2 | `Stage` 枚举 + 命名计时 + `precheck` / 合法化批次外提 | 待做 |
| M3 | `runtime/jit.rs` 拆文件，棘轮扩到 `src/runtime/**` | 待做 |
| M4 | `JitSignatureInfo` + `JitSignature::get`，迁移 153 处调用点 | 待做 |
| M5 | 契约文档/CHANGELOG/公开面守卫收口 + 全量门禁 | 待做 |

### M0 金标

- [`tests/golden_codegen.rs`](../../crates/backend/forge-codegen/tests/golden_codegen.rs)：
  x86/arm64/riscv64 三份 `TargetMachine` × 6 个宿主无关标量用例
  （`ret_i64`/`add_i64`/`branch`/`many_args`/`direct_call`/`f64_add`），
  逐字节比对 `tests/fixtures/golden/<isa>.txt`。
- 只收标量：宽向量档位是"谱申报 ∩ 宿主 CPU"，向量字节会随机器变。
- 更新口：`FORGE_UPDATE_GOLDEN=1 cargo test -p forge-codegen --test golden_codegen`，
  只允许在确认差异有理由时使用。

### M1 入口与选项

- `pipeline/options.rs`：`CompileOptions` / `Parallelism`，吸收并删除
  `pipeline/compiler/opt_default.rs`。
- `pipeline/error.rs`：`CompileError`；`CompileSession` 四类入口改返回它。
- `CompileSession<M>` / `JitLoader<M>` 存 `Arc<M>`：`compile_module` 不再要求 `M: Clone`。
- 删除 `with_opt_level` / `opt_level()`；`tests/session_api.rs` 改用 `with_options`。
- 新增 `tests/compile_error_context.rs`：断言 `stage`/`function`/`gap_id` 三项。

### M2 管线阶段化

- `pipeline/stage.rs`（`Stage`）+ `pipeline/profile.rs`（命名计时）；
  删除 `pipeline::STAGES`，文档守卫改读 `Stage::ALL`。
- `pipeline/precheck.rs`：值池门 / SIMD 宽向量门 / 异常门整体外提，逐门单测。
- `pipeline/legalize/schedule.rs`：`scalar → aggregates_except_va → wide →
  plan_abi_slots → va_only → finalize` 批次编排外提；`compiler/mod.rs` 只留驱动。
- 删除 `_t1.._t7` / `_t2b`；`compiler/mod.rs` 目标 ≤470 行并按实测下调棘轮。

### M3 JIT 拆文件（纯搬移）

- `runtime/jit/{mod,loader,symbols}.rs` + `runtime/jit/tests/*.rs`；
  `mod.rs` 只留公开类型与 re-export。
- `tests/file_size_ratchet.rs` 扫描面扩到 `src/runtime/**`（缺省 700；
  测试模块按"向上找 `#[cfg(test)] mod`"规则跳过）。

### M4 类型安全 JIT 调用

- `CompiledModule` 改为 `Vec<CompiledUnit>`，在 `compile_module` 里从
  `param_types()` / `return_types()` / `calling_convention` + `signature_table().variadic`
  填 `JitSignatureInfo`；`add_function` 从 `FunctionSignature` 填。
- `runtime/jit/signature.rs`：`JitParam`/`JitReturn`/`JitSignature` + 按 arity 的宏 impl（0–6）。
- `get::<F>` 校验参数个数/`TypeId`/返回/非变参/C 约定；失败 → `IrError::Semantic`
  （同时打印期望与实际，**不**进缺口台账）。
- 迁移 153 处 `get_fn::<fn(..)>` → `get::<extern "C" fn(..)>`；`mem.get_fn`（不同 API）不动。
- 新增 `tests/jit_signature.rs`：正例（i64/f64/指针）、反例（少参、错类型、变参被拒）。

### M5 文档与收口

- 重写 `docs/reference/backend-pipeline.md` §1/§2/§10，新增 §15「v30 人体工学（破坏性）」。
- `docs/README.md` 索引、`CHANGELOG.md`（`Changed (breaking)` / `Removed`）。
- 新增 `tests/public_surface.rs`：根导出精确集合；旧符号生产源码零出现。

## 5. 验收

1. 重构前后金标机器码逐字节相同（三 ISA）。
2. `cargo forge gate` 全绿；`cargo forge gate --subset rustc` 绿。
3. `cargo test -p forge-tests`（x86 JIT 矩阵 + riscv64 QEMU）绿。
4. `tests/public_surface.rs` 绿；`with_opt_level` / `JitLoader::get_fn` / `pipeline::STAGES`
   在生产源码零出现。
5. 改动文档 `npx markdownlint-cli2 <文件>` 0 error。

## 6. 进度记录

- **2026-11-01**：立项；M0 金标测试与夹具落地（三 ISA × 六用例，重构前采集）。
  本机 `target/debug` 里的旧 proc-macro DLL 被外部进程占用（重建时报 `LNK1104`），
  本轮验证改用 `CARGO_TARGET_DIR=target/v30`（仓库约定：实验产物进 `target/<名字>`）。
- **2026-11-01 M1**：`CompileOptions`/`Parallelism` 落地（`pipeline/options.rs`，
  吸收并删除 `compiler/opt_default.rs`）；`CompileError` 落地（`pipeline/error.rs`）；
  `CompileSession`/`JitLoader` 改持 `Arc<M>`，`compile_module` 不再要求 `M: Clone`；
  `Parallelism::Threads(n)` 用固定大小 worker 池（`min(n, 函数数)`），`Auto` 保持历史行为。
  两处对批准方案的细化：① `Stage` 增 `Module` 变体（整模块编排失败；**不进**管线阶段表
  与 `Stage::ALL`）；② `CompileError::gap_id()` 返回 `Option<String>`（`IrError::gap_id`
  本就是 String，避免返回临时值的引用）。金标字节不变；`compile_error_context` 绿。
