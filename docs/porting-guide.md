# 架构移植指南 — 如何为 codegen-lib 添加新 ISA 后端

本指南 step-by-step 介绍如何用 ISA-DSL v10 为新架构添加后端。

## 目录

1. [Step 1: 注册文件与元信息](#step-1)
2. [Step 2: 定义寄存器组](#step-2)
3. [Step 3: 定义 ABI](#step-3)
4. [Step 4: 定义机器指令 + emit template](#step-4)
5. [Step 5: 定义 lowering 规则](#step-5)
6. [Step 6: 终结指令 lowering](#step-6)
7. [Step 7: 序言/尾声](#step-7)
8. [Step 8: 集成测试](#step-8)
9. [快速路径：使用 SD_* 标准指令](#快速路径)

---

## <a id="step-1"></a>Step 1: 创建 TOML 文件与元信息

在 `examples/isa/` 下创建 `<arch>_v10.toml`：

```toml
[meta]
name = "my_arch"
version = "1.0"
endian = "little"       # 或 "big"
mode = 64               # 地址位宽: 32 或 64
max_inst_len = 4        # 定长指令=4, 变长=实际最大值

[meta.capabilities]
variable_length = false
prefix_layers = 0
```

参考: `examples/isa/x86_64_v10.toml` (变长 CISC), `examples/isa/riscv64_v10.toml` (定长 RISC)。

## <a id="step-2"></a>Step 2: 定义寄存器组

```toml
[reg.gpr]
count = 16          # 通用寄存器数量
width = 64          # 位宽
names = ["R0","R1",...]  # 或用 prefix = "R" 自动生成

[reg.xmm]           # 可选: 浮点/SIMD 寄存器
count = 16
width = 128
prefix = "XMM"
```

**规则**:
- `[reg.gpr]` 是必需的
- `names` 显式命名 or `prefix` 自动生成 (R0..R15)
- 生成的 `enum Reg` 实现 `PhysReg` trait (to_index/class)

## <a id="step-3"></a>Step 3: 定义 ABI

```toml
[abi]
stack_align = 16
red_zone = 128       # (可选) x86-64 SysV=128, Windows=None

[abi.frame]
sp = "RSP"           # 栈指针寄存器名
fp = "RBP"           # 帧指针 (可选)

[abi.callee_saved]
gpr = ["RBX","R12","R13","R14","R15"]
xmm = []

[abi.arg_regs]       # 参数传递寄存器 (按顺序)
gpr = ["RDI","RSI","RDX","RCX","R8","R9"]
xmm = ["XMM0","XMM1"]

[abi.ret_regs]       # 返回值寄存器
gpr = ["RAX"]
xmm = ["XMM0"]

[abi.precolor]       # 虚拟寄存器预着色 (固定映射)
VReg0 = "RAX"        # 返回值 → VReg(0)
VReg2 = "RDX"        # 除法余数 → VReg(2)
```

## <a id="step-4"></a>Step 4: 定义机器指令 + emit template

每条机器指令定义字段和编码模板：

```toml
[inst.ADD_RR]
fields = [
    { name = "dest", type = "VReg" },
    { name = "src",  type = "VReg" }
]
[inst.ADD_RR.emit]
template = "enc_rr_simple(sink, 0x01, preg(*dest, rm)?, preg(*src, rm)?);"
[inst.ADD_RR.effect]
effect = ["Pure"]
```

**字段类型**:
| 类型 | Rust 类型 | 用途 |
|------|----------|------|
| `VReg` | `VReg` | 虚拟寄存器 |
| `I64` | `i64` | 立即数 |
| `U8` | `u8` | 条件码/小常量 |
| `BlockTarget` | `i64` | 跳转目标 |
| `CondCode` | `u8` | 条件码 |

**emit template** 是 Rust 代码，可访问:
- `sink: &mut CodeSink` — 字节发射器
- `rm: &RegMap` — VReg→PReg 映射
- `preg(vreg, rm)?` → `u8` — 获取物理寄存器索引
- 指令字段: `*dest`, `imm`, `*cond` (VReg/U8 需 `*` 解引用)

**常用 sink 方法**:
- `sink.put1(u8)` / `sink.put4(u32)` — 发射固定宽度
- `sink.use_label_at(offset, block_id, reloc_kind)` — 记录重定位
- `sink.bind_label(block_id)` — 绑定标签 (在 prologue/epilogue 中)

**effect 值**:
- `"Pure"` — 无副作用 (可删除/重排)
- `"Read"` / `"Write"` — 内存访问
- `"Branch"` / `"Jump"` / `"Ret"` / `"Call"` — 控制流
- `"Trap"` — 陷阱/不可达

## <a id="step-5"></a>Step 5: 定义 lowering 规则

将 IR Opcode 翻译为机器指令序列：

```toml
[lower.Iconst]
insts = [{ inst = "MOV_REG_IMM", args = { reg = "rd", imm = "iconst" } }]

[lower.Iadd]
insts = [
    { inst = "MOV_RR", args = { dest = "rd", src = "rs1" } },
    { inst = "ADD_RR", args = { dest = "rd", src = "rs2" } },
]

[lower.Imul]
insts = [{ inst = "MUL_RR", args = { dest = "rd", src1 = "rs1", src2 = "rs2" } }]
```

**arg 关键字**:
| 关键字 | 含义 | 来源 |
|--------|------|------|
| `rd` | 目标寄存器 | `result` VReg |
| `rs1` | 第一个操作数 | `args[0]` |
| `rs2` | 第二个操作数 | `args[1]` |
| `rs3` | 第三个操作数 | `args[2]` |
| `iconst` | 整数常量 | 常量池索引 |
| `fconst` | 浮点常量 | 常量池索引 |
| `VReg(N)` | 固定 VReg | 字面量 |
| `0xNN` / `0` | 立即数 | 字面量 |

**覆盖范围**: 需为每个要支持的 IR Opcode 提供规则。
完整列表见 [docs/isa-dsl-v10.md](isa-dsl-v10.md) 的字段参考表。

若不想手动覆盖所有 opcode，可设置 `no_default_lowering = false` 使用
[标准指令集 SD_*](#快速路径) 的默认 lowering。

## <a id="step-6"></a>Step 6: 终结指令 lowering

```toml
[lower_term.Return]
insts = [
    { inst = "MOV_RR", args = { dest = "VReg(0)", src = "val" } },
    { inst = "RET", args = {} },
]

[lower_term.Jump]
insts = [{ inst = "JMP", args = { offset = "target" } }]

[lower_term.Branch]
insts = [
    { inst = "CMP_RR", args = { src1 = "cond", src2 = "VReg(0)" } },
    { inst = "JCC", args = { cond = "1", offset = "false_block" } },
    { inst = "JMP", args = { offset = "true_block" } },
]

[lower_term.Unreachable]
insts = [{ inst = "TRAP", args = {} }]
```

## <a id="step-7"></a>Step 7: 序言/尾声

```toml
[emit.prologue]
template = """
    // push rbp; mov rbp, rsp; push callee-save regs; sub rsp, fs
    enc_push_rbp(sink, rm);
    for &r in callee_save_regs() {
        enc_push_reg(sink, rm, r);
    }
    if fs > 0 { enc_sub_rsp(sink, fs); }
"""

[emit.epilogue]
template = """
    // add rsp, fs; pop callee-save; pop rbp; ret
    if fs > 0 { enc_add_rsp(sink, fs); }
    for &r in callee_save_regs().iter().rev() {
        enc_pop_reg(sink, rm, r);
    }
    enc_pop_rbp(sink, rm);
    sink.put1(0xC3);
"""
```

**可用变量**:
- `fs: u32` — 帧大小 (栈槽字节数)
- `rm: &RegMap` — 寄存器映射
- `sink: &mut CodeSink` — 字节发射器

## <a id="step-8"></a>Step 8: 集成测试

在库中添加 `isa_from_file!` 调用：

```rust
// src/backend/my_arch.rs
use codegen_dsl::isa_from_file;
isa_from_file!("examples/isa/my_arch_v10.toml");

#[test]
fn test_my_arch_codegen() {
    // 编译一个简单函数验证 lowering 管线
    let func = build_simple_add_function();
    let compiled = FunctionCompiler::<MyArchIsa>::compile_raw(&func)
        .expect("compilation failed");
    assert!(!compiled.code.is_empty());
}
```

在 `tests/` 中添加集成测试：

```rust
// tests/my_arch_tests.rs
use codegen_lib::backend::my_arch::MyArchIsa;
use codegen_lib::backend::FunctionCompiler;

#[test]
fn test_my_arch_constant() {
    let sig = Signature::new(&[], &[Type::I32]);
    let mut b = FunctionBuilder::new("test", sig);
    let entry = b.create_block();
    b.switch_to_block(entry);
    let v = b.iconst(42, Type::I32);
    b.return_(&[v]);
    let func = b.finish();
    let compiled = FunctionCompiler::<MyArchIsa>::compile_raw(&func)
        .expect("compile");
    assert!(compiled.code.len() > 0);
}
```

## <a id="快速路径"></a>快速路径：使用 SD_* 标准指令

如果你不想为每个 IR Opcode 手写 lowering 规则，可以使用内置的
[标准指令集 (SD_*)](isa-dsl-v10.md#标准指令集与默认-lowering)：

1. 定义 SD_* 指令的 emit template
2. 不设置 `no_default_lowering` (或设为 `false`)
3. 代码生成器自动为所有 IR Opcode 生成 lowering

```toml
[meta]
name = "my_riscv"
# 不设 no_default_lowering → 自动使用默认 lowering

# 只需定义标准指令的 emit template：
[inst.SD_MOV]
fields = [{ name = "dest", type = "VReg" }, { name = "src", type = "VReg" }]
[inst.SD_MOV.emit]
template = "enc_rr(sink, 0x13, preg(*dest, rm)?, preg(*src, rm)?);"
[inst.SD_MOV.effect]
effect = ["Pure"]

[inst.SD_ADD]
fields = [{ name = "dest", type = "VReg" }, { name = "src", type = "VReg" }]
[inst.SD_ADD.emit]
template = "enc_rr(sink, 0x33, preg(*dest, rm)?, preg(*src, rm)?);"
[inst.SD_ADD.effect]
effect = ["Pure"]

# ... 共 ~31 条 SD_* 指令 (见 docs/isa-dsl-v10.md 标准指令全集) ...
```

这种方法的好处：只需定义 ~31 条架构无关标准指令的 emit template，
即可自动支持全部 ~45 个 IR Opcode 的 lowering。
参考: `examples/isa/minimal_sd.toml`。

## 参考文件

| 文件 | 用途 |
|------|------|
| `examples/isa/x86_64_v10.toml` | 完整 CISC 后端参考 (~900 行) |
| `examples/isa/riscv64_v10.toml` | 完整 RISC 后端参考 (~350 行) |
| `examples/isa/minimal_sd.toml` | 最小 SD_* 后端 (~300 行) |
| `docs/isa-dsl-v10.md` | ISA-DSL v10 语法规范 |
| `src/backend/encode/mod.rs` | x86 编码辅助函数 (enc_rr_simple 等) |
| `src/backend/emit.rs` | CodeSink API |
