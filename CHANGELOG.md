# CHANGELOG

<!-- markdownlint-configure-file { "MD013": { "line_length": 512, "code_block_line_length": 512, "heading_line_length": 512 }, "MD024": false } -->
<!-- 文件级豁免原因：历史条目按单行记录（最长 ~440 列），且多年条目同置 [Unreleased] 下，
     版本子节用 `### Added (日期)`，同层 Fixed/Changed 标题按惯例重复——均为 CHANGELOG
     结构与单行惯例，不按正文 120 列规则折行；重构留待 changelog 整顿时处理。 -->

All notable changes to the `code-forge` project will be documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.0.0/).

## [Unreleased]

### Added (2026-09-12)

- **V256/V512 向量 IR 层 Load/Store（ISA 规则 + 编译入口能力门）**：`isa/x86_v12.toml` 的 `Load`/`Store` 规则原先只覆盖
  `rd_vec`/`rs1_vec` = 8/16（V64/V128），>16B 由编译入口 **fail-closed 拒绝**（"ISA 类模型缺 YMM 槽类"）。现有：
  - 规则补齐 32/64 两档 → `VMOVUPS_256_R_MEM`/`VMOVUPS_256_MEM_R`（VEX.256，`vex_l=1`）、`VMOVUPS_512_R_MEM`/`VMOVUPS_512_MEM_R`（EVEX.512，`evex_l=2`）；
  - 编译入口的字节门改为：32B（V256）放行（与既有 V256 算术路径一致）、**>32B（V512/EVEX）需 AVX-512F**（与宽向量 ABI 守卫同一判据）、
    其余非 32/64 的 >16B 宽度仍显式拒绝——四条路径都不产出静默错码；
  - 新增 **reg 基址**内存形式（`modrm = { rm = "[reg]" }`，仅基址 `[base]`、disp 恒 0）：IR Load/Store 的地址是 lowering 的**寄存器操作数**，
    而 lowering 模板只能绑定寄存器、无法现场构造 MemRef（原有的 `vs_memref` 形式继续供 ABI by-ref 路径的 `[RSP+off]`）。
  - 验证：生成级 `test_v256_slot_load_store_is_lowered`（VEX `C4 .. 7C 10/11`，且不得退回 `movsd`）、
    `test_v512_slot_load_store_requires_avx512`（无能力必须编译期拒绝 + `FORGE_ASSUME_AVX512` 下 EVEX `62 .. 10/11`，P2 L'L=10）、
    runtime `test_jit_v256_slot_roundtrip`（真执行 VEX.256 槽往返 lane7 = 16.5 → 16）、
    `test_jit_wide_vec_reg_base_mem_roundtrip_bytes`（asm→encode→decode→encode 字节往返 + objdump 实证
    `c4 e1 7c 10 00` = `vmovups ymm0, YMMWORD PTR [rax]`、`62 f1 7c 48 10 00` = `vmovups zmm0, ZMMWORD PTR [rax]`）。

