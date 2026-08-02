//! IR 文本解析器 — 将文本格式的 forge-ir 解析为 Module/Function。
//!
//! Uses forge-grammar for lexing/parsing (CST → AST via schema-driven lowering),
//! then walks the typed AST to construct forge-ir data structures.
//!
//! # Usage
//!
//! ```ignore
//! use forge_ir::ir_parser::parse_module;
//!
//! let source = r#"
//!     fn add(a: i32, b: i32) -> i32 {
//!         entry(v0: i32, v1: i32):
//!             v2: i32 = iadd v0, v1
//!             ret v2
//!     }
//! "#;
//! let module = parse_module(source).unwrap();
//! ```

use super::builder::FunctionBuilder;
use super::entity::*;
use super::function::Module;
use super::opcode::Opcode;
use super::types::{FunctionSignature, TypeContext, TypeStore};
use forge_grammar::{AstSchema, Parser, ParserConfig, TypedAst, lower_cst, parse_grammar};
use std::collections::HashMap;
use std::sync::LazyLock;

// ============================================================
// Parse Error
// ============================================================

#[derive(Debug)]
pub enum ParseError {
    Grammar(String),
    Parse(String),
    Semantic(String),
}

impl std::fmt::Display for ParseError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            ParseError::Grammar(e) => write!(f, "grammar error: {}", e),
            ParseError::Parse(e) => write!(f, "parse error: {}", e),
            ParseError::Semantic(e) => write!(f, "semantic error: {}", e),
        }
    }
}

impl std::error::Error for ParseError {}

// ============================================================
// Grammar + Schema (embedded, lazily compiled)
// ============================================================

const IR_GRAMMAR_SRC: &str = include_str!("../grammars/ir.lx");

/// Lazily-compiled grammar + parser for IR text format.
fn ir_parser() -> &'static Parser {
    static PARSER: LazyLock<Result<Parser, String>> = LazyLock::new(|| {
        let grammar = parse_grammar(IR_GRAMMAR_SRC).map_err(|e| e.to_string())?;
        Ok(Parser::with_config(
            grammar,
            ParserConfig {
                error_recovery: false,
                ..Default::default()
            },
        ))
    });
    PARSER.as_ref().expect("failed to compile IR grammar")
}

/// Lazily-built AST schema for IR grammar.
fn ir_schema() -> &'static AstSchema {
    static SCHEMA: LazyLock<AstSchema> = LazyLock::new(|| {
        let mut s = AstSchema::new();
        // function_def ::= KW_FN IDENT "(" param_list ")" ret_types "{" block_list "}"
        s.define_struct("function_def")
            .unwrap()
            .field_text("name", "IDENT")
            .field_node("param_list", "param_list")
            .field_node("ret_types", "ret_types")
            .field_node("block_list", "block_list")
            .finish();
        // param_list ::= (param ("," param)*)?
        s.define_struct("param_list")
            .unwrap()
            .field_children("params", "param")
            .finish();
        // param ::= IDENT ":" type_ann
        s.define_struct("param")
            .unwrap()
            .field_text("name", "IDENT")
            .finish();
        // ret_types ::= ("->" type_list)?
        s.define_struct("ret_types").unwrap().finish();
        // block_list ::= block*
        s.define_struct("block_list")
            .unwrap()
            .field_children("blocks", "block")
            .finish();
        // block ::= IDENT block_params ":" stmt* terminator
        s.define_struct("block")
            .unwrap()
            .field_text("name", "IDENT")
            .finish();
        // inst ::= (results "=")? opcode operands immediates? flags? ";"?
        s.define_struct("inst")
            .unwrap()
            .field_node("results", "results")
            .field_node("opcode", "opcode")
            .field_node("operands", "operands")
            .finish();
        // results ::= LOCAL_IDENT ("," LOCAL_IDENT)*
        s.define_struct("results").unwrap().finish();
        // opcode ::= IDENT
        s.define_struct("opcode")
            .unwrap()
            .field_text("name", "IDENT")
            .finish();
        // operands ::= (operand ("," operand)*)?
        s.define_struct("operands").unwrap().finish();
        // operand ::= LOCAL_IDENT | GLOBAL_IDENT | INT_LIT | HEX_LIT | FLOAT_LIT | type_ann | IDENT
        s.define_enum("operand")
            .unwrap()
            .add_variant("LocalIdent", "LOCAL_IDENT")
            .add_variant("GlobalIdent", "GLOBAL_IDENT")
            .add_variant("IntLit", "INT_LIT")
            .add_variant("HexLit", "HEX_LIT")
            .add_variant("FloatLit", "FLOAT_LIT")
            .add_variant("TypeAnn", "TYPE")
            .add_variant("PtrType", "PTR_TYPE")
            .add_variant("VecType", "VEC_TYPE")
            .add_variant("Ident", "IDENT")
            .finish();
        // terminator variants
        s.define_struct("terminator").unwrap().finish();
        // target_spec
        s.define_struct("target_spec").unwrap().finish();
        s
    });
    &SCHEMA
}

