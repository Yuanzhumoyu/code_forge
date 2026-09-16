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
use super::terminator::TermKind;
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
///
/// **块内顺序的唯一事实源是 `BlockData.inst_order`**；`Instruction.block` 只记
/// 归属块。历史实现还有一个 `pos: u32`（创建时写一次、**全仓无读取点**，删/移
/// 指令后即陈旧）——它是第二份顺序信息，已删除（2026-09-14）。
#[derive(Clone, Debug)]
pub struct Instruction {
    pub opcode: Opcode,
    pub block: Block,
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
    ///
    /// **单写**（v3 方案 S5）：读 [`Instruction::metadata`]、写
    /// [`Instruction::attach_metadata`]；字段私有，避免"整表替换 / 绕过追加约定"
    /// 的第三条路径。创建期由 `make_inst_with_meta_and_loc` 接收初始表。
    pub(crate) metadata: SmallVec<[AttachedMetadata; 2]>,
    /// Source location — file, line, column for debugging and diagnostics.
    pub loc: Option<SourceLocation>,
    /// Instruction-selection strategy tag produced by pattern matching
    /// (Stage 3). When set, the lowering pass should emit the fused machine
    /// sequence named by the tag instead of lowering the opcode verbatim
    /// (e.g. `"lea_sib:4"` for `Iadd(Imul(idx,4), base)` → LEA).
    /// `None` = default lowering.
    ///
    /// 类型见 [`crate::isel_strategy::IselStrategy`]：标签名由目标 ISA 数据
    /// 决定，forge-ir 不认识任何具体名字（无枚举/白名单/长度限制）。
    ///
    /// **字段私有**（v3 方案 S5 可见性）：读 [`Instruction::isel_strategy`]、
    /// 写 [`Instruction::set_isel_strategy`] / [`Instruction::clear_isel_strategy`]
    /// ——与 `BlockData.terminator` 同一约定，附件只经入口改。
    pub(crate) isel_strategy: Option<crate::isel_strategy::IselStrategy>,
    /// **墓碑标志**（v3 方案 S2：墓碑语义显式化）。
    ///
    /// 删除是"标墓碑"而非回收槽位：`remove_inst` 把块内顺序表条目摘掉，就地墓碑化
    /// 的调用方（`Function::tombstone_inst`）保留条目。此前"是不是墓碑"靠
    /// `opcode == Nop` 猜——而 `Opcode::Nop` **是合法指令**（`Builder::nop()` 会发
    /// 一条进 `inst_order`），于是同一个问题在全仓有两种答案（`Nop` / `Nop &&
    /// results.is_empty()`），合法 Nop 会被当成墓碑跳过。
    ///
    /// 现在唯一事实源是本字段：读 [`Instruction::is_tombstone`]，写经
    /// [`DataFlowGraph::tombstone_inst`]（crate 内）/ [`crate::Function::tombstone_inst`]
    /// （跨 crate，附带 use-lists 重登记）。字段私有。
    pub(crate) tombstone: bool,
}

impl Instruction {
    /// 该指令是否已被墓碑化（**唯一判据**，见 `Instruction::tombstone` 字段文档）。
    ///
    /// 与"`opcode == Nop`"的区别：`Opcode::Nop` 是合法指令（`Builder::nop()`），
    /// 只有本判据为真才是"删除后的占位"。
    pub fn is_tombstone(&self) -> bool {
        self.tombstone
    }

    /// 指令附件 metadata（`(kind, node)` 对，按附加序）。
    pub fn metadata(&self) -> &[AttachedMetadata] {
        &self.metadata
    }

    /// 附加一条 metadata —— **指令附件的唯一写入口**（S5：metadata 单写）。
    ///
    /// 追加语义：同一 kind 再次附加即多一条（与文本里多处 `!dbg !N` 一一对应；
    /// 重复 kind 的合并/拒绝不在本步范围）。初始表请走
    /// [`DataFlowGraph::make_inst_with_meta_and_loc`]。
    pub fn attach_metadata(&mut self, metadata: AttachedMetadata) {
        self.metadata.push(metadata);
    }

    /// 指令选择标签；`None` = 无标签（按 opcode 逐条降级）。
    pub fn isel_strategy(&self) -> Option<&crate::isel_strategy::IselStrategy> {
        self.isel_strategy.as_ref()
    }

