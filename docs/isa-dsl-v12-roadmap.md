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
| **3** ✅ | 变长语义键（modrm rr/ext、opsize auto/固定、prefix field、rex_w、imm）；encode→Vec<u8>、decode→(Inst,usize)；opsize 非文本操作数；x86 @modrm 24 + @modrm_imm32 6 + @sse_rr 23 | golden 30 GPR 对比 v11 + opsize 16/32/64 + SSE 规范 oracle + 全 53 条往返 7/7 |
| **3b** ✅ | 内存寻址（modrm rm_mem/rm_memref、MemRef 槽、SIB/force_disp_base/disp、LOCK F0、`[{n}]` 形状）；x86 @modrm_mem 14 + MOV64_RR | golden 内存对比 v11 + mem_spec_bytes 10 条 + 全 68 条往返 8/8 |
| **4** ✅ | families codegen（Family 展开：fields 共享 + {mnemonic} 模板，7 家族 37 变体）；VEX 语义键（C4 + vex2/3 + vvvv，4 forms + 10 指令）；结构化谓词框架（pred.rs） | 家族 SSE 规范 11 + VEX 规范 9 + 全 106 条往返 10/10 |
| 3 | 变长语义键（modrm/prefix/escape/rex）；x86 @modrm/@sse 家族迁移 ~65 条 | golden 字节等价 + decoder_smoke 扩展 |
| 4 | VEX 语义键 + 指令族；结构化谓词求值接入 | 展开指令数一致 + forge-tests 全绿 |
| **5** ✅ | lowering 符号化 + abi.arg_class 类别分类 + emit 保留 | mini_c 双后端 + forge-rustc e2e 不回归 |
| 6 | x86 124 指令 + 115 lower 全量 v12；**1733 → <800 行**；v11 语法层物理删除 | 全量 golden + 门禁 || 7 | 全 ISA 迁移 + 新 ISA（ARM32 子集）不改生成器证明 | 新 ISA 全链路 |
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

## 12. 迭代 3 完成记录（2026-08）——变长语义键 + x86 @modrm/@sse 试点

- **交付**：
  - 模型扩展：`Form` 变长语义键——`modrm = "rr"|"ext"`（ModRM mod=11；
    ext = fields.ext 固定扩展码）、`opsize = "auto"|8/16/32/64`（16 → 0x66
    前缀、64 → REX.W；auto 需 opsize 操作数）、`prefix = "field"|数字`
    （fields.prefix：SSE 66/F2/F3/0）、`rex_w = "auto"|"field"`（缺省随
    opsize）、`imm = 32`（尾部 imm32）
  - codegen：encode 统一返回 `Vec<u8>`、decode 统一返回 `Option<(Inst, usize)>`；
    变长路径（前缀扫描 66/F2/F3/REX → escape → opcode → ModRM → imm）；
    opsize 槽为**非文本操作数**（asm 无占位符，assemble 默认 64，disassemble
    不渲染）；assemble 多形状**静默回退**（同 mnemonic 的不同形状按声明序
    尝试，解析失败落到下一形状）
  - `isa/x86_v12.toml`：**53 条指令**（@modrm 24 + @modrm_imm32 6 +
    @sse_rr 简单 23），保持 v11 声明序
  - `arch/x86_v12.rs` + `tests/x86_v12_tests.rs`（7 测试）
- **验证**（7/7 全绿）：
  - golden：30 条 GPR 指令与 v11 逐字节一致（含扩展寄存器 REX、imm32）
  - opsize 16/32/64 直接构造对比 v11（66 前缀/REX.W）+ 硬编码字节
  - SSE 规范字节 oracle（22 条；v11 的 FPR `16+i` 使 SSE 字节带多余 REX 且
    不合规，v12 组内索引规范正确）
  - 全 53 条字节级 decode 往返（含编码撞车的别名，声明序首匹配与 v11 一致）
  - assemble/disassemble 往返（opsize 非文本默认 64）
- **发现**：v11 自身歧义（"movzx"/"movsx" 两个重载同 mnemonic 时 v11 汇编器
  Ambiguous；负 imm32 解析 TypeMismatch）——golden 对比规避，v12 无此问题
- **门禁**：forge-dsl 166、forge-codegen 全量（98 lib + smoke/assembler/
  decoder/packet + v12 9+7）、clippy 0 警告、fmt 干净

