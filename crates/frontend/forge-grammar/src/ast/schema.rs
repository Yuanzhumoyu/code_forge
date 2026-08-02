//! AstSchema — 运行时 AST 形状定义。
//!
//! `AstSchema` 描述每个语法规则对应的 AST 节点"长什么样"：
//! 有哪些字段、每个字段的类型（文本/子节点/可选子节点/...）。
//!
//! Lowering 引擎读取 schema，自动将 CST 转换为结构化的 AST。
//!
//! # 使用示例
//!
//! ```ignore
//! let mut schema = AstSchema::new();
//! schema.define_struct("inst_like")?
//!     .field_text("mnemonic", "IDENT")?
//!     .field_children("operands", "operand")?
//!     .build()?;
//!
//! schema.define_enum("operand")?
//!     .variant("Ident", "IDENT")?
//!     .variant("Reg", "REG")?
//!     .variant("Dec", "DEC")?
//!     .build()?;
//! ```

use crate::grammar::Grammar;
use std::collections::HashMap;
use std::fmt;

// ============================================================
// Symbol — interned string for fast kind comparison
// ============================================================

/// Interned string symbol — kind/field name 比较退化为 u32 整数比较。
///
/// 全局 interner 确保相同字符串始终映射到相同 `Symbol`。
/// Leaked strings allow returning `&'static str` without cloning.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct Symbol(u32);

// Module-level interner — shared by all Symbol methods
static SYMBOL_DATA: std::sync::LazyLock<std::sync::RwLock<Interner>> =
    std::sync::LazyLock::new(|| std::sync::RwLock::new(Interner::new()));

impl Symbol {
    /// 获取或创建一个 interned symbol。
    pub fn intern(s: &str) -> Self {
        let interner = &*SYMBOL_DATA;
        if let Ok(lock) = interner.read()
            && let Some(&id) = lock.str_to_id.get(s)
        {
            return Symbol(id);
        }
        if let Ok(mut lock) = interner.write() {
            if let Some(&id) = lock.str_to_id.get(s) {
                return Symbol(id);
            }
            let id = lock.next_id;
            lock.next_id += 1;
            let static_str: &'static str = Box::leak(s.to_string().into_boxed_str());
            lock.str_to_id.insert(static_str, id);
            lock.id_to_str.insert(id, static_str);
            Symbol(id)
        } else {
            Symbol(0)
        }
    }

    /// 从 Symbol 获取原始字符串。
    pub fn as_str(self) -> &'static str {
        if let Ok(lock) = SYMBOL_DATA.read()
            && let Some(&s) = lock.id_to_str.get(&self.0)
        {
            return s;
        }
        "<unknown>"
    }

    /// 内部 u32 值（用于序列化/调试）。
    pub fn raw(self) -> u32 {
        self.0
    }
}

impl fmt::Display for Symbol {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.as_str())
    }
}

impl From<&str> for Symbol {
    fn from(s: &str) -> Self {
        Symbol::intern(s)
    }
}

/// Internal interner storage. Strings are leaked as &'static str.
struct Interner {
    str_to_id: HashMap<&'static str, u32>,
    id_to_str: HashMap<u32, &'static str>,
    next_id: u32,
}

impl Interner {
    fn new() -> Self {
        Self {
            str_to_id: HashMap::new(),
            id_to_str: HashMap::new(),
            next_id: 1,
        }
    }
}

// Pre-interned common kind names can be added here when needed:
//   pub fn program_sym() -> Symbol { Symbol::intern("program") }
// These are created lazily via Symbol::intern() — the global interner
// ensures the same string always maps to the same Symbol.

// ============================================================
// NodeKind
// ============================================================

/// AST 节点的结构类型。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum NodeKind {
    /// 结构体节点：固定字段集合。
    Struct,
    /// 枚举节点：根据匹配的 token 分发到不同 variant。
    Enum,
}

// ============================================================
// FieldType
// ============================================================

