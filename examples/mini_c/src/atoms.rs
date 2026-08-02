//! Mini C atom definitions using the forge-hir brick framework.
//!
//! This module uses `define_lowering!` to register all Mini C atomic operations.
//! The generated `minic_lowering` module provides:
//! - OpTag functions for each atom (`xxx_tag()`)
//! - Type-safe constructors for each atom (`build_xxx()`)
//! - `register_atoms()` for BrickRegistry initialization
//!
//! AST→IR lowering is written by hand in `codegen_hir.rs`.

use forge_hir::define_lowering;

define_lowering! {
    language MiniC;

    // ── Arithmetic ──
    atom iconst {
        attrs { value: i32 }
        outputs { result: i32 }
        maps_to arith.iconst;
    }

    atom iadd {
        inputs { lhs: i32, rhs: i32 }
        outputs { result: i32 }
        maps_to arith.iadd;
    }

    atom isub {
        inputs { lhs: i32, rhs: i32 }
        outputs { result: i32 }
        maps_to arith.isub;
    }

    atom imul {
        inputs { lhs: i32, rhs: i32 }
        outputs { result: i32 }
        maps_to arith.imul;
    }

    atom sdiv {
        inputs { lhs: i32, rhs: i32 }
        outputs { result: i32 }
        maps_to arith.sdiv;
    }

    atom srem {
        inputs { lhs: i32, rhs: i32 }
        outputs { result: i32 }
        maps_to arith.srem;
    }

    // ── Bitwise ──
    atom band {
        inputs { lhs: i32, rhs: i32 }
        outputs { result: i32 }
        maps_to arith.and;
    }

    atom bor {
        inputs { lhs: i32, rhs: i32 }
        outputs { result: i32 }
        maps_to arith.or;
    }

    atom bxor {
        inputs { lhs: i32, rhs: i32 }
        outputs { result: i32 }
        maps_to arith.xor;
    }

    atom bnot {
        inputs { val: i32 }
        outputs { result: i32 }
        maps_to arith.not;
    }

    atom ishl {
        inputs { lhs: i32, rhs: i32 }
        outputs { result: i32 }
        maps_to arith.shl;
    }

    atom sshr {
        inputs { lhs: i32, rhs: i32 }
        outputs { result: i32 }
        maps_to arith.sshr;
    }

    // ── Comparison ──
    atom icmp {
        attrs { cond: IntCC }
        inputs { lhs: i32, rhs: i32 }
        outputs { result: i1 }
        maps_to arith.icmp;
    }

    // ── Type conversion ──
    atom sextend {
        attrs { to: Type }
        inputs { val: value }
        outputs { result: value }
        maps_to arith.sextend;
    }

    // ── Memory ──
    atom load {
        attrs { ty: Type }
        inputs { addr: ptr }
        outputs { result: value }
        maps_to mem.load;
    }

    atom store {
        inputs { value: value, addr: ptr }
        maps_to mem.store;
    }

    atom stack_addr {
        attrs { offset: i32 }
        outputs { result: ptr }
        maps_to mem.stack_addr;
    }

    // ── Control flow ──
    atom branch {
        inputs { cond: i1 }
        regions { then_body, else_body }
        maps_to cf.branch;
    }

    atom jump {
        inputs { target: Block }
        maps_to cf.jump;
    }

    atom ret {
        inputs { value: i32 }
        maps_to cf.ret;
    }
}

// Re-export the generated module
pub use minic_lowering::*;

#[cfg(test)]
mod tests {
    use super::*;
    use forge_hir::BrickRegistry;

    #[test]
    fn test_mini_c_atoms_registered() {
        let mut registry = BrickRegistry::new();
        minic_lowering::register_atoms(&mut registry);
        assert_eq!(registry.atom_count(), 20);

        // Verify key atoms
        assert!(registry.has_atom(&minic_lowering::iadd_tag()));
        assert!(registry.has_atom(&minic_lowering::iconst_tag()));
        assert!(registry.has_atom(&minic_lowering::ret_tag()));
        assert!(registry.has_atom(&minic_lowering::branch_tag()));
    }

    #[test]
    fn test_atom_specs() {
        let mut registry = BrickRegistry::new();
        minic_lowering::register_atoms(&mut registry);

        let iadd = registry.lookup_atom(&minic_lowering::iadd_tag()).unwrap();
        assert_eq!(iadd.inputs.len(), 2);
        assert_eq!(iadd.outputs.len(), 1);

        let branch = registry.lookup_atom(&minic_lowering::branch_tag()).unwrap();
        assert_eq!(branch.inputs.len(), 1);
        assert_eq!(branch.regions.len(), 2);
    }

    #[test]
    fn test_generated_build_iconst() {
        // The macro must generate a type-safe constructor usable in lowering.
        use forge_hir::IrGraph;

        let mut graph = IrGraph::new();
        graph.create_block(&[]);
        let v = minic_lowering::build_iconst(&mut graph, 42).unwrap();
        assert_eq!(graph.value_type(v), Some(forge_hir::ir::TypeId::I32));
    }

    #[test]
    fn test_generated_build_icmp_all_ccs() {
        use forge_hir::IrGraph;

        let mut graph = IrGraph::new();
        graph.create_block(&[]);
        let lhs = minic_lowering::build_iconst(&mut graph, 1).unwrap();
        let rhs = minic_lowering::build_iconst(&mut graph, 2).unwrap();
        for cc in [
            forge_hir::ir::IntCC::Equal,
            forge_hir::ir::IntCC::NotEqual,
            forge_hir::ir::IntCC::UnsignedLessThan,
            forge_hir::ir::IntCC::UnsignedGreaterThanOrEqual,
        ] {
            let r = minic_lowering::build_icmp(&mut graph, cc, lhs, rhs).unwrap();
            assert_eq!(graph.value_type(r), Some(forge_hir::ir::TypeId::BOOL));
        }
    }

    #[test]
    fn test_generated_build_load_uses_ty() {
        use forge_hir::IrGraph;

        let mut graph = IrGraph::new();
        graph.create_block(&[]);
        let addr = minic_lowering::build_stack_addr(&mut graph, 0).unwrap();
        let v = minic_lowering::build_load(&mut graph, forge_hir::ir::TypeId::I32, addr).unwrap();
        assert_eq!(graph.value_type(v), Some(forge_hir::ir::TypeId::I32));
    }
}
