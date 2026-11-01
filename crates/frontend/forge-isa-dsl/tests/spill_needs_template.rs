//! **溢出必须有指令可发**：没有 `[machine.spill.<名>]` 时不得静默通过。
//!
//! `[BUG-block-param-copy]` 的根因：分配器决定溢出（栈帧里有溢出槽）后，
//! 生成物在"本类没有溢出模板"时发出的是 `let _ = (__dst, __off);`——**一条 ld/st
//! 都没有**。于是槽里永远是垃圾，被 spill 的值在 use 处读到别人的值（复现形状：
//! `add r9,r6,r5` 定义 t 后被常量的 `addi r9,zr,1` 覆盖，`mov r3,r9` 读到 1）。
//!
//! 本测试钉住修复：同一份谱在没有溢出模板时，生成物必须带**能力缺口**错误
//! （`SPEC_FRAME_OP_MISSING`），删掉命中的是静默 no-op 的习惯。

use std::path::Path;

use forge_isa_dsl::expand_str;

/// 最小谱：有帧/有 ret/有 frame_alloc|frame_free/有算术 lowering，但**没有**
/// `[machine.spill.*]`——正是报告里那份谱的形状（只有寄存器运算，没有访存指令）。
const NO_SPILL_TEMPLATE: &str = r#"
[meta]
name = "nospill"
endian = "little"
mode = 32

[encoding]
kind = "fixed"
bits = 32

[reg.gpr32]
names = ["r0", "r1", "r2", "r3", "r4", "r5", "r6", "r7"]

[operand.g]
kind = "reg"
class = "gpr32"

[operand.u16]
kind = "imm"
bits = 16
signed = false

[form.OP8]
fields = ["u8[31:24]:opcode"]

[form.RR]
fields = ["u8[31:24]:opcode", "u4[23:20]:a", "u4[19:16]:b"]

[form.RI]
fields = ["u8[31:24]:opcode", "u4[23:20]:a", "u4[19:16]:b", "u16[15:0]:imm"]

[inst.RET]
form = "OP8"
opcode = 0x01
roles = ["ret"]
effect = ["Ret"]
asm = "ret"

[inst.SUBI]
form = "RI"
opcode = 0x02
roles = ["frame_alloc"]
asm = "subi {a:g:out}, {b:g}, {imm:u16}"

[inst.ADDI]
form = "RI"
opcode = 0x03
roles = ["frame_free"]
asm = "addi {a:g:out}, {b:g}, {imm:u16}"

[inst.MOV]
form = "RR"
opcode = 0x04
asm = "mov {a:g:out}, {b:g}"

[inst.ADD]
form = "RR"
opcode = 0x05
asm = "add {a:g:out}, {b:g}"

[machine]
gpr = "gpr32"
value_gpr = "gpr32"
reg_move = "MOV"

[machine.stack]
slot = 32
align = 32
fp_save = 32

[machine.frame]
sp = "r7"
fp = "r6"

[[lower.Iadd]]
insts = ["ADD {out}, {0}"]

[[lower.Copy]]
insts = ["MOV {out}, {0}"]
"#;

fn expanded() -> String {
    let ts = expand_str(NO_SPILL_TEMPLATE, "nospill", Path::new("nospill.toml"))
        .expect("谱本身应能解析/生成（缺溢出模板不是**生成期**错误——只有真的溢出才失败）");
    // `TokenStream::to_string` 会在符号间插空格，比较前先归一化。
    ts.to_string()
        .chars()
        .filter(|c| !c.is_whitespace())
        .collect()
}

#[test]
fn spill_without_template_is_a_capability_error_not_a_noop() {
    let s = expanded();
    assert!(
        s.contains("SPEC_FRAME_OP_MISSING"),
        "缺 `[machine.spill.<名>]` 时必须生成能力缺口错误（fail-closed）——\
         否则分配器一旦溢出，槽就没人读写，use 读到垃圾（静默错值）"
    );
    assert!(
        !s.contains("let_=(__dst,__off)"),
        "不得再生成静默 no-op：`let _ = (__dst, __off);` 是 BUG-block-param-copy 的根因"
    );
    assert!(
        s.contains("emit_spill_load") && s.contains("emit_spill_store"),
        "能力缺口必须同时覆盖 load 与 store 两侧"
    );
}