/// 字段类型 — 描述如何从 CST 子树中提取字段值。
#[derive(Debug, Clone)]
pub enum FieldType {
    /// 提取单个子节点的文本（如 mnemonic: IDENT → String）。
    Text {
        /// CST 中要查找的子节点 kind。
        child_kind: String,
    },
    /// 提取一组子节点（如 operands: operand* → Vec<AstId>）。
    Children {
        /// CST 中要查找的子节点 kind。
        child_kind: String,
    },
    /// 提取一个可选子节点（如 ret_types: ("->" type_list)?）。
    OptionalChild {
        /// CST 中要查找的子节点 kind。
        child_kind: String,
    },
    /// 提取并自动跳过透明 wrapper 的多个子节点。
    FlattenedChildren {
        /// CST 中要查找的子节点 kind。
        child_kind: String,
    },
    /// 拼接所有叶子 token 文本（如多 token 标识符）。
    ConcatenatedText {
        /// 分隔符。
        sep: String,
    },
    /// 收集该节点所有直接 token 子节点的文本。
    AllTokens,
    /// 递归 lower 单个子节点（子节点在 schema 中有定义）。
    Node {
        /// 子节点对应的 grammar rule 名。
        rule_name: String,
    },
    /// 递归 lower 多个子节点。
    NodeList {
        /// 子节点对应的 grammar rule 名。
        rule_name: String,
    },
    /// 自定义字段 lowering 函数。
    /// 函数签名: `Fn(&CstNode, &mut AstArena) -> Result<FieldValue, LowerError>`
    /// Box 存储在 schema 中；实现 Clone 需要通过 Arc。
    Custom,
}

impl FieldType {
    /// 创建 Text 字段类型。
    pub fn text(child_kind: impl Into<String>) -> Self {
        FieldType::Text {
            child_kind: child_kind.into(),
        }
    }

    /// 创建 Children 字段类型。
    pub fn children(child_kind: impl Into<String>) -> Self {
        FieldType::Children {
            child_kind: child_kind.into(),
        }
    }

    /// 创建 OptionalChild 字段类型。
    pub fn optional(child_kind: impl Into<String>) -> Self {
        FieldType::OptionalChild {
            child_kind: child_kind.into(),
        }
    }

    /// 创建 FlattenedChildren 字段类型。
    pub fn flattened_children(child_kind: impl Into<String>) -> Self {
        FieldType::FlattenedChildren {
            child_kind: child_kind.into(),
        }
    }

    /// 创建 Node 字段类型。
    pub fn node(rule_name: impl Into<String>) -> Self {
        FieldType::Node {
            rule_name: rule_name.into(),
        }
    }

    /// 创建 NodeList 字段类型。
    pub fn node_list(rule_name: impl Into<String>) -> Self {
        FieldType::NodeList {
            rule_name: rule_name.into(),
        }
    }

    /// 验证此字段类型引用的 child_kind 是否在 grammar 中存在。
    pub fn validate_references(&self, grammar: &Grammar) -> Result<(), SchemaError> {
        match self {
            FieldType::Text { child_kind }
            | FieldType::Children { child_kind }
            | FieldType::OptionalChild { child_kind }
            | FieldType::FlattenedChildren { child_kind } => {
                let is_token = grammar.tokens.iter().any(|t| t.name == *child_kind);
                let is_rule = grammar.rules.contains_key(child_kind);
                if !is_token && !is_rule {
                    return Err(SchemaError::UnknownReference {
                        kind: child_kind.clone(),
                        message: format!(
                            "field references '{}' which is neither a token nor a rule in the grammar",
                            child_kind
                        ),
                    });
                }
            }
            FieldType::Node { rule_name } | FieldType::NodeList { rule_name } => {
                if !grammar.rules.contains_key(rule_name) {
                    return Err(SchemaError::UnknownReference {
                        kind: rule_name.clone(),
                        message: format!(
                            "field references rule '{}' which is not defined in the grammar",
                            rule_name
                        ),
                    });
                }
            }
            FieldType::ConcatenatedText { .. } | FieldType::AllTokens | FieldType::Custom => {
                // 这些类型不需要验证引用
            }
        }
        Ok(())
    }
}

// ============================================================
// FieldDef
// ============================================================

/// 一个 AST 节点字段的定义。
#[derive(Debug, Clone)]
pub struct FieldDef {
    /// 字段名（用于 AstRef 访问）。
    pub name: String,
    /// 字段类型。
    pub field_type: FieldType,
}

