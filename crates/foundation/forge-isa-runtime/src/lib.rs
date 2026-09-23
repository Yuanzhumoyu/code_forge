//! forge-isa-runtime — ISA-DSL 生成物的**唯一运行时依赖面**（v19 V1）。
//!
//! `isa_from_file!` 生成出来的模块只应依赖本 crate（+ 宿主自己的**编译管线**）：
//! 机器指令/编码器/解码器/汇编器/帧/ABI/lowering 的 trait 体系、`Inst`/`Reg` 需要的
//! 数据型（`AllocResult`/`CodeSink`/`LabelRef`/`CompiledFunction`/`RelocKind`）、
//! `LowerCtx`/`MemRef`、CPU 能力探测、`Registry` 与 `prelude` 都在这里。
//!
//! 与 `forge-codegen` 的分工：本 crate **不认识**编译管线（JIT/寄存器分配/发射/VCode
//! 装配之外的一切），也不依赖它；`forge-codegen` 依赖本 crate，并保留同名 re-export
//! 供内部路径使用（`crate::machine::…` 等仍然有效）。
//!
//! 边界守卫见 `crates/foundation/forge-isa-runtime/tests/runtime_surface.rs`
//! （源码里不得出现 `pipeline`/`forge_codegen` 依赖）。

pub use forge_ir as ir;
pub use forge_ir::IrError;

pub mod alloc_result;
pub mod cpu;
pub mod ctx;
pub mod emit;
mod erased;
pub mod machine;
pub mod output_types;
pub mod pipeline;
pub mod prelude;
pub mod registry;
pub mod vcode;

pub use alloc_result::{AllocResult, FrameInfo, SpillSlot};
pub use cpu::{
    AVX512_ENV_LOCK, avx_available, avx2_available, avx512_available, avx512_hardware_available,
};
pub use ctx::{LowerCtx, MemRef};
pub use emit::{CodeSink, ExternalLabel, LabelRef};
pub use machine::encoder::EncodeError;
pub use machine::inst::{EffectKind, MachineInst};
pub use machine::isa_info::{IsaCapabilities, IsaInfo, RegisterClassInfo};
pub use machine::lowering::InstPacket;
pub use output_types::{CompiledFunction, RelocKind, Relocation};
pub use pipeline::{FunctionPipeline, PipelineFactory, compile_via_pipeline, register_pipeline};
pub use registry::Registry;
pub use vcode::{VBlockId, VCode, VCodeBlock};
