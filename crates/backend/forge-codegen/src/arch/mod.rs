//! 发行后端模块 + 注册表（v28 M2：由 `isa/manifest.toml` 生成）。
//!
//! **不要在这里手写 `pub mod`**：新增 ISA 只改 `isa/manifest.toml`，构建脚本会生成
//! `$OUT_DIR/arch_registry.rs`（模块 include + 目标前缀表 + `ensure_all` + alloc shim 表）。
//! 本文件只负责 `include!` 与测试模块。

include!(concat!(env!("OUT_DIR"), "/arch_registry.rs"));

#[cfg(test)]
mod tests;
