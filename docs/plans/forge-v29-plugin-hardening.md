# v29 插件硬化计划（Plugins v2.1）

> 状态：进行中（2026-10-31 起）· 上游：v28 M3 的“插件零胶水”收口
> 目标：把插件从“能加载简单函数”提升到“能承载带重定位/外部调用的真实函数”，
> 并让不兼容插件在加载期 fail-closed，而不是运行期未定义行为。

## 范围

### A1 编译产物跨 ABI（✅ 2026-10-31）

- 机器码仍走原始字节 C ABI；重定位单独编码（`forge_codegen::plugin_abi`）：
  offset/kind/width/adj/addend + reloc/symbol 名。
- 插件导出 `__forge_isa_reloc_required` / `__forge_isa_reloc_copy`；宿主解码后重建
  `CompiledFunction.relocations`。
- **未覆盖**：`line_entries`、`cfi`（不是链接正确性前提，后续按需补）。
- 验收：`plugin_abi::roundtrip_relocations` 单测；插件夹具简单函数真执行。

### A2 插件调用点布局钩子（✅ 2026-10-31）

- 插件加载时 `__forge_isa_init_abi` 在**插件自己的** runtime 注册该 ISA 的
  `call_planner`（按 `IsaInfo::name()`，不是模块名）。插件编译函数时的 Call 降级
  因此能查到 plan；宿主不需要持有插件机器类型。
- 宿主侧仍只注册 `ErasedTargetMachine`；跨 ISA 调用不属于本插件范围。

### A3 ABI 版本握手（✅ 2026-10-31）

- `PLUGIN_ABI_VERSION = 2`；插件导出 `__forge_plugin_abi_version`；加载器精确比对，
  不匹配直接拒绝（不静默兼容）。

### A4 可变缓冲区（✅ 2026-10-31）

- 插件导出 `__forge_isa_compile_required`；宿主按精确大小分配，取消固定 1 MiB。

### A5 插件错误文本（✅ 2026-10-31）

- 插件线程局部错误槽 + `__forge_isa_last_error`；宿主在 `-1` 后立即拷贝，带进
  `IrError::Unsupported`，不再只看到 `-1`。

### A6 测试（✅ 2026-10-31 基本闭合）

- ✅ 真 dylib 加载、编译、执行（`tests/plugin_v2.rs`）。
- ✅ 重定位编解码单测。
- ✅ “带 Call 的插件函数”端到端测试：`FuncRef::from_raw` 构造外部引用，
  断言宿主收到非空 `CompiledFunction.relocations`（此前旧 ABI 会丢）。
- ✅ 非插件 DLL（kernel32）加载被拒绝，错误指向缺失的插件符号。
- ⏳ ABI 版本号不匹配的正向 fixture（需要第二个旧 ABI 构建）仍待补。

## 验收

- 插件函数含一次外部调用/全局引用时，`CompiledFunction.relocations` 非空且宿主可用。
- 版本不匹配/缺符号时加载失败信息明确。
- `cargo forge gate` 全量绿。

## 验证记录

- 2026-10-31：`cargo forge gate` **54/54 全绿**（workspace 曾因环境 OOM 单独红一次，
  清内存后重跑通过）。
- 插件测试 3/3（加载执行、Call 重定位、非插件 DLL 拒绝）；`forge-isa-runtime`
  41 tests + mem 11 tests；`forge-codegen` JIT 基本用例通过。

## 不做

- 跨 rustc/forge 构建的 ABI 兼容（只保证同一构建，版本不匹配即拒绝）。
- 插件卸载/热替换生命周期管理。
