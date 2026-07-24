//! 编译期加载 `riscv64_v10.toml`，验证 RISC-V 后端。
//!
//! `isa_from_file!` 宏生成 `pub mod riscv64`。
//! 外部可通过 `codegen_lib::backend::riscv64_test::riscv64::*` 访问。
//! 编译成功即验证通过。

use codegen_dsl::isa_from_file;

isa_from_file!("examples/isa/riscv64_v10.toml");
