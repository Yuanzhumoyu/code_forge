# ISA-DSL v12 路线图（唯一语法，不兼容 v11，允许重构）

> 状态：**迭代 1 已完成**（2026-08）。本文档持久化已批准的 v12 方案，供后续迭代参考。
> 已定决策：**v12 是唯一 DSL 语法**。v11 语法层（`encoding` 字符串 + `@原语`、紧凑
> `fields` 串、`when` 谓词串、asm 隐式魔法）**整体移除**，无兼容层、无转换工具、
> 无逃生门。forge-dsl 内部重构为直接消费 v12 结构化模型。

## 1. 现状诊断（实测数据）

- `isa/x86_v10.toml`：**1733 行 / 124 指令 / 115 lower**；16 种编码原语，前 6 种占 76%
  （@sse_rr 27、@modrm 24、@modrm_mem 14、@modrm_imm32 6、@vex_rrvvv_avx2 6、@vex_rrvvv 5）
- 复杂度根因：逐指令手写低层机制（选原语、写位域、写类型分派），无"声明意图"抽象层
- 通用性短板：原语硬编码白名单（bitstring.rs 30+），x86 专有概念与其他 ISA 无关

## 2. 参考方案

| 方案 | 借鉴点 |
| --- | --- |
| Ghidra SLEIGH | 命名位域 token 模型：位域一次声明，指令引用 |
| nML（MicroTESK） | 操作三属性分离（语法/编码/语义）+ 宽度模式 |
| LLVM TableGen | 类继承（form）+ 参数化族（family） |
| Cranelift ISLE | lower 声明规则化 |
| LLVM MDL RFC | ISA 描述与实现解耦 |

## 3. 架构（无 v11 层，直接消费 v12 结构化模型）

```text
用户 TOML（v12：conventions/forms/instructions/families/lowering/abi）
   ↓ 严格 TOML 解析 + 语义校验
v12 结构化模型（无字符串编码描述）
   ↓ 代码生成（encoder/decoder/assembler/lowering 全部从结构化模型直接生成）
生成 Rust（Reg/Inst/TargetMachine 组件）
```

**移除**：bitstring 字符串解析层（parse_encoding/expand_encoding/位域字符串语法）、
`@原语` 字符串白名单、紧凑 fields 串、when 谓词串、asm 隐式魔法名（rd/rs1）。

**转变**：

- `@modrm/@vex_rrvvv/...` 原语字符串 → **form 语义键**（结构化配置），实现为内部 Rust 函数
- decoder 从 **form + bitfields 结构化描述**生成（替代基于 ParsedEncoding 字符串解析的 gen_decoder）
- 位域位置从 `conventions.bitfields` 推导，指令不再写位偏移

## 4. 完整 TOML Schema（严格 TOML，无字符串 DSL）

### 4.1 `[meta]` / 寄存器组

```toml
[meta]
name = "x86_64"
version = "12.0"
endian = "little"            # 缺省 little
mode = 64                    # 缺省 64
default_inst_width = 32      # 定宽 ISA（riscv/aarch64）；变长 ISA 省略
variable_length = true       # 变长 ISA（x86）；与 default_inst_width 互斥
max_inst_len = 15            # 变长 ISA 最大指令长度

[reg.gpr64]
width = 64
names = ["RAX", "..."]       # 或 count + prefix 生成式声明
count = 16
prefix = "XMM"
base_index = 4               # 物理编号偏移（gpr8h）
```

### 4.2 `[conventions]` —— ISA 约定（一次声明）

```toml
[conventions.bitfields]        # 命名位域（SLEIGH 风格）
rd  = { offset = 7,  width = 5 }
rs1 = { offset = 15, width = 5 }
rs2 = { offset = 20, width = 5 }
opcode = { offset = 0, width = 7 }

[conventions.modrm]            # 表存在即启用
reg_field = "modrm_reg"
rm_field = "modrm_rm"
force_disp_base = [5, 13]      # mod=00+rm=101 是 RIP-rel，须强制位移

[conventions.rex]
w_opsize = 64

[conventions.opsize_prefix]    # 键为数字字符串（TOML 裸整数键）
16 = 0x66
```

### 4.3 `[[operand_slots]]` —— 操作数槽

```toml
[[operand_slots]]
name = "gpr"
kind = "reg"                   # reg / imm / mem / label / cond / opsize
class = "gpr"                  # reg：所属 [reg.*] 组
field_width = 5                # reg：编码宽度（位）
roles = ["in", "out"]          # 缺省 ["in"]；"inout" = 读改写（in 且 out）

[[operand_slots]]
name = "imm32"
kind = "imm"
signed = true
width = 32
```

