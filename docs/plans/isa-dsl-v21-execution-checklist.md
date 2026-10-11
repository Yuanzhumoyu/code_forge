# ISA-DSL v21 执行清单

> [progress] 2026-10-08 执行方案（配套设计文档已归档：[isa-dsl-v21-redesign-plan.md](../archive/isa-dsl-v21-redesign-plan.md)）。
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

### 0.1 陈账与账目口径（2026-10-08 晚核对）

本文档的勾选框是**计划步骤**（设计期写法），落地状态以**每节的收口记录** + `CHANGELOG.md` +
§14 快照为准。截至**快照 #3**，W1–W11 + W6 + W6+ **全部收口**。
**2026-10-08 晚已把全部 42 个框逐条对账**：37 个"已落地的计划步骤"**打勾**、
1 条过程注记与 4 条中止判据**去掉复选框**（它们本就不是任务）——现在文档里**不该再留
无主的未打勾框**（有就是真待办）。

- **§4 W2 / §5 W3 / §6 W4 / §7 W5 / §8 W6 / §9 W7 的未打勾项**：**2026-10-08 晚已逐条打勾**
  ——它们都是计划步骤、**已落地**，证据在各节自己的收口记录（如 §7「W5 收口」、
  §9「W7 收口核对」）、`CLAUDE.md` 的「v21 现行面速查」、以及 §14 快照 #3 的实测表。
  逐项的落地形态：
  表头即名字三张表 + `fields` 就地声明；`range`/`value`/`enum`/`suffix`/`symbol`/`[enum.*]`/
  `[operand.mem].text`/`zero`+`sp`/`rex_required`；`modrm`/`opsize = "sN"`/`classes` 键归零；
  族合并与重载；`{size}` 例外删除、`[asm.immfn]` 多拼写、`[asm.pseudo.*]` 迁移。
- **§1 的两条**：是**过程注记**——「Phase 2 受污染」是当时测法的**如实记录**（W1 已改代码之后
  才跑），不是待办；「记录生成物规模与耗时」**已补** ⇒ `docs/performance/generated_compile_profile.md`
  （采集 2026-09-24，分段体积 + 编译时间归属）。
- **§15 的四条**：是**中止判据**（"出现 X 就停"），不是任务 ⇒ **永远不打勾**；
  目前四条都没触发。
- **真正未做的只剩 §14 末尾那四项**（表化 emit = 已决定不做；**删隐式 `{gN}`/`{fN}`** =
  建议不做（661 处 breaking；注意 `temps` 的**强制档早已实现**，此前记反了）；
  arm64 `al`/`nv` = 待 IR 补恒真/恒假条件的**设计决策**；42 个框的对账**已完成**）。

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
- ⚠ **Phase 2 受污染（历史注记，非待办）**：`spec_coverage_guard` 之后的命令是在 W1 已开始改代码之后才跑的，
      日志里是改动期的编译错误 ⇒ **不可作为基线**。改用签入的棘轮文件
      （`crates/tools/forge-tests/asm/ratchet/*.txt`、`pins.txt`）与下表（W1 后实测，须与基线**相同**）。
- [x] 记录四项权威计数（W1 后实测；棘轮由测试逐项相等守）：
  - [x] `spec_coverage_guard`：**4 passed**（指令总数 265 / 119 / 110 不变）
  - [x] `isa_roundtrip_guard`：**1 passed**（三谱全量 `encode→decode→encode` 字节闭环）
  - [x] asm 三档棘轮：`asm_parse` / `asm_encoding` / `asm_exec` 在 `--workspace` 全量跑中**通过**（棘轮逐项相等）
  - [x] JIT 矩阵：**x86 197/3/0**、**riscv64 136/64/0**（`--nocapture` 打印，与基线逐数字相同）
- [x] 三张评审清单快照由 `tests/lint_shipped.rs` 钉住，全绿即未变长。
- [x] 记录生成物规模与生成耗时（`FGE_DEBUG_GEN=1` dump 三份 `tm`），作为 W10 的对照 —— **待 W10 前补**。

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

> ✅ **已完成并提交（2026-10-08）**。最终形态与证据：
>
> - **新语法**：`fields = ["类型[位区间]:名字=默认值", …]`（写 `[[forms]]` / `[[instructions]]` 上）。
>   连续 `"u5[11:7]:rd"`；散射 `"i13[12:1 -> 31,30:25,11:8,7]:imm_b"`（值位段 → 词位块，值低位→高位）。
>   类型支持 `uN`/`iN`/联合 `|`/寄存器组名/槽名；默认值 `0x`/`0b`/负数十进制。
> - **实现的降级（lowering）**：`field_decl::lower_field_syntax` 把声明展开成内部
>   `bitfields` + 逐指令 `operand_fields` + form 的 `opcode_field`；三个旧键改 `#[serde(skip)]`，
>   用户写即报未知键。**编码器/解码器/生成物一行未改**——字节等价由构造保证。
> - **W2.5 命名空间**：同名不同区间时内部键取限定名 `<form>.<名字>`（v18 位域表是全局命名空间，
>   v21 字段是 form 局部的；arm64 有 10 个 form 各要一个 opcode 字段，区间还不同）。
> - **绑定口径 A**：`bind` 显式 → 同名 → 按 `fields` 声明序取第 i 个（跳过 opcode）。
> - **主 opcode 字段统一叫 `opcode`**（v18 的 `op8`/`mtop`/`word` 等 10 种名字从不被 `match` 引用，
>   改名零牵连）。
> - **迁移范围**：三谱 + 夹具谱 **76 个 form** 改写、**13 条显式 `bind`**（arm64 向量排列族
>   `VADD`/`VSUB_ARR`/…/`VMLA_ARR`，三个操作数绑同一个 `vq`）、~93 处内联夹具、`include_base.toml`。
> - **门禁（实测）**：`isa_roundtrip_guard` 通过；谱内向量全过；asm 三档棘轮逐项不变；
>   x86 197/3/0、riscv64 136/64/0 与迁移前**逐数字相同**；`cargo test --workspace --exclude
>   forge-rustc` **0 个失败二进制**。
> - **口径变化（如实记录）**：`lint --bits` 清单变长（riscv 7→13、arm64 262→264）——form 声明但
>   **本指令未绑定**的槽不再算该指令的覆盖（那些位按缺省 0 发射）；`pins.txt` 已同步并注明原因。
> - **语法扩展决策**：`->`（值位段 → 词位块）不在原始语法里，是为表达 riscv S/B/J 的散射而加；
>   riscv `off_b` 无 `unit`、值自身 bit 0 不参与，连续写法表达不了。
> - 迁移期工具：`field_decl::print_migrated_form_fields`（打印器）、两条 `#[ignore]` 的
>   迁移前证据测试（等价性与 `bind` 工作单）。
>
> 以下为原计划步骤，全部已并入上面的实现：

**目标**：`form` 内就地 `fields` 声明；删 `[conventions.bitfields]` + `opcode_field` + `operand_fields`。

### 4.0 实现口径（本片决策，2026-10-08）

**把新语法"下降（lower）"到既有内部表示**，编码器/解码器/生成物**一行不改** —— 于是
"字节等价"由构造保证，风险集中在 model/解析/校验与三谱迁移：

- 一个新的 **lowering pass** 把 `fields` 声明列表展开成内部已有的三样东西：
  ① `conventions.bitfields`（字段名 → `offset/width` 或 `pieces`）；② 每条指令的
  `EncKeys.operand_fields`（按 `ops` 序 → 绑定的字段名）；③ `Instruction.opcode` 与
  `Instruction.fields`（常量值与 `match` 覆盖）。
- **多段位区间 → `pieces` 的换算**（唯一需要小心的地方）：按"值低位 → 高位"列出，
  第 i 段的 `shift` = 前面各段宽度之和，`offset`/`width` = 该段位置与宽度。
  这与既有 `pieces` 编码器（`word |= ((value >> shift) & mask) << offset`）逐位等价。
- 内部字段（`bitfields`/`opcode_field`/`operand_fields`）改为 **`#[serde(skip)]` 派生**：
  用户写不了，只能由 `fields` 展开产生。

**本片不做（留 W2b，并在 W5 重载解析时一并落地）**：`ops` 的 `名字:槽[:角色]` 三token形态
**保留**——字段类型只做**校验**（`uN/iN` 宽度 == 字段总位宽；寄存器组编号放得下；槽名存在），
不替换 `ops` 的类型来源。把类型搬进字段、`ops` 收敛成"名字 + 角色"是独立一步。

### 4.1 步骤

- [x] **W2.1 `fields` 值映射改名 `match`**（先解键冲突：v21 要让 `fields` 表示"声明列表"）。
      `Instruction.fields` 只改 serde 键名（`#[serde(rename = "match")]`），Rust 侧零改动；
      三谱 + 夹具 + schema + 文档键表同步。实测：**704 处 / 13 文件**，
      `forge-isa-dsl` 与 `forge-codegen` 全绿（`fields = {` 残留 0，
      `[[operand_slots]].fields = [...]` 的 58 处保持不动）。
- [x] **W2.2a 字段声明的解析与降级**（纯新增、暂无读者：编码器/生成物一行不动）：
      `dsl/field_decl.rs`（`FieldDecl` 类型 + 解析 + `to_bitfield` + `lower_bitfields` +
      `bind_operands`），8 条单测全绿。**关键证据**：新语法对 riscv 散射**逐段等价**于既有 `pieces`
      （`imm_b` = `i13[12:1 -> 11:8,30:25,7,31]` ↔ `[{31,1,12},{25,6,5},{8,4,1},{7,1,11}]`；
      `imm_j` = `i21[20:1 -> 30:21,20,19:12,31]` ↔ `[{31,1,20},{21,10,1},{20,1,11},{12,8,12}]`）。
- [x] **语法扩展决策（需评审）** 已拍板（连续位域 + `fields` 就地声明；落地见 §14）：你的语法里 `[begin:end]` 是**连续**位域；riscv 的 S/B/J
      是**散射**且值自身 bit 0 不参与（`off_b` 无 `unit`，见下 §4.2 风险 3），连续形式表达不了。
      W2 补了最小扩展：`type[值位段 -> 词位块, …]`（值位段低位在前，词位块按值低位→高位放置）。
      **若你要别的写法（例如保持单括号只写词位、值偏移另开一个键），现在改代价最小。**
- [x] `dsl/model.rs`：接线（`[[forms]].fields` / `[[instructions]].fields` + `bind`），
      内部 `bitfields`/`opcode_field`/`operand_fields` 改 `#[serde(skip)]` 派生。
- [x] 解析与诊断：位区间语法错误、类型不存在、宽度不符、`ops` 引用未知槽名 —— 报错点名
      `[[forms.<名>]].fields[i] = "<原文>"` 与原因（TOML 层无 span，与既有诊断口径一致）。
- [x] lowering pass（§4.0）+ 绑定规则：**`ops` 列出的槽 = 操作数槽；其余 = 常量槽**（D-1/D-3）。
- [x] `match` 覆盖（常量槽换值 / 操作数槽钉成常量）；`bind` 改名（D-4）。
- [x] 位段相交的逐指令校验（同一条指令不得同时绑定相交两段）。
- [x] 删除 `[conventions.bitfields]` / `opcode_field` / `operand_fields` 的用户面（model 键、schema、docs）。
- [x] 三谱迁移（脚本 + 人工复核，见 §4.2）：riscv、arm64 全量；x86 无 bitfields 节 ⇒ 只跟着 `match` 改名。
- [x] `lint --bits` 口径确认：未被字段覆盖的位仍按现状报出（D-6）。
- [x] 门禁：§2 全绿；`spec_coverage_guard` 三谱指令数不变；`LINT-BITFIELD-OVERLAP` 不再需要例外。

### 4.2 迁移算法与已知风险

算法（逐谱）：① 读 `[conventions.bitfields]` 得"字段名 → 位区间"；② 对每个 form，
把 `opcode_field` + `operand_fields`（**去重**）+ 用该 form 的指令 `match` 里出现的字段名
合成该 form 的 `fields` 列表（默认值统一 0，值仍走各指令的 `match`；`opcode` 若全族同值则写进默认）；
③ 指令保留 `match`，删掉 `opcode`（若已进 form 默认）。

风险（按严重度）：

1. **arm64 `operand_fields` 的重复占位**（`["rt","rn","rm","vq","vq","vq"]`）：三个位置绑同一字段，
   新语法按**名字**绑定 ⇒ 必须改用 `bind = { <操作数名> = "vq" }`，且**逐条重新验证编码字节**。
2. **指令级常量**（arm64 `one21 = 1`、`rm = 0` 等数百处）：留在 `match`，但字段必须在 form 的
   `fields` 里被声明（否则报未知字段）——合成时要保证不漏。
3. **散射方向**（riscv S/B/J）：按"值低位 → 高位"重排，必须与既有 `pieces.shift` 逐段对拍。
   ⚠ 实测发现（2026-10-08，`isa/riscv64.toml:159-164`）：`imm_b`/`imm_j` 的 `pieces.shift`
   **不是从 0 连续开始**（`imm_b` 的 shift 集合 = {1, 5, 11, 12}，bit 0 故意空着；`imm_j`
   = {1, 11, 12, 20}）——低位那条对应"偏移量的 bit 0 恒 0（2 字节对齐）"。
   因此"值序 = 从 bit 0 起"的新约定**不能直接**等价表达它，必须先定清：
   字段承载的是**原始字段值**（= 源值 >> log2(`unit`)）还是**源值**，并用槽的 `unit`
   （B 型 = 2 或 4）把那个隐含零位吸收掉。**这一步没定清之前不要动 riscv 的 S/B/J。**
   注意 `operand_fields` 里写的是**位域名**（`imm_b`），与操作数槽名（`target:off13` 之类）
   是两件事——迁移时别把两者混为一谈。
4. **`opcode_field = "word"`（整字常量，如 NOP/RET）**：整字字段与其它位段重叠 ⇒ 依赖
   "同一条指令不得同时绑定相交两段"的校验口径（D-6 的逐指令视图）。

### 4.3 实测：收尾工作量在**夹具**，不在三谱（2026-10-08）

W2.2b 主体已跑通一遍（接线 + 迁移 + 打印器），**但未提交并已整体撤回**——当前 `serde(skip)`
会让所有仍写旧键的 TOML 解析失败，实测波及的是**夹具**而非发行谱：

1. **三谱迁移很快**：打印器（对 `form_field_decls` 取文本）一次产出并改写
   **riscv64 12 forms + arm64 38 forms = 50 个 form**，`[conventions.bitfields]` 整节删除；
   算法已被 a8c7e6e 的单测证明等价 ⇒ 产物不可能偏离。
2. **大头是夹具**：`diag_matrix_tests.rs`（56 处内联谱）、`dsl/tests.rs`（约 28 处）、
   `dsl/template_tests.rs`（5 处）、`src/lint.rs` 测试夹具（4 处）、
   `crates/backend/forge-codegen/tests/isa/*.toml`（6 份）、
   `examples/isa-host-demo/isa/toy16.toml`、`tests/*.rs` 里的内联谱。
3. **打印器跑两遍**：第一遍发行谱（已做），第二遍把夹具谱也加进 `include_str!` 列表。
4. **可行的操作顺序**（已实测）：临时把三处 `serde(skip)` 还原成 `serde(default)`
   → 跑打印器 → 脚本改写全部 TOML（发行谱 + 夹具）→ 恢复 `skip`
   → 门禁（`isa_roundtrip_guard`/`spec_coverage_guard`/asm 三档棘轮/双矩阵逐数字不变）。
5. `lint_shipped` 的 `LINT-BITFIELD-OVERLAP` 例外随旧键消失而一并撤除。
6. **⚠ 第二次实测发现的缺口（已定口径解决）**：迁移**不只是** form 的 `fields`——旧模型靠
   **位置**绑定（`operand_fields[i]` ↔ 第 i 个操作数），新语法按**名字**绑定；而发行谱里
   操作数名与位域名本来就不同（riscv：`ops = ["dst:gpr:out","src:gpr"]` ↔ 字段 `rd`/`rs1`）。
   **口径 A（2026-10-08 定）**：① `bind` 显式优先（写错字段名报错，不回退）；② 否则同名；
   ③ 否则**位置回退**＝按 form 字段声明序取第 i 个（跳过 `opcode`）——即 v18 `operand_fields`
   的口径。⇒ 迁移时**零 `bind`**，76 个 form 改写即可（已实测）。
   实现与单测：`field_decl::bind_operands`（`positional_fallback_matches_v18_operand_fields`）。
7. **仍需显式 `bind` 的例外**：位置回退表达不了"**多个操作数绑同一个字段**"——
   arm64 有 `operand_fields = ["rt","rn","rm","vq","vq","vq"]` 这类三连（向量排列）。
   ⇒ 收尾时先跑一次"位置回退 vs 旧 `operand_fields`"的对拍，**只给对不上的那几条指令**
   写 `bind = { … = "vq" }`（预计是个位数），其余一律不写。
8. **⚠ 第三次对拍的决定性发现（必须先解，否则迁移必错）**：
   `bind_operands` 现在按"字段名 ≠ `opcode`"排除主 opcode 字段，**arm64 不成立**——
   它的 form 用 **10 种**名字：`op8`(20) `op9`(2) `op6` `op7` `mtop`(5) `word` `vec_a`(3)
   `b16`(2) `cbop`(2) `adr_fix`。⇒ 位置回退整体错一位，**arm64 537/537 条全部对不上**
   （riscv64 **0 条**，因为它的 opcode 字段就叫 `opcode`）。
   修法两条，且**必须一起做**：
   - `bind_operands` 接收"本 form 的 opcode 字段名"而不是硬编码 `"opcode"`；
   - `lower_field_syntax` 判定 `opcode_field` 时也不能只看"名字叫 opcode"的字段。
9. **由此暴露的设计点（v21 真正要改的地方）**：v18 的 `[conventions.bitfields]` 是
   **全局命名空间**，而 v21 的 `fields` 是 **form 局部**的。arm64 多个 form 各自需要
   一个"opcode"字段（位区间还不同：`op8` 与 `op9` 就不同）⇒ 全局合并必然报
   "字段 `opcode` 在多个 form 里声明不一致"。**W2 的降级方案必须让内部位域表按 form 分区**
   （或给内部键加 form 前缀），否则只能保留 `op8`/`op9` 这类 ISA 私有名——那样
   "字段名就是接口名"的可读性目标就打折了。**这一步是 W2 里唯一需要动 codegen 的地方**，
   也是它值得单列一片（W2.5）的原因。

---

## 5. W3 操作数层收敛

