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
- `struct Name { int x; int y; };` — struct definitions（解析/运行期均已支持）

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
- `switch`, `goto`, labels
- Ternary conditional `?:`
- Comma operator
- Recursive function calls（调用经 AST 级内联实现，递归会无限内联）
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

1. ~~**x86 backend branch conditions (i1)**~~ **已修复（2026-08-03）**。旧记录称
   `JCC_REL32` 用 SETcc 风格 opcode（JE=0x94）导致条件分支总是 fall-through。
   实际当前 `[cc_names]` 已用正确的 Jcc 值（JE=0F 84），`if`/`while`/`for` 的
   条件分支均正确（`test_hir_e2e_if_else`、`test_hir_e2e_loops` 通过）。残留
   小问题：`o/no`（jo/jno）两行曾是 SETcc 值（0x90/0x91），已修为 0x80/0x81。

2. ~~**No `Call` support**~~ **已支持（2026-08-03）**。x86 后端已实现
   `[lower.Call]`（ABI 参数移动 + relocatable CALL + 返回值移动）+ Windows x64
   shadow space（本次补充）；跨函数调用经 `JitCompiler::compile_module` 的
   relocation 解析。mini_c 的 `add(2,3)` 跨函数调用测试通过
   （`test_hir_e2e_params`）。

3. ~~**Struct field access**~~ **已支持（2026-08）**。旧实现依赖
   `load`/`store` 在 `iadd(ptr, offset)` 后的 x86 编码（返回指针值而非数据）；
   现改为**每字段独立栈槽**（`alloc_struct_fields`，src/codegen.rs），字段访问
   不再依赖指针算术。9 个 struct 集成测试 + `dual_backend_tests::both_structs`
   双后端一致通过。

4. **`alloca` not supported**: Uses `stack_addr(0) + iadd` for local variable
   stack allocation instead of the `alloca` IR instruction.

5. **JIT 栈布局曾有大类 bug（已修复，2026-08-03）**: 局部变量槽曾落在
   callee-saved push 槽内（`callee_saved_bytes` 漏计帧指针）、i32 的 load/store
   曾固定 64 位访问导致相邻槽重叠、frame 大小未包含局部变量区（槽落在 rsp
   之下）、`$modrm_mem_rr` 对 rbp/r13 基址误编 RIP-rel 且缺 SIB 语法。全部
   修复后 `cargo test -p mini_c` 全绿（见 BENCHMARKS.md Known Issues）。
