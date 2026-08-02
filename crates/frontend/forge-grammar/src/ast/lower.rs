//! CST → AST Lowering 引擎。
//!
//! 从 CST + AstSchema + Grammar 自动构建 TypedAst。
//!
//! # 核心流程
//!
//! 1. 从根 CST 节点开始
//! 2. 查找 schema 中对应的 NodeDef
//! 3. 根据 NodeDef（struct/enum/透明）提取字段并递归 lower 子节点
//! 4. Fixup parent 链接
//!
//! # 使用
//!
//! ```ignore
//! let ast = lower_cst(&cst, &schema, &grammar)?;
//! let root = ast.root_ref();
//! ```

use crate::ast::AstArena;
use crate::ast::AstId;
use crate::ast::AstNodeData;
use crate::ast::FieldValue;
use crate::ast::TypedAst;
use crate::ast::schema::{AstSchema, FieldType, NodeKind};
use crate::cst::CstNode;
use crate::error::LowerError;
use crate::grammar::Grammar;
use crate::visit::CstPattern;
use std::collections::HashMap;

// ============================================================
// Public API
// ============================================================

/// 从 CST + schema + grammar 自动构建 TypedAst。
///
/// 这是 lowering 管线的主要入口。
pub fn lower_cst(
    cst: &CstNode,
    schema: &AstSchema,
    grammar: &Grammar,
) -> Result<TypedAst, LowerError> {
    let mut arena = AstArena::new();
    let ctx = LowerCtx::new(schema, grammar);
    let root = ctx.lower_node(cst, &mut arena)?;

    // Fixup all parent links
    arena.fixup_parents(root);

    Ok(TypedAst::with_root(schema.clone(), arena, root))
}

/// Lower CST，使用 schema 但不验证 grammar 一致性。
/// 当你信任 schema 已通过 `schema.validate()` 时使用。
pub fn lower_cst_unchecked(cst: &CstNode, schema: &AstSchema) -> Result<TypedAst, LowerError> {
    let mut arena = AstArena::new();
    let ctx = LowerCtx::new_unchecked(schema);
    let root = ctx.lower_node(cst, &mut arena)?;
    arena.fixup_parents(root);
    Ok(TypedAst::with_root(schema.clone(), arena, root))
}

// ============================================================
// LowerCtx
// ============================================================

/// Lowering 上下文 — 持有 schema 引用。
struct LowerCtx<'a> {
    schema: &'a AstSchema,
}

impl<'a> LowerCtx<'a> {
    fn new(schema: &'a AstSchema, _grammar: &'a Grammar) -> Self {
        Self { schema }
    }

    fn new_unchecked(schema: &'a AstSchema) -> Self {
        Self { schema }
    }

    // ── Main dispatch ──

    /// Lower 一个 CST 节点为 AstId。
    fn lower_node(&self, cst: &CstNode, arena: &mut AstArena) -> Result<AstId, LowerError> {
        // Step 1: Flatten transparent EBNF wrappers
        let flat = cst.flatten();

        // Step 2: Look up schema definition
        if let Some(node_def) = self.schema.lookup(flat.kind.as_str()) {
            match &node_def.node_kind {
                NodeKind::Struct => return self.lower_struct(flat, node_def, arena),
                NodeKind::Enum => return self.lower_enum(flat, node_def, arena),
            }
        }

        // Step 3: No schema definition → transparent pass-through
        self.lower_transparent(cst, arena)
    }

    // ── Struct lowering ──

    fn lower_struct(
        &self,
        cst: &CstNode,
        def: &crate::ast::schema::NodeDef,
        arena: &mut AstArena,
    ) -> Result<AstId, LowerError> {
        let mut fields: HashMap<String, FieldValue> = HashMap::new();

        for field_def in &def.fields {
            let value = self.lower_field(cst, field_def, arena)?;
            fields.insert(field_def.name.clone(), value);
        }

        let id = arena.alloc(AstNodeData::with_fields(
            def.rule_name.clone(),
            fields,
            cst.span.clone(),
        ));
        Ok(id)
    }

