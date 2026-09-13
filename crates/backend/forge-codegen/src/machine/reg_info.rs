//! TargetRegInfo — 物理寄存器文件描述。
//!
//! 与 ABI 分离，因为寄存器文件结构（名称、数量、宽度）独立于调用约定。

use forge_ir::{FrameAccess, PReg, PhysReg, RegClass, TypeId, XReg};

/// 物理寄存器文件描述。
///
/// 为寄存器分配器提供寄存器资源信息。
pub trait TargetRegInfo: Send + Sync + 'static {
    type Reg: PhysReg;

    /// 通用寄存器数量。
    fn num_gp_regs(&self) -> u32;
    /// 浮点/向量寄存器数量。
    fn num_fp_regs(&self) -> u32;

    /// 寄存器类表（ISA 声明）：**分配器类表的唯一来源**。
    ///
    /// 每个"宿主可能请求的类"都指向同族的物理寄存器文件；未声明的族不产生类
    /// （1 字节寄存器 ISA 只有 `GPR(1)`，请求 `i16`/`i32`/`i64` 由值池门在编译期
    /// 拒绝）。缺省空 = 非 DSL 后端/测试替身（编译器会用主类补两条兜底）。
    fn register_classes(&self) -> Vec<super::isa_info::RegisterClassInfo> {
        Vec::new()
    }

    /// ISA 默认整数值类（lowering 中 alloc_xreg 的默认目标类）。
    /// 元数据驱动：由 DSL 从 `[meta].default_gpr_width` / 最宽 `[reg.gpr<N>]`
    /// 组生成；trait 缺省回退 GPR64（历史值）。
    fn default_gpr_class(&self) -> RegClass {
        RegClass::GPR64
    }

    /// ISA 默认浮点值类（ABI/SSE 占位基准）。
    /// 元数据驱动：由 DSL 从 `[meta].default_fpr_width` / `[reg.fpr16]`（XMM 基准）
    /// / 最宽 FPR 组生成；trait 缺省回退 FPR64。
    fn default_fpr_class(&self) -> RegClass {
        RegClass::FPR64
    }

    /// 地址/指针类（MemRef base/index、`lea`、sp/fp、帧地址计算）。
    /// 元数据驱动：DSL 从 `[meta].addr_width` 或主 GPR 组派生；缺省 = 主整数值类。
    fn addr_class(&self) -> RegClass {
        self.default_gpr_class()
    }

    /// 宿主「整数值寄存器池」类（值 XReg / 零值 / 临时 vreg 的默认类）。
    /// 与 `addr_class` 分离：1 字节 ISA 可让值池 = 地址池，但二者的*用途*
    /// 不同（地址类不得用于承载宽整数值）。缺省 = 主整数值类（历史 GPR64）。
    fn value_gpr_class(&self) -> RegClass {
        self.default_gpr_class()
    }

    /// 宿主「浮点值寄存器池」类。**缺省 FPR(8)**（= 历史 `FPR64`：f64 值池宽），
    /// **不等于** `default_fpr_class()`——后者是 ABI/SSE 占位基准（x86 = FPR(16)
    /// = XMM）。元数据驱动：DSL 从 `[meta].value_fpr_width` 生成。
    fn value_fpr_class(&self) -> RegClass {
        RegClass::FPR(8)
    }

    /// ABI 栈槽单位（字节）：alloca/聚合拆分/传参栈槽/spill 槽对齐。
    /// 元数据驱动：DSL 从 `[meta].slot_bytes` 或地址类宽度派生（x86 = 8）。
    fn slot_bytes(&self) -> u16 {
        self.addr_class().width()
    }

    /// 向量类的字节档位（升序）：`reg_class_for` 把向量字节数夹到"最小的
    /// ≥ 请求字节数的档位"，超出最大档 → 最大档。缺省 = x86 三档
    /// （16/32/64 = XMM/YMM/ZMM），元数据可覆盖（`[meta].vector_tiers`）。
    fn vector_tiers(&self) -> &[u16] {
        &[16, 32, 64]
    }

    /// 类型 → 寄存器类；`None` = 本 ISA 无法承载该类型（调用方必须报
    /// `Unsupported`，**不得**静默降级到某个宽度缺省）。
    /// 缺省实现 = `RegClass::from_type_id`（i8→GPR(1)/i64→GPR(8)/f64→FPR(8)/…）。
    /// DSL 生成的实现：① 先查 `[types]` 显式映射（ISA 数据，可表达软浮点
    /// `f64 = "gpr8"`、1 字节地址 `ptr = "gpr1"` 等非常规映射）；② 再走
    /// `class_for_type_in_pool`（族 + 值池宽 + 寄存器文件存在性）。
    fn class_for_type(&self, ty: TypeId) -> Option<RegClass> {
        Some(RegClass::from_type_id(ty))
    }

    /// `[types]` 显式类型→类映射（ISA 数据）。**lowering 必须与
    /// `class_for_type` 用同一份映射**（否则门放行、lowering 却按别的类分配）。
    /// 缺省空 = 只用 `RegClass::from_type_id` 的族/宽度规则。
    fn type_map(&self) -> &'static [(TypeId, RegClass)] {
        &[]
    }

    /// 寄存器类的字节宽度（从 ISA TOML [reg_classes] 读取）。
    /// 默认使用 RegClass::default_width()。
    fn reg_class_width(&self, class: RegClass) -> u8 {
        class.default_width()
    }

    /// 栈指针寄存器。
    fn sp_reg(&self) -> FrameAccess<Self::Reg>;
    /// 帧指针寄存器（None 表示 ISA 不使用帧指针）。
    fn fp_reg(&self) -> Option<Self::Reg>;

    /// 通用寄存器分配优先级顺序（靠前的优先分配）。
    /// 排除 SP、FP、spill scratch 与 `[abi].reserved`（DSL 生成的实现；
    /// callee-saved 仍可分配——跨调用由 prologue/epilogue 保存）。
    fn allocatable_gp_order(&self) -> Vec<u32>;

    /// 浮点寄存器分配优先级顺序。
    fn allocatable_fp_order(&self) -> Vec<u32>;

    /// 溢出代码可用的临时寄存器（不跨 spill load/emit/store 使用）。
    fn scratch_regs(&self) -> Vec<u32>;

    /// 被调用者保存的寄存器索引列表。
    fn callee_saved(&self) -> Vec<u32>;

    /// Prologue 在帧指针上方 push 的字节数（帧指针保存槽，如 x86 `push rbp`
    /// = 8；aarch64/riscv64 `stp/sd fp,lr` = 16；wasm 无帧 = 0）。
    /// codegen 用它计算局部变量区基址（`fp - overhead - callee_saved_bytes`）。
    /// 缺省 = **地址类宽度**（元数据派生：1 字节寄存器 ISA = 1；DSL 生成的
    /// `RegInfo` 一律用 `[meta].fp_overhead_bytes` 覆写）。
    fn frame_pointer_overhead(&self) -> u32 {
        self.addr_class().width() as u32
    }

    /// 预着色的 VReg → PReg 映射（如 RAX = VReg(0) 用于返回值）。
    fn precolored_xregs(&self) -> Vec<(XReg, PReg)> {
        Vec::new()
    }
}

