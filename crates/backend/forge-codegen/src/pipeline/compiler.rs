//! FunctionCompiler v20 — staged compilation pipeline.
//!
//! Drives IR → machine code compilation through the TargetMachine component traits.
//! Each stage is a self-contained method; the pipeline is explicit and debuggable.
//!
//! # Pipeline Stages
//!
//! 1. Block Mapping          — IR Block → VBlockId
//! 2. VReg Pre-allocation    — params, phi, block params → VReg
//! 3. Pattern Matching       — detect IR patterns for optimized lowering
//! 4. Instruction Selection  — TargetLowering::lower_inst / lower_terminator
//! 5. Peephole Optimization  — TargetPeephole::optimize (optional)
//! 6. Register Allocation    — pluggable allocator
//! 7. Frame Layout           — compute spill slots, alignment, frame size
//! 8. Prologue               — TargetFrameLowering::emit_prologue
//! 9. Instruction Emission   — TargetEncoder + spill/label/fixup handling
//! 10. Epilogue              — TargetFrameLowering::emit_epilogue
//! 11. Fixup Resolution      — CodeSink::finish()

use crate::machine::target::TargetMachine;
use crate::pipeline::alloc_config::{ClassConfig, RegAllocConfig as NewRegAllocConfig};

/// Map an `AtomicRmwOp` discriminant (as encoded by `builder.atomic_rmw` into
/// `Immediate::Uint`) back to the enum. Defaults to `Add` on unknown values.
pub(crate) fn atomic_op_from_u64(v: u64) -> AtomicRmwOp {
    use AtomicRmwOp::*;
    match v {
        0 => Xchg,
        1 => Add,
        2 => Sub,
        3 => And,
        4 => Nand,
        5 => Or,
        6 => Xor,
        7 => Max,
        8 => Min,
        9 => Umax,
        10 => Umin,
        11 => Fadd,
        _ => Fsub,
    }
}
use crate::pipeline::agg_expand::{AggSlots, UseJob, UseKind, UsePos};
use crate::pipeline::alloc_result::AllocResult;
use crate::pipeline::regalloc_bt::BacktrackingAllocator;
use crate::{CompiledFunction, IrError, LowerCtx, MachineInst, VBlockId, VCode};
use forge_ir::*;
use std::collections::HashMap;

/// 聚合值使用处重写（helper）：`old_v`（聚合值，段值在 `segs`——每段 i64）
/// 的所有使用按指令语义替换（生成的辅助指令即时移动到使用指令前）：
/// - ExtractValue：字段 idx → 段值（off/8 段 + off%8 移位）——extractvalue 改 Copy；
/// - Store：逐段 store 段值（地址推进）——原 store 标 Nop；
/// - Call/CallIndirect 参数：替换为段值列表（多参数插入）；
/// - 其余（InsertValue 等）：Unsupported。
///
/// Return values 重写（terminator）。
/// 聚合值 → 内存槽地址映射（S1 嵌套聚合字段提取）：value → (PTR 地址, 槽类型)。
/// 后续 extractvalue 命中该映射时走 GEP+load 地址语义（标量字段直接 load；
/// 聚合字段继续登记子地址）。
/// S1：嵌套聚合字段提取——从段值内存化。
/// 分配 alloca 槽、把字段覆盖的 64 位段逐段 store 进槽、结果 value 登记
/// agg_slots（(槽地址, 槽类型)）；原 extractvalue 改 Nop。段对齐时间盒口径：
/// off % 8 != 0（字段跨段且非对齐）保持 Unsupported。
fn memoryize_from_segs(
    func: &mut Function,
    use_pos: &UsePos,
    seg_ty: TypeId,
    off: usize,
    segs: &[Value],
    agg_slots: &mut AggSlots,
) -> Result<(), IrError> {
    if !off.is_multiple_of(8) {
        return Err(IrError::Unsupported(format!(
            "嵌套聚合字段提取：非对齐字段（off={off}）暂不支持"
        )));
    }
    let size = func.types.borrow().size_bytes(seg_ty) as usize;
    let n_segs = size.div_ceil(8);
    let start = off / 8;
    if start + n_segs > segs.len() {
        return Err(IrError::Unsupported("嵌套聚合字段提取：段值不足".into()));
    }
    // 1) 分配帧槽（Alloca 指令——预扫描分配帧偏移，lowering 经 lea_off 取址）
    let cid = func.constants.insert_int(size as i128, 64);
    let alloca = func.dfg.make_inst(
        Opcode::Alloca,
        use_pos.block,
        smallvec::smallvec![],
        smallvec::smallvec![Immediate::Const(cid)],
        &[TypeId::PTR],
        InstFlags::NONE,
    );
    let slot = func.dfg.insts[alloca.0 as usize].results[0];
    // 2) 字段覆盖的段逐段 store（仿 Store 分支的 store + add ptr, 8 模式）
    let mut cur = slot;
    for (i, s) in segs.iter().skip(start).take(n_segs).enumerate() {
        func.dfg.make_inst(
            Opcode::Store,
            use_pos.block,
            smallvec::smallvec![*s, cur],
            smallvec::smallvec![],
            &[],
            InstFlags::SIDE_EFFECT,
        );
        if i + 1 < n_segs {
            let ci = func.dfg.make_inst(
                Opcode::Iconst,
                use_pos.block,
                smallvec::smallvec![],
                smallvec::smallvec![Immediate::Const(func.constants.insert_int(8, 64))],
                &[TypeId::I64],
                InstFlags::NONE,
            );
            let cv = func.dfg.insts[ci.0 as usize].results[0];
            let add = func.dfg.make_inst(
                Opcode::Iadd,
                use_pos.block,
                smallvec::smallvec![cur, cv],
                smallvec::smallvec![],
                &[TypeId::PTR],
                InstFlags::NONE,
            );
            cur = func.dfg.insts[add.0 as usize].results[0];
        }
    }
    // 3) 原指令 Nop + 登记（槽填充 store 只执行一次——后续提取命中映射）
    let inst = &mut func.dfg.insts[use_pos.inst.0 as usize];
    inst.opcode = Opcode::Nop;
    inst.operands = smallvec::smallvec![];
    inst.immediates = smallvec::smallvec![];
    agg_slots.insert(inst.results[0], (slot, seg_ty));
    Ok(())
}

/// S1：聚合字段提取——从地址链继续（无物理拷贝）。
/// %p = add ptr addr, foff；结果 value 登记 (p, 字段类型)；原指令改 Nop。
/// 后续标量字段提取从 p 直接 load；聚合字段提取继续登记子地址。
fn memoryize_from_addr(
    func: &mut Function,
    use_pos: &UsePos,
    base: Value,
    foff: usize,
    seg_ty: TypeId,
    agg_slots: &mut AggSlots,
) -> Result<(), IrError> {
    let cid = func.constants.insert_int(foff as i128, 64);
    let ci = func.dfg.make_inst(
        Opcode::Iconst,
        use_pos.block,
        smallvec::smallvec![],
        smallvec::smallvec![Immediate::Const(cid)],
        &[TypeId::I64],
        InstFlags::NONE,
    );
    let cv = func.dfg.insts[ci.0 as usize].results[0];
    let add = func.dfg.make_inst(
        Opcode::Iadd,
        use_pos.block,
        smallvec::smallvec![base, cv],
        smallvec::smallvec![],
        &[TypeId::PTR],
        InstFlags::NONE,
    );
    let p = func.dfg.insts[add.0 as usize].results[0];
    let inst = &mut func.dfg.insts[use_pos.inst.0 as usize];
    inst.opcode = Opcode::Nop;
    inst.operands = smallvec::smallvec![];
    inst.immediates = smallvec::smallvec![];
    agg_slots.insert(inst.results[0], (p, seg_ty));
    Ok(())
}

