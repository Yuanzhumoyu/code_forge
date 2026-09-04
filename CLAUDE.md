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

```text
code-forge (root umbrella)
├── forge-ir          (no internal deps)
├── forge-mem         (no internal deps)
├── forge-opt         → forge-ir
├── forge-codegen     → forge-ir, forge-opt, forge-mem, forge-dsl
├── forge-dsl         (proc-macro; 无内部依赖——v12 自包含，不依赖 forge-grammar/lalrpop)
├── forge-grammar     (no internal deps)
├── forge-hir-macro   (no internal deps)
├── forge-hir         → forge-ir, forge-grammar, forge-hir-macro
├── forge-object      → forge-ir, forge-codegen
├── forge-plugin      → forge-codegen
├── forge-rustc       → code-forge (强制 object-file/plugins features)
└── forge-tests       → code-forge, forge-rustc(optional, nightly feature)
```

### Key Architecture Rules

1. **`isa_from_file!`（v12 唯一语法）generates code with `crate::` paths** — it expects to be called from within forge-codegen (where `crate::prelude::*`, `crate::machine::*` resolve)。生成模块名 = 文件 stem。Integration tests in `tests/` cannot use `isa_from_file!` inline; they import ISA types from forge-codegen's backend modules.

2. **Assembler/JIT coupling** — `Assembler` trait ↔ `JitCompiler` are circularly coupled. Both live in forge-codegen. Cannot split into separate crates without first refactoring to remove the cycle.

3. **Proc-macro limitation** — `forge-dsl` is a proc-macro crate; Rust prohibits proc-macro crates from exporting non-proc-macro items. Types like `MemRef` must be defined in forge-codegen, not forge-dsl.

4. **v12 自包含 asm** — v12 生成模块内联实现 assemble（表驱动，首词=mnemonic），不再经 lalrpop 语法与 forge-asm 运行时（v11 时代已随语法层删除）。`TargetAssembler` trait（`crate::machine::assembler`）仅要求 `parse_insts`。

### ISA Backend Pattern

```rust
// crates/backend/forge-codegen/src/arch/my_isa.rs
forge_dsl::isa_from_file!("isa/my_isa.toml");
pub use self::my_isa::*; // 生成 TargetMachine / Inst / Reg 等全套组件
// 注册由 DSL 生成的 ensure_registered() 完成（OnceLock 注册 Registry + reloc patcher），
// 无需手写——见 arch/x86_v12.rs 的实际形态。
```

