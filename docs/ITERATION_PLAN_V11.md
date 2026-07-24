# codegen-lib 迭代清单 v11

> 创建日期: 2026-07-23
> 状态: 所有 242 测试通过
> 上一迭代: v10 (PhysReg/Reg 类型、序言/尾声汇编化、bitstring 条件修复)

---

## 当前架构概览

```
TOML ISA 定义
  ├── [inst.*]        → Inst 枚举 + MachineInst impl + emit_inst (编码) + disasm (反汇编)
  ├── [enc_macros.*]  → bitstring 宏展开 → 生成 pack_bits() 调用
  ├── [emit.prologue] → 汇编序列 (@move_args, PUSH64_R, MOV64_RR, ...)
  ├── [emit.epilogue] → 汇编序列 (@frame_free, @pop_callee, POP64_R, RET)
  ├── [lower.*]       → IR Opcode → 机器指令序列 (rd, rs1, rs2 位置映射)
  ├── [lower_term.*]  → Terminator → 机器指令序列
  └── [lower_pattern.*] → IR pattern → 优化后的机器指令序列
```

---

## I1: 指令关联性 — 表达式内的临时值/标签/常量

### 问题分析

当前 lowering 序列中的指令通过**位置参数** (`rd`, `rs1`, `rs2`) 关联，这只适用于简单的单表达式 lowering。对于复杂表达式，需要一个 lowering 序列内部的**临时值传递机制**。

**当前** (简单 Add):
```toml
[lower.Iadd]
insts = ["MOV_R8_RM rd, rs1", "ADD_RM8_R8 rd, rs2"]
```

**问题场景** — 无法表达的复杂表达式:
```
复杂表达式: (a * b) + (c / d)
需要: t1 = a * b; t2 = c / d; result = t1 + t2
但是 lowering 只有 rd, rs1, rs2 三个位置参数，无法创建中间临时值
```

**当前处理方式**: 每个 IR 指令独立 lower，中间值由 regalloc 处理。这其实是正确的设计 — lowering 是 1:1 的 IR→机器指令映射，复杂表达式在 IR 层已分解。**但序言/尾声的 `@` 伪指令和分支 lowering 确实需要改进**。

### I1 子任务

| ID | 任务 | 描述 | 难度 |
|----|------|------|------|
| I1a | **Lowering 序列中的命名临时 VReg** | 支持 `%tmp = MOV_R8_RM rs1` 语法，后续指令引用 `%tmp` | 中 |
| I1b | **Lowering 序列中的标签** | 支持 `.L0:` 标签 + `JMP_REL32 .L0` 跳转，用于多出口 lowering | 中 |
| I1c | **常量池引用改进** | 当前 iconst/fconst 通过 `ctx.constant_pool` 的 `index` 访问 — 需支持 `{const 42}` 内联语法 | 低 |
| I1d | **条件汇编伪指令** | 支持 `.if is_float_return` / `.endif` 条件块 | 低 |

### I1a 详细设计: 命名临时 VReg

**现状**: `parse_asm_inst` 识别 `rd`, `rs1`, `rs2`, `VReg(N)`, 字面量
**新增**: `%name` 语法 — `%` 前缀表示临时变量

```toml
# 示例: Branch lowering 中使用临时变量 (未来扩展)
[lower_term.Branch]
insts = [
    "TEST_RM8_R8 cond, cond",
    "JCC_REL32 0x84, %false_target",
    "JMP_REL32 %true_target",
]
```

在 `gen_lower_insts` 中，`%name` 被解析为局部变量：
- 首次出现 (作为 def) → `let %name = ctx.alloc_vreg()`
- 后续引用 (作为 use) → 使用已分配的 VReg

**实现步骤**:
1. `parse_asm_inst` 识别 `%name` 语法
2. `gen_lower_insts` 收集 `%` 变量 → 生成 `let __tmp0 = ctx.alloc_vreg();`
3. 引用 `%name` → 映射回 VReg 变量

