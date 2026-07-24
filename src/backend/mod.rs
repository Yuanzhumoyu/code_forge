#![allow(unused_imports)]
//! 后端框架。
//!
//! 提供可扩展的代码生成基础设施：
//! - [`MachineInst`] — 用户定义机器指令需实现的 trait
//! - [`InstructionSet`] — 用户定义 ISA 需实现的 trait
//! - [`RegMap`] — 寄存器分配结果
//! - [`CodeSink`] — 代码发射接收器
//! - [`Registry`] — 全局后端注册表

use crate::ir::*;
use crate::CompileError;
use std::collections::HashMap;

// ============================================================
// 编码子系统
// ============================================================
pub mod encode;

// ============================================================
// 后端 trait 与核心接口
// ============================================================
pub mod asm_trait;
pub mod composer;
pub mod decode_trait;
pub mod disasm_trait;
pub mod encode_trait;
pub mod instruction_set;
pub mod isa_info;
pub mod machine_inst;
pub mod sim_trait;

// ============================================================
// lowering → VCode → emit 管线
// ============================================================
pub mod lowering;
pub mod vcode;
pub mod emit;
pub mod debug_emit;
pub mod eh_frame;

// ============================================================
// 寄存器分配
// ============================================================
pub mod regalloc;

// ============================================================
// 后端优化
// ============================================================
pub mod peephole;
pub mod scheduler;

// ============================================================
// 基于模式的指令选择
// ============================================================
pub mod pattern_isel;

// ============================================================
// 架构支持
// ============================================================
pub mod x86_64;
pub mod wasm32;

// ============================================================
// COFF / PE 异常处理 (Windows x64)
// ============================================================
// ============================================================
pub mod coff_eh;

// ============================================================
// 架构定义 (编译期验证)
// ============================================================
pub mod minimal_sd_test;
pub mod riscv64_test;

// ============================================================
// 全局注册表
// ============================================================
pub mod registry;

pub use emit::*;
pub use encode::*;
pub use lowering::*;
pub use registry::*;
pub use vcode::*;

// ============================================================

// ============================================================
// InstructionSet trait
// ============================================================

/// 指令选择上下文 — 在 lowering 阶段提供给 [`InstructionSet::lower`]。
pub struct LowerCtx {
    /// 下一个可用的虚拟寄存器号。
    pub next_vreg: u32,
    /// 当前函数的调用约定。
    pub call_conv: CallConv,
    /// 当前函数的常量池（用于解析 Iconst/Fconst 的索引）。
    pub constant_pool: Option<crate::ir::ConstantPool>,
    /// VReg → 寄存器类别映射（用于寄存器分配）。
    pub vreg_classes: HashMap<VReg, RegClass>,
    /// 是否为浮点返回值（影响 Return 降低时使用 RetVal 还是 RetValFloat）。
    pub is_float_return: bool,
    /// 当前指令的默认操作数宽度 (8/16/32/64)，由 IR 类型推导。
    /// 在调用 `InstructionSet::lower` 之前由 FunctionCompiler 设置。
    pub default_opsize: u8,
}

impl LowerCtx {
    pub fn new() -> Self {
        Self {
            next_vreg: 0,
            call_conv: CallConv::Default,
            constant_pool: None,
            is_float_return: false,
            vreg_classes: HashMap::new(),
            default_opsize: 64,
        }
    }

    /// 从 IR 类型推导操作数宽度 (opsize)。
    /// 将 IR 类型映射为 x86 opsize: 8→8, 16→16, 32→32, 64+→64。
    /// 浮点类型返回 64（SSE 操作始终为 64/128-bit，不使用 opsize 前缀）。
    pub fn opsize_from_type(ty: &crate::ir::Type) -> u8 {
        match ty.size_bytes() {
            0..=1 => 32,  // Bool/I8: default to 32-bit (8-bit x86 uses separate opcodes)
            2 => 16,
            4 => 32,
            _ => 64,       // 8+ bytes: 64-bit
        }
    }

