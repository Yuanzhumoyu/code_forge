//! IR 实体类型 — 零开销 newtype over u32。
//!
//! 所有实体都是 Copy + Eq + Hash，作为 PrimaryMap/SecondaryMap 的键。
//! 实体本身不携带数据，数据存储在 DataFlowGraph 的对应表中。

use std::fmt;

// ============================================================
// 实体定义
// ============================================================

/// SSA 值。
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Default)]
pub struct Value(pub u32);

/// 指令。
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Default)]
pub struct Inst(pub u32);

/// 基本块。
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Default)]
pub struct Block(pub u32);

/// 类型句柄 (在 TypeStore 中解析)。
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Default)]
pub struct TypeId(pub u32);

/// 函数引用 (在 Module 中解析)。
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Default)]
pub struct FuncRef(pub u32);

/// 常量引用 (在 ConstantPool 中解析)。
///
/// ## Encoding
///
/// ConstId packs a 2-bit type tag to disambiguate int/float/big pools:
/// - bits 31..30: tag (0 = int, 1 = float, 2 = big)
/// - bits 29..0:  index (30 bits, up to ~1B entries per category)
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Default)]
pub struct ConstId(pub u32);

impl ConstId {
    /// Tag indicating this is an integer constant.
    pub const TAG_INT: u32 = 0;
    /// Tag indicating this is a float constant.
    pub const TAG_FLOAT: u32 = 1;
    /// Tag indicating this is a big (arbitrary precision) constant.
    pub const TAG_BIG: u32 = 2;

    /// Maximum index value per category (30 bits).
    const MAX_INDEX: u32 = (1 << 30) - 1;

    /// Pack a tag and index into a ConstId.
    /// Invalid tag/index values are silently masked to prevent corruption in release builds.
    pub const fn pack(tag: u32, index: u32) -> Self {
        ConstId(((tag & 0x3) << 30) | (index & Self::MAX_INDEX))
    }

    /// Extract the type tag (upper 2 bits).
    pub fn tag(self) -> u32 {
        self.0 >> 30
    }

    /// Extract the index (lower 30 bits).
    pub fn index(self) -> u32 {
        self.0 & Self::MAX_INDEX
    }
}

/// 全局变量引用 (在 Module 中解析)。
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Default)]
pub struct GlobalId(pub u32);

/// 函数签名引用 (在 TypeStore 中解析)。
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Default)]
pub struct SigRef(pub u32);

/// 虚拟寄存器 (lowering 后使用，与 SSA Value 不同)。
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Default)]
pub struct VReg(pub u32);

impl fmt::Display for VReg {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "%{}", self.0)
    }
}

// ============================================================
// XReg — 临时寄存器（虚拟寄存器，用户 API 层句柄）
// ============================================================

/// 临时寄存器（虚拟寄存器）——用户实际调用 API 时使用的寄存器句柄。
///
/// 语义约定：
/// - **无限数量**：`index` 单调递增，无上限（由 [`XRegAllocator`] 分配）。
/// - **类型严格限定**：`class` 内嵌在值上，创建时固定；Int/Float 类不可错配
///   （分配器与编码器按值校验，杜绝跨类使用）。
/// - **位宽内嵌**：`width`（8/16/32/64 等）内嵌在值上，适配不同框架的寄存器
///   （如 16 位 AX、32 位 EAX、64 位 RAX）；分配器按位宽匹配物理寄存器。
/// - **被动**：值不可变，无 setter；只能作为指令的 def（结果）被赋予，
///   用户无法直接修改；外部只能经 [`XRegAllocator::alloc`] 创建，
///   不存在公开的 `XReg::new`（`new` 为 `#[doc(hidden)]` 内部 API）。
/// - **SSA 不可变**：一个 XReg 在整个函数内只被定义一次。
#[derive(Clone, Copy, PartialEq, Eq, Hash)]
pub struct XReg {
    index: u32,
    class: RegClass,
    width: u8,
}

impl XReg {
    /// 分配索引（内部使用）。
    ///
    /// `#[doc(hidden)]`：非公开 API。业务代码应经 [`XRegAllocator::alloc`] /
    /// `LowerCtx::alloc_xreg` 获取 XReg；此处仅供 DSL 生成代码与分配器内部使用。
    #[doc(hidden)]
    pub fn new(index: u32, class: RegClass, width: u8) -> Self {
        Self {
            index,
            class,
            width,
        }
    }

