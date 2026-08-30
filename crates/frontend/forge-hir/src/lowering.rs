//! Graph lowering — translates IrGraph nodes into forge-ir instructions.
//!
//! `LoweringContext` bridges the gap between the declarative [`IrGraph`] (which stores
//! symbolic [`OpTag`](crate::atom::OpTag) operations with [`AttrValue`] attributes) and the imperative
//! [`FunctionBuilder`] (which emits concrete `ins_iadd`/`ins_iconst`/etc. instructions).
//!
//! # How it works
//!
//! 1. All [`BlockId`]s from the IrGraph are created in the FunctionBuilder
//! 2. Blocks are processed in order, each node looked up in [`BrickRegistry`]
//! 3. [`GraphValue`] handles are mapped to [`forge_ir::Value`] via a value map
//! 4. Node attributes (stored as [`AttrValue`]) are extracted and converted to
//!    the concrete types expected by FunctionBuilder (i64→i32, Symbol→IntCC, etc.)
//! 5. Control-flow terminators are recognized by their OpTag (cf.branch/cf.jump/cf.ret)
//!    and emit the corresponding FunctionBuilder terminator methods

use crate::atom::cf;
use crate::attr::{AttrValue, Symbol, sym_intern};
use crate::block::BlockId;
use crate::error::HirError;
use crate::graph::{GraphValue, IrGraph, NodeId};
use crate::registry::BrickRegistry;
use forge_ir::opcode::{IntCC, Opcode};
use forge_ir::{Block, FunctionBuilder, TypeId, Value};
use std::collections::HashMap;

// ============================================================
// LoweringContext
// ============================================================

/// Bridges IrGraph → FunctionBuilder.
///
/// Walks every block in the IrGraph, maps each node's [`OpTag`](crate::atom::OpTag) to a concrete
/// [`Opcode`] via the [`BrickRegistry`], extracts attributes and operands, and
/// emits the corresponding [`FunctionBuilder`] instruction.
pub struct LoweringContext<'a> {
    builder: &'a mut FunctionBuilder,
    registry: &'a BrickRegistry,
    /// BlockId → forge_ir::Block
    block_map: HashMap<BlockId, Block>,
    /// GraphValue → forge_ir::Value
    value_map: HashMap<GraphValue, Value>,
}

impl<'a> LoweringContext<'a> {
    pub fn new(builder: &'a mut FunctionBuilder, registry: &'a BrickRegistry) -> Self {
        Self {
            builder,
            registry,
            block_map: HashMap::new(),
            value_map: HashMap::new(),
        }
    }

    /// Lower an entire IrGraph into the FunctionBuilder.
    ///
    /// This creates all blocks first, then processes each block's nodes in order.
    /// The first block in the graph becomes the entry block; control flow from
    /// there is driven by terminators (branch/jump/ret) in each block.
    pub fn lower_graph(&mut self, graph: &IrGraph) -> Result<(), HirError> {
        // Phase 1: create all blocks in FunctionBuilder, build the map
        self.create_blocks(graph)?;
        self.lower_blocks_internal(graph)
    }

    /// Like `lower_graph`, but maps IrGraph block 0 to an already-created entry block.
    pub fn lower_graph_with_entry(
        &mut self,
        graph: &IrGraph,
        entry_block: Block,
    ) -> Result<(), HirError> {
        self.lower_graph_with_entry_and_params(graph, entry_block, &[])
    }

    /// Like `lower_graph_with_entry`, but also maps the graph's entry-block
    /// parameters to the forge_ir entry-block parameters (function arguments).
    pub fn lower_graph_with_entry_and_params(
        &mut self,
        graph: &IrGraph,
        entry_block: Block,
        entry_params: &[Value],
    ) -> Result<(), HirError> {
        // Pre-map IrGraph block 0 to the provided entry block
        self.block_map.insert(BlockId(0), entry_block);

        // Map the graph entry block's parameter values to the forge_ir
        // entry params, so `load`ing a parameter reads the ABI argument.
        if let Some(block_data) = graph.block(BlockId(0)) {
            for (i, &gv) in block_data.param_values.iter().enumerate() {
                if i < entry_params.len() {
                    self.value_map.insert(gv, entry_params[i]);
                }
            }
        }

        // Create remaining blocks (create_blocks skips already-mapped blocks)
        self.create_blocks(graph)?;
        self.lower_blocks_internal(graph)
    }

