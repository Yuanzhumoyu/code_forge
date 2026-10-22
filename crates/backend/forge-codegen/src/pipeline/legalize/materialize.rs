//! 合法化**物化相**（v24 M1 Phase B）：**按指令序单遍**改写，取代散在多个 pass 里的
//! 旧的就地改写函数与"生成后搬运"那套写法。
//!
//! ## 为什么要单遍 + 自己构造块内指令序
//!
//! 旧机制的形状是"生成指令（`make_inst` 追加到**块尾**）→ 再把它们搬到使用点之前"
//! （6 个调用点各搬一次）。v23 收尾的真凶就是这么来的：by-ref 分支漏搬一次
//! ⇒ 生成的 `Load` 排在自己消费者（`Copy`）**之后** ⇒ **use before def** ⇒ 分配器读到陈旧
//! 寄存器（症状：字段各自读对、一起用就错、每次运行值都不同）。
//!
//! 本相把这件事变成**构造式**：逐块读出指令序快照，为每条指令算出要插入的生成指令，
//! 拼成新的 `inst_order` **一次写回**。生成指令**天生排在它的消费者之前**——
//! 不需要"搬"，也就没有"漏搬"。
//!
//! 生成指令的**捕获方式**：`make_inst` 一律追加到块尾，所以比较处理前后块序的长度即可捕获
//! "这条指令生成的"那些（这也让复用既有助手——例如 `memoryize_from_segs`——不必改签名）。
//!
//! ## 归属（搬迁期）
//!
//! 本相目前接管 `extractvalue` 与 `store`（内存档与段值档都管）；`call`/`ret`/`insertvalue`
//! 与 `extractvalue`/`store`/`call` 一样由本相接管（旧函数已随 v24 M1 退场）。
//!
//! 判据见文件末尾两条守卫：生成指令**先于**其消费者、以及本相**幂等**。

use crate::IrError;
use crate::pipeline::agg_expand::{ChangeSet, IrSize, Place, Places, UsePos, memoryize_from_segs};
use forge_ir::entity::map::SecondaryMap;
use forge_ir::*;

/// **物化相主入口**：把 `Places` 里登记过的聚合值，其 `extractvalue` / `store` 使用
/// 单遍展开成显式 IR。
///
/// 返回本相改动的规模（`ChangeSet`）；同一函数跑第二遍应为零（幂等）。
pub(crate) fn materialize_memory_uses(
    func: &mut Function,
    places: &mut Places,
    shadow_agg: &SecondaryMap<Value, TypeId>,
) -> Result<ChangeSet, IrError> {
    let before = IrSize::of(func);
    // 读路径纪律：整个 pass 只取**一次**读锁（`types` 是 Arc 克隆，不与 `func` 的可变借用冲突）。
    let types = func.types.clone();
    let store = types.borrow();
    let blocks: Vec<Block> = func.dfg.blocks().map(|(b, _)| b).collect();
    for b in blocks {
        let order: Vec<Inst> = func.dfg.block(b).inst_order.clone();
        let mut new_order: Vec<Inst> = Vec::with_capacity(order.len());
        for ii in order {
            let tail_before = func.dfg.block(b).inst_order.len();
            handle_inst(func, &store, places, shadow_agg, b, ii)?;
            let after = func.dfg.block(b).inst_order.clone();
            // **生成指令天生排在消费者之前**（构造式，而不是"事后搬"）。
            new_order.extend_from_slice(&after[tail_before..]);
            new_order.push(ii);
        }
        func.dfg.block_mut(b).inst_order = new_order;
    }
    // **终结符**（`ret`）：段值档的聚合返回值就地展开成多个返回值；内存档**不动**——
    // 它由 `expand_large_agg_ret` 按约定写进 sret 缓冲（或拆成多返回值）。
    for b in func.dfg.blocks().map(|(b, _)| b).collect::<Vec<_>>() {
        let Some(values) = func.dfg.term_return_values(b) else {
            continue;
        };
        if !values
            .iter()
            .any(|v| matches!(places.get(*v), Some(Place::Segs(_))))
        {
            continue;
        }
        let mut new_vals: Vec<Value> = Vec::with_capacity(values.len());
        for &v in values.iter() {
            match places.get(v) {
                Some(Place::Segs(segs)) => new_vals.extend(segs.iter().copied()),
                _ => new_vals.push(v),
            }
        }
        func.set_return_values(b, &new_vals);
    }
    Ok(IrSize::of(func).delta_to(before, false))
}

