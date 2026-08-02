//! WASM32 编码测试：encode_golden! 宏断言 + 编码集成测试（从根 tests/wasm32_encoder_tests.rs 迁移）。

#![cfg(test)]

use code_forge::AllocResult;
use code_forge::backend::arch::wasm32;
use code_forge::backend::machine::encoder::TargetEncoder;
use code_forge::ir::{PReg, RegClass};
use code_forge::prelude::VReg;

type Inst = wasm32::Inst;

fn r(n: u32) -> VReg {
    VReg(n)
}

fn encode(inst: &Inst) -> Result<Vec<u8>, code_forge::backend::EncodeError> {
    let encoder = wasm32::Encoder;
    let mut rm = AllocResult::new();
    for n in 0..64u32 {
        rm.insert(VReg(n), PReg::new((n % 32) as u8, RegClass::Int));
    }
    encoder.encode_to_bytes(inst, &rm)
}

// ═══════════════════════════════════════════════
// Data Movement
// ═══════════════════════════════════════════════

#[test]
fn test_mov_encodes() {
    // SD_MOV dest, src = local.get src; local.set dest (via $wasm_mov)
    let result = encode(&Inst::SdMov {
        dest: r(0),
        src: r(1),
    });
    assert!(result.is_ok(), "SD_MOV should encode: {:?}", result.err());
}

