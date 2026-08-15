# forge-tests — 通用 ISA 测试框架

`forge-tests` 为 code-forge 的 ISA 后端（内置 x86_64 / aarch64 / riscv64；wasm32 测试模块已移除，
编译覆盖在 forge-codegen 的 `arch/wasm32.rs` 内嵌测试；用户自定义 ISA 接入示例见 forge-codegen
的 `arch/minimal_sd_test.rs`）提供统一的测试框架：覆盖矩阵（compile-only）、
编码断言（encode-golden）、反汇编断言（disasm）、执行测试（本机 / unicorn 跨架构模拟）、
性质测试（确定性 / 恒等式 / 边界值），全部通过 Cargo features 按 ISA 与指令类型开关。

根 `tests/` 下的集成测试已全部迁入本 crate（无重复维护）。

## features 矩阵

| feature | 作用 |
| --- | --- |
| `isa-x86_64` / `isa-aarch64` / `isa-riscv64` | 打开对应 ISA 的测试模块（wasm32 测试模块已移除——编译覆盖在 forge-codegen 内嵌测试） |
| `test-int` / `test-float` / `test-io` / `test-control` | 指令类型分组（整数 / 浮点 / IO / 控制流），默认全开 |
| `exec-unicorn` | 启用 vendored unicorn-engine 跨架构模拟执行（aarch64/riscv64） |
| `nightly` | forge-rustc 后端集成测试（需 nightly 工具链 + `rustc-dev` 组件） |

默认 features：`isa-x86_64 + test-int + test-float + test-io + test-control`。

运行示例：

```bash
# 只跑 x86_64 的整数测试
cargo test -p forge-tests --no-default-features --features isa-x86_64,test-int

# 跑 aarch64 + riscv64 的浮点测试
cargo test -p forge-tests --no-default-features --features isa-aarch64,isa-riscv64,test-float

# 全 ISA + 全部指令类型
cargo test -p forge-tests --features isa-x86_64,isa-aarch64,isa-riscv64

# 跨架构模拟执行（build.rs 用 cmake 编译 vendored unicorn 静态库；
# 只需 cmake + C 编译器——无需 libclang / pkg-config / 系统 unicorn / 预编译 DLL）
cargo test -p forge-tests --features exec-unicorn

# forge-rustc 后端集成（需 nightly + rustc-dev）
cargo test -p forge-tests --features nightly
```

## 测试文件组织

```text
src/
├── lib.rs            # backend_tests! / coverage! / encode_golden! / encode_golden_err! /
│                     # disasm! / exec! / exec_f64! / exec_args! 宏 + nightly 模块
├── coverage.rs       # 75 ops 覆盖矩阵（compile-only）+ capability 守卫 + 全清零断言
├── isa/
│   ├── mod.rs        # ISA 模块门控（isa-* feature）
│   ├── cross_arch_exec.rs  # 跨架构模拟执行（exec-unicorn）
│   ├── x86_64/
│   │   ├── mod.rs    # 指令类型模块门控（test-* feature）
│   │   ├── int.rs / float.rs / io.rs / control.rs  # 本机执行测试
│   │   ├── encode.rs # 编码 golden（encode_golden! 宏 + 迁移自根 encoder_tests）
│   │   ├── disasm.rs # 反汇编断言（disasm! 宏）
│   │   └── jit.rs    # JIT 集成测试（298 条，迁移自根 jit_integration）
│   ├── aarch64/ riscv64/   # 同 x86_64 结构（compile-only；执行经 exec-unicorn）
└── exec/
    ├── harness.rs    # 统一 harness：build → compile → exec（run_i32/i64/f64/args_i64、compile_ok）
    ├── executor.rs   # Executor trait（本机 NativeExecutor + unicorn）
    ├── fuzz.rs       # 性质测试（确定性 / 恒等式 / 边界值，16 条）
    ├── unicorn.rs    # unicorn execute-from-buffer shim（exec-unicorn）
    └── unicorn_ffi.rs# 手写 FFI 绑定（含寄存器 id 回归测试）
```

