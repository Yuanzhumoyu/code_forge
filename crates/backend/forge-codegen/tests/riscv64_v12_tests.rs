//! riscv64 v12 验证。v11 后端已删除，golden 全部为硬编码 RISC-V 规范 oracle
//! 字节（v12 组内寄存器索引；v11 的 FPR `16+i` 索引与 S 型散布曾非规范）。
//!
//! - **规范字节**：GPR/F 指令按 RISC-V 规范硬编码 oracle。
//! - **全量往返**：decode(encode(X)) == X（含分支/负立即数；负立即数按槽
//!   宽度规范符号扩展）。
//! - **assemble/disassemble** 往返。

use forge_codegen::riscv64_v12::{Inst, Reg, assemble, decode, disassemble, encode};

fn v12_bytes(asm: &str) -> Vec<u8> {
    let inst = assemble(asm).unwrap_or_else(|e| panic!("v12 assemble `{asm}`: {e}"));
    encode(&inst).unwrap_or_else(|e| panic!("v12 encode {inst:?}: {e}"))
}

// ─────────────────────── 规范字节：GPR ───────────────────────

#[test]
fn golden_gpr_spec_bytes() {
    let cases: &[(&str, &[u8])] = &[
        ("add X1, X2, X3", &[0xB3, 0x00, 0x31, 0x00]),
        ("sub X1, X2, X3", &[0xB3, 0x00, 0x31, 0x40]),
        ("sll X1, X2, X3", &[0xB3, 0x10, 0x31, 0x00]),
        ("slt X1, X2, X3", &[0xB3, 0x20, 0x31, 0x00]),
        ("sltu X1, X2, X3", &[0xB3, 0x30, 0x31, 0x00]),
        ("xor X1, X2, X3", &[0xB3, 0x40, 0x31, 0x00]),
        ("srl X1, X2, X3", &[0xB3, 0x50, 0x31, 0x00]),
        ("sra X1, X2, X3", &[0xB3, 0x50, 0x31, 0x40]),
        ("or X1, X2, X3", &[0xB3, 0x60, 0x31, 0x00]),
        ("and X1, X2, X3", &[0xB3, 0x70, 0x31, 0x00]),
        ("mul X1, X2, X3", &[0xB3, 0x00, 0x31, 0x02]),
        ("div X1, X2, X3", &[0xB3, 0x40, 0x31, 0x02]),
        ("divu X1, X2, X3", &[0xB3, 0x50, 0x31, 0x02]),
        ("rem X1, X2, X3", &[0xB3, 0x60, 0x31, 0x02]),
        ("remu X1, X2, X3", &[0xB3, 0x70, 0x31, 0x02]),
        ("addi X1, X2, 5", &[0x93, 0x00, 0x51, 0x00]),
        ("addi X1, X2, -5", &[0x93, 0x00, 0xB1, 0xFF]),
        ("slti X1, X2, 5", &[0x93, 0x20, 0x51, 0x00]),
        ("sltiu X1, X2, 5", &[0x93, 0x30, 0x51, 0x00]),
        ("xori X1, X2, 5", &[0x93, 0x40, 0x51, 0x00]),
        ("ori X1, X2, 5", &[0x93, 0x60, 0x51, 0x00]),
        ("andi X1, X2, 5", &[0x93, 0x70, 0x51, 0x00]),
        ("slli X1, X2, 3", &[0x93, 0x10, 0x31, 0x00]),
        ("srli X1, X2, 3", &[0x93, 0x50, 0x31, 0x00]),
        ("srai X1, X2, 3", &[0x93, 0x50, 0x31, 0x40]),
        ("lui X1, 4096", &[0xB7, 0x10, 0x00, 0x00]),
        ("ld X1, 8(X2)", &[0x83, 0x30, 0x81, 0x00]),
        // 注：sd/FSW 等 S 型——v11 的 S 型散布移位反了（[7;5;5]/[25;7] 应为
        // [7;5;0]/[25;7;5]），字节非规范；v12 规范正确，见 `s_type_spec_bytes`。
        ("jalr X1, 8(X2)", &[0xE7, 0x00, 0x81, 0x00]),
        ("ecall", &[0x73, 0x00, 0x00, 0x00]),
        ("fence", &[0x0F, 0x00, 0xF0, 0x0F]),
        ("clz X1, X2", &[0xB3, 0x10, 0x01, 0x60]),
        ("ctz X1, X2", &[0xB3, 0x10, 0x11, 0x60]),
        ("cpop X1, X2", &[0xB3, 0x10, 0x21, 0x60]),
        ("rev8 X1, X2", &[0xB3, 0x50, 0x81, 0xD0]),
        ("rol X1, X2, X3", &[0xB3, 0x10, 0x31, 0x60]),
        ("ror X1, X2, X3", &[0xB3, 0x50, 0x31, 0x60]),
        ("min X1, X2, X3", &[0xB3, 0x00, 0x31, 0x0A]),
        ("max X1, X2, X3", &[0xB3, 0x10, 0x31, 0x0A]),
        ("minu X1, X2, X3", &[0xB3, 0x40, 0x31, 0x0A]),
        ("maxu X1, X2, X3", &[0xB3, 0x50, 0x31, 0x0A]),
        ("amoadd.w X1, X2, (X3)", &[0xAF, 0xA0, 0x21, 0x00]),
    ];
    for (asm, expected) in cases {
        let got = v12_bytes(asm);
        assert_eq!(got.as_slice(), *expected, "spec mismatch for `{asm}`");
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
        ("fcvt.w.s X1, F2, rtz", r_type(0x60, 0, 2, 1, 1, 0x53)),
        ("fcvt.s.w F1, X2", r_type(0x68, 0, 2, 0, 1, 0x53)),
        ("flt.s X1, F2, F3", r_type(0x50, 3, 2, 1, 1, 0x53)),
        ("feq.s X1, F2, F3", r_type(0x50, 3, 2, 2, 1, 0x53)),
        ("fle.s X1, F2, F3", r_type(0x50, 3, 2, 0, 1, 0x53)),
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
        assert_eq!(got.as_slice(), expected, "spec mismatch for `{asm}`");
    }
}

