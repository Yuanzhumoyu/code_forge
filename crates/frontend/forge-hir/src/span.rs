//! 源位置区间——HIR 库的诊断定位类型。
//!
//! `SourceSpan` 是字节偏移的半开区间 `[start, end)`，语义清晰且可组合
//! （`contains`/`len`/`is_empty`）；行/列换算与行文本提取收敛为结构体
//! 方法（依赖源码文本），`HirCtx` 只存 span，诊断时一次性取用。

/// 源码字节偏移区间（半开 `[start, end)`）。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct SourceSpan {
    /// 起始字节偏移（含）。
    pub start: usize,
    /// 结束字节偏移（不含）。
    pub end: usize,
}

impl SourceSpan {
    /// 构造区间（调用方保证 `start <= end`；`new` 不校验，宽松处理越界）。
    pub fn new(start: usize, end: usize) -> Self {
        Self { start, end }
    }

    /// 区间长度（字节）。
    pub fn len(&self) -> usize {
        self.end.saturating_sub(self.start)
    }

    /// 是否为空区间（`start >= end`）。
    pub fn is_empty(&self) -> bool {
        self.start >= self.end
    }

    /// 是否包含字节偏移 `offset`。
    pub fn contains(&self, offset: usize) -> bool {
        self.start <= offset && offset < self.end
    }

    /// 在源码文本中定位 start：返回 (1-based 行号, 1-based 列号, 行首字节偏移)。
    fn locate(&self, source: &str) -> (usize, usize, usize) {
        let mut line = 1usize;
        let mut line_start = 0usize;
        for (i, b) in source.bytes().enumerate() {
            if i >= self.start {
                break;
            }
            if b == b'\n' {
                line += 1;
                line_start = i + 1;
            }
        }
        let col = self.start - line_start + 1;
        (line, col, line_start)
    }

    /// 换算 1-based (行, 列)——错误消息定位用。
    pub fn line_col(&self, source: &str) -> (usize, usize) {
        let (line, col, _) = self.locate(source);
        (line, col)
    }

    /// 该行文本（无换行；按 start 所在行）。
    pub fn line_text<'a>(&self, source: &'a str) -> &'a str {
        let (_, _, line_start) = self.locate(source);
        source[line_start..].split('\n').next().unwrap_or("")
    }
}

impl From<forge_grammar::Span> for SourceSpan {
    fn from(s: forge_grammar::Span) -> Self {
        Self::new(s.start, s.end)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn span_basics() {
        let s = SourceSpan::new(2, 5);
        assert_eq!(s.len(), 3);
        assert!(!s.is_empty());
        assert!(s.contains(2));
        assert!(s.contains(4));
        assert!(!s.contains(5));
        assert!(SourceSpan::new(4, 4).is_empty());
        assert_eq!(SourceSpan::new(4, 4).len(), 0);
    }

    #[test]
    fn line_col_and_line_text() {
        let source = "int a;\nint b = 42;\nreturn b;\n";
        // 第 2 行 "int b = 42;" 从偏移 7 起；42 占偏移 15..17（15='4'、16='2'）
        let s = SourceSpan::new(15, 17);
        assert_eq!(s.line_col(source), (2, 9));
        assert_eq!(s.line_text(source), "int b = 42;");
        // 第 1 行
        let s1 = SourceSpan::new(0, 6);
        assert_eq!(s1.line_col(source), (1, 1));
        assert_eq!(s1.line_text(source), "int a;");
        // 第 3 行
        let s3 = SourceSpan::new(19, 28);
        assert_eq!(s3.line_col(source), (3, 1));
        assert_eq!(s3.line_text(source), "return b;");
    }

    #[test]
    fn from_grammar_span() {
        let g = forge_grammar::Span::new(3, 7, 0, 2);
        let s = SourceSpan::from(g);
        assert_eq!(s.start, 3);
        assert_eq!(s.end, 7);
    }
}
