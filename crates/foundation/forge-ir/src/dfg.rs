//! 数据流图 (DataFlowGraph) — 核心 IR 数据结构。
//!
//! DFG 管理所有 IR 实体的数据存储:
//! - `values: Vec<ValueData>` — SSA 值 (按 Value.0 索引)
//! - `insts: Vec<Instruction>` — 指令 (按 Inst.0 索引)
//! - `blocks: Vec<BlockData>` — 基本块 (按 Block.0 索引)
//!
//! 遵循 Cranelift 的实体-组件分离模式: 实体是 Copy 句柄,
//! 数据存储在 Vec 中 (索引即实体 ID)。

use super::debug_info::SourceLocation;
use super::entity::*;
use super::immediate::Immediate;
use super::inst_flags::InstFlags;
use super::mem_flags::MemFlags;
use super::metadata::AttachedMetadata;
use super::opcode::Opcode;
use super::terminator::Terminator;
use smallvec::SmallVec;

// ============================================================
// ValueDef — Value 的来源
// ============================================================

/// Value 的定义来源。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ValueDef {
    /// 由指令的第 `result_idx` 个结果定义。
    Inst(Inst, u8),
    /// 基本块的第 `param_idx` 个参数。
    Param(Block, u16),
}

// ============================================================
// ValueData, Instruction, BlockData
// ============================================================

/// SSA 值的数据。
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ValueData {
    pub def: ValueDef,
    pub ty: TypeId,
}

/// 指令的数据。
#[derive(Clone, Debug)]
pub struct Instruction {
    pub opcode: Opcode,
    pub block: Block,
    pub pos: u32,
    pub results: SmallVec<[Value; 2]>,
    pub operands: SmallVec<[Value; 4]>,
    pub immediates: SmallVec<[Immediate; 4]>,
    pub flags: InstFlags,
    /// Memory access flags — meaningful for Load/Store; MemFlags::NONE otherwise.
    pub mem_flags: MemFlags,
    /// Attached metadata — TBAA, alias scope, alignment hints, etc.
    pub metadata: SmallVec<[AttachedMetadata; 2]>,
    /// Source location — file, line, column for debugging and diagnostics.
    /// Moved from `Function.source_locations` HashMap to the Instruction itself
    /// for consistency with LLVM/Cranelift design.
    pub loc: Option<SourceLocation>,
}

/// 基本块的数据。
#[derive(Clone, Debug)]
pub struct BlockData {
    /// 块参数的类型列表
    pub params: SmallVec<[TypeId; 2]>,
    /// 块参数的 Value 句柄 (与 params 一一对应, O(1) 访问)
    pub param_values: SmallVec<[Value; 2]>,
    /// 块内指令的顺序列表 (索引到 DataFlowGraph.insts)
    pub inst_order: Vec<Inst>,
    pub terminator: Terminator,
}

// ============================================================
// DataFlowGraph
// ============================================================

#[derive(Clone, Debug)]
pub struct DataFlowGraph {
    pub values: Vec<ValueData>,
    pub insts: Vec<Instruction>,
    pub blocks: Vec<BlockData>,
}

impl DataFlowGraph {
    pub fn new() -> Self {
        Self {
            values: Vec::new(),
            insts: Vec::new(),
            blocks: Vec::new(),
        }
    }

    // === 实体分配 ===

    pub fn make_value(&mut self, ty: TypeId, def: ValueDef) -> Value {
        let v = Value(self.values.len() as u32);
        self.values.push(ValueData { def, ty });
        v
    }

    pub fn make_inst(
        &mut self,
        opcode: Opcode,
        block: Block,
        operands: SmallVec<[Value; 4]>,
        immediates: SmallVec<[Immediate; 4]>,
        result_tys: &[TypeId],
        flags: InstFlags,
    ) -> Inst {
        self.make_inst_with_meta(
            opcode,
            block,
            operands,
            immediates,
            result_tys,
            flags,
            MemFlags::NONE,
            SmallVec::new(),
        )
    }

    /// Create an instruction with memory flags and metadata.
    #[allow(clippy::too_many_arguments)]
    pub fn make_inst_with_meta(
        &mut self,
        opcode: Opcode,
        block: Block,
        operands: SmallVec<[Value; 4]>,
        immediates: SmallVec<[Immediate; 4]>,
        result_tys: &[TypeId],
        flags: InstFlags,
        mem_flags: MemFlags,
        metadata: SmallVec<[AttachedMetadata; 2]>,
    ) -> Inst {
        self.make_inst_with_meta_and_loc(
            opcode, block, operands, immediates, result_tys, flags, mem_flags, metadata, None,
        )
    }

