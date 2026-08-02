//! x86_64 编码测试：encode_golden! 宏断言 + 编码集成测试（从根 tests/encoder_tests.rs 迁移）。

#![cfg(test)]

crate::encode_golden!(
    code_forge::backend::x86_64::Encoder,
    &[
        (code_forge::backend::x86_64::Inst::Ret, &[0xC3u8][..]),
        (code_forge::backend::x86_64::Inst::Nop, &[0x90u8][..]),
        (code_forge::backend::x86_64::Inst::Cqo, &[0x48, 0x99][..]),
    ]
);

// ═══════════════════════════════════════════════════
// 迁移自根 tests/encoder_tests.rs 的编码集成测试
// ═══════════════════════════════════════════════════

use code_forge::AllocResult;
use code_forge::backend::arch::x86_64;
use code_forge::backend::machine::encoder::TargetEncoder;
use code_forge::ir::{PReg, RegClass};
use code_forge::prelude::VReg;

type Inst = x86_64::Inst;

fn encode(inst: &Inst) -> Result<Vec<u8>, code_forge::backend::EncodeError> {
    let encoder = x86_64::Encoder;
    let mut rm = AllocResult::new();
    // Pre-populate AllocResult with identity VReg→PReg mapping for all VRegs used in tests.
    // VReg(N) maps to PReg(N), which gives correct register numbers for encoding.
    // This avoids the "unallocated vreg" error during unit testing.
    for n in 0..32u32 {
        rm.insert(VReg(n), PReg::new(n as u8, RegClass::Int));
    }
    encoder.encode_to_bytes(inst, &rm)
}

fn r(n: u32) -> VReg {
    VReg(n)
}

// ═══════════════════════════════════════════════════
// Single-byte instructions
// ═══════════════════════════════════════════════════

#[test]
fn test_encode_ret() {
    assert_eq!(encode(&Inst::Ret).unwrap(), vec![0xC3]);
}

#[test]
fn test_encode_nop() {
    assert_eq!(encode(&Inst::Nop).unwrap(), vec![0x90]);
}

#[test]
fn test_encode_cqo() {
    assert_eq!(encode(&Inst::Cqo).unwrap(), vec![0x48, 0x99]);
}

// ═══════════════════════════════════════════════════
// PUSH/POP — register encoding with conditional REX
// ═══════════════════════════════════════════════════

#[test]
fn test_encode_push_low_reg() {
    // RBP(5), < 8 → no REX, 0x50 | 5 = 0x55
    assert_eq!(encode(&Inst::PushIreg { reg: r(5) }).unwrap(), vec![0x55]);
}

#[test]
fn test_encode_push_high_reg() {
    // R12(12), >= 8 → REX 0x41, 0x50 | 4 = 0x54
    assert_eq!(
        encode(&Inst::PushIreg { reg: r(12) }).unwrap(),
        vec![0x41, 0x54]
    );
}

#[test]
fn test_encode_pop_low_reg() {
    // RBP(5), < 8 → 0x58 | 5 = 0x5D
    assert_eq!(encode(&Inst::PopIreg { reg: r(5) }).unwrap(), vec![0x5D]);
}

#[test]
fn test_encode_pop_high_reg() {
    // R12(12), >= 8 → REX 0x41, 0x58 | 4 = 0x5C
    assert_eq!(
        encode(&Inst::PopIreg { reg: r(12) }).unwrap(),
        vec![0x41, 0x5C]
    );
}

// ═══════════════════════════════════════════════════
// ModR/M encoding
// ═══════════════════════════════════════════════════

#[test]
fn test_encode_mov_r8_rm_low_regs() {
    // MOV r64, r/m64: REX.W + 8B /r, ModR/M=11 000 001 = 0xC1
    assert_eq!(
        encode(&Inst::MovRRm {
            opsize: 64u8,
            dest: r(0),
            src: r(1)
        })
        .unwrap(),
        vec![0x48, 0x8B, 0xC1]
    );
}

#[test]
fn test_encode_mov_rm_r8_low_regs() {
    // MOV r/m64, r64: REX.W + 89 /r, ModR/M=11 000 001 = 0xC1
    assert_eq!(
        encode(&Inst::MovRmR {
            opsize: 64u8,
            dest: r(1),
            src: r(0)
        })
        .unwrap(),
        vec![0x48, 0x89, 0xC1]
    );
}

#[test]
fn test_encode_mov_r8_rm_high_regs() {
    // MOV_R8_RM dest=R8(8), src=R9(9): both >= 8 → REX 0x4D
    // ModR/M: reg=dest_bit3&7=0, rm=src_bit3&7=1 → 11 000 001 = 0xC1
    assert_eq!(
        encode(&Inst::MovRRm {
            opsize: 64u8,
            dest: r(8),
            src: r(9)
        })
        .unwrap(),
        vec![0x4D, 0x8B, 0xC1]
    );
}