/// 单条指令的使用改写分派（目前只有 `extractvalue` / `store` 两类）。
fn handle_inst(
    func: &mut Function,
    store: &forge_isa_runtime::ir::ir::types::TypeStore,
    places: &mut Places,
    shadow_agg: &SecondaryMap<Value, TypeId>,
    block: Block,
    ii: Inst,
) -> Result<(), IrError> {
    match func.dfg.inst_data(ii).opcode {
        Opcode::ExtractValue => expand_extract(func, store, places, shadow_agg, block, ii),
        Opcode::Store => expand_store(func, store, places, shadow_agg, block, ii),
        Opcode::Call | Opcode::CallIndirect => expand_call(func, places, ii),
        Opcode::InsertValue => memoryize_insertvalue(func, store, places, shadow_agg, block, ii),
        _ => Ok(()),
    }
}

/// **沿定义链求一个聚合值的内存地址**（v24 M3 的共享助手）：拿到地址 ⇒ 它的字节就在那儿。
///
/// 三种情形：
///
/// * `places` 里已有地址（形参 / call 结果 / 已物化过的链）⇒ 直接返回；
/// * 定义是 `insertvalue` ⇒ **先物化它**（递归，因为它的基值也可能是链），再取地址；
/// * `undef` / `poison` / 其它未知来源 ⇒ `None`（由调用方决定"补 0"还是 fail-closed）。
///
/// 为什么要"沿链"而不是只看一步：`%t1 = insertvalue undef, 4, 0` → `%t2 = insertvalue %t1, 7, 1`
/// 这种构造里，**`%t2` 的基值 `%t1` 自己就是链**。只看一步会拿到 `None`、把 `%t1` 的字段
/// 按 0 处理——那是**静默错值**（v23 的教训），所以这里必须递归。
pub(crate) fn ensure_value_addr(
    func: &mut Function,
    store: &forge_isa_runtime::ir::ir::types::TypeStore,
    places: &mut Places,
    shadow_agg: &SecondaryMap<Value, TypeId>,
    block: Block,
    v: Value,
) -> Result<Option<Value>, IrError> {
    if let Some(a) = places.addr(v) {
        return Ok(Some(a));
    }
    let ValueDef::Inst(def, _) = func.dfg.value_data(v).def else {
        return Ok(None);
    };
    match func.dfg.inst_data(def).opcode {
        Opcode::InsertValue => {
            memoryize_insertvalue(func, store, places, shadow_agg, block, def)?;
            Ok(places.addr(v))
        }
        _ => Ok(None),
    }
}

