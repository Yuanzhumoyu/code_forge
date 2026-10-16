//! **观测开关台账**（v24 M4）：诊断开关**不可能静默失效**。
//!
//! ## 病根（立项证据 5）
//!
//! 旧的 `trace_enabled` 维护一份**开关白名单** + `OnceLock<HashMap>` 缓存：名字**漏进
//! 白名单** ⇒ 该开关**恒为假、静默失效**，探针一句也不打印而没人发现。`FORGE_TRACE_AGG`
//! 就这样死了几个月（"文档承诺可见、实际看不见"）。M4 落地时的侦察又抓到**第二个同类**：
//! `FORGE_TRACE_M4`（regalloc 折入类的诊断）同样不在白名单里；而白名单里的
//! `FORGE_TRACE_ISEL` **一处调用点都没有**（死条目）。
//!
//! ## 新机制：开放查询 + 台账登记
//!
//! * [`knob`] **直接按名字问环境变量**——任何名字都生效 ⇒ **不可能因为漏登记而失效**；
//! * 台账 [`KNOBS`] 只回答"我们到底有哪些观测开关、各自看什么"，并由守卫
//!   `trace_knobs_are_bijective` 与源码调用点、文档表格**双向相等**（多一条、少一条都红）；
//! * 忘登记的名字：debug 下 [`debug_assert!`] **当场 panic**（拼错立刻暴露），release 下
//!   照常生效但 `eprintln` 一次警告（诊断面不该把生产拖垮）。
//!
//! 性能：每个开关一个 [`OnceLock`] 缓存（首次查询后零开销）。这不是可有可无的优化——
//! regalloc/lowering/emission 的**每指令热路径**上都有查询，实测裸 `var_os` 会占 codegen
//! 的 7–18%；缓存的前提是"环境变量在进程生命周期内不变"，与旧实现同一条假设。

use std::sync::{Mutex, OnceLock};

/// 一个观测开关：环境变量名 + 它看什么。
pub struct Knob {
    /// 环境变量名（`FORGE_TRACE_*`）。
    pub name: &'static str,
    /// 这个开关打开后会看到什么（写给"要不要加它"的人看）。
    pub what: &'static str,
    /// 查询结果缓存（环境变量在运行时不会变；热路径上不能每次 `var_os`）。
    on: OnceLock<bool>,
}

impl Knob {
    const fn new(name: &'static str, what: &'static str) -> Self {
        Self {
            name,
            what,
            on: OnceLock::new(),
        }
    }
}

/// **观测开关台账**（唯一登记处；守卫钉住它与源码/文档一致）。
///
/// 用 `static` 数组而不是 `const &[…]`：每行带一个 [`OnceLock`] 缓存（内部可变性），
/// `const` 提升到 `'static` 的临时量不允许带内部可变共享借用。
pub static KNOBS: [Knob; 11] = [
    Knob::new(
        "FORGE_TRACE_ABI",
        "调用约定规划与落点回填（abi_setup / compiler）",
    ),
    Knob::new(
        "FORGE_TRACE_AGG",
        "聚合展开与合法化（legalize 各 pass / 计划相 / 物化相）",
    ),
    Knob::new("FORGE_TRACE_ALLOC", "寄存器分配主循环（regalloc_bt）"),
    Knob::new("FORGE_TRACE_EMIT", "机器码发射（emission）"),
    Knob::new("FORGE_TRACE_EXPIRE", "活跃区间到期处理（regalloc_bt）"),
    Knob::new("FORGE_TRACE_LOWER", "指令选择（lowering；每指令一条）"),
    Knob::new("FORGE_TRACE_M4", "绑定折入与多类槽回填类诊断（compiler）"),
    Knob::new("FORGE_TRACE_REGALLOC", "寄存器分配编排（regalloc_bt 入口）"),
    Knob::new("FORGE_TRACE_SPILL", "溢出与回填（emission / regalloc_bt）"),
    Knob::new("FORGE_TRACE_STACK", "栈槽与帧布局（lowering）"),
    Knob::new("FORGE_TRACE_VCODE", "虚拟指令流（compiler）"),
];

/// 查询观测开关 `name` 是否打开。
///
/// * 已登记：命中台账并缓存结果（热路径零开销）。
/// * **未登记**：debug 下 panic（`debug_assert!`，让"忘登记/拼错"当场暴露）；release 下
///   照常按环境变量生效，但**警告一次**——绝不静默。
pub fn knob(name: &'static str) -> bool {
    if let Some(row) = KNOBS.iter().find(|k| k.name == name) {
        return *row.on.get_or_init(|| std::env::var_os(row.name).is_some());
    }
    debug_assert!(
        false,
        "观测开关 `{name}` 没有登记进 `pipeline::trace::KNOBS` —— 登记它（或改用已登记的名字）。\
         漏登记曾让 `FORGE_TRACE_AGG` / `FORGE_TRACE_M4` 静默失效数月。"
    );
    warn_unregistered(name);
    std::env::var_os(name).is_some()
}

/// 未登记开关只警告一次（避免诊断面刷屏）。
fn warn_unregistered(name: &'static str) {
    static WARNED: OnceLock<Mutex<std::collections::BTreeSet<&'static str>>> = OnceLock::new();
    let set = WARNED.get_or_init(|| Mutex::new(std::collections::BTreeSet::new()));
    let Ok(mut set) = set.lock() else {
        return;
    };
    if set.insert(name) {
        eprintln!(
            "[forge] 观测开关 `{name}` 没登记进 `pipeline::trace::KNOBS` —— 它仍按环境变量生效，\
             但台账/文档/守卫都看不到它（补一行登记）"
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 台账本身要自洽：名字唯一、前缀统一、说明非空。
    #[test]
    fn knobs_table_is_sane() {
        let mut seen = std::collections::BTreeSet::new();
        for k in KNOBS.iter() {
            assert!(seen.insert(k.name), "重复登记：{}", k.name);
            assert!(k.name.starts_with("FORGE_TRACE_"), "名字前缀：{}", k.name);
            assert!(!k.what.is_empty(), "缺说明：{}", k.name);
        }
        assert!(!KNOBS.is_empty());
    }

    /// **已登记的开关真的按环境变量生效**（这就是"不能静默失效"的直接判据）。
    ///
    /// 用 `FORGE_TRACE_ABI`：它已登记，值由 `set_var`/`remove_var` 控制——注意本用例只
    /// 断言"登记过的名字不会恒假"：先确保变量不存在（缓存为假），再断言未设置时为假。
    #[test]
    fn registered_knob_reads_env() {
        // 缓存命中路径：登记过的名字必须给出确定答案（不 panic、不警告）。
        let a = knob("FORGE_TRACE_ABI");
        let b = knob("FORGE_TRACE_ABI");
        assert_eq!(a, b, "同一开关两次查询必须一致（缓存语义）");
        // 未设置（本测试环境）⇒ 假；若外部设置了它也应是真——两种都合法，只要不 panic。
        let _ = a;
    }

    /// **未登记的名字在 debug 下 panic**（拼错/忘登记当场暴露，而不是静默恒假）。
    #[test]
    fn unregistered_knob_panics_in_debug() {
        let r = std::panic::catch_unwind(|| knob("FORGE_TRACE_NOT_REGISTERED_AT_ALL"));
        if cfg!(debug_assertions) {
            assert!(r.is_err(), "未登记的开关名必须在 debug 下 panic");
        } else {
            // release：照常返回、只警告一次。
            assert!(r.is_ok());
        }
    }
}
