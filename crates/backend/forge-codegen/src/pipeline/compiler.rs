//! CompileSession v20 — staged compilation pipeline.
//!
//! Drives IR → machine code compilation through the TargetMachine component traits.
//! Each stage is a self-contained method; the pipeline is explicit and debuggable.
//!
//! # Pipeline Stages
//!
//! 1. Block Mapping          — IR Block → VBlockId
//! 2. VReg Pre-allocation    — params, phi, block params → VReg
//! 3. Pattern Matching       — detect IR patterns for optimized lowering
//! 4. Instruction Selection  — TargetLowering::lower_inst / lower_terminator
//! 6. Register Allocation    — pluggable allocator
//! 7. Frame Layout           — compute spill slots, alignment, frame size
//! 8. Prologue               — TargetFrameLowering::emit_prologue
//! 9. Instruction Emission   — TargetEncoder + spill/label/fixup handling
//! 10. Epilogue              — TargetFrameLowering::emit_epilogue
//! 11. Fixup Resolution      — CodeSink::finish()

use crate::machine::target::TargetMachine;
use crate::pipeline::alloc_config::{ClassConfig, RegAllocConfig as NewRegAllocConfig};

/// Map an `AtomicRmwOp` discriminant (as encoded by `builder.atomic_rmw` into
/// `Immediate::Uint`) back to the enum. Defaults to `Add` on unknown values.
pub(crate) fn atomic_op_from_u64(v: u64) -> AtomicRmwOp {
    use AtomicRmwOp::*;
    match v {
        0 => Xchg,
        1 => Add,
        2 => Sub,
        3 => And,
        4 => Nand,
        5 => Or,
        6 => Xor,
        7 => Max,
        8 => Min,
        9 => Umax,
        10 => Umin,
        11 => Fadd,
        _ => Fsub,
    }
}
use crate::pipeline::agg_expand::{AggSlots, UseJob, UseKind, rewrite_agg_value_uses};
use crate::pipeline::alloc_result::AllocResult;
use crate::pipeline::regalloc_bt::BacktrackingAllocator;
use crate::{CompiledFunction, IrError, LowerCtx, MachineInst, VBlockId, VCode};
use forge_ir::entity::map::SecondaryMap;
use forge_ir::*;
use std::collections::HashMap;

/// 大聚合实参/形参的**落点类别**（判据来自 `forge-abi` 的约定规则，不是本地写死的阈值）。
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum AggArgLoc {
    /// 按引用（win64 `aggregate size_gt 8 ⇒ indirect via caller_stack_copy`）：
    /// IR 侧统一表示成"**调用方自建副本 + 传指针**"——两端形状都是 `ptr` ⇒ 落点都是
    /// `Reg`，位置自然一致；机器级效果与 Win64 psABI 相同（指针进第一个可用整数槽）。
    ByRef,
    /// **按值整块**进传出参数区（sysv64 `aggregate size_gt 16 ⇒ stack`）：调用方从
    /// "指向本地副本的指针"逐段拷 `size` 字节进传出区（发射侧 `ArgPlace::StackByVal`），
    /// 被调方按 `offset`（被调方视角、已含 shadow）当内存读。
    StackByVal { offset: i32, size: u16 },
    /// 寄存器传值（≤ 约定阈值）——现有拆段路径。
    Registers,
}

/// 只读地问一次"这份约定下**这个类型**的聚合实参落在哪"（单参探针）。
///
/// 返回 `(落点, callee-saved 区平移)`：平移量取自**同源**的
/// [`crate::pipeline::frame_layout::frame_layout_info`]（`LowerCtx::stack_slot_shift`
/// 也是它），用于把 ABI 偏移换成 `[[lower.StackAddr]]` 要的立即数（见 `StackAddr` 的补偿）。
///
/// **形状必须摊开成员**（C1 修复）：早期版本伪造 `Aggregate { members: [] }`，于是
/// AAPCS64/LP64D 的 HFA（`{f32,f32,f32}` 走浮点池）在这里被当成"普通聚合"⇒ 返回
/// `ByRef`，调用方按"本地副本 + 传指针"改写，而被调方按浮点寄存器收参 ⇒ **静默错值**。
/// 现在形状与**调用点/被调方**同一份投影（`TyView::from_ir_type`，递归摊开成员/lane）。
fn agg_arg_place(
    machine: &impl forge_isa_runtime::machine::target::TargetMachine,
    conv: &str,
    store: &forge_isa_runtime::ir::ir::types::TypeStore,
    ty: TypeId,
) -> Result<(AggArgLoc, i32), IrError> {
    use forge_isa_runtime::machine::abi_model::TyView;
    let size = store.size_bytes(ty).max(1);
    let req = forge_isa_runtime::machine::call_plan::CallRequest::new(conv.to_string())
        .args([TyView::from_ir_type(store, ty)]);
    let reg = crate::pipeline::conv_registry::registry()
        .read()
        .expect("约定注册表被投毒");
    let plan = crate::pipeline::abi_target::plan_for_shapes(machine, &reg, &req).map_err(|e| {
        IrError::Unsupported(format!("聚合落点查询：约定 `{conv}` 规划失败（{e}）"))
    })?;
    // callee-saved 区平移：与 `LowerCtx::stack_slot_shift` 同源（`frame_layout_info`）。
    // `[[lower.StackAddr]]` 的 lea 位移 = **立即数 − 平移**，所以被调方要落在
    // `[fp + ABI 偏移]` 上就得把平移加回立即数（见 `expand_large_agg_params` 的补偿）。
    let shift =
        crate::pipeline::frame_layout::frame_layout_info(machine, Some(&plan)).stack_slot_shift;
    Ok(match plan.args.first().map(|a| &a.place) {
        Some(forge_abi::Placement::Indirect { .. }) => (AggArgLoc::ByRef, shift),
        Some(forge_abi::Placement::StackByVal { offset, size, .. }) => (
            AggArgLoc::StackByVal {
                offset: *offset,
                size: *size,
            },
            shift,
        ),
        // 标量语义的 `Stack`（一格一个值）对聚合不该出现；出现了就是约定数据有问题。
        Some(forge_abi::Placement::Stack { .. }) => {
            return Err(IrError::Unsupported(format!(
                "聚合落点查询：约定 `{conv}` 把 {size} 字节聚合判成标量栈落点（约定数据有问题）"
            )));
        }
        _ => (AggArgLoc::Registers, shift),
    })
}

/// 只读地问一次"这份约定下**这个类型**的聚合返回落在哪"（单参探针）。
///
/// 与 [`agg_arg_place`] 同源：`plan.ret` 是 `Indirect`（sret，隐藏返回缓冲指针）
/// 就是"按值返回要写进调用方缓冲"。win64/`sysv64` 的 `>8B`/`>16B` 聚合返回都是这条路。
/// 形状同样摊开成员（C1 修复，理由见 [`agg_arg_place`]）。
fn agg_ret_is_sret(
    machine: &impl forge_isa_runtime::machine::target::TargetMachine,
    conv: &str,
    store: &forge_isa_runtime::ir::ir::types::TypeStore,
    ty: TypeId,
) -> Result<bool, IrError> {
    use forge_isa_runtime::machine::abi_model::TyView;
    let req = forge_isa_runtime::machine::call_plan::CallRequest::new(conv.to_string())
        .rets([TyView::from_ir_type(store, ty)]);
    let reg = crate::pipeline::conv_registry::registry()
        .read()
        .expect("约定注册表被投毒");
    let plan = crate::pipeline::abi_target::plan_for_shapes(machine, &reg, &req).map_err(|e| {
        IrError::Unsupported(format!("聚合返回落点查询：约定 `{conv}` 规划失败（{e}）"))
    })?;
    Ok(matches!(plan.ret, forge_abi::RetLoc::Indirect { .. }))
}

/// 给函数**追加**一个形参（签名 + entry 块同步），返回该形参的值。
///
/// 追加而不是前插：sret 形参的**位置**不影响落点（引擎把它收进约定的隐藏槽
/// ——首个整数槽，正是调用方把缓冲指针放在第 0 个实参里的那个位置），
/// 而前插要让所有已有 `ValueDef::Param(entry, i)` 的下标整体搬家。
fn append_param(
    func: &mut Function,
    ty: TypeId,
    name: &str,
    attrs: forge_ir::ir::function::ParamAttributes,
) -> Result<Value, IrError> {
    let entry = func.entry();
    let old_sig = func.types.borrow().get_signature(func.signature).clone();
    let mut params = old_sig.params.clone();
    params.push((ty, ImmStr::from(name)));
    let new_sig = FunctionSignature {
        params,
        returns: old_sig.returns.clone(),
        calling_convention: old_sig.calling_convention,
        variadic: old_sig.variadic,
    };
    func.signature = func.types.borrow_mut().register_signature(new_sig);
    let bd = func.dfg.block(entry);
    let idx = bd.param_values.len() as u16;
    let v = func.dfg.make_value(ty, ValueDef::Param(entry, idx));
    let bd = func.dfg.block_mut(entry);
    bd.params.push(ty);
    bd.param_values.push(v);
    while func.param_attrs.len() < idx as usize + 1 {
        func.param_attrs.push(Default::default());
    }
    func.param_attrs[idx as usize] = attrs;
    Ok(v)
}

