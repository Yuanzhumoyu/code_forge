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
        }
    }
}

impl std::error::Error for HirError {}
