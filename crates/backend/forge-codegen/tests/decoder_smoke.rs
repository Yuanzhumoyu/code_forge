//! v12 生成解码器冒烟测试（变长语义键 + TargetMachine 组件装配）。
//!
//! 原理：TargetAssembler 把文本解析为物理化 Inst → Encoder 编码为字节 →
//! Decoder 反解 → 再编码，断言字节一致（避免 Reg::from_index 视图选择差异
//! 干扰断言）。v11 后端已删除，本测试只覆盖 x86_v12。

use forge_codegen::machine::assembler::TargetAssembler;
use forge_codegen::machine::target::TargetMachine;
use forge_codegen::x86_v12::TargetMachine as X86V12;

fn roundtrip_asm(name: &str, src: &str) {
    let tm = X86V12::new();
    let asm = forge_codegen::x86_v12::Assembler;
    let insts = asm
        .parse_insts(src)
        .unwrap_or_else(|e| panic!("{name}: parse `{src}`: {e:?}"));
    assert_eq!(insts.len(), 1, "{name}: `{src}` 应解析出 1 条指令");
    let inst = insts.into_iter().next().unwrap();
    let encoder = tm.encoder();
    let decoder = tm
        .decoder()
        .unwrap_or_else(|| panic!("{name}: TargetMachine 缺 decoder 组件"));
    let rm = forge_codegen::AllocResult::new();

    let bytes = encoder
        .encode_to_bytes(&inst, &rm)
        .unwrap_or_else(|e| panic!("{name}: encode {inst:?}: {e:?}"));
    assert!(!bytes.is_empty(), "{name}: 空编码");

    let (dec, n) = decoder
        .decode(&bytes)
        .unwrap_or_else(|e| panic!("{name}: decode {bytes:02x?}: {e:?}"));
    assert_eq!(n, bytes.len(), "{name}: 解码消费字节数");

    let bytes2 = encoder
        .encode_to_bytes(&dec, &rm)
        .unwrap_or_else(|e| panic!("{name}: re-encode {dec:?}: {e:?}"));
    assert_eq!(
        bytes2, bytes,
        "{name}: decode→encode 往返字节不一致\n  orig {bytes:02x?}\n  dec  {bytes2:02x?}\n  inst {inst:?}\n  decd {dec:?}"
    );
}

// ── x86_v12（变长语义键：ModRM/REX/opsize/SSE/VEX）──

#[test]
fn x86_modrm_roundtrip() {
    roundtrip_asm("x86_movrr", "mov RAX, RBX");
    // 高编号寄存器 → REX.R/REX.B 扩展位
    roundtrip_asm("x86_movrr_r8", "mov R8, R9");
    // 32 位视图 → MOV_R_RM（8B 方向）
    roundtrip_asm("x86_mov32", "mov EAX, EBX");
    roundtrip_asm("x86_movsxd", "movsxd RAX, RBX");
}

#[test]
fn x86_imm32_roundtrip() {
    roundtrip_asm("x86_add_imm", "add RAX, 42");
    roundtrip_asm("x86_add_imm_r8", "add R8, 42");
}

#[test]
fn x86_r_forms_roundtrip() {
    roundtrip_asm("x86_push_r12", "push R12"); // REX.B 扩展
    roundtrip_asm("x86_pop_rax", "pop RAX");
    roundtrip_asm("x86_bswap", "bswap R12");
    roundtrip_asm("x86_mov_imm", "mov_imm RAX, 0x1234");
}

#[test]
fn x86_mem_roundtrip() {
    // modrm_mem：mod=00 无位移；[RSP] → SIB
    roundtrip_asm("x86_xchg_mem", "xchg [RAX], RBX");
    roundtrip_asm("x86_xchg_mem_rsp", "xchg [RSP], RBX");
    roundtrip_asm("x86_mov_mem", "mov_mem RAX, [RBX]");
    roundtrip_asm("x86_mov_sto", "mov_sto [RAX], RBX");
}