- **V512 向量常量 `Vconst rd=512`（WA-43）**：ISA 模型的 `Vconst` 规则原只覆盖 `rd = 64/128/256`，64 字节常量落到「no matching rule」→
  `Unsupported`。该缺口只在**有 AVX-512F 的机器**暴露（运行级 `test_jit_v512_byref_param` 无 AVX-512F 时提前 return，
  本机即如此）⇒ 历史上按 CI runner 分配偶发红（`test result: FAILED. 118 passed; 1 failed`）。
  - 实现：新增 EVEX 指令 `VINSERTF32X4`（`EVEX.512.66.0F3A.W0 18 /r ib`）、常量池占位符
    `{vconst_lo_h2}` / `{vconst_hi_h2}` / `{vconst_lo_h3}` / `{vconst_hi_h3}`（`__vconst_half` 本已按 half 索引泛化），
    以及两条 `Vconst` 规则（32 位 lane：F32/I32 用 `PUNPCKLDQ`；64 位 lane：F64/I64 用 `PUNPCKLQDQ`）：
    4 个 128 位段各自装好后按 imm=0/1/2/3 插入；4 条插入覆盖全部 lane（`{out}` 自身当累加器，**初始值不影响结果**）。
  - 验证：新增**生成级**测试 `test_v512_vconst_generates_four_evex_inserts`（无需 AVX-512 硬件——无宽向量参数/返回，
    不触发宽向量 ABI 的 AVX-512 门控；断言恰有 4 条 `62 … 18 /r ib` 且 imm = {0,1,2,3}）；
    `objdump -D -b binary -m i386:x86-64` 解码实测为 `vinsertf32x4 zmm13, zmm13, xmm15, 0x0/1/2/3`，
    且 8 条 `movabs` 常量逐 lane 与源码 f32 位型一致（`0x404000003fc00000` … `0x4180000041600000`）；
    运行级 lane15=16 断言仍由 `test_jit_v512_byref_param` 在有 AVX-512F 的 runner 上守护。

### Fixed (2026-09-12)

- **niche 枚举 tag 偏移一般化（WA-44）**：`lower/mod.rs` 新增唯一助手 `niche_tag_offset`（`statement.rs` 写侧与 `rvalue.rs` 判别读侧共用）——
  旧实现只认「`ScalarPair` 且第二标量是指针 → `b_offset`」，**其余一律 0**；于是 niche 落在聚合 payload **非 0 偏移**的枚举（如 24 字节 `Memory` repr、
  niche = 第 3 个字段 offset 16）读写都在 offset 0 ⇒ 判别读到字段 0 的值，该值为 0 时 `Some` 被误判成 `None`。
  - 修法（范围收窄）：①`ScalarPair` 分支**逐字保留** WA-26/WA-28/vl3 的经验判据（`b` 是指针类才用 `b_offset`，否则 0）；
    ②**只新增**「非 ScalarPair」分支 → `Variants::Multiple { tag_field }` + `fields().offset(tag_field)`（rustc_abi 文档明示 Niche 的 niche 在该 `tag_field` 字段）；
    ③其余仍 0；另加 fail-closed 尺寸守卫（`偏移 + tag 宽度 > 枚举尺寸` → 编译错误）。
  - 验证：新增 e2e `niche_offset_some_zero_first`（期望 12；修复前 exit=99）与 `niche_offset_none_roundtrip`（99）；
    e2e 全量 `passed=105/105 known=[]`；hammer（计划 §9.2）5 轮 `105/105 KNOWN=[]` + 5 例各 ×10 全过。
  - 范围教训：曾把 `ScalarPair` 分支"一般化"为「tag 与 `b` 同类就用 `b_offset`」——grow 链三例（`vec_push`/`vec_iter_enumerate`/`string_concat_len`）
    立即 `exit=0xC000001D`（gdb：`ud2`/`unreachable_unchecked`）⇒ 那些枚举的判据不能按 tag 标量类推，故保留原判据、只补聚合分支。

