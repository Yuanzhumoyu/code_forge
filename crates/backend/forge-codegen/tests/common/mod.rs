//! 测试共享夹具：在**测试 crate**里生成三个 demo ISA（库本体不含它们）。
//!
//! `isa_from_file!(…, krate = forge_codegen)` 把生成物里的 `crate::…` 改写为
//! `forge_codegen::…`、`forge_ir::…` 改写为 `forge_codegen::ir::…`——因此生成
//! 代码只依赖 forge-codegen 的**公开 API**，与"生成在库内部"完全等价，却不再
//! 把 demo 谱编进 rlib。
//!
//! 谱文件在 `tests/isa/`（路径相对 `CARGO_MANIFEST_DIR` 解析）：
//! `demo_v12`（8 字节寄存器 / 32 位字）、`demo8_v12`（1 字节寄存器 / 32 位字）、
//! `demo_inst8_v12`（8 位指令字）、`demo_inst12_v12`（12 位字，非 8 倍数）、
//! `demo_inst100_v12`（100 位字：超机器字 + 非 8 倍数）、
//! `demo_mixed16_32_v12`（**混合字长**：16 位短编码 + 32 位长编码共存）。
//!
//! `spec_tests = false`：本文件被**多个测试二进制**包含，生成的自测会在每个二进制里
//! 重复跑一遍。生成期自测（v18 S6）由专门的二进制
//! `tests/spec_tests_v12.rs` 重新宿主这三个夹具并打开——那里每个夹具只跑一次。
#![allow(dead_code)] // 各 test target 只用到其中一个夹具；未用到的分支不算错误

forge_dsl::isa_from_file!(
    "tests/isa/demo_v12.toml",
    krate = forge_codegen,
    spec_tests = false
);
forge_dsl::isa_from_file!(
    "tests/isa/demo8_v12.toml",
    krate = forge_codegen,
    spec_tests = false
);
forge_dsl::isa_from_file!(
    "tests/isa/demo_inst8_v12.toml",
    krate = forge_codegen,
    spec_tests = false
);
forge_dsl::isa_from_file!(
    "tests/isa/demo_inst12_v12.toml",
    krate = forge_codegen,
    spec_tests = false
);
forge_dsl::isa_from_file!(
    "tests/isa/demo_inst100_v12.toml",
    krate = forge_codegen,
    spec_tests = false
);
forge_dsl::isa_from_file!(
    "tests/isa/demo_mixed16_32_v12.toml",
    krate = forge_codegen,
    spec_tests = false
);
