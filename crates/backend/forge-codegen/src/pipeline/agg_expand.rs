//! 聚合 ABI 展开(Stage 0.5)——从 compiler.rs 分批迁出(第四十一轮)。
//!
//! 聚合参数/返回/存储/call 结果 >8 字节的 IR 层展开:
//! - 段值拆分(≤16 字节:寄存器路径;>16 字节 Unsupported)
//! - 展开触发检测(has_* 族)与展开执行(expand_* 族)分居此处
//!
//! 依赖约定:仅依赖 forge_ir 与 crate::pipeline::agg_const;
//! 不依赖 CompileState(展开在 CompileState::new 之前执行——常量池克隆时序)。

use forge_ir::entity::map::SecondaryMap;
use forge_ir::*;

/// 函数是否含 GEP(决定是否需要可变副本做 GEP 展开)。
pub(crate) fn has_gep(func: &Function) -> bool {
    for (_, bd) in func.dfg.blocks() {
        for &ii in &bd.inst_order {
            if func.dfg.inst_data(ii).opcode == Opcode::GetElementPtr {
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
            let inst = &func.dfg.inst_data(ii);
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
            let inst = &func.dfg.inst_data(ii);
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
            let inst = &func.dfg.inst_data(ii);
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
            let inst = &func.dfg.inst_data(ii);
            match inst.opcode {
                Opcode::Store => {
                    if let Some(v) = inst.operands.first()
                        && matches!(func.dfg.value_data(*v).def, ValueDef::AggConst(_))
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
                        .any(|v| matches!(func.dfg.value_data(*v).def, ValueDef::AggConst(_)))
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
pub(crate) type AggSlots = SecondaryMap<Value, (Value, TypeId)>;

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

// rewrite 族（第四十四轮记录的"第三批"）已于本轮迁入：memoryize_from_segs /
// memoryize_from_addr / rewrite_agg_value_uses / move_to_front。仍留在 compiler.rs 的
// 是 expand_* 执行族（expand_large_agg_params / _ret / _call_results / expand_agg_call_args /
// collect_extract_jobs / expand_large_aggs / expand_geps / expand_agg_stores）——
// 它们与 CompileState 的时序耦合更紧，按批再迁。

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
pub(crate) fn memoryize_from_segs(
    func: &mut Function,
    use_pos: &UsePos,
    seg_ty: TypeId,
    off: usize,
    segs: &[Value],
    agg_slots: &mut AggSlots,
) -> Result<(), IrError> {
    // 取一次读锁（v3 S3 读路径纪律）：本函数只读类型，内部不再逐次取锁。
    // 先从 func.types clone 出 TypeContext（Arc，O(1)）再取锁——守卫借的是这个局部，
    // 不与 func 的 &mut 借用冲突。
    let types_ctx = func.types.clone();
    let store = types_ctx.borrow();
    if !off.is_multiple_of(8) {
        return Err(IrError::Unsupported(format!(
            "嵌套聚合字段提取：非对齐字段（off={off}）暂不支持"
        )));
    }
    let size = store.size_bytes(seg_ty) as usize;
    let n_segs = size.div_ceil(8);
    let start = off / 8;
    if start + n_segs > segs.len() {
        return Err(IrError::Unsupported("嵌套聚合字段提取：段值不足".into()));
    }
    // 1) 分配帧槽（Alloca 指令——预扫描分配帧偏移，lowering 经 lea_off 取址）
    let cid = func.constants.insert_int(size as i128, 64);
    let alloca = func.make_inst(
        Opcode::Alloca,
        use_pos.block,
        smallvec::smallvec![],
        smallvec::smallvec![Immediate::Const(cid)],
        &[TypeId::PTR],
        InstFlags::NONE,
    );
    let slot = func.dfg.inst_data(alloca).results[0];
    // 2) 字段覆盖的段逐段 store（仿 Store 分支的 store + add ptr, 8 模式）
    let mut cur = slot;
    for (i, s) in segs.iter().skip(start).take(n_segs).enumerate() {
        func.make_inst(
            Opcode::Store,
            use_pos.block,
            smallvec::smallvec![*s, cur],
            smallvec::smallvec![],
            &[],
            InstFlags::SIDE_EFFECT,
        );
        if i + 1 < n_segs {
            // 先把常量塞进池（`func.make_inst` 会借整个 func，参数里不能再借 constants）
            let c8 = func.constants.insert_int(8, 64);
            let ci = func.make_inst(
                Opcode::Iconst,
                use_pos.block,
                smallvec::smallvec![],
                smallvec::smallvec![Immediate::Const(c8)],
                &[TypeId::I64],
                InstFlags::NONE,
            );
            let cv = func.dfg.inst_data(ci).results[0];
            let add = func.make_inst(
                Opcode::Iadd,
                use_pos.block,
                smallvec::smallvec![cur, cv],
                smallvec::smallvec![],
                &[TypeId::PTR],
                InstFlags::NONE,
            );
            cur = func.dfg.inst_data(add).results[0];
        }
    }
    // 3) 原指令 Nop + 登记（槽填充 store 只执行一次——后续提取命中映射）
    let __tomb = use_pos.inst;
    func.tombstone_inst(__tomb);
    agg_slots.insert(func.dfg.inst_data(__tomb).results[0], (slot, seg_ty));
    Ok(())
}

/// S1：聚合字段提取——从地址链继续（无物理拷贝）。
/// %p = add ptr addr, foff；结果 value 登记 (p, 字段类型)；原指令改 Nop。
/// 后续标量字段提取从 p 直接 load；聚合字段提取继续登记子地址。
pub(crate) fn memoryize_from_addr(
    func: &mut Function,
    use_pos: &UsePos,
    base: Value,
    foff: usize,
    seg_ty: TypeId,
    agg_slots: &mut AggSlots,
) -> Result<(), IrError> {
    let cid = func.constants.insert_int(foff as i128, 64);
    let ci = func.make_inst(
        Opcode::Iconst,
        use_pos.block,
        smallvec::smallvec![],
        smallvec::smallvec![Immediate::Const(cid)],
        &[TypeId::I64],
        InstFlags::NONE,
    );
    let cv = func.dfg.inst_data(ci).results[0];
    let add = func.make_inst(
        Opcode::Iadd,
        use_pos.block,
        smallvec::smallvec![base, cv],
        smallvec::smallvec![],
        &[TypeId::PTR],
        InstFlags::NONE,
    );
    let p = func.dfg.inst_data(add).results[0];
    let __tomb = use_pos.inst;
    func.tombstone_inst(__tomb);
    agg_slots.insert(func.dfg.inst_data(__tomb).results[0], (p, seg_ty));
    Ok(())
}

/// extractvalue 使用点（块 + 指令序位 + 指令句柄）
pub(crate) fn rewrite_agg_value_uses(
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
            let inst = &func.dfg.inst_data(ii);
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
        let inst = &func.dfg.inst_data(use_pos.inst);
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
                if let Some(&(addr, slot_ty)) = agg_slots.get(inst.operands[0]) {
                    let ts = func.types.borrow();
                    let foff = ts.field_offset(slot_ty, idx).ok_or_else(|| {
                        IrError::Unsupported(format!("extractvalue 字段偏移缺失：{idx}"))
                    })? as usize;
                    let fty = ts.aggregate_elem_type(slot_ty, idx).ok_or_else(|| {
                        IrError::Unsupported(format!("extractvalue 字段类型缺失：{idx}"))
                    })?;
                    if ts.is_aggregate(fty) {
                        drop(ts);
                        let res = func.dfg.inst_data(use_pos.inst).results[0];
                        memoryize_from_addr(func, &use_pos, addr, foff, fty, agg_slots)?;
                        // 递归：登记的新值（内层聚合）的后续提取继续处理
                        return rewrite_agg_value_uses(func, res, &[], fty, agg_slots);
                    }
                    drop(ts);
                    // 标量字段：%p = add ptr addr, foff; %v = load fty, ptr %p
                    let cid = func.constants.insert_int(foff as i128, 64);
                    let ci = func.make_inst(
                        Opcode::Iconst,
                        use_pos.block,
                        smallvec::smallvec![],
                        smallvec::smallvec![Immediate::Const(cid)],
                        &[TypeId::I64],
                        InstFlags::NONE,
                    );
                    let cv = func.dfg.inst_data(ci).results[0];
                    let add = func.make_inst(
                        Opcode::Iadd,
                        use_pos.block,
                        smallvec::smallvec![addr, cv],
                        smallvec::smallvec![],
                        &[TypeId::PTR],
                        InstFlags::NONE,
                    );
                    let p = func.dfg.inst_data(add).results[0];
                    let ld = func.make_inst(
                        Opcode::Load,
                        use_pos.block,
                        smallvec::smallvec![p],
                        smallvec::smallvec![],
                        &[fty],
                        InstFlags::NONE,
                    );
                    let lv = func.dfg.inst_data(ld).results[0];
                    // 结果 value 重定向：原指令改 Copy（值传播到 load 结果）
                    let __cp = use_pos.inst;
                    {
                        let inst = func.dfg.inst_mut(__cp);
                        inst.opcode = Opcode::Copy;
                        inst.operands = smallvec::smallvec![lv];
                        inst.immediates = smallvec::smallvec![];
                    }
                    func.refresh_inst_uses(__cp);
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
                    let res = func.dfg.inst_data(use_pos.inst).results[0];
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
                    let ci = func.make_inst(
                        Opcode::Iconst,
                        use_pos.block,
                        smallvec::smallvec![],
                        smallvec::smallvec![Immediate::Const(cid)],
                        &[TypeId::I64],
                        InstFlags::NONE,
                    );
                    let cv = func.dfg.inst_data(ci).results[0];
                    local_gen.push(ci);
                    let sh = func.make_inst(
                        Opcode::Ushr,
                        use_pos.block,
                        smallvec::smallvec![base, cv],
                        smallvec::smallvec![],
                        &[TypeId::I64],
                        InstFlags::NONE,
                    );
                    final_v = func.dfg.inst_data(sh).results[0];
                    local_gen.push(sh);
                }
                // 浮点字段：bitcast（段值 i64 → 字段类型——movq_to_xmm 位模式
                // 转换）；整数字段：Copy 截断
                let mut target = final_v;
                if func.types.borrow().is_float(seg_ty) {
                    let bc = func.make_inst(
                        Opcode::Bitcast,
                        use_pos.block,
                        smallvec::smallvec![final_v],
                        smallvec::smallvec![],
                        &[seg_ty],
                        InstFlags::NONE,
                    );
                    target = func.dfg.inst_data(bc).results[0];
                    local_gen.push(bc);
                }
                let __cp = use_pos.inst;
                {
                    let inst = func.dfg.inst_mut(__cp);
                    inst.opcode = Opcode::Copy;
                    inst.operands = smallvec::smallvec![target];
                    inst.immediates = smallvec::smallvec![];
                }
                func.refresh_inst_uses(__cp);
                move_to_front(func, use_pos.block, use_pos.pos, &local_gen);
            }
            Opcode::Store => {
                let addr = inst.operands[1];
                let mut prev_addr = addr;
                let mut local_gen: Vec<Inst> = Vec::new();
                for (k, s) in segs.iter().enumerate() {
                    let st = func.make_inst(
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
                        let ci = func.make_inst(
                            Opcode::Iconst,
                            use_pos.block,
                            smallvec::smallvec![],
                            smallvec::smallvec![Immediate::Const(cid)],
                            &[TypeId::I64],
                            InstFlags::NONE,
                        );
                        let cv = func.dfg.inst_data(ci).results[0];
                        local_gen.push(ci);
                        let add = func.make_inst(
                            Opcode::Iadd,
                            use_pos.block,
                            smallvec::smallvec![prev_addr, cv],
                            smallvec::smallvec![],
                            &[TypeId::PTR],
                            InstFlags::NONE,
                        );
                        prev_addr = func.dfg.inst_data(add).results[0];
                        local_gen.push(add);
                    }
                }
                // 原 store 标 Nop
                let __tomb = use_pos.inst;
                func.tombstone_inst(__tomb);
                move_to_front(func, use_pos.block, use_pos.pos, &local_gen);
            }
            Opcode::Call | Opcode::CallIndirect => {
                let op_idx = inst
                    .operands
                    .iter()
                    .position(|&v| v == old_v)
                    .ok_or_else(|| IrError::Unsupported("call 参数重写：未找到".into()))?;
                let mut ops = func.dfg.inst_data(use_pos.inst).operands.clone();
                ops.remove(op_idx);
                for (k, s) in segs.iter().enumerate() {
                    ops.insert(op_idx + k, *s);
                }
                func.dfg.inst_mut(use_pos.inst).operands = ops;
                // 就地改写操作数后必须重登记 use-lists（否则留下指向旧操作数的陈旧 use）
                func.refresh_inst_uses(use_pos.inst);
            }
            _ => {
                return Err(IrError::Unsupported(format!(
                    "大聚合值用于 {:?}（仅支持 extractvalue/store/return/call 传参）",
                    inst.opcode
                )));
            }
        }
    }
    // Return values 重写（先收集再写，避免借用冲突；投影读取）
    let mut ret_updates: Vec<(Block, Vec<Value>)> = Vec::new();
    for (b, _bd) in func.dfg.blocks() {
        let Some(values) = func.dfg.term_return_values(b) else {
            continue;
        };
        if values.contains(&old_v) {
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
        // 经按形式写入口：改完终结符用值后同步 use-def
        func.set_return_values(b, &new_vals);
    }
    Ok(())
}

/// 把块尾的生成指令（make_inst push 到末尾）移动到指定位置前。
pub(crate) fn move_to_front(func: &mut Function, block: Block, pos: usize, insts: &[Inst]) {
    let mut moved: Vec<Inst> = Vec::with_capacity(insts.len());
    for _ in 0..insts.len() {
        let order = &mut func.dfg.block_mut(block).inst_order;
        moved.push(order.pop().unwrap());
    }
    moved.reverse();
    for (k, &ni) in moved.iter().enumerate() {
        func.dfg.block_mut(block).inst_order.insert(pos + k, ni);
    }
}
