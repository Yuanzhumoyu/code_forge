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
use smallvec::SmallVec;

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

    /// 并入另一批的变化量（v23 收尾 M7）：入口把合法化拆成"聚合族 + va"两批跑
    /// （ABI 槽必须在聚合族**之后**排布），所以账要合起来报。
    pub fn merge(&mut self, other: Self) {
        self.new_insts += other.new_insts;
        self.new_values += other.new_values;
        self.new_consts += other.new_consts;
        self.sret |= other.sret;
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

/// **一个聚合值在 IR 层的位置**（v24 M1：唯一表示）。
///
/// 与旧的 `Places` 的区别：① 显式表达"注意：段值"这半边（旧表只记内存那半边，
/// 段值靠调用点另传一个 `segs: &[Value]` ⇒ 同一个值的表示**散在两个参数里**）；
/// ② 语义更窄——`Mem` 只表示"值住在这些字节里"，不再兼职"槽类型"（类型从值的 IR 类型取）。
#[derive(Debug, Clone)]
pub(crate) enum Place {
    /// 段值：住在 `n` 个 `i64` 值里（`n = ceil(size/8)`）——寄存器档的 ABI 边界。
    Segs(SmallVec<[Value; 2]>),
    /// 内存：住在某个 `Ptr` 值指向的字节里（帧槽 / 调用方副本 / `[fp + offset]`）。
    Mem(Value),
}

/// **位置表**：键 `Value` → [`Place`]，**每条键只允许写一次**。
///
/// 写一次是硬约束而不是约定：旧的 `Places` 由多个 pass 陆续写入、后写覆盖先写，
/// 于是"谁先跑"变成了语义（v23 的 pass 次序脆弱性根因）。第二次写必然意味着
/// 计划或物化有 bug，因此 debug 构建直接 panic。
#[derive(Debug, Default)]
pub(crate) struct Places {
    map: SecondaryMap<Value, Place>,
}

impl Places {
    /// 登记一个值的位置。**同一值第二次登记 = bug**（debug 下 panic）。
    pub fn set(&mut self, v: Value, p: Place) {
        debug_assert!(
            self.map.get(v).is_none(),
            "位置表：值 {v:?} 被登记了两次（先 {:?}）——同一值只能有一个位置",
            self.map.get(v)
        );
        self.map.insert(v, p);
    }

    /// 查询（`None` = 这个值没有"内存位置"，按普通标量/段值处理）。
    pub fn get(&self, v: Value) -> Option<&Place> {
        self.map.get(v)
    }

    /// 值**是否已登记位置**（任何种类）。
    pub fn contains(&self, v: Value) -> bool {
        self.map.get(v).is_some()
    }

    /// 值的**内存地址**（只有 [`Place::Mem`] 有）；`None` = 不在内存里（段值 / 未登记）。
    ///
    /// 这是旧代码里 `agg_slots.get(v).map(|&(a, _)| a)` 的**唯一替代**——把"取地址"这件事
    /// 收成一个访问器，免得各处再各写一遍模式匹配。
    pub fn addr(&self, v: Value) -> Option<Value> {
        match self.map.get(v) {
            Some(Place::Mem(a)) => Some(*a),
            _ => None,
        }
    }
}

/// 使用位置（重写任务队列条目——block/pos/inst 定位使用指令）。
pub(crate) struct UsePos {
    pub block: Block,
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

// rewrite 族（第四十四轮记录的"第三批"）已于那轮迁入：memoryize_from_segs /
// 旧的三件：按地址内存化、按值驱动的就地改写、以及"生成后搬到使用点前"。
// **v23 M6 批次 2+3+4 之后，`expand_*` 执行族与共享件也全部迁出 `compiler.rs`**——现在住在
// `pipeline/legalize/`（`agg_call.rs`/`agg_store.rs`/`agg_param.rs`/`gep.rs` + `common.rs`），
// 由 `LegalizeDriver` 定序调用；`compiler.rs` 只留调用点。

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
    agg_slots: &mut Places,
) -> Result<(), IrError> {
    // 取一次读锁（v3 S3 读路径纪律）：本函数只读类型，内部不再逐次取锁。
    // 先从 func.types clone 出 TypeContext（Arc，O(1)）再取锁——守卫借的是这个局部，
    // 不与 func 的 &mut 借用冲突。
    let types_ctx = func.types.clone();
    let store = types_ctx.borrow();
    // **非对齐字段**（`off % 8 != 0`）现在也支持（v23 M6 缺口泛化）：段值先按 `off % 8`
    // 字节**移位拼接**再写槽——以前直接 `Unsupported`（"off={off} 暂不支持"），于是
    // `{i8, {i32}}` 这类字段偏移不是 8 倍数的聚合根本展开不了。
    let shift_bytes = (off % 8) as u32;
    let size = store.size_bytes(seg_ty) as usize;
    let n_segs = size.div_ceil(8);
    let start = off / 8;
    if start + n_segs > segs.len() {
        return Err(IrError::Internal("嵌套聚合字段提取：段值不足".into()));
    }
    // 1) 分配帧槽（Alloca 指令——预扫描分配帧偏移，lowering 经 lea_off 取址）
    // **immediate 必须是规范形态** `Type(elem) + Uint(count)`：帧槽预扫描
    //（`frame_slots.rs`）只按这两个 immediate 算字节数，写 `Const(池 id)` 会让这个
    // Alloca **不进槽表** ⇒ 没有帧偏移 ⇒ 地址是垃圾（实测：往那儿写 24 字节会毁帧崩溃）。
    // v23 M6：规范 immediate 形态的唯一出口（`legalize::common::place_addr_of_slot`）。
    let (_alloca, slot) =
        crate::pipeline::legalize::common::place_addr_of_slot(func, use_pos.block, size);
    // 2) 段值 → 槽：对齐时逐段直存；非对齐时每段先拼好再存。
    //
    // 拼接用**移位 + 加法**（`(lo >> 8r) + (hi << (64-8r))`）：两半位域不相交 ⇒ 加法与
    // 按位或等价，而 `Ior` 在三份谱里都没有 lowering（`Ishl`/`Ushr` x86 与 riscv64 都有）。
    // 取 `hi` 时严格按"这一段的字节是否落在字段内"判，越界/超出字段就补 0（不读别人的字节）。
    let mut cur = slot;
    let const_i64 = |func: &mut Function, v: i64| -> Value {
        let cid = func.constants.intern_int(i128::from(v), 64);
        let ci = func.make_inst(
            Opcode::Iconst,
            use_pos.block,
            smallvec::smallvec![],
            smallvec::smallvec![Immediate::Const(cid)],
            &[TypeId::I64],
            InstFlags::NONE,
        );
        func.dfg.inst_data(ci).results[0]
    };
    for i in 0..n_segs {
        let lo_src = segs[start + i];
        let stored = if shift_bytes == 0 {
            lo_src
        } else {
            let sh = u64::from(shift_bytes * 8);
            let lo_k = const_i64(func, sh as i64);
            let lo = func.make_inst(
                Opcode::Ushr,
                use_pos.block,
                smallvec::smallvec![lo_src, lo_k],
                smallvec::smallvec![],
                &[TypeId::I64],
                InstFlags::NONE,
            );
            let lo_v = func.dfg.inst_data(lo).results[0];
            // 这一段还要别人的高位吗？字段覆盖到第 i+1 段才要。
            let hi_v = if (i + 1) * 8 < shift_bytes as usize + size {
                let hi_src = match segs.get(start + i + 1) {
                    Some(v) => *v,
                    // 字段末端就在这一段内 ⇒ 高位补 0（不读相邻聚合成员的字节）。
                    None => const_i64(func, 0),
                };
                let inv_k = const_i64(func, (64 - sh) as i64);
                let hi = func.make_inst(
                    Opcode::Ishl,
                    use_pos.block,
                    smallvec::smallvec![hi_src, inv_k],
                    smallvec::smallvec![],
                    &[TypeId::I64],
                    InstFlags::NONE,
                );
                func.dfg.inst_data(hi).results[0]
            } else {
                const_i64(func, 0)
            };
            let add = func.make_inst(
                Opcode::Iadd,
                use_pos.block,
                smallvec::smallvec![lo_v, hi_v],
                smallvec::smallvec![],
                &[TypeId::I64],
                InstFlags::NONE,
            );
            func.dfg.inst_data(add).results[0]
        };
        func.make_inst(
            Opcode::Store,
            use_pos.block,
            smallvec::smallvec![stored, cur],
            smallvec::smallvec![],
            &[],
            InstFlags::SIDE_EFFECT,
        );
        if i + 1 < n_segs {
            // 先把常量塞进池（`func.make_inst` 会借整个 func，参数里不能再借 constants）
            let c8 = func.constants.intern_int(8, 64);
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
    agg_slots.set(func.dfg.inst_data(__tomb).results[0], Place::Mem(slot));
    let _ = seg_ty;
    Ok(())
}

// 旧的就地改写三件套已随 v24 M1 **退场**（删除清单与证据见 docs/plans/backend-v24-plan.md）：
// 使用处改写现在由 `legalize::materialize` 按指令序**单遍**完成——它自己构造块内指令序
// 一次写回，生成指令天生排在消费者之前，因此不需要"生成到块尾再搬"（那个"漏搬一次"
// 正是 v23 收尾 use-before-def 真凶的形状）。
// `memoryize_from_segs` 留着：段值档的嵌套聚合字段没有地址，仍需要它开槽写入。

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
        let one = b.const_int(1, TypeId::I64);
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
        let one = b.const_int(1, TypeId::I64);
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

    /// **非对齐嵌套字段**（`off % 8 != 0`）现在能展开（v23 M6 缺口泛化）。
    ///
    /// 以前这里直接 `Unsupported`（"非对齐字段暂不支持"）⇒ `{i8, {i32,i32}}` 这类字段偏移
    /// 不是 8 倍数的聚合整类编译不了。现在段值先按 `off % 8` 字节移位拼接再写槽。
    /// 这条用例钉两件事：① 不再报错；② **确实生成了移位拼接**——否则就是"悄悄按对齐处理"，
    /// 数据会整体错位，那比报错更糟。
    #[test]
    fn unaligned_nested_field_is_repacked_with_shifts() {
        use super::{Places, UsePos};
        use forge_ir::{Opcode, TypeId};

        let ctx = TypeContext::new();
        // 内层聚合：{i32, i32}（8 字节），外层把它放在偏移 4（非 8 倍数）。
        let inner = ctx
            .borrow_mut()
            .struct_anon(vec![TypeId::I32, TypeId::I32], false);
        let sig = FunctionSignature::new(&[(TypeId::I64, "a")], &[inner]);
        let mut b = FunctionBuilder::new("unaligned_nested", ctx, sig);
        let (e, params) = b.create_block_with_params(&[(TypeId::I64, "a")]);
        b.switch_to_block(e);
        // 两个段值（i64）与一个"提取点"指令（有结果——函数会把它墓碑化并登记结果值）。
        let s0 = params[0];
        let one = b.constant(1i64);
        let s1 = b.iadd(s0, one);
        let _extract = b.iadd(s0, s1);
        b.ret(&[s0]); // 每个块必须有终结符（builder 的契约）
        let mut f = b.finish().expect("build");
        // "提取点"必须是**有结果**的指令（函数会把它的 results[0] 登记进 agg_slots）；
        // `ret` 没有结果，所以按结果非空挑最后一条。
        let inst = f
            .dfg
            .insts()
            .filter(|(_, d)| !d.results.is_empty())
            .map(|(i, _)| i)
            .last()
            .expect("至少有一条带结果的指令");

        let use_pos = UsePos { block: e, inst };
        let mut agg_slots = Places::default();
        super::memoryize_from_segs(&mut f, &use_pos, inner, 4, &[s0, s1], &mut agg_slots)
            .expect("非对齐嵌套字段必须能展开（以前是 Unsupported）");

        let ops: Vec<Opcode> = f.dfg.insts().map(|(_, d)| d.opcode).collect();
        assert!(
            ops.contains(&Opcode::Ushr),
            "非对齐路径必须生成右移拼接（否则是按对齐直存 ⇒ 数据错位）：{ops:?}"
        );
        assert!(
            ops.contains(&Opcode::Ishl),
            "非对齐路径必须生成左移拼接：{ops:?}"
        );
    }

    /// **`va_arg` 产生的聚合必须让"要不要合法化"为真**（v23 收尾 M7 实测抓到）：
    /// 真执行用例（`test_jit_va_arg_reads_unnamed_aggregate_arg`）里，`va_arg(聚合)` +
    /// 后续 `extractvalue` 的函数**没被合法化** ⇒ `ExtractValue` 掉到 lowering ⇒
    /// `Unsupported("DSL lowering")`。根因就在这个判据上。
    #[test]
    fn va_arg_of_aggregate_needs_agg_expand() {
        use forge_ir::ir::builder::FunctionBuilder;
        use forge_ir::ir::types::{FunctionSignature, TypeContext};
        use forge_ir::{Opcode, TypeId};

        let ctx = TypeContext::new();
        let agg = ctx
            .borrow_mut()
            .struct_anon(vec![TypeId::I64, TypeId::I64], false);
        let sig =
            FunctionSignature::new(&[(TypeId::I64, "fmt")], &[TypeId::I64]).with_variadic(true);
        let mut b = FunctionBuilder::new("callee", ctx, sig);
        let (e, _p) = b.create_block_with_params(&[(TypeId::I64, "fmt")]);
        b.switch_to_block(e);
        let va = b.va_start();
        let a = b.va_arg(va, agg);
        let f0 = b.extract_value(a, 0);
        b.ret(&[f0]);
        let f = b.finish().expect("build");
        assert!(
            super::needs_agg_expand(&f),
            "`va_arg` 的结果是聚合 ⇒ 必须判为\"要合法化\"（否则 ExtractValue 落到 lowering）"
        );
        assert!(
            f.dfg.insts().any(|(_, d)| d.opcode == Opcode::VaArg),
            "夹具里应有 VaArg"
        );
    }
}
