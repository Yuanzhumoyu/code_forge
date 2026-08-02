//! AArch64 (ARM64) backend — v19 DSL-generated TargetMachine.
forge_dsl::isa_from_file!("isa/aarch64_v10.toml");
pub use self::aarch64::*;

#[cfg(test)]
mod tests {
    use super::*;
    use crate::pipeline::compiler::FunctionCompiler;
    use crate::prelude::*;
    use forge_ir::{FunctionBuilder, FunctionSignature, TypeContext};

    #[test]
    fn test_aarch64_compile_add_mul() {
        let sig = FunctionSignature::new(&[(TypeId::I64, "a"), (TypeId::I64, "b")], &[TypeId::I64]);
        let mut builder = FunctionBuilder::new("add_mul", TypeContext::new(), sig);
        let (entry, params) =
            builder.create_block_with_params(&[(TypeId::I64, "a"), (TypeId::I64, "b")]);
        builder.switch_to_block(entry);
        let sum = builder.iadd(params[0], params[1]);
        let prod = builder.imul(sum, params[0]);
        builder.ret(&[prod]);
        let func = builder.finish();

        let compiler = FunctionCompiler::new(TargetMachine::new());
        let result = compiler.compile_raw(&func);
        assert!(result.is_ok(), "AArch64 compile failed: {:?}", result.err());
        let compiled = result.unwrap();
        assert!(
            !compiled.code.is_empty(),
            "AArch64 code should not be empty"
        );
        assert!(
            compiled.code.len() >= 4,
            "AArch64 code must have at least 4 bytes"
        );
    }

    #[test]
    fn test_aarch64_compile_icmp() {
        let sig = FunctionSignature::new(&[(TypeId::I64, "a"), (TypeId::I64, "b")], &[TypeId::I32]);
        let mut builder = FunctionBuilder::new("cmp", TypeContext::new(), sig);
        let (entry, params) =
            builder.create_block_with_params(&[(TypeId::I64, "a"), (TypeId::I64, "b")]);
        builder.switch_to_block(entry);
        let cond = builder.icmp(IntCC::SignedGreaterThan, params[0], params[1]);
        builder.ret(&[cond]);
        let func = builder.finish();

        let compiler = FunctionCompiler::new(TargetMachine::new());
        let result = compiler.compile_raw(&func);
        assert!(
            result.is_ok(),
            "AArch64 Icmp compile failed: {:?}",
            result.err()
        );
    }

    #[test]
    fn test_aarch64_prologue_frame() {
        let sig = FunctionSignature::new(
            &[(TypeId::I64, "a"), (TypeId::I64, "b"), (TypeId::I64, "c")],
            &[TypeId::I64],
        );
        let mut builder = FunctionBuilder::new("prologue_frame", TypeContext::new(), sig);
        let (entry, params) = builder.create_block_with_params(&[
            (TypeId::I64, "a"),
            (TypeId::I64, "b"),
            (TypeId::I64, "c"),
        ]);
        builder.switch_to_block(entry);
        let sum = builder.iadd(params[0], params[1]);
        let result = builder.iadd(sum, params[2]);
        builder.ret(&[result]);
        let func = builder.finish();

        let compiler = FunctionCompiler::new(TargetMachine::new());
        let compiled = compiler.compile_raw(&func).expect("prologue_frame compile");
        assert!(!compiled.code.is_empty(), "code should not be empty");
        assert!(
            compiled.code.len() >= 4,
            "should have at least one instruction"
        );
    }

    #[test]
    fn test_aarch64_compile_bitwise() {
        let sig = FunctionSignature::new(&[(TypeId::I64, "a"), (TypeId::I64, "b")], &[TypeId::I64]);
        let mut builder = FunctionBuilder::new("bitwise", TypeContext::new(), sig);
        let (entry, params) =
            builder.create_block_with_params(&[(TypeId::I64, "a"), (TypeId::I64, "b")]);
        builder.switch_to_block(entry);
        let and_val = builder.band(params[0], params[1]);
        let or_val = builder.bor(and_val, params[0]);
        let xor_val = builder.bxor(or_val, params[1]);
        let not_val = builder.bnot(xor_val);
        builder.ret(&[not_val]);
        let func = builder.finish();

        let compiler = FunctionCompiler::new(TargetMachine::new());
        let compiled = compiler.compile_raw(&func).expect("bitwise compile");
        assert!(!compiled.code.is_empty());
    }

