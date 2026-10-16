//! **codegen 的机读事件出口**（v25 M3）。
//!
//! ## 为什么要有这一层
//!
//! 立项证据（v25 方案 §1 E6）：JIT / asm / E2E 三个测试面都已有落盘事件
//! （`FORGE_JIT_EVENTS` / `FORGE_ASM_EVENTS` / `FORGE_E2E_EVENTS`）——libtest 会吞掉**通过**
//! 测试的输出，没有事件就无法从 CI 日志判断"当轮到底跑了什么"。而 **codegen 阶段是唯一
//! 没有机读出口的**：11 个诊断开关全是 `eprintln!` 人读文本，CI 只能靠正则去啃散文。
//!
//! 这一层把"编译发生了什么"变成**可机读的一行 JSON**：
//!
//! ```text
//! {"event":"COMPILE-DONE","fn":"main","total_us":8123,"blocks_us":120,...}
//! ```
//!
//! * 出口 = 环境变量 **`FORGE_CODEGEN_EVENTS=<文件>`**（追加写；未设置 ⇒ 零开销返回）；
//! * **写失败静默**（与 `FORGE_JIT_EVENTS` 同约定）：可见性辅助**不得**影响编译结论；
//! * **字段序固定**（调用点给的顺序即输出顺序）⇒ 输出确定、测试可整行比较，不必引入
//!   JSON 依赖（自带最小转义）。
//!
//! ## 契约
//!
//! * [`EVENTS`] 是**唯一登记处**：只回答"我们有哪些事件、各自什么时候发"；
//! * `emit` 的名字**必须**在 [`EVENTS`] 里——debug 下 `debug_assert`（当场暴露），release 下
//!   照常发出（观测不得改变行为）；
//! * 守卫 `event_names_are_bijective` 把**源码调用点 ↔ [`EVENTS`] ↔
//!   `docs/reference/backend-pipeline.md` 的事件表**三方双向钉住。

/// 一条事件的登记项：名字 + "什么时候发"。
#[derive(Debug, Clone, Copy)]
pub struct EventSpec {
    pub name: &'static str,
    pub what: &'static str,
}

/// **事件台账**（唯一登记处；守卫与文档表双向相等）。
pub static EVENTS: [EventSpec; 6] = [
    EventSpec {
        name: "COMPILE-START",
        what: "开始编译一个函数（`fn`）",
    },
    EventSpec {
        name: "COMPILE-DONE",
        what: "函数编译成功：总耗时与各阶段耗时（微秒）+ `code_bytes`",
    },
    EventSpec {
        name: "CAPABILITY-GAP",
        what: "编译失败：`gap` = 缺口 ID（非缺口错误时为空）+ `fn` + `stage`",
    },
    EventSpec {
        name: "LEGALIZE-PLAN",
        what: "合法化计划相完成：可动边界条数 `actionable`、问引擎次数 `plans`",
    },
    EventSpec {
        name: "ALLOC-SUMMARY",
        what: "寄存器分配完成：`assigned` = 分配到的值数、`spills` = 溢出编辑数",
    },
    EventSpec {
        name: "EMIT-SUMMARY",
        what: "发射完成：`code_bytes`、`relocs` = 重定位条数",
    },
];

/// 事件的字段值：直接用下沉后的实现（`forge-isa-runtime::diagnostics`）——
/// 转义与落盘只有一份，这里只保留**名字台账**。
pub use forge_isa_runtime::diagnostics::Field;

/// 发出一个事件（`FORGE_CODEGEN_EVENTS=<文件>` 时追加一行 JSON；写失败静默）。
///
/// `fields` 的顺序即输出顺序（确定性）。名字未登记 ⇒ debug 下 panic、release 下照常发出。
pub fn emit(name: &'static str, fields: &[(&str, Field<'_>)]) {
    debug_assert!(
        EVENTS.iter().any(|e| e.name == name),
        "事件 `{name}` 未登记进 `pipeline::events::EVENTS`（守卫 event_names_are_bijective 会红）"
    );
    // 落盘与转义只有一份实现（v26 M1 下沉到 `forge-isa-runtime::diagnostics`）。
    forge_isa_runtime::diagnostics::emit("codegen", name, fields);
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 台账自洽：名字唯一、非空、大写短横线风格。
    #[test]
    fn events_table_is_sane() {
        let mut names: Vec<&str> = EVENTS.iter().map(|e| e.name).collect();
        let n = names.len();
        names.sort_unstable();
        names.dedup();
        assert_eq!(names.len(), n, "事件名必须唯一");
        for e in EVENTS.iter() {
            assert!(!e.name.is_empty() && !e.what.is_empty(), "登记项不得留空");
            assert!(
                e.name
                    .chars()
                    .all(|c| c.is_ascii_uppercase() || c == '-' || c.is_ascii_digit()),
                "事件名用大写短横线风格：{}",
                e.name
            );
        }
    }

    /// 字段转义：字符串带引号/反斜杠不得破坏 JSON 行。
    #[test]
    fn field_escaping() {
        assert_eq!(format!("{}", Field::Str("a\"b\\c")), "\"a\\\"b\\\\c\"");
        assert_eq!(format!("{}", Field::U64(7)), "7");
        assert_eq!(format!("{}", Field::Usize(9)), "9");
    }
}
