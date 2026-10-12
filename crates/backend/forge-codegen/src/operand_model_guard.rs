//! **操作数模型一致性守卫**（v23 M4）。
//!
//! ## 两个阶段的作用
//!
//! - **批次 A（已过）**：这里是**双边对拍**——`operands()`（新表）对
//!   `uses`/`defs`/`use_constraints`/`def_constraints`/`reg_field`/
//!   `is_reg_field_settable`（旧方法，当时生成物还发射它们）。它证明两边在
//!   **三份发行谱的全部指令变体**上逐条一致（含 `def_reuse → ReuseInput` 映射），
//!   这是"删旧方法"的前提。
//! - **批次 B 之后**：那 4 个查询方法改由 `operands()` 的缺省实现派生、生成物不再发射
//!   （回潮守卫在 `forge-isa-dsl/tests/machine_shape_table.rs`），因此本文件守的是
//!   **派生本身在全部变体上成立**，外加两条**不依赖派生结果**的独立判据：
//!
//! 1. **顺序契约**：`Use` 全部排在 `Def` 之前——regalloc 的"use 在 inst_start、
//!    def 在 inst_end"由此保证，顺序错了活区间就错（静默错值）；
//! 2. **`is_move()` 交叉判据**：`is_move()` 由**另一张表**（`move_arms`）驱动，
//!    它说有搬移的指令必须恰好 1 个 `Def` 且至少 1 个 `Use`。
//!
//! ## 关于虚拟寄存器
//!
//! 只对拍**形状面**（字段/槽/约束/角色/可改写性）。微指令字段里放的是**占位物理寄存器**
//! （`Reg`），vreg → 字段的对应仍由 lowering 的 `InstPacket::map_reg_field` 给出；
//! 把它并进本表是 M4 批次 C 的前置条件（让字段占位直接携带 vreg 号），见方案 §13。

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

            // ── 不依赖派生结果的两条独立判据（批次 B 之后本守卫的主要价值）──
            //
            // ① 顺序契约：`Use` 全部在 `Def` 之前（regalloc 的"use 在 inst_start、
            //    def 在 inst_end"由此保证）。
            let first_def = ops.iter().position(|o| o.role == OperandRole::Def);
            let last_use = ops.iter().rposition(|o| o.role == OperandRole::Use);
            if let (Some(d), Some(u)) = (first_def, last_use) {
                assert!(
                    u < d,
                    "{}/{}: 操作数顺序违反契约（有 Use 排在 Def 之后）：{ops:?}",
                    $name,
                    inst_name
                );
            }
            // ② `is_move()` 与操作数表交叉核对（`is_move` 由另一张表 `move_arms` 驱动）。
            if let Some((dst, src)) = inst.is_move() {
                let defs_n = ops
                    .iter()
                    .filter(|o| o.role == OperandRole::Def)
                    .count();
                let uses_n = ops
                    .iter()
                    .filter(|o| o.role == OperandRole::Use)
                    .count();
                assert_eq!(
                    (defs_n, uses_n >= 1),
                    (1, true),
                    "{}/{}: `is_move()` 说这是搬移（{dst} ← {src}），但操作数表是 {ops:?}",
                    $name,
                    inst_name
                );
            }
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
fn operand_table_is_consistent_across_all_variants() {
    let x86 = check_isa!("x86", crate::arch::x86::x86);
    let riscv = check_isa!("riscv64", crate::arch::riscv64::riscv64);
    let arm = check_isa!("arm64", crate::arch::arm64::arm64);
    // 可见性事件：libtest 吞掉通过测试的输出，写一行便于在 CI 日志里确认覆盖量\n    // （当前实测：x86 1550 / riscv64 624 / arm64 2581 个指令变体）。
    eprintln!("[operands] 对拍指令变体：x86 {x86}、riscv64 {riscv}、arm64 {arm}");
}
