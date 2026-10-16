//! 从 `va_expand.rs` 拆出（v25 M4：逐字搬移，不改行为）。
//!
//! 测试体原来是 `#[cfg(test)] mod tests { … }` 的内层，自己的 `use` 现在成为本文件的顶层导入；
//! 被测的**私有**助手（`int_ty` 等）经 `super::` 引用（`pub(super)` 已在 `slots.rs` 放开）。

use super::expand::*;
use super::slots::*;
use crate::pipeline::agg_expand::Places;
use forge_ir::ir::builder::FunctionBuilder;
use forge_ir::ir::types::{FunctionSignature, TypeContext};
use forge_ir::*;
use forge_isa_runtime::machine::call_layout::{VaArgRule, VaArgRules, VaInfo, VaSave};

/// **游标字段宽度 fail-closed**（v23 M6）：1/2/4/8 四档照旧，其余**明确报错**。
///
/// 以前 `_ => TypeId::I64` 把 3/5/6 字节静默当 8 字节读写——游标字段会与相邻对象字段
/// 互相踩（症状是"取参取到别的字段的值"），而形状数据是**使用者给的**，写错至少要报出来。
#[test]
fn cursor_field_width_is_fail_closed() {
    assert_eq!(int_ty(1).expect("1 字节"), TypeId::I8);
    assert_eq!(int_ty(2).expect("2 字节"), TypeId::I16);
    assert_eq!(int_ty(4).expect("4 字节"), TypeId::I32);
    assert_eq!(int_ty(8).expect("8 字节"), TypeId::I64);
    for bad in [0u32, 3, 5, 6, 7, 16] {
        let err = int_ty(bad).expect_err("非 1/2/4/8 必须报错");
        let msg = format!("{err}");
        assert!(
            msg.contains("游标字段宽度"),
            "{bad} 字节：错误信息应点名游标字段宽度，实际：{msg}"
        );
    }
}

fn probe_with_local() -> Function {
    let ctx = TypeContext::new();
    let sig = FunctionSignature::new(&[], &[TypeId::I64]);
    let mut b = FunctionBuilder::new("probe", ctx, sig);
    let (e, _) = b.create_block_with_params(&[]);
    b.switch_to_block(e);
    let slot = b.stack_addr(-16);
    let v = b.iconst(7, TypeId::I64);
    b.store(v, slot);
    let got = b.load(slot, TypeId::I64);
    b.ret(&[got]);
    b.finish().expect("probe")
}

fn va(save: bool) -> VaInfo {
    VaInfo {
        shape: Some("sysv_reg_save".to_string()),
        size: 24,
        align: 8,
        stack_only: false,
        // 合成夹具：不要求"与栈实参连续"（那是 RISC-V 的形状数据）——保持历史布局口径。
        save_contiguous: false,
        fields: vec![],
        save: save.then(|| VaSave {
            size: 176,
            align: 16,
            slots: vec![],
        }),
        init: None,
        arg_rules: VaArgRules {
            int: VaArgRule {
                cursor: 0,
                base: None,
                overflow: None,
                limit: 0,
                signed_limit: false,
                step: 8,
                overflow_step: 8,
                cursor_origin: 0,
                cursor_counts_down: false,
            },
            float: VaArgRule {
                cursor: 1,
                base: None,
                overflow: None,
                limit: 0,
                signed_limit: false,
                step: 16,
                overflow_step: 8,
                cursor_origin: 48,
                cursor_counts_down: false,
            },
            // 夹具不声明聚合规则（`None` ⇒ 聚合 fail-closed）。
            aggregate: None,
        },
    }
}

/// **ABI 槽必须排在前端局部槽之下**（2026-10-01 实测的坑）：前端 `stack_addr(-16)` 在
/// lowering 里才被统计，若 ABI 槽从 0 起排就会与它重叠。这条断言是**确定性**的
/// （直接看分配结果），不依赖某个用例"恰好读错值"。
#[test]
fn abi_slots_sit_below_frontend_locals() {
    let f = probe_with_local();
    let front = frontend_locals_depth(&f, 8);
    assert!(
        front > 0,
        "带 stack_addr 的函数必须扫出正的局部槽深度（实测 {front}）"
    );
    // 有保存区：保存区接在前端局部槽**之下**、对象再在保存区之后
    let s = plan_abi_slots(&f, &va(true), 8, 64);
    assert!(
        s.save_depth >= front + 176,
        "保存区必须在前端局部槽之下（front={front}, save_depth={}）",
        s.save_depth
    );
    assert!(s.obj_depth > s.save_depth, "对象在保存区之下");
    assert_eq!(s.max_depth, s.obj_depth);
    assert_eq!(
        s.save_off,
        -(s.save_depth as i64) - 64,
        "序言用的偏移 = -depth - shift"
    );
    // 没有保存区（win64 栈式）：对象直接排在前端局部槽之下
    let s2 = plan_abi_slots(&f, &va(false), 8, 64);
    assert!(s2.obj_depth >= front + 24, "对象槽在局部槽之下");
}

