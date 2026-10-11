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
///
/// **变体名 = 助记符 = DSL 里的条件名**——三处同名，于是"IR 条件 → 本 ISA 编码"
/// 在谱里读起来是一一对应的：
///
/// ```text
/// IntCC::Sge  ⇔  "sge"  ⇔  [enum.cond] 的 ir = "sge"  ⇔  JCC 的 {cc}
/// ```
///
/// **语义**：`s*` = **有符号**比较，`u*` = **无符号**比较；`lt`/`gt`/`le`/`ge` 对
/// 操作数顺序敏感（`a slt b` ⇔ `b sgt a`），`eq`/`ne` 对称。结果是布尔值，
/// 两个操作数必须**同宽**（混宽要先 `upcast`，见 `TypeId::upcast`）。
///
/// | 变体 | 助记符 | 含义 |
/// | --- | --- | --- |
/// | `Eq` | `eq` | `a == b` |
/// | `Ne` | `ne` | `a != b` |
/// | `Slt` | `slt` | 有符号 `a < b` |
/// | `Sgt` | `sgt` | 有符号 `a > b` |
/// | `Sle` | `sle` | 有符号 `a <= b` |
/// | `Sge` | `sge` | 有符号 `a >= b` |
/// | `Ult` | `ult` | 无符号 `a < b` |
/// | `Ugt` | `ugt` | 无符号 `a > b` |
/// | `Ule` | `ule` | 无符号 `a <= b` |
/// | `Uge` | `uge` | 无符号 `a >= b` |
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum IntCC {
    Eq,
    Ne,
    Slt,
    Sgt,
    Sle,
    Sge,
    Ult,
    Ugt,
    Ule,
    Uge,
}

