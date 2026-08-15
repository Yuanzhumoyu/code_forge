//! RelocPatcher — ISA-specific relocation encoder.
//!
//! The relocation framework only deals with generic concepts
//! ([`RelocKind::Absolute`] / [`RelocKind::Relative`]); how a relocation is
//! encoded into machine code bytes is backend-specific (e.g. AArch64 `BL`
//! packs a 26-bit immediate with `>>2`, RISC-V `JAL` reorders the UJ-type
//! immediate bits, x86 writes a plain 32-bit relative displacement). Each
//! backend implements this trait and hands it to the pipeline via
//! [`TargetMachine::reloc_patcher`](crate::machine::target::TargetMachine).
//!
//! `apply` is used for both in-function label fixups (resolved in
//! [`CodeSink::finish`]) and cross-function symbol relocations (resolved by
//! the JIT/object layers), keeping the encoding logic in one place.

use crate::{IrError, RelocKind};
use std::collections::HashMap;
use std::sync::{Arc, Mutex, OnceLock};

/// Applies a relocation's resolved target to code bytes at `offset`.
///
/// * `code` — full emitted code buffer.
/// * `offset` — byte offset of the relocation site (instruction word / field).
/// * `site` — absolute address of the relocation site (for PC-relative math).
/// * `target` — resolved absolute address of the relocation target.
pub trait RelocPatcher: Send + Sync {
    fn apply(
        &self,
        code: &mut [u8],
        offset: usize,
        kind: RelocKind,
        target: u64,
        site: u64,
    ) -> Result<(), IrError>;
}

/// Per-ISA registration table. Backends register their patcher in
/// `ensure_registered()`; the pipeline and JIT look it up by ISA name.
static RELOC_PATCHERS: OnceLock<Mutex<HashMap<String, Arc<dyn RelocPatcher>>>> = OnceLock::new();

/// Register a backend's relocation patcher under its ISA name.
pub fn register_reloc_patcher(isa: &str, patcher: Arc<dyn RelocPatcher>) {
    let map = RELOC_PATCHERS.get_or_init(|| Mutex::new(HashMap::new()));
    map.lock().unwrap().insert(isa.to_string(), patcher);
}

/// Look up a backend's patcher by ISA name.
pub fn reloc_patcher_for(isa: &str) -> Option<Arc<dyn RelocPatcher>> {
    RELOC_PATCHERS
        .get()
        .and_then(|m| m.lock().ok())
        .and_then(|g| g.get(isa).cloned())
}

/// Register the default patcher for a backend ISA (called by the DSL-generated
/// `ensure_registered`). Unknown ISAs (e.g. wasm32 stack machine) get None and
/// fall back to the generic relative/absolute write path.
pub fn register_default_reloc_patcher(isa: &str) {
    if RELOC_PATCHERS
        .get()
        .and_then(|m| m.lock().ok())
        .map(|g| g.contains_key(isa))
        .unwrap_or(false)
    {
        return;
    }
    let patcher: Arc<dyn RelocPatcher> = match isa {
        "x86_64" => Arc::new(X86RelocPatcher),
        "aarch64" => Arc::new(AArch64RelocPatcher),
        "riscv64" => Arc::new(RiscvRelocPatcher),
        _ => return,
    };
    register_reloc_patcher(isa, patcher);
}

// ─────────────────────────────────────────────────────────────
// Backend implementations
// ─────────────────────────────────────────────────────────────

/// x86-64: plain little-endian writes (rel32 displacement, or absolute addr).
pub struct X86RelocPatcher;

impl RelocPatcher for X86RelocPatcher {
    fn apply(
        &self,
        code: &mut [u8],
        offset: usize,
        kind: RelocKind,
        target: u64,
        site: u64,
    ) -> Result<(), IrError> {
        match kind {
            RelocKind::Relative(w, adj) => {
                let v = (target as i64 - site as i64 + adj as i64) as u64;
                match w {
                    1 => {
                        code[offset] = v as u8;
                    }
                    4 => {
                        code[offset..offset + 4].copy_from_slice(&(v as u32).to_le_bytes());
                    }
                    8 => {
                        code[offset..offset + 8].copy_from_slice(&v.to_le_bytes());
                    }
                    _ => {}
                }
            }
            RelocKind::Absolute(w) => {
                let n = w as usize;
                if offset + n <= code.len() {
                    code[offset..offset + n].copy_from_slice(&target.to_le_bytes()[..n]);
                }
            }
        }
        Ok(())
    }
}

/// AArch64: read-modify-write of branch instruction words. Recognizes
/// B/BL (imm26 at [0;26]) and CBZ/CBNZ (imm19 at [5;19]); other words are
/// left untouched.
pub struct AArch64RelocPatcher;