- **宽聚合 payload 的 niche 枚举 `None` 写入宽度（WA-42，关闭 WA-41）**：`lower/statement.rs` 的 niche 构造把 tag 宽度按 `backend_repr`
  两分支（`Scalar`/`ScalarPair`）+ `_ => 4` 兜底推导——24 字节 `Memory` payload + offset 0 的 8 字节指针 niche（如 `Option<(NonNull<u8>, Layout)>`，
  即 `RawVecInner::current_memory` 的返回类型）落进兜底 → 发 `store i32 0`（只写低 4 字节），**高 4 字节残留栈上旧值** → 调用方按 8 字节判空失败
  → `finish_grow` 误取 `Some(野指针)` + 栈残留 `old_layout` → `grow_impl_runtime` 的 `copy_nonoverlapping` 解引用 → AV（`Vec::new(); v.push(1)` 即崩）。
  - 修法：宽度改取**枚举 tag 标量自身**（`Variants::Multiple { tag, .. }`，Niche 编码下 rustc 的 `tag` 即 niche 字段的标量）→ `tag.primitive().size()`；
    ≥8 字节写 I64（完整清零），窄 tag（u8/u16）行为不变。
  - 验证：本机 WA-41 最小复现由 `exit=-1073741819` 变 **16**；`FORGE_TRACE_IR` 对照 `store i32 0` 1 处 → 0 处；
    e2e `passed: 103/103 known=[]`（新增 IR 级回归门 `niche_wide_payload_none_tag_store_uses_tag_width`）。
  - 同一缺陷即 CI 上 `vec_push`/`vec_iter_enumerate` 的机型相关 AV（同一 grow 链、同一误判路径；AMD runner 栈残留高位恒非零、本机多数布局恰为 0）。
    CI 跨机器实证（run 93927221004，runner = AMD64 Family 25 Model 1，即修复前 20/20 AV 的同机型）：`[SUMMARY] stage_a passed=103/103 known=[]`、
    两例各 20 次单跑 `2x20`/`80x20`、alloc 逐步探针 21/21 全 ok。
- **VEX/EVEX 解码臂对「内存形式 + reg 槽」直接报错（forge-dsl `vlen.rs`）**：v15 的 ModRM 模型允许
  `modrm = { rm = "[名字]" }` 指向 `reg` 槽（= 仅基址 `[base]`、disp 恒 0，见 `docs/reference/isa-dsl.md`），
  编码侧也已实现该风味，但 **VEX/EVEX 解码侧**仍保留旧的「内存形式 rm 必须是 mem 槽」守卫（生成期 `Err`）——
  于是任何 `[reg]` 形式的 VEX/EVEX 指令都无法加入 ISA。现已对齐：rm 槽为 reg 类时 base 取 `ModRM.rm + B`（SIB 在场按 `SIB.base`）。
  回归守卫 `test_jit_wide_vec_reg_base_mem_roundtrip_bytes`（字节往返 + objdump 实证）。
- **`FORGE_ASSUME_AVX512` 泄漏到运行级 EVEX 用例 → 非法指令（`STATUS_ILLEGAL_INSTRUCTION` 0xC000001D）**：
  该 env 只应放开**生成期**可行性门，但 `test_jit_v512_byref_param` 用 `avx512_available()`（读 env）判断是否跳过——
  另一个测试留下的 env 会让它在无 AVX-512F 的 CPU 上**真的执行** EVEX。新增 `avx512_hardware_available()`
  （纯 cpuid、不读 env），运行级用例改用它判 skip；生成级用例的 env 开关收敛到 panic 安全的 RAII 守卫
  （`AssumeAvx512`，Drop 时清除）。实测：`cargo test -p forge-codegen --lib --all-features` 修复前 `0xc000001d` 崩在
  `test_jit_v512_byref_param`，修复后 `123 passed; 0 failed`。
- **宽向量守卫里一条空断言**：`test_v512_byref_callee_load_is_64b` 的负向断言按 2 字节 VEX（`C5 FC 10`）匹配，
  而本编码器**恒发 3 字节 VEX**（`C4`）⇒ 该断言恒真（空守卫）。改为 `C4 .. 7C 10` 形态。
- **e2e 门禁转正收官**：`vec_push` / `vec_string` / `vec_from_slice` / `vec_iter_enumerate` / `box_value` 移出 `FLAKY` 名单并翻 `known_failure=false`
  —— 5 例的错码/AV 从此**硬失败**（不再有 `CI-ENV-AV`/超时容忍路径；`FORGE_E2E_STRICT_FLAKY` 机制保留但名单为空即等价全量门禁）。
  判据按计划 §9.5 执行：CI run 93927221004 在同一 AMD 机型给出 `known=[]` + 探针 21/21 ok（§9.7.1）。

### Added (2026-08-08)

