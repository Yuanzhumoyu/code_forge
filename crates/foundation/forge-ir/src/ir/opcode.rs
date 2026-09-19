//! IR 操作码定义。
//!
//! **指令清单与派生属性的单一事实源 = `ops.toml`**（crate 根目录）。`build.rs`
//! 读它生成 `$OUT_DIR/opcode_gen.rs`，本文件 `include!` 进来：`Opcode` 枚举、
//! `Opcode::ALL`/`INFOS`、`name`/`from_name`/`mnemonic`/`from_mnemonic`/
//! `result_count`/`expected_operand_count`/`may_ub`/`has_side_effect` 全部由它投影。
//! **新增一个 opcode = 在 `ops.toml` 加一行**（生成物与枚举同源，不可能漂移）。
//!
//! 类型信息、常量、块引用等通过 `Immediate` 传递；`Icmp`/`Fcmp` 的比较条件同样走
//! immediate 通道（`Immediate::IntCC`/`FloatCC`，`Opcode::cond_kind()` 声明该契约），
//! 数值表示由 [`IntCC::code`]/[`FloatCC::code`] 提供（ISA TOML 的 `cond` 谓词与宿主
//! lowering 共用同一份映射）。
//!
//! 本文件手写保留的只有**非 opcode** 的枚举：比较条件（`IntCC`/`FloatCC`）、
//! 内存序（`Ordering`）与原子操作码（`AtomicRmwOp`）。

include!(concat!(env!("OUT_DIR"), "/opcode_gen.rs"));

// ============================================================
// 比较条件
// ============================================================

/// 整数比较条件（LLVM `icmp` 的 10 个谓词）。
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum IntCC {
    Equal,
    NotEqual,
    SignedLessThan,
    SignedGreaterThan,
    SignedLessThanOrEqual,
    SignedGreaterThanOrEqual,
    UnsignedLessThan,
    UnsignedGreaterThan,
    UnsignedLessThanOrEqual,
    UnsignedGreaterThanOrEqual,
}

impl IntCC {
    pub fn mnemonic(&self) -> &'static str {
        match self {
            IntCC::Equal => "eq",
            IntCC::NotEqual => "ne",
            IntCC::SignedLessThan => "slt",
            IntCC::SignedGreaterThan => "sgt",
            IntCC::SignedLessThanOrEqual => "sle",
            IntCC::SignedGreaterThanOrEqual => "sge",
            IntCC::UnsignedLessThan => "ult",
            IntCC::UnsignedGreaterThan => "ugt",
            IntCC::UnsignedLessThanOrEqual => "ule",
            IntCC::UnsignedGreaterThanOrEqual => "uge",
        }
    }

    /// 规范条件码（**1..=10**，与 ISA TOML 的 `when = { eq = ["cond", N] }` 契约一致）。
    ///
    /// 这是条件的**唯一**数字表示：宿主 lowering 把 `Immediate::IntCC(cc)` 映射成
    /// `cc.code()` 交给 [`LowerCtx::current_immediates`]，ISA 规则按此数值分派。
    /// 历史上这段映射是 forge-dsl 生成代码里的 `icmp_id`（每个 ISA 模块各生成一份），
    /// v3 S1 收敛到本 crate（生成物与宿主共用同一份，不再重复）。
    ///
    /// [`LowerCtx::current_immediates`]: 见 `forge-codegen` 的同名文档
    pub fn code(&self) -> u8 {
        match self {
            IntCC::Equal => 1,
            IntCC::NotEqual => 2,
            IntCC::SignedLessThan => 3,
            IntCC::SignedLessThanOrEqual => 4,
            IntCC::SignedGreaterThan => 5,
            IntCC::SignedGreaterThanOrEqual => 6,
            IntCC::UnsignedLessThan => 7,
            IntCC::UnsignedLessThanOrEqual => 8,
            IntCC::UnsignedGreaterThan => 9,
            IntCC::UnsignedGreaterThanOrEqual => 10,
        }
    }

    /// [`IntCC::code`] 的逆（未知码返回 `None`，不兜底）。
    pub fn from_code(code: u8) -> Option<IntCC> {
        Some(match code {
            1 => IntCC::Equal,
            2 => IntCC::NotEqual,
            3 => IntCC::SignedLessThan,
            4 => IntCC::SignedLessThanOrEqual,
            5 => IntCC::SignedGreaterThan,
            6 => IntCC::SignedGreaterThanOrEqual,
            7 => IntCC::UnsignedLessThan,
            8 => IntCC::UnsignedLessThanOrEqual,
            9 => IntCC::UnsignedGreaterThan,
            10 => IntCC::UnsignedGreaterThanOrEqual,
            _ => return None,
        })
    }
}

/// 整数条件的**全部规范名**（与 [`IntCC::code`] 一一对应，顺序 = 码序）。
///
/// 这是"IR 条件"这一侧的唯一名单：ISA 数据（`[conventions.cond]` 的 `ir` 字段）
/// 用它作为键空间，生成器用它校验"每个条件都映射到了本 ISA 的编码"。
pub const INTCC_NAMES: [&str; 10] = [
    "eq", "ne", "slt", "sle", "sgt", "sge", "ult", "ule", "ugt", "uge",
];

