//! 生成期自测（v18 S6）在**夹具谱**上的宿主：
//!
//! `tests/common/mod.rs` 里的夹具关掉了 `spec_tests`（同一份谱会被多个测试二进制
//! 反复展开，生成的自测会重复跑）；这里把三个"极端形状"的夹具重新宿主并打开：
//!
//! - `demo8`：1 字节寄存器 + 32 位字（值池/类派生极端）；
//! - `demo_inst12`：12 位指令字（非 8 倍数，末字节填充位必须为 0）；
//! - `demo_mixed16_32`：混合字长（16/32 位共存，解码按宽度分组）。
//!
//! 生成的自测直接住在生成的模块里（`<夹具>::__spec_tests`），随 `cargo test` 运行；
//! 这里只额外核对**覆盖率常量**确实被导出（生成端与宿主端的接口守卫）。

forge_dsl::isa_from_file!("tests/isa/demo8.toml", spec_tests = true);
forge_dsl::isa_from_file!("tests/isa/demo_inst12.toml", spec_tests = true);
forge_dsl::isa_from_file!("tests/isa/demo_mixed16_32.toml", spec_tests = true);
// v21 W11：第四份 ISA（24 位定宽，非 x86/riscv64/arm64）——它的谱内 `[[vectors]]`
// 与每条指令的闭环自测都在这里真跑（"新 ISA 只要 TOML + 宿主"的证据链一环）。
forge_dsl::isa_from_file!("tests/isa/demo_v21.toml", spec_tests = true);
// v21 W11：变长一态（`prefix_scan` + **stream 段**）——1 字节 opcode 后跟不同宽度的立即数
// ⇒ 指令长 2 或 3 字节。它逼出并守住一处生成器缺口（空前缀表时的孤儿 `else`）。
forge_dsl::isa_from_file!("tests/isa/demo_vlen.toml", spec_tests = true);

/// 生成的自测模块导出了覆盖率常量，且夹具谱是**零跳过**的。
#[test]
fn spec_tests_export_coverage_constants() {
    use demo_inst12::__spec_tests as d12;
    use demo_mixed16_32::__spec_tests as dm;
    use demo8::__spec_tests as d8;

    // 夹具必须有指令（生成期常量 ⇒ 用 const 块保持编译期断言，且不被 lint 判为恒真）。
    const { assert!(d8::SPEC_TOTAL > 0) };
    assert_eq!(d8::SPEC_COVERED, d8::SPEC_TOTAL, "demo8 应全指令覆盖");
    assert!(d8::SPEC_SKIPPED.is_empty(), "{:?}", d8::SPEC_SKIPPED);

    assert_eq!(
        d12::SPEC_COVERED,
        d12::SPEC_TOTAL,
        "demo_inst12 应全指令覆盖"
    );
    assert!(d12::SPEC_SKIPPED.is_empty(), "{:?}", d12::SPEC_SKIPPED);

    assert_eq!(
        dm::SPEC_COVERED,
        dm::SPEC_TOTAL,
        "demo_mixed16_32 应全指令覆盖"
    );
    assert!(dm::SPEC_SKIPPED.is_empty(), "{:?}", dm::SPEC_SKIPPED);

    // v21 W11：第四份 ISA 也是**零跳过**（全部指令都有闭环自测）。
    use demo_v21::__spec_tests as dv;
    const { assert!(dv::SPEC_TOTAL > 0) };
    assert_eq!(dv::SPEC_COVERED, dv::SPEC_TOTAL, "demo_v21 应全指令覆盖");
    assert!(dv::SPEC_SKIPPED.is_empty(), "{:?}", dv::SPEC_SKIPPED);
}
