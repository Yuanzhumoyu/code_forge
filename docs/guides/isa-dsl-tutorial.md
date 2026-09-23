# ISA-DSL 教程：30 分钟接入一个小 ISA

> 状态：[active]（2026-09-21 起）。对应语法：[`docs/reference/isa-dsl.md`](../reference/isa-dsl.md)（v18）；
> 错误码速查：[`docs/reference/isa-dsl-errors.md`](../reference/isa-dsl-errors.md)；
> 旧语法历史：[`docs/archive/isa-dsl-v12-v17.md`](../archive/isa-dsl-v12-v17.md)。

本文造一个能跑通全链路的玩具 ISA —— **TOY16**：16 位定宽字、4 个 8 位寄存器、`add`/`sub`/`mov`/`brz`
四条指令。每一步都给出 TOML、说明它在生成物里对应什么、以及怎么立刻验证（`forge-isa` CLI 不接后端
就能校验与查看展开结果）。全部文件可放在仓库任意位置；本文按 `toy16.toml` 命名。

前置：`cargo run -p forge-isa -- validate toy16.toml`（写一点就校一次，比等到编译期快得多）。

## 0. 一次跑通的最小谱

```toml
# toy16.toml
[meta]
name = "toy16"
endian = "little"
mode = 16

[encoding]
kind = "fixed"
bits = 16

[reg.gpr1]
names = ["R0", "R1", "R2", "R3"]

[conventions.bitfields]
op  = { offset = 12, width = 4 }
rd  = { offset = 8, width = 3 }
rs1 = { offset = 5, width = 3 }
imm = { offset = 0, width = 5 }

[[operand_slots]]
name = "g"
kind = "reg"
class = "gpr1"

[[operand_slots]]
name = "imm5"
kind = "imm"
width = 5
signed = true

[[forms]]
name = "RR"
opcode_field = "op"
operand_fields = ["rd", "rs1"]

[[forms]]
name = "RI"
opcode_field = "op"
operand_fields = ["rd", "imm"]

[[instructions]]
name = "ADD"
form = "RR"
opcode = 1
ops = ["dst:g:out", "src:g"]
asm = "add {dst}, {src}"
effect = ["Pure"]

[[instructions]]
name = "SUB"
form = "RR"
opcode = 2
ops = ["dst:g:out", "src:g"]
asm = "sub {dst}, {src}"
effect = ["Pure"]

[[instructions]]
name = "MOVI"
form = "RI"
opcode = 3
ops = ["dst:g:out", "imm:imm5"]
asm = "movi {dst}, {imm}"
effect = ["Pure"]

[[instructions]]
name = "BRZ"
form = "RI"
opcode = 4
ops = ["src:g", "target:imm5"]
asm = "brz {src}, {target}"
effect = ["Branch"]
```

```console
$ cargo run -p forge-isa -- validate toy16.toml
OK（ISA = toy16）
$ cargo run -p forge-isa -- insts toy16.toml
# toy16 （version -；encoding = fixed 16 位；4 条指令 / 0 条模板 / 0 条 lowering）
ADD  ...
```

`insts` 打印的是**展开后**的指令与生效编码键——比读 TOML 更接近生成物。

### 接到 Rust 里：宿主 crate 的两处三行（v18 S10d 起必须）

生成物是 `$OUT_DIR` 下的**文件**（`isa_from_file!` 只展开成一句 `include!`），由宿主的
build script 预生成——**没有 build script 就编译不过**（fail-closed，报错里直接给这两步）：

```toml
# Cargo.toml
[build-dependencies]
forge-isa-dsl = { path = "../../frontend/forge-isa-dsl" }
```

```rust
// build.rs
fn main() {
    forge_isa_dsl::pregenerate_host().expect("ISA 预生成失败");
}
```

```rust
// src/arch/toy16.rs
forge_dsl::isa_from_file!("isa/toy16.toml");
pub use self::toy16::*;
```

为什么不让宏自己写文件：rust-analyzer 只在分析开始前加载一次文件集（宏写出的文件它
看不见，会报几十条 "unresolved import"），而且 `TokenStream::to_string()` 在 proc 宏里
与普通二进制里打印结果不同——两侧都写会互相覆盖。完整实测见
[`docs/guides/rust-analyzer-notes.md`](rust-analyzer-notes.md) §1。

