//! IR 操作码定义。
//!
//! 操作码描述指令的语义类别。类型信息、常量、块引用等通过
//! `Immediate` 传递，不再嵌入 Opcode 变体中。

// ============================================================
// Opcode
// ============================================================

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Opcode {
    // === 整数算术 (7) ===
    Iadd,
    Isub,
    Imul,
    Udiv,
    Sdiv,
    Urem,
    Srem,

    // === 浮点算术 (7) ===
    Fadd,
    Fsub,
    Fmul,
    Fdiv,
    Fneg,
    Fabs,
    Fsqrt,

    // === 位运算 (7) ===
    Band,
    Bor,
    Bxor,
    Bnot,
    Ishl,
    Ushr,
    Sshr,

    // === 位操作 (6) ===
    /// Count leading zeros
    Clz,
    /// Count trailing zeros
    Ctz,
    /// Population count (Hamming weight)
    Popcnt,
    /// Reverse bit order
    Bitreverse,
    /// Rotate left
    Rotl,
    /// Rotate right
    Rotr,

    // === 整数扩展 (10) ===
    /// Absolute value
    Abs,
    /// Signed minimum
    Smin,
    /// Signed maximum
    Smax,
    /// Unsigned minimum
    Umin,
    /// Unsigned maximum
    Umax,
    /// Signed saturating add
    SaddSat,
    /// Signed saturating sub
    SsubSat,
    /// Unsigned saturating add
    UaddSat,
    /// Unsigned saturating sub
    UsubSat,
    /// Byte swap (endian conversion)
    Bswap,

    // === 浮点扩展 (8) ===
    /// Fused multiply-add: a * b + c
    Fma,
    /// Minimum (IEEE 754-2019 minimumNumber)
    Fmin,
    /// Maximum
    Fmax,
    /// Copy sign bit
    Fcopysign,
    /// Round toward negative infinity
    Ffloor,
    /// Round toward positive infinity
    Fceil,
    /// Round toward zero
    Ftrunc,
    /// Round to nearest, ties to even
    Fround,

    // === 比较 (2) ===
    Icmp {
        cond: IntCC,
    },
    Fcmp {
        cond: FloatCC,
    },

    // === 宽算术 / 溢出检测 (6) ===
    /// Signed add with overflow flag (returns {value, overflow:i1})
    SaddOverflow,
    /// Unsigned add with overflow
    UaddOverflow,
    /// Signed sub with overflow
    SsubOverflow,
    /// Unsigned sub with overflow
    UsubOverflow,
    /// Signed mul with overflow
    SmulOverflow,
    /// Unsigned mul with overflow
    UmulOverflow,

    // === 内存 (2) ===
    Load,
    Store,
    /// 浮点 load（按类型分派到 MOVSD_RM 等——普通 Load 走 GPR，F64 值会错乱）
    Fload,
    /// 浮点 store（按类型分派到 MOVSD_MR 等）
    Fstore,

    // === 常量 (2) ===
    Iconst,
    Fconst,
    Vconst,

    // === 值语义 (2) ===
    /// Produce a poison value of given type (deferred UB)
    Poison,
    /// Produce an undef value (arbitrary bit pattern, each use independent)
    Undef,

    // === 类型转换 (12) ===
    Sextend,
    Uextend,
    Ireduce,
    Fptrunc,
    Fpext,
    Fptosi,
    Sitofp,
    Fptoui,
    Uitofp,
    Ptrtoint,
    Inttoptr,
    Bitcast,

    // === 函数调用 (2) ===
    Call,
    CallIndirect,

    // === 地址 (2) ===
    StackAddr,
    GlobalAddr,

    // === 栈分配 (1) ===
    Alloca,

    // === 地址计算 (1) ===
    GetElementPtr,

    // === 向量/SIMD (11) ===
    Vadd,
    Vsub,
    Vmul,
    Vdiv,
    Vneg,
    Vabs,
    Vextract,
    Vinsert,
    Vbitcast,
    Vbroadcast,
    ShuffleVector,
    /// 宽向量 → 128 位片段提取（V256 → V128；fragment 立即数选片段）。
    Vsplit,
    /// 128 位片段拼接成宽向量（V128×2 → V256）。
    Vconcat,

    // === 陷阱 (1) ===
    /// Trigger a trap (unreachable by definition, e.g. failed bounds check)
    Trap,

    // === 指针操作 (4) ===
    /// Test if pointer is null
    IsNull,
    /// Test if pointer is not null
    IsNotNull,
    /// 指针地址空间转换（LLVM `addrspacecast`；结果类型 = 目标 addrspace 指针）。
    AddrSpaceCast,
    /// 可变参数读取（LLVM `va_arg`；ABI 布局见 P1——文本层存 (Type, ptr) 操作数）。
    VaArg,
    /// 异常着陆垫（LLVM `landingpad`；P1.1 文本层解析，codegen Unsupported）。
    LandingPad,

    // === 原子操作 (3) ===
    AtomicRmw,
    Cmpxchg,
    Fence,

    // === 复合类型操作 (2) ===
    ExtractValue,
    InsertValue,

    // === 其他 (4) ===
    Copy,
    Select,
    Freeze,
    Nop,
}

