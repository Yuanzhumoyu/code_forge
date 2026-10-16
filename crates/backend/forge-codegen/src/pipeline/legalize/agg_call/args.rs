//! 从 `agg_call.rs` 拆出（v25 M4：逐字搬移，不改行为）。
use crate::IrError;
use crate::pipeline::agg_expand::Places;
use crate::pipeline::legalize::classify::is_by_ref;
use crate::pipeline::legalize::materialize::ensure_value_addr;
use forge_abi::Placement;
use forge_ir::*;

/// 函数是否含 >8 字节聚合 load（决定是否需要可变副本做大聚合值传播）。：`call @f({i64,i64} %v)` 的
/// 聚合实参（>8 且 ≤16 字节——SysV 整数寄存器路径）拆为 ceil(size/8) 个 i64
/// 段值追加到 call operands（内部签名宽松——callee 签名不校验；lowering 按
/// operands 逐参数占槽——RCX/RDX/R8/R9 + 栈参数）。
/// 段来源：AggConst（字节已知——插 Iconst 段常量）、load 结果（追踪 addr——
/// 插段 load）；其余（块参数/小聚合寄存器值）Unsupported。>16 字节栈传递
/// 未实现（Unsupported）。
/// 必须在 expand_large_aggs **之前**：拆段后大聚合值不再被 call 使用，
/// expand_large_aggs 的使用检测不命中 Call。
/// 从地址 `addr` **逐段 load** `size` 字节（返回 (段值, 生成的指令)）。
///
/// v24 M3：抽出来给两个来源共用——`load` 结果（追踪它的地址）与
/// `insertvalue`/`undef` 链（先由物化相求出槽地址）。两处共用一份实现，
/// "这边支持了、那边还 fail-closed"的分叉就没了。
fn load_segs_from_addr(
    func: &mut Function,
    block: Block,
    addr: Value,
    size: usize,
) -> (Vec<Value>, Vec<Inst>) {
    let mut segs: Vec<Value> = Vec::new();
    let mut insts: Vec<Inst> = Vec::new();
    let mut prev_addr = addr;
    let mut off = 0usize;
    while off < size {
        let ld = func.make_inst(
            Opcode::Load,
            block,
            smallvec::smallvec![prev_addr],
            smallvec::smallvec![],
            &[TypeId::I64],
            InstFlags::NONE,
        );
        let ldv = func.dfg.inst_data(ld).results[0];
        insts.push(ld);
        segs.push(ldv);
        off += 8;
        if off < size {
            let off_cid = func.constants.insert_int(8, 64);
            let off_inst = func.make_inst(
                Opcode::Iconst,
                block,
                smallvec::smallvec![],
                smallvec::smallvec![Immediate::Const(off_cid)],
                &[TypeId::I64],
                InstFlags::NONE,
            );
            let off_v = func.dfg.inst_data(off_inst).results[0];
            insts.push(off_inst);
            let add = func.make_inst(
                Opcode::Iadd,
                block,
                smallvec::smallvec![prev_addr, off_v],
                smallvec::smallvec![],
                &[TypeId::PTR],
                InstFlags::NONE,
            );
            prev_addr = func.dfg.inst_data(add).results[0];
            insts.push(add);
        }
    }
    (segs, insts)
}

