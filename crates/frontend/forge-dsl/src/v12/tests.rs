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
ops = ["dst:gpr:out", "src:gpr", "src2:gpr"]
asm = "add {dst}, {src}, {src2}"
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
    assert_eq!(form.keys.opcode_field.as_deref(), Some("opcode"));
    let add = &m.instructions[0];
    assert_eq!(add.opcode, Some(0x33));
    assert_eq!(add.fields.as_ref().unwrap()["funct3"], 0);
    assert_eq!(add.asm.as_str(), "add {dst}, {src}, {src2}");
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

    assert_eq!(form.keys.rex.as_deref(), Some("auto"));
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
[[instructions]]
name = "ADD_RM_R"
form = "RM"
modrm = { reg = "src", rm = "dst" }
opcode = 0x01
ops = ["dst:rm:inout", "src:rm"]
asm = "add {dst}, {src}"
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
ops = ["dst:nope:out"]
asm = "nop {dst}"
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
ops = ["dst:g:out", "src:g"]
asm = "foo {dst}, {src}"
"#;
    let err = parse_and_validate(doc).unwrap_err();
    match err {
        V12Error::Validation { msg, .. } => {
            assert!(msg.contains("exceed operand_fields"), "msg: {msg}");
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
ops = ["dst:g:out"]
asm = "f {dst}"
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
ops = ["dst:g:out"]
asm = "foo {dst}"
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
ops = ["dst:g:out"]
asm = "foo {dst}"
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
ops = ["dst:g:out"]
asm = "foo {dst}"
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
ops = ["dst:g:out", "src:g", "src2:i"]
asm = "addi {dst}, {src}, {src2}"
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
fn asm_full_template_renders_and_scans() {
    // asm = 完整模板：disassemble 直接渲染整条 asm；assemble 左→右整模板扫描
    let model = gen_min_model(
        r#"
[[instructions]]
name = "ADDI"
form = "I"
opcode = 0x13
fields = { funct3 = 0 }
ops = ["dst:g:out", "src:g", "src2:i"]
asm = "addi {dst}, {src}, {src2}"
"#,
    );
    let ts = super::codegen::generate(&model).unwrap();
    let s = ts.to_string();
    // disassemble 渲染完整格式（前导字面 + 段）
    assert!(
        s.contains(r#""addi {__o0}, {__o1}, {__o2}""#),
        "disassemble 应含完整格式：{s}"
    );
    // assemble 左→右整模板扫描：前导字面（助记符位）大小写豁免匹配
    assert!(
        s.contains("__eat_name"),
        "assemble 应发射前导字面大小写豁免匹配器 __eat_name：{s}"
    );
    // 不再有 `match 助记符` 分派
    assert!(
        !s.contains("unknown mnemonic"),
        "assemble 不应再有助记符分派错误：{s}"
    );
}

#[test]
fn operand_first_asm_scans_without_mnemonic() {
    // v17：asm 可操作数前置（无前导助记符字面）——首段即操作数解析，整模板
    // 左→右扫描不再依赖"首词 = 助记符"。disassemble 渲染操作数在前的完整格式。
    let doc = r#"
[meta]
name = "x"
default_inst_width = 32
[reg.gpr4]
count = 8
[conventions.bitfields]
opcode = { offset = 0, width = 8 }
rd = { offset = 8, width = 3 }
rs1 = { offset = 11, width = 3 }
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
name = "SWAP"
form = "RR"
opcode = 1
ops = ["dst:g:out", "src:g"]
asm = "{dst} = {src}"
"#;
    let model = parse_and_validate(doc).unwrap();
    let ts = super::codegen::generate(&model).unwrap();
    let s = ts.to_string();
    // 无前导助记符字面 → 不发射 __eat_name 的**调用**（整模板扫描从操作数解析
    // 开始；`__eat_name` 函数定义仍无条件发射，故只断言无调用点）
    assert!(
        !s.contains("__eat_name(& mut it"),
        "操作数前置 asm 不应发射 __eat_name 调用：{s}"
    );
    // disassemble 渲染操作数前置的完整格式
    assert!(
        s.contains(r#""{__o0} = {__o1}""#),
        "disassemble 应渲染操作数前置格式：{s}"
    );
}

#[test]
fn missing_asm_is_parse_error() {
    // asm 必填（v17）：缺省不再自动派生，缺 asm → Parse 错误
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
name = "ADD"
form = "R"
opcode = 0x13
ops = ["dst:g:out"]
"#;
    let err = parse_and_validate(doc).expect_err("缺 asm 必须报错");
    assert!(
        matches!(err, crate::v12::V12Error::Parse { .. }),
        "缺 asm 应为 Parse 错误，得到 {err:?}"
    );
}
#[test]
fn validate_asm_placeholder_must_be_declared() {
    // asm 里的 `{...}` 必须是已声明的操作数名（v17：前导字面不再特殊，助记符
    // 概念已消解——含 `{` 的前导字面同样按占位符规则拒绝）。
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
ops = ["dst:g:out"]
asm = "{bad {dst}"
"#;
    let err = parse_and_validate(doc).unwrap_err();
    match err {
        V12Error::Validation { msg, .. } => {
            assert!(msg.contains("占位符"), "msg: {msg}");
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
ops = ["dst:g:out", "src:g", "src2:i"]
asm = "ldb {dst}, byte ptr [{src}+{src2}]"
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
ops = ["dst:g:out", "src:g", "src2:i"]
asm = "bad {dst}{src} {src2}"
"#,
    );
    let err = super::codegen::generate(&model).unwrap_err();
    assert!(err.contains("adjacent placeholders"), "err: {err}");
}

#[test]
fn generic_template_out_of_range_rejected() {
    // v15：模板里的 `{名字}` 必须在 ops 里声明过——拼错的名字不再静默落
    // fallback（v14 是索引不连续报错，命名之后错误更直接）
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
ops = ["dst:g:out", "src:g"]
asm = "bad {dst}, {srcc}"
"#;
    let err = parse_and_validate(doc).unwrap_err();
    match err {
        V12Error::Validation { msg, .. } => {
            assert!(msg.contains("'{srcc}'"), "msg: {msg}");
            assert!(msg.contains("dst, src"), "需列出已声明的名字: {msg}");
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
ops = ["dst:g:out", "imm:imm20"]
asm = "auipc.g {dst}, {imm}"
global_reloc = "pcrel_hi"
"#;
    let m = parse_and_validate(doc).expect("valid doc with global_reloc must parse");
    let inst = m
        .instructions
        .iter()
        .find(|i| i.name == "AUIPC_GLOBAL")
        .expect("instruction present");
    assert_eq!(
        inst.global_reloc,
        Some(crate::v12::model::GlobalReloc::PcrelHi)
    );
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
ops = ["dst:g:out", "imm:imm20"]
asm = "bad {dst}, {imm}"
global_reloc = "bogus"
"#;
    let err = parse_and_validate(doc).unwrap_err();
    // S1：global_reloc 改枚举后由 serde 在反序列化期拒绝——错误更早、且自带
    // 候选列表与 TOML 行号（原先是手写 validate 分支）。
    match err {
        V12Error::Parse { msg, line, .. } => {
            assert!(msg.contains("bogus"), "msg: {msg}");
            assert!(msg.contains("abs8"), "需列出候选: {msg}");
            assert_eq!(line, 29, "行号指向 global_reloc 那一行");
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
ops = ["dst:g:out", "src:g", "src2:g"]
asm = "mymov {dst}, {src}, {src2}"
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
ops = ["dst:g:out", "src:g"]
asm = "mov {{dst}}, {{src}}"
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
fn lowering_accepts_declared_name() {
    let doc = lowering_doc("[[lowering]]\nop = \"Copy\"\ninsts = [\"MOV {out}, {0}\"]");
    parse_and_validate(&doc).expect("已声明指令名 + 已知占位符必须通过");
}

#[test]
fn lowering_rejects_unknown_ref() {
    let msg = lowering_err("[[lowering]]\nop = \"Copy\"\ninsts = [\"MOVV {out}, {0}\"]");
    assert!(msg.contains("未声明的指令/别名 'MOVV'"), "msg: {msg}");
    assert!(msg.contains("[[lowering.Copy]]"), "msg 需带声明路径: {msg}");
}

#[test]
fn lowering_rejects_unknown_placeholder() {
    // `{iconst_lo}` 少了 `12`：过去落 fallback 装成字面量，生成能编译但语义错的代码
    let msg = lowering_err("[[lowering]]\nop = \"Iconst\"\ninsts = [\"MOV {out}, {iconst_lo}\"]");
    assert!(msg.contains("未知占位符 '{iconst_lo}'"), "msg: {msg}");
}

#[test]
fn lowering_rejects_unknown_when_attr() {
    // 未知属性 → pred::eval 恒假 → 规则永不命中（既不报错也不生效）
    let msg = lowering_err(
        "[[lowering]]\nop = \"Copy\"\nwhen = { eq = [\"rs1_widht\", 32] }\ninsts = [\"MOV {out}, {0}\"]",
    );
    assert!(msg.contains("未知属性 'rs1_widht'"), "msg: {msg}");
    assert!(msg.contains("rs1_width"), "需列出可用属性: {msg}");
}

#[test]
fn lowering_rejects_exact_duplicate() {
    let rule = "[[lowering]]\nop = \"Copy\"\ninsts = [\"MOV {out}, {0}\"]\n\
                [[lowering]]\nop = \"Copy\"\ninsts = [\"MOV {out}, {0}\"]";
    let msg = lowering_err(rule);
    assert!(msg.contains("完全重复"), "msg: {msg}");
}

/// 同 op 同 insts 但 when 不同 → 合法（宽度/条件分派的正常形态）。
#[test]
fn lowering_allows_same_insts_with_different_when() {
    let rule = "[[lowering]]\nop = \"Copy\"\nwhen = { eq = [\"rs1_width\", 32] }\ninsts = [\"MOV {out}, {0}\"]\n\
                [[lowering]]\nop = \"Copy\"\ninsts = [\"MOV {out}, {0}\"]";
    parse_and_validate(&lowering_doc(rule)).expect("when 不同不算重复");
}

/// families 展开出的变体指令名也算已声明（`{name}` → 变体名小写）。
#[test]
fn lowering_accepts_family_name() {
    let rule = "[[families]]\nname = \"F\"\nform = \"RR\"\n\
                ops = [\"dst:g:out\", \"src:g\"]\nasm = \"{name} {dst}, {src}\"\n\
                [[families.variants]]\nname = \"NEG\"\nopcode = 9\n\
                [[lowering]]\nop = \"Ineg\"\ninsts = [\"NEG {out}, {0}\"]";
    parse_and_validate(&lowering_doc(rule)).expect("family 变体指令名必须被识别");
}

// ─────────── S10e：显式 [[aliases]] 解析（多态 / 1:1 / 校验） ───────────

fn aliases_doc(extra: &str) -> String {
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
name = "MOV16"
form = "RR"
opcode = 1
ops = ["dst:g:out", "src:g"]
asm = "mov {{dst}}, {{src}}"
[[instructions]]
name = "MOV32"
form = "RR"
opcode = 2
ops = ["dst:g:out", "src:g"]
asm = "mov {{dst}}, {{src}}"
{extra}
"#
    )
}

#[test]
fn aliases_polymorphic_lowering_accepted() {
    // 别名指向多成员指令；lowering 首词引用别名名 → 解析通过（多态分派在 codegen 消歧）
    let doc = aliases_doc(
        "[[aliases]]\nname = \"mov\"\ninsts = [\"MOV16\", \"MOV32\"]\n\
         [[lowering]]\nop = \"Copy\"\ninsts = [\"mov {out}, {0}\"]",
    );
    parse_and_validate(&doc).expect("多态别名 + lowering 引用别名必须通过");
}

#[test]
fn aliases_1_to_1_lowering_accepted() {
    // 别名指向单成员指令（1:1）——等价于直接引用指令名，仍须被 lowering 接受
    let doc = aliases_doc(
        "[[aliases]]\nname = \"copy\"\ninsts = [\"MOV16\"]\n\
         [[lowering]]\nop = \"Copy\"\ninsts = [\"copy {out}, {0}\"]",
    );
    parse_and_validate(&doc).expect("1:1 别名 + lowering 引用别名必须通过");
}

#[test]
fn aliases_reject_conflict_with_inst_name() {
    let doc = aliases_doc("[[aliases]]\nname = \"MOV16\"\ninsts = [\"MOV32\"]");
    let msg = match parse_and_validate(&doc).unwrap_err() {
        V12Error::Validation { msg, .. } => msg,
        other => panic!("expected Validation error, got {other:?}"),
    };
    assert!(msg.contains("别名名与指令名冲突"), "msg: {msg}");
}

#[test]
fn aliases_reject_unknown_member() {
    let doc = aliases_doc("[[aliases]]\nname = \"mov\"\ninsts = [\"MOV16\", \"NOPE\"]");
    let msg = match parse_and_validate(&doc).unwrap_err() {
        V12Error::Validation { msg, .. } => msg,
        other => panic!("expected Validation error, got {other:?}"),
    };
    assert!(msg.contains("成员指令 'NOPE' 未声明"), "msg: {msg}");
}

#[test]
fn aliases_reject_empty_insts() {
    let doc = aliases_doc("[[aliases]]\nname = \"mov\"\ninsts = []");
    let msg = match parse_and_validate(&doc).unwrap_err() {
        V12Error::Validation { msg, .. } => msg,
        other => panic!("expected Validation error, got {other:?}"),
    };
    assert!(msg.contains("insts must not be empty"), "msg: {msg}");
}

#[test]
fn aliases_reject_duplicate_member() {
    let doc = aliases_doc("[[aliases]]\nname = \"mov\"\ninsts = [\"MOV16\", \"MOV16\"]");
    let msg = match parse_and_validate(&doc).unwrap_err() {
        V12Error::Validation { msg, .. } => msg,
        other => panic!("expected Validation error, got {other:?}"),
    };
    assert!(msg.contains("成员指令 'MOV16' 重复"), "msg: {msg}");
}

#[test]
fn aliases_reject_duplicate_name() {
    let doc = aliases_doc(
        "[[aliases]]\nname = \"mov\"\ninsts = [\"MOV16\"]\n\
         [[aliases]]\nname = \"mov\"\ninsts = [\"MOV32\"]",
    );
    let msg = match parse_and_validate(&doc).unwrap_err() {
        V12Error::Validation { msg, .. } => msg,
        other => panic!("expected Validation error, got {other:?}"),
    };
    assert!(msg.contains("别名名重复"), "msg: {msg}");
}

// ─────────── S2：vary 行表 / in 谓词 / 特异性裁决 / 死规则 ───────────

#[test]
fn vary_expands_rows_and_adds_predicates() {
    // elem 是谓词属性 → 每行追加 eq[elem, 值]；m 不是 → 纯替换
    let doc = lowering_doc(
        "[[lowering]]\nop = \"Vadd\"\nwhen = { eq = [\"rd\", 256] }\n\
         vary = { elem = [1, 2], m = [\"MOV\", \"MOV\"] }\ninsts = [\"{m} {out}, {0}\"]",
    );
    let m = parse_and_validate(&doc).expect("vary 必须展开");
    let rs: Vec<_> = m.lowering.iter().filter(|r| r.op == "Vadd").collect();
    assert_eq!(rs.len(), 2, "两行 → 两条具体规则");
    for r in &rs {
        assert!(r.vary.is_none(), "展开后 vary 必须清空");
        assert_eq!(r.insts, vec!["MOV {out}, {0}"], "{{m}} 已替换");
    }
    // 每条都带 rd + elem 两个谓词叶子
    for r in &rs {
        let p = super::pred::parse(r.when.as_ref().unwrap()).unwrap();
        assert_eq!(
            super::pred::leaf_count(&p),
            2,
            "base when 与行 eq 合并为 and"
        );
    }
}

#[test]
fn vary_requires_equal_length_lists() {
    let doc = lowering_doc(
        "[[lowering]]\nop = \"Copy\"\nvary = { elem = [1, 2], m = [\"mov\"] }\ninsts = [\"{m} {out}, {0}\"]",
    );
    let msg = match parse_and_validate(&doc).unwrap_err() {
        V12Error::Parse { msg, .. } => msg,
        other => panic!("expected Parse error, got {other:?}"),
    };
    assert!(msg.contains("等长"), "msg: {msg}");
}

#[test]
fn vary_predicate_attr_needs_integer() {
    let doc = lowering_doc(
        "[[lowering]]\nop = \"Copy\"\nvary = { elem = [\"f32\"] }\ninsts = [\"mov {out}, {0}\"]",
    );
    let msg = match parse_and_validate(&doc).unwrap_err() {
        V12Error::Parse { msg, .. } => msg,
        other => panic!("expected Parse error, got {other:?}"),
    };
    assert!(msg.contains("必须是整数"), "msg: {msg}");
}

#[test]
fn in_predicate_parses_and_evals() {
    use super::pred::{Pred, eval, parse as pparse};
    let p = pparse(&toml::from_str(r#"in = ["elem", [1, 3]]"#).unwrap()).unwrap();
    assert_eq!(p, Pred::In("elem".into(), vec![1, 3]));
    let ctx = |a: &str| match a {
        "elem" => Some(3),
        _ => None,
    };
    assert!(eval(&p, &ctx));
    let ctx2 = |a: &str| match a {
        "elem" => Some(2),
        _ => None,
    };
    assert!(!eval(&p, &ctx2));
    // 空集合恒假 → 拒绝
    assert!(pparse(&toml::from_str(r#"in = ["elem", []]"#).unwrap()).is_err());
}

#[test]
fn lowering_order_is_specificity_then_priority() {
    // 声明序：先兜底、后具体。裁决序必须把具体的排前面——作者不再需要记住
    // "兜底必须写最后"，写反了也不会静默改变分派。
    let doc = lowering_doc(
        "[[lowering]]\nop = \"Copy\"\ninsts = [\"MOV {out}, {0}\"]\n\
         [[lowering]]\nop = \"Copy\"\nwhen = { eq = [\"rd\", 32] }\ninsts = [\"MOV {out}, {1}\"]",
    );
    let m = parse_and_validate(&doc).expect("声明序颠倒不再是错误");
    let by_op = m.lowering_by_op();
    let (_, rules) = by_op
        .iter()
        .find(|(op, _)| *op == "Copy")
        .expect("Copy 组存在");
    assert_eq!(
        rules[0].insts,
        vec!["MOV {out}, {1}"],
        "1 叶子的具体规则排在 0 叶子的兜底之前"
    );
    assert!(rules[1].when.is_none(), "兜底垫底");
}

#[test]
fn priority_overrides_specificity() {
    // 更宽的规则（1 叶子）用 priority 压过更具体的（2 叶子）——Vextract lane0 形态
    let doc = lowering_doc(
        "[[lowering]]\nop = \"Copy\"\npriority = 1\nwhen = { eq = [\"rd\", 32] }\ninsts = [\"MOV {out}, {0}\"]\n\
         [[lowering]]\nop = \"Copy\"\nwhen = { and = [{ eq = [\"rd\", 32] }, { eq = [\"elem\", 1] }] }\ninsts = [\"MOV {out}, {1}\"]",
    );
    // prio 1 的规则覆盖了 prio 0 那条的全部取值域 → 后者是死规则
    let msg = match parse_and_validate(&doc).unwrap_err() {
        V12Error::Validation { msg, .. } => msg,
        other => panic!("expected Validation error, got {other:?}"),
    };
    assert!(msg.contains("死规则"), "msg: {msg}");
    // 反过来（不加 priority）则合法：具体的自动排前
    let ok = lowering_doc(
        "[[lowering]]\nop = \"Copy\"\nwhen = { eq = [\"rd\", 32] }\ninsts = [\"MOV {out}, {0}\"]\n\
         [[lowering]]\nop = \"Copy\"\nwhen = { and = [{ eq = [\"rd\", 32] }, { eq = [\"elem\", 1] }] }\ninsts = [\"MOV {out}, {1}\"]",
    );
    parse_and_validate(&ok).expect("特异性自动裁决：2 叶子排前，1 叶子不再吃掉它");
}

#[test]
fn disjoint_rules_are_not_dead() {
    let doc = lowering_doc(
        "[[lowering]]\nop = \"Copy\"\nwhen = { eq = [\"rd\", 32] }\ninsts = [\"MOV {out}, {0}\"]\n\
         [[lowering]]\nop = \"Copy\"\nwhen = { eq = [\"rd\", 64] }\ninsts = [\"MOV {out}, {1}\"]",
    );
    parse_and_validate(&doc).expect("互斥谓词不构成死规则");
}

#[test]
fn or_not_predicates_skip_dead_check() {
    // 含 or/not → RuleDomain::Opaque，放弃判定（保守，不误报）
    let doc = lowering_doc(
        "[[lowering]]\nop = \"Copy\"\nwhen = { or = [{ eq = [\"rd\", 32] }, { eq = [\"rd\", 64] }] }\ninsts = [\"MOV {out}, {0}\"]\n\
         [[lowering]]\nop = \"Copy\"\nwhen = { eq = [\"rd\", 32] }\ninsts = [\"MOV {out}, {1}\"]",
    );
    parse_and_validate(&doc).expect("Opaque 谓词不参与死规则判定");
}

// ─────────── S3：form 预设 ⊕ 指令级逐键覆盖 / opsize 位宽单位 ───────────

/// 指令可省略 `form`，把全部编码键写在自己身上（组合不再需要命名）。
#[test]
fn instruction_without_form_carries_all_enc_keys() {
    let doc = r#"
[meta]
name = "x"
variable_length = true
max_inst_len = 15
[reg.gpr8]
count = 16
[[operand_slots]]
name = "g"
kind = "reg"
class = "gpr8"
roles = ["in", "out"]
[[instructions]]
name = "MOVQ_REV"
modrm = { reg = "src", rm = "dst" }
opsize = 64
escape = [0x0F]
prefix = "field"
opcode = 0x7E
fields = { prefix = 0x66, w = 1 }
ops = ["dst:g:out", "src:g"]
asm = "movq {dst}, {src}"
"#;
    let m = parse_and_validate(doc).expect("无 form 的指令必须合法");
    let inst = &m.instructions[0];
    assert!(inst.form.is_none(), "没有 form 引用");
    assert!(inst.enc.modrm.is_some(), "modrm 映射已声明");
    assert_eq!(inst.enc.escape.as_deref(), Some(&[0x0Fu8][..]));
}

/// 指令级键逐个压过 form 预设；未覆盖的键继承预设。
#[test]
fn instruction_enc_keys_override_form_preset() {
    let doc = r#"
[meta]
name = "x"
variable_length = true
[reg.gpr8]
count = 16
[[operand_slots]]
name = "g"
kind = "reg"
class = "gpr8"
roles = ["in", "out"]
[[forms]]
name = "MRR"
modrm = { reg = "dst", rm = "src" }
opsize = "s0"
escape = [0x0F]
[[instructions]]
name = "I"
form = "MRR"
modrm = { reg = "src", rm = "dst" }
opcode = 1
ops = ["dst:g:out", "src:g"]
asm = "i {dst}, {src}"
"#;
    let m = parse_and_validate(doc).expect("valid");
    let preset = &m.forms[0].keys;
    let enc = m.instructions[0].enc.over(preset);
    assert_eq!(
        enc.modrm.as_ref().and_then(|m| m.rm.as_deref()),
        Some("dst"),
        "指令覆盖生效"
    );
    assert_eq!(
        enc.escape.as_deref(),
        Some(&[0x0Fu8][..]),
        "未覆盖的键继承预设"
    );
    assert_eq!(enc.opsize, Some(crate::v12::model::Opsize::Slot(0)));
}

/// opsize 固定宽度写**位宽整数**；旧的 "rN"（字节）写法给出明确迁移提示。
#[test]
fn opsize_fixed_width_is_bits() {
    use crate::v12::model::Opsize;
    let bits: Opsize = toml::from_str::<toml::Value>("v = 64").unwrap()["v"]
        .clone()
        .try_into()
        .expect("裸整数 = 位宽");
    assert_eq!(bits, Opsize::Reg(8), "64 位 = 8 字节（内部单位）");
    // 序列化回位宽整数（往返一致）
    assert_eq!(
        toml::Value::try_from(Opsize::Reg(8)).unwrap().as_integer(),
        Some(64)
    );
    // 非 8 倍数拒绝
    let bad: Result<Opsize, _> = toml::from_str::<toml::Value>("v = 12").unwrap()["v"]
        .clone()
        .try_into();
    assert!(bad.is_err(), "12 位不是 8 的倍数");
    // 旧字节写法带迁移提示
    let old: Result<Opsize, _> = toml::Value::String("r8".into()).try_into();
    let msg = format!("{}", old.unwrap_err());
    assert!(msg.contains("opsize = 64"), "需给出位宽写法提示: {msg}");
}

// ─────────── S3c：命名操作数（ops 声明 + asm 引用） ───────────

/// 最小命名形态：ops 声明序 = 编码序；asm 只引用名字；opsize 用名字。
#[test]
fn named_ops_declare_and_reference() {
    let doc = r#"
[meta]
name = "x"
variable_length = true
[reg.gpr8]
count = 16
[[operand_slots]]
name = "gx"
kind = "reg"
class = "gpr8"
roles = ["in", "out", "inout"]
[[instructions]]
name = "ADD_RM_R"
modrm = { reg = "src", rm = "dst" }
opsize = "dst"
opcode = 0x01
ops = ["src:gx", "dst:gx:inout"]
asm = "add {dst}, {src}"
"#;
    let m = parse_and_validate(doc).expect("命名形态必须合法");
    let inst = &m.instructions[0];
    assert_eq!(
        inst.ops.as_deref(),
        Some(&["src:gx".to_string(), "dst:gx:inout".to_string()][..])
    );
    // opsize 保留名字形态到模型层，collect_inst_infos 才解析成位置
    assert_eq!(
        inst.enc.opsize,
        Some(crate::v12::model::Opsize::Named("dst".into()))
    );
    // 打印序与编码序无关：asm 先打 dst（= 编码序 1）
    assert_eq!(inst.asm.as_str(), "add {dst}, {src}");
}

#[test]
fn named_ops_reject_duplicate_name() {
    let doc = ops_doc(
        r#"ops = ["a:gx", "a:gx"]
asm = "i {a}, {a}""#,
    );
    let msg = validation_msg(&doc);
    assert!(msg.contains("重复"), "msg: {msg}");
}

#[test]
fn named_ops_reject_bad_role() {
    let doc = ops_doc(
        r#"ops = ["a:gx:sideways"]
asm = "i {a}""#,
    );
    let msg = validation_msg(&doc);
    assert!(msg.contains("sideways"), "msg: {msg}");
}

#[test]
fn named_ops_reject_double_reference() {
    let doc = ops_doc(
        r#"ops = ["a:gx", "b:gx"]
asm = "i {a}, {a}""#,
    );
    let msg = validation_msg(&doc);
    assert!(msg.contains("引用 2 次"), "msg: {msg}");
}

#[test]
fn opsize_named_must_reference_declared_op() {
    let doc = ops_doc(
        r#"opsize = "nope"
ops = ["a:gx"]
asm = "i {a}""#,
    );
    // opsize 名字解析在 codegen（collect_inst_infos）——校验期先过，生成期报错
    let m = parse_and_validate(&doc).expect("模型层合法");
    let err = super::codegen::generate(&m).unwrap_err();
    assert!(err.contains("'nope'"), "err: {err}");
}

fn ops_doc(body: &str) -> String {
    format!(
        r#"
[meta]
name = "x"
variable_length = true
[reg.gpr8]
count = 16
[[operand_slots]]
name = "gx"
kind = "reg"
class = "gpr8"
roles = ["in", "out", "inout"]
[[instructions]]
name = "I"
modrm = {{ reg = "a", rm = "a" }}
opcode = 1
{body}
"#
    )
}

fn validation_msg(doc: &str) -> String {
    match parse_and_validate(doc).unwrap_err() {
        V12Error::Validation { msg, .. } => msg,
        other => panic!("expected Validation error, got {other:?}"),
    }
}

// ─────────── S3d：modrm 显式映射（取代六个魔法串） ───────────

/// `{ reg = "名", rm = "名" }`：哪个操作数进哪个字段直接写出来。
/// v14 的 `"rr"` 是位置隐含——同一个串在 ADD_RM_R 里 reg=源、在 MOV_R_RM 里
/// reg=目的，读者必须回查生成器才知道。
#[test]
fn modrm_map_declares_field_sources() {
    let m = parse_and_validate(&modrm_doc(r#"modrm = { reg = "src", rm = "dst" }"#))
        .expect("映射形态必须合法");
    let mm = m.instructions[0].enc.modrm.as_ref().unwrap();
    assert_eq!(mm.reg, Some(crate::v12::model::ModrmReg::Op("src".into())));
    assert_eq!(mm.rm_operand(), (false, Some("dst")));
}

/// `reg = <整数>` = 固定扩展码（取代 `"ext"` + `fields.ext` 两处声明）。
#[test]
fn modrm_map_reg_integer_is_ext_code() {
    let m = parse_and_validate(&modrm_doc(r#"modrm = { reg = 3, rm = "dst" }"#)).expect("valid");
    let mm = m.instructions[0].enc.modrm.as_ref().unwrap();
    assert_eq!(mm.reg, Some(crate::v12::model::ModrmReg::Ext(3)));
}

/// `rm = "[名]"` = 内存形式（与 asm 里的 `[{base}]` 同形）。
#[test]
fn modrm_map_bracket_means_memory() {
    let m =
        parse_and_validate(&modrm_doc(r#"modrm = { reg = "src", rm = "[dst]" }"#)).expect("valid");
    let mm = m.instructions[0].enc.modrm.as_ref().unwrap();
    assert_eq!(mm.rm_operand(), (true, Some("dst")));
}

/// 引用了没声明的操作数名 → 生成期报错（带 ops 清单）。
#[test]
fn modrm_map_rejects_unknown_operand_name() {
    let m = parse_and_validate(&modrm_doc(r#"modrm = { reg = "nope", rm = "dst" }"#))
        .expect("模型层合法");
    let err = super::codegen::generate(&m).unwrap_err();
    assert!(err.contains("'nope'"), "err: {err}");
    assert!(err.contains("dst, src"), "需列出已声明的名字: {err}");
}

fn modrm_doc(modrm: &str) -> String {
    format!(
        r#"
[meta]
name = "x"
variable_length = true
[reg.gpr8]
count = 16
[[operand_slots]]
name = "gx"
kind = "reg"
class = "gpr8"
roles = ["in", "out", "inout"]
[[instructions]]
name = "I"
{modrm}
opsize = "dst"
opcode = 1
ops = ["dst:gx:inout", "src:gx"]
asm = "i {{dst}}, {{src}}"
"#
    )
}

/// 含一条助记符为 `i` 的指令（`asm = "i {dst}, {src}"`）的最小合法模型——
/// `[[pattern]].insts` 用 `i …` 引用它（助记符已声明），叶变量 `{a}`/`{b}` 走
/// extra_known 通道。
const PATTERN_BASE: &str = r#"
[meta]
name = "x"
variable_length = true
[reg.gpr8]
count = 16
[[operand_slots]]
name = "gx"
kind = "reg"
class = "gpr8"
roles = ["in", "out", "inout"]
[[instructions]]
name = "I"
modrm = { reg = "dst", rm = "src" }
opsize = "dst"
opcode = 1
ops = ["dst:gx:inout", "src:gx"]
asm = "i {dst}, {src}"
"#;

#[test]
fn pattern_parses_and_validates() {
    let doc = PATTERN_BASE.to_string()
        + r#"
[[pattern]]
match = "Iadd(Imul(a, b), c)"
insts = ["I {out}, {a}, {b}"]
"#;
    let m = parse_and_validate(&doc).expect("valid pattern");
    assert_eq!(m.pattern.len(), 1);
    assert_eq!(m.pattern[0].r#match, "Iadd(Imul(a, b), c)");
    assert_eq!(m.pattern[0].insts, vec!["I {out}, {a}, {b}"]);
}

#[test]
fn pattern_rejects_payload_op() {
    let doc = PATTERN_BASE.to_string()
        + r#"
[[pattern]]
match = "Fcmp(a, b)"
insts = ["i {out}, {a}, {b}"]
"#;
    let err = parse_and_validate(&doc).unwrap_err().to_string();
    assert!(err.contains("Fcmp"), "err: {err}");
}

#[test]
fn pattern_rejects_duplicate_var() {
    let doc = PATTERN_BASE.to_string()
        + r#"
[[pattern]]
match = "Iadd(a, a)"
insts = ["i {out}, {a}"]
"#;
    let err = parse_and_validate(&doc).unwrap_err().to_string();
    assert!(err.contains("重复"), "err: {err}");
}

#[test]
fn pattern_rejects_unknown_placeholder() {
    let doc = PATTERN_BASE.to_string()
        + r#"
[[pattern]]
match = "Iadd(a, b)"
insts = ["I {out}, {nope}"]
"#;
    let err = parse_and_validate(&doc).unwrap_err().to_string();
    assert!(err.contains("未知占位符"), "err: {err}");
}

#[test]
fn codegen_emits_pattern_statics_and_lower_pattern() {
    let doc = PATTERN_BASE.to_string()
        + r#"
[[pattern]]
match = "Iadd(Imul(a, b), c)"
insts = ["I {out}, {a}", "I {out}, {b}", "I {out}, {c}"]
"#;
    let model = parse_and_validate(&doc).expect("valid pattern model");
    let ts = super::codegen::generate(&model).expect("codegen must succeed");
    let s = ts.to_string();
    for needle in [
        "__PATTERNS",
        "PatternSpec",
        "fn patterns",
        "fn lower_pattern",
        "pattern_0",
        "Opcode :: Iadd",
        "Opcode :: Imul",
    ] {
        assert!(s.contains(needle), "generated code missing '{needle}'");
    }
}

// ─────────────────── 通用内存模板（S9） ───────────────────

#[test]
fn mem_template_parse_mips() {
    use super::codegen::mem::{Comp, Item, parse_mem_template};
    let items = parse_mem_template("{disp}({base})").unwrap();
    assert_eq!(items.len(), 4);
    assert!(matches!(&items[0], Item::Comp(Comp::Disp)));
    assert!(matches!(&items[1], Item::Lit { text, .. } if text == "("));
    assert!(matches!(&items[2], Item::Comp(Comp::Base)));
    assert!(matches!(&items[3], Item::Lit { text, .. } if text == ")"));
}

#[test]
fn mem_template_parse_default() {
    use super::codegen::mem::{Comp, Item, parse_mem_template};
    let items = parse_mem_template("[{base}+{index}*{scale}+{disp}]").unwrap();
    assert_eq!(items.len(), 9);
    assert!(matches!(&items[0], Item::Lit { text, .. } if text == "["));
    assert!(matches!(&items[1], Item::Comp(Comp::Base)));
    assert!(matches!(&items[3], Item::Comp(Comp::Index)));
    assert!(matches!(&items[5], Item::Comp(Comp::Scale)));
    assert!(matches!(&items[7], Item::Comp(Comp::Disp)));
    assert!(matches!(&items[8], Item::Lit { text, .. } if text == "]"));
}

#[test]
fn mem_template_render_mips_style() {
    use super::codegen::mem::{gen_render_mem, parse_mem_template};
    let items = parse_mem_template("{disp}({base})").unwrap();
    let s = gen_render_mem(&items).to_string();
    let c = s.replace(' ', "");
    assert!(!c.contains("\"[\""), "MIPS 不应有方括号：{s}");
    assert!(c.contains("\"(\""), "应有 '('：{s}");
    assert!(c.contains("\")\""), "应有 ')'：{s}");
    assert!(c.contains("m.disp"), "disp 裸有符号输出：{s}");
}

#[test]
fn mem_template_parser_mips_style() {
    use super::codegen::mem::{gen_mem_parser, parse_mem_template};
    let items = parse_mem_template("{disp}({base})").unwrap();
    let s = gen_mem_parser(&items).to_string();
    let c = s.replace(' ', "");
    assert!(
        c.contains("__raw_signed_int"),
        "MIPS disp 需有符号立即数：{s}"
    );
    assert!(c.contains("__Tok::LParen"), "应有 '('：{s}");
    assert!(!c.contains("__Tok::LBracket"), "不应有 '['：{s}");
}

#[test]
fn mem_template_default_render_matches_x86() {
    use super::codegen::mem::{gen_render_mem, parse_mem_template};
    let items = parse_mem_template("[{base}+{index}*{scale}+{disp}]").unwrap();
    let s = gen_render_mem(&items).to_string();
    let c = s.replace(' ', "");
    assert!(c.contains("\"[\""), "x86 应有 '['：{s}");
    assert!(c.contains("\"+\""), "x86 应有 '+'：{s}");
    assert!(c.contains("\"*\""), "x86 应有 '*'：{s}");
    assert!(c.contains("\"]\""), "x86 应有 ']'：{s}");
}

#[test]
fn conventions_mem_validation() {
    let base = r#"
[meta]
name = "mips"
default_inst_width = 32
[reg.gpr4]
count = 8
[conventions.bitfields]
opcode = { offset = 0, width = 7 }
rd = { offset = 7, width = 3 }
[conventions.mem]
template = "{disp}({base})"
[[operand_slots]]
name = "g"
kind = "reg"
class = "gpr"
[[forms]]
name = "R"
opcode_field = "opcode"
operand_fields = ["rd"]
[[instructions]]
name = "ADD"
form = "R"
opcode = 0x13
ops = ["dst:g:out"]
asm = "add {dst}"
"#;
    parse_and_validate(base).expect("MIPS 模板合法");
    // 缺 base → 报错
    let bad = base.replace(r#"template = "{disp}({base})""#, r#"template = "{disp}""#);
    let err = parse_and_validate(&bad).unwrap_err().to_string();
    assert!(err.contains("base"), "err: {err}");
    // scale 未紧随 index → 报错
    let bad = base.replace(
        r#"template = "{disp}({base})""#,
        r#"template = "[{base}*{scale}]""#,
    );
    let err = parse_and_validate(&bad).unwrap_err().to_string();
    assert!(err.contains("index"), "err: {err}");
    // 未知占位符 → 报错
    let bad = base.replace(
        r#"template = "{disp}({base})""#,
        r#"template = "[{base}+{foo}]""#,
    );
    let err = parse_and_validate(&bad).unwrap_err().to_string();
    assert!(err.contains("foo"), "err: {err}");
}
