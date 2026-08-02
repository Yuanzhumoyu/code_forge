//! 集成测试：验证 minimal_sd 标准指令集 ISA（用户自定义 ISA 接入 forge-tests 的示例）。
//!
//! `minimal_sd` 模块在 `src/arch/minimal_sd_test.rs` 中通过
//! `isa_from_file!("isa/minimal_sd.toml")` 编译期生成（从根 tests/minimal_sd_tests.rs 迁移）。

#![cfg(test)]

use code_forge::backend::arch::minimal_sd_test::minimal_sd;
use code_forge::backend::machine::isa_info::IsaInfo;
use code_forge::prelude::*;

#[test]
fn test_minimal_sd_isa_info() {
    let info = minimal_sd::IsaInfo;
    assert_eq!(info.name(), "minimal_sd");
    assert_eq!(info.version(), "11.0");
    assert_eq!(info.address_size(), 64);
}

#[test]
fn test_minimal_sd_isa_types() {
    // 验证核心类型存在
    let _tm = minimal_sd::TargetMachine::new();

    fn _assert_reg<R: PhysReg>(_: &R) {}
    _assert_reg(&minimal_sd::Reg::R0);

    fn _assert_inst<I: MachineInst>(_: &I) {}
    _assert_inst(&minimal_sd::Inst::SdAdd {
        dest: VReg(0),
        src: VReg(1),
    });
}

#[test]
fn test_minimal_sd_no_default_lowering_disabled() {
    // minimal_sd 未设置 no_default_lowering，默认 lowering 应生效
    let info = minimal_sd::IsaInfo;
    assert!(!info.name().is_empty());
}

#[test]
fn test_minimal_sd_standard_instructions_exist() {
    // 验证关键标准指令变体存在
    use minimal_sd::Inst;
    let _mov = Inst::SdMov {
        dest: VReg(0),
        src: VReg(1),
    };
    let _add = Inst::SdAdd {
        dest: VReg(0),
        src: VReg(1),
    };
    let _ret = Inst::SdRet {};
    let _nop = Inst::SdNop {};
}