impl FieldDef {
    /// 创建新的字段定义。
    pub fn new(name: impl Into<String>, field_type: FieldType) -> Self {
        Self {
            name: name.into(),
            field_type,
        }
    }

    /// 验证此字段引用的 child_kind 是否在 grammar 中存在。
    pub fn validate_references(&self, grammar: &Grammar) -> Result<(), SchemaError> {
        self.field_type.validate_references(grammar)
    }
}

// ============================================================
// NodeDef
// ============================================================

/// 单个 AST 节点的完整定义。
#[derive(Debug, Clone)]
pub struct NodeDef {
    /// 对应的 grammar 规则名。
    pub rule_name: String,
    /// 节点结构类型。
    pub node_kind: NodeKind,
    /// 字段定义列表。
    pub fields: Vec<FieldDef>,
    /// 对于 enum 节点：variant 名称 → token kind 的映射。
    pub variants: Vec<VariantDef>,
}

/// Enum 节点的 variant 定义。
#[derive(Debug, Clone)]
pub struct VariantDef {
    /// Variant 名称（如 "Ident", "Reg"）。
    pub name: String,
    /// 匹配的 token kind（如 "IDENT", "REG"）。
    pub token_kind: String,
}

impl NodeDef {
    /// 创建一个 struct 类型的节点定义。
    pub fn new_struct(rule_name: impl Into<String>) -> Self {
        Self {
            rule_name: rule_name.into(),
            node_kind: NodeKind::Struct,
            fields: Vec::new(),
            variants: Vec::new(),
        }
    }

    /// 创建一个 enum 类型的节点定义。
    pub fn new_enum(rule_name: impl Into<String>) -> Self {
        Self {
            rule_name: rule_name.into(),
            node_kind: NodeKind::Enum,
            fields: Vec::new(),
            variants: Vec::new(),
        }
    }

    /// 添加一个字段。
    pub fn add_field(&mut self, name: impl Into<String>, field_type: FieldType) {
        self.fields.push(FieldDef::new(name, field_type));
    }

    /// 添加一个 enum variant。
    pub fn add_variant(&mut self, name: impl Into<String>, token_kind: impl Into<String>) {
        self.variants.push(VariantDef {
            name: name.into(),
            token_kind: token_kind.into(),
        });
    }
}

// ============================================================
// SchemaError / SchemaDiagnostic
// ============================================================

/// Schema 构建或验证时的错误。
#[derive(Debug, Clone)]
pub enum SchemaError {
    /// 引用了未知的 rule 或 token。
    UnknownReference { kind: String, message: String },
    /// 重复的节点定义。
    DuplicateNode(String),
    /// 重复的 enum variant。
    DuplicateVariant { enum_name: String, variant: String },
    /// 验证失败（通用）。
    Validation(String),
}

impl fmt::Display for SchemaError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            SchemaError::UnknownReference { kind, message } => {
                write!(f, "unknown reference '{}': {}", kind, message)
            }
            SchemaError::DuplicateNode(name) => write!(f, "duplicate node definition '{}'", name),
            SchemaError::DuplicateVariant { enum_name, variant } => {
                write!(f, "duplicate variant '{}' in enum '{}'", variant, enum_name)
            }
            SchemaError::Validation(msg) => write!(f, "schema validation error: {}", msg),
        }
    }
}

/// Schema 构建时的诊断（非致命）。
#[derive(Debug, Clone)]
pub enum SchemaDiagnostic {
    /// Grammar 中的 rule 在 schema 中没有对应定义。
    MissingDefinition(String),
    /// Schema 中的 node 在 grammar 中找不到。
    OrphanedNode(String),
}

// ============================================================
// AstSchema
// ============================================================

/// 运行时 AST 形状定义。
///
/// 描述每个 grammar rule 对应的 AST 节点结构。
/// 被 lowering 引擎用于自动将 CST 转换为结构化 AST。
#[derive(Debug, Clone, Default)]
pub struct AstSchema {
    /// 规则名 → 节点定义。
    nodes: HashMap<String, NodeDef>,
    /// Schema 构建过程中的诊断。
    pub diagnostics: Vec<SchemaDiagnostic>,
}

