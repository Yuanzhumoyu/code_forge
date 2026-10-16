//! 从 `agg_call.rs` 拆出（v25 M4：逐字搬移，不改行为）。
use crate::IrError;
use crate::pipeline::agg_expand::{Place, Places};
use forge_ir::*;

/// 调用方 call 结果（大聚合 >8 ≤16）拆 2 段：`%q = call {i64,i64}` 的 results
/// 改 [i64, i64]（ValueDef::Inst(call, 0/1)——lowering 按 results.len() 生成
/// RAX/RDX 双 mov），旧结果值使用处重写（extractvalue/store/转传）。
pub(crate) fn expand_large_agg_call_results(
    func: &mut Function,
    edges: &crate::pipeline::legalize::plan::AbiEdges,
    agg_slots: &mut Places,
) -> Result<(), IrError> {
    // 取一次读锁（v3 S3 读路径纪律）：本函数只读类型，内部不再逐次取锁。
    // 先从 func.types clone 出 TypeContext（Arc，O(1)）再取锁——守卫借的是这个局部，
    // 不与 func 的 &mut 借用冲突。
    let types_ctx = func.types.clone();
    let store = types_ctx.borrow();
    // 收集（block, pos, inst, 旧结果, 聚合类型, 是否 sret）——`block/pos` 供 sret 档
    // 把缓冲的 Alloca 插到 call 之前。
    struct Job {
        block: Block,
        pos: usize,
        inst: Inst,
        old_r: Value,
        ty: TypeId,
        sret: bool,
    }
    let mut jobs: Vec<Job> = Vec::new();
    for (b, bd) in func.dfg.blocks() {
        for (pos, &ii) in bd.inst_order.iter().enumerate() {
            let inst = &func.dfg.inst_data(ii);
            if matches!(inst.opcode, Opcode::Call | Opcode::CallIndirect)
                && inst.results.len() == 1
                && let Some(rt) = inst.results.first().and_then(|v| func.dfg.value_type(*v))
                && store.is_aggregate(rt)
                && store.size_bytes(rt) > 8
            {
                // **只读计划**（v25 M1）：「既不是 sret 也不是寄存器档」这条判据只有一处
                //（计划相），走到这里说明计划与 pass 不一致。
                let Some(ret) = edges.call_result_of(ii) else {
                    return Err(IrError::Internal(format!(
                        "call {ii:?} 的宽聚合结果不在计划里（计划相应已登记）"
                    )));
                };
                let sret = matches!(ret, forge_abi::RetLoc::Indirect { .. });
                jobs.push(Job {
                    block: b,
                    pos,
                    inst: ii,
                    old_r: inst.results[0],
                    ty: rt,
                    sret,
                });
            }
        }
    }
    for job in jobs {
        // **sret 档**（A1′-ret）：调用方出缓冲，把它作为**第 0 个实参**传进去——
        // 约定把隐藏返回槽放在首个整数槽，正是实参 0 的落点；call 结果清空（值不回寄存器），
        // 旧结果值登记成"住在该缓冲"，其使用处按地址重写。
        if job.sret {
            let size = store.size_bytes(job.ty) as usize;
            // v23 M6：规范 immediate 形态的唯一出口（`frame_slots.rs` 只认 `Type + Uint`）。
            let (_alloca, buf) =
                crate::pipeline::legalize::common::place_addr_of_slot(func, job.block, size);
            // 缓冲的取址必须**先于** call 执行（SSA 支配）——把刚 push 到块尾的 Alloca
            // 移到 call 之前（同 `expand_agg_call_args` 的做法）。
            {
                let order = &mut func.dfg.block_mut(job.block).inst_order;
                let ni = order.pop().expect("刚 push 的 Alloca");
                order.insert(job.pos, ni);
            }
            let mut ops = func.dfg.inst_data(job.inst).operands.clone();
            ops.insert(0, buf);
            func.dfg.inst_mut(job.inst).operands = ops;
            func.dfg.inst_mut(job.inst).results = smallvec::smallvec![];
            func.refresh_inst_uses(job.inst);
            agg_slots.set(job.old_r, Place::Mem(buf));
            let _ = job.ty;
            continue;
        }
        // results 拆 2（新值——Inst(call, 0/1)）
        let r0 = func
            .dfg
            .make_value(TypeId::I64, ValueDef::Inst(job.inst, 0));
        let r1 = func
            .dfg
            .make_value(TypeId::I64, ValueDef::Inst(job.inst, 1));
        let inst = func.dfg.inst_mut(job.inst);
        inst.results = smallvec::smallvec![r0, r1];
        // 旧结果使用处重写
    }
    Ok(())
}

