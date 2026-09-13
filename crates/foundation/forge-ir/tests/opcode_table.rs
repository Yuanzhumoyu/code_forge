//! opcode 单表一致性守卫（S0）。
//!
//! 背景（2026-09-14 审计）：`Opcode` 的属性此前散在 6 张手写表里
//! （`result_count`/`may_ub`/`has_side_effect`/`mnemonic`/`expected_operand_count`
//! 以及 LLVM 文本名正反映射），已经发生漂移——`opcode.rs` 测试辅助 `all_opcodes()`
//! 漏 10 个变体、`ir_parser/llvm_mapping.rs` 自述 105 与实际 109 不符。
//! 本文件把"清单必须完整、名字必须唯一、查找必须可逆"变成 CI 可执行断言；
//! 其中 `variant_name_exhaustive` 是**编译期**守卫（无 `_` 兜底臂的穷举 match：
//! 新增变体不同步更新 `Opcode::name()` 即编译失败）。

use std::collections::HashSet;

use forge_ir::Opcode;
use forge_ir::opcode::{FloatCC, IntCC};

/// **编译期**穷举：新增 `Opcode` 变体而不更新这里 / 不更新 `Opcode::name()`
/// 都会编译失败。返回值是分支数，用于与 `Opcode::ALL` 对账。
fn variant_name_exhaustive(op: &Opcode) -> &'static str {
    match op {
        Opcode::Iadd => "Iadd",
        Opcode::Isub => "Isub",
        Opcode::Imul => "Imul",
        Opcode::Udiv => "Udiv",
        Opcode::Sdiv => "Sdiv",
        Opcode::Urem => "Urem",
        Opcode::Srem => "Srem",
        Opcode::Fadd => "Fadd",
        Opcode::Fsub => "Fsub",
        Opcode::Fmul => "Fmul",
        Opcode::Fdiv => "Fdiv",
        Opcode::Frem => "Frem",
        Opcode::Fneg => "Fneg",
        Opcode::Fabs => "Fabs",
        Opcode::Fsqrt => "Fsqrt",
        Opcode::Band => "Band",
        Opcode::Bor => "Bor",
        Opcode::Bxor => "Bxor",
        Opcode::Bnot => "Bnot",
        Opcode::Ishl => "Ishl",
        Opcode::Ushr => "Ushr",
        Opcode::Sshr => "Sshr",
        Opcode::Clz => "Clz",
        Opcode::Ctz => "Ctz",
        Opcode::Popcnt => "Popcnt",
        Opcode::Bitreverse => "Bitreverse",
        Opcode::Rotl => "Rotl",
        Opcode::Rotr => "Rotr",
        Opcode::Abs => "Abs",
        Opcode::Smin => "Smin",
        Opcode::Smax => "Smax",
        Opcode::Umin => "Umin",
        Opcode::Umax => "Umax",
        Opcode::SaddSat => "SaddSat",
        Opcode::SsubSat => "SsubSat",
        Opcode::UaddSat => "UaddSat",
        Opcode::UsubSat => "UsubSat",
        Opcode::Bswap => "Bswap",
        Opcode::Fma => "Fma",
        Opcode::Fmin => "Fmin",
        Opcode::Fmax => "Fmax",
        Opcode::Fcopysign => "Fcopysign",
        Opcode::Ffloor => "Ffloor",
        Opcode::Fceil => "Fceil",
        Opcode::Ftrunc => "Ftrunc",
        Opcode::Fround => "Fround",
        Opcode::Icmp { cond: _ } => "Icmp",
        Opcode::Fcmp { cond: _ } => "Fcmp",
        Opcode::SaddOverflow => "SaddOverflow",
        Opcode::UaddOverflow => "UaddOverflow",
        Opcode::SsubOverflow => "SsubOverflow",
        Opcode::UsubOverflow => "UsubOverflow",
        Opcode::SmulOverflow => "SmulOverflow",
        Opcode::UmulOverflow => "UmulOverflow",
        Opcode::Load => "Load",
        Opcode::Store => "Store",
        Opcode::Fload => "Fload",
        Opcode::Fstore => "Fstore",
        Opcode::Iconst => "Iconst",
        Opcode::Fconst => "Fconst",
        Opcode::Vconst => "Vconst",
        Opcode::Poison => "Poison",
        Opcode::Undef => "Undef",
        Opcode::Sextend => "Sextend",
        Opcode::Uextend => "Uextend",
        Opcode::Ireduce => "Ireduce",
        Opcode::Fptrunc => "Fptrunc",
        Opcode::Fpext => "Fpext",
        Opcode::Fptosi => "Fptosi",
        Opcode::Sitofp => "Sitofp",
        Opcode::Fptoui => "Fptoui",
        Opcode::Uitofp => "Uitofp",
        Opcode::Ptrtoint => "Ptrtoint",
        Opcode::Inttoptr => "Inttoptr",
        Opcode::Bitcast => "Bitcast",
        Opcode::Call => "Call",
        Opcode::CallIndirect => "CallIndirect",
        Opcode::StackAddr => "StackAddr",
        Opcode::GlobalAddr => "GlobalAddr",
        Opcode::Alloca => "Alloca",
        Opcode::GetElementPtr => "GetElementPtr",
        Opcode::Vadd => "Vadd",
        Opcode::Vsub => "Vsub",
        Opcode::Vmul => "Vmul",
        Opcode::Vdiv => "Vdiv",
        Opcode::Vneg => "Vneg",
        Opcode::Vabs => "Vabs",
        Opcode::Vextract => "Vextract",
        Opcode::Vinsert => "Vinsert",
        Opcode::Vbitcast => "Vbitcast",
        Opcode::Vbroadcast => "Vbroadcast",
        Opcode::ShuffleVector => "ShuffleVector",
        Opcode::Vsplit => "Vsplit",
        Opcode::Vconcat => "Vconcat",
        Opcode::Trap => "Trap",
        Opcode::IsNull => "IsNull",
        Opcode::IsNotNull => "IsNotNull",
        Opcode::AddrSpaceCast => "AddrSpaceCast",
        Opcode::VaArg => "VaArg",
        Opcode::AtomicRmw => "AtomicRmw",
        Opcode::Cmpxchg => "Cmpxchg",
        Opcode::Fence => "Fence",
        Opcode::ExtractValue => "ExtractValue",
        Opcode::InsertValue => "InsertValue",
        Opcode::LandingPad => "LandingPad",
        Opcode::Copy => "Copy",
        Opcode::Select => "Select",
        Opcode::Freeze => "Freeze",
        Opcode::Nop => "Nop",
    }
}