#[test]
fn test_encode_mov_r8_rm_mixed_regs() {
    // MOV_R8_RM dest=R8(8), src=RCX(1): dest>=8, src<8 → REX 0x4C
    // ModR/M: reg=dest&7=0, rm=src&7=1 → 11 000 001 = 0xC1
    assert_eq!(
        encode(&Inst::MovRRm {
            opsize: 64u8,
            dest: r(8),
            src: r(1)
        })
        .unwrap(),
        vec![0x4C, 0x8B, 0xC1]
    );
}

#[test]
fn test_encode_add_rm8_r8_low_regs() {
    // ADD r/m64, r64: REX.W + 01 /r, ModR/M=11 011 010 = 0xDA
    assert_eq!(
        encode(&Inst::AddRmR {
            opsize: 64u8,
            dest: r(2),
            src: r(3)
        })
        .unwrap(),
        vec![0x48, 0x01, 0xDA]
    );
}

#[test]
fn test_encode_sub_rm8_r8_low_regs() {
    // SUB_RM8_R8 dest=RSI(6), src=RDI(7): both < 8 → no REX
    // opcode 0x29, ModR/M: rm=dest&7=6, reg=src&7=7 → 11 111 110 = 0xFE
    assert_eq!(
        encode(&Inst::SubRmR {
            opsize: 64u8,
            dest: r(6),
            src: r(7)
        })
        .unwrap(),
        vec![0x48, 0x29, 0xFE]
    );
}

// ═══════════════════════════════════════════════════
// Register+immediate encoding
// ═══════════════════════════════════════════════════

#[test]
fn test_encode_mov_reg_imm64_rax() {
    // reg=0 (<8): REX 0x48, opcode 0xB8|0=0xB8, 8 bytes imm LE
    assert_eq!(
        encode(&Inst::MovRegImm64 { imm: 42, reg: r(0) }).unwrap(),
        vec![0x48, 0xB8, 42, 0, 0, 0, 0, 0, 0, 0]
    );
}

#[test]
fn test_encode_mov_reg_imm64_r9() {
    // reg=8 (>=8): REX 0x49, opcode 0xB8|0=0xB8, 8 bytes imm LE
    assert_eq!(
        encode(&Inst::MovRegImm64 {
            imm: 0xDEADBEEF,
            reg: r(8)
        })
        .unwrap(),
        vec![0x49, 0xB8, 0xEF, 0xBE, 0xAD, 0xDE, 0, 0, 0, 0]
    );
}

// ═══════════════════════════════════════════════════
// Branch encoding
// ═══════════════════════════════════════════════════

#[test]
fn test_encode_jmp_rel32() {
    let bytes = encode(&Inst::JmpRel32 { rel: 0 }).unwrap();
    assert!(
        !bytes.is_empty() && bytes[0] == 0xE9,
        "JMP_REL32 should start with 0xE9, got {:?}",
        bytes
    );
}

#[test]
fn test_encode_jcc_rel32() {
    let bytes = encode(&Inst::JccRel32 { cond: 0x94, rel: 0 }).unwrap();
    assert_eq!(
        &bytes[..2],
        &[0x0F, 0x94],
        "JCC_REL32 should start with 0F 94, got {:?}",
        bytes
    );
}

// ═══════════════════════════════════════════════════
// NOT_RM encoding
// ═══════════════════════════════════════════════════

#[test]
fn test_encode_not_rm() {
    // NOT_RM dest=RAX(0): $op_rm 2 dest → rm<8 so no REX, opcode 0xF7, modrm 11 010 000=0xD0
    assert_eq!(
        encode(&Inst::NotRm {
            opsize: 64u8,
            dest: r(0)
        })
        .unwrap(),
        vec![0x48, 0xF7, 0xD0]
    );
}

// ═══════════════════════════════════════════════════
// SSE encoding
// ═══════════════════════════════════════════════════

#[test]
fn test_encode_xorpd_xmm() {
    let bytes = encode(&Inst::Xorpd {
        dest: r(0),
        src: r(1),
    })
    .unwrap();
    let expected: &[u8] = &[0x66, 0x0F, 0x57, 0xC1];
    assert_eq!(&bytes[..expected.len()], expected);
}

// ═══════════════════════════════════════════════════
// Encoded size / encode_into
// ═══════════════════════════════════════════════════

#[test]
fn test_encoded_size_ret() {
    let encoder = x86_64::Encoder;
    assert_eq!(encoder.encoded_size(&Inst::Ret).unwrap(), 1);
}

#[test]
fn test_encoded_size_push_r12() {
    let encoder = x86_64::Encoder;
    assert_eq!(
        encoder
            .encoded_size(&Inst::PushIreg { reg: r(12) })
            .unwrap(),
        2
    );
}

#[test]
fn test_encode_into_buffer() {
    let encoder = x86_64::Encoder;
    let mut buf = [0u8; 16];
    let n = encoder
        .encode_into(&Inst::Ret, &mut buf, &AllocResult::new())
        .unwrap();
    assert_eq!(n, 1);
    assert_eq!(buf[0], 0xC3);
}