impl AstSchema {
    /// 创建空的 schema。
    pub fn new() -> Self {
        Self {
            nodes: HashMap::new(),
            diagnostics: Vec::new(),
        }
    }

    /// 查找指定规则名对应的节点定义。
    pub fn lookup(&self, rule_name: &str) -> Option<&NodeDef> {
        self.nodes.get(rule_name)
    }

    /// 查找 enum 节点的 variant（根据 token kind）。
    pub fn lookup_variant(&self, rule_name: &str, token_kind: &str) -> Option<&VariantDef> {
        self.nodes
            .get(rule_name)
            .and_then(|def| def.variants.iter().find(|v| v.token_kind == token_kind))
    }

    /// 是否有指定规则的节点定义。
    pub fn contains(&self, rule_name: &str) -> bool {
        self.nodes.contains_key(rule_name)
    }

    /// 开始定义一个 struct 节点。
    pub fn define_struct(&mut self, rule_name: &str) -> Result<StructBuilder<'_>, SchemaError> {
        if self.nodes.contains_key(rule_name) {
            return Err(SchemaError::DuplicateNode(rule_name.to_string()));
        }
        Ok(StructBuilder {
            schema: self,
            rule_name: rule_name.to_string(),
            fields: Vec::new(),
        })
    }

    /// 开始定义一个 enum 节点。
    pub fn define_enum(&mut self, rule_name: &str) -> Result<EnumBuilder<'_>, SchemaError> {
        if self.nodes.contains_key(rule_name) {
            return Err(SchemaError::DuplicateNode(rule_name.to_string()));
        }
        Ok(EnumBuilder {
            schema: self,
            rule_name: rule_name.to_string(),
            variants: Vec::new(),
        })
    }

    /// 验证 schema 与 grammar 的一致性。
    ///
    /// 检查：
    /// 1. schema 中的每个节点引用的 token/rule 在 grammar 中存在
    /// 2. enum variant 引用的 token 在 grammar 中已定义
    /// 3. grammar 中所有规则都有 schema 定义（仅产生诊断，不报错）
    pub fn validate(&mut self, grammar: &Grammar) -> Result<(), SchemaError> {
        for def in self.nodes.values() {
            // 校验 rule 在 grammar 中存在
            if !grammar.rules.contains_key(&def.rule_name)
                && !grammar.tokens.iter().any(|t| t.name == def.rule_name)
            {
                // Rule 不在 grammar 中，但可能在 lowering 时作为 transparent pass-through
                self.diagnostics
                    .push(SchemaDiagnostic::OrphanedNode(def.rule_name.clone()));
            }

            // 校验字段引用
            for field in &def.fields {
                field.validate_references(grammar).map_err(|e| {
                    SchemaError::Validation(format!(
                        "in node '{}', field '{}': {}",
                        def.rule_name, field.name, e
                    ))
                })?;
            }

            // 校验 enum variants
            if def.node_kind == NodeKind::Enum {
                for variant in &def.variants {
                    let exists = grammar.tokens.iter().any(|t| t.name == variant.token_kind);
                    if !exists {
                        return Err(SchemaError::UnknownReference {
                            kind: variant.token_kind.clone(),
                            message: format!(
                                "enum '{}' variant '{}' references unknown token '{}'",
                                def.rule_name, variant.name, variant.token_kind
                            ),
                        });
                    }
                }
            }
        }

        // 诊断：grammar 中所有 rule 是否都有 schema 定义
        for rule_name in grammar.rules.keys() {
            if !self.nodes.contains_key(rule_name) {
                self.diagnostics
                    .push(SchemaDiagnostic::MissingDefinition(rule_name.clone()));
            }
        }

        Ok(())
    }

    /// 从 Grammar 自动推导基本 schema。
    ///
    /// 对于每个 grammar rule：
    /// - 直接 token 引用 → Text 字段
    /// - 子 rule 引用 → Children 字段
    /// - Alt → Enum 节点
    ///
    /// 这是一个启发式推导，可能不完美；用户可以用 `define_struct`/`define_enum`
    /// 覆盖任何自动生成的条目。
    pub fn infer_from_grammar(grammar: &Grammar) -> Self {
        let mut schema = Self::new();

        for (rule_name, rule_def) in &grammar.rules {
            // 跳过明显是 EBNF wrapper 的规则名
            if rule_name == "epsilon" {
                continue;
            }

            match &rule_def.expr {
                // 如果是 Alt，可能是 enum
                crate::grammar::Expr::Alt(alternatives) => {
                    let is_simple_enum = alternatives.iter().all(|alt| {
                        matches!(alt, crate::grammar::Expr::Token(_))
                            || matches!(alt, crate::grammar::Expr::Lit(_))
                    });
                    if is_simple_enum {
                        if let Ok(mut builder) = schema.define_enum(rule_name) {
                            for alt in alternatives {
                                let (variant_name, token_kind) = match alt {
                                    crate::grammar::Expr::Token(name) => {
                                        (name.clone(), name.clone())
                                    }
                                    crate::grammar::Expr::Lit(text) => (text.clone(), text.clone()),
                                    _ => continue,
                                };
                                builder.add_variant(&variant_name, &token_kind);
                            }
                            builder.finish();
                        }
                    } else {
                        // Complex Alt → struct with children
                        if let Ok(mut builder) = schema.define_struct(rule_name) {
                            builder.field_children("alternatives", &format!("{}_alt", rule_name));
                            builder.finish();
                        }
                    }
                }
                // 其他情况 → struct
                _ => {
                    // 先收集引用（避免借用冲突）
                    let refs = AstSchema::collect_direct_refs_static(&rule_def.expr);
                    if let Ok(mut builder) = schema.define_struct(rule_name) {
                        for (child_kind, is_multiple) in refs {
                            if grammar.tokens.iter().any(|t| t.name == child_kind) {
                                if is_multiple {
                                    builder.field_children(&child_kind, &child_kind);
                                } else {
                                    builder.field_text(&child_kind, &child_kind);
                                }
                            } else if grammar.rules.contains_key(&child_kind) {
                                builder.field_children(&child_kind, &child_kind);
                            }
                        }
                        builder.finish();
                    }
                }
            }
        }

        schema
    }

    /// 收集表达式中的直接子引用（不递归）。静态版本，避免 borrow 冲突。
    fn collect_direct_refs_static(expr: &crate::grammar::Expr) -> Vec<(String, bool)> {
        let mut refs = Vec::new();
        match expr {
            crate::grammar::Expr::Seq(items) => {
                for item in items {
                    refs.extend(AstSchema::collect_direct_refs_static(item));
                }
            }
            crate::grammar::Expr::Alt(items) => {
                for item in items {
                    refs.extend(AstSchema::collect_direct_refs_static(item));
                }
            }
            crate::grammar::Expr::Token(name) => {
                refs.push((name.clone(), false));
            }
            crate::grammar::Expr::Rule(name) => {
                refs.push((name.clone(), false));
            }
            crate::grammar::Expr::ZeroOrMore(inner) => {
                let inner_refs = AstSchema::collect_direct_refs_static(inner);
                for (name, _) in inner_refs {
                    refs.push((name, true)); // * → multiple
                }
            }
            crate::grammar::Expr::OneOrMore(inner) => {
                let inner_refs = AstSchema::collect_direct_refs_static(inner);
                for (name, _) in inner_refs {
                    refs.push((name, true)); // + → multiple
                }
            }
            crate::grammar::Expr::Opt(inner) => {
                // 对于 Optional，仍然收集引用（作为可选处理）
                refs.extend(AstSchema::collect_direct_refs_static(inner));
            }
            _ => {}
        }
        refs
    }
}

