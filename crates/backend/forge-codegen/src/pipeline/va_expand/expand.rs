//! 从 `va_expand.rs` 拆出（v25 M4：逐字搬移，不改行为）。

use super::AbiSlots;
use super::slots::int_ty;
use crate::pipeline::agg_expand::{Place, Places};
use forge_ir::*;
use forge_isa_runtime::machine::call_layout::{VaInfo, VaInit, VaInitVal};

/// 把函数里所有 `VaStart`/`VaArg` 展开成显式 IR。
///
/// 需要两样数据齐备：`va.init`（对象每个字段的初值，宿主按 plan 算）与 `va.arg_rules`
/// （取参规则，引擎从形状数据解析）——缺任何一样都**明确报错**（不猜、也不套别的形状的偏移）。
/// `slots` 是管线在编译入口预留的 ABI 槽（保存区 + 对象）。返回 `(物化了几份对象, 取值几条)`。
pub(crate) fn expand_va(
    func: &mut Function,
    va: &VaInfo,
    slots: &AbiSlots,
    agg_slots: &mut Places,
) -> Result<(usize, usize), IrError> {
    let Some(init) = va.init.clone() else {
        return Err(crate::IrError::capability(
            "VA_INIT_MISSING",
            format!(
                "变参：宿主没给 `LowerCtx::conv.va.init`（对象字段初值），无法物化 `va_list`——\
             形状 = {}（保存区 {} / 字段 {} 个）",
                va.shape.as_deref().unwrap_or("<explicit>"),
                if va.save.is_some() { "有" } else { "无" },
                va.fields.len()
            ),
        ));
    };
    // ① 收集（不能边遍历边改）
    let mut starts: Vec<(Block, Inst)> = Vec::new();
    let mut args: Vec<(Block, Inst)> = Vec::new();
    for (bid, bd) in func.dfg.blocks() {
        for &ii in &bd.inst_order {
            match func.dfg.inst_data(ii).opcode {
                Opcode::VaStart => starts.push((bid, ii)),
                Opcode::VaArg => args.push((bid, ii)),
                _ => {}
            }
        }
    }
    let (n_start, n_arg) = (starts.len(), args.len());
    // ② 逐条展开：每条都"现查自己当前下标再插"，顺序无关。
    for (b, ii) in starts.into_iter().rev() {
        expand_start(func, b, ii, &init, va, slots)?;
    }
    for (b, ii) in args.into_iter().rev() {
        expand_arg(func, b, ii, va, agg_slots)?;
    }
    Ok((n_start, n_arg))
}

// ── 生成小工具（**自由函数**、显式传输出向量：闭包会同时可变借用输出）────────────

fn mk_iconst(func: &mut Function, b: Block, out: &mut Vec<Inst>, v: i64) -> Value {
    let cid = func.constants.insert_int(v as i128, 64);
    let ci = func.make_inst(
        Opcode::Iconst,
        b,
        smallvec::smallvec![],
        smallvec::smallvec![Immediate::Const(cid)],
        &[TypeId::I64],
        InstFlags::NONE,
    );
    let r = func.dfg.inst_data(ci).results[0];
    out.push(ci);
    r
}

fn mk_binop(
    func: &mut Function,
    b: Block,
    out: &mut Vec<Inst>,
    op: Opcode,
    a: Value,
    c: Value,
    ty: TypeId,
) -> Value {
    let i = func.make_inst(
        op,
        b,
        smallvec::smallvec![a, c],
        smallvec::smallvec![],
        &[ty],
        InstFlags::NONE,
    );
    let r = func.dfg.inst_data(i).results[0];
    out.push(i);
    r
}

fn mk_load(func: &mut Function, b: Block, out: &mut Vec<Inst>, addr: Value, ty: TypeId) -> Value {
    let i = func.make_inst(
        Opcode::Load,
        b,
        smallvec::smallvec![addr],
        smallvec::smallvec![],
        &[ty],
        InstFlags::NONE,
    );
    let r = func.dfg.inst_data(i).results[0];
    out.push(i);
    r
}

fn mk_fload(func: &mut Function, b: Block, out: &mut Vec<Inst>, addr: Value, ty: TypeId) -> Value {
    let i = func.make_inst(
        Opcode::Fload,
        b,
        smallvec::smallvec![addr],
        smallvec::smallvec![],
        &[ty],
        InstFlags::NONE,
    );
    let r = func.dfg.inst_data(i).results[0];
    out.push(i);
    r
}

