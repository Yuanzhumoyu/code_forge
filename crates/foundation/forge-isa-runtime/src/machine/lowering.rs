//! TargetLowering — 指令选择接口。
//!
//! 将 IR 操作码和终止指令降低为微指令包（InstPacket）。
//! 输入包含 XReg（临时寄存器）操作数，输出为指令包：微指令数组 +
//! 临时寄存器分配器 + XReg→微指令寄存器字段映射 + 输入/输出。

use crate::{LowerCtx, VBlockId};
use forge_ir::entity::map::SecondaryMap;
use forge_ir::{Block, IrError, Opcode, Value, XReg, XRegAllocator};

/// **一次 vreg → 字段的绑定**（管线的操作数表项，v23 M4）。
///
/// 为什么单独一个类型（而不是 `(XReg, u8, bool)` 元组）：元组里那个 `bool` 是
/// "这条登记是 def 吗"，与指令**形状表**里的 `OperandRole` 是同一件事的两种写法；
/// 本仓实测（2026-10-11）证明**不能**把它删掉一律按形状表派生——同一个 InOut 字段
/// 在旁表里可能 0 条（物理寄存器占位，不参与 vreg 映射）或 1 条 `Def`（vreg 绑定），
/// 而形状表里该字段同时是 `Use` 与 `Def`。因此角色**由 lowering 给出**，这里只是
/// 把它类型化（`bool` → `OperandRole`），语义不变。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct OperandBind {
    /// 虚拟寄存器。
    pub vreg: XReg,
    /// 指令的寄存器字段序号（`reg_field`/`set_reg_field` 的 `i`）。
    pub field: u8,
    /// 这条绑定是读还是写（见上文：**不可**从形状表派生）。
    pub role: crate::machine::inst::OperandRole,
    /// 该 vreg 的**值宽类**（回填多类槽 `gprx` 用；`None` = 回填时退到 vreg 自身类）。
    ///
    /// v23 M4 批次 C 第二步：这类信息以前存在管线级的 `LowerCtx::xreg_types`
    /// （vreg → IR 类型）里，**两个回填点各自把它推成类、而口径还不一样**
    /// （`compiler.rs` 走 `ctx.reg_class_for(ty)`、`emission.rs` 走
    /// `RegClass::from_type_id`）。现在由管线在 lowering 之后**折一次**进这里，
    /// 两个回填点读同一个值。
    pub class: Option<forge_ir::RegClass>,
}

impl OperandBind {
    pub fn new(vreg: XReg, field: u8, role: crate::machine::inst::OperandRole) -> Self {
        Self {
            vreg,
            field,
            role,
            class: None,
        }
    }

    /// 写绑定（`is_def = true` 的等价写法）。
    pub fn def(vreg: XReg, field: u8) -> Self {
        Self::new(vreg, field, crate::machine::inst::OperandRole::Def)
    }

    /// 读绑定。
    pub fn use_(vreg: XReg, field: u8) -> Self {
        Self::new(vreg, field, crate::machine::inst::OperandRole::Use)
    }

    /// 带上值宽类（管线折 `xreg_types` 时用）。
    pub fn with_class(mut self, class: forge_ir::RegClass) -> Self {
        self.class = Some(class);
        self
    }
}