// ============================================================
// Builders
// ============================================================

/// Struct 节点的 builder。
#[derive(Debug)]
pub struct StructBuilder<'a> {
    schema: &'a mut AstSchema,
    rule_name: String,
    fields: Vec<FieldDef>,
}

impl<'a> StructBuilder<'a> {
    /// 添加一个文本字段。
    pub fn field_text(&mut self, field_name: &str, child_kind: &str) -> &mut Self {
        self.fields
            .push(FieldDef::new(field_name, FieldType::text(child_kind)));
        self
    }

    /// 添加一个子节点列表字段。
    pub fn field_children(&mut self, field_name: &str, child_kind: &str) -> &mut Self {
        self.fields
            .push(FieldDef::new(field_name, FieldType::children(child_kind)));
        self
    }

    /// 添加一个可选子节点字段。
    pub fn field_optional(&mut self, field_name: &str, child_kind: &str) -> &mut Self {
        self.fields
            .push(FieldDef::new(field_name, FieldType::optional(child_kind)));
        self
    }

    /// 添加一个展平子节点列表字段。
    pub fn field_flattened(&mut self, field_name: &str, child_kind: &str) -> &mut Self {
        self.fields.push(FieldDef::new(
            field_name,
            FieldType::flattened_children(child_kind),
        ));
        self
    }

