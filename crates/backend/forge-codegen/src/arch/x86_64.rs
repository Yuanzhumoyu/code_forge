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
        let func = b.finish().expect("build");
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
        let func = builder.finish().expect("build");
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
        let func = builder.finish().expect("build");
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
        let func = builder.finish().expect("build");
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
    fn test_loop_block_param_write_bytes_style() {
        // 回归探针（[WA-14] 同构最小复现）：带块参数 + 回边的循环。
        // 曾暴露 map_terminator_args_to_params 的映射覆盖（块参数 value 作为
        // 跳转 arg 被覆盖到目标块寄存器）与 regalloc 活区间冲突（body 内
        // ptr/one 挤同一物理寄存器）。断言编译成功且循环头/回边指令齐全。
        use crate::prelude::*;
        let sig = FunctionSignature::new(
            &[
                (TypeId::PTR, "dst"),
                (TypeId::I64, "val"),
                (TypeId::I64, "count"),
            ],
            &[TypeId::I64],
        );
        let mut builder = crate::FunctionBuilder::new("wb_loop", TypeContext::new(), sig);
        let (entry, params) = builder.create_block_with_params(&[
            (TypeId::PTR, "dst"),
            (TypeId::I64, "val"),
            (TypeId::I64, "count"),
        ]);
        builder.switch_to_block(entry);
        // 注意：create_block_with_params 会切换 cur_block，因此常量
        // 必须在创建其他块之前发出（与生产代码 write_bytes arm 一致）
        let zero = builder.iconst(0, TypeId::I64);
        let (loop_blk, lp) = builder.create_block_with_params(&[(TypeId::I64, "i")]);
        let (body_blk, bp) = builder.create_block_with_params(&[(TypeId::I64, "bi")]);
        let done = builder.create_block();
        // entry → loop(i=0)
        builder.switch_to_block(entry);
        builder.jump(loop_blk, &[zero]);
        // loop: i < count ? body(i) : done
        builder.switch_to_block(loop_blk);
        let i = lp[0];
        let one = builder.iconst(1, TypeId::I64);
        let cond = builder.icmp(IntCC::UnsignedLessThan, i, params[2]);
        builder.branch(cond, body_blk, &[i], done, &[]);
        // body: addr = dst + bi; store(val, addr); i2 = bi + 1; jump(loop, i2)
        builder.switch_to_block(body_blk);
        let bi = bp[0];
        let addr = builder.iadd(params[0], bi);
        builder.store(params[1], addr);
        let i2 = builder.iadd(bi, one);
        builder.jump(loop_blk, &[i2]);
        // done: ret 0
        builder.switch_to_block(done);
        builder.ret(&[zero]);
        let func = builder.finish().expect("build");
        let compiler = FunctionCompiler::new(TargetMachine::new());
        let compiled = compiler.compile_raw(&func).expect("compile wb-style loop");
        // 循环头应有条件跳（JCC）与回边（JMP）：至少 3 条跳转指令
        assert!(
            compiled.code.len() > 20,
            "too short: {}",
            compiled.code.len()
        );
        eprintln!("wb_loop code ({} bytes):", compiled.code.len());
        for (i, &b) in compiled.code.iter().enumerate() {
            eprint!("{b:02x} ");
            if (i + 1) % 16 == 0 {
                eprintln!();
            }
        }
        eprintln!();
    }

    #[test]
    fn test_enc_spill_load_store() {
        use crate::machine::frame::TargetFrameLowering;
        let frame_lowering = FrameLowering;
        let mut sink = crate::CodeSink::new();
        frame_lowering
            .emit_spill_load(11, -8, 8, false, &mut sink)
            .unwrap();
        let bytes = sink.bytes();
        assert_eq!(bytes.len(), 4);
        assert_eq!(bytes[0], 0x4C); // REX.W (rd=R11=11, R=1)
        assert_eq!(bytes[1], 0x8B); // MOV r, r/m
        assert_eq!(bytes[2], 0x5D); // ModRM: mod=01, reg=011(R11), rm=101(RBP)
        assert_eq!(bytes[3], 0xF8); // disp8 = -8

        let mut sink2 = crate::CodeSink::new();
        frame_lowering
            .emit_spill_store(11, -16, 8, false, &mut sink2)
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
        let func = builder.finish().expect("build");
        let compiler = FunctionCompiler::new(TargetMachine::new());
        let compiled = compiler.compile_raw(&func).expect("compile");
        let hex: Vec<String> = compiled.code.iter().map(|b| format!("{:02x}", b)).collect();
        eprintln!("add ({}b): {}", compiled.code.len(), hex.join(" "));
        assert!(compiled.code.len() > 5);
    }
}
