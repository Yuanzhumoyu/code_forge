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
├── forge-codegen     → forge-ir, forge-opt, forge-mem, forge-dsl, forge-asm
├── forge-dsl         → forge-grammar (proc-macro; forge-grammar 经此传递使用)
├── forge-grammar     (no internal deps)
├── forge-hir-macro   (no internal deps)
├── forge-hir         → forge-ir, forge-grammar, forge-hir-macro
├── forge-asm         (零内部依赖；外部依赖 logos/lalrpop-util)
├── forge-object      → forge-ir, forge-codegen
├── forge-plugin      → forge-codegen
├── forge-rustc       → code-forge (强制 object-file/plugins features)
└── forge-tests       → code-forge, forge-rustc(optional, nightly feature)
```

### Key Architecture Rules

1. **`isa_from_file!` generates code with `crate::` paths** — it expects to be called from within forge-codegen (where `crate::prelude::*`, `crate::encode::*`, `crate::assembler::*` resolve). Integration tests in `tests/` cannot use `isa_from_file!` inline; they import ISA types from forge-codegen's backend modules.

2. **Assembler/JIT coupling** — `Assembler` trait ↔ `JitCompiler` are circularly coupled. Both live in forge-codegen. Cannot split into separate crates without first refactoring to remove the cycle.

3. **Proc-macro limitation** — `forge-dsl` is a proc-macro crate; Rust prohibits proc-macro crates from exporting non-proc-macro items. Types like `MemRef` must be defined in forge-codegen, not forge-dsl.

4. **`pub use forge_asm as assembler`** — forge-codegen re-exports forge-asm as `assembler` so that DSL-generated code's `use crate::assembler::*` works.

### ISA Backend Pattern

```rust
// crates/backend/forge-codegen/src/my_isa.rs
forge_dsl::isa_from_file!("isa/my_isa.toml");
pub use self::my_isa::*; // 生成 TargetMachine / Inst / IsaInfo 等全套组件
// 注册由 DSL 生成的 ensure_registered() 完成（OnceLock 注册 Registry + reloc patcher），
// 无需手写——见 arch/x86_64.rs 的实际形态。
```

> 注：生成模块导出的是 `TargetMachine`（组合 RegInfo/ABI/Lowering/Encoder/
> FrameLowering/Disassembler/Assembler），**没有 `Isa` 类型**；无 `register_backend!` 宏。

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

## SIMD 支持矩阵（x86_64，isa/x86_v10.toml）

| 维度 | 支持 | 说明 |
|---|---|---|
| 长度 | V64（2×f32）、V128（4×f32）、V256（8×f32，AVX）/（8×i32、4×i64，AVX2） | 动态 `vector_ty(elem, len)` 与内建去重；`<3 x f32>` 等非 2 幂长度用 128 位指令低 lane 语义（未用 lane 无定义）；V256 测试在无 AVX/AVX2 机器自动 skip |
| 元素 | f32/f64/i32/i64 | vadd/vsub/vneg 全元素（f32→addps、f64→addpd、i32→paddd、i64→paddq/psubq；V256 整数走 AVX2 vpaddd/vpsubd/vpaddq/vpsubq）；vmul 浮点 + i32（PMULLD/VPMULLD）；i64 vmul/vdiv 与整数 vdiv 无 SIMD 指令 → 编译期 Unsupported；vabs 用按位掩码（andps 0x7FFFFFFF×4）对 f64/i64 亦正确 |
| 运算 | vconst/vconst_array/vadd/vsub/vmul/vdiv/vneg/vabs/vbitcast/vextract（全 lane）/vinsert/vbroadcast/vsplit/vconcat/shuffle_vector | `vconst<T: Vector>(Vec<T>)` 泛型值语义（动态数组，ty 由 T+长度推导）；`vconst_array([T; N])` 静态数组；`vconst_bytes(Vec<u8>, ty)` 底层字节 API（元素 LE 字节序，用户自定义 `Vector::lane_bytes` 即可接入）；shuffle_vector：V128 单 shufps、V256 拆半双 shufps（mask 组内语义）；vbroadcast 64 位元素用 vbroadcastsd |
| 常量 | 扁平字节池（`Vec<u8>` + offset 表 + 每段端序 `vec_endian`） | `vconst<T: Vector>(Vec<T>)` 泛型值语义（ty 由 T+len 推导，默认 Little）、`vconst_array([T; N])` 静态数组、`vconst_bytes` 底层字节；**端序**：`Vector::lane_bytes(endian)`（Little→to_le、Big→to_be，u8..u128/f32/f64 全位宽，u128 16 字节不截断）、`vconst_with_endian`/`vconst_bytes_with_endian` 显式端序（大端框架数据）、`ConstantPool::get_vector_endian` 查询；DSL 按常量端序还原（LE→from_le、BE→from_be，32 位元素逐元素 BE 读）；rodata 数据段加载为长期优化 |
| ABI | 参数/返回仅 ≤128 位向量 | `>128` 位向量传参编译期 Unsupported（compile 入口守卫）；函数内 vconcat/vsplit 组合；YMM 传参需 ABI 层扩展（长期） |
| 编码 | SSE（0F/0F38 前缀族）+ AVX（VEX C4：`@vex_rrvvv`/`@vex_rrvvv_imm` 宏，`avx_available()`）+ AVX2（`@vex_rrvvv_avx2` 宏，`avx2_available()`） | VEX 三操作数 r/m=src2、vvvv=~src1（`dest src2 src1 1`）；无源指令 vvvv 编码 1111；vextractf128 的 dest 在 r/m、src 在 reg；vzeroupper 无需（Windows x64 ABI 允许破坏 YMM 高半） |

DSL variants 谓词：`rs1`/`rd`（位宽，动态类型按 size_bytes）、`elem`（元素 TypeId）、`immN`（立即数），支持 `&&`/`||` 复合与括号（`parse_when_cond` 子条件加括号防 `&&` 优先级高于 `||` 致 guard 误真——有回归单测）。

## Code Conventions

- Edition 2024 throughout
- `forge-ir` types are re-exported in `forge-codegen::prelude` for DSL-generated code
- All generated code paths use `crate::` relative to forge-codegen
- Root `src/lib.rs` is a thin facade — all real code in `crates/`