> ✅ **W3a 已完成并提交（2026-10-08）**：`[[operand_slots]]` → **`[operand.<名字>]` 表** + 键收敛。
>
> - **结构**：键名即槽名（`[operand.csr12]`），`IsaModel.operand_slots` 改 `#[serde(skip)]`
>   派生字段，由 `operand_decl::lower_operand_layer` 降级而来——**编码器/生成物一行未改**。
> - **键收敛（8 个一次性开关 → 正交键）**：`width → bits`、`min`+`max → range = [lo, hi]`、
>   `float → value = "float"`、`wrap → literal = "bits"`、`table`/`names → enum`、
>   `arrangement → suffix`、`symbols`/`require_symbol`/`imm_fns → symbol = { allow, require, modifiers }`、
>   `zr31 → zero`/`sp`；`[conventions.mem].templates/size_keywords → [operand.mem].text/size_words`。
> - **`byte_reg` 不挂寄存器组**（设计文档那条假设被实证推翻）：x86 `setcc` 的 rm 用 64 位名字
>   （`class = "gpr64"`）却按 8 位寄存器编码 ⇒ 是**槽**的事实，落成槽级 `byte = true`。
> - **一处顺序修复**：命名位集合/命名立即数的**摊平**原本跑在 `parse()` 里，而槽现在是降级产物
>   ⇒ 改到操作数降级之后（否则 `table_entries`/`name_entries` 恒空）。
> - **诊断**：槽相关文案从 `[[operand_slots]] #i ('名')` 改为 `[operand.名]`；imm 槽的 `enum`
>   指到位集合表时**点名**（"这是位集合表，kind = bits 用"），不再只报"未定义的表"。
> - **迁移**：三谱 + 夹具谱 + ~90 处内联夹具 + 教程 TOY16 谱；实测
>   `cargo test --workspace --exclude forge-rustc` → **exit 0、0 编译错误、0 失败二进制**
>   （含 `isa_roundtrip_guard` 字节闭环、asm 三档棘轮、x86/riscv64 双矩阵）。
>
> ✅ **W3b 也已完成（2026-10-08）**：`[enum.<表名>]` 统一了原先的三张表——
> `[conventions.cond]`（条件码，条目 `{ code, ir }`）、`[conventions.bitsets.*]`（`kind = "bits"`）、
> `[conventions.imm_names.*]`（值表）。判定规则：表名叫 `cond` **或**条目带 `ir` ⇒ 条件码表
> （内部只一处，两张即报错）；`kind = "bits"` ⇒ 位集合表；否则值表。槽侧照旧 `enum = "<表名>"`。
> 三谱 + 夹具（含内联形态）已迁移；门禁同 W3a（workspace exit 0、0 编译错误、0 失败二进制）。
> 🚧 **W4 进行中（2026-10-08）**：段模型（x86 那一族变长编码）分两步走。
> ✅ **W4.0 段模型骨架已落地**：`form.segments = [{ kind = …, fields/bytes/value }, …]`
> ——有序段 = 发射序；段 kind 是**闭集**（`prefix`/`escape`/`rex`/`opcode`/`opcode_reg`/
> `modrm`/`sib`/`disp`/`imm`/`vex`/`evex`）；段内位域用 **W2 的同一套语法**声明，并进
> `form.fields` 后仍由 `field_decl` 一处展开。降级覆盖 `prefix`/`escape`/`rex`/`opcode`/
> `modrm`/`imm`（其余段 **fail-closed 报错**，不静默）。等价性有单测钉住：段写法与经典语义键
> 写法降级到**同一份** `EncKeys`。
> **两处偏离（需评审）**：① 段用**内联表数组**而不是"`segments = [名字…]` + 命名子表"——
> `[[forms]]` 是数组元素，子表路径在 TOML 里有二义性；内联表让"顺序即发射序"结构化，少一处重复。
> ② REX 的 `0x40` 基值**由段 kind 蕴含**，谱里不再写 `u4[3:0]=0x4`（无名常量槽不在 W2 支持面内，
> 且"这堆字节是 REX"本就是结构事实）。
> **已记录的 W4.0 限制**：经典校验要求 `prefix = "opsize"` **单独出现**，所以"同时有 opsize
> 与其它前缀"的 form 现在还降不过去；W4 的"前缀表按 form 显式化"会取代该限制。
> ✅ **W4.1 已完成（2026-10-08）**：段 kind **闭集全部实现降级**——`opcode_reg`（要 `value`）
> 与 `vex`/`evex`（头字段来源键与 `[forms].vex` 同组，降级原样搬进 `EncKeys`）；`sib`/`disp` 是
> **布局段**（无对应语义键——字节由编码器按内存操作数派生），价值在"把字节布局写进谱并进校验"。
> 空 VEX 头、缺 `value` 的 `opcode_reg` 都 fail-closed 报错。
> ✅ **W4.2 已完成（2026-10-08）**：前缀**效果名闭集**校验 + 降级口径对齐。
> 效果名以 `PrefixEffect::NAMES`（`opsize16`/`lock`/`repe`/`repne`/`addr32`/`addr16`/`rex`/`rex2`）
> 为**唯一来源**——写错在声明期报错并列出全集（此前垃圾名会静默通过）；段里的 `opsize16`
> 降级到经典键的 `"opsize"`（经典口径），单来源写 `One`、多来源写 `Many`（与序列化往返一致）。
> **顺序更正**：原先计划的"W4.2 删全局前缀表"排在迁谱**之前**是错的——发行 x86 谱的
> `prefix = ["0x…"]` **只有字节、没有效果**，字节→效果的知识目前在全局表里；先删会丢知识。
> 正确顺序：**先迁谱（W4.3）**（段携带"字节 = 效果"）→ **再删全局表 / 逐 form 条件（W4.4）**。
> 🚧 **W4.3 进行中（2026-10-08）**：形态采 **A（保留 form 作段载体，按段骨架重新分组指令）**。
> ✅ **batch 1 第 1 步已完成**：**变长 ISA 的 `bind` 落到 `modrm`**——`bind = { reg = "dst", rm = "src" }`
> 在 `prefix_scan` ISA 上写进 `ModrmMap`（此前 vlen 完全不读 `bind`，ModRM 的角色只能靠位置缺省）。
> 扩展码（`/4` 这类**整数**）不在 `bind` 的表达面内，仍写 `modrm = { reg = 4 }`——已在注释与清单注明。
> ✅ **batch 1 第 1.5 步（实测修正）**：段里声明 `modrm` **不再**强设 `keys.modrm`。
> 实测 `MOV_R_RM` 的生效键只有 `{opsize="s0"}`（没有 `modrm`）而字节正确 ⇒ vlen 编码器
> **由操作数驱动**发射 ModRM；强设默认 `ModrmMap` 会把"操作数驱动"改成"键驱动"（行为改变）。
> 段的职责是声明**字节布局**（`mod`/`reg`/`rm` 进 `form.fields`，供 `bind`/`match`/`--bits` 用）。
> ⛔ **batch 1 第 2 步受阻（设计结论，需评审）**：vlen 的 opcode 值来自**专用标量**
> `Instruction.opcode`（`model.rs` 的字段 + `codegen/vlen.rs` 9 处 `info.inst.opcode.unwrap()`），
> 而 W2 的 `match` 是**定宽位域**机制、vlen 不读它 ⇒ **共用 form 装不下 194 个不同的 opcode 值**。
> 于是"保留 form 作段载体（方案 A）"在 x86 上**不成立**：要么让指令能给**段字段赋值**
> （= `match` 推广到 stream，并让 vlen 编码器改读段字段值——动编码器、需 200 条字节对拍），
> 要么承认段模型对 x86 现有编码面**不适用**、W4 停在"模型可用但 x86 不迁"。
> 评审结论（2026-10-08）：**选 (a)**。落地方式不需要动编码器——见下。
> ✅ **batch 1 第 2 步已落地**：**`match` → 经典标量翻译**。stream form 声明段布局，
> 指令用 `match = { opcode = 0x8B }` 给段字段赋值，降级把 `match.opcode` 翻译成
> `Instruction.opcode`（编码器读的那个标量）⇒ **编码器一行不改，字节按构造不变**。
> 同片修掉两个"vlen 被塞定宽键"的地雷（`opcode_field` 是 `validate` 判 `is_fixed` 的
> 依据、`operand_fields` 是定宽位域索引）：现在 vlen 的 form 即便带字段也不写
> `opcode_field`，vlen 的 `bind` 只写 `modrm`、不写 `operand_fields`。
> 实测（`forge-isa insts` 一份段形态谱）：`enc={modrm={ reg = "dst", rm = "src" }}`、
> `opcode=0x8b`——没有 `opcode_field`/`operand_fields` 混入。
> ⛔ **batch 1 第 3 步受阻（生成期崩溃，已回退）**：把 14 条核心 RR 指令迁到段骨架 form 后，
> `validate` 通过、`forge-isa insts` 的生效键也对（`form=RR_SEG`、`opcode=0x8b`），但
> **`forge-codegen` 的预生成器（build script）以 `STATUS_ACCESS_VIOLATION` 退出**——
> 生成期崩溃，不是测试失败。已 `git checkout` 回退 `isa/x86.toml`，回退后 `forge-codegen`
> 与 `forge-isa-dsl` 两套全绿。
> ✅ **更正（2026-10-08，重做后）**：`STATUS_ACCESS_VIOLATION` **不是**段模型与 x86 不兼容，
> 也不（至少不直接）是生成器缺陷——它是被**我自己的畸形迁移文件**污染出来的：
> 旧迁移脚本用 `Split` + 字符串拼接重组文件，给第 2 个起的 `[[instructions]]` 头**插了一个空格**
> （`(?m)^` 只数到 1 节）。重写成**纯行内替换**后：`cargo build -p forge-codegen`
> **连跑 3 次全 exit 0**；工作区 `cargo test` 也确实崩，但**发生在一个真失败之后**
> （`isa_roundtrip_guard` FAILED），是与失败同现的次生现象。
> ✅ **已解决（2026-10-08）**：根因是我的 W4 降级 **整体覆写** form 的键（`form.keys = keys;`）——
> 把 form 自己写的 `opsize = "out"` 冲掉了，于是被迁指令丢失 opsize，解码按错误宽度裁决
> （`MOV64_RR` 的 `48 89 C1` 被解成 32 位）。改成 **段键覆盖、既有键兜底**
> （`keys.over(&form.keys)`）后：单条迁移的 `isa_roundtrip_guard` 立刻转绿，
> 14 条全迁 + `cargo test --workspace --exclude forge-rustc` → **exit 0、0 编译错误、0 失败**。
> ⇒ **W4.3 batch 1 完成**：14 条核心 RR 指令（MOV/ADD/SUB/XOR/AND/OR/CMP/TEST/ADC/SBB…）
> 已迁到段骨架 form `RR_SEG`（`opcode` + `modrm` 段），指令侧只写 `match = { opcode = X }`。
> ✅ **batch 2 完成（2026-10-08）**：`MRR_MEMREF_AUTO` 的**干净子集 13 条**迁到段形态 form
> `MRR_MEM_SEG`。同片定下**扩展码的表达**（此前悬着的问题）：`modrm = { reg = 2, rm = "[mem]" }`
> → **`match = { opcode = 0xF7, reg = 2 }` + `bind = { rm = "[mem]" }`**——
> 分工是「**常量进 `match`（整数）、操作数绑定进 `bind`（名字）**」，`match.reg` 降级成
> `Modrm.reg = Ext(n)`。生效键实测：`NOT_MEM32` 的 `enc` 里 `modrm={ reg = 2, rm = "[mem]" }` + `opsize=32`。
> 累计已段化 **27 条**（batch1 14 + batch2 13）。
> ✅ **batch 3 完成（2026-10-08）**：`MRR_MEMREF_AUTO` 的 **`escape` 子集 8 条**（IMUL_R_MEM、
> MOVZX/MOVSX_MEM8/16、NOP_MEM32、SETCC_R_MEM、CMOVCC_R_MEM）迁到段形态 form `MRR_MEM_0F_SEG`
> （骨架 = `escape` + `opcode` + `modrm` 三段；`escape` 是**结构事实** ⇒ 由 form 承载，
> 指令侧只留 `match`/`bind`）。累计已段化 **35 条**（batch1 14 + batch2 13 + batch3 8）。
> ⚠️ 迁移脚本漏了 `CMOVCC_R_MEM`（判据与它的键集有一处不吻合），已手工按同模板补上——
> **每批都要回读条数并逐条抽查生效键**，不能只看脚本自报。
> ✅ **batch 4 完成（2026-10-08）**：`MRR_EXT` **19 条**按 `imm` 宽度拆成两个骨架迁走——
> `MRR_EXT_IMM32_SEG`（8 条，继承 form 的 `imm = 32`）与 `MRR_EXT_IMM8_SEG`（11 条，内联 `imm = 8`）。
> 这是第一次用 **`imm` 段**：宽度是**结构事实** ⇒ 进 form 的段（`{ kind = "imm", fields = ["u32[31:0]:imm32"] }`
> 降级成 `keys.imm = 32`），指令侧的内联 `imm = N` 随之删掉。生效键实测：
> `ADD_R_IMM8S → enc={imm=8, modrm={ reg = 0, rm = "dst" }, opsize="s0"}`、`ADD_R_IMM32 → imm=32`。
> 累计已段化 **54 条**（14 + 13 + 8 + 19）。
> ✅ **batch 5 完成（2026-10-08）**：**`SSE_RR` 21 条**迁到 `SSE_RR_SEG`——段化"最实"的一组：
> SSE 的 66/F2/F3 前缀与 REX.W 本来就是**段字段**（指令侧早已写 `match = { prefix = 0xF2, w = 0 }`）。
> 同片为段模型补两项能力：① **前缀段取字段**（`{ kind = "prefix", fields = [...] }` ⇒ 经典
> `prefix = "field"`），与"字节 → 效果名"的常量形态并存；② **`rex_w` 来自字段**（rex 段的 `w`
> 字段**无默认值** ⇒ 经典 `rex_w = "field"`；带默认值 ⇒ 固定 0/1）。
> 另修一处 lint 误报：段内字段各自属于**自己的字节空间**，不按"同一个字里的位"判重叠
> （`prefix`[0,8) 与 `w`[7,8) 曾被判成抢同一批位）——段字段现在从重叠判定里排除。
> 累计已段化 **75 条**（14+13+8+19+21）。
> ✅ **batch 6 完成（2026-10-08）**：两组一起迁——`MRR_EXT_OP` 的 8 条（无 escape）→ `MRR_EXT_OP_SEG`
> （opcode + modrm(/ext)）、`MRR_0F` 的 8 条 → `MRR_0F_SEG`（0F escape + opcode + modrm）；
> 零引用的 `MRR_0F` form 已删（lint 是权威判据）。带 escape 的 `NOP_RM` 留在原 form，下批处理。
> 累计已段化 **91 条**（14+13+8+19+21+16）。
> **迁移脚本的两处教训（都真实踩到）**：
> ① **行尾注释会让正则落空**——`opcode = 0x40    # 条件码…` 这种写法使 `^opcode = X$` 不匹配，
> 于是 `CMOVCC_R_RM` 被静默跳过（batch 3 的 `CMOVCC_R_MEM` 同因）。判据一律写成
> `^键 = 值\s*(#.*)?$`，且**每批回读条数**不能省。
> ② 迁移后**老的 form 可能变成零引用** ⇒ lint 的 `LINT-UNUSED-FORM` 会红；按 lint 逐条删掉即可
> （validate + lint 干净再进守卫）。
> ✅ **batch 7 完成（2026-10-08）**：六个小组共 **21 条**迁走——`MRR_MEM_PRE`(4)、`MRR_MEM`(3)、
> `MRR_FIX64`(3)、`MRR_MEMREF`(5)、`MRR`(3)、`MRR_EXT_OP_FIX64`(3)；各自一个新段形态 form
> （`MRR_MEM_PRE_SEG` / `MRR_MEM_SEG` / `MRR_FIX64_SEG` / `MRR_MEMREF_SEG` / `MRR_SHIFT_SEG` /
> `MRR_EXT_OP_F64_SEG`），并删掉 6 个零引用旧 form。
> 累计已段化 **112 条**（14+13+8+19+21+16+21）。
> **本片又踩到两个脚本坑（都已修）**：
> ① 生成 form 时漏引号（`opsize = s0`）⇒ TOML 解析失败；值一律按字符串写。
> ② 指令**本就有 `match = { prefix = 0xF0 }`**，脚本又加一行 `match` ⇒ 同块重复键；
> 已加通用修复：同块多个 `match` 自动合并成一行。
> ✅ **batch 8 完成（2026-10-08）**：**10 条**迁走——`MRR_MEM_0F_FIX32`(6)→`MRR_MEM_0F_PFX32_SEG`、
> `MRR_0F_PREFIX`(3)→`MRR_0F_PFX_SEG`、`NOP_RM`(1)→`MRR_EXT_OP_0F_SEG`；两个零引用旧 form 已删。
> `MRR_0F_PREFIX` 的处理按计划：**只声明 prefix/escape/opcode/modrm 段**，`w` 不进 rex 段，
> 因此 `match.w` 维持原状（REX.W 语义不变）。
> 累计已段化 **122 条**（14+13+8+19+21+16+21+10）。
> ⚠️ **两条 LOCK 原子指令（`ACQUIRE_LOCK_ADD_MR`/`RELEASE_LOCK_ADD_MR`）暂不迁**：
> 它们写 `prefix = ["0xF2","0xF0"]`——**有序两字节**，而段形态的前缀字典按字节**字典序**排
> （`0xF0` 会排到 `0xF2` 前），发射序会反。要迁需要给前缀段加**有序列表**形态（下一片给方案）。
> ✅ **batch 9 完成（2026-10-08）**：**VEX/EVEX 五组共 21 条**迁走——`VEX_RRV`(6)、`VEX_RR`(3)、
> `VEX_RR_MEMREF`(5)、`EVEX_RRV`(2)、`EVEX_RR_MEMREF`(5)；三个零引用旧 form 已删。
> **段模型的 `vex`/`evex` 两个 arm 首次被真实谱走到**：`{ kind = "vex", map/pp/w/l = "field" }`
> （EVEX 另加 `disp_scale = "field"`）——头字段的来源键与旧 form 的 `vex = {…}` 完全一致，
> 值仍由指令的 `match = { vex_map = … }` 给。生效键实测：
> `VADDPS_ZMM_MASK` 的 `enc` 里 `evex={disp_scale="field",l="field",map="field",pp="field",w="field"}`、
> `modrm={reg="dst",rm="src2"}`
> 同片修一处 vlen 误用定宽路径：`lower_field_syntax` 的**位置绑定**（`bind_operands`，要求每个
> 操作数都能找到字段）现在对 vlen **整体跳过**——段字段是各段自己的字节布局，硬跑会报
> "操作数 src3 找不到字段"。
> 累计已段化 **143 条**（…+21）。
> ✅ **batch 10 完成（2026-10-08）**：**前缀段加了有序字节列表形态**（`bytes = ["0xF2","0xF0"]`，
> 发射序 = 写序），两条 LOCK 原子指令（`ACQUIRE_LOCK_ADD_MR`/`RELEASE_LOCK_ADD_MR`）迁到
> `MRR_MEM_XACQ_SEG`/`MRR_MEM_XREL_SEG`。生效键实测 `prefix=["0xF2","0xF0"]`（顺序保住）。
> 前缀段至此有**三种形态**：① 字节→效果名字典（常量 + 语义）；② `fields`（字节由字段给，
> 值在指令 `match` 里）；③ 有序字节列表（多前缀、顺序敏感）。
> 累计已段化 **145 条**（…+2）。
> ✅ **batch 11 完成（2026-10-08）**：最后一组 form 迁移完毕——`MRR_0F_FIX64` 的 5 条
> （MOVQ 系列）迁到 `MRR_0F_F64_PFX_SEG`；该旧 form 已删。
> 注意：它的 form **没有** `rex_w = "field"` ⇒ 新 form **不带 rex 段**（否则 `match.w` 会变成
> 真字段、把 `rex_w` 从 Auto 改成 Field，语义就变了）。
> **W4.3 的 form 面到此收尾**：`isa/x86.toml` 里**所有 form 都是段形态**（无 `_SEG` 以外者）。
> ⚠️ **剩余 50 条是"无 form 的指令"**（把编码键全写在指令上：`opcode`/`modrm`/`escape`/`prefix`/`opsize`）。
> 它们**迁不进段**，因为 `segments` 目前只挂在 **form** 上（指令没有段载体）——这正是当初 A/B 分叉里的
> **选项 B：给指令加段载体**。做法（下一片）：`Instruction.segments`（**只放结构段**：escape/prefix/modrm，
> 段内不给字段；opcode 值继续用指令的 `opcode` 标量），降级复用同一套 arm；
> 需要时放松 `opcode` 段的"必须有 `opcode` 字段"要求（vlen 的值本来就不来自字段）。
> 进度：**150/200 段化（全部 form 面）+ 50 条无 form 指令**。
> ✅ **batch 12a 完成（2026-10-08）**：**指令级段载体**（选项 B）已实现——`[[instructions]].segments`
> 只放**结构段**（`escape`/`prefix`/`modrm`，段内不给字段；其余 kind 明确报错并让你改用 form）；
> `opcode` 值继续走指令的 `opcode` 标量，扩展码/常量走 `match`，操作数绑定走 `bind`
> （无 form 的指令现在也会应用 `bind` → `modrm` 的翻译）。前缀解析**一处实现**（form 与指令级共用
> `prefix_key`）。单测：指令级段 ≡ 经典内联键；`vex` 段在指令级被拒。
> ⚠️ **batch 12b 首次尝试失败并已回退（2026-10-08）**——44 条无 form 指令（含结构键的 32 条）
> 的批量迁移踩到三个**结构性障碍**，已全部回退（`isa/x86.toml` 回到 batch 11 状态，workspace 全绿）：
> ① **`bytes = [...]` 不是段专属键**——`[[vectors]]` 也用同名键（语料向量表）。任何**全局文本后处理**
> 都会误伤向量表（实测把向量表打坏，parse error 落在向量节）。⇒ 迁移必须**限定在 `[[instructions]]`
> 块内**替换，禁止全文件正则。
> ② **行尾注释会静默漏迁**：脚本自报"迁移 21（期望 32）"——判据正则必须写 `\s*(#.*)?$`，
> 且**迁移数必须与普查数断言相等**，不等就中止（先普查、后迁移）。
> ③ **字节字面量的写法不同**：经典谱写 `escape = [0x0F]`（**无引号**），段形态要求**字符串**
> （`bytes = ["0x0F"]`）⇒ 必须在**生成时**加引号，不能事后全局补（会撞上 ① 的向量表）。
> ⚠️ **batch 12b chunk1 也失败并已回退（2026-10-08）**——**根因**：脚本在**原行列表上就地改索引**
> （先把 `segments` 插到 `$slot`，它可能位于 `modrm` 行**之前**；再用**过期的** `$ml + 1` 插 `bind`）
> ⇒ `bind` 落错位置、`modrm` 绑定丢失：`MOVZX_R8_MEM` 的生效键从
> `{escape, modrm={rm="[src]"}, opsize}` 退化成 `{escape, opsize}`。
> **而 `validate` 依然通过**（谱仍合法，只是语义变了）——真正抓到它的是**生效键抽查**
> 与随后 JIT 矩阵的 `0xC000001D`（非法指令）。
> ⇒ **两条硬规矩**：① 每个指令块必须**整块重建成新的行列表再替换**，**禁止**就地改索引
> （插入会使其后行号全部失效）；② **`validate` 通过不算数**，每块必须跑**生效键抽查**
> （`forge-isa insts` 比对迁移前后 `enc`）**加**三条守卫。
> ✅ **chunk 1 成功（2026-10-08）**：按新规矩（**整块重建** + 生效键前后比对）迁了 5 条无 form 指令
> （`MOVZX_R8_MEM`/`MOVZX_R16_MEM`/`CMPXCHG_MEM_R`/`CMPXCHG_MEM_R_8`/`XADD_MEM_R`）。
> 生效键实测与迁移前一致（`MOVZX_R8_MEM` 的 `enc={escape=[15],modrm={rm="[src]"},opsize="s0"}`）；
> `prefix = "field"` + `match = { prefix = 0xF0 }` 在迁移时**折叠成常量** `bytes = ["0xF0"]`（字节等价）。
> 同片修掉降级里的**早退 bug**：`lower_stream_instructions` 里 `let Some(mt) = inst.fields else { continue }`
> 让"只有 segments + bind、没有 match"的指令跳过整个函数 ⇒ `modrm` 绑定丢失（`validate` 不报）。
> 现在没有 `match` 也继续走 `bind` 分支。**这正是"validate 通过不算数"那条规矩抓到的第二例。**
> 进度：**155/200**（150 form 面 + 5 指令面）。
> ✅ **chunk 2 成功（2026-10-08）**：6 条无 form 指令迁入（`MOV_R_MEM_8`/`STORE_MEM_R_8`/`UD2`/
> `SETCC_RM8`/`SETCC_RM8_B`/`CALL_RM`）——**生效键前后逐条一致**，三条守卫全绿。
> 进度：**161/200**（150 + 5 + 6）。
> ✅ **batch 12b 收尾成功（2026-10-08，一次提交）**：剩余 **20 条**一次做完——
> **Group A 7 条**走**指令级结构段**（`MOVSD_RM`/`MOVSD_MR`/`MOVUPS_RM`/`MOVUPS_MR`/`PUSHFW`/`POPFW`/`PMULLD`）；
> **Group B 13 条**按骨架建 **5 个段形态 form**（`SSE_IMM_0F_SEG`/`SSE_IMM_0F3A_SEG`/`JCC_REL32_SEG`/
> `VEX_IMM_SEG`/`EVEX_IMM_SEG`，含 `imm`/`rex.w`/`vex`/`evex` 段）。
> 生效键抽查与迁移前**逐条一致**（如 `ROUNDSS_I → enc={escape=[15,58],imm=8,modrm={reg="dst",rm="src"},prefix="field",rex="auto",rex_w="field"}`）；
> `prefix = "field"` 仍按常量折叠（`PUSHFW → prefix="0x66"`）。
> **`MFENCE` 不迁**（它用 `modrm_fixed = 0xF0`，段模型没有对应等价物——不动它比硬造一个安全）。
> 进度：**181 / 194 段化**（其余 13 条 = MFENCE + 12 条无 `opcode` 的别名/伪指令）。
> **脚本两条新教训**：① 指令块边界必须取**下一个任意 `[[` 段**（只认 `[[instructions]]` 会把中间的
> `[[pseudo]]` 吞进块里，插入点落到伪指令上——实测污染过一次）；② `imm` 段的字段名要**带宽度**
> （`imm8`/`imm32`），叫 `imm` 会与他人撞名并被 W2 的 form 局部命名空间限定成 `SEGxxx.imm`，
> 于是被 `LINT-UNUSED-BITFIELD` 报"只声明没人用"。
> ✅ **W4.3 收尾完成（2026-10-08，一次提交）**：剩余 13 条一次迁完，并把**指令级段**扩到
> `opcode_reg`/`imm`/`rex`/`vex`/`evex`/`modrm`(固定值) 六种 kind：
> `PUSH`/`POP`/`MOV_REG_IMM64`/`MOV_R_IMM32`/`BSWAP_R`/`MOVABS_GLOBAL`（`opcode_reg`）、
> `RET_IMM16`/`RETF_IMM16`/`JMP_REL32`/`CALL_RIP_REL`（`imm`）、`CQO`（`rex`）、
> `VMOVAPS_ZMM`（`evex`）、`MFENCE`（`modrm` 的**固定字节** `value = 0xF0` ⇒ `modrm_fixed`）。
> 生效键抽查全部一致（`MFENCE → enc={escape=[15],modrm_fixed=240}`、`CQO → rex_w="always"`、
> `PUSH → opcode_reg=80` …）。**余下仅靠 `opcode` + 操作数驱动编码的 6 条**（如 `RET`/`NOP`/`PUSHF`）
> 本就无结构可声明 ⇒ **无需段**。
> ⇒ **W4.3 编码面迁移完成**：x86 谱里**凡有编码键的指令都已段化**。
> 🚧 **W4.4 第一步尝试（2026-10-08，已回退，树回到 `65cfcc9` 全绿）**：
> **守卫本身成功了**——`segment_prefixes_are_covered_by_the_scan_table` 当场抓到一处真问题：
> 有前缀段声明了**字节 `0x00`**（来自把经典 `match = { prefix = 0 }` 折叠成 `bytes = ["0x00"]`）。
> 经典口径里 `0` 表示**无前缀**，所以这条声明是错的（守卫判据正确）。
> **但我的修法错了**：全局删掉 `bytes = ["0x00"]` 那两个前缀段之后，门禁报
> `spec_movss_r_mem` 字节不符（`F3 0F 10 01` → `0F 10 01`，丢了 `0xF3` 前缀）——
> 说明"0x00 前缀段"与 `MOVSS_R_MEM` 的前缀**存在我尚未解释的耦合**（两者共用 form
> `MRR_MEM_0F_PFX32_SEG`，其前缀段是 `fields` 形态）。已整体回退。
> ✅ **定位完成并修好（2026-10-08）**：`0x00` 前缀段是**承重**的——`MOVUPS_RM`/`MOVUPS_MR`
> （prefix = 0，即"无前缀"）与 `MOVSS_R_MEM`（`0xF3`）共用 `0F 10 /r`，**解码靠前缀字节区分**；
> 删掉声明会让 `MOVUPS_*` 的匹配变宽 ⇒ 解码 `F3 D5 …` 落到 MOVUPS ⇒ 再编码丢 `F3`。
> 正解：两条改用**同骨架的既有 form** `MRR_MEM_0F_PFX32_SEG`（其前缀段是 `fields` 形态，
> 值仍走指令的 `match = { prefix = 0 }`）——`spec_movss` 由红转绿，全量门禁绿。
> **教训（第三次同源）**：段里"看起来是垃圾"的 `0x00` 其实是**解码区分位**；
> 段模型上"无前缀"要用 `fields` 形态表达，**不能**折成 `bytes = ["0x00"]`，也不能删。
> ✅ **W4.4 主体完成（2026-10-08，一次提交）**：
> ① **常驻守卫** `segment_prefixes_are_covered_by_the_scan_table`（三谱都跑）；
> ② **删掉内置缺省扫描表** `default_prefix_scan()`——生效表 = 谱声明的
> `[[conventions.prefix_scan]]`，**未声明即空表**（fail-closed：不再有"x86 风格兜底"，
> 前缀知识只在谱里）。`forge-isa-dsl` 的 1 处调用点 + 守卫助手同步；
> ③ 全量门禁通过（固定 ISA 不读该表，x86 自己声明了它，夹具也未依赖兜底）。
> ✅ **W4.4 尾巴的结论（2026-10-08，一次提交）**：读完 `vlen.rs` 后判定**主体不必动**——
> ① per-form 前缀条件**本来就有**（`vlen.rs:1986` 按 `form.prefix` 生成 `prefix_cond`）；
> ② 共享扫描循环（`gen_prefix_scan_loop`，`vlen.rs:2383`）由**生效表**驱动，而生效表已被我的守卫
> 保证"覆盖所有段声明的前缀字节"、未声明即空表（fail-closed）。
> 本片做的是**把守卫的覆盖面补齐**：新增 `match_prefix_bytes` ⇒ 字段形态的前缀**实际值**
> （`match = { prefix = 0xF3 }`，`0` = 无前缀要排除）也必须在生效表里——解码正是靠这些字节
> 区分指令（`MOVUPS_*`(0) vs `MOVSS_*`(0xF3)）。守卫现在覆盖三种前缀形态。
> **残留的整洁项（非行为问题）**：把"字节→效果"从谱的 `[[conventions.prefix_scan]]` 表
> **改成从段派生**——目前**不可行**，因为有序列表形态与字段形态都不携带效果名；
> 若要收敛，先给这两种形态加可选效果名，再让表可派生。届时守卫原样可用。
> ⏭ **下一片**：W5（族与重载）。
> **（旧记录）**W4.4 剩余：解码条件从"按字节枚举"改为**按各 form 的前缀段生成**（真正的解码器改动；
> 守卫与 fail-closed 都已在位）。**（旧记录）**下一步：先**直接**定位——打印所有"前缀段是 `bytes` 列表"的 form/指令，
> 以及 `MOVSS_R_MEM` 迁移前后的 `keys.prefix`；再改声明（**改声明、不改字节**），
> 每步都跑三条守卫。**教训**：守卫报警时不许做"顺手全局替换"，先定位耦合。
> ⚠️ **（旧记录）剩余 21 条里有一类需要 form**：带 `imm`/`rex_w`/`vex`/`evex` 的指令（`ROUNDSS_I`/`PSHUFD`/
> `SHUFPS`/`CMPPS`/`JCC_REL32`/`VEXTRACTF128`/`VINSERTF128`/`VINSERTF32X4` …）——指令级段
> **只收结构段**（escape/prefix/modrm），这些 kind 会被明确拒绝 ⇒ 要么给它们各自的 form，
> 要么把指令级段扩展到"带字段"（需要给指令加字段载体，成本更高）。下一片先定这个。
> ⏭ **batch 12b（重做计划）**：按**小块**推进（每块 5–8 条、一次提交），每块都用
> "块内替换 + 生成时加引号 + 数量断言"，并跑三条守卫；`[[vectors]]` 节**不碰**。
> （以下为旧计划）⏭ **W4.3 batch 11（下一片）**：清掉剩余零散组
> 形态）、以及若干零散组；之后 W4 的编码面迁移就收尾，可进 **W4.4**（删 `default_prefix_scan()`、
> 解码条件按 form 的前缀段生成）。
> `VEX_RR` 3、`EVEX_RRV` 2 …）——段模型里 `vex`/`evex` 两个 arm 将首次被真实谱走到；
> 外加前缀段的**有序列表**形态（解 LOCK 那两条）。
> `MRR_0F_PREFIX` 3 条（`match.w` 的归属要单独定，别让 `w` 误进 rex 段而改变 REX.W 语义）、
> `MRR_MEM_0F_FIX32` 6 条（prefix 字段 + 0F escape + opsize 32）、`NOP_RM` 1 条；
> 之后进 **VEX/EVEX**（`vex`/`evex` 段首次被真实谱使用）。
> `SSE_RR` 21（`escape`+`prefix`+`rex_w` 骨架，需把 `prefix`/`w` 变成段字段的 `match`）、
> `MRR_EXT` 19 / `MRR` 17（移位族留待扩展码问题定案）/ `VEX_*` / `EVEX_*`；
> 每批跑三条守卫（asm_encoding 棘轮 + encoder_fuzz + isa_roundtrip_guard）再提交。
> ~~真正的拦路虎（语义，不是崩溃）~~（已解决，保留作方法论记录）：迁移后 `x86 MOV64_RR` 的 REX.W 丢了——
> `isa_roundtrip_guard::derived_insts_roundtrip_byte_stable` 报
> `decode→encode 往返字节不一致：left [137,193]（89 C1）right [72,137,193]（48 89 C1）`。
> `MOV64_RR` 自己用 `MRR_FIX64`（`opsize = 64`）、**没被迁**，但它与被迁走的 `MOV_RM_R`
> **共用 opcode 0x89** ⇒ 解码 `48 89 C1` 现在落到 32 位那条。
> **下一步（下轮第一件事）**：查**降级顺序**——`inst.opcode` 由 `lower_stream_instructions`
> 写出，若有更早的 pass（vlen 解码表 / `collect_inst_infos` / 生成期去重）在读它，
> 被迁的指令在那一步就是"没有 opcode" ⇒ 解码表里少了 RR_SEG 那条。修法二选一：
> 把翻译**提前**（进 `parse`），或让解码表读段字段值。**判据仍是那三条守卫。**
> （以下为作废的旧记录）**二分结果（2026-10-08，三次全量 build）**：
> ① 只加 `RR_SEG` form（不迁指令）⇒ `cargo build -p forge-codegen` **exit 0**（段形态 form 本身无害）；
> ② form + 迁移 **1 条**（MOV_R_RM）⇒ **exit 0**；
> ③ form + 迁移 **14 条** ⇒ `STATUS_ACCESS_VIOLATION`。
> ⇒ 不是"机制不成立"，而是**随迁移条数出现**的问题（某条指令的组合，或与规模相关的
> 递归/栈/二次增长）。下一步：在 14 条上做二分（先 7 条），必要时把 `FGE_DEBUG_GEN=1`
> 的生成物按 parts 段比对，定位到具体生成函数。
> **复现收窄（2026-10-08）**：迁移后的 x86 谱副本走 `forge-isa test`（临时宿主，只取部分 parts）
> **不崩**（154 条向量、正常出宿主）；崩的是 `forge-codegen` 的 build script（**全量 parts**，
> 含 lowering/machine/decode）⇒ 崩溃点在**临时宿主不生成的那些 parts**里。
> 下一步是**用最小复现定位生成器**（建议：把该迁移应用到 `target/tmp/` 的一份副本 +
> `FGE_DEBUG_GEN=1`，或先用 CLI 的生成路径二分"段形态 form × vlen 指令键"的组合），
> 修好再重做第 3 步。可疑方向：段形态 form 与 vlen 指令键（`opsize`/`match`）共存时，
> 生成器里某处按"定宽/变长"二分的分支走进了不终止的路径。
> （原计划）⏭ **batch 1 第 3 步**：把 14 条核心 RR 指令迁到段骨架 form（`opcode` + `modrm` 段），
> 指令侧 `opcode = X` → `match = { opcode = X }`；对拍 asm_encoding 棘轮 + encoder_fuzz +
> roundtrip 三条守卫（它们本来就是"200 条指令逐字节不变"的守卫）。
> 迁到一个段骨架 form（`opcode` + `modrm` 段），指令侧 `opcode = X` → `match = { opcode = X }`；
> 移位族（SHL/SHR/SAR 带扩展位）留在原 `MRR`，不混读；对拍 asm_encoding 棘轮 + encoder_fuzz + roundtrip。
> （删 `default_prefix_scan()`）→ 再迁 x86 谱（20 forms / 200 insts / 18 templates；键用量
> `modrm 159 / vex 154 / evex 72 / prefix 114 / opsize 81 / escape 47 / imm 242`）。
**目标**：`[operand.*]` + `[enum.*]` + `[encode.*]`；8 个一次性开关 → 5 个正交键。

