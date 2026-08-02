//! 编译期加载 `minimal_sd.toml`，验证 DSL + v19 代码生成链路。
//!
//! `isa_from_file!` 宏生成 `pub mod minimal_sd`。
//! 编译成功即验证通过（TOML 语法 + lowering 规则完整性 + v19 组件生成）。

use forge_dsl::isa_from_file;
isa_from_file!("isa/minimal_sd.toml");
