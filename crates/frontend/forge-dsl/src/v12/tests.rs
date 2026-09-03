//! v12 迭代 1 验证：解析器单测（合法/非法 TOML 诊断）+ 语义校验 + 序列化往返
//! + v11 文件拒绝证明（不兼容的直接体现）。

use crate::v12::model::RegClass;

use super::model::{Endian, OperandKind, OperandRole};
use super::{V12Error, parse, parse_and_validate};

// ─────────────────────── 合法解析 ───────────────────────

const RISCV_DOC: &str = r#"
[meta]
name = "riscv64_v12"
version = "12.0"
endian = "little"
mode = 64
default_inst_width = 32

[reg.gpr8]
names = ["X0", "X1", "X2", "X3", "X4", "X5", "X6", "X7", "X8", "X9", "X10", "X11",
         "X12", "X13", "X14", "X15", "X16", "X17", "X18", "X19", "X20", "X21",
         "X22", "X23", "X24", "X25", "X26", "X27", "X28", "X29", "X30", "X31"]

[conventions.bitfields]
rd     = { offset = 7,  width = 5 }
rs1    = { offset = 15, width = 5 }
rs2    = { offset = 20, width = 5 }
opcode = { offset = 0,  width = 7 }
funct3 = { offset = 12, width = 3 }
funct7 = { offset = 25, width = 7 }

[[operand_slots]]
name = "gpr"
kind = "reg"
class = "gpr8"
roles = ["in", "out"]

[[operand_slots]]
name = "imm12"
kind = "imm"
signed = true
width = 12

[[forms]]
name = "R"
opcode_field = "opcode"
operand_fields = ["rd", "rs1", "rs2"]

[[instructions]]
name = "ADD"
form = "R"
opcode = 0x33
fields = { funct3 = 0, funct7 = 0 }
asm = "add {0:[gpr:out]}, {1:[gpr:in]}, {2:[gpr:in]}"
"#;

#[test]
fn parse_minimal_riscv_style() {
    let m = parse_and_validate(RISCV_DOC).expect("valid riscv-style doc must parse");
    assert_eq!(m.meta.name, "riscv64_v12");
    assert_eq!(m.meta.default_inst_width, Some(32));
    assert!(!m.meta.variable_length);
    assert_eq!(m.reg[&RegClass::GPR(8)].names.as_ref().unwrap().len(), 32);
    assert_eq!(m.conventions.bitfields.len(), 6);
    assert_eq!(m.conventions.bitfields["rd"].offset, Some(7));
    assert_eq!(m.conventions.bitfields["rd"].width, Some(5));
    assert_eq!(m.operand_slots.len(), 2);
    assert_eq!(m.operand_slots[0].kind, OperandKind::Reg);
    assert_eq!(m.operand_slots[0].class.as_ref(), Some(&RegClass::GPR(8)));
    assert_eq!(m.operand_slots[1].kind, OperandKind::Imm);
    assert_eq!(m.operand_slots[1].signed, Some(true));
    let form = &m.forms[0];
    assert_eq!(form.name, "R");
    assert_eq!(form.opcode_field.as_deref(), Some("opcode"));
    let add = &m.instructions[0];
    assert_eq!(add.opcode, Some(0x33));
    assert_eq!(add.fields.as_ref().unwrap()["funct3"], 0);
    assert_eq!(add.asm, "add {0:[gpr:out]}, {1:[gpr:in]}, {2:[gpr:in]}");
}

const X86_DOC: &str = r#"
[meta]
name = "x86_64"
version = "12.0"
endian = "little"
mode = 64
variable_length = true
max_inst_len = 15

[reg.gpr8]
names = ["RAX", "RCX", "RDX", "RBX", "RSP", "RBP", "RSI", "RDI",
         "R8", "R9", "R10", "R11", "R12", "R13", "R14", "R15"]

[reg.fpr16]
count = 16
prefix = "XMM"

[conventions.bitfields]
modrm_reg = { offset = 8, width = 3 }
modrm_rm  = { offset = 0, width = 3 }

[conventions.modrm]
reg_field = "modrm_reg"
rm_field = "modrm_rm"
force_disp_base = [5, 13]

[[operand_slots]]
name = "gpr"
kind = "reg"
class = "gpr8"

[[operand_slots]]
name = "fpr"
kind = "reg"
class = "fpr16"

[[operand_slots]]
name = "imm32"
kind = "imm"
signed = true
width = 32

[[forms]]
name = "RR"
modrm = "rr"
rex = "auto"
"#;

#[test]
fn parse_x86_conventions() {
    let m = parse_and_validate(X86_DOC).expect("valid x86-style doc must parse");
    assert!(m.meta.variable_length);
    assert_eq!(m.meta.max_inst_len, Some(15));
    // 生成式寄存器组：count + prefix
    assert_eq!(m.reg[&RegClass::FPR(16)].count, Some(16));
    assert_eq!(m.reg[&RegClass::FPR(16)].prefix.as_deref(), Some("XMM"));
    // ModRM 约定
    let modrm = m.conventions.modrm.as_ref().expect("modrm present");
    assert_eq!(modrm.reg_field.as_deref(), Some("modrm_reg"));
    assert_eq!(modrm.rm_field.as_deref(), Some("modrm_rm"));
    assert_eq!(modrm.force_disp_base, vec![5, 13]);
    // 形式语义键
    let form = &m.forms[0];
    assert_eq!(form.modrm.as_deref(), Some("rr"));
    assert_eq!(form.rex.as_deref(), Some("auto"));
}

