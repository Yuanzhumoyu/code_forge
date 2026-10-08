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
- [ ] **语法扩展决策（需评审）**：你的语法里 `[begin:end]` 是**连续**位域；riscv 的 S/B/J
      是**散射**且值自身 bit 0 不参与（`off_b` 无 `unit`，见下 §4.2 风险 3），连续形式表达不了。
      W2 补了最小扩展：`type[值位段 -> 词位块, …]`（值位段低位在前，词位块按值低位→高位放置）。
      **若你要别的写法（例如保持单括号只写词位、值偏移另开一个键），现在改代价最小。**
- [ ] `dsl/model.rs`：接线（`[[forms]].fields` / `[[instructions]].fields` + `bind`），
      内部 `bitfields`/`opcode_field`/`operand_fields` 改 `#[serde(skip)]` 派生。
- [ ] 解析与诊断：位区间语法错误、类型不存在、宽度不符、`ops` 引用未知槽名 —— 报错点名
      `[[forms.<名>]].fields[i] = "<原文>"` 与原因（TOML 层无 span，与既有诊断口径一致）。
- [ ] lowering pass（§4.0）+ 绑定规则：**`ops` 列出的槽 = 操作数槽；其余 = 常量槽**（D-1/D-3）。
- [ ] `match` 覆盖（常量槽换值 / 操作数槽钉成常量）；`bind` 改名（D-4）。
- [ ] 位段相交的逐指令校验（同一条指令不得同时绑定相交两段）。
- [ ] 删除 `[conventions.bitfields]` / `opcode_field` / `operand_fields` 的用户面（model 键、schema、docs）。
- [ ] 三谱迁移（脚本 + 人工复核，见 §4.2）：riscv、arm64 全量；x86 无 bitfields 节 ⇒ 只跟着 `match` 改名。
- [ ] `lint --bits` 口径确认：未被字段覆盖的位仍按现状报出（D-6）。
- [ ] 门禁：§2 全绿；`spec_coverage_guard` 三谱指令数不变；`LINT-BITFIELD-OVERLAP` 不再需要例外。

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
> ⏭ **W4.2（下一片）**：按 form 的前缀扫描表（删 `default_prefix_scan()`）
> （删 `default_prefix_scan()`）→ 再迁 x86 谱（20 forms / 200 insts / 18 templates；键用量
> `modrm 159 / vex 154 / evex 72 / prefix 114 / opsize 81 / escape 47 / imm 242`）。
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
