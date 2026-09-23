//! 最小**外部宿主**（ISA-DSL v19 V2）：证明任意普通 crate 都能承载一份 ISA 谱。
//!
//! ## 依赖面（G1 的硬证据）
//!
//! - `[dependencies]` 只有 `forge-isa-runtime`：生成物里的 `crate::…` / `forge_ir::…`
//!   已被改写成 `forge_isa_runtime::…` / `forge_isa_runtime::ir::…`（v19 V1b），
//!   所以**不需要** forge-codegen（JIT/regalloc/指令发射都在那里）；
//! - `[build-dependencies]` 只有 `forge-isa-dsl`（生成器本体）；
//! - 本 crate 连 proc-macro 都不依赖：`build.rs` 直接调 `pregenerate`，
//!   这里 include 生成物（`isa_from_file!` 只是一句 `include!` 的语法糖）。
//!
//! 三条都由 `tests/host_surface.rs` 钉住（含生成物文本级断言：路径必须指向
//! `forge_isa_runtime`、不得再出现 `crate::`）。
//!
//! ## 谱与部件
//!
//! 谱是 [`docs/guides/isa-dsl-tutorial.md`](../../docs/guides/isa-dsl-tutorial.md) §0
//! 的 16 位玩具 ISA（`isa/toy16.toml`，4 条指令）；部件 = `encode`/`decode`/`asm`
//! ——**不要** TargetMachine 集成层，因此不注册、也不需要任何编译管线。
//! `spec_tests = true` 仍成立：生成期自测只要 encode/decode/asm 三块
//! （`Parts::supports_spec_tests`，v19 V2 起不再要求 `tm`），
//! 于是 `cargo test -p isa-host-demo` 直接跑规格用例。
//!
//! ## 一般在外部 crate 里怎么写
//!
//! 普通宿主不用手写上面的文件名管道，写宏即可（同一条生成路径）：
//!
//! ```toml
//! [build-dependencies]
//! forge-isa-dsl = { path = "…/forge-isa-dsl" }
//! ```
//!
//! ```rust,ignore
//! // build.rs
//! fn main() { forge_isa_dsl::pregenerate_host().expect("ISA 预生成失败"); }
//!
//! // src/lib.rs
//! forge_dsl::isa_from_file!("isa/toy16.toml", parts = ["encode", "decode", "asm"]);
//! pub use self::toy16::*;
//! ```
//!
//! 本 crate 走的是"无 proc-macro"那条（`build.rs` 里 `pregenerate` + 这里的
//! `include!`），因此能证明**最极端**的依赖面：只有运行时 crate。

include!(concat!(env!("OUT_DIR"), "/", env!("ISA_HOST_DEMO_GEN")));

pub use self::toy16::*;
