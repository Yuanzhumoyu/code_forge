//! Standard instruction set and default lowering rules.
#![allow(dead_code)] // Utility functions available for external use
//!
//! Defines a minimal complete set of architecture-neutral machine instructions
//! (prefix `SD_`) that are sufficient to lower ALL IR opcodes.
//!
//! If a user's ISA TOML does not provide a `[lower.<Opcode>]` rule, the codegen
//! falls back to lowering rules expressed in terms of these standard instructions.
//! The user only needs to provide `[inst.SD_*]` emit templates; lowering is automatic.
//!
//! ## CondCode convention
//!
//! `SD_SETCC` and `SD_JCC` use ordinal values following the `IntCC` / `FloatCC` enum:
//!
//! **Integer conditions:** 0=Equal, 1=NotEqual, 2=SignedLessThan, 3=SignedLessThanOrEqual,
//!   4=SignedGreaterThan, 5=SignedGreaterThanOrEqual, 6=UnsignedLessThan,
//!   7=UnsignedLessThanOrEqual, 8=UnsignedGreaterThan, 9=UnsignedGreaterThanOrEqual
//!
//! **Float conditions:** 0=Equal, 1=NotEqual, 2=LessThan, 3=LessThanOrEqual,
//!   4=GreaterThan, 5=GreaterThanOrEqual, 6=Unordered, 7=Ordered
//!
//! ## Reserved scratch registers
//!
//! Default lowering rules use VReg(N) indices because they must be ISA-agnostic.
//! VReg(96)—VReg(100) are reserved for default lowering temporaries.
//! In ISA-specific lowering rules (e.g. x86_64_v10.toml), use physical register
//! names directly (RAX, R10, XMM0) per the unified register model.
//!
//! These are mapped via `[abi.precolor]`:
//! - VReg(96) → GPR scratch (R10 on x86)
//! - VReg(97) → GPR scratch (R11 on x86)
//! - VReg(99) → SP proxy for StackAddr
//! - VReg(100) → FPR scratch (XMM0 on x86)
//!
//! Symbolic names are centralized here; all lowering rules reference the constants
//! rather than hardcoded string literals.

use crate::model::*;
use std::collections::HashSet;

/// Symbolic scratch register names for default lowering temporaries.
/// These are resolved to concrete VReg indices via `[abi.precolor]` in each ISA TOML.
pub const SCRATCH_GPR: &str = "VReg(96)";
pub const SCRATCH_GPR2: &str = "VReg(97)";
pub const STACKADDR_VREG: &str = "VReg(99)";
pub const SCRATCH_FPR: &str = "VReg(100)";
pub const RETVAL_VREG: &str = "VReg(0)";
pub const RETVAL2_VREG: &str = "VReg(2)";

// ============================================================
// Standard instruction definitions (field signatures)
// ============================================================

/// A standard instruction with its name and field types.
pub struct StdInstDef {
    pub name: &'static str,
    pub fields: &'static [(&'static str, FieldType)],
}