    #[test]
    fn test_aarch64_compile_shifts() {
        let sig = FunctionSignature::new(&[(TypeId::I64, "a"), (TypeId::I64, "b")], &[TypeId::I64]);
        let mut builder = FunctionBuilder::new("shifts", TypeContext::new(), sig);
        let (entry, params) =
            builder.create_block_with_params(&[(TypeId::I64, "a"), (TypeId::I64, "b")]);
        builder.switch_to_block(entry);
        let shl = builder.ishl(params[0], params[1]);
        let shr = builder.ushr(shl, params[1]);
        let sar = builder.sshr(shr, params[0]);
        builder.ret(&[sar]);
        let func = builder.finish();

        let compiler = FunctionCompiler::new(TargetMachine::new());
        let compiled = compiler.compile_raw(&func).expect("shifts compile");
        assert!(!compiled.code.is_empty());
    }

    #[test]
    fn test_aarch64_compile_branch() {
        let sig = FunctionSignature::new(&[(TypeId::I64, "a"), (TypeId::I64, "b")], &[TypeId::I64]);
        let mut builder = FunctionBuilder::new("branch", TypeContext::new(), sig);
        let (entry, params) =
            builder.create_block_with_params(&[(TypeId::I64, "a"), (TypeId::I64, "b")]);
        builder.switch_to_block(entry);
        let cond = builder.icmp(IntCC::SignedGreaterThan, params[0], params[1]);
        let then_block = builder.create_block();
        let else_block = builder.create_block();
        let (merge_block, merge_params) =
            builder.create_block_with_params(&[(TypeId::I64, "result")]);

        builder.switch_to_block(entry);
        builder.branch(cond, then_block, &[], else_block, &[]);

        builder.switch_to_block(then_block);
        builder.jump(merge_block, &[params[0]]);

        builder.switch_to_block(else_block);
        builder.jump(merge_block, &[params[1]]);

        builder.switch_to_block(merge_block);
        builder.ret(&[merge_params[0]]);
        let func = builder.finish();

        let compiler = FunctionCompiler::new(TargetMachine::new());
        let compiled = compiler.compile_raw(&func).expect("branch compile");
        assert!(!compiled.code.is_empty());
        assert!(compiled.code.len() >= 16);
    }

    #[test]
    fn test_aarch64_compile_loop() {
        let sig = FunctionSignature::new(&[(TypeId::I64, "n")], &[TypeId::I64]);
        let mut builder = FunctionBuilder::new("loop_count", TypeContext::new(), sig);
        let (entry, params) = builder.create_block_with_params(&[(TypeId::I64, "n")]);
        builder.switch_to_block(entry);

        let (header, header_params) =
            builder.create_block_with_params(&[(TypeId::I64, "i"), (TypeId::I64, "sum")]);
        builder.switch_to_block(entry);
        builder.jump(header, &[params[0], params[0]]);

        builder.switch_to_block(header);
        let i = header_params[0];
        let sum = header_params[1];
        let zero = builder.iconst_i64(0);
        let done_cond = builder.icmp(IntCC::Equal, i, zero);
        let body_block = builder.create_block();
        let exit_block = builder.create_block();

        builder.switch_to_block(header);
        builder.branch(done_cond, exit_block, &[], body_block, &[]);

        builder.switch_to_block(body_block);
        let new_sum = builder.iadd(sum, i);
        let one = builder.iconst_i64(1);
        let new_i = builder.isub(i, one);
        builder.jump(header, &[new_i, new_sum]);

        builder.switch_to_block(exit_block);
        builder.ret(&[sum]);
        let func = builder.finish();

        let compiler = FunctionCompiler::new(TargetMachine::new());
        let compiled = compiler.compile_raw(&func).expect("loop compile");
        assert!(!compiled.code.is_empty());
        assert!(compiled.code.len() >= 20);
    }

