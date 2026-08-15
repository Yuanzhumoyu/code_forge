//! x86-64 v12 pilot 验证（迭代 3：变长语义键）。
//!
//! - **golden**：GPR 指令（@modrm 24 + @modrm_imm32 6）字节与 v11 逐字节一致
//!   （汇编→编码对比）；opsize 16/32/64 用直接 Inst 构造对比（v11 的 66 前缀/
//!   REX.W 由 opsize 字段驱动）。
//! - **规范字节**：SSE 指令按 x86 规范硬编码 oracle（v11 的 FPR `16+i` 索引
//!   使 SSE 字节带多余 REX 且不合规；v12 组内索引规范正确）。
//! - **字节级往返**：decode(encode(X)) 再 encode 字节一致（含编码撞车的别名
//!   指令，如 MOVSXD_R_GPR vs MOVSXD_R_RM、MOVSD 三胞胎、MOVZX_B vs
//!   MOVZX_R8_RM——声明序首匹配，与 v11 一致）。
//! - **assemble/disassemble** 往返（opsize 操作数非文本，默认 64）。

use forge_codegen::x86_v12::{Inst, MemRef, assemble, decode, disassemble, encode};

fn v11_bytes(asm: &str) -> Vec<u8> {
    use forge_codegen::machine::assembler::TargetAssembler;
    use forge_codegen::machine::target::TargetMachine;
    let tm = forge_codegen::x86_64::TargetMachine::new();
    let asm_ = forge_codegen::x86_64::Assembler;
    let insts = asm_
        .parse_insts(asm)
        .unwrap_or_else(|e| panic!("v11 parse `{asm}`: {e:?}"));
    let inst = insts.into_iter().next().expect("one inst");
    tm.encoder()
        .encode_to_bytes(&inst, &forge_codegen::AllocResult::new())
        .unwrap_or_else(|e| panic!("v11 encode {inst:?}: {e:?}"))
}

fn v12_bytes(asm: &str) -> Vec<u8> {
    let inst = assemble(asm).unwrap_or_else(|e| panic!("v12 assemble `{asm}`: {e}"));
    encode(&inst).unwrap_or_else(|e| panic!("v12 encode {inst:?}: {e}"))
}

// ─────────────────── golden：GPR @modrm/@modrm_imm32 与 v11 一致 ───────────────────

#[test]
fn golden_gpr_matches_v11() {
    let cases = [
        "movrr RAX, RBX",
        "movrr R8, R9", // REX.R/B 扩展
        "mov RAX, RBX", // 0x89 MR 方向
        "mov R8, R9",
        "movsxd RAX, RBX",
        "add RAX, RBX",
        "sub RAX, RBX",
        "imul RAX, RBX",
        "xor RAX, RBX",
        "and RAX, RBX",
        "or RAX, RBX",
        "cmp RAX, RBX",
        "test RAX, RBX",
        "cmpxchg RAX, RBX",
        "xadd RAX, RBX",
        "bt RAX, RBX",
        "bts RAX, RBX",
        "btr RAX, RBX",
        "btc RAX, RBX",
        "movrm RAX, RBX", // 0x89
        "movr8 RAX, RBX", // 0x8B
        // imm32 形式（81 /digit + imm32）
        "add RAX, 42",
        "sub RAX, 42",
        "and RAX, 42",
        "or RAX, 42",
        "xor RAX, 42",
        "cmp RAX, 42",
        "add R8, 42", // 扩展寄存器
        // 内存寻址（@modrm_mem）
        "xadd [RAX], RBX",
        "xadd [R8], R9",
        "xchg [RAX], RBX",
        "xchg [RSP], RBX", // base=RSP → SIB
        // 注：locksub/lockand/lockor/lockxor 不在 golden——v11 带多余 0F escape
        //（F0 48 0F 29 非规范），v12 规范修正（F0 48 29），见 mem_spec_bytes。
        "mov_mem RAX, [RBX]",
        "mov_sto [RAX], RBX",
        "mov64rr RAX, RBX",
    ];
    for c in cases {
        let vb = v11_bytes(c);
        let v12b = v12_bytes(c);
        assert_eq!(
            v12b, vb,
            "golden mismatch for `{c}` (v12 {v12b:02x?} vs v11 {vb:02x?})"
        );
    }
}