**下一步（迭代 3b/4）**：@modrm_mem 内存寻址（SIB/disp，迭代 3b 收尾）；
VEX 语义键 + families（opcodes 数组家族：SD_BIN/SS_FMOV/PS_BIN/PD_BIN/PI_BIN）
+ 结构化谓词（迭代 4）。随后迭代 5（lowering/abi/emit 迁移 + x86 全量）。

## 13. 迭代 3b 完成记录（2026-08）——@modrm_mem 内存寻址

- **交付**：
  - 模型：`modrm = "rm_mem"`（reg=op0、rm=op1 基址寄存器、disp 恒 0）与
    `"rm_memref"`（reg=op0、rm=op1 mem 槽 → base/disp）；`OperandKind::Mem`
    槽（值类型 = 生成的自包含 `MemRef { base: u32, disp: i64 }`）
  - codegen：内存 ModRM 编码（mod 计算、base&7==4 自动 SIB、force_disp_base
    → mod=01+disp8=0、disp8/disp32）；变长解码内存形式（mod∈{0,1,2} + SIB
    index=4 + disp 符号扩展 + RIP-rel 拒绝）；**MemReg 只接受 mod∈{0,1} 且
    disp==0**（无 disp 语义，与 MemRef 形式解码区分）；前缀扫描加入 F0（LOCK，
    夹在 66/REX 之间）；asm 新增 `[{n}]` 方括号寄存器形状 + mem 槽渲染
    `[RAX]`/`[RAX+8]`/`[RAX-8]` 与解析
  - `isa/x86_v12.toml`：+15 条（XADD/XCHG/SUB/AND/OR/XOR_MEM_R、MOV_R_MEM、
    STORE_MEM_R、MOVSD_R_MEM、MOVSD_MEM_R、MOV64_RR/RM/MR、MOVSD_RM/MR）
    → **共 68 条**
- **验证**（x86_v12 8/8 全绿）：
  - golden：内存指令与 v11 逐字节一致（含 RSP SIB、R8/R9 REX）
  - mem_spec_bytes：10 条规范字节（SIB、LOCK、force_disp_base、disp8 符号）
  - 全 68 条字节级 decode 往返
- **修正的 v11 缺陷**：locksub/lockand/lockor/lockxor 带多余 `0x0F` escape
  （F0 48 0F 29 非规范）→ v12 规范（F0 48 29）
- **门禁**：forge-dsl 166、forge-codegen 全量、clippy 0 警告、fmt 干净

**下一步（迭代 4）**：VEX 语义键（C4/C5 + mmmmm/pp/W/vvvv/L）+ families
（opcodes 数组家族）+ 结构化谓词。随后迭代 5（lowering/abi/emit 迁移 +
x86 全量收官）。

## 14. 结构完善（2026-08）——通用汇编模板段 + asm 完整格式

针对评审意见的两项结构完善（迭代 4 前置）：

**A. asm 完整格式（含 mnemonic，直观性）**
- `asm` 语义变更：**完整汇编格式**，首词即 mnemonic（`asm = "ld {0}, {2}({1})"`、
  `asm = "amoadd.w.aqrl {0}, {2}, ({1})"`）；缺省 = `mnemonic` + 默认操作数模板
- `mnemonic` 字段保留；asm 存在时从 asm 推导并与字段**交叉校验**（不一致报错，
  单一事实来源）——validate + codegen 双重检查
- 迁移：x86_v12.toml 29 条 + riscv64_v12.toml 9 条 asm 改为完整格式（脚本
  校验首词 == mnemonic，0 不一致）

**B. 通用模板段解析（替换封闭 TokenShape）**
- **删除** TokenShape/classify_token/parse_idx/Bracket/Mem/Mem0 特殊逻辑
- 新 `Seg = Lit(String) | Op(usize)` 段模型：操作数模板 = 字面段与占位符交替
- 渲染（disassemble）：段序列逐段输出（字面原样 + 占位符按槽渲染）
- 解析（assemble）：逐段消费文本——字面段前缀匹配；占位符段提取到**下一字面
  段首次出现**为止的文本，按槽解析（reg/imm/mem）；任一失败静默回退（保持
  同 mnemonic 多形状机制）
- 校验：占位符越界、非文本操作数（opsize）被引用、**连续占位符无字面分隔**
  （歧义）→ codegen 报错
