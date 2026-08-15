//! 聚合 ABI 展开(Stage 0.5)——从 compiler.rs 分批迁出(第四十一轮)。
//!
//! 聚合参数/返回/存储/call 结果 >8 字节的 IR 层展开:
//! - 段值拆分(≤16 字节:寄存器路径;>16 字节 Unsupported)
//! - 展开触发检测(has_* 族)与展开执行(expand_* 族)分居此处
//!
//! 依赖约定:仅依赖 forge_ir 与 crate::pipeline::agg_const;
//! 不依赖 CompileState(展开在 CompileState::new 之前执行——常量池克隆时序)。

use forge_ir::*;
use std::collections::HashMap;

/// 函数是否含 GEP(决定是否需要可变副本做 GEP 展开)。
pub(crate) fn has_gep(func: &Function) -> bool {
    for (_, bd) in func.dfg.blocks() {
        for &ii in &bd.inst_order {
            if func.dfg.insts[ii.0 as usize].opcode == Opcode::GetElementPtr {
                return true;
            }
        }
    }
    false
}

/// 函数是否含 >8 字节聚合返回(ret 拆段触发——第三十五轮:needs_agg_expand
/// 原缺此检测,大聚合返回的纯 call+extractvalue 函数依赖 pattern_isel 开关
/// 蹭 func_owned 克隆才展开——开关关闭即 5 vs 6 错误;第三十七轮修复生效)。
pub(crate) fn has_large_agg_ret(func: &Function) -> bool {
    func.return_types().iter().any(|t| {
        let s = func.types.borrow();
        s.is_aggregate(*t) && s.size_bytes(*t) > 8
    })
}

/// 函数是否含 >8 字节聚合 call 结果(call 结果拆段触发;判定与
/// expand_large_agg_call_results 一致)。
pub(crate) fn has_large_agg_call_result(func: &Function) -> bool {
    for (_, bd) in func.dfg.blocks() {
        for &ii in &bd.inst_order {
            let inst = &func.dfg.insts[ii.0 as usize];
            if matches!(inst.opcode, Opcode::Call | Opcode::CallIndirect)
                && inst.results.len() == 1
                && let Some(rt) = inst.results.first().and_then(|v| func.dfg.value_type(*v))
                && func.types.borrow().is_aggregate(rt)
                && func.types.borrow().size_bytes(rt) > 8
            {
                return true;
            }
        }
    }
    false
}

/// 函数是否含 >8 字节聚合实参(决定是否需要可变副本做 call 拆段)。
pub(crate) fn has_large_agg_call_arg(func: &Function) -> bool {
    for (_, bd) in func.dfg.blocks() {
        for &ii in &bd.inst_order {
            let inst = &func.dfg.insts[ii.0 as usize];
            if matches!(inst.opcode, Opcode::Call | Opcode::CallIndirect) {
                let skip = usize::from(inst.opcode == Opcode::CallIndirect);
                for v in inst.operands.iter().skip(skip) {
                    if let Some(ty) = func.dfg.value_type(*v)
                        && func.types.borrow().is_aggregate(ty)
                        && func.types.borrow().size_bytes(ty) > 8
                    {
                        return true;
                    }
                }
            }
        }
    }
    false
}

/// 函数是否含 >8 字节聚合形参(大聚合参数拆段触发)。
pub(crate) fn has_large_agg_param(func: &Function) -> bool {
    func.param_types().iter().any(|t| {
        let s = func.types.borrow();
        s.is_aggregate(*t) && s.size_bytes(*t) > 8
    })
}

/// 函数是否含 >8 字节聚合 load(决定是否需要可变副本做大聚合值传播)。
pub(crate) fn has_large_agg_load(func: &Function) -> bool {
    for (_, bd) in func.dfg.blocks() {
        for &ii in &bd.inst_order {
            let inst = &func.dfg.insts[ii.0 as usize];
            if matches!(inst.opcode, Opcode::Load)
                && let Some(ty) = inst.results.first().and_then(|v| func.dfg.value_type(*v))
                && func.types.borrow().is_aggregate(ty)
                && func.types.borrow().size_bytes(ty) > 8
            {
                return true;
            }
        }
    }
    false
}

/// 函数是否含任何聚合内存访问(store/gep/大聚合 load/call 实参/形参/返回/
/// call 结果)——决定是否需要可变副本做 Stage 0.5 展开(第三十七轮:含
/// has_large_agg_ret/has_large_agg_call_result,展开触发独立于 pattern 开关)。
pub(crate) fn has_any_agg(func: &Function) -> bool {
    has_agg_mem_access(func)
        || has_gep(func)
        || has_large_agg_load(func)
        || has_large_agg_call_arg(func)
        || has_large_agg_param(func)
        || has_large_agg_ret(func)
        || has_large_agg_call_result(func)
}

/// 函数是否含聚合 store/call 实参/load(AggConst 字面量或聚合类型触发——
/// 决定是否需要可变副本做聚合 store 展开)。
fn has_agg_mem_access(func: &Function) -> bool {
    for (_, bd) in func.dfg.blocks() {
        for &ii in &bd.inst_order {
            let inst = &func.dfg.insts[ii.0 as usize];
            match inst.opcode {
                Opcode::Store => {
                    if let Some(v) = inst.operands.first()
                        && matches!(func.dfg.values[v.0 as usize].def, ValueDef::AggConst(_))
                    {
                        return true;
                    }
                }
                Opcode::Call | Opcode::CallIndirect => {
                    let skip = usize::from(inst.opcode == Opcode::CallIndirect);
                    if inst
                        .operands
                        .iter()
                        .skip(skip)
                        .any(|v| matches!(func.dfg.values[v.0 as usize].def, ValueDef::AggConst(_)))
                    {
                        return true;
                    }
                }
                Opcode::Load => {
                    if let Some(ty) = inst.results.first().and_then(|v| func.dfg.value_type(*v))
                        && func.types.borrow().is_aggregate(ty)
                    {
                        return true;
                    }
                }
                _ => {}
            }
        }
    }
    false
}

// 聚合值 → 内存槽地址映射（S1 嵌套聚合字段提取）：value → (PTR 地址, 槽类型)。
pub(crate) type AggSlots = HashMap<Value, (Value, TypeId)>;

/// 使用位置（重写任务队列条目——block/pos/inst 定位使用指令）。
pub(crate) struct UsePos {
    pub block: Block,
    pub pos: usize,
    pub inst: Inst,
}

/// 聚合值使用种类（rewrite 分派）。
pub(crate) enum UseKind {
    CopyStore(Value), // store 目标地址
    Extract(u32),     // extractvalue 字段索引
}

/// 聚合 load 使用展开任务（S1：UseJob 提升为模块级——collect_extract_jobs 复用）。
pub(crate) struct UseJob {
    pub block: Block,
    pub pos: usize,
    pub inst: Inst,
    pub kind: UseKind,
    pub load: usize, // loads 索引
}

// 占位:expand_* 族与辅助函数将按批迁入(第四十一轮分批)。
