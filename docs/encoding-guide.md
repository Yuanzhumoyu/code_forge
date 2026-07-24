# 编码移植指南 — 声明式编码系统

codegen-lib 提供两种方式定义指令编码：**声明式 TOML 格式** 和 **手写 emit 模板**。

## 推荐：声明式编码 (`format = "bits"`)

在 TOML 的 `[inst.X.encoding]` 中直接声明二进制布局：

```toml
[inst.SD_ADD]
fields = [
    { name = "dest", type = "VReg" },
    { name = "src1", type = "VReg" },
    { name = "src2", type = "VReg" },
]
[inst.SD_ADD.encoding]
format = "bits"
width = 32
bitfields = [
    { value = "dest", offset = 0, width = 5 },
    { value = "src1", offset = 5, width = 5 },
    { value = "src2", offset = 16, width = 5 },
    { value = "0x22C", offset = 21, width = 10 },
    { value = "1", offset = 31, width = 1 },
]
```

### 字段说明

- `format`: `"bits"` (固定宽度) 或 `"segments"` (变长多段)
- `width`: 指令总宽度 (8/16/32/64)
- `bitfields`: 位域列表
  - `value`: 字段名 (如 `"dest"`) 或常量 (十六进制 `"0x8B"`, 十进制 `"42"`)
  - `offset`: 起始位偏移 (0 = LSB)
  - `width`: 字段宽度 (bits)

### VReg 字段自动解析

当 `value` 匹配 `fields` 中的 `VReg` 字段名时，codegen-dsl 自动生成 `preg(*field, rm)?` 调用将虚拟寄存器解析为物理寄存器编码。

### 常数字段

非字段名的 value 被解析为数字常量：

- 十六进制：`"0x8B"`, `"0X1F"`
- 十进制：`"42"`, `"31"` (常用于 XZR/SP 编码)

## 多段编码 (`format = "segments"`)

用于 x86/CISC 变长指令：

```toml
[inst.MOV_RM64_R64.encoding]
format = "segments"
segments = [
    { width = 8, fields = [{ value = "0x48", offset = 0, width = 8 }] },
    { width = 8, fields = [{ value = "0x89", offset = 0, width = 8 }] },
    { width = 8, fields = [
        { value = "dest", offset = 3, width = 3 },
        { value = "src", offset = 0, width = 3 },
        { value = "0x3", offset = 6, width = 2 },
    ]},
]
```

### 条件段 (`condition`)

段可以带有一个 `condition` 属性——一个使用字段名作为变量的 Rust 布尔表达式。仅当条件为真时才发射该段。这对于 x86 REX prefix 等可选前缀至关重要：

```toml
segments = [
    # REX prefix：仅当任一寄存器 >= 8 时发射
    { width = 8, condition = "src >= 8 || dest >= 8", fields = [
        { value = "0x40", offset = 0, width = 4 },
        { value = "src", offset = 2, width = 1, shift = 3 },
        { value = "dest", offset = 0, width = 1, shift = 3 },
    ]},
    # opcode 始终发射
    { width = 8, fields = [{ value = "0x01", offset = 0, width = 8 }] },
    # ModR/M 始终发射
    { width = 8, fields = [
        { value = "3", offset = 6, width = 2 },
        { value = "src", offset = 3, width = 3 },
        { value = "dest", offset = 0, width = 3 },
    ]},
]
```

condition 中的字段名会自动解析为：

- VReg 字段：`preg(*field_name, rm)?`（产生一个 u8 物理寄存器编号）
- 其他字段：`*field_name as i32`

### 位提取 (`shift`)

BitFieldDesc 中可选的 `shift` 字段在将值打包到位域之前对其进行右移。这对于从寄存器编号中提取特定比特（例如提取 bit 3 作为 REX 扩展位）是必需的：

```toml
# 提取 src[3]（REX.R 位）并将其放置在 REX 字节的 bit 2 位置
{ value = "src", offset = 2, width = 1, shift = 3 }
```

这生成：`((preg(*src, rm)? as u64) >> 3) & 1 << 2`

### x86_64 REX 前缀模式

x86_64 中 REX 前缀的条件发射使用该模式：

1. **段 1 (条件)**：REX 前缀字节 — `0x40 | (reg_ext << 2) | (rm_ext << 0)`，仅当 reg>=8 或 rm>=8
2. **段 2**：操作码字节
3. **段 3**：ModR/M 字节 — `mod(2) | reg[2:0](3) | rm[2:0](3)`，位 7:6=mod=3（寄存器直接模式）

完整示例（`ADD r/m8, r8`，操作码 0x01）：

```toml
[inst.ADD_RM8_R8.encoding]
format = "segments"
segments = [
    { width = 8, condition = "src >= 8 || dest >= 8", fields = [
        { value = "0x40", offset = 0, width = 4 },
        { value = "src", offset = 2, width = 1, shift = 3 },
        { value = "dest", offset = 0, width = 1, shift = 3 },
    ]},
    { width = 8, fields = [{ value = "0x01", offset = 0, width = 8 }] },
    { width = 8, fields = [
        { value = "3", offset = 6, width = 2 },
        { value = "src", offset = 3, width = 3 },
        { value = "dest", offset = 0, width = 3 },
    ]},
]
```

## 迁移步骤（从手写函数到声明式编码）

1. 识别手写函数中的位布局模式
2. 将 (sf << 31) | (opcode << 21) | (rm << 16) | (rn << 5) | rd 映射为 bitfields
3. 在 TOML 中替换 `emit = '...'` 为 `[inst.X.encoding]`
4. 运行 `cargo test` 验证编码正确性
5. 删除手写函数

## 支持的格式类型

| format | 用途 | 示例 ISA |
| -------- | ------ | --------- |
| `bits` | 固定宽度编码 | AArch64, RISC-V, WASM |
| `segments` | 变长多段编码 | x86, x86_64 |
| `rr` | x86 ModRM reg-reg | x86_64 |
| `rv_r` | RISC-V R-type | RISC-V |
| `rv_i` | RISC-V I-type | RISC-V |
