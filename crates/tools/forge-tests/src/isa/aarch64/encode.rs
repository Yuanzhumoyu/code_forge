//! AArch64 编码测试：encode_golden! 宏断言 + 编码集成测试（从根 tests/aarch64_encoder_tests.rs 迁移）。

#![cfg(test)]

use code_forge::AllocResult;
use code_forge::backend::arch::aarch64::*;
use code_forge::backend::machine::encoder::TargetEncoder;
use code_forge::ir::{PReg, RegClass};
use code_forge::prelude::VReg;

type Inst = aarch64::Inst;

fn encode(inst: &Inst) -> Result<Vec<u8>, code_forge::backend::EncodeError> {
    let encoder = aarch64::Encoder;
    let mut rm = AllocResult::new();
    for n in 0..64u32 {
        rm.insert(VReg(n), PReg::new((n % 32) as u8, RegClass::Int));
    }
    encoder.encode_to_bytes(inst, &rm)
}

fn r(n: u32) -> VReg {
    VReg(n)
}

use aarch64::Reg;
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
        31 => Reg::SP,
        _ => Reg::X0,
    }
}

// ═══════════════════════════════════════════════
// Data Movement
// ═══════════════════════════════════════════════

#[test]
fn test_mov_encodes() {
    let result = encode(&Inst::SdMov {
        dest: r(0),
        src: r(1),
    });
    assert!(result.is_ok(), "SD_MOV should encode: {:?}", result.err());
    assert!(!result.unwrap().is_empty());
}

#[test]
fn test_mov_imm_encodes() {
    let result = encode(&Inst::SdMovImm {
        dest: r(0),
        imm: 42,
    });
    assert!(result.is_ok(), "SD_MOV_IMM should encode");
    assert!(!result.unwrap().is_empty());
}

#[test]
fn test_mov_imm_negative() {
    let result = encode(&Inst::SdMovImm {
        dest: r(0),
        imm: -1,
    });
    assert!(result.is_ok());
}

// ═══════════════════════════════════════════════
// Integer Single-source — 字节级编码断言
// ═══════════════════════════════════════════════

fn encode_single(inst: &Inst) -> Vec<u8> {
    encode(inst).unwrap_or_else(|e| panic!("encode: {e:?}"))
}

#[test]
fn test_clz_encodes() {
    // CLZ X0, X0 — 0xDAC00800（sf=1, [30:21]=0x2D6, [15:10]=0x2）
    let bytes = encode_single(&Inst::SdClz {
        dest: r(0),
        src: r(0),
    });
    assert_eq!(
        bytes,
        vec![0x00, 0x08, 0xC0, 0xDA],
        "CLZ X0,X0: {:02x?}",
        bytes
    );
}

#[test]
fn test_rbit_encodes() {
    // RBIT X0, X0 — 0xDAC00000（[15:10]=0x0）
    let bytes = encode_single(&Inst::SdRbit {
        dest: r(0),
        src: r(0),
    });
    assert_eq!(
        bytes,
        vec![0x00, 0x00, 0xC0, 0xDA],
        "RBIT X0,X0: {:02x?}",
        bytes
    );
}

#[test]
fn test_rev64_encodes() {
    // REV64 X0, X0 — 0xDAC00C00（[15:10]=0x3）
    let bytes = encode_single(&Inst::SdRev64 {
        dest: r(0),
        src: r(0),
    });
    assert_eq!(
        bytes,
        vec![0x00, 0x0C, 0xC0, 0xDA],
        "REV64 X0,X0: {:02x?}",
        bytes
    );
}

#[test]
fn test_abs_encodes() {
    // ABS X0, X0 — 0xC5800000（sf=1, [30:21]=0x22C, [15:10]=0x0）
    let bytes = encode_single(&Inst::SdAbs {
        dest: r(0),
        src: r(0),
    });
    assert_eq!(
        bytes,
        vec![0x00, 0x00, 0x80, 0xC5],
        "ABS X0,X0: {:02x?}",
        bytes
    );
}

// ═══════════════════════════════════════════════
// Integer Arithmetic (3-register R-type)
// ═══════════════════════════════════════════════