    /// Create an instruction with memory flags, metadata, and source location.
    #[allow(clippy::too_many_arguments)]
    pub fn make_inst_with_meta_and_loc(
        &mut self,
        opcode: Opcode,
        block: Block,
        operands: SmallVec<[Value; 4]>,
        immediates: SmallVec<[Immediate; 4]>,
        result_tys: &[TypeId],
        flags: InstFlags,
        mem_flags: MemFlags,
        metadata: SmallVec<[AttachedMetadata; 2]>,
        loc: Option<SourceLocation>,
    ) -> Inst {
        let inst = Inst(self.insts.len() as u32);
        let results: SmallVec<[Value; 2]> = result_tys
            .iter()
            .enumerate()
            .map(|(i, &ty)| {
                let v = Value(self.values.len() as u32);
                self.values.push(ValueData {
                    def: ValueDef::Inst(inst, i as u8),
                    ty,
                });
                v
            })
            .collect();
        let pos = self.blocks[block.0 as usize].inst_order.len() as u32;
        let instruction = Instruction {
            opcode,
            block,
            pos,
            results: results.clone(),
            operands,
            immediates,
            flags,
            mem_flags,
            metadata,
            loc,
        };
        // Push instruction to global table first (determines Inst.0 index),
        // then record its index in the block's inst_order.
        self.insts.push(instruction);
        self.blocks[block.0 as usize].inst_order.push(inst);
        inst
    }

    pub fn make_block(&mut self) -> Block {
        let block = Block(self.blocks.len() as u32);
        self.blocks.push(BlockData {
            params: SmallVec::new(),
            param_values: SmallVec::new(),
            inst_order: Vec::new(),
            terminator: Terminator::Unreachable,
        });
        block
    }

    pub fn make_block_with_params(&mut self, param_tys: &[TypeId]) -> (Block, Vec<Value>) {
        let block = Block(self.blocks.len() as u32);
        let params: Vec<Value> = param_tys
            .iter()
            .enumerate()
            .map(|(i, &ty)| {
                let v = Value(self.values.len() as u32);
                self.values.push(ValueData {
                    def: ValueDef::Param(block, i as u16),
                    ty,
                });
                v
            })
            .collect();
        self.blocks.push(BlockData {
            params: param_tys.iter().copied().collect(),
            param_values: params.iter().copied().collect(),
            inst_order: Vec::new(),
            terminator: Terminator::Unreachable,
        });
        (block, params)
    }

    // === 修改 ===

    pub fn set_terminator(&mut self, block: Block, term: Terminator) {
        self.blocks[block.0 as usize].terminator = term;
    }

    pub fn remove_inst(&mut self, inst: Inst) {
        let idx = inst.0 as usize;
        if idx < self.insts.len() {
            // Remove from block's inst_order
            let block = self.insts[idx].block;
            if (block.0 as usize) < self.blocks.len() {
                self.blocks[block.0 as usize]
                    .inst_order
                    .retain(|&i| i != inst);
            }
            // Mark as Nop in global table
            self.insts[idx].opcode = Opcode::Nop;
            self.insts[idx].results.clear();
            self.insts[idx].operands.clear();
            self.insts[idx].immediates.clear();
        }
    }

    pub fn remove_block(&mut self, block: Block) {
        // Mark all instructions in this block as Nop
        let insts: Vec<Inst> = self.blocks[block.0 as usize].inst_order.drain(..).collect();
        for inst in insts {
            if (inst.0 as usize) < self.insts.len() {
                self.insts[inst.0 as usize].opcode = Opcode::Nop;
                self.insts[inst.0 as usize].results.clear();
                self.insts[inst.0 as usize].operands.clear();
                self.insts[inst.0 as usize].immediates.clear();
            }
        }
    }

    // === 查询 ===

    pub fn value_def(&self, v: Value) -> Option<&ValueDef> {
        self.values.get(v.0 as usize).map(|d| &d.def)
    }
    pub fn value_type(&self, v: Value) -> Option<TypeId> {
        self.values.get(v.0 as usize).map(|d| d.ty)
    }
    pub fn inst_results(&self, i: Inst) -> &[Value] {
        self.insts
            .get(i.0 as usize)
            .map(|d| d.results.as_slice())
            .unwrap_or(&[])
    }
    pub fn inst_opcode(&self, i: Inst) -> Option<&Opcode> {
        self.insts.get(i.0 as usize).map(|d| &d.opcode)
    }
    pub fn inst_operands(&self, i: Inst) -> &[Value] {
        self.insts
            .get(i.0 as usize)
            .map(|d| d.operands.as_slice())
            .unwrap_or(&[])
    }
    pub fn inst_block(&self, i: Inst) -> Option<Block> {
        self.insts.get(i.0 as usize).map(|d| d.block)
    }
    pub fn block_params(&self, b: Block) -> &[TypeId] {
        self.blocks
            .get(b.0 as usize)
            .map(|d| d.params.as_slice())
            .unwrap_or(&[])
    }

