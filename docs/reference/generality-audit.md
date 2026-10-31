# forge 通用性审计（v28）

> 状态：`[active]`（2026-10-31 建立；随 M0–M7 推进回写）
> 范围：ISA-DSL 接入面、`forge-rustc` 目标/宽度假设、动态插件与动态产物、已知被
> `#[ignore]` 的正确性缺陷。
> 方法：只读代码 + 现有测试/文档交叉核对；每条给出**可复核的文件:行**（行号随迭代
> 漂移，以符号名为准）。本页不替代 `backend-gaps.md`：能力缺口的权威 ID 仍在那里，
> 这里记录“通用性被写死/未接线”的结构性问题与决策。

## 1. 结论摘要

| 编号 | 问题 | 严重度 | 决策 |
| --- | --- | --- | --- |
| GEN-01 | 占位符注册表 66 行、每行一个 ISA 专用 ctor；新编码/分片就要改 Rust | 高（阻塞 DSL 作者） | ✅ 已修（M1）：数据化表达式引擎 + `[placeholder]` |
| GEN-02 | 新 ISA 接入要改 4 处本体代码；生成的 `ensure_registered()` 不注册管线/call planner | 高（阻塞新 ISA） | ✅ 已修（M2）：`isa/manifest.toml` + 构建期生成注册 |
| GEN-03 | 动态插件 API 与实现不符、未注册管线、无目标映射、无测试 | 高（功能不可用） | ✅ 已修（M3）：v2 原始字节 ABI + 宿主适配器 + 目标表 + 真 dylib 测试 |
| GEN-04 | `forge-rustc` 从不设置 IR `DataLayout`；`usize` 只认 32/64；lowering/alloc 大量 I64 假设 | 高（非 64 位 ISA 必错） | ✅ 已修（M4）：DataLayout 注入 + 8/16/32/64 映射 + demo16 |
| GEN-05 | 产物动态链接未建模：`prefer-dynamic` 拒绝、`RelocKind` 无 GOT/PLT、对象层无动态段 | 中（能力边界） | ✅ M5：`DYNAMIC_OUTPUT` fail-closed + 对象层最小 DLL 链接/导出验证 |
| GEN-06 | 两个 `#[ignore]` 是真实误编译（栈形参 + 循环 + 调用 ⇒ AV/死循环） | 高（已知错值） | ✅ 已复核通过（M6）：v28 实测两条均通过；注释/索引已转正 |
| GEN-07 | 外部宿主 `parts=["tm"]` 没有零胶水路径；host demo 只覆盖 encode/decode/asm | 中（与 GEN-02 同源） | ✅ M2/M3：manifest 生成注册；插件 `export_isa_plugin!` 两行完成 tm 宿主接入 |
| GEN-08 | 文档/守卫腐化：plugin 文档 API 不存在、`TARGET` 仍写死两目标、guard 硬编码 3 个 ISA 名 | 低（误导） | ✅ 文档/目标表随 M2/M3 修；守卫仍逐个列 3 个发行 ISA（新增 ISA 需同步测试，列为后续） |

## 2. 证据与复现

### GEN-01 占位符表是代码而不是数据

- `crates/frontend/forge-isa-dsl/src/dsl/codegen/placeholder.rs:80-998`：66 个 `Ph` 行，
  `name`/`kind`/`temp`/`xreg`/`ctor` 全部手写；`{off_f0..f3}`、`{iconst_f0..f3}`、
  `{iconst_hi20}`、`{fconst_hi32_hi20}`、`{vconst_lo_f0..f3}`、`{shufps_imm8}` 等
  各带一段 `quote!` 闭包。
- `crates/frontend/forge-isa-dsl/src/dsl/codegen/lowering.rs:3141-3152`：lowering 只认
  这张表；`validate.rs:2510` 的“未知占位符”判定也以它为唯一事实源。
- 复现：`rg 'name: "\{' crates/frontend/forge-isa-dsl/src/dsl/codegen/placeholder.rs`
  得到 66 行；新增一个分片占位符必须改这个文件。
