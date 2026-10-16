//! DSL Parser for the `define_lowering!` macro.
//!
//! Uses `syn` for token-based parsing with Ident + string dispatch
//! for maximum robustness with custom keywords.
//!
//! The DSL now supports ONLY the op catalog: `atom` definitions.
//! `rule`/`struct` were removed — AST→IR lowering is written by hand
//! against `HirCtx` (see forge-hir docs).

use syn::{
    Ident, Token, braced,
    parse::{Parse, ParseStream},
};

/// The parsed lowering spec: a language name + its atomic op catalog.
#[derive(Debug)]
pub struct LoweringSpec {
    pub language: String,
    pub atoms: Vec<AtomDef>,
}

/// A single atomic operation definition.
#[derive(Debug)]
pub struct AtomDef {
    pub name: String,
    pub attrs: Vec<(String, String)>,
    pub inputs: Vec<(String, String)>,
    pub outputs: Vec<(String, String)>,
    pub regions: Vec<String>,
    pub maps_to: Option<String>,
}

/// Parse a keyword-like token and verify it matches `expected`.
/// Handles both Rust keywords (struct) and custom identifiers (atom, attrs...).
fn parse_kw(input: ParseStream<'_>, expected: &str) -> syn::Result<()> {
    let fork = input.fork();

    // Try as Rust keyword first, then as Ident
    let matched = match expected {
        "struct" => fork.parse::<Token![struct]>().is_ok(),
        _ => {
            if let Ok(ident) = fork.parse::<Ident>() {
                ident == expected
            } else {
                false
            }
        }
    };

    if matched {
        // Consume the same token from the real input
        match expected {
            "struct" => {
                input.parse::<Token![struct]>()?;
            }
            _ => {
                input.parse::<Ident>()?;
            }
        }
        Ok(())
    } else {
        Err(input.error(format!("expected '{}'", expected)))
    }
}

/// Peek at the next keyword/ident without consuming.
fn peek_kw(input: ParseStream<'_>) -> Option<String> {
    let fork = input.fork();
    // Try Rust keywords first
    if fork.parse::<Token![struct]>().is_ok() {
        return Some("struct".to_string());
    }
    // Try as Ident
    fork.parse::<Ident>().ok().map(|i: Ident| i.to_string())
}

impl Parse for LoweringSpec {
    fn parse(input: ParseStream<'_>) -> syn::Result<Self> {
        parse_kw(input, "language")?;
        let lang_name: Ident = input.parse()?;
        input.parse::<Token![;]>()?;

        let mut atoms = Vec::new();

        while !input.is_empty() {
            let kw = peek_kw(input).ok_or_else(|| input.error("expected 'atom'"))?;

            match kw.as_str() {
                "atom" => atoms.push(input.parse()?),
                // Explicitly reject the removed DSL features so users get a
                // clear migration hint instead of a cryptic parse error.
                "rule" | "struct" => {
                    return Err(input.error(format!(
                        "'{}' is no longer supported by define_lowering!: \
                         write AST lowering by hand against forge_hir::HirCtx \
                         instead (see the mini_c example)",
                        kw
                    )));
                }
                other => return Err(input.error(format!("unexpected keyword '{}'", other))),
            }
        }

        Ok(LoweringSpec {
            language: lang_name.to_string(),
            atoms,
        })
    }
}

impl Parse for AtomDef {
    fn parse(input: ParseStream<'_>) -> syn::Result<Self> {
        parse_kw(input, "atom")?;
        let name: Ident = input.parse()?;
        let content;
        braced!(content in input);

        let mut atom_attrs = Vec::new();
        let mut atom_inputs = Vec::new();
        let mut atom_outputs = Vec::new();
        let mut atom_regions = Vec::new();
        let mut atom_maps_to = None;

        while !content.is_empty() {
            let kw = peek_kw(&content).ok_or_else(|| {
                content.error("expected attrs, inputs, outputs, regions, or maps_to")
            })?;

            match kw.as_str() {
                "attrs" => {
                    content.parse::<Ident>()?;
                    atom_attrs = parse_field_list(&content)?;
                }
                "inputs" => {
                    content.parse::<Ident>()?;
                    atom_inputs = parse_field_list(&content)?;
                }
                "outputs" => {
                    content.parse::<Ident>()?;
                    atom_outputs = parse_field_list(&content)?;
                }
                "regions" => {
                    content.parse::<Ident>()?;
                    atom_regions = parse_ident_list(&content)?;
                }
                "maps_to" => {
                    content.parse::<Ident>()?;
                    let dialect: Ident = content.parse()?;
                    content.parse::<Token![.]>()?;
                    let op: Ident = content.parse()?;
                    atom_maps_to = Some(format!("{}.{}", dialect, op));
                    content.parse::<Token![;]>()?;
                }
                _ => return Err(content.error(format!("unexpected '{}' in atom body", kw))),
            }
        }

        Ok(AtomDef {
            name: name.to_string(),
            attrs: atom_attrs,
            inputs: atom_inputs,
            outputs: atom_outputs,
            regions: atom_regions,
            maps_to: atom_maps_to,
        })
    }
}

