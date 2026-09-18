//! 指令选择标签 —— `Instruction.isel_strategy` 的类型（v3 方案 S5：附件强类型化）。
//!
//! # 它是什么
//!
//! 一条指令**可选**的"我是怎么被选出来的"标注：模式匹配/融合阶段命中后，把
//! 目标 ISA 侧的策略名挂到指令上，供后续重写、融合、诊断与统计识别。名字的
//! 语法与语义**全由目标 ISA 数据决定**（例：`"lea_sib:4"` 表示
//! `Iadd(Imul(idx,4), base)` 走 LEA 的 SIB 形式、位移倍率 4）——forge-ir
//! **不解析它、不认识任何具体名字**：这里没有枚举、没有白名单、没有字符或
//! 长度限制，也没有"已知标签"常量表。
//!
//! # 为什么不是 `&'static str`（2026-09-15 之前）
//!
//! 1. **`'static` 逼生产者泄漏**：名字来自运行期数据（TOML/匹配表）时必须
//!    `Box::leak` 才能变成 `&'static str`——历史上确实这么干过
//!    （`docs/archive/forge-ir/code-quality-audit.md` 记录过那次泄漏修复）。
//! 2. **裸字符串让错配静默**：手写标签 `"lea_sib"` 与 DSL 侧
//!    `"lea-merge-iadd-imul-4"` 是两个不同字面量，比较为假时编译期毫无提示
//!    （审计记录的断点正是这两套命名体系）。类型化后要比较就必须构造
//!    `IselStrategy`，"这个名字从哪来"在调用点显式可见；本类型**刻意不实现**
//!    `PartialEq<str>` 与 `Deref<Target = str>`，以免又退回字符串比较。
//! 3. **附件语义无处安放**：`Option<&'static str>` 分不清"没有标签"与"空标签"。
//!
//! # 值语义而非池内句柄
//!
//! 内容用 [`ImmStr`] 承载：≤22 字节内联零分配（`"lea_sib:4"` 走这条）、长名字
//! 走 `Arc<str>` 共享、`Clone` O(1)。**不用 `InternedStr`**（[`crate::StringPool`]
//! 的池内 id）：inline / lto / func_specialize 要把标签从**被调方的 DFG** 搬到
//! **调用方的 DFG**，池内 id 跨池无意义，而 `ImmStr` 自带内容、跨 DFG 安全。
//!
//! **刻意不实现 `Default`**：`None` 已经表示"没有标签"，再给一个空名默认值就是
//! 同一件事的第二份编码（S4-c 对终结符消灭过同类歧义）。
//!
//! # 现状（2026-09-15）
//!
//! 手写 pattern-isel 生产者（`ext/pattern_isel.rs`）已随 ISA-DSL v15 的
//! "删死模块"删除，`[[pattern]]`/`lower_pattern` 侧用的是 pattern 名——两者
//! 对接（统一命名）仍是 backlog #2 的功能开发项（会改变指令序列，需专项验证）。
//! 本次只做**类型化与可见性**：字段私有 + 类型化入口，使未来的生产者无法再
//! 直接塞进一个裸字符串。

use std::fmt;

use crate::util::imm_str::ImmStr;

/// 指令选择标签 —— 带名字的不透明句柄（见模块文档）。
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct IselStrategy(ImmStr);

impl IselStrategy {
    /// 由标签名构造（任意 `&str` / `String` / [`ImmStr`]，**不要求 `'static`**）。
    ///
    /// **空名 panic**（fail-closed）：空字符串不是一种标签，允许它就等于允许
    /// "标签存在但无意义"这个坏状态。
    pub fn new(name: impl Into<ImmStr>) -> Self {
        let name = name.into();
        assert!(!name.is_empty(), "指令选择标签名不能为空");
        Self(name)
    }

    /// 编译期字面量的零拷贝构造（短串内联、长串 `Static` 借用，均不分配、
    /// 不涉及 `Box::leak`）。
    pub const fn from_static(name: &'static str) -> Self {
        assert!(!name.is_empty(), "指令选择标签名不能为空");
        Self(ImmStr::from_static(name))
    }

    /// 标签名。**不透明**：名字里可能内嵌目标侧约定的参数（如 `"lea_sib:4"`），
    /// forge-ir 不解析、原样保留。
    pub fn name(&self) -> &str {
        self.0.as_str()
    }
}

impl fmt::Display for IselStrategy {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.name())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 常用短标签内联（零堆分配）——标签是每条命中指令都要挂的热路径附件。
    #[test]
    fn short_name_is_inline() {
        let s = IselStrategy::new("lea_sib:4");
        assert!(matches!(s.0, ImmStr::Inline(_)), "短标签应内联，零堆分配");
        assert_eq!(s.name(), "lea_sib:4");
        assert!(IselStrategy::from_static("tagged").0.is_inline());
    }

    /// 运行期名字（非 `'static`）可直接构造：不再需要 `Box::leak`。
    #[test]
    fn runtime_name_needs_no_leak() {
        let owned = String::from("lea-merge-iadd-imul-4-with-const"); // 31B > 22B
        let s = IselStrategy::new(owned.clone());
        assert_eq!(s.name(), owned);
        assert!(s.0.is_shared(), ">22B 的运行期名字走 Arc 共享");

        // 短借用同样合法（内容内联，不要求 'static）
        let borrowed: &str = &owned[..8];
        assert_eq!(IselStrategy::new(borrowed).name(), "lea-merg");
    }

    /// 长字面量零拷贝借用：既零分配也不泄漏。
    #[test]
    fn static_literal_is_borrowed() {
        const LONG: &str = "isa-specific-pattern-name-long"; // 30B > 22B
        let s = IselStrategy::from_static(LONG);
        assert!(s.0.is_static(), "字面量应零拷贝借用");
        assert!(std::ptr::eq(s.name(), LONG));
    }

    /// 空名 fail-closed（`new` 与 `from_static` 两条入口一致）。
    #[test]
    #[should_panic(expected = "指令选择标签名不能为空")]
    fn empty_name_panics() {
        let _ = IselStrategy::new("");
    }

    #[test]
    #[should_panic(expected = "指令选择标签名不能为空")]
    fn empty_static_name_panics() {
        let _ = IselStrategy::from_static("");
    }

    /// 值语义：相等按**内容**，跨 DFG/跨函数复制后仍相等（池内 id 做不到）。
    #[test]
    fn value_semantics_across_dfgs() {
        let a = IselStrategy::new("lea_sib:4");
        let b = IselStrategy::new(String::from("lea_sib:4"));
        assert_eq!(a, b);
        assert_eq!(a.clone(), b);
        assert_ne!(a, IselStrategy::new("lea_sib:8"), "参数不同即不同标签");
        assert_eq!(format!("{}", a), "lea_sib:4");
        assert_eq!(format!("{:?}", a), "IselStrategy(\"lea_sib:4\")");
    }
}