    /// Process all blocks' nodes in order (shared by lower_graph / lower_graph_with_entry).
    fn lower_blocks_internal(&mut self, graph: &IrGraph) -> Result<(), HirError> {
        let block_count = graph.block_count() as u32;
        for i in 0..block_count {
            let bid = BlockId(i);
            if bid == BlockId::INVALID {
                continue;
            }
            if let Some(block_data) = graph.block(bid) {
                let fb_block = self.get_fb_block(bid)?;
                self.builder.switch_to_block(fb_block);

                let term_id = block_data.terminator;

                // Emit regular (non-terminator) nodes
                for &node_id in &block_data.nodes {
                    if Some(node_id) == term_id {
                        continue; // skip — handle below
                    }
                    self.lower_node(graph, node_id)?;
                }

                // Emit terminator
                if let Some(term_id) = term_id {
                    self.lower_terminator(graph, term_id)?;
                }
            }
        }

        Ok(())
    }

    /// Look up the forge_ir Block for a BlockId.
    fn get_fb_block(&self, bid: BlockId) -> Result<Block, HirError> {
        self.block_map
            .get(&bid)
            .copied()
            .ok_or_else(|| HirError::Internal(format!("BlockId {:?} not mapped", bid.0)))
    }

    /// Lower a "regular" (non-terminator) node.
    fn lower_node(&mut self, graph: &IrGraph, node_id: NodeId) -> Result<(), HirError> {
        let node = graph
            .node(node_id)
            .ok_or_else(|| HirError::Internal(format!("node {:?} not found", node_id.0)))?;

        let op = &node.op;

        let atom_spec = self
            .registry
            .lookup_atom(op)
            .ok_or_else(|| HirError::UnknownAtom {
                dialect: format!("{}", op.dialect),
                name: format!("{}", op.name),
            })?;

        let operands: Vec<Value> = node
            .operands
            .iter()
            .map(|gv| {
                self.value_map
                    .get(gv)
                    .copied()
                    .ok_or_else(|| HirError::Internal(format!("unmapped value {:?}", gv.0)))
            })
            .collect::<Result<_, _>>()?;

        let results = self.lower_opcode(&atom_spec.backend_op, &operands, &node.attrs)?;

        // Map result GraphValues → forge_ir Values
        for (i, result_val) in results.iter().enumerate() {
            if i < node.results.len() {
                self.value_map.insert(node.results[i], *result_val);
            }
        }

        Ok(())
    }

    /// Lower a terminator node.
    ///
    /// Terminators are recognized by their OpTag (cf.branch, cf.jump, cf.ret)
    /// rather than by Opcode, because control-flow operations are handled by
    /// FunctionBuilder methods, not by Opcode variants.
    fn lower_terminator(&mut self, graph: &IrGraph, node_id: NodeId) -> Result<(), HirError> {
        let node = graph.node(node_id).ok_or_else(|| {
            HirError::Internal(format!("terminator node {:?} not found", node_id.0))
        })?;

        let op = &node.op;

        // Map operands
        let operands: Vec<Value> = node
            .operands
            .iter()
            .map(|gv| {
                self.value_map
                    .get(gv)
                    .copied()
                    .ok_or_else(|| HirError::Internal(format!("unmapped value {:?}", gv.0)))
            })
            .collect::<Result<_, _>>()?;

        if *op == cf::branch() {
            let cond = operands
                .first()
                .copied()
                .ok_or_else(|| HirError::Internal("branch missing cond operand".into()))?;
            let then_block = self.extract_block_attr(&node.attrs, "then_block")?;
            let else_block = self.extract_block_attr(&node.attrs, "else_block")?;
            self.builder.branch(cond, then_block, &[], else_block, &[]);
        } else if *op == cf::jump() {
            let target = self.extract_block_attr(&node.attrs, "target")?;
            self.builder.jump(target, &[]);
        } else if *op == cf::ret() {
            self.builder.ret(&operands);
        } else if *op == cf::unreachable() {
            self.builder.unreachable();
        } else {
            // For non-CF terminators, try to lower as a regular node
            // (this allows other tagged operations to act as terminators)
            self.lower_node(graph, node_id)?;
        }

        Ok(())
    }

