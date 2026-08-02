//! Proc-macro crate for forge-hir.
//!
//! Provides the `define_lowering!` macro — a declarative DSL for declaring a
//! language's **op catalog**: the atomic IR operations (with their ports,
//! attributes and backend opcode mapping) that the language's manual lowering
//! code emits.

use proc_macro::TokenStream;

mod codegen;
mod parser;

/// Define a language's op catalog (the "atom" layer of the brick framework).
///
/// Each `atom` expands to three things inside the generated module:
/// - `xxx_tag() -> OpTag` — the symbolic operation identifier
/// - `build_xxx(&mut IrGraph, …) -> Result<…, HirError>` — a type-safe
///   constructor (attributes first, then SSA operands, in DSL declaration
///   order), mirroring the Cranelift `InstBuilder` pattern
/// - a registration entry for `register_atoms(&mut BrickRegistry)`
///
/// AST→IR lowering is **not** declared here: write it by hand against
/// `forge_hir::HirCtx`, calling the generated `build_xxx` constructors.
///
/// # Example
///
/// ```ignore
/// use forge_hir_macro::define_lowering;
///
/// define_lowering! {
///     language MiniC;
///
///     atom iadd {
///         inputs { lhs: i32, rhs: i32 }
///         outputs { result: i32 }
///         maps_to arith.iadd;
///     }
/// }
/// ```
#[proc_macro]
pub fn define_lowering(input: TokenStream) -> TokenStream {
    let input_str = input.to_string();

    if input_str.trim().is_empty() {
        return TokenStream::new();
    }

    // Parse the DSL input
    let spec = match parser::parse_lowering_spec(&input_str) {
        Ok(spec) => spec,
        Err(e) => {
            return syn::Error::new(proc_macro2::Span::call_site(), e)
                .to_compile_error()
                .into();
        }
    };

    // Generate Rust code from the spec
    let expanded = codegen::generate_lowering_code(&spec);
    expanded.into()
}
