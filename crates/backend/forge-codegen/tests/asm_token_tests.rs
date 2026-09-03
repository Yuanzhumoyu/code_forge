//! 生成汇编器 token 化行为测试（x86_v12）：
//! 空白容错、立即数进制、范围校验、多类型槽约束过滤（gprx 拒绝 8 位）。
//!
//! 这些是 v13 汇编器重写的**新行为**，与旧字面段扫描对比：
//! - 旧：`mov  RAX , RBX`（多余空格）解析失败；新：token 化 → 成功。
//! - 旧：`0b` 二进制立即数不支持；新：支持。
//! - 旧：立即数越界静默截断；新：按槽范围报错。
//! - 旧：`gprx` 接受任意 GPR 名（含 8 位 → 8B 误编码）；新：显式 classes
//!   {2,4,8} → 8 位拒绝。

use forge_codegen::x86_v12::assemble;

#[test]
fn whitespace_insensitive() {
    let a = assemble("mov RAX, RBX").unwrap();
    let b = assemble("  mov   RAX ,  RBX  ").unwrap();
    let c = assemble("mov rax,rbx").unwrap();
    assert_eq!(a, b);
    assert_eq!(a, c);
    // 大小写不敏感（case_insensitive_regs + mnemonic_case 缺省 insensitive）
    assert_eq!(assemble("MOV RAX, RBX").unwrap(), a);
}

#[test]
fn immediate_radixes() {
    // 0b / 0x / 十进制等价
    assert_eq!(
        assemble("add RAX, 0b101010").unwrap(),
        assemble("add RAX, 42").unwrap()
    );
    assert_eq!(
        assemble("add RAX, 0x2a").unwrap(),
        assemble("add RAX, 42").unwrap()
    );
    // 负十六进制
    assert_eq!(
        assemble("add RAX, -0x10").unwrap(),
        assemble("add RAX, -16").unwrap()
    );
}

#[test]
fn immediate_range_x86() {
    // imm32 signed：[-2^31, 2^31-1]
    assert!(assemble("add RAX, 2147483647").is_ok());
    assert!(assemble("add RAX, -2147483648").is_ok());
    assert!(assemble("add RAX, 2147483648").is_err());
    assert!(assemble("add RAX, -2147483649").is_err());
}

#[test]
fn gprx_rejects_8bit() {
    // gprx classes = {2,4,8}：8 位走独立 8A form（MOV_R8_RM8），gprx 不误吞
    //（防 8B 误编码）。`mov AL, BL` 现由 8 位 form 处理，见 byte_mov_width_dispatch。
    assert!(assemble("mov32 AL, BL").is_err()); // mov32（FIX32 gprx）拒绝 8 位
}

#[test]
fn mov_width_dispatch() {
    // v13：movrr/mov64rr 合并为 `mov`——按操作数实际宽度自动分发
    assert!(assemble("mov RAX, RBX").is_ok()); // 64 位 → MOV_RM_R（89）
    assert!(assemble("mov EAX, EBX").is_ok()); // 32 位 → MOV_R_RM（8B）
    assert!(assemble("mov AX, BX").is_ok()); // 16 位 → 66 8B
    // 混宽拒绝（opsize s0 一致性；不再静默按 op0 编码）
    assert!(assemble("mov RAX, EBX").is_err());
    assert!(assemble("mov EAX, AX").is_err());
}

#[test]
fn byte_mov_width_dispatch() {
    use forge_codegen::x86_v12::{Inst, encode};
    // 8 位 mov（opcode 8A）→ MOV_R8_RM8；mov 全宽度（8/16/32/64）齐备
    let inst = assemble("mov AL, BL").unwrap();
    assert!(matches!(inst, Inst::MovR8Rm8 { .. }));
    assert_eq!(encode(&inst).unwrap(), vec![0x8A, 0xC3]);
    // 8 位与 16/32/64 位混宽 → 拒绝（无匹配 form）
    assert!(assemble("mov AL, BX").is_err());
    assert!(assemble("mov AX, BL").is_err());
    // 8 位 REX 路径（spl/bpl/sil/dil）
    assert!(assemble("mov SIL, DIL").is_ok());
}

#[test]
fn mov_width_auto_dispatch() {
    // v14：mov32（FIX32 独立助记符）已删——32 位语义由 mov gprx auto（操作数
    // 宽度驱动）：mov EAX,EBX 合法（01 无 REX.W 形态）、mov RAX,RBX 合法
    // （REX.W）。混宽仍拒绝（多类槽 encode 宽度一致性）。
    assert!(assemble("mov EAX, EBX").is_ok());
    assert!(assemble("mov RAX, RBX").is_ok());
    assert!(assemble("mov AX, BX").is_ok());
    assert!(assemble("mov RAX, EBX").is_err());
    assert!(assemble("mov32 RAX, RBX").is_err()); // mov32 助记符已不存在
}

