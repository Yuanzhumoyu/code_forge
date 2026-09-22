//! 生成代码的 prelude：`isa_from_file!` 生成物里 `prelude::…` 的来源。

pub use crate::{
    AllocResult, CodeSink, EffectKind, EncodeError, InstPacket, IsaCapabilities, IsaInfo, LowerCtx,
    MachineInst, MemRef, RegisterClassInfo, VBlockId, VCodeBlock, avx_available, avx2_available,
};
pub use forge_ir::entity::map::SecondaryMap;
pub use forge_ir::{
    AtomicRmwOp, Block, ConstId, DataFlowGraph, Endianness, FloatCC, FrameAccess, Immediate,
    Instruction, IntCC, IrError, IselStrategy, Opcode, PReg, PhysReg, RegClass, TermKind, TypeId,
    VReg, Value, XReg, XRegAllocator, intcc_name,
};
