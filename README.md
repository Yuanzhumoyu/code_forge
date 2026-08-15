# Code Forge

**Specification-driven compiler infrastructure** — write language grammars + ISA definitions, get a working compiler.

[![CI](https://github.com/Yuanzhumoyu/codegen-lib/actions/workflows/ci.yml/badge.svg)](https://github.com/Yuanzhumoyu/codegen-lib/actions/workflows/ci.yml)

## Architecture

```text
┌─────────────────┐  ┌──────────────────┐
│ forge-grammar   │  │ forge-dsl        │
│ (EBNF→CST→AST)  │  │ (TOML→Rust code) │
└────────┬────────┘  └────────┬─────────┘
         │                    │
┌────────▼────────────────────▼─────────┐
│            forge-codegen              │
│  (lowering, regalloc, emit, JIT)      │
└────────┬──────────────────────────────┘
         │
┌────────▼────────┐  ┌──────────────────┐
│   forge-opt     │  │   forge-ir       │
│  (O1=5/O2=13/   │  │ (SSA IR + LLVM   │
│   O3=18 passes) │  │  IR 文本 parser) │
└─────────────────┘  └──────────────────┘
```

## Crate Map

| Crate | Layer | Description |
| ------- | ------- | ------------- |
| `forge-ir` | Foundation | SSA IR types, function/module, constant pool, LLVM-IR 文本 parser（logos+lalrpop）、verifier、display |
| `forge-mem` | Foundation | Executable memory (W^X), CPU feature detection, GDB JIT |
| `forge-opt` | Middle | Optimization pipeline (O1=5 / O2=13 / O3=18 passes), LTO, PGO |
| `forge-codegen` | Backend | ISA lowering, register allocation, emit, JIT, x86/RISC-V/AArch64/WASM 后端 |
| `forge-dsl` | Frontend | `isa_from_file!` proc-macro: TOML ISA 定义 → 全套后端组件代码 |
| `forge-grammar` | Frontend | EBNF 解析、CST 构造、schema 驱动 AST 降级（AstSchema/TypedAst/AstVisitor） |
| `forge-hir` | Frontend | 手写 AST→IR 降级层（IrGraph + HirCtx + lower_into_module） |
| `forge-hir-macro` | Frontend | `define_lowering!` proc-macro（atom → Opcode 映射声明） |
| `forge-asm` | Tools | 汇编器运行时类型（零内部依赖；外部依赖 logos/lalrpop-util） |
| `forge-object` | Tools | ELF/PE/Mach-O object file writer |
| `forge-plugin` | Tools | Dynamic ISA backend loader (.dll/.so/.dylib) |
| `forge-rustc` | Tools | rustc codegen backend（Windows x64 no_std；需 nightly + rustc-dev） |
| `forge-tests` | Tools | Integration test macros、ISA 编码 golden、unicorn 跨架构执行 |

## Quick Start

```rust
use code_forge::backend::arch::x86_64;
use code_forge::backend::jit::JitCompiler;
use code_forge::ir::*;

let tm = x86_64::TargetMachine::new();
let mut jit = JitCompiler::new(tm);

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

完整 JIT 集成演示见 [`examples/jit_demo.rs`](examples/jit_demo.rs)（编译→JIT→执行的逐指令管线验证）。

## Supported ISAs

| ISA | Status | File |
| ------- | -------- | ------ |
| x86_64 | ✅ 完整后端（编码+寄存器分配+JIT 执行闭环；16 个 fcmp 条件全支持） | `isa/x86_v10.toml` |
| RISC-V64 | ✅ 完整后端（编码+汇编；执行经可选 `exec-unicorn` feature） | `isa/riscv64_v10.toml` |
| AArch64 | ✅ 完整后端（编码+lowering；复杂用例 unicorn compile-only） | `isa/aarch64_v10.toml` |
| WASM32 | ✅ 编译后端（仅字节码输出，**不可运行**：无 prologue/reloc patch/validator） | `isa/wasm32_v10.toml` |
| Minimal SD | ✅ 验证 only（默认 lowering 与汇编器） | `isa/minimal_sd.toml` |

## Build

```bash
# Requires nightly Rust (edition 2024)
rustup default nightly

# Check all crates
cargo check --workspace

# Run tests (exclude forge-rustc which needs special rustc environment)
cargo test --workspace --exclude forge-rustc

# Run with all features
cargo test --workspace --exclude forge-rustc --all-features
```

## License

MIT OR Apache-2.0
