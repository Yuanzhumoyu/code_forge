//! ISA 测试模块 — DSL 后端。x86（本机 JIT）、riscv64（QEMU）、
//! arm64（QEMU aarch64）。各 v11 ISA 套件与 unicorn 模拟已随 v11
//! 语法层删除；执行/覆盖测试见 `exec`、`coverage` 与 `jit_matrix`。

pub mod arm64;
pub mod riscv64;
pub mod x86;
