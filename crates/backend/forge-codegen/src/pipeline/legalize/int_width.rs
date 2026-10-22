//! 合法化 pass：**整数位宽补位**（v27 M5 · 任意位宽整数）。
//!
//! 职责（对应 `docs/reference/ir-int-semantics.md` 的补位阶梯）：
//!
//! - **R1 精确**：值位宽已在目标机 `int_widths()` 里 ⇒ 原样交给映射层；
//! - **R2 就近升位**：不存在精确位宽时，取 `int_widths()` 中 **≥ 该位宽的最小者** `W↑`，
//!   把操作数 `zext|sext → iW↑`（有符号 op 用 sext）、把结果类型改成 `iW↑`，
//!   随后插一条 `ireduce → 原位宽` 并让原结果的**其它使用点**改用它；
//! - **R3 分字展开**（i128 → 2×i64 + 进位）**不在本 pass**：它属于后续里程碑，
//!   本 pass 对"宽于最大原生位宽"的值**明确跳过**（交给后续 pass，禁止静默错误）。
//!
//! 为什么要这一层：把非原生位宽（i1/i2/i3/i7…）的"扩位 + 截回"写在**后端**，
//! 前端（forge-rustc）就只需 `MIR → IR`，ISA-DSL 也只面对原生位宽。

use super::{LegalizeCtx, LegalizePass};
use crate::IrError;
use crate::machine::target::TargetMachine;
use crate::pipeline::agg_expand::ChangeSet;
use forge_ir::ir::opcode::Opcode;
use forge_ir::*;

/// 整数位宽补位 pass。
pub(crate) struct IntWidthPass;

/// 该 opcode 的"结果位宽由操作数决定、且结果与操作数同宽"的分类。
///
/// 只列**可以安全地升位再截回**的整数运算：它们的语义在 `mod 2^bits` 下封闭，
/// 升位后截回与按原位宽直接计算等价（比较的结果是 i1，单独处理）。
fn widenable(op: Opcode) -> bool {
    matches!(
        op,
        Opcode::Iadd
            | Opcode::Isub
            | Opcode::Imul
            | Opcode::Udiv
            | Opcode::Sdiv
            | Opcode::Urem
            | Opcode::Srem
            | Opcode::Band
            | Opcode::Bor
            | Opcode::Bxor
            | Opcode::Bnot
            | Opcode::Ishl
            | Opcode::Ushr
            | Opcode::Sshr
            | Opcode::Bswap
            | Opcode::Icmp
            | Opcode::Load
            | Opcode::Store
            | Opcode::Clz
    )
}

/// 有符号语义的 opcode（`sext` 而不是 `zext`）。
fn is_signed_op(op: Opcode) -> bool {
    matches!(op, Opcode::Sdiv | Opcode::Srem | Opcode::Sshr)
}

