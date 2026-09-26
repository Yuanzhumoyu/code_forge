//! **fail-closed 目录**：把"说不清就不做"的每一条都钉住，并且要求**消息能指路**。
//!
//! 这里的每个用例都对应一次真实的踩坑或一条设计红线：
//! 未注册的约定不静默退回缺省、写错的键不静默忽略、缺寄存器不猜名字、
//! ISA 没有的能力不降级成"差不多能用"。
//!
//! 合成规矩：约定/绑定的 `isa` 一律写 `x86_64_v12`，目标用 `common::x86_64_v12()`
//! （寄存器名 `RAX/RCX/RDX/RBX` 都能解析），这样测的就是**被测的那条错误路径**，
//! 不会被"ISA 名不符"抢先拦掉。

mod common;

use common::*;
use forge_abi::{AbiBinding, AbiError, AbiRegistry, AbiRules, Placement, Signature, plan_fn};

#[test]
fn unregistered_convention_is_an_error_not_a_default() {
    let reg = AbiRegistry::with_builtin_rules().unwrap();
    let t = x86_64_v12();
    let err = reg
        .plan(&t, "stdcall", &Signature::new(vec![], None))
        .unwrap_err();
    assert!(matches!(err, AbiError::BadRules { .. }), "{err:?}");
    // 消息要能让使用者自己修：说清"未注册"、列出已注册的名字、点名内置来源。
    let msg = err.to_string();
    assert!(msg.contains("未注册"), "{msg}");
    assert!(msg.contains("win64"), "应列出已注册的约定：{msg}");
    assert!(msg.contains("内置"), "应指向内置来源：{msg}");
}

#[test]
fn missing_binding_names_the_isa_and_convention() {
    let reg = AbiRegistry::with_builtin_rules().unwrap();
    let t = arm64_v12();
    let err = reg
        .plan(&t, "win64", &Signature::new(vec![], None))
        .unwrap_err();
    let msg = err.to_string();
    assert!(msg.contains("arm64_v12") && msg.contains("win64"), "{msg}");
    assert!(msg.contains("绑定"), "{msg}");
}

#[test]
fn unknown_register_name_fails_with_pool_and_position() {
    let mut reg = AbiRegistry::with_builtin_rules().unwrap();
    reg.insert_binding_toml(
        r#"
isa = "x86_64_v12"
conv = "win64"
[pools]
int = ["RCX", "NOT_A_REG"]
ret_int = ["RAX"]
ret_float = ["XMM0"]
cs_gpr = ["RBX"]
"#,
    )
    .unwrap();
    // 第一个参数用 RCX 成功，第二个撞上坏名字 → 报错要带池名与那个选择子。
    let sig = Signature::new(vec![("a".into(), i64_()), ("b".into(), i64_())], None);
    let err = reg.plan(&x86_64_v12(), "win64", &sig).unwrap_err();
    assert!(matches!(err, AbiError::UnresolvedReg { .. }), "{err:?}");
    let msg = err.to_string();
    assert!(msg.contains("NOT_A_REG"), "{msg}");
    assert!(msg.contains("int"), "要点名池：{msg}");
}

#[test]
fn empty_pool_is_rejected_at_registration() {
    let mut reg = AbiRegistry::with_builtin_rules().unwrap();
    let e = reg
        .insert_binding_toml(
            r#"
isa = "x86_64_v12"
conv = "win64"
[pools]
int = []
"#,
        )
        .unwrap_err();
    assert!(e.to_string().contains("空"), "{e}");
}

#[test]
fn missing_pool_is_explicit_not_silent() {
    // 用一份**故意不给 `float` 池**的绑定（覆盖内置那份）：浮点参数必须**明确报缺池**，
    // 绝不静默把它当整数塞进 X0。（v20 A5 起内置 arm64 绑定已有 `float` 池，所以这里
    // 自己提供一份缺池的——测的是"缺池的报法"，不是"arm64 有没有池"。）
    let mut reg = forge_abi::builtin::registry().unwrap();
    reg.insert_binding_toml(
        "isa = \"arm64_v12\"\nconv = \"aapcs64\"\n[pools]\nint = [\"X0\", \"X1\"]\nret_int = [\"X0\"]\n",
    )
    .unwrap();
    let err = reg
        .plan(
            &arm64_v12_with_fpr(),
            "aapcs64",
            &Signature::new(vec![("x".into(), f64_())], None),
        )
        .unwrap_err();
    assert!(matches!(err, AbiError::MissingPool { .. }), "{err:?}");
    let msg = err.to_string();
    assert!(msg.contains("float"), "{msg}");
    assert!(msg.contains("arm64_v12"), "{msg}");
}

