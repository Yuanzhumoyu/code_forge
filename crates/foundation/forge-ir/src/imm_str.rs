//! 不可变字符串 — SSO 栈上内联 + 静态借用 + Arc 堆上共享。
//!
//! `ImmStr` 是一个自包含的不可变字符串值类型（设计文档见 `docs/imm_str.md`）：
//!
//! - [`ImmStr::Inline`]：≤ 22 字节的短字符串内联在栈上（零堆分配）；
//! - [`ImmStr::Static`]：编译期字面量的零拷贝借用（`&'static str`）；
//! - [`ImmStr::Shared`]：堆上 `Arc<str>` 引用计数共享（Clone 仅 refcount +1）。
//!
//! `Clone` 恒为 O(1)：Inline 是固定 23 字节 memcpy，Static 是胖指针拷贝，
//! Shared 是原子引用计数递增——任何路径都不深拷贝字符串内容。
//! 类型为 `Send + Sync`（`Arc<str>` 与 `&'static str` 均满足）。
//! 64 位下 `size_of::<ImmStr>() == 24`，与 `String` 同尺寸。
//!
//! 实现 `Borrow<str>`，故 `HashMap<ImmStr, V>` 可直接以 `&str` 无克隆查询。

use std::borrow::Cow;
use std::fmt;
use std::hash::{Hash, Hasher};
use std::ops::Deref;
use std::sync::Arc;

/// 栈上内联的最大字节数（22 = 24 字节枚举布局下三变体的最优解）。
pub const INLINE_CAP: usize = 22;

/// 不可变字符串 — SSO 栈上内联 + 静态借用 + Arc 堆上共享。
#[derive(Clone)]
#[repr(u8)]
pub enum ImmStr {
    /// ≤ 22 字节字符串内联在栈上，零堆分配。
    Inline(InlineStr),
    /// 编译期字面量的零拷贝借用（'static）。
    Static(&'static str),
    /// 堆上引用计数共享（`Arc<str>`）。
    Shared(Arc<str>),
}

/// 栈上内联存储：22 字节内容 + 1 字节长度，共 23 字节、无 padding。
#[derive(Clone, Copy, PartialEq, Eq, Hash)]
#[repr(C)]
pub struct InlineStr {
    buf: [u8; INLINE_CAP],
    len: u8,
}

impl InlineStr {
    /// 从 `&str` 构造内联存储（调用方须保证 `s.len() <= INLINE_CAP`）。
    #[inline]
    const fn new(s: &str) -> Self {
        debug_assert!(s.len() <= INLINE_CAP);
        let bytes = s.as_bytes();
        let mut buf = [0u8; INLINE_CAP];
        let mut i = 0;
        while i < bytes.len() {
            buf[i] = bytes[i];
            i += 1;
        }
        InlineStr {
            buf,
            len: bytes.len() as u8,
        }
    }

    /// 以 `&str` 视图访问内联内容。
    #[inline]
    pub fn as_str(&self) -> &str {
        // SAFETY: buf[..len] 只经 InlineStr::new 从合法 &str 字节拷入，恒为 UTF-8。
        unsafe { std::str::from_utf8_unchecked(&self.buf[..self.len as usize]) }
    }
}

impl ImmStr {
    /// 从 `'static` 借用零拷贝构造。const 友好：短串（≤22 B）内联进栈，
    /// 长串零拷贝借用（`Static`），任何路径都不产生堆分配。
    #[inline]
    pub const fn from_static(s: &'static str) -> Self {
        if s.len() <= INLINE_CAP {
            ImmStr::Inline(InlineStr::new(s))
        } else {
            ImmStr::Static(s)
        }
    }

    /// 以 `&str` 视图访问内容。
    #[inline]
    pub fn as_str(&self) -> &str {
        match self {
            ImmStr::Inline(s) => s.as_str(),
            ImmStr::Static(s) => s,
            ImmStr::Shared(s) => s,
        }
    }