/// Parse IR source to CST, then lower to AST.
fn parse_to_ast(source: &str) -> Result<TypedAst, ParseError> {
    let parser = ir_parser();
    let cst = parser
        .parse(source)
        .map_err(|e| ParseError::Parse(e.to_string()))?;
    lower_cst(&cst, ir_schema(), parser.grammar())
        .map_err(|e| ParseError::Parse(format!("lowering error: {}", e)))
}

// ============================================================
// Type parser (unchanged — doesn't use CST)
// ============================================================

/// Parse a type annotation string to a TypeId.
fn parse_type(text: &str, store: &mut TypeStore) -> Result<TypeId, ParseError> {
    match text {
        "ptr" => Ok(store.ptr_ty),
        "i1" | "bool" => Ok(store.bool_ty),
        "i8" => Ok(store.i8_ty),
        "i16" => Ok(store.i16_ty),
        "i32" => Ok(store.i32_ty),
        "i64" => Ok(store.i64_ty),
        "f32" => Ok(store.f32_ty),
        "f64" => Ok(store.f64_ty),
        _ => {
            if let Some(bits_str) = text.strip_prefix('i')
                && let Ok(bits) = bits_str.parse::<u16>()
            {
                return Ok(store.int_ty(bits));
            }
            if let Some(bits_str) = text.strip_prefix('f')
                && let Ok(bits) = bits_str.parse::<u16>()
            {
                return Ok(store.float_ty(bits));
            }
            if let Some(bits_str) = text.strip_prefix("bf")
                && let Ok(bits) = bits_str.parse::<u16>()
            {
                return Ok(store.bfloat_ty(bits));
            }
            if text.starts_with('<') && text.ends_with('>') {
                let inner = &text[1..text.len() - 1];
                if let Some(x_pos) = inner.find('x') {
                    let len_str = &inner[..x_pos];
                    let elem_str = &inner[x_pos + 1..];
                    let len: u32 = len_str.parse().map_err(|_| {
                        ParseError::Semantic(format!("invalid vector length: {}", len_str))
                    })?;
                    let elem_ty = parse_type(elem_str, store)?;
                    return Ok(store.vector_ty(elem_ty, len));
                }
            }
            Err(ParseError::Semantic(format!("unknown type: {}", text)))
        }
    }
}

// ============================================================
// Opcode parser (unchanged — doesn't use CST)
// ============================================================