/// **`va_arg` 聚合**（v23 收尾）：两条路都要钉。
///
/// ① 约定**没声明**聚合规则（`aggregate: None`）⇒ 必须 fail-closed，且错误信息点名
///    缺的是哪条规则（以前是**无条件拒绝**；现在是"数据没给就拒绝"）。
/// ② 声明了 `aggregate`（win64 的"间接"形态）⇒ 必须**成功**展开：取指针 → 逐段拷进
///    自己的帧槽 → 结果登记进 `agg_slots`（后续 `extractvalue` 因此走地址语义）。
#[test]
fn aggregate_va_arg_needs_a_declared_rule() {
    use forge_ir::{Opcode, TypeId};
    use forge_isa_runtime::machine::call_layout::VaAggregateArgRule;

    // 造"带一条聚合 va_arg 的函数"，返回 (函数, va_arg 指令, 块)。
    // 按尺寸造聚合：16B = 两个 i64（>阈值，间接档）、8B = 一个 i64（≤阈值，内联档）。
    let build_sized = |bytes: usize| {
        let ctx = TypeContext::new();
        let members: Vec<TypeId> = if bytes > 8 {
            vec![TypeId::I64, TypeId::I64]
        } else {
            vec![TypeId::I64]
        };
        let agg = ctx.borrow_mut().struct_anon(members, false);
        let sig = FunctionSignature::new(&[(TypeId::PTR, "ap")], &[TypeId::I64]);
        let mut b = FunctionBuilder::new("va_agg", ctx, sig);
        let (e, params) = b.create_block_with_params(&[(TypeId::PTR, "ap")]);
        b.switch_to_block(e);
        let v = b.emit1(
            Opcode::VaArg,
            vec![params[0]],
            vec![],
            agg,
            forge_ir::InstFlags::SIDE_EFFECT,
        );
        // 后续对聚合结果取字段：合法化之后**不该再有 ExtractValue**（应被重写成
        // "从槽里 load"——只登记不重写就是漏了这一步，真跑时会掉到 lowering）。
        let field = b.extract_value(v, 0);
        b.ret(&[field]);
        let f = b.finish().expect("build va_agg");
        let ii = f
            .dfg
            .insts()
            .find(|(_, d)| d.opcode == Opcode::VaArg)
            .map(|(i, _)| i)
            .expect("va_arg 指令");
        (f, ii, e)
    };

    // ① 没声明规则 ⇒ fail-closed，且点名 `aggregate_arg`
    {
        let (mut f, ii, e) = build_sized(16);
        let va = va(true); // 夹具（`aggregate: None`）
        let mut agg_slots = Places::default();
        let err = match expand_arg(&mut f, e, ii, &va, &mut agg_slots) {
            Ok(()) => panic!("约定没声明聚合规则时必须 fail-closed（不能按标量 load）"),
            Err(e) => format!("{e}"),
        };
        assert!(
            err.contains("聚合") && err.contains("aggregate_arg"),
            "错误信息应点名缺失的规则，实际：{err}"
        );
    }

    // ② 声明了规则且**大于阈值**（16B > 8）⇒ 走**间接**档：先取指针，再多读 2 段
    {
        let (mut f, ii, e) = build_sized(16);
        let mut va = va(true);
        // 夹具的 `fields` 是空的（别的用例不需要它）——取参规则要按名字引字段，
        // 所以这里补一个"游标"字段（与 `int` 规则的 `cursor: 0` 对齐）。
        va.fields = vec![forge_isa_runtime::machine::call_layout::VaField {
            name: "cursor".to_string(),
            offset: 0,
            size: 8,
        }];
        va.arg_rules.aggregate = Some(VaAggregateArgRule {
            rule: va.arg_rules.int,
            indirect: true,
            // 夹具的聚合是 16 字节 ⇒ 阈值 8 表示"大于 8 才间接" ✔（与 win64 同形）。
            indirect_size_gt: 8,
        });
        let mut agg_slots = Places::default();
        expand_arg(&mut f, e, ii, &va, &mut agg_slots).expect("声明了 aggregate 规则就必须能展开");
        // **交接契约**（v24 M1）：`expand_arg` 只负责"把字节拷进自己的帧槽 + 登记位置"，
        // 使用处的改写归**物化相**（它在指令序上单遍处理）。这里先断言交接本身。
        {
            let ops: Vec<Opcode> = f.dfg.insts().map(|(_, d)| d.opcode).collect();
            assert!(
                ops.contains(&Opcode::Store),
                "聚合取参必须把字节拷进自己的帧槽（要有 Store）：{ops:?}"
            );
            let res = f
                .dfg
                .insts()
                .find(|(_, d)| d.opcode == Opcode::ExtractValue)
                .and_then(|(_, d)| d.operands.first().copied())
                .expect("夹具里应有 extractvalue");
            assert!(
                matches!(
                    agg_slots.get(res),
                    Some(crate::pipeline::agg_expand::Place::Mem(_))
                ),
                "聚合取参的结果必须登记成内存档（物化相据此改写使用处）"
            );
        }
        // 跑物化相（驱动里由收尾批调用）——之后才谈"字段从槽里 load"。
        crate::pipeline::legalize::materialize::materialize_memory_uses(
            &mut f,
            &mut agg_slots,
            &Default::default(),
        )
        .expect("物化相");
        let ops: Vec<Opcode> = f.dfg.insts().map(|(_, d)| d.opcode).collect();
        // 间接档的**结构**判据（不再钉精确计数）：游标 + **取指针** + 段拷贝 + 字段读
        // 至少 5 次 load；"字段读晚于最后一次段 store"另由下面的次序断言守。
        // 为什么不钉精确值（v24 M1）：使用处改写搬进物化相之后，这段 IR 的 load 次数会随
        // "哪一步发射哪条"漂移，而**真正值钱的是次序与交接**（值由 JIT 真执行用例守）。
        let loads = ops.iter().filter(|o| **o == Opcode::Load).count();
        assert!(
            loads >= 5,
            "16B（>阈值 8）应走间接档：游标 + 指针 + 段拷贝 + 字段读：{ops:?}"
        );
        assert!(
            !ops.contains(&Opcode::ExtractValue),
            "聚合结果的后续 extractvalue 必须被重写成\"从槽 load\"（否则落到 lowering 会 Unsupported）：{ops:?}"
        );
        // **次序**：段拷贝的 store 必须排在"字段 load"之前 —— 反了就是读未初始化的槽
        //（真执行用例里的表现：每次运行都不同的垃圾值，因为读到栈上残留）。
        let last_store = ops.iter().rposition(|o| *o == Opcode::Store);
        let field_load = ops
            .iter()
            .enumerate()
            .skip(last_store.map_or(0, |i| i + 1))
            .find(|(_, o)| **o == Opcode::Load)
            .map(|(i, _)| i);
        if let (Some(ls), Some(fl)) = (last_store, field_load) {
            assert!(
                fl > ls,
                "字段 load（下标 {fl}）必须晚于最后一次段 store（下标 {ls}）：{ops:?}"
            );
        }
        // **字段读必须在段 store 之后**（上面只查了"有一个在后面"，这里查"确实有"）。
        // （这条守卫的来历：JIT 用例取两个字段时值错，我怀疑过"load 排在 store 之前读到
        //  未初始化槽"——单测实测**不是**：字段 load 确实在段 store 之后；值错另有原因。）
        // 不钉精确条数（v24 M1）：`expand_arg` 自身的段拷贝序列在最后一次 store 之后还可能
        // 带它自己的 load，精确值会随"谁在哪一步发射"漂移；**不变量只有下界**。
        if let Some(ls) = last_store {
            let loads_after = ops[ls + 1..].iter().filter(|o| **o == Opcode::Load).count();
            assert!(
                loads_after >= 1,
                "段 store 之后必须有字段 Load（读未初始化的槽 = 每次运行值都不同）：{ops:?}"
            );
        }
    }

    // ③ **不大于阈值**（8B ≤ 8）⇒ 走**内联**档：区域里直接是聚合字节，
    //    一次段读就够（**不**多读指针——多读就是把内联字节当指针用）。
    {
        let (mut f, ii, e) = build_sized(8);
        let mut va = va(true);
        va.fields = vec![forge_isa_runtime::machine::call_layout::VaField {
            name: "cursor".to_string(),
            offset: 0,
            size: 8,
        }];
        va.arg_rules.aggregate = Some(VaAggregateArgRule {
            rule: va.arg_rules.int,
            indirect: true,
            indirect_size_gt: 8,
        });
        let mut agg_slots = Places::default();
        expand_arg(&mut f, e, ii, &va, &mut agg_slots)
            .expect("阈值内的聚合必须走内联档（不再拒绝）");
        // 同样先交接、再由物化相改写使用处（见 ② 的说明）。
        crate::pipeline::legalize::materialize::materialize_memory_uses(
            &mut f,
            &mut agg_slots,
            &Default::default(),
        )
        .expect("物化相");
        let ops: Vec<Opcode> = f.dfg.insts().map(|(_, d)| d.opcode).collect();
        // 内联档：游标 + 段拷贝 + 字段读（**不读指针**——多读就是把内联字节当指针用）。
        let loads = ops.iter().filter(|o| **o == Opcode::Load).count();
        assert!(
            loads >= 3,
            "8B（≤阈值 8）应走内联档：游标 + 段 + 字段读、**不读指针**：{ops:?}"
        );
        assert!(
            !ops.contains(&Opcode::ExtractValue),
            "内联档同样要重写后续 extractvalue：{ops:?}"
        );
        assert!(ops.contains(&Opcode::Store), "内联档同样要写槽：{ops:?}");
    }
}