### I1b 详细设计: Lowering 内部标签

**现状**: Branch/Jump 的 target 由 `lower_terminator` 提供 (`true_block`, `false_block`)，直接映射到 IR block
**新增**: Lowering 序列内部标签，用于需要多个出口的复杂分支

```toml
# 设想: Switch 的 if-else 链展开
[lower_term.Switch]
insts = [
    "CMP_RM8_R8 discriminant, VReg(97)",
    "JCC_REL32 0x94, .L_case0",
    "CMP_RM8_R8 discriminant, VReg(98)",
    "JCC_REL32 0x94, .L_case1",
    "JMP_REL32 .L_default",
]
```

**实现**: 标签 `.L_name` 转换为 VCode 内部的临时 label。

---

## I2: 指令映射符合汇编规范

### 问题分析

当前有三个层面的规范问题：

#### 2a. 字段顺序 ≠ 汇编操作数顺序

由于 TOML inline table 使用 BTreeMap，字段按**字母序**排列：

```toml
# MOV_REG_IMM64 的实际字段顺序 (alpha):
fields = { imm = "I64", reg = "VReg" }   # BTreeMap: imm < reg
asm = "mov {reg}, 0x{imm:x}"              # 模板写 reg 在前
# 但在 lowering/emit 中，操作数顺序是 imm, reg !
```

**影响**:
- `"MOV_REG_IMM64 0x42, rd"` → 第一个操作数是 `imm`，第二个是 `reg`
- Intel 语法 `mov rax, 0x42` → 第一个是 dest，第二个才是 imm
- 操作数顺序与汇编惯例不一致！

#### 2b. 指令名是内部编码名

`ADD_RM8_R8`, `MOV_R8_RM`, `JCC_REL32` — 这些名字反映的是编码细节 (R=register, M=modrm, 8=64bit)，而非用户友好的汇编助记符。不过 `asm` 模板已提供正确的反汇编输出，指令名只是内部标识。

#### 2c. 操作数修饰符缺失

x86 汇编中常见:
- `mov rax, qword ptr [rbx]` — 大小修饰符
- `add rax, 0x10` vs `add eax, 0x10` — 操作数大小决定操作
- 当前编码宏不区分 `mov` (32-bit) 和 `mov` (64-bit)

### I2 子任务

| ID | 任务 | 描述 | 难度 |
|----|------|------|------|
| I2a | **字段有序化 — 数组语法** | TOML `fields` 支持数组格式 `[{name="dest", type="VReg"}, ...]`，保持声明顺序 | 低 |
| I2b | **操作数位置映射文档化** | 在 TOML 注释中明确标注操作数顺序 | 低 |
| I2c | **汇编风格检查器** | 编译时验证 asm 模板中的 `{field}` 顺序与字段声明一致 | 低 |
| I2d | **操作数大小修饰符** | asm 模板支持 `{dest:q}` (qword) 等修饰符 | 中 |

### I2a 详细设计: 字段有序化

**当前**: TOML inline table → BTreeMap → 字母序
```toml
fields = { dest = "VReg", src = "VReg" }    # alpha: dest, src ✓ (巧合)
fields = { imm = "I64", reg = "VReg" }       # alpha: imm, reg ✗ (反直觉)
```

**改为数组** (已有 parser 支持 `deser_fields` 的 `Array` 分支):
```toml
fields = [
    { name = "reg", type = "VReg" },   # 第一个操作数 = reg
    { name = "imm", type = "I64" },    # 第二个操作数 = imm
]
```

**好处**:
- 操作数顺序 = 声明顺序 (Intel 语法: dest, src)
- 不再受 BTreeMap 字母序困扰
- 更清晰的意图表达

**迁移影响**: 所有现有 TOML 的 `fields = { ... }` 需迁移为数组格式。但 `deser_fields` 已同时支持两种格式。

---

## I3: enc_macros 条件可读性

### 问题分析

