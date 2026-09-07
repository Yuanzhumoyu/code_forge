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
        "arm64_v12" => Arc::new(Arm64RelocPatcher),
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

/// AArch64 定宽 32 位：相对偏移重编码进 B/BL imm26 与 B.cond/CBZ/CBNZ
/// imm19 位段（A64 分支 PC = 指令起始地址，offset = target - site）。
pub struct Arm64RelocPatcher;

impl Arm64RelocPatcher {
    /// B/BL：imm26 = offset>>2（[25:0]），要求 offset % 4 == 0。
    fn b_imm26(offset: i64) -> Result<u32, IrError> {
        if offset % 4 != 0 {
            return Err(IrError::Internal(format!(
                "arm64 reloc: B offset {offset} 非 4 对齐"
            )));
        }
        let imm = offset >> 2;
        if !(-0x200_0000..=0x1FF_FFFF).contains(&imm) {
            return Err(IrError::Internal(format!(
                "arm64 reloc: B offset {offset} 超 imm26 范围"
            )));
        }
        Ok((imm as u32) & 0x3FF_FFFF)
    }
    /// B.cond/CBZ/CBNZ：imm19 = offset>>2（[23:5]）。
    fn b_imm19(offset: i64) -> Result<u32, IrError> {
        if offset % 4 != 0 {
            return Err(IrError::Internal(format!(
                "arm64 reloc: branch offset {offset} 非 4 对齐"
            )));
        }
        let imm = offset >> 2;
        if !(-0x4_0000..=0x3_FFFF).contains(&imm) {
            return Err(IrError::Internal(format!(
                "arm64 reloc: branch offset {offset} 超 imm19 范围"
            )));
        }
        Ok((imm as u32) & 0x7_FFFF)
    }
}

