//! **保存区形态的 `VaArg` IR 展开**（v20 变参 V3 的最后一环）。
//!
//! ## 为什么在 IR 层展开，而不是生成器里发序列
//!
//! 保存区形态的取值是**有条件的**：`gp_offset` 没超过寄存器区上限就从保存区取、否则从溢出区取
//! （还要各自推进游标）。生成器要发这段就得有"比较 / 条件选择 / 寄存器间加 / 掩码"四个能力，
//! 而 x86 谱里对应指令都没有角色——为一条 op 加四个能力，且别的 ISA 还得各来一遍。
//! IR 层则**全都有现成降级**：`Icmp`、`Select`、`Iadd`、`Uextend`、`Ireduce`、`Load`/`Store`
//! （宽度按 IR 类型）、`Fload`/`Fptrunc`（`f32` 的默认提升就是"取 `f64` 再窄回"）。
//!
//! ## 展开形态（sysv64 一族，整数类）
//!
//! ```text
//! gp      = load u32 [ap]                    ; gp_offset
//! in_reg  = icmp ult gp, gp_limit
//! p_reg   = reg_save + uextend(gp)
//! p_src   = select in_reg, p_reg, overflow
//! v       = load <ty> [p_src]
//! gp_out  = select in_reg, ireduce(gp + gp_step), gp
//! ov_out  = select in_reg, overflow, overflow + slot_bytes
//! store u32 [ap]   = gp_out
//! store i64 [ap+8] = ov_out
//! ```
//!
//! 浮点类把 `gp_offset` 换成 `fp_offset`（字段偏移 4、上限 `fp_limit`、步长 `fp_step`），
//! 取值走 `Fload`；`f32` 结果按默认提升**先取 `f64` 再 `Fptrunc`**。
//!
//! 展开完把原 `VaArg` 指令改成 `Copy <v>`（结果值原样保留 ⇒ 后续使用无需重写）——与
//! `compiler.rs` 的聚合展开同一套写法。
//!
//! ## 两个坑（照抄既有展开的处理方式）
//!
//! 1. `Dfg::make_inst` 只**建**指令，新指令落在块尾——必须显式移到 `VaArg` 之前
//!    （否则"原指令改 `Copy`"会读到尚未计算的值）；
//! 2. 逐指令改写要**先收集再改**两遍（遍历 `blocks()` 时不能再可变借用 `func`）。

use forge_ir::*;
use forge_isa_runtime::machine::call_layout::VaInfo;

/// **ABI 槽的帧内分配**（v20 变参 V3）：保存区与 `va_list` 对象各一段。
///
/// 关键点（2026-10-01 实测踩到）：**必须排在前端局部槽之下**。前端的 `stack_addr(-N)` 是在
/// lowering 里才被 `max_stack_bytes` 统计的，而本展开跑在 lowering **之前**——若在编译入口按
/// "当时的 max"（= 0）预留，保存区就会与前端的 `stack_addr(-16)` 之类**重叠**（sysv64 那条
/// 用例恰好没有局部槽所以没暴露）。所以这里先**扫一遍 IR** 算前端局部槽的最大深度
/// （与 lowering 同一算式 `-v + slot_bytes`），ABI 槽再从它之后往上排。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct AbiSlots {
    /// 保存区深度（正值；`StackAddr` 立即数用 `-depth`）。
    pub save_depth: u32,
    /// `va_list` 对象深度（正值）。
    pub obj_depth: u32,
    /// 保存区相对**帧基址**的偏移（序言 spill 用；`-depth - stack_slot_shift`）。
    pub save_off: i64,
    /// 帧基址 → 局部槽区起点的平移：`StackAddr(v)` 的地址 = `fp + v - shift`，
    /// 所以"帧基址相对偏移 X"要发 `StackAddr(X + shift)`。
    pub shift: i64,
    /// ABI 槽占用的总深度（喂给 `CompileState`，让帧尺寸覆盖到）。
    pub max_depth: u32,
}

