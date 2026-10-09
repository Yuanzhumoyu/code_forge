//! v21 W8：重定位补丁器**由谱面生成**后，这里钉住它的关键语义
//! （旧的手写 `RiscvRelocPatcher`/`Arm64RelocPatcher` 及其单测已随宿主实现删除）。
//!
//! 覆盖：riscv64 的 J/U/B 型**散布位段**、`via` 值变换（hi20/lo12）、
//! `addend`（走 `RelocKind::Relative` 的修正字段）、arm64 的 imm26/imm19，
//! 以及"占位值必须被**覆盖**而不是 OR 上去"（历史回归：jal 恒跳 -1）。

use forge_codegen::arm64::RelocPatcherImpl as Arm64;
use forge_codegen::riscv64::RelocPatcherImpl as Riscv;
use forge_codegen::{RelocKind, RelocPatcher};

fn le(w: u32) -> Vec<u8> {
    w.to_le_bytes().to_vec()
}

fn word(code: &[u8]) -> u32 {
    u32::from_le_bytes(code[0..4].try_into().unwrap())
}

#[test]
fn riscv_jal_scattered_imm20() {
    // jal x0, +0x1000：imm20 散布 [20|10:1|11|19:12]，这里只有 [12] 有值。
    let mut code = le(0x0000_006F);
    Riscv
        .apply(&mut code, 0, RelocKind::Relative(4, 0), 0x1000, 0)
        .unwrap();
    assert_eq!(word(&code), 0x0000_106F);
}

#[test]
fn riscv_jal_overwrites_placeholder_imm() {
    // encoder 的 Call label 槽占位值 = -(FuncRef+1)：FuncRef 0 → -1 → imm20 位段全 1
    //（`jal ra, -1` = 0xFFFFF0EF）。patch 必须先清掉占位位段（旧实现是 OR 上去 ⇒ 恒跳 -1）。
    let mut code = le(0xFFFF_F0EF);
    Riscv
        .apply(
            &mut code,
            0,
            RelocKind::Relative(4, 0),
            0x8000_0030,
            0x8000_0088,
        )
        .unwrap();
    // d = -88 ⇒ UJ 位段（rd=ra 保留）
    assert_eq!(word(&code), 0xFA9F_F0EF);
    assert_ne!(word(&code), 0xFFFF_F0EF);
}

#[test]
fn riscv_branch_b_type_scattered_imm13() {
    // beq x0, x0, +0x24：imm13 分散 [12|10:5|4:1|11]。
    let mut code = le(0x0000_0063);
    Riscv
        .apply(&mut code, 0, RelocKind::Relative(4, 0), 0x24, 0)
        .unwrap();
    assert_eq!(word(&code), 0x0200_0263);
}

#[test]
fn riscv_auipc_hi20_uses_via_transform() {
    // `[asm.reloc.pcrel_hi].via = "hi"` ⇒ hi20 = (d + 0x800) >> 12。
    // d = 0x1234 ⇒ (0x1234 + 0x800) >> 12 = 1 ⇒ 位段 [31:12] = 1。
    let mut code = le(0x0000_0017);
    Riscv
        .apply(&mut code, 0, RelocKind::Relative(4, 0), 0x1234, 0)
        .unwrap();
    assert_eq!(word(&code), 0x0000_1017);
}

#[test]
fn riscv_addi_lo12_uses_addend_and_via() {
    // pcrel_lo：`addend = 4`（addi 紧跟 auipc，偏移相对 auipc 算）＋ `via = "lo"`
    //（低 12 位按有符号读数）。起点用**真实占位形态** `addi a0, a0, -1`（imm 位段全 1）：
    // site=0x100、target=0x1334 ⇒ d = 0x1234 + 4 = 0x1238 ⇒ lo12 = 0x238
    // ⇒ 位段 [31:20] = 0x238（rd/rs1 保留 ⇒ 0x23850513）。
    let mut code = le(0xFFF5_0513);
    Riscv
        .apply(&mut code, 0, RelocKind::Relative(4, 4), 0x1334, 0x100)
        .unwrap();
    assert_eq!(word(&code), 0x2385_0513);
}

/// **已知边界（大声报错，不静默不动）**：补丁器按"解码这条指令"来选字段，而
/// **imm = 0** 的 `addi` 形状会先命中 riscv 自己的无 imm 操作数变体
/// （`MOV_RM8_R64`，即 `addi rd, rs, 0` 的搬运形态；全零时是 `Nop`）⇒ 没有可写的字段
/// ⇒ 返回错误。真实 GlobalAddr 序列的 imm 是负占位值（Funcref/GlobalId 编码），
/// 因此走 `Addi` 那条（riscv64 JIT 矩阵覆盖）。
#[test]
fn degenerate_operandless_encoding_is_rejected_loudly() {
    let mut code = le(0x0005_0513); // `addi a0, a0, 0` 的搬运形状（无 imm 操作数）
    let err = Riscv
        .apply(&mut code, 0, RelocKind::Relative(4, 4), 0x1334, 0x100)
        .unwrap_err();
    assert!(
        format!("{err:?}").contains("不承载重定位"),
        "应明确报错：{err:?}"
    );
    assert_eq!(word(&code), 0x0005_0513, "报错时不得改动字节");
}

#[test]
fn arm64_b_imm26_and_placeholder() {
    // b .+8 ⇒ imm26 = 2（[25:0] 单位 4 字节）。
    let mut code = le(0x1400_0000);
    Arm64
        .apply(&mut code, 0, RelocKind::Relative(4, 0), 8, 0)
        .unwrap();
    assert_eq!(word(&code), 0x1400_0002);

    // 初编码的 label 占位（尾声哨兵塞进 imm26）污染高位：0x17FFFFFD 也必须按
    // **解码结果**（B）识别并干净重写 imm26。
    let mut code = le(0x17FF_FFFD);
    Arm64
        .apply(&mut code, 0, RelocKind::Relative(4, 0), 8, 0)
        .unwrap();
    assert_eq!(word(&code), 0x1400_0002);
}

#[test]
fn arm64_imm19_patch_bcond_cbz() {
    // b.eq .+12 ⇒ imm19 = 3（[23:5]）。
    let mut code = le(0x5400_0000);
    Arm64
        .apply(&mut code, 0, RelocKind::Relative(4, 0), 12, 0)
        .unwrap();
    assert_eq!(word(&code), 0x5400_0060);

    // cbz x0（0xB4000000）同 imm19 位段。
    let mut code = le(0xB400_0000);
    Arm64
        .apply(&mut code, 0, RelocKind::Relative(4, 0), 12, 0)
        .unwrap();
    assert_eq!(word(&code), 0xB400_0060);
}

#[test]
fn unsupported_kind_is_rejected() {
    // 定宽 ISA 的 label/global 只发 pc_relative；绝对重定位要明确报错（不静默不动）。
    let mut code = le(0x0000_006F);
    assert!(
        Riscv.apply(&mut code, 0, RelocKind::ABS4, 0, 0).is_err(),
        "定宽 ISA 不支持绝对重定位"
    );
    assert_eq!(word(&code), 0x0000_006F, "报错时不得改动字节");
}
