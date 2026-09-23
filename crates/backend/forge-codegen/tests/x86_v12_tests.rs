//! x86-64 v12 验证（迭代 3+：变长语义键）。v11 后端已删除，golden 全部为
//! 硬编码 x86 规范 oracle 字节（v12 组内寄存器索引，无 v11 的 `16+i` 偏差）。
//!
//! **规范字节已迁进谱内 `[[vectors]]`**（v19 V3b）：`isa/x86_v12.toml` 末尾 102 条，
//! 由生成物 `__spec_tests::spec_vector_*` 执行（`cargo test -p forge-codegen --lib`），
//! 也可单跑 `cargo run -p forge-isa -- test isa/x86_v12.toml`。本文件只留集成/往返/
//! ABI 类断言。
//!
//! - **字节级往返**：decode(encode(X)) 再 encode 字节一致（含编码撞车的别名
//!   指令，声明序首匹配）。
//! - **assemble/disassemble** 往返（opsize 操作数非文本，默认 64）。

use forge_codegen::x86_v12::{Inst, MemRef, Reg, assemble, decode, disassemble, encode};
use forge_ir::{PhysReg, RegClass};

// ─────────────────── 规范字节：GPR @modrm/@modrm_imm32/内存 ───────────────────

#[test]
fn opsize_prefix_and_rex_w() {
    // v12.1：opsize 由寄存器宽度视图推导——AX→0x66 前缀、EAX→无、RAX→REX.W。
    // from_index_grp 按组名构造视图（gpr16/gpr32/gpr64 共享物理编号 0-15）。
    let cases: &[(RegClass, u32, u32, &[u8])] = &[
        (RegClass::GPR(2), 0, 1, &[0x66, 0x8B, 0xC1]), // movrr ax, cx
        (RegClass::GPR(4), 0, 1, &[0x8B, 0xC1]),       // movrr eax, ecx
        (RegClass::GPR(8), 0, 1, &[0x48, 0x8B, 0xC1]), // movrr RAX, rcx
        (RegClass::GPR(8), 8, 9, &[0x4D, 0x8B, 0xC1]), // movrr R8, r9（REX.W+R+B）
        (RegClass::GPR(8), 1, 8, &[0x49, 0x8B, 0xC8]), // movrr rcx, r8（REX.W+B）
    ];
    for (gpr, dest, src, expected) in cases {
        let v12b = encode(&Inst::MovRRm {
            dst: Reg::from_index(*dest, *gpr),
            src: Reg::from_index(*src, *gpr),
        })
        .unwrap();
        assert_eq!(
            v12b.as_slice(),
            *expected,
            "grp: {gpr} dest: {dest} src: {src}"
        );
    }
}

// ─────────────────── `+r` 形式（push/pop/bswap/mov_imm64，迭代 5）───────────────────

// ─────────────────── 控制流（JMP/CALL/RET，迭代 6）───────────────────

#[test]
fn control_flow_roundtrip() {
    use Inst::*;
    let insts = vec![
        Ret,
        JmpRel32 { target: 0 },
        JmpRel32 { target: 42 },
        JmpRel32 { target: -1 },
        CallRipRel { target: 0 },
        CallRipRel { target: 0x1234 },
    ];
    for inst in insts {
        let b = encode(&inst).unwrap();
        let (dec, n) = decode(&b).unwrap();
        assert_eq!(n, b.len(), "{inst:?}: 解码消费 {n} != {} 字节", b.len());
        let b2 = encode(&dec).unwrap();
        assert_eq!(b2, b, "{inst:?}: decode→encode 往返字节不一致");
    }
}

// ─────────────────── SSE 规范字节（v11 的 16+i 索引不合规）───────────────────

// ─────────────────── 内存寻址规范字节（@modrm_mem，迭代 3b）───────────────────

// ─────────────────── 家族（SSE）与 VEX 规范字节（迭代 4）───────────────────

// ─────────────────── 字节级 decode 往返 ───────────────────