    /// Lower a specific Opcode with given operands and attributes.
    fn lower_opcode(
        &mut self,
        opcode: &Opcode,
        operands: &[Value],
        attrs: &HashMap<Symbol, AttrValue>,
    ) -> Result<Vec<Value>, HirError> {
        let results = match *opcode {
            // === Integer arithmetic (7) ===
            Opcode::Iadd => vec![self.builder.iadd(operands[0], operands[1])],
            Opcode::Isub => vec![self.builder.isub(operands[0], operands[1])],
            Opcode::Imul => vec![self.builder.imul(operands[0], operands[1])],
            Opcode::Udiv => vec![self.builder.udiv(operands[0], operands[1])],
            Opcode::Sdiv => vec![self.builder.sdiv(operands[0], operands[1])],
            Opcode::Urem => vec![self.builder.urem(operands[0], operands[1])],
            Opcode::Srem => vec![self.builder.srem(operands[0], operands[1])],

            // === Bitwise (7) ===
            Opcode::Band => vec![self.builder.band(operands[0], operands[1])],
            Opcode::Bor => vec![self.builder.bor(operands[0], operands[1])],
            Opcode::Bxor => vec![self.builder.bxor(operands[0], operands[1])],
            Opcode::Bnot => vec![self.builder.bnot(operands[0])],
            Opcode::Ishl => vec![self.builder.ishl(operands[0], operands[1])],
            Opcode::Ushr => vec![self.builder.ushr(operands[0], operands[1])],
            Opcode::Sshr => vec![self.builder.sshr(operands[0], operands[1])],

            // === Comparison (2) ===
            Opcode::Icmp { .. } => {
                let cond = extract_intcc(attrs)?;
                vec![self.builder.icmp(cond, operands[0], operands[1])]
            }
            Opcode::Fcmp { .. } => {
                let cond = extract_floatcc(attrs)?;
                vec![self.builder.fcmp(cond, operands[0], operands[1])]
            }

            // === Constants (2) ===
            Opcode::Iconst => {
                let value = extract_attr_i64(attrs, "value")?;
                let ty = attrs
                    .get(&sym_intern("ty"))
                    .and_then(|a| a.as_type())
                    .unwrap_or(TypeId::I32);
                vec![self.builder.iconst(value, ty)]
            }
            Opcode::Fconst => {
                let bits = extract_attr_u64(attrs, "bits")?;
                let ty = extract_attr_type(attrs, "ty").unwrap_or(TypeId::F64);
                vec![self.builder.fconst(bits, ty)]
            }

            // === Type conversions (4) ===
            Opcode::Sextend => {
                let to_ty = extract_attr_type(attrs, "to").unwrap_or(TypeId::I64);
                vec![self.builder.sextend(operands[0], to_ty)]
            }
            Opcode::Uextend => {
                let to_ty = extract_attr_type(attrs, "to").unwrap_or(TypeId::I64);
                vec![self.builder.uextend(operands[0], to_ty)]
            }
            Opcode::Ireduce => {
                let to_ty = extract_attr_type(attrs, "to").unwrap_or(TypeId::I32);
                vec![self.builder.ireduce(operands[0], to_ty)]
            }
            Opcode::Bitcast => {
                let to_ty = extract_attr_type(attrs, "to").unwrap_or(TypeId::I64);
                vec![self.builder.bitcast(operands[0], to_ty)]
            }

            // === Memory (2) ===
            Opcode::Load => {
                let ty = extract_attr_type(attrs, "ty").unwrap_or(TypeId::I32);
                vec![self.builder.load(operands[0], ty)]
            }
            Opcode::Store => {
                self.builder.store(operands[0], operands[1]);
                vec![]
            }

            // === Address (2) ===
            Opcode::StackAddr => {
                let offset = extract_attr_i64(attrs, "offset").unwrap_or(0) as i32;
                vec![self.builder.stack_addr(offset)]
            }
            Opcode::GlobalAddr => {
                use forge_ir::GlobalId;
                let id = extract_attr_u64(attrs, "id").unwrap_or(0) as u32;
                vec![self.builder.global_addr(GlobalId(id))]
            }

            // === Select (1) ===
            Opcode::Select => {
                vec![self.builder.select(operands[0], operands[1], operands[2])]
            }

            // === Value semantics (2) ===
            Opcode::Poison => {
                let ty = extract_attr_type(attrs, "ty").unwrap_or(TypeId::I32);
                vec![self.builder.poison(ty)]
            }
            Opcode::Undef => {
                let ty = extract_attr_type(attrs, "ty").unwrap_or(TypeId::I32);
                vec![self.builder.undef(ty)]
            }

            // === Copy (1) ===
            Opcode::Copy => vec![self.builder.copy(operands[0])],

            // === Freeze (1) ===
            Opcode::Freeze => vec![self.builder.freeze(operands[0])],

            // === Nop (0) — used as placeholder for CF ops that are handled
            //     by lower_terminator; if reached here, it's a no-op ===
            Opcode::Nop => vec![],

            // === Unsupported ===
            _ => {
                return Err(HirError::Lowering(format!(
                    "opcode {:?} not yet supported in lowering",
                    opcode.mnemonic()
                )));
            }
        };

        Ok(results)
    }