当前 `enc_macros` 将所有 bitfield 和条件写在一行内，条件复杂时极难阅读：

```
# 可读性极差 — 12 个 {} 在一行内
pattern = "?${reg}>=8||${rm}>=8 {0x4:[4;4]}{1:[3;1]}{${reg}:[2;1;3]}{0:[1;1]}{${rm}:[0;1;3]} {${opcode}:[0;8]} {${rm}:[0;3]}{${reg}:[3;3]}{3:[6;2]}"
```

**问题**:
1. **魔法数字**: `0x4:[4;4]` — 为什么是 0x4? 为什么是 [4;4]? 需要 x86 手册才能理解
2. **条件表达式**: `?${reg}>=8||${rm}>=8` — 嵌入在字节序列中难以发现
3. **无注释**: 无法在 pattern 中添加注释说明每个 bitfield 的含义
4. **多条件分支**: `?${reg}>=8 {0x49} ?${reg}<8 {0x48}` — 两个分支写在一起像正则表达式

### I3 子任务

| ID | 任务 | 描述 | 难度 |
|----|------|------|------|
| I3a | **命名字段常量** | 定义 `REX_BASE = 0x4`, `MODRM_MOD_MASK = 3` 等可复用常量 | 中 |
| I3b | **多行 pattern 支持** | pattern 支持换行，每行一个 segment，提高可读性 | 低 |
| I3c | **pattern 内注释** | 支持 `# 注释` 在 pattern 内 | 低 |
| I3d | **条件分支可视化** | 多条件用缩进/分组表示 | 高 |

### I3a 详细设计: 命名字段常量

新增 TOML section `[enc_constants]`:
```toml
[enc_constants]
REX_PREFIX  = "0x4"
REX_W       = "1"
REX_R       = "${reg}"
REX_X       = "0"
REX_B       = "${rm}"
MODRM_MOD   = "3"
OP_ADD_RM_R = "0x01"
```

然后 `enc_macros` 引用常量:
```toml
[enc_macros.rex_modrm_rr]
params = ["opcode", "reg", "rm"]
pattern = """
    ?${reg}>=8||${rm}>=8 {
        $REX_PREFIX:[4;4]    # 0100
        $REX_W:[3;1]         # W=0 (32-bit default)
        ${reg}:[2;1;3]       # R (reg bit3)
        $REX_X:[1;1]         # X=0
        ${rm}:[0;1;3]        # B (rm bit3)
    }
    {${opcode}:[0;8]}
    {${rm}:[0;3]}            # ModRM.rm
    {${reg}:[3;3]}           # ModRM.reg
    {$MODRM_MOD:[6;2]}       # ModRM.mod = 11b (register mode)
"""
```

### I3b 详细设计: 多行 pattern

当前 parser: `expand_macros` 在空格处分隔 segment。多行支持只需:
1. TOML 多行字符串 `"""..."""` 自然支持换行
2. `parse_segmented` 的 `skip_ws` 已跳过 whitespace (包括换行)
3. 无需改动 parser！

**唯一需要验证的**: macro expander 是否在换行处正确处理。当前 `expand_macros` 在检测到 `{`, `!`, `?`, `$`, `[` 时停止参数收集 — 换行也是合法分隔符。

### I3c 详细设计: pattern 内注释

