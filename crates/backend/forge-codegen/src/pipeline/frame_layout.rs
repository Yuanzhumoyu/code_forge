//! Stage 7 of the compilation pipeline: frame layout.
//!
//! Computes spill slots, stack alignment and the total frame size from the
//! ABI/register metadata of the target machine.

use crate::AllocResult;
use crate::machine::abi::FrameLayoutKind;
use crate::machine::target::TargetMachine;
use crate::pipeline::compiler::CompileState;
use forge_isa_runtime::machine::abi_model::AbiPlan;

/// 帧布局三数值——从声明式 `[machine.frame]`（layout + fp_push_bytes）+ reg_info
/// 推导，是 min_frame / callee_saved_bytes / stack_slot_shift 的唯一来源。
/// 取代 v14 的 `[machine.frame].min_frame_bytes / callee_saved_bytes_override /
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
///   帧外与递归帧重叠——fib 死循环）；栈槽平移 = **fp_push_bytes + cs_bytes**
///   （整块保存区都在帧内顶部，局部槽必须整体下移；v27 M6-118 实测只移
///   fp_push 会让局部落在 callee-saved 槽上）。
#[derive(Debug, Clone, Copy)]
pub(crate) struct FrameLayoutInfo {
    pub min_frame: u32,
    pub callee_saved_bytes: i32,
    pub stack_slot_shift: i32,
}

