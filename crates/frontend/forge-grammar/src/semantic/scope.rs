//! 符号表 & 作用域管理。
//!
//! 提供层次化作用域栈，支持：
//! - 进入/离开作用域
//! - 定义符号（插入当前作用域）
//! - 解析符号（从内到外查找）
//! - 作用域 shadowing 检测

use crate::ast::AstId;
use std::collections::HashMap;

// ============================================================
// Symbol
// ============================================================

/// 符号表中的符号。
#[derive(Debug, Clone)]
pub struct Symbol {
    /// 符号名称。
    pub name: String,
    /// 符号类型。
    pub kind: SymbolKind,
    /// 定义此符号的 AST 节点。
    pub def_node: AstId,
    /// 可选的类型信息。
    pub ty: Option<TypeInfo>,
}

/// 符号种类。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SymbolKind {
    Variable,
    Function,
    Parameter,
    Label,
    Block,
    Type,
    Module,
    Unknown,
}

/// 类型信息（简约版，可扩展）。
#[derive(Debug, Clone)]
pub struct TypeInfo {
    /// 类型名称（如 "i32", "ptr", "void"）。
    pub name: String,
    /// 类型的位宽（如果适用）。
    pub width: Option<u16>,
    /// 是否为浮点类型。
    pub is_float: bool,
    /// 是否为指针类型。
    pub is_ptr: bool,
}

impl TypeInfo {
    pub fn named(name: impl Into<String>) -> Self {
        Self {
            name: name.into(),
            width: None,
            is_float: false,
            is_ptr: false,
        }
    }

    pub fn int(name: impl Into<String>, width: u16) -> Self {
        Self {
            name: name.into(),
            width: Some(width),
            is_float: false,
            is_ptr: false,
        }
    }

    pub fn float(name: impl Into<String>, width: u16) -> Self {
        Self {
            name: name.into(),
            width: Some(width),
            is_float: true,
            is_ptr: false,
        }
    }

    pub fn ptr(inner: impl Into<String>) -> Self {
        Self {
            name: format!("*{}", inner.into()),
            width: Some(64),
            is_float: false,
            is_ptr: true,
        }
    }
}

// ============================================================
// Scope
// ============================================================

/// 单个作用域。
#[derive(Debug, Clone)]
pub struct Scope {
    /// 作用域类型。
    pub kind: ScopeKind,
    /// 此作用域中定义的符号。
    pub symbols: HashMap<String, Symbol>,
    /// 父作用域索引（在 SymbolTable 的 scopes vec 中）。
    pub parent: Option<usize>,
}

/// 作用域类型。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ScopeKind {
    Global,
    Function,
    Block,
    Loop,
    /// 自定义作用域（用于 DSL 特定的结构）。
    Custom(&'static str),
}

impl Scope {
    pub fn new(kind: ScopeKind, parent: Option<usize>) -> Self {
        Self {
            kind,
            symbols: HashMap::new(),
            parent,
        }
    }

    /// 在此作用域中定义一个符号。
    pub fn define(&mut self, name: impl Into<String>, kind: SymbolKind, def_node: AstId) {
        let name = name.into();
        self.symbols.insert(
            name.clone(),
            Symbol {
                name,
                kind,
                def_node,
                ty: None,
            },
        );
    }

    /// 在此作用域中查找符号（不向上查找父作用域）。
    pub fn lookup_local(&self, name: &str) -> Option<&Symbol> {
        self.symbols.get(name)
    }
}

// ============================================================
// SymbolTable
// ============================================================

/// 层次化符号表 — 作用域栈。
#[derive(Debug, Clone)]
pub struct SymbolTable {
    /// 所有作用域（栈式管理）。
    scopes: Vec<Scope>,
    /// 当前作用域索引。
    current: Option<usize>,
}

impl SymbolTable {
    /// 创建空的符号表（无作用域）。
    pub fn new() -> Self {
        Self {
            scopes: Vec::new(),
            current: None,
        }
    }

    /// 进入一个新作用域。
    /// 返回新作用域的索引。
    pub fn enter(&mut self, kind: ScopeKind) -> usize {
        let parent = self.current;
        let scope = Scope::new(kind, parent);
        let idx = self.scopes.len();
        self.scopes.push(scope);
        self.current = Some(idx);
        idx
    }