#[test]
fn x86_sse_roundtrip() {
    // SSE：prefix + 0F escape；Freg 字段走组内索引
    roundtrip_asm("x86_sqrtsd", "sqrtsd XMM2, XMM3");
    roundtrip_asm("x86_addsd", "addsd XMM2, XMM3");
    // GPR 字段的 0F 形式（MOVZX 0F B6/B7 /r，v13 合并助记符）
    roundtrip_asm("x86_movzx_b", "movzx RAX, AL");
    roundtrip_asm("x86_movzx_w", "movzx RAX, AX");
}

#[test]
fn x86_vex_roundtrip() {
    // VEX：C4 + vex2/vex3；无源（vvvv=1111）与三操作数有源
    roundtrip_asm("x86_vmovaps", "vmovaps XMM0, XMM1");
    roundtrip_asm("x86_vaddps", "vaddps XMM0, XMM1, XMM2");
}

#[test]
fn x86_byte_mov_roundtrip() {
    // v13：8 位 mov（opcode 8A）——mov 覆盖 8/16/32/64 全宽度
    roundtrip_asm("x86_mov8", "mov AL, BL");
    roundtrip_asm("x86_mov8_high", "mov SIL, DIL"); // byte_reg → 强制 REX
    roundtrip_asm("x86_mov8_r8b", "mov R8B, AL"); // REX.R
    // 内存 store 宽度：mov_sto 数据槽 gprx auto——32 位数据（EBX）→ 无
    // REX.W 4 字节写；64 位数据（RBX）→ REX.W 8 字节写（v14：mov_sto32 已删）。
    roundtrip_asm("x86_mov_sto32", "mov_sto [EAX], EBX");
    roundtrip_asm("x86_mov_sto64", "mov_sto [RAX], RBX");
}

#[test]
fn x86_byte_mov_spec_bytes() {
    use forge_codegen::x86_v12::{assemble, encode};
    // mov al, bl → 8A C3（reg=src=bl、rm=dest=al；无 REX）
    let b = encode(&assemble("mov AL, BL").unwrap()).unwrap();
    assert_eq!(b, vec![0x8A, 0xC3]);
    // mov sil, dil → 40 8A F7（SIL=6、DIL=7；byte_reg ≥4 → 强制 REX）
    let b = encode(&assemble("mov SIL, DIL").unwrap()).unwrap();
    assert_eq!(b, vec![0x40, 0x8A, 0xF7]);
    // mov r8b, al → 44 8A C0（REX.R）
    let b = encode(&assemble("mov R8B, AL").unwrap()).unwrap();
    assert_eq!(b, vec![0x44, 0x8A, 0xC0]);
}

#[test]
fn x86_evex_roundtrip() {
    // EVEX（AVX-512）：62 + P0/P1/P2 + opcode + ModRM；ZMM reg-reg
    roundtrip_asm("x86_vaddps_zmm", "vaddps ZMM0, ZMM1, ZMM2");
    roundtrip_asm("x86_vaddps_zmm_hi", "vaddps ZMM16, ZMM17, ZMM18"); // R'/V' 第 4 位
    roundtrip_asm("x86_vsubps_zmm", "vsubps ZMM3, ZMM4, ZMM5");
    // 同助记符按操作数类分发：xmm → VEX，zmm → EVEX
    roundtrip_asm("x86_vaddps_xmm", "vaddps XMM0, XMM1, XMM2");
}

