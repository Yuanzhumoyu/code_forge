//! IR 基础类型定义。

use crate::FloatFormat;
use std::fmt;

/// SSA 值标识。
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct Value(pub u32);

/// 基本块标识。
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct BlockId(pub u32);

/// 函数引用（用于调用指令）。
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct FuncRef(pub u32);

/// 虚拟寄存器（lowering 后使用，与 SSA Value 不同）。
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct VReg(pub u32);

/// 物理寄存器。
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct PReg {
    pub num: u8,
    pub class: RegClass,
}

impl PReg {
    pub fn new(num: u8, class: RegClass) -> Self {
        Self { num, class }
    }
}

/// 寄存器类别。
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
#[non_exhaustive]
pub enum RegClass {
    Int,
    Float,
}

// ============================================================
// 物理寄存器抽象
// ============================================================

/// 物理寄存器 trait — ISA 后端定义自己的寄存器枚举实现此 trait。
///
/// # Example
/// ```ignore
/// #[derive(Clone, Copy, Debug, PartialEq, Eq)]
/// enum MyReg { R0, R1, R2, R3, Sp, Fp }
///
/// impl PhysReg for MyReg {
///     fn to_index(self) -> u8 { self as u8 }
///     fn from_index(idx: u8, _class: RegClass) -> Self {
///         match idx { 0 => MyReg::R0, 1 => MyReg::R1, ... _ => MyReg::R0 }
///     }
///     fn class(self) -> RegClass { RegClass::Int }
/// }
/// ```
pub trait PhysReg: Copy + Clone + core::fmt::Debug + PartialEq + Send + Sync + 'static {
    /// 转换为寄存器索引（用于 regalloc 和 emit）。
    fn to_index(self) -> u8;
    /// 从索引和类别构建寄存器。
    fn from_index(idx: u8, class: RegClass) -> Self;
    /// 寄存器类别（整数或浮点）。
    fn class(self) -> RegClass;
}

/// 帧/栈指针访问模式。
///
/// 不是所有 ISA 都使用专用寄存器作为帧指针。
/// 有些实现直接通过栈偏移访问，有些则根本不使用帧指针。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum FrameAccess<R: PhysReg> {
    /// 帧指针/栈指针存在于物理寄存器中。
    Register(R),
    /// 通过相对于栈顶的固定偏移访问帧。
    StackOffset(i32),
    /// 不使用帧指针。
    None,
}

impl<R: PhysReg> FrameAccess<R> {
    /// 如果是寄存器访问，返回寄存器的索引；否则返回 None。
    pub fn register_index(&self) -> Option<u8> {
        match self {
            FrameAccess::Register(r) => Some(r.to_index()),
            _ => None,
        }
    }
}

/// IR 类型系统。
///
/// 包含基本类型（整数、浮点、向量）和复合类型（结构体、数组、指针、函数类型）。
/// 复合类型使用 u32 索引引用 Context 中的注册表，以保持 Type 的 Copy 语义。
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Type {
    Void,
    /// 布尔类型（1 字节，仅取值 0 或 1）。
    /// 由 icmp/fcmp 产生，用于 branch/select 的条件。
    Bool,
    I8,
    I16,
    I32,
    I64,
    I128,
    F16,
    F32,
    F64,
    F128,
    /// SIMD 向量类型。
    V64,
    V128,
    V256,
    Ptr,
    // ============================================================
    // 复合类型（通过索引引用 Context 中的注册表）
    // ============================================================
    /// 命名结构体（索引指向 Context::struct_types）。
    StructNamed(u32),
    /// 匿名结构体（索引指向 Context::anonymous_structs）。
    StructAnon(u32),
    /// 数组类型（索引指向 Context::array_types）。
    Array(u32),
    /// 向量类型（索引指向 Context::vector_types）。
    Vector(u32),
    /// 指针类型（索引指向 Context::pointer_types）。
    Pointer(u32),
    /// 函数类型（索引指向 Context::function_types）。
    Function(u32),
}

impl Type {
    /// 返回类型的字节大小。
    pub fn size_bytes(&self) -> u32 {
        match self {
            Type::Void => 0,
            Type::Bool => 1,
            Type::I8 => 1,
            Type::I16 | Type::F16 => 2,
            Type::I32 | Type::F32 => 4,
            Type::I64 | Type::F64 | Type::Ptr => 8,
            Type::I128 | Type::F128 | Type::V128 => 16,
            Type::V64 => 8,
            Type::V256 => 32,
            // 复合类型的精确大小需通过 Context 查询
            Type::StructNamed(_)
            | Type::StructAnon(_)
            | Type::Array(_)
            | Type::Vector(_)
            | Type::Pointer(_)
            | Type::Function(_) => 8,
        }
    }

    /// 是否为向量类型。
    pub fn is_vector(&self) -> bool {
        matches!(self, Type::V64 | Type::V128 | Type::V256 | Type::Vector(_))
    }

    /// 是否为整数类型。
    pub fn is_int(&self) -> bool {
        matches!(
            self,
            Type::I8 | Type::I16 | Type::I32 | Type::I64 | Type::I128
        )
    }