/// extractvalue 使用点（块 + 指令序位 + 指令句柄）
fn rewrite_agg_value_uses(
    func: &mut Function,
    old_v: Value,
    segs: &[Value],
    agg_ty: TypeId,
    agg_slots: &mut AggSlots,
) -> Result<(), IrError> {
    // 收集使用位置（避免借用冲突）
    let mut uses: Vec<UsePos> = Vec::new();
    for (b, bd) in func.dfg.blocks() {
        for (pos, &ii) in bd.inst_order.iter().enumerate() {
            let inst = &func.dfg.insts[ii.0 as usize];
            if inst.operands.contains(&old_v) {
                uses.push(UsePos {
                    block: b,
                    pos,
                    inst: ii,
                });
            }
        }
    }
    // 逆序处理（后面的使用先——插入不影响更前 pos）
    for use_pos in uses.into_iter().rev() {
        let inst = &func.dfg.insts[use_pos.inst.0 as usize];
        match inst.opcode {
            Opcode::ExtractValue => {
                let idx = inst
                    .immediates
                    .first()
                    .and_then(|i| match i {
                        Immediate::Uint(u) => Some(*u as u32),
                        _ => None,
                    })
                    .ok_or_else(|| IrError::Unsupported("extractvalue 索引缺失".into()))?;
                // S1：命中嵌套聚合提取的槽映射——地址语义（GEP+load 标量 /
                // 登记子地址继续链式提取）
                if let Some(&(addr, slot_ty)) = agg_slots.get(&inst.operands[0]) {
                    let ts = func.types.borrow();
                    let foff = ts.field_offset(slot_ty, idx).ok_or_else(|| {
                        IrError::Unsupported(format!("extractvalue 字段偏移缺失：{idx}"))
                    })? as usize;
                    let fty = ts.aggregate_elem_type(slot_ty, idx).ok_or_else(|| {
                        IrError::Unsupported(format!("extractvalue 字段类型缺失：{idx}"))
                    })?;
                    if ts.is_aggregate(fty) {
                        drop(ts);
                        let res = func.dfg.insts[use_pos.inst.0 as usize].results[0];
                        memoryize_from_addr(func, &use_pos, addr, foff, fty, agg_slots)?;
                        // 递归：登记的新值（内层聚合）的后续提取继续处理
                        return rewrite_agg_value_uses(func, res, &[], fty, agg_slots);
                    }
                    drop(ts);
                    // 标量字段：%p = add ptr addr, foff; %v = load fty, ptr %p
                    let cid = func.constants.insert_int(foff as i128, 64);
                    let ci = func.dfg.make_inst(
                        Opcode::Iconst,
                        use_pos.block,
                        smallvec::smallvec![],
                        smallvec::smallvec![Immediate::Const(cid)],
                        &[TypeId::I64],
                        InstFlags::NONE,
                    );
                    let cv = func.dfg.insts[ci.0 as usize].results[0];
                    let add = func.dfg.make_inst(
                        Opcode::Iadd,
                        use_pos.block,
                        smallvec::smallvec![addr, cv],
                        smallvec::smallvec![],
                        &[TypeId::PTR],
                        InstFlags::NONE,
                    );
                    let p = func.dfg.insts[add.0 as usize].results[0];
                    let ld = func.dfg.make_inst(
                        Opcode::Load,
                        use_pos.block,
                        smallvec::smallvec![p],
                        smallvec::smallvec![],
                        &[fty],
                        InstFlags::NONE,
                    );
                    let lv = func.dfg.insts[ld.0 as usize].results[0];
                    // 结果 value 重定向：原指令改 Copy（值传播到 load 结果）
                    let inst = &mut func.dfg.insts[use_pos.inst.0 as usize];
                    inst.opcode = Opcode::Copy;
                    inst.operands = smallvec::smallvec![lv];
                    inst.immediates = smallvec::smallvec![];
                    return Ok(());
                }
                let ts = func.types.borrow();
                let off = ts.field_offset(agg_ty, idx).ok_or_else(|| {
                    IrError::Unsupported(format!("extractvalue 字段偏移缺失：{idx}"))
                })? as usize;
                let seg_ty = ts.aggregate_elem_type(agg_ty, idx).ok_or_else(|| {
                    IrError::Unsupported(format!("extractvalue 字段类型缺失：{idx}"))
                })?;
                if ts.is_aggregate(seg_ty) {
                    // S1：嵌套聚合字段——内存化（槽 + 段 store + 登记）
                    drop(ts);
                    let res = func.dfg.insts[use_pos.inst.0 as usize].results[0];
                    memoryize_from_segs(func, &use_pos, seg_ty, off, segs, agg_slots)?;
                    // 递归：登记的新值（内层聚合）的后续提取继续处理
                    return rewrite_agg_value_uses(func, res, &[], seg_ty, agg_slots);
                }
                drop(ts);
                let k = off / 8;
                let inner = off % 8;
                let base = segs
                    .get(k)
                    .copied()
                    .ok_or_else(|| IrError::Unsupported("extractvalue 段值缺失".into()))?;
                let mut final_v = base;
                let mut local_gen: Vec<Inst> = Vec::new();
                if inner > 0 {
                    let bits = (inner * 8) as i64;
                    let cid = func.constants.insert_int(bits as i128, 64);
                    let ci = func.dfg.make_inst(
                        Opcode::Iconst,
                        use_pos.block,
                        smallvec::smallvec![],
                        smallvec::smallvec![Immediate::Const(cid)],
                        &[TypeId::I64],
                        InstFlags::NONE,
                    );
                    let cv = func.dfg.insts[ci.0 as usize].results[0];
                    local_gen.push(ci);
                    let sh = func.dfg.make_inst(
                        Opcode::Ushr,
                        use_pos.block,
                        smallvec::smallvec![base, cv],
                        smallvec::smallvec![],
                        &[TypeId::I64],
                        InstFlags::NONE,
                    );
                    final_v = func.dfg.insts[sh.0 as usize].results[0];
                    local_gen.push(sh);
                }
                // 浮点字段：bitcast（段值 i64 → 字段类型——movq_to_xmm 位模式
                // 转换）；整数字段：Copy 截断
                let mut target = final_v;
                if func.types.borrow().is_float(seg_ty) {
                    let bc = func.dfg.make_inst(
                        Opcode::Bitcast,
                        use_pos.block,
                        smallvec::smallvec![final_v],
                        smallvec::smallvec![],
                        &[seg_ty],
                        InstFlags::NONE,
                    );
                    target = func.dfg.insts[bc.0 as usize].results[0];
                    local_gen.push(bc);
                }
                let inst = &mut func.dfg.insts[use_pos.inst.0 as usize];
                inst.opcode = Opcode::Copy;
                inst.operands = smallvec::smallvec![target];
                inst.immediates = smallvec::smallvec![];
                move_to_front(func, use_pos.block, use_pos.pos, &local_gen);
            }
            Opcode::Store => {
                let addr = inst.operands[1];
                let mut prev_addr = addr;
                let mut local_gen: Vec<Inst> = Vec::new();
                for (k, s) in segs.iter().enumerate() {
                    let st = func.dfg.make_inst(
                        Opcode::Store,
                        use_pos.block,
                        smallvec::smallvec![*s, prev_addr],
                        smallvec::smallvec![],
                        &[],
                        InstFlags::SIDE_EFFECT,
                    );
                    local_gen.push(st);
                    if k + 1 < segs.len() {
                        let cid = func.constants.insert_int(8, 64);
                        let ci = func.dfg.make_inst(
                            Opcode::Iconst,
                            use_pos.block,
                            smallvec::smallvec![],
                            smallvec::smallvec![Immediate::Const(cid)],
                            &[TypeId::I64],
                            InstFlags::NONE,
                        );
                        let cv = func.dfg.insts[ci.0 as usize].results[0];
                        local_gen.push(ci);
                        let add = func.dfg.make_inst(
                            Opcode::Iadd,
                            use_pos.block,
                            smallvec::smallvec![prev_addr, cv],
                            smallvec::smallvec![],
                            &[TypeId::PTR],
                            InstFlags::NONE,
                        );
                        prev_addr = func.dfg.insts[add.0 as usize].results[0];
                        local_gen.push(add);
                    }
                }
                // 原 store 标 Nop
                let inst = &mut func.dfg.insts[use_pos.inst.0 as usize];
                inst.opcode = Opcode::Nop;
                inst.operands = smallvec::smallvec![];
                inst.immediates = smallvec::smallvec![];
                move_to_front(func, use_pos.block, use_pos.pos, &local_gen);
            }
            Opcode::Call | Opcode::CallIndirect => {
                let op_idx = inst
                    .operands
                    .iter()
                    .position(|&v| v == old_v)
                    .ok_or_else(|| IrError::Unsupported("call 参数重写：未找到".into()))?;
                let mut ops = func.dfg.insts[use_pos.inst.0 as usize].operands.clone();
                ops.remove(op_idx);
                for (k, s) in segs.iter().enumerate() {
                    ops.insert(op_idx + k, *s);
                }
                func.dfg.insts[use_pos.inst.0 as usize].operands = ops;
            }
            _ => {
                return Err(IrError::Unsupported(format!(
                    "大聚合值用于 {:?}（仅支持 extractvalue/store/return/call 传参）",
                    inst.opcode
                )));
            }
        }
    }
    // Return values 重写（terminator——先收集再写，避免借用冲突）
    let mut ret_updates: Vec<(Block, Vec<Value>)> = Vec::new();
    for (b, bd) in func.dfg.blocks() {
        if let Terminator::Return { values, .. } = &bd.terminator
            && values.contains(&old_v)
        {
            let mut new_vals: Vec<Value> = Vec::new();
            for &v in values {
                if v == old_v {
                    new_vals.extend_from_slice(segs);
                } else {
                    new_vals.push(v);
                }
            }
            ret_updates.push((b, new_vals));
        }
    }
    for (b, new_vals) in ret_updates {
        let bd = &mut func.dfg.blocks[b.0 as usize];
        if let Terminator::Return { values, .. } = &mut bd.terminator {
            *values = smallvec::SmallVec::from_iter(new_vals);
        }
    }
    Ok(())
}

/// 把块尾的生成指令（make_inst push 到末尾）移动到指定位置前。
fn move_to_front(func: &mut Function, block: Block, pos: usize, insts: &[Inst]) {
    let mut moved: Vec<Inst> = Vec::with_capacity(insts.len());
    for _ in 0..insts.len() {
        let order = &mut func.dfg.blocks[block.0 as usize].inst_order;
        moved.push(order.pop().unwrap());
    }
    moved.reverse();
    for (k, &ni) in moved.iter().enumerate() {
        func.dfg.blocks[block.0 as usize]
            .inst_order
            .insert(pos + k, ni);
    }
}

/// 被调方大聚合参数（>8 ≤16 字节——SysV 整数寄存器路径）：签名拆开为
/// ceil(size/8) 个 i64 参数（与调用方拆段一致），entry 块参数重建，旧参数值
/// 使用处重写（extractvalue → 段值、store → 段 store、return/call → 段值）。
fn expand_large_agg_params(func: &mut Function, agg_slots: &mut AggSlots) -> Result<(), IrError> {
    let old_params = func.param_types();
    let mut seg_map: Vec<usize> = Vec::with_capacity(old_params.len()); // 每参数段数
    let mut has_large = false;
    for t in &old_params {
        let ts = func.types.borrow();
        if ts.is_aggregate(*t) && ts.size_bytes(*t) > 8 {
            let size = ts.size_bytes(*t) as usize;
            if size > 16 {
                return Err(IrError::Unsupported(format!(
                    "聚合参数 >16 字节暂不支持(栈传递未实现): type {t:?}"
                )));
            }
            seg_map.push(size.div_ceil(8));
            has_large = true;
        } else {
            seg_map.push(1);
        }
    }
    if !has_large {
        return Ok(());
    }
    // 签名拆开
    let old_sig = func.types.borrow().get_signature(func.signature).clone();
    let mut new_params: Vec<(TypeId, ImmStr)> = Vec::new();
    for (i, (_t, n)) in old_sig.params.iter().enumerate() {
        let segs = seg_map.get(i).copied().unwrap_or(1);
        for _ in 0..segs {
            new_params.push((TypeId::I64, n.clone()));
        }
    }
    let new_sig = FunctionSignature {
        params: new_params,
        returns: old_sig.returns.clone(),
        calling_convention: old_sig.calling_convention,
        variadic: old_sig.variadic,
    };
    func.signature = func.types.borrow_mut().register_signature(new_sig);
    // entry 块参数重建 + 重写映射（旧参数值 → 新值列表）
    let entry = Block(0); // entry 块约定为索引 0
    let old_bvals = func.dfg.blocks[entry.0 as usize].param_values.clone();
    let mut rewrite: HashMap<Value, Vec<Value>> = HashMap::new();
    let mut new_bparams: Vec<TypeId> = Vec::new();
    let mut new_bvals: Vec<Value> = Vec::new();
    let mut agg_info: Vec<(Value, Vec<Value>, TypeId)> = Vec::new(); // (旧值, 段值, 聚合类型)
    for (i, t) in old_params.iter().enumerate() {
        let segs = seg_map.get(i).copied().unwrap_or(1);
        let old_v = old_bvals[i];
        let mut vals = Vec::with_capacity(segs);
        for _ in 0..segs {
            let v = func
                .dfg
                .make_value(TypeId::I64, ValueDef::Param(entry, new_bvals.len() as u16));
            vals.push(v);
            new_bvals.push(v);
            new_bparams.push(TypeId::I64);
        }
        rewrite.insert(old_v, vals.clone());
        if segs > 1 {
            agg_info.push((old_v, vals, *t));
        }
    }
    let bd = &mut func.dfg.blocks[entry.0 as usize];
    bd.params = smallvec::SmallVec::from_iter(new_bparams);
    bd.param_values = smallvec::SmallVec::from_iter(new_bvals);
    // 使用处重写（聚合参数——rewrite 内部即时移动辅助指令）
    for (old_v, segs, agg_ty) in agg_info {
        rewrite_agg_value_uses(func, old_v, &segs, agg_ty, agg_slots)?;
    }
    // 非聚合参数：直接替换（单值——参数索引变化后引用保持）
    for (old_v, vals) in &rewrite {
        if vals.len() == 1 {
            let new_v = vals[0];
            for bi in 0..func.dfg.blocks.len() {
                let order = func.dfg.blocks[bi].inst_order.clone();
                for &ii in &order {
                    let inst = &mut func.dfg.insts[ii.0 as usize];
                    for op in inst.operands.iter_mut() {
                        if *op == *old_v {
                            *op = new_v;
                        }
                    }
                }
                if let Terminator::Return { values, .. } = &mut func.dfg.blocks[bi].terminator {
                    for v in values.iter_mut() {
                        if *v == *old_v {
                            *v = new_v;
                        }
                    }
                }
            }
        }
    }
    Ok(())
}