#[test]
fn s_type_spec_bytes() {
    // v11 的 S 型散布（[7;5;5] + [25;7]）把移位写反，字节非规范；
    // v12 用规范布局（imm[4:0]→bit11:7，imm[11:5]→bit31:25）。
    // sd x1, 8(x2) 规范 = 0x00113423（与 GNU as 一致）
    let b = v12_bytes("sd X1, 8(X2)");
    assert_eq!(
        b.as_slice(),
        [0x23, 0x34, 0x11, 0x00],
        "sd X1,8(X2) 规范字节"
    );
    // 负位移：sd x1, -8(x2) → imm[4:0]=0x18, imm[11:5]=0x7F
    let b = v12_bytes("sd X1, -8(X2)");
    assert_eq!(
        b.as_slice(),
        [0x23, 0x3C, 0x11, 0xFE],
        "sd X1,-8(X2) 规范字节"
    );
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
        // 注：clz/ctz/cpop 与 rol/ror 编码重叠（clz rd,rs = rol rd,rs,x0 别名）——
        // decode 按声明序/叶优先返回 rol，roundtrip 语义歧义，故不列入；
        // 编码正确性由 golden_gpr_spec_bytes 覆盖。
        "rev8 X1, X2",
        "rol X1, X2, X3",
        "ror X1, X2, X3",
        "min X1, X2, X3",
        "max X1, X2, X3",
        "minu X1, X2, X3",
        "maxu X1, X2, X3",
        "amoadd.w X1, X2, (X3)",
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
        let (dec, n) = decode(&bytes).unwrap_or_else(|| panic!("decode `{c}` ({bytes:02x?})"));
        assert_eq!(n, bytes.len(), "decode `{c}` 消费字节数");
        assert_eq!(dec, inst, "round-trip `{c}`: dec {dec:?} != inst {inst:?}");
    }
}

#[test]
fn signed_immediate_sign_extension() {
    // v11 解码负立即数不做符号扩展（-1 → 4095）；v12 按槽宽度规范扩展。
    let inst = assemble("addi X1, X2, -1").unwrap();
    let bytes = encode(&inst).unwrap();
    let (dec, _n) = decode(&bytes).unwrap();
    assert_eq!(dec, inst, "addi -1 往返（含符号扩展）");
    // 12 位有符号边界：-2048 / 2047
    for imm in [-2048i64, 2047] {
        let asm = format!("addi X1, X2, {imm}");
        let inst = assemble(&asm).unwrap();
        let bytes = encode(&inst).unwrap();
        assert_eq!(decode(&bytes).unwrap().0, inst, "addi {imm}");
    }
}