impl RelocPatcher for AArch64RelocPatcher {
    fn apply(
        &self,
        code: &mut [u8],
        offset: usize,
        kind: RelocKind,
        target: u64,
        site: u64,
    ) -> Result<(), IrError> {
        if let RelocKind::Relative(4, _adj) = kind {
            if offset + 4 > code.len() {
                return Err(IrError::Emit("aarch64 reloc out of bounds".into()));
            }
            let mut word = u32::from_le_bytes(code[offset..offset + 4].try_into().unwrap());
            let delta = target as i64 - site as i64 - 4;
            let imm26 = (delta >> 2) as u32 & 0x3FF_FFFF;
            let top6 = word >> 26;
            match top6 {
                0x25 | 0x05 => {
                    // BL / B — imm26 at [0;26]
                    word = (word & !0x3FF_FFFF) | imm26;
                }
                0x1A | 0x19 => {
                    // CBNZ / CBZ — imm19 at [5;23] (sf bit at 31 folded into top6)
                    let imm19 = (delta >> 2) as u32 & 0x7FFFF;
                    word = (word & !(0x7FFFF << 5)) | (imm19 << 5);
                }
                _ => {
                    // Unknown branch encoding — leave as-is (caller emitted
                    // zero placeholder; direct CALL uses @call_reloc path).
                }
            }
            code[offset..offset + 4].copy_from_slice(&word.to_le_bytes());
        }
        Ok(())
    }
}

/// RISC-V: read-modify-write of JAL (UJ-type) and B-type branch immediates.
pub struct RiscvRelocPatcher;
impl RelocPatcher for RiscvRelocPatcher {
    fn apply(
        &self,
        code: &mut [u8],
        offset: usize,
        kind: RelocKind,
        target: u64,
        site: u64,
    ) -> Result<(), IrError> {
        if let RelocKind::Relative(4, _adj) = kind {
            if offset + 4 > code.len() {
                return Err(IrError::Emit("riscv reloc out of bounds".into()));
            }
            let mut word = u32::from_le_bytes(code[offset..offset + 4].try_into().unwrap());
            let opcode = word & 0x7F;
            let delta = target as i64 - site as i64;
            match opcode {
                0x6F => {
                    // JAL — UJ-type immediate reordering
                    let imm = (delta >> 1) as u32 & 0x1F_FFFF;
                    let reordered = ((imm >> 20) & 0x1) << 31
                        | ((imm >> 1) & 0x3FF) << 21
                        | ((imm >> 11) & 0x1) << 20
                        | ((imm >> 12) & 0xFF) << 12;
                    word = (word & !0xFFF0_0000) | reordered;
                }
                0x63 => {
                    // B-type branch immediate
                    let imm = (delta >> 1) as u32 & 0xFFF;
                    let reordered = ((imm >> 12) & 0x1) << 31
                        | ((imm >> 5) & 0x3F) << 25
                        | ((imm >> 1) & 0xF) << 8
                        | ((imm >> 11) & 0x1) << 7;
                    word = (word & !0xFE00_0F80) | reordered;
                }
                _ => {}
            }
            code[offset..offset + 4].copy_from_slice(&word.to_le_bytes());
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn x86_rel32_patch() {
        let p = X86RelocPatcher;
        let mut code = vec![0xE8u8, 0, 0, 0, 0];
        p.apply(&mut code, 1, RelocKind::REL4, 0x1000, 0x1001)
            .unwrap();
        // target - site + adj = 0x1000 - 0x1001 - 4 = -5
        assert_eq!(&code[1..5], &(-5i32).to_le_bytes());
    }

    #[test]
    fn x86_abs8_patch() {
        let p = X86RelocPatcher;
        let mut code = vec![0u8; 8];
        p.apply(&mut code, 0, RelocKind::ABS8, 0x1234_5678_9ABC_DEF0, 0)
            .unwrap();
        assert_eq!(&code, &0x1234_5678_9ABC_DEF0u64.to_le_bytes());
    }

    #[test]
    fn aarch64_bl_imm26_patch() {
        let p = AArch64RelocPatcher;
        // BL #0 template
        let mut code = 0x9400_0000u32.to_le_bytes().to_vec();
        p.apply(&mut code, 0, RelocKind::REL4, 0x1000, 0).unwrap();
        let word = u32::from_le_bytes(code[0..4].try_into().unwrap());
        assert_eq!(word >> 26, 0x25, "opcode bits (BL) preserved");
        let imm26 = word & 0x3FF_FFFF;
        assert_eq!(imm26, ((0x1000 - 4) >> 2) as u32);
    }

    #[test]
    fn riscv_jal_uj_patch() {
        let p = RiscvRelocPatcher;
        // JAL rd=x0, #0 template
        let mut code = 0x6Fu32.to_le_bytes().to_vec();
        p.apply(&mut code, 0, RelocKind::REL4, 0x2000, 0).unwrap();
        let word = u32::from_le_bytes(code[0..4].try_into().unwrap());
        assert_eq!(word & 0x7F, 0x6F, "JAL opcode preserved");
        // Unpack UJ immediate and compare with (target - site) >> 1
        let imm = ((word >> 31) & 0x1) << 20
            | ((word >> 21) & 0x3FF) << 1
            | ((word >> 20) & 0x1) << 11
            | ((word >> 12) & 0xFF) << 12;
        assert_eq!(imm, (0x2000 >> 1) as u32);
    }
}