#[test]
fn x86_evex_spec_bytes() {
    use forge_codegen::x86_v12::{assemble, encode};
    // vaddps zmm0, zmm1, zmm2 → 62 F1 74 48 58 C2（mm=01、vvvv=~1、L'L=10、V'=1）
    let b = encode(&assemble("vaddps ZMM0, ZMM1, ZMM2").unwrap()).unwrap();
    assert_eq!(b, vec![0x62, 0xF1, 0x74, 0x48, 0x58, 0xC2]);
    // zmm 高半：vaddps zmm16, zmm17, zmm18 → R=0（reg bit4=1）、V'=0（src bit4=1）、
    // R'/B'=1（reg/rm bit3=0）：62 E1 74 40 58 C2
    let b = encode(&assemble("vaddps ZMM16, ZMM17, ZMM18").unwrap()).unwrap();
    assert_eq!(b, vec![0x62, 0xE1, 0x74, 0x40, 0x58, 0xC2]);
    // vaddps xmm0,... 仍走 VEX（fpr 槽，三操作数 vvvv=~src1 → 74）
    let b = encode(&assemble("vaddps XMM0, XMM1, XMM2").unwrap()).unwrap();
    assert_eq!(b, vec![0xC4, 0xE1, 0x74, 0x58, 0xC2]);
}

#[test]
fn x86_evex_mem_roundtrip() {
    // EVEX 内存形式 + 压缩位移（vmovaps zmm, [mem]，scale=64）
    roundtrip_asm("x86_vmovaps_zmm_mem", "vmovaps ZMM0, [RAX]");
    roundtrip_asm("x86_vmovaps_zmm_mem_disp", "vmovaps ZMM0, [RAX+64]"); // disp8 = 64/64
    roundtrip_asm("x86_vmovaps_zmm_mem_disp32", "vmovaps ZMM0, [RAX+32]"); // 非 64 倍数 → disp32
    roundtrip_asm("x86_vmovaps_zmm_mem_neg", "vmovaps ZMM0, [RAX-128]"); // disp8 = -2
    roundtrip_asm("x86_vmovaps_zmm_mem_rsp", "vmovaps ZMM1, [RSP]"); // SIB
}

#[test]
fn x86_evex_mem_spec_bytes() {
    use forge_codegen::x86_v12::{assemble, encode};
    // vmovaps zmm0, [rax] → 62 F1 7C 48 28 00（mod=0）
    let b = encode(&assemble("vmovaps ZMM0, [RAX]").unwrap()).unwrap();
    assert_eq!(b, vec![0x62, 0xF1, 0x7C, 0x48, 0x28, 0x00]);
    // [rax+64]：压缩位移 disp8 = 64/64 = 1 → mod=1（40）disp8=01
    let b = encode(&assemble("vmovaps ZMM0, [RAX+64]").unwrap()).unwrap();
    assert_eq!(b, vec![0x62, 0xF1, 0x7C, 0x48, 0x28, 0x40, 0x01]);
    // [rax+32]：非 64 倍数 → mod=2 disp32 不缩放（80 20 00 00 00）
    let b = encode(&assemble("vmovaps ZMM0, [RAX+32]").unwrap()).unwrap();
    assert_eq!(
        b,
        vec![0x62, 0xF1, 0x7C, 0x48, 0x28, 0x80, 0x20, 0x00, 0x00, 0x00]
    );
    // [rax-128]：disp8 = -2（FE）
    let b = encode(&assemble("vmovaps ZMM0, [RAX-128]").unwrap()).unwrap();
    assert_eq!(b, vec![0x62, 0xF1, 0x7C, 0x48, 0x28, 0x40, 0xFE]);
}

#[test]
fn x86_evex_512_roundtrip() {
    // 512 位族：无源 2 操作数（EVEX_RR）与三操作数（EVEX_RRV）往返
    roundtrip_asm("x86_vmovaps_zmm", "vmovaps ZMM0, ZMM1");
    roundtrip_asm("x86_vmovaps_zmm_hi", "vmovaps ZMM20, ZMM21"); // R'/B' 第 4 位
    roundtrip_asm("x86_vpaddq_zmm", "vpaddq ZMM0, ZMM1, ZMM2");
    roundtrip_asm("x86_vpsubq_zmm", "vpsubq ZMM3, ZMM4, ZMM5");
    roundtrip_asm("x86_vpaddd_zmm", "vpaddd ZMM6, ZMM7, ZMM8");
    roundtrip_asm("x86_vdivps_zmm", "vdivps ZMM9, ZMM10, ZMM11");
    roundtrip_asm("x86_vdivpd_zmm", "vdivpd ZMM12, ZMM13, ZMM14");
}

