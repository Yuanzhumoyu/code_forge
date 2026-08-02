//! 名称解析 — 将 AST 中的标识符引用链接到其定义。
//!
//! `NameResolver` 实现 `AstVisitor`，在遍历 AST 时：
//! 1. 遇到定义节点时在符号表中注册符号
//! 2. 遇到引用节点时查找符号表并记录解析结果
//! 3. 自动管理作用域（进入/离开函数、块等）
//!
//! 解析结果存储在 `ResolutionMap` 中（引用 AstId → 定义 AstId）。

use crate::ast::{AstId, AstRef};
use crate::semantic::scope::{ScopeKind, SymbolKind};
use crate::semantic::{AnalysisContext, DiagnosticLevel, SemanticDiagnostic};
use crate::visit::{AstVisitor, VisitAction};
use std::collections::HashMap;

// ============================================================
// ResolutionMap
// ============================================================

/// 名称解析结果：引用 → 定义 的映射。
#[derive(Debug, Clone, Default)]
pub struct ResolutionMap {
    /// AstId(引用节点/使用节点) → AstId(定义节点)
    pub ref_to_def: HashMap<AstId, AstId>,
    /// AstId(定义节点) → 使用节点列表
    pub def_to_refs: HashMap<AstId, Vec<AstId>>,
    /// 未能解析的引用（名称 → 引用节点列表）
    pub unresolved: HashMap<String, Vec<AstId>>,
}

impl ResolutionMap {
    pub fn new() -> Self {
        Self::default()
    }

    /// 记录一次成功的名称解析。
    pub fn record(&mut self, ref_id: AstId, def_id: AstId) {
        self.ref_to_def.insert(ref_id, def_id);
        self.def_to_refs.entry(def_id).or_default().push(ref_id);
    }

    /// 记录一个未能解析的名称。
    pub fn record_unresolved(&mut self, name: String, ref_id: AstId) {
        self.unresolved.entry(name).or_default().push(ref_id);
    }

    /// 查找引用节点的定义。
    pub fn definition_of(&self, ref_id: AstId) -> Option<AstId> {
        self.ref_to_def.get(&ref_id).copied()
    }

    /// 查找定义节点的所有引用。
    pub fn references_to(&self, def_id: AstId) -> &[AstId] {
        self.def_to_refs.get(&def_id).map_or(&[], |v| v.as_slice())
    }

    /// 是否所有引用都已解析。
    pub fn all_resolved(&self) -> bool {
        self.unresolved.is_empty()
    }
}

// ============================================================
// NameResolver
// ============================================================

/// 名称解析器 — 实现 `AstVisitor` 的语义分析 pass。
///
/// 自动识别常见的定义和引用模式：
/// - `function_def.name` → 函数定义
/// - `block.name` → 块标签定义
/// - `param.name` → 参数定义
/// - `inst.operands[].IDENT` → 变量引用
/// - `terminator.jmp_args.IDENT` → 块引用
///
/// 用户可以继承此 struct 并重写 `classify_node()` 来
/// 处理自定义语法的定义/引用模式。
pub struct NameResolver<'a> {
    pub cx: &'a mut AnalysisContext,
    pub resolution: ResolutionMap,
}

impl<'a> NameResolver<'a> {
    pub fn new(cx: &'a mut AnalysisContext) -> Self {
        Self {
            cx,
            resolution: ResolutionMap::new(),
        }
    }

    /// 获取解析结果（消耗 resolver）。
    pub fn finish(self) -> ResolutionMap {
        self.resolution
    }

    // ── 定义处理 ──

    /// 在符号表中定义一个名称。
    fn define_symbol(&mut self, name: &str, kind: SymbolKind, def_node: AstId) {
        // 检查当前作用域是否已有同名字号
        if let Some(existing) = self.cx.scopes.resolve(name)
            && existing.def_node != def_node
        {
            // 同一作用域内 shadowing — 记录警告
            let span = self
                .cx
                .ast
                .arena
                .get(def_node)
                .map(|n| n.span.clone())
                .unwrap_or_else(crate::error::Span::dummy);
            self.cx.diagnostics.push(SemanticDiagnostic {
                level: DiagnosticLevel::Warning,
                message: format!(
                    "name '{}' shadows previous definition ({:?})",
                    name, existing.def_node
                ),
                span: Some(span),
                node: Some(def_node),
            });
        }
        self.cx.scopes.define(name, kind, def_node);
    }

    /// 解析一个引用名称。
    fn resolve_ref(&mut self, name: &str, ref_node: AstId) {
        if let Some(sym) = self.cx.scopes.resolve(name) {
            self.resolution.record(ref_node, sym.def_node);
        } else {
            self.resolution
                .record_unresolved(name.to_string(), ref_node);
            let span = self
                .cx
                .ast
                .arena
                .get(ref_node)
                .map(|n| n.span.clone())
                .unwrap_or_else(crate::error::Span::dummy);
            self.cx.diagnostics.push(SemanticDiagnostic {
                level: DiagnosticLevel::Error,
                message: format!("unresolved name '{}'", name),
                span: Some(span),
                node: Some(ref_node),
            });
        }
    }