    /// 是否为浮点类型。
    pub fn is_float(&self) -> bool {
        matches!(self, Type::F16 | Type::F32 | Type::F64 | Type::F128)
    }

    /// 获取浮点格式描述。
    pub fn float_format(&self) -> Option<FloatFormat> {
        match self {
            Type::F16 => Some(FloatFormat::F16),
            Type::F32 => Some(FloatFormat::F32),
            Type::F64 => Some(FloatFormat::F64),
            Type::F128 => Some(FloatFormat::F128),
            _ => None,
        }
    }

    /// 是否为指针类型。
    pub fn is_ptr(&self) -> bool {
        matches!(self, Type::Ptr | Type::Pointer(_))
    }

    /// 是否为 void 类型。
    pub fn is_void(&self) -> bool {
        matches!(self, Type::Void)
    }

    /// 是否为复合类型（结构体/数组/函数）。
    pub fn is_aggregate(&self) -> bool {
        matches!(
            self,
            Type::StructNamed(_) | Type::StructAnon(_) | Type::Array(_) | Type::Function(_)
        )
    }

    /// 返回类型的位宽。
    pub fn bits(&self) -> u32 {
        self.size_bytes() * 8
    }
}

/// 目标端序。
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum Endianness {
    #[default]
    Little,
    Big,
}

/// 整数比较条件。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
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

/// 浮点比较条件。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
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

/// 内存排序约束（用于原子操作和屏障）。
///
/// 对应 LLVM 的 atomic ordering，从最弱到最强。
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum Ordering {
    /// 非原子（普通 load/store）。
    NotAtomic = 0,
    /// 无序（最低级别的原子保证）。
    Unordered = 1,
    /// 单调（仅保证同一地址的操作顺序）。
    Monotonic = 2,
    /// 获取（Acquire）：此操作后的内存访问不会被重排到此操作之前。
    Acquire = 3,
    /// 释放（Release）：此操作前的内存访问不会被重排到此操作之后。
    Release = 4,
    /// 获取-释放（AcquireRelease）：结合 Acquire 和 Release。
    AcquireRelease = 5,
    /// 顺序一致性（SeqCst）：最强的内存序保证。
    SequentiallyConsistent = 6,
}

impl Ordering {
    /// 是否为获取语义（包含 Acquire 或更强的）。
    pub fn is_acquire(self) -> bool {
        matches!(
            self,
            Ordering::Acquire | Ordering::AcquireRelease | Ordering::SequentiallyConsistent
        )
    }

    /// 是否为释放语义（包含 Release 或更强的）。
    pub fn is_release(self) -> bool {
        matches!(
            self,
            Ordering::Release | Ordering::AcquireRelease | Ordering::SequentiallyConsistent
        )
    }
}

/// 原子 Read-Modify-Write 操作类型。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum AtomicRmwOp {
    /// 交换（atomic swap）：`*ptr = val`，返回旧值。
    Xchg,
    /// 原子加法：`*ptr += val`，返回旧值。
    Add,
    /// 原子减法：`*ptr -= val`，返回旧值。
    Sub,
    /// 原子按位与：`*ptr &= val`，返回旧值。
    And,
    /// 原子按位与非：`*ptr = ~(*ptr & val)`，返回旧值。
    Nand,
    /// 原子按位或：`*ptr |= val`，返回旧值。
    Or,
    /// 原子按位异或：`*ptr ^= val`，返回旧值。
    Xor,
    /// 原子有符号最大值：`*ptr = max(*ptr, val)`，返回旧值。
    Max,
    /// 原子有符号最小值：`*ptr = min(*ptr, val)`，返回旧值。
    Min,
    /// 原子无符号最大值：返回旧值。
    Umax,
    /// 原子无符号最小值：返回旧值。
    Umin,
}

/// Fast-math 标志 — 控制浮点优化的激进程度。
///
/// 设置这些标志允许优化器执行不符合 IEEE 754 严格语义的变换，
/// 与 LLVM 的 `fast-math` 标志和 Clang 的 `-ffast-math` 对齐。
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub struct FastMathFlags(u8);

impl FastMathFlags {
    pub const NONE: Self = Self(0);
    pub const FAST: Self = Self(0x1F);

    pub const fn new() -> Self { Self(0) }
    pub const fn with_nnan(mut self) -> Self { self.0 |= 0x01; self }
    pub const fn with_ninf(mut self) -> Self { self.0 |= 0x02; self }
    pub const fn with_nsz(mut self) -> Self { self.0 |= 0x04; self }
    pub const fn with_arcp(mut self) -> Self { self.0 |= 0x08; self }
    pub const fn with_reassoc(mut self) -> Self { self.0 |= 0x10; self }

    pub const fn nnan(self) -> bool { self.0 & 0x01 != 0 }
    pub const fn ninf(self) -> bool { self.0 & 0x02 != 0 }
    pub const fn nsz(self) -> bool { self.0 & 0x04 != 0 }
    pub const fn arcp(self) -> bool { self.0 & 0x08 != 0 }
    pub const fn reassoc(self) -> bool { self.0 & 0x10 != 0 }
    pub const fn is_fast(self) -> bool { self.0 == 0x1F }
}