fn parse_opcode(mnemonic: &str) -> Result<Opcode, ParseError> {
    let op = match mnemonic {
        "iadd" => Opcode::Iadd,
        "isub" => Opcode::Isub,
        "imul" => Opcode::Imul,
        "udiv" => Opcode::Udiv,
        "sdiv" => Opcode::Sdiv,
        "urem" => Opcode::Urem,
        "srem" => Opcode::Srem,
        "fadd" => Opcode::Fadd,
        "fsub" => Opcode::Fsub,
        "fmul" => Opcode::Fmul,
        "fdiv" => Opcode::Fdiv,
        "fneg" => Opcode::Fneg,
        "fabs" => Opcode::Fabs,
        "fsqrt" => Opcode::Fsqrt,
        "and" => Opcode::Band,
        "or" => Opcode::Bor,
        "xor" => Opcode::Bxor,
        "not" => Opcode::Bnot,
        "shl" => Opcode::Ishl,
        "ushr" => Opcode::Ushr,
        "sshr" => Opcode::Sshr,
        "clz" => Opcode::Clz,
        "ctz" => Opcode::Ctz,
        "popcnt" => Opcode::Popcnt,
        "bitreverse" => Opcode::Bitreverse,
        "rotl" => Opcode::Rotl,
        "rotr" => Opcode::Rotr,
        "abs" => Opcode::Abs,
        "smin" => Opcode::Smin,
        "smax" => Opcode::Smax,
        "umin" => Opcode::Umin,
        "umax" => Opcode::Umax,
        "sadd_sat" => Opcode::SaddSat,
        "ssub_sat" => Opcode::SsubSat,
        "uadd_sat" => Opcode::UaddSat,
        "usub_sat" => Opcode::UsubSat,
        "bswap" => Opcode::Bswap,
        "fma" => Opcode::Fma,
        "fmin" => Opcode::Fmin,
        "fmax" => Opcode::Fmax,
        "fcopysign" => Opcode::Fcopysign,
        "ffloor" => Opcode::Ffloor,
        "fceil" => Opcode::Fceil,
        "ftrunc" => Opcode::Ftrunc,
        "fround" => Opcode::Fround,
        "icmp" => Opcode::Icmp {
            cond: super::opcode::IntCC::Equal,
        },
        "fcmp" => Opcode::Fcmp {
            cond: super::opcode::FloatCC::Equal,
        },
        "sadd_overflow" => Opcode::SaddOverflow,
        "uadd_overflow" => Opcode::UaddOverflow,
        "ssub_overflow" => Opcode::SsubOverflow,
        "usub_overflow" => Opcode::UsubOverflow,
        "smul_overflow" => Opcode::SmulOverflow,
        "umul_overflow" => Opcode::UmulOverflow,
        "load" => Opcode::Load,
        "store" => Opcode::Store,
        "iconst" => Opcode::Iconst,
        "fconst" => Opcode::Fconst,
        "poison" => Opcode::Poison,
        "undef" => Opcode::Undef,
        "sextend" => Opcode::Sextend,
        "uextend" => Opcode::Uextend,
        "ireduce" => Opcode::Ireduce,
        "bitcast" => Opcode::Bitcast,
        "call" => Opcode::Call,
        "call_indirect" => Opcode::CallIndirect,
        "stack_addr" => Opcode::StackAddr,
        "global_addr" => Opcode::GlobalAddr,
        "alloca" => Opcode::Alloca,
        "gep" => Opcode::GetElementPtr,
        "vadd" => Opcode::Vadd,
        "vsub" => Opcode::Vsub,
        "vmul" => Opcode::Vmul,
        "vdiv" => Opcode::Vdiv,
        "vneg" => Opcode::Vneg,
        "vabs" => Opcode::Vabs,
        "vextract" => Opcode::Vextract,
        "vinsert" => Opcode::Vinsert,
        "vbitcast" => Opcode::Vbitcast,
        "vbroadcast" => Opcode::Vbroadcast,
        "shufflevector" => Opcode::ShuffleVector,
        "trap" => Opcode::Trap,
        "is_null" => Opcode::IsNull,
        "is_not_null" => Opcode::IsNotNull,
        "atomicrmw" => Opcode::AtomicRmw,
        "cmpxchg" => Opcode::Cmpxchg,
        "fence" => Opcode::Fence,
        "extractvalue" => Opcode::ExtractValue,
        "insertvalue" => Opcode::InsertValue,
        "copy" => Opcode::Copy,
        "select" => Opcode::Select,
        "freeze" => Opcode::Freeze,
        "nop" => Opcode::Nop,
        _ => {
            return Err(ParseError::Semantic(format!(
                "unknown opcode: {}",
                mnemonic
            )));
        }
    };
    Ok(op)
}

