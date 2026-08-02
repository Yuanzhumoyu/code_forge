//! x86_64 ISA — v19 DSL-generated TargetMachine.
forge_dsl::isa_from_file!("isa/x86_v10.toml");
pub use self::x86_64::*;

#[cfg(test)]
mod tests {
    use super::*;
    use crate::pipeline::compiler::FunctionCompiler;
    use crate::prelude::*;
    use forge_ir::{FunctionSignature, TypeContext};

    #[test]
    fn test_types() {
        let _ = std::mem::size_of::<TargetMachine>();
        let _ = std::mem::size_of::<Inst>();
    }

    #[test]
    fn test_modrm() {
        assert_eq!(modrm(3, 0, 0), 0xC0);
        assert_eq!(modrm(3, 1, 2), 0xCA);
    }

    #[test]
    fn test_x86_diag_fadd() {
        let sig = FunctionSignature::new(&[], &[TypeId::F64]);
        let mut b = crate::FunctionBuilder::new("fadd", TypeContext::new(), sig);
        b.create_block_here();
        let a = b.fconst_f64(2.5);
        let c = b.fconst_f64(3.5);
        let sum = b.fadd(a, c);
        b.ret(&[sum]);
        let func = b.finish();
        let compiler = FunctionCompiler::new(TargetMachine::new());
        let compiled = compiler.compile_raw(&func).expect("compile");
        eprintln!("fadd code ({} bytes):", compiled.code.len());
        for (i, &byte) in compiled.code.iter().enumerate() {
            eprint!("{:02x} ", byte);
            if (i + 1) % 16 == 0 {
                eprintln!();
            }
        }
        eprintln!();
        assert!(compiled.code.len() > 5, "too short");
    }

    #[test]
    fn test_x86_diag_bytes() {
        let sig = FunctionSignature::new(&[(TypeId::I32, "a"), (TypeId::I32, "b")], &[TypeId::I32]);
        let mut builder = crate::FunctionBuilder::new("add", TypeContext::new(), sig);
        let (entry, params) =
            builder.create_block_with_params(&[(TypeId::I32, "a"), (TypeId::I32, "b")]);
        builder.switch_to_block(entry);
        let sum = builder.iadd(params[0], params[1]);
        builder.ret(&[sum]);
        let func = builder.finish();
        let compiler = FunctionCompiler::new(TargetMachine::new());
        let compiled = compiler.compile_raw(&func).expect("compile");
        eprintln!("add code ({} bytes):", compiled.code.len());
        for (i, &b) in compiled.code.iter().enumerate() {
            eprint!("{:02x} ", b);
            if (i + 1) % 16 == 0 {
                eprintln!();
            }
        }
        eprintln!();
        assert!(compiled.code.len() > 5, "too short");
    }

    #[test]
    fn test_diag_iconst() {
        let sig = FunctionSignature::new(&[], &[TypeId::I32]);
        let mut builder = crate::FunctionBuilder::new("mov_ri", TypeContext::new(), sig);
        let entry = builder.create_block();
        builder.switch_to_block(entry);
        let v = builder.iconst_i32(42);
        builder.ret(&[v]);
        let func = builder.finish();
        eprintln!("Constant pool len: {}", func.constants.len());
        let compiler = FunctionCompiler::new(TargetMachine::new());
        let compiled = compiler.compile_raw(&func).expect("compile");
        eprintln!("mov_ri code ({} bytes):", compiled.code.len());
        for (i, &b) in compiled.code.iter().enumerate() {
            eprint!("{:02x} ", b);
            if (i + 1) % 16 == 0 {
                eprintln!();
            }
        }
        eprintln!();
        assert!(compiled.code.len() > 5, "too short");
    }

    #[test]
    fn test_x86_diag() {
        let sig = FunctionSignature::new(&[(TypeId::I32, "a"), (TypeId::I32, "b")], &[TypeId::I32]);
        let mut builder = crate::FunctionBuilder::new("add", TypeContext::new(), sig);
        let (entry, params) =
            builder.create_block_with_params(&[(TypeId::I32, "a"), (TypeId::I32, "b")]);
        builder.switch_to_block(entry);
        let sum = builder.iadd(params[0], params[1]);
        builder.ret(&[sum]);
        let func = builder.finish();
        let compiler = FunctionCompiler::new(TargetMachine::new());
        let compiled = compiler.compile_raw(&func).expect("compile");
        eprintln!("add code ({} bytes):", compiled.code.len());
        for (i, &b) in compiled.code.iter().enumerate() {
            eprint!("{:02x} ", b);
            if (i + 1) % 16 == 0 {
                eprintln!();
            }
        }
        eprintln!();
        assert!(compiled.code.len() > 5);
    }

    #[test]
    fn test_enc_lea_sib() {
        let mut sink = crate::CodeSink::new();
        enc_lea_sib(&mut sink, 0, 1, 2, 1, 0);
        let bytes = sink.bytes();
        assert_eq!(bytes[0], 0x48); // REX.W
        assert_eq!(bytes[1], 0x8D); // LEA
        assert_eq!(bytes[2], 0x04); // ModRM: mod=0, reg=0, rm=4(SIB)
        assert_eq!(bytes[3], 0x11); // SIB: scale=0, index=2, base=1
    }

    #[test]
    fn test_enc_spill_load_store() {
        use crate::machine::frame::TargetFrameLowering;
        let frame_lowering = FrameLowering;
        let mut sink = crate::CodeSink::new();
        frame_lowering
            .emit_spill_load(11, -8, 8, &mut sink)
            .unwrap();
        let bytes = sink.bytes();
        assert_eq!(bytes.len(), 4);
        assert_eq!(bytes[0], 0x4C); // REX.W (rd=R11=11, R=1)
        assert_eq!(bytes[1], 0x8B); // MOV r, r/m
        assert_eq!(bytes[2], 0x5D); // ModRM: mod=01, reg=011(R11), rm=101(RBP)
        assert_eq!(bytes[3], 0xF8); // disp8 = -8

        let mut sink2 = crate::CodeSink::new();
        frame_lowering
            .emit_spill_store(11, -16, 8, &mut sink2)
            .unwrap();
        let bytes2 = sink2.bytes();
        assert_eq!(bytes2.len(), 4);
        assert_eq!(bytes2[0], 0x4C);
        assert_eq!(bytes2[1], 0x89);
        assert_eq!(bytes2[2], 0x5D);
        assert_eq!(bytes2[3], 0xF0); // disp8 = -16
    }

    #[test]
    fn test_x86_diag_add() {
        let sig = FunctionSignature::new(&[(TypeId::I32, "a"), (TypeId::I32, "b")], &[TypeId::I32]);
        let mut builder = crate::FunctionBuilder::new("add", TypeContext::new(), sig);
        let (entry, params) =
            builder.create_block_with_params(&[(TypeId::I32, "a"), (TypeId::I32, "b")]);
        builder.switch_to_block(entry);
        let sum = builder.iadd(params[0], params[1]);
        builder.ret(&[sum]);
        let func = builder.finish();
        let compiler = FunctionCompiler::new(TargetMachine::new());
        let compiled = compiler.compile_raw(&func).expect("compile");
        let hex: Vec<String> = compiled.code.iter().map(|b| format!("{:02x}", b)).collect();
        eprintln!("add ({}b): {}", compiled.code.len(), hex.join(" "));
        assert!(compiled.code.len() > 5);
    }
}
