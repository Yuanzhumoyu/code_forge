//! 内置 AST 节点类型 — 为常见节点模式提供类型安全的包装。
//!
//! 这些包装器不是由 proc-macro 生成的，而是手工编写的 thin wrapper
//! 围绕 `AstRef`。用户可以为自己的语法定义类似的结构体。
//!
//! # 模式
//!
//! 每个包装器实现 `From<AstRef<'a>>`（或提供 `from_astref`），
//! 允许从动态 AST 安全地转换到类型化包装器。

use super::AstRef;

// ============================================================
// Ident — 标识符节点
// ============================================================

/// 标识符节点：包装一个带有 "text" 字段的 AST 节点。
#[derive(Debug, Clone)]
pub struct Ident {
    /// 标识符文本。
    pub name: String,
}

impl Ident {
    /// 从 AstRef 提取标识符（通过 "text" 字段或节点 text）。
    pub fn from_astref(node: AstRef<'_>) -> Option<Self> {
        let text = node.get_text("text").or_else(|| node.text())?;
        Some(Self {
            name: text.to_string(),
        })
    }
}

// ============================================================
// Literal — 字面量节点
// ============================================================

/// 字面量节点。
#[derive(Debug, Clone)]
pub struct Literal {
    /// 字面量文本。
    pub text: String,
    /// 字面量类型（"DEC", "HEX", "FLOAT", "STRING" 等）。
    pub lit_kind: String,
}

impl Literal {
    /// 从 AstRef 提取字面量。
    pub fn from_astref(node: AstRef<'_>) -> Option<Self> {
        let text = node.get_text("text").or_else(|| node.text())?;
        Some(Self {
            text: text.to_string(),
            lit_kind: node.kind().to_string(),
        })
    }

    /// 尝试解析为 i64。
    pub fn as_int(&self) -> Option<i64> {
        match self.lit_kind.as_str() {
            "DEC" => self.text.parse().ok(),
            "HEX" => {
                let stripped = self.text.trim_start_matches("0x").replace('_', "");
                i64::from_str_radix(&stripped, 16).ok()
            }
            "BIN" => {
                let stripped = self.text.trim_start_matches("0b").replace('_', "");
                i64::from_str_radix(&stripped, 2).ok()
            }
            "OCT" => {
                let stripped = self.text.trim_start_matches("0o").replace('_', "");
                i64::from_str_radix(&stripped, 8).ok()
            }
            _ => None,
        }
    }

    /// 尝试解析为 u64。
    pub fn as_uint(&self) -> Option<u64> {
        match self.lit_kind.as_str() {
            "DEC" => self.text.parse().ok(),
            "HEX" => {
                let stripped = self.text.trim_start_matches("0x").replace('_', "");
                u64::from_str_radix(&stripped, 16).ok()
            }
            _ => None,
        }
    }

    /// 尝试解析为 f64。
    pub fn as_float(&self) -> Option<f64> {
        self.text.parse().ok()
    }

    /// 获取字符串内容（去除引号）。
    pub fn as_string(&self) -> Option<String> {
        if self.lit_kind == "STRING" {
            let s = self.text.trim();
            if (s.starts_with('"') && s.ends_with('"'))
                || (s.starts_with('\'') && s.ends_with('\''))
            {
                Some(s[1..s.len() - 1].to_string())
            } else {
                Some(s.to_string())
            }
        } else {
            None
        }
    }
}

// ============================================================
// Seq — 序列节点
// ============================================================

/// 序列（同构列表）节点包装器。
#[derive(Debug, Clone)]
pub struct Seq<T> {
    /// 子节点列表。
    pub items: Vec<T>,
}

impl<'a> Seq<AstRef<'a>> {
    /// 从 AstRef 提取序列（通过 "items" 字段）。
    pub fn from_astref(node: AstRef<'a>) -> Self {
        let items = node.get_children("items");
        Self { items }
    }
}

// ============================================================
// Optional — 可选节点
// ============================================================

/// 可选节点包装器。
#[derive(Debug, Clone)]
pub struct Optional<T> {
    /// 内部节点（如果存在）。
    pub inner: Option<T>,
}

// ============================================================
// NodeExt trait — 扩展 AstRef 的便捷方法
// ============================================================