    /// 添加一个递归 lower 的单个子节点字段。
    pub fn field_node(&mut self, field_name: &str, rule_name: &str) -> &mut Self {
        self.fields
            .push(FieldDef::new(field_name, FieldType::node(rule_name)));
        self
    }

    /// 添加一个递归 lower 的多个子节点字段。
    pub fn field_node_list(&mut self, field_name: &str, rule_name: &str) -> &mut Self {
        self.fields
            .push(FieldDef::new(field_name, FieldType::node_list(rule_name)));
        self
    }

    /// 添加一个拼接文本字段。
    pub fn field_concat_text(&mut self, field_name: &str, sep: &str) -> &mut Self {
        self.fields.push(FieldDef::new(
            field_name,
            FieldType::ConcatenatedText {
                sep: sep.to_string(),
            },
        ));
        self
    }

    /// 添加一个所有 token 字段。
    pub fn field_all_tokens(&mut self, field_name: &str) -> &mut Self {
        self.fields
            .push(FieldDef::new(field_name, FieldType::AllTokens));
        self
    }

    /// 完成构建，将节点定义注册到 schema 中。
    pub fn finish(&mut self) {
        let def = NodeDef {
            rule_name: self.rule_name.clone(),
            node_kind: NodeKind::Struct,
            fields: std::mem::take(&mut self.fields),
            variants: Vec::new(),
        };
        self.schema.nodes.insert(self.rule_name.clone(), def);
    }
}

/// Enum 节点的 builder。
#[derive(Debug)]
pub struct EnumBuilder<'a> {
    schema: &'a mut AstSchema,
    rule_name: String,
    variants: Vec<VariantDef>,
}

impl<'a> EnumBuilder<'a> {
    /// 添加一个 variant。
    pub fn add_variant(&mut self, variant_name: &str, token_kind: &str) -> &mut Self {
        self.variants.push(VariantDef {
            name: variant_name.to_string(),
            token_kind: token_kind.to_string(),
        });
        self
    }

    /// 完成构建，将 enum 定义注册到 schema 中。
    /// Enum 节点不需要预定义字段——lowering 引擎直接处理 variant 分发。
    pub fn finish(&mut self) {
        let def = NodeDef {
            rule_name: self.rule_name.clone(),
            node_kind: NodeKind::Enum,
            fields: Vec::new(),
            variants: std::mem::take(&mut self.variants),
        };
        self.schema.nodes.insert(self.rule_name.clone(), def);
    }
}

// ============================================================
// Tests
// ============================================================

#[cfg(test)]
mod tests {
    use super::*;
    use crate::grammar::parse_grammar;

    #[test]
    fn test_symbol_intern() {
        let a = Symbol::intern("function_def");
        let b = Symbol::intern("function_def");
        let c = Symbol::intern("block");

        assert_eq!(a, b);
        assert_ne!(a, c);
        assert_eq!(a.raw(), b.raw());
    }

    #[test]
    fn test_schema_builder_struct() {
        let mut schema = AstSchema::new();

        schema
            .define_struct("inst_like")
            .unwrap()
            .field_text("mnemonic", "IDENT")
            .field_children("operands", "operand")
            .finish();

        let def = schema.lookup("inst_like").unwrap();
        assert_eq!(def.rule_name, "inst_like");
        assert_eq!(def.node_kind, NodeKind::Struct);
        assert_eq!(def.fields.len(), 2);
        assert_eq!(def.fields[0].name, "mnemonic");
        assert_eq!(def.fields[1].name, "operands");
    }

