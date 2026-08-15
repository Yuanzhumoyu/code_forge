//! Code generation — converts parsed DSL specs into Rust token streams.
//!
//! Each `atom` produces:
//! - `xxx_tag() -> OpTag` — the symbolic operation identifier
//! - `build_xxx(&mut IrGraph, …) -> Result<…, HirError>` — a type-safe
//!   constructor (attributes first, then value operands, matching the DSL
//!   declaration order), mirroring the Cranelift `InstBuilder` pattern
//! - registration entry for `register_atoms(&mut BrickRegistry)`

use crate::parser::{AtomDef, LoweringSpec};
use proc_macro2::TokenStream;
use quote::{format_ident, quote};

pub fn generate_lowering_code(spec: &LoweringSpec) -> TokenStream {
    let language_mod = format_ident!("{}_lowering", spec.language.to_lowercase());

    let atom_constants = spec.atoms.iter().map(generate_atom_constant);
    let atom_constructors = spec.atoms.iter().map(generate_atom_constructor);
    let atom_registrations = spec.atoms.iter().map(generate_atom_registration);

    // Emit the IntCC → string helper only if some atom declares an IntCC attr.
    let needs_intcc = spec
        .atoms
        .iter()
        .any(|a| a.attrs.iter().any(|(_, t)| t == "IntCC"));
    let intcc_helper = if needs_intcc {
        quote! {
            fn _intcc_str(cc: ::forge_hir::ir::IntCC) -> &'static str {
                match cc {
                    ::forge_hir::ir::IntCC::Equal => "eq",
                    ::forge_hir::ir::IntCC::NotEqual => "ne",
                    ::forge_hir::ir::IntCC::SignedLessThan => "slt",
                    ::forge_hir::ir::IntCC::SignedGreaterThan => "sgt",
                    ::forge_hir::ir::IntCC::SignedLessThanOrEqual => "sle",
                    ::forge_hir::ir::IntCC::SignedGreaterThanOrEqual => "sge",
                    ::forge_hir::ir::IntCC::UnsignedLessThan => "ult",
                    ::forge_hir::ir::IntCC::UnsignedGreaterThan => "ugt",
                    ::forge_hir::ir::IntCC::UnsignedLessThanOrEqual => "ule",
                    ::forge_hir::ir::IntCC::UnsignedGreaterThanOrEqual => "uge",
                }
            }
        }
    } else {
        quote! {}
    };

    let expanded = quote! {
        #[allow(unused_imports)]
        pub mod #language_mod {
            use ::forge_hir::*;
            use ::forge_hir::atom::*;

            #(#atom_constants)*
            #intcc_helper
            #(#atom_constructors)*

            pub fn register_atoms(registry: &mut BrickRegistry) {
                #(#atom_registrations)*
            }
        }
    };
    expanded
}

fn generate_atom_constant(atom: &AtomDef) -> TokenStream {
    let fn_name = format_ident!("{}_tag", atom.name);
    let (dialect, name) = atom
        .maps_to
        .as_deref()
        .and_then(|m| m.split_once('.'))
        .map(|(d, n)| (d.to_string(), n.to_string()))
        .unwrap_or_else(|| ("custom".to_string(), atom.name.clone()));

    quote! {
        pub fn #fn_name() -> OpTag { OpTag::new(#dialect, #name) }
    }
}