/// 类型 → 寄存器类，并按 ISA **值池宽度 + 寄存器文件存在性**做 fail-closed
/// 校验（2026-09-12 去「宽度寄存器写死」）：DSL 生成的 `RegInfo::class_for_type`
/// 与宿主编译入口共用本函数。
///
/// 规则：
/// - 整数族（GPR，含 bool/ptr）：类宽 ≤ `gpr_pool.width()`，否则 `None`；
///   （更窄的 GPR 类在值语义上是合法视图，故不要求存在同名宽度组）
/// - 浮点族（FPR）：`fpr_pool` 为 `None`（ISA 未声明任何浮点寄存器组）→ `None`；
///   否则类宽 ≤ `fpr_pool.width()`；
/// - 向量：同样要求 `fpr_pool` 存在（向量寄存器与浮点共用寄存器文件），再按
///   字节数夹到 `vector_tiers` 中**最小的 ≥ 请求值**的档位；超过最大档 → 取最大档；
/// - `KReg`：原样通过。
///
/// 返回 `None` 的调用方必须报 `Unsupported`（点名类型与值池宽度），
/// **不得**回退到某个宽度缺省（1 字节寄存器 ISA 上把 i64 放进 GPR(8)、或把 f64
/// 放进根本不存在的 FPR(8)，都是"生成成功但语义错误"）。
pub fn class_for_type_in_pool(
    ty: TypeId,
    gpr_pool: RegClass,
    fpr_pool: Option<RegClass>,
    vector_tiers: &[u16],
) -> Option<RegClass> {
    match RegClass::from_type_id(ty) {
        RegClass::GPR(w) => (w <= gpr_pool.width()).then_some(RegClass::GPR(w)),
        RegClass::FPR(w) => {
            let pool = fpr_pool?;
            (w <= pool.width()).then_some(RegClass::FPR(w))
        }
        RegClass::VEC(_) => {
            // 无浮点寄存器文件的 ISA 也没有向量寄存器（x86 的 XMM/VEC 与
            // FPR 同组）——不能放行，否则会构造该 ISA 不存在的 VEC 类。
            let _ = fpr_pool?;
            let bytes = (ty.bits() / 8) as u16;
            let tier = vector_tiers
                .iter()
                .copied()
                .find(|t| *t >= bytes)
                .or_else(|| vector_tiers.last().copied())?;
            Some(RegClass::VEC(tier.max(1)))
        }
        RegClass::KReg(w) => Some(RegClass::KReg(w)),
    }
}

/// 类型的可读名（错误信息用；`TypeId` 的 `Debug` 只有裸编号 `TypeId(5)`，而
/// 值池拒绝信息必须点名 `i64` 这类类型名才可操作）。
pub fn type_label(ty: TypeId) -> String {
    match ty {
        TypeId::VOID => "void".into(),
        TypeId::BOOL => "bool".into(),
        TypeId::I8 => "i8".into(),
        TypeId::I16 => "i16".into(),
        TypeId::I32 => "i32".into(),
        TypeId::I64 => "i64".into(),
        TypeId::F32 => "f32".into(),
        TypeId::F64 => "f64".into(),
        TypeId::PTR => "ptr".into(),
        TypeId::I128 => "i128".into(),
        TypeId::F16 => "f16".into(),
        TypeId::F128 => "f128".into(),
        TypeId::V64 => "v64".into(),
        TypeId::V128 => "v128".into(),
        TypeId::V256 => "v256".into(),
        other => format!("{other:?}"),
    }
}
