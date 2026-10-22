//! **宽整数除/余：分肢长除法**（v27 M6-79，`WA-49` 的正题）。
//!
//! 为什么需要：`wide_split` 把宽值拆成 n 个原生宽肢之后，`Udiv/Urem/Sdiv/Srem` 就没有规则可用
//! （isel 只认原生位宽），而**不能在降级层**发 `__udivti3` 之类的运行时调用——那会写死
//! ① 运行时符号（不同目标名字不同）② 向量寄存器（`xmm0`/`V128` 只在某些 ISA 上存在）
//! ③ 原生位宽（肢宽来自 `machine.int_widths()`，8 位 ISA 上是 8）。见 `CLAUDE.md`「分层红线」。
//!
//! 做法：把除法改写成**逐位**的恢复余数长除（restoring division），只用
//! `Ishl/Ushr/Band/Bor/Icmp/Select/Isub/Bxor/Sshr` 这些 ISA 无关的 IR：
//!
//! ```text
//! r = 0; q = 0
//! for i in (0..N).rev() {
//!     bit = (a >> i) & 1
//!     r2  = (r << 1) | bit
//!     ge  = r2 >= b
//!     r   = select(ge, r2 - b, r2)
//!     q  |= select(ge, 1 << i, 0)
//! }
//! ```
//!
//! **展开而非循环**：本 pass 只做块内指令替换、不碰 CFG（循环版要拆分原块）。展开版对
//! **任意原生位宽**成立（N 与肢宽都来自机器事实）。
//!
//! 有符号：先取绝对值（`mask = x >> (N-1)`、`|x| = (x ^ mask) - mask`），做无符号除法，
//! 再按符号补回（`%` 跟被除数、`/` 跟两符号异或），与"向零取整"一致。

use forge_ir::*;

use super::{ChangeSet, LegalizeCtx, LegalizePass};
use crate::IrError;
use crate::machine::target::TargetMachine;

pub(crate) struct WideDivPass;

/// 一条待改写的宽值除/余。
#[derive(Clone, Copy)]
struct Job {
    block: Block,
    inst: Inst,
    bits: u32,
    opcode: Opcode,
    a: Value,
    b: Value,
}

/// 就地发指令的小工具：新指令先落在**块尾**，最后由调用方整体搬到原位置。
struct Emit<'a> {
    func: &'a mut Function,
    block: Block,
    made: Vec<Inst>,
}

impl Emit<'_> {
    fn inst(&mut self, op: Opcode, ops: &[Value], imms: Vec<Immediate>, ty: TypeId) -> Value {
        let id = self.func.make_inst(
            op,
            self.block,
            ops.iter().copied().collect(),
            imms.into(),
            &[ty],
            InstFlags::NONE,
        );
        self.made.push(id);
        self.func.dfg.inst_data(id).results[0]
    }

    fn konst(&mut self, v: i128, ty: TypeId) -> Value {
        let width = ty.builtin_scalar_bits().unwrap_or(64);
        let cid = self.func.constants.intern_int(v, width);
        self.inst(Opcode::Iconst, &[], vec![Immediate::Const(cid)], ty)
    }
}

