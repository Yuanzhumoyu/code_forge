//! RISC-V64 编码测试：encode_golden! 宏断言 + 编码集成测试（从根 tests/riscv64_encoder_tests.rs 迁移）。

#![cfg(test)]

use code_forge::AllocResult;
use code_forge::backend::arch::riscv64;
use code_forge::backend::machine::encoder::TargetEncoder;
use code_forge::ir::{PReg, RegClass};
use code_forge::prelude::VReg;

type Inst = riscv64::Inst;

fn encode(inst: &Inst) -> Result<Vec<u8>, code_forge::backend::EncodeError> {
    let encoder = riscv64::Encoder;
    let mut rm = AllocResult::new();
    for n in 0..64u32 {
        rm.insert(VReg(n), PReg::new((n % 32) as u8, RegClass::Int));
    }
    encoder.encode_to_bytes(inst, &rm)
}

fn r(n: u32) -> VReg {
    VReg(n)
}

use riscv64::Reg;
fn xr(n: u8) -> Reg {
    match n {
        0 => Reg::X0,
        1 => Reg::X1,
        2 => Reg::X2,
        3 => Reg::X3,
        4 => Reg::X4,
        5 => Reg::X5,
        6 => Reg::X6,
        7 => Reg::X7,
        8 => Reg::X8,
        9 => Reg::X9,
        10 => Reg::X10,
        11 => Reg::X11,
        12 => Reg::X12,
        13 => Reg::X13,
        14 => Reg::X14,
        15 => Reg::X15,
        16 => Reg::X16,
        17 => Reg::X17,
        18 => Reg::X18,
        19 => Reg::X19,
        20 => Reg::X20,
        21 => Reg::X21,
        22 => Reg::X22,
        23 => Reg::X23,
        24 => Reg::X24,
        25 => Reg::X25,
        26 => Reg::X26,
        27 => Reg::X27,
        28 => Reg::X28,
        29 => Reg::X29,
        30 => Reg::X30,
        31 => Reg::X31,
        _ => Reg::X0,
    }
}

// ═══════════════════════════════════════════════
// R-type Integer Arithmetic
// ═══════════════════════════════════════════════

#[test]
fn test_add_encodes() {
    let result = encode(&Inst::Add {
        dest: r(0),
        src1: r(1),
        src2: r(2),
    });
    assert!(result.is_ok());
    assert_eq!(result.unwrap().len(), 4);
}

#[test]
fn test_sub_encodes() {
    let result = encode(&Inst::Sub {
        dest: r(0),
        src1: r(1),
        src2: r(2),
    });
    assert!(result.is_ok());
}

#[test]
fn test_mul_encodes() {
    let result = encode(&Inst::Mul {
        dest: r(0),
        src1: r(1),
        src2: r(2),
    });
    assert!(result.is_ok());
}

#[test]
fn test_divu_encodes() {
    let result = encode(&Inst::Divu {
        dest: r(0),
        src1: r(1),
        src2: r(2),
    });
    assert!(result.is_ok());
}

#[test]
fn test_div_encodes() {
    let result = encode(&Inst::Div {
        dest: r(0),
        src1: r(1),
        src2: r(2),
    });
    assert!(result.is_ok());
}

#[test]
fn test_remu_encodes() {
    let result = encode(&Inst::Remu {
        dest: r(0),
        src1: r(1),
        src2: r(2),
    });
    assert!(result.is_ok());
}

#[test]
fn test_rem_encodes() {
    let result = encode(&Inst::Rem {
        dest: r(0),
        src1: r(1),
        src2: r(2),
    });
    assert!(result.is_ok());
}

// ═══════════════════════════════════════════════
// R-type Bitwise
// ═══════════════════════════════════════════════

#[test]
fn test_and_encodes() {
    let result = encode(&Inst::And {
        dest: r(0),
        src1: r(1),
        src2: r(2),
    });
    assert!(result.is_ok());
}