/// IR 整数条件码（[`IntCC::code`]）→ 规范名（[`IntCC::mnemonic`]）；未知码 → `None`。
///
/// 生成器用它把 `LowerCtx::current_immediates[0]` 里的条件码折成
/// `[conventions.cond]` 的键——这样**生成的代码里不出现 `IntCC`**：
/// "IR 条件 → 本 ISA 编码"的映射完全由 ISA 数据给出（见
/// `docs/reference/isa-dsl.md` 的"条件码"一节）。
pub fn intcc_name(code: u8) -> Option<&'static str> {
    IntCC::from_code(code).map(|c| c.mnemonic())
}

/// 浮点比较条件（LLVM `fcmp` 的 16 个谓词）。
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum FloatCC {
    /// Ordered (no NaN operands).
    Ordered,
    /// Unordered (at least one NaN operand).
    Unordered,
    /// Ordered and equal (`oeq`).
    Equal,
    /// Ordered and not equal (`one`).
    NotEqual,
    /// Ordered and less than (`olt`).
    LessThan,
    /// Ordered and less than or equal (`ole`).
    LessThanOrEqual,
    /// Ordered and greater than (`ogt`).
    GreaterThan,
    /// Ordered and greater than or equal (`oge`).
    GreaterThanOrEqual,
    /// Always false.
    False,
    /// Always true.
    True,
    /// Unordered or equal (`ueq`).
    Ueq,
    /// Unordered or greater than (`ugt`).
    Ugt,
    /// Unordered or greater than or equal (`uge`).
    Uge,
    /// Unordered or less than (`ult`).
    Ult,
    /// Unordered or less than or equal (`ule`).
    Ule,
    /// Unordered or not equal (`une`).
    Une,
}

impl FloatCC {
    pub fn mnemonic(&self) -> &'static str {
        match self {
            FloatCC::Ordered => "ord",
            FloatCC::Unordered => "uno",
            FloatCC::Equal => "oeq",
            FloatCC::NotEqual => "one",
            FloatCC::LessThan => "olt",
            FloatCC::LessThanOrEqual => "ole",
            FloatCC::GreaterThan => "ogt",
            FloatCC::GreaterThanOrEqual => "oge",
            FloatCC::False => "false",
            FloatCC::True => "true",
            FloatCC::Ueq => "ueq",
            FloatCC::Ugt => "ugt",
            FloatCC::Uge => "uge",
            FloatCC::Ult => "ult",
            FloatCC::Ule => "ule",
            FloatCC::Une => "une",
        }
    }

    /// 规范条件码（**1..=16**，与 ISA TOML 的 `when = { eq = ["cond", N] }` 契约一致）。
    ///
    /// 与 [`IntCC::code`] 同理：条件的唯一数字表示，宿主 lowering 用它喂
    /// `LowerCtx.current_immediates`；历史上是 forge-dsl 生成的 `fcmp_id`。
    pub fn code(&self) -> u8 {
        match self {
            FloatCC::Ordered => 1,
            FloatCC::Unordered => 2,
            FloatCC::Equal => 3,
            FloatCC::NotEqual => 4,
            FloatCC::LessThan => 5,
            FloatCC::LessThanOrEqual => 6,
            FloatCC::GreaterThan => 7,
            FloatCC::GreaterThanOrEqual => 8,
            FloatCC::False => 9,
            FloatCC::True => 10,
            FloatCC::Ueq => 11,
            FloatCC::Ugt => 12,
            FloatCC::Uge => 13,
            FloatCC::Ult => 14,
            FloatCC::Ule => 15,
            FloatCC::Une => 16,
        }
    }

    /// [`FloatCC::code`] 的逆（未知码返回 `None`，不兜底）。
    pub fn from_code(code: u8) -> Option<FloatCC> {
        Some(match code {
            1 => FloatCC::Ordered,
            2 => FloatCC::Unordered,
            3 => FloatCC::Equal,
            4 => FloatCC::NotEqual,
            5 => FloatCC::LessThan,
            6 => FloatCC::LessThanOrEqual,
            7 => FloatCC::GreaterThan,
            8 => FloatCC::GreaterThanOrEqual,
            9 => FloatCC::False,
            10 => FloatCC::True,
            11 => FloatCC::Ueq,
            12 => FloatCC::Ugt,
            13 => FloatCC::Uge,
            14 => FloatCC::Ult,
            15 => FloatCC::Ule,
            16 => FloatCC::Une,
            _ => return None,
        })
    }
}

// ============================================================
// 原子操作 / 内存序
// ============================================================

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum Ordering {
    NotAtomic = 0,
    Unordered = 1,
    Monotonic = 2,
    Acquire = 3,
    Release = 4,
    AcquireRelease = 5,
    SequentiallyConsistent = 6,
}

impl Ordering {
    pub fn is_acquire(self) -> bool {
        matches!(
            self,
            Ordering::Acquire | Ordering::AcquireRelease | Ordering::SequentiallyConsistent
        )
    }