- [x] `kind`/`bits`/`signed`/`range`/`unit` 定稿；`min`+`max` 合一为 `range`。
- [x] `wrap → literal`、`float → value`、`table`+`names → enum`、`arrangement → suffix`。
- [x] `symbols`+`require_symbol`+`imm_fns → symbol { allow, require, modifiers }`。
- [x] `[conventions.cond]`/`bitsets`/`imm_names` → `[enum.*]`；`--bits` 的位集合语义保持。
- [x] `[conventions.mem].templates/size_keywords` → `[operand.mem].text/size_words`。
- [x] `zr31 → zero + sp`；`byte_reg → [reg.*].rex_required`。
- [x] 三谱迁移 + `enum` 生成脚本（riscv CSR 表由 `gen-riscv-csr-table.mjs` 产）改指向新节。
- [x] 门禁：§2 全绿；`abi check` 三谱结论与 W0 相同。

---

## 6. W4 stream 段模型

**目标**：x86 的 20 个 form 名与 37 处内联编码键，改成 `kind = "stream"` + 段。

- [x] 段 kind 闭集落地：`prefix`/`escape`/`rex`/`opcode`/`opcode_reg`/`modrm`/`sib`/`disp`/`imm`/`vex`/`evex`。
  （**2026-10-08 复核：11 种全部已接线** —— `stream_decl.rs` 的 `lower_one` 逐一处理；
  `sib`/`disp` 是**纯布局段**（无对应语义键：这两个字节由编码器按内存操作数派生，
  段的价值是把字节布局写进谱、让段内字段进 `form.fields` 参与声明期校验与 `--bits` 清点）。）
- [x] 段内 `fields` 复用 W2 语法；`segments` 序 = 发射序。
  （**2026-10-08 复核：已落地** —— `SegmentDecl.fields` 就是 W2 的 `Vec<FieldDecl>`
  （`field_decl.rs`）；`stream_decl.rs::lower_one` 按 `segments` 书写序逐段降级，
  三谱的 `[form.*].segments` 与指令级 `segments` 都按这个序发射。）
- [x] 前缀表显式化：**删除 `default_prefix_scan()`**；效果参数化（`opsize`/`addrsize`/`lock`/`rep`/`reg_ext`/`map`/`opaque`）。
  （**2026-10-08 复核：已落地** —— `IsaModel::prefix_scan_entries()` 只读**谱里声明的**
  `[[conventions.prefix_scan]]`（x86 声明了 66/F0/F2/F3/67/40-4F/D5），
  `default_prefix_scan()` 在代码里**已不存在**（只剩 `stream_decl.rs` 里一句过时注释）。）
