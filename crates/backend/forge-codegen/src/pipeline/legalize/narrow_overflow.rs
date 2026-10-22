//! **窄整数（8/16 位）的 `*_overflow` 合法化**（v27 M6-53）。
//!
//! 为什么需要：x86 的 8 位 reg-reg 算术是**另一个 opcode**（`00 /r`），而谱里的
//! `ADD_RM_R`/`SUB_RM_R` 只有 16/32/64 形态（`gprx` 显式排除 8 位）。8 位操作数因此
//! 落到 16/32 位编码上 ⇒ `SETCC_FLAG` 读到的 `CF`/`OF` 是**加宽后**的进位/溢出
//! ⇒ 标志恒 0（JIT 复现 `jit_uadd_overflow_flag_is_correct`：值对、标志假）。
//! 16 位走 `addw` 本来是对的，这里一并改写只为"一条路径、语义与机器宽度无关"。
//!
//! 改法：把窄的 `(value, flag) = op(a, b)` 展开成**不依赖标志位**的等价序列——
//! 扩宽算 + 比较区间出标志 + 截回：
//!
//! ```text
//! aw,bw = zext|sext(a,b) → W            // W = I32（加/减）、I64（乘法，16×16 会超 u32）
//! wide  = aw + bw  |  aw - bw  |  aw * bw
//! flag  = wide > MAX                    // 无符号：区间上界即溢出（含 0-1 下溢，模 2^W 后必 > MAX）
//!       |= (wide > MAX) + (wide < MIN) != 0   // 有符号：上下界各判一次
//! value = ireduce(wide → 原位宽)
//! ```
//!
//! 与 `wide_split`/`int_width` 同一层（`LegalizeDriver::scalar`），**对 rustc 与 JIT
//! 两条路径同等生效**——`forge-rustc` 的 `lower_checked_op` 里那份等价实现就是从这里
//! 搬到 IR 层后可以删掉的重复（v27 的"无冗余"目标）。

use forge_ir::ir::opcode::Opcode;
use forge_ir::*;
use smallvec::smallvec;

use super::{ChangeSet, LegalizeCtx, LegalizePass};
use crate::IrError;
use crate::machine::target::TargetMachine;

/// 一条待改写的窄 `*_overflow`。
struct Job {
    block: Block,
    inst: Inst,
    /// 原位宽（8 / 16）。
    bits: u32,
    /// 有符号？（决定 extend 与区间比较的方向）
    signed: bool,
    /// 乘法？（决定中间宽度用 I64）
    mul: bool,
    /// 原 opcode（选 Iadd/Isub/Imul）
    opcode: Opcode,
}

pub(crate) struct NarrowOverflowPass;