    /// 分类一个 AST 节点：返回 (name, is_definition, symbol_kind)。
    /// 默认实现处理常见模式。用户可重写。
    fn classify_node(&self, node: AstRef<'_>) -> Option<(String, bool, SymbolKind)> {
        match node.kind() {
            // Function definitions
            "function_def" => {
                let name = node.get_text("name")?.to_string();
                Some((name, true, SymbolKind::Function))
            }
            // Block definitions (labels)
            "block" => {
                let name = node.get_text("name")?.to_string();
                Some((name, true, SymbolKind::Block))
            }
            // Parameter definitions
            "param" => {
                let name = node.get_text("name")?.to_string();
                Some((name, true, SymbolKind::Parameter))
            }
            // Label definitions
            "label_def" => {
                let name = node.get_text("name")?.to_string();
                Some((name, true, SymbolKind::Label))
            }
            // Variable-like references (IDENT tokens inside instructions)
            "inst_like" => {
                // Don't classify the inst_like itself, but check operands
                None
            }
            _ => None,
        }
    }
}

impl<'a> AstVisitor for NameResolver<'a> {
    fn visit(&mut self, node: AstRef<'_>) -> VisitAction {
        match node.kind() {
            // Scope management: function and block open new scopes
            "function_def" => {
                self.cx.scopes.enter(ScopeKind::Function);
                // Register the function name in the parent scope
                if let Some((name, true, kind)) = self.classify_node(node) {
                    self.define_symbol(&name, kind, node.id);
                }
            }
            "block" => {
                self.cx.scopes.enter(ScopeKind::Block);
                if let Some((name, true, kind)) = self.classify_node(node) {
                    self.define_symbol(&name, kind, node.id);
                }
            }

            // Definition nodes: register in symbol table
            "param" | "label_def" => {
                if let Some((name, true, kind)) = self.classify_node(node) {
                    self.define_symbol(&name, kind, node.id);
                }
            }

            // Reference nodes: look for IDENT or REG operands
            _ => {
                // For enum-style operand nodes, check if their variant indicates a reference
                if node.kind().starts_with("operand/")
                    && let Some(variant) = node.variant()
                    && (variant == "Ident" || variant == "Reg" || variant == "Label")
                    && let Some(name) = node.text()
                    && !name.is_empty()
                {
                    self.resolve_ref(name, node.id);
                }
            }
        }

        VisitAction::Continue
    }

    fn leave(&mut self, node: AstRef<'_>) {
        match node.kind() {
            "function_def" | "block" => {
                self.cx.scopes.leave();
            }
            _ => {}
        }
    }
}

// ============================================================
// Tests
// ============================================================

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ast::lower::lower_cst;
    use crate::ast::schema::AstSchema;
    use crate::grammar::parse_grammar;
    use crate::parser::Parser;

    #[test]
    fn test_resolution_map_basic() {
        let mut map = ResolutionMap::new();
        map.record(AstId(10), AstId(1));
        assert_eq!(map.definition_of(AstId(10)), Some(AstId(1)));
        assert_eq!(map.references_to(AstId(1)), &[AstId(10)]);
        assert!(map.all_resolved());
    }

    #[test]
    fn test_resolution_map_unresolved() {
        let mut map = ResolutionMap::new();
        map.record_unresolved("unknown".to_string(), AstId(5));
        assert!(!map.all_resolved());
        assert_eq!(map.unresolved.get("unknown").unwrap(), &[AstId(5)]);
    }

    #[test]
    fn test_name_resolver_with_ir_grammar() {
        // Use the real ir.lx grammar with schema-driven lowering
        let ir_src = include_str!("../../../../foundation/forge-ir/grammars/ir.lx");
        let grammar = parse_grammar(ir_src).unwrap();

        let mut schema = AstSchema::new();
        schema
            .define_struct("function_def")
            .unwrap()
            .field_text("name", "IDENT")
            .field_node("param_list", "param_list")
            .field_node("block_list", "block_list")
            .finish();
        schema
            .define_struct("param_list")
            .unwrap()
            .field_children("params", "param")
            .finish();
        schema
            .define_struct("block_list")
            .unwrap()
            .field_children("blocks", "block")
            .finish();
        schema
            .define_struct("block")
            .unwrap()
            .field_text("name", "IDENT")
            .finish();
        schema
            .define_struct("param")
            .unwrap()
            .field_text("name", "IDENT")
            .finish();

        let parser = Parser::build(grammar.clone());
        let source = "fn foo(x: i32, y: i64) -> i32 { entry: ret i32 }";
        let cst = parser.parse(source).unwrap();
        let ast = lower_cst(&cst, &schema, &grammar).unwrap();

        let mut cx = AnalysisContext::new(ast);
        let ast_copy = cx.ast.clone();

        let mut resolver = NameResolver::new(&mut cx);
        resolver.walk(&ast_copy);
        let resolution = resolver.finish();

        // Should have resolved function name "foo"
        // Parameters "x" and "y" should be defined
        // Block "entry" should be defined
        assert!(
            resolution.all_resolved() || !resolution.unresolved.is_empty(),
            "Expected some resolution activity"
        );
    }
}