/// 扫一遍 IR 算前端局部槽的最大深度（与 `pipeline/lowering.rs` 的 `StackAddr` 处理同算式）。
fn frontend_locals_depth(func: &Function, slot_bytes: u32) -> u32 {
    let unit = i64::from(slot_bytes.max(1));
    let mut stackaddr_depth: i64 = 0;
    // ② `Iadd(StackAddr, Iconst<0)`：mini_c 的 `alloc_slot` 把真偏移放在 iconst 里。
    for (_, bd) in func.dfg.blocks() {
        for &ii in &bd.inst_order {
            let inst = func.dfg.inst_data(ii);
            if inst.opcode != Opcode::Iadd {
                continue;
            }
            let (mut has_stack, mut has_const, mut cval) = (false, false, 0i64);
            for &op in &inst.operands {
                let Some(val) = func.dfg.value_data_opt(op) else {
                    continue;
                };
                let ValueDef::Inst(def_ii, _) = val.def else {
                    continue;
                };
                let def = func.dfg.inst_data(def_ii);
                match def.opcode {
                    Opcode::StackAddr => has_stack = true,
                    Opcode::Iconst => {
                        if let Some(Immediate::Const(cid)) = def.immediates.first()
                            && let Some((v, _)) = func.constants.get_int(*cid)
                        {
                            cval = v as i64;
                            has_const = true;
                        }
                    }
                    _ => {}
                }
            }
            if has_stack && has_const && cval < 0 {
                stackaddr_depth = stackaddr_depth.max(-cval + unit);
            }
        }
    }
    // ① `StackAddr` 立即数 + ③ `Alloca`（从"StackAddr 区底 + 一槽"起连续向下）
    let mut allocas: Vec<u32> = Vec::new();
    for (_, bd) in func.dfg.blocks() {
        for &ii in &bd.inst_order {
            let inst = func.dfg.inst_data(ii);
            match inst.opcode {
                Opcode::StackAddr => {
                    if let Some(Immediate::Int(v)) = inst.immediates.first()
                        && *v < 0
                    {
                        stackaddr_depth = stackaddr_depth.max(-*v + unit);
                    }
                }
                Opcode::Alloca => {
                    let mut ty = None;
                    let mut count = 1u64;
                    for imm in &inst.immediates {
                        match imm {
                            Immediate::Type(t) => ty = Some(*t),
                            Immediate::Uint(c) => count = (*c).max(1),
                            _ => {}
                        }
                    }
                    if let Some(t) = ty {
                        let size = func.types.borrow().size_bytes(t).max(1) as u64;
                        allocas.push((size * count).min(u32::MAX as u64) as u32);
                    }
                }
                _ => {}
            }
        }
    }
    let mut slot = -(stackaddr_depth + unit);
    for bytes in allocas {
        slot -= ((i64::from(bytes) + unit - 1) / unit) * unit;
    }
    (-slot).max(stackaddr_depth).max(0) as u32
}

/// 算 ABI 槽（保存区 → 对象，依次排在前端局部槽之下）。
pub(crate) fn plan_abi_slots(
    func: &Function,
    va: &VaInfo,
    slot_bytes: u32,
    shift: i64,
) -> AbiSlots {
    let align_up = |v: u32, a: u32| {
        let a = a.max(1);
        v.div_ceil(a) * a
    };
    let mut depth = frontend_locals_depth(func, slot_bytes);
    let mut save_depth = depth;
    if let Some(save) = va.save.as_ref() {
        depth = align_up(depth, save.align) + save.size;
        save_depth = depth;
    }
    depth = align_up(depth, va.align) + va.size;
    AbiSlots {
        save_depth,
        obj_depth: depth,
        save_off: -(save_depth as i64) - shift,
        shift,
        max_depth: depth,
    }
}

/// 函数里有没有 `VaArg` / `VaStart`（决定要不要准备可变副本 / 跑展开）。
pub(crate) fn has_va_op(func: &Function) -> bool {
    for (_, bd) in func.dfg.blocks() {
        for &ii in &bd.inst_order {
            if matches!(
                func.dfg.inst_data(ii).opcode,
                Opcode::VaArg | Opcode::VaStart
            ) {
                return true;
            }
        }
    }
    false
}

