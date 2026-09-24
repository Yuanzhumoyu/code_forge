#![cfg(test)]

//! v19 V3d：**谱内派生枚举器的全指令编解码往返**守卫。
//!
//! 生成物里 `__spec_tests::all_insts()` 由**谱派生**（每条指令 × 宽度视图 + 立即数边界 +
//! 内存风味，取值规则与生成期自测同源），因此本文件是"宿主侧全指令往返"的**唯一**实现——
//! 历史上 x86 手抄过一份 650 行的 `all_insts()`，谱加指令时它不会自动跟上（漏测）。
//!
//! 判据两条：
//!
//! 1. **覆盖清点**：`SPEC_INSTS` 里的每条指令都要在枚举器里出现（派生逻辑漏了哪条就报哪条）；
//! 2. **闭环**：`encode(x) → decode → encode` 字节稳定，且 `decode` 必须吃满整条字节。
//!
//! 不做的事：不求 `decode(encode(x)) == x`（别名撞车时按声明序首匹配，本就不成立——
//! 那条断言仍由各 ISA 测试里的"规范指令"小清单守着）。

/// 单个 ISA 的往返检查（宏：三个 ISA 的 `Inst`/`encode`/`decode` 是各自生成模块的类型）。
macro_rules! roundtrip_of {
    ($isa:literal, $m:path) => {{
        use $m as isa;
        use isa::{decode, encode};
        let all = isa::__spec_tests::all_insts();
        let names = isa::__spec_tests::SPEC_INSTS;
        assert!(!all.is_empty(), "{}: 派生枚举器为空", $isa);
        for name in names {
            let name: &str = name;
            let hit = all.iter().any(|(label, _)| {
                *label == name || label.strip_prefix(name).is_some_and(|r| r.starts_with('['))
            });
            assert!(hit, "{}: 指令 '{name}' 不在派生枚举器里", $isa);
        }
        let mut checked = 0usize;
        for (label, inst) in &all {
            let b = encode(inst)
                .unwrap_or_else(|e| panic!("{} 编码 {label}（{inst:?}）失败：{e}", $isa));
            let (dec, n) =
                decode(&b).unwrap_or_else(|| panic!("{} 解码 {label}（{b:02x?}）失败", $isa));
            assert_eq!(
                n,
                b.len(),
                "{} {label}: 解码消费 {n} != {} 字节",
                $isa,
                b.len()
            );
            let b2 = encode(&dec).unwrap_or_else(|e| panic!("{} 重编码 {label} 失败：{e}", $isa));
            assert_eq!(b2, b, "{} {label}: decode→encode 往返字节不一致", $isa);
            checked += 1;
        }
        checked
    }};
}

/// 三份发行谱：谱里每条指令（含宽度视图/立即数边界/内存风味）编解码闭环。
///
/// 下界钉住的是**枚举器不许退化**：指令总数见 `spec_coverage_guard`（x86 197 /
/// riscv64 116 / arm64 104），枚举器条目只会更多（视图与风味），少于总数就是派生漏了。
#[test]
fn derived_insts_roundtrip_byte_stable() {
    let x86 = roundtrip_of!("x86_v12", crate::arch::x86_v12::x86_v12);
    let riscv = roundtrip_of!("riscv64_v12", crate::arch::riscv64_v12::riscv64_v12);
    let arm64 = roundtrip_of!("arm64_v12", crate::arch::arm64_v12::arm64_v12);
    assert!(x86 >= 197, "x86 枚举器条目 {x86} < 指令总数 197");
    assert!(riscv >= 116, "riscv64 枚举器条目 {riscv} < 指令总数 116");
    assert!(arm64 >= 104, "arm64 枚举器条目 {arm64} < 指令总数 104");
    // 实测条目数（2026-09-24）：x86 **602** / riscv64 **327** / arm64 **332**——
    // 远多于指令总数（197/116/104），因为每条指令还带宽度视图、立即数边界与内存风味。
    // 数字变了 ⇒ 谱的指令/操作数风味变了（或派生逻辑改了），人工复核后同步本行。
    assert_eq!(
        (x86, riscv, arm64),
        (602, 327, 332),
        "枚举器条目数变了：确认是谱的预期变更还是派生逻辑退化"
    );
}
