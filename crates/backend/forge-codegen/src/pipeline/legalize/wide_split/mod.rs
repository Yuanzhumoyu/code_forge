//! 合法化 pass：**宽值的多寄存器表示**（v27 M5 · M-F）。
//!
//! 一个宽整数（`bits > int_widths().max()`）在 IR 里不再是一个值，而是 **n 个原生宽 SSA 值**
//!（肢，`n = ceil(bits / W)`，下标 0 = 最低肢）。这样 isel 永远只看到原生宽度。
//!
//! 为什么不"拆半再递归"（LLVM 的 `ExpandIntegerResult` 做法）：本 IR 没有整数子区间
//! 提取/拼接（`bitcast [i64;2] ↔ i128` 实测 `[cap:SPEC_LOWERING_RULE_MISSING]`），所以
//! 只能**一次拆到位**。逐 op 的实现就是一个 `for k in 0..n` 循环，`n = 2` 即 i128 形态。
//!
//! **与 `wide_mem` 的关系**：本 pass 先跑；只有在它能**完整**处理该函数时才动手，否则整函数
//! 回退给 `wide_mem`（槽载体）。这样每一步都可交付、可回退，避免一个函数里混用两种表示。

use super::*;

// 子模块通过 `use super::*` 共享这些名字（`pub(super) use` 才对外可见）。
pub(super) use forge_ir::ir::constant::canonical_int;
pub(super) use forge_ir::ir::opcode::Opcode;
pub(super) use forge_ir::util::big::Big;
pub(super) use forge_ir::*;

mod arith;
mod bitcount;
mod cmp;
mod emit;
mod mem;
mod mul;
mod shift;
#[cfg(test)]
mod tests;

use arith::{
    add_sub, bitwise, const_limbs, ext_from_narrow, not_limbs, reduce_narrow, select_wide,
};
use cmp::compare;
use emit::{LimbEmitter, mask_of};
use mem::{load_limbs, store_limbs};
use mul::mul_limbs;
use shift::{shift_const, shift_runtime};

/// 一个宽值的 n 个肢（下标 0 = 最低肢）。
type Limbs = smallvec::SmallVec<[Value; 4]>;

/// 软上限：超过就 fail-closed（64 肢 @W=64 = 4096 位；O(n²) 算法再宽已不现实）。
const MAX_WIDE_BITS: u32 = 4096;

/// 肢数。
fn limb_count(bits: u32, w: u32) -> usize {
    bits.div_ceil(w) as usize
}

fn is_wide_int(store: &forge_ir::ir::types::TypeStore, ty: TypeId, w: u32) -> bool {
    store.is_int(ty) && store.scalar_bits(ty).map(|b| b > w).unwrap_or(false)
}

/// 宽值多寄存器表示 pass。
pub(crate) struct WideSplitPass;

