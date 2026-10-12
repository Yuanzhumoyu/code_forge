//! 聚合 ABI 展开(Stage 0.5)——从 compiler.rs 分批迁出(第四十一轮)。
//!
//! 聚合参数/返回/存储/call 结果 >8 字节的 IR 层展开:
//!
//! - 段值拆分(≤16 字节:寄存器路径;>16 字节 Unsupported)
//! - **触发判据只有一处**（[`needs_agg_expand`]）：保守的"提没提到聚合"，只用于决定
//!   要不要克隆 IR——`v23 M3` 之前这里是 7 个各扫一种模式的 `has_*`，
//!   漏一个就**静默少跑一个 pass**（历史事故：`has_large_agg_ret` 曾缺失 ⇒ 大聚合返回
//!   的纯 call+extractvalue 函数展开不到 ⇒ 5 vs 6 错值）。
//! - 每个 pass **自报改动了什么**（[`ChangeSet`]），不再靠预判"可能要有改动"。
//!
//! 依赖约定:仅依赖 forge_ir 与 crate::pipeline::agg_const;
//! 不依赖 CompileState(展开在 CompileState::new 之前执行——常量池克隆时序)。

use forge_ir::entity::map::SecondaryMap;
use forge_ir::*;

/// 一次聚合展开**改动了什么**（v23 M3：由展开驱动自报，取代 7 个 `has_*` 预判）。
///
/// 口径 = **IR 的变化量**（新指令/新值/新常量）加"sret 形参是否立起来"。
/// 为什么用变化量而不是让 8 个 pass 各自返回计数：变化量是**完整**的（新增的 pass
/// 自动被覆盖，不会像 `has_*` 那样"新加一类模式却忘了加判据"），而且它就是我们要
/// 断言的那件事——"含聚合的函数必须真的改了东西"。
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub(crate) struct ChangeSet {
    /// 新增指令数（墓碑化/改写不计，只有真的插入了新指令才算）。
    pub new_insts: usize,
    /// 新增值数。
    pub new_values: usize,
    /// 新增常量数（常量池是展开的产物之一：打包出的 i64 段值）。
    pub new_consts: usize,
    /// 是否立起了 sret 形参（`expand_sret_return` 的产物）。
    pub sret: bool,
}

impl ChangeSet {
    /// 改动总数（含 sret 的那 1 次）。
    pub fn total(&self) -> usize {
        self.new_insts + self.new_values + self.new_consts + usize::from(self.sret)
    }
}

/// IR 的规模快照（`ChangeSet` 的差分量）。
#[derive(Debug, Clone, Copy)]
pub(crate) struct IrSize {
    pub insts: usize,
    pub values: usize,
    pub consts: usize,
}

impl IrSize {
    /// 取当前规模。
    pub fn of(func: &Function) -> Self {
        Self {
            insts: func.dfg.insts().count(),
            values: func.dfg.values().count(),
            consts: func.constants.len(),
        }
    }

    /// 与 `before` 的差 + sret 标记 → [`ChangeSet`]。
    pub fn delta_to(self, before: Self, sret: bool) -> ChangeSet {
        ChangeSet {
            new_insts: self.insts.saturating_sub(before.insts),
            new_values: self.values.saturating_sub(before.values),
            new_consts: self.consts.saturating_sub(before.consts),
            sret,
        }
    }
}

/// **是否需要可变副本**（Stage 0.5 展开的克隆判据）——**唯一**的预判。
///
/// 判据 = 含 GEP **或** 提到过任何聚合类型/聚合字面量。**保守**（宁可多克隆一次，
/// 不可漏判）：漏判的后果不是慢，而是**该展开的没展开**（静默错值）；多判的后果
/// 只是克隆一份 `Function`。
///
/// `v23 M3` 之前这里是 7 个各扫一种模式的 `has_*`（`has_large_agg_ret` 等），
/// 每个都可能漏——历史事故就是这么来的。现在只有这一处，且它是**过近似**的。
pub(crate) fn needs_agg_expand(func: &Function) -> bool {
    has_gep(func) || mentions_aggregate(func)
}

/// 函数是否含 GEP（GEP 的源元素类型不在 IR 值类型里，必须单独扫）。
fn has_gep(func: &Function) -> bool {
    for (_, bd) in func.dfg.blocks() {
        for &ii in &bd.inst_order {
            if func.dfg.inst_data(ii).opcode == Opcode::GetElementPtr {
                return true;
            }
        }
    }
    false
}