#[test]
fn typo_in_toml_keys_is_rejected() {
    // `deny_unknown_fields`：写错的键直接报（而不是静默用缺省值）。
    let e = AbiRules::from_toml(
        r#"
name = "x"
calssify = [ { when = { kind = "scalar" }, do = { direct = { pool = "int" } } } ]
"#,
    )
    .unwrap_err();
    assert!(matches!(e, AbiError::Parse(_)), "{e:?}");

    // 未知的取数规则名（`slots = "hfa4"`）在 validate 期就报。
    let r = AbiRules::from_toml(
        r#"
name = "x"
classify = [ { when = { kind = "aggregate" }, do = { direct = { pool = "float", slots = "hfa4" } } } ]
"#,
    )
    .unwrap();
    let e = r.validate().unwrap_err();
    assert!(e.to_string().contains("hfa4"), "{e}");
}

#[test]
fn malformed_rules_are_rejected_with_reasons() {
    // 非 2 的幂的栈对齐。
    let r = AbiRules::from_toml(
        r#"
name = "x"
stack_align = 12
classify = [ { when = { kind = "scalar" }, do = { direct = { pool = "int" } } } ]
"#,
    )
    .unwrap();
    assert!(r.validate().unwrap_err().to_string().contains("2 的幂"));

    // 未知族名。
    let r = AbiRules::from_toml(
        r#"
name = "x"
classify = [ { when = { kind = "banana" }, do = { direct = { pool = "int" } } } ]
"#,
    )
    .unwrap();
    assert!(
        r.validate()
            .unwrap_err()
            .to_string()
            .contains("不是已知族名")
    );

    // 没有条件的 when：会吃掉后面所有规则。
    let r = AbiRules::from_toml(
        r#"
name = "x"
classify = [ { when = {}, do = { direct = { pool = "int" } } } ]
"#,
    )
    .unwrap();
    assert!(
        r.validate()
            .unwrap_err()
            .to_string()
            .contains("没有任何条件")
    );

    // 空名字。
    let r = AbiRules::from_toml(
        r#"
name = ""
classify = [ { when = { kind = "scalar" }, do = { direct = { pool = "int" } } } ]
"#,
    )
    .unwrap();
    assert!(r.validate().unwrap_err().to_string().contains("name"));
}

/// 绑定的 `conv` 必须与规则的 `name` 一致（挂错名字 = 一套约定被另一套的寄存器驱动）。
#[test]
fn binding_convention_must_match_the_rules() {
    let reg = AbiRegistry::with_builtin_rules().unwrap();
    let rules = reg.rules("win64").unwrap().clone();
    let binding = AbiBinding::from_toml(
        r#"
isa = "x86_64_v12"
conv = "sysv64"
[pools]
int = ["RDI"]
"#,
    )
    .unwrap();
    let err = plan_fn(
        &x86_64_v12(),
        &rules,
        &binding,
        &Signature::new(vec![], None),
        None,
    )
    .unwrap_err();
    assert!(err.to_string().contains("必须一致"), "{err}");
}

/// 约定说"宽返回走 sret"却没给槽 → 明确拒绝（不猜一个寄存器当 sret 指针）。
#[test]
fn sret_without_a_declared_pool_is_unsupported() {
    let mut reg = AbiRegistry::new();
    reg.insert_rules_toml(
        r#"
name = "nosret"
classify = [ { when = { kind = "scalar" }, do = { direct = { pool = "int" } } } ]
ret_classify = [ { when = { kind = "aggregate" }, do = { indirect = { via = "hidden_sret" } } } ]
"#,
    )
    .unwrap();
    reg.insert_binding_toml(
        r#"
isa = "x86_64_v12"
conv = "nosret"
[pools]
int = ["RAX"]
"#,
    )
    .unwrap();
    let err = reg
        .plan(
            &x86_64_v12(),
            "nosret",
            &Signature::new(vec![], Some(agg_ii())),
        )
        .unwrap_err();
    assert!(matches!(err, AbiError::Unsupported { .. }), "{err:?}");
    let msg = err.to_string();
    assert!(msg.contains("sret"), "{msg}");
}