### 4.4 `[[forms]]` —— 编码形式（语义键组合，非原语字符串）

```toml
[[forms]]
name = "RR"                    # x86 ModRM 寄存器-寄存器
modrm = "rr"                   # 语义键 → 内部实现（开放集合，迭代 3/4 定型）
rex = "auto"
prefix = "opsize"
opcode_bytes = 1

[[forms]]
name = "SSE_RR"
modrm = "rr"
escape = [0x0F]                # 结构化数组
opcode_bytes = 1

[[forms]]
name = "VEX_RRV"               # AVX 三操作数
vex = { map = "inst", pp = "inst", l = "inst" }
modrm = "rr"

[[forms]]
name = "R"                     # riscv R-type
opcode_field = "opcode"        # 定宽：主 opcode 位域名
operand_slots = ["gpr", "gpr", "gpr"]
```

### 4.5 `[[instructions]]` —— 指令（2-4 行）

```toml
[[instructions]]
name = "MOV_R_RM"
form = "RR"
opcode = 0x8B
operands = [
  { slot = "gpr", role = "out" },
  { slot = "gpr", role = "in" },
]

[[instructions]]
name = "ADD"                   # riscv R-type
form = "R"
opcode = 0x33
fields = { funct3 = 0, funct7 = 0 }   # 固定字段值，按位域名引用
operands = [
  { slot = "gpr", role = "out", field = "rd" },   # field：定宽位域绑定
  { slot = "gpr", role = "in",  field = "rs1" },
  { slot = "gpr", role = "in",  field = "rs2" },
]
mnemonic = "add"               # 缺省 = 指令名小写
asm = "movrr {0}, {1}"         # 可选；占位符 {0}/{name}，语法文档化
```

### 4.6 `[[families]]` —— 参数化指令族

```toml
[[families]]
name = "PS_BIN_V"
form = "VEX_RRV"
operands = [{ slot = "fpr", role = "out" }, { slot = "fpr", role = "in" }, { slot = "fpr", role = "in" }]
[[families.variants]]
name = "VADDPS"
opcode = 0x58
mnemonic = "vaddps"
[[families.variants]]
name = "VSUBPS"
opcode = 0x5C
mnemonic = "vsubps"
```

### 4.7 类型分派谓词（结构化，无字符串；迭代 4 定型）

```toml
[[families.variants]]
when = { and = [
  { eq = ["rs1_width", 256] },
  { eq = ["elem", "F32"] },
] }
```

小型结构化谓词语言：`and/or/not/eq/ne/lt/le/gt/ge`，操作数属性引用
（`rs1_width`/`elem`/`rd`），纯 TOML 数据。迭代 1 模型层以 `toml::Value` 透传。

### 4.8 `[[lowering]]` —— 指令选择（符号化操作数）

```toml
[[lowering]]
op = "Iadd"
insts = ["{out} = MOV_R_RM {0}, {1}", "{out} = ADD_R_RM {out}, {2}"]
```

`{out}`/`{0}` 符号替代 rd/rs1 魔法名；默认 lowering 从 operands 角色推导。

### 4.9 `[abi]` —— 调用约定（类型类别分类，为 YMM 铺路）

```toml
[abi]
stack_align = 16
[[abi.arg_class]]
class = "int"
regs = ["RCX", "RDX", "R8", "R9"]
[[abi.arg_class]]
class = "float"
regs = ["XMM0", "XMM1", "XMM2", "XMM3"]
[[abi.arg_class]]
class = "vector"
strategy = "by-ref"            # >limit 位按引用传参（YMM ABI）
limit = 128
```

### 4.10 `[emit]` —— 序言/尾声

```toml
[emit.prologue]
insts = ["push RBP", "mov64rr RBP, RSP", "@push_callee", "@frame_alloc"]
```

## 5. forge-dsl 重构（v11 语法层移除）