pub(crate) fn expand_agg_call_args(
    func: &mut Function,
    edges: &crate::pipeline::legalize::plan::AbiEdges,
    agg_slots: &mut Places,
    shadow_agg: &forge_ir::entity::map::SecondaryMap<Value, TypeId>,
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
        // **只读计划**（v25 M1）：按**值**查（`expand_large_agg_call_results` 可能已经
        // 往实参表前插了 sret 缓冲指针 ⇒ `op_idx` 会漂移，值句柄不会，见 `CallArgEdge`）。
        let Some(loc) = edges.call_arg_of(job.inst, job.val) else {
            return Err(IrError::Internal(format!(
                "call {:?} 的宽聚合实参 {:?} 不在计划里（计划相应已登记）",
                job.inst, job.val
            )));
        };
        let place = loc.place.clone();
        let by_ref = is_by_ref(&place);
        // **按值整块进传出区**（sysv64）：与 by-ref 一样先在调用方本地建副本、传指针；
        // 区别是**写 `byval(聚合类型)` 属性**——调用点形状据此按**聚合**算（⇒ 落点
        // `StackByVal`），发射侧再从该指针逐 8 字节拷进传出区（`arg_bytes_*` 三角色）。
        // 被调方一侧的地址口径已由 lowering 的"正立即数 = ABI 相对"钉住。
        let stack_by_val = matches!(place, Placement::StackByVal { .. });
        if !by_ref && !stack_by_val && size > 16 {
            return Err(crate::IrError::capability(
                "AGG_ARG_PLACEMENT",
                format!(
                    "聚合实参 {size} 字节：约定落点既不是寄存器档也不是按引用档（type {ty:?}）"
                ),
            ));
        }
        // 段值生成
        let mut segs: Vec<Value> = Vec::new();
        let mut new_insts: Vec<Inst> = Vec::new();
        match func.dfg.value_data(job.val).def {
            ValueDef::AggConst(aid) => {
                let agg = func
                    .constants
                    .get_aggregate(aid)
                    .ok_or_else(|| IrError::Internal("聚合实参：常量池缺失 AggId".into()))?;
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
                    .ok_or_else(|| IrError::Internal("聚合实参：load 无地址".into()))?;
                let (s, ins) = load_segs_from_addr(func, job.block, addr, size);
                segs.extend(s);
                new_insts.extend(ins);
            }
            _ => {
                // **v24 M3：`insertvalue`/`undef` 链搭出来的聚合**——沿定义链求地址
                //（复用物化相的 `ensure_value_addr`，递归处理链式构造），拿到槽地址后
                // 与上面的 `Load` 臂走**同一条**逐段 load 路径。
                //
                // 来历：v23 收尾 M7 批次 8 曾写过"只内存化、不动使用处"的助手，能编能跑但
                // **值不对**（`main_ins` 返回 `72198297058681447` 而非 11）⇒ 当时按铁律回退成
                // fail-closed。M3 的修法是**沿定义链递归**：那次错误正是因为**基值自己也是链**
                //（`%t1 = insertvalue undef, …`）而助手只看了一步，把基值字段按 0 处理了。
                // **捕获物化生成的指令**：`ensure_value_addr` 会往块尾追加一串指令
                //（开槽 + 搬基值字节 + 写插入字段），它们**必须**与下面的逐段 load 一起
                // 被搬到调用点之前——否则 load 会排在自己的地址定义之前，又是 use-before-def
                //（实测症状：编译通过、运行 ACCESS_VIOLATION，因为用的地址是垃圾）。
                let tail_before = func.dfg.block(job.block).inst_order.len();
                let addr =
                    ensure_value_addr(func, &store, agg_slots, shadow_agg, job.block, job.val)?;
                let emitted = func.dfg.block(job.block).inst_order[tail_before..].to_vec();
                new_insts.extend(emitted);
                match addr {
                    Some(a) => {
                        let (s, ins) = load_segs_from_addr(func, job.block, a, size);
                        segs.extend(s);
                        new_insts.extend(ins);
                    }
                    None => {
                        return Err(crate::IrError::capability(
                            "AGG_ARG_INSERTVALUE_CHAIN",
                            format!(
                                "聚合实参拆段：值来源既不是 load，也不是可物化的 `insertvalue`/`undef` \
                                 链（def {:?}）",
                                func.dfg.value_data(job.val).def
                            ),
                        ));
                    }
                }
            }
        }
        // 落点落地：ByRef ⇒ 建一个调用方帧内副本、逐段 store，实参传**该副本的指针**
        // （Win64 psABI 的"指向调用方副本的指针"就是这个形状；机器码层与 byval 等价，
        // 但不进生成物的 `Indirect` 那条"从指针 load 出值"的路径）；
        // 其它 ⇒ 在 op_idx 处插入段值。
        let mut ops = func.dfg.inst_data(job.inst).operands.clone();
        if by_ref || stack_by_val {
            // v23 M6：规范 immediate 形态的唯一出口（写 `Const(池 id)` 会让槽不进表 ⇒ 没偏移）。
            let (alloca, slot) =
                crate::pipeline::legalize::common::place_addr_of_slot(func, job.block, size);
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
