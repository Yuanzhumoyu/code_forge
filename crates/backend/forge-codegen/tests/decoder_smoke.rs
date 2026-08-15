//! DSL 生成解码器冒烟测试（P2 Phase 1：定宽 ISA encode→decode 往返）。
//!
//! 原理：汇编器把文本解析为物理化 Inst → Encoder 编码为字节 → Decoder 反解 →
//! 再编码，断言字节一致（避免 Reg::from_index 视图选择差异干扰断言）。

use forge_codegen::machine::assembler::TargetAssembler;
use forge_codegen::machine::decoder::TargetDecoder;
use forge_codegen::machine::encoder::TargetEncoder;
use forge_codegen::machine::target::TargetMachine;

fn roundtrip_asm<M: TargetMachine>(
    name: &str,
    tm: M,
    asm: impl TargetAssembler<Inst = M::Inst>,
    src: &str,
) {
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

// ── riscv64（32 位定宽 R/I 型）──

#[test]
fn riscv_addi_roundtrip() {
    roundtrip_asm(
        "riscv_addi",
        forge_codegen::riscv64::TargetMachine::new(),
        forge_codegen::riscv64::Assembler,
        "addi X1, X2, 5",
    );
}

#[test]
fn riscv_add_roundtrip() {
    roundtrip_asm(
        "riscv_add",
        forge_codegen::riscv64::TargetMachine::new(),
        forge_codegen::riscv64::Assembler,
        "add X1, X2, X3",
    );
}

#[test]
fn riscv_fadd_s_roundtrip() {
    // Freg 字段 → FPR 寄存器类解码
    roundtrip_asm(
        "riscv_fadd_s",
        forge_codegen::riscv64::TargetMachine::new(),
        forge_codegen::riscv64::Assembler,
        "fadd.s F0, F1, F2",
    );
}

// ── aarch64（32 位定宽）──

#[test]
fn aarch64_mov_roundtrip() {
    roundtrip_asm(
        "aarch64_mov",
        forge_codegen::aarch64::TargetMachine::new(),
        forge_codegen::aarch64::Assembler,
        "mov X0, X1",
    );
}

// ── x86_64（Phase 2 变长原语：@modrm/@mov_imm64/@push_reg/@pop_reg，mod=11）──

#[test]
fn x86_modrm_roundtrip() {
    roundtrip_asm(
        "x86_movrr",
        forge_codegen::x86_64::TargetMachine::new(),
        forge_codegen::x86_64::Assembler,
        "movrr RAX, RBX",
    );
    // 高编号寄存器 → REX.R/REX.B 扩展位
    roundtrip_asm(
        "x86_movrr_r8",
        forge_codegen::x86_64::TargetMachine::new(),
        forge_codegen::x86_64::Assembler,
        "movrr R8, R9",
    );
}

#[test]
fn x86_mov_imm64_roundtrip() {
    roundtrip_asm(
        "x86_mov_imm",
        forge_codegen::x86_64::TargetMachine::new(),
        forge_codegen::x86_64::Assembler,
        "mov_imm RAX, 0x10",
    );
}

#[test]
fn x86_push_pop_roundtrip() {
    roundtrip_asm(
        "x86_push_r12",
        forge_codegen::x86_64::TargetMachine::new(),
        forge_codegen::x86_64::Assembler,
        "push R12", // REX.B 扩展
    );
    roundtrip_asm(
        "x86_pop_rax",
        forge_codegen::x86_64::TargetMachine::new(),
        forge_codegen::x86_64::Assembler,
        "pop RAX",
    );
}

#[test]
fn x86_sse_rr_roundtrip() {
    // @sse_rr：addsd(0xF2 0F 58)——Freg 字段走裸索引 FPR 映射（movsd 有 MOVSD_RR
    // 重载歧义，用 addsd 规避）
    roundtrip_asm(
        "x86_addsd",
        forge_codegen::x86_64::TargetMachine::new(),
        forge_codegen::x86_64::Assembler,
        "addsd XMM2, XMM3",
    );
    // @sse_rr 前缀 0：MOVZX 0F B6 /r（GPR 字段）
    roundtrip_asm(
        "x86_movzx_b",
        forge_codegen::x86_64::TargetMachine::new(),
        forge_codegen::x86_64::Assembler,
        "movzx_b RAX, RBX",
    );
}

#[test]
fn x86_modrm_mem_roundtrip() {
    // @modrm_mem 内存寻址（mod=00 无位移；base 为 Reg 字段）
    roundtrip_asm(
        "x86_xchg_mem",
        forge_codegen::x86_64::TargetMachine::new(),
        forge_codegen::x86_64::Assembler,
        "xchg [RAX], RBX",
    );
    // [RBP] 强制带位移（force_disp_base → mod=01 + disp8=0）
    roundtrip_asm(
        "x86_xchg_mem_rbp",
        forge_codegen::x86_64::TargetMachine::new(),
        forge_codegen::x86_64::Assembler,
        "xchg [RBP], RBX",
    );
}

// ── minimal_sd：编码不含寄存器字段 → 无可解码变体（负例）──

#[test]
fn minimal_sd_decode_unknown() {
    let tm = forge_codegen::minimal_sd_test::minimal_sd::TargetMachine::new();
    let decoder = tm.decoder().expect("decoder");
    // SD_MOV = {0x10}（仅 opcode 常量，无字段）——decode 应报 unknown
    let r = decoder.decode(&[0x10, 0x00]);
    assert!(r.is_err(), "minimal_sd 无可解码指令，应报错，got {r:?}");
}