#[test]
fn opsize_prefix_and_rex_w() {
    // 直接构造对比 v11（v11 的 66 前缀/REX.W 由 opsize 字段驱动）
    use forge_codegen::machine::target::TargetMachine;
    let tm = forge_codegen::x86_64::TargetMachine::new();
    let rm = forge_codegen::AllocResult::new();
    let v11e = |inst: &forge_codegen::x86_64::Inst| {
        tm.encoder()
            .encode_to_bytes(inst, &rm)
            .unwrap_or_else(|e| panic!("v11 encode {inst:?}: {e:?}"))
    };
    let cases: &[(u8, u32, u32, &[u8])] = &[
        // opsize → 期望字节（v11 与 v12 应一致；索引 = 物理编号，RAX=0 RCX=1）
        (16, 0, 1, &[0x66, 0x8B, 0xC1]), // movrr ax, cx
        (32, 0, 1, &[0x8B, 0xC1]),       // movrr eax, ecx
        (64, 0, 1, &[0x48, 0x8B, 0xC1]), // movrr rax, rcx
        (64, 8, 9, &[0x4D, 0x8B, 0xC1]), // movrr r8, r9（REX.W+R+B）
        (64, 1, 8, &[0x49, 0x8B, 0xC8]), // movrr rcx, r8（REX.W+B）
    ];
    for (opsize, dest, src, expected) in cases {
        let d0 = match dest {
            0 => forge_codegen::x86_64::Reg::RAX,
            1 => forge_codegen::x86_64::Reg::RCX,
            8 => forge_codegen::x86_64::Reg::R8,
            _ => panic!("bad dest"),
        };
        let s0 = match src {
            1 => forge_codegen::x86_64::Reg::RCX,
            8 => forge_codegen::x86_64::Reg::R8,
            9 => forge_codegen::x86_64::Reg::R9,
            _ => panic!("bad src"),
        };
        let vb = v11e(&forge_codegen::x86_64::Inst::MovRRm {
            dest: d0,
            src: s0,
            opsize: *opsize,
        });
        let v12b = encode(&Inst::MovRRm {
            op0: *dest,
            op1: *src,
            op2: *opsize,
        })
        .unwrap();
        assert_eq!(
            v12b.as_slice(),
            *expected,
            "opsize={opsize} dest={dest} src={src}"
        );
        assert_eq!(
            v12b, vb,
            "opsize={opsize} dest={dest} src={src} 与 v11 不一致"
        );
    }
}

// ─────────────────── SSE 规范字节（v11 的 16+i 索引不合规）───────────────────

