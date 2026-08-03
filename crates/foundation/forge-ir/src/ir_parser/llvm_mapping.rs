//! LLVM IR 指令/条件/类型 → forge IR 的映射表。
//!
//! 覆盖全部 105 个 forge Opcode 的 LLVM 文本形式。指令名经 logos `Ident`
//! 进入此处查表；向量类型指令（`add <4 x i32>`）由 `vector_op` 精化。

use crate::opcode::{FloatCC, IntCC, Opcode};

/// LLVM 指令名 → forge Opcode（标量基础；向量类型由 [`vector_op`] 精化）。
/// 返回错误信息用于语义错误报告。
pub fn opcode(name: &str) -> Result<Opcode, String> {
    Ok(match name {
        // 整数算术
        "add" => Opcode::Iadd,
        "sub" => Opcode::Isub,
        "mul" => Opcode::Imul,
        "udiv" => Opcode::Udiv,
        "sdiv" => Opcode::Sdiv,
        "urem" => Opcode::Urem,
        "srem" => Opcode::Srem,
        // 浮点算术
        "fadd" => Opcode::Fadd,
        "fsub" => Opcode::Fsub,
        "fmul" => Opcode::Fmul,
        "fdiv" => Opcode::Fdiv,
        "fneg" => Opcode::Fneg,
        "fabs" => Opcode::Fabs,
        "fsqrt" => Opcode::Fsqrt,
        // 位运算/移位
        "and" => Opcode::Band,
        "or" => Opcode::Bor,
        "xor" => Opcode::Bxor,
        "shl" => Opcode::Ishl,
        "lshr" => Opcode::Ushr,
        "ashr" => Opcode::Sshr,
        // 位操作
        "clz" => Opcode::Clz,
        "ctz" => Opcode::Ctz,
        "ctpop" => Opcode::Popcnt,
        "bitreverse" => Opcode::Bitreverse,
        "bswap" => Opcode::Bswap,
        "rotl" => Opcode::Rotl, // forge 扩展（LLVM 用 fshl 模拟）
        "rotr" => Opcode::Rotr,
        // 饱和/极值
        "abs" => Opcode::Abs,
        "smin" => Opcode::Smin,
        "smax" => Opcode::Smax,
        "umin" => Opcode::Umin,
        "umax" => Opcode::Umax,
        "sadd.sat" => Opcode::SaddSat,
        "ssub.sat" => Opcode::SsubSat,
        "uadd.sat" => Opcode::UaddSat,
        "usub.sat" => Opcode::UsubSat,
        // 浮点扩展
        "fma" => Opcode::Fma,
        "fmin" => Opcode::Fmin,
        "fmax" => Opcode::Fmax,
        "fcopysign" => Opcode::Fcopysign,
        "floor" => Opcode::Ffloor,
        "ceil" => Opcode::Fceil,
        "round" => Opcode::Fround,
        // 转换（trunc 按操作数类型在语义层分发：整数→Ireduce、浮点→Ftrunc）
        "trunc" => Opcode::Ireduce,
        "zext" => Opcode::Uextend,
        "sext" => Opcode::Sextend,
        "bitcast" => Opcode::Bitcast,
        // 溢出（with.overflow）
        "sadd.with.overflow" => Opcode::SaddOverflow,
        "uadd.with.overflow" => Opcode::UaddOverflow,
        "ssub.with.overflow" => Opcode::SsubOverflow,
        "usub.with.overflow" => Opcode::UsubOverflow,
        "smul.with.overflow" => Opcode::SmulOverflow,
        "umul.with.overflow" => Opcode::UmulOverflow,
        // 内存
        "load" => Opcode::Load,
        "store" => Opcode::Store,
        "alloca" => Opcode::Alloca,
        "getelementptr" => Opcode::GetElementPtr,
        // 调用/地址
        "call" => Opcode::Call,
        "stack_addr" => Opcode::StackAddr, // forge 扩展
        "global_addr" => Opcode::GlobalAddr,
        // 值语义
        "select" => Opcode::Select,
        "freeze" => Opcode::Freeze,
        "undef" => Opcode::Undef,
        "poison" => Opcode::Poison,
        "trap" => Opcode::Trap,
        // 聚合/向量
        "extractvalue" => Opcode::ExtractValue,
        "insertvalue" => Opcode::InsertValue,
        "vextractelement" => Opcode::Vextract,
        "insertelement" => Opcode::Vinsert,
        "shufflevector" => Opcode::ShuffleVector,
        "vbroadcast" => Opcode::Vbroadcast, // forge 扩展
        // 原子
        "atomicrmw" => Opcode::AtomicRmw,
        "cmpxchg" => Opcode::Cmpxchg,
        "fence" => Opcode::Fence,
        // forge 扩展
        "not" => Opcode::Bnot,
        "copy" => Opcode::Copy,
        "nop" => Opcode::Nop,
        "isnull" => Opcode::IsNull,
        "isnotnull" => Opcode::IsNotNull,
        "icmp" | "fcmp" => {
            return Err("icmp/fcmp need a condition; use icmp <cond> / fcmp <cond>".into());
        }
        _ => return Err(format!("unknown LLVM instruction '{}'", name)),
    })
}

/// LLVM icmp 条件 → forge IntCC（全 10）。
pub fn int_cc(name: &str) -> Result<IntCC, String> {
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
        _ => return Err(format!("unknown icmp condition '{}'", name)),
    })
}

