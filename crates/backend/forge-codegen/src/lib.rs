//! Forge Codegen — 可扩展编译器后端框架 (v21)。
//!
//! # 架构
//!
//! 组件化 trait 体系，通过 [`machine::target::TargetMachine`] 组合成一个完整的 ISA 后端。
//! DSL (`isa_from_file!`，v12 唯一语法) 从 TOML 文件自动生成所有组件实现。
//!
//! ## 核心组件
//!
//! - [`machine::target::TargetMachine`] — 顶层入口，组合所有后端组件
//! - [`machine::lowering::TargetLowering`] — 指令选择 (IR → 机器指令)
//! - [`machine::encoder::TargetEncoder`] — 指令编码 (Inst → bytes)
//! - [`machine::abi::TargetABI`] — 调用约定
//! - [`machine::reg_info::TargetRegInfo`] — 寄存器文件描述
//! - [`machine::frame::TargetFrameLowering`] — 栈帧管理
//! - [`pipeline::regalloc_bt::BacktrackingAllocator`] — Backtracking 寄存器分配器 (唯一分配器)
//!
//! ## 核心类型
//!
//! - [`MachineInst`] — 机器指令 trait
//! - [`AllocResult`] — 寄存器分配结果
//! - [`CodeSink`] — 代码发射接收器
//! - [`Registry`] — 全局后端注册表

use forge_ir::IrError;
use forge_ir::*;
use std::collections::HashMap;

// ISA TOML 变更探针：build.rs 生成，内容含 isa/*.toml 的修改时间——
// TOML 变化会触发本 crate 重编（否则 proc-macro 读文件不触发，见 build.rs）。
include!(concat!(env!("OUT_DIR"), "/isa_probe.rs"));

// ============================================================
// Prelude — types needed by DSL-generated code (isa_from_file!)
// ============================================================

/// 内存引用 —— DSL `MemRef` 字段的 Rust 表示（基址 + 偏移 + 数据宽度）。
///
/// 供指令字段携带"内存操作数"（如 load/store 的地址）使用：lowering
/// 阶段从 `StackAddr` / `Iconst` 基址解析填充，编码阶段读取 `base`/`offset`
/// 发射地址。`width` 表示被访问数据的字节宽度。
///
/// 这是架构无关的通用结构（不引用任何 ISA 寄存器名），`base` 为物理
/// 寄存器编号（PReg index），由各 ISA 的 lowering 负责映射。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct MemRef {
    /// 基址物理寄存器编号（PReg index；未解析时为 [`MemRef::UNRESOLVED_BASE`]）。
    pub base: u32,
    /// 字节偏移（可为负）。
    pub offset: i32,
    /// 数据字节宽度（1/2/4/8/16）。
    pub width: u8,
}

impl MemRef {
    /// 未解析基址的哨兵值。
    pub const UNRESOLVED_BASE: u32 = u32::MAX;

    /// 构造已解析的内存引用。
    pub fn new(base: u32, offset: i32, width: u8) -> Self {
        Self {
            base,
            offset,
            width,
        }
    }

    /// 未解析占位（供 DSL 默认字段值 / 汇编解析未定基址使用）。
    pub fn unresolved(width: u8) -> Self {
        Self {
            base: Self::UNRESOLVED_BASE,
            offset: 0,
            width,
        }
    }

    /// 是否已解析出基址寄存器。
    pub fn is_resolved(&self) -> bool {
        self.base != Self::UNRESOLVED_BASE
    }
}

impl Default for MemRef {
    fn default() -> Self {
        Self::unresolved(8)
    }
}

pub mod prelude {
    pub use crate::{
        AllocResult, CodeSink, EffectKind, EncodeError, InstPacket, IsaCapabilities, IsaInfo,
        LowerCtx, MachineInst, MemRef, RegisterClassInfo, VBlockId, VCodeBlock, avx_available,
        avx2_available,
    };
    pub use forge_ir::{
        AtomicRmwOp, Block, ConstId, Endianness, FloatCC, FrameAccess, Immediate, Instruction,
        IntCC, IrError, Opcode, PReg, PhysReg, RegClass, Terminator, TypeId, VReg, Value, XReg,
        XRegAllocator,
    };
}