/// Returns all standard instruction definitions.
/// Users MUST provide `[inst.<name>]` entries with emit templates for
/// any standard instruction referenced by default lowering rules.
pub fn standard_instructions() -> Vec<StdInstDef> {
    vec![
        // === Integer data movement ===
        StdInstDef {
            name: "SD_MOV",
            fields: &[("dest", FieldType::Ireg), ("src", FieldType::Ireg)],
        },
        StdInstDef {
            name: "SD_MOV_IMM",
            fields: &[("dest", FieldType::Ireg), ("imm", FieldType::I64)],
        },
        // === Integer arithmetic ===
        StdInstDef {
            name: "SD_ADD",
            fields: &[("dest", FieldType::Ireg), ("src", FieldType::Ireg)],
        },
        StdInstDef {
            name: "SD_SUB",
            fields: &[("dest", FieldType::Ireg), ("src", FieldType::Ireg)],
        },
        StdInstDef {
            name: "SD_MUL",
            fields: &[("dest", FieldType::Ireg), ("src", FieldType::Ireg)],
        },
        StdInstDef {
            name: "SD_UDIV",
            fields: &[("dest", FieldType::Ireg), ("src", FieldType::Ireg)],
        },
        StdInstDef {
            name: "SD_SDIV",
            fields: &[("dest", FieldType::Ireg), ("src", FieldType::Ireg)],
        },
        StdInstDef {
            name: "SD_UREM",
            fields: &[("dest", FieldType::Ireg), ("src", FieldType::Ireg)],
        },
        StdInstDef {
            name: "SD_SREM",
            fields: &[("dest", FieldType::Ireg), ("src", FieldType::Ireg)],
        },
        // === Bitwise and shifts ===
        StdInstDef {
            name: "SD_AND",
            fields: &[("dest", FieldType::Ireg), ("src", FieldType::Ireg)],
        },
        StdInstDef {
            name: "SD_OR",
            fields: &[("dest", FieldType::Ireg), ("src", FieldType::Ireg)],
        },
        StdInstDef {
            name: "SD_XOR",
            fields: &[("dest", FieldType::Ireg), ("src", FieldType::Ireg)],
        },
        StdInstDef {
            name: "SD_NOT",
            fields: &[("dest", FieldType::Ireg)],
        },
        StdInstDef {
            name: "SD_NEG",
            fields: &[("dest", FieldType::Ireg)],
        },
        StdInstDef {
            name: "SD_SHL",
            fields: &[("dest", FieldType::Ireg), ("src", FieldType::Ireg)],
        },
        StdInstDef {
            name: "SD_SHR",
            fields: &[("dest", FieldType::Ireg), ("src", FieldType::Ireg)],
        },
        StdInstDef {
            name: "SD_SAR",
            fields: &[("dest", FieldType::Ireg), ("src", FieldType::Ireg)],
        },
        // === Comparison ===
        StdInstDef {
            name: "SD_CMP",
            fields: &[("src1", FieldType::Ireg), ("src2", FieldType::Ireg)],
        },
        StdInstDef {
            name: "SD_SETCC",
            fields: &[("dest", FieldType::Ireg), ("cond", FieldType::CondCode)],
        },
        // === Float data movement ===
        StdInstDef {
            name: "SD_FMOV",
            fields: &[("dest", FieldType::Freg), ("src", FieldType::Freg)],
        },
        StdInstDef {
            name: "SD_FMOV_BITS",
            fields: &[("dest", FieldType::Freg), ("src", FieldType::Freg)],
        },
        // === Float arithmetic ===
        StdInstDef {
            name: "SD_FADD",
            fields: &[("dest", FieldType::Freg), ("src", FieldType::Freg)],
        },
        StdInstDef {
            name: "SD_FSUB",
            fields: &[("dest", FieldType::Freg), ("src", FieldType::Freg)],
        },
        StdInstDef {
            name: "SD_FMUL",
            fields: &[("dest", FieldType::Freg), ("src", FieldType::Freg)],
        },
        StdInstDef {
            name: "SD_FDIV",
            fields: &[("dest", FieldType::Freg), ("src", FieldType::Freg)],
        },
        StdInstDef {
            name: "SD_FSQRT",
            fields: &[("dest", FieldType::Freg), ("src", FieldType::Freg)],
        },
        StdInstDef {
            name: "SD_FNEG",
            fields: &[("dest", FieldType::Freg)],
        },
        StdInstDef {
            name: "SD_FABS",
            fields: &[("dest", FieldType::Freg)],
        },
        StdInstDef {
            name: "SD_FCMP",
            fields: &[("src1", FieldType::Freg), ("src2", FieldType::Freg)],
        },
        StdInstDef {
            name: "SD_I2F",
            fields: &[("dest", FieldType::Freg), ("src", FieldType::Ireg)],
        },
        StdInstDef {
            name: "SD_F2I",
            fields: &[("dest", FieldType::Ireg), ("src", FieldType::Freg)],
        },
        // === Integer sign extension ===
        StdInstDef {
            name: "SD_SEXT",
            fields: &[("dest", FieldType::Ireg), ("src", FieldType::Ireg)],
        },
        // === Memory ===
        StdInstDef {
            name: "SD_LOAD",
            fields: &[("dest", FieldType::Ireg), ("src", FieldType::Ireg)],
        },
        StdInstDef {
            name: "SD_STORE",
            fields: &[("addr", FieldType::Ireg), ("val", FieldType::Ireg)],
        },
        // === Control flow ===
        StdInstDef {
            name: "SD_JMP",
            fields: &[("rel", FieldType::BlockTarget)],
        },
        StdInstDef {
            name: "SD_JCC",
            fields: &[
                ("cond", FieldType::CondCode),
                ("rel", FieldType::BlockTarget),
            ],
        },
        StdInstDef {
            name: "SD_CALL",
            fields: &[("target", FieldType::Ireg)],
        },
        StdInstDef {
            name: "SD_RET",
            fields: &[],
        },
        StdInstDef {
            name: "SD_NOP",
            fields: &[],
        },
        StdInstDef {
            name: "SD_UD2",
            fields: &[],
        },
        // === Stack ===
        StdInstDef {
            name: "SD_PUSH",
            fields: &[("reg", FieldType::Ireg)],
        },
        StdInstDef {
            name: "SD_POP",
            fields: &[("reg", FieldType::Ireg)],
        },
        StdInstDef {
            name: "SD_STACK_ADDR",
            fields: &[("dest", FieldType::Ireg), ("offset", FieldType::I64)],
        },
    ]
}

