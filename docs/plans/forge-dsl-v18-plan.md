# forge-dsl 改进方案（ISA-DSL v18）

> 状态：[progress]（2026-09-19 撰写）。用户 2026-09-19 拍板口径：**允许破坏性更新、无需兼容旧版本、
> 可参考网络上的设计方案、需兼顾用户体验、需足够通用而非服务于个别指令集**。
>
> 现行实现：`crates/frontend/forge-dsl/src/v12/`（生成器内部仍叫 `v12`，schema 已迭代到 v17——
> 命名本身就是本方案要修的问题之一）。v18 语法规范落地后重写 `docs/reference/isa-dsl.md`，
> 旧语法（v12–v17）整篇归档 `docs/archive/isa-dsl-v12-v17.md`。
> 文中一切数字为本机实测（命令与日期随行标注），**以代码与测试为准**。

## 目录

- [1. 目标与成功判据](#1-目标与成功判据)
- [2. 现状证据](#2-现状证据)
- [3. 设计原则](#3-设计原则)
- [4. 参考设计](#4-参考设计)
- [5. v18 语法（逐节变更）](#5-v18-语法逐节变更)
- [6. 删除与改名总表](#6-删除与改名总表)
- [7. 切片计划](#7-切片计划)
- [8. 迁移](#8-迁移)
- [9. 边缘情况与失败模式](#9-边缘情况与失败模式)
- [10. 显式假设](#10-显式假设)
- [11. 文档落地清单](#11-文档落地清单)
- [12. 附录：S0 基线实测](#12-附录s0-基线实测)

## 1. 目标与成功判据

**目标**：把 forge-dsl 从"能表达三个 ISA 的 TOML 生成器"提升为**通用 ISA 规格语言**——数据驱动
（ISA 形状全在数据里，生成器不含任何 ISA 常量）、可参数化（族式重复由模板实例化消除）、
诊断可用（一次列全部错误 + 精确行列 + 可操作建议）、能力完备（条件码/重定位/伪指令/混合宽度/
生成自测都是数据）。

| 判据 | 当前基线（实测） | 目标 |
| --- | --- | --- |
| 生成器里的 ISA 常量 | `lowering.rs:110-138` 硬编码 x86 setcc 码；`asm.rs:396` x86 cond 缺省；`[conventions.cond]` 只有 x86 声明 | **0 处**，守卫测试钉死，全部从 ISA 数据取 |
| 族式重复 | arm64 38 对 X/W（76/89 = 85%）、riscv 16 对 S/D（32/78 = 41%）、x86 宽度变体与 SSE 族 | 模板实例化后重复声明消失；arm64 TOML 行数 **−30%**、x86/riscv **−10%** |
| 条件码能力 | arm64 只能编码 `b.eq` 一条（`BCOND` + `fields={cond=0}`）；x86 用 `cond` 槽 | arm64 `b.cond` **16 个条件一条声明**；两 ISA 共用同一机制 |
| 手工别名清单 | x86 25 条 / arm64 29 条 `[[aliases]]`，已见漂移（`vaddps` 漏 `VADDPS_ZMM_MASKZ`） | 别名由模板 `ref` 自动派生，清单消失 |
| 诊断 | fail-fast 单条错误；定位靠 `source.find("name = ...")` 启发式（`diag.rs:60-72`） | 一次列全部（≤32 + 计数）；精确 span；错误码 + did-you-mean |
| 校验缺口 | `[emit]`/`[spill]` 只查非空（`validate.rs:1124-1152`） | 与 lowering 同等严格（引用名/占位符/槽绑定全覆盖） |
| 指令级自测 | 生成面手写测试 2,952 行（含 `decoder_smoke.rs` 253 行覆盖十余条指令） | 生成 `__spec_tests`：全指令 encode↔decode↔disasm↔asm + 立即数边界 |
| 指令宽度 | `default_inst_width: Option<u32>` 单一全局值（`model.rs:361`） | `[encoding]` 三态 + 逐指令/模板 `width` ⇒ RVC/Thumb 可表达 |
| 规格书一致性 | 文档停在 v15/S1–S6，代码注释已到 v16/v17；`aliases`、`[conventions.mem]`、指令级 `when`、`prefix_scan.range`、EVEX `b/z/disp_scale` 未文档化；文档 L522 说 lowering 用"助记符"而模型 `model.rs:56-58` 说用"别名/指令名" | schema ↔ 文档 ↔ JSON Schema 三方针由守卫测试强制一致 |
| 门禁 | matrix x86 195/3/0、riscv64 131/67/0、arm64 23/175/0；语料与编码黄金值全绿 | 全部**不回归**；arm64 矩阵通过数上升（具体数落地实测回填） |

**非目标（写死）**：不做 LLVM bitcode/MC 互操作；不做从外部 ISA 描述（SLEIGH/Sail/riscv-opcodes）
自动导入；不做指令选择的形式化验证/SMT；不引入新依赖（如确需 `clap` 或 JSON-Schema 生成器另开请示）；
不动 forge-ir/forge-opt/forge-codegen 的运行期语义（本方案点名的 reloc 宿主契约除外）。

## 2. 现状证据

### 2.1 语法已迭代到 v16/v17，文档停在 v15

- `model.rs:56` `[[aliases]]（S10e）`、`model.rs:684` `[conventions.mem]（v16）`、
  `model.rs:1386` `asm 必填（v17…）`；文档正文只讲 v15 的 S1–S6。
- `docs/reference/isa-dsl.md` 里 `aliases` **0 次出现**；`[conventions.cond/mem]` 现语义、
  `prefix_scan.range`、EVEX `b/z/disp_scale`、指令级 `when`（`Instruction.when`）均无记载。
- 文档 L522：`insts` 用**助记符**引用；`model.rs:56-58` 与 `validate.rs:817-839`：用**别名名或指令名**引用。
  实际 x86 写 `mov`/`add`（别名）、riscv 写 `ADDW`（指令名）——文档这条是错的。

### 2.2 参数化缺失 ⇒ 三个 ISA 都在手抄族式重复

| ISA | 重复形态 | 实测 |
| --- | --- | --- |
| arm64 | X/W 对（sf 位、寄存器槽、opcode 三处不同，其余全同） | **38 对 / 76 条 = 89 条的 85%**：`ADDIMMX`(0x91, r64) 与 `ADDIMMW`(0x11, r32) 仅三处不同 |
| riscv | `_S`/`_W` ↔ `_D` 对（fpr4/fpr8、opcode） | **16 对 / 32 条 = 41%** |
| x86 | 宽度变体（`ADD_R_IMM32`/`ADD64_R_IMM32`、`MOV64_*`/`MOV_R8_*`…）与 SSE SD/SS 族 | 146 指令 + 51 族变体、19 forms；仅 6 组 12 条能被"同 form+asm+ops"归并，其余因变体无法覆盖 `ops`/`form`/enc 键而只能整条重写 |

`[[families]]` 的变体只能覆盖 `name/opcode/fields/asm/roles/when`（见 `model.rs` 的 `FamilyVariant`），
**不能覆盖 `ops`/`form`/其他 enc 键** ⇒ arm64 的 X/W 对写不进 family，只能整条复制。

### 2.3 别名是手工清单，且已漂移

x86 `[[aliases]]` 25 条、arm64 29 条（几乎全是 X/W 对或 1:1 改名），riscv 0 条。实测漂移：
`VADDPS_ZMM_MASKZ` 被 1 条 lowering 直接按指令名引用，却**不在** `vaddps` 别名里。
清单式成员登记 = 新增变体时忘了登记就静默失去"按引用名分派"的能力。

### 2.4 条件码不是数据（最典型的"服务个别指令集"）

- `lowering.rs:110-138`：在**通用** lowering 生成器里硬编码 `IntCC → x86 setcc 码`
  （`Equal => 4`、`SignedLessThan => 12`…），注释自述"x86 的 setcc 编码"。
- `asm.rs:396` `cond_default()`：`[conventions.cond]` 缺省 = x86 的 `o/no/b/ae/e/ne/...`；riscv/arm64 均未声明该表。
- 后果（实测）：arm64 只有 `BCOND{fields={cond=0}, asm="b.eq {target}"}` —— 只能编 `b.eq`；
  其余 15 个条件既无法声明也无法编码。同一件事在 x86 是"cond 槽 + 表"，在 arm64 只能是字面量。

### 2.5 `[emit]`/`[spill]` 是未校验的名字引用面

`validate_emit`/`validate_spill` 只查非空；`insts = ["PUSH RBP", "MOV64_RR RSP, RBP",
"SUB64_R_IMM32 RSP, {callee_saved_bytes}"]` 里的指令名、占位符没有任何编译期校验
（对比：lowering 已有引用名/占位符/属性三重校验，`validate.rs:875-924`）。
另有 5 个固定伪指令（`@push_callee`/`@pop_callee`/`@frame_alloc`/`@frame_dealloc`/`@move_args`）
与 41 条占位符分支硬编码在 Rust 里（`frame.rs`/`placeholder.rs`）。

### 2.6 指令宽度只有全局单值

`model.rs:361` `default_inst_width: Option<u32>` + `variable_length: bool`：定宽 ISA 全局一个值，
变长 = x86 式前缀扫描。**16/32 混合宽度（RVC、Thumb、MIPS16）无法表达**；`Instruction` 无 `width` 字段。

### 2.7 诊断：fail-fast + 启发式定位

`validate.rs:9-23` 顺序调用 14 个校验器，每个 `Result<(), String>` ⇒ **一次只报第一条**；
`diag.rs:60-72` 用 `source.find("name = \"X\"")` 回找行号（同名或同名前缀会指错），
抽不出名字就退化到 `1:1`。没有错误码、没有多错误、did-you-mean 只在个别消息里手写。

### 2.8 生成器里仍有 x86 缺省

`asm.rs:396`（cond 表）、`[conventions.prefix_scan]` 缺省 = x86 扫描集、`[abi]` 多项缺省 = x86 形态
（含 `call_ret_reg` 缺省 `"X1"` = riscv 的 ra）、`value_fpr_width` 缺省 8、`vector_tiers` 缺省 `[16,32,64]`。
这些缺省让非 x86 ISA 在**不报错**的情况下拿到别家形状。

### 2.9 工程形态

`forge-dsl` 是 `proc-macro = true` 的 crate，装着 model（1,781 行）+ validate（1,107 行）+
codegen（约 10.7k 行）+ 单测 `tests.rs`（2,713 行）：**无法出 bin/example**（proc-macro crate 限制），
单测无法作为普通 lib 被工具复用。`isa_from_file!` 只有 `krate` 一个参数（无模块名覆盖、无部件选择、
无多文件组合），没有 JSON Schema/编辑器补全，`FGE_DEBUG_GEN` dump 的是未格式化 token 串。

## 3. 设计原则

1. **数据 vs 宿主**：凡"某个 ISA 的形状"（条件码、重定位语义、伪指令、宽度、寄存器角色）都是数据；
   凡"编译器基础设施语义"（IR 算子、ABI 参数类、regalloc 契约）才是宿主。§2.4/§2.5/§2.6 三处越界改回数据。
2. **一族一写法**：同一概念在所有 ISA、所有节里用同一写法。`lowering.vary` 的"等长参数表 + 下标 zip +
   自动谓词"已经存在，就把它提升为**指令模板**的同一机制，而不是再造一套。
3. **单一事实源 + 守卫**：schema（serde 模型）是唯一事实源；文档、JSON Schema、编辑器补全、
   "生成器不得含 ISA 常量"都由会失败的测试钉住（沿用 `tests/no_hardcoded_widths.rs` 的
   "白名单 + 理由 + 防腐烂"机制）。
4. **fail-closed 与可读性优先于省行**：宁可报错也不静默默认；错误消息必须能直接改数据（带位置、
   带建议、一次给全）。数据文件里的历史迁移注释（"v13 起…""已删除…"）一律移出，进归档文档。

## 4. 参考设计

| 参考 | 借用点 |
| --- | --- |
| LLVM TableGen（[语言参考](https://llvm.org/docs/TableGen/ProgRef.html)、[FOSDEM 讲稿](https://archive.fosdem.org/2019/schedule/event/llvm_tablegen/attachments/slides/3304/export/events/attachments/llvm_tablegen/slides/3304/tablegen.pdf)） | `multiclass`/`defm` 参数化实例化、`let` 逐实例覆盖、`include` 组合、`InstAlias` 与伪指令、"只给汇编器用"的标记 |
| Cranelift ISLE（[crate](https://crates.io/crates/cranelift-isle)） | 选择规则写成**规则语言**（模式 + if-let + 优先级 + 可复用规则），规则与匹配器分离 |
| QEMU decodetree（[文档](https://www.qemu.org/docs/master/devel/decodetree.html)、`scripts/decodetree.py`） | **按宽度分组**的定宽解码（多宽度并存的现实写法）、字段抽取、伪指令 |
| Ghidra SLEIGH（本仓库已借鉴其位域风格） | 上下文/变长解码的字段-上下文分离（只借"宽度分组 + 上下文变量"的组织方式） |

Cranelift ISLE 与 decodetree 的页面本次抓取超时（网络受限），以上按已知设计要点引用；落地时以官方文档核对措辞。

## 5. v18 语法（逐节变更）

### 5.1 文件头与版本

```toml
schema = 18                              # 必填；不匹配即报错并给出迁移指引
name = "arm64_v12"                       # 顶层（原 [meta].name）
include = ["common/rv_base.toml"]        # 可选：多文件组合（见 5.7）
```

`[meta].version` 改为 **ISA 自己的版本串**（自由字符串，与 schema 无关）；生成器模块 `src/v12/` 改名 `src/schema/`。

### 5.2 `[[templates]]` — 参数化指令（取代 `[[families]]` + `[[aliases]]`）

```toml
# arm64：38 对 X/W → 19 条模板
[[templates]]
name   = "ADDIMM{x}"                     # 实例名（{x} 逐行替换）
ref    = "add"                           # 引用名：全部实例自动并入（取代手写 [[aliases]]）
form   = "ALUIMM"
opcode = [0x91, 0x11]                    # 整数键：单值 = 全部相同；数组 = 逐行
ops    = ["dst:{slot}:out", "src:{slot}", "imm:imm12u"]
asm    = "add {dst}, {src}, #{imm}"
params = { x = ["X", "W"], slot = ["r64", "r32"] }   # 等长 ⇒ 下标 zip（与 lowering.vary 同语义）

[[templates.overrides]]                  # 罕见的逐行例外（如只给 X 版一个角色）
row   = 0
roles = ["frame_free"]
```

```toml
# riscv：16 对 S/D → 8 条模板
[[templates]]
name   = "FADD_{p}"
ref    = "fadd.{p_lc}"
params = { p = ["S", "D"], p_lc = ["s", "d"], slot = ["fpr4", "fpr8"] }
form   = "R4"
opcode = [0x53, 0x53]
fields = { funct7 = [0x00, 0x01] }
ops    = ["rd:{slot}:out", "rs1:{slot}", "rs2:{slot}"]
asm    = "fadd.{p_lc} {rd}, {rs1}, {rs2}"
```

```toml
# x86：SSE 家族（一个模板 N 条助记符）
[[templates]]
params = { m = ["movsd", "addsd", "subsd", "mulsd", "divsd", "minsd", "maxsd"],
           opcode = [0x10, 0x58, 0x5C, 0x59, 0x5E, 0x5D, 0x5F] }
names  = ["MOVSD", "ADDSD", "SUBSD", "MULSD", "DIVSD", "MINSD", "MAXSD"]
ref    = "{m}"                           # 每实例各自 1:1 引用名
form   = "SSE_RR"
fields = { prefix = 0xF2, w = 0 }
ops    = ["dst:fpr:out", "src:fpr"]
asm    = "{m} {dst}, {src}"
roles  = { MOVSD = ["fpr_mov_f64"] }      # 稀疏角色：名字 → 角色列表
```

**规则（写死、可校验）**：

- `params` 各列表**等长**，按下标 zip；行数 = 实例数。
- 字符串字段里的 `{param}` 做文本替换；整数键可单值或等长数组；`fields` 表内同样支持单值/数组。
- 实例名 = `name` 插值或 `names` 列表；实例名与指令名同池、全局唯一。
- `ref`：单值（所有实例共用，多态分派）或插值/列表（逐实例各得一个 1:1 引用名）。
- `roles`/`effects`/`implicit_regs` 等列表键支持"名字 → 列表"稀疏表或 `[[templates.overrides]]` 逐行覆盖。
- 完整性校验：参数域非空等长；替换后名非空且唯一；`ref` 不与指令名冲突；
  同模板两行产生相同 `(ops 签名, asm 骨架)` ⇒ 报"实例不可区分"。
- 生成顺序 = 模板声明序 + 行序 ⇒ 生成的 `Inst` 枚举与编码臂顺序稳定可复现。

### 5.3 条件码数据化（删除 x86 硬编码）

```toml
# [conventions.cond] 的键 = IR 条件名（与 IntCC/FloatCC 语义一一对应），值 = 本 ISA 的编码
[conventions.cond]
eq = 4
ne = 5
slt = 12
sge = 13
sgt = 15
sle = 14
ult = 2
ule = 6
ugt = 7
uge = 3
```

- `{cc}` 占位符 = "当前 IR 条件 → 本 ISA 编码"，由表驱动；删掉 `lowering.rs:110-138` 的 x86 表与
  `asm.rs:396` 的 x86 缺省（缺表 = 明确报错，不再静默按 x86）。
- `cond` 槽的**名字表**由同一份数据派生（可另给别名组，如 `e`/`z` 同指 `eq`）。
- arm64 由此获得全 16 条件：`ops = ["cond:cond", "target:off19"]`、`asm = "b.{cond} {target}"` 一条声明。

### 5.4 `[[reloc]]` — 重定位数据化（取代 `global_reloc` 枚举）

```toml
[[reloc]]
name = "abs64"                  # 指令引用：reloc = "abs64"
semantics = "absolute"          # 宿主契约（有限、ISA 无关）：absolute | pc_relative | hi20 | lo12 | got | tls_*
slot = "imm"                    # 绑定到哪个操作数槽
addend = 0                      # 可选：编码期加减
```

宿主保留**有限语义集**（不是每个 ISA 一个 patcher）；新增语义才需要宿主代码。
删除 `GlobalReloc{Abs8,PcrelHi,PcrelLo}` 与 `Instruction.global_reloc`，改 `Instruction.reloc: Option<String>`。

### 5.5 `[[pseudo]]` — 汇编器伪指令（新增）

```toml
[[pseudo]]
name = "li"                     # 汇编/反汇编层可见的名字
params = ["rd", "imm"]
when = { lt = ["imm", 4096] }    # 复用结构化谓词；同 name 多条按同一裁决序分派
emit = ["lui {rd}, {imm_hi20}", "addi {rd}, {rd}, {imm_lo12}"]
```

展开是**汇编器层**行为（`.equ`/`.macro` 的结构化兄弟），不影响编码器与 lowering；
`pseudo_fold = true` 时反汇编可折叠回伪指令（缺省关闭）。

### 5.6 `[[derive]]` — 派生属性与派生值（新增）

```toml
[[derive]]
name = "is_64"
expr = { eq = ["rs1_width", 64] }

[[derive]]
name = "imm0_m4"
expr = { sub = ["imm0", 4] }
```

谓词属性补齐纯算术/逻辑节点（`sub`/`add`/`and`/`or` 等，宿主实现一次、与 ISA 无关）；
核心属性集保留为宿主契约但补齐并文档化，ISA 专属派生一律走 `[[derive]]`。

### 5.7 指令宽度三态（新增能力）

```toml
[encoding]
kind = "fixed"        # fixed | mixed | prefix_scan
bits = 32
# kind = "mixed" 时：widths = [16, 32]（解码按宽度分组尝试）
# kind = "prefix_scan" 时：max_len = 15，prefixes = [ { byte = 0x66, effects = ["opsize16"] }, … ]
```

`Instruction`/`[[templates]]` 可写 `width = 16`（缺省 = `[encoding].bits`；`mixed` 时必填或按 form 给）。
定宽解码把现有位级 trie 扩展为**按宽度分组 + 组内 trie**；`mixed` 按 `widths` 顺序尝试、首个完整匹配即停。
新增夹具 `demo_mixed16_32_v12.toml` 证明通用性。

### 5.8 组合与部件（新增）

```toml
include = ["common/rv_base.toml"]      # 深合并：数组按 include 序追加；同名标量冲突报错
[[override]]                            # 显式覆盖（取代"后出现的赢"这种隐式规则）
key = "meta.endian"
value = "big"
```

多文件诊断带**来源文件名**。`isa_from_file!` 参数扩展：`name = "..."`（模块名覆盖）、
`parts = ["encode", "decode", "asm", "tm"]`（部件选择）、`krate = ...`（保留）。

## 6. 删除与改名总表

| 旧 | 新 | 说明 |
| --- | --- | --- |
| `[meta].name` / `[meta].version` / `[meta].mode` | 顶层 `name` / `schema` | `version` 退为 ISA 自己的版本串；`mode` 删除（无消费者） |
| `[meta].default_inst_width` / `variable_length` / `max_inst_len` | `[encoding].kind/bits/widths/max_len` | 三态 |
| `[meta].default_opsize` | `[encoding].default_opsize` | 归位 |
| `[[families]]` | `[[templates]]` | 变体可覆盖任意字段（含 `ops`/`form`/enc 键） |
| `[[aliases]]` | `[[templates]].ref` | 自动派生 + 冲突校验 |
| `Instruction.global_reloc` | `Instruction.reloc = "<[[reloc]].name>"` | 数据化 |
| `[conventions.cond]`（x86 名 → 码） | 同名节，键 = IR 条件名 | 删除 Rust 侧 x86 表与缺省 |
| `Instruction.when`（三份谱 0 处使用） | 删除 | 死键 |
| `[conventions.mem]`（v16，未文档化） | 保留并文档化 | 已是数据 |
| 生成器内 x86 缺省（cond / prefix_scan / `[abi]` / `value_fpr_width` / `vector_tiers`） | 必填或 fail-closed 报错 | 不再静默给别家形状 |

## 7. 切片计划

> 门禁沿用 `docs/plans/forge-ir-binary-serialization-plan.md` §5 的固定命令（fmt / clippy 两道 /
> workspace 测试 / 语料 / release check / doc / markdownlint），**外加**每片必跑：三架构 JIT 矩阵、
> arm64 与 riscv 编码黄金值逐字节不变、以及本片新增守卫。每片结束提交，同批推送。

| 片 | 范围 | 关键产出 | 证据/验收 | 叫停价值 |
| --- | --- | --- | --- | --- |
| **S0** 基线 | 不改语法 | 采集生成代码规模/编译时间/TOML 与测试行数/坏 spec 首错行为；新增 `generality_guard.rs`（扫 `src/**` 里的 ISA 名、寄存器名、cond 码；白名单 + 理由 + 防腐烂） | 数字进 `docs/performance/bench_baseline.md` §ir_dsl；守卫先红后绿（红：§2.4 的 x86 setcc 表） | — |
| **S1** 诊断与校验 | 不换语法 | `Diags`（code + span + msg + notes + suggestions，收集式，≤32 + 计数）；**声明索引**（一次预扫建"节 + 名字 → 精确 span"，替代 `source.find` 启发式）；`[emit]`/`[spill]` 引用名与占位符校验；错误码目录 | 坏 spec 矩阵 ≥30 例断言条数/位置/建议；把 `MOV64_RR` 写成 `MOV64_R` 必须**编译期**报并点名行号 | 立刻可用 |
| **S2** 参数化模板 | `[[templates]]` 取代 families + aliases | 迁移 arm64（38 对）、riscv（16 对）、x86（SSE 族与宽度变体） | 全指令编码逐字节不变；arm64 TOML −30%、riscv/x86 −10% 行数；新增"实例不可区分/名冲突/域不等长"负向用例 | 核心收益 |
| **S3** 数据化 | cond / reloc / pseudo / derive | 删 x86 硬编码；arm64 得 `b.cond` 全 16 条件；`[[reloc]]`、`[[pseudo]]`、`[[derive]]` | arm64 16 条件编码对照 `docs/reference/aarch64-encoding-ref.md` 黄金值 + 反汇编往返；x86 `cond` 语义不变；`generality_guard` 白名单清空 | 通用性实质提升 |
| **S4** 宽度三态 | `[encoding]` + 逐指令 `width` | mixed 解码（按宽度分组 trie）+ 新夹具 `demo_mixed16_32_v12.toml` | 新夹具黄金字节全绿；x86/riscv/arm64 不回归 | 打开 RVC/Thumb 类 ISA |
| **S5** 降低语言升级 | 结构化 `insts` + `[[sequences]]` + 属性补齐 + pattern 统一 | `lowering.emit` 表形式（`inst`/`let`/`select`/`switch`）；共享序列；pattern 与 lowering 同一裁决序与死规则检测 | x86 220 条 lowering 下降 ≥15%；黄金值不变 | 中收益、风险最高（可延后） |
| **S6** 生成自测 | 生成 `__spec_tests`（`cfg(test)`） | 每指令 encode↔decode↔encode、disassemble↔assemble↔encode、立即数边界（min/max/min−1/max+1）、全宽度视图 | 覆盖 x86 197 / riscv 116 / arm64 89（100%）；命中缺陷即修并记录；样板测试可删减 | 每条新指令自动进回归网 |
| **S7** 工具链与文档 | 拆 crate + CLI + 文档 | `forge-isa-dsl`（普通 lib）+ `forge-dsl`（薄 proc-macro）；`forge-isa` CLI：`validate`/`explain`/`schema`/`fmt`/`diff`/`insts`；JSON Schema + `#:schema`；文档重写 | schema ↔ 文档 ↔ JSON Schema 三方针守卫；CLI 集成测试；`isa_from_file!` 新参数用例 | UX 与可维护性长期收益 |
| **S8** 生成物减薄（可选） | 静态逻辑下沉 `forge-codegen::runtime`，生成物只留表 + 分派 | 生成代码 token 数 −≥40%、`cargo check -p forge-codegen` −≥20% | 以 S0 基线对比；黄金值与矩阵不变 | 按度量决定 |

**建议顺序**：S0 → S1 → S2 → S3 → S6 →（S7）→ S4 → S5 →（S8）。
**最小可用子集**：S0 + S1 + S2 + S3；**可在 S3 后叫停**并保留全部价值。

## 8. 迁移

1. **迁移面**：`isa/{x86_v12,riscv64_v12,arm64_v12}.toml`（实测 6,235 行）+ 5 个夹具 +
   生成器 15,976 行（不含单测）+ 生成面手写测试 2,952 行（仅删样板部分）。
2. **顺序**：先落 v18 模型与校验（新语法可解析）→ 逐 ISA 迁移（每 ISA 一次提交，黄金值比对）→
   删旧语法代码与旧夹具 → 文档重写 + 旧语法归档。
3. **等价判据**（防"迁移顺手改语义"）：
   - 每 ISA 的**全指令编码**与迁移前逐字节一致：用 S0 采集的全指令样本 dump（指令 + 固定操作数样本 → sha256）迁移后重跑比对；
   - arm64/riscv/x86 既有黄金测试与 JIT 矩阵全绿（除 S3/S4 明确新增的能力）；
   - `forge-isa explain` 输出的"展开后有效规格"在迁移前后 diff 为空（除有意删除的死键与缺省）。
4. **数据清理**（同批）：TOML 里的历史迁移注释（"v13 起…""已删除…""历史行为"）全部移出，进归档文档。
5. **风险**：x86 单文件最大（3,356 行、146 + 51 条指令）最易出错；对策 = 分节迁移 + 每节跑全指令字节比对；
   x86 的 lowering（220 条）在 S5 之前不动。

## 9. 边缘情况与失败模式

| 场景 | 期望 |
| --- | --- |
| 模板参数域不等长/空/替换后名重复 | 编译期错误，指向 `params` 行 |
| 模板实例与手写指令同名；`ref` 与指令名冲突 | 编译期错误（现 `validate_aliases` 已有同类检查） |
| 两实例的 `(ops 签名, asm 骨架)` 相同 ⇒ 汇编不可区分 | 编译期错误（新增） |
| `[conventions.cond]` 缺项，`{cc}` 遇到未覆盖条件 | 编译期错误点名缺失条件（不再静默 0） |
| `[emit]`/`[spill]` 引用不存在的指令/别名/占位符 | 编译期错误 + 行号（S1） |
| `[encoding].kind = "mixed"` 但某指令无 `width` 且无法从 form 推导 | 编译期错误 |
| mixed 解码：短宽度前缀与长宽度指令前缀冲突 | 按 `widths` 顺序尝试，且校验器报"解码歧义" |
| `include` 循环 / 同名标量冲突 / 缺文件 | 编译期错误，消息含来源文件与 include 链 |
| 伪指令展开递归（`li` → `li`） | 深度上限 + 明确错误 |
| reloc 语义未知（宿主不支持） | 编译期错误列出可用语义集 |
| 坏 spec 有多条错误 | 一次列全（≤32），末尾"还有 N 条" |

## 10. 显式假设

1. **允许重写三个 ISA 的 TOML**（实测 6,235 行）——用户已明确"允许破坏性更新、无需兼容旧版本"。
2. **`forge-codegen` 的运行期面可对齐**：删除 `GlobalReloc`、新增 reloc 语义集、S8 的生成物减薄需要改
   `crates/backend/forge-codegen/src/machine/*`；这些改动不改变既有指令编码（黄金值守护）。
3. **补齐条件分支能力会改变 arm64 测试计数**（更多用例从 skip 转绿）；方案只承诺"不回归 + 上升"，
   具体数字落地实测回填。
4. **不新增依赖**：JSON Schema 由手写发射器 + 三方一致性守卫实现（不引 `schemars`）；CLI 手写参数解析
   （不引 `clap`）。若用户愿意放宽依赖策略，S7 可换成熟库简化。
5. 参照设计（TableGen/ISLE/decodetree）本次按已知要点引用（抓取超时），落地时以官方文档核对。
6. 归档文档只在头部声明现行替代入口，不参与持续维护。

## 11. 文档落地清单

- 本方案 `docs/plans/forge-dsl-v18-plan.md`（已落地）
- 重写 `docs/reference/isa-dsl.md`（v18 语法：一节一概念 + 迁移对照 + 生成代码契约）
- 新增 `docs/guides/isa-dsl-tutorial.md`（30 分钟接入一个小 ISA：寄存器组 → 位域 → 模板 → asm → 自测）
- 新增 `docs/reference/isa-dsl-errors.md`（错误码目录：码、含义、典型修法）
- 归档 `docs/archive/isa-dsl-v12-v17.md`（旧语法 + v15 的 S1–S6 + v16/v17 增量）
- 更新 `CLAUDE.md` 的 ISA-DSL 节（去掉 v12/v15 混述）、`CHANGELOG.md`、`docs/README.md` 索引
- `docs/performance/bench_baseline.md` 增 DSL 段（S0 基线：生成代码规模、编译时间、诊断响应）

## 12. 附录：S0 基线实测

命令与日期：本机 2026-09-19；`cargo clean -p forge-codegen` 后 `FGE_DEBUG_GEN=1 cargo check -p forge-codegen`。

### 12.1 规格规模（`isa/*.toml`）

| 文件 | 行数 | `[[instructions]]` | families 变体 | `[[aliases]]` | `[[lowering]]` | `[[pattern]]` | slots | forms |
| --- | ---: | ---: | ---: | ---: | ---: | ---: | ---: | ---: |
| `x86_v12.toml` | 3,356 | 146 | 51 | 25 | 220 | 2 | 15 | 19 |
| `riscv64_v12.toml` | 1,754 | 78 | 38 | 0 | 110 | 0 | 7 | 11 |
| `arm64_v12.toml` | 1,125 | 89 | 0 | 29 | 19 | 0 | 9 | 19 |
| 合计 | **6,235** | 313 | 89 | 54 | 349 | 2 | 31 | 49 |

夹具：`demo_v12` 232 行、`demo8_v12` 233 行、`demo_inst8/12/100_v12` 107/81/82 行。

### 12.2 生成代码规模（`FGE_DEBUG_GEN=1` dump，未格式化 token 串）

| 生成模块 | 字节 | 行 |
| --- | ---: | ---: |
| `forge_gen_x86_v12.rs` | 2,986,514 | 40,317 |
| `forge_gen_riscv64_v12.rs` | 1,263,470 | 21,638 |
| `forge_gen_arm64_v12.rs` | 644,018 | 9,241 |
| 合计 | **4,894,002** | **71,196** |

### 12.3 编译时间与手写测试规模

| 指标 | 实测 |
| --- | --- |
| `cargo clean -p forge-codegen` 后 `cargo check -p forge-codegen`（宏重跑 + 生成 4.9 MB 代码） | **36.7 s / 37.9 s**（两次） |
| 同上，随后空跑 `cargo check` | **0.4 s**（三次 0.81 / 0.40 / 0.41 s；紧跟 cold 的第一次 2.6 s） |
| 生成器 + 模型（`src/**` 除 `tests.rs`） | 15,976 行 |
| `src/v12/tests.rs`（crate 内单测） | 2,713 行 |
| 生成面手写测试（`*_v12_tests.rs`/`decoder_smoke`/`encoder_fuzz`/`asm_*`/`decoder_enhance`/`v12_integration`） | 2,952 行 |

> 备注：首轮测量曾得到 96.9 s / 70.6 s 的偏大值（与后台任务重叠）；上表是单独重跑的稳定值。

### 12.4 诊断行为基线（S0 取证）

命令：`cargo test -p forge-dsl --lib s0_baseline -- --nocapture`。夹具见
`crates/frontend/forge-dsl/src/v12/s0_baseline_tests.rs`（该文件**故意断言现状**，
S1/S3 落地时必须改写——它是"基线钉"）。

| 坏 spec | 现状输出 | 结论 |
| --- | --- | --- |
| 三处**互相独立**错误（未知 form + 指令名重复 + 未知 `when` 属性） | 只报第一条（`28:1: [[instructions.ADD]]: form 'NOPE' is not declared`） | **fail-fast**：一次只报一条（§2.7） |
| 无名节的错误（`[stack].align = 0`） | `1:1: v12 validation: [stack].align must be > 0` | 抽不出声明名 ⇒ **退化为 1:1**，丢失跳转（§2.7） |
| `[emit.prologue].insts = ["NO_SUCH_INST …", "@nope", "{bogus}"]` + `[spill.GPR]` 引用不存在的指令 | `<ok>`（**通过校验**） | `[emit]`/`[spill]` 名字与占位符**未校验**（§2.5） |
| 未知 form / 未知 `when` 属性 | 均带 `行:列` 与声明名，消息点名了出错的名字 | 已有基础可用，S1 补齐 span 精确性与 did-you-mean |

### 12.5 通用性守卫（S0 新增）

`crates/frontend/forge-dsl/tests/generality_guard.rs`：从 `isa/*.toml` **读出** 397 个指令名与
225 个寄存器名，再扫 `src/**`（除 `tests.rs` 与 `cfg(test)`）里的字符串字面量。

- **首次运行（空白名单）= 失败 12 处**，其中 5 处为真欠账、7 处为抽取器误报；
- 修正抽取器（`prefix` 只在 `[reg.*]` 块内取、剔除 IR 类型常量名 `F16/F32/...` 与类名撞名）后，
  真欠账 5 行（含 `lowering.rs` 的 `IntCC` 两行与 `"X1"` 缺省、`frame.rs` 的 `R10` 回退与
  `"MOV64_RM8_R64"` 回退）进白名单并注明"由 S3 删除"；
- 白名单防腐烂：每条必须被命中，S3 收尾时必须清空。

### 12.6 S1 落地后的诊断行为（与 §12.4 对照）

| 场景 | S0（改造前） | S1（现在） |
| --- | --- | --- |
| 三处独立错误（未知 form + 重名 + 未知 `when` 属性） | 只报 1 条 | **3 条一次给全**，各带 `路径:行:列: 码` |
| 重复声明 | 只报"重名"，位置落在首处声明 | 指向**后出现**的那一处 + 附注 `同名声明也出现在 行:列` |
| 未知 `when` 属性 | 位置到 `[[lowering.OP]]` 声明行 | 精确到**出错的 `when = …` 键行** |
| `[emit]`/`[spill]` 引用不存在的指令/伪指令/占位符 | **通过校验**（只查非空） | 编译期错误 + 键行位置（错误码 `DSL-EMIT`/`DSL-SPILL`） |
| 无名节的错误（`[stack].align = 0`） | 退化为 `1:1` | 落在该节块内（节头 / 键行） |
| 错误条数上限 | 无（一次一条） | 32 条 + `（另有 N 条错误未列出）` |

交付物：`v12/diag.rs`（`Diag`/`Diags`/`DeclIndex`：节级错误码、块内精准定位、附注、上限）、
`validate_all`（收集式驱动 + 指令/lowering 逐条收集）、`validate_emit_all`/`validate_spill_all`
（引用名 + 伪指令 + 占位符 + `base` 寄存器全校验）、`src/v12/diag_matrix_tests.rs`（**30 例**矩阵 +
多错并列 + 附注 + 上限 + 头条精确位置）、`docs/reference/isa-dsl-errors.md`（错误码目录）。
门禁：fmt/clippy 两道/`cargo test -p forge-dsl`（135 + 2 + 1）/`cargo clean -p forge-codegen`
后 `cargo check -p forge-codegen`（**三份发行谱通过新校验**，34.7 s）/三架构矩阵/语料/doc/markdownlint。