    /// 该临时寄存器的编号（无符号语义，仅用于调试/展示）。
    pub fn index(&self) -> u32 {
        self.index
    }

    /// 严格限定的寄存器类型（Int/Float 类）。
    pub fn class(&self) -> RegClass {
        self.class
    }

    /// 位宽（字节数，如 2=16 位、4=32 位、8=64 位）。
    pub fn width(&self) -> u8 {
        self.width
    }

    /// 位宽（bit，如 16/32/64）。
    pub fn bits(&self) -> u16 {
        (self.width as u16) * 8
    }
}

impl fmt::Display for XReg {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "x{}", self.index)
    }
}

impl fmt::Debug for XReg {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "XReg({})", self.index)
    }
}

/// 临时寄存器分配器——XReg 的唯一受控创建入口。
///
/// 持有单调递增的 index 计数器，保证 XReg 无限数量且全局唯一。
#[derive(Clone, Debug, Default)]
pub struct XRegAllocator {
    next: u32,
}

impl XRegAllocator {
    /// 创建新的分配器。
    pub fn new() -> Self {
        Self { next: 0 }
    }

    /// 分配一个指定类型与位宽的新临时寄存器（被动语义：只能由后续指令 def 赋予值）。
    ///
    /// `width` 为字节数（1=8 位、2=16 位、4=32 位、8=64 位等）。
    pub fn alloc(&mut self, class: RegClass, width: u8) -> XReg {
        let x = XReg::new(self.next, class, width);
        self.next += 1;
        x
    }

    /// 分配一个默认位宽的临时寄存器（按类型默认宽度）。
    pub fn alloc_default(&mut self, class: RegClass) -> XReg {
        self.alloc(class, class.default_width())
    }

    /// 当前已分配的临时寄存器数量（下一个 index）。
    pub fn next_index(&self) -> u32 {
        self.next
    }
}

/// 物理寄存器。
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct PReg {
    pub num: u8,
    pub class: RegClass,
}

impl PReg {
    pub const fn new(num: u8, class: RegClass) -> Self {
        Self { num, class }
    }
}

/// 目标端序。
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum Endianness {
    #[default]
    Little,
    Big,
}

impl TypeId {
    /// 获取位宽 — 对基本整数/浮点类型有效。
    /// 对于指针、向量、结构体、数组等复合类型返回 0。
    /// 对于完整的大小/对齐/类型查询，使用 `TypeStore::size_bytes()`。
    pub fn bits(&self) -> u32 {
        match *self {
            // void, bool
            TypeId::VOID => 0,
            TypeId::BOOL => 1,
            // i8, i16, i32, i64
            TypeId::I8 => 8,
            TypeId::I16 => 16,
            TypeId::I32 => 32,
            TypeId::I64 => 64,
            // f32, f64
            TypeId::F32 => 32,
            TypeId::F64 => 64,
            // ptr — dynamic, use DataLayout
            TypeId::PTR => 64, // default for backward compat
            // extended types — valid basic types indexed 10-15
            TypeId::I128 => 128, // I128
            TypeId::F16 => 16,   // F16
            TypeId::F128 => 128, // F128
            TypeId::V64 => 64,   // V64
            TypeId::V128 => 128, // V128
            TypeId::V256 => 256, // V256
            _ => 0,              // composite types
        }
    }

    /// 获取位宽 — 对基本整数/浮点类型返回 `Some(bits)`。
    /// 对于指针、向量、结构体、数组等复合类型返回 `None`。
    pub fn try_bits(&self) -> Option<u32> {
        match self.bits() {
            0 if self.0 >= 9 || (self.0 == 8) => None, // ptr (8) and composite types
            b => Some(b),
        }
    }
}

/// 寄存器类别 — 表示一组可互换的物理寄存器。
///
/// 不同类之间的寄存器不能互换，但可能共享相同的物理寄存器文件。
/// 子类通过 [`overlaps`] 方法表达 interference 关系。
///
/// ## 宽度子类
///
/// GPR8/GPR16/GPR32 与 GPR 共享物理寄存器文件（如 x86 的 RAX/EAX/AX/AL）。
/// 分配不同宽度的 VReg 到同一物理寄存器时，通过 [`overlaps`] 检测冲突。
///
/// ## 向量子类
///
/// VEC128 (XMM) 与 FPR 共享物理寄存器文件。
/// VEC256 (YMM) 包含 VEC128 子寄存器。
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum RegClass {
    /// 通用整数寄存器（默认，64-bit）
    GPR,
    /// 8-bit 整数子寄存器（如 x86 AL/CL/DL/BL, R8B-R15B）
    GPR8,
    /// 16-bit 整数子寄存器
    GPR16,
    /// 32-bit 整数子寄存器
    GPR32,

    /// 浮点/SSE 寄存器
    FPR,
    /// 128-bit 向量寄存器（XMM — 与 FPR 共享物理寄存器文件）
    VEC128,
    /// 256-bit 向量寄存器（YMM — 包含 XMM 子寄存器）
    VEC256,
}