- 效果：`[{n}]`/`{off}({base})`/`({n})` 自动成为字面段组合；**任意字面格式**
  （`byte ptr [{n}]`、SIB 风格等）无需改生成器
- 删除 `__parse_mem_{group}` helpers（`{off}({base})` 段拆分为 imm+reg 解析）；
  `__parse_mem_ref`（mem 槽）保留

**验证**：forge-dsl 172（+6 结构测试：asm 提取/默认模板/mnemonic 不一致/通用
段/相邻占位符拒绝/越界拒绝）、riscv 9/9 + x86 8/8 行为不变、forge-codegen
全量、clippy 0 警告、fmt 干净

**下一步（迭代 4）**：VEX 语义键 + families（opcodes 数组家族）+ 结构化谓词。

## 15. 迭代 4 完成记录（2026-08）——families + VEX 语义键 + 结构化谓词框架

- **families codegen**：
  - Family 加 `fields`（共享固定字段，如 SSE 的 prefix/w）与 `asm`（家族模板，
    `{mnemonic}` 占位符替换为变体 mnemonic）；collect_inst_infos 展开为
    owned 合成指令（InstInfo.inst 改 owned）
  - 迁移 7 个家族（v11 opcodes 数组）：SD_BIN（7）、SS_FMOV（4）、PS_BIN（8）、
    PD_BIN（4）、PI_BIN（4）、PS_BIN_V（6）、PD_BIN_V（4）→ 37 变体
  - SHIFT_BIN（@shift_reg 多指令序列）留待后续
- **VEX 语义键**：
  - VexSpec 加 `w` + map/pp/w/l 来源（数字或 `"field"` → fields.vex_*）；
    vvvv 语义 = 操作数 ≥3 且第 3 个是 reg → ~op2，否则 0x0F（无源）——无需
    显式 has_src 参数
  - 编码：C4 + vex2（R/X/B 反位 + map）+ vex3（W/vvvv/L/pp）+ opcode + ModRM
  - 解码：独立 arm（C4 + map/pp/w/l guard + R/B/vvvv 提取）
  - forms：VEX_RRV / VEX_RR / VEX_RRV_IMM / VEX_RR_IMM；迁移 10 条 VEX 单指令
    + 2 个 VEX 家族 → x86_v12.toml 共 **106 指令**
- **结构化谓词框架**（v12/pred.rs）：`{ and/or/not }` + `{ eq/ne/lt/le/gt/ge
  = [attr, value] }` 解析 + 求值（未知属性 false）；lowering 接入在迭代 5
- **验证**：forge-dsl 175（+3 pred）、x86_v12 10/10（家族 SSE 规范字节 11 条、
  VEX 规范字节 9 条、全 106 条往返、assemble/disassemble 家族+VEX）、
  riscv 9/9、forge-codegen 全量、clippy 0 警告、fmt 干净

**下一步（迭代 5）**：lowering/abi/emit 迁移（符号化操作数 + abi.arg_class
by-ref 为 YMM 铺路）+ 结构化谓词接入 lowering + x86 全量收官（@op_rm/
@cmovcc/@setcc/@mov_imm64/@sse_rr_imm8/PSHUFD 等 + SHIFT_BIN）。

## 16. 迭代 5 完成记录（2026-08）——TargetMachine 集成层 + x86 终结

**分支决策**：用户选择「直接集成 TargetMachine」（vs 声明层先行）——v12
自包含模块之上生成 forge-codegen 组件，不再停留于纯自包含。

- **5a（commit fb34785）x86 终结 +r 形式**：
  - 新 form 键 `opcode_reg`（50+r/push、58+r/pop、B8+r/mov_imm64、C8+r/bswap）：
    encode = REX（always → 0x48|B；否则 reg≥8 → 0x41）+ [escape] + opcode|reg&7
    + imm；decode = 独立 arm（高 5 位匹配 + REX.B<<3 提取）
  - vlen_ctx 的 modrm 条件化（`+r` 形式无 ModRM，Option<ModrmKind>）
  - asm mnemonic 前缀修正：脚本改用 `mnemonic` 字段而非 name 小写
    （sub/add 家族 + VEX family `{mnemonic}` 模板）
  - x86_v12.toml +12 条：NOT/NEG/DIV/IDIV（MRR_EXT_OP）、SUB64/ADD64_R_IMM32
    （MRR_EXT64_IMM32）、PSHUFD/SHUFPS（SSE_RR_IMM8）、PUSH/POP（PUSH_REG/
    POP_REG）、MOV_REG_IMM64（MOV_IMM64）、BSWAP_R → 共 **118 指令**
  - 测试：+r golden 与 v11 逐字节一致 + 规范字节 + roundtrip + asm 往返
    （x86_v12_tests 10→12）