impl<M: TargetMachine> LegalizePass<M> for WideSplitPass {
    fn name(&self) -> &'static str {
        "wide-split"
    }

    fn run(&self, func: &mut Function, ctx: &mut LegalizeCtx<'_, M>) -> Result<ChangeSet, IrError> {
        let Some(&wmax) = ctx.machine.int_widths().iter().max() else {
            return Ok(ChangeSet::default());
        };
        let w = wmax as u32;
        let types_ctx = func.types.clone();

        // ① 收集宽值（指令结果 + 块参数）；超上限/不可行 ⇒ 整函数回退给 wide_mem
        let mut wide: Vec<Value> = Vec::new();
        // 宽值**块参数**（phi）：先记下来，稍后两阶段拆肢（见下面的 ③.5）。
        let mut wide_phis: Vec<(Block, usize, Value)> = Vec::new();
        {
            let store = types_ctx.borrow();
            let entry = func.entry();
            for (b, bd) in func.dfg.blocks() {
                for (idx, &pv) in bd.param_values.iter().enumerate() {
                    if !is_wide_int(&store, func.dfg.value_type(pv).unwrap_or(TypeId::VOID), w) {
                        continue;
                    }
                    // **入口块的宽值参数不拆**：那是函数 ABI 边界（签名按 `i128` 声明、
                    // 收参由约定层负责），拆了会让"IR 签名"与"块参数"两套说法打架。
                    // 非入口的 phi 才是纯 SSA 汇合点，拆肢是安全的。
                    if b == entry {
                        return Err(wide_shape_unsupported("入口块的宽值参数（函数 ABI 边界）"));
                    }
                    wide_phis.push((b, idx, pv));
                }
                for &ii in bd.inst_order.iter() {
                    for r in func.dfg.inst_data(ii).results.iter() {
                        let ty = func.dfg.value_type(*r).unwrap_or(TypeId::VOID);
                        if !is_wide_int(&store, ty, w) {
                            continue;
                        }
                        let bits = store.scalar_bits(ty).unwrap_or(0);
                        if bits > MAX_WIDE_BITS {
                            return Err(wide_shape_unsupported("宽值位宽超过软上限 4096"));
                        }
                        wide.push(*r);
                    }
                }
            }
        }
        if wide.is_empty() && wide_phis.is_empty() {
            return Ok(ChangeSet::default());
        }

        // ② 可行性：本 pass 不覆盖的形态**明确失败**（绝不静默把宽值留给 isel）
        if let Err(op) = is_fully_supported(func, &types_ctx, w) {
            return Err(wide_shape_unsupported(&format!(
                "宽值指令形态 {op:?} 未覆盖（操作数或结果是宽整数）"
            )));
        }

        // ③ 逐值建肢 + 改写使用点（迭代到不动点：SSA 定义可能跨块，无法保证按拓扑序遍历）
        let mut parts: forge_ir::entity::map::SecondaryMap<Value, Limbs> =
            forge_ir::entity::map::SecondaryMap::new();
        let mut e = LimbEmitter::new(func);
        let mut kills: Vec<Inst> = Vec::new();
        // ③.5(a) **宽值块参数（phi）拆肢——前半**（v27 M6-74）。
        //
        // 为什么必须先于不动点循环：使用点在循环里通过 `reg(parts, v)` 取肢，而它对未知值
        // 返回**空肢** ⇒ phi 的肢句柄必须在循环**之前**登记好。肢的**内容**（前驱实参的肢）
        // 则要等循环跑完才算得出来，所以回填在前驱侧留到 (b)。
        //
        // 逐块**从后往前**：替换会改变同块后续参数的下标，倒序才能保证 `idx` 仍有效。
        let mut phi_limbs: Vec<(Block, usize, Value)> = Vec::new();
        if !wide_phis.is_empty() {
            let limb_ty = e.limb_ty(w);
            let mut by_block: std::collections::BTreeMap<u32, (Block, Vec<(usize, Value)>)> =
                std::collections::BTreeMap::new();
            for &(b, idx, pv) in wide_phis.iter() {
                by_block
                    .entry(b.index())
                    .or_insert_with(|| (b, Vec::new()))
                    .1
                    .push((idx, pv));
            }
            for (_bk, (b, mut items)) in by_block {
                items.sort_by_key(|(idx, _)| std::cmp::Reverse(*idx));
                for (idx, pv) in items {
                    let bits = types_ctx
                        .borrow()
                        .scalar_bits(e.func.dfg.value_type(pv).unwrap_or(TypeId::VOID))
                        .unwrap_or(0);
                    let n = limb_count(bits, w);
                    let tys: Vec<TypeId> = std::iter::repeat_n(limb_ty, n).collect();
                    let limbs = e.func.split_block_param_shallow(b, idx, &tys);
                    if limbs.len() == n {
                        phi_limbs.push((b, idx, pv));
                        parts.insert(pv, limbs.iter().copied().collect());
                    }
                }
            }
        }

        // 已处理的指令：**必须**记住，否则不动点循环会反复处理同一条（死循环）。
        let mut done: std::collections::HashSet<Inst> = std::collections::HashSet::new();
        loop {
            let mut progress = false;
            let blocks: Vec<Block> = e.func.dfg.blocks().map(|(b, _)| b).collect();
            for b in blocks {
                let insts: Vec<Inst> = e.func.dfg.block(b).inst_order.clone();
                for ii in insts {
                    if done.contains(&ii) {
                        continue;
                    }
                    if rewrite_one(&mut e, &mut parts, &mut kills, ii, w, &types_ctx)? {
                        done.insert(ii);
                        progress = true;
                    }
                }
            }
            if !progress {
                break;
            }
        }

        // ③.5(b) **宽值块参数（phi）拆肢——后半**：把每条入边上的宽实参换成它的肢。
        for (b, idx, _pv) in phi_limbs.iter() {
            let preds: Vec<Block> = e.func.predecessors().get(*b).cloned().unwrap_or_default();
            for pred in preds {
                let Some(arg) = terminator_arg(e.func, pred, *b, *idx) else {
                    continue;
                };
                let repl: Vec<Value> = parts
                    .get(arg)
                    .map(|l: &Limbs| l.iter().copied().collect())
                    .unwrap_or_default();
                if !repl.is_empty() {
                    e.func.splice_terminator_args(pred, *b, *idx, &repl);
                }
            }
        }

        // ④ 清理原宽指令（此时已无使用者），再跑泄漏守卫
        for ii in kills {
            e.kill(ii);
        }
        guard_no_wide_left(e.func, &types_ctx, w)?;

        Ok(ChangeSet {
            new_insts: e.created,
            new_values: e.created,
            new_consts: e.consts,
            sret: false,
        })
    }
}

