//! v12 迭代 1 验证：解析器单测（合法/非法 TOML 诊断）+ 语义校验 + 序列化往返
//! + v11 文件拒绝证明（不兼容的直接体现）。

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

[reg.gpr]
width = 64
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
class = "gpr"
field_width = 5
roles = ["in", "out"]

[[operand_slots]]
name = "imm12"
kind = "imm"
signed = true
width = 12

[[forms]]
name = "R"
opcode_field = "opcode"
operand_slots = ["gpr", "gpr", "gpr"]

[[instructions]]
name = "ADD"
form = "R"
opcode = 0x33
fields = { funct3 = 0, funct7 = 0 }
operands = [
  { slot = "gpr", role = "out", field = "rd" },
  { slot = "gpr", role = "in",  field = "rs1" },
  { slot = "gpr", role = "in",  field = "rs2" },
]
mnemonic = "add"
"#;

#[test]
fn parse_minimal_riscv_style() {
    let m = parse_and_validate(RISCV_DOC).expect("valid riscv-style doc must parse");
    assert_eq!(m.meta.name, "riscv64_v12");
    assert_eq!(m.meta.default_inst_width, Some(32));
    assert!(!m.meta.variable_length);
    assert_eq!(m.reg["gpr"].width, 64);
    assert_eq!(m.reg["gpr"].names.as_ref().unwrap().len(), 32);
    assert_eq!(m.conventions.bitfields.len(), 6);
    assert_eq!(m.conventions.bitfields["rd"].offset, Some(7));
    assert_eq!(m.conventions.bitfields["rd"].width, Some(5));
    assert_eq!(m.operand_slots.len(), 2);
    assert_eq!(m.operand_slots[0].kind, OperandKind::Reg);
    assert_eq!(m.operand_slots[0].class.as_deref(), Some("gpr"));
    assert_eq!(m.operand_slots[1].kind, OperandKind::Imm);
    assert_eq!(m.operand_slots[1].signed, Some(true));
    let form = &m.forms[0];
    assert_eq!(form.name, "R");
    assert_eq!(form.opcode_field.as_deref(), Some("opcode"));
    let add = &m.instructions[0];
    assert_eq!(add.opcode, Some(0x33));
    assert_eq!(add.fields.as_ref().unwrap()["funct3"], 0);
    assert_eq!(add.operands[0].field.as_deref(), Some("rd"));
    assert_eq!(add.operands[0].role, Some(OperandRole::Out));
    assert_eq!(add.mnemonic.as_deref(), Some("add"));
}

const X86_DOC: &str = r#"
[meta]
name = "x86_64"
version = "12.0"
endian = "little"
mode = 64
variable_length = true
max_inst_len = 15

[reg.gpr64]
width = 64
names = ["RAX", "RCX", "RDX", "RBX", "RSP", "RBP", "RSI", "RDI",
         "R8", "R9", "R10", "R11", "R12", "R13", "R14", "R15"]

[reg.xmm]
width = 128
count = 16
prefix = "XMM"

[conventions.bitfields]
modrm_reg = { offset = 8, width = 3 }
modrm_rm  = { offset = 0, width = 3 }

[conventions.modrm]
reg_field = "modrm_reg"
rm_field = "modrm_rm"
force_disp_base = [5, 13]

[conventions.rex]
w_opsize = 64

[conventions.opsize_prefix]
16 = 0x66

[[operand_slots]]
name = "gpr"
kind = "reg"
class = "gpr64"
field_width = 4

[[operand_slots]]
name = "fpr"
kind = "reg"
class = "xmm"
field_width = 4

[[operand_slots]]
name = "imm32"
kind = "imm"
signed = true
width = 32

[[forms]]
name = "RR"
modrm = "rr"
rex = "auto"
prefix = "opsize"
opcode_bytes = 1
operand_slots = ["gpr", "gpr"]
"#;

#[test]
fn parse_x86_conventions() {
    let m = parse_and_validate(X86_DOC).expect("valid x86-style doc must parse");
    assert!(m.meta.variable_length);
    assert_eq!(m.meta.max_inst_len, Some(15));
    // 生成式寄存器组：count + prefix
    assert_eq!(m.reg["xmm"].count, Some(16));
    assert_eq!(m.reg["xmm"].prefix.as_deref(), Some("XMM"));
    // ModRM 约定
    let modrm = m.conventions.modrm.as_ref().expect("modrm present");
    assert_eq!(modrm.reg_field, "modrm_reg");
    assert_eq!(modrm.rm_field, "modrm_rm");
    assert_eq!(modrm.force_disp_base, vec![5, 13]);
    // REX 约定
    assert_eq!(m.conventions.rex.as_ref().unwrap().w_opsize, Some(64));
    // 操作数宽度前缀：TOML 裸整数键 "16" → 0x66
    let p = m
        .conventions
        .opsize_prefix
        .as_ref()
        .expect("opsize_prefix present");
    assert_eq!(p.get("16"), Some(&0x66));
    // 形式语义键
    let form = &m.forms[0];
    assert_eq!(form.modrm.as_deref(), Some("rr"));
    assert_eq!(form.rex.as_deref(), Some("auto"));
    assert_eq!(form.prefix.as_deref(), Some("opsize"));
    assert_eq!(form.opcode_bytes, Some(1));
    assert_eq!(
        form.operand_slots.as_ref().unwrap(),
        &vec!["gpr".to_string(), "gpr".to_string()]
    );
}

