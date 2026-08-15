# 汇编语法规范（DSL 驱动的 TargetAssembler）

本文件描述 `forge-dsl` 生成每 ISA 专属汇编器的语法规范：**asm 模板即语法
规范**——`[inst.*].asm` 模板在编译期被编译为 lalrpop 语法规则与类型签名，
运行时由共享词法/中间表示基础设施驱动。

## 架构总览

```text
isa/*.toml（asm 模板 + [cc_names] + [reg.*]）
   │  forge-dsl（proc-macro 编译期）
   ├─ asm_grammar：模板 → 类型签名 → 每 ISA .lalrpop 语法 → lalrpop 编译为 parser 代码
   ├─ Token 枚举（logos derive，每 ISA 一份：保留字 + 通用 token）
   └─ bind 层生成（字段绑定 + 候选组语义消歧 + 两遍标签解析）
   ▼
forge-asm（运行时引擎）
   ├─ TokenStream（泛型 logos 词法流）/ LexError
   └─ OperandValue / OperandTy / AsmLine / RawInst（类型化中间表示）
   ▼
forge-codegen
   └─ machine/assembler.rs：TargetAssembler trait（parse_insts / parse_lines / bind）
```

## 语法规范来源：asm 模板

每条 `[inst.*].asm` 模板定义一条指令的汇编语法：

| 模板元素 | 生成语法 |
| --- | --- |
| 字面 mnemonic（`mov`、`fadd.s`） | 保留字 token（priority=2，胜于 Ident） |
| 内嵌 cond（`set{cond}`、`j{cond}`、`b.{cond}`、`cmov{cond}`） | 按 `[cc_names]` 展开为多个 mnemonic（`sete`/`setne`/…） |
| 字面锚点（`,`、`0x`、`#`、`[`、`]`、`+`、`*`、`(`、`)`、`!`） | 标点 token / 前缀专用非终结符（HexImm/HashImm） |
| 字段：`Ireg/Freg/GprReg/XmmReg` | `Reg` 非终结符（Ident/TmpReg/VRegLit/保留字） |
| 字段：`I/U 整数` | `Imm`（IntLit/HexLit/HashInt/ConstLit，支持负号） |
| 字段：`F32/F64` | `Float`（FloatLit） |
| 字段：`MemRef` | `Mem`（`[base]` / `[base ± disp]`） |
| 字段：`BlockTarget` | `Label`（`.Lxxx` / `@xxx` / 数字） |
| 字段：`CondCode`（独立操作数） | `Cond`（cc 名 → 值） |
| 字段：`Opsize` | 隐式，绑定默认 64 |
| 模板内 `;;`（wasm 注释） | 裁剪，其后内容不参与规则 |

## 词法 token（logos）

- **保留字**（每 ISA 生成）：全部 mnemonic（含 cc 展开与点号变体）、cc 名、
  模板字面标识符（如 `lea_off` 模板里的 `RBP`）。`priority = 2`。
- **通用 token**：`Ident`（priority=1，允许内部点号：`fmov.bits`）、
  `DotLabel`（`.Lxx`）、`AtLabel`（`@xx`）、`TmpReg`（`%t`）、
  `VRegLit`（`VReg(N)`）、`ConstLit`（`{const N}`）、`HexLit`（`0xFF`）、
  `IntLit`、`HashInt`（`#5`，aarch64）、`FloatLit`、标点
  （`,` `[` `]` `(` `)` `+` `-` `*` `!` `:`）、`Newline`。
- **注释**：`//`、`;`、`#`（后跟非数字）到行尾。
- 负数由语法层组合（`-` + 立即数），`[RBP-8]` 与 `[RBP+8]` 均接受。

## 类型系统消歧（双层）

1. **语法层**：同 mnemonic 候选按操作数非终结符首 token 区分
   （`Reg`→Ident、`Imm`→IntLit、`Mem`→`[`、`Label`→`.L`/`@`）——LR lookahead。
2. **语义层**：同 mnemonic 同语法形状的候选合并为**候选组**，bind 时按
   **字段类型签名**过滤：
   - 特异度打分：`GprReg/XmmReg` 物理约束（权重 2）优于 `Ireg/Freg`（权重 1）
     ——如 `add RAX, 5` 选 `ADD64_R_IMM32`（GprReg）而非 `ADD_R_IMM32`（Ireg）。
   - 唯一匹配 → 绑定；多匹配同权重 → `AsmError::Ambiguous`；
     无匹配 → `AsmError::TypeMismatch`。
3. **生成期**：保留字 token 变体命名碰撞自动加数字后缀，不阻断编译。

## bind 字段绑定

| 字段类型 | 绑定规则 |
| --- | --- |
| `Ireg/Freg` | 任意 Reg 操作数 → 物理寄存器（查 `[reg.*]` 表） |
| `GprReg` | 必须物理 GPR（`TypeMismatch` 否则） |
| `XmmReg` | 必须物理 FPR |
| 整数 | 宽度/符号范围检查（i8: -128..=127 …） |
| `CondCode` | cc 名查 `[cc_names]` → u8 值 |
| `MemRef` | base 查表 + disp（含负号）→ `MemRef::new` |
| `BlockTarget` | 标签→**字节偏移**两遍解析：首遍收集标签→指令序号，占位绑定后经`Encoder::encoded_size` 逐条累计字节长度，次遍回填字节偏移；数字标签（`jmp 8` / `jmp 0x10`）直接作为偏移 |
| `Opsize`（隐式） | 默认 64 |

## 指令包（lower/emit insts）语法

`parse_lines`（中间表示 API）完整支持指令包文本：

- 虚拟操作数（`rd`/`rs1`/`rs2`/`func`/`iconst`/…）→ `OperandValue::Ident`
- 临时寄存器 `%t` → `TmpReg`；`VReg(N)` → `VReg`；`{const N}` → `Const`
- `@pop_callee` 等 emit 宏 → 伪指令行（`inst_idx = usize::MAX` 哨兵）
- 物理寄存器/立即数混合（如 `xor RDX, RDX`）→ 正常绑定

`parse_insts` 对虚拟操作数（无法物理化）报 `TypeMismatch`；
`parse_lines` 完整保留操作数值供调用方消费。

## 用法

```rust
use code_forge::machine::assembler::TargetAssembler;
use code_forge::x86_64::Assembler;

let asm = Assembler;
let insts = asm.parse_insts("mov RAX, RBX\nadd RAX, 5\n")?;      // Vec<Inst>
let lines = asm.parse_lines("mov rd, rs1\n")?;                    // 中间表示
```

## 设计边界与说明

- **标签地址为字节偏移**：占位绑定（rel=0）后经每 ISA `Encoder::encoded_size`
  计算逐条指令长度，标签回填为字节偏移（rel 固定宽度，占位不影响长度）。
- `MemRef` 绑定支持 `[base ± disp]`（x86 spill 模板形态，`MemRef` 类型仅含
  base/offset）；SIB 展开式（`lea` 的 `[base+index*scale+disp]`）按独立字段
  绑定——这是 `MemRef` 类型的能力边界，非解析器限制。
- 模板字面写死的标识符（如 `[RBP+…]`）会作为保留字 token，但 Reg 规则显式
  接受全部保留字，`mov RBP, …` 等写法不受影响。
- 同 mnemonic 同权重候选仍歧义（如 aarch64 `mov` 的 `Ireg,GprReg` 双候选
  定义重叠）时按特异度选优，仍无法区分则报 `Ambiguous`（安全优先，不猜测）。