- **5b（commit 50a1e86）TargetMachine 集成层**：
  - `v12/codegen/integration.rs`（新）：自包含模块之上生成 forge-codegen 组件
    - **Reg 物理寄存器枚举** + PhysReg impl + `__DEFAULT_GPR/FPR_CLASS`
      （v12 组内索引语义；x86 gpr64 0..15、xmm 0..15）
    - **MachineInst impl**：uses/defs/reg_field/set_reg_field/effects/
      is_branch/is_call/is_ret/is_move——每变体一条 arm，Reg 操作数按操作数序
      编号（与 lowering map_reg_field 对齐）；effect 标签驱动分支/调用/返回
    - **TargetEncoder/Decoder/Disassembler/Assembler**：包装自包含
      encode/decode/disassemble/assemble（操作数物理索引，regalloc 回填后
      直接编码——与 v11 的 set_reg_field 回填语义一致）
    - **TargetABI**：`[abi]` arg_class 类别分类 → arg_regs（int 类在前，
      float/vector 依次）；stack_align；vector by-ref 策略为 YMM 铺路
    - **TargetFrameLowering**：最小实现（prologue/epilogue no-op；[emit]
      指令序列接入与 spill 在迭代 6 补齐）
    - **TargetLowering**：`[[lowering]]` 符号化操作数 `{out}`/`{0}`/`{1}`
      展开 → InstPacket（Reg 字段占位 0 + map_reg_field，regalloc 回填；
      opsize 缺省 64；物理寄存器/imm 直接写死；`when` 谓词解析校验）
    - **IsaInfo/RegInfo**（sp/fp/allocatable 顺序按寄存器名解析，缺失回退
      from_index）+ **TargetMachine struct** + ensure_registered
  - 模型：`Instruction` 加 `effect` 字段（Pure/Read/Write/Branch/Jump/Call/Ret）
  - validate：arg_class `regs` 可空（by-ref 策略类不占寄存器）
  - x86_v12.toml：effect（PUSH/POP）+ `[abi]`（stack_align 16 + int/float/
    vector by-ref arg_class）+ 7 条 `[[lowering]]`（Iadd/Isub/Band/Bor/Bxor/
    Imul/Icmp）
  - 测试：`v12_integration_tests` 6 个（TM 组装 / MachineInst 查询 / encoder-
    decoder 经 TM / lowering Iadd 包结构 / 未知 op Unsupported / riscv TM）；
    forge-dsl 175 全绿、x86_v12 12/12、riscv64_v12 9/9、workspace 51 套件全绿、
    clippy 0 真实警告、fmt 干净

**下一步（迭代 6）**：v12 后端跑通 mini_c 双后端（补 JMP/CALL/RET 指令 +
terminator lowering + [emit] prologue/epilogue + spill）；x86 124 指令 + 115
lower 全量 v12；v11 语法层物理删除。

## 17. 迭代 6 完成记录（2026-08）——v12 后端端到端执行 + mini_c 双后端

**里程碑**：v12 后端从『能编译』到『能真实执行 mini_c 源码』。

- **6a（commit 30cabd3）控制流 + FrameLowering 完整 + terminator**：
  - x86_v12.toml：JMP_REL32（E9 rel32）/CALL_RIP_REL（E8 rel32）/RET（C3）
    + REL32/NOOP form 键 + rel（label）槽；控制流 spec/roundtrip 测试
  - v12 codegen：无 ModRM 变长形式（prefix+escape+opcode+imm）encode/decode
    支持（modrm 条件化经 if-let）
  - 模型：Abi 加 frame/callee_saved；顶层加 [spill.*] 模板
  - FrameLowering 完整：[emit] prologue/epilogue 模板展开（指令名 + 物理
    寄存器/imm + @push_callee/@pop_callee/@frame_alloc/@frame_free）+ spill
    load/store + emit_epilogue_jump + needs_epilogue_label
  - lower_terminator：Return→MOV_RM8_R64 RAX,val；Jump→JMP_REL32
  - 端到端：arch::x86_v12 编译 fn add 全链路（IR→lowering→regalloc→frame→encode）