    /// 离开当前作用域。
    /// 返回离开的作用域索引（如果还有父作用域，则回到父作用域）。
    pub fn leave(&mut self) -> Option<usize> {
        let current = self.current?;
        self.current = self.scopes[current].parent;
        Some(current)
    }

    /// 获取当前作用域索引。
    pub fn current(&self) -> Option<usize> {
        self.current
    }

    /// 获取指定索引的作用域。
    pub fn get(&self, idx: usize) -> Option<&Scope> {
        self.scopes.get(idx)
    }

    /// 在当前作用域中定义符号。
    pub fn define(&mut self, name: impl Into<String>, kind: SymbolKind, def_node: AstId) {
        if let Some(current) = self.current {
            self.scopes[current].define(name, kind, def_node);
        }
    }

    /// 在当前作用域中定义带类型信息的符号。
    pub fn define_with_type(
        &mut self,
        name: impl Into<String>,
        kind: SymbolKind,
        def_node: AstId,
        ty: TypeInfo,
    ) {
        if let Some(current) = self.current {
            let name = name.into();
            self.scopes[current].symbols.insert(
                name.clone(),
                Symbol {
                    name,
                    kind,
                    def_node,
                    ty: Some(ty),
                },
            );
        }
    }

    /// 从内到外解析名称 — 返回第一个匹配的符号。
    pub fn resolve(&self, name: &str) -> Option<&Symbol> {
        let mut current = self.current;
        while let Some(idx) = current {
            if let Some(sym) = self.scopes[idx].lookup_local(name) {
                return Some(sym);
            }
            current = self.scopes[idx].parent;
        }
        None
    }

    /// 解析名称 — 返回符号和定义所在的作用域索引。
    pub fn resolve_with_scope(&self, name: &str) -> Option<(&Symbol, usize)> {
        let mut current = self.current;
        while let Some(idx) = current {
            if let Some(sym) = self.scopes[idx].lookup_local(name) {
                return Some((sym, idx));
            }
            current = self.scopes[idx].parent;
        }
        None
    }

    /// 作用域数量。
    pub fn len(&self) -> usize {
        self.scopes.len()
    }

    /// 是否为空。
    pub fn is_empty(&self) -> bool {
        self.scopes.is_empty()
    }
}

impl Default for SymbolTable {
    fn default() -> Self {
        Self::new()
    }
}

// ============================================================
// Tests
// ============================================================

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ast::AstId;

    #[test]
    fn test_scope_define_and_lookup() {
        let mut scope = Scope::new(ScopeKind::Global, None);
        scope.define("x", SymbolKind::Variable, AstId(1));
        assert!(scope.lookup_local("x").is_some());
        assert!(scope.lookup_local("y").is_none());
    }

    #[test]
    fn test_symbol_table_nested_scopes() {
        let mut table = SymbolTable::new();

        // Global scope
        table.enter(ScopeKind::Global);
        table.define("x", SymbolKind::Variable, AstId(1));

        // Function scope
        table.enter(ScopeKind::Function);
        table.define("y", SymbolKind::Parameter, AstId(2));

        // Block scope
        table.enter(ScopeKind::Block);
        table.define("z", SymbolKind::Variable, AstId(3));

        // Resolve from innermost scope
        assert!(table.resolve("x").is_some()); // From global
        assert!(table.resolve("y").is_some()); // From function
        assert!(table.resolve("z").is_some()); // From block
        assert!(table.resolve("w").is_none()); // Undefined

        // Leave block
        table.leave();
        assert!(table.resolve("z").is_none()); // z is out of scope
        assert!(table.resolve("y").is_some()); // y still visible

        // Leave function
        table.leave();
        assert!(table.resolve("y").is_none()); // y is out of scope
        assert!(table.resolve("x").is_some()); // x still visible
    }

    #[test]
    fn test_symbol_table_shadowing() {
        let mut table = SymbolTable::new();

        table.enter(ScopeKind::Global);
        table.define("x", SymbolKind::Variable, AstId(1));

        table.enter(ScopeKind::Block);
        table.define("x", SymbolKind::Variable, AstId(2)); // Shadow global x

        // Resolve should return the innermost (shadowing) definition
        let sym = table.resolve("x").unwrap();
        assert_eq!(sym.def_node, AstId(2));

        table.leave();
        let sym = table.resolve("x").unwrap();
        assert_eq!(sym.def_node, AstId(1));
    }
}