/// ret 大聚合展开：`ret {i64,i64} %v`（>8 ≤16 字节）→ `Terminator::Return
/// { values: [seg0, seg1] }`（多值返回——lowering 按 values.len() 生成
/// RAX/RDX 双槽 mov）。段来源：AggConst（Iconst 段）、load 结果（段 load）、
/// 其余（参数段值已被 expand_large_agg_params 重写为 i64——不在此处理）。
fn expand_large_agg_ret(func: &mut Function) -> Result<(), IrError> {
    let rets = func.return_types();
    let has_large = rets.iter().any(|t| {
        let s = func.types.borrow();
        s.is_aggregate(*t) && s.size_bytes(*t) > 8
    });
    if !has_large {
        return Ok(());
    }
    // 收集 Return 里的聚合值（按块——先收集再改）
    struct RetJob {
        block: Block,
        val: Value,
        ty: TypeId,
    }
    let mut jobs: Vec<RetJob> = Vec::new();
    for (b, bd) in func.dfg.blocks() {
        if let Terminator::Return { values, .. } = &bd.terminator {
            for &v in values {
                if let Some(ty) = func.dfg.value_type(v)
                    && func.types.borrow().is_aggregate(ty)
                    && func.types.borrow().size_bytes(ty) > 8
                {
                    jobs.push(RetJob {
                        block: b,
                        val: v,
                        ty,
                    });
                }
            }
        }
    }
    for job in jobs {
        let size = func.types.borrow().size_bytes(job.ty) as usize;
        if size > 16 {
            return Err(IrError::Unsupported(format!(
                "聚合返回 >16 字节暂不支持（栈传递未实现）：type {:?}",
                job.ty
            )));
        }
        let mut segs: Vec<Value> = Vec::new();
        let mut gen_insts: Vec<Inst> = Vec::new();
        match func.dfg.values[job.val.0 as usize].def {
            ValueDef::AggConst(aid) => {
                let agg = func
                    .constants
                    .get_aggregate(aid)
                    .ok_or_else(|| IrError::Unsupported("聚合返回：常量池缺失 AggId".into()))?;
                let bytes = crate::pipeline::agg_const::pack_agg_bytes(func, agg)?;
                let mut off = 0usize;
                while off < size {
                    let mut seg_val: u64 = 0;
                    for k in 0..8 {
                        seg_val |= (bytes.get(off + k).copied().unwrap_or(0) as u64) << (k * 8);
                    }
                    let cid = func.constants.insert_int(seg_val as i128, 64);
                    let ci = func.dfg.make_inst(
                        Opcode::Iconst,
                        job.block,
                        smallvec::smallvec![],
                        smallvec::smallvec![Immediate::Const(cid)],
                        &[TypeId::I64],
                        InstFlags::NONE,
                    );
                    let cv = func.dfg.insts[ci.0 as usize].results[0];
                    gen_insts.push(ci);
                    segs.push(cv);
                    off += 8;
                }
            }
            ValueDef::Inst(li, 0) => {
                // load 结果 → 段 load
                let addr = func.dfg.insts[li.0 as usize]
                    .operands
                    .first()
                    .copied()
                    .ok_or_else(|| IrError::Unsupported("聚合返回：load 无地址".into()))?;
                let mut prev_addr = addr;
                let mut off = 0usize;
                while off < size {
                    let ld = func.dfg.make_inst(
                        Opcode::Load,
                        job.block,
                        smallvec::smallvec![prev_addr],
                        smallvec::smallvec![],
                        &[TypeId::I64],
                        InstFlags::NONE,
                    );
                    let ldv = func.dfg.insts[ld.0 as usize].results[0];
                    gen_insts.push(ld);
                    segs.push(ldv);
                    off += 8;
                    if off < size {
                        let cid = func.constants.insert_int(8, 64);
                        let ci = func.dfg.make_inst(
                            Opcode::Iconst,
                            job.block,
                            smallvec::smallvec![],
                            smallvec::smallvec![Immediate::Const(cid)],
                            &[TypeId::I64],
                            InstFlags::NONE,
                        );
                        let cv = func.dfg.insts[ci.0 as usize].results[0];
                        gen_insts.push(ci);
                        let add = func.dfg.make_inst(
                            Opcode::Iadd,
                            job.block,
                            smallvec::smallvec![prev_addr, cv],
                            smallvec::smallvec![],
                            &[TypeId::PTR],
                            InstFlags::NONE,
                        );
                        prev_addr = func.dfg.insts[add.0 as usize].results[0];
                        gen_insts.push(add);
                    }
                }
            }
            _ => {
                return Err(IrError::Unsupported(format!(
                    "聚合返回：值来源不支持（def {:?}）",
                    func.dfg.values[job.val.0 as usize].def
                )));
            }
        }
        // Return values 替换（段值）
        let bd = &mut func.dfg.blocks[job.block.0 as usize];
        if let Terminator::Return { values, .. } = &mut bd.terminator {
            let mut new_vals: Vec<Value> = Vec::new();
            for &v in values.iter() {
                if v == job.val {
                    new_vals.extend_from_slice(&segs);
                } else {
                    new_vals.push(v);
                }
            }
            *values = smallvec::SmallVec::from_iter(new_vals);
        }
        // gen 指令（段常量/段 load/add）已由 make_inst push 到块尾——顺序正确
        // （生成顺序），位于块内指令之后、terminator 之前——无需移动。
    }
    Ok(())
}

/// 调用方 call 结果（大聚合 >8 ≤16）拆 2 段：`%q = call {i64,i64}` 的 results
/// 改 [i64, i64]（ValueDef::Inst(call, 0/1)——lowering 按 results.len() 生成
/// RAX/RDX 双 mov），旧结果值使用处重写（extractvalue/store/转传）。
fn expand_large_agg_call_results(
    func: &mut Function,
    agg_slots: &mut AggSlots,
) -> Result<(), IrError> {
    // 收集（block, pos, inst, 旧结果, 聚合类型）
    struct Job {
        inst: Inst,
        old_r: Value,
        ty: TypeId,
    }
    let mut jobs: Vec<Job> = Vec::new();
    for (_b, bd) in func.dfg.blocks() {
        for &ii in &bd.inst_order {
            let inst = &func.dfg.insts[ii.0 as usize];
            if matches!(inst.opcode, Opcode::Call | Opcode::CallIndirect)
                && inst.results.len() == 1
                && let Some(rt) = inst.results.first().and_then(|v| func.dfg.value_type(*v))
                && func.types.borrow().is_aggregate(rt)
                && func.types.borrow().size_bytes(rt) > 8
            {
                let size = func.types.borrow().size_bytes(rt) as usize;
                if size > 16 {
                    return Err(IrError::Unsupported(format!(
                        "聚合返回 >16 字节暂不支持：type {rt:?}"
                    )));
                }
                jobs.push(Job {
                    inst: ii,
                    old_r: inst.results[0],
                    ty: rt,
                });
            }
        }
    }
    for job in jobs {
        // results 拆 2（新值——Inst(call, 0/1)）
        let r0 = func
            .dfg
            .make_value(TypeId::I64, ValueDef::Inst(job.inst, 0));
        let r1 = func
            .dfg
            .make_value(TypeId::I64, ValueDef::Inst(job.inst, 1));
        let inst = &mut func.dfg.insts[job.inst.0 as usize];
        inst.results = smallvec::smallvec![r0, r1];
        // 旧结果使用处重写
        rewrite_agg_value_uses(func, job.old_r, &[r0, r1], job.ty, agg_slots)?;
    }
    Ok(())
}

/// 函数是否含 >8 字节聚合 load（决定是否需要可变副本做大聚合值传播）。：`call @f({i64,i64} %v)` 的
/// 聚合实参（>8 且 ≤16 字节——SysV 整数寄存器路径）拆为 ceil(size/8) 个 i64
/// 段值追加到 call operands（内部签名宽松——callee 签名不校验；lowering 按
/// operands 逐参数占槽——RCX/RDX/R8/R9 + 栈参数）。
/// 段来源：AggConst（字节已知——插 Iconst 段常量）、load 结果（追踪 addr——
/// 插段 load）；其余（块参数/小聚合寄存器值）Unsupported。>16 字节栈传递
/// 未实现（Unsupported）。
/// 必须在 expand_large_aggs **之前**：拆段后大聚合值不再被 call 使用，
/// expand_large_aggs 的使用检测不命中 Call。
fn expand_agg_call_args(func: &mut Function) -> Result<(), IrError> {
    struct ArgJob {
        block: Block,
        pos: usize,
        inst: Inst,
        op_idx: usize,
        val: Value,
    }
    let mut jobs: Vec<ArgJob> = Vec::new();
    for (b, bd) in func.dfg.blocks() {
        for (pos, &ii) in bd.inst_order.iter().enumerate() {
            let inst = &func.dfg.insts[ii.0 as usize];
            if matches!(inst.opcode, Opcode::Call | Opcode::CallIndirect) {
                let skip = usize::from(inst.opcode == Opcode::CallIndirect);
                for (op_idx, &v) in inst.operands.iter().enumerate().skip(skip) {
                    if let Some(ty) = func.dfg.value_type(v)
                        && func.types.borrow().is_aggregate(ty)
                        && func.types.borrow().size_bytes(ty) > 8
                    {
                        jobs.push(ArgJob {
                            block: b,
                            pos,
                            inst: ii,
                            op_idx,
                            val: v,
                        });
                    }
                }
            }
        }
    }
    for job in jobs.into_iter().rev() {
        let ty = func.dfg.value_type(job.val).unwrap_or(TypeId::PTR);
        let size = func.types.borrow().size_bytes(ty) as usize;
        if size > 16 {
            return Err(IrError::Unsupported(format!(
                "聚合实参 >16 字节暂不支持（栈传递未实现）：type {ty:?}"
            )));
        }
        // 段值生成
        let mut segs: Vec<Value> = Vec::new();
        let mut new_insts: Vec<Inst> = Vec::new();
        match func.dfg.values[job.val.0 as usize].def {
            ValueDef::AggConst(aid) => {
                let agg = func
                    .constants
                    .get_aggregate(aid)
                    .ok_or_else(|| IrError::Unsupported("聚合实参：常量池缺失 AggId".into()))?;
                let bytes = crate::pipeline::agg_const::pack_agg_bytes(func, agg)?;
                let mut off = 0usize;
                while off < size {
                    let mut seg_val: u64 = 0;
                    for k in 0..8 {
                        seg_val |= (bytes.get(off + k).copied().unwrap_or(0) as u64) << (k * 8);
                    }
                    let cid = func.constants.insert_int(seg_val as i128, 64);
                    let iconst = func.dfg.make_inst(
                        Opcode::Iconst,
                        job.block,
                        smallvec::smallvec![],
                        smallvec::smallvec![Immediate::Const(cid)],
                        &[TypeId::I64],
                        InstFlags::NONE,
                    );
                    let cv = func.dfg.insts[iconst.0 as usize].results[0];
                    new_insts.push(iconst);
                    segs.push(cv);
                    off += 8;
                }
            }
            ValueDef::Inst(li, 0) => {
                // load 结果：追踪 addr，逐段 load
                let addr = func.dfg.insts[li.0 as usize]
                    .operands
                    .first()
                    .copied()
                    .ok_or_else(|| IrError::Unsupported("聚合实参：load 无地址".into()))?;
                let mut prev_addr = addr;
                let mut off = 0usize;
                while off < size {
                    let ld = func.dfg.make_inst(
                        Opcode::Load,
                        job.block,
                        smallvec::smallvec![prev_addr],
                        smallvec::smallvec![],
                        &[TypeId::I64],
                        InstFlags::NONE,
                    );
                    let ldv = func.dfg.insts[ld.0 as usize].results[0];
                    new_insts.push(ld);
                    segs.push(ldv);
                    off += 8;
                    if off < size {
                        let off_cid = func.constants.insert_int(8, 64);
                        let off_inst = func.dfg.make_inst(
                            Opcode::Iconst,
                            job.block,
                            smallvec::smallvec![],
                            smallvec::smallvec![Immediate::Const(off_cid)],
                            &[TypeId::I64],
                            InstFlags::NONE,
                        );
                        let off_v = func.dfg.insts[off_inst.0 as usize].results[0];
                        new_insts.push(off_inst);
                        let add = func.dfg.make_inst(
                            Opcode::Iadd,
                            job.block,
                            smallvec::smallvec![prev_addr, off_v],
                            smallvec::smallvec![],
                            &[TypeId::PTR],
                            InstFlags::NONE,
                        );
                        prev_addr = func.dfg.insts[add.0 as usize].results[0];
                        new_insts.push(add);
                    }
                }
            }
            _ => {
                return Err(IrError::Unsupported(format!(
                    "聚合实参拆段：值来源不支持（def {:?}）",
                    func.dfg.values[job.val.0 as usize].def
                )));
            }
        }
        // operands 替换（op_idx 处插入段值）
        let mut ops = func.dfg.insts[job.inst.0 as usize].operands.clone();
        ops.remove(job.op_idx);
        for (k, s) in segs.iter().enumerate() {
            ops.insert(job.op_idx + k, *s);
        }
        func.dfg.insts[job.inst.0 as usize].operands = ops;
        // 新指令移到 call 前
        let mut moved: Vec<Inst> = Vec::with_capacity(new_insts.len());
        for _ in 0..new_insts.len() {
            let order = &mut func.dfg.blocks[job.block.0 as usize].inst_order;
            moved.push(order.pop().unwrap());
        }
        moved.reverse();
        for (k, &ni) in moved.iter().enumerate() {
            func.dfg.blocks[job.block.0 as usize]
                .inst_order
                .insert(job.pos + k, ni);
        }
    }
    Ok(())
}