/// 展开 `%r = insertvalue <聚合> %agg, %x, i`：把**来源聚合的字节**搬进一个**自己的**帧槽
/// （不能就地改来源——来源可能住在调用方内存里，改它就是改调用方的数据），再把插入的字段写进
/// 槽内对应偏移；结果值登记成"住在该槽"。后续使用由本相在这条指令**之后**自然处理，无需递归。
///
/// **v24 M3：公开为共享助手**——调用实参 pass 也要用同一条路径（`insertvalue` 链搭出来的聚合
/// 作为实参时，它需要先拿到槽地址才能逐段 load）。两处共用一份实现，才不会出现
/// "这边支持了、那边还 fail-closed"的分叉。
pub(crate) fn memoryize_insertvalue(
    func: &mut Function,
    store: &forge_isa_runtime::ir::ir::types::TypeStore,
    places: &mut Places,
    shadow_agg: &SecondaryMap<Value, TypeId>,
    block: Block,
    ii: Inst,
) -> Result<(), IrError> {
    let (agg_v, elem, res, idx) = {
        let inst = func.dfg.inst_data(ii);
        let (Some(&agg_v), Some(&elem), Some(&res)) = (
            inst.operands.first(),
            inst.operands.get(1),
            inst.results.first(),
        ) else {
            return Ok(());
        };
        let Some(Immediate::Uint(idx)) = inst.immediates.first() else {
            return Err(IrError::Internal("insertvalue 缺字段索引 immediate".into()));
        };
        (agg_v, elem, res, *idx as u32)
    };
    let Some(agg_ty) = agg_ty_of(func, store, shadow_agg, agg_v) else {
        return Ok(());
    };
    let size = store.size_bytes(agg_ty) as usize;
    let foff = store
        .field_offset(agg_ty, idx)
        .ok_or_else(|| IrError::Internal(format!("insertvalue 字段越界：{idx}")))?;
    let fty = store
        .aggregate_elem_type(agg_ty, idx)
        .ok_or_else(|| IrError::Internal(format!("insertvalue 字段类型缺失：{idx}")))?;
    let field_is_fp = store.is_float(fty);
    // **沿定义链求来源地址**（v24 M3）：基值可能是另一个 `insertvalue`（链式构造）或 `undef`
    //（全 0）。以前只看 `places` 里有没有现成的地址，于是链式场景里基值的字段会**静默丢掉**
    //（症状：v23 那条 `main_ins` 返回 `72198297058681447` 而非 11）。现在递归地把基值也
    // 内存化，拿到地址后再搬；`undef`/`poison` 与"没有来源"一样按**补 0** 处理。
    let src = ensure_value_addr(func, store, places, shadow_agg, block, agg_v)?.map(Place::Mem);
    // 1) 新槽 + 把来源字节搬进去（内存档逐段 load；段值档直接 store；无来源则**补 0**——
    //    保守且确定，不读未初始化内存）。
    let (_alloca, slot) = crate::pipeline::legalize::common::place_addr_of_slot(func, block, size);
    let n_segs = size.div_ceil(8);
    let mut cur = slot;
    for k in 0..n_segs {
        let seg = match &src {
            Some(Place::Mem(src_addr)) => {
                let p = if k == 0 {
                    *src_addr
                } else {
                    add_const(func, block, *src_addr, (k * 8) as u32)
                };
                let ld = func.make_inst(
                    Opcode::Load,
                    block,
                    smallvec::smallvec![p],
                    smallvec::smallvec![],
                    &[TypeId::I64],
                    InstFlags::NONE,
                );
                func.dfg.inst_data(ld).results[0]
            }
            Some(Place::Segs(segs)) => match segs.get(k) {
                Some(v) => *v,
                None => const_i64(func, block, 0),
            },
            _ => const_i64(func, block, 0),
        };
        func.make_inst(
            Opcode::Store,
            block,
            smallvec::smallvec![seg, cur],
            smallvec::smallvec![],
            &[],
            InstFlags::SIDE_EFFECT,
        );
        if k + 1 < n_segs {
            cur = add_const(func, block, cur, 8);
        }
    }
    // 2) 把插入的字段写进槽内对应偏移（浮点字段先位转换为 i64 再存）。
    let store_v = if field_is_fp {
        let bc = func.make_inst(
            Opcode::Bitcast,
            block,
            smallvec::smallvec![elem],
            smallvec::smallvec![],
            &[TypeId::I64],
            InstFlags::NONE,
        );
        func.dfg.inst_data(bc).results[0]
    } else {
        elem
    };
    let field_addr = field_addr_of(func, block, slot, foff)?;
    func.make_inst(
        Opcode::Store,
        block,
        smallvec::smallvec![store_v, field_addr],
        smallvec::smallvec![],
        &[],
        InstFlags::SIDE_EFFECT,
    );
    // 3) 结果值登记成"住在该槽"，原指令墓碑化。
    if !places.contains(res) {
        places.set(res, Place::Mem(slot));
    }
    func.tombstone_inst(ii);
    Ok(())
}