#[test]
fn default_flags_and_roles() {
    let doc = r#"
[meta]
name = "x"
[reg.gpr]
width = 32
count = 8
[[operand_slots]]
name = "g"
kind = "reg"
class = "gpr"
field_width = 3
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
    // 往返后 opsize_prefix 内容不变（键可能带引号，解析等价）
    assert_eq!(
        m2.conventions.opsize_prefix.as_ref().unwrap().get("16"),
        Some(&0x66)
    );
}

/// inout 角色：读改写操作数（x86 ADD RM, R 的 RM），模板只需声明一次。
#[test]
fn inout_role_parses() {
    let doc = r#"
[meta]
name = "x"
[reg.gpr]
width = 32
count = 8
[[operand_slots]]
name = "rm"
kind = "reg"
class = "gpr"
field_width = 3
roles = ["inout"]
[[forms]]
name = "RM"
modrm = "rm"
[[instructions]]
name = "ADD_RM_R"
form = "RM"
opcode = 0x01
operands = [
  { slot = "rm", role = "inout" },
  { slot = "rm", role = "in" },
]
"#;
    let m = parse_and_validate(doc).expect("valid");
    // 槽能力声明：inout = 读改写
    assert_eq!(
        m.operand_slots[0].roles.as_deref(),
        Some(&[OperandRole::InOut][..])
    );
    // 指令操作数角色
    assert_eq!(m.instructions[0].operands[0].role, Some(OperandRole::InOut));
    assert_eq!(m.instructions[0].operands[1].role, Some(OperandRole::In));
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
[reg.gpr]
width = 32
count = 8
"#;
    let err = parse(doc).unwrap_err();
    match err {
        V12Error::Parse(msg) => {
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
[reg.gpr]
width = 32
count = 8
"#;
    let err = parse(doc).unwrap_err();
    match err {
        V12Error::Parse(msg) => assert!(msg.contains("no_default_lowering"), "msg: {msg}"),
        other => panic!("expected Parse error, got {other:?}"),
    }
}

#[test]
fn rejects_unknown_operand_slot_key() {
    let doc = r#"
[meta]
name = "x"
[reg.gpr]
width = 32
count = 8
[[operand_slots]]
name = "g"
kind = "reg"
class = "gpr"
field_width = 3
garbage = 1
"#;
    let err = parse(doc).unwrap_err();
    match err {
        V12Error::Parse(msg) => {
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
[reg.gpr]
width = 32
count = 8
[[operand_slots]]
name = "g"
kind = "float"
"#;
    let err = parse(doc).unwrap_err();
    match err {
        V12Error::Parse(msg) => assert!(msg.contains("float"), "msg: {msg}"),
        other => panic!("expected Parse error, got {other:?}"),
    }
}

/// 不兼容证明：v11 主 ISA 文件必须被 v12 解析器拒绝。
///
/// 注：toml crate 按字母序迭代表（BTreeMap），`[abi]` 排在 `[meta]` 之前，
/// 所以首个报错是 v11 的 `[abi.arg_regs]` 而非 meta 的 `no_default_lowering`。
#[test]
fn rejects_v11_isa_file() {
    let v11 = include_str!("../../../../../isa/x86_v10.toml");
    let err = parse(v11).expect_err("v11 file must NOT parse under v12");
    match err {
        V12Error::Parse(msg) => {
            assert!(msg.contains("unknown field"), "msg: {msg}");
            assert!(msg.contains("arg_regs"), "msg: {msg}");
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
[reg.gpr]
width = 32
count = 8
[[operand_slots]]
name = "g"
kind = "reg"
class = "gpr"
field_width = 3
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
field_width = 3
"#;
    match parse(no_reg).unwrap_err() {
        V12Error::Parse(msg) => assert!(msg.contains("missing field `reg`"), "msg: {msg}"),
        other => panic!("expected Parse error, got {other:?}"),
    }
    let no_slots = r#"
[meta]
name = "x"
[reg.gpr]
width = 32
count = 8
"#;
    match parse(no_slots).unwrap_err() {
        V12Error::Parse(msg) => {
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
field_width = 3
"#;
    let err = parse_and_validate(doc).unwrap_err();
    match err {
        V12Error::Validation(msg) => assert!(msg.contains("missing [reg.*]"), "msg: {msg}"),
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
[reg.gpr]
width = 32
count = 8
"#;
    let err = parse_and_validate(doc).unwrap_err();
    match err {
        V12Error::Validation(msg) => {
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
        V12Error::Validation(msg) => {
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
        V12Error::Validation(msg) => {
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
[reg.gpr]
width = 32
count = 8
[[operand_slots]]
name = "g"
kind = "reg"
class = "gpr"
field_width = 3
[[forms]]
name = "R"
[[instructions]]
name = "NOP"
form = "ZZZ"
"#;
    let err = parse_and_validate(doc).unwrap_err();
    match err {
        V12Error::Validation(msg) => {
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
[reg.gpr]
width = 32
count = 8
[[operand_slots]]
name = "g"
kind = "reg"
class = "gpr"
field_width = 3
[[forms]]
name = "R"
[[instructions]]
name = "NOP"
form = "R"
operands = [{ slot = "nope" }]
"#;
    let err = parse_and_validate(doc).unwrap_err();
    match err {
        V12Error::Validation(msg) => {
            assert!(msg.contains("'nope'"), "msg: {msg}");
        }
        other => panic!("expected Validation error, got {other:?}"),
    }
}

#[test]
fn validation_instruction_unknown_field() {
    let doc = r#"
[meta]
name = "x"
[reg.gpr]
width = 32
count = 8
[[operand_slots]]
name = "g"
kind = "reg"
class = "gpr"
field_width = 3
[[forms]]
name = "R"
[[instructions]]
name = "NOP"
form = "R"
operands = [{ slot = "g", field = "zzz" }]
"#;
    let err = parse_and_validate(doc).unwrap_err();
    match err {
        V12Error::Validation(msg) => {
            assert!(msg.contains("'zzz'"), "msg: {msg}");
        }
        other => panic!("expected Validation error, got {other:?}"),
    }
}

#[test]
fn validation_duplicate_instruction() {
    let doc = r#"
[meta]
name = "x"
[reg.gpr]
width = 32
count = 8
[[operand_slots]]
name = "g"
kind = "reg"
class = "gpr"
field_width = 3
[[forms]]
name = "R"
[[instructions]]
name = "NOP"
form = "R"
[[instructions]]
name = "NOP"
form = "R"
"#;
    let err = parse_and_validate(doc).unwrap_err();
    match err {
        V12Error::Validation(msg) => {
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
[reg.gpr]
width = 32
count = 8
[[operand_slots]]
name = "g"
kind = "reg"
class = "gpr"
field_width = 3
[[forms]]
name = "R"
[[families]]
name = "F"
form = "R"
[[families.variants]]
name = "V1"
"#;
    let err = parse_and_validate(doc).unwrap_err();
    match err {
        V12Error::Validation(msg) => {
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
[reg.gpr]
width = 32
names = ["R0", "R0"]
[[operand_slots]]
name = "g"
kind = "reg"
class = "gpr"
field_width = 3
"#;
    let err = parse_and_validate(doc).unwrap_err();
    match err {
        V12Error::Validation(msg) => {
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
[reg.gpr]
width = 32
count = 8
[[operand_slots]]
name = "g"
kind = "reg"
class = "gpr"
field_width = 3
"#;
    let err = parse_and_validate(doc).unwrap_err();
    match err {
        V12Error::Validation(msg) => {
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
[reg.gpr]
width = 32
count = 8
[[operand_slots]]
name = "g"
kind = "reg"
class = "gpr"
field_width = 3
"#;
    let err = parse_and_validate(doc).unwrap_err();
    match err {
        V12Error::Validation(msg) => {
            assert!(msg.contains("not a valid ISA name"), "msg: {msg}");
        }
        other => panic!("expected Validation error, got {other:?}"),
    }
}

// ─────────────────────── 迭代 2：codegen ───────────────────────

#[test]
fn codegen_scatter_pieces_rejected_in_single_contexts() {
    // 散布位段用于 opcode_field → codegen 报错（迭代 2 约束）
    let doc = r#"
[meta]
name = "x"
default_inst_width = 32
[reg.gpr]
width = 32
count = 8
[conventions.bitfields]
rd     = { offset = 7,  width = 3 }
opcode = { pieces = [ { offset = 0, width = 7, shift = 0 } ] }
[[operand_slots]]
name = "g"
kind = "reg"
class = "gpr"
field_width = 3
[[forms]]
name = "R"
opcode_field = "opcode"
operand_fields = ["rd"]
[[instructions]]
name = "FOO"
form = "R"
opcode = 0x33
operands = [{ slot = "g", role = "out", field = "rd" }]
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
[reg.gpr]
width = 32
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
field_width = 3
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
operands = [{ slot = "g", role = "out" }, { slot = "g", role = "in" }, { slot = "i", role = "in" }]
[[instructions]]
name = "NOP"
form = "I"
opcode = 0x13
operands = []
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
