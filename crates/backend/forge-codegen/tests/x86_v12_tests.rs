//! x86-64 v12 验证（迭代 3+：变长语义键）。v11 后端已删除，golden 全部为
//! 硬编码 x86 规范 oracle 字节（v12 组内寄存器索引，无 v11 的 `16+i` 偏差）。
//!
//! - **规范字节**：GPR/imm32/内存/VEX/SSE 指令按 x86 规范硬编码 oracle
//!   （v11 对比曾验证 v12 与其 GPR 字节一致；SSE 字节 v11 带多余 REX 不合规，
//!   v12 组内索引规范正确——以规范为准）。
//! - **字节级往返**：decode(encode(X)) 再 encode 字节一致（含编码撞车的别名
//!   指令，声明序首匹配）。
//! - **assemble/disassemble** 往返（opsize 操作数非文本，默认 64）。

use forge_codegen::x86_v12::{Inst, MemRef, Reg, assemble, decode, disassemble, encode};
use forge_ir::{PhysReg, RegClass};

fn v12_bytes(asm: &str) -> Vec<u8> {
    let inst = assemble(asm).unwrap_or_else(|e| panic!("v12 assemble `{asm}`: {e}"));
    encode(&inst).unwrap_or_else(|e| panic!("v12 encode {inst:?}: {e}"))
}

// ─────────────────── 规范字节：GPR @modrm/@modrm_imm32/内存 ───────────────────

#[test]
fn golden_gpr_spec_bytes() {
    let cases: &[(&str, &[u8])] = &[
        // v13：movrr/mov32/mov64rr 已合并为 `mov`（类型签名自动分发）。
        // mov EAX,EBX → MOV_R_RM（8B，32 位无前缀）；mov RAX,RBX → MOV_RM_R（89）。
        ("mov EAX, EBX", &[0x8B, 0xC3]),
        ("mov R8D, R9D", &[0x45, 0x8B, 0xC1]), // REX.R/B 扩展（32 位无 W）
        ("mov RAX, RBX", &[0x48, 0x89, 0xD8]), // 0x89 MR 方向
        ("mov R8, R9", &[0x4D, 0x89, 0xC8]),
        ("movsxd RAX, RBX", &[0x48, 0x63, 0xC3]),
        ("add RAX, RBX", &[0x48, 0x01, 0xD8]),
        ("sub RAX, RBX", &[0x48, 0x29, 0xD8]),
        ("imul RAX, RBX", &[0x48, 0x0F, 0xAF, 0xC3]),
        ("xor RAX, RBX", &[0x48, 0x31, 0xD8]),
        ("and RAX, RBX", &[0x48, 0x21, 0xD8]),
        ("or RAX, RBX", &[0x48, 0x09, 0xD8]),
        ("cmp RAX, RBX", &[0x48, 0x39, 0xD8]),
        ("test RAX, RBX", &[0x48, 0x85, 0xD8]),
        ("cmpxchg RAX, RBX", &[0x48, 0x0F, 0xB1, 0xD8]),
        ("xadd RAX, RBX", &[0x48, 0x0F, 0xC1, 0xD8]),
        ("bt RAX, RBX", &[0x48, 0x0F, 0xA3, 0xD8]),
        ("bts RAX, RBX", &[0x48, 0x0F, 0xAB, 0xD8]),
        ("btr RAX, RBX", &[0x48, 0x0F, 0xB3, 0xD8]),
        ("btc RAX, RBX", &[0x48, 0x0F, 0xBB, 0xD8]),
        ("mov RAX, RBX", &[0x48, 0x89, 0xD8]), // movrm 已合并进 mov（真重复）
        // imm32 形式（81 /digit + imm32）
        ("add RAX, 42", &[0x48, 0x81, 0xC0, 0x2A, 0x00, 0x00, 0x00]),
        ("sub RAX, 42", &[0x48, 0x81, 0xE8, 0x2A, 0x00, 0x00, 0x00]),
        ("and RAX, 42", &[0x48, 0x81, 0xE0, 0x2A, 0x00, 0x00, 0x00]),
        ("or RAX, 42", &[0x48, 0x81, 0xC8, 0x2A, 0x00, 0x00, 0x00]),
        ("xor RAX, 42", &[0x48, 0x81, 0xF0, 0x2A, 0x00, 0x00, 0x00]),
        ("cmp RAX, 42", &[0x48, 0x81, 0xF8, 0x2A, 0x00, 0x00, 0x00]),
        ("add R8, 42", &[0x49, 0x81, 0xC0, 0x2A, 0x00, 0x00, 0x00]), // 扩展寄存器
        // 内存寻址（@modrm_mem）
        ("xadd [RAX], RBX", &[0xF0, 0x48, 0x0F, 0xC1, 0x18]),
        ("xadd [R8], R9", &[0xF0, 0x4D, 0x0F, 0xC1, 0x08]),
        ("xchg [RAX], RBX", &[0x48, 0x87, 0x18]),
        ("xchg [RSP], RBX", &[0x48, 0x87, 0x1C, 0x24]), // base=RSP → SIB
        // 注：lock sub/lock and/lock or/lock xor 不在 golden——v11 带多余 0F escape
        //（F0 48 0F 29 非规范），v12 规范修正（F0 48 29），见 mem_spec_bytes。
        ("mov RAX, [RBX]", &[0x48, 0x8B, 0x03]),
        ("mov [RAX], RBX", &[0x48, 0x89, 0x18]),
    ];
    for (c, expected) in cases {
        let got = v12_bytes(c);
        assert_eq!(got.as_slice(), *expected, "spec mismatch for `{c}`");
    }
}

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
            dest: Reg::from_index(*dest, *gpr),
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

