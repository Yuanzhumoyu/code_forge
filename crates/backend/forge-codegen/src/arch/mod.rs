//! 真实 ISA 后端（DSL 生成）。库本体只含**发行后端**；
//! ISA-DSL 的示例/夹具谱（demo_v12、demo8_v12）在
//! `../../tests/isa/` + `../../tests/common/mod.rs` 里生成，不进 rlib。
pub mod arm64_v12;
pub mod riscv64_v12;
pub mod x86_v12;