    /// Lower 单个字段。
    fn lower_field(
        &self,
        cst: &CstNode,
        field_def: &crate::ast::schema::FieldDef,
        arena: &mut AstArena,
    ) -> Result<FieldValue, LowerError> {
        match &field_def.field_type {
            FieldType::Text { child_kind } => {
                // 查找第一个匹配 kind 的子节点并提取文本
                let text = cst
                    .flatten_child(child_kind)
                    .map(|n| n.text().to_string())
                    .or_else(|| {
                        // Fallback: search deeper via CstPattern
                        let pattern = CstPattern::kind(child_kind.as_str());
                        cst.find_first_deep(&pattern).map(|n| n.text().to_string())
                    })
                    .unwrap_or_default();
                Ok(FieldValue::Text(text))
            }

            FieldType::Children { child_kind } => {
                let children: Vec<AstId> = cst
                    .flatten_children_by(child_kind)
                    .iter()
                    .map(|child_cst| self.lower_node(child_cst, arena))
                    .collect::<Result<Vec<_>, _>>()?;
                Ok(FieldValue::Children(children))
            }

            FieldType::OptionalChild { child_kind } => {
                let child = cst
                    .flatten_child(child_kind)
                    .map(|child_cst| self.lower_node(child_cst, arena))
                    .transpose()?;
                Ok(FieldValue::OptionalChild(child))
            }

            FieldType::FlattenedChildren { child_kind } => {
                // 类似 Children，但使用更激进的展平策略
                let mut children: Vec<AstId> = Vec::new();
                for child in &cst.children {
                    let flat = child.flatten();
                    if flat.kind == *child_kind {
                        children.push(self.lower_node(flat, arena)?);
                    } else if flat.kind == "rep" || flat.kind == "rep1" {
                        // Recurse into repetition wrappers
                        for sub_child in &flat.children {
                            let sub_flat = sub_child.flatten();
                            if sub_flat.kind == *child_kind {
                                children.push(self.lower_node(sub_flat, arena)?);
                            }
                        }
                    }
                }
                Ok(FieldValue::Children(children))
            }

            FieldType::ConcatenatedText { sep } => {
                let texts: Vec<&str> = cst.leaves().iter().map(|t| t.text.as_str()).collect();
                let joined = texts.join(sep);
                Ok(FieldValue::Text(joined))
            }

            FieldType::AllTokens => {
                let texts: Vec<String> = cst
                    .children
                    .iter()
                    .filter(|c| c.is_leaf())
                    .map(|c| c.text().to_string())
                    .collect();
                Ok(FieldValue::Text(texts.join(" ")))
            }

            FieldType::Node { rule_name } => {
                // Lower the first matching child
                let child_cst: Option<CstNode> =
                    cst.flatten_child(rule_name).cloned().or_else(|| {
                        let pattern = CstPattern::kind(rule_name.as_str());
                        cst.find_first_deep(&pattern).cloned()
                    });
                match child_cst {
                    Some(child) => {
                        let id = self.lower_node(&child, arena)?;
                        Ok(FieldValue::Child(id))
                    }
                    None => Err(LowerError::missing_field(field_def.name.clone(), rule_name)),
                }
            }

            FieldType::NodeList { rule_name } => {
                let children: Vec<AstId> = cst
                    .flatten_children_by(rule_name)
                    .iter()
                    .map(|child_cst| self.lower_node(child_cst, arena))
                    .collect::<Result<Vec<_>, _>>()?;
                Ok(FieldValue::Children(children))
            }

            FieldType::Custom => {
                // Custom lowering is handled via a different path.
                // In practice, users register custom lowering functions
                // that are invoked before the schema-based lowering.
                Err(LowerError::lowering(
                    "Custom field type requires explicit lowering handler",
                    cst.span.clone(),
                ))
            }
        }
    }

    // ── Enum lowering ──