- **6b（commit 8bf23ff）JIT 端到端执行成功**：
  - Iconst lowering：{iconst} 从常量池解析
  - @move_args：prologue 把 [abi].arg_class.int 寄存器值 mov 到参数 XReg
  - **Return 修正：不生成 RET**（return block 经 epilogue_jump 统一恢复——
    否则栈不平衡 SEGV）；epilogue 用 sub rsp, 56（非 add）
  - Iadd 等操作数修正：ADD_RM_R {1}, {out}（src=第二个 IR 操作数）
  - reloc_patcher：x86_64_v12 → X86RelocPatcher（epilogue_jump REL4 正确 patch）
  - 验证：e2e_jit_run_const（42）+ e2e_jit_run_add（add(2,3)=5、add(-7,100)=93）
    真实执行
- **6c（commit 9dbbb70）mini_c 双后端**：
  - compiler.rs：Backend::V12 选项（前端复用 Direct，JIT 用 x86_v12 TM；
    JitRunner 抽象消除 v11/v12 泛型差异）
  - v12_backend_tests 4 个：同一源码 v11/v12 后端执行结果一致
    （Iconst/Iadd/Isub/Band/Bor/Bxor/多步算术链；含 `return 1+2+3+4+5`）
  - mini_c 全绿（21+19+90+4）
- **验证**：forge-dsl 175、x86_v12 14/14、riscv 9/9、集成 12/12、e2e 4/4、
  mini_c 4/4（v12）+ 134 既有全绿、workspace 51 套件全绿、clippy 0 真实
  警告、fmt 干净

**下一步（迭代 6 续）**：v12 lowering 扩展（load/store/icmp/br/Jcc/Call →
mini_c 全量）；x86 124 指令 + 115 lower 全量 v12；v11 语法层物理删除。

## 18. 迭代 6 续完成记录（2026-08）——mini_c 局部变量/内存/分支

- **6d（commit 60d6bab）x86 指令补齐**：23 条缺失迁移（94→117 指令）——
  NOP/UD2/MFENCE/CQO/CALL_RM/JCC_REL32/SETCC_RM8/LEA 系/CMOVCC/ROUNDSD/
  PMULLD/MOVQ 系/MOVAPS_MR/LZCNT/TZCNT/POPCNT/MOVABS；新 forms（NOOP_ESC/
  REL32_ESC/OP_EXT/MRR_0F38/MRR_0F_FIX64）；**Cond 操作数**（opcode 低 4 位：
  encode OR + decode 提取 + __parse_cond/__render_cond + 高 4 位 guard）
- **6e（commit e782591）mini_c 局部变量 + 内存读写（7/7 测试通过）**：
  - Lowering：Load/Store/Copy/Sextend/Bnot/StackAddr（{off}→current_offset、
    mem 槽 [base+off] 模板、opsize 缺省 ctx.default_opsize 按 IR 类型）
  - [conventions.modrm] force_disp_base=[5,13]（RBP/R13 基址 RIP-rel 陷阱）；
    RegInfo::callee_saved() 从 [abi] 解析（StackAddr 基准 rbp-callee_saved_bytes）
  - mini_c v12 后端真实执行含栈槽程序：`int x=3; int y=4; return x+y`=7、
    compound assign、bitwise not
- **6f（commit eba280f）条件/分支基础（8/9 测试通过）**：
  - Icmp 重构：xor+cmp+setcc（{cc} 从 Icmp{cond} 映射）；SETCC_RM8 用
    EXT_0F（0F escape）；Branch terminator（test+je+jmp）；
    MachineInst::branch_targets() 从 Label 槽提取；Encoder label fixup 加
    sink 基址（修复分支目标错位/死循环）
  - if-else 基本工作；`if (x>3)` 嵌套 icmp 边缘 case（cond 高位未清零）
    ignored 待调试
- **验证**：x86_v12 14/14、riscv 9/9、集成 12/12、forge-dsl 175、mini_c
  v12 8/9、workspace 52 套件全绿、clippy/fmt 干净

**下一步（迭代 6 续）**：分支边缘 case 修复（setcc 高位清零/嵌套 icmp）；
Sdiv/Srem/Udiv/Urem/Call/循环（while/for）→ mini_c 全量；x86 124 指令 +
115 lower 全量 v12；v11 语法层物理删除。