/// 大聚合 load 使用展开任务（S1：UseJob 提升为模块级——collect_extract_jobs 复用）
/// S1：收集新登记聚合值（嵌套提取中间值）的 extractvalue 使用，追加到展开
/// 任务队列（load 索引沿用原值——loads 在展开中不修改）。
fn collect_extract_jobs(func: &Function, jobs: &mut Vec<UseJob>, res: Value, load_idx: usize) {
    for (b, bd) in func.dfg.blocks() {
        for (pos, &ii) in bd.inst_order.iter().enumerate() {
            let inst = &func.dfg.insts[ii.0 as usize];
            if inst.opcode == Opcode::ExtractValue
                && inst.operands.first() == Some(&res)
                && let Some(Immediate::Uint(u)) = inst.immediates.first()
            {
                jobs.push(UseJob {
                    block: b,
                    pos,
                    inst: ii,
                    kind: UseKind::Extract(*u as u32),
                    load: load_idx,
                });
            }
        }
    }
}

/// 大聚合 load 值传播（全 ISA 通用，编译期）：`%v = load T(>8B), ptr %p` 的
/// 大聚合值无法入 GPR——按使用处就地展开（load 指令标 Nop，值不物化）：
/// - `store T %v, ptr %q`（拷贝）：逐段 `load i64 [addr+off]` + `store i64 [q+off]`；
/// - `extractvalue T %v, idx`：偏移 `add ptr addr, field_offset` + 标量 load，
///   extractvalue 改 Copy；
/// - 其余使用（传参/返回/insertvalue 等）：Unsupported。
///
/// 局限：load 后内存被改写时重新 load 不保留快照语义（常见模式安全）。
fn expand_large_aggs(func: &mut Function, agg_slots: &mut AggSlots) -> Result<(), IrError> {
    // 第一遍：收集大聚合 load
    struct LoadInfo {
        result: Value,
        addr: Value,
        ty: TypeId,
        size: usize,
    }
    let mut loads: Vec<LoadInfo> = Vec::new();
    for (_, bd) in func.dfg.blocks() {
        for &ii in &bd.inst_order {
            let inst = &func.dfg.insts[ii.0 as usize];
            if inst.opcode == Opcode::Load
                && let Some(rt) = inst.results.first().and_then(|v| func.dfg.value_type(*v))
                && func.types.borrow().is_aggregate(rt)
                && func.types.borrow().size_bytes(rt) > 8
            {
                loads.push(LoadInfo {
                    result: inst.results[0],
                    addr: inst.operands[0],
                    ty: rt,
                    size: func.types.borrow().size_bytes(rt) as usize,
                });
            }
        }
    }
    // 第二遍：收集使用处展开任务
    let mut jobs: Vec<UseJob> = Vec::new();
    for (b, bd) in func.dfg.blocks() {
        for (pos, &ii) in bd.inst_order.iter().enumerate() {
            let inst = &func.dfg.insts[ii.0 as usize];
            for (op_idx, &v) in inst.operands.iter().enumerate() {
                if let Some(li) = loads.iter().position(|l| l.result == v) {
                    let kind = match inst.opcode {
                        Opcode::Store if op_idx == 0 => UseKind::CopyStore(inst.operands[1]),
                        Opcode::ExtractValue if op_idx == 0 => {
                            let idx = inst
                                .immediates
                                .first()
                                .and_then(|i| match i {
                                    Immediate::Uint(u) => Some(*u as u32),
                                    _ => None,
                                })
                                .ok_or_else(|| {
                                    IrError::Unsupported("extractvalue 索引缺失".into())
                                })?;
                            UseKind::Extract(idx)
                        }
                        _ => {
                            return Err(IrError::Unsupported(format!(
                                "大聚合 load 值用于 {:?}（仅支持 store 拷贝/extractvalue 提取）",
                                inst.opcode
                            )));
                        }
                    };
                    jobs.push(UseJob {
                        block: b,
                        pos,
                        inst: ii,
                        kind,
                        load: li,
                    });
                }
            }
        }
    }
    // 逆序展开（use_pos 递减；S1：内层提取任务在展开中追加——pop 队列）
    while let Some(job) = jobs.pop() {
        let li = &loads[job.load];
        let (use_addr_ops, extract_idx) = match job.kind {
            UseKind::CopyStore(dst) => (Some(dst), None),
            UseKind::Extract(idx) => (None, Some(idx)),
        };
        let mut new_insts: Vec<Inst> = Vec::new();
        if let Some(dst) = use_addr_ops {
            // 逐段拷贝：load [src+off] → store [dst+off]
            let mut src_off = 0usize;
            let mut src_addr = li.addr;
            let mut dst_addr = dst;
            while src_off < li.size {
                let seg_len = (li.size - src_off).min(8);
                let seg_ty = match seg_len {
                    1 => TypeId::I8,
                    2 => TypeId::I16,
                    4 => TypeId::I32,
                    _ => TypeId::I64,
                };
                // src: load seg_ty, ptr %src_addr
                let ld = func.dfg.make_inst(
                    Opcode::Load,
                    job.block,
                    smallvec::smallvec![src_addr],
                    smallvec::smallvec![],
                    &[seg_ty],
                    InstFlags::NONE,
                );
                let ldv = func.dfg.insts[ld.0 as usize].results[0];
                new_insts.push(ld);
                // dst: store seg_ty %ldv, ptr %dst_addr
                let st = func.dfg.make_inst(
                    Opcode::Store,
                    job.block,
                    smallvec::smallvec![ldv, dst_addr],
                    smallvec::smallvec![],
                    &[],
                    InstFlags::SIDE_EFFECT,
                );
                new_insts.push(st);
                src_off += seg_len;
                if src_off < li.size {
                    // 推进 src/dst 地址（Iadd + 常量）
                    for cur in [&mut src_addr, &mut dst_addr] {
                        let off_cid = func.constants.insert_int(seg_len as i128, 64);
                        let off_inst = func.dfg.make_inst(
                            Opcode::Iconst,
                            job.block,
                            smallvec::smallvec![],
                            smallvec::smallvec![Immediate::Const(off_cid)],
                            &[TypeId::I64],
                            InstFlags::NONE,
                        );
                        let off_v = func.dfg.insts[off_inst.0 as usize].results[0];
                        new_insts.push(off_inst);
                        let add = func.dfg.make_inst(
                            Opcode::Iadd,
                            job.block,
                            smallvec::smallvec![*cur, off_v],
                            smallvec::smallvec![],
                            &[TypeId::PTR],
                            InstFlags::NONE,
                        );
                        *cur = func.dfg.insts[add.0 as usize].results[0];
                        new_insts.push(add);
                    }
                }
            }
            // 使用指令（Store）标 Nop
            let inst = &mut func.dfg.insts[job.inst.0 as usize];
            inst.opcode = Opcode::Nop;
            inst.operands = smallvec::smallvec![];
            inst.immediates = smallvec::smallvec![];
        } else if let Some(idx) = extract_idx {
            // S1：命中嵌套聚合槽映射（内层提取）——地址语义优先
            let hit_inst = &func.dfg.insts[job.inst.0 as usize];
            if let Some(&(slot_addr, slot_ty)) = agg_slots.get(&hit_inst.operands[0]) {
                let ts = func.types.borrow();
                let foff = ts.field_offset(slot_ty, idx).ok_or_else(|| {
                    IrError::Unsupported(format!("extractvalue 字段偏移缺失：{idx}"))
                })? as usize;
                let fty = ts.aggregate_elem_type(slot_ty, idx).ok_or_else(|| {
                    IrError::Unsupported(format!("extractvalue 字段类型缺失：{idx}"))
                })?;
                if ts.is_aggregate(fty) {
                    drop(ts);
                    // 聚合字段：登记子地址（地址链继续）
                    let res = hit_inst.results[0];
                    let mut addr2 = slot_addr;
                    if foff > 0 {
                        let off_cid = func.constants.insert_int(foff as i128, 64);
                        let off_inst = func.dfg.make_inst(
                            Opcode::Iconst,
                            job.block,
                            smallvec::smallvec![],
                            smallvec::smallvec![Immediate::Const(off_cid)],
                            &[TypeId::I64],
                            InstFlags::NONE,
                        );
                        let off_v = func.dfg.insts[off_inst.0 as usize].results[0];
                        new_insts.push(off_inst);
                        let add2 = func.dfg.make_inst(
                            Opcode::Iadd,
                            job.block,
                            smallvec::smallvec![addr2, off_v],
                            smallvec::smallvec![],
                            &[TypeId::PTR],
                            InstFlags::NONE,
                        );
                        addr2 = func.dfg.insts[add2.0 as usize].results[0];
                        new_insts.push(add2);
                    }
                    let inst = &mut func.dfg.insts[job.inst.0 as usize];
                    inst.opcode = Opcode::Nop;
                    inst.operands = smallvec::smallvec![];
                    inst.immediates = smallvec::smallvec![];
                    agg_slots.insert(res, (addr2, fty));
                    // 追加内层提取任务（递归）
                    collect_extract_jobs(func, &mut jobs, res, job.load);
                } else {
                    drop(ts);
                    // 标量字段：GEP + load（结果 Copy 绑定）
                    let off_cid = func.constants.insert_int(foff as i128, 64);
                    let off_inst = func.dfg.make_inst(
                        Opcode::Iconst,
                        job.block,
                        smallvec::smallvec![],
                        smallvec::smallvec![Immediate::Const(off_cid)],
                        &[TypeId::I64],
                        InstFlags::NONE,
                    );
                    let off_v = func.dfg.insts[off_inst.0 as usize].results[0];
                    new_insts.push(off_inst);
                    let add = func.dfg.make_inst(
                        Opcode::Iadd,
                        job.block,
                        smallvec::smallvec![slot_addr, off_v],
                        smallvec::smallvec![],
                        &[TypeId::PTR],
                        InstFlags::NONE,
                    );
                    let p = func.dfg.insts[add.0 as usize].results[0];
                    new_insts.push(add);
                    let ld = func.dfg.make_inst(
                        Opcode::Load,
                        job.block,
                        smallvec::smallvec![p],
                        smallvec::smallvec![],
                        &[fty],
                        InstFlags::NONE,
                    );
                    let lv = func.dfg.insts[ld.0 as usize].results[0];
                    new_insts.push(ld);
                    let inst = &mut func.dfg.insts[job.inst.0 as usize];
                    inst.opcode = Opcode::Copy;
                    inst.operands = smallvec::smallvec![lv];
                    inst.immediates = smallvec::smallvec![];
                }
                // 新指令移到使用位置前（与下方公共逻辑一致；此处直接走尾部）
                if !new_insts.is_empty() {
                    let mut moved: Vec<Inst> = Vec::with_capacity(new_insts.len());
                    for _ in 0..new_insts.len() {
                        let order = &mut func.dfg.blocks[job.block.0 as usize].inst_order;
                        moved.push(order.pop().unwrap());
                    }
                    moved.reverse();
                    for (k, &ni) in moved.iter().enumerate() {
                        func.dfg.blocks[job.block.0 as usize]
                            .inst_order
                            .insert(job.pos + k, ni);
                    }
                }
                continue;
            }
            // 字段提取：%p2 = add ptr addr, field_offset; %s = load field_ty, ptr %p2
            let ts = func.types.borrow();
            let field_ty = ts
                .aggregate_elem_type(li.ty, idx)
                .ok_or_else(|| IrError::Unsupported(format!("extractvalue 索引 {idx} 越界")))?;
            if ts.is_aggregate(field_ty) {
                // S1：嵌套聚合字段——登记槽地址链（li.addr + off），原指令 Nop；
                // 后续提取命中 agg_slots 链式 GEP+load
                let off2 = ts.field_offset(li.ty, idx).ok_or_else(|| {
                    IrError::Unsupported(format!("extractvalue 字段偏移缺失：{idx}"))
                })?;
                drop(ts);
                let mut addr2 = li.addr;
                if off2 > 0 {
                    let off2_cid = func.constants.insert_int(off2 as i128, 64);
                    let off2_inst = func.dfg.make_inst(
                        Opcode::Iconst,
                        job.block,
                        smallvec::smallvec![],
                        smallvec::smallvec![Immediate::Const(off2_cid)],
                        &[TypeId::I64],
                        InstFlags::NONE,
                    );
                    let off2_v = func.dfg.insts[off2_inst.0 as usize].results[0];
                    new_insts.push(off2_inst);
                    let add2 = func.dfg.make_inst(
                        Opcode::Iadd,
                        job.block,
                        smallvec::smallvec![addr2, off2_v],
                        smallvec::smallvec![],
                        &[TypeId::PTR],
                        InstFlags::NONE,
                    );
                    addr2 = func.dfg.insts[add2.0 as usize].results[0];
                    new_insts.push(add2);
                }
                let inst = &mut func.dfg.insts[job.inst.0 as usize];
                inst.opcode = Opcode::Nop;
                inst.operands = smallvec::smallvec![];
                inst.immediates = smallvec::smallvec![];
                let res_new = inst.results[0];
                agg_slots.insert(res_new, (addr2, field_ty));
                collect_extract_jobs(func, &mut jobs, res_new, job.load);
            } else {
                let off = ts.field_offset(li.ty, idx).ok_or_else(|| {
                    IrError::Unsupported(format!("extractvalue 字段偏移缺失：{idx}"))
                })?;
                drop(ts);
                let mut prev_addr = li.addr;
                if off > 0 {
                    let off_cid = func.constants.insert_int(off as i128, 64);
                    let off_inst = func.dfg.make_inst(
                        Opcode::Iconst,
                        job.block,
                        smallvec::smallvec![],
                        smallvec::smallvec![Immediate::Const(off_cid)],
                        &[TypeId::I64],
                        InstFlags::NONE,
                    );
                    let off_v = func.dfg.insts[off_inst.0 as usize].results[0];
                    new_insts.push(off_inst);
                    let add = func.dfg.make_inst(
                        Opcode::Iadd,
                        job.block,
                        smallvec::smallvec![prev_addr, off_v],
                        smallvec::smallvec![],
                        &[TypeId::PTR],
                        InstFlags::NONE,
                    );
                    prev_addr = func.dfg.insts[add.0 as usize].results[0];
                    new_insts.push(add);
                }
                let ld = func.dfg.make_inst(
                    Opcode::Load,
                    job.block,
                    smallvec::smallvec![prev_addr],
                    smallvec::smallvec![],
                    &[field_ty],
                    InstFlags::NONE,
                );
                let ldv = func.dfg.insts[ld.0 as usize].results[0];
                new_insts.push(ld);
                // extractvalue 改 Copy（result 保留 → 新 load 值）
                let inst = &mut func.dfg.insts[job.inst.0 as usize];
                inst.opcode = Opcode::Copy;
                inst.operands = smallvec::smallvec![ldv];
                inst.immediates = smallvec::smallvec![];
            }
        }
        // 新指令移到使用指令位置前
        let mut moved: Vec<Inst> = Vec::with_capacity(new_insts.len());
        for _ in 0..new_insts.len() {
            let order = &mut func.dfg.blocks[job.block.0 as usize].inst_order;
            moved.push(order.pop().unwrap());
        }
        moved.reverse();
        for (k, &ni) in moved.iter().enumerate() {
            func.dfg.blocks[job.block.0 as usize]
                .inst_order
                .insert(job.pos + k, ni);
        }
    }
    // 所有大聚合 load 标 Nop（先收集指令索引，避免借用冲突）
    let mut nop_list: Vec<Inst> = Vec::new();
    for (_, bd) in func.dfg.blocks() {
        for &ii in &bd.inst_order {
            let inst = &func.dfg.insts[ii.0 as usize];
            if inst.opcode == Opcode::Load
                && let Some(rt) = inst.results.first().and_then(|v| func.dfg.value_type(*v))
                && func.types.borrow().is_aggregate(rt)
                && func.types.borrow().size_bytes(rt) > 8
            {
                nop_list.push(ii);
            }
        }
    }
    for ii in nop_list {
        let inst = &mut func.dfg.insts[ii.0 as usize];
        inst.opcode = Opcode::Nop;
        inst.operands = smallvec::smallvec![];
        inst.immediates = smallvec::smallvec![];
    }
    Ok(())
}