/// 展开 `call`/`call_indirect` 的**聚合实参**：`Mem(addr)` ⇒ 换成指针；
/// `Segs(segs)` ⇒ 就地展开成多个段实参（与旧就地改写函数的 Call 臂同形）。
///
/// 注：**by-ref 的"调用方副本 + `byval` 属性"**不在这里——那是 `expand_agg_call_args` 的职责
/// （它在聚合族里跑、早于本相，并且会自己把实参换成指向副本的指针）。
fn expand_call(func: &mut Function, places: &Places, ii: Inst) -> Result<(), IrError> {
    let (ops, skip) = {
        let inst = func.dfg.inst_data(ii);
        (
            inst.operands.clone(),
            usize::from(inst.opcode == Opcode::CallIndirect),
        )
    };
    let mut new_ops: smallvec::SmallVec<[Value; 4]> = smallvec::SmallVec::with_capacity(ops.len());
    let mut changed = false;
    for (k, &v) in ops.iter().enumerate() {
        if k < skip {
            new_ops.push(v);
            continue;
        }
        match places.get(v) {
            Some(Place::Mem(addr)) => {
                new_ops.push(*addr);
                changed = true;
            }
            Some(Place::Segs(segs)) => {
                new_ops.extend(segs.iter().copied());
                changed = true;
            }
            _ => new_ops.push(v),
        }
    }
    if changed {
        func.dfg.inst_mut(ii).operands = new_ops;
        // 就地改写操作数后必须重登记 use-lists（否则留下指向旧操作数的陈旧 use）。
        func.refresh_inst_uses(ii);
    }
    Ok(())
}

/// 展开一条 `%r = extractvalue <聚合> %agg, idx`，其中 `%agg` 在 `Places` 里有位置。
///
/// - **内存档**（`Mem(addr)`）：字段就在 `addr + foff`。标量字段 ⇒ `Iadd`+`Load`，原指令改
///   `Copy`；嵌套聚合字段 ⇒ **直接登记子地址**并墓碑化（**不拷贝**——旧实现在这里会新开一个槽
///   再把字节搬一遍，纯冗余）。
/// - **段值档**（`Segs(segs)`）：字段落在第 `off/8` 段 + `off%8` 字节处。标量字段 ⇒ 必要时
///   `Ushr` 移出、浮点再 `Bitcast`，原指令改 `Copy`；嵌套聚合字段 ⇒ 寄存器里没有地址，
///   只能**开槽写进去**（复用 `memoryize_from_segs`，它自己会登记 + 墓碑化）。
fn expand_extract(
    func: &mut Function,
    store: &forge_isa_runtime::ir::ir::types::TypeStore,
    places: &mut Places,
    shadow_agg: &SecondaryMap<Value, TypeId>,
    block: Block,
    ii: Inst,
) -> Result<(), IrError> {
    // 先把要用的字段**拷出来**（避免与随后的 `make_inst`（可变借用）冲突）。
    let (agg_v, res, idx) = {
        let inst = func.dfg.inst_data(ii);
        let Some(&agg_v) = inst.operands.first() else {
            return Ok(());
        };
        let Some(&res) = inst.results.first() else {
            return Ok(());
        };
        let Some(Immediate::Uint(idx)) = inst.immediates.first() else {
            return Err(IrError::Internal("extractvalue 索引缺失".into()));
        };
        (agg_v, res, *idx as u32)
    };
    let Some(place) = places.get(agg_v).cloned() else {
        return Ok(());
    };
    // 聚合类型：先看值自己的 IR 类型；**by-ref 形参被重映射成 `ptr` 之后**该信息只剩桥表还记着
    // （`shadow_agg`，见 `LegalizeCtx` 的说明）——那时必须在桥表里查，否则这条 extractvalue
    // 会被漏掉（实测：它一路掉到 lowering 报 `Unsupported("DSL lowering")`）。
    let Some(agg_ty) = agg_ty_of(func, store, shadow_agg, agg_v) else {
        return Ok(());
    };
    let foff = store
        .field_offset(agg_ty, idx)
        .ok_or_else(|| IrError::Internal(format!("extractvalue 字段偏移缺失：{idx}")))?;
    let fty = store
        .aggregate_elem_type(agg_ty, idx)
        .ok_or_else(|| IrError::Internal(format!("extractvalue 字段类型缺失：{idx}")))?;

    match place {
        Place::Mem(addr) => {
            let field_addr = field_addr_of(func, block, addr, foff)?;
            if store.is_aggregate(fty) {
                // 嵌套聚合：**登记子地址**即可（字段所在的那段字节就在父内存里）。
                if !places.contains(res) {
                    places.set(res, Place::Mem(field_addr));
                }
                func.tombstone_inst(ii);
                return Ok(());
            }
            let ld = func.make_inst(
                Opcode::Load,
                block,
                smallvec::smallvec![field_addr],
                smallvec::smallvec![],
                &[fty],
                InstFlags::NONE,
            );
            let lv = func.dfg.inst_data(ld).results[0];
            rewrite_to_copy(func, ii, lv);
            Ok(())
        }
        Place::Segs(segs) => {
            let off = foff as usize;
            if store.is_aggregate(fty) {
                let use_pos = UsePos { block, inst: ii };
                return memoryize_from_segs(func, &use_pos, fty, off, &segs, places);
            }
            let base = segs
                .get(off / 8)
                .copied()
                .ok_or_else(|| IrError::Internal("extractvalue 段值缺失".into()))?;
            let inner = (off % 8) as u32;
            let mut v = base;
            if inner > 0 {
                let k = const_i64(func, block, i64::from(inner * 8));
                let sh = func.make_inst(
                    Opcode::Ushr,
                    block,
                    smallvec::smallvec![base, k],
                    smallvec::smallvec![],
                    &[TypeId::I64],
                    InstFlags::NONE,
                );
                v = func.dfg.inst_data(sh).results[0];
            }
            if store.is_float(fty) {
                // 浮点字段：段值是位模式 ⇒ Bitcast 到字段类型（movq_to_xmm 的 IR 形态）。
                let bc = func.make_inst(
                    Opcode::Bitcast,
                    block,
                    smallvec::smallvec![v],
                    smallvec::smallvec![],
                    &[fty],
                    InstFlags::NONE,
                );
                v = func.dfg.inst_data(bc).results[0];
            }
            rewrite_to_copy(func, ii, v);
            Ok(())
        }
    }
}