    /// 写指令选择标签（覆盖既有标签）。
    pub fn set_isel_strategy(&mut self, strategy: crate::isel_strategy::IselStrategy) {
        self.isel_strategy = Some(strategy);
    }

    /// 摘除指令选择标签，返回被摘下的那个。
    pub fn clear_isel_strategy(&mut self) -> Option<crate::isel_strategy::IselStrategy> {
        self.isel_strategy.take()
    }
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
    /// 终结符**指令**句柄（v3 方案 S4 主体）。
    ///
    /// 终结符就是一条指令：存 [`DataFlowGraph::insts`]，有 opcode/operands/
    /// immediates/metadata，**同样进 use-def**；但它**不进 `inst_order`**
    /// （块内指令列表的语义保持不变：只含非终结符指令）。
    ///
    /// `None` = 该块尚未终止（构建中/坏 IR）：旧表示用"默认 `Unreachable` +
    /// `has_terminator` 布尔位"编码这件事，会让"漏写终结符"看起来像"显式
    /// unreachable"；现在两件事在类型上分开。
    pub(crate) terminator: Option<Inst>,
}

impl BlockData {
    /// 终结符指令句柄。**块未终止则 panic**（fail-closed）。
    ///
    /// "没有终结符"是**构建中/坏 IR** 状态，不是一种终结符：静默回退成
    /// `Unreachable` 正是 S4-c 消灭掉的伪装。需要容忍该状态的调用方
    /// （**校验器**、builder、display、解析器）请用 [`BlockData::terminator_opt`]。
    pub fn terminator(&self) -> Inst {
        self.terminator
            .unwrap_or_else(|| panic!("块尚未终止（无终结符指令）：读取方应改用 terminator_opt()"))
    }

    /// 终结符指令句柄；`None` = 该块尚未终止（见 [`BlockData::terminator`]）。
    pub fn terminator_opt(&self) -> Option<Inst> {
        self.terminator
    }
}

/// [`DataFlowGraph::attach_term_metadata`] 的结果（三种"没写成"的原因要分开报）。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TermMetadataAttach {
    /// 已附加到终结符指令。
    Attached,
    /// 块尚未终止（构建中/坏 IR）。
    NoTerminator,
    /// 终结符是 `Unreachable`——文本里没有可挂 metadata 的位置。
    Unreachable,
}

/// `switch` 终结符的解码视图（case 表在指令 immediates 里，这里结构化）。
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SwitchView<'a> {
    /// 判别值。
    pub discriminant: Value,
    /// default 目标。
    pub default_block: Block,
    /// default 实参。
    pub default_args: &'a [Value],
    /// case 表（声明序）。
    pub cases: Vec<SwitchCaseView<'a>>,
}

/// `switch` 的单个 case。
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SwitchCaseView<'a> {
    /// case 常量。
    pub value: i64,
    /// 目标块。
    pub target: Block,
    /// 传给目标块的实参。
    pub args: &'a [Value],
}

/// 取第 `i` 个 immediate 的块（非 `Block` 则 `None`）。
fn imm_block(imms: &[Immediate], i: usize) -> Option<Block> {
    match imms.get(i) {
        Some(Immediate::Block(b)) => Some(*b),
        _ => None,
    }
}

/// 取第 `i` 个 immediate 的无符号数（非 `Uint` 则 `None`）。
fn imm_uint(imms: &[Immediate], i: usize) -> Option<usize> {
    match imms.get(i) {
        Some(Immediate::Uint(v)) => Some(*v as usize),
        _ => None,
    }
}

// ============================================================
// DataFlowGraph
// ============================================================