#[test]
fn r_forms_spec_bytes() {
    // 规范 oracle：+r 编码（REX 仅扩展寄存器；mov_imm64/bswap 恒 REX.W）
    let cases: &[(&str, &[u8])] = &[
        ("push RAX", &[0x50]),
        ("push R8", &[0x41, 0x50]),
        ("pop RBX", &[0x5B]),
        ("pop R9", &[0x41, 0x59]),
        ("bswap RAX", &[0x48, 0x0F, 0xC8]),
        ("bswap R12", &[0x49, 0x0F, 0xCC]),
        (
            "mov RAX, 0x1234",
            &[0x48, 0xB8, 0x34, 0x12, 0, 0, 0, 0, 0, 0],
        ),
        (
            "mov R8, -1",
            &[0x49, 0xB8, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF],
        ),
    ];
    for (asm, expected) in cases {
        let got = v12_bytes(asm);
        assert_eq!(got.as_slice(), *expected, "+r spec mismatch for `{asm}`");
    }
}

// ─────────────────── 控制流（JMP/CALL/RET，迭代 6）───────────────────

#[test]
fn control_flow_spec_bytes() {
    let cases: &[(&str, &[u8])] = &[
        // ret — C3
        ("ret", &[0xC3]),
        // jmp rel32 — E9 + rel（直接编码目标值）
        ("jmp 0", &[0xE9, 0x00, 0x00, 0x00, 0x00]),
        ("jmp 42", &[0xE9, 0x2A, 0x00, 0x00, 0x00]),
        // call rel32 — E8 + rel
        ("call 0", &[0xE8, 0x00, 0x00, 0x00, 0x00]),
        ("call -1", &[0xE8, 0xFF, 0xFF, 0xFF, 0xFF]),
    ];
    for (asm, expected) in cases {
        let got = v12_bytes(asm);
        assert_eq!(
            got.as_slice(),
            *expected,
            "control-flow spec mismatch for `{asm}`"
        );
    }
}

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