/// 展开一条 `store <聚合> %v, ptr %dst`（`%v` 在 `Places` 里有位置）：逐 8 字节搬运，
/// 原指令墓碑化（不静默丢数据）。
fn expand_store(
    func: &mut Function,
    store: &forge_isa_runtime::ir::ir::types::TypeStore,
    places: &Places,
    shadow_agg: &SecondaryMap<Value, TypeId>,
    block: Block,
    ii: Inst,
) -> Result<(), IrError> {
    let (src_v, dst) = {
        let inst = func.dfg.inst_data(ii);
        let (Some(&src_v), Some(&dst)) = (inst.operands.first(), inst.operands.get(1)) else {
            return Ok(());
        };
        (src_v, dst)
    };
    let Some(place) = places.get(src_v).cloned() else {
        return Ok(());
    };
    match place {
        Place::Mem(src) => {
            // 字节数：优先取值的 IR 类型；by-ref 重映射之后取桥表（与 extractvalue 同一口径）。
            let size = agg_ty_of(func, store, shadow_agg, src_v)
                .map(|t| store.size_bytes(t) as usize)
                .ok_or_else(|| {
                    IrError::Internal("聚合 store：内存档值缺字节数（类型未知）".into())
                })?;
            let mut cur_src = src;
            let mut cur_dst = dst;
            let mut off = 0usize;
            while off < size {
                let ld = func.make_inst(
                    Opcode::Load,
                    block,
                    smallvec::smallvec![cur_src],
                    smallvec::smallvec![],
                    &[TypeId::I64],
                    InstFlags::NONE,
                );
                let lv = func.dfg.inst_data(ld).results[0];
                func.make_inst(
                    Opcode::Store,
                    block,
                    smallvec::smallvec![lv, cur_dst],
                    smallvec::smallvec![],
                    &[],
                    InstFlags::SIDE_EFFECT,
                );
                off += 8;
                if off < size {
                    cur_src = add_const(func, block, cur_src, 8);
                    cur_dst = add_const(func, block, cur_dst, 8);
                }
            }
            func.tombstone_inst(ii);
            Ok(())
        }
        Place::Segs(segs) => {
            if segs.is_empty() {
                return Err(IrError::Internal("聚合 store：段值档却没有段值".into()));
            }
            let mut addr = dst;
            for (k, s) in segs.iter().enumerate() {
                func.make_inst(
                    Opcode::Store,
                    block,
                    smallvec::smallvec![*s, addr],
                    smallvec::smallvec![],
                    &[],
                    InstFlags::SIDE_EFFECT,
                );
                if k + 1 < segs.len() {
                    addr = add_const(func, block, addr, 8);
                }
            }
            func.tombstone_inst(ii);
            Ok(())
        }
    }
}

