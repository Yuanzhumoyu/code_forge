# CLAUDE.md

## Build Commands

```bash
# Check all crates (requires nightly)
cargo check --workspace

# Run all tests (exclude forge-rustc which needs special rustc sysroot)
cargo test --workspace --exclude forge-rustc

# Run tests with all features
cargo test --workspace --exclude forge-rustc --all-features

# Run specific crate tests
cargo test -p forge-ir
cargo test -p forge-opt
cargo test -p forge-codegen
cargo test -p forge-grammar

# Check forge-rustc (requires nightly + rustc dev components)
cargo check -p forge-rustc
```

## Project Architecture

`code-forge` is a specification-driven compiler infrastructure. Users write:
1. **ISA TOML files** (`isa/*.toml`) defining instruction encodings, register banks, and lowering rules
2. **Grammar files** (`.lx` EBNF format) defining assembler syntax

The `forge-dsl` proc-macro (`isa_from_file!`) compiles TOML → Rust code at build time, generating the complete ISA module (instructions, encoder, disassembler, assembler, lowering).

### Crate Dependency Graph

```
code-forge (root umbrella)
├── forge-ir          (no internal deps)
├── forge-mem         (no internal deps)
├── forge-opt         → forge-ir
├── forge-codegen     → forge-ir, forge-opt, forge-mem, forge-dsl, forge-grammar, forge-asm
├── forge-dsl         → forge-grammar (proc-macro)
├── forge-grammar     (no internal deps)
├── forge-asm         (zero deps)
├── forge-object      → forge-ir, forge-codegen
├── forge-plugin      → forge-codegen
├── forge-rustc       → code-forge
└── forge-tests       → code-forge
```

### Key Architecture Rules

1. **`isa_from_file!` generates code with `crate::` paths** — it expects to be called from within forge-codegen (where `crate::prelude::*`, `crate::encode::*`, `crate::assembler::*` resolve). Integration tests in `tests/` cannot use `isa_from_file!` inline; they import ISA types from forge-codegen's backend modules.

2. **Assembler/JIT coupling** — `Assembler` trait ↔ `JitCompiler` are circularly coupled. Both live in forge-codegen. Cannot split into separate crates without first refactoring to remove the cycle.

3. **Proc-macro limitation** — `forge-dsl` is a proc-macro crate; Rust prohibits proc-macro crates from exporting non-proc-macro items. Types like `MemRef` must be defined in forge-codegen, not forge-dsl.

4. **`pub use forge_asm as assembler`** — forge-codegen re-exports forge-asm as `assembler` so that DSL-generated code's `use crate::assembler::*` works.

### ISA Backend Pattern

```rust
// crates/backend/forge-codegen/src/my_isa.rs
use crate::Registry;
use crate::register_backend;

forge_dsl::isa_from_file!("isa/my_isa.toml");

pub type MyIsa = self::my_isa::Isa;
pub type MyInst = self::my_isa::Inst;

pub fn ensure_registered() {
    static INIT: std::sync::OnceLock<()> = std::sync::OnceLock::new();
    INIT.get_or_init(|| {
        if !Registry::global().contains("my_isa") {
            register_backend!(MyIsa);
        }
    });
}
```

### Frontend Pipeline (forge-grammar v21)

```text
Grammar (.lx)  ──►  Lexer + CST Parser  ──►  AST Lowering (schema-driven)  ──►  Semantic Analysis
     (runtime)        (existing, unchanged)         (NEW: AstSchema)                (NEW: SymbolTable, etc.)
```

Key types for consuming the AST:

- `AstSchema` — runtime definition of AST node shapes (mirrors `Grammar` pattern)
- `TypedAst` / `AstRef` — type-safe AST access (replaces raw `CstNode` walking)
- `AstVisitor` / `AstTransform` — read-only traversal and bottom-up rewriting
- `SymbolTable` / `NameResolver` — semantic analysis passes
- `Parser::parse_to_ast()` — one-step lex→parse→lower

Example: migrating from CST to AST:

```rust
// OLD: CST manual walking
let name = cst.flatten_child("IDENT").ok_or(...)?.text().to_string();

// NEW: Schema-driven AST
let schema = ir_schema(); // lazy_static
let ast = parser.parse_to_ast(source, &schema)?;
let name = node.get_text("name")?;
```

## HIR Lowering (forge-hir)

`forge-hir` 是手写 AST→IR 降级层（HIR = 高层 IR 图，lowering 后进 forge-ir）：

1. **op 目录**：`define_lowering!`（forge-hir-macro）只声明 atom → 后端 Opcode
   映射。每个 atom 生成 `xxx_tag()`、类型安全 `build_xxx(&mut IrGraph, 属性先,
   操作数后) -> Result<…, HirError>`（仿 Cranelift InstBuilder）、`register_atoms()`。
   `rule`/`struct` 关键字已移除——AST 降级必须手写。
2. **手写 lowering**：所有降级函数统一签名 `fn(…, ctx: &mut HirCtx, node: AstRef)
   -> Result<…, HirError>`。`HirCtx` 是唯一可变借用对象（graph + syms + loops +
   next_offset + return_slot/return_block），禁止字段拆分/闭包适配器。
3. **入口**：`lower_into_module(&graph, &registry, module, name, sig)` 一步把
   IrGraph 降级为 forge-ir Function（entry 块参数会映射为函数实参）。
4. 示例：`examples/mini_c/src/codegen_hir.rs`（与 `codegen.rs` 双后端并存，
   `compiler::Backend::{Direct, Hir}` 切换，`tests/dual_backend_tests.rs` 回归对比）。

语义约定：`alloc_slot` 发真正的 `mem.stack_addr` 节点；`load` 结果类型取自
`ty` 属性；`AttrValue::BlockId` 存 IrGraph 内部块句柄（lowering 经 block_map
转换，勿直接 cast 成 forge_ir::Block）。

## Testing Notes

- Integration tests in `tests/` import ISA types from `code_forge::backend::<isa_module>::*`
- `forge-rustc` tests require nightly Rust with `rustc-dev` component

## Code Conventions

- Edition 2024 throughout
- `forge-ir` types are re-exported in `forge-codegen::prelude` for DSL-generated code
- All generated code paths use `crate::` relative to forge-codegen
- Root `src/lib.rs` is a thin facade — all real code in `crates/`
