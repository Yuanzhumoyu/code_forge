# Code Forge

**Specification-driven compiler infrastructure** —— 写 ISA 定义与语言文法，得到可运行的编译器后端
（编码 / 解码 / 汇编 / 降级 / 寄存器分配 / JIT）。

[![CI](https://github.com/Yuanzhumoyu/code_forge/actions/workflows/ci.yml/badge.svg)](https://github.com/Yuanzhumoyu/code_forge/actions/workflows/ci.yml)

## 它是什么

- **ISA 用 TOML 描述**（`isa/*.toml`）：指令编码、位域、汇编模板、降级规则；`isa_from_file!`
  在编译期生成完整后端，谱与生成物同源、不可能漂移。
- **语言用 EBNF 描述**（`.lx`）：词法 / 语法 → CST / AST → HIR → SSA IR → 优化 → 机器码。
- **自带三套真实后端**：x86_64、RISC-V64、AArch64，含 JIT 与 native / QEMU 真执行回归。

## 快速开始

```rust
use code_forge::backend::arch::x86::{self, ensure_registered};
use code_forge::backend::jit::JitCompiler;
use code_forge::ir::*;

ensure_registered();
let mut jit = JitCompiler::new(x86::TargetMachine::new());

let sig = FunctionSignature::new(&[(TypeId::I32, "a"), (TypeId::I32, "b")], &[TypeId::I32]);
jit.add_function("add", &sig, |b| {
    let (entry, params) = b.create_block_with_params(&[(TypeId::I32, "a"), (TypeId::I32, "b")]);
    b.switch_to_block(entry);
    let sum = b.iadd(params[0], params[1]);
    b.ret(&[sum]);
})?;

let add: extern "C" fn(i32, i32) -> i32 = jit.get_fn("add")?;
assert_eq!(unsafe { add(3, 4) }, 7);
```

完整演示见 [`examples/jit_demo.rs`](examples/jit_demo.rs)（编译 → JIT → 执行的逐指令管线验证）。

## Crate 地图

| Crate | 层 | 说明 |
| --- | --- | --- |
| `forge-ir` | 基础 | SSA IR、函数 / 模块、常量池、操作码元数据（`ops.toml`）、LLVM-IR 文本解析 |
| `forge-abi` | 基础 | 调用约定：约定是**使用者的数据**，ISA 只申报能力 |
| `forge-mem` | 基础 | 可执行内存（W^X）、CPU 特性探测、GDB JIT |
| `forge-opt` | 中层 | 优化流水线（O1 / O2 / O3）、LTO、PGO |
| `forge-codegen` | 后端 | ISA 降级、寄存器分配、发射、JIT |
| `forge-dsl` / `forge-isa-dsl` | 前端 | `isa_from_file!` 与 DSL 编译器本体（模型 / 解析 / 校验 / 代码生成） |
| `forge-grammar` | 前端 | EBNF 解析、CST 构造、schema 驱动 AST 降级 |
| `forge-hir` / `forge-hir-macro` | 前端 | 手写 AST→IR 降级层（`IrGraph` + `HirCtx`） |
| `forge-object` / `forge-plugin` | 工具 | ELF / PE / Mach-O 目标文件写出；动态 ISA 后端加载 |
| `forge-rustc` | 工具 | rustc codegen 后端（需 nightly + `rustc-dev`） |
| `forge-tests` | 工具 | 集成测试、ISA 编码黄金、JIT / 汇编真执行与模糊测试 |

## 支持的 ISA

| ISA | 状态 | 谱 |
| --- | --- | --- |
| x86_64 | 完整后端（编码 / 解码 / 汇编 / 寄存器分配 / JIT 真执行） | `isa/x86.toml` |
| RISC-V64 | 后端 + QEMU 真执行矩阵 | `isa/riscv64.toml` |
| AArch64 | 后端 + QEMU 真执行矩阵（FPR 参数寄存器仍有已知缺口） | `isa/arm64.toml` |

## 文档

- 文档总索引：[`docs/README.md`](docs/README.md)
- ISA-DSL 规范：[`docs/reference/isa-dsl.md`](docs/reference/isa-dsl.md)、
  30 分钟接入教程：[`docs/guides/isa-dsl-tutorial.md`](docs/guides/isa-dsl-tutorial.md)
- 调用约定：[`docs/reference/calling-conventions.md`](docs/reference/calling-conventions.md)
- 开发约定与测试口径：[`CLAUDE.md`](CLAUDE.md)

## 构建与测试

需要 nightly（edition 2024）：

```bash
cargo test --workspace --exclude forge-rustc   # 主门禁（forge-rustc 另需 nightly + rustc-dev 组件）
cargo test -p forge-codegen --features jit     # JIT 真执行
```

## 许可

**MIT 或 Apache-2.0 双许可**，任选其一：

- [`LICENSE-MIT`](LICENSE-MIT)
- [`LICENSE-APACHE`](LICENSE-APACHE)

除非你明确另行声明，任何有意提交给本项目的贡献（按 Apache-2.0 的定义）均按上述双许可授权，
不附加额外条款。