#[test]
fn x86_evex_512_spec_bytes() {
    use forge_codegen::x86_v12::{assemble, encode};
    // vmovaps zmm0, zmm1（无源 EVEX_RR）：62 F1 7C 48 28 C1
    // P0=R'X'B'R=1111、mm=01；P1=W0 vvvv=1111 1 pp=00 → 7C；P2=z0 L'L=10 b0 V'=1 → 48
    let b = encode(&assemble("vmovaps ZMM0, ZMM1").unwrap()).unwrap();
    assert_eq!(b, vec![0x62, 0xF1, 0x7C, 0x48, 0x28, 0xC1]);
    // vpaddq zmm0, zmm1, zmm2（W=1、pp=1）：P1 = 1 1110 1 01 → F5
    let b = encode(&assemble("vpaddq ZMM0, ZMM1, ZMM2").unwrap()).unwrap();
    assert_eq!(b, vec![0x62, 0xF1, 0xF5, 0x48, 0xD4, 0xC2]);
    // vpsubq（W=1）：62 F1 F5 48 FB C2
    let b = encode(&assemble("vpsubq ZMM0, ZMM1, ZMM2").unwrap()).unwrap();
    assert_eq!(b, vec![0x62, 0xF1, 0xF5, 0x48, 0xFB, 0xC2]);
    // vpaddd（W=0）：P1 = 0 1110 1 01 → 75
    let b = encode(&assemble("vpaddd ZMM0, ZMM1, ZMM2").unwrap()).unwrap();
    assert_eq!(b, vec![0x62, 0xF1, 0x75, 0x48, 0xFE, 0xC2]);
    // vdivps zmm0, zmm1, zmm2（W=0、pp=0）：62 F1 74 48 5E C2
    let b = encode(&assemble("vdivps ZMM0, ZMM1, ZMM2").unwrap()).unwrap();
    assert_eq!(b, vec![0x62, 0xF1, 0x74, 0x48, 0x5E, 0xC2]);
}

#[test]
fn x86_evex_opmask() {
    use forge_codegen::x86_v12::{assemble, encode};
    // opmask 写合并（z=0）：vaddps zmm0, zmm1, zmm2, k1 → P2 aaa=001（48→49）
    let b = encode(&assemble("vaddps ZMM0, ZMM1, ZMM2, K1").unwrap()).unwrap();
    assert_eq!(b, vec![0x62, 0xF1, 0x74, 0x49, 0x58, 0xC2]);
    // k7 → aaa=111（4F）
    let b = encode(&assemble("vaddps ZMM0, ZMM1, ZMM2, K7").unwrap()).unwrap();
    assert_eq!(b, vec![0x62, 0xF1, 0x74, 0x4F, 0x58, 0xC2]);
    // 零掩码（z=1）：vaddpsz zmm0, zmm1, zmm2, k1 → P2 = z1(80) | aaa(01) = C9
    let b = encode(&assemble("vaddpsz ZMM0, ZMM1, ZMM2, K1").unwrap()).unwrap();
    assert_eq!(b, vec![0x62, 0xF1, 0x74, 0xC9, 0x58, 0xC2]);
    // 3 操作数无掩码（分发：vaddps zmm0, zmm1, zmm2 → aaa=0，48）
    let b = encode(&assemble("vaddps ZMM0, ZMM1, ZMM2").unwrap()).unwrap();
    assert_eq!(b, vec![0x62, 0xF1, 0x74, 0x48, 0x58, 0xC2]);
}

#[test]
fn x86_evex_opmask_roundtrip() {
    roundtrip_asm("x86_vaddps_zmm_mask", "vaddps ZMM0, ZMM1, ZMM2, K1");
    roundtrip_asm("x86_vaddps_zmm_mask7", "vaddps ZMM0, ZMM1, ZMM2, K7");
    roundtrip_asm("x86_vaddpsz_zmm_mask", "vaddpsz ZMM0, ZMM1, ZMM2, K1");
    roundtrip_asm("x86_vaddps_zmm_nomask", "vaddps ZMM0, ZMM1, ZMM2");
}