#[test]
fn test_or_encodes() {
    let result = encode(&Inst::Or {
        dest: r(0),
        src1: r(1),
        src2: r(2),
    });
    assert!(result.is_ok());
}

#[test]
fn test_xor_encodes() {
    let result = encode(&Inst::Xor {
        dest: r(0),
        src1: r(1),
        src2: r(2),
    });
    assert!(result.is_ok());
}

#[test]
fn test_sll_encodes() {
    let result = encode(&Inst::Sll {
        dest: r(0),
        src1: r(1),
        src2: r(2),
    });
    assert!(result.is_ok());
}

#[test]
fn test_srl_encodes() {
    let result = encode(&Inst::Srl {
        dest: r(0),
        src1: r(1),
        src2: r(2),
    });
    assert!(result.is_ok());
}

#[test]
fn test_sra_encodes() {
    let result = encode(&Inst::Sra {
        dest: r(0),
        src1: r(1),
        src2: r(2),
    });
    assert!(result.is_ok());
}

#[test]
fn test_slt_encodes() {
    let result = encode(&Inst::Slt {
        dest: r(0),
        src1: r(1),
        src2: r(2),
    });
    assert!(result.is_ok());
}

#[test]
fn test_sltu_encodes() {
    let result = encode(&Inst::Sltu {
        dest: r(0),
        src1: r(1),
        src2: r(2),
    });
    assert!(result.is_ok());
}

// ═══════════════════════════════════════════════
// I-type Immediate
// ═══════════════════════════════════════════════

#[test]
fn test_addi_encodes() {
    let result = encode(&Inst::Addi {
        dest: r(0),
        src1: r(1),
        imm: 42,
    });
    assert!(result.is_ok());
}

#[test]
fn test_andi_encodes() {
    let result = encode(&Inst::Andi {
        dest: r(0),
        src1: r(1),
        imm: 0xFF,
    });
    assert!(result.is_ok());
}

#[test]
fn test_ori_encodes() {
    let result = encode(&Inst::Ori {
        dest: r(0),
        src1: r(1),
        imm: 0x100,
    });
    assert!(result.is_ok());
}

#[test]
fn test_xori_encodes() {
    let result = encode(&Inst::Xori {
        dest: r(0),
        src1: r(1),
        imm: -1,
    });
    assert!(result.is_ok());
}

#[test]
fn test_slti_encodes() {
    let result = encode(&Inst::Slti {
        dest: r(0),
        src1: r(1),
        imm: 10,
    });
    assert!(result.is_ok());
}

#[test]
fn test_sltiu_encodes() {
    let result = encode(&Inst::Sltiu {
        dest: r(0),
        src1: r(1),
        imm: 10,
    });
    assert!(result.is_ok());
}

// ═══════════════════════════════════════════════
// Shift-I-type
// ═══════════════════════════════════════════════

#[test]
fn test_slli_encodes() {
    let result = encode(&Inst::Slli {
        dest: r(0),
        src1: r(1),
        shamt: 3,
    });
    assert!(result.is_ok());
}

#[test]
fn test_srli_encodes() {
    let result = encode(&Inst::Srli {
        dest: r(0),
        src1: r(1),
        shamt: 2,
    });
    assert!(result.is_ok());
}

#[test]
fn test_srai_encodes() {
    let result = encode(&Inst::Srai {
        dest: r(0),
        src1: r(1),
        shamt: 4,
    });
    assert!(result.is_ok());
}

// ═══════════════════════════════════════════════
// U-type
// ═══════════════════════════════════════════════

#[test]
fn test_lui_encodes() {
    let result = encode(&Inst::Lui {
        dest: r(0),
        imm: 0x10000,
    });
    assert!(result.is_ok());
}

// ═══════════════════════════════════════════════
// Load/Store
// ═══════════════════════════════════════════════

#[test]
fn test_ld_encodes() {
    let result = encode(&Inst::Ld {
        dest: r(0),
        base: r(1),
        offset: 0,
    });
    assert!(result.is_ok());
}