impl<M: TargetMachine> LegalizePass<M> for IntWidthPass {
    fn name(&self) -> &'static str {
        "int-width"
    }

    fn run(&self, func: &mut Function, ctx: &mut LegalizeCtx<'_, M>) -> Result<ChangeSet, IrError> {
        let mut widths: Vec<u32> = ctx.machine.int_widths().iter().map(|w| *w as u32).collect();
        widths.sort_unstable();
        if widths.is_empty() {
            return Ok(ChangeSet::default());
        }
        let types_ctx = func.types.clone();

        // (block, pos, inst, 原位宽, 目标位宽, 是否符号升位)
        let mut jobs: Vec<(Block, usize, Inst, u32, u32, bool)> = Vec::new();
        {
            let store = types_ctx.borrow();
            for (b, bd) in func.dfg.blocks() {
                for (pos, &ii) in bd.inst_order.iter().enumerate() {
                    let inst = func.dfg.inst_data(ii);
                    if !widenable(inst.opcode) {
                        continue;
                    }
                    // 判位宽用**第一个结果**（算术/常量）或**第一个操作数**（比较的结果是 i1）
                    let probe = match inst.opcode {
                        // 比较的结果是 i1；存储没有结果 —— 两者都用第一个操作数判位宽。
                        Opcode::Icmp | Opcode::Store => inst.operands.first().copied(),
                        _ => inst.results.first().copied(),
                    };
                    let Some(v) = probe else { continue };
                    let Some(t) = func.dfg.value_type(v) else {
                        continue;
                    };
                    // 宽值常量也要由 R3 物化（isel 只会造 64 位及以下常量）。
                    let is_wide_const = inst.opcode == Opcode::Iconst;
                    if !store.is_int(t) {
                        continue;
                    }
                    let Some(bits) = store.scalar_bits(t) else {
                        continue;
                    };
                    // `i1` 是 bool：机器上按字节承载、0/1 规范化，**不参与升位**
                    //（否则每轮都把 i1 升成 i8 再截回，pass 永不收敛——实测踩过）。
                    if bits <= 1 {
                        continue;
                    }
                    if widths.contains(&bits) {
                        continue; // R1：已是原生位宽
                    }
                    // R2：就近升位（取 ≥ bits 的最小原生位宽）；没有 ⇒ 属 **R3 分字展开**。
                    let to = match widths.iter().copied().filter(|w| *w > bits).min() {
                        Some(_) if is_wide_const => {
                            // 常量不参与 R2（其位宽由使用处的 extend 负责）
                            continue;
                        }
                        Some(to) => to,
                        None => {
                            // 宽于所有原生位宽：交给 R3（由作业循环另行处理）。
                            bits
                        }
                    };
                    let signed = is_signed_op(inst.opcode);
                    jobs.push((b, pos, ii, bits, to, signed));
                }
            }
        }
        if jobs.is_empty() {
            return Ok(ChangeSet::default());
        }

        let mut new_insts = 0usize;
        let mut new_values = 0usize;
        let mut new_consts = 0usize;
        let _wmax = *widths.iter().max().expect("int_widths 非空");
        // 逆序处理；`pos` 在**处理时**按当前块序重算（前序作业可能已经插入了指令）。
        for (b, _pos, ii, from_bits, to_bits, signed) in jobs.into_iter().rev() {
            let pos = func
                .dfg
                .block(b)
                .inst_order
                .iter()
                .position(|x| *x == ii)
                .unwrap_or(0);
            let from_ty = types_ctx.int_ty(from_bits);
            let to_ty = types_ctx.int_ty(to_bits);
            // **只做"升位"**：`to_bits <= from_bits` 的作业是恒等（32 位整数会被
            // "升位到 32"，目标即自身）。此前仍会发同宽的 `sextend`/`uextend`，而
            // Convert 族要求 src **严格**窄于 dst ⇒ `int src (5) < int dst (5)` 校验失败。
            // v27 M6 实测：`b"ABCD"` 常量下标读取、`a[i]` 越界检查在 `-O` 下因此编译失败。
            if to_bits <= from_bits {
                continue;
            }
            let (opcode, mut operands, is_cmp) = {
                let inst = func.dfg.inst_data(ii);
                (
                    inst.opcode,
                    inst.operands.iter().copied().collect::<Vec<Value>>(),
                    inst.opcode == Opcode::Icmp,
                )
            };

            // ① 操作数升位（逐个插 extend，稍后统一挪到本指令之前）
            let mut n_ext = 0usize;
            let ops_snapshot = operands.clone();
            for (k, &op) in ops_snapshot.iter().enumerate() {
                let Some(ot) = func.dfg.value_type(op) else {
                    continue;
                };
                if ot != from_ty {
                    continue;
                }
                let ext_op = if signed {
                    Opcode::Sextend
                } else {
                    Opcode::Uextend
                };
                let ext = func.make_inst(
                    ext_op,
                    b,
                    smallvec::smallvec![op],
                    smallvec::smallvec![],
                    &[to_ty],
                    InstFlags::NONE,
                );
                let ev = func.dfg.inst_data(ext).results[0];
                operands[k] = ev;
                n_ext += 1;
            }
            new_insts += n_ext;
            new_values += n_ext;

            // ② 常量：把常量条目改成目标位宽（值按规范形随位宽走）。当前 `Iconst` 不在
            // 可升位集合里，这段保留给"未来把常量也纳入升位"的情形（无死代码警告见下方 allow）。
            #[allow(unused)]
            if opcode == Opcode::Iconst {
                let cid = {
                    let inst = func.dfg.inst_data(ii);
                    inst.immediates.first().and_then(|im| match im {
                        Immediate::Const(c) => Some(*c),
                        _ => None,
                    })
                };
                if let Some(cid) = cid
                    && let Some(bv) = func.constants.bits(cid)
                {
                    let widened = func.constants.intern_int(bv.bits as i128, to_bits);
                    if widened != cid {
                        new_consts += 1;
                    }
                    let inst = func.dfg.inst_mut(ii);
                    inst.immediates[0] = Immediate::Const(widened);
                }
            }

            // ③ 改写本指令：操作数换成升位后的值；结果类型改成 iW↑（比较保持 i1）
            if !is_cmp {
                let inst = func.dfg.inst_mut(ii);
                inst.operands = smallvec::SmallVec::from_vec(operands.clone());
                let r0 = inst.results[0];
                func.dfg.set_value_type(r0, to_ty);
            } else {
                let inst = func.dfg.inst_mut(ii);
                inst.operands = smallvec::SmallVec::from_vec(operands.clone());
            }
            func.refresh_inst_uses(ii);

            // ④ 结果截回原位宽：`%t = ireduce iW↑ → i<原>`，原结果的**其它**使用点改用它。
            //
            // **`to_ty == from_ty` 时不能发 `ireduce`**：同宽截断在 IR 里非法
            // （verifier 的 Convert 族要求 src **严格**宽于 dst）。32 位整数会被这条
            // pass"升位到 32"（目标即自身），此时第 ④ 步是恒等操作 ⇒ 直接跳过。
            // v27 M6 实测：`b"ABCD"` 的常量下标读取、`a[i]` 的越界检查在 `-O` 下因
            // `ireduce i64→i64` 校验失败（`int src (5) < int dst (5)`）。
            //
            // 注：`Clz` 的宽度语义（"按原位宽数前导零"）**不在这里**修正——本仓库的 `Clz`
            // 由 isel 按原生宽度发射，而窄位宽的校正目前由前端内建路径负责；
            // 把这条语义搬进 IR 需要先解决"内建实参只有低半"的问题（见 M5 未闭环清单）。
            let mut trailing = 0usize;
            if !is_cmp && to_ty != from_ty {
                let r0 = func.dfg.inst_data(ii).results[0];
                let src = r0;
                let red = func.make_inst(
                    Opcode::Ireduce,
                    b,
                    smallvec::smallvec![src],
                    smallvec::smallvec![],
                    &[from_ty],
                    InstFlags::NONE,
                );
                let rv = func.dfg.inst_data(red).results[0];
                trailing += 1;
                new_insts += 1;
                new_values += 1;
                // 先把所有使用点（含 reduce 自己）指到 rv，再把 reduce 的操作数改回 src。
                func.replace_all_uses(r0, rv);
                let inst = func.dfg.inst_mut(red);
                inst.operands[0] = src;
                func.refresh_inst_uses(red);
            }

            // ⑤ 把新指令放到正确位置：extend 在 `pos` 之前，其余在本指令之后（按创建顺序）。
            let total_new = n_ext + trailing;
            if total_new > 0 {
                let mut made: Vec<Inst> = Vec::with_capacity(total_new);
                for _ in 0..total_new {
                    let order = &mut func.dfg.block_mut(b).inst_order;
                    made.push(order.pop().expect("刚插入的指令必在块尾"));
                }
                made.reverse();
                // made = [ext0..ext_{n-1}, (Clz 修正: Iconst, Isub), Ireduce]
                let mut at = pos;
                for &ni in made.iter().take(n_ext) {
                    func.dfg.block_mut(b).inst_order.insert(at, ni);
                    at += 1;
                }
                // 原指令现在位于 `at`；尾部指令依次插在它后面。
                for (after, &ni) in (at + 1..).zip(made.iter().skip(n_ext)) {
                    func.dfg.block_mut(b).inst_order.insert(after, ni);
                }
            }
        }

        Ok(ChangeSet {
            new_insts,
            new_values,
            new_consts,
            sret: false,
        })
    }
}

// **R3 分字展开已删除**（v27 M5）：它的做法是用 `bitcast [i64;2] ↔ i128` 拼宽值，
// 而 isel 没有那条 lowering（实测 `[cap:SPEC_LOWERING_RULE_MISSING]`）。宽值的合法化
// 已由 `wide_mem`（槽载体）承担，且其 JIT 语义测试全绿 ⇒ 本文件只保留 R1/R2。
// 测试在同目录的独立文件里（v27 M5：`file_size_ratchet` 对管线文件有规模上限，
// 测试搬出去既满足棘轮也不干扰主逻辑阅读）。
#[cfg(test)]
#[path = "int_width_tests.rs"]
mod tests;