/// LLVM fcmp 条件 → forge FloatCC。
/// forge FloatCC 仅 8 个有序语义条件；其余 8 个 LLVM 条件（false/ueq/ugt/
/// uge/ult/ule/une/true）无对应，报语义错误。
pub fn float_cc(name: &str) -> Result<FloatCC, String> {
    Ok(match name {
        "oeq" => FloatCC::Equal,
        "one" => FloatCC::NotEqual,
        "olt" => FloatCC::LessThan,
        "ole" => FloatCC::LessThanOrEqual,
        "ogt" => FloatCC::GreaterThan,
        "oge" => FloatCC::GreaterThanOrEqual,
        "ord" => FloatCC::Ordered,
        "uno" => FloatCC::Unordered,
        _ => {
            return Err(format!(
                "fcmp condition '{}' not supported by forge FloatCC (8 ordered conditions only)",
                name
            ));
        }
    })
}

/// 标量指令 → 向量指令（操作数为向量类型时精化）。
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

/// forge Opcode → LLVM 指令名（display 用）。Icmp/Fcmp 带条件名；
/// 转换/向量指令的精确文本由 display 层按操作数类型补全。
pub fn llvm_mnemonic(op: &Opcode) -> String {
    let base = match op {
        // 整数算术
        Opcode::Iadd => "add",
        Opcode::Isub => "sub",
        Opcode::Imul => "mul",
        Opcode::Udiv => "udiv",
        Opcode::Sdiv => "sdiv",
        Opcode::Urem => "urem",
        Opcode::Srem => "srem",
        // 浮点
        Opcode::Fadd => "fadd",
        Opcode::Fsub => "fsub",
        Opcode::Fmul => "fmul",
        Opcode::Fdiv => "fdiv",
        Opcode::Fneg => "fneg",
        Opcode::Fabs => "fabs",
        Opcode::Fsqrt => "fsqrt",
        // 位/移位
        Opcode::Band => "and",
        Opcode::Bor => "or",
        Opcode::Bxor => "xor",
        Opcode::Bnot => "not", // forge 扩展
        Opcode::Ishl => "shl",
        Opcode::Ushr => "lshr",
        Opcode::Sshr => "ashr",
        // 位操作
        Opcode::Clz => "clz",
        Opcode::Ctz => "ctz",
        Opcode::Popcnt => "ctpop",
        Opcode::Bitreverse => "bitreverse",
        Opcode::Bswap => "bswap",
        Opcode::Rotl => "rotl",
        Opcode::Rotr => "rotr",
        // 饱和/极值
        Opcode::Abs => "abs",
        Opcode::Smin => "smin",
        Opcode::Smax => "smax",
        Opcode::Umin => "umin",
        Opcode::Umax => "umax",
        Opcode::SaddSat => "sadd.sat",
        Opcode::SsubSat => "ssub.sat",
        Opcode::UaddSat => "uadd.sat",
        Opcode::UsubSat => "usub.sat",
        // 浮点扩展
        Opcode::Fma => "fma",
        Opcode::Fmin => "fmin",
        Opcode::Fmax => "fmax",
        Opcode::Fcopysign => "fcopysign",
        Opcode::Ffloor => "floor",
        Opcode::Fceil => "ceil",
        Opcode::Fround => "round",
        // 比较（条件由 display 拼）
        Opcode::Icmp { .. } => "icmp",
        Opcode::Fcmp { .. } => "fcmp",
        // 溢出
        Opcode::SaddOverflow => "sadd.with.overflow",
        Opcode::UaddOverflow => "uadd.with.overflow",
        Opcode::SsubOverflow => "ssub.with.overflow",
        Opcode::UsubOverflow => "usub.with.overflow",
        Opcode::SmulOverflow => "smul.with.overflow",
        Opcode::UmulOverflow => "umul.with.overflow",
        // 内存
        Opcode::Load => "load",
        Opcode::Store => "store",
        // 常量（display 内联，不输出指令行）
        Opcode::Iconst | Opcode::Fconst => "iconst",
        // 值语义
        Opcode::Poison => "poison",
        Opcode::Undef => "undef",
        // 转换
        Opcode::Sextend => "sext",
        Opcode::Uextend => "zext",
        Opcode::Ireduce | Opcode::Ftrunc => "trunc",
        Opcode::Bitcast => "bitcast",
        // 调用/地址
        Opcode::Call | Opcode::CallIndirect => "call",
        Opcode::StackAddr => "stack_addr",
        Opcode::GlobalAddr => "global_addr",
        Opcode::Alloca => "alloca",
        Opcode::GetElementPtr => "getelementptr",
        // 向量
        Opcode::Vadd => "add",
        Opcode::Vsub => "sub",
        Opcode::Vmul => "mul",
        Opcode::Vdiv => "div",
        Opcode::Vneg => "fneg",
        Opcode::Vabs => "fabs",
        Opcode::Vextract => "vextractelement",
        Opcode::Vinsert => "insertelement",
        Opcode::Vbitcast => "bitcast",
        Opcode::Vbroadcast => "vbroadcast",
        Opcode::ShuffleVector => "shufflevector",
        // 陷阱/指针/原子/复合/其他
        Opcode::Trap => "trap",
        Opcode::IsNull => "isnull",
        Opcode::IsNotNull => "isnotnull",
        Opcode::AtomicRmw => "atomicrmw",
        Opcode::Cmpxchg => "cmpxchg",
        Opcode::Fence => "fence",
        Opcode::ExtractValue => "extractvalue",
        Opcode::InsertValue => "insertvalue",
        Opcode::Copy => "copy",
        Opcode::Select => "select",
        Opcode::Freeze => "freeze",
        Opcode::Nop => "nop",
    };
    // Icmp/Fcmp 附加条件
    match op {
        Opcode::Icmp { cond } => format!("icmp {}", cond.mnemonic()),
        Opcode::Fcmp { cond } => format!("fcmp {}", cond.mnemonic()),
        _ => base.to_string(),
    }
}
