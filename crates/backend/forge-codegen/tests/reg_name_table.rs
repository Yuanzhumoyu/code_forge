//! **`reg_name` 守卫**（v27 M2.5/S2 的前置条件）。
//!
//! S2（内联 asm 的寄存器约束）要在**汇编文本里**写物理寄存器名、再交给本 ISA 的汇编器。
//! 两件事都必须成立：
//!
//! 1. `TargetRegInfo::reg_name(reg)` **拿得到**名字；
//! 2. 汇编器**认得**这个名字（拿到的名字若拼不出可解析的指令，S2 会变成离现场很远的
//!    "解析失败"，甚至静默错）。
//!
//! 本守卫把两件都钉住：对 x86 的三个通用寄存器做"名字 → 汇编 → 解析成功"的往返。

use forge_codegen::arch::x86::{Reg, TargetMachine};
use forge_codegen::machine::target::TargetMachine as _;
use forge_ir::PhysReg;
use forge_isa_runtime::machine::reg_info::TargetRegInfo;

#[test]
fn reg_name_is_assembler_compatible() {
    let tm = TargetMachine::new();
    let asm = tm
        .assembler()
        .expect("x86 谱必须接汇编器（S2 依赖它把替换后的文本编成指令）");

    for (reg, expect) in [(Reg::RAX, "rax"), (Reg::RCX, "rcx"), (Reg::RDX, "rdx")] {
        let idx = PhysReg::to_index(reg);
        let name = tm
            .reg_info()
            .reg_name(idx, PhysReg::class(reg))
            .unwrap_or_else(|| panic!("x86 必须给出 {reg:?} 的汇编拼写（编号 {idx}）"));
        assert_eq!(
            name.to_ascii_lowercase(),
            expect,
            "拼写不符：{reg:?} → {name:?}（期望 {expect}）"
        );
        // 名字必须能被汇编器消费（用一条寄存器-寄存器指令做见证）
        let text = format!("add {name}, {name}");
        let insts = asm
            .parse_insts(&text)
            .unwrap_or_else(|e| panic!("汇编器不认 {text:?}（reg_name 给出 {name:?}）：{e:?}"));
        assert_eq!(insts.len(), 1, "{text:?} 应编成恰好一条指令");
    }
}
