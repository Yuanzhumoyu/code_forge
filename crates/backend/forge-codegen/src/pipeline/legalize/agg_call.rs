//! 合法化 pass：**调用侧**聚合展开（sret 形参、call 结果拆段）。
//!
//! 从 `compiler.rs` 迁出（v23 M6 批次 2）——pass 体不再住在调用点旁边，
//! 由 `legalize::LegalizeDriver` 定序调用。

use super::common::{AggArgLoc, agg_arg_place, agg_ret_is_sret, append_param};
use crate::IrError;
use crate::pipeline::agg_expand::AggSlots;
use crate::pipeline::agg_expand::rewrite_agg_value_uses;
use forge_ir::*;

/// 按值返回 sret 的**前置**处理（A1′-ret）：约定把 `>8B` 聚合返回落进**调用方缓冲**
/// （`plan.ret = Indirect`，即 sret）时，给函数**追加**一个带 `sret` 属性的 `ptr` 形参，
/// 并把签名的返回清空——值写进缓冲由 [`expand_large_agg_ret`] 完成。
///
/// **必须先于其它聚合展开**：那些展开（形参/实参/call 结果）会把返回值重写或登记进
/// `agg_slots`，早一步把 sret 形参立起来，后续它们才知道"返回值往哪儿写"。
pub(crate) fn expand_sret_return(
    func: &mut Function,
    machine: &impl forge_isa_runtime::machine::target::TargetMachine,
    conv: &str,
) -> Result<Option<Value>, IrError> {
    // **一次取锁**：找宽聚合返回 + 问它的落点，在同一段读锁里做（探针只要 `&TypeStore`）。
    let sret_ty: Option<TypeId> = {
        let store = func.types.borrow();
        let hit = func
            .return_types()
            .into_iter()
            .find(|t| store.is_aggregate(*t) && store.size_bytes(*t) > 8);
        match hit {
            Some(ty) if agg_ret_is_sret(machine, conv, &store, ty)? => Some(ty),
            _ => None,
        }
    };
    let Some(_ty) = sret_ty else {
        return Ok(None);
    };
    // **不写 `sret` 属性**（与 A1′ 的 by-ref 形参同一条规律）：引擎对带 sret/byval 的落点
    // 给 `Indirect`，而生成物 `move_args` 的 `Indirect` 收参是"从指针 **load** 出值"
    // （值为寄存器语义）——那样形参拿到的是**缓冲里的内容**而不是缓冲地址。
    // 按约定，sret 缓冲指针本来就落在**首个整数槽**（win64 = RCX、sysv64 = RDI），
    // 与"一个普通的第 0 个指针实参"完全同形 ⇒ 直接用普通 `ptr` 形参，调用方把它当实参 0 传。
    let p = append_param(func, TypeId::PTR, "<sret>", Default::default())?;
    // 签名返回清空：缓冲由 sret 形参承载（引擎把它收进约定的隐藏返回槽）
    let old_sig = func.types.borrow().get_signature(func.signature).clone();
    let new_sig = FunctionSignature {
        params: old_sig.params.clone(),
        returns: Vec::new(),
        calling_convention: old_sig.calling_convention,
        variadic: old_sig.variadic,
    };
    func.signature = func.types.borrow_mut().register_signature(new_sig);
    Ok(Some(p))
}

/// 调用方 call 结果（大聚合 >8 ≤16）拆 2 段：`%q = call {i64,i64}` 的 results
/// 改 [i64, i64]（ValueDef::Inst(call, 0/1)——lowering 按 results.len() 生成
/// RAX/RDX 双 mov），旧结果值使用处重写（extractvalue/store/转传）。
pub(crate) fn expand_large_agg_call_results(
    func: &mut Function,
    agg_slots: &mut AggSlots,
    machine: &impl forge_isa_runtime::machine::target::TargetMachine,
    conv: &str,
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
                let size = store.size_bytes(rt) as usize;
                let sret = agg_ret_is_sret(machine, conv, &store, rt)?;
                if !sret && size > 16 {
                    return Err(IrError::Unsupported(format!(
                        "聚合返回 {size} 字节：这份约定既不是 sret 也不是寄存器档：type {rt:?}"
                    )));
                }
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
            let (_alloca, buf) = super::common::place_addr_of_slot(func, job.block, size);
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
            agg_slots.insert(job.old_r, (buf, job.ty));
            rewrite_agg_value_uses(func, job.old_r, &[], job.ty, agg_slots)?;
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
        rewrite_agg_value_uses(func, job.old_r, &[r0, r1], job.ty, agg_slots)?;
    }
    Ok(())
}