#[test]
fn default_flags_and_roles() {
    let doc = r#"
[meta]
name = "x"
[reg.gpr4]
count = 8
[[operand_slots]]
name = "g"
kind = "reg"
class = "gpr"
"#;
    let m = parse_and_validate(doc).expect("valid");
    assert_eq!(m.meta.endian, Endian::Little);
    assert_eq!(m.meta.mode, 64);
    assert_eq!(m.meta.default_inst_width, None);
    let slot = &m.operand_slots[0];
    assert_eq!(slot.roles, None); // 缺省 ["in"]，模型层保持 None
    assert_eq!(slot.signed, None);
    assert_eq!(slot.width, None);
}

// ─────────────────────── 序列化往返 ───────────────────────

#[test]
fn roundtrip_serialize() {
    let m1 = parse_and_validate(X86_DOC).expect("parse");
    let text = toml::to_string(&m1).expect("serialize");
    let m2 = parse(&text).expect("reparse");
    assert_eq!(m1, m2, "serialize → parse round-trip must be lossless");
    // 往返后 modrm 约定内容不变
    assert_eq!(
        m2.conventions.modrm.as_ref().unwrap().force_disp_base,
        vec![5, 13]
    );
}

/// inout 角色：读改写操作数（x86 ADD RM, R 的 RM），模板只需声明一次。
#[test]
fn inout_role_parses() {
    let doc = r#"
[meta]
name = "x"
[reg.gpr4]
count = 8
[[operand_slots]]
name = "rm"
kind = "reg"
class = "gpr"
roles = ["inout"]
[[forms]]
name = "RM"
modrm = "rm"
[[instructions]]
name = "ADD_RM_R"
form = "RM"
opcode = 0x01
asm = "add {0:[rm:inout]}, {1:[rm:in]}"
"#;
    let m = parse_and_validate(doc).expect("valid");
    // 槽能力声明：inout = 读改写
    assert_eq!(
        m.operand_slots[0].roles.as_deref(),
        Some(&[OperandRole::InOut][..])
    );
    // 序列化往返保留 inout
    let text = toml::to_string(&m).expect("serialize");
    assert!(text.contains("inout"), "got: {text}");
    let m2 = parse(&text).expect("reparse");
    assert_eq!(m, m2);
}

// ─────────────────────── 非法 TOML 诊断 ───────────────────────

#[test]
fn rejects_unknown_top_level_key() {
    let doc = r#"
[meta]
name = "x"
bogus = 1
[reg.gpr4]
count = 8
"#;
    let err = parse(doc).unwrap_err();
    match err {
        V12Error::Parse { msg, .. } => {
            assert!(msg.contains("bogus"), "msg: {msg}");
            assert!(msg.contains("unknown field"), "msg: {msg}");
        }
        other => panic!("expected Parse error, got {other:?}"),
    }
}

#[test]
fn rejects_unknown_meta_key() {
    let doc = r#"
[meta]
name = "x"
no_default_lowering = true
[reg.gpr4]
count = 8
"#;
    let err = parse(doc).unwrap_err();
    match err {
        V12Error::Parse { msg, .. } => assert!(msg.contains("no_default_lowering"), "msg: {msg}"),
        other => panic!("expected Parse error, got {other:?}"),
    }
}

#[test]
fn rejects_unknown_operand_slot_key() {
    let doc = r#"
[meta]
name = "x"
[reg.gpr4]
count = 8
[[operand_slots]]
name = "g"
kind = "reg"
class = "gpr"
garbage = 1
"#;
    let err = parse(doc).unwrap_err();
    match err {
        V12Error::Parse { msg, .. } => {
            assert!(msg.contains("garbage"), "msg: {msg}");
            assert!(msg.contains("unknown field"), "msg: {msg}");
        }
        other => panic!("expected Parse error, got {other:?}"),
    }
}

#[test]
fn rejects_bad_operand_kind() {
    let doc = r#"
[meta]
name = "x"
[reg.gpr4]
count = 8
[[operand_slots]]
name = "g"
kind = "float"
"#;
    let err = parse(doc).unwrap_err();
    match err {
        V12Error::Parse { msg, .. } => assert!(msg.contains("float"), "msg: {msg}"),
        other => panic!("expected Parse error, got {other:?}"),
    }
}

/// 不兼容证明：v11 主 ISA 文件必须被 v12 解析器拒绝。
///
/// 注：toml crate 按字母序迭代表（BTreeMap），`[abi]` 排在 `[meta]` 之前，
/// 所以首个报错是 v11 的 `[abi.arg_regs]` 而非 meta 的 `no_default_lowering`。
#[test]
fn rejects_v11_isa_file() {
    let v11 = r#"[meta]
name = "x"
version = "11.0"
[reg.gpr8]
count = 16
[abi]
arg_regs = ["RCX", "RDX"]
[inst.MOV]
encoding = "@modrm 0x01 /r"
"#;
    let err = parse(v11).expect_err("v11 file must NOT parse under v12");
    match err {
        V12Error::Parse { msg, .. } => {
            assert!(msg.contains("unknown field"), "msg: {msg}");
        }
        other => panic!("expected Parse error, got {other:?}"),
    }
}

// ─────────────────────── 语义校验 ───────────────────────

fn slot_doc(extra: &str) -> String {
    format!(
        r#"
[meta]
name = "x"
[reg.gpr4]
count = 8
[[operand_slots]]
name = "g"
kind = "reg"
class = "gpr"
{extra}
"#
    )
}