/// 向后兼容别名。
impl RegClass {
    /// [`RegClass::GPR`] 的旧称。
    #[allow(nonstandard_style)]
    pub const Int: Self = RegClass::GPR;
    /// [`RegClass::FPR`] 的旧称。
    #[allow(nonstandard_style)]
    pub const Float: Self = RegClass::FPR;

    /// 此类寄存器的默认宽度（字节）。
    pub fn default_width(self) -> u8 {
        match self {
            RegClass::GPR => 8,
            RegClass::GPR8 => 1,
            RegClass::GPR16 => 2,
            RegClass::GPR32 => 4,
            RegClass::FPR => 8,
            RegClass::VEC128 => 16,
            RegClass::VEC256 => 32,
        }
    }

    /// 是否为整数类（GPR + 所有宽度子类）。
    pub fn is_int(self) -> bool {
        matches!(
            self,
            RegClass::GPR | RegClass::GPR8 | RegClass::GPR16 | RegClass::GPR32
        )
    }

    /// 是否为浮点/向量类。
    pub fn is_fp(self) -> bool {
        matches!(self, RegClass::FPR | RegClass::VEC128 | RegClass::VEC256)
    }

    /// 两个类是否有重叠的物理寄存器（决定 interference）。
    /// - GPR 子类之间始终重叠（共享 GPR 寄存器文件）
    /// - FPR/VEC128/VEC256 之间始终重叠（共享 XMM 寄存器文件）
    /// - Int 与 Float 从不重叠
    pub fn overlaps(self, other: RegClass) -> bool {
        if self == other {
            return true;
        }
        if self.is_int() && other.is_int() {
            return true;
        }
        if self.is_fp() && other.is_fp() {
            return true;
        }
        false
    }

    /// 从 TypeId 推导寄存器类（统一处理，消除多处硬编码）。
    pub fn from_type_id(ty: TypeId) -> Self {
        match ty {
            // bool, ptr, i64
            TypeId::BOOL | TypeId::PTR | TypeId::I64 => RegClass::GPR,
            // i8
            TypeId::I8 => RegClass::GPR8,
            // i16
            TypeId::I16 => RegClass::GPR16,
            // i32
            TypeId::I32 => RegClass::GPR32,
            // f32, f64
            TypeId::F32 | TypeId::F64 => RegClass::FPR,
            // I128
            TypeId::I128 => RegClass::GPR,
            // F16, F128
            TypeId::F16 | TypeId::F128 => RegClass::FPR,
            // V64, V128
            TypeId::V64 | TypeId::V128 => RegClass::VEC128,
            // V256
            TypeId::V256 => RegClass::VEC256,
            // void (0), composite (9, 16+), unknown → GPR fallback
            _ => RegClass::GPR,
        }
    }
}

/// 帧/栈指针访问模式。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum FrameAccess<R: PhysReg> {
    Register(R),
    StackOffset(i32),
    None,
}

impl<R: PhysReg> FrameAccess<R> {
    pub fn register_index(&self) -> Option<u8> {
        match self {
            FrameAccess::Register(r) => Some(r.to_index()),
            _ => None,
        }
    }
}

/// 物理寄存器 trait — ISA 后端定义自己的寄存器枚举实现此 trait。
pub trait PhysReg: Copy + Clone + core::fmt::Debug + PartialEq + Send + Sync + 'static {
    fn to_index(self) -> u8;
    fn from_index(idx: u8, class: RegClass) -> Self;
    fn class(self) -> RegClass;
}

// ============================================================
// Display impls
// ============================================================

impl fmt::Display for Value {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "v{}", self.0)
    }
}

impl fmt::Display for Inst {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "i{}", self.0)
    }
}

impl fmt::Display for Block {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "b{}", self.0)
    }
}

impl fmt::Display for TypeId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "t{}", self.0)
    }
}

impl fmt::Display for FuncRef {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "@{}", self.0)
    }
}

