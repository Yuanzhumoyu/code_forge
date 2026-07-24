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
//! ## Reserved scratch VRegs
//!
//! VReg(96)—VReg(99) are reserved for default lowering temporaries:
//! - VReg(96): GPR scratch (Fconst, Fabs, Fneg mask loads)
//! - VReg(97): GPR scratch (shift count copy, branch zero-reg)
//! - VReg(98): reserved
//! - VReg(99): SP proxy (StackAddr)
//! - VReg(100): XMM0 scratch (precolored Float)

use crate::model::*;
use std::collections::HashSet;

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
        StdInstDef { name: "SD_MOV",     fields: &[("dest", FieldType::VReg), ("src", FieldType::VReg)] },
        StdInstDef { name: "SD_MOV_IMM", fields: &[("dest", FieldType::VReg), ("imm", FieldType::I64)] },

        // === Integer arithmetic ===
        StdInstDef { name: "SD_ADD",  fields: &[("dest", FieldType::VReg), ("src", FieldType::VReg)] },
        StdInstDef { name: "SD_SUB",  fields: &[("dest", FieldType::VReg), ("src", FieldType::VReg)] },
        StdInstDef { name: "SD_MUL",  fields: &[("dest", FieldType::VReg), ("src", FieldType::VReg)] },
        StdInstDef { name: "SD_UDIV", fields: &[("dest", FieldType::VReg), ("src", FieldType::VReg)] },
        StdInstDef { name: "SD_SDIV", fields: &[("dest", FieldType::VReg), ("src", FieldType::VReg)] },
        StdInstDef { name: "SD_UREM", fields: &[("dest", FieldType::VReg), ("src", FieldType::VReg)] },
        StdInstDef { name: "SD_SREM", fields: &[("dest", FieldType::VReg), ("src", FieldType::VReg)] },

        // === Bitwise and shifts ===
        StdInstDef { name: "SD_AND", fields: &[("dest", FieldType::VReg), ("src", FieldType::VReg)] },
        StdInstDef { name: "SD_OR",  fields: &[("dest", FieldType::VReg), ("src", FieldType::VReg)] },
        StdInstDef { name: "SD_XOR", fields: &[("dest", FieldType::VReg), ("src", FieldType::VReg)] },
        StdInstDef { name: "SD_NOT", fields: &[("dest", FieldType::VReg)] },
        StdInstDef { name: "SD_NEG", fields: &[("dest", FieldType::VReg)] },
        StdInstDef { name: "SD_SHL", fields: &[("dest", FieldType::VReg), ("src", FieldType::VReg)] },
        StdInstDef { name: "SD_SHR", fields: &[("dest", FieldType::VReg), ("src", FieldType::VReg)] },
        StdInstDef { name: "SD_SAR", fields: &[("dest", FieldType::VReg), ("src", FieldType::VReg)] },

        // === Comparison ===
        StdInstDef { name: "SD_CMP",   fields: &[("src1", FieldType::VReg), ("src2", FieldType::VReg)] },
        StdInstDef { name: "SD_SETCC", fields: &[("dest", FieldType::VReg), ("cond", FieldType::CondCode)] },

        // === Float data movement ===
        StdInstDef { name: "SD_FMOV",      fields: &[("dest", FieldType::VReg), ("src", FieldType::VReg)] },
        StdInstDef { name: "SD_FMOV_BITS", fields: &[("dest", FieldType::VReg), ("src", FieldType::VReg)] },

        // === Float arithmetic ===
        StdInstDef { name: "SD_FADD",  fields: &[("dest", FieldType::VReg), ("src", FieldType::VReg)] },
        StdInstDef { name: "SD_FSUB",  fields: &[("dest", FieldType::VReg), ("src", FieldType::VReg)] },
        StdInstDef { name: "SD_FMUL",  fields: &[("dest", FieldType::VReg), ("src", FieldType::VReg)] },
        StdInstDef { name: "SD_FDIV",  fields: &[("dest", FieldType::VReg), ("src", FieldType::VReg)] },
        StdInstDef { name: "SD_FSQRT", fields: &[("dest", FieldType::VReg), ("src", FieldType::VReg)] },
        StdInstDef { name: "SD_FNEG",  fields: &[("dest", FieldType::VReg)] },
        StdInstDef { name: "SD_FABS",  fields: &[("dest", FieldType::VReg)] },
        StdInstDef { name: "SD_FCMP",  fields: &[("src1", FieldType::VReg), ("src2", FieldType::VReg)] },
        StdInstDef { name: "SD_I2F",   fields: &[("dest", FieldType::VReg), ("src", FieldType::VReg)] },
        StdInstDef { name: "SD_F2I",   fields: &[("dest", FieldType::VReg), ("src", FieldType::VReg)] },

        // === Integer sign extension ===
        StdInstDef { name: "SD_SEXT", fields: &[("dest", FieldType::VReg), ("src", FieldType::VReg)] },

        // === Memory ===
        StdInstDef { name: "SD_LOAD",  fields: &[("dest", FieldType::VReg), ("src", FieldType::VReg)] },
        StdInstDef { name: "SD_STORE", fields: &[("addr", FieldType::VReg), ("val", FieldType::VReg)] },

        // === Control flow ===
        StdInstDef { name: "SD_JMP",  fields: &[("rel", FieldType::BlockTarget)] },
        StdInstDef { name: "SD_JCC",  fields: &[("cond", FieldType::CondCode), ("rel", FieldType::BlockTarget)] },
        StdInstDef { name: "SD_CALL", fields: &[("target", FieldType::VReg)] },
        StdInstDef { name: "SD_RET",  fields: &[] },
        StdInstDef { name: "SD_NOP",  fields: &[] },
        StdInstDef { name: "SD_UD2",  fields: &[] },

        // === Stack ===
        StdInstDef { name: "SD_PUSH",        fields: &[("reg", FieldType::VReg)] },
        StdInstDef { name: "SD_POP",         fields: &[("reg", FieldType::VReg)] },
        StdInstDef { name: "SD_STACK_ADDR",  fields: &[("dest", FieldType::VReg), ("offset", FieldType::I64)] },
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
        _ => None,
    }
}

