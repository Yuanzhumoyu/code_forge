//! AST Schema — defines the shape of every AST node kind for CST→AST lowering.
//!
//! Uses `AstSchema` builder pattern to define Struct nodes (fixed fields).
//!
//! **Key design notes:**
//! - The CST parser does NOT create wrapper nodes for single-rule chains.
//!   Since `expr ::= assignment`, the `expr` node never appears in the CST.
//!   All expression references in the schema must use `"assignment"` instead of `"expr"`.
//! - `block_item` (stmt dispatch) has NO schema — it is transparent (single child → inline).
//! - Expression precedence levels (`logical_or`, `additive`, …) are also transparent.

use code_forge::forge_grammar::AstSchema;

/// Build the AST schema for Mini C.
pub fn build_schema() -> AstSchema {
    let mut schema = AstSchema::new();

    // ── Function definition ──
    schema
        .define_struct("func_def")
        .expect("define func_def")
        .field_text("return_type", "TYPE_INT")
        .field_text("name", "IDENT")
        .field_children("params", "param")
        .field_node("body", "block")
        .finish();

    // ── Parameters ──
    schema
        .define_struct("param")
        .expect("define param")
        .field_text("type_name", "TYPE_INT")
        .field_text("name", "IDENT")
        .finish();

    // ── Block ──
    schema
        .define_struct("block")
        .expect("define block")
        .field_children("items", "block_item")
        .finish();

    // ── Statements ──

    schema
        .define_struct("return_stmt")
        .expect("define return_stmt")
        .field_optional("value", "assignment")
        .finish();

    schema
        .define_struct("var_decl")
        .expect("define var_decl")
        .field_text("type_name", "TYPE_INT")
        .field_text("name", "IDENT")
        .field_optional("init", "assignment")
        .finish();

    schema
        .define_struct("assign_stmt")
        .expect("define assign_stmt")
        .field_text("name", "IDENT")
        .field_optional("target_expr", "member_access")
        .field_node("value", "assignment")
        .finish();

    schema
        .define_struct("expr_stmt")
        .expect("define expr_stmt")
        .field_node("expr", "assignment")
        .finish();

    // ── Control flow ──

    schema
        .define_struct("if_stmt")
        .expect("define if_stmt")
        .field_node("condition", "assignment")
        .field_node("then_body", "block")
        .field_optional("else_body", "else_clause")
        .finish();

    schema
        .define_struct("while_stmt")
        .expect("define while_stmt")
        .field_node("condition", "assignment")
        .field_node("body", "block")
        .finish();

    schema
        .define_struct("for_stmt")
        .expect("define for_stmt")
        .field_optional("init", "for_init")
        .field_optional("condition", "assignment")
        .field_optional("update", "for_update")
        .field_node("body", "block")
        .finish();

    schema
        .define_struct("for_init")
        .expect("define for_init")
        .field_text("type_name", "TYPE_INT")
        .field_text("name", "IDENT")
        .field_node("value", "assignment")
        .finish();

    schema
        .define_struct("for_update")
        .expect("define for_update")
        .field_text("name", "IDENT")
        .field_node("value", "assignment")
        .finish();

    // ── Break / continue ──

    schema
        .define_struct("break_stmt")
        .expect("define break_stmt")
        .finish();

    schema
        .define_struct("continue_stmt")
        .expect("define continue_stmt")
        .finish();

    // ── Do-while loop ──

    schema
        .define_struct("do_while_stmt")
        .expect("define do_while_stmt")
        .field_node("body", "block")
        .field_node("condition", "assignment")
        .finish();

    // ── Function call ──

    schema
        .define_struct("call")
        .expect("define call")
        .field_text("name", "IDENT")
        .field_optional("args", "arg_list")
        .finish();

    schema
        .define_struct("arg_list")
        .expect("define arg_list")
        .field_children("items", "assignment")
        .finish();

    // ── Enum ──

    schema
        .define_struct("enum_def")
        .expect("define enum_def")
        .field_text("name", "IDENT")
        .field_children("items", "enumerator")
        .finish();

    schema
        .define_struct("enumerator")
        .expect("define enumerator")
        .field_text("name", "IDENT")
        .field_optional("value", "assignment")
        .finish();

    // ── Struct ──

    schema
        .define_struct("struct_def")
        .expect("define struct_def")
        .field_text("name", "IDENT")
        .field_children("fields", "struct_field")
        .finish();

    schema
        .define_struct("struct_field")
        .expect("define struct_field")
        .field_text("type_name", "TYPE_INT")
        .field_text("name", "IDENT")
        .finish();

    schema
        .define_struct("struct_decl")
        .expect("define struct_decl")
        .field_children("idents", "IDENT")
        .finish();

    schema
        .define_struct("struct_init")
        .expect("define struct_init")
        .field_children("idents", "IDENT")
        .field_children("values", "assignment")
        .finish();

    // ── Member access ──

    schema
        .define_struct("member_access")
        .expect("define member_access")
        .field_children("idents", "IDENT")
        .finish();

    schema
}