fn mk_store(func: &mut Function, b: Block, out: &mut Vec<Inst>, val: Value, addr: Value) {
    let i = func.make_inst(
        Opcode::Store,
        b,
        smallvec::smallvec![val, addr],
        smallvec::smallvec![],
        &[],
        InstFlags::SIDE_EFFECT,
    );
    out.push(i);
}

fn mk_select(
    func: &mut Function,
    b: Block,
    out: &mut Vec<Inst>,
    cond: Value,
    a: Value,
    c: Value,
    ty: TypeId,
) -> Value {
    let i = func.make_inst(
        Opcode::Select,
        b,
        smallvec::smallvec![cond, a, c],
        smallvec::smallvec![],
        &[ty],
        InstFlags::NONE,
    );
    let r = func.dfg.inst_data(i).results[0];
    out.push(i);
    r
}

fn mk_unop(
    func: &mut Function,
    b: Block,
    out: &mut Vec<Inst>,
    op: Opcode,
    v: Value,
    ty: TypeId,
) -> Value {
    let i = func.make_inst(
        op,
        b,
        smallvec::smallvec![v],
        smallvec::smallvec![],
        &[ty],
        InstFlags::NONE,
    );
    let r = func.dfg.inst_data(i).results[0];
    out.push(i);
    r
}

/// **物化 `va_list` 对象**（`VaStart`）：`ap = StackAddr(obj_off)`，随后按 `va.fields` 的
/// 顺序、用**每个字段自己的宽度**把宿主给的初值写进去。
///
/// 值有三种来源（[`VaInitVal`]）：常量 / 帧内地址（`StackAddr`）/ 保存区基址（`StackAddr`）。
/// 最后把原 `VaStart` 改成 `Copy ap`——结果值即对象地址。
fn expand_start(
    func: &mut Function,
    b: Block,
    ii: Inst,
    init: &VaInit,
    va: &VaInfo,
    slots: &AbiSlots,
) -> Result<(), IrError> {
    {
        let inst = func.dfg.inst_data(ii);
        if inst.results.is_empty() {
            return Err(IrError::Internal("va_start：缺结果值（对象地址）".into()));
        }
    }
    if init.fields.len() != va.fields.len() {
        return Err(IrError::Internal(format!(
            "va_start（保存区形态）：`va.init` 给了 {} 个字段初值，但布局有 {} 个字段",
            init.fields.len(),
            va.fields.len()
        )));
    }
    let pos = func
        .dfg
        .block(b)
        .inst_order
        .iter()
        .position(|&x| x == ii)
        .ok_or_else(|| IrError::Internal("va_start：指令不在块序里".into()))?;
    let mut out: Vec<Inst> = Vec::new();
    // ap = StackAddr(-obj_depth)：对象槽（深度由 `plan_abi_slots` 排在前端局部槽之下）
    let ap = {
        let i = func.make_inst(
            Opcode::StackAddr,
            b,
            smallvec::smallvec![],
            smallvec::smallvec![Immediate::Int(-(slots.obj_depth as i64))],
            &[TypeId::PTR],
            InstFlags::NONE,
        );
        let v = func.dfg.inst_data(i).results[0];
        out.push(i);
        v
    };
    // 逐字段写（宽度 = 字段自己的宽度；地址类字段写指针）
    for (f, val) in va.fields.iter().zip(init.fields.iter()) {
        let field_ty = int_ty(f.size.max(1))?;
        let value = match val {
            VaInitVal::Imm(v) => {
                let c = mk_iconst(func, b, &mut out, *v as i64);
                // 常量按字段宽度截断（宽度不足时）
                if f.size.max(1) >= 8 {
                    c
                } else {
                    mk_unop(func, b, &mut out, Opcode::Ireduce, c, field_ty)
                }
            }
            VaInitVal::FrameOff(off) => {
                // 帧基址相对偏移 X ⇒ `StackAddr(X + shift)`（`StackAddr(v)` 的地址 =
                // `fp + v - shift`）；正偏移（调用方的栈实参区）也走这一条。
                let i = func.make_inst(
                    Opcode::StackAddr,
                    b,
                    smallvec::smallvec![],
                    smallvec::smallvec![Immediate::Int(*off + slots.shift)],
                    &[TypeId::PTR],
                    InstFlags::NONE,
                );
                let v = func.dfg.inst_data(i).results[0];
                out.push(i);
                v
            }
            VaInitVal::SaveOff(off) => {
                // 保存区基址（管线分配的那段；"要求连续"时在**帧顶**，否则在前端局部槽之下）
                // + 形状给的静态偏移：AAPCS64 的 `__gr_top`/`__vr_top` 就是"本类区域的顶端"。
                // 基址的 `StackAddr` 立即数由 `plan_abi_slots` 统一给出（`save_base_v`）——
                // 它把"与 shift 有关/无关"这件事收在一处，这里不重算。
                let i = func.make_inst(
                    Opcode::StackAddr,
                    b,
                    smallvec::smallvec![],
                    smallvec::smallvec![Immediate::Int(slots.save_base_v + i64::from(*off))],
                    &[TypeId::PTR],
                    InstFlags::NONE,
                );
                let v = func.dfg.inst_data(i).results[0];
                out.push(i);
                v
            }
        };
        // 地址 = ap + 字段偏移（0 偏移直接用 ap，省一条 add）
        let addr = if f.offset == 0 {
            ap
        } else {
            let off_v = mk_iconst(func, b, &mut out, i64::from(f.offset));
            mk_binop(func, b, &mut out, Opcode::Iadd, ap, off_v, TypeId::PTR)
        };
        mk_store(func, b, &mut out, value, addr);
    }
    {
        let inst = func.dfg.inst_mut(ii);
        inst.opcode = Opcode::Copy;
        inst.operands = smallvec::smallvec![ap];
        inst.immediates = smallvec::smallvec![];
    }
    func.refresh_inst_uses(ii);
    move_new_before(func, b, pos, &out)?;
    Ok(())
}

