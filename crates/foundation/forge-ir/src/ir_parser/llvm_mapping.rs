//! LLVM IR 的条件名/向量精化/显示拼装。
//!
//! **指令文本名不再是手写表**：`ops.toml` 是单一事实源（`llvm` = display 名、
//! `llvm_parse` = 该名是否可反解析回本 opcode、`llvm_alias` = 旧名别名），
//! `OpcodeInfo::llvm` / `Opcode::from_llvm_name` 都是它的投影（名字唯一性由
//! `build.rs` 生成期断言）。有损对（`Fload` 复用 `load`、`trunc → Ireduce`、
//! 向量指令由标量名精化…）现在**声明在 `ops.toml` 的 `llvm_parse = false` 上**，
//! 不再靠"读者自己比对两张表"。
//!
//! 本文件只剩与文本层真正绑定的三类：条件名（[`int_cc`]/[`float_cc`]）、
//! 向量精化（[`vector_op`]）、display 拼装（[`llvm_mnemonic`]）。

use crate::error::IrError;
use crate::immediate::Immediate;
use crate::opcode::{CondKind, FloatCC, IntCC, Opcode};

/// LLVM 指令名 → forge Opcode（标量基础；向量类型由 [`vector_op`] 精化）。
///
/// 名字表来自 `ops.toml`（`llvm_parse` + `llvm_alias`）。需要条件的 `icmp`/`fcmp`
/// 在 `ops.toml` 里是 `llvm_parse = false`（它们由语义层带条件构造），这里按
/// **元数据**识别该情形并给出可操作的解析错误——不是硬编码名字串。
pub fn opcode(name: &str) -> Result<Opcode, IrError> {
    if let Some(op) = Opcode::from_llvm_name(name) {
        return Ok(op);
    }
    // display 名相同但需要条件的指令（`cond_kind()` 有值者）——生成的 O(1) 表，
    // 不线性扫 `ALL`。
    if Opcode::from_cond_llvm_name(name).is_some() {
        return Err(IrError::Parse(format!(
            "{name} needs a condition; use {name} <cond>"
        )));
    }
    Err(IrError::UnknownOpcode(name.to_string()))
}

/// LLVM icmp 条件 → forge IntCC（全 10）。
pub fn int_cc(name: &str) -> Result<IntCC, IrError> {
    Ok(match name {
        "eq" => IntCC::Equal,
        "ne" => IntCC::NotEqual,
        "slt" => IntCC::SignedLessThan,
        "sgt" => IntCC::SignedGreaterThan,
        "sle" => IntCC::SignedLessThanOrEqual,
        "sge" => IntCC::SignedGreaterThanOrEqual,
        "ult" => IntCC::UnsignedLessThan,
        "ugt" => IntCC::UnsignedGreaterThan,
        "ule" => IntCC::UnsignedLessThanOrEqual,
        "uge" => IntCC::UnsignedGreaterThanOrEqual,
        _ => return Err(IrError::UnknownIntCc(name.to_string())),
    })
}

/// LLVM fcmp 条件 → forge FloatCC（全 16，LLVM LangRef 语义）。
/// `o*` = ordered（无 NaN 才有真）、`u*` = unordered-or（NaN 也算真）。
pub fn float_cc(name: &str) -> Result<FloatCC, IrError> {
    Ok(match name {
        "false" => FloatCC::False,
        "true" => FloatCC::True,
        "oeq" => FloatCC::Equal,
        "one" => FloatCC::NotEqual,
        "olt" => FloatCC::LessThan,
        "ole" => FloatCC::LessThanOrEqual,
        "ogt" => FloatCC::GreaterThan,
        "oge" => FloatCC::GreaterThanOrEqual,
        "ord" => FloatCC::Ordered,
        "uno" => FloatCC::Unordered,
        "ueq" => FloatCC::Ueq,
        "ugt" => FloatCC::Ugt,
        "uge" => FloatCC::Uge,
        "ult" => FloatCC::Ult,
        "ule" => FloatCC::Ule,
        "une" => FloatCC::Une,
        _ => {
            return Err(IrError::UnknownFloatCc(name.to_string()));
        }
    })
}

/// 标量指令 → 向量指令（操作数为向量类型时精化）。
///
/// 这是"标量文本名 + 向量类型"的语义精化，因此这些向量 opcode 在 `ops.toml` 里
/// 是 `llvm_parse = false`（文本名解析出标量 opcode，再由本函数精化）。
pub fn vector_op(op: Opcode) -> Opcode {
    match op {
        Opcode::Iadd | Opcode::Fadd => Opcode::Vadd,
        Opcode::Isub | Opcode::Fsub => Opcode::Vsub,
        Opcode::Imul | Opcode::Fmul => Opcode::Vmul,
        Opcode::Udiv | Opcode::Sdiv | Opcode::Fdiv => Opcode::Vdiv,
        Opcode::Fneg => Opcode::Vneg,
        Opcode::Fabs | Opcode::Abs => Opcode::Vabs,
        _ => op,
    }
}

/// forge Opcode → LLVM 指令名（display 用）。
///
/// base 名来自 `ops.toml` 的 `llvm` 字段；`Icmp`/`Fcmp` 的条件从 `immediates` 取
/// （`Immediate::IntCC`/`FloatCC`）。转换/向量指令的精确文本由 display 层按操作数
/// 类型补全。
///
/// 条件缺失（IR 未过 verifier 的 `MissingCondImmediate`）时打印 `icmp <cond?>`
/// ——**响亮地**暴露坏 IR，而不是静默省略条件（省略会让输出看起来合法）。
pub fn llvm_mnemonic(op: &Opcode, immediates: &[Immediate]) -> String {
    let base = op.info().llvm;
    match op.cond_kind() {
        Some(CondKind::IntCC) => match immediates.iter().find_map(Immediate::as_int_cc) {
            Some(cc) => format!("{base} {}", cc.mnemonic()),
            None => format!("{base} <cond?>"),
        },
        Some(CondKind::FloatCC) => match immediates.iter().find_map(Immediate::as_float_cc) {
            Some(cc) => format!("{base} {}", cc.mnemonic()),
            None => format!("{base} <cond?>"),
        },
        None => base.to_string(),
    }
}
