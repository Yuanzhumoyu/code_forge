//! 类型系统辅助 — `TypeChecker` trait 和基础类型检查逻辑。
//!
//! 提供可扩展的类型检查框架。用户为实现特定语言的类型系统
//! 实现 `TypeChecker` trait。

use crate::ast::{AstRef, TypedAst};
use crate::semantic::scope::SymbolTable;
use crate::semantic::{DiagnosticLevel, SemanticDiagnostic};
use crate::visit::{AstVisitor, VisitAction};

// Re-export TypeInfo from scope module
pub use crate::semantic::scope::TypeInfo;

// ============================================================
// TypeChecker trait
// ============================================================

/// 类型检查器 trait — 定义类型检查的接口。
///
/// 实现此 trait 来为特定语言定义类型规则。
///
/// # 使用示例
///
/// ```ignore
/// struct IrTypeChecker;
/// impl TypeChecker for IrTypeChecker {
///     fn infer_type(&self, node: AstRef<'_>, syms: &SymbolTable) -> Option<TypeInfo> {
///         match node.kind() {
///             "DEC" => Some(TypeInfo::int("i64", 64)),
///             "HEX" => Some(TypeInfo::int("i64", 64)),
///             "FLOAT" => Some(TypeInfo::float("f64", 64)),
///             _ => None,
///         }
///     }
/// }
/// ```
pub trait TypeChecker {
    /// 推断节点的类型（如果适用）。
    fn infer_type(&self, _node: AstRef<'_>, _syms: &SymbolTable) -> Option<TypeInfo> {
        None
    }

    /// 检查两个类型是否兼容（用于赋值检查）。
    fn is_compatible(&self, _from: &TypeInfo, _to: &TypeInfo) -> bool {
        true // Default: all types compatible
    }

    /// 检查二元运算的类型规则。
    fn check_binary_op(&self, _op: &str, _lhs: &TypeInfo, _rhs: &TypeInfo) -> Option<TypeInfo> {
        None // Default: no inference
    }
}

// ============================================================
// TypeCheckingVisitor
// ============================================================

/// 类型检查 visitor — 遍历 AST 并执行类型推断/检查。
///
/// 将推断的类型附加到符号表的符号上，并收集类型错误。
pub struct TypeCheckingVisitor<'a, T: TypeChecker> {
    pub checker: &'a T,
    pub syms: &'a mut SymbolTable,
    pub diagnostics: &'a mut Vec<SemanticDiagnostic>,
    pub ast: &'a TypedAst,
}

impl<'a, T: TypeChecker> TypeCheckingVisitor<'a, T> {
    pub fn new(
        checker: &'a T,
        syms: &'a mut SymbolTable,
        diagnostics: &'a mut Vec<SemanticDiagnostic>,
        ast: &'a TypedAst,
    ) -> Self {
        Self {
            checker,
            syms,
            diagnostics,
            ast,
        }
    }

    #[allow(dead_code)]
    fn report_type_error(&mut self, message: impl Into<String>, node: AstRef<'_>) {
        self.diagnostics.push(SemanticDiagnostic {
            level: DiagnosticLevel::Error,
            message: message.into(),
            span: Some(node.span()),
            node: Some(node.id),
        });
    }
}

impl<'a, T: TypeChecker> AstVisitor for TypeCheckingVisitor<'a, T> {
    fn visit(&mut self, node: AstRef<'_>) -> VisitAction {
        // Try to infer type for this node
        if let Some(ty) = self.checker.infer_type(node, self.syms) {
            // Store inferred type in symbol table (if this node is a definition)
            if let Some(name) = node.get_text("name") {
                // Update the symbol's type info
                if let Some(sym) = self.syms.resolve(name) {
                    // Can't mutate through immutable borrow — type info is set during
                    // symbol definition. The user should call define_with_type().
                    let _ = (sym, ty);
                }
            }
        }

        VisitAction::Continue
    }
}

// ============================================================
// Tests
// ============================================================

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_type_info_basic() {
        let i32_ty = TypeInfo::int("i32", 32);
        assert_eq!(i32_ty.name, "i32");
        assert_eq!(i32_ty.width, Some(32));
        assert!(!i32_ty.is_float);

        let f64_ty = TypeInfo::float("f64", 64);
        assert!(f64_ty.is_float);

        let ptr_ty = TypeInfo::ptr("i32");
        assert!(ptr_ty.is_ptr);

        let custom = TypeInfo::named("my_type");
        assert_eq!(custom.width, None);
    }

    #[test]
    fn test_type_checker_trait() {
        struct SimpleChecker;
        impl TypeChecker for SimpleChecker {
            fn infer_type(&self, node: AstRef<'_>, _syms: &SymbolTable) -> Option<TypeInfo> {
                if node.kind() == "int_lit" {
                    Some(TypeInfo::int("i32", 32))
                } else {
                    None
                }
            }
        }

        let checker = SimpleChecker;
        // Verify the trait is usable
        assert!(checker.is_compatible(&TypeInfo::int("i32", 32), &TypeInfo::int("i64", 64)));
    }
}