- **forge-rustc 模块化重构（P1）**：`lib.rs` 2143 行 → 61 行（薄 facade），拆分为 9 个模块（`prelude`/`backend`/`func_ref`/`alloc_runtime`/`layout`/`types`/`abi`/`compile`/`rustc_compat` + `lower/` 6 文件）；对齐 CGCL 架构（mod prelude + codegen backend 模式）。
- **rustc 1.99 nightly API 漂移适配（37 处）**：`CodegenBackend::codegen_crate`/`join_codegen` 签名变化、`CompiledModule.global_asm_object`、`BackendRepr::ScalarPair {..}`、`VariantLayout.field_offsets`、`EarlyBinder::bind(tcx, ..)`、`LangItem::DropGlue`、`substs.skip_binder()`、`Instance::resolve_drop_glue` 等——全部收敛进 `rustc_compat.rs` + `abi.rs`。
- **ABI 层收敛（P4.1）**：`abi_kind_of_ty`（PassMode 投影：16 字节 Scalar → Indirect、SimdVector → Direct）成为 `is_agg_mem`/`is_scalar_pair_abi` 统一内核；`pad_call_args` 按 FnAbi 补齐 track_caller 隐藏 `&Location` 参数（修复 panic 路径参数错位）；sret 计数修正（rustc FnAbi.args 不含 sret 指针但调用方须传）；Ignore/ZST 参数跳过（Global 等零大小类型不占参数槽）。
- **块参数传参一致性修复（WA-14）**：`map_terminator_args_to_params` 复用 arg 已有寄存器 + `pre_allocate_block_param_xregs` 顺序调整——write_bytes 内联循环从"完全不执行"变为执行（count=1 变体返回 0xAB 正确）；新增最小复现回归测试 `test_loop_block_param_write_bytes_style`。
- **测试体系统一（P4.5）+ CI 纳入（P4.6）**：`stage_a.rs`/`run_tests.sh`/`test_runner.sh` 并入 `tests/e2e.rs`（58 用例，known_failure+reason 回归探针）；`.github/workflows/ci.yml` 新增 `forge-rustc-check` job。
- **WORKAROUNDS.md**：14 条机读绕法清单（`[WA-NN]` 编号 + 代码注释引用）。

### Fixed

- **write_bytes_loop 转正（e2e 56/58，WA-14 关闭）**：三层根因全修——①块参数传参两层（map_terminator 复用 arg 映射 + pre_allocate 顺序）；②**窄类型宽度（真根因）**——`opsize_from_type(u8)=32` 导致主库 Load/Store 越界 4 字节读写（u8 元素读 0xABABABAB 垃圾、write_bytes 循环写 32 字节覆盖相邻槽）+ `ireduce(mov)` 不扩展导致 cast 后高 24 位残留（wb1 返回 0xFFFFFFAB）。修复：新增 `LowerCtx::mem_opsize_from_type`（Load/Store 用真实内存宽度，**不枚举不截断**——u8→8、u16→16、自定义非常规宽度如 12 字节 GPR→96 原样传递）+ forge-rustc IntToInt cast 对 u8/u16 无符号源零扩展 mask。
- **vec_push/vec_string 根因最终定性（十二轮深挖，仍 known_failure）**：**嵌套 niche 传播**（rustc 的 niche 布局传播——外层枚举判别与内层 payload 判别共享/嵌入字节，LLVM 级特性）：grow 链（Result/ControlFlow/TryReserveError 错误传播）与最小复现 cf5（`CF::Break(Err(5u8))` 应=7 现=1）同源——rvalue.rs/statement.rs 的 Niche 单层实现需扩展为嵌套传播（以 cf5 为驱动用例），WA-11 记录。
- **field_offset Primitive 防护（落库）**：`lower/mod.rs` 的 `field_offset` 对非 enum 类型无条件调 `fields().offset()`——标量（`FieldsShape::Primitive`）无字段触发 "Primitive has no fields" 编译 ICE（嵌套枚举投影如 `CF::Break(Err(1u8))` 的 `.0` 对标量）——修复后嵌套 `ControlFlow<Result>` 从编译 ICE → 正确运行。
- **诊断基础设施（落库，无行为影响）**：`FORGE_TRACE_TERM`（每块 terminator 打印）+ vcode dump 加 fn 名前缀（62 个函数的 vcode 此前无法区分——十二轮深挖的关键工具）+ regalloc_bt 的 spill/reload/evict trace。
- **regalloc_bt 诊断 trace 补齐**：`spill_vreg`/`reload_from_stack`/`evict_and_assign` 增加 `[spill]`/`[reload]`/`[evict]` 输出（此前完全静默，无法定位跨块寄存器问题）。
- **forge-rustc 3 个 known_failure 的编译层问题**：`vec_push`/`vec_string` 从 compile failed(101 ICE) 变为编译通过（sret 计数 + Ignore/ZST 参数跳过）。
- **forge-codegen liverange 测试编译修复**：`RegClass::GPR` → `RegClass::GPR(8)`（多宽度化重构后测试代码未跟上）。
- **forge-dsl 删除未使用 `stack_scratch` 变量**（`[abi.call]` 必需性校验块残留）。