#[test]
fn test_add_encodes() {
    assert!(
        encode(&Inst::SdAdd {
            dest: r(0),
            src1: r(1),
            src2: r(2)
        })
        .is_ok()
    );
}
#[test]
fn test_sub_encodes() {
    assert!(
        encode(&Inst::SdSub {
            dest: r(0),
            src1: r(1),
            src2: r(2)
        })
        .is_ok()
    );
}
#[test]
fn test_mul_encodes() {
    assert!(
        encode(&Inst::SdMul {
            dest: r(0),
            src1: r(1),
            src2: r(2)
        })
        .is_ok()
    );
}
#[test]
fn test_sdiv_encodes() {
    assert!(
        encode(&Inst::SdSdiv {
            dest: r(0),
            src1: r(1),
            src2: r(2)
        })
        .is_ok()
    );
}
#[test]
fn test_udiv_encodes() {
    assert!(
        encode(&Inst::SdUdiv {
            dest: r(0),
            src1: r(1),
            src2: r(2)
        })
        .is_ok()
    );
}

// ═══════════════════════════════════════════════
// Bitwise
// ═══════════════════════════════════════════════

#[test]
fn test_and_encodes() {
    assert!(
        encode(&Inst::SdAnd {
            dest: r(0),
            src1: r(1),
            src2: r(2)
        })
        .is_ok()
    );
}
#[test]
fn test_or_encodes() {
    assert!(
        encode(&Inst::SdOr {
            dest: r(0),
            src1: r(1),
            src2: r(2)
        })
        .is_ok()
    );
}
#[test]
fn test_xor_encodes() {
    assert!(
        encode(&Inst::SdXor {
            dest: r(0),
            src1: r(1),
            src2: r(2)
        })
        .is_ok()
    );
}
#[test]
fn test_mvn_encodes() {
    assert!(
        encode(&Inst::SdMvn {
            dest: r(0),
            src: r(1)
        })
        .is_ok()
    );
}
#[test]
fn test_shl_encodes() {
    assert!(
        encode(&Inst::SdShl {
            dest: r(0),
            src1: r(1),
            src2: r(2)
        })
        .is_ok()
    );
}
#[test]
fn test_shr_encodes() {
    assert!(
        encode(&Inst::SdShr {
            dest: r(0),
            src1: r(1),
            src2: r(2)
        })
        .is_ok()
    );
}
#[test]
fn test_sar_encodes() {
    assert!(
        encode(&Inst::SdSar {
            dest: r(0),
            src1: r(1),
            src2: r(2)
        })
        .is_ok()
    );
}

// ═══════════════════════════════════════════════
// Comparison
// ═══════════════════════════════════════════════

#[test]
fn test_cmp_encodes() {
    assert!(
        encode(&Inst::SdCmp {
            src1: r(0),
            src2: r(1)
        })
        .is_ok()
    );
}
#[test]
fn test_setcc_encodes() {
    assert!(
        encode(&Inst::SdSetcc {
            dest: r(0),
            cond: 0
        })
        .is_ok()
    );
}

// ═══════════════════════════════════════════════
// Control Flow
// ═══════════════════════════════════════════════

#[test]
fn test_ret_encodes() {
    let result = encode(&Inst::SdRet);
    assert!(result.is_ok());
    assert_eq!(result.unwrap(), vec![0xC0, 0x03, 0x5F, 0xD6]);
}

#[test]
fn test_nop_encodes() {
    let result = encode(&Inst::SdNop);
    assert!(result.is_ok());
    assert_eq!(result.unwrap(), vec![0x1F, 0x20, 0x03, 0xD5]);
}

#[test]
fn test_ud2_encodes() {
    let result = encode(&Inst::SdUd2);
    assert!(result.is_ok());
    assert_eq!(result.unwrap(), vec![0x00, 0x00, 0x20, 0xD4]);
}

// ═══════════════════════════════════════════════
// Frame Management (GprReg fields)
// ═══════════════════════════════════════════════

#[test]
fn test_stp_pre_encodes() {
    let result = encode(&Inst::SdStpPre {
        rt1: xr(29),
        rt2: xr(30),
        rn: xr(31),
        imm: -16,
    });
    assert!(result.is_ok());
}

