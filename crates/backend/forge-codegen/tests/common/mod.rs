//! 测试共享夹具：在**测试 crate**里生成两个 demo ISA（库本体不含它们）。
//!
//! `isa_from_file!(…, krate = forge_codegen)` 把生成物里的 `crate::…` 改写为
//! `forge_codegen::…`、`forge_ir::…` 改写为 `forge_codegen::ir::…`——因此生成
//! 代码只依赖 forge-codegen 的**公开 API**，与"生成在库内部"完全等价，却不再
//! 把 demo 谱编进 rlib。
//!
//! 谱文件在 `tests/isa/`（路径相对 `CARGO_MANIFEST_DIR` 解析）。
#![allow(dead_code)] // 各 test target 只用到其中一个夹具；未用到的分支不算错误

forge_dsl::isa_from_file!("tests/isa/demo_v12.toml", krate = forge_codegen);
forge_dsl::isa_from_file!("tests/isa/demo8_v12.toml", krate = forge_codegen);