    /// 获取基本块参数对应的 Value 句柄 (O(1) 访问).
    pub fn block_param_values(&self, b: Block) -> &[Value] {
        self.blocks
            .get(b.0 as usize)
            .map(|d| d.param_values.as_slice())
            .unwrap_or(&[])
    }

    pub fn block_terminator(&self, b: Block) -> Option<&Terminator> {
        self.blocks.get(b.0 as usize).map(|d| &d.terminator)
    }
    pub fn block_terminator_mut(&mut self, b: Block) -> Option<&mut Terminator> {
        self.blocks.get_mut(b.0 as usize).map(|d| &mut d.terminator)
    }

    // === 迭代 ===

    pub fn values(&self) -> impl Iterator<Item = (Value, &ValueData)> {
        self.values
            .iter()
            .enumerate()
            .map(|(i, vd)| (Value(i as u32), vd))
    }
    pub fn insts(&self) -> impl Iterator<Item = (Inst, &Instruction)> {
        self.insts
            .iter()
            .enumerate()
            .filter(|(_, id)| !matches!(id.opcode, Opcode::Nop) || !id.results.is_empty())
            .map(|(i, id)| (Inst(i as u32), id))
    }
    pub fn blocks(&self) -> impl Iterator<Item = (Block, &BlockData)> {
        self.blocks
            .iter()
            .enumerate()
            .map(|(i, bd)| (Block(i as u32), bd))
    }
    pub fn block_inst_iter(&self, b: Block) -> impl Iterator<Item = &Instruction> {
        struct InstIter<'a> {
            dfg: &'a DataFlowGraph,
            iter: std::slice::Iter<'a, Inst>,
        }
        impl<'a> Iterator for InstIter<'a> {
            type Item = &'a Instruction;
            fn next(&mut self) -> Option<Self::Item> {
                self.iter
                    .next()
                    .and_then(|&inst| self.dfg.insts.get(inst.0 as usize))
            }
        }
        if (b.0 as usize) < self.blocks.len() {
            InstIter {
                dfg: self,
                iter: self.blocks[b.0 as usize].inst_order.iter(),
            }
        } else {
            InstIter {
                dfg: self,
                iter: [].iter(),
            }
        }
    }

    // === 便捷访问 ===

    /// 按 Block 索引获取 BlockData (O(1)).
    pub fn block(&self, b: Block) -> &BlockData {
        &self.blocks[b.0 as usize]
    }
    /// 按 Block 索引获取可变 BlockData。
    pub fn block_mut(&mut self, b: Block) -> &mut BlockData {
        &mut self.blocks[b.0 as usize]
    }

    pub fn block_count(&self) -> usize {
        self.blocks.len()
    }
    /// 已分配的指令总数（包括已删除的 Nop 指令）。用于调试。
    pub fn inst_count(&self) -> usize {
        self.insts.len()
    }
    /// 当前活跃的指令数量（不包括 Nop）。
    pub fn live_inst_count(&self) -> usize {
        let mut count = 0;
        for block in &self.blocks {
            for &inst in &block.inst_order {
                if (inst.0 as usize) < self.insts.len()
                    && !matches!(self.insts[inst.0 as usize].opcode, Opcode::Nop)
                {
                    count += 1;
                }
            }
        }
        count
    }
    pub fn value_count(&self) -> usize {
        self.values.len()
    }

    /// Compact the DFG: remove dead (Nop) instructions and rebuild all ID
    /// mappings. Returns statistics about the compaction.
    pub fn compact(&mut self) {
        let old_inst_count = self.insts.len();

        // 1. Build live inst set: non-Nop or has results
        let mut is_live: Vec<bool> = vec![false; self.insts.len()];
        for (i, inst) in self.insts.iter().enumerate() {
            if !matches!(inst.opcode, Opcode::Nop) || !inst.results.is_empty() {
                is_live[i] = true;
            }
        }

        // 2. Build old_inst_id → new_inst_id mapping
        let mut inst_map: Vec<Option<Inst>> = vec![None; self.insts.len()];
        let mut new_insts: Vec<Instruction> = Vec::with_capacity(self.insts.len());
        for (old_idx, live) in is_live.iter().enumerate() {
            if *live {
                let new_id = Inst(new_insts.len() as u32);
                inst_map[old_idx] = Some(new_id);
                let inst = self.insts[old_idx].clone();
                new_insts.push(inst);
            }
        }
        let _removed_insts = old_inst_count - new_insts.len();

        // 3. Remap ValueDef::Inst references
        for vd in &mut self.values {
            if let ValueDef::Inst(old_inst, idx) = vd.def
                && let Some(&Some(new_inst)) = inst_map.get(old_inst.0 as usize)
            {
                vd.def = ValueDef::Inst(new_inst, idx);
            }
        }

        // 4. Remap block.inst_order
        for block_data in &mut self.blocks {
            let new_order: Vec<Inst> = block_data
                .inst_order
                .iter()
                .filter_map(|&old_inst| {
                    inst_map.get(old_inst.0 as usize).and_then(|mapped| *mapped)
                })
                .collect();
            block_data.inst_order = new_order;
        }

        // 5. Replace insts Vec with compacted version
        self.insts = new_insts;
    }
}