/// 全部指令的代表性 Inst 值（含别名——字节级往返不要求变体相等）。
/// 变长 form 的操作数字段名 = 位置名 op0/op1/op2（ModRM 语义由 form 键驱动）。
fn all_insts() -> Vec<Inst> {
    use Inst::*;
    vec![
        // @modrm（opsize 各值）
        MovRRm {
            dst: Reg::from_index(0, forge_ir::RegClass::GPR64),
            src: Reg::from_index(1, forge_ir::RegClass::GPR64),
        },
        MovRRm {
            dst: Reg::from_index(8, forge_ir::RegClass::GPR64),
            src: Reg::from_index(9, forge_ir::RegClass::GPR64),
        },
        MovRRm {
            dst: Reg::from_index(0, forge_ir::RegClass::GPR64),
            src: Reg::from_index(1, forge_ir::RegClass::GPR64),
        },
        MovRRm {
            dst: Reg::from_index(0, forge_ir::RegClass::GPR64),
            src: Reg::from_index(1, forge_ir::RegClass::GPR64),
        },
        MovRRm {
            dst: Reg::from_index(2, forge_ir::RegClass::GPR64),
            src: Reg::from_index(3, forge_ir::RegClass::GPR64),
        },
        MovsxdRRm {
            dst: Reg::from_index(0, forge_ir::RegClass::GPR64),
            src: Reg::from_index(1, forge_ir::RegClass::GPR64),
        },
        AddRmR {
            src: Reg::from_index(0, forge_ir::RegClass::GPR64),
            dst: Reg::from_index(1, forge_ir::RegClass::GPR64),
        },
        SubRmR {
            src: Reg::from_index(0, forge_ir::RegClass::GPR64),
            dst: Reg::from_index(1, forge_ir::RegClass::GPR64),
        },
        ImulRRm {
            dst: Reg::from_index(0, forge_ir::RegClass::GPR64),
            src: Reg::from_index(1, forge_ir::RegClass::GPR64),
        },
        XorRmR {
            src: Reg::from_index(0, forge_ir::RegClass::GPR64),
            dst: Reg::from_index(1, forge_ir::RegClass::GPR64),
        },
        AndRmR {
            src: Reg::from_index(0, forge_ir::RegClass::GPR64),
            dst: Reg::from_index(1, forge_ir::RegClass::GPR64),
        },
        OrRmR {
            src: Reg::from_index(0, forge_ir::RegClass::GPR64),
            dst: Reg::from_index(1, forge_ir::RegClass::GPR64),
        },
        CmpRmR {
            src: Reg::from_index(0, forge_ir::RegClass::GPR64),
            src2: Reg::from_index(1, forge_ir::RegClass::GPR64),
        },
        TestRmR {
            src: Reg::from_index(0, forge_ir::RegClass::GPR64),
            src2: Reg::from_index(1, forge_ir::RegClass::GPR64),
        },
        MovzxR8Rm {
            dst: Reg::from_index(0, forge_ir::RegClass::GPR64),
            src: Reg::from_index(1, forge_ir::RegClass::GPR(1)),
        },
        MovzxR16Rm {
            dst: Reg::from_index(0, forge_ir::RegClass::GPR64),
            src: Reg::from_index(1, forge_ir::RegClass::GPR(2)),
        },
        MovsxR8Rm {
            dst: Reg::from_index(0, forge_ir::RegClass::GPR64),
            src: Reg::from_index(1, forge_ir::RegClass::GPR(1)),
        },
        MovsxR16Rm {
            dst: Reg::from_index(0, forge_ir::RegClass::GPR64),
            src: Reg::from_index(1, forge_ir::RegClass::GPR(2)),
        },
        CmpxchgRmR {
            src: Reg::from_index(0, forge_ir::RegClass::GPR64),
            dst: Reg::from_index(1, forge_ir::RegClass::GPR64),
        },
        XaddRmR {
            src: Reg::from_index(0, forge_ir::RegClass::GPR64),
            dst: Reg::from_index(1, forge_ir::RegClass::GPR64),
        },
        BtRmR {
            src: Reg::from_index(0, forge_ir::RegClass::GPR64),
            src2: Reg::from_index(1, forge_ir::RegClass::GPR64),
        },
        BtsRmR {
            src: Reg::from_index(0, forge_ir::RegClass::GPR64),
            src2: Reg::from_index(1, forge_ir::RegClass::GPR64),
        },
        BtrRmR {
            src: Reg::from_index(0, forge_ir::RegClass::GPR64),
            src2: Reg::from_index(1, forge_ir::RegClass::GPR64),
        },
        BtcRmR {
            src: Reg::from_index(0, forge_ir::RegClass::GPR64),
            src2: Reg::from_index(1, forge_ir::RegClass::GPR64),
        },
        MovRm8R64 {
            src: Reg::from_index(0, forge_ir::RegClass::GPR64),
            dst: Reg::from_index(1, forge_ir::RegClass::GPR64),
        },
        MovR8Rm64 {
            dst: Reg::from_index(0, forge_ir::RegClass::GPR64),
            src: Reg::from_index(1, forge_ir::RegClass::GPR64),
        },
        // @modrm_imm32
        AddRImm32 {
            dst: Reg::from_index(0, forge_ir::RegClass::GPR64),
            imm: 42,
        },
        AddRImm32 {
            dst: Reg::from_index(8, forge_ir::RegClass::GPR64),
            imm: -1,
        },
        OrRImm32 {
            dst: Reg::from_index(0, forge_ir::RegClass::GPR64),
            imm: 42,
        },
        AndRImm32 {
            dst: Reg::from_index(0, forge_ir::RegClass::GPR64),
            imm: 42,
        },
        SubRImm32 {
            dst: Reg::from_index(0, forge_ir::RegClass::GPR64),
            imm: 42,
        },
        XorRImm32 {
            dst: Reg::from_index(0, forge_ir::RegClass::GPR64),
            imm: 42,
        },
        CmpRImm32 {
            src: Reg::from_index(0, forge_ir::RegClass::GPR64),
            imm: 42,
        },
        CmpRImm32 {
            src: Reg::from_index(0, forge_ir::RegClass::GPR64),
            imm: -2048,
        },
        // @sse_rr / 合并后的 movzx（MOVZX_R8_RM/R16_RM；v13 删除了重复的
        // MOVZX_B/MOVZX_W 变体）。源是 8/16 位寄存器（gpr1/gpr2 类）。
        MovzxR8Rm {
            dst: Reg::from_index(0, forge_ir::RegClass::GPR64),
            src: Reg::from_index(1, forge_ir::RegClass::GPR(1)),
        },
        MovzxR16Rm {
            dst: Reg::from_index(0, forge_ir::RegClass::GPR64),
            src: Reg::from_index(1, forge_ir::RegClass::GPR(2)),
        },
        Sqrtsd {
            dst: Reg::from_index(0, forge_ir::RegClass::FPR(16)),
            src: Reg::from_index(1, forge_ir::RegClass::FPR(16)),
        },
        Sqrtsd {
            dst: Reg::from_index(6, forge_ir::RegClass::FPR(16)),
            src: Reg::from_index(7, forge_ir::RegClass::FPR(16)),
        },
        Sqrtss {
            dst: Reg::from_index(0, forge_ir::RegClass::FPR(16)),
            src: Reg::from_index(1, forge_ir::RegClass::FPR(16)),
        },
        Cvtsi2sd {
            dst: Reg::from_index(0, forge_ir::RegClass::FPR(16)),
            src: Reg::from_index(1, forge_ir::RegClass::GPR64),
        },
        Cvtsd2si {
            dst: Reg::from_index(0, forge_ir::RegClass::GPR64),
            src: Reg::from_index(1, forge_ir::RegClass::FPR(16)),
        },
        Cvttsd2si {
            dst: Reg::from_index(0, forge_ir::RegClass::GPR64),
            src: Reg::from_index(1, forge_ir::RegClass::FPR(16)),
        },
        Cvtsd2ss {
            dst: Reg::from_index(0, forge_ir::RegClass::FPR(16)),
            src: Reg::from_index(1, forge_ir::RegClass::FPR(16)),
        },
        Cvtss2sd {
            dst: Reg::from_index(0, forge_ir::RegClass::FPR(16)),
            src: Reg::from_index(1, forge_ir::RegClass::FPR(16)),
        },
        Andpd {
            dst: Reg::from_index(0, forge_ir::RegClass::FPR(16)),
            src: Reg::from_index(1, forge_ir::RegClass::FPR(16)),
        },
        Xorpd {
            dst: Reg::from_index(0, forge_ir::RegClass::FPR(16)),
            src: Reg::from_index(1, forge_ir::RegClass::FPR(16)),
        },
        Comisd {
            src: Reg::from_index(0, forge_ir::RegClass::FPR(16)),
            src2: Reg::from_index(1, forge_ir::RegClass::FPR(16)),
        },
        Comiss {
            src: Reg::from_index(0, forge_ir::RegClass::FPR(16)),
            src2: Reg::from_index(1, forge_ir::RegClass::FPR(16)),
        },
        MovsdXmmFreg {
            dst: Reg::from_index(0, forge_ir::RegClass::FPR(16)),
            src: Reg::from_index(1, forge_ir::RegClass::FPR(16)),
        },
        Cvtsi2ss {
            dst: Reg::from_index(0, forge_ir::RegClass::FPR(16)),
            src: Reg::from_index(1, forge_ir::RegClass::GPR64),
        },
        MovdFregIreg {
            dst: Reg::from_index(0, forge_ir::RegClass::FPR(16)),
            src: Reg::from_index(1, forge_ir::RegClass::GPR64),
        },
        MovdIregFreg {
            src: Reg::from_index(0, forge_ir::RegClass::FPR(16)),
            dst: Reg::from_index(1, forge_ir::RegClass::GPR64),
        },
        Punpckldq {
            dst: Reg::from_index(0, forge_ir::RegClass::FPR(16)),
            src: Reg::from_index(1, forge_ir::RegClass::FPR(16)),
        },
        Punpcklqdq {
            dst: Reg::from_index(0, forge_ir::RegClass::FPR(16)),
            src: Reg::from_index(1, forge_ir::RegClass::FPR(16)),
        },
        Punpckhdq {
            dst: Reg::from_index(0, forge_ir::RegClass::FPR(16)),
            src: Reg::from_index(1, forge_ir::RegClass::FPR(16)),
        },
        Movss {
            dst: Reg::from_index(0, forge_ir::RegClass::FPR(16)),
            src: Reg::from_index(1, forge_ir::RegClass::FPR(16)),
        },
        Movsd {
            dst: Reg::from_index(0, forge_ir::RegClass::FPR(16)),
            src: Reg::from_index(1, forge_ir::RegClass::FPR(16)),
        },
        // @modrm_mem（迭代 3b）
        XaddMemR {
            dst: Reg::from_index(1, forge_ir::RegClass::GPR64),
            src: Reg::from_index(2, forge_ir::RegClass::GPR64),
        },
        XaddMemR {
            dst: Reg::from_index(8, forge_ir::RegClass::GPR64),
            src: Reg::from_index(9, forge_ir::RegClass::GPR64),
        },
        XchgMemR {
            dst: Reg::from_index(1, forge_ir::RegClass::GPR64),
            src: Reg::from_index(2, forge_ir::RegClass::GPR64),
        },
        XchgMemR {
            dst: Reg::from_index(1, forge_ir::RegClass::GPR64),
            src: Reg::from_index(4, forge_ir::RegClass::GPR64),
        }, // base=RSP → SIB
        SubMemR {
            src: Reg::from_index(1, forge_ir::RegClass::GPR64),
            src2: Reg::from_index(2, forge_ir::RegClass::GPR64),
        },
        AndMemR {
            src: Reg::from_index(1, forge_ir::RegClass::GPR64),
            src2: Reg::from_index(2, forge_ir::RegClass::GPR64),
        },
        OrMemR {
            src: Reg::from_index(1, forge_ir::RegClass::GPR64),
            src2: Reg::from_index(2, forge_ir::RegClass::GPR64),
        },
        XorMemR {
            src: Reg::from_index(1, forge_ir::RegClass::GPR64),
            src2: Reg::from_index(2, forge_ir::RegClass::GPR64),
        },
        MovRMem {
            dst: Reg::from_index(0, forge_ir::RegClass::GPR64),
            src: Reg::from_index(2, forge_ir::RegClass::GPR64),
        },
        StoreMemR {
            src: Reg::from_index(1, forge_ir::RegClass::GPR64),
            src2: Reg::from_index(2, forge_ir::RegClass::GPR64),
        },
        MovsdRMem {
            dst: Reg::from_index(0, forge_ir::RegClass::FPR(16)),
            src: Reg::from_index(2, forge_ir::RegClass::GPR64),
        },
        MovsdMemR {
            src: Reg::from_index(0, forge_ir::RegClass::FPR(16)),
            src2: Reg::from_index(2, forge_ir::RegClass::GPR64),
        },
        Mov64Rr {
            src: Reg::from_index(1, forge_ir::RegClass::GPR64),
            dst: Reg::from_index(0, forge_ir::RegClass::GPR64),
        },
        Mov64Rm {
            dst: Reg::from_index(0, forge_ir::RegClass::GPR64),
            mem: MemRef {
                base: Reg::from_index(2, forge_ir::RegClass::GPR64),
                disp: 0,
                index: None,
                scale: 1,
            },
        },
        Mov64Rm {
            dst: Reg::from_index(8, forge_ir::RegClass::GPR64),
            mem: MemRef {
                base: Reg::from_index(4, forge_ir::RegClass::GPR64),
                disp: -8,
                index: None,
                scale: 1,
            },
        },
        Mov64Mr {
            src: Reg::from_index(1, forge_ir::RegClass::GPR64),
            mem: MemRef {
                base: Reg::from_index(2, forge_ir::RegClass::GPR64),
                disp: 0,
                index: None,
                scale: 1,
            },
        },
        MovsdRm {
            dst: Reg::from_index(0, forge_ir::RegClass::FPR(16)),
            mem: MemRef {
                base: Reg::from_index(2, forge_ir::RegClass::GPR64),
                disp: 8,
                index: None,
                scale: 1,
            },
        },
        MovsdMr {
            src: Reg::from_index(0, forge_ir::RegClass::FPR(16)),
            mem: MemRef {
                base: Reg::from_index(2, forge_ir::RegClass::GPR64),
                disp: 0,
                index: None,
                scale: 1,
            },
        },
        // 家族展开（SSE 5 家族）
        Movsd {
            dst: Reg::from_index(0, forge_ir::RegClass::FPR(16)),
            src: Reg::from_index(1, forge_ir::RegClass::FPR(16)),
        },
        Minsd {
            dst: Reg::from_index(6, forge_ir::RegClass::FPR(16)),
            src: Reg::from_index(7, forge_ir::RegClass::FPR(16)),
        },
        Maxsd {
            dst: Reg::from_index(0, forge_ir::RegClass::FPR(16)),
            src: Reg::from_index(1, forge_ir::RegClass::FPR(16)),
        },
        Addsd {
            dst: Reg::from_index(0, forge_ir::RegClass::FPR(16)),
            src: Reg::from_index(1, forge_ir::RegClass::FPR(16)),
        },
        Subsd {
            dst: Reg::from_index(0, forge_ir::RegClass::FPR(16)),
            src: Reg::from_index(1, forge_ir::RegClass::FPR(16)),
        },
        Mulsd {
            dst: Reg::from_index(0, forge_ir::RegClass::FPR(16)),
            src: Reg::from_index(1, forge_ir::RegClass::FPR(16)),
        },
        Divsd {
            dst: Reg::from_index(0, forge_ir::RegClass::FPR(16)),
            src: Reg::from_index(1, forge_ir::RegClass::FPR(16)),
        },
        Addss {
            dst: Reg::from_index(0, forge_ir::RegClass::FPR(16)),
            src: Reg::from_index(1, forge_ir::RegClass::FPR(16)),
        },
        Subss {
            dst: Reg::from_index(0, forge_ir::RegClass::FPR(16)),
            src: Reg::from_index(1, forge_ir::RegClass::FPR(16)),
        },
        Mulss {
            dst: Reg::from_index(0, forge_ir::RegClass::FPR(16)),
            src: Reg::from_index(1, forge_ir::RegClass::FPR(16)),
        },
        Divss {
            dst: Reg::from_index(0, forge_ir::RegClass::FPR(16)),
            src: Reg::from_index(1, forge_ir::RegClass::FPR(16)),
        },
        Movaps {
            dst: Reg::from_index(0, forge_ir::RegClass::FPR(16)),
            src: Reg::from_index(1, forge_ir::RegClass::FPR(16)),
        },
        Addps {
            dst: Reg::from_index(0, forge_ir::RegClass::FPR(16)),
            src: Reg::from_index(1, forge_ir::RegClass::FPR(16)),
        },
        Subps {
            dst: Reg::from_index(0, forge_ir::RegClass::FPR(16)),
            src: Reg::from_index(1, forge_ir::RegClass::FPR(16)),
        },
        Mulps {
            dst: Reg::from_index(0, forge_ir::RegClass::FPR(16)),
            src: Reg::from_index(1, forge_ir::RegClass::FPR(16)),
        },
        Divps {
            dst: Reg::from_index(0, forge_ir::RegClass::FPR(16)),
            src: Reg::from_index(1, forge_ir::RegClass::FPR(16)),
        },
        Xorps {
            dst: Reg::from_index(0, forge_ir::RegClass::FPR(16)),
            src: Reg::from_index(1, forge_ir::RegClass::FPR(16)),
        },
        Andps {
            dst: Reg::from_index(0, forge_ir::RegClass::FPR(16)),
            src: Reg::from_index(1, forge_ir::RegClass::FPR(16)),
        },
        Orps {
            dst: Reg::from_index(0, forge_ir::RegClass::FPR(16)),
            src: Reg::from_index(1, forge_ir::RegClass::FPR(16)),
        },
        Addpd {
            dst: Reg::from_index(0, forge_ir::RegClass::FPR(16)),
            src: Reg::from_index(1, forge_ir::RegClass::FPR(16)),
        },
        Subpd {
            dst: Reg::from_index(0, forge_ir::RegClass::FPR(16)),
            src: Reg::from_index(1, forge_ir::RegClass::FPR(16)),
        },
        Mulpd {
            dst: Reg::from_index(0, forge_ir::RegClass::FPR(16)),
            src: Reg::from_index(1, forge_ir::RegClass::FPR(16)),
        },
        Divpd {
            dst: Reg::from_index(0, forge_ir::RegClass::FPR(16)),
            src: Reg::from_index(1, forge_ir::RegClass::FPR(16)),
        },
        Paddd {
            dst: Reg::from_index(0, forge_ir::RegClass::FPR(16)),
            src: Reg::from_index(1, forge_ir::RegClass::FPR(16)),
        },
        Psubd {
            dst: Reg::from_index(0, forge_ir::RegClass::FPR(16)),
            src: Reg::from_index(1, forge_ir::RegClass::FPR(16)),
        },
        Paddq {
            dst: Reg::from_index(0, forge_ir::RegClass::FPR(16)),
            src: Reg::from_index(1, forge_ir::RegClass::FPR(16)),
        },
        Psubq {
            dst: Reg::from_index(0, forge_ir::RegClass::FPR(16)),
            src: Reg::from_index(1, forge_ir::RegClass::FPR(16)),
        },
        // VEX（三操作数/无源/imm）
        Vaddps {
            dst: Reg::from_index(0, forge_ir::RegClass::FPR(16)),
            src: Reg::from_index(2, forge_ir::RegClass::FPR(16)),
            src2: Reg::from_index(1, forge_ir::RegClass::FPR(16)),
        },
        Vaddps {
            dst: Reg::from_index(5, forge_ir::RegClass::FPR(16)),
            src: Reg::from_index(6, forge_ir::RegClass::FPR(16)),
            src2: Reg::from_index(7, forge_ir::RegClass::FPR(16)),
        },
        Vsubps {
            dst: Reg::from_index(0, forge_ir::RegClass::FPR(16)),
            src: Reg::from_index(2, forge_ir::RegClass::FPR(16)),
            src2: Reg::from_index(1, forge_ir::RegClass::FPR(16)),
        },
        Vmulps {
            dst: Reg::from_index(0, forge_ir::RegClass::FPR(16)),
            src: Reg::from_index(2, forge_ir::RegClass::FPR(16)),
            src2: Reg::from_index(1, forge_ir::RegClass::FPR(16)),
        },
        Vdivps {
            dst: Reg::from_index(0, forge_ir::RegClass::FPR(16)),
            src: Reg::from_index(2, forge_ir::RegClass::FPR(16)),
            src2: Reg::from_index(1, forge_ir::RegClass::FPR(16)),
        },
        Vxorps {
            dst: Reg::from_index(0, forge_ir::RegClass::FPR(16)),
            src: Reg::from_index(2, forge_ir::RegClass::FPR(16)),
            src2: Reg::from_index(1, forge_ir::RegClass::FPR(16)),
        },
        Vandps {
            dst: Reg::from_index(0, forge_ir::RegClass::FPR(16)),
            src: Reg::from_index(2, forge_ir::RegClass::FPR(16)),
            src2: Reg::from_index(1, forge_ir::RegClass::FPR(16)),
        },
        Vaddpd {
            dst: Reg::from_index(0, forge_ir::RegClass::FPR(16)),
            src: Reg::from_index(2, forge_ir::RegClass::FPR(16)),
            src2: Reg::from_index(1, forge_ir::RegClass::FPR(16)),
        },
        Vsubpd {
            dst: Reg::from_index(0, forge_ir::RegClass::FPR(16)),
            src: Reg::from_index(2, forge_ir::RegClass::FPR(16)),
            src2: Reg::from_index(1, forge_ir::RegClass::FPR(16)),
        },
        Vmulpd {
            dst: Reg::from_index(0, forge_ir::RegClass::FPR(16)),
            src: Reg::from_index(2, forge_ir::RegClass::FPR(16)),
            src2: Reg::from_index(1, forge_ir::RegClass::FPR(16)),
        },
        Vdivpd {
            dst: Reg::from_index(0, forge_ir::RegClass::FPR(16)),
            src: Reg::from_index(2, forge_ir::RegClass::FPR(16)),
            src2: Reg::from_index(1, forge_ir::RegClass::FPR(16)),
        },
        Vmovaps {
            dst: Reg::from_index(0, forge_ir::RegClass::FPR(16)),
            src: Reg::from_index(1, forge_ir::RegClass::FPR(16)),
        },
        Vbroadcastss {
            dst: Reg::from_index(0, forge_ir::RegClass::FPR(16)),
            src: Reg::from_index(1, forge_ir::RegClass::FPR(16)),
        },
        Vbroadcastsd {
            dst: Reg::from_index(0, forge_ir::RegClass::FPR(16)),
            src: Reg::from_index(1, forge_ir::RegClass::FPR(16)),
        },
        Vpxor {
            dst: Reg::from_index(0, forge_ir::RegClass::FPR(16)),
            src: Reg::from_index(2, forge_ir::RegClass::FPR(16)),
            src2: Reg::from_index(1, forge_ir::RegClass::FPR(16)),
        },
        Vpaddd {
            dst: Reg::from_index(0, forge_ir::RegClass::FPR(16)),
            src: Reg::from_index(2, forge_ir::RegClass::FPR(16)),
            src2: Reg::from_index(1, forge_ir::RegClass::FPR(16)),
        },
        Vpsubd {
            dst: Reg::from_index(0, forge_ir::RegClass::FPR(16)),
            src: Reg::from_index(2, forge_ir::RegClass::FPR(16)),
            src2: Reg::from_index(1, forge_ir::RegClass::FPR(16)),
        },
        Vpaddq {
            dst: Reg::from_index(0, forge_ir::RegClass::FPR(16)),
            src: Reg::from_index(2, forge_ir::RegClass::FPR(16)),
            src2: Reg::from_index(1, forge_ir::RegClass::FPR(16)),
        },
        Vpsubq {
            dst: Reg::from_index(0, forge_ir::RegClass::FPR(16)),
            src: Reg::from_index(2, forge_ir::RegClass::FPR(16)),
            src2: Reg::from_index(1, forge_ir::RegClass::FPR(16)),
        },
        Vpmulld {
            dst: Reg::from_index(0, forge_ir::RegClass::FPR(16)),
            src: Reg::from_index(2, forge_ir::RegClass::FPR(16)),
            src2: Reg::from_index(1, forge_ir::RegClass::FPR(16)),
        },
        Vextractf128 {
            src: Reg::from_index(2, forge_ir::RegClass::FPR(16)),
            dst: Reg::from_index(0, forge_ir::RegClass::FPR(16)),
            imm: 0,
        },
        Vinsertf128 {
            dst: Reg::from_index(0, forge_ir::RegClass::FPR(16)),
            src: Reg::from_index(2, forge_ir::RegClass::FPR(16)),
            src2: Reg::from_index(1, forge_ir::RegClass::FPR(16)),
            imm: 1,
        },
        // `+r` 形式（迭代 5）
        Push {
            src: Reg::from_index(0, forge_ir::RegClass::GPR64),
        },
        Push {
            src: Reg::from_index(8, forge_ir::RegClass::GPR64),
        },
        Pop {
            dst: Reg::from_index(3, forge_ir::RegClass::GPR64),
        },
        Pop {
            dst: Reg::from_index(9, forge_ir::RegClass::GPR64),
        },
        MovRegImm64 {
            dst: Reg::from_index(0, forge_ir::RegClass::GPR64),
            imm: 0x1234,
        },
        MovRegImm64 {
            dst: Reg::from_index(8, forge_ir::RegClass::GPR64),
            imm: -1,
        },
        BswapR {
            dst: Reg::from_index(0, forge_ir::RegClass::GPR64),
        },
        BswapR {
            dst: Reg::from_index(12, forge_ir::RegClass::GPR64),
        },
        // 控制流（迭代 6）
        Ret,
        JmpRel32 { target: 0 },
        JmpRel32 { target: 42 },
        CallRipRel { target: 0 },
        // 新增指令（迭代 6 收官）
        Nop,
        Ud2,
        Mfence,
        Cqo,
        CallRm {
            src: Reg::from_index(3, forge_ir::RegClass::GPR64),
        },
        JccRel32 { cc: 4, target: 0 },
        JccRel32 { cc: 15, target: -1 },
        SetccRm8 {
            dst: Reg::from_index(2, forge_ir::RegClass::GPR64),
            cc: 5,
        },
        LeaR64Sib {
            dst: Reg::from_index(0, forge_ir::RegClass::GPR64),
            mem: MemRef {
                base: Reg::from_index(5, forge_ir::RegClass::GPR64),
                disp: 8,
                index: None,
                scale: 1,
            },
        },
        LeaRbpOff {
            dst: Reg::from_index(0, forge_ir::RegClass::GPR64),
            mem: MemRef {
                base: Reg::from_index(5, forge_ir::RegClass::GPR64),
                disp: -8,
                index: None,
                scale: 1,
            },
        },
        CmovccRRm {
            dst: Reg::from_index(1, forge_ir::RegClass::GPR64),
            src: Reg::from_index(2, forge_ir::RegClass::GPR64),
            cc: 5,
        },
        RoundsdI {
            dst: Reg::from_index(0, forge_ir::RegClass::FPR(16)),
            src: Reg::from_index(1, forge_ir::RegClass::FPR(16)),
            imm: 3,
        },
        Pmulld {
            dst: Reg::from_index(0, forge_ir::RegClass::FPR(16)),
            src: Reg::from_index(1, forge_ir::RegClass::FPR(16)),
        },
        MovqXmmR64 {
            dst: Reg::from_index(0, forge_ir::RegClass::FPR(16)),
            src: Reg::from_index(1, forge_ir::RegClass::GPR64),
        },
        MovqR64Xmm {
            dst: Reg::from_index(1, forge_ir::RegClass::GPR64),
            src: Reg::from_index(0, forge_ir::RegClass::FPR(16)),
        },
        MovapsMr {
            src: Reg::from_index(1, forge_ir::RegClass::GPR64),
            src2: Reg::from_index(0, forge_ir::RegClass::FPR(16)),
        },
        LzcntR {
            dst: Reg::from_index(0, forge_ir::RegClass::GPR64),
            src: Reg::from_index(1, forge_ir::RegClass::GPR64),
        },
        TzcntR {
            dst: Reg::from_index(0, forge_ir::RegClass::GPR64),
            src: Reg::from_index(1, forge_ir::RegClass::GPR64),
        },
        PopcntR {
            dst: Reg::from_index(0, forge_ir::RegClass::GPR64),
            src: Reg::from_index(1, forge_ir::RegClass::GPR64),
        },
    ]
}