#[test]
fn test_sd_encodes() {
    let result = encode(&Inst::Sd {
        src2: r(0),
        base: r(1),
        offset: 8,
    });
    assert!(result.is_ok());
}

// ═══════════════════════════════════════════════
// System
// ═══════════════════════════════════════════════

#[test]
fn test_nop_encodes() {
    let result = encode(&Inst::Nop);
    assert!(result.is_ok());
    // NOP = ADDI X0, X0, 0 = 0x00000013
    assert_eq!(result.unwrap(), vec![0x13, 0x00, 0x00, 0x00]);
}

#[test]
fn test_ecall_encodes() {
    let result = encode(&Inst::Ecall);
    assert!(result.is_ok());
    // ECALL = 0x00000073
    assert_eq!(result.unwrap(), vec![0x73, 0x00, 0x00, 0x00]);
}

// ═══════════════════════════════════════════════
// Frame Management (GprReg fields)
// ═══════════════════════════════════════════════

#[test]
fn test_sd_r_encodes() {
    let result = encode(&Inst::SdR {
        src: xr(1),
        base: xr(2),
        offset: 8,
    });
    assert!(result.is_ok());
}

#[test]
fn test_ld_r_encodes() {
    let result = encode(&Inst::LdR {
        dest: xr(1),
        base: xr(2),
        offset: 8,
    });
    assert!(result.is_ok());
}

#[test]
fn test_addi_r_encodes() {
    let result = encode(&Inst::AddiR {
        dest: xr(2),
        src: xr(2),
        imm: -16,
    });
    assert!(result.is_ok());
}

#[test]
fn test_jalr_r_encodes() {
    let result = encode(&Inst::JalrR {
        dest: xr(0),
        base: xr(1),
        offset: 0,
    });
    assert!(result.is_ok());
}

// ═══════════════════════════════════════════════
// Size correctness
// ═══════════════════════════════════════════════

#[test]
fn test_encoded_size_always_4() {
    let insts: Vec<Inst> = vec![
        Inst::Add {
            dest: r(0),
            src1: r(1),
            src2: r(2),
        },
        Inst::Sub {
            dest: r(0),
            src1: r(1),
            src2: r(2),
        },
        Inst::And {
            dest: r(0),
            src1: r(1),
            src2: r(2),
        },
        Inst::Addi {
            dest: r(0),
            src1: r(1),
            imm: 0,
        },
        Inst::Nop,
        Inst::Ecall,
        Inst::Lui {
            dest: r(0),
            imm: 0x1000,
        },
    ];
    for inst in insts {
        let bytes = encode(&inst).unwrap();
        assert_eq!(
            bytes.len(),
            4,
            "all RISC-V instructions must be exactly 4 bytes"
        );
    }
}

// ═══════════════════════════════════════════════
// All instructions encode without panic
// ═══════════════════════════════════════════════