/// `reg`/`operand_slots` 是必填字段：缺失在解析阶段报 `missing field`。
#[test]
fn required_sections_missing_is_parse_error() {
    let no_reg = r#"
[meta]
name = "x"
[[operand_slots]]
name = "g"
kind = "reg"
class = "gpr"
"#;
    match parse(no_reg).unwrap_err() {
        V12Error::Parse { msg, .. } => assert!(msg.contains("missing field `reg`"), "msg: {msg}"),
        other => panic!("expected Parse error, got {other:?}"),
    }
    let no_slots = r#"
[meta]
name = "x"
[reg.gpr4]
count = 8
"#;
    match parse(no_slots).unwrap_err() {
        V12Error::Parse { msg, .. } => {
            assert!(msg.contains("missing field `operand_slots`"), "msg: {msg}");
        }
        other => panic!("expected Parse error, got {other:?}"),
    }
}

/// 空 `[reg]` 表（存在但无组）→ 语义校验报错。
#[test]
fn validation_empty_reg() {
    let doc = r#"
[meta]
name = "x"
[reg]
[[operand_slots]]
name = "g"
kind = "reg"
class = "gpr"
"#;
    let err = parse_and_validate(doc).unwrap_err();
    match err {
        V12Error::Validation { msg, .. } => assert!(msg.contains("missing [reg.*]"), "msg: {msg}"),
        other => panic!("expected Validation error, got {other:?}"),
    }
}

/// 空 `operand_slots = []`（存在但为空）→ 语义校验报错。
#[test]
fn validation_empty_operand_slots() {
    let doc = r#"
operand_slots = []
[meta]
name = "x"
[reg.gpr4]
count = 8
"#;
    let err = parse_and_validate(doc).unwrap_err();
    match err {
        V12Error::Validation { msg, .. } => {
            assert!(msg.contains("[[operand_slots]]"), "msg: {msg}");
        }
        other => panic!("expected Validation error, got {other:?}"),
    }
}

#[test]
fn validation_bitfield_overflow() {
    let doc = slot_doc("[conventions.bitfields]\nbig = { offset = 63, width = 2 }");
    let err = parse_and_validate(&doc).unwrap_err();
    match err {
        V12Error::Validation { msg, .. } => {
            assert!(msg.contains("exceeds 64 bits"), "msg: {msg}");
            assert!(msg.contains("big"), "msg: {msg}");
        }
        other => panic!("expected Validation error, got {other:?}"),
    }
}

#[test]
fn validation_modrm_ref() {
    let doc = slot_doc(
        r#"
[conventions.bitfields]
foo = { offset = 0, width = 3 }
[conventions.modrm]
reg_field = "nope"
rm_field = "foo"
"#,
    );
    let err = parse_and_validate(&doc).unwrap_err();
    match err {
        V12Error::Validation { msg, .. } => {
            assert!(msg.contains("'nope'"), "msg: {msg}");
            assert!(msg.contains("bitfield"), "msg: {msg}");
        }
        other => panic!("expected Validation error, got {other:?}"),
    }
}

#[test]
fn validation_unknown_form() {
    let doc = r#"
[meta]
name = "x"
[reg.gpr4]
count = 8
[[operand_slots]]
name = "g"
kind = "reg"
class = "gpr"
[[forms]]
name = "R"
[[instructions]]
name = "NOP"
form = "ZZZ"
asm = "nop"
"#;
    let err = parse_and_validate(doc).unwrap_err();
    match err {
        V12Error::Validation { msg, .. } => {
            assert!(msg.contains("form 'ZZZ'"), "msg: {msg}");
        }
        other => panic!("expected Validation error, got {other:?}"),
    }
}

#[test]
fn validation_instruction_unknown_slot() {
    let doc = r#"
[meta]
name = "x"
[reg.gpr4]
count = 8
[[operand_slots]]
name = "g"
kind = "reg"
class = "gpr"
[[forms]]
name = "R"
[[instructions]]
name = "NOP"
form = "R"
asm = "nop {0:[nope:out]}"
"#;
    let err = parse_and_validate(doc).unwrap_err();
    match err {
        V12Error::Validation { msg, .. } => {
            assert!(msg.contains("'nope'"), "msg: {msg}");
        }
        other => panic!("expected Validation error, got {other:?}"),
    }
}

#[test]
fn validation_instruction_exceeds_operand_fields() {
    // 定宽 form：操作数数量 > operand_fields → 校验错误
    let doc = r#"
[meta]
name = "x"
[reg.gpr4]
count = 8
[conventions.bitfields]
opcode = { offset = 0, width = 7 }
rd = { offset = 7, width = 3 }
[[operand_slots]]
name = "g"
kind = "reg"
class = "gpr"
[[forms]]
name = "R"
opcode_field = "opcode"
operand_fields = ["rd"]
[[instructions]]
name = "NOP"
form = "R"
opcode = 0x13
asm = "foo {0:[g:out]}, {1:[g:in]}"
"#;
    let err = parse_and_validate(doc).unwrap_err();
    match err {
        V12Error::Validation { msg, .. } => {
            assert!(msg.contains("exceed form"), "msg: {msg}");
        }
        other => panic!("expected Validation error, got {other:?}"),
    }
}

#[test]
fn validation_duplicate_instruction() {
    let doc = r#"
[meta]
name = "x"
[reg.gpr4]
count = 8
[[operand_slots]]
name = "g"
kind = "reg"
class = "gpr"
[[forms]]
name = "R"
[[instructions]]
name = "NOP"
form = "R"
asm = "nop"
[[instructions]]
name = "NOP"
form = "R"
asm = "nop"
"#;
    let err = parse_and_validate(doc).unwrap_err();
    match err {
        V12Error::Validation { msg, .. } => {
            assert!(
                msg.contains("duplicate instruction name 'NOP'"),
                "msg: {msg}"
            );
        }
        other => panic!("expected Validation error, got {other:?}"),
    }
}