/// 读 `pred` 终结符传给 `target` 的第 `idx` 个实参（四种分支形态）。
fn terminator_arg(func: &Function, pred: Block, target: Block, idx: usize) -> Option<Value> {
    use forge_ir::ir::terminator::TermKind;
    match func.dfg.term_kind(pred)? {
        TermKind::Jump => {
            let (t, args) = func.dfg.term_jump(pred)?;
            (t == target).then(|| args.get(idx).copied()).flatten()
        }
        TermKind::Branch => {
            let (_, t, targs, e, eargs) = func.dfg.term_branch(pred)?;
            if t == target {
                targs.get(idx).copied()
            } else if e == target {
                eargs.get(idx).copied()
            } else {
                None
            }
        }
        TermKind::Switch => {
            let view = func.dfg.term_switch(pred)?;
            if view.default_block == target
                && let Some(v) = view.default_args.get(idx)
            {
                return Some(*v);
            }
            for c in view.cases.iter() {
                if c.target == target
                    && let Some(v) = c.args.get(idx)
                {
                    return Some(*v);
                }
            }
            None
        }
        TermKind::Invoke => {
            let (_, _, _, n, nargs, u, uargs) = func.dfg.term_invoke(pred)?;
            if n == target {
                nargs.get(idx).copied()
            } else if u == target {
                uargs.get(idx).copied()
            } else {
                None
            }
        }
        _ => None,
    }
}

/// 本 pass 的处理能力判断：**任何**一处不支持就让整函数回退给 `wide_mem`。
fn is_fully_supported(
    func: &Function,
    types_ctx: &forge_ir::ir::types::TypeContext,
    w: u32,
) -> Result<(), Opcode> {
    let store = types_ctx.borrow();
    for (_, bd) in func.dfg.blocks() {
        for &ii in bd.inst_order.iter() {
            let d = func.dfg.inst_data(ii);
            let touches =
                d.results.iter().chain(d.operands.iter()).any(|v| {
                    is_wide_int(&store, func.dfg.value_type(*v).unwrap_or(TypeId::VOID), w)
                });
            if !touches {
                continue;
            }
            let ok = match d.opcode {
                Opcode::Iconst
                | Opcode::Copy
                | Opcode::Iadd
                | Opcode::Isub
                | Opcode::Band
                | Opcode::Bor
                | Opcode::Bxor
                | Opcode::Icmp
                | Opcode::Ireduce
                | Opcode::Sextend
                | Opcode::Uextend
                | Opcode::Clz
                | Opcode::Ctz
                | Opcode::Load
                | Opcode::Store
                | Opcode::Select
                | Opcode::Bnot
                | Opcode::Imul => true,
                // 移位：常量与运行期移位量都支持
                Opcode::Ishl | Opcode::Ushr | Opcode::Sshr => true,
                _ => false, // Div/Rem/Call/Ret/… ⇒ 回退
            };
            if !ok {
                // 返回**具体 opcode**：失败消息里点名它，比"存在未覆盖的宽值指令形态"
                // 好查一百倍（v27 M6-56 实测：正是这条诊断让我们一眼看到是哪种形态）。
                return Err(d.opcode);
            }
        }
    }
    Ok(())
}