// Helpers

fn parse_field_list(input: ParseStream<'_>) -> syn::Result<Vec<(String, String)>> {
    let content;
    braced!(content in input);
    let mut fields = Vec::new();
    while !content.is_empty() {
        let name: Ident = content.parse()?;
        content.parse::<Token![:]>()?;
        let ty: Ident = content.parse()?;
        fields.push((name.to_string(), ty.to_string()));
        if content.peek(Token![,]) {
            content.parse::<Token![,]>()?;
        }
    }
    Ok(fields)
}

fn parse_ident_list(input: ParseStream<'_>) -> syn::Result<Vec<String>> {
    let content;
    braced!(content in input);
    let mut names = Vec::new();
    while !content.is_empty() {
        let name: Ident = content.parse()?;
        names.push(name.to_string());
        if content.peek(Token![,]) {
            content.parse::<Token![,]>()?;
        }
    }
    Ok(names)
}

pub fn parse_lowering_spec(input: &str) -> Result<LoweringSpec, String> {
    let tokens: proc_macro2::TokenStream = input
        .parse()
        .map_err(|e| format!("token parse error: {}", e))?;
    syn::parse2::<LoweringSpec>(tokens).map_err(|e| e.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_language() {
        let s = parse_lowering_spec("language Test;").unwrap();
        assert_eq!(s.language, "Test");
    }

    #[test]
    fn test_atom() {
        let s = parse_lowering_spec(
            r#"language T; atom iadd { inputs { lhs: i32, rhs: i32 } outputs { result: i32 } maps_to arith.iadd; }"#
        ).unwrap();
        assert_eq!(s.atoms.len(), 1);
        assert_eq!(s.atoms[0].inputs.len(), 2);
        assert_eq!(s.atoms[0].maps_to.as_deref(), Some("arith.iadd"));
    }

    #[test]
    fn test_atom_regions() {
        let s = parse_lowering_spec(
            r#"language T; atom branch { inputs { cond: i1 } regions { then_body, else_body } maps_to cf.branch; }"#
        ).unwrap();
        assert_eq!(s.atoms[0].regions.len(), 2);
    }

    #[test]
    fn test_atom_attrs() {
        let s = parse_lowering_spec(
            r#"language T; atom icmp { attrs { cond: IntCC } inputs { lhs: i32, rhs: i32 } outputs { result: i1 } maps_to arith.icmp; }"#
        ).unwrap();
        assert_eq!(s.atoms[0].attrs.len(), 1);
        assert_eq!(
            s.atoms[0].attrs[0],
            ("cond".to_string(), "IntCC".to_string())
        );
    }

    #[test]
    fn test_multiple_atoms() {
        let s = parse_lowering_spec(
            r#"language T; atom a { outputs { r: i32 } maps_to arith.iconst; } atom b { inputs { x: i32 } outputs { r: i32 } maps_to arith.iadd; } atom c { inputs { v: i32 } maps_to cf.ret; }"#
        ).unwrap();
        assert_eq!(s.atoms.len(), 3);
    }

    #[test]
    fn test_rule_rejected() {
        let err = parse_lowering_spec(
            r#"language T; rule return_stmt { ret(value: lower_expr { child: "value" }); }"#,
        )
        .unwrap_err();
        assert!(
            err.contains("'rule' is no longer supported"),
            "err: {}",
            err
        );
    }

    #[test]
    fn test_struct_rejected() {
        let err = parse_lowering_spec(r#"language T; struct IfElse { inputs { cond: i32 } }"#)
            .unwrap_err();
        assert!(
            err.contains("'struct' is no longer supported"),
            "err: {}",
            err
        );
    }

    #[test]
    fn test_empty_spec() {
        let s = parse_lowering_spec("language T;").unwrap();
        assert!(s.atoms.is_empty());
    }
}