// ============================================================
// 新 Machine trait 体系 (v19)
// ============================================================
pub mod machine;

// ============================================================
// ============================================================
// 编码子系统 (v12 DSL 生成代码内联实现 encode/decode；无需运行时辅助模块)
// ============================================================

// ============================================================
// lowering → VCode → emit 管线 + 寄存器分配
// ============================================================
pub mod pipeline;

// ============================================================
// JIT / 运行时 / 注册表
// ============================================================
pub mod runtime;
pub use runtime::output_types::{CompiledFunction, RelocKind, Relocation};

// ============================================================
// 架构后端 (DSL 生成；v12 唯一语法)
// ============================================================
pub mod arch;
pub use arch::demo_v12;
pub use arch::riscv64_v12;
pub use arch::x86_v12;

// ============================================================
// 扩展（优化器、异常处理、调试）
// ============================================================
pub mod ext;

// ============================================================
// Module re-exports (for paths like code_forge::backend::pipeline::...)
// ============================================================
pub use ext::pattern_isel;
pub use pipeline::vcode;
#[cfg(feature = "jit")]
pub use runtime::jit;
pub use runtime::output_types;
pub use runtime::registry;

// ============================================================
// Shared type re-exports
// ============================================================
pub use pipeline::emit::CodeSink;
pub use pipeline::vcode::{VBlockId, VCode, VCodeBlock};

// ============================================================
// Machine re-exports (v19 — primary API)
// ============================================================
pub use machine::encoder::EncodeError;
pub use machine::encoder::TargetEncoder;
pub use machine::inst::{EffectKind, MachineInst};
pub use machine::isa_info::IsaCapabilities;
pub use machine::isa_info::IsaInfo;
pub use machine::isa_info::RegisterClassInfo;
pub use machine::lowering::InstPacket;
pub use machine::reloc_patcher::{RelocPatcher, RiscvRelocPatcher, X86RelocPatcher};
pub use machine::simulator::SimulationState;
pub use machine::target::{ErasedTargetMachine, TargetMachine};
pub use pipeline::alloc_config::RegAllocConfig;
pub use pipeline::compiler::FunctionCompiler;
pub use pipeline::regalloc_bt::BacktrackingAllocator;

// ============================================================
// Runtime re-exports
// ============================================================
pub use runtime::registry::Registry;

