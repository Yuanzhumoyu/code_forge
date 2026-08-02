# Mini C — A C Subset Compiler Demo

Demonstrates building a complete compiler frontend→backend pipeline using:

- **[forge-grammar](../../crates/frontend/forge-grammar/)** — grammar-driven lexer/parser/CST/AST
- **[forge-codegen](../../crates/backend/forge-codegen/)** — SSA IR → x86_64 machine code → JIT execute

## Quick Start

```bash
# Build
cargo build -p mini_c

# Compile and run a Mini C program
cargo run -p mini_c -- examples/mini_c/tests/hello.c

# Run tests
cargo test -p mini_c
```

## Supported C Subset

### Types

- `int` (32-bit signed integer)

### Statements

- `return expr;` / `return;`
- `int x = expr;` — variable declaration
- `x = expr;` — assignment
- `x += expr;` `x -= expr;` `x *= expr;` `x /= expr;` `x %= expr;` — compound arithmetic
- `x &= expr;` `x |= expr;` `x ^= expr;` `x <<= expr;` `x >>= expr;` — compound bitwise
- `if (cond) { ... }` / `if (cond) { ... } else { ... }`
- `while (cond) { ... }`
- `do { ... } while (cond);`
- `for (int i = init; cond; update) { ... }`
- `break;` / `continue;`
- `enum Name { A, B, C=10, D };` — enum defs (auto-increment, explicit values, hex)
- `struct Name { int x; int y; };` — struct definitions (parsing/schema done; runtime WIP)

### Expressions

- Integer literals: `42`, `0xFF`, `0X2A`, `052` (octal)
- Character literals: `'*'` `'A'`
- Binary: `+` `-` `*` `/` `%`
- Bitwise: `&` `|` `^` `~` `<<` `>>`
- Comparison: `==` `!=` `<` `>` `<=` `>=`
- Logical: `&&` `||`
- Unary: `-` `+` `!` `~`
- Function calls: `add(40, 2)` — implemented via AST-level inlining
- Parentheses: `(expr)`

### Not Supported

- Pointers, arrays, floats, `char` type, `void`
- Struct field access at runtime (parsing + schema complete)
- Named struct types (e.g. `struct Point` — parsing + schema complete)
- `switch`, `goto`, labels
- Ternary conditional `?:`
- Comma operator
- Nested/multi-call expressions and recursive function calls
- Multi-file, preprocessor directives

## Architecture

```text
Mini C Source (.c)
    │
    ▼
grammars/mini_c.lx  ──►  parse_grammar()  ──►  Grammar
    │                                                │
    ▼                                                ▼
Parser::build()  ──►  .parse_to_ast(source, schema)  ──►  TypedAst
                                                             │
    ┌────────────────────────────────────────────────────────┘
    ▼
src/schema.rs  ◄──  AstSchema (StructBuilder + EnumBuilder)
    │
    ├─ Backend::Direct ──►  src/codegen.rs (FunctionBuilder 直接 codegen)
    │                              │
    │                              ▼
    │                     src/atoms.rs (define_lowering! op 目录)
    │                              │
    │                              ▼
    └─ Backend::Hir ─────►  src/codegen_hir.rs (IrGraph + HirCtx +
                             build_xxx 构造器 → lower_into_module)
    │
    ▼
src/compiler.rs ──►  JitCompiler::compile_module() → JIT execute
    │
    ▼
main() returns exit code
```

两个后端可用 `compiler::Backend::{Direct, Hir}` 选择：
`compile_and_run(source)` 默认走 Direct；`compile_and_run_with(source, Backend::Hir)`
走 forge-hir 管线。`tests/dual_backend_tests.rs` 对同一份源码跑两个后端并断言
JIT 结果一致，作为从 Direct 迁移到 HIR 的回归网。

## Project Structure

```text
examples/mini_c/
├── Cargo.toml
├── README.md
├── grammars/
│   └── mini_c.lx            # EBNF grammar
├── src/
│   ├── lib.rs               # Crate root
│   ├── main.rs              # CLI entry point
│   ├── grammar.rs           # Grammar loader
│   ├── schema.rs            # AST schema builder
│   ├── atoms.rs             # forge-hir op 目录 (define_lowering!)
│   ├── codegen.rs           # IR code generation (Direct 后端)
│   ├── codegen_hir.rs       # forge-hir 后端 (HirCtx + build_xxx)
│   └── compiler.rs          # Pipeline orchestration + Backend 切换
└── tests/
    ├── hello.c              # Sample C program
    ├── integration_tests.rs # Direct 后端测试套件
    └── dual_backend_tests.rs # 双后端一致性测试
```

## HIR 后端 (forge-hir)

`codegen_hir.rs` 是 `codegen.rs` 的 forge-hir 移植：AST→IR 降级使用统一的
`HirCtx`（图 + 符号表 + 循环栈 + 内联状态合并为一个可变借用对象），op 发射
通过 `define_lowering!` 生成的类型安全 `build_xxx` 构造方法。两者共享
`SymTable`/`parse_number`/枚举预扫描等纯逻辑，功能覆盖一致（for/do-while、
复合赋值、逻辑与或、位运算、一元、字面量、结构体、枚举、函数内联）。

## Known Limitations

1. **x86 backend branch conditions (i1)**: The JCC_REL32 instruction encoding
   uses SETcc-style opcode byte (e.g. 0x94 for JE) instead of Jcc near byte
   (0x84 for JE). This causes conditional branches to always fall through to
   the next block in memory. The `for` loop works because body→cond jumps
   happen to fall through correctly. The `if` statement and `while` condition
   are affected.

2. **`continue` in for loops**: May cause infinite loops due to the branch
   encoding issue.

3. **No `Call` support**: The x86 backend does not lower `Call` IR instructions.
   Cross-function calls use AST-level inlining (callee body expanded into caller).
   Nested calls and recursion are not supported.

4. **Struct field access**: Parsing and schema are fully implemented, but
   `load`/`store` after `iadd` on pointers doesn't produce correct x86 code
   for field offsets (returns pointer value instead of loaded data).

5. **`alloca` not supported**: Uses `stack_addr(0) + iadd` for local variable
   stack allocation instead of the `alloca` IR instruction.