#[derive(Clone, Debug)]
pub struct DataFlowGraph {
    /// 值 arena。**私有**（v3 方案 S5：`dfg` 私有化 + 受限编辑 API）。
    ///
    /// 读：`value_data`（fail-closed）/ `value_data_opt` / `value_def` /
    /// `value_type` / `values()` / `value_count`；
    /// 写：`set_value_type`（pass 精化值类型）——其余写入（创建、墓碑化）只在
    /// 本文件内。`insts`/`blocks` 两个 arena 的私有化是后续切片。
    pub(crate) values: Vec<ValueData>,
    /// 指令 arena。**私有**（v3 方案 S5：`dfg` 私有化 + 受限编辑 API）。
    ///
    /// 读：`inst_data`（fail-closed）/ `inst_data_opt` / `inst_opcode` /
    /// `inst_operands` / `inst_results` / `inst_block` / `insts()` / `inst_count`；
    /// 写：`inst_mut` / `inst_mut_opt`（`(dfg, Inst)` 寻址的就地编辑口，改
    /// `operands` 后必须 `Function::refresh_inst_uses`）；结构性增删走
    /// `make_inst*` / `remove_inst`（在 `dfg.rs` 内）。`blocks` arena 的收口是
    /// 后续切片。
    pub(crate) insts: Vec<Instruction>,
    /// 块 arena。**私有**（v3 方案 S5：`dfg` 私有化 + 受限编辑 API）。
    ///
    /// 读：`block`（fail-closed）/ `block_opt`（容忍坏 IR）/ `block_data_iter` /
    /// `blocks()` / `block_count` / `block_params` / `block_param_values` /
    /// `block_terminator` / `block_inst_iter`；
    /// 写：`block_mut`（就地编辑口，契约见其文档）；结构性增删只有
    /// `make_block*` / `remove_block`（在 `dfg.rs` 内）。
    pub(crate) blocks: Vec<BlockData>,
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
        let instruction = Instruction {
            opcode,
            block,
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
            tombstone: false,
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
        if let Some(strategy) = self.insts[inst.0 as usize].isel_strategy.clone() {
            self.insts[new_inst.0 as usize].set_isel_strategy(strategy);
        }
        self.insts[new_inst.0 as usize].tombstone = self.insts[inst.0 as usize].tombstone;
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
            terminator: None,
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
            terminator: None,
        });
        (block, params)
    }

    // === 修改 ===

    /// 写块终结符的**低层原语**：把终结符编码成一条指令并挂到块上。
    ///
    /// 编码约定见 [`DataFlowGraph::term_kind`] 上方的说明 + `ops.toml`。
    /// 只动 DFG，**不动 use-lists**：调用方（[`crate::Function::set_terminator`]）
    /// 必须在写前摘除旧用值、写后登记新用值。
    pub(crate) fn set_terminator(
        &mut self,
        block: Block,
        opcode: Opcode,
        operands: SmallVec<[Value; 4]>,
        immediates: SmallVec<[Immediate; 4]>,
    ) {
        if let Some(old) = self.blocks[block.0 as usize].terminator.take() {
            self.remove_inst(old);
        }
        let inst = Inst(self.insts.len() as u32);
        self.insts.push(Instruction {
            opcode,
            block,
            results: SmallVec::new(),
            operands,
            immediates,
            flags: InstFlags::NONE,
            mem_flags: MemFlags::NONE,
            metadata: SmallVec::new(),
            loc: None,
            isel_strategy: None,
            param_attrs: SmallVec::new(),
            fn_attrs: crate::function::FunctionAttributes::NONE,
            tombstone: false,
        });
        // **不登记 inst_order**：块内指令列表只含非终结符指令。
        self.blocks[block.0 as usize].terminator = Some(inst);
    }

    /// **墓碑化的唯一实现**（crate 内低层原语，S2）：标标志 + 清空指令内容与附件。
    ///
    /// 与"合法 `nop`"无关：只有经这里的指令 `is_tombstone()` 才为真。
    /// **不动 `inst_order`**（`remove_inst` 负责摘条目；就地墓碑化的调用方保留
    /// 条目）、**不动 use-lists**（跨 crate 调用方请用
    /// [`crate::Function::tombstone_inst`]）、**不动 `results`**——部分就地墓碑化
    /// 的调用方（大聚合展开）之后仍要读那条被作废指令的结果值；"删除语义"的
    /// 结果处理由 [`DataFlowGraph::remove_inst`] 追加（清空 + VOID）。
    pub(crate) fn tombstone_inst_low(&mut self, inst: Inst) {
        let idx = inst.0 as usize;
        if idx >= self.insts.len() {
            return;
        }
        let i = &mut self.insts[idx];
        i.tombstone = true;
        i.opcode = Opcode::Nop;
        i.operands.clear();
        i.immediates.clear();
        // 附件一并清掉：墓碑上的 metadata/isel_strategy/param_attrs 是陈旧状态，
        // 只会误导 dump 与诊断（"删了但看起来还带 TBAA/融合标签"）。
        i.metadata.clear();
        i.param_attrs.clear();
        i.fn_attrs = crate::function::FunctionAttributes::NONE;
        i.isel_strategy = None;
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
            self.tombstone_inst_low(inst);
            // 删除语义额外处理结果值：清空 + VOID（防下游把已删除值当活跃值读类型）
            for r in std::mem::take(&mut self.insts[idx].results) {
                if let Some(vd) = self.values.get_mut(r.0 as usize) {
                    vd.ty = TypeId::VOID;
                }
            }
        }
    }

    pub fn remove_block(&mut self, block: Block) {
        // 终结符指令也一并墓碑化（它不在 inst_order 里）
        if let Some(term) = self.blocks[block.0 as usize].terminator.take() {
            self.remove_inst(term);
        }
        // 块内指令全部墓碑化（并摘掉顺序表条目）
        let insts: Vec<Inst> = std::mem::take(&mut self.blocks[block.0 as usize].inst_order);
        for inst in insts {
            self.tombstone_inst_low(inst);
        }
    }

    // === 查询 ===

    pub fn value_def(&self, v: Value) -> Option<&ValueDef> {
        self.values.get(v.0 as usize).map(|d| &d.def)
    }
    pub fn value_type(&self, v: Value) -> Option<TypeId> {
        self.values.get(v.0 as usize).map(|d| d.ty)
    }

    /// 值的完整数据（`def` + `ty`）。**句柄不合法即 panic**（fail-closed）：
    /// 越界句柄/来自别的 DFG 的句柄都是坏 IR，静默返回默认值只会让错误漂远
    /// （与 `BlockData::terminator` 同一契约）。必须容忍坏 IR 的调用方
    /// （校验器/display/lowering 的容忍路径）请用 [`DataFlowGraph::value_data_opt`]。
    pub fn value_data(&self, v: Value) -> &ValueData {
        self.values
            .get(v.0 as usize)
            .unwrap_or_else(|| panic!("值句柄不合法（越界或来自别的 DFG）：{v:?}"))
    }

    /// 值的完整数据；句柄不合法返回 `None`（见 [`DataFlowGraph::value_data`]）。
    pub fn value_data_opt(&self, v: Value) -> Option<&ValueData> {
        self.values.get(v.0 as usize)
    }

    /// 改写值的类型 —— **值 arena 的唯一写入口**（受限编辑 API，v3 S5）：
    /// pass 精化/统一类型时用（常量折叠定宽、GVN 折叠类型、代数化简结果类型）。
    ///
    /// 返回是否写入：句柄不合法（越界/墓碑）时**不写**并返回 `false`
    /// ——与读口 [`DataFlowGraph::value_type`] 的 `Option` 一致，不 panic。
    pub fn set_value_type(&mut self, v: Value, ty: TypeId) -> bool {
        match self.values.get_mut(v.0 as usize) {
            Some(vd) => {
                vd.ty = ty;
                true
            }
            None => false,
        }
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

    /// 指令的完整数据。**句柄不合法即 panic**（fail-closed，与
    /// [`DataFlowGraph::value_data`] 同一契约）：越界句柄/墓碑句柄都是坏 IR。
    /// 容忍坏 IR 的调用方（校验器/display）请用 [`DataFlowGraph::inst_data_opt`]。
    pub fn inst_data(&self, i: Inst) -> &Instruction {
        self.insts
            .get(i.0 as usize)
            .unwrap_or_else(|| panic!("指令句柄不合法（越界或来自别的 DFG）：{i:?}"))
    }

    /// 指令的完整数据；句柄不合法返回 `None`（见 [`DataFlowGraph::inst_data`]）。
    pub fn inst_data_opt(&self, i: Inst) -> Option<&Instruction> {
        self.insts.get(i.0 as usize)
    }

    /// 指令的可变编辑口 —— **指令 arena 的唯一外部可变入口**（v3 S5）。
    ///
    /// # 契约（改错字段会破坏不变量）
    ///
    /// - 安全字段：`opcode`/`immediates`/`flags`/`mem_flags`/`param_attrs`/
    ///   `fn_attrs`/`metadata`/`loc`/`isel_strategy`/`block`（块归属变更是 S2 余项）。
    /// - **`operands`/`results` 不在此列**：改操作数会绕过 use-lists，必须走
    ///   [`crate::Function::replace_all_uses`]/`apply_replacements`，或"就地改写 +
    ///   [`crate::Function::refresh_inst_uses`] 重登记"（后者是本口的既有用法）。
    /// - 结构性增删（push/remove）不在此列：用 `make_inst*`/`remove_inst`。
    ///
    /// 句柄不合法即 panic（fail-closed）。
    pub fn inst_mut(&mut self, i: Inst) -> &mut Instruction {
        let idx = i.0 as usize;
        if idx >= self.insts.len() {
            panic!("指令句柄不合法（越界或来自别的 DFG）：{i:?}");
        }
        &mut self.insts[idx]
    }

    /// 指令的可变编辑口；句柄不合法返回 `None`（见 [`DataFlowGraph::inst_mut`]）。
    pub fn inst_mut_opt(&mut self, i: Inst) -> Option<&mut Instruction> {
        self.insts.get_mut(i.0 as usize)
    }

    /// crate 内批量就地编辑迭代（`(Inst, &mut Instruction)`，含墓碑槽位）。
    ///
    /// **仅供 crate 内实现用**（如 `Function::apply_replacements`）：与
    /// [`DataFlowGraph::inst_mut`] 同一契约——改 `operands` 后必须同步 use-lists。
    pub(crate) fn insts_iter_mut(&mut self) -> impl Iterator<Item = (Inst, &mut Instruction)> {
        self.insts
            .iter_mut()
            .enumerate()
            .map(|(k, inst)| (Inst(k as u32), inst))
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

    /// 块终结符的**指令句柄**（块不存在或尚未终止则 `None`）。
    ///
    /// S4 主体起终结符就是一条指令（存 `insts`，不进 `inst_order`）；
    /// 读取请优先用投影访问器（[`DataFlowGraph::term_kind`] 等）。
    pub fn block_terminator(&self, b: Block) -> Option<Inst> {
        self.blocks.get(b.0 as usize).and_then(|d| d.terminator)
    }

    /// 块的后继（去重保序；未终止的块没有后继）。
    pub fn block_successors(&self, b: Block) -> Vec<Block> {
        let mut out: Vec<Block> = Vec::new();
        let mut push = |t: Block| {
            if !out.contains(&t) {
                out.push(t);
            }
        };
        match self.term_kind(b) {
            Some(TermKind::Jump) => {
                if let Some((t, _)) = self.term_jump(b) {
                    push(t);
                }
            }
            Some(TermKind::Branch) => {
                if let Some((_, t, _, e, _)) = self.term_branch(b) {
                    push(t);
                    push(e);
                }
            }
            Some(TermKind::Switch) => {
                if let Some(v) = self.term_switch(b) {
                    push(v.default_block);
                    for case in &v.cases {
                        push(case.target);
                    }
                }
            }
            Some(TermKind::Invoke) => {
                if let Some((_, _, _, n, _, u, _)) = self.term_invoke(b) {
                    push(n);
                    push(u);
                }
            }
            _ => {}
        }
        out
    }

    // ============================================================
    // 终结符投影访问器（S4-d 建立；S4 主体起解码终结符**指令**）
    // ============================================================
    //
    // 终结符是一条指令（opcode ∈ {Ret, Jmp, Br, Switch, Unreachable, Invoke,
    // Resume}），载荷按 ops.toml「终结符」节的约定编码：
    //   Ret          operands = 返回值
    //   Jmp          operands = 实参；immediates = [Block(target)]
    //   Br           operands = [cond, then_args…, else_args…]；
    //                immediates = [Block(then), Block(else), Uint(then_argc), Uint(else_argc)]
    //   Switch       operands = [disc, default_args…, case_args…]；
    //                immediates = [Block(default), Uint(default_argc),
    //                              Uint(case_count), (Int, Block, Uint(argc))…]
    //   Unreachable  operands = []
    //   Invoke       operands = [args…, normal_args…, unwind_args…]；
    //                immediates = [Func, Block(normal), Block(unwind), Uint(argc),
    //                              Uint(normal_argc), Uint(unwind_argc), Type(ret_ty)]
    //   Resume       operands = [value]
    // **operands 的顺序就是终结符用值的规范序**（use-list 下标即操作数下标）。

    fn term_inst(&self, b: Block) -> Option<&Instruction> {
        let inst = self.block_terminator(b)?;
        self.insts.get(inst.0 as usize)
    }

    /// 终结符种类（块未终止 = `None`）。
    pub fn term_kind(&self, b: Block) -> Option<TermKind> {
        super::terminator::term_kind_of(&self.term_inst(b)?.opcode)
    }

    /// 终结符附件元数据（无终结符 / `unreachable` 则空切片）。
    pub fn term_metadata(&self, b: Block) -> &[AttachedMetadata] {
        match self.term_kind(b) {
            None | Some(TermKind::Unreachable) => &[],
            Some(_) => self
                .term_inst(b)
                .map(|i| i.metadata.as_slice())
                .unwrap_or(&[]),
        }
    }

    /// 给块终结符附加 metadata —— **终结符附件的唯一写入口**（S5：metadata 单写）。
    ///
    /// 取代了此前返回 `&mut SmallVec` 的 `term_metadata_mut`：那种"逃逸可变引用"
    /// 让调用方可以整表替换、清空或乱序，附件约定无处安放。
    ///
    /// 返回 `Unreachable` / `NoTerminator` 时**不写**（调用方据此给精确诊断）：
    /// `unreachable` 在文本里没有可挂 metadata 的位置，未终止则是坏 IR。
    pub fn attach_term_metadata(
        &mut self,
        b: Block,
        metadata: AttachedMetadata,
    ) -> TermMetadataAttach {
        if self.block_terminator(b).is_none() {
            return TermMetadataAttach::NoTerminator;
        }
        if self.term_kind(b) == Some(TermKind::Unreachable) {
            return TermMetadataAttach::Unreachable;
        }
        let Some(inst) = self.block_terminator(b) else {
            return TermMetadataAttach::NoTerminator;
        };
        match self.insts.get_mut(inst.0 as usize) {
            Some(i) => {
                i.attach_metadata(metadata);
                TermMetadataAttach::Attached
            }
            None => TermMetadataAttach::NoTerminator,
        }
    }

    /// 分支形式：`(cond, then_block, then_args, else_block, else_args)`。
    #[allow(clippy::type_complexity)]
    pub fn term_branch(&self, b: Block) -> Option<(Value, Block, &[Value], Block, &[Value])> {
        let inst = self.term_inst(b)?;
        if inst.opcode != Opcode::Br {
            return None;
        }
        let cond = *inst.operands.first()?;
        let then_block = imm_block(&inst.immediates, 0)?;
        let else_block = imm_block(&inst.immediates, 1)?;
        let then_argc = imm_uint(&inst.immediates, 2)?;
        let else_argc = imm_uint(&inst.immediates, 3)?;
        let rest = inst.operands.get(1..)?;
        if then_argc + else_argc != rest.len() {
            return None; // 编码自洽性：两个 argc 必须覆盖全部实参
        }
        let (then_args, else_args) = rest.split_at(then_argc);
        Some((cond, then_block, then_args, else_block, else_args))
    }

    /// 无条件跳转形式：`(target, args)`。
    pub fn term_jump(&self, b: Block) -> Option<(Block, &[Value])> {
        let inst = self.term_inst(b)?;
        if inst.opcode != Opcode::Jmp {
            return None;
        }
        Some((imm_block(&inst.immediates, 0)?, &inst.operands))
    }

    /// 返回形式的返回值切片（非 `Return` 则 `None`）。
    pub fn term_return_values(&self, b: Block) -> Option<&[Value]> {
        let inst = self.term_inst(b)?;
        (inst.opcode == Opcode::Ret).then_some(inst.operands.as_slice())
    }

    /// switch 形式（解码成结构化视图；case 表在 immediates 里）。
    pub fn term_switch(&self, b: Block) -> Option<SwitchView<'_>> {
        let inst = self.term_inst(b)?;
        if inst.opcode != Opcode::Switch {
            return None;
        }
        let discriminant = *inst.operands.first()?;
        let default_block = imm_block(&inst.immediates, 0)?;
        let default_argc = imm_uint(&inst.immediates, 1)?;
        let case_count = imm_uint(&inst.immediates, 2)?;
        let rest = inst.operands.get(1..)?;
        let default_args = rest.get(..default_argc.min(rest.len()))?;
        let mut cursor = default_args.len();
        let mut cases = Vec::with_capacity(case_count);
        for c in 0..case_count {
            let base = 3 + c * 3;
            let value = match inst.immediates.get(base) {
                Some(Immediate::Int(v)) => *v,
                _ => return None,
            };
            let target = imm_block(&inst.immediates, base + 1)?;
            let argc = imm_uint(&inst.immediates, base + 2)?;
            let args = rest.get(cursor..cursor + argc)?;
            cursor += argc;
            cases.push(SwitchCaseView {
                value,
                target,
                args,
            });
        }
        Some(SwitchView {
            discriminant,
            default_block,
            default_args,
            cases,
        })
    }

    /// invoke 形式：`(callee, args, ret_ty, normal_block, normal_args, unwind_block, unwind_args)`。
    #[allow(clippy::type_complexity)]
    pub fn term_invoke(
        &self,
        b: Block,
    ) -> Option<(FuncRef, &[Value], TypeId, Block, &[Value], Block, &[Value])> {
        let inst = self.term_inst(b)?;
        if inst.opcode != Opcode::Invoke {
            return None;
        }
        let callee = match inst.immediates.first()? {
            Immediate::Func(f) => *f,
            _ => return None,
        };
        let normal_block = imm_block(&inst.immediates, 1)?;
        let unwind_block = imm_block(&inst.immediates, 2)?;
        let argc = imm_uint(&inst.immediates, 3)?;
        let normal_argc = imm_uint(&inst.immediates, 4)?;
        let unwind_argc = imm_uint(&inst.immediates, 5)?;
        let ret_ty = match inst.immediates.get(6)? {
            Immediate::Type(t) => *t,
            _ => return None,
        };
        if argc + normal_argc + unwind_argc != inst.operands.len() {
            return None;
        }
        let args = inst.operands.get(..argc)?;
        let normal_args = inst.operands.get(argc..argc + normal_argc)?;
        let unwind_args = inst.operands.get(argc + normal_argc..)?;
        Some((
            callee,
            args,
            ret_ty,
            normal_block,
            normal_args,
            unwind_block,
            unwind_args,
        ))
    }

    /// `resume` 的用值（非 `Resume` 则 `None`）。
    pub fn term_resume_value(&self, b: Block) -> Option<Value> {
        let inst = self.term_inst(b)?;
        if inst.opcode != Opcode::Resume {
            return None;
        }
        inst.operands.first().copied()
    }

    /// 是否为显式 `unreachable`。
    pub fn term_is_unreachable(&self, b: Block) -> bool {
        self.term_kind(b) == Some(TermKind::Unreachable)
    }

    /// 传给 `target` 的实参（非该目标的分支则空切片）。
    pub fn term_args_to(&self, b: Block, target: Block) -> &[Value] {
        match self.term_kind(b) {
            Some(TermKind::Jump) => match self.term_jump(b) {
                Some((t, args)) if t == target => args,
                _ => &[],
            },
            Some(TermKind::Branch) => match self.term_branch(b) {
                Some((_, t, targs, e, eargs)) => {
                    if t == target {
                        targs
                    } else if e == target {
                        eargs
                    } else {
                        &[]
                    }
                }
                None => &[],
            },
            Some(TermKind::Switch) => match self.term_switch(b) {
                Some(v) => {
                    if v.default_block == target {
                        v.default_args
                    } else {
                        v.cases
                            .iter()
                            .find(|c| c.target == target)
                            .map(|c| c.args)
                            .unwrap_or(&[])
                    }
                }
                None => &[],
            },
            Some(TermKind::Invoke) => match self.term_invoke(b) {
                Some((_, _, _, n, nargs, u, uargs)) => {
                    if n == target {
                        nargs
                    } else if u == target {
                        uargs
                    } else {
                        &[]
                    }
                }
                None => &[],
            },
            _ => &[],
        }
    }

    /// 按规范序遍历终结符用值（= 终结符指令的 operands 顺序）。
    pub fn for_each_term_value(&self, b: Block, mut f: impl FnMut(u32, Value)) {
        if let Some(inst) = self.term_inst(b) {
            for (i, v) in inst.operands.iter().enumerate() {
                f(i as u32, *v);
            }
        }
    }

    /// 终结符用值（规范序）。
    pub fn term_used_values(&self, b: Block) -> Vec<Value> {
        self.term_inst(b)
            .map(|i| i.operands.to_vec())
            .unwrap_or_default()
    }

    /// 块是否已终止（显式设置了 ret/jmp/br/unreachable/switch/invoke/resume 之一）。
    pub fn block_has_terminator(&self, b: Block) -> bool {
        self.blocks
            .get(b.0 as usize)
            .is_some_and(|d| d.terminator.is_some())
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
            .filter(|(_, id)| !id.is_tombstone())
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
                    .and_then(|&inst| self.dfg.inst_data_opt(inst))
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

    /// 按 Block 索引获取 BlockData。**句柄不合法即 panic**（fail-closed，
    /// 与 `value_data`/`inst_data` 同一契约）。容忍坏 IR 的调用方用
    /// [`DataFlowGraph::block_opt`]。
    pub fn block(&self, b: Block) -> &BlockData {
        self.blocks
            .get(b.0 as usize)
            .unwrap_or_else(|| panic!("块句柄不合法（越界或来自别的 DFG）：{b:?}"))
    }

    /// 按 Block 索引获取 BlockData；句柄不合法返回 `None`。
    pub fn block_opt(&self, b: Block) -> Option<&BlockData> {
        self.blocks.get(b.0 as usize)
    }

    /// 按 Block 索引获取可变 BlockData（**块 arena 的唯一外部可变入口**）。
    ///
    /// 契约：`params`/`param_values` 决定块参数集合，改它们必须与
    /// `Function::add_block_param`/`remove_block_param` 保持口径一致（它们会同步
    /// 值表与 use-lists）；`inst_order` 是**块内指令顺序的唯一事实源**，只在
    /// "指令插入/搬移"实现里改（`make_inst*` 追加、`move_insts_to` 重排）；
    /// `terminator` 字段已私有，写终结符走 `Function::{jump, branch, ret, …}`。
    pub fn block_mut(&mut self, b: Block) -> &mut BlockData {
        let idx = b.0 as usize;
        if idx >= self.blocks.len() {
            panic!("块句柄不合法（越界或来自别的 DFG）：{b:?}");
        }
        &mut self.blocks[idx]
    }

    /// **原始 arena 迭代**（含墓碑）：`insts()` 会跳过墓碑，供校验器等"必须看到
    /// 全部槽位"的调用方使用。crate 内专用。
    pub(crate) fn all_insts(&self) -> impl Iterator<Item = (Inst, &Instruction)> {
        self.insts
            .iter()
            .enumerate()
            .map(|(k, inst)| (Inst(k as u32), inst))
    }

    /// 按块序迭代 `BlockData`（不带句柄）——[`DataFlowGraph::blocks`] 的无句柄版，
    /// 供"只关心内容"的遍历（`enumerate()` 得到的下标即块序）。
    pub fn block_data_iter(&self) -> impl Iterator<Item = &BlockData> {
        self.blocks.iter()
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
            if inst.is_tombstone() {
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

    #[test] // 恢复丢失的 #[test]（v14 重构时属性遗失，clippy dead_code 暴露）
    fn test_make_block() {
        let mut dfg = DataFlowGraph::new();
        let b = dfg.make_block();
        assert_eq!(dfg.block_count(), 1);
        // 新建块**尚未终止**：`None` 而不是默认 `Unreachable`
        //（后者会把"漏写终结符"伪装成"显式 unreachable"，S4-c 起不再如此）
        assert!(dfg.block_terminator(b).is_none());
        assert!(!dfg.block_has_terminator(b));
    }

    /// block_insts_mut：按 inst_order 可变迭代，跳过 Nop 墓碑。
    #[test]
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
            inst.set_isel_strategy(crate::isel_strategy::IselStrategy::from_static("tagged"));
            visited += 1;
        }
        assert_eq!(visited, 1, "墓碑指令应被跳过");
        assert_eq!(
            dfg.inst_data(i2)
                .isel_strategy()
                .map(crate::isel_strategy::IselStrategy::name),
            Some("tagged"),
            "可变迭代应写回"
        );
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
}