#[test]
fn decode_encode_byte_roundtrip_all() {
    for inst in all_insts() {
        let b = encode(&inst).unwrap_or_else(|e| panic!("encode {inst:?}: {e}"));
        let (dec, n) = decode(&b).unwrap_or_else(|| panic!("decode {b:02x?} ({inst:?})"));
        assert_eq!(n, b.len(), "{inst:?}: 解码消费 {n} != {} 字节", b.len());
        let b2 = encode(&dec).unwrap_or_else(|e| panic!("re-encode {dec:?}: {e}"));
        assert_eq!(
            b2, b,
            "{inst:?}: decode→encode 往返字节不一致\n  orig {b:02x?}\n  dec  {b2:02x?}\n  decd {dec:?}"
        );
    }
}

#[test]
fn decode_identity_canonical() {
    // 规范指令（非别名）：decode(encode(X)) == X
    let canonical = [
        Inst::MovRRm {
            dst: Reg::from_index(0, forge_ir::RegClass::GPR64),
            src: Reg::from_index(1, forge_ir::RegClass::GPR64),
        },
        Inst::MovRRm {
            dst: Reg::from_index(8, forge_ir::RegClass::GPR64),
            src: Reg::from_index(9, forge_ir::RegClass::GPR64),
        },
        Inst::MovsxdRRm {
            dst: Reg::from_index(0, forge_ir::RegClass::GPR64),
            src: Reg::from_index(1, forge_ir::RegClass::GPR64),
        },
        Inst::AddRmR {
            src: Reg::from_index(0, forge_ir::RegClass::GPR64),
            dst: Reg::from_index(1, forge_ir::RegClass::GPR64),
        },
        Inst::ImulRRm {
            dst: Reg::from_index(0, forge_ir::RegClass::GPR64),
            src: Reg::from_index(1, forge_ir::RegClass::GPR64),
        },
        Inst::CmpRmR {
            src: Reg::from_index(0, forge_ir::RegClass::GPR64),
            src2: Reg::from_index(1, forge_ir::RegClass::GPR64),
        },
        Inst::AddRImm32 {
            dst: Reg::from_index(0, forge_ir::RegClass::GPR64),
            imm: 42,
        },
        Inst::CmpRImm32 {
            src: Reg::from_index(8, forge_ir::RegClass::GPR64),
            imm: -1,
        },
        Inst::Sqrtsd {
            dst: Reg::from_index(0, forge_ir::RegClass::FPR(16)),
            src: Reg::from_index(1, forge_ir::RegClass::FPR(16)),
        },
        Inst::Cvtsi2sd {
            dst: Reg::from_index(0, forge_ir::RegClass::FPR(16)),
            src: Reg::from_index(1, forge_ir::RegClass::GPR64),
        },
        Inst::Andpd {
            dst: Reg::from_index(0, forge_ir::RegClass::FPR(16)),
            src: Reg::from_index(1, forge_ir::RegClass::FPR(16)),
        },
        Inst::Comiss {
            src: Reg::from_index(0, forge_ir::RegClass::FPR(16)),
            src2: Reg::from_index(1, forge_ir::RegClass::FPR(16)),
        },
        Inst::Movss {
            dst: Reg::from_index(0, forge_ir::RegClass::FPR(16)),
            src: Reg::from_index(1, forge_ir::RegClass::FPR(16)),
        },
        Inst::Punpcklqdq {
            dst: Reg::from_index(0, forge_ir::RegClass::FPR(16)),
            src: Reg::from_index(1, forge_ir::RegClass::FPR(16)),
        },
    ];
    for inst in canonical {
        let b = encode(&inst).unwrap();
        let (dec, _) = decode(&b).unwrap();
        assert_eq!(dec, inst, "{inst:?}: 解码未还原（{b:02x?}）");
    }
}