在 `expand_macros` 或 `expand_encoding` 开头添加预处理步骤:
```rust
fn strip_comments(input: &str) -> String {
    input.lines()
        .map(|line| {
            // Remove `# ...` comments, but keep `#` inside bit specs
            if let Some(pos) = line.find('#') {
                // Only strip if # is preceded by whitespace or at line start
                if pos == 0 || line.as_bytes()[pos - 1].is_ascii_whitespace() {
                    &line[..pos]
                } else {
                    line
                }
            } else {
                line
            }
        })
        .collect::<Vec<_>>()
        .join("\n")
}
```

---

## I4: bitstring 条件编码修复 (遗留)

### 问题

`$sub_rm_imm` / `$add_rm_imm` 的 bitstring 条件段在**多字节编码**场景下失效。

**根因**: 一个 segment 中的所有 `{...}` 组（无空格分隔）被视为**一个 segment**，所有字段共享同一个 `pack_bits` 调用。对于多字节指令（如 `{0x83:[0;8]}{5:[3;3]}{rm:[0;3]}...`），所有字段被打包到**同一个** width 中，导致值被 OR 在一起。

**当前 workaround**: SUB64_R_IMM32 / ADD64_R_IMM32 无 encoding，使用 direct sink fallback。

### I4 子任务

| ID | 任务 | 描述 | 难度 |
|----|------|------|------|
| I4a | **同一条件多段支持** | `?... {seg1} {seg2}` — 条件应用于多个连续段 | 中 |
| I4b | **恢复 SUB64/ADD64 的 encoding** | 修复后为 SUB64_R_IMM32 / ADD64_R_IMM32 添加正确的 bitstring encoding | 低 |

### I4a 详细设计

当前 `parse_segmented` 将空格分隔的 `{...}` 组视为不同 segment。每个 segment 可以有自己的 condition。但如果多个连续 segment 共享同一条件，需要重复写条件表达式。

**方案**: 引入条件组语法 `?: condition [ ... ]`:
```
pattern = "?: ${imm}<128 [{0x83:[0;8]} {5:[3;3]}{${rm}:[0;3]} {${imm}:[0;8;4]}] {0x81:[0;8]} {5:[3;3]}{${rm}:[0;3]} {${imm}:[0;8;4]} {${imm}:[0;8;12]} {${imm}:[0;8;20]} {${imm}:[0;8;28]}"
```

但这引入了新语法。更简单的方案：**让 parser 支持多个连续的 `?...{seg}` 段合并为一个 if-else 链**。

---

## I5: 测试改进 — 多指令复杂场景

### 问题

当前测试主要是单指令 encode/disasm 验证，缺少：
- 完整函数编译后的逐字节验证
- 多基本块 CFG 测试
- 栈帧布局验证

### I5 子任务

| ID | 任务 | 描述 |
|----|------|------|
| I5a | 多块 CFG 编译测试 | if-else / loop / switch 的完整编译 + 代码检查 |
| I5b | 栈帧布局验证 | 验证 prologue 后 RSP 对齐、spill 区域正确 |
| I5c | 边界值编码测试 | INT_MIN, INT_MAX, NaN, Inf 等边界常量的 encoding |

---

## 实施排序

按影响面和依赖关系排列：

| 优先级 | ID | 说明 |
|--------|----|------|
| P0 | I2a | 字段有序化 — 解决操作数顺序混乱，影响所有后续 TOML 编写 |
| P0 | I3b | 多行 pattern — parser 已天然支持，仅需验证 |
| P1 | I3c | pattern 注释 — 小改动，大幅提升可读性 |
| P1 | I3a | 命名字段常量 — 消除魔法数字 |
| P1 | I1a | 命名临时 VReg — 扩展 lowering 表达能力 |
| P2 | I1b | Lowering 内部标签 |
| P2 | I1c | 常量池引用改进 |
| P2 | I4a | 多段条件修复 — 恢复 SUB64/ADD64 encoding |
| P3 | I2c | 汇编风格检查器 |
| P3 | I5a-c | 测试改进 |

---

## 快速验证命令

```bash
cargo check -p codegen-dsl          # DSL 编译检查
cargo test -p codegen-dsl           # DSL 单元测试 (29 tests)
cargo test --test disasm_tests      # 反汇编测试 (22 tests)
cargo test --test encoder_tests     # 编码器测试 (28 tests)
cargo test --test asm_integration_tests  # 汇编集成测试 (14 tests)
cargo test --test fuzz_lowering     # 模糊测试 (11 tests)
cargo test --test jit_integration   # JIT 集成测试 (137 tests)
```
