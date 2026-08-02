# ISA-DSL v10 — 语法规范与使用方法

## 目录

1. [概述](#概述)
2. [快速开始](#快速开始)
3. [TOML 结构总览](#toml-结构总览)
4. [`[meta]` — 元信息](#meta--元信息)
5. [`[reg.*]` — 寄存器组](#reg--寄存器组)
6. [`[abi]` — ABI 声明](#abi--abi-声明)
7. [`[inst.*]` — 指令定义](#inst--指令定义)
8. [`[lower.*]` — IR Lowering 规则](#lower--ir-lowering-规则)
9. [`[lower_term.*]` — 终止指令 Lowering](#lower_term--终止指令-lowering)
10. [`[emit.*]` — 序言/尾声](#emit--序言尾声)
11. [代码生成器输出](#代码生成器输出)
12. [完整的 x86-64 示例](#完整的-x86-64-示例)
13. [标准指令集与默认 Lowering](#标准指令集与默认-lowering)
14. [字段参考速查表](#标准指令全集)

---

## 概述

**ISA-DSL v10** 是一个基于 TOML 的指令集架构（ISA）定义系统，通过 Rust proc macro 自动生成：

- **Reg 枚举** — 物理寄存器，实现 `PhysReg` trait
- **Inst 枚举** — 机器指令，实现 `MachineInst` trait（编码发射、效果分析、寄存器使用）
- **Isa 结构体** — ISA 后端，实现 `InstructionSet` trait（lowering、代码发射、ABI 信息）
- **IsaInfo trait 实现** — 运行时 ISA 元信息反射

**核心原则**：在 TOML 中声明 ISA 语义（寄存器、指令、lowering 规则），代码生成器输出类型安全的 Rust 代码。

## 快速开始

### 入口宏

```rust
// 方式 1：内联 TOML
use codegen_dsl::isa;
isa! {
    // 这里是 TOML 文本，直接写在宏调用中
    [meta]
    name = "my_arch"
    // ...
}

// 方式 2：从文件加载（推荐）
use codegen_dsl::isa_from_file;
isa_from_file!("isa/my_arch_v1.toml");

// 方式 3：显式指定完整路径
isas_from_file!("examples/isa/x86_64_v10.toml");
```

`isa_from_file!` 先相对于 `CARGO_MANIFEST_DIR` 查找文件，找不到再相对于当前工作目录查找。

### 最小完整示例

下面是一个只有一条 ADD 指令的最简 RISC-like ISA：

```toml
[meta]
name = "mini"
version = "1.0"
endian = "little"
mode = 64

[reg.gpr]
count = 8
width = 64
names = ["R0","R1","R2","R3","R4","R5","R6","R7"]

[inst.ADD_RR]
fields = [{ name = "dest", type = "VReg" }, { name = "src", type = "VReg" }]
[inst.ADD_RR.emit]
template = "enc_rr(sink, &[0x00], preg(*dest, rm)?, preg(*src, rm)?, EncFlags::default());"
[inst.ADD_RR.effect]
effect = ["Pure"]

[inst.MOV_REG_IMM]
fields = [{ name = "reg", type = "VReg" }, { name = "imm", type = "I64" }]
[inst.MOV_REG_IMM.emit]
template = "/* emit MOV imm -> reg */"
[inst.MOV_REG_IMM.effect]
effect = ["Read", "Write"]

[lower.Iconst]
insts = [{ inst = "MOV_REG_IMM", args = { reg = "rd", imm = "iconst" } }]
[lower.Iadd]
insts = [{ inst = "MOV_REG_IMM", args = { reg = "rd", imm = "rs1" } }, { inst = "ADD_RR", args = { dest = "rd", src = "rs2" } }]
```

生成的 Rust 代码：

```rust
pub mod mini {
    use crate::prelude::*;

    #[derive(Clone, Copy, Debug, PartialEq, Eq)]
    pub enum Reg { R0, R1, R2, R3, R4, R5, R6, R7 }

    #[derive(Clone, Debug, PartialEq, Eq)]
    pub enum Inst {
        AddRr { dest: VReg, src: VReg },
        MovRegImm { reg: VReg, imm: i64 },
        Unknown(VReg),
    }

    impl MachineInst for Inst {
        fn uses(&self) -> SmallVec<[VReg; 4]> { /* ... */ }
        fn defs(&self) -> SmallVec<[VReg; 4]> { /* ... */ }
        fn effects(&self) -> SmallVec<[EffectKind; 2]> { /* ... */ }
        fn is_branch(&self) -> bool { /* ... */ }
        fn branch_targets(&self) -> SmallVec<[BlockId; 2]> { /* ... */ }
        fn has_side_effects(&self) -> bool { /* ... */ }
        fn is_move(&self) -> bool { /* ... */ }
    }

    pub struct Isa;
    // Isa implements InstructionSet with lower(), emit(), ABI methods
}
```

---

## TOML 结构总览

ISA-DSL TOML 文件由以下顶层 section 组成：

| Section | 必需 | TOML 语法 | 说明 |
| ------- | ---- | --------- | ------ |
| `[meta]` | ✅ | 单表 | ISA 名称、版本、端序、位宽、能力 |
| `[reg.<group>]` | ✅ (至少 `gpr`) | 多表 | 寄存器组（gpr, xmm, ymm 等） |
| `[abi]` | ❌ | 嵌套表 | 调用约定、栈对齐、参数/返回值寄存器 |
| `[inst.<NAME>]` | ✅ | 多表 + 子表 | 机器指令定义（字段、编码、效果） |
| `[lower.<Opcode>]` | ✅ | 多表 | IR Opcode → 机器指令的转换规则 |
| `[lower_term.<Term>]` | ❌ | 多表 | 终止指令（Return/Jump/Branch）的 lowering |
| `[emit.prologue]` / `[emit.epilogue]` | ❌ | 子表 | 函数序言/尾声的汇编模板 |

> **命名约定**：指令名使用 `SCREAMING_SNAKE_CASE`（如 `MOV_R8_RM`），生成的 Rust 枚举变体自动转为 `PascalCase`（如 `MovR8Rm`）。

---

## `[meta]` — 元信息

### `[meta]`全部字段

```toml
[meta]
name = "x86_64"             # (必需) ISA 名称，用作 Rust 模块名
version = "10.0"            # (默认 "") 版本字符串 → IsaInfo::version()
endian = "little"           # (默认 "little") 端序，可选 "big"
mode = 64                   # (默认 64) 地址位宽 → IsaInfo::address_size()
max_inst_len = 15           # (默认 0) 最大指令长度（字节），0 表示定长
```

### `[meta.capabilities]` — 架构能力

```toml
[meta.capabilities]
variable_length = true      # (默认 false) 变长指令集？
prefix_layers = 4           # (默认 0) 前缀层级（x86:4, RISC-V:0）
simd_widths = []            # (默认 []) SIMD 寄存器宽度列表
mask_registers = false      # (默认 false) 是否有掩码寄存器（AVX-512）
broadcast = false           # (默认 false) 是否支持广播
rounding_mode = false       # (默认 false) 是否支持舍入模式
```

### 生成代码映射

| TOML 字段 | 生成的 Rust |
| --------- | --------- |
| `name` | 模块名 `pub mod x86_64` |
| `version` | `fn IsaInfo::version() -> &'static str` |
| `mode` | `fn IsaInfo::address_size() -> u8` |
| `capabilities.*` | `fn IsaInfo::capabilities() -> IsaCapabilities` |

---

## `[reg.*]` — 寄存器组

### 语法

```toml
[reg.gpr]                      # 整数通用寄存器组
count = 16                     # (必需) 寄存器数量
width = 64                     # (默认 64) 寄存器位宽
names = ["RAX","RCX","RDX",    # (可选) 显式列举寄存器名
         "RBX","RSP","RBP",
         "RSI","RDI","R8","R9",
         "R10","R11","R12",
         "R13","R14","R15"]
prefix = "R"                   # (可选) 寄存器前缀（用于自动生成名称）

[reg.xmm]                      # 浮点/SIMD 寄存器组
count = 16
width = 128
prefix = "XMM"
```

### 生成规则

- **有 `names`**：直接枚举，例 `names = ["RAX","RCX"]` → `enum Reg { RAX, RCX, ... }`
- **无 `names`，有 `prefix`**：自动生成 `prefix0`, `prefix1`, ... `prefix{count-1}`
- **两者都无**：生成 `R0`, `R1`, ... `R{count-1}`

### 寄存器索引

生成的 `Reg` 枚举实现 `PhysReg` trait：

```rust
impl PhysReg for Reg {
    fn to_index(self) -> u8 { self as u8 }
    fn from_index(idx: u8, class: RegClass) -> Self { /* ... */ }
    fn class(self) -> RegClass {
        // gpr → RegClass::Int, xmm → RegClass::Float
    }
}
```

---

## `[abi]` — ABI 声明

### `[abi]`全部字段

```toml
[abi]
stack_align = 16              # (默认 16) 栈对齐字节数
red_zone = 128                # (默认无) 红区大小，x86-64 System V 为 128

[abi.frame]
sp = "RSP"                    # (必需) 栈指针寄存器名
fp = "RBP"                    # (可选) 帧指针寄存器名

[abi.callee_saved]            # 被调用者保存寄存器
gpr = ["RBX","R12","R13","R14","R15"]
xmm = []                      # (默认 []) XMM 被调用者保存寄存器

[abi.arg_regs]                # 函数参数传递寄存器
gpr = ["RDI","RSI","RDX","RCX","R8","R9"]
xmm = ["XMM0","XMM1","XMM2","XMM3","XMM4","XMM5","XMM6","XMM7"]

[abi.ret_regs]                # 返回值寄存器
gpr = ["RAX"]
xmm = ["XMM0"]

[abi.precolor]                # 虚拟寄存器预着色（固定映射）
VReg0 = "RAX"                 # VReg(0) → RAX
VReg2 = "RDX"                 # VReg(2) → RDX
```

### `[abi]`生成代码映射

| TOML 路径 | 生成的 `InstructionSet` 方法 |
| --------- | --------------------------- |
| `abi.frame.sp` | `fn sp_reg() -> FrameAccess<Reg>` |
| `abi.frame.fp` | `fn fp_reg() -> FrameAccess<Reg>` |
| `abi.callee_saved` | `fn callee_save_regs() -> Vec<Reg>` |
| `abi.arg_regs` | `fn arg_regs() -> Vec<Reg>` |
| `abi.ret_regs` | `fn ret_regs() -> Vec<Reg>` |
| `abi.stack_align` | `fn stack_align() -> u32` |
| `abi.precolor` | `fn precolored_vregs() -> HashMap<VReg, u8>` |

---

## `[inst.*]` — 指令定义

指令是 ISA-DSL 的核心单元。每条指令定义一个枚举变体，以及它的编码、效果和对寄存器的使用。

### 基本语法

```toml
[inst.ADD_RM8_R8]
fields = [
    { name = "dest", type = "VReg" },
    { name = "src",  type = "VReg" }
]
[inst.ADD_RM8_R8.emit]
template = "enc_rr_simple(sink, 0x01, preg(*src, rm)?, preg(*dest, rm)?);"
[inst.ADD_RM8_R8.effect]
effect = ["Pure"]
```

等价于生成的 Rust 代码：

```rust
enum Inst {
    AddRm8R8 { dest: VReg, src: VReg },
    // ...
}
```

### `fields` — 指令字段

字段定义每条指令的操作数。字段在生成的 `Inst` 枚举变体中按 **TOML inline table 的 BTreeMap 字母序**排列（例如 `{ dest, src }` → `[dest, src]`，`{ cond, dest }` → `[cond, dest]`）。

> **重要**：`[lower.*]` 和 `[emit.*]` 中的操作数顺序必须与 BTreeMap 字母序一致。例如 `SD_SETCC` 的字段字母序为 `[cond, dest]`，lowering 序列应写为 `SD_SETCC 0, rd`（先 cond 再 dest）。asm 模板中的 `{field}` 占位符不影响 lowering 操作数顺序——它仅用于汇编器解析和消歧。

| FieldType | Rust 类型 | 用途 |
| --------- | ---------- | ------ |
| `VReg` | `VReg` | 虚拟/物理寄存器操作数 |
| `I64` | `i64` | 64 位立即数 |
| `U8` | `u8` | 8 位无符号立即数 |
| `U32` | `u32` | 32 位无符号立即数 |
| `F64` | `f64` | 64 位浮点立即数 |
| `BlockTarget` | `i64` | 基本块跳转目标（用于 branch_targets 提取） |
| `Reg` | `Reg` | 物理寄存器（固定寄存器引用） |
| `CondCode` | `u8` | 条件码（如 SETcc / Jcc 的 opcode 后缀） |

> **命名约定**：名为 `dest` 或 `reg` 的字段自动标记为 **def**（写入）寄存器操作，其他 `VReg` 字段标记为 **use**（读取）寄存器操作。这影响寄存器分配的 live-range 分析。

### `emit.template` — 编码模板

`template` 是一段**内联 Rust 代码**，直接插入到生成的 `emit_inst()` 函数中。

可用的上下文变量：

| 变量 | 类型 | 说明 |
| --------- | ------ | ------ |
| `sink` | `&mut CodeSink` | 代码输出缓冲区 |
| `rm` | `&RegMap` | 寄存器映射（VReg → 物理寄存器编号） |
| `preg(v, rm)?` | `Result<u8, CompileError>` | 将 VReg 解析为物理寄存器编号 |
| `*field_name` | (字段类型) | 解引用指令字段值 |
| `fs` | `u32` | 函数帧大小（仅在 prologue/epilogue 中） |

**常用编码辅助函数**（定义在 `crate::backend::encode`）：

| 函数 | 用途 |
| ------ | ------ |
| `enc_rr_simple(sink, opcode, reg, rm)` | 单字节 opcode + ModRM (自动 REX) |
| `enc_rr_0f(sink, opcode, reg, rm)` | 双字节 opcode (0F prefix) + ModRM |
| `enc_sse_rr(sink, prefix, opcode, w, reg, rm)` | SSE 双操作数 (prefix + 0F + opcode + ModRM) |
| `modrm(mod, reg, rm)` | 计算 ModRM 字节 |
| `sink.put1(u8)` / `sink.put4(u32)` / `sink.put8(u64)` | 写入 1/4/8 字节 |
| `sink.offset()` | 当前偏移（用于 fixup） |
| `sink.use_label_at(offset, block, RelocKind)` | 记录重定位 fixup |

**emit template 示例**（复杂指令）：

```toml
# SETcc — 条件设置，根据 cond 字段选择 opcode
[inst.SETCC_RM8]
fields = [
    { name = "dest", type = "VReg" },
    { name = "cond", type = "CondCode" }
]
[inst.SETCC_RM8.emit]
template = """
let __d = preg(*dest, rm)?;
if __d >= 4 { sink.put1(0x40 | if __d >= 8 { 1u8 } else { 0u8 }); }
sink.put1(0x0F);
sink.put1(0x90 | (*cond));
sink.put1(modrm(3, 0, __d));
"""
```

### `effect` — 指令效果

效果声明用于代码生成器判断指令的副作用属性：

| 效果值 | 含义 | 触发行为 |
| -------- | ------ | --------- |
| `Pure` | 纯计算，无副作用 | `has_side_effects() = false` |
| `Read` | 读取状态 | `has_side_effects() = true`（保守） |
| `Write` | 写入状态 | `has_side_effects() = true` |
| `Branch` | 条件分支 | `is_branch() = true` |
| `Jump` | 无条件跳转 | `is_branch() = true` |
| `Trap` | 陷阱/异常 | 特殊处理 |
| `Ret` | 函数返回 | `is_ret() = true` |
| `Call` | 函数调用 | `is_call() = true` |

> **规则**：只要 effect 列表非空且不全为 `Pure`，则 `has_side_effects() = true`。

**两种 TOML 写法**：

```toml
# 写法 1：内联数组（推荐）
[inst.ADD_RM8_R8.effect]
effect = ["Pure"]

# 写法 2：子表语法（兼容旧格式）
[inst.ADD_RM8_R8.effect]
[inst.ADD_RM8_R8.effect.effect]
effect = ["Pure"]
```

两种写法等价，代码生成器通过 `EffectsCompat` 枚举自动兼容。

### 特殊字段：`BlockTarget`

标记为 `BlockTarget` 的字段会被 `branch_targets()` 方法自动提取：

```toml
[inst.JMP_REL32]
fields = [{ name = "rel", type = "BlockTarget" }]
[inst.JMP_REL32.effect]
effect = ["Jump"]

# 生成代码：
# fn branch_targets(&self) -> SmallVec<[BlockId; 2]> {
#     Inst::JmpRel32 { rel, .. } => smallvec::smallvec![BlockId(*rel as u32)]
# }
```

---

## `[lower.*]` — IR Lowering 规则

Lowering 规则定义 **IR Opcode → 机器指令序列** 的转换。每个规则指定一个或多个机器指令，用 `args` 映射 IR 操作数到机器指令字段。

### `[lower.*]`基本语法

```toml
[lower.Iadd]
insts = [
    { inst = "MOV_R8_RM", args = { dest = "rd", src = "rs1" } },
    { inst = "ADD_RM8_R8", args = { dest = "rd", src = "rs2" } }
]
```

规则名称是 IR Opcode（如 `Iadd`, `Isub`, `Imul`）。对于有子变体的 Opcode（如 `Icmp`），使用点号分隔：

```toml
[lower.Icmp.Equal]
insts = [
    { inst = "XOR_RM8_R8", args = { dest = "rd", src = "rd" } },
    { inst = "CMP_RM8_R8", args = { src1 = "rs1", src2 = "rs2" } },
    { inst = "SETCC_RM8", args = { dest = "rd", cond = "0x94" } }
]
```

### `args` — 参数替换

`args` 是一个 `HashMap<String, String>`，将机器指令字段映射到 lowering 上下文中的值：

| args 值 | 展开 |
| --------- | ------ |
| `rd` | `result` VReg（IR 指令的结果寄存器） |
| `rs1` | `args[0]` — 第一个操作数 |
| `rs2` | `args[1]` — 第二个操作数 |
| `rs3` | `args[2]` — 第三个操作数 |
| `iconst` | `ctx.constant_pool[index].try_to_i64()` — 整数常量值 |
| `fconst` | `ctx.constant_pool[index].to_f64().to_bits()` — 浮点常量位 |
| `VReg(N)` | 固定虚拟寄存器编号（如 `VReg(0) = RAX`, `VReg(2) = RDX`） |
| `"字面量"` | 直接作为字面量嵌入（如 `"0x94"`, `"0x80000000"`） |

### 全部支持的 IR Opcode

#### 整数算术

| Opcode | 说明 | 操作数 |
| -------- | ------ | -------- |
| `Iadd` | 整数加法 | rd, rs1, rs2 |
| `Isub` | 整数减法 | rd, rs1, rs2 |
| `Imul` | 整数乘法 | rd, rs1, rs2 |
| `Udiv` | 无符号除法 | rd, rs1, rs2 |
| `Sdiv` | 有符号除法 | rd, rs1, rs2 |
| `Urem` | 无符号取余 | rd, rs1, rs2 |
| `Srem` | 有符号取余 | rd, rs1, rs2 |

#### 浮点算术

| Opcode | 说明 | 操作数 |
| -------- | ------ | -------- |
| `Fadd` | 浮点加法 | rd, rs1, rs2 |
| `Fsub` | 浮点减法 | rd, rs1, rs2 |
| `Fmul` | 浮点乘法 | rd, rs1, rs2 |
| `Fdiv` | 浮点除法 | rd, rs1, rs2 |
| `Fneg` | 浮点取反 | rd, rs1 |
| `Fabs` | 浮点绝对值 | rd, rs1 |
| `Fsqrt` | 浮点平方根 | rd, rs1 |

#### 位运算

| Opcode | 说明 | 操作数 |
| -------- | ------ | -------- |
| `Band` | 按位与 | rd, rs1, rs2 |
| `Bor` | 按位或 | rd, rs1, rs2 |
| `Bxor` | 按位异或 | rd, rs1, rs2 |
| `Bnot` | 按位取反 | rd, rs1 |
| `Ishl` | 左移 | rd, rs1, rs2 |
| `Ushr` | 逻辑右移 | rd, rs1, rs2 |
| `Sshr` | 算术右移 | rd, rs1, rs2 |

#### 比较

| Opcode | 说明 | 子变体 |
| -------- | ------ | -------- |
| `Icmp` | 整数比较 | `Equal`, `NotEqual`, `SignedLessThan`, `SignedLessThanOrEqual`, `SignedGreaterThan`, `SignedGreaterThanOrEqual`, `UnsignedLessThan`, `UnsignedLessThanOrEqual`, `UnsignedGreaterThan`, `UnsignedGreaterThanOrEqual` |
| `Fcmp` | 浮点比较 | `Equal`, `NotEqual`, `LessThan`, `LessThanOrEqual`, `GreaterThan`, `GreaterThanOrEqual`, `Ordered`, `Unordered` |

#### 类型转换

| Opcode | 说明 | 操作数 |
| -------- | ------ | -------- |
| `Sextend` | 符号扩展 | rd, rs1 |
| `Uextend` | 零扩展 | rd, rs1 |
| `Ireduce` | 整数截断 | rd, rs1 |
| `Bitcast` | 位重解释 | rd, rs1 |

#### 常量

| Opcode | 说明 | 特殊处理 |
| -------- | ------ | --------- |
| `Iconst` | 整数常量 | `{ index }` 解构 → 通过 `ctx.constant_pool` 查询 |
| `Fconst` | 浮点常量 | `{ index }` 解构 → 通过 `ctx.constant_pool` 查询 |

> `Iconst` 和 `Fconst` 的 lowering 规则中，`args` 的值可以包含 `iconst` 和 `fconst`，它们会自动从常量池中提取对应位的值。

#### 内存

| Opcode | 说明 | 操作数 |
| -------- | ------ | -------- |
| `Load` | 加载 | rd, rs1 |
| `Store` | 存储 | rs1, rs2 |
| `StackAddr` | 获取栈地址 | rd |

#### 其他

| Opcode | 说明 | 操作数 |
| -------- | ------ | -------- |
| `Copy` | 寄存器复制 | rd, rs1 |
| `Select` | 条件选择 | rd, cond, true_val, false_val |
| `Call` | 函数调用 | rd, func_ref, args... |
| `CallIndirect` | 间接调用 | rd, addr, args... |
| `Nop` | 空操作 | — |
| `Alloca` | 栈分配 | rd |
| `GetElementPtr` | 地址计算 | rd, ptr, indices... |
| `Phi` | Phi 节点（SSA） | (不生成机器指令) |

### 典型 lowering 模式

**二元运算**（mov + op）：

```toml
[lower.Iadd]
insts = [
    { inst = "MOV_R8_RM",  args = { dest = "rd", src = "rs1" } },
    { inst = "ADD_RM8_R8", args = { dest = "rd", src = "rs2" } }
]
```

**使用 scratch 寄存器**（移位需要 CL）：

```toml
[lower.Ishl]
insts = [
    { inst = "MOV_R8_RM", args = { dest = "rd", src = "rs1" } },
    { inst = "MOV_RM_R8", args = { dest = "VReg(97)", src = "rs2" } },
    { inst = "SHL_RM_CL", args = { dest = "rd", src = "VReg(97)" } }
]
```

**固定寄存器操作**（除法使用 RAX/RDX）：

```toml
[lower.Udiv]
insts = [
    { inst = "XOR_RM8_R8", args = { dest = "VReg(2)", src = "VReg(2)" } },
    { inst = "MOV_R8_RM",  args = { dest = "VReg(0)", src = "rs1" } },
    { inst = "DIV_RM",     args = { src = "rs2" } },
    { inst = "MOV_R8_RM",  args = { dest = "rd", src = "VReg(0)" } }
]
```

**常量加载**：

```toml
[lower.Iconst]
insts = [{ inst = "MOV_REG_IMM64", args = { reg = "rd", imm = "iconst" } }]
```

**浮点常量**（GPR scratch → MOVQ → XMM）：

```toml
[lower.Fconst]
insts = [
    { inst = "MOV_REG_IMM64", args = { reg = "VReg(96)", imm = "fconst" } },
    { inst = "MOVQ_XMM_R64",  args = { dest = "rd", src = "VReg(96)" } }
]
```

---

## `[lower_term.*]` — 终止指令 Lowering

终止指令处理基本块的控制流转移。

### `[lower_term.*]`基本语法

```toml
[lower_term.Return]
insts = [{ inst = "MOV_RM_R8", args = { dest = "VReg(0)", src = "val" } }]

[lower_term.Jump]
insts = [{ inst = "JMP_REL32", args = { rel = "target" } }]

[lower_term.Branch]
insts = [
    { inst = "TEST_RM8_R8", args = { dest = "cond", src = "cond" } },
    { inst = "JCC_REL32",   args = { cond = "0x84", rel = "false_block" } },
    { inst = "JMP_REL32",   args = { rel = "true_block" } }
]

[lower_term.Unreachable]
insts = [{ inst = "NOP", args = {} }]
```

### 特殊 args 值

| args 值 | 展开 |
| -------- | ------ |
| `val` | 返回值 VReg（仅 Return） |
| `target` | 跳转目标 BlockId（Jump） |
| `cond` | 条件值 VReg（Branch） |
| `true_block` | 真分支 BlockId（Branch） |
| `false_block` | 假分支 BlockId（Branch） |

---

## `[emit.*]` — 序言/尾声

定义函数入口和出口的汇编代码模板。

```toml
[emit.prologue]
template = """
sink.put1(0x55);                       // push rbp
sink.put1(0x48); sink.put1(0x89); sink.put1(0xE5);  // mov rbp, rsp
for &r in &[3u8,12,13,14,15] {        // push callee-save
    if r>=8 { sink.put1(0x41); }
    sink.put1(0x50|(r&7));
}
if fs > 0 {
    if fs < 128 {
        sink.put1(0x48); sink.put1(0x83); sink.put1(0xEC); sink.put1(fs as u8);
    } else {
        sink.put1(0x48); sink.put1(0x81); sink.put1(0xEC); sink.put4(fs);
    }
}
"""

[emit.epilogue]
template = """
if fs > 0 {
    if fs < 128 {
        sink.put1(0x48); sink.put1(0x83); sink.put1(0xC4); sink.put1(fs as u8);
    } else {
        sink.put1(0x48); sink.put1(0x81); sink.put1(0xC4); sink.put4(fs);
    }
}
for &r in &[15u8,14,13,12,3] {        // pop callee-save
    if r>=8 { sink.put1(0x41); }
    sink.put1(0x58|(r&7));
}
sink.put1(0x5D);                       // pop rbp
sink.put1(0xC3);                       // ret
"""
```

可用上下文：

| 变量 | 类型 | 说明 |
| -------- | ------ | ------ |
| `sink` | `&mut CodeSink` | 代码输出缓冲区 |
| `rm` | `&RegMap` | 寄存器映射 |
| `fs` | `u32` | 帧大小（栈分配字节数） |

---

## 代码生成器输出

`isa!` / `isa_from_file!` 宏展开为一个 Rust 模块，包含：

### 1. `Reg` 枚举

```rust
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Reg {
    RAX, RCX, RDX, RBX, RSP, RBP, RSI, RDI,
    R8, R9, R10, R11, R12, R13, R14, R15,
    XMM0, XMM1, XMM2, XMM3, XMM4, XMM5, XMM6, XMM7,
    XMM8, XMM9, XMM10, XMM11, XMM12, XMM13, XMM14, XMM15,
}

impl PhysReg for Reg { /* ... */ }
```

### 2. `Inst` 枚举

每个 `[inst.NAME]` 生成一个 PascalCase 变体：

```rust
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Inst {
    MovR8Rm { dest: VReg, src: VReg },
    AddRm8R8 { dest: VReg, src: VReg },
    MovRegImm64 { reg: VReg, imm: i64 },
    Ret,
    JmpRel32 { rel: i64 },
    JccRel32 { cond: u8, rel: i64 },
    // ... (每个 [inst.*] 一条)
    Unknown(VReg),  // 用于 NOP/占位
}
```

**Unknown 变体**：当 lowering 规则中某条指令的参数集不完整时，生成 `Unknown(vreg)` 占位。

### 3. `MachineInst` trait 实现

```rust
impl MachineInst for Inst {
    fn uses(&self) -> SmallVec<[VReg; 4]>;
    fn defs(&self) -> SmallVec<[VReg; 4]>;
    fn effects(&self) -> SmallVec<[EffectKind; 2]>;     // 从 [effect] 生成
    fn is_branch(&self) -> bool;                         // effect 含 Branch/Jump
    fn branch_targets(&self) -> SmallVec<[BlockId; 2]>;  // BlockTarget 字段
    fn is_call(&self) -> bool;                           // effect 含 Call
    fn is_ret(&self) -> bool;                            // effect 含 Ret
    fn is_move(&self) -> bool;
    fn has_side_effects(&self) -> bool;                  // effect 非空且非全 Pure
}
```

### 4. `Isa` 结构体 + `InstructionSet` trait 实现

```rust
pub struct Isa;

impl InstructionSet for Isa {
    type Inst = Inst;
    type Reg = Reg;

    // ABI 方法
    fn sp_reg() -> FrameAccess<Reg>;
    fn fp_reg() -> FrameAccess<Reg>;
    fn stack_align() -> u32;
    fn callee_save_regs() -> Vec<Reg>;
    fn arg_regs() -> Vec<Reg>;
    fn ret_regs() -> Vec<Reg>;
    fn precolored_vregs() -> HashMap<VReg, u8>;
    fn num_gp_regs() -> u8;
    fn num_fp_regs() -> u8;

    // 代码发射
    fn emit(inst: &Inst, rm: &RegMap, sink: &mut CodeSink) -> Result<(), CompileError>;
    fn emit_prologue(frame_size: u32, rm: &RegMap, sink: &mut CodeSink);
    fn emit_epilogue(frame_size: u32, rm: &RegMap, sink: &mut CodeSink);

    // Lowering
    fn lower(op: &Opcode, args: &[VReg], result: Option<VReg>, ctx: &mut LowerCtx)
        -> Result<Vec<Inst>, CompileError>;
    fn lower_terminator(term: &Terminator, /* ... */)
        -> Result<Vec<Inst>, CompileError>;

    // 窥孔优化
    fn peephole_optimize(insts: &mut Vec<Inst>) -> usize;
}
```

### 5. `IsaInfo` trait 实现

```rust
impl IsaInfo for Isa {
    fn name() -> &'static str;           // meta.name
    fn version() -> &'static str;         // meta.version
    fn address_size() -> u8;              // meta.mode
    fn capabilities() -> IsaCapabilities; // meta.capabilities
    fn formats() -> &'static [FormatInfo];
    fn register_classes() -> &'static [RegisterClassInfo];
    fn num_instructions() -> usize;
}
```

---

## 标准指令集与默认 Lowering

ISA-DSL v10 内置了一套**最小完备标准指令集**（前缀 `SD_`），共 37 条架构无关指令。
用户只需为这些标准指令提供 `[inst.SD_*]` 定义（emit template），代码生成器会**自动合成**
所有 IR Opcode 的 lowering 规则——无需手动编写 `[lower.*]`。

### 工作原理

1. 用户在 TOML 中定义 `[inst.SD_XXX]` 指令（emit template）
2. 若用户未提供 `[lower.Opcode]`，代码生成器使用默认 lowering（由 `[inst.SD_*]` 组合而成）
3. 若用户提供了 `[lower.Opcode]`，优先使用用户规则
4. 若 `no_default_lowering = true`（如 x86_64_v10.toml），完全禁用默认 lowering

### 标准指令全集

#### 整数核心 (17条)

| 指令名 | 字段 | 效果 |
| -------- | ------ | ------ |
| `SD_MOV` | `dest: VReg, src: VReg` | 寄存器拷贝 |
| `SD_MOV_IMM` | `dest: VReg, imm: I64` | 加载立即数 |
| `SD_ADD` | `dest: VReg, src: VReg` | `dest += src` |
| `SD_SUB` | `dest: VReg, src: VReg` | `dest -= src` |
| `SD_MUL` | `dest: VReg, src: VReg` | `dest *= src` |
| `SD_UDIV` | `dest: VReg, src: VReg` | 无符号除 |
| `SD_SDIV` | `dest: VReg, src: VReg` | 有符号除 |
| `SD_UREM` | `dest: VReg, src: VReg` | 无符号取余 |
| `SD_SREM` | `dest: VReg, src: VReg` | 有符号取余 |
| `SD_AND` | `dest: VReg, src: VReg` | 按位与 |
| `SD_OR` | `dest: VReg, src: VReg` | 按位或 |
| `SD_XOR` | `dest: VReg, src: VReg` | 按位异或 |
| `SD_NOT` | `dest: VReg` | 按位取反 |
| `SD_NEG` | `dest: VReg` | 取负 |
| `SD_SHL` | `dest: VReg, src: VReg` | 左移 |
| `SD_SHR` | `dest: VReg, src: VReg` | 逻辑右移 |
| `SD_SAR` | `dest: VReg, src: VReg` | 算术右移 |
| `SD_CMP` | `src1: VReg, src2: VReg` | 比较(设标志位) |
| `SD_SETCC` | `dest: VReg, cond: CondCode` | 条件置位 |

#### 浮点核心 (11条)

| 指令名 | 字段 | 效果 |
| -------- | ------ | ------ |
| `SD_FMOV` | `dest: VReg, src: VReg` | 浮点拷贝 |
| `SD_FMOV_BITS` | `dest: VReg, src: VReg` | 位模式 GPR→FPR |
| `SD_FADD` | `dest: VReg, src: VReg` | 浮点加 |
| `SD_FSUB` | `dest: VReg, src: VReg` | 浮点减 |
| `SD_FMUL` | `dest: VReg, src: VReg` | 浮点乘 |
| `SD_FDIV` | `dest: VReg, src: VReg` | 浮点除 |
| `SD_FSQRT` | `dest: VReg, src: VReg` | 平方根 |
| `SD_FNEG` | `dest: VReg` | 浮点取负 |
| `SD_FABS` | `dest: VReg` | 浮点绝对值 |
| `SD_FCMP` | `src1: VReg, src2: VReg` | 浮点比较 |
| `SD_I2F` | `dest: VReg, src: VReg` | 整数→浮点 |
| `SD_F2I` | `dest: VReg, src: VReg` | 浮点→整数 |

#### 内存与控制流 (9条)

| 指令名 | 字段 | 效果 |
| -------- | ------ | ------ |
| `SD_LOAD` | `dest: VReg, src: VReg` | 从地址加载 |
| `SD_STORE` | `addr: VReg, val: VReg` | 存储到地址 |
| `SD_JMP` | `rel: BlockTarget` | 无条件跳转 |
| `SD_JCC` | `cond: CondCode, rel: BlockTarget` | 条件跳转 |
| `SD_CALL` | `target: VReg` | 间接调用 |
| `SD_RET` | (无字段) | 函数返回 |
| `SD_NOP` | (无字段) | 空操作 |
| `SD_UD2` | (无字段) | 陷入(不可达/不支持) |
| `SD_PUSH` | `reg: VReg` | 压栈 |
| `SD_POP` | `reg: VReg` | 弹栈 |
| `SD_STACK_ADDR` | `dest: VReg, offset: I64` | 栈地址计算 |

#### 扩展指令

以下指令在标准集中定义但不被默认 lowering 引用（用户可按需实现）：

| 指令名 | 字段 | 效果 |
| -------- | ------ | ------ |
| `SD_SEXT` | `dest: VReg, src: VReg` | 有符号扩展 |
| `SD_UREM` | `dest: VReg, src: VReg` | 无符号取余（硬件指令，默认 lowering 不使用） |
| `SD_SREM` | `dest: VReg, src: VReg` | 有符号取余（硬件指令，默认 lowering 不使用） |
| `SD_PUSH` | `reg: VReg` | 压栈 |
| `SD_POP` | `reg: VReg` | 弹栈 |
| `SD_I2F` | `dest: VReg, src: VReg` | 整数→浮点 |
| `SD_F2I` | `dest: VReg, src: VReg` | 浮点→整数 |
| `SD_STACK_ADDR` | `dest: VReg, offset: I64` | 栈地址计算 |

> **Urem/Srem 默认 lowering**：当前不使用 `SD_UREM`/`SD_SREM`，而是用
> `SD_UDIV + SD_MUL + SD_SUB` 五指令序列展开：
> `remainder = dividend - (dividend / divisor) * divisor`。
> 用户若想获得高效的取余实现，可编写自定义 `[lower.Urem]` / `[lower.Srem]` 规则。
> **Switch 默认 lowering**：对每个 case 生成 `SD_MOV_IMM → SD_CMP → SD_JCC Equal` 的 if-else 链，
> 最后以 `SD_JMP default_block` 收尾。所需标准指令：`SD_MOV_IMM`, `SD_CMP`, `SD_JCC`, `SD_JMP`。

### CondCode 约定

`SD_SETCC` 和 `SD_JCC` 的 `cond` 字段使用标准序数值：

**整数条件 (IntCC):**
0=Equal, 1=NotEqual, 2=SignedLessThan, 3=SignedLessThanOrEqual,
4=SignedGreaterThan, 5=SignedGreaterThanOrEqual, 6=UnsignedLessThan,
7=UnsignedLessThanOrEqual, 8=UnsignedGreaterThan, 9=UnsignedGreaterThanOrEqual

**浮点条件 (FloatCC):**
0=Equal, 1=NotEqual, 2=LessThan, 3=LessThanOrEqual,
4=GreaterThan, 5=GreaterThanOrEqual, 6=Unordered, 7=Ordered

### 预留 Scratch VReg

默认 lowering 使用以下预留虚拟寄存器：

| VReg | 用途 |
| ------ | ------ |
| VReg(96) | GPR 临时 (Fconst bitcast, Urem/Srem 展开中间值) |
| VReg(97) | GPR 临时 (分支零寄存器, Switch case 立即数) |
| VReg(98) | 预留 |
| VReg(99) | 栈指针代理 (StackAddr, Alloca) |
| VReg(100) | XMM0 浮点预着色 (float return) |

### 最小示例

以下 TOML 展示仅用标准指令构建的完整 ISA：

```toml
[meta]
name = "my-risc-v"
version = "1.0"
mode = 64

[reg.gpr]
count = 32
width = 64

[abi]
[abi.frame]
sp = "X2"

# === 只需定义标准指令的 emit template ===
[inst.SD_MOV]
fields = [{name = "dest", type = "VReg"}, {name = "src", type = "VReg"}]
[inst.SD_MOV.emit]
template = "enc_rr_simple(sink, 0x13, preg(*dest, rm)?, preg(*src, rm)?);"
[inst.SD_MOV.effect]
effect = ["Pure"]

[inst.SD_ADD]
fields = [{name = "dest", type = "VReg"}, {name = "src", type = "VReg"}]
[inst.SD_ADD.emit]
template = "enc_rr_simple(sink, 0x33, preg(*dest, rm)?, preg(*src, rm)?);"
[inst.SD_ADD.effect]
effect = ["Pure"]

[inst.SD_RET]
fields = []
[inst.SD_RET.emit]
template = "sink.put1(0x00);"
[inst.SD_RET.effect]
effect = ["Ret"]

[inst.SD_NOP]
fields = []
[inst.SD_NOP.emit]
template = "sink.put1(0x00);"
[inst.SD_NOP.effect]
effect = ["Pure"]

# ... 其他 SD_* 指令 ...

# 可选：覆盖特定 opcode
[lower.Udiv]
insts = [{inst = "MY_DIV64", args = {dest = "rd", src1 = "rs1", src2 = "rs2"}}]
```

仅需定义 emit template，即可自动获得所有 IR Opcode 的 lowering 支持。

### 禁用默认 Lowering

如果 ISA 定义已完整覆盖所有 lowering 规则（如 x86_64_v10.toml），可以禁用默认行为：

```toml
[meta]
name = "x86_64"
no_default_lowering = true
```

此时仅有用户显式定义的 `[lower.*]` 生效，未定义 opcode 返回 `CompileError::Unsupported`。

### 默认 Lowering 覆盖范围

| IR Opcode 类别 | 默认 Lowering | 备注 |
| --------------- | -------------- | ------ |
| 整数算术 (Iadd, Isub, ...) | SD_MOV + SD_ADD 等 | 全部支持 |
| 位运算 (Band, Bor, ...) | SD_MOV + SD_AND 等 | 全部支持 |
| 浮点算术 (Fadd, Fsub, ...) | SD_FMOV + SD_FADD 等 | 全部支持 |
| 比较 (Icmp, Fcmp) | SD_XOR + SD_CMP/SD_FCMP + SD_SETCC | 全部条件 |
| 类型转换 | SD_MOV / SD_SEXT | 全部支持 |
| 常量 (Iconst, Fconst) | SD_MOV_IMM / SD_FMOV_BITS | 全部支持 |
| 内存 (Load, Store) | SD_LOAD / SD_STORE | 全部支持 |
| 控制流 (Jump, Branch) | SD_JMP / SD_CMP + SD_JCC | 全部支持 |
| 调用 (Call, CallIndirect) | SD_CALL / SD_UD2 | Call 需架构特化 |
| 取余 (Urem, Srem) | SD_UDIV + SD_MUL + SD_SUB 展开 | 不需 SD_UREM/SD_SREM |
| SIMD (Vadd, Vsub, Vmul) | SD_MOV + SD_ADD/SUB/MUL 标量 fallback | 用户可覆盖自定义 SIMD lowering |
| SIMD (Vextract, Vinsert) | SD_MOV | 标量拷贝 |
| 终结指令 (Jump, Branch) | SD_JMP / SD_CMP + SD_JCC | 全部支持 |
| 终结指令 (Switch) | SD_MOV_IMM + SD_CMP + SD_JCC if-else 链 | 动态展开 |
| 终结指令 (Return, Unreachable) | SD_MOV + SD_RET / SD_UD2 | 全部支持 |

---

### `[meta]` 字段

| 字段 | 类型 | 默认值 | 必需 |
| ------ | ------ | -------- | ------ |
| `name` | String | — | ✅ |
| `version` | String | `""` | ❌ |
| `endian` | String | `"little"` | ❌ |
| `mode` | u8 | `64` | ❌ |
| `max_inst_len` | u8 | `0` | ❌ |
| `capabilities.variable_length` | bool | `false` | ❌ |
| `capabilities.prefix_layers` | u8 | `0` | ❌ |
| `capabilities.simd_widths` | u16[] | `[]` | ❌ |
| `capabilities.mask_registers` | bool | `false` | ❌ |
| `capabilities.broadcast` | bool | `false` | ❌ |
| `capabilities.rounding_mode` | bool | `false` | ❌ |

### `[reg.<group>]` 字段

| 字段 | 类型 | 默认值 | 必需 |
| ------ | ------ | -------- | ------ |
| `count` | u16 | — | ✅ |
| `width` | u16 | `64` | ❌ |
| `names` | String[] | (自动生成) | ❌ |
| `prefix` | String | — | ❌ |

### `[abi]` 字段

| 字段 | 类型 | 默认值 | 必需 |
| ------ | ------ | -------- | ------ |
| `stack_align` | u32 | `16` | ❌ |
| `frame.sp` | String | — | 部分 |
| `frame.fp` | String | — | ❌ |
| `callee_saved.gpr` | String[] | `[]` | ❌ |
| `callee_saved.xmm` | String[] | `[]` | ❌ |
| `arg_regs.gpr` | String[] | `[]` | ❌ |
| `arg_regs.xmm` | String[] | `[]` | ❌ |
| `ret_regs.gpr` | String[] | `[]` | ❌ |
| `ret_regs.xmm` | String[] | `[]` | ❌ |
| `precolor.<VRegName>` | String | — | ❌ |

### `[inst.<NAME>]` 字段

| 字段 | 类型 | 默认值 | 必需 |
| ------ | ------ | -------- | ------ |
| `fields` | InstField[] | `[]` | ❌ |
| `fields[].name` | String | — | ✅ |
| `fields[].type` | FieldType | — | ✅ |
| `emit.template` | String | — | ❌ |
| `effect` | String[] | — | ❌ |

### FieldType 值

```text
VReg | I64 | U8 | U32 | F64 | BlockTarget | Reg | CondCode
```

### Effect 值

```text
Pure | Read | Write | Branch | Jump | Trap | Ret | Call
```

### `[lower.<Opcode>]` / `[lower_term.<Term>]` 字段

| 字段 | 类型 | 默认值 | 必需 |
| ------ | ------ | -------- | ------ |
| `insts` | LowerInst[] | — | ✅ |
| `insts[].inst` | String | — | ✅ |
| `insts[].args` | Map<String, String> | `{}` | ❌ |

### `[emit]` 字段

| 字段 | 类型 | 默认值 | 必需 |
| ------ | ------ | -------- | ------ |
| `prologue.template` | String | — | ❌ |
| `epilogue.template` | String | — | ❌ |

### args 特殊值

| 值 | 上下文 | 含义 |
| -------- | ------ | ------ |
| `rd` | lower | 结果 VReg |
| `rs1` | lower | 第一个操作数 VReg |
| `rs2` | lower | 第二个操作数 VReg |
| `rs3` | lower | 第三个操作数 VReg |
| `iconst` | lower (Iconst) | 常量池整数值 |
| `fconst` | lower (Fconst) | 常量池浮点位模式 |
| `VReg(N)` | lower | 固定虚拟寄存器 |
| `val` | lower_term (Return) | 返回值 |
| `cond` | lower_term (Branch) | 条件 VReg |
| `target` | lower_term (Jump) | 跳转目标 |
| `true_block` | lower_term (Branch) | 真分支目标 |
| `false_block` | lower_term (Branch) | 假分支目标 |
| `"..."` | 任意 | 字面量字符串 |

---

## 完整的 x86-64 示例

完整可工作的 x86-64 ISA 定义参见 [`examples/isa/x86_64_v10.toml`](../examples/isa/x86_64_v10.toml)（约 490 行），包含：

- 16 个 GPR + 16 个 XMM 寄存器
- System V AMD64 ABI 声明
- 41 条机器指令（MOV、算术、位运算、移位、比较、控制流、SSE2）
- 完整的 IR Opcode lowering 规则（整数、浮点、比较、转换、常量）
- 函数序言/尾声（含 callee-save push/pop）

### 在代码中使用

```rust
use codegen_dsl::isa_from_file;
isa_from_file!("examples/isa/x86_64_v10.toml");

// 生成的模块：self::x86_64
pub type X86Isa  = self::x86_64::Isa;
pub type X86Inst = self::x86_64::Inst;
pub type X86Reg  = self::x86_64::Reg;

// 使用编译管线
let compiled = FunctionCompiler::<X86Isa>::compile_raw(&ir_function)?;
```
