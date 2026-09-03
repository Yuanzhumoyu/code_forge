//! x86-64 v12 — v12 唯一语法生成的变长 encode/decode/asm 自包含模块 +
//! TargetMachine 集成层（MachineInst/Encoder/Decoder/ABI/FrameLowering/
//! Lowering/TargetMachine）。v11 后端已删除；golden 见
//! `tests/x86_v12_tests.rs`、`tests/v12_integration_tests.rs`。
//!
//! 模块名 = 文件 stem（v12 约定）。

forge_dsl::isa_from_file!("isa/x86_v12.toml");
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

    /// 编译 const42 并打印字节（不执行——用于调试 JIT 崩溃）。
    #[test]
    fn e2e_compile_const_print() {
        use forge_ir::{Module, TypeId};
        ensure_registered();
        let mut m = Module::new();
        {
            let sig = FunctionSignature::new(&[], &[TypeId::I64]);
            let mut b = crate::FunctionBuilder::new("const42", TypeContext::new(), sig);
            b.create_block_here();
            let v = b.iconst_i64(42);
            b.ret(&[v]);
            m.add_function(b.finish().expect("build"));
        }
        let compiler = FunctionCompiler::new(TargetMachine::new());
        let func = m.get_function(forge_ir::FuncRef(0));
        let cf = compiler.compile_raw(func).expect("compile const42");
        eprintln!("const42 code ({} bytes):", cf.code.len());
        for (i, &byte) in cf.code.iter().enumerate() {
            eprint!("{byte:02x} ");
            if (i + 1) % 16 == 0 {
                eprintln!();
            }
        }
        eprintln!();
    }

    /// 端到端 + JIT 执行：v12 后端编译并运行 fn const42() -> i64 { 42 }。
    #[cfg(all(target_arch = "x86_64", feature = "jit"))]
    #[test]
    fn e2e_jit_run_const() {
        use forge_ir::{Module, TypeId};
        ensure_registered();
        let mut m = Module::new();
        {
            let sig = FunctionSignature::new(&[], &[TypeId::I64]);
            let mut b = crate::FunctionBuilder::new("const42", TypeContext::new(), sig);
            b.create_block_here();
            let v = b.iconst_i64(42);
            b.ret(&[v]);
            m.add_function(b.finish().expect("build"));
        }
        let mut jit = crate::jit::JitCompiler::new(TargetMachine::new());
        jit.compile_module(&m).expect("v12 module compile");
        let f: extern "C" fn() -> i64 = jit.get_fn("const42").expect("const42");
        assert_eq!(f(), 42, "v12 JIT 执行 const42 应返回 42");
    }

    /// 端到端 + JIT 执行：fn add(a: i64, b: i64) -> i64 { a + b }。
    #[cfg(all(target_arch = "x86_64", feature = "jit"))]
    #[test]
    fn e2e_jit_run_add() {
        use forge_ir::{Module, TypeId};
        ensure_registered();
        let mut m = Module::new();
        {
            let sig =
                FunctionSignature::new(&[(TypeId::I64, "a"), (TypeId::I64, "b")], &[TypeId::I64]);
            let mut b = crate::FunctionBuilder::new("add", TypeContext::new(), sig);
            let (entry, params) =
                b.create_block_with_params(&[(TypeId::I64, "a"), (TypeId::I64, "b")]);
            b.switch_to_block(entry);
            let sum = b.iadd(params[0], params[1]);
            b.ret(&[sum]);
            m.add_function(b.finish().expect("build"));
        }
        let mut jit = crate::jit::JitCompiler::new(TargetMachine::new());
        jit.compile_module(&m).expect("v12 module compile");
        let f: extern "C" fn(i64, i64) -> i64 = jit.get_fn("add").expect("add");
        assert_eq!(f(2, 3), 5, "v12 JIT add(2,3) 应返回 5");
        assert_eq!(f(-7, 100), 93, "v12 JIT add(-7,100) 应返回 93");
    }
}
// touch
// touch2
// touch3
// touch4
// touch5
// touch6

// touch14

// touch15

// t

// touch16

// touch17

// touch18

// touch19