#[test]
fn sse_spec_bytes() {
    // v12 组内索引 → 规范编码（无多余 REX）
    let cases: &[(&str, &[u8])] = &[
        ("sqrtsd xmm0, xmm1", &[0xF2, 0x0F, 0x51, 0xC1]),
        ("sqrtss xmm0, xmm1", &[0x0F, 0x51, 0xC1]),
        ("cvtsi2sd xmm0, rax", &[0xF2, 0x0F, 0x2A, 0xC0]),
        ("cvtsd2si rax, xmm0", &[0xF2, 0x0F, 0x2D, 0xC0]),
        ("cvttsd2si rax, xmm0", &[0xF2, 0x0F, 0x2C, 0xC0]),
        ("cvtsd2ss xmm0, xmm1", &[0xF2, 0x0F, 0x5A, 0xC1]),
        ("cvtss2sd xmm0, xmm1", &[0xF3, 0x0F, 0x5A, 0xC1]),
        ("andpd xmm0, xmm1", &[0x66, 0x0F, 0x54, 0xC1]),
        ("xorpd xmm0, xmm1", &[0x66, 0x0F, 0x57, 0xC1]),
        ("comisd xmm0, xmm1", &[0x66, 0x0F, 0x2F, 0xC1]),
        ("comiss xmm0, xmm1", &[0x0F, 0x2F, 0xC1]),
        ("cvtsi2ss xmm0, rax", &[0xF3, 0x0F, 0x2A, 0xC0]),
        ("movd xmm0, rax", &[0x66, 0x0F, 0x6E, 0xC0]),
        ("movd_fr rax, xmm0", &[0x66, 0x0F, 0x7E, 0xC0]),
        ("punpckldq xmm0, xmm1", &[0x66, 0x0F, 0x62, 0xC1]),
        ("punpcklqdq xmm0, xmm1", &[0x66, 0x0F, 0x6C, 0xC1]),
        ("punpckhdq xmm0, xmm1", &[0x66, 0x0F, 0x6A, 0xC1]),
        ("movss xmm0, xmm1", &[0xF3, 0x0F, 0x10, 0xC1]),
        ("movsd xmm0, xmm1", &[0xF2, 0x0F, 0x10, 0xC1]),
        // 扩展 XMM → REX.R/B
        ("sqrtsd xmm8, xmm9", &[0xF2, 0x45, 0x0F, 0x51, 0xC1]),
        // GPR-字段的 SSE_RR 形式（MOVZX_B）
        ("movzx_b rax, rbx", &[0x0F, 0xB6, 0xC3]),
        ("movzx_w rax, rbx", &[0x0F, 0xB7, 0xC3]),
    ];
    for (asm, expected) in cases {
        let got = v12_bytes(asm);
        assert_eq!(got.as_slice(), *expected, "spec mismatch for `{asm}`");
    }
}

// ─────────────────── 内存寻址规范字节（@modrm_mem，迭代 3b）───────────────────

#[test]
fn mem_spec_bytes() {
    let cases: &[(&str, &[u8])] = &[
        // xchg [rax], rbx — 48 87 /r（mod=00）
        ("xchg [RAX], RBX", &[0x48, 0x87, 0x18]),
        // xchg [rsp], rbx — base=RSP → SIB（index=4 无 index）
        ("xchg [RSP], RBX", &[0x48, 0x87, 0x1C, 0x24]),
        // xadd [rax], rbx — F0 LOCK + 48 0F C1 /r
        ("xadd [RAX], RBX", &[0xF0, 0x48, 0x0F, 0xC1, 0x18]),
        // locksub [rax], rbx — F0 48 29 /r（v11 带多余 0F escape 非规范，v12 修正）
        ("locksub [RAX], RBX", &[0xF0, 0x48, 0x29, 0x18]),
        // mov_mem rax, [rbx] — 48 8B /r
        ("mov_mem RAX, [RBX]", &[0x48, 0x8B, 0x03]),
        // mov_sto [rax], rbx — 48 89 /r
        ("mov_sto [RAX], RBX", &[0x48, 0x89, 0x18]),
        // movsd xmm0, [rax] — F2 48?? 不——F2 0F 10（64 位无 REX.W；base=RAX<8）
        ("movsd_mem XMM0, [RAX]", &[0xF2, 0x0F, 0x10, 0x00]),
        // mov64rr rax, rbx — 48 89 /r（reg=src=3, rm=dest=0）
        ("mov64rr RAX, RBX", &[0x48, 0x89, 0xD8]),
        // mov64rm rax, [rbp+8] — RBP ∈ force_disp_base → mod=01 + disp8
        ("mov64rm RAX, [RBP+8]", &[0x48, 0x8B, 0x45, 0x08]),
        // mov64rm rax, [rbx-8] — mod=01 + disp8=-8
        ("mov64rm RAX, [RBX-8]", &[0x48, 0x8B, 0x43, 0xF8]),
    ];
    for (asm, expected) in cases {
        let got = v12_bytes(asm);
        assert_eq!(got.as_slice(), *expected, "mem spec mismatch for `{asm}`");
    }
}

// ─────────────────── 家族（SSE）与 VEX 规范字节（迭代 4）───────────────────

