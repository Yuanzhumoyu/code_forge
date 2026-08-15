# ISA-DSL v11 — 语法规范与使用方法

> 本文档对应 `forge-dsl`（crate 名 `forge_dsl`，proc-macro）的 **ISA-DSL v11** 版本，
> 与仓库当前代码逐项核对（`crates/frontend/forge-dsl/src/`、`isa/x86_v10.toml`）。
> 旧文档 `docs/isa-dsl-v10.md` 已过时（`codegen_dsl` 改名、`emit.template` 删除、
> `InstructionSet`/`Isa` 组件体系重构等），不再适用。

## 目录

1. [概述](#概述)
2. [快速开始](#快速开始)
3. [TOML 结构总览](#toml-结构总览)
4. [`[meta]` — 元信息](#meta--元信息)
5. [`[reg.*]` — 寄存器组](#reg--寄存器组)
6. [`[abi]` — ABI 声明](#abi--abi-声明)
7. [`[reg_classes.*]` / `[spill.*]` / `[dyn.*]`](#reg_classes--spill--dyn)
8. [`[inst.*]` — 指令定义](#inst--指令定义)
9. [encoding 字符串语法](#encoding-字符串语法)
10. [编码原语完整清单](#编码原语完整清单)
11. [`[enc_macros.*]` / `[enc_scatters.*]` / `[enc_constants.*]` / `[cc_names]`](#enc_macros--enc_scatters--enc_constants--cc_names)
12. [`[lower.*]` / `[lower_term.*]` / `[lower_pattern.*]` — IR Lowering 规则](#lower--lower_term--lower_pattern--ir-lowering-规则)
13. [`[emit.*]` — 序言/尾声](#emit--序言尾声)
14. [`[lang.*]` — 汇编语法扩展（可选）](#lang--汇编语法扩展可选)
15. [代码生成输出（组件体系）](#代码生成输出组件体系)
16. [标准指令集与默认 Lowering](#标准指令集与默认-lowering)
17. [x86_v10.toml 实例解析](#x86_v10toml-实例解析)
18. [附录：TOML 字段速查表](#附录toml-字段速查表)

---

## 概述

**ISA-DSL v11** 是一个基于 TOML 的指令集架构（ISA）定义系统，由 Rust proc-macro
`forge_dsl::isa!` / `forge_dsl::isa_from_file!` 在**编译期**把 TOML 编译成完整的
Rust ISA 模块。v11 生成的不是旧版的 `InstructionSet` trait 实现，而是一套
**组件化的 `TargetMachine`**：

- `TargetMachine` — 顶层后端入口（组合所有组件）
- `IsaInfo` / `RegInfo` / `ABI` — ISA 元信息、寄存器堆、调用约定
- `Lowering` — IR Opcode → 机器指令（`InstPacket`）
- `Encoder` / `FrameLowering` — 指令编码与函数序言/尾声/溢栈（spill）
- `Disassembler` / `Assembler` — 每 ISA 专属汇编器/反汇编器
- `Reg` / `Inst` 枚举 + `MachineInst` trait 实现

**核心原则**：在 TOML 中声明 ISA 语义（寄存器、指令编码、lowering 规则），
代码生成器输出类型安全的 Rust 代码。所有示例代码均取自仓库真实文件
（`isa/x86_v10.toml`、`isa/minimal_sd.toml`、`crates/backend/forge-codegen/src/arch/*.rs`）。

---

## 快速开始

### 入口宏

v11 提供两个 proc-macro（注意 crate 名是 **`forge_dsl`**，不是旧文档的 `codegen_dsl`）：

```rust
// 方式 1：内联 TOML 文本（TOML 直接写在宏调用里，不是字符串字面量）
use forge_dsl::isa;
isa! {
    [meta]
    name = "my_arch"
    // ... 完整 TOML ...
}

// 方式 2：从文件加载（推荐，仓库所有后端都用这种方式）
forge_dsl::isa_from_file!("isa/x86_v10.toml");
```

**文件查找顺序**（`crates/frontend/forge-dsl/src/lib.rs`）：

1. 先按**原路径**（相对调用方 crate 的工作目录）读；
2. 读不到 → `CARGO_MANIFEST_DIR/<path>`；
3. 还读不到 → `CARGO_MANIFEST_DIR/../../.. /<path>`（上溯 3 级，适配
   `crates/backend/forge-codegen` 位于 workspace 根目录下 3 层的布局，
   使 `isa/x86_v10.toml` 从 forge-codegen 直接可达）。

宏生成一个与 `meta.name` 同名（**小写**、`-` → `_`）的 `pub mod`，模块内
`use crate::prelude::*;`——因此 `isa_from_file!` 必须从 **forge-codegen 内部**
调用（`crate::prelude::*`、`crate::machine::*`、`crate::assembler::*` 才能解析），
集成测试不能内联使用，需从 forge-codegen 的 backend 模块导入。

仓库真实用法（`crates/backend/forge-codegen/src/arch/x86_64.rs`）：

```rust
//! x86_64 ISA — v19 DSL-generated TargetMachine.
forge_dsl::isa_from_file!("isa/x86_v10.toml");
pub use self::x86_64::*;

// 使用编译管线
let compiler = FunctionCompiler::new(TargetMachine::new());
let compiled = compiler.compile_raw(&func).expect("compile");
```

> 调试：设置环境变量 `FGE_DEBUG_GEN=1` 时，宏会把展开后的生成代码写入
> `%TEMP%/forge_gen_<isa文件名>.rs`（按 ISA 文件名区分）。

### 编译管线

`isa_from_file!` 内部依次执行（`lib.rs::compile_source`）：

```text
parse（serde 反序列化） → validate（模型校验） → expand_opcodes（opcode 表展开）
→ expand_variants（重载组展开） → expand_templates（$CC 模板展开）
→ validate_lowering（lowering 指令名校验） → codegen（生成 Rust 代码）
```

### 最小完整示例

仓库 `isa/minimal_sd.toml` 是**真实可编译**的最小示例（利用标准指令集 + 默认
lowering，只需为每条 `SD_*` 指令提供 encoding/asm/effect，无需写任何 `[lower.*]`）。
节选：

```toml
[meta]
name = "minimal_sd"
version = "11.0"
endian = "little"
mode = 64
max_inst_len = 1

[meta.capabilities]
variable_length = false

[reg.gpr]
count = 8
width = 64
prefix = "R"                # 自动生成 R0..R7

[abi]
stack_align = 16
[abi.frame]
sp = "R7"
[abi.arg_regs]
gpr = ["R0", "R1", "R2", "R3"]
[abi.ret_regs]
gpr = ["R0"]

# 只需定义标准指令的 encoding/asm/effect，lowering 自动合成
[inst.SD_MOV]
fields = { dest = "Ireg", src = "Ireg" }
encoding = "{0x10:[0;8]}"
asm = "mov {dest}, {src}"
effect = "Pure"

[inst.SD_ADD]
fields = { dest = "Ireg", src = "Ireg" }
encoding = "{0x20:[0;8]}"
asm = "add {dest}, {src}"
effect = "Pure"

# ... 其余 SD_*（见 §16 完整清单）...
```

---

## TOML 结构总览

v11 的 ISA TOML 顶层 section 全集（`model.rs::IsaModel` 字段）：

| Section | 必需 | 形态 | 说明 |
| ------- | ---- | ---- | ---- |
| `[meta]` | ✅ | 单表 | ISA 名称、版本、端序、位宽、能力开关 |
| `[meta.capabilities]` | ❌ | 子表 | 变长/前缀层级/SIMD 宽度等能力元数据 |
| `[reg.<group>]` | ✅（至少 `gpr64` 或 `gpr`） | 多表 | 寄存器组（gpr64/gpr32/xmm 等） |
| `[abi]` | ❌ | 嵌套表 | 调用约定、帧布局、call lowering 配置 |
| `[inst.<NAME>]` | ❌ | 多表 | 机器指令（fields/encoding/asm/effect） |
| `[lower.<Opcode>]` | ❌ | 多表 | IR Opcode → 指令序列 |
| `[lower_term.<Term>]` | ❌ | 多表 | 终止指令（Return/Jump/Branch）lowering |
| `[lower_pattern.<Name>]` | ❌ | 多表 | 模式融合规则（当前为死配置，见下） |
| `[emit.prologue]` / `[emit.epilogue]` | ❌ | 子表 | 序言/尾声指令序列 |
| `[enc_macros.<NAME>]` | ❌ | 多表 | 用户编码宏（`$name arg...` 调用） |
| `[enc_scatters.<NAME>]` | ❌ | 多表 | 位域散点（`{field:name}` 调用） |
| `[enc_constants]` | ❌ | 单表 | 编码常量（`$NAME` 引用） |
| `[dyn.<NAME>]` | ❌ | 多表 | 动态类型维度（如 opsize ∈ {8,16,32,64}） |
| `[reg_classes.<NAME>]` | ❌ | 多表 | 宽度感知的分配寄存器类 |
| `[spill.<CLASS>]` | ❌ | 多表 | 溢栈 load/store 指令模板 |
| `[cc_names]` | ❌ | 单表 | 条件码 → 汇编后缀映射 |
| `[lang.tokens.*]` | ❌ | 多表 | 自定义汇编 token（正则） |
| `[lang.keywords]` | ❌ | 单表 | 自定义关键字 |
| `[lang.rules]` | ❌ | 单表 | 自定义语法规则（EBNF 表达式） |

> `[lower_pattern.*]`：模型仍接受该 section（`lower_pattern` 字段存在并参与校验），
> 但 IR 层模式融合目前由 forge-codegen 手写的 `standard_matcher`
> （`crates/backend/forge-codegen/src/ext/pattern_isel.rs`）提供，经
> `TargetMachine::pattern_matcher()` 暴露（由 `[meta].enable_pattern_isel` 门控），
> TOML 中的 `[lower_pattern.*]` 为死配置（x86_v10.toml 已删除并注释说明）。

> **命名约定**：指令名使用 `SCREAMING_SNAKE_CASE`（如 `MOV_R_RM`），生成的
> Rust 枚举变体自动转 PascalCase（`MovRRm`）。模块名 = `meta.name` 小写。

---

## `[meta]` — 元信息

### 全部字段

```toml
[meta]
name = "x86_64"               # (必需) ISA 名称 → Rust 模块名（小写化）
version = "11.0"              # (默认 "") 版本字符串 → IsaInfo::version()
endian = "little"             # (默认 "little") 端序，"big" 亦可
mode = 64                     # (默认 64) 地址位宽 → IsaInfo::address_size()
max_inst_len = 15             # (默认 0) 最大指令长度（字节），0 = 定长
no_default_lowering = true    # (默认 false) 完全禁用 SD_* 默认 lowering
no_epilogue_label = false     # (默认 false) 不生成 epilogue 标签
gpr_bank_order = ["gpr64", "gpr32", "gpr16", "gpr8l", "gpr8h"]
                              # (默认 []) GPR 宽度视图组的规范顺序（决定 Reg 枚举
                              # 变体顺序；空 → 按 [reg.*] 声明顺序）
modrm_force_disp_base = [5, 13]
                              # (默认 []) ModRM/SIB 寻址中必须强制带位移的 base
                              # 物理编号（x86: RBP=5/R13=13，mod=00+rm=101 是
                              # RIP-relative，无法表达 [rbp]/[r13]）
epilogue_jump_opcode = 0xE9   # (默认 无) 收尾跳转 opcode（x86 0xE9 = JMP rel32）；
                              # 缺失时 emit_epilogue_jump 用 trait 默认（Unimplemented）
default_fpr_width = 8         # (默认 8) 默认浮点值宽度（字节）；x86 = f64
enable_pattern_isel = false   # (默认 false) 启用 IR 层模式融合
                              #（TargetMachine::pattern_matcher() 返回标准匹配器）

[meta.capabilities]
variable_length = true        # (默认 false) 变长指令集？
prefix_layers = 4             # (默认 0) 前缀层级（x86:4）
simd_widths = [128, 256]      # (默认 []) SIMD 寄存器宽度（位）
mask_registers = false        # (默认 false) 掩码寄存器（AVX-512）
broadcast = false             # (默认 false) 广播支持
rounding_mode = false         # (默认 false) 舍入模式支持
```

### 生成代码映射

| TOML 字段 | 生成的 Rust |
| --------- | ----------- |
| `name` | 模块名 `pub mod x86_64` |
| `version` | `IsaInfo::version()` |
| `mode` | `IsaInfo::address_size()` |
| `endian` | `IsaInfo::endianness()` |
| `capabilities.*` | `IsaInfo::capabilities()` → `IsaCapabilities` |
| `variable_length` | 是否生成 `modrm`/`sib` 编码辅助函数与 spill 实现 |
| `max_inst_len` | `IsaCapabilities.max_inst_len` |

---

## `[reg.*]` — 寄存器组

### 语法

```toml
[reg.gpr64]                    # 主 GPR 组（64 位视图）
count = 16                     # (必需) 寄存器数量
width = 64                     # (默认 64) 寄存器位宽（bit）
names = ["RAX","RCX",...]      # (可选) 显式寄存器名列表
# prefix = "R"                 # (可选) 前缀 → 自动生成 prefix0..prefix{count-1}
# base_index = 4               # (可选, 默认 0) 组内索引 → 物理编号的偏移
                               # （x86 gpr8h: AH/BH/CH/DH 声明 base_index=4
                               #   → 物理编号 4..7）

[reg.xmm]                      # 浮点/SIMD 组
count = 16
width = 128
names = ["XMM0", ...]
prefix = "XMM"
```

生成规则（与 v10 相同）：

- **有 `names`**：直接枚举；**无 `names` 有 `prefix`**：生成 `prefix0..prefix{count-1}`；
  两者都无：生成 `R0..R{count-1}`。
- 组名含 `xmm`/`float` 的组 → FPR（`RegClass::Float`），其余 → GPR（`RegClass::Int`）。
- `to_index()` 返回**物理编码编号**：GPR 组内索引 + `base_index`；XMM = `16 + n`
  （低 4 位即 ModRM XMM 编码号）。
- `[meta].gpr_bank_order` 控制 GPR 组在 `Reg` 枚举中的排列顺序
  （x86 的 `gpr64 → gpr32 → gpr16 → gpr8l → gpr8h → xmm`），
  同族不同宽度视图共享物理编号（RAX/EAX/AX/AL 都是编号 0）。

生成的 `Reg` 枚举实现 `forge_ir::PhysReg`：

```rust
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
#[repr(u32)]
pub enum Reg { /* 所有组的变体 */ }

impl forge_ir::PhysReg for Reg {
    fn to_index(self) -> u32 { /* 物理编号 */ }
    fn class(self) -> forge_ir::RegClass { /* Int / Float */ }
    fn from_index(idx: u32, cls: forge_ir::RegClass) -> Self { /* ... */ }
}
```

---

## `[abi]` — ABI 声明

### 全部字段

```toml
[abi]
stack_align = 16              # (默认 16) 栈对齐字节数
red_zone = 128                # (可选) 红区字节数（x86 SysV = 128；Windows x64 省略）
frame_padding = 8             # (默认 0) 帧布局额外栈填充（x86 = align/2 = 8：
                              # push rbp + callee-saved 后 rsp%16==8，sub rsp 需
                              # 使 call 前 rsp%16==0——SysV/Windows x64 ABI）
fp_push_bytes = 8             # (默认 8) 帧指针上方 prologue 压栈字节数
                              #（x86 push rbp = 8；aarch64/riscv64 stp = 16；wasm = 0）

[abi.frame]
sp = "RSP"                    # (必需) 栈指针寄存器名
fp = "RBP"                    # (可选) 帧指针寄存器名
alloc_inst = "SUB64_R_IMM32"  # (可选) 帧增长指令（@frame_alloc 用；默认 "SUB64_R_IMM32"）
free_inst = "ADD64_R_IMM32"   # (可选) 帧收缩指令（@frame_free 用；默认 "ADD64_R_IMM32"）
neg_alloc_imm = false         # (可选) alloc 立即数取负（RISC-V ADDI 12 位有符号立即数）

[abi.callee_saved]
gpr = ["RBX", "RDI", "RSI", "R12", "R13", "R14", "R15"]
xmm = []

[abi.arg_regs]
gpr = ["RCX", "RDX", "R8", "R9"]
xmm = ["XMM0", "XMM1", "XMM2", "XMM3"]

[abi.ret_regs]
gpr = ["RAX", "RDX"]
xmm = ["XMM0"]

[abi.precolor]                # 虚拟寄存器预着色（VReg(N) → 物理寄存器）
VReg0 = "RAX"
VReg100 = "XMM0"

[abi.scratch]                 # 命名 scratch 虚拟寄存器（→ VReg 编号）
TMP0 = 10                     # R10
TMP1 = 11                     # R11
```

### `[abi.call]` — 类型化 Call lowering 配置

v11 的 Call lowering 由 DSL 生成**通用骨架**，所有指令名/寄存器名均从 TOML 声明
（`model.rs::AbiCall`）。缺失时回退到模板式 Call lowering（`arg0`–`arg7`）。

```toml
[abi.call]
# 发参侧（caller）
arg_mov = "MOV_R8_RM64"          # 整型参数 → ABI GPR（dest=物理 GPR, src=VReg）
arg_mov_f = "MOVSD_XMM_FREG"     # 浮点参数 → ABI XMM
ret_mov = "MOV_RM8_R64"          # 整型返回值 ← 物理 GPR（dest=VReg, src=物理 GPR）
ret_mov_f = "MOVSD_FREG_XMM"     # 浮点返回值 ← 物理 XMM
call_inst = "CALL_RIP_REL"       # 跨函数调用指令（含重定位）
call_field = "target"            # 该指令承载 FuncRef 编号的字段
shadow_space = 32                # caller 分配的 shadow space（Windows x64 = 32）
stack_slot_size = 8              # (默认 8) 每个栈传参槽字节数
reg_arg_limit = 4                # (默认 4) 前 N 个参数位置走寄存器
stack_alloc_inst = "SUB64_R_IMM32"  # 栈增长（shadow + 栈参数）
stack_free_inst = "ADD64_R_IMM32"   # 栈收缩
stack_addr_inst = "LEA_R64_SIB"     # 栈参数地址计算
stack_store_inst = "STORE_MEM_R"    # 栈参数 store
stack_addr_scratch = "R10"          # 栈参数地址 scratch
sp_reg = "RSP"                      # 栈指针
stack_opsize = 64                   # (默认 64) stack_store_inst 操作数宽度
arg_opsize = 64                     # (默认 64) arg_mov 操作数宽度
# 收参侧（callee，@move_args 用）
entry_fp_reg = "RBP"             # 帧指针基址（栈参数从 [fp+off] 取）
entry_load_scratch = "R11"       # 栈参数整数加载 scratch
entry_stack_load_inst = "MOV_R_MEM"   # 栈参数加载（字段 dest/base/opsize）
entry_sext_inst = "MOVSXD_R_GPR"      # i32 收参符号扩展（字段 dest/src）
entry_mov_inst = "MOV_RM8_R64"        # 整数寄存器参数接收（dest/src/opsize）
entry_gpr_to_fp_inst = "MOVQ_XMM_FREG" # GPR→FP 收参 mov
entry_ret_addr_bytes = 8          # 返回地址占用字节（栈参数偏移计算）
```

---

## `[reg_classes.*]` / `[spill.*]` / `[dyn.*]`

### `[reg_classes.<NAME>]` — 宽度感知分配类

```toml
# 主 GPR 类（8 字节 = 64 位）。allocatable 列出可被普通 VReg 分配的物理编号；
# RSP(4)/RBP(5)/R10(10)/R11(11) 被排除（栈指针/帧指针/scratch）。
[reg_classes.GPR]
width = 8
allocatable = [0,1,2,3,6,7,8,9,12,13,14,15]

# 多宽度类：RegClass 宽度化后（GPR(w) payload = 字节宽），lowering 按类型分派——
# I32 结果/参数分到 GPR(4) 池。GPR(4) 与 GPR(8) 同号是同一物理寄存器（RAX=EAX），
# 分配器按 overlaps() 检测跨宽度类物理重叠。
[reg_classes.GPR32]
width = 4
allocatable = [0,1,2,3,6,7,8,9,12,13,14,15]

[reg_classes.FPR]
width = 16
allocatable = [0,1,2,3,4,5,6,7,8,9,12,13,14,15]

# VEC(32) — 256 位向量（AVX YMM 池）；YMMn 与 XMMn 同号（编码层同编号，
# 由 VEX 前缀 + opcode 区分宽度）。
[reg_classes.VEC32]
width = 32
allocatable = [0,1,2,3,4,5,6,7,8,9,12,13,14,15]
```

类名前缀决定 kind：`FPR*` → `FPR`、`VEC*` → `VEC`、其余 → `GPR`。
生成的 `RegisterClassInfo`（name/count/width/reg_class/allocatable）供分配器按类型
分派；未定义 `GPR`/`FPR` 类时回退到全量可分配。

### `[spill.<CLASS>]` — 溢栈指令模板

```toml
[spill.GPR]
load  = { inst = "MOV64_RM", base = "RBP" }   # 从 [base+offset] 加载到 GPR
store = { inst = "MOV64_MR", base = "RBP" }   # 从 GPR 存储到 [base+offset]

[spill.FPR]
load  = { inst = "MOVSD_RM", base = "RBP" }
store = { inst = "MOVSD_MR", base = "RBP" }
```

只有 **variable-length** ISA 且有完整 `[spill.*]`（GPR + FPR 的 load/store 均定义）
时才生成真实 `emit_spill_load`/`emit_spill_store`/`emit_epilogue_jump` 实现；
否则生成带诊断信息的 `Unimplemented` stub（或 trait 默认）。

### `[dyn.<NAME>]` — 动态类型维度

```toml
# opsize: 操作数宽度，x86-64 支持 8/16/32/64-bit。作为指令字段（类型 "Opsize"），
# 驱动编码宏的条件前缀/REX 发射（opsize=16 → 0x66；64 → REX.W=1）。
[dyn.opsize]
kind = "u8"            # 元素类型：u8/u16/u32/u64/i8/i16/i32/i64
values = [8, 16, 32, 64]
default = 64           # 必须 ∈ values
```

---

## `[inst.*]` — 指令定义

指令是 ISA-DSL 的核心单元。每条指令生成一个 `Inst` 枚举变体 + 编码 + 效果 +
寄存器使用信息。

### 基本语法

```toml
[inst.MOV_R_RM]
fields = { dest = "Ireg", src = "Ireg", opsize = "Opsize" }
encoding = "@modrm opsize 0x8B dest src"
asm = "movrr {dest}, {src}"
effect = "Pure"
```

`encoding` 是**字符串**（v10 的 `[inst.*.emit] template = "Rust 代码"` 已删除），
`asm` 是汇编模板，`effect` 声明副作用。

### `fields` 三种形态

```toml
# 1. 内联表（字段名 → 类型）
[inst.A]
fields = { dest = "Ireg", src = "Freg" }
asm = "a {dest}, {src}"

# 2. 紧凑字符串（类型列表，按 asm 占位符顺序映射字段名）
[inst.B]
fields = "Ireg Freg"
asm = "b {dest}, {src}"

# 3. 省略 —— 字段名从 asm 模板 {name} 占位符推断，类型默认 Ireg
[inst.C]
asm = "c {x}, {y}"        # → fields = [x: Ireg, y: Ireg]
```

- 紧凑字符串的**类型数量必须等于 asm 占位符数量**，否则解析报错（防静默错配）。
- `{mnemonic}` 是 opcodes 模板的保留占位符，不参与字段推断。
- 无占位符的 asm（`ret`/`nop`）推断为空字段集。

### FieldType — 18 种

| FieldType | 生成 Rust 类型 | 用途 |
| --------- | ------------- | ---- |
| `i8` `i16` `i32` `i64` | `i8`..`i64` | 有符号立即数 |
| `u8` `u16` `u32` `u64` | `u8`..`u64` | 无符号立即数 |
| `f32` `f64` | `f32`/`f64` | 浮点立即数 |
| `Ireg` | `Reg` | 整型虚拟寄存器（须绑定 `[reg.gpr64]`/`[reg.gpr]`） |
| `Freg` | `Reg` | 浮点虚拟寄存器（须绑定 `[reg.xmm]`） |
| `GprReg` | `Reg` | 物理 GPR（须在 `[reg.gpr64]`/`[reg.gpr]`.names） |
| `XmmReg` | `Reg` | 物理 XMM（须在 `[reg.xmm]`.names） |
| `MemRef` | `MemRef` | 内存引用 `[base + offset]`（base=物理编号, offset, width） |
| `BlockTarget` | `i64` | 基本块跳转目标（`branch_targets()` 提取） |
| `CondCode` | `u8` | 条件码（SETcc/Jcc 的 opcode 后缀） |
| `Opsize` | `u8` | 操作数宽度（来自 `[dyn.opsize]`，驱动编码宏） |

> **已删除的类型**：v10 的 `VReg`、`Reg` 字段类型不存在了——寄存器操作数统一用
> `Ireg`（虚拟 GPR）/`Freg`（虚拟 XMM）/`GprReg`（物理 GPR）/`XmmReg`（物理 XMM）。

### `effect` — 指令效果

| 效果值 | 含义 | 触发行为 |
| ------ | ---- | -------- |
| `Pure` | 纯计算 | `has_side_effects() = false` |
| `Read` / `Write` | 读写状态 | `has_side_effects() = true` |
| `Branch` / `Jump` | 条件/无条件跳转 | `is_branch() = true` |
| `Trap` | 陷阱 | `effects()` 含 Trap |
| `Ret` | 函数返回 | `is_ret() = true` |
| `Call` | 函数调用 | `is_call() = true` |

**三种 TOML 写法**（`deser_effects` 自动兼容）：

```toml
effect = ["Pure"]                 # 数组（推荐）
effect = "Read+Write"             # "+" 连接的字符串
effect = { effect = ["Pure"] }    # 子表（旧格式兼容）
```

### `role` — 显式寄存器角色（可选）

```toml
fields = { dest = "Ireg", src = "Ireg" }   # 无 role：按名字推断
# dest/reg → def（写入）；src*/base → use（读取）
```

显式 `role = "def" | "use" | "both"` 可覆盖推断。这影响 `MachineInst::uses()/defs()`
与寄存器分配 live-range。

### `implicit` — 隐式破坏的物理寄存器

指令执行时隐式写坏的、不在操作数里的物理寄存器（分配器在该指令点避开）：

```toml
[inst.CQO]
encoding = "{0x48:[0;8]} {0x99:[0;8]}"
asm = "cqo"
implicit = ["RDX"]        # cqo 把 RAX 符号扩展到 RDX

[inst.CALL_RIP_REL]
encoding = "@call_reloc32 target, 0xE8"
asm = "call {target}"
effect = "Call"
implicit = ["RAX", "RCX", "RDX", "RSI", "RDI", "R8", "R9", "R10", "R11"]  # caller-saved
```

### `opcodes` 表 — 同族指令一表生成

当一族指令仅 opcode/mnemonic 不同时，用 `opcodes` 表（每条 `[name, opcode, mnemonic]`
展开为一条指令，`{opcode}` 替换 encoding、`{mnemonic}` 替换 asm；模板本身不生成指令）：

```toml
# SSE packed-single 家族：movaps/addps/subps/... 仅 opcode/mnemonic 不同
[inst.PS_BIN]
fields = "Freg Freg"
encoding = "@sse_ps_rr {opcode} dest src"
asm = "{mnemonic} {dest}, {src}"
opcodes = [
  ["MOVAPS", "0x28", "movaps"],
  ["ADDPS",  "0x58", "addps"],
  ["SUBPS",  "0x5C", "subps"],
  # ...
]
```

展开后生成 `[inst.MOVAPS]`、`[inst.ADDPS]` 等具体指令（fields/effect 继承自模板，
`{mnemonic}` 推断字段被过滤）。展开发生在 `expand_opcodes`（parse 之后）。

### `variants` — 重载组

每条变体用自己的字段类型/encoding（`enc`）/asm/effect，生成 `{parent}_{后缀}` 合成
指令（后缀 = 类型签名，如 `PUSH_Ireg`；可用 `name` 自定义）：

```toml
[inst.PUSH]
asm = "push {reg}"
encoding = "@push_reg reg"
effect = "Write"
variants = [{ reg = "Ireg" }, { reg = "GprReg" }]
# → PUSH（父条目保留）、PUSH_Ireg、PUSH_Gpr

[inst.PSH]
fields = "Ireg Ireg"
asm = "psh {dest}, {src}"
variants = [
  { name = "RR", dest = "Ireg", src = "Ireg",
    enc = "@op_rm 64 0x02 0 dest src",
    asm = "pshrr {dest}, {src}" },   # asm/enc 均可覆盖
]
```

### `MemRef` 字段 — 内存操作数

`MemRef` 字段携带 `base`（物理编号）+ `offset` + `width`。编码原语 `@modrm_mem`
遇到 MemRef 字段时自动展开 base/offset（含 SIB/强制位移选择）：

```toml
[inst.MOV64_RM]
fields = { dest = "GprReg", mem = "MemRef" }
encoding = "@modrm_mem 64 0x8B dest mem"
asm = "mov64rm {dest}, {mem}"
effect = "Read"
```

---

## encoding 字符串语法

`encoding` 字符串有三种顶层形态（`bitstring.rs::parse_encoding`）：

1. **原语调用**：`@name arg1 arg2`（参数可用逗号分隔，如 `@call_reloc32 target, 0xE8`）
2. **定长**：前导数字 = 总位宽，如 `"32 {dest:7:5}{0x33:0:7}"`，可带尾随 `!fixup`
3. **分段/变长**：`"[REX]{...} {...}"` 或 `"?cond {...}"`，段间空白分隔

### 位域 `{...}` 三种写法

```text
{VALUE:OFFSET:WIDTH}            — 基本：值放在位区间 [offset, offset+width)
{VALUE:[OFFSET;WIDTH]}          — 括号形式（同义）
{VALUE:[OFFSET;WIDTH;SHIFT]}    — 括号形式 + 右移 SHIFT 位再放置
```

| 元素 | 含义 | 示例 |
| ---- | ---- | ---- |
| `VALUE` | 字段名 / 十六进制 `0xNN` / 十进制 | `dest`、`0x33`、`3` |
| `OFFSET` | 起始位（0 = LSB） | `7` |
| `WIDTH` | 位数 | `5` |
| `SHIFT` | 可选：放置前右移位数 | `3` |

字段名引用指令字段：Ireg/Freg 字段生成 `field.to_index()`，立即数生成 `*field`。

### 段定界符

| 语法 | 含义 | 示例 |
| ---- | ---- | ---- |
| `?cond {...}` | 内联条件段（条件为真才发射） | `?dest>=8||src>=8 {0x41:[0;8]} ...` |
| `{...} {...}` | 无名段（始终发射） | `{0xE9:[0;8]} !rel4` |
| `!fixup_kind` | 标签 fixup 占位段 | `!rel4` |
| `@name args` | 段内内联原语调用 | `{0x4:[4;4]}... @abs_reloc global` |
| `$NAME` | 编码常量引用（`[enc_constants]`） | `$OPCODE_PREFIX` |

### Fixup 类型

| 语法 | 占位字节 | RelocKind | 用途 |
| ---- | -------- | --------- | ---- |
| `!rel4` | 4 | `REL4` | x86 JMP/JCC rel32 |
| `!isa0` | 4 | `Isa(0)` | AArch64 B/BL（26 位偏移） |
| `!isa1` | 4 | `Isa(1)` | RISC-V B 型分支 |
| `!isa2` | 4 | `Isa(2)` | RISC-V J 型跳转 |

### 示例

```text
# x86 RET（定长分段）
{0xC3:[0;8]}

# x86 JMP rel32（fixup）
{0xE9:[0;8]} !rel4

# x86 JCC rel32（cond 字段 + fixup）
{0x0F:[0;8]} {cond:[0;8]} !rel4

# x86 CALL r/m64（条件 REX 段：target>=8 时发 0x41 前缀）
?target>=8 {0x4:[4;4]}{0:[3;1]}{0:[2;1]}{0:[1;1]}{target:[0;1;3]} {0xFF:[0;8]} {target:[0;3]}{0x2:[3;3]}{3:[6;2]}

# RISC-V ADD（定长 32 位）
32 {dest:7:5}{0:12:3}{src1:15:5}{src2:20:5}{0:25:7}{0x33:0:7}

# 原语
@modrm opsize 0x8B dest src
@modrm_mem 64 0x8B dest mem
@mov_imm64 reg imm
@call_reloc32 target, 0xE8
```

---

## 编码原语完整清单

编码原语定义在 `bitstring.rs` 的 `gen_primitive_emit`（match `name`），共 **31 个**：

| 原语 | 参数 | 说明 |
| ---- | ---- | ---- |
| `@modrm` | `opsize opcode reg rm [escape]` | 宽度感知 ModRM(mod=11b)；16→0x66、64→REX.W=1；escape 插在 opcode 前 |
| `@modrm_mem` | `opsize opcode reg rm|mem [disp] [prefix] [escape]` | ModRM 内存寻址；base=100 自动 SIB；`[meta].modrm_force_disp_base` 强制位移；rm 可为 MemRef 字段 |
| `@op_rm` | `opsize opcode ext rm` | `/digit` 单操作数（reg 固定为 ext） |
| `@modrm_imm32` | `opsize opcode ext rm imm` | 81 /digit + imm32 |
| `@op_rm_imm32` | `opcode ext rm imm` | 固定 64 位 /digit + imm32（帧分配） |
| `@cmovcc` | `opsize cc dest src` | 0F 4{cc} /r 条件传送 |
| `@sse_rr` | `prefix opcode w reg rm` | SSE reg-reg（w = REX.W 位值） |
| `@sse_rr_3a` | `prefix opcode reg rm imm` | SSE 66 0F 3A xx /r ib（ROUNDSD 等） |
| `@sse_rr_38` | `prefix opcode reg rm` | SSE 66 0F 38 xx /r（SSE4.1 PMULLD） |
| `@sse_rr_opsize` | `prefix opcode opsize reg rm` | SSE/GPR 0F xx /r，REX.W/66 按运行时 opsize（LZCNT/TZCNT/POPCNT） |
| `@sse_rr_imm8` | `prefix opcode reg rm imm` | SSE 0F xx /r ib（PSHUFD/SHUFPS） |
| `@sse_rr_w` | `prefix opcode reg rm` | SSE 恒发 REX.W（MOVQ GPR↔XMM） |
| `@sse_ps_rr` | `opcode reg rm` | SSE packed-single（无强制前缀） |
| `@vex_rrvvv` | `map pp w l opcode reg rm vv has_src` | 3 字节 VEX（C4）三操作数 AVX；vvvv=~src1；has_src=0 → vvvv=1111；断言 `avx_available()` |
| `@vex_rrvvv_avx2` | 同上 | 同 VEX 但断言 `avx2_available()`（整数 ymm） |
| `@vex_rrvvv_imm` | `map pp w l opcode reg rm vv has_src imm` | VEX + imm8（VINSERTF128/VEXTRACTF128） |
| `@push_reg` / `@pop_reg` | `reg` | PUSH/POP r64（REX 扩展） |
| `@mov_imm64` | `reg imm` | MOV r64, imm64（REX.W + B8+r + 8 字节） |
| `@setcc` | `dest cond` | SETcc r/m8（0F 90+cc /r） |
| `@lea_rbp_disp` | `dest base disp` | LEA rd, [base+disp]（disp8/disp32） |
| `@lea_sib` | `dest base index scale disp` | LEA rd, [base+index*scale+disp]（SIB + 强制位移） |
| `@lea_rip_rel` | `dest target` | LEA rd, [rip+disp32] + PC-relative reloc（ASLR 安全） |
| `@shift_reg` | `count dest src ext opsize` | 可变计数移位（计数不在 count 寄存器时先 mov 进 CL） |
| `@call_reloc` | `target tpl` | 指令模板字 + 对 "@N" 符号的 REL4 重定位（AArch64 BL 等） |
| `@call_reloc32` | `target opc` | opcode 字节 + rel32 占位 + REL4 重定位（x86 CALL） |
| `@abs_reloc` | `target` | 绝对 8 字节占位 + "G{id}" 符号重定位（已废弃，ASLR 不可靠） |
| `@leb128` | `value` | 无符号 LEB128 |
| `@sleb128` | `value` | 有符号 LEB128 |
| `@leb128_reg` | `prefix reg` | 前缀字节 + 寄存器编号 LEB128 |
| `@bswap_r` | `dest opsize` | BSWAP r/m（0F C8+r；64 位加 REX.W） |

> 校验错误消息会列出可用原语清单（`bitstring.rs` 末尾的 `unknown encoding primitive`）。

---

## `[enc_macros.*]` / `[enc_scatters.*]` / `[enc_constants.*]` / `[cc_names]`

### `[enc_macros.<NAME>]` — 用户编码宏

定义命名编码宏，在 encoding 字符串中以 `$name arg1 arg2` 调用（参数按位置绑定，
`${param}` 在 pattern 中替换）：

```toml
# TOML 定义（x86_v10.toml 未使用，语法如下）
[enc_macros.modrm_rr]
params = ["opsize", "opcode", "reg", "rm"]
pattern = "@modrm ${opsize} ${opcode} ${reg} ${rm}"

# 指令中调用
encoding = "$modrm_rr opsize 0x8B dest src"
```

### `[enc_scatters.<NAME>]` — 位域散点

以 `{field:name}` 形式引用，把指令字段名替换进 pattern 的 `_` 占位符
（展开时 `pattern.replace('_', 字段名)`，通常无 params）：

```toml
[enc_scatters.rr]
pattern = "{_:[0;3]}{src:[3;3]}{3:[6;2]}"

# 指令中调用：{dest:rr} → pattern 中的 _ 替换为 dest
encoding = "{0x48:[0;8]} {0x89:[0;8]} {dest:rr}"
```

### `[enc_constants]` — 编码常量

命名常量，pattern 中以 `$NAME` 引用：

```toml
[enc_constants]
OP_PREFIX_REX = "0x48"

# encoding = "$OP_PREFIX_REX {0x89:[0;8]} ..."
```

### `[cc_names]` — 条件码映射

条件码值（`CondCode` 字段，如 JCC 的 opcode 后缀）→ 汇编助记符后缀。
`asm` 模板中的 `{cond}` 占位符按此映射展开（`set{cond}` → `sete`/`setne`...），
也用于 lowering 校验时的小写 mnemonic 匹配（`sete` 匹配 `set{cond}`）：

```toml
[cc_names]
"0x84" = "e"    # JE  / SETE
"0x85" = "ne"   # JNE / SETNE
"0x8C" = "l"    # JL  / SETL
"0x8E" = "le"   # JLE / SETLE
"0x8F" = "g"    # JG  / SETG
"0x8D" = "ge"   # JGE / SETGE
"0x82" = "b"    # JB  / SETB（无符号小于）
"0x86" = "be"   # JBE
"0x87" = "a"    # JA
"0x83" = "ae"   # JAE
# ... 共 16 个（含 p/np/s/ns/o/no）
```

---

## `[lower.*]` / `[lower_term.*]` / `[lower_pattern.*]` — IR Lowering 规则

v11 的 lowering 规则把每条 IR Opcode 映射为**汇编指令字符串序列**（不是 v10 的
`{inst, args}` 对象数组）。每条字符串形如 `"MNEMONIC op1, op2, ..."`。

- **大写**助记符（如 `UD2`、`LEA_R64_SIB`）：直接查 `[inst.*]` 键；
- **小写**助记符（如 `mov`、`add`）：匹配某条指令 `asm` 模板的首词；
- `@...` 伪指令、`...:` 标签、`#` 注释、空行：跳过。
- 校验阶段（`validate_lowering`）会验证所有助记符可解析，报错带
  Levenshtein 建议（"Did you mean ..."）。

### 基本语法

```toml
[lower.Iadd]
insts = ["mov rd, rs1", "add rd, rs2"]

[lower.Copy]
insts = ["mov rd, rs1"]

[lower.Load]
insts = ["mov_mem rd, [rs1]"]        # 内存操作数必须带方括号（形状校验）

[lower.Store]
insts = ["mov_sto [rs2], rs1"]       # builder.store(value, addr)：value=rs1, addr=rs2

[lower.Udiv]
insts = ["xor RDX, RDX", "mov RAX, rs1", "div rs2", "mov rd, RAX"]
# 规则里写死的物理寄存器（RAX/RDX）自动推导为 clobbers，分配器在序列内避开
```

### 操作数标识符表

`lowering_arg_expr`（`codegen/mod.rs`）把每个操作数 token 展开为 lower 上下文表达式：

| 标识符 | 含义 |
| ------ | ---- |
| `rd` | 结果 XReg（`results[0]`；缺失时新分配） |
| `r2` | 第二结果 XReg（`results[1]`；溢出操作的 flag 半） |
| `r3` | 第三临时结果（无第三个 IR 结果时新分配，如 UmulOverflow 组合标志） |
| `rs1` / `rs2` / `rs3` | `args[0..2]`（缺失时新分配） |
| `arg0` .. `arg7` | 函数调用参数位置（Call 模板用；越界回退新分配） |
| `func` | 当前 FuncRef 编号（IR Call 的 `Immediate::Func`；`@call_reloc*` 用） |
| `global` | 当前 GlobalId 编号（`@lea_rip_rel`/`@abs_reloc` 用） |
| `iconst` | 常量池整数（`Iconst`；`ctx.constant_pool.resolve_int(index)`） |
| `fconst` | 常量池浮点位模式（`Fconst`） |
| `vconst_lo`/`vconst_hi`/`vconst_lo2`/`vconst_lo_hi`/`vconst_hi_hi`/`vconst_q0`..`vconst_q3` | 向量常量字节还原（32 位元素奇偶分离 / 64 位连续块） |
| `offset` | StackAddr 帧偏移（`ctx.current_offset`） |
| `alloca_offset` | Alloca 帧槽偏移（预扫描分配） |
| `zero` | 零寄存器（`ctx.alloc_zero_vreg()`） |
| `imm0`..`imm3` | 立即数（ExtractValue/InsertValue 字段偏移、ShuffleVector mask 等） |
| `imm0_mul8/16/32/64` | 字段位偏移（imm0 × 字段位宽） |
| `imm0_sub4` | V256 lane 4-7 的组内选择码（lane − 4） |
| `shufps_imm8` / `shufps_imm8_hi` | SHUFPS imm8 编码（Intel 语义；V256 高组 mask[4..8]） |
| `VReg(N)` | 固定虚拟寄存器编号（**过渡兼容，已废弃**） |
| `%name` | GPR 临时（`ctx.alloc_xreg(GPR)`；`%name:fpr` = FPR 临时） |
| `{const 42}` / `{const 3.14}` | 常量池内联字面量（直接嵌入，不走常量池） |
| `[rs1]` / `[RAX+8]` | 内存操作数（Ireg 字段剥括号按寄存器间接；MemRef 字段解析 base/disp） |
| `0x...` / 十进制 | 数值字面量（按字段类型 cast） |
| 物理寄存器名 | `RAX`/`RSP`/`XMM0`/`R10` 等（大小写不敏感，named/prefix 组均可） |
| 其他小写标识符 | 按变量引用（`val`、`val2`、`cond`、`target`、`true_block`、`false_block`） |

### `variants` — 宽度/类型条件化变体

静态序列无法按源宽度选择指令（如 Uextend 按源宽度 movzx/mov），v11 用
`variants` 按**操作数位宽/元素类型**运行时分派：

```toml
[lower.Uextend]
variants = [
  { when = "rs1<=8",  insts = ["movzx_b rd, rs1"] },
  { when = "rs1<=16", insts = ["movzx_w rd, rs1"] },
  { when = "rs1==32", insts = ["mov rd, rs1"] },
  { when = "rs1==64", insts = ["mov rd, rs1"] },
]

[lower.Bitcast]
variants = [
  { when = "rs1elem==I64 && elem==F64", insts = ["movq_to_xmm rd, rs1"] },
  { when = "rs1elem==F64 && elem==I64", insts = ["movq_to_gpr rd, rs1"] },
  # ...
]
insts = ["mov rd, rs1"]    # 变体全不匹配时的回退（默认 insts 非空时执行）
```

**`when` 谓词**（`parse_when_cond`，支持 `&&`/`||` 复合与括号）：

| 谓词 | 含义 |
| ---- | ---- |
| `rs1<=16` / `rs1==32` / `rs1>=64` / `rs1!=8` ... | rs1 操作数位宽（bit；动态 vector/scalable 按 size_bytes×8） |
| `rd==128` / `rd<=128` / `rd==256` ... | 结果位宽 |
| `elem==F32` / `elem==I64` / `elem==F64` ... | 元素类型（`TypeId`，rd 优先、rs1 次之、rs2 兜底） |
| `rs1elem==I64` ... | rs1 的（元素）类型（区分源/结果方向，如 bitcast） |
| `imm0==0` / `imm0>=4` ... | 立即数数值（`ctx.current_immediates[n]`） |

> 复合条件子条件必须加括号（如 `rd==128 && (elem==F32 || elem==I32)`）——
> 解析器按子条件整体加括号，防止 `&&` 优先级高于 `||` 导致 guard 误真
> （`codegen/mod.rs` 有回归单测）。

### `template` + `conditions` — 条件比较规则一表生成

```toml
[lower.Icmp]
template = ["xor rd, rd", "cmp rs1, rs2", "set$CC rd"]
conditions = [
  ["Equal", "e"], ["NotEqual", "ne"],
  ["SignedLessThan", "l"], ["SignedLessThanOrEqual", "le"],
  ["SignedGreaterThan", "g"], ["SignedGreaterThanOrEqual", "ge"],
  ["UnsignedLessThan", "b"], ["UnsignedLessThanOrEqual", "be"],
  ["UnsignedGreaterThan", "a"], ["UnsignedGreaterThanOrEqual", "ae"],
]
```

- `$CC` 占位符替换为每个 condition 的值；规则名 = `{name}.{cond}`
  （展开为 `[lower.Icmp.Equal]` → `insts = ["xor rd, rd", "cmp rs1, rs2", "sete rd"]`）。
- `conditions` 必须用数组形式（`[["Cond", "value"], ...]`，跨行合规）；
  旧内联表（`{ Equal = "e" }`）仍可解析但不推荐。
- 纯模板规则（insts 空）展开后删除。

### `.if` / `.else` / `.endif` — 条件汇编指令

规则序列内支持条件汇编指令（`cst_codegen.rs`）：

```text
.if rs1_is_float       # 支持：<operand>_is_float / <operand>_is_int
                       #       / is_float_return / has_frame
...
.else
...
.endif
```

### `%name` 临时寄存器

`%name` = GPR 临时、`%name:fpr` = FPR 临时（`%name:gpr` 显式等价）。同名同类型
去重为**一个** `ctx.alloc_xreg(...)`；同名不同类型报错。

```toml
[lower.Fabs]
variants = [{ when = "elem==F32", insts = [
    "movss rd, rs1",
    "mov_imm %t, 0x7FFFFFFF",
    "movq_to_xmm %tf:fpr, %t",
    "andps rd, %tf:fpr",
] }]
insts = [
    "movsd rd, rs1",
    "mov_imm %t, 0x7FFFFFFFFFFFFFFF",
    "movq_to_xmm %tf:fpr, %t",
    "andpd rd, %tf:fpr",
]
```

### `[lower_term.*]` — 终止指令

```toml
[lower_term.Return]
# movr8（0x8B：dest ← src）确保返回值 XReg → RAX 方向正确
insts = ["movr8 RAX, val", "movr8 RDX, val2"]

[lower_term.Jump]
insts = ["jmp target"]

[lower_term.Branch]
insts = ["test cond, cond", "je false_block", "jmp true_block"]

[lower_term.Unreachable]
insts = ["UD2"]
```

特殊操作数：`val`/`val2`（返回值）、`cond`（条件）、`target`（跳转目标）、
`true_block`/`false_block`（分支目标）。

---

## `[emit.*]` — 序言/尾声

v11 的 prologue/epilogue 是**指令字符串序列**（不是 v10 的 Rust `template`），
可混用普通指令与 `@伪指令`：

```toml
[emit.prologue]
# 注意：@move_args 必须在 @push_callee 之后——参数 VReg 可能被分配到
# callee-saved 寄存器（如 R14/R15），先搬参数会覆盖调用者值。
insts = [
    "push RBP",
    "mov64rr RBP, RSP",
    "@push_callee",
    "@move_args",
    "@frame_alloc",
]

[emit.epilogue]
insts = ["mov64rr RSP, RBP", "sub RSP, 56", "@pop_callee", "pop RBP", "ret"]
```

### 伪指令

| 伪指令 | 作用 |
| ------ | ---- |
| `@push_callee` | 按 `[abi.callee_saved].gpr` 顺序压栈 callee-saved |
| `@pop_callee` | 逆序弹栈 callee-saved |
| `@move_args` | 收参：把 ABI 寄存器参数/栈参数搬入参数 VReg（需要完整 `[abi.call].entry_*` 配置，缺失则 `compile_error!`） |
| `@frame_alloc` | 用 `[abi.frame].alloc_inst` 分配帧空间（需 `[abi.frame].sp`） |
| `@frame_free` | 用 `[abi.frame].free_inst` 收缩帧空间 |

生成代码 `emit_prologue_impl(frame_size, rm, sink)` / `emit_epilogue_impl(...)`。
指令字符串里的操作数可以是物理寄存器名、`frame_size`、数值字面量。

---

## `[lang.*]` — 汇编语法扩展（可选）

`[lang.tokens.*]` / `[lang.keywords]` / `[lang.rules]` 用于扩展每 ISA 汇编
语法（CST 代码生成 `build_grammar_from_model`，基于 `forge-grammar` 的
`grammars/base.lx`）：

```toml
[lang.tokens.HEX]                 # 自定义 token（正则）
pattern = "0x[0-9a-fA-F]+"

[lang.keywords]                   # 关键字（作为 IDENT 级高优先级字面量）
"fn" = "fn"

[lang.rules.operand_imm]          # 自定义语法规则（EBNF 表达式字符串）
source = "DEC | HEX"
```

x86_v10.toml 未使用这些 section（可选）。

---

## 代码生成输出（组件体系）

`isa!` / `isa_from_file!` 展开为一个 `pub mod <name>`，内容（`components.rs` v19）：

### 1. `Reg` 枚举

```rust
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
#[repr(u32)]
pub enum Reg { RAX, RCX, ..., XMM0, ... }  // 按 gpr_bank_order 排列

impl forge_ir::PhysReg for Reg { /* to_index / class / from_index */ }
```

### 2. `Inst` 枚举

每个 `[inst.NAME]`（含 opcodes/variants 展开产物）生成一个 PascalCase 变体；
无字段 → 单元变体；有字段 → 结构体变体（字段类型见 §8 的 FieldType 表）。
末尾固定追加 **`Unknown(u32)`**：

```rust
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub enum Inst {
    MovRRm { dest: Reg, src: Reg, opsize: u8 },
    MovRegImm64 { reg: Reg, imm: i64 },
    Ret,
    JmpRel32 { rel: i64 },
    JccRel32 { cond: u8, rel: i64 },
    // ...
    Unknown(u32),
}
```

### 3. `MachineInst` trait 实现（新签名）

```rust
impl crate::prelude::MachineInst for Inst {
    fn uses(&self) -> smallvec::SmallVec<[u32; 4]>;       // use 角色寄存器字段 → 物理编号
    fn defs(&self) -> smallvec::SmallVec<[u32; 2]>;       // def 角色寄存器字段
    fn use_constraints(&self) -> SmallVec<[OperandConstraint; 4]>;
    fn def_constraints(&self) -> SmallVec<[OperandConstraint; 2]>;
    fn effects(&self) -> SmallVec<[EffectKind; 2]>;       // 从 [effect] 生成
    fn is_branch(&self) -> bool;                          // effect 含 Branch/Jump
    fn branch_targets(&self) -> SmallVec<[Block; 2]>;     // BlockTarget 字段
    fn is_call(&self) -> bool;                            // effect 含 Call
    fn is_ret(&self) -> bool;                             // effect 含 Ret
    fn clobbers(&self) -> &[(u32, RegClass)];             // [implicit] + 写死物理寄存器
    fn is_move(&self) -> Option<(u32, u32)>;              // MOV_*/SD_MOV/SD_FMOV
    fn reg_field(&self, i: usize) -> u32;                 // 按 asm 模板占位符序
    fn set_reg_field(&mut self, i: usize, idx: u32);
    fn has_side_effects(&self) -> bool;
}
```

> `uses()`/`defs()` 返回 **`u32` 物理编号数组**（v10 返回 `SmallVec<[VReg;4]>`）——
> 寄存器字段在指令构造时以默认物理 Reg 占位，分配器通过
> `InstPacket.xreg_map` + `reg_field/set_reg_field` 回填。

### 4. 组件结构体（模块级简单名）

```rust
pub struct IsaInfo;          // impl crate::machine::isa_info::IsaInfo
pub struct RegInfo;          // impl crate::machine::reg_info::TargetRegInfo
pub struct ABI;              // impl crate::machine::abi::TargetABI
pub struct Lowering;         // impl crate::machine::lowering::TargetLowering
pub struct Encoder;          // impl crate::machine::encoder::TargetEncoder
pub struct FrameLowering;    // impl crate::machine::frame::TargetFrameLowering
pub struct Disassembler;     // impl crate::machine::disasm::TargetDisassembler
pub struct Assembler;        // impl crate::machine::assembler::TargetAssembler

#[derive(Clone)]
pub struct TargetMachine { /* 8 个 Arc<dyn ...> 组件 */ }

impl TargetMachine {
    pub fn new() -> Self;        // 组装各组件
    pub fn new_arc() -> Arc<Self>;
}

impl crate::machine::target::TargetMachine for TargetMachine {
    type Inst = Inst;
    type Reg = Reg;
    fn isa_info(&self) -> &Arc<dyn IsaInfo>;          // ...
    fn lowering(&self) -> &Arc<dyn TargetLowering<Inst = Self::Inst>>;
    // ...
    // enable_pattern_isel = true 时额外生成：
    // fn pattern_matcher(&self) -> Option<&PatternMatcher>
}

crate::impl_erased_target_machine!(TargetMachine);

pub fn ensure_registered() { /* 注册默认 reloc patcher + 全局 Registry */ }
```

`Lowering` 委托给模块级自由函数（也是 DSL 生成）：`lower_impl(op, args, results, ctx)`
→ `InstPacket<Inst>`、`lower_terminator_impl(term, value_to_xreg, ctx)`、
`lower_pattern_impl(...)`；`Encoder` 委托 `emit_inst(inst, rm, sink)`；
`FrameLowering` 委托 `emit_prologue_impl`/`emit_epilogue_impl` + spill/jump 覆盖。

### 5. `Disassembler` / `Assembler`

- `Disassembler::disassemble(&Inst) -> String`：按 `asm` 模板格式化
  （`{field}` → Debug、`{field:x}` → 小写十六进制；`Unknown` → `"<unknown>"`）。
- `Assembler`：每 ISA 专属——logos token 枚举 + lalrpop parser（嵌入 token stream，
  编译期生成）+ bind 层（类型签名驱动消歧 + 两遍标签解析）。
  `parse_lines(source) -> Vec<AsmLine>`，`bind(lines) -> Vec<Inst>`。

### 6. 元数据驱动默认类

生成代码中的 `RegClass::GPR64/FPR64` 引用在 post-process 阶段统一替换为
`__DEFAULT_GPR_CLASS` / `__DEFAULT_FPR_CLASS`（`[reg.gpr64]` → `GPR(8)`；
仅 `[reg.gpr]` → 按声明宽度；FPR 默认类 = `[meta].default_fpr_width`，缺省 8）。
非 64 位主类 ISA（如 wasm32）因此不需要硬编码假设。

---

## 标准指令集与默认 Lowering

v11 内置一套**最小完备标准指令集**（前缀 `SD_`），共 **43 条**（`standard_insts.rs`，
v10 是 37 条）。用户只需为这些标准指令提供 `[inst.SD_*]` 定义（encoding/asm/effect），
代码生成器**自动合成**所有 IR Opcode 的 lowering——无需手动编写 `[lower.*]`。

### 工作原理

1. 用户定义 `[inst.SD_XXX]`；
2. 未提供 `[lower.Opcode]` 时使用默认 lowering（由 `[inst.SD_*]` 组合）；
3. 用户规则优先；
4. `[meta].no_default_lowering = true`（如 x86_v10.toml）完全禁用默认 lowering，
   未定义 opcode 返回 `CompileError::Unsupported`。

### 标准指令全集（43 条）

**整数数据传送（2）**：`SD_MOV`(dest: Ireg, src: Ireg)、`SD_MOV_IMM`(dest, imm: I64)

**整数算术（11）**：`SD_ADD`、`SD_SUB`、`SD_MUL`、`SD_UDIV`、`SD_SDIV`、`SD_UREM`、
`SD_SREM`、`SD_AND`、`SD_OR`、`SD_XOR`（均 dest/src）、`SD_NOT`(dest)

**移位/一元（3）**：`SD_SHL`、`SD_SHR`、`SD_SAR`（dest/src）、`SD_NEG`(dest)

**比较（2）**：`SD_CMP`(src1, src2)、`SD_SETCC`(dest, cond: CondCode)

**浮点（12）**：`SD_FMOV`、`SD_FMOV_BITS`、`SD_FADD`、`SD_FSUB`、`SD_FMUL`、
`SD_FDIV`、`SD_FSQRT`（dest/src）、`SD_FNEG`、`SD_FABS`(dest)、`SD_FCMP`(src1, src2)、
`SD_I2F`(dest: Freg, src: Ireg)、`SD_F2I`(dest: Ireg, src: Freg)

**符号扩展（1）**：`SD_SEXT`(dest, src)

**内存（2）**：`SD_LOAD`(dest, src)、`SD_STORE`(addr, val)

**控制流（5）**：`SD_JMP`(rel: BlockTarget)、`SD_JCC`(cond, rel)、`SD_CALL`(target)、
`SD_RET`、`SD_NOP`、`SD_UD2`

**栈（3）**：`SD_PUSH`(reg)、`SD_POP`(reg)、`SD_STACK_ADDR`(dest, offset: I64)

### 默认 Lowering 覆盖范围

| IR Opcode | 默认序列 |
| --------- | -------- |
| Iadd/Isub/Imul/Udiv/Sdiv/Urem/Srem/Band/Bor/Bxor/Ishl/Ushr/Sshr | `SD_MOV` + 对应 SD_* |
| Fadd/Fsub/Fmul/Fdiv/Fsqrt/Fneg/Fabs | `SD_FMOV` + 对应 SD_* |
| Iconst | `SD_MOV_IMM`(rd, iconst) |
| Fconst | `SD_MOV_IMM`(SCRATCH_GPR, fconst) + `SD_FMOV_BITS`(rd, SCRATCH_GPR) |
| Sextend / Uextend / Ireduce / Bitcast | `SD_SEXT` / `SD_MOV` |
| Load / Store | `SD_LOAD` / `SD_STORE`(addr=rs2, val=rs1) |
| Copy / Nop / StackAddr / GetElementPtr | `SD_MOV` / `SD_NOP` / `SD_MOV`(rd, VReg(99)) / `SD_MOV`+`SD_ADD` |
| Call | `SD_UD2`（占位，Call 需架构特化） |
| CallIndirect | `SD_CALL`(rs1) |
| Alloca / GlobalAddr | 占位（`SD_MOV` / `SD_MOV_IMM 0`） |
| Vadd/Vsub/Vmul | 标量 fallback（`SD_MOV` + SD_ADD/SUB/MUL） |
| Vextract/Vinsert | `SD_MOV` 拷贝 |
| Icmp.<Cond> | `SD_XOR`(rd,rd) + `SD_CMP`(rs1,rs2) + `SD_SETCC`(rd, cc) |
| Fcmp.<Cond> | `SD_XOR`(rd,rd) + `SD_FCMP`(rs1,rs2) + `SD_SETCC`(rd, cc) |
| Return / Jump / Branch / Unreachable / Switch | `SD_MOV`(VReg(0),val)+`SD_RET` / `SD_JMP` / `SD_XOR`+`SD_CMP`+`SD_JCC`+`SD_JMP` / `SD_UD2` / `SD_UD2` |

### CondCode 约定

`SD_SETCC`/`SD_JCC` 的 `cond` 字段使用序数值：

- **整数（IntCC）**：0=Equal, 1=NotEqual, 2=SignedLessThan, 3=SignedLessThanOrEqual,
  4=SignedGreaterThan, 5=SignedGreaterThanOrEqual, 6=UnsignedLessThan,
  7=UnsignedLessThanOrEqual, 8=UnsignedGreaterThan, 9=UnsignedGreaterThanOrEqual
- **浮点（FloatCC）**：0=Equal, 1=NotEqual, 2=LessThan, 3=LessThanOrEqual,
  4=GreaterThan, 5=GreaterThanOrEqual, 6=Unordered, 7=Ordered

### 预留 Scratch VReg

默认 lowering 使用以下预留虚拟寄存器（经 `[abi.precolor]` 映射到物理寄存器）：

| VReg | 用途 |
| ---- | ---- |
| VReg(96) | GPR scratch（x86 → R10；Fconst bitcast 中间值） |
| VReg(97) | GPR scratch（x86 → R11；Branch 零寄存器） |
| VReg(99) | 栈指针代理（StackAddr/Alloca） |
| VReg(100) | FPR scratch（x86 → XMM0；浮点返回值） |

---

## x86_v10.toml 实例解析

仓库主示例：`isa/x86_v10.toml`（**1993 行**、**124 个 `[inst.*]` 区块**、
107 个 `[lower.*]`、4 个 `[lower_term.*]`）。展开统计：8 个 opcodes 模板
（`SD_BIN`/`SS_FMOV`/`PS_BIN`/`PD_BIN`/`PI_BIN`/`PS_BIN_V`/`PD_BIN_V`/`SHIFT_BIN`）
展开 42 条，2 个 variants 组（`PUSH`/`POP`）展开 4 条 → 最终 `Inst` 枚举
**162 个变体 + `Unknown(u32)`**。

### 头部声明（meta / reg / abi）

```toml
[meta]
name = "x86_64"
version = "11.0"
endian = "little"
mode = 64
max_inst_len = 15
no_default_lowering = true
gpr_bank_order = ["gpr64", "gpr32", "gpr16", "gpr8l", "gpr8h"]
modrm_force_disp_base = [5, 13]
epilogue_jump_opcode = 0xE9
default_fpr_width = 8
enable_pattern_isel = false

[meta.capabilities]
variable_length = true
prefix_layers = 4
```

寄存器：`gpr64`（RAX–R15）、`gpr32`（EAX–R15D）、`gpr16`（AX–R15W）、
`gpr8l`（AL–R15B）、`gpr8h`（AH/BH/CH/DH，`base_index = 4`）、`xmm`（XMM0–15）。
同族不同宽度视图共享物理编号（RAX/EAX/AX/AL = 0）。

ABI（Windows x64 风格）：`stack_align = 16`、`frame_padding = 8`、
`frame(sp="RSP", fp="RBP", alloc_inst="SUB64_R_IMM32", free_inst="ADD64_R_IMM32")`、
`callee_saved.gpr = [RBX, RDI, RSI, R12-R15]`、`arg_regs.gpr = [RCX, RDX, R8, R9]`、
`ret_regs.gpr = [RAX, RDX]`、`scratch = { TMP0 = 10 /* R10 */, TMP1 = 11 /* R11 */ }`，
以及完整 `[abi.call]`（见 §6）。

### 宽度感知通用指令

```toml
[inst.MOV_R_RM]
fields = { dest = "Ireg", src = "Ireg", opsize = "Opsize" }
encoding = "@modrm opsize 0x8B dest src"
asm = "movrr {dest}, {src}"

[inst.ADD_RM_R]
fields = { dest = "Ireg", src = "Ireg", opsize = "Opsize" }
encoding = "@modrm opsize 0x01 src dest"     # 0x01 是 ADD r/m, r：reg=src、rm=dest
asm = "add {dest}, {src}"

[inst.SUB64_R_IMM32]
fields = { dest = "GprReg", imm = "u32" }
encoding = "@op_rm_imm32 0x81 5 dest imm"    # REX.W + 81 /5 + imm32（帧分配）
asm = "sub {dest}, {imm}"
```

### 移位家族（opcodes 表 + implicit）

```toml
[inst.SHIFT_BIN]
fields = { dest = "Ireg", src = "Ireg", opsize = "Opsize" }
encoding = "@shift_reg 1 dest src {opcode} opsize"
asm = "{mnemonic} {dest}, {src}"
implicit = ["RCX"]                            # CL 是隐式计数寄存器
opcodes = [
  ["ROL_RM_CL", "0", "rol"],
  ["ROR_RM_CL", "1", "ror"],
  ["SHL_RM_CL", "4", "shl"],
  ["SHR_RM_CL", "5", "shr"],
  ["SAR_RM_CL", "7", "sar"],
]
```

`@shift_reg` 的第一个参数 1 是 x86 CL 的物理编号（架构事实进 TOML 不进 DSL），
计数不在 CL 时自动先 `mov CL, src`。

### SSE / AVX 指令族（SIMD 编码）

```toml
# SSE packed-single 家族（@sse_ps_rr：0F xx /r 无强制前缀）
[inst.PS_BIN]
fields = "Freg Freg"
encoding = "@sse_ps_rr {opcode} dest src"
asm = "{mnemonic} {dest}, {src}"
opcodes = [
  ["MOVAPS", "0x28", "movaps"],
  ["ADDPS",  "0x58", "addps"],
  ["SUBPS",  "0x5C", "subps"],
  ["MULPS",  "0x59", "mulps"],
  ["DIVPS",  "0x5E", "divps"],
  ["XORPS",  "0x57", "xorps"],
  ["ANDPS",  "0x54", "andps"],
  ["ORPS",   "0x56", "orps"],
]

# SSE 双精度家族（@sse_rr 66 前缀）
[inst.PD_BIN]
fields = "Freg Freg"
encoding = "@sse_rr 0x66 {opcode} 0 dest src"
asm = "{mnemonic} {dest}, {src}"
opcodes = [
  ["ADDPD", "0x58", "addpd"], ["SUBPD", "0x5C", "subpd"],
  ["MULPD", "0x59", "mulpd"], ["DIVPD", "0x5E", "divpd"],
]

# AVX 三操作数（VEX.256.0F.W0：dest@ModRM.reg、src2@r/m、vvvv=~src1、has_src=1）
[inst.PS_BIN_V]
fields = "Freg Freg Freg"
encoding = "@vex_rrvvv 1 0 0 1 {opcode} dest src2 src1 1"
asm = "{mnemonic} {dest}, {src1}, {src2}"
opcodes = [
  ["VADDPS", "0x58", "vaddps"], ["VSUBPS", "0x5C", "vsubps"],
  ["VMULPS", "0x59", "vmulps"], ["VDIVPS", "0x5E", "vdivps"],
  ["VXORPS", "0x57", "vxorps"], ["VANDPS", "0x54", "vandps"],
]

# AVX2 整数 ymm（断言 avx2_available）
[inst.VPADDD]
fields = "Freg Freg Freg"
encoding = "@vex_rrvvv_avx2 1 1 0 1 0xFE dest src2 src1 1"
asm = "vpaddd {dest}, {src1}, {src2}"

# 无源指令 vvvv=1111：VMOVAPS（has_src=0）、VBROADCASTSS/SD
[inst.VMOVAPS]
fields = "Freg Freg"
encoding = "@vex_rrvvv 1 0 0 1 0x28 dest src 15 0"
asm = "vmovaps {dest}, {src}"

# VEXTRACTF128：dest 在 r/m、src 在 reg（方向与常规相反）、vvvv=1111
[inst.VEXTRACTF128]
fields = { dest = "Freg", src = "Freg", imm = "u8" }
encoding = "@vex_rrvvv_imm 3 1 0 1 0x19 src dest 15 0 imm"
asm = "vextractf128 {dest}, {src}, {imm}"

# VINSERTF128：V128×2 → V256 拼接
[inst.VINSERTF128]
fields = { dest = "Freg", src1 = "Freg", src2 = "Freg", imm = "u8" }
encoding = "@vex_rrvvv_imm 3 1 0 1 0x18 dest src2 src1 1 imm"
asm = "vinsertf128 {dest}, {src1}, {src2}, {imm}"
```

`@vex_rrvvv` 参数：`map pp w l opcode reg rm vv has_src`（如 `1 0 0 1` =
map=0F、pp=00、W=0、L=1）。VEX 编码断言 `avx_available()` / `avx2_available()`。

### 控制流与重定位

```toml
[inst.RET]
encoding = "{0xC3:[0;8]}"
asm = "ret"
effect = "Ret"

[inst.JMP_REL32]
fields = { rel = "BlockTarget" }
encoding = "{0xE9:[0;8]} !rel4"
asm = "jmp .L{rel}"
effect = "Jump"

[inst.JCC_REL32]
fields = { cond = "CondCode", rel = "BlockTarget" }
encoding = "{0x0F:[0;8]} {cond:[0;8]} !rel4"
asm = "j{cond} .L{rel}"
effect = "Branch"

# 跨函数调用：`func` 携带 FuncRef 编号，@call_reloc32 记录 "@N" 重定位
[inst.CALL_RIP_REL]
fields = { target = "i32" }
encoding = "@call_reloc32 target, 0xE8"
asm = "call {target}"
effect = "Call"
implicit = ["RAX", "RCX", "RDX", "RSI", "RDI", "R8", "R9", "R10", "R11"]

# 间接调用（条件 REX 段）
[inst.CALL_RM]
fields = { target = "Ireg" }
encoding = "?target>=8 {0x4:[4;4]}{0:[3;1]}{0:[2;1]}{0:[1;1]}{target:[0;1;3]} {0xFF:[0;8]} {target:[0;3]}{0x2:[3;3]}{3:[6;2]}"
asm = "call *{target}"
effect = "Call"
implicit = ["RAX", "RCX", "RDX", "RSI", "RDI", "R8", "R9", "R10", "R11"]

# 全局地址（PC-relative，ASLR 安全；movabs 绝对地址已废弃）
[inst.LEA_RIP_REL]
fields = { dest = "Ireg", global = "i32" }
encoding = "@lea_rip_rel dest global"
asm = "lea_rip {dest}, [rip+{global}]"
```

### 内存指令（@modrm_mem）

```toml
[inst.MOV_R_MEM]
fields = { dest = "Ireg", base = "Ireg", opsize = "Opsize" }
encoding = "@modrm_mem opsize 0x8B dest base 0"
asm = "mov_mem {dest}, [{base}]"

[inst.STORE_MEM_R]
fields = { base = "Ireg", src = "Ireg", opsize = "Opsize" }
encoding = "@modrm_mem opsize 0x89 src base 0"
asm = "mov_sto [{base}], {src}"

# 原子操作（LOCK 前缀）
[inst.XADD_MEM_R]
fields = { base = "Ireg", src = "Ireg", opsize = "Opsize" }
encoding = "@modrm_mem opsize 0xC1 src base 0 0xF0 0x0F"
asm = "xadd [{base}], {src}"

# spill 模板指令（MemRef 字段）
[inst.MOV64_RM]
fields = { dest = "GprReg", mem = "MemRef" }
encoding = "@modrm_mem 64 0x8B dest mem"
asm = "mov64rm {dest}, {mem}"
effect = "Read"
```

### 序言/尾声与 lowering 片段

```toml
[emit.prologue]
insts = ["push RBP", "mov64rr RBP, RSP", "@push_callee", "@move_args", "@frame_alloc"]

[emit.epilogue]
insts = ["mov64rr RSP, RBP", "sub RSP, 56", "@pop_callee", "pop RBP", "ret"]
```

```toml
[lower.Iadd]
insts = ["mov rd, rs1", "add rd, rs2"]

[lower.Call]
insts = [
    "SUB64_R_IMM32 RSP, 32",       # shadow space
    "MOV_R8_RM64 RCX, arg0",
    "MOV_R8_RM64 RDX, arg1",
    "MOV_R8_RM64 R8, arg2",
    "MOV_R8_RM64 R9, arg3",
    "CALL_RIP_REL func",
    "ADD64_R_IMM32 RSP, 32",
    "MOV_RM8_R64 rd, RAX",
]

[lower.Select]
insts = ["test rs1, rs1", "mov rd, rs3", "cmovne rd, rs2"]   # cond ? then : else

[lower.Fabs]
variants = [{ when = "elem==F32", insts = ["movss rd, rs1", "mov_imm %t, 0x7FFFFFFF",
    "movq_to_xmm %tf:fpr, %t", "andps rd, %tf:fpr"] }]
insts = ["movsd rd, rs1", "mov_imm %t, 0x7FFFFFFFFFFFFFFF",
    "movq_to_xmm %tf:fpr, %t", "andpd rd, %tf:fpr"]
```

### SIMD lowering（variants 谓词示例）

```toml
# Vadd：按结果宽度 × 元素类型分派（f32→PS、f64→PD、i32→PADDD、i64→PADDQ；
# V256 整数走 AVX2 vpaddd/vpaddq）
[lower.Vadd]
variants = [
  { when = "rd==256 && elem==F32", insts = ["vaddps rd, rs1, rs2"] },
  { when = "rd<=128 && elem==F32", insts = ["movaps rd, rs1", "addps rd, rs2"] },
  { when = "rd==256 && elem==F64", insts = ["vaddpd rd, rs1, rs2"] },
  { when = "rd<=128 && elem==F64", insts = ["movaps rd, rs1", "addpd rd, rs2"] },
  { when = "rd==256 && elem==I32", insts = ["vpaddd rd, rs1, rs2"] },
  { when = "rd==256 && elem==I64", insts = ["vpaddq rd, rs1, rs2"] },
  { when = "rd<=128 && elem==I32", insts = ["movaps rd, rs1", "paddd rd, rs2"] },
  { when = "rd<=128 && elem==I64", insts = ["movaps rd, rs1", "paddq rd, rs2"] },
]

# Vconst：常量池 lane 位模式 → XMM（vconst_lo/hi = 32 位元素奇偶分离）
[lower.Vconst]
variants = [
  { when = "rd==64 && elem==F32", insts = ["mov_imm %t1, vconst_lo2", "movq_to_xmm rd, %t1"] },
  { when = "rd==128 && (elem==F32 || elem==I32)", insts = [
      "mov_imm %t1, vconst_lo", "movq_to_xmm %tf1:fpr, %t1",
      "mov_imm %t2, vconst_hi", "movq_to_xmm %tf2:fpr, %t2",
      "punpckldq %tf1:fpr, %tf2:fpr", "movaps rd, %tf1:fpr",
  ] },
  { when = "rd==256 && (elem==F32 || elem==I32)", insts = [
      "mov_imm %t1, vconst_lo", "movq_to_xmm %tf1:fpr, %t1",
      "mov_imm %t2, vconst_hi", "movq_to_xmm %tf2:fpr, %t2",
      "punpckldq %tf1:fpr, %tf2:fpr",
      "mov_imm %t3, vconst_lo_hi", "movq_to_xmm %tf3:fpr, %t3",
      "mov_imm %t4, vconst_hi_hi", "movq_to_xmm %tf4:fpr, %t4",
      "punpckldq %tf3:fpr, %tf4:fpr",
      "vinsertf128 rd, %tf1:fpr, %tf3:fpr, 1",
  ] },
  # ... V64/V128 f64/i64、V256 f64/i64 见文件 ...
  { when = "rd>=64", insts = ["ud2"] },
]

# Vextract：lane0 直取 / V256 拆半 + pshufd / 64 位元素旋转
[lower.Vextract]
variants = [
  { when = "imm0==0 && elem==F32", insts = ["movss rd, rs1"] },
  { when = "rs1==256 && imm0>=4 && elem==F32", insts = [
      "vextractf128 %tf1:fpr, rs1, 1", "pshufd %tf1:fpr, %tf1:fpr, imm0_sub4",
      "movss rd, %tf1:fpr",
  ] },
  # ...
]

# ShuffleVector：SHUFPS imm8（shufps_imm8 编码 mask）或 V256 拆半双 shufps
[lower.ShuffleVector]
variants = [
  { when = "rd==256 && elem==F32", insts = [
      "vextractf128 %tf1:fpr, rs1, 0", "vextractf128 %tf2:fpr, rs2, 0",
      "shufps %tf1:fpr, %tf2:fpr, shufps_imm8",
      "vextractf128 %tf3:fpr, rs1, 1", "vextractf128 %tf4:fpr, rs2, 1",
      "shufps %tf3:fpr, %tf4:fpr, shufps_imm8_hi",
      "vinsertf128 rd, %tf1:fpr, %tf3:fpr, 1",
  ] },
  { when = "rd<=128 && elem==F32", insts = ["movaps rd, rs1", "shufps rd, rs2, shufps_imm8"] },
]
```

### reg_classes / spill / abi.call 小结

- `[reg_classes.GPR]`(w=8)、`GPR32`(w=4)、`FPR`(w=16)、`VEC32`(w=32)——
  allocatable 均排除 RSP(4)/RBP(5)/R10(10)/R11(11)（spill scratch 保护）。
- `[spill.GPR]` = `MOV64_RM`/`MOV64_MR`（base=RBP）、`[spill.FPR]` = `MOVSD_RM`/`MOVSD_MR`。
- `[abi.call]` 配置完整（发参 + 收参 8 个 `entry_*` 字段），`[emit.prologue]` 的
  `@move_args` 才能编译通过。

---

## 附录：TOML 字段速查表

### `[meta]`

| 字段 | 类型 | 默认 | 必需 |
| ---- | ---- | ---- | ---- |
| `name` | String | — | ✅ |
| `version` | String | `""` | ❌ |
| `endian` | String | `"little"` | ❌ |
| `mode` | u8 | `64` | ❌ |
| `max_inst_len` | u8 | `0` | ❌ |
| `no_default_lowering` | bool | `false` | ❌ |
| `no_epilogue_label` | bool | `false` | ❌ |
| `gpr_bank_order` | String[] | `[]` | ❌ |
| `modrm_force_disp_base` | u8[] | `[]` | ❌ |
| `epilogue_jump_opcode` | u8 | 无 | ❌ |
| `default_fpr_width` | u8 | `8` | ❌ |
| `enable_pattern_isel` | bool | `false` | ❌ |
| `capabilities.variable_length` | bool | `false` | ❌ |
| `capabilities.prefix_layers` | u8 | `0` | ❌ |
| `capabilities.simd_widths` | u16[] | `[]` | ❌ |
| `capabilities.mask_registers` | bool | `false` | ❌ |
| `capabilities.broadcast` | bool | `false` | ❌ |
| `capabilities.rounding_mode` | bool | `false` | ❌ |

### `[reg.<group>]`

| 字段 | 类型 | 默认 | 必需 |
| ---- | ---- | ---- | ---- |
| `count` | u16 | — | ✅ |
| `width` | u16 | `64` | ❌ |
| `names` | String[] | 自动生成 | ❌ |
| `prefix` | String | — | ❌ |
| `base_index` | u32 | `0` | ❌ |

### `[abi]`

| 路径 | 类型 | 默认 |
| ---- | ---- | ---- |
| `stack_align` | u32 | `16` |
| `red_zone` | u32 | 无 |
| `frame_padding` | i32 | `0` |
| `fp_push_bytes` | u32 | `8` |
| `frame.sp` | String | 无 |
| `frame.fp` | String | 无 |
| `frame.alloc_inst` | String | `"SUB64_R_IMM32"` |
| `frame.free_inst` | String | `"ADD64_R_IMM32"` |
| `frame.neg_alloc_imm` | bool | `false` |
| `callee_saved.gpr/xmm` | String[] | `[]` |
| `arg_regs.gpr/xmm` | String[] | `[]` |
| `ret_regs.gpr/xmm` | String[] | `[]` |
| `precolor.<VReg>` | String | — |
| `scratch.<NAME>` | u32 | — |
| `call.*` | 见 §6 | — |

### `[inst.<NAME>]`

| 字段 | 类型 | 默认 | 必需 |
| ---- | ---- | ---- | ---- |
| `fields` | 内联表 / 紧凑串 / 省略 | 从 asm 推断 | ❌ |
| `encoding` | String | — | ❌ |
| `asm` | String | — | ✅ |
| `effect` | String[] / String / 子表 | — | ❌ |
| `role` | `"def"\|"use"\|"both"` | 按名字推断 | ❌ |
| `implicit` | String[] | `[]` | ❌ |
| `variants` | Variant[] | — | ❌ |
| `opcodes` | `[String;3][]` | — | ❌ |

FieldType 值（18）：`i8 i16 i32 i64 u8 u16 u32 u64 f32 f64 Ireg Freg GprReg XmmReg MemRef BlockTarget CondCode Opsize`

Effect 值：`Pure Read Write Branch Jump Trap Ret Call`

### `[lower.*]` / `[lower_term.*]` / `[lower_pattern.*]`

| 字段 | 类型 | 必需 |
| ---- | ---- | ---- |
| `insts` | String[] | 二选一 |
| `template` + `conditions` | String[] + `[String;2][]` | 二选一 |
| `variants` | `[{ when, insts }]` | ❌ |

### `[emit]`

| 路径 | 类型 |
| ---- | ---- |
| `prologue.insts` | String[] |
| `epilogue.insts` | String[] |

### `[enc_macros.*]` / `[enc_scatters.*]` / `[dyn.*]` / `[spill.*]`

| Section | 字段 | 类型 |
| ------- | ---- | ---- |
| `[enc_macros.NAME]` | `params` / `pattern` | String[] / String |
| `[enc_scatters.NAME]` | `pattern` | String |
| `[dyn.NAME]` | `kind` / `values` / `default` | String / u8[] / u8 |
| `[spill.CLASS]` | `load` / `store` | `{ inst, base }` |
| `[reg_classes.NAME]` | `width` / `allocatable` | u8 / u8[] |
| `[cc_names]` | 键 = cc 值 → 值 = 后缀 | String → String |
| `[lang.tokens.NAME]` | `pattern` | String |
| `[lang.keywords]` / `[lang.rules]` | String → String | — |