最小可抄的**完整宿主**（本仓库内）= [`examples/isa-host-demo`](../../examples/isa-host-demo/README.md)：
谱 + `build.rs` + 依赖面守卫齐备，`cargo test -p isa-host-demo` 直接跑通
（运行期只依赖 `forge-isa-runtime`，连 proc-macro 都不依赖）。

## 1. 四步骨架：元信息 → 宽度 → 寄存器 → 位域

| 步骤 | 写什么 | 生成的什么 |
| --- | --- | --- |
| `[meta]` | ISA 名（`ensure_registered` 用它注册）、端序、地址位宽 | `IsaInfo::name/version/address_size/endianness` |
| `[encoding]` | `kind = "fixed"` + `bits = 16` | 每条指令的字长（字节数组 `[u8; 2]`）与编解码路径 |
| `[reg.gpr1]` | 寄存器组名 + 名字表或 `count` | `Reg` 枚举、`PhysReg` impl、寄存器名表 |
| `[conventions.bitfields]` | 位域名 → `{offset, width}` | 编解码用的位域布局（散布位段用 `pieces`） |

注意三点：

1. **寄存器组名的数字是字节宽**：`gpr1` = 1 字节，`gpr8` = 8 字节。生成物里的类/宽度/栈槽全都由它派生，
   不需要在别处再写一遍"8 字节"。
2. **位域名（`rd`/`rs1`/`imm`）不是生成的字段名**——它只决定"第 i 个操作数编到哪一段位"。生成的
   `Inst` 字段名取 `ops` 里作者声明的名字（见第 3 步）。
3. `[encoding].kind` 三态：`fixed`（全部同一字长）、`mixed`（`widths = [16, 32]` 逐指令选）、
   `prefix_scan`（x86 式前缀链）。定宽 ISA 写 `fixed` 即可。

## 2. 操作数槽：`[[operand_slots]]`

一次声明"这类操作数长什么样"，指令里按名字引用：

```toml
[[operand_slots]]
name = "g"          # 指令里写 "dst:g:out"
kind = "reg"        # reg | imm | mem | label | cond
class = "gpr1"      # 引用 [reg.gpr1]；多宽度用 classes = [...]
```

- `kind = "imm"` 的槽要写 `width`（位）与 `signed`；越界立即数在 `encode` 层**报错**而不是静默截断。
- `kind = "label"` 的槽用于分支目标：汇编接受数字偏移或符号，生成物里字段类型是 `i64`（IR 块号）。
- 多宽度寄存器槽（`classes = ["gpr4", "gpr8"]`）会让生成期自测逐宽度各生成一条用例。

## 3. 指令：`ops` 声明 + `asm` 引用

```toml
[[instructions]]
name = "ADD"
form = "RR"                                   # 可选：编码键预设
opcode = 1
ops = ["dst:g:out", "src:g"]                  # 数组序 = 编码序；角色 in/out/inout
asm = "add {dst}, {src}"                      # 只引用名字，不声明
```

- **`ops` 的名字就是生成的 `Inst` 字段名**：上面生成 `Inst::Add { dst: Reg, src: Reg }`。
  字段名不能直接当 Rust 标识符时按最小规则归一（`type` → `r#type`、`8bit` → `_8bit`）。
- **`operand_fields = ["rd", "rs1"]`（在 form 上）按下标把操作数绑到位域**：第 0 个 `dst` → `rd`、
  第 1 个 `src` → `rs1`。这也是为什么位域名与字段名可以不同、且互不影响。
- **`asm` 是必填的**，助记符就是模板首段字面量；`disassemble` 由同一个模板反向渲染。
- `effect = ["Branch"]`/`["Move"]` 等语义标签驱动 `MachineInst::is_branch/effects()`——生成器
  **不按指令名判断**。

```console
$ cargo run -p forge-isa -- explain toy16.toml ADD
ISA toy16 （encoding = fixed 16 位；4 条指令）
指令 ADD
  来源：[[instructions]] 手写指令
  生效规格：
    width_bits = 16
    len_bytes = 2
    form = RR
    opcode = 0x1
    ops = dst:g:out,src:g
    asm = add {0}, {1}
    enc = opcode_field="op",operand_fields=["rd", "rs1"]
```