    fn lower_enum(
        &self,
        cst: &CstNode,
        def: &crate::ast::schema::NodeDef,
        arena: &mut AstArena,
    ) -> Result<AstId, LowerError> {
        // Find the first non-transparent child to determine the variant
        let child = cst.children.iter().find(|c| {
            let k = c.flatten().kind.as_str();
            k != "seq" && k != "rep" && k != "rep1" && k != "opt" && k != "alt" && k != "epsilon"
        });

        let (variant, text) = if let Some(child_node) = child {
            let flat = child_node.flatten();
            let child_kind = flat.kind.as_str();

            // Try to match this child_kind against known variants
            if let Some(variant_def) = def.variants.iter().find(|v| v.token_kind == child_kind) {
                (variant_def.name.clone(), flat.text().to_string())
            } else {
                // Unknown variant: use the child_kind as the variant name
                (child_kind.to_string(), flat.text().to_string())
            }
        } else {
            ("epsilon".to_string(), String::new())
        };

        let mut fields = HashMap::new();
        fields.insert("variant".to_string(), FieldValue::Text(variant.clone()));
        fields.insert("text".to_string(), FieldValue::Text(text));

        let kind = format!("{}/{}", def.rule_name, variant);

        let id = arena.alloc(AstNodeData::with_fields(kind, fields, cst.span.clone()));
        Ok(id)
    }

    // ── Transparent pass-through ──

    /// 对无 schema 定义的节点：递归展平子节点，收集有意义的内容。
    fn lower_transparent(&self, cst: &CstNode, arena: &mut AstArena) -> Result<AstId, LowerError> {
        // If it's a leaf/token node, create a simple text node
        if cst.is_leaf() {
            let mut fields = HashMap::new();
            fields.insert("text".to_string(), FieldValue::Text(cst.text().to_string()));
            return Ok(arena.alloc(AstNodeData::with_fields(
                cst.kind.clone(),
                fields,
                cst.span.clone(),
            )));
        }

        // If it's an epsilon node, create an epsilon AST node
        if cst.kind == "epsilon" {
            return Ok(arena.alloc(AstNodeData::new("epsilon", cst.span.clone())));
        }

        // Otherwise: recursively lower children and collect them
        let lowered_children: Vec<AstId> = cst
            .children
            .iter()
            .map(|child| self.lower_node(child, arena))
            .collect::<Result<Vec<_>, _>>()?;

        if lowered_children.len() == 1 {
            // Single child → inline it (transparent pass-through)
            // But update its span to cover the parent
            Ok(lowered_children[0])
        } else {
            // Multiple children → wrap in a seq node
            let mut fields = HashMap::new();
            fields.insert(
                "children".to_string(),
                FieldValue::Children(lowered_children),
            );
            Ok(arena.alloc(AstNodeData::with_fields(
                cst.kind.clone(),
                fields,
                cst.span.clone(),
            )))
        }
    }
}

