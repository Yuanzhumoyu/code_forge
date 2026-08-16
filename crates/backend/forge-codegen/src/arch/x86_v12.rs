//! x86-64 v12 试点 — 迭代 3：变长语义键（ModRM/REX/opsize/SSE）。
//!
//! v12 唯一语法生成的变长 encode/decode/asm 自包含模块 + TargetMachine
//! 集成层（迭代 5/6：MachineInst/Encoder/Decoder/ABI/FrameLowering/
//! Lowering/TargetMachine）。与 v11 `x86_64` 后端并存用于 golden 字节
//! 对比（见 `tests/x86_v12_tests.rs`、`tests/v12_integration_tests.rs`）。
//!
//! 模块名 = 文件 stem（v12 约定）；不 glob 导出以免与 v11 x86_64 冲突。

forge_dsl::isa_v12_from_file!("isa/x86_v12.toml");
pub use self::x86_v12::*;

#[cfg(test)]
mod tests {
    use super::*;
    use crate::pipeline::compiler::FunctionCompiler;
    use forge_ir::{FunctionSignature, TypeContext, TypeId};

    /// 端到端：IR → v12 lowering → regalloc → frame → encode 全链路。
    #[test]
    fn e2e_compile_add() {
        let sig = FunctionSignature::new(&[(TypeId::I64, "a"), (TypeId::I64, "b")], &[TypeId::I64]);
        let mut builder = crate::FunctionBuilder::new("add", TypeContext::new(), sig);
        let (entry, params) =
            builder.create_block_with_params(&[(TypeId::I64, "a"), (TypeId::I64, "b")]);
        builder.switch_to_block(entry);
        let sum = builder.iadd(params[0], params[1]);
        builder.ret(&[sum]);
        let func = builder.finish().expect("build");
        let compiler = FunctionCompiler::new(TargetMachine::new());
        let compiled = compiler.compile_raw(&func).expect("v12 compile add");
        eprintln!("v12 e2e add ({} bytes):", compiled.code.len());
        for (i, &byte) in compiled.code.iter().enumerate() {
            eprint!("{byte:02x} ");
            if (i + 1) % 16 == 0 {
                eprintln!();
            }
        }
        eprintln!();
        assert!(!compiled.code.is_empty(), "生成代码非空");
        assert!(compiled.code.contains(&0x55), "prologue push rbp");
        assert!(compiled.code.contains(&0xC3), "ret");
    }
}