/// 一个值代表的聚合类型：先看它自己的 IR 类型，再退回"重映射桥表"（by-ref 形参的情形）。
fn agg_ty_of(
    func: &Function,
    store: &forge_isa_runtime::ir::ir::types::TypeStore,
    shadow_agg: &SecondaryMap<Value, TypeId>,
    v: Value,
) -> Option<TypeId> {
    func.dfg
        .value_type(v)
        .filter(|t| store.is_aggregate(*t))
        .or_else(|| shadow_agg.get(v).copied())
}

/// 字段地址 = `addr + foff`（`foff == 0` 时就是 `addr`，省掉一条常量与一条加法）。
fn field_addr_of(
    func: &mut Function,
    block: Block,
    addr: Value,
    foff: u32,
) -> Result<Value, IrError> {
    if foff == 0 {
        return Ok(addr);
    }
    Ok(add_const(func, block, addr, foff))
}

/// `addr + k`（PTR 算术；`k` 以 i64 常量参与——与既有代码同形）。
fn add_const(func: &mut Function, block: Block, addr: Value, k: u32) -> Value {
    let c = const_i64(func, block, i64::from(k));
    let add = func.make_inst(
        Opcode::Iadd,
        block,
        smallvec::smallvec![addr, c],
        smallvec::smallvec![],
        &[TypeId::PTR],
        InstFlags::NONE,
    );
    func.dfg.inst_data(add).results[0]
}

/// `i64` 常量值（常量池去重）。
fn const_i64(func: &mut Function, block: Block, v: i64) -> Value {
    let cid = func.constants.intern_int(i128::from(v), 64);
    let ci = func.make_inst(
        Opcode::Iconst,
        block,
        smallvec::smallvec![],
        smallvec::smallvec![Immediate::Const(cid)],
        &[TypeId::I64],
        InstFlags::NONE,
    );
    func.dfg.inst_data(ci).results[0]
}

/// 把一条指令就地改成 `Copy <v>`（值由生成指令承载，原指令保留以便其使用者不必改）。
fn rewrite_to_copy(func: &mut Function, ii: Inst, v: Value) {
    {
        let inst = func.dfg.inst_mut(ii);
        inst.opcode = Opcode::Copy;
        inst.operands = smallvec::smallvec![v];
        inst.immediates = smallvec::smallvec![];
    }
    func.refresh_inst_uses(ii);
}

#[cfg(test)]
mod tests {
    use super::*;
    use forge_ir::ir::builder::FunctionBuilder;
    use forge_ir::ir::types::{FunctionSignature, TypeContext};

    /// 夹具：`f({i64,i64} %s) -> i64`，体内 `extractvalue %s,0` 与 `extractvalue %s,1` 相加。
    ///
    /// 与 v23 收尾那条真执行用例同形（它当初就是因为"两个字段一起用"才暴露 use-before-def）。
    fn fixture() -> (Function, Value, Value) {
        let ctx = TypeContext::new();
        let agg = ctx
            .borrow_mut()
            .struct_anon(vec![TypeId::I64, TypeId::I64], false);
        let sig = FunctionSignature::new(&[(agg, "s")], &[TypeId::I64]);
        let mut b = FunctionBuilder::new("mem_extract", ctx, sig);
        let (e, ps) = b.create_block_with_params(&[(agg, "s")]);
        b.switch_to_block(e);
        let slot = b.stack_addr(-32);
        let e0 = b.extract_value(ps[0], 0);
        let e1 = b.extract_value(ps[0], 1);
        let r = b.iadd(e0, e1);
        b.ret(&[r]);
        let f = b.finish().expect("build");
        (f, ps[0], slot)
    }

