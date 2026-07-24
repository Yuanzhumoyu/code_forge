//! codegen-lib — 支持外部注册自定义指令集的代码生成库。
//!
//! # 快速开始
//!
//! ```ignore
//! use codegen_lib::prelude::*;
//!
//! // 1. 定义你的机器指令
//! #[derive(Clone, Debug)]
//! enum MyInst { Add { rd: VReg, rs1: VReg, rs2: VReg }, Ret, ... }
//!
//! impl MachineInst for MyInst { ... }
//!
//! // 2. 实现你的 ISA
//! struct MyIsa;
//! impl InstructionSet for MyIsa {
//!     type Inst = MyInst;
//!     fn name() -> &'static str { "my-arch" }
//!     fn lower(op: &Opcode, args: &[VReg], result: Option<VReg>, ctx: &mut LowerCtx)
//!         -> Result<Vec<MyInst>, CompileError> { ... }
//!     fn emit(inst: &MyInst, reg_map: &RegMap, sink: &mut CodeSink)
//!         -> Result<(), CompileError> { ... }
//!     fn lower_terminator(term: &Terminator, ...)
//!         -> Result<Vec<MyInst>, CompileError> { ... }
//! }
//!
//! // 3. 注册后端
//! register_backend!(MyIsa);
//!
//! // 4. 编译函数
//! let code = compile::<MyIsa>(
//!     "add",
//!     &Signature::new(&[(Type::I32, "a"), (Type::I32, "b")], &[Type::I32]),
//!     |b| {
//!         let entry = b.create_block_with_params(&[(Type::I32, "a"), (Type::I32, "b")]);
//!         b.switch_to_block(entry.0);
//!         let sum = b.iadd(entry.1[0], entry.1[1]);
//!         b.return_(&[sum]);
//!     },
//! );
//! ```

// ============================================================
// 模块声明
// ============================================================

// 核心层 — IR 类型系统与优化管线
pub mod ir;
pub mod optimize;

// 平台抽象 — ABI、主机 CPU 检测、可执行内存
pub mod abi;
pub mod host_cpu;
pub mod executable_memory;

// 后端框架 — 代码生成基础设施
pub mod backend;

// 寄存器分配 — trait 抽象层 (具体实现在 backend::regalloc)
pub mod regalloc_adapter;

// 汇编器 — ASM 文本 → 机器码 / JIT
pub mod assembler;

// JIT 编译器 — 运行时代码生成
pub mod jit;

// 可选模块 — Object 文件写入 (需要 object-file feature)
#[cfg(feature = "object-file")]
pub mod target;
#[cfg(feature = "object-file")]
pub mod object_writer;

// 可选模块 — 插件系统 (需要 plugins feature)
#[cfg(feature = "plugins")]
pub mod plugin;

// ============================================================
// 关键子模块公开导出 (平铺到 crate 根)
// ============================================================
pub use abi::*;
pub use backend::*;
pub use ir::*;
pub use regalloc_adapter::*;

// ============================================================
// 核心类型
// ============================================================

/// 编译完成的函数。
#[derive(Debug, Clone)]
pub struct CompiledFunction {
    /// 机器码字节。
    pub code: Vec<u8>,
    /// 重定位信息。
    pub relocations: Vec<Relocation>,
    /// 机器码大小（字节）。
    pub code_size: usize,
}

/// 重定位记录。
#[derive(Debug, Clone)]
pub struct Relocation {
    /// 需要重定位的偏移量。
    pub offset: usize,
    /// 重定位类型。
    pub kind: RelocKind,
    /// 符号名（外部符号时使用）。
    pub symbol: String,
    /// 加数。
    pub addend: i64,
}

/// 重定位类型 — 数据驱动设计，无需为每个架构新增变体。
///
/// # 变体
///
/// - `Abs(width_bytes)`: 绝对地址重定位
/// - `Rel(width_bytes, adjustment)`: PC 相对重定位
/// - `Isa(id)`: 架构特定重定位，由 `InstructionSet::encode_isa_reloc` 处理
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RelocKind {
    /// 绝对地址重定位。`width_bytes` ∈ {1, 2, 4, 8}。
    Abs(u8),
    /// PC 相对重定位。`width_bytes` ∈ {1, 2, 4, 8}。
    /// `adjustment` 是偏移修正值（x86: -width_bytes，ARM: 0）。
    Rel(u8, i8),
    /// 架构特定重定位。由 `InstructionSet::encode_isa_reloc` 处理。
    /// `id` 由 ISA 自定义（ARM64 Branch26=0, RISC-V B-type=1, J-type=2）。
    Isa(u8),
}

impl RelocKind {
    /// 4 字节绝对地址。
    pub const ABS4: Self = RelocKind::Abs(4);
    /// 8 字节绝对地址。
    pub const ABS8: Self = RelocKind::Abs(8);
    /// 4 字节 PC 相对偏移（x86 rel32: target - offset - 4）。
    pub const REL4: Self = RelocKind::Rel(4, -4);
    /// 8 字节 PC 相对偏移。
    pub const REL8: Self = RelocKind::Rel(8, -8);
    /// 调用重定位（同 REL4）。
    pub const CALL: Self = RelocKind::Rel(4, -4);
    /// 间接调用重定位（绝对地址 8 字节）。
    pub const CALL_IND: Self = RelocKind::Abs(8);
    /// 1 字节 PC 相对偏移（x86 短分支: target - offset - 1）。
    pub const REL1: Self = RelocKind::Rel(1, -1);
    /// ARM64 B/BL 分支（imm26 位域编码）。
    pub const ARM64_BRANCH26: Self = RelocKind::Isa(0);
    /// RISC-V B-type 分支。
    pub const RISCV_BRANCH: Self = RelocKind::Isa(1);
    /// RISC-V J-type 跳转。
    pub const RISCV_JAL: Self = RelocKind::Isa(2);
}