/// ret 大聚合展开：`ret {i64,i64} %v`（>8 ≤16 字节）→ `Terminator::Return
/// { values: [seg0, seg1] }`（多值返回——lowering 按 values.len() 生成
/// RAX/RDX 双槽 mov）。段来源：AggConst（Iconst 段）、load 结果（段 load）、
/// 其余（参数段值已被 expand_large_agg_params 重写为 i64——不在此处理）。
pub(crate) fn expand_large_agg_ret(
    func: &mut Function,
    agg_slots: &mut AggSlots,
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
            return Err(IrError::Unsupported(format!(
                "聚合返回 {size} 字节：这份约定既不是 sret 也不是寄存器档（栈返回未实现）：type {:?}",
                job.ty
            )));
        }
        // by-ref 档优先：值可能住在内存里（by-ref 形参 / sret call 结果）⇒ 段值 = 逐段 load
        let mem_addr = agg_slots.get(job.val).map(|&(a, _)| a);
        let mut segs: Vec<Value> = Vec::new();
        let mut gen_insts: Vec<Inst> = Vec::new();
        match func.dfg.value_data(job.val).def {
            ValueDef::AggConst(aid) if mem_addr.is_none() => {
                let agg = func
                    .constants
                    .get_aggregate(aid)
                    .ok_or_else(|| IrError::Unsupported("聚合返回：常量池缺失 AggId".into()))?;
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
                    .ok_or_else(|| IrError::Unsupported("聚合返回：load 无地址".into()))?;
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
                    return Err(IrError::Unsupported(format!(
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

/// 函数是否含 >8 字节聚合 load（决定是否需要可变副本做大聚合值传播）。：`call @f({i64,i64} %v)` 的
/// 聚合实参（>8 且 ≤16 字节——SysV 整数寄存器路径）拆为 ceil(size/8) 个 i64
/// 段值追加到 call operands（内部签名宽松——callee 签名不校验；lowering 按
/// operands 逐参数占槽——RCX/RDX/R8/R9 + 栈参数）。
/// 段来源：AggConst（字节已知——插 Iconst 段常量）、load 结果（追踪 addr——
/// 插段 load）；其余（块参数/小聚合寄存器值）Unsupported。>16 字节栈传递
/// 未实现（Unsupported）。
/// 必须在 expand_large_aggs **之前**：拆段后大聚合值不再被 call 使用，
/// expand_large_aggs 的使用检测不命中 Call。
pub(crate) fn expand_agg_call_args(
    func: &mut Function,
    machine: &impl forge_isa_runtime::machine::target::TargetMachine,
    conv: &str,
) -> Result<(), IrError> {
    // 取一次读锁（v3 S3 读路径纪律）：本函数只读类型，内部不再逐次取锁。
    // 先从 func.types clone 出 TypeContext（Arc，O(1)）再取锁——守卫借的是这个局部，
    // 不与 func 的 &mut 借用冲突。
    let types_ctx = func.types.clone();
    let store = types_ctx.borrow();
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
            let inst = &func.dfg.inst_data(ii);
            if matches!(inst.opcode, Opcode::Call | Opcode::CallIndirect) {
                let skip = usize::from(inst.opcode == Opcode::CallIndirect);
                for (op_idx, &v) in inst.operands.iter().enumerate().skip(skip) {
                    if let Some(ty) = func.dfg.value_type(v)
                        && store.is_aggregate(ty)
                        && store.size_bytes(ty) > 8
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
        let size = store.size_bytes(ty) as usize;
        // 落点分流（只读探针）：
        //  - `ByRef`（win64）：本地建副本 + 传**指针**，不写属性 ⇒ 形状 = ptr ⇒ 落点 `Reg`；
        //  - `StackByVal`（sysv64）：同样建副本 + 传指针，但**写 `byval(聚合类型)` 属性**
        //    ⇒ 调用点形状按**聚合**算 ⇒ 落点 `StackByVal`，发射侧从指针逐段拷进传出区；
        //  - `Registers`：现有拆段路径。
        let (place, _) = agg_arg_place(machine, conv, &store, ty)?;
        let by_ref = matches!(place, AggArgLoc::ByRef);
        // **按值整块进传出区**（sysv64）：与 by-ref 一样先在调用方本地建副本、传指针；
        // 区别是**写 `byval(聚合类型)` 属性**——调用点形状据此按**聚合**算（⇒ 落点
        // `StackByVal`），发射侧再从该指针逐 8 字节拷进传出区（`arg_bytes_*` 三角色）。
        // 被调方一侧的地址口径已由 lowering 的"正立即数 = ABI 相对"钉住。
        let stack_by_val = matches!(place, AggArgLoc::StackByVal { .. });
        if !by_ref && !stack_by_val && size > 16 {
            return Err(IrError::Unsupported(format!(
                "聚合实参 {size} 字节：约定落点既不是寄存器档也不是按引用档（type {ty:?}）"
            )));
        }
        // 段值生成
        let mut segs: Vec<Value> = Vec::new();
        let mut new_insts: Vec<Inst> = Vec::new();
        match func.dfg.value_data(job.val).def {
            ValueDef::AggConst(aid) => {
                let agg = func
                    .constants
                    .get_aggregate(aid)
                    .ok_or_else(|| IrError::Unsupported("聚合实参：常量池缺失 AggId".into()))?;
                let bytes = crate::pipeline::agg_const::pack_agg_bytes(func, agg)?;
                let mut off = 0usize;
                while off < size {
                    // v23 M6：打包口径单点化（`agg_const::pack_segment`）。
                    let seg_val = crate::pipeline::agg_const::pack_segment(&bytes, off, 8);
                    let cid = func.constants.insert_int(seg_val as i128, 64);
                    let iconst = func.make_inst(
                        Opcode::Iconst,
                        job.block,
                        smallvec::smallvec![],
                        smallvec::smallvec![Immediate::Const(cid)],
                        &[TypeId::I64],
                        InstFlags::NONE,
                    );
                    let cv = func.dfg.inst_data(iconst).results[0];
                    new_insts.push(iconst);
                    segs.push(cv);
                    off += 8;
                }
            }
            // **只认"load 结果"这一种指令来源**（有可追踪的地址）。其它指令来源
            //（`insertvalue`/`undef`/`copy` 链搭出来的聚合值）落到下面的 `_` 臂：先内存化、
            // 拿到槽地址再逐段 load。写成模式守卫而不是在臂里判 opcode，是为了**不复制**
            // 逐段 load 的那段代码。
            ValueDef::Inst(li, 0) if func.dfg.inst_data(li).opcode == Opcode::Load => {
                // load 结果：追踪 addr，逐段 load
                let addr = func
                    .dfg
                    .inst_data(li)
                    .operands
                    .first()
                    .copied()
                    .ok_or_else(|| IrError::Unsupported("聚合实参：load 无地址".into()))?;
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
                    new_insts.push(ld);
                    segs.push(ldv);
                    off += 8;
                    if off < size {
                        let off_cid = func.constants.insert_int(8, 64);
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
                }
            }
            _ => {
                // **未知来源**：`insertvalue`/`undef`/`copy` 链搭出来的聚合值。
                //
                // v23 收尾 M7 批次 8 实测：给它写了个"只内存化、不动使用处"的助手
                //（按值定义链开槽 + 搬字节 + 写插入字段 + 登记），编译与运行都通了，但**值不对**
                // （`main_ins` 返回 `72198297058681447` 而非 11）⇒ 按本片的铁律**回退**：
                // 宁可在编译期明确拒绝，也不要静默错值。助手已删除，缺陷记在账本待查。
                return Err(IrError::Unsupported(format!(
                    "聚合实参拆段：值来源不支持（def {:?}）——`insertvalue`/`undef` 链搭出来的聚合\
                     作为实参尚未实现（记档待查）",
                    func.dfg.value_data(job.val).def
                )));
            }
        }
        // 落点落地：ByRef ⇒ 建一个调用方帧内副本、逐段 store，实参传**该副本的指针**
        // （Win64 psABI 的"指向调用方副本的指针"就是这个形状；机器码层与 byval 等价，
        // 但不进生成物的 `Indirect` 那条"从指针 load 出值"的路径）；
        // 其它 ⇒ 在 op_idx 处插入段值。
        let mut ops = func.dfg.inst_data(job.inst).operands.clone();
        if by_ref || stack_by_val {
            // v23 M6：规范 immediate 形态的唯一出口（写 `Const(池 id)` 会让槽不进表 ⇒ 没偏移）。
            let (alloca, slot) = super::common::place_addr_of_slot(func, job.block, size);
            new_insts.push(alloca);
            let mut cur = slot;
            for (k, s) in segs.iter().enumerate() {
                let st = func.make_inst(
                    Opcode::Store,
                    job.block,
                    smallvec::smallvec![*s, cur],
                    smallvec::smallvec![],
                    &[],
                    InstFlags::SIDE_EFFECT,
                );
                new_insts.push(st);
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
                    new_insts.push(ci);
                    let add = func.make_inst(
                        Opcode::Iadd,
                        job.block,
                        smallvec::smallvec![cur, cv],
                        smallvec::smallvec![],
                        &[TypeId::PTR],
                        InstFlags::NONE,
                    );
                    cur = func.dfg.inst_data(add).results[0];
                    new_insts.push(add);
                }
            }
            ops[job.op_idx] = slot;
            if stack_by_val {
                // 指令级 `byval(聚合类型)`：调用点形状据此按**聚合**算（⇒ `StackByVal` 落点），
                // 发射侧再从该指针逐段拷进传出区。参数下标 = 操作数下标 − CallIndirect 的被调方位。
                let arg_index = job.op_idx
                    - usize::from(func.dfg.inst_data(job.inst).opcode == Opcode::CallIndirect);
                let n = func.dfg.inst_data(job.inst).param_attrs.len();
                if n <= arg_index {
                    func.dfg
                        .inst_mut(job.inst)
                        .param_attrs
                        .resize(arg_index + 1, Default::default());
                }
                let mut a = func.dfg.inst_data(job.inst).param_attrs[arg_index].clone();
                a.byval = Some(ty);
                func.dfg.inst_mut(job.inst).param_attrs[arg_index] = a;
            }
        } else {
            ops.remove(job.op_idx);
            for (k, s) in segs.iter().enumerate() {
                ops.insert(job.op_idx + k, *s);
            }
        }
        func.dfg.inst_mut(job.inst).operands = ops;
        // 就地改写操作数后必须重登记 use-lists
        func.refresh_inst_uses(job.inst);
        // 新指令移到 call 前
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
    Ok(())
}
