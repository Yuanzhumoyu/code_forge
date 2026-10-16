//! 从 `regalloc_bt.rs` 拆出（v25 M4：逐字搬移，不改行为）。

use super::*;
use crate::machine::inst::DummyInst;
use crate::machine::lowering::OperandBind;
use forge_ir::entity::*;

fn xgpr(n: u32) -> XReg {
    let mut xa = XRegAllocator::new();
    for _ in 0..n {
        xa.alloc_default(RegClass::GPR64);
    }
    xa.alloc_default(RegClass::GPR64)
}

fn make_config(num_gp: u32, num_fp: u32) -> RegAllocConfig {
    let mut cfg = RegAllocConfig::new(num_gp, num_fp, 4, Some(5));
    // 溢出编辑由分配器产出后，"scratch 池够不够"在**分配期**就判——测试给一个，
    // 好让"可改写字段上的 def spill"这类用例走到它本意的那条路（借用 scratch 写回）。
    cfg.scratch_regs = vec![PReg::new(15, RegClass::GPR64)];
    cfg
}

fn clobber_vcode() -> crate::VCode<DummyInst> {
    let mut vcode = crate::VCode::<DummyInst>::new();
    let bid = vcode.create_block(Block::new(0));
    vcode.switch_to_block(bid);
    for i in 0..3 {
        vcode.push_inst(DummyInst { id: i });
    }
    vcode
}

/// clobber 点占用者若同时是本指令 use：值保留在寄存器（不 spill）——
/// spill_vreg 不 store 当前寄存器值，reload 会读空槽垃圾。
#[test]
fn test_clobber_use_owner_not_spilled() {
    let mut config = make_config(16, 16);
    config
        .classes
        .get_mut(&RegClass::GPR64)
        .unwrap()
        .allocatable = vec![0, 1]; // RAX + RCX
    let x0 = xgpr(0);
    let vcode = clobber_vcode();
    let inst_ops = vec![
        smallvec::smallvec![OperandBind::def(x0, 0)], // inst0: def
        smallvec::smallvec![OperandBind::use_(x0, 0)], // inst1: use（clobber 点）
        smallvec::smallvec![OperandBind::use_(x0, 0)], // inst2: use 结束
    ];
    let clobber_map = vec![vec![], vec![(0u32, RegClass::GPR64)], vec![]]; // inst1 写死 RAX
    let alloc = BacktrackingAllocator::new();
    let result = alloc
        .allocate(&vcode, &config, &inst_ops, &clobber_map)
        .expect("alloc should succeed");
    // x0 在 clobber 点被使用（值需在寄存器）→ 不应被 spill
    assert!(
        !result.is_spilled(x0),
        "clobber 点 use 的占用者不应 spill（值保留在寄存器）"
    );
}

/// phys_conflicts 避开路径：唯一可分配寄存器在 clobber 点被写死 → 新分配失败。
#[test]
fn test_clobber_blocks_fresh_alloc() {
    let mut config = make_config(16, 16);
    config
        .classes
        .get_mut(&RegClass::GPR64)
        .unwrap()
        .allocatable = vec![0];
    let x0 = xgpr(0);
    let vcode = clobber_vcode();
    let inst_ops = vec![smallvec::smallvec![OperandBind::def(x0, 0)]]; // inst0: def
    let clobber_map = vec![vec![(0u32, RegClass::GPR64)]]; // inst0 写死 RAX → def 无法分配
    let alloc = BacktrackingAllocator::new();
    let result = alloc.allocate(&vcode, &config, &inst_ops, &clobber_map);
    // 2026-09 语义变更：def（写新值）在唯一寄存器被 clobber 时**spill 到栈**
    // 而非报错——emission 的 emit_inst_with_spills 用 scratch 写入 def、
    // 随后 store 回槽，后续 use 经 reload_from_stack 从槽 load，值正确。
    // 这只对 def 安全（写新值，不读旧寄存器）；use 场景仍保守报错。
    assert!(
        result.is_ok(),
        "def 在 clobber 时 spill 到栈（emission scratch 写入）"
    );
    assert!(
        result.as_ref().unwrap().is_spilled(x0),
        "def 应被 spill 到栈槽"
    );
}

/// 对照：无 clobber 时 1 个 XReg 配唯一寄存器 RAX 成功且不 spill。
/// **scratch 的号在任何类里都保留**（C4 根因的守卫，v23 M5）。
///
/// 溢出搬运用"号取自 scratch 池、类取值的类"的寄存器（x86: R10/R11 ⇒ 浮点 reload 实际
/// 写 XMM10/XMM11）。若某个类的池能把**活值**分到这些号上，reload 就会踩掉活值。
/// 这条用例把 scratch 号放进 GPR 池内，断言分配器**绝不**把它分给任何 vreg。
#[test]
fn scratch_reg_numbers_are_never_allocated() {
    const SCRATCH: u32 = 3;
    let mut config = RegAllocConfig::new(8, 8, 7, Some(6));
    config.scratch_regs = vec![PReg::new(SCRATCH, RegClass::GPR64)];
    let x0 = xgpr(0);
    let x1 = xgpr(1);
    let vcode = clobber_vcode();
    let inst_ops = vec![
        smallvec::smallvec![OperandBind::def(x0, 0), OperandBind::def(x1, 1)],
        smallvec::smallvec![OperandBind::use_(x0, 0), OperandBind::use_(x1, 1)],
        smallvec::smallvec![OperandBind::use_(x0, 0), OperandBind::use_(x1, 1)],
    ];
    let clobber_map = vec![vec![], vec![], vec![]];
    let alloc = BacktrackingAllocator::new();
    let result = alloc
        .allocate(&vcode, &config, &inst_ops, &clobber_map)
        .expect("两个 vreg 在 8 个寄存器的池里应当都有位置");
    for v in [x0, x1] {
        if let Some(p) = result.preg(v) {
            assert_ne!(
                p.num,
                SCRATCH,
                "scratch 号 {SCRATCH} 被分给了 v{}——浮点 reload 会踩掉这里的活值",
                v.index()
            );
        }
    }
}