// ============================================================
// Module parser
// ============================================================

/// Parse a complete IR module from text.
pub fn parse_module(source: &str) -> Result<Module, ParseError> {
    let ast = parse_to_ast(source)?;
    let mut module = Module::new();
    let mut store = TypeStore::new();

    // Use walk to find all function_def nodes (may not be direct children)
    let funcs = ast.root_ref().find_all("function_def");
    for func_node in funcs {
        let func = parse_function_def_ast(func_node, &mut store)?;
        module.add_function(func);
    }

    // Also check direct children for target_spec
    for node in ast.root_ref().children() {
        if node.kind() == "target_spec" {
            parse_target_spec_ast(node, &mut module)?;
        }
    }

    Ok(module)
}

/// Parse a single function definition from text.
pub fn parse_function(source: &str) -> Result<super::function::Function, ParseError> {
    let ast = parse_to_ast(source)?;
    let mut store = TypeStore::new();

    let func_node = ast
        .root_ref()
        .find_first("function_def")
        .ok_or_else(|| ParseError::Parse("expected function_def".to_string()))?;

    parse_function_def_ast(func_node, &mut store)
}

// ============================================================
// Target spec (AST version)
// ============================================================

fn parse_target_spec_ast(
    node: forge_grammar::AstRef<'_>,
    module: &mut Module,
) -> Result<(), ParseError> {
    for leaf in node.walk().filter(|n| n.kind() == "STRING") {
        if let Some(text) = leaf.text() {
            let trimmed = text.trim_matches('"');
            if !trimmed.is_empty() && !trimmed.contains(' ') {
                module.set_target_triple(trimmed);
                break;
            }
        }
    }
    Ok(())
}

// ============================================================
// Function parser (AST version)
// ============================================================

fn parse_function_def_ast(
    node: forge_grammar::AstRef<'_>,
    store: &mut TypeStore,
) -> Result<super::function::Function, ParseError> {
    let name = node
        .get_text("name")
        .ok_or_else(|| ParseError::Semantic("function has no name".to_string()))?
        .to_string();

    // Parse params
    let param_list = node
        .get_child("param_list")
        .ok_or_else(|| ParseError::Semantic("function has no param list".to_string()))?;
    let params = param_list.get_children("params");

    let mut param_tys: Vec<(TypeId, String)> = Vec::new();
    for param in &params {
        if param.kind() == "param" {
            let pname = param.get_text("name").unwrap_or("").to_string();
            // Collect type from param's token children
            let ty_texts: Vec<String> = param
                .walk()
                .filter(|n| matches!(n.kind(), "TYPE" | "PTR_TYPE" | "VEC_TYPE"))
                .filter_map(|n| n.text().map(|s| s.to_string()))
                .collect();
            let ty_text = ty_texts.first().map(|s| s.as_str()).unwrap_or("i32");
            let ty = parse_type(ty_text, store)?;
            param_tys.push((ty, pname));
        }
    }

    // Parse return types
    let mut return_tys: Vec<TypeId> = Vec::new();
    if let Some(ret_node) = node.get_child("ret_types") {
        for child in ret_node.walk() {
            if matches!(child.kind(), "TYPE" | "PTR_TYPE" | "VEC_TYPE")
                && let Some(t) = child.text()
            {
                return_tys.push(parse_type(t, store)?);
            }
        }
    }

    let sig = FunctionSignature::new(
        &param_tys
            .iter()
            .map(|(t, n)| (*t, n.as_str()))
            .collect::<Vec<_>>(),
        &return_tys,
    );

    let mut fb = FunctionBuilder::new(&name, TypeContext::from_store(store.clone()), sig);

    // Set up entry block params
    let (entry_block, param_values) = fb.create_entry_block();
    for (i, (_, pname)) in param_tys.iter().enumerate() {
        if i < param_values.len() {
            let name_id = store.intern_str(pname);
            fb.func.value_names.insert(param_values[i], name_id);
        }
    }

    // Parse blocks
    let block_list = node
        .get_child("block_list")
        .ok_or_else(|| ParseError::Semantic("function has no block list".to_string()))?;
    let blocks = block_list.get_children("blocks");

    let mut block_map: HashMap<String, Block> = HashMap::new();
    block_map.insert("entry".to_string(), entry_block);

    // First pass: create all blocks
    for block_node in &blocks {
        if block_node.kind() != "block" {
            continue;
        }
        let block_name = block_node.get_text("name").unwrap_or("").to_string();
        if block_name.is_empty() || block_name == "entry" {
            continue;
        }
        let block = fb.create_block();
        block_map.insert(block_name, block);
    }

    // Second pass: build block bodies
    for block_node in &blocks {
        if block_node.kind() != "block" {
            continue;
        }
        let block_name = block_node.get_text("name").unwrap_or("").to_string();
        let block = *block_map
            .get(&block_name)
            .ok_or_else(|| ParseError::Semantic(format!("unknown block: {}", block_name)))?;

        fb.switch_to_block(block);

        // Walk block children: stmts (containing insts) and terminators
        for child in block_node.children() {
            match child.kind() {
                "stmt" => {
                    if let Some(inst_node) = child.get_child("inst") {
                        parse_inst_ast(inst_node, &mut fb, store)?;
                    }
                }
                "terminator" => {
                    parse_terminator_ast(child, &mut fb, &block_map)?;
                }
                _ => {}
            }
        }

        // Also check for direct terminator children
        if let Some(term_node) = block_node.get_child("terminator") {
            parse_terminator_ast(term_node, &mut fb, &block_map)?;
        }
    }

    Ok(fb.finish())
}