impl<M: TargetMachine> LegalizePass<M> for NarrowOverflowPass {
    fn name(&self) -> &'static str {
        "narrow-overflow"
    }

    fn run(
        &self,
        func: &mut Function,
        _ctx: &mut LegalizeCtx<'_, M>,
    ) -> Result<ChangeSet, IrError> {
        let mut jobs: Vec<Job> = Vec::new();
        // 宽于"最大原生位宽"的整数（如 i128）**没有** isel 表示 ⇒ 走 `wide_split`
        // 的分肢；`wide_split` 只认 Iadd/Isub/…，不认 `*_overflow` 家族 ⇒ 128 位的
        // **检查算术**（debug 档 = `opt-level=0` 时收 `debug-assertions` 打开）会
        // fail-closed。这里对 **加/减** 补一条同构改写（宽值的 Iadd 本来就是模 2^N 的）。
        let wmax = _ctx
            .machine
            .int_widths()
            .iter()
            .copied()
            .max()
            .unwrap_or(64) as u32;
        for (b, bd) in func.dfg.blocks() {
            for &ii in bd.inst_order.iter() {
                let inst = func.dfg.inst_data(ii);
                let (signed, mul) = match inst.opcode {
                    Opcode::UaddOverflow | Opcode::UsubOverflow => (false, false),
                    Opcode::SaddOverflow | Opcode::SsubOverflow => (true, false),
                    Opcode::UmulOverflow => (false, true),
                    Opcode::SmulOverflow => (true, true),
                    _ => continue,
                };
                // 判宽用**第一个操作数**（`*_overflow` 的两个操作数与值结果同宽）。
                let Some(t) = inst.operands.first().and_then(|v| func.dfg.value_type(*v)) else {
                    continue;
                };
                let Some(bits) = t.builtin_scalar_bits() else {
                    continue;
                };
                if bits >= 32 && !(bits > wmax && !mul) {
                    // 32/64 位走 ISA 规则（`addl`/`addq`），本来就对；
                    // 宽值只接管**加/减**（乘法仍 fail-closed，等宽 div/rem 落地）。
                    continue;
                }
                let opcode = inst.opcode;
                jobs.push(Job {
                    block: b,
                    inst: ii,
                    bits,
                    signed,
                    mul,
                    opcode,
                });
            }
        }
        if jobs.is_empty() {
            return Ok(ChangeSet::default());
        }

        let (mut new_insts, mut new_values, mut new_consts) = (0usize, 0usize, 0usize);
        // 类型上下文是 Rc 包装的 store（`int_ty` 会 intern）⇒ 先 clone 句柄，
        // 与 `int_width` 同一手法（不能一边 `borrow()` 一边 `make_inst` 拿 `&mut func`）。
        let tys = func.types.clone();
        // 逆序处理：前面的改写不会让后面的位置失效（位置每次重算）。
        for job in jobs.into_iter().rev() {
            let Job {
                block: b,
                inst: ii,
                bits,
                signed,
                mul,
                opcode,
            } = job;
            let narrow_ty = tys.int_ty(bits);
            // **宽值（> wmax，如 i128）不扩宽**：iseli 没有表示，直接在本宽度上做
            // `Iadd`/`Isub`（`wide_split` 会把它分肢）并用**回绕比较**出标志：
            //   无符号加：`flag = value < a`（回绕后必小于任一操作数）
            //   无符号减：`flag = a < b`
            //   有符号  ：`flag = ((a ^ value) & (b ^ value)) < 0`（加）
            //            `flag = ((a ^ b) & (a ^ value)) < 0`（减）
            // 这套不依赖 CF/OF，与宽度无关。
            let is_wide = bits > wmax;
            let wide_bits: u32 = if is_wide {
                bits
            } else if mul {
                64
            } else {
                32
            };
            let wide_ty = tys.int_ty(wide_bits);
            let (a, bb) = {
                let inst = func.dfg.inst_data(ii);
                (inst.operands[0], inst.operands[1])
            };

            let mut made: Vec<Inst> = Vec::with_capacity(12);
            let ext_op = if signed {
                Opcode::Sextend
            } else {
                Opcode::Uextend
            };
            let mut push = |f: &mut Function, opcode, ops, imms, tys: &[TypeId]| -> Value {
                let id = f.make_inst(opcode, b, ops, imms, tys, InstFlags::NONE);
                made.push(id);
                f.dfg.inst_data(id).results[0]
            };
            let arith = match opcode {
                Opcode::UaddOverflow | Opcode::SaddOverflow => Opcode::Iadd,
                Opcode::UsubOverflow | Opcode::SsubOverflow => Opcode::Isub,
                _ => Opcode::Imul,
            };
            if is_wide {
                // 宽值：操作数已经是宽类型，不扩宽；值 = 回绕加/减。
                let value_w = push(func, arith, smallvec![a, bb], smallvec![], &[wide_ty]);
                let zero = {
                    let cid = func.constants.intern_int(0, wide_bits);
                    new_consts += 1;
                    push(
                        func,
                        Opcode::Iconst,
                        smallvec![],
                        smallvec![Immediate::Const(cid)],
                        &[wide_ty],
                    )
                };
                let flag = if signed {
                    // 有符号宽值：`加` → `((a ^ r) & (b ^ r)) < 0`；
                    //               `减` → `((a ^ b) & (a ^ r)) < 0`。
                    let is_add = matches!(arith, Opcode::Iadd);
                    let first = if is_add {
                        push(
                            func,
                            Opcode::Bxor,
                            smallvec![a, value_w],
                            smallvec![],
                            &[wide_ty],
                        )
                    } else {
                        push(
                            func,
                            Opcode::Bxor,
                            smallvec![a, bb],
                            smallvec![],
                            &[wide_ty],
                        )
                    };
                    let second = if is_add {
                        push(
                            func,
                            Opcode::Bxor,
                            smallvec![bb, value_w],
                            smallvec![],
                            &[wide_ty],
                        )
                    } else {
                        push(
                            func,
                            Opcode::Bxor,
                            smallvec![a, value_w],
                            smallvec![],
                            &[wide_ty],
                        )
                    };
                    let and = push(
                        func,
                        Opcode::Band,
                        smallvec![first, second],
                        smallvec![],
                        &[wide_ty],
                    );
                    push(
                        func,
                        Opcode::Icmp,
                        smallvec![and, zero],
                        smallvec![Immediate::IntCC(forge_ir::IntCC::Slt)],
                        &[TypeId::BOOL],
                    )
                } else if matches!(arith, Opcode::Iadd) {
                    push(
                        func,
                        Opcode::Icmp,
                        smallvec![value_w, a],
                        smallvec![Immediate::IntCC(forge_ir::IntCC::Ult)],
                        &[TypeId::BOOL],
                    )
                } else {
                    push(
                        func,
                        Opcode::Icmp,
                        smallvec![a, bb],
                        smallvec![Immediate::IntCC(forge_ir::IntCC::Ult)],
                        &[TypeId::BOOL],
                    )
                };
                new_insts += made.len();
                new_values += made.len();
                let mut popped: Vec<Inst> = Vec::with_capacity(made.len());
                for _ in 0..made.len() {
                    let order = &mut func.dfg.block_mut(b).inst_order;
                    popped.push(order.pop().expect("刚插入的指令必在块尾"));
                }
                popped.reverse();
                let pos = func
                    .dfg
                    .block(b)
                    .inst_order
                    .iter()
                    .position(|x| *x == ii)
                    .expect("正要改写的指令必须在本块的 inst_order 里");
                func.replace_all_uses_and_kill(ii, &[value_w, flag]);
                for (k, &ni) in popped.iter().enumerate() {
                    func.dfg.block_mut(b).inst_order.insert(pos + k, ni);
                }
                continue;
            }
            let aw = push(func, ext_op, smallvec![a], smallvec![], &[wide_ty]);
            let bw = push(func, ext_op, smallvec![bb], smallvec![], &[wide_ty]);
            let wide = push(func, arith, smallvec![aw, bw], smallvec![], &[wide_ty]);

            // 区间常量（按原位宽算，宽类型里表示）
            let (min_v, max_v): (i128, i128) = if signed {
                let half = 1i128 << (bits - 1);
                (-half, half - 1)
            } else {
                (0, (1i128 << bits) - 1)
            };
            let cid_max = func.constants.intern_int(max_v, wide_bits);
            new_consts += 1;
            let max_c = push(
                func,
                Opcode::Iconst,
                smallvec![],
                smallvec![Immediate::Const(cid_max)],
                &[wide_ty],
            );
            let over = push(
                func,
                Opcode::Icmp,
                smallvec![wide, max_c],
                smallvec![Immediate::IntCC(if signed {
                    forge_ir::IntCC::Sgt
                } else {
                    forge_ir::IntCC::Ugt
                })],
                &[TypeId::BOOL],
            );
            let flag = if signed {
                let cid_min = func.constants.intern_int(min_v, wide_bits);
                new_consts += 1;
                let min_c = push(
                    func,
                    Opcode::Iconst,
                    smallvec![],
                    smallvec![Immediate::Const(cid_min)],
                    &[wide_ty],
                );
                let under = push(
                    func,
                    Opcode::Icmp,
                    smallvec![wide, min_c],
                    smallvec![Immediate::IntCC(forge_ir::IntCC::Slt)],
                    &[TypeId::BOOL],
                );
                let i32_ty = tys.int_ty(32);
                let a1 = push(
                    func,
                    Opcode::Uextend,
                    smallvec![over],
                    smallvec![],
                    &[i32_ty],
                );
                let a2 = push(
                    func,
                    Opcode::Uextend,
                    smallvec![under],
                    smallvec![],
                    &[i32_ty],
                );
                let any = push(
                    func,
                    Opcode::Iadd,
                    smallvec![a1, a2],
                    smallvec![],
                    &[i32_ty],
                );
                let cid_zero = func.constants.intern_int(0, 32);
                new_consts += 1;
                let zero = push(
                    func,
                    Opcode::Iconst,
                    smallvec![],
                    smallvec![Immediate::Const(cid_zero)],
                    &[i32_ty],
                );
                push(
                    func,
                    Opcode::Icmp,
                    smallvec![any, zero],
                    smallvec![Immediate::IntCC(forge_ir::IntCC::Ne)],
                    &[TypeId::BOOL],
                )
            } else {
                over
            };
            let value = push(
                func,
                Opcode::Ireduce,
                smallvec![wide],
                smallvec![],
                &[narrow_ty],
            );

            new_insts += made.len();
            new_values += made.len();

            // 新指令都落在块尾（make_inst 的插入点）⇒ 取出来按原顺序插回原位置。
            let mut popped: Vec<Inst> = Vec::with_capacity(made.len());
            for _ in 0..made.len() {
                let order = &mut func.dfg.block_mut(b).inst_order;
                popped.push(order.pop().expect("刚插入的指令必在块尾"));
            }
            popped.reverse();
            let pos = func
                .dfg
                .block(b)
                .inst_order
                .iter()
                .position(|x| *x == ii)
                .expect("正要改写的指令必须在本块的 inst_order 里");
            // 原指令的两个结果 → (value, flag)，然后 kill 原指令。
            func.replace_all_uses_and_kill(ii, &[value, flag]);
            for (k, &ni) in popped.iter().enumerate() {
                func.dfg.block_mut(b).inst_order.insert(pos + k, ni);
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