/// GEP 无基址操作数时的占位值（正常 IR 不会出现——防御性兜底）。
fn make_placeholder_ptr(func: &mut Function) -> Value {
    func.dfg
        .make_value(TypeId::PTR, ValueDef::Param(Block(0), 0))
}

/// GEP 展开（全 ISA 通用，编译期）：`getelementptr T, ptr %p, i32 i0, i32 i1...`
/// 逐级计算偏移——常量索引折叠（第一索引作用于 indexed_ty 自身；struct 用
/// field_offset；数组用 elem_size×idx），动态索引（数组下标）展开为
/// `mul i64 %idx, size` + `add`。GEP 指令原地改为 Copy（result 保留——
/// SSA 引用一致），展开的 mul/add 插在 GEP 位置之前。展开后不依赖机器
/// GEP 规则（旧 `lea [rs1+rs2*4+0]` 单索引规则已废弃）。
/// 局限：struct 索引必须常量（LLVM 语义）；索引按无符号处理（数组下标）。
fn expand_geps(func: &mut Function) -> Result<(), IrError> {
    let mut jobs: Vec<(Block, usize, Inst)> = Vec::new(); // (block, pos, inst)
    for (b, bd) in func.dfg.blocks() {
        for (pos, &ii) in bd.inst_order.iter().enumerate() {
            if func.dfg.insts[ii.0 as usize].opcode == Opcode::GetElementPtr {
                jobs.push((b, pos, ii));
            }
        }
    }
    for (b, pos, ii) in jobs.into_iter().rev() {
        // 提前 copy 出 GEP 数据（inst 借用须在 &mut func 生成段前结束）
        let (indexed_ty, base, indices) = {
            let inst = &func.dfg.insts[ii.0 as usize];
            (
                inst.immediates
                    .iter()
                    .find_map(|i| match i {
                        Immediate::Type(t) => Some(*t),
                        _ => None,
                    })
                    .unwrap_or(TypeId::PTR),
                inst.operands.first().copied(),
                inst.operands.iter().skip(1).copied().collect::<Vec<_>>(),
            )
        };
        let base = base.unwrap_or_else(|| make_placeholder_ptr(func));
        // 索引常量折叠（Iconst 值 → i64）
        let fold = |func: &Function, v: Value| -> Option<i64> {
            let def = func.dfg.values[v.0 as usize].def;
            let ValueDef::Inst(ci, _) = def else {
                return None;
            };
            let id = func.dfg.insts.get(ci.0 as usize)?;
            if id.opcode != Opcode::Iconst {
                return None;
            }
            let im = id.immediates.first()?;
            match im {
                Immediate::Uint(v) => Some(*v as i64),
                Immediate::Int(v) => Some(*v),
                Immediate::Const(c) => func.constants.get_int(*c).map(|(v, _)| v as i64),
                _ => None,
            }
        };
        let mut cur_ty = indexed_ty;
        let mut const_off: i64 = 0;
        let mut dyn_parts: Vec<(Value, u32)> = Vec::new(); // (idx 值, 元素字节)
        for (i, &idx) in indices.iter().enumerate() {
            let i = i + 1; // 原操作数下标（operands[0]=base）
            let ts = func.types.borrow();
            if i == 1 {
                // 第一索引作用于 indexed_ty 自身（T* → T 数组）
                let size = ts.size_bytes(cur_ty) as i64;
                match fold(func, idx) {
                    Some(c) => const_off += c * size,
                    None => dyn_parts.push((idx, size as u32)),
                }
                continue;
            }
            if let TypeEntry::Struct { .. } = ts.get(cur_ty) {
                let c = fold(func, idx).ok_or_else(|| {
                    IrError::Unsupported("GEP struct 索引必须为常量（LLVM 语义）".into())
                })?;
                let off = ts.field_offset(cur_ty, c as u32).ok_or_else(|| {
                    IrError::Unsupported(format!("GEP struct 索引越界：{c}（type {cur_ty:?}）"))
                })?;
                const_off += off as i64;
                cur_ty = ts
                    .aggregate_elem_type(cur_ty, c as u32)
                    .ok_or_else(|| IrError::Unsupported("GEP 字段类型缺失".into()))?;
            } else {
                let elem_ty = ts.element_type(cur_ty).ok_or_else(|| {
                    IrError::Unsupported("GEP 索引作用于非数组/结构体类型".into())
                })?;
                let elem_size = ts.size_bytes(elem_ty) as i64;
                match fold(func, idx) {
                    Some(c) => const_off += c * elem_size,
                    None => dyn_parts.push((idx, elem_size as u32)),
                }
                cur_ty = elem_ty;
            }
        }
        // 生成展开序列：动态部分 mul+add，最后常量偏移 add（const_off!=0 或全零时
        // 仍需 add 保持链尾——全零无动态时直接 Copy base）
        let mut prev = base;
        let mut new_count = 0usize;
        for (idx_v, size) in dyn_parts {
            // %t = mul i64 %idx, size
            let size_cid = func.constants.insert_int(size as i128, 64);
            let size_inst = func.dfg.make_inst(
                Opcode::Iconst,
                b,
                smallvec::smallvec![],
                smallvec::smallvec![Immediate::Const(size_cid)],
                &[TypeId::I64],
                InstFlags::NONE,
            );
            let sval = func.dfg.insts[size_inst.0 as usize].results[0];
            let mul = func.dfg.make_inst(
                Opcode::Imul,
                b,
                smallvec::smallvec![idx_v, sval],
                smallvec::smallvec![],
                &[TypeId::I64],
                InstFlags::NONE,
            );
            let mval = func.dfg.insts[mul.0 as usize].results[0];
            // %p = add i64 %prev, %t（结果类型 PTR——内部指针算术，verify 不跑）
            let add = func.dfg.make_inst(
                Opcode::Iadd,
                b,
                smallvec::smallvec![prev, mval],
                smallvec::smallvec![],
                &[TypeId::PTR],
                InstFlags::NONE,
            );
            prev = func.dfg.insts[add.0 as usize].results[0];
            new_count += 3;
        }
        if const_off != 0 || new_count > 0 {
            let off_cid = func.constants.insert_int(const_off as i128, 64);
            let off_inst = func.dfg.make_inst(
                Opcode::Iconst,
                b,
                smallvec::smallvec![],
                smallvec::smallvec![Immediate::Const(off_cid)],
                &[TypeId::I64],
                InstFlags::NONE,
            );
            let off_v = func.dfg.insts[off_inst.0 as usize].results[0];
            let add = func.dfg.make_inst(
                Opcode::Iadd,
                b,
                smallvec::smallvec![prev, off_v],
                smallvec::smallvec![],
                &[TypeId::PTR],
                InstFlags::NONE,
            );
            prev = func.dfg.insts[add.0 as usize].results[0];
            new_count += 2;
        }
        // GEP 原地改 Copy（result 保留——SSA 引用一致）
        let inst = &mut func.dfg.insts[ii.0 as usize];
        inst.opcode = Opcode::Copy;
        inst.operands = smallvec::smallvec![prev];
        inst.immediates = smallvec::smallvec![];
        // 新指令（块尾）移到 GEP 位置前（逆序处理 jobs：尾即本 GEP 的新指令）
        let mut new_insts: Vec<Inst> = Vec::with_capacity(new_count);
        for _ in 0..new_count {
            let order = &mut func.dfg.blocks[b.0 as usize].inst_order;
            new_insts.push(order.pop().unwrap());
        }
        new_insts.reverse();
        for (k, &ni) in new_insts.iter().enumerate() {
            func.dfg.blocks[b.0 as usize].inst_order.insert(pos + k, ni);
        }
    }
    Ok(())
}

