//! RISC-V64 v12 — v12 唯一语法生成的定宽 encode/decode/asm 自包含模块
//! （仅依赖 std，未接入 TargetMachine——ABI/lowering 后续迭代接入）。
//!
//! 模块名 = 文件 stem（v12 约定）。验证见 `tests/riscv64_v12_tests.rs`。

forge_dsl::isa_from_file!("isa/riscv64_v12.toml");
pub use self::riscv64_v12::*;
// touch-b2a
// touch-b2b
// touch-b2c
// touch-b2d
// touch-d1
// touch-d2
// touch-rv-call
// touch-icmp-cond
// touch-branch-epilogue
// touch-callee
// touch-clobbers
// touch-reserved
// touch-push-callee
// touch-q
// touch-pop-fix
// touch-csb
// touch-spill-fp
// touch-cs-priority
// touch-cs2
// touch-stack-slot
// touch-A1-div
// touch-A1b-lwsw
// touch-A2
// touch-A2-zbb-fix
// touch-A2-seq
// touch-rot-t3
// touch-maxfix
// touch-A5
// touch-A67
// touch-A7b
// touch-A8
// touch-A9
// touch-A7c
// touch-A10
// touch-hi20
// touch-A10b
// touch-A10c
// touch-A11
// touch-fp-scalar

// touch: isa/riscv64_v12.toml updated (imm range fixes)

// touch: isa/riscv64_v12.toml (Ireduce 16-bit fix)
