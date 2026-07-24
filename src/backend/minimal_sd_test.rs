//! 编译期加载 `minimal_sd.toml`，验证默认 lowering 机制。
//!
//! `isa_from_file!` 宏生成 `pub mod minimal_sd`，
//! 外部可通过 `codegen_lib::backend::minimal_sd_test::minimal_sd::*` 访问。
//! 编译成功即验证通过（TOML 语法 + lowering 规则完整性）。

use codegen_dsl::isa_from_file;

isa_from_file!("examples/isa/minimal_sd.toml");