/// 变参签名但约定没声明 `va_list` 形态 → 明确不支持（不是静默按普通调用处理）。
#[test]
fn variadic_without_a_va_list_shape_is_unsupported() {
    let mut reg = AbiRegistry::new();
    reg.insert_rules_toml(
        r#"
name = "nova"
classify = [ { when = { kind = "scalar" }, do = { direct = { pool = "int" } } } ]
"#,
    )
    .unwrap();
    reg.insert_binding_toml(
        r#"
isa = "x86_64_v12"
conv = "nova"
[pools]
int = ["RAX", "RCX"]
"#,
    )
    .unwrap();
    let sig = Signature::new(vec![("a".into(), i64_())], None).variadic(1);
    let err = reg.plan(&x86_64_v12(), "nova", &sig).unwrap_err();
    assert!(matches!(err, AbiError::Unsupported { .. }), "{err:?}");
    assert!(err.to_string().contains("va_list"), "{err}");
}

/// 约定要求 push 保存（需要 `gpr_mov`），ISA 却没有这个能力 → 规划期就报，不是生成期炸。
#[test]
fn capability_gap_is_reported_before_codegen() {
    let mut reg = AbiRegistry::new();
    reg.insert_rules_toml(
        r#"
name = "pushy"
stack = { slot_bytes = 8, first_offset_slots = 1 }
classify = [ { when = { kind = "scalar" }, do = { direct = { pool = "int" } } } ]
callee_saved = { mechanism = "push", pools = ["cs_gpr"] }
"#,
    )
    .unwrap();
    reg.insert_binding_toml(
        r#"
isa = "poc_isa"
conv = "pushy"
[pools]
int = ["R1"]
cs_gpr = ["R10"]
"#,
    )
    .unwrap();
    let err = reg
        .plan(&NoCaps, "pushy", &Signature::new(vec![], None))
        .unwrap_err();
    assert!(matches!(err, AbiError::CapabilityGap { .. }), "{err:?}");
    let msg = err.to_string();
    assert!(msg.contains("gpr_mov"), "{msg}");
    assert!(msg.contains("pushy"), "{msg}");
}

/// 目标能力全无（连 mov 都没有）。
struct NoCaps;

impl forge_abi::AbiTarget for NoCaps {
    fn isa_name(&self) -> &str {
        "poc_isa"
    }
    fn reg_count(&self) -> u32 {
        11
    }
    fn reg_name(&self, i: u32) -> Option<String> {
        Some(format!("R{i}"))
    }
    fn reg_class_name(&self, _i: u32) -> String {
        "GPR(8)".into()
    }
    fn reg_index(&self, n: &str) -> Option<u32> {
        n.strip_prefix('R')?.parse().ok()
    }
    fn reg_width(&self, _i: u32) -> u8 {
        8
    }
    fn pinned(&self, _i: u32) -> bool {
        false
    }
    fn allocatable(&self) -> Vec<u32> {
        vec![1]
    }
    fn cap(&self, _cap: forge_abi::Capability) -> Option<u16> {
        None
    }
}

/// 返回值要 2 个槽、返回池只有 1 个 → 明确报"不够"（不是只回半个）。
#[test]
fn two_slot_return_with_a_one_slot_pool_is_unsupported() {
    let mut reg = AbiRegistry::new();
    reg.insert_rules_toml(
        r#"
name = "thin"
classify = [ { when = { kind = "scalar" }, do = { direct = { pool = "int" } } } ]
ret_classify = [ { when = { kind = "aggregate", size_le = 16 }, do = { direct = { pool = "ret_int", slots = 2 } } } ]
"#,
    )
    .unwrap();
    reg.insert_binding_toml(
        r#"
isa = "x86_64_v12"
conv = "thin"
[pools]
int = ["RAX", "RCX"]
ret_int = ["RAX"]
"#,
    )
    .unwrap();
    let err = reg
        .plan(
            &x86_64_v12(),
            "thin",
            &Signature::new(vec![], Some(agg_ii())),
        )
        .unwrap_err();
    assert!(matches!(err, AbiError::Unsupported { .. }), "{err:?}");
    assert!(err.to_string().contains("2 个寄存器槽"), "{err}");
}