// ============================================================
// Helper: build an asm string "INST op1, op2, ..."
// ============================================================

fn li(inst: &str, args: &[(&str, &str)]) -> String {
    if args.is_empty() {
        inst.to_string()
    } else {
        // Sort by field name to match BTreeMap order in instruction definitions
        let mut sorted: Vec<_> = args.iter().collect();
        sorted.sort_by_key(|(k, _)| *k);
        let ops: Vec<&str> = sorted.iter().map(|(_, v)| *v).collect();
        format!("{} {}", inst, ops.join(", "))
    }
}

// ============================================================
// Condition code byte maps
// ============================================================

/// Maps IntCC variant name to standard condition code byte.
pub fn icmp_cond_byte(cond_name: &str) -> Option<u8> {
    match cond_name {
        "Equal" => Some(0),
        "NotEqual" => Some(1),
        "SignedLessThan" => Some(2),
        "SignedLessThanOrEqual" => Some(3),
        "SignedGreaterThan" => Some(4),
        "SignedGreaterThanOrEqual" => Some(5),
        "UnsignedLessThan" => Some(6),
        "UnsignedLessThanOrEqual" => Some(7),
        "UnsignedGreaterThan" => Some(8),
        "UnsignedGreaterThanOrEqual" => Some(9),
        _ => None,
    }
}

/// Maps FloatCC variant name to standard condition code byte.
/// 序号即 SD_SETCC 的 cond 立即数（minimal_sd 等标准指令集的约定）。
/// 0-7 为有序/无序基础条件；8-15 为 LLVM 扩展条件（false/true/u* 系列）。
pub fn fcmp_cond_byte(cond_name: &str) -> Option<u8> {
    match cond_name {
        "Equal" => Some(0),
        "NotEqual" => Some(1),
        "LessThan" => Some(2),
        "LessThanOrEqual" => Some(3),
        "GreaterThan" => Some(4),
        "GreaterThanOrEqual" => Some(5),
        "Unordered" => Some(6),
        "Ordered" => Some(7),
        "False" => Some(8),
        "True" => Some(9),
        "Ueq" => Some(10),
        "Ugt" => Some(11),
        "Uge" => Some(12),
        "Ult" => Some(13),
        "Ule" => Some(14),
        "Une" => Some(15),
        _ => None,
    }
}