impl<M: TargetMachine> LegalizePass<M> for WideDivPass {
    fn name(&self) -> &'static str {
        "wide-div"
    }

    fn run(&self, func: &mut Function, ctx: &mut LegalizeCtx<'_, M>) -> Result<ChangeSet, IrError> {
        let wmax = ctx.machine.int_widths().iter().copied().max().unwrap_or(64) as u32;
        let mut jobs: Vec<Job> = Vec::new();
        for (b, bd) in func.dfg.blocks() {
            for &ii in bd.inst_order.iter() {
                let d = func.dfg.inst_data(ii);
                if !matches!(
                    d.opcode,
                    Opcode::Udiv | Opcode::Urem | Opcode::Sdiv | Opcode::Srem
                ) {
                    continue;
                }
                let (Some(&a), Some(&bb)) = (d.operands.first(), d.operands.get(1)) else {
                    continue;
                };
                let Some(ty) = func.dfg.value_type(a) else {
                    continue;
                };
                let Some(bits) = ty.builtin_scalar_bits() else {
                    continue;
                };
                if bits <= wmax {
                    continue; // 原生位宽有 isel 规则
                }
                jobs.push(Job {
                    block: b,
                    inst: ii,
                    bits,
                    opcode: d.opcode,
                    a,
                    b: bb,
                });
            }
        }
        if jobs.is_empty() {
            return Ok(ChangeSet::default());
        }

        let mut total = 0usize;
        for job in jobs.into_iter().rev() {
            let ty = func.dfg.value_type(job.a).unwrap_or(TypeId::I128);
            let mut e = Emit {
                func,
                block: job.block,
                made: Vec::new(),
            };
            let signed = matches!(job.opcode, Opcode::Sdiv | Opcode::Srem);
            let rem = matches!(job.opcode, Opcode::Urem | Opcode::Srem);
            let (ua, ub, sa, sb) = if signed {
                let sh = e.konst(i128::from(job.bits - 1), ty);
                let ma = e.inst(Opcode::Sshr, &[job.a, sh], vec![], ty);
                let mb = e.inst(Opcode::Sshr, &[job.b, sh], vec![], ty);
                let xa = e.inst(Opcode::Bxor, &[job.a, ma], vec![], ty);
                let ua = e.inst(Opcode::Isub, &[xa, ma], vec![], ty);
                let xb = e.inst(Opcode::Bxor, &[job.b, mb], vec![], ty);
                let ub = e.inst(Opcode::Isub, &[xb, mb], vec![], ty);
                (ua, ub, Some(ma), Some(mb))
            } else {
                (job.a, job.b, None, None)
            };
            let (q, r) = long_divide(&mut e, ty, ua, ub, job.bits);
            let v = if !signed {
                if rem { r } else { q }
            } else {
                let m = if rem {
                    sa.expect("有符号必有符号掩码")
                } else {
                    let x = sa.expect("有符号必有符号掩码");
                    let y = sb.expect("有符号必有符号掩码");
                    e.inst(Opcode::Bxor, &[x, y], vec![], ty)
                };
                let base = if rem { r } else { q };
                let x = e.inst(Opcode::Bxor, &[base, m], vec![], ty);
                e.inst(Opcode::Isub, &[x, m], vec![], ty)
            };

            let made = std::mem::take(&mut e.made);
            let n = made.len();
            total += n;
            // 新指令都落在块尾 ⇒ 取出来按原顺序插回原位置。
            let mut popped: Vec<Inst> = Vec::with_capacity(n);
            for _ in 0..n {
                let order = &mut func.dfg.block_mut(job.block).inst_order;
                popped.push(order.pop().expect("刚插入的指令必在块尾"));
            }
            popped.reverse();
            let pos = func
                .dfg
                .block(job.block)
                .inst_order
                .iter()
                .position(|x| *x == job.inst)
                .expect("正要改写的指令必须在本块的 inst_order 里");
            func.replace_all_uses_and_kill(job.inst, &[v]);
            for (k, &ni) in popped.iter().enumerate() {
                func.dfg.block_mut(job.block).inst_order.insert(pos + k, ni);
            }
        }

        Ok(ChangeSet {
            new_insts: total,
            new_values: total,
            new_consts: total,
            sret: false,
        })
    }
}

/// 无符号恢复余数长除：返回 `(商, 余数)`，两者都是 `ty` 类型的宽值。
fn long_divide(e: &mut Emit<'_>, ty: TypeId, a: Value, b: Value, bits: u32) -> (Value, Value) {
    let zero = e.konst(0, ty);
    let one = e.konst(1, ty);
    let mut q = zero;
    let mut r = zero;
    for i in (0..bits).rev() {
        let iv = e.konst(i128::from(i), ty);
        let bit = {
            let s = e.inst(Opcode::Ushr, &[a, iv], vec![], ty);
            e.inst(Opcode::Band, &[s, one], vec![], ty)
        };
        let r2 = {
            let s = e.inst(Opcode::Ishl, &[r, one], vec![], ty);
            e.inst(Opcode::Bor, &[s, bit], vec![], ty)
        };
        let ge = e.inst(
            Opcode::Icmp,
            &[r2, b],
            vec![Immediate::IntCC(IntCC::Uge)],
            TypeId::BOOL,
        );
        let sub = e.inst(Opcode::Isub, &[r2, b], vec![], ty);
        r = e.inst(Opcode::Select, &[ge, sub, r2], vec![], ty);
        let sh = e.inst(Opcode::Ishl, &[one, iv], vec![], ty);
        let qb = e.inst(Opcode::Select, &[ge, sh, zero], vec![], ty);
        q = e.inst(Opcode::Bor, &[q, qb], vec![], ty);
    }
    (q, r)
}
