# ISA-DSL v15 — 语法规范（唯一 DSL 语法）

> 本文档对应 `forge-dsl`（crate 名 `forge_dsl`，proc-macro）当前的 **v15** schema，
> 与仓库当前代码逐项核对（`crates/frontend/forge-dsl/src/v12/`、`isa/x86_v12.toml`、
> `isa/riscv64_v12.toml`）。
>
> **命名说明**：DSL 生成器的 Rust 模块路径仍是 `v12/`（`crates/frontend/forge-dsl/
> src/v12/`，历史遗留命名，稳定不动）；"v15" 是 schema 的迭代号（v15-S1…S6 破坏性
> 简化），`[meta].version` 是自由字符串（当前两份谱写 `"13.0"`），三者互相独立。
> **v11 语法层**（`encoding` 字符串 + `@原语`、紧凑 `fields` 串、`when` 谓词串、
> asm 隐式魔法名）已整体移除——无兼容层、无转换工具、无逃生门。v11 风格文件解析
> 必然失败（`deny_unknown_fields`）。历史设计决策与迭代记录见
> [`docs/archive/isa-dsl-v12-roadmap.md`](../archive/isa-dsl-v12-roadmap.md)。

## 目录

- [ISA-DSL v15 — 语法规范（唯一 DSL 语法）](#isa-dsl-v15--语法规范唯一-dsl-语法)
  - [目录](#目录)
  - [v15 迭代总览（S1–S6）](#v15-迭代总览s1s6)
  - [快速开始](#快速开始)
  - [`[meta]` — 元信息与寄存器组](#meta--元信息与寄存器组)
  - [`[conventions]` — ISA 约定](#conventions--isa-约定)
  - [`[[operand_slots]]` — 操作数槽](#operand_slots--操作数槽)
  - [`[[forms]]` — 编码形式（可选预设）](#forms--编码形式可选预设)
  - [`[[instructions]]` — 指令](#instructions--指令)
  - [`[[families]]` — 参数化指令族](#families--参数化指令族)
  - [结构化谓词](#结构化谓词)
  - [`[[lowering]]` — 指令选择](#lowering--指令选择)
  - [`[[pattern]]` — 树型多指令匹配](#pattern--树型多指令匹配)
  - [`[abi]` — 调用约定](#abi--调用约定)
  - [`[abi.frame]` — 帧布局](#abiframe--帧布局)
  - [`[emit]` — 序言/尾声](#emit--序言尾声)
  - [`[spill.*]` — 溢出模板](#spill--溢出模板)
  - [asm 模板](#asm-模板)
  - [代码生成输出](#代码生成输出)
  - [已有 ISA 文件](#已有-isa-文件)

---

## v15 迭代总览（S1–S6）

v14 是 v12 的增量扩展，沉淀出三类结构性债务：`[[forms]]` 组合爆炸（x86 47 个
form 里 17 个只被一条指令用）、`when` 表达力不足导致的复制粘贴（x86 261 条
lowering 里 66 条同 op + 逐字节相同 insts、只差 when）、以及 16 处 x86 指令名
硬编码默认值。v15 分六步做了破坏性简化（每步门禁全绿、逐字节等价）：

| 阶段 | 内容 |
| --- | --- |
| S1 | 删死键（`forms.opcode_bytes`/`operand_slots.field_width`/`values`/`forms.operand_slots`/`[conventions.rex]`/`[conventions.opsize_prefix]`/死 form）；`Spanned` 行号诊断；`include_bytes!` 依赖跟踪（删 `touch` workaround）；补齐 lowering 校验；字符串键枚举化 |
| S2 | lowering `vary` 行表 + `in` 集合谓词 + 特异性排序（去声明序依赖）+ 重复/死规则编译期报错 |
| S3 | 命名操作数（`ops` 声明、`asm` 只引用）；`modrm` 六魔法串 → 显式映射；`[[forms]]` 降为可选预设（指令逐键覆盖）；`opsize` 单位统一为位 |
| S4 | `[abi]` 13 个 `*_inst` 名指针 + `tags` → 指令上的 `roles` 枚举；删 16 处 x86 硬编码默认 |
| S5 | `[abi.frame]` 4 个 riscv 旋钮 → `layout` 枚举 + 运行期推导；`{callee_saved_bytes}` 占位符替掉 x86 尾声魔法数 56 |
| S6 | `[[pattern]]` 树型多 op 匹配（新增能力）；删 `ext/pattern_isel.rs` 死模块 |

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

**S1 起**：生成模块内嵌 `include_bytes!(<TOML 绝对路径>)`，rustc 把 TOML 当编译
依赖——改 `isa/*.toml` 直接触发重编译，**不再需要手动 `touch arch/<isa>.rs`**。
`FGE_DEBUG_GEN=1` 可 dump 生成代码到 `%TEMP%\forge_gen_*.rs`。

## `[meta]` — 元信息与寄存器组

```toml
[meta]
name = "x86_64_v12"          # Registry 注册名（ensure_registered 用它）
version = "13.0"             # 自由字符串（与 schema 迭代号 v15 无关）
endian = "little"            # 缺省 little
mode = 64                    # 缺省 64
default_inst_width = 32      # 定宽 ISA（riscv64_v12）；变长 ISA 省略
variable_length = true       # 变长 ISA（x86）；与 default_inst_width 互斥
max_inst_len = 15            # 变长 ISA 最大指令长度
default_opsize = 64          # 可选：变长 ISA 无显式 opsize 的 form 在 decode 时
                             # 的 __opsize 初始值（位；缺省 4 = 32 位）
case_insensitive_regs = true # 可选：寄存器解析大小写不敏感
mnemonic_case = "insensitive" # insensitive（缺省）/ sensitive
comment_char = "#"           # 行注释起始字符（缺省 "#"）
label_suffix = ":"           # 标签定义后缀（缺省 ":"）
directive_prefix = "."       # 伪指令前缀（缺省 "."）
imm_prefix = "$"             # 可选：立即数前缀（x86 AT&T "$"、ARM "#"）

[reg.gpr64]                  # 寄存器组
width = 64
names = ["RAX", "..."]       # 显式名单；或 count + prefix 生成式声明
count = 16
prefix = "XMM"
base_index = 4               # 物理编号偏移（如 gpr8h 高字节组）
```

寄存器物理编号 = **组内索引**（`Reg::to_index()`），这是 v12 与 v11
（`16+i` 浮点索引，产生非规范字节）的根本区别。`[reg.*]` 的组名编码宽度
（`gpr8`/`fpr4`/`vec8`/`kreg8`），`RegClass` 四族 GPR/FPR/VEC/KReg 各带字节宽。

## `[conventions]` — ISA 约定

一次声明、指令引用，避免逐指令写位域。

```toml
[conventions.bitfields]        # 命名位域（SLEIGH 风格；定宽 ISA 用）
rd  = { offset = 7,  width = 5 }
rs1 = { offset = 15, width = 5 }
imm_b = { pieces = [ { offset = 31, width = 1, shift = 12 }, ... ] }  # 散布位段

[conventions.modrm]            # 表存在即启用 ModRM 语义
reg_field = "modrm_reg"        # （定宽 ISA；变长 x86 直接用 ModRM 字节）
rm_field = "modrm_rm"
force_disp_base = [5, 13]      # mod=00+rm=101 是 RIP-rel，须强制位移

[conventions.cond]             # 条件码表（name → 编码值）；缺省 = x86 16 项
o = 0x0                        # (o/no/b/ae/e/ne/be/a/s/ns/p/np/l/ge/le/g ↔ 0..15)

[conventions.prefix_scan]      # 可选：变长解码前缀扫描表（缺省 = x86 扫描集）
[[conventions.prefix_scan]]
byte = 0x66
effects = ["opsize16"]
```

> **S1 删除**：`[conventions.rex]`（`w_opsize`）与 `[conventions.opsize_prefix]`
> 已移除——它们 codegen 从不读取，66 前缀与 REX.W 在 `vlen.rs` 里按 `__opsize`
> 硬编码。写这两个表现在直接 `deny_unknown_fields` 报错。

位域支持**散布位段**（`pieces`）：立即数分段放置（S/U/B/J 型）。编码
`word |= ((value >> shift) & mask) << offset`；解码 `value |= ((word >> offset)
& mask) << shift`（可逆）。

## `[[operand_slots]]` — 操作数槽

```toml
[[operand_slots]]
name = "gpr"
kind = "reg"                   # reg / imm / mem / label / cond
class = "gpr"                  # reg：所属 [reg.*] 组；省略 → 多态槽（宽度由实际寄存器推导）
# classes = ["gpr8", "gpr16"]  # 多宽度/多类型集合（class 是单元素糖；两者皆无 = 任意寄存器类）
byte_reg = true                # 可选：8 位寄存器操作数（spl/bpl/sil/dil 无 REX 时编码 ah/ch/dh/bh）
roles = ["in", "out"]          # 缺省 ["in"]；"inout" = 读改写（in 且 out）

[[operand_slots]]
name = "imm32"
kind = "imm"
signed = true
width = 32
min = -2147483648              # 可选：值域约束（缺省按 width/signed 推导）
max = 2147483647
```

`cond` 槽 = 条件码（x86 opcode 低 4 位）。

> **S1 删除 `field_width`**：v14 的 `operand_slots.field_width` 是死键（validate
> 强制要求、codegen 从不读取）。操作数宽度语义由**寄存器视图**推导（Reg 枚举多
> 宽度视图 gpr8/gpr16/gpr32/gpr64，`Reg::width()`），不再有显式 opsize 操作数
> 或 `field_width`。class 固定的槽 → 固定宽度；class 省略的槽（如 `gprx` 多态）
> → 宽度随分配到的寄存器。

## `[[forms]]` — 编码形式（可选预设）

形式 = 语义键组合（结构化配置，实现为生成器内部函数；开放可扩展集合）。

**S3 起 form 是可选预设**：`[[forms]]` 与 `[[instructions]]` **共用同一组编码键**
（`EncKeys`：`modrm`/`modrm_fixed`/`rex`/`vex`/`evex`/`prefix`/`opsize`/`rex_w`/
`opcode_reg`/`imm`/`escape`/`opcode_field`/`operand_fields`），指令可逐键覆盖
（`EncKeys::over`，指令优先），`form` 本身可省略——组合不再需要预先命名。

```toml
[[forms]]
name = "MRR"                   # @modrm：reg=op0, rm=op1, opsize auto
modrm = { reg = "dst", rm = "src" }

[[forms]]
name = "MRR_0F"                # @modrm + 0F escape
modrm = { reg = "dst", rm = "src" }
escape = [0x0F]

[[forms]]
name = "VEX_RRV"               # AVX 三操作数：reg=dest、rm=src2、vvvv=~src1
vex = { map = "field", pp = "field", w = "field", l = "field" }
modrm = { reg = "dst", rm = "src2" }

[[forms]]
name = "R"                     # riscv R-type（定宽）
opcode_field = "opcode"
operand_fields = ["rd", "rs1", "rs2"]
```

`modrm` 的显式映射（S3）：`modrm = { reg = <操作数名 | 固定扩展码>, rm = <操作数名> }`
——哪个命名操作数进 `reg` 字段、哪个进 `rm` 字段直接写出来。`reg = 3` = 固定扩展码
（取代 v14 的 `"ext"` + `fields.ext` 两处声明）；`rm = "[名字]"` = 内存形式
（mod≠11，与 asm 里 `[{base}]` 同形）。两种内存风味由 `rm` 引用的槽 kind 区分：
`mem` 槽带 base/disp/index/scale、`reg` 槽仅 `[base]`（disp 恒 0）。取代 v14 的
六个魔法串（`rr`/`rr_rev`/`rr_src2`/`ext`/`rm_mem`/`rm_memref`）——它们是**位置
隐含**的：同一个 `"rr"` 在 ADD_RM_R 里 reg=源、在 MOV_R_RM 里 reg=目的。

> **S1 删除 `opcode_bytes`**（3 份谱 58 处，codegen 引用 0）；**S3 删除
> `forms.operand_slots`**。x86 forms 47 → 19；37 条指令直接内联编码键（不再有
> `MRR_0F_NOOS_MEM` 与 `MRR_MEM_0F_NOOS` 这种把 4 个事实编进名字、且只差
> `rex_w` 的并存命名）。

`opsize` 单位统一为**位**（S3）：`opsize = 64`（位宽整数）、`opsize = "dst"`
（命名操作数的寄存器宽度）、`opsize = "max"`（全部 Reg 操作数的最大宽度）。
v14 的 `"r8"` 是字节单位（= 64 位）、与 `[conventions]` 的位单位矛盾，旧写法
现在报错并给出迁移提示；`"s<N>"`（第 N 个操作数）仍可解析但已被命名形态取代。
`opsize` 省略 → 唯一的 `out`/`inout` 操作数；无或多个则 `ops[0]` 并要求显式。

## `[[instructions]]` — 指令

```toml
[[instructions]]
name = "MOV_R_RM"              # 指令名（PascalCase；生成 Inst 枚举变体）
form = "MRR"                   # 可选；省略则全部编码键内联在本指令
opcode = 0x8B
ops = ["dst:gprx:out", "src:gprx"]        # 命名操作数声明（数组序 = 编码序）
modrm = { reg = "dst", rm = "src" }       # 指令级覆盖 form（见下）
asm = "movrr {dst}, {src}"     # 必填；只用 {名字} 引用，不声明
effect = ["Move"]              # 语义标签（见下）

[[instructions]]
name = "ADD_RM_R"              # 源在前、目的在后的编码序
ops = ["src:gprx", "dst:gprx:inout"]
modrm = { reg = "src", rm = "dst" }
asm = "add {dst}, {src}"

[[instructions]]
name = "ADD_R_IMM32"           # 固定扩展码 /0（无操作数占 reg 字段）
ops = ["dst:gpr:inout", "imm:imm32"]
modrm = { reg = 0, rm = "dst" }
asm = "add {dst}, {imm}"

[[instructions]]
name = "ADD"                   # riscv R-type（定宽位域绑定）
form = "R"
opcode = 0x33
fields = { funct3 = 0, funct7 = 0 }   # 固定字段值，按位域名引用
ops = ["rd:gpr:out", "rs1:gpr", "rs2:gpr"]
asm = "add {rd}, {rs1}, {rs2}"        # 操作数按 form.operand_fields 位置绑定位域
effect = ["Pure"]

[[instructions]]
name = "CQO"                   # 隐式寄存器破坏声明（物理寄存器名）
form = "NOOP_REXW"
opcode = 0x99
implicit_regs = ["RDX"]        # cqo 写 RDX：regalloc 据此避开

[[instructions]]
name = "RET"                   # 语义角色声明（见下）
roles = ["ret"]
asm = "ret"

[[instructions]]
name = "MOVABS_GLOBAL"         # 全局地址重定位语义
global_reloc = "abs8"          # abs8 / pcrel_hi / pcrel_lo
asm = "movabs {0}, {1}"
```

**命名操作数**（S3）：`ops = ["src:gprx", "dst:gprx:inout"]` **数组序 = 编码序**
（modrm reg/rm、定宽位域绑定都按这个序；角色缺省 `in`），`asm` 只用 `{名字}`
**引用**——声明与打印彻底分离。`opsize` 因此能写 `opsize = "dst"`（自解释），
取代 `"s1"` 这种"读者无法判断指哪个"的位置引用（v14 那个把 64 位指针截成 32 位的
bug 就出在 `s0` 恰好是**源**）。v14 的 asm 内联声明 `{i:[槽:角色]}` **已删除**，
无兼容层。`collect_inst_infos` 把 `{名字}` 规范化成 `{序号}` 后交给下游，生成器
（asm/machine/decode）只认索引、不感知命名。

**指令级编码键覆盖**（S3）：指令 `#[serde(flatten)]` 直接内联 `EncKeys`，逐键压过
`form` 预设。例如 `form = "MRR"` 但某条需要 `rex_w = "always"`——直接写在本指令，
不必新起一个 `MRR_FIX64` form 名。

**`effect` 语义标签**：`Instruction.effect: Vec<String>`，值域 `Pure` / `Read` /
`Write` / `Branch` / `Jump` / `Call` / `Ret` / `Trap` / `Move`（枚举，未知标签
serde 报错）。`MachineInst::is_branch/is_call/is_ret/is_move` 与 `effects()`
**全部从 effect 标签派生**——生成器**不以指令名作判断依据**。`Move` = 纯 reg→reg
copy（regalloc coalesce 依据）；`Trap` = 陷阱（ud2/ebreak）；缺省 `Pure`。

**`roles` 语义角色**（S4）：指令声明自己担任的 ABI/帧/参数角色，生成器按角色查
指令，取代 `[abi]` 的 13 个 `*_inst` 名指针与 v14 的 `tags` 字符串标签。每个角色
全 ISA 唯一（validate 强制）。完整角色枚举：

| 角色 | 语义 |
| --- | --- |
| `gpr_mov` | 整数寄存器移动（收参 / Copy / 溢出前搬运） |
| `ret_mov` | 返回值 → 返回寄存器移动 |
| `fpr_mov_f64` / `fpr_mov_f32` | f64 / f32 标量寄存器移动 |
| `vec_mov` | ≤16B 向量按值全宽移动（x86 MOVAPS） |
| `call` / `call_indirect` | 直接 / 间接调用 |
| `ret` / `jump` / `branch` | 返回 / 无条件跳转 / 条件分支 |
| `test` | 条件测试（Branch 的 test-cond 序列） |
| `push` / `pop` | 硬件 push / pop（callee-saved 保存；缺则回退 `[spill.*]`） |
| `frame_alloc` / `frame_free` | 帧分配 / 释放（`@frame_alloc`/`@frame_free`） |
| `epilogue_jump` | 尾声跳转（缺省用 `jump`） |
| `wide_vec_store_32` / `wide_vec_store_64` | 宽向量 by-ref 调用方栈拷贝 store |
| `wide_vec_load_32` / `wide_vec_load_64` | 宽向量 by-ref/sret 收参与回读 load |
| `frame_addr` | 帧内 `[FP+disp]` 地址计算（sret/by-ref 临时槽） |
| `stack_arg_load` / `stack_arg_store` | 栈参数收参 load / 传参 store |

角色缺失 → 明确的 `Unsupported("<角色> 未声明")`，不再静默去查一个别的 ISA 的
指令名（v14 靠 `unwrap_or_else(|| "MOV_RM8_R64")` 兜底，共 16 处 x86 硬编码）。

**`global_reloc`**（枚举）：`abs8`（x86 MOVABS_GLOBAL：imm 槽 <0 编码 GlobalId
→ ABS8 `"G{id}"`）、`pcrel_hi`/`pcrel_lo`（riscv AUIPC/ADDI 的 PC-relative hi20/
lo12）。生成器按此字段生成 encoder reloc arm，替代按指令名特判。

**asm 占位符语法**：有 `ops` 时 `{名字}` 引用；助记符 = asm 首词（唯一事实来源，
无独立 `mnemonic` 字段）。`implicit_regs` 是显式隐式寄存器声明（x86 shift 用 CL、
idiv 用 RAX:RDX、cqo 写 RDX），生成的 `MachineInst::clobbers()` 供 regalloc 避开。

## `[[families]]` — 参数化指令族

```toml
[[families]]
name = "MOVSD_M"                # 3 操作数指令族（mnemonic 由变体名小写生成）
form = "MRR_0F_FIX64"
ops = ["dst:fpr:out", "src:mem"]      # 家族共享命名操作数（同 Instruction.ops）
modrm = { reg = "dst", rm = "[src]" } # 家族共享编码键覆盖（引用 ops 的名字）
asm = "movsd {dst}, {1}"        # 变体 asm 可引用 `{name}` 占位符继承族模板
[[families.variants]]
name = "MOVSD_RR"               # 变体名小写 = 助记符（movsd_rr）；同名不同形状按操作数签名消歧
opcode = 0x10
roles = ["fpr_mov_f64"]         # 变体级语义角色（家族里个别变体担任 ABI 角色）
```

族展开 = 每个 variant 生成一条完整指令；`{name}` 占位符（族模板内）= 变体名小写，
实现共享 asm 的助记符继承。家族共享 `ops`/`enc` 键（`modrm` 引用 `ops` 的名字），
故可放在家族层；`roles` 一般在变体级声明。

## 结构化谓词

纯 TOML 数据的小型谓词语言（无字符串）：`and` / `or` / `not` / `eq` / `ne` /
`lt` / `le` / `gt` / `ge` / `in`，操作数属性引用，求值实现见
`crates/frontend/forge-dsl/src/v12/pred.rs`（lowering 经 `gen_lowering_attrs`
求值，谓词 false → 该规则不匹配，落入下一条或 Unsupported）。`in`（S2）=
集合成员：`in = [attr, [v1, v2, ...]]`。

可用属性（lowering 时由 IR 层求值）：

| 属性 | 语义 |
| --- | --- |
| `rd` / `rs1_width` / `rs2_width` | 结果/操作数 1/2 的宽度（标量 = `bits()`；向量 = `size_bytes()*8`） |
| `rd_vec` / `rs1_vec` | 结果/操作数 1 是向量时为字节数（`Some(size_bytes)`），标量无此属性 |
| `elem` | 元素类型 id（F32=1、F64=2、I32=3、I64=4、I8=5、I16=6；向量经 `element_type()`） |
| `cond` | 比较条件 id（Fcmp→fcmp_id、Icmp→icmp_id；setcc/cmovcc 用） |
| `imm0` | `current_immediates[0]`（Vextract/Vinsert/Vsplit 的 lane 索引；**AtomicRmw 的 op 判别值**：Xchg=0/Add=1/Sub=2/...） |

## `[[lowering]]` — 指令选择

```toml
[[lowering]]
op = "Iadd"
insts = ["movrr {out}, {0}", "add {1}, {out}"]
```

**指令用助记符引用**：`insts` 里的指令名 = 该指令 asm 的**首词**（助记符，唯一
事实来源），不再用指令名（PascalCase）。同助记符多形状（如 `add reg,reg` 与
`add reg,imm`）由生成器按**操作数 token 签名**自动消歧（reg/imm/mem/cond/label；
`[n]` 剥括号为内存基址=reg；数字字面量兼容 imm/cond）；仍多候选时再按**字面段
一致性**（模板 vs asm 的内存括号）过滤。

**宽度分派**（`when` 谓词按 IR 类型选 32/64 位指令）：i32 操作数须用 32 位指令，
否则 64 位 load/store 会读写相邻 i32 栈槽（槽距 4 字节）——Load/Store/Icmp 均按
`rd`/`rs1_width` 分派：

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

**`vary` 参数化行表**（S2）：各列表**等长**，按下标 zip 成行展开成多条具体规则。
键在 `PRED_ATTRS` 里（`rd`/`rs1_width`/`rs2_width`/`rd_vec`/`rs1_vec`/`elem`/
`cond`/`imm0`）→ 该行自动追加 `eq = [键, 值]` 到 `when`，**且**可在模板里用
`{键}` 引用；否则是纯替换变量。消除"一个 op 一堆只差助记符的规则"：

```toml
# v14：8 条 Vadd（4 elem × 2 宽度）→ v15：2 条
[[lowering]]
op = "Vadd"
vary = { elem = [1, 2, 3, 4], m = ["vaddps", "vaddpd", "vpaddd", "vpaddq"] }
insts = ["{m} {out}, {0}, {1}"]

[[lowering]]
op = "Vadd"
vary = { elem = [1, 2, 3], m = ["addps", "addpd", "paddd"] }
insts = ["movaps {out}, {0}", "{m} {out}, {1}"]

# Fcmp 32 条（16 cond × 2 elem）→ 4 条
[[lowering]]
op = "Fcmp"
vary = { cond = [1, 2, 4, 7, 8, 11, 14, 15], k = [11, 10, 5, 7, 3, 4, 2, 6] }
insts = ["xor {out}, {out}", "comiss {0}, {1}", "setcc {out}, {k}"]
```

`in` 集合谓词消掉 33 组纯重复：

```toml
[[lowering]]
op = "Cvtf2i"
when = { and = [ { eq = ["rs1_width", 64] }, { in = ["elem", [3, 4]] } ] }
insts = ["cvttsd2si {out}, {0}"]
```

**特异性排序**（S2，去掉声明序依赖）：候选按 `(priority 降, 谓词叶子数降, 声明序
升)` 裁决（见 `V12Model::lowering_by_op`）。`priority`（可选，缺省 0）只在"故意让
更宽的规则赢"时用（x86 `Vextract` 的 lane 0 快路径）。编译期报两类错：**重复**
（同 op 且规范化后谓词相同）；**死规则**（谓词被更靠前且更宽的规则完全包含，判定
域 = 每属性闭区间集合，含 `or`/`not` 记为 Opaque 跳过）。

符号化操作数（每条 `insts` 是直线指令序列，同 op 多规则用 `when` 谓词 if-else-if
链分派，无 when 的规则 = 无条件兜底）：

| token | 语义 |
| --- | --- |
| `{out}` / `{out2}` | 结果（out2 = 第二结果，如溢出 flag；rd2 预绑定） |
| `{N}` | 第 N+1 个 IR 操作数（`{0}`→rs1、`{N}`→rs{N+1}）——**动态编号，任意上限**，3+ 操作数指令照常支持（lowering 预绑定 rs1..=rsN 按模板实际最大编号生成） |
| `{g}`、`{gN}` | 临时 GPR XReg（模板级预分配，变量 `__g`/`__gN`） |
| `{f}`、`{fN}` | 临时 FPR XReg（变量 `__f`/`__fN`） |
| `{iconst}` / `{fconst}` | 常量池整数/浮点位模式（`current_const_index`） |
| `{off}` | `ctx.current_offset`（StackAddr 帧偏移，已平移 callee_saved） |
| `{alloca}` | `ctx.current_alloca_offset`（Alloca 栈槽偏移） |
| `{imm0}` / `{imm0_sub4}` | `current_immediates[0]`（lane 索引；sub4 = 值-4） |
| `{shufps_imm8}` / `{shufps_imm8_hi}` | SHUFPS mask 常量（低/高 2 位字段） |
| `{vconst_lo2}` / `{vconst_lo}` / `{vconst_hi}` / `{vconst_lo_hi}` / `{vconst_hi_hi}` | 向量常量字节池拆分（V64/V128/V256 lane 位模式） |
| `{cc}` | Icmp 条件码（`__cc`） |
| `{global}` | `ctx.current_global` 的 GlobalId（**负编码 -(id+1)**，配 `MOVABS_GLOBAL` → ABS8 重定位） |

> **占位符单一注册表**：所有占位符的 token 分类、临时声明、xreg 绑定、ctor 表达式
> 统一收敛于 `v12/codegen/placeholder.rs`——**新增占位符只改注册表一处**。临时命名
> `g`/`f` 与 `PhTemp::{Gpr,Fpr}` 一一对应，且与既有 token 无前缀冲突（`{g}`≠
> `{global}`、`{f}`≠`{fconst}`，精确匹配）。旧样式 `{t}`/`{tN}`/`{t_f}`/`{t_fN}`
> 已废弃（TOML 全量迁移，解析即报错）。

物理寄存器名（`RAX`/`RCX`/`XMM0` 等）直接写死为 `Reg::*` 并自动收集为 clobber
（regalloc 本点避开）；`opsize` 槽缺省 `ctx.default_opsize`（按 IR 类型 32/64）；
数字立即数/条件码直接写（如 `ROUNDSD_I {f1}, {f1}, 3`）。

**符号重定位（JIT）**：

- **Call**：`CALL_RIP_REL` 的 Label 槽 `rel<0` 编码函数符号 `-(FuncRef+1)` →
  encoder 转 `add_reloc(CALL, "@N")`（JIT 符号表按 FuncRef 序注册）；`rel>=0` 为
  块内 label（REL4）。
- **GlobalAddr**：`MOVABS_GLOBAL` 的 imm 槽 `<0` 编码 GlobalId `-(id+1)` →
  encoder 转 `add_reloc(ABS8, "G{id}")`（JIT 编译模块时按全局变量注册数据段符号
  并即时 patch）；imm≥0 为普通 64 位立即数。

## `[[pattern]]` — 树型多指令匹配

**S6 新增能力（opt-in）**：声明一棵 IR **匹配树**与一段发射序列。运行期 lowering
驱动在块内逆序预扫时，把命中整棵子树的 IR 值合并成一次 `lower_pattern` 发射（叶
变量按 DFS 序绑定为 `{N}` 输入），跳过被 consumed 的内部节点。**无 `[[pattern]]`
的 ISA 生成空匹配器 → 零行为变化、零开销**。

```toml
[[pattern]]
match = "Fadd(Fmul(a, b), c)"          # 匹配树：根必须是 Op 调用，变量是裸标识符
when  = { eq = ["elem", 1] }           # 根指令派生属性上的结构化谓词（同 lowering.when）
insts = ["movss {out}, {a}", "mulss {out}, {b}", "addss {out}, {c}"]

[[pattern]]
match = "Fadd(Fmul(a, b), c)"
when  = { eq = ["elem", 2] }
insts = ["movsd {out}, {a}", "mulsd {out}, {b}", "addsd {out}, {c}"]
```

- **`match`**：递归下降小解析器（`v12/match_tree.rs`）解析 `Op(a, Op(b, c))`，
  根必须是 Op 调用，叶子是裸标识符（变量）。`when` 与 `[[lowering]].when` 同语法，
  两个结构相同、只差守卫（如 f32 vs f64）的模式靠它区分。
- **叶变量 DFS 序**：`insts` 里 `{a}`/`{b}`/`{c}` 按树 DFS 序编号为 `{0}`/`{1}`/
  `{2}`（codegen 改写后交给 lowering 发射器）。`{out}` = 结果。
- **内部节点单 use**：匹配要求内部节点（如上例的 `Fmul`）**单 use 且同块**——防止
  把一个还被别处引用的值错误融合掉。命中后整棵子树进 consumed 集合，前向主循环
  `continue` 跳过这些已发射的指令。
- **裁决序**：块内逆序预扫，模式按（节点数降序）试匹配，最大树优先；`when` 对根
  指令的派生属性求值（与 `gen_lowering_attrs` 的 `__attr` 同语义）。
- **发射**：`TargetLowering::lower_pattern(name, &leaf_xregs, &results, ctx)`，
  复用现有 XReg/InstPacket 全管线，不改 IR、不 clone Function。默认实现返回
  Unsupported（不声明 `[[pattern]]` 的 ISA 不生成该方法体）。

上例把 `Fadd(Fmul(a,b),c)` 融合成 3 条（movss/mulss/addss），比逐条 lowering
（Fmul 2 条 + Fadd 2 条 = 4 条，且各带一条 mov 拷贝）省 1 条 mov。

## `[abi]` — 调用约定

```toml
[abi]
stack_align = 16
frame_padding = 8              # 帧额外栈填充（x86 = 8；见下）
stack_arg_shadow = 32          # Windows x64 shadow space（Some 启用栈参数；None 不支持）
arg_slot = "by-position"       # 参数槽位计数策略：by-class（缺省，riscv）/ by-position（x86）
scratch = ["R10", "R11"]       # spill load/store 专用（须排除 allocatable）
ret_regs = ["X10"]             # 返回寄存器（缺省空 = index 0，x86 RAX 语义）
call_ret_reg = "X1"            # Call 的返回地址寄存器（缺省 "X1"=riscv ra）
call_clobbers = ["X1", "X7", ...]  # Call 点被调用方破坏的寄存器
reserved = ["X0", "X1", "X3", "X4"]  # regalloc 不可分配寄存器

[[abi.arg_class]]
class = "int"                  # int / float / vector / other
regs = ["RCX", "RDX", "R8", "R9"]

[[abi.arg_class]]
class = "vector"
strategy = "by-ref"            # 超过 limit 位按引用传参（YMM ABI）
limit = 128
```

> **S4 删除全部 `*_inst` 名指针**：v14 的 `move_inst`/`ret_mov_inst`/`call_inst`/
> `call_indirect_inst`/`ret_inst`/`jump_inst`/`branch_inst`/`test_inst`/
> `push_inst`/`pop_inst`/`fpr_mov_inst`/`fpr_mov_inst32`/`vec_mov_inst` 全部移除，
> 改为指令上的 [`roles`](#instructions--指令)。生成器查角色表取代按名字查找，缺角色
> → 明确 `Unsupported("<角色> 未声明")`。寄存器名类键（`ret_regs`/`call_ret_reg`/
> `scratch`/`reserved`/`call_clobbers`/`callee_saved`）留在 `[abi]`——它们是寄存器
> 不是指令。

- `arg_slot`（枚举）：`by-class`（缺省，riscv SysV——int/float 各自独立推进）/
  `by-position`（Windows x64——int/float 共享位置计数，参数 i 用 GPR{i}/XMM{i}）。
- `frame_padding`：prologue push rbp + callee-saved 后 rsp%16==8，sub rsp 需使
  call 前 rsp%16==0（Windows x64 ABI，缺省 0 会让系统 DLL 在未对齐栈上 SEGV）。
- `stack_arg_shadow`：第 5+ 参数（寄存器耗尽后）由调用方 store 到
  `[rsp+n+(k-nregs)*8]`、被调方从 `[rbp+n+8+(k-nregs)*8]` load。None = 不支持栈
  参数（超寄存器参数 → Unsupported）。
- `call_clobbers`：Call 点被调用方破坏的寄存器。缺省 = 整数参数寄存器 + 返回寄存
  器。**定宽 ISA 无 callee-saved 保存序列时须列全 caller-saved**，否则跨调用存活
  值留在寄存器被覆盖（实测递归 fib 死循环）。s 系（@push_callee 保存）不在列表。
- `reserved`：regalloc 不可分配寄存器（riscv X0=zero 写入无效、X1=ra 被
  prologue/call 占用、X3/X4=gp/tp）——不排除会分配出垃圾（实测 `subw x0`）。

## `[abi.frame]` — 帧布局

```toml
[abi.frame]
sp = "X2"                  # 栈指针寄存器名
fp = "X8"                  # 帧指针寄存器名（None = 无帧指针）
layout = "fp-inside"       # 帧布局模式：fp-outside（缺省，x86/demo）/ fp-inside（riscv）
fp_push_bytes = 16         # prologue 在帧指针上方 push 的字节数（x86 = 8）
alloc_neg = true           # @frame_alloc 立即数取负（riscv addi sp,sp,-N；x86 SUB 语义不需要）
```

**`layout` 模式**（S5）：决定 callee-saved 保存槽相对帧的位置，其余帧数值全部由
运行期推导（`pipeline/frame_layout.rs::frame_layout_info`），不再手写魔法数：

- `fp-outside`（缺省，x86/demo）：callee-saved 用硬件 push 在帧指针**上方**（帧外）。
  spill 槽 sp_base = -(frame) - callee_saved_bytes、栈槽基准 fp - callee_saved_bytes。
- `fp-inside`（riscv）：ra/fp/callee-saved 保存槽在帧**内顶部**（@push_callee 的 SD
  到 `[sp+frame-fp_push-(k+1)*8]`，帧分配覆盖到固定最小帧）。spill 槽
  sp_base = -(frame)（帧内底部）、栈槽平移 = fp_push。

> **S5 删除** `min_frame_bytes`/`callee_saved_bytes_override`/`stack_slot_shift`
> 三个纯 riscv 旋钮——它们全部可从"`layout` + `fp_push_bytes` + callee_saved 表"
> 推导（已核对现值：fp-inside → 104 = 16 + 11×8、override 0、shift 16）。`alloc_neg`
> 保留为布尔（帧分配/释放**指令**已随 S4 移到 `roles` 的 `frame_alloc`/`frame_free`）。

## `[emit]` — 序言/尾声

```toml
[emit]
align_pad = 0x90            # `.align` 伪指令填充字节（缺省 0x00）
epilogue_label = true       # 是否生成独立尾声标签 + return block 的 epilogue 跳转
                            # （缺省 true；定宽 ISA 无 JMP 可设 false 走 fall-through）

[emit.prologue]
insts = ["PUSH RBP", "MOV64_RR RSP, RBP", "@push_callee", "@move_args", "@frame_alloc"]

[emit.epilogue]
insts = ["MOV64_RR RBP, RSP", "SUB64_R_IMM32 RSP, {callee_saved_bytes}", "@pop_callee", "POP RBP", "RET"]
```

`@push_callee` / `@frame_alloc` / `@frame_dealloc` / `@pop_callee` 为伪指令，由
FrameLowering 展开为具体序列（`@frame_alloc`/`@frame_free` 的指令由 `roles` 的
`frame_alloc`/`frame_free` 提供）。

占位符：

| 占位符 | 语义 |
| --- | --- |
| `{frame_size}` | 运行时 `frame_size`（emit 模式） |
| `{frame_size_neg}` | `-frame_size` |
| `{frame_size_mN}` | `frame_size - N`（如 riscv 的 `SD X1, X2, {frame_size_m8}` 保存 ra 到帧顶） |
| `{callee_saved_bytes}` | **S5 新增**：callee-saved 区字节数（`fp_overhead + Σcallee_saved×宽`）——替掉 x86 尾声里的魔法数 `56`（7×8） |

## `[spill.*]` — 溢出模板

callee-saved 保存/恢复与寄存器溢出用的 load/store 指令 + 基址寄存器。两种形态：

```toml
# x86 式（Mem 槽）：
[spill.GPR]
load = "MOV64_RM {0}, {1}"     # {0}=目标寄存器、{1}=MemRef{base, __off}
store = "MOV64_MR {0}, {1}"    # {0}=源寄存器、{1}=MemRef{base, __off}
base = "RBP"

# riscv 式三操作数（reg + reg + imm，无 Mem 槽）：
[spill.GPR]
load = "LD {0}, X8, {1}"       # {0}=目标寄存器、字面 X8=基址、{1}=__off 立即数
store = "SD {0}, X8, {1}"
base = "X8"
```

> **spill 模板操作数绑定**：`{N}` 为**任意编号**（不限于 `{0}`/`{1}`——4+ 操作数
> spill 指令照常支持），按槽类型绑定语义：Reg 槽 → `__dst`（load 目标 / store 源）、
> Mem 槽 → MemRef{base, __off}、Imm/Label 槽 → `__off as i64`；字面物理寄存器 →
> 基址（riscv `X8`）；字面立即数 → 固定值。

`base` 的选取是递归安全的关键（见 riscv TOML 注释）：`base = fp`（帧指针）时
emission 的 sp_base = `-(frame) - callee_saved_bytes` 是**相对帧指针**的偏移，spill
槽落在 `[fp - frame + off] = [sp + off]` 帧内底部；`base = sp` 会双重减 frame（spill
槽落帧外更深处），递归时与 callee 帧重叠（ra 槽被覆盖 → ret 跳 0，实测 fault_fetch
epc=0）。

## asm 模板

asm 完整格式：**首词 = mnemonic**，后续为逗号分隔操作数；支持 `{0}`/`{1}` 位置
占位（无 `ops` 的旧式）与 `{名字}` 命名占位（有 `ops`），以及通用模板段
`Seg = Lit | Op`（mnemonic 与模板分离）。操作数形状：simple（寄存器/立即数）、
memory（`{I}({J})` 基址+位移，如 `8(X2)`）、memory0（`({J})`）。寄存器名大小写
不敏感（`xmm0` / `XMM0` 等价）。

## 代码生成输出

`pub mod <file_stem>`（生成模块名 = 文件 stem，非 meta.name）：

- **自包含部分**：`Reg` 枚举、`Inst` 枚举、`encode(&Inst) -> Result<Vec<u8>, String>`、
  `decode(&[u8]) -> Option<(Inst, usize)>`、`disassemble(&Inst) -> String`、
  `assemble(&str) -> Result<Inst, String>`——仅依赖 std。
- **Inst 字段类型化**：寄存器操作数为 `Reg` 枚举（`MovRRm { dest: Reg, src: Reg,
  opsize: u8 }`、riscv `Add { rd: Reg, rs1: Reg, rs2: Reg }`），opsize → `u8`、
  cond → `u8`、mem → `MemRef`、imm/label → `i64`；字段名按操作数语义生成
  （out→dest/rd、in→src/rs1/rs2、cond→cond、rel→target 等），不再有裸 `u32` 位置
  字段（op0/op1/op2）。regalloc 经 `Reg::from_index(phys, <槽位 RegClass>)` 回填。
- **集成层**：`TargetMachine`（组合 `IsaInfo`/`RegInfo`/`ABI`/`Lowering`/`Encoder`/
  `FrameLowering`/`Disassembler`/`Assembler`/`Decoder`）、`MachineInst`（uses/defs/
  reg_field/set_reg_field/effects/branch_targets/clobbers）、`ensure_registered()`
  （注册 Registry + reloc patcher）。
- **S6 起**：`TargetLowering` 增加 `patterns()`（返回 `__PATTERNS` 静态切片，无
  `[[pattern]]` 时为空）与 `lower_pattern(name, leaves, results, ctx)`（默认
  Unsupported）。**已删除** v14 的 `TargetMachine::pattern_matcher()` 挂点与
  `ext/pattern_isel.rs`（476 行死模块）。

### 汇编器能力（`TargetAssembler::parse_insts`）

- **`.equ name, expr`**：符号常量（顺序求值，前向引用失败；指令立即数表达式可引用）。
- **立即数表达式**：`+ - * / % << >> & | ^ ~ ( )` 递归下降求值；label 槽（分支目标）
  同样支持表达式。
- **数据伪指令**：`.word`/`.hword`/`.dword`（按 meta.endian 写多字节）、
  `.ascii "..."`/`.asciz "..."`（`\n \t \r \" \\ \0` 转义）、`.zero n`。
- **`.macro name params` / `.endm`**：文本宏（参数引用 `\arg` 或 `%arg`；嵌套宏
  就地展开，深度上限 16 轮 / 64 行防递归）。
- **结构化错误带行号**：`line N:` 前缀（ParseError/UndefinedLabel/Other/Ambiguous/
  TypeMismatch 均带）。

### 解码器能力

- **decode 错误带部分匹配偏移**：`TargetDecoder::decode` 失败返回
  `DecodeError::InvalidBytes(n)`，n = 已消费的前缀字节数（x86 的 66/F0/F2/F3/REX
  前缀；定宽 ISA = 0）。生成 `decode_partial` 内部函数。
- **大端变长 imm**：`imm_read_ts` 按 `[meta].endian` 装配（little → `from_le_bytes`、
  big → `from_be_bytes`）。

## 已有 ISA 文件

- **`isa/x86_v12.toml`**：140 条 `[[instructions]]` + 51 条 families 变体 + 2 条
  `[[pattern]]`，19 个 form 预设，208 条 lowering。变长语义键，接 TargetMachine；
  jit 矩阵 195 passed / 3 skipped / 0 failed。
- **`isa/riscv64_v12.toml`**：117 条指令，定宽试点（QEMU 真执行验证）；jit 矩阵
  131 passed / 67 skipped / 0 failed。`[abi.frame] layout = "fp-inside"` 全推导。
- **`isa/demo_v12.toml`**：同助记符多宽度自动分发演示基线。
