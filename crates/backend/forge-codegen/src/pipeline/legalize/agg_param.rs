//! 合法化 pass：**被调方形参侧**聚合展开（宽聚合形参接收）。
//!
//! 从 `compiler.rs` 迁出（v23 M6 批次 2）。

use super::common::{AggArgLoc, agg_arg_place, collect_extract_jobs};
use crate::IrError;
use crate::pipeline::agg_expand::UseJob;
use crate::pipeline::agg_expand::UseKind;

use crate::pipeline::agg_expand::{Place, Places};
use forge_ir::*;

/// 被调方大聚合参数展开：按**约定落点**分流——
///
/// - `Registers`（≤ 阈值）：签名拆开为 ceil(size/8) 个 i64 参数（与调用方拆段一致），
///   entry 块参数重建，旧参数值使用处重写（extractvalue → 段值、store → 段 store、
///   return/call → 段值）；
/// - `ByRef`（win64 `>8B`）：形参改为**单个 `ptr`**，并把"该聚合值住在指针指向的内存"
///   登记进 `agg_slots` ⇒ 使用处按地址重写（由物化相接指令序单遍展开
///   分支走 GEP+load、store 走向内存拷贝、传参传该指针）。**不写 `byval` 属性**：
///   引擎对 `byval` 落点给 `Indirect`，而生成物 `move_args` 的 `Indirect` 收参是
///   "从指针 load 出寄存器里的值"（向量语义）——聚合没有寄存器表示，要的是**指针本身**
///   进参数寄存器（`Reg` 落点）；
/// - `Stack`（sysv64 `>16B`）：发射侧尚未接 ⇒ 明确 Unsupported（A2′）。
pub(crate) fn expand_large_agg_params(
    func: &mut Function,
    agg_slots: &mut Places,
    shadow_agg: &mut forge_ir::entity::map::SecondaryMap<Value, TypeId>,
    machine: &impl forge_isa_runtime::machine::target::TargetMachine,
    conv: &str,
) -> Result<(), IrError> {
    let old_params = func.param_types();
    // 每参数：寄存器档的段数（by-ref / 按值进传出区档记 0 = 不拆段）
    let mut seg_map: Vec<usize> = Vec::with_capacity(old_params.len());
    let mut by_ref: Vec<bool> = Vec::with_capacity(old_params.len());
    // 按值整块进传出参数区（sysv64）：**形参原样保留**（布局/偏移由它决定），
    // 只是把"值住在 [fp + offset]"登记进 agg_slots。
    let mut keep_param: Vec<bool> = Vec::with_capacity(old_params.len());
    let mut stack_agg: Vec<(usize, i32, TypeId)> = Vec::new();
    // 按值栈档的 callee-saved 区平移（每个形参探针给出的都是同一个函数级事实）
    let mut agg_stack_shift = 0i32;
    let mut has_large = false;
    for (i, t) in old_params.iter().enumerate() {
        // **一次取锁**（读路径纪律）：判定与落点探针在同一段读锁里做——探针只要
        // `&TypeStore`，不碰 `&mut func`，所以不必为它再取一次读锁。
        let (size, probed) = {
            let ts = func.types.borrow();
            let is_agg = ts.is_aggregate(*t);
            let size = ts.size_bytes(*t);
            let probed = if is_agg && size > 8 {
                Some(agg_arg_place(machine, conv, &ts, *t)?)
            } else {
                None
            };
            (size, probed)
        };
        if let Some((place, shift)) = probed {
            match place {
                AggArgLoc::ByRef => {
                    seg_map.push(0);
                    by_ref.push(true);
                    keep_param.push(false);
                    has_large = true;
                }
                AggArgLoc::StackByVal { offset, .. } => {
                    seg_map.push(0);
                    by_ref.push(false);
                    keep_param.push(true);
                    agg_stack_shift = shift;
                    stack_agg.push((i, offset, *t));
                    has_large = true;
                }
                AggArgLoc::Registers => {
                    seg_map.push(size.div_ceil(8) as usize);
                    by_ref.push(false);
                    keep_param.push(false);
                    has_large = true;
                }
            }
        } else {
            seg_map.push(1);
            by_ref.push(false);
            keep_param.push(true);
        }
    }
    if !has_large {
        return Ok(());
    }
    // 签名重建（by-ref 档一个 `ptr`；寄存器档 N 个 i64）
    let old_sig = func.types.borrow().get_signature(func.signature).clone();
    let mut new_params: Vec<(TypeId, ImmStr)> = Vec::new();
    let mut new_attrs: Vec<forge_ir::ir::function::ParamAttributes> = Vec::new();
    for (i, (t, n)) in old_sig.params.iter().enumerate() {
        let attrs = func.param_attrs.get(i).cloned().unwrap_or_default();
        if keep_param.get(i).copied().unwrap_or(true) {
            // 保留原形参（非聚合参数；以及 sysv64 按值进传出区的聚合——布局由它决定）
            new_params.push((*t, n.clone()));
            new_attrs.push(attrs);
        } else if by_ref.get(i).copied().unwrap_or(false) {
            new_params.push((TypeId::PTR, n.clone()));
            new_attrs.push(attrs);
        } else {
            for _ in 0..seg_map.get(i).copied().unwrap_or(1) {
                new_params.push((TypeId::I64, n.clone()));
                new_attrs.push(attrs.clone());
            }
        }
    }
    let new_sig = FunctionSignature {
        params: new_params,
        returns: old_sig.returns.clone(),
        calling_convention: old_sig.calling_convention,
        variadic: old_sig.variadic,
    };
    func.signature = func.types.borrow_mut().register_signature(new_sig);
    func.param_attrs = new_attrs;
    // entry 块参数重建 + 重写映射（旧参数值 → 新值列表）
    let entry = func.entry(); // fail-closed：缺失入口即编程错误（不再假定 Block(0)）
    let old_bvals = func.dfg.block(entry).param_values.clone();
    let mut rewrite: SecondaryMap<Value, Vec<Value>> = SecondaryMap::new();
    let mut new_bparams: Vec<TypeId> = Vec::new();
    let mut new_bvals: Vec<Value> = Vec::new();
    let mut agg_info: Vec<(Value, Vec<Value>, TypeId)> = Vec::new(); // (旧值, 段值, 聚合类型)
    let mut byref_info: Vec<(Value, Value, TypeId)> = Vec::new(); // (旧值, 指针值, 聚合类型)
    for (i, t) in old_params.iter().enumerate() {
        let old_v = old_bvals[i];
        if keep_param.get(i).copied().unwrap_or(true) {
            // 原样保留（值不变、使用不重写）
            new_bvals.push(old_v);
            new_bparams.push(*t);
            continue;
        }
        if by_ref.get(i).copied().unwrap_or(false) {
            let v = func
                .dfg
                .make_value(TypeId::PTR, ValueDef::Param(entry, new_bvals.len() as u16));
            new_bvals.push(v);
            new_bparams.push(TypeId::PTR);
            rewrite.insert(old_v, vec![v]);
            byref_info.push((old_v, v, *t));
            continue;
        }
        let segs = seg_map.get(i).copied().unwrap_or(1);
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
    let bd = func.dfg.block_mut(entry);
    bd.params = smallvec::SmallVec::from_iter(new_bparams);
    bd.param_values = smallvec::SmallVec::from_iter(new_bvals);
    // 按值进传出区档（sysv64）：形参保留（布局/偏移由它决定），但值**不占寄存器**——
    // 把"该聚合住在 `[fp + offset]`"登记进 `agg_slots`，使用处按地址读。
    // `StackAddr` 的 lea 位移 = 立即数 − callee-saved 区平移（`lowering.rs` 的
    // `current_offset`）⇒ 立即数写 `offset + shift` 才正好落在 `[fp + offset]`
    //（`shift` 与 `LowerCtx::stack_slot_shift` 同源，见 `agg_arg_place`）。
    for (i, offset, _ty) in stack_agg {
        let old_v = old_bvals[i];
        let sa = func.make_inst(
            Opcode::StackAddr,
            entry,
            smallvec::smallvec![],
            smallvec::smallvec![Immediate::Int(
                i64::from(offset) + i64::from(agg_stack_shift)
            )],
            &[TypeId::PTR],
            InstFlags::NONE,
        );
        let addr = func.dfg.inst_data(sa).results[0];
        // 取址要**支配**所有使用 ⇒ 放到 entry 开头（先 pop 再插，保持 inst_order 一致）
        {
            let order = &mut func.dfg.block_mut(entry).inst_order;
            let ni = order.pop().expect("刚 push 的 StackAddr");
            order.insert(0, ni);
        }
        agg_slots.set(old_v, Place::Mem(addr));
    }
    // by-ref 档：登记"聚合住在指针指向的内存"，再按地址重写使用处
    for (old_v, ptr_v, agg_ty) in byref_info {
        agg_slots.set(old_v, Place::Mem(ptr_v));
        // **事实交接**：签名重建会把 `old_v` 全局替换成 `ptr_v`（`replace_all_uses`），
        // 之后"`ptr_v` 代表 `agg_ty` 型聚合"就不在 IR 类型里了 ⇒ 记进桥表给物化相用。
        shadow_agg.insert(ptr_v, agg_ty);
        // `ptr_v` **本身就是地址**（指向调用方副本）：重映射之后的使用处要按内存档处理，
        // 所以对"新值"也登记一条位置——物化相查的是**使用点上那个值**。实测只登记旧值不够：
        // 重映射后残留的 extractvalue 查不到位置 ⇒ 一路掉到 lowering 报
        // `Unsupported("DSL lowering")`。
        agg_slots.set(ptr_v, Place::Mem(ptr_v));
    }
    // 使用处重写（聚合参数——rewrite 内部即时移动辅助指令）
    for (old_v, segs, _agg_ty) in agg_info {
        // v24 M1：把"值住在这些段里"登记进位置表 ⇒ 物化相据此接管它的 `extractvalue`/`store`
        //（`call`/`ret`/`insertvalue` 也已由物化相统一接管）。
        if !agg_slots.contains(old_v) {
            agg_slots.set(old_v, Place::Segs(segs.clone().into()));
        }
    }
    // 非聚合参数：直接替换（单值——参数索引变化后引用保持）。
    // 用 `Function::replace_all_uses` 一次覆盖**指令操作数与终结符用值**并同步
    // use-def（此前手写逐块改 `inst.operands` 不刷新 use-lists，会留下陈旧 use 项）。
    for (old_v, vals) in rewrite.iter() {
        if vals.len() == 1 {
            func.replace_all_uses(old_v, vals[0]);
        }
    }
    Ok(())
}

/// 大聚合 load 值传播（全 ISA 通用，编译期）：`%v = load T(>8B), ptr %p` 的
/// 大聚合值无法入 GPR——按使用处就地展开（load 指令标 Nop，值不物化）：
/// - `store T %v, ptr %q`（拷贝）：逐段 `load i64 [addr+off]` + `store i64 [q+off]`；
/// - `extractvalue T %v, idx`：偏移 `add ptr addr, field_offset` + 标量 load，
///   extractvalue 改 Copy；
/// - 其余使用（传参/返回/insertvalue 等）：Unsupported。
///
/// 局限：load 后内存被改写时重新 load 不保留快照语义（常见模式安全）。
pub(crate) fn expand_large_aggs(
    func: &mut Function,
    agg_slots: &mut Places,
) -> Result<(), IrError> {
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
            let inst = &func.dfg.inst_data(ii);
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
            let inst = &func.dfg.inst_data(ii);
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
                let ld = func.make_inst(
                    Opcode::Load,
                    job.block,
                    smallvec::smallvec![src_addr],
                    smallvec::smallvec![],
                    &[seg_ty],
                    InstFlags::NONE,
                );
                let ldv = func.dfg.inst_data(ld).results[0];
                new_insts.push(ld);
                // dst: store seg_ty %ldv, ptr %dst_addr
                let st = func.make_inst(
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
                        let off_inst = func.make_inst(
                            Opcode::Iconst,
                            job.block,
                            smallvec::smallvec![],
                            smallvec::smallvec![Immediate::Const(off_cid)],
                            &[TypeId::I64],
                            InstFlags::NONE,
                        );
                        let off_v = func.dfg.inst_data(off_inst).results[0];
                        new_insts.push(off_inst);
                        let add = func.make_inst(
                            Opcode::Iadd,
                            job.block,
                            smallvec::smallvec![*cur, off_v],
                            smallvec::smallvec![],
                            &[TypeId::PTR],
                            InstFlags::NONE,
                        );
                        *cur = func.dfg.inst_data(add).results[0];
                        new_insts.push(add);
                    }
                }
            }
            // 使用指令（Store）标 Nop
            let __tomb = job.inst;
            func.tombstone_inst(__tomb);
        } else if let Some(idx) = extract_idx {
            // S1：命中嵌套聚合槽映射（内层提取）——地址语义优先
            let hit_inst = &func.dfg.inst_data(job.inst);
            if let Some(slot_addr) = agg_slots.addr(hit_inst.operands[0]) {
                let slot_ty = func
                    .dfg
                    .value_type(hit_inst.operands[0])
                    .ok_or_else(|| crate::IrError::Internal("嵌套字段源值没有类型".into()))?;
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
                        let off_inst = func.make_inst(
                            Opcode::Iconst,
                            job.block,
                            smallvec::smallvec![],
                            smallvec::smallvec![Immediate::Const(off_cid)],
                            &[TypeId::I64],
                            InstFlags::NONE,
                        );
                        let off_v = func.dfg.inst_data(off_inst).results[0];
                        new_insts.push(off_inst);
                        let add2 = func.make_inst(
                            Opcode::Iadd,
                            job.block,
                            smallvec::smallvec![addr2, off_v],
                            smallvec::smallvec![],
                            &[TypeId::PTR],
                            InstFlags::NONE,
                        );
                        addr2 = func.dfg.inst_data(add2).results[0];
                        new_insts.push(add2);
                    }
                    let __tomb = job.inst;
                    func.tombstone_inst(__tomb);
                    agg_slots.set(res, Place::Mem(addr2));
                    // 追加内层提取任务（递归）
                    collect_extract_jobs(func, &mut jobs, res, job.load);
                } else {
                    drop(ts);
                    // 标量字段：GEP + load（结果 Copy 绑定）
                    let off_cid = func.constants.insert_int(foff as i128, 64);
                    let off_inst = func.make_inst(
                        Opcode::Iconst,
                        job.block,
                        smallvec::smallvec![],
                        smallvec::smallvec![Immediate::Const(off_cid)],
                        &[TypeId::I64],
                        InstFlags::NONE,
                    );
                    let off_v = func.dfg.inst_data(off_inst).results[0];
                    new_insts.push(off_inst);
                    let add = func.make_inst(
                        Opcode::Iadd,
                        job.block,
                        smallvec::smallvec![slot_addr, off_v],
                        smallvec::smallvec![],
                        &[TypeId::PTR],
                        InstFlags::NONE,
                    );
                    let p = func.dfg.inst_data(add).results[0];
                    new_insts.push(add);
                    let ld = func.make_inst(
                        Opcode::Load,
                        job.block,
                        smallvec::smallvec![p],
                        smallvec::smallvec![],
                        &[fty],
                        InstFlags::NONE,
                    );
                    let lv = func.dfg.inst_data(ld).results[0];
                    new_insts.push(ld);
                    let __cp = job.inst;
                    {
                        let inst = func.dfg.inst_mut(__cp);
                        inst.opcode = Opcode::Copy;
                        inst.operands = smallvec::smallvec![lv];
                        inst.immediates = smallvec::smallvec![];
                    }
                    func.refresh_inst_uses(__cp);
                }
                // 新指令移到使用位置前（与下方公共逻辑一致；此处直接走尾部）
                if !new_insts.is_empty() {
                    let mut moved: Vec<Inst> = Vec::with_capacity(new_insts.len());
                    for _ in 0..new_insts.len() {
                        let order = &mut func.dfg.block_mut(job.block).inst_order;
                        moved.push(order.pop().unwrap());
                    }
                    moved.reverse();
                    for (k, &ni) in moved.iter().enumerate() {
                        func.dfg
                            .block_mut(job.block)
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
                    let off2_inst = func.make_inst(
                        Opcode::Iconst,
                        job.block,
                        smallvec::smallvec![],
                        smallvec::smallvec![Immediate::Const(off2_cid)],
                        &[TypeId::I64],
                        InstFlags::NONE,
                    );
                    let off2_v = func.dfg.inst_data(off2_inst).results[0];
                    new_insts.push(off2_inst);
                    let add2 = func.make_inst(
                        Opcode::Iadd,
                        job.block,
                        smallvec::smallvec![addr2, off2_v],
                        smallvec::smallvec![],
                        &[TypeId::PTR],
                        InstFlags::NONE,
                    );
                    addr2 = func.dfg.inst_data(add2).results[0];
                    new_insts.push(add2);
                }
                let __tomb = job.inst;
                func.tombstone_inst(__tomb);
                let res_new = func.dfg.inst_data(__tomb).results[0];
                agg_slots.set(res_new, Place::Mem(addr2));
                collect_extract_jobs(func, &mut jobs, res_new, job.load);
            } else {
                let off = ts.field_offset(li.ty, idx).ok_or_else(|| {
                    IrError::Unsupported(format!("extractvalue 字段偏移缺失：{idx}"))
                })?;
                drop(ts);
                let mut prev_addr = li.addr;
                if off > 0 {
                    let off_cid = func.constants.insert_int(off as i128, 64);
                    let off_inst = func.make_inst(
                        Opcode::Iconst,
                        job.block,
                        smallvec::smallvec![],
                        smallvec::smallvec![Immediate::Const(off_cid)],
                        &[TypeId::I64],
                        InstFlags::NONE,
                    );
                    let off_v = func.dfg.inst_data(off_inst).results[0];
                    new_insts.push(off_inst);
                    let add = func.make_inst(
                        Opcode::Iadd,
                        job.block,
                        smallvec::smallvec![prev_addr, off_v],
                        smallvec::smallvec![],
                        &[TypeId::PTR],
                        InstFlags::NONE,
                    );
                    prev_addr = func.dfg.inst_data(add).results[0];
                    new_insts.push(add);
                }
                let ld = func.make_inst(
                    Opcode::Load,
                    job.block,
                    smallvec::smallvec![prev_addr],
                    smallvec::smallvec![],
                    &[field_ty],
                    InstFlags::NONE,
                );
                let ldv = func.dfg.inst_data(ld).results[0];
                new_insts.push(ld);
                // extractvalue 改 Copy（result 保留 → 新 load 值）
                let __cp = job.inst;
                {
                    let inst = func.dfg.inst_mut(__cp);
                    inst.opcode = Opcode::Copy;
                    inst.operands = smallvec::smallvec![ldv];
                    inst.immediates = smallvec::smallvec![];
                }
                func.refresh_inst_uses(__cp);
            }
        }
        // 新指令移到使用指令位置前
        let mut moved: Vec<Inst> = Vec::with_capacity(new_insts.len());
        for _ in 0..new_insts.len() {
            let order = &mut func.dfg.block_mut(job.block).inst_order;
            moved.push(order.pop().unwrap());
        }
        moved.reverse();
        for (k, &ni) in moved.iter().enumerate() {
            func.dfg
                .block_mut(job.block)
                .inst_order
                .insert(job.pos + k, ni);
        }
    }
    // 所有大聚合 load 标 Nop（先收集指令索引，避免借用冲突）
    let mut nop_list: Vec<Inst> = Vec::new();
    for (_, bd) in func.dfg.blocks() {
        for &ii in &bd.inst_order {
            let inst = &func.dfg.inst_data(ii);
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
        let __tomb = ii;
        func.tombstone_inst(__tomb);
    }
    Ok(())
}