### Added (2026-08-05)

- **RegClass 宽度化重构（多宽度寄存器类）**：
  - `forge-ir::RegClass` 由 7 个硬编码变体改为 `GPR(u16)/FPR(u16)/VEC(u16)` 三变体，payload = 字节宽度，可表达任意 ISA 非常规宽度（如 12 字节 GPR）；`GPR64/GPR32/GPR8/FPR64/VEC128/VEC256/Int/Float` 等均为便捷常量，语义不变。
  - 物理寄存器索引全链 `u8 → u32`（`PhysReg::to_index/from_index`、`PReg.num`、`FrameAccess::register_index`、DSL 生成的 `uses/defs/reg_field/set_reg_field/clobbers`、`TargetRegInfo` 各列表、`ClassConfig.allocatable/sp_reg/fp_reg`）——支持 >255 寄存器的 ISA（如 JVM 类）。
  - `TargetRegInfo::register_classes()` 暴露 `[reg_classes.*]` 全部多宽度类（`RegisterClassInfo` 新增 `allocatable`）；`reg_class_width` 全类支持（TOML 优先，未定义类回退 payload）。
  - lowering 按类型分派：I32 结果/参数/块参数 → `GPR(4)` 池（32 位指令语义），F32/F64 → `FPR(8)`；未定义类回退（GPR 族继承 GPR64 池、FPR/VEC 继承 FPR64 池）。
  - 分配器新增跨宽度类物理重叠检测（`phys_conflicts`/`phys_owner`）：GPR(4) 与 GPR(8) 同编号视为同一物理寄存器，杜绝两个 XReg 分到同一物理寄存器。
  - `isa_from_file!` 路径解析支持向上查找（`CARGO_MANIFEST_DIR/../../..`），非 workspace 根 cwd 下编译也可定位 `isa/*.toml`。

### Added (2026-07-26)

- **Directory restructuring**: `forge-codegen` (37 files → 5 subdirectories: `arch/`, `traits/`, `pipeline/`, `runtime/`, `ext/`) and `forge-opt` (24 files → 5 subdirectories: `scalar/`, `loops/`, `ipa/`, `advanced/`, `support/`). All backward-compatible `pub use` re-exports preserved.
- **Multi-segment conditional merge (E1)**: Consecutive `?cond` segments with the same condition now share one `if` block in generated code instead of generating separate `if` blocks.
- **Multi-block CFG JIT tests (F1)**: JIT tests for if-else branching, multi-parameter branching, loop countdown, stack frame with many locals, and boundary constants (i32::MAX/MIN, i64::MAX).
- **AArch64 Icmp lowering (B1)**: All 10 integer comparison conditions activated in `isa/aarch64_v10.toml` (EQ/NE/LT/GT/LE/GE/LO/HI/LS/HS via SD_CMP + SD_SETCC).
- **AArch64 + RISC-V compile tests (C2/C3)**: 5 new compile tests verifying add/mul, Icmp, and branch lowering across AArch64 and RISC-V backends.
- **Constant pool inline syntax (C1)**: `{const 42}` / `{const 0xFF}` / `{const 3.14}` in lowering operands emits immediate values without constant pool lookup.
- **Name-based field indexing**: V10 lowering path uses `ParsedTemplate` field names for operand-to-field mapping. `[lower.*]` operands must match BTreeMap alphabetical field order（v10 语法文档已随 v11 删除——现行唯一 DSL 语法见 `docs/reference/isa-dsl.md`）。

