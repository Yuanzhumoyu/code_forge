//! **操作数模型一致性守卫**（v23 M4 批次 A 的接口等价证明）。
//!
//! ## 钉什么
//!
//! M4 把七个查询/回写方法（`uses`/`defs`/`use_constraints`/`def_constraints`/
//! `reg_field`/`set_reg_field`/`is_reg_field_settable`）与指令包里的平行旁表
//! `xreg_map` 收敛成**一张操作数表**（`MachineInst::operands()`）。
//! 收敛过程中最危险的一步是"删旧方法"——只要旧方法与新表在**任何一条指令**上不一致，
//! regalloc 拿到的活区间/约束就变了，而症状是静默错值（不是编译错误）。
//!
//! 因此本守卫在删旧之前先把两者**逐指令对拍**：三份发行谱的**全部**指令变体
//! （`__spec_tests::all_insts()`，谱派生枚举器）都要满足
//!
//! - `uses()`  == `operands()` 里 `Use` 的槽（跳过无槽字段）；
//! - `defs()`  == `operands()` 里 `Def` 的槽；
//! - `use_constraints()` / `def_constraints()` == `operands()` 里对应角色的约束
//!   （`def_reuse` 的 `ReuseInput` 特别重要：它决定 x86 双操作数指令的复用）；
//! - `reg_field(field)` == 该操作数的槽（无槽 = 0）；
//! - `is_reg_field_settable(field)` == 该操作数的 `settable`。
//!
//! 说明：只对拍**形状面**（字段/槽/约束/角色/可改写性）。虚拟寄存器 → 字段的对应
//! 仍由 lowering 的 `InstPacket::map_reg_field` 给出（管线侧旁表），把它并进本表是
//! M4 批次 C 的前置条件（让字段占位直接携带 vreg 号），见方案 §13。

use forge_isa_runtime::machine::inst::{MachineInst, OperandRole};

/// 逐指令对拍一份谱的 `MachineInst` 实现。
macro_rules! check_isa {
    ($name:expr, $m:path) => {{
        use $m as isa;
        let all = isa::__spec_tests::all_insts();
        assert!(!all.is_empty(), "{}: 派生枚举器为空", $name);
        let mut checked = 0usize;
        let mut with_operands = 0usize;
        for (inst_name, inst) in all {
            let ops = inst.operands();

            let uses: Vec<u32> = ops
                .iter()
                .filter(|o| o.role == OperandRole::Use)
                .filter_map(|o| o.slot)
                .collect();
            assert_eq!(
                inst.uses().to_vec(),
                uses,
                "{}/{}: `uses()` 与操作数表不一致",
                $name,
                inst_name
            );

            let defs: Vec<u32> = ops
                .iter()
                .filter(|o| o.role == OperandRole::Def)
                .filter_map(|o| o.slot)
                .collect();
            assert_eq!(
                inst.defs().to_vec(),
                defs,
                "{}/{}: `defs()` 与操作数表不一致",
                $name,
                inst_name
            );

            let use_c: Vec<_> = ops
                .iter()
                .filter(|o| o.role == OperandRole::Use)
                .map(|o| o.constraint)
                .collect();
            assert_eq!(
                inst.use_constraints().to_vec(),
                use_c,
                "{}/{}: use 约束与操作数表不一致",
                $name,
                inst_name
            );

            let def_c: Vec<_> = ops
                .iter()
                .filter(|o| o.role == OperandRole::Def)
                .map(|o| o.constraint)
                .collect();
            assert_eq!(
                inst.def_constraints().to_vec(),
                def_c,
                "{}/{}: def 约束与操作数表不一致（`def_reuse` 的 ReuseInput 映射）",
                $name,
                inst_name
            );

            for op in &ops {
                let f = usize::from(op.field);
                assert_eq!(
                    inst.reg_field(f),
                    op.slot.unwrap_or(0),
                    "{}/{}: `reg_field({})` 与操作数表的槽不一致",
                    $name,
                    inst_name,
                    f
                );
                assert_eq!(
                    inst.is_reg_field_settable(f),
                    op.settable,
                    "{}/{}: 字段 {} 的可改写性与操作数表不一致",
                    $name,
                    inst_name,
                    f
                );
            }
            checked += 1;
            if !ops.is_empty() {
                with_operands += 1;
            }
        }
        assert!(
            with_operands > 0,
            "{}: 没有任何指令带操作数——扫描口径失效",
            $name
        );
        checked
    }};
}

#[test]
fn operand_table_agrees_with_the_legacy_methods() {
    let x86 = check_isa!("x86", crate::arch::x86::x86);
    let riscv = check_isa!("riscv64", crate::arch::riscv64::riscv64);
    let arm = check_isa!("arm64", crate::arch::arm64::arm64);
    // 可见性事件：libtest 吞掉通过测试的输出，写一行便于在 CI 日志里确认覆盖量。
    eprintln!("[operands] 对拍指令变体：x86 {x86}、riscv64 {riscv}、arm64 {arm}");
}