#[test]
fn test_all_insts_encode_no_panic() {
    let all: Vec<Inst> = vec![
        Inst::Add {
            dest: r(0),
            src1: r(1),
            src2: r(2),
        },
        Inst::Sub {
            dest: r(0),
            src1: r(1),
            src2: r(2),
        },
        Inst::Mul {
            dest: r(0),
            src1: r(1),
            src2: r(2),
        },
        Inst::Divu {
            dest: r(0),
            src1: r(1),
            src2: r(2),
        },
        Inst::Div {
            dest: r(0),
            src1: r(1),
            src2: r(2),
        },
        Inst::Remu {
            dest: r(0),
            src1: r(1),
            src2: r(2),
        },
        Inst::Rem {
            dest: r(0),
            src1: r(1),
            src2: r(2),
        },
        Inst::And {
            dest: r(0),
            src1: r(1),
            src2: r(2),
        },
        Inst::Or {
            dest: r(0),
            src1: r(1),
            src2: r(2),
        },
        Inst::Xor {
            dest: r(0),
            src1: r(1),
            src2: r(2),
        },
        Inst::Sll {
            dest: r(0),
            src1: r(1),
            src2: r(2),
        },
        Inst::Srl {
            dest: r(0),
            src1: r(1),
            src2: r(2),
        },
        Inst::Sra {
            dest: r(0),
            src1: r(1),
            src2: r(2),
        },
        Inst::Slt {
            dest: r(0),
            src1: r(1),
            src2: r(2),
        },
        Inst::Sltu {
            dest: r(0),
            src1: r(1),
            src2: r(2),
        },
        Inst::Addi {
            dest: r(0),
            src1: r(1),
            imm: 0,
        },
        Inst::Andi {
            dest: r(0),
            src1: r(1),
            imm: 0,
        },
        Inst::Ori {
            dest: r(0),
            src1: r(1),
            imm: 0,
        },
        Inst::Xori {
            dest: r(0),
            src1: r(1),
            imm: 0,
        },
        Inst::Slti {
            dest: r(0),
            src1: r(1),
            imm: 0,
        },
        Inst::Sltiu {
            dest: r(0),
            src1: r(1),
            imm: 0,
        },
        Inst::Slli {
            dest: r(0),
            src1: r(1),
            shamt: 0,
        },
        Inst::Srli {
            dest: r(0),
            src1: r(1),
            shamt: 0,
        },
        Inst::Srai {
            dest: r(0),
            src1: r(1),
            shamt: 0,
        },
        Inst::Lui { dest: r(0), imm: 0 },
        Inst::Ld {
            dest: r(0),
            base: r(1),
            offset: 0,
        },
        Inst::Sd {
            src2: r(0),
            base: r(1),
            offset: 0,
        },
        Inst::Nop,
        Inst::Ecall,
    ];
    for inst in &all {
        let result = encode(inst);
        assert!(
            result.is_ok(),
            "every riscv64 instruction should encode without error"
        );
        assert!(!result.unwrap().is_empty());
    }
}

// ═══════════════════════════════════════════════
// Zbb 位操作 — 字节级编码断言
// ═══════════════════════════════════════════════

#[test]
fn test_zbb_encodes() {
    // CLZ X0, X0 — 0x60001013（funct7=0x30, funct3=1, opcode 0x13）
    let bytes = encode(&Inst::Clz {
        dest: r(0),
        src: r(0),
    })
    .unwrap();
    assert_eq!(bytes, vec![0x13, 0x10, 0x00, 0x60], "CLZ: {:02x?}", bytes);
    // CTZ — 0x60101013
    let bytes = encode(&Inst::Ctz {
        dest: r(0),
        src: r(0),
    })
    .unwrap();
    assert_eq!(bytes, vec![0x13, 0x10, 0x10, 0x60], "CTZ: {:02x?}", bytes);
    // CPOP — 0x60201013
    let bytes = encode(&Inst::Cpop {
        dest: r(0),
        src: r(0),
    })
    .unwrap();
    assert_eq!(bytes, vec![0x13, 0x10, 0x20, 0x60], "CPOP: {:02x?}", bytes);
    // REV8 — 0x69801013
    let bytes = encode(&Inst::Rev8 {
        dest: r(0),
        src: r(0),
    })
    .unwrap();
    assert_eq!(bytes, vec![0x13, 0x10, 0x80, 0x68], "REV8: {:02x?}", bytes);
}

#[test]
fn test_min_max_encodes() {
    // MIN X0, X0, X0 — 0x00A30333
    let bytes = encode(&Inst::Min {
        dest: r(0),
        src1: r(0),
        src2: r(0),
    })
    .unwrap();
    assert_eq!(bytes, vec![0x33, 0x00, 0x00, 0x28], "MIN: {:02x?}", bytes);
    // MAX — 0x00B30333
    let bytes = encode(&Inst::Max {
        dest: r(0),
        src1: r(0),
        src2: r(0),
    })
    .unwrap();
    assert_eq!(bytes, vec![0x33, 0x00, 0x00, 0x2A], "MAX: {:02x?}", bytes);
}
