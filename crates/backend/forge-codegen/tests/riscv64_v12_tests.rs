//! riscv64 v12 pilot 验证（迭代 2）。
//!
//! - **golden**：GPR 指令字节与 v11 后端一致（汇编→编码对比）。
//! - **规范字节**：F 指令按 RISC-V 规范独立 oracle 计算（v11 的 FPR 索引
//!   `16+i` 使字节不合规且仅内部往返一致；v12 用组内索引，规范合规）。
//! - **全量往返**：decode(encode(X)) == X（含 v11 不可解码的分支/负立即数；
//!   v11 对负立即数解码不符号扩展，v12 按槽宽度规范扩展）。
//! - **assemble/disassemble** 往返。

use forge_codegen::riscv64_v12::{Inst, assemble, decode, disassemble, encode};

fn v11_bytes(asm: &str) -> Vec<u8> {
    use forge_codegen::machine::assembler::TargetAssembler;
    use forge_codegen::machine::target::TargetMachine;
    let tm = forge_codegen::riscv64::TargetMachine::new();
    let asm_ = forge_codegen::riscv64::Assembler;
    let insts = asm_
        .parse_insts(asm)
        .unwrap_or_else(|e| panic!("v11 parse `{asm}`: {e:?}"));
    let inst = insts.into_iter().next().expect("one inst");
    tm.encoder()
        .encode_to_bytes(&inst, &forge_codegen::AllocResult::new())
        .unwrap_or_else(|e| panic!("v11 encode {inst:?}: {e:?}"))
}

fn v12_bytes(asm: &str) -> [u8; 4] {
    let inst = assemble(asm).unwrap_or_else(|e| panic!("v12 assemble `{asm}`: {e}"));
    encode(&inst).unwrap_or_else(|e| panic!("v12 encode {inst:?}: {e}"))
}

// ─────────────────────── golden：GPR 与 v11 一致 ───────────────────────

#[test]
fn golden_gpr_matches_v11() {
    let cases = [
        "add X1, X2, X3",
        "sub X1, X2, X3",
        "sll X1, X2, X3",
        "slt X1, X2, X3",
        "sltu X1, X2, X3",
        "xor X1, X2, X3",
        "srl X1, X2, X3",
        "sra X1, X2, X3",
        "or X1, X2, X3",
        "and X1, X2, X3",
        "mul X1, X2, X3",
        "div X1, X2, X3",
        "divu X1, X2, X3",
        "rem X1, X2, X3",
        "remu X1, X2, X3",
        "addi X1, X2, 5",
        "addi X1, X2, -5",
        "slti X1, X2, 5",
        "sltiu X1, X2, 5",
        "xori X1, X2, 5",
        "ori X1, X2, 5",
        "andi X1, X2, 5",
        "slli X1, X2, 3",
        "srli X1, X2, 3",
        "srai X1, X2, 3",
        "lui X1, 4096",
        "ld X1, 8(X2)",
        // 注：sd/FSW 等 S 型不在 golden 列表——v11 的 S 型散布移位反了
        //（[7;5;5]/[25;7] 应为 [7;5;0]/[25;7;5]），字节非规范；v12 规范正确，
        // 见 `s_type_spec_bytes`。
        "jalr X1, 8(X2)",
        "ecall",
        "fence",
        "clz X1, X2",
        "ctz X1, X2",
        "cpop X1, X2",
        "rev8 X1, X2",
        "rol X1, X2, X3",
        "ror X1, X2, X3",
        "min X1, X2, X3",
        "max X1, X2, X3",
        "minu X1, X2, X3",
        "maxu X1, X2, X3",
        "amoadd.w.aqrl X1, X2, (X3)",
    ];
    for c in cases {
        let vb = v11_bytes(c);
        let v12b = v12_bytes(c);
        assert_eq!(
            v12b.to_vec(),
            vb,
            "golden mismatch for `{c}` (v12 {v12b:02x?} vs v11 {vb:02x?})"
        );
    }
}

// ─────────────────── 规范字节：F 指令（独立 oracle）───────────────────

/// RISC-V R 型字：funct7|rs2|rs1|funct3|rd|opcode（规范位布局）。
fn r_type(funct7: u32, rs2: u32, rs1: u32, funct3: u32, rd: u32, opcode: u32) -> [u8; 4] {
    let w = (funct7 << 25) | (rs2 << 20) | (rs1 << 15) | (funct3 << 12) | (rd << 7) | opcode;
    w.to_le_bytes()
}

