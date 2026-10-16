//! 合法化 pass 之间的**共享件**（v23 M6 批次 4）。
//!
//! 这些原本住在 `compiler.rs`、由迁出的 pass 反向 `use` 回来——方向是反的：它们只服务
//! 合法化，与编译管线的其余部分无关。搬到这里之后 `compiler.rs` 不再被 legalize 依赖。

use crate::IrError;
use crate::pipeline::agg_expand::{UseJob, UseKind};
use forge_ir::*;

// 旧的手抄落点枚举与两个单类型探针已随 v25 M1 **退场**：落点分类现在只有一处——
// `legalize/classify.rs` 的 `Classifier`（返回 `forge-abi-model` 的规范类型
// `Placement`/`RetLoc`，并按类型去重）。

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