### Fixed

- **Display round-trip 两个真 bug**（forge-ir）：`CallIndirect` 丢失函数指针（现输出 `call <retty> %ptr(...)`）；`StackAddr`/`GlobalAddr`/`Alloca` 丢失立即数（现输出 offset/大小，如 `stack_addr -4`）——经扩展指令 round-trip 测试暴露。
- **审查驱动补测试（+7）**：forge 扩展指令 round-trip（stack_addr/copy/call_indirect/fconst 内联）、语义错误路径（undefined block/function）、`DataLayout::is_default`。见 `docs/archive/coverage-history.md`。
- **覆盖率工具链诊断记录**：cargo-llvm-cov 在 Windows msvc + rustc 1.96 无法产出可靠报告（profraw 与二进制 counter 错位，全 0%），四种方案验证记录见 `docs/archive/coverage-history.md`。

### Fixed

- **WASM32 `end` opcode**: Added `needs_epilogue_label()` trait method to `InstructionSet`, guarding x86-specific JMP emission. WASM functions now correctly terminate with `end` opcode (0x0B).
- **`forge-plugin` missing `log` dependency**: Added `log = "0.4"` to Cargo.toml — `--all-features` compilation now succeeds.
- **LEA constant pool FIXME**: Changed `constants: None` to `_constants_clone.as_deref()` in `lowering.rs`, enabling scale-value detection for LEA merge optimization.
- **CLAUDE.md cleanup**: Removed outdated `.rs.bak` file references.

### Added (2026-07-24)

- **Width-aware instruction model**: `FieldType::Opsize` + `DynType` in ISA model. Every GPR instruction now supports 16/32/64-bit operands via a unified encoding macro (`$modrm_rr`), automatically emitting 0x66 prefix (16-bit), default encoding (32-bit), or REX.W (64-bit) based on the opsize field.
- **Opsize propagation from IR types**: `LowerCtx::default_opsize` is set from `Type::size_bytes()` before instruction lowering. `default_for_type()` for Opsize reads `ctx.default_opsize`, making all instructions width-aware automatically.
- **17 new x86-64 instructions**: MOVZX (R8/R16), MOVSX (R8/R16), ADD/SUB/AND/OR/XOR/CMP r,imm32, CMPXCHG, XADD, BT/BTS/BTR/BTC, CMOVcc.
- **3 new encoding macros**: `$modrm_r_imm32` (width-aware r,imm32), `$cmovcc_rr` (conditional move with embedded condition code).
- **64-bit boundary fuzz tests**: 5 new tests exercising i64::MAX, i64::MIN, large multiply, power-of-two shift, and NOT operations.
- **DynType validation**: `IsaModel::validate()` now checks that kind is a supported type and default is in values list.

### Fixed