/// 被调方大聚合参数展开：按**约定落点**分流——
///
/// - `Registers`（≤ 阈值）：签名拆开为 ceil(size/8) 个 i64 参数（与调用方拆段一致），
///   entry 块参数重建，旧参数值使用处重写（extractvalue → 段值、store → 段 store、
///   return/call → 段值）；
/// - `ByRef`（win64 `>8B`）：形参改为**单个 `ptr`**，并把"该聚合值住在指针指向的内存"
///   登记进 `agg_slots` ⇒ 使用处按地址重写（`rewrite_agg_value_uses` 的 ExtractValue
///   分支走 GEP+load、store 走向内存拷贝、传参传该指针）。**不写 `byval` 属性**：
///   引擎对 `byval` 落点给 `Indirect`，而生成物 `move_args` 的 `Indirect` 收参是
///   "从指针 load 出寄存器里的值"（向量语义）——聚合没有寄存器表示，要的是**指针本身**
///   进参数寄存器（`Reg` 落点）；
/// - `Stack`（sysv64 `>16B`）：发射侧尚未接 ⇒ 明确 Unsupported（A2′）。
fn expand_large_agg_params(
    func: &mut Function,
    agg_slots: &mut AggSlots,
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
    for (i, offset, ty) in stack_agg {
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
        agg_slots.insert(old_v, (addr, ty));
        rewrite_agg_value_uses(func, old_v, &[], ty, agg_slots)?;
    }
    // by-ref 档：登记"聚合住在指针指向的内存"，再按地址重写使用处
    for (old_v, ptr_v, agg_ty) in byref_info {
        agg_slots.insert(old_v, (ptr_v, agg_ty));
        rewrite_agg_value_uses(func, old_v, &[], agg_ty, agg_slots)?;
    }
    // 使用处重写（聚合参数——rewrite 内部即时移动辅助指令）
    for (old_v, segs, agg_ty) in agg_info {
        rewrite_agg_value_uses(func, old_v, &segs, agg_ty, agg_slots)?;
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

/// 按值返回 sret 的**前置**处理（A1′-ret）：约定把 `>8B` 聚合返回落进**调用方缓冲**
/// （`plan.ret = Indirect`，即 sret）时，给函数**追加**一个带 `sret` 属性的 `ptr` 形参，
/// 并把签名的返回清空——值写进缓冲由 [`expand_large_agg_ret`] 完成。
///
/// **必须先于其它聚合展开**：那些展开（形参/实参/call 结果）会把返回值重写或登记进
/// `agg_slots`，早一步把 sret 形参立起来，后续它们才知道"返回值往哪儿写"。
fn expand_sret_return(
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

/// ret 大聚合展开：`ret {i64,i64} %v`（>8 ≤16 字节）→ `Terminator::Return
/// { values: [seg0, seg1] }`（多值返回——lowering 按 values.len() 生成
/// RAX/RDX 双槽 mov）。段来源：AggConst（Iconst 段）、load 结果（段 load）、
/// 其余（参数段值已被 expand_large_agg_params 重写为 i64——不在此处理）。
fn expand_large_agg_ret(
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
                    let mut seg_val: u64 = 0;
                    for k in 0..8 {
                        seg_val |= (bytes.get(off + k).copied().unwrap_or(0) as u64) << (k * 8);
                    }
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

/// 调用方 call 结果（大聚合 >8 ≤16）拆 2 段：`%q = call {i64,i64}` 的 results
/// 改 [i64, i64]（ValueDef::Inst(call, 0/1)——lowering 按 results.len() 生成
/// RAX/RDX 双 mov），旧结果值使用处重写（extractvalue/store/转传）。
fn expand_large_agg_call_results(
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
            // 规范 immediate 形态（帧槽预扫描只认 `Type + Uint`，见 `frame_slots.rs`）
            let alloca = func.make_inst(
                Opcode::Alloca,
                job.block,
                smallvec::smallvec![],
                smallvec::smallvec![Immediate::Type(TypeId::I8), Immediate::Uint(size as u64)],
                &[TypeId::PTR],
                InstFlags::NONE,
            );
            let buf = func.dfg.inst_data(alloca).results[0];
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

/// 函数是否含 >8 字节聚合 load（决定是否需要可变副本做大聚合值传播）。：`call @f({i64,i64} %v)` 的
/// 聚合实参（>8 且 ≤16 字节——SysV 整数寄存器路径）拆为 ceil(size/8) 个 i64
/// 段值追加到 call operands（内部签名宽松——callee 签名不校验；lowering 按
/// operands 逐参数占槽——RCX/RDX/R8/R9 + 栈参数）。
/// 段来源：AggConst（字节已知——插 Iconst 段常量）、load 结果（追踪 addr——
/// 插段 load）；其余（块参数/小聚合寄存器值）Unsupported。>16 字节栈传递
/// 未实现（Unsupported）。
/// 必须在 expand_large_aggs **之前**：拆段后大聚合值不再被 call 使用，
/// expand_large_aggs 的使用检测不命中 Call。
fn expand_agg_call_args(
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
                    let mut seg_val: u64 = 0;
                    for k in 0..8 {
                        seg_val |= (bytes.get(off + k).copied().unwrap_or(0) as u64) << (k * 8);
                    }
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
            ValueDef::Inst(li, 0) => {
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
                return Err(IrError::Unsupported(format!(
                    "聚合实参拆段：值来源不支持（def {:?}）",
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
            // **规范 immediate 形态** `Type(elem) + Uint(count)`——帧槽预扫描
            //（`frame_slots.rs`）只认这两个；写 `Const(池 id)` 会让槽不进表 ⇒ 没偏移。
            let alloca = func.make_inst(
                Opcode::Alloca,
                job.block,
                smallvec::smallvec![],
                smallvec::smallvec![Immediate::Type(TypeId::I8), Immediate::Uint(size as u64)],
                &[TypeId::PTR],
                InstFlags::NONE,
            );
            let slot = func.dfg.inst_data(alloca).results[0];
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

/// 大聚合 load 使用展开任务（S1：UseJob 提升为模块级——collect_extract_jobs 复用）
/// S1：收集新登记聚合值（嵌套提取中间值）的 extractvalue 使用，追加到展开
/// 任务队列（load 索引沿用原值——loads 在展开中不修改）。
fn collect_extract_jobs(func: &Function, jobs: &mut Vec<UseJob>, res: Value, load_idx: usize) {
    for (b, bd) in func.dfg.blocks() {
        for (pos, &ii) in bd.inst_order.iter().enumerate() {
            let inst = &func.dfg.inst_data(ii);
            if inst.opcode == Opcode::ExtractValue
                && inst.operands.first() == Some(&res)
                && let Some(Immediate::Uint(u)) = inst.immediates.first()
            {
                jobs.push(UseJob {
                    block: b,
                    pos,
                    inst: ii,
                    kind: UseKind::Extract(*u as u32),
                    load: load_idx,
                });
            }
        }
    }
}

/// 大聚合 load 值传播（全 ISA 通用，编译期）：`%v = load T(>8B), ptr %p` 的
/// 大聚合值无法入 GPR——按使用处就地展开（load 指令标 Nop，值不物化）：
/// - `store T %v, ptr %q`（拷贝）：逐段 `load i64 [addr+off]` + `store i64 [q+off]`；
/// - `extractvalue T %v, idx`：偏移 `add ptr addr, field_offset` + 标量 load，
///   extractvalue 改 Copy；
/// - 其余使用（传参/返回/insertvalue 等）：Unsupported。
///
/// 局限：load 后内存被改写时重新 load 不保留快照语义（常见模式安全）。
fn expand_large_aggs(func: &mut Function, agg_slots: &mut AggSlots) -> Result<(), IrError> {
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
            if let Some(&(slot_addr, slot_ty)) = agg_slots.get(hit_inst.operands[0]) {
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
                    agg_slots.insert(res, (addr2, fty));
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
                agg_slots.insert(res_new, (addr2, field_ty));
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

/// GEP 无基址操作数时的占位值（正常 IR 不会出现——防御性兜底）。
///
/// `Block(0)` 在这里只是"必须给一个块句柄"的**语法占位**（`ValueDef::Param`
/// 要求块 + 下标），**不是**"入口是 0 号块"的约定——该值不参与任何支配/
/// CFG 判断（GEP 展开只把它当 PTR 值用）。入口约定请用 `Function::entry()`。
fn make_placeholder_ptr(func: &mut Function) -> Value {
    func.dfg
        .make_value(TypeId::PTR, ValueDef::Param(Block::new(0), 0))
}

/// GEP 展开（全 ISA 通用，编译期）：`getelementptr T, ptr %p, i32 i0, i32 i1...`
/// 逐级计算偏移——常量索引折叠（第一索引作用于 indexed_ty 自身；struct 用
/// field_offset；数组用 elem_size×idx），动态索引（数组下标）展开为
/// `mul i64 %idx, size` + `add`。GEP 指令原地改为 Copy（result 保留——
/// SSA 引用一致），展开的 mul/add 插在 GEP 位置之前。展开后不依赖机器
/// GEP 规则（旧 `lea [rs1+rs2*4+0]` 单索引规则已废弃）。
/// 局限：struct 索引必须常量（LLVM 语义）；索引按无符号处理（数组下标）。
fn expand_geps(func: &mut Function) -> Result<(), IrError> {
    // 取一次读锁（v3 S3 读路径纪律）：本函数只读类型，内部不再逐次取锁。
    // 先从 func.types clone 出 TypeContext（Arc，O(1)）再取锁——守卫借的是这个局部，
    // 不与 func 的 &mut 借用冲突。
    let types_ctx = func.types.clone();
    let store = types_ctx.borrow();
    let mut jobs: Vec<(Block, usize, Inst)> = Vec::new(); // (block, pos, inst)
    for (b, bd) in func.dfg.blocks() {
        for (pos, &ii) in bd.inst_order.iter().enumerate() {
            if func.dfg.inst_data(ii).opcode == Opcode::GetElementPtr {
                jobs.push((b, pos, ii));
            }
        }
    }
    for (b, pos, ii) in jobs.into_iter().rev() {
        // 提前 copy 出 GEP 数据（inst 借用须在 &mut func 生成段前结束）
        let (indexed_ty, base, indices) = {
            let inst = &func.dfg.inst_data(ii);
            (
                inst.immediates
                    .iter()
                    .find_map(|i| match i {
                        Immediate::Type(t) => Some(*t),
                        _ => None,
                    })
                    .unwrap_or(TypeId::PTR),
                inst.operands.first().copied(),
                inst.operands.iter().skip(1).copied().collect::<Vec<_>>(),
            )
        };
        let base = base.unwrap_or_else(|| make_placeholder_ptr(func));
        // 索引常量折叠（Iconst 值 → i64）
        let fold = |func: &Function, v: Value| -> Option<i64> {
            let def = func.dfg.value_data(v).def;
            let ValueDef::Inst(ci, _) = def else {
                return None;
            };
            let id = func.dfg.inst_data_opt(ci)?;
            if id.opcode != Opcode::Iconst {
                return None;
            }
            let im = id.immediates.first()?;
            match im {
                Immediate::Uint(v) => Some(*v as i64),
                Immediate::Int(v) => Some(*v),
                Immediate::Const(c) => func.constants.get_int(*c).map(|(v, _)| v as i64),
                _ => None,
            }
        };
        let mut cur_ty = indexed_ty;
        let mut const_off: i64 = 0;
        let mut dyn_parts: Vec<(Value, u32)> = Vec::new(); // (idx 值, 元素字节)
        for (i, &idx) in indices.iter().enumerate() {
            let i = i + 1; // 原操作数下标（operands[0]=base）
            let ts = &store;
            if i == 1 {
                // 第一索引作用于 indexed_ty 自身（T* → T 数组）
                let size = ts.size_bytes(cur_ty) as i64;
                match fold(func, idx) {
                    Some(c) => const_off += c * size,
                    None => dyn_parts.push((idx, size as u32)),
                }
                continue;
            }
            if let TypeEntry::Struct { .. } = ts.get(cur_ty) {
                let c = fold(func, idx).ok_or_else(|| {
                    IrError::Unsupported("GEP struct 索引必须为常量（LLVM 语义）".into())
                })?;
                let off = ts.field_offset(cur_ty, c as u32).ok_or_else(|| {
                    IrError::Unsupported(format!("GEP struct 索引越界：{c}（type {cur_ty:?}）"))
                })?;
                const_off += off as i64;
                cur_ty = ts
                    .aggregate_elem_type(cur_ty, c as u32)
                    .ok_or_else(|| IrError::Unsupported("GEP 字段类型缺失".into()))?;
            } else {
                let elem_ty = ts.element_type(cur_ty).ok_or_else(|| {
                    IrError::Unsupported("GEP 索引作用于非数组/结构体类型".into())
                })?;
                let elem_size = ts.size_bytes(elem_ty) as i64;
                match fold(func, idx) {
                    Some(c) => const_off += c * elem_size,
                    None => dyn_parts.push((idx, elem_size as u32)),
                }
                cur_ty = elem_ty;
            }
        }
        // 生成展开序列：动态部分 mul+add，最后常量偏移 add（const_off!=0 或全零时
        // 仍需 add 保持链尾——全零无动态时直接 Copy base）
        let mut prev = base;
        let mut new_count = 0usize;
        for (idx_v, size) in dyn_parts {
            // %t = mul i64 %idx, size
            let size_cid = func.constants.insert_int(size as i128, 64);
            let size_inst = func.make_inst(
                Opcode::Iconst,
                b,
                smallvec::smallvec![],
                smallvec::smallvec![Immediate::Const(size_cid)],
                &[TypeId::I64],
                InstFlags::NONE,
            );
            let sval = func.dfg.inst_data(size_inst).results[0];
            let mul = func.make_inst(
                Opcode::Imul,
                b,
                smallvec::smallvec![idx_v, sval],
                smallvec::smallvec![],
                &[TypeId::I64],
                InstFlags::NONE,
            );
            let mval = func.dfg.inst_data(mul).results[0];
            // %p = add i64 %prev, %t（结果类型 PTR——内部指针算术，verify 不跑）
            let add = func.make_inst(
                Opcode::Iadd,
                b,
                smallvec::smallvec![prev, mval],
                smallvec::smallvec![],
                &[TypeId::PTR],
                InstFlags::NONE,
            );
            prev = func.dfg.inst_data(add).results[0];
            new_count += 3;
        }
        if const_off != 0 || new_count > 0 {
            let off_cid = func.constants.insert_int(const_off as i128, 64);
            let off_inst = func.make_inst(
                Opcode::Iconst,
                b,
                smallvec::smallvec![],
                smallvec::smallvec![Immediate::Const(off_cid)],
                &[TypeId::I64],
                InstFlags::NONE,
            );
            let off_v = func.dfg.inst_data(off_inst).results[0];
            let add = func.make_inst(
                Opcode::Iadd,
                b,
                smallvec::smallvec![prev, off_v],
                smallvec::smallvec![],
                &[TypeId::PTR],
                InstFlags::NONE,
            );
            prev = func.dfg.inst_data(add).results[0];
            new_count += 2;
        }
        // GEP 原地改 Copy（result 保留——SSA 引用一致）
        let __cp = ii;
        {
            let inst = func.dfg.inst_mut(__cp);
            inst.opcode = Opcode::Copy;
            inst.operands = smallvec::smallvec![prev];
            inst.immediates = smallvec::smallvec![];
        }
        func.refresh_inst_uses(__cp);
        // 新指令（块尾）移到 GEP 位置前（逆序处理 jobs：尾即本 GEP 的新指令）
        let mut new_insts: Vec<Inst> = Vec::with_capacity(new_count);
        for _ in 0..new_count {
            let order = &mut func.dfg.block_mut(b).inst_order;
            new_insts.push(order.pop().unwrap());
        }
        new_insts.reverse();
        for (k, &ni) in new_insts.iter().enumerate() {
            func.dfg.block_mut(b).inst_order.insert(pos + k, ni);
        }
    }
    Ok(())
}

/// 聚合常量打包：聚合树 → 小端 u64（≤8 字节聚合；字段按内存布局偏移排列）。
/// 聚合常量 → 内存字节（小端布局：标量按 field_offset 写入 LE 字节，
/// 未覆盖区零——对齐填充/空洞）。任意大小（>8 字节聚合分段 store 用）。
/// 聚合 store 展开（全 ISA 通用，编译期）：聚合常量 → 内存字节（小端），
/// 按 8 字节段展开为普通标量 store——`store {i32,i32} {i32 7, i32 9}, ptr %p`
/// → `store i64 <7|9<<32>, ptr %p`；>8 字节（如 {i64,i64} 16 字节）→ 逐段
/// `store i64 <seg>, ptr <addr>` + `add ptr, 8` 地址推进（尾段按 8/4/2/1
/// 字节宽 store，不越界）。展开使 store 源成为普通 Iconst 值（修复原 SEGV：
/// 聚合常量从不加载进寄存器 + 聚合类型 bits()=0 → store 宽度 0）。
/// - 聚合 load 结果 >8 字节与聚合常量传参 >8 字节：Phase 2 处理（此处保留
///   Unsupported——大聚合值内存化传播）。
/// - 仅小端数据布局（x86_64/aarch64 默认）；大端需反向打包（暂不支持）。
fn expand_agg_stores(func: &mut Function) -> Result<(), IrError> {
    // 取一次读锁（v3 S3 读路径纪律）：本函数只读类型，内部不再逐次取锁。
    // 先从 func.types clone 出 TypeContext（Arc，O(1)）再取锁——守卫借的是这个局部，
    // 不与 func 的 &mut 借用冲突。
    let types_ctx = func.types.clone();
    let store = types_ctx.borrow();
    // 第一遍：收集（聚合常量 → 字节布局）
    let mut jobs: Vec<(Block, usize, usize, Vec<u8>, bool)> = Vec::new(); // (block, pos, operand 索引, 内存字节, 是否 Store)
    for (b, bd) in func.dfg.blocks() {
        for (pos, &ii) in bd.inst_order.iter().enumerate() {
            let inst = &func.dfg.inst_data(ii);
            match inst.opcode {
                Opcode::Store => {
                    if let Some(v) = inst.operands.first()
                        && let ValueDef::AggConst(aid) = func.dfg.value_data(*v).def
                    {
                        let agg = func.constants.get_aggregate(aid).ok_or_else(|| {
                            IrError::Unsupported("聚合常量 store：常量池缺失 AggId".into())
                        })?;
                        let bytes = crate::pipeline::agg_const::pack_agg_bytes(func, agg)?;
                        jobs.push((b, pos, 0, bytes, true));
                    }
                }
                // 大聚合 load 由 expand_large_aggs 处理（值传播展开）——此处不拦截
                // Call/CallIndirect 的聚合常量参数：与 Store 同理打包展开（否则
                // AggConst 值从不加载——传参 0/垃圾）。builder.call 的 operands
                // 全是实参（callee 在 Immediate::Func）；CallIndirect 的
                // operands[0] 是 callee 指针（skip 1）。
                Opcode::Call | Opcode::CallIndirect => {
                    let skip = usize::from(inst.opcode == Opcode::CallIndirect);
                    for (op_idx, v) in inst.operands.iter().enumerate().skip(skip) {
                        if let ValueDef::AggConst(aid) = func.dfg.value_data(*v).def {
                            let agg = func.constants.get_aggregate(aid).ok_or_else(|| {
                                IrError::Unsupported("聚合常量参数：常量池缺失 AggId".into())
                            })?;
                            let size = store.size_bytes(agg.ty);
                            if size > 8 {
                                // >8 字节由 expand_agg_call_args 拆段处理——跳过
                                continue;
                            }
                            let bytes = crate::pipeline::agg_const::pack_agg_bytes(func, agg)?;
                            jobs.push((b, pos, op_idx, bytes, false));
                        }
                    }
                }
                _ => {}
            }
        }
    }
    // 逆序替换：inst_order.insert(pos) 移动后续下标——从后往前插入不受影响。
    for (b, pos, op_idx, bytes, is_store) in jobs.into_iter().rev() {
        let store_ii = func.dfg.block(b).inst_order[pos];
        // Store：分段展开（operands[1]=addr）；Call 参数：单打包值替换 operand
        let addr = if is_store {
            let inst = &func.dfg.inst_data(store_ii);
            inst.operands.get(op_idx + 1).copied()
        } else {
            None
        };
        let mut new_insts: Vec<Inst> = Vec::new();
        if let Some(addr) = addr {
            let mut seg_off = 0usize;
            let mut prev_addr = addr;
            while seg_off < bytes.len() {
                let seg_len = (bytes.len() - seg_off).min(8);
                let mut seg_val: u64 = 0;
                for k in 0..seg_len {
                    seg_val |= (bytes[seg_off + k] as u64) << (k * 8);
                }
                let store_ty = match seg_len {
                    1 => TypeId::I8,
                    2 => TypeId::I16,
                    4 => TypeId::I32,
                    _ => TypeId::I64,
                };
                // 段常量（复用 make_inst 的 results[0]——不可再 make_value）
                let cid = func
                    .constants
                    .insert_int(seg_val as i128, (seg_len * 8) as u32);
                let iconst = func.make_inst(
                    Opcode::Iconst,
                    b,
                    smallvec::smallvec![],
                    smallvec::smallvec![Immediate::Const(cid)],
                    &[store_ty],
                    InstFlags::NONE,
                );
                let cv = func.dfg.inst_data(iconst).results[0];
                new_insts.push(iconst);
                let st = func.make_inst(
                    Opcode::Store,
                    b,
                    smallvec::smallvec![cv, prev_addr],
                    smallvec::smallvec![],
                    &[],
                    InstFlags::SIDE_EFFECT,
                );
                new_insts.push(st);
                seg_off += seg_len;
                if seg_off < bytes.len() {
                    // prev_addr = add ptr prev_addr, seg_len（非 GEP——expand_geps 不碰）
                    let off_cid = func.constants.insert_int(seg_len as i128, 64);
                    let off_inst = func.make_inst(
                        Opcode::Iconst,
                        b,
                        smallvec::smallvec![],
                        smallvec::smallvec![Immediate::Const(off_cid)],
                        &[TypeId::I64],
                        InstFlags::NONE,
                    );
                    let off_v = func.dfg.inst_data(off_inst).results[0];
                    new_insts.push(off_inst);
                    let add = func.make_inst(
                        Opcode::Iadd,
                        b,
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
        if !is_store {
            // Call 参数（≤8 字节）：打包为单 i64 常量替换 operand
            let mut packed: u64 = 0;
            for (k, &byte) in bytes.iter().enumerate() {
                packed |= (byte as u64) << (k * 8);
            }
            let cid = func.constants.insert_int(packed as i128, 64);
            let iconst = func.make_inst(
                Opcode::Iconst,
                b,
                smallvec::smallvec![],
                smallvec::smallvec![Immediate::Const(cid)],
                &[TypeId::I64],
                InstFlags::NONE,
            );
            let cv = func.dfg.inst_data(iconst).results[0];
            {
                let inst = func.dfg.inst_mut(store_ii);
                inst.operands[op_idx] = cv;
            }
            // 就地改单个操作数也要重登记 use-lists
            func.refresh_inst_uses(store_ii);
            new_insts.push(iconst);
        } else {
            // 原 Store 标 Nop（无结果——不生成机器指令；lowering 跳过 Nop）
            let __tomb = store_ii;
            func.tombstone_inst(__tomb);
        }
        // 新指令（块尾）移到 Store 位置前（逆序处理 jobs：尾即本 Store 的新指令）
        let mut moved: Vec<Inst> = Vec::with_capacity(new_insts.len());
        for _ in 0..new_insts.len() {
            let order = &mut func.dfg.block_mut(b).inst_order;
            moved.push(order.pop().unwrap());
        }
        moved.reverse();
        for (k, &ni) in moved.iter().enumerate() {
            func.dfg.block_mut(b).inst_order.insert(pos + k, ni);
        }
    }
    Ok(())
}

/// **编译会话**——IR → 机器码的唯一编译实现（单函数与整模块两条入口都走它）。
///
/// 单函数：`new`/`for_module` → `compile`（或 `compile_with_alloc`）。
/// 整模块：`for_module` → `compile_module`（见 `pipeline/session.rs` 的 `CompiledModule`）。
pub struct CompileSession<M: TargetMachine> {
    machine: M,
    reg_alloc: BacktrackingAllocator,
    /// IR 级优化级别（None = 不优化，直接编译）。Some(level) 时
    /// `compile()`/`compile_with_alloc()` 先对函数克隆体跑对应 PassManager 管线再 lower。
    /// P0-1：把 forge-opt 优化管线接入生产编译（此前 PassManager 是死代码）。
    opt_level: Option<forge_opt::OptimizationLevel>,
    /// **模块级签名表**（变参 D6）：`FuncRef::index()` → 签名事实。
    ///
    /// 编译**整个模块**的入口（或装过表的单函数入口）填它（[`Self::with_module_sigs`]）；
    /// 没装 ⇒ 调用点按非变参处理。
    module_sigs: Option<std::sync::Arc<forge_ir::ModuleSigs>>,
}

impl<M: TargetMachine> CompileSession<M> {
    /// Create a new compiler with the default register allocator (backtracking).
    pub fn new(machine: M) -> Self {
        // 宿主级**一次**注册（幂等）：管线工厂 + **调用点布局钩子**（v20 A5-3）。
        // 生成物的 lowering 在调用点按 `(ISA, 约定, 实参形状)` 查后者拿被调方落点；
        // 没注册就 fail-closed——所以放在每个编译入口（OnceLock，热路径只是一次原子读）。
        crate::pipeline_hooks::ensure_registered();
        Self {
            machine,
            reg_alloc: BacktrackingAllocator,
            opt_level: None,
            module_sigs: None,
        }
    }

    /// 宿主机器（整模块编排要用它的克隆；会话自己持有它，不再外借）。
    pub(crate) fn machine(&self) -> &M {
        &self.machine
    }

    /// 装上**模块级签名表**（变参 D6）。
    ///
    /// 表由 IR 模块自己给出（[`forge_ir::Module::signature_table`]）——别在宿主侧另抄一份。
    pub fn with_module_sigs(mut self, sigs: forge_ir::ModuleSigs) -> Self {
        self.module_sigs = Some(std::sync::Arc::new(sigs));
        self
    }

    /// **编译"住在某个模块里的函数"**——从模块自己取签名表装上（变参 D6）。
    ///
    /// 宿主侧的默认写法：只要函数住在某个模块里，就把模块一起传进来，
    /// 别让调用点去猜被调方是不是变参（[`Self::compile_module`] 就是这么做的）。
    pub fn for_module(machine: M, module: &forge_ir::Module) -> Self {
        Self::new(machine).with_module_sigs(module.signature_table())
    }

    /// 启用 IR 级优化（`compile`/`compile_with_alloc` 前跑对应 O 级别管线）。
    pub fn with_opt_level(mut self, level: forge_opt::OptimizationLevel) -> Self {
        self.opt_level = Some(level);
        self
    }

    /// 当前优化级别（None = 未启用）。
    pub fn opt_level(&self) -> Option<forge_opt::OptimizationLevel> {
        self.opt_level
    }

    /// Compile an IR function (full pipeline，含 IR 级优化若已启用 `with_opt_level`)。
    pub fn compile(&self, func: &Function) -> Result<CompiledFunction, IrError> {
        self.compile_with_alloc(func).map(|(cf, _)| cf)
    }

    /// 值池门：函数内每个值类型都必须能被 ISA 承载
    /// （`TargetRegInfo::class_for_type` 返回 `Some`）。否则报 `Unsupported`
    /// 并点名类型与值池宽度——**绝不**静默按某个宽度缺省生成寄存器类。
    fn check_value_pools(machine: &M, func: &Function) -> Result<(), IrError> {
        let ri = machine.reg_info();
        let gpr = ri.value_gpr_class();
        let fpr = ri.value_fpr_class();
        for (v, _) in func.dfg.values() {
            let Some(ty) = func.dfg.value_type(v) else {
                continue;
            };
            // 已删除/墓碑值：DCE 之后 `ValueData.ty` 被置为 `TypeId::VOID`
            // （见 `dfg.rs` 的 replace/tombstone 路径），它不承载任何寄存器——
            // 必须跳过，否则 1 字节值池的 ISA 上**任何**被优化掉的死值都会让
            // 整个函数编译失败（`from_type_id(VOID)` 兜底是 GPR(8)）。
            if matches!(ty, TypeId::VOID) {
                continue;
            }
            if ri.class_for_type(ty).is_none() {
                return Err(IrError::Unsupported(format!(
                    "ISA 值池无法承载类型 {}（{} 位；值池 GPR {} 字节 / FPR {} 字节。\
                     需要 [meta].value_gpr_width / addr_width / value_fpr_width 声明更宽的\
                     寄存器组，或改用更窄的 IR 类型）",
                    crate::machine::reg_info::type_label(ty),
                    ty.builtin_scalar_bits()
                        .map(|b| b.to_string())
                        .unwrap_or_else(|| "非标量".to_string()),
                    gpr.bytes(),
                    fpr.bytes()
                )));
            }
        }
        Ok(())
    }

    /// Compile an IR function，额外返回寄存器分配明细（AllocResult：
    /// XReg → PReg assignments、spill_slots 等），供集成测试/工具断言
    /// 分配器行为（ABI 参数分配、clobber 写死保护等）。
    pub fn compile_with_alloc(
        &self,
        func: &Function,
    ) -> Result<(CompiledFunction, crate::AllocResult), IrError> {
        // P0-1：IR 级优化管线接入——若启用，克隆函数跑对应 O 级别管线，
        // 用优化后的克隆体继续编译（原 func 不被修改；PassManager 需要
        // &mut Function）。未启用时不克隆（零开销）。
        // 注：**没有**"绕过优化"的旁路入口——要优化就 `with_opt_level`，不要就默认。
        let mut owned: Function;
        let func: &Function = if let Some(level) = self.opt_level {
            owned = func.clone();
            let pm = forge_opt::PassManager::for_level(level);
            pm.run_on_function(&mut owned)?;
            &owned
        } else {
            func
        };

        let _cf_t0 = std::time::Instant::now();
        // 值池门（fail-closed，2026-09-12 去「宽度写死」）：函数里出现的每个值
        // 类型都必须落在 ISA 值池内（`TargetRegInfo::class_for_type`）。1 字节
        // 寄存器 ISA 上出现 i16/i32/i64/指针 等宽类型时**编译期拒绝**，而不是
        // 静默按 8 字节池生成不存在的寄存器类（历史行为：生成成功但语义错误）。
        Self::check_value_pools(&self.machine, func)?;
        // 宽向量参数/返回（>16 字节）ABI：
        // - 若 ISA 声明 `vector by-ref limit`（如 x86 limit=128 位）→ 宽向量按
        //   引用传参（调用方栈拷贝 + 传指针 GPR；被调方入口从 [ptr] 加载）。
        // - 否则拒绝（ABI 仅寄存器传值，宽向量会静默截断）。
        let by_ref_limit = self.machine.abi().vector_by_ref_limit();
        // **一次快照**（v3 S3：`TypeStoreRef` 是 owned，不与下面 `func` 的借用冲突）——
        // 原来每个参数/返回值/每个指令类型各 `borrow()` 一次（取锁次数 ∝ IR 规模）。
        let types = func.types.borrow();
        let param_tys = func.param_types();
        let return_tys = func.return_types();
        let wide = param_tys
            .iter()
            .chain(&return_tys)
            .find(|t| {
                (types.is_vector(**t) || types.is_scalable_vector(**t))
                    && types.size_bytes(**t) > 16
            })
            .copied();
        if let Some(ty) = wide {
            let bytes = types.size_bytes(ty);
            // S4：超过 16 字节的定宽向量要**本机能编能跑**才放行（V512 = EVEX 前提）——
            // 判据来自 ISA 后端（`TargetRegInfo::wide_vector_bytes` = 谱申报档位 ∩ 宿主上界），
            // 通用管线**不认识** `avx512` 这类能力名（v23 M3）。
            let wide_caps = self.machine.reg_info().wide_vector_bytes();
            if bytes > 16 && !wide_caps.contains(&(bytes as u16)) {
                return Err(IrError::Unsupported(format!(
                    "SIMD 参数/返回 {bytes} 字节宽向量：本 ISA/宿主的宽向量档位是 {wide_caps:?}\
                     （type {ty:?}）"
                )));
            }
            match by_ref_limit {
                Some(lim) if bytes <= lim => {
                    // 在寄存器传值范围内（≤16 字节）：走现有 XMM/GPR 传参路径。
                }
                Some(lim) => {
                    // 超过阈值：by-ref 传参（调用方栈拷贝 + 传指针；被调方入口
                    // 从 [ptr] 加载）。入口守卫通过——move_args/call lowering
                    // 的 by-ref 路径处理。V512（64 字节）等超宽向量若 ISA 未
                    // 声明足够 limit 时由 encode 侧能力决定（avx512 检测）。
                    let _ = lim;
                }
                None => {
                    return Err(IrError::Unsupported(format!(
                        "SIMD 参数/返回 {bytes} 字节暂不支持（ABI 未声明 vector by-ref；type {ty:?}）"
                    )));
                }
            }
        }
        // IR 层宽向量（>16B）能力门（W3，2026-09-12）：
        //  - 32B（V256）：VEX.256（如 vmovups ymm）——ISA 已有算术/常量/槽往返
        //    全套规则；与既有 V256 算术路径一致，不在此加硬件门（运行级用例
        //    自行按 AVX 能力 skip）。
        //  - 64B（V512）：EVEX（zmm）——**需 AVX-512F**；无则编译期拒绝
        //    （与上方 ABI 守卫同一判据），避免发射本机无法执行的 EVEX。
        //  - 其它 >16B 宽度（非 32/64）：无 lowering 规则 → 显式拒绝。
        // 旧行为：>16B 向量 Load/Store 一律拒绝（ISA 缺宽槽类 → 曾落到默认
        // 8 字节 MOV 静默截断）；现由 isa/x86.toml 的 rd_vec/rs1_vec
        // = 32/64 规则 + 本条硬件门共同把住。
        let wide_caps = self.machine.reg_info().wide_vector_bytes();
        for (_, inst) in func.dfg.insts() {
            let tys: Vec<TypeId> = inst
                .results
                .iter()
                .chain(inst.operands.iter())
                .filter_map(|v| func.dfg.value_type(*v))
                .collect();
            for ty in tys {
                if !(types.is_vector(ty) || types.is_scalable_vector(ty)) {
                    continue;
                }
                let bytes = types.size_bytes(ty);
                if bytes <= 16 {
                    continue;
                }
                if bytes > 16 && !wide_caps.contains(&(bytes as u16)) {
                    return Err(IrError::Unsupported(format!(
                        "IR 层 {bytes} 字节宽向量：本 ISA/宿主的宽向量档位是 {wide_caps:?}\
                         （type {ty:?}）"
                    )));
                }
                if bytes != 32 && bytes != 64 {
                    return Err(IrError::Unsupported(format!(
                        "IR {bytes} 字节向量暂不支持（仅 V256 32B / V512 64B 有 lowering \
                         规则；type {ty:?}）"
                    )));
                }
            }
        }
        // 聚合参数/返回 >8 字节：由 expand_agg_call_args / expand_large_agg_params /
        // expand_large_agg_ret 处理（≤16 字节拆段；>16 字节在 pass 内拒绝）。
        // 异常处理（P1.1）：invoke/landingpad/resume 无机器指令映射，编译期拒绝
        for (_, inst) in func.dfg.insts() {
            if matches!(inst.opcode, Opcode::LandingPad) {
                return Err(IrError::Unsupported(
                    "异常处理 landingpad 无机器指令映射（P1.1 仅文本层解析/展示）".to_string(),
                ));
            }
        }
        for (b, _bd) in func.dfg.blocks() {
            if matches!(
                func.dfg.term_kind(b),
                Some(TermKind::Invoke | TermKind::Resume)
            ) {
                return Err(IrError::Unsupported(
                    "异常处理 invoke/resume 无机器指令映射（P1.1 仅文本层解析/展示）".to_string(),
                ));
            }
        }
        // 可变的 IR 副本：聚合 store/参数展开（agg_expand）需要 &mut Function。
        // 展开后读取 func_ref（Stage 4+ 全部只读原函数；S6 的树型 树型匹配
        // 匹配发生在 lowering 驱动内逐块预扫，不需要在编译期克隆整个 Function）。
        let needs_agg_expand = crate::pipeline::agg_expand::needs_agg_expand(func);
        let mut func_owned: Option<Function> = needs_agg_expand.then(|| func.clone());

        // **变参 IR 展开**（v20 变参 V3/V6）：`VaStart`/`VaArg` 在这里展开成显式 IR——按 plan 的
        // 形状数据（字段布局 + 保存区 + 取参规则）跑同一套算法，理由见 `pipeline/va_expand.rs`。
        //
        // 时机：与聚合展开同一处（**必须**在 `CompileState::new` 之前——它克隆常量池；展开会
        // 往池里插常量），也在 lowering 之前。
        //
        // **ABI 槽（保存区 + `va_list` 对象）由这里排布**：必须排在前端局部槽（`stack_addr(-N)`）
        // **之下**，而前端的槽要 lowering 才统计——所以 `plan_abi_slots` 先扫一遍 IR 算深度，
        // 再把结果喂给 `CompileState`（帧尺寸与序言 spill 偏移都读它）。
        let mut abi_slots: Option<crate::pipeline::va_expand::AbiSlots> = None;
        if crate::pipeline::va_expand::has_va_op(func) {
            match crate::pipeline::va_expand::va_info_for_expansion(
                &self.machine,
                func,
                self.module_sigs.clone(),
            )? {
                Some((va, layout)) => {
                    let fl = crate::pipeline::frame_layout::frame_layout_info(
                        &self.machine,
                        Some(&layout),
                    );
                    let slots = crate::pipeline::va_expand::plan_abi_slots(
                        func,
                        &va,
                        crate::machine::target::TargetMachine::reg_info(&self.machine).slot_bytes()
                            as u32,
                        i64::from(fl.stack_slot_shift),
                    );
                    let f = func_owned.get_or_insert_with(|| func.clone());
                    let (ns, na) = crate::pipeline::va_expand::expand_va(f, &va, &slots)?;
                    abi_slots = Some(slots);
                    if crate::pipeline::trace_enabled("FORGE_TRACE_LOWER") {
                        eprintln!(
                            "[forge] 变参展开：物化 {ns} 份对象、取值 {na} 条\
                             （形状 {}、保存区深度 {}、对象深度 {}）",
                            va.shape.as_deref().unwrap_or("<explicit>"),
                            slots.save_depth,
                            slots.obj_depth
                        );
                    }
                }
                None => {
                    return Err(IrError::Unsupported(format!(
                        "变参：函数 `{}` 用了 VaStart/VaArg，但它的约定没声明 `hidden.va_list`\
                         （`va_list` 的形状）——变参需要形状数据，生成器不再有专用臂",
                        func.name
                    )));
                }
            }
        }

        // Stage 0.5: 聚合 store 展开（聚合字面量 → 打包 i64 常量 store；
        // >8 字节聚合显式 Unsupported）。必须在 CompileState 创建**之前**
        // 执行：CompileState 克隆 func 的常量池，若先建 state 再展开，新常量
        // 只进 func_owned 副本的池，lowering 读 state 里旧池（idx 错位 → 0）。
        if let Some(f) = func_owned.as_mut() {
            let before = crate::pipeline::agg_expand::IrSize::of(f);
            expand_agg_stores(f)?;
            // call 结果拆 2 先于参数拆段：%q = call {i64,i64} 传给后续 call 时
            // 参数已是段值（i64），expand_agg_call_args 的 load 追踪不会误判
            let mut agg_slots: AggSlots = SecondaryMap::new();
            // 聚合按值传参的落点判据来自**约定数据**（只读单参探针）：win64 `>8B ⇒ 调用方
            // 副本 + 指针`、sysv64 `>16B ⇒ 传出参数区（未接）`。约定名解析失败 ⇒ fail-closed
            //（与 `abi_setup::setup_conv` 同口径）。
            let agg_conv = crate::pipeline::conv_registry::resolve(&f.calling_convention)?;
            // A1′-ret：sret 形参**先立起来**——后面几个展开要把返回值重写或登记进
            // `agg_slots`，得先知道"往哪写"。
            let sret_ptr = expand_sret_return(f, &self.machine, &agg_conv)?;
            expand_large_agg_call_results(f, &mut agg_slots, &self.machine, &agg_conv)?;
            expand_agg_call_args(f, &self.machine, &agg_conv)?;
            expand_large_agg_params(f, &mut agg_slots, &self.machine, &agg_conv)?;
            expand_large_agg_ret(f, &mut agg_slots, sret_ptr)?;
            expand_large_aggs(f, &mut agg_slots)?;
            expand_geps(f)?;
            // **展开自报改动了什么**（v23 M3）：`ChangeSet` 取代了原来"先按 7 个
            // `has_*` 预判、再决定跑不跑"的做法——预判漏一类模式就是**静默少跑一个 pass**
            // （历史事故：大聚合返回展开不到 ⇒ 5 vs 6 错值）。现在 pass 无条件跑，
            // 改动量由 IR 规模差给出，`FORGE_TRACE_AGG` 可见、下方断言可查。
            let changes =
                crate::pipeline::agg_expand::IrSize::of(f).delta_to(before, sret_ptr.is_some());
            if crate::pipeline::trace_enabled("FORGE_TRACE_AGG") {
                eprintln!(
                    "[forge] 聚合展开 {}：共 {} 项（+{} 指令 / +{} 值 / +{} 常量 / sret={}）",
                    f.name,
                    changes.total(),
                    changes.new_insts,
                    changes.new_values,
                    changes.new_consts,
                    changes.sret
                );
            }
            // 聚合展开是**就地改写 IR**（墓碑化/改 Copy/重写操作数）——debug 构建下
            // 立刻校验 **use-def 一致性**（2026-09-14 S6：这些站点此前只改字段、
            // 不更新 use-lists，留下陈旧 use 项）。
            //
            // 这里只跑 use-lists 契约而不是完整 `Verifier`：本阶段会**有意**造出
            // 类型不完全自洽的 IR —— `expand_geps` 的指针算术把 PTR 基址与 i64 偏移
            // 放进同一条 `Iadd`（结果标 PTR；源码注释也写着"verify 不跑"），
            // 而 DSL 后端没有 `Ptrtoint`/`Inttoptr` 的 lowering，无法用显式转换
            // 表达同一语义。该类型模型欠账记在 `docs/archive/forge-ir-v3-plan.md`
            // （S4/S5：指针算术的 typed 表示），不在本阶段用"放宽校验"掩盖。
            #[cfg(debug_assertions)]
            {
                if let Err(errors) = f.use_lists.verify(&f.dfg) {
                    return Err(IrError::Internal(format!(
                        "聚合展开后 use-lists 不一致（fn {}）：{errors:?}",
                        f.name
                    )));
                }
            }
        }
        // Stage 4+ 读取的 Function：有改写（pattern/聚合展开）时用副本。
        let func_ref: &Function = func_owned.as_ref().unwrap_or(func);

        let mut state =
            CompileState::new(&self.machine, func_ref, self.module_sigs.clone(), abi_slots)?;
        let _t = std::time::Instant::now();

        // Stage 1: Block mapping
        state.create_blocks(func_ref);
        let _t1 = _t.elapsed();
        let _t = std::time::Instant::now();

        // Stage 2: VReg pre-allocation
        state.alloc_params(func_ref);
        state.pre_allocate_phi_vregs(func_ref);
        state.pre_allocate_block_param_xregs(func_ref);
        let _t2 = _t.elapsed();
        let _t = std::time::Instant::now();
        let _t2b = _t.elapsed();
        let _t = std::time::Instant::now();

        // Stage 4: Instruction selection
        state.lower_all_blocks(func_ref, self.machine.lowering().as_ref())?;
        let _t3 = _t.elapsed();
        let _t = std::time::Instant::now();

        // v20 A3b 预备：`FORGE_TRACE_ABI=1` 打印该函数的调用计划（引擎产物）——发射尚未切换，
        // 但"计划长什么样"现在可诊断（与既有 ABI 路径对照时这是唯一证据面）。
        if crate::pipeline::trace_enabled("FORGE_TRACE_ABI") {
            eprintln!("[abi] === fn: {}", func_ref.name);
            for line in state.abi_plan.to_text().lines() {
                eprintln!("[abi] {line}");
            }
        }

        if crate::pipeline::trace_enabled("FORGE_TRACE_VCODE") {
            eprintln!("[vcode] === fn: {}", func_ref.name);
            for (bi, block) in state.vcode.blocks().enumerate() {
                eprintln!("[vcode] block {bi}: {:?}", block.ir_block);
                for inst in &block.instructions {
                    eprintln!("[vcode]   {:?}", inst);
                }
            }
        }

        let _t4 = _t.elapsed();
        let _t = std::time::Instant::now();

        // Stage 6: Register allocation
        let alloc_result = state.run_regalloc(&self.reg_alloc, &self.machine)?;
        let _t5 = _t.elapsed();
        let _t = std::time::Instant::now();

        // Stage 7: Frame layout
        let frame_size = state.calculate_frame_size(&alloc_result, &self.machine);
        let _t6 = _t.elapsed();
        let _t = std::time::Instant::now();

        // Stage 8-11: Emission
        let result = state.emit_code(func_ref, &alloc_result, frame_size, &self.machine);
        let _t7 = _t.elapsed();
        if std::env::var("CF_CODEGEN_TIMING").is_ok() {
            eprintln!(
                "[codegen {}] total={:?} blocks={:?} vreg={:?} lower={:?} peep={:?} regalloc={:?} frame={:?} emit={:?}",
                func_ref.name,
                _cf_t0.elapsed(),
                _t1,
                _t2,
                _t3,
                _t4,
                _t5,
                _t6,
                _t7
            );
        }
        if std::env::var_os("FORGE_TRACE_FN_SIZE").is_some() {
            let sz = result.as_ref().map(|r| r.code.len()).unwrap_or(0);
            eprintln!("[codegen] fn={} code_bytes={sz}", func_ref.name);
            if std::env::var_os("FORGE_TRACE_CODE_HEX").is_some()
                && let Ok(cf) = result.as_ref()
            {
                let hex: String = cf
                    .code
                    .iter()
                    .take(64)
                    .map(|b| format!("{b:02x}"))
                    .collect();
                eprintln!("[codegen] fn={} head={hex}", func_ref.name);
            }
        }
        Ok((result?, alloc_result))
    }
}

// ============================================================
// CompileState — mutable pipeline state
// ============================================================

pub(crate) struct CompileState<I: MachineInst> {
    pub(crate) vcode: VCode<I>,
    /// XReg → 微指令寄存器字段映射（与 vcode 的指令序列平行）。
    /// 由各指令包的 inst_ops 聚合；分配器按此构建活区间并回写字段。
    pub(crate) inst_ops: Vec<smallvec::SmallVec<[crate::machine::lowering::OperandBind; 2]>>,
    /// 每条 vcode 指令的写死物理寄存器集（与 inst_ops/instructions 平行；
    /// 来自 lower 规则的 clobbers 声明，包级展开到包内每条指令）。
    pub(crate) inst_clobbers: Vec<Vec<(u32, crate::prelude::RegClass)>>,
    /// IR 值 → XReg 映射（v3 S2：`Value` 是密集句柄 ⇒ `SecondaryMap`，不哈希）。
    pub(crate) value_to_xreg: SecondaryMap<Value, XReg>,
    /// IR 块 → VCode 块映射（同理：`Block` 是密集句柄）。
    pub(crate) block_map: SecondaryMap<Block, VBlockId>,
    pub(crate) ctx: LowerCtx,
    pub(crate) param_xregs: Vec<XReg>,
    /// Alloca 指令 → 帧槽偏移（预扫描分配；lowering 时经 ctx.current_alloca_offset
    /// 供 `lea_off rd, alloca_offset` 规则取用）。`Inst` 是密集句柄 ⇒ `SecondaryMap`。
    pub(crate) alloca_offsets: SecondaryMap<Inst, i64>,
    /// **该函数的 `AbiPlan`**（v20 A3b 预备 / A6 fail-closed）：编译入口按 IR 签名 +
    /// 约定数据算出来，供发射侧（`CallLayout`）与 `FORGE_TRACE_ABI=1` 诊断使用。
    ///
    /// v20 A6 起**算不出来 = 编译错误**（不再留个 note 继续走旧路径）：每条发射路径都必须
    /// 有 plan，否则调用点/收参处会各自 fail-closed，错误消息离现场远，且"算不出来"容易被
    /// 误当成"这条签名能编"。缺口清单见 `docs/archive/calling-convention-redesign-plan.md` 的 A6。
    pub(crate) abi_plan: forge_abi::AbiPlan,
}

impl<I: MachineInst + 'static> CompileState<I> {
    fn new<M: TargetMachine>(
        machine: &M,
        func: &Function,
        module_sigs: Option<std::sync::Arc<forge_ir::ModuleSigs>>,
        abi_slots: Option<crate::pipeline::va_expand::AbiSlots>,
    ) -> Result<Self, IrError> {
        // 取一次读锁（v3 S3 读路径纪律）：本函数只读类型，内部不再逐次取锁。
        // 先从 func.types clone 出 TypeContext（Arc，O(1)）再取**一次快照**——守卫借的是
        // 这个局部，不与 func 的 &mut 借用冲突；快照交给 LowerCtx（lowering 期间类型表
        // 只读，生成代码不再逐指令取锁，见 v3 S3 余项①）。
        let types_ctx = func.types.clone();
        let store = types_ctx.borrow();
        let mut ctx = LowerCtx::new();
        ctx.type_store = Some(store.clone());
        // 值/地址寄存器类与栈槽单位：全部由 TargetRegInfo 元数据提供
        //（DSL 从 [meta].value_gpr_width/addr_width/slot_bytes 生成）——
        // lowering 里不再出现 RegClass::GPR64/FPR64 字面量（1 字节寄存器 ISA
        // 的值池是 GPR(1)，写死 8 字节会生成不存在的类）。
        {
            let ri = machine.reg_info();
            ctx.value_gpr_class = ri.value_gpr_class();
            ctx.value_fpr_class = ri.value_fpr_class();
            ctx.addr_class = ri.addr_class();
            ctx.slot_bytes = ri.slot_bytes();
            ctx.vector_tiers = ri.vector_tiers().to_vec();
            ctx.type_map = ri.type_map().to_vec();
        }
        // **调用约定装配**（v20）：名字解析（未注册 fail-closed）、约定级返回槽、函数级
        // plan（算不出 = 编译错误）、约定级破坏集、模块签名表——规则与消息都在
        // `pipeline/abi_setup.rs`，这里一个调用点。
        let abi_plan =
            crate::pipeline::abi_setup::setup_conv(&mut ctx, machine, func, module_sigs)?;

        // StackAddr 的 lea 基准需要跳过 callee-saved 区（局部变量不能写在 push
        // 槽上）：fp 保存槽（frame_pointer_overhead）+ callee-saved 寄存器区。
        // 之前只算了 callee-saved 区，漏掉 fp 的 8 字节，局部变量落进 push 槽
        //（覆盖调用者寄存器保存值 → mini_c JIT SEGV/逻辑错误）。
        // 帧布局三数值统一推导：min_frame / callee_saved_bytes / 栈槽平移
        // 都由 frame_layout_info() 从 [machine.frame].layout + callee-saved 表算出
        //（fp-inside：csb=0、栈槽平移=fp_push；fp-outside：csb=pushed、平移
        // 回退 csb）。compiler.rs 的 LowerCtx 与 emission.rs 共用同一来源；
        // **有 plan 时用 plan 的 callee-saved 表**（v20 A5-3 ④：同一台机器换约定，
        // 保存集与破坏集必须一起换）。
        // v23 M5（C6 帧记账单点）：三数值**只在这里算一次**，此后 emission / spill /
        // 序言账目全读 ctx 上这三个字段（min_frame 一并填，calculate_frame_size
        // 不再自己重算——同一份输入重算两次，口径一漂移就是帧字节数不一致）。
        let fl = crate::pipeline::frame_layout::frame_layout_info(machine, ctx.conv.layout());
        ctx.callee_saved_bytes = fl.callee_saved_bytes;
        ctx.stack_slot_shift = fl.stack_slot_shift;
        ctx.min_frame = fl.min_frame;
        ctx.is_float_return = func
            .return_types()
            .iter()
            .any(|t| ctx.reg_class_for(t).is_fp());
        // S2：sret 隐藏参数——函数返回宽向量（>16 字节）→ 首 int 槽被
        // sret 指针占用（move_args 收参 __gi 从 1 起；调用方 sret 约定）。
        ctx.is_sret_return = func.return_types().iter().any(|t| {
            let s = &store;
            (s.is_vector(*t) || s.is_scalable_vector(*t)) && s.size_bytes(*t) > 16
        });
        ctx.constant_pool = Some(func.constants.clone());

        // **变参 ABI 槽**（v20 变参 V3）：保存区与 `va_list` 对象两段的深度由
        // `plan_abi_slots` 在编译入口算好（排在前端局部槽之下）并经 `abi_slots` 传进来——
        // 这里只**播种**：抬 `max_stack_bytes`（帧尺寸要覆盖它们）并记下序言 spill 用的
        // 保存区偏移。展开出来的 `StackAddr` 会让 lowering 自己再统计一次（同一组深度）。
        //
        // **保存区要求与栈实参连续**时（v20 V7，形状数据）保存区改放**帧顶**
        // （`[入口 sp - save_size, 入口 sp)`）：`va_top` 把 ra/fp、callee-saved 的保存槽与
        // 局部槽一起下移一个 save_size（前两者由生成物读 `AllocResult.va_top`，后者由
        // `stack_slot_shift` 加同一份），帧尺寸也要跟着长。
        if let Some(sl) = abi_slots {
            ctx.va_save_off = (sl.save_depth > 0).then_some(sl.save_off);
            ctx.va_top = sl.va_top;
            // 帧顶给保存区让出的字节：局部槽（`StackAddr`）与 ra/fp、callee-saved 的保存槽
            // 一起下移同一份（后者由生成物读 `AllocResult.va_top`），帧尺寸也因此长一截。
            ctx.stack_slot_shift += sl.va_top as i32;
            ctx.max_stack_bytes = ctx.max_stack_bytes.max(sl.max_depth);
        }
        Ok(Self {
            vcode: VCode::new(),
            inst_ops: Vec::new(),
            inst_clobbers: Vec::new(),
            value_to_xreg: SecondaryMap::new(),
            block_map: SecondaryMap::new(),
            ctx,
            param_xregs: Vec::new(),
            alloca_offsets: SecondaryMap::new(),
            abi_plan,
        })
    }

    // ── Stage 6: Register Allocation ──

    fn run_regalloc<M2: TargetMachine<Inst = I>>(
        &mut self,
        allocator: &BacktrackingAllocator,
        machine: &M2,
    ) -> Result<AllocResult, IrError> {
        let ri = machine.reg_info();

        // Build RegAllocConfig from TargetRegInfo — 类表 = **ISA 声明**。
        // DSL 生成的 `RegInfo::register_classes()` 由 `[reg.*]`/`[meta]`/`[abi]`
        // 派生（每个可请求的类指向同族物理寄存器文件）；**不再有编造的 fallback
        // 类表**（2026-09-13，R1）：未声明的类不会出现在分配器配置里，请求即
        // 编译期 Unsupported（此前 GPR(2)/FPR(16)/VEC(16)… 会被凭空造出并借池）。
        let mut classes: HashMap<RegClass, ClassConfig> = HashMap::new();
        for info in ri.register_classes() {
            classes.insert(
                info.reg_class,
                ClassConfig {
                    allocatable: info.allocatable.clone(),
                    reg_width: info.width as u8,
                },
            );
        }

        // 主 GPR/FPR 类推导（元数据驱动）：由 ISA 的 default_*_class() 显式
        // 暴露（DSL 从 `[reg.*]` 与 `[meta]` 生成），compiler 不再猜测
        // GPR64/FPR64。例如 x86 浮点值主类为 FPR(8)（f64 值宽），即使其 XMM
        // 寄存器组是 FPR(16)。
        let main_gpr = ri.default_gpr_class();
        let main_fpr = ri.default_fpr_class();

        // 非 DSL 后端/测试替身（类表为空）时的兜底：至少让主类可用。
        if classes.is_empty() {
            classes.insert(
                main_gpr,
                ClassConfig {
                    allocatable: ri.allocatable_gp_order(),
                    reg_width: main_gpr.bytes() as u8,
                },
            );
            classes.insert(
                main_fpr,
                ClassConfig {
                    allocatable: ri.allocatable_fp_order(),
                    reg_width: main_fpr.bytes() as u8,
                },
            );
        }

        let precolored: HashMap<XReg, PReg> = ri.precolored_xregs().into_iter().collect();

        // v20 A5-3 ④：callee-saved 是**约定事实**——有 plan 就用 plan 的（同一台机器
        // 换约定必须跟着换：sysv64 的 RDI/RSI 不是 callee-saved，win64 是）。谱里那份
        // 是无 plan（未注册约定 / 合成夹具）时的兜底。
        //
        // v20 A6 起**两类都喂**：GPR 与 FPR 各占一项（编号空间分开，免得 X8 与 V8 同号
        // 相撞）。FPR 那半只有在"发射侧能按类保存"之后才敢开——`callee_save` 角色已带类
        // 限定（arm64 的 STURD），`callee_saved_to_save` 也扫主 FPR 类表，`frame_layout_info`
        // 的帧字节数按 plan 的表算（见那里的注释）。缺任何一环，FPR 值就会跨调用被静默破坏。
        // v23 M5（C4）：**(类, 号) 一张表** —— 以前这里按类分成两条平行 `Vec<u32>`，
        // 消费端"只比号不比类"（GPR 号与 FPR 号互相误判）。现在类随号一起走。
        let callee_saved: Vec<(forge_ir::RegClass, u32)> = match self.ctx.conv.layout() {
            Some(cl) => cl
                .callee_saved
                .regs
                .iter()
                .map(|r| (r.class, r.slot))
                .collect(),
            // 无 plan（未注册约定/合成夹具）：v21 W9 删除谱面 callee-saved 两键之后这里
            // 没有兜底表可读——按 A6 的口径，没有 plan 的签名在更早的入口就 fail-closed，
            // 因此空表即可（保持"不静默用别家寄存器名"的纪律）。
            None => Vec::new(),
        };

        let reg_info = NewRegAllocConfig {
            classes,
            main_gpr_class: main_gpr,
            main_fpr_class: main_fpr,
            sp_reg: ri.sp_reg().register_index().unwrap_or(0),
            fp_reg: ri.fp_reg().map(|r| r.to_index()),
            callee_saved,
            precolored,
            scratch_regs: ri
                .scratch_regs()
                .iter()
                // scratch 是值搬运临时寄存器 → 类 = ISA 值池类（x86 = GPR(8)）。
                .map(|&n| PReg::new(n, ri.value_gpr_class()))
                .collect(),
            param_xregs: self.param_xregs.clone(),
            // 寄存器参数数 = **布局里在寄存器里落一个值的形参个数**（v20 A5-3：谱面的
            // `[abi.arg_class]` 已删除，不再用"int 参数槽上限"这个生成期近似——
            // `Indirect { reg: Some(..) }` 也要算：by-ref 的指针在寄存器里、被调方还要把 `[ptr]` 的值
            // load 进自己的形参寄存器——不预分配就会走"spilled ⇒ 不收参"（实测宽向量 by-ref 全 0）。
            param_reg_count: self
                .ctx
                .conv
                .layout()
                .map(|cl| {
                    cl.args
                        .iter()
                        .filter(|a| {
                            // 受支持的落点 = 单寄存器 / 带指针的间接（v23 M2：走访问器，
                            // 不再穷举运行时镜像的枚举）。
                            a.place.as_reg().is_some() || a.place.indirect_ptr().is_some()
                        })
                        .count()
                })
                .unwrap_or(0),
        };

        // **v23 M4 批次 C 第二步：把 `xreg_types`（vreg → IR 类型）折进绑定表一次。**
        //
        // 以前两个回填点各自把这张表推成"值宽类"，**且口径不同**：
        // `compiler.rs` 主回填走 `ctx.reg_class_for(ty)`（ISA 类型映射），
        // `emission.rs` 的 spill 回填走 `forge_ir::RegClass::from_type_id(ty)`。
        // 现场只有一处是对的、另一处是兜底——同一件事两种写法。现在在这里折成
        // `OperandBind::class`，两个回填点读同一个值。折完 `xreg_types` 在本函数后续
        // 不再被读（写入面仍留在 lowering 期作草稿，见方案 §13 的余项）。
        for slot in self.inst_ops.iter_mut() {
            for bind in slot.iter_mut() {
                if let Some(ty) = self.ctx.xreg_types.get(&bind.vreg).copied() {
                    bind.class = Some(self.ctx.reg_class_for(&ty));
                }
            }
        }

        // **v23 M4 收敛守卫**：旁表（`inst_ops`）与指令的**操作数表**必须逐条对齐。
        //
        // 批次 C 要删掉旁表，这条断言就是"删了以后行为不变"的判据——现在先当守卫跑起来。
        // 只对拍 `(field, role)`：旁表里存的是**虚拟寄存器**，而字段里放的是**占位物理
        // 寄存器**，两者本就不相等（让字段占位携带 vreg 号是批次 C 的前置条件，见方案 §13）。
        //
        // 判据的意义：`map_reg_field(xreg, inst, field, is_def)` 的 `(field, is_def)` 必须
        // 能在该指令的操作数表里找到同字段同角色的项；找不到就说明"旁表说的落点"与
        // "指令形状表说的落点"已经分叉——regalloc 会按旁表分配、发射按形状表回填。
        #[cfg(debug_assertions)]
        {
            use crate::machine::inst::OperandRole;
            let mut gi = 0usize;
            for block in self.vcode.blocks() {
                for inst in &block.instructions {
                    if let Some(slot) = self.inst_ops.get(gi) {
                        let ops = inst.operands();
                        for bind in slot.iter() {
                            let (xr, field) = (&bind.vreg, bind.field);
                            let want = bind.role;
                            let hit = ops.iter().any(|o| {
                                usize::from(o.field) == usize::from(field) && o.role == want
                            });
                            assert!(
                                hit,
                                "v23 M4：旁表与操作数表分叉——第 {gi} 条指令的字段 {field} \
                                 被登记为 {want:?}（vreg {}），但操作数表里没有这一项：{ops:?}",
                                xr.index()
                            );
                            // **多类槽的值宽类来源：可见但不强制**（v23 M4，2026-10-11 实测）。
                            //
                            // 形状表说该字段是**多类槽**（`class == None` = 宽度由运行期决定）
                            // 时，回填用的是绑定带的类；绑定没带（`xreg_types` 里没有这个 vreg
                            // 的记录）就退到 vreg 的**池宽类**。
                            //
                            // 实测：确实存在这种绑定（如 `test_jit_agg_by_value_stack_sysv64`
                            // 第 15 条指令、riscv 侧也有）——但这是**既有**行为：改动前的两个
                            // 回填点同样先查 `xreg_types`、查不到就退池宽类，所以 M4 没有改变
                            // 它。这里**断言**会过强（等于要求代码具备它从来没有的性质），
                            // 因此改成**可观测**：`FORGE_TRACE_M4=1` 时报出来，留存量级。
                            if let Some(op) = ops
                                .iter()
                                .find(|o| o.field == bind.field && o.role == bind.role)
                                && op.class.is_none()
                                && bind.class.is_none()
                                && crate::pipeline::trace_enabled("FORGE_TRACE_M4")
                            {
                                eprintln!(
                                    "[m4] 多类槽字段 {} 的 vreg {} 没有值宽类记录 ⇒ 回填退池宽类\
                                     （第 {gi} 条指令，指令形状：{ops:?}）",
                                    bind.field,
                                    xr.index()
                                );
                            }
                        }
                        // **旁表条数不是从形状表推导出来的（2026-10-11 实测，两次修正）**：
                        //
                        // - InOut 字段（如 x86 `add dst, src` 的 dst）在操作数表里**同时**进
                        //   `uses` 与 `defs`，但旁表里它可能 **0 条**（该槽是物理寄存器占位、
                        //   不参与 vreg 映射）或 **1 条 Def**（vreg 绑定）——实测两种都有。
                        // - 因此"删掉 `is_def`、一律按表派生角色"会把这些情况变成 use+def 各一条，
                        //   **改变 regalloc 的活区间口径**（属行为变更，不是纯重构）。
                        //
                        // 这里只钉一条**可证伪的**事实：旁表条数不得为形状表里该字段出现次数的
                        // 函数以外的值——即 InOut 字段最多 1 条（0 或 1），普通字段最多 1 条。
                        // 真正的判据仍是上面那条子集关系；本块是"改动会被看见"的哨兵。
                        for op in ops.iter() {
                            let in_uses = ops
                                .iter()
                                .any(|o| o.field == op.field && o.role == OperandRole::Use);
                            let in_defs = ops
                                .iter()
                                .any(|o| o.field == op.field && o.role == OperandRole::Def);
                            if in_uses && in_defs {
                                let n = slot.iter().filter(|b| b.field == op.field).count();
                                assert!(
                                    n <= 1,
                                    "v23 M4：InOut 字段 {} 的旁表条数 {n} > 1（实测契约是 0 或 1）\
                                     ——旁表语义变了，请当行为变更处理：{slot:?}",
                                    op.field
                                );
                            }
                        }
                    }
                    gi += 1;
                }
            }
        }

        let alloc_result =
            allocator.allocate(&self.vcode, &reg_info, &self.inst_ops, &self.inst_clobbers)?;

        // S2：sret 隐藏参数标记（move_args 收参跳过首 int 槽）——
        // 由 CompileState::new 的 LowerCtx.is_sret_return 预计算。
        let mut alloc_result = alloc_result;
        alloc_result.sret = self.ctx.is_sret_return;
        // v20 A3b-2b：把中性调用布局一并交给帧件（序言/尾声只拿到 AllocResult）。
        alloc_result.call_layout = self.ctx.conv.layout.clone();
        // 栈参数区字节数（move_args 收栈参数时计算 spill 槽地址）
        alloc_result.stack_arg_bytes = self.ctx.max_stack_arg_bytes;
        // 寄存器保存区 / 对象槽偏移（v20 变参 V3）：序言 spill 用前者。
        alloc_result.va_save_off = self.ctx.va_save_off;
        // 帧顶保存区字节数（v20 V7）：fp-inside 的存取机制要按它把自己的偏移减掉一份。
        alloc_result.va_top = self.ctx.va_top;
        // 参数字节宽（IR 类型 size_bytes）——@move_args 的 by-ref 宽向量收参按
        // 真实字节宽分派 32B/64B load 变体。寄存器类宽对 >128 位向量恒为
        // VEC(32)（reg_class_for），无法区分 V256/V512：旧实现用 __pv.width()
        // 使 64B 分支永不可达 → V512 收参只 load 32B（WA-37 D5）。
        alloc_result.param_bytes = self
            .param_xregs
            .iter()
            .map(|v| {
                self.ctx
                    .xreg_types
                    .get(v)
                    .and_then(|t| self.ctx.type_store.as_ref().map(|s| s.size_bytes(*t)))
                    .map(|b| b as u16)
                    .unwrap_or(0)
            })
            .collect();

        // 分配后回写：按 inst_ops 把 XReg 的分配结果填入微指令寄存器字段（物理 Reg）
        let mut global_inst = 0usize;
        for block in self.vcode.blocks_mut() {
            for inst in block.instructions.iter_mut() {
                if let Some(slot) = self.inst_ops.get(global_inst) {
                    for bind in slot.iter() {
                        let (xreg, field_idx) = (bind.vreg, bind.field);
                        if let Some(preg) = alloc_result.preg(xreg) {
                            // class：值宽类已由上面的折入步骤写进绑定（原实现现场从
                            // `xreg_types` 推，与 emission 的 spill 回填口径不同）；
                            // `None`（该 vreg 没有类型记录）退到 vreg 自身类。
                            let cls = bind.class.unwrap_or_else(|| xreg.class());
                            inst.set_reg_field(field_idx as usize, preg.num, cls);
                        }
                    }
                }
                global_inst += 1;
            }
        }
        let _ = std::mem::take(&mut self.param_xregs); // param_xregs 现在由 alloc_result 管理
        Ok(alloc_result)
    }
}

#[cfg(test)]
mod alloc_integration_tests {
    use super::*;
    use crate::arch::x86;

    /// 生成期 AVX-512 可行性开关的作用域守卫（Drop 时清除，**panic 安全**）。
    /// 必须与 `crate::AVX512_ENV_LOCK` 配对使用——env 只放开"能否发射 EVEX"
    /// 的生成期门；**执行** EVEX 的测试用 `avx512_hardware_available()` 判 skip
    /// （env 泄漏到运行时测试会真的执行 EVEX → STATUS_ILLEGAL_INSTRUCTION，
    /// 2026-09-12 实测 0xC000001D）。
    struct AssumeAvx512;

    impl AssumeAvx512 {
        fn set() -> Self {
            unsafe { std::env::set_var("FORGE_ASSUME_AVX512", "1") };
            Self
        }
    }

    impl Drop for AssumeAvx512 {
        fn drop(&mut self) {
            unsafe { std::env::remove_var("FORGE_ASSUME_AVX512") };
        }
    }

    fn build_sig(params: &[TypeId], ret: TypeId) -> FunctionSignature {
        FunctionSignature::new(&params.iter().map(|t| (*t, "")).collect::<Vec<_>>(), &[ret])
    }

    /// **C1 守卫**：聚合落点探针必须带**成员形状**（`TyView::from_ir_type`），否则
    /// AAPCS64/LP64D 的 HFA 会被当成"普通聚合"分流成 by-ref，而被调方按浮点寄存器收参
    /// ⇒ 静默错值（v23 M2 修复）。
    ///
    /// 判据用**同尺寸、不同成员**的一对类型对照——这是唯一能证明"成员真的进了探针"的
    /// 方式（只比 size 的实现对两者给同一答案）：
    ///
    /// - `{f64,f64,f64}`（24 B）是 HFA（aapcs64 `hfa_max = 4`）⇒ 浮点寄存器档；
    /// - `{i64,i64,i64}`（24 B）不是 HFA、且 > 16 B ⇒ 按引用（内存）档。
    #[test]
    fn aggregate_place_probe_carries_member_shapes() {
        crate::pipeline_hooks::ensure_registered();
        let tm = crate::arch::arm64::TargetMachine::new();
        let mut store = TypeStore::new();
        let f64ty = store.float_ty(64);
        let i64ty = store.int_ty(64);
        let hfa = store.struct_anon(vec![f64ty, f64ty, f64ty], false);
        let plain = store.struct_anon(vec![i64ty, i64ty, i64ty], false);
        assert_eq!(store.size_bytes(hfa), 24);
        assert_eq!(store.size_bytes(plain), 24);

        let (hfa_place, _) = agg_arg_place(&tm, "aapcs64", &store, hfa).expect("HFA 探针");
        assert_eq!(
            hfa_place,
            AggArgLoc::Registers,
            "aapcs64 的 24 字节 HFA 必须走寄存器档（探针丢了成员就会错判成 by-ref）"
        );
        let (plain_place, _) = agg_arg_place(&tm, "aapcs64", &store, plain).expect("普通聚合探针");
        assert_eq!(
            plain_place,
            AggArgLoc::ByRef,
            "同尺寸但非 HFA 的聚合按引用（内存）——两者答案必须不同，才说明形状带了成员"
        );
    }

    /// Phase 4.2：builder 构造 IR 走完整管线，断言 AllocResult.assignments 非空且合法。
    #[test]
    fn test_alloc_result_assignments() {
        x86::ensure_registered();
        let mut b = FunctionBuilder::new(
            "alloc_test",
            TypeContext::new(),
            build_sig(&[], TypeId::I64),
        );
        b.create_block_here();
        let a = b.iconst_i64(21);
        let c = b.iconst_i64(2);
        let s = b.imul(a, c);
        b.ret(&[s]);
        let func = b.finish().expect("build");
        let machine = x86::TargetMachine::new();
        let (cf, alloc) = CompileSession::new(machine)
            .compile_with_alloc(&func)
            .expect("compile_with_alloc");
        assert!(!cf.code.is_empty(), "编译产物非空");
        assert!(
            !alloc.assignments.is_empty(),
            "assignments 非空（有 XReg 被分配）"
        );
        for (i, preg) in alloc.assignments.iter().enumerate() {
            let Some(preg) = preg else { continue };
            let xreg = XReg::new(i as u32, preg.class);
            assert!(
                preg.class == RegClass::GPR64 || preg.class.is_fp(),
                "XReg {xreg:?} 分配类非法: {:?}",
                preg.class
            );
        }
    }

    /// Phase 4.3：参数 ABI 分配——f(i64×4) 的前 4 参数 XReg 分配到 RCX/RDX/R8/R9。
    #[test]
    fn test_param_abi_allocation() {
        x86::ensure_registered();
        let ctx = TypeContext::new();
        let sig = FunctionSignature::new(
            &[
                (ctx.i64_ty(), "a"),
                (ctx.i64_ty(), "b"),
                (ctx.i64_ty(), "c"),
                (ctx.i64_ty(), "d"),
            ],
            &[ctx.i64_ty()],
        );
        let mut b = FunctionBuilder::new("param_abi", ctx, sig);
        let (_entry, params) = b.create_entry_block();
        // 四个参数求和（强制全部活跃跨函数体）
        let s0 = b.iadd(params[0], params[1]);
        let s1 = b.iadd(s0, params[2]);
        let s2 = b.iadd(s1, params[3]);
        b.ret(&[s2]);
        let func = b.finish().expect("build");
        let machine = x86::TargetMachine::new();
        let (_cf, alloc) = CompileSession::new(machine)
            .compile_with_alloc(&func)
            .expect("compile_with_alloc");
        // 参数 XReg（入口收参）由分配器自由分配（非 precolor arg_regs）：
        // entry 收参 mov 把 [abi.arg_regs]（RCX/RDX/R8/R9）的值搬给参数 XReg，
        // 随后 XReg 可任意分配——分配器从 GPR 高编号起（r15 先），天然避开
        // div 的 RAX/RDX 等低编号 clobber 寄存器。ABI 正确性由收参 mov +
        // 执行测试保证；这里断言：所有参数 XReg 都有合法 GPR 分配。
        let param_pregs: Vec<u32> = alloc
            .param_vregs
            .iter()
            .map(|x| alloc.preg(*x).map(|p| p.num).unwrap_or(u32::MAX))
            .collect();
        assert_eq!(
            alloc.param_vregs.len(),
            4,
            "应有 4 个参数 XReg（收参），实际 {}",
            alloc.param_vregs.len()
        );
        assert!(
            param_pregs.iter().all(|&n| n < 16),
            "参数 XReg 应全部有合法 GPR 分配，实际 {param_pregs:?}"
        );
        // 参数分配不得与 div clobber 寄存器（RAX=0/RDX=2）冲突——若参数落在
        // 其上，div 序列的 clobber 会强制溢出（见写死保护测试）。自由分配
        // 从高编号起，低压力下参数集中在 r15-r12。
        assert!(
            !param_pregs.contains(&0) && !param_pregs.contains(&2),
            "参数不得分配到 RAX/RDX（div clobber 区），实际 {param_pregs:?}"
        );
    }

    /// Phase 4.4：写死保护——div 序列（clobber RAX/RDX）+ 16 值活跃压力下，
    /// 分配器必须产出完整分配（无 RegAlloc 错误、无未处理冲突），写死值不被覆盖。
    /// 执行语义正确性由 forge-tests 的 test_udiv_pressure（672）验证；此处断言
    /// 分配层完整性：每个分配/溢出的 XReg 都有合法去向。
    #[test]
    fn test_div_clobber_alloc_integrity() {
        x86::ensure_registered();
        let ctx = TypeContext::new();
        let sig = FunctionSignature::new(&[], &[ctx.i64_ty()]);
        let mut b = FunctionBuilder::new("div_clobber_alloc", ctx, sig);
        b.create_block_here();
        let mut acc = b.iconst_i64(0);
        for i in 1..=16i64 {
            let a = b.iconst_i64(i * 42);
            let c = b.iconst_i64(i);
            let d = b.udiv(a, c);
            acc = b.iadd(acc, d);
        }
        b.ret(&[acc]);
        let func = b.finish().expect("build");
        let machine = x86::TargetMachine::new();
        let (cf, alloc) = CompileSession::new(machine)
            .compile_with_alloc(&func)
            .expect("div + clobber 压力下编译必须成功（无 RegAlloc 错误）");
        assert!(!cf.code.is_empty(), "编译产物非空");
        assert!(!alloc.assignments.is_empty(), "必须有分配");
        // 写死保护：clobber 寄存器（RAX=0/RDX=2）上的分配不可能是"跨 div 序列
        // 活跃的中间值"——clobber 机制在序列点把它们溢出（spill_slots 记录）
        // 或分配器避开。断言：每个 spill 槽的 XReg 大小合法（spill 数据完整）。
        for (i, slot) in alloc.spill_slots.iter().enumerate() {
            let Some(slot) = slot else { continue };
            let xreg = XReg::new(i as u32, forge_ir::RegClass::GPR64);
            assert!(
                slot.size >= 4 && slot.size <= 16,
                "spill 槽大小非法: {xreg:?} -> {slot:?}"
            );
        }
    }

    /// P0-1 集成：`with_opt_level` 编译常量折叠函数——O1 管线把
    /// `iconst(21)*iconst(2)` 折叠为常量，编译产物仍正确（此前优化管线
    /// 在生产路径是死代码）。
    #[test]
    fn p0_compile_with_opt_level() {
        x86::ensure_registered();
        let mut b = FunctionBuilder::new(
            "opt_const_fold",
            TypeContext::new(),
            build_sig(&[], TypeId::I64),
        );
        b.create_block_here();
        let a = b.iconst_i64(21);
        let c = b.iconst_i64(2);
        let s = b.imul(a, c);
        b.ret(&[s]);
        let func = b.finish().expect("build");

        let machine = x86::TargetMachine::new();
        // 无优化编译（对照）
        let (cf0, _) = CompileSession::new(machine.clone())
            .compile_with_alloc(&func)
            .expect("compile 无优化");
        // O1 优化编译（常量折叠 21*2 → 42）
        let (cf1, _) = CompileSession::new(machine.clone())
            .with_opt_level(forge_opt::OptimizationLevel::O1)
            .compile_with_alloc(&func)
            .expect("compile 带 O1");
        assert!(!cf0.code.is_empty(), "无优化产物非空");
        assert!(!cf1.code.is_empty(), "O1 产物非空");
        // O1 常量折叠后指令更少（movabs 42 而非 movabs 21 + movabs 2 + imul）
        assert!(
            cf1.code.len() <= cf0.code.len(),
            "O1 常量折叠应不增大代码（P0-1 回归：优化管线未生效）"
        );
    }

    /// WA-37 D5 守卫：V512（64 字节）by-ref 参数的**被调方**收参，load 变体必须
    /// 按参数 IR 字节宽分派到 **64B（EVEX zmm）**。旧实现按参数 XReg 的寄存器类
    /// 宽分派（`reg_class_for` 对 >128 位恒给 `VEC(32)`）→ 64B 分支永不可达，
    /// 只从 [ptr] 拷 32B（lane8..15 丢失；原测试只断言 lane0 故未暴露）。
    ///
    /// 本机无 AVX-512F 时用 `FORGE_ASSUME_AVX512=1` 放开**可行性守卫**以验证
    /// 编码分派——本测试只编译不执行（EVEX 真执行需硬件）。
    #[test]
    fn test_v512_byref_callee_load_is_64b() {
        x86::ensure_registered();
        // 安全：仅本测试设置该 env（进程内其他测试只读；只影响能力判断）
        let _env_guard = crate::AVX512_ENV_LOCK
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        let _assume = AssumeAvx512::set();
        // 可见性事件：本用例恒跑（env 放开可行性门）——落盘以便 CI 日志佐证。
        crate::jit_event("V512-GEN", "test_v512_byref_callee_load_is_64b");
        // 类型必须建立在**同一个** TypeContext 上（跨上下文 TypeId 会越界：
        // 参考 jit.rs 的 V512 用例在本机因无 AVX-512 直接 return，其体从未跑到）。
        let tc = TypeContext::new();
        let vt = tc.vector_ty(TypeId::F32, 16); // 16 × f32 = 64 字节 = V512
        let sig = FunctionSignature::new(&[(vt, "v")], &[TypeId::I32]);
        let mut b = FunctionBuilder::new("callee_v512", tc, sig);
        let (blk, p) = b.create_block_with_params(&[(vt, "v")]);
        b.switch_to_block(blk);
        let idx = b.iconst_i32(0);
        let lane = b.vextract(p[0], idx);
        let wide = b.fpext(lane, TypeId::F64);
        let int = b.fptosi(wide, TypeId::I32);
        b.ret(&[int]);
        let func = b.finish().expect("build v512 callee");
        let cf = CompileSession::new(x86::TargetMachine::new())
            .compile(&func)
            .expect("compile v512 callee（FORGE_ASSUME_AVX512 下应通过守卫）");
        // EVEX 前缀（62）+ EVEX 编码的 0F 10 家族；32B 变体是 3 字节 VEX
        // （C4 P0 P1 10，P1 = W0/vvvv=1111/L=1/pp=00 → 0x7C）——本编码器恒发
        // 3 字节 VEX，故负向断言按 C4 形态匹配（旧的 `C5 FC 10` 恒不命中 =
        // 空断言，2026-09-12 W3 修正）。
        let has_evex_load = cf.code.windows(5).any(|w| w[0] == 0x62 && w[4] == 0x10);
        let has_vex_ymm_load = cf
            .code
            .windows(4)
            .any(|w| w[0] == 0xC4 && w[2] == 0x7C && w[3] == 0x10);
        assert!(
            has_evex_load,
            "V512 收参应含 EVEX zmm load（62 ... 10）：D5 回归（按类宽分派 → 只拷 32B）。\
             prologue bytes = {:02x?}",
            &cf.code[..cf.code.len().min(48)]
        );
        assert!(
            !has_vex_ymm_load,
            "V512 收参不得退回 32B VEX.256 ymm load（C4 .. 7C 10，lane8..15 丢失）"
        );
    }

    /// V512（64 字节）`Vconst` 的**生成级**守卫（不需要 AVX-512 硬件：只编译不执行）。
    ///
    /// 背景：`isa/x86.toml` 的 `Vconst` 规则原只覆盖 `rd = 64/128/256`，V512
    /// 常量落到「no matching rule」→ `Unsupported`。该缺口只在**有 AVX-512F 的机器**
    /// 上暴露（`runtime::jit` 的 `test_jit_v512_byref_param` 无 AVX-512F 时提前
    /// return，本机即如此），历史上因此在 CI 上偶发红（Test (Windows) 命中
    /// AVX-512 runner 的 run）。本测试用「**无宽向量参数/返回** + 函数内局部 V512
    /// 常量」构造——不触发宽向量 ABI 的 AVX-512 入口守卫，任何机器都能编译验证。
    ///
    /// 断言：①编译成功（修复前 `Unsupported("DSL lowering: Vconst no matching rule")`）；
    /// ②机器码含 **4 条 EVEX `VINSERTF32X4`**（`62 P0 P1 P2 18 /r ib`）且 imm 恰为
    /// 0/1/2/3 各一次 ⇒ 512 位的 4 个 128 位 lane 都被显式写入（这也是 Vconst 规则
    /// 「`{out}` 自身当累加器、初始值不影响结果」这一构造前提的可执行证据）。
    ///
    /// W3（2026-09-12）起：64B 向量值（含 Vconst）统一由编译入口的字节门守卫
    /// 「>32B 需 AVX-512F」——无硬件时用 `FORGE_ASSUME_AVX512=1` 放开可行性门
    /// （本例只编译不执行，EVEX 真执行由有硬件的 runner 守护）。
    #[test]
    fn test_v512_vconst_generates_four_evex_inserts() {
        x86::ensure_registered();
        // 安全：仅本测试设置该 env（AVX512_ENV_LOCK 串行化，见 lib.rs 说明）。
        let _env_guard = crate::AVX512_ENV_LOCK
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        let _assume = AssumeAvx512::set();
        // 可见性事件：本用例恒跑（env 放开生成期门）——落盘以便 CI 日志佐证。
        crate::jit_event("V512-GEN", "test_v512_vconst_generates_four_evex_inserts");
        let tc = TypeContext::new();
        let sig = FunctionSignature::new(&[], &[TypeId::I32]);
        let mut b = FunctionBuilder::new("v512_vconst", tc, sig);
        b.create_block_here();
        // 16 × f32 = 64 字节 = V512（类型由 T+len 推导）。
        let v = b.vconst(vec![
            1.5f32, 2.0, 3.0, 4.0, 5.0, 6.0, 7.0, 8.0, 9.0, 10.0, 11.0, 12.0, 13.0, 14.0, 15.0,
            16.0,
        ]);
        // 取 lane15（=16.0）——与运行级 V512 用例同形，便于两处对照。
        let idx = b.iconst_i32(15);
        let lane = b.vextract(v, idx);
        let wide = b.fpext(lane, TypeId::F64);
        let int = b.fptosi(wide, TypeId::I32);
        b.ret(&[int]);
        let func = b.finish().expect("build v512 vconst");
        let cf = CompileSession::new(x86::TargetMachine::new())
            .compile(&func)
            .expect("V512 常量应可降级（修复前：Vconst no matching rule）");
        // EVEX = 62 + P0 P1 P2 + opcode：窗口 [62, P0, P1, P2, opcode, modrm, imm]。
        let mut imms: Vec<u8> = Vec::new();
        for w in cf.code.windows(7) {
            if w[0] == 0x62 && w[4] == 0x18 {
                imms.push(w[6]);
            }
        }
        imms.sort_unstable();
        assert_eq!(
            imms,
            vec![0u8, 1, 2, 3],
            "V512 常量应由 4 条 EVEX VINSERTF32X4 拼成、imm = 0/1/2/3 各一次（4 个 lane 全写）。\
             实际 imm = {imms:?}；code = {:02x?}",
            &cf.code[..cf.code.len().min(64)]
        );
    }

    /// 构造并编译 `${lanes}×f32` 向量的**栈槽往返**函数（W3 宽向量 Load/Store
    /// 用例共用）：vconst → alloca → store → load → vextract lane0 → i32。
    fn compile_wide_slot_roundtrip(lanes: usize) -> Result<CompiledFunction, IrError> {
        let tc = TypeContext::new();
        let vt = tc.vector_ty(TypeId::F32, lanes as u32);
        let sig = FunctionSignature::new(&[], &[TypeId::I32]);
        let mut b = FunctionBuilder::new("wide_vec_slot", tc, sig);
        let (blk, _p) = b.create_block_with_params(&[]);
        b.switch_to_block(blk);
        let v = b.vconst(vec![1.0f32; lanes]);
        let slot = b.alloca(vt, 1);
        b.store(v, slot);
        let back = b.load(slot, vt);
        let idx = b.iconst_i32(0);
        let lane = b.vextract(back, idx);
        let wide = b.fpext(lane, TypeId::F64);
        let int = b.fptosi(wide, TypeId::I32);
        b.ret(&[int]);
        let func = b.finish().expect("build wide vector slot roundtrip");
        CompileSession::new(x86::TargetMachine::new()).compile(&func)
    }

    /// W3（2026-09-12）：V256（32B）向量的 **IR 层 Load/Store 已可降级**——
    /// ISA 规则 `rd_vec/rs1_vec = 32` → `VMOVUPS_RM/MR`（VEX.256，`vex_l=1`；
    /// 槽类 `fpr16` 的寄存器编号即 ymm 号）。旧实现把 >16B 的 Load/Store 一律
    /// fail-closed 拒绝（"ISA 类模型缺 YMM 槽类"）——本轮由规则 + 编译入口字节门
    /// （>32B 才要 AVX-512F）共同承载。
    ///
    /// 只编译不执行（运行级 lane 往返见 `runtime::jit::test_jit_v256_slot_roundtrip`）。
    /// 断言机器码含 **VEX.256** 的 0F 10/11（vmovups ymm）：本编码器恒发 3 字节
    /// VEX（`C4 P0 P1`），P1 = W=0/vvvv=1111/L=1/pp=00 → `0x7C`（仅 R̄/X̄/B̄ 在 P0）。
    #[test]
    fn test_v256_slot_load_store_is_lowered() {
        x86::ensure_registered();
        let cf = compile_wide_slot_roundtrip(8).expect("32B 向量槽往返应可降级（W3）");
        let is_vex256 = |w: &[u8], op: u8| w[0] == 0xC4 && w[2] == 0x7C && w[3] == op;
        assert!(
            cf.code.windows(4).any(|w| is_vex256(w, 0x10)),
            "32B load 应为 VEX.256 vmovups ymm, [base]（C4 .. 7C 10）：code = {:02x?}",
            &cf.code[..cf.code.len().min(64)]
        );
        assert!(
            cf.code.windows(4).any(|w| is_vex256(w, 0x11)),
            "32B store 应为 VEX.256 vmovups [base], ymm（C4 .. 7C 11）：code = {:02x?}",
            &cf.code[..cf.code.len().min(64)]
        );
        // 反向守卫：32B 槽往返**不得**只搬 8 字节（旧默认规则只会发 movsd
        // F3 0F 10/11 → lane1..7 静默截断）。
        assert!(
            !cf.code
                .windows(3)
                .any(|w| w == [0xF3, 0x0F, 0x10] || w == [0xF3, 0x0F, 0x11]),
            "32B 槽往返不得退回 movsd（8 字节静默截断）：code = {:02x?}",
            &cf.code[..cf.code.len().min(64)]
        );
    }

    /// W3（2026-09-12）：V512（64B）向量的 IR 层 Load/Store——**需 AVX-512F**
    /// （EVEX vmovups zmm）。分两半：①无 AVX-512F 的机器上必须**编译期拒绝**
    /// （与 ABI 守卫同一判据，不发射本机无法执行的 EVEX）；②`FORGE_ASSUME_AVX512=1`
    /// 放开可行性门后成功降级且含 EVEX 0F 10/11（只编译不执行）。
    ///
    /// 本机（无 AVX-512F）两半都跑到；有 AVX-512F 的机器跳过①（无从构造"无
    /// 能力"场景）——②由生成级断言守护编码。
    #[test]
    fn test_v512_slot_load_store_requires_avx512() {
        x86::ensure_registered();
        // 安全：仅本测试设置该 env（与 D5 用例共用 AVX512_ENV_LOCK 串行化）。
        let _env_guard = crate::AVX512_ENV_LOCK
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        if crate::avx512_hardware_available() {
            // 可见性事件：本机是否有 AVX-512F 决定这一半是否跑到（libtest 吞掉
            // 通过测试的输出 → 只能靠 `FORGE_JIT_EVENTS` 落盘判断）。
            crate::jit_event("AVX512-HW=1", "test_v512_slot_load_store_requires_avx512");
            eprintln!("[w3] 本机有 AVX-512F——跳过「无能力必须拒绝」一半（无从构造）");
        } else {
            crate::jit_event("AVX512-HW=0", "test_v512_slot_load_store_requires_avx512");
            let err = match compile_wide_slot_roundtrip(16) {
                Ok(_) => panic!("无 AVX-512F 时 64B 向量必须编译期拒绝（不得发射 EVEX）"),
                Err(e) => e,
            };
            let msg = format!("{err}");
            assert!(
                msg.contains("64 字节宽向量") && msg.contains("宽向量档位"),
                "64B：错误信息应说明字节数与「本机宽向量档位」这一判据：{msg}"
            );
        }
        let _assume = AssumeAvx512::set();
        let cf = compile_wide_slot_roundtrip(16)
            .expect("FORGE_ASSUME_AVX512 下 64B 向量槽往返应可降级（W3）");
        // EVEX：62 + P0 P1 P2 + opcode（窗口 [62, P0, P1, P2, opcode, modrm]）；
        // P2 的 L'L = 10（bits 6:5）= 512 位宽。
        let has_evex = |op: u8| {
            cf.code
                .windows(5)
                .any(|w| w[0] == 0x62 && w[4] == op && (w[3] & 0x60) == 0x40)
        };
        assert!(
            has_evex(0x10),
            "64B load 应为 EVEX vmovups zmm, [mem]（62 ... 10）：code = {:02x?}",
            &cf.code[..cf.code.len().min(64)]
        );
        assert!(
            has_evex(0x11),
            "64B store 应为 EVEX vmovups [mem], zmm（62 ... 11）：code = {:02x?}",
            &cf.code[..cf.code.len().min(64)]
        );
    }

    /// **WA-46 宽度分档守卫（类映射）**：向量值的寄存器类必须按**值宽**分档——
    /// ≤16B → `VEC(128)`、≤32B → `VEC(256)`、>32B → **`VEC(512)`**。旧实现 >128 位
    /// 一律 `VEC(256)`，而类的 `reg_width` 决定 spill 槽大小与 `xreg.width()`
    /// ⇒ 64B 的 V512 值只有 32B 槽、搬运宽度也只报 32（溢出后高半区静默截断）。
    /// 这里同时锁住链路：`VEC(512).bytes() == 64`（类表 fallback 用
    /// `class.bytes()` 作 `reg_width`，见上方 classes 构建）。
    #[test]
    fn test_vector_reg_class_widths() {
        let tc = TypeContext::new();
        let v64 = tc.vector_ty(TypeId::F32, 2);
        let v128 = tc.vector_ty(TypeId::F32, 4);
        let v256 = tc.vector_ty(TypeId::F32, 8);
        let v512 = tc.vector_ty(TypeId::F32, 16);
        let mut ctx = LowerCtx::new();
        ctx.type_store = Some(tc.borrow());
        assert_eq!(ctx.reg_class_for(&v64), RegClass::VEC(128));
        assert_eq!(ctx.reg_class_for(&v128), RegClass::VEC(128));
        assert_eq!(ctx.reg_class_for(&v256), RegClass::VEC(256));
        assert_eq!(
            ctx.reg_class_for(&v512),
            RegClass::VEC(512),
            "64 字节向量必须归 VEC(512)（槽宽/搬运宽度按值宽；WA-46）"
        );
        assert_eq!(
            RegClass::VEC(512).bytes(),
            64,
            "VEC(512) 的 reg_width 必须是 64 字节（spill 槽 = 64 字节）"
        );
    }

    /// **WA-46 溢出宽度守卫（生成级，直接驱动 FrameLowering）**：FPR 溢出
    /// 必须按 `width` 搬运——旧实现只有一份 8 字节 `MOVSD` 模板且生成的
    /// `emit_spill_load/store` 忽略 width ⇒ 向量值 spill 只搬低 8 字节
    /// （高半区静默截断）。用例绕过分配器（高压力下分配器会因 3-scratch 限制
    /// 拒绝，测不到该路径），直接对每个宽度检查发射的机器码形状。
    #[test]
    fn test_fpr_spill_width_dispatch() {
        use crate::machine::frame::TargetFrameLowering;
        x86::ensure_registered();
        let fl = x86::FrameLowering;
        let emit = |width: u16, is_load: bool| -> Result<Vec<u8>, IrError> {
            let mut sink = crate::pipeline::emit::CodeSink::new();
            if is_load {
                fl.emit_spill_load(0, -32, width, true, &mut sink)?;
            } else {
                fl.emit_spill_store(0, -32, width, true, &mut sink)?;
            }
            Ok(sink.bytes().to_vec())
        };
        // 8 字节标量：movsd（F2 0F 10/11）
        let s8 = emit(8, false).expect("8B store");
        assert_eq!(&s8[..3], &[0xF2, 0x0F, 0x11], "8B spill store 应为 movsd");
        let l8 = emit(8, true).expect("8B load");
        assert_eq!(&l8[..3], &[0xF2, 0x0F, 0x10], "8B spill load 应为 movsd");
        // 16 字节：movups（0F 10/11，无 F2/F3 前缀）
        let s16 = emit(16, false).expect("16B store");
        assert_eq!(
            &s16[..2],
            &[0x0F, 0x11],
            "16B spill store 应为 movups（0F 11）"
        );
        let l16 = emit(16, true).expect("16B load");
        assert_eq!(
            &l16[..2],
            &[0x0F, 0x10],
            "16B spill load 应为 movups（0F 10）"
        );
        // 32 字节：VEX.256 vmovups（C4 .. 7C 10/11）
        let s32 = emit(32, false).expect("32B store");
        assert!(
            s32.len() >= 4 && s32[0] == 0xC4 && s32[2] == 0x7C && s32[3] == 0x11,
            "32B spill store 应为 VEX.256 vmovups（C4 .. 7C 11）：{s32:02x?}"
        );
        let l32 = emit(32, true).expect("32B load");
        assert!(
            l32.len() >= 4 && l32[0] == 0xC4 && l32[2] == 0x7C && l32[3] == 0x10,
            "32B spill load 应为 VEX.256 vmovups（C4 .. 7C 10）：{l32:02x?}"
        );
        // 64 字节：EVEX.512 vmovups（62 .. 10/11，P2 的 L'L = 10）
        let s64 = emit(64, false).expect("64B store");
        assert!(
            s64.len() >= 5 && s64[0] == 0x62 && s64[4] == 0x11 && (s64[3] & 0x60) == 0x40,
            "64B spill store 应为 EVEX zmm vmovups（62 .. 11）：{s64:02x?}"
        );
        let l64 = emit(64, true).expect("64B load");
        assert!(
            l64.len() >= 5 && l64[0] == 0x62 && l64[4] == 0x10 && (l64[3] & 0x60) == 0x40,
            "64B spill load 应为 EVEX zmm vmovups（62 .. 10）：{l64:02x?}"
        );
        // 其它宽度 → fail-closed（不静默退回窄搬运）。
        for w in [24u16, 48] {
            let e = emit(w, false).expect_err("未声明宽度的 store 必须报错");
            let msg = format!("{e}");
            assert!(
                msg.contains("spill"),
                "{w}B：错误信息应说明 spill 宽度缺失：{msg}"
            );
            assert!(emit(w, true).is_err(), "{w}B：load 同样必须 fail-closed");
        }
    }
}
