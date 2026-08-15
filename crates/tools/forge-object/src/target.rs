//! 目标平台配置 — 表示目标三元组和平台属性。
//!
//! 需要启用 `object-file` feature。

use forge_ir::IrError;
use target_lexicon::Triple;

/// 目标平台配置。
///
/// 封装目标三元组信息，用于确定对象文件格式、指针宽度和字节序。
#[derive(Clone, Debug)]
pub struct TargetConfig {
    /// 目标三元组（例如 `x86_64-unknown-linux-gnu`）。
    pub triple: Triple,
}

impl TargetConfig {
    /// 获取宿主平台的目标配置。
    pub fn host() -> Result<Self, IrError> {
        let triple = Triple::host();
        Ok(Self { triple })
    }

    /// 从三元组字符串解析目标配置。
    ///
    /// # Example
    /// ```ignore
    /// let config = TargetConfig::from_triple("x86_64-unknown-linux-gnu")?;
    /// ```
    pub fn from_triple(triple_str: &str) -> Result<Self, IrError> {
        let triple = triple_str
            .parse::<Triple>()
            .map_err(|e| IrError::Unsupported(format!("invalid triple '{triple_str}': {e}")))?;
        Ok(Self { triple })
    }

    /// 返回指针对齐宽度（字节）。
    pub fn pointer_width(&self) -> u8 {
        self.triple.pointer_width().map(|w| w.bytes()).unwrap_or(8)
    }

    /// 返回本地字节序。
    pub fn endianness(&self) -> target_lexicon::Endianness {
        self.triple
            .endianness()
            .unwrap_or(target_lexicon::Endianness::Little)
    }
}

impl Default for TargetConfig {
    fn default() -> Self {
        Self::host().unwrap_or_else(|_| {
            // 回退到 x86_64 Linux
            TargetConfig {
                triple: "x86_64-unknown-linux-gnu".parse().unwrap(),
            }
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_triple() {
        let config = TargetConfig::from_triple("x86_64-unknown-linux-gnu").unwrap();
        assert_eq!(config.pointer_width(), 8);
    }

    #[test]
    fn test_parse_aarch64() {
        let config = TargetConfig::from_triple("aarch64-unknown-linux-gnu").unwrap();
        assert_eq!(config.pointer_width(), 8);
    }

    #[test]
    fn test_invalid_triple() {
        assert!(TargetConfig::from_triple("not-a-valid-triple").is_err());
    }

    #[test]
    fn test_host() {
        let config = TargetConfig::host().unwrap();
        // 宿主三元组应该能成功解析
        assert!(!config.triple.to_string().is_empty());
    }
}