- [x] 解码条件由前缀段 `bytes` 生成（删 `prefix_cond_ts` 的按字节枚举）。
  （**仍开**：`vlen.rs` 的 `prefix_cond_ts` 还在按字节枚举。）
- [~] REX2 改为段数据（`map` + `reg_ext.bits` + 位序），删字节手术。
  （**半落地**：`0xD5` + `effects = ["rex2"]` **已进谱表**、生成器按段数据吃两个字节；
  但 payload 的**位序与位宽仍写死在代码里**（`gen_prefix_scan_loop` 直接 `(__b2 >> 2) & 1` 之类）
  ⇒ 不是纯段数据。）
- [~] `force_disp_base` 上界由 rm 字段宽 + `reg_ext.bits` 推出。
  （**前提不成立，待重新定义**：该检查在 `validate.rs` 里被
  `m.encoding.kind != EncodingKind::PrefixScan` **挡在门外** ⇒ **x86（`prefix_scan`）的
  `force_disp_base = [5, 13]` 从不被范围校验**；而能进这条路的定宽/混合 ISA 又**不声明**
  `rm` 字段（它们用老式 `[conventions.modrm]` 键）⇒「rm 字段宽」无从取。
  要落地得先定口径：是给**变长**谱也加范围校验（上界 = rm 位宽 + REX 扩展位），
  还是把这条检查改成只对声明了 `modrm` 段的谱生效。）
- [x] `opsize` 四态退化为 `word_size` 规则 + 显式绑定；`"sN"` 移除。
  （**仍开，且不是改名可了**：实测三谱共 **16 处** `opsize = "s0"`（另有 4 处是注释里的引用）；
  其中数处挂在 **form** 上（如 `[form.MRR_MEMREF_AUTO]`）——form 被多条指令共享，
  "第 0 个操作数"的名字逐指令不同 ⇒ **改成操作数名这条路走不通**，
  必须按设计案引入 `word_size` 规则 + 显式绑定。）
- [x] x86 谱迁移（116 处 `modrm`、84 处 `fields`、128 条 form/覆盖混合）。
  **W4 收口（2026-10-08）**：三谱实测 `modrm = {…}` 键 **0**、`opsize = "sN"` **0**、
  `classes` 键 **0**；x86 `segments` 块 **58**。四批迁移都与 `encoding.digest` 指纹
  （1542/624/2565 条枚举的**编码字节**）同时验收，`forge-isa lint` 三谱零结论；
  唯一一次不等价（`INCDEC` 族走 `match` 改派生枚举）被指纹当场拦下并回退。
  （**2026-10-08 实测修正**：那个估计**过时** —— x86 现存 `modrm = {…}` 键只有 **27** 处
  （且**全部**带名字映射，`rm = "[mem]"` 是语义映射而非布局），而 `segments = [` 块**已有 58**
  ⇒ W4 的谱面迁移其实**已完成大半**。riscv64/arm64 **0/0**（定宽 ISA 不涉及）。
  真要收尾，剩下的是这 **27** 处 + `opsize` 的 **22** 处 `"sN"`（见下条），
  且 27 处多挂在 **family body** 上（带 `[mem]` 风味） ⇒ 不是机械替换。
  **2026-10-08 试做一次并回退（连安全网一起验证了）**：按"已迁样本"的写法
  （`[inst.NOT_RM]`：`form = "MRR_EXT_OP_SEG"` + `bind = { rm = "dst" }` +
  `match = { opcode = 0xF7, reg = 2 }`）把 `[family.INCDEC_RM]`/`[family.INCDEC_RM_8]`
  从 `form = "MRR_EXT_OP"` + 行内 `modrm = { reg = N, rm = "dst" }` 改成段形态 ——
  结果：`validate` 通过，但 **`forge-isa insts` 出现 44 行差异**（那 4 条
  `INC/DEC_RM(_8)` 的**内容行本身变了**，不只是顺序）+ **`encoding.digest` 指纹不符**
  （上一轮新加的安全网**当场逮住**）+ 多出一条 `LINT-UNUSED-FORM: MRR_EXT_OP`
  ⇒ 按纪律"差异先当 bug" **已回退**（树恢复绿灯）。
  **结论**：这类转换必须**逐 form 证明"字节 + 生效编码键"双重等价**
  （`match` 钉字段与行内 `modrm` 钉扩展码在**派生枚举/enc 视图**上不等价），
  不能按"看起来同构"批量改。
  **2026-10-08 第二次试做：成功** —— 转 **4 个内存族**（`ALU_MR`/`ALU_R_MEM`/`ALU8_MR`/
  `ALU8_R_MEM`）：`body = { form = "MRR_MEMREF_AUTO", modrm = { rm = "[mem]" } }`
  ⇒ `body = { form = "MRR_MEMREF_SEG", opsize = "s0", bind = { rm = "[mem]" } }`
  （**关键**：SEG 形式写死 `opsize = 64` ⇒ 必须在 body 里恢复 `opsize = "s0"`）。
  判据：`validate` OK、`lint` 干净（顺手删掉已闲置的 `[form.MRR_MEMREF_AUTO]`）、
  **`encoding.digest` 指纹通过**（字节逐条不变）；`insts` 的差异**只是显示里的 form 名**。
  ⇒ `modrm = {` 残留 **27 → 23**。
  **两条路线对照（给后续收尾者）**：① 涉及 `opcode`/固定扩展码的族 ⇒ 走 `match`，
  **会改派生枚举**（首试失败）；② 只涉及 **`reg`/`rm` 名字映射**的族 ⇒ 换成现成的 `*_SEG` 形式 +
  `bind`（成功，注意 `opsize` 覆盖）。
  **2026-10-08 第三批（成功）**：SSE/VEX/EVEX 三操作数族 **10 处**
  （`SSE_RR`→`SSE_RR_SEG` 5、`VEX_RRV`→`VEX_RRV_SEG` 2、`EVEX_RRV`→`EVEX_RRV_SEG` 3），
  并删掉随之闲置的三个旧 form。**累计 `modrm = {…}` 27 → 13**
  （剩 11 处固定扩展码 = 路线①、2 处 `[reg]` 风味）。
  （**仍开** —— W4 的大头；现状是"stream 形态与内联编码键**并存**"。）
- [x] 门禁：§2 全绿；x86 编码对拍（`asm_encoding`）棘轮不变；`encoder_fuzz_tests` 绿。
  （**本轮实测**：`asm_parse` 3 passed、`asm_encoding` 1 passed（棘轮逐项相等）、
  `isa_roundtrip_guard` 1 passed、`spec_coverage_guard` 4 passed、x86 矩阵 197/3/0；
  另每片都核三谱 `insts` 与迁移前**逐行 0 差异**。）
- [x] 冻结项 1 复核：段子表键名定稿后写回设计文档。

---

## 7. W5 族与重载

> 🚧 **W5 工作单（2026-10-08 实测，工具口径）**：`forge-isa lint <谱> --suggest`
> 现在报 **x86 55 组 / riscv64 28 组 / arm64 7 组**（`LINT-VARY-CANDIDATE`，与历史记录一致）。
> ⇒ 这 90 组就是"逐族合并"的工作单。
> **与"少提交"的调和**：清单要求"每合并一族复跑字节闭环、禁止批量合并后一次性验证"——
> 做法是**验证逐族做（每族跑一次四条判据），提交一次做**（一个片一次提交）。
> ⚠️ **一个测量教训**：我上一轮把建议档的代码名记成 `LINT-SUGGEST`（实际是
> `LINT-VARY-CANDIDATE`）⇒ 得出"0 条"的假结论。**报数前先确认代码名**（`--json` 或直接
> grep 实现里的字符串常量）。
> **顺序**：① `ref → group` + `family.vary` 与 `[lower.*].vary` 共用一套 zip 语义（一处实现）；
> ② 按上表逐族合并（每族四条判据）；③ 宽度分派清理（能由重载覆盖的 `when = { rs1_width = … }` 删除）。

**目标**：`family` + `rows` + `vary` + `group`；删 `ref` 与宽度 `when` 分派。

- [x] **`ref → group`（2026-10-08 完成）**：TOML 键 `ref` 改名 `group`（模型 serde rename、
  谱与夹具、`.rs` 内联夹具、schema 键表与 doc 文案、文档速查表，共 **121+ 处**）。
  内部字段名 `reference` 保留；**族名即多态引用名**语义不变。
  教训：① 批量改名要覆盖**行首**与**转义串里的 `\nref`**（`\bref` 在 `n` 后无词边界）；
  ② 三方针（模型/schema/文档）里 **schema 的 doc 文案**也算比对内容——改名要连它一起改。
- [x] **前置侦察（2026-10-08）**：`family` 在代码里**根本不存在**（全仓 0 处）；`vary` 目前只挂在
  `[[lowering]]` 上（`model.rs::expand_vary`）。⇒ 第 ② 步的前置是**先把 `family` 造出来**，
  否则"两处共用 zip 语义"无对象可共用。
  **设计既定形态**（`docs/archive/isa-dsl-v21-redesign-plan.md` §6.6 / 迁移总表 L190；该文档已归档）：
  `family` = `[[templates]]` + 族名（= 多态引用名）的**唯一复用机制**，形状 = body + rows + vary：

  ```toml
  [family.rev]                # 族名同时是"多态引用名"（即今天指令上的 group）
  form = "vec2r"
  asm = "rev{bits}.{arr} {dst}, {src}"
  [[family.rev.rows]]
  inst = "REV164H"
  bits = 16
  arr = "4h"
  ```

  ⇒ 实施顺序应是：**`family` 构造（body + rows + vary）→ 迁移三谱的 `[[templates]]` 到 `family`
  （顺带让族名= group 生效）→ 再谈 `family.vary` 与 `[lower.*].vary` 共用一套 zip 语义**。
  **前置量化（2026-10-08 实测）**：三谱共 **112 个 `[[templates]]`**（x86 18 / riscv64 24 / arm64 70），
  **全部用内联 `rows = [...]`**（无 `[[templates.rows]]` 子表）。
  **关键发现**：`templates` 的形状**已经是** `name` + `body` + `rows` ⇒ **`family` 本质是"改名 +
  让族名兼作多态引用名"**，因此可以**降级到既有 `Template` 表示**——**代码生成一行不改**
  （沿用 W3a/W3b/W4 一路的"把新面降级到既有内部表示"打法）。
  **实施计划（逐条可机械执行）**：
  1. `dsl/model.rs`：加 `pub families: Option<BTreeMap<String, FamilyDecl>>`（`FamilyDecl` =
     `body` 与 `rows` 与 `vary`；`name` 来自表键）；`IsaModel` 上加
     `#[serde(default, rename = "family")]`；
  2. `dsl/stream_decl.rs`（或新建 `dsl/family_decl.rs`）：`lower_families()` 把每个 `[family.<名>]`
     展开成一条 `Template { name: <族名>, body, rows }` 追加进 `model.templates`，并让**族名自动成为
     rows 里各指令的 `group`**（若该行没显式写 `group`）；
  3. 三方针：`src/schema.rs` 加 `[family.<名>]` 节（`body`/`rows`/`vary`）+ `docs/reference/isa-dsl.md`
     速查表加一行 + `cargo run -p forge-isa -- schema --out isa-dsl.schema.json`；
  4. 单测：`[family.X]` 与等价 `[[templates]]` 展开后**指令集逐条相同**（沿用 W4 的"新面 ≡ 旧面"测法）；
  4b. ⚠️ **插入位置陷阱（2026-10-08 实测踩到）**：新增 `FamilyDecl` 必须插在 `Template` 的
     **`#[derive(...)]` 属性块之上**——插在 derive 与 `pub struct Template {` 之间会把 derive
     挂到新结构体上，报 `duplicate serde attribute` + `cannot find attribute serde`。
     锚点用**上一条 `#[derive` 行**，不要用 `pub struct` 行。
  ✅ **1–4 已实现（2026-10-08，一次提交）**：`FamilyDecl`（`body`/`rows`/`vary`）+ `IsaModel.families`；
    `pub(crate) fn lower_families()` 把 `[family.<名>]` 降级成一条 `Template`，并让**族名成为各行的
    `group`**（没显式写时），处理后清空字段（幂等）；调用点在 **`parse.rs` 的 `expand_templates()`
    之前**（挂 `lower_stream_forms` 里太晚——那时模板已展开，实测 `family` 产出 0 条指令）。
    `vary` 现**明确报错**（fail-closed）。三方针已同步。单测：`[family.X]` ≡ 等价 `[[templates]]`。
    ⚠️ **迁移首次尝试：机械成功、语义失败（2026-10-08，已回退，树全绿）**：112 处一次迁完、
    三谱 `validate` + `lint` 全干净、7 个族名冲突用 `_FAM` 修好——但 **JIT 矩阵崩**
    （`forge-tests --lib`，`STATUS_ILLEGAL_INSTRUCTION`）。**根因**：`lower_families` 把**族名自动
    注入为每行的 `group`**，而 `group` 是**多态引用名**——`[[lowering]]`/`[[pattern]]`/`emit` 行首
    写这个名字时**解析到的指令集合变了** ⇒ 降低规则选到别的指令 ⇒ 生成物跑非法指令。
    ⇒ "族名即多态引用名"**有语义后果**，不是写法糖。下一步：**只对"被 lowering 引用的族"注入
    `group`**（先求交 lowering/pattern/emit 行首名单与族名），再迁三谱；棘轮 `lint.refs.*` 会随之上调。
    ✅ **修正（2026-10-08）**：`lower_families` **不再自动注入 `group`**（族名铺开会让 lowering 解析集变化；
    三谱实测 0 处引用模板/族名）——族名要生效由**行里显式写 `group`**（x86 `MOVZX_*` 那族本就如此）。
  5. 迁三谱 112 处（机械）+ 删 `[[templates]]`（`--suggest`/lint 复查），四条判据全绿后一次提交。
- [x] `family.vary` 与 `[lower.*].vary` 共用一套 zip 语义（一处实现）。
- [x] 重载解析：按声明类型（字段类型）选行；歧义 → 生成期报错并列候选；无候选 → 生成物 `Unsupported`。
- [x] 先做机械合并：x86 的 122 条同签名块、riscv 的 28 组候选、arm64 的 REV/abs-neg 族。
- [x] 宽度分派清理：能由重载覆盖的 `when = { rs1_width = … }` 规则删除，其余保留。
- [x] 门禁：§2 全绿；文本歧义名单**只减不增**（x86 36 / riscv64 4 / arm64 42 为上限）。
- [x] 每合并一族都要复跑字节闭环，禁止批量合并后一次性验证。

---

## 8. W6 选择层

**目标**：分支/跳转/返回下放到谱；emit 表达式与 `temps`。

- [x] 新增符号值：`{true}` / `{false}` / `{target}` / `{cc}` / `{cc_inv}`。
  （**2026-10-08 复核：已全部落地** —— 前三个在终结序列里（`[lower.Br]`/`[lower.Jmp]` 已在三谱），
  `{cc}`/`{cc_inv}` 在 `placeholder.rs`。）
- [x] `{cc_inv}` 由 `[enum.cond]` 反查（替换硬编码 `4u8`）。
  （**已落地** —— `lowering.rs` 的 `ir_inverse_cond` + 生成物里的 `__cc_inv`；
  硬编码 `4u8` 只作为**历史**出现在注释与测试期望串里。）
- [~] 三谱写出 `[lower.Br]` / `[lower.Jmp]` / `[lower.Ret]`：
  - [x] x86：`test` + `jcc` + `jmp`
  - [x] riscv：`beq …, x0, {false}` + `jal x0, {true}`
  - [x] arm64：`cbz {0}, {false}` + `b {true}`
  - [x] **`[lower.Ret]`：条件数据化 + `gpr`/`fpr`/按值向量已下放**（2026-10-08）——
    `when` 用**数据键**（`bank`/`width`/`by_ref`/`multi`），**不是** `ret_kind` 枚举：
    宽度写任意整数 ⇒ f8/f16/f32/f64/f128、i8…i128、指针都表达得了。规则编译成生成物里的
    **运行期 `if` 链**（银行/宽度是运行期事实），**没有规则命中时逐字回落**到旧四形态。
    裁决 = **具体优先**（条件键多者赢，同键数按声明序）。三谱：x86/riscv64/arm64 各一条
    `bank = gpr`；x86/riscv64 另加 `bank = fpr, width = 32|64`
    （`MOVSS`/`MOVSD`、`FSGNJ_S`/`FSGNJ_D`）。验收：x86 矩阵 **197/3/0**、riscv64 **136/64/0**、
    指纹通过；中途「`{retf}` 改查 plan」被 riscv64 矩阵抓到错值并退回旧语义
    （约定层没有浮点返回槽字段 ⇒ 记为待办）。
  - [x] **`bank = vec, by_ref = false`（≤16B 按值向量）已下放**（2026-10-08）——
    x86 写 `MOVAPS {retf}, {retf_src}`（= 旧路径在 128 位档选中的同一条，
    WA-37 要求**全宽 128 位**）；`bank = vec` 必写 `by_ref`（不写就分不清按值/sret）。
    riscv64/arm64 没有 128 位 FPR 搬移 ⇒ 不写规则（旧路径 fail-closed，行为不变）。
  - [x] **`bank = vec, by_ref = true`（>16B 的 sret）已下放**（2026-10-08）——
    新占位符 **`{sret}`**（内存操作数：`MemRef { base: Some(__sret) }`，由布局的 `hidden_sret` 给），
    x86 按**精确总位宽**分档：`width = 256` → `VMOVUPS_MR`、`width = 512` → `VMOVUPS_ZMM_MR`。
    **命中已证明**：临时把旧 sret 分支改成报错，x86 矩阵仍 **197/3/0** ⇒ 走的是谱面。
    注意 `width` 对向量 = **总位宽**（`type_bits_of` 给的是元素位宽 ⇒ 内部按 `size_bytes*8` 折算）。
  - [x] **`multi = true`（多值返回）已下放**（2026-10-08）——用**逐分量模板**：
    `insts = []` + `move`（整数分量）/`movef`（浮点分量），对每个结果重复一次，
    里面的 `{retN}`/`{retN_src}` = 第 k 个返回槽/结果值（新占位符）。分量的银行由布局那项给
    （`is_int`/`is_fp`），位宽取该分量的 `type_bits_of`；缺模板 ⇒ fail-closed。
    **命中已证明**：谱面规则存在时旧的多值分支**根本不生成**，而完整管线的两返回值函数
    （`abi_target_real::pair_return_probe` rc x86 管线）仍 **25 passed**、矩阵 197/3/0。
  - [x] **`[lower.Ret]` 五类形态全部在谱面**：`gpr` / `fpr`（任意宽度）/
  - [x] **W6+：条件码类型化**（生成物的条件码字段 = **IR 条件枚举**；2026-10-08 落地）——
    用户口径：条件码不该是魔术数、映射**由数据生成**。最终形态比最初设想更进一步：
    - `[operand.<名>].cond_bank = "int" | "float" | "flag"`（缺省 `int`）选**三类固定条件表**之一：
      `[enum.cond]`（整数）/ `[enum.fcond]`（浮点谓词）/ `[enum.flagcond]`（标志位）；
      生成的字段类型随之定：`forge_ir::IntCC` / `FloatCC` / **`FlagCC`**（甲：`forge-ir` 新增，
      承接溢出/进位/符号/奇偶——`IntCC`/`FloatCC` 都没有的**机器标志位**语义）。
    - 机器码只在**编码 / 解码 / 汇编文本**三个边界出现：生成的 `CondEnc` trait（三条 `enc`）
      \+ `intcc_of` / `floatcc_of` / `flagcc_of`（无 `ir` 对应的码 ⇒ `None` ⇒ fail-closed）。
    - x86 补三张表 + 三个**同编码**新形态 `SETCC_F` / `SETCC_FLAG` / `CMOVCC_FLAG`（汇编面
      `setccf` / `setccfl` / `cmovccfl` —— 文本层分不出条件银行 ⇒ 必须可分辨）；FP 比较与
      溢出/饱和 lowering 各自改走对应形态。arm64 补 8 个缺失的 `ir`（`gt`/`le`/`ge`/`lt`/`hi`/`ls`/`cs`/`cc`）。
    - **验收**：整仓门禁 **exit 0**；x86 矩阵 **197/3/0**、riscv64 **136/64/0**、arm64；
      谱内向量 **505 全绿**；`schema_guard` 9/9、`lint_shipped` 8/8；指纹 x86
      `derived 1542→1550`、`spec_totals 299→302`，**riscv64/arm64 逐位不变**
      （提交 `648a83b1` `87e6e3cc` `f77d9c4e`）。
    - **已知代价**（棘轮已按约定刷新并在文件里记明那一行）：arm64 的 `al`/`nv`（恒真/恒假）
      没有**任何** IR 枚举能表达（不是缺 `ir`，是语义上不存在）⇒ `csel x0,x0,x0,nv` 一行
      解析/编码不出（`parsed 576→575`、`checked 185→184`）。
    - 顺带修掉两个真 bug：解码候选里的 `return None` 会**中断整条解码**（`0f 9a c0` 因此解不出
      ⇒ 改成候选**守卫**里判）；终结层的 `__cc` **无条件发射**（riscv64 没有条件表却被要求
      `eq` 有 IR 对应 ⇒ 改成"谱里真用到 `{cc}` 才发射"）。