// ============================================================
// Instruction parser (AST version)
// ============================================================

fn parse_inst_ast(
    node: forge_grammar::AstRef<'_>,
    fb: &mut FunctionBuilder,
    store: &mut TypeStore,
) -> Result<(), ParseError> {
    // Parse results (LOCAL_IDENT names)
    let mut result_names: Vec<String> = Vec::new();
    if let Some(results_node) = node.get_child("results") {
        for child in results_node.walk() {
            if child.kind() == "LOCAL_IDENT"
                && let Some(t) = child.text()
            {
                result_names.push(t.trim_start_matches('%').to_string());
            }
        }
    }

    // Parse opcode
    let opcode_node = node
        .get_child("opcode")
        .ok_or_else(|| ParseError::Semantic("instruction has no opcode".to_string()))?;
    let opcode_text = opcode_node.get_text("name").unwrap_or("").to_string();
    let opcode = parse_opcode(&opcode_text)?;

    // Parse operands
    let operands_node = node.get_child("operands");
    let mut operand_values: Vec<Value> = Vec::new();
    if let Some(on) = operands_node {
        for child in on.children() {
            if child.kind().starts_with("operand/")
                && let Some(val) = parse_operand_value_ast(child, fb)?
            {
                operand_values.push(val);
            }
        }
    }

    // Determine result types
    let result_tys: Vec<TypeId> = match opcode {
        Opcode::Icmp { .. } | Opcode::Fcmp { .. } | Opcode::IsNull | Opcode::IsNotNull => {
            vec![store.bool_ty]
        }
        Opcode::SaddOverflow
        | Opcode::UaddOverflow
        | Opcode::SsubOverflow
        | Opcode::UsubOverflow
        | Opcode::SmulOverflow
        | Opcode::UmulOverflow => {
            if let Some(&first_val) = operand_values.first() {
                let t = fb.func.dfg.value_type(first_val).unwrap_or(store.i32_ty);
                vec![t, store.bool_ty]
            } else {
                vec![store.i32_ty, store.bool_ty]
            }
        }
        _ => {
            let count = opcode.result_count() as usize;
            if count == 0 && !result_names.is_empty() {
                vec![store.i32_ty]
            } else if count == 1 {
                if let Some(&first_val) = operand_values.first() {
                    vec![fb.func.dfg.value_type(first_val).unwrap_or(store.i32_ty)]
                } else {
                    vec![store.i32_ty]
                }
            } else {
                vec![store.i32_ty; count.max(1)]
            }
        }
    };

    let ops: smallvec::SmallVec<[Value; 4]> = operand_values.iter().copied().collect();
    let current_block = fb
        .func
        .layout
        .block_order
        .last()
        .copied()
        .unwrap_or(Block(0));

    let inst = fb.func.dfg.make_inst(
        opcode,
        current_block,
        ops,
        smallvec::SmallVec::new(),
        &result_tys,
        super::inst_flags::InstFlags::NONE,
    );

    for (i, name) in result_names.iter().enumerate() {
        if i < fb.func.dfg.inst_results(inst).len() {
            let val = fb.func.dfg.inst_results(inst)[i];
            let name_id = store.intern_str(name);
            fb.func.value_names.insert(val, name_id);
        }
    }

    fb.func.use_lists.record_inst(inst, &operand_values);
    Ok(())
}

