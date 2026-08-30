//! Error types for forge-hir.

use std::fmt;

/// Errors that can occur during HIR construction or lowering.
#[derive(Debug, Clone)]
pub enum HirError {
    /// Referenced an unknown atom (not in registry).
    UnknownAtom { dialect: String, name: String },
    /// Referenced an unknown composite.
    UnknownComposite(String),
    /// Port type mismatch.
    PortTypeMismatch {
        brick_name: String,
        port_name: String,
        expected: String,
        actual: String,
    },
    /// Missing required input port.
    MissingInput {
        brick_name: String,
        port_name: String,
    },
    /// Missing required region.
    MissingRegion {
        brick_name: String,
        region_name: String,
    },
    /// Block was already terminated.
    AlreadyTerminated(String),
    /// Generic lowering error.
    Lowering(String),
    /// Internal error.
    Internal(String),
    /// 带源位置上下文的前端错误（诊断升级：行/列 + 行文本预览）。
    Located {
        error: Box<HirError>,
        /// 1-based 行号。
        line: usize,
        /// 1-based 列号。
        col: usize,
        /// 该行文本（无换行）。
        line_text: String,
    },
}

impl HirError {
    /// 提取底层错误（剥掉 Located 外壳；嵌套时递归）。
    pub fn underlying(&self) -> &HirError {
        match self {
            HirError::Located { error, .. } => error.underlying(),
            other => other,
        }
    }
}

impl fmt::Display for HirError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            HirError::UnknownAtom { dialect, name } => {
                write!(f, "unknown atom: {}.{}", dialect, name)
            }
            HirError::UnknownComposite(name) => write!(f, "unknown composite: {}", name),
            HirError::PortTypeMismatch {
                brick_name,
                port_name,
                expected,
                actual,
            } => {
                write!(
                    f,
                    "port type mismatch in '{}': port '{}' expects {} but got {}",
                    brick_name, port_name, expected, actual
                )
            }
            HirError::MissingInput {
                brick_name,
                port_name,
            } => {
                write!(
                    f,
                    "missing required input '{}' in brick '{}'",
                    port_name, brick_name
                )
            }
            HirError::MissingRegion {
                brick_name,
                region_name,
            } => {
                write!(
                    f,
                    "missing required region '{}' in brick '{}'",
                    region_name, brick_name
                )
            }
            HirError::AlreadyTerminated(block) => {
                write!(f, "block '{}' is already terminated", block)
            }
            HirError::Lowering(msg) => write!(f, "lowering error: {}", msg),
            HirError::Internal(msg) => write!(f, "internal error: {}", msg),
            HirError::Located {
                error,
                line,
                col,
                line_text,
            } => {
                writeln!(f, "{error} at {line}:{col}")?;
                writeln!(f, "  | {line_text}")?;
                write!(f, "  | {}^", " ".repeat(col.saturating_sub(1)))
            }
        }
    }
}

impl std::error::Error for HirError {}
