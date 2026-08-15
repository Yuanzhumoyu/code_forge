//! Compilation output types — result of the code generation pipeline.

use forge_ir::ImmStr;

/// A compiled function with its machine code and relocation information.
#[derive(Debug, Clone)]
pub struct CompiledFunction {
    pub code: Vec<u8>,
    pub relocations: Vec<Relocation>,
    pub code_size: usize,
}

/// A relocation record within compiled code.
#[derive(Debug, Clone)]
pub struct Relocation {
    pub offset: usize,
    pub kind: RelocKind,
    pub symbol: ImmStr,
    pub addend: i64,
}

/// Relocation type — data-driven design, no per-architecture variants needed.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RelocKind {
    /// Absolute address relocation. `width_bytes` ∈ {1, 2, 4, 8}.
    Absolute(u8),
    /// PC-relative relocation. `width_bytes` ∈ {1, 2, 4, 8}.
    /// `adjustment` is the offset correction (x86: -width_bytes, ARM: 0).
    /// ISA-specific encoding (e.g. AArch64 BL imm26, RISC-V JAL UJ layout)
    /// is applied by the backend's `RelocPatcher`, not by this enum.
    Relative(u8, i8),
}

impl RelocKind {
    pub const ABS4: Self = RelocKind::Absolute(4);
    pub const ABS8: Self = RelocKind::Absolute(8);
    pub const REL4: Self = RelocKind::Relative(4, -4);
    pub const REL8: Self = RelocKind::Relative(8, -8);
    pub const REL1: Self = RelocKind::Relative(1, -1);
    pub const CALL: Self = RelocKind::Relative(4, -4);
    pub const CALL_IND: Self = RelocKind::Absolute(8);
}