/// 指令包 — DSL 中间指令（IR op）映射到微指令的返回结构，取代旧的 `Vec<Inst>`。
///
/// 各字段语义：
/// - `xregs`：临时寄存器分配器。指令包内新分配的 XReg 由它管理；
///   组合/展开多个指令包时，子包分配器可并入父包。
/// - `insts`：微指令数组。寄存器字段（Ireg/Freg）在构造时以**默认寄存器**占位
///   （对应类型的 0 号物理寄存器），由寄存器分配器分配后回填真正寄存器。
/// - `inst_ops`：vreg → 微指令寄存器字段的**绑定表**（v23 M4 由 `xreg_map` 改名并
///   类型化：`(XReg, u8, bool)` → `OperandBind { vreg, field, role }`），与 `insts`
///   **平行**：每项是该微指令的 Ireg/Freg 字段对应的绑定列表（按字段声明顺序）。
///   分配器按此回填字段：`insts[i]` 的第 `j` 个字段 = `inst_ops[i][j]` 的分配结果。
/// - `inputs` / `outputs`：指令包的输入/输出，类型为临时寄存器（XReg）。
///   供指令包组合/展开时连接（A 的 output 连 B 的 input 为同一 XReg）。
#[derive(Clone, Debug, Default)]
pub struct InstPacket<I> {
    /// 临时寄存器分配器。
    pub xregs: XRegAllocator,
    /// 微指令数组（寄存器字段以默认物理寄存器占位）。
    pub insts: Vec<I>,
    /// vreg → 寄存器字段的绑定表（与 `insts` 平行，见类型级文档）。
    pub inst_ops: Vec<smallvec::SmallVec<[OperandBind; 2]>>,
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
            inst_ops: Vec::new(),
            inputs: Vec::new(),
            outputs: Vec::new(),
        }
    }

    /// 添加一条微指令并返回其索引（`inst_ops` 同步推入空绑定槽）。
    pub fn push_inst(&mut self, inst: I) -> usize {
        self.insts.push(inst);
        self.inst_ops.push(smallvec::SmallVec::new());
        self.insts.len() - 1
    }

    /// 记录 vreg → 微指令寄存器字段（追加到 `inst_ops[inst_idx]`，字段顺序即记录顺序）。
    /// `is_def = true` 表示该 XReg 是这条指令的输出（def），false 为输入（use）。
    /// 区分 use/def 是 regalloc 活区间正确性的前提（use 在 inst_start、def 在 inst_end）。
    ///
    /// **v23 M4 收敛守卫**：登记的 `(field, is_def)` 必须与指令的**操作数表**一致——
    /// 一旦全部发射点都成立，`is_def` 就可由 `operands()` 派生（指令就在手边），
    /// 旁表随之只剩 `(vreg, field)`、"两处必须逐条对齐"这件事也一并消失。
    /// 在**发射点**断言（而不是编译尾部逐条扫）能把错误定位到具体哪条 lowering 写错。
    pub fn map_reg_field(&mut self, xreg: XReg, inst_idx: usize, field_idx: u8, is_def: bool)
    where
        I: crate::machine::inst::MachineInst,
    {
        #[cfg(debug_assertions)]
        if let Some(inst) = self.insts.get(inst_idx) {
            use crate::machine::inst::OperandRole;
            let want = if is_def {
                OperandRole::Def
            } else {
                OperandRole::Use
            };
            let ops = inst.operands();
            let hit = ops.iter().any(|o| o.field == field_idx && o.role == want);
            assert!(
                hit,
                "v23 M4：`map_reg_field` 与操作数表分叉——第 {inst_idx} 条指令的字段 \
                 {field_idx} 登记为 {want:?}，但操作数表里没有这一项：{ops:?}"
            );
        }
        if let Some(slot) = self.inst_ops.get_mut(inst_idx) {
            slot.push(OperandBind::new(
                xreg,
                field_idx,
                if is_def {
                    crate::machine::inst::OperandRole::Def
                } else {
                    crate::machine::inst::OperandRole::Use
                },
            ));
        }
    }

    /// 追加另一指令包的微指令与映射（组合展开；xregs 并入）。
    ///
    /// 子包若通过自己的 `xregs`（XRegAllocator）分配过临时寄存器（编号从 0 起），
    /// 合并时必须以本包分配器的续接编号对子包 XReg **偏移重映射**，并重写子包
    /// `inst_ops` / inputs / outputs 中的编号——否则多个包的同编号 XReg 在合并后的
    /// `inst_ops` 里互相重叠，regalloc 会把不同包的同编号 XReg 当作同一个 XReg
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
            self.inst_ops.extend(other.inst_ops);
        } else {
            self.insts.extend(other.insts);
            self.inst_ops.extend(other.inst_ops);
        }
    }

    /// 把本包内部分配的 XReg（编号 < self.xregs.next_index()）偏移到 `base` 续接，
    /// 并重写 `inst_ops` / inputs / outputs 中的编号。供 append 与 pipeline 展开处
    /// （compiler.rs 的 `self.inst_ops.extend(...)`）共用——展开路径若直接 extend
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
        for slot in self.inst_ops.iter_mut() {
            for b in slot.iter_mut() {
                remap(&mut b.vreg);
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
    ///
    /// 终结符是一条指令（S4 主体），因此这里给的是**块** + DFG：实现方用
    /// `DataFlowGraph::term_kind` 判形态，再用 `term_*` 投影取载荷
    /// （ISA 侧不需要知道终结符的存储形态）。
    fn lower_terminator(
        &self,
        dfg: &forge_ir::DataFlowGraph,
        block: Block,
        value_to_xreg: &SecondaryMap<Value, XReg>,
        block_to_vblock: &SecondaryMap<Block, VBlockId>,
        ctx: &mut LowerCtx,
    ) -> Result<InstPacket<Self::Inst>, IrError>;

    /// 树型多指令模式（树型匹配，S6）的发射入口：`pattern_name` 是
    /// [`PatternSpec`](crate::machine::pattern::PatternSpec) 的 `name`，
    /// `args` 是按树 DFS 序绑定的叶变量 XReg，`results` 是根指令的结果 XReg。
    ///
    /// lowering 驱动对无 `树型匹配` 的 ISA 永不调用（`patterns()` 空表 →
    /// 预扫空转），缺省实现返回空包（防御性兜底，正常不可达）。
    fn lower_pattern(
        &self,
        _pattern_name: &str,
        _args: &[XReg],
        _results: &[XReg],
        _ctx: &mut LowerCtx,
    ) -> Result<InstPacket<Self::Inst>, IrError> {
        Err(IrError::Unsupported(format!(
            "DSL lowering: 未声明的树型匹配 '{_pattern_name}'（无此模式的发射实现）"
        )))
    }

    /// 已编译的模式规格表（S6）。已按（Op 节点数降, when 叶子数降, 声明序升）
    /// 排序——驱动 per-root 首个结构+when 全中者生效。无 `树型匹配` → 空表。
    fn patterns(&self) -> &'static [crate::machine::pattern::PatternSpec] {
        &[]
    }
}