- **✅ 已修（2026-10-31，M1）**：`placeholder.rs` 改为表达式引擎（整数/位运算 +
  固定原语源/函数）；谱内 `[placeholder.<名>] expr = "…"` 即可定义新占位符。
  旧 66 名迁到 `dsl/codegen/default_placeholders.toml`（数据文件），三份谱不改；
  未知名字/成环/覆盖原语在加载期报错。新增测试 `spec_defined_placeholder_generates_without_rust_change`
  与成环/未知名字负向用例；全门禁 54/54。

### GEN-02 新 ISA 要改本体

- `crates/backend/forge-codegen/src/arch/mod.rs:4-6`：三个 `pub mod` 手写。
- `crates/backend/forge-codegen/src/pipeline_hooks.rs:43-83`：三个工厂 + 三次
  `register_pipeline` + 三次 `register_isa_call_planner` 手写。
- `crates/tools/forge-rustc/src/compile.rs:105-132`：目标三元组 → ISA 名的
  `contains("x86_64") / contains("aarch64")` 分支；未命中即拒绝。
- `crates/tools/forge-rustc/src/alloc_runtime.rs:16`：`SUPPORTED_ARCHES = ["x86_64"]`。
- `crates/frontend/forge-isa-dsl/src/dsl/codegen/integration.rs:991-1001`：生成的
  `ensure_registered()` 只做 `Registry::register_backend`，没有管线工厂与调用点钩子。
- **✅ 已修（2026-10-31，M2）**：新增 `isa/manifest.toml`；`forge-codegen/build.rs`
  读清单，逐条预生成并写 `OUT_DIR/arch_registry.rs`（模块 + 目标前缀表 +
  `ensure_all` + alloc shim 表）与 `backend_hooks.rs`（管线工厂/调用点钩子）。
  `arch/mod.rs` 只 `include!`，`pipeline_hooks.rs` 只调生成函数，`compile.rs` 查
  目标表，`alloc_runtime::SUPPORTED_ARCHES` 删除。新增 ISA 只改 TOML + manifest 一条。

### GEN-03 动态插件不可用

- `crates/tools/forge-plugin/src/lib.rs:10-16` 文档示例调用
  `registry.register(Arc::new(IsaCompiler::<MyIsa>::new()))`；实际
  `forge-isa-runtime/src/registry.rs:27` 是 `register_backend(Arc<dyn ErasedTargetMachine>)`。
- `lib.rs:96-114` 加载 `__codegen_isa_register` 后只把对象塞进 `Registry`；未注册
  `register_pipeline` / `register_isa_call_planner`，而 `impl_erased_target_machine!`
  的 `compile` 走 `compile_via_pipeline`（`forge-isa-runtime/src/erased.rs:13-18`）⇒
  插件 ISA 编不出函数。
- `crates/tools/forge-rustc/src/compile.rs:122-131` 的 `isa_name_for_target` 不认识
  插件声明的目标；插件也没有导出目标表的 ABI。
- 全仓没有插件加载测试（`PluginLoader` 只有空加载/未设环境变量两条单测）。
- **✅ 已修（2026-10-31，M3）**：v2 ABI（`__forge_isa_compile_raw`/`__forge_isa_name`/
  `__forge_isa_targets`/可选 ABI 数据）+ 宿主 `PluginBackend: ErasedTargetMachine` 适配器 +
  runtime `target_registry`；`isa_name_for_target` 回落查询插件表；loader 用 `Box::leak`
  保持 Library 存活。插件编译走**原始字节 C ABI**（宿主分配缓冲区），避免 Rust 容器跨
  dylib 由错误分配器释放。真 dylib 夹具
  `crates/tools/forge-plugin/tests/plugin_v2.rs` 已验证加载 → 编译 → 本机执行。

### GEN-04 rustc 的 64 位假设

- `crates/tools/forge-rustc/src/types.rs:26-35`：`Isize/Usize` 只匹配 32/64，其他回落
  `I64`。
