//! Operation tag and atom specification.
//!
//! An [`OpTag`] is a symbolic operation identifier = dialect + name.
//! An [`AtomSpec`] describes a single atomic operation — its input/output ports,
//! attributes, regions, and mapping to a backend `Opcode`.

use crate::attr::{AttrValue, Symbol, sym_intern};
use forge_ir::opcode::Opcode;
use forge_ir::{InternedStr, StringPool, TypeId};
use std::fmt;
use std::sync::{LazyLock, Mutex};

// ============================================================
// Global string pool for OpTag interning
// ============================================================

static GLOBAL_POOL: LazyLock<Mutex<StringPool>> = LazyLock::new(|| Mutex::new(StringPool::new()));

/// Intern a string in the global pool.
fn intern_str(s: &str) -> InternedStr {
    GLOBAL_POOL.lock().unwrap().intern(s)
}

/// Look up an interned string in the global pool.
pub fn global_lookup(id: InternedStr) -> String {
    GLOBAL_POOL.lock().unwrap().lookup(id).to_string()
}

// ============================================================
// OpTag
// ============================================================

/// A symbolic operation identifier = dialect + name.
///
/// Two `OpTag` values are equal iff both dialect and name are the same
/// interned string (pointer equality, O(1)).
#[derive(Clone, Copy, PartialEq, Eq, Hash)]
pub struct OpTag {
    pub dialect: InternedStr,
    pub name: InternedStr,
}

impl OpTag {
    /// Create a new OpTag, interning both strings in the global pool.
    pub fn new(dialect: &str, name: &str) -> Self {
        Self {
            dialect: intern_str(dialect),
            name: intern_str(name),
        }
    }

    /// Create from already-interned strings.
    pub fn from_interned(dialect: InternedStr, name: InternedStr) -> Self {
        Self { dialect, name }
    }
}

impl fmt::Display for OpTag {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let pool = GLOBAL_POOL.lock().unwrap();
        write!(
            f,
            "{}.{}",
            pool.lookup(self.dialect),
            pool.lookup(self.name)
        )
    }
}

impl fmt::Debug for OpTag {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let pool = GLOBAL_POOL.lock().unwrap();
        write!(
            f,
            "OpTag({}.{})",
            pool.lookup(self.dialect),
            pool.lookup(self.name)
        )
    }
}

// ============================================================
// Well-known OpTags — function-based for lazy interning
// ============================================================

/// Well-known operation tags for the "arith" dialect.
pub mod arith {
    use super::OpTag;

    pub fn iconst() -> OpTag {
        OpTag::new("arith", "iconst")
    }
    pub fn fconst() -> OpTag {
        OpTag::new("arith", "fconst")
    }
    pub fn iadd() -> OpTag {
        OpTag::new("arith", "iadd")
    }
    pub fn isub() -> OpTag {
        OpTag::new("arith", "isub")
    }
    pub fn imul() -> OpTag {
        OpTag::new("arith", "imul")
    }
    pub fn sdiv() -> OpTag {
        OpTag::new("arith", "sdiv")
    }
    pub fn udiv() -> OpTag {
        OpTag::new("arith", "udiv")
    }
    pub fn srem() -> OpTag {
        OpTag::new("arith", "srem")
    }
    pub fn urem() -> OpTag {
        OpTag::new("arith", "urem")
    }
    pub fn band() -> OpTag {
        OpTag::new("arith", "and")
    }
    pub fn bor() -> OpTag {
        OpTag::new("arith", "or")
    }
    pub fn bxor() -> OpTag {
        OpTag::new("arith", "xor")
    }
    pub fn bnot() -> OpTag {
        OpTag::new("arith", "not")
    }
    pub fn ishl() -> OpTag {
        OpTag::new("arith", "shl")
    }
    pub fn ushr() -> OpTag {
        OpTag::new("arith", "ushr")
    }
    pub fn sshr() -> OpTag {
        OpTag::new("arith", "sshr")
    }
    pub fn icmp() -> OpTag {
        OpTag::new("arith", "icmp")
    }
    pub fn fcmp() -> OpTag {
        OpTag::new("arith", "fcmp")
    }
    pub fn sextend() -> OpTag {
        OpTag::new("arith", "sextend")
    }
    pub fn uextend() -> OpTag {
        OpTag::new("arith", "uextend")
    }
    pub fn ireduce() -> OpTag {
        OpTag::new("arith", "ireduce")
    }
    pub fn bitcast() -> OpTag {
        OpTag::new("arith", "bitcast")
    }
    pub fn select() -> OpTag {
        OpTag::new("arith", "select")
    }
}

/// Well-known operation tags for the "cf" (control flow) dialect.
pub mod cf {
    use super::OpTag;

    pub fn branch() -> OpTag {
        OpTag::new("cf", "branch")
    }
    pub fn jump() -> OpTag {
        OpTag::new("cf", "jump")
    }
    pub fn ret() -> OpTag {
        OpTag::new("cf", "ret")
    }
    pub fn switch() -> OpTag {
        OpTag::new("cf", "switch")
    }
    pub fn unreachable() -> OpTag {
        OpTag::new("cf", "unreachable")
    }
}

