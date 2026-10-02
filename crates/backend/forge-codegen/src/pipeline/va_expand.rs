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
    /// 保存区深度（正值；`StackAddr` 立即数用 `-depth`）。**要求连续时**它是"相对帧顶的
    /// 深度"（= 保存区字节数），不再是"排在前端局部槽之下"的那个深度。
    pub save_depth: u32,
    /// `va_list` 对象深度（正值）。
    pub obj_depth: u32,
    /// 保存区相对**帧基址**的偏移（序言 spill 用；`-depth - stack_slot_shift`）。
    pub save_off: i64,
    /// 保存区基址的 `StackAddr` 立即数（`va_start` 的 `SaveOff` 初值按它算）：
    /// 地址 = `fp + 本值 - stack_slot_shift`。不连续 = `-save_depth`（历史口径）；
    /// 连续 = `shift - save.size`（地址 = `fp - save.size`，**与 shift 无关**）。
    pub save_base_v: i64,
    /// 帧顶给保存区留出的字节数（v20 V7）：> 0 时 ra/fp、callee-saved 的保存槽与局部槽
    /// 整体下移这么多（生成物读 `AllocResult.va_top`，局部槽读 `stack_slot_shift`）。
    pub va_top: u32,
    /// 帧基址 → 局部槽区起点的平移：`StackAddr(v)` 的地址 = `fp + v - shift`，
    /// 所以"帧基址相对偏移 X"要发 `StackAddr(X + shift)`。
    pub shift: i64,
    /// ABI 槽占用的总深度（喂给 `CompileState`，让帧尺寸覆盖到）。
    pub max_depth: u32,
}

/// 前端局部槽的深度——**直接用唯一实现**（`pipeline/frame_slots.rs`），不再自己镜像一份：
/// ABI 槽必须排在这些槽之下，否则保存区/`va_list` 对象会被局部变量覆盖（`va_arg` 读出垃圾）。
fn frontend_locals_depth(func: &Function, slot_bytes: u32) -> u32 {
    let types = func.types.clone();
    let slots = crate::pipeline::frame_slots::scan_frontend_slots(func, slot_bytes, |t| {
        types.borrow().size_bytes(t).max(1)
    });
    slots.depth_bytes(slot_bytes)
}

/// 算 ABI 槽（保存区 → 对象，依次排在前端局部槽之下）。
///
/// **例外（v20 V7）**：形状要求"保存区与栈实参连续"时（`va.save_contiguous`）保存区放
/// **帧顶** `[入口 sp - save.size, 入口 sp)`——于是它**不**占局部槽之下的深度，`va_list`
/// 对象仍排在局部槽之下；`va_top` 告诉调用方把其余东西（ra/fp、callee-saved、局部槽）
/// 整体下移一个 `save.size`。这样"单一线性游标"走到保存区末尾时，下一个地址正好是调用方
/// 写在 `[入口 sp + …)` 的栈实参。
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
    if let Some(save) = va.save.as_ref().filter(|_| va.save_contiguous) {
        // 帧顶：只记 va_top 与"相对帧基址"的偏移，不占深度。
        depth = align_up(depth, va.align) + va.size;
        return AbiSlots {
            save_depth: save.size,
            obj_depth: depth,
            save_off: -(i64::from(save.size)),
            // `save_base_v` 是 `StackAddr` 的立即数，而 `StackAddr(v)` 的地址 = `fp + v - shift`；
            // 调用方会把 `stack_slot_shift` **加上** `va_top`（局部槽下移），所以这里的立即数要用
            // **加上之前**的 shift：`fp + shift - (shift + save.size) = fp - save.size` ✓。
            // （第一版写成 `shift - save.size` ⇒ 地址少了 2×save.size，QEMU 用例当场读出错值。）
            save_base_v: shift,
            va_top: save.size,
            shift,
            // 帧要装得下：帧顶保存区 + 下面那一整串（局部槽/对象槽的深度已含后者）。
            max_depth: depth + save.size,
        };
    }
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
        save_base_v: -(save_depth as i64),
        va_top: 0,
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

/// **变参信息**（形状 + 保存区 + 逐字段初值）：没有 ⇒ 调用方给出"该约定不支持变参"的明确错误。
///
/// 实现 = 拿一份 `LowerCtx` 走 `abi_setup::setup_conv`（与编译入口同一份装配逻辑与
/// fail-closed 消息），只取其中的 `va` 镜像。
pub(crate) fn va_info_for_expansion<M: crate::machine::target::TargetMachine>(
    machine: &M,
    func: &Function,
    module_sigs: Option<&[(bool, u32)]>,
) -> Result<Option<(VaInfo, forge_isa_runtime::machine::call_layout::CallLayout)>, IrError> {
    use crate::machine::target::TargetMachine;
    crate::pipeline_hooks::ensure_registered();
    let mut ctx = forge_isa_runtime::ctx::LowerCtx::new();
    ctx.value_gpr_class = TargetMachine::reg_info(machine).value_gpr_class();
    crate::pipeline::abi_setup::setup_conv(&mut ctx, machine, func, module_sigs)?;
    // 只要求"有 `va_list` 信息 + 有 plan"：形状/初值齐不齐由 `expand_va` 报错（消息更近现场）。
    Ok(ctx
        .conv
        .layout()
        .and_then(|cl| cl.va.clone().map(|va| (va, cl.clone()))))
}

