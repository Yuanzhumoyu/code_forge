//! Forge Codegen — 可扩展编译器后端框架 (v21)。
//!
//! # 架构
//!
//! 组件化 trait 体系，通过 [`machine::target::TargetMachine`] 组合成一个完整的 ISA 后端。
//! DSL (`isa_from_file!`) 从 TOML 文件自动生成所有组件实现。
//!
//! ## 核心组件
//!
//! - [`machine::target::TargetMachine`] — 顶层入口，组合所有后端组件
//! - [`machine::lowering::TargetLowering`] — 指令选择 (IR → 机器指令)
//! - [`machine::encoder::TargetEncoder`] — 指令编码 (Inst → bytes)
//! - [`machine::abi::TargetABI`] — 调用约定
//! - [`machine::reg_info::TargetRegInfo`] — 寄存器文件描述
//! - [`machine::frame::TargetFrameLowering`] — 栈帧管理
//! - [`pipeline::regalloc_trait::RegisterAllocator`] — Backtracking 寄存器分配器 (唯一分配器)
//!
//! ## 核心类型
//!
//! - [`MachineInst`] — 机器指令 trait
//! - [`AllocResult`] — 寄存器分配结果
//! - [`CodeSink`] — 代码发射接收器
//! - [`Registry`] — 全局后端注册表

use forge_ir::CompileError;
use forge_ir::*;
use std::collections::HashMap;

// ============================================================
// Assembler runtime types (from forge-asm, consumed by DSL-generated code)
// ============================================================
pub use forge_asm as assembler;

// ============================================================
// Prelude — types needed by DSL-generated code (isa_from_file!)
// ============================================================
pub mod prelude {
    pub use crate::{
        AllocResult, CodeSink, EffectKind, EncodeError, InstPacket, IsaCapabilities, IsaInfo,
        LowerCtx, MachineInst, RegisterClassInfo, VBlockId, VCodeBlock,
    };
    pub use forge_ir::{
        AtomicRmwOp, Block, CompileError, ConstId, Endianness, FloatCC, FrameAccess, Immediate,
        Instruction, IntCC, Opcode, PReg, PhysReg, RegClass, Terminator, TypeId, VReg, Value, XReg,
        XRegAllocator,
    };
}

// ============================================================
// 新 Machine trait 体系 (v19)
// ============================================================
pub mod machine;

// ============================================================
// ISA-agnostic encoding primitives (v19)
// ============================================================
pub mod primitives;

// ============================================================
// 编码子系统 (x86 runtime helpers — called by DSL-generated code)
// ============================================================
pub mod encode;

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
// 架构后端 (DSL 生成)
// ============================================================
pub mod arch;
pub use arch::aarch64;
// 临时排除：minimal_sd/wasm32 的 default_lowering 分支有 3 个生成代码类型错误未定位
// pub use arch::minimal_sd_test;
pub use arch::riscv64;
// pub use arch::wasm32;
pub use arch::x86_64;

// ============================================================
// 扩展（优化器、异常处理、调试）
// ============================================================
pub mod ext;

// ============================================================
// Module re-exports (for paths like code_forge::backend::pipeline::...)
// ============================================================
pub use ext::pattern_isel;
pub use pipeline::vcode;
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
pub use encode::packer::BitField;
pub use machine::assembler::AsmError;
pub use machine::decoder::DecodeError;
pub use machine::encoder::EncodeError;
pub use machine::lowering::InstPacket;
pub use machine::encoder::TargetEncoder;
pub use machine::inst::{EffectKind, MachineInst};
pub use machine::isa_info::IsaCapabilities;
pub use machine::isa_info::IsaInfo;
pub use machine::isa_info::RegisterClassInfo;
pub use machine::simulator::{SimError, SimulationState};
pub use machine::target::{ErasedTargetMachine, TargetMachine};
pub use pipeline::alloc_config::RegAllocConfig;
pub use pipeline::compiler::FunctionCompiler;
pub use pipeline::regalloc_trait::RegisterAllocator;

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
    /// 当前指令引用的函数 (Call 的 Immediate::Func；供 lowering 生成
    /// cross-function relocation 的符号名 "@N")。
    pub current_func_ref: Option<FuncRef>,
    /// 当前 AtomicRmw 的操作数 (Immediate::Uint(op as u64) 解析)。
    pub current_atomic_op: Option<forge_ir::opcode::AtomicRmwOp>,
    /// 当前指令引用的全局变量 (GlobalAddr 的 Immediate::Global)。
    pub current_global: Option<GlobalId>,
    /// StackAddr 的帧偏移（Immediate::Int）。
    pub current_offset: i64,
    /// 临时 VReg 集合（替代 VReg(96-100) 硬编码）。
    pub temp_vregs: HashSet<VReg>,
    /// 零值 VReg（复用，避免重复分配）。
    zero_vreg: Option<VReg>,
    /// 零值 XReg（复用，避免重复分配）。
    zero_xreg: Option<XReg>,
}

use std::collections::HashSet;

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
            current_func_ref: None,
            current_atomic_op: None,
            current_global: None,
            current_offset: 0,
            vreg_classes: HashMap::new(),
            vreg_types: HashMap::new(),
            vreg_widths: HashMap::new(),
            default_opsize: 64,
            temp_vregs: HashSet::new(),
            zero_vreg: None,
            zero_xreg: None,
        }
    }

    /// 分配一个新的临时寄存器（XReg），类型由 `class` 严格限定。
    /// 被动语义：XReg 只能作为后续指令的 def（结果）被赋予值。
    pub fn alloc_xreg(&mut self, class: RegClass) -> XReg {
        let x = self.xregs.alloc_default(class);
        x
    }

    /// 分配一个指定宽度（8/16/32/64）的临时寄存器。
    pub fn alloc_xreg_with_width(&mut self, class: RegClass, width: u8) -> XReg {
        let x = self.xregs.alloc(class, width);
        x
    }

    /// 分配或复用零值临时寄存器（用于比较零值）。
    pub fn alloc_zero_xreg(&mut self) -> XReg {
        if let Some(x) = self.zero_xreg {
            return x;
        }
        let x = self.alloc_xreg(RegClass::GPR);
        self.zero_xreg = Some(x);
        x
    }

    /// 查询 XReg 的字节宽度（位宽内嵌在值上）。
    pub fn xreg_width(&self, x: XReg) -> u8 {
        x.width()
    }

    /// 查询 VReg 对应的 IR 类型是否为浮点类型。
    pub fn is_float_vreg(&self, vreg: VReg) -> bool {
        self.vreg_types
            .get(&vreg)
            .map(|t| RegClass::from_type_id(*t).is_fp())
            .unwrap_or(false)
    }

    /// 从 TypeId 推导寄存器类（统一的集中处理）。
    pub fn reg_class_for_type(ty: TypeId) -> RegClass {
        RegClass::from_type_id(ty)
    }

    /// 从 IR 类型推导操作数宽度 (opsize)。
    pub fn opsize_from_type(ty: &TypeId) -> u8 {
        match ty.bits().div_ceil(8) {
            0..=1 => 32,
            2 => 16,
            4 => 32,
            _ => 64,
        }
    }

    /// 分配一个新的整数类虚拟寄存器。
    pub fn alloc_vreg(&mut self) -> VReg {
        self.alloc_vreg_with_class(RegClass::GPR)
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
        let vreg = self.alloc_temp_vreg(RegClass::GPR);
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
