//! Compile-time constant value type shared across optimization passes.
//!
//! `ConstValue` represents a value known at compile time, used by:
//! - `scalar::const_fold` — constant folding
//! - `scalar::sccp` — sparse conditional constant propagation
//! - `ipa::func_specialize` — function specialization
//! - `support::interpreter` — IR interpreter

use forge_ir::Big;
use forge_ir::TypeId;

/// A compile-time known value.
///
/// Preserves type information for correct truncation, extension, and bit-width
/// handling. For example `Int(0xFF, I8)` and `Int(0xFF, I32)` are different constants.
#[derive(Clone, Debug, PartialEq)]
pub enum ConstValue {
    /// Integer value (arbitrary precision) + type.
    Int(Big, TypeId),
    /// Floating-point value (arbitrary precision) + type.
    Float(Big, TypeId),
    /// Boolean value (result of integer comparison).
    Bool(bool),
}

impl ConstValue {
    /// If this value is an integer, return its value and type.
    pub fn as_int(&self) -> Option<(&Big, TypeId)> {
        match self {
            ConstValue::Int(v, t) => Some((v, *t)),
            _ => None,
        }
    }

    /// If this value is a boolean, return it.
    pub fn as_bool(&self) -> Option<bool> {
        match self {
            ConstValue::Bool(b) => Some(*b),
            _ => None,
        }
    }

    /// If this value is a float, return its value and type.
    pub fn as_float(&self) -> Option<(&Big, TypeId)> {
        match self {
            ConstValue::Float(v, t) => Some((v, *t)),
            _ => None,
        }
    }

    /// Convert to boolean (for branch condition evaluation).
    /// Integer 0 = false, non-zero = true.
    pub fn to_bool(&self) -> Option<bool> {
        match self {
            ConstValue::Bool(b) => Some(*b),
            ConstValue::Int(v, _) => Some(!v.is_zero()),
            ConstValue::Float(v, _) => Some(!v.is_zero()),
        }
    }
}
