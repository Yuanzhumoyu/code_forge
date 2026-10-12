//! **规则侧的小枚举**（v23 M2）：它们既是 `AbiRules`（TOML 数据）的字段类型，也是
//! `AbiPlan`（产物）的字段类型——所以放在模型 crate 里，规则层 `pub use` 回去。
//!
//! 放在这里而不是 `rules.rs`：`AbiPlan` 携带它们（`position` / `callee_pop` 的解析结果），
//! 而产物 crate 不能反过来依赖规则加载层（那会把 toml 拖进运行时）。

use serde::{Deserialize, Serialize};

/// 寄存器位置的计数规则。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum PositionRule {
    /// 按类各自计数（riscv SysV：整数与浮点序列独立推进）。
    #[default]
    ByClass,
    /// 按**位置**计数：第 i 个参数用第 i 个槽（Windows x64：第 2 个参数即使第 1 个是整数，
    /// 浮点也走 XMM1）——int/float 共用同一个位置游标。
    ByPosition,
}

/// 被叫方弹栈规则（stdcall/fastcall/pascal）。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum CalleePop {
    /// 调用方弹栈（cdecl/SysV/AAPCS64/RISC-V…）。
    #[default]
    None,
    /// 被叫方按实际栈参数字节数弹栈（stdcall/pascal）。
    SumStackArgs,
    /// 固定字节数（thiscall 的 `ret 4` 等）。
    Fixed(u32),
}

/// 把规则的弹栈方式展开成**字节数**（产物里只留结果）。
pub fn resolve_callee_pop(pop: CalleePop, stack_args_bytes: u32) -> u32 {
    match pop {
        CalleePop::None => 0,
        CalleePop::SumStackArgs => stack_args_bytes,
        CalleePop::Fixed(n) => n,
    }
}
