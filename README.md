# Code Forge

**Specification-driven compiler infrastructure** — write language grammars + ISA definitions, get a working compiler.

[![CI](https://github.com/Yuanzhumoyu/codegen-lib/actions/workflows/ci.yml/badge.svg)](https://github.com/Yuanzhumoyu/codegen-lib/actions/workflows/ci.yml)

## Architecture

```text
┌─────────────────┐  ┌──────────────────┐
│ forge-grammar   │  │ forge-dsl        │
│ (parser/CST)    │  │ (TOML→Rust code) │
└────────┬────────┘  └────────┬─────────┘
         │                    │
┌────────▼────────────────────▼─────────┐
│           forge-codegen               │
│  (lowering, regalloc, emit, JIT)      │
└────────┬──────────────────────────────┘
         │
┌────────▼────────┐  ┌──────────────────┐
│   forge-opt     │  │   forge-ir       │
│   (15+ passes)  │  │   (SSA IR types) │
└─────────────────┘  └──────────────────┘
```

## Crate Map

| Crate | Layer | Description |
| ------- | ------- | ------------- |
| `forge-ir` | Foundation | SSA IR types, function/module, constant pool, CFG analysis |
| `forge-mem` | Foundation | Executable memory (W^X), CPU feature detection, GDB JIT |
| `forge-opt` | Middle | Optimization pipeline (15+ passes), LTO, PGO |
| `forge-codegen` | Backend | ISA lowering, register allocation, emit, JIT, x86/ARM/RISC-V/WASM |
| `forge-dsl` | Frontend | `isa_from_file!` proc-macro: TOML ISA definition → Rust code |
| `forge-grammar` | Frontend | EBNF parser, CST construction, grammar-driven visitors |
| `forge-asm` | Tools | Assembler runtime types (zero-dependency) |
| `forge-object` | Tools | ELF/PE/Mach-O object file writer |
| `forge-plugin` | Tools | Dynamic ISA backend loader (.dll/.so/.dylib) |
| `forge-rustc` | Tools | rustc codegen backend (requires nightly) |
| `forge-tests` | Tools | Integration test macros and utilities |

## Quick Start

```rust
use code_forge::prelude::*;
use code_forge::backend::x86_64::X86Isa;

let mut jit = JitCompiler::<X86Isa>::new();
jit.add_function("add", &Signature::new(
    &[(Type::I32, "a"), (Type::I32, "b")], &[Type::I32]
), |b| {
    let (entry, params) = b.create_block_with_params(&[
        (Type::I32, "a"), (Type::I32, "b")
    ]);
    b.switch_to_block(entry);
    let sum = b.iadd(params[0], params[1]);
    b.ret(&[sum]);
})?;

let add: extern "C" fn(i32, i32) -> i32 = jit.get_fn("add")?;
assert_eq!(unsafe { add(3, 4) }, 7);
```

## Supported ISAs

| ISA | Status | File |
| ------- | -------- | ------ |
| x86_64 | ✅ Full backend | `isa/x86_v10.toml` |
| RISC-V64 | ✅ Full backend | `isa/riscv64_v10.toml` |
| WASM32 | ✅ 编译后端(forge-tests 测试模块已于第三十三轮移除——wasm32 编译覆盖在 forge-codegen arch/wasm32.rs 内嵌测试) | `isa/wasm32_v10.toml` |
| AArch64 | 🚧 TOML validated | `isa/aarch64_v10.toml` |
| Minimal SD | ✅ Validation only | `isa/minimal_sd.toml` |

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