    /// **生成指令必须先于它的消费者**（这是 v23 真凶的守卫）。
    ///
    /// 判据：展开后不再有 `ExtractValue`；且每条被改写成 `Copy` 的指令，其操作数（`Load` 的结果）
    /// 的**定义指令**在同一块的序里**排在它之前**。
    #[test]
    fn materialized_load_precedes_its_consumer() {
        let (mut f, agg_v, slot) = fixture();
        let mut places = Places::default();
        places.set(agg_v, Place::Mem(slot));
        let cs =
            materialize_memory_uses(&mut f, &mut places, &Default::default()).expect("materialize");
        assert!(cs.total() > 0, "夹具必须真的改动了东西");
        for (b, bd) in f.dfg.blocks() {
            let order: Vec<Inst> = bd.inst_order.clone();
            let pos = |i: Inst| order.iter().position(|&x| x == i).expect("指令在块序里");
            for (k, &ii) in order.iter().enumerate() {
                let inst = f.dfg.inst_data(ii);
                assert_ne!(
                    inst.opcode,
                    Opcode::ExtractValue,
                    "内存档 extractvalue 必须已被物化：{inst:?}"
                );
                if inst.opcode != Opcode::Copy {
                    continue;
                }
                let src = inst.operands[0];
                let ValueDef::Inst(def_ii, _) = f.dfg.value_data(src).def else {
                    continue;
                };
                assert!(
                    pos(def_ii) < k,
                    "块的 {b:?} 里：Copy 排在它操作数的定义之前（use before def）——\
                     这正是 v23 收尾那个真凶的形状"
                );
            }
        }
    }

    /// **段值档**也要走本相（`Place::Segs`）：字段从段值里取（第二个字段在 `off%8` 处 ⇒ 要移位）。
    #[test]
    fn segment_resident_extract_is_materialized() {
        let ctx = TypeContext::new();
        let agg = ctx
            .borrow_mut()
            .struct_anon(vec![TypeId::I32, TypeId::I32], false);
        let sig = FunctionSignature::new(&[(agg, "s")], &[TypeId::I32]);
        let mut b = FunctionBuilder::new("seg_extract", ctx, sig);
        let (e, ps) = b.create_block_with_params(&[(agg, "s")]);
        b.switch_to_block(e);
        let e1 = b.extract_value(ps[0], 1);
        b.ret(&[e1]);
        let mut f = b.finish().expect("build");
        // 段值档：`{i32,i32}` 合 8 字节 ⇒ 一段。夹具里拿不到真实段值，用一个占位值充当
        //（本用例只关心"段值档的 extractvalue 是否被物化"，不关心机器码）。
        let seg = f.dfg.make_value(TypeId::I64, ValueDef::Param(f.entry(), 1));
        let mut places = Places::default();
        places.set(ps[0], Place::Segs(smallvec::smallvec![seg]));
        materialize_memory_uses(&mut f, &mut places, &Default::default()).expect("materialize");
        let ops: Vec<Opcode> = f.dfg.insts().map(|(_, d)| d.opcode).collect();
        assert!(
            !ops.contains(&Opcode::ExtractValue),
            "段值档的 extractvalue 也必须被物化：{ops:?}"
        );
        assert!(
            ops.contains(&Opcode::Ushr),
            "字段在 off%8 != 0 处 ⇒ 必须移位取出：{ops:?}"
        );
    }

    /// **物化相幂等**：同一函数跑第二遍零改动。
    #[test]
    fn materialize_is_idempotent() {
        let (mut f, agg_v, slot) = fixture();
        let mut places = Places::default();
        places.set(agg_v, Place::Mem(slot));
        let first =
            materialize_memory_uses(&mut f, &mut places, &Default::default()).expect("first");
        assert!(first.total() > 0);
        let second =
            materialize_memory_uses(&mut f, &mut places, &Default::default()).expect("second");
        assert_eq!(second.total(), 0, "第二遍不应再有改动：{second:?}");
    }
}