impl RelocPatcher for Arm64RelocPatcher {
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
                "arm64 reloc: unsupported kind {kind:?}"
            )));
        };
        let word = u32::from_le_bytes(
            code[offset..offset + 4]
                .try_into()
                .map_err(|_| IrError::Internal("arm64 reloc: 越界".into()))?,
        );
        let d = (target as i64) - (site as i64);
        // B/BL 的 imm26 占 [25:0]——初编码时 label 占位（块号）会污染
        // bit25:24，top8 判定随 imm 漂移（如 0x17FFFFFD top8=0x17），
        // 必须用 top6（[31:26] op6，B=0x05/BL=0x25，恒定）。
        // B.cond/CBZ/CBNZ 的 imm19 在 [23:5]，top8（cbop 8 位）恒定可判。
        let top6 = (word >> 26) & 0x3F;
        let new = match top6 {
            // B(0x05)/BL(0x25)：imm26 [25:0]
            0x05 | 0x25 => (word & !0x03FF_FFFFu32) | Arm64RelocPatcher::b_imm26(d)?,
            _ => {
                let top8 = (word >> 24) & 0xFF;
                match top8 {
                    // B.cond(0x54)/CBZ/CBNZ(0x34/0xB4/0x35/0xB5)：imm19 [23:5]
                    0x54 | 0x34 | 0xB4 | 0x35 | 0xB5 => {
                        (word & !(0x7_FFFFu32 << 5)) | (Arm64RelocPatcher::b_imm19(d)? << 5)
                    }
                    t => {
                        return Err(IrError::Internal(format!(
                            "arm64 reloc: 不支持的指令 top8=0x{t:02x}（word 0x{word:08x}）"
                        )));
                    }
                }
            }
        };
        code[offset..offset + 4].copy_from_slice(&new.to_le_bytes());
        Ok(())
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
        // 由 opcode 判定 J 型（0x6F）、B 型（0x63）、U 型 auipc（0x17）还是
        // I 型 addi（0x13）：前两者是 label/函数符号槽，后两者是 GlobalAddr
        // 的 PC-relative 对（auipc 写 hi20 [31:12]、addi 写 lo12 [31:20]）。
        // 注意：写入前必须先清零 imm 位段——encoder 的占位值（如 Call 的
        // label 槽 = -(FuncRef+1)，可能 = -1 使 imm 位段全 1）与新 enc 是
        // **OR 关系**，不清零会永远保持占位值（实测 jal 恒跳 -1）。
        let opcode = word & 0x7F;
        let (mask, enc) = match opcode {
            0x6F => (0xFFFF_F000u32, Self::uj_imm20(diff)), // JAL：imm20 位段
            0x63 => (0xFE00_0F80u32, Self::b_imm13(diff)),  // B 型：imm13 位段
            // auipc：rd = pc + (imm20 << 12)；hi20 = (diff + 0x800) >> 12
            0x17 => {
                let hi20 = ((diff + 0x800) >> 12) as u32 & 0xFFFFF;
                (0xFFFF_F000u32, hi20 << 12)
            }
            // addi：rd += sext(imm12)；lo12 = (diff + 4) 低 12 位（有符号修正）。
            // 关键：hi20/lo12 都必须相对 **auipc 指令地址**（RISC-V psABI
            // %pcrel_hi/%pcrel_lo 同分母）——addi 紧跟 auipc（定宽 4 字节），
            // 它的 site 比 auipc 大 4，故 +4 对齐（否则地址偏 4，实测 store
            // 写错位）。
            0x13 => {
                let diff = diff + 4;
                let lo12 = (diff & 0xFFF) as u32;
                let lo12 = if lo12 >= 0x800 { lo12 - 0x1000 } else { lo12 };
                (0xFFF0_0000u32, (lo12 & 0xFFF) << 20)
            }
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

    #[test]
    fn arm64_b_imm26_patch() {
        let p = Arm64RelocPatcher;
        // b .（offset 0）；target=site+8 → diff=8 → imm26=2
        let mut code = 0x14000000u32.to_le_bytes().to_vec();
        p.apply(&mut code, 0, RelocKind::Relative(4, 0), 8, 0)
            .unwrap();
        assert_eq!(
            u32::from_le_bytes(code[0..4].try_into().unwrap()),
            0x14000002
        );
        // bl 负偏移：target=site-4 → imm26=-1（全 1 补码）
        let mut code2 = 0x94000000u32.to_le_bytes().to_vec();
        p.apply(&mut code2, 0, RelocKind::Relative(4, 0), 0, 4)
            .unwrap();
        assert_eq!(
            u32::from_le_bytes(code2[0..4].try_into().unwrap()),
            0x94000000 | 0x3FF_FFFF
        );
        // 初编码 label 占位（块号 0xFFFFFFFD 塞入 imm26）污染 top8：
        // word=0x17FFFFFD（top8=0x17）——必须按 top6=0x05 识别 B，
        // patch diff=8 → 0x14000002（imm26 域干净重写）
        let mut code3 = 0x17FFFFFDu32.to_le_bytes().to_vec();
        p.apply(&mut code3, 0, RelocKind::Relative(4, 0), 8, 0)
            .unwrap();
        assert_eq!(
            u32::from_le_bytes(code3[0..4].try_into().unwrap()),
            0x14000002
        );
    }

    #[test]
    fn arm64_imm19_patch_bcond_cbz() {
        let p = Arm64RelocPatcher;
        // b.eq：word 0x54000000；diff=8 → imm19=2（[23:5]）
        let mut code = 0x54000000u32.to_le_bytes().to_vec();
        p.apply(&mut code, 0, RelocKind::Relative(4, 0), 8, 0)
            .unwrap();
        assert_eq!(
            u32::from_le_bytes(code[0..4].try_into().unwrap()),
            0x54000040
        );
        // cbz x0：0xB4000000 同 imm19 位段；diff=12 → imm19=3
        let mut code2 = 0xB4000000u32.to_le_bytes().to_vec();
        p.apply(&mut code2, 0, RelocKind::Relative(4, 0), 12, 0)
            .unwrap();
        assert_eq!(
            u32::from_le_bytes(code2[0..4].try_into().unwrap()),
            0xB4000060
        );
        // cbnz w1：0x35000001 保留 Rt；diff=-4 → imm19=-1
        let mut code3 = 0x35000001u32.to_le_bytes().to_vec();
        p.apply(&mut code3, 0, RelocKind::Relative(4, 0), 0, 4)
            .unwrap();
        assert_eq!(
            u32::from_le_bytes(code3[0..4].try_into().unwrap()),
            0x35000001 | (0x7_FFFFu32 << 5)
        );
    }

    #[test]
    fn arm64_non_aligned_and_unknown_rejected() {
        let p = Arm64RelocPatcher;
        let mut c1 = 0x14000000u32.to_le_bytes().to_vec();
        assert!(
            p.apply(&mut c1, 0, RelocKind::Relative(4, 0), 6, 0)
                .is_err()
        ); // 非 4 对齐
        let mut c2 = 0xD503201Fu32.to_le_bytes().to_vec(); // nop
        assert!(
            p.apply(&mut c2, 0, RelocKind::Relative(4, 0), 8, 0)
                .is_err()
        );
        let mut c3 = vec![0u8; 4];
        assert!(p.apply(&mut c3, 0, RelocKind::ABS4, 0, 0).is_err());
    }
}