#[test]
fn test_mov_imm_encodes() {
    let result = encode(&Inst::SdMovImm {
        dest: r(0),
        imm: 42,
    });
    assert!(result.is_ok(), "SD_MOV_IMM should encode");
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
// Integer Arithmetic
// ═══════════════════════════════════════════════

#[test]
fn test_add_encodes() {
    let result = encode(&Inst::SdAdd {
        dest: r(0),
        src: r(1),
    });
    assert!(result.is_ok());
}

#[test]
fn test_sub_encodes() {
    let result = encode(&Inst::SdSub {
        dest: r(0),
        src: r(1),
    });
    assert!(result.is_ok());
}

#[test]
fn test_mul_encodes() {
    let result = encode(&Inst::SdMul {
        dest: r(0),
        src: r(1),
    });
    assert!(result.is_ok());
}

#[test]
fn test_udiv_encodes() {
    let result = encode(&Inst::SdUdiv {
        dest: r(0),
        src: r(1),
    });
    assert!(result.is_ok());
}

#[test]
fn test_sdiv_encodes() {
    let result = encode(&Inst::SdSdiv {
        dest: r(0),
        src: r(1),
    });
    assert!(result.is_ok());
}

#[test]
fn test_urem_encodes() {
    let result = encode(&Inst::SdUrem {
        dest: r(0),
        src: r(1),
    });
    assert!(result.is_ok());
}

#[test]
fn test_srem_encodes() {
    let result = encode(&Inst::SdSrem {
        dest: r(0),
        src: r(1),
    });
    assert!(result.is_ok());
}

// ═══════════════════════════════════════════════
// Bitwise
// ═══════════════════════════════════════════════

#[test]
fn test_and_encodes() {
    let result = encode(&Inst::SdAnd {
        dest: r(0),
        src: r(1),
    });
    assert!(result.is_ok());
}

#[test]
fn test_or_encodes() {
    let result = encode(&Inst::SdOr {
        dest: r(0),
        src: r(1),
    });
    assert!(result.is_ok());
}

#[test]
fn test_xor_encodes() {
    let result = encode(&Inst::SdXor {
        dest: r(0),
        src: r(1),
    });
    assert!(result.is_ok());
}

#[test]
fn test_shl_encodes() {
    let result = encode(&Inst::SdShl {
        dest: r(0),
        src: r(1),
    });
    assert!(result.is_ok());
}

#[test]
fn test_shr_encodes() {
    let result = encode(&Inst::SdShr {
        dest: r(0),
        src: r(1),
    });
    assert!(result.is_ok());
}

#[test]
fn test_sar_encodes() {
    let result = encode(&Inst::SdSar {
        dest: r(0),
        src: r(1),
    });
    assert!(result.is_ok());
}

#[test]
fn test_not_encodes() {
    let result = encode(&Inst::SdNot { dest: r(0) });
    assert!(result.is_ok());
}

#[test]
fn test_neg_encodes() {
    let result = encode(&Inst::SdNeg { dest: r(0) });
    assert!(result.is_ok());
}

// ═══════════════════════════════════════════════
// Controls
// ═══════════════════════════════════════════════

#[test]
fn test_ret_encodes() {
    let result = encode(&Inst::SdRet);
    assert!(result.is_ok());
    assert_eq!(result.unwrap(), vec![0x0F]);
}

#[test]
fn test_nop_encodes() {
    let result = encode(&Inst::SdNop);
    assert!(result.is_ok());
    assert_eq!(result.unwrap(), vec![0x01]);
}

#[test]
fn test_ud2_encodes() {
    let result = encode(&Inst::SdUd2);
    assert!(result.is_ok());
    assert_eq!(result.unwrap(), vec![0x00]);
}

// ═══════════════════════════════════════════════
// Size correctness
// ═══════════════════════════════════════════════

#[test]
fn test_encoded_size_matches_actual() {
    let encoder = wasm32::Encoder;
    let insts: Vec<Inst> = vec![
        Inst::SdRet,
        Inst::SdNop,
        Inst::SdUd2,
        Inst::SdAdd {
            dest: r(0),
            src: r(1),
        },
        Inst::SdSub {
            dest: r(0),
            src: r(1),
        },
        Inst::SdAnd {
            dest: r(0),
            src: r(1),
        },
    ];
    for inst in insts {
        let expected = encoder.encoded_size(&inst).unwrap();
        let actual = encode(&inst).unwrap().len();
        assert_eq!(expected, actual, "encoded_size should match actual len");
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
            src: r(1),
        },
        Inst::SdSub {
            dest: r(0),
            src: r(1),
        },
        Inst::SdMul {
            dest: r(0),
            src: r(1),
        },
        Inst::SdUdiv {
            dest: r(0),
            src: r(1),
        },
        Inst::SdSdiv {
            dest: r(0),
            src: r(1),
        },
        Inst::SdUrem {
            dest: r(0),
            src: r(1),
        },
        Inst::SdSrem {
            dest: r(0),
            src: r(1),
        },
        Inst::SdAnd {
            dest: r(0),
            src: r(1),
        },
        Inst::SdOr {
            dest: r(0),
            src: r(1),
        },
        Inst::SdXor {
            dest: r(0),
            src: r(1),
        },
        Inst::SdShl {
            dest: r(0),
            src: r(1),
        },
        Inst::SdShr {
            dest: r(0),
            src: r(1),
        },
        Inst::SdSar {
            dest: r(0),
            src: r(1),
        },
        Inst::SdNot { dest: r(0) },
        Inst::SdNeg { dest: r(0) },
        Inst::SdFadd {
            dest: r(0),
            src: r(1),
        },
        Inst::SdFsub {
            dest: r(0),
            src: r(1),
        },
        Inst::SdFmul {
            dest: r(0),
            src: r(1),
        },
        Inst::SdFdiv {
            dest: r(0),
            src: r(1),
        },
        Inst::SdFsqrt {
            dest: r(0),
            src: r(1),
        },
        Inst::SdFneg {
            dest: r(0),
            src: r(1),
        },
        Inst::SdFabs {
            dest: r(0),
            src: r(1),
        },
        Inst::SdRet,
        Inst::SdNop,
        Inst::SdUd2,
    ];
    for inst in &all {
        let result = encode(inst);
        assert!(
            result.is_ok(),
            "every wasm32 instruction should encode without error"
        );
        assert!(!result.unwrap().is_empty());
    }
}