/// 把新指令按 `new_insts` 的顺序插到块序的 `pos` 之前。
///
/// **不依赖任何位置假设**：不猜"`make_inst` 落在块尾"、也不用"弹 N 次 / 摘 N 条"的写法
/// （本轮实测这两种都被顺序细节坑过）。直接**重建块序**：遍历原序，在 `pos` 处插入
/// `new_insts`，并跳过原序里属于 `new_insts` 的项（它们已被 `make_inst` 放进去了，只保留一次）。
fn move_new_before(
    func: &mut Function,
    b: Block,
    pos: usize,
    new_insts: &[Inst],
) -> Result<(), IrError> {
    if new_insts.is_empty() {
        return Ok(());
    }
    let order = &mut func.dfg.block_mut(b).inst_order;
    let old: Vec<Inst> = order.clone();
    let mut rebuilt: Vec<Inst> = Vec::with_capacity(old.len() + new_insts.len());
    for (k, &x) in old.iter().enumerate() {
        if k == pos {
            rebuilt.extend_from_slice(new_insts);
        }
        if !new_insts.contains(&x) {
            rebuilt.push(x);
        }
    }
    if pos >= old.len() {
        rebuilt.extend_from_slice(new_insts);
    }
    *order = rebuilt;
    Ok(())
}

/// 一条 `va_arg` 的现场：`va_list` 指针操作数 + 结果类型判据。
struct VaArgSite {
    ap: Value,
    ty: TypeId,
    is_fp: bool,
    is_f32: bool,
    /// 结果类型是否为**聚合**（走 `arg_rules.aggregate` 那条路；没声明那条规则 ⇒ fail-closed）。
    is_agg: bool,
}