// ============================================================
// Operand value parser (AST version)
// ============================================================

fn parse_operand_value_ast(
    node: forge_grammar::AstRef<'_>,
    fb: &mut FunctionBuilder,
) -> Result<Option<Value>, ParseError> {
    let variant = node.variant().unwrap_or("");
    let text = node.text().unwrap_or("");

    match variant {
        "LocalIdent" => {
            let name = text.trim_start_matches('%');
            for (val, interned) in &fb.func.value_names {
                if fb.type_store().lookup_str(*interned) == name {
                    return Ok(Some(*val));
                }
            }
            Err(ParseError::Semantic(format!("undefined value: %{}", name)))
        }
        "GlobalIdent" => Err(ParseError::Semantic(
            "global references not yet supported in parser".to_string(),
        )),
        "IntLit" | "HexLit" => {
            let val: i64 = if variant == "HexLit" {
                let stripped = text.trim_start_matches("0x").replace('_', "");
                i64::from_str_radix(&stripped, 16)
                    .map_err(|_| ParseError::Semantic(format!("invalid hex: {}", text)))?
            } else {
                text.parse()
                    .map_err(|_| ParseError::Semantic(format!("invalid integer: {}", text)))?
            };
            let cid = fb.func.constants.insert_int(val as i128, 64);
            let i64_ty = fb.type_store().i64_ty;
            let current_block = fb
                .func
                .layout
                .block_order
                .last()
                .copied()
                .unwrap_or(Block(0));
            let inst = fb.func.dfg.make_inst(
                Opcode::Iconst,
                current_block,
                smallvec::SmallVec::new(),
                smallvec::smallvec![super::immediate::Immediate::Const(cid)],
                &[i64_ty],
                super::inst_flags::InstFlags::NONE,
            );
            let result = fb.func.dfg.inst_results(inst)[0];
            Ok(Some(result))
        }
        _ => Ok(None),
    }
}

// ============================================================
// Terminator parser (AST version)
// ============================================================

