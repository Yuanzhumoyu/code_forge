//! ISA 测试模块 — v12 后端。x86_v12（本机 JIT）与 riscv64_v12（QEMU）。
//! 各 v11 ISA 套件（x86_64/aarch64/riscv64）与 unicorn 跨架构模拟已随
//! v11 语法层删除；执行/覆盖测试见 `exec`、`coverage` 与 `jit_matrix`。

pub mod riscv64_v12;
pub mod x86_v12;