/// **`va_arg` 的展开**（v20 V6，**唯一实现**）：按 plan 的取参规则跑同一套算法，
/// 对"这是哪份约定"一无所知。
///
/// ```text
/// cur    = load <游标字段宽> [ap + 游标偏移]         ; 宽度 = 字段自己的宽度；往下数式按有符号扩展
/// addr   = base ? load [ap + 基址偏移] + cur : cur   ; 没有基址 ⇒ 游标本身就是要取的地址
/// 有溢出支时：
///   in_reg = icmp <slt|ult> cur, limit               ; 未超上限 ⇒ 实参还在保存区里
///   ov     = load [ap + 溢出字段偏移]
///   p      = select in_reg, addr, ov
///   cur'   = select in_reg, cur + step, cur          ; 保存区步长
///   ov'    = select in_reg, ov, ov + 溢出步长
/// 没有溢出支时：
///   p = addr ; cur' = cur + step
/// val    = load/fload <结果类型> [p]                 ; f32 先取 f64 再窄回（默认提升）
/// 写回游标（按字段宽度截断）与（有溢出支时的）溢出指针
/// ```
///
/// 四份内置约定落在这套数据上（见 `forge_abi::rules::preset_va_shape`）：SysV 是"无符号偏移
/// 游标 + 溢出区"、Win64 是"地址式单游标"、AAPCS64 是"有符号计数 + 溢出区（基准 = 区域顶端）"、
/// RISC-V LP64D 是"地址式游标 + 保存区"。**任何一份约定都不在这里出现名字**。
pub(super) fn expand_arg(
    func: &mut Function,
    b: Block,
    ii: Inst,
    va: &VaInfo,
    agg_slots: &mut Places,
) -> Result<(), IrError> {
    let site = va_arg_site(func, ii)?;
    let (ap, ty, is_fp, is_f32, is_agg) = (site.ap, site.ty, site.is_fp, site.is_f32, site.is_agg);
    // 聚合走**自己的那条规则**（v23 收尾）：它在区域里是"指向聚合的指针"还是内联字节，
    // 不是 int/fp 两类能表达的。没声明 ⇒ fail-closed（不猜、也不按标量 load）。
    let agg_rule = if is_agg {
        Some(va.arg_rules.aggregate.ok_or_else(|| {
            crate::IrError::capability(
                "VA_ARG_AGG_RULE",
                "va_arg：本约定没有声明聚合取参规则（`va_list` 形状的 `aggregate_arg`）——\
                 调用方可按标量逐个 va_arg，或先在约定层补这条规则",
            )
        })?)
    } else {
        None
    };
    let rule = match agg_rule {
        Some(a) => a.rule,
        None if is_fp => va.arg_rules.float,
        None => va.arg_rules.int,
    };
    let field = |i: usize| -> Result<(u32, u32), IrError> {
        va.fields
            .get(i)
            .map(|f| (f.offset, f.size.max(1)))
            .ok_or_else(|| {
                // 宿主给的 plan 里缺了取参规则引用的字段 ⇒ **输入/数据不一致**，不是能力缺口
                //（v25 M2 修正：这里以前借用了向量缺口的 ID，让台账里那条的语义被稀释）。
                IrError::Internal(format!(
                    "va_arg：plan 的 `va.fields` 缺第 {} 个字段（取参规则引用了它）",
                    i + 1
                ))
            })
    };
    let (cur_off, cur_size) = field(rule.cursor)?;
    let pos = func
        .dfg
        .block(b)
        .inst_order
        .iter()
        .position(|&x| x == ii)
        .ok_or_else(|| IrError::Internal("va_arg：指令不在块序里".into()))?;

    let mut out: Vec<Inst> = Vec::new();
    // ① 游标（宽度 = 字段自己的宽度；往下数式是负值 ⇒ 按**有符号**扩展）
    let cur_off_v = mk_iconst(func, b, &mut out, i64::from(cur_off));
    let cur_addr = mk_binop(func, b, &mut out, Opcode::Iadd, ap, cur_off_v, TypeId::PTR);
    let cur_raw = mk_load(func, b, &mut out, cur_addr, int_ty(cur_size)?);
    let cur = if cur_size >= 8 {
        cur_raw
    } else {
        let op = if rule.cursor_counts_down {
            Opcode::Sextend
        } else {
            Opcode::Uextend
        };
        mk_unop(func, b, &mut out, op, cur_raw, TypeId::I64)
    };
    // ② 保存区里的地址：有基址 ⇒ 基址值 + 游标；否则游标本身就是地址
    let addr = match rule.base {
        Some(bi) => {
            let (base_off, _) = field(bi)?;
            let off_v = mk_iconst(func, b, &mut out, i64::from(base_off));
            let base_addr = mk_binop(func, b, &mut out, Opcode::Iadd, ap, off_v, TypeId::PTR);
            let base = mk_load(func, b, &mut out, base_addr, TypeId::PTR);
            mk_binop(func, b, &mut out, Opcode::Iadd, base, cur, TypeId::PTR)
        }
        None => cur,
    };
    // ③ 取哪个地址（保存区里 / 溢出区）、两条游标各自怎么推进
    let p = match rule.overflow {
        Some(oi) => {
            let (ov_off, _) = field(oi)?;
            let lim_v = mk_iconst(func, b, &mut out, rule.limit as i64);
            let cc = if rule.signed_limit {
                IntCC::Slt
            } else {
                IntCC::Ult
            };
            let in_reg = {
                let i = func.make_inst(
                    Opcode::Icmp,
                    b,
                    smallvec::smallvec![cur, lim_v],
                    smallvec::smallvec![Immediate::IntCC(cc)],
                    &[TypeId::I8],
                    InstFlags::NONE,
                );
                let r = func.dfg.inst_data(i).results[0];
                out.push(i);
                r
            };
            let off_v = mk_iconst(func, b, &mut out, i64::from(ov_off));
            let ov_addr = mk_binop(func, b, &mut out, Opcode::Iadd, ap, off_v, TypeId::PTR);
            let ov = mk_load(func, b, &mut out, ov_addr, TypeId::PTR);
            let p = mk_select(func, b, &mut out, in_reg, addr, ov, TypeId::PTR);
            // 推进：在保存区里就推游标，否则推溢出指针（各自**只推一边**）
            let step_v = mk_iconst(func, b, &mut out, i64::from(rule.step));
            let cur_next = mk_binop(func, b, &mut out, Opcode::Iadd, cur, step_v, TypeId::I64);
            let cur_out = mk_select(func, b, &mut out, in_reg, cur_next, cur, TypeId::I64);
            let ov_step_v = mk_iconst(func, b, &mut out, i64::from(rule.overflow_step));
            let ov_next = mk_binop(func, b, &mut out, Opcode::Iadd, ov, ov_step_v, TypeId::PTR);
            let ov_out = mk_select(func, b, &mut out, in_reg, ov, ov_next, TypeId::PTR);
            store_cursor(func, b, &mut out, cur_out, cur_size, cur_off, ap)?;
            store_at(func, b, &mut out, ov_out, ov_off, ap);
            p
        }
        None => {
            let step_v = mk_iconst(func, b, &mut out, i64::from(rule.step));
            let cur_next = mk_binop(func, b, &mut out, Opcode::Iadd, cur, step_v, TypeId::I64);
            store_cursor(func, b, &mut out, cur_next, cur_size, cur_off, ap)?;
            addr
        }
    };
    // ④ 取值：聚合 → 拷贝进自己的帧槽并登记（`agg_slots`）；f32 先取 f64 再窄回；
    //    f64 走 Fload；整数/指针走 Load。
    if let Some(a) = agg_rule {
        let size = {
            let ts = func.types.borrow();
            ts.size_bytes(ty) as usize
        };
        // 形态**按尺寸分档**（win64：`>8B ⇒ 指针`、`≤8B ⇒ 内联整数槽`）：
        //   `use_pointer = indirect && (阈值 == 0 || size > 阈值)`
        // 阈值 0 = 所有尺寸同一形态；阈值 > 0 = 只有更大的那档才是指针。
        // 内联档就是"区域里直接放聚合的字节"（≤8B 一个槽、>8B 连续多槽），
        // 逐 8 字节段拷进来即可——**不要**在这里多读一次指针（那是把内联字节当指针）。
        let use_pointer = a.indirect
            && (a.indirect_size_gt == 0 || (size as u64) > u64::from(a.indirect_size_gt));
        // 区域里放的是**指针**时先取指针（win64 的 ">8B ⇒ 调用方副本 + 指针"）。
        let src = if use_pointer {
            mk_load(func, b, &mut out, p, TypeId::PTR)
        } else {
            p
        };
        // 逐 8 字节段从来源拷进槽（`size` 已按契约：调用方给的是完整聚合字节）。
        let (alloca, slot) = crate::pipeline::legalize::common::place_addr_of_slot(func, b, size);
        out.push(alloca);
        let mut cur_dst = slot;
        let mut cur_src = src;
        let mut off = 0usize;
        while off < size {
            let seg = mk_load(func, b, &mut out, cur_src, TypeId::I64);
            mk_store(func, b, &mut out, seg, cur_dst);
            off += 8;
            if off < size {
                let c8 = mk_iconst(func, b, &mut out, 8);
                cur_dst = mk_binop(func, b, &mut out, Opcode::Iadd, cur_dst, c8, TypeId::PTR);
                cur_src = mk_binop(func, b, &mut out, Opcode::Iadd, cur_src, c8, TypeId::PTR);
            }
        }
        // 结果值登记成"住在该槽"，原指令墓碑化，再递归重写它的后续使用
        // （后续 `extractvalue` 因此走地址语义——与聚合展开族同一套机制）。
        let res = func.dfg.inst_data(ii).results[0];
        func.tombstone_inst(ii);
        agg_slots.set(res, Place::Mem(slot));
        let _ = ty;
        move_new_before(func, b, pos, &out)?;
    }
    let val = if is_f32 {
        let d = mk_fload(func, b, &mut out, p, TypeId::F64);
        mk_unop(func, b, &mut out, Opcode::Fptrunc, d, ty)
    } else if is_fp {
        mk_fload(func, b, &mut out, p, ty)
    } else {
        mk_load(func, b, &mut out, p, ty)
    };
    // 原指令改 Copy（结果值 = 新取到的值）
    {
        let inst = func.dfg.inst_mut(ii);
        inst.opcode = Opcode::Copy;
        inst.operands = smallvec::smallvec![val];
        inst.immediates = smallvec::smallvec![];
    }
    func.refresh_inst_uses(ii);
    move_new_before(func, b, pos, &out)?;
    Ok(())
}

