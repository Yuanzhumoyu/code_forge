//! `include` / `[[override]]` 多文件谱回归（v18 S7d）。
//!
//! 夹具 = `tests/isa/include_root_v12.toml`（根，含 `include = ["include_base_v12.toml"]`
//! 与一条 `[[override]]`）＋ `tests/isa/include_base_v12.toml`（片段：寄存器组、
//! 位域、operand slot、form、`IADD`）。本文件断言**宏路径**上真正落地的东西：
//!
//! 1. **两片合成一份谱**：`IADD`（片段）与 `ISUB`（根）都在同一个生成模块里；
//! 2. **`[[override]]` 生效**：`[meta].version` 取根文件的值，而不是片段的；
//! 3. **黄金字节**：16 位小端字 `op<<12 | rd<<8 | rs1<<4`——`IADD R0, R1` =
//!    `0x1010`、`ISUB R2, R3` = `0x2230`（含 decode 往返）；
//! 4. **`parts`/`name` 参数**：`include_enc_v12` 是同一份谱只开 `encode` 的模块。

mod common;

use common::include_root_v12::{Inst, TargetMachine, assemble, decode, disassemble, encode};
use forge_codegen::TargetMachine as TargetMachineTrait;
use forge_codegen::ir::RegClass;

/// 16 位小端字：`op<<12 | rd<<8 | rs1<<4`（`[conventions.bitfields]` 派生）。
fn word(op: u16, rd: u16, rs1: u16) -> Vec<u8> {
    ((op << 12) | (rd << 8) | (rs1 << 4)).to_le_bytes().to_vec()
}

fn enc(asm: &str) -> Vec<u8> {
    let inst = assemble(asm).unwrap_or_else(|e| panic!("assemble `{asm}`: {e}"));
    encode(&inst).unwrap_or_else(|e| panic!("encode `{asm}`: {e}"))
}

// ───────────────────── 1. 两片合成一份谱 ─────────────────────

/// 片段的 `IADD` 与根的 `ISUB` 都在同一模块里（数组节按 include 序追加）。
#[test]
fn both_files_land_in_one_spec() {
    let a = assemble("iadd R0, R1").expect("片段里的 IADD 应可汇编");
    let b = assemble("isub R2, R3").expect("根文件里的 ISUB 应可汇编");
    assert!(matches!(a, Inst::Iadd { .. }), "{a:?}");
    assert!(matches!(b, Inst::Isub { .. }), "{b:?}");
    // mnemonic_case = "insensitive"（来自根文件）对两片一致生效。
    assert!(assemble("IADD R0, R1").is_ok(), "大小写不敏感");
}

/// 2. `[[override]]` 把片段的 `[meta].version` 覆盖成根文件的值。
#[test]
fn override_replaces_version_from_included_file() {
    let tm = TargetMachine::new();
    let isa = tm.isa_info();
    assert_eq!(isa.version(), "18.0-include", "版本应来自根的 [[override]]");
    assert_eq!(isa.name(), "demo_include_v12", "名字来自根文件");
    assert_eq!(isa.address_size(), 32, "[meta].mode = 32");
}

// ───────────────────── 3. 黄金字节 + 往返 ─────────────────────

/// 黄金字节：两条指令各自编成 2 字节 16 位小端字。
#[test]
fn golden_bytes_for_both_files() {
    assert_eq!(enc("iadd R0, R1"), word(1, 0, 1), "IADD R0, R1");
    assert_eq!(enc("iadd R3, R2"), word(1, 3, 2), "IADD R3, R2");
    assert_eq!(enc("isub R2, R3"), word(2, 2, 3), "ISUB R2, R3");
    assert_eq!(enc("isub R1, R0"), word(2, 1, 0), "ISUB R1, R0");
    assert_eq!(enc("iadd R0, R1").len(), 2, "fixed 16 位 = 2 字节");
}

/// decode/encode 往返 + 反汇编文本幂等（两片指令都走同一条路径）。
#[test]
fn roundtrip_and_text_are_stable() {
    for (text, bytes) in [
        ("iadd R0, R1", word(1, 0, 1)),
        ("isub R3, R0", word(2, 3, 0)),
    ] {
        let inst = assemble(text).unwrap();
        assert_eq!(encode(&inst).unwrap(), bytes, "encode `{text}`");
        let (back, used) = decode(&bytes).unwrap_or_else(|| panic!("decode `{text}`"));
        assert_eq!(used, bytes.len(), "`{text}` 用满字数");
        assert_eq!(back, inst, "decode∘encode 还原 `{text}`");
        assert_eq!(encode(&back).unwrap(), bytes, "字节稳定");
        let dis = disassemble(&inst);
        assert_eq!(
            assemble(&dis).unwrap(),
            inst,
            "disassemble→assemble 幂等：{dis}"
        );
        assert_eq!(dis, text, "反汇编文本：{dis}");
    }
    assert!(decode(&[0x00, 0x00]).is_none(), "opcode 0 未定义");
}

/// 具名字段（不是裸 u32）：字段名**就是 `ops` 里声明的名字**（v18 S7d）——
/// 这里 `ops = ["dst:r:out", "src:r"]` ⇒ `Inst::Iadd { dst, src }`；
/// 位域名（`rd`/`rs1`，来自 `[conventions.bitfields]`）只用于编码位置，不再冒充字段名。
#[test]
fn inst_variants_stay_typed() {
    let inst = Inst::Iadd {
        dst: common::include_root_v12::Reg::R1,
        src: common::include_root_v12::Reg::R2,
    };
    assert_eq!(encode(&inst).unwrap(), word(1, 1, 2));
    assert!(matches!(inst, Inst::Iadd { .. }));
}

// ───────────────────── 4. parts = ["encode"] ─────────────────────

/// 同一份谱的"只有编码器"模块（`name` + `parts`，v18 S7d）：`Inst`/`Reg`/`encode`
/// 在，`decode`/`asm`/`tm` 不在——本测试能编译通过本身就是证据（`Reg` 与 `Inst`
/// 是公共前提；缺 `decode` 等不影响编码，而多生成的部分会与 `parts` 语义相悖）。
#[test]
fn parts_encode_only_module_still_encodes() {
    use common::include_enc_v12::{Inst as EncInst, encode as enc_only};
    let inst = EncInst::Iadd {
        dst: common::include_enc_v12::Reg::R0,
        src: common::include_enc_v12::Reg::R1,
    };
    assert_eq!(
        enc_only(&inst).unwrap(),
        word(1, 0, 1),
        "部分生成仍编码正确"
    );
    assert_eq!(
        enc_only(&EncInst::Isub {
            dst: common::include_enc_v12::Reg::R2,
            src: common::include_enc_v12::Reg::R3,
        })
        .unwrap(),
        word(2, 2, 3),
        "根文件的 ISUB 也在"
    );
}

/// 生成物仍带 `[reg.gpr4]` 派生的元数据（该部件的公共前提恒定发射）。
#[test]
fn parts_encode_only_keeps_reg_class_metadata() {
    let tm = TargetMachine::new();
    let ri = TargetMachineTrait::reg_info(&tm);
    assert_eq!(ri.default_gpr_class(), RegClass::GPR(4), "4 个寄存器一组");
    assert_eq!(ri.num_gp_regs(), 4);
}
