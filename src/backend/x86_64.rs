//! x86_64 ISA — CST-driven codegen (base.lx grammar).
use crate::Registry;
use crate::register_backend;

codegen_dsl::isa_from_file!("examples/isa/x86_64_v10.toml");

pub type X86Isa = self::x86_64::Isa;
pub type X86Inst = self::x86_64::Inst;
pub type X86Reg = self::x86_64::Reg;

pub fn ensure_registered() {
    static INIT: std::sync::OnceLock<()> = std::sync::OnceLock::new();
    INIT.get_or_init(|| {
        if !Registry::global().contains("x86_64") {
            register_backend!(X86Isa);
        }
    });
}
pub fn generate_trampoline(t: u64, s: u64) -> Vec<u8> {
    let d = t as i64 - s as i64;
    if (-0x8000_0000i64..0x8000_0000).contains(&d) {
        let mut c = vec![0xE9];
        c.extend_from_slice(&((d - 5) as i32).to_le_bytes());
        c
    } else {
        let mut c = vec![0x48, 0xB8];
        c.extend_from_slice(&t.to_le_bytes());
        c.push(0xFF);
        c.push(0xE0);
        c
    }
}
pub fn trampoline_max_size() -> usize {
    12
}
#[unsafe(no_mangle)]
pub fn __codegen_isa_register(_: &Registry) {
    ensure_registered();
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn test_types() {
        let _ = std::mem::size_of::<X86Isa>();
        let _ = std::mem::size_of::<X86Inst>();
    }
    #[test]
    fn test_tramp_near() {
        let c = generate_trampoline(0x1000, 0x100);
        assert_eq!(c.len(), 5);
        assert_eq!(c[0], 0xE9);
    }
    #[test]
    fn test_tramp_far() {
        let c = generate_trampoline(0x8_0000_0000, 0x100);
        assert_eq!(c.len(), 12);
        assert_eq!(c[0], 0x48);
    }
    #[test]
    fn test_modrm() {
        assert_eq!(crate::backend::encode::modrm(3, 0, 0), 0xC0);
        assert_eq!(crate::backend::encode::modrm(3, 1, 2), 0xCA);
    }

    #[test]
    fn test_x86_diag_fadd() {
        use crate::ir::*;
        let sig = Signature::new(&[], &[Type::F64]);
        let mut b = crate::FunctionBuilder::new("fadd", sig);
        b.create_block_here();
        let a = b.fconst_f64(2.5);
        let c = b.fconst_f64(3.5);
        let sum = b.fadd(a, c);
        b.return_(&[sum]);
        let func = b.finish();
        let compiled = crate::FunctionCompiler::<X86Isa>::compile_raw(&func).expect("compile");
        eprintln!("fadd code ({} bytes):", compiled.code.len());
        for (i, &byte) in compiled.code.iter().enumerate() {
            eprint!("{:02x} ", byte);
            if (i + 1) % 16 == 0 { eprintln!(); }
        }
        eprintln!();
        assert!(compiled.code.len() > 5, "too short");
    }

    #[test]
    fn test_x86_diag_bytes() {
        use crate::ir::*;
        let sig = Signature::new(&[(Type::I32, "a"), (Type::I32, "b")], &[Type::I32]);
        let mut builder = crate::FunctionBuilder::new("add", sig);
        let (entry, params) =
            builder.create_block_with_params(&[(Type::I32, "a"), (Type::I32, "b")]);
        builder.switch_to_block(entry);
        let sum = builder.iadd(params[0], params[1]);
        builder.return_(&[sum]);
        let func = builder.finish();
        let compiled = crate::FunctionCompiler::<X86Isa>::compile_raw(&func).expect("compile");
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
        use crate::ir::*;
        let sig = Signature::new(&[], &[Type::I32]);
        let mut builder = crate::FunctionBuilder::new("mov_ri", sig);
        let entry = builder.create_block();
        builder.switch_to_block(entry);
        let v = builder.iconst_i32(42);
        builder.return_(&[v]);
        let func = builder.finish();
        eprintln!("Constant pool len: {}", func.constant_pool.len());
        let compiled = crate::FunctionCompiler::<X86Isa>::compile_raw(&func).expect("compile");
        eprintln!("mov_ri code ({} bytes):", compiled.code.len());
        for (i, &b) in compiled.code.iter().enumerate() {
            eprint!("{:02x} ", b);
            if (i + 1) % 16 == 0 { eprintln!(); }
        }
        eprintln!();
        assert!(compiled.code.len() > 5, "too short");
    }

    #[test]
    fn test_x86_diag() {
        use crate::ir::*;
        let sig = Signature::new(&[(Type::I32, "a"), (Type::I32, "b")], &[Type::I32]);
        let mut builder = crate::FunctionBuilder::new("add", sig);
        let (entry, params) =
            builder.create_block_with_params(&[(Type::I32, "a"), (Type::I32, "b")]);
        builder.switch_to_block(entry);
        let sum = builder.iadd(params[0], params[1]);
        builder.return_(&[sum]);
        let func = builder.finish();
        let compiled = crate::FunctionCompiler::<X86Isa>::compile_raw(&func).expect("compile");
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
    fn test_x86_diag_add() {
        use crate::ir::*;
        let sig = Signature::new(&[(Type::I32, "a"), (Type::I32, "b")], &[Type::I32]);
        let mut builder = crate::FunctionBuilder::new("add", sig);
        let (entry, params) =
            builder.create_block_with_params(&[(Type::I32, "a"), (Type::I32, "b")]);
        builder.switch_to_block(entry);
        let sum = builder.iadd(params[0], params[1]);
        builder.return_(&[sum]);
        let func = builder.finish();
        let compiled = crate::FunctionCompiler::<X86Isa>::compile_raw(&func).expect("compile");
        let hex: Vec<String> = compiled.code.iter().map(|b| format!("{:02x}", b)).collect();
        eprintln!("add ({}b): {}", compiled.code.len(), hex.join(" "));
        assert!(compiled.code.len() > 5);
    }

}