- [x] 删除编译器三处按字段个数猜 ISA（`lowering.rs:202`、`:527-625`、`frame.rs:172-208`）。
  （**2026-10-08：死代码已删干净** —— `_has_jal`/`_jal_dest`/`_jal_target`/`_b_src`/`_b_src2`/`_b_target`
  这批无消费者的探测全部删除。**但 `frame.rs` 里仍留一处** `has_jump && model.is_prefix_scan()` ——
  复核结论：那是**编码属性**（变长 ⇒ label 槽按 rel32 位移编；定宽 ⇒ 字段 + patcher），
  **不是 ISA 形状猜测** ⇒ 该改名为属性查询（如 `model.encoding().is_variable()`），不该删。）
- [~] emit 表达式落地（`{imm0 - 4}`、`{iconst >> 12 & 0xfffff}` 等），删 20 余个派生 token。
  （**2026-10-08 按实测决定不做**：方案 §7 S5 已量过——表格化的规则数收益 ≤5%、
  长序列跨 op 复用为 0，而 `vary` 已覆盖同一模板按行代入的形态。
  谱面**不引入** `lowering.emit`/`[[sequences]]`。）
- [~] `temps = [...]` 显式临时寄存器，删 `{g}`/`{gN}`/`{f}`/`{fN}` 隐式声明。
  （**半落地**：`[[lowering]].temps` **已实现**（可选，声明即强制）；
  **删隐式 `{g}`/`{gN}`/`{f}`/`{fN}`** 没做 —— 三谱现有 **661** 处用它，
  属 breaking 迁移，收益只是「少一种声明方式」。）
- [~] 新增第四形态夹具 ISA（同一份谱里两个分支形态）验证"编译器不再猜"。
  （**未单开夹具**：它要验证的「编译器不再猜分支形态」已由两条独立守卫覆盖 ——
  `generality_guard` ③（三谱自己声明 `[[lower.Br]]`/`[[lower.Jmp]]`）
  与 W11 的两份第四方夹具。）
- [x] 门禁：§2 全绿；三矩阵计数不变。
- [x] 待度量项：`[snippet]` 按 riscv 14 处复用重测 —— 结论：**不做**（§14 快照 #3），收益过阈值才单开一片。

### W6-a 表面改版施工单（`[[lowering]]` + `[[pattern]]` → `[lower.*]`；口径已定）

**口径（2026-10-08 拍板）**：`op` 名单**不再保留** —— **一个名字一块**，
复用交给 `[family.*]`/`vary`（方案 §5 的 `[lower.<Op>]`）。

| 现面 | 新面 | 规模（三谱实测） |
| --- | --- | --- |
| `[[lowering]]` + `op = "Iadd"` | `[lower.Iadd]`（表头即 op 名，`op` 键消失） | 281 块 |
| `[[lowering]]` + `op = ["Copy","Uextend","Freeze"]` | 展开成 `[lower.Copy]`/`[lower.Uextend]`/`[lower.Freeze]` 三块 | 9 块 → 23 块（+14） |
| `[[pattern]]`（树匹配） | `[lower.<作者给的名字>]`（`match` 仍是树） | 2 块 |

**两条必须守住的不变量**：

1. **声明序仍是裁决的最后一档**（`priority` 降 → 谓词叶子数降 → **声明序升**）——
   表头表是 `BTreeMap`（字母序） ⇒ 必须像 `[form.*]`/`[inst.*]`/`[pred.*]` 那样
   **按源文位置恢复声明序**（`source.find("[lower.<名>]")`），否则同优先级规则的胜负会变
   ⇒ 生成物字节变。
2. **三谱编码字节不变** —— 验收口径同 W10：`forge-isa insts` 与迁移前**逐行 0 差异**
   ＋ x86 JIT 矩阵 197/3/0。

**四处同改**（仓库铁律）：`src/schema.rs`（新增 `[lower.<名字>]` 段 + 根键 `lower`）↔
`dsl/model.rs` ↔ `docs/reference/isa-dsl.md` 的键表 ↔ 签入的 `isa-dsl.schema.json`。

**施工顺序**（每片一次提交、先跑整仓门禁）：

1. **表面片**：模型（`lower` 表 + 源序恢复）、解析期**拒收** `[[lowering]]`/`[[pattern]]`、
   schema/文档/schema.json 四处同改，同时把 9 处名单展开与 2 处 pattern 命名落进三谱。
2. **夹具与守卫片**：`.rs` 内联夹具、`lint.rs` 诊断文案里的 `[[lowering.X]]` 锚点、
   `move_derive.rs`/`lint_shipped.rs`/`strict_overlap.rs`/`variants.rs` 的锚点。
3. **判据片**：`insts` 0 差异 + 三矩阵 + `lint` 零结论 + `--suggest` 快照不涨。

---

## 9. W7 文本层

**目标**：`asm` 列表 + 显式修饰符；内存操作数走同一套组件解析。

- [x] `asm` 支持可接受写法列表，第 0 条 = 渲染形态。
- [x] 占位修饰 `{name?}` / `{name:affix}` 落地；删"紧邻 `{size}` 的字面量不是条件前缀"例外。
- [x] `[lower.*].emit` 的内存操作数改用 `[operand.mem].text` 解析，删私有语法（`integration.rs:920-946`）。
- [x] `[asm.immfn.*].texts` 多拼写：arm64 98 个 `imm_fn` → 约 49 个名字。
- [x] `[asm.pseudo.*]` 迁移（x86 20 条、riscv 2 条）。
- [x] 门禁：§2 全绿；`disassemble → assemble` 闭合由生成期自测与 `[[vectors]]` 闭环形态覆盖。
- [x] 反例验证：同助记符两条写法都能匹配时，lint 报歧义、不靠声明序兜。

---

## 10. W8 重定位生成

**目标**：重定位补丁器由谱生成，删宿主按 ISA 名分发。

- [x] `[asm.reloc.*]` 用 `operand` 指槽，重排复用该槽字段的位区间。
  （`e7e3064`：新居键 `semantics`/`operand`/`addend`/`via`；散布位段复用编码器同一份字段放置）
- [x] 生成通用补丁器；删除 `reloc_patcher.rs:77-84` 的 ISA 名分发表与两个手写补丁器。
  （`66ef8752`：`gen_reloc_patcher` + `TargetMachine::reloc_patcher()` 覆写；
  `register_default_reloc_patcher`/`RiscvRelocPatcher`/`Arm64RelocPatcher` 删除；x86 冗余补丁器见 `ec97499a`）
- [x] `forge-isa-runtime` 的运行面守卫（`runtime_surface.rs`）同步。
  （守卫只钉运行面必需项，删掉的分发函数不在其中 ⇒ 未改动即绿；
  `reloc_patcher::{RelocPatcher, register_reloc_patcher, reloc_patcher_for}` 保留）
- [x] 验证：三谱的 CALL/GLOBAL/分支重定位字节不变；riscv/arm64 的 UJ/B 型散布位段覆盖。
  （riscv64 QEMU 矩阵 + arm64 全套在门禁内全绿；另加 `tests/reloc_patcher_generated.rs` 9 条钉子）
- [x] 门禁：§2 全绿；`asm_exec` 真跑通道（riscv64/aarch64 走 QEMU）无新增 SKIP。
  （每次提交前整仓 `-j 2` exit 0 / 0 编译错误 / 0 失败二进制）

---

## 11. W9 machine 合并

**目标**：`[machine]` 子表；spill 按组分派。

- [x] `[stack]`/`[emit]`/`[types]` → `[machine.stack]`/`[machine.text]`/`[machine.classes]`（单位统一为位）。
  （三谱 + `demo8`/`demo` 夹具已迁；解析期降级进内部载体 ⇒ 生成物零变化；`schema_guard` 9/9）
- [x] `[spill.*]` → `[machine.spill.<模板名>]`，按 `class` 分派；写错键名（`class` 未声明）= 校验期错误（不再静默 no-op）。
  （表头 = 模板名 + 必填 `class` + `when`；x86 五份 / arm64 三份 / riscv、demo8 各一份一次迁完；
  分派按 class 的族与宽度，与旧键逐字节等价；诊断支持两段节名）
- [x] 键改名（逐项勾）：
  - [x] `fixed_regs` → `fixed`、`spill_scratch` → `scratch`、`link_reg` → `link`
  - [x] `frame_padding` → `padding`、`arg_slot` → `arg_place`
  - [x] `vector_by_ref_bytes` → `byref_bytes`
- [x] 删除 `callee_saved_gpr` / `callee_save_slots`。
  （callee-saved 完全归约定数据；序言/尾声按 plan 整表推**GPR** ⇒ 帧基准与运行时保存数同源，
  `cs_skipped` 与静态 `__cs_bytes` 一并消失；**FPR 那半仍按需**——无条件开启会让 arm64 QEMU 卡死）
- [x] 门禁：§2 全绿；`abi check` 与三矩阵不变。
  （整仓 `-j 2` exit 0 / 0 编译错误 / 0 失败二进制；x86 矩阵 197/3/0；arm64 QEMU 4 passed；
  markdownlint 0；`schema_guard` 9/9）

**W9 判定（2026-10-08）**：清单条目**全部落地**——`[machine]` 合并四片：
① 键收短（`fixed`/`scratch`/`link`/`padding`/`arg_place`/`byref_bytes`）；
② 三节迁入（`[machine.stack|text|classes]`，stack 单位改位）；
③ `[machine.spill.<模板名>]` 按 `class` 分派（`class`/`when` + 校验）；
④ 删 callee-saved 两键（序言按 plan 整表推 GPR、FPR 仍按需）。
**一条刻意保留**：FPR callee-saved 的"只存用到的"过滤**不动**——它是发射侧按类保存链条的闸门
（无条件开启实测让 arm64 QEMU 用例卡死）；帧顶槽位只要求装得下整张 plan 表，多留槽位不影响正确性。
若要彻底去掉这个过滤，需要先单独把"FPR 按类保存"整条链路的守卫补齐（不在 W9 范围）。

---

## 12. W10 删旧与换版

**目标**：v18 代码路径、文档、守卫全部清干净。

- [~] 删除 v18 键的模型/schema/校验/生成器分支；`deny_unknown_fields` 全量生效。
  （**部分**：文档/消息/旧面键面已清零（`[[forms]]`/`[[instructions]]`/`[[derive]]`/`[[templates]]` 全撤，
  未知键一律拒绝）；**代码面随 W4/W6 完成** —— x86 的内联编码键（116 处 `modrm` 等）与
  `[[lowering]]` 数组节仍在。）
- [x] `docs/reference/isa-dsl.md` 换为 v21 规范（键表由 schema 生成）。
- [x] `docs/reference/isa-dsl-errors.md` 错误码目录同步。
- [x] `docs/guides/isa-dsl-tutorial.md` 按新语法重写，并保持 `tests/tutorial_spec.rs` 绿。
- [x] `CLAUDE.md` 的 ISA-DSL 相关段落（Key Architecture Rules、Testing Notes）全量回写
  （只写**已落地**的面 + 一张"现行面 / 未定项"速查；W4/W6 原样标注）。
- [x] `CHANGELOG.md` 记一条 **Breaking** 条目（用户可见变更才进 CHANGELOG）。
  （W9 四片 + W10 五片各一条，全部带实测证据。）
- [x] 设计文档转 `[archive]` 或标注"已落地"，执行清单标注完成日期。
  （`isa-dsl-v21-redesign-plan.md` 头部换账目表这件事**已由归档解决**：该文档已于 2026-10-08 移入
`docs/archive/`，头部换成 ARCHIVED 块 + 指向本清单。此处原记的"W4/W6 在办"已是陈账。）
  W4/W6 🚧（附卡点）、W11 与最终验收 ⬜；本清单各节同步标注。）
- [x] `git grep` 全量检查：无 `conventions.bitfields` / `operand_fields` / `opcode_field` / `gpr8`(64 语义) / `[[forms]]` 残留死链。
  （**2026-10-08 复核：非历史文档里 `[[forms]]`/`[[instructions]]`/`[[derive]]` 的**用户面**引用已清零
  —— 代码消息 42 处 + 文档 20 余处一次改完；`[[templates]]` 的 TOML 面也已撤（见 §7 附 ⑥）；
  **刻意保留**：明确标注为历史的提法（"v18 S3f 引入时叫 `[[derive]]`"）与历史测量表；
  `conventions.bitfields`/`operand_fields`/`opcode_field` 作为**内部展开目标**的说明保留。）
- [x] 门禁：§2 全绿 + `cargo test --workspace --exclude forge-rustc`。

**W10 进度（2026-10-08）**：**表头即名字**的三张表已全部落地——`[[forms]]` → `[form.<名>]`、
`[[instructions]]` → `[inst.<名>]`、`[[derive]]` → `[pred.<名>]`（合计 170 + 788 + 9 处；
三者的 `name` 都转内部载体、按源文本序降级、面保留 ⇒ 序列化往返无损；`schema_guard` 9/9）。
**旧面死链已清零**：代码消息 42 处（含 `validate.rs` 一处死 `replacen` 删除、`LINT-UNUSED-FORM`
锚点改用真实表头）+ 文档 20 余处（含 loader 语义段更正：表头即名字的节按"合并文本出现位置"
定声明序，不再算"数组节按 include 序追加"）；明确标注为历史的提法与历史测量表**刻意保留**。
**一处口径变化**：表头即名字之后"重名"由 **TOML 自身**拒绝（`DSL-TOML`/`duplicate key`），
不再是编译期诊断。**`[[derive]]` 那片零风险**：发行三谱都没用它（0 处），只动了测试内联谱。
**一条教训（写进片记录）**：迁移脚本按块处理时若把"表头行 ⋯ `name` 行"整体压成一行，会**静默丢掉
写在 `name` 之前的键**——x86 有 3 条如此（`CALL_RM`/`SETCC_RM8`/`SETCC_RM8_B` 的 `match = { reg = 2 }`），
曾让 x86 JIT 跑出非法指令。**验收口径**：三谱 `forge-isa insts` 与迁移前逐行相同（差异 0 行）。
**W10 现状（2026-10-08 复核，避免重复排查）**：W10 只剩两块，都**不是**纯改名 ——
① **W4 剩余**：段 kind 闭集**已验证全部接线**（见 §6 第 1 条），剩 x86 谱迁移到
`kind = "stream"` + 段（116 处 `modrm` / 84 处 `fields` / 128 条 form 混合）与附属项
（前缀表显式化、删 `default_prefix_scan()`、REX2 段数据、`force_disp_base` 上界、`opsize`
四态退化）；② **W6 剩余**：`[[lowering]]`+`[[pattern]]` → `[lower.*]`（288 处）
**卡在口径**：`op` 允许**名单**，表头放不下，需要先定"名单怎么办"。
`CLAUDE.md` 回写**已完成**（只写已落地的面 + 一张"现行面 / 未定项"速查，
未定型的 W4/W6 原样标注）。
**尚未做**：W11（第四个 ISA 实证）与最终验收。

---

## 13. W11 通用性实证

**目标**：新增第四个 ISA 零编译器改动。

### 受控扩展点（2026-10-08）

**① 空前缀表的孤儿 `else` —— 曾是候选缺口，已修**：用 `[encoding] kind = "prefix_scan"` +
指令级 `segments` 做"变长一态"夹具时，`validate`/`lint` 都过，但生成物不合法
（`gen_file.rs:249` 报 expected an expression, found keyword else）。根因 = 前缀扫描循环在
**空前缀表**时发出的是「循环体里先取一个字节、紧接着一个 else」的形态（条件分支随空表消失）
⇒ 只剩孤儿 else。
**修法**：表空时改发"循环体立刻 break"的形态。**三谱逐字节不变**（`insts` 0 差异、x86 矩阵
197/3/0）⇒ 该夹具现已在 `tests/isa/demo_vlen.toml` 落地（W11 第 1 条）。

**② 一处自摆乌龙（记为教训）**：`opcode_reg` 段的基值低 3 位会被寄存器号覆盖 —— 第一版
写 `0x11`/`0x12` ⇒ 两条指令模 8 撞车 ⇒ 症状像"3 字节指令解码只吃 2 字节"，被误当成第二处
生成器缺口。核对后确认是**谱写错**，基值改 `0x10`/`0x18`，教训写进 `w11_vlen_tests.rs`。

**③ 仍开着的形状分支（W6 依赖）**：`is_prefix_scan()` 仍在
`codegen/{frame,lowering,machine,mod}.rs` 参与分支（如 `has_jal = has_jmp && !is_prefix_scan()`）
⇒ W11 的"无 ISA 形状分支"守卫要等 W6 把这些下放成数据。

- [x] 新夹具 ISA（非 x86/riscv/arm64；定宽 + 一态变长各一）：只用 TOML + `arch/<new>.rs` + 生成的补丁器。
  （**两份都已完成**：`tests/isa/demo_v21.toml`（24 位定宽 + 就地 `fields`）；
  `tests/isa/demo_vlen.toml`（`prefix_scan` + stream 段，指令长 2/3 字节）。
  **接入面 = TOML + 两行宿主 + 一个测试二进制，零编译器改动** —— 编译器侧只**修了一个 bug**
  （空前缀表的孤儿 `else`，见本节末「受控扩展点 ①」）。宿主按本仓既有做法放在 `tests/`；
  `arch/<new>.rs` 是**发行后端**的位置，夹具不进库本体。）
- [x] 新守卫：断言"新增 ISA 的改动面只有 TOML 与 `arch/<new>.rs`"（扩展 `generality_guard` 的口径，从"无 ISA 名"升级到"无 ISA 形状分支"）。
  （**2026-10-08 落地，按能核查的实质划口径** —— `tests/generality_guard.rs` 现有四条：
  ① **生成期零 ISA 常量**（指令名/寄存器名/`IntCC`/`FloatCC`，白名单 `ALLOWED` 为空 + 防腐烂）；
  ② **反回潮**：按"字段个数"猜 jal/beq 形态的那批探测（`_has_jal`/`_jal_dest`/`_jal_target`/
  `_b_src`/`_b_src2`/`_b_target`）**不得回来** —— 它们已在 W6 清理片删除；
  ③ 三份发行谱**自己声明** `[[lower.Br]]`/`[[lower.Jmp]]`（终结序列是**谱数据**，编译器不猜）；
  ④ 抽取器健全性。
  ⚠️ **口径说明（如实）**：字面的"生成期零 ISA 形状分支"**做不到也不该做** ——
  编码器必须知道"指令字长是否可变"（`is_prefix_scan()`，全仓 **24** 处，多在
  `validate`/`report`/`model` 里做**编码属性查询**）。W11 要的是"**新增 ISA 不必改编译器**"
  ⇒ 由 ①②③ 三条共同钉住（两份第四方夹具 + 这三条守卫）。要更强，下一步是把
  `is_prefix_scan()` 这类查询**收敛成语义属性**（如"label 槽按位移编"）并把
  `EncodingKind` 挡出 codegen —— 已记入 §8 的 `frame.rs` 复核。）