    // ============================================================
    // Block creation
    // ============================================================

    /// Create all blocks from the IrGraph in the FunctionBuilder.
    fn create_blocks(&mut self, graph: &IrGraph) -> Result<(), HirError> {
        let block_count = graph.block_count() as u32;
        for i in 0..block_count {
            let bid = BlockId(i);
            if bid == BlockId::INVALID {
                continue;
            }
            // Skip blocks already mapped (e.g., entry block mapped by caller)
            if self.block_map.contains_key(&bid) {
                continue;
            }
            if let Some(block_data) = graph.block(bid) {
                let param_tys: Vec<TypeId> = block_data.params.iter().copied().collect();

                let (fb_block, param_values) = if param_tys.is_empty() {
                    let block = self.builder.create_block();
                    (block, vec![])
                } else {
                    self.builder.create_block_with_tys(&param_tys)
                };

                self.block_map.insert(bid, fb_block);

                // Map block param GraphValues → forge_ir Values
                for (j, &gv) in block_data.param_values.iter().enumerate() {
                    if j < param_values.len() {
                        self.value_map.insert(gv, param_values[j]);
                    }
                }
            }
        }
        Ok(())
    }

    // ============================================================
    // Helpers
    // ============================================================

    /// Extract a graph-internal BlockId attribute and resolve it through
    /// `block_map` to the corresponding forge_ir block.
    ///
    /// (Legacy `AttrValue::Block` handles pass through unchanged.)
    fn extract_block_attr(
        &self,
        attrs: &HashMap<Symbol, AttrValue>,
        key: &str,
    ) -> Result<Block, HirError> {
        let sym = sym_intern(key);
        let attr = attrs
            .get(&sym)
            .ok_or_else(|| HirError::Internal(format!("missing attr '{}'", key)))?;
        match attr {
            AttrValue::BlockId(bid) => self
                .block_map
                .get(bid)
                .copied()
                .ok_or_else(|| HirError::Internal(format!("BlockId {:?} not mapped", bid.0))),
            AttrValue::Block(fb) => Ok(*fb),
            _ => Err(HirError::Internal(format!(
                "attr '{}' is not a Block/BlockId, got {:?}",
                key, attr
            ))),
        }
    }
}

// ============================================================
// Attribute extraction helpers
// ============================================================

/// Extract an i64 from attrs.
pub fn extract_attr_i64(attrs: &HashMap<Symbol, AttrValue>, key: &str) -> Result<i64, HirError> {
    let sym = sym_intern(key);
    let val = attrs
        .get(&sym)
        .ok_or_else(|| HirError::Internal(format!("missing attr '{}'", key)))?;
    val.as_int()
        .ok_or_else(|| HirError::Internal(format!("attr '{}' is not an int, got {:?}", key, val)))
}

