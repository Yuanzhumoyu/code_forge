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
use std::collections::HashMap;

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
    /// 聚合常量值（ConstantPool 聚合常量——store/ret 等操作数位置的
    /// `{i32 1, i32 2}` 字面量；display 原样还原）。
    AggConst(crate::AggId),
    /// 未定义引用占位（LLVM 前向引用/故意未定义值——operand_to_value
    /// 宽松保留名字,display 输出 `%name` 原样还原;第二十九轮）。
    UndefNamed(crate::string_pool::InternedStr),
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
    /// call 实参属性（与 operands[1..] 对齐；非 call 指令为空）。
    pub param_attrs: SmallVec<[crate::function::ParamAttributes; 4]>,
    /// call-site 函数属性（`call ... nounwind`；仅 call 指令）。
    pub fn_attrs: crate::function::FunctionAttributes,
    /// Attached metadata — TBAA, alias scope, alignment hints, etc.
    pub metadata: SmallVec<[AttachedMetadata; 2]>,
    /// Source location — file, line, column for debugging and diagnostics.
    pub loc: Option<SourceLocation>,
    /// Instruction-selection strategy tag produced by pattern matching
    /// (Stage 3). When set, the lowering pass should emit the fused machine
    /// sequence named by the tag instead of lowering the opcode verbatim
    /// (e.g. `"lea_sib:4"` for `Iadd(Imul(idx,4), base)` → LEA).
    /// `None` = default lowering.
    pub isel_strategy: Option<&'static str>,
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
    /// 终结符是否被显式设置（ret/jump/branch/unreachable/switch 均可）。
    /// finish() 时校验：块漏写终结符会保持默认 Unreachable，被编译期
    /// 无条件 lower 成 UD2，运行到该块即非法指令崩溃。
    pub has_terminator: bool,
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

    /// 创建聚合常量值（`{i32 1, i32 2}` 字面量操作数）。
    pub fn make_agg_const_value(&mut self, ty: TypeId, agg_id: crate::AggId) -> Value {
        self.make_value(ty, ValueDef::AggConst(agg_id))
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
        self.make_inst_with_meta_and_loc(
            opcode,
            block,
            operands,
            immediates,
            result_tys,
            flags,
            MemFlags::NONE,
            SmallVec::new(),
            None,
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
            results,
            operands,
            immediates,
            flags,
            mem_flags,
            metadata,
            loc,
            isel_strategy: None,
            param_attrs: SmallVec::new(),
            fn_attrs: crate::function::FunctionAttributes::NONE,
        };
        // Push instruction to global table first (determines Inst.0 index),
        // then record its index in the block's inst_order.
        self.insts.push(instruction);
        self.blocks[block.0 as usize].inst_order.push(inst);
        inst
    }

    /// 克隆一条指令到目标块，**保留全字段**（opcode/operands/immediates/
    /// flags/mem_flags/metadata/loc/isel_strategy——licm/loop_unroll/inline
    /// 此前手写模板会丢 flags/mem_flags/metadata）。
    ///
    /// `value_remap`（old → new）：克隆前先对 operands 应用已有映射（链式克隆
    /// 把先前克隆的值重定向）；克隆后把旧结果 → 新结果写入映射。
    /// 返回新指令。
    pub fn clone_inst(
        &mut self,
        inst: Inst,
        target_block: Block,
        value_remap: &mut HashMap<Value, Value>,
    ) -> Inst {
        let (opcode, operands, immediates, result_tys, flags, mem_flags, metadata, loc) = {
            let src = &self.insts[inst.0 as usize];
            let operands: SmallVec<[Value; 4]> = src
                .operands
                .iter()
                .map(|v| value_remap.get(v).copied().unwrap_or(*v))
                .collect();
            let result_tys: SmallVec<[TypeId; 2]> = src
                .results
                .iter()
                .map(|&r| self.values[r.0 as usize].ty)
                .collect();
            (
                src.opcode,
                operands,
                src.immediates.clone(),
                result_tys,
                src.flags,
                src.mem_flags,
                src.metadata.clone(),
                src.loc.clone(),
            )
        };
        let new_inst = self.make_inst_with_meta_and_loc(
            opcode,
            target_block,
            operands,
            immediates,
            &result_tys,
            flags,
            mem_flags,
            metadata,
            loc,
        );
        // 保留 isel_strategy / param_attrs（make_inst_* 系列不接收该字段）
        if let Some(strategy) = self.insts[inst.0 as usize].isel_strategy {
            self.insts[new_inst.0 as usize].isel_strategy = Some(strategy);
        }
        self.insts[new_inst.0 as usize].param_attrs =
            self.insts[inst.0 as usize].param_attrs.clone();
        self.insts[new_inst.0 as usize].fn_attrs = self.insts[inst.0 as usize].fn_attrs;
        // 记录旧结果 → 新结果映射（链式克隆/后续 RAUW 使用）
        let new_results: Vec<Value> = self.inst_results(new_inst).to_vec();
        for (old, &new) in self.insts[inst.0 as usize]
            .results
            .iter()
            .zip(new_results.iter())
        {
            value_remap.insert(*old, new);
        }
        new_inst
    }

    pub fn make_block(&mut self) -> Block {
        let block = Block(self.blocks.len() as u32);
        self.blocks.push(BlockData {
            params: SmallVec::new(),
            param_values: SmallVec::new(),
            inst_order: Vec::new(),
            terminator: Terminator::Unreachable,
            has_terminator: false,
        });
        block
    }

    /// 块内活跃指令的可变迭代（按 inst_order 顺序，跳过 Nop 墓碑）。
    ///
    /// 注意：迭代器持有对 `dfg` 的可变借用——期间不可同时访问 `use_lists`
    /// 等其它字段。适合"只改指令字段、不动 use-lists"的场景（打标签、
    /// 改 flags 等）；需要同步 use-lists 的修改请用「收集 Inst ID + 循环」
    /// 模式（`Function::kill_inst` / `apply_replacements`）。
    pub fn block_insts_mut(&mut self, block: Block) -> BlockInstsMut<'_> {
        BlockInstsMut {
            insts: &mut self.insts,
            ids: &self.blocks[block.0 as usize].inst_order,
            idx: 0,
        }
    }

    /// 把块 inst_order **末尾的 `count` 条指令**移动到 `insert_at` 位置之前。
    /// 统一"克隆指令后重组"语义：
    /// - `insert_at = 0`：移到块首（pgo 计数器注入）
    /// - `insert_at = call_pos`：移到 call 指令之前（inline 内联体）
    ///
    /// 等价于 `inst_order[insert_at..].rotate_right(count)`，但语义更明确。
    pub fn move_insts_to(&mut self, block: Block, insert_at: usize, count: usize) {
        let bd = &mut self.blocks[block.0 as usize];
        let len = bd.inst_order.len();
        if count == 0 || count > len {
            return;
        }
        let insert_at = insert_at.min(len - count);
        let slice = bd.inst_order.as_mut_slice();
        slice[insert_at..].rotate_right(count);
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
            has_terminator: false,
        });
        (block, params)
    }

    // === 修改 ===

    pub fn set_terminator(&mut self, block: Block, term: Terminator) {
        self.blocks[block.0 as usize].terminator = term;
        self.blocks[block.0 as usize].has_terminator = true;
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
            // 结果值 VOID 化（防下游把已删除值当活跃值读类型）
            for &r in &self.insts[idx].results {
                if let Some(vd) = self.values.get_mut(r.0 as usize) {
                    vd.ty = TypeId::VOID;
                }
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
        let insts: Vec<Inst> = std::mem::take(&mut self.blocks[block.0 as usize].inst_order);
        for inst in insts {
            if (inst.0 as usize) < self.insts.len() {
                for &r in &self.insts[inst.0 as usize].results {
                    if let Some(vd) = self.values.get_mut(r.0 as usize) {
                        vd.ty = TypeId::VOID;
                    }
                }
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
    pub fn value_count(&self) -> usize {
        self.values.len()
    }
}

impl Default for DataFlowGraph {
    fn default() -> Self {
        Self::new()
    }
}

/// 块内活跃指令的可变迭代器（见 [`DataFlowGraph::block_insts_mut`]）。
pub struct BlockInstsMut<'a> {
    insts: &'a mut Vec<Instruction>,
    ids: &'a [Inst],
    idx: usize,
}

impl<'a> Iterator for BlockInstsMut<'a> {
    type Item = &'a mut Instruction;

    fn next(&mut self) -> Option<Self::Item> {
        while let Some(&id) = self.ids.get(self.idx) {
            self.idx += 1;
            // SAFETY: `self.insts` 独占持有整个 `Vec<Instruction>`（&'a mut），
            // 索引在其生命周期内；id 来自同一 DFG 的 inst_order，恒 < len。
            // 返回的 &'a mut 引用从 &'a mut Vec 派生，是标准迭代器模式。
            let inst = unsafe { &mut *self.insts.as_mut_ptr().add(id.0 as usize) };
            if matches!(inst.opcode, Opcode::Nop) && inst.results.is_empty() {
                continue;
            }
            return Some(inst);
        }
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn test_types() -> (TypeId, TypeId, TypeId) {
        (TypeId(2), TypeId(2), TypeId(1))
    }

    fn test_make_block() {
        let mut dfg = DataFlowGraph::new();
        let b = dfg.make_block();
        assert_eq!(dfg.block_count(), 1);
        assert!(matches!(
            dfg.block_terminator(b).unwrap(),
            Terminator::Unreachable
        ));
    }

    /// block_insts_mut：按 inst_order 可变迭代，跳过 Nop 墓碑。
    fn test_block_insts_mut() {
        let mut dfg = DataFlowGraph::new();
        let b = dfg.make_block();
        // 构造 2 条活跃指令 + 1 条墓碑
        let ty = TypeId::I32;
        let i1 = dfg.make_inst(
            Opcode::Iconst,
            b,
            SmallVec::new(),
            smallvec::smallvec![Immediate::Const(
                crate::constant::ConstantPool::new().insert_int(1, 32)
            )],
            &[ty],
            InstFlags::NONE,
        );
        let i2 = dfg.make_inst(
            Opcode::Iconst,
            b,
            SmallVec::new(),
            smallvec::smallvec![Immediate::Const(
                crate::constant::ConstantPool::new().insert_int(2, 32)
            )],
            &[ty],
            InstFlags::NONE,
        );
        dfg.remove_inst(i1); // 墓碑

        // 遍历活跃指令：应只剩 i2，且可打 isel_strategy 标签
        let mut visited = 0;
        for inst in dfg.block_insts_mut(b) {
            assert_eq!(inst.opcode, Opcode::Iconst);
            inst.isel_strategy = Some("tagged");
            visited += 1;
        }
        assert_eq!(visited, 1, "墓碑指令应被跳过");
        assert_eq!(
            dfg.insts[i2.0 as usize].isel_strategy,
            Some("tagged"),
            "可变迭代应写回"
        );
    }

    fn test_make_block_with_params() {
        let mut dfg = DataFlowGraph::new();
        let (i32_ty, _, _) = test_types();
        let (block, params) = dfg.make_block_with_params(&[i32_ty, i32_ty]);
        assert_eq!(params.len(), 2);
        let v0 = dfg.value_def(params[0]).unwrap();
        assert!(matches!(v0, ValueDef::Param(b, 0) if b.0 == block.0));
    }

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


}
