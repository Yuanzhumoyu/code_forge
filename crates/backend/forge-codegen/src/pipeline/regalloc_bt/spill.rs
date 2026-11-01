//! 从 `regalloc_bt.rs` 拆出（v25 M4：逐字搬移，不改行为）。

use crate::IrError;
use std::collections::HashMap;

use crate::pipeline::alloc_config::RegAllocConfig;
use crate::pipeline::alloc_result::{SpillEdit, SpillSlot};
use forge_ir::*;

/// **逐指令溢出编辑**（v23 M5 的 `SpillEdit`）：决策在分配器里做出、发射侧只按序执行。
///
/// 原来决策与执行混在发射侧（`emit_inst_with_spills` 现查最终表、现挑 scratch、现判
/// `settable`）——分配器真正的逐点状态反而用不上。搬到这里之后：① scratch 池不够 =
/// **分配期** fail-closed；② `settable` 预检也在这里（指令在手）；③ 将来要驱逐**活跃值**，
/// 只需多产出一条 `Store`。
///
/// 口径与旧发射实现逐条对齐（**输出等价**）：scratch 的**号**取自 `config.scratch_regs`、
/// **类**取值的类；每条指令的编辑顺序是 `Reload`（全部被溢出的值）→ `Bind`（每个被溢出的
/// 绑定）→ `Store`（被溢出的 def）。没被溢出的绑定**不**产生编辑：它的字段已由管线在发射
/// 前按 `inst_ops` 统一回写过（旧发射实现里那次重写是冗余的）。
pub(super) fn build_spill_edits<I: crate::machine::inst::MachineInst>(
    vcode: &crate::VCode<I>,
    inst_ops: &[smallvec::SmallVec<[crate::machine::lowering::OperandBind; 2]>],
    assignments: &[Option<PReg>],
    spill_slots: &[Option<SpillSlot>],
    config: &RegAllocConfig,
) -> Result<Vec<Vec<SpillEdit>>, IrError> {
    use crate::machine::inst::OperandRole;
    // **逐点判据**（取证 ②）：此刻真的在栈上 = 有槽 **且** 没有寄存器。只看"有槽"会把
    // "曾被驱逐、之后 reload 回寄存器"的值也当成栈上的——那会把字段重定向到 scratch 并
    // 从槽里读**陈旧值**。
    // **判据回到"有槽"**（取证 ② 被推翻）：发射前的统一回写用的是**最终**分配表，所以
    // 只要某 vreg 有槽，就一律"从槽经 scratch 读"——这样才与回写无关地自洽。反过来按
    // "此刻没有寄存器"判，会让**早期**指令的字段停在**最终**（可能更晚的）寄存器号上，
    // 读到那个时刻的垃圾（实测：单跑 atomic 用例直接 0xc0000005 访问违规）。
    let slot_of =
        |v: XReg| -> Option<SpillSlot> { spill_slots.get(v.index() as usize).copied().flatten() };
    let mut edits: Vec<Vec<SpillEdit>> = Vec::new();
    let mut gi = 0usize;
    // **溢出事件不产生编辑**（2026-10-11 取证；2026-10-19 两次实测复核）：槽是**权威副本**
    // （def 走 scratch 写槽、use 从槽读），寄存器里往往是**陈旧值**；在溢出点把寄存器写回槽
    // 会覆盖正确内容。两次尝试（"驱逐即落槽"与"仅首次落槽"）都打红了原本绿的用例
    //（第二次直接让 `jit_stack_param_loop_const_bound_with_call` 死循环），故整条回退。
    // 诊断探针保留在 `spill_vreg`（`FORGE_VERIFY_SPILL_LIVE`）。
    for block in vcode.blocks() {
        for inst in block.instructions.iter() {
            let mut e: Vec<SpillEdit> = Vec::new();
            if let Some(binds) = inst_ops.get(gi) {
                // 去重收集被溢出的 vreg（同一 vreg 的 use/def 共用同一个 scratch）。
                let mut spilled: smallvec::SmallVec<[XReg; 8]> = smallvec::SmallVec::new();
                for b in binds.iter() {
                    if slot_of(b.vreg).is_some() && !spilled.contains(&b.vreg) {
                        spilled.push(b.vreg);
                    }
                }
                if !spilled.is_empty() {
                    // ── scratch 规划（2026-11-01 重写）──
                    //
                    // 一个 scratch **可以同时承载本指令的一个 use 与 def**：指令先读完
                    // 所有输入、再写输出，所以 `add s1, s1, s2`（def 与 b 共用 s1）与
                    // 三条不同的 scratch 等价。旧实现给每个 spilled vreg 各分一个槽，
                    // 于是"三操作数 ALU 三个操作数全溢出"要 **3** 个 scratch——只声明
                    // 2 个的谱（Symphony 就是）直接编译失败，而它其实只需要 2 个。
                    //
                    // 唯一不能共用的是**同一字段既是输入又是输出**（x86 两地址指令
                    // `add dst, src`：dst 的 scratch 必须持有旧值）——判据取自指令的
                    // 操作数表（同字段既有非 Def 项）。
                    let inplace_field = |f: u8| {
                        inst.operands().iter().any(|o| {
                            o.field == f && o.role != crate::machine::inst::OperandRole::Def
                        })
                    };
                    let mut slot_of_v: HashMap<XReg, usize> = HashMap::new();
                    let mut use_slots: smallvec::SmallVec<[usize; 4]> = smallvec::SmallVec::new();
                    let mut next_slot = 0usize;
                    // ① 先把 use 各自占一个槽（两条 use 的值必须同时在寄存器里）。
                    for b in binds
                        .iter()
                        .filter(|b| b.role != crate::machine::inst::OperandRole::Def)
                    {
                        if slot_of(b.vreg).is_none() || slot_of_v.contains_key(&b.vreg) {
                            continue;
                        }
                        slot_of_v.insert(b.vreg, next_slot);
                        use_slots.push(next_slot);
                        next_slot += 1;
                    }
                    // ② def：同值（in-place 的同一 vreg）沿用它的 use 槽；不同值且字段
                    //    不是 in-place ⇒ 复用一个 use 槽（先读后写，安全）；否则新槽。
                    let mut pure_def_ord = 0usize;
                    for b in binds
                        .iter()
                        .filter(|b| b.role == crate::machine::inst::OperandRole::Def)
                    {
                        if slot_of(b.vreg).is_none() || slot_of_v.contains_key(&b.vreg) {
                            continue;
                        }
                        let idx = if !inplace_field(b.field) && pure_def_ord < use_slots.len() {
                            let s = use_slots[pure_def_ord];
                            pure_def_ord += 1;
                            s
                        } else {
                            let s = next_slot;
                            next_slot += 1;
                            s
                        };
                        slot_of_v.insert(b.vreg, idx);
                    }
                    if next_slot > config.scratch_regs.len() {
                        return Err(IrError::RegAlloc(format!(
                            "第 {gi} 条指令需要 {next_slot} 个 scratch 寄存器搬溢出值，\
                             但本 ISA 只声明了 {} 个（def 已尽量与 use 共用槽；还不够说明\
                             这条指令的**同时**活跃溢出操作数过多）",
                            config.scratch_regs.len()
                        )));
                    }
                    // scratch 绑定：号来自 scratch 池，**类取值的类**（与旧发射口径一致）。
                    let scratch_of =
                        |v: XReg, i: usize| PReg::new(config.scratch_regs[i].num, v.class());
                    for &v in spilled.iter() {
                        let slot = slot_of(v).expect("上面刚判过该 vreg 有溢出槽");
                        let i = slot_of_v[&v];
                        e.push(SpillEdit::Reload {
                            slot,
                            scratch: scratch_of(v, i),
                            class: v.class(),
                            width: v.bytes(),
                        });
                    }
                    let mut stored: smallvec::SmallVec<[XReg; 8]> = smallvec::SmallVec::new();
                    for b in binds.iter() {
                        let Some(i) = slot_of_v.get(&b.vreg).copied() else {
                            continue;
                        };
                        if !inst.is_reg_field_settable(usize::from(b.field)) {
                            return Err(IrError::RegAlloc(format!(
                                "第 {gi} 条指令的字段 {} 不可改写（固定物理字段）\
                                 却是被溢出值的落点",
                                b.field
                            )));
                        }
                        e.push(SpillEdit::Bind {
                            field: b.field,
                            scratch: scratch_of(b.vreg, i),
                            // **与该绑定的折入类一致**（`bind.class` = 值宽类，多类槽靠它定
                            // opsize；退到 vreg 的池宽类是 WA-35 的老病）。
                            class: b.class.unwrap_or_else(|| b.vreg.class()),
                        });
                        if b.role == OperandRole::Def && !stored.contains(&b.vreg) {
                            stored.push(b.vreg);
                            let slot = slot_of(b.vreg).expect("上面刚判过该 vreg 有溢出槽");
                            e.push(SpillEdit::Store {
                                slot,
                                scratch: scratch_of(b.vreg, i),
                                class: b.vreg.class(),
                                width: b.vreg.bytes(),
                            });
                        }
                    }
                }
            }
            edits.push(e);
            gi += 1;
        }
    }
    let _ = assignments;
    Ok(edits)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::machine::inst::{InstOperand, MachineInst, OperandConstraint, OperandRole};
    use crate::machine::lowering::OperandBind;
    use forge_ir::{Block, PReg, RegClass, XReg};

    /// `add {a:out}, {b}, {c}` 的替身：**一条指令三个寄存器槽**（1 Def + 2 Use）。
    #[derive(Clone, Debug, PartialEq, Eq, Hash)]
    struct ThreeOp;

    impl MachineInst for ThreeOp {
        fn operands(&self) -> smallvec::SmallVec<[InstOperand; 4]> {
            let cls = RegClass::GPR(64);
            let op = |field: u8, role: OperandRole| InstOperand {
                field,
                slot: Some(u32::from(field)),
                class: Some(cls),
                constraint: OperandConstraint::Any,
                role,
                settable: true,
            };
            // 契约：Use 在前、Def 在后。
            smallvec::smallvec![
                op(1, OperandRole::Use),
                op(2, OperandRole::Use),
                op(0, OperandRole::Def),
            ]
        }
        fn is_branch(&self) -> bool {
            false
        }
        fn branch_targets(&self) -> smallvec::SmallVec<[Block; 2]> {
            smallvec::smallvec![]
        }
        fn is_call(&self) -> bool {
            false
        }
        fn is_ret(&self) -> bool {
            false
        }
        fn is_reg_field_settable(&self, i: usize) -> bool {
            i < 3
        }
    }

    fn cfg(scratch: usize) -> RegAllocConfig {
        let mut c = RegAllocConfig::new(8, 8, 30, None);
        c.scratch_regs = (9..9 + scratch as u32)
            .map(|n| PReg::new(n, RegClass::GPR(64)))
            .collect();
        c
    }

    /// 一条指令的 3 个操作数**全部**溢出 ⇒ 2 个 scratch 够：def 与一个 use 共用槽
    /// （指令先读完输入再写输出）。旧实现要 3 个 ⇒ 2 scratch 的谱直接编译失败。
    #[test]
    fn three_spilled_operands_share_two_scratch_slots() {
        let mut vcode: crate::VCode<ThreeOp> = crate::VCode::new();
        let b = vcode.create_block(Block::new(0));
        vcode.switch_to_block(b);
        vcode.push_inst(ThreeOp);

        let v = |n: u32| XReg::new(n, RegClass::GPR(64));
        let bind = |n: u32, field: u8, role: OperandRole| OperandBind {
            vreg: v(n),
            field,
            role,
            class: Some(RegClass::GPR(64)),
        };
        let inst_ops = vec![smallvec::smallvec![
            bind(0, 0, OperandRole::Def),
            bind(1, 1, OperandRole::Use),
            bind(2, 2, OperandRole::Use),
        ]];
        let slots: Vec<Option<SpillSlot>> = (0..3)
            .map(|i| {
                Some(SpillSlot {
                    offset: i * 8,
                    size: 8,
                })
            })
            .collect();

        let edits = build_spill_edits(&vcode, &inst_ops, &[], &slots, &cfg(2))
            .expect("3 个操作数全溢出时 2 个 scratch 必须够");
        let mut used: Vec<u32> = edits[0]
            .iter()
            .filter_map(|e| match e {
                SpillEdit::Bind { scratch, .. } => Some(scratch.num),
                _ => None,
            })
            .collect();
        used.sort_unstable();
        used.dedup();
        assert_eq!(used.len(), 2, "三个绑定只能占 2 个 scratch：{used:?}");

        // 只给 1 个 scratch ⇒ 必须 fail-closed（两条 use 的值必须同时可读）。
        let err = build_spill_edits(&vcode, &inst_ops, &[], &slots, &cfg(1))
            .expect_err("1 个 scratch 装不下两条 use");
        assert!(
            format!("{err}").contains("scratch"),
            "错误信息应点明 scratch 不够：{err}"
        );
    }

    /// in-place 字段（`add dst, src`：dst 既读又写）**不能**与另一个 use 共用槽——
    /// 它的 scratch 必须持有旧值。
    #[test]
    fn inplace_def_does_not_alias_another_use_slot() {
        #[derive(Clone, Debug, PartialEq, Eq, Hash)]
        struct InPlace;
        impl MachineInst for InPlace {
            fn operands(&self) -> smallvec::SmallVec<[InstOperand; 4]> {
                let cls = RegClass::GPR(64);
                let op = |field: u8, role: OperandRole| InstOperand {
                    field,
                    slot: Some(u32::from(field)),
                    class: Some(cls),
                    constraint: OperandConstraint::Any,
                    role,
                    settable: true,
                };
                // dst（字段 0）既是 Use 又是 Def ⇒ in-place。
                smallvec::smallvec![
                    op(0, OperandRole::Use),
                    op(1, OperandRole::Use),
                    op(0, OperandRole::Def),
                ]
            }
            fn is_branch(&self) -> bool {
                false
            }
            fn branch_targets(&self) -> smallvec::SmallVec<[Block; 2]> {
                smallvec::smallvec![]
            }
            fn is_call(&self) -> bool {
                false
            }
            fn is_ret(&self) -> bool {
                false
            }
            fn is_reg_field_settable(&self, i: usize) -> bool {
                i < 2
            }
        }

        let mut vcode: crate::VCode<InPlace> = crate::VCode::new();
        let b = vcode.create_block(Block::new(0));
        vcode.switch_to_block(b);
        vcode.push_inst(InPlace);

        let v = |n: u32| XReg::new(n, RegClass::GPR(64));
        let bind = |n: u32, field: u8, role: OperandRole| OperandBind {
            vreg: v(n),
            field,
            role,
            class: Some(RegClass::GPR(64)),
        };
        // dst(v0) 既 Use 又 Def；src(v1) 是另一个 use。
        let inst_ops = vec![smallvec::smallvec![
            bind(0, 0, OperandRole::Use),
            bind(1, 1, OperandRole::Use),
            bind(0, 0, OperandRole::Def),
        ]];
        let slots: Vec<Option<SpillSlot>> = (0..2)
            .map(|i| {
                Some(SpillSlot {
                    offset: i * 8,
                    size: 8,
                })
            })
            .collect();

        // in-place 的 dst 需要独立槽 ⇒ 2 个 use 各一槽，1 个 scratch 不够。
        let err = build_spill_edits(&vcode, &inst_ops, &[], &slots, &cfg(1))
            .expect_err("in-place 的 dst 不能与 src 共用槽");
        assert!(format!("{err}").contains("scratch"), "{err}");
        assert!(
            build_spill_edits(&vcode, &inst_ops, &[], &slots, &cfg(2)).is_ok(),
            "2 个 scratch 应足够（dst 一个、src 一个）"
        );
    }
}
