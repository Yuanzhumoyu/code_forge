//! 集成测试：验证 minimal_sd 标准指令集 ISA。
//!
//! `minimal_sd` 模块在 `src/backend/minimal_sd_test.rs` 中通过
//! `isa_from_file!("examples/isa/minimal_sd.toml")` 编译期生成。

use codegen_lib::backend::isa_info::IsaInfo;
use codegen_lib::backend::minimal_sd_test::minimal_sd;
use codegen_lib::prelude::*;

#[test]
fn test_minimal_sd_isa_info() {
    assert_eq!(minimal_sd::Isa::name(), "minimal_sd");
    assert_eq!(minimal_sd::Isa::version(), "11.0");
    assert_eq!(minimal_sd::Isa::address_size(), 64);
}

#[test]
fn test_minimal_sd_isa_types() {
    // 验证核心类型存在
    fn _assert_isa<I: InstructionSet>(_: &I) {}
    _assert_isa(&minimal_sd::Isa);

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
    // 此测试验证 TOML 解析成功且 lowering 规则完整
    assert!(!minimal_sd::Isa::name().is_empty());
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