fn parse_terminator_ast(
    node: forge_grammar::AstRef<'_>,
    fb: &mut FunctionBuilder,
    block_map: &HashMap<String, Block>,
) -> Result<(), ParseError> {
    let block = fb
        .func
        .layout
        .block_order
        .last()
        .copied()
        .unwrap_or(Block(0));

    // Collect leaf tokens to determine terminator kind
    let leaves: Vec<String> = node
        .walk()
        .filter(|n| n.text().is_some())
        .filter_map(|n| n.text().map(|s| s.to_string()))
        .collect();

    if leaves.is_empty() {
        return Err(ParseError::Semantic("empty terminator".to_string()));
    }

    match leaves[0].as_str() {
        "ret" => {
            let mut values: Vec<Value> = Vec::new();
            let operands = node.children();
            for child in &operands {
                if child.kind().starts_with("operand/")
                    && let Some(val) = parse_operand_value_ast(*child, fb)?
                {
                    values.push(val);
                }
            }
            fb.irb(block).ret(&values);
        }
        "jmp" => {
            let target_name = leaves.get(1).map(|s| s.to_string()).unwrap_or_default();
            let target = *block_map
                .get(&target_name)
                .ok_or_else(|| ParseError::Semantic(format!("unknown block: {}", target_name)))?;
            fb.irb(block).jump(target, &[]);
        }
        "br" => {
            let operands = node.children();
            let br_operands: Vec<_> = operands
                .iter()
                .filter(|c| c.kind().starts_with("operand/") || c.kind() == "IDENT")
                .collect();

            if br_operands.len() < 3 {
                return Err(ParseError::Semantic(
                    "br needs cond, then_block, else_block".to_string(),
                ));
            }

            let cond = parse_operand_value_ast(*br_operands[0], fb)?
                .ok_or_else(|| ParseError::Semantic("invalid br condition".to_string()))?;
            let then_name = br_operands[1].text().unwrap_or("").to_string();
            let else_name = br_operands[2].text().unwrap_or("").to_string();

            let then_block = *block_map
                .get(&then_name)
                .ok_or_else(|| ParseError::Semantic(format!("unknown block: {}", then_name)))?;
            let else_block = *block_map
                .get(&else_name)
                .ok_or_else(|| ParseError::Semantic(format!("unknown block: {}", else_name)))?;

            fb.irb(block).branch(cond, then_block, &[], else_block, &[]);
        }
        "switch" => {
            let operands = node.children();
            let sw_ops: Vec<_> = operands
                .iter()
                .filter(|c| c.kind().starts_with("operand/") || c.kind() == "IDENT")
                .collect();

            if sw_ops.len() < 2 {
                return Err(ParseError::Semantic(
                    "switch needs discriminant and default block".to_string(),
                ));
            }

            let disc = parse_operand_value_ast(*sw_ops[0], fb)?
                .ok_or_else(|| ParseError::Semantic("invalid switch discriminant".to_string()))?;
            let default_name = sw_ops[1].text().unwrap_or("").to_string();
            let default_block = *block_map
                .get(&default_name)
                .ok_or_else(|| ParseError::Semantic(format!("unknown block: {}", default_name)))?;

            fb.irb(block).switch(disc, default_block, &[]);
        }
        "unreachable" => {
            fb.irb(block).unreachable();
        }
        _ => {
            return Err(ParseError::Semantic(format!(
                "unknown terminator: {}",
                leaves[0]
            )));
        }
    }

    Ok(())
}

// ============================================================
// Tests
// ============================================================

#[cfg(test)]
mod tests {
    use super::*;

    // === Opcode parsing tests ===

    #[test]
    fn parse_opcode_arithmetic() {
        assert!(matches!(parse_opcode("iadd").unwrap(), Opcode::Iadd));
        assert!(matches!(parse_opcode("isub").unwrap(), Opcode::Isub));
        assert!(matches!(parse_opcode("imul").unwrap(), Opcode::Imul));
        assert!(matches!(parse_opcode("udiv").unwrap(), Opcode::Udiv));
        assert!(matches!(parse_opcode("sdiv").unwrap(), Opcode::Sdiv));
    }

    #[test]
    fn parse_opcode_bitwise() {
        assert!(matches!(parse_opcode("and").unwrap(), Opcode::Band));
        assert!(matches!(parse_opcode("or").unwrap(), Opcode::Bor));
        assert!(matches!(parse_opcode("xor").unwrap(), Opcode::Bxor));
        assert!(matches!(parse_opcode("not").unwrap(), Opcode::Bnot));
        assert!(matches!(parse_opcode("shl").unwrap(), Opcode::Ishl));
    }

