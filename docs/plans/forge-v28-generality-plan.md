# forge v28 通用性整治计划（progress）

> 状态：`[progress]`（2026-10-31 立项；执行顺序 M0→M1→M2→M3→M4→M5→M6→M7）
> 审计证据与缺陷编号见 [`../reference/generality-audit.md`](../reference/generality-audit.md)。
> 现行规范入口：[`../reference/isa-dsl.md`](../reference/isa-dsl.md)、
> [`../reference/backend-pipeline.md`](../reference/backend-pipeline.md)、
> [`../reference/backend-gaps.md`](../reference/backend-gaps.md)。

## 1. 目标

1. **占位符数据化**：`placeholder.rs` 不再有“每缺一个占位符就改 Rust 表”；新占位符
   由谱内 `[placeholder.<名>] expr = "…"` 定义，Rust 只保留固定原语与表达式引擎。
2. **ISA 接入零本体改动**：新增 ISA = TOML + `isa/manifest.toml` 一条，不改
   `arch/*.rs`、`pipeline_hooks.rs`、`compile.rs`、`alloc_runtime.rs`。
3. **动态后端零胶水**：插件 dylib 只声明一次即可完成 Registry + 管线 + call planner +
   目标映射；宿主零改动。产物动态链接做到“最小可验证 PIC/DLL + 明确 fail-closed”。
4. **rustc 宽度泛化**：指针宽度、槽单位、原生整型宽度由 target/ISA 数据派生；
   16/32 位 profile + demo16 验证，不新增真实 32 位 ISA 后端。
5. **缺陷审查落盘**：审计页逐项有证据、有决策；修不了的要登记，不许口头带过。

## 2. 里程碑

### M0 审计与文档（本提交）

- `docs/reference/generality-audit.md`、本计划、文档地图、CHANGELOG。
- 能力缺口 ID 随各自里程碑加入（`capability_gaps` 守卫要求源码与台账双向相等，
  不能提前挂空行）。

### M1 占位符表达式引擎（✅ 2026-10-31 落地）

- 谱内 `[placeholder.<名>] expr`；表达式支持字面量、运算符、原语源与原语函数。
- 原语源：`out/out2/arg(n)/g(n)/f(n)/off/alloca/global/sret/ret(k)/ret_src(k)/
  retf(k)/retf_src(k)/retN/retN_src/imm(n)/iconst/fconst/vconst/cc/cc_inv/cond/
  true/false/target`。
- 原语函数：`bits/sext/zext/round/sat_sub/vconst_raw64/vconst_half/
  vconst_elem_bits/shufps/mem`。
- 旧 66 名迁到 `crates/frontend/forge-isa-dsl/src/dsl/codegen/default_placeholders.toml`（数据文件），
  三份内置谱不需要改动；生成字节由 e2e/arm64 oracle 与 pins 守卫验证不变。
- 诊断：未知源/函数/元数、类型不匹配、循环定义，禁止回退 0。
- 验收：三 ISA oracle/pins 不变；新谱只加一条 `[placeholder]` 即可生成/编码；
  `placeholder.rs` 无 ISA 名。

### M2 ISA 接入零本体改动（✅ 2026-10-31 落地）

- `isa/manifest.toml`（name/spec/module/parts/targets/alloc_shim）。
- `forge-codegen/build.rs` 读 manifest、预生成、写 `OUT_DIR/arch_registry.rs`；
  `arch/mod.rs` 只 include。
- `forge_codegen::include_backend!` 一行完成注册（Registry + 管线 + call planner）。
- `compile.rs` 目标映射改查 manifest/插件表；`alloc_runtime` 能力改 manifest 声明。
- 三个后端守卫遍历 manifest。
- 验收：新增玩具 ISA 只加 TOML + manifest 条目即可编译。

### M3 插件零胶水（✅ 核心 2026-10-31：v2 ABI + 宿主适配器；fixture 待补）

- 插件 ABI v2：`__forge_isa_register_v2` / `__forge_isa_targets` / `__forge_isa_name`。
- `export_plugin!` 生成注册包；`PluginLoader` 记录 target→isa。
- fixture 插件真加载、编译 const42 并 JIT 执行；宿主零 ISA 名。
- 修 stale 文档。

### M4 rustc 宽度泛化（✅ 核心 2026-10-31：DataLayout + 8/16/32/64 映射；demo16 待补）

