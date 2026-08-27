# forge-tests — 通用 ISA 测试框架（v12 唯一后端）

`forge-tests` 为 code-forge 的 ISA 后端（v12 唯一后端 `x86_v12`）提供统一的
测试框架：覆盖矩阵（compile-only）、执行测试（本机 JIT）、性质测试
（确定性 / 恒等式 / 边界值）与 forge-rustc 后端集成（`nightly` feature）。

v11 时代的多 ISA 套件（x86_64/aarch64/riscv64）、encode-golden/disasm 断言与
unicorn 跨架构模拟（`exec-unicorn`）已随 v11 语法层删除——aarch64/riscv64
无 v12 对应后端；编码/解码/汇编断言由 forge-codegen 的 `x86_v12_tests.rs` /
`riscv64_v12_tests.rs` / `decoder_smoke.rs` 覆盖。

根 `tests/` 下的集成测试已全部迁入本 crate（无重复维护）。

## features

| feature | 作用 |
| --- | --- |
| （无默认 features） | 覆盖矩阵 / 执行 / 模糊测试（x86_v12）默认运行 |
| `nightly` | forge-rustc 后端集成测试（需 nightly 工具链 + `rustc-dev` 组件） |

运行示例：

```bash
# 覆盖矩阵 + 执行 + 模糊测试（x86_v12）
cargo test -p forge-tests

# forge-rustc 后端集成（需 nightly + rustc-dev）
cargo test -p forge-tests --features nightly
```

## 测试文件组织

```text
src/
├── lib.rs            # backend_tests! 等宏 + nightly 模块 + run_x86_64_pipeline_tests
├── coverage.rs       # 75 ops 覆盖矩阵（compile-only）+ capability 守卫 + v12 声明集零缺口断言
├── isa/mod.rs        # 已随 v11 套件清空（模块保留占位）
└── exec/
    ├── harness.rs    # 统一 harness：build → compile → exec（run_i32/i64/f64/args_i64、compile_ok）
    ├── executor.rs   # Executor trait（本机 NativeExecutor，x86_64）
    └── fuzz.rs       # 性质测试（确定性 / 恒等式 / 边界值）
```

## 用户自定义指令集测试

### 1. 定义 ISA

在 `isa/` 下新建 TOML（如 `isa/my_isa.toml`），按 v12 DSL 语法定义指令编码、
寄存器、lowering 规则（见 `docs/isa-dsl.md`）。在 forge-codegen 中注册 backend：

```rust
// crates/backend/forge-codegen/src/arch/my_isa.rs
forge_dsl::isa_from_file!("isa/my_isa.toml");
pub use self::my_isa::*; // 生成 TargetMachine / Inst / Reg 等全套组件
// 注册由 DSL 生成的 ensure_registered() 完成（Registry + reloc patcher OnceLock）
```

> 注：生成的顶层类型是 `TargetMachine`（组合 RegInfo/ABI/Lowering/Encoder/
> FrameLowering/Disassembler/Assembler/Decoder 组件），不是 `Isa`；注册走
> `Registry::global().register_backend(...)`（DSL 生成的 `ensure_registered()` 内），
> 无 `register_backend!` 宏。

### 2. 接入测试

覆盖矩阵与执行测试复用 `coverage.rs` 与 `exec/` 的泛型接口：

```rust
// compile-only 覆盖矩阵（75 ops，返回 (op, outcome) 列表）
let results = forge_tests::coverage::check_isa(| | code_forge::backend::x86_v12::TargetMachine::new());

// 本机执行（返回 i32）
forge_tests::exec::harness::run_i32("my_isa_add", |b| {
    let a = b.iconst_i32(20);
    let bv = b.iconst_i32(22);
    b.iadd(a, bv)
});
```
