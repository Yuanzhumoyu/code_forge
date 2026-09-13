//! demo8_v12 — 1 字节寄存器 ISA 后端注册。
//!
//! 夹具目的见 `isa/demo8_v12.toml` 头部：唯一的 1 字节 GPR 组，用于回归
//! 「寄存器类型/宽度写死」——历史实现（主 GPR 组锚定 GPR(8)/GPR(4)、地址类/
//! 槽宽/帧开销内置 8 字节）在本文件上会构造不存在的类或静默丢名字。
//!
//! 验证：`crates/backend/forge-codegen/tests/demo8_v12_tests.rs`。

forge_dsl::isa_from_file!("isa/demo8_v12.toml");
pub use self::demo8_v12::*;