    /// 分配一个新的整数类虚拟寄存器。
    pub fn alloc_vreg(&mut self) -> VReg {
        self.alloc_vreg_with_class(RegClass::Int)
    }

    /// 分配一个指定寄存器类别的虚拟寄存器。
    pub fn alloc_vreg_with_class(&mut self, class: RegClass) -> VReg {
        let vreg = VReg(self.next_vreg);
        self.next_vreg += 1;
        self.vreg_classes.insert(vreg, class);
        vreg
    }
}

impl Default for LowerCtx {
    fn default() -> Self {
        Self::new()
    }
}

/// 寄存器位置 — 表示一个虚拟寄存器被分配到哪里。
#[derive(Clone, Debug)]
pub enum RegLocation {
    /// 分配在物理寄存器中。
    Reg(PReg),
    /// 溢出到栈上（偏移量）。
    Stack(i32),
}

/// 寄存器映射 — 寄存器分配的输出结果。
pub struct RegMap {
    pub vreg_to_preg: HashMap<VReg, PReg>,
    pub spills: HashMap<VReg, i32>,
    /// 参数 VReg 列表（用于 prologue 中从 ABI 寄存器复制参数）
    pub param_vregs: Vec<VReg>,
}

impl RegMap {
    pub fn new() -> Self {
        Self {
            vreg_to_preg: HashMap::new(),
            spills: HashMap::new(),
            param_vregs: Vec::new(),
        }
    }

    /// 解析虚拟寄存器到物理寄存器。
    /// 对于溢出的 VReg，返回错误 — 调用者应先检查 `is_spilled()` 并使用 `resolve_or_spill()`。
    pub fn resolve(&self, vreg: VReg) -> Result<PReg, CompileError> {
        self.vreg_to_preg.get(&vreg).copied().ok_or_else(|| {
            if self.spills.contains_key(&vreg) {
                CompileError::RegAlloc(format!(
                    "vreg {} is spilled to stack (offset {}), not in a register",
                    vreg,
                    self.spills.get(&vreg).unwrap_or(&-1)
                ))
            } else {
                CompileError::RegAlloc(format!("unallocated vreg {}", vreg))
            }
        })
    }

    /// 检查 VReg 是否已溢出到栈上。
    pub fn is_spilled(&self, vreg: VReg) -> bool {
        self.spills.contains_key(&vreg)
    }

    /// 获取溢出 VReg 的栈偏移量。
    pub fn spill_offset(&self, vreg: VReg) -> Option<i32> {
        self.spills.get(&vreg).copied()
    }

    /// 解析虚拟寄存器，支持溢出回退。
    pub fn resolve_or_spill(&self, vreg: VReg) -> Result<RegLocation, CompileError> {
        if let Some(&preg) = self.vreg_to_preg.get(&vreg) {
            Ok(RegLocation::Reg(preg))
        } else if let Some(&offset) = self.spills.get(&vreg) {
            Ok(RegLocation::Stack(offset))
        } else {
            Err(CompileError::RegAlloc(format!("unknown vreg {}", vreg)))
        }
    }

    /// 插入一个虚拟寄存器到物理寄存器的映射。
    pub fn insert(&mut self, vreg: VReg, preg: PReg) {
        self.vreg_to_preg.insert(vreg, preg);
    }

    /// 插入一个溢出槽映射。
    pub fn insert_spill(&mut self, vreg: VReg, offset: i32) {
        self.spills.insert(vreg, offset);
    }
}

impl Default for RegMap {
    fn default() -> Self {
        Self::new()
    }
}

// 指令集 trait — 用户实现的核心接口（re-export）。
pub use asm_trait::*;
pub use composer::*;
pub use decode_trait::*;
pub use disasm_trait::*;
pub use encode_trait::*;
pub use instruction_set::*;
pub use isa_info::*;
pub use machine_inst::*;
pub use sim_trait::*;