#[test]
fn validation_family_variant_needs_opcode() {
    let doc = r#"
[meta]
name = "x"
[reg.gpr4]
count = 8
[[operand_slots]]
name = "g"
kind = "reg"
class = "gpr"
[[forms]]
name = "R"
[[families]]
name = "F"
form = "R"
asm = "f {0:[g:out]}"
[[families.variants]]
name = "V1"
"#;
    let err = parse_and_validate(doc).unwrap_err();
    match err {
        V12Error::Validation { msg, .. } => {
            assert!(
                msg.contains("variant needs `opcode` or `fields`"),
                "msg: {msg}"
            );
        }
        other => panic!("expected Validation error, got {other:?}"),
    }
}

#[test]
fn validation_duplicate_reg_names() {
    let doc = r#"
[meta]
name = "x"
[reg.gpr4]
names = ["R0", "R0"]
[[operand_slots]]
name = "g"
kind = "reg"
class = "gpr"
"#;
    let err = parse_and_validate(doc).unwrap_err();
    match err {
        V12Error::Validation { msg, .. } => {
            assert!(msg.contains("duplicate 'R0'"), "msg: {msg}");
        }
        other => panic!("expected Validation error, got {other:?}"),
    }
}

#[test]
fn validation_fixed_vs_variable_conflict() {
    let doc = r#"
[meta]
name = "x"
default_inst_width = 32
variable_length = true
[reg.gpr4]
count = 8
[[operand_slots]]
name = "g"
kind = "reg"
class = "gpr"
"#;
    let err = parse_and_validate(doc).unwrap_err();
    match err {
        V12Error::Validation { msg, .. } => {
            assert!(
                msg.contains("conflicts with `variable_length = true`"),
                "msg: {msg}"
            );
        }
        other => panic!("expected Validation error, got {other:?}"),
    }
}

#[test]
fn validation_bad_isa_name() {
    let doc = r#"
[meta]
name = "123"
[reg.gpr4]
count = 8
[[operand_slots]]
name = "g"
kind = "reg"
class = "gpr"
"#;
    let err = parse_and_validate(doc).unwrap_err();
    match err {
        V12Error::Validation { msg, .. } => {
            assert!(msg.contains("not a valid ISA name"), "msg: {msg}");
        }
        other => panic!("expected Validation error, got {other:?}"),
    }
}

// ─────────────────────── 迭代 2：codegen ───────────────────────

#[test]
fn codegen_big_endian_decode_reads_be() {
    // big-endian 定宽：encode 写 to_be_bytes、decode 读 from_be_bytes
    let doc = r#"
[meta]
name = "x"
endian = "big"
default_inst_width = 32
[reg.gpr4]
count = 8
[conventions.bitfields]
rd = { offset = 7, width = 3 }
opcode = { offset = 0, width = 7 }
[[operand_slots]]
name = "g"
kind = "reg"
class = "gpr"
[[forms]]
name = "R"
opcode_field = "opcode"
operand_fields = ["rd"]
[[instructions]]
name = "FOO"
form = "R"
opcode = 0x13
asm = "foo {0:[g:out]}"
"#;
    let model = parse_and_validate(doc).unwrap();
    let ts = super::codegen::generate(&model).unwrap();
    let s = ts.to_string();
    assert!(
        s.contains("from_be_bytes"),
        "big-endian decode 应读 BE：{s}"
    );
    assert!(s.contains("to_be_bytes"), "big-endian encode 应写 BE：{s}");
}