/// 某个值是否由一条 `Iconst` 定义，是则返回其常量值。
fn constant_of(func: &Function, v: Value) -> Option<u128> {
    let ValueDef::Inst(ci, _) = func.dfg.value_data(v).def else {
        return None;
    };
    let d = func.dfg.inst_data_opt(ci)?;
    if d.opcode != Opcode::Iconst {
        return None;
    }
    match d.immediates.first()? {
        Immediate::Const(cid) => func.constants.bits(*cid).map(|bv| bv.bits),
        Immediate::Int(val) => Some(*val as u128),
        _ => None,
    }
}

/// 处理一条指令；返回是否"动了手"。
fn rewrite_one(
    e: &mut LimbEmitter<'_>,
    parts: &mut forge_ir::entity::map::SecondaryMap<Value, Limbs>,
    kills: &mut Vec<Inst>,
    ii: Inst,
    w: u32,
    types_ctx: &forge_ir::ir::types::TypeContext,
) -> Result<bool, IrError> {
    let d = e.func.dfg.inst_data(ii).clone();
    let store = types_ctx.borrow();
    let ty_of = |v: Value| e.func.dfg.value_type(v).unwrap_or(TypeId::VOID);
    let wide_results: Vec<Value> = d
        .results
        .iter()
        .copied()
        .filter(|v| is_wide_int(&store, ty_of(*v), w))
        .collect();
    let wide_operands: Vec<Value> = d
        .operands
        .iter()
        .copied()
        .filter(|v| is_wide_int(&store, ty_of(*v), w))
        .collect();
    if wide_results.is_empty() && wide_operands.is_empty() {
        return Ok(false);
    }
    // ready：所有宽操作数的肢都已就绪
    let ready = wide_operands.iter().all(|v| parts.get(*v).is_some());
    if !ready {
        return Ok(false);
    }
    fn reg(parts: &forge_ir::entity::map::SecondaryMap<Value, Limbs>, v: Value) -> Limbs {
        parts.get(v).cloned().unwrap_or_default()
    }

    match d.opcode {
        Opcode::Iconst => {
            let Some(r) = wide_results.first().copied() else {
                return Ok(false);
            };
            let bits = store.scalar_bits(ty_of(r)).unwrap_or(0);
            let n = limb_count(bits, w);
            let cid = d.immediates.iter().find_map(|im| match im {
                Immediate::Const(c) => Some(*c),
                _ => None,
            });
            let Some(cid) = cid else {
                return Err(wide_shape_unsupported("宽值常量缺少常量池条目"));
            };
            let ls = const_limbs(e, ii, cid, w, n);
            parts.insert(r, ls);
            kills.push(ii);
            Ok(true)
        }
        Opcode::Copy => {
            let (Some(r), Some(src)) = (
                wide_results.first().copied(),
                wide_operands.first().copied(),
            ) else {
                return Ok(false);
            };
            parts.insert(r, reg(parts, src));
            kills.push(ii);
            Ok(true)
        }
        Opcode::Iadd | Opcode::Isub => {
            let Some(r) = wide_results.first().copied() else {
                return Ok(false);
            };
            let (a, b) = (reg(parts, wide_operands[0]), reg(parts, wide_operands[1]));
            let bits = store.scalar_bits(ty_of(r)).unwrap_or(0);
            let ls = add_sub(e, ii, &a, &b, w, bits, d.opcode == Opcode::Iadd);
            parts.insert(r, ls);
            kills.push(ii);
            Ok(true)
        }
        Opcode::Band | Opcode::Bor | Opcode::Bxor => {
            let Some(r) = wide_results.first().copied() else {
                return Ok(false);
            };
            let (a, b) = (reg(parts, wide_operands[0]), reg(parts, wide_operands[1]));
            let bits = store.scalar_bits(ty_of(r)).unwrap_or(0);
            let ls = bitwise(e, ii, &a, &b, w, bits, d.opcode);
            parts.insert(r, ls);
            kills.push(ii);
            Ok(true)
        }
        Opcode::Select => {
            let Some(r) = wide_results.first().copied() else {
                return Ok(false);
            };
            // operands = [cond, then, else]
            let (a, b) = (reg(parts, d.operands[1]), reg(parts, d.operands[2]));
            let bits = store.scalar_bits(ty_of(r)).unwrap_or(0);
            let ls = select_wide(e, ii, d.operands[0], &a, &b, w, bits);
            parts.insert(r, ls);
            kills.push(ii);
            Ok(true)
        }
        Opcode::Icmp => {
            let Some(r) = d.results.first().copied() else {
                return Ok(false);
            };
            let Some(cc) = d.immediates.iter().find_map(|im| match im {
                Immediate::IntCC(c) => Some(*c),
                _ => None,
            }) else {
                return Err(wide_shape_unsupported("宽值比较缺少条件码"));
            };
            let (a, b) = (reg(parts, wide_operands[0]), reg(parts, wide_operands[1]));
            let bits = store.scalar_bits(ty_of(wide_operands[0])).unwrap_or(0);
            let out = compare(e, ii, &a, &b, cc, w, bits);
            e.rewrite_in_place(ii, Opcode::Copy, &[out]);
            let _ = r;
            Ok(true)
        }
        Opcode::Ireduce => {
            let Some(src) = wide_operands.first().copied() else {
                return Ok(false);
            };
            let Some(r) = d.results.first().copied() else {
                return Ok(false);
            };
            let a = reg(parts, src);
            let to_ty = ty_of(r);
            let out = reduce_narrow(e, ii, &a, w, to_ty);
            e.rewrite_in_place(ii, Opcode::Copy, &[out]);
            Ok(true)
        }
        Opcode::Sextend | Opcode::Uextend => {
            let Some(r) = wide_results.first().copied() else {
                return Ok(false);
            };
            let Some(src) = d.operands.first().copied() else {
                return Ok(false);
            };
            let bits = store.scalar_bits(ty_of(r)).unwrap_or(0);
            let n = limb_count(bits, w);
            let ls = ext_from_narrow(e, ii, src, w, n, d.opcode == Opcode::Sextend, bits);
            parts.insert(r, ls);
            kills.push(ii);
            Ok(true)
        }
        Opcode::Load => {
            let Some(r) = wide_results.first().copied() else {
                return Ok(false);
            };
            let bits = store.scalar_bits(ty_of(r)).unwrap_or(0);
            let ls = load_limbs(e, ii, d.operands[0], w, bits);
            parts.insert(r, ls);
            kills.push(ii);
            Ok(true)
        }
        Opcode::Store => {
            let Some(src) = wide_operands.first().copied() else {
                return Ok(false);
            };
            let bits = store.scalar_bits(ty_of(src)).unwrap_or(0);
            let ls = reg(parts, src);
            let ptr = d.operands[1];
            store_limbs(e, ii, &ls, ptr, w, bits);
            // **不要 kill**：原 store 已被就地改写成"存最后一个肢"，把它 Nop 掉会丢掉那次写。
            Ok(true)
        }
        Opcode::Clz | Opcode::Ctz => {
            // 位计数：结果宽度由指令结果类型定——窄（内建返回 `u32`）⇒ 截断后改 `Copy`；
            // 宽（`Clz i128 → i128`）⇒ 拼成 n 肢当宽值。
            let Some(src) = wide_operands.first().copied() else {
                return Ok(false);
            };
            let Some(r) = d.results.first().copied() else {
                return Ok(false);
            };
            let a = reg(parts, src);
            let bits = store.scalar_bits(ty_of(src)).unwrap_or(0);
            let rt = ty_of(r);
            let rt_bits = store.scalar_bits(rt).unwrap_or(0);
            let leading = d.opcode == Opcode::Clz;
            if rt_bits > w {
                let ls = bitcount::count_limbs(e, ii, &a, w, bits, leading);
                parts.insert(r, ls);
                kills.push(ii);
            } else {
                let out = bitcount::count_scalar(e, ii, &a, w, bits, leading, rt);
                e.rewrite_in_place(ii, Opcode::Copy, &[out]);
            }
            Ok(true)
        }
        Opcode::Bnot => {
            let Some(r) = wide_results.first().copied() else {
                return Ok(false);
            };
            let a = reg(parts, wide_operands[0]);
            let bits = store.scalar_bits(ty_of(r)).unwrap_or(0);
            let ls = not_limbs(e, ii, &a, w, bits);
            parts.insert(r, ls);
            kills.push(ii);
            Ok(true)
        }
        Opcode::Imul => {
            let Some(r) = wide_results.first().copied() else {
                return Ok(false);
            };
            let (a, b) = (reg(parts, wide_operands[0]), reg(parts, wide_operands[1]));
            let bits = store.scalar_bits(ty_of(r)).unwrap_or(0);
            let ls = mul_limbs(e, ii, &a, &b, w, bits);
            parts.insert(r, ls);
            kills.push(ii);
            Ok(true)
        }
        Opcode::Ishl | Opcode::Ushr | Opcode::Sshr => {
            let Some(r) = wide_results.first().copied() else {
                return Ok(false);
            };
            let a = reg(parts, wide_operands[0]);
            let bits = store.scalar_bits(ty_of(r)).unwrap_or(0);
            let ls = match constant_of(e.func, d.operands[1]) {
                Some(amt) => shift_const(e, ii, &a, amt as u64, d.opcode, w, bits),
                None => {
                    // 移位量可能是**窄**类型（如 `u128 << u32`）：它是单肢，没有高位肢。
                    let amt_v = d.operands[1];
                    let amt_limbs = match parts.get(amt_v) {
                        Some(ls) => ls.clone(),
                        None => smallvec::smallvec![amt_v],
                    };
                    shift_runtime(e, ii, &a, &amt_limbs, d.opcode, w, bits)
                }
            };
            parts.insert(r, ls);
            kills.push(ii);
            Ok(true)
        }
        _ => Ok(false),
    }
}