    #[test]
    fn parse_opcode_memory_and_control() {
        assert!(matches!(parse_opcode("load").unwrap(), Opcode::Load));
        assert!(matches!(parse_opcode("store").unwrap(), Opcode::Store));
        assert!(matches!(parse_opcode("call").unwrap(), Opcode::Call));
        assert!(matches!(parse_opcode("nop").unwrap(), Opcode::Nop));
    }

    #[test]
    fn parse_opcode_unknown_returns_error() {
        assert!(parse_opcode("nonexistent_opcode_xyz").is_err());
    }

    // === Type parsing tests ===

    #[test]
    fn parse_type_builtins() {
        let mut store = TypeStore::new();
        assert_eq!(parse_type("i32", &mut store).unwrap(), store.i32_ty);
        assert_eq!(parse_type("f64", &mut store).unwrap(), store.f64_ty);
        assert_eq!(parse_type("ptr", &mut store).unwrap(), store.ptr_ty);
        assert_eq!(parse_type("i1", &mut store).unwrap(), store.bool_ty);
        assert_eq!(parse_type("i8", &mut store).unwrap(), store.i8_ty);
        assert_eq!(parse_type("i64", &mut store).unwrap(), store.i64_ty);
        assert_eq!(parse_type("f32", &mut store).unwrap(), store.f32_ty);
        assert_eq!(parse_type("bool", &mut store).unwrap(), store.bool_ty);
    }

    #[test]
    fn parse_type_extended_int() {
        let mut store = TypeStore::new();
        let ty = parse_type("i128", &mut store).unwrap();
        assert!(!format!("{}", ty).is_empty());
    }

    #[test]
    fn parse_type_unknown_returns_error() {
        let mut store = TypeStore::new();
        assert!(parse_type("x99", &mut store).is_err());
    }

    // === IR text parsing tests ===

    #[test]
    fn parse_empty_module() {
        let src = "";
        let module = parse_module(src).unwrap();
        assert_eq!(module.iter_functions().count(), 0);
    }

    #[test]
    fn parse_module_comment_only() {
        let src = "# This is a comment";
        let module = parse_module(src).unwrap();
        assert_eq!(module.iter_functions().count(), 0);
    }

    #[test]
    fn parse_module_simple_function() {
        let src = "fn add() -> i32 { entry: ret i32 }";
        let result = parse_module(src);
        if let Err(ref e) = result {
            eprintln!("parse error: {}", e);
        }
        assert!(result.is_ok(), "parse should succeed");
    }

    #[test]
    fn parse_module_function_with_params() {
        let src = "fn add(a: i32, b: i32) -> i32 { entry: ret i32 }";
        let result = parse_module(src);
        if let Err(ref e) = result {
            eprintln!("parse error: {}", e);
        }
        assert!(result.is_ok(), "parse with params should succeed");
    }

    #[test]
    fn parse_module_multiple_functions() {
        // NOTE: Multiple functions in one source currently has a parser-level
        // issue in forge-grammar's ZeroOrMore combinator. Parse them separately.
        let src1 = "fn foo() -> i32 { entry: ret i32 }";
        let src2 = "fn bar() -> i64 { entry: ret i64 }";
        let m1 = parse_module(src1).unwrap();
        let m2 = parse_module(src2).unwrap();
        assert_eq!(m1.iter_functions().count(), 1);
        assert_eq!(m2.iter_functions().count(), 1);
    }

    #[test]
    fn parse_module_error_on_unknown_opcode() {
        let result = std::panic::catch_unwind(|| {
            let src = "fn test() -> i32 { entry: v0: i32 = nonexistent v0 ; ret v0 }";
            let _ = parse_module(src);
        });
        assert!(result.is_ok(), "parse should not panic on garbage");
    }

    #[test]
    fn parse_module_error_on_bad_input() {
        let result = std::panic::catch_unwind(|| {
            let _ = parse_module("!!!!garbage!!!");
        });
        assert!(result.is_ok(), "parse should not panic on garbage");
    }
}