#[test]
fn test_ldp_post_encodes() {
    let result = encode(&Inst::SdLdpPost {
        rt1: xr(29),
        rt2: xr(30),
        rn: xr(31),
        imm: 16,
    });
    assert!(result.is_ok());
}

#[test]
fn test_mov_rr_encodes() {
    let result = encode(&Inst::SdMovRr {
        dest: xr(29),
        src: xr(31),
    });
    assert!(result.is_ok());
}

#[test]
fn test_sub_sp_encodes() {
    let result = encode(&Inst::SdSubSp {
        dest: xr(31),
        src: xr(31),
        imm: 16,
    });
    assert!(result.is_ok());
}

// ═══════════════════════════════════════════════
// Load/Store
// ═══════════════════════════════════════════════

#[test]
fn test_load_encodes() {
    assert!(
        encode(&Inst::SdLoad {
            dest: r(0),
            src: r(1)
        })
        .is_ok()
    );
}
#[test]
fn test_store_encodes() {
    assert!(
        encode(&Inst::SdStore {
            val: r(0),
            addr: r(1)
        })
        .is_ok()
    );
}

// ═══════════════════════════════════════════════
// Size correctness
// ═══════════════════════════════════════════════

#[test]
fn test_encoded_size_always_4() {
    let encoder = aarch64::Encoder;
    let insts: Vec<Inst> = vec![
        Inst::SdRet,
        Inst::SdNop,
        Inst::SdUd2,
        Inst::SdAdd {
            dest: r(0),
            src1: r(1),
            src2: r(2),
        },
        Inst::SdSub {
            dest: r(0),
            src1: r(1),
            src2: r(2),
        },
        Inst::SdAnd {
            dest: r(0),
            src1: r(1),
            src2: r(2),
        },
        Inst::SdMovImm {
            dest: r(0),
            imm: 42,
        },
    ];
    for inst in insts {
        let size = encoder.encoded_size(&inst).unwrap();
        assert_eq!(size, 4, "all AArch64 instructions must be exactly 4 bytes");
    }
}

// ═══════════════════════════════════════════════
// All instructions encode without panic
// ═══════════════════════════════════════════════

#[test]
fn test_all_insts_encode_no_panic() {
    let all: Vec<Inst> = vec![
        Inst::SdMov {
            dest: r(0),
            src: r(1),
        },
        Inst::SdMovImm {
            dest: r(0),
            imm: 42,
        },
        Inst::SdAdd {
            dest: r(0),
            src1: r(1),
            src2: r(2),
        },
        Inst::SdSub {
            dest: r(0),
            src1: r(1),
            src2: r(2),
        },
        Inst::SdMul {
            dest: r(0),
            src1: r(1),
            src2: r(2),
        },
        Inst::SdSdiv {
            dest: r(0),
            src1: r(1),
            src2: r(2),
        },
        Inst::SdUdiv {
            dest: r(0),
            src1: r(1),
            src2: r(2),
        },
        Inst::SdAnd {
            dest: r(0),
            src1: r(1),
            src2: r(2),
        },
        Inst::SdOr {
            dest: r(0),
            src1: r(1),
            src2: r(2),
        },
        Inst::SdXor {
            dest: r(0),
            src1: r(1),
            src2: r(2),
        },
        Inst::SdMvn {
            dest: r(0),
            src: r(1),
        },
        Inst::SdShl {
            dest: r(0),
            src1: r(1),
            src2: r(2),
        },
        Inst::SdShr {
            dest: r(0),
            src1: r(1),
            src2: r(2),
        },
        Inst::SdSar {
            dest: r(0),
            src1: r(1),
            src2: r(2),
        },
        Inst::SdCmp {
            src1: r(0),
            src2: r(1),
        },
        Inst::SdSetcc {
            dest: r(0),
            cond: 0,
        },
        Inst::SdRet,
        Inst::SdNop,
        Inst::SdUd2,
    ];
    for inst in &all {
        let result = encode(inst);
        assert!(
            result.is_ok(),
            "every aarch64 instruction should encode without error"
        );
        assert!(!result.unwrap().is_empty());
    }
}