// ============================================================
// Tests
// ============================================================

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ast::schema::AstSchema;
    use crate::grammar::parse_grammar;
    use crate::parser::Parser;

    /// Helper: create grammar + schema + parse → lower
    fn parse_and_lower(
        grammar_src: &str,
        build_schema: fn(&mut AstSchema),
        source: &str,
    ) -> Result<TypedAst, LowerError> {
        let grammar = parse_grammar(grammar_src).unwrap();
        let mut schema = AstSchema::new();
        build_schema(&mut schema);
        let parser = Parser::build(grammar.clone());
        let cst = parser.parse(source).map_err(|e| {
            LowerError::lowering(format!("parse error: {}", e), crate::error::Span::dummy())
        })?;
        lower_cst(&cst, &schema, &grammar)
    }

    #[test]
    fn test_lower_simple_struct() {
        // Note: the existing CST parser does NOT wrap single-rule chains
        // in rule nodes. E.g., "program ::= stmt; stmt ::= IDENT" with input "hello"
        // produces just CstNode(IDENT). So we test with a grammar that has
        // structural wrapping via sequences.
        let grammar_src = r#"
            token IDENT = "[a-z]+"
            punct "(" ")"
            skip "[ \t]+"
            program ::= IDENT "(" ")"
        "#;

        let grammar = parse_grammar(grammar_src).unwrap();
        let mut schema = AstSchema::new();
        schema
            .define_struct("program")
            .unwrap()
            .field_text("name", "IDENT")
            .finish();

        let parser = Parser::build(grammar.clone());
        let cst = parser
            .parse("hello ( )")
            .map_err(|e| LowerError::lowering(format!("parse: {}", e), crate::error::Span::dummy()))
            .unwrap();

        let ast = lower_cst(&cst, &schema, &grammar).unwrap();
        let root = ast.root_ref();
        let all_kinds: Vec<String> = ast
            .root_ref()
            .walk()
            .map(|n| n.kind().to_string())
            .collect();
        eprintln!("AST kinds: {:?}, root kind: {}", all_kinds, root.kind());

        // Root should be "program"
        assert_eq!(
            root.kind(),
            "program",
            "Expected root to be program, got {}",
            root.kind()
        );
        assert_eq!(root.get_text("name"), Some("hello"));
    }

    #[test]
    fn test_lower_enum() {
        let grammar_src = r#"
            token IDENT = "[a-z]+"
            token REG = "[A-Z]+"
            token DEC = "[0-9]+"
            skip "[ \t]+"
            program ::= operand
            operand ::= IDENT | REG | DEC
        "#;

        let ast = parse_and_lower(
            grammar_src,
            |s| {
                s.define_enum("operand")
                    .unwrap()
                    .add_variant("Ident", "IDENT")
                    .add_variant("Reg", "REG")
                    .add_variant("Dec", "DEC")
                    .finish();
            },
            "RAX",
        )
        .unwrap();

        let _operands = ast.root_ref().find_all("operand");
        // Walk through transparent wrappers to find the operand
        let all_kinds: Vec<String> = ast
            .root_ref()
            .walk()
            .map(|n| n.kind().to_string())
            .collect();
        eprintln!("All kinds: {:?}", all_kinds);

        // The operand should show up
        let op_nodes: Vec<_> = ast
            .root_ref()
            .walk()
            .filter(|n| n.kind().starts_with("operand"))
            .collect();
        assert!(!op_nodes.is_empty(), "Expected operand node in AST");

        if let Some(op) = op_nodes.first() {
            let variant = op.get_text("variant");
            assert_eq!(variant, Some("Reg"));
            let text = op.get_text("text");
            assert_eq!(text, Some("RAX"));
        }
    }

    #[test]
    fn test_lower_inst_like() {
        let grammar_src = r#"
            token IDENT = "[a-z][a-z0-9_]*"
            token REG = "[A-Z][A-Z0-9]*"
            token DEC = "[0-9]+"
            punct "," ";"
            skip "[ \t]+"

            program ::= inst_like
            inst_like ::= IDENT (operand ("," operand)*)?
            operand ::= IDENT | REG | DEC
        "#;

        let ast = parse_and_lower(
            grammar_src,
            |s| {
                s.define_struct("inst_like")
                    .unwrap()
                    .field_text("mnemonic", "IDENT")
                    .field_children("operands", "operand")
                    .finish();
                s.define_enum("operand")
                    .unwrap()
                    .add_variant("Ident", "IDENT")
                    .add_variant("Reg", "REG")
                    .add_variant("Dec", "DEC")
                    .finish();
            },
            "mov rd, rs1",
        )
        .unwrap();

        // Find inst_like nodes
        let insts: Vec<_> = ast.root_ref().find_all("inst_like");
        assert!(!insts.is_empty(), "Expected inst_like node");

        if let Some(inst) = insts.first() {
            assert_eq!(inst.get_text("mnemonic"), Some("mov"));
            let operands = inst.get_children("operands");
            assert_eq!(operands.len(), 2, "Expected 2 operands");

            // Check operand values
            let op_texts: Vec<String> = operands
                .iter()
                .map(|op| op.get_text("text").unwrap_or("").to_string())
                .collect();
            assert!(op_texts.contains(&"rd".to_string()));
            assert!(op_texts.contains(&"rs1".to_string()));
        }
    }

    #[test]
    fn test_lower_transparent_no_schema() {
        let grammar_src = r#"
            token IDENT = "[a-z]+"
            skip "[ \t]+"
            program ::= stmt*
            stmt ::= IDENT
        "#;

        // Don't define any schema — should still lower via transparent pass-through
        let grammar = parse_grammar(grammar_src).unwrap();
        let schema = AstSchema::new();
        let parser = Parser::build(grammar.clone());
        let cst = parser.parse("hello world").unwrap();
        let ast = lower_cst(&cst, &schema, &grammar).unwrap();

        // Should have produced SOME AST (transparent)
        assert!(!ast.arena.is_empty(), "Expected non-empty arena");
        let root = ast.root_ref();
        let all_texts: Vec<String> = root
            .walk()
            .map(|n| n.get_text("text").unwrap_or("").to_string())
            .collect();
        // We should find the IDENT text somewhere
        let has_hello = all_texts.iter().any(|t| t == "hello");
        assert!(
            has_hello,
            "Expected to find 'hello' in AST texts: {:?}",
            all_texts
        );
    }

    #[test]
    fn test_lower_optional_field() {
        let grammar_src = r#"
            token KW_RET = "ret"
            token IDENT = "[a-z]+"
            skip "[ \t]+"
            program ::= return_stmt
            return_stmt ::= KW_RET IDENT?
        "#;

        let ast = parse_and_lower(
            grammar_src,
            |s| {
                s.define_struct("return_stmt")
                    .unwrap()
                    .field_text("keyword", "KW_RET")
                    .field_optional("value", "IDENT")
                    .finish();
            },
            "ret",
        )
        .unwrap();

        let stmts: Vec<_> = ast.root_ref().find_all("return_stmt");
        assert!(!stmts.is_empty());
        if let Some(stmt) = stmts.first() {
            assert_eq!(stmt.get_text("keyword"), Some("ret"));
            // Optional should be None since there's no IDENT
            let opt = stmt.get_optional("value");
            assert!(opt.is_some()); // field exists
            assert!(opt.unwrap().is_none()); // but is None
        }
    }

    #[test]
    fn test_lower_with_ir_grammar() {
        // Use the real ir.lx grammar
        let ir_src = include_str!("../../../../foundation/forge-ir/grammars/ir.lx");
        let grammar = parse_grammar(ir_src).unwrap();

        let mut schema = AstSchema::new();
        // function_def has param_list → block_list as children
        // These are rule nodes that contain the actual params/blocks
        schema
            .define_struct("function_def")
            .unwrap()
            .field_text("name", "IDENT")
            .field_node("param_list", "param_list")
            .field_node("block_list", "block_list")
            .finish();
        // param_list contains param nodes
        schema
            .define_struct("param_list")
            .unwrap()
            .field_children("params", "param")
            .finish();
        // block_list contains block nodes
        schema
            .define_struct("block_list")
            .unwrap()
            .field_children("blocks", "block")
            .finish();
        // A block starts with an IDENT (block name)
        schema
            .define_struct("block")
            .unwrap()
            .field_text("name", "IDENT")
            .finish();
        // A param has a name (IDENT) and a type
        schema
            .define_struct("param")
            .unwrap()
            .field_text("name", "IDENT")
            .field_text("type", "TYPE")
            .finish();

        let parser = Parser::build(grammar.clone());
        let source = r#"
            fn foo(x: i32, y: i64) -> i32 {
                entry:
                    ret i32
            }
        "#;
        let cst = parser.parse(source).unwrap();
        let ast = lower_cst(&cst, &schema, &grammar).unwrap();

        // Should have a function_def node
        let funcs = ast.root_ref().find_all("function_def");
        assert!(!funcs.is_empty(), "Expected function_def node");
        let func = &funcs[0];
        assert_eq!(func.get_text("name"), Some("foo"));

        // Get param_list child
        let param_lists: Vec<_> = ast
            .root_ref()
            .walk()
            .filter(|n| n.kind() == "param_list")
            .collect();
        assert!(!param_lists.is_empty(), "Expected param_list node");
        let params = param_lists[0].get_children("params");
        assert_eq!(params.len(), 2, "Expected 2 params, got {}", params.len());

        // Get blocks
        let block_lists: Vec<_> = ast
            .root_ref()
            .walk()
            .filter(|n| n.kind() == "block_list")
            .collect();
        assert!(!block_lists.is_empty(), "Expected block_list node");
        let blocks = block_lists[0].get_children("blocks");
        assert_eq!(blocks.len(), 1, "Expected 1 block, got {}", blocks.len());
    }
}
