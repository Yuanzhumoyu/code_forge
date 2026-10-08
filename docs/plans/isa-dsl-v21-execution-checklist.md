# ISA-DSL v21 执行清单

> [progress] 2026-10-08 执行方案（配套设计文档 [isa-dsl-v21-redesign-plan.md](isa-dsl-v21-redesign-plan.md)）。
> 用法：**一片一提交**，上一片的门禁全绿才勾下一片；每片先勾"步骤"再勾"门禁"。
> 数字基线以 §1.3 的 W0 快照为准（本文写的数字是 2026-10-08 的设计期快照，会随迭代漂移）。

## 目录

- [0. 全局纪律与不变量](#0-全局纪律与不变量)
- [1. S0 冻结与基线](#1-s0-冻结与基线)
- [2. 通用门禁命令](#2-通用门禁命令)
- [3. W1 寄存器重定基](#3-w1-寄存器重定基)
- [4. W2 字段语法](#4-w2-字段语法)
- [5. W3 操作数层收敛](#5-w3-操作数层收敛)
- [6. W4 stream 段模型](#6-w4-stream-段模型)
- [7. W5 族与重载](#7-w5-族与重载)
- [8. W6 选择层](#8-w6-选择层)
- [9. W7 文本层](#9-w7-文本层)
- [10. W8 重定位生成](#10-w8-重定位生成)
- [11. W9 machine 合并](#11-w9-machine-合并)
- [12. W10 删旧与换版](#12-w10-删旧与换版)
- [13. W11 通用性实证](#13-w11-通用性实证)
- [14. 最终验收](#14-最终验收)
- [15. 中止条件](#15-中止条件)

---

## 0. 全局纪律与不变量

- [x] **没有兼容层**：旧键一律 `deny_unknown_fields` 报未知键；不写"迁移期双读"。
  - 例外只有一个：W1/W2 期间为让三谱同时可解析，允许**同一次提交内**改完谱与代码；不允许跨提交并存两套读法。
- [x] **单一写者**：生成物只由宿主 build script 写（`pregenerate_host()`），宏侧只发 `include!`。
- [x] **字节等价**：W1–W9 的任何一步都不得改变三谱的编码字节；出现差异一律先当 bug，不许改期望值。
- [x] **逐数字不变**：每片的门禁数字与基线逐项相等；**只允许"减少"的方向**（歧义名单、lint 清单）。
- [x] **一处实现**：schema 表 ↔ `dsl/model.rs` ↔ 文档键表 ↔ 签入的 `isa-dsl.schema.json` 四处同改（`schema_guard` 钉住）。
- [x] **不做**：`[snippet]`、表化 lowering 解释器、自研文本语法（见设计文档 §8.3）。

---

## 1. S0 冻结与基线

### 1.1 冻结决策（未签字不动代码）

- [x] D-1 "是不是操作数"由 `ops` 是否列出该槽决定（设计文档 §13.2）
- [x] D-2 删除 `[meta]` 五个宽度键，改由 `[machine] gpr/fpr/addr` 指组名
- [x] D-3 操作数槽缺 `= bits` 视作 wildcard；只有常量槽缺省才是"全 0"
- [x] D-4 保留 `bind = { 槽名 = "操作数名" }` 作为唯一改名机制
- [x] D-5 散布位段按**值低位 → 高位**书写
- [x] D-6 未被字段覆盖的位 = wildcard，由 `lint --bits` 报出（不强制 0）
- [x] 冻结项 1：`stream` 段子表键名（默认方案见设计文档 §6.2.7）
- [x] 冻结项 2：`[snippet]` 不引入，只登记为待度量项
- [x] 冻结项 3：`form` 允许一层 `base` 继承

### 1.2 决策落地记录

- [x] 把上述勾选结果写回设计文档 §13（决策 → 已定稿），并注明日期。（2026-10-08）

### 1.3 基线快照（先采集，后动手）

- [x] 采集命令输出并记入执行记录（每条都记日期与命令原文）：

```powershell
cargo run -p forge-isa -- validate isa/x86.toml isa/riscv64.toml isa/arm64.toml
cargo run -p forge-isa -- lint isa/x86.toml isa/riscv64.toml isa/arm64.toml
cargo run -p forge-isa -- insts isa/x86.toml
cargo test -p forge-isa-dsl
cargo test -p forge-codegen --lib isa_roundtrip_guard
cargo test -p forge-codegen --lib spec_coverage_guard
cargo test -p forge-tests --test asm_parse
cargo test -p forge-tests --test asm_encoding
cargo test -p forge-tests --test asm_exec
cargo test -p forge-tests --lib jit_matrix_x86 -- --nocapture
cargo test -p forge-tests --lib jit_matrix_riscv64 -- --test-threads=1 --nocapture
```

- [x] **Phase 1（干净，改动前）**：`validate`/`lint` 三谱**全部 exit 0**；`forge-isa-dsl` 19 个二进制全绿；
  `isa_roundtrip_guard` ok。日志：`target/v21-baseline/W0.txt`。
- [ ] ⚠ **Phase 2 受污染**：`spec_coverage_guard` 之后的命令是在 W1 已开始改代码之后才跑的，
      日志里是改动期的编译错误 ⇒ **不可作为基线**。改用签入的棘轮文件
      （`crates/tools/forge-tests/asm/ratchet/*.txt`、`pins.txt`）与下表（W1 后实测，须与基线**相同**）。
- [x] 记录四项权威计数（W1 后实测；棘轮由测试逐项相等守）：
  - [x] `spec_coverage_guard`：**4 passed**（指令总数 265 / 119 / 110 不变）
  - [x] `isa_roundtrip_guard`：**1 passed**（三谱全量 `encode→decode→encode` 字节闭环）
  - [x] asm 三档棘轮：`asm_parse` / `asm_encoding` / `asm_exec` 在 `--workspace` 全量跑中**通过**（棘轮逐项相等）
  - [x] JIT 矩阵：**x86 197/3/0**、**riscv64 136/64/0**（`--nocapture` 打印，与基线逐数字相同）
- [x] 三张评审清单快照由 `tests/lint_shipped.rs` 钉住，全绿即未变长。
- [ ] 记录生成物规模与生成耗时（`FGE_DEBUG_GEN=1` dump 三份 `tm`），作为 W10 的对照 —— **待 W10 前补**。

---

## 2. 通用门禁命令

每片结束都跑这一组；表中"期望"以 W0 快照为基准。

| 命令 | 期望 |
| --- | --- |
| `cargo run -p forge-isa -- validate isa/*.toml` | 0 诊断、退出码 0 |
| `cargo run -p forge-isa -- lint isa/*.toml` | 三谱零结论 |
| `cargo test -p forge-isa-dsl` | 全绿（含 `schema_guard` / `lint_shipped` / `determinism` / `vectors`） |
| `cargo test -p forge-codegen --lib isa_roundtrip_guard` | 条数与 W0 相同 |
| `cargo test -p forge-codegen --lib spec_coverage_guard` | 三谱指令总数与 W0 相同，零跳过 |
| `cargo run -p forge-isa -- schema --out isa-dsl.schema.json` | 有键改动时才跑；跑完 `git diff` 必须只有预期差异 |
| `cargo test -p forge-tests --test asm_parse` | 计数棘轮逐项相等 |
| `cargo test -p forge-tests --test asm_encoding` | 计数棘轮逐项相等 |
| `cargo test -p forge-tests --test asm_exec` | 无新增 SKIP/FAIL |

> 棘轮刷新只在**明确要改口径**时做：`$env:FORGE_ASM_WRITE_RATCHET = "1"`，刷完**必须逐行看 diff**。

---

## 3. W1 寄存器重定基

> ✅ **已完成（2026-10-08）**。口径 = 决策 B（**连 IR 一起重定基**）：
> `forge_ir::RegClass` 的 payload 由字节改为**位**，`width()` 拆为 `bits()`（payload）与 `bytes()`（派生），
> `default_width()` → `default_bits()`。用"改名驱动"逼编译器暴露全部调用点，共分类
> **92 处** `.width()`/`.default_width()`/`.bits()`。

**目标**：组名 = 类 + 位宽（`gpr64`），删 `class`/`bits`，`RegClass` 内部由字节改位。

- [x] `dsl/model.rs`：`RegClass` 的承载由"字节数"改"位数"（`GPR(64)`），`FromStr`/`Display` 同步。
- [x] `dsl/validate.rs`：组名解析改为"族前缀 + 位宽"，宽度不再来自 `[meta]`。
- [x] `[meta]` 五个宽度键删除；新增 `[machine] gpr/fpr/addr/value_gpr/value_fpr` 指组名（决策 D-2）。
- [x] `src/schema.rs` + 重生成 `isa-dsl.schema.json` + 文档键表（四处同改）。
- [x] 15 份 TOML 原子改名（**单遍映射**，避免 `gpr8→gpr64` 与 `gpr1→gpr8` 相撞）：
      三谱 + `tests/isa/*.toml` + `examples/isa-host-demo/isa/toy16.toml`。
      注意：**`kreg8→kreg64`**（x86 掩码寄存器是 64 位），文档里同理。
- [x] 全仓 grep 旧名残留；`forge-ir`/`forge-isa-runtime`/`forge-codegen`/`forge-isa-dsl` 各自测试模块同步。
- [x] 生成物常量（`__DEFAULT_GPR_CLASS`/`__ADDR_CLASS`/`___SLOT_BYTES` 等）与宿主 `TargetRegInfo` 同步。
- [x] `no_hardcoded_widths.rs`（三份）与 `library_surface.rs` 白名单核对。
- [x] 门禁全绿：§2 + 工作区 `--all-targets` 零错误；`cargo test --workspace --exclude forge-rustc` **0 个失败二进制**；
      `isa_roundtrip_guard` 1 passed；`spec_coverage_guard` 4 passed；
      JIT 矩阵 x86 **197/3/0**、riscv64 **136/64/0**（与基线逐数字相同）。
- [x] 回滚点：本片改动尚未提交，可整体 revert（不做部分回滚）。

**重定基过程中由编译器/门禁抓出的 6 处单位错（均已修，记录以免复发）**：

1. `dsl/codegen/vlen.rs` 的 `reg_view`（opsize 守卫）必须是**字节**——误用位会让 64 位变体守卫失效、
   解码落到 32 位（实测 `MOV64_RR` 往返丢 REX.W）。
2. `vlen.rs` 的 opsize 表达式：`PhysReg::width()` 是**位**，生成物 `__opsize` 是**字节** ⇒ 需 `/8`。
3. `vlen.rs` 的宽度校验比较（位 vs 字节）。
4. `dsl/codegen/mod.rs`：`RegClass::GPR(__opsize)` ⇒ `RegClass::GPR((__opsize as u16) * 8)`。
5. `forge-isa-runtime/src/ctx.rs`：`vector_tiers`（字节）→ `RegClass::VEC(tier * 8)`。
6. `dsl/codegen/lowering.rs`：`pred_width_hint` 把谓词值（位）除以 8 ⇒ demo ISA 宽度分派选错指令。

**风险提示**：`gpr8` 旧 = 64 位、新 = 8 位，**语义相反**；漏改即静默编错字宽，必须靠字节回归兜住。

---

## 4. W2 字段语法

**目标**：`form` 内就地 `fields` 语法；删 `[conventions.bitfields]` + `opcode_field` + `operand_fields`。

- [ ] `dsl/model.rs`：新增字段声明类型（类型表达式 / 位区间列表 / 接口名 / 默认值）。
- [ ] 解析与诊断：位区间语法错误、类型不存在、宽度不符、`ops` 引用未知槽名 —— 全部带 `路径:行:列`。
- [ ] 绑定规则落地：**`ops` 列出的槽 = 操作数槽（wildcard）；其余 = 常量槽**（决策 D-1/D-3）。
- [ ] `match` 覆盖（常量槽换值 / 操作数槽钉常量）；`bind` 改名（决策 D-4）。
- [ ] 位段相交的逐指令校验（同一条指令不得同时绑定相交两段）。
- [ ] 删除 `[conventions.bitfields]` / `opcode_field` / `operand_fields` 的模型、schema、校验、lint 码。
- [ ] 三谱迁移：riscv（R/I/S/B/U/J + 散射）、arm64（含 TBZ 双段、保留位）、x86（先只做**定宽字段部分**，stream 留 W4）。
- [ ] `lint --bits` 口径确认：未被字段覆盖的位仍按现状报出（决策 D-6）。
- [ ] 门禁：§2 全绿；`spec_coverage_guard` 三谱指令数不变；`LINT-BITFIELD-OVERLAP` 不再需要例外。
- [ ] 迁移脚本与人工决策分开：脚本只搬位置，**保留位/散射方向**逐条人工复核（arm64 最高风险）。

---

## 5. W3 操作数层收敛

**目标**：`[operand.*]` + `[enum.*]` + `[encode.*]`；8 个一次性开关 → 5 个正交键。

- [ ] `kind`/`bits`/`signed`/`range`/`unit` 定稿；`min`+`max` 合一为 `range`。
- [ ] `wrap → literal`、`float → value`、`table`+`names → enum`、`arrangement → suffix`。
- [ ] `symbols`+`require_symbol`+`imm_fns → symbol { allow, require, modifiers }`。
- [ ] `[conventions.cond]`/`bitsets`/`imm_names` → `[enum.*]`；`--bits` 的位集合语义保持。
- [ ] `[conventions.mem].templates/size_keywords` → `[operand.mem].text/size_words`。
- [ ] `zr31 → zero + sp`；`byte_reg → [reg.*].rex_required`。
- [ ] 三谱迁移 + `enum` 生成脚本（riscv CSR 表由 `gen-riscv-csr-table.mjs` 产）改指向新节。
- [ ] 门禁：§2 全绿；`abi check` 三谱结论与 W0 相同。

---

## 6. W4 stream 段模型

**目标**：x86 的 20 个 form 名与 37 处内联编码键，改成 `kind = "stream"` + 段。

- [ ] 段 kind 闭集落地：`prefix`/`escape`/`rex`/`opcode`/`opcode_reg`/`modrm`/`sib`/`disp`/`imm`/`vex`/`evex`。
- [ ] 段内 `fields` 复用 W2 语法；`segments` 序 = 发射序。
- [ ] 前缀表显式化：**删除 `default_prefix_scan()`**；效果参数化（`opsize`/`addrsize`/`lock`/`rep`/`reg_ext`/`map`/`opaque`）。
- [ ] 解码条件由前缀段 `bytes` 生成（删 `prefix_cond_ts` 的按字节枚举）。
- [ ] REX2 改为段数据（`map` + `reg_ext.bits` + 位序），删字节手术。
- [ ] `force_disp_base` 上界由 rm 字段宽 + `reg_ext.bits` 推出。
- [ ] `opsize` 四态退化为 `word_size` 规则 + 显式绑定；`"sN"` 移除。
- [ ] x86 谱迁移（116 处 `modrm`、84 处 `fields`、128 条 form/覆盖混合）。
- [ ] 门禁：§2 全绿；x86 编码对拍（`asm_encoding`）棘轮不变；`encoder_fuzz_tests` 绿。
- [ ] 冻结项 1 复核：段子表键名定稿后写回设计文档。

---

## 7. W5 族与重载

**目标**：`family` + `rows` + `vary` + `group`；删 `ref` 与宽度 `when` 分派。

- [ ] `ref → group`；族名即多态引用名。
- [ ] `family.vary` 与 `[lower.*].vary` 共用一套 zip 语义（一处实现）。
- [ ] 重载解析：按声明类型（字段类型）选行；歧义 → 生成期报错并列候选；无候选 → 生成物 `Unsupported`。
- [ ] 先做机械合并：x86 的 122 条同签名块、riscv 的 28 组候选、arm64 的 REV/abs-neg 族。
- [ ] 宽度分派清理：能由重载覆盖的 `when = { rs1_width = … }` 规则删除，其余保留。
- [ ] 门禁：§2 全绿；文本歧义名单**只减不增**（x86 36 / riscv64 4 / arm64 42 为上限）。
- [ ] 每合并一族都要复跑字节闭环，禁止批量合并后一次性验证。

---

## 8. W6 选择层

**目标**：分支/跳转/返回下放到谱；emit 表达式与 `temps`。

- [ ] 新增符号值：`{true}` / `{false}` / `{target}` / `{cc}` / `{cc_inv}`。
- [ ] `{cc_inv}` 由 `[enum.cond]` 反查（替换硬编码 `4u8`）。
- [ ] 三谱写出 `[lower.Br]` / `[lower.Jmp]` / `[lower.Ret]`：
  - [ ] x86：`test` + `jcc` + `jmp`
  - [ ] riscv：`beq …, x0, {false}` + `jal x0, {true}`
  - [ ] arm64：`cbz {0}, {false}` + `b {true}`
- [ ] 删除编译器三处按字段个数猜 ISA（`lowering.rs:202`、`:527-625`、`frame.rs:172-208`）。
- [ ] emit 表达式落地（`{imm0 - 4}`、`{iconst >> 12 & 0xfffff}` 等），删 20 余个派生 token。
- [ ] `temps = [...]` 显式临时寄存器，删 `{g}`/`{gN}`/`{f}`/`{fN}` 隐式声明。
- [ ] 新增第四形态夹具 ISA（同一份谱里两个分支形态）验证"编译器不再猜"。
- [ ] 门禁：§2 全绿；三矩阵计数不变。
- [ ] 待度量项：`[snippet]` 按 riscv 14 处复用重测，收益过阈值才单开一片。

---

## 9. W7 文本层

**目标**：`asm` 列表 + 显式修饰符；内存操作数走同一套组件解析。

- [ ] `asm` 支持可接受写法列表，第 0 条 = 渲染形态。
- [ ] 占位修饰 `{name?}` / `{name:affix}` 落地；删"紧邻 `{size}` 的字面量不是条件前缀"例外。
- [ ] `[lower.*].emit` 的内存操作数改用 `[operand.mem].text` 解析，删私有语法（`integration.rs:920-946`）。
- [ ] `[asm.immfn.*].texts` 多拼写：arm64 98 个 `imm_fn` → 约 49 个名字。
- [ ] `[asm.pseudo.*]` 迁移（x86 20 条、riscv 2 条）。
- [ ] 门禁：§2 全绿；`disassemble → assemble` 闭合由生成期自测与 `[[vectors]]` 闭环形态覆盖。
- [ ] 反例验证：同助记符两条写法都能匹配时，lint 报歧义、不靠声明序兜。

---

## 10. W8 重定位生成

**目标**：重定位补丁器由谱生成，删宿主按 ISA 名分发。

- [ ] `[asm.reloc.*]` 用 `operand` 指槽，重排复用该槽字段的位区间。
- [ ] 生成通用补丁器；删除 `reloc_patcher.rs:77-84` 的 ISA 名分发表与两个手写补丁器。
- [ ] `forge-isa-runtime` 的运行面守卫（`runtime_surface.rs`）同步。
- [ ] 验证：三谱的 CALL/GLOBAL/分支重定位字节不变；riscv/arm64 的 UJ/B 型散布位段覆盖。
- [ ] 门禁：§2 全绿；`asm_exec` 真跑通道（riscv64/aarch64 走 QEMU）无新增 SKIP。

---

## 11. W9 machine 合并

**目标**：`[machine]` 子表；spill 按组分派。

- [ ] `[stack]`/`[emit]`/`[types]` → `[machine.stack]`/`[machine.text]`/`[machine.classes]`（单位统一为位）。
- [ ] `[spill.*]` → `[machine.spill.<组名>]`，按组分派；写错键名 = 校验期错误（不再静默 no-op）。
- [ ] 键改名（逐项勾）：
  - [ ] `fixed_regs` → `fixed`、`spill_scratch` → `scratch`、`link_reg` → `link`
  - [ ] `frame_padding` → `padding`、`arg_slot` → `arg_place`
  - [ ] `vector_by_ref_bytes` → `byref_bytes`
- [ ] 删除 `callee_saved_gpr` / `callee_save_slots`。
- [ ] 门禁：§2 全绿；`abi check` 与三矩阵不变。

---

## 12. W10 删旧与换版

**目标**：v18 代码路径、文档、守卫全部清干净。

- [ ] 删除 v18 键的模型/schema/校验/生成器分支；`deny_unknown_fields` 全量生效。
- [ ] `docs/reference/isa-dsl.md` 换为 v21 规范（键表由 schema 生成）。
- [ ] `docs/reference/isa-dsl-errors.md` 错误码目录同步。
- [ ] `docs/guides/isa-dsl-tutorial.md` 按新语法重写，并保持 `tests/tutorial_spec.rs` 绿。
- [ ] `CLAUDE.md` 的 ISA-DSL 相关段落（Key Architecture Rules、Testing Notes）全量回写。
- [ ] `CHANGELOG.md` 记一条 **Breaking** 条目（用户可见变更才进 CHANGELOG）。
- [ ] 设计文档转 `[archive]` 或标注"已落地"，执行清单标注完成日期。
- [ ] `git grep` 全量检查：无 `conventions.bitfields` / `operand_fields` / `opcode_field` / `gpr8`(64 语义) / `[[forms]]` 残留死链。
- [ ] 门禁：§2 全绿 + `cargo test --workspace --exclude forge-rustc`。

---

## 13. W11 通用性实证

**目标**：新增第四个 ISA 零编译器改动。

- [ ] 新夹具 ISA（非 x86/riscv/arm64；定宽 + 一态变长各一）：只用 TOML + `arch/<new>.rs` + 生成的补丁器。
- [ ] 新守卫：断言"新增 ISA 的改动面只有 TOML 与 `arch/<new>.rs`"（扩展 `generality_guard` 的口径，从"无 ISA 名"升级到"无 ISA 形状分支"）。
- [ ] 用 `forge-isa validate/lint/abi/test` 四档跑通该夹具。
- [ ] 若有任何一处必须改编译器 ⇒ 记入设计文档"受控扩展点"清单并说明理由。

---

## 14. 最终验收

- [ ] 三谱编码字节与 W0 逐字节相同（`isa_roundtrip_guard` + `asm_encoding` 双证）。
- [ ] 三矩阵计数与 W0 逐数字相同。
- [ ] `lint` 三谱零结论；`--refs`/`--bits`/`--suggest` 清单只减不增。
- [ ] 设计文档 §1.2 的 D1–D6 逐条给出实测证据（节数、机制数、位置语义数、后门清除条数）。
- [ ] 度量对比：三谱行数、生成物 token 规模、`cargo check -p forge-codegen` 时间（相对 W0）。
- [ ] 文档三方针一致：`schema.rs` ↔ `dsl/model.rs` ↔ 参考文档键表 ↔ `isa-dsl.schema.json`。
- [ ] 所有新增/改动文档 `npx markdownlint-cli2 <文件>` 到 0 error。

---

## 15. 中止条件

出现下列任一情况，**停下来回到设计文档**，不要硬推：

- [ ] 字节等价无法保持，且原因不是迁移脚本笔误（例如新语法表达不了某个现有编码）。
- [ ] 某片门禁连续两轮不过，且根因是设计（不是实现）。
- [ ] 出现必须"为某个 ISA 加编译器分支"才能表达的写法 ⇒ 说明"slots × patterns"层没设计够，回到设计文档补。
- [ ] 迁移人工判断量超出设计文档 §10 的 40% 估算 1.5 倍以上 ⇒ 重新评估是否拆分或缩小范围。
