//! v12 共享工具：codegen（定宽 + 集成层）与 validate 复用的纯函数。
//!
//! 拆分动机：`group_names` 曾在 codegen/mod.rs 与 codegen/integration.rs 各
//! 有一份（行为几乎相同）；`parse_u64` 曾在 codegen/mod.rs 与 validate.rs 各
//! 有一份（完全一致）。集中单点定义，避免后续修改只改一处导致漂移。

use super::model::RegGroup;

/// 解析 TOML 数值字符串（0x 十六进制或十进制）。
pub(crate) fn parse_u64(s: &str) -> Option<u64> {
    let t = s.trim();
    if let Some(h) = t.strip_prefix("0x").or_else(|| t.strip_prefix("0X")) {
        u64::from_str_radix(h, 16).ok()
    } else {
        t.parse::<u64>().ok()
    }
}

/// 组寄存器名列表（names 或 prefix+count 生成；count-only → 默认 "R" 前缀）。
/// 空 names 视为错误（防静默生成空寄存器组）。
pub(crate) fn group_names(g: &RegGroup) -> Result<Vec<String>, String> {
    if let Some(names) = &g.names {
        if names.is_empty() {
            return Err("reg group must have at least one name".into());
        }
        return Ok(names.clone());
    }
    let prefix = g.prefix.clone().unwrap_or_else(|| "R".to_string());
    let count = g
        .count
        .ok_or_else(|| "reg group needs `names` or `count`".to_string())?;
    Ok((0..count as usize)
        .map(|i| format!("{prefix}{i}"))
        .collect())
}