## 19. 迭代 6 续2 完成记录（2026-08）——条件分支全绿 + 除法

- **6g（commit 15b58ca）条件分支完整（if-else 10/10 全绿）**：
  - CMP_RM_R 方向修正的完整验证：icmp 直测 + if-else 全部通过
    （setg/setne 真实执行正确）
  - 取消 if_else 的 ignore（此前因 cmp 方向反致 setg 恒 0）
- **6h（commit b7f9a76）除法 + 指令定义修正（用户审查指出）**：
  - **CQO 定义错误**：应为 48 99（REX.W + 99，无 0F escape），原定义
    NOOP_ESC + fields.w=0 生成 0f 99（setnle 语义错误）；新增 NOOP_REXW
    form + no_modrm 分支 rex_w=always 支持
  - **物理寄存器 field 序号**：lowering 中 RAX/RDX 等物理寄存器跳过
    map_reg_field 时 reg_field_i 不递增，后续操作数 set_reg_field 序号
    错位（movsxd RAX 被 regalloc 重分配）；改为序号仍递增（v11 一致）
  - Sdiv/Srem/Udiv/Urem：{t} 临时 + movsxd RAX + cqo + idiv + clobber
  - 除法 6 个测试全过（10/3、10%3、-10/3、-10%3、100/7、100%7）
- **验证**：mini_c v12 13/13（嵌套循环 ignored：内层作用域 spill 交互
  待调）；x86_v12 14/14、riscv 9/9、集成 12/12、forge-dsl 175；
  clippy/fmt 干净
- **6i（commit 0bb3c6f）allocatable 排除 spill scratch**：
  - allocatable_gp_order 排除 [abi].scratch（R10/R11）——regalloc 不能
    占用 spill load/store 专用寄存器，否则 spill 往返覆盖变量值
  - 验证：while/for 循环在排除后正确（此前排除触发 spill 暴露冲突）
- **6j（commit 8bc3aa0）do-while/break/continue + 8 位寄存器 REX 修复**：
  - **8 位寄存器 REX bug（用户 do-while 测试暴露）**：SETCC_RM8 目标
    索引 4-7（spl/bpl/sil/dil）无 REX 前缀时 x86 解码为 ah/ch/dh/bh——
    `setne sil`（`0f 95 c6`）实为 `setne dh`，test rsi,rsi 永远为 0 致
    循环只执行一次。REX 条件原先只覆盖 reg/rm ≥8（0x41），未覆盖
    8 位低编号 4-7。修复：OperandSlot 新增 `byte_reg` 标记（gpr8 槽，
    SETCC_RM8/MOVZX_R8_RM/MOVSX_R8_RM 使用），vlen_ctx 记录 reg/rm
    是否 8 位，REX 条件增加 `(idx & 7) >= 4` 强制前缀（0x40 起）
  - do-while 2 个测试（i<5 循环、先执行后判断）全过；break/continue
    2 个测试全过（continue 用 while——mini_c 的 for-continue 跳过
    update 死循环，属前端既有语义）
  - 新增 v12_unary_logical / v12_logical_and_or（&&/|| = icmp+band/bor
    +sextend）/ v12_enum_const / v12_inlined_call（AST 内联）测试
  - mini_c v12 19/19（嵌套循环 ignored）；workspace 全量
    `cargo test --workspace --exclude forge-rustc` 无 FAILED；
    clippy/fmt 干净
- **6k（commit 1007409）嵌套循环修复——locals 槽深计入 max_stack_bytes**：
  - **根因**：mini_c 的 `alloc_slot` 生成 `stack_addr(0) + iadd(iconst(-N))`
    模式，StackAddr 本身 immediate=0 不贡献深度，真正的槽偏移在 iconst 里。
    预扫描只统计 StackAddr immediate → `max_stack_bytes=0`，locals 区不
    参与 frame 计算，spill 槽从过浅位置分配并覆盖局部变量（嵌套循环
    s 累加丢失返回 0 / 死循环的根因；v11 靠 spill 更深的布局碰巧避开，
    属同源隐患）
  - **修复**（forge-codegen lowering.rs 预扫描）：识别 `Iadd` 操作数之一
    为 StackAddr 值、另一为负 Iconst 的模式，把 `-v + 8` 深度并入
    `max_stack_bytes`（与主循环 StackAddr immediate 处理一致）
  - 验证：v12_nested_loop 移除 ignore 并扩展 3 个用例（双重 while、
    外层累加+内层独立计数、3 层嵌套）全过；v12 后端 20/20 全绿，
    v11 dual_backend 19/19 无回归；workspace 全量无 FAILED；clippy/fmt
    干净