- **MOV64_RR encoding**: Changed from 0x8B to 0x89 (correct data direction: MOV r/m64, r64 → dest←src).
- **MOVQ_R64_XMM mnemonic**: Changed from `movq.to_gpr` (dot breaks IDENT lexer) to `movq_to_gpr`.
- **MOV_REG_IMM64 mnemonic disambiguation**: Changed to `mov_imm` to avoid AsmResolver collisions with MOV variants.
- **@shift_cl primitive**: Added missing REX.W prefix for 64-bit shift operations (was emitting 32-bit shift with 0x41 instead of 0x49/0x48).
- **Sshr lowering**: Uses `movsxd` (sign-extend 32→64) instead of zero-extending `mov` for arithmetic right shift.
- **MOVSXD_R_RM**: Hardcoded to always emit REX.W (always sign-extends to 64-bit), removed opsize field.
- **Prologue param copy TODO**: Resolved — `@move_args` already copies ABI arg regs → vregs via `Reg` type operands.
- **LEA constant pool**: Added cloning pattern to avoid borrow conflicts (scale validation disabled pending PatternMatcher vreg allocation fix).
- **Dead code warning**: Eliminated for `DynType.kind` and `DynType.values` (now used in validation).

### Changed

- **AArch64 TODO updated**: Prologue/epilogue require STP/LDP/MOV_SP/SUB_SP with Reg-type operands.
- **RISC-V TODO updated**: ADDI/SD/LD/JALR already defined; prologue needs Reg-typed variants.
- **Backend TODOs cleared**: AArch64 and RISC-V prologue requirements accurately documented.

### Added (2026-07)

- **Type::Bool**: New `Bool` type for comparison results (icmp/fcmp). Replaces `Type::I32` for boolean values, improving type safety and semantic clarity.
- **Opcode::Freeze**: New IR instruction to prevent undefined behavior propagation. Optimization passes treat `Freeze` as a barrier for constant folding and value inference.
- **SROA pass** (`src/optimize/sroa.rs`): Scalar Replacement of Aggregates optimization. Splits struct/array allocas into scalar allocas for mem2reg promotion.
- **AArch64 backend** (`examples/isa/aarch64_v10.toml`): New ISA backend targeting 64-bit ARM (AAPCS64 calling convention). Supports GPRs (X0-X30), FPRs (V0-V31), and standard instruction set.
- **CI configuration** (`.github/workflows/ci.yml`): Automated formatting, clippy, test, and docs checks across Linux/Windows/macOS.
- **Encoding DSL enhancements**: Declarative encoding format support for x86 and RISC-V instruction patterns.
- **PE/COFF relocation**: Format-aware relocation mapping for PE COFF (IMAGE_REL_AMD64_*) and Mach-O (X86_64_RELOC_*).
- **MIR extensions**: Expanded rustc MIR rvalue/terminator coverage (Repeat, Aggregate, CastKind variants, Assert, Yield).
- **LTO integration**: `Module::optimize_with_lto()` for cross-module optimization (inlining + dead function elimination).
- **E-Graph ISel**: `ISelPass` for algebraic simplification before instruction selection.
- **Performance benchmarks**: Criterion-based compilation pipeline benchmarks in `benches/compile_bench.rs`.
- **JIT multi-return**: Support for two-value returns (RAX + RDX) in the x86-64 JIT backend.
- **Register spill/reload**: Full spill handling for high register pressure scenarios using R10/R11 scratch registers.
- **Extended JIT test suite**: 129 integration tests covering parameters, returns, stack balance, register pressure, multi-return.

### Changed

- **Comparison result type**: `icmp` and `fcmp` now produce `Type::Bool` instead of `Type::I32`.
- **Copy instruction**: Now infers result type from source operand instead of hardcoding `Type::I32`.
- **Clippy clean**: All clippy warnings resolved in the main library and DSL codegen.
- **Rustc backend**: MonoItem path updated for latest nightly; Bool type mapping fixed to `Type::Bool`.

### Fixed

- Register allocator spill offset calculation (RBP-relative negative offsets).
- PE/COFF relocation flag mapping for x86-64 Windows targets.
- Mach-O relocation field naming (r_type, r_pcrel, r_length).
- Collapsible `str::replace` calls in DSL codegen (clippy).

---

## Version Policy

- **0.x.y**: API may change without notice. No stability guarantees.
- **1.0.0** (future): Public API frozen. Requires: rustc backend passes core/alloc tests, AArch64 backend functional, CI all-green, CHANGELOG maintained.