// ─────────────────── assemble/disassemble 往返 ───────────────────

#[test]
fn assemble_disassemble_roundtrip() {
    let cases = [
        "mov RAX, RBX",
        "mov R8, R9",
        "mov EAX, EBX",
        "mov AX, BX",
        "add RAX, RBX",
        "add RAX, 42",
        "cmp R8, -1",
        "movzx RAX, AL",
        "movzx RAX, AX",
        "movsxd RAX, RBX",
        "imul RAX, RBX",
        "sqrtsd XMM0, XMM1",
        "cvtsi2sd XMM0, RAX",
        "andpd XMM0, XMM1",
        "comiss XMM0, XMM1",
        "movd XMM0, RAX",
        "punpcklqdq XMM0, XMM1",
        // 家族展开（SSE）
        "addsd XMM0, XMM1",
        "addss XMM0, XMM1",
        "movaps XMM0, XMM1",
        "addpd XMM0, XMM1",
        "paddd XMM0, XMM1",
        // VEX 三操作数（asm 模板重排 dest, src1, src2）
        "vaddps XMM0, XMM1, XMM2",
        "vaddpd XMM0, XMM1, XMM2",
        "vpxor XMM0, XMM1, XMM2",
        // VEX 无源
        "vmovaps XMM0, XMM1",
        "vbroadcastss XMM0, XMM1",
        // VEX imm
        "vextractf128 XMM1, XMM2, 0",
        "vinsertf128 XMM0, XMM1, XMM2, 1",
        // `+r` 形式
        "push RAX",
        "push R8",
        "pop RBX",
        "pop R9",
        "bswap RAX",
        "bswap R12",
        "mov RAX, 0x1234",
    ];
    for c in cases {
        let inst = assemble(c).unwrap_or_else(|e| panic!("assemble `{c}`: {e}"));
        let text = disassemble(&inst);
        let inst2 = assemble(&text).unwrap_or_else(|e| panic!("re-assemble `{text}`: {e}"));
        assert_eq!(inst, inst2, "disassemble `{c}` → `{text}` 往返");
    }
}