impl fmt::Display for ConstId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "c{}", self.0)
    }
}

impl fmt::Display for GlobalId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "g{}", self.0)
    }
}

impl fmt::Display for SigRef {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "sig{}", self.0)
    }
}

impl fmt::Display for PReg {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let class = if self.class.is_int() { "r" } else { "f" };
        write!(f, "{}{}", class, self.num)
    }
}

impl fmt::Debug for Value {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "Value({})", self.0)
    }
}

impl fmt::Debug for Inst {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "Inst({})", self.0)
    }
}

impl fmt::Debug for Block {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "Block({})", self.0)
    }
}

impl fmt::Debug for TypeId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "TypeId({})", self.0)
    }
}

impl fmt::Debug for FuncRef {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "FuncRef({})", self.0)
    }
}

impl fmt::Debug for ConstId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "ConstId({})", self.0)
    }
}

impl fmt::Debug for GlobalId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "GlobalId({})", self.0)
    }
}

impl fmt::Debug for SigRef {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "SigRef({})", self.0)
    }
}

// ============================================================
// 测试
// ============================================================

#[cfg(test)]
mod tests {
    use super::*;

    // === TypeId tests ===

    #[test]
    fn type_id_bits_void() {
        assert_eq!(TypeId(0).bits(), 0); // void
    }

    #[test]
    fn type_id_bits_bool() {
        assert_eq!(TypeId(1).bits(), 1); // bool / i1
    }

    #[test]
    fn type_id_bits_integers() {
        assert_eq!(TypeId(2).bits(), 8); // i8
        assert_eq!(TypeId(3).bits(), 16); // i16
        assert_eq!(TypeId(4).bits(), 32); // i32
        assert_eq!(TypeId(5).bits(), 64); // i64
    }

    #[test]
    fn type_id_bits_floats() {
        assert_eq!(TypeId(6).bits(), 32); // f32
        assert_eq!(TypeId(7).bits(), 64); // f64
    }

    #[test]
    fn type_id_bits_ptr() {
        assert_eq!(TypeId(8).bits(), 64); // ptr — default for backward compat
    }

    #[test]
    fn type_id_bits_extended() {
        assert_eq!(TypeId(10).bits(), 128); // I128
        assert_eq!(TypeId(11).bits(), 16); // F16
        assert_eq!(TypeId(12).bits(), 128); // F128
        assert_eq!(TypeId(13).bits(), 64); // V64
        assert_eq!(TypeId(14).bits(), 128); // V128
        assert_eq!(TypeId(15).bits(), 256); // V256
    }

    #[test]
    fn type_id_bits_composite_returns_zero() {
        // TypeId(9) and TypeId(16+) are composite types
        assert_eq!(TypeId(9).bits(), 0);
        assert_eq!(TypeId(16).bits(), 0);
        assert_eq!(TypeId(17).bits(), 0);
        assert_eq!(TypeId(100).bits(), 0);
    }

    #[test]
    fn type_id_try_bits_basic() {
        assert_eq!(TypeId(2).try_bits(), Some(8)); // i8
        assert_eq!(TypeId(5).try_bits(), Some(64)); // i64
        assert_eq!(TypeId(6).try_bits(), Some(32)); // f32
        assert_eq!(TypeId(1).try_bits(), Some(1)); // bool
        assert_eq!(TypeId(0).try_bits(), Some(0)); // void
    }

    #[test]
    fn type_id_try_bits_ptr() {
        // ptr (TypeId(8)) returns Some(64) from bits(), same behavior as try_bits()
        assert_eq!(TypeId(8).try_bits(), Some(64));
    }

    #[test]
    fn type_id_try_bits_composite_returns_none() {
        assert_eq!(TypeId(9).try_bits(), None);
        assert_eq!(TypeId(17).try_bits(), None);
    }

    // === ConstId tests ===

    #[test]
    fn const_id_pack_roundtrip_int() {
        let c = ConstId::pack(ConstId::TAG_INT, 42);
        assert_eq!(c.tag(), ConstId::TAG_INT);
        assert_eq!(c.index(), 42);
    }

    #[test]
    fn const_id_pack_roundtrip_float() {
        let c = ConstId::pack(ConstId::TAG_FLOAT, 7);
        assert_eq!(c.tag(), ConstId::TAG_FLOAT);
        assert_eq!(c.index(), 7);
    }