impl Opcode {
    /// 此操作码的结果值数量。
    pub fn result_count(&self) -> u8 {
        match self {
            // 无结果 (副作用)
            Opcode::Store | Opcode::Fstore | Opcode::Fence | Opcode::Nop | Opcode::Trap => 0,
            // 双结果 (value + overflow flag)
            Opcode::SaddOverflow
            | Opcode::UaddOverflow
            | Opcode::SsubOverflow
            | Opcode::UsubOverflow
            | Opcode::SmulOverflow
            | Opcode::UmulOverflow => 2,
            // 单结果
            _ => 1,
        }
    }

    /// 是否可能有未定义行为。
    pub fn may_ub(&self) -> bool {
        matches!(
            self,
            Opcode::Udiv
                | Opcode::Sdiv
                | Opcode::Urem
                | Opcode::Srem
                | Opcode::Trap
                | Opcode::SaddOverflow
                | Opcode::UaddOverflow
                | Opcode::SsubOverflow
                | Opcode::UsubOverflow
                | Opcode::SmulOverflow
                | Opcode::UmulOverflow
                | Opcode::SaddSat
                | Opcode::SsubSat
                | Opcode::UaddSat
                | Opcode::UsubSat
        )
    }

    /// 是否必然有副作用 (不可被 DCE 删除)。
    pub fn has_side_effect(&self) -> bool {
        matches!(
            self,
            Opcode::Store
                | Opcode::Fstore
                | Opcode::Call
                | Opcode::CallIndirect
                | Opcode::AtomicRmw
                | Opcode::Cmpxchg
                | Opcode::Fence
                | Opcode::VaArg
                | Opcode::Trap
        )
    }

