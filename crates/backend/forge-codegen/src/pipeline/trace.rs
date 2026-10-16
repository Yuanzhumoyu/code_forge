//! **观测开关**（v24 M4 立项 → v26 M1 下沉）：诊断开关**不可能静默失效**。
//!
//! ## 病根
//!
//! 旧的 `trace_enabled` 维护一份**开关白名单** + `OnceLock<HashMap>` 缓存：名字**漏进
//! 白名单** ⇒ 该开关**恒为假、静默失效**，探针一句也不打印而没人发现。`FORGE_TRACE_AGG`
//! 就这样死了几个月（"文档承诺可见、实际看不见"）。M4 落地时的侦察又抓到**第二个同类**：
//! `FORGE_TRACE_M4`（regalloc 折入类的诊断）同样不在白名单里；而白名单里的
//! `FORGE_TRACE_ISEL` **一处调用点都没有**（死条目，已随 M4 删）。
//!
//! ## 现在：开放查询 + **一张**台账（v26 M1 归并）
//!
//! 台账与查询实现在 [`forge_isa_runtime::diagnostics`]（`KNOBS` / `knob`）： **后端与
//! `forge-rustc` 共用同一张表**——此前 `forge-rustc` 自带一份名字集合（无守卫），两侧能各说
//! 各话。本模块只做**同源转发**，调用点（`crate::pipeline::trace::knob("FORGE_TRACE_AGG")`）
//! 一行都不用改。
//!
//! * [`knob`] **直接按名字问环境变量**——任何名字都生效 ⇒ **不可能因为漏登记而失效**；
//! * 台账 [`KNOBS`] 只回答"我们到底有哪些观测开关、各自看什么"，并由守卫
//!   `trace_knobs_are_bijective` 与源码调用点、契约文档表格**三方双向相等**（多一条、少一条都红）；
//! * 忘登记的名字：debug 下 panic（拼错立刻暴露），release 下照常生效但 `eprintln` 一次警告。

pub use forge_isa_runtime::diagnostics::{KNOBS, Knob, knob};

#[cfg(test)]
mod tests {
    use super::*;

    /// 台账已下沉：本模块看到的必须就是运行时那一张（条数与查询结果都对得上）。
    ///
    /// 断言的是"**不 panic 且两次一致**"，不是"为真"——开关未设置时 `knob` 本就该是假。
    #[test]
    fn knobs_live_in_the_runtime_ledger() {
        assert_eq!(KNOBS.len(), forge_isa_runtime::diagnostics::KNOBS.len());
        let a = knob("FORGE_TRACE_AGG");
        assert_eq!(a, knob("FORGE_TRACE_AGG"), "缓存语义：两次查询必须一致");
    }
}
