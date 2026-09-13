//! demo_inst8_v12 — **1 字节指令字 ISA** 回归测试（去「指令字宽写死」）。
//!
//! 夹具（`tests/isa/demo_inst8_v12.toml`）的 `[meta].default_inst_width = 8`：
//! 8 位字 = 2 位 opcode + 三个 2 位寄存器域。历史实现把「定宽 = 4 字节」写死在
//! `codegen/mod.rs`（encode 出 4 字节、decode 读 4 字节、返回消费 4）与
//! `codegen/machine.rs`（`RelocKind::Relative(4, 0)`）——本文件断言字宽已成为
//! 真正的 ISA 数据：
//!
//! 1. **encode** 产出**字长字节**（1 字节），**decode** 读同一宽度并返回
//!    `consumed = 1`；不足一个字 → None（不越界读）。
//! 2. **汇编/反汇编往返**在 1 字节字上仍然成立。
//! 3. **分支 fixup 宽度 = 字长**：生成物里是 `Relative(1, 0)`（不是 4），
//!    且 label 域落在**字内**（bits 0..4）——由该 ISA 的 `RelocPatcher`
//!    重排位段（本文件注册一个 4 位域 patcher），通用写入路径不参与。

mod common;

use common::demo_inst8_v12::{Inst, Reg, assemble, decode, decode_partial, disassemble, encode};
use forge_codegen::AllocResult;
use forge_codegen::CodeSink;
use forge_codegen::TargetMachine as TargetMachineTrait;
use forge_codegen::ir::Block;
use forge_codegen::machine::reloc_patcher::RelocPatcher;
use forge_codegen::{IrError, RelocKind};
use std::sync::Arc;

/// 8 位字的位布局（与 TOML 的 `[conventions.bitfields]` 一致）：
/// `op(2) | rd(2) | rs1(2) | rs2(2)`，高位在左。
fn word(op: u8, rd: u8, rs1: u8, rs2: u8) -> u8 {
    (op << 6) | (rd << 4) | (rs1 << 2) | rs2
}

fn enc(asm: &str) -> Vec<u8> {
    let inst = assemble(asm).unwrap_or_else(|e| panic!("assemble `{asm}`: {e}"));
    encode(&inst).unwrap_or_else(|e| panic!("encode `{asm}`: {e}"))
}

// ───────────────── 1. 字长 = ISA 数据（核心回归）─────────────────

/// encode 出**1 字节**（历史：定宽固定 4 字节）；decode 读 1 字节并返回 1。
#[test]
fn instruction_word_is_one_byte() {
    // nop = 全字常量 0
    assert_eq!(
        enc("nop"),
        vec![0x00],
        "nop = 8 位字 0（1 字节，不是 4 字节）"
    );
    // add a1, a2, a3 → op=1 | rd=1 | rs1=2 | rs2=3
    assert_eq!(enc("add a1, a2, a3"), vec![word(1, 1, 2, 3)]);
    // mov a0, a3 → op=2 | rd=0 | rs1=3
    assert_eq!(enc("mov a0, a3"), vec![word(2, 0, 3, 0)]);

    let (inst, n) = decode(&[word(1, 1, 2, 3)]).expect("decode 1 字节字");
    assert_eq!(n, 1, "消费字节数 = 字长 1（历史写死 4）");
    assert_eq!(
        inst,
        Inst::Add8 {
            rd: Reg::A1,
            rs1: Reg::A2,
            rs2: Reg::A3,
        }
    );
}

/// 不足一个字 → None；`decode_partial` 报告已给字节数（不 panic、不越界读）。
#[test]
fn decode_requires_a_full_word() {
    assert!(decode(&[]).is_none(), "空缓冲 → 无完整字");
    assert_eq!(decode_partial(&[]), Err(0), "0 字节 → Err(0)（< 1 字节）");
    assert!(decode(&[word(1, 0, 0, 0)]).is_some(), "恰好 1 字节可解码");
}

/// 全部指令 encode → decode 往返一致（Inst 相等）。
#[test]
fn all_instructions_roundtrip() {
    for asm in ["nop", "add a3, a2, a1", "mov a2, a3"] {
        let bytes = enc(asm);
        assert_eq!(bytes.len(), 1, "`{asm}` 必须是 1 字节字");
        let (inst, n) = decode(&bytes).unwrap_or_else(|| panic!("decode `{asm}`"));
        assert_eq!(n, 1);
        assert_eq!(encode(&inst).expect("re-encode"), bytes, "`{asm}` 往返字节");
    }
}

/// 汇编 ↔ 反汇编文本往返（寄存器名大写化）。
#[test]
fn asm_disasm_roundtrip() {
    let inst = assemble("add a1, a2, a3").expect("assemble");
    assert_eq!(disassemble(&inst), "add A1, A2, A3");
    let inst = assemble("nop").expect("assemble");
    assert_eq!(disassemble(&inst), "nop");
}

// ───────────────── 2. 分支 fixup 宽度 = 字长 ─────────────────

/// 1 字节字 ISA 的 2 位 label 域 patcher：把 `target - site` 写进 bits 0..2，
/// 保留高位（opcode/rs1）。真实定宽 ISA（riscv JAL/B、arm64 imm26）做同样的事，
/// 只是位段更复杂——说明"字内位段重排属于 ISA"，而"fixup 宽度 = 字长"属于框架。
struct LabelFieldPatcher;

impl RelocPatcher for LabelFieldPatcher {
    fn apply(
        &self,
        code: &mut [u8],
        offset: usize,
        kind: RelocKind,
        target: u64,
        site: u64,
    ) -> Result<(), IrError> {
        // 框架必须按**字长**给出 fixup 宽度（1 字节 = 本 ISA 的字长）。
        assert_eq!(
            kind,
            RelocKind::Relative(1, 0),
            "定宽 label fixup 的宽度应为字长 1 字节（历史写死 4）"
        );
        let rel = target as i64 - site as i64;
        let cur = code[offset];
        code[offset] = (cur & !0b11) | ((rel as u8) & 0b11);
        Ok(())
    }
}

/// `brz a1, <label>`：encoder 记录 `Relative(1, 0)`，patcher 把相对偏移写进
/// 字内 4 位域——一条指令恰好 1 字节，且 opcode/rs1 位不被破坏。
#[test]
fn branch_fixup_uses_word_width() {
    let tm = common::demo_inst8_v12::TargetMachine::new();
    let mut sink = CodeSink::default();
    sink.set_patcher(Arc::new(LabelFieldPatcher));
    let inst = Inst::Brz8 {
        rs1: Reg::A1,
        lab: 1, // label 槽在 IR 里是块号（正数 → use_label_at；负数 = 函数符号）
    };
    tm.encoder()
        .encode(&inst, &AllocResult::new(), &mut sink)
        .expect("encode brz");
    assert_eq!(sink.bytes().len(), 1, "1 字节字");
    let placeholder = sink.bytes()[0];
    assert_eq!(
        placeholder >> 6,
        3,
        "opcode 位（bits 6..8）在占位编码里已就位"
    );
    // 目标标签紧跟在分支之后 → rel = 1
    sink.bind_label(Block(1));
    let code = sink.finish().expect("finish 补 fixup");
    assert_eq!(code.len(), 1);
    assert_eq!(
        code[0],
        word(3, 0, 1, 1),
        "op=3 | rs1=A1(1) | label 域 = rel(1)；高位未被 fixup 覆盖"
    );
}
