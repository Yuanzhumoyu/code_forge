//! **能力缺口台账的代码侧锚点**（v24 M3）。
//!
//! ## 为什么要有这一层
//!
//! v24 立项时的侦察结论：能力缺口以 60+ 处 `Unsupported(...)` 的形态**散在代码里**——
//! 没人知道"这台机器 / 这份约定 / 这份谱到底缺什么"，失败信息也只在触发那一刻可见。
//! 于是同一类问题被反复重新发现（"这个本来就不支持"），而"其实已经支持了、只是没人试"
//! 的缺口也长期没人动。
//!
//! 这一层把缺口**变成可机读的集合**：每个"做不了"的失败点都用 [`capability`] 构造，
//! 消息以 `[cap:ID]` 开头。于是：
//!
//! * **台账可守卫**：`capability_gaps_guard` 把源码里的 ID 集合与
//!   `docs/reference/backend-gaps.md` 的台账**逐一相等**（多一条、少一条都红）；
//! * **失败信息自带身份**：用户/CI 能按 `[cap:…]` 搜索、归类、豁免；
//! * **决策有落点**：每个 ID 在台账里必须写清"已实现 / 不做 / 待做"与钉住它的守卫或数据条件。
//!
//! ## 边界（什么**不**算能力缺口）
//!
//! 三类**不**用 `capability`——它们不是"这台机器做不了"，混进来会让台账失去意义：
//!
//! 1. **畸形 IR / 内部不变量**（字段越界、常量池缺项、索引缺失…）：那是输入坏或代码 bug，
//!    用 `IrError::Internal` / 普通 `Unsupported`（**待办**：M3 之后逐条收编到 `Internal`）；
//! 2. **使用错误**（约定没注册、JIT 装载 ISA 不匹配…）：使用者可当场修；
//! 3. **约定/形状数据自相矛盾**（"把 24 字节聚合判成标量栈落点"）：是数据 bug，
//!    报错已点名"约定数据有问题"。

use crate::IrError;

/// 构造一条**能力缺口**错误：消息以 `[cap:ID]` 开头。
///
/// `id` 用大写蛇形（`VA_ARG_AGG_RULE`），与 `docs/reference/backend-gaps.md` 台账的 ID 列一致；
/// 每个 ID 在源码里**只允许出现一次**（守卫钉住）——同一缺口在多处触发就抽成一个失败点。
pub(crate) fn capability(id: &str, msg: impl std::fmt::Display) -> IrError {
    IrError::Unsupported(format!("[cap:{id}] {msg}"))
}

/// 从一条错误里取出**缺口 ID**（`[cap:ID]`）；不是缺口错误 ⇒ `None`。
///
/// 为什么要访问器而不是让调用方自己解析字符串：`IrError::Unsupported` 的 `Display`
/// 会在前面加 `Unsupported: `，位置会变；把"怎么从错误里认出缺口"收在一处，
/// 上层（测试、CI、诊断工具）就不用各自写正则。
pub fn gap_id(e: &IrError) -> Option<String> {
    let s = format!("{e}");
    let start = s.find("[cap:")? + "[cap:".len();
    let rest = &s[start..];
    let end = rest.find(']')?;
    Some(rest[..end].to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 缺口错误的消息里必须带 `[cap:ID]`（守卫靠这个前缀把集合抓出来），
    /// 且 `gap_id` 能原样取回 ID（不依赖 `Display` 的前缀位置）。
    #[test]
    fn capability_errors_are_marked() {
        let e = capability("DEMO_GAP", "这台机器没有该能力");
        let s = format!("{e}");
        assert!(s.contains("[cap:DEMO_GAP]"), "实际：{s}");
        assert!(s.contains("这台机器没有该能力"));
        assert_eq!(gap_id(&e).as_deref(), Some("DEMO_GAP"));
        // 非缺口错误 ⇒ `None`（不能把普通 Unsupported 也认成缺口）。
        assert_eq!(gap_id(&IrError::Unsupported("普通失败".into())), None);
    }
}
