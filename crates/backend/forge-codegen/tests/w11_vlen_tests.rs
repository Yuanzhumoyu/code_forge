//! v21 **W11 第二片**：**变长一态**的第四方 ISA（`isa/demo_vlen.toml`）——`prefix_scan` +
//! **stream 段**（v21 W4 的面）：1 字节 opcode 后跟不同宽度的立即数 ⇒ 指令长 **2 或 3 字节**。
//!
//! 与 `demo_v21`（24 位定宽 + 就地 `fields`）配对，合起来就是执行清单 W11 的
//! "**定宽 + 一态变长各一**"。接入面同样只有：TOML + 两行宿主 + 本文件 ⇒ **零编译器改动**
//! （编译器侧只**修 bug**：空前缀表时 `gen_prefix_scan_loop` 会发孤儿 `else`，生成物不合法）。
//!
//! 编码算术：
//! - `opcode_reg` 段：opcode 字节 = **基值 | 目的寄存器号**（基值低 3 位必须为 0，否则两条
//!   指令会在模 8 上撞车 —— 本文件就是被这条咬过一次，见 `opcode_reg_*` 的注释）；
//! - `imm` 段：`value = 8` / `value = 16` 决定跟几个字节。
//!
//! 本文件只断言**结构事实**（长度可变、opcode 在首字节、寄存器号在低位、往返闭合、截断不解），
//! 不手抄字节表（精确字节交给谱内 `[[vectors]]` 与生成期自测）。

mod common;

use common::demo_vlen::{assemble, decode, disassemble, encode};

fn enc(asm: &str) -> Vec<u8> {
    let inst = assemble(asm).unwrap_or_else(|e| panic!("assemble `{asm}`: {e}"));
    encode(&inst).unwrap_or_else(|e| panic!("encode `{asm}`: {e}"))
}

/// 变长的来源：同一条 opcode 后面跟 8 位或 16 位立即数 ⇒ 2 字节 vs 3 字节。
#[test]
fn instruction_length_varies_with_the_immediate_width() {
    assert_eq!(enc("add R1, R2").len(), 2, "RR = opcode + modrm");
    assert_eq!(enc("sub R3, R4").len(), 2, "RR = opcode + modrm");
    assert_eq!(enc("addi8 R1, 5").len(), 2, "ADDI8 = opcode_reg + imm8");
    assert_eq!(enc("addi16 R2, 300").len(), 3, "ADDI16 = opcode_reg + imm16");
}

/// 段序 = 发射序：`opcode` 段恒在首字节。
#[test]
fn opcode_is_the_first_byte() {
    assert_eq!(enc("add R1, R2")[0], 0x01);
    assert_eq!(enc("sub R3, R4")[0], 0x02);
    assert_eq!(enc("addi8 R1, 5")[0], 0x10 | 1);
    assert_eq!(enc("addi16 R2, 300")[0], 0x18 | 2);
}

/// `opcode_reg` 段：opcode 字节 = 基值 | 寄存器号。
///
/// **基值低 3 位必须为 0**：基值 `0x11` 与 `0x12` 会被寄存器号覆盖成同一个模 8 类
/// （`0x11|2 == 0x12|1`），两条指令就此撞车、解码恒选先声明的那条 —— 本夹具第一版
/// 正是这么写的，症状是"3 字节指令解码只吃 2 字节"（被误当成生成器缺口）。
#[test]
fn opcode_reg_encodes_the_destination_register() {
    for (mnemonic, base, r) in [
        ("addi8", 0x10u8, 1u8),
        ("addi8", 0x10, 2),
        ("addi16", 0x18, 5),
        ("addi16", 0x18, 7),
    ] {
        let b = enc(&format!("{mnemonic} R{r}, 1"));
        assert_eq!(b[0], base | r, "opcode 字节 = 基值 {base:#04x} | 寄存器号 {r}");
    }
}

/// 编解码往返 + 反汇编文本再编回：**两种长度都要吃满**。
#[test]
fn round_trip_for_both_lengths() {
    for asm in ["add R1, R2", "sub R3, R4", "addi8 R1, 5", "addi16 R2, 300"] {
        let bytes = enc(asm);
        let (inst, used) = decode(&bytes).unwrap_or_else(|| panic!("decode `{asm}` 失败"));
        assert_eq!(used, bytes.len(), "`{asm}` 应吃满 {} 字节", bytes.len());
        assert_eq!(encode(&inst).unwrap(), bytes, "decode→encode 字节不变（`{asm}`）");
        let text = disassemble(&inst);
        assert_eq!(enc(&text), bytes, "反汇编文本应能再编回：`{text}`");
    }
}

/// 长度不足 ⇒ 解不出（变长解码不能把短输入当成完整指令）。
#[test]
fn truncated_input_decodes_to_nothing() {
    let long = enc("addi16 R2, 300");
    assert!(decode(&long[..1]).is_none(), "只有 opcode 字节不算一条指令");
    assert!(decode(&[]).is_none(), "空缓冲");
}