    /// 字节长度。
    #[inline]
    pub fn len(&self) -> usize {
        self.as_str().len()
    }

    /// 是否为空串。
    #[inline]
    pub fn is_empty(&self) -> bool {
        self.as_str().is_empty()
    }

    /// 是否落在栈上内联变体。
    #[inline]
    pub fn is_inline(&self) -> bool {
        matches!(self, ImmStr::Inline(_))
    }

    /// 是否为静态借用变体（零拷贝，不持有堆内存）。
    #[inline]
    pub fn is_static(&self) -> bool {
        matches!(self, ImmStr::Static(_))
    }

    /// 是否为 `Arc<str>` 堆上共享变体。
    #[inline]
    pub fn is_shared(&self) -> bool {
        matches!(self, ImmStr::Shared(_))
    }
}

// ============================================================
// 构造入口
// ============================================================

impl From<&str> for ImmStr {
    /// 短串（≤22 B）内联进栈，零分配；长串拷贝进 `Arc<str>`（借用无法
    /// 脱离原生命周期）。编译期字面量想零拷贝借用请用 [`ImmStr::from_static`]
    /// 或 `From<Cow<'static, str>>` 的 `Borrowed` 路径。
    #[inline]
    fn from(s: &str) -> Self {
        if s.len() <= INLINE_CAP {
            ImmStr::Inline(InlineStr::new(s))
        } else {
            ImmStr::Shared(Arc::from(s))
        }
    }
}

impl From<String> for ImmStr {
    /// 短串搬移字节进栈；长串经 `Arc::from(String)` 零拷贝 move（不重分配内容）。
    #[inline]
    fn from(s: String) -> Self {
        if s.len() <= INLINE_CAP {
            ImmStr::Inline(InlineStr::new(&s))
        } else {
            ImmStr::Shared(Arc::from(s))
        }
    }
}

impl From<Cow<'static, str>> for ImmStr {
    /// `Borrowed` → 静态借用（短串内联，长串零拷贝 `Static`）；
    /// `Owned` → 零拷贝 move 进 Arc。
    #[inline]
    fn from(c: Cow<'static, str>) -> Self {
        match c {
            Cow::Borrowed(s) => ImmStr::from_static(s),
            Cow::Owned(s) => ImmStr::from(s),
        }
    }
}

impl From<Arc<str>> for ImmStr {
    /// 长串直接共享该 Arc（零拷贝）；短串拷贝出内容后释放 Arc，
    /// 以维持"短串恒 Inline"不变量（Arc 可能被共享，无法 move 出 buffer）。
    #[inline]
    fn from(s: Arc<str>) -> Self {
        if s.len() <= INLINE_CAP {
            ImmStr::Inline(InlineStr::new(&s))
        } else {
            ImmStr::Shared(s)
        }
    }
}

impl From<ImmStr> for String {
    /// 按需深拷贝（如错误消息拼接）。
    #[inline]
    fn from(s: ImmStr) -> String {
        s.as_str().to_string()
    }
}

// ============================================================
// 比较 / 哈希
// ============================================================

impl PartialEq for ImmStr {
    /// 按内容比较；`Shared` 同指针走 `Arc::ptr_eq` 快速路径。
    #[inline]
    fn eq(&self, other: &Self) -> bool {
        match (self, other) {
            (ImmStr::Shared(a), ImmStr::Shared(b)) => Arc::ptr_eq(a, b) || a.as_ref() == b.as_ref(),
            (ImmStr::Static(a), ImmStr::Static(b)) => std::ptr::eq(*a, *b) || a == b,
            _ => self.as_str() == other.as_str(),
        }
    }
}

impl Eq for ImmStr {}

impl PartialOrd for ImmStr {
    #[inline]
    fn partial_cmp(&self, other: &Self) -> Option<std::cmp::Ordering> {
        Some(self.cmp(other))
    }
}

