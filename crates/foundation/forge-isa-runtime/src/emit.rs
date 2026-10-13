//! CodeSink — byte-level code buffer with label fixup support.
//!
//! Provides a growable byte buffer for machine code emission.
//! Supports deferred label resolution: branches can reference labels
//! that haven't been bound yet; `finish()` patches all pending fixups.

use crate::machine::reloc_patcher::RelocPatcher;
use crate::{RelocKind, Relocation};
use forge_ir::Block;
use forge_ir::ImmStr;
use std::sync::Arc;

/// 机器层 label —— IR 块，或**机器层自造（分配出来）的标签**。
///
/// 存在理由：机器 IR 的 label 复用 IR 的 `Block` 句柄，但统一尾声**不是 IR 块**
/// （它由 `TargetFrameLowering::emit_epilogue` 发射、`return` 块通过
/// `emit_epilogue_jump` 跳到它）。历史实现用过魔数 `Block(0xFFFFFFFD)`、
/// 后来又用"唯一的哨兵常量"——两者都要么可能与真实块索引碰撞、要么把"只能有一个
/// 机器标签"写死。v23 M5：机器标签改为**在 [`CodeSink::new_label`] 里分配**
/// （id 落在高位区间，与 IR 块 id 天然分离），于是"要几个有几个"，也没有哨兵常量。
///
/// 编码侧仍需要一个数字 id（定宽 ISA 把它塞进 label 位域、变长走 reloc，
/// 由 encoder → `use_label_at` → patcher 消费），用 [`LabelRef::id`] 取 /
/// [`LabelRef::from_id`] 还原。
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub enum LabelRef {
    /// 真实 IR 块（id = `block.0`）。
    Block(Block),
    /// 机器层分配的标签（id = `MACHINE_LABEL_BASE + n`，见 [`LabelRef::MACHINE_LABEL_BASE`]）。
    Machine(u32),
}

impl LabelRef {
    /// 机器标签 id 区间的基址。IR 块 id 由 `Block::new` 从 0 起给，实际函数的块数
    /// 远小于此（发射侧有 `debug_assert` 兜住"块数 < 基址"）⇒ 两个区间不可能撞上。
    ///
    /// 定宽 ISA 会把 id 写进 label 位域（位宽可能远小于 32），但那个数字**只是查找键**，
    /// 真正的偏移由 fixup 打补丁——所以基址取高位不影响可编码性（既有实现用
    /// `0xFFFF_FFFD` 也是同理）。
    pub const MACHINE_LABEL_BASE: u32 = 0x8000_0000;

    /// 编码 id（塞进指令 label 槽 / 供 fixup 查表）。
    pub fn id(self) -> u32 {
        match self {
            LabelRef::Block(b) => b.index(),
            LabelRef::Machine(n) => Self::MACHINE_LABEL_BASE + n,
        }
    }

    /// 由编码 id 还原：高位区间 → 机器标签；其余 → IR 块。
    ///
    /// 编码器把 label 槽写成数字、`use_label_at` 再从数字还原——**这是唯一
    /// 允许"数字 ↔ 标签"互转的边界**（生成的 machine.rs 调它）。
    pub fn from_id(id: u32) -> LabelRef {
        if id >= Self::MACHINE_LABEL_BASE {
            LabelRef::Machine(id - Self::MACHINE_LABEL_BASE)
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
    /// 下一个可分配的**机器标签**序号（v23 M5：机器标签由 sink 分配，不再用哨兵常量）。
    next_machine_label: u32,
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
            next_machine_label: 0,
            patcher: None,
        }
    }

    /// Attach a backend relocation patcher for ISA-specific fixup encoding.
    /// **分配一个机器标签**（v23 M5：替掉 `ExternalLabel::EPILOGUE` 哨兵）。
    ///
    /// id 落在 [`LabelRef::MACHINE_LABEL_BASE`] 之上的独立区间，与 IR 块 id 天然分离，
    /// 因此"要几个有几个"；调用方在自己的位置 `bind_label` 它即可（前向引用由 pending
    /// fixup 兜住）。
    pub fn new_label(&mut self) -> LabelRef {
        let n = self.next_machine_label;
        self.next_machine_label += 1;
        LabelRef::Machine(n)
    }

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

#[cfg(test)]
mod tests {
    use super::*;

    /// **机器标签由 `new_label` 分配**（v23 M5：替掉 `ExternalLabel::EPILOGUE` 哨兵）。
    ///
    /// 钉三件事：① 每次分配都不同（"要几个有几个"）；② id 落在独立区间，与 IR 块 id
    /// 不可能相撞（这是哨兵时代要 `debug_assert` 才能守的事）；③ id ↔ 标签**互转闭环**
    /// ——编码器把 id 写进指令字段、`use_label_at` 再从数字还原，这条边界错了会静默把
    /// 分支指向别的块。
    #[test]
    fn machine_labels_are_allocated_uniquely_and_round_trip() {
        let mut sink = CodeSink::new();
        let a = sink.new_label();
        let b = sink.new_label();
        assert_ne!(a, b, "两次分配必须是不同的标签");
        assert_ne!(a.id(), b.id(), "id 也必须不同（它是 fixup 的查找键）");

        assert!(
            a.id() >= LabelRef::MACHINE_LABEL_BASE,
            "机器标签 id 必须落在独立区间（{:#X}）",
            LabelRef::MACHINE_LABEL_BASE
        );
        // 真实函数的块数远小于基址——取一个远超实际的上界做代表性检查。
        for block in 0..4096u32 {
            assert_ne!(
                LabelRef::Block(Block::new(block)).id(),
                a.id(),
                "块 {block} 的 id 与机器标签撞了"
            );
        }
        assert_eq!(LabelRef::from_id(a.id()), a, "id → 标签必须闭环");
        assert_eq!(LabelRef::from_id(b.id()), b);
        let blk = LabelRef::Block(Block::new(7));
        assert_eq!(LabelRef::from_id(blk.id()), blk, "IR 块同样闭环");
    }

    /// **前向引用**：先发一条指向机器标签的引用、之后再绑定 ⇒ `finish` 必须打上补丁。
    ///
    /// 这正是删掉"发射次序 hack"后依赖的性质（统一尾声在所有 return 块之后发射）。
    #[test]
    fn forward_machine_label_reference_is_resolved() {
        let mut sink = CodeSink::new();
        let epilogue = sink.new_label();
        // 在偏移 4 处写一条 4 字节的相对引用（补丁站点在偏移 4；目标稍后绑定）。
        sink.put1(0x90);
        sink.put1(0x90);
        sink.put1(0x90);
        sink.put1(0x90);
        let site = sink.offset();
        sink.put1(0);
        sink.put1(0);
        sink.put1(0);
        sink.put1(0);
        sink.use_label_at(site, epilogue, RelocKind::Relative(4, -4));
        // 中间再塞 8 字节，然后绑定尾声标签。
        for _ in 0..8 {
            sink.put1(0x90);
        }
        sink.bind_label(epilogue);
        let code = sink.finish().expect("前向机器标签必须能解析");
        // 目标偏移 16，站点 4 ⇒ 相对 4 字节 = 16 - 4 - 4 = 8。
        assert_eq!(
            &code[site..site + 4],
            &8i32.to_le_bytes(),
            "前向引用应打成 target - site - 4"
        );
    }
}
