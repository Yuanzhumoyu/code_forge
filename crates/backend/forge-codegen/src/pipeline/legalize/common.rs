//! 合法化 pass 之间的**共享件**（v23 M6 批次 4）。
//!
//! 这些原本住在 `compiler.rs`、由迁出的 pass 反向 `use` 回来——方向是反的：它们只服务
//! 合法化，与编译管线的其余部分无关。搬到这里之后 `compiler.rs` 不再被 legalize 依赖。

use crate::IrError;
use crate::pipeline::agg_expand::{UseJob, UseKind};
use forge_ir::*;

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(crate) enum AggArgLoc {
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
pub(crate) fn agg_arg_place(
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
pub(crate) fn agg_ret_is_sret(
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
pub(crate) fn append_param(
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

/// 大聚合 load 使用展开任务（S1：UseJob 提升为模块级——collect_extract_jobs 复用）
/// S1：收集新登记聚合值（嵌套提取中间值）的 extractvalue 使用，追加到展开
/// 任务队列（load 索引沿用原值——loads 在展开中不修改）。
pub(crate) fn collect_extract_jobs(
    func: &Function,
    jobs: &mut Vec<UseJob>,
    res: Value,
    load_idx: usize,
) {
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

/// GEP 无基址操作数时的占位值（正常 IR 不会出现——防御性兜底）。
///
/// `Block(0)` 在这里只是"必须给一个块句柄"的**语法占位**（`ValueDef::Param`
/// 要求块 + 下标），**不是**"入口是 0 号块"的约定——该值不参与任何支配/
/// CFG 判断（GEP 展开只把它当 PTR 值用）。入口约定请用 `Function::entry()`。
pub(crate) fn make_placeholder_ptr(func: &mut Function) -> Value {
    func.dfg
        .make_value(TypeId::PTR, ValueDef::Param(Block::new(0), 0))
}

/// **栈缓冲取址的唯一出口**（v23 M6 去重）。
///
/// `Alloca(Type(I8), Uint(n))` 是帧槽预扫描（`frame_slots.rs`）**唯一**认得的规范 immediate
/// 形态——写 `Const(池 id)` 会让槽根本不进表（没有偏移，后续 store/load 全错位）。这个形态
/// 原来在两个 pass 里各写一遍，口径散在两处；现在只有这里能造。
///
/// 返回 `(alloca_inst, addr_value)`。**调用方负责把它移到正确的块位置**（SSA 支配：
/// 缓冲的取址必须先于用它的那条指令——由物化相在构造块内指令序时保证）。
pub(crate) fn place_addr_of_slot(func: &mut Function, block: Block, size: usize) -> (Inst, Value) {
    let alloca = func.make_inst(
        Opcode::Alloca,
        block,
        smallvec::smallvec![],
        smallvec::smallvec![Immediate::Type(TypeId::I8), Immediate::Uint(size as u64)],
        &[TypeId::PTR],
        InstFlags::NONE,
    );
    let addr = func.dfg.inst_data(alloca).results[0];
    (alloca, addr)
}