/// ≥3 槽的**返回**在本片明确拒绝（args 侧走 `RegGroup`，返回侧的搬运是 A6）。
#[test]
fn three_slot_return_is_explicitly_unsupported() {
    let mut reg = AbiRegistry::new();
    reg.insert_rules_toml(
        r#"
name = "wide"
classify = [ { when = { kind = "scalar" }, do = { direct = { pool = "int" } } } ]
ret_classify = [ { when = { kind = "aggregate" }, do = { direct = { pool = "ret_int", slots = "hfa" } } } ]
"#,
    )
    .unwrap();
    reg.insert_binding_toml(
        r#"
isa = "x86_64_v12"
conv = "wide"
[pools]
int = ["RAX"]
ret_int = ["RAX", "RCX", "RDX", "RBX"]
"#,
    )
    .unwrap();
    let err = reg
        .plan(&x86_64_v12(), "wide", &Signature::new(vec![], Some(hfa4())))
        .unwrap_err();
    assert!(matches!(err, AbiError::Unsupported { .. }), "{err:?}");
    assert!(err.to_string().contains("A6"), "{err}");
}

/// 内置数据自身的守卫：四份内置约定都注册得上、每份都有绑定、池名都能在合成目标上解析。
#[test]
fn builtin_catalog_is_self_consistent() {
    let reg = forge_abi::builtin::registry().unwrap();
    let names = reg.conv_names();
    for want in ["c", "win64", "sysv64", "aapcs64", "lp64d"] {
        assert!(names.contains(&want), "缺内置约定 {want}：{names:?}");
    }
    let pairs = reg.binding_names();
    for want in [
        ("x86_64_v12".to_string(), "win64".to_string()),
        ("x86_64_v12".to_string(), "sysv64".to_string()),
        ("arm64_v12".to_string(), "aapcs64".to_string()),
        ("riscv64_v12".to_string(), "lp64d".to_string()),
    ] {
        assert!(pairs.contains(&want), "缺内置绑定 {want:?}：{pairs:?}");
    }
    // 绑定引用的每个池名都要能在目标上解析（名字写错靠这一条兜住）。
    for (isa, conv) in &pairs {
        let Some(t) = target_for(isa) else { continue };
        let b = reg.binding(isa, conv).unwrap();
        for pool in b.pools.keys() {
            b.resolve_pool(pool, &t)
                .unwrap_or_else(|e| panic!("{isa}/{conv} 的池 `{pool}` 解析失败：{e}"));
        }
    }
    // **C 别名**：内置里恰好三份**约定**声称"我是这台机器的 C"，且归属明确。
    // 别名是"整套代答"（规则 + 绑定同源），所以断言落在约定名上。
    let aliased: Vec<&str> = reg
        .conv_names()
        .into_iter()
        .filter(|n| {
            reg.rules(n)
                .is_some_and(|r| r.aliases.iter().any(|a| a == "c"))
        })
        .collect();
    assert_eq!(aliased, ["aapcs64", "lp64d", "win64"], "声称 C 别名的约定");
    for (isa, want) in [
        ("x86_64_v12", "win64"),
        ("arm64_v12", "aapcs64"),
        ("riscv64_v12", "lp64d"),
    ] {
        assert_eq!(
            reg.resolve_conv(isa, "c").unwrap(),
            want,
            "{isa} 上 `c` 应解析成整套 {want}"
        );
    }
}

// ───────────────── C 别名：抽象约定名 → 这台机器上整套生效的约定 ─────────────────

/// 单寄存器落点的寄存器名。
fn reg_name(p: &Placement) -> String {
    match p {
        Placement::Reg { reg, .. } => reg.name.clone(),
        Placement::RegPair { lo, hi } => format!("{}:{}", lo.name, hi.name),
        other => panic!("期望寄存器落点，实际 {other:?}"),
    }
}