/// 字节宽 → IR 整数类型（游标字段的宽度由 plan 给：sysv64 是 u32，别的形态可能更宽）。
fn int_ty(bytes: u32) -> TypeId {
    match bytes {
        1 => TypeId::I8,
        2 => TypeId::I16,
        4 => TypeId::I32,
        _ => TypeId::I64,
    }
}

/// 把函数里所有 `VaStart`/`VaArg` 展开成显式 IR。
///
/// 需要两样数据齐备：`va.init`（对象每个字段的初值，宿主按 plan 算）与 `va.arg_rules`
/// （取参规则，引擎从形状数据解析）——缺任何一样都**明确报错**（不猜、也不套别的形状的偏移）。
/// `slots` 是管线在编译入口预留的 ABI 槽（保存区 + 对象）。返回 `(物化了几份对象, 取值几条)`。
pub(crate) fn expand_va(
    func: &mut Function,
    va: &VaInfo,
    slots: &AbiSlots,
) -> Result<(usize, usize), IrError> {
    let Some(init) = va.init.clone() else {
        return Err(IrError::Unsupported(format!(
            "变参：宿主没给 `CallLayout.va.init`（对象字段初值），无法物化 `va_list`——\
             形状 = {}（保存区 {} / 字段 {} 个）",
            va.shape.as_deref().unwrap_or("<explicit>"),
            if va.save.is_some() { "有" } else { "无" },
            va.fields.len()
        )));
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
            forge_isa_runtime::machine::call_layout::VaInitVal::SaveOff(off) => {
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
fn expand_arg(
    func: &mut Function,
    b: Block,
    ii: Inst,
    init: &forge_isa_runtime::machine::call_layout::VaInit,
    va: &VaInfo,
) -> Result<(), IrError> {
    let _ = init; // 初值在 `va_start` 里写；取值只读对象
    let site = va_arg_site(func, ii)?;
    let (ap, ty, is_fp, is_f32) = (site.ap, site.ty, site.is_fp, site.is_f32);
    let rule = if is_fp {
        va.arg_rules.float
    } else {
        va.arg_rules.int
    };
    let field = |i: usize| -> Result<(u32, u32), IrError> {
        va.fields
            .get(i)
            .map(|f| (f.offset, f.size.max(1)))
            .ok_or_else(|| {
                IrError::Unsupported(format!(
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
    let cur_raw = mk_load(func, b, &mut out, cur_addr, int_ty(cur_size));
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
                IntCC::SignedLessThan
            } else {
                IntCC::UnsignedLessThan
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
            store_cursor(func, b, &mut out, cur_out, cur_size, cur_off, ap);
            store_at(func, b, &mut out, ov_out, ov_off, ap);
            p
        }
        None => {
            let step_v = mk_iconst(func, b, &mut out, i64::from(rule.step));
            let cur_next = mk_binop(func, b, &mut out, Opcode::Iadd, cur, step_v, TypeId::I64);
            store_cursor(func, b, &mut out, cur_next, cur_size, cur_off, ap);
            addr
        }
    };
    // ④ 取值：f32 先取 f64 再窄回（C/LLVM 的默认实参提升）；f64 走 Fload；整数/指针走 Load
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
) {
    let stored = if cur_size >= 8 {
        cur
    } else {
        mk_unop(func, b, out, Opcode::Ireduce, cur, int_ty(cur_size))
    };
    store_at(func, b, out, stored, off, ap);
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
    let (is_fp, is_f32, bits, is_vec) = {
        let ts = func.types.borrow();
        (
            ts.is_float(ty),
            ts.is_float(ty) && ts.size_bytes(ty) == 4,
            ts.size_bytes(ty) * 8,
            ts.is_vector(ty) || ts.is_scalable_vector(ty),
        )
    };
    if is_vec {
        return Err(IrError::Unsupported(
            "va_arg: 向量结果需要「mem → 向量寄存器」的取值能力（尚未申报）".into(),
        ));
    }
    if is_fp && bits != 32 && bits != 64 {
        return Err(IrError::Unsupported(
            "va_arg: 只支持 f64 与 f32（f32 走默认提升的窄回）；更窄的浮点还没有能力声明".into(),
        ));
    }
    Ok(VaArgSite {
        ap,
        ty,
        is_fp,
        is_f32,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use forge_ir::ir::builder::FunctionBuilder;
    use forge_ir::ir::types::{FunctionSignature, TypeContext};
    use forge_isa_runtime::machine::call_layout::{VaArgRule, VaArgRules, VaInfo, VaSave};

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
            shape: Some("sysv_reg_save".to_string()),
            size: 24,
            align: 8,
            stack_only: false,
            // 合成夹具：不要求"与栈实参连续"（那是 RISC-V 的形状数据）——保持历史布局口径。
            save_contiguous: false,
            fields: vec![],
            save: save.then(|| VaSave {
                size: 176,
                align: 16,
                slots: vec![],
            }),
            init: None,
            arg_rules: VaArgRules {
                int: VaArgRule {
                    cursor: 0,
                    base: None,
                    overflow: None,
                    limit: 0,
                    signed_limit: false,
                    step: 8,
                    overflow_step: 8,
                    cursor_origin: 0,
                    cursor_counts_down: false,
                },
                float: VaArgRule {
                    cursor: 1,
                    base: None,
                    overflow: None,
                    limit: 0,
                    signed_limit: false,
                    step: 16,
                    overflow_step: 8,
                    cursor_origin: 48,
                    cursor_counts_down: false,
                },
            },
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