/// P1 宽度修正：散布位域（S 型 imm_s）的立即数范围检查——
/// store 偏移超 imm12 范围必须报错（此前 encode 静默截断为错值）。
#[test]
fn scattered_imm_range_check() {
    // 合法边界：assemble + encode 都通过
    for imm in [-2048i64, 2047] {
        let asm = format!("sw X1, {imm}(X2)");
        let inst = assemble(&asm).unwrap_or_else(|e| panic!("assemble `{asm}`: {e}"));
        assert!(encode(&inst).is_ok(), "sw 偏移 {imm} 应在 [-2048, 2047] 内");
    }
    // 越界：asm 层 __imm 范围检查已拒绝（"operand mismatch"）——
    // 不静默截断
    for imm in [-2049i64, 2048, 3000, -3000] {
        let asm = format!("sw X1, {imm}(X2)");
        assert!(
            assemble(&asm).is_err(),
            "sw 偏移 {imm} 超 imm12 范围应在 assemble 层拒绝"
        );
    }
}

/// P1 宽度修正：encode 层直接构造越界 store 偏移 → 报错
/// （绕过 asm 层兜底，验证 P0-16 检查对散布位域生效）。
#[test]
fn scattered_imm_encode_range_check() {
    // 合法：encode 通过
    let ok = Inst::Sw {
        rs2: Reg::X1,
        rs1: Reg::X2,
        imm_s: 2047,
    };
    assert!(encode(&ok).is_ok(), "imm_s=2047 应可编码");
    // 越界：encode 报错（不静默截断）
    for imm in [-2049i64, 2048, 3000, -3000] {
        let inst = Inst::Sw {
            rs2: Reg::X1,
            rs1: Reg::X2,
            imm_s: imm,
        };
        assert!(
            encode(&inst).is_err(),
            "imm_s={imm} 超 [-2048, 2047] 应报错（P0-16 散布位域检查）"
        );
    }
}

/// P1 宽度修正：预移位散布位域（LUI imm20 存左移 12 的值）不误报——
/// fconst hi20 场景（完整 32 位值域）保持可编码。
#[test]
fn preshifted_imm_not_range_checked() {
    // LUI 槽存预移位值（lui X1, 4096 = 1<<12）——越界位宽内任意值合法
    for imm in [4096i64, 0xFFFFF, 0x80000] {
        let asm = format!("lui X1, {imm}");
        let inst = assemble(&asm).unwrap();
        assert!(encode(&inst).is_ok(), "lui 预移位值 {imm:#x} 不应误报");
    }
}

#[test]
fn branch_scatter_layout() {
    // B 型散布布局（规范）：beq x1,x2,8 → imm[4:1]=4 置于 bit 11:8
    let b = v12_bytes("beq X1, X2, 8");
    assert_eq!(b.as_slice(), [0x63, 0x84, 0x20, 0x00], "beq X1,X2,8 字节");
    // J 型：jal x1, 8 → imm[10:1]=4 置于 bit 30:21
    let b = v12_bytes("jal X1, 8");
    assert_eq!(b.as_slice(), [0xEF, 0x00, 0x80, 0x00], "jal X1,8 字节");
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
        "amoadd.w x1, x2, (x3)",
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
        disassemble(&assemble("amoadd.w X1, X2, (X3)").unwrap()),
        "amoadd.w X1, X2, (X3)"
    );
}

#[test]
fn inst_enum_shapes() {
    // Inst 字段类型：reg → Reg 枚举，imm/label → i64（枚举形状抽查）
    let add = assemble("add X1, X2, X3").unwrap();
    assert_eq!(
        add,
        Inst::Add {
            rd: Reg::X1,
            rs1: Reg::X2,
            rs2: Reg::X3
        }
    );
    let addi = assemble("addi X1, X2, -1").unwrap();
    assert_eq!(
        addi,
        Inst::Addi {
            rd: Reg::X1,
            rs1: Reg::X2,
            imm12: -1
        }
    );
    let sd = assemble("sd X1, 8(X2)").unwrap();
    assert_eq!(
        sd,
        Inst::Sd {
            rs2: Reg::X1,
            rs1: Reg::X2,
            imm_s: 8
        }
    );
    let beq = assemble("beq X1, X2, 8").unwrap();
    assert_eq!(
        beq,
        Inst::Beq {
            rs1: Reg::X1,
            rs2: Reg::X2,
            imm_b: 8
        }
    );
    let jal = assemble("jal X1, 8").unwrap();
    assert_eq!(
        jal,
        Inst::Jal {
            rd: Reg::X1,
            imm_j: 8
        }
    );
    let lui = assemble("lui X1, 4096").unwrap();
    assert_eq!(
        lui,
        Inst::Lui {
            rd: Reg::X1,
            imm20: 4096
        }
    );
    assert_eq!(assemble("fence").unwrap(), Inst::Fence);
    assert_eq!(assemble("nop").unwrap(), Inst::Nop);
}
