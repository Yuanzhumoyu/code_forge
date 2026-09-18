//! 字符串 interning — 去重字符串存储，InternedStr 是 Copy 句柄。

use std::collections::HashMap;
use std::fmt;

use crate::ImmStr;

/// 字符串池 — 按内容去重，返回轻量级句柄。
#[derive(Clone, Debug, Default)]
pub struct StringPool {
    /// InternedStr(u32) → String
    pool: Vec<ImmStr>,
    /// String → InternedStr
    dedup: HashMap<ImmStr, InternedStr>,
}

impl StringPool {
    pub fn new() -> Self {
        Self::default()
    }

    /// 插入字符串，返回其 InternedStr 句柄。已存在则返回已有句柄。
    pub fn intern(&mut self, s: impl Into<ImmStr>) -> InternedStr {
        let s = s.into();
        if let Some(&id) = self.dedup.get(&s) {
            return id;
        }
        let id = InternedStr(self.pool.len() as u32);
        self.dedup.insert(s.clone(), id);
        self.pool.push(s); // single allocation, clone into dedup
        id
    }

    /// 按句柄查找字符串。
    pub fn lookup(&self, id: InternedStr) -> &str {
        &self.pool[id.0 as usize]
    }

    /// 字符串数量。
    pub fn len(&self) -> usize {
        self.pool.len()
    }

    /// 是否为空。
    pub fn is_empty(&self) -> bool {
        self.pool.is_empty()
    }
}

// ============================================================
// InternedStr 定义 (放在这里以便与 StringPool 共存)
// ============================================================

/// Interned 字符串句柄。
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Default)]
pub struct InternedStr(pub u32);

impl fmt::Debug for InternedStr {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "InternedStr({})", self.0)
    }
}

impl fmt::Display for InternedStr {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "str{}", self.0)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_intern_dedup() {
        let mut pool = StringPool::new();
        let a = pool.intern("hello");
        let b = pool.intern("hello");
        assert_eq!(a, b);
        assert_eq!(pool.len(), 1);
        assert_eq!(pool.lookup(a), "hello");
    }

    #[test]
    fn test_intern_distinct() {
        let mut pool = StringPool::new();
        let a = pool.intern("foo");
        let b = pool.intern("bar");
        assert_ne!(a, b);
        assert_eq!(pool.len(), 2);
        assert_eq!(pool.lookup(a), "foo");
        assert_eq!(pool.lookup(b), "bar");
    }
}
