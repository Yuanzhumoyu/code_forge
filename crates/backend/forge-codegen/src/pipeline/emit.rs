//! CodeSink — byte-level code buffer with label fixup support.
//!
//! Provides a growable byte buffer for machine code emission.
//! Supports deferred label resolution: branches can reference labels
//! that haven't been bound yet; `finish()` patches all pending fixups.

use crate::machine::reloc_patcher::RelocPatcher;
use crate::runtime::output_types::{RelocKind, Relocation};
use forge_ir::Block;
use forge_ir::ImmStr;
use std::sync::Arc;

/// 机器层 label —— IR 块，或**机器层自造的外部标签**。
///
/// 存在理由：机器 IR 的 label 复用 IR 的 `Block` 句柄，但统一尾声**不是 IR 块**
/// （它由 `TargetFrameLowering::emit_epilogue` 发射、`return` 块通过
/// `emit_epilogue_jump` 跳到它）。历史实现用魔数 `Block(0xFFFFFFFD)` 表示，
/// 既可能与真实块索引碰撞，也挡不住别处乱造同值句柄；现在把两种来源写进类型。
///
/// 编码侧仍需要一个数字 id（定宽 ISA 把它塞进 label 位域、变长走 reloc，
/// 由 encoder → `use_label_at` → patcher 消费），用 [`LabelRef::id`] 取 /
/// [`LabelRef::from_id`] 还原。
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub enum LabelRef {
    /// 真实 IR 块（id = `block.0`）。
    Block(Block),
    /// 机器层外部标签（id 落在保留区间，见 [`ExternalLabel`]）。
    External(ExternalLabel),
}

/// 机器层的外部（非 IR 块）标签。
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub enum ExternalLabel {
    /// 统一尾声：多 return 块共用的收尾代码入口。
    Epilogue,
}

impl ExternalLabel {
    /// 保留区间的基址（u32 索引空间最高 3 个值留给外部标签）。
    pub const BASE: u32 = 0xFFFF_FFFD;

    /// 编码 id。
    pub fn id(self) -> u32 {
        match self {
            ExternalLabel::Epilogue => Self::BASE,
        }
    }
}

impl LabelRef {
    /// 统一尾声标签（`Block` 之外唯一的机器层标签）。
    pub const EPILOGUE: LabelRef = LabelRef::External(ExternalLabel::Epilogue);

    /// 编码 id（塞进指令 label 槽 / 供 fixup 查表）。
    pub fn id(self) -> u32 {
        match self {
            LabelRef::Block(b) => b.index(),
            LabelRef::External(e) => e.id(),
        }
    }

    /// 由编码 id 还原：保留区间 → 外部标签；其余 → IR 块。
    ///
    /// 编码器把 label 槽写成数字、`use_label_at` 再从数字还原——**这是唯一
    /// 允许"数字 ↔ 标签"互转的边界**（生成的 machine.rs 调它）。
    pub fn from_id(id: u32) -> LabelRef {
        if id >= ExternalLabel::BASE {
            LabelRef::External(ExternalLabel::Epilogue)
        } else {
            LabelRef::Block(Block::new(id))
        }
    }
}

impl From<Block> for LabelRef {
    fn from(block: Block) -> Self {
        LabelRef::Block(block)
    }
}

/// Byte-level code emission buffer with label fixup support.
pub struct CodeSink {
    data: Vec<u8>,
    /// Label bindings: Block(label_id) → offset in data.
    labels: std::collections::HashMap<LabelRef, usize>,
    /// Pending label references: (patch_offset, label, reloc_kind).
    pending: Vec<(usize, LabelRef, RelocKind)>,
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
    pub fn bind_label(&mut self, label: impl Into<LabelRef>) {
        let label: LabelRef = label.into();
        self.labels.insert(label, self.data.len());
    }

    /// Record a label reference at `patch_offset` for later fixup.
    pub fn use_label_at(
        &mut self,
        patch_offset: usize,
        label: impl Into<LabelRef>,
        kind: RelocKind,
    ) {
        let label: LabelRef = label.into();
        self.pending.push((patch_offset, label, kind));
    }

    /// Record a relocation entry.
    pub fn add_reloc(&mut self, offset: usize, kind: RelocKind, symbol: &str, addend: i64) {
        self.relocs.push(Relocation {
            offset,
            kind,
            symbol: ImmStr::from(symbol),
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
    /// Errors are returned as `String` for use with `IrError::Emit`.
    pub fn finish(mut self) -> Result<Vec<u8>, String> {
        for (patch_offset, label, kind) in &self.pending {
            let target = self.labels.get(label).ok_or_else(|| {
                format!(
                    "unresolved label {:?} (id {}) referenced at offset {}",
                    label,
                    label.id(),
                    patch_offset
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
                    symbol: ImmStr::default(),
                    addend: *target as i64,
                });
                continue;
            }

            // Legacy path (no patcher): plain relative/absolute write-back.
            // 宽度 = `RelocKind` 声明的**字节数**，由 ISA/后端决定（**不设白名单**：
            // 任意 ≥ 1 字节；超出 8 字节的部分按符号/零扩展补位）。
            let width = kind.width_bytes() as usize;
            if width == 0 {
                return Err(format!(
                    "reloc width 0 at offset {patch_offset} ({kind:?}): 宽度由 ISA 声明，必须 ≥ 1 字节"
                ));
            }
            let patch_value = match kind {
                RelocKind::Absolute(_) => {
                    self.relocs.push(Relocation {
                        offset: *patch_offset,
                        kind: *kind,
                        symbol: ImmStr::default(),
                        addend: *target as i64,
                    });
                    *target as u64
                }
                RelocKind::Relative(_, adjustment) => {
                    let rel_target = *target as i64 - *patch_offset as i64 + *adjustment as i64;
                    self.relocs.push(Relocation {
                        offset: *patch_offset,
                        kind: *kind,
                        symbol: ImmStr::default(),
                        addend: rel_target,
                    });
                    rel_target as u64
                }
            };

            // Patch the bytes in-place（width 字节 LE；越界 → 调用方给的
            // patch_offset 或缓冲区长度不对，明确报错不 panic）。
            let end = *patch_offset + width;
            if end > self.data.len() {
                return Err(format!(
                    "reloc at offset {patch_offset} (width {width}) exceeds code buffer \
                     length {} ",
                    self.data.len()
                ));
            }
            let le = kind.encode_value(patch_value);
            self.data[*patch_offset..end].copy_from_slice(&le);
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
