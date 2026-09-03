//! 严格 TOML 解析：TOML 文本 → v12 模型。
//!
//! 委托 `toml` crate 做标准 TOML 解析；`deny_unknown_fields` 保证未知键/表
//! 一律报错（v11 文件无法通过本解析器——这是"不兼容"的直接体现）。
//! 错误带 TOML 行列（`toml::de::Error::span()` → `diag::line_col`），
//! 由调用方拼上文件路径构成可点击的 `路径:行:列`。

use super::V12Error;
use super::diag;
use super::model::V12Model;

/// 解析 TOML 文本为 v12 模型（不做语义校验）。
pub fn parse(source: &str) -> Result<V12Model, V12Error> {
    toml::from_str(source).map_err(|e| {
        let (line, col) = e
            .span()
            .map(|s| diag::line_col(source, s.start))
            .unwrap_or((1, 1));
        // toml 的 Display 自带多行 caret 片段，这里保留（信息量大），
        // 位置由前缀给出，两者不冲突。
        V12Error::Parse {
            line,
            col,
            msg: format!("TOML: {e}"),
        }
    })
}