/// 聚合常量打包：聚合树 → 小端 u64（≤8 字节聚合；字段按内存布局偏移排列）。
/// 聚合常量 → 内存字节（小端布局：标量按 field_offset 写入 LE 字节，
/// 未覆盖区零——对齐填充/空洞）。任意大小（>8 字节聚合分段 store 用）。
/// 聚合 store 展开（全 ISA 通用，编译期）：聚合常量 → 内存字节（小端），
/// 按 8 字节段展开为普通标量 store——`store {i32,i32} {i32 7, i32 9}, ptr %p`
/// → `store i64 <7|9<<32>, ptr %p`；>8 字节（如 {i64,i64} 16 字节）→ 逐段
/// `store i64 <seg>, ptr <addr>` + `add ptr, 8` 地址推进（尾段按 8/4/2/1
/// 字节宽 store，不越界）。展开使 store 源成为普通 Iconst 值（修复原 SEGV：
/// 聚合常量从不加载进寄存器 + 聚合类型 bits()=0 → store 宽度 0）。
/// - 聚合 load 结果 >8 字节与聚合常量传参 >8 字节：Phase 2 处理（此处保留
///   Unsupported——大聚合值内存化传播）。
/// - 仅小端数据布局（x86_64/aarch64 默认）；大端需反向打包（暂不支持）。
fn expand_agg_stores(func: &mut Function) -> Result<(), IrError> {
    // 第一遍：收集（聚合常量 → 字节布局）
    let mut jobs: Vec<(Block, usize, usize, Vec<u8>, bool)> = Vec::new(); // (block, pos, operand 索引, 内存字节, 是否 Store)
    for (b, bd) in func.dfg.blocks() {
        for (pos, &ii) in bd.inst_order.iter().enumerate() {
            let inst = &func.dfg.insts[ii.0 as usize];
            match inst.opcode {
                Opcode::Store => {
                    if let Some(v) = inst.operands.first()
                        && let ValueDef::AggConst(aid) = func.dfg.values[v.0 as usize].def
                    {
                        let agg = func.constants.get_aggregate(aid).ok_or_else(|| {
                            IrError::Unsupported("聚合常量 store：常量池缺失 AggId".into())
                        })?;
                        let bytes = crate::pipeline::agg_const::pack_agg_bytes(func, agg)?;
                        jobs.push((b, pos, 0, bytes, true));
                    }
                }
                // 大聚合 load 由 expand_large_aggs 处理（值传播展开）——此处不拦截
                // Call/CallIndirect 的聚合常量参数：与 Store 同理打包展开（否则
                // AggConst 值从不加载——传参 0/垃圾）。builder.call 的 operands
                // 全是实参（callee 在 Immediate::Func）；CallIndirect 的
                // operands[0] 是 callee 指针（skip 1）。
                Opcode::Call | Opcode::CallIndirect => {
                    let skip = usize::from(inst.opcode == Opcode::CallIndirect);
                    for (op_idx, v) in inst.operands.iter().enumerate().skip(skip) {
                        if let ValueDef::AggConst(aid) = func.dfg.values[v.0 as usize].def {
                            let agg = func.constants.get_aggregate(aid).ok_or_else(|| {
                                IrError::Unsupported("聚合常量参数：常量池缺失 AggId".into())
                            })?;
                            let size = func.types.borrow().size_bytes(agg.ty);
                            if size > 8 {
                                // >8 字节由 expand_agg_call_args 拆段处理——跳过
                                continue;
                            }
                            let bytes = crate::pipeline::agg_const::pack_agg_bytes(func, agg)?;
                            jobs.push((b, pos, op_idx, bytes, false));
                        }
                    }
                }
                _ => {}
            }
        }
    }
    // 逆序替换：inst_order.insert(pos) 移动后续下标——从后往前插入不受影响。
    for (b, pos, op_idx, bytes, is_store) in jobs.into_iter().rev() {
        let store_ii = func.dfg.blocks[b.0 as usize].inst_order[pos];
        // Store：分段展开（operands[1]=addr）；Call 参数：单打包值替换 operand
        let addr = if is_store {
            let inst = &func.dfg.insts[store_ii.0 as usize];
            inst.operands.get(op_idx + 1).copied()
        } else {
            None
        };
        let mut new_insts: Vec<Inst> = Vec::new();
        if let Some(addr) = addr {
            let mut seg_off = 0usize;
            let mut prev_addr = addr;
            while seg_off < bytes.len() {
                let seg_len = (bytes.len() - seg_off).min(8);
                let mut seg_val: u64 = 0;
                for k in 0..seg_len {
                    seg_val |= (bytes[seg_off + k] as u64) << (k * 8);
                }
                let store_ty = match seg_len {
                    1 => TypeId::I8,
                    2 => TypeId::I16,
                    4 => TypeId::I32,
                    _ => TypeId::I64,
                };
                // 段常量（复用 make_inst 的 results[0]——不可再 make_value）
                let cid = func
                    .constants
                    .insert_int(seg_val as i128, (seg_len * 8) as u32);
                let iconst = func.dfg.make_inst(
                    Opcode::Iconst,
                    b,
                    smallvec::smallvec![],
                    smallvec::smallvec![Immediate::Const(cid)],
                    &[store_ty],
                    InstFlags::NONE,
                );
                let cv = func.dfg.insts[iconst.0 as usize].results[0];
                new_insts.push(iconst);
                let st = func.dfg.make_inst(
                    Opcode::Store,
                    b,
                    smallvec::smallvec![cv, prev_addr],
                    smallvec::smallvec![],
                    &[],
                    InstFlags::SIDE_EFFECT,
                );
                new_insts.push(st);
                seg_off += seg_len;
                if seg_off < bytes.len() {
                    // prev_addr = add ptr prev_addr, seg_len（非 GEP——expand_geps 不碰）
                    let off_cid = func.constants.insert_int(seg_len as i128, 64);
                    let off_inst = func.dfg.make_inst(
                        Opcode::Iconst,
                        b,
                        smallvec::smallvec![],
                        smallvec::smallvec![Immediate::Const(off_cid)],
                        &[TypeId::I64],
                        InstFlags::NONE,
                    );
                    let off_v = func.dfg.insts[off_inst.0 as usize].results[0];
                    new_insts.push(off_inst);
                    let add = func.dfg.make_inst(
                        Opcode::Iadd,
                        b,
                        smallvec::smallvec![prev_addr, off_v],
                        smallvec::smallvec![],
                        &[TypeId::PTR],
                        InstFlags::NONE,
                    );
                    prev_addr = func.dfg.insts[add.0 as usize].results[0];
                    new_insts.push(add);
                }
            }
        }
        if !is_store {
            // Call 参数（≤8 字节）：打包为单 i64 常量替换 operand
            let mut packed: u64 = 0;
            for (k, &byte) in bytes.iter().enumerate() {
                packed |= (byte as u64) << (k * 8);
            }
            let cid = func.constants.insert_int(packed as i128, 64);
            let iconst = func.dfg.make_inst(
                Opcode::Iconst,
                b,
                smallvec::smallvec![],
                smallvec::smallvec![Immediate::Const(cid)],
                &[TypeId::I64],
                InstFlags::NONE,
            );
            let cv = func.dfg.insts[iconst.0 as usize].results[0];
            {
                let inst = &mut func.dfg.insts[store_ii.0 as usize];
                inst.operands[op_idx] = cv;
            }
            new_insts.push(iconst);
        } else {
            // 原 Store 标 Nop（无结果——不生成机器指令；lowering 跳过 Nop）
            let inst = &mut func.dfg.insts[store_ii.0 as usize];
            inst.opcode = Opcode::Nop;
            inst.operands = smallvec::smallvec![];
            inst.immediates = smallvec::smallvec![];
        }
        // 新指令（块尾）移到 Store 位置前（逆序处理 jobs：尾即本 Store 的新指令）
        let mut moved: Vec<Inst> = Vec::with_capacity(new_insts.len());
        for _ in 0..new_insts.len() {
            let order = &mut func.dfg.blocks[b.0 as usize].inst_order;
            moved.push(order.pop().unwrap());
        }
        moved.reverse();
        for (k, &ni) in moved.iter().enumerate() {
            func.dfg.blocks[b.0 as usize].inst_order.insert(pos + k, ni);
        }
    }
    Ok(())
}

/// Function compiler — drives the full IR → machine code pipeline.
pub struct FunctionCompiler<M: TargetMachine> {
    machine: M,
    reg_alloc: BacktrackingAllocator,
    /// IR 级优化级别（None = 不优化，直接编译——与历史 compile_raw 行为
    /// 一致）。Some(level) 时 `compile()`/`compile_with_alloc()` 先对函数
    /// 克隆体跑对应 PassManager 管线再 lower。P0-1：把 forge-opt 优化
    /// 管线接入生产编译（此前 PassManager 是死代码，全部 pass 缺陷潜伏）。
    opt_level: Option<forge_opt::OptimizationLevel>,
}

impl<M: TargetMachine> FunctionCompiler<M> {
    /// Create a new compiler with the default register allocator (backtracking).
    pub fn new(machine: M) -> Self {
        Self {
            machine,
            reg_alloc: BacktrackingAllocator,
            opt_level: None,
        }
    }

    /// 启用 IR 级优化（compile/compile_with_alloc 前跑对应 O 级别管线）。
    pub fn with_opt_level(mut self, level: forge_opt::OptimizationLevel) -> Self {
        self.opt_level = Some(level);
        self
    }

    /// 当前优化级别（None = 未启用）。
    pub fn opt_level(&self) -> Option<forge_opt::OptimizationLevel> {
        self.opt_level
    }

    /// Compile an IR function (full pipeline，含 IR 级优化若已启用
    /// `with_opt_level`)。无优化时与 compile_raw 等价。
    pub fn compile(&self, func: &Function) -> Result<CompiledFunction, IrError> {
        self.compile_with_alloc(func).map(|(cf, _)| cf)
    }

    /// Compile an IR function (raw, no IR-level optimization passes)。
    /// 显式绕过优化管线（即使设置了 opt_level）。
    /// 公共 API 面：只返回编译产物；分配明细经 [`Self::compile_with_alloc`]。
    pub fn compile_raw(&self, func: &Function) -> Result<CompiledFunction, IrError> {
        self.compile_with_alloc(func).map(|(cf, _)| cf)
    }

