//! 立即数操作数 — 编译时已知的常量或实体引用。
//!
//! 与 `Value` 操作数分离，存储指令的非 SSA 值操作数:
//! 常量、块引用、函数引用、类型引用等。

use super::entity::*;
use crate::string_pool::InternedStr;

/// 非 Value 的操作数 — 编译时已知的常量或实体引用。
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Immediate {
    /// 有符号小整数 (直接嵌入指令，避免常量池查找)。
    Int(i64),
    /// 无符号小整数。
    Uint(u64),
    /// 常量池引用 (大整数、浮点常量等)。
    Const(ConstId),
    /// 基本块引用 (jump/branch 目标)。
    Block(Block),
    /// 函数引用 (call 的目标)。
    Func(FuncRef),
    /// 全局变量引用。
    Global(GlobalId),
    /// 类型引用 (用于 bitcast/sextend 等转换指令)。
    Type(TypeId),
    /// Interned 字符串。
    String(InternedStr),
}

impl Immediate {
    pub fn as_i64(&self) -> Option<i64> {
        match self {
            Immediate::Int(v) => Some(*v),
            _ => None,
        }
    }

    pub fn as_u64(&self) -> Option<u64> {
        match self {
            Immediate::Uint(v) => Some(*v),
            _ => None,
        }
    }

    pub fn as_block(&self) -> Option<Block> {
        match self {
            Immediate::Block(b) => Some(*b),
            _ => None,
        }
    }

    pub fn as_func(&self) -> Option<FuncRef> {
        match self {
            Immediate::Func(f) => Some(*f),
            _ => None,
        }
    }

    pub fn as_type(&self) -> Option<TypeId> {
        match self {
            Immediate::Type(t) => Some(*t),
            _ => None,
        }
    }

    pub fn as_const(&self) -> Option<ConstId> {
        match self {
            Immediate::Const(c) => Some(*c),
            _ => None,
        }
    }
}

impl From<i64> for Immediate {
    fn from(v: i64) -> Self {
        Immediate::Int(v)
    }
}

impl From<u64> for Immediate {
    fn from(v: u64) -> Self {
        Immediate::Uint(v)
    }
}

impl From<Block> for Immediate {
    fn from(b: Block) -> Self {
        Immediate::Block(b)
    }
}

impl From<FuncRef> for Immediate {
    fn from(f: FuncRef) -> Self {
        Immediate::Func(f)
    }
}

impl From<TypeId> for Immediate {
    fn from(t: TypeId) -> Self {
        Immediate::Type(t)
    }
}

impl From<ConstId> for Immediate {
    fn from(c: ConstId) -> Self {
        Immediate::Const(c)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_int_immediate() {
        let imm = Immediate::Int(42);
        assert_eq!(imm.as_i64(), Some(42));
        assert_eq!(imm.as_u64(), None);
        assert!(imm.as_block().is_none());
        assert!(imm.as_type().is_none());
    }

    #[test]
    fn test_uint_immediate() {
        let imm = Immediate::Uint(99);
        assert_eq!(imm.as_u64(), Some(99));
        assert_eq!(imm.as_i64(), None);
    }

    #[test]
    fn test_block_immediate() {
        let imm = Immediate::Block(Block(5));
        assert_eq!(imm.as_block(), Some(Block(5)));
        assert!(imm.as_i64().is_none());
    }

    #[test]
    fn test_type_immediate() {
        let imm = Immediate::Type(TypeId::I32);
        assert_eq!(imm.as_type(), Some(TypeId::I32));
    }

    #[test]
    fn test_from_i64() {
        let imm: Immediate = 42i64.into();
        assert_eq!(imm.as_i64(), Some(42));
    }

    #[test]
    fn test_from_block() {
        let imm: Immediate = Block(3).into();
        assert_eq!(imm.as_block(), Some(Block(3)));
    }

    #[test]
    fn test_from_type_id() {
        let imm: Immediate = TypeId::I8.into();
        assert_eq!(imm.as_type(), Some(TypeId::I8));
    }
}
