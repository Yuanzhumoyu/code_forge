//! v21 **W11 通用性实证**：第四份 ISA（`isa/demo_v21.toml`）——24 位定宽、16 个 16 位寄存器。
//!
//! 这份夹具的**全部改动**只有：① 一个 TOML 文件；② `tests/common/mod.rs` 一行宿主；
//! ③ `tests/spec_tests.rs` 一行自测宿主；④ 本文件。**没有一行编译器改动**。
//!
//! 它同时把 v21 的现面串起来跑：表头即名字的 `[form.*]`/`[inst.*]`/`[pred.*]`/`[asm.pseudo.*]`、
//! 字段就地 `fields`、`[family.*]` 唯一复用机制（`body` + `rows`）、`[machine]`/`[machine.stack]`
//! 机器事实、谱内 `[[vectors]]`（由 `tests/spec_tests.rs` 的自测宿主真跑）。
//!
//! 编码算术（24 位小端 3 字节）：
//! - RRR：`opcode << 16 | rd << 12 | rs1 << 8 | rs2 << 4`（低 4 位保留）
//! - RRI：`opcode << 16 | rd << 12 | rs1 << 8 | imm`

mod common;

use common::demo_v21::{TargetMachine, assemble, decode, disassemble, encode};
use forge_codegen::TargetMachine as TargetMachineTrait;
use forge_ir::RegClass;

fn enc(asm: &str) -> Vec<u8> {
    let inst = assemble(asm).unwrap_or_else(|e| panic!("assemble `{asm}`: {e}"));
    encode(&inst).unwrap_or_else(|e| panic!("encode `{asm}`: {e}"))
}

/// RRR 的期望字节。
fn rrr(opcode: u32, rd: u32, rs1: u32, rs2: u32) -> Vec<u8> {
    let w = (opcode << 16) | (rd << 12) | (rs1 << 8) | (rs2 << 4);
    w.to_le_bytes()[..3].to_vec()
}

/// RRI 的期望字节（`rs1` 未绑定时为 0）。
fn rri(opcode: u32, rd: u32, rs1: u32, imm: u32) -> Vec<u8> {
    let w = (opcode << 16) | (rd << 12) | (rs1 << 8) | imm;
    w.to_le_bytes()[..3].to_vec()
}

// ───────────────── 1. 寄存器组/宽度全部由 TOML 派生 ─────────────────

#[test]
fn metadata_is_derived_from_the_spec_alone() {
    let tm = TargetMachine::new();
    let ri = TargetMachineTrait::reg_info(&tm);
    assert_eq!(
        ri.default_gpr_class(),
        RegClass::GPR(16),
        "主 GPR 类 = 组名里的位宽（gpr16）"
    );
    assert_eq!(ri.slot_bytes(), 2, "[machine.stack].slot = 16 位 = 2 字节");
}

// ───────────────── 2. 三寄存器/立即数形态的字节精确 ─────────────────

#[test]
fn rrr_and_rri_bytes_are_exact() {
    assert_eq!(enc("add R1, R2, R3"), rrr(0x01, 1, 2, 3), "add");
    assert_eq!(enc("sub R4, R5, R6"), rrr(0x02, 4, 5, 6), "sub");
    assert_eq!(enc("and R7, R8, R9"), rrr(0x03, 7, 8, 9), "and");
    assert_eq!(enc("addi R1, R2, 5"), rri(0x11, 1, 2, 5), "addi");
    // MOVI 的 `rs1` 字段没有操作数绑定 ⇒ 按缺省 0 发射。
    assert_eq!(enc("movi R3, 7"), rri(0x12, 3, 0, 7), "movi（rs1 缺省 0）");
}

// ───────────────── 3. `[family.*]`（body + rows）生成的实例可用 ─────────────────

#[test]
fn family_rows_become_instructions() {
    assert_eq!(enc("shift.l R1, R2, 1"), rri(0x20, 1, 2, 1), "SHL");
    assert_eq!(enc("shift.r R2, R3, 4"), rri(0x21, 2, 3, 4), "SHR");
    assert_eq!(enc("shift.a R3, R4, 8"), rri(0x22, 3, 4, 8), "SAR");
}

// ───────────────── 4. `[asm.pseudo.*]`：伪指令展开到真指令 ─────────────────

#[test]
fn pseudo_instruction_expands_to_a_real_one() {
    let pseudo = enc("li R1, 5");
    let real = enc("movi R1, 5");
    assert_eq!(pseudo, real, "`li` 应与它 emit 的 `movi` 编出同样的字节");
}

// ───────────────── 5. 解码/反汇编往返（v21 面在"读"方向也成立） ─────────────────

#[test]
fn decode_and_disassemble_round_trip() {
    for asm in [
        "add R1, R2, R3",
        "sub R4, R5, R6",
        "and R7, R8, R9",
        "addi R1, R2, 5",
        "movi R3, 7",
        "shift.l R1, R2, 1",
    ] {
        let bytes = enc(asm);
        let (inst, used) = decode(&bytes).unwrap_or_else(|| panic!("decode `{asm}` 失败"));
        assert_eq!(used, 3, "24 位字应吃满 3 字节（`{asm}`）");
        assert_eq!(
            encode(&inst).unwrap(),
            bytes,
            "decode→encode 字节不变（`{asm}`）"
        );
        let text = disassemble(&inst);
        assert_eq!(enc(&text), bytes, "反汇编文本应能再编回同样字节：`{text}`");
    }
}

// ───────────────── 6. 残缺输入不得解出一个字 ─────────────────

#[test]
fn truncated_input_decodes_to_nothing() {
    let bytes = enc("add R1, R2, R3");
    assert!(decode(&bytes[..2]).is_none(), "2 字节不足一个 24 位字");
    assert!(decode(&[]).is_none(), "空缓冲");
    assert!(
        decode(&[0x00, 0x00, 0x00]).is_none(),
        "opcode 0 未定义（0 是保留值）"
    );
}