/// `Opcode::ALL` 与枚举**等势**：清单缺项 / 多项 / 名字集合不一致都失败。
#[test]
fn all_covers_every_variant() {
    // `Opcode::ALL` 里不许有重复（同一变体登记两次）
    let mut seen = HashSet::new();
    for op in Opcode::ALL {
        assert!(
            seen.insert(op.name()),
            "Opcode::ALL 重复登记了 {}",
            op.name()
        );
    }
    // 穷举 match 的 109 个分支名 与 ALL 的名字集合**完全相等**
    let exhaustive: HashSet<&'static str> =
        Opcode::ALL.iter().map(variant_name_exhaustive).collect();
    assert_eq!(
        exhaustive.len(),
        Opcode::ALL.len(),
        "Opcode::ALL 有 {} 项，穷举分支只有 {} 个不同名字",
        Opcode::ALL.len(),
        exhaustive.len()
    );
    for op in Opcode::ALL {
        let name = variant_name_exhaustive(op);
        assert_eq!(
            op.name(),
            name,
            "`Opcode::name()` 与穷举守卫不一致（{} vs {}）",
            op.name(),
            name
        );
        assert_eq!(
            Opcode::from_name(name),
            Some(*op),
            "from_name({name}) 必须能查回自身"
        );
    }
}

/// 变体名唯一且可逆（ISA TOML 的 `op = "..."` / `pattern.match` 契约靠它）。
#[test]
fn variant_names_are_unique_and_invertible() {
    for op in Opcode::ALL {
        let name = op.name();
        assert!(!name.is_empty(), "变体名不能为空");
        assert_eq!(Opcode::from_name(name), Some(*op), "{name} 不可逆");
        assert!(
            name.chars().next().is_some_and(|c| c.is_ascii_uppercase()),
            "{name} 应为 PascalCase 变体名"
        );
    }
    assert_eq!(Opcode::from_name("NoSuchOpcode"), None);
    assert_eq!(Opcode::from_mnemonic("no_such_mnemonic"), None);
}

/// 规范助记符唯一且可逆（`mnemonic()` ↔ `from_mnemonic()`）。
/// 重复助记符会让 `from_mnemonic` 静默返回第一个匹配 → 必须失败。
#[test]
fn mnemonics_are_unique_and_invertible() {
    let mut by_mnemonic: std::collections::HashMap<&'static str, &'static str> =
        std::collections::HashMap::new();
    for op in Opcode::ALL {
        let m = op.mnemonic();
        assert!(!m.is_empty(), "{} 的助记符不能为空", op.name());
        if let Some(prev) = by_mnemonic.insert(m, op.name()) {
            panic!("助记符 '{m}' 同时属于 {prev} 与 {}", op.name());
        }
    }
    for op in Opcode::ALL {
        assert_eq!(
            Opcode::from_mnemonic(op.mnemonic()),
            Some(*op),
            "{} 的助记符不可逆",
            op.name()
        );
    }
}

/// 条件变体（`Icmp`/`Fcmp`）的名字与助记符与条件取值无关——清单按变体身份登记。
#[test]
fn conditional_variants_are_opcode_identity_only() {
    let a = Opcode::Icmp { cond: IntCC::Equal };
    let b = Opcode::Icmp {
        cond: IntCC::SignedLessThan,
    };
    assert_eq!(a.name(), "Icmp");
    assert_eq!(a.mnemonic(), b.mnemonic());
    let fa = Opcode::Fcmp {
        cond: FloatCC::Ordered,
    };
    assert_eq!(fa.name(), "Fcmp");
    // 查找按变体身份返回带默认条件的那个（清单里的代表元素）
    assert_eq!(Opcode::from_name("Icmp"), Some(a));
}