#[test]
fn test_encode_into_buffer_too_small() {
    let encoder = x86_64::Encoder;
    let mut buf = [0u8; 0];
    let result = encoder.encode_into(&Inst::Ret, &mut buf, &AllocResult::new());
    assert!(result.is_err());
}

// ═══════════════════════════════════════════════════
// Frame allocation encoding (Reg type)
// ═══════════════════════════════════════════════════

// ═══════════════════════════════════════════════════
// Multi-instruction sequences
// ═══════════════════════════════════════════════════

/// Encode a sequence of instructions and concatenate their bytes.
fn encode_seq(insts: &[Inst]) -> Result<Vec<u8>, code_forge::backend::EncodeError> {
    let encoder = x86_64::Encoder;
    // Use identity-mapped AllocResult so VReg→PReg resolution works in tests
    let mut rm = AllocResult::new();
    for n in 0..32u32 {
        rm.insert(VReg(n), PReg::new(n as u8, RegClass::Int));
    }
    let mut buf = Vec::new();
    for inst in insts {
        buf.extend(encoder.encode_to_bytes(inst, &rm)?);
    }
    Ok(buf)
}

#[test]
fn test_encode_sequence_push_mov_ret() {
    // Prologue-like: push rbp; mov rbp, rsp; ...
    let seq = &[
        Inst::PushIreg { reg: r(5) }, // push RBP
        Inst::MovRRm {
            opsize: 64u8,
            dest: r(5),
            src: r(4),
        }, // mov RBP, RSP
    ];
    let bytes = encode_seq(seq).unwrap();
    // push rbp = 0x55; mov rbp, rsp (no REX) = 0x8B 0xEC
    assert_eq!(bytes, vec![0x55, 0x48, 0x8B, 0xEC]);
}

#[test]
fn test_encode_sequence_arithmetic() {
    // add rdx, rbx; sub rsi, rdi; xor rsp, rbp
    let seq = &[
        Inst::AddRmR {
            opsize: 64u8,
            dest: r(2),
            src: r(3),
        },
        Inst::SubRmR {
            opsize: 64u8,
            dest: r(6),
            src: r(7),
        },
        Inst::XorRmR {
            opsize: 64u8,
            dest: r(4),
            src: r(5),
        },
    ];
    let bytes = encode_seq(seq).unwrap();
    assert!(!bytes.is_empty());
    // Verify sequential encoding is deterministic
    let bytes2 = encode_seq(seq).unwrap();
    assert_eq!(bytes, bytes2, "encoding should be deterministic");
}

#[test]
fn test_encode_sequence_epilogue() {
    // Epilogue-like: pop r15; pop r14; ...; pop rbp; ret
    let seq = &[
        Inst::PopIreg { reg: r(15) },
        Inst::PopIreg { reg: r(14) },
        Inst::PopIreg { reg: r(5) }, // rbp
        Inst::Ret,
    ];
    let bytes = encode_seq(seq).unwrap();
    // R15→0x41,0x5F; R14→0x41,0x5E; RBP→0x5D; RET→0xC3
    assert_eq!(bytes, vec![0x41, 0x5F, 0x41, 0x5E, 0x5D, 0xC3]);
}

#[test]
fn test_encode_sequence_branch() {
    // cmp rax, rcx; je .L10
    let seq = &[
        Inst::CmpRmR {
            opsize: 64u8,
            src1: r(0),
            src2: r(1),
        },
        Inst::JccRel32 {
            cond: 0x94,
            rel: 10,
        },
    ];
    let bytes = encode_seq(seq).unwrap();
    // REX.W + cmp rax,rcx = 0x48,0x39,0xC8; jcc = 0x0F,0x94 + 4 bytes
    assert_eq!(bytes[0], 0x48);
    assert_eq!(bytes[1], 0x39);
    assert_eq!(bytes[2], 0xC8);
    assert_eq!(bytes[3], 0x0F);
    assert_eq!(bytes[4], 0x94);
    // Total: 3 (REX+cmp) + 6 (jcc) = 9 bytes
    assert_eq!(bytes.len(), 9);
}

#[test]
fn test_encode_large_sequence_deterministic() {
    // 100 instructions — verify deterministic encoding
    let mut seq = Vec::new();
    for i in 0..50u32 {
        seq.push(Inst::MovRRm {
            opsize: 64u8,
            dest: r(i % 16),
            src: r((i + 1) % 16),
        });
    }
    let bytes1 = encode_seq(&seq).unwrap();
    let bytes2 = encode_seq(&seq).unwrap();
    assert_eq!(
        bytes1, bytes2,
        "large sequence encoding should be deterministic"
    );
    assert!(bytes1.len() > 100, "should produce substantial output");
}

#[test]
fn test_encode_movabs_global() {
    // movabs r0, <global 8-byte placeholder> — 48 B8 + imm64
    assert_eq!(
        encode(&Inst::MovabsGlobal {
            dest: r(0),
            global: 0
        })
        .unwrap(),
        vec![0x48, 0xB8, 0, 0, 0, 0, 0, 0, 0, 0]
    );
}