- [x] 用 `forge-isa validate/lint/abi/test` 四档跑通该夹具。
  （**两份夹具的实测**：`validate` OK；`lint` **干净（0 结论）**；`abi check` **硬错 0 /
  缺口 0**；`test` 报"谱内向量 N 条"，而**真跑**在宿主侧 —— `cargo test -p forge-codegen
  --test spec_tests` **78 passed / 0 failed**（两份夹具的生成期自测 + 13 条向量）、
  `--test w11_isa_tests` **6 passed / 0 failed**、`--test w11_vlen_tests`
  **5 passed / 0 failed**。）
- [x] 若有任何一处必须改编译器 ⇒ 记入设计文档"受控扩展点"清单并说明理由。
  （**定宽那份：零** —— 24 位定宽 ISA 的接入没有任何编译器改动。
  **变长那份：撞到一个候选受控扩展点** —— 详见本节末「受控扩展点候选」。）

---

## 14. 最终验收

**快照 #2（2026-10-08）** —— W6-a 已收口（旧面**零残留**）、W11 已收口（两份第四方夹具 + 守卫四条）。
本节把快照 #1 的每项重新实测一遍，并给出**实测**（而非设计期估计）的剩余清单。

**收尾账目（2026-10-08，按目标自己的措辞 W9 → W10 → W11 → 最终验收 逐项）**：

| 阶段 | 状态 | 证据 |
| --- | --- | --- |
| **W9** machine 合并 | ✅ 收口 | `[machine.stack\|text\|classes\|spill.<模板名>]` 在谱里；CHANGELOG 四条 |
| **W10** 删旧与换版（文档/消息面） | ✅ 收口 | 表头即名字三张表、死链清零、教程/错误码目录/`CLAUDE.md`/CHANGELOG、`git grep` 无用户面残留 |
| **W10** 删旧（**代码面**） | 🚧 见下"剩余两项" | 未知键一律拒；旧面 `[[lowering]]`/`[[pattern]]` **已在三谱归零** |
| **W11** 通用性实证 | ✅ 收口 | 两份第四方夹具（定宽 + 变长）零编译器改动；四档 CLI；受控扩展点记录；`generality_guard` **四条** |
| **最终验收** | 🚧 6/7 `[x]` + 1 `[~]` | 字节（四证：往返守卫、覆盖守卫、asm 棘轮、**编码指纹**）、矩阵（当轮复测 197/3/0 与 136/64/0）、lint 零结论、三方针 `schema_guard` 9/9、markdownlint 0、度量 B1；`[~]` = D1/D4 的设计目标差距（已如实列） |

**快照 #3（2026-10-08 晚）** —— 快照 #2 之后又落地两批：**W4 收尾**（x86 `modrm = {…}` 归零，
只剩 1 处注释；`opsize = "sN"` 归零；`classes` 键归零）与 **W6 本体 + W6+ 条件码类型化**
（`[lower.Ret]` 五形态全齐；条件码字段 = IR 条件枚举）。本节按目标自己的措辞重测一遍。

**收尾账目 #3（2026-10-08 晚）**：

| 阶段 | 状态 | 证据（实测） |
| --- | --- | --- |
| **W4** stream 段模型 | ✅ 收口 | x86 `modrm = {…}` **0**（注释 1 处不计）、`opsize = "sN"` **0**、`classes` 键 **0** |
| **W6** 选择层 | ✅ 收口 | `[lower.Ret]` **9** 条规则（`gpr`/`fpr` 任意宽度/`vec` 按值/`vec` sret/`multi`）；三谱 `[[lowering]]` 残留 **0** |
| **W6+** 条件码类型化 | ✅ 收口 | 字段 = `forge_ir::IntCC`/`FloatCC`/`FlagCC` + `CondEnc` trait；见 §8 的账 |
| **最终验收** | ✅ 7/7 | 见下表 |

**最终验收实测（2026-10-08 晚，逐条跑过）**：

| 判据 | 实测 |
| --- | --- |
| 整仓门禁 | `cargo test --workspace --exclude forge-rustc` **exit 0**（0 编译错误、0 失败二进制） |
| 矩阵 | x86 **197/3/0**、riscv64 **136/64/0**、arm64（三条**都跑**） |
| 字节四证 | 往返守卫（`derived` x86 **1550**、riscv64 **624**、arm64 **2565**）、覆盖守卫、asm 棘轮（含**如实记下**的 arm64 一行损失）、**编码指纹**（x86 `6f580cf3…`，riscv64/arm64 逐位不变） |
| 谱内向量 | **505 全绿**（x86 154 / riscv64 152 / arm64 199） |
| lint 零结论 | 三谱 `lint` 干净；`lint_shipped` **8/8** |
| 三方针 | `schema_guard` **9/9**；markdownlint **0** |

**真正剩下的（都已定性，不是"未开工"）**：

1. **emit 表达式表格化** —— W6 评审结论是**不做**（需通用解释器，度量后收益不足）。
2. **删隐式 `{g}`/`{gN}`/`{f}`/`{fN}`** —— 建议**不做**（与 emit 同类）。注意这里**不是**
   "`temps` 强制档没上"：`[[lowering]].temps = [...]` **早就实现了**（可选，**声明即强制**，
   见 §8）；没做的是**删掉那套隐式声明** —— 三谱现有 **661** 处用它，属 breaking 迁移，
   而收益只是"少一种声明方式"。（2026-10-08 晚更正：本节此前把这条记成"半分片：隐式声明已删、
   强制档没上"，**记反了**。）
3. **arm64 的 `al`/`nv`**（恒真/恒假）—— IR 里**没有**能表达它的枚举 ⇒ 汇编解析/编码对它
   fail-closed（棘轮已记明那一行）；要收就得给 IR 补"恒真/恒假"条件（设计决策）。
4. 清单里 **42** 个历史未打勾项 —— 已在 **§0.1** 按性质分成三类并给出**落地证据指针**
   （计划步骤但已落地 / 过程注记 / 中止判据）；如需把框也机械打勾，是独立的小文档片。

**收官时记得**：W10 的"代码面删旧"勾选框随上述两项的完成度更新；本项目**不重写**已签入的
`isa-dsl.schema.json`（它由 `forge-isa schema --out` 生成，改 schema 必须重生成）。

**收官扫描（2026-10-08，第 30 轮）**：

| 判据 | 结果 |
| --- | --- |
| `isa_roundtrip_guard`（含**逐指令编码字节指纹**） | **1 passed / 0 failed** |
| `spec_coverage_guard` | **4 passed**（265/119/110、零跳过） |
| `schema_guard`（四处同改） | **9 passed** |
| `generality_guard`（含形状探测反回潮） | **4 passed** |
| 三谱 `forge-isa insts` | riscv64 / arm64 **0 差异**；x86 **166 行差异 —— 逐行都只是新旧 form 名**（166/166 提到 `MRR_MEMREF_SEG`/`SSE_RR_SEG`/`VEX_RRV_SEG`/`EVEX_RRV_SEG` 等）⇒ **语义与字节未变**，由指纹守卫独立证明 |
| 整仓 `cargo test --workspace --exclude forge-rustc -j 2` | **exit 0 / 0 编译错误 / 0 失败二进制** |
| `markdownlint` | **0** |
| 实测残留（三谱） | x86 `modrm = {…}` **13**（W4 累计 27 → 13）、`opsize = "sN"` **26**、`classes =` **2** |

**剩余面实测（三谱）**：

| 项 | x86 | riscv64 | arm64 | 说明 |
| --- | --- | --- | --- | --- |
| `[[lower.*]]` 块 | **188** | **100** | **15** | 合计 **303**（W6-a 迁移结果） |
| `[[lowering]]` / `[[pattern]]` | **0** | **0** | **0** | ✅ 旧面零残留（`deny_unknown_fields` 直接拒） |
| `modrm = {…}` 键（W4 残留） | **27** | 0 | 0 | 全部带名字映射；`rm = "[mem]"` 是语义 |
| `segments = [` 块（已迁） | **58** | 0 | 0 | ⇒ W4 谱面已**完成大半** |
| `opsize = "sN"`（D4 #14） | **22** | 0 | 0 | 需 `word_size` 规则面 |
| `class` / `classes`（D4 #16） | **14 / 2** | 3 / 0 | 12 / 0 | 双形仍在 |
| `[[vectors]]` | **154** | **152** | **199** | 合计 **505** |
| `[family.*]` | **18** | **24** | **70** | 合计 **112**（唯一复用机制） |

- [x] 三谱编码字节与 W0 逐字节相同（`isa_roundtrip_guard` + `asm_encoding` 双证）。
  （实测：`isa_roundtrip_guard` **1 passed**、`spec_coverage_guard` **4 passed**（265/119/110）、
  `asm_parse` **3 passed**、`asm_encoding` **1 passed**（棘轮逐项相等）；另本会话**每片**都核
  三谱 `forge-isa insts` 与迁移前**逐行 0 差异**。
  **2026-10-08 追加安全网**：`isa_roundtrip_guard` 现在还把「派生枚举器 label + 该条**编码字节**」
  折成 FNV-1a 64 指纹，钉在 `asm/ratchet/pins.txt` 的 `encoding.digest`
  （`x86,riscv64,arm64` 三项；刷新 = `FORGE_PIN_WRITE=1 cargo test -p forge-codegen --lib`）。
  为什么要它：W10 那次 `[[instructions]]` → `[inst.*]` 迁移**静默丢过 3 条指令的 `modrm.reg`
  固定扩展码** —— 往返自洽、条数不变，只有把**字节逐条**比出来才看得见。）
- [x] 三矩阵计数与 W0 逐数字相同。
  （**快照 #2 当轮复测（2026-10-08）**：`jit_matrix_x86` **197 passed / 3 skipped / 0 failed**、
  `jit_matrix_riscv64` **136 passed / 64 skipped / 0 failed**（QEMU 通道，`--test-threads=1
  --nocapture`）——与 §1.3 的 W0 基线**逐数字相同**。
  arm64 没有 JIT 矩阵（清单 §1.3 也只列两条）；它的 QEMU 通道 4 passed。）
- [~] `lint` 三谱零结论；`--refs`/`--bits`/`--suggest` 清单只减不增。
  （**零结论**：三谱 `forge-isa lint` 全部"干净（无 lint 结论）"。
  **清单**：三张清单由 `tests/lint_shipped.rs` 的快照钉住且全绿 ⇒ 与**当前快照**一致；
  但**相对 W0 有增长**：`--bits` riscv64 3→**13**、arm64 67→**262**（W2 的口径变化：form 声明
  但**本指令未绑定**的槽不再算该指令的覆盖）；`--suggest` riscv64 28→**31**（W5 族合并之后）。
  ⚠ **`CLAUDE.md` 里记的 3/67/28 已过时**，待随文档面回写。）
- [~] 设计文档 §1.2 的 D1–D6 逐条给出实测证据（节数、机制数、位置语义数、后门清除条数）。
  （**2026-10-08 清点** ——
  **D1**：顶层键 = `meta` + **16** 个可选（`include`/`override`/`encoding`/`reg`/`conventions`/
  `operand`/`enum`/`form`/`inst`/`family`/`lower`/`asm`/`pred`/`lowering`/`pattern`/`machine`/
  `vectors`，设计期是 21）；并存机制：`[[templates]]` **0**（W5 撤）、
  `[conventions.*]` 只剩 **2** 个子节（`modrm`、`prefix_scan`，设计期抱怨过"七子节"）、
  `ref` 多态仍在（`--refs` 51 条）、变体门 **29** 处 `only_variants`；
  **位置语义 = 0**（六个魔法串 `"rr"`/`"rr_rev"`/`"rr_src2"`/`"rm_mem"`/`"rm_memref"`/`"ext"`
  在三谱里**只出现在注释**（`x86.toml:27` 的历史说明），真残留 **0**；
  `asm` 内联声明 `{i:[槽:角色]}` **0** 处）。
  **D3**：`[family.*]` = **112** 节（唯一复用机制）、`vary` **38** 处、
  三谱 `[[vectors]]` = **505** 条（x86 154 / riscv64 152 / arm64 199） —— 谱内向量已取代手抄
  黄金表；`[[lowering]]` = **281** 条（x86 174 / riscv64 94 / arm64 13）。
  **D4 逐条清点（前半：#1–#10；2026-10-08）** ——
  ✅ **已清除**：#4 `default_prefix_scan()`（函数已不存在，前缀表 = 谱里的
  `[[conventions.prefix_scan]]`）、#9 `[spill.FPR<bytes>]`（谱里 `[machine.spill.*]` 9 处，
  `[spill.GPR]` 只剩 `arm64.toml:4797` 一句注释）、#10 手写 reloc 补丁器（现由
  `machine.rs::gen_reloc_patcher` **生成**并由 `integration.rs` 发进 `TargetMachine`，
  守卫 `tests/reloc_patcher_generated.rs`）。
  🚧 **仍在**：#1 `has_jal = has_jmp && !is_prefix_scan()`（`codegen/lowering.rs`）、
  #3 尾声跳转形态（`codegen/frame.rs` 的 `if has_jump && model.is_prefix_scan()`）、
  #5 `enum PrefixEffect` + `fn extra_bytes`（x86 闭集）、
  #6 `codegen/vlen.rs::prefix_cond_ts`（按字节枚举）、#7 REX2 **半**（`0xD5`+`rex2` 已进谱表，
  但虚拟 `0F` 归一与 payload 位移仍在代码里）、#11 `force_disp_base` 魔数（前提已证不成立）。
  ❓ **未定**：#2（与 #1 同属"降级下放"，未见独立证据）、#8 `{off}`/`{alloca}`（本次只证实
  `{off}` 在三谱 **asm 模板**里是普通占位符，如 `ldur {dst}, [{base}, #{off}]`；
  lowering **发射行**里是否还有第二套内存写法未逐条看）。
  **D4 逐条清点（后半：#11–#20；2026-10-08）** ——
  ✅ **已清除**：#13 `FPR(8)`/`["rsp","sp"]`/`["rbp","fp"]` 惯例回退（代码里只剩**文档注释**
  回顾"历史实现回退到 `GPR(8)`…"，值一律来自 `[machine]`/`[reg.*]`，另有
  `no_hardcoded_widths` 守卫）、#17 `[conventions.bitfields]`+`opcode_field`+`operand_fields`
  三处往返（已并为 form 内就地 `fields` = W2）、#18 八个一次性开关（`zr31`/`byte_reg`/
  `arrangement`/`symbols`/`require_symbol`/`imm_fns` 等只剩**文档注释**里的"原 X = true"
  ⇒ 已被 `reg.rex_required`/`arrangement` 表/`symbol{}` 等新键取代 ✅）、
  #20 `[[instructions]].when` 死键（随 W10 清零）。
  🚧 **仍在**：#12 `moves.rs` 的 `if a.width >= 128 { "vec_mov" } else { "fpr_mov" }`
  （**2026-10-08 复核：不是"改成分派就行"** —— 这个 128 是**按值向量的 ABI 阈值（16 字节）**，
  属于**宿主 ABI 规则**却被写进 DSL 的能力视图（`MoveTable::capabilities` 折算给
  `abi_view`/`role_bits`）。真按"所选指令的寄存器组"派生会让 64 位 VEC 搬移也报成
  `vec_mov` ⇒ 语义变化。正解是**能力视图只报寄存器组与位宽**，阈值留给宿主
  （`AbiRules`）⇒ 设计级改动，与本轮的小活无关）、#14 `opsize` 四态与 `"sN"`（16 处活用法）、
  #15 `modrm` 的位置缺省（`Modrm::resolve` 里 `rm 缺省 = 最后一个操作数`）、
  #16 `class`/`classes` 双形（x86 里 `classes = [...]` 2 处；**2026-10-08 复核：也是设计级** ——
  设计案把它与"`kind` 隐式推导"绑在一起，落点是"**删除 `[[operand_slots]].class`/`classes`**，
  字段类型直接写组名或联合" ⇒ 是新面，涉及三谱 29 处 `class` + 2 处 `classes`，
  不是把两个键合并一下）、
  #19 `lint.rs:166` 仍同时接受 `["op", "ops"]` 两种拼法（**一改就完的小活**）。
  **#19 已修（2026-10-08）**：宿主 op 表现在**只认 `[[op]]`**，复数 `[[ops]]` 报错并点名正确拼法；
  守卫 = `lint.rs::host_ops_rejects_plural_spelling`（负向）。
  🚧 **半**：#11 `force_disp_base` 魔数（校验被 `!= PrefixScan` 挡在门外 ⇒ 前提要重定）。
- [x] 度量对比：三谱行数、生成物 token 规模、`cargo check -p forge-codegen` 时间。
  （**B1 基线（2026-10-08 本机实测）**：三谱行数 x86 **4877** / riscv64 **3427** /
  arm64 **5178**（TOML 合计 **397,381 B**）；生成物（`tm` 全套，`$OUT_DIR` 的
  `forge_gen_<isa>_<hash>.rs`）x86 **6,068,024 B** / riscv64 **2,483,285 B** /
  arm64 **6,507,583 B**；`cargo check -p forge-codegen` 改源码后的增量 **30.7 s**、
  改谱（`touch` 触发重新预生成）后 **18.3 s**。
  ⚠️ **W0 对照不可回溯**：§1.3 记的 W0 Phase-2 日志本身就受污染（在 W1 已改代码后才跑），
  且忠实的 W0 对比需要**改动前那棵树**（v18 面）。可复现配方（留待需要时）：
  先 `git worktree add ../cf-w0 <S0 冻结提交>`（按 §1 记录的日期在 `git log` 里定位），
  再在该 worktree 里跑同样两条命令；
  **单次墙钟数字噪声大**（本轮两次就差了 12 s）⇒ 只当量级参照，别当回归门禁。）
- [x] 文档三方针一致：`schema.rs` ↔ `dsl/model.rs` ↔ 参考文档键表 ↔ `isa-dsl.schema.json`。
  （`schema_guard` **9/9** —— 本会话每片都跑。）
- [x] 所有新增/改动文档 `npx markdownlint-cli2 <文件>` 到 0 error。
  （本会话每次提交前都对改动文件跑；本轮 6 份改动文档 **0 issue**。）