/// 指令选择上下文 — 在 lowering 阶段提供给 [`machine::lowering::TargetLowering`]。
pub struct LowerCtx {
    /// 下一个可用的虚拟寄存器号。
    pub next_vreg: u32,
    /// 临时寄存器（XReg）分配器 — XReg 的唯一受控创建入口。
    pub xregs: forge_ir::XRegAllocator,
    /// XReg → IR 类型映射（用于 `.if` 条件汇编中的类型查询）。
    pub xreg_types: HashMap<XReg, TypeId>,
    /// 当前函数的调用约定。
    pub call_conv: CallConv,
    /// 当前函数的常量池（用于解析 Iconst/Fconst 的索引）。
    pub constant_pool: Option<forge_ir::ConstantPool>,
    /// VReg → 寄存器类别映射（用于寄存器分配）。
    pub vreg_classes: HashMap<VReg, RegClass>,
    /// VReg → IR 类型映射（用于 `.if` 条件汇编中的类型查询）。
    pub vreg_types: HashMap<VReg, TypeId>,
    /// VReg → 字节宽度（用于寄存器分配器宽度感知）。
    pub vreg_widths: HashMap<VReg, u8>,
    /// 是否为浮点返回值（影响 Return 降低时使用 RetVal 还是 RetValFloat）。
    pub is_float_return: bool,
    /// 当前指令的默认操作数宽度 (8/16/32/64)，由 IR 类型推导。
    pub default_opsize: u8,
    /// 当前指令的常量索引 (从 Instruction.immediates 中提取)。
    pub current_const_index: u32,
    /// 当前指令的全部 immediates 值（Uint/Int/Const 提取后的 u64 列表；
    /// ShuffleVector 的 mask、Vextract/Vinsert 的 index 等使用）。
    pub current_immediates: Vec<u64>,
    /// 当前指令引用的函数 (Call 的 Immediate::Func；供 lowering 生成
    /// cross-function relocation 的符号名 "@N")。
    pub current_func_ref: Option<FuncRef>,
    /// 当前 AtomicRmw 的操作数 (Immediate::Uint(op as u64) 解析)。
    pub current_atomic_op: Option<forge_ir::opcode::AtomicRmwOp>,
    /// 当前指令引用的全局变量 (GlobalAddr 的 Immediate::Global)。
    pub current_global: Option<GlobalId>,
    /// StackAddr 的帧偏移（Immediate::Int）。
    pub current_offset: i64,
    /// 当前 Alloca 的帧槽偏移（预扫描分配；`lea_off rd, alloca_offset` 规则取用）。
    pub current_alloca_offset: i64,
    /// prologue 压入的 callee-saved 寄存器总字节数（局部变量 lea 的基准平移）。
    pub callee_saved_bytes: i32,
    /// 栈槽（StackAddr/Alloca）的帧顶平移字节数。x86 = callee_saved_bytes
    ///（槽在 rbp - callee_saved 之下）；riscv = fp_push_bytes（16，槽在
    /// fp - 16 之下，避开 ra/fp 保存槽）。独立于 callee_saved_bytes——
    /// riscv 的 spill 布局需 callee_saved_bytes=0（spill 槽帧内底部）而
    /// 栈槽平移需 16（fp 基准）。
    pub stack_slot_shift: i32,
    /// StackAddr 局部变量区需求（从 callee-saved 区底向下到最深槽的字节数）。
    /// calculate_frame_size 必须把它算进 sub rsp 的帧大小，否则局部槽落在
    /// rsp 之下（Windows 无 red zone）→ 写栈越界 SEGV（mini_c 参数内联场景）。
    pub max_stack_bytes: u32,
    /// 临时 VReg 集合（替代 VReg(96-100) 硬编码）。
    pub temp_vregs: HashSet<VReg>,
    /// 零值 VReg（复用，避免重复分配）。
    zero_vreg: Option<VReg>,
    /// 零值 XReg（复用，避免重复分配）。
    zero_xreg: Option<XReg>,
    /// 当前 lowering 包的写死物理寄存器（来自 insts 自动推导）——
    /// 生成代码在规则 arm 尾部设置，编译器聚合到 inst_clobbers 供分配器避开。
    /// 值 = (物理寄存器编号, 寄存器类)。
    pub current_clobbers: Vec<(u32, RegClass)>,
    /// 当前函数的类型上下文（动态 vector/scalable 类型 → 寄存器类的
    /// 位宽感知推导，消除 RegClass::from_type_id 对动态类型的 GPR(8) 兜底）。
    pub type_ctx: Option<forge_ir::TypeContext>,
}

use std::collections::HashSet;

/// AVX 可用性（OnceLock 缓存 cpuid 检测，供 VEX 编码宏断言）。
/// 非 x86_64 或检测失败 → false（V256 指令编码时 panic 提示）。
pub fn avx_available() -> bool {
    static AVX: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
    *AVX.get_or_init(|| {
        #[cfg(target_arch = "x86_64")]
        {
            std::arch::is_x86_feature_detected!("avx")
        }
        #[cfg(not(target_arch = "x86_64"))]
        {
            false
        }
    })
}

/// AVX2 可用性（整数 256 位 SIMD 的前提：VPADDD/VPADDQ ymm 等）。
/// 当前 x86 后端整数向量仅支持 ≤128 位（SSE2 PADDD/PADDQ）；整数 V256 落地时
/// 需在对应编码宏断言此函数（与 avx_available 同模式）。
pub fn avx2_available() -> bool {
    static AVX2: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
    *AVX2.get_or_init(|| {
        #[cfg(target_arch = "x86_64")]
        {
            std::arch::is_x86_feature_detected!("avx2")
        }
        #[cfg(not(target_arch = "x86_64"))]
        {
            false
        }
    })
}

