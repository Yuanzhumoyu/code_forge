//! TargetLowering — 指令选择接口。
//!
//! 将 IR 操作码和终止指令降低为微指令包（InstPacket）。
//! 输入包含 XReg（临时寄存器）操作数，输出为指令包：微指令数组 +
//! 临时寄存器分配器 + XReg→微指令寄存器字段映射 + 输入/输出。

use crate::{LowerCtx, VBlockId};
use forge_ir::{Block, IrError, Opcode, Terminator, Value, XReg, XRegAllocator};
use std::collections::HashMap;

/// 指令包 — DSL 中间指令（IR op）映射到微指令的返回结构，取代旧的 `Vec<Inst>`。
///
/// 各字段语义：
/// - `xregs`：临时寄存器分配器。指令包内新分配的 XReg 由它管理；
///   组合/展开多个指令包时，子包分配器可并入父包。
/// - `insts`：微指令数组。寄存器字段（Ireg/Freg）在构造时以**默认寄存器**占位
///   （对应类型的 0 号物理寄存器），由寄存器分配器分配后回填真正寄存器。
/// - `xreg_map`：临时寄存器 → 微指令寄存器（物理 Reg）字段的映射，与 `insts` **平行**：
///   每项是该微指令的 Ireg/Freg 字段对应的 XReg 列表（按字段声明顺序）。
///   分配器按此回填字段：`insts[i]` 的第 `j` 个寄存器字段 = `xreg_map[i][j]` 的分配结果。
/// - `inputs` / `outputs`：指令包的输入/输出，类型为临时寄存器（XReg）。
///   供指令包组合/展开时连接（A 的 output 连 B 的 input 为同一 XReg）。
#[derive(Clone, Debug, Default)]
pub struct InstPacket<I> {
    /// 临时寄存器分配器。
    pub xregs: XRegAllocator,
    /// 微指令数组（寄存器字段以默认物理寄存器占位）。
    pub insts: Vec<I>,
    /// XReg → 微指令寄存器字段映射（与 insts 平行；每条指令一个
    /// SmallVec<[XReg, field_idx, is_def; 2]>，is_def 区分该 XReg 是输出还是输入）。
    pub xreg_map: Vec<smallvec::SmallVec<[(XReg, u8, bool); 2]>>,
    /// 指令包输入（XReg）。
    pub inputs: Vec<XReg>,
    /// 指令包输出（XReg）。
    pub outputs: Vec<XReg>,
}

impl<I> InstPacket<I> {
    /// 创建空的指令包。
    pub fn new() -> Self {
        Self {
            xregs: XRegAllocator::new(),
            insts: Vec::new(),
            xreg_map: Vec::new(),
            inputs: Vec::new(),
            outputs: Vec::new(),
        }
    }

    /// 添加一条微指令并返回其索引（xreg_map 同步推入空映射槽）。
    pub fn push_inst(&mut self, inst: I) -> usize {
        self.insts.push(inst);
        self.xreg_map.push(smallvec::SmallVec::new());
        self.insts.len() - 1
    }

    /// 记录 XReg → 微指令寄存器字段（追加到 `xreg_map[inst_idx]`，字段顺序即记录顺序）。
    /// `is_def = true` 表示该 XReg 是这条指令的输出（def），false 为输入（use）。
    /// 区分 use/def 是 regalloc 活区间正确性的前提（use 在 inst_start、def 在 inst_end）。
    pub fn map_reg_field(&mut self, xreg: XReg, inst_idx: usize, field_idx: u8, is_def: bool) {
        if let Some(slot) = self.xreg_map.get_mut(inst_idx) {
            slot.push((xreg, field_idx, is_def));
        }
    }

    /// 追加另一指令包的微指令与映射（组合展开；xregs 并入）。
    ///
    /// 子包若通过自己的 `xregs`（XRegAllocator）分配过临时寄存器（编号从 0 起），
    /// 合并时必须以本包分配器的续接编号对子包 XReg **偏移重映射**，并重写子包
    /// xreg_map / inputs / outputs 中的编号——否则多个包的同编号 XReg 在合并后的
    /// xreg_map 里互相重叠，regalloc 会把不同包的同编号 XReg 当作同一个 XReg
    /// 处理（活区间/寄存器位置错乱）。
    pub fn append(&mut self, other: InstPacket<I>) {
        let other_count = other.xregs.next_index();
        if other_count > 0 {
            let base = self.xregs.next_index();
            let mut other = other;
            other.remap_internal(base);
            // 并入子包分配器的编号空间
            for _ in 0..other_count {
                self.xregs.alloc_default(forge_ir::RegClass::GPR64);
            }
            self.insts.extend(other.insts);
            self.xreg_map.extend(other.xreg_map);
        } else {
            self.insts.extend(other.insts);
            self.xreg_map.extend(other.xreg_map);
        }
    }

    /// 把本包内部分配的 XReg（编号 < self.xregs.next_index()）偏移到 `base` 续接，
    /// 并重写 xreg_map / inputs / outputs 中的编号。供 append 与 pipeline 展开处
    /// （compiler.rs 的 `self.xreg_map.extend(...)`）共用——展开路径若直接 extend
    /// 而不重映射，包内 XReg 会与函数级 XReg 编号重叠。
    pub fn remap_internal(&mut self, base: u32) {
        let count = self.xregs.next_index();
        if count == 0 {
            return;
        }
        let remap = |x: &mut XReg| {
            if x.index() < count {
                *x = XReg::new(x.index() + base, x.class());
            }
        };
        for slot in self.xreg_map.iter_mut() {
            for (x, _fi, _d) in slot.iter_mut() {
                remap(x);
            }
        }
        for x in self.inputs.iter_mut() {
            remap(x);
        }
        for x in self.outputs.iter_mut() {
            remap(x);
        }
    }

    /// 指令包是否为空。
    pub fn is_empty(&self) -> bool {
        self.insts.is_empty()
    }
}

/// 指令选择：IR → 微指令包。
///
/// lowering 阶段运行在寄存器分配之前，所有操作数使用临时寄存器（XReg）。
pub trait TargetLowering: Send + Sync + 'static {
    type Inst: super::inst::MachineInst;

    /// 将一条 IR 指令降低为微指令包。
    fn lower_inst(
        &self,
        op: &Opcode,
        args: &[XReg],
        results: &[XReg],
        ctx: &mut LowerCtx,
    ) -> Result<InstPacket<Self::Inst>, IrError>;

    /// 将 IR 终止指令降低为微指令包。
    fn lower_terminator(
        &self,
        term: &Terminator,
        value_to_xreg: &HashMap<Value, XReg>,
        block_to_vblock: &HashMap<Block, VBlockId>,
        ctx: &mut LowerCtx,
    ) -> Result<InstPacket<Self::Inst>, IrError>;

    /// 基于模式的优化 lowering（pattern_isel 匹配后调用）。
    fn lower_pattern(
        &self,
        _pattern_name: &str,
        _args: &[XReg],
        _results: &[XReg],
        _ctx: &mut LowerCtx,
    ) -> Result<InstPacket<Self::Inst>, IrError> {
        Ok(InstPacket::new())
    }
}