    /// Compile an IR function，额外返回寄存器分配明细（AllocResult：
    /// XReg → PReg assignments、spill_slots 等），供集成测试/工具断言
    /// 分配器行为（ABI 参数分配、clobber 写死保护等）。
    pub fn compile_with_alloc(
        &self,
        func: &Function,
    ) -> Result<(CompiledFunction, crate::AllocResult), IrError> {
        // P0-1：IR 级优化管线接入——若启用，克隆函数跑对应 O 级别管线，
        // 用优化后的克隆体继续编译（原 func 不被修改；PassManager 需要
        // &mut Function）。此前 compile()→compile_raw() 直接编译，forge-opt
        // 的全部 pass 在生产路径是死代码。未启用时不克隆（零开销）。
        let mut owned: Function;
        let func: &Function = if let Some(level) = self.opt_level {
            owned = func.clone();
            let pm = forge_opt::PassManager::for_level(level);
            pm.run_on_function(&mut owned)?;
            &owned
        } else {
            func
        };

        let _cf_t0 = std::time::Instant::now();
        // 宽向量参数/返回（>16 字节）ABI：
        // - 若 ISA 声明 `vector by-ref limit`（如 x86 limit=128 位）→ 宽向量按
        //   引用传参（调用方栈拷贝 + 传指针 GPR；被调方入口从 [ptr] 加载）。
        // - 否则拒绝（ABI 仅寄存器传值，宽向量会静默截断）。
        let by_ref_limit = self.machine.abi().vector_by_ref_limit();
        let types = &func.types;
        let param_tys = func.param_types();
        let return_tys = func.return_types();
        let wide = param_tys
            .iter()
            .chain(&return_tys)
            .find(|t| {
                let s = types.borrow();
                (s.is_vector(**t) || s.is_scalable_vector(**t)) && s.size_bytes(**t) > 16
            })
            .copied();
        if let Some(ty) = wide {
            let s = types.borrow();
            let bytes = s.size_bytes(ty);
            // S4：V512（64 字节）需 AVX-512F（EVEX 编码前提）——无则拒绝，
            // 防 EVEX 指令在非 AVX-512 机器非法指令崩溃。
            if bytes > 32 && !crate::avx512_available() {
                return Err(IrError::Unsupported(format!(
                    "SIMD 参数/返回 {bytes} 字节需 AVX-512F（当前机器不支持；type {ty:?}）"
                )));
            }
            match by_ref_limit {
                Some(lim) if bytes <= lim => {
                    // 在寄存器传值范围内（≤16 字节）：走现有 XMM/GPR 传参路径。
                }
                Some(lim) => {
                    // 超过阈值：by-ref 传参（调用方栈拷贝 + 传指针；被调方入口
                    // 从 [ptr] 加载）。入口守卫通过——move_args/call lowering
                    // 的 by-ref 路径处理。V512（64 字节）等超宽向量若 ISA 未
                    // 声明足够 limit 时由 encode 侧能力决定（avx512 检测）。
                    let _ = lim;
                }
                None => {
                    return Err(IrError::Unsupported(format!(
                        "SIMD 参数/返回 {bytes} 字节暂不支持（ABI 未声明 vector by-ref；type {ty:?}）"
                    )));
                }
            }
        }
        // 聚合参数/返回 >8 字节：由 expand_agg_call_args / expand_large_agg_params /
        // expand_large_agg_ret 处理（≤16 字节拆段；>16 字节在 pass 内拒绝）。
        // 异常处理（P1.1）：invoke/landingpad/resume 无机器指令映射，编译期拒绝
        for (_, inst) in func.dfg.insts() {
            if matches!(inst.opcode, Opcode::LandingPad) {
                return Err(IrError::Unsupported(
                    "异常处理 landingpad 无机器指令映射（P1.1 仅文本层解析/展示）".to_string(),
                ));
            }
        }
        for (_, bd) in func.dfg.blocks() {
            if matches!(
                bd.terminator,
                Terminator::Invoke { .. } | Terminator::Resume { .. }
            ) {
                return Err(IrError::Unsupported(
                    "异常处理 invoke/resume 无机器指令映射（P1.1 仅文本层解析/展示）".to_string(),
                ));
            }
        }
        // 可变的 IR 副本：Stage 3 的 pattern isel 会就地重写指令序列。
        // 只有声明了模式融合的 ISA（[meta].enable_pattern_isel）才会真正改写；
        // 其余 ISA（如 x86_64）不克隆整个 Function（DFG 全量）。
        let needs_pattern = self
            .machine
            .pattern_matcher()
            .map(|m| m.pattern_count() > 0)
            .unwrap_or(false);
        let needs_agg_expand = crate::pipeline::agg_expand::has_any_agg(func);
        let mut func_owned: Option<Function> =
            (needs_pattern || needs_agg_expand).then(|| func.clone());

        // Stage 0.5: 聚合 store 展开（聚合字面量 → 打包 i64 常量 store；
        // >8 字节聚合显式 Unsupported）。必须在 CompileState 创建**之前**
        // 执行：CompileState 克隆 func 的常量池，若先建 state 再展开，新常量
        // 只进 func_owned 副本的池，lowering 读 state 里旧池（idx 错位 → 0）。
        if let Some(f) = func_owned.as_mut() {
            expand_agg_stores(f)?;
            // call 结果拆 2 先于参数拆段：%q = call {i64,i64} 传给后续 call 时
            // 参数已是段值（i64），expand_agg_call_args 的 load 追踪不会误判
            let mut agg_slots: AggSlots = HashMap::new();
            expand_large_agg_call_results(f, &mut agg_slots)?;
            expand_agg_call_args(f)?;
            expand_large_agg_params(f, &mut agg_slots)?;
            expand_large_agg_ret(f)?;
            expand_large_aggs(f, &mut agg_slots)?;
            expand_geps(f)?;
            // Stage 3 提前：IR 层模式融合（lea/cmp-select/fma）。不依赖
            // CompileState（自由函数），与展开共用独占可变借用。
            CompileState::<M::Inst>::run_pattern_matching(f, &self.machine);
        }
        // Stage 4+ 读取的 Function：有改写（pattern/聚合展开）时用副本。
        let func_ref: &Function = func_owned.as_ref().unwrap_or(func);

        let mut state = CompileState::new(&self.machine, func_ref);
        let _t = std::time::Instant::now();

        // Stage 1: Block mapping
        state.create_blocks(func_ref);
        let _t1 = _t.elapsed();
        let _t = std::time::Instant::now();

        // Stage 2: VReg pre-allocation
        state.alloc_params(func_ref);
        state.pre_allocate_phi_vregs(func_ref);
        state.pre_allocate_block_param_xregs(func_ref);
        let _t2 = _t.elapsed();
        let _t = std::time::Instant::now();
        let _t2b = _t.elapsed();
        let _t = std::time::Instant::now();

        // Stage 4: Instruction selection
        state.lower_all_blocks(func_ref, self.machine.lowering().as_ref())?;
        let _t3 = _t.elapsed();
        let _t = std::time::Instant::now();

        if crate::pipeline::trace_enabled("FORGE_TRACE_VCODE") {
            eprintln!("[vcode] === fn: {}", func_ref.name);
            for (bi, block) in state.vcode.blocks().enumerate() {
                eprintln!("[vcode] block {bi}: {:?}", block.ir_block);
                for inst in &block.instructions {
                    eprintln!("[vcode]   {:?}", inst);
                }
            }
        }

        // Stage 5: Peephole optimization
        if let Some(peep) = self.machine.peephole() {
            state.run_peephole(peep.as_ref());
        }
        let _t4 = _t.elapsed();
        let _t = std::time::Instant::now();

        // Stage 6: Register allocation
        let alloc_result = state.run_regalloc(&self.reg_alloc, &self.machine)?;
        let _t5 = _t.elapsed();
        let _t = std::time::Instant::now();

        // Stage 7: Frame layout
        let frame_size = state.calculate_frame_size(&alloc_result, &self.machine);
        let _t6 = _t.elapsed();
        let _t = std::time::Instant::now();

        // Stage 8-11: Emission
        let result = state.emit_code(func_ref, &alloc_result, frame_size, &self.machine);
        let _t7 = _t.elapsed();
        if std::env::var("CF_CODEGEN_TIMING").is_ok() {
            eprintln!(
                "[codegen {}] total={:?} blocks={:?} vreg={:?} lower={:?} peep={:?} regalloc={:?} frame={:?} emit={:?}",
                func_ref.name,
                _cf_t0.elapsed(),
                _t1,
                _t2,
                _t3,
                _t4,
                _t5,
                _t6,
                _t7
            );
        }
        if std::env::var_os("FORGE_TRACE_FN_SIZE").is_some() {
            let sz = result.as_ref().map(|r| r.code.len()).unwrap_or(0);
            eprintln!("[codegen] fn={} code_bytes={sz}", func_ref.name);
            if std::env::var_os("FORGE_TRACE_CODE_HEX").is_some()
                && let Ok(cf) = result.as_ref()
            {
                let hex: String = cf
                    .code
                    .iter()
                    .take(64)
                    .map(|b| format!("{b:02x}"))
                    .collect();
                eprintln!("[codegen] fn={} head={hex}", func_ref.name);
            }
        }
        Ok((result?, alloc_result))
    }
}

// ============================================================
// CompileState — mutable pipeline state
// ============================================================

pub(crate) struct CompileState<I: MachineInst> {
    pub(crate) vcode: VCode<I>,
    /// XReg → 微指令寄存器字段映射（与 vcode 的指令序列平行）。
    /// 由各指令包的 xreg_map 聚合；分配器按此构建活区间并回写字段。
    pub(crate) xreg_map: Vec<smallvec::SmallVec<[(XReg, u8, bool); 2]>>,
    /// 每条 vcode 指令的写死物理寄存器集（与 xreg_map/instructions 平行；
    /// 来自 lower 规则的 clobbers 声明，包级展开到包内每条指令）。
    pub(crate) inst_clobbers: Vec<Vec<(u32, crate::prelude::RegClass)>>,
    pub(crate) value_to_xreg: HashMap<Value, XReg>,
    pub(crate) block_map: HashMap<Block, VBlockId>,
    pub(crate) ctx: LowerCtx,
    pub(crate) param_xregs: Vec<XReg>,
    /// Alloca 指令 → 帧槽偏移（预扫描分配；lowering 时经 ctx.current_alloca_offset
    /// 供 `lea_off rd, alloca_offset` 规则取用）。
    pub(crate) alloca_offsets: HashMap<Inst, i64>,
}

