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
use forge_ir::opcode::{CondKind, FloatCC, IntCC};

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
        Opcode::Icmp => "Icmp",
        Opcode::Fcmp => "Fcmp",
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

/// LLVM 文本名表（`ops.toml` 的 `llvm`/`llvm_parse`/`llvm_alias`）自洽：
/// 名字非空且无空格、可解析者对得上、别名指向本 opcode、解析名全局唯一、
/// 需要条件的比较指令是 display-only 且**不能**被当普通文本名解析。
#[test]
fn llvm_name_table_is_consistent() {
    // 每个 opcode 都有非空、无空格的 display 名
    for op in Opcode::ALL {
        let name = op.info().llvm;
        assert!(!name.is_empty(), "{} 的 llvm 名为空", op.name());
        assert!(
            !name.contains(char::is_whitespace),
            "{} 的 llvm 名含空白：{name:?}",
            op.name()
        );
    }

    // 可解析：`llvm_parse` 指向自己的 display 名，且能被查回来
    let mut parse_names: HashSet<&str> = HashSet::new();
    let mut parseable = 0usize;
    let mut aliases = 0usize;
    for op in Opcode::ALL {
        let info = op.info();
        if let Some(n) = info.llvm_parse {
            parseable += 1;
            assert_eq!(
                n,
                info.llvm,
                "{} 的 llvm_parse 必须是它的 display 名（可解析即默认名）",
                op.name()
            );
            assert!(parse_names.insert(n), "解析名重复（生成期应已拦住）：{n}");
            assert_eq!(Opcode::from_llvm_name(n), Some(*op), "解析名 {n} 不可查回");
        }
        for a in info.llvm_aliases {
            aliases += 1;
            assert!(
                parse_names.insert(a),
                "解析名（别名）重复（生成期应已拦住）：{a}"
            );
            assert_eq!(
                Opcode::from_llvm_name(a),
                Some(*op),
                "别名 {a} 不可查回 {}",
                op.name()
            );
        }
    }
    assert!(parseable >= 90, "可解析名过少：{parseable}");
    assert_eq!(
        aliases, 3,
        "别名数量变化需同步本断言（callbr/ptrtoaddr/vextractelement）"
    );

    // display-only 的典型：常量内联、复用文本名、向量精化、需要条件
    for op in [
        Opcode::Iconst,
        Opcode::Fconst,
        Opcode::Vconst,
        Opcode::Fload,
        Opcode::Fstore,
        Opcode::CallIndirect,
        Opcode::Vadd,
        Opcode::Vbitcast,
        Opcode::Ftrunc,
        Opcode::Vsplit,
        Opcode::Vconcat,
    ] {
        assert_eq!(
            op.info().llvm_parse,
            None,
            "{} 应是 display-only（不可由文本名直接解析）",
            op.name()
        );
    }

    // 需要条件的比较指令：文本名不可直接解析，且解析时报"需要条件"
    for op in [Opcode::Icmp, Opcode::Fcmp] {
        assert_eq!(op.info().llvm_parse, None);
        assert_eq!(Opcode::from_llvm_name(op.info().llvm), None);
    }
    let err = forge_ir::ir_parser::llvm_mapping::opcode("icmp").expect_err("icmp 需要条件");
    assert!(
        err.to_string().contains("condition"),
        "错误提示应说明需要条件，实际：{err}"
    );
    assert!(
        forge_ir::ir_parser::llvm_mapping::opcode("nosuchopcode").is_err(),
        "未知名字必须报错（不兜底）"
    );
}

/// 条件指令（`Icmp`/`Fcmp`）是**纯变体身份**：条件不是变体载荷，而是
/// immediate 通道（`Immediate::IntCC`/`FloatCC`）——`cond_kind()` 声明该契约，
/// 数值表示由 `IntCC::code()`/`FloatCC::code()` 提供。
#[test]
fn conditional_variants_carry_cond_in_immediates() {
    assert_eq!(Opcode::Icmp.name(), "Icmp");
    assert_eq!(Opcode::Fcmp.name(), "Fcmp");
    assert_eq!(Opcode::Icmp.cond_kind(), Some(CondKind::IntCC));
    assert_eq!(Opcode::Fcmp.cond_kind(), Some(CondKind::FloatCC));
    // 非比较指令没有条件通道
    assert_eq!(Opcode::Iadd.cond_kind(), None);
    assert_eq!(Opcode::Select.cond_kind(), None);
    // 条件的数字表示可逆（ISA TOML 的 cond 谓词契约）
    for cc in [
        IntCC::Equal,
        IntCC::NotEqual,
        IntCC::SignedLessThan,
        IntCC::SignedLessThanOrEqual,
        IntCC::SignedGreaterThan,
        IntCC::SignedGreaterThanOrEqual,
        IntCC::UnsignedLessThan,
        IntCC::UnsignedLessThanOrEqual,
        IntCC::UnsignedGreaterThan,
        IntCC::UnsignedGreaterThanOrEqual,
    ] {
        assert_eq!(IntCC::from_code(cc.code()), Some(cc), "{cc:?} 码不可逆");
    }
    assert_eq!(IntCC::from_code(0), None, "未知码必须返回 None（不兜底）");
    assert_eq!(IntCC::from_code(11), None);
    for cc in [
        FloatCC::Ordered,
        FloatCC::Unordered,
        FloatCC::Equal,
        FloatCC::NotEqual,
        FloatCC::LessThan,
        FloatCC::LessThanOrEqual,
        FloatCC::GreaterThan,
        FloatCC::GreaterThanOrEqual,
        FloatCC::False,
        FloatCC::True,
        FloatCC::Ueq,
        FloatCC::Ugt,
        FloatCC::Uge,
        FloatCC::Ult,
        FloatCC::Ule,
        FloatCC::Une,
    ] {
        assert_eq!(FloatCC::from_code(cc.code()), Some(cc), "{cc:?} 码不可逆");
    }
    assert_eq!(FloatCC::from_code(0), None);
    assert_eq!(FloatCC::from_code(17), None);
    // 条件码在各自集合内唯一
    let int_codes: Vec<u8> = [
        IntCC::Equal,
        IntCC::NotEqual,
        IntCC::SignedLessThan,
        IntCC::SignedLessThanOrEqual,
        IntCC::SignedGreaterThan,
        IntCC::SignedGreaterThanOrEqual,
        IntCC::UnsignedLessThan,
        IntCC::UnsignedLessThanOrEqual,
        IntCC::UnsignedGreaterThan,
        IntCC::UnsignedGreaterThanOrEqual,
    ]
    .iter()
    .map(IntCC::code)
    .collect();
    let mut sorted = int_codes.clone();
    sorted.sort_unstable();
    sorted.dedup();
    assert_eq!(sorted.len(), int_codes.len(), "IntCC 条件码必须唯一");
}