/// **保存区形态**的 `va` 信息（`save` + `init` 齐备才有值）：没有 ⇒ 调用方不展开
/// （win64 栈式走生成器专用臂）。
///
/// 实现 = 拿一份 `LowerCtx` 走 `abi_setup::setup_conv`（与编译入口同一份装配逻辑与
/// fail-closed 消息），只取其中的 `va` 镜像。
pub(crate) fn save_area_va_info<M: crate::machine::target::TargetMachine>(
    machine: &M,
    func: &Function,
    module_sigs: Option<&[(bool, u32)]>,
) -> Result<Option<(VaInfo, forge_isa_runtime::machine::call_layout::CallLayout)>, IrError> {
    use crate::machine::target::TargetMachine;
    crate::pipeline_hooks::ensure_registered();
    let mut ctx = forge_isa_runtime::ctx::LowerCtx::new();
    ctx.value_gpr_class = TargetMachine::reg_info(machine).value_gpr_class();
    crate::pipeline::abi_setup::setup_conv(&mut ctx, machine, func, module_sigs)?;
    // 只要求"有 va 信息 + 有 plan"，**不**在这里过滤形态：保存区一族由调用方去展开
    // （`init` 缺 = 该形态的初值还没算 ⇒ `expand_va` 给出明确错误）；win64 栈式则交给
    // 生成器那条专用臂（`save` 为 `None`，调用方据此跳过）。
    Ok(ctx
        .conv
        .layout()
        .and_then(|cl| cl.va.clone().map(|va| (va, cl.clone()))))
}

/// 保存区一族（"主游标 + 次游标 + 溢出指针 + 保存区指针"）在 `va.fields` 里的**字段序**。
///
/// 这是**一族的契约**，不是某个 ISA 的常量：偏移与宽度一律从 `va.fields` 取（plan 里就有
/// 名/偏移/宽），所以换一份约定/换一台机器（不同槽宽、不同指针宽）不改代码——只要它还是
/// 这一族的形态（`sysv_reg_save` 就是；`aapcs64`/`riscv` 的语义不同，各自落地时再加一族）。
const F_MAIN_CURSOR: usize = 0;
const F_ALT_CURSOR: usize = 1;
const F_OVERFLOW: usize = 2;
const F_REG_SAVE: usize = 3;

/// 字节宽 → IR 整数类型（游标字段的宽度由 plan 给：sysv64 是 u32，别的形态可能更宽）。
fn int_ty(bytes: u32) -> TypeId {
    match bytes {
        1 => TypeId::I8,
        2 => TypeId::I16,
        4 => TypeId::I32,
        _ => TypeId::I64,
    }
}

