//! 指令属性位掩码。

use bitflags::bitflags;

bitflags! {
    /// 指令属性 — 位掩码，影响优化 pass 的行为。
    /// 第二十五轮:u16→u32(16 位已占满致 INALLOCA/INBOUNDS 撞 bit 13)。
    #[derive(Clone, Copy, Debug, PartialEq, Eq)]
    pub struct InstFlags: u32 {
        const NONE          = 0;

        /// 指令可能产生未定义行为 (如 udiv by zero / OOB load)。
        const MAY_UB        = 1 << 0;

        /// 指令有副作用 (不可被 DCE 删除，不可被重排)。
        const SIDE_EFFECT   = 1 << 1;

        /// 内存操作是原子的。
        const ATOMIC        = 1 << 2;

        /// 尾调用标记 — 此 Call 必须进行尾调用优化 (musttail 语义)。
        /// 尾调用在函数返回位置，复用当前栈帧。
        const TAIL_CALL     = 1 << 3;

        /// 无未定义值 — 操作数保证不含 poison/undef。
        const NOUNDEF       = 1 << 4;

        // Fast-math flags (仅当指令是浮点操作时生效)

        /// No NaN: 操作数不包含 NaN (允许 x == x 优化)。
        const FMF_NNAN      = 1 << 5;

        /// No Inf: 操作数不包含 ±Inf。
        const FMF_NINF      = 1 << 6;

        /// No Signed Zero: 忽略零的符号 (±0 等价于 0)。
        const FMF_NSZ       = 1 << 7;

        /// Allow Reciprocal: 允许 x/y → x*(1/y)。
        const FMF_ARCP      = 1 << 8;

        /// Allow Reassociation: 允许 (a+b)+c → a+(b+c)。
        const FMF_REASSOC   = 1 << 9;

        /// Contract: 允许 FMA 融合 (a*b+c → fma)。
        const FMF_CONTRACT  = 1 << 14;

        /// Approximate Functions: 允许不精确的数学函数近似。
        const FMF_AFN       = 1 << 15;

        /// 所有 fast-math 标志 (最激进的浮点优化)。
        const FMF_FAST      = Self::FMF_NNAN.bits()
                            | Self::FMF_NINF.bits()
                            | Self::FMF_NSZ.bits()
                            | Self::FMF_ARCP.bits()
                            | Self::FMF_REASSOC.bits()
                            | Self::FMF_CONTRACT.bits()
                            | Self::FMF_AFN.bits();

        /// inalloca 前缀（LLVM `alloca inalloca <ty>`；C++ 类成员传参约定）。
        /// 第二十五轮:原 1<<13 与 INBOUNDS 冲突(真实 bug)——换 1<<16 空位。
        const INALLOCA      = 1 << 16;

        // LLVM 算术标志（文本层 round-trip：`add nsw i32 %a, i32 %b`）

        /// `nsw` — no signed wrap（add/sub/mul/shl；无符号回绕即 UB）。
        const NSW          = 1 << 10;

        /// `nuw` — no unsigned wrap（add/sub/mul/shl）。
        const NUW          = 1 << 11;

        /// `exact` — 除尽保证（udiv/sdiv/lshr/ashr；余数非零即 UB）。
        const EXACT        = 1 << 12;

        /// `inbounds` — getelementptr 不越界（指针运算保证；越界即 poison）。
        const INBOUNDS     = 1 << 13;
    }
}

impl Default for InstFlags {
    fn default() -> Self {
        InstFlags::NONE
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_fmf_fast_contains_all_flags() {
        assert!(InstFlags::FMF_FAST.contains(InstFlags::FMF_NNAN));
        assert!(InstFlags::FMF_FAST.contains(InstFlags::FMF_NINF));
        assert!(InstFlags::FMF_FAST.contains(InstFlags::FMF_NSZ));
        assert!(InstFlags::FMF_FAST.contains(InstFlags::FMF_ARCP));
        assert!(InstFlags::FMF_FAST.contains(InstFlags::FMF_REASSOC));
    }

    #[test]
    fn test_flags_no_overlap_may_ub_vs_side_effect() {
        // System flags don't overlap with optimization hints
        assert!(!InstFlags::MAY_UB.intersects(InstFlags::FMF_FAST));
        assert!(!InstFlags::SIDE_EFFECT.intersects(InstFlags::FMF_FAST));
        assert!(!InstFlags::ATOMIC.intersects(InstFlags::FMF_FAST));
    }

    #[test]
    fn test_default_is_none() {
        assert_eq!(InstFlags::default(), InstFlags::NONE);
    }

    #[test]
    fn test_none_is_empty() {
        assert!(InstFlags::NONE.is_empty());
        assert!(!InstFlags::MAY_UB.is_empty());
    }

    #[test]
    fn test_flag_composition() {
        let flags = InstFlags::MAY_UB | InstFlags::NOUNDEF;
        assert!(flags.contains(InstFlags::MAY_UB));
        assert!(flags.contains(InstFlags::NOUNDEF));
        assert!(!flags.contains(InstFlags::SIDE_EFFECT));
    }
}
