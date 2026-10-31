# Code Forge

**规范驱动的编译器基础设施**：用 TOML 描述 ISA、用 EBNF 描述语言，得到可运行的编码/解码/汇编/
降级/寄存器分配/JIT 后端，并可把同一套后端作为 **rustc codegen backend** 使用。

[![CI](https://github.com/Yuanzhumoyu/code_forge/actions/workflows/ci.yml/badge.svg)](https://github.com/Yuanzhumoyu/code_forge/actions/workflows/ci.yml)

## 它是什么

- **ISA 用 TOML 描述**（`isa/*.toml`）：指令编码、位域、汇编模板、降级规则、机器事实；
  `isa_from_file!` 在编译期生成完整后端。发行后端由 [`isa/manifest.toml`](isa/manifest.toml)
  统一登记，`forge-codegen/build.rs` 生成模块、目标映射、管线注册与 alloc shim 表。
- **语言用 EBNF 描述**（`.lx`）：词法 / 语法 → CST / AST → HIR → SSA IR → 优化 → 机器码。
  `forge-grammar` 仍在工作区；`forge-hir` / `mini_c` 已降级到 `examples/hir-demo/`，只作示例。
- **三套真实后端**：x86_64、RISC-V64、AArch64；x86_64 带完整 JIT，riscv64/arm64 有
  QEMU 真执行回归。
- **rustc 后端**：`forge-rustc` 是 `-Zcodegen-backend` dylib；e2e 214/214 真编译+真执行，
  覆盖标量、控制流、调用、聚合、枚举、alloc、闭包、`dyn`、DWARF、多 CGU 与负向 compile-fail。
- **JIT 可执行内存**：W^X、Windows RX/RW dual mapping、guard pages、64 KiB 块 + 64 B 子页
  分配器（`query`/`shrink`/`reset_jit_memory`）。详见
  [`docs/reference/jit-memory.md`](docs/reference/jit-memory.md)。

## 快速开始

```rust
use code_forge::backend::arch::x86;
use code_forge::backend::jit::JitLoader;
use code_forge::ir::*;

let mut jit = JitLoader::new(x86::TargetMachine::new());
let sig = FunctionSignature::new(&[(TypeId::I32, "a"), (TypeId::I32, "b")], &[TypeId::I32]);
jit.add_function("add", &sig, |b| {
    let (entry, params) =
        b.create_block_with_params(&[(TypeId::I32, "a"), (TypeId::I32, "b")]);
    b.switch_to_block(entry);
    let sum = b.iadd(params[0], params[1]);
    b.ret(&[sum]);
})?;

let add: extern "C" fn(i32, i32) -> i32 = jit.get_fn("add")?;
assert_eq!(unsafe { add(3, 4) }, 7);
# Ok::<(), code_forge::IrError>(())
```

需要 JIT feature（见 `crates/backend/forge-codegen/Cargo.toml`）。更完整的逐指令演示见
[`examples/jit_demo.rs`](examples/jit_demo.rs)。

## Crate 地图

| Crate | 层 | 说明 |
| --- | --- | --- |
| `forge-ir` | 基础 | SSA IR、函数/模块、任意位宽整数与位模式常量、操作码元数据、二进制序列化 |
| `forge-isa-runtime` | 基础 | 生成物使用的 trait/数据型、`Registry`、可执行内存 `mem`、CPU 探测、诊断通道 |
| `forge-abi-model` / `forge-abi` | 基础 | 调用约定：规则/绑定/计划三层；约定是使用者的数据，ISA 只申报能力 |
| `forge-opt` | 中层 | 优化流水线（O1/O2/O3）与合法化；优化默认关闭直到有真执行证据 |
| `forge-codegen` | 后端 | ISA 降级、寄存器分配、发射、对象写出、JIT；发行后端由 manifest 生成 |
| `forge-dsl` / `forge-isa-dsl` | ISA 前端 | `isa_from_file!` proc-macro 与 DSL 编译器本体（模型/解析/校验/代码生成） |
| `forge-grammar` | 语言前端 | EBNF 解析、CST 构造、schema 驱动 AST 降级；下游示例已移出工作区 |
| `forge-object` | 工具 | ELF / PE / Mach-O 对象写出与重定位 |
| `forge-plugin` | 工具 | 动态 ISA 后端加载（v2 原始字节 ABI、版本握手、重定位跨 dylib） |
| `forge-rustc` | 工具 | rustc codegen backend（nightly + `rustc-dev`） |
| `forge-tests` | 工具 | 集成测试、ISA 编码黄金、JIT/汇编真执行与模糊测试 |

`forge-hir` / `forge-hir-macro` / `mini_c` 已移到 [`examples/hir-demo/`](examples/hir-demo/)，
不属于工作区门禁。

## 支持的 ISA

| ISA | 状态 | 谱 |
| --- | --- | --- |
| x86_64 | 完整后端：编码/解码/汇编/寄存器分配/JIT 真执行 | `isa/x86.toml` |
| RISC-V64 | 后端 + QEMU 真执行矩阵 | `isa/riscv64.toml` |
| AArch64 | 后端 + QEMU 真执行矩阵 | `isa/arm64.toml` |

当前**不提供**真实 riscv32/i686 后端；宽度泛化验证到 IR/JIT 编译层（demo8/demo16）。
动态产物 `cdylib/dylib/proc-macro` 仍 fail-closed；对象层有最小 DLL 链接/导出验证。

## 构建与测试

需要 nightly（edition 2024；`forge-rustc` 另需 `rustc-dev`）：

```bash
# 全量门禁（推荐；逐项报告 + 非零退出）
cargo run -p cargo-forge -- gate

# 只跑后端
cargo run -p cargo-forge -- gate --subset rustc

# 全量 workspace（除 rustc 后端）
cargo test --workspace --exclude forge-rustc
```

本机内存紧张时建议：

```bash
CARGO_HOME=target/cargo-home \
CARGO_TARGET_DIR=target/agent-licm \
CARGO_BUILD_JOBS=1 \
CARGO_PROFILE_TEST_DEBUG=0 \
cargo run -p cargo-forge -- gate
```

`forge-rustc` 的 e2e 需要仓库根 `target/debug/forge_rustc.dll`，先
`cargo build -p forge-rustc`（或 `cargo-forge backend`）。真 `std` 示例见
[`examples/forge-rustc-std-hello/run-offline.ps1`](examples/forge-rustc-std-hello/run-offline.ps1)。

## 文档

- 文档总索引：[`docs/README.md`](docs/README.md)
- ISA-DSL 规范：[`docs/reference/isa-dsl.md`](docs/reference/isa-dsl.md)；教程：
  [`docs/guides/isa-dsl-tutorial.md`](docs/guides/isa-dsl-tutorial.md)
- 后端管线契约：[`docs/reference/backend-pipeline.md`](docs/reference/backend-pipeline.md)
- 调用约定：[`docs/reference/calling-conventions.md`](docs/reference/calling-conventions.md)
- JIT 内存：[`docs/reference/jit-memory.md`](docs/reference/jit-memory.md)
- 能力缺口台账：[`docs/reference/backend-gaps.md`](docs/reference/backend-gaps.md)
- 开发约定与测试口径：[`CLAUDE.md`](CLAUDE.md)

## 许可

**MIT 或 Apache-2.0 双许可**，任选其一：

- [`LICENSE-MIT`](LICENSE-MIT)
- [`LICENSE-APACHE`](LICENSE-APACHE)

除非你明确另行声明，任何有意提交给本项目的贡献（按 Apache-2.0 的定义）均按上述双许可授权，
不附加额外条款。