/// 把函数里所有 `VaStart`/`VaArg` 展开成显式 IR（`va` 必须是**保存区形态**且带 `init`）。
///
/// `obj_off` = `va_list` 对象槽、`save_off` = 寄存器保存区，两者都由管线在编译入口预留
/// （帧基址相对偏移）。返回 `(物化了几份对象, 展开了几条取值)`。
pub(crate) fn expand_va(
    func: &mut Function,
    va: &VaInfo,
    slots: &AbiSlots,
) -> Result<(usize, usize), IrError> {
    let Some(init) = va.init.clone() else {
        return Err(IrError::Unsupported(
            "变参（保存区形态）：宿主没给 `CallLayout.va.init`（逐字段初值/上限/步长），无法展开"
                .into(),
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
        expand_arg(func, b, ii, &init, va)?;
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
    init: &forge_isa_runtime::machine::call_layout::VaInit,
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
        return Err(IrError::Unsupported(format!(
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
        let field_ty = int_ty(f.size.max(1));
        let value = match val {
            forge_isa_runtime::machine::call_layout::VaInitVal::Imm(v) => {
                let c = mk_iconst(func, b, &mut out, *v as i64);
                // 常量按字段宽度截断（宽度不足时）
                if f.size.max(1) >= 8 {
                    c
                } else {
                    mk_unop(func, b, &mut out, Opcode::Ireduce, c, field_ty)
                }
            }
            forge_isa_runtime::machine::call_layout::VaInitVal::FrameOff(off) => {
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
            forge_isa_runtime::machine::call_layout::VaInitVal::SaveOff => {
                let i = func.make_inst(
                    Opcode::StackAddr,
                    b,
                    smallvec::smallvec![],
                    smallvec::smallvec![Immediate::Int(-(slots.save_depth as i64))],
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

fn expand_arg(
    func: &mut Function,
    b: Block,
    ii: Inst,
    init: &forge_isa_runtime::machine::call_layout::VaInit,
    va: &VaInfo,
) -> Result<(), IrError> {
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
    let (is_fp, is_f32) = {
        let ts = func.types.borrow();
        (ts.is_float(ty), ts.is_float(ty) && ts.size_bytes(ty) == 4)
    };
    // **布局与宽度全部来自 plan 的 `va.fields`**（不写死任何 ISA 的偏移/宽度）：
    //   主游标（整数类用 gp_offset、浮点类用 fp_offset）与它的宽度、溢出指针、保存区指针。
    let field = |i: usize| -> Result<(u32, u32), IrError> {
        va.fields
            .get(i)
            .map(|f| (f.offset, f.size.max(1)))
            .ok_or_else(|| {
                IrError::Unsupported(format!(
                    "va_arg（保存区形态）：plan 的 `va.fields` 缺第 {} 个字段（该形态不在本族）",
                    i + 1
                ))
            })
    };
    let (cur_off, cur_size) = field(if is_fp { F_ALT_CURSOR } else { F_MAIN_CURSOR })?;
    let (ov_field_off, _) = field(F_OVERFLOW)?;
    let (rs_field_off, _) = field(F_REG_SAVE)?;
    let cur_ty = int_ty(cur_size);
    let (limit, step) = if is_fp {
        (init.fp_limit, init.fp_step)
    } else {
        (init.gp_limit, init.gp_step)
    };
    let slot_bytes = va
        .save
        .as_ref()
        .and_then(|s| s.slots.first().map(|x| x.size))
        .unwrap_or(8)
        .max(1);
    let pos = func
        .dfg
        .block(b)
        .inst_order
        .iter()
        .position(|&x| x == ii)
        .ok_or_else(|| IrError::Internal("va_arg：指令不在块序里".into()))?;

    let mut out: Vec<Inst> = Vec::new();
    // cur = zext(load <cur_size> [ap + cur_off])   ← 偏移与宽度都来自 plan
    let off_v = mk_iconst(func, b, &mut out, i64::from(cur_off));
    let p_off = mk_binop(func, b, &mut out, Opcode::Iadd, ap, off_v, TypeId::PTR);
    let cur_raw = mk_load(func, b, &mut out, p_off, cur_ty);
    // 游标按**无符号**扩展到地址宽（宽度由 ISA 的地址类决定，不写死 I64）。
    let addr_ty = if cur_size >= 8 { cur_ty } else { TypeId::I64 };
    let cur = if cur_size >= 8 {
        cur_raw
    } else {
        mk_unop(func, b, &mut out, Opcode::Uextend, cur_raw, TypeId::I64)
    };
    // in_reg = icmp ult cur, limit
    let lim_v = mk_iconst(func, b, &mut out, i64::from(limit));
    let in_reg = {
        let i = func.make_inst(
            Opcode::Icmp,
            b,
            smallvec::smallvec![cur, lim_v],
            smallvec::smallvec![Immediate::IntCC(IntCC::UnsignedLessThan)],
            &[TypeId::I8],
            InstFlags::NONE,
        );
        let r = func.dfg.inst_data(i).results[0];
        out.push(i);
        r
    };
    // reg_save = [ap + rs_field_off]；overflow = [ap + ov_field_off]（偏移来自 plan）
    let rs_off = mk_iconst(func, b, &mut out, i64::from(rs_field_off));
    let rs_addr = mk_binop(func, b, &mut out, Opcode::Iadd, ap, rs_off, TypeId::PTR);
    let reg_save = mk_load(func, b, &mut out, rs_addr, TypeId::PTR);
    let ov_off = mk_iconst(func, b, &mut out, i64::from(ov_field_off));
    let ov_addr = mk_binop(func, b, &mut out, Opcode::Iadd, ap, ov_off, TypeId::PTR);
    let overflow = mk_load(func, b, &mut out, ov_addr, TypeId::PTR);
    // p_src = select(in_reg, reg_save + cur, overflow)
    let p_reg = mk_binop(func, b, &mut out, Opcode::Iadd, reg_save, cur, TypeId::PTR);
    let p_src = mk_select(func, b, &mut out, in_reg, p_reg, overflow, TypeId::PTR);
    // v = load/fload ty [p_src]（f32 先取 f64 再 Fptrunc —— 默认提升）
    let val = if is_f32 {
        let d = mk_fload(func, b, &mut out, p_src, TypeId::F64);
        mk_unop(func, b, &mut out, Opcode::Fptrunc, d, ty)
    } else if is_fp {
        mk_fload(func, b, &mut out, p_src, ty)
    } else {
        mk_load(func, b, &mut out, p_src, ty)
    };
    // 游标推进（in_reg 时推寄存器游标，否则推溢出游标）
    let step_v = mk_iconst(func, b, &mut out, i64::from(step));
    let cur_next = mk_binop(func, b, &mut out, Opcode::Iadd, cur, step_v, addr_ty);
    let cur_out = mk_select(func, b, &mut out, in_reg, cur_next, cur, addr_ty);
    let ov_step = mk_iconst(func, b, &mut out, i64::from(slot_bytes));
    let ov_next = mk_binop(
        func,
        b,
        &mut out,
        Opcode::Iadd,
        overflow,
        ov_step,
        TypeId::PTR,
    );
    let ov_out = mk_select(func, b, &mut out, in_reg, overflow, ov_next, TypeId::PTR);
    // 写回：游标字段按**它的宽度**截断后写回，溢出指针整宽写回
    let cur_stored = if cur_size >= 8 {
        cur_out
    } else {
        mk_unop(func, b, &mut out, Opcode::Ireduce, cur_out, cur_ty)
    };
    let w_off = mk_iconst(func, b, &mut out, i64::from(cur_off));
    let w_addr = mk_binop(func, b, &mut out, Opcode::Iadd, ap, w_off, TypeId::PTR);
    mk_store(func, b, &mut out, cur_stored, w_addr);
    let o_off = mk_iconst(func, b, &mut out, i64::from(ov_field_off));
    let o_addr = mk_binop(func, b, &mut out, Opcode::Iadd, ap, o_off, TypeId::PTR);
    mk_store(func, b, &mut out, ov_out, o_addr);
    // 原指令改 Copy（结果值原样保留 → 新取到的值）
    {
        let inst = func.dfg.inst_mut(ii);
        inst.opcode = Opcode::Copy;
        inst.operands = smallvec::smallvec![val];
        inst.immediates = smallvec::smallvec![];
    }
    func.refresh_inst_uses(ii);
    // 新指令（块尾）按原顺序移到 VaArg 之前
    let mut moved: Vec<Inst> = Vec::with_capacity(out.len());
    for _ in 0..out.len() {
        let order = &mut func.dfg.block_mut(b).inst_order;
        moved.push(
            order
                .pop()
                .ok_or_else(|| IrError::Internal("va_arg 展开：块尾取不到新指令".into()))?,
        );
    }
    move_new_before(func, b, pos, &out)?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use forge_ir::ir::builder::FunctionBuilder;
    use forge_ir::ir::types::{FunctionSignature, TypeContext};
    use forge_isa_runtime::machine::call_layout::{VaInfo, VaKind, VaSave};

    fn probe_with_local() -> Function {
        let ctx = TypeContext::new();
        let sig = FunctionSignature::new(&[], &[TypeId::I64]);
        let mut b = FunctionBuilder::new("probe", ctx, sig);
        let (e, _) = b.create_block_with_params(&[]);
        b.switch_to_block(e);
        let slot = b.stack_addr(-16);
        let v = b.iconst(7, TypeId::I64);
        b.store(v, slot);
        let got = b.load(slot, TypeId::I64);
        b.ret(&[got]);
        b.finish().expect("probe")
    }

    fn va(save: bool) -> VaInfo {
        VaInfo {
            kind: VaKind::SysvRegSave,
            size: 24,
            align: 8,
            stack_only: false,
            fields: vec![],
            save: save.then(|| VaSave {
                size: 176,
                align: 16,
                slots: vec![],
            }),
            init: None,
        }
    }

    /// **ABI 槽必须排在前端局部槽之下**（2026-10-01 实测的坑）：前端 `stack_addr(-16)` 在
    /// lowering 里才被统计，若 ABI 槽从 0 起排就会与它重叠。这条断言是**确定性**的
    /// （直接看分配结果），不依赖某个用例"恰好读错值"。
    #[test]
    fn abi_slots_sit_below_frontend_locals() {
        let f = probe_with_local();
        let front = frontend_locals_depth(&f, 8);
        assert!(
            front > 0,
            "带 stack_addr 的函数必须扫出正的局部槽深度（实测 {front}）"
        );
        // 有保存区：保存区接在前端局部槽**之下**、对象再在保存区之后
        let s = plan_abi_slots(&f, &va(true), 8, 64);
        assert!(
            s.save_depth >= front + 176,
            "保存区必须在前端局部槽之下（front={front}, save_depth={}）",
            s.save_depth
        );
        assert!(s.obj_depth > s.save_depth, "对象在保存区之下");
        assert_eq!(s.max_depth, s.obj_depth);
        assert_eq!(
            s.save_off,
            -(s.save_depth as i64) - 64,
            "序言用的偏移 = -depth - shift"
        );
        // 没有保存区（win64 栈式）：对象直接排在前端局部槽之下
        let s2 = plan_abi_slots(&f, &va(false), 8, 64);
        assert!(s2.obj_depth >= front + 24, "对象槽在局部槽之下");
    }
}
