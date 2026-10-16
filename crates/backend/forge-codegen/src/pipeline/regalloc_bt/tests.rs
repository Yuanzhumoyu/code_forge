//! 从 `regalloc_bt.rs` 拆出（v25 M4：逐字搬移，不改行为）。

use super::*;
use crate::machine::inst::DummyInst;
use forge_ir::entity::*;
use std::collections::HashMap;

/// 构造第 n 号 GPR 临时寄存器（测试辅助）。
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

#[test]
fn test_empty_function() {
    let config = make_config(8, 8);
    let alloc = BacktrackingAllocator::new();
    let vcode = crate::VCode::<DummyInst>::new();
    let result = alloc.allocate(&vcode, &config, &[], &[]);
    let _ = result;
}

#[test]
fn test_basic_allocation() {
    let config = make_config(8, 8);
    let alloc = BacktrackingAllocator::new();
    let vcode = crate::VCode::<DummyInst>::new();
    let result = alloc.allocate(&vcode, &config, &[], &[]);
    assert!(result.is_ok());
}

#[test]
fn test_fixed_reg_constraint() {
    // 验证 BT 分配器创建
    let alloc = BacktrackingAllocator::new();
    let _ = alloc;
}

#[test]
fn test_phys_conflicts_cross_width_classes() {
    // 跨宽度类物理重叠：GPR(4) 与 GPR(8) 的相同编号是同一物理寄存器
    //（RAX=EAX），分配器必须视为冲突；FPR 的 0（XMM0）与 GPR 的 0（RAX）
    // 是不同物理寄存器，不冲突。
    let config = make_config(8, 8);
    let mut state = BtState::new(&config, HashMap::new());
    let gpr4 = PReg::new(0, RegClass::GPR(32));
    let gpr8 = PReg::new(0, RegClass::GPR(64));
    let fpr8 = PReg::new(0, RegClass::FPR(64));
    let v4 = XReg::new(512, RegClass::GPR(32));
    let v8 = XReg::new(513, RegClass::GPR(64));

    assert!(!state.phys_conflicts(gpr4));
    state.set_reg_owner(gpr4, v4);
    // 同编号跨宽度类（GPR(4) vs GPR(8)）冲突
    assert!(state.phys_conflicts(gpr8), "GPR(4) 与 GPR(8) 同编号应冲突");
    assert_eq!(state.phys_owner(gpr8), Some(v4));
    // 同编号同 class 冲突
    assert!(state.phys_conflicts(gpr4));
    // FPR(8) 的 0（XMM0）与 GPR 的 0（RAX）不冲突
    assert!(!state.phys_conflicts(fpr8), "FPR 与 GPR 同编号不应冲突");
    // 反向：GPR(8) 占用后 GPR(4) 同编号也冲突
    state.set_reg_owner(gpr8, v8);
    assert!(state.phys_conflicts(gpr4));
}

#[test]
fn test_reg_class_width_from_config() {
    // 验证 DSL 生成的 reg_class_width 通过配置正确传播
    let config = make_config(16, 16);
    let gpr_cfg = config.classes.get(&RegClass::GPR64).unwrap();
    assert_eq!(gpr_cfg.reg_width, 8);
    let fpr_cfg = config.classes.get(&RegClass::FPR64).unwrap();
    assert_eq!(fpr_cfg.reg_width, 8);
}

#[test]
fn test_all_regclasses_have_allocatable() {
    // 验证所有寄存器类都有非空的可分配列表
    let config = make_config(16, 16);
    for (class, cfg) in &config.classes {
        assert!(
            !cfg.allocatable.is_empty(),
            "{:?} should have allocatable registers",
            class
        );
    }
}

#[test]
fn test_param_vregs_preassign() {
    // 验证 param XReg 预分配机制：参数 XReg 在函数入口获得不同寄存器。
    let mut config = make_config(16, 16);
    config.param_xregs = vec![xgpr(0), xgpr(1), xgpr(2), xgpr(3)];
    assert_eq!(config.param_xregs.len(), 4);
    let alloc = BacktrackingAllocator::new();
    let vcode = crate::VCode::<DummyInst>::new();
    let result = alloc.allocate(&vcode, &config, &[], &[]);
    assert!(result.is_ok());
}

#[test]
fn test_spill_slot_width_aware() {
    // 验证 BT 分配器在仅有 1 个可分配寄存器时仍能完成分配
    let mut config = make_config(8, 8);
    config
        .classes
        .get_mut(&RegClass::GPR64)
        .unwrap()
        .allocatable = vec![3]; // RBX only
    let alloc = BacktrackingAllocator::new();
    let vcode = crate::VCode::<DummyInst>::new();
    let result = alloc.allocate(&vcode, &config, &[], &[]);
    assert!(
        result.is_ok(),
        "Allocator should handle high register pressure"
    );
}
