//! 严格 TOML 解析：TOML 文本 → v12 模型。
//!
//! 委托 `toml` crate 做标准 TOML 解析；`deny_unknown_fields` 保证未知键/表
//! 一律报错（v11 文件无法通过本解析器——这是"不兼容"的直接体现）。

use super::V12Error;
use super::model::V12Model;

/// 解析 TOML 文本为 v12 模型（不做语义校验）。
pub fn parse(source: &str) -> Result<V12Model, V12Error> {
    toml::from_str(source).map_err(|e| V12Error::Parse(format!("TOML: {e}")))
}