impl Ord for ImmStr {
    /// 按内容字典序（与 `str::cmp` 一致）。
    #[inline]
    fn cmp(&self, other: &Self) -> std::cmp::Ordering {
        self.as_str().cmp(other.as_str())
    }
}

impl Hash for ImmStr {
    /// 按内容哈希，与 `Eq`（内容相等）保持一致。
    #[inline]
    fn hash<H: Hasher>(&self, state: &mut H) {
        self.as_str().hash(state);
    }
}

// ============================================================
// 借用 / 转换 / 格式化
// ============================================================

impl std::borrow::Borrow<str> for ImmStr {
    /// 使 `HashMap<ImmStr, V>` 支持 `get(name: &str)` 无克隆查询。
    #[inline]
    fn borrow(&self) -> &str {
        self.as_str()
    }
}

impl AsRef<str> for ImmStr {
    #[inline]
    fn as_ref(&self) -> &str {
        self.as_str()
    }
}

impl Deref for ImmStr {
    type Target = str;

    #[inline]
    fn deref(&self) -> &str {
        self.as_str()
    }
}

impl fmt::Display for ImmStr {
    #[inline]
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

impl fmt::Debug for ImmStr {
    #[inline]
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        fmt::Debug::fmt(&self.as_str(), f)
    }
}

impl Default for ImmStr {
    /// 空串（Inline 空缓冲）。
    #[inline]
    fn default() -> Self {
        ImmStr::Inline(InlineStr {
            buf: [0u8; INLINE_CAP],
            len: 0,
        })
    }
}