/// All IntCC condition names in enum order.
pub const ICMP_CONDS: &[&str] = &[
    "Equal",
    "NotEqual",
    "SignedLessThan",
    "SignedLessThanOrEqual",
    "SignedGreaterThan",
    "SignedGreaterThanOrEqual",
    "UnsignedLessThan",
    "UnsignedLessThanOrEqual",
    "UnsignedGreaterThan",
    "UnsignedGreaterThanOrEqual",
];

/// All FloatCC condition names in enum order (LLVM 16 条件全量)。
pub const FCMP_CONDS: &[&str] = &[
    "Ordered",
    "Unordered",
    "Equal",
    "NotEqual",
    "LessThan",
    "LessThanOrEqual",
    "GreaterThan",
    "GreaterThanOrEqual",
    "False",
    "True",
    "Ueq",
    "Ugt",
    "Uge",
    "Ult",
    "Ule",
    "Une",
];

// ============================================================
// Default lowering for regular opcodes
// ============================================================

/// Returns the default lowering rule for an opcode (e.g., "Iadd" → [SD_MOV, SD_ADD]).
/// Returns `None` for opcodes that have no generic lowering (Tier 2: trap).
pub fn default_opcode_lowering(opcode_name: &str) -> Option<Vec<String>> {
    match opcode_name {
        // === Integer arithmetic ===
        "Iadd" => Some(vec![
            li("SD_MOV", &[("dest", "rd"), ("src", "rs1")]),
            li("SD_ADD", &[("dest", "rd"), ("src", "rs2")]),
        ]),
        "Isub" => Some(vec![
            li("SD_MOV", &[("dest", "rd"), ("src", "rs1")]),
            li("SD_SUB", &[("dest", "rd"), ("src", "rs2")]),
        ]),
        "Imul" => Some(vec![
            li("SD_MOV", &[("dest", "rd"), ("src", "rs1")]),
            li("SD_MUL", &[("dest", "rd"), ("src", "rs2")]),
        ]),
        "Udiv" => Some(vec![
            li("SD_MOV", &[("dest", "rd"), ("src", "rs1")]),
            li("SD_UDIV", &[("dest", "rd"), ("src", "rs2")]),
        ]),
        "Sdiv" => Some(vec![
            li("SD_MOV", &[("dest", "rd"), ("src", "rs1")]),
            li("SD_SDIV", &[("dest", "rd"), ("src", "rs2")]),
        ]),
        "Urem" => Some(vec![
            // remainder = dividend - (dividend / divisor) * divisor
            // VReg(96): scratch; result in rd
            li("SD_MOV", &[("dest", SCRATCH_GPR), ("src", "rs1")]),
            li("SD_UDIV", &[("dest", SCRATCH_GPR), ("src", "rs2")]),
            li("SD_MUL", &[("dest", SCRATCH_GPR), ("src", "rs2")]),
            li("SD_MOV", &[("dest", "rd"), ("src", "rs1")]),
            li("SD_SUB", &[("dest", "rd"), ("src", SCRATCH_GPR)]),
        ]),
        "Srem" => Some(vec![
            // remainder = dividend - (dividend / divisor) * divisor
            // VReg(96): scratch; result in rd
            li("SD_MOV", &[("dest", SCRATCH_GPR), ("src", "rs1")]),
            li("SD_SDIV", &[("dest", SCRATCH_GPR), ("src", "rs2")]),
            li("SD_MUL", &[("dest", SCRATCH_GPR), ("src", "rs2")]),
            li("SD_MOV", &[("dest", "rd"), ("src", "rs1")]),
            li("SD_SUB", &[("dest", "rd"), ("src", SCRATCH_GPR)]),
        ]),

        // === Bitwise ===
        "Band" => Some(vec![
            li("SD_MOV", &[("dest", "rd"), ("src", "rs1")]),
            li("SD_AND", &[("dest", "rd"), ("src", "rs2")]),
        ]),
        "Bor" => Some(vec![
            li("SD_MOV", &[("dest", "rd"), ("src", "rs1")]),
            li("SD_OR", &[("dest", "rd"), ("src", "rs2")]),
        ]),
        "Bxor" => Some(vec![
            li("SD_MOV", &[("dest", "rd"), ("src", "rs1")]),
            li("SD_XOR", &[("dest", "rd"), ("src", "rs2")]),
        ]),
        "Bnot" => Some(vec![
            li("SD_MOV", &[("dest", "rd"), ("src", "rs1")]),
            li("SD_NOT", &[("dest", "rd")]),
        ]),
        "Ishl" => Some(vec![
            li("SD_MOV", &[("dest", "rd"), ("src", "rs1")]),
            li("SD_SHL", &[("dest", "rd"), ("src", "rs2")]),
        ]),
        "Ushr" => Some(vec![
            li("SD_MOV", &[("dest", "rd"), ("src", "rs1")]),
            li("SD_SHR", &[("dest", "rd"), ("src", "rs2")]),
        ]),
        "Sshr" => Some(vec![
            li("SD_MOV", &[("dest", "rd"), ("src", "rs1")]),
            li("SD_SAR", &[("dest", "rd"), ("src", "rs2")]),
        ]),

        // === Float arithmetic ===
        "Fadd" => Some(vec![
            li("SD_FMOV", &[("dest", "rd"), ("src", "rs1")]),
            li("SD_FADD", &[("dest", "rd"), ("src", "rs2")]),
        ]),
        "Fsub" => Some(vec![
            li("SD_FMOV", &[("dest", "rd"), ("src", "rs1")]),
            li("SD_FSUB", &[("dest", "rd"), ("src", "rs2")]),
        ]),
        "Fmul" => Some(vec![
            li("SD_FMOV", &[("dest", "rd"), ("src", "rs1")]),
            li("SD_FMUL", &[("dest", "rd"), ("src", "rs2")]),
        ]),
        "Fdiv" => Some(vec![
            li("SD_FMOV", &[("dest", "rd"), ("src", "rs1")]),
            li("SD_FDIV", &[("dest", "rd"), ("src", "rs2")]),
        ]),
        "Fneg" => Some(vec![
            li("SD_FMOV", &[("dest", "rd"), ("src", "rs1")]),
            li("SD_FNEG", &[("dest", "rd")]),
        ]),
        "Fabs" => Some(vec![
            li("SD_FMOV", &[("dest", "rd"), ("src", "rs1")]),
            li("SD_FABS", &[("dest", "rd")]),
        ]),
        "Fsqrt" => Some(vec![
            li("SD_FMOV", &[("dest", "rd"), ("src", "rs1")]),
            li("SD_FSQRT", &[("dest", "rd"), ("src", "rs1")]),
        ]),

        // === Constants ===
        "Iconst" => Some(vec![li("SD_MOV_IMM", &[("dest", "rd"), ("imm", "iconst")])]),
        "Fconst" => Some(vec![
            li("SD_MOV_IMM", &[("dest", SCRATCH_GPR), ("imm", "fconst")]),
            li("SD_FMOV_BITS", &[("dest", "rd"), ("src", SCRATCH_GPR)]),
        ]),

        // === Type conversion ===
        "Sextend" => Some(vec![li("SD_SEXT", &[("dest", "rd"), ("src", "rs1")])]),
        "Uextend" => Some(vec![li("SD_MOV", &[("dest", "rd"), ("src", "rs1")])]),
        "Ireduce" => Some(vec![li("SD_MOV", &[("dest", "rd"), ("src", "rs1")])]),
        "Bitcast" => Some(vec![li("SD_MOV", &[("dest", "rd"), ("src", "rs1")])]),

        // === Memory ===
        "Load" => Some(vec![li("SD_LOAD", &[("dest", "rd"), ("src", "rs1")])]),
        "Store" => Some(vec![li("SD_STORE", &[("addr", "rs2"), ("val", "rs1")])]),
        // === Other ===
        "Copy" => Some(vec![li("SD_MOV", &[("dest", "rd"), ("src", "rs1")])]),
        "Nop" => Some(vec![li("SD_NOP", &[])]),
        "Select" => Some(vec![
            // Simplified: just copy the false value (user should customize)
            li("SD_MOV", &[("dest", "rd"), ("src", "rs2")]),
        ]),
        "StackAddr" => Some(vec![li(
            "SD_MOV",
            &[("dest", "rd"), ("src", STACKADDR_VREG)],
        )]),
        "Call" => Some(vec![
            // Call is complex (needs FuncRef); emit trap to surface missing lowering
            li("SD_UD2", &[]),
        ]),
        "CallIndirect" => Some(vec![li("SD_CALL", &[("target", "rs1")])]),
        "Alloca" => Some(vec![
            // Alloca is normally handled by frontend; placeholder
            li("SD_MOV", &[("dest", "rd"), ("src", STACKADDR_VREG)]),
        ]),
        "GlobalAddr" => Some(vec![li("SD_MOV_IMM", &[("dest", "rd"), ("imm", "0")])]),
        "GetElementPtr" => Some(vec![
            li("SD_MOV", &[("dest", "rd"), ("src", "rs1")]),
            li("SD_ADD", &[("dest", "rd"), ("src", "rs2")]),
        ]),

        // === SIMD (标量 fallback) ===
        // SIMD 操作的默认 lowering 降级为标量操作。
        // 用户可通过自定义 [lower.V*] 规则实现真正的 SIMD lowering。
        "Vadd" => Some(vec![
            li("SD_MOV", &[("dest", "rd"), ("src", "rs1")]),
            li("SD_ADD", &[("dest", "rd"), ("src", "rs2")]),
        ]),
        "Vsub" => Some(vec![
            li("SD_MOV", &[("dest", "rd"), ("src", "rs1")]),
            li("SD_SUB", &[("dest", "rd"), ("src", "rs2")]),
        ]),
        "Vmul" => Some(vec![
            li("SD_MOV", &[("dest", "rd"), ("src", "rs1")]),
            li("SD_MUL", &[("dest", "rd"), ("src", "rs2")]),
        ]),
        "Vextract" | "Vinsert" => Some(vec![li("SD_MOV", &[("dest", "rd"), ("src", "rs1")])]),

        // Phi is handled by lowering.rs (skipped, no instructions emitted)
        "Phi" => Some(vec![li("SD_NOP", &[])]),

        _ => None,
    }
}

