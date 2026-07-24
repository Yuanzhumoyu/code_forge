//! WebAssembly (wasm32) backend — generated from DSL.
//!
//! Provides Wasm binary format code generation via the standard
//! SD_* instruction set with Wasm-specific emit templates.

use crate::Registry;
use crate::register_backend;

codegen_dsl::isa_from_file!("examples/isa/wasm32_v10.toml");

pub type Wasm32Isa = self::wasm32::Isa;
pub type Wasm32Inst = self::wasm32::Inst;

pub fn ensure_registered() {
    static INIT: std::sync::OnceLock<()> = std::sync::OnceLock::new();
    INIT.get_or_init(|| {
        if !Registry::global().contains("wasm32") {
            register_backend!(Wasm32Isa);
        }
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_wasm32_types() {
        let _ = std::mem::size_of::<Wasm32Isa>();
        let _ = std::mem::size_of::<Wasm32Inst>();
    }

    #[test]
    fn test_wasm32_registration() {
        ensure_registered();
        assert!(Registry::global().contains("wasm32"));
    }

    #[test]
    fn test_wasm32_compile_iconst() {
        use crate::ir::*;
        use crate::backend::FunctionCompiler;

        let sig = Signature::new(&[], &[Type::I32]);
        let mut builder = crate::FunctionBuilder::new("answer", sig);
        let entry = builder.create_block();
        builder.switch_to_block(entry);
        let v = builder.iconst_i32(42);
        builder.return_(&[v]);
        let func = builder.finish();

        let result = FunctionCompiler::<Wasm32Isa>::compile_raw(&func);
        assert!(result.is_ok(), "Wasm32 compilation should succeed: {:?}", result.err());
        let compiled = result.unwrap();
        assert!(!compiled.code.is_empty(), "Wasm32 code should not be empty");
        // Wasm binary should start with function body locals count
        // and end with 'end' opcode (0x0B)
        assert_eq!(compiled.code.last(), Some(&0x0B), "Wasm function should end with 'end' opcode");
    }
}