/// 泄漏守卫：任何**活着的**整数 `bits > W` ⇒ 明确失败（不静默回退成宽 GPR）。
///
/// **只扫活值**（指令结果/操作数 + 块参数）：`kill` 清空 `results` 后，`dfg.values()` 里仍留着
/// 已死值的条目（类型还是宽类型），全量扫描会误报。
fn guard_no_wide_left(
    func: &Function,
    types_ctx: &forge_ir::ir::types::TypeContext,
    w: u32,
) -> Result<(), IrError> {
    let store = types_ctx.borrow();
    // 用法表：**没有使用者的宽值不算泄漏**——ABI 边界改写会把原来的形参值摘掉
    // （换成指针 + load），旧值仍留在块参数表里但已没人用。判"还在参与"才是有意义的判据。
    let check = |v: Value| -> Result<(), IrError> {
        // 没有使用者的宽值不算泄漏（返回值也走 use_lists ⇒ 真被用着就一定有使用者）。
        if !func.use_lists.has_uses(v) {
            return Ok(());
        }
        let Some(t) = func.dfg.value_type(v) else {
            return Ok(());
        };
        if matches!(t, TypeId::VOID) {
            return Ok(());
        }
        if is_wide_int(&store, t, w) {
            // **诊断价值**：说清"剩下的是谁"。只报"泄漏"会让下一个人从头猜（本轮实测踩到）。
            return Err(wide_shape_unsupported(&format!(
                "宽值未完全 split（泄漏守卫）：值 {v:?} 类型 {t:?}（{} 位）",
                store.size_bytes(t) * 8
            )));
        }
        Ok(())
    };
    for (_, bd) in func.dfg.blocks() {
        for &pv in bd.param_values.iter() {
            check(pv)?;
        }
        for &ii in bd.inst_order.iter() {
            let d = func.dfg.inst_data(ii);
            for &r in d.results.iter() {
                check(r)?;
            }
            for &o in d.operands.iter() {
                check(o)?;
            }
        }
    }
    Ok(())
}