/// Returns the default Icmp lowering for a specific condition.
pub fn default_icmp_lowering(cond_name: &str) -> Option<Vec<String>> {
    let cc = icmp_cond_byte(cond_name)?;
    let cc_str = cc.to_string();
    Some(vec![
        li("SD_XOR", &[("dest", "rd"), ("src", "rd")]),
        li("SD_CMP", &[("src1", "rs1"), ("src2", "rs2")]),
        li("SD_SETCC", &[("dest", "rd"), ("cond", &cc_str)]),
    ])
}

/// Returns the default Fcmp lowering for a specific condition.
pub fn default_fcmp_lowering(cond_name: &str) -> Option<Vec<String>> {
    let cc = fcmp_cond_byte(cond_name)?;
    let cc_str = cc.to_string();
    Some(vec![
        li("SD_XOR", &[("dest", "rd"), ("src", "rd")]),
        li("SD_FCMP", &[("src1", "rs1"), ("src2", "rs2")]),
        li("SD_SETCC", &[("dest", "rd"), ("cond", &cc_str)]),
    ])
}

// ============================================================
// Default lowering for terminators
// ============================================================

/// Returns the default terminator lowering.
/// Returns `None` for terminators that have no generic lowering.
pub fn default_terminator_lowering(term_name: &str) -> Option<Vec<String>> {
    match term_name {
        "Return" => Some(vec![
            li("SD_MOV", &[("dest", RETVAL_VREG), ("src", "val")]),
            li("SD_RET", &[]),
        ]),
        "Jump" => Some(vec![li("SD_JMP", &[("rel", "target")])]),
        "Branch" => Some(vec![
            // cond != 0 → then_block; cond == 0 → else_block
            // CMP cond, 0; JCC NE → then_block; JMP → else_block
            li("SD_XOR", &[("dest", SCRATCH_GPR2), ("src", SCRATCH_GPR2)]),
            li("SD_CMP", &[("src1", "cond"), ("src2", SCRATCH_GPR2)]),
            li("SD_JCC", &[("cond", "1"), ("rel", "true_block")]), // 1 = NotEqual: cond != 0 → then
            li("SD_JMP", &[("rel", "false_block")]),               // cond == 0 → else
        ]),
        "Unreachable" => Some(vec![li("SD_UD2", &[])]),
        "Switch" => Some(vec![
            // Switch cannot be lowered generically without value range info
            li("SD_UD2", &[]),
        ]),
        _ => None,
    }
}

