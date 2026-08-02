//! Machine backend trait system — v19 redesign.
//!
//! Each component is a focused trait with a single responsibility.
//! [`TargetMachine`] composes them into a complete backend.
//!
//! # Trait Hierarchy
//!
//! ```text
//! TargetMachine (composes all below)
//!   ├── IsaInfo          (static ISA metadata)
//!   ├── TargetRegInfo    (register file description)
//!   ├── TargetABI        (calling convention)
//!   ├── TargetLowering   (IR → machine instructions)
//!   ├── TargetEncoder    (machine inst → bytes)
//!   ├── TargetFrameLowering (prologue / epilogue)
//!   ├── TargetPeephole   (machine inst optimization)
//!   │
//!   └── Optional: TargetDecoder, TargetDisassembler,
//!                 TargetAssembler, TargetSimulator
//! ```

pub mod abi;
pub mod assembler;
pub mod decoder;
pub mod disasm;
pub mod encoder;
pub mod frame;
pub mod inst;
pub mod isa_info;
pub mod lowering;
pub mod peephole;
pub mod reg_alloc;
pub mod reg_info;
pub mod reloc_patcher;
pub mod simulator;
pub mod target;