/// ret 大聚合展开：`ret {i64,i64} %v`（>8 ≤16 字节）→ `Terminator::Return
/// { values: [seg0, seg1] }`（多值返回——lowering 按 values.len() 生成
/// RAX/RDX 双槽 mov）。段来源：AggConst（Iconst 段）、load 结果（段 load）、
/// 其余（参数段值已被 expand_large_agg_params 重写为 i64——不在此处理）。
pub(crate) fn expand_large_agg_ret(
    func: &mut Function,
    agg_slots: &mut Places,
    sret_ptr: Option<Value>,
) -> Result<(), IrError> {
    // 取一次读锁（v3 S3 读路径纪律）：本函数只读类型，内部不再逐次取锁。
    // 先从 func.types clone 出 TypeContext（Arc，O(1)）再取锁——守卫借的是这个局部，
    // 不与 func 的 &mut 借用冲突。
    let types_ctx = func.types.clone();
    let store = types_ctx.borrow();
    // sret 档：签名的返回已被 `expand_sret_return` 清空 ⇒ 判据只能看**终结符里的实参**
    let sret = sret_ptr.is_some();
    let rets = func.return_types();
    let has_large = rets.iter().any(|t| {
        let s = &store;
        s.is_aggregate(*t) && s.size_bytes(*t) > 8
    });
    if !has_large && !sret {
        return Ok(());
    }
    // 收集 Return 里的聚合值（按块——先收集再改）
    struct RetJob {
        block: Block,
        val: Value,
        ty: TypeId,
    }
    let mut jobs: Vec<RetJob> = Vec::new();
    for (b, _bd) in func.dfg.blocks() {
        let Some(values) = func.dfg.term_return_values(b) else {
            continue;
        };
        for &v in values {
            if let Some(ty) = func.dfg.value_type(v)
                && store.is_aggregate(ty)
                && store.size_bytes(ty) > 8
            {
                jobs.push(RetJob {
                    block: b,
                    val: v,
                    ty,
                });
            }
        }
    }
    for job in jobs {
        let size = store.size_bytes(job.ty) as usize;
        if !sret && size > 16 {
            return Err(crate::IrError::capability(
                "AGG_RET_STACK_UNSUPPORTED",
                format!(
                    "聚合返回 {size} 字节：这份约定既不是 sret 也不是寄存器档（栈返回未实现）：type {:?}",
                    job.ty
                ),
            ));
        }
        // by-ref 档优先：值可能住在内存里（by-ref 形参 / sret call 结果）⇒ 段值 = 逐段 load
        let mem_addr = agg_slots.addr(job.val);
        let mut segs: Vec<Value> = Vec::new();
        let mut gen_insts: Vec<Inst> = Vec::new();
        match func.dfg.value_data(job.val).def {
            ValueDef::AggConst(aid) if mem_addr.is_none() => {
                let agg = func
                    .constants
                    .get_aggregate(aid)
                    .ok_or_else(|| IrError::Internal("聚合返回：常量池缺失 AggId".into()))?;
                let bytes = crate::pipeline::agg_const::pack_agg_bytes(func, agg)?;
                let mut off = 0usize;
                while off < size {
                    // v23 M6：打包口径单点化（`agg_const::pack_segment`）。
                    let seg_val = crate::pipeline::agg_const::pack_segment(&bytes, off, 8);
                    let cid = func.constants.insert_int(seg_val as i128, 64);
                    let ci = func.make_inst(
                        Opcode::Iconst,
                        job.block,
                        smallvec::smallvec![],
                        smallvec::smallvec![Immediate::Const(cid)],
                        &[TypeId::I64],
                        InstFlags::NONE,
                    );
                    let cv = func.dfg.inst_data(ci).results[0];
                    gen_insts.push(ci);
                    segs.push(cv);
                    off += 8;
                }
            }
            ValueDef::Inst(li, 0) if mem_addr.is_none() => {
                // load 结果 → 段 load
                let addr = func
                    .dfg
                    .inst_data(li)
                    .operands
                    .first()
                    .copied()
                    .ok_or_else(|| IrError::Internal("聚合返回：load 无地址".into()))?;
                let mut prev_addr = addr;
                let mut off = 0usize;
                while off < size {
                    let ld = func.make_inst(
                        Opcode::Load,
                        job.block,
                        smallvec::smallvec![prev_addr],
                        smallvec::smallvec![],
                        &[TypeId::I64],
                        InstFlags::NONE,
                    );
                    let ldv = func.dfg.inst_data(ld).results[0];
                    gen_insts.push(ld);
                    segs.push(ldv);
                    off += 8;
                    if off < size {
                        let cid = func.constants.insert_int(8, 64);
                        let ci = func.make_inst(
                            Opcode::Iconst,
                            job.block,
                            smallvec::smallvec![],
                            smallvec::smallvec![Immediate::Const(cid)],
                            &[TypeId::I64],
                            InstFlags::NONE,
                        );
                        let cv = func.dfg.inst_data(ci).results[0];
                        gen_insts.push(ci);
                        let add = func.make_inst(
                            Opcode::Iadd,
                            job.block,
                            smallvec::smallvec![prev_addr, cv],
                            smallvec::smallvec![],
                            &[TypeId::PTR],
                            InstFlags::NONE,
                        );
                        prev_addr = func.dfg.inst_data(add).results[0];
                        gen_insts.push(add);
                    }
                }
            }
            _ => {
                // by-ref 档（值住在指针指向的内存）：从该地址逐段 load
                let Some(addr) = mem_addr else {
                    return Err(IrError::Internal(format!(
                        "聚合返回：值来源不支持（def {:?}）",
                        func.dfg.value_data(job.val).def
                    )));
                };
                let mut prev_addr = addr;
                let mut off = 0usize;
                while off < size {
                    let ld = func.make_inst(
                        Opcode::Load,
                        job.block,
                        smallvec::smallvec![prev_addr],
                        smallvec::smallvec![],
                        &[TypeId::I64],
                        InstFlags::NONE,
                    );
                    let ldv = func.dfg.inst_data(ld).results[0];
                    gen_insts.push(ld);
                    segs.push(ldv);
                    off += 8;
                    if off < size {
                        let cid = func.constants.insert_int(8, 64);
                        let ci = func.make_inst(
                            Opcode::Iconst,
                            job.block,
                            smallvec::smallvec![],
                            smallvec::smallvec![Immediate::Const(cid)],
                            &[TypeId::I64],
                            InstFlags::NONE,
                        );
                        let cv = func.dfg.inst_data(ci).results[0];
                        gen_insts.push(ci);
                        let add = func.make_inst(
                            Opcode::Iadd,
                            job.block,
                            smallvec::smallvec![prev_addr, cv],
                            smallvec::smallvec![],
                            &[TypeId::PTR],
                            InstFlags::NONE,
                        );
                        prev_addr = func.dfg.inst_data(add).results[0];
                        gen_insts.push(add);
                    }
                }
            }
        }
        // **sret 档**：段值写进调用方缓冲，函数变 void 返回（值不再经返回寄存器）
        if let Some(sp) = sret_ptr {
            let mut cur = sp;
            for (k, s) in segs.iter().enumerate() {
                let st = func.make_inst(
                    Opcode::Store,
                    job.block,
                    smallvec::smallvec![*s, cur],
                    smallvec::smallvec![],
                    &[],
                    InstFlags::SIDE_EFFECT,
                );
                gen_insts.push(st);
                if k + 1 < segs.len() {
                    let c8 = func.constants.insert_int(8, 64);
                    let ci = func.make_inst(
                        Opcode::Iconst,
                        job.block,
                        smallvec::smallvec![],
                        smallvec::smallvec![Immediate::Const(c8)],
                        &[TypeId::I64],
                        InstFlags::NONE,
                    );
                    let cv = func.dfg.inst_data(ci).results[0];
                    gen_insts.push(ci);
                    let add = func.make_inst(
                        Opcode::Iadd,
                        job.block,
                        smallvec::smallvec![cur, cv],
                        smallvec::smallvec![],
                        &[TypeId::PTR],
                        InstFlags::NONE,
                    );
                    cur = func.dfg.inst_data(add).results[0];
                    gen_insts.push(add);
                }
            }
            func.set_return_values(job.block, []);
            continue;
        }
        // Return values 替换（段值）——按形式写入口同步 use-def
        let old_values: Vec<Value> = func
            .dfg
            .term_return_values(job.block)
            .unwrap_or(&[])
            .to_vec();
        let mut new_vals: Vec<Value> = Vec::new();
        for &v in old_values.iter() {
            if v == job.val {
                new_vals.extend_from_slice(&segs);
            } else {
                new_vals.push(v);
            }
        }
        func.set_return_values(job.block, &new_vals);
        // gen 指令（段常量/段 load/add）已由 make_inst push 到块尾——顺序正确
        // （生成顺序），位于块内指令之后、terminator 之前——无需移动。
    }
    Ok(())
}