impl Default for DataFlowGraph {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn test_types() -> (TypeId, TypeId, TypeId) {
        (TypeId(2), TypeId(2), TypeId(1))
    }

    #[test]
    fn test_make_block() {
        let mut dfg = DataFlowGraph::new();
        let b = dfg.make_block();
        assert_eq!(dfg.block_count(), 1);
        assert!(matches!(
            dfg.block_terminator(b).unwrap(),
            Terminator::Unreachable
        ));
    }

    #[test]
    fn test_make_block_with_params() {
        let mut dfg = DataFlowGraph::new();
        let (i32_ty, _, _) = test_types();
        let (block, params) = dfg.make_block_with_params(&[i32_ty, i32_ty]);
        assert_eq!(params.len(), 2);
        let v0 = dfg.value_def(params[0]).unwrap();
        assert!(matches!(v0, ValueDef::Param(b, 0) if b.0 == block.0));
    }

    #[test]
    fn test_make_inst() {
        let mut dfg = DataFlowGraph::new();
        let (i32_ty, _, _) = test_types();
        let (block, params) = dfg.make_block_with_params(&[i32_ty, i32_ty]);
        let inst = dfg.make_inst(
            Opcode::Iadd,
            block,
            smallvec::smallvec![params[0], params[1]],
            SmallVec::new(),
            &[i32_ty],
            InstFlags::NONE,
        );
        let results = dfg.inst_results(inst);
        assert_eq!(results.len(), 1);
        let def = dfg.value_def(results[0]).unwrap();
        assert!(matches!(def, ValueDef::Inst(i, 0) if i.0 == inst.0));
    }

    #[test]
    fn test_compact_removes_nops() {
        let mut dfg = DataFlowGraph::new();
        let (i32_ty, _, _) = test_types();
        let block = dfg.make_block();

        // Create 3 instructions
        dfg.make_inst(
            Opcode::Iconst,
            block,
            smallvec::SmallVec::new(),
            SmallVec::new(),
            &[i32_ty],
            InstFlags::NONE,
        );
        let i2 = dfg.make_inst(
            Opcode::Iconst,
            block,
            smallvec::SmallVec::new(),
            SmallVec::new(),
            &[i32_ty],
            InstFlags::NONE,
        );
        dfg.make_inst(
            Opcode::Iconst,
            block,
            smallvec::SmallVec::new(),
            SmallVec::new(),
            &[i32_ty],
            InstFlags::NONE,
        );
        let old_inst_count = dfg.insts.len();
        assert_eq!(dfg.live_inst_count(), 3);

        // Remove the middle instruction
        dfg.remove_inst(i2);
        assert_eq!(dfg.live_inst_count(), 2);

        // Compact — reclaims dead inst slot
        dfg.compact();
        assert!(dfg.insts.len() < old_inst_count);
        assert_eq!(dfg.live_inst_count(), 2);
    }

    #[test]
    fn test_compact_preserves_values() {
        let mut dfg = DataFlowGraph::new();
        let (i32_ty, _, _) = test_types();
        let block = dfg.make_block();

        let i1 = dfg.make_inst(
            Opcode::Iconst,
            block,
            smallvec::SmallVec::new(),
            SmallVec::new(),
            &[i32_ty],
            InstFlags::NONE,
        );
        let v1 = dfg.inst_results(i1)[0];
        dfg.make_inst(
            Opcode::Iconst,
            block,
            smallvec::SmallVec::new(),
            SmallVec::new(),
            &[i32_ty],
            InstFlags::NONE,
        );

        // Compact (no Nops to remove) should keep everything
        dfg.compact();
        assert_eq!(dfg.live_inst_count(), 2);
        // v1 should still be defined after compaction
        assert!(dfg.value_def(v1).is_some());
    }
}