```text
crates/frontend/forge-dsl/src/
├── v12/
│   ├── mod.rs         # 模块入口 + V12Error + parse_and_validate
│   ├── model.rs       # v12 模型（唯一模型，deny_unknown_fields）
│   ├── parse.rs       # 严格 TOML 解析
│   ├── validate.rs    # 语义校验（引用完整/位域越界/重复声明/类型约束）
│   └── pred.rs        # 结构化谓词求值（迭代 4）
├── codegen/
│   ├── mod.rs          # 从 v12 模型生成（直接，无字符串中间层；迭代 2+）
│   ├── emit_gen.rs     # encoder 生成（form 语义键 → 发射代码）
│   ├── decoder_gen.rs  # decoder 生成（form+bitfields 结构化 → 解码逻辑）
│   └── components.rs   # TargetMachine 组装（保留）
├── asm_grammar.rs      # assembler 生成（保留，从 asm 模板 + operand_slots）
├── cst_codegen.rs      # 保留
├── lib.rs              # 入口：v12 TOML → 校验 → 生成（迭代 2 切换）
└── (删除) bitstring.rs 字符串解析层、旧 model 的 encoding 字符串路径
```

## 6. 迭代序列（9 迭代）

| # | 内容 | 验证 |
| --- | --- | --- |
| **1** ✅ | v12 模型 + 严格解析 + 语义校验（meta/reg/conventions/operand_slots；forms/instructions/families/lowering/abi/emit 结构校验） | forge-dsl 166 测试全绿（含 v11 拒绝证明 + 序列化往返） |
| **2** ✅ | 定宽编码生成（form R/I/S/U/B/J/SHIFT/OP/W32 → encode/decode/asm 从 bitfields 直接生成）；散布位段 + operand_fields；riscv64 全 76 指令迁移 | golden 逐字节对比 v11（42 GPR）+ 规范 oracle（F/分支/S 型）+ 全量往返 9/9 |
| 3 | 变长语义键（modrm/prefix/escape/rex）；x86 @modrm/@sse 家族迁移 ~65 条 | golden 字节等价 + decoder_smoke 扩展 |
| 4 | VEX 语义键 + 指令族；结构化谓词求值接入 | 展开指令数一致 + forge-tests 全绿 |
| 5 | lowering 符号化 + abi.arg_class 类别分类 + emit 保留 | mini_c 双后端 + forge-rustc e2e 不回归 |
| 6 | x86 124 指令 + 115 lower 全量 v12；**1733 → <800 行**；v11 语法层物理删除 | 全量 golden + 门禁 |
| 7 | 全 ISA 迁移 + 新 ISA（ARM32 子集）不改生成器证明 | 新 ISA 全链路 |
| 8 | 生成器深度清理（bitstring 残留/死代码） | 全量测试 + forge-dsl 行数下降 |
| 9 | 文档 v12 重写（isa-dsl.md/encoding-guide） | 文档-代码一致性核对 |

## 7. 与剩余两项的关系

- **C1 YMM ABI**：依赖迭代 5 的 `abi.arg_class by-ref` 策略，迭代 6 之后实施
- **C2 forge-rustc vec**：与 DSL 迁移正交（主库 regalloc），可并行；建议迭代 5 后启动
- 技术路径见 `docs/roadmap-status.md`

## 8. 风险与决策点

| 风险 | 缓解 |
| --- | --- |
| form 语义键表达力不足 | 语义键是开放可扩展集合，新增键 = 新增内部实现函数 |
| decoder 从 form 生成的正确性 | 每步 golden + 往返验证；decoder_smoke 12/12 作回归基准 |
| 迁移回归（x86 124 指令） | 分家族迁移 + golden 对比 + 全量门禁 |
| 结构化谓词冗长 | TOML 内联简写（单条件直接 `{ eq = [...] }`） |

**执行期决策点**（遇到时暂停询问）：

1. 语义键集合边界（迭代 3/4：modrm/vex/sib/prefix/escape/imm/reloc/leb128...）
2. 谓词字段集合（rs1_width/elem/rd/opsize...，迭代 4）
3. C1/C2 与 DSL 迁移的先后（建议迭代 5 后启动）
4. 新 ISA 验证对象（ARM32 子集 vs 自定义 SIMD，迭代 7）

## 9. 成功标准

1. x86 定义 **1733 → <800 行**；新 ISA 加入**不改生成器**
2. v12 与 v11 生成结果**行为等价**，但语法层已物理移除
3. decoder 从 form 结构化描述生成（无字符串解析）
4. YMM ABI / vec 两项按 Phase C 执行并验证
5. 文档（isa-dsl.md v12）与代码一致

## 10. 迭代 1 完成记录（2026-08）

- **交付**：`crates/frontend/forge-dsl/src/v12/`（mod.rs / model.rs / parse.rs / validate.rs / tests.rs）
- **模型**：V12Model{meta, reg, conventions, operand_slots, forms, instructions, families, lowering, abi, emit}，
  全部 `deny_unknown_fields`；Meta（endian/mode/default_inst_width/variable_length/max_inst_len）、
  RegGroup（names 或 count+prefix）、Conventions（bitfields/modrm/rex/opsize_prefix）、
  OperandSlot（kind/class/field_width/width/signed/roles）、Form（语义键开放集合）、
  Instruction（opcode/fields/operands[{slot,role,field}]/mnemonic/asm/when）、Family、Lowering、Abi、Emit