/// 函数**提到过**聚合吗（形参/返回/指令操作数与结果类型，或聚合字面量）。
///
/// 过近似：任何一处聚合类型出现即真——包括"其实不需要展开"的情形（如纯 `insertvalue`
/// 链）。代价只是一次克隆。
fn mentions_aggregate(func: &Function) -> bool {
    let ts = func.types.borrow();
    if func
        .param_types()
        .iter()
        .chain(func.return_types().iter())
        .any(|t| ts.is_aggregate(*t))
    {
        return true;
    }
    for (_, bd) in func.dfg.blocks() {
        for &ii in &bd.inst_order {
            let inst = func.dfg.inst_data(ii);
            let tys = inst
                .results
                .iter()
                .chain(inst.operands.iter())
                .filter_map(|v| func.dfg.value_type(*v));
            for ty in tys {
                if ts.is_aggregate(ty) {
                    return true;
                }
            }
            if inst
                .operands
                .iter()
                .any(|v| matches!(func.dfg.value_data(*v).def, ValueDef::AggConst(_)))
            {
                return true;
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
    // **immediate 必须是规范形态** `Type(elem) + Uint(count)`：帧槽预扫描
    //（`frame_slots.rs`）只按这两个 immediate 算字节数，写 `Const(池 id)` 会让这个
    // Alloca **不进槽表** ⇒ 没有帧偏移 ⇒ 地址是垃圾（实测：往那儿写 24 字节会毁帧崩溃）。
    let alloca = func.make_inst(
        Opcode::Alloca,
        use_pos.block,
        smallvec::smallvec![],
        smallvec::smallvec![Immediate::Type(TypeId::I8), Immediate::Uint(size as u64)],
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
    // by-ref 档（值在内存）的字节数：**入口取一次锁**，使用点循环里不再逐次取
    //（v3 S3 读路径纪律；`read_path_budget` 守卫钉住处数）。
    let mem_size: Option<usize> = agg_slots
        .get(old_v)
        .map(|&(_, t)| func.types.borrow().size_bytes(t) as usize);
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
                let dst = inst.operands[1];
                // **按地址源**（by-ref 档：聚合住在指针指向的内存）——逐 8 字节从源拷到目标。
                // 必须先查 `agg_slots`：只按 `segs` 走会在"聚合在内存、段值列表为空"时
                // **一条 store 都不生成**（静默丢数据）。
                if let Some(&(src, _slot_ty)) = agg_slots.get(inst.operands[0]) {
                    // 字节数在入口已算好（无需再取锁）；登记过就一定算得出来
                    let size = mem_size.ok_or_else(|| {
                        IrError::Unsupported("聚合 store：by-ref 值缺字节数（入口未登记）".into())
                    })?;
                    let mut local_gen: Vec<Inst> = Vec::new();
                    let mut cur_src = src;
                    let mut cur_dst = dst;
                    let mut off = 0usize;
                    while off < size {
                        let ld = func.make_inst(
                            Opcode::Load,
                            use_pos.block,
                            smallvec::smallvec![cur_src],
                            smallvec::smallvec![],
                            &[TypeId::I64],
                            InstFlags::NONE,
                        );
                        let lv = func.dfg.inst_data(ld).results[0];
                        local_gen.push(ld);
                        let st = func.make_inst(
                            Opcode::Store,
                            use_pos.block,
                            smallvec::smallvec![lv, cur_dst],
                            smallvec::smallvec![],
                            &[],
                            InstFlags::SIDE_EFFECT,
                        );
                        local_gen.push(st);
                        off += 8;
                        if off < size {
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
                            let sa = func.make_inst(
                                Opcode::Iadd,
                                use_pos.block,
                                smallvec::smallvec![cur_src, cv],
                                smallvec::smallvec![],
                                &[TypeId::PTR],
                                InstFlags::NONE,
                            );
                            cur_src = func.dfg.inst_data(sa).results[0];
                            local_gen.push(sa);
                            let cid2 = func.constants.insert_int(8, 64);
                            let ci2 = func.make_inst(
                                Opcode::Iconst,
                                use_pos.block,
                                smallvec::smallvec![],
                                smallvec::smallvec![Immediate::Const(cid2)],
                                &[TypeId::I64],
                                InstFlags::NONE,
                            );
                            let cv2 = func.dfg.inst_data(ci2).results[0];
                            local_gen.push(ci2);
                            let da = func.make_inst(
                                Opcode::Iadd,
                                use_pos.block,
                                smallvec::smallvec![cur_dst, cv2],
                                smallvec::smallvec![],
                                &[TypeId::PTR],
                                InstFlags::NONE,
                            );
                            cur_dst = func.dfg.inst_data(da).results[0];
                            local_gen.push(da);
                        }
                    }
                    let __tomb = use_pos.inst;
                    func.tombstone_inst(__tomb);
                    move_to_front(func, use_pos.block, use_pos.pos, &local_gen);
                    continue;
                }
                // **段值源**：既没有地址也没有段值 ⇒ 明确报错（不静默丢）
                if segs.is_empty() {
                    return Err(IrError::Unsupported(
                        "聚合 store：值既不在 slot 映射里、也没有段值（by-ref 档漏登记）".into(),
                    ));
                }
                let mut prev_addr = dst;
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
                // **按地址源**（by-ref 档）：把该实参换成"指向聚合副本的指针"。两端形状都是
                // `ptr` ⇒ 落点都是 `Reg`（**不**走 byval/Indirect 那条"从指针 load 出值"的
                // 向量路径——那条要求值的寄存器表示，聚合没有）。
                if let Some(&(addr, _ty)) = agg_slots.get(old_v) {
                    let mut ops = func.dfg.inst_data(use_pos.inst).operands.clone();
                    ops[op_idx] = addr;
                    func.dfg.inst_mut(use_pos.inst).operands = ops;
                    func.refresh_inst_uses(use_pos.inst);
                    continue;
                }
                if segs.is_empty() {
                    return Err(IrError::Unsupported(
                        "call 聚合实参：值既不在 slot 映射里、也没有段值".into(),
                    ));
                }
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
            // by-ref 档（值在内存）按值返回：**这里不动**——`expand_large_agg_ret` 会按约定
            // 把段值写进 sret 缓冲（或拆成多返回值）。此刻值可能刚被换掉，终结符里的引用
            // 由那一趟统一替换，之后再校验 use-def。
            if agg_slots.get(old_v).is_some() {
                continue;
            }
            if segs.is_empty() {
                return Err(IrError::Unsupported(
                    "按值返回聚合：值既不在 slot 映射里、也没有段值".into(),
                ));
            }
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

#[cfg(test)]
mod tests {
    use forge_ir::ir::builder::FunctionBuilder;
    use forge_ir::ir::types::{FunctionSignature, TypeField};
    use forge_ir::{TypeContext, TypeId};

    /// **C2 回归判据**（v23 M3）：克隆判据必须**保守**——只要函数里出现过聚合，
    /// 就必须说"要展开"（宁可多克隆一次，不可漏跑一个 pass）。
    ///
    /// 背景：这里是 7 个各扫一种模式的 `has_*`（`has_large_agg_ret` 等），
    /// 漏一个模式 ⇒ 那个 pass 静默不跑（历史事故：大聚合返回的纯 call+extractvalue
    /// 函数展开不到 ⇒ 5 vs 6 错值）。
    #[test]
    fn needs_agg_expand_is_conservative() {
        // ① 聚合形参（>8B）⇒ 真。
        let ctx = TypeContext::new();
        let agg = ctx.borrow_mut().struct_named(
            "Agg2",
            vec![TypeField::new(TypeId::I64), TypeField::new(TypeId::I64)],
            false,
        );
        let sig = FunctionSignature::new(&[(agg, "a")], &[TypeId::I64]);
        let mut b = FunctionBuilder::new("agg_param", ctx, sig);
        let (e, params) = b.create_block_with_params(&[(agg, "a")]);
        b.switch_to_block(e);
        let one = b.iconst(1, TypeId::I64);
        let _ = params;
        b.ret(&[one]);
        let f = b.finish().expect("agg_param");
        assert!(
            super::needs_agg_expand(&f),
            "聚合形参必须触发展开（宁多克隆，不可漏跑）"
        );

        // ② 只有标量的函数 ⇒ 假（不白克隆）。
        let ctx = TypeContext::new();
        let sig = FunctionSignature::new(&[(TypeId::I64, "x")], &[TypeId::I64]);
        let mut b = FunctionBuilder::new("scalar", ctx, sig);
        let (e, params) = b.create_block_with_params(&[(TypeId::I64, "x")]);
        b.switch_to_block(e);
        let one = b.iconst(1, TypeId::I64);
        let sum = b.iadd(params[0], one);
        b.ret(&[sum]);
        let f = b.finish().expect("scalar");
        assert!(
            !super::needs_agg_expand(&f),
            "纯标量函数不该触发聚合展开（否则每个函数都白克隆一份 IR）"
        );
    }

    /// `ChangeSet` 的差分量：只算**新增**，减少（墓碑化/删除）不计为改动。
    #[test]
    fn change_set_counts_only_growth() {
        let before = super::IrSize {
            insts: 10,
            values: 20,
            consts: 3,
        };
        let after = super::IrSize {
            insts: 14,
            values: 20,
            consts: 5,
        };
        let cs = after.delta_to(before, true);
        assert_eq!((cs.new_insts, cs.new_values, cs.new_consts), (4, 0, 2));
        assert!(cs.sret);
        assert_eq!(cs.total(), 7, "总数 = 3 项增量 + sret 的 1");
    }
}