#[test]
fn x86_vex_mem_roundtrip() {
    // VEX 内存形式（迭代：mod≠3 + base/disp + SIB）
    roundtrip_asm("x86_vmovaps_mem", "vmovaps XMM0, [RAX]");
    roundtrip_asm("x86_vmovaps_mem_disp", "vmovaps XMM0, [RAX+8]");
    roundtrip_asm("x86_vmovaps_mem_neg", "vmovaps XMM0, [RAX-16]");
    roundtrip_asm("x86_vmovaps_mem_rsp", "vmovaps XMM1, [RSP]"); // SIB
    roundtrip_asm("x86_vmovaps_mem32", "vmovaps XMM0, [R12+4096]"); // disp32
}

#[test]
fn x86_vex_mem_spec_bytes() {
    use forge_codegen::x86_v12::{assemble, encode};
    // vmovaps xmm0, [rax] → C4 E1 7C 28 00（vex_l=1；mod=00, base=rax）
    let b = encode(&assemble("vmovaps XMM0, [RAX]").unwrap()).unwrap();
    assert_eq!(b, vec![0xC4, 0xE1, 0x7C, 0x28, 0x00]);
    // [rax+8] → mod=01 + disp8
    let b = encode(&assemble("vmovaps XMM0, [RAX+8]").unwrap()).unwrap();
    assert_eq!(b, vec![0xC4, 0xE1, 0x7C, 0x28, 0x40, 0x08]);
    // [rsp] → SIB（base=RSP；reg=XMM1 → modrm.reg=1）
    let b = encode(&assemble("vmovaps XMM1, [RSP]").unwrap()).unwrap();
    assert_eq!(b, vec![0xC4, 0xE1, 0x7C, 0x28, 0x0C, 0x24]);
}

#[test]
fn x86_sib_index_scale() {
    // 索引寻址 [base + index*scale + disp]：SIB index/scale + REX.X
    roundtrip_asm("sib_idx", "mov64rm RAX, [RBX+RCX*4+8]");
    roundtrip_asm("sib_idx_scale1", "mov64rm RAX, [RBX+RCX]");
    roundtrip_asm("sib_idx_scale2", "mov64rm RAX, [RBX+RCX*2-16]");
    roundtrip_asm("sib_idx_scale8", "mov64rm RAX, [RBP+RCX*8]");
    roundtrip_asm("sib_idx_r12", "mov64rm RAX, [RBX+R12*4]"); // REX.X
    roundtrip_asm("sib_idx_r13", "mov64rm RAX, [RBX+R13]"); // REX.X index
    roundtrip_asm("sib_idx_base_r12", "mov64rm RAX, [R12+RCX*2+64]"); // REX.B base
    // 字节断言：[rbx+rcx*4+8] → 48 8B 44 8B 08
    use forge_codegen::x86_v12::{assemble, encode};
    let b = encode(&assemble("mov64rm RAX, [RBX+RCX*4+8]").unwrap()).unwrap();
    assert_eq!(b, vec![0x48, 0x8B, 0x44, 0x8B, 0x08]);
    // [rbx+r12*4] → REX.X=1（R12 索引 = SIB index 4 | X<<3）+ scale 4：4A 8B 04 A3
    let b = encode(&assemble("mov64rm RAX, [RBX+R12*4]").unwrap()).unwrap();
    assert_eq!(b, vec![0x4A, 0x8B, 0x04, 0xA3]);
}

#[test]
fn x86_control_roundtrip() {
    roundtrip_asm("x86_ret", "ret");
    roundtrip_asm("x86_jmp", "jmp 42");
    roundtrip_asm("x86_cqo", "cqo"); // NOOP_REXW：48 99
}
