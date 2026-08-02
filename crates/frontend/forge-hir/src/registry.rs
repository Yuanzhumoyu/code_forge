//! Brick Registry — global registry of all known atoms and composites.
//!
//! The registry is populated at program startup by `define_lowering!` macro
//! expansions. It provides lookup by [`OpTag`] and by name.

use crate::atom::{AtomSpec, OpTag};
use crate::composite::CompositeSpec;
use std::collections::HashMap;

/// Global registry of all known bricks (atoms and composites).
///
/// Populated at startup by `define_lowering!` macro expansions through
/// [`BrickRegistry::register_atom`] and [`BrickRegistry::register_composite`].
pub struct BrickRegistry {
    /// Atoms indexed by OpTag.
    atoms: HashMap<OpTag, AtomSpec>,
    /// Composites indexed by name.
    composites: HashMap<String, CompositeSpec>,
}

impl BrickRegistry {
    /// Create a new empty registry.
    pub fn new() -> Self {
        Self {
            atoms: HashMap::new(),
            composites: HashMap::new(),
        }
    }

    /// Register an atom specification.
    pub fn register_atom(&mut self, spec: AtomSpec) -> Result<(), String> {
        if self.atoms.contains_key(&spec.op) {
            return Err(format!("atom '{}' is already registered", spec.op));
        }
        self.atoms.insert(spec.op, spec);
        Ok(())
    }

    /// Register a composite specification.
    pub fn register_composite(&mut self, spec: CompositeSpec) -> Result<(), String> {
        if self.composites.contains_key(&spec.name) {
            return Err(format!("composite '{}' is already registered", spec.name));
        }
        self.composites.insert(spec.name.clone(), spec);
        Ok(())
    }

    /// Look up an atom by its operation tag.
    pub fn lookup_atom(&self, op: &OpTag) -> Option<&AtomSpec> {
        self.atoms.get(op)
    }

    /// Look up an atom by (dialect, name).
    pub fn lookup_atom_by_name(&self, dialect: &str, name: &str) -> Option<&AtomSpec> {
        let tag = OpTag::new(dialect, name);
        self.lookup_atom(&tag)
    }

    /// Look up a composite by name.
    pub fn lookup_composite(&self, name: &str) -> Option<&CompositeSpec> {
        self.composites.get(name)
    }

    /// Check if an atom is registered.
    pub fn has_atom(&self, op: &OpTag) -> bool {
        self.atoms.contains_key(op)
    }

    /// Check if a composite is registered.
    pub fn has_composite(&self, name: &str) -> bool {
        self.composites.contains_key(name)
    }

    /// Iterate over all registered atoms.
    pub fn iter_atoms(&self) -> impl Iterator<Item = &AtomSpec> {
        self.atoms.values()
    }

    /// Iterate over all registered composites.
    pub fn iter_composites(&self) -> impl Iterator<Item = &CompositeSpec> {
        self.composites.values()
    }

    /// Number of registered atoms.
    pub fn atom_count(&self) -> usize {
        self.atoms.len()
    }

    /// Number of registered composites.
    pub fn composite_count(&self) -> usize {
        self.composites.len()
    }
}

impl Default for BrickRegistry {
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
    use crate::atom::AtomSpec;
    use forge_ir::opcode::Opcode;

    #[test]
    fn test_register_and_lookup_atom() {
        let mut registry = BrickRegistry::new();

        let iadd_tag = crate::atom::arith::iadd();
        let spec = AtomSpec::new(iadd_tag, Opcode::Iadd)
            .input("lhs", forge_ir::TypeId::I32)
            .input("rhs", forge_ir::TypeId::I32)
            .output("result", forge_ir::TypeId::I32);

        registry.register_atom(spec).unwrap();
        assert!(registry.has_atom(&iadd_tag));

        let found = registry.lookup_atom(&iadd_tag).unwrap();
        assert_eq!(found.backend_op, Opcode::Iadd);
        assert_eq!(found.inputs.len(), 2);
    }

    #[test]
    fn test_register_composite() {
        let mut registry = BrickRegistry::new();
        let spec = CompositeSpec::new("IfElse").with_implicit_merge();
        registry.register_composite(spec).unwrap();
        assert!(registry.has_composite("IfElse"));
    }

    #[test]
    fn test_duplicate_atom_error() {
        let mut registry = BrickRegistry::new();
        let iadd_tag = crate::atom::arith::iadd();
        let spec = AtomSpec::new(iadd_tag, Opcode::Iadd);
        registry.register_atom(spec.clone()).unwrap();
        assert!(registry.register_atom(spec).is_err());
    }
}