/// 调用约定。
///
/// 影响参数传递方式、寄存器使用、栈布局等。
/// ISA 后端在 lowering 时根据此信息生成正确的调用序列。
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum CallConv {
    /// 平台默认（如 x86_64 System V, Aarch64 AAPCS）。
    #[default]
    Default,
    /// System V AMD64 ABI (Linux, macOS)。
    SystemV,
    /// Microsoft x64 calling convention (Windows)。
    WindowsX64,
    /// 快速调用（前两个参数通过寄存器传递）。
    Fast,
    /// 被调用者清理栈。
    CDecl,
    /// 内部调用（不暴露给外部，可自由优化）。
    Internal,
}

/// 函数签名。
#[derive(Clone, Debug)]
pub struct Signature {
    pub params: Vec<(Type, String)>,
    pub returns: Vec<Type>,
    /// 调用约定。
    pub calling_convention: CallConv,
}

impl Signature {
    pub fn new(params: &[(Type, &str)], returns: &[Type]) -> Self {
        Self {
            params: params.iter().map(|(t, n)| (*t, n.to_string())).collect(),
            returns: returns.to_vec(),
            calling_convention: CallConv::default(),
        }
    }

    /// 无参数、无返回值的签名。
    pub fn void() -> Self {
        Self {
            params: Vec::new(),
            returns: Vec::new(),
            calling_convention: CallConv::default(),
        }
    }

    /// 设置调用约定（Builder 模式）。
    pub fn with_calling_convention(mut self, cc: CallConv) -> Self {
        self.calling_convention = cc;
        self
    }
}

// Display impls

impl fmt::Display for Value {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "v{}", self.0)
    }
}

impl fmt::Display for BlockId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "b{}", self.0)
    }
}

impl fmt::Display for FuncRef {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "f{}", self.0)
    }
}

impl fmt::Display for VReg {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "%{}", self.0)
    }
}

impl fmt::Display for PReg {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let class = match self.class {
            RegClass::Int => "r",
            RegClass::Float => "f",
        };
        write!(f, "{}{}", class, self.num)
    }
}

impl fmt::Display for Type {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Type::Void => write!(f, "void"),
            Type::Bool => write!(f, "bool"),
            Type::I8 => write!(f, "i8"),
            Type::I16 => write!(f, "i16"),
            Type::I32 => write!(f, "i32"),
            Type::I64 => write!(f, "i64"),
            Type::I128 => write!(f, "i128"),
            Type::F16 => write!(f, "f16"),
            Type::F32 => write!(f, "f32"),
            Type::F64 => write!(f, "f64"),
            Type::F128 => write!(f, "f128"),
            Type::V64 => write!(f, "v64"),
            Type::V128 => write!(f, "v128"),
            Type::V256 => write!(f, "v256"),
            Type::Ptr => write!(f, "ptr"),
            Type::StructNamed(idx) => write!(f, "struct@{}", idx),
            Type::StructAnon(idx) => write!(f, "struct_anon@{}", idx),
            Type::Array(idx) => write!(f, "array@{}", idx),
            Type::Vector(idx) => write!(f, "vector@{}", idx),
            Type::Pointer(idx) => write!(f, "ptr@{}", idx),
            Type::Function(idx) => write!(f, "fn@{}", idx),
        }
    }
}

#[cfg(test)]
mod test {
    use super::*;

    #[test]
    fn test_composite_type_variants() {
        // 验证复合类型变体的基本属性
        assert!(Type::StructNamed(0).is_aggregate());
        assert!(Type::StructAnon(1).is_aggregate());
        assert!(Type::Array(2).is_aggregate());
        assert!(Type::Function(3).is_aggregate());

        // 向量和指针不是 aggregate
        assert!(!Type::Vector(4).is_aggregate());
        assert!(!Type::Pointer(5).is_aggregate());

        // 复合类型是指针
        assert!(Type::Pointer(0).is_ptr());
        assert!(Type::Ptr.is_ptr());
        assert!(!Type::I32.is_ptr());

        // 向量类型检测
        assert!(Type::Vector(0).is_vector());
        assert!(!Type::Array(0).is_vector());
    }

    #[test]
    fn test_composite_type_display() {
        assert_eq!(format!("{}", Type::StructNamed(0)), "struct@0");
        assert_eq!(format!("{}", Type::StructAnon(1)), "struct_anon@1");
        assert_eq!(format!("{}", Type::Array(2)), "array@2");
        assert_eq!(format!("{}", Type::Vector(3)), "vector@3");
        assert_eq!(format!("{}", Type::Pointer(4)), "ptr@4");
        assert_eq!(format!("{}", Type::Function(5)), "fn@5");
    }

    #[test]
    fn test_composite_type_size_and_align() {
        // 复合类型默认大小为 8 字节（占位值）
        assert_eq!(Type::StructNamed(0).size_bytes(), 8);
        assert_eq!(Type::Array(0).size_bytes(), 8);
        assert_eq!(Type::Function(0).size_bytes(), 8);
    }
}