#[test]
fn test_no_clobber_allocates_rax() {
    let mut config = make_config(16, 16);
    config
        .classes
        .get_mut(&RegClass::GPR64)
        .unwrap()
        .allocatable = vec![0];
    let x0 = xgpr(0);
    let vcode = clobber_vcode();
    let inst_ops = vec![
        smallvec::smallvec![OperandBind::def(x0, 0)],
        smallvec::smallvec![OperandBind::use_(x0, 0)],
        smallvec::smallvec![OperandBind::use_(x0, 0)],
    ];
    let clobber_map = vec![vec![], vec![], vec![]];
    let alloc = BacktrackingAllocator::new();
    let result = alloc
        .allocate(&vcode, &config, &inst_ops, &clobber_map)
        .expect("no clobber: RAX 可用，分配成功");
    assert!(
        !result.is_spilled(x0),
        "无 clobber 时 x0 应留在 RAX 不 spill"
    );
}

/// W1 正向守卫（WORKAROUNDS WA-40）：def-spill 在**可被 set_reg_field 改写**
/// 的字段上合法——emission 会把该字段重设 scratch 后 store 回槽，值仍正确。
/// 与 `test_clobber_blocks_fresh_alloc` 同形，但显式声明"可改写性"前提。
#[test]
fn test_def_spill_on_settable_field_ok() {
    let mut config = make_config(16, 16);
    config
        .classes
        .get_mut(&RegClass::GPR64)
        .unwrap()
        .allocatable = vec![0]; // 仅 RAX，且被 clobber → def 只能 spill
    let x0 = xgpr(0);
    let vcode = clobber_vcode();
    let inst_ops = vec![smallvec::smallvec![OperandBind::def(x0, 0)]];
    let clobber_map = vec![vec![(0u32, RegClass::GPR64)]];
    let alloc = BacktrackingAllocator::new();
    let result = alloc
        .allocate(&vcode, &config, &inst_ops, &clobber_map)
        .expect("def-spill 在可改写字段上必须成功（emission 用 scratch 写入 + store 回槽）");
    assert!(
        result.is_spilled(x0),
        "该构造下 def 应被 spill（守卫不得误报）"
    );
    // DummyInst 未覆写 → 默认字段可改写（旧契约）。
    assert!(
        DummyInst { id: 0 }.is_reg_field_settable(0),
        "默认 MachineInst 实现必须保持字段可改写（向后兼容）"
    );
}

/// W1 fail-closed（WORKAROUNDS WA-40）：def-spill 落在**固定物理字段**
/// （`is_reg_field_settable == false`，DSL 对这类字段不生成 set_reg_field
/// 分支 → 静默 no-op）时必须编译期报错——否则指令写物理寄存器、store-back
/// 从 scratch 读，spill 槽静默留下陈旧值（后续 reload 读垃圾 → 偶发 AV/挂起）。
#[test]
fn test_def_spill_on_fixed_field_errors() {
    #[derive(Clone, Debug, PartialEq, Eq, Hash)]
    struct FixedFieldInst {
        #[allow(dead_code)]
        id: u32,
    }
    impl MachineInst for FixedFieldInst {
        /// 手写替身：没有操作数表（v23 M4 起这是唯一必答的方法）。
        fn operands(&self) -> smallvec::SmallVec<[crate::machine::inst::InstOperand; 4]> {
            smallvec::SmallVec::new()
        }
        fn is_branch(&self) -> bool {
            false
        }
        fn branch_targets(&self) -> smallvec::SmallVec<[Block; 2]> {
            smallvec::SmallVec::new()
        }
        fn is_call(&self) -> bool {
            false
        }
        fn is_ret(&self) -> bool {
            false
        }
        /// 模拟"模板里写死的物理字段"：无字段可被改写。
        fn is_reg_field_settable(&self, _i: usize) -> bool {
            false
        }
    }

    let mut config = make_config(16, 16);
    config
        .classes
        .get_mut(&RegClass::GPR64)
        .unwrap()
        .allocatable = vec![0];
    let x0 = xgpr(0);
    let mut vcode = crate::VCode::<FixedFieldInst>::new();
    let bid = vcode.create_block(Block::new(0));
    vcode.switch_to_block(bid);
    vcode.push_inst(FixedFieldInst { id: 0 });
    let inst_ops = vec![smallvec::smallvec![OperandBind::def(x0, 0)]];
    let clobber_map = vec![vec![(0u32, RegClass::GPR64)]];
    let alloc = BacktrackingAllocator::new();
    let err = alloc
        .allocate(&vcode, &config, &inst_ops, &clobber_map)
        .expect_err("固定物理字段上的 def-spill 必须 fail-closed 报错");
    let msg = format!("{err}");
    assert!(
        msg.contains("WA-40") && msg.contains("fixed physical"),
        "错误信息应说明固定物理字段并指向 WA-40：{msg}"
    );
}