`explain` 给出这条指令的完整来源（来自哪个模板哪一行、生效的编码键逐字段），是排查"我写的键到底
生效了没"的第一工具。

## 4. 复用：`[[templates]]` 是唯一的指令分组机制

同一族指令（只差 opcode/宽度/寄存器类）写成**一张表**，而不是抄 N 遍：

```toml
[[templates]]
name = "ARITH"                       # 模板名（只用于诊断/explain 溯源）
[templates.body]                     # 族级共享字段
form = "RR"
effect = ["Pure"]

[[templates.rows]]                   # 每行一条指令
inst = "ADD"
opcode = 1
asm = "add {dst}, {src}"

[[templates.rows]]
inst = "SUB"
opcode = 2
asm = "sub {dst}, {src}"
```

- 行里的键 = 指令字段空间：可以覆盖 `form`/`ops`/编码键/`roles`，`body` 提供共享值。
- 需要按行插值（如助记符带后缀）时用行键写模板，例如 `asm = "{inst.lower}.s {dst}, {src}"`。
- 别名（同一件事多个名字）用**成员指令上的 `ref`**：多条共用同一个 `ref` = 多态分发。
- 用 `cargo run -p forge-isa -- explain toy16.toml ADD` 可以确认某条指令来自哪个模板的哪一行。

## 5. 立刻拿到回归网：生成期自测

`isa_from_file!` 默认在生成模块里带 `#[cfg(test)] mod __spec_tests`——**每条指令**自动得到
encode∘decode 字节稳定、汇编↔反汇编文本幂等、立即数边界等断言，随着 TOML 自动更新：

```rust
// 发行后端：crates/backend/forge-codegen/src/arch/toy16.rs
forge_dsl::isa_from_file!("isa/toy16.toml");
pub use self::toy16::*;

// 任意普通 crate（生成物只依赖 forge-isa-runtime；v19 起没有 krate 参数）
forge_dsl::isa_from_file!("tests/isa/toy16.toml");
```

最小可抄的**完整宿主** = [`examples/isa-host-demo`](../../examples/isa-host-demo/README.md)
（谱 + build script + 依赖面守卫齐备；连 proc-macro 都不依赖，验证
`cargo test -p isa-host-demo`）。

自己**不用**手抄"这条指令编出来是不是这几个字节"：字节正确性由各 ISA 的编码参考文档 + 少量黄金值
测试守（见 `docs/reference/aarch64-encoding-ref.md` 的写法）。

多文件谱（公共骨架 + 扩展）用 `include` + `[[override]]`：

```toml
include = ["common/toy16_base.toml"]

[[override]]
key = "meta.version"
value = "0.2"
```

`cargo run -p forge-isa -- fmt toy16.toml` 能把多文件折叠成一份单文件，便于分发与 diff。

## 6. 收工前自查

```bash
cargo run -p forge-isa -- validate toy16.toml           # 全部诊断，退出码 0/1
cargo run -p forge-isa -- insts toy16.toml              # 展开后的指令 + 生效编码键
cargo test -p forge-codegen --test toy16_v12_tests      # 你的黄金值/往返测试（若写了）
```

- 诊断格式是 `路径:行:列: 错误码: 消息`，一行一条、可点击；一次列全（≤32 条）。
  常见错法与修法见 [`docs/reference/isa-dsl-errors.md`](../reference/isa-dsl-errors.md)。
- 想减薄生成物：`parts = ["encode", "decode"]` 只生成编解码器（`Inst`/`Reg` 是公共前提恒定生成；
  受限时必须 `spec_tests = false`）。

## 7. 接下来读什么

- 键的完整清单（机器校验、随 schema 同步）：[`docs/reference/isa-dsl.md`](../reference/isa-dsl.md) 的「键总览」。
- 真实规模怎么写：`isa/riscv64_v12.toml`（定宽 + 模板 + 条件码）、`isa/arm64_v12.toml`（模板 + `b.cond`）、
  `isa/x86_v12.toml`（变长前缀链 + VEX/EVEX）。
- 极端形状夹具（1 字节寄存器、12 位字、100 位字、混合字长、多文件）：
  [`crates/backend/forge-codegen/tests/isa/README.md`](../../crates/backend/forge-codegen/tests/isa/README.md)。