#[test]
fn arithmetic_width_dispatch() {
    // v14：add/sub/xor/... 数据槽 gprx 多类——同变体按操作数实际宽度自动
    // 分发（add EAX,EBX → 01 D8 无 REX.W；add RAX,RBX → 48 01 D8）。固定
    // _32 变体（AddRmR32 等）已删（WA-35 DSL 回填修复）。
    use forge_codegen::x86_v12::{Inst, encode};
    // add eax, ebx（32 位）→ ADD_RM_R：01 D8（Intel 语义 dst=eax，无 REX.W）
    let inst = assemble("add EAX, EBX").unwrap();
    assert!(matches!(inst, Inst::AddRmR { .. }));
    assert_eq!(encode(&inst).unwrap(), vec![0x01, 0xD8]);
    // add rax, rbx（64 位）→ ADD_RM_R：48 01 D8（REX.W 由操作数宽度）
    let inst = assemble("add RAX, RBX").unwrap();
    assert!(matches!(inst, Inst::AddRmR { .. }));
    assert_eq!(encode(&inst).unwrap(), vec![0x48, 0x01, 0xD8]);
    // 混宽拒绝
    assert!(assemble("add RAX, EBX").is_err());
    // xor 同理（同一变体，宽度 auto）
    assert!(matches!(
        assemble("xor EAX, EBX").unwrap(),
        Inst::XorRmR { .. }
    ));
    assert!(matches!(
        assemble("xor RAX, RBX").unwrap(),
        Inst::XorRmR { .. }
    ));
}

#[test]
fn opsize_slot_width_consistency() {
    // mov（opsize s0，宽度由操作数推导）：混宽拒绝（不再静默按 op0 编码）
    assert!(assemble("mov RAX, EBX").is_err());
    assert!(assemble("mov AX, BX").is_ok());
    assert!(assemble("mov EAX, EBX").is_ok());
    assert!(assemble("mov RAX, RBX").is_ok());
}

#[test]
fn opsize_dest_slot_and_max_width_dispatch() {
    // v14 修正：两地址 RM_R 族 opsize = s1（inout 目的槽 = IR 结果宽度）、
    // CMP/TEST（无目的槽）opsize = "max"（取宽者）。两者都不改文本汇编语义
    // ——汇编路径仍要求全 GPR 操作数同宽，且同宽时编码与 s0 逐字节一致。
    use forge_codegen::x86_v12::{Inst, encode};
    // s1：sub/and/or 同 add，32 位无 REX.W、64 位 REX.W
    assert_eq!(
        encode(&assemble("sub EAX, EBX").unwrap()).unwrap(),
        vec![0x29, 0xD8]
    );
    assert_eq!(
        encode(&assemble("sub RAX, RBX").unwrap()).unwrap(),
        vec![0x48, 0x29, 0xD8]
    );
    assert!(assemble("sub RAX, EBX").is_err());
    assert!(assemble("and RAX, EBX").is_err());
    assert!(assemble("or RAX, EBX").is_err());
    assert!(assemble("xor RAX, EBX").is_err());
    // max：cmp/test 同宽仍按该宽度编码，混宽文本仍拒绝（max 只放宽 IR 降级）
    let inst = assemble("cmp EAX, EBX").unwrap();
    assert!(matches!(inst, Inst::CmpRmR { .. }));
    assert_eq!(encode(&inst).unwrap(), vec![0x39, 0xD8]);
    assert_eq!(
        encode(&assemble("cmp RAX, RBX").unwrap()).unwrap(),
        vec![0x48, 0x39, 0xD8]
    );
    assert!(assemble("cmp RAX, EBX").is_err());
    assert_eq!(
        encode(&assemble("test EAX, EBX").unwrap()).unwrap(),
        vec![0x85, 0xD8]
    );
    assert_eq!(
        encode(&assemble("test RAX, RBX").unwrap()).unwrap(),
        vec![0x48, 0x85, 0xD8]
    );
    assert!(assemble("test RAX, EBX").is_err());
}

#[test]
fn mem_bracket_whitespace_immune() {
    // Mem 槽 `[base±disp]` token 化：括号内空白免疫
    let a = assemble("mov64rm RAX, [RBX+8]").unwrap();
    let b = assemble("mov64rm RAX, [ RBX + 8 ]").unwrap();
    assert_eq!(a, b);
    let c = assemble("mov64rm RAX, [RBX]").unwrap();
    let d = assemble("mov64rm RAX, [ RBX ]").unwrap();
    assert_eq!(c, d);
}