pub(crate) fn frame_layout_info<M: TargetMachine + ?Sized>(
    machine: &M,
    layout: Option<&AbiPlan>,
) -> FrameLayoutInfo {
    let fl = machine.abi().frame_layout();
    let ri = machine.reg_info();
    // 帧指针上方推入区 = fp 保存槽（frame_pointer_overhead）+ callee-saved ×
    // 主 GPR 类宽度（主类宽度取 default_gpr_class()，元数据驱动不再假设 GPR64）。
    //
    // **两条路都按 plan 的整张表数**（v20 A6 / v23 M3 收口）：
    //
    // - fp-inside（riscv/arm64）：保存/恢复本来就是运行时按
    //   `alloc_result.callee_saved_to_save` 循环（`callee_saved_loop`），保存集随约定而变
    //   （arm64 的 AAPCS64 = X19-X28 **+ V8-V15**）。帧顶槽位公式
    //   `frame - fp_push - (k+1)*slot` 要求帧至少装得下**整张表**；plan 的表 ⊇ 实际保存集
    //   （regalloc 只保存用到的那些），所以按 plan 算必然够用。
    // - fp-outside（x86 的 push/pop）**也已经是运行时的**：序言按
    //   `alloc_result.callee_saved_to_save` 循环 push、`cs_imm`/`sp_base` 都取
    //   `callee_saved_to_save.len()`（见 `dsl/codegen/frame.rs` 的 `gen_push_mechanism`），
    //   而这张表由管线从 plan 填。谱面的静态 push 表（`[machine].callee_saved_gpr`）与
    //   `[emit.prologue]` 模板在 v20 A4/v21 W9 就已删除。
    //
    // 无 plan ⇒ **明确报错**（A6 的 fail-closed）：调用计划在编译入口就是必需的，
    // 走到这里还缺 plan 说明有路径漏接——**不**退回"按谱表数/零"这种会少算帧字节的缺省
    // （x86 显式选 sysv64：5 vs 7，少算 16 字节会让局部/spill 槽与 push 槽重叠，
    // 覆盖调用者保存值 ⇒ 静默错值）。
    let Some(plan) = layout else {
        panic!(
            "帧布局需要调用计划（plan）：没有 plan 时 callee-saved/栈区账目无从计算\
             （编译入口 fail-closed，见 docs/reference/backend-pipeline.md 与 v23 M3 的 A6 收口）"
        );
    };
    let cs_count = plan.callee_saved.regs.len() as i32;
    let cs_bytes = cs_count * (ri.reg_class_width(ri.default_gpr_class()) as i32);
    let pushed = (ri.frame_pointer_overhead() as i32) + cs_bytes;
    // **帧指针保存槽不算"rbp 之下"**（v27 M6-95）：fp-outside 的序言是
    // `push rbp; mov rsp,rbp; push callee-saved…` ⇒ rbp **指向** fp 保存槽，
    // rbp 之下只有 callee-saved。此前这里把 `frame_pointer_overhead` 也算进
    // `callee_saved_bytes`/`stack_slot_shift`，于是 spill 基点比 rsp 低 8 字节
    // （槽 0 = `rsp - 8`），而循环里的 `call` 恰好把返回地址压在那里 ⇒ 槽 0 被
    // 写成代码地址，reload 当基址用就 AV（hoisted `StackAddr` 跨调用实测）。
    // fp-inside（arm64/riscv）不用这两个字段（保存区在帧内顶部），不受影响。
    // （`frame_pointer_overhead` 仍由 frame 最小帧/序言账目消费，不是丢弃。）
    // **帧顶保存区**（v20 V7）：形状要求"保存区与栈实参连续"时，保存区占 `[入口 sp - save, 入口 sp)`，
    // 帧顶那一串（fp/lr 保存槽 + callee-saved）整体下移 ⇒ 帧至少要装得下 保存区 + 那串。
    let va_top = plan
        .va_area
        .as_ref()
        .filter(|v| v.save_contiguous)
        .and_then(|v| v.save.as_ref())
        .map(|s| s.size)
        .unwrap_or(0);
    match fl.kind {
        FrameLayoutKind::Outside => FrameLayoutInfo {
            min_frame: va_top,
            // rbp 之下只有 callee-saved push（fp 保存槽在 `[rbp]`）。**只**影响 spill
            // 基点；`stack_slot_shift` 仍按 `pushed` 保留原来的 8 字节间隙——局部槽
            // （`stack_addr`）的既有偏移已由 DWARF fbreg 等消费方按 64 写死/推算，
            // 一起改会牵动无关路径（实测 u128 用例挂起）。
            callee_saved_bytes: cs_bytes,
            stack_slot_shift: pushed,
        },
        FrameLayoutKind::Inside => FrameLayoutInfo {
            min_frame: va_top + fl.fp_push_bytes + cs_bytes as u32,
            callee_saved_bytes: 0,
            // **栈槽平移必须覆盖整块保存区**（v27 M6-118 修正）：保存槽
            // [fp - fp_push - cs_bytes, fp) 全在帧内顶部，局部槽若只按
            // `fp_push` 平移就会落在 callee-saved 槽上（实测 `write_str` 的
            // 局部 `fp-0x40` 正是 `x24` 的保存槽 ⇒ 返回时 x24 被局部值覆盖
            // ⇒ `core::fmt::write` 里 `blr x24` 跳到 0）。
            stack_slot_shift: fl.fp_push_bytes as i32 + cs_bytes,
        },
    }
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
        // **fp-inside 的偏移基准要算进帧**（v27 M6-106 修正）：局部槽地址是
        // `fp - stack_slot_shift - depth`（riscv/arm64 的 shift = ra/fp 保存槽宽度），
        // 而深度只记到 `max_stack_bytes` ⇒ 帧若只按深度分配，最深的槽会落到
        // `sp` 之下（实测 40 个 i32 槽：地址 sp-12 → data abort 循环）。
        // fp-outside（x86）的 shift 是**已 push 的 callee-saved 字节数**（在 rsp
        // 之上、不在帧里），不能再加——那会白抬帧并挪错 spill 基点。
        let locals = match machine.abi().frame_layout().kind {
            crate::machine::abi::FrameLayoutKind::Inside => {
                locals.saturating_add(self.ctx.stack_slot_shift.max(0) as u32)
            }
            crate::machine::abi::FrameLayoutKind::Outside => locals,
        };
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
        // v23 M5（C6）：读 lowering 前算好的那一份（不再重算）。
        let min_frame = self.ctx.min_frame;
        let size = size.max(min_frame);
        // 栈填充（x86 = 8 = align/2）：prologue push rbp + callee-saved 后
        // rsp%16==8（入口 rsp%16==8 由 call 压入的返回地址造成），sub rsp 必须
        // 使 call 前 rsp%16==0（SysV/Windows x64 ABI）。spill 区 size 是 16 的
        // 倍数（%16==0），故 frame 需额外 +8 才能满足。无 call 的函数多 8 字节
        // 栈帧无害；有 call 的否则外部函数（Rust C ABI 的 movaps 保存）会 SEGV。
        // 架构事实由约定数据声明（win64 = 8，其他 ABI 缺省 0）。v20 A5-3 ④：
        // **有 plan 时用 plan 的**（帧填充是规则的 `frame_padding`，随约定而变）；
        // 谱里那份只是无 plan 时的兜底。
        let padding: i32 = self
            .ctx
            .conv
            .layout()
            .map(|cl| cl.stack.frame_padding)
            .unwrap_or_else(|| machine.abi().frame_padding());
        size.div_ceil(align) * align + padding as u32
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::arch::arm64::TargetMachine as ArmTm;

    /// **fp 保存槽不在 rbp 之下**（v27 M6-95 回归守卫）：spill 基点用的
    /// `callee_saved_bytes` 必须**只算 callee-saved push**；`stack_slot_shift`
    /// （局部槽平移）保持历史值（含 fp）不动。两者相等会把槽 0 放到 `rsp - 8`——
    /// 正好是 `call` 压返回地址的位置（hoisted `StackAddr` 跨调用 ⇒ AV）。
    #[test]
    fn outside_spill_base_excludes_the_frame_pointer_slot() {
        let tm = crate::arch::x86::TargetMachine::new();
        let reg = forge_abi::builtin::registry().expect("内置注册表");
        let sig = forge_abi::Signature::new(vec![], Some(forge_abi::TyView::int(8, 8)));
        let plan = crate::pipeline::abi_target::plan_for_signature(&tm, &reg, "win64", &sig)
            .expect("win64 plan");
        let info = frame_layout_info(&tm, Some(&plan));
        let fp = tm.reg_info().frame_pointer_overhead() as i32;
        let pushed = plan.callee_saved.regs.len() as i32
            * (tm
                .reg_info()
                .reg_class_width(tm.reg_info().default_gpr_class()) as i32)
            + fp;
        assert!(
            info.callee_saved_bytes > 0 && info.callee_saved_bytes < pushed,
            "spill 基点必须排除 fp 保存槽：csb={} pushed={pushed}",
            info.callee_saved_bytes
        );
        assert_eq!(
            info.stack_slot_shift, pushed,
            "局部槽平移保持历史口径（含 fp），不随 spill 基点一起改"
        );
    }

    /// **fp-inside**（arm64）不需要扣：保存区在帧内顶部，`callee_saved_bytes = 0`。
    #[test]
    fn inside_spill_base_is_zero() {
        let tm = ArmTm::new();
        let reg = forge_abi::builtin::registry().expect("内置注册表");
        let sig = forge_abi::Signature::new(vec![], Some(forge_abi::TyView::float(8)));
        let plan = crate::pipeline::abi_target::plan_for_signature(&tm, &reg, "aapcs64", &sig)
            .expect("aapcs64 plan");
        let info = frame_layout_info(&tm, Some(&plan));
        assert_eq!(info.callee_saved_bytes, 0);
    }

    /// **FPR callee-saved 进得了帧**（v20 A6 激活的守卫）：AAPCS64 的表是
    /// X19-X28（10）+ V8-V15（8）= 18 条，而谱里声明的 `[machine].callee_saved_gpr`
    /// 只有 10 条——帧顶槽位公式要求帧装得下**整张表**，所以 fp-inside 的
    /// `min_frame` 必须按 **plan** 数（16 的 fp/lr 保存区 + 18×8 = 160），
    /// 否则多保存的 8 个 FPR 会写到帧外。
    #[test]
    fn arm64_fp_inside_frame_counts_the_plan_callee_saved_table() {
        let tm = ArmTm::new();
        let reg = forge_abi::builtin::registry().expect("内置注册表");
        let sig = forge_abi::Signature::new(vec![], Some(forge_abi::TyView::float(8)));
        let plan = crate::pipeline::abi_target::plan_for_signature(&tm, &reg, "aapcs64", &sig)
            .expect("aapcs64 plan");
        let cl = plan.clone();

        let gpr = cl
            .callee_saved
            .regs
            .iter()
            .filter(|r| !r.class.is_fp())
            .count();
        let fpr = cl
            .callee_saved
            .regs
            .iter()
            .filter(|r| r.class.is_fp())
            .count();
        assert_eq!(gpr, 10, "AAPCS64 的 GPR callee-saved = X19-X28");
        assert_eq!(fpr, 8, "AAPCS64 的 FPR callee-saved = V8-V15");

        let with_plan = frame_layout_info(&tm, Some(&cl));
        assert_eq!(
            with_plan.min_frame,
            16 + 18 * 8,
            "fp-inside 的最小帧 = fp/lr 保存区 + plan 整张表"
        );
        // fp-inside 下 spill 寻址不依赖 cs 字节数 ⇒ 这条切换不影响 sp_base。
        assert_eq!(with_plan.callee_saved_bytes, 0);
        // **无 plan ⇒ 明确报错**（A6 收口）：不再退回"零/谱表数"这种会少算帧字节的缺省。
        assert!(
            std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                frame_layout_info(&tm, None)
            }))
            .is_err(),
            "无 plan 时帧布局必须 fail-closed，而不是按零算"
        );
    }
}