/// Well-known operation tags for the "mem" (memory) dialect.
pub mod mem {
    use super::OpTag;

    pub fn load() -> OpTag {
        OpTag::new("mem", "load")
    }
    pub fn store() -> OpTag {
        OpTag::new("mem", "store")
    }
    pub fn stack_addr() -> OpTag {
        OpTag::new("mem", "stack_addr")
    }
    pub fn global_addr() -> OpTag {
        OpTag::new("mem", "global_addr")
    }
    pub fn alloca() -> OpTag {
        OpTag::new("mem", "alloca")
    }
    pub fn gep() -> OpTag {
        OpTag::new("mem", "gep")
    }
}

// ============================================================
// Port and Region specifications
// ============================================================

/// The kind of a port on a brick.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum PortKind {
    ValueIn,
    ValueOut,
    Region,
    BlockRef,
}

/// Specification for a single port on a brick.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PortSpec {
    pub name: Symbol,
    pub kind: PortKind,
    pub ty: Option<TypeId>,
}

impl PortSpec {
    pub fn value_in(name: &str, ty: TypeId) -> Self {
        Self {
            name: sym_intern(name),
            kind: PortKind::ValueIn,
            ty: Some(ty),
        }
    }
    pub fn value_out(name: &str, ty: TypeId) -> Self {
        Self {
            name: sym_intern(name),
            kind: PortKind::ValueOut,
            ty: Some(ty),
        }
    }
    pub fn region(name: &str) -> Self {
        Self {
            name: sym_intern(name),
            kind: PortKind::Region,
            ty: None,
        }
    }
    pub fn block_ref(name: &str) -> Self {
        Self {
            name: sym_intern(name),
            kind: PortKind::BlockRef,
            ty: None,
        }
    }
}

/// Specification for a region on a composite brick.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RegionSpec {
    pub name: Symbol,
    pub optional: bool,
}

impl RegionSpec {
    pub fn required(name: &str) -> Self {
        Self {
            name: sym_intern(name),
            optional: false,
        }
    }
    pub fn optional(name: &str) -> Self {
        Self {
            name: sym_intern(name),
            optional: true,
        }
    }
}

/// Specification for an attribute on a brick.
#[derive(Clone, Debug, PartialEq)]
pub struct AttrSpec {
    pub name: Symbol,
    pub kind: AttrKind,
    pub default: Option<AttrValue>,
}

/// The kind of an attribute.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum AttrKind {
    Int,
    Uint,
    Float,
    Str,
    Type,
    Block,
    Func,
    Any,
}

impl AttrSpec {
    pub fn int(name: &str) -> Self {
        Self {
            name: sym_intern(name),
            kind: AttrKind::Int,
            default: None,
        }
    }
    pub fn int_default(name: &str, default: i64) -> Self {
        Self {
            name: sym_intern(name),
            kind: AttrKind::Int,
            default: Some(AttrValue::Int(default)),
        }
    }
    pub fn str(name: &str) -> Self {
        Self {
            name: sym_intern(name),
            kind: AttrKind::Str,
            default: None,
        }
    }
    pub fn ty(name: &str) -> Self {
        Self {
            name: sym_intern(name),
            kind: AttrKind::Type,
            default: None,
        }
    }
}

// ============================================================
// AtomSpec
// ============================================================

/// Metadata for a single atomic operation.
#[derive(Clone, Debug)]
pub struct AtomSpec {
    pub op: OpTag,
    pub inputs: Vec<PortSpec>,
    pub outputs: Vec<PortSpec>,
    pub attrs: Vec<AttrSpec>,
    pub regions: Vec<RegionSpec>,
    pub backend_op: Opcode,
}

impl AtomSpec {
    pub fn new(op: OpTag, backend_op: Opcode) -> Self {
        Self {
            op,
            inputs: vec![],
            outputs: vec![],
            attrs: vec![],
            regions: vec![],
            backend_op,
        }
    }
    pub fn input(mut self, name: &str, ty: TypeId) -> Self {
        self.inputs.push(PortSpec::value_in(name, ty));
        self
    }
    pub fn output(mut self, name: &str, ty: TypeId) -> Self {
        self.outputs.push(PortSpec::value_out(name, ty));
        self
    }
    pub fn attr_int(mut self, name: &str) -> Self {
        self.attrs.push(AttrSpec::int(name));
        self
    }
    pub fn attr_int_default(mut self, name: &str, default: i64) -> Self {
        self.attrs.push(AttrSpec::int_default(name, default));
        self
    }
    pub fn attr_str(mut self, name: &str) -> Self {
        self.attrs.push(AttrSpec::str(name));
        self
    }
    pub fn attr_ty(mut self, name: &str) -> Self {
        self.attrs.push(AttrSpec::ty(name));
        self
    }
    pub fn region(mut self, name: &str) -> Self {
        self.regions.push(RegionSpec::required(name));
        self
    }
    pub fn region_optional(mut self, name: &str) -> Self {
        self.regions.push(RegionSpec::optional(name));
        self
    }
}