#[test]
fn codegen_align_pad_emit_key() {
    // [emit].align_pad 进入生成的 parse_insts（.align 填充字节可配置）
    let doc = r#"
[meta]
name = "x"
default_inst_width = 32
[reg.gpr4]
count = 8
[conventions.bitfields]
opcode = { offset = 0, width = 7 }
rd = { offset = 7, width = 3 }
[[operand_slots]]
name = "g"
kind = "reg"
class = "gpr"
[[forms]]
name = "R"
opcode_field = "opcode"
operand_fields = ["rd"]
[emit]
align_pad = 0x90
[[instructions]]
name = "FOO"
form = "R"
opcode = 0x13
asm = "foo {0:[g:out]}"
"#;
    let model = parse_and_validate(doc).unwrap();
    assert_eq!(model.emit.as_ref().unwrap().align_pad, Some(0x90));
    let ts = super::codegen::generate(&model).unwrap();
    let s = ts.to_string();
    // .align 伪指令分支存在于生成的 parse_insts（填充字节值已在模型层断言）
    assert!(
        s.contains(r#""align" =>"#),
        "生成的 parse_insts 应有 .align 分支：{s}"
    );
    // 填充字节 0x90 = 144 进入生成代码
    assert!(
        s.contains("144"),
        "align_pad 0x90 应进入生成的填充代码：{s}"
    );
}

#[test]
fn codegen_scatter_pieces_rejected_in_single_contexts() {
    // 散布位段用于 opcode_field → codegen 报错（迭代 2 约束）
    let doc = r#"
[meta]
name = "x"
default_inst_width = 32
[reg.gpr4]
count = 8
[conventions.bitfields]
rd     = { offset = 7,  width = 3 }
opcode = { pieces = [ { offset = 0, width = 7, shift = 0 } ] }
[[operand_slots]]
name = "g"
kind = "reg"
class = "gpr"
[[forms]]
name = "R"
opcode_field = "opcode"
operand_fields = ["rd"]
[[instructions]]
name = "FOO"
form = "R"
opcode = 0x33
asm = "foo {0:[g:out]}"
"#;
    let model = parse_and_validate(doc).unwrap();
    let err = super::codegen::generate(&model).unwrap_err();
    assert!(err.contains("scattered"), "err: {err}");
}

#[test]
fn codegen_generates_core_surface() {
    // 定宽模型 → 生成模块含 Inst/encode/decode/disassemble/assemble
    let doc = r#"
[meta]
name = "x"
default_inst_width = 32
[reg.gpr4]
count = 8
[conventions.bitfields]
rd     = { offset = 7,  width = 3 }
rs1    = { offset = 15, width = 3 }
opcode = { offset = 0,  width = 7 }
funct3 = { offset = 12, width = 3 }
imm12  = { offset = 20, width = 12 }
[[operand_slots]]
name = "g"
kind = "reg"
class = "gpr"
[[operand_slots]]
name = "i"
kind = "imm"
signed = true
width = 12
[[forms]]
name = "I"
opcode_field = "opcode"
operand_fields = ["rd", "rs1", "imm12"]
[[instructions]]
name = "ADDI"
form = "I"
opcode = 0x13
fields = { funct3 = 0 }
asm = "addi {0:[g:out]}, {1:[g:in]}, {2:[i:in]}"
[[instructions]]
name = "NOP"
form = "I"
opcode = 0x13
  asm = "nop"
"#;
    let model = parse_and_validate(doc).unwrap();
    let ts = super::codegen::generate(&model).unwrap();
    let s = ts.to_string();
    for needle in [
        "pub enum Inst",
        "Inst :: Addi",
        "pub fn encode",
        "pub fn decode",
        "pub fn disassemble",
        "pub fn assemble",
        "Addi {",
        "imm12",
    ] {
        assert!(s.contains(needle), "generated code missing '{needle}'");
    }
    // 确定性：两次生成一致
    let ts2 = super::codegen::generate(&model).unwrap();
    assert_eq!(
        ts.to_string(),
        ts2.to_string(),
        "generation must be deterministic"
    );
}

// ───────────────── 结构完善：asm 完整格式 + 通用模板段 ─────────────────

/// 定宽最小模型（供 codegen 测试）。
fn gen_min_model(inst_body: &str) -> super::model::V12Model {
    let doc = format!(
        r#"
[meta]
name = "x"
default_inst_width = 32
[reg.gpr4]
count = 8
[conventions.bitfields]
opcode = {{ offset = 0, width = 7 }}
rd = {{ offset = 7, width = 3 }}
rs1 = {{ offset = 15, width = 3 }}
imm12 = {{ offset = 20, width = 12 }}
funct3 = {{ offset = 12, width = 3 }}
[[operand_slots]]
name = "g"
kind = "reg"
class = "gpr"
[[operand_slots]]
name = "i"
kind = "imm"
signed = true
width = 12
[[forms]]
name = "I"
opcode_field = "opcode"
operand_fields = ["rd", "rs1", "imm12"]
{inst_body}
"#
    );
    parse_and_validate(&doc).unwrap()
}

#[test]
fn asm_full_format_extracts_mnemonic() {
    // asm = 完整格式：首词即 mnemonic；disassemble 直接含完整格式
    let model = gen_min_model(
        r#"
[[instructions]]
name = "ADDI"
form = "I"
opcode = 0x13
fields = { funct3 = 0 }
asm = "addi {0:[g:out]}, {1:[g:in]}, {2:[i:in]}"
"#,
    );
    let ts = super::codegen::generate(&model).unwrap();
    let s = ts.to_string();
    // disassemble 渲染完整格式（mnemonic + 段）
    assert!(
        s.contains(r#""addi {__o0}, {__o1}, {__o2}""#),
        "disassemble 应含完整格式：{s}"
    );
    // assemble 按 mnemonic 匹配
    assert!(
        s.contains(r#""addi" =>"#),
        "assemble 应匹配 mnemonic 'addi'"
    );
}

#[test]
fn asm_required() {
    // asm 必填（v12.1：助记符唯一事实来源 = asm 首词，操作数内联声明）
    let doc = r#"
[meta]
name = "x"
default_inst_width = 32
[reg.gpr4]
count = 8
[conventions.bitfields]
opcode = { offset = 0, width = 7 }
rd = { offset = 7, width = 3 }
[[operand_slots]]
name = "g"
kind = "reg"
class = "gpr"
[[forms]]
name = "R"
opcode_field = "opcode"
operand_fields = ["rd"]
[[instructions]]
name = "FOO"
form = "R"
opcode = 0x13
"#;
    let err = parse_and_validate(doc).unwrap_err();
    match err {
        // asm 缺失在 TOML 反序列化层报错（必填字段）
        V12Error::Parse { msg, .. } => {
            assert!(msg.contains("missing field `asm`"), "msg: {msg}");
        }
        other => panic!("expected Parse error, got {other:?}"),
    }
}
#[test]
fn validate_asm_mnemonic_extracted() {
    // asm 首词 = 助记符（唯一事实来源）；助记符含 '{' 拒绝
    let doc = r#"
[meta]
name = "x"
default_inst_width = 32
[reg.gpr4]
count = 8
[conventions.bitfields]
opcode = { offset = 0, width = 7 }
rd = { offset = 7, width = 3 }
[[operand_slots]]
name = "g"
kind = "reg"
class = "gpr"
[[forms]]
name = "R"
opcode_field = "opcode"
operand_fields = ["rd"]
[[instructions]]
name = "FOO"
form = "R"
opcode = 0x13
asm = "{bad {0:[g:out]}"
"#;
    let err = parse_and_validate(doc).unwrap_err();
    match err {
        V12Error::Validation { msg, .. } => {
            assert!(msg.contains("mnemonic"), "msg: {msg}");
        }
        other => panic!("expected Validation error, got {other:?}"),
    }
}
#[test]
fn generic_template_segments() {
    // 通用模板：任意字面片段（byte ptr / 括号 / 方括号）作为段组合
    let model = gen_min_model(
        r#"
[[instructions]]
name = "LDB"
form = "I"
opcode = 0x03
fields = { funct3 = 0 }
asm = "ldb {0:[g:out]}, byte ptr [{1:[g:in]}+{2:[i:in]}]"
"#,
    );
    let ts = super::codegen::generate(&model).unwrap();
    let s = ts.to_string();
    // 字面段原样输出
    assert!(
        s.contains(r#""ldb {__o0}, byte ptr [{__o1}+{__o2}]""#),
        "通用字面段应原样渲染：{s}"
    );
    // 段解析：字面段原样进入生成的匹配/输出（token 流转字符串含空格，放宽匹配）
    assert!(s.contains("byte ptr ["), "通用字面段应进入生成的代码：{s}");
    assert!(s.contains('['), "占位符方括号字面段");
}

#[test]
fn generic_template_adjacent_placeholder_rejected() {
    // 连续占位符无字面分隔 → 歧义，codegen 报错
    let model = gen_min_model(
        r#"
[[instructions]]
name = "BAD"
form = "I"
opcode = 0x13
fields = { funct3 = 0 }
asm = "bad {0:[g:out]}{1:[g:in]} {2:[i:in]}"
"#,
    );
    let err = super::codegen::generate(&model).unwrap_err();
    assert!(err.contains("adjacent placeholders"), "err: {err}");
}

#[test]
fn generic_template_out_of_range_rejected() {
    // 占位符索引不连续（{0}, {3}）→ validate 报错（操作数序号必须 0..k）
    let doc = r#"
[meta]
name = "x"
default_inst_width = 32
[reg.gpr4]
count = 8
[conventions.bitfields]
opcode = { offset = 0, width = 7 }
rd = { offset = 7, width = 3 }
rs1 = { offset = 15, width = 3 }
imm12 = { offset = 20, width = 12 }
[[operand_slots]]
name = "g"
kind = "reg"
class = "gpr"
[[operand_slots]]
name = "i"
kind = "imm"
signed = true
width = 12
[[forms]]
name = "I"
opcode_field = "opcode"
operand_fields = ["rd", "rs1", "imm12"]
[[instructions]]
name = "BAD"
form = "I"
opcode = 0x13
fields = { funct3 = 0 }
asm = "bad {0:[g:out]}, {3:[g:in]}"
"#;
    let err = parse_and_validate(doc).unwrap_err();
    match err {
        V12Error::Validation { msg, .. } => {
            assert!(msg.contains("contiguous"), "msg: {msg}");
        }
        other => panic!("expected Validation error, got {other:?}"),
    }
}

// ─────────────────────── global_reloc（P2 模型化字段）───────────────────────

/// `global_reloc = "pcrel_hi"` 合法解析。
#[test]
fn global_reloc_pcrel_parses() {
    let doc = r#"
[meta]
name = "t"
default_inst_width = 32
[reg.gpr4]
count = 8
[[operand_slots]]
name = "g"
kind = "reg"
class = "gpr4"
[[operand_slots]]
name = "imm20"
kind = "imm"
width = 20
[conventions.bitfields]
rd = { offset = 7, width = 5 }
opcode = { offset = 0, width = 7 }
imm20 = { pieces = [ { offset = 12, width = 20, shift = 12 } ] }
[[forms]]
name = "U"
opcode_field = "opcode"
operand_fields = ["rd", "imm20"]
[[instructions]]
name = "AUIPC_GLOBAL"
form = "U"
opcode = 0x17
asm = "auipc.g {0:[g:out]}, {1:[imm20:in]}"
global_reloc = "pcrel_hi"
"#;
    let m = parse_and_validate(doc).expect("valid doc with global_reloc must parse");
    let inst = m
        .instructions
        .iter()
        .find(|i| i.name == "AUIPC_GLOBAL")
        .expect("instruction present");
    assert_eq!(inst.global_reloc, Some(crate::v12::model::GlobalReloc::PcrelHi));
}

/// `global_reloc = "bogus"` → 校验拒绝。
#[test]
fn global_reloc_invalid_rejected() {
    let doc = r#"
[meta]
name = "t"
default_inst_width = 32
[reg.gpr4]
count = 8
[[operand_slots]]
name = "g"
kind = "reg"
class = "gpr4"
[[operand_slots]]
name = "imm20"
kind = "imm"
width = 20
[conventions.bitfields]
rd = { offset = 7, width = 5 }
opcode = { offset = 0, width = 7 }
imm20 = { pieces = [ { offset = 12, width = 20, shift = 12 } ] }
[[forms]]
name = "U"
opcode_field = "opcode"
operand_fields = ["rd", "imm20"]
[[instructions]]
name = "BAD"
form = "U"
opcode = 0x17
asm = "bad {0:[g:out]}, {1:[imm20:in]}"
global_reloc = "bogus"
"#;
    let err = parse_and_validate(doc).unwrap_err();
    // S1：global_reloc 改枚举后由 serde 在反序列化期拒绝——错误更早、且自带
    // 候选列表与 TOML 行号（原先是手写 validate 分支）。
    match err {
        V12Error::Parse { msg, line, .. } => {
            assert!(msg.contains("bogus"), "msg: {msg}");
            assert!(msg.contains("abs8"), "需列出候选: {msg}");
            assert_eq!(line, 28, "行号指向 global_reloc 那一行");
        }
        other => panic!("expected Parse error, got {other:?}"),
    }
}

/// `effect = ["Move"]` 语义标签解析（is_move 声明，替代指令名前缀启发式）。
#[test]
fn effect_move_label_parses() {
    let doc = r#"
[meta]
name = "t"
default_inst_width = 32
[reg.gpr4]
count = 8
[[operand_slots]]
name = "g"
kind = "reg"
class = "gpr4"
[conventions.bitfields]
rd = { offset = 7, width = 5 }
rs1 = { offset = 15, width = 5 }
rs2 = { offset = 20, width = 5 }
opcode = { offset = 0, width = 7 }
funct3 = { offset = 12, width = 3 }
funct7 = { offset = 25, width = 7 }
[[forms]]
name = "R"
opcode_field = "opcode"
operand_fields = ["rd", "rs1", "rs2"]
[[instructions]]
name = "MY_MOV"
form = "R"
opcode = 0x33
fields = { funct3 = 0, funct7 = 0 }
asm = "mymov {0:[g:out]}, {1:[g:in]}, {2:[g:in]}"
effect = ["Move"]
"#;
    let m = parse_and_validate(doc).expect("doc with Move effect must parse");
    let inst = m.instructions.iter().find(|i| i.name == "MY_MOV").unwrap();
    assert!(inst.effect.contains(&crate::v12::model::Effect::Move));
}

/// 占位符注册表唯一性：无重名 token、无重复临时变量（第三轮重构核心——
/// 4 处消费（ctor/token_kind/临时声明/xreg 绑定）都从该表派生，表必须一致）。
#[test]
fn placeholder_registry_unique() {
    super::codegen::placeholder::assert_unique().expect("占位符注册表必须唯一");
}

/// 占位符注册表关键条目可查（防止重构误删符号寄存器/临时/常量 token）。
#[test]
fn placeholder_registry_lookup() {
    use super::codegen::placeholder::{PhKind, PhTemp, lookup};
    // 符号寄存器（静态表：out/out2；{N} 是动态编号操作数）
    for name in ["{out}", "{out2}"] {
        let p = lookup(name).unwrap_or_else(|| panic!("{name} 必须在注册表"));
        assert_eq!(p.token_kind, "reg", "{name} token_kind");
        assert!(p.loose, "{name} 应 loose（任何槽可用）");
    }
    // 编号操作数 {N}：动态解析，任意上限（不只 {0}/{1}/{2}）
    for (name, idx) in [
        ("{0}", 0usize),
        ("{1}", 1),
        ("{2}", 2),
        ("{3}", 3),
        ("{7}", 7),
    ] {
        let n = super::codegen::placeholder::numbered_operand(name)
            .unwrap_or_else(|| panic!("{name} 必须按编号操作数解析"));
        assert_eq!(n, idx, "{name} 编号");
        assert_eq!(
            super::codegen::placeholder::token_kind(name),
            "reg",
            "{name} token_kind"
        );
    }
    // 临时（静态表：{g}/{f}；编号 {gN}/{fN} 动态）
    for (name, var, cls) in [("{g}", "__g", PhTemp::Gpr), ("{f}", "__f", PhTemp::Fpr)] {
        let p = lookup(name).unwrap_or_else(|| panic!("{name} 必须在注册表"));
        assert_eq!(p.temp, Some(var), "{name} 临时变量名");
        assert_eq!(p.temp_class, Some(cls), "{name} 临时类别");
    }
    // 编号临时 {gN}/{fN}：动态解析，任意上限；g=GPR、f=FPR 与 PhTemp 对应
    for (name, var, cls) in [
        ("{g1}", "__g1", PhTemp::Gpr),
        ("{g5}", "__g5", PhTemp::Gpr),
        ("{g6}", "__g6", PhTemp::Gpr),
        ("{f1}", "__f1", PhTemp::Fpr),
        ("{f4}", "__f4", PhTemp::Fpr),
        ("{f9}", "__f9", PhTemp::Fpr),
    ] {
        let (v, c) = super::codegen::placeholder::numbered_temp(name)
            .unwrap_or_else(|| panic!("{name} 必须按编号临时解析"));
        assert_eq!(v, var, "{name} 变量名");
        assert_eq!(c, cls, "{name} 类别");
    }
    // 旧样式 {t}/{tN}/{t_f}/{t_fN} 已废弃：必须解析失败（防止误用回退）
    for name in ["{t}", "{t1}", "{t_f}", "{t_f1}"] {
        assert!(
            super::codegen::placeholder::lookup(name).is_none(),
            "{name} 旧样式必须不在静态表"
        );
        assert!(
            super::codegen::placeholder::numbered_temp(name).is_none(),
            "{name} 旧样式必须不可按编号临时解析"
        );
    }
    // 与既有占位符无前缀冲突：{g}≠{global}、{f}≠{fconst}（精确匹配）
    for name in ["{global}", "{fconst}", "{fconst_hi32_hi20}", "{iconst}"] {
        assert!(
            super::codegen::placeholder::lookup(name).is_some(),
            "{name} 静态条目必须仍在"
        );
    }
    // 常量/立即数
    for name in [
        "{iconst}", "{fconst}", "{off}", "{alloca}", "{global}", "{imm0}",
    ] {
        let p = lookup(name).unwrap_or_else(|| panic!("{name} 必须在注册表"));
        assert_eq!(p.kind, PhKind::Imm, "{name} 槽类别");
    }
    let cc = lookup("{cc}").unwrap();
    assert_eq!(cc.kind, PhKind::Cond, "cc 槽类别");
    // token 分类委托
    assert_eq!(super::codegen::placeholder::token_kind("{out}"), "reg");
    assert_eq!(
        super::codegen::placeholder::token_kind("{iconst_hi20}"),
        "imm"
    );
    assert_eq!(super::codegen::placeholder::token_kind("{cc}"), "cond");
    assert_eq!(super::codegen::placeholder::token_kind("RAX"), "reg");
    assert_eq!(super::codegen::placeholder::token_kind("16"), "num");
    assert_eq!(
        super::codegen::placeholder::token_kind("{0}+8"),
        "mem",
        "内存表达式保持 mem"
    );
}

// ─────────────── [[lowering]] 校验（S1 补齐：三类静默失效） ───────────────

/// 带一条完整 lowering 的最小 ISA，`{extra}` 处插入待测规则片段。
fn lowering_doc(rule: &str) -> String {
    format!(
        r#"
[meta]
name = "x"
default_inst_width = 32
[reg.gpr4]
count = 8
[conventions.bitfields]
opcode = {{ offset = 0, width = 8 }}
rd = {{ offset = 8, width = 3 }}
rs1 = {{ offset = 11, width = 3 }}
[[operand_slots]]
name = "g"
kind = "reg"
class = "gpr4"
roles = ["in", "out"]
[[forms]]
name = "RR"
opcode_field = "opcode"
operand_fields = ["rd", "rs1"]
[[instructions]]
name = "MOV"
form = "RR"
opcode = 1
asm = "mov {{0:[g:out]}}, {{1:[g:in]}}"
{rule}
"#
    )
}

fn lowering_err(rule: &str) -> String {
    match parse_and_validate(&lowering_doc(rule)).unwrap_err() {
        V12Error::Validation { msg, .. } => msg,
        other => panic!("expected Validation error, got {other:?}"),
    }
}

#[test]
fn lowering_accepts_declared_mnemonic() {
    let doc = lowering_doc("[[lowering]]\nop = \"Copy\"\ninsts = [\"mov {out}, {0}\"]");
    parse_and_validate(&doc).expect("已声明助记符 + 已知占位符必须通过");
}

#[test]
fn lowering_rejects_unknown_mnemonic() {
    let msg = lowering_err("[[lowering]]\nop = \"Copy\"\ninsts = [\"movv {out}, {0}\"]");
    assert!(msg.contains("未声明的助记符 'movv'"), "msg: {msg}");
    assert!(msg.contains("[[lowering.Copy]]"), "msg 需带声明路径: {msg}");
}

#[test]
fn lowering_rejects_unknown_placeholder() {
    // `{iconst_lo}` 少了 `12`：过去落 fallback 装成字面量，生成能编译但语义错的代码
    let msg = lowering_err("[[lowering]]\nop = \"Iconst\"\ninsts = [\"mov {out}, {iconst_lo}\"]");
    assert!(msg.contains("未知占位符 '{iconst_lo}'"), "msg: {msg}");
}

#[test]
fn lowering_rejects_unknown_when_attr() {
    // 未知属性 → pred::eval 恒假 → 规则永不命中（既不报错也不生效）
    let msg = lowering_err(
        "[[lowering]]\nop = \"Copy\"\nwhen = { eq = [\"rs1_widht\", 32] }\ninsts = [\"mov {out}, {0}\"]",
    );
    assert!(msg.contains("未知属性 'rs1_widht'"), "msg: {msg}");
    assert!(msg.contains("rs1_width"), "需列出可用属性: {msg}");
}

#[test]
fn lowering_rejects_exact_duplicate() {
    let rule = "[[lowering]]\nop = \"Copy\"\ninsts = [\"mov {out}, {0}\"]\n\
                [[lowering]]\nop = \"Copy\"\ninsts = [\"mov {out}, {0}\"]";
    let msg = lowering_err(rule);
    assert!(msg.contains("完全重复"), "msg: {msg}");
}

/// 同 op 同 insts 但 when 不同 → 合法（宽度/条件分派的正常形态）。
#[test]
fn lowering_allows_same_insts_with_different_when() {
    let rule = "[[lowering]]\nop = \"Copy\"\nwhen = { eq = [\"rs1_width\", 32] }\ninsts = [\"mov {out}, {0}\"]\n\
                [[lowering]]\nop = \"Copy\"\ninsts = [\"mov {out}, {0}\"]";
    parse_and_validate(&lowering_doc(rule)).expect("when 不同不算重复");
}

/// families 展开出的助记符也算已声明（`{name}` → 变体名小写）。
#[test]
fn lowering_accepts_family_mnemonic() {
    let rule = "[[families]]\nname = \"F\"\nform = \"RR\"\nasm = \"{name} {0:[g:out]}, {1:[g:in]}\"\n\
                [[families.variants]]\nname = \"NEG\"\nopcode = 9\n\
                [[lowering]]\nop = \"Ineg\"\ninsts = [\"neg {out}, {0}\"]";
    parse_and_validate(&lowering_doc(rule)).expect("family 变体助记符必须被识别");
}
