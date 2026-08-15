//! TargetMachine — 顶层后端入口 trait。
//!
//! 组合所有组件 trait，形成一个完整的 ISA 后端。
//! 由 DSL 生成或手动实现。

use std::sync::Arc;

use super::abi::TargetABI;
use super::assembler::TargetAssembler;
use super::decoder::TargetDecoder;
use super::disasm::TargetDisassembler;
use super::encoder::TargetEncoder;
use super::frame::TargetFrameLowering;
use super::inst::MachineInst;
use super::isa_info::IsaInfo;
use super::lowering::TargetLowering;
use super::peephole::TargetPeephole;
use super::reg_info::TargetRegInfo;
use super::reloc_patcher::RelocPatcher;
use super::simulator::TargetSimulator;
use forge_ir::PhysReg;

/// ISA 后端的完整描述。
///
/// 不包含逻辑，仅持有各组件的 `Arc` 引用。
/// 管线（FunctionCompiler）通过此 trait 访问所需组件。
///
/// # 实现方式
///
/// 1. **DSL 生成**: `isa_from_file!` 读取 TOML → 生成 struct 实现此 trait
/// 2. **手动实现**: 手动实现各组件 trait，组合为 TargetMachine
pub trait TargetMachine: Send + Sync + 'static {
    /// 机器指令类型。
    type Inst: MachineInst;
    /// 物理寄存器类型。
    type Reg: PhysReg;

    // ── 必需组件 ──

    /// ISA 静态元数据。
    fn isa_info(&self) -> &Arc<dyn IsaInfo>;

    /// 寄存器文件描述。
    fn reg_info(&self) -> &Arc<dyn TargetRegInfo<Reg = Self::Reg>>;

    /// 调用约定。
    fn abi(&self) -> &Arc<dyn TargetABI<Reg = Self::Reg>>;

    /// 指令选择。
    fn lowering(&self) -> &Arc<dyn TargetLowering<Inst = Self::Inst>>;

    /// 指令编码。
    fn encoder(&self) -> &Arc<dyn TargetEncoder<Inst = Self::Inst>>;

    /// 栈帧管理。
    fn frame_lowering(&self) -> &Arc<dyn TargetFrameLowering<Inst = Self::Inst>>;

    // ── 可选组件 ──

    /// 窥孔优化（可选）。
    /// Optional IR-level pattern matcher (Stage 3). ISAs that support
    /// instruction fusion (e.g. LEA / CMOV / FMA) return a matcher with
    /// registered patterns; others return `None` and skip the pass.
    fn pattern_matcher(&self) -> Option<&crate::ext::pattern_isel::PatternMatcher> {
        None
    }

    /// Optional peephole pass (runs after instruction selection).
    fn peephole(&self) -> Option<&Arc<dyn TargetPeephole<Inst = Self::Inst>>> {
        None
    }

    /// 指令解码器（可选）。
    fn decoder(&self) -> Option<&Arc<dyn TargetDecoder<Inst = Self::Inst>>> {
        None
    }

    /// 重定位编码器（可选）。默认按 ISA 名查后端注册表；后端也可覆盖。
    /// 用于把 PC 相对/绝对 relocation 按 ISA 编码写入代码（x86 rel32、
    /// AArch64 BL imm26、RISC-V B/J 位重组）。
    fn reloc_patcher(&self) -> Option<Arc<dyn RelocPatcher>> {
        crate::machine::reloc_patcher::reloc_patcher_for(self.isa_info().name())
    }

    /// 反汇编器（可选）。
    fn disassembler(&self) -> Option<&Arc<dyn TargetDisassembler<Inst = Self::Inst>>> {
        None
    }

    /// 汇编器（可选）。
    fn assembler(&self) -> Option<&Arc<dyn TargetAssembler<Inst = Self::Inst>>> {
        None
    }

    /// 模拟器（可选）。
    fn simulator(&self) -> Option<&Arc<dyn TargetSimulator<Inst = Self::Inst>>> {
        None
    }
}

// ============================================================
// ErasedTargetMachine — 类型擦除的 TargetMachine
// ============================================================

/// 类型擦除的 TargetMachine，用于 Registry 存储。
///
/// 通过 `impl_erased_target_machine!` 宏为具体类型实现。
pub trait ErasedTargetMachine: Send + Sync {
    /// 后端名称（对应 ISA 名称）。
    fn erased_name(&self) -> &str;
    /// 编译一个 IR 函数（类型擦除路径，用于 Registry 查找）。
    fn compile(
        &self,
        func: &forge_ir::Function,
    ) -> Result<crate::CompiledFunction, forge_ir::IrError>;
}

/// 为具体 TargetMachine 类型实现 ErasedTargetMachine。
///
/// 要求目标类型实现 `Clone`（所有字段为 `Arc`，因此廉价）。
///
/// # Example
/// ```ignore
/// impl_erased_target_machine!(X86TargetMachine);
/// ```
#[macro_export]
macro_rules! impl_erased_target_machine {
    ($tm:ty) => {
        impl $crate::machine::target::ErasedTargetMachine for $tm {
            fn erased_name(&self) -> &str {
                $crate::machine::target::TargetMachine::isa_info(self).name()
            }

            fn compile(
                &self,
                func: &forge_ir::Function,
            ) -> Result<$crate::CompiledFunction, forge_ir::IrError> {
                let compiler = $crate::pipeline::compiler::FunctionCompiler::new(self.clone());
                compiler.compile_raw(func)
            }
        }
    };
}