/// Extract a u64 from attrs.
pub fn extract_attr_u64(attrs: &HashMap<Symbol, AttrValue>, key: &str) -> Result<u64, HirError> {
    let sym = sym_intern(key);
    let val = attrs
        .get(&sym)
        .ok_or_else(|| HirError::Internal(format!("missing attr '{}'", key)))?;
    val.as_uint()
        .ok_or_else(|| HirError::Internal(format!("attr '{}' is not a uint, got {:?}", key, val)))
}

/// Extract a TypeId from attrs.
pub fn extract_attr_type(attrs: &HashMap<Symbol, AttrValue>, key: &str) -> Option<TypeId> {
    let sym = sym_intern(key);
    attrs.get(&sym).and_then(|v| v.as_type())
}

/// Extract an IntCC from attrs. Supports both string and integer representations.
pub fn extract_intcc(attrs: &HashMap<Symbol, AttrValue>) -> Result<IntCC, HirError> {
    let key = sym_intern("cond");
    let val = attrs
        .get(&key)
        .ok_or_else(|| HirError::Internal("missing 'cond' attr for icmp".into()))?;

    // Try string-based lookup
    if let Some(s) = val.as_str() {
        return parse_intcc_str(&crate::attr::sym_lookup(s));
    }

    // Try int-based lookup
    if let Some(i) = val.as_int() {
        return parse_intcc_int(i);
    }

    Err(HirError::Internal(format!(
        "cannot parse 'cond' attr as IntCC: {:?}",
        val
    )))
}

fn parse_intcc_str(s: &str) -> Result<IntCC, HirError> {
    match s {
        "eq" | "Equal" => Ok(IntCC::Equal),
        "ne" | "NotEqual" => Ok(IntCC::NotEqual),
        "slt" | "SignedLessThan" => Ok(IntCC::SignedLessThan),
        "sgt" | "SignedGreaterThan" => Ok(IntCC::SignedGreaterThan),
        "sle" | "SignedLessThanOrEqual" => Ok(IntCC::SignedLessThanOrEqual),
        "sge" | "SignedGreaterThanOrEqual" => Ok(IntCC::SignedGreaterThanOrEqual),
        "ult" | "UnsignedLessThan" => Ok(IntCC::UnsignedLessThan),
        "ugt" | "UnsignedGreaterThan" => Ok(IntCC::UnsignedGreaterThan),
        "ule" | "UnsignedLessThanOrEqual" => Ok(IntCC::UnsignedLessThanOrEqual),
        "uge" | "UnsignedGreaterThanOrEqual" => Ok(IntCC::UnsignedGreaterThanOrEqual),
        _ => Err(HirError::Internal(format!("unknown IntCC string: '{}'", s))),
    }
}

fn parse_intcc_int(i: i64) -> Result<IntCC, HirError> {
    match i {
        0 => Ok(IntCC::Equal),
        1 => Ok(IntCC::NotEqual),
        2 => Ok(IntCC::SignedLessThan),
        3 => Ok(IntCC::SignedGreaterThan),
        4 => Ok(IntCC::SignedLessThanOrEqual),
        5 => Ok(IntCC::SignedGreaterThanOrEqual),
        6 => Ok(IntCC::UnsignedLessThan),
        7 => Ok(IntCC::UnsignedGreaterThan),
        8 => Ok(IntCC::UnsignedLessThanOrEqual),
        9 => Ok(IntCC::UnsignedGreaterThanOrEqual),
        _ => Err(HirError::Internal(format!("unknown IntCC int: {}", i))),
    }
}