- `TargetProfile { ptr_bits, slot_bytes, int_widths, endian, mangling }` 从 rustc
  `TargetDataLayout` + ISA `TargetRegInfo` 构造。
- 每个 Function/TypeContext 注入 IR `DataLayout`。
- `ptr_ty()/size_ty()/ptr_const()/usize_const()`；按审计表替换指针/尺寸/索引路径，
  其余 I64 白名单写理由。
- `map_type` 支持 8/16 指针；`abi.rs` 用 slot/profile；demo16 编译测试。
- 验收：16/32 profile 单测 + demo16；x86_64/aarch64 e2e 不变。

### M5 动态产物链接（✅ fail-closed 2026-10-31：`DYNAMIC_OUTPUT`；PIC 对象待做）

- `[cap:DYNAMIC_OUTPUT]` 明确拒绝未支持组合（cdylib/dylib/proc-macro/prefer-dynamic）。
- `RelocKind::Plt32/GotPcRel` + 对象层映射；ELF `cdylib` 经 `ld.lld -shared` 验证
  导出符号、无 TEXTREL；Windows DLL 导出表检查；`prefer-dynamic` 仍拒绝。
- 验收：至少一个共享对象/ DLL 链接与导出检查通过 + 负向用例。

### M6 GEN-06 分配器缺陷（✅ 2026-10-31 复核转正）

- `test_jit_spill_pressure_loop_inline`（AV）与 `jit_stack_param_loop_call_only_step`
  （hang）确定性复现；修后取消 `#[ignore]` 进门禁；不能修则登记 KD。

### M7 收口

- 文档更新（isa-dsl/backend-pipeline/backend-gaps/CHANGELOG/CLAUDE）；
- `cargo forge gate` 全量、workspace 测试、pins 不变检查；
- 审计页回写状态；未做项（真实 32 位 ISA、std 动态运行时）明确列为后续。

## 3. 边界

- 占位符表达式只做纯整数/位/内存表达式；全新机器事实仍加一个原语源。
- 本计划不新增 riscv32/i686 真后端；宽度验证到 IR/JIT 编译层。
- 动态链接只做插件零胶水 + PIC 共享对象最小验证；`std` 动态运行时、TLS/GOT 全链路
  不做，保持 fail-closed。
- 插件 ABI 只保证同一 rustc/forge 构建内兼容。

## 5. 当前状态（2026-10-31）

已完成并落地：

- M0 审计（`docs/reference/generality-audit.md`）。
- M1 占位符数据化（三份谱字节不变；新谱内定义测试）。
- M2 manifest 注册（arch 模块/管线/目标表/alloc 表全部生成；手写 arch 文件删除）。
- M3 核心（v2 插件 ABI + 宿主适配器 + 目标表）。
- M4 核心（IR DataLayout 注入 + 8/16/32/64 指针映射 + 单测）。
- M5 fail-closed（`DYNAMIC_OUTPUT` 明确拒绝）。
- M6 GEN-06 复核：两个历史失败测试（`test_jit_spill_pressure_loop_inline` /
  `jit_stack_param_loop_call_only_step`）在 v28 实测均通过，已转正为回归；注释/索引同步。

仍待补：

- M3 真实 dylib fixture 的加载 + JIT 执行测试。
- M4 demo16 谱与 16 位编译用例（当前只有 16/32/64 profile 单测）。
- M5 最小 PIC 对象/共享库链接验证（当前只做负向拒绝）。
- M7 全量门禁复跑与文档收口。

## 6. 验证

- 2026-10-31 全量门禁：`cargo run -p cargo-forge -- gate` **54/54 全绿**
  （`rustc-e2e` 214、arm64 oracle/exec、jit-x86/riscv64、clippy、workspace、docs）。
  本机需 `CARGO_BUILD_JOBS=1` 防内存耗尽；arm64 探针偶发环境性失败，单独复跑 13/13、
  LICM 探针 10/10 通过。

```bash
cargo fmt --all -- --check
cargo test -p forge-isa-dsl
cargo test -p forge-codegen --lib --all-features
cargo test --workspace --exclude forge-rustc
cargo test -p forge-rustc --test e2e -- --test-threads=1
cargo test -p forge-rustc --test arm64_exec
cargo run -p cargo-forge -- gate
```

M1/M2 迁移后 `git diff -- crates/tools/forge-tests/asm/ratchet/pins.txt` 必须为空。
