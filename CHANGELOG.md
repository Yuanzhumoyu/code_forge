# CHANGELOG

<!-- markdownlint-configure-file { "MD013": { "line_length": 512, "code_block_line_length": 512, "heading_line_length": 512 }, "MD024": false } -->
<!-- 文件级豁免原因：历史条目按单行记录（最长 ~440 列），且多年条目同置 [Unreleased] 下，
     版本子节用 `### Added (日期)`，同层 Fixed/Changed 标题按惯例重复——均为 CHANGELOG
     结构与单行惯例，不按正文 120 列规则折行；重构留待 changelog 整顿时处理。 -->

All notable changes to the `code-forge` project will be documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.0.0/).

## [Unreleased]

### Fixed (2026-10-28) — `&str → &[u8]` 胖指针常量 transmute

+ `Cast(Transmute)` 的源是 `ConstValue::Slice`（`&str`/`&[T]` 字面量常量）时，按
  ScalarPair 写目标槽：ptr@0 = 数据地址、len@8 = meta。旧实现进标量兜底写 `null`，
  `PtrMetadata` 读 len=0（-O2 的 `str::as_bytes().len()` 得 0、`parse_u32` 恒 0；
  -O0 正常）。
+ sort 守卫探针追加 `"abcd".as_bytes().len()==4` 与首字节断言（O0/O1/O2）。
+ 未修（已登记）：-O0 `iter().copied().filter(..).sum()` 挂起。

### Fixed (2026-10-28) — `Ord::cmp`/`binary_search` 与负判别 switchInt

+ 新增 `BinOp::Cmp`（rustc 2026 三路比较）降级：i32 里 select -1/0/1 再 `ireduce`
  到 i8（`Ordering` 的 repr），浮点 fail-closed。
+ 窄有符号判别 switchInt：判别值 `sextend` 到 i64、case 常量按低位符号扩展后比较
  （修 `matches!(3.cmp(&7), Ordering::Less)` 在 -O0 恒假：A64 32 位 `CMPIMMW`
  拿 0xFF 比 0xFFFF_FFFF）。
+ 按需补编追加到 CGU0（多对象/DWARF per-CGU 对象数契约不变）；补编队列加
  `is_mir_available` 守卫（`extern` 声明/无 MIR 项不再触发 `instance_mir` ICE）。
+ sort 守卫探针追加 `Ord::cmp` 三态断言（Greater/Less/Equal），O0/O1/O2 仍真执行 15。
+ 已知新缺口（未修，已登记方案）：-O0 `iter().copied().filter(..).sum()` 挂起；
  -O2 `&str` 逐字节 `parse` 错值。

### Fixed (2026-10-28) — `-O1+` 缺失实例按需补编；aarch64 sort 三档真跑

+ forge-rustc 现在记录每个直接调用目标的 `(符号名, Instance)`（含 drop glue、函数项
  地址），并在 CGU 分区编译后做 fixpoint：把"被引用但未进分区"、
  `upstream_monomorphization` 为 `None` 的 `AvailableExternally` 实例补编进合成
  `<on-demand>` CGU。`-O1+` 的 `core::slice::sort` 因 `CopyOnDrop::drop`/`revswap`
  缺符号而链接失败的问题消失。
+ 守卫 `rustc_aarch64_sort_probe_runs_under_qemu` 升格为 **-O0/O1/O2**：16 元乱序
  `sort_unstable()` 后校验连续 0..15（真执行 15）。
+ LICM 默认关闭（`FORGE_ENABLE_LICM=1` 可 opt-in）：`-O2` 的 sort 在 LICM 下错值
  （最窄复现 `FORGE_LICM_ONLY=iconst`），待补全支配/插入位置/链依赖条件后转正；
  e2e/门禁在无 LICM 下全绿。

### Fixed (2026-10-28) — `core::slice::sort` 在 aarch64 真跑（4 类后端缺陷）

+ `ptr::copy_nonoverlapping`/`ptr::copy`：按元素 `size`+`align` 用 8/4/2/1 阶梯精确复制
  （旧实现每元素固定 8 字节：i32 越界 + 4 对齐 `str x` ⇒ arm64 Data Abort）；`copy` 补
  memmove 反向分支（重叠右移曾把元素复制成 `[2,2,4,3]`）。
+ 小聚合按 `backend_repr` 选 load/store 宽度（整数 Scalar / 自然对齐 Memory → I8..I64；
  指针 Scalar 保持 PTR）——修 `swap_chunk` 对 `MaybeUninit<[u8;4]>` 的 8 字节读写。
+ `TerminatorKind::Drop` 真调 drop glue（`CopyOnDrop` 的 gap 元素现在会写回）。
+ `size_of_val_raw::<[T]>`/`str` 从胖指针元数据动态求长（旧实现按 unsized layout 得 0，
  `swap_nonoverlapping` 被整段跳过）。
+ `FnDef` place 调用直接取实例符号地址（零大小函数项被当 8 字节 load ⇒ `blr 0`）。
+ arm64 补 `[[lower.Trap]]`（UDF）；`core::intrinsics::abort()` 按目标分派：Windows 保持
  `ExitProcess(3)`，非 Windows 落 `Trap`（freestanding 目标不再缺 `ExitProcess`）。
+ 新守卫 `rustc_aarch64_sort_probe_runs_under_qemu`（-O0）：16 元 `sort_unstable()` 排序后
  校验连续 0..15，真执行 15。

### Fixed (2026-10-28) — 常量折叠的 `Big→固定类型→Big` 往返丢数据

+ `Big::from_float_bits(u128, fmt)` / `Big::to_float_bits(fmt) -> u128`：f16/f32/f64/f128
  全部**精确**编解码（dashu `Real` + 原生 `split_bits`/`bit_len`/`bit`；f128 的 112 位尾数
  逐位保留，不经 f64；NaN 返回 `None`）。
+ 折叠层：`Fconst` 已知值按浮点解码（此前把位模式当整数）；浮点结果回 **Bits** 通道
  （f32 不再落成 f64 位模式低 32 位）；`+ - * / fmod fma sqrt floor/ceil/trunc/round`
  **全格式**折叠（守卫位中间态 + 一次正确舍入）。
+ 整数：`popcnt/clz/ctz/bitreverse/bswap` 用 dashu 原生位运算按目标位宽算（i128 高 64 位
  不再丢）；`sat` 算术用 Big 任意精度边界（i128 正确饱和）；移位量按位宽取模（与后端
  硬件语义一致），任意大移位量都能折叠。
+ 新增 9 条单测覆盖上述不变量（f128 尾数、NaN 拒绝、负零、i128 位操作/饱和/移位、f32 位宽）。

### Fixed (2026-10-28) — `const-fold` 按结果位宽截断 `icmp` 操作数（O1 误编译）

+ `fold_icmp` 用**结果类型**（i1）去截断比较操作数：`icmp ult i64 0, i64 4` 把 4 截成
  1 位 = 0 ⇒ 判成 false ⇒ `store i1 0` ⇒ `ub_checks` 的 `0 < pointee_size` 断言
  panic（arm64 `-O1` 的 `ExactSizeIterator::len` 一进就炸，`-O0/-O2` 正常）。
+ 修法：按各操作数自己的类型截断（有符号先 `truncate_to_type`、无符号走
  `as_unsigned_big`）。
+ 守卫升级：迭代器探针（真执行 20）与 fmt 探针（真执行 65）都覆盖 **O0/O1/O2 全档**。
+ `Big` 的比较回到**原生任意精度 `Ord`**：`PartialOrd` 直接复用 `Ord`（`Big::Float`
  是实数、拒绝 NaN ⇒ 全序），跨变体经 `Integer`/`Real` 精确转换（不再 `to_f64_lossy`）；
  `fold_fcmp`/`Fmin`/`Fmax`/`Fcopysign` 不再把值转 f64（f128/超大值不再丢精度或变 `Inf`）；
  删除语义含糊、与 `Ord` 冲突的私有 `cmp_total`。

### Fixed (2026-10-28) — arm64 在 `-O1/-O2` 编不过（pass 后 IR 非法）

+ `copy_agg` 的地址统一 `inttoptr` 成 PTR（sret 指针与间接聚合形参在签名里是 i64 槽）——
  修 `StoreAddrNotPointer`。
+ `CastKind::PointerExposeProvenance` / `PointerWithExposedProvenance` 显式
  `ptrtoint`/`inttoptr`；`ptr_offset_from(_unsigned)` 先把指针转整数再差/除——修
  `udiv ptr %x, i64 4` 这类非法 IR（`Iter::len` 必走）。
+ GVN-PRE `insert_expression` 复制表达式的 **immediates**——修 `MissingCondImmediate`
  （v22 起比较条件在立即数通道）。
+ 溢出槽最小 8 字节、8 对齐：vreg 类可能 4 字节宽，而写槽是 8 字节 `STRX`——修
  `Strx: immediate 4 不是 8 的整数倍`。
+ 守卫升级：`rustc_aarch64_fmt_probe_runs_under_qemu` 在 **O0/O1/O2 全档**真执行 65。
+ 已知未闭环：迭代器探针在 **O1** 运行期 panic（`const-fold` 误编译，`-O2` 正常），
  已在账本记录，下一步专项。

### Fixed (2026-10-28) — fp-inside 栈槽落在 callee-saved 保存区上

+ `frame_layout_info` 的 fp-inside 分支 `stack_slot_shift` 只算了 `fp_push_bytes`
  （16），漏掉 callee-saved 区（x19..x28，最多 80 字节）：局部槽会落进保存槽，
  函数返回时恢复出被局部值覆盖的 x21/x24（实测 `core::fmt::write` 的 `blr x24`
  跳到地址 0）。现在 `stack_slot_shift = fp_push_bytes + cs_bytes`。
+ 守卫 `rustc_aarch64_fmt_probe_runs_under_qemu`：`write!` 整数格式化 + 手写排序 +
  `count_ones` + Collatz，O0 真执行 **65**；`write!(buf, "{}", a)` 也真执行（输出 "1"）。
+ 定位手段：QEMU gdbstub 硬件写观察点打在 `Buf::write_str` 的 x24 保存槽上。
+ 连带修正：`aapcs64_struct` 的寄存器保存区改为 `contiguous = true`（帧顶，紧贴入口 sp）。
  原先排在局部槽之下，栈槽整体下移后偏移超出 `STURX` imm9 范围（`-368`）；
  AAPCS64 定本本来就要求 `__stack` 指向入口 sp。

### Fixed (2026-10-28) — ZST 字段写越界打断迭代器

+ 结构体逐字段构造（Adt/Tuple/Array）没跳过 ZST 字段：`Iter::_marker`（PhantomData）
  被写 8 字节到 `slot+16`，越界落在被调方 `x29` 保存槽 ⇒ 调用者 fp=0 ⇒ 任何
  `v.iter()` 都 Data Abort。现在与枚举分支一致：`f_sz == 0` 跳过。
+ 守卫 `rustc_aarch64_iterator_probe_runs_under_qemu`（`next()` 循环 + `copied().sum()`，
  O0 真执行 20）。定位手段：QEMU gdbstub 硬件写观察点 + `P 20` 跳过触发指令。
+ 仍未闭环：`core::fmt` 格式化跳 0（ELR=0x0）；O1/O2 const-fold 后 IR 校验失败。

### Fixed (2026-10-28) — ZST 字段/值不再产生 Store

+ 聚合构造里 `()` 等 ZST 字段原本会造出 `Iconst ty=VOID` 并 `Store` ⇒
  `Store no matching rule`（真实 fmt/sort 程序在 O0 全部卡住）。ZST 没有内存表示：
  字段循环按 `f_sz == 0` 跳过，`store_ty` 对 VOID 提前返回。
+ 守卫 `rustc_aarch64_zst_aggregate_probe_runs_under_qemu`（`S{a,b:()}`，O0 真执行 7）。

### Added (2026-10-28) — aarch64：`select_unpredictable` / `Clz` / 大帧

+ `select_unpredictable` intrinsic 映射到 `Select`（core 的 sort 实现依赖它）。
+ 新 `[form.DP1BIT]` 与 `CLZ`/`RBIT`（X/W 两档）；`Clz` 降级补齐（arm64 op_gap 59/44）。
+ 帧调整分块：`sp_adjust`（frame_alloc/free）与 `frame_set` 都按槽 `min/max` 发多条，
  `[u64; 600]`（5936B 帧）在 O0/O2 真执行通过（守卫
  `rustc_aarch64_big_frame_probe_runs_under_qemu`，期望 1799）。
+ pins：arm64 `59, 6, 9, 44` / `unassigned_bits 276` / `vary 61` / `spec_totals 626`。

### Added (2026-10-27) — arm64 原子（LSE：SWPAL/LDADDAL/LDCLRAL/LDSETAL/LDEORAL/CASAL）

+ 新 `[form.LSE3]` 与六个原子族（X/W 两档）：`AtomicRmw`（xchg/add/sub/and/or/xor）与
  `Cmpxchg` 用 LSE 单指令实现（AL = seqcst 档；IR 的 ordering 是第二立即数，统一用最强序）。
+ CAS 的 `[11:10]` 位必须为 `11`（CASP 位型）；只写 4 位 opc2 会编出非法指令（QEMU 挂起）。
+ 矩阵解锁 5 条原子用例：arm64 **100 passed / 0 failed**；QEMU 测试默认 CPU 改 `max`
  （LSE 需 ARMv8.1-A），`FORGE_A64_CPU` 可覆盖。
+ pins 刷新：`op_gap = 58,6,9,45`、`spec_totals` 622、`roundtrip` +24、encoding digest。

### Added (2026-10-27) — arm64 SMULH 与 SmulOverflow

+ 新指令 `SMULH`（MUL3 形状，`ra` 固定 31）：有符号乘法的 64 位高半。
+ `SmulOverflow`（64/32）：64 位用 `SMULH + MUL + ASRV #63` 比较；32 位把操作数 `sxtw`
  进 64 位乘积后判 `p>>63 != p>>31`。arm64 矩阵解锁 2 条 `smul_overflow_*` 用例
  （**95 passed / 0 failed**）。
+ 注意：`ADD_X_EXTW_NA` 的 ext 字段是 `[enum.extend_w]` 枚举值（`sxtw = 6`）。

### Added (2026-10-27) — arm64 GlobalAddr + UmulOverflow（debug 档 core 探针真跑通）

+ `[asm.reloc.<名>]` 新数据键 `width`（补丁宽度，缺省 = 槽宽）与 `fixup`（`slot`|`inst`）：
  AArch64 的 `MOVW` 族是**指令字补丁**（16 位立即数嵌在 4 字节指令里），用 `fixup = "inst"`
  + `width = 4` 表达；链接器类型仍写在 `conventions/*.toml` 的 `[link.elf]`。
+ arm64 `GlobalAddr`：`MOVZ/MOVK` 四片（G3→G2→G1→G0）带 `R_AARCH64_MOVW_UABS_G*` 重定位，
  静态数据地址可被链接器解析（探针读 `static MAGIC = 5` 得到 5）。
+ arm64 `UmulOverflow`（64/32）：`MUL + MOVN + UDIV + CMP/CSEL + AND` 分支自由判溢出。
+ 守卫 `rustc_aarch64_core_swap_probe_runs_under_qemu` 扩成 **O0 / O2 / O0+debug-assertions**
  三档（都断言 13）：debug 档会编出并执行 core 的 precondition/overflow 检查。
+ `QemuAarch64Executor::exec_module` 补齐（多函数 + 全局数据）：`@N` 调用与 `G{id}` 全局地址
  就地补丁，全局地址用数据段**页对齐 p_vaddr**（用文件偏移会一读就 fault/hang）。arm64 矩阵
  因此多跑 4 条 Module 用例：**93 passed / 0 failed**。

### Fixed (2026-10-27) — arm64/riscv 控制流 effect 缺失导致活区间 CFG 无边

+ arm64 的 `B`/`B_*`/`CBZ`/`CBNZ`/`BL`/`BLR`、riscv 的 `JAL`/条件分支补上 `effect`
  （Jump/Branch/Call）——`is_branch`/`branch_targets` 只从 effect 派生，缺了它循环回边
  不进 CFG，preheader 常量被当成已死、寄存器在循环体里被复用（`core::mem::swap` 实测
  上界被指针覆盖 → 越界 fault）。
+ arm64 `[machine].scratch` 2 → 3（X16/X17/X18）：修好 CFG 后实测"一条指令要 3 个 scratch"。
+ 新 QEMU 守卫 `rustc_aarch64_core_swap_probe_runs_under_qemu`：`core::mem::swap` + 循环探针
  在 O0/O2 两档都断言返回 8（此前两档都挂）。
+ `docs/reference/isa-dsl.md` 的 effect 小节加红线：控制流指令必须写 effect。
+ 新增观测开关 `FORGE_TRACE_LIVE_FN`（活区间 dump 的函数名过滤；已进 KNOBS 台账与契约文档表）。
+ `arm64_exec` 的 core 探针测试各自独立产物目录 + `current_dir`：并行跑时不再争用 rustc 的
  `forge_codegen_output*` 临时对象（此前 gate 里会 `无法把对象写到 …（os error 2）`）。

### Changed (2026-10-26) — 栈实参写槽统一 + 缺失值描述补齐 + 诊断带属性

+ `move_args` 的"spilled 形参 → spill 槽"改走 `emit_spill_store`（统一 fp/sp 口径与按类
  分派），修掉大函数 `Sturx: -352` 编不出。
+ `lower_inst` 前用 IR 类型补齐**缺失**的值描述（`desc_of`）——修复 O2 下
  `Iadd no matching rule（rs1_width=None）`（按宽度分派的规则需要描述）。
+ 无规则诊断升级：打印 8 个谓词属性 + 每个实参描述，`None` 直接暴露"描述缺失"根因。
+ 记录 core swap 的下一步：O2 可编过但运行坏码（`csel` 临时被当作 store 地址），疑似
  指令包内 vreg 编号/字段串位。

### Added (2026-10-26) — aarch64 scaled 溢出真执行守卫

+ 新测试 `rustc_aarch64_scaled_spill_probe_runs_under_qemu`：40 个跨调用活跃标量强制溢出，
  断言对象里**没有** unscaled `stur/ldur`、**有** `str/ldr [sp,#imm>255]`，并 QEMU 真跑出
  `39`（实测帧 ~0x6c0、24 条 >255B scaled 访存）——证明上一轮 SP+scaled imm12 形态真被走到。
+ 记录 core swap 侦查结论与 O2 暴露的两个后续缺口（栈实参负偏移仍 imm9、某实例
  `Iadd no matching rule`），见方案 M6-107。

### Changed (2026-10-26) — aarch64/riscv 大帧溢出：imm9 → SP + scaled imm12

+ `[machine.spill.<名>].offset_base = "sp"`（数据键）：发射侧把溢出偏移换成 sp 口径
  （fp-inside 下 = `frame + fp_offset`），模板用无符号 scaled imm12 ⇒ 溢出范围从
  ±256B 扩到 32KB 量级；arm64 的溢出与 callee-save 全部换到 `LDRX/STRX`、
  `LDRD_ND/STRD_ND`、`LDRQ_FP/STRQ_FP`。
+ 栈形参的**负**偏移仍留在 `LDURX/STURX`（scaled imm12 只收无符号）——角色按形态拆分。
+ 修正 fp-inside 帧记账：局部槽在 `fp - stack_slot_shift - depth`，帧须补
  `stack_slot_shift`（否则深槽落到 `sp` 之下）。
+ 新矩阵用例 `deep_frame_many_slots`（40 个局部槽，真执行）⇒ `jit_matrix_arm64`
  **89 passed / 0 failed**；x86/riscv 矩阵同步回归绿。

### Added (2026-10-25) — aarch64 + `core` 首光（溢出/Undef 规则 + 可运行闭环）

+ **aarch64 真跑 `#![no_std]` + `core`**：forge 编出对象，与从 `rust-src` 直接为目标编的
  `libcore.rlib`/`libcompiler_builtins.rlib` 一起 `ld.lld` 链接，QEMU 退出码 12；
  新增测试 `rustc_aarch64_core_probe_runs_under_qemu`。
+ arm64 谱面补 `SaddOverflow/UaddOverflow/SsubOverflow/UsubOverflow`（`ADDS/SUBS` +
  `CSEL`/`CSEL_F` 读标志）与 `Undef/Poison`（零值特化）。
+ lowering 兜底错误现在**点名**缺失的 op（旧文本只有 `DSL lowering`）。
+ 结果：arm64 op 覆盖 47→53、真缺口 56→50。仍缺 `SmulOverflow/UmulOverflow`、`GlobalAddr`、
  大函数溢出槽偏移超 imm9（帧形态选择）——已记入方案 M6-105 的下一步。

### Added (2026-10-25) — 窄宽度访存 / 掩码截断 / UDF 陷阱（arm64 + riscv64）

+ `Load`/`Store` 补 **i1/i8/i16**：arm64 用 `ldrb/ldrh/strb/strh`、riscv64 用
  `lbu/lhu/sb/sh`；`Ireduce` 补 1/8/16 位截断的**显式掩码**（`and 1`/`and 0xFF`/`and 0xFFFF`）。
+ arm64 新增 `UDF`（全零字）并标 `effect = ["Trap"]`：`Unreachable` 现在按 DSL 约定
  发射陷阱指令；守卫 `tm_unreachable_emits_udf`。
+ 矩阵新增三条真执行用例 `narrow_i8_store_load` / `narrow_i16_store_load` /
  `bool_i1_store_load` ⇒ arm64 **79 passed / 0 failed**、riscv64 **139 passed / 0 failed**。
+ aarch64 `core` 路线侦察：可从 rust-src 直接编 `libcore.rlib` + `libcompiler_builtins.rlib`
  供 forge 后端使用；当前阻塞在**溢出槽偏移超出 imm9**（下一步修帧/溢出形态选择）。

### Added (2026-10-25) — arm64：`Fcopysign`

+ `Fcopysign` 降级 = `fabs` + GPR 里拼符号位（`fmov` 取位型 → `movz` 造单 bit 掩码 →
  `and` → `orr` → `fmov` 回 FPR），32/64 两套。
+ 矩阵新增两条可判定用例 `fcopysign_neg_to_pos_i64` / `fcopysign_pos_to_neg_i64`
  （经 `Fptosi` 落进 i8 值域）。结果：`jit_matrix_arm64` **77 passed / 0 failed**；
  arm64 op 覆盖 46→47、真缺口 57→56。

### Added (2026-10-25) — arm64：浮点常量 / Fma / 取整 + FPR↔GPR 桥

+ 新增占位符 `{fconst_f0}..{fconst_f3}`（浮点位模式的 16 位分片，与 `{iconst_f*}` 同序），
  以及 `fmov x↔d` / `w↔s` 四条桥指令 ⇒ `Fconst` 可用（4 片 movz/movk + `fmov` 进 FPR）。
+ 新增 `fmadd`（`Fma`）、`frintp`/`frintm`（`Fceil`/`Ffloor`）的 S/D 编码与降级。
+ 修正 `Fcmp` 降级谓词：`elem` 取的是**结果**类型（bool ⇒ 5），全部规则不命中；
  改用 `rs1_width`（32/64）。
+ 结果：`jit_matrix_arm64` **75 passed / 0 failed**（was 63）；arm64 op 覆盖 42→46、真缺口 61→57。

### Added (2026-10-25) — arm64：标量浮点（算术 / 比较 / 转换 / 访存）

+ `isa/arm64.toml` 新增标量浮点编码（clang oracle 逐字节核对）：`fadd/fsub/fmul/
  fdiv/fmax/fmin`(s/d)、`fneg/fabs`、`fcmp`、`fcvt`(f32↔f64)、`scvtf/ucvtf`、
  `fcvtzs/fcvtzu`、FP `ldr/str`。所有标量浮点统一成 **11 位 opcode 前缀**
  （解码 trie 要求固定位段两两不相交）。
+ 降级：`Fadd/Fsub/Fmul/Fdiv/Fmax/Fmin/Fneg/Fabs`、`Fcmp`（14 条件，NaN 条件用
  `csel` + `vs` 组合，`mi/vs/vc` 走 flag 银行 `CSEL_F`+`FlagCC`）、
  `Fpext/Fptrunc`、`Sitofp/Uitofp/Fptosi/Fptoui`、`Fload/Fstore`。
+ QEMU aarch64 夹具：crt0 开 `CPACR_EL1.FPEN`；浮点实参进 `d0..d7`
  （`Executor::exec_f64_args` 已有 aarch64 实现）。
+ 结果：`jit_matrix_arm64` **63 passed / 0 failed**；arm64 op 覆盖 25→42、真缺口 78→61。

### Added (2026-10-25) — arm64：移位 / 除法 / 取余

+ `isa/arm64.toml` 新增 `LSLV`/`LSRV`/`ASRV` 的 X/W 六条编码（`DIV2A` 族，clang
  oracle 逐字节核对），并补齐 `Ishl`/`Ushr`/`Sshr`（LSLV/LSRV/ASRV）、
  `Sdiv`/`Udiv`（SDIV/UDIV）、`Srem`/`Urem`（`q=a/b; a−q*b`，SDIV/UDIV + MSUB）。
+ 结果：`jit_matrix_arm64` **61 passed / 0 failed**（was 50）；arm64 op 覆盖 18→25、
  真缺口 85→78。

### Added (2026-10-24) — arm64：比较 / 选择 / 窄宽扩展；aapcs64 变参可编可跑

+ `isa/arm64.toml` 补齐 `Icmp`（`cmp` + `csel 1/0`）、`Select`、`Sextend`/`Uextend`
  （`add {dst}, {zero}, {src}, sxtb/sxth/sxtw/uxtb/uxth/uxtw`）、`Ireduce`，以及
  `Band`/`Bor`/`Bxor` 的窄宽度（i1/i8/i16/i32→W）兜底。
+ 结果：`jit_matrix_arm64` **50 passed / 0 failed**（was 26）；aapcs64 变参
  （`va_start` + `va_arg`）在 arm64 上**编得出并在 QEMU 真跑**，
  原"必须 Unsupported"守卫升级为"必须编得出"。
+ 已知坑（已写进注释）：扩展寄存器 ADD 的 **Rn=31 是 SP 不是 XZR**（首版
  `add x27, sp, w28, sxtw` 把栈地址当结果）；窄宽度逻辑运算不能只靠
  `dispatch = "rs1_width"`（它只排序不兜底）。

### Added (2026-10-24) — `forge-rustc`：`--target aarch64-unknown-none` 首光

+ 后端按 `--target` 建对象（此前固定 `TargetConfig::host()`，任何非宿主目标都会
  静默写出 host 的 COFF 对象）；裸机三元组（`*-none`）按 **ELF** 处理。
+ 对象层重定位改成**数据驱动**（库里零架构分支）：`Relocation` 带上 ISA 语义名，
  链接器重定位类型写在 `conventions/*.toml` 的 `[link.<format>]`（arm64 的
  `call26 = { type = 283, clear_mask = 0x03FF_FFFF }` = `R_AARCH64_CALL26`）；
  `forge-object` 只按 `(格式, 名字)` 查表落类型、按 `clear_mask` 清零位段，
  未命中回退 `object` 的 Generic。**新增指令集只加 ISA/约定 TOML，不改
  `forge-object`/`forge-rustc` 代码**。
+ `[asm.reloc.<名>]` 支持**仅命名形态**（省略 `operand`）：给指令的 label 槽外部引用
  起语义名（arm64 `BL` 的 `call26`），供目标约定映射到链接器重定位类型。
+ arm64 谱面补齐 `StackAddr` / `Load` / `Store` / `Inttoptr` / `Ptrtoint` 的降级规则；
  新占位符 `{off_f0}`..`{off_f3}`（栈槽偏移的 16 位分片，供 `movz+movk` 造任意偏移）；
  降级模板校验现在接受算术表达式占位符。**同时把 arm64 的 `Iconst` 改成按 X 宽
  材料化**（规范寄存器形态 = 符号扩展后的 64 位，与 x86 的 `MOV_REG_IMM64` 同约定）
  ——W 形式会把 `-4` 的高位清零，混合宽度使用（`iadd(ptr, i32)`）会算出野地址。
+ Windows VirtualAlloc 的 alloc runtime 只在 Windows x64 注入（目标面由运行时自己
  声明 `SUPPORTED_ARCHES`，后端不再按 arch 分支判断）。
+ 守卫：`forge-rustc` 的 `arm64_exec`（ELF `e_machine=183` + clang crt0 + `ld.lld` +
  `qemu-system-aarch64` 真执行 42）进全量门禁（`arm64-rustc-exec`）。
+ **边界**：当前仅 `#![no_core]` crate（本机无 aarch64 core/std）；Icmp/Select/移位/
  除法/原子与 DWARF/CFI 的 aarch64 形态尚未覆盖。

### Added (2026-10-24) — arm64 后端：函数调用（`BL`/`BLR`）与 QEMU 真执行

+ `isa/arm64.toml` 的 `BL` 现在声明 `roles = ["call"]`、`BLR` 声明
  `roles = ["call_indirect"]`，"最简 CPU" 之外又补上了**函数调用**这条谱面能力；
  `LDURX/STURX` 同时承担 `stack_arg_load/stack_arg_store`（栈形参收参 / 传参写槽）。
+ 测试夹具（`forge-tests` 的 `exec/qemu_aarch64.rs`）现在会布局**全部**函数并按 `@N`
  重定位回填 `BL` 的 imm26：`qemu-system-aarch64` 真执行覆盖直接调用（add(20,22)=42）
  与栈实参（sum9(1..9)=45）。
+ 由真机执行纠正 `aapcs64` 的数据：`first_offset_slots` 2 → **0**（本实现的 arm64 帧基址
  等于入口 sp，A64 `bl` 不压返回地址 ⇒ 首个栈实参在 `[x29]`）；变参对象 `__stack`
  初值同步为 `FrameOff(0)`。

### Fixed (2026-10-24) — 帧记账：spill 槽 0 与 `call` 返回地址重叠（M6-⑩）

+ `callee_saved_bytes` 不再把帧指针保存槽算进"rbp 之下"：`push rbp; mov rsp,rbp` 之后
  rbp 指向那个槽。此前 spill 基点比 `rsp` 低 8 字节，**槽 0 正好是 `call` 压返回地址的位置**；
  循环内一调用就把槽写成代码地址，下一轮 reload 拿它当基址 ⇒ 访问违规。
+ `@move_args` 的 spilled-param 基点改用 `AllocResult.callee_saved_bytes`（与 emission 同源），
  不再在生成物里用 `fp_push + saved*slot` 另算一份。
+ `LICM` 对 `StackAddr` 的外提回归默认（`FORGE_LICM_STACKADDR` 门控删除）：此前的"不外提"
  规避的就是上述帧缺陷。守卫：帧账单测 `outside_spill_base_excludes_the_frame_pointer_slot`、
  LICM 单测 `stack_addr_is_hoisted_by_default`、e2e `opt_loop_call_inline`（O2、真执行）。

### Added (2026-10-24) — `forge-rustc`：内联 `asm!` 的 `label` 操作数

+ `asm!("jmp {0}", label { ... })` / `asm!("jcc e, {0}", label { ... })` 现在可用：`label`
  指向的目标基本块被映射成"外部标签"（名字 → IR 块下标），汇编器回填后由编码器
  `use_label_at` 生成相对位移；命中/不命中两个方向都有 e2e（`inline_asm_label*`）。
+ 连带修复：内联 asm 的可达性分析此前只看 fallthrough（`targets[0]`），`label` 目标块会被
  当不可达；现在 `targets` 全部计入。
+ **边界**：外部标签的值与"文本标签回填值"同空间（IR 块下标），所以只对**标签槽 unit = 1**
  的 ISA 成立（x86_64 变长谱）；定宽谱（如 A64 的 4 字节单位）会**明确拒绝**，不静默乘错。

### Added (2026-10-24) — `forge-rustc`：内联 `asm!` 的 `sym` 操作数

+ `asm!("mov {0}, {1}", out(reg) p, sym target)` / `sym STATIC` 现在可用：符号**地址**被物化成
  IR 值（隐藏全局符号 + `abs64` 重定位），以输入寄存器承载，模板按寄存器形态使用
  （`mov` / `call {0}` / `[…{0}…]` 均正确）。
+ **语义边界（fail-closed）**：把 `sym` 当**字面符号**写的模板（如 `[rip + {0}]`）会被
  汇编器拒绝并给出编译期错误，不会静默生成错码。
+ 顺带修复内联 asm 定寄存器的一个缺陷：池选择现在避开**所有显式名**（此前 `sym` 可能
  先挑走 `RAX`，随后显式 `out("rax")` 被判成"同一寄存器重复使用"），并允许纯 clobber
  与值槽同名（LLVM 的 `={rax},~{rax}` 同形）。
+ **回归守卫**：e2e 新增 `inline_asm_sym` / `inline_asm_sym_call` / `inline_asm_sym_literal`
  （删除过时的 `neg_inline_asm_sym`），README 机读计数同步到 205。

### Fixed (2026-10-24) — `forge-rustc`：函数指针 / `dyn` 虚调用不再崩溃或丢实参

+ **函数指针常量是 `nullptr`**：`let f: fn(i64)->i64 = add1;` 的常量分配是
  `GlobalAlloc::Function`，不在数据段路径里 ⇒ 被当成标量求值成 0 ⇒ 间接调用 AV
  （连最普通的标量 fn 指针都崩）。现在取该实例的符号地址（隐藏全局符号 + `abs64` 重定位），
  `FnDef` 常量与 `GlobalAlloc::Function` 两种形态都接。
+ **虚方法丢显式实参**：`dyn Trait` 分支此前只传 receiver（`args[0]`），vtable 之后的
  实参整批丢弃 ⇒ 带参数的 `dyn` 方法一律错值。现在按
  `[receiver.data] + args[2..]` 传（vtable 是元数据）。
+ **间接调用的 `u128`/`i128` 形态**：fn 指针与虚方法拿不到被调方实例 ⇒ 由
  `FnSig` 走 `fn_abi_of_fn_ptr` 取 `PassMode`（`wide_int_abi_of_callee_ty`），不再
  fail-closed；`[cap:WIDE_INT_ABI]` 收窄到"签名本身不可得"的边角。
+ **回归守卫**：e2e 新增 `fnptr_scalar` / `fnptr_u128` / `dyn_args` / `dyn_u128`
  （全 42）；README 机读计数同步到 203。

### Fixed (2026-10-24) — `forge-rustc`：128 位标量的 C-ABI 与 LLVM 双向互操作（`WA-52` 收口）

+ **症状**：`extern "C"` 边界上的 `u128`/`i128` 与 **LLVM 编的代码混链**时崩溃或错值
  （实测调用方 AV `0xC0000005`、被调方返回垃圾）。自编自调的 e2e **测不出来**：两边用同一套
  （错误的）形态也自洽——这正是该缺陷长期只在"混链"时暴露的原因。
+ **真因**：降级层用**启发式**（`is_scalar_pair_abi`）决定 128 位值的形态，一律发两个整数槽
  （`RCX:RDX` / `RAX:RDX`）；而 Win64 的 C 家族是**按引用传参 + `XMM0` 返回**（LLVM 实测
  `mov 0x8(%rcx),%rax` / `movdqa (%rsp),%xmm0`）。
+ **修法**：形态改**问 rustc `FnAbi` 的 `PassMode`**——`Indirect` ⇒ 传"16 字节对齐副本"的指针；
  `Cast{Vector 16}` ⇒ 以 `V128` 返回（`XMM0`）；`Direct` ⇒ 两整数槽（Rust ABI 仍是 `RAX:RDX`）。
  定义侧（签名/收参/返回）与调用侧（实参/结果）同源；被调方解析不出（间接调用）⇒ 明确
  `[cap:WIDE_INT_SHAPE]` fail-closed。合法化层的"按大小"ABI 编组显式排除**向量**
  （≤16B 向量按值进 XMM，>16B 是 sret）。
+ **回归守卫**：新增 `crates/tools/forge-rustc/tests/cross_abi.rs`——两个方向都**真执行** 42：
  forge 编调用方 + LLVM 编被调方、LLVM 编调用方 + forge 编被调方；覆盖按引用实参 /
  向量返回 / 两者同时。门禁新增 `rustc-abi-cross` 步骤。
+ **验证**：`cross_abi` 2/2；e2e 全量不回归（`u128_abi_param`/`u128_abi_return` 钉住 Rust ABI
  两槽形态）；`forge-codegen --lib` 2864/2864；`forge-rustc` clippy 干净。

### Fixed (2026-10-19) — `forge-rustc`：`--emit=obj -o <path>` 现在真的写出那个路径

+ **症状**：`rustc -Zcodegen-backend=forge_rustc.dll --emit=obj -o foo.obj` 编译"成功"，
  但 `foo.obj` **根本不存在**——目录里只多出后端自选的 `forge_codegen_output.o`。
  同一条命令交 LLVM 后端则正常得到 `foo.obj`。
+ **真因**：把 CGU 临时对象复制到用户 `-o` 的那一步（`copy_if_one_unit`）住在
  `rustc_codegen_ssa::back::write` 里，只有"走 ssa 自带 write 流程"的后端才执行；
  自定义 `CodegenBackend` 走 `codegen_crate` → `link_binary`，**这一跳不会替我们复制**。
+ **修法**：单对象时在写完自己的对象后，再复制一份到 `output_filenames().path(Object)`
  （它优先返回用户的 `-o`，否则是 `--out-dir` + 文件名词干 + `.o`）；用户点名了具体文件时
  清掉我们那份临时对象（`-C save-temps` 保留）。多 CGU 维持 rustc 自己的口径
  （警告 + 保留编号文件），不逐个覆盖同一目标。
+ **回归守卫**：新增 `crates/tools/forge-rustc/tests/object_output.rs`——同时钉
  "`-o <具体文件>` 必须存在且不留临时对象"与"`--out-dir` 形态下固定名仍在"
  （确定性与数据符号两类测试按 `forge_codegen_output*` 前缀认产物）。

### Changed (2026-10-18) — v28 新常量池：比特值统一 + 四元枚举句柄 + `AsConst`（IR 二进制 v6）

+ **`ConstantPool` 重设计**（不兼容旧池）：整数与浮点合并为**比特值** `BitValue { bits, width }`
  ——"整数还是浮点"由使用点的类型/opcode 决定，池不再分通道；新增 `intern_bits` /
  `intern_int`（符号扩展规范形，`i1` 例外）/ `intern_float` / `intern_agg` / `intern_vector`
  （**端序进去重键**）/ `intern_big`；读取统一为 `get → ConstValue` 与 `bits`/`big`/
  `vector`/`aggregate` 分型入口，视图强转 `as_i64`/`as_big`/`as_f64_bits`（按位宽取补码值）；
  跨池 `import`（聚合递归）。删除：bool 预置槽、`int_dedup`/`float_dedup` 与 `big` 双去重、
  `insert_sint`、`total_len`、死类型 `IntConst`/`FloatConst`。
+ **`ConstId` 四元枚举**：`Bits` / `Big` / `Vector` / `Agg`（2 bit tag 恰好铺满 ⇒ `raw`/`from_raw`
  全域定义、永不 panic）；`AggId`/`AggChild` 删除，聚合并入同一句柄空间
  （`ValueDef::AggConst(ConstId)`、`Immediate::Agg` → `Immediate::Const`）。
+ **`AsConst` 成为唯一常量糖**：`fb.constant(42i32)` / `42i32.as_const(&mut fb)`；
  原语精简为 `const_int` / `const_float` / `const_bool` / `const_big`；
  删除 `iconst_i8/i16/i32/i64`、`fconst_f32/f64`、`iconst_i128`、`fconst128`。
+ **IR 二进制格式升到 v6**（`IR_FORMAT_VERSION = 6`）：CONSTS 改为带 tag 单流
  （bits / big / vector / aggregate），**无兼容读取**；`docs/reference/binary-format.md` 同步。
+ **验证**：`cargo test -p forge-ir` 全绿（含新增"合并键/无预置槽/端序进键/跨池导入/
  句柄编码全域/`AsConst`"6 组回归）；`cargo test --workspace --exclude forge-rustc` 全绿；
  `forge-rustc` e2e **164/164**。

### Fixed (2026-10-11) — v27 M5：NaN 比较缺陷修复（`Ne` 该走无序不等 `Une`）

+ **先测后改**：用位掩码探针把六种浮点比较逐项打出来 ⇒ 只有 **`NaN != 1.0` 返回 false**
  （应为 true），其余五项全对 ⇒ 说明前两轮"重写谱规则"的方向完全错了，谱是对的。
+ **真因**：`lower/rvalue.rs` 把 `BinOp::Ne` 映射成 `FloatCC::One`（**有序**不等）；
  Rust 的 `!=` 是**无序**不等（`Une`，任一侧 NaN ⇒ true）。
+ **修法**：改成 `FloatCC::Une`（一行）。`Eq`/`<`/`>`/`<=`/`>=` 本来就映射到有序条件 ✔。
+ **验证**：`float_cmp_semantics`（六项判定 + 掩码断言）⇒ **exit=42**，用例数 **128**。
+ 账本 §2 的「M5 未闭环清单」同步把 NaN 一项划掉（**M5 仍未完成**，其余项仍在）。

### Docs (2026-10-11) — 状态修正：**M5 未完成**（不提交 PR）

+ 用户明确要求「M5 没有完成，不要提交 PR」⇒ 修正方案账本 §2 状态总表：M5 由 `✅ 落地` 改为
  **🔶 进行中（未完成）**，并新增「M5 未闭环清单」：
  ① **NaN 比较缺陷仍在**（`0.0/0.0 == 0.0/0.0` 返回 `true`）；
  ② "已知缺陷不进套件"本身就是未闭环证据（零容忍目前靠把有缺陷的用例挡在套件外）；
  ③ `i128`/`u128` 仍 `[cap:WIDE_INT]` 拒绝；④ `label`/`sym` 仍 fail-closed。
+ 确认纪律：**子集全绿 + 一次提交 ≠ 里程碑完成**；完成 = 契约面无已知缺陷。
+ 本轮无任何 PR / 看板动作。

### Added (2026-10-11) — v27 M5 补面之八：溢出 panic 路径用例（此前零覆盖）

+ 新增 `overflow_panic_shl`（`1u32 << 32`）与 `overflow_panic_div`（`i32::MIN / -1`）⇒ 两条都
  **exit=101**（模板 panic 处理器），证明 `-C overflow-checks=on` 的**运行期检查真的走得到**。
+ 两次自己的坑（记进账本）：① 直接写常量会被**编译期 lint** `unconditional_panic` 拦下，走不到
  运行期检查；② 改成非常量还不够，需要 `core::hint::black_box` 才能阻止**常量折叠**。
+ **同时探到新缺陷（未进套件，记入台账）**：NaN 比较语义错——`0.0/0.0 == 0.0/0.0` 在 Rust 里必须
  `false`，后端给 `true`；可疑点是 `comisd` 在无序时置 **PF** 而规则只发 `sete`（缺 `setnp` 合取）。
+ 用例数 **127**。

### Fixed (2026-10-11) — v27 M5 缺陷 ②：`f64 → i8/i16` 转换补上谱规则

+ **真因**：`elem` 谓词用的既不是位宽也不是 `TypeId`，而是 `dsl/codegen/lowering.rs::elem_id_of`
  的**自有编号**（`F32=1/F64=2/I32=3/I64=4/I8=5/I16=6`）；既有规则 `{ in = ["elem", [3,4]] }`
  覆盖的是 **i32/i64** ⇒ `f64 → i8/i16`（elem 5/6）**一条规则都没有**，`3.99f64 as u8` 因此报
  `[cap:SPEC_LOWERING_RULE_MISSING] Fptosi no matching rule`。
+ **修法**（`isa/x86.toml`）：补 `Fptosi`/`Fptoui` 的 `(rs1_width=64 && elem ∈ {5,6})` 两条规则，
  `CVTTSD2SI`（x86 只有 32/64 位目的形式 ⇒ 先按 32 位目的截断，`as` 就是截断向零）。
+ **验证**：`cast_int_float_matrix`（`-7i32→i8→i64→f64→i32`、`3.99f64 as u8`、`12345u64 as f64 as u64`、
  `-2.5f32 as i8`、`1e10f64 as i64`）⇒ **exit=42**，用例数 **125**。
+ 两条被否掉的猜法记进账本（`elem` 当位宽 ✗、当 `TypeId` ✗），另记一条坑：
  **规则插在 `when` 与 `insts` 之间会静默失效**，必须插在完整规则之后。

### Docs (2026-10-11) — v27 M5 缺陷 ② 调查：排除三种猜法（`elem` = TypeId，不是位宽）

+ 缺陷 ②（`3.99f64 as u8` ⇒ `Fptosi no matching rule`）本轮尝试补谱规则，**仍未命中**，
  按"不留未验证改动"**回退了谱与用例**，把实测事实记进方案账本 §8：
  1. `elem` 谓词的值是**结果类型的 TypeId**（`VOID=0/I8=2/I16=3/I32=4/I64=5/F32=6/F64=7/PTR=8`），
     **不是位宽**——既有规则 `{ in = ["elem", [3,4]] }` 覆盖的是 i16/i32；
  2. 照此补 `rs1_width=64 && elem=[2]`(I8) 的两条规则**仍然不命中** ⇒ `rs1_width` 或 `elem`
     在"转换"这种 op 上取到的值与我的假设不同（转换的源是浮点、结果是整数，没有常规的
     "源操作数宽度"语义）；
  3. 下一步应**在 lowering 侧打印实际谓词上下文**（或读 `dsl/codegen/integration.rs` 的
     `elem`/`rs1_width` 取值路径），而不是继续猜。

### Fixed (2026-10-11) — v27 M5 缺陷 ①/③：窄有符号值被按**零扩展**参与运算

+ **根因**（`FORGE_TRACE_ALLOC` 实证）：`i8` 局部变量用 `MovzxR8Mem` **零扩展**载入
  （`-128` ⇒ `0x80` = 128），谱规则里的 `movsxd {out}, {0}` 只扩展低 32 位符号位、**救不回来**
  ⇒ `i8: -128 >> 7` 得到 1（应为 -1）、`-100i8 / 3` 得到 52（应为 -33）。
+ **修法**（`lower/rvalue.rs::lower_binary_op`）：把"窄操作数扩展"从**只对比较**扩展到
  **所有窄整数运算**（算术/位运算/移位）——按**符号性** `sextend`/`uextend` 到 64 位，
  运算后再 `reduce_narrow_shift` **截回原宽度**（否则高阶位会漏出去）；比较仍返回 i1。
+ **验证**：`narrow_shift_semantics`（`i8 >> 7` = -1、`u8/u16` 逻辑右移 = 1）与
  `narrow_arith_signed`（回绕加/乘、除法/取模向零截断，`-100i8*40i8` = 96 由 LLVM 对照）
  ⇒ 两条都 **exit=42**，用例数 **124**。
+ 剩余缺陷 ②（浮点→整数转换缺谱规则 `Fptosi`）仍待做，记在方案账本 §8。

### Docs (2026-10-11) — v27 M5 缺陷台账：窄类型/转换三处（可复现，含 LLVM 对照值）

+ 继续"探未覆盖语义族"，探到三处缺陷，**全部未进 e2e 套件**（套件零容忍，带缺陷的用例
  不能提交）⇒ 以"复现体 + 对照值"记进方案账本 §8 的缺陷台账：
  ① **窄有符号右移错值**：`i8: -128 >> 7` 后端 1、LLVM -1（无符号是对的；已排除"加载不
  符号扩展"，`(a as i64) + 200` = 72 ✔）；可疑点是谱的 `[[lower.Sshr]]` 序列里**硬写 RCX**；
  ② **浮点→整数转换缺规则**：`3.99f64 as u8` ⇒ `[cap:SPEC_LOWERING_RULE_MISSING] Fptosi no matching rule`
  （x86 只有 32/64 位目的形式，8/16 位要先转再截断，属谱侧规则序列）；
  ③ **窄有符号算术**：`-100i8 * 40i8` 回绕应 96（LLVM 实证），后端不符（可能与 ① 同源）。
+ 修复顺序与"修好后转成 e2e 用例（+3）"写进了台账，避免后来者重复探索。
+ 本轮顺带纠了自己的一个期望值错误（`-100i8 * 40i8` 是 96 不是 -4000）——**用 LLVM 当 oracle**
  这个做法记进了方法里。

### Changed (2026-10-11) — v27 M5 补面之七：`map_type` 兜底加**标量**断言（一次被否掉的过界改动）

+ 上轮 `u128` 的根因是"兜底 `_ => PTR` 把未识别类型静默当指针"。本轮试图把兜底改成
  **白名单**（只有 `Ref/RawPtr/FnPtr/FnDef/Dynamic/Str` 走 PTR）——**全量 e2e 12+ 条红**
  （`nested_struct`/`box_value`/`vec_push`/`vec_string`/`closure_capture`/`drop_glue`/…）
  ⇒ 兜底是**load-bearing** 的：聚合/闭包/分配器形态确实靠它拿"指针形状"表示。
+ 精确的防线是**只钉标量**：兜底分支加 `debug_assert!`（标量落到这里直接炸；debug 构建
  就是 e2e，release 不受影响），并保留 `Int/Uint(I128)` 的显式拒绝。回退后 **122/122 绿**。
+ 教训记在账本 §8：审计要**先测再改**——"看起来更严格的兜底"可能正在承载别的语义；
  但结论比改动值钱：这一类（标量静默变指针）现在被断言永久钉住。

### Fixed (2026-10-11) — v27 M5 补面之六：`u128`/`i128` 从**静默错值**改为明确拒绝

+ **怎么发现的**：想给 128 位整数补用例（此前**零覆盖**），写完真跑 ⇒ 返回 1 而不是 42；
  按位掩码定位后单独取一半 ⇒ `(1u128 << 64) + 5` 的**低半是 6**（应为 5）。
+ **根因**：`map_type` **没有** `I128/U128` 分支 ⇒ 落到兜底被当成 **`PTR`（64 位）**
  ⇒ `1u128 << 64` 按 64 位移位的计数掩码变成 `1 << (64 & 63) = 1`，整条链静默错值
  （能编、能跑、答案错）。
+ **处置**：`map_type` 显式拒绝，缺口 ID **`WIDE_INT`**（台账决策 `待做`）；e2e 改负向
  `neg_u128`（`expect_compile_err` 匹配 `WIDE_INT`）⇒ 用例数 **122**。真支持需要
  「两寄存器对」+ 全套运算 + libcall（`__multi3`/`__udivti3`/…）+ ABI，属独立专项。
+ 附带踩坑：缺口台账的 **ID 字符集只允许大写字母与下划线**（含数字会被解析器跳过，表现成
  "台账没有行"）⇒ `I128` 改名 `WIDE_INT`。

### Added (2026-10-11) — v27 M5 补面之五：静对齐的**运行期**用例

+ `align` 取自 layout（`inner.align.bytes()`）⇒ `#[repr(align(N))]` 一直生效，但**没有任何用例**
  验证"符号地址真的对齐"。新增 e2e `static_align_64`：`#[repr(align(64))] static LATE` 的地址
  运行期检查 `% 64 == 0`，返回 42（否则 1）⇒ **exit=42**，用例数 **121**。
+ 同轮否掉一个假设：no_std 自带 `#[global_allocator]` + 我们的 alloc shim **不会**撞重复符号
  ——`vec_push`/`vec_string` 既有用例本来就是这样且全绿 ⇒ 不必改（实测优先于猜测）。

### Fixed (2026-10-11) — v27 M5 补面之四：`#[link_section]` 从**静默忽略**改为明确拒绝

+ 后端**从没读过** `codegen_fn_attrs(..).link_section` ⇒ 用户写 `#[link_section = ".CRT$XCU"]`
  之类会被**静默忽略**（照旧进 `.data`/`.rodata`）。"写了不生效"是最难查的一种错。
+ 现在 `backend.rs::reject_link_section`（**缺口 ID `LINK_SECTION`，全仓只一处**）明确拒绝，
  诊断点名**段名**并指向 M4-T4 的对象层"任意段"支持；静态与函数两个入口共用同一助手。
+ e2e `neg_link_section`（`#[unsafe(link_section = ".mydata")] static X` ⇒ 编译期拒绝）⇒ 用例数 **120**。
+ 顺带确认：`#[export_name]` **不需要**后端做什么（由 rustc 的符号名解析负责，既有用例已覆盖）。

### Added (2026-10-11) — v27 M5 补面之三：DWARF 调试段自动核对

+ "DWARF5（gdb 实证）"此前只是**手工**验过，改坏了没人拦。新增 `tests/dwarf_sections.rs`：
  直接读**对象文件的 COFF 段表**（文件头 + 段头 + 长名字符串表，不引第三方库），断言
  `-C debuginfo=2` 产出 `.debug_info/.debug_abbrev/.debug_line/.debug_line_str/.debug_aranges/`
  `.debug_frame`，而 `-C debuginfo=0` **一个都不产出**（负向对照，防止空断言）；另有
  "debuginfo=2 不改变程序语义"（仍返回 42）。
+ 实测纠了一个预期：没有 `.debug_str`，行号表用的是 DWARF5 的 **`.debug_line_str`**
  ⇒ 断言按实测集合写，不照抄 LLVM 段名。
+ 纳入门禁：新步骤 `rustc-dwarf` + `--subset rustc` ⇒ 子集 **15 步**。

### Added (2026-10-11) — v27 M5 补面之二：其它 codegen 旋钮

+ 与 opt-level 同一测试文件（`tests/opt_levels.rs`）：

  | 旋钮 | 结果 |
  | --- | --- |
  | `-C debuginfo=0/1/2` | 各档能编能跑，答案 42 ✔ |
  | `-C codegen-units=1/8`、`-Z threads=4` | 同上 ✔ |
  | `-C panic=unwind` | **干净拒绝** ✔ |

+ `panic=unwind` 的实测口径：**no_std** crate 被 **rustc 前端**先拒（"unwinding panics are not
  supported without std"），走不到后端的 `[cap:UNWIND]`——那条只对 **std** 场景有意义，而 std
  场景本地跑不了（M3-②）。测试因此断言"拒绝 + 诊断提到 unwind"，**不假装**后端那条路被测到了。

### Added (2026-10-11) — v27 M5 补面：`-C opt-level` 全档回归

+ **缺口**：e2e 的用例结构没有"rustc 旗标"这一维（`extra` 是**源码级**附加）⇒ 全部用例
  **只跑过 debug 档**，而"后端在 `-O` 各档也对"是完整后端的硬承诺。
+ 新增 `tests/opt_levels.rs`：同一个无 std 程序（循环 + 递归 + 乘法 + 窄化 + 溢出检查）在
  **`0/1/2/3/s/z` 六档**各编一次、链接、**真执行**，断言都返回 **42**；
  外加"同档编两次"的**确定性**断言（object 逐字节相等）。
+ 两个坑（都是断言选错对象）：① 拿 `.exe` 比逐字节 ⇒ 含链接器时间戳/PDB 路径，测的是**链接器**；
  ② `--emit=obj=<路径>` 在本版 rustc 不生效，且后端对象名固定为 `forge_codegen_output.o`
  ⇒ 用 `--out-dir` 分目录定位再比。
+ 纳入门禁：新步骤 `rustc-opt-levels`（`Scope::Windows`）+ `--subset rustc` ⇒ 子集 **14 步**。

### Changed (2026-10-11) — v27 M2.5/S3-sym：受**谱汇编器文法**限制，改为 fail-closed + 点名诊断

+ **实测过程**：把 `sym` 的符号名替换写进去后，汇编器报
  `内联汇编文本无法被汇编器解析："mov RAX, _RNv…seven"` ⇒ **替换是对的**，卡在
  **谱汇编器的操作数文法**（只接手寄存器与数值字面量）。
+ **关键区别**：`global_asm!` 能用符号，是因为它**不走本谱汇编器**（包成 `global_asm!`
  交给 **LLVM** 汇编）；inline `asm!` 走的是本谱汇编器 ⇒ 两者能力不同，别互相类推。
+ **处置**：按"不留未测代码"原则回退替换实现，改成 **fail-closed + 点名诊断**
  （说清"谱汇编器只吃寄存器/数值"、"`global_asm!` 为何能用"、"根治方向"），
  并用负向用例 `neg_inline_asm_sym` 钉住（`expect_compile_err` 匹配"操作数暂不可用"）。
  e2e 用例数 **119**。
+ **根治方向（ISA-DSL 的活）**：给汇编器加「符号立即数 + `reloc abs64`」的解析形态；
  谱侧已有 `MOVABS_GLOBAL`（`reloc = "abs64"`）承接重定位，缺的只是词法/文法这一层。

### Fixed (2026-10-11) — v27 M5 补漏：两个"子集看不见"的守卫

+ **`error_taxonomy`（forge-codegen）**：仓库规定后端 `Unsupported(` 必须是能力缺口
  （`IrError::capability`），畸形 IR ⇒ `Internal`、数据/使用错误 ⇒ `Semantic`。v27 S1–S4 在
  `pipeline/inline_asm.rs` 写了 **26 处**裸 `Unsupported(` ⇒ 一直红而子集看不见。按分类学重判：
  **`Internal` 7 处**（约束协议/搬运形态等我们自己产物的形状）、**`Semantic` 19 处**
  （汇编文本解析失败、显式寄存器不存在、无汇编器等）。不新增能力 ID（`INLINE_ASM` 的唯一失败点
  在 `forge-rustc` 侧）。
+ **`lint_shipped`/`op_gap`（forge-isa-dsl）**：新增的 `InlineAsm` op 谱里没有也不该有
  `[[lower.InlineAsm]]`（带文本/变长操作数，由宿主管线展开）⇒ 被算成"真缺口"。归入
  `HOST_PIPELINE_OPS`（与 `VaStart` 同类）：三谱 pin 的"宿主管线" 8 → 9、真缺口不变。
+ **两个守卫都加进 `--subset rustc`**（各 0.2s / 0.6s）⇒ 后端子集 **13 步**。
  教训：只跑后端子集会漏掉跨 crate 的契约守卫。
+ 方案账本 §2 新增**状态总表**（里程碑 → 状态 → 证据落点）并修掉 M3-② 的失效措辞
  （TLS 与内联 asm 两项阻塞已解除）。

### Added (2026-10-11) — forge-rustc v27 M2.5/S4 第二刀：内联 `asm!` 的 `options(noreturn)` 可用

+ `asm!("ud2", options(noreturn))` 从 **编译期拒绝**变成真 trap（e2e `inline_asm_noreturn`
  ⇒ `0xC000001D`），用例总数 **118**（`expect_trap` 3 条）。
+ 实现口径：`noreturn` **不改变汇编文本**、只改**落点**——没有 fallthrough（rustc 侧 `targets`
  为空）⇒ 当前块以 `unreachable()` 收尾（与"发散调用"同一个终结符写法）；CFG 可达性那一步本就连
  `NORETURN` 不排后继，两处口径一致。
+ 口径提醒（本轮踩过）：trap 用例的 `expected` 必须写**非零 NTSTATUS**（`-1073741795`），写 0
  会被判 "want 0" 红。

### Changed (2026-10-11) — v27 M4-T4：`WA-01` 按设计关闭（vtable 落 `.data` 不是技术债）

+ **证据链**：① `add_data_with_relocs` 的文档里有**实测**——"MSVC 链接器对 `.rodata` 段的
  ADDR64 重定位不应用（链接后全 0），`.data` 段正常"；② `forge-object` 的 `add_rodata`
  **故意没有** relocs 变体 ⇒ "只读段 + 重定位"这种链接后必坏的形态**不可表达**；
  ③ 所以 vtable 指针表落 `.data` 是**给定工具链下的正确实现**。
+ 处置：`WA-01` 判**已关闭（按设计）**；`add_rodata` 的文档把不变量写清（后来者不必再试一次）；
  `WORKAROUNDS.md` 的"根治方向"不再写"需主库兼容"（那是**无法根治**的外部行为）。
+ **残余代价如实记录**：指针表运行期**可写** ⇒ 丢掉只读页保护（W^X 层面的轻微退化）；
  恢复只读语义要换链接器口径或做加载时重定位，都超出本后端的可控范围。

### Added (2026-10-11) — cargo-forge：`init --std`（std 探测从"手工改配置"变成一条命令）

+ `cargo forge init --std` 直接生成 **std** 口径的 `.cargo/config.toml`
  （`build-std = ["std", "panic_abort"]`），与 `--alloc` **互斥**（两种不同的 build-std 口径）。
  此前 M3-② 的探测要"先 init（no_std 口径）再手工改 build-std"，容易漏一步。
+ **不**加 `-Zshare-generics=yes`：本后端明确拒绝它（`[cap:SHARE_GENERICS]`，`WA-04`）。
+ 实测：探针工程跑 `init --std` ⇒ 配置正确 ✔；`--std --alloc` ⇒ clap 报互斥 ✔；
  `examples/forge-rustc-std-hello/README.md` 的跑法同步改成一条命令。

### Added (2026-10-11) — v27：确定性测试进门禁（多 CGU/并行/增量）

+ **`rustc-determinism` 步骤**（`Scope::Windows`）：`cargo test -p forge-rustc --test determinism`
  ——多 CGU 逐字节相等、并行与串行全等、`-C incremental`。此前它**既不在全量门禁也不在后端子集**
  （只有 CI 作业跑），而这是"完整后端"的**对外承诺**（`WA-38`/`WA-39`）⇒ 本地可复现性也该被门禁覆盖。
  实测 3 个测试 16s，够便宜。
+ `--subset rustc` 现 **11 步**（含 e2e、e2e 不变量、确定性、三个后端守卫、ABI 真机核对、文档）。
+ 方案账本 §8 同时记下 **`label` 操作数的结构性阻塞分析**：标签必须落在指令流的某个**位置**，
  而当前模型是"文本 → 一次汇编 → 一串机器指令"、发射按块线性拼接 ⇒ 中间标签表达不了；
  可行解法（汇编器分段 / 把 asm 拆成多段 IR op）都要动结构，属 S4 的独立设计项。

### Added (2026-10-11) — v27 M4-T2：约定按目标选（**实测已成立**，新增双向守卫）

+ 实测结论：同一个**抽象** `CallConvId::Builtin(C)` 在 aarch64 机器上 plan 出 **`aapcs64`**、
  在 x86_64 机器上是 **`win64`** ⇒ v20 的"`c` 由规则 `aliases` + 该机绑定共同代答"在设计层面
  已经工作，后端**没有**把 win64 写死。**T2 无需改动**。
+ 新守卫 `abi_target_real::abstract_c_on_arm64_resolves_to_aapcs64`：同一个探针跑两台真机、
  两条断言互为对照（只断言 aapcs64 的话，"偶然默认"也能过）。
+ `abi_target_real`（26 个 ABI 用例，0.01s）纳入 `cargo forge gate --subset rustc` 白名单。
+ T2 剩下的部分不在约定层而在目标相关的**对象层/链接层**（`.debug_frame` 的 CFI、PE 异常表、
  `global_asm` 的汇编器参数、链接器与入口）⇒ 已并入方案里的 T3。

### Fixed (2026-10-11) — forge-rustc v27 M4-T1：未知目标不再静默回落 x86_64

+ `isa_name_for_target` 原来在 `else` 里返回 `"x86_64"` ⇒ 交叉编译到 riscv64 之类会**静默编出
  x86 机器码**。现在返回 `Option<&'static str>`，`codegen_crate` 入口对 `None` 报
  **`[cap:TARGET]`** 并直接返回空模块；目标校验**只有一个失败点**（缺口守卫要求 ID 恰好一处），
  `compile_fn_task` 里的推导写成不变量（`expect`）。
+ 台账新增 `TARGET` 行（决策 `不做`）；方案账本 §8 记 M4 施工图（实测：18 处 `x86_64|win64`
  命中里**真正写死的只有 4 处**，ISA 与约定层已就绪）与 T1 落地。
+ 诚实边界：本仓沙箱**无法端到端验证**交叉目标（只有宿主 target 的 core）⇒ 验收靠单点+台账+
  守卫+审查；T2（约定）/T3（对象层/SEH）的验证条件也记在方案里。

### Added (2026-10-11) — forge-rustc v27 M2.5/S4 首刀：内联 `asm!` 的 `clobber_abi` 可用

+ `asm!("nop", clobber_abi("C"))` 从 **fail-closed** 变成可用（e2e `inline_asm_clobber_abi` ⇒ 42）。
+ **发现**：这个 nightly 的 MIR `TerminatorKind::InlineAsm` **没有 `clobber_abi` 字段**——
  它在 MIR 构建期就被展开成一批 `Out { reg, place: None }` 操作数。之前我们把它当"输出寄存器"
  去解析（要 GPR 类）⇒ 撞上 `xmm0`/`k0` 直接拒绝（不静默错，但真实代码编不过）。
+ **落地**：`Out`/`InOut` 的 `place: None` 视为**纯 clobber 槽**（约束 `c,<suffix>`）：不产物值、
  不搬数据，只把寄存器记进 `inst_clobbers`；纯 clobber 的显式名**跨类解析**
  （新增 `CompileState::reg_classes` 快照），解析不到（本 ISA 没有的寄存器，如 AVX-512 mask
  `k0`）**跳过无害**——我们从不在那些寄存器里放值。
+ 两处踩坑：① 去重必须按 **(编号, 类)**（不同类的 `to_index` 会撞车，`rax`/`xmm0` 都可能为 0）；
  ② 手工验证一度"挂起"，实为 shell 里 `Start-Job` 的 toolchain 解析问题，换到 e2e 夹具
  （自带超时与正确旗标）后 **0.77s 通过**——别把工具链问题误判成后端 bug。
+ 验收：`cargo forge gate --subset rustc` **9 步全绿（217.8s）**；e2e **117/117**。

### Fixed (2026-10-11) — forge-rustc v27 M4-② 首刀：`-Zshare-generics=yes` 编译期明确拒绝

+ 用户显式开 `-Zshare-generics=yes` 时，过去会在**链接期**缺符号（LNK2019，`WA-04`）——
  错误离现场很远。现在 `codegen_crate` 开头就判 `share_generics == Some(true)`并给出
  **`[cap:SHARE_GENERICS]`** 的编译期诊断（实测：`error: Unsupported: [cap:SHARE_GENERICS] …`）。
+ **只拒"用户显式指定"**：该选项默认 `None`，由 rustc 按优化级别解析（debug/`-O0` 下默认 true）
  ⇒ 默认路径与既有用例**完全不受影响**（不带 flag 时正常产出）。
+ 台账：新增 `SHARE_GENERICS` 行（决策 `不做`）；`WORKAROUNDS.md` 的 `WA-04` 改判**已关闭**。
  根治（补辅助实例）仍归 M4——需要对象层的"实例导出/引用"模型。
+ 验收：`cargo forge gate --subset rustc` 全绿（含 `capability_gaps` 对新增 ID 的双向校验）。

### Fixed (2026-10-11) — v27 M2.5/S2b：传统寄存器**短名映射**（`out("rax")` 现在可用）

+ rustc 对 x86 传统寄存器的**规范名是短名**（`out("rax")` 的 `InlineAsmReg::name()` 给 `ax`），
  与 x86 谱的 `RAX` 不同名 ⇒ 上一轮只能 `out("r8")`。现在 `[reg.gpr64]` 用
  `aliases = { ax = 0, cx = 1, … }` 补上映射（上一轮没生效是**插入位置**问题：落在 `names`
  数组范围内侧；正确位置是该表的顶层键）。
+ 生成的 `reg_by_name` 改为**直接扫"主名 + 别名"两张表**（`eq_ignore_ascii_case`、类必须一致），
  不再走生成的 `FromStr`——跨组同名时它的先匹配规则不可靠（`ax` 会落到 16 位组）。
  别名对与主名对同源（都在 `gen_reg_info` 里由 `group_names` + `aliases` 生成，不新增数据）。
+ 守卫 `reg_name_table` 新增断言：短名 `ax`/`AX` 必须解析到 `RAX`；并记下"两个寄存器的
  `to_index` 恰好都是 0"（跨组冲突的由来，别再拿编号区分类）。
+ e2e `inline_asm_named_reg` 改回最自然的写法 `asm!("xor rax, rax", out("rax") x)` ⇒ **exit=0**
  （若显式名被忽略，x 会留在 5 ⇒ 必红）。
+ 验收：`cargo forge gate --subset rustc` **9 步全绿（233.5s）**；e2e 116/116。

### Added (2026-10-11) — forge-rustc v27 M2.5/S2b：内联 `asm!` 的**显式寄存器名**

+ `asm!("xor r8, r8", out("r8") x)` 现在能用：x 从 5 变 **0**（强断言——若实现忽略显式名而改挑
  别的寄存器，x 会留在 5 ⇒ 必红）。e2e `inline_asm_named_reg`，用例总数 **116**。
+ 机制：约束后缀从 `g`（池）扩展成"`g` 或显式拼写"；后端经新的
  **`TargetRegInfo::reg_by_name(name, class)`** 反查编号（生成器用 `FromStr` + `PhysReg::class`
  回验，与汇编器同源）；同一寄存器被重复使用会被拒绝。
+ 实测的两条 rustc 前端规则：① 模板里**不能**用 `{0}` 引用显式寄存器操作数
  （"explicit register arguments cannot be used in the asm template"）⇒ 文本里直接写寄存器名；
  ② `InlineAsmReg::name()` 是 rustc 的**规范短名**——x86 传统寄存器是 `ax`/`cx`/…（不是
  `rax`/`rcx`），与 x86 谱的 `RAX`/`RCX` 不同名 ⇒ **传统寄存器的显式名暂不可用**，
  `r8`/`r9`… 两侧一致 ⇒ 已可用。给谱加 `aliases` 试过但没进生成物（已回退、不留死数据），
  记为待办。
+ 验收：`cargo forge gate --subset rustc` **9 步全绿（146.2s）**；e2e 116/116、零 `KNOWN`。

### Added (2026-10-11) — forge-rustc v27 M2.5/S3 前半：内联 `asm!` 的 `const` 占位符

+ `asm!("add {0}, {1}", inout(reg) x, const 7)` 现在真的能用：`const` 占位符在 **rustc 侧
  就地换成字面文本**（只有寄存器占位符进入约束表），后端无需改动、`const` 也不需要寄存器。
+ **无冗余**：常量格式化抽成 `global_asm::scalar_text`（纯函数）+ `const_text`（吃 `DefId`），
  `global_asm!` 的 HIR 路径与内联 `asm!` 的 MIR 路径**共用同一处**——两侧的
  `InlineAsmOperand` 是不同类型（`rustc_hir` / `rustc_middle::mir`），格式化内核不复制。
+ e2e：`inline_asm_const`（`x = 35`、`add {0}, {1}` + `const 7` ⇒ **exit=42**），用例总数 **115**；
  README 机读计数同步。
+ 边界：`sym` 仍是 fail-closed（MIR 侧 `SymFn { value }` 是另一种形态，取符号名要另做一层，
  属 S3 的下一半）；显式寄存器名（S2b）、`label`/`clobber_abi`（S4）同理。

### Added (2026-10-11) — forge-rustc v27 M2.5/S2a：内联 `asm!` 的**寄存器约束端到端可用**

+ `asm!("add {0}, {1}", inout(reg) x, in(reg) y)` 现在**真的能用**：挑物理寄存器（可分配 GPR
  顺序、互不重复）→ 占位符换成寄存器名 → 搬入/搬出 `mov`（一侧写死、另一侧绑 vreg）→
  `asm` 本体 → 相关指令带 `inst_clobbers` → 结果经 `store_place` 写回 IR 值。
+ **上一轮的字段问题解法**：搬运指令的操作数字段**按角色挑**而不写死 0/1
  ——`InstOperand` 的契约是"Use 在前、Def 在后"且模板写死的字段 `settable == false`，
  于是 `role == Def && settable` 唯一确定 dst、`role == Use && settable && field != dst`
  唯一确定 src。误绑时管线自己的 v23 M4 一致性检查会当场拒绝（这次正是它抓到的）。
+ e2e：`inline_asm_reg_inout`（`x = 5`、`y = 37`、`add` ⇒ **exit=42**；不带 asm 时是 5），
  用例总数 **114**；README 机读计数同步。
+ 边界：只接 **64 位整数**与 `in`/`out`/`inout` + `reg` 类；显式寄存器名（S2b）、
  `const`/`sym`（S3）、`label`/`clobber_abi`（S4）仍 fail-closed（带 `[cap:INLINE_ASM]`）。

### Added (2026-10-11) — forge-ir v27 M2.5/S2a：`results` 变参 + 两侧接线（接线暂挂）

+ **`ops.toml` 的 `results` 支持 `"variadic"`**（`build.rs` 新增 `ResArity::{Fixed, Variadic}` +
  `Display`；生成物的 `result_count: u8` 保持原样，变参记 0——它只是"声明形状"，不参与校验）。
  `InlineAsm` 因此是 `operands = "variadic"` + `results = "variadic"`。
+ **`builder::inline_asm_with(constraints, text, in_operands, out_tys) -> Vec<Value>`**：
  输入走操作数、输出作为结果（本后端局部变量在栈槽 ⇒ 写回必须经 `store_place(place, value)`，
  所以输出必须是 IR 值）。
+ `pipeline/inline_asm.rs` 的 S2a 改用**操作数/结果双游标**；`forge-rustc` 侧完成模板重建
  （`Placeholder{operand_idx}` → `{idx}`）、`In`/`Out`/`InOut` 收集与结果写回。
+ **接线暂挂（诚实记录）**：跑起来后管线的一致性检查当场拒绝——**搬运指令的操作数字段索引
  取决于汇编器为 `mov rax, rax` 选中的编码形态**，按 `(0=dst, 1=src)` 硬绑定是错的
  （`第 22 条指令的字段 0 被登记为 Def 但操作数表里没有这一项`）。⇒ 下一轮改成**按角色挑
  字段**（或加一个"造 bound 操作数的 mov"的辅助构造器）。当前行为与 S1 一致、e2e 113/113。
+ 两处实测细节已写进注释：MIR 侧的 `InlineAsmRegOrRegClass` 来自 **`rustc_target`**
  （与 `rustc_ast` 同名不同类型）；`Out`/`InOut` 的 `place` 是 **`Option<Place>`**。
+ 验收：`cargo forge gate --subset rustc` **9 步全绿**（252.3s）。

### Added (2026-10-11) — forge-ir/forge-codegen v27 M2.5/S2a 基础设施（内联汇编的寄存器约束）

+ **`Opcode::InlineAsm` 的操作数改成变参**（`ops.toml: operands = "variadic"`，`results` 仍 0），
  立即数约定为 `[0] = 约束描述`、`[1] = 汇编文本`；`builder::inline_asm_with(constraints, text,
  operands)` 是新的入口（`inline_asm(text)` 保留为 S1 纯文本形态）。
+ **`CompileState` 新增两个 `'static` 快照**：`reg_name_of`（寄存器编号+类 → 汇编拼写，来自
  `Arc::clone(machine.reg_info())`——借 `&M` 活不过构造函数）与 `gp_alloc_order`
  （可分配 GPR 顺序，内联 asm 挑约束寄存器用）。
+ **降级期的 S2a 全流程**（`pipeline/inline_asm.rs`，已编译通过但**尚未接线**）：
  解析约束 → 挑寄存器 → 名字替换 → 占位符替换（`{}`/`{n}`，其它修饰明确拒绝）→ 汇编 →
  `in`/`inout` 搬入（dst 侧写死、src 侧绑 `OperandRole::Use`）→ asm 本体 → `out`/`inout` 搬出
  （dst 侧绑 `Def`）→ 每条相关指令带 `inst_clobbers`。
+ **两处待接线**（下一轮）：① `ops.toml` 的 `results` 需要变参（`out` 值必须作为 IR 结果——
  实测本后端局部变量在栈槽，写回要 `store_place(place, v)`，而它要 IR `Value`）；
  ② rustc 侧把 `InlineAsmOperand::{In,Out,InOut}` 变成约束+操作数并重建 `{n}` 文本。
+ 验收：`cargo forge gate --subset rustc` **9 步全绿**（行为未变：还没有东西发约束，
  e2e 仍 113/113）。

### Added (2026-10-11) — forge-rustc v27 M2.5/S2 前置：寄存器名表（`TargetRegInfo::reg_name`）

+ **`TargetRegInfo::reg_name(reg, class) -> Option<&'static str>`**：寄存器编号 + 寄存器类 →
  **汇编文本里的拼写**。生成器（`forge-isa-dsl`）从**与 `FromStr` / `From<Reg> for &'static str`
  同源**的 `group_names` 发出反向表（不为第二个名字表维护两份数据）；缺省 `None`
  （非 DSL 后端/测试替身），调用方按"拿不到就 fail-closed"处理。
+ **接口必须带 `RegClass`**——这是新守卫当场抓到的真缺陷：`PhysReg::to_index` 在"多宽度
  视图"ISA 上**跨组冲突**（x86 的 `AL` 与 `RAX` 同为编号 0），第一版只按编号查会返回 `"AL"`。
  反向表因此按 `(编号, 类)` 匹配。
+ **新守卫 `reg_name_table`**（`crates/backend/forge-codegen/tests/reg_name_table.rs`）：
  对 x86 的 RAX/RCX/RDX 做"名字 → `add {name}, {name}` → 汇编器解析成功"的往返，
  同时钉住"拿得到名字"与"汇编器认这个名字"两件事；已登记进 `GUARDS` 与 `--subset rustc`
  （后者现为 **9 步**）。
+ 验收：`cargo forge gate --subset rustc` ⇒ **9 步全绿，174.7s**（含全量 e2e 113/113、
  规模棘轮、三个既有守卫与文档 lint）；工作区 `cargo check --workspace --exclude forge-rustc`
  全绿（生成器改动会重生成三份 ISA 模块）。

### Fixed (2026-10-11) — forge-rustc v27：规模棘轮回退修复 + 后端管线契约回写

+ **修回退**：v27 M2.5/S1 把 `pipeline/lowering.rs` 撑到 871 行、超过 `file_size_ratchet`
  的上限 835（上一轮只跑了 `--subset rustc`，该子集不含这条守卫 ⇒ 验收不完整）。按纪律
  **切文件而不是抬上限**：内联汇编展开体抽成 `pipeline/inline_asm.rs`
  （`CompileState::expand_inline_asm`），并压缩两处冗长注释 ⇒ 836 行，棘轮恢复绿。
+ **`file_size_ratchet` 进入 `--subset rustc` 白名单**——它是"改后端"最该跑到的守卫，
  上一轮正是漏了它。
+ **契约回写**：`docs/reference/backend-pipeline.md` 新增 §13「v27 增量」——内联汇编
  （IR 形态 / 降级期展开 / 文本来源 / fail-closed / S1 边界）、运行时注入
  （alloc 按需注入、动态 TLS 的隐藏符号与索引约定）、`abort()` 的出口；原 §13 顺延为 §14。
+ 验收：全量 e2e **113/113**、零 `KNOWN`；`file_size_ratchet` 绿；文档 0 issues。

### Added (2026-10-11) — forge-rustc v27 M2.5/S1：无操作数的内联 `asm!` 真实现

+ **内联 `asm!` 从"明确拒绝"推进到真实现**（S1 口径：模板无 `{}` 占位符、非 `options(noreturn)`）。
  链路：`TerminatorKind::InlineAsm` → IR `Opcode::InlineAsm`（文本走 `Immediate::String`）→
  降级期用**本 ISA 的汇编器**（`TargetAssembler::parse_insts`）解析 → 机器指令就地插进函数体
  （`inst_ops`/`inst_clobbers` 平行推进）。**不需要**给生成的 `Inst` 加"raw 字节"变体。
+ 管线接线：`CompileState` 新增 `asm_parse` 闭包（由 `machine.assembler()` 传入，
  `CompileState::new` 的 where 子句补 `M: TargetMachine<Inst = I>`）；文本解析**复用已有的**
  `ctx.type_store`（不需要新字段——施工图里"要穿两样东西"被推翻一半）。
+ 仍需 fail-closed（`[cap:INLINE_ASM]`）：寄存器/`const`/`sym` 约束、标签、
  `options(noreturn)`、非本 ISA 文本（S2–S4）。
+ e2e：`inline_asm_nop`（**exit=42**：编过、跑通、后续代码照常）与 `inline_asm_ud2`
  （**exit=-1073741795** = NTSTATUS `#UD`：证明插进去的字节**真的被 CPU 执行**）；
  用例总数 **113**（README 机读计数标记同步，守卫钉住）。
+ 施工时发现的两处坑（已写进代码注释）：`forge-opt` 的 `const_fold` 对 `Opcode` 是**穷尽
  match**（新 op 必须显式加一条，`InlineAsm => Ok(None)`）；本 nightly 的
  `InlineAsm.template` 用 `.iter()` 直接迭代（`.as_ref()` 会撞 `AsRef` 歧义）。
+ 验收：全量 e2e **113/113**、零 `KNOWN`；三个守卫绿；`cargo forge gate --subset rustc` 7 步全绿。

### Added (2026-10-11) — forge-rustc v27 M5-②：`cargo forge gate --subset rustc`（后端一条命令）

+ **`--subset rustc`**：只跑后端相关的 **7 步**（`fmt` / `rustc-e2e` / `rustc-e2e-guards` /
  `capability_gaps` / `rustc_doc_counts` / `workarounds_guard` / `docs`），实测 **133.6s 全绿**。
  全量门禁 44 步里后端只占 2 步，改后端的人此前要么等全量、要么手工挑命令（容易漏守卫）。
  实现用**显式白名单**（`RUSTC_STEPS`）而不是给 `Scope` 加变体——后端步骤横跨 fmt/守卫/e2e
  三类，白名单本身就是这个子集的文档。
+ `rustc-e2e` 步显式 `--skip e2e_cargo_template_workflow`：该用例要 rustup 下载固定的 dated
  通道（本机/沙箱里失败），由 CI 作业覆盖——门禁跑"能跑的"，不装作跑过。
+ 修三处**存量漂移**（都是"写死会变的数字/状态"）：门禁步描述里的"110 用例"、`CLAUDE.md` 的
  "28 条守卫"/"110 用例"、`CLAUDE.md` 里 TLS 仍写"待做"（实际已实现动态 TLS）。改法统一为
  **去掉具体数字 / 指向单一事实源**。
+ 方案账本 §8 记入 **S1 施工图**（二轮侦察结论）：不需要给生成的 `Inst` 加 raw 变体；真正的
  阻塞是"汇编器与字符串表都拿不到"——5 点施工清单与验收用例（`inline_asm_nop` /
  `inline_asm_ud2`）已落纸。

### Added (2026-10-11) — forge-rustc v27 M3-②：`std` 探测工程（一条命令取结论）

+ 新增 [`examples/forge-rustc-std-hello/`](examples/forge-rustc-std-hello/README.md)：
  用 forge-rustc 当后端、`std` 由 **LLVM 预编译**（`-Zbuild-std=std,panic_abort` +
  `tools/forge-rustc-wrapper` 按 crate 名剥掉系统 crate 的 `-Zcodegen-backend`）编译的
  最小 `println!` 程序。命令、前置（`rust-src`、dll、wrapper）与"已排除的前置阻塞"
  （动态 TLS、`abort()`、alloc 按需注入）都写在它自己的 README 里。
+ **本仓 sandbox 内跑不通（环境限制，非后端缺陷）**：`-Zbuild-std=std` 需要从 registry 解析
  `std` 的构建依赖（`cfg-if`/`getopts`），而 sandbox 无网络凭证（实测
  `SSL connect error … SEC_E_NO_CREDENTIALS`）。把 `CARGO_HOME` 重定向进 workspace 能绕过
  "`~/.cargo` 不可写"，但绕不过没有网络 ⇒ 结论须在沙箱外/CI 取（命令已固化）。
+ 方案账本 §8 记入 M3-② 的现状与下一步验收口径（跑通 `hello from std`；
  失败时应以**明确的 `[cap:…]` 或编译错误**暴露缺口，而后逐条落成实现项或缺口 ID）。
+ 验收：文档 0 issues、`rustc_doc_counts` 守卫绿（用例数未变）。

### Added (2026-10-11) — forge-rustc v27：`#[thread_local]` 真实现（动态 TLS）

+ **线程局部静态真的能用**（此前是 `[cap:TLS]` fail-closed，更早则会把 `#[thread_local]`
  发成普通 `.data`、**线程私有语义静默丢失**）。选**动态 TLS**：静态模型（`.tls$` + `gs:[58]`）
  需要 IR 支持**段前缀**（`MemRef` 只有 base/disp/index/scale），而 `TlsAlloc`/`TlsGetValue`/
  `TlsSetValue`（kernel32 已链）**不需要新的 IR 能力**，代价是每次访问一次函数调用。
  新增 `src/tls_runtime.rs`：每个 TLS 静态发两个隐藏符号（`__forge_tls_init_<sym>` 初值模板 +
  `__forge_tls_slot_<sym>` 索引槽），运行时 `__forge_tls_addr` 用「`TlsAlloc` 索引 + 1」惰性
  初始化（首访为当前线程分配 + 拷贝初值 + `TlsSetValue`）；`TlsAlloc` 失败 ⇒ `ExitProcess(3)`
  （不做半吊子降级）。地址物化统一到 `lower::const_data_addr` 与 **`Rvalue::ThreadLocalRef`**。
  `FuncRefTable` 新增 `intern_global_named`（隐藏符号不是 `AllocId`）。
  e2e：`thread_local_roundtrip`（零初值写 7 + 非零初值 5→7 ⇒ **exit=14**）；
  负向用例 `neg_thread_local` 随之删除。
+ **实测踩坑（写进注释）**：① `create_block_with_params` **会切换当前块**（doc 却写"不切换"）
  ⇒ 运行时函数入口块丢终结符，处置是建完块后显式切回入口块；② TLS 的地址走的是**专门的**
  MIR rvalue（`ThreadLocalRef`），只改 `const_data_addr` 会报 `unsupported rvalue`；
  ③ 缺口守卫是**文本扫描**——注释里原样写构造器调用形态会被算成第二个站点。
+ 契约：`backend-gaps.md` 的 `TLS` 行改判**已实现**（边界写明：每静态独占一个索引、无析构、
  索引耗尽即终止；静态 `.tls$` 模型仍需 IR 段前缀，未做）；方案账本 §8 新增"TLS 真实现"节。
+ 验收：全量 e2e **111/111**、零 `KNOWN`；三个守卫绿；文档 0 issues。

### Added (2026-10-11) — forge-rustc v27：`abort()` intrinsics 落地 + TLS 明确决策 + 契约回写

+ **`core::intrinsics::abort()` 落地**（`lower/intrinsics.rs`）：此前落到 `unsupported intrinsic`
  ⇒ 任何走 `abort` 路径的真实 crate（`std` 的 `abort_internal`、部分 `panic=abort` 收尾）都编不过。
  实现发 kernel32 `ExitProcess(3)`（MSVC `abort()` 的退出码；Rust 对 `abort()` 的退出码**未规定**）；
  其后的 `unreachable()` 由调用点（`Call` 终结符的 `target: None` 分支）发——后端没有内联 `asm!`
  （`[cap:INLINE_ASM]`），FFI 终止进程是当前唯一正确出口。
  e2e：`intrinsic_abort` ⇒ **`PASS intrinsic_abort exit=3`**；用例总数 **111**
  （README 机读计数标记同步，由 `rustc_doc_counts_guard` 强制）。
+ **TLS 从"待做"改为"明确 `不做`（附条件）"**：两条路线都已量到代价——**静态路**（`.tls$` +
  `gs:[58]`）需要 IR 先支持**段前缀**（`MemRef` 现只有 base/disp/index/scale）；**动态路**
  （`TlsAlloc`/`TlsGetValue` + 首触初始化 + 每静态两个隐藏全局 + **4 个静态地址物化调用点**）
  约两轮，且半成品会"能编过但语义可疑" ⇒ 保持 `[cap:TLS]` fail-closed。**条件**：推进 `std`
  （M3-②）时必须先做动态路，清单已写进方案 §8。台账行同步改判。
+ **契约回写**：`crates/tools/forge-rustc/README.md` 路线图结尾不再写"无剩余开放项"
  （新增「v27 能力与缺口面」节：`global_asm!`、trap 语义、四个缺口 ID 决策表、门禁两步+两守卫、
  M2.5→TLS→M3 顺序依赖）；`CLAUDE.md` 新增「forge-rustc 现状速查」；`WORKAROUNDS.md` 的
  `WA-02` 改判已关闭、补回悬空的 `WA-26`/`WA-28`；`M3-①` alloc 运行时改**按需注入**
  （链 `std` 时不再注入手写 shim，`WA-02` 的正解）。
+ 验收：全量 e2e **111/111**、零 `KNOWN`；三个守卫（`capability_gaps` / `rustc_doc_counts` /
  `workarounds_guard`）绿；文档 0 issues。

### Added (2026-10-11) — forge-rustc v27 M5：门禁与守卫收口（后端 e2e 进门禁 + 两处文档漂移被钉住）

+ **门禁覆盖后端真值**（治 E9）：`cargo forge gate` 新增两步（`Scope::Windows`）——
  `rustc-e2e`（110 用例：编译→链接→**真执行**，`--test-threads=1`）与 `rustc-e2e-guards`
  （trap 语义 / 零容忍名单 / 模板契约不变量）。此前门禁对 rustc 只有 `check -p forge-rustc`，
  这些用例只活在 CI 作业里 ⇒ 本地"全套门禁通过"并不覆盖后端。门禁现共 **44 步**。
+ **新守卫 `rustc_doc_counts_guard`**（治 E4：文档与代码分叉）：README 的「支持矩阵」写
  「103 用例 + 5 个 `FLAKY`」，而代码实测 **110 用例 / `known_failure: true` = 0 / `FLAKY` 空**
  （那 5 例已随 `WA-42` 于 2026-09-12 全部转正）。现在 README 带一段**机读计数标记**
  （`cases=… hard=… flaky=… known=… expect_trap=…`），守卫与 `tests/e2e.rs` **逐项双向比对**；
  README 缺标记也红。README 的陈旧段落与 `Vec/String` 行一并改正。
+ **新守卫 `workarounds_guard`**：`[WA-NN]` 编号唯一（口径 = **裸编号行唯一**，允许
  `WA-33 增补（M2）` 这类共用基础编号的续记）+ 代码里的引用都能在表里找到（反向只报告不失败）。
+ **守卫首次运行抓到两处真漂移**：① `WA-26`/`WA-28` 是**悬空引用**——代码注释
  （`lower/mod.rs`、`lower/rvalue.rs`）引用这两个编号，表里却没有行（某轮清理时删了行、注释未跟上）
  ⇒ 补回两行并标明「已并入」（结论现由 `WA-44`/`WA-42` 承载）；② `WA-33` 有三行（原行 +
  `增补（M2）`/`（M10）`）⇒ 守卫口径据此修正。
+ 验收：`cargo forge gate` **44 步全绿**；两个新守卫自证
  （`rustc_doc_counts_guard` → `{cases: 110, hard: 110, flaky: 0, known: 0, expect_trap: 1}`；
  `workarounds_guard` → 43 编号、唯一、引用全可解析）；文档 0 issues。

### Added (2026-10-11) — forge-rustc v27 M2：缺能力闭环（`global_asm!` 落地 · 三项明确决策 · 守卫扩面）

+ **`global_asm!` 落地**（`crates/tools/forge-rustc/src/global_asm.rs` 新增；`WA-03` 改判已关闭）。
  设计照 cranelift 后端（`global_asm.rs`）的**可移植**那条路：把 crate 里所有 `global_asm!` 的模板
  拼成一份文本，另起一个 `rustc` 子进程（默认后端 = LLVM，`--emit obj` + `#![no_core]` 包装）
  汇编成**独立对象**，再由 `CompiledModule::global_asm_object` 交给 rustc 链接（rustc 的
  `link.rs` 会把它加进链接命令）。⇒ 指令/标签/**伪指令全支持**（就是 LLVM 的 asm parser）。
  支持 `const` 占位符（按常量类型的符号性格式化）与 `sym` 占位符（复用 crate 内同一份
  `symbol_name_for_instance`）；寄存器操作数/`label` fail-closed 并点名 `[cap:GLOBAL_ASM_OPERAND]`。
  子进程**必须清 `RUSTFLAGS`/`CARGO_ENCODED_RUSTFLAGS`**（否则会再次加载本后端 ⇒ 递归）。
  e2e：`global_asm_call`（exit=42）、`global_asm_sym_const`（exit=42，`sym` 写错名字会在链接期报错）；
  **删除已过时的负向用例 `neg_global_asm`**（它断言"global_asm 必须编译失败"，被本项正当推翻）。
+ **内联 `asm!` 从"静默错值"改为 fail-closed**：此前 `TerminatorKind::InlineAsm` 落进终结符 match
  的兜底 `_ => unreachable()` —— 实测 `asm!("mov {0}, 7", out(reg) x)` **编译无声通过**、运行退出
  `0xC000001D`。现在 ⇒ `[cap:INLINE_ASM]`（诊断带模板首片）+ e2e 负向用例 `neg_inline_asm`；
  真实现另立 **M2.5**（设计：IR 层「原始文本 + 操作数约束」节点 + 发射期汇编，S1–S4 分期）。
+ **MIR 兜底不再静默**：`_ => unreachable()` ⇒ `[cap:MIR_TERMINATOR]` 并点名终结符 kind。
  静默 `unreachable` 会把「没实现」伪装成「程序自己不可达」（内联 asm 就被藏了很久）。
+ **`panic=unwind` 明确 `不做`**（`[cap:UNWIND]` + 台账行 + e2e `neg_panic_unwind`）。原来只是一个
  裸字符串（不在台账、无守卫覆盖）。用例自带哑 `#[lang = "eh_personality"]`——`no_std` +
  `panic=unwind` 下 rustc 会**先**要求该 lang item，否则走不到后端检查。
+ **TLS 先 fail-closed**（本轮新发现的静默错值）：`#[thread_local] static` 会走普通 static 路径
  ⇒ 发射成 `.data`、**线程私有语义静默丢失**。现在 ⇒ `[cap:TLS]` + 台账行（决策 `待做`，
  判据 = 双线程读写互不干扰的真执行用例）+ e2e `neg_thread_local`。
+ **守卫扩面**：`capability_gaps_guard` 的扫描面加 `crates/tools/forge-rustc/src` ⇒ 后端侧 4 个
  缺口 ID（`INLINE_ASM`/`UNWIND`/`TLS`/`MIR_TERMINATOR`）与台账**双向相等**。
+ **连带变更（M1 ↔ M2 的真实冲突）**：M2 的 fail-closed 让 M1 的 panic 出口 `asm!("ud2")` 编不过
  ⇒ 改为 FFI `ExitProcess(101)`（`#[link_name]` 别名，避免与既有 `ffi_exit_process` 撞名——该撞名是
  全量套件实测撞出来的）；`expect_trap` 的期望码随之变为 **101**（确定性 panic abort），
  M1 的守卫同步改判（模板必须确定性 abort、且不许出现内联 asm）。
+ 验收：`cargo forge gate` **40 步全绿**（204 s）；全量 e2e **110/110、零 `KNOWN`**、
  `known_failure: true` = 0；`capability_gaps` + `e2e_trap_semantics_invariants` 守卫绿；文档 0 issues。

### Added (2026-10-11) — forge-rustc v27 M1：trap 语义 + 零容忍名单守卫（含台账纠偏）

+ **e2e trap 语义**（`crates/tools/forge-rustc/tests/e2e.rs`，破坏性）：模板的 `#[panic_handler]`
  从 `loop {}` 改成 `unsafe { core::arch::asm!("ud2", options(noreturn)) }`；`Case` 新增
  **`expect_trap`**；退出结局分成**正常退出 / 异常终止 / 超时**三态（此前"异常终止"与"超时"
  共用一个分支 ⇒ **溢出被拦住看起来像产物挂起**）；新增 `is_trap_code` 按 Windows **NTSTATUS
  severity**（`0xC0000000` 及以上）判异常终止。新用例 `overflow_runtime_trap` 把期望钉到
  **`0xC000_001D`**（`ud2` 的 NTSTATUS）——静默回绕会正常退出 `-2147483648`，必红。
+ **新守卫 `e2e_trap_semantics_invariants`**：钉住三件"改坏了不会红"的事——panic handler 必须是
  `ud2`、代码里不许有 `known_failure: true`、每个 `expect_trap` 用例必须钉一个真实 trap 码
  （≥ `0xC0000000`，避免与"回绕值"撞车）。守卫自己两次踩到**自我命中**（扫描面含自身字面量），
  已改用 `concat!` 编译期拼针。
+ **台账纠偏（实测推翻立项前提）**：`WORKAROUNDS.md` 的 `WA-05`（溢出 Assert）改判**已关闭**
  ——`TerminatorKind::Assert` 早已实现（`lower/mod.rs:847-899`）、套件默认路径**早已不传**
  `-C overflow-checks=off`；`WA-36`（窄有符号）v14 已关闭（`lower/rvalue.rs:47-75`）；
  `WA-46`（向量溢出宽度）生成级已就位（`isa/x86.toml` 的 `[machine.spill.fpr16/32/64]` +
  按宽度分派 + fail-closed），残余只是**仅有 AVX-512F 机器**才能复现的 CI 偶发；
  `WA-40`/`WA-41` 已随 `WA-42` 根治关闭。方案 §1 的 E5 加了"已被 M1 推翻"标注，
  账本 §8 记下教训：**`WORKAROUNDS.md` 的"根治方向"列是写那一行时的判断，不是现状**。
+ 验收：全量 e2e **106/106、零 `KNOWN`**；代码里 `known_failure: true` = 0；新守卫绿；文档 0 issues。

### Docs (2026-10-11) — 项目 v26 M5：契约回写 + 账本归档（`CLAUDE.md` 回到指令预算内）

+ **`CLAUDE.md` 的 v26 口径回写**：依赖图删掉已删除的 `forge-mem`（并写明 `forge-isa-runtime`
  现在承载什么：生成物 trait/数据型 + `cpu` 主机探测 + `diagnostics` 诊断出口 + `mem`）；
  前端线（`forge-hir`/`forge-hir-macro`/`mini_c`）从图里移出，改成"已降级为 `examples/hir-demo/`
  并从工作区 exclude"的说明；`Frontend Pipeline` 与 `HIR Lowering` 两节加降级横幅；
  `IR_FORMAT_VERSION` 的说明从"当前 3"更新到 **4**。
+ **修掉一个真问题：`CLAUDE.md` 超出工作区指令预算（65536 字节）而被加载器截断**——
  实测回写后 66069 字节 ⇒ 代理看到的指令文件不完整。按文档纪律把体量最大的 `## Testing Notes`
  （297 行）**原地搬**进 [`docs/guides/testing-notes.md`](docs/guides/testing-notes.md)
  （带状态头与来历），`CLAUDE.md` 留 4 条最易踩的摘要 + 链接；两处文档地图（`CLAUDE.md` 树、
  `docs/README.md` guides 表）登记新文档，`Key Architecture Rules §4` 的指针改指它。
  **实测 66069 → 37659 字节**（预算内），新文档 30112 字节，两者 markdownlint 0 issues。
+ **账本归档**：`docs/plans/project-v26-plan.md` → `docs/archive/project-v26-plan.md`
  （`git mv` + `⚠️ ARCHIVED` 头 + 五个里程碑提交号 + 现行入口），并全量更新引用
  （`docs/README.md` 的 plans 表、`CLAUDE.md` 的树、`docs/archive/README.md` 的清单）；
  归档为单独一次 docs 提交。
+ 验收：`cargo forge gate` 全套见下。

### Changed (2026-10-11) — 项目 v26 M4：能力补齐（谱侧缺口台账 · 三条待做收口 · 两项 P1 分析）

+ **谱侧能力台账（`[cap:SPEC_*]`）**：生成器 `forge-isa-dsl/src/dsl/codegen/**` 里有 **66 个发射型
  `Unsupported`**（写在 `quote!` 里 ⇒ 使用者看得见的报错），**零个**带 `[cap:]` 标记。按 v25 M2 的
  四分类逐站点判定后**归并成 5 个 `SPEC_*` 类**（`SPEC_MOVE_TABLE_GAP` 19 / `SPEC_TERMINATOR_MISSING` 10 /
  `SPEC_ROLE_MISSING` 6 / `SPEC_FRAME_OP_MISSING` 5 / 复用 `SPEC_LOWERING_RULE_MISSING`）+ 复用
  `ABI_PLAN_UNAVAILABLE` + **3 个新 `ABI_*`**（`ABI_RET_SLOT_MISSING`/`ABI_CLOBBERS_MISSING`/
  `ABI_PLACEMENT_UNEMITTED`）；畸形 IR 的 9 处改 `IrError::Internal`。判据：`dsl/` 下裸 `Unsupported(`
  **== 0**。落地踩到 `capability(id, msg: impl Display)` 下 `.into()` 的类型推断歧义（E0283，42 处、
  含尾随逗号形态）⇒ 全部去掉。
+ **守卫扩面**：`capability_gaps_guard` 扫描面加生成器目录，并引入**类别型 ID**（`SPEC_*`/`ABI_*`
  允许多触发点 = "一个能力类"；其余 ID 仍守"一个决定点"）；台账加 7 行 + §3.4 证据。
+ **三条 `待做` 清零**（计划口径：落地或明确降级）：`VA_ARG_AGG_RULE`/`VA_ARG_VECTOR_MEM`/`AGG_LOAD_USE`
  全部改为**有条件的 `不做`**（台账 `待做` 行数 = **0**，见 `backend-gaps.md` §3.5）。其中
  `VA_ARG_AGG_RULE` 的结论最值得记：照 win64 形状声明能让**我方自洽**通过，但与**外部 C 调用方**
  互操作会静默错值（引擎给 aapcs64/lp64d 的聚合落点是 `ByRef`，而这两份 psABI 的聚合变参按值展平）
  ⇒ 按"宁可明确拒绝"保持 fail-closed。
+ **两项 P1 只做分析**（计划口径）：补测 `codegen_mem`（criterion 中位 **291.5 µs**，regalloc 55–70%）
  与 `codegen_big_loop`（**453.2 µs**，regalloc 60–70%），证实原先"以多数函数比例推断"的结论
  ⇒ **无新的低风险高收益落地项**（唯一大头是 regalloc 的数据流规模本身）。写回 `OPTIMIZATION.md`
  §3 与新增 §3.1（含计时口径 vs criterion 口径的说明）、`codegen_stage_profile.md` 文末两例实测表。
+ **顺手修掉一个真缺陷**：`CF_CODEGEN_TIMING` 是**第三个开关命名族**（`CF_*`），绕过了 M1 立起来的
  观测开关台账（M1 守卫只认 `FORGE_TRACE_*`，所以一直没被发现）⇒ 改名 **`FORGE_TRACE_TIMING`**、
  登记进 `KNOBS`（41 → **42**）与契约文档 §7 表，三方守卫钉住。
+ 验收：`cargo forge gate` 全套见下；`forge-isa-dsl` 301 lib + 12 测试二进制（**真的编译三份发行谱与
  全部夹具的生成物**）与 `forge-codegen --lib` 2746 全绿。

### Added (2026-10-11) — 项目 v26 M3：单命令门禁（`cargo forge gate`）+ CI 补齐

+ **`cargo forge gate`**：把只写在 `CLAUDE.md` 里的门禁清单变成**可执行注册表**——一条命令跑
  fmt → clippy（workspace `--all-features -D warnings`）→ 文本层门（`forge-ir --no-default-features`）
  → `check -p forge-rustc` → workspace tests → `forge-codegen --lib --all-features` → **28 条守卫**
  → JIT 两矩阵 → asm 三档 → 文档 lint；逐项打印 `名称/在验什么/用时/退出码` + 汇总表 + 非零退出。
  开关：`--guards`（只跑守卫，实测 28/28 全绿 55.7s）、`--subset quick|linux|windows`（CI 复用）、
  `--no-docs`、`--list`、`--json`。复用既有 clap CLI（未新开 xtask 层）；子进程统一
  `CARGO_INCREMENTAL=0`。
+ **新增 CI `docs` 作业**：与 gate 文档步同一口径（`git ls-files '*.md'` 去掉 `docs/archive/**`
  ——归档冻结、明示豁免）⇒ 当前 0 issues。顺带修掉两颗现行文档的钉子（`isa-dsl.md` 连续空行、
  `aarch64-encoding-ref.md` 行内 `<op>`），并按实测重写 `docs/guides/lint.md` 的基线节。
+ **`--all-features` 执行覆盖**：Windows 作业加 `cargo test -p forge-codegen --lib --all-features`
  （`parallel` 此前只被 clippy 覆盖、从未被执行）；Linux 作业加 `cargo check -p forge-codegen --all-features`。
+ **codegen 事件落盘**：Windows 作业设 `FORGE_CODEGEN_EVENTS` + `if: always()` 打印步。
+ **覆盖率下限棘轮**：`coverage` 作业此前只上传（`fail_ci_if_error: false`）⇒ 现按仓库内的判定程序
  `docs/performance/coverage_floor.awk` 比对 `docs/performance/coverage_floor.txt` 的下限。
  **实测基线 81.16%**（LF 59678 / LH 48436），当轮下限钉 **80**（跨平台余量）；达标/低于下限/缺数据
  三个方向都验过。落地时改过一次实现（`--fail-under-lines` 在 profdata 缺失时读成 0/0）并修掉一处
  awk 缺陷（按文件名匹配下限文件 ⇒ 换名会静默读成 0，改为 `NR == 1`）。
+ **文档**：`CLAUDE.md` 的 Build Commands 改门禁优先；`docs/guides/coverage.md` 加下限棘轮节。
+ 验收：**以新命令本身**跑全套（自举自验）——39 步 + 文档步全绿。

### Changed (2026-10-11) — 项目 v26 M2：无消费者面清理（三项破坏性变更）

**破坏性**：① `Instruction::isel_strategy` 整条删除，`IR_FORMAT_VERSION` **3 → 4**（旧流在头部
即报版本不符）；② `forge-mem` crate 删除（并入 `forge-isa-runtime::mem`）；③ 前端线
（`forge-hir`/`forge-hir-macro`/`mini_c`）移出工作区、降级为 `examples/hir-demo/`。

+ **`forge-mem` 折叠**：`ExecutableMemory`/`MemError` 搬进 `forge_isa_runtime::mem`（`git mv`
  保留历史），`forge-codegen` 改一处 `use`，删 crate + 依赖边 + workspace 成员。其
  `CpuFeatures::detect()`（含一套 CPUID/xgetbv）**零调用者**，只有 `host_arch_name()` 有用
  ⇒ 并入 `forge-isa-runtime/src/cpu.rs`，其余删。根的 `code_forge::mem` 路径保持不变 ⇒ 7 处调用点零改动。
+ **删 `isel_strategy`**（IR 字段只有存取 + 二进制往返、后端零读点）：字段 + 三访问器 +
  `ir/isel_strategy.rs` + 二进制读写 + verifier 校验项 + prelude + `forge-opt` 三处**真透传代码块**
  （此前只看到注释）；契约文档 `binary-format.md` 同步（标题/状态头/FUNCS 字段清单）。
  测试面：删专用测试文件，`mini_inst` 夹具少一个字段（两个手写字节布局的负向夹具随之错位，已同步）。
+ **v11 遗留**：删 `forge-grammar/grammars/asm_v11.lx` 与只测它的 `asm_grammar_test.rs`；
  `ast_lowering_test.rs` 收敛到 `ir.lx`。
+ **前端线降级为示例**：三个目录 `git mv` 到 `examples/hir-demo/`（`hir/`、`hir-macro/`、mini_c 内容），
  根 `Cargo.toml` 去掉三个成员并 `exclude`；新增 README 写明"不再有 CI 覆盖、会静默失效"与重新纳入的两条路。
  `forge-grammar`（CST/AST/visit + `ir.lx` 测试）仍是成员。
+ **`allow(dead_code)` 审计（34 处，证据化）**：先全摘、让 rustc/clippy 指认再分类回填——
  **17 处多余**（发射进生成物；生成物整体带 `#[allow(warnings, clippy::all)]`，证据 `gen_file.rs:274`）已删；
  **1 处真死**（`debug_check_shape`，其文档自述"不在发射路径上强制"）已删；**2 处只被 test 目标用**
  （`matches_pattern`/`Reader::header`）升级为精确写法 `#[cfg_attr(not(test), allow(dead_code))]` + 理由。
+ 验收：fmt / clippy（workspace `--all-features -D warnings` + `forge-ir --no-default-features` 文本层门）/
  `cargo check -p forge-rustc` / `cargo test --workspace --exclude forge-rustc` 全绿；JIT 两矩阵
  （197/3/0 · 136/64/0）；asm 三档（3/1/1）；守卫全绿；改动文档 markdownlint 0 issues。

### Changed (2026-10-11) — 项目 v26 M1：诊断面单一来源（错误码台账 · 事件 sink · 开关台账）

**破坏性变更**：ISA-DSL 错误码 `DSL-ABI` → **`DSL-MACHINE`**（`[abi]` 节 v20 A5-3 已删，
用 `DSL-ABI` 指 `[machine]` 误导）；`[operand.<名>]` 与 `[machine]` 的诊断码从 `DSL-OTHER`
改为承诺已久的 `DSL-SLOT`/`DSL-MACHINE`。按码过滤/统计的下游需同步。

+ **错误码台账（+ 修一个真 bug）**：`section_code` 的 kind 映射表**缺 `operand`/`operand_slots`/
  `machine`** ⇒ 这两节的诊断一律落到 `DSL-OTHER`，而文档写着"出现 `DSL-OTHER` 即说明消息缺前缀、
  应报 bug"——判据被反用，文档承诺的 `DSL-SLOT`/`DSL-ABI` **从未发出过**。补齐映射后：
  源码 **31** 码 == `docs/reference/isa-dsl-errors.md` §2 表 **31** 码。
  文档另修两处：被划掉写成"已删除"的 `DSL-EMIT`（该码仍在用）与 `LINT` 行漏掉的两个码
  （`LINT-ASM-AMBIG`/`LINT-FORM-FIELD-UNUSED`）。
+ **新守卫** `error_codes_match_doc`（`forge-isa-dsl/tests/`）：源码字面量 ↔ 文档 §2 表
  **双向相等**（只扫该小节，避免 `dsl-tutorial` 这类链接文本误命中）。诊断矩阵
  `diag_matrix_tests.rs` 如实抓到 2 例把"缺映射"当契约的期望码，已改正。
+ **事件 sink 下沉**：新增 `forge_isa_runtime::diagnostics`——四个 `FORGE_*_EVENTS` 通道原先
  各写一遍 `OpenOptions+append+writeln`，现共用一处落盘实现 + `CHANNELS` 注册表；
  `pipeline::events`、`jit_event`、asm 与 e2e 事件全部改薄转发（**行的格式不动**，CI 的
  `Select-String` 依赖那些 token）。写失败仍静默，未登记通道 fail-closed。
  根 facade 补 `pub use forge_isa_runtime;`。
+ **新守卫** `event_channels_are_bijective`：`CHANNELS` ↔ 源码调用点 ↔ 契约文档通道表三方双向。
+ **开关台账归并**：`KNOBS`/`knob` 从 `forge-codegen::pipeline::trace` 下沉到同一 leaf，**后端与
  `forge-rustc` 并成一张 41 项的表**（新增 `trace(后缀)`）；4 处**绕过台账直读环境变量**的写法
  （`FORGE_TRACE_FN_SIZE`/`CODE_HEX`/`LIVE`/`DW`）改走 API；`forge-rustc/src/trace.rs` 里那份
  会漂移的 13 行名字表删除。守卫 `trace_knobs_are_bijective` 重写为**真三方**（源码字面量 ↔
  台账 ↔ 文档 §7 表，扩到 41 行）。
+ **文档**：契约文档修掉 v25 插入小节留下的**重号**（同号 `## 6`/`## 7` 并存），现按物理顺序 1–13；
  §7 开关表扩到 41 项并写明"一张表管两侧"；§12 新增事件通道表。
+ 验收：`forge-codegen --lib --all-features` 全绿；workspace exit 0；JIT 两矩阵（197/3/0 · 136/64/0）；
  asm 三档（3/1/1）；守卫全绿（新增 2 条）；clippy `-D warnings`、fmt、`cargo check -p forge-rustc` 干净；
  改动文档 markdownlint 0 issues。

### Docs (2026-10-11) — 后端 v25 M5：契约回写（`backend-pipeline.md` 补齐 v25 的四项结构）

`reference/backend-pipeline.md` 此前是 v24 口径（合法化两相 / 值描述 / 缺口台账 / 观测开关 +
M3 加的事件表），v25 的四项结构变化没有落进契约。

+ 新增 **§6 一次分类，处处消费**：数据流图 + 五条契约（一个入口 / 规范类型 / 生产者签名里没有
  机器与约定名 / **按值查不按序号查**（sret 前插会让 `op_idx` 漂移）/ 不登记 `va_arg` 的理由）。
+ 新增 **§7 错误分类学**：四分类表（`capability`/`internal`/`semantic`/`other`）+ 构造器住
  `forge-ir` + "缺口 ID 必须是字面量"这条实测教训 + `Unimplemented` 已删除。
+ 新增 **§8 模块边界与规模棘轮**：切分后的目录职责表 + 叶子方向纪律（含 emission/frame_layout/
  lowering 的例外与理由）+ **规模棘轮表** + "测试专属模块怎么识别"。
+ 状态头与"别绕过"的纪律从三条扩到**四条**（新增"ABI 落点只有一个来源"）；相关账本改指 v25。
+ **守卫扩展**（文档表 ↔ 代码表双向相等）：`file_size_ratchet` 新增 `ratchet_table_matches_doc`
  （规模表 = `BUDGETS`；目录表 = 四个切分目录且树上存在）；`error_taxonomy` 新增
  `taxonomy_matches_doc`（文档分类表 = 守卫冻结的四分类）。
+ 验收：`forge-codegen --lib` 2836 passed；workspace 全量 exit 0；JIT 两矩阵（197/3/0 ·
  136/64/0）；asm 三档（3/1/1）；**十二条**管线守卫全绿；clippy `-D warnings` 与 fmt 干净；
  改动文档 markdownlint 0 issues。
+ **v25 收口**：M1–M5 全部落地、各自单独提交；删除清单全部清零（`AggArgLoc`、两个单类型探针、
  五种计划枚举、`IrError::Unimplemented`、`pipeline/gaps.rs` 在代码里零引用）。

### Changed (2026-10-11) — 后端 v25 M4：管线大文件按职责切分（模块名不变，调用点零改动）

**问题**：四个管线文件 >1000 行（`compiler.rs` 1616 / `regalloc_bt.rs` 1544 / `va_expand.rs` 1113、
`agg_call.rs` 690）——读改成本高、评审难。

+ **逐字搬移**成目录（模块路径不变 ⇒ 调用点**零改动**）：`compiler/`（mod 615 / state 399 /
  alloc_integration_tests 615）、`regalloc_bt/`（mod 148 / state 944 / spill 113 / tests 126 /
  clobber_map_tests 239）、`va_expand/`（mod 73 / slots 126 / expand 621 / tests 311）、
  `legalize/agg_call/`（mod 45 / args 313 / results 342）。
+ **新增守卫** `file_size_ratchet`（2 条）：`src/pipeline/**` 每个文件 ≤ 签入上限（缺省 700；
  超限的 6 个文件逐条写明"为什么它必须长"），**只许按实测下调**，白名单死条目也红；
  `leaf_modules_do_not_depend_on_the_driver`：`legalize/**`、`va_expand/**`、`regalloc_bt/**`
  不得依赖 `compiler`/`session`（次序与状态只由驱动持有）。
+ 拆分的连带修正（都有实测记录）：子模块私有成员经 `pub(super)` 放开；`CompileState` 仍被
  emission/frame_layout/lowering 引用 ⇒ `pub(crate)` + `mod.rs` 重导出；
  **导入裁剪必须按 `cargo test` 口径**（只被测试模块经 `use super::*;` 使用的导入在 `cargo check`
  下"未用"，删了测试就红）；三条按路径白名单的旧守卫（`entity_tables`/`classification_single_entry`/
  规模棘轮）同步更新到新布局。
+ 验收：`forge-codegen --lib` 2836 passed；workspace 全量 exit 0；JIT 两矩阵（197/3/0 ·
  136/64/0）；asm 三档（3/1/1）；八条管线守卫全绿；clippy `-D warnings` 与 fmt 干净。

### Added (2026-10-11) — 后端 v25 M3：codegen 的机读事件出口（`FORGE_CODEGEN_EVENTS`）

**问题**：JIT / asm / E2E 三个面早就有落盘事件，**codegen 是唯一没有机读出口的阶段**——
11 个诊断开关全是 `eprintln!` 人读文本，CI 只能靠正则去啃散文。

+ 新增 `pipeline::events`：出口 = **`FORGE_CODEGEN_EVENTS=<文件>`**（追加写；未设置零开销；
  **写失败静默**——观测不得影响编译结论，与 `FORGE_JIT_EVENTS` 同约定）。字段序固定 ⇒ 输出
  确定、测试可整行比对；自带最小 JSON 转义，不引入依赖；`EVENTS` 台账是唯一登记处，
  未登记的名字 debug 下当场 panic。
+ **六个事件**：`COMPILE-START`、`COMPILE-DONE`（总耗时 + 八段微秒 + `code_bytes`，
  **无条件发**，不再藏在 `CF_CODEGEN_TIMING` 后面）、`CAPABILITY-GAP`（失败路径的缺口 ID）、
  `LEGALIZE-PLAN`（可动边界条数 + 问引擎次数）、`ALLOC-SUMMARY`、`EMIT-SUMMARY`。
  `compile_with_alloc` 拆成外层包装（失败路径）+ 内层实现（持有阶段耗时）⇒ 两边字段都真实。
+ **契约文档新增「事件表」**；新增守卫 `events_guard`（2 条）：`event_names_are_bijective`
  把源码调用点 ↔ `EVENTS` ↔ 文档事件表**三方双向**钉住（比 v24 M4 多钉一层文档）；
  `events_land_on_disk` 端到端验证事件真的落盘（不可用时明确 SKIP，不假绿）。
+ 验收：`forge-codegen --lib` 2836 passed；workspace 全量 exit 0；JIT 两矩阵（197/3/0 ·
  136/64/0）；asm 三档（3/1/1）；六条管线守卫 + `error_taxonomy`/`capability_gaps` 全绿；
  clippy `-D warnings` 与 fmt 干净。

### Changed (2026-10-11) — 后端 v25 M2：错误分类学（失败语义分层 + 变体去冗余）

**问题**：`IrError::Unsupported` 一名三用——**能力缺口**（换约定/补规则/换 ISA 就能解）、
**畸形 IR / 内部不变量**（输入坏或代码 bug）、**使用错误**（约定没注册、装载 ISA 不匹配）。
三者对用户的意义完全不同，混在一起 ⇒ 台账失焦、CI 没法按类分流；而
`IrError::Unimplemented` 与它语义重叠（"没实现"就是"做不了"），是纯粹的第二套说法。

+ **构造器下沉**：`capability`/`gap_id` 从 `forge-codegen::pipeline::gaps` 移到 **`forge-ir`**
  （`IrError::capability(id, msg)` / `IrError::gap_id()`）——`forge-isa-runtime` 的 trait
  默认实现也要能构造缺口错误；`pipeline/gaps.rs` 删除，**21 处调用点**迁移，自测随之下沉
  （并新增"`Internal`/`Semantic` 不该伪装成缺口"的断言）。
+ **四分类落地（41 处）**：畸形 IR/内部不变量 ⇒ `Internal`（33 处）；使用/数据错误 ⇒
  `Semantic`（5 处：约定未注册、JIT 装载 ISA 不匹配、管线未注册/工厂不匹配、约定数据自相矛盾）；
  能力缺口 ⇒ `IrError::capability(ID)`（3 处）。前提核对：`IrError` **不进二进制格式** ⇒
  删变体无需升 `IR_FORMAT_VERSION`。
+ **删掉第二套说法**：`IrError::Unimplemented` 变体连同 `Display` 臂删除，全仓零出现；它的
  3 处调用点是 `TargetFrameLowering` 的 trait 默认实现（"这份谱没覆盖这个帧操作"）⇒ 转为能力缺口
  `FRAME_EMIT_EPILOGUE_JUMP`/`FRAME_SPILL_LOAD`/`FRAME_SPILL_STORE`（进台账，可搜索、可追踪
  "哪份谱没接"）。
+ **台账补 5 行**（`FRAME_*`×3、`SPEC_LOWERING_RULE_MISSING`、`VECTOR_TIER_UNSUPPORTED`），
  守卫扫描面扩到 `forge-isa-runtime/src`；能力缺口 ID 一律以**字面量**写在构造点
  （改成变量会让台账守卫把行判成死行——M1 实测撞到过）。
+ **新增守卫** `error_taxonomy`（2 条）：后端 + 运行时的 `src/` 不允许裸 `Unsupported(`；
  全仓无 `Unimplemented`。**守卫抓出一处 M3 期错误标注**：`va_expand` 里"宿主 plan 缺字段"
  借用了向量缺口的 ID ⇒ 改为 `Internal`，那条 ID 的语义不再被稀释。
+ 验收：`forge-codegen --lib` 2834 passed；workspace 全量 exit 0；JIT 两矩阵（197/3/0 ·
  136/64/0）；asm 三档（3/1/1）；`error_taxonomy`/`capability_gaps`/`trace_knobs`/
  `entity_tables`/`classification_single_entry`/`read_path_budget` 全绿；clippy `-D warnings`
  与 fmt 干净。

### Changed (2026-10-11) — 后端 v25 M1：一次分类处处消费（ABI 落点只有一个来源）

**问题**（立项侦察实测）：v24 的两相化只做完了**改写**那一半——计划相产出的 `AbiEdges`
**8 处引用里只有 1 处写、零消费者**；落点决策散在 **5 个调用点**，每处各自跑一遍引擎规划
（`plan_for_shapes` + 注册表读锁），并用本仓手抄的三值枚举表达（它只是 `Placement` 的投影）。

+ 新增 `pipeline/legalize/classify.rs`：`Classifier` 是**唯一**问 ABI 引擎的通道，返回
  `forge-abi-model` 的**规范类型**（`ArgLoc`/`RetLoc`/`Placement`），**按类型去重**
  （同一类型只问一次引擎）。另配两个口径函数：`is_by_ref` 与 `scalar_stack_is_a_data_error`
  （"约定把聚合判成标量栈落点" = **数据错误**，不是能力缺口）。
+ `AbiEdges` 成为**唯一落点来源**并被生产者消费：`params`/`ret`/`call_args`/`call_results`
  换成规范类型（**删除**五种计划枚举 `ParamPlan`/`RetPlan`/`ArgPlan`/`CallResultPlan`/
  `VaArgPlan` 及其手抄边结构）；生产者签名里**不再有机器与约定名**——规则由类型系统保证，
  计划缺失 ⇒ `IrError::Internal`。
+ **删除清单清零**：手抄落点枚举与两个单类型探针（`common.rs` 92 行）全仓零引用。
+ **按值查而不是按序号查**（实测踩到的坑）：`agg-call` 会往 call 实参表前插 sret 缓冲指针，
  之后 `op_idx` 比计划记录时大 1 ⇒ 计划按**值句柄**查，不受插入影响。
+ 两处"判据重复"收进计划相（`[cap:AGG_RET_PLACEMENT]`、标量栈落点），pass 侧改为 `Internal`；
  删掉只在摘要里被读的两处装饰（`va_args` 载荷、`Classifier::ret_is_sret`）。
+ 新增守卫 `classification_single_entry`（3 条）：`plan_for_shapes` 只许出现在三处白名单
  （双向相等）；`Classifier::new` 只许在计划相、生产者**必须**引用 `AbiEdges`
  （否则计划只是装饰）；旧符号全仓含注释零出现。
+ 读路径预算按实测**下调**（`agg_call` 5→4、`agg_param` 9→8）。
+ 验收：`forge-codegen --lib` **2835 passed**；workspace 全量 exit 0；JIT 两矩阵（197/3/0 ·
  136/64/0）；asm 三档（3/1/1）；`capability_gaps`/`trace_knobs`/`entity_tables`/
  `read_path_budget`/`classification_single_entry` 全绿；clippy `-D warnings` 与 fmt 干净。

### Docs (2026-10-11) — 后端 v24 M5：契约回写（`backend-pipeline.md` 改为 v24 现行契约）

`reference/backend-pipeline.md` 此前只写 v23 M1 的编译入口，v24 的四项结构变化没有落进契约。

+ 文档改写为 **8 节现行契约**：编译入口与装载 / **管线阶段表** / **合法化两相 + pass
  合并次序**（含"位置只有一种表示、它写一次"、三批入口的理由、聚合与变参的表示契约）/
  **值描述单一来源**（含"别用 `ValDesc::bytes()` 代替 `value_bytes()`"这条实测教训）/
  能力缺口台账指针 / 模块签名表与擦除入口 / 观测开关表 / 相关账本。
+ 新增机器可读的阶段清单 `pipeline::STAGES`（`legalize`/`isel`/`regalloc`/`frame`/`emit`/`jit`），
  以及两条守卫：`pipeline_stages_match_doc`（文档阶段表 ↔ `STAGES` 集合相等）与
  `legalize_passes_match_doc`（文档 pass 表 ↔ 驱动合并次序**含顺序**相等）。
+ 文档地图指向 v24 口径；交叉引用复查无死链。
+ 验收：`forge-codegen --lib` 2832 passed；workspace 全量、JIT 两矩阵（197/3/0 · 136/64/0）、
  asm 三档（3/1/1）、clippy `-D warnings`、fmt、markdownlint 全绿。
+ **v24 收口**：M1–M5 全部落地、各自单独提交；删除清单全部清零（`AggSlots`、旧改写函数、
  `move_to_front`、`xreg_types`、旧开关白名单在代码里零引用）。

### Changed (2026-10-11) — 后端 v24 M4：观测开关**台账化**（诊断开关不可能静默失效）

旧实现维护一份开关白名单（`TRACE_KEYS`）+ 整表缓存：**名字漏进白名单 ⇒ 该开关恒为假、
静默失效**，探针一句都不打印而没人发现——`FORGE_TRACE_AGG` 就这样死了几个月
（"文档承诺可见、实际看不见"）。

+ 新增 `pipeline::trace`：`KNOBS` 台账（11 项，每项写清"看什么"）+ `knob(name)`。
  **直接按名字问环境变量 ⇒ 任何名字都生效、不可能因为漏登记而失效**；忘登记的名字
  debug 下 **panic**（当场暴露）、release 下警告一次后照常生效。每开关一个 `OnceLock`
  缓存（每指令热路径上都有查询，裸 `var_os` 实测占 codegen 7–18%）。
+ **落地时抓到第二个同类静默失效**：`FORGE_TRACE_M4` 的调用点在、白名单里却从来没有它
  （与 `FORGE_TRACE_AGG` 同病）；反过来白名单里的 `FORGE_TRACE_ISEL` 一处调用点都没有
  （死条目）。新机制下前者自动生效、后者不再登记。
+ **删除**旧白名单与 `trace_enabled`，**35 处调用点**迁移到 `trace::knob("…")`。
+ 新增守卫 `trace_knobs_are_bijective`：源码调用点 ↔ `KNOBS` 台账 ↔
  `reference/backend-pipeline.md` 的表格**三方双向相等**（源码多 = 旧病、台账多 = 死条目、
  文档分叉 = 承诺与实现不符）；同文件钉住"旧白名单名全仓归零"。
+ 契约文档新增「观测开关」一节（11 行表格由守卫钉住与代码一致）。
+ 验收：`forge-codegen --lib` 2830 passed；`trace_knobs` 2 passed；workspace 全量、
  JIT 两矩阵（197/3/0 · 136/64/0）、asm 三档（3/1/1）、clippy `-D warnings`、fmt 全绿。

### Changed (2026-10-11) — 后端 v24 M3：能力缺口**台账化** + `insertvalue` 链聚合实参**打通**

能力缺口过去以 60+ 处 `Unsupported(...)` 的形态散在代码里：没人知道"这台机器 / 这份约定 /
这份谱到底缺什么"，失败信息也只在触发那一刻可见。

+ **失败信息自带身份**：新增 `pipeline::gaps::capability(ID, msg)`，21 处能力类失败点改成
  以 **`[cap:ID]`** 开头（另有 `gap_id(&IrError)` 供上层取 ID）。用户/CI 可按 `[cap:…]`
  搜索、归类、豁免。
+ **新增台账** [`docs/reference/backend-gaps.md`](docs/reference/backend-gaps.md)：21 行，
  每行给出"谁在什么条件下撞到 / 决策（**已实现·数据层·不做·待做** 四值封闭）/ 钉住它的
  守卫或数据条件"，§3 记逐条证据。
+ **新增双向守卫** `capability_gaps_guard`：源码缺口集合与台账 ID 列**逐一相等**（多一行、
  少一行都红）、每个 ID 在源码里恰好一处、决策列必须落在封闭词表；守卫自身抓出并修好了
  两处盲点（ID 写在下一行的多行调用、决策列的 Markdown 强调）。
+ **`insertvalue`/`undef` 链搭出来的聚合作为实参：从 v23 的 fail-closed 变为已实现**——
  物化相导出共享助手 `ensure_value_addr`（**沿定义链递归**求地址），调用实参 pass 复用它
  并与 `load` 来源共用同一条逐段 load 路径。v23 的契约断言翻转为**真执行断言**
  （`test_jit_va_arg_reads_unnamed_aggregate_arg` 的 `f2() == 11`）。
+ **aapcs64/lp64d 的聚合变参**：只读测量（生产同一探针）——24B 聚合形参 `win64 ⇒ ByRef`、
  `sysv64 ⇒ StackByVal{16,24}`、`aapcs64/lp64d ⇒ ByRef`。台账记 `待做`：与 win64 同形**可**
  声明，但这两份约定没有聚合变参的真执行通道 ⇒ 按"不静默错值"先不声明（前置条件写明）。
+ 文档地图登记 `reference/backend-gaps.md`（`docs/README.md` + `CLAUDE.md`）。
+ 验收：`forge-codegen --lib` 2827 passed；`capability_gaps_guard` 1 passed；
  workspace 全量、JIT 两矩阵（197/3/0 · 136/64/0）、asm 三档（3/1/1）、clippy `-D warnings`
  与 fmt 全绿。

### Changed (2026-10-11) — 后端 v24 M2：值描述**单一来源**（`xreg_types` 退场）

"值的类 / 位宽 / IR 类型"过去分散在**四张平行表**里（一张 `XReg` 键的 `xreg_types` 加三张
`VReg` 键的 `vreg_classes`/`vreg_types`/`vreg_widths`），消费点取到类型后还要各自再推类与位宽
（两个回填点口径不同 ⇒ 同一条绑定两种结果）。现在**一个值只有一份描述**。

+ 新增 `forge_isa_runtime::ctx::ValDesc { class, bits, ty }`：类与位宽**只有一个推导处**
  （`LowerCtx::set_value_desc`，从 IR 类型推），读写一律走访问器（`desc_of` /
  `is_fp_value` / `is_vector_value` / `value_bytes` / `type_bits_of`），**表本身私有**。
+ **删除** `LowCtx::xreg_types` 与三张 `VReg` 键的表；实测其中三张是**孑遗**
  （只写不读 / 唯一读者 `is_float_vreg` 无调用者，`xreg_width` 亦无调用者）⇒ 连同
  `alloc_vreg*`/`next_vreg`/`temp_vregs`/`zero_vreg` 整族删除。
+ 生成物**不再依赖任何"值→类型"旁表**：模板 16 处 + 谓词属性 6 处改走访问器，
  宿主 9 处（7 写点 + 2 读点）改走 `set_value_desc`/`desc_of`；**三份发行谱的 TOML 零改动**。
+ 新守卫 `only_one_value_desc_table`（`tests/entity_tables.rs`）：值描述表恰好一张、值类型是
  `ValDesc`、表不得 `pub`，且四个旧名在全仓生产代码里零出现。
+ **不回潮实测**（按时间切分的产物 A/B）：生成物 x86 6,355,380 → 6,348,453 字节
  （−0.109%）、riscv64 −0.130%、arm64 −0.011%——三份谱全部变小。
+ 验收全绿：workspace 全量测试、JIT 两矩阵（197/3/0 · 136/64/0）、asm 三档（3/1/1）、
  `forge-codegen` 2826 passed、`read_path_budget` 2 passed、clippy `-D warnings`/fmt 干净。
+ 逐批证据见 `docs/archive/backend-v24-plan.md` §6（v24 收口后归档）。

### Changed (2026-10-11) — 后端 v24 M1：合法化**两相化**（计划只读 + 一次性物化）

聚合合法化从"每个 pass 就地改写 IR 并各自重写使用处"改成**两相**：只读的**计划相**算出
ABI 边界计划，**物化相**按指令序**单遍**改写。用户可见行为不变（值/ABI 与既有约定一致），
但这一族的结构性缺陷被消除。

+ **新增计划相**（`pipeline/legalize/plan.rs`）：只读地产出整函数的 ABI 边界计划
  （形参/返回/调用实参/调用结果/`va_arg` 五类落点 + 完整描述），落点一律问 `forge-abi`
  探针，管线里**没有约定名分支**。守卫 `plan_is_read_only`（跑完计划相后指令/值/常量数
  **逐项相等**）钉住"决策不改写"。
+ **新增物化相**（`pipeline/legalize/materialize.rs`）：按指令序单遍处理，逐块读出快照、
  为每条指令算出要插入的生成指令、拼成新的 `inst_order` **一次写回** ⇒ 生成指令**天生排在
  消费者之前**。"生成到块尾再搬"那套写法（6 个调用点，v23 收尾 use-before-def 真凶的温床）
  **整族删除**。嵌套聚合字段提取改为**登记子地址**（旧实现在此处会新开槽再搬一遍字节）。
+ **位置表换掉**：`Value → (地址, 槽类型)` 的旧表换成 `Places`（`Mem`/`Segs`，**写一次**——
  同值二次登记 debug 下 panic）。旧表的"槽类型"是**冗余**（登记时传的都是该值自己的 IR 类型）。
+ **事实交接 `shadow_agg`**：by-ref 聚合形参重建签名时旧值被全局替换成 `ptr`，
  "这个 ptr 代表 T 型聚合"此后不在 IR 类型里——生产者显式交给物化相。
+ **驱动分三批**（定序守卫同步）：聚合族 → va（要排完 ABI 槽）→ **收尾**（物化 + GEP）；
  理由：`Va` 也会把聚合实参拷进自己的帧槽，物化必须等它。
+ 新守卫：`materialize_is_idempotent`、`materialized_load_precedes_its_consumer`
  （逐块核对"每条 `Copy` 的操作数定义排在它之前"）、`segment_resident_extract_is_materialized`。
+ **删除清单清零**：`AggSlots`、按值驱动的旧改写函数、按地址内存化助手与"生成后搬运"助手
  在代码里**零引用**；`markdownlint`/`clippy`/`fmt` 与全量门禁（workspace + JIT 两矩阵 +
  asm 三档棘轮）全绿；读路径预算按**实测**下调（`agg_expand.rs` 6 → 2，新增两相各 1）。
+ 逐批证据与后续批次见 `docs/archive/backend-v24-plan.md` §6（v24 收口后归档）。

### Docs (2026-10-11) — v23 方案收口归档

+ `docs/plans/backend-v23-plan.md` → **`docs/archive/backend-v23-plan.md`**（头部加
  `⚠️ ARCHIVED（2026-10-11）` + 现行替代入口；状态由 `[progress]` 改 `[archive]`。
  归档理由：M1–M6 与追加的 M7 **全部收口**）。
+ 全量更新引用与索引：`CLAUDE.md`（plans 段 → archive 段）、`docs/README.md`（plans 表 +
  reference 行）、`docs/archive/README.md`（归档清单加一行）、
  `docs/reference/backend-pipeline.md`（两处路径）；`git grep 'plans/backend-v23-plan'` 已清零。
+ **现行入口**：管线契约 = `docs/reference/backend-pipeline.md`；逐批变更 = 本文件（搜 `v23`）。

### Reviewed (2026-10-11) — 后端 v23 收尾 M7 批次 7：末项「`>16B` 非 sret 栈返回」**前提有误，无消费者**

三条独立证据（都不需要改代码）：

+ 四份内置约定的 `ret_classify` 把大聚合返回**全部**判成 `indirect = { via = "hidden_sret" }`
  （win64 `>8B`、sysv64/aapcs64/lp64d `>16B`）——没有一份写 `do = { stack = {} }`；
+ `RetLoc` **没有栈变体**（`Reg`/`RegPair`/`RegGroup`/`Indirect`/`Void`）⇒ 写了 `stack` 也无处落地；
+ `engine.rs` 返回位的 `ClassAction::Stack` 臂**也走 sret 指针**（"栈上返回需要约定声明
  `hidden.sret_pool`"）⇒ 模型里不存在"非 sret 的栈返回"。

⇒ 这一项**不实现**。发射侧的 `ArgPlace::StackByVal` 说的是**实参**（sysv64 `>16B` 实参按值进
传出区，**已接**）。守卫 `wide_return_uses_each_conventions_own_sret_slot`（forge-abi）已把
"四份约定的大聚合返回都是 `RetLoc::Indirect`"与这三条理由一起钉住。

**M7 收口**：批次 1–6 落地（数据面 + 消费侧 + 调用方 + 四个缺陷修复 + 真执行用例通过），
批次 7 以"前提有误"结案。

### Fixed (2026-10-11) — 后端 v23 收尾 M7 批次 6：**`va_arg` 聚合真执行打通**（根因：改写 `extractvalue` 时 use-before-def）

**真凶**：`rewrite_agg_value_uses` 的 **by-ref（值在内存）`ExtractValue` 分支**用 `make_inst`
生成 `Iconst`/`Iadd`/`Load`——`make_inst` 是**追加到块尾**的——而把 `extractvalue` 就地改成
`Copy(load结果)` 后**留在原位置** ⇒ `Copy` 排在自己操作数的定义**之前**，即 **use before def**。
机器码层表现为分配器读到**陈旧寄存器**：

```text
StoreMemR { R14 → RDI }        ← 段1 store（R14 = 段数据 7、RDI = 槽+8 地址）
MovRRm { dst: R12, src: R14 }  ← iadd 取到"段数据"而不是字段0
AddRmR { src: RDI, dst: R12 }  ← 再加一个**地址** ⇒ 垃圾（随栈地址变 ⇒ 每次运行都不同）
MovRMem { dst: RDI, src: RBX } ← 字段1 load（排在 add 之后）
MovRMem { dst: R14, src: RSI } ← 字段0 load（排在 add 之后）
```

**为什么难查**：**单独回读任一字段都是对的**（那条路径上 `Copy` 恰好在最后），只有"两个字段
一起用"才暴露；值每次运行都不同（像读未初始化内存），一度被误判为指针/内联口径、ABI 槽重叠、
溢出、字段 load 早于段 store（四项都被逐条实测否掉，见批次 4/5 条目）。

**修法**：生成的三条指令搬到使用点之前 —— `move_to_front(func, block, pos, &[ci, add, ld])`
（同族的分支早就这么做，只有这条 by-ref 分支漏了）。

**结果**：真执行用例 `test_jit_va_arg_reads_unnamed_aggregate_arg` 通过（`f(Agg16{4,7}) == 11`），
**删掉 `#[ignore]`**，临时诊断用例删除；`forge-codegen --lib --all-features` **2821 passed / 0 ignored**。

**这一条用例（连同本片）抓出并修掉四个缺陷**：① 上述 use-before-def（真凶）；② by-ref 分支
`return Ok(())` 跳出整个循环 ⇒ 第 2+ 个使用点没被重写；③ `trace_enabled` 白名单漏
`FORGE_TRACE_AGG` ⇒ 诊断开关静默失效；④ 驱动次序（形参 pass 必须早于实参，"大聚合 load 值
传播"必须晚于它 ⇒ 拆成 `agg-param` + `agg-leftover`）。

### Changed (2026-10-11) — 后端 v23 收尾 M7 批次 5：**ABI 槽改为在聚合族之后排布**（修一处帧槽重叠隐患）

+ **隐患**：`plan_abi_slots`（变参保存区 + `va_list` 对象的槽排布）原来在 Stage 0.5 **之前**
  跑，用**合法化之前**的 IR 扫深度；而 lowering 扫的是**合法化之后**的 IR。聚合族 pass 会
  **新增 `Alloca`**（聚合槽）⇒ 局部区变大 ⇒ 新槽可能落进 ABI 槽区（被游标推进/序言保存覆盖）。
  这不只影响 `va_arg` 聚合，而是**所有**合法化新建的槽（agg-store/agg-call/agg-param/
  insertvalue 链）的共性隐患。
+ **修法**：驱动拆成可分批的形态 —— `with_aggregates_except_va()`（聚合族）与 `va_only()`；
  入口次序改为 **① 聚合族 → ② `plan_abi_slots`（此时含新建的 `Alloca`）→ ③ va**；
  `ChangeSet::merge` 把两批的变化量合起来报。守卫
  `driver_has_the_documented_pass_order` 同时钉住"合并清单"与"分批两份拼起来 = 合并清单"。
+ **新增守卫**：`va_arg` 聚合单测里断言"字段 load 必须晚于段 store"（"先拷贝后读"）。
  它**否掉**了一个怀疑：单测路径下次序是对的（字段 load 确实在段 store 之后）。
+ **未解决（范围进一步收窄）**：真执行用例的和仍是垃圾且**每次运行不同**
  （实测 `344364925599`、`1028139237199`）——但**两个字段各自单独回读分别正确**（`e0 = 4`、
  `e1 = 7`，两次独立运行）。已排除：① 指针/内联二选一（两种读法都错，且值不同）；
  ② ABI 槽重叠（本批已修，症状不变）；③ 溢出（`FORGE_TRACE_SPILL` 无输出）；
  ④ 字段 load 早于段 store（单测序列实测不是）。下一步方向：对照"单字段版（对）"与"两字段
  相加版（错）"的 `FORGE_TRACE_EMIT` 序列差异，以及调用方副本槽与 `iadd` 两侧操作数的寄存器分配。

### Changed (2026-10-11) — 后端 v23 收尾 M7 批次 4：调用方聚合实参**不再 fail-closed**（两处修复）；投递仍未对（记档）

+ **驱动定序改成两条方向相反的约束都能满足**：`agg-param` 提到实参之前（形参槽必须先立起来，
  否则 by-ref 聚合形参转发时实参拆段看不到地址），同时把原来的 `agg-param` 拆出新 pass
  `agg-leftover`（大聚合 load 值传播）挪到最后——它靠"前面 pass 已收拾干净"判断没剩下东西，
  排在实参之前会报 `call 聚合实参：值既不在 slot 映射里、也没有段值`（本批实测撞到才拆的）。
  新次序：`va → agg-param → agg-store → agg-call → agg-leftover → gep`。
+ **实参拆段支持"没有可追踪地址"的来源**：`insertvalue`/`undef`/`copy` 链搭出来的聚合值
  先**内存化**（`rewrite_agg_value_uses` 开槽 + 搬字节 + 登记）再逐段 load；模式守卫让
  `ValueDef::Inst(..)` 只对 `Load` 走原来的地址追踪路径（不复制逐段 load 的代码）。
+ **效果**：`main(agg: {i64,i64})` 把**按引用形参**转发成未命名实参——以前 fail-closed
  （`聚合实参拆段：值来源不支持（def Param(Block(0), 0)）`），现在模块能编、能跑。
+ **仍不对（如实记档）**：它跑出来的不是 11。两个反证把范围钉死在"调用方对变参聚合实参的
  **投递**"上：读指针档（psABI 口径，`aggregate_indirect = true`）得 `45478821343`；
  内联档（把它当字节读的反证实验）得 `1755035184008`。两个都不对 ⇒ 不是"指针还是内联"的
  二选一，而是**写进去的槽与游标指的槽没对上**（或传出区根本没写）。
  真执行用例 `test_jit_va_arg_reads_unnamed_aggregate_arg`（断言 = 11）标 `#[ignore]` 并写明
  两条反证与下一步（对照该调用的 `AbiPlan` 未命名实参落点与 `main` 的传出区布局逐步核对偏移）；
  修好后删掉标记即成为验收。

### Changed (2026-10-11) — 后端 v23 收尾 M7 批次 2/3：聚合变参**按尺寸分档**（含内联档）、修两个真缺陷

+ **内联档（≤阈值）**：`va_arg` 的聚合形态由 `use_pointer = indirect && (阈值 == 0 || size > 阈值)`
  决定——win64 的 `≤8B` 是"内联整数槽"、`>8B` 才是"指针"。内联档就是"区域里直接放聚合字节"，
  逐 8 字节段拷进来即可（**不**多读一次指针）。守卫按三档钉住：无规则 ⇒ 报错点名；16B ⇒
  4 次 Load（游标 1 + 指针 1 + 段 2 + 字段 1 = 5，见下）；8B ⇒ 3 次（**不读指针**）。
+ **修 `rewrite_agg_value_uses` 的"跳出整个循环"**（真缺陷）：by-ref（值在内存）分支的
  ExtractValue 处理里写的是 `return Ok(())` ⇒ **同一聚合值的第 2 个及以后的使用点全被漏掉**，
  漏掉的那些 `ExtractValue` 掉到 lowering ⇒ `Unsupported("DSL lowering")`。单个使用点时代
  这条路径看着是对的，所以一直没暴露；`va_arg(聚合)` 一次取两个字段才现形。改成 `continue`。
+ **修 `trace_enabled` 白名单漏 `FORGE_TRACE_AGG`**（真缺陷）：`FORGE_TRACE_AGG` 被方案、
  `legalize/*` 与 M3 的 `ChangeSet` 报告多处承诺"可见"，但它**不在 `TRACE_KEYS` 白名单里**
  ⇒ 所有相关 `eprintln` 是死代码、开关静默失效（本轮排查一度被它误导）。已加进白名单。
+ **调用方一半未实现（如实记录 + 断言限制）**：被调方 `va_arg(聚合)` 已完成（单测覆盖两档与
  多使用点）；但调用方把聚合值作为**未命名实参**传下去，两条现成来源都被 `expand_agg_call_args`
  明确拒绝（fail-closed，不是静默错值）：`insertvalue` 链 ⇒ `聚合实参拆段：值来源不支持`；
  按引用**形参**转发 ⇒ `聚合实参拆段：值来源不支持（def Param(Block(0), 0)）`。
  用例 `test_jit_va_arg_unnamed_aggregate_arg_is_known_limited` 断言该限制仍存在，
  一旦调用方落地必须改成真执行断言（期望值 11 已写在注释里）。

### Changed (2026-10-11) — 后端 v23 收尾：**变参聚合取参**（`va_arg` 聚合）打通一条路

+ **数据面**：`VaArgRules` 增 `aggregate: Option<VaAggregateArgRule>`（规则 + `indirect`）；`va_list` 形状声明增两个可选键 `aggregate_arg` / `aggregate_indirect`（`#[serde(default)]`，老形状与快照零改动）；`engine` 用同一个 `rule(...)` 构造器造它。
+ **内置声明**：win64 的 `win64_stack` 声明 `aggregate_arg`（游标同整数类）+ `indirect = true`——psABI 的"聚合 >8B ⇒ 调用方副本 + 指针"，所以变参区里放的是**指针**。sysv64/aapcs64/lp64d **不声明**（它们的聚合变参依赖尚未接通的调用方 `StackByVal`）。
+ **消费侧**（`va_expand::expand_arg`）：聚合走自己的那条规则；`indirect` ⇒ 先 `Load(ptr)`；然后逐 8 字节段拷进 `place_addr_of_slot(size)` 开的**自己的**帧槽，结果值登记进 `agg_slots`、原指令墓碑化、递归 `rewrite_agg_value_uses`（后续 `extractvalue` 走地址语义）。
+ **fail-closed 的判据变了但没有消失**：以前对聚合**无条件**拒绝；现在数据没声明才拒绝，且错误信息点名 `aggregate_arg`。守卫 `aggregate_va_arg_needs_a_declared_rule` 钉两条路（没声明 ⇒ 报错且点名；声明了 ⇒ 成功且**生成 Store**，只登记不写就是读未初始化内存）。
+ 顺带删掉 `expand_arg` 那个从没用过的 `init` 形参（`let _ = init;` 一直写在函数体第一行）。

### Reviewed (2026-10-11) — 后端 v23 收尾：聚合槽"按段整写"查证为非缺陷（+ 两条守卫）

+ **查证**：合法化按 `ceil(size/8)*8` 字节把聚合往 `Alloca` 槽里整段搬，而 `frame_slots` 按
  `Type(I8) × count` 精确算槽字节数——看着像越界写。实测**不是**：槽的帧占用按槽单位向上取整
  （`frame_slots::depth_bytes` 与 `lowering.rs` 分配偏移处是同一算式 `((b + u - 1)/u)*u`），
  槽单位 = 8 时"写宽 == 占宽"。
+ **新守卫**：`alloca_slot_footprint_rounds_up_to_slot_unit`（取整 + 逐尺寸断言相等）、
  `shipped_isas_slot_unit_is_at_least_eight`（三份发行谱槽单位 ≥ 8；槽单位 < 8 时合法化需给末段加掩码）。

### Changed (2026-10-11) — 后端 v23 M6：**合法化单一驱动 + 去重 + 缺口泛化**（里程碑收口）

**单一驱动**：新 `pipeline/legalize/` —— `trait LegalizePass`（`name()` + `run() -> ChangeSet`）、
`LegalizeCtx`（机器视图/约定名/`agg_slots`/`sret_ptr`/变参输入）、`LegalizeDriver`（定序、变化报告、
幂等）。8 个 `expand_*` 收成**五个 pass**：`va → agg-store → agg-call → agg-param → gep`
（`rg 'fn expand_' pipeline/compiler.rs` = **0**，验收要求 ≤3）；调用点只剩一行 `driver.run(...)`。

**函数体与共享件迁出**：`expand_*` 搬进 `legalize/{agg_call,agg_store,agg_param,gep}.rs`；
`AggArgLoc`/`agg_arg_place`/`agg_ret_is_sret`/`append_param`/`collect_extract_jobs`/
`make_placeholder_ptr` 收进 `legalize/common.rs` ⇒ **`legalize` 不再反向依赖 `compiler`**。
`read_path_budget` 随迁移下调：`compiler.rs` 20 → 13 → 4 → **3**，并登记 legalize 侧实测值。

**去重（R6）**：`agg_const::pack_segment`（4 处内联打包 → 1，且统一了"越界补零"口径）；
`legalize::common::place_addr_of_slot`（`Alloca(Type(I8), Uint(n))` 规范形态的**唯一出口**，
3 处内联 → 1）；`move_to_front` 复核为"1 定义 + 3 调用"，无内联副本。

**缺口泛化**：① `va_expand::int_ty` 非 1/2/4/8 宽度改 **fail-closed**（以前静默折 I64 ⇒ 游标与
相邻字段互相踩）；② **非对齐嵌套字段**（`off % 8 != 0`）从 `Unsupported` 改为**能展开**
（段值按 `off % 8` 移位拼接，`hi` 严格按"是否落在字段内"取，不读相邻成员字节）；③ **`insertvalue`
值内存化**（分配自己的帧槽、写字段、结果登记进 `agg_slots` 并递归重写使用处）；④ `va_arg`
**聚合结果改 fail-closed**（以前按标量 load ⇒ 静默错值；真做对需要约定层新增"聚合取参规则"）。

**新守卫**：`driver_has_the_documented_pass_order`、`legalization_is_idempotent`（第二遍
`ChangeSet::total() == 0` 且指令序列不变）、`unaligned_nested_field_is_repacked_with_shifts`、
`insertvalue_is_memoryized`、`cursor_field_width_is_fail_closed`、`aggregate_va_arg_is_fail_closed`。

**未达成（如实记录，留给约定数据的后续里程碑）**：① `va_arg` 聚合**真做**（要 `va.arg_rules`
的聚合条目 + 真执行用例；当前已 fail-closed，不再静默错值）；② `>16B` 非 sret 栈返回走
`StackByVal`（发射侧 `ArgPlace::StackByVal` 已有，缺"返回侧也走它"）。

### Changed (2026-10-11) — 后端 v23 M5：**替掉 `EPILOGUE` 哨兵**（机器标签改为分配）；M5 六条全部落地

+ **机器标签由 `CodeSink::new_label()` 分配**：新增 `LabelRef::Machine(u32)`，id 落在
  [`LabelRef::MACHINE_LABEL_BASE`] 之上的独立区间，与 IR 块 id 天然分离 ⇒ "要几个有几个"，
  不再有哨兵常量、也不再需要"块数不撞保留区间"的空间 `debug_assert`。
  跳点/绑定方（`emission.rs`）改为 `let epilogue_label = sink.new_label()` → 各 return 块跳它 →
  所有块之后 `sink.bind_label(epilogue_label)`。
+ `ExternalLabel` 类型与 `LabelRef::EPILOGUE` 常量删除（`lib.rs` 的再导出同步）。
  `needs_epilogue_label()` **保留**——它问的是"本 ISA 是否用共享尾声（而非每 return 内联）"，
  是正当的能力判据，与"哨兵"无关。
+ **新守卫**（运行时单测）：`machine_labels_are_allocated_uniquely_and_round_trip`
  （两次分配必不同、id 落独立区间且与 0..4096 块 id 全不相撞、id↔标签闭环）与
  `forward_machine_label_reference_is_resolved`（先引用后绑定必须打上补丁——正是删掉发射
  次序 hack 后依赖的性质）。

### Changed (2026-10-11) — 后端 v23 M5：删掉"发射次序 hack"；重定位补丁语义单点化

+ **删掉"非返回块先发、返回块最后"的重排**（`emission.rs` 的 Pass1/Pass2）：当年重排的理由是"返回块的
  `emit_epilogue_jump` 会干扰前向 PC-relative 分支偏移计算"——那是**编码器急切算偏移**时代的遗留。
  现在 `emit_epilogue_jump` 把目标写成**标签号**，由 `CodeSink` 记 pending fixup、`bind_label`/`finish`
  时统一打补丁，前向引用天然成立。块现在按 vcode 自然次序发射。
  **回归判据（方案指定的三条硬回归）**：`jit_matrix_x86` **197/3/0**（前向跳转）、
  `jit_matrix_riscv64` **136/64/0**（分支范围）、`asm_exec` **1 passed**；`forge-codegen --lib
  --all-features` 2810、workspace 门禁 exit 0。
+ **重定位补丁语义单点化**：新增 `RelocKind::patch_value(target, site)`（`Relative` ⇒
  `target - site + adj`、`Absolute` ⇒ `target`），JIT 里那份私有副本 `compute_patch_value` 删除并改调它。
  同一件事以前在 JIT / ObjectWriter 两处各表达一次，改一处容易漂移。

### Changed (2026-10-11) — 后端 v23 M5：**真正的 Belady 驱逐**（不再只驱逐死值）+ 消除"活跃值落盘"这条错误路径

**背景**：旧实现只驱逐**死值**（`next_use == MAX`），无死值就 spill 当前 def 或直接报错（WA-46 的
"3 scratch"已知限制就是这么来的）。

+ **驱逐策略改为真 Belady**：候选人按 `next_use_after` 取"下次使用最远"者，死值优先；候选人**排除
  当前 vreg 自身、本指令的 use 与本指令的 def**（占走自己马上要读/写的寄存器 = 弄丢输入）。
+ **关键取证：活跃值驱逐根本不需要"落盘"**。发射侧按**最终表**决定溢出搬运——凡"最终有槽"的 vreg，
  其每次 def 都被重定向到 scratch 并 `Store` 回槽、每次 use 都**从槽 load** ⇒ **槽始终是权威副本**，
  寄存器的内容可能早已陈旧（def 走 scratch 路径时寄存器根本没被写）。早先版本在驱逐点"把受害者的
  寄存器内容写回槽"（`SpillEdit::SpillLive`），实测是**用陈旧值覆盖槽里的正确值**：atomic 用例读到
  `-1774193535`（期望 50）、V256 用例读到 `-163`（期望 -153）。所以驱逐活跃值只需**释放寄存器**
  （`spill_vreg` 分配槽并归还寄存器）——`SpillEdit::SpillLive` 与配套的 `live_stores` 全套随之删除
  （不留死代码）。
+ **`spill_vreg` 的"已有槽就早退"补齐释放寄存器**：活跃值被驱逐 → 之后 reload 拿到寄存器 →
  **再次**被驱逐时命中早退分支，寄存器既不归还也不清 `assignments`/`reg_owner`，而调用方仍把那个
  寄存器交给当前 vreg ⇒ 两个活值共用一个寄存器。死值驱逐永远走不到这条（死值不会再被 reload），
  所以旧策略一直掩盖着它——把诊断探针插在这里跑**全量门禁一次都没触发**，正是这个原因。
+ **用例**：原"已知限制"用例改写为**正例** `test_jit_v256_high_pressure_spill_evicts_live_values`
  （17 个 V256 值 > 16 个架构寄存器 ⇒ 必须驱逐活跃值；断言**编得过且算得对** = -153），它同时是
  "槽是权威副本"的守卫。

### Changed (2026-10-11) — 后端 v23 M5：**C4 收口** —— 根因是「scratch 的号被 FPR 池复用」，reload 踩掉活值

+ **根因（先排除猜测，再定位）**：编码级先排掉"高编号 FPR 缺 REX"的假设（`movsd [rbx], xmm10`
  = `f2 44 0f 11 13`、`xmm15` = `f2 44 0f 11 3b`，REX.R 都在）。真因在**溢出搬运的 scratch**：
  值装进"号取自 scratch 池、类取值的类"的寄存器，而 x86 的 scratch 是 **R10/R11** ⇒ 浮点值的
  reload 实际写的是 **XMM10/XMM11**；若 FPR 池能把**活值**分到这两个号上，reload 就**踩掉活值**。
  这正好解释上轮 trace：类感知偏好把栈上浮点形参分到 f10/f11 后 `e+f` 由 7 变 8；旧"只比号"偏好
  恰好从不把 FPR 分到 10/11（win64 的 RSI=6/RDI=7 让 XMM6/XMM7 总被优先选中），bug 一直被掩盖。
+ **修法**：① 池构造改为**scratch 的号在任何类里都保留**（号是跨类共享资源）；② callee-saved
  优先改为**按 (类, 号)** 匹配（原来只比号，是巧合而非语义）。两处都正之后
  `test_jit_float_params_beyond_xmm_registers_come_from_the_stack` 通过 —— C4 的类感知偏好
  正式启用。
+ **新守卫** `scratch_reg_numbers_are_never_allocated`：把 scratch 号放进池内，断言分配器绝不把它
  分给任何 vreg（这条不变量以前没有用例钉住）。测试 `make_config` 的 scratch 号由 0 改为 15
  （测试池是 0..8），否则新不变量会挡住 `test_no_clobber_allocates_rax` 要检查的 RAX=0。

### Changed (2026-10-11) — 后端 v23 M5：**溢出编辑显式化**（分配器产出 `SpillEdit`，发射退化为按序执行）

+ **新类型** `SpillEdit { Reload{slot,scratch,class,width}, Bind{field,scratch,class}, Store{slot,scratch,class,width} }` + `AllocResult::edits: Vec<Vec<SpillEdit>>`（下标 = 指令全局序号）。顺序契约：同一条指令里 `Reload` 在前、`Bind` 居中、`Store` 在后 ⇒ 发射侧只做"装值 → 改写字段 → 发指令 → 写回槽"四段。
+ **分配器侧新增 `build_spill_edits`**：逐指令去重收集被溢出的 vreg（同一 vreg 的 use/def 共用一个 scratch），产出编辑。两项判据**从发射期前移到分配期**：① scratch 池不够 ⇒ 分配期 fail-closed（点名第几条指令与池大小）；② 被溢出值落在**不可改写字段** ⇒ 分配期 fail-closed（指令在手，正是 WA-40 那道守卫的上游入口）。没被溢出的绑定不产生编辑——它的字段已由管线按 `inst_ops` 统一回写过。
+ **发射侧删繁**：`emit_inst_with_spills` 去掉 scratch 池索引与绑定表两个参数（改收该指令的编辑切片），删掉 scratch 选择、`overrides`/`xreg_preg` 覆盖表、spilled 去重收集与 `settable` 预检；`sp_base` 仍在发射侧算（它依赖 `frame_size`/callee-saved/栈参数这些**发射期**事实）。
+ **过程中被 mini_c 的 e2e 抓住一处真分歧**：第一版把 `Bind` 的类写成 `vreg.class()`（池宽类），而旧实现用的是 `bind.class.unwrap_or_else(|| vreg.class())`——M4 折进来的**值宽类**。多类槽靠它定 opsize，写成池宽类正是 WA-35 的老病，`test_hir_e2e_loop_bound_from_param` 当场返回垃圾值（2750685 vs 45）。改为与旧实现同源后 mini_c 22 passed。
+ `RegAllocConfig::new` 保持**空** scratch 池（有值即写死类，会撞"宿主生产代码不得出现写死寄存器类"守卫）；需要走溢出编辑的测试在**测试模块内**自行塞一个（`regalloc_bt.rs` 的 `make_config`）。

### Changed (2026-10-11) — 后端 v23 M5：溢出搬运改按**寄存器类**分派（`is_fp: bool` → `RegClass`），掩码类显式 fail-closed

+ `TargetFrameLowering::emit_spill_load/store` 的 `is_fp: bool` → **`class: RegClass`**。布尔只分得出"是不是浮点文件"：FPR/VEC 与 **KReg** 在同一个 `false` 上撞车，掩码值会**静默**按通用寄存器搬运（发一条 mov，读回来的东西与掩码寄存器无关）。现在：FPR/VEC ⇒ 原有按宽度档的浮点模板（WA-46 分派不变）、GPR ⇒ 通用模板（逐字节同前）、**KReg ⇒ 明确 `Unsupported`**（本 ISA 没有掩码溢出模板时不假装能搬）。
+ **`class` 与 `width` 不重叠，两者都保留**（核到代码层）：生成物里 `width` 是**运行期分派**（`width <= 值池` / `== 16/32/64` 各取一档模板），而 `class` 决定**指令族**。`class.bytes()` 是寄存器**池**宽、`width` 是**值**宽 —— 同一个 `FPR(8)` 池的寄存器可装 f32(4B) 或 f64(8B)、同一个 `GPR(8)` 池的可装 i32(4B) 或 i64(8B)，因此不能互相替代。该理由（含反例）写进了 trait 文档。
+ **顺手修掉一处调用点不一致**：`emit_spill_store` 原来传 `preg.class`（分配到的**池类**）却用 vreg 的 `bytes()` 当宽度——两者不同源，宽度档与类会各说各话。现在两个调用点统一为 **`xreg.class()` + `xreg.bytes()`**（值的类 + 值的宽），行为不变但语义自洽。
+ **新守卫两条**：`test_mask_class_spill_is_fail_closed`（KReg 的 load/store 都必须报错、错误信息点名掩码类与 spill）、`spill_width_and_class_are_independent`（同族换宽度要换档，且不因"值只有 4 字节"退成通用寄存器搬运）。

### Changed (2026-10-11) — 后端 v23 M5：删掉「预着色」整条死线（C5 的「接通或删 ⇒ 选删」）

+ `TargetRegInfo::precolored_xregs()` 只有默认空实现、**全仓无实现者**；`RegAllocConfig.precolored` 由它填出来却**没有任何读取点**，唯一"存在感"是 `alloc_config` 的一条 `assert!(cfg.precolored.is_empty())`。三个站点（trait 方法 / 配置字段 / `compiler.rs` 的构造与传递）整条删除，连带两个不再使用的 import。
+ `WORKAROUNDS.md` 的 WA-40 ④ 同步更正：该条原本把它记为"防线是死的"——现在写明**已删除**，并把真正的防线指到 `is_reg_field_settable` + regalloc 的 fail-closed。

### Fixed (2026-10-11) — 后端 v23 M5：修掉上一批发浮点栈形参收参时的**重复发射**，并删掉三个与 `param_vregs` 平行的可推导数组

+ **重复发射（我上一批引入的缺陷）**：栈上浮点形参"直接收进寄存器"那条分支后面**残留了一句**无条件的 `#layout_stack_stmt`——于是浮点形参除了收到 FPR 之外，**还**会多发一条把 `__dest` 当 GPR 用的收参（写坏一个与 FPR 同号的通用寄存器；寄存器分配之后那个 GPR 可能正持有活值）。测试没抓到（被写坏的寄存器在序言那一点通常已死），属于必须靠读代码发现的形态。修法：删掉那句残留，收参严格二选一。
+ **删三个平行数组**：`AllocResult::param_is_float` / `param_by_ref` / `param_is_32` —— 三个都**从 vreg 自身可推**（`v.class().is_fp()`、`is_fp() && v.bytes() > 16`、`v.bytes() == 4`），其中 `param_is_32` 连读点都没有（纯死数组）。生成的序言改为直接算（5 处读点），`abi_target_real.rs` 的两个断言按同一口径现算，`regalloc` 的三段填充随之删除。保留 `param_vregs` 与 `param_bytes`（后者是**真数据**：IR 类型字节数不在 vreg 里，by-ref 宽向量收参靠它分派 32B/64B）。
+ **守卫锚点同步**：`call_layout_emission.rs` 原来用 `param_by_ref` 当"回退段"的锚点做顺序断言；该数组删除后换成**回退段自己的 fail-closed 文案**（`收参需要调用布局`）——实测三份发行谱里都在布局路径之后，比原来的锚点更贴近它要钉的东西。

### Changed (2026-10-11) — 后端 v23 M5：**分配结果密集化**（`HashMap` → 按下标直取的 `Vec`，热路径省掉哈希）

+ `AllocResult::assignments: HashMap<XReg, PReg>` → **`Vec<Option<PReg>>`**（下标 = `XReg::index()`），`spill_slots: HashMap<XReg, SpillSlot>` → **`Vec<Option<SpillSlot>>`**。发射与序言**每条指令每个操作数**都要查一次（`resolve`/`preg`/`is_spilled`/`spill_slot`），密集表把查询降成一次边界检查。
+ **语义不变、读侧统一走访问器**：`preg`/`is_spilled`/`spill_slot` 实现改密集查；新增 `has_preg`（生成的序言原先直接 `assignments.contains_key(&pv)`，现在读访问器，**生成物与 `AllocResult` 的字段布局解耦**）；越界/空槽在 `spill_slot` 里显式 panic（编程错误，不再静默取到别的槽）。
+ 分配器**内部**仍用两张 `HashMap` 做工作集（插入/删除频繁），只在出口**一次性**折成密集表（`n = max(vreg index)+1`）；两张表由 `insert`/`insert_spill` 保证**同长**（长度不一致会让越界查询把"没有该状态"误读成另一种状态）。
+ 连带改动：生成的序言 3 处（`__rm.is_spilled(__pv)` ×2、`__rm.has_preg(__pv)`）、`alloc_result.rs` 的测试与 `vreg_class`、`compiler.rs`/`regalloc_bt.rs` 里直接读字段的测试。

### Changed (2026-10-11) — 后端 v23 M5：**删掉算了没人用的 spill weight**，活区间与每块 uses/defs **合成一趟**，并把 spill 启发式的文档改成与实现一致

+ **删 `LiveInterval::weight` 及其整条计算链**（字段 + 初始化 + 每操作数一次 `weight += w` + 循环权重常量 + 活区间 dump 的那一列），连带删掉只为它服务的**循环检测预扫**（每指令扫分支目标、标记回边区间）。按方案"`weight` 要么参与驱逐、要么删 ⇒ **选实现**"：全仓没有任何决策读它（只在 `FORGE_TRACE_LIVE` 里打印）。这是 regalloc 热路径（占 codegen 58–76%）上的纯浪费。
+ **活区间与"每块局部 uses/defs"合成一趟**：原先两趟各扫一遍全部块 × 指令 × 操作数（第二趟还多一个与第一趟平行的 `global_inst_map` 计数器）；现在同一趟里一起填，删掉第二趟与其计数器。
+ **把 spill 启发式的文档改成与实现一致**（方案的硬要求）：实际策略是**只驱逐死值**（`next_use == MAX`，按 `(next_use, vreg index)` 确定性取最大），无死值时若当前 vreg 是 **def** 就 spill 自己，是 **use** 就明确报错。**完整 Belady（取最远的活跃下一次使用）与 spill cost 驱逐做不到**，因为 `spill_vreg` 只分配槽、不把寄存器当前值写进槽——驱逐活跃值会让它下次 use 从空槽读垃圾。要实现它必须先有 **`SpillEdit`**（分配器产出"把活跃值 store 回槽"的显式编辑序列）。这段校正写进了 `regalloc_bt.rs` 的模块文档。

### Added (2026-10-11) — 后端 v23 M5：**浮点直接收参分支的唯一直接覆盖**（新守卫用例）

+ `test_jit_stack_float_params_are_received_into_registers`：六个 f64 形参**全参与运算**（配 16 个 XMM 的池 ⇒ 栈上的第 5/6 个形参**拿到寄存器**），断言和值 63。它是上一批新增的**浮点直接收参**分支（`{ role = "stack_arg_load", class = "fpr" }` + `layout_stack_fpr_stmt`）的**唯一直接覆盖**——原有用例在池紧张时走的是溢出槽那条，覆盖不到新分支。
+ 同时记下 C4 类感知偏好的**第二次实测**：改成 `(类族, 号)` 匹配后该用例报 **e+f = 8（期望 7）** —— 8 = 4 + 4，即**第 5 个浮点形参拿到了第 6 个的值**（两者都来自栈）。收参已无条件化、直接收参分支也已有覆盖（当前配置下正确），因此取证方向收窄为**调用方写传出区**（`stack_arg_store` 的 fpr 版）在同一偏好下的编号/偏移，或对两种偏好 dump 机器码逐条对照。写入代码注释与账本 §14。

### Fixed (2026-10-11) — 后端 v23 M5：**修掉"条件式收参"这个静默错值缺陷**，并据此落地 C3（FPR 池释放 2 个寄存器）

+ **缺陷本体**：被调方的**栈上形参**收参是**条件式**的——只有该形参 vreg **没有**拿到寄存器（`spill_slots` 里有它）时才走"load 布局槽 → GPR scratch → store 溢出槽"。于是**池一有余量**（分配器给栈形参也分了寄存器），这条收参**整条被跳过** ⇒ 值没落地 ⇒ **静默错值**（无编译错误、无异常）。三组对照实验（FPR 池只放 XMM4 过 / 只放 XMM5 过 / 两个都放红）证明问题不在"某个寄存器号"，而在那条收参。
+ **修法**：收参改为**由分配结果驱动**——拿到寄存器 ⇒ 直接收进那个寄存器（整数走 `stack_arg_load`，**浮点走新增的类限定角色** `{ role = "stack_arg_load", class = "fpr" }`，x86 落在 `MOVSD_RM`，与既有的浮点 `stack_arg_store`（`MOVSD_MR`）对称）；没拿到 ⇒ 仍走原来的按位搬运进溢出槽。两条路都不再依赖"池里有没有余量"。
+ **C3 随之落地**：池过滤只对**整数类**排除 sp/fp —— FPR 池里同号的寄存器不再被误伤（x86 的 XMM4/XMM5、riscv 的 f2/f8、arm64 的 V29/V31），三个发行 ISA 各释放 2 个可用寄存器。
+ **C4 的"类感知偏好"仍挡住**（复测）：即使收参已无条件化，把 `pop_free` 的 callee-saved 偏好改成按 `(类族, 号)` 匹配**仍**打红同一条用例 ⇒ 它牵着别的东西，与 C3 不同源。回退到与旧实现逐位等价的口径并记录（表已是 `(类, 号)`，将来只改这一处过滤）。
+ **夹具同步**：`stack_args_capability_is_declared_by_roles` 的字符串手术改为 `strip_role`（删某角色的**全部声明形态**——类限定形态出现后，只删平铺形态的夹具会留下"声明了角色却找不到指令"的假缺口）。

### Changed (2026-10-11) — 后端 v23 M5：**帧记账单点（C6）+ callee-saved 单一表（C4）**；C3 与"C4 类感知偏好"按实测回退

+ **C6 帧记账单点**：`LowerCtx` 增 `min_frame` —— 与既有的 `callee_saved_bytes`/`stack_slot_shift` **同一时刻、同一输入**由 `frame_layout_info` 算出（lowering 之前那一处），此后全流程只读这三个字段。删两处重算：`emission.rs` 的 spill 基点（必须与序言实际推入量同源）改读 `ctx.callee_saved_bytes`；`calculate_frame_size` 改读 `ctx.min_frame`。助手 `frame_layout::callee_saved_bytes` 随之删除。两处**同形但输入不同**的调用（聚合落点探针 plan、`va_info_for_expansion` 的 plan）保持不动并在注释里写明原因。
+ **C4 callee-saved 两张平行表合成一张 `Vec<(RegClass, u32)>`**：由 plan 的 `callee_saved.regs` 直接投影（类随号走）；`callee_saved_fpr` 字段与 `alloc_config` 里多余的初始化一并删除；保存集扫描归一成"按类查同一张表"（GPR 侧语义不变；FPR 侧仍保留"只存实际分配到的"这道 arm64 QEMU 通道的闸门）。
+ **C4 的"类感知偏好"（真正的缺陷修复）试过、按实测回退**：`pop_free` 的"优先用 callee-saved"确实是 `contains(&p.num)` —— **只比号不比类**（arm64 的 V8 与 X8 同号会互相误判）。改成类族匹配后打红 x86 的 `test_jit_float_params_beyond_xmm_registers_come_from_the_stack`（期望 7、实得别的值）。**该用例只在 `--all-features`/workspace 的 feature 统一下才编译进来**（单 crate 默认特性跑不到那 84 个），这是它第一次被抓住的原因——也说明门禁必须包含该配置。现保持与旧实现逐位等价的匹配口径，把"类感知偏好"归到方案自己划的"改策略"那一步（它同 C3 一样会动分配序，需先量清浮点栈参数那条路径对寄存器选择的依赖）。
+ **C3 池过滤：试过、打红、回退（如实记录）**：把 sp/fp 排除改成"只对整数类生效"（好让 FPR 池放出同号的 2 个寄存器，如 x86 的 XMM4/XMM5）同样打红上面那条用例 ⇒ **这两个号的排除是承重的**：它们与"浮点栈参数的搬运路径"共用寄存器。结论写进代码注释与账本 §14。

### Changed (2026-10-11) — 后端 v23 M5（第二批，C4）：**callee-saved 两张平行表合成一张 (类, 号) 表**（顺带修掉类盲偏好）

+ `RegAllocConfig.callee_saved: Vec<u32>` + `callee_saved_fpr: Vec<u32>` → **`callee_saved: Vec<(RegClass, u32)>` 一张表**（由 plan 的 `callee_saved.regs` 直接投影，类随号走）。
+ **修掉真实缺陷**：`pop_free` 的"优先用 callee-saved"以前是 `callee_saved.contains(&p.num)` —— **只比号不比类**。arm64 的 V8 与 X8 同号，于是真正的 FPR callee-saved 可能被漏掉、别类的号又可能被误认成 callee-saved。现在按 `(类, 号)` 精确匹配（这是**行为变更**：FPR 侧从此会正确优先 callee-saved，由两矩阵 + `asm_exec` 判定正确性）。
+ 保存集扫描同样归一：GPR 与 FPR 各扫一遍的逻辑改读同一张表（GPR 侧语义不变——池类恒为主 GPR 类；FPR 侧仍保留"只存实际分配到的"这道历史闸门，注释写明它是 arm64 QEMU 通道的闸门，**不**跟着无条件打开）。
+ 顺带：`alloc_config.rs` 的 `new()` 测试构造器少填一项。

### Changed (2026-10-11) — 后端 v23 M5（第一批，C6）：**帧记账单点**（三数值只算一次，两处重算与一个助手函数删除）

+ `LowerCtx` 增 `min_frame: u32` —— 与既有的 `callee_saved_bytes`/`stack_slot_shift` **同一时刻、同一输入**由 `frame_layout_info` 算出（`compile` 里 lowering 之前那一处），此后全流程只读这三个字段。
+ **删两处重算**：`emission.rs` 的 `callee_saved_bytes(machine, self.ctx.conv.layout())`（spill 基点用它，与序言实际推入量必须同源）改为读 `self.ctx.callee_saved_bytes`；`frame_layout::calculate_frame_size` 的 `frame_layout_info(...).min_frame` 改为读 `self.ctx.min_frame`。
+ **删助手**：`frame_layout::callee_saved_bytes(machine, layout)` 随最后一个调用点消失（它只是 `frame_layout_info(..).callee_saved_bytes` 的转发）。
+ 为什么这也算修 bug 面：同一份输入在两个地方重算，口径一漂移就是"spill 基点 ≠ 序言推入量"，症状是局部/spill 槽覆盖 push 槽 → 静默错值（历史上正是补 `frame_pointer_overhead` 那次修的同类问题）。
+ 未动两处**同形但输入不同**的调用（已如实标注）：`compiler.rs` 的聚合落点探针与 `va_info_for_expansion` 用的是**各自签名的 probe plan**（不是 `conv.layout()`），它们的 `stack_slot_shift` 必须按那份 plan 算。

### Changed (2026-10-11) — 后端 v23 M4（收口）：**操作数模型收敛完成**，`xreg_types` 按实测保留（记为偏差）

+ **收口过程中被自己的守卫拦下两次**（都按实测处理，没有"放宽断言了事"）：
  ① 先写的"多类槽必须带值宽类"断言在 `test_jit_agg_by_value_stack_sysv64` 与 riscv64 通道**打红**——实测确实存在多类槽绑定没有值宽类记录（`xreg_types` 里没有那个 vreg）。核对改动前的两个回填点：它们同样先查 `xreg_types`、查不到退池宽类 ⇒ **这是既有行为，M4 没有改变它**，因此断言过强（等于要求代码具备它从来没有的性质）；降级为**可观测**（`FORGE_TRACE_M4=1` 报出来），并把"存量条目"记进账本。
  ② 上一批的"旁表角色可派生"推断同样是被实测推翻（InOut 字段旁表可为 0 条）——两次都写进了账本而不是悄悄改口。
+ **M4 收口判据**（逐条对照方案）：`uses`/`defs`/`use_constraints`/`def_constraints` 由 `operands()` 派生且生成物不再发射 ✔；旁表改名 `inst_ops` 并类型化（`OperandBind{vreg,field,role,class}`）✔；值宽类的两种推导口径合一 ✔；写回路径只有一个入口（`is_reg_field_settable` + `set_reg_field`）✔；发射侧平行数组清零 ✔。
+ **偏差（实测记录）**：方案写的"`CompileState` 无 `xreg_types`"**未达成**，且判定为**不该达成**——vreg 的**值宽**（I32→`GPR(4)`）是既不在指令形状里、也不在 vreg 的池宽类里的独立事实；它只能由"vreg → IR 类型"这张表承载。现在的状态是：读取面**只有一处**（lowering 之后折进绑定表），消费面（两个回填点）读绑定；要彻底删表必须先让 `XReg` 自己带值宽类（或让每个绑定创建点都拿得到类型），那属于模型改动，不在 M4 范围内。

### Changed (2026-10-11) — 后端 v23 M4（批次 C 第三步）：发射侧**三个平行数组收敛成一个绑定切片**；`set_operand_reg` 按实测删除

+ `emit_inst_with_spills` 的 `inst_xregs`/`inst_field_idx`/`inst_classes` 三个平行切片换成**一个 `binds: &[OperandBind]`** —— 它们本来就是同一张绑定表的三个投影（调用点少铺三次 `collect`，签名少两个参数）。
+ spill 回写补上 **fail-closed**：字段不可改写（固定物理字段）时返回 `IrError::RegAlloc` 而不是静默 no-op（与 regalloc 的 WA-40 守卫同口径；生成物里 `settable` 恒真，故发行 ISA 行为不变）。
+ **删除 `MachineInst::set_operand_reg`**（方案原设想）并写明原因：实测发现两个**序号空间不同源** —— 绑定表只登记绑定到 vreg 的字段，形状表 `operands()` 登记该变体全部 use/def 字段（物理寄存器占位的操作数只在后者）。于是 `set_operand_reg(bind_idx)` 会写到**另一个字段**（`add` 这类 InOut 指令：旁表 1 条、形状表 2 条）。回写统一为 `is_reg_field_settable(field)` + `set_reg_field(field, …)`：字段号来自绑定表、可改写性来自形状表，各自只回答自己知道的事。

### Fixed (2026-10-11) — 后端 v23 M4（批次 C 第二步）：**同一件事的两种类推导口径合一**（并把类折进绑定表）

+ **发现并消除一处口径分叉**：回填微指令字段时，`compiler.rs` 的主回填用 `ctx.reg_class_for(ty)`（ISA 类型映射），而 `emission.rs` 的 spill 回填用 `forge_ir::RegClass::from_type_id(ty)` —— 同一个 vreg 的同一个 IR 类型，两个回填点可能推出**不同的值宽类**。
+ `OperandBind` 增 `class: Option<RegClass>`（`with_class` 构造器）；管线在 lowering 之后**折一次**（`CompileState`：`inst_ops` 逐项从 `ctx.xreg_types` 取类型、用 `ctx.reg_class_for` 推出类写入绑定），两个回填点此后读同一个值（`bind.class` / `inst_classes[i]`），缺省仍退到 vreg 自身类。
+ 连带收口：`emission::emit_inst_with_spills` 的签名里 `xreg_types: &HashMap<XReg, TypeId>` 换成 `inst_classes: &[Option<RegClass>]`（少一个跨字段借用），`ctx.xreg_types` 的**读取面**从两处降到一处（只剩那次折入）。
+ 余项（记在账本 §13）：`xreg_types` 字段本身仍是 lowering 期草稿（~26 个 vreg 分配点写入）；彻底删除需要让这些分配点把类直接记进绑定（或让分配点拿到所属包）。

### Changed (2026-10-11) — 后端 v23 M4（批次 C 第一步）：旁表改名 `inst_ops` 并把角色**类型化**

+ `InstPacket::xreg_map: Vec<SmallVec<[(XReg, u8, bool); 2]>>` → **`inst_ops: Vec<SmallVec<[OperandBind; 2]>>`**，`OperandBind { vreg, field, role: OperandRole }`（新类型带 `new`/`def`/`use_` 构造器）。那个裸 `bool` 与指令形状表里的 `OperandRole` 本是同一件事的两种写法，现在只剩一种；**语义完全不变**（`is_def=true` ⇔ `role == Def`）。
+ 全仓同步：`CompileState.inst_ops`、`liverange`/`regalloc_bt`/`emission` 的三处签名与八处解构、`pipeline/lowering.rs` 的两处 `extend`、测试夹具（`integration_tests.rs`/`regalloc_bt` 两个测试模块）与九处注释里的旧名。`liverange`/`regalloc` 里"这条登记是 def 吗"的判断随之从 `d`/`is_def` 变成 `b.role == OperandRole::Def`。
+ 类型注释里写明**实测依据**（为什么角色不能从形状表派生）：同一 InOut 字段在旁表里可能 0 条（物理寄存器占位不参与映射）或 1 条 `Def`，而形状表里它同时是 `Use` 与 `Def`。

### Changed (2026-10-11) — 后端 v23 M4（批次 C 的判据修正）：**旁表角色不可派生**（实测纠正上一轮推断）

+ 上一轮记录"`is_def` 可由操作数表派生"——本轮加了**哨兵断言**（InOut 字段的旁表条数）后，全量单测立刻把它打红：`e2e_compile_add` 里那条指令的旁表是 `[(XReg(514), 0, false)]`，**InOut 的 dst 字段一条都没有**。原因：`map_reg_field` 只登记**绑定到 vreg** 的槽，物理寄存器占位不参与映射。
+ 结论**更硬一格**：同一 InOut 字段在旁表里可能是 **0 条**（物理槽）或 **1 条 `Def`**（vreg 槽），而形状表里它同时是 `Use` 与 `Def` ⇒ **"删掉 `is_def`、一律按表派生角色"这条路关闭**（会凭空加出 use 项 ⇒ 改 regalloc 活区间口径 = 行为变更）。
+ 守卫改为两条且都保留：① **子集关系**（旁表登记项必须在形状表里有同字段同角色的对应项）；② **哨兵**（InOut 字段旁表条数 ≤ 1，实测契约 0 或 1）——将来谁改旁表语义，这里会响。
+ 账本 §13 记下**修订后的批次 C 路径**：把三张表并成一张 `inst_ops` —— 旁表（vreg → 字段 + **角色**，角色来自 lowering、保留）、`ctx.xreg_types`（vreg → IR 类型 ⇒ 回填宽度类；写点 ~26 处、读点 ~6 处）、指令形状（字段/约束/`settable`/声明类，来自 `operands()`）；`is_def: bool` 顺手升格为 `role: OperandRole`。

### Changed (2026-10-11) — 后端 v23 M4（批次 C 的前置验证）：旁表与操作数表**逐条对齐**已成守卫

+ **`InstPacket::map_reg_field` 的发射点断言**：登记的 `(field, is_def)` 必须能在该指令的 `operands()` 里找到同字段同角色的项——找不到即 panic（并把两条 lowering 的具体位置点出来）。这条断言证明 **`is_def` 可由操作数表派生**，因而旁表能从 `(vreg, field, is_def)` 缩到 `(vreg, field)`。
+ **`CompileState` 编译尾部的全量对拍**（`#[cfg(debug_assertions)]`）：按 vcode 块序逐条指令核对 `xreg_map` 与 `operands()` 的 `(field, role)`（旁表里存的是**虚拟寄存器**、字段里是**占位物理寄存器**，两者本就不等，所以只对拍形状面）。它是"删旁表后行为不变"的判据——现在先当守卫跑起来。
+ 说明：这一批**只加守卫不加删除**，因为删除的前置条件（字段占位携带 vreg 号）还没做：`reg::from_index(0, cls)` 这类占位在生成物的 42 个发射点上，改它才能让 `xreg_map` 真正冗余。两条守卫已把"能删"这件事变成可验证的判据（门禁与两矩阵全绿即成立）。

### Changed (2026-10-11) — 后端 v23 M4（批次 B）：生成物不再发射那 4 个查询方法（改由操作数表派生）

+ **`uses`/`defs`/`use_constraints`/`def_constraints` 从生成物里整条删除**：它们现在由 `MachineInst::operands()` 的**缺省实现**派生（批次 A 已用三谱全部指令变体逐条对拍证明两边等价）。每少发射一份就是少一处"同一事实两处表示"。
+ **回潮守卫**（`forge-isa-dsl/tests/machine_shape_table.rs`）：`TABLE_DRIVEN` 缩到 5 个（`effects`/`operands`/`reg_field`/`set_reg_field`/`is_reg_field_settable`），并新增 `derived_methods_are_not_emitted_any_more` —— 生成物里再出现 `fn uses` 等 4 个方法体即红。
+ **操作数模型守卫升级**（`forge-codegen/src/operand_model_guard.rs`）：批次 A 它是双边对拍；批次 B 之后改为守**派生在全部变体上成立** + 两条**不依赖派生结果**的独立判据——① 顺序契约（`Use` 必须在 `Def` 之前，regalloc 的 use@start/def@end 靠它）；② `is_move()` 交叉判据（`is_move` 由另一张表 `move_arms` 驱动，说搬移就必须恰好 1 个 `Def` 且 ≥1 个 `Use`）。实测覆盖 **x86 1550 / riscv64 624 / arm64 2581 = 4755 个指令变体**。

### Changed (2026-10-11) — 后端 v23 M4（批次 A）：**操作数表先立起来**（七个方法族收敛到一张表，旧接口等价性已对拍）

+ `MachineInst` 新增 **`operands() -> SmallVec<[InstOperand; 4]>`**（v23 M4 的单一操作数模型）：每个操作数 = `{ field, slot, class, constraint, role, settable }`，顺序契约是 **`Use` 在前、`Def` 在后**（regalloc 的"use 在 inst_start、def 在 inst_end"由此保证）。
+ **旧方法族改为由它派生**（`uses`/`defs`/`use_constraints`/`def_constraints`/`reg_field` 全部走缺省实现；生成物暂仍显式发射，作为对拍的另一边）。`set_operand_reg(i, preg, class) -> bool` 成为新的回写入口（`settable` 为假时返回 `false` —— regalloc 对 spilled def 据此 fail-closed，WA-40）。手写实现（`DummyInst`、`FixedFieldInst`）改为只实现 `operands()`。
+ **等价性守卫**（`src/operand_model_guard.rs`，`#[cfg(test)]` 模块，放在文件末尾以免截断"写死类"守卫的扫描）：三份发行谱**全部指令变体**（谱派生枚举器 `all_insts()`）逐条对拍 `uses`/`defs`/`use_constraints`/`def_constraints`/`reg_field`/`is_reg_field_settable` 与操作数表一致——**包括 `def_reuse` → `ReuseInput` 的映射**（x86 双操作数指令的复用约束）。这条守卫是"删旧方法"（批次 B/C）的前提证明。
+ 顺带被"写死寄存器类"守卫拦下一次真错：`set_operand_reg` 最初用 `RegClass::GPR(64)` 兜底缺省类——**改对了实现**（要求调用方给类，与旧 `set_reg_field` 同签名），而不是加白名单。

### Changed (2026-10-11) — 后端 v23 M3（第四批）：**触发判据只剩一处**（7 个 `has_*` → 单一保守判据 + `ChangeSet`）

+ **7 个各扫一种模式的 `has_*`（`has_large_agg_ret`/`has_large_agg_call_result`/`has_large_agg_call_arg`/`has_large_agg_param`/`has_large_agg_load`/`has_agg_mem_access`/`has_any_agg`）整条删除**，换成**一处**判据 `needs_agg_expand` = 含 GEP **或** 提到过任何聚合类型/聚合字面量。新判据是**过近似**的：宁可多克隆一份 `Function`，不可漏跑一个 pass——`has_*` 的失效模式恰恰是"漏一类模式 ⇒ 那个 pass 静默不跑"（历史事故：`has_large_agg_ret` 缺失 ⇒ 大聚合返回的纯 `call`+`extractvalue` 函数展开不到 ⇒ 5 vs 6 错值）。
+ **`ChangeSet`（展开自报改动了什么）**：新增 `agg_expand::{ChangeSet, IrSize}`——`IrSize` 取 IR 规模快照（指令/值/常量），差分量 + sret 标记构成 `ChangeSet`，由管线在展开后算出并写入 `FORGE_TRACE_AGG`。用"IR 变化量"而不是让 8 个 pass 各自返回计数：变化量是**完整**的（将来新增 pass 自动被覆盖，不会再出现"新加一类模式却忘了加判据"）。
+ 守卫：`agg_expand::tests::{needs_agg_expand_is_conservative, change_set_counts_only_growth}`——前者钉"聚合形参必须触发、纯标量函数不得触发"，后者钉差分口径（只算增长，墓碑化不计）。

### Fixed (2026-10-11) — 后端 v23 M3（第三批）：帧账目只认 plan（删「无 plan 按零算」的缺省）

+ `frame_layout_info()` 的 callee-saved 计数**只从 plan 来**：删掉 `unwrap_or(0)`——那是"没有 plan 就按零算"的缺省，少算的 16 字节会让局部/spill 槽与 push 槽重叠，**覆盖调用者保存值**（x86 显式选 sysv64 的 5 vs 7 就是这个量级）。无 plan ⇒ 明确报错（编译入口本就 fail-closed，走到这里说明有路径漏接）。
+ 同时纠正该函数里**已经过期的注释**：它说"fp-outside 仍按谱里声明的表数（x86 的 push 是静态发射）"，但 v20 A4/v21 W9 之后谱面的静态 push 表与 `[emit.prologue]` 模板都已删除，`gen_push_mechanism` 的序言按 `alloc_result.callee_saved_to_save` **运行时循环** push、`cs_imm`/`sp_base` 取运行时长度。现在两条路（fp-inside / fp-outside）都是"运行时按 plan 的整张表"。这是 M3 的 A6 收口项。
+ 单测改钉新契约：`catch_unwind(frame_layout_info(&tm, None)).is_err()`。

### Changed (2026-10-11) — 后端 v23 M3（第二批）：**宽向量能力数据化**，通用管线不再认识 `avx*`

+ **新增能力查询** `TargetRegInfo::wide_vector_bytes(&self) -> &'static [u16]`：ISA 后端回答"本谱申报、且本机能执行的 >16B 定宽向量档位"（= `[meta].vector_tiers` 里 >16 的档位 ∩ 宿主上界）。缺省 `&[]` ⇒ 通用管线对 >16B 定宽向量 **fail-closed**，不静默降级。
+ **宿主探测留在宿主层**：`forge_isa_runtime::cpu::host_wide_vector_limit()`（16 = SSE2 基线 / 32 = AVX / 64 = AVX-512F；与旧 `avx512_available()` 同口径，`FORGE_ASSUME_AVX512=1` 仍能放开**生成期**可行性门）+ `machine::reg_info::wide_vectors_upto(tiers, limit)`（取最长前缀，`&'static` ⇒ 生成物直接返回）。
+ **管线两处 `crate::avx512_available()` 直接调用删除**（`compiler.rs` 的 ABI 宽向量门与 IR 层宽向量门），改为问 `wide_vector_bytes()`；错误消息也从"需 AVX-512F"改成 ISA 中立表述（顺带更新断言该文案的 W3 用例）。
+ **新守卫** `crates/backend/forge-codegen/tests/generic_pipeline_is_isa_neutral.rs`：`src/pipeline/**`（通用管线）的**代码行**里不得出现 `avx*`/`sse*`/`neon`/`sve`（标识符级匹配——`classes` 里的 `sse` 不算；注释与 `#[cfg(test)]` 之后豁免）。守卫做过**负例实测**：临时加回 `crate::avx512_available()` 立刻红，撤掉即绿。

### Changed (2026-10-11) — 后端 v23 M3（第一批）：删死钩子与死 API + 聚合常量端序修正

+ **删掉四个"没人实现/没人调用"的钩子与 API**（审计 G4/R7）：
  `TargetPeephole`（trait + `CompileState::run_peephole` + 编译器 Stage 5 + `TargetMachine::peephole()`；三份发行谱一个都没实现过，调用点恒为 `None`）、`TargetSimulator`/`SimulationState`/`SimError`（trait + `TargetMachine::simulator()` + 再导出）、`TargetEncoder::{encoded_size, encode_into}`（`encoded_size` 还得伪造 `AllocResult::dummy_for_sizing` 才能跑）、`AllocContext`（空结构 + `allocate()` 的第 3 个参数 + 9 处测试调用点）。
+ **`AllocResult::dummy_for_sizing` 删除**：它是"为估尺寸伪造一份分配记录"的辅助（写死 `RegClass::GPR64` 与 `% 16` 编号）——随 `encoded_size` 一起去掉。
+ **`agg_const.rs` 的端序修正**：聚合常量的标量字段此前**写死小端**（`out[off + k] = (raw >> (k*8)) as u8`），大端目标上会整字节反序。现在从 `TypeStore::data_layout.endianness` 取目标端序（`Endianness::Big` ⇒ 低位在后）。三份发行谱都是小端 ⇒ 字节不变，由既有 `isa_roundtrip_guard`/spec 向量守住。
+ 两条"宽度写死"守卫的白名单同步瘦身（`machine/encoder.rs` 2 条、`alloc_result.rs` 2 条随被删 API 消失；守卫要求白名单条目必须被命中，因此这是强制同步项）。

### Changed (2026-10-11) — 后端 v23 M2 收口：**删掉运行时的手抄调用布局镜像**，生成物直接读 `AbiPlan`

+ **`CallLayout`/`CallArg`/`ArgPlace`/`RetPlace`/`Ext` 整条删除**（运行时 `machine/call_layout.rs` 从 255 行缩到 78 行，只留 `va_list` 的宿主侧外壳）。`AllocResult.call_layout` 与 `CallConvCtx.layout` 现在的类型**就是** `forge_abi_model::AbiPlan`——"调用方与被调方共用的那份数据"从模型一路通到生成物，中间不再有第二份表示。
+ **`abi_target::call_layout` 的 ~260 行翻译整个消失**：`plan_for_shapes`/`plan_for_function` 直接返回 `AbiPlan`，调用点钩子 `plan_call` 也返回它；只剩下宿主算得出的变参字段初值（`va_info`/`va_init`，写进新的 `CallConvCtx.va`）。
+ **落点分类改走访问器**（`Placement::{as_reg, indirect_ptr, stack_offset, is_stack_byval, reg_refs}`、`RetLoc::{as_reg, reg_refs}`、`AbiPlan::{arg, caller_offset, slot_bytes, shadow_bytes, hidden_sret_slot, va_meta_slot}`）：生成物不再穷举枚举变体，因此"引擎加一个变体、某条发射路径静默走错分支"这类缺陷在结构上不可能发生。生成物侧 12 处模式匹配与调用点 1 处五臂匹配都换成访问器形式（更短、更直白）。
+ 连带修正：`RetLoc` 不再是 `Option`（`Void` 表达"无返回"）；`__cl.va` → `__cl.va_area`；`cl.slot_bytes`/`cl.shadow_bytes`/`cl.hidden_sret`/`cl.va_meta` 等字段名收敛成方法（栈区事实统一从 `stack` 读——与 M5 的"单一记账"同向）。
+ 守卫同步：`crates/frontend/forge-isa-dsl/tests/call_layout_emission.rs` 的判据从"生成物里有 `ArgPlace::Reg/Stack`"改为"有落点访问器（`as_reg`/`stack_offset`）"——它守的语义（布局路径生效、且排在旧路径之前）不变。
+ 生成物解析失败时的诊断补强（`gen_file::generated_body`）：`syn::parse2` 失败时打印**谱名**并把 token 原文写到 `%TEMP%\forge_gen_parse_fail.rs`（此前只有一句 `expected …`，1.4 MB 的生成物无从定位）。

### Changed (2026-10-11) — 后端 v23 M2：变参类型只剩一份（删镜像的 5 个手抄类型）

+ 运行时镜像里的 `VaArgRule`/`VaArgRules`/`VaField`/`VaSaveArea`/`VaSaveSlot` **五个手抄类型整条删除**，改为 `pub use forge_abi_model::plan::{…}`（`VaSaveArea as VaSave` 保持旧名）——`VaArgRule` 那 9 个字段此前是"计划面写一遍、镜像写一遍"。
+ 连带删掉翻译层里两个"逐字段搬家"函数（`va_arg_rules`/`va_arg_rule_one`，20 行）与字段/槽表的逐项重建（~15 行）：现在 `VaInfo { fields, save, arg_rules, … }` 与 `AbiPlan.va_area` **是同一份类型**，只剩"宿主算出来的 `VaInit`"要填。
+ **被丢掉的信息回来了**：`VaField.name`（psABI 里的字段叫法）此前在翻译时静默丢弃，现在随类型一起保留（诊断用）。
+ 生成物的保存区 store 序列跟着改读 `__slot.reg.slot`/`__slot.reg.class`（`dsl/codegen/frame.rs`，4 处 emitted 站点）。
+ `VaArgRule`/`VaArgRules` 在模型里补 `Copy`（纯小记录，本来就能复制，consumer 因此不必 `.clone()`）。

### Changed (2026-10-11) — 后端 v23 M2：`RegRef` 带**有类型的寄存器类 + 类内号**（翻译层收敛）

+ **`AbiTarget` 新增两个方法**：`reg_class(index) -> forge_ir::RegClass` 与 `reg_slot(index) -> u32`（类内号）。引擎在造 plan 时一并填进 `RegRef{ index, slot, class, name }`——`class` 从"无人读的字符串"变成**有类型**的 `RegClass`（`slot` 是发射侧要的那个号，`index` 仍是引擎的 ABI 空间号，两个坐标各有其主，文档写明不是重复表示）。
+ **连带删除**上一轮留的 `RegRef` 死字段整改方案的反面：`abi_target::call_layout` 里那个"按名字回查目标、再自己折类内号、名字解析不到还要退化"的 20 行闭包，收敛成一行 `(r.class, r.slot)`——这正是"同一事实两处表示"的缩影。
+ 5 处 `AbiTarget` 实现补齐：`MachineAbiTarget`（codegen，读自己的 `n_gp`/类）、`forge-isa` 的 `ViewTarget`（谱侧类名串 → `RegClass`，新增 `forge_ir::RegClass::from_name` 作为 `Debug` 形态的逆）、`StubTarget` 与 forge-abi 的三处测试替身。
+ `forge-abi` 与 `forge-isa` 因此新增 `forge-ir` 依赖（公开面出现 `RegClass`）；`AbiPlan` 一族的 **serde 派生整体去掉**（实测无消费者，只有 TOML 面的 `CalleeSaveMechanism` 保留）。

### Changed (2026-10-11) — 后端 v23 M2：形状只剩 `TyView`（删 `ArgShape`/`ShapeKind` + 两份 IR 投影合一）

+ **运行时侧的中性形状类型 `ArgShape`/`ShapeKind` 整条删除**，调用点 plan 的请求面改用它一直该用的那份视图：`CallRequest { args: Vec<TyView>, rets: Vec<TyView>, … }`（`forge_abi_model::TyView`，运行时经 `machine::abi_model` 再导出）。`abi_target::shape_to_ty` 那座"运行时形状 → 引擎视图"的桥（30 行 match）随之消失——**同一份类型，不再有桥**。
+ **IR → 类型视图的投影只剩一份**：`TyView::from_ir_type(store, ty)`（`forge-abi-model/src/ir_view.rs`，带深度上限 4 与成员上限 16）。此前有两份且**退化策略不一致**——运行时的 `ArgShape::from_ir_type` 与代码生成的 `sig_view::ty_view`（后者**没有深度上限**，递归类型会栈溢出，且长数组折成"一个整块 int"）。`sig_view::ty_view`/`element_kind`/`expand_members` 全删，`signature_view` 直接用唯一投影。
+ 生成物侧路径从 `crate::machine::call_layout::ArgShape` 改为 `crate::machine::abi_model::TyView`（读口子 = 运行时对 leaf crate 的再导出）；`forge-isa-runtime` 的依赖面守卫（`runtime_surface.rs`）把该 leaf crate 加入**带理由**的白名单。

### Changed (2026-10-11) — 后端 v23 M2：ABI 产物抽成 leaf crate `forge-abi-model`（+ 删 `RegRef.class` 死字段）

+ 新 crate `crates/foundation/forge-abi-model`（**只依赖 serde**）：`AbiPlan`/`Placement`/`RetLoc`/`ArgLoc`/`StackLayout`/`HiddenSlots`/`VaArea`/`VaArgRule(s)`/`VaField`/`VaSave*`/`DeclAttrs` + `TyView`/`TyKind`/`Elem` + 两个规则侧枚举（`PositionRule`/`CalleePop`，它们同时是 `AbiRules` 与 `AbiPlan` 的字段类型）。`forge-abi` 收缩为"规则/绑定/引擎/注册表"并 re-export 模型——`forge_abi::plan::RegRef`、`forge_abi::AbiPlan`、`forge_abi::TyView` 等**路径全部照旧**，全仓 0 处调用改动。
+ 动机：`AbiPlan` 是调用方与被调方共用的唯一数据形状，而运行时/生成物只依赖 `forge-isa-runtime`——抽成 leaf crate 后，运行时可以直接读它，运行时侧那份**手抄镜像**（`machine::call_layout::CallLayout` + `ArgPlace`/`RetPlace`/`VaInfo`…）与 300 行翻译才有条件删除（下一批做）；把 toml/引擎拖进运行时的路被排除。
+ **删掉 `RegRef.class: String` 死字段**：实测全仓**没有一个读者**（发射侧要的"类 + 类内号"由 ISA 的目标视图给出，不由这份数据携带）。连带删掉只喂养它的 `AbiTarget::reg_class_name`（trait 方法 + 6 处实现：`MachineAbiTarget`、`forge-isa` 的 `ViewTarget`/`StubTarget`、forge-abi 的三处测试替身）与 `ViewTarget.classes` 字段。`RegRef` 现在是 `{ index: ABI 空间号, name: 谱里的寄存器名 }`。

### Fixed (2026-10-11) — 后端 v23 M2：聚合落点探针丢了成员形状（AAPCS64/LP64D 的 HFA 会静默错值）

+ `pipeline/compiler.rs` 的两个只读探针 `agg_arg_place` / `agg_ret_is_sret` 原先伪造 `Aggregate { members: [] }`（只报 size/align）。后果：AAPCS64/LP64D 的 HFA（`{f64,f64,f64}` 这类走浮点寄存器池）在**调用方一侧**被判成"普通聚合"⇒ `ByRef`，调用方按"本地副本 + 传指针"改写两端，而被调方按浮点寄存器收参 ⇒ 静默错值。
+ 现在探针取 `(&TypeStore, TypeId)`，形状一律走 `ArgShape::from_ir_type`（与**调用点 plan**、**被调方 plan** 同一份投影，递归摊开成员/lane）；三个调用点（`expand_large_agg_params` / `expand_sret_return` / `expand_agg_call_args`）改传类型。**读锁处数不变**（判定与探针对合并进同一段读锁，`forge-ir` 的 `read_path_budget` 仍为 20——它是精确相等守卫）。
+ 守卫：`compiler.rs::alloc_integration_tests::aggregate_place_probe_carries_member_shapes`——用**同尺寸、不同成员**的一对类型证明"成员真的进了探针"：`{f64,f64,f64}`（24 B，HFA）⇒ `Registers`；`{i64,i64,i64}`（24 B，非 HFA）⇒ `ByRef`。

### Changed (2026-10-11) — 后端 v23 M1：唯一编译入口 `CompileSession`（破坏性）

+ **`FunctionCompiler` 并入 `CompileSession`**（`forge_codegen::CompileSession`）：单函数与整模块两条入口都走同一份实现（`pipeline/session.rs` 只做编排，`pipeline/compiler.rs` 是单函数管线）。删 `compile()`/`compile_raw()` 的重复面——后者的"显式绕过优化"承诺从来就不成立（实现与 `compile` 是同一句调用）。
+ **新增 `CompileSession::compile_module(&Module) -> CompiledModule`**：整模块编译的唯一实现（全局数据段与 `名字`/`G{id}` 双符号 → 逐函数编译，函数数 ≥ 8 走 `std::thread::scope` 并行）。`JitLoader::compile_module` 与 `JitLoader::install` 只是它的委托 + 装载，JIT 里"并行/串行两份编排 + 符号注册"的复制代码删除。
+ **新增 `CompileSession::add_function(name, &FunctionSignature, build_fn) -> CompiledFunction`**：单函数便捷入口（与整模块同一条编译实现）。
+ **`JitCompiler` 更名 `JitLoader`**（它只负责可执行内存/符号/重定位/`get_fn`；编译委托给 `CompileSession`）。
+ **模块签名表类型化**：`Module::signature_table()` 返回 `forge_ir::ModuleSigs`（逐项 `FuncSigEntry { variadic, fixed_args }`），`LowerCtx::conv.module_sigs` 改为 `Option<Arc<ModuleSigs>>`——替代满仓传递的 `Vec<(bool, u32)>` 元组。
+ `ErasedTargetMachine` / `FunctionPipeline` 的 `compile_raw` 方法名改为 `compile`（`compile_raw` 这个名字在全仓消除）。

### Fixed (2026-10-11) — 后端 v23 M1：`JitLoader::add_function` 不再丢模块签名表

+ 单函数入口此前**不装**模块签名表 ⇒ 跨函数变参调用按"被调方非变参"规划落点（SysV 下漏写调用的元信息寄存器 `%al`、浮点实参落位错）。现在 `JitLoader` 按调用序累积签名表并交给 `CompileSession`（整模块入口仍以模块自己的表为准）。
+ 守卫：`crates/backend/forge-codegen/tests/session_api.rs`（4 条：整模块按 `FuncRef` 序返回每个函数、整模块与单函数两条路径字节相同、"≥8 函数并行路径同源"、`add_function` 认签名表——装表与不装表的字节必须不同）。

### Added (2026-10-11) — ISA-DSL v22：操作数声明与字段绑定就地写进 `asm`

+ `asm` 模板支持 `{名字:槽[:角色]}` 声明、`{@字段:槽[:角色]}` 绑定与 `{@[字段]:槽[:角色]}`（内存形态，等价旧 `bind` 的 `"[操作数]"`）；旧的 `ops`/`bind` 键迁移期并存，**同一条指令只许写一处**（混用即报错）。
+ 抽取走"模板 → 内部 `ops`/`bind`"的回填：编码/解码/汇编/降级生成与 `validate`/`lint`/`report` **一行未改**，因此内联写法与旧写法在 `insts` 报告面**逐字段等价**（守卫 7 条）。
+ 新增 `forge_isa_dsl::spec_migrate`：把旧写法（`ops` + `bind` + `{名字}`）纯文本迁成内联写法，硬门 = 迁移前后 `expand_file` 展开结果**逐字节相同**；`forge-isa new <名>` 脚手架（内容与教程 §0 同源）。
+ 边界：顺序敏感的指令（出现序 ≠ 旧 `ops` 序）暂用 `ops`——显式标注编码序的写法（`#序号`）尚未定稿。

### Fixed (2026-10-08) — forge-codegen：Stage 0.5 建的 `Alloca` 用错 immediate 形态，帧槽不进表（会毁帧崩溃）

+ **根因**：帧槽预扫描（`pipeline/frame_slots.rs`）只按 `Immediate::Type(elem)` + `Immediate::Uint(count)` 算 `Alloca` 的字节数；而**聚合展开**（Stage 0.5）建槽时写的是 `Immediate::Const(常量池 id)` ⇒ 该 `Alloca` **不进槽表** ⇒ 没有帧偏移 ⇒ 取址落到帧基址一带（写 24 字节直接毁帧，实测 `0xC0000005`）。规范形态见 `FunctionBuilder::alloca`（`builder.rs:575`）。
+ **修法**：两处 Stage 0.5 的建槽改回规范形态 `Type(TypeId::I8) + Uint(size)`——`memoryize_from_segs`（嵌套聚合字段的帧槽，`agg_expand.rs`，**既有隐患**：此前任何走到这条路的运行时路径都会写坏帧）与调用方 by-ref 副本（`expand_agg_call_args`，`compiler.rs`）。
+ **同时接上了 A1′-ret 的调用方一侧**：IR 内部 `call` 按值取 `>8B` 聚合时，调用方 `Alloca` 缓冲 + 作为实参 0 传 + 结果清空 + 结果值登记成"住在缓冲"（后续 `extractvalue` 变 load）。`test_jit_agg_by_value_ret_call_win64` 真跑通过（含"只 call 不取结果"的探针 + 取第 2 个字段两条）。
+ 这条崩溃曾被误判为"调用方 sret 落点不对"——**实测纠正**：lowering 序列与两侧计划本来就是对的（`Alloca -> Call args=[XReg(buf)]`；callee `Store [param], seg ×3`；`[ptr] -> RCX` 对 `<sret> -> reg RCX`），问题在**帧偏移**这一层。

### Added (2026-10-08) — 声明注释：ISA-DSL 的 `doc`（继承 + 插值）与 IR 操作码/构建器文档

+ **ISA-DSL：`[inst.<名>].doc` / `[form.<名>].doc` / family 的 `body.doc`**（都可选）：解析后按 `inst` > family `body` > `form` 继承，做**固定小集 + 操作数派生**的 `{}` 插值（`{name}` `{form}` `{family}` `{width}` `{data_width}` `{ops}` 与 `{<操作数名>.slot|.width|.class}`），**未知占位符报错**（注释没有第二个消费者，拼错必须当场暴露），`{{`/`}}` 是字面花括号；多行按 `\n` 拆成多条 `#[doc]`。落在**生成物 `Inst` 变体的 `#[doc = …]`**（rustdoc 可见）与 `forge-isa insts`/`explain` 的 `doc` 列。三处都不写 ⇒ 生成物一字不加（发行谱现状不变，`determinism` 仍逐字节稳定）。
+ **family 复用**：`[family.<名>]` 的 `body.doc` 由族展开写入**每一行**，行键（`{cc}` 之类）在展开期就已替换 ⇒ 一份注释覆盖一族指令，正是"少写重复注释"的地方。
+ **IR 操作码文档**（`ops.toml` 117 条 `doc` 全部改写）：写清语义 + 操作数/结果 + UB 条件 + 副作用性质。`build.rs` 在每条变体的 rustdoc 末尾追加一行**自动属性行**（操作数/结果/可能 UB/副作用/条件通道/类型规则/LLVM 名），机械事实不再靠人抄、也不可能与行为漂移；`doc` 支持多行。构建期门槛：`>= 24` 字符 + `may_ub` 条目必须提 UB（**门槛从计划里的 40 改为 24**：`Ret`/`Nop`/`Clz` 这类简单指令写满 40 字符只能靠凑字，24 仍能挡住"按位或"式的一句话）。
+ **`FunctionBuilder` 全部指令方法补注释**（`src/ir/builder.rs`）：六个宏（`int_binop`/`int_binop_ub`/`float_binop`/`unop_int`/`unop_float`/`overflow_binop`，共 49 个生成方法）在宏体内发一条**指向该 opcode 生成文档**的 `#[doc]`（不复述语义）；87 个手写方法逐条补"构建器契约 + `Opcode::X` 指针"，其中 opcode 语义首句由 `ops.toml` 派生。
+ 守卫：`crates/frontend/forge-isa-dsl/tests/doc_comment.rs`（继承/覆盖/插值/字面括号/未知占位符 fail-closed/发行谱仍可展开）与 `crates/foundation/forge-ir/tests/opcode_docs.rs`（长度、UB、副作用三条从运行期 `Opcode::INFOS` 守）。
+ **发行谱的示范批次**：三份谱各挑一个族在 `body` 里写共享 `doc`——x86 `family.ALU_MR`、riscv64 `family.RR`、arm64 `family.ADDIMM`（后者用行键 `{slot}` 演示逐行插值：`ADDIMMX` 得 `r64`、`ADDIMMW` 得 `r32`）；`tests/isa/demo.toml` 的 `[form.R]` 也加了一条（`{ops}`/`{dst.slot}`）。生成物实测：`///ADD_MR（族 ALU_MR）：ALU 的内存形式——内存操作数参与运算（src, mem）；写内存有副作用。`。
+ 文档：`docs/reference/isa-dsl.md` 新增「声明注释（`doc`，可选）」一节（含键速查表同步与 `isa-dsl.schema.json` 重生成）；`crates/foundation/forge-ir/README.md` 增「`doc` 的写作要求」；`docs/guides/isa-dsl-tutorial.md` 加最小示例；`forge-isa explain` 的文本输出打印「注释」行（与生成物 `#[doc]` 同源）。
+ **第二批族注释**（12 个族）：x86 `ALU_R_MEM`/`ALU8_RM_IMM8`/`SD_BIN`/`SS_FMOV`，riscv64 `II`/`BB`/`WW`/`FADD_RM`，arm64 `ADDSIMM`/`SUBREG`/`ANDREG`/`ORRREG`——都只用族内**每行都有**的插值键（riscv 的 `{dst.slot}` 得 `gpr`、arm64 的 `{slot}` 得 `r64`/`r32`），生成物已抽查。

### Added (2026-10-08) — A2′ 调用方一侧的机制全部就位（角色/属性透传/拷贝发射），并查明坐标差一槽

+ **三条能力角色**（`arg_bytes_load`/`arg_bytes_imm`/`arg_bytes_add`）：x86 分别声明在**现成指令** `MOV_R_MEM`（寄存器地址 load）/`MOV_REG_IMM64`/`ADD_RM_R` 上，谱校验与 DSL 守卫全绿。它们是"按值栈实参拷贝"的指令选择，不进 ABI 能力视图（`role_capability` 归 `None`）；文档见 `docs/reference/isa-dsl.md` 的角色表。
+ **`LowerCtx::current_param_attrs`**：逐实参声明属性快照（管线每条指令填一次）。生成的 `lower_inst` 只看得到 `(op, args, results, ctx)`，读不到指令本身——调用点的**形状**正需要它：`byval(聚合类型)` 的实参在 IR 里是 `ptr`（指向调用方本地副本），但 ABI 上按**聚合**算。
+ **调用点形状覆盖 + 拷贝发射**（DSL）：`byval` 存在时按聚合取形状（⇒ `ArgPlace::StackByVal`），并按"游标 ← 指针 / 步长 ← 8 / 逐 8 字节 load → `stack_arg_store` 到 `[sp + caller_offset(k) + 8j]` / 推进游标"生成拷贝；缺角色 ⇒ 生成期 fail-closed。
+ **坐标口径定论并修好**：接上后自洽真跑曾返回 `33`（期望 `22`）——根因不在 ABI 数据，而在 `[[lower.StackAddr]]` 的 lea 位移是 `立即数 − callee-saved 区平移`（`lowering.rs` 的 `current_offset`）：我发的正立即数被平移了 8 字节 ⇒ 地址偏低一槽（读到下一个字段）。**没有**去动管线那条换算（`va_arg` 一族正是靠"正立即数也参与平移"工作的，改它会红 3 条 varargs 用例），而是让**宿主**在 Stage 0.5 用**同源**的 `frame_layout::frame_layout_info(machine, Some(&call_layout(&plan, machine)))` 取到平移量，发 `offset + shift`，管线减回去正好落在 `[fp + offset]`。
+ **调用方已启用**：sysv64 的 `>16B` 聚合按值传参**两端打通**——自洽真跑 `test_jit_agg_by_value_stack_sysv64` 返回 22（模块内 `call` + 被调方按传出区读）；负向用例改为"win64 走 by-ref / sysv64 走按值栈，两端都能编译"。
+ **独立裁判已就位（待 Linux CI 跑）**：`test_jit_agg_by_value_stack_sysv64_extern_c_referee`（`#[cfg(all(target_arch = "x86_64", target_os = "linux"))]`）——Linux 上 Rust 的 `extern "C"` 就是 sysv64，由平台 C ABI 按值传 24 字节结构体，我们编的被调方必须读到 22；本机是 win64 故不跑（CI 排除的是 win64 约定那批，见 `ci.yml`）。

### Added (2026-10-08) — 调用约定：新落点 `StackByVal` + sysv64 按值聚合的**被调方**一侧（A2′ 前半）

+ **新落点变体 `StackByVal`**（`forge-abi` 的 `Placement` / `forge-isa-runtime` 的 `ArgPlace`）：与标量的 `Stack`（一格一个寄存器值）分开，表示"**聚合按值整块进传出参数区**"——调用方从指向本地副本的指针逐段拷进传出区、被调方按 `offset` 当内存读。引擎的 `stack_place` 对**聚合**返回它（标量不变）；`sysv64.plan.txt` 黄金随之改一行（`arg s size=24 -> stack` ⇒ `stack_byval`，win64/aapcs64/lp64d 未变）。
+ **被调方一侧已接**：`expand_large_agg_params` 对 `StackByVal` 档的聚合形参**保留形参**（布局/偏移由它决定，必须与调用方一致），改用 `StackAddr(+offset)` 得到"传入区地址"（**正立即数 = ABI 相对**：`frame_slots` 只对负立即数记账局部槽、`[[lower.StackAddr]]` 本就是 `LEA [RBP+off]`），再按 A1′ 已打通的 `agg_slots` 地址语义重写使用处；DSL 生成的 `move_args` 对这类形参**跳过**收参（它没有入场寄存器、也不进 spill 槽——此前会报 `spilled 寄存器参数需要布局给的寄存器落点`）。
+ **顺带的行为改进**：形参展开此前把**所有**形参重写成 `i64`（包括 ptr/f64 等单段参数）✗；现在只对真正要拆段的聚合段做，**其余形参保留原类型**（`keep_param`）。
+ **调用方一侧仍未接**（sysv64 调用 IR 内部 `call` ⇒ 明确 fail-closed、消息点名 `StackByVal`）：要让它成立，调用点形状必须按**聚合**算（否则引擎给 `Reg`、指针进寄存器、被调方按传出区读 ⇒ 静默错值），而生成的 `lower_inst` 只拿到 `(op, args, results, ctx)`，**看不到指令的 `param_attrs`** ⇒ 需要把 `param_attrs` 经 `LowerCtx` 透出来（或改 trait）之后才能读 `byval`。在那之前不发可能错值的代码。
+ 验收：`forge-abi` 全绿、`forge-codegen --features jit` 全绿、整仓门禁全绿；`unsupported_inst_negative.rs::agg_param_over_16_bytes_routes_by_convention` 更新为"win64 两端通 / sysv64 **被调方通**、调用方 fail-closed"。

### Added (2026-10-08) — forge-codegen：按值返回 >8B 聚合的**被调方**一侧（A1′-ret 前半）

+ **被调方把按值返回的 `>8B` 聚合写进调用方缓冲**（win64/sret）：新增只读探针 `agg_ret_is_sret`（`plan.ret` 是否 `Indirect`）+ `expand_sret_return`——给函数**追加**一个**普通 `ptr`** 形参承载调用方缓冲（约定把 sret 指针放在首个整数槽：win64 = RCX、sysv64 = RDI，与"第 0 个普通指针实参"完全同形），并把签名的返回清空；`expand_large_agg_ret` 再把段值写进 `[buf]`（AggConst / load / 内存里的值三条来源）。
+ **为什么用普通 `ptr` 而不写 `sret` 属性**（与 A1′ 的 byval 同一条规律，实测踩到）：引擎对 `sret`/`byval` 落点给 `Indirect`，而生成物 `move_args` 的 `Indirect` 收参是"从指针 **load** 出值"——那样形参拿到的是缓冲**内容**而不是缓冲**地址**，会往垃圾地址写（实测 0xC0000005）。
+ **调用方一侧也已接**（见下方最新的 `Fixed` 条：根因是 Stage 0.5 的 `Alloca` immediate 形态，修好后"缓冲 + 实参 0 + 结果清空"这条路真跑通过）。
+ 验收：`cargo test -p forge-codegen --features jit` 全绿（3 条聚合用例：按值形参 + 按值实参 + 按值返回），`unsupported_inst_negative.rs` 的约定分流用例（win64 通 / sysv64 拒）保持绿。

### Added (2026-10-08) — forge-codegen：win64 按值聚合传参（形参 + 实参，A1′）

+ **>8 字节聚合按值传参在 win64 上走通了**（`docs/forge-ir/backlog.md` #6 的 A1′）：此前 `agg_expand` 用写死的 `size > 16` 一律拒绝（与约定无关）；现在改成**读约定**——新增只读落点探针 `agg_arg_place`（用 `forge_abi` 的单参 `plan_for_shapes` 问"这份约定把 `size` 字节聚合放哪"；聚合规则是纯尺寸判据，与寄存器耗尽无关，所以单参探针与真实计划一致）。据此分流：
  + **win64（`aggregate size_gt 8 ⇒ indirect`）**：两端统一变换成"**调用方帧内副本 + 传 `ptr`**"——被调方形参改 `ptr` 并把"聚合住在该指针指向的内存"登记进 `agg_slots`（`extractvalue` 走 GEP+load、store 走向内存拷贝、传参传该指针）；调用方 `Alloca` 副本 + 逐段 store + 传副本指针。机器级效果与 Win64 psABI 的"指向调用方副本的指针"一致，且**顺带修掉** 9–16 字节聚合此前被拆成两个寄存器传的偏差。
  + **sysv64（`aggregate size_gt 16 ⇒ stack`）**：仍是明确 `Unsupported`（整块进传出参数区，发射侧未接 = A2′）。
  + **实现取舍（实测得出）**：**不给**被调方的 `ptr` 形参写 `byval` 属性——引擎对 byval 落点给 `Indirect`，而生成物 `move_args` 的 `Indirect` 收参是"从指针 load 出值"（向量语义，要求值有寄存器表示），聚合要的是指针本身进参数寄存器。
  + **顺带修一个会静默丢数据的坑**：`rewrite_agg_value_uses` 的 `Store`/`Call` 支此前只认段值列表——by-ref 档（值在内存、段值列表为空）会**一条 store/一个实参都不生成**。现在 `Store` 走"从源地址逐段拷到目标"、`Call` 把实参换成副本指针，`Return`（按值返回在内存里的聚合）显式 fail-closed（A1′-ret 才接）。
  + 验收：`forge-codegen` 的 `test_jit_agg_by_value_param_win64`（24 字节按值形参 + 聚合字面量实参，**两个方向真跑**）与 `unsupported_inst_negative.rs::agg_param_over_16_bytes_routes_by_convention`（win64 通过 / sysv64 拒绝）；`cargo test -p forge-codegen --features jit` 全绿。

### Changed (2026-10-08) — forge-codegen：`agg_expand` 第三批迁移（rewrite 族 4 函数）

+ **`memoryize_from_segs` / `memoryize_from_addr` / `rewrite_agg_value_uses` / `move_to_front` 从 `pipeline/compiler.rs` 迁入 `pipeline/agg_expand.rs`**（第四十四轮记录、第四十六轮"剩余状态"里的"第三批"，`docs/forge-ir/backlog.md` #1）。搬移用逐行切片（字节精确，非人工转写）：`compiler.rs` 2940 → 2544 行、`agg_expand.rs` 177 → 574 行（含文档注释块）；四个函数改 `pub(crate)`，调用点统一经 `use crate::pipeline::agg_expand::{…}` 引入；`agg_expand.rs` 里"expand_* 族将按批迁入"的占位注释改成实况（仍留 `compiler.rs` 的是 `expand_*` 执行族，与 `CompileState` 时序耦合更紧）。
+ **读路径预算随之更新**（`crates/foundation/forge-ir/tests/read_path_budget.rs`，v3 S3 静态预算守卫按"迁移完就下调"的纪律）：`pipeline/compiler.rs` 21 → 17，并新增 `pipeline/agg_expand.rs` 的 13 条目（本次搬移转移了 4 处 `.borrow()`；`has_*` 与本轮迁入的 rewrite 族都是"每入口一次"，不引入逐指令取锁）。
+ 验收：`cargo check -p forge-codegen` 0 error、整仓门禁 **exit 0**。

### Fixed (2026-10-08) — forge-opt：嵌套聚合常量上的 `extractvalue` 链现在能折到底

+ **`extractvalue` 常量折叠支持"深层索引"**（`docs/forge-ir/backlog.md` #3）：此前 `fold_extract_value` 只折**单层** —— 第一层提取若结果是**子聚合**就返回 `None`，「提取子聚合的字段」的第二条指令因此看不到常量，**整条链都不折叠**（实测复现：`changed=false, removed=0`）。现在沿 `extractvalue` 链**上溯出（根部常量聚合 × 索引路径）**，直接在 `ConstantPool` 的 `AggConst` 树里**深层下钻**（新增 `agg_child_at` / `resolve_agg_chain`）⇒ 嵌套链**一次折到标量**；折叠应用点的"操作数必须全是常量"闸门对这类指令放开（链上的聚合源是**值**而非 immediate）。回归用例 `fold_nested_extractvalue_chain`（`{i32, {i32, i32}}` 常量上两层提取），`cargo test -p forge-opt` **100 passed**。
+ **不做**：`builder.extract_value`/`insert_value` 的"路径版" —— 这套 IR 里聚合常量只作为 `extractvalue` 的 **immediate** 出现（**没有"独立聚合常量值"**），路径助手没有根可起步，加了是投机 API；`insertvalue` 的常量折叠（要新造聚合常量）也无实测用例，需要时再评估。

### Changed (2026-10-08) — ISA-DSL v21 W6+：标志条件码**移出 IR**（改由 DSL 生成）+ 条件码补全

+ **标志位条件码不再属于 IR**：`forge_ir::FlagCC` **删除** —— `IntCC`/`FloatCC` 是**值的比较谓词**，标志位（溢出/进位/符号/奇偶/恒真恒假）是**机器语义**，两者差别很大，而且 **IR 层根本用不到标志条件**（IR 不产生"读 OF"这类操作）。现在 `FlagCC` **由 DSL 按谱里的标志位表生成**（生成物里的本地枚举，`forge_ir::FlagCC` 不存在），只活在 lowering 与生成物里。
+ **条件码补全**（"不能因为指令集用不上就不写"）：
  + 规范标志助记符 **8 → 10**：补 `al`/`nv`（恒真/恒假）—— 条件码空间里的**退化成员**，A64 的 16 个条件各占一个码 ⇒ 必须能表达。
  + x86 `[enum.flagcond]` 补齐 `p`/`np`（8 个）。
  + **arm64 补出整块标志位能力**：新增 `[enum.flagcond]`（`mi→s`、`pl→ns`、`vs→o`、`vc→no`、`al→al`、`nv→nv`）+ `[operand.flag4]`（`cond_bank = "flag"`）+ **4 个标志位形态** `cself`/`csincf`/`csinvf`/`csnegf`（各 64/32 两位宽，共 8 条指令）—— 于是 A64 的 16 个条件**全部**都有类型化的归宿。（汇编面用独立助记符：与整数形态同文本同签名会被歧义 lint 判成真歧义。）
+ **修一个真 bug**：定宽 ISA 的解码是按掩码分组的 if 链 ⇒ 条件码"没有 IR 对应"必须在**候选自己的条件**里判（写进 `guard`），否则会连坐同组的兄弟候选（arm64 的 `csel …, nv` 正是这样炸出 `unreachable!`）。
+ 验收：整仓门禁 **exit 0**；三矩阵 x86 197/3/0、riscv64 136/64/0、arm64；谱内向量 505；指纹 x86 `derived` 1550、arm64 `2581`（+16）、`spec_totals` arm64 **552**（+8）；lint 快照 arm64 `refs 27→31`、`unassigned_bits 262→266`（**+4 / +4 = 四个新家族**，与其整数兄弟同形，已复核）。

### Changed (2026-10-08) — ISA-DSL v21 W6+ 甲：条件槽字段 = IR 条件枚举（`IntCC`/`FloatCC`/`FlagCC`）

+ **生成物里条件码字段的类型就是 IR 条件枚举**（不再是裸 `u8`）：`[operand.<名>].cond_bank` 选银行 ⇒ 整数表 ⇒ `IntCC`、浮点谓词表 ⇒ `FloatCC`、标志位表 ⇒ `FlagCC`。机器码只在**编码 / 解码 / 汇编文本**三个边界出现，走生成的 `CondEnc` trait（`IntCC`/`FloatCC`/`FlagCC` 各一条 `enc`）与 `intcc_of`/`floatcc_of`/`flagcc_of`（表里没有 `ir` 对应的码返回 `None` ⇒ fail-closed）。
+ **`forge-ir` 新增 `FlagCC`**（`O`/`No`/`C`/`Nc`/`S`/`Ns`/`P`/`Np`，变体名 = 助记符 + 逐变体注释）：溢出/进位/符号/奇偶是**机器标志位**语义，`IntCC`/`FloatCC` 都没有（LLVM 的 `IntCC` 同样没有），而 x86 `seto`/`setc`/`sets`、arm64 `b.vs`/`b.mi` 的 lowering 必须用到——这就是"IR 解释汇编层、赋予新语义"。
+ x86 谱补三张表与三个形态：`[enum.fcond]`（`p→uno`/`np→ord`/`b,a,be,ae,e,ne`）+ `[enum.flagcond]`（`o`/`no`/`c`/`nc`/`s`/`ns`）+ `[operand.fcc]`/`[operand.flag]` + `SETCC_F`/`SETCC_FLAG`/`CMOVCC_FLAG`（同编码、汇编面 `setccf`/`setccfl`/`cmovccfl`，因为文本层分不出条件银行）；FP 比较与溢出/饱和 lowering 各自改走对应形态。arm64 条件表补 8 个缺失的 `ir`（`gt→sgt`/`le→sle`/`ge→sge`/`lt→slt`/`hi→ugt`/`ls→ule`/`cs→uge`/`cc→ult`）。
+ **修一个真 bug**：解码候选里写 `return None` 会**中断整条解码**（`0f 9a c0` 因此解不出来）⇒ 改成"候选**守卫**里判 `…is_some()`"，字段构造侧不可达。
+ 通用性守卫口径随之更新：`IntCC`/`FloatCC`/`FlagCC` 出现在生成期是**设计要求**，被禁的是**写死"变体 → 机器码"配对**（映射必须来自条件表的 `ir`）。
+ **已知代价**（棘轮已按约定刷新并记明原因）：arm64 的 `al`/`nv`（恒真/恒假）等无 IR 对应的条件**表达不了** ⇒ `csel …, nv` 一行解析不出（`parsed 576→575`、`tail_mismatch 0→1`）。
+ 验收：整仓门禁 **exit 0**；x86 矩阵 **197/3/0**、riscv64 **136/64/0**、arm64；谱内向量 **505 全绿**；指纹 x86 `derived 1542→1550`（4 个新形态）、`spec_totals 299→302`，riscv64/arm64 **逐位不变**。

### Changed (2026-10-08) — ISA-DSL v21 W6+ 丙（片 1）：条件码表**固定两类**（整数 / 浮点，枚举化）

+ **条件码表不再是一张**：整数条件一张（约定名 `[enum.cond]`）、浮点谓词一张（如 `[enum.fcond]`，条目写 `ir = "oeq"` / `"uno"` / `"ord"` …）。**同类只能一张**，多了报错。
+ **不用字符串键**：模型里是 `CondBank { Int, Float }` + `CondTables { int, float }`（固定两类），用户面键 `cond_bank = "int" | "float"`（cond 槽；缺省 `int`，既有谱零改动）。
+ 表归哪一类由**表内容**判定：只看两个枚举**各自独有**的助记符（`ugt`/`uge`/`ult`/`ule` 两边同名 ⇒ 不参与）；没有定论（空表 / 纯别名 / 只有共有名）按约定名当**整数**表；**两类混装** ⇒ 明确报错。
+ 这是"条件槽字段类型 = `IntCC` / `FloatCC`"（丙）的**数据面**：下一步按槽的银行定字段类型，x86 的双性槽（整数条件 + 浮点奇偶位惯用法）拆成两形。
+ **行为不变**：缺省银行 = 整数，三谱条件表与编码未动；整仓门禁 exit 0、`schema_guard` 与三目标 lint 全绿。

### Changed (2026-10-08) — forge-ir：`IntCC`/`FloatCC` 变体名简化为助记符 + 补逐变体注释

+ **`IntCC`**（10 个）：`Equal`→`Eq`、`NotEqual`→`Ne`、`SignedLessThan`→`Slt`、
  `SignedGreaterThan`→`Sgt`、`SignedLessThanOrEqual`→`Sle`、`SignedGreaterThanOrEqual`→`Sge`、
  `UnsignedLessThan`→`Ult`、`UnsignedGreaterThan`→`Ugt`、`UnsignedLessThanOrEqual`→`Ule`、
  `UnsignedGreaterThanOrEqual`→`Uge`。
+ **`FloatCC`**（16 个）：`Ordered`→`Ord`、`Unordered`→`Uno`、`Equal`→`Oeq`、`NotEqual`→`One`、
  `LessThan`→`Olt`、`LessThanOrEqual`→`Ole`、`GreaterThan`→`Ogt`、`GreaterThanOrEqual`→`Oge`
  （`False`/`True`/`Ueq`/`Ugt`/`Uge`/`Ult`/`Ule`/`Une` 不变）。
+ **变体名 = `mnemonic()` = DSL 里的条件名**（`eq`/`sge`/`oeq`/…）⇒ 读 IR、读谱、读生成物
  不再需要一张翻译表。
+ 补上两个枚举的完整文档：`s*`/`u*` 的有符号/无符号语义、`o*`/`u*` 的 NaN 语义、
  `One`（ordered 且不等）与 `Une`（unordered 或不等）**不是**简单组合的坑、
  `Ord`/`Uno` 只判 NaN 不判大小、`False`/`True` 是折叠残余，以及逐变体对照表。
+ 全仓 **396 处**引用同步改名（38 个文件）；**行为不变**：编码指纹守卫通过、整仓门禁 exit 0。

+ 多值返回用**逐分量模板**表达：`when = { multi = true }` + `insts = []`（多值没有单一收尾序列）
  \+ `move`（整数分量）/`movef`（浮点分量），对**每个结果重复一次**；新占位符
  **`{retN}`/`{retN_src}`** 指第 k 个返回槽/结果值。
+ 分量按它自己的银行（布局的 `Pair`/`Group` 那项 ⇒ `is_int`/`is_fp`）与位宽分派：
  浮点分量按 `width` 分档（x86：32 → `MOVSS`、64 → `MOVSD`；riscv64：`FSGNJ_S`/`FSGNJ_D`）。
  缺对应模板 ⇒ 生成物里 fail-closed。
+ 闸门：`multi = true` **必须**给 `move`/`movef`；`move`/`movef` 只对 `multi = true` 有意义；
  `multi = true` 与 `by_ref = true` 互斥。
+ **命中已证明**：谱面规则存在时旧的逐分量分支**根本不生成**（生成期分支），而完整管线的
  两返回值函数（`abi_target_real::pair_return_probe`）仍 **25 passed**，两矩阵 **197/3/0**、
  **136/64/0** 不变。
+ 至此 **`[lower.Ret]` 的五类形态全部在谱面**：`gpr` / `fpr`（任意宽度）/ `vec + by_ref = false` /
  `vec + by_ref = true`（sret）/ `multi = true`。

### Changed (2026-10-08) — ISA-DSL v21 W6 乙（四）：按引用返回（sret）下放 + `{sret}` 占位符

+ 新占位符 **`{sret}`**：**内存操作数**占位符（`PhKind::Mem`），产
  `MemRef { base: Some(__sret), disp: 0, index: None, scale: 1 }`；`__sret` 来自布局的
  `hidden_sret`（缺 ⇒ fail-closed）。模板里直接写 `{sret}`（不带方括号）。
+ 放行 **`bank = "vec", by_ref = true`**（>16B 向量按引用返回，且**必须**配 `bank = "vec"`）：
  x86 按**精确总位宽**分档 —— `width = 256` → `VMOVUPS_MR {retf_src}, {sret}`、
  `width = 512` → `VMOVUPS_ZMM_MR {retf_src}, {sret}`。
+ 修一处**形态错误**：`width` 对向量必须用**总位宽**（`type_bits_of` = `scalar_bits` 对向量给的是
  **元素**位宽）⇒ 生成器内部按 `size_bytes * 8` 折算。
+ **命中已证明**：临时把旧的 sret 分支改成报错，x86 矩阵仍 **197/3/0** ⇒ 宽向量 by-ref/sret
  确实走谱面规则（不是静默回落）。
+ 仍关着：`multi = true`（多值返回，需要「每结果一份搬运模板」）。
+ **行为不变**：`encoding.digest` 指纹通过；x86 矩阵 **197/3/0**。

### Changed (2026-10-08) — ISA-DSL v21 W6 乙（三）：按值向量返回下放（`bank = "vec"`）

+ `[lower.Ret]` 放行 **`bank = "vec", by_ref = false`**（≤16B 按值向量返回）：
  x86 写 `MOVAPS {retf}, {retf_src}`（**与旧路径在 128 位档选中的是同一条指令**，WA-37 的全宽要求不变）。
  `bank = "vec"` **必须**同时写 `by_ref`——否则会连 >16B 的 sret 形态一起匹配（安全闸会拒）。
+ riscv64/arm64 没有 128 位 FPR 搬移 ⇒ 不写该规则，旧路径照旧 fail-closed（行为不变）。
+ 仍关着：`by_ref = true`（sret，需要 sret 指针占位符）与 `multi = true`（多值，
  需要「每结果一份搬运模板」）。
+ **行为不变**：`encoding.digest` 指纹通过；x86 矩阵 **197/3/0**（含 V128 按值返回用例）。

### Changed (2026-10-08) — ISA-DSL v21 W6 乙（二）：`[lower.Ret]` 条件**数据化** + 浮点返回下放

+ `[lower.Ret].when` 不再用 `ret_kind = "int"/"f32"/…` 那种**枚举**（它把宽度写死 ⇒ f8/f16/f128、
  i8…i128、任意宽度指针都表达不了，违背「宽度是数据」），改成四个**数据键**：
  `bank = "gpr"|"fpr"|"vec"`（IR 侧封闭集）、`width = <任意位宽整数>`、`by_ref`、`multi`。
  裁决 = **具体优先**（条件键多者赢，同键数按声明序）；未知键/未知银行一律报错。
+ 规则编译成生成物里的**运行期 `if` 链**（银行/宽度是运行期事实）；**没有规则命中时逐字回落**
  到原来的四形态分派（行为不变）。
+ 三谱的 `[[lower.Ret]]` 改成数据化写法：x86/riscv64/arm64 各一条 `bank = "gpr"`，
  x86/riscv64 另加 `bank = "fpr", width = 32|64`（`MOVSS`/`MOVSD`、`FSGNJ_S`/`FSGNJ_D`）。
+ 新增两条测试：`ret_rules_compile_to_runtime_branches`（生成物里出现条件与两个银行的槽绑定）、
  `unsupported_term_lowering_is_rejected` 扩成七条（缺 `when`、未下放的 `bank`/`by_ref`/`multi`、
  未知键、`when` 挂错 op、未接线 op、三种放行）。
+ **行为不变**：`encoding.digest` 指纹通过；x86 矩阵 **197/3/0**、riscv64 矩阵 **136/64/0**
  （中途"`{retf}` 改查 plan"被 riscv64 矩阵抓到错值，按"差异先当 bug"退回旧语义，
  并把"约定层没有浮点返回槽字段"记为待办）。

### Changed (2026-10-08) — ISA-DSL v21 W6 乙：`[lower.Ret]` 上线，**整数返回下放到谱**（三谱）

+ 终结规则改成**数组**（`[[lower.<名字>]]` 同名多块按声明序累积；此前同名第二块会静默覆盖第一块），
  并新增 `when = { ret_kind = … }`：终结序列可以按 **IR 语义**分支。
+ 新增 **ABI 相对占位符**：`{ret}`/`{retf}` = ABI 返回槽（宿主约定解析成物理寄存器，缺 ⇒ fail-closed）、
  `{ret_src}`/`{retf_src}` = 承载返回值的虚拟寄存器（`map_reg_field` 绑 IR 返回值）。
+ 三份发行谱都写了 `[[lower.Ret]] when = { ret_kind = "int" }`（x86 `MOV_RM8_R64`、
  riscv64 `MOV_RM8_R64`、arm64 `MOVX`）——**与生成器旧路径选中的是同一条指令、同一操作数映射**
  （`terminator_return_packet` 等集成测试逐条印证）。
+ 安全闸：`Ret` 必须写 `when`；写还没下放的形态（float/vec16/byref/multi）或未知形态名一律拒，
  不静默不生效。其余四种返回形态仍在生成器里（分片进行中）。
+ **行为不变**：`encoding.digest` 编码字节指纹通过；x86 矩阵 **197/3/0**、riscv64 矩阵 **136/64/0**
  （与基线逐数字相同）；riscv64/arm64 的 `forge-isa insts` 与基线 **0 差异**。

### Changed (2026-10-08) — ISA-DSL v21 W4/D4 #16：`class`/`classes` 双键合一

+ `[operand.<名字>]` 的 `class` 键现在**接受组名或组名数组**：
  `class = "gpr64"` 与 `class = ["gpr16", "gpr32"]` 同键两种写法；`classes` 键**删除**
  （写了会被 `deny_unknown_fields` 拒）。内部槽仍归一成集合（`classes` 字段 +
  "恰好一个类"时的 `class` 快捷字段，两者同源）。
+ x86 两处 `classes = [...]` 已迁到 `class = [...]`；四处同改同步：`src/schema.rs`
  的 `[operand.<名字>]` 键表、`docs/reference/isa-dsl.md` 键速查表与示例、
  `docs/reference/isa-dsl-errors.md`、`docs/guides/isa-dsl-tutorial.md`、`CLAUDE.md`；
  重生成 `isa-dsl.schema.json`（`schema_guard` 9/9）。
+ 新增两条守卫：`operand_slot_class_accepts_name_or_list`（同键两写法）与
  `rejects_regular_operand_slot_classes_key`（`classes` 必须被拒）。
+ **行为不变**：`encoding.digest` 编码字节指纹通过；`forge-isa lint` 三谱零结论。

### Changed (2026-10-08) — ISA-DSL v21 W4 第三批：SSE/VEX/EVEX 三操作数族改走段形态

+ 10 处族 body 从行内 `modrm = { reg = "dst", rm = "src"|"src2" }` 改成段形式 + `bind`：
  `SSE_RR` → `SSE_RR_SEG`（5 处）、`VEX_RRV` → `VEX_RRV_SEG`（2 处）、
  `EVEX_RRV` → `EVEX_RRV_SEG`（3 处）；随之闲置的三个旧 form 一并删除。
+ x86 的 `modrm = {…}` 键 **23 → 13**（W4 累计 27 → 13）。
+ **字节逐条不变**：`encoding.digest` 指纹守卫通过；`insts` 的差异只有显示里的 form 名。

### Changed (2026-10-08) — ISA-DSL v21 W4 第二批：x86 四个内存族改走段形态

+ `[family.ALU_MR]` / `[family.ALU_R_MEM]` / `[family.ALU8_MR]` / `[family.ALU8_R_MEM]` 从
  `form = "MRR_MEMREF_AUTO"` + 行内 `modrm = { rm = "[mem]" }` 改成
  `form = "MRR_MEMREF_SEG"` + `opsize = "s0"` + `bind = { rm = "[mem]" }`；已闲置的
  `[form.MRR_MEMREF_AUTO]` 一并删除。x86 的 `modrm = {…}` 键 **27 → 23**。
+ **字节逐条不变**：`forge-isa insts` 的差异只是显示里的 form 名；新增的 `encoding.digest`
  指纹守卫**通过**（上一轮它刚拦下一次不等价迁移，见执行清单 §6）。

### Changed (2026-10-08) — ISA-DSL v21 W6-a 第三片（下）：**旧面删除**（`[[lowering]]`/`[[pattern]]` 消失）

+ **`[[lowering]]` 与 `[[pattern]]` 从谱面彻底删除**：`deny_unknown_fields` 现在直接拒它们。
  `IsaModel::lowering`/`pattern` 转 `#[serde(skip)]` 内部载体，由 `[[lower.*]]` 按名字/`match` 路由填。
+ **`op` 名单机制删除**（口径：一个名字一块，复用交给 `[family.*]`/`vary`）：`OpsField` 枚举与
  `Lowering::expand_ops` 删除，`Lowering::op` 收成 `String`（由表头填、不再进 TOML）。
+ 诊断：声明索引认 `[[lower.*]]`（键 `("lower","<名字>")`）；`section_code` 把 `lower` 映到
  `DSL-LOWER`；校验/生成的消息前缀从 `[[lowering.X]]` 改为 `[[lower.X]]`。
+ 四处同改同步：`src/schema.rs` 删两节与根键、`docs/reference/isa-dsl.md` 删键表两行并把两节改写为
  指向 `[[lower.*]]` 的短节、重生成 `isa-dsl.schema.json`、`schema_guard::INTERNAL_FIELDS` 登记
  `lowering`/`pattern`。
+ **行为不变**：三谱 `forge-isa insts` 与迁移前**逐行 0 差异**；`lint` 三谱零结论；
  `schema_guard` **9/9**；整仓门禁 exit 0。

### Changed (2026-10-08) — ISA-DSL v21 W6-a 第二片：**三谱已迁到 `[[lower.*]]`**

+ 三谱的选择层全部改成 `[[lower.<名字>]]`：**272** 个单名块 + **9** 个 `op` 名单块展开成
  **23** 块（名单序）+ **2** 个树模式块命名为 `FMA_F32`/`FMA_F64` +
  原有 **6** 个终结块（`Jmp`/`Br`）统一成同一形式 ⇒ 共 **303** 块；
  `[[lowering]]` 与 `[[pattern]]` 在三谱里**归零**。
+ **口径修正（实测逼出来的）**：表头即名字**不能是"一个 op 一块"** —— 一个 op 本来
  就有多条规则（x86 实测 `Vextract` **24** 条、`Fcmp` **16** 条、`Icmp` **11** 条）
  ⇒ `lower` 是 **名字 → 规则数组** 的映射，`[[lower.Iadd]]` **可以出现多次**
  （与设计案 §6.8 草稿里的双括号写法一致）。声明序按**每块**的源文位置还原。
+ **行为不变**：三谱 `forge-isa insts` 与迁移前**逐行 0 差异**；`lint` 三谱零结论；
  `schema_guard` **9/9**；整仓门禁 **exit 0**。
+ **过渡期**：旧面 `[[lowering]]`/`[[pattern]]` 仍被接受（其余 crate/示例里还有
  **20 + 5** 处夹具未迁）——第三片迁完夹具后连同旧键一起删。

### Changed (2026-10-08) — ISA-DSL v21 W6-a 第一片：`[lower.*]` 成为**唯一**的选择层面

+ `[lower.<名字>]` 现在**一张表三样东西**，按名字与 `match` 路由：
  ① 名字是终结 op（`Ret`/`Jmp`/`Br`/`Switch`/`Unreachable`/`Invoke`）⇒ 终结序列（只吃 `insts`，
  可用 `{true}`/`{false}`/`{target}`，仍受"只放行 `Jmp`/`Br`"的进行中安全闸约束）；
  ② 写了 `match` ⇒ 树型多指令匹配（名字是作者起的模式名）；
  ③ 其余 ⇒ op 规则（**名字就是 op 名**，等价于旧面 `[[lowering]]` + `op = "<名字>"`）。
  解析期**按源码声明序**降级（`BTreeMap` 是字母序，而声明序是裁决的最后一档），
  内部仍走 `lowering`/`pattern`/`lower_terms` 三个载体。
+ **行为不变**：三谱 `forge-isa insts` 与改动前**逐行 0 差异**；`schema_guard` **9/9**。
+ **过渡期**：旧面 `[[lowering]]`/`[[pattern]]` **仍被接受**（三谱还有 281 + 2 处未迁）——
  下一片把三谱迁到 `[lower.*]` 后连同旧键一起删。四处同改已同步
  （`src/schema.rs` ↔ `dsl/model.rs` ↔ `docs/reference/isa-dsl.md` 键表 ↔ 签入 `isa-dsl.schema.json`）。

### Changed (2026-10-08) — ISA-DSL v21 D4 #19：宿主 op 表唯一拼法（`[[ops]]` 不再接受）

+ `forge-isa lint --ops <宿主 op 表.toml>` 的读取：只认 `[[op]]`（本仓 `forge-ir/ops.toml` 的形态）。
  **复数的 `[[ops]]` 不再宽松兼容** —— 报错并点名正确拼法（v21 的"唯一拼法"口径，设计案 §7 后门 #19）。
+ 守卫：`crates/frontend/forge-isa-dsl/src/lint.rs` 的
  `host_ops_rejects_plural_spelling`（负向）+ 原有的正向用例改成 `[[op]]`。

### Fixed (2026-10-08) — ISA-DSL v21 W11 第二片：修掉"空前缀表 ⇒ 孤儿 `else`"的生成器缺口 + 变长一态夹具

+ **生成器 bug（已修）**：`codegen/vlen.rs::gen_prefix_scan_loop` 在**空前缀表**时发出的是
  `while … { let __b = bytes[__o]; else { break; } }` —— 条目为空 ⇒ `if` 分支随空表消失
  ⇒ 只剩一个**孤儿 `else`** ⇒ 生成物不是合法 Rust（预生成期报
  `expected an expression, found keyword else`，位置 `gen_file.rs:249`）。
  后果：**非 x86 的`prefix_scan` ISA（前缀表为空）完全无法接入**。修法：表空时发"只
  `break` 的循环"。
  **三份发行谱逐字节不变**（x86 表非空、走原路径；riscv64/arm64 是定宽、根本不走这条）
  —— `forge-isa insts` 对照 **0 差异**、x86 JIT 矩阵 **197 passed / 3 skipped / 0 failed**。
+ **新增 W11 第二片夹具**：`crates/backend/forge-codegen/tests/isa/demo_vlen.toml` ——
  **变长一态**（`prefix_scan` + **stream 段**）：opcode 段 + modrm/imm 段 ⇒ 指令长
  **2 或 3 字节**；与 `demo_v21`（24 位定宽）配对，凑齐 W11 的"定宽 + 一态变长各一"。
  接入面仍只有 TOML + 两行宿主 + 一个测试二进制（编译器侧只动了上面那个 bug）。
+ 证据：`forge-isa validate` **OK**、`lint` **干净**；`--test spec_tests`
  **78 passed / 0 failed**（该夹具 13 条生成期自测 + 8 条谱内向量）；
  `--test w11_vlen_tests` **5 passed / 0 failed**（长度 2 vs 3、opcode 在首字节、
  `opcode_reg` 的寄存器号在低位、两种长度都吃满的编解码/反汇编往返、截断不解）。
+ **一处如实记录的自摆乌龙**：第一版把 `opcode_reg` 的基值写成 `0x11`/`0x12` —— 低 3 位
  会被寄存器号覆盖 ⇒ 两条指令在模 8 上撞车，症状是"3 字节指令解码只吃 2 字节"；
  我起初把它当成第二处生成器缺口，逐条核对后确认是**谱写错** ⇒ 基值改为 `0x10`/`0x18`，
  教训写进该测试文件的注释（"基值低 3 位必须为 0"）。

### Added (2026-10-08) — ISA-DSL v21 W11 第一片：第四份 ISA 夹具（24 位定宽）**零编译器改动**接入

+ 新增 `crates/backend/forge-codegen/tests/isa/demo_v21.toml`：**非 x86/riscv64/arm64** 的
  第四份 ISA（24 位定宽、16 个 16 位寄存器），纯 TOML；接入面只有两行宿主
  （`tests/common/mod.rs`、`tests/spec_tests.rs`）+ 一个测试二进制 `tests/w11_isa_tests.rs`
  ⇒ **改动面 = TOML + 宿主 + 测试，零编译器改动**。
+ 该夹具刻意用上 v21 的现面：表头即名字的 `[form.*]`/`[inst.*]`/`[pred.*]`/`[asm.pseudo.*]`、
  字段就地 `fields`、`[family.*]`（`body` + `rows`）、`[machine]`/`[machine.stack]` 机器事实、
  `[reg.<名>]` 的 `prefix`+`count` 派生名字、谱内 `[[vectors]]`。
+ 证据：`forge-isa validate` **OK**；`forge-isa lint` **干净（0 结论）**；
  `forge-isa abi check` **硬错 0 / 缺口 0**；`cargo test -p forge-codegen --test spec_tests` →
  **65 passed / 0 failed**（含该夹具 **25 条**生成期自测 + **8 条**谱内向量）；
  `cargo test -p forge-codegen --test w11_isa_tests` → **6 passed / 0 failed**（字节精确、
  `decode→encode` 回环、反汇编文本再编回同样字节、伪指令 `li` ≡ `movi`、族行三实例、
  截断输入不解码）。
+ **如实记录一处手算错误**：族行两条向量的期望字节我把 `rd`/`rs1` 的高低半字节写反了，
  被**生成期自测**当场抓出（`assertion left == right` 报了实际字节）⇒ 这类谱内向量正好是
  "手写期望值"的自动核对网。
+ W11 其余（未完成）：守卫升级到"无 ISA 形状分支" —— 它依赖 W6（分支/跳转降级下放），
  以及"变长一态"的第二份夹具。

### Changed (2026-10-08) — ISA-DSL v21 W10 第四片：旧面文档与消息的死链清零

+ **代码侧消息**：`[[forms]]` / `[[forms.X]]` / `[[derive]]` / `[[derive.X]]` 一律改成新面
  `[form.<名字>]` / `[form.X]` / `[pred.<名字>]` / `[pred.X]`（`validate.rs` / `stream_decl.rs` /
  `field_decl.rs` / `model.rs` / `lint.rs` / `diag.rs` / CLI 输出，共 42 处）——文档里的示例诊断
  因此与**真实输出**一致。顺带：删掉 `validate.rs` 里已成死代码的一处
  `replacen("[[instructions]]", …)`（旧面撤掉后不会再产出"裸节名"前缀的消息）；
  `LINT-UNUSED-FORM` 的锚点改用真实表头 `[form.<名字>]`（不再用 `[[forms]] #i`）。
+ **文档**：`docs/reference/isa-dsl.md`（键表外的散文 10 处 + 2 个围栏示例）、
  `docs/reference/isa-dsl-errors.md`（示例诊断、`DSL-FORM`/`DSL-INST` 的节名、"重名改由 TOML
  自身拒绝"）、`docs/guides/isa-dsl-tutorial.md`、`docs/reference/calling-conventions.md`、
  `docs/performance/bench_baseline.md`（列头标注"当时写 `[[instructions]]`"）、
  `crates/tools/forge-tests/asm/README.md`（5 处）全部对齐 v21 面。
+ **loader 语义段更正（实质内容，不只是改名）**：`[form.<名字>]`/`[inst.<名字>]`/`[pred.<名字>]`
  不再是数组节 ⇒ 文档里"数组节按 include 序追加"的说法改为"数组节
  （`[[operand_slots]]`/`[[templates]]`/`[[vectors]]`…）按 include 序追加；**表头即名字的节**
  按合并文本里的**出现位置**决定声明序（解析期按源文本序恢复）"。
+ **刻意保留**：明确标注为历史的提法（"v18 S3f 引入时叫 `[[derive]]`"）与历史测量表。
+ 证据：`forge-isa-dsl` 整包全绿；`schema_guard` 9/9；`markdownlint` 0 issue；三谱 `insts`
  与迁移前**逐行相同**；整仓 `-j 2` → exit 0、0 编译错误、0 失败二进制。

### Changed (2026-10-08) — ISA-DSL v21 W10 第三片：`[[derive]]` → `[pred.<名>]`（表头即属性名）

+ 命名谓词改成**表头即属性名**（设计案 §6.7）：`[[derive]]` + `name = "is_64"` 改写为
  `[pred.is_64]`，`expr` 保留；旧写法按 `deny_unknown_fields` 报未知键。`DeriveDef.name` 转为
  **内部载体**（`#[serde(skip)]` + `INTERNAL_FIELDS` 登记），解析期按**源码声明序**降级，且必须
  早于 `expand_derives`（`vary` 的"键是谓词属性还是替换变量"判定要看派生名）；`pred_decls` 保留
  ⇒ `serialize → parse` 往返无损。
+ **连带口径变化**：重名谓词由 **TOML 自身**拒绝（`DSL-TOML` / `duplicate key`），不再有编译期
  "名字重复"诊断；`DSL-DERIVE` 仍负责"与核心属性重名 / `expr` 非法 / 引用未知属性或另一个派生"。
+ 规模：**三谱 0 处**（发行 ISA 都没用 `[[derive]]`）⇒ 只迁测试内联谱 **9 处**；schema 节
  `[pred.<名字>]`（必填 `expr`）、根键 `derive` → `pred`、`INTERNAL_FIELDS` 两条；文档键表
  （根行 + 本节行）、小节标题与目录锚点、错误码目录同步。
+ 证据：`forge-isa-dsl` 整包全绿；`schema_guard` 9/9；`markdownlint` 0 issue；三谱 `insts` 与
  迁移前**逐行相同**（发行 ISA 生成物零变化）；整仓 `-j 2` → exit 0、0 编译错误、0 失败二进制。
+ W10 其余（下一步）：**W4 stream 段模型**（x86 段化）、文档面（`CLAUDE.md` 回写 / 设计文档
  archive 标注 / `git grep` 死链清零）。

### Changed (2026-10-08) — ISA-DSL v21 W10 第二片：`[[instructions]]` → `[inst.<名>]`（表头即指令名）

+ 指令改成**表头即指令名**（设计案 §6.6）：`[[instructions]]` + `name = "ADD"` 改写为 `[inst.ADD]`；
  旧写法按 `deny_unknown_fields` 报未知键。`Instruction.name` 转为**内部载体**
  （`#[serde(skip)]` + `INTERNAL_FIELDS` 登记），解析期按**源码声明序**填充（声明序决定别名
  首匹配与解码分派）；`inst_decls` 保留 ⇒ `serialize → parse` 往返仍然无损。
+ **连带口径变化（如实记录）**：表头即名字之后，"重名指令"不再有编译期诊断——**TOML 自身**拒绝
  重复表头（`DSL-TOML` / `duplicate key`）。相关守卫与夹具按新口径改写：矩阵用例"指令重名"的
  期望码由 `DSL-INST` 改为 `DSL-TOML`；`validation_duplicate_instruction` 改断言 `Parse`；
  `duplicate_declaration_points_at_both_lines` / `headline_positions_are_exact` 改断言
  **第二处表头的行号**；`three_independent_errors…` 的第三条改成"未声明操作数槽"；
  `loader::array_of_tables_appends_in_include_order` 改为查**合并文本里的出现位置**（表本身无序，
  声明序由解析期按源文本恢复）。`diag::section_code` 新增 `inst` → `DSL-INST` / `form` → `DSL-FORM`
  （否则新前缀的诊断码会退化成 `DSL-OTHER`）。
+ 迁移规模：三谱 **640 块**（arm64 348 / x86 200 / riscv64 92）+ `demo*`/`include_*` 夹具 **40 块** +
  源码内联谱 **99 处** + 示例宿主 4 + 教程 5；报错/断言文案的点号形态（含 `{}` 占位符形态）
  另用脚本批量改 **93 处**。
+ **迁移脚本的一个致命 bug（已修，如实记录）**：脚本按块处理时把"表头行 ⋯ `name` 行"**整体压成一行**，
  于是**写在 `name` 之前的键被静默丢掉**。x86 恰好有 3 条这样的指令（`CALL_RM` / `SETCC_RM8` /
  `SETCC_RM8_B`：`match = { reg = 2 }` 在 `name` 之前）⇒ `modrm` 的固定扩展码丢失 ⇒
  **x86 JIT 矩阵跑出 `STATUS_ILLEGAL_INSTRUCTION`**。修法：只替换表头行、只删 `name` 行、其余逐行搬。
  验收方式：**三谱 `forge-isa insts` 输出与迁移前逐行相同（差异 0 行）+ x86 矩阵回到 197/3/0**。
  （`[[forms]]` 那片经同法复核：0 个 form 块在 `name` 前有键，未受影响。）
+ 证据：`forge-isa-dsl` 全绿（**295 通过 / 0 失败**）；`schema_guard` 9/9；`markdownlint` 0 issue；
  整仓 `-j 2` → exit 0、0 编译错误、0 失败二进制。
+ W10 其余（下一步）：`[[derive]]` → `[pred.<名>]`、**W4 stream 段模型**、文档面
  （`isa-dsl-errors.md` 同步 / `CLAUDE.md` 回写 / 设计文档 archive 标注 / `git grep` 死链清零）。

### Changed (2026-10-08) — ISA-DSL v21 W10 第一片：`[[forms]]` → `[form.<名>]`（表头即形式名）

+ 编码形式改成**表头即形式名**（设计案 §6.5）：`[[forms]]` + `name = "R"` 改写为 `[form.R]`；
  旧写法按 `deny_unknown_fields` 报未知键。`Form.name` 转为**内部载体**（`#[serde(skip)]` +
  `INTERNAL_FIELDS` 登记），解析期按**源码声明序**填充（`BTreeMap` 是字母序，而声明序会影响
  别名/lowering 的解析结果——与 `[family.<名>]` 同一教训）。
+ **模型的"面"保留**（`form_decls` 不 `take()`）⇒ `serialize → parse` 往返仍然**无损**
  （`forms` 只是解析后的内部视图；这一步是实测发现"往返丢形式"后改的）。
+ 迁移：三谱 **86 块**（arm64 38 / x86 36 / riscv64 12）+ `demo*`/`include_base` 夹具 **25 块** +
  内联谱 **59 处**（脚本 `target/tmp/form_rename.py`：**先 dry-run 逐文件复核计数与样例**再应用；
  带行尾注释的 `name` 行把注释保留成独立注释行，不丢信息）。
+ 连带：schema 节 `[form.<名字>]`（必填为空、可选 `fields`/`segments` + 编码键 flatten）、
  根键 `forms` → `form`、`schema_guard` 的 `INTERNAL_FIELDS` 两条；DSL 内部夹具与断言同步
  （`diag` 三条行号、`lint` 三处 replace 模式、`tests` 的往返）。
+ 证据：`forge-isa-dsl` 全绿（**295 通过 / 0 失败**）；`schema_guard` 9/9；`markdownlint` 0 issue；
  整仓 `-j 2` → exit 0、0 编译错误、0 失败二进制。
+ W10 其余（下一步）：`[[instructions]]` → `[inst.<名>]`、`[[derive]]` → `[pred.<名>]`、
  W4 stream 段模型、文档换版与 `git grep` 死链清零。

### Changed (2026-10-08) — ISA-DSL v21 W9 第四片：删掉 callee-saved 两键（序言改为按 plan 整表推）

+ `[machine].callee_saved_gpr` / `callee_save_slots` **删除**（写它们报未知键）：callee-saved
  完全归**约定数据**（`AbiPlan`/绑定），谱面不再有"帧按几个推入槽算"这份事实。
+ **序言/尾声按 plan 的整张表推**：regalloc 的 **GPR** 那半不再过滤"只存实际分配到的"，
  `AllocResult.callee_saved_to_save` = plan 的整表 ⇒ 帧基准（plan）与运行时保存数**同源**，
  `emission` 的 `cs_skipped` 补偿与生成期 `__cs_bytes` 的静态表长一并消失（后者改读
  `__rm.callee_saved_to_save.len()`）。**FPR 那半仍按需**——发射侧"按类保存 FPR"的链条靠这个
  过滤当闸门（无条件开启实测让 arm64 的 QEMU 用例卡死）；帧顶槽位只要求"装得下整张 plan 表"。
+ 连带：`frame_layout` 的 fp-inside/outside 都按 plan 计数；`compiler.rs` 的无 plan 回退改空表
  （A6 fail-closed）；`abi_target_real` 的 ①（plan vs 谱表）改为"主约定非空"的结构性断言；
  生成物 `RegInfo::callee_saved()` 返回空表、`callee_save_slots()` 走 trait 缺省（= 0）。
+ 已知代价（如实记录）：**小函数会多推几个 GPR**（riscv64 lp64d 平函数：ra/fp + 少量 s 系 → 13 条 SD/LD），
  `riscv64_tm_tests` 的"按需保存"断言按新语义改写；换来"计数/名字同源、不可能再出现半迁移的静默破坏"。
+ 证据：整仓 `-j 2` → exit 0、0 编译错误、0 失败二进制（含 riscv64 QEMU 矩阵与 arm64 全套）；
  **x86 JIT 矩阵 197 passed / 3 skipped / 0 failed**；arm64 QEMU 通道 4 passed；`schema_guard` 9/9；
  `markdownlint` 0 issue。
+ **W9 完成**：`[machine]` 合并四片（键收短 / 三节迁入 / spill 按 class 分派 / 删 callee-saved 两键）全部落地。

### Changed (2026-10-08) — ISA-DSL v21 W9 第三片：`[machine.spill.<模板名>]` 按 `class` 分派

+ `[spill.<名>]` → `[machine.spill.<模板名>]`：表头 = **模板名**（不再是 `GPR`/`FPR<bytes>`
  命名合同），新增**必填** `class` = 它服务的寄存器类（已声明 `[reg.<名>]` 组名，或
  `[meta].vector_tiers` 的向量档如 `vec512`）；`only_variants` → `when`。旧键报未知键。
+ 分派改按 **class 的族与宽度**：GPR 族取一份（多份取最宽者）；浮点/向量族 = 宽度 ≤ 浮点值池的
  **最宽**者作标量缺省 + 更宽各一份档位（缺档 fail-closed，不退回窄搬运）⇒ 与旧的
  `GPR`/`FPR`/`FPR16/32/64` 逐字节等价。x86 五份、arm64 三份、riscv/demo8 各一份模板一次迁完。
+ 新增校验：`class` 必须已声明（组名或向量档）——写错 = 校验期错误（不再静默 no-op）；
  R5 覆盖校验改按 class 宽度判（比浮点值池更宽的类必须有服务它的模板）。
+ 诊断侧支持**两段节名**：`[machine.spill.fpr].load` → `DSL-SPILL`、`[machine.stack]` →
  `DSL-STACK`、`[machine.classes]` → `DSL-TYPES`、`[machine.text]` → `DSL-EMIT`，
  且"块内精准定位到具体键"照常生效（旧的一段键索引保持不变，纯加法）。
+ 证据：`forge-isa-dsl` **294 通过 / 0 失败**；整仓 `-j 2` → exit 0、0 编译错误、0 失败二进制；
  `schema_guard` 9/9；`markdownlint` 0 issue。
+ W9 剩余：删 `callee_saved_gpr`/`callee_save_slots`（改走 ABI 层）。

### Changed (2026-10-08) — ISA-DSL v21 W9 第二片：`[machine.stack|text|classes]` 三节迁入（stack 单位改**位**）

+ `[stack]` → `[machine.stack]`（**单位由字节改成位**：三份发行谱写 `slot = 64 / align = 128 /
  fp_save = 64`，`demo8` 写 `8/8/8`）、`[emit]` → `[machine.text]`、`[types]` → `[machine.classes]`。
  三个旧键按 `deny_unknown_fields` 报未知键（无迁移期双读）。
+ 解析期降级进**内部载体**（`IsaModel.{stack,emit,types}` 改 `#[serde(skip)]` + `INTERNAL_FIELDS`
  登记）：位的硬检查只有"必须是 8 的整数倍"（`0` 交回校验期报 `must be > 0`），换算成字节后与旧口径
  **逐字节一致** ⇒ 生成物零变化。`[machine]` 的键表纳入三个子表，新增面结构体 `MachineStack`。
+ 同步：三份发行谱 + `demo8`/`demo` 夹具；诊断锚点用例（`[machine.stack]`/`[machine.text]` 节头定位）；
  DSL 内联谱用例（`[emit]`/`[stack]`/`[types]` 共 5 处）；校验消息改点名 `[machine.stack].*` 与
  `[machine.classes].*`；文档键表五行 + 正文/示例/目录锚点（锚点实测，如
  `#machinestack--栈与对齐v21-w9`）、`isa-dsl-errors.md` 的 `DSL-STACK`/`DSL-TYPES` 行。
+ 证据：`schema_guard` 9/9；整仓 `-j 2` → exit 0、0 编译错误、0 失败二进制；`markdownlint` 0 issue。
+ W9 剩余：`[spill.*]` → `[machine.spill.<组名>]` 按组分派；删 `callee_saved_gpr`/`callee_save_slots`
  （走 ABI 层）。

### Changed (2026-10-08) — ISA-DSL v21 W9 第一片：`[machine]` 键收短（`fixed` / `scratch` / `link` / `padding` / `arg_place` / `byref_bytes`）

+ **破坏性键改名**（设计案 §6.10 目标形状）：`[machine].fixed_regs` → `fixed`、
  `spill_scratch` → `scratch`、`link_reg` → `link`、`frame_padding` → `padding`、
  `arg_slot` → `arg_place`、`vector_by_ref_bytes` → `byref_bytes`。旧键现在按
  `deny_unknown_fields` 报未知键（无迁移期双读）。
+ 三份发行谱 + `demo8` 夹具一次迁完；`model.rs` 字段与访问器、schema 键表、校验与
  生成期 fail-closed 消息（`[machine].scratch` / `[machine].link`）、`abi_view` 的 notes、
  负向用例（`diag_matrix_tests` 的 `DSL-ABI` 改键串）全部同步。
+ 文档同步：`docs/reference/isa-dsl.md` 键表行与三处正文、`calling-conventions.md` 的
  `MachineView` 输入说明、`isa-dsl-errors.md` 的 `DSL-ABI` 行、运行时 `machine/abi.rs` 与
  两份测试辅助的注释。
+ **零行为变化**（纯改名）：整仓 `-j 2` → exit 0、0 编译错误、0 失败二进制（含 riscv64 QEMU
  矩阵与 arm64 全套）；`markdownlint` 0 issue；`schema_guard` 9/9。
+ W9 其余部分（`[stack]`/`[emit]`/`[types]` → `[machine.stack|text|classes]` 且单位改位、
  `[spill.*]` → `[machine.spill.<组名>]` 按组分派、删 `callee_saved_gpr`/`callee_save_slots`）
  仍未做，见执行清单 §11。

### Changed (2026-10-08) — ISA-DSL v21 W8 第三片：重定位补丁器**由谱面生成**（宿主分发与两个手写补丁器删除）

+ 生成器新增 `gen_reloc_patcher`：定宽 ISA 发射 `RelocPatcherImpl` —— 读回指令字 →
  `decode_partial` 还原指令 → 把 `d = target − site + adj` 写进该变体的 label 槽字段
  （有 `reloc` 声明时用声明的 `operand`，否则用 `Label` 槽）→ `encode` 重放整字。
  散布位段复用编码器**同一份字段放置代码**；臂按**槽名**匹配（riscv 的 `AUIPC`/`AUIPC_GLOBAL`
  共享编码，解码只会还原出先声明的那条）。
+ 生成物在 `impl TargetMachine` 里覆写 `reloc_patcher()`；`register_default_reloc_patcher`
  （按 ISA 名的懒惰注册分发）、`RiscvRelocPatcher`、`Arm64RelocPatcher`（含单测）整条删除。
  宿主 `reloc_patcher.rs` 现在只剩 trait + 显式注册表 + 通用路径的两条钉子测试。
+ 新键 `[asm.reloc.<名>].via`：值先过该 `[asm.immfn.<名>]` 的 `expr`（riscv 复用已声明的
  `hi`/`lo`，谱面不再需要宿主为它写代码）；`addend` 改为经 `RelocKind::Relative` 的修正字段
  进补丁器（riscv `pcrel_lo` 写 `addend = 4`），即它**参与**补丁值。
+ 覆盖迁移：删掉的手写补丁器单测改在**生成物**上重建
  （`crates/backend/forge-codegen/tests/reloc_patcher_generated.rs` 9 条：riscv J/U/B 散布位段、
  hi20/lo12 变换、占位值必须被覆盖、arm64 imm26/imm19、不支持的 kind 报错、报错时不动字节）。
+ **已知边界（明确报错，不静默）**：识别靠解码 ⇒ `imm = 0` 的 `addi` 形状会命中 riscv 自己的
  `MOV_RM8_R64`/`Nop`（都无 imm 操作数）⇒ 报错；真实 GlobalAddr 的 imm 是负占位值，走 `Addi`，
  不受影响（riscv64 JIT 矩阵覆盖）。
+ 证据：整仓 `-j 2` → exit 0、0 编译错误、0 失败二进制（含 riscv64 QEMU 矩阵与 arm64 全套）；
  **x86 JIT 矩阵 197 passed / 3 skipped / 0 failed**。

### Changed (2026-10-08) — ISA-DSL v21 W8 第二片：删掉 x86 的**冗余** RelocPatcher

+ `X86RelocPatcher` 与 `CodeSink::finish` 的通用路径**逐字节等价**（同样的 `kind.encode_value`
  平铺小端写入、同样的 `Relocation` 记录、同样的 site 语义）⇒ 该结构体连同注册表里的
  `"x86_64" | "x86"` 分支**整条删除**（x86 改走通用路径）。
+ 于是"按 ISA 名分发"只剩 riscv64/arm64 两支（W8 后续：改为**按谱面字段生成**散布重排后，
  连这两支与两个手写补丁器一起删——`u12[12:1 -> 11:8,30:25,7,31]` 这类散布映射**已是谱面数据**，
  生成侧已有依据）。
+ 两条 x86 单测改为钉住**通用路径**（`CodeSink`：rel32 = target − site + adj；ABS8 平铺 LE），
  覆盖不降级；两处再导出（`forge-codegen`、根 facade）同步。
+ 证据：整仓 `-j 2` → exit 0、0 编译错误、0 失败二进制；**x86 JIT 矩阵 197 passed / 3 skipped /
  0 failed**（与既有基线一致）。

### Changed (2026-10-08) — ISA-DSL v21 W8 第一片：`[asm.reloc.<名>]` 取代 `[[reloc]]`

+ 重定位表搬进 `[asm]` 家族：**表头即重定位名**，键改为 `semantics` / `operand`（原 `slot`）/ `addend`
  —— 与 `[asm.pseudo.<名>]` / `[asm.immfn.<名>]` 同一形态（设计案 §6.9）。
+ 解析期降级进内部 `IsaModel.reloc`（表头 → `name`、`operand` → `RelocDef.operand`）；旧键 `[[reloc]]`
  成为内部载体（`serde(skip)` + `INTERNAL_FIELDS` 登记），schema 的根键与 Section 同步撤换。
+ 三谱 **3 条**一次性搬迁（x86 `abs64`；riscv64 `pcrel_hi` / `pcrel_lo`）＋ 测试夹具 **8 处**迁移；
  “名字重复”一例改为断言 **TOML 重复表头被拒**（表头即名字的连带效果）。
+ 报错文案改指新面（`[asm.reloc.<名>]`）；文档键表两行、目录锚点、正文小节与两处散文同步。
+ 整仓 `-j 2` → exit 0、0 编译错误、0 失败二进制（本次是 surface-only：生成物逐字节不变）。
+ **W8 的正题未做**（留下一片）：按谱面字段位区间**生成**补丁器 ⇒ 删掉宿主侧按 ISA 名的
  `RelocPatcher` 分发与两个手写补丁器（`reloc_patcher.rs:77-84`、`Arm64RelocPatcher`/`RiscvRelocPatcher`）。

### Added (2026-10-08) — ISA-DSL v21 W7 第二十片：`{size}` 渲染尺寸关键字（机制就位）

+ 已定规则：宽度（字节）= `size_words` 下标 ⇒ `1 << i`。渲染器改为收一个宽度参数
  （`__render_mem(m, size_bytes)`），`Comp::Size` 按宽度查关键字表：命中则**连同其条件前缀**
  一起输出；未知宽度（0）或谱未声明关键字表 ⇒ 空串（不谎报尺寸）。
+ 两处调用点随签名改：`codegen` 生成处传 `size_keywords(&model.conventions.mem)`；
  `asm` 渲染处传操作数槽位宽（`slot.width / 8`，未声明 ⇒ 0）。
+ **零行为变化**：三谱的 `[operand.mem]` 都没有声明 `width` ⇒ x86 的 `{size}` 仍渲染空串，
  汇编棘轮与全部测试不变（整仓 `-j 2` 全绿，`corpus_parse_counts_match_the_ratchet` 通过）。
+ **实测结论（如实记录）**：x86 的内存访问宽度是**逐指令**的（`movzx` byte/word、`mov` qword…），
  而 `[operand.mem]` 只是槽声明、不带宽度 ⇒ 要让 x86 真的打印 `qword ptr`，需要给相关槽/指令补
  宽度声明（或把逐指令的 `data_width`/`opsize` 穿进渲染）——那属于**谱面与设计取舍**，
  不在本片擅自决定。

### Changed (2026-10-08) — ISA-DSL v21 W7 第十九片：删掉“紧邻 `{size}` 字面量例外”

+ `mem.rs` 渲染分支里那条特例（紧跟在 `{size}` **前面**的字面量恒发出）已删除：`{size}` 现在与其它
  组件走同一套规则——其后的字面量是它的条件前缀，组件缺席则前缀也不发出。
+ **零行为变化（门禁可证）**：三谱里只有 x86 用 `{size}`，且它位于模板首位（前面没有字面量），
  因此本条改动不产生任何输出差异——整仓 `-j 2` 全绿，没有任何测试变化。
+ 同步更新 `mem.rs` 模块文档与 `docs/reference/isa-dsl.md` 的那句说明。
+ 仍待做（本清单最后一项）：`{size}` 渲染出**尺寸关键字**（已定：宽度 = `size_words` 下标 ⇒
  `1 << i` 字节）。它需要把操作数位宽穿进 `__render_mem`（现签名只收 `&MemRef`）并给渲染器
  一张关键字表，会改变三谱若干反汇编文本 ⇒ 需重刷汇编棘轮与相关黄金，故单独成片。

### Added (2026-10-08) — ISA-DSL v21 W7 第十八片：`{n?}` 解析半边（缺席绑 0，只允许 imm 槽）

+ 语义已定：整块缺席 ⇒ 该字段填 **0**；且 `{n?}` **只允许**用在 `kind = "imm"` 的槽上。
+ 解析侧（`gen_assemble_try_tok`）：`{n?}` 的元素改成“永不失败”——
  `__imm(&mut it, float).unwrap_or(0)`，模式直接绑字段；于是同一条写法既能匹配带该操作数的文本，
  也能匹配省略它的文本（与渲染半边“值为 0 ⇒ 不输出”对称）。
+ 声明期校验（`validate_segs`）：非 imm 槽写 `?` 直接报错（寄存器/内存没有缺省值，
  不留运行期的静默行为）。
+ 测试三条（自建含 imm 槽的最小谱 `optional_placeholder_doc`）：渲染条件分支 `__p1`/`__l1`、
  解析绑 0、非 imm 槽报错。
+ 坑（写进测试注释）：`validate_segs` 属 **codegen**，`parse_and_validate` 不跑它 ⇒ 负例必须断言
  `generate()` 的报错——与 `temps` 那片是同一个坑。
+ 整仓 `-j 2` → exit 0、0 编译错误、0 失败二进制（三谱未用 `?`，生成物逐字节不变）。

### Added (2026-10-08) — ISA-DSL v21 W7 第十七片：`{n?}` 渲染语义（按取值缺省省略）

+ 语义已定：`{n?}` = **按取值缺省**省略——渲染文本为 `"0"` 时，该组件连同**紧邻的字面词缀**
  一起不输出（判据在数据里，不需要新声明）。这是 W7 清单“占位修饰”一条的**渲染半边**。
+ 实现（`gen_disassemble`）：`{n?}` 生成在场判定 `__p{n}` 与条件词缀 `__l{n}`——把紧邻字面
  从静态格式串里摘出来，改由 `__l{n}` 条件输出；配 `{n:affix}` 时组件缺席则词缀也不出现
  （`+` 对已带 `-` 的抑制照旧）。
+ 未写 `?` 的占位符走原路径，生成物逐字节不变（三谱未用 `?`，整仓门禁全绿）。
+ 单测 `dsl::tests::optional_placeholder_renders_conditionally`（生成物含 `__p1` / `__l1`）。
+ **解析半边未做**：允许整块缺席意味着要为指令字段**补一个缺省值**，属解析器构造语义，
  留下一片；`{size}` 改为一律输出尺寸关键字（已定）也留下一片——那片会改三谱反汇编文本，
  需重刷汇编棘轮与黄金值。
+ 整仓 `-j 2` → exit 0、0 编译错误、0 失败二进制。

### Changed (2026-10-08) — ISA-DSL v21 W7 第十六片：撤掉旧 `[[conventions.imm_fn]]` 面

+ 旧键撤除：`Conventions.imm_fn` 改成 `#[serde(skip)]` 的内部载体（在 `INTERNAL_FIELDS` 登记），
  schema 去掉 `[[conventions.imm_fn]]` Section（键表行随动），文档该小节整段改写为
  `[asm.immfn.<名>]`（标题、TOML 示例、要点表，并新增“多拼写”一行说明）。
+ 测试夹具迁移：`imm_fn` 形状组（“名字重复/为空”两例改为说明它们现在由**表头**本身管）、
  多拼写组（新面只有 `texts`）、`imm_fn_absent_keeps_generated_code_unchanged` 的声明一处，
  以及“与旧面同名报错”改为“旧面被拒”。
+ 结果：`[asm]` 家族两张表（`pseudo` / `immfn`）都只剩新面——全仓 `[[conventions.imm_fn]]` 残留 **0**，
  `[[pseudo]]` 残留 **0**。
+ 整仓 `-j 2` → exit 0、0 编译错误、0 失败二进制；三谱 `validate` 通过。

### Changed (2026-10-08) — ISA-DSL v21 W7 第十五片：55 条 `imm_fn` 搬进 `[asm.immfn.<名>]`

+ 三谱与夹具的 `[[conventions.imm_fn]]` **全部搬迁**：arm64 **49** + riscv64 **4** + 夹具
  `crates/backend/forge-codegen/tests/isa/demo.toml` **2** = **55 条**，改成 `[asm.immfn.<名>]`
  （**表头即修饰名**、体内 `texts`/`expr`；单写法也写成单元素 `texts`，与设计案 L743 一致）。
+ 全仓残留 **0** 条；三谱 `validate` 通过；整仓 `-j 2` → exit 0、0 编译错误、0 失败二进制
  （两面对等期间生成物逐字节不变）。
+ 本片**只改数据**，不动生成器；旧面 `[[conventions.imm_fn]]` 仍被接受，**撤旧键**留下一片
  （模型 `Conventions.imm_fn` 标 `skip` + `INTERNAL_FIELDS` + schema/文档去 Section/行 + 测试夹具迁移）。

### Added (2026-10-08) — ISA-DSL v21 W7 第十四片：`[asm.immfn.<名>]` 新居（表头即修饰名）

+ 新增 `[asm.immfn.<名>]`（按设计案 §6.9 与迁移表 L743）：**表头就是修饰名**，体内键 = `texts` / `expr`
  —— 与 `[asm.pseudo.<名>]` 同一形态，共用 `AsmSection` 与"降级进内部载体"的手法。
+ 解析期降级进内部 `Conventions.imm_fn`（表头 → `name`、`texts` → `ImmFnDef.texts`），
  与旧面 `[[conventions.imm_fn]]` **同名报错**；生成器与校验继续只读内部表示。
+ `asm` 根键在 schema 里改为**自由表**（它下面挂多张按名字索引的表），两张表各有一条
  Section 与一条文档键表行。
+ 单测 `dsl::tests::asm_immfn_section_lowers_to_conventions_imm_fn`（新居可降级 + 与旧面同名报错）。
+ **本片是"加新居"**：旧面仍被接受（搬迁未做）——三谱 **55 条**（arm64 49 / riscv64 4）加夹具 2 条
  的搬迁与旧键撤除留下一片。
+ 整仓 `-j 2` → exit 0、0 编译错误、0 失败二进制（三谱未用新居，生成物逐字节不变）。

### Changed (2026-10-08) — ISA-DSL v21 W7 第十三片：撤掉旧 `[[pseudo]]` 面（只留 `[asm.pseudo.<名>]`）

+ 承接第十二片：三谱 **22 条**伪指令（x86 20 / riscv64 2）全部搬进 `[asm.pseudo.<名>]`（表头即名字），
  夹具谱 `crates/backend/forge-codegen/tests/isa/demo.toml` 的 2 条一并搬迁 ⇒ 全仓 `[[pseudo]]` 残留 **0**。
+ **旧键撤除**：`IsaModel.pseudo` 改成 `#[serde(skip)]` 的内部载体（并在 `INTERNAL_FIELDS` 登记），
  schema 根键去掉 `pseudo`、旧 `[[pseudo]]` Section 换成 `[asm.pseudo.<名>]`（模型 `PseudoBody`），
  键表两行与文档该小节（含 TOC 锚点）同步改写 ⇒ 写 `[[pseudo]]` 现在解析期即被拒。
+ 伪指令回归测试的夹具全部迁移（15 处）：“名字重复”一例改为说明它现在由 TOML 重复表头拒绝；
  新增/保留 `asm_pseudo_section_lowers_to_pseudo`（新居可解析并降级 + 旧键被拒）。
+ 踩坑（两个都实测）：① 撤 Section 时漏了 `schema.rs` 根键→Section 映射表里的
  `("pseudo", "[[pseudo]]", …)`，`forge-codegen` 的 build script 直接 panic；② 文档键表两行必须与
  schema 的 `doc` 逐字一致，任一侧不同即红。
+ 另修本片连带的两处文档/模式泄露：`[lower.<终结 op>]` 的 schema `doc` 与键表行曾同时含同一标记字符，
  因此守卫“侥幸通过”——两侧已一并清干净。
+ 整仓 `-j 2` → exit 0、0 编译错误、0 失败二进制；三谱 `validate` 全通过。

### Added (2026-10-08) — ISA-DSL v21 W7 第十二片：`[asm.pseudo.<名>]` 新居（表头即名字）

+ 新增 **`[asm]`** 节（v21 §6.9）与首张表 **`[asm.pseudo.<名>]`**：**表头就是伪指令名**
  （`name` 不再是字段），体内键 = `asm` / `emit` / `only_variants` —— 与 `[family.<名>]` 同一形态。
+ 解析期**降级**进既有内部载体 `IsaModel.pseudo`（与 `[family.<名>]` → `templates` 同一手法）：
  生成器与校验只见内部表示，不留第二份事实；与 `[[pseudo]]` **同名 ⇒ 报错**（名字必须唯一）。
+ 单测 `dsl::tests::asm_pseudo_section_lowers_to_pseudo`：新居可解析并降级 / 与旧面同名报错。
+ 三方针同步：模型（`AsmSection`/`PseudoBody` + `IsaModel.asm`）+ schema 根键 `asm`
  + 文档键表 `<root>` 行 + 重生成 `isa-dsl.schema.json`。
+ **本片是"加新居"**：旧 `[[pseudo]]` **仍被接受**（迁移尚未做）⇒ 三谱 22 条（x86 20 / riscv64 2）
  的搬迁与旧键撤除留下一片；`[asm.reloc.*]` / `[asm.immfn.*]` 两张表亦未落地（属 W8 与后续）。
+ 整仓 `-j 2` → exit 0、0 编译错误、0 失败二进制（三谱未用新居 ⇒ 生成物逐字节不变）。

### Changed (2026-10-08) — ISA-DSL v21 W7 第十一片：arm64 `imm_fn` 折叠**完成**（98 → 49）

+ 承接第十片：剩余 31 对全部折叠 ⇒ arm64 `[[conventions.imm_fn]]` **98 → 49**
  （`texts` 49 条、残留单写法 `text` **0** 条），`validate` 通过、整仓 `-j 2` exit 0。
+ 清单 §9 的"arm64 98 个 `imm_fn` → 约 49 个名字"**达成**。等价性依据同前：98 条 `expr` 全为
  恒等 `"{0}"`、两种拼写首 token 不同（`#` vs `:`）⇒ 不互相遮蔽、与尝试顺序无关；
  另有 arm64 的 `[[vectors]]` 89 条 + 生成期自测 + 汇编三档棘轮兜底。
+ 本片只改**数据**（`isa/arm64.toml`），未动生成器。

### Changed (2026-10-08) — ISA-DSL v21 W7 第十片：arm64 `imm_fn` 多拼写折叠（进行中）

+ 用新 `texts` 把 arm64 **成对**的 `imm_fn` 折叠：`abs_g0`（`#:abs_g0:{0}`）与 `abs_g0_n`
  （`:abs_g0:{0}`）这类"只差开头 `#`"的两条 ⇒ 合成一条声明收两种写法
  （98 条 → 目标约 49 条；**本片折掉 18 条**，余 80 条下次继续）。
+ **踩到的真风险**（值得记）：折叠会**删掉** `*_n` 名字，而 `[operand.<槽>]` 的
  `symbol = { allow = …, modifiers = [...] }` 过滤器**按名字**引用它们 ⇒ 折叠后 arm64
  **validate 直接红**。已把这四处 `modifiers` 列表里的 `*_n` 条目去掉（基名已覆盖该拼写）。
  教训：侦察"谁引用了这些名字"时 `grep 'imm_fns = \['` **不够** —— v21 W3 已把过滤声明合并进
  `symbol = { modifiers = [...] }`。
+ 折叠**可证等价**：98 条 `expr` 实测**全为恒等** `"{0}"`（98/98），且 `#`/非 `#` 两种拼写
  首 token 不同 ⇒ 不会互相遮蔽、匹配结果与顺序无关。
+ 整仓 `-j 2` → exit 0、0 编译错误、0 失败二进制。

### Added (2026-10-08) — ISA-DSL v21 W7 第九片：`[[conventions.imm_fn]]` 支持**多拼写** `texts`

+ `ImmFnDef` 的 `text` 变为可选、新增 `texts = ["…", "…"]`（同一语义的**多条等价写法**）：
  两者**只能给一个**、至少给一个（校验期报错）；`all_texts()` 给出全部写法。
+ 生成期改为**两相**：① 值语义（`expr`）每条声明**一次** —— 多拼写共享同一个求值函数
  `__imm_fn{i}` 与同一个下标；② 文本形态**每条写法各一次匹配尝试**（`__imm_fn_lit`）。
  ⇒ 多拼写不会产生第二份值语义，也不会改变任何既有写法的匹配结果。
+ 单测 `dsl::tests::imm_fn_texts_multi_spelling`：多拼写合法 / `text` 与 `texts` 同时给报错 /
  两者都不给报错。
+ 三方针同步：模型 + schema（`required` 去掉 `text`、`optional` 加 `text`/`texts`）
  + 文档键表该行 + 重生成 `isa-dsl.schema.json`。
+ 整仓 `-j 2` → exit 0、0 编译错误、0 失败二进制（三谱未用 `texts` ⇒ 生成物逐字节不变）。
+ **下一片**：用 `texts` 把 arm64 的 98 条 `imm_fn` 折叠成约 49 条（成对的 `abs_g0` 与
  `abs_g0_n` 只差开头的 `#`）。

### Changed (2026-10-08) — ISA-DSL v21 W7 第八片：emit 路径的内存偏移**统一取占位符注册表**

+ `integration.rs::parse_mem_template`（清单点名的"私有内存语法"）原先**手抄**了偏移符号语义
  （`{off}` → `ctx.current_offset`、`{alloca}` → `ctx.current_alloca_offset as i64`）——
  与 `placeholder.rs` 的注册表条目**逐字重复**。现在改为调 `placeholder::ctor_for(name, slot)`
  （该路径**有真实 `slot`** ⇒ 无需伪造）⇒ 偏移符号语义**单点**。
+ 实测使用面：这条语法**只有两处**在用 —— `isa/x86.toml` 的 `[RBP+{off}]` 与 `[RBP+{alloca}]`
  （arm64/riscv64 命中都是 `asm` 里的**操作数名**，不是内存文本）⇒ 改动面极小、且被 x86 矩阵直接覆盖。
+ 整仓 `-j 2` → exit 0、0 编译错误、0 失败二进制（生成物表达式与旧路径逐字相同）。

### Changed (2026-10-08) — ISA-DSL v21 W7 第七片：尺寸关键字**只认** `[operand.<mem>].size_words`

+ 清单 §9 的"尺寸关键字属于 `[operand.mem].size_words`"落地：**撤掉**旧键
  `[conventions.mem].size_keywords`（写它现在被拒）——`MemTemplate.size_keywords` 改成
  `#[serde(skip)]` 的**内部载体**，只由 `[operand.<mem>].size_words` 降级时写入
  （与 `IsaModel.templates` / `Template.from_family` 同一手法），并在 `INTERNAL_FIELDS` 登记。
+ 实测：**三谱本来就已经用新键**（x86 `[operand.mem].size_words`；riscv64/arm64 不用 `{size}`）
  ⇒ 本片是"删旧家"而非迁移，生成物与谱文本零变化。
+ 三方针同步：模型（`skip`）+ schema（`[conventions.mem]` 的可选键清空）+ 文档键表该行
  + 文档 TOML 示例（改为 `[operand.mem]` + `text`/`size_words`）+ `{size}` 散文说明
  + 生成物 `isa-dsl.schema.json`；报错文案与单测断言同步指向新键。
+ 整仓 `-j 2` → exit 0、0 编译错误、0 失败二进制。

### Added (2026-10-08) — ISA-DSL v21 W7 第六片：`{name:affix}` 的**解析语义**

+ 解析侧（`gen_assemble_try_tok`）现在**先尝试吃词缀**：写法里带 `{n:affix}` 时，源文本可以带该词缀。
+ **为什么"吃了也算、没吃也行"是安全的**：`__eat_lit` 只在**整段匹配成功**时才提交 `it.pos`，
  失败会回退 ⇒ 把词缀元素写成"尝试后无条件通过"既不误吞，又覆盖了 `+` 被渲染抑制（负值）
  时文本里本就没有词缀的情形。
+ 整仓 `-j 2` → exit 0、0 编译错误、0 失败二进制（无谱使用词缀 ⇒ 生成物逐字节不变）。

### Added (2026-10-08) — ISA-DSL v21 W7 第五片：`{name:affix}` 的**渲染语义**

+ 渲染侧（`gen_disassemble`）实现词缀：`{n:affix}` ⇒ 词缀**随组件出现**，且 **`+` 对已带 `-`** 的
  渲染值**抑制**（生成期算出 `__aN` 局部，格式串用 `{__aN}{__oN}`）——即设计案 §6.9 的原话。
+ 未做（下一片）：**解析侧**接受词缀（现在解析只认 `{n}` 本身）、`{name?}` 的**省略**语义
  （"连同紧邻字面词缀一起省略" 需要"该操作数是否在场"的信号 —— 设计案未定义该信号从哪来，
  需先定：是按**取值缺省**还是按**谱面声明**）。
+ 整仓 `-j 2` → exit 0、0 编译错误、0 失败二进制。

### Added (2026-10-08) — ISA-DSL v21 W7 第四片：占位修饰 `{name?}` / `{name:affix}` 的**语法层**

+ `Seg::Op(usize)` 扩成 **`Seg::Op { idx, optional, affix }`**：`parse_template` 现在认四种写法
  —— `{N}` / `{N?}`（可选）/ `{N:affix}`（词缀随组件出现）/ `{N?:affix}`。
+ **规范化必须保留修饰**：`{名字}` → `{索引}` 的改写器原先按整串查名 ⇒ `{dst?}` 被当成操作数名
  `dst?` 报"不是已声明的操作数名"。现在先**剥掉修饰**查名、查出索引后**原样带回**
  （否则修饰会在规范化这一步静默丢失）。
+ 测试：`dsl::tests::placeholder_modifiers_parse`（四种写法逐项断言）；
  `dsl::tests::normalization_keeps_placeholder_modifiers`（规范化后 `?` 与 `:affix` 仍在）。
+ 12 处 `Seg::Op` 匹配点随改（先被我的通配正则误伤了另一个枚举 `NamedSeg::Op(…)` ⇒ 已还原）。
+ 整仓 `-j 2` → exit 0、0 编译错误、0 失败二进制。
+ **下一片**：修饰的**语义**（`{name?}` 的"连同紧邻字面词缀一起省略"、`{name:affix}` 的渲染/解析、
  `+` 对已带 `-` 的数值抑制）＋ 尺寸关键字迁到 `[operand.mem].size_words`。

### Added (2026-10-08) — ISA-DSL v21 W7 第三片：`--ambiguity` 报"靠声明序兜"的写法歧义

+ 新增可选档 **`--ambiguity`**（`LintOpts.check_asm_ambiguity`，默认关）：报"**同助记符 + 同操作数类**
  ⇒ 两条写法都能匹配、只能靠**声明序**兜"的指令（清单要求：报歧义、不靠声明序兜）。
+ 判据是**代理**（真 `type_signature` 在 codegen 私有）：`asm.render()` 的首字面段（助记符）
  + `ops` 的**类列表** ⇒ **偏粗**：实测 x86 **35 处**、riscv64/arm64 亦有若干，
  其中含"文本可区分但类列表相同"的误报（如 `movzx {dst}, byte ptr [{src}]` 对 `movzx {dst}, {src}`）
  ⇒ 与仓库其它可选档（`--bits`/`--suggest` 等）同样定位为**评审清单，不是错误判据**，
  故**默认关**（否则与"发行谱零结论"门禁冲突）。
+ 单测 `dsl::tests::asm_ambiguity_is_reported_when_opted_in`：两条同助记符同类的指令 ⇒
  默认档**不报**、可选档**报**。
+ CLI 接线（手写解析）：`--ambiguity` 进 `has_flag` + 从 `paths` 排除 + 传参；
  帮助文本补记。
+ 整仓 `-j 2` → exit 0、0 编译错误、0 失败二进制。

### Added (2026-10-08) — ISA-DSL v21 W7 第二片：解析器**按序尝试备选写法**

+ `gen_assemble_try_tok` 改为接受**一条写法**；`gen_assemble` 为每条指令的**每个可接受写法**各生成一次尝试
  （去重键带上写法，否则备选会被"同类型签名"误跳过）⇒ 解析按 `asm` 列表序尝试、**第 0 条优先** ⇒ 旧行为逐字节一致。
+ `gen_scan_probe`（`could_be_instruction`）改认**全部**写法的首字面段。
+ **关键一环**：`codegen/mod.rs` 的 asm 规范化原先只处理渲染形态 ⇒ 备选写法在入口就被丢掉；现在**逐条写法各自规范化**
  （下游 asm/machine/decode 只认索引形态），`uses` 仍只统计渲染形态（各写法应一致）。
+ 单测 `dsl::tests::alternative_spellings_are_emitted`：生成物里同时出现主写法与备选写法的字面段。
+ 整仓 `-j 2` → exit 0、0 编译错误、0 失败二进制（1326 处单写法谱零变化）。

### Added (2026-10-08) — ISA-DSL v21 W7 第一片：`asm` 支持**可接受写法列表**

+ 新增 `model::AsmList`：`asm = "add {dst}, {src}"` 与 `asm = ["add {dst}, {src}", "add {src}, {dst}"]`
  **两种写法都接受** —— **第 0 条 = 渲染形态**（反汇编只吐它），其余为**解析专用备选**
  （按列表序尝试，与 `[conventions.mem].templates` 同一口径）。
+ 设计要点：`Deref<Target = str>`（指向渲染形态）+ `AsRef<str>` + `PartialEq<str>` + `Display`
  ⇒ **既有 43 个读取点只需改 3 处**（`validate` 参数替换、`report` 渲染文本、`codegen` 规范化 asm）；
  参数替换现在作用于**全部写法**。
+ 单测 `dsl::tests::asm_accepts_a_list_of_spellings`（列表两条都在、`render()` = 第 0 条、
  单字符串等价单元素列表）；另修两处测试的 `.as_str()`（经 `Deref` 会落到不稳定的 `str::as_str`）。
+ 整仓 `-j 2` → exit 0、0 编译错误、0 失败二进制（1326 处 `asm = "…"` 逐字节不变）。
+ **下一片**：让**解析器**真的按序尝试备选写法（本片只落模型/渲染口径）。

### Changed (2026-10-08) — ISA-DSL v21 W6：`frame.rs` 尾声跳转不再"按字段个数猜"

+ `frame.rs:167` 的 `let (jal_dest, jal_target) = if jump_f.len() >= 2 { … }` 与
  `} else if has_jump && jump_f.len() >= 2 {` —— 清单点名的**第三处**字段个数猜测 —— 全部改成
  **声明数据判定**：jump 指令若有 `Reg` 类操作数（riscv `JAL dst, target`）⇒ 需要填 dest；
  否则是单 label 槽（x86 `JMP_REL32 target`）。dest/label 的**字段名**也从槽种类推出，
  不再固定取 `jump_f[0]`/`jump_f[1]`。
+ 验证：整仓 `-j 2` → exit 0、0 编译错误、0 失败二进制（尾声跳转影响三谱 ⇒ x86/riscv64 JIT 矩阵 +
  汇编三档 + 三矩阵计数全不变）⇒ 与旧行为**等价**。
+ 至此清单「删除编译器三处按字段个数猜 ISA」**全部完成**（`lowering.rs:202`、`:527-625`、
  `frame.rs:172-208`）。

### Added (2026-10-08) — ISA-DSL v21 W6：第四形态夹具（验证形态**只由谱面决定**）

+ 清单原话是"同一份谱里两个分支形态" —— **实测造不出来**：DSL 禁止同角色同类两条声明
  （"角色 \"branch\" 与 … 冲突"）。改用**等价且更硬**的验证：让 `[lower.Br]` 点名**另一条无角色指令**
  —— 旧实现**只会**用角色表里那条，谱面却能任意选。
+ 新测试 `dsl::tests::branch_form_comes_from_spec_not_role_table`：断言 `Br` 分支体里出现
  **谱面点名的** `Inst::Brchosen`，且该片段内**不得**出现角色表里的 `Brrole`。
+ 三个实测坑（都写进注释）：① 谱面序列**不能**用 `{out}`（那是*规则*操作数角色，终结层没有）⇒
  用 `{cond}`；② 变体名是 mnemonic 的 **Pascal 形式**（`Brchosen`，不是 `BRCHOSEN`）；
  ③ 否定断言必须**限定在分支体片段** —— 每条声明过的指令都会在枚举/表里出现变体，全串搜必然命中。
+ 整仓 `-j 2` → exit 0、0 编译错误、0 失败二进制（三矩阵计数不变）。

### Added (2026-10-08) — ISA-DSL v21 W6：`[[lowering]].temps` 显式临时寄存器（可选，声明即强制）

+ 新增可选键 **`temps = ["g1:gpr", "f2:fpr"]`**：把"靠 `g`/`f` 前缀 + 数字**猜类别**"的隐式魔法
  变成**可选显式** —— 声明了它 就必须列出本规则用到的**全部**临时，出现未声明者 ⇒
  **codegen 报错并点名**；不声明 ⇒ 沿用旧扫描（**不强制迁移**）。
+ 实测迁移成本：三谱隐式临时共 **1222 处**（x86 289+196、riscv64 737、arm64 0）⇒
  按"可选 + 仅当声明才强制"落地，避免 1222 处机械重写。
+ 校验在 **codegen**（`parse_and_validate` 不跑 codegen）⇒ 单测用 `codegen::generate` 断言：
  合规通过 / 未声明点名 `g2` / 类别写错点名 `vec`。
+ **两个串扰坑（都实测踩到）**：① 守卫用 `struct …Lowering` 的**子串匹配** ⇒ 我新增的
  `TermLowering` 被当成 `Lowering` ⇒ 已改名 **`TermSeq`**；② 文档注释里写了**孤立的 `{`**
  （`` `{g`/`{f` ``）⇒ 守卫的**朴素花括号扫描失衡** ⇒ 把整个文件后半都当成 `Lowering` 的字段
  ⇒ 注释里不再出现不配对花括号。
+ 三方针同步（模型 + schema + 文档生成表该行 + 重生成 `isa-dsl.schema.json`）；
  整仓 `-j 2` → exit 0、0 编译错误、0 失败二进制。

### Added (2026-10-08) — ISA-DSL v21 W6：`insts` 支持**算术表达式占位符**（`{imm0 - 4}` 等）

+ 新增 `placeholder::{is_arith_expr, expr_ctor}`：`insts` 可写 `{imm0 - 4}`、`{iconst >> 12 & 0xfffff}`
  这类**表达式** —— 认法 = "整串被一层 `{}` 包住 + 顶层出现运算符"，操作数必须是**已注册占位符**
  或整数字面量。
+ 槽适配（`kind_matches`）与 token 种类（`token_kind`）都**按首个操作数**判定（`{imm0 - 4}` 与
  `{imm0}` 同槽适配）；ctor 由各操作数既有 ctor 拼接运算符，**纯生成期展开**（生成物里就是
  `(<imm0 表达式> - 4)`）。
+ 单测：`dsl::tests::arith_expr_placeholder_is_recognized`（认法 + 两个反例 + 首操作数种类一致）。
+ 踩坑：首操作数最初返回**不带花括号**的 `imm0` ⇒ `lookup` 找不到（注册名是 `{imm0}`）⇒ 落到
  "物理寄存器"启发式返回 `"reg"` ⇒ 单测当场抓到。
+ **下一步**：用它替换谱里的派生 token（`imm0_sub4`、`iconst_hi20`/`_lo12`/`_hi32_*`/`_lo32_*`、
  `fconst_hi32_*`/`lo32_*` 等 20 余个）并删掉这些注册条目；`vconst_*`/`seg`/`ph`/`iconst_c16`
  依赖常量池/相位/类型，不是算术，保留。
+ 整仓 `-j 2` → exit 0、0 编译错误、0 失败二进制。

### Fixed (2026-10-08) — ISA-DSL v21 W6：清掉删除死回落后的残留告警（含元组解构）

+ 上一片删掉终结回落链后留下 17 个"未使用变量"告警（其中多数来自**元组解构**：
  `let (jal_dest, jal_target) = …`、`let (b_src, b_src2, b_target) = …`）。已全部清成 0 告警。
+ 侦察清单第三处猜测（`frame.rs:167` / `:182`）：**尾声跳转**也按 `jump_f.len() >= 2` 猜字段，
  但它的发射 API 是 `encoder.encode(&inst, …)`（不是 `push_inst`）⇒ **谱面序列无法直接复用**
  ⇒ 记为独立小项（要引入"以编码器为宿主的序列发射" 才能谱面化），不硬塞进本片。
+ 整仓 `-j 2` → exit 0、0 编译错误、0 失败二进制。

### Removed (2026-10-08) — ISA-DSL v21 W6：删除终结 lowering 里已成死代码的"猜测回落"

+ `jump_body` / `branch_body` 的 `else if …` 链**整条删除** —— 它们包含清单点名要删的
  `jal_f.len() >= 2` / `beq_f.len() >= 3` / `test_f.len() == 2` / `jcc_f.len() == 2` 与 `eq_code`。
  现在只剩："谱面声明的序列" ⊕ "缺 `[lower.*]` ⇒ `Unsupported`" —— **终结序列唯一来源 = 谱面**。
+ 顺带清掉随之无用的局部（`has_test`/`has_jal`/`jump_vn`/`jal_dest`…）。**仍剩几处来自元组解构的
  未使用告警**（`let (jal_dest, jal_target) = …` 这类，纯装饰，下轮清）。
+ 验证：整仓 `-j 2` → exit 0、0 编译错误、0 失败二进制（x86/riscv64 JIT 矩阵 + 汇编三档 +
  三矩阵计数全不变）。

### Changed (2026-10-08) — ISA-DSL v21 W6：终结序列**唯一来源 = 谱面**（旧猜测路径 fail-closed）

+ 生成期新增检查：谱若声明了 `roles = ["jump"]` / `roles = ["branch"]` 的指令，就**必须**给出
  `[lower.Jmp]` / `[lower.Br]` —— 否则**生成期直接报错**（报错点名缺哪一节、为什么）。
+ 意义：`lowering.rs` 里"按字段个数猜 ISA"的旧回落（`jal_f.len() >= 2`、`beq_f.len() >= 3`、
  `test_f.len() == 2`、`jcc_f.len() == 2` 与 `eq_code`）从此**不可达** —— 它不再是"静默兜底"，
  而是被谱面契约取代。
+ 验证：整仓 `-j 2` → exit 0、0 编译错误、0 失败二进制 ⇒ 证明**没有任何夹具依赖旧路径**
  （三谱都声明了这两节，其余夹具没有终结角色）。
+ 后续（同一目标内）：删掉已成死代码的回落分支本身、`[lower.Ret]` 的谱面表达、
  以及 `frame.rs:172-208` 的同类猜测。

### Changed (2026-10-08) — ISA-DSL v21 W6：riscv64/arm64 落地 `[lower.Br]`（三谱分支全部谱面化）

+ riscv64：`insts = ["BEQ {cond}, X0, {false}", "JAL X0, {true}"]`（条件形态是 `beq cond,x0`，
  条件码**不参与**）；arm64：`insts = ["CBZX {cond}, {false}", "B {true}"]`（用 `cbz`）。
+ 至此 **`[lower.Jmp]` + `[lower.Br]` 三谱全部落地且与旧硬编码等价**：
  整仓 `-j 2` → exit 0、0 编译错误、0 失败二进制（riscv64 JIT 矩阵 + x86 JIT 矩阵 + 汇编三档 +
  三矩阵计数全不变）。
+ 踩坑：三谱的 `branch` 角色是写在**族行内联表**里的（`{ inst = "BEQ", …, roles = ["branch"] }`），
  按行找 `name =` 会落空；另外用 PowerShell 拼接含 `{}` 的**注释**会把内容吃掉
  （arm64/riscv64 各因此红一次，改成纯文本注释后通过）。
+ 仍关着：`[lower.Ret]`（返回搬移的谱面表达未落地）。

### Changed (2026-10-08) — ISA-DSL v21 W6：`{cond}` 符号值 + x86 落地 `[lower.Br]`（谱面 ≡ 硬编码）

+ 新增终结层符号值 **`{cond}`**（`[lower.Br]` 的条件寄存器）：与 `{g}` 同形（字段先填 0、
  映射期由 `xreg: "__cond"` 改写成真寄存器），但**不分配**临时——直接用终结层算好的 `__cond`。
+ 终结层 `Br` 谱面体在**生成期**（`quote!` 之外）算好 `{cc}`/`{cc_inv}` 的**字面**再插值
  （`Br` 里 `test` 置 ZF ⇒ 条件码语义固定为本 ISA 的 `eq`/`ne`）；生成代码里绑定
  `__cc`/`__cc_inv`/`__cond`/`__true_block`/`__false_block`。
+ 放行 `Br`，给 **x86** 落地真名序列：
  `insts = ["TEST_RM_R {cond}, {cond}", "JCC_REL32 {cc}, {false}", "JMP_REL32 {true}"]`
  （硬编码走**角色查表**，谱面必须写指令真名）。
+ 安全闸测试随动：现在只拦 `Ret`，`Jmp`/`Br` 放行。
+ 验证：整仓 `-j 2` → exit 0、0 编译错误、0 失败二进制（x86 JIT 矩阵 + 汇编三档 + 三矩阵计数不变）
  ⇒ 谱面路径与旧硬编码**等价**。
+ 仍关着：`[lower.Ret]`（返回搬移的谱面表达未落地）；riscv64/arm64 的 `[lower.Br]` 未落地。

### Added (2026-10-08) — ISA-DSL v21 W6：`[lower.Br]`/`[lower.Ret]` 的**解析期安全闸**

+ **问题**：`Br` 的旧硬编码体还带一步 `map_reg_field`（把 IR 条件寄存器写进 test/branch 的操作数），
  当前占位符体系表达不了；`Ret` 的体含"整数值→返回寄存器 / 浮点→浮点返回寄存器"的搬移与 bank 选择。
  ⇒ 若让谱现在就声明 `[lower.Br]`/`[lower.Ret]`，会**静默生成错代码**。
+ **闸门**：`model::check_term_lowering_supported`（接在 `parse_and_validate` 链上）——目前
  **只放行已完成接线并验证等价的 `[lower.Jmp]`**；`Br`/`Ret` 声明即解析期报错，并点明"W6 进行中"
  与缺什么（条件寄存器映射 / 返回搬移的谱面表达）。
+ 测试：`unsupported_term_lowering_is_rejected`（两种都拒 + `Jmp` 放行）；
  `lower_terminator_section_parses` 随动为只验 `Jmp`。
+ 整仓 `-j 2` → exit 0、0 编译错误、0 失败二进制。

### Changed (2026-10-08) — ISA-DSL v21 W6：三谱落地 `[lower.Jmp]`（谱面 ≡ 硬编码，行为逐字节不变）

+ 三谱写出等价的跳转序列：x86 `JMP_REL32 {target}` · riscv64 `JAL X0, {target}` · arm64 `B {target}`
  ⇒ **首批真实谱面**开始驱动终结 lowering（此前只在测试夹具里验证过）。
+ **token 种类**要一起教：块号符号值 `{true}`/`{false}`/`{target}` 在 `kind_matches` 里
  **同时接受 Imm 与 Label** —— 各 ISA 的目标槽种类不同（x86 `target:rel`、riscv `target:off_j`、
  arm64 `target:off26`）；否则会落到"字面立即数"分支并报
  `lowering 模板立即数 '{target}' 无法解析`。
+ 验证：整仓 `-j 2` → exit 0、0 编译错误、0 失败二进制（JIT 矩阵 + 汇编三档 + 三矩阵计数全不变）
  ⇒ 谱面路径与旧硬编码**等价**。
+ 仍未做：`[lower.Ret]` 接线、`Br` 的谱面落地（它需要 `map_reg_field` 这类"条件寄存器映射"，
  当前占位符体系表达不了 ⇒ 是 W6 后续要设计的一环）、删旧猜测路径。

### Added (2026-10-08) — ISA-DSL v21 W6：`[lower.Jmp]` 谱面路径的**行为测试**

+ 新增 `dsl::tests::lower_jmp_spec_path_is_used`：夹具声明 `[lower.Jmp]` 并带一条
  `roles = ["jump"]` 的指令（**触发条件**：`has_ret || has_jmp || has_jcc` 才会生成终结实体），
  断言生成物里出现 **`__target_block`** —— 该符号**只可能来自谱面路径**（旧硬编码体用的是
  `target.index()`）⇒ 证明"声明了就真的走谱面"。
+ 踩坑：终结 lowering 的实体**只在有终结角色指令时生成**，否则落到 `Err(Unsupported)` 的桩
  ⇒ 没有角色指令的夹具**测不到**这条路径（第一次就这么红了）。
+ 整仓 `-j 2` → exit 0、0 编译错误、0 失败二进制。

### Changed (2026-10-08) — ISA-DSL v21 W6：终结层开始消费 `[lower.Br]`/`[lower.Jmp]`

+ 新增三个**块号符号值**（终结层专用）：`{true}` / `{false}` / `{target}` ⇒ 展开成生成代码里的
  `__true_block` / `__false_block` / `__target_block`（值来自 `dfg.term_branch`/`term_jump` 的块号）。
+ `lower_terminator` 的 `Br`/`Jmp` 体现在**谱面优先**：谱里声明了 `[lower.Br]`/`[lower.Jmp]` 就用
  `gen_lowering_insts(&insts, …)` 生成的序列（自行定义那三个块号局部）；**未声明则完全走旧路径**
  ⇒ 三谱行为与三矩阵计数**逐字节不变**。
+ 踩坑：那个通用发射器在终结层里拿到的 `name_to_vn` 是**值**而非引用 ⇒ 要写 `&name_to_vn`。
+ **尚未做**：`[lower.Ret]` 的接线（返回体还含"整数值→RAX / 浮点→XMM0"的搬移，比 Br/Jmp 复杂）、
  谱面路径的**行为测试**（需要一个带 Jump/Branch 角色的夹具）、三谱写出这三节。
+ 整仓 `-j 2` → exit 0、0 编译错误、0 失败二进制。

### Added (2026-10-08) — ISA-DSL v21 W6：`[lower.<终结 op>]` 谱面落地（模型 + schema + 文档 + 解析测试）

+ 新增谱面 **`[lower.Br]` / `[lower.Jmp]` / `[lower.Ret]`**（各带 `insts = [...]`）：
  分支/跳转/返回的序列从编译器搬进谱。
+ **为什么与 `[[lowering]]` 分家**（本轮勘明）：`then`/`else` 块号只有终结层拿得到
  （`dfg.term_branch(block)`），per-op 的规则路径**没有这个上下文** ⇒ 终结序列必须由
  `lower_terminator` 消费，故独立成节。
+ 该谱面可用既有占位符，并预留 W6 的三个新符号值 **`{true}` / `{false}`（Br 两个后继块号）
  与 `{target}`（Jmp 目标块号）**——本片只落地"能被解析并进模型"（含解析测试），
  codegen 接线留下一片（保持三矩阵不变）。
+ 三方针同步：`model.rs` + `schema.rs` + 文档生成表（**整段粘贴**）+ `isa-dsl.schema.json` 重生成；
  `schema_guard` **9 passed / 0 failed**。
+ 整仓 `-j 2` → exit 0、0 编译错误、0 失败二进制。

### Changed (2026-10-08) — ISA-DSL v21 W6：`Br` 的条件码从硬编码 `4u8` 改为数据驱动

+ `test`+`jcc` 那条分支体里"条件不成立就跳 false"原先**硬编码 `4u8`**（= x86 的 `je`） ——
  正是 W6 清单点名要删的那处。现在生成期从谱的 **`[enum.cond]`** 取本 ISA 的 **`eq` 码**；
  谱没映射 `eq` 时回落 4 以保持旧行为（**过渡态**，等 `[lower.Br]` 下放到谱后彻底删掉）。
+ 踩坑：`quote!` 里的**裸标识符是原样发射**的 —— 第一版写成 `#jcc_cond: eq_code` ⇒ 生成物里出现
  `cc: eq_code`（`forge-codegen` 编译期报 `cannot find value`）。正确写法是插值 `#eq_code`。
+ 整仓 `-j 2` → exit 0、0 编译错误、0 失败二进制（三谱行为逐字节不变）。

### Added (2026-10-08) — ISA-DSL v21 W6：`{cc_inv}` 补**端到端生成测试**

+ 新增 `dsl::tests::cc_inv_uses_inverse_condition_code`：用同一张 `[enum.cond]` 夹具跑真实生成，
  断言 ① 发射 `__cc_inv`；② **逆映射正确**（`eq→ne` 的码、`ne→eq`、`slt→sge`、`uge→ult`）；
  ③ **零死代码**（只用 `{cc_inv}` 时不发射 `__cc`）。
+ 踩坑：`TokenStream::to_string()` 是**美化打印**（`::`/`=>` 两侧可能带空格）⇒ 断言前先抹掉空白。
+ 整仓 `-j 2` → exit 0、0 编译错误、0 失败二进制。

### Added (2026-10-08) — ISA-DSL v21 W6 第一片：`{cc_inv}` 占位符（条件取反）

+ `lowering` 的 `insts` 现可用 **`{cc_inv}`**：IR 条件的**逆**在本 ISA 的编码。
  逆名由宿主契约层 `model::ir_inverse_cond` 给（eq↔ne、slt↔sge、sgt↔sle、ult↔uge、ugt↔ule，
  与 ISA 无关），**编码仍从 `[enum.cond]` 反查**（数据驱动，不硬编码 `code ^ 1`——那只对 x86 成立）。
+ 生成侧**按需发射**：规则只用 `{cc}` 就只发 `__cc`，只用 `{cc_inv}` 就只发 `__cc_inv`，
  都不用则两条都不发（延续 S8d 的"零死代码"口径）。
+ 新增单测：逆配对**可逆且非自反**、未知名返回 `None`。
+ 整仓 `-j 2` → exit 0、0 编译错误、0 失败二进制。

### Fixed (2026-10-08) — ISA-DSL v21 W5 遗留：`explain` 的 provenance 打印族感知

+ `forge-isa explain` 原先恒打印 `[[templates.<名>]]`（族降级来的实例也照旧）——这是最后
  一处**用户可见**的旧面残留。现在 `report::TemplateRow` 带 `from_family`，打印按来源输出
  **`[family.<名>]`** 或 `[[templates.<名>]]`；`cli_tests` 的期望随动。
+ 整仓 `-j 2` → exit 0、0 编译错误、0 失败二进制。

### Changed (2026-10-08) — ISA-DSL v21 W5 ⑥ 完成：`[[templates]]` **撤出 TOML 面**

+ `IsaModel.templates` 改为**内部载体**（`#[serde(default, skip)]`，并在
  `schema_guard::INTERNAL_FIELDS` 登记）：`[family.<名>]` 仍在解析期降级进它、codegen 继续读它，
  但 **TOML 里写 `[[templates]]` 会被未知键守卫拒绝**。
+ `schema.rs` 精确删掉 root 级 `templates` 键与 `[[templates]]` / `[[templates.rows]]` 两节
  （**不动** `[conventions.mem].templates` —— 那是内存操作数文本形态，上一轮误删过一次）。
+ `docs/reference/isa-dsl.md` 的键速查表是**生成物**（`schema::markdown_table()`）：本轮改用
  **整段粘贴生成文本**（临时打印测试取表 → 替换标记区 → 删临时测试），手改必红；
  重生成 `isa-dsl.schema.json`。
+ 证据：`schema_guard` **9 passed / 0 failed**（含"发行谱只许用 schema 键"与"键表 ≡ schema"两条）；
  `cargo test -p forge-isa-dsl` 276 passed；整仓 `-j 2` → exit 0、0 编译错误、0 失败二进制。
+ 至此 **W5 的 ⑥ 全链路完成**：112 处模板迁移 → 夹具全部 `[family.*]`（7 文件代码归零）→
  诊断锚点族感知（行号不再退化成 1）→ TOML 面撤出 → schema/文档/守卫三方针同步。

### Fixed (2026-10-08) — ISA-DSL v21 W5 ⑥b 完成：族感知诊断锚点 + 全部夹具迁到 `[family.*]`

+ **产品行为修复**：`[family.<名>]` 降级出的载体原先锚回内部名 `[[templates.X]]`（源里没有这一节）
  ⇒ `push_anchored` 找不到节、**诊断行号退化成 1**。现在 `Template` 带内部标记 `from_family`
  （`serde(skip)` + `schema_guard::INTERNAL_FIELDS` 登记），`Template::expand` 的路径前缀与
  `validate` 的锚点替换都输出 **`[family.<名>]`**；`diag.rs` 加 `"family" => "DSL-TEMPLATE"`。
+ **夹具迁移收口**：`dsl/tests.rs` · `dsl/lint.rs` · `report.rs` · `dsl/stream_decl.rs` ·
  `dsl/template_tests.rs` · `dsl/diag_matrix_tests.rs` 全部改 `[family.*]` ⇒ 7 个文件代码里
  `[[templates]]` **归零**。迁移器 `target/tmp/migrate_tpl.py` + `fix_cont.py`：三条硬教训——
  停止条件含原始字符串终结符 `"#`；内联 `rows` 与行标记**二选一**；替换模式容忍 `\r?\n`。
+ **⑥ a+c（撤出 TOML 面）未完成、已回退保绿**：它要求三方针同时改，其中文档键速查表是
  **生成物**（`schema::markdown_table()`）⇒ 必须整段粘贴生成文本，不能手改；下轮先取该文本再动。
+ `cargo test -p forge-isa-dsl` → 276 passed；整仓 `-j 2` → exit 0、0 编译错误、0 失败二进制。

### Changed (2026-10-08) — ISA-DSL v21 W5 ⑥ b：4 个文件的模板夹具迁到 `[family.*]`

+ 本轮**一次做完**：`dsl/tests.rs` · `lint.rs` · `report.rs` · `dsl/stream_decl.rs` 的夹具全部迁移，
  四个文件**代码里 0 处** `[[templates]]`；`dsl/template_tests.rs` 上一轮已完成（仅剩 1 处**断言**里
  的内部锚点期望）。
+ 迁移器 `target/tmp/migrate_tpl.py`（Python）：吃"原始字符串式 / 单行转义式"，配 `fix_cont.py`
  吃"Rust `\` 续行转义串"。**三条硬教训**（都实测踩到）：
  ① 停止条件必须含原始字符串终结符 `"#` 与空行，否则吞掉夹具之后的 Rust 代码；
  ② 模板**同时**有内联 `rows = [ … ]` 与 `[[templates.rows]]` 时会重复输出 `rows = [` 并吃掉行内容
  ⇒ 必须二选一（本轮踩到 2 处，手工按已知实例名 `T1`/`V1` 修复）；
  ③ 替换模式必须容忍 `\r?\n`（同一次提交里文件行尾可能 LF/CRLF 混合，否则静默 0 命中）。
+ 顺带改名：`validation_template_row_needs_encoding` → `validation_family_row_needs_encoding`
  （函数体在转换中被吞，已按邻居测试的写法补回）。
+ **暂缓项**：`dsl/diag_matrix_tests.rs` 的 3 处**已回退** —— 迁移后"族 `rows` 为空"的诊断
  **退化成第 1 行**（族形态报错**缺行锚点**），而该测试的 `d.line > 1` 是合理不变量 ⇒
  **先给族报错补锚点**，再迁这 3 处（记为下一步）。
+ `cargo test -p forge-isa-dsl` → 275+ passed；整仓 `-j 2` → exit 0、0 编译错误、0 失败二进制。

### Changed (2026-10-08) — ISA-DSL v21 W5 ⑥：`report.rs` 夹具迁到 `[family.*]`

+ `report.rs` 的 3 处 `[[templates]]` 全部迁走（该文件现已 **0 处**）：
  ① 原始字符串夹具 `[[templates]] + name = "ALU" + body + rows = [ … ]` **与 `[family.ALU]` 完全同形**
  ⇒ 一行替换；② `diff` 用例里的两处 `.replace("[[templates]]", …)` 随动为 `[family.ALU]`。
+ **行尾坑（本轮实测）**：同一次提交里，`report.rs` 在盘上是 **LF**（git 尚未落 CRLF 转换），
  而其他文件可能是 CRLF ⇒ 替换模式必须写成 `\r?\n` 容忍两种，否则**静默 0 命中**
  （本轮连续两次 0 命中，第三次改正则后 1 处命中）。
+ `cargo test -p forge-isa-dsl --lib report` → **13 passed / 0 failed**；整仓 `-j 2` → exit 0、
  0 编译错误、0 失败二进制。

### Changed (2026-10-08) — ISA-DSL v21 W5 ⑥：模板夹具迁移到 `[family.*]`（`template_tests.rs` 完成）

+ `template_tests.rs` 的 **15 个 `[[templates]]` 夹具**全部机械迁移为 `[family.<名>]`
  （`body` 保留、行折进 `rows = [ … ]`；无名模板按 `lower_families` 的同款规则派生成 `<inst>_FAM`）。
  变换脚本的停止条件必须包含**原始字符串终结符 `"#`**——否则会把夹具之后的 Rust 代码吞进行里
  （本轮实测踩到，`git checkout` 回退后修正）。
+ 唯一非机械期望：`from_template` 由模板名 `"ADDW"` 变为派生族名 `"ADDW_FAM"`（断言已随动）。
+ 该文件仅剩 **1 处** `[[templates` 字样 = 一条断言**内部锚点**的期望（内部路径仍以模板名锚定诊断）。
+ `cargo test -p forge-isa-dsl --lib template` → **30 passed / 0 failed**；整仓 `-j 2` → exit 0、
  0 编译错误、0 失败二进制。

### Added (2026-10-08) — ISA-DSL v21 W5 ⑥ 第一步：**发行谱零 `[[templates]]` 守卫**

+ 实测确认：三份发行谱已**归零**（`isa/x86.toml` 那处命中只是**注释里的字样**，已改写为
  `[family.*]` 的正确说明）——即 112 处模板迁移的**收口条件已满足**。
+ 新增守卫 `lint_shipped.rs::shipped_specs_have_no_template_sections`：三谱出现 `[[templates`
  段即红，把"迁移已归零"锁死（回潮当场可见，而不是等到删 TOML 面时才发现）。
+ 剩余工作（下一步）：把 `templates` 从 **TOML 面**降为**内部载体**（`serde(skip)` + schema/文档
  三方针同步），并迁移 ~25 处 Rust 夹具（`template_tests.rs` 17 处等）到 `[family.*]`。
+ `validate` + `lint` 干净；整仓 `-j 2` → exit 0、0 编译错误、0 失败二进制。

### Changed (2026-10-08) — ISA-DSL v21 W5：x86 `Load` 的 `rd` 对折叠（累计 37/52）

+ 给 `MOVZX_R8_MEM`/`MOVZX_R16_MEM` 补 `group = "load_rd"` + `word_size`（`rd` 用**位**：8/16），
  把两条 `when = { eq = ["rd", N] }` 折成一条 `dispatch = "rd"` 的宽度无关规则
  ⇒ x86 `[[lowering]]` 块数 **175 → 174**。
+ 第 (4) 项累计删除 **37 / 52** 条（riscv64 24 + x86 7 + arm64 6）。棘轮随动；
  `validate` + `lint` 干净；JIT 58/0；整仓 `-j 2` → exit 0、0 编译错误、0 失败二进制。

### Changed (2026-10-08) — ISA-DSL v21 W5：arm64 宽度分派清 6 条（累计 36/52）

+ 给 `[family.ADDREG]`/`SUBREG`/`MUL`/`ANDREG`/`ORRREG`/`EORREG` 的 12 条 rows 补 `word_size`
  （`X` = 64 / `W` = 32；`group` 已由族体给出，**不覆盖**），把 6 对 `rs1_width` 分派折成 6 条
  `dispatch = "rs1_width"` 的宽度无关规则 ⇒ arm64 `[[lowering]]` 块数 **19 → 13**。
+ **注意**：arm64 **没有 JIT 矩阵**（只有 x86/riscv64）⇒ 本批的行为验证靠整仓门禁里的
  `isa_roundtrip_guard` + 谱内向量 + asm 棘轮 + `strict_overlap` 快照。
+ 快照随动：`--strict-overlap` arm64 **8 → 2**（被删的正是"`{rs1_width=32}` 特化 + 无 `when` 兜底"形状）。
+ 第 (4) 项累计删除 **36 / 52** 条（riscv64 24 + x86 6 + arm64 6）。
+ 三谱 `validate` + `lint` 干净；整仓 `-j 2` → exit 0、0 编译错误、0 失败二进制。

### Changed (2026-10-08) — ISA-DSL v21 W5：x86 向量 `Store` 组折叠（3 条 → 1 条，累计 30/52）

+ 给 `MOVUPS_128_MEM_R`/`VMOVUPS_256_MEM_R`/`VMOVUPS_512_MEM_R` 补 `group = "store_vec"` +
  `word_size`（字节 16/32/64），把 3 条 `rs1_vec` 规则折成一条 `dispatch = "rs1_vec"` 的规则
  ⇒ x86 `[[lowering]]` 块数 **177 → 175**。
+ **解开分组冲突的做法**（不改设计）：`MOVSD_MEM_R` 已属 Fstore 族（`rs1_width`/位），
  不要重复归组 ⇒ `Store` 的 **8 字节那条保留显式 `when`**（`rs1_vec = 8`），向量组只管 16/32/64。
  两组谓词**不相交** ⇒ 语义等价（JIT 58/0 验证）。
+ 第 (4) 项累计删除 **30 / 52** 条（riscv64 24 + x86 6）。棘轮随动；
  `validate` + `lint` 干净；整仓 `-j 2` → exit 0、0 编译错误、0 失败二进制。

### Changed (2026-10-08) — ISA-DSL v21 W5：x86 向量 `Load` 组折叠（4 条 → 1 条，累计 28 条）

+ 给 `MOVSD_R_MEM`/`MOVUPS_128_R_MEM`/`VMOVUPS_256_R_MEM`/`VMOVUPS_512_R_MEM` 补
  `group = "load_vec"` + `word_size`（**字节** 8/16/32/64，与既有 `rd_vec` 规则一一对应），
  把 4 条 `when = { eq = ["rd_vec", N] }` 折成一条 `dispatch = "rd_vec"` 的宽度无关规则
  ⇒ x86 `[[lowering]]` 块数 **180 → 177**。
+ **关键**：这次之所以能做，是因为上一轮把展开改成**显式 opt-in** —— 否则给这些指令补 `group`
  会让既有规则被误展开（前两轮 JIT 崩的根因）。
+ 第 (4) 项累计删除 **28 / 52** 条（riscv64 24 + x86 4）。棘轮随动；
  `validate` + `lint` 干净；JIT 58/0；整仓 `-j 2` → exit 0、0 编译错误、0 失败二进制。

### Fixed (2026-10-08) — ISA-DSL v21 W5：重载展开改为**显式 opt-in**（修两轮 JIT 崩溃的根因）

+ **根因（二分定位）**：只给 4 条向量 load 指令补 `group`（lowering 规则一条未动）JIT 就崩 ⇒
  问题不在"折叠"，而在**组属性本身**：既有那些"引用它们、没写宽度叶子"的规则被重载**自动展开**，
  从"对任意宽度适用"变成"只在某一档适用" ⇒ 覆盖丢失 ⇒ 行为改变。
+ **修法**：展开改为**显式 opt-in** —— 只有规则自己写了 `dispatch = "<键>"` 才展开，缺省一律不动。
  连带修掉一个**遮蔽 bug**：叶子构造处 `key` 被"族名查找"重新绑定 ⇒ 叶子属性写成了**族名**
  （`when = { eq = ["addw", …] }`）—— 这正是前两轮崩溃的直接原因（`validate` 报"未知属性 addw"抓到）。
+ **迁移**：给已折叠的规则补 `dispatch = "rs1_width"`（riscv64 **27** 条含 3 条多行 `insts` 规则 +
  x86 **1** 条）；行为逐字节不变 ⇒ JIT 矩阵 **58 passed / 0 failed**。
+ 棘轮随动（`lint.vary_candidates.riscv64` 28 → 31）；三谱 `validate` + `lint` 干净；
  整仓 `-j 2` → exit 0、0 编译错误、0 失败二进制。

### Changed (2026-10-08) — ISA-DSL v21 W5：x86 首条宽度分派删除（`Fstore`，累计 25 条）

+ `MOVSS_MEM_R`/`MOVSD_MEM_R` 补 `group = "fstore_x86"` + `word_size`（32/64 位），把 `Fstore` 的
  两条 `when = { eq = ["rs1_width", …] }` 折成**一条宽度无关**规则 ⇒ x86 `[[lowering]]` 块数
  **181 → 180**。
+ 第 (4) 项累计删除 **25 / 52** 条（riscv64 24 + x86 1）。棘轮随动（`lint.refs.x86` 1 → 2）。
+ `validate` + `lint` 干净；JIT 矩阵 58/0；整仓 `-j 2` → exit 0、0 编译错误、0 失败二进制。
+ 备注：自动折叠脚本（按整组收集成员）本轮尝试失败并已弃用（它在 x86 上改出了一条 `when` 含未知
  属性 `r` 的规则）；x86 这类"两条规则中夹着注释块"的形态，**手工折叠更稳**（本轮即如此）。

### Added (2026-10-08) — ISA-DSL v21 W5：降级重载支持**可选分派键** `dispatch`

+ `[[lowering]]` 新增可选键 **`dispatch = "<谓词属性>"`**（缺省 `rs1_width`）：重载自动补的
  宽度叶子改用该属性 —— 用于"按结果宽度分派"（`rd`/`rd_vec`）或"按源向量宽度分派"（`rs1_vec`）
  的场合（x86 的向量 Load/Store 就是这一类）。
+ 触发条件同步按该键判断（`when` 里已有该键的叶子 ⇒ 不接管）。
+ 三方针同步（模型 / schema / 文档速查表 / `isa-dsl.schema.json`）；`schema_guard` 9 passed。
+ 动机：x86 的 46 条宽度类分派里，`rd_vec`/`rs1_vec`/`elem` 多键并存，硬编码 `rs1_width`
  无法覆盖；有了 `dispatch` 才能继续折叠 x86 的向量对。

### Changed (2026-10-08) — ISA-DSL v21 W5：riscv64 再删 2 组（原子 `AMOSWAP`/`AMOADD` 的 `_W/_D`）

+ 给 `AMOSWAP_W/D`、`AMOADD_W/D`、`LR_W/D`（`[family.AMO*]` 的 rows）补 `group` + `word_size`，
  严格折叠 **2 对**（`AtomicRmw` 的 Xchg/Add）⇒ `riscv64.toml` 的 `[[lowering]]` 块数
  **96 → 94**（本轮累计 118 → 94，删掉 **24** 条宽度分派）。
+ **折叠安全守卫（新）**：差异必须**全落在行首段**（第一条 `", "` 之前）——重载只重写行首名，
  若宽度名出现在序列中段（如 `Cmpxchg` 的 `LR_*` + `SC_*`），改一行会留下不一致 ⇒ 这类**自动跳过**。
+ 插入器改为**逐行字符串手术**（在行内 `inst = "X",` 之后插键），比正则可靠；
  `$` 吃 `\r` 的坑同样规避。
+ 棘轮随动（`lint.refs.riscv64` 16 → 19）；`validate` + `lint` 干净；JIT 58/0；整仓 `-j 2` → exit 0。

### Changed (2026-10-08) — ISA-DSL v21 W5：riscv64 再删 2 组（`Fcmp` cond 1/2 的 `FEQ_S/D` 对）

+ 折叠判据放宽一档：除"单一差异段/纯插入"外，新增**同长度 + 各处差异都是孤立的 1 字符替换**
  （≤4 处）的判定 —— 用于同一侧出现**多次**的 `_S`/`_D`（如 `"FEQ_D {g}, {0}, {0}", "FEQ_D {g2}, {1}, {1}"`）。
  同时把 `[",\[\]{}]` 单 token 守卫**限定在单段路径**（多次替换时 `$ra/$rb` 会跨 `", "`，此前被误拒）。
+ `riscv64.toml` 的 `[[lowering]]` 块数 **98 → 96**（本轮累计 118 → 96，删掉 **22** 条宽度分派）。
+ `validate` + `lint` 干净；JIT 矩阵 58 passed / 0 failed；整仓 `-j 2` → exit 0、0 编译错误、0 失败。

### Changed (2026-10-08) — ISA-DSL v21 W5：riscv64 宽度分派再删 2 组（`Store`/`Fstore`）

+ 给 `SW/SD/FSW/FSD`（`[[instructions]]` 形态）补 `group` + `word_size`，严格折叠 2 对
  ⇒ `riscv64.toml` 的 `[[lowering]]` 块数 **100 → 98**（本轮累计 118 → 98，删掉 **20** 条宽度分派）。
+ **踩坑记录**：`(?m)$` 在 .NET 里会**吃掉行尾 `\r`**——把我的插入值拼上 `\r\n` 后写成 `\r\r\n`，
  报 `carriage return must be followed by newline`。修法：捕获值先 `TrimEnd("\r")` 再拼换行。
+ 随动：`lint.refs.riscv64` 14 → 16；`--strict-overlap` riscv64 16 → 14。JIT 矩阵 58/0。

### Changed (2026-10-08) — ISA-DSL v21 W5：riscv64 宽度分派再删 4 组（浮点算术 S/D 对）

+ 给 `FADD/FSUB/FMUL/FDIV` 的 `_S`/`_D` 对补 `group` + `word_size`（8 行），严格折叠 4 对
  ⇒ `riscv64.toml` 的 `[[lowering]]` 块数 **104 → 100**（本轮累计 118 → 100，删掉 **18** 条宽度分派）。
+ 随动：`lint.refs.riscv64` 10 → 14；`--strict-overlap` riscv64 20 → 16（被删的正是"特化 + 兜底"形状）。
+ JIT 矩阵 58 passed / 0 failed ⇒ 行为不变。未覆盖：`Store`/`Fstore`（`SW/SD/FSW/FSD` 行写法不同，下批处理）。

### Changed (2026-10-08) — ISA-DSL v21 W5：riscv64 宽度分派再删 8 组（累计 14 条），修展开误改，快照随动

+ 给 `ADD/SUB/MUL/DIV/DIVU/REM/REMU` 的 **W 变体对**补 `group` + `word_size`（14 行），再严格折叠 8 对
  ⇒ `riscv64.toml` 的 `[[lowering]]` 块数 **112 → 104**（本轮合计 118 → 104）。
+ **展开误改修复**：重载原先把行首名替换到**所有行**——`Popcnt` 序列里 `ADDI {g2}, …` 恰好以 `ADD`
  开头，被改成不存在的 `ADDWI`（`validate` 抓到）。现在只重写**提供行首名的那一行**。
+ 折叠判据另修两处：允许**不等长**（`ADDW` vs `ADD`）与**一侧为空**（纯插入/删除），安全性由
  "两侧必须同族"保证。
+ 快照/棘轮随动：`--strict-overlap` riscv64 27 → 20；`variants.rs`（RV32 投影 106 → 105、
  原生 122 → 125、丢弃 17 → 21、级联边界 34 → 35）；`pins.txt` 的 `lint.refs/vary_candidates`。
+ 整仓 `cargo test --workspace --exclude forge-rustc -j 2` → exit 0、0 编译错误、0 失败二进制
  （JIT 矩阵 58/0；`validate` + `lint` 干净）。

### Changed (2026-10-08) — ISA-DSL v21 W5：riscv64 删除 6 组宽度分派（降级重载接管，全门禁绿）

+ **只折叠"两侧都能被重载接住"的对**：先建"指令名 → (族名, word_size)"表，只对**同族**、
  两侧都带 `group` + `word_size`、且 `insts` 只差一个 token 的成对规则动手。本轮命中 **6 对**
  （`FEQ`/`FLE`/`FLT` 的 `_S`/`_D`），`riscv64.toml` 的 `[[lowering]]` 块数 **118 → 112**。
+ **展开语义修正**（JIT 矩阵抓到的真 bug）：重载在"`when` 里没有 `rs1_width` 叶子"时触发，
  并把自动叶子**合并**进原 `when`（原 `when` 成为 `and` 的一项），**不覆盖**——否则作者的
  `cond` 分派会被丢掉，行为改变。
+ 棘轮：`lint.refs.riscv64` 0 → 3（新增族名）。`validate` + `lint` 干净；
  整仓 `cargo test --workspace --exclude forge-rustc -j 2` → exit 0、0 编译错误、0 失败二进制
  （含 JIT 矩阵 riscv64 通道与变体投影快照）。

### Changed (2026-10-08) — ISA-DSL v21 W5：riscv64 浮点比较族补 `group` + `word_size`（重载前置）

+ `[family.FEQ]` / `[family.FLE]` / `[family.FLT]` 的 rows 补上 **`group` = 族名**（族名即多态
  引用名）与 **`word_size`**（`_S` = 32 / `_D` = 64）——这是"降级重载自动按宽度选行"能工作的前提。
+ `validate` + `lint` 干净；`forge-tests`（JIT 矩阵）58 passed / 0 failed ⇒ 加族名未改变任何行为。
+ 下一步：把 29 条 `when = { eq = ["rs1_width", …] }` 规则折成宽度无关的一条（脚本需处理
  **交错**的成对规则，不能只认相邻两条）。

### Added (2026-10-08) — ISA-DSL v21 W5：降级重载（`word_size` + 自动按宽度选行）

+ 新指令级键 **`word_size`**（位）：本条**指令形态**操作的数据宽度。与 `data_width`（纯搬运专用、
  供搬移族派生）是两件事——`word_size` 的消费者是**降级重载**。
+ **自动选行**：一条 `[[lowering]]` 规则**没有 `when`**、且行首 имен 解析到多条**只在 `word_size`
  上不同**的指令（同 `group`）时，引擎展开成"每条候选一条规则"，各自加
  `when = { eq = ["rs1_width", <该候选 word_size>] }`，并把行首名字换成该候选名。
  有 `when` 的规则不动（作者显式意图）。
+ 意义：这是目标第 (4) 项（宽度分派清理，52 条）的**前置**——作者从此只写一条宽度无关的规则。
+ 单测：宽度无关规则 → 展开成两条带 `rs1_width` 分派的规则（行首名字各换成候选名）。
+ 三方针同步（schema / 文档速查表 / `isa-dsl.schema.json`）。

### Changed (2026-10-08) — ISA-DSL v21 W5：x86 lowering 去重 +1（任意单差异对）

+ 打法推广：不只认 `SS`/`SD`，而是**自动求两条 insts 的单一连续差异段**（等长、前后缀全同），
  把它作为 zip 键的值对（`sfx = [左, 右]`）——只能**证明**等价的才合并，其余保持原样。
+ `x86.toml` 的 `[[lowering]]` 块数 **182 → 181**（累计 191 → 181）；本轮 3 组差异段不唯一 ⇒ 不动。

### Changed (2026-10-08) — ISA-DSL v21 W5：x86 lowering 去重再 +2 组（显式 `when` 对）

+ 新增打法：两条规则**都显式写** `when = { eq = ["K", V] }` 时，从两条读出 zip 值
  （`vary = { K = [V1, V2], sfx = ["SS", "SD"] }`）——**不猜默认值**，只合并能证明等价的。
+ `x86.toml` 的 `[[lowering]]` 块数 **184 → 182**（累计 191 → 182）；4 组形状不符保持原样。

### Changed (2026-10-08) — ISA-DSL v21 W5：x86 浮点 lowering 去重（13 组 → `vary` 族）

+ 把"同一 op 的两条规则只差 `elem` 谓词与 `SS`/`SD` 后缀"的 **13 组**折成一条 `vary` 规则：
  `Fsub`/`Fadd`/`Fmul`/`Fdiv`/`Ftrunc`/`Frem` 等（`vary = { elem = [1, 2], sfx = ["SS", "SD"] }`
  + 模板里 `{sfx}` 替换）。
+ `x86.toml` 的 `[[lowering]]` 块数 **191 → 184**（外形不符的 3 组**不动**，不硬凑）。
+ 逐簇验证：每组并入后跑 `validate` + `lint` + `forge-tests`（JIT 矩阵 58 passed）；本批末跑整仓门禁全绿。

### Added (2026-10-08) — ISA-DSL v21 W5：`family.vary` 落地（zip 语义与 lowering 共用一处）

+ **`vary` 的 zip 语义抽成唯一实现** `Lowering::zip_vary`：校验（非空、各列表等长）后按下标
  zip 成行，返回"每行一列键 → 取值"。`[[lowering]].vary` 与 `[family.<名>].vary` **共用它**。
+ **`[family.<名>].vary`**：与 `[[lowering]].vary` 同一类型；`name` 列表给每行的指令名 `inst`，
  其余键成为该行的字段（整数→整数字段，其余→字符串字段）。与显式 `rows` **互斥**（同时给明确报错）。
+ 单测：`family.vary` 与等价显式 `rows` 展开后**指令集逐条相同**；`rows` + `vary` 同时给要报错。

### Changed (2026-10-08) — ISA-DSL v21 W5：三谱 112 处模板迁到 `[family.<族名>]`（**破坏性**）

+ **112 处 `[[templates]]` 全部迁成 `[family.<族名>]`**（x86 18 / riscv64 24 / arm64 70），残留 0。
+ **关键修复**：`families` 是 `BTreeMap`（按名字字母序迭代），而**模板/指令声明序**决定别名首匹配与
  lowering 解析 ⇒ 直接按下标展开会**打乱指令表顺序**（实测 JIT 矩阵跑出非法指令）。现在按
  `[family.<名>]` 在**源码中的出现位置**排序展开，恢复声明序（迁移前后指令表**顺序+集合逐条相同**）。
+ 族名与指令名同池：arm64 8 个同名族加 `_FAM` 后缀；x86 无名模板取 `MOVZX_R8_RM_FAM` 并保留行上显式 `group`。
+ 逐字节不变：`isa_roundtrip_guard` + asm 棘轮 + 谱内向量 + 双 JIT 矩阵全绿。

### Fixed (2026-10-08) — ISA-DSL v21 W5：`family` 不再自动把族名铺成 `group`

+ `lower_families` 原先会把**族名自动注入为每行的 `group`**——而 `group` 是**多态引用名**，
  铺开后 `[[lowering]]`/`[[pattern]]`/`emit` 行首解析到的指令集合会变（三谱实测量：**0 处**
  引用模板/族名，属无用户的语义变更），JIT 矩阵因此崩（`STATUS_ILLEGAL_INSTRUCTION`）。
  现在**不自动注入**：族名要生效由**行里显式写 `group`**（x86 `MOVZX_*`/`MOVSX_*` 那族本来就是
  这么写的——`group = "movzx"`）。
+ 单测同步：`[family.X]` 与等价 `[[templates]]` 展开后指令集逐条相同、且**不注入** `group`。
+ 验证：`cargo test -p forge-isa-dsl` 全绿（276 + 7 + 3 + 4 + 4 + 2 …）。
  ⚠️ 整仓门禁本轮被 **linker `exit code 1102`** 挡住（`forge-isa-dsl`/`isa-host-demo` 的测试
  产物链接失败，疑似磁盘/句柄不足——环境性，与本次改动无关），清理 `target/` 或腾出磁盘后可复跑。

### Added (2026-10-08) — ISA-DSL v21 W5：`[family.<名>]` 族构造

+ 新构造 **`[family.<名>]`**（`body` + `rows`，预留 `vary`）：**族名来自表键且兼作"多态引用名"**
  （即指令上的 `group`）。
+ 实现 = **降级**：解析期展开成一条既有 `Template`（族名作 `name`，并自动成为各行的 `group`），
  **代码生成一行不改**；调用点在 `expand_templates()` 之前。
+ `vary` 暂未实现：写它会**明确报错**（fail-closed），留待与 `[[lowering]].vary` 统一 zip 语义。
+ 三方针同步；单测保证 `[family.X]` 与等价 `[[templates]]` 展开后指令集逐条相同。

### Changed (2026-10-08) — ISA-DSL v21 W5 第一步：`ref` → `group`（**破坏性**）

+ 指令的 TOML 键 **`ref` 改名 `group`**（族名即多态引用名；多条指令共用一个族名 = 多态引用）。
  发行谱/夹具/`.rs` 内联夹具/schema 键表与 doc 文案/文档速查表共 **121+ 处**同步；
  模型内部字段名 `reference` 保留（`group` 是用户面概念）。
+ 语义不变：逐字节闭环、asm 棘轮、双 JIT 矩阵、前缀守卫全绿。

### Added (2026-10-08) — ISA-DSL v21 W4.4 尾巴：守卫覆盖字段形态前缀（`match.prefix` 实际值）

+ 新增 `match_prefix_bytes`：把指令 `match = { prefix = … }` 的**实际前缀字节值**
  （`0` = 无前缀，排除）纳入守卫——解码正是靠这些字节区分指令
  （`MOVUPS_*`(0) vs `MOVSS_*`(0xF3) 共用 `0F 10 /r`）。守卫至此覆盖三种前缀形态
  （字节→效果名字典、有序字节列表、字段形态）。
+ 结论记录：共享前缀扫描循环已由"生效表"驱动，而生效表被守卫保证覆盖全部已声明前缀 ⇒
  "解码条件按 form 前缀段生成"**已等效成立**；把表改成从段派生是整洁项，暂无行为收益，
  且受"有序列表/字段形态不带效果名"所限（计划写在清单里）。

### Changed (2026-10-08) — ISA-DSL v21 W4.4 主体：删掉内置前缀扫描兜底 + 常驻等价性守卫

+ **删掉 `default_prefix_scan()`**：生效前缀扫描表 = 谱声明的 `[[conventions.prefix_scan]]`；
  **未声明即空表**（fail-closed）。前缀知识从此只在谱里——不再有"x86 风格内置兜底"
  隐式强加给别的谱。
+ 新增常驻守卫 `segment_prefixes_are_covered_by_the_scan_table`：段里声明的前缀字节必须在生效
  扫描表里，字典形态写了效果名时**效果必须一致**（x86/arm64/riscv64 三谱都跑）。
+ 逐字节不变：`isa_roundtrip_guard` + asm 棘轮 + 谱内向量 + 双 JIT 矩阵全绿。

### Changed (2026-10-08) — ISA-DSL v21 W4.3 收尾：指令级段扩六种 kind，x86 编码面迁移完成

+ 指令级段（选项 B）现支持：`opcode_reg`（`value` = 基值）、`imm`（`value` = 位数）、
  `rex`（`value` 0/1 ⇒ auto/always）、`vex`/`evex`（`map`/`pp`/`w`/`l` 来源键）、
  `modrm` 的**固定字节**（`value` ⇒ 经典 `modrm_fixed`）。
+ `isa/x86.toml`：剩余 13 条迁完（PUSH/POP/MOV_REG_IMM64/MOV_R_IMM32/BSWAP_R/MOVABS_GLOBAL/
  RET_IMM16/RETF_IMM16/JMP_REL32/CALL_RIP_REL/CQO/VMOVAPS_ZMM/MFENCE）；生效键抽查全一致。
+ **x86 谱里凡有编码键的指令都已段化**；仅靠 `opcode` + 操作数驱动编码的 6 条（RET/NOP/PUSHF 等）
  本就无结构可声明。
+ 逐字节不变：`isa_roundtrip_guard` + asm 棘轮 + 谱内向量 + 双 JIT 矩阵。

### Changed (2026-10-08) — ISA-DSL v21 W4.3 batch 12b 收尾：x86 段化 181/194

+ 剩余 20 条一次迁完：7 条走**指令级结构段**（MOVSD_RM/MOVSD_MR/MOVUPS_RM/MOVUPS_MR/PUSHFW/
  POPFW/PMULLD），13 条按骨架建 **5 个段形态 form**（SSE_IMM_0F_SEG/SSE_IMM_0F3A_SEG/
  JCC_REL32_SEG/VEX_IMM_SEG/EVEX_IMM_SEG，含 imm/rex.w/vex/evex 段）。
+ 生效键与迁移前逐条一致；`prefix = "field"` 按常量折叠。
+ `MFENCE` 不迁：它用 `modrm_fixed = 0xF0`，段模型暂无等价物。
+ 逐字节不变：`isa_roundtrip_guard` + asm 棘轮 + 谱内向量 + 双 JIT 矩阵。

### Changed (2026-10-08) — ISA-DSL v21 W4.3 batch 12b chunk2：6 条无 form 指令用指令级段

+ `isa/x86.toml`：`MOV_R_MEM_8`/`STORE_MEM_R_8`/`UD2`/`SETCC_RM8`/`SETCC_RM8_B`/`CALL_RM`
  迁到指令级结构段；**生效键前后逐条一致**。
+ 逐字节不变：`isa_roundtrip_guard` + asm 棘轮 + 谱内向量 + 双 JIT 矩阵；累计 **161/200** 段化。

### Changed (2026-10-08) — ISA-DSL v21 W4.3 batch 12b chunk1：首批无 form 指令用指令级段

+ `isa/x86.toml`：`MOVZX_R8_MEM`/`MOVZX_R16_MEM`/`CMPXCHG_MEM_R`/`CMPXCHG_MEM_R_8`/`XADD_MEM_R`
  迁到**指令级结构段**（选项 B）；生效键与迁移前一致（`prefix = "field"` + `match.prefix`
  折叠成常量 `bytes = ["0xF0"]`，字节等价）。
+ 修降级早退：`lower_stream_instructions` 在指令没有 `match` 时曾跳过整个函数，
  使"只有 segments + bind"的指令**丢掉 `modrm` 绑定**（且 `validate` 不报）——
  现在没有 `match` 也继续走 `bind` 分支。
+ 逐字节不变：`isa_roundtrip_guard` + asm 棘轮 + 谱内向量 + 双 JIT 矩阵；累计 **155/200** 段化。

### Added (2026-10-08) — ISA-DSL v21 W4.3 batch 12a：指令级段载体（选项 B）

+ `[[instructions]].segments`：不依赖 form 的指令可直接声明**结构段**
  （`escape` / `prefix` / `modrm`；段内**不给字段**——需要布局字段的场合仍用 form，
  非结构 kind 会明确报错并指路）。
+ `opcode` 值继续走指令的 `opcode` 标量；扩展码/常量走 `match`；操作数绑定走 `bind`
  （此前无 form 的指令拿不到 `bind` → `modrm` 的翻译，现已补上）。
+ 前缀解析收敛为**一处实现**（form 与指令级共用）；单测覆盖"指令级段 ≡ 经典内联键"与
  "`vex` 段在指令级被拒"。

### Changed (2026-10-08) — ISA-DSL v21 W4.3 batch 11：x86 最后一个 form 组段化（form 面收尾）

+ `isa/x86.toml`：`MRR_0F_FIX64` 的 5 条（MOVQ 系列）迁到 `MRR_0F_F64_PFX_SEG`；旧 form 已删。
+ 该 form 未声明 `rex_w = "field"`，故新段形态**不带 rex 段**——否则 `match.w` 会由"无意义"
  变成真字段，把 `rex_w` 从 Auto 改成 Field，语义就变了。
+ **x86 谱里所有 form 都已是段形态**；剩余 50 条是"把编码键写在指令上"的无 form 指令
+ （`segments` 目前只挂在 form 上，需给指令加段载体——即 A/B 分叉里的选项 B）。
+ 逐字节不变：`isa_roundtrip_guard` + asm 棘轮 + 谱内向量 + 双 JIT 矩阵；**150/200 段化**。

### Added/Changed (2026-10-08) — ISA-DSL v21 W4.3 batch 10：前缀段有序列表 + LOCK 族段化

+ **前缀段的第三种形态**：`bytes = ["0xF2", "0xF0"]`（**有序字节列表**，发射序 = 写序）。
  必需：字节→效果名字典按字节字典序排，会把 XACQUIRE/XRELEASE 的 `0xF2 0xF0` 排反。
  前缀段至此三形态齐备：① 字节→效果名（常量+语义）② `fields`（值在指令 `match`）③ 有序字节列表。
+ `isa/x86.toml`：`ACQUIRE_LOCK_ADD_MR`/`RELEASE_LOCK_ADD_MR` 迁到 `MRR_MEM_XACQ_SEG`/`MRR_MEM_XREL_SEG`。
+ 逐字节不变：`isa_roundtrip_guard` + asm 棘轮 + 谱内向量 + 双 JIT 矩阵；累计 **145 条**已段化。

### Changed (2026-10-08) — ISA-DSL v21 W4.3 batch 9：x86 VEX/EVEX 五组 21 条段化

+ `isa/x86.toml`：`VEX_RRV`(6)、`VEX_RR`(3)、`VEX_RR_MEMREF`(5)、`EVEX_RRV`(2)、`EVEX_RR_MEMREF`(5)
  迁到段形态 form；三个零引用旧 form 已删。
+ **`vex`/`evex` 段首次被真实谱使用**：`{ kind = "vex"|"evex", map/pp/w/l = "field" }`
  （EVEX 另加 `disp_scale = "field"`）——来源键与旧 form 的 `vex = {…}` 一致，
  值仍由指令 `match = { vex_map = … }` 给。
+ 修一处 vlen 误用定宽路径：`lower_field_syntax` 的位置绑定（`bind_operands`）对 vlen 整体跳过。
+ 逐字节不变：`isa_roundtrip_guard` + asm 棘轮 + 谱内向量 + 双 JIT 矩阵；累计 **143 条**已段化。

### Changed (2026-10-08) — ISA-DSL v21 W4.3 batch 8：x86 零散组 10 条段化

+ `isa/x86.toml`：`MRR_MEM_0F_FIX32`(6)、`MRR_0F_PREFIX`(3)、`NOP_RM`(1) 迁到各自的段形态 form；
  两个零引用旧 form 已删。
+ 两条 LOCK 原子指令**暂不迁**（`prefix = ["0xF2","0xF0"]` 是有序两字节，段形态的前缀字典
  按字节字典序排会反序）——待前缀段的**有序列表**形态落地。
+ 逐字节不变：`isa_roundtrip_guard` + asm 棘轮 + 谱内向量 + 双 JIT 矩阵；累计 **122 条**已段化。

### Changed (2026-10-08) — ISA-DSL v21 W4.3 batch 7：x86 六个小组共 21 条段化

+ `isa/x86.toml`：`MRR_MEM_PRE`(4)、`MRR_MEM`(3)、`MRR_FIX64`(3)、`MRR_MEMREF`(5)、`MRR`(3)、
  `MRR_EXT_OP_FIX64`(3) 各迁到自己的段形态 form；6 个零引用旧 form 已删。
+ 逐字节不变：`isa_roundtrip_guard` + asm 棘轮 + 谱内向量 + 双 JIT 矩阵；累计 **112 条**已段化。

### Changed (2026-10-08) — ISA-DSL v21 W4.3 batch 6：x86 两组共 16 条段化

+ `isa/x86.toml`：`MRR_EXT_OP` 的 8 条 → `MRR_EXT_OP_SEG`（opcode + modrm(/ext)）；
+ `MRR_0F` 的 8 条 → `MRR_0F_SEG`（0F escape + opcode + modrm）；零引用的 `MRR_0F` 已删。
+ 逐字节不变：`isa_roundtrip_guard` + asm 棘轮 + 谱内向量 + 双 JIT 矩阵；累计 **91 条**已段化。

### Changed (2026-10-08) — ISA-DSL v21 W4.3 batch 5：x86 SSE 族 21 条段化 + 段模型两项能力

+ `isa/x86.toml`：`SSE_RR` 21 条迁到 `SSE_RR_SEG`（prefix + 0F escape + rex.W + opcode + modrm）。
+ **前缀段可取字段**：`{ kind = "prefix", fields = ["u8[7:0]:prefix"] }` 降级成经典
  `prefix = "field"`（值与"字节 → 效果名"的常量形态并存）。
+ **`rex_w` 可来自字段**：rex 段的 `w` 字段**无默认值** ⇒ 经典 `rex_w = "field"`（值由指令的
  `match = { w = … }` 给）；带默认值 ⇒ 固定 0/1。
+ 修 lint 误报：**段内字段各自属于自己的字节空间**（前缀字节/REX 字节/ModRM 字节），
  不按"同一个字里的位"判重叠——此前 `prefix`[0,8) 与 `w`[7,8) 被误报为抢同一批位。
+ 逐字节不变：`isa_roundtrip_guard` + asm 棘轮 + 谱内向量 + 双 JIT 矩阵；累计 **75 条**已段化。

### Changed (2026-10-08) — ISA-DSL v21 W4.3 batch 4：x86 imm32/imm8 族 19 条段化

+ `isa/x86.toml`：`MRR_EXT` 19 条按 `imm` 宽度拆成两个段形态 form——
  `MRR_EXT_IMM32_SEG`（8 条）与 `MRR_EXT_IMM8_SEG`（11 条）。
+ 首次使用 **`imm` 段**：立即数**宽度是结构事实** ⇒ 进 form 的段
  （`{ kind = "imm", fields = ["u32[31:0]:imm32"] }` 降级成 `keys.imm = 32`），
  指令侧的内联 `imm = N` 随之删除。
+ 逐字节不变：`isa_roundtrip_guard` + asm 棘轮 + 谱内向量 + 双 JIT 矩阵；累计 **54 条**已段化。

### Changed (2026-10-08) — ISA-DSL v21 W4.3 batch 3：x86 0F 转义族 8 条段化

+ `isa/x86.toml`：`MRR_MEMREF_AUTO` 的 `escape` 子集 8 条（IMUL_R_MEM、MOVZX/MOVSX_MEM8/16、
  NOP_MEM32、SETCC_R_MEM、CMOVCC_R_MEM）迁到段形态 form `MRR_MEM_0F_SEG`——
  `escape` 是**结构事实**（`bytes = ["0x0F"]`），由 form 的段承载，指令侧只写 `match`/`bind`。
+ 逐字节不变：`isa_roundtrip_guard` + asm 棘轮 + 谱内向量 + 双 JIT 矩阵；累计 **35 条**已段化。

### Changed (2026-10-08) — ISA-DSL v21 W4.3 batch 2：x86 内存族 13 条段化 + 扩展码表达定案

+ `isa/x86.toml`：`MRR_MEMREF_AUTO` 的干净子集（MOV/STORE/XCHG 内存形式 + NOT/NEG/INC/DEC/
  MUL/IMUL/DIV/IDIV）迁到段形态 form `MRR_MEM_SEG`（`opcode` + `modrm` 段）。
+ **扩展码（`/2`、`/7`）的表达定案**：**常量进 `match`（整数）、操作数绑定进 `bind`（名字）**——
  `modrm = { reg = 2, rm = "[mem]" }` 迁成 `match = { opcode = 0xF7, reg = 2 }` +
  `bind = { rm = "[mem]" }`；降级把 `match.reg` 翻成 `Modrm.reg = Ext(n)`。
+ 逐字节不变：`isa_roundtrip_guard` + asm 棘轮 + 谱内向量 + 双 JIT 矩阵；累计 27 条已段化。

### Fixed (2026-10-08) — ISA-DSL v21 W4.3：段降级不再覆写 form 的既有编码键

+ `form.keys = keys`（段键**整体覆写**）会把 form 自己写的语义键（`opsize`/`prefix`/
  `rex_w`/`escape`…）冲掉 ⇒ 编码/解码按错误宽度裁决（实测：x86 `MOV64_RR` 的 `48 89 C1`
  被解成 32 位）。改为 **段键覆盖、既有键兜底**（`keys.over(&form.keys)`）。
+ 同片：`isa/x86.toml` 的 **14 条**核心 RR 指令（MOV/ADD/SUB/XOR/AND/OR/CMP/TEST/ADC/SBB…）
  迁到段骨架 form（`segments` 声明 `opcode` + `modrm` 布局，指令只写 `match = { opcode = X }`），
  逐字节不变由 `isa_roundtrip_guard` + asm 棘轮 + 谱内向量共同守住。

### Added (2026-10-08) — ISA-DSL v21 W4.3：段字段值 → 经典标量（`match` 推广到 stream）

+ stream form 声明段布局；指令用 `match = { opcode = 0x8B }` 给段的 `opcode` 字段赋值，
  降级把它翻译成 `Instruction.opcode`（编码器读的标量）——**编码器一行不改**，
  字节按构造不变（vlen 的 opcode 原本来自专用标量、不读 `match`，这是补齐的那一段）。
+ 同片修掉两个"把定宽键塞进 vlen"的地雷：`opcode_field`（`validate` 拿它判 `is_fixed`）
  与 `operand_fields`（定宽位域索引）在 `prefix_scan` ISA 上一律不写——否则 v21 段模型
  给 vlen form 加了字段就会把校验/编码带进定宽分支。

### Fixed (2026-10-08) — ISA-DSL v21 W4.3 batch1：`modrm` 段不再强设 `keys.modrm`

+ 实测 `MOV_R_RM` 的生效编码键只有 `{opsize="s0"}`（**没有** `modrm`）而字节正确
  ⇒ vlen 编码器**由操作数驱动**发射 ModRM。段里声明 `modrm` 时若强设一个默认
  `ModrmMap`，就把"操作数驱动"改成了"键驱动"——是行为改变，已去掉。
+ 段在此的职责是声明**字节布局**（`mod`/`reg`/`rm` 三个字段进 `form.fields`，
  供 `bind`/`match` 引用与 `--bits` 清点）；谁进 reg/rm 由指令 `bind` 给出。

### Added (2026-10-08) — ISA-DSL v21 W4.3 batch 1：变长 ISA 的 `bind` 落到 `modrm`

+ `bind = { reg = "dst", rm = "src" }` 在 `kind = "prefix_scan"` 的 ISA 上写进指令的
  `ModrmMap`——此前 vlen 路径**完全不读** `bind`（定宽 ISA 走 `operand_fields`），
  ModRM 的 reg/rm 归属只能靠位置缺省。
+ 扩展码（`/4`/`/5`/`/7` 这类整数）不在 `bind` 的表达面内（`bind` 的值是字符串），
  仍写 `modrm = { reg = 4 }`；两者可叠加（`bind` 只覆盖它给出的那半边）。

### Fixed (2026-10-08) — ISA-DSL v21 W4.2：前缀效果名闭集校验

+ 段（`prefix`）里的效果名以 `PrefixEffect::NAMES` 为唯一来源（`opsize16`/`lock`/`repe`/
  `repne`/`addr32`/`addr16`/`rex`/`rex2`）——写错在**声明期**报错并列出全集。
  （W4.0 的实现不做校验，垃圾效果名会静默通过。）
+ `opsize16` ⇔ 经典 `prefix` 键的 `"opsize"`（两处拼法不同，降级时换算）；
  单来源降级成 `PrefixKey::One`、多来源成 `Many`（与序列化往返一致）。

### Added (2026-10-08) — ISA-DSL v21 W4.1：段 kind 闭集全部落地

+ `opcode_reg` 段（`+r` 形式，需 `value` 基值）、`vex`/`evex` 段（头字段来源键 `map`/`pp`/
  `w`/`l`，与 `[forms].vex` **同一组键**，降级原样搬进 `EncKeys`）。
+ `sib`/`disp` 段是**布局段**：没有对应的语义键（SIB/位移字节由编码器按内存操作数派生），
  价值在"把字节布局写进谱并进 `form.fields` 参与校验与 `--bits` 清点"。
+ fail-closed：空的 VEX/EVEX 头（一个来源键都没有）、缺 `value` 的 `opcode_reg` 报错；
  重复段 kind、坏字节字面量、未知段 kind 也各有明确报错。

### Added (2026-10-08) — ISA-DSL v21 W4.0：指令流段模型（骨架，暂无谱使用）

+ `[[forms]].segments`：`[{ kind = "prefix" | "escape" | "rex" | "opcode" | "opcode_reg" |
  "modrm" | "sib" | "disp" | "imm" | "vex" | "evex", fields/bytes/value }, …]`——**有序段
  即发射序**，段内位域沿用 W2 的字段语法（并入 `form.fields` 后由同一处展开）。
+ 段 kind 是**闭集**：写错由 serde 报 unknown variant 并列出全集；未实现的段
  （`opcode_reg`/`sib`/`disp`/`vex`/`evex`）**明确报错**而不是静默忽略。
+ 降级到既有语义键（编码器与生成物一行未改）；等价性单测：段写法与经典写法得到
  **同一份** `EncKeys`。三方针（model/schema/docs）已同步；`isa/*.toml` 尚未使用。

### Changed (2026-10-08) — ISA-DSL v21 W3b：命名取值表统一为 `[enum.*]`（**破坏性**）

原先三张各写一套的表合并成 `[enum.<表名>]`：

+ `[conventions.cond]` → **`[enum.cond]`**（条目形状不变：`e = { code = 4, ir = "eq" }`；纯别名写 `c = { code = 2 }`）。
+ `[conventions.bitsets.<表>]` → **`[enum.<表>]` + `kind = "bits"`**（名字拼接、编码取按位或）。
+ `[conventions.imm_names.<表>]` → **`[enum.<表>]`**（一个名字 = 一个值；也支持简写 `mstatus = 0x300`）。
+ 判定：表名叫 `cond` **或**任一条目带 `ir` ⇒ 条件码表（内部只有一处，两张就报错）；
  `kind = "bits"` ⇒ 位集合表；否则值表。槽侧照旧用 `enum = "<表名>"` 引用。
+ **编码字节逐条不变**；`cargo test --workspace --exclude forge-rustc` → exit 0、0 编译错误、0 失败二进制。

### Changed (2026-10-08) — ISA-DSL v21 W3a：操作数声明收敛（**破坏性**）

`[[operand_slots]]`（数组）改为 **`[operand.<名字>]` 表**，8 个一次性开关收敛成正交键：

+ `width → bits`、`min`+`max → range = [lo, hi]`、`float → value = "float"`、
  `wrap → literal = "bits"`、`table`/`names → enum = "<表名>"`、`arrangement → suffix`、
  `symbols`/`require_symbol`/`imm_fns → symbol = { allow, require, modifiers }`、`zr31 → zero`/`sp`；
  内存文本形态从 `[conventions.mem]` 移到槽上（`text` / `size_words`）。
+ `byte_reg` 落成槽级 **`byte = true`**：x86 `setcc` 的 rm 用 64 位名字却按 8 位寄存器编码
  ⇒ 是槽的事实，不是寄存器组的事实（设计文档原打算挂 `[reg.*].rex_required`，实证推翻）。
+ 槽相关的诊断文案改为 `[operand.名]`；imm 槽的 `enum` 指到位集合表时**点名**指错。
+ **编码字节逐条不变**：`isa_roundtrip_guard`、谱内向量、asm 三档棘轮、双 JIT 矩阵全过；
  `cargo test --workspace --exclude forge-rustc` → exit 0、0 编译错误、0 失败二进制。

### Changed (2026-10-08) — ISA-DSL v21 W2：字段就地声明（**破坏性**，无兼容层）

`[conventions.bitfields]` + `[[forms]].opcode_field`/`operand_fields` + 指令的 `fields = {…}` 三处往返，被**一处就地声明**取代：

+ **`fields = ["类型[位区间]:名字=默认值", …]`**（写在 `[[forms]]` 或 `[[instructions]]` 上）：连续写 `"u5[11:7]:rd"`；散射写 `"i13[12:1 -> 31,30:25,11:8,7]:imm_b"`（值位段 → 词位块，按值低位→高位；与 riscv S/B/J 既有 `pieces` **逐段等价**，有单测证明）。类型支持 `uN`/`iN`/联合 `|`/寄存器组名/槽名，默认值支持 `0x`/`0b`/负数。
+ **主 opcode 字段统一叫 `opcode`**：v18 的 `opcode_field` 名是 ISA 私有的（arm64 有 `op8`/`op9`/`mtop`/`word`/`vec_a` 等 **10 种**），现在按"名字即接口"统一。
+ **操作数 → 字段绑定 = 口径 A**：`bind` 显式优先 → 否则**同名** → 否则按 `fields` **声明序**取第 i 个（跳过 `opcode`）= v18 `operand_fields` 的位置口径。三谱迁移因此**零 `bind`**（只有 arm64 向量排列族 13 条"三操作数绑同一字段"需显式写）。
+ **指令固定值键 `fields` → `match`**（同一提交内完成，先解键冲突）。
+ **内部命名空间按 form 分区**：同名不同区间时内部键为 `<form>.<名字>`——v18 的位域表是全局命名空间，v21 的字段是 form 局部的（arm64 多个 form 各要一个 `opcode`，区间还不同）。对编码器零改动：下游一律"按名字查表"。
+ **迁移范围**：三份发行谱 + 全部夹具谱，共 **76 个 form** 改写 + 13 条显式 `bind`；`opcode_field`/`operand_fields`/`bitfields` 三个键改为 `#[serde(skip)]` 派生，写了即报未知键。
+ **编码字节逐条不变**：`isa_roundtrip_guard`、谱内向量、asm 三档棘轮、x86/riscv64 JIT 矩阵**全部与迁移前逐数字相同**。
+ `lint --bits` 清单如实变长（riscv 7→13、arm64 262→264）：form 声明但**本指令未绑定**的槽不再算作该指令的覆盖（那些位按缺省 0 发射）——口径更精确，快照已同步并注明原因。

### Added (2026-10-07) — riscv64：`jal rd, rs` 别名（清掉棘轮里最后一条已登记红桶）

`rv32i-valid.s:88` 的 `jal a0, a0` 是 `jalr rd, rs, 0` 的**别名**（LLVM 接受该写法）。新增 `JAL_R`
（`form = "I"`、`opcode = 0x67`、`funct3 = 0`、`ops = ["dst:gpr:out", "src:gpr"]`）：与
`jal {dst}, {target}` 首段相同 ⇒ 靠**第二操作数的槽类型**分派（reg 槽比标签/偏移槽更具体）；
`I` 形式的 `imm12` 不绑操作数 ⇒ 编码缺省发 0、解码的补集零守卫要求它 0（正是别名语义）。

实测：riscv64 `parsed` 1776 → **1777**、**`tail_mismatch 1 → 0`**（棘轮里最后一条已登记红桶清零）。
**加一条 riscv 指令牵动多处钉死值**（都已同步）：`pins.txt` 的 `spec_totals`（173 → 174）与
`roundtrip.derived`（622 → 624）；`forge-isa` CLI 测试的 `"173 条指令"`（**已改为读快照**，不再手写
数字）；`variants.rs` 里该测试的原生视角计数（173 → 174）与 RV32 投影计数（140 → 141，多处）。

### Fixed (2026-10-07) — x86 EVEX 双精度（PD）族的 **W 位**：`vaddpd zmm…` 一直编错

`VADDPD_ZMM`/`VSUBPD_ZMM`/`VMULPD_ZMM`/`VDIVPD_ZMM` 四条行复用了**单精度**模板（`evex_w = 0`），
而 EVEX 的双精度必须 **W=1** ⇒ 编码字节的 **P1 位 7** 少 1：

```text
vaddpd zmm1, zmm1, zmm2 : 我们 62 F1 75 48 58 CA ≠ 上游 62 F1 f5 48 58 CA
                                   ^^ P1              ^^ P1（W=1）
```

修法：给这 4 行各自补 `fields = { evex_w = 1 }`（其余 P0/P2/opcode/ModRM 本来就对）。

**实测**：全集编码 `known` **157 → 148（−9）**（`avx512-intel.s` 里的 PD 条目全部消失）；
`validate`/`lint` 干净，四档 `verify=0`，`ws=0`。
（`known` 只在全集运行里非空——vendored 集恒 0，所以这条修复不会让门禁数字变化，只会让**真错编码**消失。）

### Changed (2026-10-07) — asm 语料**不入 git**（本地缓存 + 缺语料显式跳过）

`asm/parse/**` 进 `.gitignore` 并从索引移除（133 个文件；盘上保留）。**为什么**：语料体积大、
"留哪些"由 `fetch.mjs` 的逐文件取舍**算出来**，签进仓库只带来 churn、`git add -A` 误跟踪、
以及"裁回没跑成 ⇒ 棘轮拿错值"这类错。**怎么用**：`node crates/tools/forge-tests/asm/fetch.mjs`
（默认裁剪）/ `--keep-all`（量全集）；`asm/PROVENANCE.md` 仍跟踪，作语料身份的单一事实源。
**干净克隆**：`asm/parse` 缺失时三档套件打 `ASM-CORPUS-MISSING` **显式跳过**（退 0），不假绿——
判据 `asm::corpus::corpus_present()`。实测：语料在盘 ⇒ 四档 `verify=0`；移开后 ⇒ 两条跳过且退 0。

### Added (2026-10-07) — aarch64 `adr`/`adrp`（数值 + 符号两形态，117 行）

**位布局**：`adr_op[31]`（adr=0/adrp=1）+ `adr_fix[28:24]`（固定 `10000`）+ `immlo[30:29]` +
`immhi[23:5]` = 21 位有符号立即数**拆两段**，用既有 `encode = "slice"` 落点（LSB 先切）。

**值语义**：`adrp` 的操作数是**页号** —— 槽写 `unit = 4096`（语料 `adrp x0,#4096` = `00 00 00 b0`
⇒ `immlo=1, immhi=0` 反证，即字节地址 ÷ 4096）；`adr` 是字节偏移（`unit = 1`）。

**两形态 + 分派**：数值槽（`adr_off`/`adrp_off`）与符号槽（`adr_sym`，占位 0 由重定位填）同形共存，
符号槽必须写 **`require_symbol = true`**——只写 `symbols = true` 时它会**连普通字面量一起收**
⇒ 抢走数值写法、把 `#4096` 送进页重定位通路（实测编成 `0x90008000` 而非 `0xB0000000`，
根因就是这里）。`require_symbol` 让"没记到符号 ⇒ 直接不匹配"，数值写法自然落回数值槽。

实测：`parsed` 2068 → **2093**、编码 **`known` 57 不变**；守卫 540 → **544**、枚举器 2541 → **2565**。
（`tail_mismatch` 同步 +36：新语法让相邻方言写法 `foo@PAGE` / `:lo12:` 之类由 `no_prefix` 变红桶，
属已归因的"可见化"，在**已淘汰**文件内、棘轮不受影响。）

### Fixed (2026-10-07) — 切片+单位槽的解码符号丢位（阻塞 ADR/ADRP）；`forge-rustc` 漏注册编译管线（6 条 CLI e2e 红）

**① 切片解码：先符号扩展、再乘回单位**（`dsl/codegen/mod.rs`）
`encode = "slice"` + `unit > 1` + `signed` 三者叠加时，解码侧原实现**先左移 unit 再按 `slot.width`
符号扩展**，符号位在左移时被推出 ⇒ 负值解成 0：实测 `adrp x0,#4096` 的最小值 `-2^32` 被解成 `0`
（生成期用例 `encode∘decode 字节不稳定`，byte2 的 `immhi` 最高位丢）。改为**先按切片总宽（Σ 字段宽）
符号扩展、再乘回 unit**；扩展宽度用**切片总宽**而不是 `slot.width`（两者仅在"槽宽 == 切片总宽"时相等，
ADR 的 21=21 是巧合而非契约）。
回归：夹具 `demo_inst12` 的 `off_scale4` 打开 `signed = true` + 新增 `ldw A0, -4` 向量
（`0x63F` = `[0x3F,0x06]`），生成期自测随即采样 min/max 覆盖该通路。

**② `forge-rustc` 注册编译管线**（`tools/forge-rustc/src/compile.rs`）
两处编译入口只调了**生成物**注册（`backend::<isa>::ensure_registered`），漏了**编译管线工厂**
（`backend::pipeline_hooks::ensure_registered()`）⇒ 经 backend dll 编译时报
`ISA \`x86_64\` 未注册编译管线`，`cargo-forge` 的 **6 条 cli_tests 全红**（差分实验证明是**既有真缺陷**：
干净 HEAD 上同样失败）。补齐后重建 dll 实测 **6 passed / 0 failed**。

**③ `cli_tests` 陈旧 dll 显式 SKIP**（`tools/cargo-forge/tests/cli_tests.rs`）
dll 存在但**早于 `crates/tools/forge-rustc` 源码** ⇒ 它是另一份源码构建的，注册表口径与当前源码不符
（表现为上面的"未注册编译管线"，看起来像源码回归）。新增新鲜度判据 ⇒ 显式 SKIP 并写明"先
`cargo build -p forge-rustc`"，不误红也不假装通过。

**④ 守卫钉死值纠偏**（`tests/lint_shipped.rs`）
`unreferenced_ref_inventory` 的 arm64 值被误改成 262（那是 `unassigned_bits_inventory` 的值）⇒ 改回
**27** 并写明两个同名 `arm64.toml` 条目不得混淆。

**验收**：`cargo test --workspace --exclude forge-rustc` = **ws 0**；四档 asm 套件 `verify=0`；
markdownlint 114 条全在 `docs/archive/`（现行树零警告）。

### Added (2026-10-07) — aarch64：比较/乘法族批量接入（`smax`/`smin`/`umax`/`umin`/`mul`/`mla`）

共享 `varr` 形态 + **逐族常量**（从语料 `.8b` 字反推）：`smax d=25`、`smin d=27`、`umax vu=1 d=25`、
`umin vu=1 d=27`、`mul d=39`、`mla d=37`（`vu` 即 U 位，`d=[15:10]`）。**一次提交**（六族）。

实测：`parsed` 2010 → **2053（+43）**、`no_prefix` 14219 → **14137（−82）**、编码 `checked` 1021 →
**1057**、**`known` 57 不变**；守卫 534 → **540**、枚举器 2457 → **2541**。
**红行 1386 → 1425（+39）已归因**：这些族的**lane 形态**（如 `smin v0.8b, v1.8b, v2.b[3]`）尚未实现，
新拼法使它们由 `no_prefix` 变为 `tail_mismatch`；**lane 族因此成为红行指标的关键待办**
（实现它即可回收这批可见化）。

**流程**：批量做、最后提交一次；`known` 不变 = 提交前提（脚本里判，不靠人记）。
注意：门禁正则必须**锚定 `aarch64` 行**（我第一版抓到 x86 的 `known=9` 导致误判不提交）。

### Added (2026-10-07) — aarch64：`orr` 族接入（**逻辑族的第二种形态**：自带表 + 自带槽）

逻辑/位运算族的 `[23:22]` **不是元素宽**而是常量（`orr.8b` = `0x0EA01C00`，`[23:22]=10`）⇒ **不能**共用
`varr`（那会把元素宽写进去）；改为该族**自带**：`[conventions.imm_names.arr_q]`（排列只决定 `Q`：
`8b/4h/2s=0`、`16b/8h/4s/2d=1`）+ 槽 `varr_q`（`width=1`、`fields=["vq"]`）。

实测：`parsed` 1976 → **1981**、红行 1420 → **1415（−5）**、`checked` 1011 → **1013**、
**`known` 57 不变**；守卫 529 → **530**、枚举器 2387 → **2401**。
⇒ 两种形态都已验证：**元素宽在 `vsize` 的族**用共享 `varr`（`add`/`sub`），**元素宽不在那儿**的族
用自带表+槽（`orr`）。其余族按此归类推进即可。

### Added (2026-10-07) — aarch64：`sub` 族接入排列后缀新拼法（零 `known` 增长、红行下降）

按新确立的**逐族判据**落地第二族：`sub {dst}.{arrd}, …`（`VEC3RA` + 共享 `varr` 槽）。
依据是**同族旁证**而非猜测：`sub` 与 `add` 同编码族、只差 `U` 位（语料 `sub.8b` 的 `vu=1` 已证），
故常量 `(vu,c,d) = (1,1,33)` **与排列无关**，可共用共享槽（`smax/smin` 那类 size 写在 `vec_d` 的族
将来需自带表+槽）。

实测：`parsed` 1968 → **1976**、红行 1428 → **1420（−8，下降）**、编码 `checked` 1004 → **1011**、
**`known` 57 不变**（这就是判据）；守卫 528 → **529**、枚举器 2373 → **2387**。
**方法固化**：一族一次、每次以 `known` 不变为前提；语料字节稀疏（`sub` 仅 `.8b` 一条）时用
"同族布局 + 上游 `.8b` 的 U/size 位"作旁证，并用 `known` 当最终裁判。

### Added (2026-10-07) — 汇编器：NEON 排列的**读时切分**（两侧同源）+ `add` 族两种拼法

**机制**（本线剩余最大一块的前置件）：`v0.16b` 这类"寄存器 + 排列后缀"在词法器里是**一个标识符**
（`.` 属延续字符）。实测**改词法会让整跑崩**（`0xC0000409`），因此改为**读时切分**：运行时 `__lex`
出词元后把 `Ident("V0.8b")` 切成 `Ident("V0")` + `Dot` + `Ident("8b")`；**生成期的字面段匹配器做
同一套切分**（`lit_seg_expr` → `split_arrangement_ident`，并把孤立的 `.` 归一为 `Dot`）。**两侧同源
是硬要求**：只切一侧会让 `abs.8b` 族全体 `no matching instruction`（实测）。

**`add` 族两种拼法**：① 新增 `VEC3RA` 表单 + `varr` 槽（`[conventions.imm_names.arr]` 命名立即数 +
既有 `encode = "slice"` + `fields = ["vq","vsize"]`）⇒ **一条指令覆盖全排列**
（`add v0.16b, v1.16b, v2.16b`）；② 保留助记符带后缀的拼法（`add.16b v0, v1, v2`）——语料两种都写，
这是**两种语法**、不是冗余设计。排列代码表：`8b=0 / 16b=1 / 4h=2 / 8h=3 / 2s=4 / 4s=5 / 2d=7`
（`code = size*2 + q`；**`2d` 是 7 不是 6**——写 6 会让 `.2d` 编成 `q=0`，实测编码 `known` +1）。

实测：指令总数 527 → **528**、枚举器 2359 → **2373**；语料 `parsed` 1960 → **1968**、`no_prefix`
14284 → **14219**、编码 `checked` 997 → **1004**、**`known` 57 不变**。**红行 1371 → 1428（+57）已归因**：
切分使"助记符我们支持、但寄存器后缀拼法尚未实现"的行由 `no_prefix`（不计缺陷）变为 `tail_mismatch`；
这些行都在**已淘汰**文件里，**棘轮不受影响**，消掉它们正是下一步（按族铺开 + 破坏性替换）。

### Added (2026-10-07) — ISA-DSL：新槽键 `arrangement`（排列是**槽的参数**，不是指令的身份）

**破坏性重设计的契约层**：NEON 的排列此前被展开成"每族 × 每排列一条指令"（A64 约 100 条声明，
彼此只差 `Q`/`size` 两个字段）——既冗余，又**接不住语料主流写法**（`add v0.16b, v1.16b, v2.16b`
的助记符**不带**后缀；实测把后缀"吃掉"对语料**零收益**，因为排列正是**编码依据**）。改成槽参数后：
**一条指令覆盖全排列**、谱面净减。

形状（**排列名 → 代码值**，与既有 `encode = "slice"` 对齐，不引入第二张映射表）：

```toml
[[operand_slots]]
name = "v"
kind = "reg"
class = "fpr16"
arrangement = { "8b" = 0, "16b" = 1, "4h" = 2, "8h" = 3, "2s" = 4, "4s" = 5, "2d" = 6 }
encode = "slice"                 # 代码值按 fields 声明序从最低位切起（复用既有机制，零新机制）
fields = ["vq", "vsize"]         # 例：16b ⇒ vq=1、vsize=0（代码 1 = 0b001）
```

（早先一版写成 `"8b" = { vq = 0, vsize = 0 }`——**实现不了**：一个槽只能落一个字段，而排列要落
`vq`+`vsize` 两处、寄存器还要落它自己的字段。）

校验：只对 `kind = "reg"`；键名必须是「数字 + `b`/`h`/`s`/`d`」；**必须**同时写 `encode = "slice"`
与 `fields`；代码值**必须互不相同**（否则渲染无法唯一反推后缀 ⇒ 反汇编会漂移）。schema 已重生成，
`docs/reference/isa-dsl.md` 键总览同步（三方针守卫全绿）。

**下一步（同一重设计）**：解析侧把寄存器后的 `.Nt` 读出来（校验在允许集内、把代码值交给既有切片
编码）、渲染侧按同一张表写回后缀（保证 `disassemble → assemble` 闭合），然后**删掉**约 100 条
`族×排列` 声明、改为每族一条；守卫的 arm64 计数与未指定位清点届时按实际**下降**重算。

### Fixed (2026-10-07) — `unit` 在**位切片**编码路径被跳过（分段立即数 + 单位缩放会写错字段）

`[operand_slots].unit`（源值单位）原先只在**单字段**编码路径生效：`encode = "slice"` 的
**多字段**分支直接 `let __mv = *#fid as u64;`，把**源值**当字段值切片。于是任何"分段立即数 + 单位缩放"的形态都会写错——实测 ADR/ADRP 家族里 `adrp x0, #4096`（页单位）编成

`[0,128,0,144]`，上游是 `[0,0,0,176]`（差一整个字节）。

修法（与单字段路径**对称**，两行概念）：

**编码侧**：切片前先把源值除以单位（`imm` 槽查整除并报错；`label` 槽只移位不查——它塞的是
  标签 id 占位、随后由 reloc patcher 改写，查整除只会误报，口径与单字段路径一致）；
**解码侧**：拼回各段后乘回单位（缺这一步则"编码除了、解码没乘回来"的往返字节不稳）。

同时把**生成期采样**对齐到单位（`spec.rs`）：`imm` 槽的 `lo`/`hi` 采样值必须取单位的整数倍
（朝零对齐，仍落在原值域内），否则边界自测会构造出"编码期整除检查必然拒绝"的用例。

**证据**：应用本修复后，ADR/ADRP 的 4 条上游字节向量（`adr x0, #0`=`0x10`、`adr x0, #1`=`0x30`、
`adrp x0, #0`=`0x90`、`adrp x0, #4096`=`0xb0`）**逐字节全过**，往返稳定；`cargo test -p
forge-codegen --test spec_tests` 与全工作区门禁（`cargo test --workspace --exclude forge-rustc`）
均 **exit 0**。**未随本提交带夹具用例**：给 `demo_inst12` 加"切片 + unit"夹具时，该夹具的 imm
文本形态与模板不合（`ldw r0, #12` 报 no matching），因此夹具已撤，本条只落"修复 + 采样对齐"，
夹具与 ADR/ADRP 的谱侧落地留待下一步（详见 `crates/tools/forge-tests/asm/README.md` 的勘察记录）。

### Added (2026-10-07) — aarch64：`mov` 立即数的**符号/表达式**形态（8 条）+ 同型指令改用模板（谱减 ~99 行）

**形态**：`mov x0, #:abs_g0_s:sym`、`mov x0, 1b - 0b` 这类写法。**零新增概念**：槽复用 movz/movk/movn 重定位那批已有的 `imm16g0..g3`（按 hw 分组、各带 `imm_fns` 过滤 + `require_symbol`），编码沿用 `MOVXIMM`/`MOVWIMM`（`mov <Rd>, #imm` = `movz hw=0`）的 form/opcode，模板**不带 `#`**（修饰文本/符号文本自带）以与数值形态保持形状隔离。效果：`parsed` 1922 → **1924**、红 1376 → **1369**；**`mov-expr-as-immediate.s` 与 `mov-unsupported-expr-as-immediate.s` 两个文件转绿** ⇒ 全绿集 **123 → 125**（PROVENANCE 127 行）。

**同型指令改用 `[[templates]]` + `rows`**（按仓库既有风格，紧凑且更省体积）：把最近三批**34 条**逐条 `[[instructions]]` 压成 **21 个模板**（`mov` 立即数 1 个；符号/表达式形态按 hw 8 个；`movz`/`movk`/`movn` 的 `abs_g*` 按 (助记符, hw) 12 个），`isa/arm64.toml` 减少约 **99 行**。**展开后的指令名与编码完全等价**——证据就是两处计数钉死值（arm64 **454**、枚举器 **2213**）与文本歧义名单（38 条）在重构前后**一字未变**、codegen 全绿。同时删掉三个一次性生成器（`gen-aarch64-mov-{reloc,imm,imm-sym}.mjs`）：谱里已是紧凑模板形式，留着只会重新吐出冗长版本。

### Added (2026-10-07) — 汇编器：数字局部标签 `1b`/`1f`（GNU as / LLVM MC 语义，ISA 无关）

设计取"通用 + 人体工学、零冗余"：

+ **词法器**把 `数字 + b/f` 吐成一个普通 `Ident`（判据：其后不再接标识符字符或 `.`）——于是下游的**符号引用收集 + 立即数修饰**原样可用（`mov x0, #:abs_g0_s:1b` 里的 `1b` 就是一个普通符号名），**不需要新的操作数种类，也不需要新的槽键**；
+ 两遍布局里数字标签**单独收集**成"编号 → 定义位置列表"（同一编号可反复定义），回填时 `1b` 取**最近的上一个**（允许与分支同址：`1: b 1b`），`1f` 取**下一个**；找不到 → `UndefinedLabel`（准确诊断 ⇒ 语料侧归入"上下文不足"桶而不是缺陷）；
+ 与 NEON 元素后缀**互不干扰**：`v0.16b` 那类以 `.` 开头、由既有的 dot-ident 规则吃掉，不会走到这条分支。

效果：**全绿集 117 → 123（+6，六个都是 riscv64 文件）**——能力坐在共享汇编器里、与 ISA 无关，所以 riscv64 一起受益（"通用"的直接回报）；PROVENANCE 125 行；riscv64 棘轮随之刷新。aarch64 侧那 5 个含局部标签的文件仍被**别的缺口**挡着（`arm64-adr.s` 缺 ADR/ADRP 指令本身；`mov-expr-as-immediate.s` 缺 `mov` 立即数的**符号/表达式**形态）——是下一轮的靶子，本轮的能力是它们的前置件。

### Added (2026-10-07) — aarch64：`mov <Rd>, #imm` 立即数别名（2 条）

`mov x0, #0` / `mov w0, #imm`——上游把它展开成 `movz #imm, lsl #0`（`mov x0, #0` = `0xD2800000`）。与寄存器形态 `mov {dst}, {src}` **形状不同**（带 `#`）⇒ 共存不遮蔽，正是前几轮反复验证过的"同关键字、异形状"路子。生成器入库 `crates/tools/forge-tests/asm/gen-aarch64-mov-imm.mjs`。守卫同步：`spec_coverage_guard` arm64 444 → **446**、`isa_roundtrip_guard` arm64 枚举器 2153 → **2165**；全工作区门禁 exit 0。

### Added (2026-10-07) — aarch64：`movz`/`movk`/`movn` 的 `abs_g*` 类重定位 24 条

`movz x2, #:abs_g0:sym`、`movk x3, #:abs_g1:foo` 这类写法。关键点是 **`hw` 由修饰名决定**（`abs_g0..g3` ⇒ `hw` 0..3），所以四个**按 hw 分组的符号槽**各自用 `imm_fns`（上一轮落的新键）过滤修饰表——过滤表列到 20/20/18/6 个名字（带 `#` 与不带 `#` 两种写法都列）；不这么分就会把 `movz … #:abs_g2:foo` 编成 `hw = 0`、**静默写错字段**。24 条 = 3 助记符 × 4 hw × {X, W}，opcode 照 `[[templates]]` 的 rows 抄（`MOVZX`=0x1A5 / `MOVZW`=0xA5 / `MOVNX`=0x125 / `MOVNW`=0x25 / `MOVKX`=0x1E5 / `MOVKW`=0xE5）；模板**不带 `#`**（修饰文本自带）以与数值形态保持"形状隔离"。生成器入库 `crates/tools/forge-tests/asm/gen-aarch64-mov-reloc.mjs`。

守卫同步：`spec_coverage_guard` arm64 420 → **444**、文本歧义名单 6 → **30** 条（12 组 X/W 同形、靠寄存器类分派，与既有 `LDRX`/`LDRW` 同一模式）、`isa_roundtrip_guard` arm64 枚举器 2009 → **2153**。本批是**纯解析**能力（新变体同 opcode、且 `require_symbol` 把关），既有字节对拍不受影响。

### Fixed (2026-10-07) — `[meta].comment_char` 是单字符 ⇒ A64 的 `/` 会把立即数里的除法截断

`strip_comment` 按**单个字符**切注释，而 A64/GAS 的注释是 `//`：`isa/arm64.toml` 写 `comment_char = "/"` 就使 `movz x0, #(32 / 2)` 从 `/` 处被截成 `movz x0, #(32` ⇒ 报"没有匹配的指令"。上游为此专门写了 `single-slash.s`，所以这条是可复现的语料证据而非猜测。修法 = 注释标记改成 **1–4 个字符的字符串**（`#` / `//` / `;`），生成物的 `strip_comment` 按**整串**匹配，并把 `isa/arm64.toml` 改成 `comment_char = "//"`。校验放宽到 1..=4 字符（原为"必须恰好一个字符"），文档同步。

效果：aarch64 `parsed` 1917 → **1918**、红 1429 → **1428**；`single-slash.s` 整文件转绿 ⇒ 全绿集 **115 → 116**（PROVENANCE 118 行）、淘汰 618 → **617**。

### Fixed (2026-10-07) — aarch64 FP 成对的前/后索引用不了：imm7 槽是无符号的

`stp d8, d9, [sp, #-64]!` 这类写法原先借用了**无符号**的 `imm12fp*` 槽 ⇒ 负偏移直接越界、报"没有匹配的指令"（`stp d10, d11, [sp, #16]` 这类正偏移则正常，所以只有前/后索引的成对形态受影响）。修法 = 镜像 GPR 成对的 `imm7x`/`imm7w`，补 `imm7fp4`/`imm7fp8`/`imm7fp16`（**有符号**、width 7、单位 = 元素尺寸）并替换 18 条 FP 成对的 imm 槽。

效果：aarch64 `parsed` 1909 → **1917**、红 1437 → **1429**；**`seh-optimize.s`、`seh.s` 整文件转绿** ⇒ 全绿集 **113 → 115**（PROVENANCE 117 行）、淘汰 620 → **618**；aarch64 棘轮随之刷新。

### Fixed (2026-10-07) — aarch64 FP unscaled 访存的 op8 用错了族（`LDURD`/`STURD`/`LDURS`/`STURS`）

这四条写的是 `0xFD`/`0xBD`——那是**无符号偏移族**（imm12）；unscaled 族是 `0xFC`/`0xBC`（`ldur d8, [sp, #8]` 上游 = `e8 83 40 fc`）。潜伏至今的原因是 FP 槽用 V 名、语料写 `d8`/`s7`，那批行从来没解析过 ⇒ 字节对拍档看不见它。**影响不止编码**：`LDURD`/`STURD` 带 `callee_load`/`callee_save` 角色、`[spill.FPR]` 也引用它们 ⇒ arm64 后端的 FPR 保存/恢复与 FP 溢出一直在按错族发偏移。修后用 4 条谱内向量钉住（字节取自语料的上游期望值）。

### Added (2026-10-07) — aarch64：FP/NEON 宽度视图 + SIMD&FP 访存全族（60 条）

**宽度必须由寄存器组承载**：FP 访存的助记符都是 `ldr`/`str`/`ldp`/`stp`，宽度只在寄存器名上；若 B/H/S/D/Q 同属一个组（都算 FPR(8)），`ldr s0,…` 与 `ldr d0,…` 的**文本形状完全相同**，分派会提交到先声明的那条（实测 S 被编成 D、Q 被编成 B）。做法：既有 `[reg.fpr8]`（V0..V31）作 **D 视图** + `D0..D31` 别名，另开 `fpr1`/`fpr2`/`fpr4`/`fpr16`（B/H/S/Q）；**`[meta].default_fpr_width = 8` 必须显式写**（否则"主 FPR 类"按最宽组取到 Q，ABI 找 `V0` 报 `UnresolvedReg`）；新增 `[spill.FPR16]`。

指令：单寄存器 B/H/S/D/Q × {无符号偏移, unscaled, 前索引, 后索引} × {ldr, str} + S/D/Q 成对 LDP/STP × {有符号偏移, 前索引, 后索引}（D 视图的无位移形态沿用既有 `LDURD`/`STURD`，避免同形重复）。**本批也抓到我自己的一处编码错**：Q（128 位）单寄存器的 opc 是 **11 载 / 10 存**（不是 D 的 01/00），照 D 写会让 `ldr q9, …` 变成 `ldr B9, …`——被新解析出来的语料行（26 条 FP 形态差异）当场抓出，改完 FP 形态差异归零。

效果：aarch64 `parsed` 1640 → **1909**、红 1706 → **1437**、编码 `checked` 785 → **945**、`known` 53 → **57**（+4 为新解析出的 `sp` 别名写法暴露的既有编码口径差异，非 FP）；全绿集 **112 → 113**（PROVENANCE 115 行）。守卫同步：`spec_coverage_guard` arm64 364 → **420**、`isa_roundtrip_guard` arm64 枚举器 1673 → **2009**。

### Added (2026-10-07) — aarch64：移位立即数 16 条（`add w3, w4, #1024, lsl #12`）

A64 的 add/subtract (immediate) 用 **bit22 = sh** 表示"左移 12 位"，且文本里的立即数**始终是未移位的值**（`#1024, lsl #12` ⇒ 字段 1024，槽的 `unit` 不受影响）。16 条 = 数值形态 8 条（`add`/`sub`/`adds`/`subs` × X/W，槽 `imm12u`）+ **重定位 + 移位** 8 条（槽 `imm12sym`，覆盖 `add x2, x3, #:lo12:sym, lsl #12`）。效果：aarch64 红行 **1734 → 1706**、`parsed` 1618 → **1640**、`corpus_only` +6。守卫同步：`spec_coverage_guard` arm64 348 → **364**、`isa_roundtrip_guard` arm64 枚举器 1577 → **1673**。

一处**语料侧**的 `known` +1（不是我们的回归，已登记进 `asm/README.md`）：`basic-a64-instructions.s:312` 的 `subs xzr, sp, #20, lsl #12` 我们编出 64 位形态（与文本里的 `xzr`/`sp` 一致），而那一行的 CHECK 期望是 32 位形态——语料抽取把 CHECK 行配对错了（该文件另有 `subs wzr, wsp, …`）。

### Fixed (2026-10-07) — 汇编器两遍布局把「第一个冒号之前的一切」当标签（A64 重定位族 382 行一直落红的根因）

生成的 `parse_insts` 按 `name:` 识别标签定义，但判据是"**见到冒号就切**"：`add x8, x8, :lo12:sizes` 因此被切成"标签 `add x8, x8,` + 指令 `lo12:sizes`"，报 `no matching instruction`。语料抽取侧本就有"名字不含空白"的守卫，生成侧没有——**这是 A64 重定位修饰族（382 行、47 个修饰名、十几个文件）从来没解析过的真正原因**，而不是"缺指令"。修法 = 同一口径的判据：名字非空且不含空白/逗号/括号/`#` 才算标签。修后 `add x8, x8, :lo12:sizes` 装配为 `imm: 0`（与上游未重定位字段 = 0 一致），未定义符号报 `UndefinedLabel`（准确诊断 ⇒ 归入"上下文不足"桶而非缺陷）。

### Added (2026-10-07) — ISA-DSL：`[[operand_slots]].require_symbol`（只收符号/带修饰的写法）

重定位修饰形态的符号位置必须开在 `unit == 1` 的槽上（校验硬要求），而立即数槽的候选**特异性按接受域**裁决——unit-1 槽比 unit-8 窄 ⇒ 优先命中，会把**数值写法**接走（实测 `ldr x0, [x1, #8]` 被编成字段 8 而不是 1，静默错编码）。`require_symbol = true` 让该槽在"没记到符号/修饰"时**直接不匹配**，数值写法自然落回数值槽；要求 `kind = "imm"` 且同时声明 `symbols = true`。判据在 `__imm` **内部**判（它成功时会提交并清空 `__SYMREF`，外层测长度增量恒为 0）。四处同步：`model.rs` 字段、`validate.rs` 校验、`schema.rs` + 重新生成的 `isa-dsl.schema.json`、`docs/reference/isa-dsl.md`（键表 + 专节）+ 单测 `require_symbol_validates`（四种情形）。另外：含 `require_symbol` 槽的指令是**仅装配**形态（渲染出的无修饰文本由数值兄弟接手），生成期自测对它们**跳过文本闭环**、只测编解码闭环。

### Added (2026-10-07) — aarch64：重定位修饰族 12 条（`add x0, x0, #:lo12:sym`）

ALU 立即数 8 条（`add`/`sub`/`adds`/`subs` × X/W）+ LDR/STR 4 条，槽是 `imm12sym`（unit 1 + `symbols` + `require_symbol`）。修饰名做成全局数据 `[[conventions.imm_fn]]` **98 条**（49 个名字 × 带 `#`/不带 `#`）——**一个变体收全部修饰**；`text` 里带 `#`、指令模板**不带**，使重定位变体的**文本形状**与数值变体不同（同一形状的多候选里分派会提交到排第一的那条，不按操作数失败回退；形状相同会互相遮蔽，实测 `add W0, W1, #0` 因此装配不出来）。

效果：aarch64 `parsed` 1572 → **1618**、红 1877 → **1734**、`corpus_only` +97、编码 `known` 仍 **53**（无字节回归）；**4 个文件整文件转绿**（`arm64-ilp32.s`、`basic-pic.s`、`jump-table.s`、`tls-add-shift.s`）⇒ 全绿集 **109 → 112**（PROVENANCE 114 行）、淘汰 624 → **621**；aarch64 棘轮随之刷新。守卫同步：`spec_coverage_guard` arm64 336 → **348**、`isa_roundtrip_guard` arm64 枚举器 1505 → **1577**。仍缺（留在红桶）：`#:lo12:sym, lsl #12`、`str q0, …`（FP 视图）、`movz/movk/movn` 的 `abs_g*`、`adrp/adr` 的 `:got:`、`ldr x0, =sym`。

### Added (2026-10-07) — aarch64：字节/半字 GPR 访存 18 条（`ldrb`/`ldrh`/`ldrsb`/`ldrsh`/`ldrsw`/`strb`/`strh`）

9 组助记符/类别 × {带位移, 无位移} = **18 条**（语料 117 行）。编码与既有 LDR/STR 同族：`size 111 V 00 opc imm12 Rn Rt`——`opc` 00 = store / 01 = 无符号载 / 10 = 符号载到 X / 11 = 符号载到 W；imm12 的**单位 = 访问宽度**（byte 1 / halfword 2 / word 4，按单位分成 `imm12b`/`imm12h`/`imm12w` 三个槽）。

**一处真错由字节对拍档当场抓到**：无符号偏移族（imm12）的 `[25:24] = 01` ⇒ op8 是 `0x39`/`0x79`/`0xB9`，我最初按 unscaled 族的 `0x38`/`0x78`/`0xB8` 写（`ldrb w4, [x3]` 我们 `0x…38` ≠ 上游 `0x…39`）；修正后该族 `known` 清零。顺带：`0xB9` 让 `ldrsw` 的无位移形态不再与既有 `LDURSW`（`0xB8`、`mode` 占 bit10 起 2 位）在**解码位 trie** 里重叠——生成期本会把这种结构冲突直接喊出来。谱内向量 5 条（生成器从语料 CHECK 行自动挑具体字节样本：`ldrb`/`ldrsh`/`strb`/`ldrsw`/`strh`），codegen 全过。

效果：aarch64 `parsed` 1547 → **1572**、`no_prefix` 14558 → **14322**、编码 `known` 78 → **53**；新认出的 236 行里 25 条整条通过、211 条**按归因落红**（地址形态/寄存器族尚未做，非回归）。取舍账本（留 109 / 砍 624）与两个棘轮逐字不变。守卫同步：`spec_coverage_guard` arm64 318 → **336**、`isa_roundtrip_guard` arm64 枚举器 1433 → **1505**。

### Added (2026-10-07) — aarch64：扩展寄存器操作数（48 条，300 行那一族）——关键字做成「命名立即数操作数」

`add x8, x8, w5, sxtw #2`、`cmp w0, w1, uxtb` 这类"第三个操作数带扩展关键字、可再带量"的写法此前整族落红。**不新增 DSL 能力**：关键字做成**命名立即数操作数**（`{ext}` → `option` [15:13]），量做成 `imm3` 操作数（[12:10]）——于是每 (助记符, 宽度) 只需 **4 条**（W 扩展 / X 扩展 × 带量 / 省略量），
6 助记符（`add`/`sub`/`adds`/`subs`/`cmp`/`cmn`）× 2 宽度 × 4 = **48 条**（若按"每个文本组合一条指令"要 192 条，靠把关键字与量做成操作数压到 4 条）。
编码与移位寄存器族**共用 `op8`**（0x8B/0x0B/0xAB/0x2B/0xCB/0x4B/0xEB/0x6B），靠 **bit21 = 1** 区分；`cmp`/`cmn` 用无 `rd` 的两条形式 + `fields = { rd = 31 }`。

两张命名表按**操作数类别**分：W 表收全部 8 个（`add w1, w2, w3, uxtx` 也合法——32 位视图下 uxtx 是恒等，这是语料暴露出来的）、X 表只收 `uxtx`/`sxtx`。
**字节证据**：8 个 option 的上游字节在语料里排成一组（`add w1, w2, w3, <ext>` 从 `41 00 23 0b` 逐项递增到 `41 e0 23 0b`，正好是 option 0..7），谱内一次加 **10 条向量**（含 X 形式 `add x1, x2, w3, uxtb` = `41 00 23 8b`）——**首跑全过**。

效果：aarch64 语料红行 **1872 → 1666**、`parsed` 1341 → **1547**，**`arm64-leaf-compact-unwind.s` 整文件转绿**（它最后一条正是 `add x8, x8, w5, sxtw #2`）——全绿集 **109 → 110**（aarch64 语料 30 → **31** 份）、淘汰 624（= `parsed = 0` 557 + 出红 55 + 字节差异 12）、aarch64 棘轮 `parsed` 267 → **356**。守卫同步：`spec_coverage_guard` arm64 270 → **318**、`isa_roundtrip_guard` arm64 枚举器 1049 → **1433**。

### Added (2026-10-07) — aarch64：寄存器偏移寻址（32 条）+ ADD/SUB 移位寄存器带量（8 条）

① **寄存器偏移**（`ldr x9, [x27, x6]`、`ldr w0, [x0, x0, lsl #2]`、`str w14, [x26, w6, uxtw]`、`str w13, [x27, x5, sxtx #2]`）：编码是 `size 111 V 00 opc **1** Rm option S **10** Rn Rt`——与 imm9 家族**共用 `op8 = 0xF8`(X)/`0xB8`(W)**，只靠 **bit21 = 1** 与 **[11:10] = 10** 区分（这两条固定位是 **19 条上游期望字节逐条核对**出来的）。`option`/`S` 与文本关键字的对应同样是**量**出来的：LSL = 3 / UXTW = 2 / SXTW = 6 / SXTX = 7，且 **S=1 ⇔ 文本带 `#N`，N 恒 = log2(访问宽度)**（X 形式 3、W 形式 2）
⇒ 每种组合都是一条**完整的文本模板**，不需要"可选片段"机制。索引寄存器的类由 option 决定（LSL/SXTX 收 X、UXTW/SXTW 收 W；`xzr`/`wzr` 走别名）。语料里出现的 8 种组合全做，× 2 宽度 × ldr/str = **32 条**。

② **ADD/SUB(S) 移位寄存器带量**（`add w12, w13, w14, lsl #12`）：与既有的逻辑族（`ALUSH6`/`ALUSH5`）同格局——`shift` 是逐指令常量（LSL = 0），移位量是**操作数**（X 用 `imm6`、W 用 `imm5`），4 助记符 × 2 宽度 = **8 条**。

③ **字节证据**：谱内一次性加 12 条向量（8 条寄存器偏移 + 4 条 ADD/SUB 带量），字节全部取自语料 CHECK 行的**上游期望值**（`00 68 60 b8` = `ldr w0, [x0, x0]`、`69 6b 26 f8` = `str x9, [x27, x6]`、`ac 31 0e 8b` = `add x12, x13, x14, lsl #12`…）——**首跑全过**。

④ 效果：aarch64 语料红行 **1945 → 1872**、`parsed` 1268 → **1341**；与第二十七批末（aarch64 红 2122）相比 `arm64-leaf-compact-unwind.s` 16 → **1**（只差最后一条）、`arm64-memory.s` 121 → 100、`arm64-arithmetic-encoding.s` 122 → 110、`seh-packed-unwind.s` 111 → 48。本轮**没有文件跨过"整文件无红"的门槛**（近门的几个都在 1–4 条），所以取舍账本（109 / 625）与两个棘轮**逐字不变**。守卫同步：`spec_coverage_guard` arm64 230 → **270**、`isa_roundtrip_guard` arm64 枚举器 937 → **1049**。

### Added (2026-10-07) — aarch64：寄存器惯用名 `fp`/`ip0`/`ip1`；16 条前后索引（writeback）形态

① **别名表漏了 `FP`**：真实序言/尾声写的是惯用名而不是编号（`mov x29, fp`、`stp x29, lr, [sp, #-16]!`），而我们 `[reg.gpr8].aliases` 只有 `XZR`/`LR`——**注释里写着 FP、表里没写**，一行之差就让 `cfi.s`/`seh-*.s` 这些整文件落红。补 `FP = 29`，顺带 `IP0 = 16`/`IP1 = 17`（链接器 scratch 的惯用名）。

② **前后索引（writeback）16 条**：单寄存器 4 宽 × 前/后索引（`ldr x0, [x1, #16]!` / `ldr x0, [x1], #16`）——与 unscaled 家族共用 `op8 = 0xF8`(X)/`0xB8`(W)，靠新位域 `mode`（[11:10]：`11` 前索引 / `01` 后索引）区分；pair 4 宽 × 前/后索引——[25:23]：`011` 前 / `001` 后 ⇒ `op8 = 0xA9`/`0xA8`（W 形式 `0x29`/`0x28`）、`idx2 = 2`/`3`。imm 一律真字节且有符号（pair 的 imm7 三种模式下都按元素尺寸缩放，沿用上一批的 `imm7x`/`imm7w`）。

③ **字节证据**：仓库自己的 A64 编码基准 [`docs/reference/aarch64-encoding-ref.md`](docs/reference/aarch64-encoding-ref.md) 第 121/124 行给出权威值——`ldr x0,[x1],#16` = `F8410420`、`ldr x0,[x1,#16]!` = `F8410C20`、`stp x29,x30,[sp,#-16]!` = `A9BF7BFD`、`ldp x29,x30,[sp],#16` = `A8C17BFD`，外加无位移 pair 的 `A9000440`/`A9400440`。6 条谱内向量**一次通过**，说明新形态的编码与模式位都对得上。

④ 效果：aarch64 语料红行 **2122 → 1945**、红文件 76 → **72**、`parsed` 1091 → 1268；**4 个文件整文件转绿**（`cfi.s`、`seh-multi-epilog.s`、`seh-large-func.s`、`seh-large-func-multi-epilog.s`——它们此前只差这条序言/尾声写法），全绿集 104 → **109**（aarch64 26 → **30** 份）、aarch64 棘轮 `parsed` 197 → **267**；`seh-packed-unwind.s` 111 → 48、`basic-a64-instructions.s` 482 → 442。守卫同步：`spec_coverage_guard` arm64 214 → **230**、`isa_roundtrip_guard` arm64 枚举器 841 → **937**。

### Fixed (2026-10-07) — aarch64 访存立即数的「单位」：文本是字节、字段按访问宽度缩放（+ 8 条无位移形态）

A64 的 `LDR`/`STR`/`LDP`/`STP` 立即数在**文本里是字节**、在**字段里是"字节 ÷ 访问尺寸"**：`ldr x2, [sp, #32]` ⇒ `imm12 = 4`、`ldr w5, [x4, #20]` ⇒ `imm12 = 5`、`stp x29, x30, [sp, #-16]` ⇒ `imm7 = -2`（这些值直接来自语料 CHECK 行里的上游期望字节）。我们谱里 `imm12u`/`imm7u` 一直按**字段单位**收文本（`ldr x0, [x1, #1]` ⇒ 字段 1 = 8 字节），于是真实语料里**每一条带非零字节偏移**的访存都会编出错字节——只是 vendored 集里一个这样的文件都没有，字节对拍档一直看不见它。

**修法**：按访问宽度分槽（`imm12x` `unit = 8` / `imm12w` `unit = 4`、`imm7x` `unit = 8` / `imm7w` `unit = 4`；pair 的 `imm7` 同时改成**有符号**，`stp …, #-16` 才收得下），而 `imm12u` **留在原地**——ALU/CMP/CMN 的立即数**不缩放**（`add x0, x1, #5` 就是 5），同一段位的两种解释，所以分成两个槽（"单位"不是全局约定，而是每条指令的语义）。

**顺带补 8 条无位移形态**（`ldr x4, [x3]`、`str w9, [x8]`、`stp x0, x1, [x2]`…）：编码上就是"位移 = 0"那一格，与 `DCPS1B`（裸 `dcps1` = `dcps1 #0`）同款——声明序放在带位移的指令**之后**，解码仍走带位移那条（渲染出 `, #0`），新形态只承担"省略写法能装配"。

**证据**：语料 `arm64-memory.s` 的 `; CHECK: … ; encoding:` 行给出上游字节，谱内新增 6 条向量（`ldr x4, [x3]` = `64 00 40 f9`、`str x2, [sp, #32]` = `e2 13 00 f9`、`ldr w5, [x4, #20]` = `85 14 40 b9`…）；该文件里我们认得的 23 条**逐字节全等**（`known` 0），且改动前后**没有新增任何访存类字节差异**。原有 7 条访问向量只是**文本**从字段单位换成字节（`ldr x0, [x1, #1]` → `#8`、`stp x29, x30, [sp, #2]` → `#16`），**字节一字未动**——正说明改的是"解释"而不是"编码"。

效果：aarch64 语料红行 **2183 → 2122**、`parsed` 1030 → 1091（`arm64-leaf-compact-unwind.s` 34 → 16、`seh-large-func-multi-epilog.s` 37 → 8、`basic-a64-instructions.s` 494 → 482）；vendored 集与两个棘轮**逐字不变**（新增能力只影响此前落红的文件，取舍账本没动）。守卫同步：`spec_coverage_guard` arm64 206 → **214**、`isa_roundtrip_guard` arm64 枚举器 825 → **841**。

### Added (2026-10-07) — riscv64 浮点 `rm` 成为操作数；省略写法的缺省值按**字节证据**定（算术 `dyn`、转换 `rne`）

① **`rm` 是操作数**：F/D 的 `funct3` 那 3 位就是舍入模式，谱里原先把它写死（每条指令一个固定值，或干脆没有 rm），于是真实语料里**带 rm 的写法**（`fadd.s f1, f2, f3, dyn`、`fcvt.w.s a0, fs5, rne`）一条都装配不了。现在用既有机制表达：`[conventions.imm_names.rm]`（`rne`/`rtz`/`rdn`/`rup`/`rmm`/`dyn`——就是上一批那张**命名立即数表**的第二张消费者）+ 两条新 form（`R_RM` 三操作数、`R_RM2` 两操作数：`funct3` 当操作数位域），浮点算术 5 族与 `fcvt` 全 18 条各加"显式 rm"形态。

② **省略 rm 的缺省值不靠猜**：真实语料两种写法都写，所以每族另有"省略 rm"的并列声明。缺省值是从**语料自带的上游期望字节里量出来的**：**算术族（`fadd`/`fsub`/`fmul`/`fdiv`/`fsqrt`）省略 rm = `dyn`(7)**，**转换族（`fcvt.*`）省略 rm = `rne`(0)**——11 条独立样本，横跨 `rv32d-valid.s`、`rv32zdinx-valid.s`、`rv32zfhmin-valid.s`、`rv64zhinxmin-valid.s`（两个上游文件对同一件事的**反汇编**写法还给过相反的暗示，最终以**字节**为准）。我们原先两侧都写死 `0` ⇒ 算术族是**字节错**，现在两侧都由上游字节守住。

③ **顺带修掉 `FCVT.S.D` 的 `rs2`**：规范是 funct7 `0100000` + **rs2 = 1**，我们原来与 `fcvt.d.s` 同形（rs2 = 0）——字节对拍直接抓到（`fcvt.s.d fs5, fs6, dyn` 少一位）。同时删掉 6 条固定 `rtz` 的旧 `FCVT_*_RTZ` 与 2 条把省略写法写死 `rne` 的声明（已被 rm 操作数覆盖），lowering 改成显式传 `1`（= rtz）：lowering 模板要过"操作数签名"校验，**裸名字不在它的分类里**，写数字才过。

④ **效果**：`rv32f-valid.s` / `rv32d-valid.s` / `rv64d-valid.s` / `rv64f-valid.s` / `fp-default-rounding-mode.s` 与两个 aliases 文件共 **7 个进全绿层**（全绿集 97 → **104**；riscv64 语料 68 → **75** 份、`parsed` 1616 → **1741**）；riscv64 红行 465 → **446**、红文件 58 → 55；编码对拍 `cases` 1139 → **1251**、`checked` 1021 → **1111**、**`known` 仍 0**——这几个 FP 文件的每条上游期望字节**逐字节相等**（`rv32d-valid.s` 49 条期望 / 38 条对拍上 / 0 差异），`rm` 的两套缺省值因此有了直接的字节证据。
   附带一处**归因迁移**（不是回归）：Zfinx/Zdinx 那些"浮点算在整数寄存器上"的写法（`fadd.s x26, x27, x28`）原先落 `no_prefix`（连助记符都不认识），补上 FP 形态后落进红桶（认识助记符了、操作数对不上）——`no_prefix` 182 → 224、红行净减 19。

⑤ **工具修正**：`fetch.mjs` 的打分步骤加 `--no-fail-fast`。cargo 默认在**第一个失败的测试目标**就停，而两个档的棘轮在换谱/换语料时几乎必然同时不一致 ⇒ 排在后面的 `asm_parse` 根本不跑，解析档记分板停在上一轮内容上，于是"这一轮才下载、还没打过分"的候选被记成 `not-scored` **砍掉**（判据被静默绕过）。2026-10-07 实测踩到（364 个候选被误记、淘汰账错位），已修。

### Added (2026-10-07) — 命名立即数表（`[conventions.imm_names.<表名>]` + 槽 `names`）；riscv64 命名 CSR 全量落地

① **新能力：一个名字 = 一个值**。与既有的 `kind = "bits"`（名字**拼接**、编码取按位或，`fence iorw`）相反，真实 ISA 里另一类是**单名单值**：RISC-V 的 CSR 名（`mstatus` = 0x300）、浮点舍入模式（`rtz` = 1、`dyn` = 7）。谱里写一张表、槽上写 `names = "<表名>"` 即可：

```toml
[conventions.imm_names.csr]
mstatus = 0x300
mvendorid = 0xF11

[[operand_slots]]
name = "csr12"
kind = "imm"
signed = false
width = 12
names = "csr"
```

运行时**双向**：解析**既认名字也认字面量**（真实语料两种都写：`csrrs t1, mvendorid, zero` 与 `csrrs t2, 0xF11, zero` 是同一批测试里的相邻两行），名字按大小写不敏感匹配（与寄存器名同口径）、没命中就落回原来的字面量/表达式路径；渲染时**值在表里就写名字**，所以 `disassemble → assemble` 照旧闭合，**同值多名取字典序最小者**（上游有 `stval`/`sbadaddr`、`satp`/`sptbr`、`mtval`/`mbadaddr`、`dscratch`/`dscratch0` 这类别名对）。校验期 fail-closed：表必须已声明且非空、名字必须是 ident、**每个值要落在槽的接受值域内**；`names` 只对 `kind = "imm"` 有意义，变长（`prefix_scan`）ISA 写它直接拒。表是**数据**——DSL 不认识任何 CSR 名/舍入模式名。

② **riscv64：CSR 名表 432 条**，从上游 `llvm/lib/Target/RISCV/RISCVSystemOperands.td`（ref `llvmorg-19.1.0`）机械生成（含 `foreach` 区间 `hpmcounter3..31`/`pmpaddr0..63` 与 `AltName`/`DeprecatedName` 别名），并**用语料自带的期望字节逐条对账**：语料里出现的 429 个名字全部命中、值**零不符**。此前 `csr12` 槽只收数字，于是 `machine-csr-names.s`、`user-`/`supervisor-`/`hypervisor-csr-names.s` 与其 rv32 变体**整文件**落红桶。

③ 效果（`fetch.mjs` 重跑取舍 + 刷棘轮）：riscv64 **红行 1056 → 465**、红文件 72 → 58；**13 个文件自动进全绿层**（全绿集 84 → **97**、PROVENANCE 98 份 LLVM + 1 份手工；riscv64 语料 55 → **68** 份）。编码对拍档同时大涨，且**逐字节全等**：riscv64 `cases` 339 → **1139**、`checked` 221 → **1021**、`known` 仍 **0**（新进来的 CSR 文件自带 ~800 条上游期望字节，全部对上——这是 432 条表值正确性的直接证据）。生成物自测（`forge-codegen --lib` 1863 条）与三方针守卫全绿。

> 新进集的文件里有 `machine-csr-names-invalid.s`（上游"故意写错"的文件）：它那些行在 rv32 目标上被 LLVM 拒（"requires an option to be enabled"），但在 **riscv64** 目标上名字 → 12 位编码本身完全合法（上游 `rv64-machine-csr-names.s` 的注释也这么说），不是假绿。

### Added (2026-10-07) — riscv64 浮点寄存器 ABI 别名（`ft0`/`fa0`/`fs0`…）；逐文件记分板带上红桶原文

① **纯谱数据修掉一整类红桶**：`[reg.gpr8.aliases]`（`a0`/`s0`/`fp`…）早就有，`[reg.fpr4]` 却只有 `F0`…`F31` 数字名——而**真实语料里浮点操作数几乎全写 ABI 名**（实测全集 `ft*`/`fa*`/`fs*` 出现 **1840** 次，数字名只有 **47** 次），于是整批 FP 用例文件（`rv32f-valid.s`、`rv32d-valid.s`、`numeric-reg-names-*.s`、`fp-*-default-rounding-mode.s`…）**整文件**落进红桶。补上 psABI 那套映射（0–7 = `ft0`–`ft7`、8–9 = `fs0`–`fs1`、10–17 = `fa0`–`fa7`、18–27 = `fs2`–`fs11`、28–31 = `ft8`–`ft11`），口径与 GPR 别名**完全一致**：**解析认、渲染仍出规范名**（`F`），不新增任何 DSL 键、不动生成器。

② 实测（`asm/fetch.mjs` 重跑取舍 + 刷棘轮）：riscv64 红行 **1240 → 1056**、红文件 76 → 72；`numeric-reg-names-f.s` / `numeric-reg-names-d.s` **整文件解析通过**并自动进全绿层（全绿集 83 → **84** 份、PROVENANCE 85 份 LLVM + 1 份手工；riscv64 语料 53 → **55** 份、`parsed` 526 → **654**）；同族文件红行同步下降（`rv32f-valid.s` 26 → 16、`rv32d-valid.s` 27 → 18、`fp-default-rounding-mode.s` 16 → 8、`rvf-aliases-valid.s` 10 → 5）。三架构**编码对拍 `known` 仍全 0**（棘轮只动 riscv64 解析档），x86/aarch64 计数逐字不变。

③ **工具：逐文件记分板带红桶原文**（`target/asm-suite/<isa>.json` 的 `file_rows[].samples`）：每条是 `文件:行号 ⇥ 原文 ⇥ 错误`（每文件最多 8 条），此前只进棘轮的总样例（每套 12 条）——按文件归因时反面看不到"错在哪一行"。JSON 里的控制字符（制表符）现在**转义**（`esc`），否则消费方解析整份记分板会失败；`fetch.mjs` 不读该字段，**取舍判据不变**。

### Added (2026-10-06) — 真实汇编语料从"三份小样"扩到**上游全集里全绿的那一层**（取舍自动化 + 出处守卫）

① **取舍不再手抄**：新工具 `crates/tools/forge-tests/asm/fetch.mjs`（Node 自带根证书，替掉需要系统 TLS 的 `fetch.ps1`）一条命令做三件事——按钉死的 ref（`llvmorg-19.1.0`）拉上游 LLVM MC 三套目录的**顶层** `.s`（x86 只取 Intel 语法的文件名那批），跑解析档 + 编码对拍档拿**逐文件**记分板，再按「**整条解析得住（`parsed >= 1`）+ 不产生红桶（`tail_mismatch` ≤ 已登记缺口）+ **编码对拍没有字节差异**」淘汰，最后**重写** `asm/PROVENANCE.md`（逐文件 字节/行/sha256）。两个档因此新增机读产物：`target/asm-suite/<isa>.json` 的 `file_rows` 与 `encoding-files.json`（逐文件记分板），淘汰清单落 `target/asm-dropped.json`——判据在脚本里，不在人脑里：谱长本事后重跑即自动补回。

② **评了 733 个候选、收 83 个**（riscv64 339 / aarch64 337 / x86 57 顶层；淘汰 651 = 576「本谱一个候选都不认识」+ 66「出红」+ 9「有字节差异」）。成果：`parsed` riscv64 136 → **526**、aarch64 153 → **197**、x86 llvm-mc 111 → **116**；编码对拍 riscv64 134 → **221**、aarch64 119 → **125**、x86 110 不变，**三架构 `known` 仍然全 0**（每一条对拍上的期望字节都逐字节相等）；红桶只有 riscv64 那 1 条已登记的 `jal a0, a0`。淘汰面（SVE/SME/AVX-512/AMX/RVV/RVC 压缩编码/带符号 CSR 名/重定位表达式/移位与扩展寄存器操作数…）逐类记在 `asm/README.md`。

③ **语料抓出一处真缺陷并修掉**：riscv64 `clz`/`ctz`/`cpop`/`rev8` 原先写成 **R 型**（`opcode = 0x33` + `funct7`），实际是 **OP-IMM**（`opcode = 0x13` + 立即数 `0x600`/`0x601`/`0x602`/`0x6b8`）——原编码与 `rol` 撞车（同 `funct3=1`/`funct7=0x30`，只差 `rs2`），**我们自己的解码器把 `clz X5, X6` 的字节解成 `rol X5, X6, X0`**，即这三条编出来解不回自己。上游 `rv64zbb-valid.s` 的期望字节 `0x93,0x52,0x83,0x6b` 一比就露（`isa/riscv64.toml` 换 `form = "I"` + 固定 `imm12`，谱内 4 条向量同步改）；修完 `rv64zbb-valid.s`/`rv64zbkb-valid.s` 由"有字节差异"转为**逐字节全等**并进了语料集。

④ **新守卫 `crates/tools/forge-tests/tests/asm_provenance.rs`**：`asm/PROVENANCE.md`（生成物）与磁盘语料**双向一致**——磁盘有表里没有 / 表里有磁盘没有 / 字节数或行数对不上，三者都红（sha256 由 `fetch.mjs` 拉取时校验，守卫只比字节数与行数以免引入哈希依赖）。`gnu-gas-intel` 那份节选同时**钉到 commit** `19cc5b7efb3e…`（按内容寻址，任一镜像同 sha），并核出它其实是上游**前 64 行逐字未改**（原先 PROVENANCE 里"空白已归一化"的说法不成立，已删）。

### Added (2026-10-05) — x86 `movzx`/`movsx` 的内存源形态（最后一个已登记的汇编器缺口）

`movzx eax, byte ptr [rbx]` 的**源宽度只能由尺寸关键字给**（`0F B6` 与 `0F B7` 的差别在操作码，没有任何寄存器能表达源宽），而 `{size}` 按设计只是"给人读的提示"、不进类型签名 ⇒ 原来那对 `MOVZX_R{8,16}_MEM`（`[src]` 简写、asm 里**没有关键字**）文本一模一样，只能挂在歧义名单上；再照搬 16/32 位目的地的变体只会让 `movzx eax, word ptr [rbx]` 静默编成 byte 那条。

**修法用既有机制**：把尺寸关键字写成**模板字面量**（`asm = "movzx {dst}, byte ptr {mem}"`，与 `NOT_MEM32` 一族同款）——两条写法文本天然区分、候选各自命中，**渲染也带关键字** ⇒ `disassemble → assemble` 闭合。补 4 条完整内存模板形态（`MOVZX/MOVSX_MEM{8,16}`；目的宽度仍由寄存器驱动：16 位目的发 66、64 位目的发 REX.W）。

顺带修掉简写那对的两处问题：① `MOVZX_R16_MEM` 把 `66` **写死**（那是"16 位目的"的编码），配 64 位目的会编出 `66 48 0F B7`——现在宽度一律由目的寄存器驱动（`48 0F B7`）；② 基址槽收窄成单一 64 位地址类（多类槽会被**生成期采样器**强推 2/4 字节**地址**，是无意义的组合——地址宽度不是数据宽度）。

效果：`spec_coverage_guard` 的文本歧义名单**少了 `MOVZX_R8_MEM`/`MOVZX_R16_MEM` 两条**（文本不再撞车 ⇒ 恢复强断言）；落地 4 条谱内向量；文档补「源宽度只有尺寸关键字能表达时，把关键字写成模板字面量」。

### Added (2026-10-05) — 字节对拍三架构全清：长编码拆行纳入对拍 + 十六进制立即数是位模式

① **上游把长编码写成多条只有字节的注释**（`acquire lock add …` 是 `[0xf2]` + `[0xf0,0x48,0x01,0x00]`；`pushf`/`popf` 是"两条 CHECK 后面跟两条指令"），抽取器原先按"run ≥ 2 就整段丢掉"保守处理。现在按**声明的侧**绑定、且**只在块内相邻**：注释在指令**后** ⇒ 按序**拼接**成一份期望；注释在指令**前** ⇒ 按序 **1:1** 配给紧随其后的同样多条指令；绑不上就整块丢掉、后续不错位（三条单测钉住；`intel-syntax-encoding.s` 补上逐段声明）。

② **十六进制立即量是位模式**：词法器原先按 `i64::from_str_radix` 读，64 位槽写不出 `#0xfffffffffffffff0`（= -16）——aarch64 的 `and sp, x5, #0xfffffffffffffff0` 报 "bad hex immediate"。改成按 `u64` 读、再按位重解释（二进制同理；**十进制仍是值**，超 i64 报错）。

效果：编码对拍 x86 `cases` 104 → **110**、`dropped` 8 → **0**；aarch64 `unparsed` 1 → **0**、`checked` → **119** —— **三架构都是 `cases = checked`、`known` 全 0**（语料里每一条期望字节都逐字节对上）。

### Added (2026-10-05) — x86 `prefix` 收列表（`acquire/release lock`）；x86 语料全部归因完毕

`acquire lock add [mem], r` = `F2` + `F0`、`release lock add …` = `F3` + `F0`：x86 的锁 + 内存序提示是**两条前缀**，而 `EncKeys.prefix` 只收一个字节。现在 `prefix` 收**列表**（`prefix = ["0xF2", "0xF0"]`）：编码按书写序发（66 → 前缀 → REX → opcode），解码逐个按**前缀扫描标志**判（顺序无关）；校验期拒掉列表里的 `"opsize"`（混进列表会让"发不发"含糊）与第二个 `"field"`。

改这里踩到一个**必须保留的旧语义**：单条前缀值为 `0` 时生成的"**没有前缀**"断言（`!__p66 && !__pF0 && !__pF2 && !__pF3`）不能省——SSE 的 `66` 变体（`ADDPD`）与无前缀变体（`ADDPS`）同 opcode，少了它后者会先命中前者的字节（全量套件实测 `spec_addpd` 解成 `Addps`）。

效果：x86 `llvm-mc` 解析档 `parsed` 109 → **111**、`no_prefix` 2 → **0** —— 该档 447 行（111 parsed + 336 corpus_only）**全部归因完毕**；两条锁前缀的字节人工核对 `f2 f0 48 01 00` / `f3 f0 48 01 00` 与上游一致（它们不在编码档的 104 条里：上游把长编码拆成两条 `CHECK: encoding:` 注释，抽取器按"run ≥ 2 就整段丢掉"保守处理——已记入 `asm/README.md`）。

### Added (2026-10-05) — x86 助记符条件后缀（`sete`/`cmovl`）→ 语料带期望字节的用例全部逐字节对上

上游写 `sete r16b` / `cmovl eax, r16d`（助记符**自带条件**、没有 `cc` 操作数），我们只有通用形态（`setcc {dst}, {cc}` / `cmovcc {dst}, {src}, {cc}`，供 lowering 用）。两条"做成指令"的路都被**解码树**挡住（原因留在 `isa/x86.toml` 注释里）：`{cc}` 嵌进助记符要扫描器让"紧贴操作数的字面段"按 ident 前缀匹配（`sete` 是一个 token）；按条件码建**精确** opcode（`0F 94`）会与通用形态的**掩码**边（`0F 9x`）重叠——那是**真**歧义（同一批字节两种 `Inst` 形状），`check_dec_trie_overlaps` 直接拒。

**解法是既有机制**：`[[pseudo]]` 文本展开（分派键是**整词** `sete`/`cmovl`，不产生解码 arm；SSE 的八个比较别名就是先例）。配套补三条**同掩码**的兄弟指令（`SETCC_RM8_B` 8 位名、`SETCC_R_MEM` 内存、`CMOVCC_R_MEM`；同掩码 ⇒ 归到同一解码节点按声明序试，靠 mod 守卫区分），并把 `CMOVCC_R_RM` 从固定 64 位放宽到 16/32/64。

顺带修掉同族一处**恒假守卫**：`opsize = 8`（`setcc byte ptr`）生成 `__opsize == 1` 的解码条件，而 `__opsize` 只可能取 2/4/8 ⇒ arm 永远解不出来（8 位没有 opsize 前缀可查——与 v20 V10 加 8 位 ALU 族时实测过的那条同源，这次补在 `Opsize::Reg` 分支上）。

效果：x86 `llvm-mc` `parsed` 95 → **109**、`no_prefix` 16 → **2**（只剩 `acquire/release lock`）；编码对拍 **checked 104 / unparsed 0 / known 0**——**语料里带期望字节的用例全部逐字节对上**。落地 3 条新谱内向量（`sete r16b`、`sete byte ptr [r16 + r17]`、`cmovl r17d, r16d`）。

### Added (2026-10-05) — x86 补 APX 语料用到的真实形态；`llvm-mc` 解析档红桶清零

承接 APX/REX2：把 `apx-rex2-format-intel.s` 里**谱里根本没有的形态**补上（14 条指令），x86 `llvm-mc` 的 `tail_mismatch` 从 39 降到 **0**：

+ **一元族的内存形态** 8 条（`NOT/NEG/INC/DEC/MUL/IMUL/DIV/IDIV_MEM32`，`FF /0./1`、`F7 /2../7`）。宽度没有寄存器可驱动，所以模板里**写死字面 `dword ptr`**——`not byte ptr [rax]` 不会命中（真实汇编器同样要求一元内存形态给尺寸提示），不会静默按 32 位编出来。
+ 一元 **`MUL_RM`/`IMUL_RM`**（`F7 /4`、`F7 /5`）；拓宽 `NOT_RM`/`DIV_RM`/`IDIV_RM` 到 16/32/64 位（原来固定 64）。
+ 两操作数 **`IMUL_R_MEM`**（`0F AF` + 内存源）、**`NOP_RM`/`NOP_MEM32`**（`0F 1F /0`，Intel 的多字节填充形态）。
+ **`MOVSXD_R_RM32`**：真实汇编写 32 位源名（`movsxd rax, r16d`），与 lowering 用的 64 位源那条**同编码**（`MRR_FIX64`）——两条同编码指令是既有格局（`MOV_R_RM`/`MOV_RM_R`）。

顺带修掉一处**解码树漏洞**：固定扩展码（`F7 /2` 的 2）的守卫只加在寄存器分支、**内存分支漏了** ⇒ `neg dword ptr [rax]` 会解成**声明在前**的 `NotMem32`。一元内存形态是第一批"固定 ext + 内存"指令，所以现在才暴露。

效果：x86 `llvm-mc` `parsed` 36 → **95**、红桶 55 → **0**、`no_prefix` 20 → 16；字节对拍 `checked` 31 → **90 条逐字节全等**、`known` 仍 **0**。余下 16 条 `no_prefix` 已逐条登记（14 条助记符条件后缀 `cmovl`/`sete`、2 条 `acquire/release lock`），各自的实现路线写在 `asm/README.md` 与 `isa/x86.toml` 注释里。

### Added (2026-10-05) — x86 APX：REX2 前缀与 EGPR（`r16`..`r31`）编码/解码；16/32 位 `mov` 的 MR 形态

**能力由谱数据声明，不新增开关**：x86 的四个 GPR 宽度组扩到 32 项（`r16`..`r31` 四个视图 = APX 的 EGPR），编码器看到任一 GPR 字段的索引 ≥ 16 就改发 **REX2**（`0xD5` + payload `M0 R4 X4 B4 W R3 X3 B3`），第 4 位进 payload、`M0` **取代 `0F` 字节**；解码侧在 `M0=1` 时先补一个虚拟 `0F` 再走原有派发树（消费长度减一），因此派发树不用写第二份。`[[conventions.prefix_scan]]` 的效果从 `Vec<String>` 改成**枚举** `PrefixEffect`（`addr32`/`addr16` 合并成有值变体 `AddrSize(32|16)`），编码器/校验器/schema 不再各处比字符串。

新增键 `[reg.<组>].alloc_count`（**分配池 ≠ 寄存器文件**）：`[reg.gpr8]` 现在 `names` 32 项 + `alloc_count = 16`——EGPR **能编码**（`mov r16d, eax` 这类写法成立），但**不进分配池**。这是安全阀：分配器一用 r16+，JIT 产物在没有 APX 的机器上就是**非法指令**（实测 `STATUS_ILLEGAL_INSTRUCTION`，全量套件当场抓到）。

两条 fail-closed：VEX/EVEX 与 0F38/0F3A 表达不了 EGPR（本实现里 GPR 字段只有 4 位）——寄存器槽装得下 EGPR 的谱在**生成期**报错，内存地址是 EGPR 的在**编码期**报错；高编号寄存器视图按同一能力封顶（取到 15）。

顺带补上 x86 **16/32 位 `mov` 的 MR 形态**（`89`，`MOV_RM_R_24`）：上游 LLVM 对 reg-reg 用 MR，我们原先只给 64 位建了它 ⇒ 那类上游编码我们**解不回来**。字节变化（16/32 位 reg-reg `mov` 由 `8B` 变 `89`，与上游一致）属破坏性更新：谱内向量与相关测试已同步。

效果（语料 `apx-rex2-format-intel.s`）：x86 llvm-mc 档 `parsed` 36 → **52**、红桶 55 → **39**；编码对拍 `checked` 31 → **47**、`known` 保持 **0**（16 条 APX 行与上游 CHECK 逐字节一致）。落地 5 条 APX 谱内向量。

### Fixed (2026-10-05) — 谱统一落在严格 TOML 1.0 子集（`isa/riscv64.toml` 的多行内联表）

`isa/riscv64.toml` 的寄存器别名原先写成跨 5 行的内联表（`aliases = { zero = 0, … ,` … `t6 = 31 }`）——那是 **TOML 1.1** 才允许的写法（多行内联表 + 尾逗号）：本仓库的 `toml` 依赖是 spec 1.1 实现（`toml-1.1.x+spec-1.1.0`），所以构建全绿；而编辑器、Python `tomllib` 等 1.0 解析器会报语法错。改成标准子表（写法等价）：

```toml
[reg.gpr8.aliases]
zero = 0
```

生成物与语料计数**零变化**，并补一条窄守卫 `crates/frontend/forge-isa-dsl/tests/schema_guard.rs::shipped_specs_are_strict_toml_1_0`（只钉"内联表内不许有裸换行"这一条，扫描器口径另有自测）与规范说明（`docs/reference/isa-dsl.md`「谱的书写约束：严格 TOML 1.0 子集」）。

### Added (2026-10-04) — `[[operand_slots]].symbols`：立即数位置上的符号引用（`%hi(foo)`）

真实汇编里立即数常常是**符号**：`lui a0, %hi(foo)`、`addi ra, sp, %lo(foo)`、
`auipc a0, %pcrel_hi(foo)`、`jal rd, .Lpcrel_hi0`。此前这类行一律报"没有这条指令"——**诊断是错的**
（那是合法写法，只是符号要等重定位）。

新增槽属性 `symbols = true`（只对 `imm`/`label` 有意义，且要求 `unit == 1`）：

+ **解析**：未定义的 ident（`.equ` 常量优先）被记成**符号引用 + 当前立即数修饰**，当 0 参与算术；
  整条命中后随 `__lsyms` 一起提交。未声明 `symbols` 的槽照旧"未知 ident 即不匹配"——**能力是
  声明的，不是隐含的**；`.equ`/`.set` 的表达式也不受影响（那里不武装）。
+ **回填**（两遍布局的第二遍）：符号解析成"该标签的**块下标**"（`.` = 本条指令自身的下标），
  再**过一遍修饰**（复用 `[[conventions.imm_fn]]` 生成的 `__imm_fn<i>`）——`%hi(foo)` 就是
  `hi(foo 的块下标)`（= 3，与 `foo` 在 3 号指令处一致）；**未定义**的符号报 `UndefinedLabel`，
  这正是想要的准确诊断。
+ riscv64 落地：`imm12`/`imm20` 打开 `symbols`；补 `%pcrel_hi`/`%pcrel_lo` 两个修饰（值与
  `%hi`/`%lo` 同义——重定位**种类**是 linker/patcher 的事，本仓库记在 `[[reloc]]`/`reloc` 上）。
+ 顺带修掉一处**词法缺陷**：`.` 开头的局部标签名（`.Lp`、`.Lpcrel_hi0`——真实语料里遍地都是）
  原先被切成 `Dot` + `Ident`，既当不了标签也解析不了。`__lex` 现在只在 `.` 后面**不跟字母**时
  才当位置符号（`jal zero, .` 照旧）。

效果：riscv64 语料 `parsed` 135 → **136**、红桶 7 → **1**（只剩 `jal a0, a0`——上游把第二操作数当
**未定义符号 a0**（`jal rd, symbol` 只收符号），我们拒了"寄存器样子的 ident"当符号以免 `jmp rax`
这类走错候选，已登记为设计取舍），`corpus_only` 361 → **366**（5 条未定义符号行归因到"符号在别处"，
不再误报成"缺指令"）。x86/aarch64 计数不变（词法改动无回归）。

### Added (2026-10-04) — `kind = "bits"`：命名位集合操作数（`fence pred, succ`）+ riscv 收尾三条写法

**命名位集合**：有些操作数在文本里是**若干名字的拼接**——RISC-V `fence` 的 pred/succ 就是
`i`/`o`/`r`/`w` 的组合（`fence iorw, iorw`、`fence w, ir`、`fence r,w`）。表是**数据**：

```toml
[conventions.bitsets.fence]
i = 8
o = 4
r = 2
w = 1

[[operand_slots]]
name = "fence_set"
kind = "bits"      # 新 kind
table = "fence"
width = 4
```

+ 解析：取一个 ident，按表里的名字**贪心最长匹配**逐段吃掉，编码值 = 各位的**按位或**；整串必须
  吃干净（剩余字符不是名字 ⇒ 这条写法不匹配，不静默当空集）；表在**解析期**摊平进槽
  （`resolve_bitset_tables`，`#[serde(skip)]` 字段），生成器不再回头查表；
+ 渲染：按名字**字典序**把置位的名字拼起来（确定性，与 `[conventions.cond]` 的"同码取字母序
  最小名"同一口径）；
+ 校验：表必须已声明且非空、`width` 必填、位必须落在槽宽内、名字必须是 ident 片段；变长
  （`prefix_scan`）ISA 写它 → 校验期 fail-closed。三方针同步：`schema.rs` 新增
  `[conventions.bitsets.<table>]` 段与 `[[operand_slots]].table` 键、`docs/reference/isa-dsl.md`
  的键表与专节、schema_guard 的 `INTERNAL_FIELDS` 登记（`table_entries`）。

**riscv64 收尾三条写法**（都是纯数据/小改）：

+ `jalr rd, rs1, imm`（三操作数，手册里的规范形；与两操作数写法**同编码**，只是文本不同）；
+ 位置符号 `.`：`__label` 把它记成**自引用**（空名），两遍回填时换成"当前指令自身的块下标"，
  与"标签回填块下标"同一口径（`jal zero, .`）；
+ `fence`/`fence.i`/`fence.tso` 从**整字常量**改成字段形——整字常量会与 `fence pred, succ` 的
  opcode 边在 [0,7) 上判成同一条 trie 边（生成期直接报 ambiguous）；顺带删掉因此失效的
  `W32` 表单与 `word` 位域（lint 的"写了却用不上"当场抓到）。

效果：riscv64 语料 `parsed` 129 → **135**、红桶 13 → **7**，字节对拍 **129 → 134 条逐字节全等
（`unparsed` 5 → 0）**——"有期望字节的用例全部对上"（`known` 仍 0）。riscv64 指令 137 → **139**、
派生枚举器 435 → **442**、未指定位评审清单 3 → **7**（fence 族字段形的固定 0 段）。
余下 7 条红桶是**一类**：立即数位置上的未定义符号（`%hi(foo)` 等，需要"imm 槽符号引用 +
回填后才算得出 hi/lo"这一设计项）与 `jal a0, a0`（上游当未定义符号 `a0`，我们拒了寄存器样子的
ident 当符号——已登记不改）。详见 `crates/tools/forge-tests/asm/README.md`。

### Added (2026-10-04) — `encode = "logical_imm"`：位掩码立即数（值 → N/immr/imms）+ aarch64 语料清零

**逻辑（位掩码）立即数**：ARM 系逻辑运算的立即数不是任意值——它必须是"一段连续 1 循环填充
整个宽度"的位模式，编码成三分量 `N`(1 位) / `immr`(6 位) / `imms`(6 位)。这是**非线性**映射
（由值的位结构反算分量），位切片表达不了，于是 `encode` 增加第二个方案：

```toml
[[operand_slots]]
name = "logicimm64"
kind = "imm"
signed = false
width = 64                     # 元素宽度（32 或 64）
encode = "logical_imm"
fields = ["nbit_l", "immr", "imms"]   # 依序 = N、immr、imms（校验期钉死宽度 1/6/6）
```

+ 实现（`__encode_logical_imm`/`__decode_logical_imm`，只在用到它的谱里发射）：① 找最小的
  重复元素尺寸；② 把那段 1 旋到最低位并确认它**连续**（可环绕）；③ 反算 `immr` 与 `imms`。
  不可编码的值（全 0、全 1、非连续段）在编码期报错、可编码集不是区间 ⇒ 生成期自测不按
  lo/hi 采样边界（代表值由方案给）。
+ 方案由**谱按名字选择**、实现由 DSL 提供（与 `kind = "cond"`/`wrap`/`unit` 同一类）。

aarch64 落地（+8 条指令：`and`/`orr`/`eor`/`ands` × X/W）：

+ 顶层用 **op9**（[31:23]）而不是 op8——`MOVZ/MOVN/MOVK` 的 op9 只在 bit23 上与逻辑立即数
  不同（0x1A5 vs 0x1A4），用 op8 会被位 trie 判成同一条边（生成期直接报 ambiguous）；
+ W 形式 `wrap = true, min = -2^32`：收得下 `#~15`、`#~(0xfe<<24)` 这类按位取反写出来的负数，
  规范化到无符号位模式；
+ 字节与上游注释逐条相同（`and w0, w0, #1` = `12 00 00 00`、`and sp, x5, #~15` 的
  immr=60/imms=59、`eor x1, x2, #0x8000` 的 immr=49 都对上）。

**aarch64 语料清零**（余下三条写法，全是纯数据）：`ret xN`（第二条 RET，rn 成操作数，解码靠
"叶 arm 先试 + 平铺 if" 回退）、`b.al`/`b.nv`（A64 里是保留码，上游汇编器照样收）、裸
`dcps1/2/3`（= `dcpsN #0`，声明成 imm16==0 的叶 arm；先试过 `[[pseudo]]`，但伪指令的前导
字面与指令重名会被校验期正确拒绝）。另加 `[reg.*].aliases` 的 `XZR`/`WZR`（31 号在"零寄存器"
语境下）与 `LR`——`orr w8, wzr, #0x1`、`ret lr` 才解析得动。

效果：aarch64 语料解析档 `parsed` 130 → **153**、**`no_prefix` 4 → 0、红桶 19 → 0**（语料里
能认出来的写法全部通过），字节对拍 `checked` 100 → **118 条逐字节全等**（`known` 仍 0）。
arm64 指令 200 → **206**、派生枚举器 818 → **825**。

### Added (2026-10-04) — `[[operand_slots]]` 的 `encode` + `fields`：一个值摊到多个字段

有些操作数在编码里不是"一段连续的位"：A64 的 `tbz`/`tbnz` 位序号（0..63）被拆到**两个
不相邻**的位域——低 5 位进 `b40`[23:19]、第 5 位进 `b5`[31]。DSL 此前只能声明"位域 ← 常量
或操作数"，没有"一个值 → 多个字段"这一环，于是 `tbz`/`tbnz` 整族写不出来。

新增两个槽属性（成对出现）：

```toml
[[operand_slots]]
name = "bitpos64"              # 位序号 0..63
kind = "imm"
signed = false
width = 6
encode = "slice"               # 位切片：按 fields 声明序从值的最低位切起
fields = ["b40", "b5"]         # b40(5 位)[23:19] + b5(1 位)[31]
```

+ **`slice`**：第 i 段 = 值 `>>`(前面各段宽度之和)，长度 = 该位域宽度；解码反向拼回
  （编码、解码、反汇编、生成期自测四处同源）。校验期要求 **Σ字段宽度 == 槽的 `width`**；
+ 表单 `operand_fields` 里这一项仍写**首字段**（`fields[0]`），其余字段由槽声明
  （写别的 → 校验期拒绝）；
+ 方案由**谱按名字选择**、实现由 DSL 提供（与 `kind = "cond"`/`wrap`/`unit` 同一类：
  能力是声明的，不是隐含的；没声明的谱生成物逐字不变）；只实现于定宽（含 `mixed`）
  编码/解码，变长（`prefix_scan`）ISA 写它 → 校验期 fail-closed；
+ 解码的**补集零 guard 要把槽的每个字段都算作"操作数位"**：只算 `operand_fields` 那一个
  会把 `b5`[31] 当成保留位、要求它为 0——实测 `TBZX[bit=63]` 编得出但解不回。

aarch64 落地：新增 `op7`/`b5`/`b40`/`imm14` 位域与 `off14`（label，`unit = 4`）、
`bitpos64`（slice 槽）、`bitpos32` 槽，表单 `TBZ` 与 `TBZ`/`TBNZ` 模板 × X/W = 4 条指令
（arm64 指令总数 188 → **192**，派生枚举器 778 → **802**）。3 条谱内 `[[vectors]]`
（含 `tbz w3, #5, #32764` 与 `tbnz x3, #8, #-32768` 两条上游注释里的真实字节）。

效果：aarch64 语料解析档 `parsed` 123 → **130**、**`no_prefix` 11 → 4**、红桶 19 不变，
字节对拍 `checked` 98 → **100 条逐字节全等**（`known` 仍 0）。

### Added (2026-10-04) — `[[operand_slots]].unit`：源值单位（分支偏移是字节）

真实汇编器里的分支偏移是**字节**：A64 的 `b #28` 编码成 `imm26 = 28 / 4 = 7`（`cbz` 的
imm19、`tbz` 的 imm14 同理），x86 的 `jmp rel32` 本身就是字节。而 DSL 的 imm/label 槽原先
把源文本的值**原样**写进字段——于是 `b #134217724` 被判越界（字段最大 33554431）、
`b 8` 编成 imm26 = 8 而 GAS/LLVM 会编成 2。

新增槽属性 `unit`（2 的幂，缺省 1；只对 `imm`/`label` 有意义）：

1. **值域按源单位**给（`imm_range` = 字段值域 × unit），越界报错；
2. 源值不是 unit 的整数倍 ⇒ 该条写法**不匹配**（与越界同处理，不静默取整）；
3. 编码写 `源值 >> log2(unit)`，**解码乘回来** ⇒ `Inst` 字段与 `disassemble` 渲染始终是
   源单位，`disassemble → assemble` 照旧闭合；
4. **符号标签**回填"块下标 × unit"（该块的字节偏移），与数字写法同单位。

这与宿主的 `RelocPatcher` 是同一口径（`Arm64RelocPatcher` 就是 `imm = offset >> 2`），
于是"汇编器里的数字写法"与"编译器打补丁"不再各说各话。**只实现于定宽（含 `mixed`）
编码/解码**：变长（`prefix_scan`）ISA 写它会**校验期 fail-closed**（明确报错，不静默当 1）。
编码期的"非整数倍"检查只对 **imm 槽**发——label 槽在编译器路径上塞的是**标签 id 占位**
（`LabelRef::EPILOGUE.id()`，不是字节偏移，随后由 reloc patcher 整体改写），查对齐只会误报；
文本路径的对齐检查在 `__imm`/`__label` 里，两条路都覆盖到了。

aarch64 落地：`off26`/`off19` 声明 `unit = 4`；另补 `[meta].imm_prefix = "#"`（模板虽都写
`#{imm}`，但**分支目标槽前没有字面 `#`**，`b #28` 的前缀得由这条声明来吃）。
生成期自测同步：立即数边界按字段单位算完再乘回源单位；越界探针按 unit 步进（否则
"非整数倍"会先把探针拦下，值域检查永远测不到）。

效果：aarch64 语料解析档 `parsed` 115 → **123**、红桶 27 → **19**，字节对拍 93 → **98 条
逐字节全等**（`known` 仍 0——含上游注释里 `cbz w20, #1048572` 这类期望字节）。

### Changed (2026-10-04) — `[[operand_slots]].roles` 由列表改为**一个** `OperandRole`（破坏性）

槽的角色声明从 `roles = ["in", "out"]` 改成 `roles = "inout"`——**一个 `OperandRole` 就够**：
`inout`（读改写）在语义上已经涵盖 `in` 与 `out`，旧的三种拼法
（`["in"]` / `["in", "out"]` / `["in", "out", "inout"]`）里后两种是同一件事，而且
`["in", "out"]` 还**接不住**读改写操作数（必须在集合里再补一个 `inout`）——那正是这次要
消掉的冗余。

+ 判定口径：声明 `inout` 的槽接受 `in`/`out`/`inout` 三种操作数声明，声明 `in` 的槽只接受
  `in`，声明 `out` 的槽只接受 `out`；**不写 = 不限制**（旧的 `roles` 缺省语义在代码里本就是
  "不校验"，文档写"缺省 `["in"]`"是漂移，一并改正）。
+ 连带删掉两条只对集合有意义的校验（"roles 不能为空"/"角色不能重复"）。
+ 迁移：三份发行谱 + 7 份夹具 + `forge-isa-dsl` 内的 22 处字符串夹具，合计 45 处
  （`docs/archive/**` 是历史记录，按仓库约定不动）。
+ 守卫照旧：`forge-isa-dsl` 全绿（含 `inout_role_parses` 改成断言 `Some(OperandRole::InOut)`）、
  `schema_guard` 绿（schema 里该键只有描述、无类型，`isa-dsl.schema.json` 无需重生成）。

### Changed (2026-10-04) — 生成物落盘件改写成 prettyplease 规范形（可读 + 编辑器能解析）

`isa_from_file!` 的落盘件（`$OUT_DIR/forge_gen_*.rs`）此前是**紧凑 token 文本**（x86 单行
1.4 MB）——人排查生成物时读不动，编辑器/rust-analyzer 对这种单行文件也给不出有意义的语法树。

现在正文过一遍 `syn::parse2` + `prettyplease::unparse`（rustfmt 风格），并加了两条不变量
（守卫 `gen_file::tests::generated_file_is_a_faithful_rust_file`）：

+ **规范形**：`unparse(parse(落盘件))` 逐字节等于落盘件 ⇒ 同一份谱每轮生成的字节稳定
  （否则每轮重写，rustc 每轮重编这个生成模块）；
+ **忠实**：解析回来的程序与生成 token 的规范形逐字节相等。忠实性**不比 token 文本**——
  prettyplease 会把 `#[doc = r"…"]` 渲染成 `/// …`（解析回来是 `#[doc = "…"]`），是同一个
  字符串值的两种合法拼写；比 token 文本会误报这处差异。

成本（dev 档实测）：三份发行谱合计 ≈ 0.8 s（x86 parse 0.58 s + unparse 0.20 s，
arm64/riscv64 各 ≈ 0.3 s），夹具谱可忽略；落盘件体积约为紧凑形的 3 倍。生成物的**对外面**
不变（`isa-host-demo` 的 `host_surface` 文本级守卫按空白归一化后照旧通过——它按
`# [doc = r"…"]` 属性形剥散文，规范形把文档渲染成 `/// …` 行注释，两形都已兼容）。

顺带修掉三处**过期快照**（前几批补指令后没同步，与本批功能无关）：`lint_shipped` 的
arm64 未指定位评审清单 71 → 123；`variants` 的 riscv64 原生/RV32 投影指令数
119/105 → 137/120（被投影掉的 RV64 专属指令 14 → 17）；`isa/arm64.toml` 的
`ands`/`bics` 无后缀形态去掉两个没人引用的 `ref`。

### Fixed (2026-10-04) — 定宽解码位 trie 的 `else if` 链把后一条边变成死代码

**症状**：`bics x0, x1, x2`（以及 `bics` 的移位形态、`bics w0, w1, w2`）**编得出、解不回**——
`encode` 给出正确字节 `20 00 22 ea`，`decode` 却返回 `None`。同族的 `ands` 一切正常。

**根因**（`forge-isa-dsl` 的 `dsl/codegen/mod.rs::emit_bit_trie`）：定宽解码的位级 trie 把同一
节点上的边展开成 **`else if` 链**。但同一节点上两条边的位区间**可以不相交、却同时命中同一个
字**——`bics` 的 N 位在 bit21、逻辑族的移位在 bits[22,24)，`0xEA220020` 两条都命中。`else if`
只在**前一条边的条件为假**时才试下一条，于是"条件为真、但子树的叶子 guard 没过而返回不了"
会**直接跳出整条链**：`ands` 的移位边先接住这个字、guard 不过，`bics` 那条边永远到不了。

**修法**：边**平铺成互不嵌套的 `if`**（试一条、没返回就试下一条），与定宽 decode 的既有语义
（叶 arm 优先 + 声明序）一致。变长（字节）decode 的 `emit_dec_trie` 不受影响——那里同一节点的
掩码由 `check_dec_trie_overlaps` 保证互斥，`else if` 既安全又完整（该处注释已写明这个区别）。

**回归**：`crates/backend/forge-codegen/tests/arm64_tests.rs::decode_bit_trie_disjoint_edges_fall_through`
（5 条真实写法编解码往返）、`isa/arm64.toml` 的 `bics x0, x1, x2` 谱内向量，以及生成期自测
（`spec_bics*` 全部由红转绿）。这条缺陷此前不显形，是因为**没有任何谱在同一 opcode 节点上同时
用到"位区间不相交却不是同一位域"的两条边**；补 aarch64 逻辑族（移位后缀 + N 位）才第一次踩到。

### Added (2026-10-04) — aarch64：补逻辑（移位寄存器）族（移位后缀 + `ands`/`bics`）

语料 `arm64-logical-encoding.s` 里 68 行卡在逻辑族的**移位后缀**与 `ands`/`bics` 的无后缀形态上，
这批补齐（**纯谱数据，生成器零改动**）：

+ 位域新增 `shift`（[22,24)，2 位常量）、`imm6`（[10,16)）、`imm5`（[10,16) 的 5 位读法）；
  表单 `ALUSH6`/`ALUSH5`（`rd/rn/rm` + 移位量）；操作数槽/模板各若干。
+ 指令 64 条 = `and`/`ands`/`bic`/`bics`/`orr`/`orn`/`eor`/`eon` × `lsl`/`lsr`/`asr`/`ror` × X/W，
  另加 `ands`/`bics` 的无后缀形态 4 条（此前整族未声明）→ arm64 指令总数 120 → **188**。
+ **为什么移位种类是"逐指令常量"而不是新语法**：移位量是操作数（6 位/5 位槽），移位种类是 2 位
  常量——`[[templates]]` 的行正好是"同一形状、逐行给常量"的载体，故 8 条模板 × 8 行即可，
  DSL 不需要"值 → 多字段"这类新能力（位掩码立即数与 `tbz`/`tbnz` 仍需要，见
  `crates/tools/forge-tests/asm/README.md` 的缺口清单）。
+ **W 形式用 5 位槽**：A64 规定 32 位形式的 `imm6<32`；用 6 位槽会静默编出未分配的字。
+ 4 条谱内 `[[vectors]]`（`and … ror #2`、`bics x0, x1, x2`、`bics … asr #3`、`orn w … lsl #7`）。

效果：aarch64 解析档 `parsed` 47 → **115**、`no_prefix` 35 → **11**、红桶 71 → **27**；
字节对拍 `checked` 25 → **93 条逐字节全等**（`known` 仍 0）——新族的每个字节都与 LLVM 上游注释相同。

### Fixed (2026-10-04) — aarch64：两处把 `#` 当注释的缺陷（立即数全被截断）+ 系统/异常族

补 aarch64 系统/异常族时撞出**两处独立缺陷**，根因都是"`#` 被当成注释"——AArch64 的 `#` 是**立即数前缀**（注释是 `//`）：

1. **语料档**（`crates/tools/forge-tests/src/asm/corpus.rs`）：aarch64 suite 的 `comments` 列了 `#`，于是 `tbz x1, #3, foo` 被截成 `tbz x1,`、`cbz w1, #28` 截成 `cbz w1,`，这些行被误判成"本 ISA 没有这条指令"。实测这批文件里**没有以 `#` 开头的注释行**（LLVM 的 AArch64 用例用 `;` 与 `//`），`#` 只出现在立即数位置 ⇒ 从注释集里去掉。棘轮样本随即可见真实文本（`b #28`、`cbz w1, #28`、`and w0, w0, #1`）。
2. **谱**（`isa/arm64.toml`）：`comment_char = "#"` ——生成物对**每一行**按 `#` 截断，`add x0, x1, #5`、`ldr x0, [x1, #8]` 全被砍成 `add x0, x1,` 而解析失败。这解释了 aarch64 语料 `parsed` 长期只有 37/420。改成 `comment_char = "/"`（A64 的注释是 `//`，取单字符后正好是它）。

顺带修的（同一批暴露的）：

+ **`[meta].imm_prefix` 只实现了 `$`**（声明 `#` 静默不生效）：现在按声明吃对应 token（词法只有 `Dollar`/`Hash`），并在声明期**拒掉**其它字符（"写了却不起作用"改成"写不出来"）。
+ **`SYSEXC` form 的位域重叠**：系统/异常的顶层常量我一开始写成 `b16`（[31:16]），与 `imm16`（[20:5]）在 bit16-20 重叠 ⇒ imm16 取大值时把顶层常量写坏（**编得出、解不回**）。派生日志守卫实测 `SVC[imm16=65535]` 解码失败当场抓到；改成 11 位顶层（[31:21]，复用 `mtop` 段）后 32 位三段互不重叠。

**补的族**：`SVC`/`HVC`/`SMC`/`BRK`/`HLT`/`DCPS1..3`（`SYSEXC`：11 位顶层 + `imm16[20:5]` + `op5[4:0]`）与整字常量 `ERET`（0xD69F03E0）/`DRPS`（0xD6BF03E0）。模板里 `#` 与其它 A64 指令一致写成 `#{imm}` 字面量。

效果：aarch64 解析档 `parsed` 37 → **47**、`no_prefix` 45 → **35**，字节对拍 `checked` 15 → **25 条逐字节全等**（`known` 仍 0）；另加 4 条谱内 `[[vectors]]`（`brk #1`、`svc #8`、`brk #65535`（钉住重叠那处修复）、`eret`）。arm64 指令总数 110 → **120**、派生枚举器 360 → **386**（人工复核后就地更新）；x86 275 / riscv64 137 不变。

剩余 aarch64 缺口已按根因登记：位掩码立即数编码（值 → N/immr/imms）、移位后缀操作数、`tbz`/`tbnz`、分支立即数目标、`dcpsn` 的省略立即数（需要"可选操作数"）、`b.al`、`ret lr`。

### Added (2026-10-04) — riscv64：补完 Zicsr/`fence.i`/`fence.tso`/`BGEU` 与别名 `unimp`（`no_prefix` 桶清零）

语料的 `no_prefix` 桶（"没有任何候选的首段能对上" = 缺指令）在上一批之后还剩 14 条，这批清到 **0**：

+ **`BGEU`**：`BB` 分支模板只少了这一行（`funct3 = 7`）。
+ **Zicsr 六条**：`CSRRW`/`CSRRS`/`CSRRC`/`CSRRWI`/`CSRRSI`/`CSRRCI`（opcode 0x73，`funct3` 1/2/3/5/6/7）。
  文本里的操作数序（rd, csr, 源）与字段绑定序（rd, rs1, imm12 = I 型 form）**不必一致**——`asm` 用命名引用，绑定由 `operand_fields` 决定（v15-S3c 的命名操作数设计，这里正好用上）。
+ **`FENCE_I`**（0x0000100F）与 **`FENCE_TSO`**（0x8330000F = `fence rw, rw` + `fm=8`）：整字常量，与 `FENCE` 一样用 `W32` 形态。
+ **`unimp`**：LLVM/GAS 把它定义为 `csrrw x0, cycle, x0`（0xC0001073）——它是**别名**不是新编码，声明成整字指令会与 `CSRRW` 撞车（解码器按"编码不相交"建 trie，直接 `DSL-OTHER ... overlap ... ambiguous` 报错），所以用 `[[pseudo]]` 展开一条。

两个值得记的坑：

1. **CSR 号是 12 位无符号**（`0xfff` = 4095），拿有符号的 `imm12` 槽（load/store 位移用）会让 `csrrw t0, 0xfff, t1` 报越界 ⇒ 新增 `csr12` 槽（unsigned，12 位），三条 `*I` 的 zimm 另用 `zimm5`（unsigned，5 位，与 `shamt_w` 同宽但语义不同，单列）。
2. `shamt_w` 的注释顺带改正：用它的是 0x1B 的 `slliw`/`srliw`/`sraiw`（0x3B 那三条是寄存器移位）。

结果（`asm/ratchet/`）：riscv64 解析档 `parsed` 115 → **129**、**`no_prefix` 14 → 0**、红桶 13 不变；字节对拍 `checked` 115 → **129 条逐字节全等**，`known` 仍 0。另加 5 条谱内 `[[vectors]]`（`bgeu`/`csrrw t0, 0xfff, t1`/`csrrsi`/`fence.tso`/`unimp`）。riscv64 指令总数 128 → **137**、派生枚举器 383 → **435**（人工复核后就地更新）；x86 275 / arm64 110 不变。

剩余 13 条红桶已按**根因**登记进 `crates/tools/forge-tests/asm/README.md`：① 需要重定位/符号地址（5 条，缺"未定义符号 → 重定位记录"）；② 位置符号 `.`（2 条）；③ **`fence` 的字母集合操作数**（4 条）——需要一类**新能力**「命名位集合」操作数（表 + `kind = "bitset"` 槽：按表里名字贪心拼接解析、取位或编码、按声明序拼接渲染），不是 `fence` 的特例，先登记不做；另加两寄存器形式的 `jal`。

### Added (2026-10-04) — riscv64：字节/半字访存、W 立即数移位；修掉 W 寄存器移位的编码错误

从语料的 `no_prefix` 桶（"没有任何候选的首段能对上"= 缺指令）按首词归并出来的清单里，补上最基础的一族，并顺带修掉一处**真编码错误**：

+ **补** `LB`/`LH`/`LBU`/`LHU`/`SB`/`SH`（与 `LW`/`SW` 同形，只差 `funct3`）与 `SLLIW`/`SRLIW`/`SRAIW`（opcode 0x1B + `shamt_w`，5 位 shamt 独立于 6 位槽）。
+ **修**：`SLLW`/`SRLW`/`SRAW` 原先被声明成**立即数**移位（0x3B + shamt 槽）——ISA 里它们是 **R 型寄存器**移位（rs2 = 移位量），立即数那条是 0x1B 的 `slliw`/`srliw`/`sraiw`。三条改成 `WW` 模板的行（与 `ADDW`…`REMUW` 同形）；`shamt_w` 槽由新的 `*IW` 三条接着用，不留死声明。
+ **求值器补一元 `!` = 逻辑非**（`!x` = x==0 ? 1 : 0）：上游期望字节实证 `lh t1, !1(zero)` 的位移是 **0**（`03 13 00 00`）而不是按位取反的 -2（那条写的是 `~2047`）。这是**表达式运算符**（与 `~` 同类，任何 ISA 都可能出现在立即数里），不是 ISA 知识。

效果（`asm/ratchet/`）：riscv64 解析档 `parsed` 89 → **115**、红桶 15 → **13**、`no_prefix` 38 → **14**；字节对拍 `checked` 89 → **115 条逐字节全等**，`known` 仍 0——两族新指令的每个字节都与上游注释相同（不是我手算的）。另加 4 条谱内 `[[vectors]]`（`lb`/`sh`/`slliw`/`lh t1, !1(zero)`）把语义钉在谱里；x86 指令总数 275 不变，riscv64 119 → **128**、派生枚举器 341 → **383**（人工复核后就地更新）。

`crates/tools/forge-tests/asm/README.md` 的缺口清单同步收窄：riscv64 红桶只剩三类——需要重定位/符号地址（`%pcrel_hi(foo)` 等）、位置符号（`.`）、寄存器形式的 `jal`/`jalr`；`no_prefix` 只剩 Zicsr、`fence` 族与 `unimp`。

### Added (2026-10-04) — `[[conventions.imm_fn]]`：立即数修饰改成**谱声明的数据**（DSL 里没有修饰名）

各家的汇编器都有一批**立即数修饰写法**：GAS/RISC-V 的 `%hi(x)` / `%lo(x)`、GAS/ARM 的 `:lower16:x` / `:upper16:x`、MIPS 的 `hi(x)` / `lo(x)`——拼写不同、值语义也不同（带不带进位补偿、掩码几位、按不按有符号读数）。把其中某一种写进求值器，等于让通用 DSL 变成"为那个 ISA 设计"的系统；所以新能力是一张**声明表**，每条两行数据：

```toml
[[conventions.imm_fn]]
name = "hi"                  # 名字（唯一诊断名；不参与文本匹配）
text = "%hi({0})"            # 源文本形态：`{0}` 处是内层表达式，其余按字面 token 匹配
expr = "({0} + 0x800) >> 12" # 值语义：`{0}` 处是内层表达式的值
```

+ **匹配**：`text` 的两段是字面 token（`mnemonic_case = insensitive` 时 Ident 豁免大小写），中间的 `{0}` 递归解析成一个表达式；前缀/后缀/两侧都有字面都行（`{0}:lo` 合法）。
+ **求值**：`expr` 用**既有表达式语言**——生成期把 `expr` 词法化，运行期把 `{0}` 换成数值 token 再走 `__expr`，因此任意算术与任意嵌套（`%hi(%lo(x))`、`expr` 里再用别的修饰）天然成立，**不引入第二套表达式语言**。
+ **缺省=没有**：没声明 `imm_fn` 的谱生成物里根本没有修饰机制（与引入前逐字相同）；没声明的拼写（`%hi` 在 x86 上）就是"这条写法不匹配"，不猜语义、不当常量。
+ **校验**：`name` 非空唯一；`text`/`expr` 各含**恰好一个** `{0}`；`text` 必须含至少一个字面（光一个 `{0}` 是通配，会吞掉任意表达式）。
+ **三方针同步**：`dsl/model.rs`（`ImmFnDef`）/`src/schema.rs` + 重新生成的 `isa-dsl.schema.json` / `docs/reference/isa-dsl.md`（键速查表 + 「立即数修饰」节，含"为什么是数据而不是内置"的说明）。

**通用性的正面证据**：同一机制、零代码改动，`isa/riscv64.toml` 声明 `%hi(x)`/`%lo(x)`（带括号、进位补偿 + 低 12 位有符号读数），夹具 `crates/backend/forge-codegen/tests/isa/demo.toml` 声明 `:lower16:x`/`:upper16:x`（**无括号、纯掩码**），`demo_tests::imm_fn_is_spec_data_not_isa_knowledge` 真跑字节并验证嵌套与"未声明拼写报错"。

效果：riscv64 语料解析档 `parsed` 82 → **89**（红桶 22 → **15**）、字节对拍 `checked` 82 → **89 条逐字节全等**（`lui a0, %hi(2)` = `37 05 00 00`、`jalr a0, %lo(2048)(a1)` = `67 85 05 80`、`lw a0, %lo(97)(a2)` = `03 25 16 06`…，`known` 仍 0），另加 4 条谱内 `[[vectors]]` 把语义钉在谱里。**未实现**（如实登记）：需要重定位的修饰（`%pcrel_hi(foo)` 且 `foo` 未定义）——那不是值语义能表达的，要有重定位记录。

### Changed (2026-10-04) — `[[pseudo]]` 改 `asm` 声明形态：参数表从写法里长出来（破坏性，无兼容层）

`[[pseudo]]` 的 `params` 列表**删除**，改成两个键：`name` 是这条声明的**唯一标识**（与写法无关）、`asm` 是**这条伪指令的书写规范**（分派按它的前导字面），`asm` 里的 `{名字}` **就是**参数——`emit` 里引用的是同一批名字，没有第二份清单可以漂移：

```toml
[[pseudo]]
name = "li"                     # 声明标识（唯一即可，不必等于写法首词）
asm  = "li {rd}, {imm}"         # 书写规范：`{名字}` 即参数（按首次出现序）
emit = ["lui {rd}, (({imm} + 0x800) >> 12)", "addi {rd}, {rd}, ((({imm} + 0x800) & 0xfff) - 0x800)"]
```

+ **`name` 与写法解耦**：分派看 `asm` 首段的首词（同一分派键下可有多条写法，按声明序逐个试），`name = "load_imm"` + `asm = "li {rd}, {imm}"` 合法；遮蔽检查也跟着走**写法**而不是名字（`name = "alias"` + `asm = "mov …"` 会被拦下）。顺带把文档里一直写着的"emit 行可以嵌套别的伪指令"**真正实现**（展开行递归再判），并让 ci ISA 的首词匹配大小写不敏感。
+ **切参按模板的字面段走**（不再"按逗号位置猜"）：`,` 是模板写死的分隔符，`li x10,0x1234`（无空格）照样认；缺分隔符 / 多一段 / 尾部多东西都**当场报错并回显写法**（`伪指令 'li' 的写法是 'li {rd}, {imm}'：缺 ','`）。
+ **展开行装配失败时点名是哪一行展开出来的**：`li x10, 0x1234 junk` 以前只会得到 "no matching instruction"，现在是 `伪指令展开行 'lui x10, ((0x1234 junk + 0x800) >> 12)' 汇编失败：…`。
+ 校验同步收紧：`asm` 首字面必须**就是 `name`**（伪指令按整词分派）、占位符之间必须有字面分隔、`emit` 里的 `{…}` 必须是 `asm` 声明过的参数（报错时回显 `asm`）、`asm` 声明的参数必须都被用到；零操作数伪指令合法（参数表就是空的）；同一名字在 `asm` 里出现两次表示**前后必须写成一样**。
+ 三个同步点一起改：`dsl/model.rs` 结构体、`src/schema.rs` 键表 + 重新生成的 `isa-dsl.schema.json`、`docs/reference/isa-dsl.md` 的键速查表与 `[[pseudo]]` 小节（`schema_guard` 三方针全绿）。

### Added (2026-10-04) — x86 SSE 比较谓词族 + 八个经典别名（`cmpltps` 等）

上游 `intel-syntax-encoding.s` 的 `cmpltps XMM2, XMM1` 是谱里登记的缺口：x86 从来没有 SSE 谓词比较指令。补 `CMPPS`/`CMPPD`/`CMPSS`/`CMPSD_SCALAR`（`0F C2 /r ib`，`ps/pd/ss/sd` 靠强制前缀区分，谓词是立即数第 3 操作数），八个经典 packed 别名（`cmpeqps`/`cmpltps`/`cmpleps`/`cmpunordps`/`cmpneqps`/`cmpnltps`/`cmpnleps`/`cmpordps` 及 `pd` 同族）用 `[[pseudo]]` **文本展开**表达——一行别名换一行 `cmpps {a}, {b}, <谓词>`，不新增 32 条指令、也不引入第二套分派（标量别名刻意不做：那会盖掉 `cmpss xmm, xmm, imm8` 的三操作数写法）。

证据：`cmpltps XMM2, XMM1` → `0F C2 D1 01` 与上游注释逐字节相同，解析档 `llvm-mc` x86 `parsed` 35 → **36**（`no_prefix` 21 → 20），字节对拍 x86 `checked` 30 → **31**（`known` 仍 0）；四条指令进生成期自测与派生枚举器（x86 指令总数 271 → **275**、枚举器 1327 → **1351**，歧义名单未变）。

### Added (2026-10-04) — x86 内存形式的 `mov` 族（8/16/32 位）+ `movsxd`/`xchg` 的内存源形式

真实汇编里最常见的 `mov eax, [rbx+8]`、`mov [rbx+8], eax` 在谱里**一条候选都没有**：已有的 `MOV_R_MEM` 系列 rm 引用的是 `reg` 槽（`[{src}]` 简写——那是给 IR 降级用的 **vreg 基址**，没有位移/索引/尺寸前缀），而 `MOV64_RM`/`MOV64_MR` 固定 `opsize = 64`（且带 `stack_arg_*` 角色，不能动）。补 6 条内存形式指令：

+ `MOV_R_MEM_AUTO`(8B) / `STORE_MEM_R_AUTO`(89)：16/32 位 load/store，走**完整内存模板**（位移、`index*scale`、尺寸前缀、无基址），66 前缀由数据寄存器宽度驱动；
+ `MOV_R_MEM_8_AUTO`(8A) / `STORE_MEM_R_8_AUTO`(88)：8 位是独立操作码，同样走完整模板（改前连 `mov al, [rbx+2]` 都解不出来）；
+ `MOVSXD_R_MEM`(63)：`movsxd rax, dword ptr [mem]`（原来只有寄存器源）；
+ `XCHG_MEM_R_AUTO`(87)：带位移的 `xchg [mem], r`（原来的 `XCHG_MEM_R` 是简写、表示不了位移）。

新槽 `gpr24`（16/32 位）刻意**不含 64 位**：64 位的内存 mov 已由 `MOV64_RM`/`MOV64_MR` 覆盖，再收 64 位就是同一批字节的第二条候选（无冗余）。六条都**不写 `ref`**（与 ALU 内存族同纪律）：IR 降级路径不变，候选集与运行期行为逐字节不动，JIT 矩阵不受影响。

证据：字节逐条钉住（`mov ax, [rbx+2]` = `66 8B 43 02`、`mov r8w, [rbx+2]` = `66 44 8B 43 02`、`mov [rbx+2], r8d` = `44 89 43 02`、`movsxd rax, dword ptr [rbx]` = `48 63 03`、无基址 `mov eax, [0x12345678]` = `8B 04 25 …`）+ 编解码闭环测试；并且 `asm/exec/x86/` 新增 `mem_mov32.s` / `mem_mov16.s` 两个小程序**真执行**（`ASM-EXEC-SUMMARY ran=8 → 10 skipped=0`）。守卫同步：x86 指令总数 265 → **271**、派生枚举器 1263 → **1327**（歧义名单未变）。语料棘轮不动（vendored 语料里没有这些写法，全量语料里遍地都是）。

顺带登记一个**设计级**缺口（不在这批修）：`movzx`/`movsx` 的内存源形式卡在"文本分不出源宽度"——`byte`/`word` 是 `{size}` 组件、按设计不携带宽度，两条指令的文本与类型签名完全相同（已在歧义名单登记），照搬一套 16/32 位目的地 variant 只会让 `movzx eax, word ptr [rbx]` 静默编成 byte 那条；要修得先决定 `{size}` 是否携带宽度，或把尺寸关键字写成模板字面量。

### Added (2026-10-04) — 汇编器：`.set`/`.equ` 符号常量、位移与立即数共用值语法、地址尺寸覆盖前缀 `0x67`

三条都是**汇编器 / DSL 语义**的用户可见变化（破坏性，无兼容层）：

+ **符号常量**：`.equ name, expr` 与 `.set name, expr` 是同一个伪指令的两种拼法（GAS 两个名字等价，LLVM 用例写的是 `.set`）；**顺序求值**（前向引用失败、同名后定义覆盖前定义）。引用它的地方与字面量走**同一套值语法**——**立即数与内存位移都能写**（`cmp eax, FOO`、`mov rax, FOO[rbx]`、`[rbx+FOO*2]`）。位移以前只认字面量（`__raw_int` / `__raw_signed_int` 两个 helper + 四段分支），现在与立即数共用递归下降求值器 `__expr`——**两个 helper 与那些分支整体删除**（无冗余）；一元 `+` 变恒等（上游语料 `lwu x2, +4(x3)` 就是这种写法）。
+ **地址尺寸覆盖前缀**（新数据：`[[conventions.prefix_scan]].effects = ["addr32"]` / `["addr16"]`，x86 = `0x67`）：内存 base/index 寄存器宽度 ≠ `[meta].addr_width` 时（64 位模式下的 `[eax]`）**编码器自动发**这条前缀，**解码器**看到它就把 base/index 建成覆盖宽度的地址类（`[eax]` 与 `[rax]` 是不同地址，不能都建成 64 位）；`[base]` 简写（`reg` 槽当基址）的槽也是地址，同样按地址类解。**修掉的缺陷**：改前 `add eax, [eax]` 静默编成 `03 00`（真值 `67 03 00`）——地址尺寸错、字节却认得出，`gnu-gas-intel` 语料里 32 条 32 位地址行的字节据此由错转对。覆盖宽度必须指向本谱**已声明**的寄存器组；一个 ISA 只支持一个覆盖宽度；缺省扫描集**不含**该效果（是谱自己声明的数据，不隐式强加给别的 `prefix_scan` 谱）。
+ **语料台**（`crates/tools/forge-tests`）：`.equ`/`.set` 定义**按行号**喂进行上下文——只喂本行**之前**的定义（`rv32i-valid.s` 里 `CONST` 先 30 后 16，整篇一起喂会把两个引用解成同一值，字节对拍会假红）；`encoding:` 注释的位置改成**带起始行的逐段声明**（`intel-syntax-encoding.s` 前 94 行是"注释在指令前"、尾部两条是"注释在指令后"，整篇一个风格会把 `[0x83,0xf8,0x02]` 配给后一条指令、凭空造出一条字节差异）；字节对拍档开始与解析档共用同一份上下文。

效果（数值以 `crates/tools/forge-tests/asm/ratchet/` 为准）：解析档 x86 `llvm-mc` 33 → **35**（`cmp eax, FOO` 与 `cmp eax, FOO[eax]` 都从红桶进 `parsed`）、riscv64 72 → **82**、x86 `gnu-gas-intel` 50 条不变；**字节对拍** x86 28 → **30 条逐字节全等**、riscv64 72 → **82 条逐字节全等**，`known` 与 `variants` 全 0。`gnu-gas-intel` 里那 32 条 32 位地址行现在编出的是**带 `0x67` 的正确字节**（不是"以前也算 parsed"）。

### Changed (2026-10-03) — `MemRef.base` 改 `Option<Reg>`：无基址（绝对）寻址（破坏性，无兼容层）

`MemRef.base` 从 `Reg` 改成 `Option<Reg>`，**基址的有无由模板形状声明**：模板里**写了 `{base}`** 就是"这条写法必须有基址"（缺了整条不匹配）；**没写 `{base}`** 就是"这条写法**没有基址**"（`base = None`），此时其余组件一律按**必需**处理（没有基址时位移/索引就是地址本身，否则 `[0]` 会渲染成 `]`）。校验：模板最多一个 `{base}`；无基址模板必须含 `{disp}` 或 `{index}`。这样"要不要基址"只有一处声明，不需要新键，也没有第二个开关可与它矛盾。

+ x86 用 `mod=00` + `rm=100` + `SIB.base=101` + disp32 表示无基址（ModRM 的 `mod=00`/`rm=101` 是 RIP 相对，本 DSL 仍明确拒绝）；编解码两侧对称。
+ 顺带**合并两份重复的内存编解码实现**（"无冗余"）：编码侧普通 ModRM 与 VEX 两条**逐字节相同**的 ModRM/SIB/位移发射合成一个 `gen_mem_modrm`（EVEX 的 disp8 要按 `disp_scale` 压缩，另存一份）；解码侧三条（普通/VEX/EVEX）的 SIB+位移解码合成一个 `gen_mem_decode`（用 `memref` / `disp8_scale` 两个参数表达差异）。
+ 只写 `[base]` 的**简写**（`rm = "[槽名]"`，`reg` 槽当基址）表示不了无基址 ⇒ 解码时**显式拒绝**：不拒就会把绝对地址当成 `[RBP]` 解出来、还少读 4 字节 disp32，把真正的 MemRef 形式挤掉（本轮实测 `48 8B 04 25 …` 一度被 `MOV_R_MEM` 抢走，`spec_vector_83` 当场抓到）。
+ x86 谱声明第三条模板 `{size}[{disp}]`，并加一条 `[[vectors]]` 钉住它（`mov RAX, [-8]` → `48 8B 04 25 F8 FF FF FF`）。效果：`llvm-mc` 解析档 32 → **33**；x86 字节对拍 27 → **28 条逐字节全等、0 条差异**——`movsd XMM5, QWORD PTR [-8]` 编出的 `f2 0f 10 2c 25 f8 ff ff ff` 与上游注释里的期望字节一致。"无基址寻址"据此从 `asm/README.md` 的缺口清单移除。

### Added (2026-10-03) — x86 一元 `inc`/`dec`（`FF /0`、`FF /1`、`FE /0`、`FE /1`）

与 `not`/`neg` 同族（`MRR_EXT_OP`：reg = 固定扩展码、rm = 目的操作数）：16/32/64 位走 `FF /0`、`FF /1`，8 位走 `FE /0`、`FE /1`——`inc`/`dec` 的 16 位短编码（`40+r`..`4F`）在长模式里已经是 REX 前缀，所以只能用 `FF` 这一组。四条指令写成两条 `[[templates]]`（各 2 行）；**不写 `ref`**：IR 的 `+1`/`-1` 走 ADD/SUB 的立即数形式，降级不发射它们，所以候选集与运行期行为不变（JIT 矩阵逐条一致）。

效果：`gnu-gas-intel` 解析档 `parsed` 48 → **50**、`no_prefix` 6 → **4**，64 条里只剩 7 条红桶（全是 32 位模式专有的段寄存器 push/pop）。

**归因口径变化（不是回归）**：补上 `inc`/`dec` 后，`apx-rex2-format-intel.s` 里 8 行 APX 形态的 `inc`/`dec`（`inc r16d`、`dec dword ptr [rax + r16]`）从 `no_prefix`（"没有候选的首段能对上"）挪进了 `tail_mismatch`（"首段对得上、后面没对上"）——以前连 `inc` 都不认识，现在知道助记符对、错在 APX 操作数。`parsed` 不变，红桶数字变化仅此一项：`llvm-mc` 的 `no_prefix` 29 → **21**、`tail_mismatch` 50 → **58**。

### Added (2026-10-03) — 立即数槽 `wrap`：字面量按 W 位位模式读（两种读数都收）

立即数字段是 W 位**位模式**——`0x90909090` 与 `-1869574000` 是同一批字节，各家汇编器两种写法都收。DSL 原先只收 `signed` 决定的那一种读数，于是 x86 的 `add eax, 0x90909090`（`imm32` 是**有符号** 32 位，放不下 `0x90909090`）汇编不了，`gnu-gas-intel` 有 8 行卡在这里。

新槽键 `wrap = true`（声明侧校验：仅 `kind = "imm"` 且 `width < 64`）：

+ **接受域**放宽到两种读数的并集（有符号槽多收无符号写法、无符号槽多收负数写法）；
+ 解析后**规范化**回 `signed` 的读数再进编码（`v > 规范上界 ⇒ v - 2^W`、`v < 规范下界 ⇒ v + 2^W`）；
+ **规范值域 `imm_range()` 不变**：解码的符号扩展、反汇编渲染、生成期自测的 `min`/`max` 边界检查照旧——所以 `add rax, -12` 的文本与字节逐字节不变；超出 W 位**仍然报错**（不静默截断/掩码）；
+ 候选特异性（`form_specificity`）与类型签名（`type_signature`）改按**接受**域计（口径是"能接受多少输入"），规范域只影响解码侧。

x86 谱据此给 `imm32` 打开它，并加了一条 `[[vectors]]`（`add EAX, 0x90909090` → `81 C0 90 90 90 90`）：`gnu-gas-intel` 解析档 `parsed` 40 → **48**，红桶只剩 7 条**段寄存器 push/pop**——那些是 **32 位模式专有语法**（长模式下只有 `fs`/`gs` 可 push/pop，`daa`/`das`/`aaa`/`aas` 也已移除），照抄等于给 x86-64 编非法指令，已在 `asm/README.md` 登记为"边界"而非"缺口"。规范见 `docs/reference/isa-dsl.md` 的「立即数字面量的两种读数」。

### Added (2026-10-03) — x86 8 位 ALU 族（`*_MR_8` / `*_R_MEM_8` / `*_RM8_IMM8` 各 8 条）

x86 的 8 位算术是**独立操作码**（不是同一操作码换宽度前缀）：`00 /r`（r/m8 ← r/m8 op r8）、`02 /r`（r8 ← r8 op r/m8）、`80 /digit ib`（r/m8 ← r/m8 op imm8）——ADD/OR/ADC/SBB/AND/SUB/XOR/CMP 各一条，共 24 条，一条 `[[templates]]` 各 8 行。寄存器槽用 `gpr1b`（class `gpr1` + `byte_reg = true`），并给它补上 `inout`（8 位 ALU 是读改写）。与 16/32/64 位族一样**不写 `ref`**，IR 降级不变（JIT 矩阵 x86 197/3/0、riscv64 136/64/0、arm64 23/177/0 与改动前逐条一致）。

顺带给操作数槽 `gpr8` 补了说明注释：它的 `class` 是 **`gpr8`（64 位名字）**，这是**有意的**——`SETCC_RM8 {out}, …` 的降低文本里必须是承接布尔结果的 64 位 vreg 名（`xor {out}, {out}` 先清零，故低字节即结果），把 class 改成 `gpr1` 会让所有 Cmp/Fcmp 降级当场汇编失败。真正的 8 位名字槽是 `gpr1b`。

效果：`gnu-gas-intel` 解析档 `parsed` 16 → **40**、`tail_mismatch` 39 → **15**（只剩「无符号 imm32」与「段寄存器 push/pop」两类）；字节对拍不变（27 条逐字节全等、0 条差异）。

### Fixed (2026-10-03) — 1 字节操作数的 `opsize` 解码守卫恒为假，8 位指令解不出来

`opsize = "s0"`（或操作数名）指向**单类 1 字节槽**时，生成物会写下 `__opsize == 1` 的解码守卫；而 `__opsize` 只可能取 **2/4/8**（由 66 前缀与 REX.W 扫描决定）——8 位操作数**没有** opsize 前缀可查，守卫恒假 ⇒ 该指令**解码永远失败**（实测 `00 00`（`add [rax], al`）解不出来，`isa_roundtrip_guard` 当场红）。以前没暴露是因为缺省分支的 `find(|w| *w == 2 || *w == 4 || *w == 8)` 恰好把 1 字节槽过滤掉了。现在宽度 ∉ {2,4,8} ⇒ **不发守卫**（与缺省分支同一口径）；编码侧不受影响（1 字节本来就不出 66/REX.W）。

### Changed (2026-10-03) — `[conventions.mem]` 改成模板**列表** + `{size}` 尺寸前缀（破坏性，无兼容层）

声明从 `template = "…"` 变成 `templates = ["…", …]`：**第 0 条 = 反汇编渲染形态**（渲染必须唯一，否则 `disassemble` 的输出会随解析尝试顺序漂移），其余是**解析专用备选**——按列表序逐条试，第一条整条走通的赢。同一份 `MemRef` 的几种合法写法（x86 的 Intel `[base+disp]` 与 GAS `disp[base]`）各写一条，不必硬塞进同一条模板；空列表报错。

+ 新组件 `{size}`：吃掉 `size_keywords` 里的任一关键字（大小写按 `[meta].mnemonic_case`），**值不进 `MemRef`**——宽度由操作数槽/`opsize` 决定，尺寸前缀只是给人读的冗余提示（`mov QWORD PTR [rsp-16], rax` 与 `mov [rsp-16], rax` 编出同样的字节）。渲染输出空串，故 `disassemble → assemble` 照旧闭合；模板里用了 `{size}` 却没声明 `size_keywords` ⇒ 生成期报错。紧邻 `{size}` 的字面量**不是**它的条件前缀（否则 `qword ptr [rax]` 的 `[` 会随尺寸前缀一起消失）。
+ `docs/reference/isa-dsl.md` 新增「`[conventions.mem]` — 内存操作数的文本形态」小节；schema 表 / 签入的 `isa-dsl.schema.json` / 键总览速查表三处同步（`schema_guard` 钉住）。
+ x86 谱据此声明两条模板（`{size}[{base}+{index}*{scale}+{disp}]`、`{size}{disp}[{base}]`）与五个尺寸关键字（`byte/word/dword/qword/xmmword ptr`）：`mov QWORD PTR [RSP - 16], RAX` 这类写法开始能解析。

### Added (2026-10-03) — x86 内存形式的 ALU 族（`*_MR` / `*_R_MEM` 各 8 条）

与寄存器版**共用操作码**，区别只在 ModRM 的 mod（=11 走寄存器形式、≠11 走内存）——`ADD_RM_R`(0x01) 这类只覆盖前者，现在补上后者：目的在内存的 `ADD/OR/ADC/SBB/AND/SUB/XOR/CMP_MR`（0x01/09/11/19/21/29/31/39）与源在内存的 `*_R_MEM`（0x03/0B/13/1B/23/2B/33/3B），一条 `[[templates]]` 各 8 行；新编码形式 `MRR_MEMREF_AUTO`（`opsize = "s0"`：内存操作数自己不携带宽度，宽度由旁边的寄存器驱动）。

全部**不写 `ref`** ⇒ IR 降级不发射它们（目前只有汇编器入口需要），候选集不变、字节与运行期行为逐字节不动（JIT 矩阵 x86 197/3/0、riscv64 136/64/0 与改动前逐条一致）。效果：`gnu-gas-intel` 解析档 `parsed` 0 → **16**、`llvm-mc` 31 → **32**；x86 编码对拍 26 → **27 条逐字节对上、0 条差异**。

### Fixed (2026-10-03) — 生成期自测的文本歧义键漏了「打印出来是什么」

`text_key` 原先按**操作数声明序 + 带操作数序号**建键，两类指令因此被判错：① 只有占位符顺序相反（`add [mem], r` 是 `add {1}, {0}`、`add r, [mem]` 是 `add {0}, {1}`）——文本明显不同却被当同形，白白放弃强断言（本轮的 16 条新 ALU 内存形式全被误报）；② 序号不同但**渲染文本确实一样**（`MOV_R_RM` 目的在 reg 字段、`MOV64_RR` 目的在 rm 字段，都打印 `mov A, B`）——真歧义却被拆开，强断言落到文本分不清的指令上而变红（`MOV_R8_RM64[hi]`）。

现在键 = 字面段序列 + **按模板占位符序、不带序号**的「打印描述」：前者恢复强断言（`disasm → asm → encode` 必须回到同一字节），后者正确留在歧义名单里。x86 文本歧义名单 43 → **37**（`MOV64_MR`/`MOV64_RM`/`MOVSD_MR`/`MOVSD_RM`/`MOVUPS_MR`/`MOVUPS_RM` 实测全部通过强断言），riscv64 / arm64 名单不变。

### Fixed (2026-10-03) — 解码侧「无基址」内存形态少读 4 字节 disp32

`mod=00` + `SIB.base=101` 是 x86 的**无基址**形态（后面跟着 disp32）。解码器原先只按 SIB 取基址、**不读那 4 字节**，于是返回一个假的 `[rbp+disp]` 并少消费字节。`MemRef.base` 是必需字段、表示不了这种形态 ⇒ 现在 fail-closed 拒绝。缺陷一直在，只是没有指令占那条 opcode（0x2B）时 `decode` 直接 `Err`、看不见；补完 ALU 内存族后 `encoder_fuzz_tests` 的 `sub rax, [0x12345678]` 当场抓到。

### Added (2026-10-03) — `[reg.*].aliases`：寄存器别名（解析认、渲染出主名）

真实语料里 riscv 几乎全用 ABI 名（`lwu sp, 4(gp)`、`addw a2, a3, a4`），而我们反汇编/黄金输出
一直用 `x` 名。以前没有别名的位置，只能"再声明一份寄存器组"或改 asm 模板拼名字——都是冗余。
现在 `[reg.<name>]` 多一个**别名表**：

+ `aliases = { a0 = 10, fp = 8, s0 = 8 }`——`别名 = 组内下标`，与主名**位置无关**，
  同一寄存器允许多个别名（riscv `x8` 既是 `s0` 也是 `fp`）；
+ **解析认别名**（`Reg::from_str`，随 `[meta].case_insensitive_regs` 折叠大小写），
  **渲染仍用主名**（反汇编输出 `x10`，不是 `a0`）；
+ 校验在 `validate_regs`：下标越界 / 空名 / 与本组主名撞车 / **跨组重名**都报错
  （跨组重名会让 `Reg::from_str` 静默取第一个）；三方守卫照旧（`schema.rs` ↔ `model.rs` ↔
  `docs/reference/isa-dsl.md` 键表 + `isa-dsl.schema.json`）。

riscv64 谱据此补上 32 个 ABI 别名（`zero`/`ra`/`sp`/`gp`/`tp`/`t0`–`t6`/`s0`–`s11`/`a0`–`a7`/`fp`）：
解析档 `parsed` 12 → **72**，编码对拍 12 → **72 条逐字节全等**。

### Fixed (2026-10-03) — `lui`/`auipc` 的立即数口径改成 GAS/LLVM 的字段值（破坏性，无兼容层）

`imm20` 原先是**预移位**位域（`pieces = [{ offset = 12, width = 20, shift = 12 }]`），
即"立即数写绝对值、编码器负责 `>>12`"。于是 `lui a0, 2` 在我们这里编成 imm20=0
（上游/规范是 `37 25 00 00` → rd = 8192），`lui t0, 1048575` 更是被截成 255——
**riscv 语料的字节对拍一接通就抓到 7 条**。现在按标准口径改（`shift = 0`，
**立即数就是 imm20 字段值**）：

+ `codegen/placeholder.rs` 的五个 `*_hi20` 占位符（`{iconst_hi20}`、`{iconst_hi32_hi20}`、
  `{iconst_lo32_hi20}`、`{fconst_hi32_hi20}`、`{fconst_lo32_hi20}`）改为
  `((v + 0x800) >> 12) & 0xFFFFF`——原来由编码器的 `shift` 干的活挪到这里，
  **低位进位/掩码语义逐字节不变**（掩码还顺带把 32 位常量的回绕写清楚）；
+ riscv64 谱：`imm20` 的 `shift = 12` → `0`（于是 `Lui` 的立即数**重新受 0..1048575 值域检查**，
  以前因为"预移位"整条豁免）；`li` 伪指令的 `lui` 行补 `>> 12`；
  lowering 里的字面 LUI 立即数（`0x55555000`/`0x33333000`/`0xf0f1000`/`0x0ff0000`/`0x80000`/`0x10000`）
  改成字段值（`0x55555`/`0x33333`/`0xf0f1`/`0xff0`/`0x80`/`0x10`，行尾注释留绝对值）；
  谱内 3 条 `lui X1, 4096` 向量改成 `lui X1, 1`（字节不变）；
+ **riscv64 矩阵 136 passed / 64 skipped / 0 failed 不变**（编码字节与运行时行为逐条保持），
  riscv64 编码对拍 **7 条差异 → 0**。

### Added (2026-10-03) — riscv64 补 RV64I 的 `lwu` / `addiw`

两条纯 ISA 补全（`only_variants = { xlen = [64] }`，不参与 lowering）：解析档 riscv 的
`NoPrefix` 里少了它们，`insts` 也从 117 → **119** 条。守卫同步：`spec_coverage_guard`
（riscv 119）、`isa_roundtrip_guard`（riscv 派生条目 329 → **341**）、`variants.rs`
（RV32 投影丢 14 条、默认档 119）、`forge-isa` CLI 的 `insts` 计数。

### Fixed (2026-10-03) — 字节 oracle 抓到的 x86 缺陷：ALU 立即数短形式（`83 /n ib`）与 16/32 位目的地

补上真实语料当天，编码对拍档就报出 5 条差异 + 一批"解析不动"，全部是**谱缺形状**而不是语料问题：

+ **`83 /n ib`（符号扩展 imm8）整族缺失**：`xor rax, 12` 以前编成 7 字节的 imm32 形式
  （`48 81 f0 0c 00 00 00`），规范/上游用 4 字节的 `48 83 f0 0c`；更糟的是**解码器也不认**
  `48 83 /6 ib`（往返守卫注释里的"别名撞车"掩盖不了这一点）。现在补 `ADD/OR/AND/SUB/XOR/CMP`
  的 imm8s 形式 + `ADC/SBB` 的 reg-reg 与 imm 形式（共 16 条），并新增槽
  `imm8s`（**有符号 8 位**——CPU 会符号扩展到操作数宽度，与给位域用的无符号 `imm8` 不能混用）。
+ **16/32 位目的地**：`xor eax, 12` / `add ax, -12` 完全不解析（原先的 imm32/imm8 形式都只挂
  `dst:gpr` = 64 位）。imm32 形式的目的/源槽改成多宽度 `gprx`，于是 `66`/无前缀/REX.W
  按操作数宽度自动选。
+ **同文本两条编码的分派**：imm8s 与 imm32 汇编文本相同，两处得改——
  ① `type_signature`（asm 扫描的去重键）带上立即数**值域**，否则窄值域那条会把宽值域那条
  去重掉、放不进 imm8 的立即数就没候选了；② `form_specificity`（候选排序）也把立即数计入
  "能接受多少输入"，否则 `ADD64_R_IMM32`（单类 `gpr`）会排在 `ADD_R_IMM8S`（多类）前面，
  `add rax, -12` 又被编成 imm32（这正是修完第一版后**剩下的那一条**差异，靠字节 oracle 的
  逐条对拍 + `FORGE_ASM_ENCODING_CASES=1` 才定位到）。
+ **顺带删掉两条重复指令**：`ADD64_R_IMM32` / `SUB64_R_IMM32` 与多宽度的 `ADD_R_IMM32` /
  `SUB_R_IMM32` 只差 `dst:gpr`、asm 文本完全相同（前者专门承载 `frame_free`/`frame_alloc`
  角色）。角色改挂到多宽度那条上（帧调整传进来的恒是 64 位栈指针 vreg，**编码逐字节不变**，
  x86 矩阵 197/3/0 与 riscv 136/64/0 全绿可证），两条重复删除。
+ 其余补齐（都是真实语料里的常见写法）：移位立即数 `SHL/SHR/SAR_RM_ONE`（`D1 /n`，`shl edi, 1`）
  与 `*_RM_IMM8`（`C1 /n ib`）、`MOV_R_IMM32`（`B8+r id`，`mov eax, 0x1234`）、
  `RET_IMM16`/`RETF`/`RETF_IMM16`、`PUSHF`/`POPF`/`PUSHFW`/`POPFW`（16 位变体走
  `prefix = "field"` + `fields = { prefix = 0x66 }`——`opsize = 16` 对**无操作数**指令发不出
  前缀；两条 `*W` 声明在裸形式**之前**，否则 `66 9c` 会先被 `PUSHF` 解掉、往返不字节稳定）。
  新增槽 `imm16` 与 32 位目的地槽 `gpr4`。
+ **守卫同步**：`spec_coverage_guard` 指令总数 197 → **221**、文本歧义名单 +13（imm8s/imm32 成对、
  去掉被删的两条）；`isa_roundtrip_guard` 派生条目 602 → **819**；`[[vectors]]` 里 7 条 ALU
  立即数期望更新为新的规范短形式；`call_layout_emission.rs` 的帧分配断言跟着改名。
+ **实测**：编码对拍 x86 从「6 条对上 / 5 条已知差异」变成「**26 条对上 / 0 条差异**」，
  riscv64 12 条、aarch64 15 条逐字节全等不变；解析档 x86 `llvm-mc` 的 `parsed` 7 → **31**、
  GAS 那份 `no_prefix` 18 → 6（`add/adc/sbb` 等族现在认了，剩下的是 `byte ptr`/`disp[base]`
  这类写法）；`forge-codegen --lib` 1374 全绿、forge-codegen 30 个集成档全绿、
  `forge-isa-dsl` 全绿（224 + 17 档）、x86 矩阵 **197/3/0**、riscv64 矩阵 **136/64/0**
  （编译器行为不变）、`forge-abi` 51、`forge-isa` 33、`forge-tests --lib` 50。

### Added (2026-10-03) — 语料补齐：x86/aarch64 的字节 oracle 接通，编码对拍也上计数棘轮

按 `asm/PROVENANCE.md` 的清单继续补上游语料（本机 shell 无 TLS，走 `web_fetch` 一份份取回），**六份** LLVM MC 用例进仓库，全部逐字落盘并在 `PROVENANCE.md` 记了 URL/ref/许可/sha256：

+ x86：`intel-syntax-encoding.s`、`apx-rex2-format-intel.s`（Intel 语法 + `encoding:` 期望字节——x86 从此有了**字节 oracle**）；
+ aarch64：`arm64-logical-encoding.s`、`arm64-branch-encoding.s`；
+ riscv64：`rv32i-valid.s`、`rv64m-valid.s`。
+ `SUITES` 新增 `x86/llvm-mc` 一套（key = `llvm-mc`），解析档棘轮随之重刷（x86 两套：`0/18/43/3` + `7/41/63/336`）。

编码对拍档（`tests/asm_encoding.rs`）随之加固，**外部 oracle 真的开始报东西了**：

+ **期望字节的抽取支持四种上游写法**（同行尾部 / 注释自带汇编文本 / 注释在指令**后** / 注释在指令**前**）；3、4 两种**逐文件声明**（`Suite::encoding_sides`）——同一目录两种风格都有，任何"看第一条推断整篇"的启发式都会在"首条指令没有期望注释"的文件上整体错位一条（实测 `rv32i-valid.s:15` 的 `.Lpcrel_hi0: auipc …`）；前缀被拆开的期望（`acquire lock add` 的 `[0xf2]` + `[0xf0,0x48,0x01,0x00]`）与重定位形式（`[0xeb,A]`）分别"整段丢掉（计入 `dropped`）"与"占位不产 case"，都不猜。
+ **三种结果分开记**：一致（逐字节相等）、`variants`（字节不同但两边喂给**我们自己的解码器**后反汇编文本相同 = 同一指令的另一种合法编码）、`known`（字节不同且不等价）。
+ **`known` 挂进棘轮** `asm/ratchet/encoding.txt`（与解析档同一套"两个方向都红"：新增一条红、少一条也红），避免门禁永久红或静默放过。
+ 实测（2026-10-03）：riscv64 **12 条逐字节全等**、aarch64 **15 条逐字节全等**、x86 6 条对拍上；x86 的 **5 条 `known`** 是真实缺口——`xor rax, 12` 我们出 imm32 形式（7 字节 `48 81 f0 0c 00 00 00`）而上游/规范是符号扩展 imm8（4 字节 `48 83 f0 0c`），且**我们的解码器连 `48 83 /6 ib` 都解不出**；同族 `xor/or/cmp/add/adc/sbb` 的 **16/32 位目的地**形式（`xor eax, 12`、`add ax, -12`）则完全不解析（`README.md` 的「已知缺口」已逐条登记）。

### Fixed (2026-10-03) — 伪指令校验不再按"首词"猜助记符（与扫描器同口径）

`validate_pseudos`（错误码 `DSL-PSEUDO`）原来把每条指令 `asm` 的**首个空白分隔词**当成助记符集，用它查两件事：伪指令名是否与指令重名、`emit` 行首词是不是一条指令。这正是"首词 = 助记符"那套被废弃的假设，对**操作数前置**的模板（`asm = "{dst} = {src}"`）会把合法的 `emit` 行判成拼错（首词是 `{rd}` 这类占位符）。

+ 改为比**前导字面的 token**：用扫描器那份词法（`assembler::tokenize`）取每条模板第一个 `{…}` 之前的前导字面，`emit` 行同样取前导字面，**重叠部分逐 token 相容**（任一方是对方的前缀）即算"接得上"；模板以操作数开头（前导为空）不构成约束。
+ 遮蔽判定的口径同步修正：伪指令展开**先于**指令扫描，所以"重名"只发生在**模板前导字面的首个 token** 就是该名字时（多 token 前导如 `lock cmpxchg [` 只认 `lock`）；大小写按 `[meta].mnemonic_case` 走（不敏感档 `MOV` 与 `mov` 撞车）。旧错误消息"汇编器会先匹配到指令，伪指令永不生效"把顺序写反了，一并改对。
+ `emit` 行首的伪操作判定改用 `[meta].directive_prefix`（不再硬编码 `.`）。
+ 顺带把"按空白切词查助记符"的措辞从 `docs/reference/isa-dsl.md`、`docs/reference/isa-dsl-errors.md` 与 `model.rs` 的 `[[pseudo]]` 文档例里清掉；`docs/archive/**` 与 CHANGELOG 旧条目保持原样。
+ 守卫：`dsl/tests.rs::pseudo_checks_do_not_assume_first_word_is_mnemonic`（操作数前置 + 行首是参数的 emit 必须通过；多 token 前导的 `set if` 通过而 `sett` 报错；大小写不敏感/敏感两档的遮蔽判定）。实测 `forge-isa-dsl --lib` 224、三份发行谱 `validate`/`lint` 零结论、其余计数不变。

### Added (2026-10-03) — 真实汇编语料测试台（x86 / riscv64 / arm64）+ 线性扫描探针

拿**上游真实汇编语料**（GNU as / LLVM MC 的测试文件）检验生成物的汇编器：能不能读真实写法、编出的字节对不对、编出来的程序真跑不跑得起来。三档各一个测试二进制，全部在 `crates/tools/forge-tests/`：

+ `tests/asm_parse.rs`：逐行喂 `Assembler::parse_insts`，按四桶归因（`Parsed` / `NoPrefix` / `TailMismatch` / `CorpusOnly`），**门禁 = 计数棘轮** `asm/ratchet/<isa>.txt`（逐项相等，多一个少一个都红；重刷 `$env:FORGE_ASM_WRITE_RATCHET=1`）。实测（2026-10-03）x86 `0/18/43/3`、riscv64 `7/14/10/76`、aarch64 `2/0/1/17`，红桶样例写在棘轮文件里。
+ `tests/asm_encoding.rs`：把上游注释里的 `# CHECK-ASM: encoding: [0x…]` 抽成 `(指令, 上游字节)` 对拍——**外部编码 oracle**（不依赖我们自建黄金表）。riscv64 语料 31 条期望里 7 条能解析，**逐字节全等**；x86/aarch64 的语料暂未带字节期望（P2 补 NASM/XED、LLVM MC X86/AArch64），该档打印 `ASM-ENCODING-SKIP` 说明原因，不假绿。
+ `tests/asm_exec.rs`：`asm/exec/<isa>/*.s` 的**真语法叶函数小程序**汇编 → 装进 `CompiledFunction` → **真跑**（x86 原生、riscv64/aarch64 走 QEMU semihosting），断言返回值；缺通道则 `ASM-EXEC-SKIP` 说明原因。实测 `ran=8 skipped=0`（三架构都真跑通）。
+ 新增生成物公开面 **`could_be_instruction(text) -> bool`**（线性扫描探针）：只跑整模板扫描的**第一步**（首段匹配），是必要性判断（`false` ⇒ 一定不是本 ISA 的指令）。之所以要它：桶判定必须区分"上游有我们没实现的指令"与"有指令但语法对不上"，而**生成期切词得到的"助记符表"不可用**——v17 起 `asm` 模板可以操作数前置（首"词"是操作数）、首段可以是多 token 字面（`lock cmpxchg [`）、空白分词与扫描器的 token 切分也不是一回事（`amoadd.w.aqrl` 是一个 Ident）。探针与 `assemble`/`parse_insts` 共用同一份候选、同一个首段匹配器、同一个 lexer。生成器守卫 = `dsl/tests.rs::asm_scan_probe_replaces_mnemonic_table`（钉"有探针、无助记符表"）。
+ 语料 **vendor 进仓库**（`asm/parse/**` 逐字保留上游原文），出处/ref/许可/摘要见 `asm/PROVENANCE.md`（LLVM MC = `Apache-2.0 WITH LLVM-exception`，GAS = `GPL-3.0-or-later` 且独立目录）；`asm/README.md` 写清边界（只接 Intel 语法、不吃 C 预处理器的 `riscv-tests` 族、不做随机指令执行 smoke）与已跑出的真实缺口（riscv `inc`/`lwu`/`addiw` 未声明、ABI 别名寄存器不认、`%lo(2048)(x7)`；aarch64 `ret lr`；x86 `disp[base]`/`byte ptr`/无符号 `imm32`/`mov eax, imm32`）。`asm/fetch.ps1` 供有 TLS 的机器补齐全量语料（本机 `curl`/`git` 无 TLS，故 P1 只 vendored 三份小样）。
+ **验证**：`forge-isa-dsl` 223（+1 守卫）、`forge-abi` 51、`forge-isa` 33、`forge-isa-runtime` 31、`forge-tests --lib` 44、`isa-host-demo` 9+5、forge-codegen 21 个集成档计数不变（`spec_tests` 36、`demo_tests` 15、`include_tests` 7、`integration_tests` 12、`x86_tests` 5、`riscv64_tests` 7、`arm64_tests` 7、`library_surface` 3 …）；clippy `-D warnings` 0、fmt 0、markdownlint 0。

### Changed (2026-10-03) — 清除 `v12` 历史命名（破坏性，无兼容层）

文件名与标识符里的 `v12` 是 v12–v17 语法时代的残留，读起来像"当前版本是 v12"（实际现行语法
是 v18），且与 `V128`/`MOV12`/AArch64 的 `V12` 寄存器等无关名字容易混。一次清干净：

+ **ISA 谱与生成模块**：`isa/{x86,riscv64,arm64}_v12.toml` → `isa/{x86,riscv64,arm64}.toml`
  （生成模块名 = 文件 stem ⇒ `forge_codegen::{x86,riscv64,arm64}`）；`[meta].name` →
  `x86_64`/`riscv64`/`arm64`，**同步** `forge-abi/conventions/*.toml` 的 `isa =` 键与宿主
  注册表/`plan_call` 的 ISA 名键。
+ **后端包装**：`src/arch/{x86,riscv64,arm64}_v12.rs` → `arch/*.rs`（`pub use arch::*` 随之）。
+ **测试与夹具**：14 个 `tests/*_v12_tests.rs` → 去后缀；7 个 `tests/isa/*_v12.toml` → 去后缀；
  `spec_tests_v12.rs`/`v12_integration_tests.rs`/`include_v12_tests.rs` 等同步；
  `jit_matrix_*_v12` → `jit_matrix_*`（CI 命令同步）。
+ **DSL 内部**：`forge-isa-dsl/src/v12/` → `src/dsl/`（`crate::v12::` → `crate::dsl::`）；
  `V12Model` → `IsaModel`、`V12Error` → `DslError`、`V12_LOWERING_OPS` → `LOWERING_OPS`。
+ **文档**：现行文档里的路径/命令全量更新；`docs/archive/**` **保持原样**（归档内容以记录
  时点为准，`isa-dsl-v12-*` 文件名就是那段历史的标识）。
+ **验证（改名不改行为，逐数字不变）**：`forge-codegen --lib` 1285、`spec_coverage_guard`
  197/117/110、矩阵 x86 **197/3/0**、riscv **136/64/0**、`forge-isa-dsl` 222 + 集成档全绿、
  `forge-abi` 51、`forge-isa` 33、`forge-tests --lib` 44、三谱 `validate`/`lint` 零结论、
  clippy `-D warnings` 0、fmt 0、markdownlint 0。

### Changed (2026-10-03) — 搬运族派生：删掉九个手写角色，ISA 只写 `data_width`

搬运指令的四个事实里，**方向 / 寄存器族 / 立即数还是寄存器**本来就在操作数结构里
（槽角色 `out|inout|in`、槽的 `class`/`classes`、槽的 `kind`），只有**数据宽度**不在。
于是：

+ **删除** `gpr_mov` / `gpr_mov_imm` / `ret_mov` / `fpr_mov` / `vec_mov` /
  `fpr_to_gpr_mov` / `gpr_to_fpr_mov` / `wide_vec_load` / `wide_vec_store` 九个角色与
  `RoleDecl.bits`（"角色带宽度"整条机制随之消失）；相应删掉 `FpMovWidths` /
  `BankMovWidths` / `inst_by_role_for` 与三个守卫（`role_widths.rs`、
  `fpr_mov_widths.rs`、`bank_mov_roles.rs`）。
+ **新增指令键 `data_width = <位>`**（**数据宽度**，与 `width` = 指令字长是两件事）：
  写了它就是"一条搬这么多位的搬运"。选指令 = 按 **(目的形状 × 来源形状 × 位宽)** 查
  派生表（`v12/codegen/moves.rs::MoveTable`，唯一实现，生成器四处发射点与
  `abi_view`/生成物 `role_bits` 同源）：寄存器/立即数来源取**最窄覆盖者**，涉及内存取
  **精确**宽度；同形状同宽度多条候选 ⇒ **生成期报错并列出候选**；一条都没有 ⇒
  生成物里 fail-closed。三操作数形态（riscv `fsgnj.d rd, rs, rs`）的第三槽自动填成源，
  store（`mem:mem:out`）同样成立。
+ **顺带清掉两处硬编码宽度**：by-ref/sret 的宽向量栈拷贝不再按 `wide_vec_load_32/64`
  的固定 256/512 位取指令（改按运行期字节宽查表）；`f32/f64` 的 32/64 不再出现在
  生成器里。
+ **实测（逐字节/行为不变）**：x86 矩阵 **197/3/0**、riscv64 矩阵 **136/64/0**（QEMU）、
  `forge-codegen --lib` 1285、`forge-isa-dsl` 全绿（新增守卫 `tests/move_derive.rs` 6 条）、
  `forge-abi` 28 invariants、`forge-isa` 全绿、`forge-tests --lib` 44；clippy `-D warnings`
  0、fmt 0、markdownlint 0。

### Changed (2026-10-01) — lp64d 的变参改成 psABI 定本口径：未命名实参走整数寄存器，保存区紧贴入口 `sp`

RISC-V 上"变参实参只走栈"这条**已知偏差**（与 GCC/Clang 编译的变参函数互调会错）按定本
原文四件一起改完，QEMU 真执行验收：

+ **① `variadic_stack_only = false`**：未命名实参走 a0-a7（溢出才上栈）。
+ **② `variadic_classify` 按整数约定分类**：定本 `riscv-cc.adoc` 的浮点调用约定一节写着
  *"The remainder of this section applies only to **named** arguments. **Variadic arguments are
  passed according to the integer calling convention.**"* ⇒ 变参的浮点也走整数寄存器
  （`double` 的位模式进 a0-a7）。调用点为此需要**类间位搬移**（上一片已落地的
  `fpr_to_gpr_mov`/`gpr_to_fpr_mov` 角色）——这正是那条通用能力的第一位真实用户。
+ **③ 保存区只装整数参数寄存器**：`save = { int_slot = 8, float_slot = 0 }`（定本："integer
  argument registers"；浮点槽会让线性游标在"整数寄存器之后、栈实参之前"读到不该有的槽）。
+ **④ 保存区紧贴入口 `sp`、与栈实参连续**：新增形状属性 `VaSaveDecl::contiguous` ⇒ 帧布局把
  保存区放**帧顶** `[入口 sp - save_size, 入口 sp)`，ra/fp、callee-saved 的保存槽（生成物读
  `AllocResult.va_top`）与局部槽（`stack_slot_shift`）整体下移一个 `save_size`；`area` 初值 =
  保存区起点 + **已用整数寄存器数 × 槽宽**，于是单指线性游标先走完寄存器里的变参、再接着走
  栈上的变参。`contiguous = false` 时布局与历史**逐字节一致**（其余三份约定的黄金快照未变）。
+ **连带**：`forge-isa abi check` 的"游标就是栈地址却允许未命名实参进寄存器"硬错**加了例外**
  （`contiguous` 时不再矛盾），变参状态行也如实描述为"保存区（与栈实参连续）+ 线性游标"；
  `lp64d.plan.txt` 黄金重刷（`arg` 落点从栈变寄存器、`va_save` 128→64、新增
  `va_save_contiguous true`）。
+ **实测**：riscv 矩阵 **136/64/0**（`variadic_va_arg_int_only` = 47、`variadic_va_arg_int_and_float`
  = 49 真跑，走的是新的寄存器路径）、x86 矩阵 **197/3/0**、`forge-abi` 28 invariants 全绿、
  `forge-codegen --lib` 1285、`forge-isa-dsl` 全绿、`forge-isa` 全绿、`forge-tests --lib` 44；
  `clippy -D warnings` 0、`fmt --check` 0、markdownlint 0。
+ **踩到并修掉的一个真 bug**：保存区基址的 `StackAddr` 立即数第一版写成 `shift - save_size`，而
  调用方已经把 `stack_slot_shift` **加上**了 `va_top` ⇒ 地址少了 `2×save_size`；QEMU 用例当场
  读到 4 而不是 47（失败信息直接指出了症状）。修成 `shift`（`fp + shift - (shift + save_size)`）
  后两条用例全绿——这也是"每片都用真执行验收"的价值。

### Added (2026-10-01) — 变参数据面两件：`variadic_classify`（未命名实参专属分类）与"保存区只收一个类"

为 lp64d 的变参修正铺数据面（发射侧的两块能力已就位：fp 位宽表、类间位搬移）。两件都**只加
能力、不改现有行为**（四份内置约定都没用它们，黄金与矩阵逐字节不变）：

+ **`AbiRules::variadic_classify`**（新规则键）：**未命名（变参）实参**专属分类，空 = 与命名
  实参同一套。psABI 允许"变参按另一套约定传"——RISC-V 的 `riscv-cc.adoc` 在浮点调用约定一节
  写着 *"The remainder of this section applies only to named arguments. **Variadic arguments are
  passed according to the integer calling convention.**"* ⇒ LP64D 上变参的浮点也走整数寄存器。
  用一张分类表表达，而不是在引擎里给某个 ISA 开洞（与既有的 `ret_classify` 同一形状；
  `merge_parent`/`validate` 一并接上）。
+ **保存区槽宽 `0` = 该类不进保存区**（`VaSaveDecl`）：`save = { int_slot = 8, float_slot = 0 }`
  ⇒ 保存区只装整数参数寄存器。定本要求保存区装的是 *"**integer argument registers** not used
  for named arguments"*——浮点池占槽会让线性游标在"整数寄存器之后、栈实参之前"读到一批本不该
  存在的槽。
+ 守卫：`invariants::variadic_classify_switches_only_the_unnamed_arguments`（命名实参仍走
  `classify`、未命名走专属表；去掉表 ⇒ 回到普通分类——证明变化来自数据）、
  `invariants::zero_slot_width_keeps_a_class_out_of_the_save_area`（槽表与 size 逐格核对，
  并对照 `float_slot = 16` 的旧行为）。
+ 验证：`forge-abi` 27 invariants（+2）全绿、黄金快照未变；x86 矩阵 197/3/0、riscv 矩阵
  136/64/0、`forge-codegen --lib` 1285、`forge-isa-dsl` 全绿、`forge-isa` 全绿；
  `clippy -D warnings` 0、`fmt --check` 0、markdownlint 0。
+ **仍未做**（文献与顺序见 `docs/plans/varargs-plan.md` §5）：保存区放**帧顶**（与调用方栈实参
  连续，需新增一条"要求连续"的形状数据属性 + 帧布局整体下移一个 save_size）、`area` 初值改成
  "保存区起点 + 已用整数寄存器数 × 槽宽"，然后才能翻 `lp64d.variadic_stack_only = false`。

### Added (2026-10-01) — 类间位搬移角色：浮点值落在整数寄存器时不再**静默错值**

值的寄存器类与 ABI 落点的类**不同**时，旧实现只看**落点的类**决定用哪条搬移指令：浮点值落在
整数寄存器（`classify` 把浮点判给 `int` 池）时会走 `gpr_mov`，把 FPR 的号当 GPR 号用
（`Reg::from_index(10, GPR)` = `x10`）——**静默错值**。这个形状不是假想的：psABI 的**整数约定
收浮点**就是它（RISC-V 变参一律按整数约定传、Zfinx/软浮点约定），而且 lp64d 的变参修正正要
走到这里。

+ **两个新角色**（宽度写在声明里，任意 N ≥ 1，与 `fpr_mov` 同一套位宽表机制）：
  `fpr_to_gpr_mov`（FPR 的位模式 → GPR）、`gpr_to_fpr_mov`（反向）。方向在角色名里，
  字段名与 Reg 槽下标按**槽的寄存器类**取（不按操作数位置猜）。
+ **发射侧四路分派**：调用点（`arg_move_loop`）与被调方（`frame.rs` 的收参）都按
  **落点的类 × 值所在的池**分派；不一致时走类间位搬移，**缺该角色 ⇒ 生成物里明确
  `Unsupported`**（不退化、不猜）。被调方与调用点的方向**相反**（数据流向相反）。
+ **谱面申报**（指令本来就有，只加角色）：riscv `FMV_X_W`/`FMV_X_D`（f→x 32/64）、
  `FMV_W_X`/`FMV_D_X`（x→f 32/64）；x86 `MOVD_IREG_FREG`/`MOVD_FREG_IREG`（32）、
  `MOVQ_R64_XMM`/`MOVQ_XMM_R64`（64）。arm64 暂不申报（AAPCS64 不需要，缺了会 fail-closed）。
+ **验证**：新增 `forge-codegen/tests/bank_mov.rs` 用一个**合成约定**（"浮点判给 int 池"）
  走真实管线：被调方收到 `fmv.d.x`、调用点发出 `fmv.x.d`，而正常分类（lp64d）仍走同类的
  `fsgnj.d`（两路不串）；`forge-isa-dsl/tests/bank_mov_roles.rs` 钉住"发行谱申报了四个档"与
  "把角色删掉后生成物里出现 fail-closed 分支"。回归：x86 矩阵 197/3/0、riscv 136/64/0、
  `forge-codegen --lib` 1285、`forge-isa-dsl` 全绿、`clippy -D warnings` 0、`fmt --check` 0。

### Fixed (2026-10-01) — lp64d 的 `va_list` 对象布局对齐定本（`sizeof(va_list)` = 指针宽度），并纠正 §5 的修复配方

按 RISC-V psABI 定本原文重核变参一节时，发现**数据**与**下一步的做法**各错一处：

+ **数据**：`riscv_save_area` 的 `size` 由 **24 改成 8**。定本 `riscv-cc.adoc`（issue #412 之后的
  "Expand va_list description" 版本）第一句就是 *"The `va_list` type has the same representation as
  `void*`"* ⇒ LP64D 上 `sizeof(va_list)` = 8；原来的 24 是从 sysv64 抄来的余量（那 16 字节既没有
  字段也没有读者），它让 `va_copy`／把 `va_list` 交给 `vprintf` 一类**跨编译器**用法在对象布局上
  就对不上。黄金快照 `lp64d.plan.txt` 随之重刷（只动 `va_area … size=` 两行），§2 文档表同步，
  `invariants::va_shapes_match_the_documented_table`（逐格比对）与
  `va_object_layout_matches_the_psabi_numbers`（字段布局）继续钉住。
+ **做法**：`docs/plans/varargs-plan.md` §5 与 `forge-abi/src/builtin.rs` 的注释此前写着"把 lp64d
  换成 `gp_offset`/`fp_offset`/`reg_save_area`/`stack_arg_area` 四字段 + 两个游标"——那是
  **SysV AMD64 的 `va_list`**；RISC-V 的 `va_list` 就是一个 `void*`（单线性游标）。同一处还写着
  "调用方按正常分类传未命名实参"——定本的浮点调用约定一节写着 *"The remainder of this section
  applies only to **named** arguments. **Variadic arguments are passed according to the integer
  calling convention.**"* ⇒ LP64D 上**变参的浮点也走整数寄存器**（`double` 的位模式进 a0-a7），
  "正常分类"会把 `f64` 放进 `fa0`，是另一个错。两处已按定本原文改正，并把完整修复写成
  **四件一起**（`stack_only = false` / 保存区放帧顶与栈实参连续 / `area` = 保存区起点 +
  已用整数寄存器数 × 槽宽 / 变参按整数约定分类），其中第 ④ 件在发射侧还缺一块**通用**能力：
  **类间位搬移**（FPR 的位模式搬进 GPR：riscv `fmv.x.d`、x86 `MOVQ`、arm64 `FMOV`）——角色系统
  现在只有同类内的 `gpr_mov`/`fpr_mov`。
+ 守卫：`invariants::lp64d_variadic_stack_only_is_a_documented_deviation` 增加**正向**断言
  （`size`/`align` = 8、字段只有一个 = `void*`），偏差部分继续钉住并写清四件一起的条件。
+ 验证：`forge-abi` 全绿（黄金重刷后）、riscv 矩阵 **136/64/0**（未受影响）、`abi_target_real` 25、
  `forge-isa` 全绿（`abi check` 仍报 lp64d 的"可疑组合"，对象尺寸已显示 8 字节）。
+ **仍未做**（本条的真正剩余）：把 lp64d 的变参切到寄存器路径——见上面"四件一起"，其中
  类间位搬移是一块独立的通用能力。

### Changed (2026-10-01) — 浮点搬移改成**按位宽查表**：不再写死 32/64，riscv 的浮点参数/返回真跑起来

上一片留下的"钥匙"（`fpr_mov` 的三操作数形态）这一片装上了，而且**没有按 ISA 打补丁**：

+ **生成器：位宽表取代硬编码两档**。`roles = [{ role = "fpr_mov", bits = N }]`（**任意 N ≥ 1**）
  由 ISA 申报，生成器把所有声明收成一张表（`FpMovWidths`），发射点只说"这次要搬多少位"，
  由表生成 N 路分派；表里没有该位宽 ⇒ 生成物里明确 `Unsupported` 并**列出已声明档**（不猜、不回退）。
  三操作数搬移（riscv `fsgnj.d rd, rs, rs`）由表里的第三槽自动填源。删掉了
  `role_name_for(role, bits)`——它的调用点必须**写死一个宽度常量**，正是这个问题的来源。
  守卫：`crates/frontend/forge-isa-dsl/tests/fpr_mov_widths.rs`（把 x86 的两档改成 16/128 后，
  生成物里只该出现这两档、写死的 32/64 一个不留）。
+ **两处"绑定无关能力"的真开洞**（都是通用性缺陷，不是某个 ISA 的洞）：
  ① 调用点浮点实参搬运要求 `fpr_mov && vec_mov` ⇒ 没有向量寄存器的 ISA 连 `f64` 实参都发不出去；
  现在两条能力**互不依赖**，只有"按值向量"那条路会在运行期按 IR 类型 fail-closed。
  ② riscv 谱里没有 `FSGNJ_D`（只有单精度 `fsgnj.s`）⇒ f64 的搬移无处申报；已补为同一 `fpr_mov`
  的 64 位档。
+ **riscv 谱补浮点算术**：`Fadd`/`Fsub`/`Fmul`/`Fdiv`（单/双精度各一条，谱里 `FADD_S/D` 等指令早就有）。
  此前这些 op 全是**真缺口**，矩阵里所有浮点算术用例被整条 Skip 掩盖。
+ **测试通道补浮点参数执行**：`jit_matrix` 此前对 `CaseKind::F64Args` + 注入执行器直接报
  "not supported"；现在 `Executor::exec_f64_args` + riscv crt0 的 `li a0; fmv.d.x fa{i}, a0`
  （f32 用 `fmv.w.x`）让 f64/f32 参数真进 `fa0..fa3`，并新增独立冒烟用例
  `qemu_exec_f64_arg`（42.0 → 42、7.0f32 → 7）钉住这条通道。
+ **实测**：riscv 矩阵 **136 passed / 64 skipped / 0 failed**（原 133/67/0；
  `float_args_two`、`float_args_four_xmm3`、`call_float_roundtrip` 从 Skip 转**真跑**），
  x86 **197/3/0**；`forge-codegen --lib` 1285、`abi_target_real` 25（
  `riscv64_float_gap_is_engine_ok_but_emission_closed` 改名
  `riscv64_float_arg_is_open_on_both_engine_and_emission`：从"钉差异"改成"钉已闭合"）、
  `forge-isa-dsl` 全绿、`forge-isa` 24、`forge-abi` 全绿；`clippy -D warnings` 0、`fmt --check` 0。
+ 快照同步（riscv 谱的预期变更）：指令数 116 → 117、枚举器条目 327 → 329、RV32 投影指令
  104 → 105、lowering（RV32 视角）100 → 108 与默认视角 114 → 122、能力覆盖 63 → 67
  （真缺口 40 → 36）、vary 建议 24 → 28、部分重叠 23 → 27。
+ 仍未做：`lp64d` 的 `va_list` 形状切换（`stack_only = false` + 四字段 + 两条游标规则）、
  `va_meta`（SysV `%al`）、前端产出 `variadic`。

### Fixed (2026-10-01) — aapcs64 的变参数据修正：未命名实参**走寄存器**（`variadic_stack_only` → false）

上一片的"可疑组合"告警立刻见效：它把 **aapcs64** 也点了出来——`aapcs64` 与 lp64d 是**同一个模式**
（声明了寄存器保存区，却把未命名实参设成只走栈）。这一片修正它：

+ **判据（不需要定本原文）**：AAPCS64 的 `va_list` 是 `{__stack, __gr_top, __vr_top, __gr_offs,
  __vr_offs}` 的**计数式**结构——被调方把自己用过的通用/向量寄存器存进保存区，`va_arg` 按
  `__gr_offs`/`__vr_offs`（从负值数到 0）从保存区取，数到 0 再落到 `__stack`。这套结构只在
  "未命名实参**确实进寄存器**"时才讲得通；"只走栈"与保存区语义自相矛盾。
+ 改动 = **纯数据**（`builtin.rs` 一行 + 注释），黄金快照按 `FORGE_ABI_BLESS=1` 重刷；文档表
  `varargs-plan.md` §2 的 aapcs64 行从"只走栈"改为"继续用寄存器"（守卫
  `va_shapes_match_the_documented_table` 正是先报出这处不一致）。
+ **验证口径如实说明**：本机**没有 arm64 执行通道**，这次只在**计划面**验证——
  `forge-abi` 25 条 invariants（含文档表守卫）、`abi_target_real` 25（含
  `aapcs64_variadic_shape_is_pure_data`，其断言同步改成"未命名实参走寄存器"）、`forge-isa` 24、
  `forge-isa-dsl` 全绿；x86 矩阵 197/3/0、riscv 矩阵 133/67/0（未受影响）；`clippy -D warnings` 0、
  `fmt --check` 0。**这不是执行验证**，写进方案 §5。
+ 收益：`abi check` 的"可疑组合"告警此后只剩 lp64d 一处（它的修正需要先把生成器的三操作数 fp
  搬移泛化，见下一条），aapcs64 的数据先与 psABI 模型对齐。

### Added (2026-10-01) — `abi check` 报出"可疑组合"：声明了保存区却让未命名实参只走栈（数据驱动的一致性检查）

lp64d 的 psABI 偏差（未命名实参应走寄存器）此前只写在文档与守卫里，用户在 `forge-isa abi check`
里看不到。这一片把它变成**工具可见的一致性检查**——而且**不点名任何约定**，按形状数据判：

+ 判据：`va_list` 形状**声明了寄存器保存区**（`save.is_some()`）却把 `variadic_stack_only` 设成
  **true**。保存区的存在本身就意味着"未命名实参可以进寄存器、由被调方存下来"（AAPCS64 的
  `__gr_offs`/`__vr_offs`、RISC-V 的 vararg save area 都是这个模型）⇒ 这个组合**自洽但可能不符合
  psABI 定本**：自己的调用方/被调方按同一份约定对齐能跑，与外部编译器互调会错。
+ 落点：`ℹ 变参 …` 那一行追加 `⚠ …` 提示（**不改退出码**——它是"待核"而不是"这台机器做不了"；
  `--strict` 目前不覆盖它，如实写在消息里）。
+ 收益：`abi check riscv64.toml` 现在会提示 lp64d 的组合可疑；**aapcs64 同组合也一并被点名**
  （此前没人注意它和 lp64d 是同一个模式）。
+ 守卫：`cli_tests::abi_check_reports_the_variadic_state` 增加断言（riscv 输出必须含
  "可能不符合 psABI 定本"）。
+ 验证：`forge-isa` 9 + 24 全绿；`clippy -D warnings` 0、`fmt --check` 0。

### Changed (2026-10-01) — 补强证据：riscv 缺的 `fpr_mov` 是"浮点全面落地"的同一把钥匙（实验后回退）

接着上一片的 lp64d psABI 议题往下验证：把 riscv 谱里**早就有指令、却一直没有 lowering** 的三条
浮点算术（`Fadd`/`Fsub`/`Fmul`，各单/双精度两条）补上，看矩阵有什么变化——结果暴露了**同一个**
根因：

+ 补上浮点算术后，原先被"缺 `Fadd`"掩盖的三条浮点用例立刻从 **Skip 变成 Fail**，且原因全指向
  `fpr_mov`：`float_args_two` / `float_args_four_xmm3`（`v12 float args (MOVSD/MOVSS missing)`）、
  `call_float_roundtrip`（`v12 float return: 未声明 roles = ["fpr_mov_f64"]/["fpr_mov_f32"]`）。
+ 也就是说 **`fpr_mov`（含 RISC-V 的三操作数 `fsgnj.d rd, rs, rs` 形态）是 riscv 浮点落地的
  同一把钥匙**：补它一次 → 同时解掉① 浮点变参（lp64d 与 psABI 一致的前提）、② 浮点参数/返回、
  ③ 上面三条用例的真跑。而"只补浮点算术"会把 Skip 变成 Fail（更差），所以这次实验**已回退**
  （riscv 矩阵仍是 **133/67/0**），把结论与顺序写进 `docs/plans/varargs-plan.md` §5。
+ 验证（回退后）：riscv 矩阵 133/67/0、x86 197/3/0、`forge-abi` 全绿、markdownlint 0。

### Changed (2026-10-01) — lp64d 的 psABI 修正**试做后按实测回退**：整数支能过，浮点支卡在"三操作数 fp 搬移"

上一片核出 lp64d 与 RISC-V psABI 定本的偏差（未命名实参应走寄存器）。这一片动手改，并用 QEMU 矩阵
逐步验证，结论是**必须按顺序做、现在停在第 ② 步之前**：

+ **试做的改法**（**不需要**动帧布局）：把 lp64d 换成"两个游标（`gp_offset`/`fp_offset`）+ 保存区
  指针 + 栈实参指针"的形状 + `variadic_stack_only = false`。理由是定本要求"保存区与栈实参连续"
  是为了让**单个线性游标**能从保存区走进栈实参；我们的 `va_list` 自带两个指针，因此不必连续。
+ **实测 ①（整数支）**：`variadic_va_arg_int_only` 在 QEMU 上**通过**——未命名实参进 `a1`/`a2`，
  被调方 spill 进保存区、`va_arg` 按 `gp_offset` 读回 ⇒ 数据驱动的模型**能**表达定本语义。
+ **实测 ②（浮点支）**：`variadic_va_arg_int_and_float` **编译失败**——
  `v12 call: 浮点/向量实参搬运缺 MOVSS/MOVSD/MOVAPS 角色`：riscv 谱里没有 f64 的 fp→fp 搬移
  （只有 `FSGNJ_S`，没有 `FSGNJ_D`），且规范化的写法是三操作数 `fsgnj.d rd, rs, rs`；生成器的
  `fpr_mov` 路径假设两操作数（8 处发射点都在构造 `Inst::<mov>{ dst, src }`）⇒ 直接给三操作数
  指令申报角色**连生成物都编不过**。
+ **因此回退**（保持绿）：只做形状切换会让 riscv 上任何"传浮点变参"的调用点从"自洽能跑"退化成
  **编译错误**，比现状更糟。修复顺序写进方案：① 补 `FSGNJ_D` + 角色；② 生成器的 `fpr_mov`
  发射**按形状**泛化（三操作数时 `src2` 也填源寄存器，与 `StackMemShape` 同一套做法）；
  ③ 再改 lp64d 的形状。
+ **验证**：回退后 `forge-abi` 7 个 target 全绿（25 条 invariants，含钉住偏差的那条）、
  `clippy -D warnings` 0、`fmt --check` 0、markdownlint 0；riscv 矩阵仍是 **133/67/0**。

### Fixed (2026-10-01) — riscv 浮点支打通：`Fload`/`Fstore` lowering + `FSD` 的浮点栈参数角色（lp64d 变参两条用例都真跑）

上一片让 lp64d 的**整数**变参在 QEMU 上真跑；这一片把**浮点**支也接通——发现两处与变参无关的
谱面缺口：

+ **riscv 谱有 `FLD`/`FSD` 指令，却没有任何 `Fload`/`Fstore` 的 `[[lowering]]`** ⇒ `va_arg(f64)`
  展开出的 `Fload` 编不出来（矩阵报 `Unsupported("v12 lowering")`）。补上单/双精度各一条
  （`FLW`/`FLD`、`FSW`/`FSD`）。
+ **浮点栈实参写不出去**：调用方写传出区用 `stack_arg_store`，此前只给整数 `SD` 申报了角色；
  给 `FSD` 补 `{ role = "stack_arg_store", class = "fpr" }`（S 形式，生成器按形状填 sp + 偏移）。
+ **矩阵用例的 `ops` 多写了一个 `Fadd`**（用例本身不做浮点算术：只在调用点传 `2.5`、被调方
  `va_arg(f64)` 后 `fptosi`）——riscv 谱没有 `Fadd`，这条**用不到的**能力把用例误判成"能力不足"
  而 Skip；删掉它，用例两台机器都真跑。
+ **验收**：riscv QEMU 矩阵 **133/67/0**（`variadic_va_arg_int_only` = 47、
  `variadic_va_arg_int_and_float` = 49，**两条都真跑、无 variadic Skip**），x86 **197/3/0**。
+ **同步的快照**（补 lowering 改变了"能力缺口"口径，按纪律人工复核后同步）：
  `lint_shipped::op_gap_matches_section_10_3` riscv 覆盖 61 → 63、真缺口 42 → 40；
  `lint_shipped::vary_candidate_inventory` riscv 22 → 24（新增的两条同形状规则是"可合并"建议，
  不强行 `vary`）。
+ **验证**：`forge-isa-dsl` 全绿（222 + 各守卫）、`forge-tests` lib 43、矩阵两套、`forge-codegen`
  lib 1362 + `riscv64_tests` 7 + `riscv64_tm_tests` 5 + `integration_tests` 12、
  `forge-abi`、`forge-ir` 281、`forge-isa` 24；`clippy -D warnings` 0、`fmt --check` 0。

### Changed (2026-10-01) — aapcs64 的缺口量到位：arm64 后端**只有 8 个 op 的 lowering**（守卫钉住覆盖度）

上一片说"aapcs64 被 arm64 缺的 lowering 挡住"，这一片把"缺多少"量清楚并钉住——结论是**这不是
变参的问题，是 arm64 后端还没成型**：

+ 实测：arm64 谱的 `[[lowering]]` 只覆盖 **8 个 op**（`Band`/`Bor`/`Bxor`/`Copy`/`Iadd`/`Iconst`/
  `Imul`/`Isub`，全是算术）；变参展开要用的 `StackAddr`/`Store`/`Load`/`Icmp`/`Select`/`Sextend`/
  `Ireduce` **一个都没有**（x86/riscv 有 ⇒ win64/sysv64/lp64d 真跑）。
+ 守卫 `arm64_tm_tests::tm_aapcs64_varargs_are_blocked_by_missing_arm64_lowering` 现在**两件事一起钉**：
  ① 变参函数编到 arm64 必须是**明确 Unsupported**（不许静默编错）；② `arm64::SUPPORTED_OPS`
  里**没有**这些 op——arm64 补上任一条时守卫会红，提醒把 aapcs64 从"数据面"升级成"编得出 + 真跑"。
+ 方案文档（`docs/plans/varargs-plan.md` §5）据此把 aapcs64 一条改成"先补 arm64 后端（独立且大得多的
  工作）+ 本机无 arm64 执行通道"，不再把它算作变参侧的缺口。
+ 验证：`arm64_tm_tests` 7、`forge-codegen` lib 1362；`clippy -D warnings` 0、`fmt --check` 0。

### Fixed (2026-10-01) — riscv 栈参数打通：访存指令按**形状**取 + 修掉 `lp64d` 的 `first_offset_slots`（变参在 QEMU 上真跑）

上一片把 lp64d 的缺口钉在"调用方写不出传出区"。这一片补齐，顺带挖出一个与变参无关的真 bug：

+ **栈参数访存指令按形状取，不按 ISA 取**（新 `StackMemShape`，生成器 + `move_args` 共用一份
  `stack_mem_shape` 判据）：`Reg+Mem`（x86 `MOV64_MR`/`MOV64_RM`，Mem 槽填
  `MemRef{base, disp}`）或 **`值Reg+基址Reg+位移Imm`**（RISC-V S 形式 `SD {src}, {imm}({src2})`——
  没有 Mem 槽的定宽 ISA 用这种，基址填机器的帧/栈基址、位移填偏移）。riscv 的 `SD` 因此可以申报
  `roles = ["stack_arg_store"]`；`move_args` 的保存区 spill / 收参路径一并按形状发。
+ **真 bug（被这条新路径暴露）**：`lp64d` 的 `first_offset_slots` 写的是 **2**（照搬 x86 的"序言
  总是 push fp"），而 RISC-V 的帧基址 `X8` = **入口 sp**（不 push 返回地址）⇒ 被调方的 `va_list`
  比调用方写的槽**高 16 字节**。实测：QEMU 通道的 `variadic_va_arg_int_only` 读到 **0**（应 47）；
  改成 **0** 后对齐（`lp64d.plan.txt` 黄金快照 `first_arg_off` 16 → 0，`FORGE_ABI_BLESS=1` 重刷）。
  **影响面**：riscv 上任何栈实参（第 9+ 个命名实参、变参未命名实参）此前都按错位置取——没有用例
  走到过。
+ **矩阵**：`variadic_va_arg_int_only` 现在是**两台机器真跑**（不再需要伪能力门控 `va_stack_args`，
  该门控与其注释一并删除）：x86 **197/3/0**、riscv **132/68/0**（浮点那条因 riscv 谱缺 `Fadd`
  仍按能力门控 Skip）。
+ **验证**：`forge-isa-dsl` 全绿（222 + 各守卫）、`forge-codegen` lib 1362 + `abi_target_real` 25 +
  `arm64_tm_tests` 7 + `riscv64_tm_tests` 5、`forge-tests` lib 43、`forge-abi`、
  `forge-ir` 281、`forge-isa` 全绿；`clippy -D warnings` 0、`fmt --check` 0。

### Added (2026-10-01) — 变参：aapcs64/lp64d 的两条**确定性守卫** + 缺口精确到"差什么、怎么接"

V6 之后四份约定的形状/规则都是数据，但 **aapcs64/lp64d 还没有真跑**。这一片把"差什么"实测清楚并
用守卫钉住，避免后来的人以为它们已经能用：

+ **lp64d 的被调方编得出**（新守卫 `riscv64_tm_tests::tm_compiles_riscv_varargs_callee_side`）：
  `va_start` + 两次 `va_arg(i64)` 在真 riscv 后端上走完 lowering → regalloc → frame → encode，
  且帧 ≥ 保存区 128 字节。⇒ **riscv 只差调用方一侧**。
+ **lp64d 的调用方接不上（缺口修好前不许静默）**：写传出区要 `stack_arg_store` 角色，而生成器要求
  它是 **Reg+Mem 形状**；riscv 谱的 `SD` 是 `base+disp` 模板形状、`stack_arg_load`/`stack_arg_store`
  一个都没有 ⇒ 实测 `v12 call: 本 ISA 缺 roles = ["stack_arg_store"] 的指令（栈参数写不出去）`。
  **与变参无关**（riscv 上第 9+ 个命名栈实参同样走不到）；矩阵按伪能力 `va_stack_args` 门控 Skip。
+ **aapcs64 的数据面逐格钉住**（新守卫 `abi_target_real::aapcs64_variadic_shape_is_pure_data`）：
  5 个字段（`__stack`/`__gr_top`/`__vr_top`/`__gr_offs`/`__vr_offs`）、取参规则（**有符号**计数、
  基准 = 区域顶端、溢出 = `__stack`、步长 8/16）、逐字段初值（`FrameOff(16)` / `SaveOff(64)` /
  `SaveOff(192)` / `Imm(−8)` / `Imm(0)`）全部逐格断言。
+ **aapcs64 的编译面被 arm64 缺的 lowering 挡住**（新守卫
  `arm64_tm_tests::tm_aapcs64_varargs_are_blocked_by_missing_arm64_lowering`）：实测报
  `Unsupported("v12 lowering")`——变参展开用的 IR 词汇（`Icmp`/`Select`/`StackAddr`/`Sextend`/
  `Ireduce`）在 arm64 谱里一条 lowering 都没有（x86/riscv 有）。守卫钉"必须是明确 Unsupported"，
  补齐时会红，提醒改成"编得出 + 真跑"。
+ **验证**：`forge-codegen` lib 1362、`abi_target_real` 25、`arm64_tm_tests` 7、
  `riscv64_tm_tests` 5、矩阵 x86 197/3/0 与 riscv 131/69/0、`forge-tests` lib 43；
  `clippy -D warnings` 0、`fmt --check` 0、markdownlint 0。

### Changed (2026-10-01) — 变参 V6：`va_list` 形状与取参规则做成**约定数据**，管线只剩一套算法（破坏性）

前一片之后管线里还剩两处"为某个 ABI 开洞"：`va_expand` 按 `(有没有保存区, 字段数)` 判"族"
（4 字段 = SysV 一族、1 字段 = win64 一族），宿主 `abi_target` 按**字段名**（`fields[0].name ==
"cursor"`）认形态。这一片把形状整体数据化，两处判定一起消失：

+ **`VaListKind` 枚举删除**（破坏性）：`hidden.va_list` 现在是**形状数据**——预置名
  （`"win64_stack"`/`"sysv_reg_save"`/`"aapcs64_struct"`/`"riscv_save_area"`）**或**一张显式形状表
  `{ size, align, fields, save, int_arg, float_arg }`。加约定/加机器 = 加数据，不改代码；
  `va_list_size`/`va_list_align` 键并入形状（写了会报未知键）。引擎侧 `va_area_from_shape` 只做
  解析：字段**名**→下标、上限/步长/零点由保存区槽表推（显式键优先），形状写错就明确报错。
+ **取参规则 = 形状的一部分**（`int_arg`/`float_arg`）：`cursor`（游标字段）/`base`（偏移基准，
  缺省 = 游标即地址）/`overflow`（溢出区，缺省 = 没有溢出支）/`signed_limit`/`limit`/`step`/
  `overflow_step`。解析结果进 `AbiPlan::va_area.arg_rules`，黄金快照新增 `va_rule` 行钉住
  （四份内置约定：SysV = 无符号偏移游标 + 溢出区、Win64 = 地址式单游标、AAPCS64 = 有符号计数
  （基准 = 区域顶端）+ 溢出区、LP64D = 地址式游标 + 保存区）。
+ **管线只剩一套算法**（`pipeline/va_expand.rs`）：读游标 →（有基址就加基址）→（有溢出区就按
  上限 select、两条游标各自推进；没有就是直线）→ 取值（`f32` 先取 `f64` 再 `Fptrunc`）→ 写回。
  "族"的判定与按字段名认形态的代码都删了。
+ **生成器的变参臂删除**：`gen_va_start_lowering`/`gen_va_arg_lowering` 不再进生成物；
  谱里也不再申报 `ptr_load`/`ptr_store`/`add_imm`/`fpr_narrow` 四个角色（`Role` 变体一并删）。
  变参对 ISA 的**要求降到零**（只用既有 IR op 的降级）——win64 因此也走 IR 展开，三条原本跑
  生成器臂的用例（含 `f32` 默认提升）在无生成器臂的情况下继续真跑绿。
+ **`abi check` 的变参一行改成按形状数据说**（形状名 + 是"栈式游标"还是"保存区 + 溢出区" +
  字段表），"自相矛盾"硬错的判据也数据化（游标即地址 ⇒ 必须 `variadic_stack_only`）。
+ **矩阵：`VaStart`/`VaArg` 升为"宿主管线 op"**（`Capabilities::HOST_PIPELINE_OPS`，每台机器无条件
  支持）——不再由各 runner 的 `CAPS_EXTRA` 逐台声明（那等于把"变参能不能跑"绑回 ISA）。新增
  架构无关用例 `variadic_va_arg_int_only`（`4*10+7 = 47`）：x86 真跑（矩阵 **197/3/0**），
  riscv 因**调用方一侧**的已知缺口 Skip——实测报 `v12 call: 本 ISA 缺 roles =
  ["stack_arg_store"] 的指令`：生成器要求该角色是 **Reg+Mem 形状**，而 riscv 谱的 `SD` 是
  `base+disp` 模板形状（与变参无关的通用缺口：riscv 上第 9+ 个命名栈实参同样走不到）。门控用
  伪能力 `va_stack_args`，缺口与证据记进方案。
+ **验证**：`forge-codegen` 1362（8 条变参 JIT 全绿）+ `abi_target_real` 24、`forge-tests`
  矩阵 x86 **197/3/0** 与 riscv **131/69/0**（新用例的门控 Skip 可见）、lib 43、`forge-abi`、
  `forge-ir` 281、`forge-isa-dsl` 222、`forge-isa`（含 `abi check` 端到端）全绿；
  `clippy --workspace --exclude forge-rustc --all-targets --all-features -j 1 -- -D warnings` 0、
  `cargo fmt --all -- --check` 0、markdownlint 0。黄金快照按 `FORGE_ABI_BLESS=1` 重刷
  （`va_area` 行记形状名 + 新增 `va_rule` 行）。

### Changed (2026-10-01) — 前端局部槽深度扫描抽成唯一实现（`pipeline/frame_slots.rs`）

变参的 ABI 槽（寄存器保存区 + `va_list` 对象）必须排在前端局部槽**之下**，而"前端局部槽占多深"
在两个地方需要同一个答案：`pipeline/lowering.rs`（给 `Alloca` 分槽、算帧尺寸）与
`pipeline/va_expand.rs`（展开前算 ABI 槽起点）。上一片在展开侧照着 lowering 的算式**镜像了一份**
——两份实现要对齐就等于迟早漂移，且漂移的后果是静默错值（保存区被局部变量覆盖）。这一片抽掉镜像：

+ **唯一实现** `pipeline/frame_slots.rs::scan_frontend_slots(func, slot_bytes, size_of)`：一次扫完
  三处统计（`StackAddr` 的立即数偏移、`stack_addr(0)+iadd(iconst(-N))` 模式、`Alloca` 区），
  返回 `FrontendSlots { stackaddr_depth, allocas }`；`depth_bytes(slot_bytes)` 给出"这些槽覆盖到的最深
  字节"。`lowering` 与 `va_expand` 都调它，`size_of`（类型尺寸来源）由调用方给——lowering 用类型快照、
  展开用函数自己的 `types`。
+ **行为不变（逐处核对）**：立即数那部分 lowering 仍在逐指令通路里并进 `max_stack_bytes`（与预扫描
  结果取 max，幂等）；`Iadd` 模式那部分只有预扫描能看到，继续在这里并进去；`Alloca` 区仍按"StackAddr
  区底 + 一槽"往下排，槽偏移的分配留在 lowering（展开只需要深度，不需要具体槽位）。
+ **顺手删掉死代码**：`FrontendSlots::alloca_base` 抽出后无人调用（lowering 自己算 `slot` 起点）⇒ 删。
+ 验证：`forge-codegen`（1362，含 8 条变参用例 + `va_expand::tests::abi_slots_sit_below_frontend_locals`）、
  `forge-tests`（矩阵 196/3/0，lib 43）、`forge-abi`、`forge-ir`（281）、`forge-isa-dsl` 全绿；
  `clippy --workspace --exclude forge-rustc --all-targets --all-features -j 1 -- -D warnings` 0、
  `cargo fmt --all -- --check` 0。

### Changed (2026-10-01) — 变参：`va_start` 物化也搬进 IR 展开，拆掉两处 x86 专属（打包 + `gpr_imm`）

评审指出上一片"为某一个开洞"：保存区物化留在生成器里，于是有两处只为 x86 成立的东西——
① `VaInit.offsets` 把 `gp_offset|fp_offset` **打包**成一个 u64（因为生成器只有 8 字节的帧相对
store），② 为写常量而新增的角色 `gpr_imm`。这一片把物化也搬进 IR 展开，两者都消失：

+ **对象初值改成"逐字段的数据"**：`VaInit.fields: Vec<VaInitVal>`（与 `va.fields` 同序），值有三种
  来源——`Imm`（常量，按**字段自己的宽度**截断后写）/ `FrameOff`（帧内地址）/ `SaveOff`（保存区
  基址）。IR 展开按 `va.fields` 的偏移与宽度逐字段 `Store` ⇒ **不再需要打包、也不需要立即数
  能力**（常量用 `Iconst`、地址用现成的 `StackAddr`）；`gpr_imm` 角色与 `MOV_REG_IMM64` 上的声明
  一并删除。
+ **修掉一个真缺陷（同一片里发现的）**：ABI 槽（保存区 + 对象）原先在**编译入口**按"当时的
  `max_stack_bytes`"（= 0）预留，而前端 `stack_addr(-N)` 要到 lowering 才统计 ⇒ **两者会重叠**
  （sysv64 那条用例恰好没有局部槽，所以没暴露）。现在 `plan_abi_slots` 在**展开前**扫一遍 IR 算
  前端局部槽深度（三处统计：`StackAddr` 立即数、`stack_addr(0)+iadd(iconst(-N))` 模式、`Alloca` 区；
  这一片先按 lowering 的算式镜像了一份，随即抽成**唯一实现** `pipeline/frame_slots.rs`——见下一条），
  ABI 槽从它之后排，并把结果喂给 `CompileState`（帧尺寸/序言偏移）。
+ **守卫（确定性、已验证会咬）**：`va_expand::tests::abi_slots_sit_below_frontend_locals`——直接断言
  "保存区/对象排在前端局部槽之下"；把深度扫描短路后它报
  `保存区必须在前端局部槽之下（front=32, save_depth=176）`，复原后通过。
  另加一条 JIT 用例 `test_jit_va_arg_sysv64_coexists_with_frontend_locals`（变参 + 前端局部槽一起用）。
  **诚实说明**：这条 JIT 用例**不能**替代上面的单测——实测把深度短路后它仍然绿（局部槽落在保存区
  的 XMM 段，而用例读的是 GPR 参数），所以放置正确性由那条单测钉住。
+ 验证：`forge-codegen`（1362，含 8 条变参用例）、`forge-abi`、`forge-tests`（矩阵 196/3/0）、
  `forge-ir`、`forge-isa-dsl`、`forge-isa` 全绿；`clippy -D warnings` 0、`cargo fmt --check` 0。

### Added (2026-10-01) — 变参 V3 第四片：保存区形态的 `va_arg` 走 IR 展开（sysv64 两条支真跑）

保存区形态的取值是**有条件**的（游标未超上限取保存区、否则取溢出区，各自推进），这一片把它做成
**管线里的 IR 展开**（`pipeline/va_expand.rs`），而不是生成器序列：

+ **为什么在 IR 层**：条件取值在 IR 只需 `Icmp`/`Select`/`Iadd`/`Uextend`/`Ireduce`/`Load`/`Store`/
  `Fload`/`Fptrunc`——**全都有现成降级**；生成器要发同样的事得新增 `cmp`/`cmov`/`add_rr`/`and`
  四个能力角色，且别的 ISA 还得各来一遍。
+ **布局与宽度一律来自数据（评审点，已按此实现）**：字段偏移与宽度取自 plan 的 `va.fields`
  （**不再写死 sysv64 的 0/4/8/16**），游标按**它自己的宽度**读写（`u32` 就 4 字节、`u64` 就 8 字节），
  槽宽取 `va.save.slots[0].size`、上限/步长取 `va.init`（宿主从保存区槽表算，不写死 psABI 数字）。
  换机器/换约定只要还是"主游标 + 次游标 + 溢出指针 + 保存区指针"这一族就不改代码；不是这一族
  （aapcs64 的 gr/vr 计数、riscv 的分界）**明确报错**"该形态不在本族"，不按 sysv64 的偏移瞎算。
+ **验收（真跑，两条支都覆盖）**：`test_jit_va_arg_sysv64_reads_the_register_save_area`（两个未命名
  实参都在寄存器 ⇒ `4*10+7 = 47`）与 `test_jit_va_arg_sysv64_overflows_to_the_stack_arg_area`
  （第 6 个超出 GP 池 ⇒ 必须改取溢出区并推进溢出游标 ⇒ `42`）。
+ **仍然存在的 x86 专属（= 下一步要拆的洞，已记进方案）**：`va_start` 物化仍在生成器里，于是
  ① `VaInit.offsets` 把 `gp|fp` 打包成一个 u64（生成器只有 8 字节帧相对 store），② 为此新增的
  能力角色 `gpr_imm`。通用做法 = 把 `va_start` 也搬进同一个 IR 展开（字段按自己的宽度 store、
  帧内地址用现成 `StackAddr`），两者都会消失。
+ 验证：`forge-codegen`（1360，含两条新 JIT 用例）、`forge-abi`、`forge-tests`（矩阵 196/3/0）、
  `forge-ir`、`forge-isa-dsl`、`forge-isa` 全绿；`clippy -D warnings` 0、`cargo fmt --check` 0、
  markdownlint 0。

### Fixed (2026-10-01) — sysv64 的栈实参落点少了一槽（收到的是返回地址）

`sysv64` 内置规则的 `first_offset_slots` 写的是 **1**（只算返回地址），而本实现的被调方**总是
`push` 帧指针** ⇒ 从 `rbp` 看第一个栈实参在 `[rbp + 16]`（返回地址 + 保存的 fp 各占一槽）。
win64/aapcs64/lp64d 本来就都写 2，只有 sysv64 这一份漏了。

+ **实测证据**：新用例 `test_jit_sysv64_seventh_integer_arg_comes_from_the_stack`（7 个整数形参，
  第 7 个走栈）在修前返回 `2001632886933`（**返回地址**），修后返回 `7`。
+ **影响面**：任何走栈的 sysv64 形参（第 7 个整数起、超 FP 池的浮点）此前都读错。与变参无关——
  这条路径此前**没有用例**，是做变参保存区时顺手撞见的。
+ 黄金快照 `sysv64.plan.txt` 随之更新：`first_arg_off` 8 → 16、栈实参偏移整体 +8（逐行核对过；
  其余三份约定不变，因为它们本来就是 2）。

### Added (2026-10-01) — 变参 V3 发射（sysv64）：序言 spill 参数寄存器 + `va_start` 物化 `va_list` 对象

上一片把保存区的**计划面数据**做对了（槽表 + 字段布局），这一片把它接上发射：

+ **管线预留保存区**：`CallLayout.va.save` 有区就按 `align`/`size` 在 locals 区之后开一段帧字节，
  偏移落在 `LowerCtx::va_save_off` / `AllocResult.va_save_off`（与 `Opcode::VaStart` 的对象槽
  同一条通路 ⇒ 两者不重叠）。实测（sysv64）：`size=176 align=16 depth=176 → 帧基址 -240`。
+ **序言 spill**：`move_args` 在收参**之前**按槽表把参数寄存器写进保存区，**按槽的寄存器类分派**
  （GP 用 `stack_arg_store`、FP 用 `{ role = "stack_arg_store", class = "fpr" }`——两个角色本来就有）。
+ **`va_start` 物化对象**（sysv64 一族）：`lea` 对象地址 + `lea` 溢出区 + `lea` 保存区，用**新角色
  `gpr_imm`**（x86 = `MOV_REG_IMM64`）把宿主算好的 `gp_offset|fp_offset` 打包值装进寄存器，
  再三条帧相对 store 写字段。"已用掉几个参数寄存器 / 溢出区从哪开始"由**宿主**按 plan 预先算好
  （`CallLayout.va.init: VaInit`）——生成物是各约定通用的，算不出来。
+ **验收（真跑）**：`test_jit_va_start_materializes_the_sysv64_register_save_area`——两侧都用 sysv64，
  被调方用 **IR 算术**读出 `gp_offset`（= 8）与 `reg_save_area`，从 `[reg_save+8]`/`[+16]` 取回
  `4`/`7` 算出 **47**。这条路一次性验证了：序言 spill 的槽序与偏移、`gp_offset` 初值、
  `reg_save_area` 指向保存区。
+ **仍是 ⬜**：保存区形态的 `va_arg`（分支式取值——游标超界要改取溢出区）⇒ sysv64 的 `va_arg`
  继续 fail-closed；aapcs64/riscv 的 `init` 未算 ⇒ 它们的 `va_start` 也继续 fail-closed（消息点名）。
+ 验证：`forge-codegen`（1358，含两条新 JIT 用例）、`forge-abi`、`forge-tests`（矩阵 196/3/0）、
  `forge-ir`、`forge-isa-dsl`、`forge-isa` 等全绿；`clippy -D warnings` 0、`cargo fmt --check` 0、
  markdownlint 0。

### Added (2026-10-01) — 变参 V3 计划面：`va_list` 字段布局 + 寄存器保存区槽表

寄存器保存区（`sysv64`/`lp64d`/`aapcs64`）是变参最后一块功能缺口。先把**风险最大的东西**做对：
槽表与字段布局的数字——`va_arg` 的游标与序言 spill 都按它们走，算错就是**静默错值**。

+ **计划面新增两样数据**（`AbiPlan::va_area`）：`fields`（`va_list` 对象字段：名/偏移/宽）与
  `save`（寄存器保存区：大小/对齐/槽表 `{寄存器, 区内偏移, 字节数}`）；`CallLayout.va` 有对应的
  **运行时中立镜像**（只带偏移/大小/类+号，不带名字）。
+ **四份内置约定的数字**（由形态定的 psABI 事实，不是使用者选项）：
  win64 `cursor@0+8`（无保存区）；sysv64 4 字段 + 保存区 `176/16` = 6 GP（8 字节槽）+ 8 XMM
  （16 字节槽）；aapcs64 5 字段 + 保存区 `192/16` = 8 X + 8 V；lp64d `area@0+8` + 保存区 `128/8`
  = 8 X + 8 F。
+ **踩到并修掉一个真问题**：浮点槽宽**不能取"寄存器类宽"**。psABI 规定的是**保存区槽宽**——
  按绑定宽度算会得到 arm64 `128`（应 `192`：V 槽恒 16 字节）、riscv `96`（应 `128`：FP 槽 =
  XLEN）。现在槽宽按**形态**给（与字段布局同一类事实）：`sysv_reg_save`/`aapcs64_struct` → 16、
  `riscv_save_area` → `slot_bytes`。
+ **守卫**：`forge-abi` 的 `va_object_layout_matches_the_psabi_numbers`（四份约定的字段/槽表逐条
  对 psABI）+ `abi_target_real::call_layout_mirrors_the_variadic_shape` 扩展到断言镜像后的字段与
  槽表；黄金快照 `tests/golden/*.plan.txt` 重新生成（diff 只有新增的 `va_field`/`va_save*` 行）。
+ **诚实边界**：发射侧（序言按槽表 spill、给保存区与对象开帧槽、`va_start` 写字段、`va_arg`
  分支取值）仍未做 ⇒ `va_start` 对保存区形态继续 **fail-closed**；**已知偏差**：arm64 的绑定把 V
  建模成 64 位 ⇒ 序言只会写低 64 位（向量/HFA 变参要 128 位存取，arm64 落地时处理）。
+ 验证：`forge-abi`（含新守卫）、`forge-codegen`（1356 + `abi_target_real` 24）、`forge-tests`
  （矩阵 196/3/0）、`forge-ir`、`forge-isa-dsl` 全绿；`clippy -D warnings` 0、`cargo fmt --check` 0、
  markdownlint 0。

### Added (2026-10-01) — 变参进架构无关的 JIT 矩阵（`variadic_va_arg_int_and_float`）

变参此前只有 `forge-codegen` 里的 JIT 用例（单机、单 ISA），**架构无关的 `jit_matrix` 里
一个变参用例都没有**——等于跨 ISA 证据为零，而这正是矩阵存在的理由。这一片把它补上：

+ **新用例 `variadic_va_arg_int_and_float`**（`CaseKind::Module`）：被调方变参 `callee(fmt, ...)`
  用 `va_start` 物化对象、读两次 `va_arg(i64)` 与一次 `va_arg(f64)`，算
  `a*10 + b + (i64)d`；调用方（非变参 `main`）多传三个实参 ⇒ 未命名实参按被调方语义进传出
  栈区。**`4*10 + 7 + 2 = 49`**（x86 矩阵实测：**196 passed / 3 skipped / 0 failed**，195 → 196）。
+ **能力门控**：`VaStart`/`VaArg` 是**生成器专用臂**（谱里没有也不该有 `[[lowering]]`），所以
  把它们加进 x86 runner 的 `CAPS_EXTRA`（"非 lowering 路径"清单，与 `Call`/`GetElementPtr`
  同类）；其它 ISA 没申报 ⇒ 该用例按 `Skip("capability")` 诚实跳过。P1-16 的
  "`CAPS_EXTRA` 与 TOML 生成的 `SUPPORTED_OPS` 不相交"守卫继续成立。
+ **顺带成为模块签名表接线的跨 ISA 守卫**：这条用例的调用方只有靠
  `JitCompiler::compile_module` 装表才会把未命名实参发到栈上（V1 那个缺口的现场）——
  矩阵里从此有一条真跑用例盯着它。
+ `check_isa` 的 `UNCOVERED_OPS` 里 `VaStart`/`VaArg` 两条**理由改写**：它们说的是"逐 op 最小
  构造器造不出（要变参上下文）"，而不是"矩阵没覆盖"——矩阵现在覆盖了。
+ 验证：`forge-tests --lib` 43（含矩阵 196/3/0）、`forge-codegen --lib --all-features`（1356）、
  `forge-ir`、`forge-isa-dsl`、`forge-abi` 全绿；`clippy -D warnings` 0、`cargo fmt --check` 0、
  markdownlint 0。

### Added (2026-10-01) — 第 5+ 个浮点形参的收参路径：补两条真跑用例 + 更正一处误导性注释

查"栈上的浮点形参收参"时，先按上一片的对称性假设它是缺口（调用方已按类分派、被调方还在用整数
load）——**实测证伪**，结论与文档/注释都按实测更正：

+ **实际机制**：ABI 落在栈上的形参没有入场寄存器 ⇒ 不在 `assignments` 里 ⇒ 由 `move_args` 的
  "先收进 spill 槽"中转处理：`load 布局槽 → scratch(GPR) → store spill 槽`，**按位搬运**——
  所以**浮点参数本来就正确**（位型不变），FPR 溢出槽的 load 再按类还原。
+ **`ArgPlace::Stack` 那一支里的"浮点直接收进寄存器"路径当前到不了**（未分配的形参在
  `#stack_arg_receive` 之后就 `continue` 了）。它是**防御性**分支：万一将来"栈落点 + 已分配
  寄存器"同时成立，把 FPR 当整数搬会静默错值，所以保持 fail-closed，并在注释里写清它需要
  `{ role = "stack_arg_load", class = "fpr" }` 才能接。原先那句"栈上的浮点参数收参尚未接进
  发射"是**误导**（听起来像功能缺失），已改写。
+ **新增两条真跑用例（这条路径此前零覆盖）**：
  `test_jit_float_params_beyond_xmm_registers_come_from_the_stack`（6 个 f64 形参，只有前 4 个
  进 XMM0-3，取第 5/6 个算 `e + f = 7`）与
  `test_jit_stack_float_param_is_received_into_a_register`（只用第 5 个：`9.0 → 9`）——
  钉的是**端到端的值**（收参装错寄存器类会静默给错值，只看"编译通过"抓不到）。
+ 文档：`docs/reference/calling-conventions.md` 的浮点栈实参两段按实测改写，守卫索引加一行。
+ 验证：`forge-codegen --lib --all-features`（1356）、`forge-tests`（x86 矩阵 195/3/0）、
  `forge-ir`、`forge-isa-dsl`、`forge-abi` 全绿；`clippy -D warnings` 0、`cargo fmt --check` 0、
  markdownlint 0。

### Added (2026-10-01) — 变参 V4 第三片：`f32` 的默认实参提升（`va_arg` 取 f64 再窄回）

`f32` 走进来：按 **C/LLVM 的默认实参提升**，`float` 在变参调用里被传成 `double`，所以
`va_arg(ap, f32)` 必须**取 `f64` 再窄回**——只按 f32 读 4 字节会读到 promoted double 的**低半**
（垃圾位型，静默错值）。

+ **新能力角色** `{ role = "fpr_narrow", bits = 32 }`（x86 = `CVTSD2SS`）：把默认提升后的浮点值
  窄回目标宽度（`bits` 用 S9 的宽度语义）。缺它 ⇒ `va_arg(ap, f32)` **明确 Unsupported**
  （`f64` 不受影响）。
+ **D5 裁定同日更正**：提升规则**不放约定数据**，而是放在 `va_arg` 的语义里——C 的默认实参
  提升是**语言层**规则、不是 ABI 事实，LLVM 就是这么分工的（`va_arg` 指令语义规定"小于
  `double` 的浮点按 `double` 读再截断"，clang 在调用点做提升）。原先写的"进 `AbiRules` 的
  `va_promote`"是**过度数据化**：四份内置约定都是 double，加一个没人会改的键不合本仓库
  "没有消费者不加键"的纪律。**触发条件**：若某份约定的变参浮点提升在 psABI 层面不是 double，
  再加数据键。
+ 发射序列（win64）：`cur = [ap]` → `tmp = [cur]`（fpr 取值，f64）→ `val = cvtsd2ss(tmp)` →
  `cur += slot_bytes` → `[ap] = cur`。
+ **验收（真跑）**：`test_jit_va_arg_narrows_promoted_f32`——调用方传 `2.5` / `3.25`（f64），
  被调方两次 `va_arg(ap, f32)` 窄回后相加 = `5.75`，`fptosi` 得 **5**。
+ 仍是 ⬜：寄存器保存区（`sysv64`/`lp64d`/`aapcs64` 的序言物化）、更窄浮点与向量的取值能力。
+ 验证：`forge-codegen --lib --all-features`（1354）、`forge-tests`（x86 矩阵 195/3/0）、
  `forge-ir`、`forge-isa-dsl`、`forge-abi` 全绿；`clippy -D warnings` 0、`cargo fmt --check` 0、
  markdownlint 0。

### Added (2026-10-01) — 变参 V4 第二片：浮点（f64）未命名实参两端打通

整数类之后补上浮点：**调用方**把 f64 栈实参按类写进传出区、**被调方**用 `va_arg(ap, f64)` 读回。

+ **两条按寄存器类限定的能力角色**（v18 S9 的 `class` 限定，与 `callee_save` 同类用法）：
  + `{ role = "stack_arg_store", class = "fpr" }`（x86 = `MOVSD_MR`）——调用方写浮点栈实参；
  + `{ role = "ptr_load", class = "fpr" }`（x86 = `MOVSD_R_MEM`）——被调方按 fpr 取值。
+ **只做被调方那一半是不够的**：调用方若拿整数 store（`MOV64_MR`）搬 XMM，值会被静默写坏。
  所以 `arg_move_loop` 的栈参数分支改成**按实参的 IR 类型选 store**：浮点走 fpr 版；本 ISA
  没申报 fpr 版 ⇒ **明确 Unsupported**（不再静默错值）。同一路径也让**第 5+ 个命名浮点实参**
  归位（此前无用例覆盖）。
+ 被调方 `va_arg` 的值取值同样按结果类分派：整数/指针走无类限定的 `ptr_load`（x86 = `MOV_R_MEM`，
  宽度随 IR 类型自动），标量浮点走 fpr 版。
+ **覆盖范围（诚实清单）**：整数类 + **f64** 已通；`f32`/更窄要**提升规则**（D5：调用方按 C 的
  默认提升传 f64，读取方读 f64 再截断），向量要"mem → 向量寄存器"的取值能力——两者都在
  **运行期按结果类型 fail-closed**。
+ **验收（真跑）**：`test_jit_va_arg_reads_unnamed_float_args`——变参 `callee(fmt)` 两次
  `va_arg(ap, f64)` 取回 `4.5` / `7.0`，相加后 `fptosi` 得 `11`。
+ 验证：`forge-codegen --lib --all-features`（1353）、`forge-tests`（x86 矩阵 195/3/0）、
  `forge-ir`、`forge-isa-dsl`、`forge-abi` 全绿；`clippy -D warnings` 0、`cargo fmt --check` 0、
  markdownlint 0。

### Added (2026-10-01) — 变参 V4 第一片：`va_arg` 取值 + 原地推进（整数类，win64 端到端）

`va_list` 对象之上补上"取值"这一半：**`va_arg(ap, ty)`**（IR op 本来就有——`ops.toml`、文本层
`va_arg <ptrty> <ptr>, <ty>`、二进制格式往返；这一片补上 `FunctionBuilder::va_arg` 与发射）。

+ **发射按三个能力角色取**（`ptr_load`/`ptr_store`/`add_imm`），序列 =
  `cur = [ap]` → `val = [cur]` → `cur += slot_bytes` → `[ap] = cur`；x86 谱把这三个角色打在
  `MOV_R_MEM`/`STORE_MEM_R`/`ADD_R_IMM32` 上（指令本来就有，只是没有角色——值是**宽度自动**
  的 `gprx` 槽，所以 i32/i64 都走同一条发射路径）。缺角色 ⇒ 整条臂明确 `Unsupported`。
  这三个角色**不是 ABI 能力**（`abi_view::role_capability` 返回 `None`）：`forge-isa abi check`
  不看它们，缺了由生成器点名。
+ **只覆盖整数类结果**：浮点/向量要「mem → FPR」的取值能力（x86 谱今天没有，`fpr_mov` 是
  寄存器间搬运），所以**按结果类型 fail-closed**——不把 FPR 塞进 GPR 指令里静默编错值。
  提升规则（D5）随之留给"有浮点取值能力"的那一片：整数类今天靠"调用方按槽写、读取方按结果
  宽度截断"已经自洽。
+ **验收（真跑）**：`test_jit_va_arg_reads_unnamed_stack_args`（V3 那条改用 `va_arg`）——
  变参 `callee(fmt)` 两次 `va_arg` 取回 `4` / `7`，算 `4*10+7 = 47`。
+ 仍是 ⬜：寄存器保存区（`sysv64`/`lp64d`/`aapcs64` 的序言物化）、浮点取值能力、提升规则数据。
+ 验证：`forge-codegen --lib --all-features`（1352）、`forge-isa-dsl`、`forge-ir`、`forge-abi`、
  `forge-tests`（x86 矩阵 195 passed / 3 skipped / 0 failed）全绿；`clippy -D warnings` 0、
  `cargo fmt --check` 0、markdownlint 0。

### Changed (2026-10-01) — 变参 V3 第一片：`va_start` 物化 `va_list` 对象（契约收紧，破坏性）

V2 把 `va_start` 定义成"取未命名实参区的地址"——那只对 **win64 栈式**成立，往寄存器保存区
（`sysv64`/`lp64d`/`aapcs64`）走时不通用（那些约定的 `va_list` 是多字段对象）。按
`docs/plans/varargs-plan.md` 的裁定先把契约收紧，这一片落地其中的**对象物化**：

+ **`va_start` 现在物化 `va_list` 对象并返回对象地址**（破坏性：win64 多一个帧槽 + 一次 store，
  读未命名实参要先从对象里取出游标）。win64 的对象 = 1 个指针字段（偏移 0）= 未命名实参区地址。
+ **对象槽由管线分配**（不新增"ABI 临时区"机制）：`Opcode::VaStart` 与 `StackAddr` 走同一条
  通路——管线按 `va_area.size/align` 选偏移、设 `ctx.current_offset`、抬 `max_stack_bytes`，
  生成器只读 `ctx.current_offset`，不猜偏移。为什么不让前端 `alloca`：`va_list` 的大小/对齐是
  **约定的数据**（win64 = 8、sysv64 = 24），前端不该知道；而 `max_stack_bytes` 本来就是管线在算。
+ **发射**：`gen_va_start_lowering` = `lea` 对象地址 + `lea` 未命名区地址 + 帧相对 store 写字段 0，
  结果 = 对象地址。全部按角色取（`frame_addr` + `stack_arg_store`，谱里**没有** `VaStart` 的
  `[[lowering]]` 规则——与 `frame_set`/`frame_alloc` 同族：谱只申报能力，序列由生成器发）。
+ **验收（真跑）**：`test_jit_va_start_reads_unnamed_stack_args` 改成**穿过对象**读回两个未命名
  实参（`load(ap)` → `inttoptr` → `load` / `gep` / `load`），仍是 `4*10+7 = 47`。
+ **仍是 ⬜（诚实清单）**：寄存器保存区（`sysv64`/`lp64d`/`aapcs64` 的序言物化）、`VaArg` op 与
  提升规则；V4 还需要给"按 vreg 基址读写"与"寄存器 += 常量"申报三个能力角色
  （`ptr_load`/`ptr_store`/`add_imm`）——今天 x86 谱里那三条指令没有角色，生成器按纪律不按指令名探测。
+ 验证：`forge-codegen --lib --all-features`（1352）、`forge-tests`（x86 矩阵 195/3/0）全绿；
  `clippy -D warnings` 0、`cargo fmt --check` 0、markdownlint 0。

### Added (2026-10-01) — 变参 V2 发射：被调方用 `va_start` 读回未命名实参（win64 端到端）

上一片只铺了数据面（IR 有 `VaStart`、`CallLayout.va` 有形态，但**没有任何发射路径**）。这一片把
被调方这条腿接通，并顺手补上它暴露出来的一个真缺口：

+ **发射：`VaStart` 由生成器按能力角色发**（`lowering.rs::gen_va_start_lowering`）——`roles = ["frame_addr"]`
  （x86 = `LEA_RBP_OFF`）发一条 `lea dst, [fp + 未命名区起点]`，其余（`load`/`gep`/算术）全走
  既有规则。与 `frame_set`/`frame_alloc`/`callee_save` 同族：**谱只申报能力，序列由生成器发**
  （`isa/x86.toml` 里**没有也不该有** `VaStart` 的 `[[lowering]]` 规则）。寄存器保存区形态
  （`sysv64`/`lp64d`/`aapcs64`）与该约定不支持变参时**明确 fail-closed**，消息分别点名 V3 与
  "`va_list` 未声明"。
+ **新 op 的落点是"未命名区起点"**：`max(命名栈实参 offset + size)`；没有命名栈参数时是
  **`first_arg_offset + shadow_bytes`**（win64 = 16 + 32 = 48）——**两个都要加**。引擎侧不变量是
  `Stack.offset(k) = first_arg_offset + shadow_bytes + k×slot`：`first_arg_offset` 是"帧基址 → 调用方
  sp"的距离，而调用方把第 0 个栈槽写在 `sp + shadow_bytes` 处。只取 `first_arg_offset` 会读到
  shadow 区里的垃圾（实测算出 1.4e15 量级的地址值）。
+ **修掉一个真缺口：`JitCompiler::compile_module` 从未装模块签名表**。`with_module_sigs` 的唯一
  调用者是 V1 的测试本身，**整模块编译的宿主全都没接** ⇒ 变参调用点按非变参发（三个实参全进
  RCX/RDX/R8）、被调方 `va_start` 读到 shadow 区。修法是**让模块自述这张表**，而不是在每个宿主里
  再抄一份：`forge_ir::Module::signature_table()`（下标 = `FuncRef::index()`；读**每个函数自己的**
  类型上下文——`Module::add_function` 只做入表，不把签名并进模块 store，读 `module.types` 会全部
  落空）+ `FunctionCompiler::for_module(machine, &module)`（宿主侧默认写法）。
  `JitCompiler::compile_module`（串行 + 并行两条路径共用一份表）与 `forge-tests` 的矩阵/QEMU
  逐函数编译路径都改用它。
+ **验收（真跑，不是断言落点）**：`jit.rs::test_jit_va_start_reads_unnamed_stack_args`——
  变参 `callee(fmt)` 用 `va_start` → `load` → `gep` → `load` 读两个未命名实参算 `a*10+b`，
  调用方 `main` 传 `(0, 4, 7)`，**返回值 = 47**。
+ 连带：`forge-ir` 新增单测 `signature_table_mirrors_function_order`（普通/变参/void 三个函数
  的对齐与取值）。
+ **仍缺（诚实状态）**：寄存器保存区（V3）、`%al`/`va_meta` 写入、`va_arg` 与提升规则（V4）、
  以及**任何前端产出 `variadic` 签名**；单函数编译的宿主（forge-rustc 逐函数后端）拿不到签名表，
  变参调用点按非变参处理——两条都记进 `docs/reference/calling-conventions.md` 的已知缺口表。
+ 验证：`forge-ir --lib`（281）、`forge-codegen --lib --all-features`（1352）、`forge-tests --lib`（43）
  全绿；`clippy -D warnings` 0、`cargo fmt --check` 0；markdownlint 0。

### Fixed (2026-10-01) — 上一片留下的红门禁：`VaStart` 没进 lint 的能力缺口口径

`cargo test -p forge-isa-dsl --test lint_shipped` 在 HEAD 上就是红的——`f57979e` 把 `VaStart`
加进宿主 op 表（`forge-ir/ops.toml`）时没同步"能力缺口口径"快照，
`op_gap_matches_section_10_3` 于是把三谱的**真缺口**各多算一条（x86 3 → 4，列出的第 4 条正是
`VaStart`）。上一片的 CHANGELOG 验证清单里没列 `forge-isa-dsl`，所以漏掉了。

+ **修法不是改快照数字，而是归对桶**：`VaStart` 与 `Bitcast`/`Call`/`CallIndirect`/`GetElementPtr`
  等一样属于"谱里**没有** `[[lowering]]`、但**不是缺口**"——只是成因不同：它由**生成器按能力
  角色**发（`roles = ["frame_addr"]` → `lea`，与 `frame_set`/`frame_alloc` 一族同写法）。
  因此 `lint::HOST_PIPELINE_OPS` 由 7 条变 8 条（并写明两类成因的差别），三谱的**覆盖数与真
  缺口数不变**（`100/6/8/3`、`61/6/8/42`、`8/6/8/95`）。
+ 同步位置：`crates/frontend/forge-isa-dsl/tests/lint_shipped.rs` 的快照、
  `docs/plans/forge-isa-dsl-v19-plan.md`（§10.3 表 + §10.4 上方的机读化说明）、`CLAUDE.md` 的
  口径行。历史 CHANGELOG 条目（记 7 条的那段）**不改**——它记录的是当时的实测。
+ 教训（写给下一片）：**新增宿主 op 必须跑一次 `forge-isa lint <谱> --ops …`**，
  `lint_shipped` 那条守卫就是为这件事存在的。

### Added (2026-09-30) — 变参 V2 数据面：IR 新 op `VaStart` + `CallLayout.va` 镜像

变参被调方要能"读到未命名实参"，先得有两样东西：**IR 里表达"取未命名实参区的地址"**，以及
**运行时读得到的 va 形态**。这一片把这两半落地（发射侧另起一片）：

+ **IR：新 op `VaStart`**（`ops.toml` 一行：0 操作数 → 指针；`builder::va_start()`），语义是
  "取未命名实参区的地址"——它是**约定相关**的取值（win64 就是帧内那段栈地址；sysv64/riscv/arm64
  需要寄存器保存区）。`ops.toml` 追加在末尾 ⇒ 既有 opcode 编号不变（**不升格式版本**）。
  连带更新：`opcode_table` 的穷举 match + `type_rule = "none"` 计数基线（55 → 56）、
  `forge-opt` 的常量折叠（地址不是常量 ⇒ 不折叠）、`forge-tests` 覆盖矩阵的缺口清单
  （`("VaStart", "可变参数：需要 va_start ABI 夹具（方案 V2）")`）。
+ **计划镜像：`CallLayout.va: Option<VaInfo>`**（运行时中立：`VaKind`/`size`/`align`/`stack_only`），
  由宿主的 `AbiPlan::va_area` **逐条 match** 折过来（加变体即编译失败）。
  `VaKind::supports_frame_addr_va_start()` 是"能不能直接用帧内栈地址实现 `va_start`"的判据——
  只有 `win64_stack` 为真；其余形态（寄存器保存区）留给方案 V3。
+ **诚实状态**：`isa/x86.toml` 里**还没有** `VaStart` 的 lowering 规则，所以今天任何函数用
  `va_start` 都会因"没有规则"而 fail-closed——这一片只铺数据面，**不改任何现有行为**
  （新增的守卫是集成测试 `abi_target_real`，lib 计数仍是 1351）。
+ 守卫：`abi_target_real::call_layout_mirrors_the_variadic_shape`（win64 栈式 8/8、
  sysv64 保存区 24/8、非变参不带 `va`）+ `forge-ir` 的 opcode 表守卫。
  文档：方案 V2 标 🚧（数据面 ✅ / 发射待做）、参考文档的 `CallLayout` 表与守卫索引各加一行。
+ 验证：`forge-ir`（280+ 全绿）、`forge-opt`、`forge-abi`、`forge-codegen`（1352）、
  `forge-tests`（43）、`forge-hir` 全绿；`clippy -D warnings` 0、`cargo fmt --check` 0；
  markdownlint 0。

### Changed (2026-09-30) — 抽出 `pipeline/abi_setup.rs`：调用约定装配从 `CompileState::new` 里独立出来

+ **三条约定层规则集中到一处**（`forge-codegen/src/pipeline/abi_setup.rs::setup_conv`）：① 约定名解析（未注册 fail-closed）；② **约定级整数返回槽**探针（空参 + 整数返回问一次引擎）；③ **函数级 plan 的 fail-closed**（错误消息含三步修法与自查命令）+ 约定级破坏集兜底 + 模块签名表落位。返回该函数的 `AbiPlan`（调用方存进 `CompileState` 供 `FORGE_TRACE_ABI=1` 打印）。
+ 为什么：这三件事原先埋在 `CompileState::new`（约 300 行的构造过程）中间，**既读不出来也单独测不了**——只能"编译一个函数再看结果"。抽出来后调用点只剩一行，规则与错误消息在一个文件里，`#[cfg(test)] mod tests` 的 4 条单测直接断言"返回槽 = RAX / 布局 = win64 / 破坏集非空 / 未注册约定点名修法 / 模块签名表进出 `variadic_of`"。
+ 顺便清掉 `compiler.rs` 里那 70 余行的内联块与三处重复的 `crate::pipeline::abi_target::…` 长路径。
+ 验证：`cargo test -p forge-codegen --lib --all-features` **1351 passed**（含 4 条新单测）、`-p forge-codegen`（全 target，exit 0）、`-p forge-tests`、`-p forge-abi` 全绿；`clippy -D warnings` 0、`cargo fmt --check` 0。

### Added (2026-09-30) — 变参 V5：`forge-isa abi check` 上报变参状态（三种自相矛盾算硬错）

+ **新增一行 `ℹ 变参 …`**（每份约定一条）：把"这台机器 × 这份约定的变参处于什么状态"说清楚——
  `win64` 栈式（`va_list` = 栈指针，调用方一侧可用）；`sysv64`/`lp64d`/`aapcs64` 需要**寄存器
  保存区** ⇒ 如实报"发射侧尚未物化（方案 V2/V3）"。这是**已知状态而非缺口**，只进 `ℹ`、
  不影响退出码（缺口语义保持"这台机器做不了"，否则 `--strict` 会把"发射尚未实现"混进机器能力账）。
+ **三种自相矛盾升级为硬错**（影响退出码）：① `va_list = "win64_stack"` 却
  `variadic_stack_only = false`（未命名实参可能进寄存器，而 va_list 只指向栈）；
  ② 声明了形态却没给合法的 `va_list_size`/`va_list_align`（要 > 0 且 size 是 align 的整数倍）；
  ③ `va_meta_pool`/`va_len_pool` 点到的池在这台机器上解析不动。
+ 守卫：`cli_tests::abi_check_reports_the_variadic_state`（三份发行谱的 `ℹ` 行 + "不加缺口"）与
  `abi.rs` 的 5 条 `variadic_report_*` 单测（矛盾 / 尺寸 / 池名 / 保存区 / 不支持变参）。
  方案文档的 V5 标 ✅，参考文档的守卫索引加一行。
+ 验证：`cargo test -p forge-isa` 全绿（含 6 条 `abi` CLI 用例 + 5 条单测）。

### Changed (2026-09-30) — `LowerCtx` 的调用约定字段收进 `CallConvCtx`（破坏性重构）

+ **6 个散落字段 → 一个结构体**：`LowerCtx::{call_conv, call_conv_name, call_layout,
  conv_clobbers, conv_ret_gpr, module_sigs}` 合并为 `LowerCtx::conv: CallConvCtx`，字段名
  自解释：`id`（IR 声明的 `CallConvId`）/ `name`（解析后的注册表键）/ `layout`（本函数布局）/
  `clobbers`（约定级破坏集）/ `ret_gpr`（约定级整数返回槽）/ `module_sigs`（模块级签名表）。
  理由：原来看不出哪些是"IR 声明的"、哪些是"宿主算好的"、哪些是"本函数的"。
+ **两个把逻辑收进来的助手**：`conv.layout()`（替代到处写的 `.as_ref()`）与
  `conv.variadic_of(func_ref)`（把"按 `FuncRef` 查变参信息 + 越界兜底"从生成物搬回运行时，
  生成物里 4 行塌成 1 行，且宿主侧测试可直接覆盖）。参考文档与 `CLAUDE.md` 同步改名。
+ 验证：`forge-isa-runtime` 4、`forge-abi` 7、`forge-isa-dsl` 18、`forge-codegen`（29 个测试
  二进制，exit 0）、`forge-tests` 2、`forge-opt`/`forge-mem`/`forge-grammar`/`forge-hir`/
  `forge-object`/`forge-plugin`/`forge-isa` 全绿；`clippy -D warnings` 0、`cargo fmt --check` 0。

### Changed (2026-09-30) — 调用点 API 的人体工学：`CallRequest` 结构体 + `CallPlanError` 带诊断（破坏性）

两个问题都是"用错编译器不会拦、出错看不出来"，按人体工学一次改掉（不留兼容层）：

+ **一次调用 = 一个结构体**：`plan_call(isa, &CallRequest)` / `CallPlanner::plan_call(&req)` 取代
  `plan_call(isa, conv, args, rets, variadic)` 这种 4 个位置参数（`args`/`rets` 顺序与 `variadic`
  的含义都容易搞错）。builder 让调用点自解释：`CallRequest::new(conv).args(a).rets(r).variadic(2)`；
  `args`/`rets` 收 `IntoIterator<Item: Into<ArgShape>>`，数组、`Vec`、`&Vec`、`iter()` 都能直接传
  （为此加了 `impl From<&ArgShape> for ArgShape`）。宿主侧的 `plan_for_shapes(machine, reg, &req)`
  同步收敛成 3 个参数。
+ **失败带诊断**：`plan_call` 由 `Option<CallLayout>` 改为 `Result<CallLayout, CallPlanError>`——
  `NoPlanner { isa }`（宿主没接 planner，消息给 `register_call_planner` / `ensure_registered` 两条
  出路）与 `Failed { conv, why }`（`why` 是**引擎原文**：池不够 / 缺绑定 / 能力缺口，消息带
  `forge-isa abi check` 与参考文档路径）。旧版把原因丢掉，调用点只能报"布局不可得"。
+ **生成物**：调用点把 `CallPlanError` 的原文原样带进 `Unsupported`，不再自己编一句泛化文案。
+ 守卫 `abi_target_real::call_plan_errors_carry_the_reason`（未注册 ISA ⇒ `NoPlanner`；注册了但约定未注册 ⇒ `Failed` 且正文点名约定与原因）；`call_planner_registry_serves_the_shape_plan` 的"未注册 ⇒ None"断言同步改为 `NoPlanner`。文档：参考文档「调用点 plan」补两条人体工学纪律，守卫索引加一行。
+ 验证：`forge-isa-runtime` 4、`forge-abi` 7、`forge-isa-dsl` 18、`forge-codegen` 29、`forge-tests` 2、`forge-opt`/`forge-mem`/`forge-grammar`/`forge-hir`/`forge-object`/`forge-plugin`/`forge-isa` 全部绿；`clippy -D warnings` 0、`cargo fmt --check` 0。
  （**环境提示**：本机磁盘 97% / 常驻 node 进程 6 GB，`cargo test --workspace` 一次性跑会在编译
  `forge-ir` 的**测试二进制**时 OOM；逐包跑可过。`forge-ir` 本片未改动。）

### Docs (2026-09-30) — 调用约定层收口：`现状总览与守卫索引` + 文档守卫（索引不许指向不存在的测试）

+ **`docs/reference/calling-conventions.md` 新增「现状总览与守卫索引」**：一张表看完"这一层做到哪一步、由哪条守卫钉住"——约定数据→计划、IR 读路径、适配器交叉核对、调用点按形状算落点、约定级返回槽、无 plan = 编译错误、多值返回、调用点形状摊成员、自定义约定、异域钩子、变参（规划 ✅ / 调用方 ✅ / 被调方 ⬜）、`[abi]` 删键的文档一致性、以及三条"评估后不做"（按成员拆、`callee_pop`、红区/尾调用）。
+ **新守卫** `forge-abi/tests/doc_guard.rs::guard_index_names_exist`：表里"守卫"列的每个名字都必须在 `crates/**/*.rs` 里找到 `fn <名字>`——文档不许引用不存在的守卫（测试改名/删掉时会红）。同 `schema_guard` 对键表、`varargs-plan` 对 `va_list` 形状表的做法。
+ 验证：`cargo test -p forge-abi --test doc_guard` 绿；改动的文档 markdownlint 0。

### Added (2026-09-30) — 变参 V0+V1：调用点按**被调方的变参语义**发未命名实参（win64 端到端）

+ **`CallPlanner::plan_call` / `call_plan::plan_call` / `abi_target::plan_for_shapes` 多一位 `variadic: Option<(bool, u32)>`** = "被调方是不是变参、命名了几个"：调用点只看得见实参形状，而"未命名实参在**只走栈**的约定（win64/aapcs64/lp64d）里要改判到栈"只有知道被调方签名才判得出来。
+ **模块级签名表**：`JitCompiler::compile_module` 按 `FuncRef` 建 `(variadic, 命名个数)` 表 → `FunctionCompiler::with_module_sigs` → `LowerCtx::module_sigs` → 生成物在 `Call` 处查表后传给 `plan_call`。IR **未改动**（沿用 LLVM 形态：变参签名只列**命名**参数，`Call` 可以多传实参；`Call` 本来就不校验个数）。单函数编译没有这张表 ⇒ 按非变参处理，与旧行为一致。
+ **验收（可观察的发射决策，不是"算出来的值"）**：`abi_target_real::call_site_variadic_hint_decides_unnamed_argument_placement`（不给提示 → `Reg(RDX)/Reg(R8)`；给提示 → `Stack/Stack`；并与被调方视角落点一致）+ `jit.rs::test_jit_variadic_unnamed_args_go_to_stack`（装表 ⇒ `stack_arg_bytes == shadow(32) + 2×8 = 48`，不装 ⇒ 0；并 JIT 真跑一遍不崩、返回命名参数值）。
+ **更正一处我自己的误判**：上一轮把"IR 表达不了未命名实参"记成必须先动 IR（D7）。实测证明**不需要改 IR**——那次是**测试把被调方建模错了**（3 个形参全列进签名又标 `variadic` ⇒ `fixed_count == 实参个数` ⇒ 变参提示无操作、用例假绿）。方案文档的 D6/D7 与实测经过已按事实改写（含"错/对两种建模"的对照表）。
+ **仍缺**：`va_meta`（SysV 的 `%al`）写入、被调方 `va_area` 物化（V2/V3）、`va_arg`（V4）——都需要 `va_start` 的 IR 形态，且今天没有前端产出变参签名。
+ 验证：`cargo test --workspace --exclude forge-rustc` 全绿、`clippy -D warnings` 0、`cargo fmt --check` 0、改动文档 markdownlint 0。

### Docs (2026-09-30) — 变参：实测"只接调用点信息"是**假绿**，补 D7（IR 表达未命名实参）

+ **先试后写**：按 D6 选项②把整条"模块签名表 → `LowerCtx` → 生成物查表 → `plan_call`"接通，
  并加了一个 win64 的 JIT 端到端用例——**用例通过，但它是假绿的**：管线投影出来的
  `fixed_count` **恒等于** IR 形参个数（`sig_view` 只有 `variadic: bool` 可用），而 `Call`
  每个形参一个实参 ⇒ `unnamed = i >= fixed_count` 永远为假，变参提示传下去也是无操作，
  未命名实参根本不会出现。**已整片回滚**（不留无效果的机器），结论写进方案。
+ **新决策点 D7**（IR/管线怎么表达"未命名实参"）：建议照 LLVM 形态——变参**签名只列命名参数**，
  `Call` 允许带比形参更多的实参（`declare @printf(ptr, ...)`）；它是 D6 的前置，两者建议一起做。
+ **守卫加强**：`abi_target_real::call_site_shapes_cannot_express_variadic_placement` 补第 ④ 段——
  断言 `signature_view(变参函数).fixed_count == params.len()`（未命名实参在今天的 IR 里无法表达）
  且变参函数的 plan 有 `va_area`（被调方那一半是完整的）。
+ 方案文档同步：§4 新增 D7、D6/D7 证据小节改写、§5 的 **V0 改为"两件一起做"**并记录这次
  "假绿 + 回滚"的过程与原因、V1 注明"验收要等 V2/V4"。
+ 验证：`cargo test -p forge-codegen --test abi_target_real call_site_shapes` 绿；改动文档 markdownlint 0。

### Docs (2026-09-30) — 变参方案补 D6：**V1 的真正阻塞点是"调用点不知道被调方是变参"**

+ **新决策点 D6**（调用点怎么知道"被调方是变参、命名了几个"）：`plan_call(isa, conv, args, rets)` 只吃**形状**，没有变参信息；而 win64 的 `variadic_stack_only = true` 会把未命名实参从寄存器改判到栈。同一个三 i64 签名，**形状路径给 `Reg(RDX)/Reg(R8)`、签名路径给 `Stack/Stack`** —— 所以 V1 缺的不是发射代码，而是这条信息通路（建议：管线编译模块时已有每个 `FuncRef` 的签名，把 `FuncRef → (variadic, fixed_count)` 表放进 `LowerCtx`，生成物在调用点查表后传给 `plan_call`；不动 IR）。
+ **新守卫** `abi_target_real::call_site_shapes_cannot_express_variadic_placement`：把上面两条路径的落点差异钉成**可执行证据**，并断言二者不同——哪天一致了就会红，提醒复查 D6 与 V1 验收。
+ 方案文档同步：§1 补"调用点拿不到变参信息"、§3 把 V1 的前置阻塞点写在发射代码之前、§4 新增 D6（含证据小节）、§5 新增 **V0（前置裁定）** 并把 V1 移到 V0 之后。
+ 验证：`cargo test -p forge-codegen --test abi_target_real variadic` 绿；改动文档 markdownlint 0。

### Docs (2026-09-30) — 变参方案（`docs/plans/varargs-plan.md`）+ 形状表由引擎输出钉住

+ **新方案文档**：变参（varargs）**规划已完成、发射与前端为空**的现状索引——分层列清"模型/引擎/黄金快照/CLI 有，发射与前端零消费"（并把证据落到符号名），四份内置约定的 `va_list` 形态表，五个决策点（先服务哪种场景 / `va_arg` 怎么表达 / 以哪份 psABI 定本 / 先做哪台机器 / 提升规则放哪），五期切片（V1 调用方 → V2 被调方 → V3 寄存器保存区 → V4 `va_arg` → V5 体检），以及**为什么现在不做 + 触发条件**（§6）。
+ **新守卫** `invariants.rs::va_shapes_match_the_documented_table`：方案 §2 那张表（`VaListKind`/size/align/未命名实参去向/`va_meta` 寄存器）必须与引擎对四份内置约定实际算出的 `AbiPlan` **逐格相同**——这张表是"变参做到哪一步"的索引，写歪了会把后来的人引错。
+ 索引与交叉引用：`docs/README.md` 的 plans 表加一行、并把调用约定方案一行更新到 A1–A8；`docs/reference/calling-conventions.md` 的缺口表新增"变参整体"一行指向该方案；v20 方案的 A6 剩余面改为指向新方案。
+ 验证：`cargo test -p forge-abi --test invariants va_shapes` 绿；改动文档 markdownlint 0。

### Added (2026-09-29) — v20 A8：自定义约定的**端到端**证据（JIT 真跑）+ 缺口报错给下一步

+ **新守卫 `test_jit_custom_convention_drives_argument_registers`**（`forge-codegen/src/runtime/jit.rs`）：运行期注册一份**自定义约定**（`int = ["R8","R9"]`、`ret_int = ["RAX"]`），编译 `callee(a,b) -> a-b` 与 `main() -> callee(20,7)` 并 **JIT 真执行**（得 13）；同时断言实参落点是 `(GPR(8), 8)`/`(GPR(8), 9)`——**故意不是** C 的 `RCX`/`RDX`。
  为什么这条最有价值：内置四份约定恰好都是 C 家族，"某个寄存器被写死"在它们身上看不出来（旧实现真写过"第二个返回值 = 类内号 1 = RDX"）；换自定义约定后写死就一定错值。这是"约定是使用者的数据、引擎不服务于个别指令集"的可执行证明。
+ **缺口报错给下一步**：`AbiError::Unsupported`（单个聚合要 ≥3 个寄存器槽）的正文从"≥3 的返回搬运见 A6"改为**说清两条路**——"按成员拆（暂无按值聚合的产出者）"与"多个**独立**返回值请用多值签名"；守卫 `errors.rs::three_slot_return_is_explicitly_unsupported` 同步断言这两个关键词。
+ 文档：`docs/reference/calling-conventions.md` 的「加自己的约定」补端到端证据一段（含为什么内置四份证明不了这件事）。
+ 验证：`forge-abi` + `forge-isa-dsl` 24 个测试二进制全绿、`forge-isa --test cli_tests abi` 5 绿。

### Docs (2026-09-29) — v20 A8：文档一致性收口（删键后的死引用 + 去处必须存在）

+ **改掉仍把 `[abi]` 当活键的句子**：`docs/reference/calling-conventions.md`（"整函数退回 `[abi]` 路径"→ 现在**没有旧路径可退**，落点没接就是明确 `Unsupported`；"`abi_view` 读 `[reg.*]`/`[abi]`"→ 改读 `[reg.*]`/`[machine]`；Win64 的 XMM 条改为 `[machine].callee_saved_gpr`）、`docs/reference/isa-dsl.md`（push 机制保存的是**约定数据**，无 plan 才退机器事实）。
+ **补齐 `callee_pop` 的"为什么没消费"**（诚实清单新增一行）：计划里算得出来（`AbiPlan::callee_pop_bytes`），但本实现的传出参数区在**调用方帧内**（不是"push 上去"），被调方 `ret N` 会把调用方 sp 抬高 N ⇒ 只改尾声必然错位，必须同时定**调用点契约**才谈得上正确；今天没有这样的 ABI 宿主。
+ **新守卫** `schema_guard::deleted_abi_keys_stay_deleted_and_their_destinations_exist`：① schema 键表里不得再出现 `[abi]` 节；②「原键 → 去处」对照表的去处列必须写成显式 `` `[machine].<键>` ``，且这些键**真的存在于 schema 键表**——文档不许把读者引到不存在的键上（为此把对照表那一行改写为显式路径）。
+ 验证：`cargo test -p forge-isa-dsl --test schema_guard` 7 绿；改动文档 markdownlint 0。

### Changed (2026-09-29) — v20 A6：接入体验收口（fail-closed 报错带修法）+「按成员赋值」评估结论

+ **报错带得动修法**：编译入口"规划不出调用布局"的消息改为**并列两种原因 + 三步修法 + 自查命令 + 文档路径**（`register_rules_toml`/`register_binding_toml`、`forge-isa abi check`/`abi plan`、`docs/reference/calling-conventions.md`、`FORGE_TRACE_ABI=1`）；生成物里调用点的"布局不可得"同样点名两种原因（宿主没注册 planner / 该签名规划不出来）。守卫 `conv_registry_read_path` 现在逐项断言这些关键词都在——报错不允许退化成死胡同。
+ **教程补一节**：`docs/guides/isa-dsl-tutorial.md` 新增「6. 要编译（`parts` 含 `tm`）就必须接调用约定」——为什么（约定是使用者的数据、没注册就 fail-closed）、可抄的三步（规则 TOML → 绑定 TOML → IR 里 `CallConvId`）、以及三条自查命令；原 §6/§7 顺延为 §7/§8，并补指向调用约定参考文档。
+ **「按成员赋值」评估结论：暂不做（有依据）**。做它（单个聚合要 ≥3 寄存器 / HFA 部分在寄存器）的前提是"聚合按值放在寄存器里"，而**今天的 IR 没有这种形态的产出者**：`forge-rustc` 的 `AbiKind::{Scalar, Pair, Indirect}` 把 ScalarPair 拆成两个标量、内存聚合走 sret/间接；`examples/mini_c` 没有结构体类型；全仓非测试代码构造 `Struct`/`Array` 类型 **0 处**；后端的 `ExtractValue`/`InsertValue` 本就是"在聚合的 64 位域里做字段操作"（≤8 字节聚合 = 位打包标量，本来就只占 1 个寄存器）。落点侧已就绪（`RetLoc::RegGroup`/`ArgPlace::Group`），触发条件写进方案 A6 ⑤。
+ 验证：workspace 131 个测试二进制全绿、clippy `-D warnings` 0、`cargo fmt --check` 0、改动文档 markdownlint 0。

### Fixed (2026-09-29) — v20 A6：调用点形状必须**摊开成员**（HFA 聚合实参曾静默搬错寄存器）

+ **问题**：调用点的实参/返回形状是**就地**拼的，聚合只报 `Aggregate { members: [] }`、向量恒报 `elem_is_float: false, lanes: 0`。而 **HFA/HVA 判定靠成员**：AAPCS64 上的 `{f64,f64}` 该走浮点池（`V0:V1`），成员留空则退化成"≤16B 聚合按两个整数槽"（`X0:X1`）——于是**调用点**与**被调方**（函数级 plan，成员齐全）分叉，实参静默落到错寄存器。
+ **修法**：`forge-isa-runtime` 新增 `ArgShape::from_ir_type(store, ty)`——从 IR 类型递归摊开成员与元素（`Struct`/`Array`/`Vector`；深度 4、成员 16 封顶，超限退化成"没有成员"交给规则兜底）；生成物的调用点改用它投影实参与**全部**返回值形状。
+ 连带效果：单寄存器 HFA（如 `{f32}`）在调用点从"算成 2 个整数槽 ⇒ fail-closed"变成**真的能发**（`Reg(V0)`）；多寄存器 HFA 仍是 fail-closed（按成员拆属"按成员赋值"那条 A6 活），但报错点从"落点不对"变成"落点对、发射未接"。
+ 守卫 `abi_target_real::hfa_aggregate_shape_matches_the_callee_plan`：`{f64,f64}` 参数在 aapcs64 上，**调用点 plan == 被调方 plan**（`V0:V1`），并**反证**"成员留空的形状必然与真实 plan 分叉"。
+ 验证：workspace 131 个测试二进制全绿、clippy `-D warnings` 0、`cargo fmt --check` 0。

### Changed (2026-09-29) — v20 A6：**多值返回**（≥3 槽返回的搬运）——落点全部来自绑定

+ **`Signature.ret` → `rets: Vec<TyView>`**：IR 的 `FunctionSignature.returns` 本来就允许多个返回值（ScalarPair 拆出来的两个标量、`(i64,i64,i64)` 这类），而 `sig_view` 以前只取 `first()`——"第二个返回值在哪个寄存器"于是只能由发射侧猜。
+ **引擎**：`rets.len() > 1` 时**逐个分类、逐个占一个返回寄存器**，产出 `RetLoc::RegPair`（2 个）/ `RetLoc::RegGroup`（≥3 个，新增变体）；每个分量必须是 `direct { slots = 1 }`，否则明确 `Unsupported`（聚合走单值路径）；池不够 ⇒ `PoolExhausted`。
+ **修掉一处 ISA 特定硬编码**：发射侧原本写死"第二个返回值进类内号 1"（x86 恰好是 RDX）——riscv 的类内号 1 是 X1（= ra），返回槽其实在 X10/X11。现在 win64 的 `ret_int` 补为 `["RAX","RDX"]`（`ret_float` 补 `["XMM0","XMM1"]`），落点一律来自绑定池；`Return` 与调用点按 `RetPlace::Pair`/`Group` **逐值按类分派**（整数 → `gpr_mov`、浮点 → `fpr_mov32/64`）。
+ **调用点看全结果**：`call_plan::plan_call(isa, conv, args, rets)` 与 `abi_target::plan_for_shapes` 收**全部**结果形状（此前只传第一个）；`RetPlace::Group` 带 `regs()` 助手。
+ 守卫：`forge-abi/tests/invariants.rs` 两条（win64 `RAX:RDX`、lp64d `X10:X11`、aapcs64 四个 f32 → `V0..V3`、混合类 `RAX:XMM1`；以及池不够/分量是聚合两条 fail-closed）、`abi_target_real::two_value_return_is_plan_driven`、**JIT 端到端** `test_jit_multi_value_return_is_read_back`（`callee(5) -> (6,7)`，调用方算 `6*10+7 = 67`）。
+ 仍 fail-closed（诚实清单）：单个聚合要 ≥3 个寄存器的返回（AAPCS64 的 4×f32 HFA，需按成员拆，属"按成员赋值"规则语言）；`a6_gap_inventory` 的那条缺口集不变。
+ 验证：workspace 131 个测试二进制全绿。

### Added (2026-09-29) — v20 A7：Swift / Go 的 `AbiHooks` 示例（异域约定的正规出口）

+ **新示例**：`crates/foundation/forge-abi/tests/exotic_hooks.rs`（`cargo test -p forge-abi --test exotic_hooks`，3 个用例）——Swift 的 aapcs64 方言与 Go 的 amd64 内部 ABI 各一份钩子 + 规则/绑定 TOML + 断言。证明"数据表达不了的部分"有正规出口，而不是去改引擎或往谱里塞特例。
+ 三个事实各演示一个：① **隐式上下文寄存器**（Swift `self` = X20、Go `g` = R14）只能由钩子填进 `AbiPlan::hidden.context`（数据侧没有写入者，参数列表也表达不了"隐式参数"）；② **语言专属类型**（Swift 的 errortype，LLVM 靠 `swifterror` 属性标识）由 `classify` 改判到 `swift_error` 池；③ **"保留"与"保存"不同**——X21 是 error **出参**，绑定的 `cs_gpr` 池只能说"保存这组"，钩子把它从 `callee_saved.regs` 摘掉并加进 `clobbers`（与 LLVM 的 `CSR_AArch64_AAPCS_SwiftError = CSR_AArch64_AAPCS − X21` 同义）。
+ 数值有出处：LLVM `AArch64CallingConvention.td` 的 `CCIfSwiftSelf → X20` / `CCIfSwiftError → X21`；Go 内部 ABI 的 `g` 在 amd64 的 R14。文档入口：`docs/reference/calling-conventions.md` 的「数据表达不了的：AbiHooks」新增对照表。

### Changed (2026-09-29) — v20 A6：**无 plan 不再继续编译**（编译入口 fail-closed）+ A6 缺口清点

+ **fail-closed 切换**（`pipeline/compiler.rs`）：`plan_for_function` 失败不再"留个 note、退回约定级数据继续编译"，而是直接在**编译入口**报 `IrError::Unsupported`——消息点名约定名、引擎给的原因、以及 fail-closed 口径。`CompileState::abi_plan` 由 `Option` 收敛为定值，`abi_plan_note` 字段删除（`FORGE_TRACE_ABI=1` 也随之只打印计划）。理由是旧行为把错误推给发射侧的各个 fail-closed 点（调用点/收参/栈参数/帧布局），消息离现场远，而且真实的引擎缺口会被误当成"这条签名能编"。
+ **先量后切**：切换前用 `FORGE_TRACE_ABI=1 --nocapture` 跑遍 `forge-codegen --lib --all-features`（1344 例）与 `forge-tests --lib`（43 例，含三台机器的 JIT 矩阵）——`[abi-plan]` **零命中**，即既有语料里没有一条路径靠"无 plan 回退"跑通。切换后 workspace 130 个测试二进制全绿。
+ **A6 缺口清点**（守卫 `abi_target_real::a6_gap_inventory`）：16 组代表性形状 × 三台真机 × 四份约定，**精确断言**缺口集恰好一条——`arm64 / aapcs64` 的 **4×f32 HFA 返回**（要 4 个连续寄存器槽，`RetLoc` 今天只有单/双寄存器形态）。此前列为缺口的 `Pair`（2 槽聚合）、**无指针的 `Indirect`**（by-ref 指针本身溢出到栈）、按引用向量、参数溢出到栈、浮点溢出到栈、混合两套寄存器文件的形状**都已有 plan**。
+ **唯一需要改的既有用例**：`conv_registry_read_path::rules_without_binding_fail_closed_at_the_compile_entry`（原 `…_on_the_return_slot`）——只有规则没有绑定时，错误现在发生在编译入口并点名"这台 ISA 没有为约定 X 注册寄存器绑定"，比原先"发射侧返回槽报错"更早、更具体。
+ 验证：`cargo fmt --check` 0、`clippy --workspace --exclude forge-rustc --all-targets --all-features -D warnings` 0、workspace 串行全套 130 个测试二进制绿。

### Changed (2026-09-27) — v20 A5-3 收口：`[abi].arg_class` 删除，**`[abi]` 一节整体下线**

+ **调用方按被调方落点搬实参**（`gen_call_lowering`/`arg_move_loop` 重写）：调用点把实参 IR 类型摊成 `ArgShape`（size/align/族）→ `machine::call_plan::plan_call(ISA, 约定, 形状, 返回形状)` → 逐实参按 `ArgPlace` 发射——`Reg { class, index }`（int → `gpr_mov`；fp → 按字节宽 `fpr_mov32/64`；向量 → `vec_mov`）、`Indirect { reg }`（`byref` 副本 + 指针 mov）、`Stack`（`caller_offset(k)`）、`Ignore` 跳过；`Pair`/`Group`/无指针 `Indirect` ⇒ **fail-closed**（A6 缺口，旧实现会把它们静默塞进整数寄存器）。`sret` 隐藏槽也改用 `CallLayout.hidden_sret`（不再假设"首 int 参数槽"）。
+ **被调方收参只走布局路径**：谱面的 `[abi].arg_class` 删掉后，`move_args` 的"无布局回退"整段改为 **fail-closed**（旧实现按谱面顺序塞寄存器 = 静默错值）。
+ **宿主注册调用点 planner**：`pipeline_hooks::ensure_registered()` 为三个发行后端注册 `register_isa_call_planner`（`Box::leak` 持有机器），并由 `FunctionCompiler::new` 幂等触发——生成的 `ensure_registered()` 不能直接调宿主（生成物里的 `crate::pipeline` 会被改写成运行时路径）。
+ **能力缺口改由角色说话**：没有 `roles = ["gpr_mov"]` 的谱（只做编码试点的夹具）在 Call/收参处整条降级 `Unsupported`，而不是生成期报"mov 形状不符"。
+ **`[abi]` 删除**：`Abi`/`ArgClass`/`ArgClassKind`/`ArgStrategy` 类型、`IsaModel::abi` 字段、schema 的 `[abi]`/`[abi.arg_class]` 两节、文档键表与 `isa-dsl.md` 的 `[abi]` 章（改写为"已删除 + 去处对照表"）全部下线；三份发行谱与两份夹具的 `[abi]` 段删除。
+ **两处生成期近似一并归位**：向量 by-ref 阈值 → 机器事实 `[machine].vector_by_ref_bytes`（x86 = 16）；`RegAllocConfig.param_reg_count` → 由 plan 数"落在寄存器的形参个数"（不再用 `TargetABI::int_arg_slot_count`）。
+ 验证：`forge-tests --lib` **43 绿**（含 x86/riscv/arm64 三条 JIT 矩阵与 QEMU 跨函数调用）、`forge-codegen --lib` 1278 绿。

### Changed (2026-09-27) — v20 A5-3：被调方 spill 收参改按 layout（不再读谱面 `[abi.arg_class]` 的顺序）

+ **`move_args` 的 spilled 寄存器参数**（`crates/frontend/forge-isa-dsl/src/v12/codegen/frame.rs`）：来源寄存器不再按"谱面 `int_regs[__pos]`"取，而是读**被调方布局** `ArgPlace::Reg { class, index }`——按位置/按类计数、`sret` 占不占首槽都由引擎算好；拿不到布局或落点不是寄存器 ⇒ **fail-closed**（不按谱面顺序猜）。
+ 这是 `[abi].arg_class` 的消费者之一被摘掉；剩下的消费者是**调用方**的参数搬运（`gen_call_lowering`/`arg_move_loop`）与 `gen_abi` 的 `arg_regs()`/`int_arg_slot_count`（宿主 `param_reg_count`）——它们是下一步（见计划文档）。
+ 验证：workspace 串行全套绿、`forge-tests --lib` 43 绿（含 x86/riscv/arm64 三条 JIT 矩阵）、clippy `-D warnings` 0、`cargo fmt --check` 0。

### Changed (2026-09-27) — v20 A5-3：`[abi].ret_regs` 删除（返回槽由 plan / 约定级数据给）

+ **谱面删键**：`[abi].ret_regs`（`Abi` 字段 / schema / 文档键表 / riscv64 与 arm64 谱里的声明、`demo8` 夹具的声明一并去掉）。它一直是"返回槽放哪"的**生成期近似**——`x86` 靠 index 0 兜底、`riscv` 声明 `X10`，而 `arm64` 的 `X0` 恰好也是 0，这种"每台机器各写一遍"的近似正是 A5-3 要收掉的东西。
+ **`Return`**：优先读**本函数的 plan**（`CallLayout.ret` 的 `RetPlace::Reg`），否则读宿主的**约定级整数返回槽** `LowerCtx::conv_ret_gpr`；两者都没有 ⇒ **fail-closed**（不再按 `index 0` 猜——riscv 的返回槽是 `X10`）。
+ **`Call`**（读被调方返回值）：读 `LowerCtx::conv_ret_gpr`（调用点看不到被调方的签名，而"标量整数返回放哪个寄存器"是约定级事实）。
+ **宿主的约定级数据**（`pipeline/compiler.rs`）：用"**空参 + 整数返回**"的合成签名问一次引擎（`plan_for_shapes`）填 `conv_ret_gpr`——与签名无关，与 `conv_clobbers` 同一套路。
+ **trait 收敛**：`TargetABI::ret_regs()` 现在**没有消费者**（生成物改读 plan / LowerCtx），DSL 不再从谱里发射它，运行时 trait 给它一个空表缺省实现（手工后端不破）。
+ 守卫：`abi_target_real::convention_level_int_return_slot` —— `plan_for_shapes(空参, i64 返回)` 必须给出**谱里原来那些值**（win64 `RAX`、lp64d `X10`、aapcs64 `X0`）。
+ 直连 lowering 的测试（`integration_tests::terminator_return_packet`）改为自己填 `ctx.conv_ret_gpr`——真实编译路径上由管线填好。
+ 验证：workspace 串行全套绿、clippy `-D warnings` 0、`cargo fmt --check` 0、三条 JIT 矩阵同值。

### Added (2026-09-27) — v20 A5-3：demo 夹具补**测试本地约定**（夹具从此真的有 plan）

**背景**：夹具谱（`tests/isa/demo.toml`、`demo8.toml`）没有约定数据，`Builtin(C)` 解析到内置 `c` 又没有 `(demo, c)` 绑定 ⇒ `AbiPlan` 算不出来，夹具函数一直走"**无 plan 回退**"。而 A5-3 要删的 `[abi].arg_class`/`ret_regs` 正是那条回退路径的输入——夹具不先有 plan，删键就会连带炸掉它们。

+ **测试本地约定**（`crates/backend/forge-codegen/tests/common/mod.rs`）：`ensure_demo_conventions()` 幂等注册两份规则 + 绑定（`register_rules_toml`/`register_binding_toml`，v20 A2 就有的公开 API）。数据照夹具谱口径写：`demo` = int 池 `X0-X3` / `ret_int = X0` / `stack 8,8` / `aliases = ["c"]`；`demo8` = int 池 `A0-A3` / `ret_int = A0` / `stack 1,1` / `aliases = ["c"]`；两者都无 callee-saved（与 `[machine].callee_saved_gpr = []` 一致）。
+ **四个夹具测试接线**（`demo8_tests.rs`、`demo_tm_tests.rs`、`lowering_read_path.rs`；`conv_registry_read_path.rs` 用自己的 `probe_conv`，不受影响）。
+ **两条新守卫**：`demo8_fixture_now_plans_via_the_local_convention` / `demo_fixture_now_plans_via_the_local_convention`——编译产物必须带 `call_layout`（`conv` = `demo8`/`demo`、参数落 A0/A1），**证明夹具真的走在 plan 路径上**而不是继续静默回退。
+ 验证：三个夹具测试二进制（9 + 5 + 3 用例）与全量 workspace 串行套件全绿、clippy `-D warnings` 0、`cargo fmt --check` 0。

### Added (2026-09-27) — v20 A5-3：「调用点 plan」基础设施（形状 → 被调方布局）

**为什么要有**：调用点在 lowering 时只有实参的**形状**（大小/对齐/族/成员），没有被调方的 `Function`；而"第 i 个实参进哪个寄存器/栈槽"只有引擎算得出来（规则 + 绑定 = 使用者的数据）。谱里那份 `[abi.arg_class]`/`ret_regs` 只是这份数据的**生成期近似**——这才是它们删不掉的真正原因。

+ **中性形状**（`forge-isa-runtime::machine::call_layout::ArgShape`/`ShapeKind`）：`Int`/`Ptr`/`Float`/`Vector{elem_is_float, lanes, elem_bytes}`/`Aggregate{members}`/`Other`。运行时 crate 仍**不依赖 forge-abi**。
+ **桥**（`forge-codegen::pipeline::abi_target::plan_for_shapes`）：形状 → `forge_abi::TyView`（聚合成员递归，HFA 判定靠它）→ `Signature` → `AbiRegistry::plan` → `AbiPlan`。
+ **查询注册表**（`forge-isa-runtime::machine::call_plan`）：`CallPlanner` trait（闭包 blanket impl）+ `register_call_planner(isa, …)` / `plan_call(isa, conv, args, ret)`；未注册 ⇒ `None`（生成物 fail-closed，不猜落点）。宿主在 `forge_codegen::pipeline_hooks::ensure_registered()` 里为三个发行后端各注册一个闭包（`Box::leak` 持有自己的 machine）。
+ **两条守卫**（`crates/backend/forge-codegen/tests/abi_target_real.rs`）：① `shape_plan_matches_the_function_plan`——**形状算出来的 plan 必须与函数算出来的逐项相同**（win64 六整数含栈参数 / win64 混合按位置计数 / lp64d 混合按类计数 / aapcs64 整型；比落点/栈/callee-saved/hidden/clobbers，不比参数名）；② `call_planner_registry_serves_the_shape_plan`——注册表路径与直接算逐项相同，未注册 ISA 返回 `None`。
+ 本片**只建能力不改发射**（调用方仍按谱面搬实参），因此三份发行谱零行为变化；下一片切发射后即可删 `[abi].arg_class`/`ret_regs`。
+ 验证：`abi_target_real` 17 绿、workspace 串行全套绿、clippy `-D warnings` 0、`cargo fmt --check` 0。

### Changed (2026-09-27) — v20 A5-3：`[abi].call_clobbers` 删除（破坏集改由宿主的约定级数据给）

+ **谱面删键**：`[abi].call_clobbers`（`Abi` 字段 / schema / 文档键表一并去掉）。它一直是"无 plan 时按参数寄存器 + 返回寄存器猜"的兜底——**猜漏 callee 破坏的临时寄存器就是静默错值**（riscv 递归 fib 死循环的根因）。
+ **正确数据其实与签名无关**：`AbiPlan::clobbers` = 可用池 − callee-saved − pinned，只取决于 `(ISA, 约定)`。于是管线在**本函数规划失败**时用**空签名**问一次引擎（`plan_for_signature` + `Signature::new(vec![], None)`），把结果塞进新的 `LowerCtx::conv_clobbers`（`(类内号, 类)`）。
+ **生成物**：`lowering.rs` 的调用点破坏集改成「有 plan 用 plan 的 `clobbers`；否则用 `ctx.conv_clobbers`；两者都没有 ⇒ **fail-closed**」（不再按谱面猜）。
+ **守卫 ③b**：`abi_target_real.rs` 的四约定交叉核对新增「空签名的 plan 与真实函数 plan 的 `clobbers` 逐项相同」——这条不变式正是回退路径的正确性前提。
+ 验证：workspace 串行全套绿（forge-codegen lib 1344、abi_target_real 15）、clippy `--all-targets --all-features -D warnings` 0、`cargo fmt --check` 0、markdownlint 三份文档 0 issue；三条 JIT 矩阵不变（本改动只在"无 plan"路径上生效，三份发行谱都有 plan）。

### Changed (2026-09-27) — v20 A5-3：`[abi].arg_slot` 迁到 `[machine]`，`AbiPlan` 带上 `position`

+ **`[abi].arg_slot` → `[machine].arg_slot`（机器事实）**：位置计数规则（`by-class` = int/float 各自独立推进；`by-position` = 共享位置游标，Windows x64）是**这台机器的谱面缺省约定怎么数位置**，写给**无 plan 的回退形态**；约定侧的正式位置是 `AbiRules::position`，现在随 plan 一起给出（新增 `AbiPlan::position`，`to_text()` 的 `variadic` 行带上 `position <值>`，四份黄金快照同步）。
+ 读取侧单点化：`IsaModel::machine_arg_slot()`；生成物新增 `TargetABI::arg_placement()`（运行时侧的 `ArgPlacement::{ByClass, ByPosition}`，缺省 `ByClass`，手工后端无需改）；`codegen/{frame,lowering}.rs` 的 `by_position` 标志改读机器事实。
+ **新增守卫（②b）**：`abi_target_real.rs` 的四约定交叉核对新增一条——「plan 的 `position` == 机器事实 `[machine].arg_slot`（生成物的 `arg_placement()`）」；只对 **ISA 主约定**成立（x86 的 sysv64 规则本就是 `by_class`，属 `$equal_cs = false` 的第二种情形）。
+ 三方同改：`schema.rs` 键位移 + 重生成的 `isa-dsl.schema.json` + `docs/reference/isa-dsl.md` 键速查表（`[abi]` 只剩 `arg_class`/`ret_regs`/`call_clobbers`）。
+ 为什么这条能安全先迁：`position` 是**约定级而非签名级**事实，机器事实与规则值必须同值（②b 守卫钉住）；真正需要"调用点 plan"的是 `arg_class`/`ret_regs`（调用方要按**被调方**的落点搬实参）。
+ 验证：workspace 串行全套绿、clippy `-D warnings` 0、四份 `forge-abi` 黄金快照按 `FORGE_ABI_BLESS=1` 重生成后逐行核对（win64 = `by_position`，sysv64/lp64d/aapcs64 = `by_class`）。

### Changed (2026-09-27) — v20 A5-3：机器事实回退键下线（`[abi].{scratch,reserved,call_ret_reg,frame,callee_saved}` 删除）

+ **谱面**：`[abi]` 只剩真正的**约定键**（`arg_class`/`ret_regs`/`call_clobbers`/`arg_slot`）；机器事实一律只在 `[machine]`（`spill_scratch`/`fixed_regs`/`link_reg`/`frame`/`callee_saved_gpr` + `callee_save_slots`）。旧同义键**删除**（`Abi` 字段、`CalleeSaved` 结构体、schema 的 `[abi.frame]`/`[abi.callee_saved]` 两节与键表一并去掉）：写旧键现在报未知键，"两处都写、谁生效"的含糊彻底消失。
+ **读侧单点化**：`machine_scratch`/`machine_reserved`/`machine_link_reg`/`machine_frame`/`machine_callee_saved`/`machine_callee_save_slots` 全部改成**只读 `[machine]`**（`callee_save_slots` 缺省 0，不再由名单长度兜底）。名字解析校验随之只挂在 `[machine]` 键上（原先 `[abi]`/`[machine]` 双份）。
+ **夹具迁移**：`tests/isa/demo8.toml`（`scratch`/`reserved`/`[abi.callee_saved]` → `[machine] spill_scratch`/`fixed_regs`/`callee_saved_gpr`）、`tests/isa/demo.toml`（`[abi.callee_saved]` → `[machine] callee_saved_gpr = []`）、`diag_matrix_tests.rs` 的内联谱（`scratch` → `[machine] spill_scratch`；"未知寄存器名"用例改用仍在 `[abi]` 的 `ret_regs`，保住 `DSL-ABI` 码）。
+ 验证：workspace 串行全套绿、clippy `-D warnings` 0、三条 JIT 矩阵与迁移前同值。

### Changed (2026-09-27) — v20 A5-3：`[abi.stack_args]` 删除（栈参数布局全面由 plan 驱动）

+ **谱面删键**：`[abi.stack_args]`（`callee_base`/`caller_base`/`first_offset_slots`/`stride_slots`/`shadow_bytes`）整节删除，`Abi` 结构体 / schema / `isa-dsl.schema.json` / 文档键表同步。这五个键是**约定事实**，`AbiRules` 里本来就有对应（`shadow_bytes` + `stack.slot_bytes`/`stack.first_offset_slots`），谱里那份只是生成期烘死的常量。
+ **调用方**（`arg_move_loop`）：栈参数 store 改成读 plan——偏移 `CallLayout::caller_offset(k)`（= `shadow + k×槽`）、帧需求用 `shadow_bytes`/`slot_bytes`；门控从"谱里声明过 shadow"换成"**本函数有调用布局**（`ctx.call_layout`）"，没有布局就明确 `Unsupported`（不再按谱面常量猜偏移）。基址恒为 `[machine.frame].sp`（`caller_base` 键随之下线）。
+ **被调方**（`move_args`）：spill 槽收参那条路从谱面算式（`first_offset_slots×slot + shadow + (pos−n)×stride×slot`）改成读 **`ArgPlace::Stack { offset }`**；非 spill 的栈参数本来就走布局路径。
+ **能力申报改由角色说话**：`has_stack_arg` 从"谱里声明了 `shadow_bytes`"改成"声明了 `roles = ["stack_arg_load"]`"。于是"有 load 缺 store"仍是**生成期**点名角色的错误，而"两个角色都没有"只是**不支持栈参数**（生成照旧成功，收参路径运行期 `Unsupported`）；`[machine].spill_scratch` 的要求随之挂在角色上。
+ **规则侧补一条校验**（原先在谱面侧）：`shadow_bytes` 必须是槽单位的整数倍——键搬走时这条不变量不能丢。
+ 守卫：`abi_target_real.rs` 的栈参偏移核对改名为 `stack_arg_offsets_match_the_x86_convention_numbers`（对照 `AbiRules` 的 2/32/8，不再是"对照谱面算式"）；`v12/tests.rs` 的角色用例改成 `stack_args_capability_is_declared_by_roles`（含"删掉 load 角色仍能生成"的反向对照）。
+ **已知边界**（写进 `docs/reference/isa-dsl.md`）：调用方按"每个栈参数一个槽"计数，>8 字节的栈参数今天也不支持（float/vector 超出寄存器数时明确 `Unsupported`）。

### Changed (2026-09-27) — v20 A5-3：`[abi].call_clobbers` 与 `[abi].frame_padding` 两个约定键迁走

+ **`[abi].call_clobbers` 删除**（riscv64/arm64 两谱；x86 本就没写）：调用点的破坏集已全面由 plan 驱动（`AbiPlan.clobbers` = 可用池 − callee-saved，经 `CallLayout.clobbers` 到 regalloc）。谱里那份降级为缺省空、已无人读。
+ **`[abi].frame_padding` → `[machine].frame_padding`（机器事实）**：它由**这台机器的帧机制**决定——x86 是 `push fp` + 7 次 callee-saved 推入 = 8 次推入（偶数）而入口 `rsp ≡ 8 (mod 16)`，故须补 8；riscv/arm64 的 fp-inside 是显式 `sub sp`，故 0。它与 `[stack].align` 同源，归属机器事实。**约定侧**的同名字段仍在 `AbiRules::frame_padding`（win64/sysv64 都 = 8，进 plan）：管线优先读 plan，无 plan 才回退机器事实（与 `callee_save_slots` 同一套路）。
+ 读取侧：`IsaModel::machine_frame_padding()`；生成物 `TargetABI::frame_padding()` 改读 `[machine]`；`Abi` 的 `frame_padding` 字段与 schema 的 `[abi]` 键一并删除 ⇒ 写旧键现在**明确报错**（不再有"两处都写、谁生效"）。
+ 守卫：`abi_target_real.rs` 的交叉核对改成「plan 的 `frame_padding` == 机器事实 `[machine].frame_padding`」（4 份约定逐项核）。
+ 验证：clippy `-D warnings` 0、workspace 串行全套绿、三条 JIT 矩阵不变。

### Changed (2026-09-27) — v20 A6/A5-3：[abi].callee_saved 迁移到 [machine]（键已删）

+ **机器事实两组**：`[machine].callee_saved_gpr = [...]`（帧件会保存的那组 GPR；x86 = RBX/RDI/RSI/R12-R15、riscv64 = 11 个 s 系、arm64 = X19-X28）与 `[machine].callee_save_slots`（推入槽数，可由名单派生）。语义是"**这台机器的帧件会保存这组寄存器**"，与"某份约定**要求**保住哪些"（绑定/plan）分开。
+ **读取侧单点化**：`IsaModel::machine_callee_saved()`（优先 `[machine].callee_saved_gpr`、回退 `[abi].callee_saved.gpr`）与 `machine_callee_save_slots()`；生成物 `TargetRegInfo::callee_saved()` / `callee_save_slots()` 都读机器事实，运行时 trait 的 `callee_save_slots()` 带缺省实现（手工后端无需改）。
+ **删键**：三份发行谱的 `[abi].callee_saved` 段删除。**这一条曾两次实测崩溃**（`test_jit_call_indirect_wide_vector_byref`，`0xC0000005`）——**不是**"无 plan 回退缺名字"（那个说法已作废）：崩溃都发生在**半迁移的树**上，计数（`callee_save_slots` / 生成期 `__cs_bytes` = 7/64）已切到机器事实、名字（`TargetRegInfo::callee_saved()`）却还是空表 ⇒ 帧按 7 个槽布局、regalloc 却以为一个都没被保住，"计数 / 名字 / 消费者"三者不同源。名字与计数**同源**（都读 `[machine]`）之后删键即成立；全套跑完 `FORGE_TRACE_ABI=1` 打印 **0 条** `[abi-plan]`，即测试里没有一次 plan 失败、回退路径根本没被走到。
+ 验证：clippy `--all-targets --all-features -D warnings` 0、workspace 串行全套绿、三条 JIT 矩阵与迁移前同值（x86 195/3、riscv64 131/67、arm64 23/175）。

### Changed (2026-09-27) — v20 A6：x86 序言/尾声改按运行时 callee-saved 列表发射

+ **push 机制运行时化**：序言按 `alloc_result.callee_saved_to_save` 逐个 `push`，尾声 `sp -= n*槽` 取同一列表的运行时长度并逆序 `pop`。列表来自**约定数据**（有 plan 用 plan：显式选 `sysv64` 就只有 5 个，不是谱里那份 win64 的 7 个），无 plan 的夹具退回谱面表。
+ **让位设计（这次能成的原因）**：帧内偏移都是编译期常量、按谱面表长算的，少推 Δ 个槽会让 rsp 抬高 Δ×槽而整体错位（上一轮实测：跨调用读垃圾值 + 访问违例，"只修 `frame_padding`"救不了）。这次由**管线把 Δ×槽补进序言实际分配的字节数**（`pipeline/emission.rs` 的 `cs_skipped`，仅 fp-outside）⇒ rsp 落点与静态表全长时代**逐字节相同**，所有常量与栈对齐继续成立。fp-inside 的保存是帧内槽、不动 rsp（`cs_skipped = 0`）。
+ **两处字节级黄金同步**（`tests/integration_tests.rs`）：空 `AllocResult` 下新语义是"只有 `push rbp` + `mov rbp,rsp`"与尾声 `sub rsp, 0`；注释写清"管线会补 `cs_skipped`，直接调帧件时不会"。
+ 验证：workspace 串行全套绿、clippy `-D warnings` 0、三条 JIT 矩阵不变。

### Added (2026-09-26) — v20 A6：FPR callee-saved 打通（AAPCS64 的 v8-v15 真的会被保存）

承接上一片的"能力"，这一片把它**接上**：

+ **regalloc 两类都喂**：`RegAllocConfig` 拆出 `callee_saved_fpr`（主 FPR 类的编号空间），与 `callee_saved`（GPR 空间）分开——`X8` 与 `V8` 同号，合成一个集合会互相误判。有 plan 时两条都取自 plan（GPR = X19-X28、FPR = V8-V15），无 plan 的夹具退回谱面 GPR 表 + 空 FPR 表。
+ **`callee_saved_to_save` 也扫主 FPR 类表**：只把**实际分配到**的 FPR 加进保存列表（与 GPR 侧同一口径），并按 `p.class.is_fp()` 区分同号寄存器。
+ **fp-inside 的帧字节数按 plan 算**：帧顶槽位公式 `frame - fp_push - (k+1)*slot` 要求帧装得下**整张表**，而这张表随约定而变（AAPCS64 = 10 GPR + 8 FPR = 18 条）。plan 的表 ⊇ 实际保存集，所以按 plan 算必然够用；只按谱面的 GPR 表算会少 64 字节，多存的 8 个 FPR 会写到帧外。**fp-outside（x86）不动**（push 机制仍静态发谱面列表，理由见 `frame_layout.rs` 的注释）；fp-inside 的 `callee_saved_bytes = 0`，所以 spill 寻址不受影响。
+ 守卫：`pipeline/frame_layout.rs` 新增单测——aapcs64 的 plan 必须是 10 GPR + 8 FPR，且 `min_frame = 16 + 18×8 = 160`（只按谱面表算会明显更小）；同时钉住 fp-inside 的 `callee_saved_bytes == 0`、`stack_slot_shift` 不随这条切换而变。
+ 验证：workspace serially 绿（forge-codegen lib **1344** 用例）、clippy `-D warnings` 0、三条 JIT 矩阵不变（x86 195/3、riscv64 131/67、arm64 23/175，0 failed）。

### Added (2026-09-26) — v20 A6（前置）：角色声明带**寄存器类**限定，帧内保存按类分派

+ **角色声明新增类轴**：`roles = [{ role = "callee_save", class = "fpr" }]`（`class = "gpr" | "fpr"`）。同一个"存到帧"能力在 GPR 与 FPR 上要用**不同指令**时分开申报；两个轴（`bits`/`class`）正交，裸声明与该类的声明**可以共存**——裸的那条是另一类的兜底（只有"同一个类声明两条"才是冲突，编译期报错）。唯一性键由 (角色, 位宽) 扩成 (角色, 位宽, 类)。
+ **生成器按类解析**：新增 `role_name_for_class(role, class)`——类限定命中的声明优先，否则回退裸声明；`role_name(role)` 只看裸声明（写了限定的能力必须按轴解析，否则明确报错而不是静默取第一条）。
+ **帧内保存/恢复循环按运行时寄存器类分派**：`callee_saved_loop` 在生成期解析出 GPR/FPR 两条指令，生成物在循环里按 `__preg.class.is_fp()` 选一条。两类解析到同一条指令（或只有一类申报）时**照旧发单条语句**——x86/riscv 与全部夹具的生成物**逐字节不变**（已用 `FGE_DEBUG_GEN` 与改动前的基线逐文件比对：只有 arm64 变了，+456 字节 = 多出的 STURD/LDURD 两支）。
+ **arm64 谱**：`STURD`/`LDURD` 声明 `{ role = "callee_save"/"callee_load", class = "fpr" }`（`STURX`/`LDURX` 保持裸声明管 GPR）。AAPCS64 要求 v8-v15 低 64 位由被调方保存，而 `STURX` 只认 GPR——这一步把"能力"补上。
+ **本片是前置能力，尚未激活**：regalloc 的 callee-saved 仍只喂 GPR（`callee_saved_to_save` 只扫主 GPR 类），所以 FPR 那支今天不会被执行；激活需要"喂 FPR + 帧最小字节数按实际保存数算"（下一步，见计划文档 A6）。
+ 守卫：`crates/frontend/forge-isa-dsl/tests/role_widths.rs` 新增两条——"裸声明 + 类限定声明合法（用 arm64 真谱）"与"同一个类声明两条必须报错"。
+ 验证：workspace serially 绿（clippy `-D warnings` 0）、三条 JIT 矩阵不变。

### Fixed (2026-09-26) — v20 A5-3（④-1）：约定事实改读 plan，顺带修掉两处约定口径错

+ **`CallLayout` 补两个约定事实**：`clobbers`（可用池 − callee-saved）与 `frame_padding`，由 `abi_target::call_layout` 从 `AbiPlan.clobbers` / `AbiPlan.stack.frame_padding` 填。三处消费者改**优先读 plan**，谱里那份降级为无 plan（未注册约定/合成夹具）时的兜底：
  + **regalloc 的 callee-saved 集**（`compiler.rs`）：改用 plan 的 GPR 类项——同一台机器换约定，保存集必须跟着换。**只取 GPR 类**：AAPCS64 的 `cs_fpr`（V8-V15）已在 plan 里，但发射侧还没有按类分派的保存序列（`callee_save` 角色目前只指向 STURX），把 FPR 喂给 regalloc 会让它把跨调用值放进不被保存的寄存器——**类分派保存归 A6**，这条边界写在代码注释与计划里。
  + **调用点的破坏集**（`v12/codegen/lowering.rs` 的 Call/CallIndirect）：`ctx.current_clobbers` 优先取 plan 的**全类** `clobbers`（GPR + FP），无 plan 才用谱里声明的名单。
  + **帧填充**（`frame_layout.rs`）：`frame_padding` 优先取 plan（它是规则的字段、随约定而变）。
+ **修掉一条向量宽度缺陷（第四条消费者切换时暴露，根因在另一处）**：把 plan 的**完整**破坏集交给发射后，x86 的 FP 侧从谱里声明的 4 个 XMM 变成全部 16 个（物理上正确——Win64 的 XMM0-15 全 volatile），`test_jit_v128_byval_return_lane3`（lane3 的 7.5 读成 **0**）与 `test_jit_v128_byval_mixed_int_pos` 随即变红。
  + 根因**不是**破坏集本身，而是**单结果 IR 值一律按"池宽"成类**（`pipeline/lowering.rs`）：V128 也拿 `FPR(8)`，而 `XReg::width()` 就是类宽、spill/reload 与 ABI 回读的搬运宽度都取自它 ⇒ 这个值一旦被 spill 就只搬 8 字节，lane2/3 静默丢。此前靠"幸运合并"（fixup 的 dst 与 src 同为 XMM0，MOVAPS 退化成空操作）掩盖。
  + **修法**：向量值按**真实字节数**成类（同一份 `reg_class_for` 的档位表 → `VEC(16/32/64)`），标量仍走池宽。修完后 plan 的**全类**破坏集可以直接消费（不再按类过滤），守卫 `x86_plan_clobbers_cover_the_whole_fp_file` 钉住"plan 的 FP 破坏集覆盖全部 XMM"这一事实。
+ **顺带修掉的真错（两处，均由新增的交叉核对测试暴露）**：
  1. **`sysv64` 规则缺 `frame_padding`**（缺省 0）。SysV 的入口 `rsp ≡ 8 (mod 16)`，而本实现的序言是 `push fp` + 5 个 callee-saved = **偶数次 push** ⇒ `sub rsp` 前 `rsp ≡ 8`，必须补 8 字节才能在 call 点回到 16 对齐（与 Win64 同值、同理由）。规则补 `frame_padding = 8`，黄金快照 `sysv64.plan.txt` 的 `frame_pad 0 → 8`。
  2. **x86 显式选 `sysv64` 时破坏集漏 RDI/RSI**：谱里的 `[abi]` 是 **win64 口径**（RDI/RSI 是 callee-saved），拿它编 sysv64 的函数时跨调用存活值留在 RDI/RSI 上会被 callee 静默覆盖。改读 plan 后破坏集随约定切换（sysv64 = 7 个含 RDI/RSI，win64 = 5 个）。
+ **刻意保留（写进注释与计划）**：帧上方 **callee-saved 字节数**仍按**谱面**列表数——x86 的 push 机制是静态发射（谱面列表逐个 `push`），改按 plan 计数会在"plan 比谱表短"（sysv64：5 vs 7）时少算 16 字节，让局部/spill 槽与 push 槽重叠。要一起换须先把 push 机制改成运行时按 `alloc_result.callee_saved_to_save` 循环（帧字节数也随之变运行时值）——归 A6。
+ **新增交叉核对测试**（`crates/backend/forge-codegen/tests/abi_target_real.rs`，4 份约定）：`x86_win64`/`x86_sysv64`/`riscv64_lp64d`/`arm64_aapcs64` 逐项核对「plan 的 callee-saved vs 谱表」「plan 的 frame_padding vs 谱」「clobbers 的两条硬不变量（与 callee-saved 不交；参数寄存器与返回寄存器都在里面）」。相等的那三份断言**逐项同值**，sysv64 断言的是**预期差异**（plan ⊂ 谱表且严格更小）——这是"把三处消费者切到 plan"的入场券。
+ 验证：workspace 全套 serially 绿、clippy `--all-targets --all-features -D warnings` 0。

### Added (2026-09-26) — v20 A5-3（②b）：`[machine.frame]` 帧形状，三份发行谱迁完

+ **`[machine]` 增 `frame` 子表**（`sp`/`fp`/`layout`/`fp_push_bytes`/`alloc_neg`，键与旧 `[abi.frame]` 同名同义）：这五项是"这台机器怎么建帧"的**机器事实**，不是约定内容（保存谁、栈参数怎么排才是约定）。读侧统一走 `IsaModel::machine_frame()`——`[machine.frame]` 优先、回退 `[abi.frame]`，**所有**读者（含只取 `fp` 或只取 `fp_push_bytes` 的）都经它，避免迁移期"一半读新表、一半读旧表"的分裂。
+ **读者改写**（全部改为 `machine_frame()`）：`abi_view` 的 pinned 视图、`integration.rs` 的 sp/fp 解析 + 帧是否声明、`frame.rs` 的 layout/fp_push_bytes/spill 缺省 base/栈参数收参基址、`lowering.rs` 的 `frame_base_toks`/`sp_base_toks`（sret/by-ref/栈参数 store 的基址）、`validate.rs` 的 sp 非空与寄存器名检查。错误消息与文档字符串同步改点名 `[machine.frame]`。
+ **谱**：`isa/{x86,riscv64,arm64}_v12.toml` 的 `[abi.frame]` 段移入 `[machine.frame]`（值原样）；两份夹具 `crates/backend/forge-codegen/tests/isa/{demo,demo8}.toml` 与 DSL 内联夹具（`frame_sp_*` 两条反向用例）一并迁到新键——它们现在**真的走新路径**，回退路径只服务外部旧谱。
+ **三方守卫**：`schema.rs` 增 `[machine.frame]` 节（`sp` 必填）与 `[machine].frame` 键、`docs/reference/isa-dsl.md` 键表与 `## [machine.frame]` 一节的迁移说明、重新生成的 `isa-dsl.schema.json`；`docs/reference/calling-conventions.md` 的能力视图来源改述。（`[abi.frame]` 仍在 schema/校验里，作为迁移期回退路径。）
+ 验证：workspace 全套 serially 绿（130 个测试二进制 0 失败）、clippy `--all-targets --all-features -D warnings` 0；本地与 CI run 213（`983b8ee`）一致——CI 11 项里 10 项绿，唯一红的是既有慢性项 `forge-rustc (e2e, Windows)`。

### Added (2026-09-26) — v20 A5-3（①+②）：`[machine]` 机器事实层，三份发行谱迁完

+ **新增 `[machine]` 段**（只描述"这台机器是什么样"）：`fixed_regs`（regalloc 不可分配，原 `[abi].reserved`）、`spill_scratch`（溢出与栈参数收参的临时寄存器，原 `[abi].scratch`）、`link_reg`（call 写返回地址的寄存器，原 `[abi].call_ret_reg`）。键名**故意与旧键不同名**——两处都写时"谁生效"必须无歧义；`[machine]` 优先、回退 `[abi]` 旧键，所以迁移期两种写法都认，可以逐谱迁移。
+ **读取侧单点化**：所有读者改走 `IsaModel::machine_scratch()/machine_reserved()/machine_link_reg()`——`abi_view` 的静态能力视图、`integration.rs` 的 `RegInfo` scratch/reserved、`frame.rs` 的栈参数收参 scratch + 帧件保存 link、`lowering.rs` 的 `call`/`call_indirect` 返回地址槽。`validate` 对 `[machine]` 与 `[abi]` 两处都校验寄存器名（必须落在某个 `[reg.*]` 组内）。
+ **三条 fail-closed 错误消息改点名新键**（`[machine].spill_scratch` 未声明却要走栈参数收参、`[machine].link_reg` 未声明却有 Out/InOut Reg 槽），两条负向用例（`codegen_requires_scratch_for_stack_args` / `codegen_requires_call_ret_reg_when_call_has_ret_slot`）同步改成新键——它们仍钉着"不按某个 ISA 的寄存器名兜底"。
+ **三份发行谱迁移完毕**（`isa/{x86,riscv64,arm64}_v12.toml`）：`[abi].scratch`/`reserved`/`call_ret_reg` 删除，值**原样**进 `[machine]`（x86 `R10/R11`、riscv `X5/X6` + `X0/X1/X3/X4` + `X1`、arm64 `X16/X17` + `X18/X30` + `X30`）。
+ **三方一致守卫同步**：`schema.rs`（根键 + `[machine]` 节）、`docs/reference/isa-dsl.md` 键速查表与新增 `[machine]` 一节、重新生成的 `isa-dsl.schema.json`（`schema_guard` 6 项全绿）。
+ 验证：workspace 全套 serially 绿（clippy `--all-targets --all-features -D warnings` 0）；两条 JIT 矩阵与迁移前**逐条同值**——x86 **195 passed / 3 skipped**、riscv64 **131 passed / 67 skipped**，0 failed（scratch/reserved/link 三条路径都被矩阵跨调用用例实际走到）。
+ 仍待做（A5-3 ③/④）：约定事实（`arg_class`/`ret_regs`/`arg_slot`/`stack_args`/`callee_saved`/`call_clobbers`/`frame_padding`）移入 `AbiRules`/`AbiBinding`、demo 夹具补测试本地绑定、发射/管线改读 `AbiPlan`，然后删除 `[abi]` 的约定键与本次保留的回退路径。

### Added (2026-09-25) — v20 A5（续）：AAPCS64 的 v8-v15 进 callee-saved（引擎侧）

+ **规则/绑定**：`aapcs64` 的 `callee_saved.pools` 加 `cs_fpr`，绑定补 `cs_fpr = ["V8".."V15"]`。
  效果：`abi plan` 的 `callee_saved` 现在含 `V8..V15`，`clobbers` **不再**把 v8-v15 列为被破坏
  （AAPCS64 要求被调方保它们低 64 位）。黄金快照 `aapcs64.plan.txt` 随之更新。
+ **如实标注的边界**：这一步只在**引擎/计划侧**成立。**发射侧还没按寄存器类分派保存指令**
  ——`callee_save` 角色目前唯一指向 `STURX`（GPR 视图），而 regalloc 的 callee-saved 列表来自
  谱里的 `[abi.callee_saved]`（不含 FPR），所以**行为不变、且偏保守**（regalloc 仍把 v8-v15
  当跨调用不安全）。把帧件的保存/恢复按类分派（FPR 走 STURD/LDURD）需要 role 能带类限定，
  归 A6；缺口已写进绑定注释与计划文档，不留含糊。

### Added (2026-09-25) — v20 A5（子集）：arm64 浮点能力——FPR 组 + 浮点搬运角色 + 绑定补池

+ **谱**（`isa/arm64.toml`）：新增 `[reg.fpr8]`（`V0..V31`）、`fpr` 操作数槽、`FMOVR` 形式，以及 6 条指令——`FMOV_S`/`FMOV_D`（`fpr_mov` 32/64）、`LDURD`/`STURD`/`LDURS`/`STURS`（SIMD&FP 的 unscaled 访存，编码与 `LDURX`/`STURX` 同形：`0xFD…`/`0xBD…` + `opc2`），外加 `[spill.FPR]`（D 寄存器溢出）。
+ **绑定**（`conventions/aapcs64-arm64.toml`）：补 `float = ["V0".."V7"]`、`ret_float = ["V0".."V3"]`（HFA ≤4 返回落连续 v0-v3），并删掉"arm64 没有 FPR 组"的缺口说明。
+ **效果**：`forge-isa abi check --strict isa/arm64.toml` 的缺口 **6 → 1**（只剩 HFA4 *返回*搬运那条"≥3 槽见 A6"的引擎侧限制）；`abi plan` 现在给出 `f64 → V0`、`i64 → X0`、`ret → V0`（此前是 `MissingPool`）。arm64 的浮点/HFA **参数**与 ≤2 槽返回自此有寄存器可落，发射侧也拿得到 `fpr_mov` 角色（收参走 `FMOV_S`/`FMOV_D`）。
+ **刻意保留的取舍/缺口**（如实登记）：① FP 寄存器统一命名 `V0..V31`（不像 x86 那样名字自带宽度），因此 `fmov v0, v1` / `ldur v0, [x29,#8]` 的 S/D 两种编码**汇编文本相同** ⇒ 反汇编按声明序取 S；这条已知歧义写进了 `spec_coverage_guard.rs` 的钉死名单；② 暂无 `cs_fpr` 池 ⇒ `clobbers` 保守地把 v0-v31 全列为被破坏（安全方向）；③ arm64 的浮点**算术**尚未接线（矩阵里的 F64 用例仍按 ops 未覆盖 skip）。
+ **守卫同步**：`spec_coverage_guard`（arm64 指令总数 104 → **110**、歧义名单 +6）、`isa_roundtrip_guard`（arm64 派生条目 332 → **360**）、`forge-abi` 黄金快照 `aapcs64.plan.txt`（clobbers 含 V0-V31）；合成目标 `common::arm64()` 随之带上 FPR 组，`missing_pool_is_explicit_not_silent` 改为**自备缺池绑定**（测"缺池的报法"，不依赖 arm64 有没有池）。

### Added (2026-09-25) — v20 A3b-2b-2c-2：栈落点接进布局驱动的收参

+ `@move_args` 的布局路径新增 `ArgPlace::Stack { offset }` 分支：栈参数从
  `[frame_base + offset]` 收进分配的寄存器（指令仍按角色 `stack_arg_load` 取），
  **偏移完全来自 forge-abi**（`first_offset_slots + shadow + k×slot`，上一片刚修正）。
  `__layout_ok` 的入场判定也随之接受 `Stack` ⇒ 带栈参数的函数（第 5+ 个参数）不再因为
  "有一个栈落点"而整函数退回 `[abi]` 位置算式——它们的寄存器参数也走布局。
+ 被 regalloc 强制 spill 的栈参数仍由既有的 spill 收参块处理（同一套数值；其"布局偏移 ==
  旧算式"由 `abi_target_real::stack_arg_offsets_agree_with_the_legacy_formula` 钉住）。
+ **浮点参数走栈**的收参**明确拒绝**（`Unsupported`）而不是当整数搬进 GPR：旧路径同样只走
  GPR 搬运，这是这条路上的旧缺口，接它属于后续（需要按宽度分派的 FPR load）。
+ 仍未接：`Pair`/`Group`/无指针的 `Indirect`（整函数回退，边界在守卫测试里）。
+ 验证：workspace 全套 serially 绿（含两条 JIT 矩阵；`Args` 用例覆盖 5+ 参数）。

### Fixed (2026-09-25) — v20 A3b-2b-2c-1：`Placement::Stack.offset` 漏算 shadow（被调方读栈参数会读错）

+ **模型 bug**：被调方视角的栈参数偏移只算了 `first_offset_slots × slot` + 参数区位置，**漏了 `shadow_bytes`**。调用方把第 k 个栈参数写在 `[sp + shadow + k×slot]`，被调方读同一实参时要加上"返回地址 + 保存的帧指针"**以及** shadow——win64 第 5 个参数因此被算成 `off=16`，而现有发射算式是 `48`（差的就是那 32 字节 shadow）。
+ **发现方式**：`forge-codegen/tests/abi_target_real.rs::stack_arg_offsets_agree_with_the_legacy_formula`（新增）——把布局的 `Stack { offset }` 与**现有发射路径**的 `[abi.stack_args]` 算式逐参数对照。这条测试现在是"把栈参数收参切到布局"的入场券：偏移差一不是崩而是**静默错值**，矩阵未必抓得到。
+ **修法**：`engine::stack_place` 的落点偏移加上 `rules.shadow_bytes`，并把 `Stack.offset` 的语义（被调方视角、已含 shadow）与两个视角的固定关系 `Stack.offset(k) − caller_offset(k) = first_arg_offset` 写进参考文档与代码注释。
+ **影响面**：只有 `shadow_bytes != 0` 的约定受影响——`win64` 黄金快照的栈偏移整体 +32（`sysv64`/`aapcs64`/`lp64d` 的 shadow 为 0，**逐字节不变**）；`invariants.rs` 的"参数区装得下最后一个字节"随之按新口径断言；workspace 全套 serially 绿。
+ 发射侧**尚未消费** `Stack` 落点（栈参数收参仍走 `[abi.stack_args]` 算式）⇒ 运行时行为不变，`Stack` 接进布局路径是 A3b-2b-2c-2。

### Changed (2026-09-25) — v20 A4：序/尾声由生成器生成，伪指令全部删除（谱只剩裸指令）

+ **设计裁定（用户）**：谱只描述**裸指令**（形状 + 编码 + 能力角色），函数调用平衡（保存谁、帧多大、怎么建立帧指针）交给**调用约定**层；角色是"这条指令能充当什么"的**能力声明**，不是给指令加的副作用。
+ **删除**：`[emit.prologue]`/`[emit.epilogue]` 与四个伪指令（`@push_callee`/`@pop_callee`/`@frame_alloc`/`@frame_free`；`@move_args` 更早已删）。`[emit]` 只剩 `align_pad`/`epilogue_label` 两个机器事实。写回去会被明确拒绝（`DSL-TOML`：新键不存在），并有反回潮守卫 `crates/frontend/forge-isa-dsl/tests/call_layout_emission.rs`（三谱 + 夹具里不许再出现这些键/伪指令）。
+ **新增三个能力角色**：`frame_set`（`in`=源、`out`=目标，可带 imm：x86 `mov rbp, rsp` / `mov rsp, rbp`、riscv `addi x8, x2, frame`、arm64 `addimmx x29, sp, frame`）、`callee_save` / `callee_load`（`Reg[0]`=值、`Reg[1]`=基址、`Imm[0]`=偏移：riscv `SD`/`LD`、arm64 `STURX`/`LDURX`）。复用 `push`/`pop`/`frame_alloc`/`frame_free`/`ret`。
+ **生成器的规范序列**（顺序由**机制**决定）：push 机制（x86）= `push fp` → `frame_set(fp←sp)` → 逐个 `push` callee-saved → **收参** → `frame_alloc`；尾声 = `frame_set(sp←fp)` → `sp -= callee-saved 区` → 逆序 `pop` → `pop fp` → `ret`。帧内机制（riscv/arm64）= `frame_alloc` → 存 link（`[sp+frame-8]`）→ 存 fp（`[sp+frame-fp_push]`）→ `frame_set(fp←sp+frame)` → 逐个 `callee_save`（`[sp+frame-fp_push-(k+1)*slot]`）→ **收参**；尾声镜像 + `frame_free` + `ret`。三条不变量：保存早于收参、帧内保存槽只在分配之后写、尾声与序言同源（逆序恢复）。
+ **缺角色的步不发射**（角色 = 能力申报）；`ret` 例外：尾声**必须**有 `roles = ["ret"]`，否则明确报错。
+ **验收 = 三份发行谱的生成物逐字节不变**：`FGE_DEBUG_GEN=1` 对照改动前后，x86/riscv64/arm64 生成模块 **SHA256 全同**（序/尾声的每一条指令、字段顺序、立即数形态都对得上）；workspace 全套 serially 绿（含两条 JIT 矩阵）。
+ **两处有意为之的行为变化**：① demo 夹具声明了 `frame_alloc`/`frame_free` 却从没分配过帧 —— 现在按声明发射（`frame_size == 0` 时守卫不发，故测试行为不变）；② x86 尾声的 `sp -= callee-saved 区` 仍是静态表长（与 push 侧同源），"少保存时错位"的隐患留在 A3b-2b-2c/后续统一。
+ **代价**：谱再也无法给函数插入任意序言步骤（vzeroupper/栈探测/GOT 建立）——需要时**加角色**（上层可见的能力），不回到自由模板。

### Changed (2026-09-25) — v20 A3b-2b-2b：`@move_args` 退役（收参由生成器发射，谱不再定义调用约定）

+ **伪指令白名单里删掉 `move_args`**：谱里再写 `@move_args` 会被**明确拒绝**并给出迁移提示（单列一条诊断，不是含糊的"未知伪指令"）——它是**调用约定**的内容，谱不该写它。
+ **生成器自动插入收参**：位置 = 最后一个 `@push_callee` **之后**（模板里没有 `@push_callee` 则放最后），两者都在所有 callee-saved 保存之后。**顺序不是风格问题**：保存若晚于收参，存下来的是实参值而不是调用者的寄存器值，尾声恢复会毁掉调用者的寄存器。
+ `[emit.prologue]` 因此**可以缺席**（`gen_emit_block` 不再因模板缺失而整体早退）：缺席 = 空模板 + 收参；demo 夹具的整段 `@move_args` 模板随之删除。
+ 三份发行谱（x86/riscv64/arm64）与两份夹具的模板里删掉 `@move_args`；其余四个伪指令（`@push_callee`/`@pop_callee`/`@frame_alloc`/`@frame_free`）仍在，A4 才轮到它们。
+ **验收 = 生成物逐字节不变**：`FGE_DEBUG_GEN=1` 对 10 份生成模块（三份发行谱 + 七份夹具/多文件谱）取样，改动前后 **SHA256 全部相同**；workspace 全套（serially）绿。
+ 守卫 `forge-isa-dsl/tests/call_layout_emission.rs` 新增三条：写 `@move_args` 必报错且提示指路、缺席序言模板的谱仍发射收参、收参位置在保存之后且帧分配之前。

### Added (2026-09-25) — v20 A3b-2b-2a：缺省约定 `c` 的绑定 + 收参改读调用布局

+ **前提缺口**：IR 的缺省约定是抽象名 `c`，而内置绑定只有 `win64`/`sysv64`/`aapcs64`/`lp64d` ⇒ `c` 在真机上没有绑定 ⇒ `call_layout` 恒为 `None`、生成物的布局路径永远不生效。补法是把它变成**数据**：`AbiRules` 新增 `aliases = ["c"]`（"这台机器的 C 就是本约定"），内置 `win64`/`aapcs64`/`lp64d` 声明、`sysv64` 刻意不声明。**关键在"整套代答"**：解析出的必须是同一份约定的**规则 + 绑定**（`AbiRegistry::resolve_conv`）。实测教训——只让**绑定**代答（拿 `c` 自己的通用规则配 Win64 的寄存器）会得到 by_class 的槽位与 RCX 返回：混合 int/float 参数读错、`sret` + by-ref 槽位错位，三个 JIT 用例当场红。纪律：同一机器上两份约定都声称代答 ⇒ 报错（不按注册序猜）；显式 `(ISA, "c")` 绑定/规则优先于别名（宿主覆写入口）。
+ `@move_args`（被调方收参）改读 `AllocResult::call_layout`：来源寄存器取自布局（`ArgPlace::Reg` 的 **(类, 类内号)**——int 类走 `gpr_mov`、浮点/向量类按类宽分派 `vec_mov`/`fpr_mov`），宽向量 by-ref 走 `ArgPlace::Indirect`。生成器不再自己数"第几个 int 槽"、不再自己算 `sret` 偏移（这两件事现在全由绑定/规则决定）。
+ **入场判定 `__layout_ok`**：只有**每个**形参都落在本片覆盖的落点时才启用；有一个不支持（`Pair`/`Group`/`Stack`/无指针的 `Indirect`）就**整函数**退回既有 `[abi]` 路径（两条路径不混用——混用会让旧路径的 `__gi`/`__fi` 游标错位）。`call_layout = None`（无绑定/规划失败）同样退回。
+ 守卫与实测：新增 `forge-isa-dsl/tests/call_layout_emission.rs`（三份发行谱都发射布局路径、且排在旧路径之前、只按寄存器**类**分派）；`abi_target_real.rs` 新增"缺省 `c` 也能规划并到达帧件"；**x86 矩阵 195 passed / 3 skipped / 0 failed**、**riscv64 矩阵 131 passed / 67 skipped / 0 failed**、`forge-codegen` 全套绿 ⇒ 收参换来源后行为不变。
+ 仍未切：栈参数 load/store、`byval` 副本、序尾声（`@push_callee`/`@frame_alloc`/`{callee_saved_bytes}`）仍读 `[abi]`（A3b-2b-2b / A4）。

### Added (2026-09-25) — v20 A3b-2b-1：序/尾声侧拿到调用布局（`AllocResult.call_layout`）

+ **结构事实**：`TargetFrameLowering::emit_prologue/epilogue` 只拿到 `frame_size` + `AllocResult`、拿不到 `LowerCtx`，而 `@move_args`（收参）是生成 lowering 的一部分、有 `LowerCtx`。所以"生成物改读 plan"分两条通路，序/尾声这条必须先把布局放进 `AllocResult`。
+ `AllocResult` 新增 `call_layout: Option<CallLayout>`（默认 `None`），管线在收尾处从 `LowerCtx::call_layout` 填入；`None` = 没接上/算不出 ⇒ 帧件走既有 `[abi]` 路径。
+ 测试（`tests/abi_target_real.rs`）：钉住管线确实把布局带到帧件（conv/shadow/首个实参 `RCX=(GPR(8),1)`/callee-saved 非空）。
+ 发射仍未切换 ⇒ 生成物逐字节不变。

### Added (2026-09-25) — v20 A3b-2a：中性调用布局 `machine::call_layout`（发射切换的地基）

+ `forge-isa-runtime` 新增 `machine::call_layout`：把"一次调用长什么样"变成**中性数据**（`CallLayout`：逐参落点 / 返回 / 隐藏 sret / 栈区尺寸 / callee-saved / 被叫方弹栈 / 红区 / 扩展位数）。寄存器用 **(类, 类内号)** 表示（`RegClass` 是 forge-ir 的中性类型），生成物 `Reg::from_index(i, class)` 即可还原——**运行时因此不需要依赖 forge-abi**。
+ `forge_codegen::pipeline::abi_target::call_layout(plan, machine)`：`AbiPlan` → `CallLayout`（ABI 空间号折回类内号、`sret` 标记打到落点上）；管线把它塞进 `LowerCtx::call_layout`。
+ 测试（`tests/abi_target_real.rs`，真机 x86）：RCX=(GPR(8),1)、XMM1=(FPR(16),1)、返回 RAX=(GPR(8),0)、callee-saved 含 RBX=(GPR(8),3)、`caller_offset(k)=shadow+k*slot` 全部核对。
+ **发射尚未切换**（生成物读的仍是 `TargetABI`）⇒ 生成物逐字节不变；这一步是"生成物改读 plan"的地基。

### Added (2026-09-25) — v20 A3b-1：调用计划接进编译入口（只算不用，行为不变）

+ `CompileState` 在编译入口用 A3a 的宿主适配器把该函数的 `AbiPlan` 算出来挂上（`abi_plan`/`abi_plan_note`）；算不出来**不阻断编译**（发射还没用它），原因留档。`FORGE_TRACE_ABI=1` 打印计划的确定性文本或失败原因——发射尚未切换期间这是"计划长什么样"的唯一证据面。
+ **前置核对**（`tests/abi_target_real.rs` +1）：引擎的计划必须与**当前**发射路径的 `AllocResult` 一致——sret 有无、**逐参数** by-ref 判定、栈参数区字节数（`arg_area − shadow`）。这条一致性检查是把发射切到 plan 上的入场券：不一致就说明"适配器/数据/现有路径"有一边错了。
+ 本片**不改发射** ⇒ 生成物逐字节不变（`cargo test --workspace` 全绿、clippy `-D warnings` 干净）。

### Added (2026-09-25) — v20 A3a：宿主适配器（真实后端 → 引擎），ISA 正式申报能力

+ **`TargetMachine::role_bits(role) -> Option<u16>`**（`forge-isa-runtime` trait，默认 `None`）：ISA 向宿主**申报能力**的出口。DSL 从谱里的 `[[instructions]].roles` 生成 `match role { "gpr_mov" => Some(64), … , _ => None }`（按能力名折算，与 `forge-isa abi check` 的静态视图**同源**——都走 `abi_view::role_capability`，因此"谱里写了什么"与"宿主申报了什么"不会漂移；生成物里不留名字表 + 线性查找）。
+ **`forge_codegen::pipeline::abi_target`**：`MachineAbiTarget` 把真实 `TargetRegInfo` 喂给 `forge-abi` 引擎（索引空间 GPR 区 + FP 区、名字用生成枚举的变体名、`pinned` = 不在可分配表、能力走 `role_bits`），并提供 `plan_for_function`/`plan_for_signature`（IR 签名 + 约定数据 → 真机 `AbiPlan`）。
+ **交叉核对**（`tests/abi_target_real.rs`，3 条）：真机 x86 上 `win64` 的同一签名给出与 A1 合成目标**相同的落点**（第 1 个整数 RCX、第 2 个浮点 XMM1、标量返回 RAX、shadow 32、callee-saved/clobber 来自真表），未注册约定在真机上同样 fail-closed——即"适配器 + 内置绑定 + 引擎"三方对得上。
+ 说明：本片**只算不算用**（`AbiPlan` 供校验与诊断），发射仍走老路 ⇒ 现有编译行为不变；按 plan 发射调用点/入口/序尾声是 A3b（验收：x86 生成物逐字节不变）。

### Added (2026-09-24) — v20 A2b：声明属性（`byval`/`sret`/`inreg`/`zeroext`/`signext`/`align`）真的改变规划

+ **引擎吃声明属性**：`forge_abi::Signature` 新增 `attrs`/`ret_attrs`（`DeclAttrs`），`plan_fn` 按属性改分类与落点——`byval(N)` → 该形参变"调用方栈上 N 字节副本 + 指针"（副本进 `StackLayout::byval_area_bytes`）、`sret` → 该形参占约定声明的 **hidden sret 槽**（x86 RCX/RDI、AAPCS64 **x8**、riscv a0；不再按普通参数分类）、`inreg` → 分类说要走栈时再试寄存器（池空仍走栈，不硬凑）、`zeroext`/`signext` → 落点带 `Extension`（都写时 `signext` 胜，与 LLVM 一致）、`align(N)` → 栈落点对齐抬到 N。
+ **IR 侧接上**：新增 `forge_codegen::pipeline::sig_view`——`Function::param_attrs`/`ret_attrs` + `FunctionSignature` + `TypeStore` → `forge_abi::Signature`。`ParamAttributes`（`byval`/`sret`/`inreg`/`zeroext`/`signext`/`align`…）此前**只有文本层认识、没有任何代码读**（与刚删掉的旧 `CallConv` 一样是装饰），现在它是规划的输入。类型投影只摊开不猜测：大小/对齐取 `TypeStore` 的权威值（**带填充的结构体不能按成员求和**），摊不开的类型（可扩展向量）落 `TyKind::Other` 交给规则兜底。
+ 测试：`forge-abi/tests/invariants.rs` +2（`declared_attributes_change_the_plan` 逐条断言产物：`byval` 变间接落点且副本区按 N 算、`sret` 在两个约定上各就各位且不与后续参数撞号、两个 ext、`align` 抬对齐；`inreg_overrides_a_stack_classification` 覆盖"池够"与"池耗尽"两种行为）；`forge-codegen/tests/conv_registry_read_path.rs` 的投影用例（含"带填充结构体大小"这条反例）。`AbiPlan::to_text()` 只打印**产物**（`ext=` / `on_stack=` / `align=`），所以"属性生效没有"在快照里一眼可见。
+ 下一步（A3）：宿主 `AbiTarget` 适配器（`TargetRegInfo` → 引擎）把签名真正跑成 `AbiPlan`，并按 plan 发射调用点/入口/序尾声（x86 优先、生成物逐字节不变为验收）。

### Changed (2026-09-24) — v20 A2：IR 里的调用约定从"死枚举"变成"使用者命名的标识 + 宿主注册表"（破坏性）

+ **删掉 16 变体的 `CallConv`，换成 `CallConvId`**（`forge-ir`）：`Builtin(ConvName)`（`c`/`sysv64`/`win64`/`aapcs64`/`lp64d`）、`Named(ImmStr)`（**使用者自己注册的名字**，开放集合）、`Index(u32)`（LLVM `cc N`）。默认 = `Builtin(C)`，文本层对 `c` 不写关键字（与 LLVM 一致）。旧枚举里 `SystemV`/`Fast`/`Cold`/`PreserveAll` 那些变体既不是公共约定、也没有任何代码读——那是"用枚举假装支持一切"。
+ **未知约定不再被折成不可还原的占位值**：文本层原先把 `amdgpu_cs_chain` 这类名字编码成 `Custom(len ^ 0x8000_0000)`（既打印不回原样、也没人读），现在 `Named(ImmStr)` 原样保留；`to_text`/`from_text` **共用一张表**（`win64cc`/`fastcc`/`cc 42` 等 LLVM 拼写 = 同一份数据），往返不会漂移。
+ **新增真实读路径**（旧实现 `ctx.call_conv` 只写不读，等于文档）：`forge-codegen::pipeline::conv_registry` 是宿主注册表（内置五份 + `register_rules_toml`/`register_binding_toml` 加自己的），编译入口 `CompileState::new` 把 `CallConvId` 解析成注册表键（`Index(n)` → `"cc{n}"`）落在 `LowerCtx::call_conv_name`；**未注册一律 fail-closed**（报 `Unsupported` 并列出已注册的名字，**不**退回缺省约定——`CallConv::Default` 就是这么变成死值的）。A3 起同一个名字用来查 `AbiRules`/`AbiBinding` 发射调用点。
+ **二进制格式升到 `IR_FORMAT_VERSION = 3`**：签名体里那 1 字节判别值改成 tag（`0..=4` 内置 / `5` 命名（字符串表下标）/ `6` 数值（varint））；`c` 仍是 1 字节，因此字节偏移类的手工解码测试不受影响。**无兼容读取**（设计前提就是破坏性更新）：v2 字节流在头部即报版本不符；文档 `docs/reference/binary-format.md` 同步（标题 v3 + 版本历史表）。
+ 测试：`crates/backend/forge-codegen/tests/conv_registry_read_path.rs`（内置五份可解析；未注册的 `Named`/`Index` 都 fail-closed 且消息能指路；**端到端**"未注册时编译失败 → 注册后同一份 IR 编译通过"）。`forge-ir` 的文本/二进制/往返测试全部因共用表而继续绿（`fastcc`/`win64cc`/`cc 10` 的原样往返）。
+ 说明：`forge-rustc` 从不引用这个类型（前端也从没填过它）——这正是"死值"的另一半原因；A2b 会让前端把 `PassMode` 落到 IR 属性上。

### Added (2026-09-24) — 调用约定层 `forge-abi`（v20 A1：约定是使用者的数据，ISA 只申报能力）

+ **新 crate `crates/foundation/forge-abi`**（无内部依赖）：把调用约定拆成三层数据 + 一个通用引擎——`AbiRules`（约定，平台无关的 TOML，可继承）→ `AbiBinding`（`(ISA, 约定)` 的寄存器绑定）→ `AbiPlan`（引擎产物，**调用方与被调方共用**：每个实参/形参的落点、返回值、栈布局、callee-saved、hidden 槽、clobber）。内置约定 5 份（`c` 抽象基类 + `win64`/`sysv64`/`aapcs64`/`lp64d`）与参考绑定 4 份（`crates/foundation/forge-abi/conventions/*.toml`，含"为什么这么绑"的注释）。引擎只认注册表里注册过的约定名，未注册 ⇒ 明确报错（**不**退回缺省——旧设计里 IR 的 `CallConv::Default` 就是这样变成死值的）。
+ **修掉四处会写错值的模型缺口**（都是"内置数据跑起来"才暴露的，`cargo check` 看不见）：① 同质浮点聚合（HFA）判定不分整数/浮点 ⇒ `struct{i64,i64}` 会进浮点寄存器（RISC-V 应走 a0:a1）；② HFA 槽数写死 ⇒ `struct{f32}` 白吃 4 个寄存器、把后面的参数挤到栈上，改为 `slots = "hfa"`（按类型取成员数）；③ 参数位与返回位共用分类规则 ⇒ AAPCS64 的 >16B 聚合"当参数是 byval、当返回是 **x8** sret"被混成一条，新增 `ret_classify`（非空即独占）；④ 返回寄存器与参数寄存器共用池（x86 返回在 RAX、参数从 RCX 起）⇒ 拆成 `ret_int`/`ret_float` 两套池 + 两个独立游标（按位置计数时一个 2 槽返回不再把第一个参数挤走）。
+ **另修三处**：byval 副本改在**独立的调用方临时区**（`StackLayout::byval_area_bytes`；混进传出参数区会与栈参数抢内存）、`ClassRule` 的 TOML 键 `do` 缺 `#[serde(rename)]`（会让 5 份内置约定整片解析失败，`cargo check` 发现不了）、新增 `Placement::RegGroup`（AAPCS64 的 4×f32 HFA 需要 4 个连续槽；返回位本片明确 `Unsupported`）。
+ **ISA 侧能力视图 `forge_isa_dsl::abi_view`（新公开模块）**：从谱读"这台机器有什么"——寄存器表（GPR 区 + FP 区编号与 `TargetRegInfo::num_gp_regs`/`num_fp_regs` 对齐，别名解析到同一物理号）、固定用途寄存器（`[abi].reserved` + sp/fp）、链接寄存器（`[abi].call_ret_reg`）、角色→能力。**不读调用约定**（那是使用者的数据），也不暴露整个模型。
+ **CLI `forge-isa abi list|check|plan`**：新增 `abi` 子命令与 `forge-abi` 依赖。`abi check <谱>` 用能力视图 × 内置绑定跑 15 条代表签名，**硬错**（`UnresolvedReg`/`BadRules` = 名字或约定写错）退出 1、**缺口**（`MissingPool`/`Unsupported`/`CapabilityGap` = 这台机器做不了，fail-closed）默认只报 `⚠ GAP`、`--strict` 才影响退出码；`abi plan` 打印一份 `AbiPlan`（确定性文本）。**实测**：x86 `win64`/`sysv64` 与 riscv64 `lp64d` 各 15 条**全绿**；arm64 `aapcs64` **6 条缺口**（谱里没有 FPR 寄存器组 ⇒ `float`/`ret_float` 池缺 ⇒ 浮点/HFA 无寄存器可落，与矩阵 175 条 skip 同源，A5 关闭）。
+ 测试：黄金快照 4 份（四约定 × 21 条语料，`FORGE_ABI_BLESS=1` 重新生成）+ 不变量 18 条（位置计数/按类计数、整数聚合不是 HFA、单成员 HFA 只占一槽、4 成员 HFA 用 `RegGroup`、寄存器不重复记账、栈参数不重叠、sret 逐约定各就各位、变参未命名实参、钩子覆盖分类）+ fail-closed 目录 14 条（未注册约定/缺绑定/名字写错/空池/非 2 的幂/未知族名/无 `va_list`/能力缺口/池耗尽/≥3 槽返回）+ CLI 端到端 5 条。文档：`docs/reference/calling-conventions.md`（现行参考）+ `docs/plans/calling-convention-redesign-plan.md`（A1–A7 分期）。
+ **顺带修一个跨平台测试坑**（CI 的 `Test (Windows)` 抓到）：黄金快照比对现在**先归一化换行**再比。Windows 上 git 会按 CRLF 检出签入的 LF 文件，而 `str::lines()` 会吃掉 `\r` —— "直接比字符串"会红、"只比 `lines()`"看不见，叠加出来的现象是**差异点报在文件末尾 + 期望 `<缺行>`**（本地因测试自己写过一遍文件而全绿）。已有 CRLF 变异实测（把黄金文件改成 CRLF 后测试仍绿）。
+ **本片只新增**（没有任何消费者）：谱里的 `[abi]` 节、管线、IR 都还没切换，因此**现有编译行为逐字节不变**；A2–A5 分步切换（IR `CallConvId` → 管线按 plan 发射 → 删谱里 `[abi]` 换 `[machine]`）。

### Added (2026-09-24) — ISA-DSL v19 V4d（lint 收尾：未指定位 / 可合并为 `vary` 的族）

+ **`LINT-UNASSIGNED-BITS`（opt-in `lint --bits`）**：指令字里**没有任何位域覆盖**的位段（按缺省 0 发射）。只对 `kind = "fixed"` 的 ISA 判——变长 ISA 的前缀/REX/ModRM/VEX 由编码器发射、不在位域表里建模，按表判必成误报；全字常量自动无缺口。**实测 + 人工核对**：x86 0 / riscv64 3（`NOP`/`ECALL`/`EBREAK` 的固定位型，故意只声明 `opcode`）/ arm64 67（全是保留位：`BR`/`RET`/`B.cond` 的 `[4,5)`、`[0,5)`+`[10,16)`、`LDUR` 族的 `[10,12)`+`[21,24)` 抽查都对得上参考编码）⇒ 保持评审清单，不进默认档。
+ **顺带修谱（真阳性，逐字节不变）**：riscv64 的 AMO/LR/SC 四条模板体原本把 RV64A 的 `aq`/`rl` 两位留在"未指定位"里（谱注释里提过、谱里没声明）⇒ 显式声明 `aq`/`rl` 位域并在四条模板体写 0：`--bits` 的 riscv 条目 11 → 3，`cargo test -p forge-codegen --lib` 仍 **1265 passed**，将来支持 acquire/release 只改 lowering 取值、不动编码定义。
+ **`LINT-VARY-CANDIDATE`（opt-in `lint --suggest`，只建议）**：同一 op 的多条 `[[lowering]]` 规则**发射形状相同**（逐行归一：助记符→`_`，数字/寄存器/占位符→通配）⇒ 可用一条 `vary = { attr = [...], name = [...] }` 合并。实测同形状组：x86 55 / riscv64 22 / arm64 7。**默认关**：合并与否是风格取舍（有的谱故意写开、便于各自演进），建议不进门槛。
+ 两条规则的清单都由 `crates/frontend/forge-isa-dsl/tests/lint_shipped.rs` 钉住（`unassigned_bits_inventory` / `vary_candidate_inventory`，并断言**默认档仍零结论**）；CLI 增 `--bits` / `--suggest`。

### Added (2026-09-24) — ISA-DSL v19 V3d（谱内派生枚举器：宿主不再手抄 `all_insts()`）

+ **生成物新增 `__spec_tests::all_insts()`**（`#[cfg(test)]`，宿主成品零成本）：按谱派生"代表实例"——每条指令 × 每个宽度视图一条，另有每个 imm 槽的 `lo`/`hi` 边界与每个 mem 槽的 `disp=8`/`disp=-8`/`index+scale=4` 风味。取值规则与生成期自测**同源**（新抽出的 `sample_operands`，两处不再各写一遍）。
+ **宿主侧全指令往返改由它驱动**：新增 `crates/backend/forge-codegen/src/isa_roundtrip_guard.rs`——三份发行谱的**全部**派生实例（实测 x86 **602** / riscv64 **327** / arm64 **332** 条，远多于指令总数 197/116/104）跑 `encode → decode → encode` 字节闭环 + 解码吃满整条 + `SPEC_INSTS` 覆盖清点（谱里哪条指令没进枚举器就报哪条）。
+ **x86 手抄的 650 行 `all_insts()` 删除**（连同 `decode_encode_byte_roundtrip_all`）：手抄清单在谱加指令时不会自动跟上（漏测且无人发现），这一版据实记录。三个 ISA 测试文件合计 **1225 → 557 行**（相对 V3 迁移前的 1832 行 = **−69.6%**，原目标 −≥50% 达成）；`cargo test -p forge-codegen --lib` **1264 → 1265 passed**。
+ 保留两处**显式小清单**（语义上不能派生）：各 ISA 的"规范指令 `decode(encode(x)) == x`"（别名撞车时本就不成立）与类表/ABI 类断言。

### Added (2026-09-24) — ISA-DSL v19 V4c（lint 三条新规则：位域重叠 / 能力缺口 / 未引用 ref）

+ **`LINT-BITFIELD-OVERLAP`（默认档）**：同一条指令的字段视图里两个位域抢同一批位（视图 = form 预设 ⊕ 指令覆盖后的编码键 + 指令 `fields`）。**必须按逐指令视图判**：按整张 `[conventions.bitfields]` 表判会把 riscv 的 `shamt5`/`shamt6`、`funct5`/`funct6`/`funct7`、arm64 的 `op6`+`imm26`、两边的 `word`（全字常量）这类"同一批位的多种解释"全判成错。**首次落地报出 4 条真阳性**：arm64 的 STP/LDP X/W 四条的 `idx3`（bit24 与 `op8` 常量重复写同一位，取值恰好一致所以黄金字节没暴露）⇒ 已修谱为 `idx2`（`op8` 常量给 bit24），`cargo test -p forge-codegen --lib` 仍 **1264 passed**（89 条 arm64 向量逐字节不变）。
+ **`LINT-OP-GAP`（`lint --ops <宿主 op 表>`）**：能力缺口按计划 §10.3 的**三类分开报**——终结指令（`Ret/Jmp/Br/Switch/Unreachable/Invoke`，谱里不该有 `[[lowering]]`）与宿主管线直查的 7 条都不算缺口，只有真缺口报结论，另打印一行覆盖率口径。宿主 op 表是**宿主自己的数据**（`crates/foundation/forge-ir/ops.toml`，DSL 侧不留第二份会漂移的清单）。实测与 §10.3 逐数字一致：x86 `覆盖 100 / 终结 6 / 宿主管线 7 / 真缺口 3`（`AddrSpaceCast`/`Resume`/`VaArg`）、riscv64 `61/6/7/42`、arm64 `8/6/7/95`——守卫 `tests/lint_shipped.rs::op_gap_matches_section_10_3` 把它钉住。
+ **`LINT-REF-UNUSED`（opt-in `lint --refs`）**：指令声明了 `ref` 却没有任何 lowering/pattern/emit/pseudo/spill 模板行首引用它。**默认关**：`ref` 有"给还没写的 lowering 预留多态名"的合法用法（实测三谱 28 条——arm64 27 条预留 / x86 1 条 `vmovups` 疑似残留），这是作者意图，只有作者能判；写新谱时打开它抓"名字拼错 ⇒ 多态分派永不命中"最有用。清单快照 `tests/lint_shipped.rs::unreferenced_ref_inventory`。
+ CLI：`forge-isa lint <谱>... [--json] [--ops <宿主 op 表.toml>] [--refs]`；退出码口径不变（默认档零结论仍为 0）。

### Added (2026-09-24) — ISA-DSL v19 V5（参数化变体：`[meta].variants` / `only_variants` / `--params`）

+ **一份源谱可以投影出多个变体**：`[meta].variants = { xlen = [32, 64] }` 声明"参数名 → 取值域"，指令/模板行/`[emit.prologue|epilogue]`/`[spill.*]`/`[[pseudo]]`/`[[pattern]]` 六处都能标 `only_variants = { xlen = [64] }`（"本声明只在这些取值下存在"）。**默认档逐字节不变**（不传参数 ⇒ 不过滤、不替换）。
+ **`forge-isa validate|insts --params xlen=32`**（逗号分隔多个）：`insts` 第一行打印**投影账目**（丢了哪些指令、逐节丢了什么、投影后剩多少），`--json` 里是 `projection` 字段。参数名未声明 / 取值越界一律 `DSL-META` 报错——拼错参数名不会静默"什么都没发生"。
+ **`isa_from_file!` 新增 `params = { xlen = 32 }`**：生成期投影，参数进生成物**文件名哈希**（同一份谱的两个变体落到不同 `$OUT_DIR` 文件，互不覆盖）。
+ **连带规则**：引用了被投影掉指令/`ref` 的 `[[lowering]]` 规则自动随之丢掉（可推断依赖），账目单列条数；`asm`/`[[lowering]].insts` 里的 `{参数名}` 替换成取值（宽度是数据）。**其余引用 fail-closed**：`[spill]`/`[emit]`/`[[pseudo]]`/`[[pattern]]` 引用了被投影掉的声明却没标 `only_variants` ⇒ 校验期报"未知指令引用"（结构件必须由作者显式变体化，自动猜是错的）；模板里留着参数占位符却没传值 ⇒ 报错并点名传法（否则生成的汇编会打印出字面 `{width}`）。
+ **实测账目**（`forge-isa insts --params xlen=32 isa/riscv64.toml`）：`116 → 104` 条指令（丢 `LD`/`SD` 与 10 条 W 族）、`110 → 96` 条 lowering（连带丢 14 条）、逐节丢弃 `[emit.prologue]`/`[emit.epilogue]`/`[spill.GPR]` 各 1 项。守卫 `crates/frontend/forge-isa-dsl/tests/variants.rs`（6 条：默认档零投影、未声明/越界报错、RV32 账目快照、级联不多丢、只对传了的参数生效、替换语义）。
+ **MVP 边界**：只读投影（不注册后端、不生成变体专属运行期表）。RV32 投影的帧件是空的（本谱没写 LW/SW 版本），因此它不是一份可运行的后端谱——用途是**看见变体依赖面**与让生成物正确分文件。

### Changed (2026-09-24)

+ **`report::validate_file_opts` / `validate_opts` 的第二参数由 `bool` 改为 `&RunOpts`**（V5 的一部分）：`strict_overlap` 变成 `RunOpts` 的一个字段，与新的 `params`（变体投影）放在同一处；调用点（CLI、`tests/strict_overlap.rs` 等）随之同步。**更正**：本轮一度把 `tests/strict_overlap.rs` 的编译错误记成"V6b 遗留的坏测试"，核对提交 `68e3a92` 后确认当时的签名是 `(path, strict_overlap: bool)`、该测试**可编译**——错误是本轮重构引入并当场修掉的，V6b 无此问题。

### Added (2026-09-23) — ISA-DSL v19 V6（确定性守卫 + `validate --strict-overlap`）

+ **`validate --strict-overlap`**（新诊断码 `DSL-OVERLAP`）：报"同一 op 的两条 lowering 规则取值域**相交但互不包含**"（裁决序里前者先命中）——用来抓"`when` 写窄导致大部分取值掉进兜底"或"靠 `priority` 硬分胜负"这两类可疑形状。**默认档不变**（实测三份发行谱共 61 条、抽查全是故意的"特化 + 兜底"，因此它只作为评审清单，CI 不开这个档；清单条数由 `crates/frontend/forge-isa-dsl/tests/strict_overlap.rs` 钉成快照，变了要人工复核）。
+ **生成物确定性守卫**（`crates/frontend/forge-isa-dsl/tests/determinism.rs`）：同一份谱同参数展开两次，token 文本必须**逐字节相同**、文件名恒定（`spec_tests` 变体不同名）——防止生成器内部换成 `HashMap` 迭代序/引入时间戳后"缓存失效 + 本机能编 CI 编不过"而 `cargo test` 仍全绿。

### Added (2026-09-23) — ISA-DSL v19 V4a（`forge-isa lint` 静态体检）

+ **新子命令 `forge-isa lint <谱>... [--json]`**：不执行、不编译，只报"写了却用不上"的声明——`LINT-UNUSED-SLOT`（`[[operand_slots]]` 没被任何 `ops` 引用）、`LINT-UNUSED-FORM`（`[[forms]]` 没被任何指令/模板引用）、`LINT-UNUSED-BITFIELD`（`[conventions.bitfields]` 的位域名只出现在声明处，按标识符边界计数，`imm1` 不会命中 `imm12`），结论锚到 TOML 行列；退出码与 `validate` 一致（0/1/2）。三份发行谱首次跑出 2 条真阳性（x86 的 `gpr32` 槽、arm64 的 `p6` 位域，都没人引用）→ 均已删掉（零行为变化：`cargo test -p forge-codegen --lib` 仍 1150 passed），现在三谱全部干净。守卫 `crates/frontend/forge-isa-dsl/tests/lint_shipped.rs` 把"零误报"钉成快照。

### Added (2026-09-23) — ISA-DSL v19 V3a（谱内测试向量 + `forge-isa test`）

+ **谱里可以直接写测试向量（`[[vectors]]`）**：`{asm, bytes}`（`assemble → encode` 逐字节相等 + `decode` 吃满再编码一致）、`{asm, error = "<子串>"}`（汇编/编码必须失败）、`{bytes, error = "DECODE"[, partial = N]}`（解码必须失败，`partial` 钉 `decode_partial` 的消费量）、`{bytes}`（解码正向）。生成期把它们翻成 `__spec_tests::spec_vector_<下标>` 用例，**不再需要手抄 Rust 黄金字节表**。形态在解析期校验（缺 `asm`/`bytes`、正负同给、定宽字长不符、字节越界、`partial` 用错、同一文本两种期望字节、空子串均报错）。riscv64 已迁移 67 条（原先 Rust 里的三张 oracle 表；迁移前后字节集合的规范化 sha256 相同，`cargo test -p forge-codegen --lib` 903 → 970 passed）。
+ **新 CLI 子命令 `forge-isa test <谱> [--json]`**：给任意谱现搭一个**零宿主** crate（只依赖 `forge-isa-runtime` + build-dependency `forge-isa-dsl`，`parts = ["encode","decode","asm"]`）并 `cargo test --offline`，直接跑谱里的向量与每条指令的闭环用例——不需要 forge-codegen，也不需要作者先写宿主。退出码与其它子命令一致（0/1/2）。

### Added (2026-09-23) — ISA-DSL v19 V3b（三份发行谱的黄金字节全部进谱）

+ **x86（102 条）与 arm64（78 条）的 golden 字节表也迁进 `[[vectors]]`**：x86 的 7 张 oracle 表（GPR/R 型/控制流/SSE/内存/家族/VEX，含 4 条多行写法条目）与 arm64 的 78 条 `enc("…") == word_le(0x…)` 断言。三份发行谱现在合计 **247 条向量**（riscv64 67 + x86 102 + arm64 78），全部由生成物 `__spec_tests::spec_vector_*` 执行：`cargo test -p forge-codegen --lib` **1150 passed**（= 903 基线 + 247），`forge-isa test` 逐谱 `failed:0`。迁移前后字节集合的规范化 sha256 均相同；三个测试文件合计 **1832 → 1436 行（−396）**。
+ **向量冲突判据**：同一段 `asm` 给出两种期望字节 → 编译期报错；完全相同的重复**允许**（x86 谱里有一条历史记录：`mov RAX, RBX` 两种编码合并后逐字节相同）。
+ **`[[vectors]]` 增第 5 种形态：`{asm}` 闭环向量**（v19 V3c）：只断言 `assemble → encode → decode → encode` 字节稳定与 `disassemble` 幂等，**不比黄金字节**（给"文本合法、字节由别处守"的清单用，文本歧义的指令也安全）。据此把三个 ISA 里剩余的**往返清单**也迁进谱（riscv64 67 条、x86 36 条、arm64 11 条）：三份发行谱现在共 **361 条向量**（x86 138 / riscv64 134 / arm64 89），`cargo test -p forge-codegen --lib` **1150 → 1264 passed**（+114，与迁移条数逐条对上），三个测试文件合计 **1832 → 1225 行（−607，−33.1%）**。

### Changed (2026-09-23)

+ **`parts` 不含 `tm` 时补发伪指令展开助手**：`parts` 只开 `asm`、不开 `tm` 的生成物里，`assemble` 引用的 `__pseudo_expand` 原先只在 `tm` 部件的 `gen_assembler` 里发射，导致带 `[[pseudo]]` 的谱（如 riscv 的 `li`）在 asm-only 宿主上编译不过（`E0425`）。现在 `asm && !tm` 时在模块里补发（`tm` 在时不发，避免重复定义）。由 `forge-isa test` 在 riscv64 上实测发现。

### Added (2026-09-23) — ISA-DSL v19 V2（外部宿主实证）

+ **新增示例 `examples/isa-host-demo`：证明一份 ISA 谱可以独立接入**。它只有 `[dependencies] forge-isa-runtime` 与 `[build-dependencies] forge-isa-dsl`——连 proc-macro crate 都不依赖（`build.rs` 直接调 `pregenerate`、`src/lib.rs` 只写一句 `include!`），自带玩具谱 `isa/toy16.toml` 与依赖面/生成物守卫。`cargo test -p isa-host-demo` 跑出 9 条生成期自测 + 5 条宿主用例；`cargo tree -p isa-host-demo --edges normal,build` 里没有 forge-codegen。这是 v19 目标 G1（任意普通 crate 都能承载一份谱）的硬证据，也是教程"新 ISA 从这里开始"的可抄模板。

### Changed (2026-09-23)

+ **`spec_tests` 不再要求 `tm` 部件**：生成期自测（`__spec_tests`）只需要 encode/decode/asm，此前 `parts = ["encode","decode","asm"]`（不含 `tm`）会报"必须写 `spec_tests = false`"——但自测根本不碰 TargetMachine 集成层（ABI/lowering/帧布局），这条限制挡住了"只做编解码 + 汇编"的宿主（例如 `examples/isa-host-demo`）。判据收敛为 `Parts::supports_spec_tests()`；缺 encode/decode/asm 仍在编译期明确报错。

### Changed (2026-09-23) — ISA-DSL v19 V1（破坏性）

+ **生成物运行面拆成 `forge-isa-runtime`；`isa_from_file!` 删除 `krate`**。生成物不再依赖宿主 crate 的公开面，一律写绝对路径 `forge_isa_runtime::…`（`forge_ir::…` → `forge_isa_runtime::ir::…`），因此**任意普通 crate 只要依赖 `forge-isa-runtime` + build script 预生成**就能承载一份 ISA 谱；`krate` 参数与"按宿主改写路径根"的机制一并删除。
+ **编译管线改为运行时注册**：`parts` 含 `tm` 时，宿主要用 `forge_isa_runtime::register_pipeline(ISA 名, 工厂)` 注册自己的管线（forge-codegen 已在 `pipeline_hooks::ensure_registered()`（JIT 入口自动调用）接好三个发行后端）；未注册 ⇒ 明确 `IrError::Unsupported`，不 panic。`impl_erased_target_machine!` 随之只按 ISA 名查表。
+ 新增守卫 `crates/foundation/forge-isa-runtime/tests/runtime_surface.rs`：运行面依赖面只允许 `forge-ir`/`smallvec`/`thiserror`，源码不得出现 `forge_codegen`/`crate::pipeline`。

### Fixed (2026-09-23)

+ **编辑器里 `isa_from_file!` 那几行的假阳性语法错全部消失（v18 S10d）**。rust-analyzer 一直在 `crates/backend/forge-codegen/src/arch/*.rs` 与 4 个测试夹具文件的 `isa_from_file!(…)` 行上报成对的 `expected expression` / `expected R_PAREN`（实测 riscv64 15 对 / arm64 4 对 / 夹具 9、6 对，x86 0），而 `cargo build/check/test/clippy` 一直全绿。
  根因是"整份生成物（x86 全部件 1.0 MB token）作为**宏展开结果**"撑出来的 RA 展开管线问题；修法是让生成物走**文件**这条路：落到 `$OUT_DIR/forge_gen_<模块名>_<参数哈希>.rs`，`isa_from_file!` 只展开成一句 `include!(concat!(env!("OUT_DIR"), "/…"))`。实测四类文件 **68 条 → 0**，且没有引入新的 unresolved import。
  两条踩过的坑（`docs/guides/rust-analyzer-notes.md` §1 有完整证据）：① RA 只加载"分析开始前"就存在的文件——宏在展开期写出的文件它会报 `failed to load file`，随后生成模块的项在 20+ 个文件里全变 unresolved（**比原来更糟**），所以生成物改由**宿主 build script** 预生成；② `TokenStream::to_string()` 在 proc 宏里（rustc 美化打印）与普通二进制里（紧凑打印）**文本不同**，两侧都写会互相覆盖、每轮重编，因此只允许 build script 一个写者。反面也试过：给生成物 token 整体换 `Span::mixed_site()` **完全无效**（诊断逐文件一模一样）。

### Changed (2026-09-23)

+ **宿主 crate 需要 build script 预生成 ISA 生成物（v18 S10d，破坏性 / 两处三行）**。`Cargo.toml` 加 `[build-dependencies] forge-isa-dsl = { path = … }`，`build.rs` 里 `forge_isa_dsl::pregenerate_host().expect(…)`
  ——它扫 `src/ tests/ benches/ examples/` 里每一处 `isa_from_file!`，按**同一份**参数解析生成到 `$OUT_DIR`，并登记 `cargo:rerun-if-changed`（谱的全部来源文件 + 被扫描的源文件与目录）。
  没接就**编译期**报错并点名这两步（fail-closed，而不是悄悄让生成模块变空）。`forge-codegen` 已就位；落盘件里带 `#[allow(warnings, clippy::all)]`（生成物不再享受"宏展开不 lint"的豁免，实测不加会多 223 条风格类警告）。
  `forge_isa_dsl::expand_file`（完整 token 流入口，CLI 与各守卫测试用）语义不变。

### Changed (2026-09-22)

+ **角色去宽度化：`Role` 不再把位宽编进名字（v18 S9，破坏性 / 只需改 TOML 6 行）**。原先 `roles` 里是 `fpr_mov_f32` / `fpr_mov_f64` / `wide_vec_store_32` / `_64` / `wide_vec_load_32` / `_64`——位宽被写死进角色名，别的位宽的 ISA 接不进来（等于把 x86 的 32/64 当成 ISA 通用事实）。现在角色名去掉宽度后缀（`fpr_mov` / `wide_vec_store` / `wide_vec_load`），宽度**写在角色声明里**：`roles = [{ role = "fpr_mov", bits = 32 }]`（单位是**位**，与 `opsize`/`[encoding].bits` 一致；无宽度语义的角色照旧写 `"gpr_mov"`）。
  生成器按 **(角色, 位宽)** 解析（`role_name_for`），选不到时错误消息给出**请求位宽 + 已声明位宽集合**；校验同样按 (角色, 位宽) 唯一——同角色同宽度声明两次、或同一角色一处写 `bits` 一处不写，都在**编译期**报错并点出两条指令名。`isa/x86.toml` 改了 6 行（`MOVSS`/`MOVSD`/`VMOVUPS_256_*`/`VMOVUPS_512_*`），**生成的机器码逐字节不变**（黄金字节 + 三架构 JIT 矩阵与基线逐数字相同；另有两例守卫：任意位宽 16/24 合法、同 (角色,位宽) 冲突必报）。设计动机、证据与"为什么宽度不能从槽派生"（`MOVSS`/`MOVSD` 共用 `fpr` 槽）见 `docs/archive/forge-dsl-v18-plan.md` §7.1。

### Changed (2026-09-21)

+ **`[[pattern]]` 与 `[[lowering]]` 统一裁决序 + 死模式检测（v18 S5c）**。`[[pattern]]` 新增 `priority`（与 `[[lowering]].priority` 同语义：大者先试），裁决序统一为 (`priority` 降, 匹配树 Op 节点数降, `when` 谓词叶子数降, 声明序升)；
  此前 pattern 的排序只写在 codegen 里（没有 `priority`），现在抽成 `IsaModel::pattern_order()`，**codegen 与校验器共读一份**。新增**死模式检测**：匹配树结构相同、且裁决序在前者的 `when` 完全覆盖后者 → 编译期报错并点出两个 `[[pattern]]` 下标（不同树之间不做覆盖推断——保守，宁可漏报不误报；与 lowering 的死规则检测同口径）。x86 现有两条 pattern 都没有 `priority`、新旧排序键等价 ⇒ **生成物不变**（黄金值与 JIT 矩阵照旧）。`isa-dsl.md` 的 `[[pattern]]` 节、`isa-dsl-errors.md` §3.10 同步；键速查表与 `isa-dsl.schema.json` 因新增 `priority` 键重新生成。

+ **`[[lowering]].op` 支持名单：一条规则服务多个同类 op（v18 S5）**。`op = ["Copy", "Uextend", "Freeze", "Ptrtoint", "Inttoptr"]` 等价于把这五条逐条写开，但**只维护一份发射序列**（改一处不会漏另外四处）。名单在解析期展开成逐 op 的规则，名单序 = 展开序，因此每个 op 内部的裁决序与逐条写开**完全一致**；空名/重名报错。
  随迁移落地（数字为**声明数**；`forge-isa insts` 报的是 `vary`/`op` 展开后的执行规则数）：x86 同 op 冗余 `when` 折叠 13 组 + 跨 op 归并 6 组（**220 → 197 条声明（−10.5%）**，展开后执行规则 299 → 286；TOML 3,693 → 3,518 行）；riscv 3 组（**110 → 106 条声明（−3.6%）**，展开后仍 110）；arm64 不变。
  **等价证据**：逐 op 在**同一属性空间**上比较迁移前后的判定函数（属性取值 ∪ {v±1, 0, 哨兵}，按 (`priority` 降, 叶子数降, 声明序升) 取首条命中）——x86 100 op / 2,155 格、riscv 61/352、arm64 8/61 **逐格相同**；`cargo test -p forge-codegen`（27 个测试二进制，含全部黄金字节与 417 条指令的规格自测）与三架构 JIT 矩阵（195/3/0、131/67/0、23/175/0）全绿。
  **按实测收窄**（不做，附数字）：① `lowering.emit` 结构化表（`inst`/`let`/`select`/`switch`）——族式 op 在多个属性上同时分派，表格化的规则数收益 ≤5%，而 `vary` 已覆盖其绝大多数形态；② `[[sequences]]`——跨 op 的长序列复用实测为 **0**（x86 88 条长序列、77 个不同序列，无一跨 op 重复）。两条都会引入第二个参数化机制（与 S2c「三机制合一」的结论相悖）却只值 ≤5%/0，故不做；方案里"x86 ≥15%"的目标实测不可达（真实冗余 10.5%），已按实测修正。
  文档：`docs/reference/isa-dsl.md` 的 `[[lowering]]` 节补 `op` 名单与"为什么不加 emit/sequences"（含数字）；`isa-dsl-errors.md` 补 §3.9；方案 §7 记 S5 进度与 S5c 待办。

### Fixed (2026-09-21)

+ **`#:schema` 编辑器补全：指令的 `ref` 键被 schema 写成了 `reference`（v18 S7e 修）**。模型的字段名是 `reference` + `#[serde(rename = "ref")]`，而 JSON Schema 发射器按字段名发射，于是**所有用 `ref` 的谱都被编辑器标红**（`isa/x86.toml` 35 处，Taplo + `#:schema`）。
  现在 schema 发的是 TOML 里实际写的键 `ref`；三方守卫按 `#[serde(rename = "…")]` 取键名，并新增 `schema_guard.rs::shipped_specs_only_use_schema_keys`——**直接拿 `isa/*.toml` 与全部夹具当输入**，任何"schema 与真实谱不符"都会红（自由表 `fields = {…}`/`[[templates]].body`/`when = {…}` 在 schema 里本无子约束，守卫也不下钻）。`docs/reference/isa-dsl.md` 的键速查表与签入的 `isa-dsl.schema.json` 同步重新生成。
+ 顺带修 `forge-isa insts` 表头把 `[meta].version` 标成 `schema`：它是 ISA 自己的版本串，与 DSL 语法版本无关，
  现在标 `version`（`[meta].version` 缺省时显示 `-`）。

### Added (2026-09-21)

+ **ISA-DSL 文档重写（v18 S7e）**：`docs/reference/isa-dsl.md` 去掉 "v15" 历史标题与 v15 迭代总览段（移入归档），改为「版本与现状（v18）」——一张"唯一机制 ↔ 取代了什么"对照表 + 五条承诺；
  新增 **[`docs/guides/isa-dsl-tutorial.md`](docs/guides/isa-dsl-tutorial.md)**（30 分钟接入玩具 ISA TOY16：骨架 → 操作数槽 → 指令 → `[[templates]]` → 生成期自测 → CLI 自查，示例谱本机实跑 `validate`/`insts`/`explain` 通过）；
  新增归档 **[`docs/archive/isa-dsl-v12-v17.md`](docs/archive/isa-dsl-v12-v17.md)**（v12–v17 语法史 + v18 删除/改名总表 + 方案里提过但未采纳的改名）；
  `isa-dsl-errors.md` 补「多文件组合与部件选择」错误表（缺 include / 成环 / 同名标量冲突 / `[[override]]` 目标不存在 / `parts` + `spec_tests` 冲突）与多文件诊断定位说明；
  `docs/README.md`、`CLAUDE.md`（文档地图 + 三版本号辨析 + 三方守卫三条要点）同步；`bench_baseline.md` 的 `ir_dsl` 段补 v18 落地后的规格规模与**生成代码采集口径**提醒（S8 对照必须固定同一命令）。

+ **ISA-DSL 多文件组合 `include` / `[[override]]` + 部件选择 `parts` + CLI `fmt`（v18 S7d）**。新增 `forge-isa-dsl::loader`（递归 `include`，深度上限 8、重复/成环报错、**按块合并**：数组节按 include 序追加、重复 `[表头]` 视为同节续写、同名标量冲突报错并提示改用 `[[override]]`），`[[override]] key = "点分路径" value = …` 显式覆盖被包含文件里的键（目标不存在即报错，拼错不静默）；诊断带**来源文件映射**——`路径:行:列` 指向真正写那一行的那份文件；生成物对**每个来源文件**都登记 `include_bytes!`（改任一片段都触发重编译）。
  `isa_from_file!` 参数扩到四个：`krate` / `spec_tests` / **`name = "…"`**（模块名覆盖，同一份谱展开多次必需）/ **`parts = ["encode", "decode", "asm", "tm"]`**（部件选择：`Inst`/`Reg` 枚举与寄存器表是任何部件的公共前提，恒定生成；受限时必须 `spec_tests = false`，否则编译期明确报错）。CLI 新增 `fmt [--out <file>]`：把多文件谱折叠成一份单文件 TOML（可继续编辑、可单文件分发、可独立校验、幂等）。
  `include`/`[[override]]` 同时进 `IsaModel` + JSON Schema + 文档键表（三方针守卫覆盖）；绕过加载器把**裸文本**交给解析器时，带这两个组合键会明确报错而不是静默忽略。修掉 `report::validate_file` 把加载器消息改写成「读不到文件：<根路径>」的问题（缺 include / 覆盖键不存在时现在是点名文件与键的可诊断错误）。
  文档：`docs/reference/isa-dsl.md` 新增「多文件组合（`include` / `[[override]]`，v18 S7d）」节 + `isa_from_file!` 四参数表 + `fmt` 命令；方案 §5.8 改写为落地形态；夹具 README 增列 `include_root` / `include_base`。
  用例：`forge-isa-dsl/tests/parts_selection.rs`（7）、`forge-codegen/tests/include_v12_tests.rs`（7，含多文件黄金字节 + 只开 `encode` 的模块真编译）、`forge-isa/tests/cli_tests.rs` 增至 16（多文件 validate/insts、`fmt` 折叠与幂等、诊断指向片段文件、缺 include / 覆盖键不存在必须点名）。

### Changed (2026-09-21)

+ **生成 `Inst` 变体的字段名 = `ops` 里声明的操作数名（v18 S7d 修正）**。此前字段名另取一套：定宽 ISA 取位域名（`[forms].operand_fields` 的 `rd`/`rs1`），变长 ISA 取语义角色名（`dest`/`cond`/`mem`/`imm`/`target`）——`ops` 里作者写的名字**只用于 asm 模板**，于是 `ops = ["dst:r:out", "src:r"]` 生成出 `Inst::Iadd { rd, rs1 }`：作者声明的名字在用户面没有任何意义，位域名还泄漏成了 API。
  现在**字段名就是声明名**（`Inst::Iadd { dst, src }`、x86 `MovRmR { src, dst }`、条件码 `JccRel32 { cc, target }`），位域名/语义角色名退回纯内部**编码键**（只用于查 `[conventions.bitfields]`、modrm 角色与立即数编码表）。名字不能直接作标识符时按最小规则归一：Rust 关键字 → 原始标识符（`type` → `r#type`）、数字开头 → 前缀 `_`（`8bit` → `_8bit`），**不做语义改名**。
  **编码行为零变化**：`demo_inst12` 生成物 token 级对照（v18 S7a 基线 vs 现在）只有 `rd→dst`(32)/`rs1→src`(32)/`rs2→src2`(16) 共 80 处标识符改名，未触碰任何编码 token（证据 `target/s7d_field_rename_evidence.txt`）；三个 ISA 的黄金字节测试、417 条指令的生成期自测、三条 JIT 矩阵全绿。
  守卫与迁移：新增 `forge-isa-dsl/tests/field_names.rs`（定宽用声明名而非位域名、关键字原始化、数字开头加前缀、变长 ISA 按声明名）；`OperandUse` 的死字段 `field` 删除，改为携带**声明名** `name`；`crates/backend/forge-codegen/tests/*` 的引用同步改名（`dest`→`dst`、`rd`/`rs1`/`rs2`→`dst`/`src`/`src2`、`imm*`→`imm`、label 域→`target`、`cond`→`cc`）。文档：`docs/reference/isa-dsl.md` 的「命名操作数」「`[[forms]]`」「代码生成输出」三节改写为"声明名 = 字段名，位域名 = 编码位置"。

### Added (2026-09-20)

+ **ISA-DSL 的 JSON Schema + `#:schema` 编辑器补全（v18 S7c），三方一致由守卫钉住**。`forge-isa-dsl::schema`（手写发射器，不引 `schemars`）把谱的 TOML 结构发射成 JSON Schema（draft 2020-12，32 个节定义 + 18 个根键，含必填/可选/编码键与节级说明）；`forge-isa schema [--out <file>]` 打印或写出，仓库根的 `isa-dsl.schema.json` 由它生成并签入；3 个发行 ISA + 6 个夹具的 TOML 顶部加 `#:schema <相对路径>` 注释，Taplo 等语言服务据此补全。
  **三方守卫**（`crates/frontend/forge-isa-dsl/tests/schema_guard.rs`，5 条）：① schema 每节的键集与 `dsl/model.rs` 对应结构体的 `pub` 字段**逐键相等**（`#[serde(skip)]` 内部字段须在 `INTERNAL_FIELDS` 登记；`#[serde(flatten)]` 字段须在 `FLATTEN_FIELDS` 登记）；② 内部字段表不得过时；③ `docs/reference/isa-dsl.md` 新增的「键总览（速查表）」区段与 `schema::markdown_table()` **逐字相同**——文档里的键表不再手抄；④ 签入的 `isa-dsl.schema.json` 与发射器逐字相同；⑤ `--nocapture` 打印可粘贴的表格。守卫在落地时就抓到三处漂移（`enc` 被误当键、`Pattern` 的 `r#match`/TOML `match` 漏写、`[[pattern]].when` 未登记）。
  文档：`docs/reference/isa-dsl.md` 新增「键总览（速查表，v18 S7c）」节（机器校验的键表）+ 工具链节的 `schema` 与 `#:schema` 说明 + TOC；方案 §7「S7 进度」补 S7c。

### Added (2026-09-20)

+ **`forge-isa` CLI（v18 S7b）：写 TOML 时不必接后端就能校验、看展开结果、做规格 diff**。新增 `crates/tools/forge-isa`（bin，唯一依赖 `forge-isa-dsl`；手写参数解析与 JSON 发射器，不引 `clap`/`serde_json`）：

  | 命令 | 作用 |
  | --- | --- |
  | `validate <谱.toml>…` | 解析 + 校验，打印**全部**诊断（`路径:行:列: 码: 消息` + 附注），有错退出 1 |
  | `insts <谱.toml>` | 展开后的指令与生效规格：字长（位/字节）、form、opcode、ops、编码键、`ref`、`reloc`、来源模板与行号、asm |
  | `explain <谱.toml> <指令名>` | 单条指令的完整来源：哪个模板的哪一行 + 该行与 `body` 的键 + 生效规格逐字段 |
  | `diff <a> <b>` | 两份谱的**规格 diff**（增/删/改字段，`字段: A → B`）——迁移前后"展开后有效规格"对照 |

  `--json` 给机读输出；退出码 `0` 成功 / `1` 诊断或失败 / `2` 用法错误。示例：`cargo run -p forge-isa -- explain isa/arm64.toml ADDREGW` → `来源：[[templates.ADDREG]] 第 2 行` + 生效编码键。
  实现：新增 `forge-isa-dsl::report` 投影层（`IsaSummary`/`InstRow`/`Explain`/`SpecDiff`），**复用**编译器的 `collect_inst_infos`（form 预设 ⊕ 指令级覆盖的同一份判定），编码键清单由 `EncKeys` 的 serde 折出——不维护第二份键名表，新增编码键自动出现在三个子命令里。
  证据：`forge-isa` 10 条集成测试（跑真实二进制）+ `report` 6 条单测；`forge-isa-dsl` 182 单测；三份发行 ISA `validate` 全 OK；`insts isa/riscv64.toml` = `116 条指令 / 19 条模板 / 110 条 lowering`；混合字长夹具 JSON 给出 `CADD16/CMOV16 = 16 位（2 字节）`、`LNOP32/LADD32 = 32 位（4 字节）`。
  文档：`docs/reference/isa-dsl.md` 新增「工具链：`forge-isa` CLI」节 + TOC；方案 §7「S7 进度」补 S7b 已落地。

### Changed (2026-09-20)

+ **ISA-DSL 拆成两个 crate（v18 S7a）：`forge-isa-dsl`（编译器本体）+ `forge-dsl`（薄 proc-macro）**。原 `forge-dsl`（proc-macro）里的模型/解析/校验/诊断/代码生成整体搬进新的普通 lib **`forge-isa-dsl`**（`crates/frontend/forge-isa-dsl`，含 `v12/` 与 `assembler/`，以及两个反回潮守卫测试）；`forge-dsl` 只剩 `isa_from_file!` 的参数解析并调 `forge_isa_dsl::expand_file`。宏的使用方式与**生成代码逐字节不变**。
  动机：proc-macro crate 不能导出非宏项，而 ISA-DSL 的校验/`explain`/JSON Schema/`insts`/`diff`/CLI 都要在宏之外可用（S7b/S7c 的前提）。
  新公开面（`forge-isa-dsl`）：`expand_file(path, &ExpandOptions)`、`expand_str(source, mod_name, path)`、`validate_source`/`validate_file`（返回渲染好的诊断行）、`read_isa_file`、`dump_generated`、`ExpandOptions { krate, spec_tests }`。
  验证：临时 worktree 检出 S6 末状态（`113209c`）与本片各 dump 一次生成代码，路径前缀归一化后 x86/riscv64/arm64 + 6 个夹具共 9 个模块 **token 序列完全相同**（原始 dump 的行折叠差异只来自 rustc token 打印器对路径长度的换行启发式）；`forge-isa-dsl` 176 单测 + 2 `generality_guard` + 1 `no_hardcoded_widths` 全绿，`forge-dsl` 1 条参数解析单测；`forge-codegen` 全部 target、workspace 99 个 target、三架构 JIT 矩阵 195/3/0、131/67/0、23/175/0 不变；clippy（拆分后的三种组合）、release check（`--exclude forge-rustc`）、rustdoc、markdownlint 全干净。

### Added (2026-09-20)

+ **ISA-DSL 生成期自测 `__spec_tests`（v18 S6）：每条指令自动进回归网**。生成器在 ISA 模块里再吐一个 `#[cfg(test)] mod __spec_tests`——写 TOML 的人不必再手抄"这条指令编出来是不是这几个字节"：

  | 断言 | 抓什么 |
  | --- | --- |
  | `encode` 成功 ∧ 长度 = 该指令字长（`prefix_scan` 除外） | 字长声明与编码不一致 |
  | `decode(bytes)` 成功 ∧ 消费 `bytes.len()` | 解码少读/多读 |
  | `encode∘decode` 与 `decode∘encode` 字节稳定 | 编码/解码不对称 |
  | 解码字段**值**原样（立即数按位域语义、条件码、寄存器索引、内存 base/disp） | 对称的位序/槽位错位（reg/rm 互换、扩展位丢失） |
  | `disassemble → assemble` 成功 ∧ 文本幂等 ∧ 再编码稳定；文本唯一时还要求字节相等 | 汇编/反汇编不对称、操作数序错、内存模板不闭合 |
  | 立即数 `min`/`max` 原样、`min−1`/`max+1` 在 `encode` 处**报错** | 静默截断/掩码（判据与编码器共享 `imm_encode_checked`） |

  覆盖维度 = **宽度视图**（多类槽逐宽度：x86 `gprx` 的 16/32/64 位分别走 66 前缀 / 无 REX.W / REX.W）× **高编号寄存器视图**（每组最高几个索引：REX.R/B/X、8 位寄存器的 REX 强制、EVEX 的 ZMM16-31）。断言的是**闭环不变式**，"字节对不对"仍由各 ISA 的编码参考文档与既有黄金值测试守着。
  生成模块导出 `SPEC_TOTAL`/`SPEC_COVERED`/`SPEC_CASES`/`SPEC_SKIPPED`（名字+原因，S6 判据：空）/`SPEC_TEXT_AMBIGUOUS`（同名同形、编码不同 ⇒ 文本分不清）；外部守卫 `src/spec_coverage_guard.rs` 钉死指令总数（x86 197 / riscv64 116 / arm64 104）、零跳过与歧义名单（31/4/0）。`isa_from_file!` 新增第三参数 `spec_tests = <bool>`（缺省 true）；夹具谱（`tests/common/mod.rs`，同一份谱被多个测试二进制包含）显式关掉，由 `tests/spec_tests.rs` 打开三个极端形状夹具（1 字节寄存器 / 12 位字 / 混合字长）。
  覆盖结果：**417 条指令全部覆盖、零跳过**（用例数 x86 427 / riscv64 179 / arm64 258）；`cargo test -p forge-codegen` 26 个 target、workspace 97 个 target、三架构 JIT 矩阵 195/3/0、131/67/0、23/175/0 全部不变。

+ **S6 当场抓到的两处真缺陷（已修）**：① riscv `SLLW/SRLW/SRAW` 的移位量共用了 6 位 `shamt` 槽而字段只有 5 位 ⇒ `sllw rd, rs1, 32..63` 被**静默编成 `n−32`**（不报错）；修法是 ISA 数据里加 `shamt_w`（5 位）槽并让三条 W 指令用它，越界现在在 `encode` 处报错。② x86 EVEX 寄存器直寻址的 `ModRM.rm` 是 `EVEX.X':B':rm[2:0]`（内存形式下 X' 才是 SIB index bit3），编码/解码两侧都只用了 4 位 ⇒ **ZMM16-31 当 rm 时静默编成 ZMM0-15**（`vaddps zmm16, zmm17, zmm18` 旧输出 `62 E1 74 40 58 C2` 实际是 `rm=zmm2`）；修法是非内存形态用 rm bit4 生成/还原 X'，黄金值更正为 `62 A1 74 40 58 C2`，并新增独立公开参照例 `vaddps zmm15, zmm24, zmm3` → `62 71 3C 40 58 FB`。
  文档：`docs/reference/isa-dsl.md` 新增「生成期自测（`__spec_tests`）」节（断言表 + 覆盖维度 + 常量 + `spec_tests` 参数）、方案 §5.9 与 §7「S6 进度」、arm64 指令数由 89 更正为 104（S3c 的 `b.cond` 16 行）。

### Added (2026-09-20)

+ **ISA-DSL 指令宽度三态 `[encoding]`（v18 S4）：拆掉"一个 ISA 一个字长"的假设，打开 RVC/Thumb 类混合字长 ISA**。`[meta]` 的四个宽度散键（`default_inst_width` / `variable_length` / `max_inst_len` / `default_opsize`）删掉，收敛成独立的一段，**逐指令** `width` 由指令（或模板行）自己写：

  ```toml
  [encoding]
  kind = "fixed"          # fixed | mixed | prefix_scan
  bits = 32               # fixed：字长（位，任意 ≥ 1）

  # kind = "mixed"（16 位短编码 + 32 位长编码共存）
  # bits = 16             # 可选：逐指令 width 的缺省
  # widths = [16, 32]     # 必填：允许的字长集（解码按升序分组尝试）

  # kind = "prefix_scan"（x86：长度由前缀链决定）
  # max_len = 15          # 可选：最长指令字节数（缺省 15）

  default_opsize = 32     # 可选（原 [meta].default_opsize 归位到本段）
  ```

  + `fixed`：全 ISA 一个字长（= 旧 `default_inst_width` 语义），编码/解码路径与生成物**逐字节不变**；三发行 ISA 与 5 个夹具全部迁移到本段。
  + `mixed`（**新能力**）：编码按该指令字长发字节；解码**按 `widths` 升序分组**、每组一棵位级 trie、"首个完整匹配即停"（短编码优先，RVC/Thumb 同构）。新夹具 `crates/backend/forge-codegen/tests/isa/demo_mixed16_32.toml`（低 2 位判别短/长编码：`sel = 0` vs `3`）+ `tests/demo_mixed16_32_tests.rs` 5 条用例（黄金字节、按宽度分组解码与消费字节数、编解码往返、`decode_partial` 截断阈值 = 最短字长、能力集）。
  + `prefix_scan`：x86 走原有 `vlen.rs` 变长路径，只把"最长长度"换成 `max_len`；分派从"变长/定宽二分"改为按 `is_prefix_scan()` 三态分派（`fixed`/`mixed` 共用定宽位域编解码）。
  + **校验期结构性互斥**（错误码 `DSL-ENCODING`）：`fixed` 写 `widths`/`max_len`、`prefix_scan` 写 `bits`、`mixed` 写 `max_len`、逐指令 `width` 不在 `widths` 里或与 `bits` 不一致 → 编译期报错；**省略整个 `[encoding]`** 仍是合法骨架文档（= `fixed` 且无 `bits`），生成期 `inst_bytes()` 报"bits 缺失"——省略整段不会被静默当成定宽 32，逐指令 `width` 也不能替代 `bits`。
  + 能力集（`IsaCapabilities`）由三态派生：`fixed` → `fixed_inst_size` = `min` = `max` = `bits/8`（定宽 ISA 以前报 `0` = 未知，现在是真实字长）；`mixed` → `fixed_inst_size = 0`、min/max = 最窄/最宽、`variable_length = true`；`prefix_scan` → min = 1、max = `max_len`。
  + 未做（如实记录）：草案里的 `[encoding].prefixes` 前缀效果表——x86 前缀语义已在 `[conventions.prefix_scan]` + `vlen.rs`，再造一张表是同一事实两处声明。`mixed` 的短/长判别位由 ISA 自己保证互斥（校验器无法通用地证明，不做假保证），文档与夹具注释写明。
  + 证据：`cargo test -p forge-dsl` 175 + 2 + 1 全绿；workspace 测试 96 个 target 全绿；三架构 JIT 矩阵 **x86 195/3/0、riscv64 131/67/0、arm64 23/175/0**（与 S3 完全相同）；生成代码对拍（8 个模块）差异仅"文档字符串 `[meta].default_inst_width` → `[encoding].bits`"与上述能力集字长；clippy 两道、release check（`--exclude forge-rustc`，见下）、rustdoc、markdownlint 全干净。
  + 顺带发现（未修，超出本片范围）：`cargo check --workspace --release --all-targets` 在 `forge-rustc` 上失败——`types.rs::assert_assignable` 是 `#[cfg(debug_assertions)]` 而 `lower/place.rs` 无条件调用（两文件最后改动 2026-09-04，与本片无关；CI 只跑 debug 的 `cargo check -p forge-rustc`）。

### Added (2026-09-19)

+ **ISA-DSL 汇编器伪指令 `[[pseudo]]`（v18 S3e）：汇编期的文本级多指令展开**。`.equ`/`.macro` 的结构化兄弟——不碰编码器、不碰 lowering：`parse_insts` 遇到以伪指令名开头的行，就按 `params` 位置切分实参、逐行把 `{参数}` 换成实参文本，再让**同一套汇编器**装配展开出的每一行。

  ```toml
  [[pseudo]]
  name = "li"                     # 汇编可见的助记符（不得与指令助记符重名）
  params = ["rd", "imm"]          # 位置实参名（emit 里用 `{名字}` 引用）
  emit = [
    "lui {rd}, ({imm} + 0x800)",
    "addi {rd}, {rd}, ((({imm} + 0x800) & 0xfff) - 0x800)",
  ]
  ```

  实参按**顶层逗号**切分（`()`/`[]` 内的逗号不算，故 `li x1, (a + b)` 与内存操作数 `[x2, #4]` 都能写）；emit 行里的算术由**既有表达式求值器**求值（`+ - * / % << >> & | ^ ~`、括号、`.equ` 符号）——**不引入第二套表达式语言**；emit 行可以是**别的伪指令**（递归展开，深度上限 16 防自引用成环）；单条 `assemble()` API 只接受展开成 1 条的伪指令，多条要用 `TargetAssembler::parse_insts`（错误消息指引）。
  riscv 的 `li` 是实际用例：`li x10, 0x1234` → `lui x10, 1`（0x00001537）+ `addi x10, x10, 0x234`（0x23450513），与 lowering 里 `Iconst` 用的 `{iconst_hi20}`/`{iconst_lo12}` 完全同一套算术（逐值对照 0/-1/0x800/表达式实参/与普通指令混排）。
  新增校验 `validate_pseudos`（错误码 `DSL-PSEUDO`）：名字非空/唯一/**不得与指令助记符重名**（重名会让汇编器永远匹配不到它）；`params` 非空唯一；`emit` 非空、行首词必须是指令助记符或别的伪指令名；`{…}` 必须是声明的参数且**每个参数都得用到**。**没有 `[[derive]]`/`[[pseudo]]` 的 ISA 不生成任何展开器代码**（x86/arm64 与 5 个夹具的生成物只差 `assemble()` 那 2 行新增文档注释）。
  未实现（如实记录，不做半成品）：按谓词分派同名多条（汇编期没有 IR 属性可判）、`pseudo_fold`（反汇编折叠回伪指令——那是指令级模式识别，与 `[[pattern]]` 同类问题）。
  证据：forge-dsl 175 + 2 + 1（新增 6 条声明侧用例）、`forge-codegen` 全部套件（`asm_enhance_tests` 15 条，新增 2 条 riscv 行为用例）、三架构 JIT 矩阵（195/3/0、131/67/0、23/175/0）、clippy、release、rustdoc、markdownlint 全干净。

+ **ISA-DSL 派生谓词属性 `[[derive]]`（v18 S3f）：给重复出现的 `when` 条件起名字**。同一条件在多条 lowering 规则里重复时，一次声明、多处引用：

  ```toml
  [[derive]]
  name = "is_64"
  expr = { eq = ["rs1_width", 64] }

  [[lowering]]
  op = "Iadd"
  when = { eq = ["is_64", 1] }
  insts = ["ADD64 {out}, {0}, {1}"]
  ```

  `expr` 复用结构化谓词（同一份解析与校验），派生属性取值 = 1（真）/0（假），用 `eq`/`ne`/`in` 引用；展开在**解析期**——生成期的 `__attr` 只多一个派生臂（判定走核心属性表 `__attr_core`），谓词判定逻辑一行未改。名字不得与核心属性重名（否则静默遮蔽）、不得引用另一个派生（属性名出现在值位置的代换语义不唯一，如实拒绝并提示"派生不能引用派生"）；派生名可出现在 `vary` 里，与核心属性同待遇（自动追加 `eq = [名字, 行值]`）。错误码 `DSL-DERIVE`。
  **零成本**：没有 `[[derive]]` 时生成的属性表与引入前**逐字相同**（8 个模块 dump 逐个 `identical=True`）——三发行 ISA 与夹具都没用它，所以三架构 JIT 矩阵与全部黄金值不变。
  证据：新增 6 条用例（`when` 引用派生 + 生成的派生臂、无派生时不新增一层、`vary` 用派生名、核心属性重名、重名、`expr` 非法/类型错、派生引用派生）；文档 `docs/reference/isa-dsl.md` 结构化谓词节补 `[[derive]]` 小节与 `iconst` 行、`isa-dsl-errors.md` 增 `DSL-DERIVE`、方案 §5.6 改记为已落地并说明"数值派生（`sub` 等算术节点）未实现"的理由。

+ **ISA-DSL 重定位数据化（v18 S3d）：`[[reloc]]` 取代 `GlobalReloc` 枚举**。指令侧只写引用名（`reloc = "abs64"`），语义与绑定槽在表里：

  ```toml
  [[reloc]]
  name = "abs64"            # 指令引用：reloc = "abs64"
  semantics = "absolute"    # 宿主语义（有限、ISA 无关）：absolute / pc_relative
  slot = "imm64"            # 绑定到哪个操作数槽（须是 imm 槽）
  addend = 0                # 可选：重定位值的链接期加减
  ```

  两种语义的 fixup 落点由**数据**决定：`absolute` → 指令末尾该槽的字节区间（补丁宽度 = `ceil(槽宽/8)`，x86 `MOVABS_GLOBAL` 的 imm64 → `Absolute(8)`）；`pc_relative` → 指令起始（补丁宽度 = 指令字长，riscv `AUIPC_GLOBAL`/`ADDI_GLOBAL` → `Relative(4, 0)`）。**宿主语义集合就是 `RelocKind` 的两态**——计划里列的 `hi20`/`lo12`/`got`/`tls_*` 属于"新增语义才需要宿主代码"那一侧，等真有 ISA 用到再加；ISA 特有的**位段写入**（arm64 写 imm26/imm19、riscv 按 opcode 0x17/0x13 分写 hi20/lo12）仍留在各 ISA 的 reloc patcher，这是数据之外唯一的宿主代码。
  删除 `GlobalReloc{Abs8,PcrelHi,PcrelLo}` 与 `Instruction.global_reloc`（无兼容层）；新增 `validate_relocs`（名字非空唯一、`slot` 已声明且是 imm 槽、指令引用的名字必须在表里、该指令确实有那个槽的操作数）与错误码 `DSL-RELOC`。
  证据：生成代码只在重定位臂上从 `ABS8`（定义即 `Absolute(8)`）变成 `Absolute(8)`、addend 字面量 `0` → `0i64` —— **语义等价**（x86 6 行、riscv 4 行差异，其余模块逐字节相同）；三架构 JIT 矩阵不变（x86 195/3/0、riscv 131/67/0、arm64 23/175/0；x86/riscv 矩阵含 `GlobalAddr` 用例，端到端跑过重定位）；新增 7 条 reloc 用例（解析 + 生成 `Relative(4,0)`/`Absolute(ceil(槽宽/8))`、未声明名、坏槽、重名、槽不在操作数里、语义名非法）。
  文档：`docs/reference/isa-dsl.md` 新增 `[[reloc]]` 节（含两语义的 fixup 表）并更新 `reloc` 字段、`isa-dsl-errors.md` 增 `DSL-RELOC`、方案 §5.4/§7。

+ **ISA-DSL arm64 条件码符号化 + `b.cond` 全条件（v18 S3c）**：`isa/arm64.toml` 新增 `[conventions.cond]`（A64 的 16 个条件名 `eq/ne/cs/cc/mi/pl/vs/vc/hi/ls/ge/lt/gt/le/al/nv` + `hs`/`lo` 同码别名；**不写 `ir`**——arm64 的 lowering 目前不用 `{cc}`，将来加 Icmp lowering 时 validate 会强制补全 10 个），CSEL 族的 `cond4` 槽从 `kind = "imm"`（写成 `#0`）改为 `kind = "cond"`：汇编/反汇编现在用**符号名**（`csel x0, x1, x2, eq`，反汇编渲染同码首选名 `hs`→`cs`）。
  新增 `B.cond`：一条 `[[templates]]` 16 行——`asm = "b.{cname} {target}"`（助记符用行键插值，这是"条件在助记符里"的通用写法）+ 条件码做成固定位域 `bcond = [3:0]`（opcode `0x54` 进 `[31:24]`、imm19 在 `[23:5]`、bit4 恒 0）。**14 个 A64 合法条件**（`[3:0]=111x` 保留）逐个对照 `docs/reference/aarch64-encoding-ref.md` §4 的条件码表验证字节：`b.eq 0`=0x54000000、`b.ne 0`=0x54000001、`b.hs 0`=0x54000002、`b.gt 0`=0x5400000C、`b.le 0`=0x5400000D、`b.eq 2`=0x54000040（偏移进 imm19），外加汇编↔反汇编往返与 `b.hs`≡`b.cs`、`b.lo`≡`b.cc` 同码断言。
  顺带**删除**原先那条错的 `BCOND` 存根（`form = "CBZF"` + `fields = { cond = 0 }`：目标被放进 `rt=[4:0]`、条件恒 0 且落在 `[15:12]`、imm19 恒 0——从来不是合法 B.cond，也无人使用；`b.eq` 之前被它抢先匹配）。
  生成器侧补一个缺口：**定宽解码器**原先对非 Reg 槽一律产出 `i64`，cond 槽在 `Inst` 里是 `u8` ⇒ 补上 `OperandKind::Cond => raw as u8`（arm64 是定宽 ISA 的第一个 cond 槽用例）。
  文档：`docs/reference/isa-dsl.md` 的条件码节补"条件当操作数 / 条件在助记符里"两种写法、`aarch64-encoding-ref.md` 记 S3c 进展并更新"待做"、方案 §7 记 S3c。

+ **ISA-DSL 条件码数据化（v18 S3b）：`[conventions.cond]` 一张表服务三处，删掉 x86 硬编码表与缺省；通用性守卫白名单清空**。表从 `名 = 编码` 变成 `名 = { code, ir? }`（也接受整数简写 `eq = 4`，此时 `ir` 取键名）：键 = 本 ISA 汇编/反汇编可见的条件名，`code` ∈ 0..=15，`ir` = 它实现哪个 IR 整数条件（`eq/ne/slt/sle/sgt/sge/ult/ule/ugt/uge`）。三处用途：**汇编**按名解析 `cond` 槽、**反汇编**按码取同码里字母序最小的名（渲染与 S3b 前逐字节一致）、**lowering 的 `{cc}`** 按 `ir` 字段查本 ISA 编码。
  删掉两处 x86 硬编码：`lowering.rs` 的 `IntCC → setcc 编码` match（生成代码里不再出现 `IntCC`）与 `asm.rs` 的 `cond_default()`（未声明表时的 x86 16 项缺省）——现在"没声明就没有条件码能力，用到即报错"。宿主新增 `forge_ir::intcc_name(码) → 条件名`，把"IR 条件"这一侧的键空间收敛到一处（`INTCC_NAMES`/`IntCC::mnemonic`）。
  新增校验（`validate_cond`）：表非空、`code ≤ 15`（4 位条件字段）、`ir` 必须是 10 个规范名之一、**一个 IR 条件只能被映射一次**、`cond` 槽需要表、**用了 `{cc}` 就必须把 10 个条件映射全**（否则运行期静默退化成 0 = 溢出条件，是最难查的一类错）。
  证据：x86 生成代码的 `{cc}` 映射与新表逐条相同（`eq→4 / ne→5 / slt→12 / sle→14 / sgt→15 / sge→13 / ult→2 / ule→6 / ugt→7 / uge→3`），x86/riscv/arm64 其余生成物只在"未使用的 `__cc` 绑定"上变小；x86 黄金/汇编/解码 78 例 + 三架构 JIT 矩阵（195/3/0、131/67/0）不变；`tests/generality_guard.rs` 的 `ALLOWED` **清空**（生成期代码里已无任何 ISA 常量：指令名、寄存器名、条件码全来自 `isa/*.toml`）。文档：`docs/reference/isa-dsl.md` 新增"条件码表（一张表，三处用）"、`isa-dsl-errors.md` 增条件码错误表、方案 §5.3/§7。

+ **ISA-DSL 三处"别家常量兜底"改 fail-closed（v18 S3a）：缺失的 ABI 声明不再回退到某个 ISA 的寄存器名/指令名**。①`frame.rs` 的栈参数收参 scratch 寄存器原先在 `[abi].scratch` 缺失时回退 x86 的 `R10`（生成的代码引用 `Reg::R10`——非 x86 ISA 直接编译不过）；②spilled 寄存器参数收参需要 MOV 时原先用 `inst_exists(infos, "MOV_RM8_R64")` **按 x86 指令名**探测，而同一函数里 `move_inst` 早已由 `roles = ["gpr_mov"]` 派生；③`lowering.rs` 的 Call/CallIndirect 两处在 `[abi].call_ret_reg` 缺失时回退 riscv 的 `X1`。现在三处都**生成期明确报错**，且**只在真的需要时才要求**（无栈参数的 ISA 不需要 scratch；call 指令没有 Out/InOut Reg 槽的 x86 不需要 `call_ret_reg`）。
  证据：三发行 ISA + 5 个夹具的 `FGE_DEBUG_GEN` dump 与改前**逐字节相同**（对现网谱是纯收紧），新增两条负向用例——真实 x86 谱删掉 `scratch` → 报错点名 `[abi].scratch`；真实 riscv 谱删掉 `call_ret_reg` → 报错点名 `call_ret_reg`。

+ **ISA-DSL 定义指令的机制统一为**一个**（v18 S2c）：`[[templates]]` + `rows` + 指令属性 `ref`，`[[families]]`/`[[aliases]]` 全删**。起因是评审意见——同一件事有 `[[templates]]`/`[[families]]`/`[[aliases]]` 三个模块、三套校验、三种诊断前缀，是使用负担（"只能保留一个"）。
  合并规则只有两条：**多条指令共用一份声明** → 模板的 `body`（共享字段，可省略）+ `rows`（每行一条指令：`inst` 必填，其余键是取值/字段）；**一个引用名指向多条指令** → 指令上的 `ref`（多条共用同一 `ref` = 多态分派，取代 `[[aliases]]`）。
  行键分三类且**只由 `body` 决定**：body 里有同名键 ⇒ 覆盖（表递归合并）；body 没有但被 `{键}` 引用 ⇒ 纯参数（只插值）；其余 ⇒ 指令字段（拼错由 `Instruction` 反序列化点名拒绝）。`{inst}`/`{键.lower}` 派生实例名与助记符，整串恰为占位符时保留类型。`rows` 是 TOML 数组，`rows = [ { … }, … ]`（一行一条）与 `[[templates.rows]]` 是同一结构的两种写法。
  三 ISA 与全部夹具迁移：arm64 `871 → 891 行（+2.3%）`、riscv `1,899 → 1,802（−5.1%）`、x86 `3,887 → 3,678（−5.4%）`，`[[families]]` 14 条（100 变体）与 `[[aliases]]` 23 条全部消失，指令总数不变（89/116/197）。arm64 略增是换来的：原先 2 行指令用平行数组按列 zip 写得极紧凑，但要逐行对齐、加一条变体改 3 个数组。
  等价证据分三层：①**模型级**（迁移脚本 `target/unify.py` 自带自检）旧展开 vs 新展开逐指令比较 form/opcode/fields/ops/asm/roles/when/编码键 + **引用名 → 成员有序列表**，三 ISA 全等；②**生成代码级** arm64 与 5 个夹具**逐字节相同**，riscv/x86 每个函数的 token 多重集完全相同（差异只是 `Inst` 枚举/编码臂/解码 trie 的顺序——旧顺序是"手写指令 → 模板 → 族"，新顺序是"手写指令 → 按文件序的模板"，见方案 §12.8）；③**行为级** 黄金测试（arm64 12+6、riscv 12+4、x86 61 例）与三架构 JIT 矩阵（23/175/0、131/67/0、195/3/0）全不变、语料三支通过。顺带补回一条被族校验覆盖过的通用校验：**指令必须有编码来源**（`opcode`/`fields`/`opcode_reg`/`modrm`/`vex`/`evex`/`imm` 至少一个），现在对所有指令生效。
  证据与文档：`src/v12/model.rs`（`Template{name, body, rows}` + `TemplateRow{inst, fields}` + `deep_merge`/`subst`/`referenced_keys`）、`src/v12/template_tests.rs`（24 例）、`src/v12/diag_matrix_tests.rs`（30 例矩阵改成模板/`ref` 用例）、`docs/reference/isa-dsl.md` 的 `[[templates]]` 节重写为**唯一机制**（含旧→新对照表与实测收益）、`docs/reference/isa-dsl-errors.md`（`DSL-FAMILY`/`DSL-ALIAS` 删除，新增 `DSL-TEMPLATE` 与模板错误表）、方案 §5.2/§6/§7/§12.7/§12.8。

+ **ISA-DSL 参数化模板 `[[templates]]`（v18 S2a，**首版机制形态**：`params` 平行数组**，已被同日 S2c 改为 `body` + `rows`；本条目保留首版迁移的实测数字）**：一处声明 → N 条指令，自动派生别名。动机是实测出来的重复：`isa/arm64.toml` 有 **38 对**指令只差 sf 位/寄存器槽/opcode（76/89 = 85%）、`isa/riscv64.toml` 有 **16 对** S/D（41%），而旧 `[[families]]` 的变体**无法覆盖 `ops`/`form`/enc 键**，只能整条复制；与之配套的 `[[aliases]]` 还是手写清单（x86 25 条 / arm64 29 条，且已见漂移：`vaddps` 漏了 `VADDPS_ZMM_MASKZ`）。
  新机制：`params` 等长列表按下标 zip（与 `[[lowering]].vary` 同语义）；字符串字段里 `{参数}` 做文本替换、**整串就是 `{参数}`** 时保留参数类型（`opcode = "{opcode}"` 仍是整数）；`name`/`names` 给实例名；`ref` 自动派生别名（单值 = 多态共用，含 `{参数}` = 逐实例）；`[[templates.overrides]]` 逐行打补丁（表递归合并）。展开在**解析期**完成 ⇒ 下游（校验/代码生成）只看到普通指令与别名，`codegen` 一行未改；展开出的实例若非法，诊断前缀改回 `[[templates.X]]`，位置落在模板声明行。
  **arm64 首例迁移**：`isa/arm64.toml` **1,125 → 731 行（−35%）**，32 条模板取代 64 个手写指令块、**29 条手写 `[[aliases]]` 全删**；黄金测试（`arm64_tests` 12 例 + `arm64_tm_tests` 6 例）通过、arm64 JIT 矩阵 **23 passed / 175 skipped / 0 failed** 不变、生成代码里的 `Inst::` 名字集合 **90 个前后 diff 为空**、arm64 生成代码 644,018 → 636,986 B。
  证据与文档：`src/v12/template_tests.rs`（10 例：展开/类型保留/逐实例 ref/补丁合并/五类失败路径/lowering 引用实例名）、`v12/model.rs::Template` 文档、`docs/reference/isa-dsl.md` 新增 `[[templates]]` 节、`docs/reference/isa-dsl-errors.md` 增模板错误表、方案 §7/§12.7 记录实测、通用性守卫扩展为也识别模板实例名（arm64 迁移后 `ADDIMMX` 不再是 `[[instructions]]` 字面量）。

+ **ISA-DSL S2 续：riscv 与 x86 定向迁移（三 ISA 全覆盖；**数字为 S2a 形态**）**。riscv `1,754 → 1,606 行（−8.4%）`：15 条模板取代 30 个 `_S`/`_W` ↔ `_D` 指令块（`funct7`/`funct3` 逐行给、助记符后缀 `.{pl}` 插值）——顺带证明 **`[[families]]` 表达不了这类对**（族模板 `{name}` 只能派生"变体名小写"，得不到 `fadd.s` 这种带点助记符，这正是它此前只能手写两份的原因）。x86 `3,356 → 3,324 行（−1%）`：`movzx`/`movsx` 的 R8/R16 四条合成 1 条模板（`ref = "{mn}"` 顺带取代 2 条手写 `[[aliases]]`），四条指令的**生成编码臂文本 SHA-256 逐字节相同**。
  **当轮的设计结论已被 S2c 推翻（保留作过程记录）**：S2a 曾得出"`[[templates]]`/`[[families]]`/`[[aliases]]` 三者**互补而非取代**"，于是选择保留三者分工；S2c 按评审意见把它们合并为 `[[templates]]` 一个机制（见本节第一条）——"互补"成立的是**表达力**判断，不成立的是**该不该并存**：`rows` 的键空间就是指令字段空间，族变体只能覆盖 `fields`/`opcode`/`roles` 的限制消失，别名折成 `ref` 后引用表逐项相同。x86 剩余的同 asm/同 ops 组仍差在**编码键**（`form` 预设有无、`opsize`、内联 vs preset）——那是不同编码而非可参数化取值，统一它们属于语义重构，**不做**（此结论 S2c 沿用）。
  验证：三 ISA 的 `Inst::` 名字集合前后 diff 为空（90 / 117 / 198）、黄金测试（arm64 12+6、riscv 12+4、x86 61 例）与三架构 JIT 矩阵（195/3/0、131/67/0、23/175/0）全不变。

### Changed (2026-09-19)

+ **ISA-DSL 诊断升级（v18 S1）：一次列全 + 精确到键行 + 错误码；`[emit]`/`[spill]` 从"完全不校验"变为编译期校验**。执行方案 `docs/archive/forge-dsl-v18-plan.md`（用户口径：允许破坏性更新、无需兼容旧版本、兼顾体验、足够通用），本条目是其中的 S1 切片。
  诊断侧：`validate` 从"14 个校验器 fail-fast、一次只报第一条"改为**收集式**（节级 + 逐条声明，最多 32 条，超出追加"另有 N 条"）；定位从"`source.find("name = …")` 全局启发式（同名会指错、抽不出就退化 `1:1`）"改为**声明索引**（一次预扫建"节 + 名字 → 块行范围"，块内再用消息里引号点名的值精确定位到**出错的键行**）；重复声明指向**后出现**的那一处并附注另一处位置；错误码 `DSL-<节>` 稳定可过滤，目录见 `docs/reference/isa-dsl-errors.md`。
  校验侧补上两个"未校验的名字引用面"：`[emit.prologue/epilogue].insts` 与 `[spill.*].load/store` 现在校验指令引用名、`@` 伪指令（`@push_callee`/`@pop_callee`/`@frame_alloc`/`@frame_free`/`@move_args`）、占位符（emit：`{frame_size}`/`{frame_size_neg}`/`{frame_size_mN}`/`{callee_saved_bytes}`；spill：编号 `{N}`）与 `base` 寄存器名。**这会拒绝此前静默通过的 spec**（S0 基线实测：`[emit]` 里写 `NO_SUCH_INST`、`@nope`、`{bogus}` 全部 `<ok>`），三份发行谱已实测通过新校验。
  证据：`src/v12/diag_matrix_tests.rs`（30 例错误码/位置矩阵 + 多错并列 + 附注 + 上限 + 头条精确位置）、`v12/diag.rs` 单测（索引/精准定位/渲染/上限）、S0→S1 对照表在方案 §12.4/§12.6；`cargo clean -p forge-codegen` 后 `cargo check -p forge-codegen` 通过（34.7 s）。
  同批修掉两处 doc/code 漂移：`docs/README.md` 里 `reference/isa-dsl.md` 标注"文档 v15 / 代码 v16–v17"与 `reference/binary-format.md` 更新为 v2。

### Fixed (2026-09-19)

+ **文本打印不再丢"多别名 metadata"的名字**（二进制语料往返测试顺带暴露的老问题）：一个 metadata 节点被多个名字指向时（`!foo` 与 `!\23pragma` 内容相同 ⇒ `intern`/`insert_at` 去重成同一节点），display 原先每节点只打印**一个**名字（且 `MetadataStore::name_of` 用 `HashMap::iter().find(...)`，打哪个随实例随机种子变化）⇒ **丢名字 + 输出不确定**。现在 display 对每个别名各打印一行（名字按字节序 ⇒ 同输入同输出），新增回归用例 `display_llvm.rs::named_metadata_prints_every_alias`（两行都在 + 再解析再打印逐字节相同；改前必失败）；参考文档"已知限制"一节相应移除该项（现 §12）。

+ **补上二进制解码的 metadata 嵌套深度上限（执行方案 §2.6 承诺项）**：`MetadataValue::Field` 是递归结构，解码器原先会一直递归到输入末尾——一条 `Field(k, Field(k, Field(k, …)))` 就能把**解码器的栈打爆**（进程 abort，而不是可捕获的 `Err`）。现在 `MAX_METADATA_DEPTH = 64`，超过即 `IrError::BinaryDecode`（带偏移）；负向用例含 **5000 层**嵌套（必须在到达上限时返回，而不是递归到底），另有 8 层正常解码的正向对照。同步把"编码侧假定自洽 IR、不自洽时**带原因 panic**；解码侧任何输入都不 panic"写进 `Module::{to_binary_into, from_binary}` 的 API 文档，参考文档 §5 的原语表补上该上限。
  实测（本机 2026-09-19）：workspace **1690 passed / 0 failed / 19 ignored**；语料 198 正向 / 254 正确拒绝 / 0 误收（198 例二进制往返仍全部文本一致 + 字节幂等）；fmt/clippy 两道门/release/doc/markdownlint 全干净。

### Added (2026-09-19)

+ **文件级 IR 缓存 `IrCache`（消费方收益②）+ 它的盈亏实测**：新增 `binary/cache.rs`，公开面 `forge_ir::{IrCache, CacheKey}`——按**内容键**（源码字节 + 格式版本 + producer 的 128 位指纹）把"源码文本 → `Module`"这一步落盘，`load`/`store`/`get_or_insert_with`/`entry_path`/`entry_count`；**自愈**（读失败/损坏/版本不符一律当 miss，miss 时重算并覆盖）、**原子落盘**（临时文件 + `rename`，不读到半截条目）、**失败不缓存**（`build` 返回 `None` 不落盘）。定位是"可随时删除的加速层，不是事实源"（128 位指纹非密码学哈希，已在模块文档交代）。
  接线与实测（本机 2026-09-19，`cargo bench -p forge-ir --bench ir_binary -- ir_binary_cache`）：基准新增 `ir_binary_cache`（884 B 文本）与 `ir_binary_cache_large`（43 KB 文本）两组，各自拆出 `parse_text` / `cache_hit` / `read_entry` / `decode_entry`。
  **结论按尺度分**（每项三次运行）：884 B 的中等模块 `cache_hit` 114–195 µs vs `parse_text` 77–94 µs（**亏 1.2–2.3×**——命中要付文件读取的固定成本 75–204 µs）；43 KB 的大模块 `cache_hit` 723–1146 µs vs `parse_text` 1835–2320 µs（**赚 2.0–2.5×**，每个模块省 ~0.9–1.4 ms）。
  示例新增 `--cache <dir>`（打印 HIT/MISS）；在 452 个 `.ll` 的夹具目录上端到端对照：不缓存 272/155 ms、缓存 179/194 ms ⇒ **无可测收益**（332 个文件本就解析失败，可解析的 119 个都太小）。**用法约定**：机制保留，但**不进正确性测试**（缓存会短路"文本 → 模块"，让测试少测解析器一层）——完整表在 `docs/performance/bench_baseline.md`，规范与约定在 `docs/reference/binary-format.md` §11。
  **顺带给出"收益③（rustc e2e 的 IR 级缓存）"的评估结论：不做**——`crates/tools/forge-rustc/` 里 `parse_module`/`from_binary`/`to_binary` 零命中（它是 rustc 的 codegen 后端，IR 直接从 MIR 在进程内降低，从不读文本 IR），且整支 198 文件语料的解析合计 ~0.9 s 而单个 e2e 用例是秒级工作；理由与触发条件记在 `docs/plans/forge-ir-binary-serialization-plan.md` §10.2。

+ **二进制格式 v2：段体压缩（体积减半，零依赖、确定性）**。`IR_FORMAT_VERSION` 1 → **2**，段表条目加第 4 字段 `raw_len`（`0` = 段体原样存放、`> 0` = 段体是压缩体、值 = 解压后长度）；新增 `binary/pack.rs`（自研 LZ77 变体：token = 偶数字面量段 / 奇数回引段 + 距离 varint，窗口 64 KiB、最小匹配 4、单 token ≤ 64 KiB、贪心最长匹配、候选链长上限 64、哈希表长随输入自适应且**跨段复用**的 `Packer`）。写侧**只在压缩体严格更小时**才压（无旋钮、无开关 ⇒ 同输入同输出；由 `packed_entries_are_always_strictly_smaller` 钉住"永不越压越大"）；读侧解压后的段体与 v1 逐字节相同，段解码路径**一字未改**。
  实测体积（198 个 LLVM `test/Assembler` 正向语料）：字节流 241,440 → **121,257 B（×0.50）**，对源码文本 0.59× → **0.30×**，最大单例 31,038 → **8,482 B**，平均 1,219 → **612 B**。实测吞吐（`--bench ir_binary`，中等模块）：encode 21.9 → **22.5 µs**（三次运行 22.1/30.5/22.5 的中位数 ⇒ 压缩的成本落在计时噪声内；中途"每段新建 256 KiB 哈希表"版本实测 184.1 µs，改 scratch 复用 + 表长自适应后回到基线量级）、decode 77.2 → **42.9 µs**（三次运行 43.5/42.9/32.0，字节少 28% ⇒ 读侧游标/切片/校验都少）。**本机同点波动可达 ~1.7×**（同一版代码曾测得 encode 45.2 µs 与 26.0 µs），故文档一律按"方向 + 多次运行"记录、不当单点回归判据。
  解压安全（全部在**进入解压循环之前**或恰在当时 fail-closed）：单段 256 MiB 上限、压缩比 65536×+4096 上限、距离必须落在已产出字节内、token 长度不得超过"还差多少到 `raw_len`"、结束时必须恰好产出 `raw_len` 且输入恰好耗尽；负向用例覆盖坏段体首字节（空字面量 token）、`raw_len` 翻转（产出与声明不符）、压缩体截断、两路解压炸弹。格式规范见 `docs/reference/binary-format.md` §8（含"为什么自研"与"将来按段换成 zstd 不需要改格式"），基线对照表见 `docs/performance/bench_baseline.md`。
  门禁（提交 ecd2528）：fmt/clippy 两道门/`check --release --all-targets`/`doc -D warnings`/`bench --no-run`/语料三支/三架构 JIT 矩阵/markdownlint 全干净；workspace 测试 `-j 1 --test-threads=1` **1604 passed / 0 failed / 19 ignored**（92 个测试二进制）；**GitHub Actions CI run #131 全绿**（Linux/Windows/macOS 测试、Benchmarks check、Clippy、Format、Docs、Coverage、forge-rustc check + e2e、forge-tests nightly 共 11 个作业）。

+ **二进制格式有了可运行的消费者 + 吞吐/尺寸基线**：新增示例 `crates/foundation/forge-ir/examples/ir_binary.rs`（只用公开 API —— 示例是独立 crate，因此同时是"公开面够不够用"的编译期检验）：默认模式做 parse → `to_binary` → `from_binary` → 逐函数 `Verifier` → 文本一致 → 字节幂等，并打印每文件的源码/字节流尺寸与 parse/encode/decode 耗时；`--check` 只读头部报 `check_binary_compat`（版本/producer/段表）；`--write <dir>` 落盘 `.fir`。新增 `benches/ir_binary.rs`（`cargo bench -p forge-ir --bench ir_binary`）量编解码吞吐。
  首次采集（本机 2026-09-19）：中等模块（884 B 文本 → 1,335 B 字节流）encode **21.9 µs / ~58 MiB·s⁻¹**、decode **77.2 µs / ~16.5 MiB·s⁻¹**（解码慢 ~3.5×：每次读都查剩余长度、重建三个 arena、回查 value kind 并重建 use-lists）；198 个正向语料的**源码文本 408,810 B → 字节流 241,440 B（0.59×，未压缩）**；v2 压缩后的对照数字见上一条。数字记入 `docs/performance/bench_baseline.md`，命令与口径见 `docs/reference/binary-format.md` §10/§11。

+ **二进制往返的 fuzz 扩面：同一批随机模块走二进制（执行方案 §2.5 验收项）**：`tests/roundtrip_fuzz.rs` 的生成器与 `fuzz_roundtrip` 扩展为"文本往返 + 二进制往返"同批断言——默认 **10_000** 个随机模块（含第二固定种子 500 个）每个都额外做 `to_binary` → `from_binary`：①结构等价（模块/块/指令/终结符/常量/全局初始化字节，复用既有 `assert_modules_eq`/`assert_globals_eq`）；②解码后**文本打印与首次一致**；③`decode → encode` 与首次编码**逐字节相同**。本机实测 2 passed / 0 failed，整支 ~29 s（可 `FORGE_FUZZ_ITERS`/`FORGE_FUZZ_SEED` 覆盖复现）。

+ **forge-ir 二进制序列化 B5（METADATA/GLOBALS/MODULE 段 + 语料端到端 + 格式规范文档）**：`binary/meta.rs`（metadata 节点表按 id 顺序 + 命名表按名排序）、`binary/globals.rs`（全局/别名/comdat + 模块三元组·源文件·模块 asm）。回放纪律：metadata 节点**按原样落位**（不用 `intern`——显式 `!N` 经 `insert_at` 预分配槽位、arena 允许同内容两条，`intern` 会折叠导致 id 整体错位；节点内 `Node(id)` 只校验 `< 节点总数`，**前向引用合法**）；全局/别名/comdat 一律经 `Module::{add_global, add_global_alias, add_comdat}` 回放，名字索引表随之重建、重名 fail-closed；附件 `MetadataId` 在解码末尾做悬空引用校验。
  **语料端到端**（`tests/binary_module.rs`）：LLVM `test/Assembler` 全部正向用例 parse → `to_binary` → `from_binary`，断言文本打印**逐字符一致** + `decode → encode` 逐字节幂等 —— **198/198 通过**；尺寸基线：合计 **241,440 B**、最大 **31,038 B**（`auto_upgrade_nvvm_intrinsics.ll`）、平均 **1,219 B**（作为后续压缩/演进的对照）。
  **顺带修掉一处既有不确定性**（语料往返测试抓到）：`MetadataStore::name_of` 原用 `HashMap::iter().find(...)` 反查，一个 id 被多个名字指向时（`!foo` 与 `!\23pragma` 内容相同 ⇒ 去重成一个节点）返回值随实例随机种子变化，display 输出随之不确定 → 改为取**字节序最小**者，并新增 `names_of` 返回全部别名；打印器"每节点只打一个名字"（多别名时丢名字）是**既有限制**，已记录在参考文档（当时的 §10，现 §12），当轮不动。
  规范落地：新增 `docs/reference/binary-format.md`（容器/段/原语/各段细节/确定性/版本/验证基线/已知限制），`docs/README.md` 索引；负向对照补：未知 metadata 节点/值 tag、越界节点引用、附件悬空、未知 comdat kind。
  实测（本机 2026-09-19）：workspace **1689 passed / 0 failed / 19 ignored**（B5 新增 10 例）；语料 198 正向 / 254 正确拒绝 / 0 误收；矩阵 x86 195/3/0、riscv64 131/67/0、arm64 23/175/0；fmt/clippy 两道门/release/doc/markdownlint 全干净。
+ **forge-ir 二进制序列化 B4（FUNCS 段：函数 + `dfg` + layout）**：`binary/funcs.rs` 每个函数写名字/签名/调用约定/属性位/开放属性/符号（linkage·visibility·dll·section·comdat·TLS 模型·三个布尔）/`is_const`/personality/参数与返回属性/函数级 metadata/`debug_info`（`locations` + 函数名）/值名与块名/**每函数常量池**（与模块 `CONSTS` 共用同一份五通道编码器）/`dfg`/`layout`/入口块。
  `dfg` 侧：每个值写**显式 `ValueDef`**（`Inst(i,k)`/`Param(b,k)`/`AggConst`/`UndefNamed`）+ 类型；指令写 opcode **名字**（`ops.toml` 是单一事实源，读侧 `from_name` 解析，未知名即错——不受枚举声明顺序影响）、块、结果、操作数、11 种 immediates、`InstFlags`/`MemFlags`、调用点属性、metadata、`loc`、`isel_strategy`、墓碑位；块写参数类型/参数值/块内顺序/终结符。**use-lists 不落盘**，解码后按指令操作数重建（终结符指令在 `insts` 里，同样登记）。
  **解码后结构校验 `validate_dfg`**（"顺序即索引"不靠隐式假设）：值类型在界内、每条 `ValueDef` 回指一致（指令第 k 个结果 / 块第 k 个参数）、指令结果反向回指、操作数/结果/块参数类型/块顺序表/终结符/布局/入口块全部在界内——任一处不一致即 `Err`（带偏移）。解码写 arena 走新增的 `DataFlowGraph::{push_value_verbatim,push_inst_verbatim,push_block_verbatim}`（保 dense 索引、不触发 `make_*` 的附带登记），metadata 走既有唯一写入口 `attach_metadata`——两处都满足既有守卫（`dfg_privatization.rs` 的"arena 只能经受限 API 访问"、`metadata_single_write.rs` 的"附件只能经唯一写入口"），**未给守卫加白名单**。
  负向对照（全部实测为 `Err`）：未知 opcode 名、未知调用约定、value kind 与密集索引不一致、悬空操作数、未知 immediate tag、越界值类型、FUNCS 段体任意截断前缀（逐字节扫描不 panic）。
  实测（本机 2026-09-19）：workspace **1676 passed / 0 failed / 19 ignored**（B4 新增 11 例：往返/幂等/校验器/6 项负向/截断扫描/空模块段存在性）；往返用例同时断言**解码后函数过 `Verifier`**；语料 198 正向 / 254 正确拒绝 / 0 误收；矩阵 x86 195/3/0、riscv64 131/67/0、arm64 23/175/0（架构无关，未受影响）；fmt/clippy 两道门/release/doc 全干净。
+ **forge-ir 二进制序列化 B3（CONSTS 段：五通道常量）**：`binary/consts.rs` 逐条按池内索引写 int/float/big/vector/aggregate 五通道，解码按**同序** `insert_*` 重建 ⇒ `ConstId`/`AggId` 的密集索引逐位不变（`bool_const` 的固定槽位也因此原样保留）。`Big` 三变体全覆盖：`Signed`/`Unsigned` 用 dashu 的小端补码字节，`Float` 写 `repr()` 的归一化有效数 + 指数 + **`Context::precision`**。
  **实现坑（负向用例抓出并修掉）**：`Big::Float` 只写 `(significand, exponent)` 会**丢精度上下文**——`Repr::into_parts()` 会把有效数归一化，`from_parts` 又把精度重置为"有效数位数"，于是 1.5 的 `prec: 53` 变成 `prec: 2`（值相同、精度不同，Debug 逐字符对比当场暴露）；改为写 `precision()` 并用 `Repr::new` + `Context::new` + `Real::from_repr` 重建后逐字符一致。
  fail-closed 负向对照（全部实测为 `Err`）：重复常量（`insert_*` 返回索引与位置不符）、未知 `Big` 变体 tag、未知向量端序 tag、聚合标量子越界、聚合**前向/自引用**（防环）、`i128`/`u128` varint 溢出（第 19 字节越界位、20 字节续位）。
  实测（本机 2026-09-19）：workspace **1668 passed / 0 failed / 19 ignored**；语料 198 正向 / 254 正确拒绝 / 0 误收；fmt `--check`、clippy 两道门、`cargo check --release --all-targets`、`cargo doc -D warnings` 全干净。

+ **forge-ir 二进制序列化 B1+B2（IR bitcode v1：容器骨架 + 字符串表 + 类型段）**：新模块 `crates/foundation/forge-ir/src/binary/{mod,format,writer,reader,types}.rs`；公开 API `Module::{to_binary, to_binary_into, from_binary}` 与 `forge_ir::{IR_FORMAT_VERSION, SectionId, BinaryCompat, check_binary_compat}`，新错误变体 `IrError::BinaryDecode { offset, msg }`（`Display` 带偏移）。**不加 feature 门控**（零依赖、不依赖文本层 —— 对 `docs/plans/forge-ir-s8-design.md` §2.4 的修订）。
  格式 v1：`magic "FORGEIR\0"`(8B) + varint 版本 + producer（varint 长度 + UTF-8，**头部自包含**）+ varint 段数 + 段表 `[u8 id | varint 绝对偏移 | varint 长度]` + 段体（按 id 升序，无对齐无填充）。已落段：`0x00 COMPAT`（flags=0，未知位置位即错）、`0x01 STRINGS`（`num` + 长度表 + 字节拼接，**逐条往返、不预留空串槽**）、`0x02 TYPES`（12 种类型 tag 全覆盖 + 命名类型表（按名排序）+ 签名表 + `DataLayout`（三张对齐表与指针表按 key 排序））。
  fail-closed 纪律：`Cursor` 每次读先查剩余长度；长度字段先与剩余字节比对再分配（拒绝"声明 100 万段 / 4G 长度"）；varint 截断与溢出、未知段 id / 类型 tag / 调用约定 / mangling、段越界与重叠、重复段、重复字符串、非法 UTF-8、悬空 `TypeId`、预填充固定索引错位一律 `Err`（带偏移），**绝不 panic、绝不静默跳过**；`num = 0` 空池合法。
  **负向对照（真实发生并修掉，两条都记在提交信息里）**：① `BinaryCompat.producer` 原写 `String` ⇒ `open_set_boundary.rs::no_plain_string_fields_on_public_ir_surface` FAILED 并点名该行（v3 S5 的"公开面不得有裸 `String`"守卫当场生效）→ 改 `ImmStr`；② `to_binary_into` 追加语义下 `finish()` 的头部长度断言按绝对长度比较会误报 → 改成按增量比较。
  实测（本机 2026-09-19）：workspace **1658 passed / 0 failed / 19 ignored**（92 个测试目标；本片新增 42 例：`binary` 单测 16、类型库解码不变量单测 5、`tests/binary_format.rs` 21）；LLVM 语料 **198 正向 / 254 正确拒绝 / 0 误收**、语料往返幂等 189/189；JIT 矩阵 x86 **195/3/0**、riscv64 **131/67/0**、arm64 **23/175/0**；fmt `--check`、`clippy --workspace --exclude forge-rustc --all-targets --all-features -D warnings`、`clippy -p forge-ir --no-default-features --lib -D warnings`、`cargo check --release --all-targets`、`cargo doc -D warnings` 全干净。切片计划与逐片证据见
  `docs/plans/forge-ir-binary-serialization-plan.md`。

### Changed (2026-09-19)

+ **forge-ir `src/` 目录归类（A0：A0a 文本层 + A0b 其余；可维护性重构）**：`src/` 顶层原来平铺 30 个 `.rs`（最大单文件 `text/parser/semantics.rs` 4,510 行），现按职能分组，顶层只剩 `lib.rs`/`error.rs`/`verify.rs`：`entity/{mod.rs←entity.rs, map.rs←entity_map.rs}`、`ir/{types,dfg,function,opcode,constant,immediate,terminator,builder,metadata,symbol,data_layout,type_rules,inst_flags,mem_flags,isel_strategy}.rs`、`analysis/{mod.rs←analysis.rs, alias, loop_info, use_list, debug_info}.rs`、`util/{big, imm_str, string_pool}.rs`、`text/{display.rs, parser/*}`。
  **公开面不变**：类型仍全部从 crate 根扁平导出（`forge_ir::Function`/`TypeId`/…），并**新增** `forge_ir::{EntityRef, EntitySet, PackedOption, PrimaryMap, SecondaryMap}`（此前只有 `forge_ir::entity_map::SecondaryMap`）；文本层规范路径 `forge_ir::text::{parse_module, parse_function, function_to_string}`。
  **无兼容层**（按"无需兼容旧版本结构"）：旧模块路径直接删除，不留别名/双入口；全仓 71 个 `.rs` + `grammar.lalrpop` 同提交改写（`crate::types::`→`crate::ir::types::` 等），`lalrpop_mod!` 生成路径改 `/text/parser/grammar.rs`，语法文件里 135 处 `crate::ir_parser::` 与 2 处 `crate::big::` 一并改（否则下次重建回退）。
  两处真实坑（详见 `docs/archive/forge-ir-v3-plan.md` §6 的 A0 条目）：① `lalrpop_mod!` 的 include 串与 `grammar.lalrpop` 的类型路径是两处独立事实；② 群组移动后文件内 `super::` 语义变化，按"模块→全路径"统一改 `crate::…`。组名取 `util` 而非 `support`（`forge_opt::support` 已占用，两者经 `code_forge::prelude` 的 glob 重导出会 `ambiguous glob re-exports`，实测改名后清零）。
  守卫同步（都是硬编码路径的静态守卫）：`read_path_budget.rs`（`src/ir/types.rs`）、`metadata_single_write.rs` 白名单（`ir/dfg.rs`/`ir/function.rs`）、`tombstone_semantics.rs`（`ir/dfg.rs`）、`forge-opt/tests/entity_tables.rs`（`src/ir/{function,dfg}.rs`）、`entity_privatization.rs`（`src/entity/mod.rs`）、`text_feature_gate.rs`（核心集合 = `src/**` 去掉 `src/text/**`，并新增"`src/display.rs`/`src/ir_parser.rs` 不得复活"断言）。**负向对照**：不改 `metadata_single_write.rs` 白名单 ⇒ 该用例 FAILED 并点名 `ir/dfg.rs` 三行；不改 `tombstone_semantics.rs` ⇒ FAILED 点名
  `ir/dfg.rs`；恢复后全绿（两次失败证明这些守卫真的在扫路径，而不是"没扫到所以绿"）。
  实测：workspace **1515 passed / 0 failed / 19 ignored**（与归类前同数）；LLVM 语料 **198 正向 / 254 正确拒绝 / 0 误收**、语料往返幂等 **189/189**（用例内 `assert_eq!` 精确断言）；三套 JIT 矩阵 x86 **195/3/0**、riscv64 **131/67/0**、arm64 **23/175/0**（均与归类前一致）；fmt `--check`、`clippy --workspace --exclude forge-rustc --all-targets --all-features -D warnings`、`clippy -p forge-ir --no-default-features --lib -D warnings`、`cargo check --release --all-targets`、`cargo doc -D warnings` 全干净。

+ **S8 拍板：只做二进制序列化；新增执行方案文档**：`docs/plans/forge-ir-s8-design.md` 记录拍板结果（A 二进制**做**、B MemorySSA-lite 与 C crate 拆分**不做**，触发条件保留），并修订 §2.4 的 `binary` feature 决策（**不加 feature 门控**：零依赖零耦合，门控只制造"只有开 feature 才编到"的验证盲区）。新增 `docs/plans/forge-ir-binary-serialization-plan.md`：格式 v1 的**字节级规范**（magic + varint 版本 + producer + 段表，`0x00 COMPAT`…`0x07 MODULE` 八段；LEB128/zigzag、字符串表、句柄 dense index、枚举显式判别值 + 穷举 match、`Big` 规范形式、函数段 `value_kinds` 两遍解码、`Cursor` fail-closed 与嵌套深度上限、`check_binary_compat` 版本策略、确定性约束），
  B1–B5 切片表（范围/产出/负向对照/估行），每片门禁命令与基线数字，风险对策表，外部参考（MLIR bytecode / LLVM BitCode / wasmtime 序列化），以及否决 `postcard`/`bincode`/`rkyv`/`serde` 的理由（零新依赖、偏移级 fail-closed、禁止 `HashMap` 遍历的确定性都是验收项）。

### Changed (2026-09-16)

+ **读写交错函数的"先写后读"：读数段合并（forge-ir v3 S3 余项②）**：`operand_to_value` 原来每个 arm 各取一次类型锁（metadata 判定、位模式判定、浮点位宽、向量元素类型、zeroinit 尺寸、const-expr 类型/尺寸，共 9 处）；现在在 `to_type`（**写**）之后取**一次**快照，把各 arm 需要的类型事实读进 `#[derive(Clone, Copy)] OperandTyFacts`，快照**不跨越**会 `strings.intern` 的 arm（跨越会触发整表 COW 克隆）。顺带合并 `build_inst` 的 `extractvalue` 索引链、`vconst` 的 `element_type`+`size_bytes`、`agg_const_from_operands` 的重复取锁。
  确定性计数实测（新守卫 `type_store_read_path.rs::operand_reads_share_one_snapshot`）：32 条指令的浮点位模式 161 → **129**、向量字面量 224 → **192**（各 −1 次/指令），公共路径 129 不变（无回归）；静态预算 `ir_parser/semantics.rs` 24 → **15**；`write_path_never_clones_in_real_workload` 仍 0 次整表克隆。负向验证：`UInt` arm 改回逐操作数取锁 ⇒ 用例 FAILED（161 vs 129）。
  实测：workspace 1515 passed / 0 failed / 19 ignored；x86 195/3/0、riscv64 131/67/0、arm64 23/175/0。

+ **新增 S8 可选项设计方案（待拍板）**：`docs/plans/forge-ir-s8-design.md` 逐个给出二进制序列化 / MemorySSA-lite / crate 边界拆分的现状基线（forge-ir 生产代码 25,245 行、文本层占 9,995 行≈40%、10 类实体句柄、6 类常量通道…）、数据模型与格式/表示设计、分期与验证方案、成本与触发条件；建议 A（二进制）可做、B/C 不做，并列出需要拍板的四个问题。`docs/README.md` 索引与 v3 计划 S8 行已指向该文。

+ **密集句柄表收口：全仓零 `HashMap<Value|Block|Inst>`（forge-ir v3 S2 余项④ 收官）**：把上批登记的 32 处余量全部迁完（`forge-opt` 的 `const_fold`/`sccp`/`gvn`/`gvn_pre`/`loop_unroll`/`licm`/`algebraic`/`lto`/`func_specialize`/`inline`），并清掉 `forge-ir`（`analysis` 的 `postorder_rank`/`preds_map`、`loop_info`、`ir_parser/semantics::per_pred`）与 `forge-codegen`（`agg_expand::AggSlots`、`compiler::rewrite`、`liverange`、`lowering::roots`）的同类表。
  为此给 `SecondaryMap` 补 `FromIterator<(K, V)>`（与 `HashMap::collect()` 同形），`Function::apply_replacements` 与 `DataFlowGraph::clone_inst` 的 `value_remap` 改收 `&mut SecondaryMap`。
  守卫由"预算表"升级为**零容忍扫描**：`forge-opt/tests/entity_tables.rs::no_dense_handle_hashmaps_in_forge_opt` + `forge-codegen/tests/entity_tables.rs::no_dense_handle_hashmaps_repo_wide`（扫 `crates/**` 与根 `src/`，跳过注释行、精确匹配键名以免误伤 forge-hir 的 `HashMap<BlockId, _>`）。负向验证：在 `loops/licm.rs` 插一处 `HashMap<Value, u64>` ⇒ 两个用例各 FAILED 并点名 `licm.rs:247: [Value]`。
  **唯一豁免**：`XReg`（`Eq`/`Hash` 含 class，按 index 密化会把同一 index 的不同类合并——语义变化），理由由 `xreg_is_index_plus_class_so_not_dense` 钉住。本切片未做 opt 侧 A/B（不宣称提速）。
  实测：workspace 1514 passed / 0 failed / 19 ignored；x86 195/3/0、riscv64 131/67/0、arm64 23/175/0。

+ **优化 pass 侧密集句柄表 → `SecondaryMap`（forge-ir v3 S2 余项④ 第二批）**：`Value`/`Inst`/`Block` 都是 forge-ir 的密集句柄，以它们为键的表不该走 `HashMap`。本批迁移 `scalar/dead_code.rs::build_use_counts`、`scalar/copy_prop.rs::build_copy_map`、`scalar/cse.rs`/`scalar/gvn.rs` 的 `replacements`（含 `gvn_dfs` 传参）、`ipa/inline.rs::repl`，并把**核心 API** `Function::apply_replacements` 的参数由 `&HashMap<Value, Value>` 改为 `&SecondaryMap<Value, Value>`（4 处调用点同步）。
  剩余 32 处逐文件登记进新增的预算表 `crates/middle/forge-opt/tests/entity_tables.rs`（精确相等 + 每条写原因；迁移一批下调一批，新增一处即红），另有 `apply_replacements_takes_a_dense_map` 守卫核心 API 不回头。负向验证：在 `loops/licm.rs` 插一处 `HashMap<Value, u64>` ⇒ 预算用例 FAILED（实测 2 vs 预算 1）。
  本切片**未做** opt 侧 A/B 计时（不宣称 pass 提速），宣称的是结构一致性与"剩余量可数"。实测：workspace 1512 passed / 0 failed / 19 ignored；x86 195/3/0、riscv64 131/67/0、arm64 23/175/0。

+ **代码生成侧密集句柄表 → `SecondaryMap`（forge-ir v3 S2 余项④ 第一批）**：`LowerCtx::{vreg_classes,vreg_types,vreg_widths}`（VReg 键）与 `CompileState::{value_to_xreg,block_map,alloca_offsets}`（Value/Block/Inst 键）由 `HashMap` 换成 forge-ir 的 `SecondaryMap`（下标即句柄，不再逐次 SipHash）；`TargetLowering::lower_terminator` 的映射参数、`forge-dsl` 模板签名与 `prelude` 同步（值语义键 `.get(&x)` → `.get(x)`，并去掉模板里为借用而写的 `let cond_val = &cond_val;`）。
  A/B 实测（demo8 夹具 `compile_raw`，release，500 次 × 7 轮取中位数）：64 条指令 **116.7µs → 103.3µs（−11.5%，区间不重叠）**；256 条指令 412.7µs → 400.5µs（−3.0%，尾部与噪声重叠）。首轮单次计时曾给出反向 +12%，重复测量后判定为噪声——文件头记下"单次计时不算证据"。
  **`XReg` 键表未迁（如实记录）**：`XReg { index, class }` 的 `Eq`/`Hash` 含 class，同一 index 不同类是不同键；按 index 密化会合并条目（语义变化），故 `xreg_types`/`precolored`/`assignments`/`spill_slots`/`intervals`/`active` 保持 `HashMap`，待拍板"class 是否属于键"。新增守卫 3 例（`entity_tables.rs`：编译期 VReg 表密集性、源码级 `CompileState` 表密集性、XReg 键含 class 的可执行断言）。
  实测：workspace 1510 passed / 0 failed / 19 ignored；x86 195/3/0、riscv64 131/67/0、arm64 23/175/0。

+ **`LowerCtx` 带类型快照：生成物侧不再逐指令取锁（forge-ir v3 S3 余项①）**：`LowerCtx.type_ctx: Option<TypeContext>` → **`type_store: Option<TypeStoreRef>`**（lowering 入口取一次快照，lowering 期间类型表只读；`TypeStoreRef` 是 owned，解除了"守卫借着 `func`"的借用约束）。宿主的 `reg_class_for`/`mem_opsize_for`/`type_bits_of`/`type_bits_or_default` 与 `machine/pattern.rs` 的三个属性助手（参数改 `Option<&TypeStore>`）全部改读快照；`forge-dsl` 的 `quote!` 模板（`lowering.rs` 11 处 `tc.borrow()` + `integration.rs` 的逐属性一次性读封装）改读 `ctx.type_store`；`compiler.rs` 的宽向量可行性门由"每指令 × 每类型取锁"收成函数入口一次。
  取证：临时给 `TypeContext::borrow` 加 `#[track_caller]` 后按 `文件:行` 聚合——最大头是 `compiler.rs:1845`（每指令取锁）；改前编译一个函数取锁 **15 次（1 条指令）→ 252 次（80 条指令）**，改后 **恒为 10**。
  新增守卫 3 例（`crates/backend/forge-codegen/tests/lowering_read_path.rs`）：快照次数不随 IR 规模增长且 ≤16、编译期 0 次整表克隆、源码级"模板不得再出现 `type_ctx`/`.borrow()`"。`read_path_budget.rs` 预算：`forge-dsl/.../lowering.rs` 11 → 0（回潮守卫）、`pipeline/compiler.rs` 23 → 21。负向验证：宽向量门改回每指令取锁 ⇒ 规模用例 FAILED（14 vs 251）。
  实测：workspace 1507 passed / 0 failed / 19 ignored；x86 195/3/0、riscv64 131/67/0、arm64 23/175/0；LLVM 语料 198/254/0。

+ **`TypeContext` 锁 → 快照：读路径不再持锁（forge-ir v3 S3 切片）**：`store: Arc<RwLock<TypeStore>>` → `Arc<RwLock<Arc<TypeStore>>>`——读写锁只护住那个 `Arc` 指针：`borrow()` 取锁只为克隆 `Arc` 后立刻放锁，返回新的 `TypeStoreRef`（`Deref<Target = TypeStore>`），读作用域不再持锁；`borrow_mut()` 返回 `TypeStoreMut`，`DerefMut` 走 `Arc::make_mut`（无快照存活时原地改，有快照存活时克隆整表）。同时**删除 `impl Deref for TypeContext { type Target = RwLock<TypeStore> }` 逃逸口**（把原始锁暴露给调用方，与"入口显式化"相反；全仓无使用者）。
  语义变化如实记录：读快照是不可变视图（拿快照后 intern，旧快照看不到新类型）；改前"读锁存活期间 `borrow_mut()`"会自锁死，现在合法并触发一次整表克隆——因此"读快照不跨 intern"从**死锁**强制变为**性能纪律**，由新增的 `debug_cow_clone_count`（仅 debug）钉住。
  新增守卫 2 例：`snapshot_is_isolated_from_later_interning`（快照存活时写入 ⇒ 恰好 1 次整表克隆 + 新旧快照可见性；该用例本身即"读快照与写可并存"的证据）、`write_path_never_clones_in_real_workload`（真实负载 0 次克隆）。负向验证：去掉 COW 计数 / 在 `int_ty` 里故意持快照跨 intern ⇒ 各 FAILED。**不宣称吞吐提升**（未做 A/B 基准），宣称的是结构性质与"真实负载 0 克隆"这一可测后果。
  实测：workspace 1504 passed / 0 failed / 19 ignored；x86 矩阵 195/3/0；riscv64 131/67/0；LLVM 语料 198/254/0、往返幂等 189/189。

+ **splat 逐 lane 广播 + 向量元素类型名不再靠猜（forge-ir v3 S7 收官切片）**：①`ConstExpr::Splat` 在共享求值口 `const_expr_bytes` 里返回宽松 0（动它会让指令级常量池变序），故 `@s = global <4 x i32> splat (i32 7)` 的 lane 全是 0——现在**只在全局初值路径**广播（新增 `splat_init_bytes`：按向量元素类型逐 lane 重复内层标量；内层类型与元素类型不符即报错，非标量内层按既有 P1 占位零策略）。
  ②顺带挖出**向量元素类型名靠首字母猜**的静默损坏：`VecTy` 的 lexer 动作只做 `strip_prefix('i')/('b')/('f')`、其余落 `Ptr` ⇒ `float`→`f64`、`half`/`double`→`ptr`、`bfloat`→`i32`、`fp128`→`f64`（往返测试只比对 m1/m2、两边错得一样所以没暴露）。修：抽出唯一映射 `elem_ty_from_text`（`half`/`bfloat`→f16、`float`→f32、`double`→f64、`fp128`/`x86_fp80`/`ppc_fp128`→f128、`ptr`、`iN`/`bN`）。如实记录：`bfloat` 向量元素与 `half` 同为 `Float(16)`（标量 bfloat 另有 `BFloat`），`x86_fp80` 沿用既有约定映射 `f128`。
  新增守卫 2 例（`fidelity::splat_global_broadcasts_lane_value`、`fidelity::vector_element_type_names_are_not_guessed`，九种元素写法按打印形态钉住）；fuzz 生成器扩到 `double`/`half` 元素写法。负向验证：回退元素映射 / 回退 splat 广播 ⇒ 各 FAILED。
  实测：workspace 1502 passed / 0 failed / 19 ignored；LLVM 语料 198/254/0、往返幂等 189/189、结构化往返 198/0、10k fuzz 0 失败——与基线一致。**S7 全部切片落地。**

+ **解析错误恢复：一次尽量报多处（forge-ir v3 S7 切片）**：改前解析失败只报**第一处**。现在首条诊断**原样**输出（格式与既有断言不变），随后从出错偏移起把**该行剩余内容删掉**（保留换行 ⇒ 行号不变）重新解析，新错误若在更后面就再报一条并标注"（跳过第 N 行出错处后继续检查）"，上限 **3 条**；剩余部分已能解析 / 位置不再前进 / 出错处就在行尾 / 已到输入末尾即停。因为只删"出错行剩余内容"，后续诊断回显的源码行与行号都仍是用户文件里的那一行。
  **恢复只用于报告**：任一条诊断存在就仍是 `Err`——LLVM 语料"误接受 0"判据不变（`recovery_never_accepts_invalid_source` 钉住）。**边界**：只覆盖语法层，语义阶段（未知 opcode/SSA 违规）仍是报第一处（`semantic_errors_still_report_first_only` 钉住现状）。
  新增守卫 5 例（`tests/parse_error_diagnostics.rs`）：多错误、上限、恢复不放行、无进展/EOF 单条、语义边界。负向验证：停用恢复循环 ⇒ 2 例 FAILED；上限改 8 ⇒ 上限用例 FAILED。
  实测：workspace 1500 passed / 0 failed / 19 ignored；LLVM 语料 198/254/0、往返幂等 189/189、结构化往返 198/0。

+ **结构化 fuzz 扩面（forge-ir v3 S7 切片）**：`roundtrip_fuzz` 的全局生成器从 `global i32/i64 0|1|42` 扩到**保真矩阵**（i1/i5/i7/i24/i33/i128 十进制与十六进制、float/double/half/bfloat 的十进制与 `0x…` 位模式、`f0x…`、`zeroinitializer`、数组/结构体/`c"…"` 字符串聚合、向量字面量与 `splat`、常量表达式 init），断言侧新增**全局初始化字节对比**与**文本幂等**（`text1 == text2`）。10k 随机模块 0 失败。
  扩面立刻挖出三处"值悄悄丢"（往返只比对 m1/m2 两边时看不见）并修掉：①**向量字面量全局初值根本无法解析**（lexer 把 `<4 x i32> <i32 3, …>` 整段当一个 token，`TypeAndInit` 只拆 `zeroinitializer`）——新增 `GlobalInitVal::Vector` + `VecConstLit` 分支 + `vec_init_bytes`（逐 lane 按元素类型打包，lane 数/类别不符一律报错）；②**`parse_vec_lanes` 把每个 lane 都读成 0**（把整段 `i32 3` 喂给 `parse::<i64>()`，前缀必然失败）——改为先拆元素类型前缀、类别由前缀决定，lane 文本前缀也取自元素类型（`<4 x i16>` 不再打 `i32`）；③**`c"…"` 转义的收尾引号被吃掉**（`trim_end_matches('"')` 复数剥引号）：`c"T\22"` = `[84,34]` 打印成 `c"T\""` 后回读只剩 `[84]`——改为各剥一层
  `strip_prefix`/`strip_suffix`。
  回归钉子：`fidelity::vector_literal_global_keeps_lane_values`、`fidelity::malformed_vector_initializer_is_rejected`、`fidelity::escaped_trailing_quote_in_c_string_survives`。负向验证：回退 lane 前缀剥离 / 回退 `decode_c_string` ⇒ 2 例 FAILED，注掉 grammar 分支（强制重建生成物）⇒ 向量保真用例 FAILED。
  实测：workspace 1495 passed / 0 failed / 19 ignored；LLVM 语料 198/254/0、语料往返幂等 189/189、`display_llvm` 结构化往返 198/0 均与基线一致。

+ **`features = ["text"]` 文本层门控（forge-ir v3 S7 切片）**：门控前 `--no-default-features` **根本编不过**——两个核心实体字段直接存解析层 AST（`GlobalVariable::init_expr: Option<ConstExpr>`、`GlobalAlias::{aliasee_ty, aliasee}`）。
  ①这两个字段的唯一读点是 `display`（原样还原文本）、唯一写点是语义层，故改为**不透明文本载荷**：语义层构建时渲染（`e.to_llvm_string()`；别名拼 `fmt_parsed_type(ty) + " " + expr`，`Void` 占位不带前缀），核心存 `init_expr_text: Option<ImmStr>` / `aliasee_text: ImmStr`（与既有 `ifunc_params` 同一约定），display 改为原样输出文本。
  ②`default = ["text"]`、`text = ["dep:logos", "dep:lalrpop-util"]`（两者 `optional = true`）；`lalrpop` 是 build-dependency 不可选 ⇒ `build.rs` 读 `CARGO_FEATURE_TEXT` 决定是否生成 LALRPOP 表，`ops.toml` 的指令元数据生成**不随 feature 关**（核心也读 `Opcode` 表）。
  ③`lib.rs` 的 `display`/`ir_parser` 两个模块加 `#[cfg(feature = "text")]`。
  ④新增 `tests/text_feature_gate.rs`（4 例源码级边界守卫：核心文件含注释在内不得出现 `ir_parser`/`crate::display`、模块声明恰好一次且紧跟 cfg、manifest 的 `dep:`+optional、build.rs 只门控 LALRPOP）；CI Clippy job 增 `cargo clippy -p forge-ir --no-default-features --lib -- -D warnings`（`--lib` 必须——`tests/*.rs` 本来就需要 `text`）。
  负向验证：核心文件引入 `ir_parser` / 去掉 cfg 属性 / `default = []` / 把元数据生成挪进门控块 ⇒ 四条守卫各自 FAILED，恢复后全绿。行为零变化：LLVM 语料 198/254/0、语料往返幂等 189/189、`display_llvm` 结构化往返全绿。
  实测：workspace 1492 passed / 0 failed / 19 ignored；无 feature 的 check/clippy（debug + release）0 错 0 警告。

+ **IR 侧 span 贯穿：指令位置落进 `Instruction::loc`（forge-ir v3 S7 切片）**：改前实测整条解析链路上 `Instruction::loc` **恒为 `None`**——语法层不留字节区间（`ParsedBlock` 只有 `insts`），`FunctionBuilder::set_current_loc`/`emit1` 早已会把位置写进指令，却**没有任何调用者**，诊断只能指到函数级。
  现在三层各加一处真相：①语法层新增 `SpannedInst = <l: @L> <i: Inst> <r: @R>`（`grammar.lalrpop`），两个 `Block` 产生式改吃 `(<SpannedInst>)*`，由 `ast_items::split_spanned_insts` 拆出与 `insts` 平行的 `inst_spans`；
  ②`parse_to_ast` 用 `locate` 把字节偏移换算成 **1-based `(行, 列)`**（`ParsedBlock::inst_line_cols`；长度不等时视为无位置，不猜）；③`build_function` 发射每条指令前 `set_current_loc(...)`、循环尾复位，使指令内物化的常量随所属指令、循环后为终结符物化的合成常量**不继承**上一条位置。
  边界如实记录：`phi` 绑块参数、不发射指令，终结符走 `build_terminator`（不经 builder）⇒ 两者暂无位置；位置不进文本层（打印 → 重解析 → 再打印逐字节相同）。新增 `tests/source_span.rs`（4 例，含"列号必须来自源码"的非固定缩进夹具）；负向验证：删循环尾复位 ⇒ 2 例 FAILED，删发射前落位 ⇒ 3 例 FAILED，恢复后全绿。
  实测：workspace 1488 passed / 0 failed / 19 ignored；LLVM 语料 198/254/0、语料往返幂等 189/189 不变。

+ **half/bfloat 位模式保真 + splat 文本往返：语料往返漂移 2 → 0（forge-ir v3 S7 切片）**：收掉最后两条漂移，189 个正向用例**全部幂等**（`KNOWN_DRIFT` 清空）。
  ① `float-literals.ll` 的 **f16/bfloat 值静默丢失**：`@2 = global half -qnan` 打印成 `0x7fc00000`（4 字节、无 `H` 前缀），回读按整数截断成 `0x0000`；根因是 `float_init_bytes` 没有 f16/bfloat 分支（落 `(f as f32)` 存 4 字节）。现在 `float_init_bytes` 增 f16（f32 → IEEE binary16，RN-even，含 denormal/Inf/NaN）与 bfloat（高 16 位 RN-even）；
  `fmt_global_init` 增 `0xH{:04x}`/`0xR{:04x}` 打印臂；grammar 的 `FloatHexLit`（`0xH...`/`f0x...`）由折成 `Float(0.0)` 改为 `Int(位模式)`；lexer 同时剥离 `f0x` 前缀（`llvm-dis` 的 half 输出形式）并**真正解码 C99 十六进制浮点**（`0x1.e3p-16`，此前恒 0.0）。实测 `+0x1.e3p-16` 打印 `0xH01e3`，与 LLVM 期望的 `f0x01e3` 一致。
  ② `constant-splat.ll` 的 **splat 常量折成空向量**（既丢值又多一个 `<1 x i32>` 类型前缀）：新增 `ConstExpr::Splat(ParsedType, Box<ConstExpr>)`，语法保留表达式、打印回 `splat (i32 7)`（必须带内层类型，否则自己解析不回来），并去掉向量常量打印里重复的类型前缀。**逐 lane 广播值仍未落地**——实测"取内层值"会让指令级常量池变序（`display_llvm` 结构化往返在 `constant-splat.ll` 上失败，`Iconst` ConstId 2 vs 0），故保持宽松 0、文本层原样往返。
  结果：语料往返漂移 **2 → 0**（幂等 189/189）；LLVM 语料正向 198/452、负向正确拒绝 254、误接受 0 不变；`display_llvm` 结构化往返全绿。新增回归用例 `fidelity::half_and_bfloat_globals_roundtrip`（5 组）与 `fidelity::splat_constant_roundtrips_without_type_prefix`；负向验证（回退位模式映射 + splat 内层类型 + 强制重建）⇒ 3 例 FAILED，恢复后全绿。
  实测：workspace 1484 passed / 0 failed / 19 ignored；x86 矩阵 195/3/0；riscv64 131/67/0。

+ **聚合常量往返幂等：语料漂移 3 → 2（forge-ir v3 S7 切片）**：收掉 `unnamed.ll` 那条漂移，逐例打差异后定位两处根因，都在"聚合常量子元素"上：①浮点子元素被打印成整数字面量——`fmt_agg_scalar` 用 `format!("{}", f32)`，Rust 对 `4.0` 打 `"4"`，文本 `float 4` 回读成了 **i32** 常量（类型漂移）；现在用 `fmt_f32_literal`/`fmt_f64_literal` 保证带小数点或指数（`4` → `4.0`），非有限值给 hex 位模式。②聚合元素的零值被压成 i8 标量——`agg_const_from_operands` 对 `zeroinitializer`/`undef`/`poison`/`null` 元素一律 `insert_int(0, 8)`，元素类型是结构体时（`%1 zeroinitializer`）文本成了 `i8 0`，与聚合类型不符；现在新增 `zero_agg_child`：标量 → 零标量，**结构体/数组 → 递归零聚合**（`%1
  { i32 0 }`）。③顺带修整数子元素：用**元素类型**位宽打印，而不是常量池里记的宽度。
  结果：`unnamed.ll` 转幂等，往返漂移 **3 → 2**（余：`constant-splat.ll` 的 splat 展开与类型前缀、`float-literals.ll` 的 f16 hex 形态），幂等 **187/189**；LLVM 语料正向 **198/452**、负向正确拒绝 254、误接受 0 不变。新增回归用例 `fidelity::nested_zero_aggregate_roundtrips`；负向验证：临时关掉 `zero_agg_child` 的结构体分支 ⇒ 该用例 FAILED 且守卫报 `unnamed.ll` 为新漂移。
  实测：workspace 1482 passed / 0 failed / 19 ignored；x86 矩阵 195/3/0；riscv64 131/67/0。

+ **命名元数据往返幂等：语料往返漂移 8 → 3（forge-ir v3 S7 切片）**：上一片留下的最大一类往返漂移（5 个 DI/`!named` 用例）定位到根因并修掉。逐例打差异后发现真相与最初猜测不同——命名元数据的**名字与内容都没丢**（`lookup_named` 两次都在），漂移来自 **id 空洞**与**文本顺序**：解析器为显式 `!N` 预留槽位时会把中间空洞补成空 tuple（文本里有 `!15` 与 `!19` ⇒ 16..18 成为 `!{}`），这些空洞被打印后二次解析成了"显式定义"，命名节点 id 整体后移；同时打印顺序把命名节点按 store id 内联在数字节点之间，于是文本与 id 绑定。
  现在：①`MetadataNode` 增加 **`Placeholder`** 变体，`MetadataStore::insert_at` 的空洞填充改用它——与用户写明的空 tuple（`!0 = !{}`）区分开；②display **先输出命名 metadata**（LLVM 风格，文本与 id 无关），再按 id 输出数字节点并**跳过无人引用的 `Placeholder`**（被引用的仍照打，否则 reparse 会引用未定义的 `!N`）；③`!tbaa` 形状检查对 `Placeholder` 保持宽松（引用未定义节点时无从校验；实测不放宽会让语料正向收敛数 198 → 197）。
  结果：往返幂等 **186/189**（此前 181），漂移 **8 → 3**；LLVM 语料正向 **198/452**、负向正确拒绝 254、误接受 0——均与基线一致。`tests/corpus_roundtrip.rs` 的 `KNOWN_DRIFT` 收窄到 3 条、计数同步更新；负向验证：关掉"跳过空洞占位" ⇒ 5 个 DI 用例重新漂移、守卫 FAILED；删掉 `KNOWN_DRIFT` 一条 ⇒ 守卫点名"新漂移" FAILED。
  实测：workspace 1481 passed / 0 failed / 19 ignored；x86 矩阵 195/3/0；riscv64 131/67/0。

+ **语料往返断言扩面 + 两类保真缺陷修复（forge-ir v3 S7 切片）**：把"往返断言"落到真实语料——LLVM 官方 test/Assembler 的 189 个正向用例必须 `parse → print → parse → print` **幂等**（打印机做规范化，首轮不必等于原文，但规范化必须收敛）。实测：189 个 reparse 全部成功，但 9 个不幂等，逐例打差异后定位三类根因并修掉两类：
  ① **非字节位宽整数常量**：`@g = global i5 7` 打成 `0x07000000`、reparse 后又变 `0x00000007`——`int_init_bytes` 对非 8/16/32/64 位宽落 `(v as u32)` 存了 4 字节，打印端又把 LE 字节当十六进制原样输出；现在按 `ceil(bits/8)` 字节存储、打印端新增非字节位宽臂（LE 解码后十进制，`i5 7`）。
  ② **浮点类型上的整数字面量 = 位模式**：`global double 0x7FF0000000000000` 本应是 `+inf`，实测静默变成 `0.0`（`0x7FEFFFFFFFFFFFFF` 变 `0xffffffff`）——同一个 `_ => 32 位` 兜底把 64 位位模式截断；现在浮点类型按位模式编码（16/32/64），并把 f32/f64 的**大整数**打印改走 `0x` 位模式（`f64::MAX` 原先打 100+ 位十进制，解析端按整数读溢出 ⇒ 往返变全 1 位模式）。
  漂移 9 → **8**，幂等 180 → **181**。新增 `tests/corpus_roundtrip.rs`（3 例）：reparse 必须全成功、漂移必须恰好等于 `KNOWN_DRIFT`（8 条逐条记原因、必须被命中）、正向可 parse 数（189）与幂等数（181）精确相等；另含两类修复的回归用例。负向验证：删掉 `KNOWN_DRIFT` 一条 ⇒ 守卫点名"新漂移" FAILED；关掉 64 位浮点位模式分支 ⇒ 回归用例 FAILED。
  实测：workspace 1481 passed / 0 failed / 19 ignored；x86 矩阵 195/3/0；riscv64 131/67/0。仍在 `KNOWN_DRIFT` 的三类（命名元数据回读丢名字/列表、splat 向量全局多打类型前缀、聚合常量元素类型丢失）已有实测记录，留待后续切片。

+ **解析错误诊断可用（forge-ir v3 S7 文本层切片）**：`ir_parser::parse_to_ast` 此前把 lalrpop 的原始错误 `format!("{:?}", e)` 直接当消息，实测输出是 `UnrecognizedToken { token: (17, Ident("entry"), 22), expected: ["Target", "VoidTy", … 共 88 项 …] }`——**没有行列、没有出错处的源码行**，还把 88 个文法内部记号名（`VconstOp`/`UselistorderBbKw` 这类）倒给用户。
  现在 `parse_module` 的语法错误是：`解析错误 2:1：非预期 entry` + 回显该行源码 + 插入符 `^` + 期望集合收敛到 8 项（`…（共 N 个）`），内部记号名做用户化映射（`RBrace`→`}`、`IntTy`→`iN`、`VecTy`→`<N x T>`、`*Kw` 去后缀小写、`IntLit`→`整数常量`、`LocalId`→`%局部名`）；EOF 报"输入在结构未结束时结束"，非法字符报"出现无法识别的字符"；坏偏移/非字符边界一律回退（诊断路径不 panic）。顺带把 `LexError` 改成 `BadChar { offset }` / `Rejected` 两态——词法错误此前丢掉了 logos 给出的位置。
  新增 `tests/parse_error_diagnostics.rs`（4 例）；负向验证（改回 `{:?}`）⇒ 4 例全 FAILED，恢复后全绿。实测：workspace 1478 passed / 0 failed / 19 ignored；x86 矩阵 195/3/0；riscv64 131/67/0；LLVM 语料正/负向断言不变。

+ **读路径纪律批量落地 + 静态预算守卫（forge-ir v3 S3 切片）**：把"入口取一次读锁、把 `&TypeStore` 显式传下去"铺到剩余的纯读面。先按"函数体内是否有 `borrow_mut()` / 是否显式 `drop(guard)`"分类——`RwLock` 不可重入，读写交错的函数持有长读锁会自锁死，而这些函数当初写 `drop(ts)` 正是为了在读段之间放锁，属"已正确但不能改成长锁"，因此只迁移纯读函数：
  `pipeline/compiler.rs` `borrow()` **30 → 23**（7 个 `expand_*`/`memoryize_from_segs` 入口各一次）；`ir_parser/semantics.rs` **32 → 24**（6 个纯读助手 `pack_*_init`/`int_init_bytes`/`float_init_bytes`/`agg_const_from_operands`/`encode_lanes_to_bytes` 改 `&TypeStore`）；`types.rs` 13 处是 `TypeContext` 一次性查询封装（保留，新增用例钉住"每次调用恰好 1 次读锁、intern 不取读锁"）。
  `func: &mut Function` 的函数里不能用 `func.types.borrow()` 提升（守卫借着 `func` ⇒ 后续 `&mut func` 报 E0502），改用 `let types_ctx = func.types.clone();`（Arc，O(1)）再取锁。
  **实测发现**：`forge-dsl/src/v12/codegen/lowering.rs` 那 11 处 `borrow()` **不是普通代码，而是 `quote! { ... }` 模板文本**（生成的 lowering 在 codegen 期 `tc.borrow()`）——迁移它要同时改生成物与 `LowerCtx` 形态，单列为余项，本切片只用预算锁住。
  新增 `tests/read_path_budget.rs`（2 例）：逐文件把 `borrow()` 处数钉成**精确相等**的预算（每条写清"为什么还剩这些"，并断言文件存在防腐烂）；负向验证（往 `compiler.rs` 插一行 `borrow()`）实测 24 ≠ 23 ⇒ FAILED。
  实测：workspace 1474 passed / 0 failed / 19 ignored；x86 矩阵 195/3/0；riscv64 131/67/0。

+ **读路径纪律落到校验器（forge-ir v3 S3 切片）**：`verify.rs` 此前有 13 处 `borrow()`——`check_conversion`/`check_immediates`/`check_gep_indices`/`check_operand_types` 等在**每条指令上各取一次读锁**（`check_conversion` 里 `scalar_bits`/`vector_bits` 两个闭包还各自取锁），取锁次数与指令数成正比。
  现在 `verify()` 取**一次**读锁（`ctx` 先 `clone`（Arc，O(1)）再 `borrow()`，守卫借用局部而非 `self`，因此仍可调 `&mut self` 检查函数），`&TypeStore` 贯穿 `check_types` → `check_operand_types` → `check_conversion`、`check_type_refs`、`check_immediates` → `check_gep_indices`；`is_pointer_ty`/`scalar_bits`/`class_of`/`class_matches` 改为接收 `Option<&TypeStore>`。实测 `verify.rs` 的 `borrow()` **13 → 1**。
  新增守卫 `verifier_read_locks_do_not_scale_with_ir_size`：1 条指令与 80 条指令的函数取锁次数必须相等（实测均 3 = 入口 1 + `Function` 内部查询 2）；负向验证（逐指令循环里插一行 `clone`+`borrow()`）实测 5 vs 84 ⇒ FAILED，删除后全绿。顺带实测到：这类退化**多数情况下连编译都过不了**——守卫借着 `self.ctx` 时不能再对 `self` 取 `&mut`（E0502），借用检查器已经封死"守卫跨 `&mut self` 调用"。
  实测：workspace 1471 passed / 0 failed / 19 ignored；x86 矩阵 195/3/0；riscv64 131/67/0。

+ **类型读路径纪律：`&TypeStore` 显式传参（forge-ir v3 S3 切片，display 落地）**：`TypeContext::borrow()` 每次都是一次 `RwLock` 读锁获取，而 `RwLock` **不可重入**（读锁存活期间再 `borrow_mut()` intern 新类型 = 自锁死）；`display.rs` 此前有 **51 处** `borrow()`（`value_as_literal` 每值一次、`NameResolver::new` 每块/值各一次），打印一个模块的取锁次数与函数体大小成正比。
  现在 display 全量改为"**入口取一次锁、把 `&TypeStore` 显式传下去**"：`FunctionDisplay`/`BlockDisplay`/`InstDisplay`/`TerminatorDisplay`/`NameResolver`/`value_as_literal`/`fmt_phi_value` 的参数与字段改为 `&TypeStore`，`Display for Module` 与 `function_to_string` 各取一次（布局也读已取到的 store，避免 `Module::data_layout()` 再取一次）——非测试路径 `borrow()` **51 → 2**（这 2 处就是入口本身）。
  `TypeContext` 新增**仅 `debug_assertions`** 的读锁计数（`debug_read_count`/`debug_reset_read_count`；release 下字段与方法都不存在，零开销），并新增 `tests/type_store_read_path.rs`（3 例）钉住"打印一个模块/一个函数各只取 1 次读锁、重复打印 3 次 = 3 次"。负向验证：在 `Module::fmt` 临时加一行 `borrow()` ⇒ 计数 2、守卫 FAILED；删掉后全绿。
  实测：workspace 1470 passed / 0 failed / 19 ignored；x86 矩阵 195/3/0；riscv64 131/67/0。

+ **坏 IR 的越界 `TypeId`/`SigRef` 变成诊断，不再 panic（forge-ir v3 S3 切片）**：`TypeStore::get`/`get_signature` 是 fail-closed 索引（`entries[id]`/`signatures[sr]`），而 IR 里的类型句柄是**数据**。临时探针实测两条崩溃路径：越界 `SigRef` 走校验器 ⇒ `signatures[9999]` `index out of bounds`；越界 `TypeId` 走 **display** ⇒ `entries[9999]` 越界（"把坏 IR 打印出来给人看"这个最需要诊断的时刻反而崩了）。
  现在：`TypeStore` 增加容忍坏 IR 的读取口 `entry_opt`/`signature_opt`/`contains_type`（`get`/`get_signature` 保持 fail-closed 并注明是良好 IR 的快路径）；校验器新增前置自检 `check_type_refs`（扫签名句柄 + 签名参数/返回类型 + 全部值类型 + 块参数类型），越界即报新错误码 `BadTypeId`/`BadSigRef` 并跳过后续类型相关检查（结构类检查照常跑完一起上报；错误码 31 → 33）；display 的 11 处类型查询改走 `entry_opt`，`fmt_llvm_type` 对越界 id 打印 `<bad-type:N>` 占位符，5 处 `size_bytes`/`alignment` 收进容忍助手（存储自身查询仍 fail-closed）。
  新增 `tests/bad_type_id_robustness.rs`（4 例）：越界值类型 / 越界签名返回类型 / 越界 `SigRef` 均返回诊断，坏 IR 打印出占位符；其中两条在改前实测 panic。实测：workspace 1467 passed / 0 failed / 19 ignored；x86 矩阵 195/3/0；riscv64 131/67/0。

+ **`DataLayout` 单一数据源（forge-ir v3 S3 切片）**：`Module::set_data_layout` 此前是 `self.types = TypeContext::with_data_layout(..)`——**整体替换类型存储**。实测两条后果：①改布局前 intern 的签名/类型全部丢失（`get_signature` 直接在 fail-closed 分支 panic，文档里"必须在 `add_function` 之前调用（重建会清空已注册签名）"就是这条缺陷的自述）；②改布局前构建的 `Function` 持有旧存储 ⇒ 同一模块内模块侧与函数侧的指针宽度/大小/对齐各算各的（实测 `size_bytes(ptr)` 一侧 4、一侧 8）。
  现在：新增 `TypeStore::set_data_layout`（原地写那一个字段，无需失效任何缓存——布局派生结果都是查询期现算，`TypeKey` 不含宽度/对齐）；`Module::set_data_layout` 改为原地更新共享存储，**删除 `Module.data_layout` 副本字段**（唯一副本在存储里）并新增 `Module::data_layout()`，"必须在 `add_function` 之前调用"的约束消失；删除 `TypeContext::with_data_layout`（无调用方）；迁移 3 处字段读取与解析器注释。新增 `tests/data_layout_single_source.rs`（4 例，其中 2 例在实现前实测 FAILED：签名丢失 panic、指针宽度 8 ≠ 4）。
  实测：workspace 1463 passed / 0 failed / 19 ignored；x86 矩阵 195/3/0；riscv64 131/67/0。

+ **`AnalysisManager`：惰性分析缓存改为"修订号自校验 + `Arc` 快照"（forge-ir v3 S6 切片，S6 最后一项余项）**：`Function` 上的 `OnceLock` 缓存（前驱/后继/支配树/循环森林）换成 `AnalysisSlot<T>`（`RwLock<Option<(修订号, Arc<T>)>>`），由新类型 `AnalysisManager` 拥有。修订号 = `(DFG 结构修订号, 显式失效次数)`；结构修订号（`DataFlowGraph::cfg_revision`）在**三条改 CFG 的路上**自动前进——块增删、终结符写入、**终结符被墓碑化**。读到过期修订即重算并返回当前快照 ⇒ **陈旧分析结果不可能被读到**，`invalidate_analysis()` 降级为"少算一次"的优化，正确性不再依赖调用方记得失效。
  实测出的两条旧漏洞：①`tombstone_inst` 不失效缓存，而块内顺序表不含终结符 ⇒ "就地墓碑化终结符"是绕过 `set_terminator` 的改图路，读者会拿到改图前的后继；②`func.dfg.make_block()`/`dfg.remove_block()` 直接改结构，没有 `&mut Function` 可用来失效。
  访问器返回 `Arc` 快照而非 `&T`："取一份分析 → 改 CFG → 再用"不再出现同一次使用期内前后不一致（旧引用语义会指向被就地改写的缓存）。**`forge-opt`/`forge-codegen` 零改动**（`Arc` 的 Deref 让既有调用与 `&SecondaryMap` 形参原样可用；13 处 `.clone()` 语义不变，从深拷贝变成 `Arc` 克隆）。
  校验器的 `AnalysisCacheStale` 检查与错误码随旧语义一并删除（错误码 32 → 31，无过渡层）；`tests/analysis_cache.rs` 重写为 8 例（写入口后必须看到新图、快照在改图后恒定、墓碑化终结符、绕过 `Function` 直接建块、`kill_inst` 终结符、`retarget_terminator`、反面"不改控制流的写入必须复用缓存"（`Arc::ptr_eq`）、显式失效仍可用）；两条负向探针各验一次（关掉对应 bump ⇒ 对应测试 FAILED，恢复后全绿）。
  实测：workspace 1459 passed / 0 failed / 19 ignored；x86 矩阵 195/3/0；riscv64 131/67/0。

+ **错误码 × 回归测试对账守卫（forge-ir v3 S6 切片）**：`verify.rs` 的 `VerifyError` 现为 **32 个变体**，而 `tests/verify_negative.rs` 的文件头一直写着"27 个错误码全部有回归测试"——错误码 30 → 32 的两片都没人回写，属"文档声称 vs 实测"漂移。
  逐变体实测引用数后补齐 4 例：`OperandCountMismatch`（操作数个数不符）、`MultipleEntryBlocks`（多入口块，入口自带参数）、`DominanceViolation`（定义不支配使用）、`MissingTypeContext`（`TypeContext` 查不到类型）——这 4 个此前**既不在 `tests/*.rs` 出现、源码内 `VerifyError::<名>` 引用也不足 3 次**（crate 内单测同样没覆盖）。
  新增守卫 `every_verify_error_variant_has_a_regression_test`：解析 `src/verify.rs` 的枚举变体，要求每个变体在 `tests/*.rs` 里以 `VerifyError::<名>` 出现，或在同源码内出现 ≥3 次（构造点 + `Display` 臂 + crate 内单测），否则必须进本测试的 `ALLOWED`（当前为空；白名单条目必须被命中，否则报"条目已失效"）。守卫已用**负向探针**验证：临时插一个只有定义 + `Display` 臂的 `ProbeVariantNoTest` ⇒ 守卫 FAILED 并点名该变体，恢复后 green（改用测试侧改名做负向是无效的——只会得到 E0599 编译错，反而掩盖守卫是否生效）。文件头改为"对账交给守卫"，不再写会漂的计数。
  实测：`verify_negative` 44 例；workspace 1455 passed / 0 failed / 19 ignored；x86 矩阵 195/3/0；riscv64 131/67/0。

+ **校验器 severity 分级 + 健壮性钉住（forge-ir v3 S6 切片）**：新增 `VerifySeverity::{Warning, Error}` 与 `VerifyError::{severity, is_error}`——分界线是"合法 IR 上的可疑现象"vs"不变量被破坏"，`UnreachableBlock`（不可达死块，LLVM 与本仓注释都视为合法，`check_reachability` 本就对它做了死 merge 豁免）降为唯一的建议级，其余保持违规级；`forge-opt` 的 `PassVerify::Error` 改为只在违规级失败、建议级记 `log::warn!`。
  新增 `tests/verifier_robustness.rs`（8 例）：越界跳转目标、越界操作数句柄、被跳转的未终止块、`remove_block` 后被指向的块、截断的 `switch` case 表、死块建议级、结构违规违规级、合法 IR 对照组——**实测 6 个坏 IR 夹具全部返回错误、无 panic**（含前两片新增的 `expect("投影命中…")` 路径）。顺带删掉 `forge-opt` 中一段自相矛盾的过时注释（上半段说严格校验"必然失败"，下半段已写"两处都已修…全绿"）。

+ **校验器重算 CFG/支配树并比对 + 墓碑规范形态（forge-ir v3 S6 切片）**：新增 `VerifyError::{AnalysisCacheStale, TombstoneNotCanonical}`（错误码 30 → 32）。`Function::{predecessors, successors, dominator_tree}` 是 `OnceLock` 惰性缓存，此前**所有改控制流的写入口都不失效缓存**（全靠 `forge-opt` 8 处 pass 各自记得 `invalidate()`），改完 CFG 读到旧分析结果是静默错误且没有任何测试会失败。
  现在：①新增 `Function::invalidate_analysis()`（幂等 O(1)），`set_terminator`（所有按形式写入口的公共底层）/`retarget_terminator`/`kill_inst` 内部调用 ⇒ 写完自动失效；②校验器新增 `check_analysis_cache`：现场重算 successors/predecessors/支配树与**已初始化**缓存逐块比对，不一致报 `AnalysisCacheStale { what, block, cached, fresh }`（兜住绕过 `Function` 直改 `dfg` 的漏网）；③新增 `check_tombstones`：`is_tombstone()` 为真者不得仍带 operands/immediates/metadata/param_attrs/isel_strategy，否则报 `TombstoneNotCanonical`。实现中发现 `dfg.insts()` **会跳过墓碑**，校验器看不到它们——补 crate 内 `all_insts()`（原始 arena 迭代）。
  守卫 `tests/analysis_cache.rs`（4 例）+ `verify.rs` 内单测（伪墓碑上报；crate 外造不出伪墓碑，字段私有）。

+ **墓碑语义显式化：`Instruction::is_tombstone()` 成为唯一判据（forge-ir v3 S2 切片）**：删除是"标墓碑"而非回收槽位，但"是不是墓碑"此前靠 `opcode == Nop` 猜——而 `Opcode::Nop` **是合法指令**（`FunctionBuilder::nop()` 发一条进 `inst_order`）。全仓实测三种答案（12 处 `matches!(opcode, Nop)`、4 处 `Nop && results.is_empty()`、1 处 `opcode == Nop`）：合法 Nop 被当成墓碑跳过，而带 results 的就地墓碑反被当成活指令。
  现在唯一事实源是 `Instruction.tombstone: bool`（私有）+ `is_tombstone()`，唯一实现是 `DataFlowGraph::tombstone_inst_low`（标标志 + Nop + 清 operands/immediates + **清附件** metadata/param_attrs/fn_attrs/isel_strategy，此前附件留在墓碑上）。两档语义共用它：**删除**（`remove_inst`/`kill_inst`，额外摘 `inst_order` 条目 + 清 results + 值 VOID）与**就地**（新增公开入口 `Function::tombstone_inst`，保留顺序表条目与 results，附 use-lists 重登记）——`forge-codegen` 8 处手写墓碑块全部改走它；4 处复合判据与 2 处"数墓碑"改 `is_tombstone()`，lowering 边界的 `matches!(opcode, Nop)` 按原意保留（任何 Nop 都不产生机器码）。
  守卫 `tests/tombstone_semantics.rs`（4 例：删除语义规范终态、**合法 `nop()` 不是墓碑**、就地墓碑化保留 results 但清附件、源码断言"`opcode = Opcode::Nop` 只许出现在 `dfg.rs`"，已用负向探针验证会失败）。

+ **句柄字段私有化：10 个裸 u32 句柄不再能凭空构造（forge-ir v3 S2 切片）**：`Value`/`Inst`/`Block`/`TypeId`/`FuncRef`/`GlobalId`/`SigRef`/`AggId`/`VReg` 的字段由 `pub u32` 降为 `pub(crate)`，统一出入口 `::new(u32)` + `.index()`；`ConstId` 为 `::from_raw(u32)` + `.raw()`（打包值）+ 既有 `.index()`（低 30 位池内索引）+ `.tag()`。此前 crate 外可 `Value(999)` 造句柄（坏句柄从构造点泄漏到下游），也可 `v.0` 直读索引（句柄表示成了公开契约）。
  迁移面：本仓 178 + 45 处编译错误（按 `--message-format=json` 的 byte span 打补丁，4 轮收敛）；**DSL 生成器模板 32 处**（`forge-dsl/src/v12/codegen/{lowering,placeholder,machine,integration}.rs` 的 `quote!` 文本——生成物里的 `Block(...)`/`ConstId(...)`/`.0` 必须改生成器源）；`forge-rustc`（本机不可编译）走文本审计后定点修补。
  **踩坑**：机械规则"private field → `.index()`"对 `ConstId` 是错的——`ConstId::index()` 是低 30 位池内索引，而 `.0` 是含 tag 的打包值；整包测试立刻抓到 20 个 JIT 用例错值（float/vector 常量全错），改 `.raw()` 后恢复。守卫 `tests/entity_privatization.rs`（4 例：往返/Default/Display、`ConstId` 三者语义区分、源码断言句柄字段必须 `pub(crate)`，已用负向探针验证会失败）。

### Changed (2026-09-15)

+ **`dfg` 私有化第三步：`blocks` arena 收口——三个 arena 至此全部私有（forge-ir v3 S5 第 6 项）**：`DataFlowGraph.blocks` 由 `pub` 降为 `pub(crate)`——读走 `block`（fail-closed：句柄不合法即 panic，与 `value_data`/`inst_data` 同契约）/ `block_opt`（容忍坏 IR）/ 新增 `block_data_iter`（无句柄迭代）/ 既有 `block_count`/`block_params`/`block_param_values`/`block_terminator`/`block_inst_iter`；写走 `block_mut`（就地编辑口），结构性增删仍只有 `make_block*`/`remove_block`。
  **契约**：`inst_order` 是块内指令顺序的唯一事实源（只在"插入/搬移"实现里改）；`params`/`param_values` 的改动须与 `Function::{add_block_param, remove_block_param}` 口径一致；写终结符走 `Function::{jump, branch, ret, …}`。新增 `block_data_iter` 的理由：原代码大量用 `blocks.iter().enumerate()` 取块序下标，而 `blocks()` 产出 `(Block, &BlockData)` 元组——无句柄迭代口让 23 处成为纯文本替换、语义零变化。
  迁移面实测：75 处 `dfg.blocks[..]` + 1 处裸 `dfg.blocks[..]` + 40 处 `len()` + 23 处 `iter()` + 8 处 `&mut …` + 1 处 `get(..)` + 2 处 `is_empty()` + 1 处整体借用，另含 `benches/compile_bench.rs`（首次把 benches 纳入迁移面）。顺带修 8 处 `&mut …inst_order` 前缀被吞、6 处多行 `.dfg\n.blocks\n.iter()` 链、2 处经读口 `inst_order.clear()`。守卫 `tests/dfg_privatization.rs` 扩到 13 例（block/block_opt/block_data_iter/blocks()/block_count 口径一致、越界 fail-closed、block_mut 就地编辑），源码断言扩成 `dfg.values`+`dfg.insts`+`dfg.blocks` 三字段（负向探针验证会失败）。

### Changed (2026-09-15)

+ **`dfg` 私有化第二步：`insts` arena 收口 + `inst_mut` 就地编辑口（forge-ir v3 S5 第 5 项）**：`DataFlowGraph.insts` 由 `pub` 降为 `pub(crate)`——读走 `inst_data`（fail-closed：句柄不合法即 panic，与 `value_data`/`BlockData::terminator` 同契约）/ `inst_data_opt`（容忍坏 IR）/ 既有 `inst_opcode`/`inst_operands`/`inst_results`/`inst_block`/`insts()`/`inst_count`；写走 `inst_mut` / `inst_mut_opt`（`(dfg, Inst)` 寻址的就地编辑口），结构性增删仍只有 `make_inst*`/`remove_inst`。
  **契约**：安全字段是 `opcode`/`immediates`/`flags`/`mem_flags`/`param_attrs`/`fn_attrs`/`metadata`/`loc`/`isel_strategy`；`operands`/`results` 不在此列——改操作数走 `replace_all_uses`/`apply_replacements`，或"就地改写 + `refresh_inst_uses` 重登记"。另加 crate 内 `insts_iter_mut()`（`Function::apply_replacements` 用），使 `dfg.insts` 字段语法在 `src/` 里归零。
  迁移面实测 39 个文件：216 处索引 + 8 处裸 `dfg.insts[..]` + 11 处 `get(..)` + 3 处 `get_mut(..)` + 1 处 `iter()` + 25 处 `&mut …`；顺带修 8 处"经读口做写操作"（`set_isel_strategy`/`attach_metadata`/`flags |=`/`results.push`）、约 20 处 `&Inst` 接收者、4 处整数/`usize` 索引、6 处 `iter()`→`insts()` 的闭包解构、4 处多行 `.dfg\n.insts` 链。守卫 `tests/dfg_privatization.rs` 扩到 10 例（含"`inst_mut` 改操作数后必须 `refresh_inst_uses`，use 计数与 verifier 双重校验"），源码断言扩成 `dfg.values`+`dfg.insts` 双字段（负向探针验证会失败）。`blocks` arena 的收口是后续切片。

### Changed (2026-09-15)

+ **`dfg` 私有化第一步：`values` arena 收口 + `set_value_type` 受限写入口（forge-ir v3 S5 第 4 项）**：`DataFlowGraph.values` 由 `pub` 降为 `pub(crate)`——读走 `value_data`（fail-closed：句柄不合法即 panic，与 `BlockData::terminator` 同契约）/ `value_data_opt`（容忍坏 IR）/ 既有 `value_def`/`value_type`/`values()`/`value_count`；写走唯一入口 `set_value_type(v, ty) -> bool`（越界不写返回 `false`），创建与墓碑化仍只在 `dfg.rs` 内。
  迁移面实测 38 处（31 处 `dfg.values[..]` + 7 处 `dfg.values.get(..)`，含 3 处写：`const_fold` 常量定宽、`gvn` 折叠类型、`algebraic` 结果类型），跨 forge-ir / forge-codegen / forge-opt 共 14 个文件；顺带修 4 处借用冲突——`value_data` 等借用整个 `dfg`，破坏了原先靠 `dfg.insts[..]` 与 `dfg.values[..]` 字段级不相交才成立的借用（gvn 的 `inst_ids` 改复制、algebraic/gvn 把读类型提到取 `&mut inst` 之前）。守卫 `tests/dfg_privatization.rs`（6 例，含"`src/` 里 `dfg.values` 字段访问为 0"的源码断言，已用负向探针验证会失败）。`insts`/`blocks` 两个 arena 的收口是后续切片。

### Changed (2026-09-15)

+ **metadata 单写：5 个载体的附件各收成一个写入口，字段私有化（forge-ir v3 S5 第 3 项）**：附件 metadata（`(kind, node)` 对）此前有多条写路径且语义不一致——指令可经构造参数或 `inst.metadata.push(..)`、终结符要经 `term_metadata_mut` 逃逸出的 `&mut SmallVec`、函数是 `func.metadata.push(..)`、全局变量是 `gv.metadata = attached`（整表替换）。
  现统一为唯一入口：`Instruction::{metadata, attach_metadata}`、`DataFlowGraph::{term_metadata, attach_term_metadata}`（返回 `TermMetadataAttach::{Attached, NoTerminator, Unreachable}`，取代 `term_metadata_mut` 的逃逸可变引用）、`Function::{metadata, attach_metadata}`、`GlobalVariable::{metadata, attach_metadata}`、`GlobalAlias::{metadata, attach_metadata}`；创建期初始表仍走 `make_inst_with_meta_and_loc` 构造参数。
  追加语义统一（全局由"整表替换"改为逐条追加，解析期该表必为空 ⇒ 行为等价）；`unreachable` 不接受附件、未终止是坏 IR，两种失败原因由返回值分开报（解析器诊断文本未变）。跨 crate 读取面 `forge-opt` 的 inline/lto/func_specialize 三处改走 `inst.metadata()`。守卫 `tests/metadata_single_write.rs`（6 例，含文本层四载体端到端与"写入只许出现在唯一写入口实现体里"的源码断言，已用负向探针验证会失败）。

### Changed (2026-09-15)

+ **开放集合划边界：删掉按架构名/OS 名查表的宿主查询，IR 公开面字符串统一 `ImmStr`（forge-ir v3 S5 第 2 项）**：`TargetTriple::{is_32bit, is_64bit, os_name}` **删除**——它们是"把写死的常量换成合法取值集合"的典型越界：架构名写进宿主白名单（`"x86_64" | "aarch64" | …`），表外架构（`loongarch64`、用户自定 ISA 名）两个都返回 `false`（既非 32 位也非 64 位的静默错误答案），且可能与 ISA 自己声明的 `addr_width` 矛盾；实测零生产调用点（只有其自身单测），故直接删除、不留兼容层。`TargetTriple` 只留 `parse`/字段/`Display`：四个分量原样保留原样往返，无归一化表。
  边界三分与规则（写进 crate README）：闭合集合（opcode/条件码/类别/效果/终结符种类，由 `ops.toml` 等单点闭死）、目标/ISA 数据（寄存器类与宽度、栈槽、指令字宽、pattern 名、isel 标签、triple 各段）、用户程序数据（名字、`section`、metadata 自定义 kind、`Immediate::String`）；**不得从后两类字符串反推第一类或任何数值**，数值一律来自 ISA 数据（指针宽度 = `DataLayout` 的 `p:<size>:<abi>`）。
  承载统一：`Module.source_filename: Option<String>` → `Option<ImmStr>`、`module_asm: Vec<String>` → `Vec<ImmStr>`（SSO + `Arc<str>` 共享，Clone O(1)），IR 公开面不再有裸 `String` 字段。守卫 `tests/open_set_boundary.rs`（4 例：未知架构名往返、宽度只跟布局数据走、`src/` 不得重现代码表、公开面不得有 `String` 字段），**已用负向探针验证守卫会失败**。

### Changed (2026-09-15)

+ **`isel_strategy` 类型化 + 字段私有化（forge-ir v3 S5 第 1 项）**：`Instruction.isel_strategy` 从 `Option<&'static str>` 变为 `Option<IselStrategy>`（新模块 `src/isel_strategy.rs`）——**不是枚举**：标签名由目标 ISA 数据决定，forge-ir 不解析、不认识任何具体名字（无枚举/白名单/长度限制，名字内嵌的参数如 `"lea_sib:4"` 原样保留）。
  修掉两个真实缺陷：① `&'static str` 逼生产者把运行期名字 `Box::leak`（审计记录过那次泄漏修复），`IselStrategy::new` 现收任意 `&str`/`String`（短名内联零分配、长名 `Arc<str>` 共享、字面量 `from_static` 零拷贝）；② 裸字符串让"两套命名体系错配"（`lea_sib` vs `lea-merge-iadd-imul-4`）静默，类型化后比较必须先构造 `IselStrategy`，且**刻意不实现** `PartialEq<str>`/`Deref<Target = str>`/`Default`（空名 fail-closed，`None` 是"无标签"的唯一编码）。
  字段降为 `pub(crate)`，读写走 `Instruction::{isel_strategy, set_isel_strategy, clear_isel_strategy}`；5 处"保留全字段"复制点（`clone_inst`/lto/inline/func_specialize）改走写入口。**行为不变**：该通道当前无生产者（手写 `ext/pattern_isel.rs` 已随 ISA-DSL v15 删除），与 `[[pattern]]` 命名体系对接仍属 backlog #2。守卫 `tests/isel_strategy.rs`（5 例）。

### Changed (2026-09-15)

+ **终结符诊断点名真实指令句柄（forge-ir v3 S6 子项，S4 主体后续）**：S4 让终结符成为指令之后，校验器中 5 个与终结符相关的错误变体仍只报块号或带伪造句柄——`BlockParamCountMismatch`/`ReturnTypeMismatch`/`ReturnValueTypeMismatch`/`InvalidTerminatorTarget`/`TerminatorDominanceViolation` 现各带 `inst: Inst` 并在 `Display` 里打印（`block {}: terminator inst {} …`）。
  构造点全部改取真实句柄（终结符用值循环绑定 `term_inst`、块参数检查绑定前驱块的终结符指令、`switch` case 重复与非法跳转目标各取 `block_terminator(block)`），两处 `Inst(u32::MAX)` 伪造占位删除。
  守卫 `tests/verify_negative.rs::terminator_diagnostics_carry_real_inst`（`ret` 计数不符点名真实 `ret` 指令；用公开写入口 `Function::jump` 改到不存在的块后，`InvalidTerminatorTarget.inst` 等于新终结符指令且不等于被墓碑化的旧句柄）。

### Changed (2026-09-15)

+ **终结符并入指令流（forge-ir v3 S4 主体，破坏性）**：`Terminator` 枚举与 `UseSite` **删除**——终结符现在就是一条指令（新增 opcode `Ret`/`Jmp`/`Br`/`Switch`/`Unreachable`/`Invoke`/`Resume`，`category = "terminator"`），存 `DataFlowGraph::insts`，由 `BlockData.terminator: Option<Inst>` 引用，**不进 `inst_order`**（块内指令列表语义不变，约 40 处迭代点零改动）。块实参即这条指令的 operands、目标块与 `switch` case 表在它的 immediates 里（`Immediate::Block`/`Type` 早已存在，未新增 immediate 变体、未新增 arena；编码约定见 `ops.toml`「终结符」节，解码处自校验）。
  读经投影访问器（`term_kind`/`term_branch`/`term_jump`/`term_return_values`/`term_switch`（改为结构化 `SwitchView`）/`term_invoke`/`term_resume_value`/`term_is_unreachable`/`term_args_to`/`term_used_values`/`for_each_term_value`/`block_successors`），写经按形式写入口（`jump`/`branch`/`ret`/`switch`/`unreachable`/`invoke`/`resume`/`set_return_values`/`retarget_terminator`/`replace_terminator_args`）。
  **use-def 随之归一**：终结符用值就是普通操作数，`UseLists` 只剩"指令操作数"一条路径，`record_terminator`/`forget_terminator`/终结符专项校验与 `apply_replacements` 的终结符分支一并消失；`Function::replace_all_uses` 天然覆盖分支实参/`ret` 返回值。
  边界：`TargetLowering::lower_terminator` 形参由 `&Terminator` 改为 `(dfg, block)`，DSL 生成器按 `TermKind` 分派 —— **ISA TOML 与 `[[lowering]]` 规则未改动**。守卫：`terminator_is_an_inst_outside_inst_order`；`display_llvm` 往返比较改为比语义形态。

### Changed (2026-09-15)

+ **crate 内读取面也全部走终结符投影（forge-ir v3 S4 子项 f）**：`use_list` 的 `record_terminator`/`remove_terminator` 不再收 `&Terminator`（改为按块 + `&DataFlowGraph` 经 `for_each_term_value` 读取）；
  `verify` 的全部变体 match 改走投影（`block_successors`/`term_branch`/`term_jump`/`term_switch`/`term_invoke`/`term_return_values`/`term_kind`）；`display` 的 `TerminatorDisplay` 改为"持有块 + 按 `term_kind` 分派 + 投影取载荷"，打印边界不再依赖存储形态；解析器的终结符元数据校验/附着改走新增的 `DataFlowGraph::{term_metadata, term_metadata_mut}`。有意的小行为变化：非法跳转目标现在**每个**各报一条 `InvalidTerminatorTarget`（旧代码对 `Switch` 只报第一条 case）。
  目的：S4 主体（终结符并入指令流、删 `Terminator`）只剩访问器/写入口的实现体与 `Terminator` 定义本身要改。forge-ir 内 `Terminator` 引用 241 → 205（其中 60 在定义/测试、40 在解析 AST）。

### Changed (2026-09-15)

+ **读取面全部改走投影访问器，forge-opt 的终结符依赖清零（forge-ir v3 S4 子项 e）**：`crates/middle/forge-opt` 中所有 `match Terminator` 读取点（const_fold / dead_code / jump_thread / sccp / gvn_pre / tail_call / inline / lto / func_specialize / copy_prop / ind_var_simplify / block_param_coalesce / insert_preheader / loop_unroll）改为使用投影访问器（`term_branch`/`term_jump`/`term_return_values`/`term_switch`/`term_invoke`/`term_args_to`/`term_is_unreachable`/`for_each_term_value`/`block_successors`）；
  codegen 的 `ret` 读取与 invoke/resume 判别、forge-rustc 的诊断打印同步迁移。**该 crate 现已 0 处引用 `Terminator`**（`git grep -c` 实测）——为 S4 主体（终结符并入指令流、删 `Terminator`）把表示相关调用点收进访问器实现体。
  顺带修掉一处**遍历口径缺陷**：`const_fold::collect_uses` 的 `Switch` 分支漏算 `default_args`（旧手写 match 只收 discriminant + case args），改用规范序 `for_each_term_value` 后 default 实参也计入使用。

### Changed (2026-09-15)

+ **终结符写入口按形式化 + 读取面投影化（forge-ir v3 S4 子项 d）**：`Function` 新增按形式命名的写入口 `jump`/`branch`/`ret`/`switch`/`unreachable`/`invoke`/`resume` 与 `set_return_values`/`retarget_terminator`/`replace_terminator_args`；`set_terminator(Terminator)` 与 `rewrite_terminator` 收为 `pub(crate)`，**crate 外已无法构造或就地改写 `Terminator`**（实测残留写点 = 0）。DFG 新增 `TermKind`
  判别与投影访问器（`term_branch`/`term_jump`/`term_return_values`/`term_switch`/`term_invoke`/`term_resume_value`/`term_is_unreachable`/`term_args_to`/`term_used_values`/`for_each_term_value`）。
  迁移 20 处构造点（builder 7 个方法委托、forge-opt 7 文件、codegen 3 处、解析器 phi 回填、loop_unroll 的 `clone_terminator` 改为按投影读+按形式写）；顺带修掉 codegen 中"手写逐块改 `inst.operands` 却不刷新 use-lists"的又一处 use-def 置空（改用 `replace_all_uses`）。
  目的：S4 主体（终结符并入指令流、删 `Terminator`）必须一次提交内完成，本步把表示相关的调用点收进少数访问器/写入口的实现体，使那次切换只需重写实现体。守卫 `tests/terminator_api.rs` 6 例（7 种形式的写↔投影读回、`TermKind` 判别一致、use-def 新鲜）。

### Changed (2026-09-15)

+ **块级表示收口："未终止"成为显式状态（forge-ir v3 S4 子项 c）**：`BlockData` 的 `terminator: Terminator` + `has_terminator: bool` 合并为 `terminator: Option<Terminator>`——默认值 `Unreachable` 同时充当"未终止"占位与"显式 unreachable"、只能靠布尔位区分的歧义消失（按 `Default` 构造的块曾会把"漏写终结符"静默伪装成"显式 unreachable"）。
  读取口一分为二且**无静默回退**：`BlockData::terminator()` fail-closed（未终止即 panic，与 `Function::entry()` 同一契约），`BlockData::terminator_opt()` 供必须容忍坏 IR 的调用方（校验器/display/解析器元数据校验）；**CFG 构造**（`predecessors`/`successors`/支配树/循环森林）用 `terminator_opt()`——"未终止 ⇒ 无出边"是结构事实，且校验器本就要在坏 IR 上跑。
  两处兜底行为不变（`MissingTerminator` 与 `PathWithoutReturn` 仍分别上报）；新增守卫 `tests/verify_negative.rs::unterminated_block_is_explicit_state`（新块为 `None`、CFG 构造容忍不 panic、`terminator()` 读取必须 panic）。

### Changed (2026-09-15)

+ **终结符写入面收口（forge-ir v3 S4 子项 b）**：`BlockData.{terminator,has_terminator}` 降为 `pub(crate)` 并新增只读访问器，crate 外（forge-opt / forge-codegen / forge-rustc / 集成测试，22 个文件）的读取统一改走访问器——**跨 crate 直接写终结符字段已不可能**，唯一写路径是 `Function::set_terminator`；`DataFlowGraph::block_terminator_mut` 同步降为 `pub(crate)`。
  新增 `Function::rewrite_terminator(block, f)`：就地改写终结符后自动重登记 use 项。
  **顺带修掉三处真实缺陷**：`forge-codegen` 的 `ret` 值就地改写（大聚合返回值展开、段值替换、`ret` 内 RAUW）此前从不刷新 use-def——在终结符进入 use-def 之后会留下陈旧 use 项，现全部改走 `rewrite_terminator`（`insert_preheader` 的 `retarget` 亦然）。
  可达性用探针实测：该路径在 `cargo test -p forge-codegen`（21 suites）与 `cargo test -p forge-tests --lib`（含 x86/riscv JIT 矩阵）下均不可达（仅 forge-rustc e2e 覆盖），故以 `tests/use_lists.rs::rewrite_terminator_keeps_use_def_fresh` 在本地钉住 API 契约。

### Fixed (2026-09-15)

+ **终结符用值进入 use-def（forge-ir v3 S4 子项 a）**：
  `Use.user` 只能是 `Inst`，于是分支条件与 `then/else` 实参、`jump` 实参、`ret` 返回值、`switch` 判别值与 case 实参、`invoke`/`resume` 用值**完全不在 use-def 中**——`Function::replace_all_uses` 名不副实（pass 对分支实参做 RAUW 会留下悬空实参），而 `Verifier` 的 use-list 检查也看不见这一类（它同样只校验指令操作数）。
  现在 `Use { value, site: UseSite, operand_idx: u32 }`，`UseSite::{Inst(Inst), Term(Block)}`；`Terminator::for_each_value`/`for_each_value_mut` 是终结符用值的唯一遍历序（两份遍历由同一宏模板展开 ⇒ 记录序与改写序结构性一致，`used_values()` 亦由它实现）。
  `Function::set_terminator` 成为写终结符的唯一公开入口（精确摘除旧 use 项再登记新项；`DataFlowGraph::set_terminator` 降为 `pub(crate)`），新增 `refresh_terminator_uses` 供就地改写（`replace_args`/`remove_arg`）后重登记；15 处终结符写入点全部收口（builder 7、forge-opt 8），`apply_replacements` 删掉手写的 7 变体终结符 match 改用规范序遍历。
  `UseLists::verify` 改为双向（终结符用值必须在 use-lists 中；每条 `Term` 记录必须在 DFG 同一下标取到同值）⇒ 绕过 `set_terminator` 会立刻被 `PassVerify::Error` 抓到。守卫：`tests/use_lists.rs` 新增 4 例（RAUW 覆盖终结符实参、`set_terminator` 精确摘除、陈旧 use 项必报 `UseListInconsistency`）、`terminator.rs` 平坦序规格测试。

### Fixed (2026-09-14)

+ **forge-ir S0 止血（12 项，来自全仓只读审计的实证缺陷）**：
  ① 常量池浮点只有位模式不记值宽 → f32 `1.5`（`0x3FC0_0000`）与位模式相同的 f64 **去重成同一个 `ConstId`**，而 phi 打印路径恒按 `f64::from_bits` 还原 → 输出错误的十进制值；现在位宽进去重键（`insert_float_typed`/`get_float_width`/`remap_from` 带位宽）且 phi 打印按值宽还原。
  ② `ConstantPool::default()` 绕过 `new()` 的 bool 预置槽而 `bool_const` 直接构造索引 → `Default` 现在是 `new()` 的等价实现。
  ③ `total_len()/is_empty()` 漏算聚合池。
  ④ `MetadataStore::insert_at` 与 `intern` 两条写入路径互不更新（同内容双 id、去重表指向被覆盖节点）+ `get` 越界 panic → 去重表自洽、`get` 返回 `Option`、解析器侧悬空引用点名报错。
  ⑤ `Verifier` 无 `TypeContext` 时 **8 类类型检查静默跳过**、`check_gep_indices` 对坏输入 panic → 新增 `VerifyError::MissingTypeContext`（fail-closed）与越界报错。
  ⑥ `Instruction.pos` 是只写不读的死字段（删/移指令后陈旧）→ 删除，块内顺序唯一事实源 = `BlockData.inst_order`。
  ⑦ 三处与代码不符的注释（`lib.rs`/`entity.rs` 声称已有 `PrimaryMap/SecondaryMap`、`function.rs` 指向不存在的 `Terminator::map_values`）。
  ⑧ 测试辅助 `all_opcodes()` 漏 10 个 opcode 变体（"系统性覆盖测试"并不系统）→ 改用新的 `Opcode::ALL`。
  ⑨ 覆盖矩阵用 `match op { … _ => … }` 兜底，34 个 opcode 静默无覆盖 → 逐条登记 `UNCOVERED_OPS`（带原因）+ 完整性守卫。
  ⑩ forge-opt `PassResult` 的删除/新增计数在汇总时**全部丢失**（只累加 `changed`）。
  ⑪ `UntilFixedPoint` 无迭代上限（pass 的 `changed` 恒真会挂死流水线）→ `MAX_FIXED_POINT_ROUNDS = 256` + 未收敛报错。
  ⑫ pass 后 IR 校验此前只 `log::warn`（坏 IR 继续流动）→ 策略化为 `PassVerify{Off,Warn,Error}`（S0 时默认 `Warn`；同日清偿 S6 欠账后 **`Error` 已是 `#[default]`**），并**修掉一处非法 IR 测试夹具**（`o2_pipeline_nop_residue` 的 `build_loop` 建 0 参数块却传 2 个实参）。
+ **pass 后严格校验暴露的两笔欠账 → 同批清偿（v3 方案 S6 先行项，2026-09-14）**：
  `inline` 曾留下 use-list 不一致 + 返回类型不匹配（内联体建指令未登记，且**终结符操作数**没被 RAUW，`ret` 会返回 `VOID` 值 ⇒ 改用 `Function::apply_replacements`）；
  `gvn_pre`/`mem2reg` 插入指令的操作数未登记 use-lists，且 PRE 会在**不被操作数定义支配**的前驱插入（⇒ 新增 `operands_dominate` 守卫）。
  根因是 `UseLists::remove_inst` **按指令当前操作数**逐条删，改写**之后**调用就删不掉旧记录 → 新增 `UseLists::forget_inst`（按 `user == inst` 清扫）。
  现建指令一律走 `Function::make_inst*`（自动登记），就地改写走 `Function::refresh_inst_uses`；`PassVerify::Error` 成为 `#[default]`，守卫测试换成 `strict_verification_passes_for_all_pipelines`（O1/O2/O3 全绿）。详见 `docs/archive/forge-ir-v3-plan.md` §6 末。

### Changed (2026-09-14)

+ **比较条件从 `Opcode` 变体载荷归一到 immediate 通道（forge-ir v3 S1 第二步）**：
  `Opcode::Icmp { cond }` / `Opcode::Fcmp { cond }` 的载荷删除，`Opcode` 至此 109 个变体全部无载荷；条件由 `Immediate::IntCC`/`FloatCC` 承载，`ops.toml` 用 `cond = "IntCC"|"FloatCC"` 声明该契约（生成物给出 `Opcode::cond_kind()`）。
  数值表示唯一化：`IntCC::code()`（1..=10）/`FloatCC::code()`（1..=16）+`from_code()`；宿主 lowering 把它们填进 `LowerCtx.current_immediates`，ISA TOML 的 `cond` 谓词读同一个数字——**forge-dsl 此前为每个 ISA 模块各生成一张 `icmp_id`/`fcmp_id` 数字映射表，现已删除**（x86 setcc 编码映射留在 ISA 侧，输入改为 canonical code）。
  fail-closed：`Verifier` 新增 `MissingCondImmediate`/`WrongCondImmediate`（条件缺失或类型不对直接报错，不按默认条件继续）；display 对坏 IR 打印 `icmp <cond?>` 而非静默省略；forge-codegen 的 immediate 折叠表删掉 `_ => 0` 兜底臂改为显式列出全部 `Immediate` 变体。
  **归一暴露并修掉两个真实缺陷**：① `ExprKey`（CSE/GVN/GVN-PRE 表达式的键）只含 `opcode + operands + ty`——条件在 opcode 载荷里时恰好够用，归一后 `icmp eq` 与 `icmp ne` 键相同会互相消除（错值级），现在 immediate 进键（回归测试 `p0_icmp_cond_distinct` 抓到的）；② `algebraic.rs` 的 `x cmp x → 1/0` 规则读的是 `Immediate::Int(cc)`，而条件当时在变体载荷上、immediates 恒空 ⇒ 该分支从未命中（死代码），归一后真正生效。

### Changed (2026-09-14)

+ **LLVM 文本名表进 `ops.toml`；全部指令名查表改为生成 `match`（O(1)）**：
  `ir_parser/llvm_mapping.rs` 的正/反两张手写表（105+ 条 `match`）删除，改为每个 opcode 在 `ops.toml` 声明 `llvm = "<文本名>"`、`llvm_parse = false`（仅 display 用）、`llvm_alias = [...]`（旧名）。有损对从"读者自己比对两张表"变成**声明**：`Fload`/`Fstore` 复用 `load`/`store`、`Iconst`/`Fconst`/`Vconst` 常量内联、`Vadd`/`Vsub`/`Vmul`/`Vneg`/`Vabs`/`Vbitcast` 由标量名 + 向量类型精化、`Vsplit`/`Vconcat`/`Ftrunc` 解析器不接受、`CallIndirect` 复用 `call`、`Icmp`/`Fcmp` 需要条件（共 17 条 display-only）；别名 3 条（`callbr`/`ptrtoaddr`/`vextractelement`）。生成期断言变体名/助记符/**解析名集合**三者唯一。
  **查表一律 O(1)**：`from_name`/`from_mnemonic`/`from_llvm_name`/`from_cond_llvm_name`/`info()` 都是生成的 `match`（此前是 `ALL.iter().find` 线性扫 109 条）。
  本机 debug 实测（200k 轮 × 28 名 = 560 万次）：`from_llvm_name` 2770.5 → **201.2 ns**（13.8×）、`from_mnemonic` 2532.2 → **275.0 ns**（9.2×）、`from_name` 2806.0 → **509.1 ns**（5.5×）；计时工具 `forge-ir/tests/llvm_name_lookup_perf.rs` 默认 `#[ignore]` 留在仓库。
  生成器自身的构建期查表同步去掉 O(n²)/O(n·c)（改 `HashMap` O(1) 探测），**生成物 SHA256 前后完全一致**（`5A53A0F5…`）证明改写不改变行为；`semantics.rs` 的 DWARF 操作码表与 `forge-tests` 的清单腐烂检查也从线性扫描改为 `match`/`from_name`。
  **迁移保真证据**：独立脚本解析 git HEAD 两张表与生成物逐项比对 → `PARSE_NAMES=95 / OLD_PARSE_PAIRS=95 / OLD_SHOW_PAIRS=108`，**MISMATCHES=0**。

### Changed (2026-09-14)

+ **verifier 的逐指令类型规则声明化（forge-ir v3 S1 收尾）**：
  `verify.rs` 的 `check_operand_types` 原本按 opcode 手写分组（`matches!` 大名单：binop 37 个、浮点 binop 8 个、`Fma`、`Select`、`Icmp`/`Fcmp`，再加 13 个转换指令的逐 opcode 分支）。
  现在**族名声明在 `ops.toml` 的 `type_rule`**，verifier 按族分派：12 个封闭族（`none`(48)/`binop_same`(37)/`convert`(13)/`load`(2)/`store`(2)/`same3`/`cmp_int`/`cmp_float`/`select`/`cmpxchg_pair`/`call`/`call_indirect`）。
  写成未实现的族名 → **构建期报错**；新增族名 → `verify.rs` 的穷举 match（无 `_` 臂）**编译失败**（双向 fail-closed）。
  13 条转换分支收敛成一张事实表 `convert = { src, dst, width }`（`int/float/ptr/any` × `widen/narrow/any/equal_bytes/equal_total_bits`），诊断文本由事实表拼出；形状类规则抽到新模块 `src/type_rules.rs` 的 `check_shape`（verifier 与 builder 共用一份实现，自带 4 个单测）。
  守卫测试 `type_rule_classification_is_complete` 断言每个 opcode 都有分类且各族计数钉住。
  **builder 侧不做声明化（实证否决）**：把同一函数挂到 `FunctionBuilder::emit_with_mem` 后 **5 个既有 builder 测试失败**——builder 刻意允许"混合宽度操作数 + 结果类型 upcast"（`iadd(i8, i64) → i64`），而 verifier 的 `BinopSame` 要求两操作数同类型；builder 是宽松构造层（类别维度由方法内 `assert!(t.is_int())` 把关，比 verifier 更严），verifier 是严格校验层，强行统一会破坏既有语义，故保留为可选工具函数并写明原因。

### Changed (2026-09-14)

+ **`predecessors()`/`successors()` 迁到密集索引（forge-ir v3 方案 S2 第二切片）**：
  `Function` 的两处 `OnceLock<HashMap<Block, Vec<Block>>>` 改为 `OnceLock<SecondaryMap<Block, Vec<Block>>>`（`preds.entry(succ).or_default()` → `get_mut_or_default`）。
  这是**跨 crate 公开 API**：`forge-opt` 的 6 个 pass 文件（`gvn_pre`/`loop_unroll`/`insert_preheader`/`ind_var_simplify`/`block_param_coalesce`/`jump_thread`）、`forge-codegen` 的 lowering，以及 forge-ir 内的 `display.rs`/`function.rs`/`semantics.rs`/`verify.rs`/`loop_info.rs` 共 11 处调用点由 `.get(&block)` 改为 `.get(block)`。
  语义差异写进 doc：`predecessors()` 里**无前驱的块不出现**（entry），`successors()` **每个块都有条目**（无后继者空 vec）——与迁移前逐块 `insert` 行为一致。
  计量（同一 `git grep` 口径，`forge-ir/src` 全树）：S2 之前 **45 处** → 第一切片后 32 处 → 本切片后 **25 处**。门禁：clippy `-D warnings` 干净、workspace 1388 passed / 0 failed / 19 ignored（67 suites）、x86 矩阵 195/3/0、riscv64 131/67/0。

+ **支配树字段密集化（forge-ir v3 方案 S2 第三切片）**：
  `analysis.rs` 的 `DominatorTree` 五个字段（`children`/`tin`/`tout`/`idom`/`depth`）由 `HashMap<Block, _>` 改为 `SecondaryMap`（含 `empty()`、C-H-K 迭代的 `idom` 局部表、`compute_children`/`compute_intervals` 的签名与返回类型）。
  支配树是 `dominates`/`idom`/`depth`/`ncd`/`children` 的底座（`loop_info`/`licm`/`gvn` 都在用），查询从"哈希 + 探测"变为一次 `Vec` 索引（`dominates` 一次查 4 张表）。
  计量（同一 `git grep` 口径，`forge-ir/src` 全树）：句柄键 `HashMap` S2 前 45 → 第二切片后 25 → **本切片后 10 处**（`SecondaryMap` 使用点 74 处）。
  顺带记录一个待补的易用性缺口：`for (child, &parent) in &secondary_map` 需要 `IntoIterator for &SecondaryMap`（暂未提供，本次改用 `iter()`）；门禁：clippy `-D warnings` 干净、workspace 1388 passed / 0 failed / 19 ignored（67 suites）、x86 矩阵 195/3/0、riscv64 131/67/0。

+ **删除 `TypeId::bits()`/`try_bits()`，位宽改问类型事实 API（forge-ir v3 方案 S3 第二切片）**：
  旧视图有两条撒谎的默认值——`PTR` 恒 64（不查 `DataLayout`）、复合类型返回 0（与 void 不可区分）。
  按"无需兼容旧版本结构"直接删除，迁移 **76 处调用点**：`forge-ir`（verify 转换宽度规则、builder 的 `iconst` 常量位宽、entity 测试）、`forge-opt`（`const_fold` 19 处 + `algebraic`）、
  `forge-dsl`（5 处 **生成代码** —— 新增 `LowerCtx::type_bits_of`）、`forge-codegen`（`opsize_from_type`/`mem_opsize_from_type` 改实例方法并问 store、`reg_info` VEC 档位、`pattern.rs`/`compiler.rs`）。
  新增 `TypeStore::scalar_bits`（指针按 DataLayout）、`TypeContext::scalar_bits`、`TypeId::builtin_scalar_bits`/`builtin_vector_bits`。
  **两处行为修正**：32 位目标的指针 opsize 从 64 修正为 32；`const_fold` 对动态位宽标量（`i24` 等）改为不折叠（fail-closed 守卫，本模块无 store）。
  另：`TypeContext::borrow/borrow_mut` 锁中毒不再 panic（取回内部值），新增守卫 `tests/type_facts.rs`。

  实测：残留 `bits(`/`try_bits` 调用 0；workspace 1383 passed / 0 failed / 19 ignored（68 suites）；x86 矩阵 195/3/0；riscv64 131/67/0。

+ **S4 前置清理：入口约定 fail-closed + 尾声哨兵具名**：
  ① 7 处 pass/分析（`analysis.rs`、`dead_code`/`gvn`/`gvn_pre`×2/`jump_thread`/`sccp`）与 `forge-codegen/pipeline/compiler.rs` 的 entry 参数重建此前写 `entry_block.unwrap_or(Block(0))` / "entry 块约定为索引 0"——静默回退会把"没设入口"伪装成"入口是 0 号块"，支配树/循环分析会据此算出看似合理但错误的结果；现在统一走新增的 `Function::entry()`（缺失即 panic，fail-closed），要"可能没有入口"语义的调用方直接读 `entry_block` 字段。
  ② 统一尾声标签此前是字面量 `Block(0xFFFFFFFD)`（`emission.rs` 两处 + reloc patcher 注释里的魔数）→ 具名为 `pipeline::emit::EPILOGUE_LABEL` 并写明"为什么是这个值、为什么不能改（定宽 ISA 把块号写进 label 位域，reloc patcher 依赖其只占低位）"，绑定前加 debug 断言（块数不得逼近哨兵）。

  实测：workspace 1383 passed / 0 failed / 19 ignored（68 suites）；x86 矩阵 195/3/0；riscv64 131/67/0；clippy `-D warnings` 干净。

+ **`LabelRef` 取代机器层哨兵 `Block`（v3 方案 S4 子项）**：
  机器层 label 复用 IR 的 `Block` 句柄，但"统一尾声"不是 IR 块——此前用魔数 `Block(0xFFFFFFFD)` 表示（上一提交具名为 `EPILOGUE_LABEL`）。
  本轮引入 `pipeline::emit::LabelRef { Block(Block), External(ExternalLabel) }` 与 `ExternalLabel::{BASE, id()}`，把"真实块"与"机器层自造标签"写进类型；
  `LabelRef::id()`/`from_id()` 是**唯一的数字 ↔ 标签互转边界**（定宽 ISA 把 id 塞进 label 位域、变长走 reloc，编码器/patcher 仍按数字工作）。
  `CodeSink::{bind_label, use_label_at}` 收 `impl Into<LabelRef>`（块标签调用点零改动），`TargetFrameLowering::emit_epilogue_jump` 形参改为 `LabelRef`，
  DSL 生成器同步（`epilogue_block.id() as i64`、生成的 machine.rs 用 `LabelRef::from_id(rel as u32)` 还原）。
  编码 id 不变（尾声仍 `0xFFFF_FFFD`），`reloc_patcher` 位段重排语义与测试不受影响。

  实测：workspace 1383 passed / 0 failed / 19 ignored（68 suites）；x86 矩阵 195/3/0；riscv64 131/67/0；clippy `-D warnings` 干净。

### Added (2026-09-14)

+ **实体容器与密集索引（forge-ir v3 方案 S2 第一切片）**：新模块 `src/entity_map.rs`（无新依赖，8 个单测）提供 `PrimaryMap`（`push` 分配句柄、下标即句柄、**刻意不支持删除**）、`SecondaryMap`（`Vec<Option<_>>`，"未设置"与"空值"可区分，`get_mut_or_default` 等价 `entry().or_default()`）、`EntitySet`（密集位图，O(1) 增删查）、`PackedOption`（句柄可空压缩：`Option<Value>` 8 字节 → **4 字节**，`u32::MAX` 为空哨兵），以及 `EntityRef` trait + `entity_ref_impls!` 宏（已为 10 个句柄类型实现：句柄 ↔ 密集下标）。`ListPool` 明确不做（本仓库列表用途都是短生命周期局部量）。
  同批把 forge-ir 内部的**句柄键表**迁到密集索引（`HashMap` → `SecondaryMap`）：`use_list.rs` 的 `uses`（最热路径：每建/删/改指令都碰）、`verify.rs` 的 `defined`（每次 `verify()` 重建）、`function.rs` 的 `value_names`/`block_names`、`display.rs` 的 `NameResolver`（`imm_str.md` 点名的热路径）、`alias.rs` 的 `memo`、`debug_info.rs` 的 `locations`、`loop_info.rs` 的 `depths`。
  **计量**：`forge-ir/src` 的句柄键 `HashMap` **45 → 31 处**（`git grep` 对比 HEAD；全仓基线 129 处 / 36 文件）；workspace 测试 1380 → **1388 passed**。
  S2 余项记在方案 §6：`predecessors()`/`successors()` 与支配树（公开 API，牵动 forge-opt/forge-codegen 15+ 调用点）、句柄字段私有化（`.0` 约 260 处）、墓碑语义显式化、两个下游 crate 内部的句柄键表。

### Changed (2026-09-14)

+ **`ops.toml`：指令元数据单一事实源（forge-ir v3 方案 S1 第一步）**：
  crate 根新增 `crates/foundation/forge-ir/ops.toml`（109 条 `[[op]]`：变体名、助记符、分组、文档、值操作数个数、结果数、`may_ub`、`side_effect`、变体载荷），
  `build.rs` 扩容为"lalrpop + 读 `ops.toml` 生成 `$OUT_DIR/opcode_gen.rs`"，`src/opcode.rs` 以 `include!` 接入——**新增一个 opcode 从改 6 张手写表变成加一行 `[[op]]`**
  （枚举、`ALL`、`name()`、`mnemonic()`、`result_count()`、`expected_operand_count()`、`may_ub()`、`has_side_effect()` 全部由生成物投影；`Opcode::info()` 是无 `_` 兜底臂的生成 match，变体与表同源不可能漂移）。
  操作数元数建模为 `OperandArity::{Fixed(u8), Variadic}`，修掉老表"`0` 既表示无操作数又表示不检查"的语义混淆（`expected_operand_count()` 保持历史契约 `Variadic => 0`）。
  生成器 **fail-closed**：缺字段/类型不对/名字或助记符重复/未知载荷/载荷缺默认值一律构建期 `panic!`。
  **迁移保真证据**：一次性脚本把 git HEAD 的手写表与生成物**各自独立解析**后逐项比对 → `109 变体 × 6 属性，MISMATCHES=0`（`NEW_VARIADIC=6 / NEW_SIDE_EFFECT=9 / NEW_MAY_UB=15`）。门禁：workspace 1372 passed / 0 failed / 18 ignored（66 suites）、clippy `-D warnings` 干净。S1 余项（`Icmp`/`Fcmp` 载荷归一、LLVM 文本名表、verifier 规则与 builder 断言声明化）记在方案 §6 末。

+ **`Function::make_inst` / `make_inst_with_meta_and_loc` / `refresh_inst_uses`（use-list 契约的公开入口）**：建指令即登记 use-lists；`refresh_inst_uses` 在就地改写操作数后重登记（对调用顺序不敏感）。配套 `UseLists::forget_inst(inst)`。回归守卫 `crates/foundation/forge-ir/tests/use_lists.rs`（3 个用例：改写后刷新一致、`forget_inst` 全删、`make_inst` 自动登记）。
+ **codegen 侧 use-list 门禁**：`pipeline/compiler.rs` 的 47 处 `.dfg.make_inst*` 改走包装、13 处墓碑/Copy 块与 3 处操作数改写点改 `refresh_inst_uses`，聚合展开后加 **debug-only** 断言 `use_lists.verify(&dfg)`。此处**刻意只查 use-lists 而非完整 `Verifier`**：`expand_geps` 会生成类型自洽性不足的 IR（`%p = add i64 %prev, %t` 却声明 PTR 结果，源码原注释即"verify 不跑"），根因是 v12 尚无 `Ptrtoint`/`Inttoptr` 降级 ⇒ 类型化指针算术归 v3 的 S4/S5（范围写在注释里，不是静默容忍）。

+ **`Opcode::ALL` / `Opcode::name()` / `from_name()` / `from_mnemonic()`（指令清单单一事实源的第一步）**：`Opcode::ALL` 是全部 **109** 个变体的权威清单；`name()` 是**无 `_` 兜底臂的穷举 match**（新增变体不同步更新即编译失败）。新增守卫 `crates/foundation/forge-ir/tests/opcode_table.rs`：清单与枚举等势、变体名唯一可逆、助记符唯一可逆、条件变体（`Icmp`/`Fcmp`）按变体身份。ISA TOML 的 `op = "Iadd"` / `pattern.match` 名字契约从此可机器校验。
+ **`crates/foundation/forge-ir/README.md`**（此前该 crate 无 README）：结构表、`FunctionBuilder`/`TypeContext`/原子修改原语/`Opcode::ALL` 的使用要点与已知欠账入口。
+ **`docs/archive/forge-ir-v3-plan.md`**：forge-ir v3 改进方案（诊断、证据、设计原则、子系统方案、S0–S8 分期与门禁、外部参考、非目标）与 S0 落地记录。

### Fixed (2026-09-13)

+ **剩余的 x86 形态写死（残余 R8–R10）**：sret / 宽向量 by-ref / 栈参数三条路径的生成代码里仍有字面量：`Reg::RBP`（6 处）、`Reg::RSP`（1 处）、by-ref/sret 的 64 字节向量槽步长、`max(72)` 帧需求、`-8` sret 槽；`[abi].stack_align` 缺省还是 x86 的 16。现在：基址寄存器从 `[abi.frame].fp/.sp` 派生（未声明时回退主 GPR 组 0 号占位，而这些路径只在声明了对应角色的 ISA 上生成）；向量槽步长由 `[meta].vector_tiers` 最大档派生、帧需求 = 槽步长 + `[meta].slot_bytes`、sret 槽 = `slot_bytes`、栈参数偏移按 `slot_bytes`；`stack_align` 缺省 = `slot_bytes`。x86 生成物对这几处逐 token 等价（64/72/8 均由元数据算出同值），行为由 x86 矩阵 195/3/0 与 riscv64 矩阵 131/67/0 守住。
  （其中 `[abi].stack_align`、`[meta].slot_bytes` 等键随后归入 `[stack]`/`[abi.stack_args]`，见下条 B3。）

### Changed (2026-09-13)

+ **指令字宽 = ISA 数据，且无白名单/上限（接口通用化 B6）**：
  定宽 ISA 此前被写死为"32 位字、4 字节读字"（`codegen/mod.rs` 的 `!= Some(32)` 门槛、`u32::from_le_bytes` 读字、`machine.rs` 的 `RelocKind::Relative(4, 0)`）。
  现在 `[meta].default_inst_width` 可写**任意 ≥ 1 位**，生成代码把指令字表示为**字节数组**（`[u8; ceil(位/8)]`，LE 位序）+ `__place`/`__bits` 助手：位域可跨字节、非字节对齐、落在机器字之外（100 位字夹具的 op 在 bit 92..100）。
  同步去限制的还有：位域的**字侧偏移无上限**（只保留"单个位域 ≤ 64 位"这一**值表示**上限——位域值承载在 u64 常量键与 i64 操作数上，越界明确报错）、定宽 label/global fixup 宽度由字长派生、`RelocKind::{Absolute,Relative}` 宽度 `u8 → u32` 且通用写入路径按宽度**符号/零扩展**补位（不再只认 1/4/8，也不再对其它宽度静默不补或 panic）。
  过程中修掉一个被新夹具暴露的生成器缺陷：**只有寄存器操作数的 ISA**（无 imm/label）生成出引用未定义 helper（`__expr`/`__set_label_operand`）的模块——两者现在无条件生成（未用入口加 `#[allow(dead_code)]`）。
  新夹具：`tests/isa/demo_inst8.toml`（8 位字 + 字内 label 域，注册 2 位域 `RelocPatcher`）、`demo_inst12.toml`（12 位字，非 8 倍数 + 填充位必须为 0）、`demo_inst100.toml`（100 位字 = 13 字节，位域在 bit 92..100）；用例在 `tests/demo_inst{8,12,100}_v12_tests.rs`。
  验证（2026-09-13 本机）：workspace **1355 passed / 0 failed**（63 suites）、x86 jit 矩阵 **195/3/0**、riscv64（QEMU 真执行）**131/67/0**、fmt/clippy `-D warnings` 干净。
  32 位定宽 ISA（riscv64/arm64/demo）的生成物**代码形态**因此改变（不再用 `u32::from_le_bytes`/`u64 __w`），行为由上述测试与 riscv QEMU 真执行守住；x86（变长路径）生成物逐字节不变。

+ **栈/传参键归类 `[stack]` 与 `[abi.stack_args]`（接口通用化 B3）**：
  栈槽单位、栈对齐、帧指针保存宽度此前散在 `[meta].slot_bytes`/`[meta].fp_overhead_bytes`/`[abi].stack_align`；
  Windows x64 的栈参数布局（基址寄存器、首个栈参槽位、槽步长、shadow space）散在 `[abi].stack_arg_shadow` 与生成器字面量（2/1/32、`Reg::RBP`/`Reg::RSP`）里。
  现在收敛为两个新表：`[stack] { slot, align, fp_save }`（缺省 = `addr_width` / `slot` / `addr_width`）与
  `[abi.stack_args] { callee_base, caller_base, first_offset_slots, stride_slots, shadow_bytes }`（x86 缺省 = `fp` / `sp` / 2 / 1）。
  `validate` 增加值域与枚举校验（基址只能 `fp`/`sp`、槽数与步长 > 0、shadow 为栈槽单位的正整数倍），frame/lowering 一律从新键取值。
  **行为不变证据**：`FGE_DEBUG_GEN=1` 的 5 个 ISA（x86/riscv64/arm64 + 两个夹具）dump 与重构前**逐字节一致**
  （`stride_slots == 1` 时不发射 `* 1`、字面量不带 `u32` 后缀）；workspace 1340 passed / 0 failed，x86 矩阵 195/3/0，riscv64 矩阵 131/67/0。
  规范同步 `docs/reference/isa-dsl.md`（`[meta]` 示例、键表、`[abi]` 示例与 `[abi.stack_args]` 说明）与 `CLAUDE.md`（宽度元数据条目）。

+ **`[types]` 类型→类映射（接口通用化 B2）**：
  值池门与 lowering 的"类型 → 寄存器类"推导现在可被 ISA 覆盖——软浮点（`f64 = "gpr8"`）、1 字节地址（`ptr = "gpr1"`）、
  显式拒绝（`i64 = "unsupported"`）都成为 TOML 数据（新增 `TargetRegInfo::type_map()` / 生成的 `__TYPE_MAP` /
  `LowerCtx.type_map`，`class_for_type` 与 `reg_class_for` 读同一份数据）。
  校验：类型名白名单、目标必须已声明、类宽 ≥ 类型字节宽（`ptr` 按 `[meta].addr_width`，否则报"会静默截断"）、`void` 只能 unsupported。
  夹具演示：`tests/isa/demo8.toml`（`ptr = "gpr1"`）、`tests/isa/demo.toml`（`f32/f64 = "gpr8"` 软浮点）；
  新增 4 个 DSL 单测 + 3 个宿主/夹具断言。实测：workspace 1340 passed / 0 failed，x86 矩阵 195/3/0，riscv64 矩阵 131/67/0。

+ **分配器类表改为 ISA 声明（接口通用化 B1，关闭审计遗留 R1）**：
  宿主的寄存器类表此前是"编译期编造"——`run_regalloc` 用硬编码清单（GPR 1/2/4、FPR 4/8/16、值池/地址类、tier）
  给**每个 ISA** 造类并让未声明类继承同族最宽类的池；demo8 这种只声明 `[reg.gpr1]` 的 ISA 也会得到
  GPR(2)/GPR(4)/FPR(8)/VEC(16…) 等"可分配但不可编码"的类。
  现在：DSL 生成 `TargetRegInfo::register_classes()`（**类表唯一来源**）——GPR 用类型系统整数宽度 {1,2,4,8}（≤ 主 GPR 宽）
  ∪ 已声明组宽 ∪ 地址/值池；FPR 用已声明组宽 ∪ **有效的宿主浮点值池类**（`value_fpr_class()` 缺省 FPR(8)）；
  VEC 用 `[meta].vector_tiers`；池分别取主 GPR / 浮点文件的分配序。宿主删除 `fallback_classes` 编造循环。
  新增守卫：x86 类表断言（含 GPR(1..8)/FPR(8,16,32)/VEC(16,32,64)，不得含未声明的 FPR(4)）、demo8 类表断言（恰好 `[GPR(1)]`）。
  **过程中 riscv64 QEMU 矩阵抓到真实回归**（首版未登记"有效浮点值池类"→ riscv 的 7 个 fcmp 错值），已修；
  修后 x86 195/3/0、riscv64 131/67/0。

+ **demo/示例 ISA 迁出库本体，`isa_from_file!` 支持宿主 crate 路径**：ISA-DSL 的示例谱（`demo`、`demo8`）此前是 `forge-codegen` 的 `src/arch/` 模块 + 仓库根 `isa/` 谱，与 x86_64/arm64/riscv64 这些**真实后端**并列，容易误读为"发行 ISA"。现在：
  + `isa_from_file!` 新增可选第二参数 `krate = <路径>`：生成物里的 `crate::…` 改写为 `<路径>::…`、`forge_ir::…` 改写为 `<路径>::ir::…`（新增 `forge_codegen::ir` re-export），因此生成代码只依赖宿主的公开面；**缺省参数生成物逐字节不变**（已用 5 个 ISA 的 `FGE_DEBUG_GEN` dump 逐字节比对）。
  + `isa_from_file!` 参数解析与路径改写有单测（`crate` 改写只作用于路径位置，`pub(crate)` 可见性标记与字符串字面量不受影响）。
  + 夹具谱移到 `crates/backend/forge-codegen/tests/isa/{demo,demo8}.toml`（附 `README.md`），由 `tests/common/mod.rs` 用 `krate = forge_codegen` 宿住；删除 `src/arch/demo.rs`、`src/arch/demo8.rs` 与 `src/lib.rs` 的 re-export；6 个测试文件改经 `common::demo*`。
  + 新增库表面守卫 `tests/library_surface.rs`：`src/**` 不得引用 demo 谱、`src/arch/mod.rs` 只登记真实后端、仓库根 `isa/` 只剩 3 个发行谱、夹具谱必须在 `tests/isa/`。
  + 生成代码运行面所需的公开项补登：`pub use forge_ir::IrError`（此前是私有 `use`，是"生成物只能活在库内部"的最后一处硬依赖）、`pub use forge_ir as ir`；`impl_erased_target_machine!` 宏体改 `$crate::ir::…`（不再要求调用方有裸 `forge_ir` 在作用域）。
  + 文档：`CLAUDE.md`（Key Architecture Rules 第 1 条、ISA Backend Pattern、Code Conventions、Testing Notes）与 `docs/reference/isa-dsl.md`（快速开始、新增「生成代码依赖的运行面」、`已有 ISA 谱` 拆成发行后端/测试夹具）同步。

### Added (2026-09-12)

+ **宽度元数据（去「寄存器类型/宽度写死」，DSL + 宿主）**：DSL 语法层早已支持任意寄存器宽度（`RegClass` payload = 字节），但生成期与宿主流水线把主 GPR 组锚定在 `GPR(8).or(GPR(4))`、地址类/值池/栈槽单位/帧开销内置 x86 的 8 字节缺省——只声明 1 字节寄存器组的 ISA 会**生成成功但语义错误**（名字表为空 ⇒ `sp`/`fp`/`scratch`/`callee_saved`/物理 clobber 静默丢弃，或落回 `from_index(0, GPR64)` 构造该 ISA 根本不存在的类）。现在：
  + `[meta]` 新增可选键（单位字节，优先级 **显式键 > 派生 > 报错**）：`default_gpr_width`（缺省 = 最宽已声明 GPR 组）、`default_fpr_width`（`fpr16` 基准优先）、`addr_width`、`value_gpr_width`、`value_fpr_width`（缺省 8 = f64 值池）、`slot_bytes`、`fp_overhead_bytes`、`vector_tiers`（缺省 `[16,32,64]`）。显式键必须指向已声明组，否则 `validate` 报错。
  + 生成模块新增常量 `__ADDR_CLASS`/`__VALUE_GPR_CLASS`/`__VALUE_FPR_CLASS`/`__SLOT_BYTES`/`__FP_OVERHEAD_BYTES`/`__VECTOR_TIERS`；宿主 `TargetRegInfo` 新增 `addr_class`/`value_gpr_class`/`value_fpr_class`/`slot_bytes`/`vector_tiers`/`class_for_type`（全部带**等于历史值**的缺省实现）。
  + 生成期去写死：主 GPR/FPR 组派生、`name_to_idx` 空表兜底删除、sp/fp 不再回退 `from_index(0, GPR64)`、`frame_pointer_overhead()` 由常量 8 改元数据、组别名兜底 `"RAX"` 删除、MemRef base/index 用地址类、spill 基址不再回退字面量 `"RBP"`、FPR spill 宽度档改为**已声明模板键**派生、by-value 向量阈值用 `[abi.arg_class].limit`、栈槽对齐/槽深/by-ref 向量槽用 `__SLOT_BYTES`。
  + 宿主去写死：`LowerCtx` 新增 `value_gpr_class`/`value_fpr_class`/`addr_class`/`slot_bytes`/`vector_tiers`（`CompileState::new` 注入）、值 XReg/零值/临时 vreg/phi-copy 类、spill scratch 类、类表 fallback 清单、`reg_class_for` 向量档位全部元数据化。
  + **fail-closed**：`sp`/`fp`/`scratch`/`reserved`/`callee_saved`/`ret_regs`/`call_ret_reg`/`call_clobbers`/`arg_class.regs`/`implicit_regs`/`[spill.*].base` 名字必须解析到已声明组（生成期报错）；`[abi].stack_align`/`stack_arg_shadow` 的"8 的倍数"校验改为按栈槽单位（这两个键 2026-09-13 归入 `[stack].align` 与 `[abi.stack_args].shadow_bytes`）；函数内值类型必须被 `class_for_type` 承载，否则**编译期** `Unsupported`（点名类型与值池宽度）。
  + 新夹具 **`isa/demo8.toml`**（1 字节寄存器 ISA：唯一 `[reg.gpr1]` 组，`addr_width`/`slot_bytes`/`value_gpr_width`/`fp_overhead_bytes` = 1，`default_opsize = 8`）+ `crates/backend/forge-codegen/tests/demo8_tests.rs`：断言元数据派生（`GPR(1)`、1 字节槽、sp/fp/scratch 名字解析成功、`allocatable = A0..A3`）、汇编→编码→解码→反汇编往返、宿主编译 i8 函数（20 字节机器码反汇编回 `mov A3, A0`/`add A1, A3, A2`/`ret`）与 i64 的编译期拒绝。
  + 反回潮守卫：`crates/frontend/forge-dsl/tests/no_hardcoded_widths.rs` 与 `crates/backend/forge-codegen/tests/no_hardcoded_widths.rs`（白名单带理由且条目必须被命中）。
  + 行为不变证据：`cargo test -p forge-dsl --lib` 112 passed；`cargo test -p forge-codegen --all-features` 266 passed / 0 failed；x86 jit matrix `pass=195 skip=3 fail=0`（`FORGE_JIT_EVENTS` 事件核对）。规范见 `docs/reference/isa-dsl.md` 的「宽度元数据」节。
  + **独立审计后的加固（同日）**：
    ①值池门从"只比类宽"改为"**池宽 + 寄存器文件存在性**"——无 `[reg.fpr*]` 的 ISA 上 `f32/f64/v64/v128/v256` 一律编译期拒绝
    （此前会放行一个该 ISA 不存在的 `FPR(8)`/`VEC(16)` 类，失败推迟到 regalloc）；
    ②`@push_callee`/栈参数收参与帧开销的**槽步长**从写死 `8`/`16` 改为 `__SLOT_BYTES`/地址类宽度；
    ③`[abi].stack_arg_shadow` 路径的内存基址从字面量 `Reg::RBP` 改为 `[abi.frame].fp` 派生（fp 缺失即生成期报错）；
    ④`TargetRegInfo::frame_pointer_overhead` 的 trait 缺省由常量 `8` 改为地址类宽度；
    ⑤`check_value_pools` 跳过被 DCE 墓碑化的 `TypeId::VOID` 值（否则 1 字节值池 ISA 上任何含死值的函数都会被误拒；新增 O1 回归测试）；
    ⑥demo8 测试补 `F32/F64/V64/V128/V256 → None` 断言、机器码反汇编的寄存器名/收参/返回回写断言，错误信息断言收紧到本门文案；
    ⑦两个反回潮守卫的 `FORBIDDEN` 扩到全部类字面量变体，并在文档里写明"裸数字宽度不在守卫范围"这一已知边界。

+ **V256/V512 向量 IR 层 Load/Store（ISA 规则 + 编译入口能力门）**：`isa/x86.toml` 的 `Load`/`Store` 规则原先只覆盖
  `rd_vec`/`rs1_vec` = 8/16（V64/V128），>16B 由编译入口 **fail-closed 拒绝**（"ISA 类模型缺 YMM 槽类"）。现有：
  + 规则补齐 32/64 两档 → `VMOVUPS_256_R_MEM`/`VMOVUPS_256_MEM_R`（VEX.256，`vex_l=1`）、`VMOVUPS_512_R_MEM`/`VMOVUPS_512_MEM_R`（EVEX.512，`evex_l=2`）；
  + 编译入口的字节门改为：32B（V256）放行（与既有 V256 算术路径一致）、**>32B（V512/EVEX）需 AVX-512F**（与宽向量 ABI 守卫同一判据）、
    其余非 32/64 的 >16B 宽度仍显式拒绝——四条路径都不产出静默错码；
  + 新增 **reg 基址**内存形式（`modrm = { rm = "[reg]" }`，仅基址 `[base]`、disp 恒 0）：IR Load/Store 的地址是 lowering 的**寄存器操作数**，
    而 lowering 模板只能绑定寄存器、无法现场构造 MemRef（原有的 `vs_memref` 形式继续供 ABI by-ref 路径的 `[RSP+off]`）。
  + 验证：生成级 `test_v256_slot_load_store_is_lowered`（VEX `C4 .. 7C 10/11`，且不得退回 `movsd`）、
    `test_v512_slot_load_store_requires_avx512`（无能力必须编译期拒绝 + `FORGE_ASSUME_AVX512` 下 EVEX `62 .. 10/11`，P2 L'L=10）、
    runtime `test_jit_v256_slot_roundtrip`（真执行 VEX.256 槽往返 lane7 = 16.5 → 16）、
    `test_jit_wide_vec_reg_base_mem_roundtrip_bytes`（asm→encode→decode→encode 字节往返 + objdump 实证
    `c4 e1 7c 10 00` = `vmovups ymm0, YMMWORD PTR [rax]`、`62 f1 7c 48 10 00` = `vmovups zmm0, ZMMWORD PTR [rax]`）。

+ **V512 向量常量 `Vconst rd=512`（WA-43）**：ISA 模型的 `Vconst` 规则原只覆盖 `rd = 64/128/256`，64 字节常量落到「no matching rule」→
  `Unsupported`。该缺口只在**有 AVX-512F 的机器**暴露（运行级 `test_jit_v512_byref_param` 无 AVX-512F 时提前 return，
  本机即如此）⇒ 历史上按 CI runner 分配偶发红（`test result: FAILED. 118 passed; 1 failed`）。
  + 实现：新增 EVEX 指令 `VINSERTF32X4`（`EVEX.512.66.0F3A.W0 18 /r ib`）、常量池占位符
    `{vconst_lo_h2}` / `{vconst_hi_h2}` / `{vconst_lo_h3}` / `{vconst_hi_h3}`（`__vconst_half` 本已按 half 索引泛化），
    以及两条 `Vconst` 规则（32 位 lane：F32/I32 用 `PUNPCKLDQ`；64 位 lane：F64/I64 用 `PUNPCKLQDQ`）：
    4 个 128 位段各自装好后按 imm=0/1/2/3 插入；4 条插入覆盖全部 lane（`{out}` 自身当累加器，**初始值不影响结果**）。
  + 验证：新增**生成级**测试 `test_v512_vconst_generates_four_evex_inserts`（无需 AVX-512 硬件——无宽向量参数/返回，
    不触发宽向量 ABI 的 AVX-512 门控；断言恰有 4 条 `62 … 18 /r ib` 且 imm = {0,1,2,3}）；
    `objdump -D -b binary -m i386:x86-64` 解码实测为 `vinsertf32x4 zmm13, zmm13, xmm15, 0x0/1/2/3`，
    且 8 条 `movabs` 常量逐 lane 与源码 f32 位型一致（`0x404000003fc00000` … `0x4180000041600000`）；
    运行级 lane15=16 断言仍由 `test_jit_v512_byref_param` 在有 AVX-512F 的 runner 上守护。

### Fixed (2026-09-12)

+ **V512（64B）向量按 lane 提取取错 lane（WA-47，CI run #51/#57/#61 的真实根因）**：`Vextract` 规则集里
  V256 有 `rs1_width = 256` 专档，而 **V512（`rs1_width = 512`）没有任何规则** ⇒ 落到 128 位通用回退
  `PSHUFD {f1}, {0}, {imm0}`；而 `PSHUFD` 的 imm8 **只用低 2 位**选 dword ⇒ lane L 实际取到 **lane(L%4)**
  （lane0..3 恰好正确、lane4..15 全错）。症状：`Test (Windows)` 上 `test_jit_v512_byref_param` 断言
  lane15 = 16 实测得 **4 = lane3**——该用例只在**有 AVX-512F 的 runner** 上真跑，因此长期伪装成
  "偶发/机型相关"（3 红 / 11 次 run）。
  + 修法：①新增 EVEX 指令 `VEXTRACTF32X4`（`EVEX.512.66.0F3A.W0 19 /r ib`，dest 在 r/m、src 在 reg）；
    ②新增 6 条 V512 `Vextract` 规则（`vary` 压缩）：先用 `VEXTRACTF32X4` 取 `seg = lane/4` 的 128 位段，
    再段内 `PSHUFD` 取 dword（f32/i32 用 `(lane%4)*0x55`；f64/i64 偶 lane 免 shuffle、奇 lane `PSHUFD 78`）；
    lane0 由既有 `priority = 1` 快路径覆盖。
  + 验证：生成级守卫断言 lane15 必须含 `EVEX 62 … 19 … imm=3` + `PSHUFD 0xFF` 且不得出现回退形态
    `PSHUFD …, 15`（objdump 实证 `vextractf32x4 xmm14, zmm15, 0x3` + `pshufd xmm14, xmm14, 0xff`）；
    workspace tests 0 failed；e2e 8/8 + `stage_a passed=105/105 known=[]`；clippy `-D warnings`/fmt 干净。

+ **向量溢出（spill）宽度静默截断（WA-46）**：`isa/x86.toml` 的
  `[spill.FPR]` 只有一份 **8 字节 `MOVSD`** 模板，而生成的 `emit_spill_load/store` **忽略 `width` 参数**；
  同时 `reg_class_for` 把 **64 字节向量也归入 `VEC(32)`**（该类的 `reg_width = 32`）⇒ 任何 FPR 类溢出只搬低
  8 字节、V512 的 spill 槽只有 32 字节，**未被搬运的高半区是栈残留**。
  症状：`Test (Windows)` 上 `test_jit_v512_byref_param` **偶发** `lane15 != 16`（`runtime/jit.rs:1457` 断言失败）
  ——该用例只在**有 AVX-512F 的 runner** 上真跑，本机无此硬件 ⇒ 长期只表现为 CI 抖动（#51 的"未知抖动"即此）。
  + 修法：①溢出模板**按值宽分档** `[spill.FPR16/32/64]`（`MOVUPS_RM/MR` 16B、`VMOVUPS_RM/MR` VEX.256、
    `VMOVUPS_ZMM_MEM/MR` EVEX.512）+ 新增 16B **MemRef 形式**指令 `MOVUPS_RM/MOVUPS_MR`（原 reg 基址形式
    disp 恒 0，表达不了 `[RBP-off]`）；②forge-dsl 生成器按 `width` 分派，**未声明宽度 → 编译期
    `Unsupported`**（fail-closed，绝不退回窄搬运；ISA 无 FPR 溢出模板时维持原 no-op）；③`reg_class_for`
    增加 `VEC(64)` 档（>32 字节向量）并在类表登记（`reg_width = 64`，槽宽与搬运宽度都按值宽）。
  + 验证：新增 `test_fpr_spill_width_dispatch`（直接驱动 `FrameLowering`，逐宽度断言机器码：8B `F2 0F 11/10`、
    16B `0F 11/10`、32B `C4..7C 11/10`、64B `62..11/10`（L'L=10），**24B/48B 必须报错**）；
    workspace tests 0 failed；e2e 8/8 + `stage_a passed=105/105 known=[]`；clippy `-D warnings`/fmt 干净。
  + 已知残余：向量**高压力** spill（>16 个同时活跃向量值）仍受 `[abi].scratch` 只有 2 个的限制
    （`instruction needs 3 scratch regs …`），由 `test_jit_v256_high_pressure_spill_is_known_limited` 断言记录。

+ **niche 枚举 tag 偏移一般化（WA-44）**：`lower/mod.rs` 新增唯一助手 `niche_tag_offset`（`statement.rs` 写侧与 `rvalue.rs` 判别读侧共用）——
  旧实现只认「`ScalarPair` 且第二标量是指针 → `b_offset`」，**其余一律 0**；于是 niche 落在聚合 payload **非 0 偏移**的枚举（如 24 字节 `Memory` repr、
  niche = 第 3 个字段 offset 16）读写都在 offset 0 ⇒ 判别读到字段 0 的值，该值为 0 时 `Some` 被误判成 `None`。
  + 修法（范围收窄）：①`ScalarPair` 分支**逐字保留** WA-26/WA-28/vl3 的经验判据（`b` 是指针类才用 `b_offset`，否则 0）；
    ②**只新增**「非 ScalarPair」分支 → `Variants::Multiple { tag_field }` + `fields().offset(tag_field)`（rustc_abi 文档明示 Niche 的 niche 在该 `tag_field` 字段）；
    ③其余仍 0；另加 fail-closed 尺寸守卫（`偏移 + tag 宽度 > 枚举尺寸` → 编译错误）。
  + 验证：新增 e2e `niche_offset_some_zero_first`（期望 12；修复前 exit=99）与 `niche_offset_none_roundtrip`（99）；
    e2e 全量 `passed=105/105 known=[]`；hammer（计划 §9.2）5 轮 `105/105 KNOWN=[]` + 5 例各 ×10 全过。
  + 范围教训：曾把 `ScalarPair` 分支"一般化"为「tag 与 `b` 同类就用 `b_offset`」——grow 链三例（`vec_push`/`vec_iter_enumerate`/`string_concat_len`）
    立即 `exit=0xC000001D`（gdb：`ud2`/`unreachable_unchecked`）⇒ 那些枚举的判据不能按 tag 标量类推，故保留原判据、只补聚合分支。

+ **宽聚合 payload 的 niche 枚举 `None` 写入宽度（WA-42，关闭 WA-41）**：`lower/statement.rs` 的 niche 构造把 tag 宽度按 `backend_repr`
  两分支（`Scalar`/`ScalarPair`）+ `_ => 4` 兜底推导——24 字节 `Memory` payload + offset 0 的 8 字节指针 niche（如 `Option<(NonNull<u8>, Layout)>`，
  即 `RawVecInner::current_memory` 的返回类型）落进兜底 → 发 `store i32 0`（只写低 4 字节），**高 4 字节残留栈上旧值** → 调用方按 8 字节判空失败
  → `finish_grow` 误取 `Some(野指针)` + 栈残留 `old_layout` → `grow_impl_runtime` 的 `copy_nonoverlapping` 解引用 → AV（`Vec::new(); v.push(1)` 即崩）。
  + 修法：宽度改取**枚举 tag 标量自身**（`Variants::Multiple { tag, .. }`，Niche 编码下 rustc 的 `tag` 即 niche 字段的标量）→ `tag.primitive().size()`；
    ≥8 字节写 I64（完整清零），窄 tag（u8/u16）行为不变。
  + 验证：本机 WA-41 最小复现由 `exit=-1073741819` 变 **16**；`FORGE_TRACE_IR` 对照 `store i32 0` 1 处 → 0 处；
    e2e `passed: 103/103 known=[]`（新增 IR 级回归门 `niche_wide_payload_none_tag_store_uses_tag_width`）。
  + 同一缺陷即 CI 上 `vec_push`/`vec_iter_enumerate` 的机型相关 AV（同一 grow 链、同一误判路径；AMD runner 栈残留高位恒非零、本机多数布局恰为 0）。
    CI 跨机器实证（run 93927221004，runner = AMD64 Family 25 Model 1，即修复前 20/20 AV 的同机型）：`[SUMMARY] stage_a passed=103/103 known=[]`、
    两例各 20 次单跑 `2x20`/`80x20`、alloc 逐步探针 21/21 全 ok。
+ **VEX/EVEX 解码臂对「内存形式 + reg 槽」直接报错（forge-dsl `vlen.rs`）**：v15 的 ModRM 模型允许
  `modrm = { rm = "[名字]" }` 指向 `reg` 槽（= 仅基址 `[base]`、disp 恒 0，见 `docs/reference/isa-dsl.md`），
  编码侧也已实现该风味，但 **VEX/EVEX 解码侧**仍保留旧的「内存形式 rm 必须是 mem 槽」守卫（生成期 `Err`）——
  于是任何 `[reg]` 形式的 VEX/EVEX 指令都无法加入 ISA。现已对齐：rm 槽为 reg 类时 base 取 `ModRM.rm + B`（SIB 在场按 `SIB.base`）。
  回归守卫 `test_jit_wide_vec_reg_base_mem_roundtrip_bytes`（字节往返 + objdump 实证）。
+ **`FORGE_ASSUME_AVX512` 泄漏到运行级 EVEX 用例 → 非法指令（`STATUS_ILLEGAL_INSTRUCTION` 0xC000001D）**：
  该 env 只应放开**生成期**可行性门，但 `test_jit_v512_byref_param` 用 `avx512_available()`（读 env）判断是否跳过——
  另一个测试留下的 env 会让它在无 AVX-512F 的 CPU 上**真的执行** EVEX。新增 `avx512_hardware_available()`
  （纯 cpuid、不读 env），运行级用例改用它判 skip；生成级用例的 env 开关收敛到 panic 安全的 RAII 守卫
  （`AssumeAvx512`，Drop 时清除）。实测：`cargo test -p forge-codegen --lib --all-features` 修复前 `0xc000001d` 崩在
  `test_jit_v512_byref_param`，修复后 `123 passed; 0 failed`。
+ **宽向量守卫里一条空断言**：`test_v512_byref_callee_load_is_64b` 的负向断言按 2 字节 VEX（`C5 FC 10`）匹配，
  而本编码器**恒发 3 字节 VEX**（`C4`）⇒ 该断言恒真（空守卫）。改为 `C4 .. 7C 10` 形态。
+ **e2e 门禁转正收官**：`vec_push` / `vec_string` / `vec_from_slice` / `vec_iter_enumerate` / `box_value` 移出 `FLAKY` 名单并翻 `known_failure=false`
  —— 5 例的错码/AV 从此**硬失败**（不再有 `CI-ENV-AV`/超时容忍路径；`FORGE_E2E_STRICT_FLAKY` 机制保留但名单为空即等价全量门禁）。
  判据按计划 §9.5 执行：CI run 93927221004 在同一 AMD 机型给出 `known=[]` + 探针 21/21 ok（§9.7.1）。

### Added (2026-08-08)

+ **forge-rustc 模块化重构（P1）**：`lib.rs` 2143 行 → 61 行（薄 facade），拆分为 9 个模块（`prelude`/`backend`/`func_ref`/`alloc_runtime`/`layout`/`types`/`abi`/`compile`/`rustc_compat` + `lower/` 6 文件）；对齐 CGCL 架构（mod prelude + codegen backend 模式）。
+ **rustc 1.99 nightly API 漂移适配（37 处）**：`CodegenBackend::codegen_crate`/`join_codegen` 签名变化、`CompiledModule.global_asm_object`、`BackendRepr::ScalarPair {..}`、`VariantLayout.field_offsets`、`EarlyBinder::bind(tcx, ..)`、`LangItem::DropGlue`、`substs.skip_binder()`、`Instance::resolve_drop_glue` 等——全部收敛进 `rustc_compat.rs` + `abi.rs`。
+ **ABI 层收敛（P4.1）**：`abi_kind_of_ty`（PassMode 投影：16 字节 Scalar → Indirect、SimdVector → Direct）成为 `is_agg_mem`/`is_scalar_pair_abi` 统一内核；`pad_call_args` 按 FnAbi 补齐 track_caller 隐藏 `&Location` 参数（修复 panic 路径参数错位）；sret 计数修正（rustc FnAbi.args 不含 sret 指针但调用方须传）；Ignore/ZST 参数跳过（Global 等零大小类型不占参数槽）。
+ **块参数传参一致性修复（WA-14）**：`map_terminator_args_to_params` 复用 arg 已有寄存器 + `pre_allocate_block_param_xregs` 顺序调整——write_bytes 内联循环从"完全不执行"变为执行（count=1 变体返回 0xAB 正确）；新增最小复现回归测试 `test_loop_block_param_write_bytes_style`。
+ **测试体系统一（P4.5）+ CI 纳入（P4.6）**：`stage_a.rs`/`run_tests.sh`/`test_runner.sh` 并入 `tests/e2e.rs`（58 用例，known_failure+reason 回归探针）；`.github/workflows/ci.yml` 新增 `forge-rustc-check` job。
+ **WORKAROUNDS.md**：14 条机读绕法清单（`[WA-NN]` 编号 + 代码注释引用）。

### Fixed

+ **write_bytes_loop 转正（e2e 56/58，WA-14 关闭）**：三层根因全修——①块参数传参两层（map_terminator 复用 arg 映射 + pre_allocate 顺序）；②**窄类型宽度（真根因）**——`opsize_from_type(u8)=32` 导致主库 Load/Store 越界 4 字节读写（u8 元素读 0xABABABAB 垃圾、write_bytes 循环写 32 字节覆盖相邻槽）+ `ireduce(mov)` 不扩展导致 cast 后高 24 位残留（wb1 返回 0xFFFFFFAB）。修复：新增 `LowerCtx::mem_opsize_from_type`（Load/Store 用真实内存宽度，**不枚举不截断**——u8→8、u16→16、自定义非常规宽度如 12 字节 GPR→96 原样传递）+ forge-rustc IntToInt cast 对 u8/u16 无符号源零扩展 mask。
+ **vec_push/vec_string 根因最终定性（十二轮深挖，仍 known_failure）**：**嵌套 niche 传播**（rustc 的 niche 布局传播——外层枚举判别与内层 payload 判别共享/嵌入字节，LLVM 级特性）：grow 链（Result/ControlFlow/TryReserveError 错误传播）与最小复现 cf5（`CF::Break(Err(5u8))` 应=7 现=1）同源——rvalue.rs/statement.rs 的 Niche 单层实现需扩展为嵌套传播（以 cf5 为驱动用例），WA-11 记录。
+ **field_offset Primitive 防护（落库）**：`lower/mod.rs` 的 `field_offset` 对非 enum 类型无条件调 `fields().offset()`——标量（`FieldsShape::Primitive`）无字段触发 "Primitive has no fields" 编译 ICE（嵌套枚举投影如 `CF::Break(Err(1u8))` 的 `.0` 对标量）——修复后嵌套 `ControlFlow<Result>` 从编译 ICE → 正确运行。
+ **诊断基础设施（落库，无行为影响）**：`FORGE_TRACE_TERM`（每块 terminator 打印）+ vcode dump 加 fn 名前缀（62 个函数的 vcode 此前无法区分——十二轮深挖的关键工具）+ regalloc_bt 的 spill/reload/evict trace。
+ **regalloc_bt 诊断 trace 补齐**：`spill_vreg`/`reload_from_stack`/`evict_and_assign` 增加 `[spill]`/`[reload]`/`[evict]` 输出（此前完全静默，无法定位跨块寄存器问题）。
+ **forge-rustc 3 个 known_failure 的编译层问题**：`vec_push`/`vec_string` 从 compile failed(101 ICE) 变为编译通过（sret 计数 + Ignore/ZST 参数跳过）。
+ **forge-codegen liverange 测试编译修复**：`RegClass::GPR` → `RegClass::GPR(8)`（多宽度化重构后测试代码未跟上）。
+ **forge-dsl 删除未使用 `stack_scratch` 变量**（`[abi.call]` 必需性校验块残留）。

### Added (2026-08-05)

+ **RegClass 宽度化重构（多宽度寄存器类）**：
  + `forge-ir::RegClass` 由 7 个硬编码变体改为 `GPR(u16)/FPR(u16)/VEC(u16)` 三变体，payload = 字节宽度，可表达任意 ISA 非常规宽度（如 12 字节 GPR）；`GPR64/GPR32/GPR8/FPR64/VEC128/VEC256/Int/Float` 等均为便捷常量，语义不变。
  + 物理寄存器索引全链 `u8 → u32`（`PhysReg::to_index/from_index`、`PReg.num`、`FrameAccess::register_index`、DSL 生成的 `uses/defs/reg_field/set_reg_field/clobbers`、`TargetRegInfo` 各列表、`ClassConfig.allocatable/sp_reg/fp_reg`）——支持 >255 寄存器的 ISA（如 JVM 类）。
  + `TargetRegInfo::register_classes()` 暴露 `[reg_classes.*]` 全部多宽度类（`RegisterClassInfo` 新增 `allocatable`）；`reg_class_width` 全类支持（TOML 优先，未定义类回退 payload）。
  + lowering 按类型分派：I32 结果/参数/块参数 → `GPR(4)` 池（32 位指令语义），F32/F64 → `FPR(8)`；未定义类回退（GPR 族继承 GPR64 池、FPR/VEC 继承 FPR64 池）。
  + 分配器新增跨宽度类物理重叠检测（`phys_conflicts`/`phys_owner`）：GPR(4) 与 GPR(8) 同编号视为同一物理寄存器，杜绝两个 XReg 分到同一物理寄存器。
  + `isa_from_file!` 路径解析支持向上查找（`CARGO_MANIFEST_DIR/../../..`），非 workspace 根 cwd 下编译也可定位 `isa/*.toml`。

### Added (2026-07-26)

+ **Directory restructuring**: `forge-codegen` (37 files → 5 subdirectories: `arch/`, `traits/`, `pipeline/`, `runtime/`, `ext/`) and `forge-opt` (24 files → 5 subdirectories: `scalar/`, `loops/`, `ipa/`, `advanced/`, `support/`). All backward-compatible `pub use` re-exports preserved.
+ **Multi-segment conditional merge (E1)**: Consecutive `?cond` segments with the same condition now share one `if` block in generated code instead of generating separate `if` blocks.
+ **Multi-block CFG JIT tests (F1)**: JIT tests for if-else branching, multi-parameter branching, loop countdown, stack frame with many locals, and boundary constants (i32::MAX/MIN, i64::MAX).
+ **AArch64 Icmp lowering (B1)**: All 10 integer comparison conditions activated in `isa/aarch64_v10.toml` (EQ/NE/LT/GT/LE/GE/LO/HI/LS/HS via SD_CMP + SD_SETCC).
+ **AArch64 + RISC-V compile tests (C2/C3)**: 5 new compile tests verifying add/mul, Icmp, and branch lowering across AArch64 and RISC-V backends.
+ **Constant pool inline syntax (C1)**: `{const 42}` / `{const 0xFF}` / `{const 3.14}` in lowering operands emits immediate values without constant pool lookup.
+ **Name-based field indexing**: V10 lowering path uses `ParsedTemplate` field names for operand-to-field mapping. `[lower.*]` operands must match BTreeMap alphabetical field order（v10 语法文档已随 v11 删除——现行唯一 DSL 语法见 `docs/reference/isa-dsl.md`）。

### Fixed

+ **Display round-trip 两个真 bug**（forge-ir）：`CallIndirect` 丢失函数指针（现输出 `call <retty> %ptr(...)`）；`StackAddr`/`GlobalAddr`/`Alloca` 丢失立即数（现输出 offset/大小，如 `stack_addr -4`）——经扩展指令 round-trip 测试暴露。
+ **审查驱动补测试（+7）**：forge 扩展指令 round-trip（stack_addr/copy/call_indirect/fconst 内联）、语义错误路径（undefined block/function）、`DataLayout::is_default`。见 `docs/archive/coverage-history.md`。
+ **覆盖率工具链诊断记录**：cargo-llvm-cov 在 Windows msvc + rustc 1.96 无法产出可靠报告（profraw 与二进制 counter 错位，全 0%），四种方案验证记录见 `docs/archive/coverage-history.md`。

### Fixed

+ **WASM32 `end` opcode**: Added `needs_epilogue_label()` trait method to `InstructionSet`, guarding x86-specific JMP emission. WASM functions now correctly terminate with `end` opcode (0x0B).
+ **`forge-plugin` missing `log` dependency**: Added `log = "0.4"` to Cargo.toml — `--all-features` compilation now succeeds.
+ **LEA constant pool FIXME**: Changed `constants: None` to `_constants_clone.as_deref()` in `lowering.rs`, enabling scale-value detection for LEA merge optimization.
+ **CLAUDE.md cleanup**: Removed outdated `.rs.bak` file references.

### Added (2026-07-24)

+ **Width-aware instruction model**: `FieldType::Opsize` + `DynType` in ISA model. Every GPR instruction now supports 16/32/64-bit operands via a unified encoding macro (`$modrm_rr`), automatically emitting 0x66 prefix (16-bit), default encoding (32-bit), or REX.W (64-bit) based on the opsize field.
+ **Opsize propagation from IR types**: `LowerCtx::default_opsize` is set from `Type::size_bytes()` before instruction lowering. `default_for_type()` for Opsize reads `ctx.default_opsize`, making all instructions width-aware automatically.
+ **17 new x86-64 instructions**: MOVZX (R8/R16), MOVSX (R8/R16), ADD/SUB/AND/OR/XOR/CMP r,imm32, CMPXCHG, XADD, BT/BTS/BTR/BTC, CMOVcc.
+ **3 new encoding macros**: `$modrm_r_imm32` (width-aware r,imm32), `$cmovcc_rr` (conditional move with embedded condition code).
+ **64-bit boundary fuzz tests**: 5 new tests exercising i64::MAX, i64::MIN, large multiply, power-of-two shift, and NOT operations.
+ **DynType validation**: `IsaModel::validate()` now checks that kind is a supported type and default is in values list.

### Fixed

+ **MOV64_RR encoding**: Changed from 0x8B to 0x89 (correct data direction: MOV r/m64, r64 → dest←src).
+ **MOVQ_R64_XMM mnemonic**: Changed from `movq.to_gpr` (dot breaks IDENT lexer) to `movq_to_gpr`.
+ **MOV_REG_IMM64 mnemonic disambiguation**: Changed to `mov_imm` to avoid AsmResolver collisions with MOV variants.
+ **@shift_cl primitive**: Added missing REX.W prefix for 64-bit shift operations (was emitting 32-bit shift with 0x41 instead of 0x49/0x48).
+ **Sshr lowering**: Uses `movsxd` (sign-extend 32→64) instead of zero-extending `mov` for arithmetic right shift.
+ **MOVSXD_R_RM**: Hardcoded to always emit REX.W (always sign-extends to 64-bit), removed opsize field.
+ **Prologue param copy TODO**: Resolved — `@move_args` already copies ABI arg regs → vregs via `Reg` type operands.
+ **LEA constant pool**: Added cloning pattern to avoid borrow conflicts (scale validation disabled pending PatternMatcher vreg allocation fix).
+ **Dead code warning**: Eliminated for `DynType.kind` and `DynType.values` (now used in validation).

### Changed

+ **AArch64 TODO updated**: Prologue/epilogue require STP/LDP/MOV_SP/SUB_SP with Reg-type operands.
+ **RISC-V TODO updated**: ADDI/SD/LD/JALR already defined; prologue needs Reg-typed variants.
+ **Backend TODOs cleared**: AArch64 and RISC-V prologue requirements accurately documented.

### Added (2026-07)

+ **Type::Bool**: New `Bool` type for comparison results (icmp/fcmp). Replaces `Type::I32` for boolean values, improving type safety and semantic clarity.
+ **Opcode::Freeze**: New IR instruction to prevent undefined behavior propagation. Optimization passes treat `Freeze` as a barrier for constant folding and value inference.
+ **SROA pass** (`src/optimize/sroa.rs`): Scalar Replacement of Aggregates optimization. Splits struct/array allocas into scalar allocas for mem2reg promotion.
+ **AArch64 backend** (`examples/isa/aarch64_v10.toml`): New ISA backend targeting 64-bit ARM (AAPCS64 calling convention). Supports GPRs (X0-X30), FPRs (V0-V31), and standard instruction set.
+ **CI configuration** (`.github/workflows/ci.yml`): Automated formatting, clippy, test, and docs checks across Linux/Windows/macOS.
+ **Encoding DSL enhancements**: Declarative encoding format support for x86 and RISC-V instruction patterns.
+ **PE/COFF relocation**: Format-aware relocation mapping for PE COFF (IMAGE_REL_AMD64_*) and Mach-O (X86_64_RELOC_*).
+ **MIR extensions**: Expanded rustc MIR rvalue/terminator coverage (Repeat, Aggregate, CastKind variants, Assert, Yield).
+ **LTO integration**: `Module::optimize_with_lto()` for cross-module optimization (inlining + dead function elimination).
+ **E-Graph ISel**: `ISelPass` for algebraic simplification before instruction selection.
+ **Performance benchmarks**: Criterion-based compilation pipeline benchmarks in `benches/compile_bench.rs`.
+ **JIT multi-return**: Support for two-value returns (RAX + RDX) in the x86-64 JIT backend.
+ **Register spill/reload**: Full spill handling for high register pressure scenarios using R10/R11 scratch registers.
+ **Extended JIT test suite**: 129 integration tests covering parameters, returns, stack balance, register pressure, multi-return.

### Changed

+ **Comparison result type**: `icmp` and `fcmp` now produce `Type::Bool` instead of `Type::I32`.
+ **Copy instruction**: Now infers result type from source operand instead of hardcoding `Type::I32`.
+ **Clippy clean**: All clippy warnings resolved in the main library and DSL codegen.
+ **Rustc backend**: MonoItem path updated for latest nightly; Bool type mapping fixed to `Type::Bool`.

### Fixed

+ Register allocator spill offset calculation (RBP-relative negative offsets).
+ PE/COFF relocation flag mapping for x86-64 Windows targets.
+ Mach-O relocation field naming (r_type, r_pcrel, r_length).
+ Collapsible `str::replace` calls in DSL codegen (clippy).

---

## Version Policy

+ **0.x.y**: API may change without notice. No stability guarantees.
+ **1.0.0** (future): Public API frozen. Requires: rustc backend passes core/alloc tests, AArch64 backend functional, CI all-green, CHANGELOG maintained.