impl IntCC {
    pub fn mnemonic(&self) -> &'static str {
        match self {
            IntCC::Eq => "eq",
            IntCC::Ne => "ne",
            IntCC::Slt => "slt",
            IntCC::Sgt => "sgt",
            IntCC::Sle => "sle",
            IntCC::Sge => "sge",
            IntCC::Ult => "ult",
            IntCC::Ugt => "ugt",
            IntCC::Ule => "ule",
            IntCC::Uge => "uge",
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
            IntCC::Eq => 1,
            IntCC::Ne => 2,
            IntCC::Slt => 3,
            IntCC::Sle => 4,
            IntCC::Sgt => 5,
            IntCC::Sge => 6,
            IntCC::Ult => 7,
            IntCC::Ule => 8,
            IntCC::Ugt => 9,
            IntCC::Uge => 10,
        }
    }

    /// [`IntCC::code`] 的逆（未知码返回 `None`，不兜底）。
    pub fn from_code(code: u8) -> Option<IntCC> {
        Some(match code {
            1 => IntCC::Eq,
            2 => IntCC::Ne,
            3 => IntCC::Slt,
            4 => IntCC::Sle,
            5 => IntCC::Sgt,
            6 => IntCC::Sge,
            7 => IntCC::Ult,
            8 => IntCC::Ule,
            9 => IntCC::Ugt,
            10 => IntCC::Uge,
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
///
/// **变体名 = 助记符**（`oeq`/`olt`/`ueq`/…）。命名与 NaN 语义的关系：
///
/// - `o*`（**ordered**）：任一操作数是 NaN ⇒ 结果**假**；`Oeq`/`One`/`Olt`/`Ole`/`Ogt`/`Oge`；
/// - `u*`（**unordered**）：任一操作数是 NaN ⇒ 结果**真**；`Ueq`/`Une`/`Ult`/`Ule`/`Ugt`/`Uge`；
/// - `Ord`/`Uno`：只判"**有没有** NaN"，不比大小（`Uno` = "至少一个是 NaN"）；
/// - `False`/`True`：常量结果——IR 里合法（折叠后的残余会出现），编码层照常映射。
///
/// ⚠️ **不是** `o`/`u` 与 `eq`/`lt` 的简单组合，别按名字猜：
/// `One` = "ordered 且不等"（NaN ⇒ 假），而 `Une` = "unordered 或不等"（NaN ⇒ 真）；
/// 同理 `Olt` 与 `Ult` 在 NaN 上相反。
///
/// | 变体 | 助记符 | NaN 时 |
/// | --- | --- | --- |
/// | `Ord` | `ord` | 假（两个都不是 NaN） |
/// | `Uno` | `uno` | 真（任一 NaN） |
/// | `Oeq`/`One`/`Olt`/`Ole`/`Ogt`/`Oge` | `oeq`… | 假 |
/// | `Ueq`/`Une`/`Ult`/`Ule`/`Ugt`/`Uge` | `ueq`… | 真 |
/// | `False`/`True` | `false`/`true` | 常量 |
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum FloatCC {
    Ord,
    Uno,
    Oeq,
    One,
    Olt,
    Ole,
    Ogt,
    Oge,
    False,
    True,
    Ueq,
    Ugt,
    Uge,
    Ult,
    Ule,
    Une,
}

impl FloatCC {
    pub fn mnemonic(&self) -> &'static str {
        match self {
            FloatCC::Ord => "ord",
            FloatCC::Uno => "uno",
            FloatCC::Oeq => "oeq",
            FloatCC::One => "one",
            FloatCC::Olt => "olt",
            FloatCC::Ole => "ole",
            FloatCC::Ogt => "ogt",
            FloatCC::Oge => "oge",
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
            FloatCC::Ord => 1,
            FloatCC::Uno => 2,
            FloatCC::Oeq => 3,
            FloatCC::One => 4,
            FloatCC::Olt => 5,
            FloatCC::Ole => 6,
            FloatCC::Ogt => 7,
            FloatCC::Oge => 8,
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
            1 => FloatCC::Ord,
            2 => FloatCC::Uno,
            3 => FloatCC::Oeq,
            4 => FloatCC::One,
            5 => FloatCC::Olt,
            6 => FloatCC::Ole,
            7 => FloatCC::Ogt,
            8 => FloatCC::Oge,
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
// 标志位条件
// ============================================================

/// **机器标志位条件**（v21 W6+ 甲）：IR 谓词**表达不了**的那一类条件——
/// 溢出（OF/V）、进位/借位（CF/C）、符号（SF/N）、奇偶（PF）。
///
/// 为什么要有它：`IntCC`/`FloatCC` 是**值的比较谓词**（`a < b` 这类），而溢出/进位
/// 是**算术把标志位写成了什么**——LLVM 的 `IntCC` 里同样没有它们（溢出在 IR 里由
/// `sadd.with.overflow` 这类**内建**表达）。但 lowering 到真实机器时，
/// x86 的 `seto`/`setc`/`sets`/`setp` 与 arm64 的 `b.vs`/`b.mi` **必须**用到这些条件，
/// 于是给它们一个类型化的名字——这就是"IR 解释汇编层、赋予新语义"：
/// 汇编层的标志位条件，在 IR 层有了固定、可枚举的语义。
///
/// **变体名 = 助记符**（`o`/`no`/`c`/`nc`/`s`/`ns`/`p`/`np`），与 `IntCC`/`FloatCC`
/// 同一口径：每个 ISA 的表用 `ir = "…"` 把**自己的**名字（x86 `o`、arm64 `vs`）指过来。
///
/// | 变体 | 助记符 | 含义 | x86 名 | A64 名 |
/// | --- | --- | --- | --- | --- |
/// | `O` | `o` | 溢出（V 置位） | `o` | `vs` |
/// | `No` | `no` | 未溢出（V 清零） | `no` | `vc` |
/// | `C` | `c` | 进位/借位（C 置位） | `c`/`b` | `cs`/`hs`（比较语义下另归 `IntCC`） |
/// | `Nc` | `nc` | 无进位（C 清零） | `nc`/`ae` | `cc`/`lo`（同上） |
/// | `S` | `s` | 符号（N 置位，结果为负） | `s` | `mi` |
/// | `Ns` | `ns` | 非负（N 清零） | `ns` | `pl` |
/// | `P` | `p` | 奇偶（PF 置位；浮点比较后即"有 NaN"） | `p`/`pe` | — |
/// | `Np` | `np` | 非奇偶（PF 清零；浮点比较后即"有序"） | `np`/`po` | — |
///
/// ⚠️ 浮点比较后的 PF 有**两种**读法：作为"无序/有序"（`FloatCC::Uno`/`Ord`）或作为
/// 奇偶标志（本枚举）——由谱决定用哪张表，两种都合法（x86 的 FP 比较 lowering 用前者）。
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum FlagCC {
    O,
    No,
    C,
    Nc,
    S,
    Ns,
    P,
    Np,
}

impl FlagCC {
    /// IR 助记符（= 变体名的小写）。
    pub fn mnemonic(&self) -> &'static str {
        match self {
            FlagCC::O => "o",
            FlagCC::No => "no",
            FlagCC::C => "c",
            FlagCC::Nc => "nc",
            FlagCC::S => "s",
            FlagCC::Ns => "ns",
            FlagCC::P => "p",
            FlagCC::Np => "np",
        }
    }

    /// 助记符 → 枚举（不认得 ⇒ `None`）。
    pub fn from_mnemonic(s: &str) -> Option<Self> {
        Some(match s {
            "o" => FlagCC::O,
            "no" => FlagCC::No,
            "c" => FlagCC::C,
            "nc" => FlagCC::Nc,
            "s" => FlagCC::S,
            "ns" => FlagCC::Ns,
            "p" => FlagCC::P,
            "np" => FlagCC::Np,
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
            IntCC::Eq,
            IntCC::Ne,
            IntCC::Slt,
            IntCC::Sgt,
            IntCC::Sle,
            IntCC::Sge,
            IntCC::Ult,
            IntCC::Ugt,
            IntCC::Ule,
            IntCC::Uge,
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
            FloatCC::Ord,
            FloatCC::Uno,
            FloatCC::Oeq,
            FloatCC::One,
            FloatCC::Olt,
            FloatCC::Ole,
            FloatCC::Ogt,
            FloatCC::Oge,
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
