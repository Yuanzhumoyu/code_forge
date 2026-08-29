# ISA-DSL v12 — 语法规范（唯一 DSL 语法）

> 本文档对应 `forge-dsl`（crate 名 `forge_dsl`，proc-macro）的 **v12** 版本，
> 与仓库当前代码逐项核对（`crates/frontend/forge-dsl/src/v12/`、`isa/x86_v12.toml`、
> `isa/riscv64_v12.toml`）。
>
> **v12 是唯一 DSL 语法**：v11 语法层（`encoding` 字符串 + `@原语`、紧凑
> `fields` 串、`when` 谓词串、asm 隐式魔法名）已整体移除——无兼容层、无转换
> 工具、无逃生门。v11 风格文件解析必然失败（`deny_unknown_fields`）。
> 历史设计决策与迭代记录见 [`docs/isa-dsl-v12-roadmap.md`](./isa-dsl-v12-roadmap.md)。

## 目录

- [ISA-DSL v12 — 语法规范（唯一 DSL 语法）](#isa-dsl-v12--语法规范唯一-dsl-语法)
  - [目录](#目录)
  - [快速开始](#快速开始)
  - [`[meta]` — 元信息与寄存器组](#meta--元信息与寄存器组)
  - [`[conventions]` — ISA 约定](#conventions--isa-约定)
  - [`[[operand_slots]]` — 操作数槽](#operand_slots--操作数槽)
  - [`[[forms]]` — 编码形式（语义键）](#forms--编码形式语义键)
  - [`[[instructions]]` — 指令](#instructions--指令)
  - [`[[families]]` — 参数化指令族](#families--参数化指令族)
  - [结构化谓词](#结构化谓词)
  - [`[[lowering]]` — 指令选择](#lowering--指令选择)
  - [`[abi]` — 调用约定](#abi--调用约定)
  - [`[emit]` — 序言/尾声](#emit--序言尾声)
  - [asm 模板](#asm-模板)
  - [代码生成输出](#代码生成输出)
  - [迭代记录（B3/B2/D/E：TargetMachine 接入、QEMU 验证、汇编器/解码器增强）](#迭代记录b3b2detargetmachine-接入qemu-验证汇编器解码器增强)
    - [`[meta].default_opsize`（E）](#metadefault_opsizee)
    - [`[abi]` 新键（B3/B2）](#abi-新键b3b2)
    - [`[abi.frame]` 新键（B2/B2+）](#abiframe-新键b2b2)
    - [`[emit]` 新键（B2/B3）](#emit-新键b2b3)
    - [`[spill.*]` 新形态（B2）](#spill-新形态b2)
    - [解码器增强（E）](#解码器增强e)
    - [汇编器增强（D）](#汇编器增强d)
    - [指令级 opsize/rex\_w 覆盖（已随 EVEX 迭代落地）](#指令级-opsizerex_w-覆盖已随-evex-迭代落地)

---

## 快速开始

在任意 crate 中（推荐 `forge-codegen` 的 `arch/` 目录）：

```rust
// crates/backend/forge-codegen/src/arch/my_isa.rs
forge_dsl::isa_from_file!("isa/my_isa.toml");
pub use self::my_isa::*; // 模块名 = 文件 stem（小写、`-` → `_`）
```

`isa_from_file!` 生成 `pub mod <file_stem>` 自包含模块：`Reg` 物理寄存器枚举、
`Inst` 指令枚举、`encode` / `decode` / `disassemble` / `assemble` 自由函数，以及
TargetMachine 集成层（`TargetMachine` / `Encoder` / `Decoder` / `Disassembler` /
`Assembler` / `ABI` / `FrameLowering` / `Lowering` / `RegInfo` / `IsaInfo` /
`ensure_registered`）。生成代码仅依赖 std + forge-codegen 的 `crate::prelude`，
无 lalrpop / forge-asm 运行时。

## `[meta]` — 元信息与寄存器组

```toml
[meta]
name = "x86_64_v12"          # Registry 注册名（ensure_registered 用它）
version = "12.0"
endian = "little"            # 缺省 little
mode = 64                    # 缺省 64
default_inst_width = 32      # 定宽 ISA（riscv64_v12）；变长 ISA 省略
variable_length = true       # 变长 ISA（x86）；与 default_inst_width 互斥
max_inst_len = 15            # 变长 ISA 最大指令长度

[reg.gpr64]                  # 寄存器组
width = 64
names = ["RAX", "..."]       # 显式名单；或 count + prefix 生成式声明
count = 16
prefix = "XMM"
base_index = 4               # 物理编号偏移（如 gpr8h 高字节组）
```

寄存器物理编号 = **组内索引**（`Reg::to_index()`），这是 v12 与 v11
（`16+i` 浮点索引，产生非规范字节）的根本区别。

## `[conventions]` — ISA 约定

一次声明、指令引用，避免逐指令写位域。

```toml
[conventions.bitfields]        # 命名位域（SLEIGH 风格）
rd  = { offset = 7,  width = 5 }
rs1 = { offset = 15, width = 5 }
rs2 = { offset = 20, width = 5 }
opcode = { offset = 0, width = 7 }

[conventions.modrm]            # 表存在即启用 ModRM 语义
reg_field = "modrm_reg"
rm_field = "modrm_rm"
force_disp_base = [5, 13]      # mod=00+rm=101 是 RIP-rel，须强制位移

[conventions.rex]
w_opsize = 64

[conventions.opsize_prefix]    # 键为数字字符串（TOML 裸整数键）
16 = 0x66
```

## `[[operand_slots]]` — 操作数槽

```toml
[[operand_slots]]
name = "gpr"
kind = "reg"                   # reg / imm / mem / label / cond
class = "gpr"                  # reg：所属 [reg.*] 组；省略 → 多态槽（宽度由实际寄存器推导）
field_width = 5                # reg：编码宽度（位）
roles = ["in", "out"]          # 缺省 ["in"]；"inout" = 读改写（in 且 out）

[[operand_slots]]
name = "imm32"
kind = "imm"
signed = true
width = 32
```

`cond` 槽 = 条件码（x86 opcode 低 4 位）。

> **v12.1 无 `opsize` 槽**：操作数宽度语义由**寄存器视图**推导（Reg 枚举
> 多宽度视图 gpr8/gpr16/gpr32/gpr64，`Reg::width()`），不再有显式 opsize
> 操作数。class 固定的槽（如 `gpr32`）→ 固定宽度；class 省略的槽（如
> `gprx` 多态）→ 宽度随分配到的寄存器。

## `[[forms]]` — 编码形式（语义键）

形式 = 语义键组合（结构化配置，实现为生成器内部函数；开放可扩展集合），
替代 v11 的 `@原语` 字符串白名单。

```toml
[[forms]]
name = "RR"                    # x86 ModRM 寄存器-寄存器
modrm = "rr"                   # rr / ext / rm_mem / rm_memref
rex = "auto"                   # auto（opsize==64）/ field / 数字
prefix = "opsize"              # opsize / field / 数字（SSE 的 66/F2/F3）
opcode_bytes = 1

[[forms]]
name = "SSE_RR"
modrm = "rr"
escape = [0x0F]                # 结构化数组（0F 前缀）
opcode_bytes = 1

[[forms]]
name = "VEX_RRV"               # AVX 三操作数：C4 + vex2/vex3
vex = { map = "inst", pp = "inst", l = "inst" }
modrm = "rr"

[[forms]]
name = "R"                     # riscv R-type（定宽）
opcode_field = "opcode"        # 定宽：主 opcode 位域名
operand_slots = ["gpr", "gpr", "gpr"]
```

已实现的语义键见 `isa/x86_v12.toml` 的 `[[forms]]`（MRR/MRR_EXT/SSE_RR/VEX_RR* /
MRR_MEM/MRR_MEMREF/REL32/NOOP/NOOP_REXW/NOOP_ESC/OP_EXT/MOV_R_RM 等）。

## `[[instructions]]` — 指令

```toml
[[instructions]]
name = "MOV_R_RM"              # 指令名（PascalCase；生成 Inst 枚举变体）
form = "RR"
opcode = 0x8B
asm = "movrr {0:[gprx:out]}, {1:[gprx:in]}"   # 必填；操作数声明内联在 asm 中

[[instructions]]
name = "ADD"                   # riscv R-type（定宽位域绑定）
form = "R"
opcode = 0x33
fields = { funct3 = 0, funct7 = 0 }   # 固定字段值，按位域名引用
asm = "add {0:[gpr:out]}, {1:[gpr:in]}, {2:[gpr:in]}"   # 操作数按 form.operand_fields 位置绑定位域

[[instructions]]
name = "CQO"                   # 隐式寄存器破坏声明（物理寄存器名）
form = "NOOP_REXW"
opcode = 0x99
implicit_regs = ["RDX"]        # cqo 写 RDX：regalloc 据此避开
```

**asm 占位符语法**：`{n:[槽:角色]}` 内联操作数声明——`n` 为操作数序号
（0 起连续），`槽` 引用 `[[operand_slots]]` 名，`角色` 为 `in`（缺省）/
`out`/`inout`。**助记符 = asm 首词**（唯一事实来源，v12.1 起无独立
`mnemonic` 字段）；`implicit_regs` 是 v12 的显式隐式寄存器声明（x86 的
shift 用 CL、idiv 用 RAX:RDX、cqo 写 RDX），生成的 `MachineInst::clobbers()`
供寄存器分配器避开。

## `[[families]]` — 参数化指令族

```toml
[[families]]
name = "MOVSD_M"                # 3 操作数指令族（mnemonic 由变体名小写生成）
form = "MRR_0F_FIX64"
asm = "movsd {out}, {1}"        # 变体 asm 可引用 `{name}` 占位符继承族模板
[[families.variants]]
name = "MOVSD_RR"               # 变体名小写 = 助记符（movsd_rr）；同名不同形状按操作数签名消歧
opcode = 0x10
```

族展开 = 每个 variant 生成一条完整指令；`{name}` 占位符（族模板内）=

变体名小写，实现共享 asm 的助记符继承。

## 结构化谓词

纯 TOML 数据的小型谓词语言（无字符串）：`and` / `or` / `not` / `eq` / `ne` /
`lt` / `le` / `gt` / `ge`，操作数属性引用，求值实现见
`crates/frontend/forge-dsl/src/v12/pred.rs`（lowering 经
`gen_lowering_attrs` 求值，谓词 false → 该规则不匹配，落入下一条或
Unsupported）。

可用属性（lowering 时由 IR 层求值）：

| 属性 | 语义 |
| --- | --- |
| `rd` / `rs1_width` / `rs2_width` | 结果/操作数 1/2 的宽度（标量 = `bits()`；向量 = `type_ctx.size_bytes()*8`） |
| `elem` | 元素类型 id（`elem_id_of`；F32=1、F64=2、I32=3、I64=4；向量经 `type_ctx.element_type()`） |
| `cond` | 比较条件 id（Fcmp→fcmp_id、Icmp→icmp_id；setcc/cmovcc 用） |
| `imm0` | `current_immediates[0]`（Vextract/Vinsert/Vsplit 的 lane 索引；**AtomicRmw 的 op 判别值**：Xchg=0/Add=1/Sub=2/...） |

```toml
[[lowering]]
op = "AtomicRmw"
when = { eq = ["imm0", 1] }      # Add → LOCK XADD
insts = ["movrr {t1}, {1}", "xadd [{0}], {t1}", "movrr {out}, {t1}"]
```

## `[[lowering]]` — 指令选择

```toml
[[lowering]]
op = "Iadd"
insts = ["movrr {out}, {0}", "add {1}, {out}"]
```

**指令用助记符引用**（v12.1 起）：`insts` 里的指令名 = 该指令 asm 的
**首词**（助记符，唯一事实来源），不再用指令名（PascalCase）。同助记符
多形状（如 `add reg,reg` 与 `add reg,imm`、`xadd reg,mem` vs `mem,reg`）
由生成器按**操作数 token 签名**自动消歧（reg/imm/mem/cond/label；
`[n]` 剥括号为内存基址=reg；数字字面量兼容 imm/cond）；仍多候选时再按
**字面段一致性**（模板 vs asm 的内存括号）过滤。

**宽度分派**（`when` 谓词按 IR 类型选 32/64 位指令，v12.1 关键实践）：
i32 操作数须用 32 位指令，否则 64 位 load/store 会读写相邻 i32 栈槽
（槽距 4 字节）——Load/Store/Icmp 均按 `rd`/`rs1_width` 分派：

```toml
[[lowering]]
op = "Load"
when = { eq = ["rd", 32] }
insts = ["mov_mem32 {out}, [{0}]"]
[[lowering]]
op = "Icmp"
when = { eq = ["rs1_width", 32] }
insts = ["xor {out}, {out}", "cmp32 {1}, {0}", "setcc {out}, {cc}"]
```

符号化操作数（每条 `insts` 是直线指令序列，同 op 多规则用 `when` 谓词
if-else-if 链分派，无 when 的规则 = 无条件兜底）：

| token | 语义 |
| --- | --- |
| `{out}` / `{out2}` | 结果（out2 = 第二结果，如溢出 flag；rd2 预绑定） |
| `{0}`/`{1}`/`{2}` | 第 0/1/2 个 IR 操作数（rs1/rs2/rs3） |
| `{t}`、`{t1}`..`{t5}` | 临时 GPR XReg（模板级预分配） |
| `{t_f}`、`{t_f1}`..`{t_f4}` | 临时 FPR XReg |
| `{iconst}` / `{fconst}` | 常量池整数/浮点位模式（`current_const_index`） |
| `{off}` | `ctx.current_offset`（StackAddr 帧偏移，已平移 callee_saved） |
| `{alloca}` | `ctx.current_alloca_offset`（Alloca 栈槽偏移） |
| `{imm0}` / `{imm0_sub4}` | `current_immediates[0]`（lane 索引；sub4 = 值-4） |
| `{shufps_imm8}` / `{shufps_imm8_hi}` | SHUFPS mask 常量（低/高 2 位字段） |
| `{vconst_lo2}` / `{vconst_lo}` / `{vconst_hi}` / `{vconst_lo_hi}` / `{vconst_hi_hi}` | 向量常量字节池拆分（V64/V128/V256 lane 位模式） |
| `{cc}` | Icmp 条件码（`__cc`） |
| `{global}` | `ctx.current_global` 的 GlobalId（**负编码 -(id+1)**，配 `MOVABS_GLOBAL` → ABS8 重定位） |

物理寄存器名（`RAX`/`RCX`/`XMM0` 等）直接写死为 `Reg::*` 并自动收集为
clobber（regalloc 本点避开）；`opsize` 槽缺省 `ctx.default_opsize`（按 IR
类型 32/64）；数字立即数/条件码直接写（如 `ROUNDSD_I {t_f1}, {t_f1}, 3`）。

**符号重定位（JIT）**：

- **Call**：`CALL_RIP_REL` 的 Label 槽 `rel<0` 编码函数符号
  `-(FuncRef+1)` → encoder 转 `add_reloc(CALL, "@N")`（JIT 符号表按
  FuncRef 序注册）；`rel>=0` 为块内 label（REL4）。
- **GlobalAddr**：`MOVABS_GLOBAL` 的 imm 槽 `<0` 编码 GlobalId
  `-(id+1)` → encoder 转 `add_reloc(ABS8, "G{id}")`（JIT 编译模块时按
  全局变量注册数据段符号并即时 patch）；imm≥0 为普通 64 位立即数。

## `[abi]` — 调用约定

```toml
[abi]
stack_align = 16
scratch = ["R10", "R11"]       # spill load/store 专用（须排除 allocatable）
[[abi.arg_class]]
class = "int"
regs = ["RCX", "RDX", "R8", "R9"]
[[abi.arg_class]]
class = "float"
regs = ["XMM0", "XMM1", "XMM2", "XMM3"]
[[abi.arg_class]]
class = "vector"
strategy = "by-ref"            # >limit 位按引用传参（YMM ABI 铺路）
limit = 128
```

## `[emit]` — 序言/尾声

```toml
[emit.prologue]
insts = ["push RBP", "mov64rr RBP, RSP", "@push_callee", "@frame_alloc"]
```

`@push_callee` / `@frame_alloc` / `@frame_dealloc` / `@pop_callee` 为伪指令，
由 FrameLowering 展开为具体序列。

## asm 模板

asm 完整格式：**首词 = mnemonic**，后续为逗号分隔操作数；支持 `{0}`/`{1}`
位置占位与 `{name}` 命名占位，以及通用模板段 `Seg = Lit | Op`（mnemonic 与
模板分离）。操作数形状：simple（寄存器/立即数）、memory（`{I}({J})` 基址+
位移，如 `8(X2)`）、memory0（`({J})`）。寄存器名大小写不敏感（`xmm0` /
`XMM0` 等价），`opsize` 操作数非文本、asm 默认 64。

## 代码生成输出

`pub mod <file_stem>`（生成模块名 = 文件 stem，非 meta.name）：

- **自包含部分**：`Reg` 枚举、`Inst` 枚举、`encode(&Inst) -> Result<Vec<u8>, String>`、
  `decode(&[u8]) -> Option<(Inst, usize)>`、`disassemble(&Inst) -> String`、
  `assemble(&str) -> Result<Inst, String>`——仅依赖 std。
- **Inst 字段类型化**（v12 类型化重构）：寄存器操作数为 `Reg` 枚举
  （`MovRRm { dest: Reg, src: Reg, opsize: u8 }`、riscv `Add { rd: Reg, rs1: Reg,
  rs2: Reg }`），opsize → `u8`、cond → `u8`、mem → `MemRef`、imm/label → `i64`；
  字段名按操作数语义生成（out→dest/rd、in→src/rs1/rs2、cond→cond、rel→target
  等），不再有裸 `u32` 位置字段（op0/op1/op2）——类型约束与可读性对齐 v11。
  regalloc 经 `Reg::from_index(phys, <槽位 RegClass>)` 回填（GPR/FPR 消歧正确）。
- **集成层**：`TargetMachine`（组合 `IsaInfo`/`RegInfo`/`ABI`/`Lowering`/
  `Encoder`/`FrameLowering`/`Disassembler`/`Assembler`/`Decoder`）、
  `MachineInst`（uses/defs/reg_field/set_reg_field/effects/branch_targets/
  clobbers）、`ensure_registered()`（注册 Registry + reloc patcher）。

已有 ISA 文件：`isa/x86_v12.toml`（204 条指令，变长语义键）与
`isa/riscv64_v12.toml`（76 条，定宽试点，仅自包含模块未接 TargetMachine）。

---

## 迭代记录（B3/B2/D/E：TargetMachine 接入、QEMU 验证、汇编器/解码器增强）

### `[meta].default_opsize`（E）

变长 ISA 无显式 opsize 语义的 form 在 decode 时的 `__opsize` 初始值
（位；缺省 None = 4 = 32 位——现状语义）。影响 REX.W/宽度 guard 的缺省判定。

```toml
[meta]
default_opsize = 64
```

### `[abi]` 新键（B3/B2）

```toml
[abi]
move_inst = "MOV64"        # @move_args 收参 mov 指令名（缺省 "MOV_RM8_R64"）
ret_mov_inst = "MOV64"     # Return 返回值 mov 指令名（缺省 "MOV_RM8_R64"）
ret_regs = ["X10"]         # 返回寄存器（缺省空 = index 0，x86 RAX 语义）
call_inst = "JAL"          # Call 调用指令（缺省 "CALL_RIP_REL"=x86）
call_ret_reg = "X1"        # Call 的返回地址寄存器（缺省 "X1"=riscv ra）
call_clobbers = ["X1", "X7", "X10", "X28"]  # Call 点被调用方破坏的寄存器
reserved = ["X0", "X1", "X3", "X4"]         # regalloc 不可分配寄存器
```

- `move_inst`/`ret_mov_inst`：生成器按**角色**解析 src/dest（In=src、
  Out/InOut=dest），两种操作数序（x86 src/dest、demo dest/src）皆可。
- `ret_regs`：Return/Call lowering 的返回值移动目标（riscv `X10`=a0）。
- `call_inst`/`call_ret_reg`：定宽 ISA 的跨函数 Call（riscv `jal ra, @N`——
  label 槽 = -(FuncRef+1) → encoder 转 `@N` 符号 reloc，RiscvRelocPatcher 编码
  UJ 位段；Reg 槽填 `call_ret_reg`）。
- `call_clobbers`：Call 点被调用方破坏的寄存器（regalloc clobber 集）。
  缺省 = 整数参数寄存器 + 返回寄存器（x86 语义）。**定宽 ISA 无 callee-saved
  保存序列时须列全 caller-saved**，否则跨调用存活值留在寄存器被覆盖
  （实测递归 fib 死循环）。**s 系（@push_callee 保存）不在列表**。
- `reserved`：regalloc 不可分配寄存器（riscv X0=zero 写入无效、X1=ra 被
  prologue/call 占用、X3/X4=gp/tp）——不排除会分配出垃圾（实测 `subw x0`
  结果丢失）。

### `[abi.frame]` 新键（B2/B2+）

```toml
[abi.frame]
alloc_neg = true           # @frame_alloc 立即数取负（riscv `addi sp, sp, -N`）
min_frame_bytes = 16       # 帧最小字节数（riscv 的 ra/fp 保存槽需帧 ≥ 固定值）
callee_saved_bytes_override = 0  # callee-saved 区字节数覆盖（缺省 None =
                           # fp_overhead + callee_saved×宽，x86 语义）
stack_slot_shift = 16      # 栈槽（StackAddr/Alloca）帧顶平移（缺省 None =
                           # 回退 callee_saved_bytes，x86 语义）
```

- `callee_saved_bytes_override`：riscv 的 callee_saved 保存槽在**帧内顶部**
  （@push_callee 的 SD 到 `[sp+frame-16-k*8]`，min_frame_bytes 覆盖）→ 覆盖 0：
  spill 槽 sp_base = -(frame) 留在帧内底部、StackAddr 平移 0——否则 spill 槽
  落帧外（sp 下方更深处），递归时与 callee 帧重叠（实测 fib 死循环）。
- `stack_slot_shift`：栈槽基准 = fp - shift（riscv 16 = fp_push_bytes，避开
  ra/fp 保存槽且递归各帧独立）。相对 sp 的栈槽在递归时跨帧绝对地址重叠
  （实测 fib_slot 结果错）。

### `[emit]` 新键（B2/B3）

```toml
[emit]
epilogue_label = false     # 不生成独立尾声标签（无 JMP 指令的定宽 ISA；
                           # return block 直接 fall-through 到尾声——单 return
                           # block 函数安全）
```

`[emit.prologue]/[emit.epilogue]` 模板支持占位符：

| 占位符 | 语义 |
| --- | --- |
| `{frame_size}` | 运行时 `frame_size`（emit 模式） |
| `{frame_size_neg}` | `-frame_size` |
| `{frame_size_mN}` | `frame_size - N`（如 riscv 的 `SD X1, X2, {frame_size_m8}` 保存 ra 到帧顶） |

### `[spill.*]` 新形态（B2）

riscv 式三操作数（reg + reg + imm，无 Mem 槽）：

```toml
[spill.GPR]
load = "LD {0}, X2, {1}"    # {0}=目标寄存器、字面 X2=基址、{1}=__off 立即数
store = "SD {0}, X2, {1}"
base = "X2"
```

（x86 式 `MOV64_RM {0}, {1}`（{1}=MemRef）仍支持。）

### 解码器增强（E）

- **decode 错误带部分匹配偏移**：`TargetDecoder::decode` 失败返回
  `DecodeError::InvalidBytes(n)`，n = 已消费的前缀字节数（x86 的
  66/F0/F2/F3/REX 前缀；定宽 ISA = 0）。生成 `decode_partial` 内部函数。
- **大端变长 imm**：`imm_read_ts` 按 `[meta].endian` 装配（little →
  `from_le_bytes`、big → `from_be_bytes`）。

### 汇编器增强（D）

`TargetAssembler::parse_insts` 支持：

- **`.equ name, expr`**：符号常量（顺序求值，前向引用失败；指令立即数
  表达式可引用）。
- **立即数表达式**：`+ - * / % << >> & | ^ ~ ( )` 递归下降求值（token
  层已有完整运算符集合）；label 槽（分支目标）同样支持表达式。
- **数据伪指令**：`.word`/`.hword`/`.dword`（按 meta.endian 写多字节）、
  `.ascii "..."`/`.asciz "..."`（`\n \t \r \" \\ \0` 转义）、`.zero n`。
- **`.macro name params` / `.endm`**：文本宏（参数引用 `\arg` 或 `%arg`；
  嵌套宏调用就地展开，深度上限 16 轮 / 64 行防递归）。
- **结构化错误带行号**：`line N:` 前缀（ParseError/UndefinedLabel/Other/
  Ambiguous/TypeMismatch 均带）。

### 指令级 opsize/rex_w 覆盖（已随 EVEX 迭代落地）

```toml
[[instructions]]
name = "Cvtsi2sd"
opsize = "s1"          # 指令级覆盖 form 级 opsize
rex_w = "auto"
```
