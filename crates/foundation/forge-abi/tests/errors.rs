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
use forge_abi::{AbiBinding, AbiError, AbiRegistry, AbiRules, Signature, plan_fn};

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
    let reg = forge_abi::builtin::registry().unwrap();
    // arm64 谱没有 FPR/VEC 组 → 没有 `float` 池：浮点参数必须**明确报缺**，
    // 绝不静默把它当整数塞进 X0。
    let err = reg
        .plan(
            &arm64_v12(),
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
}
