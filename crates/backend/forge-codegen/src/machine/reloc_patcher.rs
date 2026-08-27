//! RelocPatcher — ISA-specific relocation encoder.
//!
//! The relocation framework only deals with generic concepts
//! ([`RelocKind::Absolute`] / [`RelocKind::Relative`]); how a relocation is
//! encoded into machine code bytes is backend-specific (x86 writes a plain
//! 32-bit relative displacement). Each backend implements this trait and
//! hands it to the pipeline via
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
/// 懒注册兜底：生产路径（FunctionCompiler/CodeSink）不显式调用
/// `ensure_registered()`——首次查询时按已知 ISA 自动注册，保证
/// CodeSink.finish 的 label fixup 走 patcher 位段重排（否则定宽 ISA 的
/// Relative(4,0) 会落到 legacy 平铺写 diff 路径，字节里只剩偏移无 opcode）。
pub fn reloc_patcher_for(isa: &str) -> Option<Arc<dyn RelocPatcher>> {
    if let Some(p) = RELOC_PATCHERS
        .get()
        .and_then(|m| m.lock().ok())
        .and_then(|g| g.get(isa).cloned())
    {
        return Some(p);
    }
    register_default_reloc_patcher(isa); // 幂等（contains_key 检查）
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
        "x86_64" | "x86_64_v12" | "x86_v12" => Arc::new(X86RelocPatcher),
        // riscv64_v12 是定宽 32 位模块（v12 TOML 的 name 键）。
        "riscv64_v12" => Arc::new(RiscvRelocPatcher),
        _ => return,
    };
    register_reloc_patcher(isa, patcher);
}

// ─────────────────────────────────────────────────────────────
// Backend implementations
// ─────────────────────────────────────────────────────────────

/// RISC-V 定宽 32 位：相对偏移重编码进 UJ 型（JAL）/ B 型（分支）位段。
/// `RelocKind::Relative(4, 0)`：`target - site`（site = 指令起始地址）写
/// 入 imm20（JAL：bit[31,21,20,12..31 布局]）或 imm13（B 型分散 4 段）。
pub struct RiscvRelocPatcher;

impl RiscvRelocPatcher {
    /// UJ 型（JAL）：imm[20|10:1|11|19:12]。
    fn uj_imm20(imm: i64) -> u32 {
        let v = imm as u32 & 0x1F_FFFF;
        (v >> 20 & 1) << 31 | (v >> 1 & 0x3FF) << 21 | (v >> 11 & 1) << 20 | (v >> 12 & 0xFF) << 12
    }
    /// B 型（BEQ/BNE/…）：imm[12|10:5|4:1|11]。
    fn b_imm13(imm: i64) -> u32 {
        let v = imm as u32 & 0x1FFF;
        (v >> 12 & 1) << 31 | (v >> 5 & 0x3F) << 25 | (v >> 1 & 0xF) << 8 | (v >> 11 & 1) << 7
    }
}

