//! 调试信息 (DebugInfo) — 源码位置元数据。
//!
//! 为 IR 指令附加源码位置信息，支持调试输出和未来的 DWARF 生成。

use super::entity::Value;
use std::fmt;

/// 源码位置。
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct SourceLocation {
    /// 文件路径。
    pub file: Option<String>,
    /// 行号 (1-based)。
    pub line: Option<u32>,
    /// 列号 (1-based)。
    pub column: Option<u32>,
}

impl SourceLocation {
    /// 创建一个新的源码位置。
    pub fn new(file: &str, line: u32, column: u32) -> Self {
        Self {
            file: Some(file.to_string()),
            line: Some(line),
            column: Some(column),
        }
    }

    /// 创建一个仅含行号的位置。
    pub fn line_only(line: u32) -> Self {
        Self {
            file: None,
            line: Some(line),
            column: None,
        }
    }
}

impl fmt::Display for SourceLocation {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match (&self.file, self.line, self.column) {
            (Some(file), Some(line), Some(col)) => write!(f, "{}:{}:{}", file, line, col),
            (_, Some(line), Some(col)) => write!(f, "line {}:{}", line, col),
            (_, Some(line), _) => write!(f, "line {}", line),
            _ => write!(f, "<unknown>"),
        }
    }
}

/// 调试信息上下文 — 存储函数级别的调试元数据。
#[derive(Clone, Debug, Default)]
pub struct DebugInfo {
    /// 指令结果 Value → 源码位置的映射。
    pub locations: std::collections::HashMap<Value, SourceLocation>,
    /// 函数名（用于调试输出）。
    pub function_name: Option<String>,
}

impl DebugInfo {
    pub fn new() -> Self {
        Self::default()
    }

    /// 为指令结果附加源码位置。
    pub fn set_location(&mut self, value: Value, loc: SourceLocation) {
        self.locations.insert(value, loc);
    }

    /// 获取指令的源码位置。
    pub fn get_location(&self, value: Value) -> Option<&SourceLocation> {
        self.locations.get(&value)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn source_location_display() {
        let loc = SourceLocation::new("test.rs", 42, 10);
        assert_eq!(format!("{}", loc), "test.rs:42:10");

        let loc2 = SourceLocation::line_only(100);
        assert_eq!(format!("{}", loc2), "line 100");
    }

    #[test]
    fn debug_info_set_get() {
        let mut di = DebugInfo::new();
        di.set_location(Value(1), SourceLocation::line_only(5));
        assert!(di.get_location(Value(1)).is_some());
        assert!(di.get_location(Value(2)).is_none());
    }
}
