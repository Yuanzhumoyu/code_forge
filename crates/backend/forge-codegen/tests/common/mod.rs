//! 测试共享夹具：在**测试 crate**里生成三个 demo ISA（库本体不含它们）。
//!
//! `isa_from_file!(…)` 把生成物里的 `crate::…` 改写为
//! `forge_codegen::…`、`forge_ir::…` 改写为 `forge_codegen::ir::…`——因此生成
//! 代码只依赖 forge-codegen 的**公开 API**，与"生成在库内部"完全等价，却不再
//! 把 demo 谱编进 rlib。
//!
//! 谱文件在 `tests/isa/`（路径相对 `CARGO_MANIFEST_DIR` 解析）：
//! `demo_v12`（8 字节寄存器 / 32 位字）、`demo8_v12`（1 字节寄存器 / 32 位字）、
//! `demo_inst8_v12`（8 位指令字）、`demo_inst12_v12`（12 位字，非 8 倍数）、
//! `demo_inst100_v12`（100 位字：超机器字 + 非 8 倍数）、
//! `demo_mixed16_32_v12`（**混合字长**：16 位短编码 + 32 位长编码共存）、
//! `include_root_v12`（**多文件谱**：`include` 片段 + `[[override]]`，v18 S7d；
//! 另以 `name`/`parts` 再展开一份"只有编码器"的模块）。
//!
//! `spec_tests = false`：本文件被**多个测试二进制**包含，生成的自测会在每个二进制里
//! 重复跑一遍。生成期自测（v18 S6）由专门的二进制
//! `tests/spec_tests_v12.rs` 重新宿主这三个夹具并打开——那里每个夹具只跑一次。
#![allow(dead_code)] // 各 test target 只用到其中一个夹具；未用到的分支不算错误

forge_dsl::isa_from_file!("tests/isa/demo_v12.toml", spec_tests = false);
forge_dsl::isa_from_file!("tests/isa/demo8_v12.toml", spec_tests = false);
forge_dsl::isa_from_file!("tests/isa/demo_inst8_v12.toml", spec_tests = false);
forge_dsl::isa_from_file!("tests/isa/demo_inst12_v12.toml", spec_tests = false);
forge_dsl::isa_from_file!("tests/isa/demo_inst100_v12.toml", spec_tests = false);
forge_dsl::isa_from_file!("tests/isa/demo_mixed16_32_v12.toml", spec_tests = false);
// v18 S7d：`include` 组合（根 + 片段两个文件合成一份谱）+ `[[override]]`。
// 生成物里登记了**两个**来源文件的 `include_bytes!`（改任一片段都触发重编译）。
forge_dsl::isa_from_file!("tests/isa/include_root_v12.toml", spec_tests = false);
// v18 S7d：`name = "…"`（模块名覆盖）+ `parts = ["encode"]`（只生成编码器）。
// 同一份谱再展开一次，证明部件选择在**真实宏路径**上也成立：这个模块里
// 没有 `decode`/`disassemble`/`assemble`/`TargetMachine`（另见
// `crates/frontend/forge-isa-dsl/tests/parts_selection.rs` 的文本级断言）。
forge_dsl::isa_from_file!(
    "tests/isa/include_root_v12.toml",
    spec_tests = false,
    name = "include_enc_v12",
    parts = ["encode"]
);
