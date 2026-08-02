//! CodeSink — byte-level code buffer with label fixup support.
//!
//! Provides a growable byte buffer for machine code emission.
//! Supports deferred label resolution: branches can reference labels
//! that haven't been bound yet; `finish()` patches all pending fixups.

use crate::machine::reloc_patcher::RelocPatcher;
use crate::runtime::output_types::{RelocKind, Relocation};
use forge_ir::Block;
use std::sync::Arc;

/// Byte-level code emission buffer with label fixup support.
pub struct CodeSink {
    data: Vec<u8>,
    /// Label bindings: Block(label_id) → offset in data.
    labels: std::collections::HashMap<Block, usize>,
    /// Pending label references: (patch_offset, label, reloc_kind).
    pending: Vec<(usize, Block, RelocKind)>,
    /// Relocations to be recorded (emitted to output).
    relocs: Vec<Relocation>,
    /// Backend-specific relocation encoder (used by `finish` to patch
    /// in-function label fixups; None keeps the legacy plain-relative path).
    patcher: Option<Arc<dyn RelocPatcher>>,
}

impl CodeSink {
    pub fn new() -> Self {
        Self {
            data: Vec::new(),
            labels: std::collections::HashMap::new(),
            pending: Vec::new(),
            relocs: Vec::new(),
            patcher: None,
        }
    }

    /// Attach a backend relocation patcher for ISA-specific fixup encoding.
    pub fn set_patcher(&mut self, patcher: Arc<dyn RelocPatcher>) {
        self.patcher = Some(patcher);
    }

    // ── raw byte emission ──

    pub fn put1(&mut self, b: u8) {
        self.data.push(b);
    }

    pub fn put2(&mut self, v: u16) {
        self.data.extend_from_slice(&v.to_le_bytes());
    }

    pub fn put4(&mut self, v: u32) {
        self.data.extend_from_slice(&v.to_le_bytes());
    }

    pub fn put8(&mut self, v: u64) {
        self.data.extend_from_slice(&v.to_le_bytes());
    }

    pub fn put_bytes(&mut self, bytes: &[u8]) {
        self.data.extend_from_slice(bytes);
    }

    /// Current offset in the code buffer (next write position).
    pub fn offset(&self) -> usize {
        self.data.len()
    }

    /// View emitted bytes so far (before fixup resolution).
    pub fn bytes(&self) -> &[u8] {
        &self.data
    }

    // ── label management ──

    /// Bind a label at the current offset.
    pub fn bind_label(&mut self, label: Block) {
        self.labels.insert(label, self.data.len());
    }

    /// Record a label reference at `patch_offset` for later fixup.
    pub fn use_label_at(&mut self, patch_offset: usize, label: Block, kind: RelocKind) {
        self.pending.push((patch_offset, label, kind));
    }

    /// Record a relocation entry.
    pub fn add_reloc(&mut self, offset: usize, kind: RelocKind, symbol: &str, addend: i64) {
        self.relocs.push(Relocation {
            offset,
            kind,
            symbol: symbol.to_string(),
            addend,
        });
    }

    /// Get recorded relocations.
    pub fn relocations(&self) -> Vec<Relocation> {
        self.relocs.clone()
    }

    // ── finalization ──

    /// Resolve all pending label fixups and produce the final bytecode.
    ///
    /// Returns an error if a referenced label was never bound.
    /// Errors are returned as `String` for use with `CompileError::Emit`.
    pub fn finish(mut self) -> Result<Vec<u8>, String> {
        for (patch_offset, label, kind) in &self.pending {
            let target = self.labels.get(label).ok_or_else(|| {
                format!(
                    "unresolved label {:?} referenced at offset {}",
                    label.0, patch_offset
                )
            })?;

            if let Some(patcher) = &self.patcher {
                // Backend-specific encoding (x86 rel32, AArch64 BL imm26,
                // RISC-V B/J immediate reordering, ...).
                patcher
                    .apply(
                        &mut self.data,
                        *patch_offset,
                        *kind,
                        *target as u64,
                        *patch_offset as u64,
                    )
                    .map_err(|e| e.to_string())?;
                // Also record the relocation for downstream consumers
                // (object files, JIT).
                self.relocs.push(Relocation {
                    offset: *patch_offset,
                    kind: *kind,
                    symbol: String::new(),
                    addend: *target as i64,
                });
                continue;
            }

            // Legacy path (no patcher): plain relative/absolute write-back.
            let patch_value = match kind {
                RelocKind::Absolute(bits) => {
                    self.relocs.push(Relocation {
                        offset: *patch_offset,
                        kind: *kind,
                        symbol: String::new(),
                        addend: *target as i64,
                    });
                    match bits {
                        4 => *target as u32 as u64,
                        8 => *target as u64,
                        _ => *target as u64,
                    }
                }
                RelocKind::Relative(bits, adjustment) => {
                    let rel_target = *target as i64 - *patch_offset as i64 + *adjustment as i64;
                    self.relocs.push(Relocation {
                        offset: *patch_offset,
                        kind: *kind,
                        symbol: String::new(),
                        addend: rel_target,
                    });
                    match bits {
                        1 => (rel_target as u8) as u64,
                        4 => (rel_target as i32) as u32 as u64,
                        8 => rel_target as u64,
                        _ => rel_target as u64,
                    }
                }
            };

            // Patch the bytes in-place
            match kind {
                RelocKind::Absolute(4) | RelocKind::Relative(4, _) => {
                    self.data[*patch_offset..*patch_offset + 4]
                        .copy_from_slice(&(patch_value as u32).to_le_bytes());
                }
                RelocKind::Absolute(8) | RelocKind::Relative(8, _) => {
                    self.data[*patch_offset..*patch_offset + 8]
                        .copy_from_slice(&patch_value.to_le_bytes());
                }
                RelocKind::Relative(1, _) => {
                    self.data[*patch_offset] = patch_value as u8;
                }
                _ => {}
            }
        }

        self.relocs.sort_by_key(|r| r.offset);
        Ok(self.data)
    }
}

impl Default for CodeSink {
    fn default() -> Self {
        Self::new()
    }
}