impl RelocPatcher for RiscvRelocPatcher {
    fn apply(
        &self,
        code: &mut [u8],
        offset: usize,
        kind: RelocKind,
        target: u64,
        site: u64,
    ) -> Result<(), IrError> {
        let RelocKind::Relative(4, _) = kind else {
            return Err(IrError::Internal(format!(
                "riscv reloc: unsupported kind {kind:?}"
            )));
        };
        if offset + 4 > code.len() {
            return Err(IrError::Internal(format!(
                "riscv reloc: offset {offset} out of range (len {})",
                code.len()
            )));
        }
        let diff = target as i64 - site as i64;
        let word = u32::from_le_bytes(code[offset..offset + 4].try_into().unwrap());
        // 由 opcode 判定 J 型（0x6F）还是 B 型（0x63）：其余立即数槽（I/S/U）
        // 不是 label 槽（本 patcher 只服务 encoder 的 label fixup）。
        // 注意：写入前必须先清零 imm 位段——encoder 的占位值（如 Call 的
        // label 槽 = -(FuncRef+1)，可能 = -1 使 imm 位段全 1）与新 enc 是
        // **OR 关系**，不清零会永远保持占位值（实测 jal 恒跳 -1）。
        let opcode = word & 0x7F;
        let (mask, enc) = match opcode {
            0x6F => (0xFFFF_F000u32, Self::uj_imm20(diff)), // JAL：imm20 位段
            0x63 => (0xFE00_0F80u32, Self::b_imm13(diff)),  // B 型：imm13 位段
            _ => {
                return Err(IrError::Internal(format!(
                    "riscv reloc: unexpected opcode 0x{opcode:02x} at {offset}"
                )));
            }
        };
        code[offset..offset + 4].copy_from_slice(&(word & !mask | enc).to_le_bytes());
        Ok(())
    }
}

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
    fn riscv_jal_positive() {
        let p = RiscvRelocPatcher;
        // JAL rd=0, +0x1000：指令字 = 0x6F | uj(0x1000)
        let mut code = 0x6Fu32.to_le_bytes().to_vec();
        p.apply(&mut code, 0, RelocKind::Relative(4, 0), 0x1000, 0)
            .unwrap();
        let w = u32::from_le_bytes(code[0..4].try_into().unwrap());
        assert_eq!(w, 0x6F | RiscvRelocPatcher::uj_imm20(0x1000));
    }

    #[test]
    fn riscv_jal_negative() {
        let p = RiscvRelocPatcher;
        // 负偏移：-4（diff = target - site）
        let mut code = 0x6Fu32.to_le_bytes().to_vec();
        p.apply(&mut code, 0, RelocKind::Relative(4, 0), 0x0, 0x4)
            .unwrap();
        let w = u32::from_le_bytes(code[0..4].try_into().unwrap());
        assert_eq!(w, 0x6F | RiscvRelocPatcher::uj_imm20(-4));
    }

    #[test]
    fn riscv_branch_b_type() {
        let p = RiscvRelocPatcher;
        // BEQ（opcode 0x63）：B 型 imm13 分散位段
        let mut code = 0x63u32.to_le_bytes().to_vec();
        p.apply(&mut code, 0, RelocKind::Relative(4, 0), 0x24, 0x0)
            .unwrap();
        let w = u32::from_le_bytes(code[0..4].try_into().unwrap());
        assert_eq!(w, 0x63 | RiscvRelocPatcher::b_imm13(0x24));
    }

    #[test]
    fn riscv_jal_overwrites_placeholder_imm() {
        // encoder 的 Call label 槽占位值 = -(FuncRef+1)：FuncRef 0 → -1 →
        // imm20 位段全 1（指令字 0xFFFFF0EF = jal ra, -1）。patch 必须清掉
        // 占位位段再写新 imm，否则 OR 后保持全 1（恒跳 -1）。
        let p = RiscvRelocPatcher;
        let mut code = 0xFFFF_F0EFu32.to_le_bytes().to_vec();
        // site=0x88、target=0x30 → diff = -88（QEMU 模块 main 的 jal 场景）
        p.apply(
            &mut code,
            0,
            RelocKind::Relative(4, 0),
            0x8000_0030,
            0x8000_0088,
        )
        .unwrap();
        let w = u32::from_le_bytes(code[0..4].try_into().unwrap());
        // 占位 0xFFFFF0EF = jal ra(rd=1), -1：patch 后 rd 保留，imm 覆盖为 -88
        assert_eq!(
            w,
            0xEF | RiscvRelocPatcher::uj_imm20(-88),
            "占位 imm 被覆盖"
        );
        assert_ne!(w, 0xFFFF_F0EF, "不再保持 -1");
    }

    #[test]
    fn riscv_branch_overwrites_placeholder_imm() {
        // B 型同理：占位 imm13 全 1 → patch 后应为新 imm。
        let p = RiscvRelocPatcher;
        // beq x0,x0,-1：imm13 = -1 → 位段全 1：0x63 | b_imm13(-1)
        let mut code = (0x63u32 | RiscvRelocPatcher::b_imm13(-1))
            .to_le_bytes()
            .to_vec();
        p.apply(&mut code, 0, RelocKind::Relative(4, 0), 0x24, 0x0)
            .unwrap();
        let w = u32::from_le_bytes(code[0..4].try_into().unwrap());
        assert_eq!(
            w,
            0x63 | RiscvRelocPatcher::b_imm13(0x24),
            "占位 B 型 imm 被覆盖"
        );
    }

    #[test]
    fn riscv_unsupported_kind_rejected() {
        let p = RiscvRelocPatcher;
        let mut code = vec![0u8; 4];
        assert!(p.apply(&mut code, 0, RelocKind::ABS4, 0, 0).is_err());
    }
}
