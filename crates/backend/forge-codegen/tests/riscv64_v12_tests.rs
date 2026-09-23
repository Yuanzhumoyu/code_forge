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

// ─────────────── 规范字节：已迁入谱内 `[[vectors]]`（v19 V3a）───────────────
//
// 原先的三张 oracle 表（`golden_gpr_spec_bytes` / `f_inst_spec_bytes` /
// `s_type_spec_bytes`，共 62 条）已迁到 `isa/riscv64_v12.toml` 的 `[[vectors]]`——
// 字节 oracle 与谱同处一地，断言由生成物里的 `__spec_tests::spec_vector_*` 执行
// （`cargo test -p forge-codegen --lib`）。迁移前后字节集合的规范化 sha256 相同：
// `fffd0549ca3cfe6c5b4705fa49d949e25c042a8ca67511b4ecad9d2b4bc53a94`
// （证据与命令见 `docs/plans/forge-isa-dsl-v19-plan.md` §5 的 V3 进度）。

// F 指令 / S 型的规范字节同样已在谱内：见 `isa/riscv64_v12.toml` 的 `[[vectors]]`
// （其中 F 表原先由 `r_type(...)` 算出的期望值，迁移时按同一布局落成字面量）。

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
        // 编码正确性由谱内 `[[vectors]]`（`isa/riscv64_v12.toml`）覆盖。
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
        src: Reg::X1,
        src2: Reg::X2,
        imm: 2047,
    };
    assert!(encode(&ok).is_ok(), "imm=2047 应可编码");
    // 越界：encode 报错（不静默截断）
    for imm in [-2049i64, 2048, 3000, -3000] {
        let inst = Inst::Sw {
            src: Reg::X1,
            src2: Reg::X2,
            imm,
        };
        assert!(
            encode(&inst).is_err(),
            "imm={imm} 超 [-2048, 2047] 应报错（P0-16 散布位域检查）"
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
    // Inst 字段名 = `ops` 里声明的名字（`ops = ["dst:gpr:out", "src:gpr", "src2:gpr"]`
    // 等），字段类型：reg → Reg 枚举，imm/label → i64（枚举形状抽查）。
    let add = assemble("add X1, X2, X3").unwrap();
    assert_eq!(
        add,
        Inst::Add {
            dst: Reg::X1,
            src: Reg::X2,
            src2: Reg::X3
        }
    );
    let addi = assemble("addi X1, X2, -1").unwrap();
    assert_eq!(
        addi,
        Inst::Addi {
            dst: Reg::X1,
            src: Reg::X2,
            imm: -1
        }
    );
    let sd = assemble("sd X1, 8(X2)").unwrap();
    assert_eq!(
        sd,
        Inst::Sd {
            src: Reg::X1,
            src2: Reg::X2,
            imm: 8
        }
    );
    let beq = assemble("beq X1, X2, 8").unwrap();
    assert_eq!(
        beq,
        Inst::Beq {
            src: Reg::X1,
            src2: Reg::X2,
            target: 8
        }
    );
    let jal = assemble("jal X1, 8").unwrap();
    assert_eq!(
        jal,
        Inst::Jal {
            dst: Reg::X1,
            target: 8
        }
    );
    let lui = assemble("lui X1, 4096").unwrap();
    assert_eq!(
        lui,
        Inst::Lui {
            dst: Reg::X1,
            imm: 4096
        }
    );
    assert_eq!(assemble("fence").unwrap(), Inst::Fence);
    assert_eq!(assemble("nop").unwrap(), Inst::Nop);
}