#[test]
fn family_sse_spec_bytes() {
    // SSE 家族展开：v11 的 FPR 16+i 使字节带多余 REX，v12 组内索引规范正确
    let cases: &[(&str, &[u8])] = &[
        ("addsd XMM0, XMM1", &[0xF2, 0x0F, 0x58, 0xC1]),
        ("minsd XMM0, XMM1", &[0xF2, 0x0F, 0x5D, 0xC1]),
        ("maxsd XMM0, XMM1", &[0xF2, 0x0F, 0x5F, 0xC1]),
        ("addss XMM0, XMM1", &[0xF3, 0x0F, 0x58, 0xC1]),
        ("divss XMM0, XMM1", &[0xF3, 0x0F, 0x5E, 0xC1]),
        ("movaps XMM0, XMM1", &[0x0F, 0x28, 0xC1]),
        ("addps XMM0, XMM1", &[0x0F, 0x58, 0xC1]),
        ("orps XMM0, XMM1", &[0x0F, 0x56, 0xC1]),
        ("addpd XMM0, XMM1", &[0x66, 0x0F, 0x58, 0xC1]),
        ("paddd XMM0, XMM1", &[0x66, 0x0F, 0xFE, 0xC1]),
        ("psubq XMM0, XMM1", &[0x66, 0x0F, 0xFB, 0xC1]),
    ];
    for (asm, expected) in cases {
        let got = v12_bytes(asm);
        assert_eq!(got.as_slice(), *expected, "family SSE mismatch for `{asm}`");
    }
}

#[test]
fn vex_spec_bytes() {
    // VEX 规范字节（组内索引；v11 的 16+i 使 VEX 字节非规范）
    let cases: &[(&str, &[u8])] = &[
        // vaddps ymm0, ymm1, ymm2：reg=0, rm=2, vvvv=~1=0xE, L=1, pp=0
        ("vaddps XMM0, XMM1, XMM2", &[0xC4, 0xE1, 0x74, 0x58, 0xC2]),
        // vsubps
        ("vsubps XMM0, XMM1, XMM2", &[0xC4, 0xE1, 0x74, 0x5C, 0xC2]),
        // vmovaps ymm1, ymm2（无源 vvvv=0xF, L=1）
        ("vmovaps XMM1, XMM2", &[0xC4, 0xE1, 0x7C, 0x28, 0xCA]),
        // vaddpd（pp=1）
        ("vaddpd XMM0, XMM1, XMM2", &[0xC4, 0xE1, 0x75, 0x58, 0xC2]),
        // vpxor（pp=1）
        ("vpxor XMM0, XMM1, XMM2", &[0xC4, 0xE1, 0x75, 0xEF, 0xC2]),
        // vbroadcastss（map=2, pp=1, 无源）
        ("vbroadcastss XMM0, XMM1", &[0xC4, 0xE2, 0x7D, 0x18, 0xC1]),
        // 扩展寄存器 → R/B 反位（reg=8→R=0, rm=10→B=0, vvvv=~9=6）
        ("vaddps XMM8, XMM9, XMM10", &[0xC4, 0x41, 0x34, 0x58, 0xC2]),
        // vextractf128 xmm1, ymm2, 0：reg=src(2), rm=dest(1), imm8
        (
            "vextractf128 XMM1, XMM2, 0",
            &[0xC4, 0xE3, 0x7D, 0x19, 0xD1, 0x00],
        ),
        // vinsertf128 ymm0, ymm1, xmm2, 1：reg=0, rm=2, vvvv=~1=0xE
        (
            "vinsertf128 XMM0, XMM1, XMM2, 1",
            &[0xC4, 0xE3, 0x75, 0x18, 0xC2, 0x01],
        ),
    ];
    for (asm, expected) in cases {
        let got = v12_bytes(asm);
        assert_eq!(got.as_slice(), *expected, "VEX mismatch for `{asm}`");
    }
}

// ─────────────────── 字节级 decode 往返 ───────────────────