/// Extract a FloatCC from attrs.
pub fn extract_floatcc(
    attrs: &HashMap<Symbol, AttrValue>,
) -> Result<forge_ir::opcode::FloatCC, HirError> {
    let key = sym_intern("cond");
    let val = attrs
        .get(&key)
        .ok_or_else(|| HirError::Internal("missing 'cond' attr for fcmp".into()))?;

    if let Some(s) = val.as_str() {
        let s_str = crate::attr::sym_lookup(s);
        match s_str.as_str() {
            "ord" | "Ordered" => return Ok(forge_ir::opcode::FloatCC::Ordered),
            "uno" | "Unordered" => return Ok(forge_ir::opcode::FloatCC::Unordered),
            "eq" | "Equal" => return Ok(forge_ir::opcode::FloatCC::Equal),
            "ne" | "NotEqual" => return Ok(forge_ir::opcode::FloatCC::NotEqual),
            "lt" | "LessThan" => return Ok(forge_ir::opcode::FloatCC::LessThan),
            "le" | "LessThanOrEqual" => {
                return Ok(forge_ir::opcode::FloatCC::LessThanOrEqual);
            }
            "gt" | "GreaterThan" => return Ok(forge_ir::opcode::FloatCC::GreaterThan),
            "ge" | "GreaterThanOrEqual" => {
                return Ok(forge_ir::opcode::FloatCC::GreaterThanOrEqual);
            }
            _ => {}
        }
    }

    Err(HirError::Internal(format!(
        "cannot parse 'cond' attr as FloatCC: {:?}",
        val
    )))
}

// ============================================================
// Convenience: lower an IrGraph directly into a Module
// ============================================================