- **角色**：`OperandRole::{In, Out, InOut}`（inout = 读改写，减少模板冗余）
- **校验**：ISA 名合法、reg 组非空/名称唯一、位域 0..=64 不越界、modrm/opsize_prefix 引用与范围、
  槽名唯一 + reg/imm 必填字段、form 存在性 + opcode_bytes 1..=4 + escape ≤4、指令重名/form 引用/
  槽引用/位域引用、family 变体 opcode 或 fields 必填、lowering/abi/emit 结构约束
- **验证**：164 测试全绿（riscv/x86 示例文档解析、非法 TOML 诊断、v11 文件拒绝证明、
  序列化往返无损、inout 角色往返、必填字段解析期报错、空表语义校验）
- **已知行为**：toml crate 按字母序迭代表（BTreeMap），解析错误位置为字母序首个未知键
  （如 v11 文件报 `[abi.arg_regs]` 而非 meta 的 `no_default_lowering`）——迁移工具若需
  文档序诊断，需自定义 deserializer（迭代 8 可选项）
- **门禁**：forge-dsl clippy 0 警告、fmt 干净、workspace check 通过

## 11. 迭代 2 完成记录（2026-08）——定宽编码生成 + riscv64 迁移试点

- **交付**：
  - `crates/frontend/forge-dsl/src/v12/codegen/mod.rs`（新）：自包含模块生成器
    （Inst 枚举 / encode / decode / disassemble / assemble，仅依赖 std）
  - 模型扩展：`Bitfield` 支持散布位段（`pieces = [{offset,width,shift}]`，编码
    `(v>>shift & mask)<<offset`、解码反向 OR——表达 S/U/B/J 立即数布局）；
    `Form.operand_fields`（定宽按位置绑定位域，操作数不足时隐式 0）
  - 新 proc 宏 `isa_v12_from_file!`（模块名 = 文件 stem；`read_isa_file` 与
    v11 宏共享）
  - `isa/riscv64_v12.toml`：全 **76 指令** v12 迁移（保持 v11 声明序）
  - `crates/backend/forge-codegen/src/arch/riscv64_v12.rs` + `lib.rs`/`arch/mod.rs`
    注册；`tests/riscv64_v12_tests.rs` 9 个验证测试
- **验证**（9/9 全绿）：
  - golden：42 条 GPR 指令字节与 v11 后端**逐字节一致**（汇编→编码对比）
  - 规范字节：F 指令按 RISC-V 规范独立 oracle 计算（v11 的 FPR `16+i` 索引
    不合规，v12 用组内索引修正）
  - 全量 decode 往返（含 v11 不可解码的分支/负立即数）；分支散布布局、
    S 型规范字节、负立即数符号扩展（v11 解码不扩展）、assemble/disassemble
    往返、Inst 枚举形状
- **发现并修复的 v11 缺陷**（v12 规范正确）：
  1. **riscv64 S 型散布移位反了**（v11 `[7;5;5]`+`[25;7]` 应为 `[7;5;0]`+
     `[25;7;5]`）→ v11 的 sd/FSW 字节非规范（如 `sd x1,8(x2)` v11=0x10113023
     规范=0x00113423）
  2. FPR 编码 `16+i` 使 F 指令字节不合规（仅内部往返一致）
  3. 解码负立即数不符号扩展（v12 按槽宽度规范扩展）
- **生成器要点**：解码 guard = opcode/fields 值 + **覆盖补集零位**（统一处理
  纯 opcode 形式 NOP/ECALL、隐式 0 位域、FENCE 全字常量）；有符号槽按槽宽度
  符号扩展；asm 模板驱动（simple / `{I}({J})` / `({J})` 三种形状）
- **门禁**：forge-dsl 166 测试、forge-codegen 全量（lib 98 + smoke/assembler/
  decoder/packet + v12 pilot 9）、clippy 0 警告、fmt 干净

**下一步（迭代 3）**：变长编码语义键（modrm/prefix/escape/rex）→ x86
@modrm/@sse 家族迁移试点（~65 条）；`OperandRole::InOut` 在 x86 RMW 操作数
落地。随后迭代 4（VEX + families + 结构化谓词）。