#[test]
fn sse_spec_bytes() {
    // v12 组内索引 → 规范编码（无多余 REX）
    let cases: &[(&str, &[u8])] = &[
        ("sqrtsd XMM0, XMM1", &[0xF2, 0x0F, 0x51, 0xC1]),
        ("sqrtss XMM0, XMM1", &[0x0F, 0x51, 0xC1]),
        ("cvtsi2sd XMM0, RAX", &[0xF2, 0x48, 0x0F, 0x2A, 0xC0]), // v13 合并：RAX 64 位源 → REX.W
        ("cvtsd2si RAX, XMM0", &[0xF2, 0x48, 0x0F, 0x2D, 0xC0]), // 64 位目的 → REX.W
        ("cvttsd2si RAX, XMM0", &[0xF2, 0x48, 0x0F, 0x2C, 0xC0]),
        ("cvtsd2ss XMM0, XMM1", &[0xF2, 0x0F, 0x5A, 0xC1]),
        ("cvtss2sd XMM0, XMM1", &[0xF3, 0x0F, 0x5A, 0xC1]),
        ("andpd XMM0, XMM1", &[0x66, 0x0F, 0x54, 0xC1]),
        ("xorpd XMM0, XMM1", &[0x66, 0x0F, 0x57, 0xC1]),
        ("comisd XMM0, XMM1", &[0x66, 0x0F, 0x2F, 0xC1]),
        ("comiss XMM0, XMM1", &[0x0F, 0x2F, 0xC1]),
        ("cvtsi2ss XMM0, RAX", &[0xF3, 0x48, 0x0F, 0x2A, 0xC0]), // v13 合并：RAX 64 位源 → REX.W
        // v13 合并自动分发：32 位操作数 → 无 REX.W
        ("cvtsi2sd XMM0, EAX", &[0xF2, 0x0F, 0x2A, 0xC0]),
        ("cvtsi2ss XMM0, EAX", &[0xF3, 0x0F, 0x2A, 0xC0]),
        ("cvtsd2si EAX, XMM0", &[0xF2, 0x0F, 0x2D, 0xC0]),
        ("cvttsd2si EAX, XMM0", &[0xF2, 0x0F, 0x2C, 0xC0]),
        ("movd XMM0, RAX", &[0x66, 0x0F, 0x6E, 0xC0]),
        ("movd RAX, XMM0", &[0x66, 0x0F, 0x7E, 0xC0]),
        ("punpckldq XMM0, XMM1", &[0x66, 0x0F, 0x62, 0xC1]),
        ("punpcklqdq XMM0, XMM1", &[0x66, 0x0F, 0x6C, 0xC1]),
        ("punpckhdq XMM0, XMM1", &[0x66, 0x0F, 0x6A, 0xC1]),
        ("movss XMM0, XMM1", &[0xF3, 0x0F, 0x10, 0xC1]),
        ("movsd XMM0, XMM1", &[0xF2, 0x0F, 0x10, 0xC1]),
        // 扩展 XMM → REX.R/B
        ("sqrtsd XMM8, XMM9", &[0xF2, 0x45, 0x0F, 0x51, 0xC1]),
        // GPR-字段的 SSE_RR 形式（合并后的 movzx：8 位源 AL → 48 B6、16 位源 AX → 66 48 B7）
        ("movzx RAX, AL", &[0x48, 0x0F, 0xB6, 0xC0]),
        ("movzx RAX, AX", &[0x66, 0x48, 0x0F, 0xB7, 0xC0]),
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
        // xchg [RAX], RBX — 48 87 /r（mod=00）
        ("xchg [RAX], RBX", &[0x48, 0x87, 0x18]),
        // xchg [rsp], RBX — base=RSP → SIB（index=4 无 index）
        ("xchg [RSP], RBX", &[0x48, 0x87, 0x1C, 0x24]),
        // xadd [RAX], RBX — F0 LOCK + 48 0F C1 /r
        ("xadd [RAX], RBX", &[0xF0, 0x48, 0x0F, 0xC1, 0x18]),
        // lock sub [RAX], RBX — F0 48 29 /r（v11 带多余 0F escape 非规范，v12 修正）
        ("lock sub [RAX], RBX", &[0xF0, 0x48, 0x29, 0x18]),
        // mov RAX, [RBX] — 48 8B /r
        ("mov RAX, [RBX]", &[0x48, 0x8B, 0x03]),
        // mov [RAX], RBX — 48 89 /r
        ("mov [RAX], RBX", &[0x48, 0x89, 0x18]),
        // movsd XMM0, [RAX] — F2 48?? 不——F2 0F 10（64 位无 REX.W；base=RAX<8）
        ("movsd XMM0, [RAX]", &[0xF2, 0x0F, 0x10, 0x00]),
        // mov RAX, RBX — 48 89 /r（reg=src: 3, rm=dest: 0；mov64rr 已合并）
        ("mov RAX, RBX", &[0x48, 0x89, 0xD8]),
        // mov RAX, [rbp+8] — RBP ∈ force_disp_base → mod=01 + disp8
        ("mov RAX, [RBP+8]", &[0x48, 0x8B, 0x45, 0x08]),
        // mov RAX, [RBX-8] — mod=01 + disp8=-8
        ("mov RAX, [RBX-8]", &[0x48, 0x8B, 0x43, 0xF8]),
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
        // vextractf128 XMM1, ymm2, 0：reg=src(2), rm=dest(1), imm8
        (
            "vextractf128 XMM1, XMM2, 0",
            &[0xC4, 0xE3, 0x7D, 0x19, 0xD1, 0x00],
        ),
        // vinsertf128 ymm0, ymm1, XMM2, 1：reg=0, rm=2, vvvv=~1=0xE
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
            dest: Reg::from_index(0, forge_ir::RegClass::GPR64),
            src: Reg::from_index(1, forge_ir::RegClass::GPR64),
        },
        MovRRm {
            dest: Reg::from_index(8, forge_ir::RegClass::GPR64),
            src: Reg::from_index(9, forge_ir::RegClass::GPR64),
        },
        MovRRm {
            dest: Reg::from_index(0, forge_ir::RegClass::GPR64),
            src: Reg::from_index(1, forge_ir::RegClass::GPR64),
        },
        MovRRm {
            dest: Reg::from_index(0, forge_ir::RegClass::GPR64),
            src: Reg::from_index(1, forge_ir::RegClass::GPR64),
        },
        MovRRm {
            dest: Reg::from_index(2, forge_ir::RegClass::GPR64),
            src: Reg::from_index(3, forge_ir::RegClass::GPR64),
        },
        MovsxdRRm {
            dest: Reg::from_index(0, forge_ir::RegClass::GPR64),
            src: Reg::from_index(1, forge_ir::RegClass::GPR64),
        },
        AddRmR {
            src: Reg::from_index(0, forge_ir::RegClass::GPR64),
            dest: Reg::from_index(1, forge_ir::RegClass::GPR64),
        },
        SubRmR {
            src: Reg::from_index(0, forge_ir::RegClass::GPR64),
            dest: Reg::from_index(1, forge_ir::RegClass::GPR64),
        },
        ImulRRm {
            dest: Reg::from_index(0, forge_ir::RegClass::GPR64),
            src: Reg::from_index(1, forge_ir::RegClass::GPR64),
        },
        XorRmR {
            src: Reg::from_index(0, forge_ir::RegClass::GPR64),
            dest: Reg::from_index(1, forge_ir::RegClass::GPR64),
        },
        AndRmR {
            src: Reg::from_index(0, forge_ir::RegClass::GPR64),
            dest: Reg::from_index(1, forge_ir::RegClass::GPR64),
        },
        OrRmR {
            src: Reg::from_index(0, forge_ir::RegClass::GPR64),
            dest: Reg::from_index(1, forge_ir::RegClass::GPR64),
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
            dest: Reg::from_index(0, forge_ir::RegClass::GPR64),
            src: Reg::from_index(1, forge_ir::RegClass::GPR(1)),
        },
        MovzxR16Rm {
            dest: Reg::from_index(0, forge_ir::RegClass::GPR64),
            src: Reg::from_index(1, forge_ir::RegClass::GPR(2)),
        },
        MovsxR8Rm {
            dest: Reg::from_index(0, forge_ir::RegClass::GPR64),
            src: Reg::from_index(1, forge_ir::RegClass::GPR(1)),
        },
        MovsxR16Rm {
            dest: Reg::from_index(0, forge_ir::RegClass::GPR64),
            src: Reg::from_index(1, forge_ir::RegClass::GPR(2)),
        },
        CmpxchgRmR {
            src: Reg::from_index(0, forge_ir::RegClass::GPR64),
            dest: Reg::from_index(1, forge_ir::RegClass::GPR64),
        },
        XaddRmR {
            src: Reg::from_index(0, forge_ir::RegClass::GPR64),
            dest: Reg::from_index(1, forge_ir::RegClass::GPR64),
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
            dest: Reg::from_index(1, forge_ir::RegClass::GPR64),
        },
        MovR8Rm64 {
            dest: Reg::from_index(0, forge_ir::RegClass::GPR64),
            src: Reg::from_index(1, forge_ir::RegClass::GPR64),
        },
        // @modrm_imm32
        AddRImm32 {
            dest: Reg::from_index(0, forge_ir::RegClass::GPR64),
            imm: 42,
        },
        AddRImm32 {
            dest: Reg::from_index(8, forge_ir::RegClass::GPR64),
            imm: -1,
        },
        OrRImm32 {
            dest: Reg::from_index(0, forge_ir::RegClass::GPR64),
            imm: 42,
        },
        AndRImm32 {
            dest: Reg::from_index(0, forge_ir::RegClass::GPR64),
            imm: 42,
        },
        SubRImm32 {
            dest: Reg::from_index(0, forge_ir::RegClass::GPR64),
            imm: 42,
        },
        XorRImm32 {
            dest: Reg::from_index(0, forge_ir::RegClass::GPR64),
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
            dest: Reg::from_index(0, forge_ir::RegClass::GPR64),
            src: Reg::from_index(1, forge_ir::RegClass::GPR(1)),
        },
        MovzxR16Rm {
            dest: Reg::from_index(0, forge_ir::RegClass::GPR64),
            src: Reg::from_index(1, forge_ir::RegClass::GPR(2)),
        },
        Sqrtsd {
            dest: Reg::from_index(0, forge_ir::RegClass::FPR(16)),
            src: Reg::from_index(1, forge_ir::RegClass::FPR(16)),
        },
        Sqrtsd {
            dest: Reg::from_index(6, forge_ir::RegClass::FPR(16)),
            src: Reg::from_index(7, forge_ir::RegClass::FPR(16)),
        },
        Sqrtss {
            dest: Reg::from_index(0, forge_ir::RegClass::FPR(16)),
            src: Reg::from_index(1, forge_ir::RegClass::FPR(16)),
        },
        Cvtsi2sd {
            dest: Reg::from_index(0, forge_ir::RegClass::FPR(16)),
            src: Reg::from_index(1, forge_ir::RegClass::GPR64),
        },
        Cvtsd2si {
            dest: Reg::from_index(0, forge_ir::RegClass::GPR64),
            src: Reg::from_index(1, forge_ir::RegClass::FPR(16)),
        },
        Cvttsd2si {
            dest: Reg::from_index(0, forge_ir::RegClass::GPR64),
            src: Reg::from_index(1, forge_ir::RegClass::FPR(16)),
        },
        Cvtsd2ss {
            dest: Reg::from_index(0, forge_ir::RegClass::FPR(16)),
            src: Reg::from_index(1, forge_ir::RegClass::FPR(16)),
        },
        Cvtss2sd {
            dest: Reg::from_index(0, forge_ir::RegClass::FPR(16)),
            src: Reg::from_index(1, forge_ir::RegClass::FPR(16)),
        },
        Andpd {
            dest: Reg::from_index(0, forge_ir::RegClass::FPR(16)),
            src: Reg::from_index(1, forge_ir::RegClass::FPR(16)),
        },
        Xorpd {
            dest: Reg::from_index(0, forge_ir::RegClass::FPR(16)),
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
            dest: Reg::from_index(0, forge_ir::RegClass::FPR(16)),
            src: Reg::from_index(1, forge_ir::RegClass::FPR(16)),
        },
        Cvtsi2ss {
            dest: Reg::from_index(0, forge_ir::RegClass::FPR(16)),
            src: Reg::from_index(1, forge_ir::RegClass::GPR64),
        },
        MovdFregIreg {
            dest: Reg::from_index(0, forge_ir::RegClass::FPR(16)),
            src: Reg::from_index(1, forge_ir::RegClass::GPR64),
        },
        MovdIregFreg {
            src: Reg::from_index(0, forge_ir::RegClass::FPR(16)),
            dest: Reg::from_index(1, forge_ir::RegClass::GPR64),
        },
        Punpckldq {
            dest: Reg::from_index(0, forge_ir::RegClass::FPR(16)),
            src: Reg::from_index(1, forge_ir::RegClass::FPR(16)),
        },
        Punpcklqdq {
            dest: Reg::from_index(0, forge_ir::RegClass::FPR(16)),
            src: Reg::from_index(1, forge_ir::RegClass::FPR(16)),
        },
        Punpckhdq {
            dest: Reg::from_index(0, forge_ir::RegClass::FPR(16)),
            src: Reg::from_index(1, forge_ir::RegClass::FPR(16)),
        },
        Movss {
            dest: Reg::from_index(0, forge_ir::RegClass::FPR(16)),
            src: Reg::from_index(1, forge_ir::RegClass::FPR(16)),
        },
        Movsd {
            dest: Reg::from_index(0, forge_ir::RegClass::FPR(16)),
            src: Reg::from_index(1, forge_ir::RegClass::FPR(16)),
        },
        // @modrm_mem（迭代 3b）
        XaddMemR {
            dest: Reg::from_index(1, forge_ir::RegClass::GPR64),
            src: Reg::from_index(2, forge_ir::RegClass::GPR64),
        },
        XaddMemR {
            dest: Reg::from_index(8, forge_ir::RegClass::GPR64),
            src: Reg::from_index(9, forge_ir::RegClass::GPR64),
        },
        XchgMemR {
            dest: Reg::from_index(1, forge_ir::RegClass::GPR64),
            src: Reg::from_index(2, forge_ir::RegClass::GPR64),
        },
        XchgMemR {
            dest: Reg::from_index(1, forge_ir::RegClass::GPR64),
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
            dest: Reg::from_index(0, forge_ir::RegClass::GPR64),
            src: Reg::from_index(2, forge_ir::RegClass::GPR64),
        },
        StoreMemR {
            src: Reg::from_index(1, forge_ir::RegClass::GPR64),
            src2: Reg::from_index(2, forge_ir::RegClass::GPR64),
        },
        MovsdRMem {
            dest: Reg::from_index(0, forge_ir::RegClass::FPR(16)),
            src: Reg::from_index(2, forge_ir::RegClass::GPR64),
        },
        MovsdMemR {
            src: Reg::from_index(0, forge_ir::RegClass::FPR(16)),
            src2: Reg::from_index(2, forge_ir::RegClass::GPR64),
        },
        Mov64Rr {
            src: Reg::from_index(1, forge_ir::RegClass::GPR64),
            dest: Reg::from_index(0, forge_ir::RegClass::GPR64),
        },
        Mov64Rm {
            dest: Reg::from_index(0, forge_ir::RegClass::GPR64),
            mem: MemRef {
                base: Reg::from_index(2, forge_ir::RegClass::GPR64),
                disp: 0,
                index: None,
                scale: 1,
            },
        },
        Mov64Rm {
            dest: Reg::from_index(8, forge_ir::RegClass::GPR64),
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
            dest: Reg::from_index(0, forge_ir::RegClass::FPR(16)),
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
            dest: Reg::from_index(0, forge_ir::RegClass::FPR(16)),
            src: Reg::from_index(1, forge_ir::RegClass::FPR(16)),
        },
        Minsd {
            dest: Reg::from_index(6, forge_ir::RegClass::FPR(16)),
            src: Reg::from_index(7, forge_ir::RegClass::FPR(16)),
        },
        Maxsd {
            dest: Reg::from_index(0, forge_ir::RegClass::FPR(16)),
            src: Reg::from_index(1, forge_ir::RegClass::FPR(16)),
        },
        Addsd {
            dest: Reg::from_index(0, forge_ir::RegClass::FPR(16)),
            src: Reg::from_index(1, forge_ir::RegClass::FPR(16)),
        },
        Subsd {
            dest: Reg::from_index(0, forge_ir::RegClass::FPR(16)),
            src: Reg::from_index(1, forge_ir::RegClass::FPR(16)),
        },
        Mulsd {
            dest: Reg::from_index(0, forge_ir::RegClass::FPR(16)),
            src: Reg::from_index(1, forge_ir::RegClass::FPR(16)),
        },
        Divsd {
            dest: Reg::from_index(0, forge_ir::RegClass::FPR(16)),
            src: Reg::from_index(1, forge_ir::RegClass::FPR(16)),
        },
        Addss {
            dest: Reg::from_index(0, forge_ir::RegClass::FPR(16)),
            src: Reg::from_index(1, forge_ir::RegClass::FPR(16)),
        },
        Subss {
            dest: Reg::from_index(0, forge_ir::RegClass::FPR(16)),
            src: Reg::from_index(1, forge_ir::RegClass::FPR(16)),
        },
        Mulss {
            dest: Reg::from_index(0, forge_ir::RegClass::FPR(16)),
            src: Reg::from_index(1, forge_ir::RegClass::FPR(16)),
        },
        Divss {
            dest: Reg::from_index(0, forge_ir::RegClass::FPR(16)),
            src: Reg::from_index(1, forge_ir::RegClass::FPR(16)),
        },
        Movaps {
            dest: Reg::from_index(0, forge_ir::RegClass::FPR(16)),
            src: Reg::from_index(1, forge_ir::RegClass::FPR(16)),
        },
        Addps {
            dest: Reg::from_index(0, forge_ir::RegClass::FPR(16)),
            src: Reg::from_index(1, forge_ir::RegClass::FPR(16)),
        },
        Subps {
            dest: Reg::from_index(0, forge_ir::RegClass::FPR(16)),
            src: Reg::from_index(1, forge_ir::RegClass::FPR(16)),
        },
        Mulps {
            dest: Reg::from_index(0, forge_ir::RegClass::FPR(16)),
            src: Reg::from_index(1, forge_ir::RegClass::FPR(16)),
        },
        Divps {
            dest: Reg::from_index(0, forge_ir::RegClass::FPR(16)),
            src: Reg::from_index(1, forge_ir::RegClass::FPR(16)),
        },
        Xorps {
            dest: Reg::from_index(0, forge_ir::RegClass::FPR(16)),
            src: Reg::from_index(1, forge_ir::RegClass::FPR(16)),
        },
        Andps {
            dest: Reg::from_index(0, forge_ir::RegClass::FPR(16)),
            src: Reg::from_index(1, forge_ir::RegClass::FPR(16)),
        },
        Orps {
            dest: Reg::from_index(0, forge_ir::RegClass::FPR(16)),
            src: Reg::from_index(1, forge_ir::RegClass::FPR(16)),
        },
        Addpd {
            dest: Reg::from_index(0, forge_ir::RegClass::FPR(16)),
            src: Reg::from_index(1, forge_ir::RegClass::FPR(16)),
        },
        Subpd {
            dest: Reg::from_index(0, forge_ir::RegClass::FPR(16)),
            src: Reg::from_index(1, forge_ir::RegClass::FPR(16)),
        },
        Mulpd {
            dest: Reg::from_index(0, forge_ir::RegClass::FPR(16)),
            src: Reg::from_index(1, forge_ir::RegClass::FPR(16)),
        },
        Divpd {
            dest: Reg::from_index(0, forge_ir::RegClass::FPR(16)),
            src: Reg::from_index(1, forge_ir::RegClass::FPR(16)),
        },
        Paddd {
            dest: Reg::from_index(0, forge_ir::RegClass::FPR(16)),
            src: Reg::from_index(1, forge_ir::RegClass::FPR(16)),
        },
        Psubd {
            dest: Reg::from_index(0, forge_ir::RegClass::FPR(16)),
            src: Reg::from_index(1, forge_ir::RegClass::FPR(16)),
        },
        Paddq {
            dest: Reg::from_index(0, forge_ir::RegClass::FPR(16)),
            src: Reg::from_index(1, forge_ir::RegClass::FPR(16)),
        },
        Psubq {
            dest: Reg::from_index(0, forge_ir::RegClass::FPR(16)),
            src: Reg::from_index(1, forge_ir::RegClass::FPR(16)),
        },
        // VEX（三操作数/无源/imm）
        Vaddps {
            dest: Reg::from_index(0, forge_ir::RegClass::FPR(16)),
            src: Reg::from_index(2, forge_ir::RegClass::FPR(16)),
            src2: Reg::from_index(1, forge_ir::RegClass::FPR(16)),
        },
        Vaddps {
            dest: Reg::from_index(5, forge_ir::RegClass::FPR(16)),
            src: Reg::from_index(6, forge_ir::RegClass::FPR(16)),
            src2: Reg::from_index(7, forge_ir::RegClass::FPR(16)),
        },
        Vsubps {
            dest: Reg::from_index(0, forge_ir::RegClass::FPR(16)),
            src: Reg::from_index(2, forge_ir::RegClass::FPR(16)),
            src2: Reg::from_index(1, forge_ir::RegClass::FPR(16)),
        },
        Vmulps {
            dest: Reg::from_index(0, forge_ir::RegClass::FPR(16)),
            src: Reg::from_index(2, forge_ir::RegClass::FPR(16)),
            src2: Reg::from_index(1, forge_ir::RegClass::FPR(16)),
        },
        Vdivps {
            dest: Reg::from_index(0, forge_ir::RegClass::FPR(16)),
            src: Reg::from_index(2, forge_ir::RegClass::FPR(16)),
            src2: Reg::from_index(1, forge_ir::RegClass::FPR(16)),
        },
        Vxorps {
            dest: Reg::from_index(0, forge_ir::RegClass::FPR(16)),
            src: Reg::from_index(2, forge_ir::RegClass::FPR(16)),
            src2: Reg::from_index(1, forge_ir::RegClass::FPR(16)),
        },
        Vandps {
            dest: Reg::from_index(0, forge_ir::RegClass::FPR(16)),
            src: Reg::from_index(2, forge_ir::RegClass::FPR(16)),
            src2: Reg::from_index(1, forge_ir::RegClass::FPR(16)),
        },
        Vaddpd {
            dest: Reg::from_index(0, forge_ir::RegClass::FPR(16)),
            src: Reg::from_index(2, forge_ir::RegClass::FPR(16)),
            src2: Reg::from_index(1, forge_ir::RegClass::FPR(16)),
        },
        Vsubpd {
            dest: Reg::from_index(0, forge_ir::RegClass::FPR(16)),
            src: Reg::from_index(2, forge_ir::RegClass::FPR(16)),
            src2: Reg::from_index(1, forge_ir::RegClass::FPR(16)),
        },
        Vmulpd {
            dest: Reg::from_index(0, forge_ir::RegClass::FPR(16)),
            src: Reg::from_index(2, forge_ir::RegClass::FPR(16)),
            src2: Reg::from_index(1, forge_ir::RegClass::FPR(16)),
        },
        Vdivpd {
            dest: Reg::from_index(0, forge_ir::RegClass::FPR(16)),
            src: Reg::from_index(2, forge_ir::RegClass::FPR(16)),
            src2: Reg::from_index(1, forge_ir::RegClass::FPR(16)),
        },
        Vmovaps {
            dest: Reg::from_index(0, forge_ir::RegClass::FPR(16)),
            src: Reg::from_index(1, forge_ir::RegClass::FPR(16)),
        },
        Vbroadcastss {
            dest: Reg::from_index(0, forge_ir::RegClass::FPR(16)),
            src: Reg::from_index(1, forge_ir::RegClass::FPR(16)),
        },
        Vbroadcastsd {
            dest: Reg::from_index(0, forge_ir::RegClass::FPR(16)),
            src: Reg::from_index(1, forge_ir::RegClass::FPR(16)),
        },
        Vpxor {
            dest: Reg::from_index(0, forge_ir::RegClass::FPR(16)),
            src: Reg::from_index(2, forge_ir::RegClass::FPR(16)),
            src2: Reg::from_index(1, forge_ir::RegClass::FPR(16)),
        },
        Vpaddd {
            dest: Reg::from_index(0, forge_ir::RegClass::FPR(16)),
            src: Reg::from_index(2, forge_ir::RegClass::FPR(16)),
            src2: Reg::from_index(1, forge_ir::RegClass::FPR(16)),
        },
        Vpsubd {
            dest: Reg::from_index(0, forge_ir::RegClass::FPR(16)),
            src: Reg::from_index(2, forge_ir::RegClass::FPR(16)),
            src2: Reg::from_index(1, forge_ir::RegClass::FPR(16)),
        },
        Vpaddq {
            dest: Reg::from_index(0, forge_ir::RegClass::FPR(16)),
            src: Reg::from_index(2, forge_ir::RegClass::FPR(16)),
            src2: Reg::from_index(1, forge_ir::RegClass::FPR(16)),
        },
        Vpsubq {
            dest: Reg::from_index(0, forge_ir::RegClass::FPR(16)),
            src: Reg::from_index(2, forge_ir::RegClass::FPR(16)),
            src2: Reg::from_index(1, forge_ir::RegClass::FPR(16)),
        },
        Vpmulld {
            dest: Reg::from_index(0, forge_ir::RegClass::FPR(16)),
            src: Reg::from_index(2, forge_ir::RegClass::FPR(16)),
            src2: Reg::from_index(1, forge_ir::RegClass::FPR(16)),
        },
        Vextractf128 {
            src: Reg::from_index(2, forge_ir::RegClass::FPR(16)),
            dest: Reg::from_index(0, forge_ir::RegClass::FPR(16)),
            imm: 0,
        },
        Vinsertf128 {
            dest: Reg::from_index(0, forge_ir::RegClass::FPR(16)),
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
            dest: Reg::from_index(3, forge_ir::RegClass::GPR64),
        },
        Pop {
            dest: Reg::from_index(9, forge_ir::RegClass::GPR64),
        },
        MovRegImm64 {
            dest: Reg::from_index(0, forge_ir::RegClass::GPR64),
            imm: 0x1234,
        },
        MovRegImm64 {
            dest: Reg::from_index(8, forge_ir::RegClass::GPR64),
            imm: -1,
        },
        BswapR {
            dest: Reg::from_index(0, forge_ir::RegClass::GPR64),
        },
        BswapR {
            dest: Reg::from_index(12, forge_ir::RegClass::GPR64),
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
        JccRel32 { cond: 4, target: 0 },
        JccRel32 {
            cond: 15,
            target: -1,
        },
        SetccRm8 {
            dest: Reg::from_index(2, forge_ir::RegClass::GPR64),
            cond: 5,
        },
        LeaR64Sib {
            dest: Reg::from_index(0, forge_ir::RegClass::GPR64),
            mem: MemRef {
                base: Reg::from_index(5, forge_ir::RegClass::GPR64),
                disp: 8,
                index: None,
                scale: 1,
            },
        },
        LeaRbpOff {
            dest: Reg::from_index(0, forge_ir::RegClass::GPR64),
            mem: MemRef {
                base: Reg::from_index(5, forge_ir::RegClass::GPR64),
                disp: -8,
                index: None,
                scale: 1,
            },
        },
        CmovccRRm {
            dest: Reg::from_index(1, forge_ir::RegClass::GPR64),
            src: Reg::from_index(2, forge_ir::RegClass::GPR64),
            cond: 5,
        },
        RoundsdI {
            dest: Reg::from_index(0, forge_ir::RegClass::FPR(16)),
            src: Reg::from_index(1, forge_ir::RegClass::FPR(16)),
            imm: 3,
        },
        Pmulld {
            dest: Reg::from_index(0, forge_ir::RegClass::FPR(16)),
            src: Reg::from_index(1, forge_ir::RegClass::FPR(16)),
        },
        MovqXmmR64 {
            dest: Reg::from_index(0, forge_ir::RegClass::FPR(16)),
            src: Reg::from_index(1, forge_ir::RegClass::GPR64),
        },
        MovqR64Xmm {
            dest: Reg::from_index(1, forge_ir::RegClass::GPR64),
            src: Reg::from_index(0, forge_ir::RegClass::FPR(16)),
        },
        MovapsMr {
            src: Reg::from_index(1, forge_ir::RegClass::GPR64),
            src2: Reg::from_index(0, forge_ir::RegClass::FPR(16)),
        },
        LzcntR {
            dest: Reg::from_index(0, forge_ir::RegClass::GPR64),
            src: Reg::from_index(1, forge_ir::RegClass::GPR64),
        },
        TzcntR {
            dest: Reg::from_index(0, forge_ir::RegClass::GPR64),
            src: Reg::from_index(1, forge_ir::RegClass::GPR64),
        },
        PopcntR {
            dest: Reg::from_index(0, forge_ir::RegClass::GPR64),
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
            dest: Reg::from_index(0, forge_ir::RegClass::GPR64),
            src: Reg::from_index(1, forge_ir::RegClass::GPR64),
        },
        Inst::MovRRm {
            dest: Reg::from_index(8, forge_ir::RegClass::GPR64),
            src: Reg::from_index(9, forge_ir::RegClass::GPR64),
        },
        Inst::MovsxdRRm {
            dest: Reg::from_index(0, forge_ir::RegClass::GPR64),
            src: Reg::from_index(1, forge_ir::RegClass::GPR64),
        },
        Inst::AddRmR {
            src: Reg::from_index(0, forge_ir::RegClass::GPR64),
            dest: Reg::from_index(1, forge_ir::RegClass::GPR64),
        },
        Inst::ImulRRm {
            dest: Reg::from_index(0, forge_ir::RegClass::GPR64),
            src: Reg::from_index(1, forge_ir::RegClass::GPR64),
        },
        Inst::CmpRmR {
            src: Reg::from_index(0, forge_ir::RegClass::GPR64),
            src2: Reg::from_index(1, forge_ir::RegClass::GPR64),
        },
        Inst::AddRImm32 {
            dest: Reg::from_index(0, forge_ir::RegClass::GPR64),
            imm: 42,
        },
        Inst::CmpRImm32 {
            src: Reg::from_index(8, forge_ir::RegClass::GPR64),
            imm: -1,
        },
        Inst::Sqrtsd {
            dest: Reg::from_index(0, forge_ir::RegClass::FPR(16)),
            src: Reg::from_index(1, forge_ir::RegClass::FPR(16)),
        },
        Inst::Cvtsi2sd {
            dest: Reg::from_index(0, forge_ir::RegClass::FPR(16)),
            src: Reg::from_index(1, forge_ir::RegClass::GPR64),
        },
        Inst::Andpd {
            dest: Reg::from_index(0, forge_ir::RegClass::FPR(16)),
            src: Reg::from_index(1, forge_ir::RegClass::FPR(16)),
        },
        Inst::Comiss {
            src: Reg::from_index(0, forge_ir::RegClass::FPR(16)),
            src2: Reg::from_index(1, forge_ir::RegClass::FPR(16)),
        },
        Inst::Movss {
            dest: Reg::from_index(0, forge_ir::RegClass::FPR(16)),
            src: Reg::from_index(1, forge_ir::RegClass::FPR(16)),
        },
        Inst::Punpcklqdq {
            dest: Reg::from_index(0, forge_ir::RegClass::FPR(16)),
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