/// Lower an entire IrGraph into a function and add it to a Module.
///
/// This is the recommended entry point for users: it creates a FunctionBuilder,
/// lowers the graph, finalizes the function, and adds it to the module in one step.
///
/// # Example
/// ```ignore
/// let mut module = Module::new();
/// let sig = FunctionSignature::new(&[(TypeId::I32, "x")], &[TypeId::I32]);
/// let func_ref = lower_into_module(&graph, &registry, &mut module, "my_func", sig)?;
/// ```
pub fn lower_into_module(
    graph: &IrGraph,
    registry: &BrickRegistry,
    module: &mut forge_ir::Module,
    name: &str,
    sig: forge_ir::FunctionSignature,
) -> Result<forge_ir::FuncRef, HirError> {
    let ctx = module.types.clone();
    let mut builder = FunctionBuilder::new(name, ctx, sig);

    // Create the function's entry block, which will be mapped to IrGraph block 0
    let (entry_block, entry_params) = builder.create_entry_block();

    let mut lowering = LoweringContext::new(&mut builder, registry);
    // Map IrGraph block 0 to the already-created entry block, and its
    // parameters to the forge_ir entry-block parameters (function arguments)
    lowering.lower_graph_with_entry_and_params(graph, entry_block, &entry_params)?;

    let func = builder
        .finish()
        .map_err(|e| HirError::Internal(e.to_string()))?;
    // P0-14：接入 forge-ir 验证器——HIR 降级产物必须通过 IR 一致性校验
    // （入口/类型/use/块参数/终结符/支配）。此前 2580 行 Verifier 在 HIR
    // 路径零调用，图级缺陷（块参数不匹配、悬空 use）变成未初始化寄存器读。
    let mut verifier = forge_ir::verify::Verifier::new();
    if let Err(errors) = verifier.verify(&func) {
        return Err(HirError::Internal(format!(
            "HIR lowered function '{name}' failed IR verification: {errors:?}"
        )));
    }
    Ok(module.add_function(func))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_extract_intcc_from_string() {
        let mut attrs = HashMap::new();
        attrs.insert(sym_intern("cond"), AttrValue::str("NotEqual"));
        assert_eq!(extract_intcc(&attrs).unwrap(), IntCC::NotEqual);
    }

    #[test]
    fn test_extract_intcc_from_int() {
        let mut attrs = HashMap::new();
        attrs.insert(sym_intern("cond"), AttrValue::Int(1)); // NotEqual
        assert_eq!(extract_intcc(&attrs).unwrap(), IntCC::NotEqual);
    }

    #[test]
    fn test_extract_attr_i64() {
        let mut attrs = HashMap::new();
        attrs.insert(sym_intern("value"), AttrValue::Int(42));
        assert_eq!(extract_attr_i64(&attrs, "value").unwrap(), 42);
    }

    #[test]
    fn test_lower_simple_expression() {
        use crate::atom;
        use forge_ir::{FunctionSignature, TypeContext};

        let mut registry = BrickRegistry::new();

        // Register atoms
        registry
            .register_atom(
                atom::AtomSpec::new(atom::arith::iconst(), Opcode::Iconst)
                    .attr_int("value")
                    .output("result", TypeId::I32)
                    .clone(),
            )
            .unwrap();
        registry
            .register_atom(
                atom::AtomSpec::new(atom::arith::iadd(), Opcode::Iadd)
                    .input("lhs", TypeId::I32)
                    .input("rhs", TypeId::I32)
                    .output("result", TypeId::I32)
                    .clone(),
            )
            .unwrap();
        registry
            .register_atom(
                atom::AtomSpec::new(atom::cf::ret(), Opcode::Nop)
                    .input("value", TypeId::I32)
                    .clone(),
            )
            .unwrap();

        // Build IrGraph: iconst(42) → iadd(iconst(1)) → ret(result)
        let mut graph = IrGraph::new();
        let block = graph.create_block(&[]);

        let mut attrs1 = HashMap::new();
        attrs1.insert(sym_intern("value"), AttrValue::Int(42));
        let n1 = graph
            .create_node(atom::arith::iconst(), &[], attrs1, &[TypeId::I32])
            .unwrap();
        graph.append_to_block(block, n1).unwrap();
        let v1 = graph.node_results(n1)[0];

        let mut attrs2 = HashMap::new();
        attrs2.insert(sym_intern("value"), AttrValue::Int(1));
        let n2 = graph
            .create_node(atom::arith::iconst(), &[], attrs2, &[TypeId::I32])
            .unwrap();
        graph.append_to_block(block, n2).unwrap();
        let v2 = graph.node_results(n2)[0];

        let n3 = graph
            .create_node(
                atom::arith::iadd(),
                &[v1, v2],
                HashMap::new(),
                &[TypeId::I32],
            )
            .unwrap();
        graph.append_to_block(block, n3).unwrap();
        let v3 = graph.node_results(n3)[0];

        let n4 = graph
            .create_node(atom::cf::ret(), &[v3], HashMap::new(), &[])
            .unwrap();
        graph.set_terminator(block, n4).unwrap();

        // Lower：不预创建 entry —— lower_graph 会把 graph 块 0 自动建成函数入口
        let ctx = TypeContext::new();
        let sig = FunctionSignature::new(&[], &[TypeId::I32]);
        let mut builder = FunctionBuilder::new("test", ctx, sig);

        let mut lowering = LoweringContext::new(&mut builder, &registry);
        lowering.lower_graph(&graph).unwrap();
        // Verify lowering succeeded (function is implicitly verified by finishing the builder)
        drop(lowering);
        let _func = builder.finish().expect("build");
    }

    #[test]
    fn test_lower_if_else() {
        use crate::atom;
        use forge_ir::{FunctionSignature, TypeContext};

        let mut registry = BrickRegistry::new();

        // iconst atom
        registry
            .register_atom(
                atom::AtomSpec::new(atom::arith::iconst(), Opcode::Iconst)
                    .attr_int("value")
                    .output("result", TypeId::I32)
                    .clone(),
            )
            .unwrap();

        // icmp atom
        registry
            .register_atom(
                atom::AtomSpec::new(atom::arith::icmp(), Opcode::Icmp { cond: IntCC::Equal })
                    .attr_str("cond")
                    .input("lhs", TypeId::I32)
                    .input("rhs", TypeId::I32)
                    .output("result", TypeId::BOOL)
                    .clone(),
            )
            .unwrap();

        // branch atom (Nop — handled by terminator logic)
        registry
            .register_atom(
                atom::AtomSpec::new(atom::cf::branch(), Opcode::Nop)
                    .input("cond", TypeId::BOOL)
                    .clone(),
            )
            .unwrap();

        // jump atom (Nop)
        registry
            .register_atom(atom::AtomSpec::new(atom::cf::jump(), Opcode::Nop).clone())
            .unwrap();

        // ret atom (Nop)
        registry
            .register_atom(
                atom::AtomSpec::new(atom::cf::ret(), Opcode::Nop)
                    .input("value", TypeId::I32)
                    .clone(),
            )
            .unwrap();

        // Build IrGraph with 4 blocks: entry, then, else, merge
        let mut graph = IrGraph::new();
        let entry = graph.create_block(&[]);
        let then_blk = graph.create_block(&[]);
        let else_blk = graph.create_block(&[]);
        let merge_blk = graph.create_block(&[]);

        // --- entry block ---
        graph.set_current_block(entry);

        // iconst(1) — condition
        let mut attrs = HashMap::new();
        attrs.insert(sym_intern("value"), AttrValue::Int(1));
        let n_cond = graph
            .create_node(atom::arith::iconst(), &[], attrs, &[TypeId::I32])
            .unwrap();
        graph.append_to_block(entry, n_cond).unwrap();
        let v_cond = graph.node_results(n_cond)[0];

        // iconst(0)
        let mut attrs = HashMap::new();
        attrs.insert(sym_intern("value"), AttrValue::Int(0));
        let n_zero = graph
            .create_node(atom::arith::iconst(), &[], attrs, &[TypeId::I32])
            .unwrap();
        graph.append_to_block(entry, n_zero).unwrap();
        let v_zero = graph.node_results(n_zero)[0];

        // icmp NE(v_cond, v_zero)
        let mut cmp_attrs = HashMap::new();
        cmp_attrs.insert(sym_intern("cond"), AttrValue::str("NotEqual"));
        let n_cmp = graph
            .create_node(
                atom::arith::icmp(),
                &[v_cond, v_zero],
                cmp_attrs,
                &[TypeId::BOOL],
            )
            .unwrap();
        graph.append_to_block(entry, n_cmp).unwrap();
        let v_cmp = graph.node_results(n_cmp)[0];

        // branch
        let mut branch_attrs = HashMap::new();
        branch_attrs.insert(sym_intern("then_block"), AttrValue::BlockId(then_blk));
        branch_attrs.insert(sym_intern("else_block"), AttrValue::BlockId(else_blk));
        let n_branch = graph
            .create_node(atom::cf::branch(), &[v_cmp], branch_attrs, &[])
            .unwrap();
        graph.set_terminator(entry, n_branch).unwrap();

        // --- then block: iconst(10); ret ---
        let mut attrs = HashMap::new();
        attrs.insert(sym_intern("value"), AttrValue::Int(10));
        let n_t = graph
            .create_node(atom::arith::iconst(), &[], attrs, &[TypeId::I32])
            .unwrap();
        graph.append_to_block(then_blk, n_t).unwrap();
        let v_then = graph.node_results(n_t)[0];
        let n_ret_then = graph
            .create_node(atom::cf::ret(), &[v_then], HashMap::new(), &[])
            .unwrap();
        graph.set_terminator(then_blk, n_ret_then).unwrap();

        // --- else block: iconst(20); ret ---
        let mut attrs = HashMap::new();
        attrs.insert(sym_intern("value"), AttrValue::Int(20));
        let n_e = graph
            .create_node(atom::arith::iconst(), &[], attrs, &[TypeId::I32])
            .unwrap();
        graph.append_to_block(else_blk, n_e).unwrap();
        let v_else = graph.node_results(n_e)[0];
        let n_ret_else = graph
            .create_node(atom::cf::ret(), &[v_else], HashMap::new(), &[])
            .unwrap();
        graph.set_terminator(else_blk, n_ret_else).unwrap();

        // --- merge block: iconst(0); ret ---
        let mut attrs = HashMap::new();
        attrs.insert(sym_intern("value"), AttrValue::Int(0));
        let n_ret = graph
            .create_node(atom::arith::iconst(), &[], attrs, &[TypeId::I32])
            .unwrap();
        graph.append_to_block(merge_blk, n_ret).unwrap();
        let v_ret_val = graph.node_results(n_ret)[0];
        let n_ret_term = graph
            .create_node(atom::cf::ret(), &[v_ret_val], HashMap::new(), &[])
            .unwrap();
        graph.set_terminator(merge_blk, n_ret_term).unwrap();

        // Lower：不预创建 entry —— lower_graph 会把 graph 块 0 自动建成函数入口
        let ctx = TypeContext::new();
        let sig = FunctionSignature::new(&[], &[TypeId::I32]);
        let mut builder = FunctionBuilder::new("test_if", ctx, sig);

        let mut lowering = LoweringContext::new(&mut builder, &registry);
        lowering.lower_graph(&graph).unwrap();
        drop(lowering);
        let _func = builder.finish().expect("build");
    }
}
