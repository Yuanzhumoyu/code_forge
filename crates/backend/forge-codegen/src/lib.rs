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

// ISA-DSL 生成代码引用 `crate::IrError`（`isa_from_file!(…, krate = 本 crate)`
// 时即 `forge_codegen::IrError`）——必须**公开**，否则生成物只能活在库内部
// （demo 夹具迁到 tests/ 正是踩到这一点）。
pub use forge_ir::IrError;
use forge_ir::*;
pub use forge_isa_runtime::{
    AVX512_ENV_LOCK, AllocResult, CodeSink, CompiledFunction, LowerCtx, MemRef, Registry,
    RelocKind, Relocation, alloc_result, avx_available, avx2_available, avx512_available,
    avx512_hardware_available, emit, machine, output_types, prelude, registry, vcode,
};

// ISA TOML 变更探针：build.rs 生成，内容含 isa/*.toml 的修改时间——
// TOML 变化会触发本 crate 重编（否则 proc-macro 读文件不触发，见 build.rs）。
include!(concat!(env!("OUT_DIR"), "/isa_probe.rs"));

// ============================================================
// Prelude — types needed by DSL-generated code (isa_from_file!)
// ============================================================

// ============================================================
// ISA-DSL 生成代码的运行面（generated-code runtime surface）
// ============================================================
// `isa_from_file!(..., krate = <宿主>)` 会把生成物里的 `forge_ir::…` 改写为
// `<宿主>::ir::…`——本 re-export 就是那个 `ir`。宿主因此**不需要**自己依赖
// forge-ir，生成代码的全部依赖都落在本 crate 的公开面上。
pub use forge_ir as ir;

// ============================================================
// 新 Machine trait 体系 (v19)
// ============================================================

// ============================================================
// ============================================================
// 编码子系统 (v12 DSL 生成代码内联实现 encode/decode；无需运行时辅助模块)
// ============================================================

// ============================================================
// lowering → VCode → emit 管线 + 寄存器分配
// ============================================================
mod erased_macro;

pub mod pipeline;

// ============================================================
// JIT / 运行时 / 注册表
// ============================================================
pub mod runtime;

// ============================================================
// 架构后端 (DSL 生成；v12 唯一语法)
// ============================================================
// 只有**发行后端**：x86_64 / aarch64 / riscv64。ISA-DSL 的示例谱
// （demo_v12 / demo8_v12）是测试夹具，在 `tests/isa/` + `tests/common/mod.rs`
// 里用 `isa_from_file!(…, krate = forge_codegen)` 生成——不进本 rlib。
pub mod arch;
pub use arch::arm64_v12;
pub use arch::riscv64_v12;
pub use arch::x86_v12;

// ============================================================
// Module re-exports (for paths like code_forge::backend::pipeline::...)
// ============================================================
#[cfg(feature = "jit")]
pub use runtime::jit;

// ============================================================
// Shared type re-exports
// ============================================================
pub use forge_isa_runtime::vcode::{VBlockId, VCode, VCodeBlock};

// ============================================================
// Machine re-exports (v19 — primary API)
// ============================================================
pub use forge_isa_runtime::machine::cfi::{CfiOp, FunctionCfi, scan_x86_prologue};
pub use forge_isa_runtime::machine::encoder::EncodeError;
pub use forge_isa_runtime::machine::encoder::TargetEncoder;
pub use forge_isa_runtime::machine::inst::{EffectKind, MachineInst};
pub use forge_isa_runtime::machine::isa_info::IsaCapabilities;
pub use forge_isa_runtime::machine::isa_info::IsaInfo;
pub use forge_isa_runtime::machine::isa_info::RegisterClassInfo;
pub use forge_isa_runtime::machine::lowering::InstPacket;
pub use forge_isa_runtime::machine::reloc_patcher::{
    RelocPatcher, RiscvRelocPatcher, X86RelocPatcher,
};
pub use forge_isa_runtime::machine::simulator::SimulationState;
pub use forge_isa_runtime::machine::target::{ErasedTargetMachine, TargetMachine};
pub use pipeline::alloc_config::RegAllocConfig;
pub use pipeline::compiler::FunctionCompiler;
pub use pipeline::regalloc_bt::BacktrackingAllocator;

// ============================================================
// Runtime re-exports
// ============================================================

/// 机器能力门控用例的**可见性事件**（`FORGE_JIT_EVENTS=<文件>` 时追加一行
/// `<事件> <用例>`）。libtest 会吞掉**通过**测试的 stdout/stderr，于是
/// 「无 AVX-512F 就 skip」这类用例在 CI 日志里与真跑过完全无法区分（两者都只
/// 打印 `... ok`）——落盘事件 + CI 的 `if: always()` 步骤是唯一判据（与 e2e 的
/// `FORGE_E2E_EVENTS` 同模式）。写失败静默：可见性辅助不得影响测试结论。
pub fn jit_event(kind: &str, case: &str) {
    use std::io::Write;
    let Some(path) = std::env::var_os("FORGE_JIT_EVENTS") else {
        return;
    };
    let Ok(mut f) = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(path)
    else {
        return;
    };
    let _ = writeln!(f, "{kind} {case}");
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

// 生成物运行面（`AllocResult`/`CodeSink`/`LabelRef`/`CompiledFunction`/`LowerCtx`/`MemRef` …）
// 已于 v19 V1 迁到 `forge-isa-runtime`；本 crate 只做 re-export（见文件上方
// `pub use forge_isa_runtime::{…}`），既有 `crate::AllocResult` 等内部路径因此继续有效。

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

#[cfg(test)]
mod type_map_tests {
    use super::*;

    /// B2：lowering 的 `reg_class_for` 必须先查 ISA 的 `[types]` 显式映射
    /// （与编译入口的值池门同一份数据），否则"门放行、lowering 按别的类分配"。
    #[test]
    fn reg_class_for_honours_isa_type_map() {
        let mut ctx = LowerCtx::new();
        // 无映射时：F64 走 `from_type_id` 的族规则 → FPR(8)。
        assert_eq!(ctx.reg_class_for(&TypeId::F64), RegClass::FPR(8));
        // 软浮点 ISA 显式声明 f64 → GPR(8)：映射优先。
        ctx.type_map = vec![(TypeId::F64, RegClass::GPR(8))];
        assert_eq!(ctx.reg_class_for(&TypeId::F64), RegClass::GPR(8));
        // 未列出的类型不受影响。
        assert_eq!(ctx.reg_class_for(&TypeId::I32), RegClass::GPR(4));
    }
}

/// v18 S6：生成期自测的覆盖率守卫（读生成模块里的 `__spec_tests` 常量——那里随
/// TOML 自动更新，这里钉"是否仍在覆盖全部指令"）。
///
/// **必须放在文件末尾**：本仓的"宿主写死寄存器类"守卫
/// （`tests/no_hardcoded_widths.rs`）按**第一个** `#[cfg(test)]` 截断扫描，
/// 在前面插 `#[cfg(test)]` 项会让它漏扫后面的生产代码。
#[cfg(test)]
mod spec_coverage_guard;