    pub fn is_release(self) -> bool {
        matches!(
            self,
            Ordering::Release | Ordering::AcquireRelease | Ordering::SequentiallyConsistent
        )
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum AtomicRmwOp {
    Xchg,
    Add,
    Sub,
    And,
    Nand,
    Or,
    Xor,
    Max,
    Min,
    Umax,
    Umin,
    Fadd,
    Fsub,
}

#[cfg(test)]
mod tests {
    use super::*;

    // Test that all opcodes have non-empty mnemonics
    #[test]
    fn test_all_mnemonics_non_empty() {
        let all = all_opcodes();
        for op in &all {
            let m = op.mnemonic();
            assert!(!m.is_empty(), "opcode {:?} has empty mnemonic", op);
        }
    }

    // Test result_count for key categories
    #[test]
    fn test_result_counts() {
        // 0-result ops
        assert_eq!(Opcode::Store.result_count(), 0);
        assert_eq!(Opcode::Fence.result_count(), 0);
        assert_eq!(Opcode::Nop.result_count(), 0);
        assert_eq!(Opcode::Trap.result_count(), 0);
        // 2-result ops (overflow)
        assert_eq!(Opcode::SaddOverflow.result_count(), 2);
        assert_eq!(Opcode::UaddOverflow.result_count(), 2);
        assert_eq!(Opcode::SmulOverflow.result_count(), 2);
        assert_eq!(Opcode::UmulOverflow.result_count(), 2);
        // 1-result ops (most)
        assert_eq!(Opcode::Iadd.result_count(), 1);
        assert_eq!(Opcode::Load.result_count(), 1);
    }

    // Test may_ub coverage
    #[test]
    fn test_may_ub() {
        assert!(Opcode::Udiv.may_ub());
        assert!(Opcode::Sdiv.may_ub());
        assert!(Opcode::Trap.may_ub());
        assert!(Opcode::SaddOverflow.may_ub());
        assert!(Opcode::SaddSat.may_ub());
        assert!(!Opcode::Iadd.may_ub());
        assert!(!Opcode::Fadd.may_ub());
        assert!(!Opcode::Nop.may_ub());
    }

    // Test has_side_effect
    #[test]
    fn test_side_effect() {
        assert!(Opcode::Store.has_side_effect());
        assert!(Opcode::Call.has_side_effect());
        assert!(Opcode::CallIndirect.has_side_effect());
        assert!(Opcode::Trap.has_side_effect());
        assert!(Opcode::AtomicRmw.has_side_effect());
        assert!(!Opcode::Iadd.has_side_effect());
        assert!(!Opcode::Load.has_side_effect()); // Load alone is not side-effecting
    }

    // Test operand counts for bit manipulation ops
    #[test]
    fn test_bit_op_operand_counts() {
        assert_eq!(Opcode::Clz.expected_operand_count(), 1);
        assert_eq!(Opcode::Rotl.expected_operand_count(), 2);
        assert_eq!(Opcode::Fma.expected_operand_count(), 3);
        assert_eq!(Opcode::Select.expected_operand_count(), 3);
        assert_eq!(Opcode::Trap.expected_operand_count(), 0);
    }

    // Test IntCC coverage
    #[test]
    fn test_intcc_variants() {
        let conds = [
            IntCC::Equal,
            IntCC::NotEqual,
            IntCC::SignedLessThan,
            IntCC::SignedGreaterThan,
            IntCC::SignedLessThanOrEqual,
            IntCC::SignedGreaterThanOrEqual,
            IntCC::UnsignedLessThan,
            IntCC::UnsignedGreaterThan,
            IntCC::UnsignedLessThanOrEqual,
            IntCC::UnsignedGreaterThanOrEqual,
        ];
        for cc in &conds {
            let m = cc.mnemonic();
            assert!(!m.is_empty());
        }
    }

    // Test FloatCC coverage
    #[test]
    fn test_floatcc_variants() {
        let conds = [
            FloatCC::Ordered,
            FloatCC::Unordered,
            FloatCC::Equal,
            FloatCC::NotEqual,
            FloatCC::LessThan,
            FloatCC::LessThanOrEqual,
            FloatCC::GreaterThan,
            FloatCC::GreaterThanOrEqual,
            FloatCC::False,
            FloatCC::True,
            FloatCC::Ueq,
            FloatCC::Ugt,
            FloatCC::Uge,
            FloatCC::Ult,
            FloatCC::Ule,
            FloatCC::Une,
        ];
        for cc in &conds {
            let m = cc.mnemonic();
            assert!(!m.is_empty());
        }
    }

    // Helper: 全部 opcode —— 单一事实源 = `Opcode::ALL`（`ops.toml` 生成）。
    // 历史版本在这里手写第二份清单，漏了 Fptrunc/Fpext/Fptosi/Sitofp/Fptoui/
    // Uitofp/Ptrtoint/Inttoptr/Vsplit/Vconcat 共 10 个变体，"系统性覆盖测试"
    // 因此并不系统（2026-09-14 审计发现）。
    fn all_opcodes() -> Vec<Opcode> {
        Opcode::ALL.to_vec()
    }
}