/// 全部指令的代表性 Inst 值（含别名——字节级往返不要求变体相等）。
/// 变长 form 的操作数字段名 = 位置名 op0/op1/op2（ModRM 语义由 form 键驱动）。
fn all_insts() -> Vec<Inst> {
    use Inst::*;
    vec![
        // @modrm（opsize 各值）
        MovRRm {
            op0: 0,
            op1: 1,
            op2: 64,
        },
        MovRRm {
            op0: 8,
            op1: 9,
            op2: 64,
        },
        MovRRm {
            op0: 0,
            op1: 1,
            op2: 16,
        },
        MovRRm {
            op0: 0,
            op1: 1,
            op2: 32,
        },
        MovRRm {
            op0: 2,
            op1: 3,
            op2: 64,
        },
        MovsxdRRm { op0: 0, op1: 1 },
        MovsxdRGpr { op0: 0, op1: 1 },
        AddRmR {
            op0: 0,
            op1: 1,
            op2: 64,
        },
        SubRmR {
            op0: 0,
            op1: 1,
            op2: 64,
        },
        ImulRRm {
            op0: 0,
            op1: 1,
            op2: 64,
        },
        XorRmR {
            op0: 0,
            op1: 1,
            op2: 64,
        },
        AndRmR {
            op0: 0,
            op1: 1,
            op2: 64,
        },
        OrRmR {
            op0: 0,
            op1: 1,
            op2: 64,
        },
        CmpRmR {
            op0: 0,
            op1: 1,
            op2: 64,
        },
        TestRmR {
            op0: 0,
            op1: 1,
            op2: 64,
        },
        MovzxR8Rm {
            op0: 0,
            op1: 1,
            op2: 64,
        },
        MovzxR16Rm {
            op0: 0,
            op1: 1,
            op2: 64,
        },
        MovsxR8Rm {
            op0: 0,
            op1: 1,
            op2: 64,
        },
        MovsxR16Rm {
            op0: 0,
            op1: 1,
            op2: 64,
        },
        CmpxchgRmR {
            op0: 0,
            op1: 1,
            op2: 64,
        },
        XaddRmR {
            op0: 0,
            op1: 1,
            op2: 64,
        },
        BtRmR {
            op0: 0,
            op1: 1,
            op2: 64,
        },
        BtsRmR {
            op0: 0,
            op1: 1,
            op2: 64,
        },
        BtrRmR {
            op0: 0,
            op1: 1,
            op2: 64,
        },
        BtcRmR {
            op0: 0,
            op1: 1,
            op2: 64,
        },
        MovRm8R64 {
            op0: 0,
            op1: 1,
            op2: 64,
        },
        MovR8Rm64 {
            op0: 0,
            op1: 1,
            op2: 64,
        },
        // @modrm_imm32
        AddRImm32 {
            op0: 0,
            op1: 42,
            op2: 64,
        },
        AddRImm32 {
            op0: 8,
            op1: -1,
            op2: 64,
        },
        OrRImm32 {
            op0: 0,
            op1: 42,
            op2: 64,
        },
        AndRImm32 {
            op0: 0,
            op1: 42,
            op2: 64,
        },
        SubRImm32 {
            op0: 0,
            op1: 42,
            op2: 64,
        },
        XorRImm32 {
            op0: 0,
            op1: 42,
            op2: 64,
        },
        CmpRImm32 {
            op0: 0,
            op1: 42,
            op2: 64,
        },
        CmpRImm32 {
            op0: 0,
            op1: -2048,
            op2: 16,
        },
        // @sse_rr
        MovzxB { op0: 0, op1: 1 },
        MovzxW { op0: 0, op1: 1 },
        Sqrtsd { op0: 0, op1: 1 },
        Sqrtsd { op0: 8, op1: 9 },
        Sqrtss { op0: 0, op1: 1 },
        Cvtsi2sd { op0: 0, op1: 1 },
        Cvtsd2si { op0: 0, op1: 1 },
        Cvttsd2si { op0: 0, op1: 1 },
        Cvtsd2ss { op0: 0, op1: 1 },
        Cvtss2sd { op0: 0, op1: 1 },
        Andpd { op0: 0, op1: 1 },
        Xorpd { op0: 0, op1: 1 },
        Comisd { op0: 0, op1: 1 },
        Comiss { op0: 0, op1: 1 },
        MovsdXmmFreg { op0: 0, op1: 1 },
        MovsdFregXmm { op0: 0, op1: 1 },
        Cvtsi2ss { op0: 0, op1: 1 },
        MovdFregIreg { op0: 0, op1: 1 },
        MovdIregFreg { op0: 0, op1: 1 },
        Punpckldq { op0: 0, op1: 1 },
        Punpcklqdq { op0: 0, op1: 1 },
        Punpckhdq { op0: 0, op1: 1 },
        Movss { op0: 0, op1: 1 },
        MovsdRr { op0: 0, op1: 1 },
        // @modrm_mem（迭代 3b）
        XaddMemR {
            op0: 1,
            op1: 2,
            op2: 64,
        },
        XaddMemR {
            op0: 8,
            op1: 9,
            op2: 64,
        },
        XchgMemR {
            op0: 1,
            op1: 2,
            op2: 64,
        },
        XchgMemR {
            op0: 1,
            op1: 4,
            op2: 64,
        }, // base=RSP → SIB
        SubMemR {
            op0: 1,
            op1: 2,
            op2: 64,
        },
        AndMemR {
            op0: 1,
            op1: 2,
            op2: 64,
        },
        OrMemR {
            op0: 1,
            op1: 2,
            op2: 64,
        },
        XorMemR {
            op0: 1,
            op1: 2,
            op2: 64,
        },
        MovRMem {
            op0: 0,
            op1: 2,
            op2: 64,
        },
        StoreMemR {
            op0: 1,
            op1: 2,
            op2: 64,
        },
        MovsdRMem { op0: 0, op1: 2 },
        MovsdMemR { op0: 0, op1: 2 },
        Mov64Rr { op0: 1, op1: 0 },
        Mov64Rm {
            op0: 0,
            op1: MemRef { base: 2, disp: 0 },
        },
        Mov64Rm {
            op0: 8,
            op1: MemRef { base: 4, disp: -8 },
        },
        Mov64Mr {
            op0: 1,
            op1: MemRef { base: 2, disp: 0 },
        },
        MovsdRm {
            op0: 0,
            op1: MemRef { base: 2, disp: 8 },
        },
        MovsdMr {
            op0: 0,
            op1: MemRef { base: 2, disp: 0 },
        },
        // 家族展开（SSE 5 家族）
        Movsd { op0: 0, op1: 1 },
        Minsd { op0: 8, op1: 9 },
        Maxsd { op0: 0, op1: 1 },
        Addsd { op0: 0, op1: 1 },
        Subsd { op0: 0, op1: 1 },
        Mulsd { op0: 0, op1: 1 },
        Divsd { op0: 0, op1: 1 },
        Addss { op0: 0, op1: 1 },
        Subss { op0: 0, op1: 1 },
        Mulss { op0: 0, op1: 1 },
        Divss { op0: 0, op1: 1 },
        Movaps { op0: 0, op1: 1 },
        Addps { op0: 0, op1: 1 },
        Subps { op0: 0, op1: 1 },
        Mulps { op0: 0, op1: 1 },
        Divps { op0: 0, op1: 1 },
        Xorps { op0: 0, op1: 1 },
        Andps { op0: 0, op1: 1 },
        Orps { op0: 0, op1: 1 },
        Addpd { op0: 0, op1: 1 },
        Subpd { op0: 0, op1: 1 },
        Mulpd { op0: 0, op1: 1 },
        Divpd { op0: 0, op1: 1 },
        Paddd { op0: 0, op1: 1 },
        Psubd { op0: 0, op1: 1 },
        Paddq { op0: 0, op1: 1 },
        Psubq { op0: 0, op1: 1 },
        // VEX（三操作数/无源/imm）
        Vaddps {
            op0: 0,
            op1: 2,
            op2: 1,
        },
        Vaddps {
            op0: 8,
            op1: 9,
            op2: 10,
        },
        Vsubps {
            op0: 0,
            op1: 2,
            op2: 1,
        },
        Vmulps {
            op0: 0,
            op1: 2,
            op2: 1,
        },
        Vdivps {
            op0: 0,
            op1: 2,
            op2: 1,
        },
        Vxorps {
            op0: 0,
            op1: 2,
            op2: 1,
        },
        Vandps {
            op0: 0,
            op1: 2,
            op2: 1,
        },
        Vaddpd {
            op0: 0,
            op1: 2,
            op2: 1,
        },
        Vsubpd {
            op0: 0,
            op1: 2,
            op2: 1,
        },
        Vmulpd {
            op0: 0,
            op1: 2,
            op2: 1,
        },
        Vdivpd {
            op0: 0,
            op1: 2,
            op2: 1,
        },
        Vmovaps { op0: 0, op1: 1 },
        Vbroadcastss { op0: 0, op1: 1 },
        Vbroadcastsd { op0: 0, op1: 1 },
        Vpxor {
            op0: 0,
            op1: 2,
            op2: 1,
        },
        Vpaddd {
            op0: 0,
            op1: 2,
            op2: 1,
        },
        Vpsubd {
            op0: 0,
            op1: 2,
            op2: 1,
        },
        Vpaddq {
            op0: 0,
            op1: 2,
            op2: 1,
        },
        Vpsubq {
            op0: 0,
            op1: 2,
            op2: 1,
        },
        Vpmulld {
            op0: 0,
            op1: 2,
            op2: 1,
        },
        Vextractf128 {
            op0: 2,
            op1: 0,
            op2: 0,
        },
        Vinsertf128 {
            op0: 0,
            op1: 2,
            op2: 1,
            op3: 1,
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
            op0: 0,
            op1: 1,
            op2: 64,
        },
        Inst::MovRRm {
            op0: 8,
            op1: 9,
            op2: 16,
        },
        Inst::MovsxdRRm { op0: 0, op1: 1 },
        Inst::AddRmR {
            op0: 0,
            op1: 1,
            op2: 64,
        },
        Inst::ImulRRm {
            op0: 0,
            op1: 1,
            op2: 32,
        },
        Inst::CmpRmR {
            op0: 0,
            op1: 1,
            op2: 64,
        },
        Inst::AddRImm32 {
            op0: 0,
            op1: 42,
            op2: 64,
        },
        Inst::CmpRImm32 {
            op0: 8,
            op1: -1,
            op2: 64,
        },
        Inst::Sqrtsd { op0: 0, op1: 1 },
        Inst::Cvtsi2sd { op0: 0, op1: 1 },
        Inst::Andpd { op0: 0, op1: 1 },
        Inst::Comiss { op0: 0, op1: 1 },
        Inst::Movss { op0: 0, op1: 1 },
        Inst::Punpcklqdq { op0: 0, op1: 1 },
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
        "movrr rax, rbx",
        "movrr r8, r9",
        "add rax, rbx",
        "add rax, 42",
        "cmp r8, -1",
        "movzx rax, rbx",
        "movsxd rax, rbx",
        "imul rax, rbx",
        "sqrtsd xmm0, xmm1",
        "cvtsi2sd xmm0, rax",
        "andpd xmm0, xmm1",
        "comiss xmm0, xmm1",
        "movd xmm0, rax",
        "punpcklqdq xmm0, xmm1",
        "movzx_b rax, rbx",
        // 家族展开（SSE）
        "addsd xmm0, xmm1",
        "addss xmm0, xmm1",
        "movaps xmm0, xmm1",
        "addpd xmm0, xmm1",
        "paddd xmm0, xmm1",
        // VEX 三操作数（asm 模板重排 dest, src1, src2）
        "vaddps xmm0, xmm1, xmm2",
        "vaddpd xmm0, xmm1, xmm2",
        "vpxor xmm0, xmm1, xmm2",
        // VEX 无源
        "vmovaps xmm0, xmm1",
        "vbroadcastss xmm0, xmm1",
        // VEX imm
        "vextractf128 xmm1, xmm2, 0",
        "vinsertf128 xmm0, xmm1, xmm2, 1",
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
        disassemble(&assemble("movrr RAX, RBX").unwrap()),
        "movrr RAX, RBX"
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