/// Generate a type-safe constructor: `build_xxx(graph, attr_args..., input_args...)`.
fn generate_atom_constructor(atom: &AtomDef) -> TokenStream {
    let fn_name = format_ident!("build_{}", atom.name);
    let tag_fn = format_ident!("{}_tag", atom.name);
    let ret_ty = ret_ty_for(atom);

    // Attribute parameters (in DSL declaration order).
    let attr_params: Vec<TokenStream> = atom
        .attrs
        .iter()
        .map(|(n, t)| {
            let ident = format_ident!("{}", n);
            let ty = attr_rust_type(t);
            quote! { #ident: #ty }
        })
        .collect();

    // Input parameters — SSA operands are GraphValue handles; `Block` inputs
    // are compile-time block references (stored as attributes, not operands).
    let input_params: Vec<TokenStream> = atom
        .inputs
        .iter()
        .map(|(n, t)| {
            let ident = format_ident!("{}", n);
            if t == "Block" {
                quote! { #ident: ::forge_hir::block::BlockId }
            } else {
                quote! { #ident: ::forge_hir::GraphValue }
            }
        })
        .collect();

    // Attribute storage entries.
    let attr_entries: Vec<TokenStream> = atom
        .attrs
        .iter()
        .map(|(n, t)| {
            let ident = format_ident!("{}", n);
            let key = n.as_str();
            let val = attr_storage(t, &ident);
            quote! { _attrs.insert(::forge_hir::attr::sym_intern(#key), #val); }
        })
        .collect();

    // Block inputs go into the attribute map as well.
    let block_attr_entries: Vec<TokenStream> = atom
        .inputs
        .iter()
        .filter(|(_, t)| t == "Block")
        .map(|(n, _)| {
            let ident = format_ident!("{}", n);
            let key = n.as_str();
            quote! {
                _attrs.insert(
                    ::forge_hir::attr::sym_intern(#key),
                    ::forge_hir::AttrValue::BlockId(#ident),
                );
            }
        })
        .collect();

    // SSA operands (non-Block inputs).
    let input_refs: Vec<TokenStream> = atom
        .inputs
        .iter()
        .filter(|(_, t)| t != "Block")
        .map(|(n, _)| {
            let ident = format_ident!("{}", n);
            quote! { #ident }
        })
        .collect();

    let result_tys = output_result_tys(atom);
    let ret_expr = match atom.outputs.len() {
        0 => quote! { Ok(()) },
        1 => quote! { Ok(graph.node_results(_node)[0]) },
        _ => quote! {
            compile_error!("multiple-output atoms are not yet supported")
        },
    };

    quote! {
        pub fn #fn_name(
            graph: &mut IrGraph,
            #(#attr_params,)*
            #(#input_params,)*
        ) -> Result<#ret_ty, HirError> {
            let mut _attrs = std::collections::HashMap::new();
            #(#attr_entries)*
            #(#block_attr_entries)*
            let _node = graph.emit(
                #tag_fn(),
                &[#(#input_refs),*],
                _attrs,
                #result_tys,
            )?;
            #ret_expr
        }
    }
}

// The Rust return type of a build_xxx constructor.
fn ret_ty_for(atom: &AtomDef) -> TokenStream {
    match atom.outputs.len() {
        0 => quote! { () },
        _ => quote! { ::forge_hir::GraphValue },
    }
}

fn generate_atom_registration(atom: &AtomDef) -> TokenStream {
    let tag_fn = format_ident!("{}_tag", atom.name);
    let backend_op = map_backend_opcode(atom.maps_to.as_deref().unwrap_or(""));
    let atom_name = &atom.name;

    let inputs: Vec<_> = atom
        .inputs
        .iter()
        .map(|(n, t)| {
            let ty = map_port_type(t);
            quote! { .input(#n, #ty) }
        })
        .collect();

    let outputs: Vec<_> = atom
        .outputs
        .iter()
        .map(|(n, t)| {
            let ty = map_port_type(t);
            quote! { .output(#n, #ty) }
        })
        .collect();

    let attrs: Vec<_> = atom
        .attrs
        .iter()
        .map(|(n, t)| {
            match t.as_str() {
                "i32" | "i64" | "u32" | "u64" => quote! { .attr_int(#n) },
                "Type" | "type" => quote! { .attr_ty(#n) },
                // IntCC / str / anything else is stored as an interned string.
                _ => quote! { .attr_str(#n) },
            }
        })
        .collect();

    let regions: Vec<_> = atom
        .regions
        .iter()
        .map(|n| {
            quote! { .region(#n) }
        })
        .collect();

    quote! {
        {
            let spec = AtomSpec::new(#tag_fn(), #backend_op) #(#inputs)* #(#outputs)* #(#attrs)* #(#regions)*;
            if let Err(e) = registry.register_atom(spec) {
                panic!("Failed to register atom '{}': {}", #atom_name, e);
            }
        }
    }
}

// ============================================================
// Type mapping helpers
// ============================================================

/// Rust parameter type for an attribute declared in the DSL.
fn attr_rust_type(t: &str) -> TokenStream {
    match t {
        "i32" => quote! { i32 },
        "i64" => quote! { i64 },
        "u32" => quote! { u32 },
        "u64" => quote! { u64 },
        "Type" | "type" => quote! { ::forge_hir::ir::TypeId },
        "IntCC" => quote! { ::forge_hir::ir::IntCC },
        other => quote! {
            compile_error!(concat!("unsupported attribute type in define_lowering!: ", #other))
        },
    }
}

/// How to store an attribute value given its DSL type.
fn attr_storage(t: &str, ident: &proc_macro2::Ident) -> TokenStream {
    match t {
        "i32" => quote! { ::forge_hir::AttrValue::Int(#ident as i64) },
        "i64" => quote! { ::forge_hir::AttrValue::Int(#ident) },
        "u32" => quote! { ::forge_hir::AttrValue::Uint(#ident as u64) },
        "u64" => quote! { ::forge_hir::AttrValue::Uint(#ident) },
        "Type" | "type" => quote! { ::forge_hir::AttrValue::Type(#ident) },
        "IntCC" => quote! { ::forge_hir::AttrValue::str(_intcc_str(#ident)) },
        _ => quote! { compile_error!("unsupported attribute type in define_lowering!") },
    }
}

/// Result types for the emitted node, based on the declared outputs.
/// A `value`-typed output picks up a `Type` attribute if one is declared
/// (e.g. `load { attrs { ty: Type } outputs { result: value } }`), else I32.
fn output_result_tys(atom: &AtomDef) -> TokenStream {
    match atom.outputs.len() {
        0 => quote! { &[] },
        1 => {
            let (_, ty) = &atom.outputs[0];
            match ty.as_str() {
                "i1" | "bool" => quote! { &[::forge_hir::ir::TypeId::BOOL] },
                "i8" => quote! { &[::forge_hir::ir::TypeId::I8] },
                "i16" => quote! { &[::forge_hir::ir::TypeId::I16] },
                "i32" => quote! { &[::forge_hir::ir::TypeId::I32] },
                "i64" => quote! { &[::forge_hir::ir::TypeId::I64] },
                "f32" => quote! { &[::forge_hir::ir::TypeId::F32] },
                "f64" => quote! { &[::forge_hir::ir::TypeId::F64] },
                "ptr" => quote! { &[::forge_hir::ir::TypeId::PTR] },
                "value" => {
                    // Look for a `Type` attribute (e.g. `to`, `ty`) declared
                    // alongside; use its parameter as the result type.
                    let ty_attr = atom
                        .attrs
                        .iter()
                        .find(|(_, t)| t == "Type" || t == "type")
                        .map(|(n, _)| format_ident!("{}", n));
                    match ty_attr {
                        Some(ident) => quote! { &[#ident] },
                        None => quote! { &[::forge_hir::ir::TypeId::I32] },
                    }
                }
                _ => quote! { compile_error!("unsupported output type in define_lowering!") },
            }
        }
        _ => quote! { compile_error!("multiple-output atoms are not yet supported") },
    }
}

/// Port type used when registering the AtomSpec (documentation/validation only).
fn map_port_type(ty: &str) -> TokenStream {
    match ty {
        "i1" | "bool" => quote! { ::forge_hir::ir::TypeId::BOOL },
        "i8" => quote! { ::forge_hir::ir::TypeId::I8 },
        "i16" => quote! { ::forge_hir::ir::TypeId::I16 },
        "i32" => quote! { ::forge_hir::ir::TypeId::I32 },
        "i64" => quote! { ::forge_hir::ir::TypeId::I64 },
        "f32" => quote! { ::forge_hir::ir::TypeId::F32 },
        "f64" => quote! { ::forge_hir::ir::TypeId::F64 },
        "ptr" => quote! { ::forge_hir::ir::TypeId::PTR },
        "void" => quote! { ::forge_hir::ir::TypeId::VOID },
        // `value` (polymorphic), `Block`, and anything unknown: registry port
        // types are not enforced during lowering, so I32 is a safe placeholder.
        _ => quote! { ::forge_hir::ir::TypeId::I32 },
    }
}

fn map_backend_opcode(maps_to: &str) -> TokenStream {
    match maps_to {
        "arith.iconst" => quote! { ::forge_hir::ir_opcode::Opcode::Iconst },
        "arith.fconst" => quote! { ::forge_hir::ir_opcode::Opcode::Fconst },
        "arith.iadd" => quote! { ::forge_hir::ir_opcode::Opcode::Iadd },
        "arith.isub" => quote! { ::forge_hir::ir_opcode::Opcode::Isub },
        "arith.imul" => quote! { ::forge_hir::ir_opcode::Opcode::Imul },
        "arith.sdiv" => quote! { ::forge_hir::ir_opcode::Opcode::Sdiv },
        "arith.udiv" => quote! { ::forge_hir::ir_opcode::Opcode::Udiv },
        "arith.srem" => quote! { ::forge_hir::ir_opcode::Opcode::Srem },
        "arith.urem" => quote! { ::forge_hir::ir_opcode::Opcode::Urem },
        "arith.and" => quote! { ::forge_hir::ir_opcode::Opcode::Band },
        "arith.or" => quote! { ::forge_hir::ir_opcode::Opcode::Bor },
        "arith.xor" => quote! { ::forge_hir::ir_opcode::Opcode::Bxor },
        "arith.not" => quote! { ::forge_hir::ir_opcode::Opcode::Bnot },
        "arith.shl" => quote! { ::forge_hir::ir_opcode::Opcode::Ishl },
        "arith.ushr" => quote! { ::forge_hir::ir_opcode::Opcode::Ushr },
        "arith.sshr" => quote! { ::forge_hir::ir_opcode::Opcode::Sshr },
        "arith.icmp" => {
            quote! { ::forge_hir::ir_opcode::Opcode::Icmp { cond: ::forge_hir::ir::IntCC::Equal } }
        }
        "arith.fcmp" => {
            quote! { ::forge_hir::ir_opcode::Opcode::Fcmp { cond: ::forge_hir::ir::FloatCC::Equal } }
        }
        "arith.sextend" => quote! { ::forge_hir::ir_opcode::Opcode::Sextend },
        "arith.uextend" => quote! { ::forge_hir::ir_opcode::Opcode::Uextend },
        "arith.ireduce" => quote! { ::forge_hir::ir_opcode::Opcode::Ireduce },
        "arith.bitcast" => quote! { ::forge_hir::ir_opcode::Opcode::Bitcast },
        "arith.select" => quote! { ::forge_hir::ir_opcode::Opcode::Select },
        "cf.branch" => quote! { ::forge_hir::ir_opcode::Opcode::Nop },
        "cf.jump" => quote! { ::forge_hir::ir_opcode::Opcode::Nop },
        "cf.ret" => quote! { ::forge_hir::ir_opcode::Opcode::Nop },
        "cf.switch" => quote! { ::forge_hir::ir_opcode::Opcode::Nop },
        "cf.unreachable" => quote! { ::forge_hir::ir_opcode::Opcode::Nop },
        "mem.load" => quote! { ::forge_hir::ir_opcode::Opcode::Load },
        "mem.store" => quote! { ::forge_hir::ir_opcode::Opcode::Store },
        "mem.stack_addr" => quote! { ::forge_hir::ir_opcode::Opcode::StackAddr },
        "mem.global_addr" => quote! { ::forge_hir::ir_opcode::Opcode::GlobalAddr },
        "mem.alloca" => quote! { ::forge_hir::ir_opcode::Opcode::Alloca },
        "mem.gep" => quote! { ::forge_hir::ir_opcode::Opcode::GetElementPtr },
        // 未知 maps_to 静默落 Nop 曾导致 HIR 图丢失指令（审计 P3）——宏展开期即报错
        _ => {
            let msg = format!("unknown maps_to dialect: `{maps_to}`（合法前缀：arith.* / cf.* / mem.*）");
            quote! { compile_error!(#msg); }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::parser::parse_lowering_spec;

    /// quote! token streams stringify with spaces (e.g. `Result < () , HirError >`);
    /// strip whitespace before matching so assertions are layout-independent.
    fn codegen_str(input: &str) -> String {
        let spec = parse_lowering_spec(input).unwrap();
        generate_lowering_code(&spec).to_string().replace(' ', "")
    }

    #[test]
    fn test_generates_tag_and_builder() {
        let out = codegen_str(
            r#"language T; atom iadd { inputs { lhs: i32, rhs: i32 } outputs { result: i32 } maps_to arith.iadd; }"#,
        );
        assert!(out.contains("pubfniadd_tag()"), "missing tag fn: {}", out);
        assert!(out.contains("pubfnbuild_iadd"), "missing builder: {}", out);
        assert!(
            out.contains("pubfnregister_atoms"),
            "missing register: {}",
            out
        );
    }

    #[test]
    fn test_constructor_signature_attrs_first() {
        let out = codegen_str(
            r#"language T; atom icmp { attrs { cond: IntCC } inputs { lhs: i32, rhs: i32 } outputs { result: i1 } maps_to arith.icmp; }"#,
        );
        // attrs come before inputs in the parameter list
        assert!(
            out.contains("pubfnbuild_icmp(graph:&mutIrGraph,cond:"),
            "signature mismatch:\n{}",
            out
        );
        assert!(
            out.contains("::forge_hir::ir::IntCC"),
            "missing IntCC param type:\n{}",
            out
        );
        assert!(
            out.contains("_intcc_str(cond)"),
            "missing intcc conversion:\n{}",
            out
        );
    }

    #[test]
    fn test_value_output_uses_type_attr() {
        let out = codegen_str(
            r#"language T; atom load { attrs { ty: Type } inputs { addr: ptr } outputs { result: value } maps_to mem.load; }"#,
        );
        assert!(
            out.contains("&[ty]"),
            "value output should use the ty attr as result type:\n{}",
            out
        );
        assert!(
            out.contains("AttrValue::Type(ty)"),
            "ty attr should be stored as a Type attr:\n{}",
            out
        );
    }

    #[test]
    fn test_void_builder_returns_unit() {
        let out = codegen_str(
            r#"language T; atom store { inputs { value: value, addr: ptr } maps_to mem.store; }"#,
        );
        assert!(
            out.contains("Result<(),HirError>"),
            "store should return ():\n{}",
            out
        );
        assert!(
            out.contains("Ok(())"),
            "store body should return Ok(()):\n{}",
            out
        );
    }

    #[test]
    fn test_block_input_is_attr_not_operand() {
        let out =
            codegen_str(r#"language T; atom jump { inputs { target: Block } maps_to cf.jump; }"#);
        // target must be stored as a block attr and NOT appear in the operand list
        assert!(
            out.contains("AttrValue::BlockId(target)"),
            "block attr storage:\n{}",
            out
        );
        assert!(
            !out.contains("&[target]"),
            "Block input must not be an SSA operand:\n{}",
            out
        );
    }

    #[test]
    fn test_registration_includes_attr_types() {
        let out = codegen_str(
            r#"language T; atom iconst { attrs { value: i32 } outputs { result: i32 } maps_to arith.iconst; }"#,
        );
        assert!(
            out.contains(".attr_int(\"value\")"),
            "int attr registration:\n{}",
            out
        );
        assert!(out.contains("Opcode::Iconst"), "backend opcode:\n{}", out);
    }
}