impl LowerCtx {
    pub fn new() -> Self {
        Self {
            next_vreg: 0,
            xregs: forge_ir::XRegAllocator::new(),
            xreg_types: HashMap::new(),
            call_conv: CallConv::Default,
            constant_pool: None,
            is_float_return: false,
            current_const_index: 0,
            current_immediates: Vec::new(),
            current_func_ref: None,
            current_atomic_op: None,
            current_global: None,
            current_offset: 0,
            current_alloca_offset: 0,
            callee_saved_bytes: 0,
            stack_slot_shift: 0,
            max_stack_bytes: 0,
            vreg_classes: HashMap::new(),
            vreg_types: HashMap::new(),
            vreg_widths: HashMap::new(),
            default_opsize: 64,
            temp_vregs: HashSet::new(),
            zero_vreg: None,
            zero_xreg: None,
            current_clobbers: Vec::new(),
            type_ctx: None,
        }
    }

    /// 分配一个新的临时寄存器（XReg），类型由 `class` 严格限定。
    /// 被动语义：XReg 只能作为后续指令的 def（结果）被赋予值。
    pub fn alloc_xreg(&mut self, class: RegClass) -> XReg {
        self.xregs.alloc_default(class)
    }

    /// 位宽感知的寄存器类推导：动态 vector/scalable 类型按字节位宽映射
    /// （≤128 → VEC(16)、>128 → VEC(32)），其余回退 RegClass::from_type_id。
    pub fn reg_class_for(&self, ty: &TypeId) -> RegClass {
        if let Some(ctx) = &self.type_ctx {
            let store = ctx.borrow();
            if store.is_vector(*ty) || store.is_scalable_vector(*ty) {
                let bits = store.size_bytes(*ty) * 8;
                return if bits <= 128 {
                    RegClass::VEC(16)
                } else {
                    RegClass::VEC(32)
                };
            }
        }
        RegClass::from_type_id(*ty)
    }

    /// 分配一个指定宽度（8/16/32/64）的临时寄存器。
    pub fn alloc_xreg_with_width(&mut self, class: RegClass) -> XReg {
        self.xregs.alloc(class)
    }

    /// 分配或复用零值临时寄存器（用于比较零值）。
    pub fn alloc_zero_xreg(&mut self) -> XReg {
        if let Some(x) = self.zero_xreg {
            return x;
        }
        let x = self.alloc_xreg(RegClass::GPR64);
        self.zero_xreg = Some(x);
        x
    }

    /// 查询 XReg 的字节宽度（位宽内嵌在值上）。
    pub fn xreg_width(&self, x: XReg) -> u16 {
        x.width()
    }

    /// 查询 VReg 对应的 IR 类型是否为浮点类型。
    pub fn is_float_vreg(&self, vreg: VReg) -> bool {
        self.vreg_types
            .get(&vreg)
            .map(|t| self.reg_class_for(t).is_fp())
            .unwrap_or(false)
    }

    /// 从 TypeId 推导寄存器类（统一的集中处理）。
    pub fn reg_class_for_type(ty: TypeId) -> RegClass {
        RegClass::from_type_id(ty)
    }

    /// 从 IR 类型推导操作数宽度 (opsize)。
    ///
    /// 不枚举、不截断：1 字节 → 32（窄类型算术的寄存器安全折中，32 位写全
    /// 低 32 位避免残留高位参与 64 位运算）；2 → 16；4 → 32；u64 → 64；
    /// 自定义非常规宽度（如 12 字节 GPR）→ 真实字节数 ×8 原样传递——
    /// 是否可编码由目标 ISA 的指令定义决定，这里不静默截断成 64。
    ///
    /// ## 窄值高位约定（bool/i8/i16）
    /// 窄值（<32 位）在 64 位 GPR 中的高位由产生方保证清零：
    /// - `setcc` 类（icmp/fcmp/is_null/overflow flag）规则必须前置 `xor rd, rd`；
    /// - 32 位写（窄算术）在 x86/aarch64 硬件上自动零扩展高 32 位；
    /// - 消费方 `[lower.Uextend]` 按源宽度 movzx（x86）或 mov（32 位源）。
    ///   违反约定会在 uextend/64 位运算中携带高位垃圾（历史缺陷，已修复）。
    pub fn opsize_from_type(ty: &TypeId) -> u8 {
        match ty.bits().div_ceil(8) {
            0..=1 => 32,
            2 => 16,
            4 => 32,
            n => (n as u8).saturating_mul(8),
        }
    }