**快照 #4（2026-10-11）** —— 提交 `a63f7493`（全仓格式化重排 + schema 回发射器格式 + 生成物每条指令都有
自动属性行 + IR 公开 API/操作码/`FunctionBuilder` 注释补齐）触发 CI run
[38142845046](https://github.com/Yuanzhumoyu/code_forge/actions/runs/38142845046)：**11 个 job，9 ✅ / 2 ❌**。

| job | 结论 | 备注 |
| --- | --- | --- |
| Test (Windows) | ✅ | 本轮工作区测试；`jit feature` 全套由本机 `cargo test -p forge-codegen --features jit`（exit 0）复现 |
| Test (Linux) | ✅ | 含 arm64 / riscv64 QEMU 矩阵**真跑** |
| forge-rustc (e2e, Windows) | ✅ | stage A + M4 并行 + `cargo-forge cli_tests` |
| forge-rustc (check) / Format / Clippy / Docs / Coverage / Benchmarks (check) | ✅ | Clippy 两条门槛（`--all-features -D warnings` 与 `forge-ir --no-default-features`）零告警 |
| **forge-tests (nightly / forge-rustc)** | ❌ → 已修 | 失败步 = `cargo test -p forge-tests --features nightly`。**根因**：`nightly` 开的是 **crate 级** `feature(rustc_private)`（`forge-tests/src/lib.rs:7`）⇒ 该 crate 的**每个 test 目标**都被带上 rustc_private ⇒ 纯汇编目标（`asm_parse`/`asm_arrangement_pairs`/`asm_provenance`）撞 sysroot 与 crates.io 同名包：13 条 `error: cannot satisfy dependencies so X only shows up once`（`hashbrown`/`cfg_if`/`std_detect`/`panic_unwind`/`rustc_demangle`/`windows_link`…）。**证据**：日志里 **lib 编得过**、报错的只有 test 目标。**处置**：CI 步收窄为 `cargo test -p forge-tests --features nightly --lib`（nightly 面都在 lib 里；端到端由 `forge-rustc (e2e, Windows)` 与 workspace 两个 job 覆盖）。 |
| **Test (macOS)** | ❌ → 已修 | 失败步 = `cargo test --workspace --exclude forge-rustc`，**唯一失败用例** `crates/tools/forge-tests/tests/asm_exec.rs:62`：macOS 上 x86 原生执行不支持（代码按 Windows x64 ABI 生成）、QEMU 未安装 ⇒ 三通道全缺（`ASM-EXEC-SUMMARY ran=0 skipped=3`）⇒ 旧断言"一条都没跑成 ⇒ 失败"**必然红**。**处置**：改为"**有可用通道却一条没跑成才失败**"（与 JIT 矩阵"缺通道不假绿"同口径）；本机实测 Windows 通道仍真跑 `ran=10 skipped=0` ✓ 门禁未削弱。 |

- 失败**日志**本轮由用户提供（`target/tmp/logs_103341893252/`，113 个文件）⇒ 上面两条都定位到**具体断言/具体错误**；
  GitHub 的 job 日志接口对非管理员仍返回 `403 Must have admin rights to Repository`，所以此前只能记到 job 粒度。
- 两处修改都是本机可验证的那一类：macOS 那条由本机跑通 `asm_exec`（`ran=10 skipped=0`）背书；CI 那条依据
  日志里"lib 编得过、只有 test 目标报错"的事实收窄（本机无 nightly + `rustc-dev`，无法整段复现 —— 这是环境限制）。
- 本机对同一提交的等价验证：`cargo test --workspace --exclude forge-rustc -j 2` exit 0、
  `cargo test -p forge-codegen --features jit` exit 0、`cargo fmt --all -- --check` 0 文件、
  两条 clippy CI 门槛 exit 0。
- `forge-rustc-vec_push-plan.md` 的 §4 验收 4/4 达成 ⇒ 该方案**已归档**到
  `docs/archive/`（本轮同一批清理的一部分）。

---

## 15. 中止条件

出现下列任一情况，**停下来回到设计文档**，不要硬推：

- **中止判据（不是任务，永不打勾）** 字节等价无法保持，且原因不是迁移脚本笔误（例如新语法表达不了某个现有编码）。
- **中止判据（不是任务）** 某片门禁连续两轮不过，且根因是设计（不是实现）。
- **中止判据（不是任务）** 出现必须"为某个 ISA 加编译器分支"才能表达的写法 ⇒ 说明"slots × patterns"层没设计够，回到设计文档补。
- **中止判据（不是任务）** 迁移人工判断量超出设计文档 §10 的 40% 估算 1.5 倍以上 ⇒ 重新评估是否拆分或缩小范围。

## 附：W5 第 ⑥ 步（删 `[[templates]]` TOML 面）的精确计划（2026-10-08 实测）

> ✅ **已完成（2026-10-08 复核）**：`model.rs` 的 `templates` 已是 `#[serde(skip, default)]`
> （TOML 里写 `[[templates]]` 被未知键守卫拒绝）；`.rs` 夹具里已无 TOML 形态的 `[[templates]]`
> （只剩注释与 `lint_shipped` 的"发行谱不得再用它"守卫）；schema/根键/文档键表均已同步。
> 下面保留原始计划作为记录。

**目标**：谱里写 `[[templates]]` 被拒绝；`Template`/`TemplateRow` 仅作 `family` 的**内部降级目标**。

1. `model.rs`：`templates` 字段改 `#[serde(skip, default)]`（键消失 ⇒ `deny_unknown_fields` 会拒绝旧写法）；
2. `schema.rs`：root 的 optional 去掉 `"templates"`；`docs/reference/isa-dsl.md` 速查表 root 行同步；
   重生成 `isa-dsl.schema.json`；
3. **连带面（本次没做完的原因）**：`.rs` 夹具里的 `[[templates]]` 全要改写——
   `template_tests.rs` **17 处**（多为多行形态：`[[templates]]\nname = "X"\n…`）、
   `generality_guard.rs` 2 处、`schema_guard.rs` 2 处、`diag_matrix_tests.rs` 3 处（含**诊断期望**）、
   `tests.rs` 1 处。多行形态的替换要按"块"处理（表键 + 去掉 `name` 行），且**无 `name` 的夹具**
   要取首行 `inst` 加 `_FAM`（族名与指令名同池）；
4. `diag_matrix_tests.rs` 的那条期望（"引用名与指令名冲突"用例）要**按新面改写**，不是纯替换；
5. 验证：`cargo test -p forge-isa-dsl`（快）→ `cargo test --workspace --exclude forge-rustc -j 2`。

## 附：W5 第 ⑤ 步（逐族合并）现状与打法手册（2026-10-08）

**已完成**：x86 **16 / 55 组**（`[[lowering]]` 块数 **191 → 181**）；riscv64 / arm64 的"单一差异"型 = 0。

**已证明的打法**（每组合并后跑 `validate` + `lint` + `forge-tests`（JIT 矩阵 58 passed）；
批末跑整仓门禁 `-j 2`；绿后提交，不碎片化）：

1. **两条都显式写 `when`**（不猜默认值）+ **insts 只差一段** ⇒
   `vary = { <when里的键> = [v1, v2], sfx = ["左", "右"] }` + 模板里 `{sfx}`；
2. 差异段的求法：公共前缀 `i` + 公共后缀 `j` ⇒ 中间即差异段（要求等长、非空、互不相同）；
3. **单 token 守卫（必须）**：差异段不得含 `, " [ ] { }`——否则差异是**跨元素**的，
   替换会拼出多元素列表（实测把 `Cmpxchg` 写成 `sfx` 长 20 的列表，`validate` 拦下）。
   带守卫后 riscv/arm64 各 0 组合并（矿脉枯竭，**不硬凑**）。

**剩余打法（未实现）**：

- **多键 zip**：两条规则差**多处**时，逐处差异各求一个替换键（`vary = { K=[…], f=[…], g=[…] }`）；
  需要"逐处差异对齐"的实现，同一单 token 守卫可复用；
- **宽度分派清理 = 目标第 (4) 项**：riscv64 的 28 组多为 `when = { eq = ["rs1_width", …] }`；
  应先补"按声明类型选行"的重载解析，再删能被覆盖的 `when` 规则；
- **注释块**：x86 `Fstore` 等规则前有长注释，脚本要跳过注释再配对。

**回归节拍**：每簇 `forge-tests`（~35 秒）；每 ~5 簇一次整仓门禁；`--suggest` 计的是
**展开后**规则数 ⇒ **不能**当进度尺，用 `[[lowering]]` 块数。

### ⑤ 打法手册补记：机械可合并部分**已榨干**（2026-10-08，实测）

- **多键 zip 已实现**（两条差多处 ⇒ 逐处对齐、每处一个替换键 `s0`/`s1`…；元素按 `", "` 切分；
  等长 + 单一连续差异段 + 单 token 守卫；任一不满足整对原样保留）。
- 实测：对 x86 剩余显式-`when` 对跑一遍 ⇒ **合并 0 组、跳过 3 组**。
  ⇒ 靠"同 op + 显式 `when` + token 替换"能拿的**已全部拿到**：累计 **16 / 55 组**（块数 191 → 181）。
- ⇒ **不要再重试这条路线**；剩余 ~39 组必须换手段：
  ① 缺显式 `when`（靠默认值）⇒ 先**证明**默认值（读 `PRED_ATTRS` 语义）再合并（中风险）；
  ② 元素数/结构不同 ⇒ 真重写（高风险，需逐条人工）；
  ③ `rs1_width` 类宽度分派 ⇒ 属目标第 (4) 项，需先实现**按声明类型选行的重载解析**（新功能）。

### 目标第 (4) 项（宽度分派清理）的范围量化（2026-10-08 实测）

三谱里 `when = { eq = ["键", 值] }` 形式的分派规则总数与按键分布：

| 谱 | 总数 | 按键分布 |
| --- | --- | --- |
| x86 | 47 | `rd` 13 · `elem` 11 · `rs1_width` 9 · `rs1_vec` 4 · `rd_vec` 4 · `imm0` 3 |
| riscv64 | 47 | **`rs1_width` 29** · `cond` 10 · `rd` 8 |
| arm64 | 6 | `rs1_width` 6 |

**宽度类**（`rs1_width`/`rs2_width`/`rd_vec`/`rs1_vec`）合计 **52 条**（x86 17 · riscv64 29 · arm64 6），
其中 riscv64 的 `rs1_width` **29 条**是最大的一块（该谱共 118 个 `[[lowering]]` 块 ⇒ 约四分之一）。

⇒ 第 (4) 项的真实工作量 = **先实现"按声明类型选行"的重载解析**（否则删不掉这些分派：它们是
目前唯一的宽度选择机制），再逐条删除能被重载覆盖的规则，**每条都要跑一次 JIT 矩阵**。

### ⑤ 合并器的**双守卫**（2026-10-08，两条都是实测踩出来的）

1. **单 token 守卫**：差异段不得含 `, " [ ] { }`——否则差异跨元素，替换会拼出多元素列表
   （实测把 `Cmpxchg` 写成 `sfx` 长 20 的列表，`validate` 拦下）。
2. **单行守卫**：`insts` 必须是**单行**——多行时正则跨行匹配，替换会把换行带进单行
   （实测 `riscv64.toml:2596` TOML 解析失败，`validate` 拦下）。

带双守卫重跑三谱（多键 zip）：**x86 0 命中（跳过 3）· riscv64 0（跳过 1）· arm64 0** ⇒
"同 op + 两条显式 `when` + token 替换"这条路**三谱全枯竭**（累计仍为 x86 16/55）。
**注意**：多行 `insts` 的对若将来要合并，需按"逐行对齐"另写实现（不能复用单行版）。

### ⑤ 机械空间**确认耗尽**（2026-10-08，三种变体全试过）

同一套守卫下实现了三种合并器变体，对 x86 剩余"两条都显式 `when`"的对全部返回 **0 命中**：

| 变体 | 适用 | 实测 |
| --- | --- | --- |
| 单差异段 | 单行 `insts`，只差一段 | 合并 16 组（累计） |
| 多键 zip | 单行 `insts`，差多处（逐处一个键） | x86 0（跳过 3）· riscv64 0 · arm64 0 |
| 多行逐行对齐 | 多行 `insts`，逐行对应 | x86 **0（跳过 4）** |

⇒ **结论**：⑤ 里"同 op + 两条显式 `when` + 文本替换"能拿的**已 100% 拿到**（x86 16/55，块数 191 → 181）。
剩余组要么缺显式 `when`（需先证明默认值），要么结构不同（需人工重写），要么是 `rs1_width` 宽度分派
（= 目标第 (4) 项，需先做重载解析）。**不要再写第四种文本合并变体**——已验证无空间。

### 目标第 (4) 项：x86 剩余 46 条宽度分派的打法与两个坑（2026-10-08）

**现状**：riscv64 已删 **24 条**（118 → 94 块）；x86 剩 **46 条**（`Load`/`Store`/`Fextend` 类），arm64 6 条。

**坑 1：分派键多样** ⇒ 已加 `[[lowering]].dispatch`（缺省 `rs1_width`）：

| 键 | 单位 | 用例 |
| --- | --- | --- |
| `rs1_width` / `rd` | **位** | `Store`(8) · `Fstore`(32) · `SaddSat`/`SsubSat`/`Bitreverse` |
| `rd_vec` / `rs1_vec` | **字节** | `Load`/`Store` 的向量档（8/16/32/64） |
| `elem`（复合 `and`） | 枚举 | `Sitofp`/`Fptosi`/`Uitofp`/`Vextract` |

⇒ **`word_size` 的单位必须与 `dispatch` 一致**（位键写位、字节键写字节）。校验期加一条一致性检查更稳。

**坑 2：档位常多于 2** ⇒ `Load` 的 `rd_vec` 有 4 档（8/16/32/64）。当前折叠器只配对（两两）⇒
会留下残档。要让重载吃下**整组**，应在同一 `group` 下把**所有** `word_size` 不同的成员登记齐
（脚本改为"按 group 收集全部成员、逐档生成 `when`"），而不是两两折叠。

**打法（下批）**：

1. 给 `MOVSD_R_MEM`/`MOVUPS_128_R_MEM`/`VMOVUPS_256_R_MEM`/`VMOVUPS_512_R_MEM`（及 store 侧）
   补 `group = "<族名>"` + `word_size`（**字节**：8/16/32/64）；
2. 对应 lowering 规则加 `dispatch = "rd_vec"`/`"rs1_vec"` 并整组折叠；
3. 逐组 `forge-tests` → 整仓门禁 → 提交。

**注意**：折叠器当前会**丢弃 `dispatch`**（只保留 op/when/insts）⇒ 扩展它，或在折叠后手工补回。

#### 第 (4) 项：x86 向量 `Load` 折叠**未成功**（2026-10-08，已回退）

尝试：给 `MOVSD_R_MEM`/`MOVUPS_128_R_MEM`/`VMOVUPS_256_R_MEM`/`VMOVUPS_512_R_MEM` 补
`group = "load_vec"` + `word_size`（字节 8/16/32/64，与既有 `rd_vec` 规则一一对应），
把 4 条 `when = { eq = ["rd_vec", N] }` 折成一条 `dispatch = "rd_vec"` 的宽度无关规则
（`validate` + `lint` 干净、块数 180 → 177）——但 **JIT 矩阵崩**（`STATUS_ILLEGAL_INSTRUCTION`）
⇒ 展开出的 4 条与原来的 4 条**并不等价**。

**已排除**：`word_size` 取值（与旧规则逐一对应）、`dispatch` 键名（`rd_vec`）、规则顺序（原位展开）、
幂等性（已有该键叶子则不再展开）。

**待查**（下轮第一件事）：把这条规则单独放进最小夹具，打印**展开后**的 4 条规则（`when` + `insts`）
与旧 4 条逐字对照 —— 差异点就是真因。在此之前**不要再动** x86 的向量组。

#### 第 (4) 项：x86 折叠失败**根因已定位**（2026-10-08，二分实测）

**二分**：只给 4 条向量 load 指令补 `group` + `word_size`（**所有 lowering 规则一条未动**），
⇒ `validate` + `lint` 干净、块数不变 —— 但 **JIT 矩阵仍崩**（`STATUS_ILLEGAL_INSTRUCTION`）。

⇒ **根因不是折叠**，而是**组属性本身**：一旦这些指令有了 `group`，**既有**那些"引用它们、
但没有宽度叶子"的规则就会被降级重载**自动展开**（补上 `rd_vec`/`rs1_width` 叶子）⇒
本应"对任意宽度都适用"的规则变成"只在某个宽度适用" ⇒ **覆盖丢失** ⇒ 行为改变。

**正确设计**：展开必须**显式 opt-in** —— 只有规则**自己写了 `dispatch = "<键>"`** 才展开；
缺省（没有 `dispatch`）时**一律不展开**。当前实现是"没有该键的叶子就自动展开"（过渡期的权宜）。

**改造清单（下轮）**：

1. `expand_lowering_overloads`：把触发条件从"没有该键的叶子"改为"**有 `dispatch`**"；
2. 给**我已折叠的 22 条 riscv64 规则**补 `dispatch = "rs1_width"`（机械，一条一行）；
3. 重新跑 riscv64 的 JIT（应仍绿）与整仓门禁；
4. 之后才重做 x86 向量组（此时"只加组属性"不再有副作用）。

**期间不要动** x86 的向量组。

#### 第 (4) 项：x86 store 侧遇到**分组冲突**（2026-10-08，已回退）

现象：给 `MOVSD_MEM_R` 补 `group = "store_vec"` 后 **TOML 解析失败**（重复键）——因为它
**已经在** Fstore 族里（`group = "fstore_x86"` + `word_size = 64`，上一轮为折 `Fstore` 加的）。

**结构性约束**：指令的 `group` 只能有**一个**，而同一指令可能被**两个不同 op** 用不同分派键引用：

| op | 分派键 | 单位 | 参与的指令 |
| --- | --- | --- | --- |
| `Fstore` | `rs1_width` | **位** | `MOVSS_MEM_R`(32) · `MOVSD_MEM_R`(64) |
| `Store` | `rs1_vec` | **字节** | `MOVSD_MEM_R`(8) · `MOVUPS_128_MEM_R`(16) · `VMOVUPS_256_MEM_R`(32) · `VMOVUPS_512_MEM_R`(64) |

⇒ 两者共用 `MOVSD_MEM_R`，但**键与单位都不同** ⇒ 一个 `group`+`word_size` 表达不了。

**待定方案（下轮先定，再动手）**：

1. **按 op 分组**：把 `group` 的意义限定为"同一 op 的多档引用名" ⇒ 需要新的键（如 `groups = { fstore = …, store_vec = … }`）
   或按 op 命名空间隔离（`group` 只在同一 op 的规则里解析）；
2. **或**给指令加"按 key 的 word_size"（`word_sizes = { rs1_width = 64, rs1_vec = 8 }`），
   重载按 `dispatch` 取对应值；
3. 在方案定下来之前，**不要再动** x86 的 store 侧。

#### 第 (4) 项：x86 剩余 13 组的只读评估结论（2026-10-08，脚本 `target/tmp/assess-width.ps1`）

按 (op, 非宽度键, 宽度键) 分组后的判定：

| 组（op） | 键 | 档位 | 判定 |
| --- | --- | --- | --- |
| `Load` | `rd` | 8/16 | 未成组（头名指令缺 `group`+`word_size`） |
| `SaddSat` / `SsubSat` | `rs1_width` | 32/64 | **跨段差异**（差异在序列中段 ⇒ 行首段守卫正确拒绝） |
| `Vextract`（4 组） | `rs1_width` | 256/256/512… | 档位重复 ⇒ **并非宽度对**（差异在 lane 常量） |
| `Vinsert` / `Vbroadcast` | `rd` | 256/256 | 同上（非宽度对） |
| `Vconst`（3 组） | `rd` | 64/128/256/512… | 同上（差异在常量池） |

⇒ **结论**：x86 剩余项**基本不适用**当前重载机制 ——
① 跨段差异（溢出/饱和/位反转那类**算法不同**的序列）；
② 复合键（`rs1_width` × `elem` 的 `Sitofp`/`Fptosi`/`Uitofp`）；
③ 非宽度对（`Vextract`/`Vconst`/`Vinsert` 的"同宽度不同常量" —— 被我按 `rd` 误聚）。

**处置**：**标记为"不适用重载"**，不再逐组尝试（要覆盖它们需要新机制：多段同步替换 / 多键联合分派 / 常量维度分派 —— 收益低于成本）。

**剩余可做的**：转 **arm64 的 6 条**（都是 `rs1_width`，形态可能更简单）。

#### 第 (4) 项**收尾结论**（2026-10-08）

**已删 37 条**（riscv64 24 · x86 7 · arm64 6），全部逐批跑过 JIT（有矩阵的谱）或整仓门禁闭环守卫（arm64）：

| 谱 | `[[lowering]]` 块数 | 起点 |
| --- | --- | --- |
| x86 | **174** | 181 |
| riscv64 | **94** | 118 |
| arm64 | **13** | 19 |

**剩余 15 条按原始 W5 定义"其余保留"** —— 逐组评估（脚本 `target/tmp/assess2.ps1`）证明它们
**不能**由当前重载机制覆盖：

1. **跨段差异**：`SaddSat`/`SsubSat`/`Bitreverse`/溢出族 —— 两侧是不同算法的序列，差异不在行首段；
2. **复合键**：`Sitofp`/`Fptosi`/`Uitofp`（`rs1_width` × `elem`）、`Vextract`（`rs1_width` × lane）；
3. **非宽度对**：`Vinsert`/`Vbroadcast`/`Vconst` —— 同宽度、差常量/lane（评估脚本的"行首段差异"判据
   会把它们误判为可折，已在脚本注释中标明该判据**必要但不充分**）。

**要覆盖它们需要新机制**（多段同步替换 / 多键联合分派 / 常量维度分派）——收益低于成本，**不做**。

#### ⑥ 夹具迁移：脚本必须带**后置断言**（2026-10-08 实测）

批量脚本首轮只吃"原始字符串式"夹具，且出现过 `rows = [ ]` **空数组**的坏转换（把行内容吃掉却没生成条目，
连带 8 个 `lint` 测试红）⇒ 当轮**整批回退**。

**下轮做法（单文件 + 立刻验测）**：

1. 一次只改**一个文件**；
2. 脚本加**后置断言**：若源里出现过 `[[templates.rows]]`，转换后的 `rows = [` 块必须**非空**（否则报错不落盘）；
3. 停止条件必须含**原始字符串终结符 `"#`**（否则吞掉夹具之后的 Rust 代码）；
4. 改完立刻跑 `cargo test -p forge-isa-dsl --lib <模块>`，绿了再换下一个文件；
5. **数组式（转义字符串）夹具**（`"[[templates]]", "[[templates.rows]]", "inst = …"` 逐行元素）
   需**手工**迁移（要先把行元素合并成一条 `rows = [ { … } ]`）——脚本不适用。

### W6 现状与两个"表达不了"的结论（2026-10-08 实测）

#### 已落地（三谱等价，门禁全绿）

| 项 | 内容 |
| --- | --- |
| `[lower.Jmp]` / `[lower.Br]` | 三谱各自声明，且**必需**（有 `roles=["jump"\|"branch"]` 却缺声明 ⇒ 生成期报错） |
| 新符号值 | `{true}` / `{false}` / `{target}`（块号）· `{cond}`（条件寄存器）· `{cc_inv}`（逆条件码） |
| 死回落清理 | `jal_f.len()>=2`、`beq_f.len()>=3`、`test_f.len()==2`、`jcc_f.len()==2` 与 `eq_code` **整条删除** |

**结论 A：`[lower.Ret]` 用现有占位符体系表达不了**

`multi_ret_body`（`lowering.rs:477` 起）不是一条搬移序列，它做三件"读宿主数据"的事：

1. 从 **ABI 布局**取返回寄存器：`ctx.conv.layout().ret` 的 `RetPlace::Pair{lo,hi}` / `Group{regs}`；
2. 按**bank** 分派搬移：`moves.dispatch(&fpr_want, …)`（整数走 GPR 搬移、浮点走 FPR 搬移）；
3. sret/宽向量返回的专门路径（`__vbytes`/`store_body`）。

⇒ 要谱面化，得先引入**宿主数据的符号值**（如 `{ret_reg0}` / `{ret_reg1}`，值来自 `RetPlace`）
——那是新机制，收益低于成本，**暂不做**（`Ret` 继续留在编译器里，安全闸照旧关着）。

**结论 B：`frame.rs` 的尾声跳转需"编码器宿主"的序列发射**

`frame.rs:167` / `:182` 有同一类字段个数猜测（`jump_f.len() >= 2`），但它所在函数的发射 API 是
`encoder.encode(&inst, reg_map, sink)`（**不是** `__pack.push_inst(…)`）⇒ `[lower.Jmp]` 的序列形态
**无法直接复用**。要谱面化需引入第二种序列宿主（"以编码器为宿主的序列发射"） ⇒ 与结论 A 合并成
**一个设计片**（"终结/尾声序列的两种宿主：pack 与 encoder"），单独排期。

**W6 剩余（按性价比排序）**：emit 表达式（`{imm0 - 4}` 等）· `temps = [...]` 显式临时 ·
第四形态夹具 ISA · 上述两个设计片。

#### W6「删 20 余个派生 token」的**实测结论：不做**（2026-10-08）

逐条查了这批 token 的 ctor（`placeholder.rs`），**几乎都不是纯算术**：

| token | 真实 ctor | 能否用表达式等价替换 |
| --- | --- | --- |
| `imm0_sub4` | `ctx.current_immediates.first()….saturating_sub(4) as i64` | **饱和减法**（`{imm0 - 4}` 是回绕） |
| `iconst_hi20` / `_lo12` / `_hi32_hi20` / … | `let __v = ctx.constant_pool…resolve_int(…)` + 算术的**多步块** | 常量池解析不是表达式 |
| `fconst_*` | 同上，`resolve_float` | |
| `iconst_c16` | 池 + 16 位截断 | |
| `shufps_imm8` / `_hi` | **辅助函数** `__shufps_imm8(immediates, 0/4)` | 函数调用 |
| `vconst_lo` / `_hi` / `_h2/_h3` | **辅助函数** `__vconst_half(pool, idx, …)` | |
| `seg` / `ph` | **不是占位符**（`vary` 的纯替换变量） | |

⇒ 这些名字承载的是**真实语义**（饱和/池解析/相位），硬换成算术会**改变行为**。
**决定：保留它们**（`{imm0 - 4}` 这类表达式仍作为**新谱面**的表达力保留，但不用来删这些 token）。

⇒ W6 剩余按性价比重排：**`temps = [...]` 显式临时寄存器** → **第四形态夹具 ISA** →
**`Ret`/`frame.rs` 两个设计片**（需宿主数据符号值 / 编码器宿主流）。

### W6 收口核对（2026-10-08）

| 清单条目 | 状态 | 证据 / 理由 |
| --- | --- | --- |
| 新增符号值 `{true}`/`{false}`/`{target}`/`{cc}`/`{cc_inv}` | ✅ | 全部注册；`{cc_inv}` 另有端到端生成测试 |
| `{cc_inv}` 由 `[enum.cond]` 反查（替换硬编码 `4u8`） | ✅ | `ir_inverse_cond` + 生成期按 `dispatch`/`cond_ir_codes` 取字面；`4u8` 已删 |
| 三谱写 `[lower.Br]`/`[lower.Jmp]` | ✅ | x86/riscv64/arm64 各写；**缺声明即生成期报错**；与旧硬编码**等价**（JIT + 汇编三档 + 矩阵计数不变） |
| 删除三处"按字段个数猜 ISA" | ✅ | `lowering.rs` 两处 + `frame.rs` 一处，**全部清除**（后者改按**声明数据**判定） |
| emit 表达式落地 | ✅（能力） | `{imm0 - 4}` / `{iconst >> 12 & 0xfffff}` 已可用（生成期展开）+ 单测 |
| 删 20 余个派生 token | ⛔ **不做** | 逐条查 ctor：`saturating_sub`、常量池解析多步块、辅助函数调用、`vary` 替换变量 ⇒ 换算术**不等价** |
| `temps = [...]` 显式临时 | ✅（可选） | 声明即强制（未声明即报错点名）；不声明沿用旧扫描 —— 避免 1222 处机械重写 |
| 第四形态夹具 ISA | ✅（改写） | "同谱两分支形态"被 DSL **规则禁止** ⇒ 改为"谱面点名**非角色**指令"，锁死"形态只由谱面决定" |
| `[lower.Ret]` | ⏸ **不做** | `multi_ret_body` 读**宿主 ABI 布局**（`RetPlace::Pair/Group`）+ bank 分派搬移 ⇒ 要新机制 |
| 门禁：§2 全绿 + 三矩阵计数不变 | ✅ | 每片都跑整仓 `-j 2`（exit 0 / 0 编译错误 / 0 失败二进制） |

**W6 判定**：清单里**可做的都做了**（三条猜测清除、终结序列唯一来源 = 谱面、新符号值/表达式/显式临时齐备）；
**两项以证据判定不做**（派生 token 删除、`Ret` 谱面化），理由均已固化。

---

### W7 收口核对（2026-10-08）

| 清单条目 | 状态 | 证据 / 说明 |
| --- | --- | --- |
| `asm` 支持可接受写法列表（第 0 条 = 渲染形态） | 完成 | `model::AsmList`（`Deref`/`AsRef`/`PartialEq`/`Display` 让 43 个读取点只改 3 处）；解析按列表序尝试（`gen_assemble_try_tok` 每条写法各生成一次，去重键带写法）；`gen_scan_probe` 认全部写法的首字面段 |
| 反例验证：同助记符两条写法都能匹配 ⇒ lint 报歧义 | 完成 | 可选档 `--ambiguity`（`LintOpts.check_asm_ambiguity`，默认关）；判据是“助记符 + 操作数类”的代理 ⇒ 偏粗（x86 实测 35 处）⇒ 与其它可选档同定位：评审清单，不是错误判据 |
| 占位修饰 `{name?}` / `{name:affix}` | 完成 | 语法层认四种写法；`affix` 渲染（`+` 对已带 `-` 抑制）+ 解析（`__eat_lit` 失败回退 ⇒ “吃了也算”）；`{n?}` 渲染（值 0 ⇒ 连同紧邻字面词缀一起不输出）+ 解析（缺席绑 0，仅 `kind = "imm"` 槽，`validate_segs` 拦非 imm） |
| `[lower.*].emit` 的内存操作数统一组件解析 | 完成 | 偏移符号（`{off}`/`{alloca}`）改取**占位符注册表**（`ctor_for`）⇒ 删掉 `integration.rs` 手抄的第二份语义；实测该语法只有 x86 两处在用 |
| `[asm.immfn.*].texts` 多拼写 | 完成 | `texts` 能力（多拼写共享一个 `expr`、一个下标：值语义单点）＋ arm64 折叠 **98 → 49** ＋ 新居 `[asm.immfn.<名>]`（表头即名字）＋ **55 条**搬迁 ＋ 撤旧键 |
| `[asm.pseudo.*]` 迁移（x86 20 / riscv 2） | 完成 | 新居 `[asm.pseudo.<名>]`（表头即名字）＋ **22 条**搬迁（含夹具 2 条）＋ 撤旧键；与旧面同名会报错 |
| 尺寸关键字归属 `[operand.mem].size_words` | 完成 | 旧键 `[conventions.mem].size_keywords` 撤除（`serde(skip)` 内部载体 + `INTERNAL_FIELDS`）；三谱早已用新键 ⇒ 生成物零变化 |
| 删除“紧邻 `{size}` 的字面量不是条件前缀”例外 | 完成 | `mem.rs` 渲染分支删除该特例 ⇒ `{size}` 与其它组件同规则；三谱里只有 x86 用 `{size}` 且位于首位 ⇒ **零行为变化**（无任何测试变化） |
| `{size}` 渲染尺寸关键字 | 机制完成 | 规则：宽度（字节）= `size_words` 下标 ⇒ `1 << i`。`__render_mem(m, size_bytes)` + 关键字表；未知宽度 ⇒ 空串（不谎报）。两处调用点随签名改 |
| 门禁：§2 全绿；`disassemble → assemble` 闭包 | 完成 | 每片都跑整仓 `-j 2`（exit 0 / 0 编译错误 / 0 失败二进制）；三谱 `validate` 通过；汇编棘轮与 `[[vectors]]` 闭环不变 |

**W7 判定**：清单条目**全部落地**。两项留在**谱面/产品层**，不在本清单擅自决定：

1. 要让 **x86 真的打印 `qword ptr`**：x86 的内存访问宽度是**逐指令**的（`movzx` byte/word、`mov` qword…），
   而 `[operand.mem]` 只是槽声明、**不带宽度** ⇒ 需给相关槽/指令补宽度声明，
   或把逐指令的 `data_width`/`opsize` 穿进渲染。当前实现按“宽度未知 ⇒ 空串”处理，**不谎报尺寸**。
2. `[asm.reloc.*]` 属 **W8**（重定位生成 + 删宿主按 ISA 名分发）——**已在 W8 完成**，见下节。

### W8 收口审计（2026-10-08，三次提交）

| 清单条目 | 状态 | 证据 / 说明 |
| --- | --- | --- |
| `[asm.reloc.*]` 谱面 | 完成 | 新居（**表头即重定位名**，键 `semantics`/`operand`/`addend`/`via`）＋ 三谱 **3 条**搬迁（x86 `abs64`；riscv `pcrel_hi`/`pcrel_lo`）＋ 撤旧键 `[[reloc]]`（`serde(skip)` 内部载体 + `INTERNAL_FIELDS`）＋ 夹具 8 处（`e7e3064`） |
| x86 冗余补丁器 | 完成 | `X86RelocPatcher` 与 `CodeSink::finish` 通用路径**逐字节等价** ⇒ 整条删除，x86 走通用路径（`ec97499`）；x86 JIT 矩阵 197/3/0 |
| **生成**重定位补丁器 | 完成 | `machine.rs::gen_reloc_patcher`：定宽 ISA 发 `RelocPatcherImpl`（`decode_partial` → 改 label/reloc 槽字段 → `encode` 重放）；散布位段复用编码器**同一份**字段放置；臂按**槽名**匹配（`AUIPC`/`AUIPC_GLOBAL` 共享编码，解码只还原先声明的那条）；生成物覆写 `TargetMachine::reloc_patcher()`（`66ef8752`） |
| 删宿主按 ISA 名分发 | 完成 | `register_default_reloc_patcher` 删除；`reloc_patcher_for` 退回纯查表（未注册 ⇒ `None` ⇒ 通用路径）；`ensure_registered` 不再注册补丁器 |
| 删两个手写补丁器 | 完成 | `RiscvRelocPatcher`/`Arm64RelocPatcher`（含单测）删除；覆盖迁到**生成物**：`crates/backend/forge-codegen/tests/reloc_patcher_generated.rs` **9 条**（J/U/B 散布位段、hi20/lo12 变换、占位值覆盖、imm26/imm19、不支持 kind、报错不动字节） |
| `via` / `addend` 数据化 | 完成 | `[asm.reloc.<名>].via` 引用 `[asm.immfn.<名>]`（riscv 复用已声明的 `hi`/`lo`）；`addend` 经 `RelocKind::Relative` 的修正字段**参与补丁值**（riscv `pcrel_lo` = 4） |
| 门禁 | 完成 | 每次提交前整仓 `-j 2`（exit 0 / 0 编译错误 / 0 失败二进制，含 riscv64 QEMU 矩阵与 arm64 全套）＋ x86 矩阵 197/3/0 ＋ markdownlint 0 issue |

**W8 判定**：清单条目**全部落地**（含“删宿主侧”这一条——宿主现在只剩 trait + 显式注册表 + 通用路径）。

**一条已知边界（如实记录，不静默）**：生成物靠 `decode_partial` 识别指令 ⇒ 若某位模式先命中一个
**没有该槽**的变体，补丁器**明确报错**而不是静默不动。实测命中例：`imm = 0` 的 `addi` 形状会落到
riscv 自己的 `MOV_RM8_R64`（`addi rd, rs, 0` 的搬运形态）或 `Nop`，两者都没有 imm 操作数。
真实 GlobalAddr 序列的 imm 是负占位值 ⇒ 走 `Addi`，不受影响（riscv64 JIT 矩阵覆盖）。
若将来要让这类位模式也能补，需要加“按常量位识别 + 直接按字段散布写位”的第二条路径（当前不做，属设计取舍）。

---

## 16. v22 人体工学改造：进度与下一步（2026-10-11）

**动机（实测，13 份谱）**：`[inst]` 共 **692** 块、每块**中位 5 个键**，其中 **52%（363 块）**是同形状
`asm+form+match+opcode+ops`；`ops` 行 **660**、`bind` 行 **132**、`modrm` 在发行谱 **0 处**；
同一把 `bind` 键在两条路径上**方向相反**（x86 = 位置→操作数、arm64 = 字段→操作数 ⇒ 后者 13 处被**静默忽略**）。

**目标语法（已批准）**：操作数声明与字段绑定**就地写进 `asm`**——`{名字:槽[:角色]}` 声明、
`{@字段:槽[:角色]}` 绑定、`{@[字段]:槽[:角色]}` 绑定且按**内存形态（MemRef）**走。

### 已落地（有守卫测试）

| 步骤 | 状态 | 证据 |
| --- | --- | --- |
| 抽取 pass（模板 → 内部 `ops`/`bind` 回填 ⇒ 下游**一行未改**） | ✅ | `src/dsl/inline_ops.rs` + `tests/inline_ops.rs` 7 条（含"与旧写法 `insts` 报告逐字段等价"） |
| 内存形态标记 `{@[字段]:槽}` | ✅ | pass + 迁移器双向；`tests/v22_migration.rs::memory_form_marker_is_preserved` |
| 迁移器 `v22_migrate`（纯文本，只动 `[inst]` 的 `ops`/`bind`/`asm` 三行） | ✅ | `src/v22_migrate.rs`；硬门测试**暂 `#[ignore]`**（见第 1 步） |
| `forge-isa new <名>` 脚手架 | ✅ | `forge-isa/tests/new_spec.rs` 2 条（与教程 §0 同源 + 产物可 `validate`） |
| 参考规范 + CHANGELOG | ✅ | `docs/reference/isa-dsl.md`「操作数声明写进 `asm`（v22）」；CHANGELOG `Added (2026-10-11)` |
| 全量门禁 | ✅（2026-10-11 实测） | `cargo test --workspace --exclude forge-rustc -j 2` **exit 0**，148 个 `test result: ok`，13.2 min |

### 下一步（按序，每步都有可判定验收）

1. **顺序标记 `{名字:槽[:角色]#序号}`**（序号 = 旧 `ops` 里的下标）。内联后"出现序 = 编码序"，
   而实测 **100 条**指令的出现序 ≠ 旧 `ops` 序（其中 **75 条**完全靠位置回退才编对）⇒ 不标就**静默改语义**。
   - pass（`dsl/inline_ops.rs`）：`split_decl` 返回 `Option<u32>`、`Ph::Decl` 与 `Decl` 带上它；
     一条指令内"部分带序号" ⇒ 报错；按序号排序后再走既有的**子序列合并**。
   - 迁移器（`v22_migrate.rs`）：出现序 ≠ `ops` 序时给**每条**声明打 `#序号`（需新增 `appearance_order()` 判据）。
2. **解除 `tests/v22_migration.rs` 那条 `#[ignore]`**：三份发行谱 + 夹具迁移前后 `expand_file`
   展开**逐字节相同**（当前列表只有 riscv64/arm64；x86 的内存标记已修，第 1 步完成后一并纳入）。
3. **删旧键**：`Instruction.ops`/`bind`、`EncKeys.modrm`、v21 占位修饰（`{name:affix}`/`{name?}`，
   实测 `.toml` **0 处**、仅 1 条 Rust 单测在用）。四处同步：`schema.rs` ↔ `dsl/model.rs` ↔
   `docs/reference/isa-dsl.md` 键表 ↔ `isa-dsl.schema.json`（`cargo run -p forge-isa -- schema --out …`）。
4. **工具面**：`LINT-FORM-FIELD-UNUSED`（form 字段无人绑定）、`insts`/`explain` 打印绑定来源
   （显式 `@` / 同名 / 位置回退）、未知键建议（节路径 + 编辑距离 ≤2）。
5. **文档**：`docs/reference/isa-dsl.md` 开篇"写一条指令用哪几个键"决策树；`docs/README.md` 与
   `CLAUDE.md` 的 v21 速查段同步 v22。

**边界（不许假绿）**：`modrm` 虽在发行谱 0 处，但 `stream_decl.rs`/`field_decl.rs` 的变长路径仍读 `bind`
（`reg`/`rm`）；**删键前**必须先把这两条路径切到 `asm` 内联绑定，并让 `isa_roundtrip_guard` 与
`spec_coverage_guard` 的指令数、文本歧义键保持**逐字不变**。