#[test]
fn disassemble_known_texts() {
    assert_eq!(
        disassemble(&assemble("mov RAX, RBX").unwrap()),
        "mov RAX, RBX"
    );
    assert_eq!(
        disassemble(&assemble("add RAX, 42").unwrap()),
        "add RAX, 42"
    );
    assert_eq!(
        disassemble(&assemble("sqrtsd XMM0, XMM1").unwrap()),
        "sqrtsd XMM0, XMM1"
    );
}

/// 类表 = **ISA 声明**（2026-09-13 去「编造 fallback 类表」）：
/// x86 的类表必须只含"已声明族/类型宽度/tier"，且**不得**出现未声明的类
/// （历史 `fallback_classes` 会凭空造出 GPR(2)/FPR(4)/VEC(16)… 并借池）。
#[test]
fn class_table_is_declared_isa_data() {
    let tm = forge_codegen::x86_v12::TargetMachine::new();
    let classes: Vec<RegClass> = forge_codegen::TargetMachine::reg_info(&tm)
        .register_classes()
        .iter()
        .map(|c| c.reg_class)
        .collect();
    for want in [
        RegClass::GPR(1),
        RegClass::GPR(2),
        RegClass::GPR(4),
        RegClass::GPR(8),
        RegClass::FPR(8),
        RegClass::FPR(16),
        RegClass::FPR(32),
        RegClass::VEC(16),
        RegClass::VEC(32),
        RegClass::VEC(64),
    ] {
        assert!(classes.contains(&want), "类表缺少 {want:?}：{classes:?}");
    }
    // `fpr4` 未声明（x86 只有 fpr8/fpr16/fpr32）→ 不得凭空出现。
    assert!(
        !classes.contains(&RegClass::FPR(4)),
        "未声明的 FPR(4) 不得出现：{classes:?}"
    );
    // 每个类都必须有非空可分配池（否则 regalloc 会在该类的值上死循环/报错）。
    for c in forge_codegen::TargetMachine::reg_info(&tm).register_classes() {
        assert!(
            !c.allocatable.is_empty(),
            "类 {}（{:?}）可分配池为空",
            c.name,
            c.reg_class
        );
    }
    // VEC(64)（V512）池必须来自浮点文件（x86 的 ZMM 与 XMM 同编号空间）。
    let v512 = forge_codegen::TargetMachine::reg_info(&tm)
        .register_classes()
        .into_iter()
        .find(|c| c.reg_class == RegClass::VEC(64))
        .expect("VEC(64)");
    assert_eq!(v512.width, 64, "VEC(64).width 必须是 64（spill 槽按值宽）");
    assert!(v512.allocatable.contains(&15), "池应含 XMM15：{v512:?}");
}
