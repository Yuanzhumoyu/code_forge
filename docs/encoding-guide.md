# 编码指南 — `encoding = "..."` bitstring 语法

本指南描述当前 ISA TOML 的指令编码声明方式：`[inst.X]` 下的单行字符串
`encoding = "..."`。旧的表格式编码（`[inst.X.encoding]` 的
`format = "bits"/"segments"` + `bitfields`/`segments` 数组）与旧的手写
`[inst.X.emit]` 模板**均已删除**（见文末[迁移指南](#迁移指南)）。

解析器位于 `crates/frontend/forge-dsl/src/bitstring.rs`，TOML 模型在
`crates/frontend/forge-dsl/src/model.rs`，生成的发射函数 `emit_inst` 在
`crates/frontend/forge-dsl/src/codegen/mod.rs`。

## 目录

1. [语法总览](#语法总览)
2. [位域](#位域)
3. [三种编码形态](#三种编码形态)
4. [条件段](#条件段-cond)
5. [Fixup 标签占位](#fixup-标签占位)
6. [编码原语速查表](#编码原语速查表)
7. [用户宏与散射](#用户宏-macro-与散射)
8. [运行期原语](#运行期原语)
9. [实例解析](#实例解析)
10. [迁移指南](#迁移指南)

---

## 语法总览

一条 `encoding` 字符串由**三种形态之一**构成（`parse_encoding` 按首字符判定，
bitstring.rs:224-244）：

| 形态 | 首字符 | 示例 |
|------|--------|------|
| 定宽 Fixed | 数字（宽度前缀） | `"32 {dest:7:5}{0x33:0:7}"` |
| 分段 Segmented | `{` / `?` / `!` / `@` | `"?cond {…}{…} {…}"`、`"{0xE9:[0;8]} !rel4"` |
| 原语 Primitive | `@` | `"@modrm opsize 0x8B dest src"` |

字符串解析前会先做文本预处理（`expand_encoding`，bitstring.rs:659-673）：

1. 剥离行内 `#` 注释（`#` 前面是空白或在行首时才剥离，bitstring.rs:678-695）；
2. 展开 `$NAME` 常量（`[enc_constants]`，bitstring.rs:698-706）；
3. 展开 `$macro` 调用（`[enc_macros.*]`，bitstring.rs:709-799）；
4. 展开 `{field:scatter}` 散射引用（`[enc_scatters.*]`，bitstring.rs:801-872）；
5. 迭代至不再变化（上限 16 轮，防无限递归）。

三种形态的解析结果分别是 `ParsedEncoding::Fixed { width, fields, fixup }`、
`ParsedEncoding::Segmented { segments }`、`ParsedEncoding::Primitive { name, args }`
（bitstring.rs:74-87）。

---

## 位域

位域是编码字符串的基本构件（`{VALUE:OFFSET:WIDTH}`，三种写法见 bitstring.rs:10-16、
parse_bit_spec bitstring.rs:574-618）：

```text
{VALUE:OFFSET:WIDTH}           基本形式：值放在 [offset, offset+width) 位区间
{VALUE:[OFFSET;WIDTH]}         方括号形式（等价）
{VALUE:[OFFSET;WIDTH;SHIFT]}   方括号形式 + 位提取右移
```

| 元素 | 含义 | 示例 |
|------|------|------|
| `VALUE` | 字段名、十六进制（`0xNN`/`0XNN`）、二进制（`0bNNNN`/`0BNNNN`）或十进制（含负数） | `dest`、`0x8B`、`0b000101`、`31` |
| `OFFSET` | 起始位偏移，0 = LSB | `7` |
| `WIDTH` | 位宽 | `5` |
| `SHIFT` | 可选：放入位域前先右移的位数（**仅方括号形式支持**） | `3` |

- 一个字段名可在同一条编码中出现多次（例如 RISC-V SD 的 `offset` 拆成
  `{offset:[7;5;5]}` 与 `{offset:[25;7]}` 两段，riscv64_v10.toml:378）。
- 十六进制、二进制、十进制字面量是常量；二进制按十六进制值存储
  （`0b000101` = `0x05`，parse_value bitstring.rs:540-547）。
- 负数字面量允许（如 `-1`，解析为 `Dec`）。
- 位域之间不允许重叠：`check_overlap`（bitstring.rs:880-898）会在重叠时报错。

### 字段解析规则

`VALUE` 是字段名时按字段类型解析（`bitfield_to_tokens`，bitstring.rs:1979-2017；
`resolve_condition`，bitstring.rs:1119-1161）：

- **寄存器字段**（`Ireg`/`Freg`）：生成 `field.to_index()`（`forge_ir::PhysReg`
  trait，返回物理寄存器编号 u32）。`Ireg`/`Freg`/`GprReg`/`XmmReg` 四个字段类型
  在生成的 `Inst` 枚举里都是 `Reg` 类型（codegen/mod.rs:473-475），`Reg` 是
  `#[repr(u32)]` 且按索引序生成变体的物理寄存器枚举（codegen/mod.rs:402-435），
  因此 `to_index()`（Ireg/Freg 路径）与直接 `as u64`（其余路径）都得到
  物理寄存器编号。
- **非寄存器字段**（`i64` 立即数、`Opsize`、`CondCode` 等）：生成 `*field as u64`
  —— `emit_inst` 以 `&Inst` 匹配，字段是引用，故先解引用再转型。
- **未声明的字段名**：编译错误 `bitstring references undeclared field '...'`
  （bitstring.rs:2000）。
- 条件表达式里的非寄存器字段解析为 `(*field as i64)`（bitstring.rs:1152）。

---

## 三种编码形态

### 1. 定宽 Fixed

以数字宽度前缀开头，宽度通常为 8/16/32/64（`pack_bits` 支持的值），后跟连续的
位域组，可选尾随 `!fixup`：

```text
32 {dest:7:5}{0:12:3}{src1:15:5}{src2:20:5}{0:25:7}{0x33:0:7}
```

这是 RISC-V ADD 的 R 型编码（bitstring.rs 测试 test_fixed_width_rv，2028-2042）。
AArch64 ADD 同理（test_aarch64_add，2140-2150）：

```text
32 {dest:0:5}{src1:5:5}{src2:16:5}{0x22C:21:10}{1:31:1}
```

真实 ISA 示例（aarch64_v10.toml:106，ORR Xd, XZR, Xm = mov）：

```toml
[inst.SD_MOV]
fields = { dest = "Ireg", src = "Ireg" }
encoding = "32 {dest:[0;5]}{src:[5;5]}{31:[16;5]}{0x22C:[21;10]}{1:[31;1]}"
```

定宽形态最终生成一次 `pack_bits(sink, width, &[...])` 调用（bitstring.rs:937）；
带 fixup 时改为 `pack_bits` + `sink.use_label_at(...)`（bitstring.rs:949-953）。

### 2. 分段 Segmented

不以数字开头的字符串走分段解析：**空白分隔多个段**，每个段可以是：

| 段形态 | 说明 |
|--------|------|
| `{…}{…}` | 无条件段（连续位域组，段宽度按最大位范围向上取整到 8/16/32/64，`round_up_width` bitstring.rs:643-651） |
| `?cond {…}{…}` | 带内联条件的段 |
| `!fixup` | 独立的 fixup 占位段（宽度 = fixup 字节数） |
| `{…} !fixup` | 段尾 fixup 后缀 |
| `@name arg1 arg2` | 段内联原语调用 |

例如 x86 JMP rel32（x86_v10.toml:921，测试 test_fixup_x86_jmp 2184-2197）：

```text
{0xE9:[0;8]} !rel4
```

两个段：opcode 字节 `0xE9` + 4 字节 fixup 占位。

```text
注意：旧注释中出现的段名写法（如 `[REX]`）不是可解析语法——parse_segmented
只接受 `{`、`?`、`!`、`@` 开头的段，`[` 开头的 token 会报 UnexpectedChar。
条件前缀段请写成 `?cond {…}` 形式。
```

### 3. 原语 Primitive

以 `@name` 开头，后跟空白或逗号分隔的参数（`parse_primitive` 按空白与逗号切分，
bitstring.rs:246-262）：

```text
@modrm opsize 0x8B dest src
@call_reloc offset, 0x94000000
```

参数规则（`gen_primitive`，bitstring.rs:1172-1977）：

- 数字字面量（十进制/`0x`）→ 直接作常量；例如 `@modrm 64 0x63 dest src`。
- 字段名 → 按位置解引用：普通参数 `*field`，寄存器参数 `field.to_index() as u32`
  （`reg_expr`，bitstring.rs:1204-1211）。
- 原语也可以在分段编码中内联（`parse_segmented` 的 `@` 分支，bitstring.rs:334-389），
  wasm32 的宏内大量使用：`@leb128_reg 0x20 dest`。

未知原语名报错：`unknown encoding primitive '@…'`（bitstring.rs:1973-1975）。

---

## 条件段 `?cond`

段前加 `?表达式` 前缀，表达式是使用字段名作为变量的 Rust 布尔表达式，仅当为真时
才发射该段（`resolve_condition`，bitstring.rs:1119-1161）。用于 x86 REX 前缀等
可选字节。

解析示例（bitstring.rs 测试 test_inline_condition，2165-2181）：

```text
?dest>=8||src>=8 {0x4:[0;4]}{1:[3;1]}{dest:[2;1;3]}{0:[1;1]}{src:[0;1;3]} {0x01:[0;8]} {dest:[0;3]}{src:[3;3]}{3:[6;2]}
```

- 段 1：条件 `dest>=8||src>=8`，REX 字节（5 个位域：`0x4`@0:4、`1`@3:1、
  `dest`@2:1 shift 3（REX.R）、`0`@1:1、`src`@0:1 shift 3（REX.B））；
- 段 2：opcode `0x01`（无条件）；
- 段 3：ModRM `dest`@0:3、`src`@3:3、`3`@6:2（无条件）。

解析结果为 3 个段，段 1 的 `condition == Some("dest>=8||src>=8")`、宽度 8、
5 个位域。较早的测试 test_segmented_x86_add（2044-2078）使用了另一种 REX 位布局
（`{0x4:[0;4]}{1:4:1}{dest:[5;1;3]}{src:[7;1;3]}`），两者都是合法写法。

条件解析规则：

- 寄存器字段：预生成 `let __field = field.to_index();`，条件里用 `__field` 替换
  （bitstring.rs:1142-1150）；
- 其他字段：替换为 `(*field as i64)`（bitstring.rs:1151-1153）；
- 整个条件用括号包裹再解析为 Rust TokenStream（bitstring.rs:1156-1160）；
- **相同条件的连续段会合并进同一个 `if` 块**（bitstring.rs:971-1085）；
- 空条件（`?` 后直接 `{`）报错，未知 fixup 报错（测试 2215-2224）。

---

## Fixup 标签占位

`!kind` 声明一个标签 fixup 占位（分支/跳转目标）。所有 fixup 占位宽度均为 4 字节
（`FixupKind::width()`，bitstring.rs:102-108），函数内分支一律按 PC-relative
记录为 `RelocKind::REL4`，ISA 特有的立即数编码（AArch64 BL imm26、RISC-V B/J
立即数重排）在 `finish()` 时由后端的 `RelocPatcher` 应用（`fixup_to_reloc`，
bitstring.rs:1107-1117）。

| 语法 | 占位宽度 | 用途 | 真实示例 |
|------|---------|------|---------|
| `!rel4` | 4 字节 | x86 JMP/JCC rel32 | `{0xE9:[0;8]} !rel4`（x86_v10.toml:921 JMP）、`{0x0F:[0;8]} {cond:[0;8]} !rel4`（x86_v10.toml:953 Jcc） |
| `!isa0` | 4 字节 | AArch64 B/B.cond/BL 26 位偏移 | `32 {0b000101:[26;6]} !isa0`（aarch64_v10.toml:317 B）、`32 {cond:[0;4]}{0b01010100:[24;8]} !isa0`（:324 B.cond）、`32 {cond:[0;5]}{0xB5:[24;8]} !isa0`（:950 CBNZ） |
| `!isa1` | 4 字节 | RISC-V B 型分支 | `32 {src2:20:5}{src1:15:5}{0:12:3}{0x63:0:7} !isa1`（riscv64_v10.toml:400 BEQ） |
| `!isa2` | 4 字节 | RISC-V J 型跳转 | `32 {dest:7:5}{0x6F:0:7} !isa2`（riscv64_v10.toml:388 JAL） |

**要求**：使用 fixup 的指令必须声明一个 `BlockTarget` 类型的字段（指向目标块），
否则报编译错误 `fixup '!…' requires a BlockTarget field`（bitstring.rs:944-946、
1022-1026）。例如：

```toml
[inst.SD_JMP]
fields = { rel = "BlockTarget" }
encoding = "32 {0b000101:[26;6]} !isa0"
```

生成代码在 fixup 位置记录 `sink.use_label_at(offset, Block(*rel as u32), REL4)`
（bitstring.rs:949-953）。

---

## 编码原语速查表

原语（`@name`）把常用编码模式（前缀/REX/ModRM/VEX/LEB128/重定位）固化为参数化
代码生成。`emit_inst` 生成的代码中直接内联展开（bitstring.rs:1171-1977）。

| 原语 | 参数形态 | 说明 | 真实示例 |
|------|---------|------|---------|
| `@modrm` | `opsize opcode reg rm [escape]` | 宽度感知 ModRM 寄存器-寄存器（mod=11b）。opsize=16→`0x66` 前缀；64→REX.W=1；32 位+扩展寄存器→REX.W=0；可选 escape 字节（如 `0x0F`）插在 opcode 前 | `@modrm opsize 0x8B dest src`（x86_v10.toml:251）、`@modrm opsize 0xAF dest src 0x0F`（:297 IMUL） |
| `@modrm_mem` | `opsize opcode reg rm [disp [prefix [escape]]]` | ModRM 内存寻址。base 编码为 100（RSP/R12）→ 自动插 SIB；base ∈ `[meta].modrm_force_disp_base`（x86 的 5/13=RBP/R13）→ 强制带位移（mod=00+rm=101 是 RIP-relative）；disp 可为 0/字面量/字段，mod 按位移大小选 0/1/2。**rm 可为 MemRef 字段**（自动展开 `mem.base`/`mem.offset`，此时无 disp 参数，prefix/escape 从第 4/5 参数读） | `@modrm_mem opsize 0x8B dest base 0`（:1019）、`@modrm_mem 64 0x8B dest mem`（:1073，MemRef 字段） |
| `@op_rm` | `opsize opcode ext rm` | `/digit` 单操作数（reg 位固定为 ext） | `@op_rm opsize 0xF7 2 dest`（:303 NOT） |
| `@modrm_imm32` | `opsize opcode ext rm imm` | `81 /digit` + imm32 | `@modrm_imm32 opsize 0x81 0 dest imm`（:453 ADD r/m, imm32） |
| `@op_rm_imm32` | `opcode ext rm imm` | 固定 64 位 `/digit` + imm32（帧分配） | `@op_rm_imm32 0x81 5 dest imm`（:1141） |
| `@cmovcc` | `opsize cc dest src` | `0F 4{cc} /r` 条件传送 | `@cmovcc opsize cond dest src`（:539） |
| `@sse_rr` | `prefix opcode w reg rm` | SSE 寄存器-寄存器：prefix 非 0 则发（`0x66`/`0xF2`/`0xF3`）；w = REX.W 位值；扩展寄存器→REX | `@sse_rr 0xF2 0x51 0 dest src`（:593 sqrtsd）、`@sse_rr 0xF2 {opcode} 0 dest src`（:561，配合 opcodes 表） |
| `@sse_rr_3a` | `prefix opcode reg rm imm` | `66 0F 3A /r ib` | `@sse_rr_3a 0x66 0x0B dest src mode`（:588 roundsd） |
| `@sse_rr_38` | `prefix opcode reg rm` | `66 0F 38 xx /r`（SSE4.1，如 PMULLD） | `@sse_rr_38 0x66 0x40 dest src`（:752 pmulld） |
| `@sse_rr_opsize` | `prefix opcode opsize reg rm` | 同 `0F xx /r`，但 REX.W/`0x66` 前缀按**运行时** opsize 动态决定（修复固定 w=1 导致窄宽度也按 64 位执行） | `@sse_rr_opsize 0xF3 0xBD opsize dest src`（:1038 popcnt） |
| `@sse_rr_imm8` | `prefix opcode reg rm imm` | `0F xx /r ib`（PSHUFD/SHUFPS 等） | `@sse_rr_imm8 0x66 0x70 dest src imm`（:898 pshufd） |
| `@sse_rr_w` | `prefix opcode reg rm` | 恒发 REX.W=1（GPR↔XMM 等） | `@sse_rr_w 0x66 0x6E dest src`（:666 movd） |
| `@sse_ps_rr` | `opcode reg rm` | packed-single，无强制前缀 | `@sse_ps_rr 0x29 src dest`（:705 movaps） |
| `@vex_rrvvv` | `map pp w l opcode reg rm vv has_src` | 3 字节 VEX（C4）三操作数 AVX（vaddps 等：dest=ModRM.reg、rm=ModRM.r/m、vvvv=~src1）。R=~reg[3]、B=~rm[3]、X=1；`has_src=0`（无源指令，如 vextractf128/vbroadcastss）→ vvvv 直接编码 1111；`has_src=1` → vvvv = `~vv & 0xF`；发射前 `assert!(avx_available())` | `@vex_rrvvv 1 0 0 1 {opcode} dest src2 src1 1`（:759 vaddps 族）、`@vex_rrvvv 1 0 0 1 0x28 dest src 15 0`（:773 vbroadcastss） |
| `@vex_rrvvv_avx2` | `map pp w l opcode reg rm vv has_src` | 同 `@vex_rrvvv` 但断言 `avx2_available()`（整数 256 位：VPADDD/VPSUBD/VPXOR/VPMULLD 等） | `@vex_rrvvv_avx2 1 1 0 1 0xEF dest src2 src1 1`（:806 vpaddd） |
| `@vex_rrvvv_imm` | `map pp w l opcode reg rm vv has_src imm` | VEX + imm8（vinsertf128/vextractf128 等） | `@vex_rrvvv_imm 3 1 0 1 0x18 dest src2 src1 1 imm`（:847 vinsertf128） |
| `@push_reg` | `reg` | PUSH r64（扩展寄存器加 `0x41` 前缀） | `@push_reg reg`（:1053） |
| `@pop_reg` | `reg` | POP r64 | `@pop_reg reg`（:1059） |
| `@mov_imm64` | `reg imm` | MOV r64, imm64（REX.W + B8+r + 8 字节） | `@mov_imm64 reg imm`（:263） |
| `@setcc` | `dest cond` | SETcc r/m8（`0F 90+cc /r`） | `@setcc dest cond`（:393） |
| `@lea_sib` | `dest base index scale disp` | REX.W + `8D` + ModRM/SIB：index=4（RSP）是无 index 哨兵、base=4 也强制发 SIB（否则 SIB 被 CPU 当 disp8、指令流错位）；scale 1/2/4/8 → 00/01/10/11；force-disp base 来自 `[meta].modrm_force_disp_base` | `@lea_sib dest base index scale disp`（:406） |
| `@lea_rbp_disp` | `dest base disp` | REX.W + `8D` + ModRM(base) + disp8/disp32（base 可为字段或物理编号字面量，x86 RBP=5） | `@lea_rbp_disp dest 5 disp`（:412） |
| `@lea_rip_rel` | `dest target` | `lea rd, [rip+disp32]`：REX.W + `8D` + ModRM(rm=101) + disp32，记录 PC-relative 重定位（`RelocKind::Relative(4,-4)` → PE REL32，ASLR 安全） | `@lea_rip_rel dest global`（:1013） |
| `@shift_reg` | `count dest src ext opsize` | 可变计数移位：count 为计数寄存器**物理编号**（x86 CL=1，架构事实由 TOML 传入）；计数不在 count 寄存器时先 mov（REX.W + `8B` + ModRM）；**opsize==64 才带 REX.W**（32 位不得加） | `@shift_reg 1 dest src {opcode} opsize`（:358） |
| `@call_reloc` | `target tpl` | 跨函数 call（32 位指令、位移在同一指令字内）：发 tpl 模板 + 对 `"@N"` 符号的重定位（AArch64 BL imm26、RISC-V JAL）。`target` 是携带 FuncRef 编号的字段 | `@call_reloc offset, 0x94000000`（aarch64_v10.toml:340 BL）、`@call_reloc offset, 0xEF`（riscv64_v10.toml:475 JAL） |
| `@call_reloc32` | `target opc` | x86 风格跨函数 call：opcode 字节 + rel32 占位 + 重定位 | `@call_reloc32 target, 0xE8`（x86_v10.toml:931） |
| `@abs_reloc` | `target` | 绝对 8 字节占位 + 对 `"G{id}"` 的重定位（模块全局符号，JIT 数据段解析） | 与 REX 段组合：`{0x4:[4;4]}{1:[3;1]}{0:[2;1]}{0:[1;1]}{dest:[0;1;3]} {0x17:[3;5]}{dest:[0;3]} @abs_reloc global`（:1006 movabs） |
| `@leb128` | `field` | 无符号 LEB128 | wasm32 宏体内（wasm32_v10.toml:37） |
| `@sleb128` | `field` | 有符号 LEB128 | `pattern = "{0x41:[0;8]} @sleb128 ${imm} @leb128_reg 0x21 ${dest}"`（wasm32_v10.toml:49） |
| `@leb128_reg` | `prefix reg` | 前缀字节 + 寄存器编号的 LEB128（wasm local.get/set） | `@leb128_reg 0x20 dest`（wasm32_v10.toml:257） |
| `@bswap_r` | `dest opsize` | BSWAP r/m（`0F C8+r`；64 位加 REX.W，REX.B=dest[3]） | `@bswap_r dest opsize`（x86_v10.toml:1031） |

说明：

- 原语参数可用**逗号或空白**分隔（`parse_primitive` 两者都切，bitstring.rs:246-262）。
- 原语未知名会报错并列出可用列表（bitstring.rs:1973-1975）。
- `opsize` 参数可以是数字字面量（如 `64`）或指令的 `Opsize` 字段名（如
  `@modrm opsize 0x8B dest src`），运行时按字段值动态选择前缀/REX。
- 与 `opcodes` 表配合：`opcodes = [[name, opcode, mnemonic], …]` 会把 encoding 中
  的 `{opcode}` 与 asm 中的 `{mnemonic}` 逐个替换，一个模板展开成多条指令
  （model.rs:438-474；例 x86_v10.toml:561 `@sse_rr 0xF2 {opcode} 0 dest src`）。

---

## 用户宏 `$macro` 与散射

### `$macro` 编码宏（`[enc_macros.*]`）

`[enc_macros.NAME]` 定义可复用编码片段，含位置参数列表 `params` 与
`pattern`（pattern 中用 `${param}` 占位符），调用形式 `$name arg1 arg2`。
展开是**纯文本替换**（`expand_macros`，bitstring.rs:709-799），参数按位置
替换 `${param}`，然后整个结果再进解析器。

aarch64_v10.toml:134-144 的真实定义：

```toml
[enc_macros.a64_rrr]
params = ["opcode", "dest", "src1", "src2"]
# R-type: dest@0:5, src1@5:5, 0@10:6, src2@16:5, opcode@21:10, sf=1@31:1
pattern = "32 {${dest}:[0;5]}{${src1}:[5;5]}{${src2}:[16;5]}{${opcode}:[21;10]}{1:[31;1]}"

[enc_macros.a64_rrr_rm]
params = ["opcode", "dest", "src1", "src2"]
pattern = "32 {${dest}:[0;5]}{${src1}:[5;5]}{31:[10;5]}{${src2}:[16;5]}{${opcode}:[21;10]}{1:[31;1]}"
```

调用（aarch64_v10.toml:153-156）：

```toml
[inst.SD_ADD]
fields = { dest = "Ireg", src1 = "Ireg", src2 = "Ireg" }
encoding = "$a64_rrr 0x22C dest src1 src2"
```

等价于手写 `32 {dest:[0;5]}{src1:[5;5]}{src2:[16;5]}{0x22C:[21;10]}{1:[31;1]}`。

宏体内可以组合原语——wasm32 的宏把整条指令序列封装起来（wasm32_v10.toml:35-61）：

```toml
[enc_macros.wasm_binop]
params = ["opcode", "dest", "src"]
pattern = "@leb128_reg 0x20 ${dest} @leb128_reg 0x20 ${src} {${opcode}:[0;8]} @leb128_reg 0x21 ${dest}"
```

规则：

- 参数个数必须与 `params` 一致，否则报错（bitstring.rs:775-783）；
- 未知宏名报错并列出可用宏（bitstring.rs:768-773）；
- pattern 中可用 `#` 注释（多行字符串时按 `strip_comments` 剥离）。

### `$NAME` 常量（`[enc_constants]`）

`[enc_constants]` 定义命名常量，在宏展开前用 `$NAME` 引用替换（model.rs:32-35、
`expand_constants` bitstring.rs:698-706）。

### `{field:pattern}` 散射（`[enc_scatters.*]`）

散射与宏共用 `EncMacro` 结构（model.rs:26-27、1201-1211）：`[enc_scatters.NAME]`
的 `pattern` 用 `_` 作字段占位符，编码中写 `{field:name}` 即把 pattern 里所有
`_` 替换为该字段名（`expand_scatters`，bitstring.rs:801-872）。判定规则：
`{X:Y}` 中 `Y` 必须是纯字母/下划线（没有第二个 `:`、不以 `[` 开头、不含 `;`），
否则按普通位域处理。当前仓库 ISA 尚未使用该特性，语法示例：

```toml
[enc_scatters.rd5]
pattern = "{_:[0;5]}{31:[16;5]}"
```

`{dest:rd5}` → 展开为 `{dest:[0;5]}{31:[16;5]}`。

---

## 运行期原语

`encoding` 字符串在编译期被 `forge-dsl` 生成进 `emit_inst`（codegen/mod.rs:766-791）；
生成的代码运行期调用以下原语（`crates/backend/forge-codegen/src/encode/packer.rs`
与 `crates/backend/forge-codegen/src/pipeline/emit.rs`）：

### `pack_bits`（packer.rs:48）

```rust
pub fn pack_bits(sink: &mut CodeSink, total_bits: u8, fields: &[BitField])
```

把 `(value, offset, width)` 位域列表打包为一个指令字并写入 sink。`total_bits`
8/16/32/64 → `put1/put2/put4/put8`；其他宽度逐字节写入（`div_ceil(8)`）。
字段值会被 mask 到 width 位（width=64 用 `u64::MAX`）。debug 构建下字段越界
`total_bits` 会 panic。

```rust
pub struct BitField { pub value: u64, pub offset: u8, pub width: u8 }
impl BitField { pub const fn new(value: u64, offset: u8, width: u8) -> Self }
```

### `pack_bits_label`（packer.rs:84）

```rust
pub fn pack_bits_label(sink: &mut CodeSink, total_bits: u8, fields: &[BitField],
                       target: Block, reloc: RelocKind)
```

等价于 `pack_bits` + `sink.use_label_at(offset, target, reloc)` —— 手动编码时
给分支/跳转指令记录标签 fixup 的便捷封装。由 bitstring 生成的代码不走这个封装，
而是直接 `pack_bits` + `sink.use_label_at(...)`（bitstring.rs:949-953、1030-1034）。

### `CodeSink`（pipeline/emit.rs）

字节级发射缓冲 + 标签 fixup：

- 写入：`put1/put2/put4/put8/put_bytes`、`offset()`（当前写位置）、`bytes()`；
- 标签：`bind_label`（当前位置绑定）、`use_label_at(patch_offset, label, kind)`、
  `add_reloc(offset, kind, symbol, addend)`、`relocations()`；
- 收尾：`finish()` 解析所有待定 fixup——若设置了后端 `RelocPatcher`
  （`set_patcher`），由它做 ISA 特定编码（x86 rel32、AArch64 BL imm26、RISC-V B/J
  立即数重排），否则走旧的纯相对/绝对回写路径。

---

## 实例解析

### x86：`@modrm` 原语（isa/x86_v10.toml:249-252）

```toml
[inst.MOV_R_RM]
fields = { dest = "Ireg", src = "Ireg", opsize = "Opsize" }
encoding = "@modrm opsize 0x8B dest src"
```

展开为（bitstring.rs:1232-1256 的模板）：

1. `__opsize = opsize as i64`（运行期取 Opsize 字段值）；
2. opsize==16 → `sink.put1(0x66)`；
3. `__reg = dest.to_index()`、`__rm = src.to_index()`；
4. REX = `0x40 | (reg&8 → 0x04) | (rm&8 → 0x01)`；opsize==64 或任一寄存器 ≥8 时
   发射 REX（64 位时 REX.W=1 → `0x48`）；
5. `sink.put1(0x8B)`；
6. `sink.put1(3<<6 | (reg&7)<<3 | (rm&7))`（mod=11 寄存器直接）。

例如 `mov rax, rbx`（opsize=64、dest=0、src=3）→ `48 8B C3`。

### AArch64：定宽位域（isa/aarch64_v10.toml:103-107）

```toml
[inst.SD_MOV]
fields = { dest = "Ireg", src = "Ireg" }
encoding = "32 {dest:[0;5]}{src:[5;5]}{31:[16;5]}{0x22C:[21;10]}{1:[31;1]}"
```

定宽 32 位、5 个位域（全部不重叠，`check_overlap` 通过）：

| 位域 | 位区间 | 值 |
|------|--------|-----|
| `{dest:[0;5]}` | 0..5 | `dest.to_index()`（Rd） |
| `{src:[5;5]}` | 5..10 | `src.to_index()`（Rm） |
| `{31:[16;5]}` | 16..21 | 常量 31（XZR） |
| `{0x22C:[21;10]}` | 21..31 | 常量 `0x22C`（ORR 变体） |
| `{1:[31;1]}` | 31 | 常量 1（sf=64 位） |

生成 `pack_bits(sink, 32, &[BitField::new(dest.to_index(),0,5), …])` ——
即 `ORR Xd, XZR, Xm`（AArch64 的 `mov Xd, Xm`）。对比 bitstring.rs 头注释里的
同款示例 `32 {dest:0:5}{src1:5:5}{src2:16:5}{0x22C:21:10}{1:31:1}`（AArch64 ADD）。

---

## 迁移指南

### 从表格式 encoding 迁移（已删除的 `[inst.X.encoding]`）

旧的表格式已删除：`Instruction` 反序列化带 `deny_unknown_fields`
（model.rs:849-864），`[inst.X.encoding]` 表（`format`/`width`/`bitfields`/
`segments`/`condition` 键）现在会直接解析失败。改写为单行字符串：

| 旧表格式 | 新字符串 |
|---------|---------|
| `format = "bits"`, `width = 32`, `bitfields = [{ value = "dest", offset = 0, width = 5 }, …]` | `encoding = "32 {dest:[0;5]}{…}"` |
| `format = "segments"`, `segments = [{ width = 8, fields = […] }, …]` | `encoding = "{…} {…} {…}"`（空白分隔段） |
| 段上的 `condition = "src >= 8 \|\| dest >= 8"` | 段前加 `?src>=8\|\|dest>=8` 前缀 |
| 位域上的 `shift = 3` | `{V:[O;W;3]}` 方括号形式 |
| 固定 opcode 段 + 标签占位 | `{…} !rel4` / `!isa1` 等 |

### 从手写 emit 模板迁移（已删除的 `[inst.X.emit] template`）

旧的手写逐指令发射模板不再存在。现在所有编码都在 `encoding` 字符串里声明
（codegen/mod.rs:772-783）；没有 `encoding` 的指令在 `emit_inst` 里生成
`IrError::Emit("no encoding for")` 错误（codegen/mod.rs:786-790）。迁移步骤：

1. 识别原手写函数里的字节布局：
   - 固定宽度位布局 → 定宽字符串 `32 {…}`（位偏移照搬，0 = LSB）；
   - 条件前缀（REX 等）→ `?cond {…}` 分段；
   - 常见模式（ModRM/前缀/REX/VEX/LEB128/重定位）→ 直接换成速查表里的原语
     `@…`，例如 `(sf<<31)|(op<<21)|(rm<<16)|(rn<<5)|rd` 这类 AArch64 布局 →
     `32 {rd:0:5}{rn:5:5}{rm:16:5}{op:21:10}{sf:31:1}`；
   - 跨函数 call / 全局地址 → `@call_reloc` / `@call_reloc32` / `@abs_reloc` /
     `@lea_rip_rel`（内部已含重定位记录）。
2. 重复出现的布局 → 提炼为 `[enc_macros.*]` 宏，用 `$name args` 调用。
3. 分支/跳转 → 声明 `BlockTarget` 字段 + `!rel4`/`!isa0`/`!isa1`/`!isa2` fixup。
4. 删除手写模板；运行 `cargo test`（缺 encoding 的指令会以
   `no encoding for` 编译错误暴露）。

### 迁移检查清单

- [ ] 字符串不以数字开头却想定宽？→ 补宽度前缀（`32 {…}`）。
- [ ] 段之间有空白吗？空白是分段符（bitstring.rs:299-306）。
- [ ] 条件表达式里的字段名与 `fields` 声明一致（否则解析为未知字段报错）。
- [ ] 位域重叠 → 编译错误，检查 offset+width。
- [ ] fixup 指令有 `BlockTarget` 字段。
- [ ] `$macro` 参数个数与 `params` 一致。
