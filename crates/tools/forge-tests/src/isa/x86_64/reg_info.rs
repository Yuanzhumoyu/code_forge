//! 多宽度寄存器类测试 — [reg_classes.*] 暴露 + 分配配置。
//!
//! 验证 RegClass 宽度化后，reg_info 层暴露全部寄存器类
//! （GPR32 → RegClass::GPR(4)），且 reg_class_width 按类返回宽度，
//! 分配器可据此为 I32 等类型分派到对应宽度的寄存器池。

#![allow(dead_code)] // ri() 辅助仅在 #[cfg(test)] 下使用

use code_forge::backend::machine::reg_info::TargetRegInfo;
use code_forge::backend::machine::target::TargetMachine;
#[cfg(test)]
use code_forge::ir::RegClass;

fn ri() -> &'static dyn TargetRegInfo<Reg = code_forge::backend::x86_64::Reg> {
    // TargetMachine::new() 返回具体类型；通过静态泄漏模拟 'static 借用
    let machine = Box::leak(Box::new(code_forge::backend::x86_64::TargetMachine::new()));
    machine.reg_info().as_ref()
}

#[test]
fn test_register_classes_expose_gpr32() {
    let classes = ri().register_classes();
    // 多宽度类 GPR32 存在且映射到 GPR(4)
    let gpr32 = classes
        .iter()
        .find(|c| c.name == "GPR32")
        .expect("register_classes 应包含 GPR32（来自 [reg_classes.GPR32]）");
    assert_eq!(gpr32.reg_class, RegClass::GPR(4));
    assert_eq!(gpr32.width, 4);
    assert!(!gpr32.allocatable.is_empty());
}

#[test]
fn test_reg_class_width_multi_width() {
    let info = ri();
    assert_eq!(info.reg_class_width(RegClass::GPR64), 8);
    assert_eq!(info.reg_class_width(RegClass::GPR(4)), 4);
    assert_eq!(info.reg_class_width(RegClass::GPR(2)), 2);
    assert_eq!(info.reg_class_width(RegClass::FPR64), 8);
    // 未定义类回退到 payload 宽度
    assert_eq!(info.reg_class_width(RegClass::VEC(16)), 16);
}

#[test]
fn test_alloc_config_all_classes_nonempty() {
    // run_regalloc 从 register_classes 构建全部类的 ClassConfig——
    // 每类必须有可分配寄存器，否则分配器会因空池失败。
    let classes = ri().register_classes();
    assert!(!classes.is_empty(), "register_classes 不应为空");
    for info in classes {
        assert!(
            !info.allocatable.is_empty(),
            "寄存器类 {} 无可分配寄存器",
            info.name
        );
    }
}