/// All IntCC condition names in enum order.
pub const ICMP_CONDS: &[&str] = &[
    "Equal", "NotEqual",
    "SignedLessThan", "SignedLessThanOrEqual",
    "SignedGreaterThan", "SignedGreaterThanOrEqual",
    "UnsignedLessThan", "UnsignedLessThanOrEqual",
    "UnsignedGreaterThan", "UnsignedGreaterThanOrEqual",
];

/// All FloatCC condition names in enum order.
pub const FCMP_CONDS: &[&str] = &[
    "Equal", "NotEqual",
    "LessThan", "LessThanOrEqual",
    "GreaterThan", "GreaterThanOrEqual",
    "Unordered", "Ordered",
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
            li("SD_MOV", &[("dest", "VReg(96)"), ("src", "rs1")]),
            li("SD_UDIV", &[("dest", "VReg(96)"), ("src", "rs2")]),
            li("SD_MUL", &[("dest", "VReg(96)"), ("src", "rs2")]),
            li("SD_MOV", &[("dest", "rd"), ("src", "rs1")]),
            li("SD_SUB", &[("dest", "rd"), ("src", "VReg(96)")]),
        ]),
        "Srem" => Some(vec![
            // remainder = dividend - (dividend / divisor) * divisor
            // VReg(96): scratch; result in rd
            li("SD_MOV", &[("dest", "VReg(96)"), ("src", "rs1")]),
            li("SD_SDIV", &[("dest", "VReg(96)"), ("src", "rs2")]),
            li("SD_MUL", &[("dest", "VReg(96)"), ("src", "rs2")]),
            li("SD_MOV", &[("dest", "rd"), ("src", "rs1")]),
            li("SD_SUB", &[("dest", "rd"), ("src", "VReg(96)")]),
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
        "Iconst" => Some(vec![
            li("SD_MOV_IMM", &[("dest", "rd"), ("imm", "iconst")]),
        ]),
        "Fconst" => Some(vec![
            li("SD_MOV_IMM", &[("dest", "VReg(96)"), ("imm", "fconst")]),
            li("SD_FMOV_BITS", &[("dest", "rd"), ("src", "VReg(96)")]),
        ]),

        // === Type conversion ===
        "Sextend" => Some(vec![
            li("SD_SEXT", &[("dest", "rd"), ("src", "rs1")]),
        ]),
        "Uextend" => Some(vec![
            li("SD_MOV", &[("dest", "rd"), ("src", "rs1")]),
        ]),
        "Ireduce" => Some(vec![
            li("SD_MOV", &[("dest", "rd"), ("src", "rs1")]),
        ]),
        "Bitcast" => Some(vec![
            li("SD_MOV", &[("dest", "rd"), ("src", "rs1")]),
        ]),

        // === Memory ===
        "Load" => Some(vec![
            li("SD_LOAD", &[("dest", "rd"), ("src", "rs1")]),
        ]),
        "Store" => Some(vec![
            li("SD_STORE", &[("addr", "rs2"), ("val", "rs1")]),
        ]),
        "StackLoad" => Some(vec![
            li("SD_LOAD", &[("dest", "rd"), ("src", "rs1")]),
        ]),
        "StackStore" => Some(vec![
            li("SD_STORE", &[("addr", "rs2"), ("val", "rs1")]),
        ]),

        // === Other ===
        "Copy" => Some(vec![
            li("SD_MOV", &[("dest", "rd"), ("src", "rs1")]),
        ]),
        "Nop" => Some(vec![
            li("SD_NOP", &[]),
        ]),
        "Select" => Some(vec![
            // Simplified: just copy the false value (user should customize)
            li("SD_MOV", &[("dest", "rd"), ("src", "rs2")]),
        ]),
        "StackAddr" => Some(vec![
            li("SD_MOV", &[("dest", "rd"), ("src", "VReg(99)")]),
        ]),
        "Call" => Some(vec![
            // Call is complex (needs FuncRef); emit trap to surface missing lowering
            li("SD_UD2", &[]),
        ]),
        "CallIndirect" => Some(vec![
            li("SD_CALL", &[("target", "rs1")]),
        ]),
        "Alloca" => Some(vec![
            // Alloca is normally handled by frontend; placeholder
            li("SD_MOV", &[("dest", "rd"), ("src", "VReg(99)")]),
        ]),
        "GlobalAddr" => Some(vec![
            li("SD_MOV_IMM", &[("dest", "rd"), ("imm", "0")]),
        ]),
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
        "Vextract" | "Vinsert" => Some(vec![
            li("SD_MOV", &[("dest", "rd"), ("src", "rs1")]),
        ]),

        // Phi is handled by lowering.rs (skipped, no instructions emitted)
        "Phi" => Some(vec![
            li("SD_NOP", &[]),
        ]),

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
            li("SD_MOV", &[("dest", "VReg(0)"), ("src", "val")]),
            li("SD_RET", &[]),
        ]),
        "Jump" => Some(vec![
            li("SD_JMP", &[("rel", "target")]),
        ]),
        "Branch" => Some(vec![
            // test cond != 0: XOR scratch, CMP cond, scratch, JCC NE → false, JMP → true
            li("SD_XOR", &[("dest", "VReg(97)"), ("src", "VReg(97)")]),
            li("SD_CMP", &[("src1", "cond"), ("src2", "VReg(97)")]),
            li("SD_JCC", &[("cond", "1"), ("rel", "false_block")]), // 1 = NotEqual
            li("SD_JMP", &[("rel", "true_block")]),
        ]),
        "Unreachable" => Some(vec![
            li("SD_UD2", &[]),
        ]),
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
    "Iadd", "Isub", "Imul", "Udiv", "Sdiv", "Urem", "Srem",
    // Float arithmetic
    "Fadd", "Fsub", "Fmul", "Fdiv", "Fneg", "Fabs", "Fsqrt",
    // SIMD
    "Vadd", "Vsub", "Vmul", "Vextract", "Vinsert",
    // Bitwise
    "Band", "Bor", "Bxor", "Bnot", "Ishl", "Ushr", "Sshr",
    // Comparison (special-cased: Icmp/Fcmp)
    // Icmp, Fcmp are handled separately
    // Memory
    "Load", "Store", "StackLoad", "StackStore",
    // Constants
    "Iconst", "Fconst",
    // Type conversion
    "Sextend", "Uextend", "Ireduce", "Bitcast",
    // Calls
    "Call", "CallIndirect",
    // Address
    "StackAddr", "GlobalAddr",
    // Other
    "Copy", "Phi", "Select", "Nop",
    // Stack alloc
    "Alloca",
    // Address calc
    "GetElementPtr",
];

/// Returns the set of opcode names that have user-defined lowering rules.
pub fn user_defined_opcodes(model: &IsaModel) -> HashSet<String> {
    model.lower.keys()
        .filter(|k| !k.starts_with("Icmp.") && !k.starts_with("Fcmp."))
        .cloned()
        .collect()
}

/// Returns the set of Icmp condition names that have user-defined lowering rules.
pub fn user_defined_icmp_conds(model: &IsaModel) -> HashSet<String> {
    model.lower.keys()
        .filter_map(|k| k.strip_prefix("Icmp."))
        .map(|s| s.to_string())
        .collect()
}

/// Returns the set of Fcmp condition names that have user-defined lowering rules.
pub fn user_defined_fcmp_conds(model: &IsaModel) -> HashSet<String> {
    model.lower.keys()
        .filter_map(|k| k.strip_prefix("Fcmp."))
        .map(|s| s.to_string())
        .collect()
}

/// Returns whether the opcode uses a destructured index field (Iconst { index } / Fconst { index }).
pub fn opcode_uses_const_index(opcode_name: &str) -> bool {
    matches!(opcode_name, "Iconst" | "Fconst")
}
