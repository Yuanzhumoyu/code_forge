//! 调用点的**约定级破坏集合**合并（单独一处：`lowering.rs` 撞规模棘轮，
//! 且这件事本来就有独立语义）。
//!
//! ## 为什么必须有
//!
//! 被调方会写坏所有 **caller-saved** 寄存器，而这些寄存器在**本函数**里是可分配的。
//! 若调用点不把它们记进 clobber 表，分配器就会把"跨调用仍然活跃"的值留在
//! caller-saved 寄存器里 ⇒ 调用返回后读到被踩掉的值。
//! v27 M6 机器级实测症状：`StoreMemR { src: RAX, src2: RAX }`（store 的地址寄存器被
//! 上一条 load 的目标覆盖）⇒ 静默错值 / AV。
//!
//! ## 数据来源
//!
//! 用**约定**（`CallConvCtx::clobbers` = 可用池 − callee-saved − pinned），不是谱面：
//! 同一 ISA 换约定（win64 / sysv64）破坏集不同，写死在谱里就会错。
//! 空（宿主没给约定数据）时**不猜**——照旧不加，保持既有行为。

use forge_ir::RegClass;

use crate::machine::inst::MachineInst;
use forge_isa_runtime::ctx::CallConvCtx;

/// 若本包里有调用指令，就把约定级破坏集并进 `pack_clobbers`（去重）。
pub(crate) fn merge_call_clobbers<I: MachineInst>(
    conv: &CallConvCtx,
    insts: &[I],
    pack_clobbers: &mut Vec<(u32, RegClass)>,
) {
    if !insts.iter().any(MachineInst::is_call) {
        return;
    }
    if std::env::var_os("FORGE_TRACE_CALLCLOBBER").is_some() {
        eprintln!(
            "[callclobber] 约定={} 破坏集={} 项",
            conv.name,
            conv.clobbers.len()
        );
    }
    for &(num, class) in &conv.clobbers {
        if !pack_clobbers.contains(&(num, class)) {
            pack_clobbers.push((num, class));
        }
    }
}
