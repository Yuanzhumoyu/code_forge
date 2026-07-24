//! codegen-dsl — ISA-DSL v10: TOML-driven ISA code generator.
//!
//! Proc macros: `isa!` and `isa_from_file!`.

use proc_macro::TokenStream;

mod model;
mod parser;
mod codegen;
mod bitstring;

// Instruction resolver (shared with CST codegen)
mod asm_resolver;

// Grammar rule expression parser (test-only: used by build_grammar_from_model)
#[cfg(test)]
mod grammar_rule;

// CST-based code generation using lang-frontend
mod cst_codegen;

fn compile_source(source: &str) -> Result<proc_macro2::TokenStream, CompileError> {
    let model = parser::parse(source).map_err(CompileError::Parse)?;
    model.validate().map_err(CompileError::Validation)?;
    let inner = codegen::generate(&model).map_err(CompileError::Codegen)?;

    let mod_name = syn::Ident::new(
        &model.meta.name.to_lowercase().replace('-', "_"),
        proc_macro2::Span::call_site(),
    );

    Ok(quote::quote! {
        pub mod #mod_name {
            use crate::prelude::*;
            #inner
        }
    })
}

#[derive(Debug, thiserror::Error)]
enum CompileError {
    #[error("parse: {0}")]
    Parse(String),
    #[error("validate: {0}")]
    Validation(String),
    #[error("codegen: {0}")]
    Codegen(String),
}

/// `isa!{ ... }` — inline TOML ISA definition.
/// Input is raw TOML text (not a string literal).
#[proc_macro]
pub fn isa(input: TokenStream) -> TokenStream {
    compile_source(&input.to_string())
        .map(Into::into)
        .unwrap_or_else(|e| {
            syn::Error::new(proc_macro2::Span::call_site(), e.to_string())
                .to_compile_error()
                .into()
        })
}

/// `isa_from_file!("path/to/arch.toml")` — load ISA from file.
/// Input is a string literal (parsed with syn::LitStr).
#[proc_macro]
pub fn isa_from_file(input: TokenStream) -> TokenStream {
    let lit: syn::LitStr = match syn::parse(input) {
        Ok(lit) => lit,
        Err(e) => return e.to_compile_error().into(),
    };
    let path = lit.value();
    let content = match std::fs::read_to_string(&path)
        .or_else(|_| {
            let manifest_dir = std::env::var("CARGO_MANIFEST_DIR").unwrap_or_default();
            std::fs::read_to_string(std::path::PathBuf::from(&manifest_dir).join(&path))
        }) {
        Ok(c) => c,
        Err(e) => {
            return syn::Error::new(
                proc_macro2::Span::call_site(),
                format!("Cannot read ISA file '{path}': {e}"),
            )
            .to_compile_error()
            .into();
        }
    };
    compile_source(&content)
        .map(Into::into)
        .unwrap_or_else(|e| {
            syn::Error::new(
                proc_macro2::Span::call_site(),
                format!("ISA-DSL error in '{path}':\n{e}"),
            )
            .to_compile_error()
            .into()
        })
}