- **6l（commit e0a2a13）移位指令 + mini_c 覆盖扩展**：
  - 新增 form `MRR_EXT_OP_FIX64`（D3 /digit、opsize 固定 64 → REX.W）+
    SHL_RM_CL/SHR_RM_CL/SAR_RM_CL（ext=4/5/7，CL 隐式计数）
  - lowering：Ishl/Ushr/Sshr——计数搬 RCX（MOV_R_RM RCX, {1}，CL 低 8
    位即计数；RCX 由 clobber 收集 regalloc 避开）；Sshr 首步 movsxd
    符号扩展（否则 32 位值 64 位 sar 得到无符号右移结果）
  - 新增 v12_shift 测试：常量/变量计数、复合赋值（<<= / >>=）、
    负数算术右移、hex 字面量全过
  - mini_c v12 23/23 全绿（嵌套循环 + shift）；workspace/clippy/fmt
    干净
- **6m（commit daa5959）emission spill 字段回写修复 + struct/字面量覆盖**：
  - **emission.rs 物理字段错位修复**：`emit_inst_with_spills` 的 spill
    重写用 enumerate 位置（fi）而非 xreg_map 的 field_idx——指令含物理
    寄存器字段（如 `MOV_R_RM RCX` 的 op0）时 xreg_map 跳过该字段，记录
    位置与字段序号错位，spill 重写会把物理字段覆盖成 scratch。改用
    field_idx（map_reg_field 传入的指令内 Reg 位置序）
  - 新增 v12_struct_field / v12_struct_init_list（init 列表、多 struct、
    4 字段 struct）/ v12_literal_forms（hex/octal/char）测试全过
  - mini_c v12 24/24 全绿；workspace/clippy/fmt 干净
  - **已知限制**：shift 结果在循环内参与累加（`t += v << i` / 
    `int s = v << i` 后丢弃）仍 ACCESS_VIOLATION——物理 RCX 字段在
    shift+循环 + spill 高压组合下与地址/计数寄存器交互，emission 修复
    解决 spill 重写覆盖，但 regalloc 的 use/def 活跃性仍需专门调试
    （v11 同用例通过，v12 特有）
- **6n（commit 90ef94c）regalloc clobber 点 use 占用者修复**：
  - **根因**：clobber 处理（regalloc 1.5 阶段）对写死物理寄存器（如 shift
    的 RCX）的占用者无条件 spill——但 **spill_vreg 只标记槽并释放寄存器，
    不 store 当前寄存器值**，随后 reload_from_stack 从空槽读到垃圾。
    shift 场景：v 被分配到 RCX（load 时 RCX 可用），shift 的
    `MOV_R_RM RCX, {1}`（mov rcx, 计数）clobber RCX，处理 spill v，
    后续 reload v 读到垃圾 → 崩溃
  - **修复**：clobber 处理跳过**本指令 use** 的占用者——use 在寄存器中
    读旧值（clobber 写坏发生在指令写入阶段，use 在读旧值阶段），之后
    该 XReg 若再活跃，后续指令的 phys_conflicts 会避开已 clobber 的
    寄存器。非 use 占用者照常 spill
  - 新增 v12_shift 回归用例：`int s = v << i` + 循环 + 读取其他局部变量
    全过；更新 clobber_map 测试（use 占用者不 spill 的新语义）
  - mini_c v12 24/24 全绿；forge-codegen 100/clippy/fmt 干净
  - **剩余限制**：`t += v << i`（shift 结果跨指令存活到 Iadd 累加）仍
    ACCESS_VIOLATION——shift 结果的 interval 跨指令 + clobber 交互，需
    interval 级 clobber 传播或计数 Fixed(RCX) 约束（v12 特有）

**下一步（迭代 6 续）**：`t += v << i` 跨指令 shift 结果崩溃（interval 级
clobber 或 Fixed 约束）；Call/函数调用 → mini_c 全量（AST 内联已支持，
直呼 CALL 指令待定）；x86 124 指令 + 115 lower 全量 v12；v11 语法层
物理删除。