    #[test]
    fn const_id_pack_roundtrip_big() {
        let c = ConstId::pack(ConstId::TAG_BIG, 999);
        assert_eq!(c.tag(), ConstId::TAG_BIG);
        assert_eq!(c.index(), 999);
    }

    #[test]
    fn const_id_boundary_values() {
        // Index 0
        let c = ConstId::pack(ConstId::TAG_INT, 0);
        assert_eq!(c.index(), 0);
        // Index MAX
        let c = ConstId::pack(ConstId::TAG_INT, ConstId::MAX_INDEX);
        assert_eq!(c.index(), ConstId::MAX_INDEX);
        // Tag > 3 is masked
        let c = ConstId::pack(7, 5);
        assert_eq!(c.tag(), 3); // 7 & 0x3 = 3
        assert_eq!(c.index(), 5);
    }

    #[test]
    fn const_id_default() {
        let c = ConstId::default();
        assert_eq!(c.tag(), ConstId::TAG_INT);
        assert_eq!(c.index(), 0);
    }

    // === Entity Display tests ===

    #[test]
    fn value_display() {
        assert_eq!(format!("{}", Value(0)), "v0");
        assert_eq!(format!("{}", Value(42)), "v42");
    }

    #[test]
    fn inst_display() {
        assert_eq!(format!("{}", Inst(0)), "i0");
        assert_eq!(format!("{}", Inst(10)), "i10");
    }

    #[test]
    fn block_display() {
        assert_eq!(format!("{}", Block(0)), "b0");
        assert_eq!(format!("{}", Block(5)), "b5");
    }

    #[test]
    fn type_id_display() {
        assert_eq!(format!("{}", TypeId(3)), "t3");
    }

    #[test]
    fn func_ref_display() {
        assert_eq!(format!("{}", FuncRef(1)), "@1");
    }

    #[test]
    fn const_id_display() {
        assert_eq!(format!("{}", ConstId(7)), "c7");
    }

    #[test]
    fn global_id_display() {
        assert_eq!(format!("{}", GlobalId(2)), "g2");
    }

    #[test]
    fn sig_ref_display() {
        assert_eq!(format!("{}", SigRef(0)), "sig0");
    }

    #[test]
    fn preg_display() {
        assert_eq!(format!("{}", PReg::new(0, RegClass::GPR)), "r0");
        assert_eq!(format!("{}", PReg::new(3, RegClass::FPR)), "f3");
        // Int/Float 常量别名仍可用
        assert_eq!(format!("{}", PReg::new(1, RegClass::Int)), "r1");
        assert_eq!(format!("{}", PReg::new(7, RegClass::Float)), "f7");
    }

    #[test]
    fn vreg_display() {
        assert_eq!(format!("{}", VReg(5)), "%5");
    }

    // === Entity Debug tests ===

    #[test]
    fn value_debug() {
        assert_eq!(format!("{:?}", Value(3)), "Value(3)");
    }

    #[test]
    fn inst_debug() {
        assert_eq!(format!("{:?}", Inst(7)), "Inst(7)");
    }

    #[test]
    fn block_debug() {
        assert_eq!(format!("{:?}", Block(1)), "Block(1)");
    }

    #[test]
    fn type_id_debug() {
        assert_eq!(format!("{:?}", TypeId(5)), "TypeId(5)");
    }

    #[test]
    fn func_ref_debug() {
        assert_eq!(format!("{:?}", FuncRef(0)), "FuncRef(0)");
    }

    #[test]
    fn const_id_debug() {
        assert_eq!(format!("{:?}", ConstId(0)), "ConstId(0)");
    }

    #[test]
    fn global_id_debug() {
        assert_eq!(format!("{:?}", GlobalId(1)), "GlobalId(1)");
    }

    #[test]
    fn sig_ref_debug() {
        assert_eq!(format!("{:?}", SigRef(2)), "SigRef(2)");
    }

    // === RegClass / Endianness tests ===

    #[test]
    fn reg_class_variants() {
        // 新变体
        let gpr = RegClass::GPR;
        let gpr8 = RegClass::GPR8;
        let fpr = RegClass::FPR;
        assert_ne!(gpr, fpr);
        assert_ne!(gpr8, fpr);
        // Copy
        let gpr2 = gpr;
        assert_eq!(gpr, gpr2);
        assert_eq!(fpr, RegClass::FPR);
        // 向后兼容别名
        assert_eq!(RegClass::Int, RegClass::GPR);
        assert_eq!(RegClass::Float, RegClass::FPR);
        // default_width
        assert_eq!(RegClass::GPR.default_width(), 8);
        assert_eq!(RegClass::GPR8.default_width(), 1);
        assert_eq!(RegClass::GPR16.default_width(), 2);
        assert_eq!(RegClass::GPR32.default_width(), 4);
        assert_eq!(RegClass::FPR.default_width(), 8);
        assert_eq!(RegClass::VEC128.default_width(), 16);
        assert_eq!(RegClass::VEC256.default_width(), 32);
    }