- `crates/foundation/forge-ir/src/ir/data_layout.rs:434-438`：`DataLayout::default()`
  是 x86_64 Linux；全仓 `set_data_layout(` 的调用者不含 `forge-rustc` ⇒ 32/16 位
  目标上 `PTR` 仍是 64 位类型。
- `crates/tools/forge-rustc/src/lower`：`TypeId::I64` 166 处、`TypeId::PTR` 32 处；
  `alloc_runtime.rs:33-44` 的 size/align 常量与签名全是 I64。
- 复现：`rg -c 'TypeId::I64' crates/tools/forge-rustc/src/lower`；对 32 位目标
  （如 `i686`）编译会在 IR verifier/ABI 处出现类型宽度矛盾。

### GEN-05 产物动态链接未建模

- `crates/tools/forge-rustc/src/backend.rs:189-201`：`prefer-dynamic` 明确拒绝；
  `relocation-model` 只接受默认/Pic。
- `crates/foundation/forge-isa-runtime/src/output_types.rs:39-47`：`RelocKind` 只有
  `Absolute/Relative`，没有 PLT/GOT 语义。
- `crates/tools/forge-object/src/object_writer.rs:528-610`：只映射绝对/相对重定位；
  没有 `.dynsym`/`.rela.dyn`/`.rela.plt`/`.got`。
- 复现：`--crate-type=cdylib` 当前没有专门拒绝或测试；外部符号在 DLL/共享库中的
  间接调用没有建模。

### GEN-06 已知误编译（`#[ignore]`）

- `crates/backend/forge-codegen/src/runtime/jit.rs:4897-4912`：
  `test_jit_spill_pressure_loop_inline` 在 `n >= 1` 时 AV 崩溃；机器码证据显示第 9 个
  形参 `n` 在 RDI，循环体把 RDI 当累加器写。
- `jit.rs:5327-5335`：`jit_stack_param_loop_call_only_step` 实测挂起（死循环）。
- 这两条不在 `docs/reference/known-defects.md`，属于“被 `#[ignore]` 藏起来的已知失败”。

### GEN-07 外部宿主 tm 无零胶水

- `examples/isa-host-demo/src/lib.rs:15-23` 与 `tests/host_surface.rs`：只覆盖
  `parts = ["encode","decode","asm"]`；`parts=["tm"]` 需要宿主自己
  `register_pipeline` + `register_isa_call_planner`（见 `pipeline_hooks.rs`），否则
  `ErasedTargetMachine::compile` 永远 `Unsupported`。

### GEN-08 文档/守卫腐化

- `docs/reference/backend-gaps.md:64` 的 `TARGET` 仍说“已支持 x86_64 / aarch64”，
  与“manifest 驱动”方向冲突。
- `forge-plugin/src/lib.rs:13` 的 `codegen_lib` / `IsaCompiler` 在仓库中不存在。
- `isa_roundtrip_guard.rs:73-75`、`spec_coverage_guard.rs:41-43`、
  `operand_model_guard.rs:157-159` 硬编码三份谱名。

## 3. 决策与去向

- GEN-01 → ✅ M1 已落地（`[placeholder]` 表达式引擎；旧名数据化）。
- GEN-02/GEN-07 → ✅ M2 已落地（manifest 生成注册；外部 tm 宿主仍可复用同一生成函数）。
- GEN-03 → M3：插件 ABI v2（注册包 + 目标表），fixture 真加载/真执行。
- GEN-04 → M4：`TargetProfile` + `DataLayout` 注入 + `ptr_ty`；16/32 位 profile 与
  demo8/demo16 验证，不新增真实 32 位后端。
- GEN-05 → M5：先 fail-closed + 审计，再做最小 PIC 共享对象/COFF DLL 验证；
  `prefer-dynamic` 与 `std` 动态运行时仍不做。
- GEN-06 → M6：用现成复现修分配器；不能修则登记 KD 并保留显式 ignore。
- GEN-08 → 随 M2/M3/M7 更新文档与守卫。