#[test]
fn f_inst_spec_bytes() {
    // v11 的 F 编码（16+i 索引）不合规；v12 组内索引 → 与 RISC-V 规范一致。
    let cases: &[(&str, [u8; 4])] = &[
        ("fadd.s F1, F2, F3", r_type(0x00, 3, 2, 0, 1, 0x53)),
        ("fsub.s F1, F2, F3", r_type(0x04, 3, 2, 0, 1, 0x53)),
        ("fmul.s F1, F2, F3", r_type(0x08, 3, 2, 0, 1, 0x53)),
        ("fdiv.s F1, F2, F3", r_type(0x0C, 3, 2, 0, 1, 0x53)),
        ("fsqrt.s F1, F2", r_type(0x2C, 0, 2, 0, 1, 0x53)),
        ("fsgnj.s F1, F2, F3", r_type(0x10, 3, 2, 0, 1, 0x53)),
        ("fsgnjn.s F1, F2, F3", r_type(0x10, 3, 2, 1, 1, 0x53)),
        ("fsgnjx.s F1, F2, F3", r_type(0x10, 3, 2, 2, 1, 0x53)),
        ("fcvt.w.s X1, F2", r_type(0x60, 0, 2, 1, 1, 0x53)),
        ("fcvt.s.w F1, X2", r_type(0x68, 0, 2, 0, 1, 0x53)),
        ("flt.s X1, F2, F3", r_type(0x20, 3, 2, 4, 1, 0x53)),
        ("feq.s X1, F2, F3", r_type(0x20, 3, 2, 5, 1, 0x53)),
        ("fle.s X1, F2, F3", r_type(0x20, 3, 2, 6, 1, 0x53)),
        ("fmin.s F1, F2, F3", r_type(0x14, 3, 2, 0, 1, 0x53)),
        ("fmax.s F1, F2, F3", r_type(0x14, 3, 2, 1, 1, 0x53)),
        ("fmv.w.x F1, X2", r_type(0x78, 0, 2, 0, 1, 0x53)),
        ("fmv.x.w X1, F2", r_type(0x70, 0, 2, 0, 1, 0x53)),
        // FLW/FSW（I/S 型）
        ("flw F1, 8(X2)", [0x87, 0x20, 0x81, 0x00]),
        ("fsw F1, 8(X2)", [0x27, 0x24, 0x11, 0x00]),
    ];
    for (asm, expected) in cases {
        let got = v12_bytes(asm);
        assert_eq!(&got, expected, "spec mismatch for `{asm}`");
    }
}

#[test]
fn s_type_spec_bytes() {
    // v11 的 S 型散布（[7;5;5] + [25;7]）把移位写反，字节非规范；
    // v12 用规范布局（imm[4:0]→bit11:7，imm[11:5]→bit31:25）。
    // sd x1, 8(x2) 规范 = 0x00113423（与 GNU as 一致）
    let b = v12_bytes("sd X1, 8(X2)");
    assert_eq!(b, [0x23, 0x34, 0x11, 0x00], "sd X1,8(X2) 规范字节");
    // 负位移：sd x1, -8(x2) → imm[4:0]=0x18, imm[11:5]=0x7F
    let b = v12_bytes("sd X1, -8(X2)");
    assert_eq!(b, [0x23, 0x3C, 0x11, 0xFE], "sd X1,-8(X2) 规范字节");
}

// ─────────────────── 全量 decode 往返（含分支/负立即数）───────────────────

#[test]
fn decode_roundtrip_all() {
    // 规范指令（避开与更早声明的别名指令编码重叠的值：addi imm≠0、add rs2≠0 等）
    let cases = [
        "add X1, X2, X3",
        "add X1, X2, X0",
        "sub X1, X2, X3",
        "mul X1, X2, X3",
        "div X1, X2, X3",
        "divu X1, X2, X3",
        "rem X1, X2, X3",
        "remu X1, X2, X3",
        "and X1, X2, X3",
        "or X1, X2, X3",
        "xor X1, X2, X3",
        "sll X1, X2, X3",
        "srl X1, X2, X3",
        "sra X1, X2, X3",
        "slt X1, X2, X3",
        "sltu X1, X2, X3",
        "addi X1, X2, 5",
        "addi X1, X2, -1",
        "slti X1, X2, -3",
        "sltiu X1, X2, 5",
        "xori X1, X2, 5",
        "ori X1, X2, 5",
        "andi X1, X2, 5",
        "slli X1, X2, 3",
        "srli X1, X2, 31",
        "srai X1, X2, 63",
        "lui X1, 4096",
        "ld X1, 8(X2)",
        "sd X1, -8(X2)",
        "jalr X1, 8(X2)",
        "beq X1, X2, 8",
        "bne X1, X2, -16",
        "blt X1, X2, 24",
        "bge X1, X2, -8",
        "bltu X1, X2, 4",
        "jal X1, 12",
        "ecall",
        "fence",
        "clz X1, X2",
        "ctz X1, X2",
        "cpop X1, X2",
        "rev8 X1, X2",
        "rol X1, X2, X3",
        "ror X1, X2, X3",
        "min X1, X2, X3",
        "max X1, X2, X3",
        "minu X1, X2, X3",
        "maxu X1, X2, X3",
        "amoadd.w.aqrl X1, X2, (X3)",
        "fadd.s F1, F2, F3",
        "fsub.s F1, F2, F3",
        "fsqrt.s F1, F2",
        "flw F1, 8(X2)",
        "fsw F1, -4(X2)",
        "fcvt.w.s X1, F2",
        "fcvt.s.w F1, X2",
        "feq.s X1, F2, F3",
    ];
    for c in cases {
        let inst = assemble(c).unwrap_or_else(|e| panic!("assemble `{c}`: {e}"));
        let bytes = encode(&inst).unwrap_or_else(|e| panic!("encode `{c}`: {e}"));
        let dec = decode(&bytes).unwrap_or_else(|| panic!("decode `{c}` ({bytes:02x?})"));
        assert_eq!(dec, inst, "round-trip `{c}`: dec {dec:?} != inst {inst:?}");
    }
}