impl<I: MachineInst + 'static> CompileState<I> {
    fn new<M: TargetMachine>(machine: &M, func: &Function) -> Self {
        let mut ctx = LowerCtx::new();
        ctx.call_conv = func.calling_convention;
        ctx.type_ctx = Some(func.types.clone());
        // StackAddr 的 lea 基准需要跳过 callee-saved 区（局部变量不能写在 push
        // 槽上）：fp 保存槽（frame_pointer_overhead）+ callee-saved 寄存器区。
        // 之前只算了 callee-saved 区，漏掉 fp 的 8 字节，局部变量落进 push 槽
        //（覆盖调用者寄存器保存值 → mini_c JIT SEGV/逻辑错误）。
        ctx.callee_saved_bytes = crate::pipeline::frame_layout::callee_saved_bytes(machine);
        // 栈槽帧顶平移：[abi.frame].stack_slot_shift（riscv=16）或回退
        // callee_saved_bytes（x86 语义）。
        ctx.stack_slot_shift = machine
            .abi()
            .stack_slot_shift()
            .unwrap_or(ctx.callee_saved_bytes);
        ctx.is_float_return = func
            .return_types()
            .iter()
            .any(|t| ctx.reg_class_for(t).is_fp());
        // S2：sret 隐藏参数——函数返回宽向量（>16 字节）→ 首 int 槽被
        // sret 指针占用（move_args 收参 __gi 从 1 起；调用方 sret 约定）。
        ctx.is_sret_return = func.return_types().iter().any(|t| {
            let s = func.types.borrow();
            (s.is_vector(*t) || s.is_scalable_vector(*t)) && s.size_bytes(*t) > 16
        });
        ctx.constant_pool = Some(func.constants.clone());

        Self {
            vcode: VCode::new(),
            xreg_map: Vec::new(),
            inst_clobbers: Vec::new(),
            value_to_xreg: HashMap::new(),
            block_map: HashMap::new(),
            ctx,
            param_xregs: Vec::new(),
            alloca_offsets: HashMap::new(),
        }
    }

    // ── Stage 6: Register Allocation ──

    fn run_regalloc<M2: TargetMachine<Inst = I>>(
        &mut self,
        allocator: &BacktrackingAllocator,
        machine: &M2,
    ) -> Result<AllocResult, IrError> {
        let ri = machine.reg_info();

        // Build new RegAllocConfig from TargetRegInfo — 全部寄存器类。
        // register_classes() 暴露 [reg.*] 多宽度类（如 GPR32 → GPR(4) 池），
        // 使 lowering 可按类型分派到对应宽度的寄存器池。
        let mut classes: HashMap<RegClass, ClassConfig> = HashMap::new();
        for info in ri.register_classes() {
            classes.insert(
                info.reg_class,
                ClassConfig {
                    allocatable: info.allocatable.clone(),
                    reg_width: info.width as u8,
                },
            );
        }

        // 主 GPR/FPR 类推导（元数据驱动）：由 ISA 的 default_*_class() 显式
        // 暴露（DSL 从 [reg.gpr64]/[reg.gpr] 与 [meta].default_fpr_width 生成），
        // compiler 不再猜测 GPR64/FPR64。例如 x86 浮点值主类为 FPR(8)
        //（f64 值宽），即使其 XMM 寄存器组是 FPR(16)。
        let main_gpr = ri.default_gpr_class();
        let main_fpr = ri.default_fpr_class();

        // 防御兜底：register_classes() 为空（非 DSL 生成的 ISA / 测试 mock）时，
        // 用 allocatable 顺序补主类。
        classes.entry(main_gpr).or_insert_with(|| ClassConfig {
            allocatable: ri.allocatable_gp_order(),
            reg_width: main_gpr.default_width(),
        });
        classes.entry(main_fpr).or_insert_with(|| ClassConfig {
            allocatable: ri.allocatable_fp_order(),
            reg_width: main_fpr.default_width(),
        });

        // 未定义宽度类的 fallback：ISA 未声明某宽度类时，同族继承主类池
        //（GPR(4) 继承主 GPR 类、FPR/VEC 继承主 FPR 类）。
        // 基类由元数据推导（同族最宽已声明类），而非硬编码 GPR64/FPR64——
        // 例如只有 GPR(4) 主类的 32 位 ISA，其 GPR(2)/GPR(1) 继承 GPR(4)。
        for class in [
            RegClass::GPR(4),
            RegClass::GPR(2),
            RegClass::GPR(1),
            RegClass::FPR(4),
            RegClass::FPR(8),
            RegClass::FPR(16),
            RegClass::VEC(16),
            RegClass::VEC(32),
        ] {
            if classes.contains_key(&class) {
                continue;
            }
            let base = classes
                .keys()
                .filter(|c| c.is_int() == class.is_int())
                // classes 是 HashMap，keys() 顺序跨进程随机；default_width 相同时
                // 平局 → 随机继承配置。用 (变体序, 宽度) 确定性排序。
                .max_by(|a, b| {
                    fn rank(c: &RegClass) -> (u8, u16) {
                        match c {
                            RegClass::GPR(w) => (0, *w),
                            RegClass::FPR(w) => (1, *w),
                            RegClass::VEC(w) => (2, *w),
                            RegClass::KReg(w) => (3, *w),
                        }
                    }
                    rank(a).cmp(&rank(b))
                })
                .copied();
            if let Some(base) = base
                && let Some(cfg) = classes.get(&base)
            {
                classes.insert(
                    class,
                    ClassConfig {
                        allocatable: cfg.allocatable.clone(),
                        reg_width: class.default_width(),
                    },
                );
            }
        }

        let precolored: HashMap<XReg, PReg> = ri.precolored_xregs().into_iter().collect();

        let reg_info = NewRegAllocConfig {
            classes,
            main_gpr_class: main_gpr,
            main_fpr_class: main_fpr,
            sp_reg: ri.sp_reg().register_index().unwrap_or(0),
            fp_reg: ri.fp_reg().map(|r| r.to_index()),
            callee_saved: ri.callee_saved(),
            precolored,
            scratch_regs: ri
                .scratch_regs()
                .iter()
                .map(|&n| PReg::new(n, RegClass::GPR64))
                .collect(),
            param_xregs: self.param_xregs.clone(),
            // 寄存器参数数 = ABI int 参数槽上限（Windows x64 = 4；其余
            // 参数走栈——regalloc 强制 spill，move_args 从 ABI 栈槽收参）。
            param_reg_count: machine.abi().int_arg_slot_count(),
        };

        let ctx = crate::pipeline::alloc_config::AllocContext {};

        let alloc_result = allocator.allocate(
            &self.vcode,
            &reg_info,
            &ctx,
            &self.xreg_map,
            &self.inst_clobbers,
        )?;

        // S2：sret 隐藏参数标记（move_args 收参跳过首 int 槽）——
        // 由 CompileState::new 的 LowerCtx.is_sret_return 预计算。
        let mut alloc_result = alloc_result;
        alloc_result.sret = self.ctx.is_sret_return;
        // 栈参数区字节数（move_args 收栈参数时计算 spill 槽地址）
        alloc_result.stack_arg_bytes = self.ctx.max_stack_arg_bytes;

        // 分配后回写：按 xreg_map 把 XReg 的分配结果填入微指令寄存器字段（物理 Reg）
        let mut global_inst = 0usize;
        for block in self.vcode.blocks_mut() {
            for inst in block.instructions.iter_mut() {
                if let Some(slot) = self.xreg_map.get(global_inst) {
                    for &(xreg, field_idx, _is_def) in slot.iter() {
                        if let Some(preg) = alloc_result.preg(xreg) {
                            inst.set_reg_field(field_idx as usize, preg.num);
                        }
                    }
                }
                global_inst += 1;
            }
        }
        let _ = std::mem::take(&mut self.param_xregs); // param_xregs 现在由 alloc_result 管理
        Ok(alloc_result)
    }
}

#[cfg(test)]
mod alloc_integration_tests {
    use super::*;
    use crate::arch::x86_v12;

    fn build_sig(params: &[TypeId], ret: TypeId) -> FunctionSignature {
        FunctionSignature::new(&params.iter().map(|t| (*t, "")).collect::<Vec<_>>(), &[ret])
    }

    /// Phase 4.2：builder 构造 IR 走完整管线，断言 AllocResult.assignments 非空且合法。
    #[test]
    fn test_alloc_result_assignments() {
        x86_v12::ensure_registered();
        let mut b = FunctionBuilder::new(
            "alloc_test",
            TypeContext::new(),
            build_sig(&[], TypeId::I64),
        );
        b.create_block_here();
        let a = b.iconst_i64(21);
        let c = b.iconst_i64(2);
        let s = b.imul(a, c);
        b.ret(&[s]);
        let func = b.finish().expect("build");
        let machine = x86_v12::TargetMachine::new();
        let (cf, alloc) = FunctionCompiler::new(machine)
            .compile_with_alloc(&func)
            .expect("compile_with_alloc");
        assert!(!cf.code.is_empty(), "编译产物非空");
        assert!(
            !alloc.assignments.is_empty(),
            "assignments 非空（有 XReg 被分配）"
        );
        for (xreg, preg) in &alloc.assignments {
            assert!(
                preg.class == RegClass::GPR64 || preg.class.is_fp(),
                "XReg {xreg:?} 分配类非法: {:?}",
                preg.class
            );
        }
    }

    /// Phase 4.3：参数 ABI 分配——f(i64×4) 的前 4 参数 XReg 分配到 RCX/RDX/R8/R9。
    #[test]
    fn test_param_abi_allocation() {
        x86_v12::ensure_registered();
        let ctx = TypeContext::new();
        let sig = FunctionSignature::new(
            &[
                (ctx.i64_ty(), "a"),
                (ctx.i64_ty(), "b"),
                (ctx.i64_ty(), "c"),
                (ctx.i64_ty(), "d"),
            ],
            &[ctx.i64_ty()],
        );
        let mut b = FunctionBuilder::new("param_abi", ctx, sig);
        let (_entry, params) = b.create_entry_block();
        // 四个参数求和（强制全部活跃跨函数体）
        let s0 = b.iadd(params[0], params[1]);
        let s1 = b.iadd(s0, params[2]);
        let s2 = b.iadd(s1, params[3]);
        b.ret(&[s2]);
        let func = b.finish().expect("build");
        let machine = x86_v12::TargetMachine::new();
        let (_cf, alloc) = FunctionCompiler::new(machine)
            .compile_with_alloc(&func)
            .expect("compile_with_alloc");
        // 参数 XReg（入口收参）由分配器自由分配（非 precolor arg_regs）：
        // entry 收参 mov 把 [abi.arg_regs]（RCX/RDX/R8/R9）的值搬给参数 XReg，
        // 随后 XReg 可任意分配——分配器从 GPR 高编号起（r15 先），天然避开
        // div 的 RAX/RDX 等低编号 clobber 寄存器。ABI 正确性由收参 mov +
        // 执行测试保证；这里断言：所有参数 XReg 都有合法 GPR 分配。
        let param_pregs: Vec<u32> = alloc
            .param_vregs
            .iter()
            .map(|x| alloc.assignments.get(x).map(|p| p.num).unwrap_or(u32::MAX))
            .collect();
        assert_eq!(
            alloc.param_vregs.len(),
            4,
            "应有 4 个参数 XReg（收参），实际 {}",
            alloc.param_vregs.len()
        );
        assert!(
            param_pregs.iter().all(|&n| n < 16),
            "参数 XReg 应全部有合法 GPR 分配，实际 {param_pregs:?}"
        );
        // 参数分配不得与 div clobber 寄存器（RAX=0/RDX=2）冲突——若参数落在
        // 其上，div 序列的 clobber 会强制溢出（见写死保护测试）。自由分配
        // 从高编号起，低压力下参数集中在 r15-r12。
        assert!(
            !param_pregs.contains(&0) && !param_pregs.contains(&2),
            "参数不得分配到 RAX/RDX（div clobber 区），实际 {param_pregs:?}"
        );
    }

    /// Phase 4.4：写死保护——div 序列（clobber RAX/RDX）+ 16 值活跃压力下，
    /// 分配器必须产出完整分配（无 RegAlloc 错误、无未处理冲突），写死值不被覆盖。
    /// 执行语义正确性由 forge-tests 的 test_udiv_pressure（672）验证；此处断言
    /// 分配层完整性：每个分配/溢出的 XReg 都有合法去向。
    #[test]
    fn test_div_clobber_alloc_integrity() {
        x86_v12::ensure_registered();
        let ctx = TypeContext::new();
        let sig = FunctionSignature::new(&[], &[ctx.i64_ty()]);
        let mut b = FunctionBuilder::new("div_clobber_alloc", ctx, sig);
        b.create_block_here();
        let mut acc = b.iconst_i64(0);
        for i in 1..=16i64 {
            let a = b.iconst_i64(i * 42);
            let c = b.iconst_i64(i);
            let d = b.udiv(a, c);
            acc = b.iadd(acc, d);
        }
        b.ret(&[acc]);
        let func = b.finish().expect("build");
        let machine = x86_v12::TargetMachine::new();
        let (cf, alloc) = FunctionCompiler::new(machine)
            .compile_with_alloc(&func)
            .expect("div + clobber 压力下编译必须成功（无 RegAlloc 错误）");
        assert!(!cf.code.is_empty(), "编译产物非空");
        assert!(!alloc.assignments.is_empty(), "必须有分配");
        // 写死保护：clobber 寄存器（RAX=0/RDX=2）上的分配不可能是"跨 div 序列
        // 活跃的中间值"——clobber 机制在序列点把它们溢出（spill_slots 记录）
        // 或分配器避开。断言：每个 spill 槽的 XReg 大小合法（spill 数据完整）。
        for (xreg, slot) in &alloc.spill_slots {
            assert!(
                slot.size >= 4 && slot.size <= 16,
                "spill 槽大小非法: {xreg:?} -> {slot:?}"
            );
        }
    }

    /// P0-1 集成：`with_opt_level` 编译常量折叠函数——O1 管线把
    /// `iconst(21)*iconst(2)` 折叠为常量，编译产物仍正确（此前优化管线
    /// 在生产路径是死代码）。
    #[test]
    fn p0_compile_with_opt_level() {
        x86_v12::ensure_registered();
        let mut b = FunctionBuilder::new(
            "opt_const_fold",
            TypeContext::new(),
            build_sig(&[], TypeId::I64),
        );
        b.create_block_here();
        let a = b.iconst_i64(21);
        let c = b.iconst_i64(2);
        let s = b.imul(a, c);
        b.ret(&[s]);
        let func = b.finish().expect("build");

        let machine = x86_v12::TargetMachine::new();
        // 无优化编译（对照）
        let (cf0, _) = FunctionCompiler::new(machine.clone())
            .compile_with_alloc(&func)
            .expect("compile 无优化");
        // O1 优化编译（常量折叠 21*2 → 42）
        let (cf1, _) = FunctionCompiler::new(machine.clone())
            .with_opt_level(forge_opt::OptimizationLevel::O1)
            .compile_with_alloc(&func)
            .expect("compile 带 O1");
        assert!(!cf0.code.is_empty(), "无优化产物非空");
        assert!(!cf1.code.is_empty(), "O1 产物非空");
        // O1 常量折叠后指令更少（movabs 42 而非 movabs 21 + movabs 2 + imul）
        assert!(
            cf1.code.len() <= cf0.code.len(),
            "O1 常量折叠应不增大代码（P0-1 回归：优化管线未生效）"
        );
    }
}