// ============================================================
// 测试
// ============================================================

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashMap;

    /// 布局断言：64 位下枚举 24 字节（与 String 同尺寸），内联存储 23 字节。
    #[test]
    fn test_layout_sizes() {
        assert_eq!(std::mem::size_of::<InlineStr>(), 23);
        assert_eq!(std::mem::size_of::<ImmStr>(), 24);
        assert_eq!(std::mem::align_of::<ImmStr>(), 8);
    }

    /// 22/23 字节边界：≤22 内联，>22 转 Static/Shared。
    #[test]
    fn test_inline_capacity_boundary() {
        let short: &'static str = "a".repeat(22).leak();
        assert_eq!(short.len(), 22);
        assert!(ImmStr::from(short).is_inline());

        let long: &'static str = "b".repeat(23).leak();
        assert_eq!(long.len(), 23);
        let s = ImmStr::from_static(long);
        assert!(s.is_static()); // 'static 长串 → 零拷贝借用
        assert_eq!(s.as_str(), long);

        // 通用 From<&str> 无法区分 'static，长串拷贝进 Arc
        let s2 = ImmStr::from(long);
        assert!(s2.is_shared());
        assert_eq!(s2.as_str(), long);

        let long_owned = "c".repeat(23);
        let s = ImmStr::from(long_owned);
        assert!(s.is_shared()); // String 长串 → Arc 共享
        assert_eq!(s.as_str(), "c".repeat(23));
    }

    /// Clone 共享：长串克隆后共享同一 Arc 堆 buffer（无内容拷贝）。
    #[test]
    fn test_clone_shares_arc() {
        let s: ImmStr = "this is a sufficiently long string".into();
        assert!(s.is_shared());
        let c = s.clone();
        match (&s, &c) {
            (ImmStr::Shared(a), ImmStr::Shared(b)) => assert!(Arc::ptr_eq(a, b)),
            _ => panic!("expected Shared variants"),
        }
    }

    /// Clone 零分配（Inline memcpy / Static 指针 / Arc refcount）。
    #[test]
    fn test_clone_inline_and_static() {
        let inline: ImmStr = "short".into();
        assert!(inline.is_inline());
        assert_eq!(inline.clone(), inline);

        let stat: ImmStr = ImmStr::from_static("a very long static string literal");
        assert!(stat.is_static());
        assert_eq!(stat.clone(), stat);
    }

    /// 静态借用零拷贝：as_str 与源 'static 串为同一指针。
    #[test]
    fn test_static_zero_copy() {
        const S: &str = "somewhat longer static string"; // 29 B > 22 B
        let s = ImmStr::from_static(S);
        assert!(s.is_static());
        assert!(std::ptr::eq(s.as_str(), S));
    }

    /// From<String> 长串零拷贝 move：内容正确且为 Shared。
    #[test]
    fn test_from_string_long_shared() {
        let s = String::from("string that exceeds the inline capacity of 22 bytes");
        let imm: ImmStr = s.into();
        assert!(imm.is_shared());
        assert_eq!(
            imm.as_str(),
            "string that exceeds the inline capacity of 22 bytes"
        );
    }

    /// From<Cow>：Borrowed → Static/Inline，Owned → Shared/Inline。
    #[test]
    fn test_from_cow() {
        let borrowed: Cow<'static, str> =
            Cow::Borrowed("borrowed static text longer than 22 chars");
        assert!(ImmStr::from(borrowed).is_static());

        let owned: Cow<'static, str> = Cow::Owned(String::from(
            "owned dynamic text longer than twenty-two bytes",
        ));
        assert!(ImmStr::from(owned).is_shared());

        let short: Cow<'static, str> = Cow::Borrowed("hi");
        assert!(ImmStr::from(short).is_inline());
    }

    /// Eq/Ord/Hash 一致性：内容相同则相等、同序、同哈希（跨变体）。
    #[test]
    fn test_eq_ord_hash_consistency() {
        use std::hash::DefaultHasher;

        let a: ImmStr = "same content".into(); // inline
        let b: ImmStr = ImmStr::from_static("same content"); // static
        let c: ImmStr = String::from("same content").into(); // inline（短）
        assert_eq!(a, b);
        assert_eq!(a, c);
        assert!(a <= b && b <= c);

        let ha = {
            let mut h = DefaultHasher::new();
            a.hash(&mut h);
            h.finish()
        };
        let hb = {
            let mut h = DefaultHasher::new();
            b.hash(&mut h);
            h.finish()
        };
        assert_eq!(ha, hb);

        // 不等内容（长串 Shared vs 短串 Inline）
        let long: ImmStr = String::from("a longer string for sharing").into();
        assert!(long < a); // 'a' < 's'，内容比较与变体无关
    }

    /// HashMap<ImmStr, V> 以 &str 无克隆查询（Borrow<str>）。
    #[test]
    fn test_hashmap_borrow_query() {
        let mut map: HashMap<ImmStr, i32> = HashMap::new();
        map.insert("func_main".into(), 1);
        map.insert(ImmStr::from_static("glob_var"), 2);
        map.insert(String::from("alias_target").into(), 3);

        assert_eq!(map.get("func_main"), Some(&1));
        assert_eq!(map.get("glob_var"), Some(&2));
        assert_eq!(map.get("alias_target"), Some(&3));
        assert_eq!(map.get("missing"), None);
    }

    /// Default / Display / Debug / AsRef / Deref。
    #[test]
    fn test_misc_traits() {
        let d = ImmStr::default();
        assert!(d.is_inline());
        assert!(d.is_empty());

        let s: ImmStr = "hello".into();
        assert_eq!(s.len(), 5);
        assert_eq!(format!("{}", s), "hello");
        assert_eq!(format!("{:?}", s), "\"hello\"");
        assert_eq!(AsRef::<str>::as_ref(&s), "hello");
        assert_eq!(&*s, "hello"); // Deref
        assert!(s.starts_with("he")); // Deref 后 str 方法可用

        let owned: String = s.into();
        assert_eq!(owned, "hello");
    }

    /// 空串与全零缓冲安全性。
    #[test]
    fn test_empty_string() {
        let s: ImmStr = "".into();
        assert!(s.is_inline());
        assert!(s.is_empty());
        assert_eq!(s.as_str(), "");
    }
}