/// 编译错误。
#[derive(thiserror::Error, Debug)]
pub enum CompileError {
    #[error("Lowering failed: {0}")]
    Lowering(String),

    #[error("Register allocation failed: {0}")]
    RegAlloc(String),

    #[error("Code emission failed: {0}")]
    Emit(String),

    #[error("Backend not found: {0}")]
    BackendNotFound(String),

    #[error("Unsupported operation: {0}")]
    Unsupported(String),

    #[error("Type mismatch: {0}")]
    TypeError(String),

    #[error("ABI error: {0}")]
    AbiError(String),

    #[error("Link error: {0}")]
    LinkError(String),

    #[error("Internal error: {0}")]
    Internal(String),

    #[error("Not yet implemented: {0}")]
    Unimplemented(String),
}

// ============================================================
// 便捷编译函数
// ============================================================

/// 从 Module 中编译一个函数（使用模块级优化管道）。
///
/// 运行完整的优化管道（内联 → const fn 求值 → 常量折叠 → CSE → DCE → 复制传播），
/// 然后进行 lowering 和代码发射。
///
/// # Example
///
/// ```ignore
/// let mut module = Module::new();
/// let func_ref = module.add_function(my_func)?;
/// let compiled = compile_from_module::<MyIsa>(&mut module, func_ref)?;
/// ```
pub fn compile_from_module<I: crate::backend::InstructionSet>(
    module: &mut crate::ir::Module,
    func_ref: crate::ir::FuncRef,
) -> Result<CompiledFunction, CompileError> {
    // 先运行模块级优化，然后直接编译（不再重复优化）
    module.optimize()?;
    let func = module.get(func_ref).ok_or_else(|| {
        CompileError::Internal(format!("function {:?} not found in module", func_ref))
    })?;
    crate::backend::FunctionCompiler::<I>::compile_raw(func)
}

/// 编译 Module 中的所有函数。
///
/// 返回 `Vec<(FuncRef, CompiledFunction)>`，每个函数编译一次。
/// 先运行一次模块级优化，然后逐个直接编译。
pub fn compile_module_all<I: crate::backend::InstructionSet>(
    module: &mut crate::ir::Module,
) -> Result<Vec<(crate::ir::FuncRef, CompiledFunction)>, CompileError> {
    module.optimize()?;

    let func_refs = module.func_refs();

    #[cfg(feature = "parallel")]
    {
        use rayon::prelude::*;
        func_refs
            .par_iter()
            .map(|&func_ref| {
                let func = module.get(func_ref).ok_or_else(|| {
                    CompileError::Internal(format!("function {:?} not found in module", func_ref))
                })?;
                let compiled = crate::backend::FunctionCompiler::<I>::compile_raw(func)?;
                Ok((func_ref, compiled))
            })
            .collect::<Result<Vec<_>, CompileError>>()
    }
    #[cfg(not(feature = "parallel"))]
    {
        let mut results = Vec::with_capacity(func_refs.len());
        for &func_ref in &func_refs {
            let func = module.get(func_ref).ok_or_else(|| {
                CompileError::Internal(format!("function {:?} not found in module", func_ref))
            })?;
            let compiled = crate::backend::FunctionCompiler::<I>::compile_raw(func)?;
            results.push((func_ref, compiled));
        }
        Ok(results)
    }
}

// ============================================================
// Prelude
// ============================================================

/// 便捷导入模块 — 包含使用 codegen-lib 所需的所有核心类型和宏。
pub mod prelude {
    pub use crate::abi::*;
    pub use crate::backend::{
        CodeSink, Disassembler, Encoder, InstructionSet, IsaCompiler, LowerCtx, MachineInst,
        RegLocation, RegMap, Registry, VCodeBlockId, compile,
    };
    pub use crate::backend::{
        EffectKind, FormatInfo, IsaCapabilities, IsaInfo, MachineInst as MachineInstV9, MemAccess,
        MemAccessKind, RegisterClassInfo,
    };
    pub use crate::executable_memory::ExecutableMemory;
    pub use crate::ir::{
        BlockId, CallConv, Endianness, FloatCC, FrameAccess, FuncRef, Function, FunctionBuilder,
        IntCC, Module, Opcode, PReg, PhysReg, RegClass, Signature, Terminator, Type, VReg, Value,
    };
    pub use crate::optimize::{
        ConstFnEvalPass, ConstFoldPass, ConstValue, CopyPropPass, CsePass, DeadCodeElimPass,
        FunctionPass, GvnPass, InlinePass, IrInterpreter, JumpThreadPass, Mem2RegPass, ModulePass,
        OptimizationLevel, OptimizationPass, PassManager, PassResult, PassRunMode,
    };
    #[cfg(feature = "plugins")]
    pub use crate::plugin::PluginLoader;
    pub use crate::{CompileError, CompiledFunction, RelocKind, Relocation, register_backend};
}

/// 增强导出模块 — 包含 prelude 的所有内容，外加 object-file 相关的类型。
///
/// 需要启用 `object-file` feature。
#[cfg(feature = "object-file")]
pub mod extended {
    pub use crate::object_writer::ObjectWriter;
    pub use crate::prelude::*;
    pub use crate::target::TargetConfig;
}
