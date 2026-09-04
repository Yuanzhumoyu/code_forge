//! Stage 7 of the compilation pipeline: frame layout.
//!
//! Computes spill slots, stack alignment and the total frame size from the
//! ABI/register metadata of the target machine.

use crate::AllocResult;
use crate::machine::abi::FrameLayoutKind;
use crate::machine::target::TargetMachine;
use crate::pipeline::compiler::CompileState;

/// 帧布局三数值——从声明式 `[abi.frame]`（layout + fp_push_bytes）+ reg_info
/// 推导，是 min_frame / callee_saved_bytes / stack_slot_shift 的唯一来源。
/// 取代 v14 的 `[abi.frame].min_frame_bytes / callee_saved_bytes_override /
/// stack_slot_shift` 三个魔法数键（riscv 的 104/0/16 全可由 fp_push_bytes +
/// callee_saved 表推出）。
///
/// - **fp-outside**（x86/demo）：callee-saved 用硬件 push 在帧指针上方——
///   spill 槽从 sp_base = -(frame) - callee_saved_bytes 起、栈槽基准
///   fp - callee_saved_bytes。min_frame = 0（无固定下限）。
/// - **fp-inside**（riscv）：ra/fp/callee-saved 保存槽在帧**内顶部**
///   （@push_callee 的 SD 到 [sp+frame-fp_push-(k+1)*8]）→ 帧最小 =
///   fp_push_bytes + Σcallee_saved×宽（否则 SD 偏移为负写坏 sp 下方）；
///   callee_saved_bytes = 0（spill 槽 sp_base = -(frame) 留在帧内，否则落
///   帧外与递归帧重叠——fib 死循环）；栈槽平移 = fp_push_bytes（基准
///   fp-16 避开保存槽、递归各帧独立）。
#[derive(Debug, Clone, Copy)]
pub(crate) struct FrameLayoutInfo {
    pub min_frame: u32,
    pub callee_saved_bytes: i32,
    pub stack_slot_shift: i32,
}

pub(crate) fn frame_layout_info<M: TargetMachine + ?Sized>(machine: &M) -> FrameLayoutInfo {
    let fl = machine.abi().frame_layout();
    let ri = machine.reg_info();
    // 帧指针上方推入区 = fp 保存槽（frame_pointer_overhead）+ callee-saved ×
    // 主 GPR 类宽度（主类宽度取 default_gpr_class()，元数据驱动不再假设
    // GPR64）。
    let cs_bytes =
        (ri.callee_saved().len() as i32) * (ri.reg_class_width(ri.default_gpr_class()) as i32);
    let pushed = (ri.frame_pointer_overhead() as i32) + cs_bytes;
    match fl.kind {
        FrameLayoutKind::Outside => FrameLayoutInfo {
            min_frame: 0,
            callee_saved_bytes: pushed,
            stack_slot_shift: pushed,
        },
        FrameLayoutKind::Inside => FrameLayoutInfo {
            min_frame: fl.fp_push_bytes + cs_bytes as u32,
            callee_saved_bytes: 0,
            stack_slot_shift: fl.fp_push_bytes as i32,
        },
    }
}

/// callee-saved 区字节数（帧指针上方的 push 区；fp-inside 布局 = 0）。
/// compiler.rs 的 LowerCtx 与 emission.rs 共用。
pub(crate) fn callee_saved_bytes<M: TargetMachine + ?Sized>(machine: &M) -> i32 {
    frame_layout_info(machine).callee_saved_bytes
}

impl<I: crate::machine::inst::MachineInst + 'static> CompileState<I> {
    // ── Stage 7: Frame Layout ──

    pub(crate) fn calculate_frame_size<M2: TargetMachine<Inst = I>>(
        &self,
        alloc_result: &AllocResult,
        machine: &M2,
    ) -> u32 {
        // spill 区域：使用 regalloc 报告的 spill_area_size（含对齐与全部槽位），
        // 而不是仅对分配到的槽求和——后者在槽释放/复用时会低估实际需要的栈帧。
        let spill: u32 = alloc_result.frame_info.spill_area_size;
        // 局部变量区（stack_addr 槽）：lowering 时跟踪的最大深度。不含它，
        // 局部槽会落在 sub rsp 分配区之外（Windows 无 red zone → SEGV）。
        let locals: u32 = self.ctx.max_stack_bytes;
        // 两块区域必须相加：spill 槽从 sp_base = -(frame) - callee_saved 起向上
        // 分配，locals 槽从 rbp - callee_saved 起向下分配；若取 max，较小一方的
        // 槽会写进另一方的区域（stack_addr 多槽 + regalloc spill 并存时互相覆盖，
        // 导致局部变量被垃圾地址覆盖——forge-rustc w2 无限循环即此 bug）。
        let size = spill.saturating_add(locals);
        // 栈参数区（Windows x64 第 5+ 参数 + shadow space）：调用方在 call
        // 前把超寄存器参数 store 到 [rsp+shadow+off]，帧底之上必须预留该
        // 区域——否则 store 写穿 rsp 之下（无 red zone）→ SEGV。
        let stack_args = self.ctx.max_stack_arg_bytes;
        let size = size.saturating_add(stack_args);
        let align = machine.abi().stack_align();
        // 最小帧（fp-inside 推导 = fp_push + callee_saved 区）：riscv 的
        // ra/fp 保存槽需帧 ≥ 固定值，否则 emit 模板的 {frame_size_mN} 偏移
        // 为负（写坏 sp 下方）。
        let min_frame = frame_layout_info(machine).min_frame;
        let size = size.max(min_frame);
        // 栈填充（x86 = 8 = align/2）：prologue push rbp + callee-saved 后
        // rsp%16==8（入口 rsp%16==8 由 call 压入的返回地址造成），sub rsp 必须
        // 使 call 前 rsp%16==0（SysV/Windows x64 ABI）。spill 区 size 是 16 的
        // 倍数（%16==0），故 frame 需额外 +8 才能满足。无 call 的函数多 8 字节
        // 栈帧无害；有 call 的否则外部函数（Rust C ABI 的 movaps 保存）会 SEGV。
        // 架构事实由 [abi].frame_padding 声明（x86=8，其他 ABI 缺省 0）。
        let padding: i32 = machine.abi().frame_padding();
        size.div_ceil(align) * align + padding as u32
    }
}
