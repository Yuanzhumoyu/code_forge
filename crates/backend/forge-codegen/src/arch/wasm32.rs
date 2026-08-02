//! WebAssembly (wasm32) backend — v19 DSL-generated TargetMachine.
forge_dsl::isa_from_file!("isa/wasm32_v10.toml");
pub use self::wasm32::*;

#[cfg(test)]
mod tests {
    use super::*;
    use crate::pipeline::compiler::FunctionCompiler;
    use crate::prelude::*;
    use forge_ir::{FunctionSignature, TypeContext};

    #[test]
    fn test_wasm32_types() {
        let _ = std::mem::size_of::<TargetMachine>();
        let _ = std::mem::size_of::<Inst>();
    }

    #[test]
    fn test_wasm32_registration() {
        ensure_registered();
        assert!(crate::Registry::global().contains("wasm32"));
    }

    #[test]
    fn test_wasm32_compile_iconst() {
        let sig = FunctionSignature::new(&[], &[TypeId::I32]);
        let mut builder = crate::FunctionBuilder::new("answer", TypeContext::new(), sig);
        let entry = builder.create_block();
        builder.switch_to_block(entry);
        let v = builder.iconst_i32(42);
        builder.ret(&[v]);
        let func = builder.finish();

        let compiler = FunctionCompiler::new(TargetMachine::new());
        let result = compiler.compile_raw(&func);
        assert!(
            result.is_ok(),
            "Wasm32 compilation should succeed: {:?}",
            result.err()
        );
        let compiled = result.unwrap();
        assert!(!compiled.code.is_empty(), "Wasm32 code should not be empty");
        assert_eq!(
            compiled.code.last(),
            Some(&0x0B),
            "Wasm function should end with 'end' opcode"
        );
    }

    #[test]
    fn test_wasm32_compile_add() {
        let sig = FunctionSignature::new(&[(TypeId::I32, "a"), (TypeId::I32, "b")], &[TypeId::I32]);
        let mut builder = crate::FunctionBuilder::new("add", TypeContext::new(), sig);
        let (entry, params) =
            builder.create_block_with_params(&[(TypeId::I32, "a"), (TypeId::I32, "b")]);
        builder.switch_to_block(entry);
        let sum = builder.iadd(params[0], params[1]);
        builder.ret(&[sum]);
        let func = builder.finish();

        let compiler = FunctionCompiler::new(TargetMachine::new());
        let compiled = compiler.compile_raw(&func).expect("wasm32 compile add");
        assert!(!compiled.code.is_empty());
        assert_eq!(compiled.code.last(), Some(&0x0B));
    }

    #[test]
    fn test_wasm32_compile_sub() {
        let sig = FunctionSignature::new(&[(TypeId::I32, "a"), (TypeId::I32, "b")], &[TypeId::I32]);
        let mut builder = crate::FunctionBuilder::new("sub", TypeContext::new(), sig);
        let (entry, params) =
            builder.create_block_with_params(&[(TypeId::I32, "a"), (TypeId::I32, "b")]);
        builder.switch_to_block(entry);
        let diff = builder.isub(params[0], params[1]);
        builder.ret(&[diff]);
        let func = builder.finish();

        let compiler = FunctionCompiler::new(TargetMachine::new());
        let compiled = compiler.compile_raw(&func).expect("wasm32 compile sub");
        assert!(!compiled.code.is_empty());
        assert_eq!(compiled.code.last(), Some(&0x0B));
    }

    #[test]
    fn test_wasm32_compile_mul() {
        let sig = FunctionSignature::new(&[(TypeId::I32, "a"), (TypeId::I32, "b")], &[TypeId::I32]);
        let mut builder = crate::FunctionBuilder::new("mul", TypeContext::new(), sig);
        let (entry, params) =
            builder.create_block_with_params(&[(TypeId::I32, "a"), (TypeId::I32, "b")]);
        builder.switch_to_block(entry);
        let prod = builder.imul(params[0], params[1]);
        builder.ret(&[prod]);
        let func = builder.finish();

        let compiler = FunctionCompiler::new(TargetMachine::new());
        let compiled = compiler.compile_raw(&func).expect("wasm32 compile mul");
        assert!(!compiled.code.is_empty());
        assert_eq!(compiled.code.last(), Some(&0x0B));
    }

    #[test]
    fn test_wasm32_compile_and() {
        let sig = FunctionSignature::new(&[(TypeId::I32, "a"), (TypeId::I32, "b")], &[TypeId::I32]);
        let mut builder = crate::FunctionBuilder::new("and", TypeContext::new(), sig);
        let (entry, params) =
            builder.create_block_with_params(&[(TypeId::I32, "a"), (TypeId::I32, "b")]);
        builder.switch_to_block(entry);
        let result = builder.band(params[0], params[1]);
        builder.ret(&[result]);
        let func = builder.finish();

        let compiler = FunctionCompiler::new(TargetMachine::new());
        let compiled = compiler.compile_raw(&func).expect("wasm32 compile and");
        assert!(!compiled.code.is_empty());
        assert_eq!(compiled.code.last(), Some(&0x0B));
    }

    #[test]
    fn test_wasm32_compile_branch() {
        let sig = FunctionSignature::new(&[(TypeId::I32, "cond")], &[TypeId::I32]);
        let mut builder = crate::FunctionBuilder::new("branch", TypeContext::new(), sig);
        let (entry, params) = builder.create_block_with_params(&[(TypeId::I32, "cond")]);
        builder.switch_to_block(entry);
        let then_block = builder.create_block();
        let else_block = builder.create_block();
        let (merge_block, merge_params) = builder.create_block_with_params(&[(TypeId::I32, "phi")]);
        builder.switch_to_block(entry);
        builder.branch(params[0], then_block, &[], else_block, &[]);
        builder.switch_to_block(then_block);
        let ten = builder.iconst_i32(10);
        builder.jump(merge_block, &[ten]);
        builder.switch_to_block(else_block);
        let zero = builder.iconst_i32(0);
        builder.jump(merge_block, &[zero]);
        builder.switch_to_block(merge_block);
        builder.ret(&[merge_params[0]]);
        let func = builder.finish();

        let compiler = FunctionCompiler::new(TargetMachine::new());
        let compiled = compiler.compile_raw(&func).expect("wasm32 compile branch");
        assert!(!compiled.code.is_empty());
    }

    #[test]
    fn test_wasm32_compile_many_ops() {
        let sig = FunctionSignature::new(&[(TypeId::I32, "x")], &[TypeId::I32]);
        let mut builder = crate::FunctionBuilder::new("many_ops", TypeContext::new(), sig);
        let (entry, params) = builder.create_block_with_params(&[(TypeId::I32, "x")]);
        builder.switch_to_block(entry);
        let two = builder.iconst_i32(2);
        let v1 = builder.imul(params[0], two);
        let v2 = builder.iadd(v1, params[0]);
        let v3 = builder.isub(v2, params[0]);
        let v4 = builder.bxor(v3, params[0]);
        let v5 = builder.band(v4, params[0]);
        builder.ret(&[v5]);
        let func = builder.finish();

        let compiler = FunctionCompiler::new(TargetMachine::new());
        let compiled = compiler
            .compile_raw(&func)
            .expect("wasm32 compile many_ops");
        assert!(compiled.code.len() > 20);
    }
}