/// 为 AstRef 提供便捷的布尔检查方法。
pub trait NodeExt {
    /// 检查节点是否为指定 kind。
    fn is_kind(&self, kind: &str) -> bool;

    /// 检查节点是否在指定 kind 列表中。
    fn is_kind_any(&self, kinds: &[&str]) -> bool;

    /// 获取节点的直接 token 文本（第一个直接 token 子节点）。
    fn direct_text(&self) -> Option<String>;
}

// We implement this via a new extension struct since AstRef is in ast::mod
// and we can't add methods to it from here without the trait being in scope.
// For simplicity, users call AstRef methods directly.

// ============================================================
// Tests
// ============================================================

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ast::schema::AstSchema;
    use crate::ast::{AstArena, AstNodeData, FieldValue, TypedAst};
    use crate::error::Span;
    use std::collections::HashMap;

    fn make_ident_astref(text: &str) -> (TypedAst, crate::ast::AstId) {
        let mut arena = AstArena::new();
        let schema = AstSchema::new();
        let mut fields = HashMap::new();
        fields.insert("text".to_string(), FieldValue::Text(text.to_string()));
        fields.insert("variant".to_string(), FieldValue::Text("Ident".to_string()));
        let id = arena.alloc(AstNodeData::with_fields(
            "operand/Ident",
            fields,
            Span::dummy(),
        ));
        let root = arena.alloc(AstNodeData::new("root", Span::dummy()));
        arena
            .get_mut(root)
            .unwrap()
            .fields
            .insert("operand".to_string(), FieldValue::Child(id));
        let ast = TypedAst::with_root(schema, arena, root);
        (ast, id)
    }

    #[test]
    fn test_ident_from_astref() {
        let (ast, id) = make_ident_astref("myVar");
        let node = ast.ref_to(id);
        let ident = Ident::from_astref(node).unwrap();
        assert_eq!(ident.name, "myVar");
    }

    #[test]
    fn test_ident_from_astref_missing_text() {
        let mut arena = AstArena::new();
        let schema = AstSchema::new();
        let id = arena.alloc(AstNodeData::new("operand/Ident", Span::dummy()));
        let ast = TypedAst::with_root(schema, arena, id);
        let node = ast.root_ref();
        assert!(Ident::from_astref(node).is_none());
    }

    #[test]
    fn test_literal_from_astref_dec() {
        let (ast, id) = make_ident_astref("42");
        // Override the kind to simulate a DEC literal
        let node = ast.ref_to(id);
        let lit = Literal::from_astref(node).unwrap();
        assert_eq!(lit.text, "42");
    }

    #[test]
    fn test_literal_as_int_dec() {
        let lit = Literal {
            text: "42".into(),
            lit_kind: "DEC".into(),
        };
        assert_eq!(lit.as_int(), Some(42));
    }

    #[test]
    fn test_literal_as_int_hex() {
        let lit = Literal {
            text: "0xFF".into(),
            lit_kind: "HEX".into(),
        };
        assert_eq!(lit.as_int(), Some(255));
    }

    #[test]
    fn test_literal_as_uint_hex() {
        let lit = Literal {
            text: "0xABCD".into(),
            lit_kind: "HEX".into(),
        };
        assert_eq!(lit.as_uint(), Some(0xABCD));
    }

    #[test]
    fn test_literal_as_float() {
        let lit = Literal {
            text: "3.14".into(),
            lit_kind: "FLOAT".into(),
        };
        assert!(lit.as_float().is_some());
    }

    #[test]
    fn test_literal_as_string() {
        let lit = Literal {
            text: "\"hello\"".into(),
            lit_kind: "STRING".into(),
        };
        assert_eq!(lit.as_string(), Some("hello".into()));
    }

    #[test]
    fn test_literal_as_int_bin() {
        let lit = Literal {
            text: "0b1010".into(),
            lit_kind: "BIN".into(),
        };
        assert_eq!(lit.as_int(), Some(10));
    }

    #[test]
    fn test_literal_as_int_oct() {
        let lit = Literal {
            text: "0o77".into(),
            lit_kind: "OCT".into(),
        };
        assert_eq!(lit.as_int(), Some(63));
    }

    #[test]
    fn test_literal_as_int_unknown_kind() {
        let lit = Literal {
            text: "hello".into(),
            lit_kind: "STRING".into(),
        };
        assert_eq!(lit.as_int(), None);
    }
}