> 注：生成模块导出的是 `TargetMachine`（组合 IsaInfo/RegInfo/ABI/Lowering/Encoder/
> FrameLowering/Disassembler/Assembler/**Decoder**），**没有 `Isa` 类型**；
> 无 `register_backend!` 宏。v11 后端（x86_64/aarch64/riscv64/wasm32/minimal_sd）
> 已随 v11 语法层删除——现仅 `arch/x86_v12.rs` 与 `arch/riscv64_v12.rs`，二者均
> 已接 TargetMachine（riscv 定宽试点，QEMU 真执行矩阵 126 用例全绿）。

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
- **JIT 集成矩阵**（`forge-tests/src/jit_matrix.rs`）：架构无关、一次编写——
  用例**零 ISA 引用**，ISA 只存在于薄 runner（`isa/<name>/` 绑定机器 +
  能力集 `Capabilities`，riscv64 未来接入复用）。当前 **x86_v12 193 passed /
  3 skipped、riscv64_v12 131 passed / 65 skipped，0 failed**：整数/浮点/调用/
  向量/饱和/指针转换/undef/poison/GlobalAddr/原子（AtomicRmw/Cmpxchg）/GEP/Nop/
  混宽整数算术与比较；V256 用例在无 AVX 机器
  自动 Skip。`CaseKind::{I32/I64/F64/Bool/Block/Args/F64Args/Module/
  CompileOnly}`；`ops` 未覆盖 → Skip（不失败，实现后自动转绿）。
  测试入口：`cargo test -p forge-tests jit_matrix_x86_v12`。
- **TOML 改动**：改 `isa/*.toml` 直接触发重编译——生成模块内嵌
  `include_bytes!(<TOML 绝对路径>)`，rustc 据此登记编译依赖（不再需要手动 touch
  `arch/<isa>.rs`）。`FGE_DEBUG_GEN=1` 可 dump 生成代码到 `%TEMP%\forge_gen_*.rs`。

## SIMD 支持矩阵（x86_64，isa/x86_v12.toml）

| 维度 | 支持 | 说明 |
| --- | --- | --- |
| 长度 | V64（2×f32）、V128（4×f32）、V256（8×f32，AVX）/（8×i32、4×i64，AVX2） | 动态 `vector_ty(elem, len)` 与内建去重；`<3 x f32>` 等非 2 幂长度用 128 位指令低 lane 语义（未用 lane 无定义）；V256 测试在无 AVX/AVX2 机器自动 skip |
| 元素 | f32/f64/i32/i64 | vadd/vsub/vneg 全元素（f32→addps、f64→addpd、i32→paddd、i64→paddq/psubq；V256 整数走 AVX2 vpaddd/vpsubd/vpaddq/vpsubq）；vmul 浮点 + i32（PMULLD/VPMULLD）；i64 vmul/vdiv 与整数 vdiv 无 SIMD 指令 → 编译期 Unsupported；vabs 用按位掩码（andps 0x7FFFFFFF×4）对 f64/i64 亦正确 |
| 运算 | vconst/vconst_array/vadd/vsub/vmul/vdiv/vneg/vabs/vbitcast/vextract（全 lane）/vinsert/vbroadcast/vsplit/vconcat/shuffle_vector | `vconst<T: Vector>(Vec<T>)` 泛型值语义（动态数组，ty 由 T+长度推导）；`vconst_array([T; N])` 静态数组；`vconst_bytes(Vec<u8>, ty)` 底层字节 API（元素 LE 字节序，用户自定义 `Vector::lane_bytes` 即可接入）；shuffle_vector：V128 单 shufps、V256 拆半双 shufps（mask 组内语义）；vbroadcast 64 位元素用 vbroadcastsd |
| 常量 | 扁平字节池（`Vec<u8>` + offset 表 + 每段端序 `vec_endian`） | `vconst<T: Vector>(Vec<T>)` 泛型值语义（ty 由 T+len 推导，默认 Little）、`vconst_array([T; N])` 静态数组、`vconst_bytes` 底层字节；**端序**：`Vector::lane_bytes(endian)`（Little→to_le、Big→to_be，u8..u128/f32/f64 全位宽，u128 16 字节不截断）、`vconst_with_endian`/`vconst_bytes_with_endian` 显式端序（大端框架数据）、`ConstantPool::get_vector_endian` 查询；DSL 按常量端序还原（LE→from_le、BE→from_be，32 位元素逐元素 BE 读）；rodata 数据段加载为长期优化 |
| ABI | **≤16B（V64/V128）按值 XMM 全宽 + >16B（V256）by-ref/sret 全线支持**（2026-09 D3 补齐 VEC(16) 全宽） | Windows x64 无 YMM 参数寄存器——>8B 非标量按引用传指针（占 GPR 槽）、返回 >64 位走首参 RCX 隐藏 sret；≤16B 向量（VEC(16) 类）按值进 XMM{pos}（by-position 槽）：收参/实参/返回用**全宽 128 位 MOVAPS**（ABI 键 `[abi].vec_mov_inst`，缺省 "MOVAPS"——MOVSD/MOVSS 只移 8/4B 会静默截断高半，WA-37 D3 修复）。Load/Store 谓词加 `rd_vec`/`rs1_vec`（向量类型字节数）→ V128 走 MOVUPS_128（0F 10/11 无前缀 16 字节）、V64 走 movsd 8B。jit 12+ 测试绿（v128/v64 byval param/return、mixed、wide byref/sret、v512）；forge-rustc B3 门控已撤、simd_v128/v64/v256 e2e 全绿。**残余**：宽向量第 5+ GPR 槽显式 Unsupported；V512 测试仅断言 lane0。见 forge-rustc WORKAROUNDS.md WA-37 |
| 编码 | SSE（0F/0F38 前缀族）+ AVX（VEX C4 语义键）+ AVX2（VEX 族） | v12 生成器内联实现 ModRM/REX/VEX 发射（`v12/codegen/mod.rs` 的 VlenCtx）；VEX 三操作数 r/m=src2、vvvv=~src1；无源指令 vvvv 编码 1111；vextractf128 的 dest 在 r/m、src 在 reg；vzeroupper 无需（Windows x64 ABI 允许破坏 YMM 高半） |

v12 结构化谓词：属性表 = `v12/pred.rs` 的 `PRED_ATTRS`（`rd`/`rs1_width`/
`rs2_width`/`rd_vec`/`rs1_vec`/`elem`/`cond`/`imm0`，与生成器 `__attr` 分派表
单点同步；写错属性名编译期报错——未知属性恒为假会让规则永不命中）+
`and/or/not/in/eq/ne/lt/le/gt/ge` 组合（纯 TOML 数据，无字符串）。
`imm0` = `current_immediates[0]`（Vextract/Vinsert lane 索引、AtomicRmw op
判别值 Xchg=0/Add=1/Sub=2）。

`[[lowering]]` 的两个消重/去序键（v15-S2）：
- `vary = { attr = [...], name = [...] }`：各列表**等长**，按下标 zip 成行展开。
  键在 `PRED_ATTRS` 里 → 该行自动追加 `eq = [键, 值]` 到 `when`；否则是模板里
  `{键}` 的纯替换变量。x86 `Fcmp` 32 条 → 8 条、`Vadd`/`Vsub` 各 8 → 3。
- 规则**不依赖声明序**：裁决序 = (`priority` 降, 谓词叶子数降, 声明序升)，
  见 `V12Model::lowering_by_op`。被前序规则完全覆盖的规则 → 编译期报"死规则"
  （判定域 = 每属性闭区间集合；含 `or`/`not` 记为 Opaque 跳过）。`priority`
  只在"故意让更宽的规则赢"时用（x86 `Vextract` 的 lane 0 快路径）。

编码键（v15-S3）：`[[forms]]` 与 `[[instructions]]` **共用同一组语义键**
（`EncKeys`：modrm/modrm_fixed/rex/vex/evex/prefix/opsize/rex_w/opcode_reg/imm/
escape/opcode_field/operand_fields）。form 退化为**可选的预设混入**，指令可逐键
覆盖（`EncKeys::over`，指令优先），`form` 本身可省略——组合不再需要预先命名。
x86 forms 47 → 19，37 条指令直接内联编码键（不再有 `MRR_0F_NOOS_MEM` 与
`MRR_MEM_0F_NOOS` 这种把 4 个事实编进名字、且只差 `rex_w` 的并存命名）。
`opsize` 固定宽度写**位宽整数**（`opsize = 64`）——v14 的 `"r8"` 是字节单位、
与 `[conventions]` 的位单位矛盾，旧写法现在报错并给出迁移提示；
`"s<N>"`（取第 N 个操作数的宽度）与 `"max"`（取全部 Reg 操作数的最大宽度）不变。

命名操作数（v15-S3c）：`ops = ["src:gprx", "dst:gprx:inout"]` **数组序 = 编码序**
（modrm reg/rm、定宽位域绑定都按这个序；角色缺省 `in`），`asm` 只用 `{名字}`
**引用**——声明与打印彻底分离。`opsize` 因此能写 `opsize = "dst"`（自解释），
取代 `"s1"` 这种"读者无法判断指哪个"的位置引用（v14 那个把 64 位指针截成 32 位的
bug 就出在 `s0` 恰好是**源**）。v14 的 asm 内联声明 `{i:[槽:角色]}` **已删除**，
无兼容层。`collect_inst_infos` 把 `{名字}` 规范化成 `{序号}` 后交给下游，
生成器（asm/machine/decode）只认索引、不感知命名。

## Code Conventions

- Edition 2024 throughout
- `forge-ir` types are re-exported in `forge-codegen::prelude` for DSL-generated code
- All generated code paths use `crate::` relative to forge-codegen
- Root `src/lib.rs` is a thin facade — all real code in `crates/`
