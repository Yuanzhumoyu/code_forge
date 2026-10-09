//! RelocPatcher — ISA 侧的重定位编码入口。
//!
//! 重定位框架只处理通用概念（[`RelocKind::Absolute`] / [`RelocKind::Relative`]）；
//! "重定位值怎么进机器码字节"是 **ISA 的事**。v21 W8 起，定宽 ISA 的补丁器
//! **由谱面生成**（`[asm.reloc.<名>]` 的 `operand`/`via`/`addend` ＋ 该指令 `Label` 槽
//! 的字段散布位段），生成物在 `impl TargetMachine` 里覆写
//! [`TargetMachine::reloc_patcher`](crate::machine::target::TargetMachine)，
//! 因此**不再有宿主按 ISA 名的分发，也没有手写补丁器**。
//!
//! 变长（`prefix_scan`）ISA 不生成补丁器：那边 label 槽是尾部连续 imm，
//! [`CodeSink::finish`](crate::emit::CodeSink::finish) 的通用路径本就正确
//! （x86 因此也删掉了自己那个与通用路径逐字节等价的 patcher）。
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

/// Per-ISA registration table（显式注册入口：夹具/自定义后端用）。
static RELOC_PATCHERS: OnceLock<Mutex<HashMap<String, Arc<dyn RelocPatcher>>>> = OnceLock::new();

/// Register a backend's relocation patcher under its ISA name.
pub fn register_reloc_patcher(isa: &str, patcher: Arc<dyn RelocPatcher>) {
    let map = RELOC_PATCHERS.get_or_init(|| Mutex::new(HashMap::new()));
    map.lock().unwrap().insert(isa.to_string(), patcher);
}

/// Look up a backend's patcher by ISA name（未注册 ⇒ `None` ⇒ 走通用路径）。
///
/// v21 W8：这里**不再**按 ISA 名懒惰注册缺省补丁器——定宽 ISA 由生成物覆写
/// `TargetMachine::reloc_patcher()` 提供；变长 ISA 用通用路径本就正确。
pub fn reloc_patcher_for(isa: &str) -> Option<Arc<dyn RelocPatcher>> {
    RELOC_PATCHERS
        .get()
        .and_then(|m| m.lock().ok())
        .and_then(|g| g.get(isa).cloned())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::emit::CodeSink;
    use crate::ir::Block;

    /// x86 语义（v21 W8 起由**通用路径**承担，patcher 已删）：rel32 = target − site + adj。
    /// 这里直接钉住 `CodeSink::finish` 的通用写入，防止将来"修"成别的语义。
    #[test]
    fn generic_path_patches_rel32_like_x86() {
        let mut s = CodeSink::new(); // 无 patcher ⇒ 通用路径
        s.put1(0xE8);
        s.put4(0);
        s.put_bytes(&[0, 0, 0, 0]);
        s.bind_label(Block::new(1)); // label 落在 offset 9
        s.use_label_at(1, Block::new(1), RelocKind::REL4);
        let code = s.finish().unwrap();
        // target(9) − site(1) + adj(−4) = 4
        assert_eq!(&code[1..5], &4i32.to_le_bytes());
    }

    /// x86 绝对地址（8 字节平铺 LE）同理走通用路径。
    #[test]
    fn generic_path_patches_abs8_like_x86() {
        let mut s = CodeSink::new();
        s.put_bytes(&[0u8; 8]);
        s.bind_label(Block::new(2)); // label 落在 offset 8
        s.use_label_at(0, Block::new(2), RelocKind::ABS8);
        let code = s.finish().unwrap();
        assert_eq!(&code, &8u64.to_le_bytes());
    }
}