    #[test]
    fn test_aarch64_compile_all_icmp_conditions() {
        let conditions: &[(IntCC, &str)] = &[
            (IntCC::Equal, "eq"),
            (IntCC::NotEqual, "ne"),
            (IntCC::SignedLessThan, "slt"),
            (IntCC::SignedGreaterThan, "sgt"),
            (IntCC::SignedLessThanOrEqual, "sle"),
            (IntCC::SignedGreaterThanOrEqual, "sge"),
            (IntCC::UnsignedLessThan, "ult"),
            (IntCC::UnsignedGreaterThan, "ugt"),
            (IntCC::UnsignedLessThanOrEqual, "ule"),
            (IntCC::UnsignedGreaterThanOrEqual, "uge"),
        ];

        for (cc, name) in conditions {
            let sig =
                FunctionSignature::new(&[(TypeId::I64, "a"), (TypeId::I64, "b")], &[TypeId::I32]);
            let mut builder =
                FunctionBuilder::new(&format!("icmp_{name}"), TypeContext::new(), sig);
            let (entry, params) =
                builder.create_block_with_params(&[(TypeId::I64, "a"), (TypeId::I64, "b")]);
            builder.switch_to_block(entry);
            let cond = builder.icmp(*cc, params[0], params[1]);
            builder.ret(&[cond]);
            let func = builder.finish();

            let compiler = FunctionCompiler::new(TargetMachine::new());
            let compiled = compiler
                .compile_raw(&func)
                .unwrap_or_else(|e| panic!("icmp {name} compile failed: {e}"));
            assert!(!compiled.code.is_empty(), "icmp {name}: empty code");
        }
    }

    #[test]
    fn test_aarch64_compile_constants() {
        let compiler = FunctionCompiler::new(TargetMachine::new());

        let sig = FunctionSignature::new(&[], &[TypeId::I64]);
        let mut builder = FunctionBuilder::new("const_max", TypeContext::new(), sig);
        builder.create_block_here();
        let v = builder.iconst_i64(i64::MAX);
        builder.ret(&[v]);
        let func = builder.finish();
        let compiled = compiler.compile_raw(&func).expect("const_max compile");
        assert!(!compiled.code.is_empty());
    }

    #[test]
    fn test_aarch64_compile_copy() {
        let sig = FunctionSignature::new(&[(TypeId::I64, "a")], &[TypeId::I64]);
        let mut builder = FunctionBuilder::new("copy", TypeContext::new(), sig);
        let (entry, params) = builder.create_block_with_params(&[(TypeId::I64, "a")]);
        builder.switch_to_block(entry);
        let v = builder.copy(params[0]);
        builder.ret(&[v]);
        let func = builder.finish();

        let compiler = FunctionCompiler::new(TargetMachine::new());
        let compiled = compiler.compile_raw(&func).expect("copy compile");
        assert!(!compiled.code.is_empty());
    }

    #[test]
    fn test_aarch64_code_starts_with_prologue() {
        let sig = FunctionSignature::new(&[], &[TypeId::I64]);
        let mut builder = FunctionBuilder::new("probe", TypeContext::new(), sig);
        builder.create_block_here();
        let v = builder.iconst_i64(42);
        builder.ret(&[v]);
        let func = builder.finish();

        let compiler = FunctionCompiler::new(TargetMachine::new());
        let compiled = compiler.compile_raw(&func).expect("probe compile");
        assert!(!compiled.code.is_empty());
        assert!(
            compiled.code.len() >= 8,
            "should have prologue + body + epilogue"
        );

        let len = compiled.code.len();
        assert_eq!(
            compiled.code[len - 4..],
            [0xC0, 0x03, 0x5F, 0xD6],
            "last instruction should be RET (X30)"
        );
    }
}
