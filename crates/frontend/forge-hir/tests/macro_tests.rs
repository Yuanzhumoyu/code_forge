//! Integration tests for the `define_lowering!` macro.
//!
//! Macro invocations are at module level (not inside test functions)
//! because proc-macros generate code at the call site.

use forge_hir::*;

// ── Macro invocations at module level ──

define_lowering! {
    language TestSingleLang;

    atom iconst {
        attrs { value: i32 }
        outputs { result: i32 }
        maps_to arith.iconst;
    }
}

define_lowering! {
    language TestMultiLang;

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

    atom ret {
        inputs { value: i32 }
        maps_to cf.ret;
    }
}

define_lowering! {
    language TestRegionsLang;

    atom branch {
        inputs { cond: i1 }
        regions { then_body, else_body }
        maps_to cf.branch;
    }
}

define_lowering! {
    language TestMemLang;

    atom load {
        inputs { addr: ptr, ty: Type }
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
}

// ── Tests ──

#[test]
fn test_single_atom() {
    let mut registry = BrickRegistry::new();
    testsinglelang_lowering::register_atoms(&mut registry);

    let tag = testsinglelang_lowering::iconst_tag();
    assert!(registry.has_atom(&tag));

    let spec = registry.lookup_atom(&tag).unwrap();
    assert_eq!(spec.outputs.len(), 1);
    assert_eq!(spec.attrs.len(), 1);
}

#[test]
fn test_multiple_atoms() {
    let mut registry = BrickRegistry::new();
    testmultilang_lowering::register_atoms(&mut registry);
    assert_eq!(registry.atom_count(), 3);
}

#[test]
fn test_atom_with_regions() {
    let mut registry = BrickRegistry::new();
    testregionslang_lowering::register_atoms(&mut registry);

    let tag = testregionslang_lowering::branch_tag();
    let spec = registry.lookup_atom(&tag).unwrap();
    assert_eq!(spec.inputs.len(), 1);
    assert_eq!(spec.regions.len(), 2);
}

#[test]
fn test_memory_atoms() {
    let mut registry = BrickRegistry::new();
    testmemlang_lowering::register_atoms(&mut registry);
    assert_eq!(registry.atom_count(), 3);
}

// ── End-to-end lowering tests ──

#[test]
fn test_e2e_simple_expression() {
    // Register atoms from the macro-generated module
    let mut registry = BrickRegistry::new();
    testmultilang_lowering::register_atoms(&mut registry);
    assert_eq!(registry.atom_count(), 3);

    // Build IrGraph for: iconst(42) ; iconst(x) is our input ; iadd(x, 42) ; ret(result)
    // In this test we simulate a simple expression: 1 + 42 = 43 and return it
    let mut graph = IrGraph::new();
    let entry = graph.create_block(&[
        (forge_ir::TypeId::I32, "x"), // block param simulating function parameter
    ]);

    // Get the block param as input value
    let param_gv = graph.block_params(entry).unwrap()[0];

    // iconst(42)
    let mut attrs = std::collections::HashMap::new();
    attrs.insert(
        forge_hir::attr::sym_intern("value"),
        forge_hir::AttrValue::Int(42),
    );
    let n_c = graph
        .create_node(
            testmultilang_lowering::iconst_tag(),
            &[],
            attrs,
            &[forge_ir::TypeId::I32],
        )
        .unwrap();
    graph.append_to_block(entry, n_c).unwrap();
    let v_const = graph.node_results(n_c)[0];

    // iadd(param, const)
    let n_add = graph
        .create_node(
            testmultilang_lowering::iadd_tag(),
            &[param_gv, v_const],
            std::collections::HashMap::new(),
            &[forge_ir::TypeId::I32],
        )
        .unwrap();
    graph.append_to_block(entry, n_add).unwrap();
    let v_result = graph.node_results(n_add)[0];

    // ret(result)
    let n_ret = graph
        .create_node(
            testmultilang_lowering::ret_tag(),
            &[v_result],
            std::collections::HashMap::new(),
            &[],
        )
        .unwrap();
    graph.set_terminator(entry, n_ret).unwrap();

    // Lower the graph
    let ctx = forge_ir::TypeContext::new();
    let sig =
        forge_ir::FunctionSignature::new(&[(forge_ir::TypeId::I32, "x")], &[forge_ir::TypeId::I32]);
    let mut builder = forge_ir::FunctionBuilder::new("add_42", ctx, sig);

    // The function has a parameter x (i32) — create entry block to get it
    let (_entry_block, _entry_params) = builder.create_entry_block();

    let mut lowering = LoweringContext::new(&mut builder, &registry);
    lowering.lower_graph(&graph).unwrap();

    // Map the block param: the IrGraph block param "x" maps to function param entry_params[0]
    // (This mapping is handled by the user of the framework; LoweringContext only
    //  maps block params within the IrGraph, not external function params)
    let func = builder.finish();

    // Verify the function exists and has the right structure
    assert_eq!(func.name, "add_42");
    assert_eq!(func.param_tys.len(), 1);
    assert_eq!(func.param_tys[0], forge_ir::TypeId::I32);
}

#[test]
fn test_e2e_if_else() {
    // Register atoms with explicit backend ops
    let mut registry = BrickRegistry::new();
    testmultilang_lowering::register_atoms(&mut registry);

    // Also need icmp and branch — register programmatically
    registry
        .register_atom(
            forge_hir::atom::AtomSpec::new(
                forge_hir::atom::arith::icmp(),
                forge_ir::opcode::Opcode::Icmp {
                    cond: forge_ir::opcode::IntCC::Equal,
                },
            )
            .attr_str("cond")
            .input("lhs", forge_ir::TypeId::I32)
            .input("rhs", forge_ir::TypeId::I32)
            .output("result", forge_ir::TypeId::BOOL)
            .clone(),
        )
        .unwrap();
    registry
        .register_atom(
            forge_hir::atom::AtomSpec::new(
                forge_hir::atom::cf::branch(),
                forge_ir::opcode::Opcode::Nop,
            )
            .input("cond", forge_ir::TypeId::BOOL)
            .clone(),
        )
        .unwrap();
    registry
        .register_atom(
            forge_hir::atom::AtomSpec::new(
                forge_hir::atom::cf::jump(),
                forge_ir::opcode::Opcode::Nop,
            )
            .clone(),
        )
        .unwrap();

    // Build IrGraph for: if (1 != 0) { return 42; } else { return 0; }
    let mut graph = IrGraph::new();
    let entry = graph.create_block(&[]);
    let then_blk = graph.create_block(&[]);
    let else_blk = graph.create_block(&[]);
    let merge_blk = graph.create_block(&[]);

    // --- entry: iconst(1) ; iconst(0) ; icmp(1, 0, ne) ; branch ---
    graph.set_current_block(entry);

    let mut attrs = std::collections::HashMap::new();
    attrs.insert(
        forge_hir::attr::sym_intern("value"),
        forge_hir::AttrValue::Int(1),
    );
    let n1 = graph
        .create_node(
            testmultilang_lowering::iconst_tag(),
            &[],
            attrs,
            &[forge_ir::TypeId::I32],
        )
        .unwrap();
    graph.append_to_block(entry, n1).unwrap();
    let v1 = graph.node_results(n1)[0];

    let mut attrs = std::collections::HashMap::new();
    attrs.insert(
        forge_hir::attr::sym_intern("value"),
        forge_hir::AttrValue::Int(0),
    );
    let n0 = graph
        .create_node(
            testmultilang_lowering::iconst_tag(),
            &[],
            attrs,
            &[forge_ir::TypeId::I32],
        )
        .unwrap();
    graph.append_to_block(entry, n0).unwrap();
    let v0 = graph.node_results(n0)[0];

    let mut cmp_attrs = std::collections::HashMap::new();
    cmp_attrs.insert(
        forge_hir::attr::sym_intern("cond"),
        forge_hir::AttrValue::str("NotEqual"),
    );
    let nc = graph
        .create_node(
            forge_hir::atom::arith::icmp(),
            &[v1, v0],
            cmp_attrs,
            &[forge_ir::TypeId::BOOL],
        )
        .unwrap();
    graph.append_to_block(entry, nc).unwrap();
    let vc = graph.node_results(nc)[0];

    let mut branch_attrs = std::collections::HashMap::new();
    branch_attrs.insert(
        forge_hir::attr::sym_intern("then_block"),
        forge_hir::AttrValue::Block(forge_ir::Block(then_blk.0)),
    );
    branch_attrs.insert(
        forge_hir::attr::sym_intern("else_block"),
        forge_hir::AttrValue::Block(forge_ir::Block(else_blk.0)),
    );
    let nb = graph
        .create_node(forge_hir::atom::cf::branch(), &[vc], branch_attrs, &[])
        .unwrap();
    graph.set_terminator(entry, nb).unwrap();

    // --- then: iconst(42) ; ret ---
    let mut attrs = std::collections::HashMap::new();
    attrs.insert(
        forge_hir::attr::sym_intern("value"),
        forge_hir::AttrValue::Int(42),
    );
    let nt = graph
        .create_node(
            testmultilang_lowering::iconst_tag(),
            &[],
            attrs,
            &[forge_ir::TypeId::I32],
        )
        .unwrap();
    graph.append_to_block(then_blk, nt).unwrap();
    let vt = graph.node_results(nt)[0];
    let ntr = graph
        .create_node(
            testmultilang_lowering::ret_tag(),
            &[vt],
            std::collections::HashMap::new(),
            &[],
        )
        .unwrap();
    graph.set_terminator(then_blk, ntr).unwrap();

    // --- else: iconst(0) ; ret ---
    let mut attrs = std::collections::HashMap::new();
    attrs.insert(
        forge_hir::attr::sym_intern("value"),
        forge_hir::AttrValue::Int(0),
    );
    let ne = graph
        .create_node(
            testmultilang_lowering::iconst_tag(),
            &[],
            attrs,
            &[forge_ir::TypeId::I32],
        )
        .unwrap();
    graph.append_to_block(else_blk, ne).unwrap();
    let ve = graph.node_results(ne)[0];
    let ner = graph
        .create_node(
            testmultilang_lowering::ret_tag(),
            &[ve],
            std::collections::HashMap::new(),
            &[],
        )
        .unwrap();
    graph.set_terminator(else_blk, ner).unwrap();

    // --- merge: unreachable (shouldn't be reached in this simple test) ---
    let mut attrs = std::collections::HashMap::new();
    attrs.insert(
        forge_hir::attr::sym_intern("value"),
        forge_hir::AttrValue::Int(0),
    );
    let nm = graph
        .create_node(
            testmultilang_lowering::iconst_tag(),
            &[],
            attrs,
            &[forge_ir::TypeId::I32],
        )
        .unwrap();
    graph.append_to_block(merge_blk, nm).unwrap();
    let vm = graph.node_results(nm)[0];
    let nmr = graph
        .create_node(
            testmultilang_lowering::ret_tag(),
            &[vm],
            std::collections::HashMap::new(),
            &[],
        )
        .unwrap();
    graph.set_terminator(merge_blk, nmr).unwrap();

    // Lower
    let ctx = forge_ir::TypeContext::new();
    let sig = forge_ir::FunctionSignature::new(&[], &[forge_ir::TypeId::I32]);
    let mut builder = forge_ir::FunctionBuilder::new("test_if", ctx, sig);
    builder.create_entry_block();

    let mut lowering = LoweringContext::new(&mut builder, &registry);
    lowering.lower_graph(&graph).unwrap();

    let func = builder.finish();
    assert_eq!(func.name, "test_if");
    // Should have 4 blocks
    assert!(func.dfg.block_count() >= 4);
}
