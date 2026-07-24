//! IsaComposer trait — 多 ISA 扩展组合接口。
//!
//! 允许将多个 ISA 子集组合为单个后端。
//! 例如: RV32I + RV32M + RV32F + RV32D = RISC-V 完整后端。
//!
//! # 实现示例
//!
//! ```ignore
//! use codegen_lib::backend::composer::IsaComposer;
//!
//! struct RiscVBackend(RiscVComposer);
//!
//! impl IsaComposer for RiscVComposer {
//!     type Inst = RiscVInst;
//!     type Reg = RiscVReg;
//!
//!     fn info() -> ComposedIsaInfo {
//!         let mut info = ComposedIsaInfo::new("riscv64", "9.0");
//!         info.add_extension::<Rv64i>();
//!         info.add_extension::<Rv64m>();
//!         info.add_extension::<Rv64f>();
//!         info
//!     }
//! }
//! ```

use crate::backend::isa_info::{IsaCapabilities, IsaInfo};
use crate::backend::machine_inst::MachineInst;
use crate::ir::*;
use crate::backend::{LowerCtx, VCodeBlockId, RegMap, CodeSink};
use crate::CompileError;
use std::collections::HashMap;

/// 组合 ISA 信息 — 由多个 ISA 扩展合并而成。
#[derive(Debug, Clone)]
pub struct ComposedIsaInfo {
    /// ISA 名称。
    pub name: &'static str,
    /// 版本。
    pub version: &'static str,
    /// 合并后的能力（取并集/最大值）。
    pub capabilities: IsaCapabilities,
    /// 所有扩展的名称列表。
    pub extensions: Vec<&'static str>,
}

impl ComposedIsaInfo {
    /// 创建新的组合 ISA 信息。
    pub fn new(name: &'static str, version: &'static str) -> Self {
        Self {
            name,
            version,
            capabilities: IsaCapabilities {
                variable_length: false,
                fixed_inst_size: 4,
                prefix_layers: 0,
                addressing_modes: &[],
                simd_widths: &[],
                mask_registers: false,
                broadcast: false,
                rounding_mode: false,
                endianness: Endianness::Little,
                min_inst_len: 4,
                max_inst_len: 4,
            },
            extensions: Vec::new(),
        }
    }

    /// 注册一个 ISA 扩展。
    pub fn add_extension<I: IsaInfo>(&mut self) {
        let cap = I::capabilities();
        // 合并能力（取并集）
        if cap.variable_length {
            self.capabilities.variable_length = true;
        }
        if cap.prefix_layers > self.capabilities.prefix_layers {
            self.capabilities.prefix_layers = cap.prefix_layers;
        }
        if cap.min_inst_len < self.capabilities.min_inst_len {
            self.capabilities.min_inst_len = cap.min_inst_len;
        }
        if cap.max_inst_len > self.capabilities.max_inst_len {
            self.capabilities.max_inst_len = cap.max_inst_len;
        }
        if cap.mask_registers {
            self.capabilities.mask_registers = true;
        }
        if cap.broadcast {
            self.capabilities.broadcast = true;
        }
        if cap.rounding_mode {
            self.capabilities.rounding_mode = true;
        }
        self.extensions.push(I::name());
    }
}

/// ISA 组合器 trait。
///
/// 当一个 ISA 后端由多个扩展组成时，实现此 trait。
/// 组合器将 `lower`/`lower_terminator` 路由到对应的扩展。
pub trait IsaComposer: Send + Sync + 'static {
    /// 组合后统一的机器指令类型。
    type Inst: MachineInst;
    /// 组合后统一的物理寄存器类型。
    type Reg: PhysReg;

    /// 获取组合后的 ISA 信息。
    fn info() -> ComposedIsaInfo;

    /// 指令选择：根据 IR 操作码路由到对应的扩展。
    fn lower(
        op: &Opcode,
        args: &[VReg],
        result: Option<VReg>,
        ctx: &mut LowerCtx,
    ) -> Result<Vec<Self::Inst>, CompileError>;

    /// 终止指令降低。
    fn lower_terminator(
        term: &Terminator,
        value_to_vreg: &HashMap<Value, VReg>,
        block_to_vblock: &HashMap<BlockId, VCodeBlockId>,
        ctx: &mut LowerCtx,
    ) -> Result<Vec<Self::Inst>, CompileError>;

    /// 代码发射。默认委托给统一的 `InstructionSet::emit`。
    fn emit(
        inst: &Self::Inst,
        reg_map: &RegMap,
        sink: &mut CodeSink,
    ) -> Result<(), CompileError>;
}
