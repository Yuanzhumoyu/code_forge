//! **黄金快照**：四份内置约定 × 固定语料的 `AbiPlan`（`AbiPlan::to_text()`）。
//!
//! 作用与"防漂移"的边界：
//!
//! - 它钉住的是**引擎 + 数据**的输出，不是某个 ISA 的机器码；
//! - 快照里出现 `ERR <变体名>` 的行是**刻意记录**的 fail-closed 边界
//!   （例如 arm64 没有浮点寄存器组 ⇒ `MissingPool`）——不要为了"全绿"把它们抹掉；
//! - 输出不符时先看 diff：**确认新输出正确**再 `FORGE_ABI_BLESS=1` 重新生成，
//!   并把变更原因写进 CHANGELOG。

mod common;

use common::*;
use forge_abi::{AbiError, AbiRegistry};

/// 直接按变体名分类（快照要稳定，所以不把中文错误消息写进黄金文本）。
fn err_label(e: &AbiError) -> String {
    match e {
        AbiError::MissingPool { .. } => "MissingPool".into(),
        AbiError::UnresolvedReg { .. } => "UnresolvedReg".into(),
        AbiError::PoolExhausted { .. } => "PoolExhausted".into(),
        AbiError::CapabilityGap { .. } => "CapabilityGap".into(),
        AbiError::Unsupported { .. } => "Unsupported".into(),
        AbiError::BadRules { .. } => "BadRules".into(),
        AbiError::Parse(_) => "Parse".into(),
    }
}

/// (约定名, ISA 名)：内置绑定里存在的组合。
const COMBOS: &[(&str, &str)] = &[
    ("win64", "x86_64_v12"),
    ("sysv64", "x86_64_v12"),
    ("aapcs64", "arm64_v12"),
    ("lp64d", "riscv64_v12"),
];

fn render(reg: &AbiRegistry, isa: &str, conv: &str) -> String {
    let target = target_for(isa).unwrap_or_else(|| panic!("没有合成目标 `{isa}`"));
    let mut out = String::new();
    out.push_str(&format!("# {isa} / {conv}\n"));
    for case in corpus().into_iter().chain(variadic_cases()) {
        match reg.plan(&target, conv, &case.sig) {
            Ok(plan) => {
                out.push_str(&format!("## {}\n", case.name));
                out.push_str(&plan.to_text());
            }
            Err(e) => out.push_str(&format!("## {}\nERR {}\n", case.name, err_label(&e))),
        }
    }
    out
}

#[test]
fn builtin_plans_match_goldens() {
    let reg = forge_abi::builtin::registry().expect("内置注册表");
    for (conv, isa) in COMBOS {
        let text = render(&reg, isa, conv);
        check_golden(&format!("{conv}.plan.txt"), &text);
    }
}

/// 快照自身必须**确定性**：同一输入两次渲染逐字节相同。
#[test]
fn plan_text_is_deterministic() {
    let reg = forge_abi::builtin::registry().expect("内置注册表");
    for (conv, isa) in COMBOS {
        assert_eq!(render(&reg, isa, conv), render(&reg, isa, conv));
    }
}