    /// 内存访问（load/store）的精确宽度——真实字节数，不枚举不截断：
    /// u8 → 8、u16 → 16、u32 → 32、u64 → 64、自定义非常规宽度 → 原样传递。
    /// 与 `opsize_from_type` 不同（后者对 <4 字节返回 32——寄存器安全折中，
    /// 窄类型算术用 32 位避免残留高位参与 64 位运算）；内存访问必须用真实
    /// 宽度，否则 u8 元素 load 读 4 字节（越界读到相邻槽垃圾）、store 写
    /// 4 字节（越界覆盖相邻槽）。
    pub fn mem_opsize_from_type(ty: &TypeId) -> u8 {
        (ty.bits().div_ceil(8) as u8).saturating_mul(8)
    }

    /// 内存访问（load/store）的精确宽度——聚合类型（struct/array）的
    /// `bits()` 为 0，`mem_opsize_from_type` 会错误返回 0（store 宽度 0 →
    /// 机器码缺陷/SEGV）；这里优先用 TypeStore::size_bytes（聚合 → 真实
    /// 字节数，如 {i32,i32} → 64 位），基础类型回退到 bits() 路径。
    pub fn mem_opsize_for(&self, ty: &TypeId) -> u8 {
        if let Some(tc) = &self.type_ctx {
            let store = tc.borrow();
            let bytes = store.size_bytes(*ty);
            if bytes > 0 {
                return (bytes as u8).saturating_mul(8).min(64);
            }
        }
        Self::mem_opsize_from_type(ty)
    }

    /// 分配一个新的整数类虚拟寄存器。
    pub fn alloc_vreg(&mut self) -> VReg {
        self.alloc_vreg_with_class(RegClass::GPR64)
    }

    /// 分配一个指定寄存器类别的虚拟寄存器。
    pub fn alloc_vreg_with_class(&mut self, class: RegClass) -> VReg {
        let vreg = VReg(self.next_vreg);
        self.next_vreg += 1;
        self.vreg_classes.insert(vreg, class);
        self.vreg_widths.insert(vreg, class.default_width());
        vreg
    }

    /// 分配一个临时 VReg（用于 lowering 中的中间值）。
    /// 替代硬编码的 VReg(96)/VReg(97)/VReg(100)。
    pub fn alloc_temp_vreg(&mut self, class: RegClass) -> VReg {
        let vreg = self.alloc_vreg_with_class(class);
        self.temp_vregs.insert(vreg);
        vreg
    }

    /// 分配或复用零值 VReg（用于比较零值）。
    pub fn alloc_zero_vreg(&mut self) -> VReg {
        if let Some(vreg) = self.zero_vreg {
            return vreg;
        }
        let vreg = self.alloc_temp_vreg(RegClass::GPR64);
        self.zero_vreg = Some(vreg);
        vreg
    }
}

impl Default for LowerCtx {
    fn default() -> Self {
        Self::new()
    }
}

/// 寄存器位置 — 表示一个虚拟寄存器被分配到哪里。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RegLocation {
    /// 分配在物理寄存器中。
    Reg(PReg),
    /// 溢出到栈上（偏移量）。
    Stack(i32),
    /// 未知（未分配）
    Unknown,
}

/// 寄存器映射 — 寄存器分配的输出结果。
/// v20: AllocResult 现在是 AllocResult 的类型别名。
/// 旧 AllocResult struct 已删除；所有字段通过 AllocResult 方法访问。
pub type AllocResult = crate::pipeline::alloc_result::AllocResult;

#[cfg(test)]
mod memref_tests {
    use super::MemRef;

    #[test]
    fn test_memref_api() {
        let m = MemRef::new(5, -8, 8);
        assert!(m.is_resolved());
        assert_eq!(m.base, 5);
        assert_eq!(m.offset, -8);
        assert_eq!(m.width, 8);

        let u = MemRef::unresolved(4);
        assert!(!u.is_resolved());
        assert_eq!(u.base, MemRef::UNRESOLVED_BASE);

        let d = MemRef::default();
        assert!(!d.is_resolved());
        assert_eq!(d.width, 8);

        // Copy + Eq（字段在 Inst 枚举中要求）
        let c = m;
        assert_eq!(c, m);
    }
}