    #[test]
    fn reg_class_is_int_is_fp() {
        assert!(RegClass::GPR.is_int());
        assert!(RegClass::GPR8.is_int());
        assert!(RegClass::GPR16.is_int());
        assert!(RegClass::GPR32.is_int());
        assert!(!RegClass::GPR.is_fp());
        assert!(!RegClass::GPR8.is_fp());

        assert!(RegClass::FPR.is_fp());
        assert!(RegClass::VEC128.is_fp());
        assert!(RegClass::VEC256.is_fp());
        assert!(!RegClass::FPR.is_int());
        assert!(!RegClass::VEC128.is_int());
    }

    #[test]
    fn reg_class_overlaps() {
        // 同类的重叠
        assert!(RegClass::GPR.overlaps(RegClass::GPR));
        // GPR 子类之间重叠
        assert!(RegClass::GPR.overlaps(RegClass::GPR8));
        assert!(RegClass::GPR32.overlaps(RegClass::GPR16));
        assert!(RegClass::GPR8.overlaps(RegClass::GPR32));
        // FPR/VEC 之间重叠
        assert!(RegClass::FPR.overlaps(RegClass::VEC128));
        assert!(RegClass::VEC128.overlaps(RegClass::VEC256));
        // Int 与 Float 不重叠
        assert!(!RegClass::GPR.overlaps(RegClass::FPR));
        assert!(!RegClass::GPR8.overlaps(RegClass::VEC128));
        assert!(!RegClass::GPR32.overlaps(RegClass::VEC256));
    }

    #[test]
    fn reg_class_from_type_id() {
        assert_eq!(RegClass::from_type_id(TypeId::I8), RegClass::GPR8);
        assert_eq!(RegClass::from_type_id(TypeId::I16), RegClass::GPR16);
        assert_eq!(RegClass::from_type_id(TypeId::I32), RegClass::GPR32);
        assert_eq!(RegClass::from_type_id(TypeId::I64), RegClass::GPR);
        assert_eq!(RegClass::from_type_id(TypeId::BOOL), RegClass::GPR);
        assert_eq!(RegClass::from_type_id(TypeId::PTR), RegClass::GPR);
        assert_eq!(RegClass::from_type_id(TypeId::F32), RegClass::FPR);
        assert_eq!(RegClass::from_type_id(TypeId::F64), RegClass::FPR);
        assert_eq!(RegClass::from_type_id(TypeId::F16), RegClass::FPR);
        assert_eq!(RegClass::from_type_id(TypeId::F128), RegClass::FPR);
        assert_eq!(RegClass::from_type_id(TypeId::V128), RegClass::VEC128);
        assert_eq!(RegClass::from_type_id(TypeId::V256), RegClass::VEC256);
        // void → GPR fallback
        assert_eq!(RegClass::from_type_id(TypeId::VOID), RegClass::GPR);
    }

    #[test]
    fn endianness_default_is_little() {
        assert_eq!(Endianness::default(), Endianness::Little);
    }

    // === FrameAccess tests ===

    #[test]
    fn frame_access_register_index() {
        // Need a concrete PhysReg impl for testing.
        // Use a test-only register type.
        #[derive(Clone, Copy, Debug, PartialEq)]
        struct TestReg(u8);
        impl PhysReg for TestReg {
            fn to_index(self) -> u8 {
                self.0
            }
            fn from_index(idx: u8, _class: RegClass) -> Self {
                TestReg(idx)
            }
            fn class(self) -> RegClass {
                RegClass::GPR
            }
        }

        let reg_access: FrameAccess<TestReg> = FrameAccess::Register(TestReg(42));
        assert_eq!(reg_access.register_index(), Some(42));

        let stack_access: FrameAccess<TestReg> = FrameAccess::StackOffset(-16);
        assert_eq!(stack_access.register_index(), None);

        let none_access: FrameAccess<TestReg> = FrameAccess::None;
        assert_eq!(none_access.register_index(), None);
    }
}
