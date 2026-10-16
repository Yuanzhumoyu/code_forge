//! **保存区形态的 `VaArg` IR 展开**（v20 变参 V3 的最后一环）。
//!
//! ## 为什么在 IR 层展开，而不是生成器里发序列
//!
//! 保存区形态的取值是**有条件的**：`gp_offset` 没超过寄存器区上限就从保存区取、否则从溢出区取
//! （还要各自推进游标）。生成器要发这段就得有"比较 / 条件选择 / 寄存器间加 / 掩码"四个能力，
//! 而 x86 谱里对应指令都没有角色——为一条 op 加四个能力，且别的 ISA 还得各来一遍。
//! IR 层则**全都有现成降级**：`Icmp`、`Select`、`Iadd`、`Uextend`、`Ireduce`、`Load`/`Store`
//! （宽度按 IR 类型）、`Fload`/`Fptrunc`（`f32` 的默认提升就是"取 `f64` 再窄回"）。
//!
//! ## 展开形态（sysv64 一族，整数类）
//!
//! ```text
//! gp      = load u32 [ap]                    ; gp_offset
//! in_reg  = icmp ult gp, gp_limit
//! p_reg   = reg_save + uextend(gp)
//! p_src   = select in_reg, p_reg, overflow
//! v       = load <ty> [p_src]
//! gp_out  = select in_reg, ireduce(gp + gp_step), gp
//! ov_out  = select in_reg, overflow, overflow + slot_bytes
//! store u32 [ap]   = gp_out
//! store i64 [ap+8] = ov_out
//! ```
//!
//! 浮点类把 `gp_offset` 换成 `fp_offset`（字段偏移 4、上限 `fp_limit`、步长 `fp_step`），
//! 取值走 `Fload`；`f32` 结果按默认提升**先取 `f64` 再 `Fptrunc`**。
//!
//! 展开完把原 `VaArg` 指令改成 `Copy <v>`（结果值原样保留 ⇒ 后续使用无需重写）——与
//! `compiler.rs` 的聚合展开同一套写法。
//!
//! ## 两个坑（照抄既有展开的处理方式）
//!
//! 1. `Dfg::make_inst` 只**建**指令，新指令落在块尾——必须显式移到 `VaArg` 之前
//!    （否则"原指令改 `Copy`"会读到尚未计算的值）；
//! 2. 逐指令改写要**先收集再改**两遍（遍历 `blocks()` 时不能再可变借用 `func`）。

/// **ABI 槽的帧内分配**（v20 变参 V3）：保存区与 `va_list` 对象各一段。
///
/// 关键点（2026-10-01 实测踩到）：**必须排在前端局部槽之下**。前端的 `stack_addr(-N)` 是在
/// lowering 里才被 `max_stack_bytes` 统计的，而本展开跑在 lowering **之前**——若在编译入口按
/// "当时的 max"（= 0）预留，保存区就会与前端的 `stack_addr(-16)` 之类**重叠**（sysv64 那条
/// 用例恰好没有局部槽所以没暴露）。所以这里先**扫一遍 IR** 算前端局部槽的最大深度
/// （与 lowering 同一算式 `-v + slot_bytes`），ABI 槽再从它之后往上排。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct AbiSlots {
    /// 保存区深度（正值；`StackAddr` 立即数用 `-depth`）。**要求连续时**它是"相对帧顶的
    /// 深度"（= 保存区字节数），不再是"排在前端局部槽之下"的那个深度。
    pub save_depth: u32,
    /// `va_list` 对象深度（正值）。
    pub obj_depth: u32,
    /// 保存区相对**帧基址**的偏移（序言 spill 用；`-depth - stack_slot_shift`）。
    pub save_off: i64,
    /// 保存区基址的 `StackAddr` 立即数（`va_start` 的 `SaveOff` 初值按它算）：
    /// 地址 = `fp + 本值 - stack_slot_shift`。不连续 = `-save_depth`（历史口径）；
    /// 连续 = `shift - save.size`（地址 = `fp - save.size`，**与 shift 无关**）。
    pub save_base_v: i64,
    /// 帧顶给保存区留出的字节数（v20 V7）：> 0 时 ra/fp、callee-saved 的保存槽与局部槽
    /// 整体下移这么多（生成物读 `AllocResult.va_top`，局部槽读 `stack_slot_shift`）。
    pub va_top: u32,
    /// 帧基址 → 局部槽区起点的平移：`StackAddr(v)` 的地址 = `fp + v - shift`，
    /// 所以"帧基址相对偏移 X"要发 `StackAddr(X + shift)`。
    pub shift: i64,
    /// ABI 槽占用的总深度（喂给 `CompileState`，让帧尺寸覆盖到）。
    pub max_depth: u32,
}

mod expand;
mod slots;
#[cfg(test)]
mod tests;

pub(crate) use expand::expand_va;
pub(crate) use slots::{has_va_op, plan_abi_slots, va_info_for_expansion};
