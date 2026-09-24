//! ABI 错误：一律**可操作**（点名约定/池/能力），绝不静默降级。

use thiserror::Error;

#[derive(Debug, Error, Clone, PartialEq, Eq)]
pub enum AbiError {
    /// 规则里引用了绑定中不存在的池。
    #[error(
        "约定 `{conv}` 引用了池 `{pool}`，但绑定 `{binding}` 里没有它——请在 AbiBinding 里补上（或改规则里的池名）"
    )]
    MissingPool {
        conv: String,
        binding: String,
        pool: String,
    },

    /// 池里的寄存器选择子在目标上解析不出来。
    #[error("绑定 `{binding}` 的池 `{pool}` 第 {index} 项 `{sel}` 解析不到寄存器：{why}")]
    UnresolvedReg {
        binding: String,
        pool: String,
        index: usize,
        sel: String,
        why: String,
    },

    /// 池耗尽且约定不允许栈参数（或该形态必须走栈但没有栈布局）。
    #[error(
        "约定 `{conv}`：参数 {arg} 的寄存器池 `{pool}` 已耗尽，且该约定没有可用的栈参数区域——谱/绑定缺 `[stack]` 信息，或这个签名超出该约定能表达的范围"
    )]
    PoolExhausted {
        conv: String,
        arg: usize,
        pool: String,
    },

    /// 目标缺少该约定需要的能力（ISA 侧的缺口）。
    #[error(
        "约定 `{conv}` 需要能力 `{cap}`（宽度 {bits} 位），但目标 ISA 未声明它——请在 ISA 谱里补一条承担该角色的指令"
    )]
    CapabilityGap {
        conv: String,
        cap: String,
        bits: u16,
    },

    /// 数据表达的形态之外（需要宿主钩子或尚未建模）。
    ///
    /// 消息结构 = "约定：<具体是什么>"，调用方把**能自己判断的那件事**写进 `what`
    /// （缺哪个池、哪个类型、哪条路径），因此这里不再追加通用的"未建模"套话——
    /// 那会让 `forge-isa abi check` 的缺口行读起来自相矛盾。
    #[error("约定 `{conv}`：{what}")]
    Unsupported { conv: String, what: String },

    /// 规则自身不自洽（继承表缺父、循环继承、池重叠…）。
    #[error("AbiRules `{name}` 自洽性错误：{why}")]
    BadRules { name: String, why: String },

    /// 规则 TOML 解析失败。
    #[error("AbiRules TOML 解析失败：{0}")]
    Parse(String),
}
