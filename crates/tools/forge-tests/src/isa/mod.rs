//! ISA 测试模块 — v12 后端。x86_v12（本机 JIT）、riscv64_v12（QEMU）、
//! arm64_v12（QEMU aarch64）。各 v11 ISA 套件与 unicorn 模拟已随 v11
//! 语法层删除；执行/覆盖测试见 `exec`、`coverage` 与 `jit_matrix`。

pub mod arm64_v12;
pub mod riscv64_v12;
pub mod x86_v12;