## 用户自定义指令集测试

### 1. 定义 ISA

在 `isa/` 下新建 TOML（如 `isa/my_isa.toml`），按 DSL 语法定义指令编码、
寄存器、lowering 规则。在 forge-codegen 中注册 backend：

```rust
// crates/backend/forge-codegen/src/my_isa.rs
forge_dsl::isa_from_file!("isa/my_isa.toml");
pub use self::my_isa::*; // 生成 TargetMachine / Inst / IsaInfo 等全套组件
// 注册由 DSL 生成的 ensure_registered() 完成（Registry + reloc patcher OnceLock）
```

> 注：生成的顶层类型是 `TargetMachine`（组合 RegInfo/ABI/Lowering/Encoder/
> FrameLowering/Disassembler/Assembler 组件），不是 `Isa`；注册走
> `Registry::global().register_backend(...)`（DSL 生成的 `ensure_registered()` 内），
> 无 `register_backend!` 宏。

### 2. 接入各类测试（宏）

```rust
// crates/tools/forge-tests/src/isa/my_isa/mod.rs
use code_forge::backend::arch::my_isa::{self, TargetMachine};

forge_tests::backend_tests!(TargetMachine, my_isa::ensure_registered, "my_isa");

// compile-only 覆盖矩阵
forge_tests::coverage!("my_isa", TargetMachine, my_isa::ensure_registered,
    build_min_fn, &["Iadd", "Icmp", "Fadd", "Call", ...]);

// 编码 golden 断言（VReg(0..64)→Int、VReg(64..128)→Float 已预置）
forge_tests::encode_golden!(my_isa::Encoder, &[
    (Inst::Ret, &[0x00][..]),
    // ...
]);

// 编码负例（期望报错）
forge_tests::encode_golden_err!(my_isa::Encoder, &[Inst::UnsupportedInst]);

// 反汇编断言
forge_tests::disasm!(TargetMachine, &[
    (Inst::Ret, "ret"),
    // ...
]);

// 本机执行（返回 i32）
forge_tests::exec!("my_isa_add", |b| {
    let a = b.iconst_i32(20);
    let bv = b.iconst_i32(22);
    b.iadd(a, bv)
}, 42);

// 本机执行（返回 f64，近似比较）
forge_tests::exec_f64!("my_isa_fadd", |b| {
    let a = b.fconst_f64(20.5);
    let bv = b.fconst_f64(21.5);
    b.fadd(a, bv)
}, 42.0);

// 本机执行（带 ≤4 个整型参数，返回 i64）
forge_tests::exec_args!("my_isa_add_args",
    &[(code_forge::prelude::TypeId::I64, "a"), (code_forge::prelude::TypeId::I64, "b")],
    &[20, 22],
    |b, p| b.iadd(p[0], p[1]),
    42);
```

> 注意：每个 `exec!` / `exec_f64!` / `exec_args!` 调用需位于独立模块
> （宏内部生成 `mod exec_tests`），同一模块内勿重复调用。

### 3. 按指令类型拆分（可选）

仿照 `src/isa/x86_64/` 的 `int.rs`/`float.rs`/`io.rs`/`control.rs` 结构，
把测试按指令类别放入对应模块，并在 `mod.rs` 用 `#[cfg(feature = "test-int")]`
等门控，即可复用指令类型 features。

## 跨架构执行说明（exec-unicorn）

- riscv64：整数算术 / 位运算 / 移位 / 控制流（icmp/select）可在 unicorn 模拟执行。
- aarch64：仅最简函数（iconst + ret）可执行；含算术 / 位运算 / 浮点 / load-store 的
  产物会编码出 vendored unicorn 2.1.5 不支持的指令（如 band 的 `c51b039a`），
  触发 `UC_ERR_EXCEPTION`——相关用例以 compile-only 断言呈现。
- 浮点结果经 `bitcast(f64 → i64)` 转位模式后返回，断言 `expected.to_bits()`
  （避免依赖 unicorn 浮点寄存器 id）。