    /// 操作码的简短助记符 (Display 用)。
    pub fn mnemonic(&self) -> &'static str {
        match self {
            Opcode::Iadd => "iadd",
            Opcode::Isub => "isub",
            Opcode::Imul => "imul",
            Opcode::Udiv => "udiv",
            Opcode::Sdiv => "sdiv",
            Opcode::Urem => "urem",
            Opcode::Srem => "srem",
            Opcode::Fadd => "fadd",
            Opcode::Fsub => "fsub",
            Opcode::Fmul => "fmul",
            Opcode::Fdiv => "fdiv",
            Opcode::Fneg => "fneg",
            Opcode::Fabs => "fabs",
            Opcode::Fsqrt => "fsqrt",
            Opcode::Band => "and",
            Opcode::Bor => "or",
            Opcode::Bxor => "xor",
            Opcode::Bnot => "not",
            Opcode::Ishl => "shl",
            Opcode::Ushr => "ushr",
            Opcode::Sshr => "sshr",
            // Bit manipulation
            Opcode::Clz => "clz",
            Opcode::Ctz => "ctz",
            Opcode::Popcnt => "popcnt",
            Opcode::Bitreverse => "bitreverse",
            Opcode::Rotl => "rotl",
            Opcode::Rotr => "rotr",
            // Integer extended
            Opcode::Abs => "abs",
            Opcode::Smin => "smin",
            Opcode::Smax => "smax",
            Opcode::Umin => "umin",
            Opcode::Umax => "umax",
            Opcode::SaddSat => "sadd_sat",
            Opcode::SsubSat => "ssub_sat",
            Opcode::UaddSat => "uadd_sat",
            Opcode::UsubSat => "usub_sat",
            Opcode::Bswap => "bswap",
            // Float extended
            Opcode::Fma => "fma",
            Opcode::Fmin => "fmin",
            Opcode::Fmax => "fmax",
            Opcode::Fcopysign => "fcopysign",
            Opcode::Ffloor => "ffloor",
            Opcode::Fceil => "fceil",
            Opcode::Ftrunc => "ftrunc",
            Opcode::Fround => "fround",
            Opcode::Icmp { .. } => "icmp",
            Opcode::Fcmp { .. } => "fcmp",
            // Overflow arithmetic
            Opcode::SaddOverflow => "sadd_overflow",
            Opcode::UaddOverflow => "uadd_overflow",
            Opcode::SsubOverflow => "ssub_overflow",
            Opcode::UsubOverflow => "usub_overflow",
            Opcode::SmulOverflow => "smul_overflow",
            Opcode::UmulOverflow => "umul_overflow",
            Opcode::Load => "load",
            Opcode::Store => "store",
            Opcode::Fload => "fload",
            Opcode::Fstore => "fstore",
            Opcode::Iconst => "iconst",
            Opcode::Fconst => "fconst",
            Opcode::Vconst => "vconst",
            Opcode::Poison => "poison",
            Opcode::Undef => "undef",
            Opcode::Sextend => "sextend",
            Opcode::Uextend => "uextend",
            Opcode::Ireduce => "ireduce",
            Opcode::Fptrunc => "fptrunc",
            Opcode::Fpext => "fpext",
            Opcode::Fptosi => "fptosi",
            Opcode::Sitofp => "sitofp",
            Opcode::Fptoui => "fptoui",
            Opcode::Uitofp => "uitofp",
            Opcode::Ptrtoint => "ptrtoint",
            Opcode::Inttoptr => "inttoptr",
            Opcode::Bitcast => "bitcast",
            Opcode::Call => "call",
            Opcode::CallIndirect => "call_indirect",
            Opcode::StackAddr => "stack_addr",
            Opcode::GlobalAddr => "global_addr",
            Opcode::Alloca => "alloca",
            Opcode::GetElementPtr => "gep",
            Opcode::Vadd => "vadd",
            Opcode::Vsub => "vsub",
            Opcode::Vmul => "vmul",
            Opcode::Vdiv => "vdiv",
            Opcode::Vneg => "vneg",
            Opcode::Vabs => "vabs",
            Opcode::Vextract => "vextract",
            Opcode::Vinsert => "vinsert",
            Opcode::Vbitcast => "vbitcast",
            Opcode::Vbroadcast => "vbroadcast",
            Opcode::Vsplit => "vsplit",
            Opcode::Vconcat => "vconcat",
            Opcode::ShuffleVector => "shufflevector",
            Opcode::Trap => "trap",
            Opcode::IsNull => "is_null",
            Opcode::IsNotNull => "is_not_null",
            Opcode::AddrSpaceCast => "addrspacecast",
            Opcode::VaArg => "va_arg",
            Opcode::LandingPad => "landingpad",
            Opcode::AtomicRmw => "atomicrmw",
            Opcode::Cmpxchg => "cmpxchg",
            Opcode::Fence => "fence",
            Opcode::ExtractValue => "extractvalue",
            Opcode::InsertValue => "insertvalue",
            Opcode::Copy => "copy",
            Opcode::Select => "select",
            Opcode::Freeze => "freeze",
            Opcode::Nop => "nop",
        }
    }

    /// 期望的值操作数数量。
    pub fn expected_operand_count(&self) -> usize {
        match self {
            // 2-operand: binary arithmetic, bitwise, comparison, shifts
            Opcode::Iadd
            | Opcode::Isub
            | Opcode::Imul
            | Opcode::Udiv
            | Opcode::Sdiv
            | Opcode::Urem
            | Opcode::Srem
            | Opcode::Fadd
            | Opcode::Fsub
            | Opcode::Fmul
            | Opcode::Fdiv
            | Opcode::Band
            | Opcode::Bor
            | Opcode::Bxor
            | Opcode::Ishl
            | Opcode::Ushr
            | Opcode::Sshr
            | Opcode::Icmp { .. }
            | Opcode::Fcmp { .. }
            | Opcode::Vadd
            | Opcode::Vsub
            | Opcode::Vmul
            | Opcode::Vdiv
            | Opcode::Rotl
            | Opcode::Rotr
            | Opcode::SaddSat
            | Opcode::SsubSat
            | Opcode::UaddSat
            | Opcode::UsubSat
            | Opcode::Fmin
            | Opcode::Fmax
            | Opcode::Fcopysign
            | Opcode::Smin
            | Opcode::Smax
            | Opcode::Umin
            | Opcode::Umax
            // Overflow arithmetic: 2 operands
            | Opcode::SaddOverflow
            | Opcode::UaddOverflow
            | Opcode::SsubOverflow
            | Opcode::UsubOverflow
            | Opcode::SmulOverflow
            | Opcode::UmulOverflow => 2,

            // 1-operand: unary ops
            Opcode::Bnot
            | Opcode::Fneg
            | Opcode::Fabs
            | Opcode::Fsqrt
            | Opcode::Freeze
            | Opcode::Copy
            | Opcode::Clz
            | Opcode::Ctz
            | Opcode::Popcnt
            | Opcode::Bitreverse
            | Opcode::Abs
            | Opcode::Bswap
            | Opcode::Ffloor
            | Opcode::Fceil
            | Opcode::Ftrunc
            | Opcode::Fround
            | Opcode::Vneg
            | Opcode::Vabs
            | Opcode::IsNull
            | Opcode::IsNotNull
            | Opcode::AddrSpaceCast
            | Opcode::VaArg
            | Opcode::Load
            | Opcode::Fload
            | Opcode::Sextend
            | Opcode::Uextend
            | Opcode::Ireduce
            | Opcode::Fptrunc
            | Opcode::Fpext
            | Opcode::Fptosi
            | Opcode::Sitofp
            | Opcode::Fptoui
            | Opcode::Uitofp
            | Opcode::Ptrtoint
            | Opcode::Inttoptr
            | Opcode::Bitcast
            | Opcode::Vextract
            | Opcode::Vbitcast
            | Opcode::Vsplit => 1,

            // 3-operand: fma, select, cmpxchg
            Opcode::Fma => 3,
            Opcode::Select => 3,
            Opcode::Cmpxchg => 3,

            // 2 + ptr: Store, atomicrmw, vinsert, insertvalue
            Opcode::Store
            | Opcode::Fstore
            | Opcode::Vinsert
            | Opcode::InsertValue
            | Opcode::AtomicRmw => 2,

            // Variable-count: Call, CallIndirect, GEP, ShuffleVector, Vbroadcast
            Opcode::Call
            | Opcode::CallIndirect
            | Opcode::GetElementPtr
            | Opcode::ShuffleVector
            | Opcode::Vbroadcast
            | Opcode::Vconcat => 0,

            // Zero-operand: constants, addresses, alloca, poison, undef, trap, nop, fence
            Opcode::Iconst
            | Opcode::Fconst
            | Opcode::Vconst
            | Opcode::Poison
            | Opcode::Undef
            | Opcode::StackAddr
            | Opcode::GlobalAddr
            | Opcode::Alloca
            | Opcode::LandingPad
            | Opcode::Trap
            | Opcode::Nop
            | Opcode::Fence => 0,

            Opcode::ExtractValue => 1,
        }
    }
}