/// **C 别名**：IR 的缺省 `CallConvId::Builtin(C)` 只是抽象名；"这台机器上的 C 是哪一份"
/// 由**约定的数据**回答（`aliases = ["c"]`）——而且是**整套**代答（规则 + 绑定同源）。
///
/// 这条断言是实测换来的：只让**绑定**代答（拿 `c` 自己的规则配 Win64 的寄存器）会得到
/// by_class 的槽位与 RCX 返回——混合 int/float 参数读错、sret+by-ref 槽位错位。
#[test]
fn the_c_convention_resolves_to_the_whole_platform_convention() {
    let reg = forge_abi::builtin::registry().unwrap();
    let sig = Signature::new(
        vec![("a".into(), i64_()), ("x".into(), f64_())],
        Some(i64_()),
    );

    let p = reg.plan(&x86_64_v12(), "c", &sig).unwrap();
    assert_eq!(p.conv, "win64", "x86_64_v12 的 C 就是整套 Win64");
    assert_eq!(reg_name(&p.args[0].place), "RCX");
    assert_eq!(
        reg_name(&p.args[1].place),
        "XMM1",
        "by_position（Win64 规则）而不是 by_class 的 XMM0"
    );
    // 精确名不受别名影响：同一台机器上 `sysv64` 仍按类计数。
    let p = reg.plan(&x86_64_v12(), "sysv64", &sig).unwrap();
    assert_eq!(reg_name(&p.args[1].place), "XMM0");

    // 另外两台机器：整数签名足以区分（v20 A5 起 arm64 也有 FPR 组与 `cs_fpr` 池，
    // 所以这里用带上 FPR 的合成目标——与 `target_for` 同一口径）。
    let int_sig = Signature::new(vec![("a".into(), i64_())], Some(i64_()));
    let p = reg.plan(&riscv64_v12(), "c", &int_sig).unwrap();
    assert_eq!(
        (p.conv.as_str(), reg_name(&p.args[0].place).as_str()),
        ("lp64d", "X10")
    );
    let p = reg.plan(&arm64_v12_with_fpr(), "c", &int_sig).unwrap();
    assert_eq!(
        (p.conv.as_str(), reg_name(&p.args[0].place).as_str()),
        ("aapcs64", "X0")
    );
}

/// **同一台机器上两份约定都声称代答同一别名 ⇒ 明确报错**（不按注册序猜）。
#[test]
fn two_conventions_claiming_one_alias_on_a_machine_are_rejected() {
    let mut reg = forge_abi::builtin::registry().unwrap();
    reg.insert_rules_toml("name = \"my_c\"\nparent = \"c\"\naliases = [\"c\"]\n")
        .unwrap();
    reg.insert_binding_toml("isa = \"x86_64_v12\"\nconv = \"my_c\"\n[pools]\nint = [\"RCX\"]\n")
        .unwrap();
    let err = reg
        .plan(&x86_64_v12(), "c", &Signature::new(vec![], None))
        .unwrap_err();
    let msg = err.to_string();
    assert!(msg.contains("win64") && msg.contains("my_c"), "{msg}");
}

/// **显式绑定优先于别名**：宿主想让某台机器的 `c` 归自己，就注册自己的 `c` 规则
/// （覆盖内置那份）+ `(ISA, "c")` 绑定——整套都由他说了算。
#[test]
fn an_explicit_registration_wins_over_the_alias() {
    let mut reg = forge_abi::builtin::registry().unwrap();
    // 覆写内置 `c` 的规则（同名插入即替换），只留一条"标量走 int 池"。
    reg.insert_rules_toml(
        "name = \"c\"\nclassify = [ { when = { kind = \"scalar\" }, do = { direct = { pool = \"int\" } } } ]\n",
    )
    .unwrap();
    reg.insert_binding_toml("isa = \"x86_64_v12\"\nconv = \"c\"\n[pools]\nint = [\"RDX\"]\n")
        .unwrap();
    let sig = Signature::new(vec![("a".into(), i64_())], Some(i64_()));
    let p = reg.plan(&x86_64_v12(), "c", &sig).unwrap();
    assert_eq!(
        (p.conv.as_str(), reg_name(&p.args[0].place).as_str()),
        ("c", "RDX")
    );
    // 别名来源那份（win64）本身没被动过。
    let p = reg.plan(&x86_64_v12(), "win64", &sig).unwrap();
    assert_eq!(reg_name(&p.args[0].place), "RCX");
}

/// 别名的写法错误在**解析/校验期**就报出来（不拖到规划期才说不清）。
#[test]
fn alias_mistakes_are_rejected_early() {
    let bad = |why: &str, toml: &str| {
        let e = AbiRules::from_toml(toml)
            .and_then(|r| r.validate())
            .expect_err(why);
        e.to_string()
    };
    let self_named = bad(
        "别名与自身 name 同名应被拒",
        "name = \"win64\"\naliases = [\"win64\"]\n",
    );
    assert!(self_named.contains("同名"), "{self_named}");
    let dup = bad(
        "同一份规则里重复别名应被拒",
        "name = \"win64\"\naliases = [\"c\", \"c\"]\n",
    );
    assert!(dup.contains("重复"), "{dup}");
}