    #[test]
    fn test_schema_builder_enum() {
        let mut schema = AstSchema::new();

        schema
            .define_enum("operand")
            .unwrap()
            .add_variant("Ident", "IDENT")
            .add_variant("Reg", "REG")
            .add_variant("Dec", "DEC")
            .finish();

        let def = schema.lookup("operand").unwrap();
        assert_eq!(def.node_kind, NodeKind::Enum);
        assert_eq!(def.variants.len(), 3);

        let variant = schema.lookup_variant("operand", "REG").unwrap();
        assert_eq!(variant.name, "Reg");
    }

    #[test]
    fn test_schema_duplicate_node_error() {
        let mut schema = AstSchema::new();
        schema
            .define_struct("foo")
            .unwrap()
            .field_text("x", "X")
            .finish();

        let err = schema.define_struct("foo").unwrap_err();
        assert!(matches!(err, SchemaError::DuplicateNode(_)));
    }

    #[test]
    fn test_schema_validate_with_grammar() {
        let grammar_src = r#"
            token IDENT = "[a-z]+"
            token REG = "[A-Z]+"
            token DEC = "[0-9]+"
            skip "[ \t]+"

            inst_like ::= IDENT operand*
            operand ::= IDENT | REG | DEC
        "#;
        let grammar = parse_grammar(grammar_src).unwrap();

        let mut schema = AstSchema::new();
        schema
            .define_struct("inst_like")
            .unwrap()
            .field_text("mnemonic", "IDENT")
            .field_children("operands", "operand")
            .finish();

        schema
            .define_enum("operand")
            .unwrap()
            .add_variant("Ident", "IDENT")
            .add_variant("Reg", "REG")
            .add_variant("Dec", "DEC")
            .finish();

        // Validation should pass
        schema.validate(&grammar).unwrap();

        // There should be a MissingDefinition diagnostic for "inst_like" rule
        // (since schema has it defined, it's fine — no MissingDefinition for defined ones)
        // Actually, "inst_like" IS defined in schema so it should not produce MissingDefinition.
    }

    #[test]
    fn test_schema_validate_bad_reference() {
        let grammar_src = r#"
            token IDENT = "[a-z]+"
            skip "[ \t]+"
            rule ::= IDENT
        "#;
        let grammar = parse_grammar(grammar_src).unwrap();

        let mut schema = AstSchema::new();
        schema
            .define_struct("rule")
            .unwrap()
            .field_text("name", "NONEXISTENT_TOKEN")
            .finish();

        // Validation should fail because NONEXISTENT_TOKEN is not in the grammar
        let err = schema.validate(&grammar).unwrap_err();
        assert!(
            format!("{}", err).contains("NONEXISTENT_TOKEN"),
            "Expected error about NONEXISTENT_TOKEN, got: {}",
            err
        );
    }

    #[test]
    fn test_schema_infer_from_grammar() {
        let grammar_src = r#"
            token IDENT = "[a-z]+"
            token REG = "[A-Z]+"
            token DEC = "[0-9]+"
            skip "[ \t]+"

            program ::= stmt*
            stmt ::= inst_like | label_def
            inst_like ::= IDENT operand*
            operand ::= IDENT | REG | DEC
            label_def ::= IDENT ":"
        "#;
        let grammar = parse_grammar(grammar_src).unwrap();
        let schema = AstSchema::infer_from_grammar(&grammar);

        // operand should be inferred as enum
        let operand_def = schema.lookup("operand");
        assert!(operand_def.is_some());
        // It should have variants for IDENT, REG, DEC
        if let Some(def) = operand_def {
            assert!(
                !def.variants.is_empty(),
                "Expected operand to have variants"
            );
        }

        // program should be inferred as struct
        assert!(schema.lookup("program").is_some());
        // inst_like should be inferred
        assert!(schema.lookup("inst_like").is_some());
    }
}