// ============================================================
// 比较条件
// ============================================================

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
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum FloatCC {
    Ordered,
    Unordered,
    Equal,
    NotEqual,
    LessThan,
    LessThanOrEqual,
    GreaterThan,
    GreaterThanOrEqual,
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
        }
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
        ];
        for cc in &conds {
            let m = cc.mnemonic();
            assert!(!m.is_empty());
        }
    }

    // Helper: list of all opcode variants for systematic testing
    fn all_opcodes() -> Vec<Opcode> {
        vec![
            // Integer arithmetic
            Opcode::Iadd,
            Opcode::Isub,
            Opcode::Imul,
            Opcode::Udiv,
            Opcode::Sdiv,
            Opcode::Urem,
            Opcode::Srem,
            // Float
            Opcode::Fadd,
            Opcode::Fsub,
            Opcode::Fmul,
            Opcode::Fdiv,
            Opcode::Fneg,
            Opcode::Fabs,
            Opcode::Fsqrt,
            // Bitwise
            Opcode::Band,
            Opcode::Bor,
            Opcode::Bxor,
            Opcode::Bnot,
            Opcode::Ishl,
            Opcode::Ushr,
            Opcode::Sshr,
            // Bit manipulation
            Opcode::Clz,
            Opcode::Ctz,
            Opcode::Popcnt,
            Opcode::Bitreverse,
            Opcode::Rotl,
            Opcode::Rotr,
            // Integer extended
            Opcode::Abs,
            Opcode::Smin,
            Opcode::Smax,
            Opcode::Umin,
            Opcode::Umax,
            Opcode::SaddSat,
            Opcode::SsubSat,
            Opcode::UaddSat,
            Opcode::UsubSat,
            Opcode::Bswap,
            // Float extended
            Opcode::Fma,
            Opcode::Fmin,
            Opcode::Fmax,
            Opcode::Fcopysign,
            Opcode::Ffloor,
            Opcode::Fceil,
            Opcode::Ftrunc,
            Opcode::Fround,
            // Comparison
            Opcode::Icmp { cond: IntCC::Equal },
            Opcode::Fcmp {
                cond: FloatCC::Equal,
            },
            // Overflow
            Opcode::SaddOverflow,
            Opcode::UaddOverflow,
            Opcode::SsubOverflow,
            Opcode::UsubOverflow,
            Opcode::SmulOverflow,
            Opcode::UmulOverflow,
            // Memory
            Opcode::Load,
            Opcode::Store,
            Opcode::Fload,
            Opcode::Fstore,
            // Constants
            Opcode::Iconst,
            Opcode::Fconst,
            Opcode::Vconst,
            // Value semantics
            Opcode::Poison,
            Opcode::Undef,
            // Conversions
            Opcode::Sextend,
            Opcode::Uextend,
            Opcode::Ireduce,
            Opcode::Bitcast,
            // Calls
            Opcode::Call,
            Opcode::CallIndirect,
            // Address
            Opcode::StackAddr,
            Opcode::GlobalAddr,
            // Alloca / GEP
            Opcode::Alloca,
            Opcode::GetElementPtr,
            // Vector
            Opcode::Vadd,
            Opcode::Vsub,
            Opcode::Vmul,
            Opcode::Vdiv,
            Opcode::Vneg,
            Opcode::Vabs,
            Opcode::Vextract,
            Opcode::Vinsert,
            Opcode::Vbitcast,
            Opcode::Vbroadcast,
            Opcode::ShuffleVector,
            // Trap / Pointer
            Opcode::Trap,
            Opcode::IsNull,
            Opcode::IsNotNull,
            // Atomic
            Opcode::AtomicRmw,
            Opcode::Cmpxchg,
            Opcode::Fence,
            // Compound
            Opcode::ExtractValue,
            Opcode::InsertValue,
            // Pointer
            Opcode::AddrSpaceCast,
            Opcode::VaArg,
            // Exception
            Opcode::LandingPad,
            // Other
            Opcode::Copy,
            Opcode::Select,
            Opcode::Freeze,
            Opcode::Nop,
        ]
    }
}