#[test]
fn signed_immediate_sign_extension() {
    // v11 解码负立即数不做符号扩展（-1 → 4095）；v12 按槽宽度规范扩展。
    let inst = assemble("addi X1, X2, -1").unwrap();
    let bytes = encode(&inst).unwrap();
    let dec = decode(&bytes).unwrap();
    assert_eq!(dec, inst, "addi -1 往返（含符号扩展）");
    // 12 位有符号边界：-2048 / 2047
    for imm in [-2048i64, 2047] {
        let asm = format!("addi X1, X2, {imm}");
        let inst = assemble(&asm).unwrap();
        let bytes = encode(&inst).unwrap();
        assert_eq!(decode(&bytes).unwrap(), inst, "addi {imm}");
    }
}

#[test]
fn branch_scatter_layout() {
    // B 型散布布局（规范）：beq x1,x2,8 → imm[4:1]=4 置于 bit 11:8
    let b = v12_bytes("beq X1, X2, 8");
    assert_eq!(b, [0x63, 0x84, 0x20, 0x00], "beq X1,X2,8 字节");
    // J 型：jal x1, 8 → imm[10:1]=4 置于 bit 30:21
    let b = v12_bytes("jal X1, 8");
    assert_eq!(b, [0xEF, 0x00, 0x80, 0x00], "jal X1,8 字节");
}

// ─────────────────── assemble/disassemble 往返 ───────────────────

#[test]
fn assemble_disassemble_roundtrip() {
    let cases = [
        "add x1, x2, x3",
        "addi x1, x2, -5",
        "ld x1, 8(x2)",
        "sd x1, -8(x2)",
        "jalr x1, 0(x2)",
        "beq x1, x2, 12",
        "jal x1, 4",
        "lui x1, 4096",
        "fadd.s f1, f2, f3",
        "flw f1, 8(x2)",
        "amoadd.w.aqrl x1, x2, (x3)",
        "fence",
        "ecall",
    ];
    for c in cases {
        let inst = assemble(c).unwrap_or_else(|e| panic!("assemble `{c}`: {e}"));
        let text = disassemble(&inst);
        // 反汇编文本再汇编应还原同一指令（寄存器名大小写归一为 X/F 大写）
        let inst2 = assemble(&text).unwrap_or_else(|e| panic!("re-assemble `{text}`: {e}"));
        assert_eq!(inst, inst2, "disassemble `{c}` → `{text}` 往返");
    }
}

#[test]
fn disassemble_known_texts() {
    assert_eq!(
        disassemble(&assemble("add X1, X2, X3").unwrap()),
        "add X1, X2, X3"
    );
    assert_eq!(
        disassemble(&assemble("ld X1, 8(X2)").unwrap()),
        "ld X1, 8(X2)"
    );
    assert_eq!(
        disassemble(&assemble("amoadd.w.aqrl X1, X2, (X3)").unwrap()),
        "amoadd.w.aqrl X1, X2, (X3)"
    );
}

#[test]
fn inst_enum_shapes() {
    // Inst 字段类型：reg → u32，imm/label → i64（枚举形状抽查）
    let add = assemble("add X1, X2, X3").unwrap();
    assert_eq!(
        add,
        Inst::Add {
            rd: 1,
            rs1: 2,
            rs2: 3
        }
    );
    let addi = assemble("addi X1, X2, -1").unwrap();
    assert_eq!(
        addi,
        Inst::Addi {
            rd: 1,
            rs1: 2,
            imm12: -1
        }
    );
    let sd = assemble("sd X1, 8(X2)").unwrap();
    assert_eq!(
        sd,
        Inst::Sd {
            rs2: 1,
            rs1: 2,
            imm_s: 8
        }
    );
    let beq = assemble("beq X1, X2, 8").unwrap();
    assert_eq!(
        beq,
        Inst::Beq {
            rs1: 1,
            rs2: 2,
            imm_b: 8
        }
    );
    let jal = assemble("jal X1, 8").unwrap();
    assert_eq!(jal, Inst::Jal { rd: 1, imm_j: 8 });
    let lui = assemble("lui X1, 4096").unwrap();
    assert_eq!(lui, Inst::Lui { rd: 1, imm20: 4096 });
    assert_eq!(assemble("fence").unwrap(), Inst::Fence);
    assert_eq!(assemble("nop").unwrap(), Inst::Nop);
}
