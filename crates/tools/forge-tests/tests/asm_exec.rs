//! **真实汇编语料 · 执行档**（v20 V9）。
//!
//! 把 `asm/exec/<isa>/*.s` 的小程序（真语法汇编，各机 ABI 的叶子函数约定）
//! 用**生成物的汇编器**汇编成字节 → 合成 [`CompiledFunction`] → 交给本 crate 现有的
//! 执行器真跑（x86 原生 / riscv64 与 aarch64 走 QEMU system+semihosting），断言返回值。
//!
//! 缺执行通道时 **Skip 并说明原因**（x86 只在 windows-x86_64 宿主、riscv64/aarch64 需要
//! `qemu-system-*`；见 `asm::exec::unavailable_reason`），不假绿。
//!
//! [`CompiledFunction`]: code_forge::backend::CompiledFunction

use forge_tests::asm::{self, exec};

/// 三架构 × 全部小程序的"汇编 → 执行 → 断言返回值"。
#[test]
fn assembled_programs_execute_and_return_expected_values() {
    let mut ran = 0usize;
    let mut skipped = Vec::new();
    for isa in ["x86", "riscv64", "aarch64"] {
        let reason = exec::unavailable_reason(isa);
        let programs =
            exec::load_programs(isa).unwrap_or_else(|e| panic!("读 {isa} 小程序失败：{e}"));
        if programs.is_empty() {
            skipped.push(format!("{isa}: asm/exec/{isa}/ 没有小程序"));
            continue;
        }
        // 先不管能不能跑：汇编必须先成功（这一半不依赖任何模拟器）。
        let mut compiled = Vec::new();
        for p in &programs {
            let cf = exec::compile_program(isa, p)
                .unwrap_or_else(|e| panic!("{isa}/{}: 汇编失败：{e}\n源码：\n{}", p.name, p.src));
            assert!(!cf.code.is_empty(), "{isa}/{}: 汇编出的机器码为空", p.name);
            compiled.push(cf);
        }
        if let Some(r) = reason {
            skipped.push(format!(
                "{isa}: {r}（已汇编 {} 条，未执行）",
                compiled.len()
            ));
            continue;
        }
        let arch = exec::run_arch_for(isa).expect("三架构都有执行通道");
        for (p, cf) in programs.iter().zip(compiled.iter()) {
            let got = forge_tests::exec::executor::run(arch, cf, &p.args);
            assert_eq!(
                got, p.ret,
                "{isa}/{}: 期望 {} 得到 {got}\n源码：\n{}",
                p.name, p.ret, p.src
            );
            ran += 1;
        }
    }
    for s in &skipped {
        println!("ASM-EXEC-SKIP {s}");
        asm::report::emit_event("ASM-EXEC-SKIP", s);
    }
    println!("ASM-EXEC-SUMMARY ran={ran} skipped={}", skipped.len());
    asm::report::emit_event(
        "ASM-EXEC-SUMMARY",
        &format!("ran={ran} skipped={}", skipped.len()),
    );
    assert!(ran > 0, "一条小程序都没真跑成（跳过原因见上）");
}