// ============================================================
// Classification helpers
// ============================================================

/// All IR opcode names (matching Opcode enum variants).
pub const ALL_OPCODES: &[&str] = &[
    // Integer arithmetic
    "Iadd",
    "Isub",
    "Imul",
    "Udiv",
    "Sdiv",
    "Urem",
    "Srem",
    // Float arithmetic
    "Fadd",
    "Fsub",
    "Fmul",
    "Fdiv",
    "Fneg",
    "Fabs",
    "Fsqrt",
    // SIMD
    "Vadd",
    "Vsub",
    "Vmul",
    "Vextract",
    "Vinsert",
    // Bitwise
    "Band",
    "Bor",
    "Bxor",
    "Bnot",
    "Ishl",
    "Ushr",
    "Sshr",
    // Comparison (special-cased: Icmp/Fcmp)
    // Icmp, Fcmp are handled separately
    // Memory
    "Load",
    "Store",
    // Constants
    "Iconst",
    "Fconst",
    // Type conversion
    "Sextend",
    "Uextend",
    "Ireduce",
    "Bitcast",
    // Calls
    "Call",
    "CallIndirect",
    // Address
    "StackAddr",
    "GlobalAddr",
    // Other
    "Copy",
    "Select",
    "Nop",
    // Stack alloc
    "Alloca",
    // Address calc
    "GetElementPtr",
];

/// Returns the set of opcode names that have user-defined lowering rules.
pub fn user_defined_opcodes(model: &IsaModel) -> HashSet<String> {
    model
        .lower
        .keys()
        .filter(|k| !k.starts_with("Icmp.") && !k.starts_with("Fcmp."))
        .cloned()
        .collect()
}

/// Returns the set of Icmp condition names that have user-defined lowering rules.
pub fn user_defined_icmp_conds(model: &IsaModel) -> HashSet<String> {
    model
        .lower
        .keys()
        .filter_map(|k| k.strip_prefix("Icmp."))
        .map(|s| s.to_string())
        .collect()
}

/// Returns the set of Fcmp condition names that have user-defined lowering rules.
pub fn user_defined_fcmp_conds(model: &IsaModel) -> HashSet<String> {
    model
        .lower
        .keys()
        .filter_map(|k| k.strip_prefix("Fcmp."))
        .map(|s| s.to_string())
        .collect()
}

/// Returns whether the opcode uses a destructured index field (Iconst { index } / Fconst { index }).
pub fn opcode_uses_const_index(opcode_name: &str) -> bool {
    matches!(opcode_name, "Iconst" | "Fconst")
}
