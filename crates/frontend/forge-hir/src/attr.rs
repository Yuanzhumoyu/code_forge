//! Attribute value type.
//!
//! Each node in the IR graph can carry arbitrary key-value attributes.
//! Attributes are compile-time constants (not SSA values) that parameterize
//! the behavior of an operation.

use forge_ir::{Block, FuncRef, StringPool, TypeId};
use std::collections::HashMap;
use std::fmt;
use std::sync::{LazyLock, Mutex};

/// Interned symbol — used as attribute keys and port names.
pub type Symbol = forge_ir::InternedStr;

/// Global string pool for interning symbols.
static SYM_POOL: LazyLock<Mutex<StringPool>> = LazyLock::new(|| Mutex::new(StringPool::new()));

/// Intern a string as a Symbol using the global pool.
pub fn sym_intern(s: &str) -> Symbol {
    SYM_POOL.lock().unwrap().intern(s)
}

/// Look up a Symbol in the global pool.
pub fn sym_lookup(sym: Symbol) -> String {
    SYM_POOL.lock().unwrap().lookup(sym).to_string()
}

/// A compile-time attribute value for an IR node.
///
/// Unlike SSA `Value`s which represent runtime data, `AttrValue`s are
/// compile-time constants that influence how an operation behaves.
#[derive(Clone, Debug, PartialEq)]
pub enum AttrValue {
    /// Signed integer constant.
    Int(i64),
    /// Unsigned integer constant.
    Uint(u64),
    /// Floating-point constant.
    Float(f64),
    /// String / interned symbol.
    Str(Symbol),
    /// IR type reference.
    Type(TypeId),
    /// Block reference in the *ir graph* (for terminators / control flow).
    ///
    /// Lowering resolves it to a forge_ir block via the `block_map`; it is
    /// NOT a forge_ir handle, so it cannot accidentally alias builder blocks.
    BlockId(crate::block::BlockId),
    /// Legacy forge_ir block handle (kept for compatibility).
    Block(Block),
    /// Function reference (for call operations).
    Func(FuncRef),
    /// Nested array of attributes.
    Array(Vec<AttrValue>),
    /// Nested dictionary of attributes.
    Dict(HashMap<Symbol, AttrValue>),
}

impl AttrValue {
    /// Create an integer attribute.
    pub fn int(v: i64) -> Self {
        AttrValue::Int(v)
    }

    /// Create an unsigned integer attribute.
    pub fn uint(v: u64) -> Self {
        AttrValue::Uint(v)
    }

    /// Create a string/symbol attribute from an interned string.
    pub fn sym(s: Symbol) -> Self {
        AttrValue::Str(s)
    }

    /// Create a string/symbol attribute from a &str.
    pub fn str(s: &str) -> Self {
        AttrValue::Str(sym_intern(s))
    }

    /// Create a type attribute.
    pub fn ty(t: TypeId) -> Self {
        AttrValue::Type(t)
    }

    /// Create a block attribute.
    pub fn block(b: Block) -> Self {
        AttrValue::Block(b)
    }

    /// Create a function attribute.
    pub fn func(f: FuncRef) -> Self {
        AttrValue::Func(f)
    }

    /// Try to get as i64.
    pub fn as_int(&self) -> Option<i64> {
        match self {
            AttrValue::Int(v) => Some(*v),
            _ => None,
        }
    }

    /// Try to get as u64.
    pub fn as_uint(&self) -> Option<u64> {
        match self {
            AttrValue::Uint(v) => Some(*v),
            _ => None,
        }
    }

    /// Try to get as symbol.
    pub fn as_str(&self) -> Option<Symbol> {
        match self {
            AttrValue::Str(s) => Some(*s),
            _ => None,
        }
    }

    /// Try to get as TypeId.
    pub fn as_type(&self) -> Option<TypeId> {
        match self {
            AttrValue::Type(t) => Some(*t),
            _ => None,
        }
    }

    /// Try to get as a graph-internal BlockId.
    pub fn as_block_id(&self) -> Option<crate::block::BlockId> {
        match self {
            AttrValue::BlockId(b) => Some(*b),
            _ => None,
        }
    }

    /// Try to get as a forge_ir Block (legacy).
    pub fn as_block(&self) -> Option<Block> {
        match self {
            AttrValue::Block(b) => Some(*b),
            _ => None,
        }
    }

    /// Try to get as FuncRef.
    pub fn as_func(&self) -> Option<FuncRef> {
        match self {
            AttrValue::Func(f) => Some(*f),
            _ => None,
        }
    }
}

impl fmt::Display for AttrValue {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            AttrValue::Int(v) => write!(f, "{}", v),
            AttrValue::Uint(v) => write!(f, "{}", v),
            AttrValue::Float(v) => write!(f, "{}", v),
            AttrValue::Str(s) => write!(f, "{}", s),
            AttrValue::Type(t) => write!(f, "{:?}", t),
            AttrValue::BlockId(b) => write!(f, "block_id({})", b.0),
            AttrValue::Block(b) => write!(f, "block({})", b.0),
            AttrValue::Func(fr) => write!(f, "func({})", fr.0),
            AttrValue::Array(arr) => {
                write!(f, "[")?;
                for (i, v) in arr.iter().enumerate() {
                    if i > 0 {
                        write!(f, ", ")?;
                    }
                    write!(f, "{}", v)?;
                }
                write!(f, "]")
            }
            AttrValue::Dict(d) => {
                write!(f, "{{")?;
                for (i, (k, v)) in d.iter().enumerate() {
                    if i > 0 {
                        write!(f, ", ")?;
                    }
                    write!(f, "{}: {}", k, v)?;
                }
                write!(f, "}}")
            }
        }
    }
}