/// 把 64 位游标按**字段宽度**截断后写回 `[ap + off]`。
fn store_cursor(
    func: &mut Function,
    b: Block,
    out: &mut Vec<Inst>,
    cur: Value,
    cur_size: u32,
    off: u32,
    ap: Value,
) -> Result<(), IrError> {
    let stored = if cur_size >= 8 {
        cur
    } else {
        mk_unop(func, b, out, Opcode::Ireduce, cur, int_ty(cur_size)?)
    };
    store_at(func, b, out, stored, off, ap);
    Ok(())
}

/// `[ap + off] = val`（偏移 0 时直接用 `ap`，省一条 add）。
fn store_at(func: &mut Function, b: Block, out: &mut Vec<Inst>, val: Value, off: u32, ap: Value) {
    let addr = if off == 0 {
        ap
    } else {
        let off_v = mk_iconst(func, b, out, i64::from(off));
        mk_binop(func, b, out, Opcode::Iadd, ap, off_v, TypeId::PTR)
    };
    mk_store(func, b, out, val, addr);
}

/// `va_arg` 的现场与**按结果类型的判据**（四份约定共用，缺一不可）：
///
/// - 向量 ⇒ `Unsupported`：要"mem → 向量寄存器"的取值能力（尚未申报），不能拿 GPR 取值糊过去；
/// - 标量浮点 ⇒ 只支持 `f64` 与 `f32`：`f32` 走 C/LLVM 的**默认实参提升**（调用方传 `f64`，
///   读取方取 `f64` 再窄回），更窄的浮点还没有能力声明。
fn va_arg_site(func: &Function, ii: Inst) -> Result<VaArgSite, IrError> {
    let (ap, res) = {
        let inst = func.dfg.inst_data(ii);
        let ap = *inst
            .operands
            .first()
            .ok_or_else(|| IrError::Internal("va_arg：缺 va_list 指针操作数".into()))?;
        let res = *inst
            .results
            .first()
            .ok_or_else(|| IrError::Internal("va_arg：缺结果值".into()))?;
        (ap, res)
    };
    let ty = func
        .dfg
        .value_type(res)
        .ok_or_else(|| IrError::Internal("va_arg：结果值没有类型".into()))?;
    let (is_fp, is_f32, bits, is_vec, is_agg) = {
        let ts = func.types.borrow();
        (
            ts.is_float(ty),
            ts.is_float(ty) && ts.size_bytes(ty) == 4,
            ts.size_bytes(ty) * 8,
            ts.is_vector(ty) || ts.is_scalable_vector(ty),
            ts.is_aggregate(ty),
        )
    };
    // 聚合不再无条件拒绝：`expand_arg` 会去查 `arg_rules.aggregate`，**没声明**那条规则才
    // fail-closed（v23 收尾：数据面已支持，win64 已声明"间接"形态）。
    if is_vec {
        return Err(IrError::capability(
            "VA_ARG_VECTOR_MEM",
            "va_arg: 向量结果需要「mem → 向量寄存器」的取值能力（尚未申报）",
        ));
    }
    if is_fp && bits != 32 && bits != 64 {
        return Err(crate::IrError::capability(
            "VA_ARG_FLOAT_WIDTH",
            "va_arg: 只支持 f64 与 f32（f32 走默认提升的窄回）；更窄的浮点还没有能力声明",
        ));
    }
    Ok(VaArgSite {
        ap,
        ty,
        is_fp,
        is_f32,
        is_agg,
    })
}
