//! ISA-DSL 迭代 1 验证：解析器单测（合法/非法 TOML 诊断）+ 语义校验 + 序列化往返
//! + v11 文件拒绝证明（不兼容的直接体现）。

use crate::dsl::model::RegClass;

use super::model::{Endian, OperandKind, OperandRole};
use super::{DslError, parse, parse_and_validate};

// ─────────────────────── 合法解析 ───────────────────────

const RISCV_DOC: &str = r#"
[meta]
name = "riscv64"
version = "12.0"
endian = "little"
mode = 64
[encoding]
kind = "fixed"
bits = 32

[reg.gpr64]
names = ["X0", "X1", "X2", "X3", "X4", "X5", "X6", "X7", "X8", "X9", "X10", "X11",
         "X12", "X13", "X14", "X15", "X16", "X17", "X18", "X19", "X20", "X21",
         "X22", "X23", "X24", "X25", "X26", "X27", "X28", "X29", "X30", "X31"]

[operand.gpr]
kind = "reg"
class = "gpr64"
roles = "inout"

[operand.imm12]
kind = "imm"
signed = true
bits = 12

[[forms]]
name = "R"
fields = [
  "u7[6:0]:opcode",
  "u5[11:7]:rd",
  "u5[19:15]:rs1",
  "u5[24:20]:rs2",
  "u3[14:12]:funct3",
  "u7[31:25]:funct7",
]

[[instructions]]
name = "ADD"
form = "R"
opcode = 0x33
match = { funct3 = 0, funct7 = 0 }
ops = ["dst:gpr:out", "src:gpr", "src2:gpr"]
asm = "add {dst}, {src}, {src2}"
"#;

#[test]
fn parse_minimal_riscv_style() {
    let m = parse_and_validate(RISCV_DOC).expect("valid riscv-style doc must parse");
    assert_eq!(m.meta.name, "riscv64");
    assert_eq!(m.encoding.bits, Some(32));
    assert!(!m.is_variable_length());
    assert_eq!(m.reg[&RegClass::GPR(64)].names.as_ref().unwrap().len(), 32);
    assert_eq!(m.conventions.bitfields.len(), 6);
    assert_eq!(m.conventions.bitfields["rd"].offset, Some(7));
    assert_eq!(m.conventions.bitfields["rd"].width, Some(5));
    assert_eq!(m.operand_slots.len(), 2);
    assert_eq!(m.operand_slots[0].kind, OperandKind::Reg);
    assert_eq!(m.operand_slots[0].class.as_ref(), Some(&RegClass::GPR(64)));
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
[encoding]
kind = "prefix_scan"
max_len = 15

[reg.gpr64]
names = ["RAX", "RCX", "RDX", "RBX", "RSP", "RBP", "RSI", "RDI",
         "R8", "R9", "R10", "R11", "R12", "R13", "R14", "R15"]

[reg.fpr128]
count = 16
prefix = "XMM"

[conventions.modrm]
reg_field = "modrm_reg"
rm_field = "modrm_rm"
force_disp_base = [5, 13]

[operand.gpr]
kind = "reg"
class = "gpr64"

[operand.fpr]
kind = "reg"
class = "fpr128"

[operand.imm32]
kind = "imm"
signed = true
bits = 32

[[forms]]
name = "RR"
rex = "auto"
"#;

#[test]
fn parse_x86_conventions() {
    let m = parse_and_validate(X86_DOC).expect("valid x86-style doc must parse");
    assert!(m.is_variable_length());
    assert_eq!(m.encoding.max_len, Some(15));
    // 生成式寄存器组：count + prefix
    assert_eq!(m.reg[&RegClass::FPR(128)].count, Some(16));
    assert_eq!(m.reg[&RegClass::FPR(128)].prefix.as_deref(), Some("XMM"));
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
[reg.gpr32]
count = 8
[operand.g]
kind = "reg"
class = "gpr"
"#;
    let m = parse_and_validate(doc).expect("valid");
    assert_eq!(m.meta.endian, Endian::Little);
    assert_eq!(m.meta.mode, 64);
    assert_eq!(m.encoding.bits, None);
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
    // 操作数槽是**派生**字段（v21 W3）：往返要经解析+降级，故用 parse_and_validate。
    let m2 = parse_and_validate(&text).expect("reparse");
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
[reg.gpr32]
count = 8
[operand.rm]
kind = "reg"
class = "gpr"
roles = "inout"
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
    // 槽能力声明：inout = 读改写（一个 `OperandRole` 就够，不是集合）
    assert_eq!(m.operand_slots[0].roles, Some(OperandRole::InOut));
    // 序列化往返保留 inout
    let text = toml::to_string(&m).expect("serialize");
    assert!(text.contains("inout"), "got: {text}");
    // 操作数槽是**派生**字段（v21 W3）：往返要经解析+降级，故用 parse_and_validate。
    let m2 = parse_and_validate(&text).expect("reparse");
    assert_eq!(m, m2);
}

// ─────────────────────── 非法 TOML 诊断 ───────────────────────

#[test]
fn rejects_unknown_top_level_key() {
    let doc = r#"
[meta]
name = "x"
bogus = 1
[reg.gpr32]
count = 8
"#;
    let err = parse(doc).unwrap_err();
    match err {
        DslError::Parse { msg, .. } => {
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
[reg.gpr32]
count = 8
"#;
    let err = parse(doc).unwrap_err();
    match err {
        DslError::Parse { msg, .. } => assert!(msg.contains("no_default_lowering"), "msg: {msg}"),
        other => panic!("expected Parse error, got {other:?}"),
    }
}

#[test]
fn rejects_unknown_operand_slot_key() {
    let doc = r#"
[meta]
name = "x"
[reg.gpr32]
count = 8
[operand.g]
kind = "reg"
class = "gpr"
garbage = 1
"#;
    let err = parse(doc).unwrap_err();
    match err {
        DslError::Parse { msg, .. } => {
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
[reg.gpr32]
count = 8
[operand.x]
kind = "float"
"#;
    let err = parse(doc).unwrap_err();
    match err {
        DslError::Parse { msg, .. } => assert!(msg.contains("float"), "msg: {msg}"),
        other => panic!("expected Parse error, got {other:?}"),
    }
}

/// 不兼容证明：v11 主 ISA 文件必须被 ISA-DSL 解析器拒绝。
///
/// 注：toml crate 按字母序迭代表（BTreeMap），`[abi]` 排在 `[meta]` 之前，
/// 所以首个报错是 v11 的 `[abi.arg_regs]` 而非 meta 的 `no_default_lowering`。
#[test]
fn rejects_v11_isa_file() {
    let v11 = r#"[meta]
version = "11.0"
[reg.gpr64]
count = 16
[abi]
arg_regs = ["RCX", "RDX"]
[inst.MOV]
encoding = "@modrm 0x01 /r"
"#;
    let err = parse(v11).expect_err("v11 file must NOT parse under the ISA-DSL parser");
    match err {
        DslError::Parse { msg, .. } => {
            assert!(msg.contains("unknown field"), "msg: {msg}");
        }
        other => panic!("expected Parse error, got {other:?}"),
    }
}

// ─────────────────────── 语义校验 ───────────────────────

/// v21 W2：字段**就地声明在 form 里**——需要"字段声明"的夹具用这个（128 位字，容得下
/// 超出 64 位的字侧偏移）。
fn field_doc(decls: &str) -> String {
    format!(
        r#"
[meta]
name = "x"
[encoding]
kind = "fixed"
bits = 128
[reg.gpr32]
count = 8
[operand.g]
kind = "reg"
class = "gpr32"
[[forms]]
name = "F"
fields = [
{decls}
]
[[instructions]]
name = "NOP"
form = "F"
opcode = 0
ops = ["dst:g:out"]
asm = "nop {{dst}}"
"#
    )
}

fn slot_doc(extra: &str) -> String {
    format!(
        r#"
[meta]
name = "x"
[reg.gpr32]
count = 8
[operand.g]
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
[operand.g]
kind = "reg"
class = "gpr"
"#;
    match parse(no_reg).unwrap_err() {
        DslError::Parse { msg, .. } => assert!(msg.contains("missing field `reg`"), "msg: {msg}"),
        other => panic!("expected Parse error, got {other:?}"),
    }
    let no_slots = r#"
[meta]
name = "x"
[reg.gpr32]
count = 8
"#;
    // v21 W3：`[operand.<名字>]` 是可选表 ⇒ 缺声明在**校验期**报（不是解析期必填字段）。
    match parse_and_validate(no_slots).unwrap_err() {
        DslError::Validation { msg, .. } => {
            assert!(msg.contains("[operand."), "msg: {msg}");
        }
        other => panic!("expected Validation error, got {other:?}"),
    }
}

/// 空 `[reg]` 表（存在但无组）→ 语义校验报错。
#[test]
fn validation_empty_reg() {
    let doc = r#"
[meta]
name = "x"
[reg]
[operand.g]
kind = "reg"
class = "gpr"
"#;
    let err = parse_and_validate(doc).unwrap_err();
    match err {
        DslError::Validation { msg, .. } => assert!(msg.contains("missing [reg.*]"), "msg: {msg}"),
        other => panic!("expected Validation error, got {other:?}"),
    }
}

/// 空 `operand_slots = []`（存在但为空）→ 语义校验报错。
#[test]
fn validation_empty_operand_slots() {
    let doc = r#"

[meta]
name = "x"
[reg.gpr32]
count = 8
"#;
    let err = parse_and_validate(doc).unwrap_err();
    match err {
        DslError::Validation { msg, .. } => {
            assert!(msg.contains("[operand."), "msg: {msg}");
        }
        other => panic!("expected Validation error, got {other:?}"),
    }
}

#[test]
fn validation_bitfield_overflow() {
    // 值表示上限：单个位域 > 64 位（位域值承载在 u64/i64 上）→ 报错
    let doc = field_doc("\"u65[64:0]:big\",");
    let err = parse_and_validate(&doc).unwrap_err();
    match err {
        DslError::Validation { msg, .. } => {
            assert!(msg.contains("超过值表示上限 64 位"), "msg: {msg}");
            assert!(msg.contains("big"), "msg: {msg}");
        }
        other => panic!("expected Validation error, got {other:?}"),
    }
    // 字侧偏移**无上限**（字长是 ISA 数据，字由字节数组承载）：offset 63 合法
    parse_and_validate(&field_doc("\"u2[64:63]:hi\","))
    .expect("字侧偏移不受 64 位限制（字长任意）");
}

#[test]
fn validation_modrm_ref() {
    let doc = slot_doc(
        r#"
[conventions.modrm]
reg_field = "nope"
rm_field = "foo"
"#,
    );
    let err = parse_and_validate(&doc).unwrap_err();
    match err {
        DslError::Validation { msg, .. } => {
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
[reg.gpr32]
count = 8
[operand.g]
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
        DslError::Validation { msg, .. } => {
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
[reg.gpr32]
count = 8
[operand.g]
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
        DslError::Validation { msg, .. } => {
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
[reg.gpr32]
count = 8
[operand.g]
kind = "reg"
class = "gpr"
[[forms]]
name = "R"
fields = [
  "u7[6:0]:opcode",
  "u3[9:7]:rd",
]
[[instructions]]
name = "NOP"
form = "R"
opcode = 0x13
ops = ["dst:g:out", "src:g"]
asm = "foo {dst}, {src}"
"#;
    let err = parse_and_validate(doc).unwrap_err();
    match err {
        DslError::Validation { msg, .. } => {
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
[reg.gpr32]
count = 8
[operand.g]
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
        DslError::Validation { msg, .. } => {
            assert!(
                msg.contains("duplicate instruction name 'NOP'"),
                "msg: {msg}"
            );
        }
        other => panic!("expected Validation error, got {other:?}"),
    }
}

/// 模板行必须给出编码信息（`opcode` 或 `fields`），否则校验失败。
#[test]
fn validation_template_row_needs_encoding() {
    let doc = r#"
[meta]
name = "x"
[reg.gpr32]
count = 8
[operand.g]
kind = "reg"
class = "gpr"
[[forms]]
name = "R"
[[templates]]
name = "F"
body = { form = "R", ops = ["dst:g:out"], asm = "f {dst}" }
rows = [ { inst = "V1" } ]
"#;
    let err = parse_and_validate(doc).unwrap_err();
    match err {
        DslError::Validation { msg, .. } => {
            assert!(msg.contains("缺少编码信息"), "msg: {msg}");
        }
        other => panic!("expected Validation error, got {other:?}"),
    }
}

#[test]
fn validation_duplicate_reg_names() {
    let doc = r#"
[meta]
name = "x"
[reg.gpr32]
names = ["R0", "R0"]
[operand.g]
kind = "reg"
class = "gpr"
"#;
    let err = parse_and_validate(doc).unwrap_err();
    match err {
        DslError::Validation { msg, .. } => {
            assert!(msg.contains("duplicate 'R0'"), "msg: {msg}");
        }
        other => panic!("expected Validation error, got {other:?}"),
    }
}

/// `[encoding]` 三态的结构化互斥（v18 S4）：三态的键不能互相串用，缺必填即报。
#[test]
fn validation_encoding_kinds_are_structurally_exclusive() {
    // fixed + widths = 写错了（widths 只属于 mixed）
    let doc = r#"
[meta]
name = "x"
[encoding]
kind = "fixed"
bits = 32
widths = [16, 32]
[reg.gpr32]
count = 8
[operand.g]
kind = "reg"
class = "gpr"
"#;
    let msg = validation_msg(doc);
    assert!(msg.contains("widths 只适用于"), "msg: {msg}");

    // prefix_scan + bits = 写错了（变长 ISA 没有单一字长）
    let doc = r#"
[meta]
name = "x"
[encoding]
kind = "prefix_scan"
bits = 32
[reg.gpr32]
count = 8
[operand.g]
kind = "reg"
class = "gpr"
"#;
    let msg = validation_msg(doc);
    assert!(msg.contains("bits 只适用于"), "msg: {msg}");

    // fixed 缺 bits 且指令写 width：逐指令 width **不能替代** bits
    // （全 ISA 只有一个字长，声明一次即可）。
    let doc = r#"
[meta]
name = "x"
[encoding]
kind = "fixed"
[reg.gpr32]
count = 8
[operand.g]
kind = "reg"
class = "gpr"
[[instructions]]
name = "I"
width = 32
opcode = 1
asm = "i"
"#;
    let msg = validation_msg(doc);
    assert!(msg.contains("不能替代 [encoding].bits"), "msg: {msg}");

    // 省略整个 [encoding] = **合法骨架文档**（还没定字长），但生成期必须报
    // "bits 缺失"——省略整段不会被静默当成定宽 32。
    let doc = r#"
[meta]
name = "x"
[reg.gpr32]
count = 8
[operand.g]
kind = "reg"
class = "gpr"
"#;
    let m = parse_and_validate(doc).expect("无 [encoding] 的骨架文档解析/校验合法");
    let err = super::codegen::generate(&m).unwrap_err();
    assert!(err.contains("bits 缺失"), "err: {err}");
}

#[test]
fn validation_bad_isa_name() {
    let doc = r#"
[meta]
name = "123"
[reg.gpr32]
count = 8
[operand.g]
kind = "reg"
class = "gpr"
"#;
    let err = parse_and_validate(doc).unwrap_err();
    match err {
        DslError::Validation { msg, .. } => {
            assert!(msg.contains("not a valid ISA name"), "msg: {msg}");
        }
        other => panic!("expected Validation error, got {other:?}"),
    }
}

// ─────────────────────── 迭代 2：codegen ───────────────────────

#[test]
fn codegen_big_endian_decode_reads_be() {
    // big-endian 定宽：字是字节数组（LE 位序），内存序 = 反转字节
    // （decode 读入后 reverse、encode 输出前 reverse）——不再是 to_be_bytes。
    let doc = r#"
[meta]
name = "x"
endian = "big"
[encoding]
kind = "fixed"
bits = 32
[reg.gpr32]
count = 8
[operand.g]
kind = "reg"
class = "gpr"
[[forms]]
name = "R"
fields = [
  "u7[6:0]:opcode",
  "u3[9:7]:rd",
]
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
    // TokenStream::to_string 会在符号间插空格（`__out . reverse ()`）——按 token 名断言。
    assert!(
        s.contains("__be") && s.contains("reverse"),
        "big-endian decode 应把内存字节序转成 LE 位序（`__be.reverse()`）"
    );
    assert!(
        s.contains("__out") && s.contains("reverse"),
        "big-endian encode 应反转字节序输出（`__out.reverse()`）"
    );
}

#[test]
fn codegen_align_pad_emit_key() {
    // [emit].align_pad 进入生成的 parse_insts（.align 填充字节可配置）
    let doc = r#"
[meta]
name = "x"
[encoding]
kind = "fixed"
bits = 32
[reg.gpr32]
count = 8
[operand.g]
kind = "reg"
class = "gpr"
[[forms]]
name = "R"
fields = [
  "u7[6:0]:opcode",
  "u3[9:7]:rd",
]
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
[encoding]
kind = "fixed"
bits = 32
[reg.gpr32]
count = 8
[operand.g]
kind = "reg"
class = "gpr"
[[forms]]
name = "R"
fields = [
  "u7[6:0 -> 3:0,10:8]:opcode",
  "u3[13:11]:rd",
]
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
[encoding]
kind = "fixed"
bits = 32
[reg.gpr32]
count = 8
[operand.g]
kind = "reg"
class = "gpr"
[operand.i]
kind = "imm"
signed = true
bits = 12
[[forms]]
name = "I"
fields = [
  "u7[6:0]:opcode",
  "u3[9:7]:rd",
  "u3[17:15]:rs1",
  "u12[31:20]:imm12",
  "u3[14:12]:funct3",
]
[[instructions]]
name = "ADDI"
form = "I"
opcode = 0x13
match = { funct3 = 0 }
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

// ───────────────── S3a：三处"别家常量兜底"改 fail-closed ─────────────────

/// `[machine].spill_scratch` 缺失 + 声明了栈参数 ⇒ **生成期**报错（原先回退 x86 的 R10）。
///
/// 用真实 x86 谱删掉 scratch 声明来构造：栈参数收参真的需要这个临时寄存器，
/// 没有它就该报错，而不是引用一个别的 ISA 恰好有的寄存器名。
#[test]
fn codegen_requires_scratch_for_stack_args() {
    let src = include_str!("../../../../../isa/x86.toml");
    let doc = src.replace("spill_scratch = [\"R10\", \"R11\"]", "");
    assert_ne!(
        doc, src,
        "x86 谱里找不到 spill_scratch 声明行——本测试需更新"
    );
    let model = parse_and_validate(&doc).unwrap();
    let err = super::codegen::generate(&model).unwrap_err();
    assert!(
        err.contains("[machine].spill_scratch"),
        "错误消息应点名 [machine].spill_scratch：{err}"
    );
}

/// `[machine].link_reg` 缺失 + call 指令确有返回地址寄存器槽 ⇒ 生成期报错
///（原先回退 riscv 的 X1）。反向对照：x86 的 CALL 没有该槽，故无需声明（见
/// `codegen_generates_core_surface` 与三份发行谱的正常生成）。
#[test]
fn codegen_requires_call_ret_reg_when_call_has_ret_slot() {
    let src = include_str!("../../../../../isa/riscv64.toml");
    let doc = src.replace("link_reg = \"X1\"", "");
    assert_ne!(doc, src, "riscv 谱里找不到 link_reg 声明行——本测试需更新");
    let model = parse_and_validate(&doc).unwrap();
    let err = super::codegen::generate(&model).unwrap_err();
    assert!(
        err.contains("[machine].link_reg"),
        "错误消息应点名 [machine].link_reg：{err}"
    );
}

// ───────────────── S3b：条件码数据化（[conventions.cond] 单表三用） ─────────────────

/// 条件码最小模型：`[conventions.cond]` 由 `tbl` 给出，`extra` 追加载荷。
///
/// 表同时服务三处——汇编解析（cond 槽）、反汇编渲染（按码取字母序最小名）、
/// lowering 的 `{cc}`（按 `ir` 字段）。
fn gen_cond_doc(tbl: &str, extra: &str) -> String {
    format!(
        r#"
[meta]
name = "x"
[encoding]
kind = "fixed"
bits = 32
[reg.gpr32]
count = 8
[conventions.cond]
{tbl}
[operand.g]
kind = "reg"
class = "gpr"
[operand.cc]
kind = "cond"
[[forms]]
name = "C"
fields = [
  "u8[7:0]:opcode",
  "u3[10:8]:rd",
  "u4[14:11]:cc",
]
[[instructions]]
name = "SETC"
form = "C"
opcode = 0x90
ops = ["dst:g:out", "cond:cc"]
asm = "setc {{dst}}, {{cond}}"
{extra}
"#
    )
}

/// 10 个 IR 整数条件全部映射（全写形式：汇编名 + `ir`）。
const CC_ALL_FULL: &str = r#"
eq  = { code = 4,  ir = "eq" }
ne  = { code = 5,  ir = "ne" }
slt = { code = 12, ir = "slt" }
sle = { code = 14, ir = "sle" }
sgt = { code = 15, ir = "sgt" }
sge = { code = 13, ir = "sge" }
ult = { code = 2,  ir = "ult" }
ule = { code = 6,  ir = "ule" }
ugt = { code = 7,  ir = "ugt" }
uge = { code = 3,  ir = "uge" }
"#;

/// 同一个表的**简写**形式（键名恰是 IR 条件名 ⇒ `ir` 取键名）。
const CC_ALL_SHORT: &str = r#"
eq = 4
ne = 5
slt = 12
sle = 14
sgt = 15
sge = 13
ult = 2
ule = 6
ugt = 7
uge = 3
"#;

fn cc_rule() -> &'static str {
    "[[lowering]]\nop = \"Icmp\"\ninsts = [\"SETC {out}, {cc}\"]"
}

/// `{cc}` 的生成代码按 `ir` 字段查表得到本 ISA 编码，且**不出现 `IntCC`**。
#[test]
fn cond_cc_arms_come_from_the_table() {
    for tbl in [CC_ALL_FULL, CC_ALL_SHORT] {
        let doc = gen_cond_doc(tbl, cc_rule());
        let model = parse_and_validate(&doc).expect("合法条件码表");
        // 生成物是 token 串，空格由 `to_string` 决定 ⇒ 比较前去掉空白。
        let s: String = super::codegen::generate(&model)
            .unwrap()
            .to_string()
            .chars()
            .filter(|c| !c.is_whitespace())
            .collect();
        for (name, code) in [
            ("eq", 4),
            ("ne", 5),
            ("slt", 12),
            ("sle", 14),
            ("sgt", 15),
            ("sge", 13),
            ("ult", 2),
            ("ule", 6),
            ("ugt", 7),
            ("uge", 3),
        ] {
            let needle = format!("Some(\"{name}\")=>{code}u8");
            assert!(s.contains(&needle), "生成的 {{cc}} 表缺 `{needle}`：{s}");
        }
        assert!(!s.contains("IntCC"), "生成代码不得点名 IntCC：{s}");
        assert!(s.contains("intcc_name("), "应经宿主函数折成 IR 条件名：{s}");
    }
}

/// 用 `{cc}` 却漏映射 IR 条件 ⇒ 编译期报错并点名缺哪些（运行期会静默退化成 0）。
#[test]
fn cond_cc_requires_all_ir_conditions() {
    let doc = gen_cond_doc("eq = 4\nne = 5\n", cc_rule());
    let msg = validation_msg(&doc);
    assert!(msg.contains("没把所有 IR 整数条件映射全"), "msg: {msg}");
    assert!(msg.contains("slt"), "缺项清单应含 slt：{msg}");
    assert!(msg.contains("uge"), "缺项清单应含 uge：{msg}");
}

/// `cond` 槽没有条件码表 ⇒ 报错（v18 S3b 起不再回退 x86 的 16 项表）。
#[test]
fn cond_slot_without_table_is_rejected() {
    let doc = gen_cond_doc("", "").replace("[conventions.cond]\n", "");
    let msg = validation_msg(&doc);
    assert!(msg.contains("需要条件码表"), "msg: {msg}");
}

/// 空表 = 声明了却什么都没给 ⇒ 报错（区别于"整节不写"）。
#[test]
fn cond_empty_table_is_rejected() {
    let msg = validation_msg(&gen_cond_doc("", ""));
    assert!(msg.contains("不能为空"), "msg: {msg}");
}

/// `code` 必须落进 4 位条件字段。
#[test]
fn cond_code_beyond_four_bits_is_rejected() {
    let msg = validation_msg(&gen_cond_doc("eq = { code = 16, ir = \"eq\" }\n", ""));
    assert!(msg.contains("超出条件字段宽度"), "msg: {msg}");
}

/// `ir` 必须是 IR 整数条件名。
#[test]
fn cond_unknown_ir_condition_is_rejected() {
    let msg = validation_msg(&gen_cond_doc("e = { code = 4, ir = \"nope\" }\n", ""));
    assert!(msg.contains("不是 IR 整数条件名"), "msg: {msg}");
    assert!(msg.contains("eq"), "应列出可用条件名：{msg}");
}

/// 同一个 IR 条件不能被映射两次（否则 `{cc}` 该取哪个编码没有答案）。
#[test]
fn cond_duplicate_ir_mapping_is_rejected() {
    let msg = validation_msg(&gen_cond_doc(
        "e = { code = 4, ir = \"eq\" }\ne2 = { code = 4, ir = \"eq\" }\n",
        "",
    ));
    assert!(msg.contains("重复映射"), "msg: {msg}");
}

/// 纯汇编别名（不写 `ir`、键名也不是 IR 条件名）合法：`{cc}` 不要求它。
#[test]
fn cond_pure_asm_alias_has_no_ir_mapping() {
    let doc = gen_cond_doc("z = 4\n", "");
    let model = parse_and_validate(&doc).expect("纯别名合法");
    assert!(model.cond_ir_codes().is_empty(), "纯别名不该产生 IR 映射");
}

// ───────────────── S3f：[[derive]] 派生谓词属性 ─────────────────

/// 派生属性可用于 `when`：**生成期展开**成它自己的谓词表达式（求值走核心属性助手），
/// 不再经过运行时 `__attr(name)` 名字查找（v18 S8d）。
#[test]
fn derive_attr_is_usable_in_when() {
    let doc = lowering_doc(
        "[[derive]]\nname = \"is_64\"\nexpr = { eq = [\"rs1_width\", 64] }\n\
         [[lowering]]\nop = \"Copy\"\nwhen = { eq = [\"is_64\", 1] }\ninsts = [\"MOV {out}, {0}\"]",
    );
    let m = parse_and_validate(&doc).expect("派生属性 + when 引用必须通过");
    assert!(
        m.derived_preds.contains_key("is_64"),
        "解析期应展开派生谓词"
    );
    let s: String = super::codegen::generate(&m)
        .unwrap()
        .to_string()
        .chars()
        .filter(|c| !c.is_whitespace())
        .collect();
    // 派生名展开成 `Some(if <核心属性谓词> {1} else {0})`，核心属性经 `__ac_get` 按需取。
    assert!(s.contains("Some(if"), "派生属性应在使用点展开：{s}");
    assert!(
        s.contains("__ac.rs1_width(&*ctx)"),
        "派生表达式应引用核心属性助手（rs1_width → 槽 1）：{s}"
    );
    assert!(!s.contains("__attr"), "不应再有运行时名字分派：{s}");
}

/// 核心属性谓词直接展开成属性助手调用（无 `__attr(name)` 字符串分派，v18 S8d）。
#[test]
fn core_attr_guard_uses_helper_directly() {
    let doc = lowering_doc(
        "[[lowering]]\nop = \"Copy\"\nwhen = { eq = [\"rs1_width\", 64] }\ninsts = [\"MOV {out}, {0}\"]",
    );
    let m = parse_and_validate(&doc).expect("合法");
    let raw = super::codegen::generate(&m).unwrap().to_string();
    assert!(!raw.contains("__attr"), "不应再有 `__attr`：{raw}");
    assert!(
        !raw.contains("__attr_core"),
        "不应再有 `__attr_core`：{raw}"
    );
    let s: String = raw.chars().filter(|c| !c.is_whitespace()).collect();
    assert!(
        s.contains("__ac.rs1_width(&*ctx)"),
        "谓词应直接取属性方法：{s}"
    );
    // 9 个核心属性各有一个助手（按需调用；`__ac_get` 保证每次调用每属性最多算一次）。
    for a in crate::dsl::pred::PRED_ATTRS {
        assert!(
            s.contains(&format!("fn{a}(&mutself,ctx:")),
            "缺属性助手 `__a_{a}`：{s}"
        );
    }
    assert!(
        s.contains("struct__AC<'__x>{done:u16"),
        "缺少按需缓存表：{s}"
    );
}

/// 不用 `{cc}` 的规则不发射 `let __cc: u8 = 0;`（死代码，v18 S8d 修）。
#[test]
fn cc_binding_only_emitted_when_used() {
    let plain = lowering_doc("[[lowering]]\nop = \"Copy\"\ninsts = [\"MOV {out}, {0}\"]");
    let m = parse_and_validate(&plain).expect("合法");
    let s = super::codegen::generate(&m).unwrap().to_string();
    assert!(
        !s.contains("__cc"),
        "不用 `{{cc}}` 的降低规则不该出现 `__cc`（连 `let __cc: u8 = 0;` 也是死代码）：{s}"
    );

    let with_cc = gen_cond_doc(CC_ALL_SHORT, cc_rule());
    let m = parse_and_validate(&with_cc).expect("带 {cc} 的规则合法");
    let s = super::codegen::generate(&m).unwrap().to_string();
    assert!(s.contains("__cc"), "用 `{{cc}}` 时必须发射条件绑定：{s}");
}

/// 派生名可以出现在 `vary` 里：自动追加 `eq = [名, 值]` 到 `when`（与核心属性一致）。
#[test]
fn derive_name_in_vary_is_a_predicate_attr() {
    let doc = lowering_doc(
        "[[derive]]\nname = \"is_64\"\nexpr = { eq = [\"rs1_width\", 64] }\n\
         [[lowering]]\nop = \"Copy\"\nvary = { is_64 = [1, 0], m = [\"MOV\", \"MOV\"] }\ninsts = [\"{m} {out}, {0}\"]",
    );
    let m = parse_and_validate(&doc).expect("vary 用派生名必须通过");
    let rs: Vec<_> = m
        .lowering
        .iter()
        .filter(|r| r.op.name() == "Copy")
        .collect();
    assert_eq!(rs.len(), 2, "两行 → 两条规则");
    let whens: Vec<String> = rs.iter().map(|r| format!("{:?}", r.when)).collect();
    assert!(
        whens.iter().any(|w| w.contains("is_64")),
        "派生名应被当作谓词属性（追加 eq）：{whens:?}"
    );
}

/// 派生名与核心属性重名 ⇒ 解析期报错（否则静默遮蔽核心属性）。
#[test]
fn derive_name_must_not_shadow_core_attr() {
    let doc = lowering_doc("[[derive]]\nname = \"rs1_width\"\nexpr = { eq = [\"rd\", 64] }\n");
    let err = parse_and_validate(&doc).unwrap_err();
    let msg = format!("{err:?}");
    assert!(msg.contains("与核心谓词属性重名"), "msg: {msg}");
}

/// 派生名重复 ⇒ 解析期报错。
#[test]
fn derive_duplicate_name_rejected() {
    let doc = lowering_doc(
        "[[derive]]\nname = \"d\"\nexpr = { eq = [\"rd\", 1] }\n\
         [[derive]]\nname = \"d\"\nexpr = { eq = [\"rd\", 2] }\n",
    );
    let err = parse_and_validate(&doc).unwrap_err();
    assert!(format!("{err:?}").contains("名字重复"), "{err:?}");
}

/// `expr` 语法非法 / 类型不对 ⇒ 报错。
#[test]
fn derive_expr_must_be_valid_predicate() {
    let doc = lowering_doc("[[derive]]\nname = \"d\"\nexpr = { nope = [\"rd\", 1] }\n");
    let err = parse_and_validate(&doc).unwrap_err();
    assert!(format!("{err:?}").contains("nope"), "{err:?}");

    let doc = lowering_doc("[[derive]]\nname = \"d\"\nexpr = { eq = [\"rd\", \"x\"] }\n");
    let err = parse_and_validate(&doc).unwrap_err();
    assert!(format!("{err:?}").contains("expr"), "{err:?}");
}

/// **派生不能引用派生**（代换语义不明确）⇒ 明确报错并给出提示。
#[test]
fn derive_cannot_reference_another_derive() {
    let doc = lowering_doc(
        "[[derive]]\nname = \"a\"\nexpr = { eq = [\"rd\", 1] }\n\
         [[derive]]\nname = \"b\"\nexpr = { eq = [\"a\", 1] }\n",
    );
    let err = parse_and_validate(&doc).unwrap_err();
    let msg = format!("{err:?}");
    assert!(msg.contains("未知属性 'a'"), "msg: {msg}");
    assert!(msg.contains("派生不能引用派生"), "应给出提示：{msg}");
}

// ───────────────── S3e：[[pseudo]] 汇编器伪指令 ─────────────────

/// 合法伪指令：声明侧自洽即可（`asm` 首字面 = name、`emit` 行首是指令助记符、
/// `{…}` 都在 `asm` 里声明过、参数都用上）。
#[test]
fn pseudo_decl_compiles_into_expander() {
    let doc = lowering_doc(
        "[[pseudo]]\nname = \"mv\"\nasm = \"mv {dst}, {src}\"\nemit = [\"mov {dst}, {src}\"]",
    );
    let m = parse_and_validate(&doc).expect("合法伪指令必须通过");
    let s: String = super::codegen::generate(&m)
        .unwrap()
        .to_string()
        .chars()
        .filter(|c| !c.is_whitespace())
        .collect();
    assert!(s.contains("__pseudo_expand"), "应生成展开器：{s}");
    // 切参走 `asm` 模板的字面段（`__cut`），不是"按逗号位置猜"；`__split_args` 只用于
    // "最后一个实参里有没有多逗号"这一个校验。
    assert!(s.contains("__cut"), "应生成模板字面切分：{s}");
    assert!(s.contains("__split_args"), "应生成顶层逗号检查：{s}");
    assert!(s.contains("伪指令"), "展开器里应有写法提示：{s}");
}

/// 没有伪指令时**不生成**任何展开器（生成的代码逐字不变）。
#[test]
fn pseudo_absent_keeps_assembler_unchanged() {
    let m = parse_and_validate(&lowering_doc("")).expect("合法");
    let s = super::codegen::generate(&m).unwrap().to_string();
    assert!(!s.contains("__pseudo_expand"), "不该生成展开器：{s}");
    assert!(!s.contains("__cut"), "不该生成切分器：{s}");
}

/// 伪指令名与某条指令模板的**前导字面**重名 ⇒ 报错
/// （伪指令展开先于指令扫描，这个名字开头的写法会被伪指令吃掉）。
#[test]
fn pseudo_name_must_not_shadow_mnemonic() {
    let doc = lowering_doc(
        "[[pseudo]]\nname = \"mov\"\nasm = \"mov {dst}, {src}\"\nemit = [\"mov {dst}, {src}\"]",
    );
    let msg = validation_msg(&doc);
    assert!(msg.contains("重名"), "msg: {msg}");
}

/// `asm` 的形状校验：**字面开头**（分派前缀）、不能为空、占位符之间要有字面分隔。
///
/// `name` **不在**这些规则里：它只是声明的唯一标识，与写法无关。
#[test]
fn pseudo_asm_shape_is_validated() {
    // name 与写法可以完全不同（name 只是标识）——合法
    parse_and_validate(&lowering_doc(
        "[[pseudo]]\nname = \"load_imm\"\nasm = \"li {a}, {b}\"\nemit = [\"mov {a}, {b}\"]",
    ))
    .expect("name 与写法不必相同");

    // 空 asm / 模板以占位符开头
    let empty = lowering_doc("[[pseudo]]\nname = \"mv\"\nasm = \"\"\nemit = [\"mov {a}, {a}\"]");
    assert!(validation_msg(&empty).contains("empty asm template"));

    let op_first =
        lowering_doc("[[pseudo]]\nname = \"mv\"\nasm = \"{a} mv\"\nemit = [\"mov {a}, {a}\"]");
    assert!(validation_msg(&op_first).contains("必须以**字面**开头"));

    // 两个占位符相邻（切不开）
    let adjacent =
        lowering_doc("[[pseudo]]\nname = \"mv\"\nasm = \"mv {a}{b}\"\nemit = [\"mov {a}, {b}\"]");
    assert!(validation_msg(&adjacent).contains("相邻"));

    // name 重复（name 的唯一要求就是唯一）
    let dup = lowering_doc(
        "[[pseudo]]\nname = \"mv\"\nasm = \"mv {a}\"\nemit = [\"mov {a}, {a}\"]\n\
         [[pseudo]]\nname = \"mv\"\nasm = \"mv2 {a}\"\nemit = [\"mov {a}, {a}\"]",
    );
    assert!(validation_msg(&dup).contains("name 重复"), "{dup}");
}

/// 分派键（`asm` 首段的首词）与指令模板的前导字面重名 ⇒ 报错（伪指令先于指令扫描）；
/// 两条伪指令的 `asm` 完全相同 ⇒ 报错（后者永远命中不到）。
#[test]
fn pseudo_dispatch_key_and_duplicate_asm_are_validated() {
    // name 无害，但**写法**首词撞上指令 ⇒ 报错（遮蔽风险在写法上，不在名字上）
    let shadow = lowering_doc(
        "[[pseudo]]\nname = \"alias_mov\"\nasm = \"mov {a}, {b}\"\nemit = [\"mov {a}, {b}\"]",
    );
    let msg = validation_msg(&shadow);
    assert!(msg.contains("重名"), "msg: {msg}");
    assert!(msg.contains("写法"), "msg: {msg}");

    // 同一写法声明两次
    let dup = lowering_doc(
        "[[pseudo]]\nname = \"a1\"\nasm = \"mv {x}, {y}\"\nemit = [\"mov {x}, {y}\"]\n\
         [[pseudo]]\nname = \"a2\"\nasm = \"mv {x}, {y}\"\nemit = [\"mov {y}, {x}\"]",
    );
    assert!(validation_msg(&dup).contains("完全相同"), "{dup}");

    // 同一分派键下的**不同**写法合法（运行时按声明序逐个试）
    parse_and_validate(&lowering_doc(
        "[[pseudo]]\nname = \"a1\"\nasm = \"mv {x}, {y}\"\nemit = [\"mov {x}, {y}\"]\n\
         [[pseudo]]\nname = \"a2\"\nasm = \"mv {x}\"\nemit = [\"mov {x}, {x}\"]",
    ))
    .expect("同键多条写法合法");
}

/// **`asm` 就是参数接口**：`emit` 里用了没在 `asm` 声明的名字 ⇒ 报错并回显 asm；
/// 声明了却没用到 ⇒ 也报错（写错名字的典型症状）。
#[test]
fn pseudo_params_come_from_asm() {
    let typo = lowering_doc(
        "[[pseudo]]\nname = \"mv\"\nasm = \"mv {dst}, {src}\"\nemit = [\"mov {dst}, {srcc}\"]",
    );
    let msg = validation_msg(&typo);
    assert!(msg.contains("不是 asm"), "msg: {msg}");
    assert!(msg.contains("srcc"), "msg: {msg}");

    let unused = lowering_doc(
        "[[pseudo]]\nname = \"mv\"\nasm = \"mv {dst}, {src}\"\nemit = [\"mov {dst}, {dst}\"]",
    );
    let msg = validation_msg(&unused);
    assert!(msg.contains("没用到"), "msg: {msg}");
    assert!(msg.contains("src"), "msg: {msg}");

    // 同一个名字在 asm 里出现两次是合法的（`li {x}, {x}` 这种对称写法）
    parse_and_validate(&lowering_doc(
        "[[pseudo]]\nname = \"mv\"\nasm = \"mv {a}, {a}\"\nemit = [\"mov {a}, {a}\"]",
    ))
    .expect("同名的重复占位符合法");

    // 零操作数的伪指令也合法（`asm = "mv"`，参数表为空；emit 可以展开成伪操作）
    parse_and_validate(&lowering_doc(
        "[[pseudo]]\nname = \"mv\"\nasm = \"mv\"\nemit = [\".byte 1\"]",
    ))
    .expect("零参数伪指令（参数表为空）合法");

    // 但 `asm` 里没有占位符时，emit 里出现 `{a}` 就是"用了没声明的参数" ⇒ 报错
    let undeclared =
        lowering_doc("[[pseudo]]\nname = \"mv\"\nasm = \"mv\"\nemit = [\"mov {a}, {a}\"]");
    assert!(
        validation_msg(&undeclared).contains("不是 asm"),
        "msg: {undeclared}"
    );
}

/// `emit` 的空行/空表、行首接不上任何指令模板、`{…}` 不是参数 ⇒ 都要报错。
#[test]
fn pseudo_emit_is_validated() {
    let empty = lowering_doc("[[pseudo]]\nname = \"mv\"\nasm = \"mv {a}\"\nemit = []");
    assert!(validation_msg(&empty).contains("emit 不能为空"));

    let blank = lowering_doc("[[pseudo]]\nname = \"mv\"\nasm = \"mv {a}\"\nemit = [\"  \"]");
    assert!(validation_msg(&blank).contains("emit 里有空行"));

    let typo_head =
        lowering_doc("[[pseudo]]\nname = \"mv\"\nasm = \"mv {a}\"\nemit = [\"moov {a}, {a}\"]");
    assert!(validation_msg(&typo_head).contains("接不到任何指令模板上"));
}

/// 伪指令的 `emit` 行可以是**别的伪指令名**（递归展开由运行时做，声明期只查名字存在）。
#[test]
fn pseudo_may_emit_another_pseudo() {
    let doc = lowering_doc(
        "[[pseudo]]\nname = \"mv\"\nasm = \"mv {a}\"\nemit = [\"mov {a}, {a}\"]\n\
         [[pseudo]]\nname = \"twice\"\nasm = \"twice {a}, {b}\"\nemit = [\"mv {a}\", \"mv {b}\"]",
    );
    parse_and_validate(&doc).expect("emit 引用别的伪指令必须通过");
}

/// 伪指令校验**不假设"首词 = 助记符"**（v20 V9 修）：v17 起 asm 模板可以操作数前置、
/// 首段可以是多 token 字面，判定改为比**前导字面的 token**（与扫描器同一份词法）。
#[test]
fn pseudo_checks_do_not_assume_first_word_is_mnemonic() {
    // ① 操作数前置的指令（`{dst} = {src}`）+ 行首是参数的 emit：
    //    旧实现按空白切词（首词 `{dst}`）判定，会把合法的 emit 行判成拼错。
    let operand_first = lowering_doc("")
        .replace("asm = \"mov {dst}, {src}\"", "asm = \"{dst} = {src}\"")
        + "[[pseudo]]\nname = \"assign\"\nasm = \"assign {a}, {b}\"\nemit = [\"{a} = {b}\"]\n";
    parse_and_validate(&operand_first).expect("操作数前置的模板不该被当成助记符表");
    //    同理，伪指令名不以任何模板的**前导字面**开头 ⇒ 不算重名（首段是操作数，比不了）。
    let op_first_name = lowering_doc("")
        .replace("asm = \"mov {dst}, {src}\"", "asm = \"{dst} = {src}\"")
        + "[[pseudo]]\nname = \"dst\"\nasm = \"dst {a}\"\nemit = [\"{a} = {a}\"]\n";
    parse_and_validate(&op_first_name).expect("首段是操作数时，名字不构成遮蔽");

    // ② 多 token 前导字面：`set if …` 的 emit 行以 `set if` 开头 ⇒ 接得上；
    //    改一个 token（`sett`）⇒ 拼错。旧实现只看首词 `set`，两种情况都放过。
    let two_tok = lowering_doc("").replace(
        "asm = \"mov {dst}, {src}\"",
        "asm = \"set if {dst}, {src}\"",
    );
    let ok = two_tok.clone()
        + "[[pseudo]]\nname = \"si\"\nasm = \"si {a}, {b}\"\nemit = [\"set if {a}, {b}\"]\n";
    parse_and_validate(&ok).expect("多 token 前导字面的 emit 行必须通过");
    let bad =
        two_tok + "[[pseudo]]\nname = \"si\"\nasm = \"si {a}, {b}\"\nemit = [\"sett {a}, {b}\"]\n";
    assert!(
        validation_msg(&bad).contains("接不到任何指令模板上"),
        "多 token 前导字面只有首个 token 相同不算接得上"
    );

    // ③ 遮蔽检测跟着扫描器的**大小写口径**走：不敏感 ISA（缺省）里 `MOV` 与 `mov` 撞车，
    //    显式 `mnemonic_case = "sensitive"` 的 ISA 里不撞（`__eat_name` 只在
    //    不敏感档忽略大小写）。
    let ci =
        lowering_doc("[[pseudo]]\nname = \"MOV\"\nasm = \"MOV {a}\"\nemit = [\"mov {a}, {a}\"]");
    assert!(
        validation_msg(&ci).contains("重名"),
        "大小写不敏感 ISA（缺省）里 MOV 与 mov 是同一个写法"
    );
    let cs =
        lowering_doc("[[pseudo]]\nname = \"MOV\"\nasm = \"MOV {a}\"\nemit = [\"mov {a}, {a}\"]")
            .replace(
                "name = \"x\"",
                "name = \"x\"\nmnemonic_case = \"sensitive\"",
            );
    parse_and_validate(&cs).expect("敏感 ISA 里 MOV 与 mov 是两回事");
}

// ───────────────── 结构完善：asm 完整格式 + 通用模板段 ─────────────────

/// 定宽最小模型（供 codegen 测试）。
fn gen_min_model(inst_body: &str) -> super::model::IsaModel {
    let doc = format!(
        r#"
[meta]
name = "x"
[encoding]
kind = "fixed"
bits = 32
[reg.gpr32]
count = 8
[operand.g]
kind = "reg"
class = "gpr"
[operand.i]
kind = "imm"
signed = true
bits = 12
[[forms]]
name = "I"
fields = [
  "u7[6:0]:opcode",
  "u3[9:7]:rd",
  "u3[17:15]:rs1",
  "u12[31:20]:imm12",
  "u3[14:12]:funct3",
]
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
match = { funct3 = 0 }
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
[encoding]
kind = "fixed"
bits = 32
[reg.gpr32]
count = 8
[operand.g]
kind = "reg"
class = "gpr32"
roles = "inout"
[[forms]]
name = "RR"
fields = [
  "u8[7:0]:opcode",
  "u3[10:8]:rd",
  "u3[13:11]:rs1",
]
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
fn asm_scan_probe_replaces_mnemonic_table() {
    // 桶判定（"这段是不是本 ISA 的指令"）由**线性扫描的第一步**回答，不再发射
    // 生成期切词得到的"助记符表"：模板可以操作数前置、首段可以多 token，切词没有意义。
    let model = gen_min_model(
        r#"
[[instructions]]
name = "ADDI"
form = "I"
opcode = 0x13
match = { funct3 = 0 }
ops = ["dst:g:out", "src:g", "src2:i"]
asm = "addi {dst}, {src}, {src2}"
"#,
    );
    let s = super::codegen::generate(&model).unwrap().to_string();
    assert!(
        s.contains("could_be_instruction"),
        "应发射线性扫描探针 could_be_instruction：{s}"
    );
    assert!(
        !s.contains("mnemonic_table"),
        "不应再有生成期切词出来的助记符表：{s}"
    );
    // 探针里真的跑了首段匹配器（try 一处 + 探针一处）
    assert!(
        s.matches("__eat_name").count() >= 3,
        "探针应复用首段字面匹配（__eat_name）：{s}"
    );
}

#[test]
fn missing_asm_is_parse_error() {
    // asm 必填（v17）：缺省不再自动派生，缺 asm → Parse 错误
    let doc = r#"
[meta]
name = "x"
[encoding]
kind = "fixed"
bits = 32
[reg.gpr32]
count = 8
[operand.g]
kind = "reg"
class = "gpr"
[[forms]]
name = "R"
fields = [
  "u7[6:0]:opcode",
  "u3[9:7]:rd",
]
[[instructions]]
name = "ADD"
form = "R"
opcode = 0x13
ops = ["dst:g:out"]
"#;
    let err = parse_and_validate(doc).expect_err("缺 asm 必须报错");
    assert!(
        matches!(err, crate::dsl::DslError::Parse { .. }),
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
[encoding]
kind = "fixed"
bits = 32
[reg.gpr32]
count = 8
[operand.g]
kind = "reg"
class = "gpr"
[[forms]]
name = "R"
fields = [
  "u7[6:0]:opcode",
  "u3[9:7]:rd",
]
[[instructions]]
name = "FOO"
form = "R"
opcode = 0x13
ops = ["dst:g:out"]
asm = "{bad {dst}"
"#;
    let err = parse_and_validate(doc).unwrap_err();
    match err {
        DslError::Validation { msg, .. } => {
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
match = { funct3 = 0 }
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
match = { funct3 = 0 }
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
[encoding]
kind = "fixed"
bits = 32
[reg.gpr32]
count = 8
[operand.g]
kind = "reg"
class = "gpr"
[operand.i]
kind = "imm"
signed = true
bits = 12
[[forms]]
name = "I"
fields = [
  "u7[6:0]:opcode",
  "u3[9:7]:rd",
  "u3[17:15]:rs1",
  "u12[31:20]:imm12",
]
[[instructions]]
name = "BAD"
form = "I"
opcode = 0x13
match = { funct3 = 0 }
ops = ["dst:g:out", "src:g"]
asm = "bad {dst}, {srcc}"
"#;
    let err = parse_and_validate(doc).unwrap_err();
    match err {
        DslError::Validation { msg, .. } => {
            assert!(msg.contains("'{srcc}'"), "msg: {msg}");
            assert!(msg.contains("dst, src"), "需列出已声明的名字: {msg}");
        }
        other => panic!("expected Validation error, got {other:?}"),
    }
}

// ─────────────────────── [[reloc]]（v18 S3d 重定位数据化）───────────────────────

/// 重定位夹具：`[[reloc]]` 表 + 一条引用它的指令（pc_relative / 20 位立即数槽）。
fn reloc_doc(reloc_tbl: &str, inst_reloc: &str) -> String {
    format!(
        r#"
[meta]
name = "t"
[encoding]
kind = "fixed"
bits = 32
[reg.gpr32]
count = 8
[operand.g]
kind = "reg"
class = "gpr32"
[operand.imm20]
kind = "imm"
bits = 20
[[forms]]
name = "U"
fields = [
  "u7[6:0]:opcode",
  "u5[11:7]:rd",
  "u20[31:12]:imm20",
]
{reloc_tbl}
[[instructions]]
name = "AUIPC_GLOBAL"
form = "U"
opcode = 0x17
ops = ["dst:g:out", "imm:imm20"]
asm = "auipc.g {{dst}}, {{imm}}"
{inst_reloc}
"#
    )
}

/// 合法表项：指令只写 `reloc = "<名>"`，语义/绑定槽在表里。
#[test]
fn reloc_ref_resolves_through_the_table() {
    let doc = reloc_doc(
        "[[reloc]]\nname = \"pcrel_hi\"\nsemantics = \"pc_relative\"\nslot = \"imm20\"\n",
        "reloc = \"pcrel_hi\"",
    );
    let m = parse_and_validate(&doc).expect("合法 reloc 表必须通过");
    let inst = m
        .instructions
        .iter()
        .find(|i| i.name == "AUIPC_GLOBAL")
        .expect("instruction present");
    assert_eq!(inst.reloc.as_deref(), Some("pcrel_hi"));
    assert_eq!(m.reloc.len(), 1);
    assert_eq!(m.reloc[0].name, "pcrel_hi");
    // 生成期：encoder 发 `Relative(字长, 0)` 的 "G{id}" 重定位
    let s: String = super::codegen::generate(&m)
        .unwrap()
        .to_string()
        .chars()
        .filter(|c| !c.is_whitespace())
        .collect();
    assert!(
        s.contains("RelocKind::Relative(4,0)"),
        "pc_relative 应发相对重定位：{s}"
    );
    assert!(s.contains("\"G{}\""), "符号名应为 G{{id}}：{s}");
}

/// 指令引用了未声明的重定位名 ⇒ 编译期报错并列出可用名。
#[test]
fn reloc_unknown_name_rejected() {
    let doc = reloc_doc(
        "[[reloc]]\nname = \"pcrel_hi\"\nsemantics = \"pc_relative\"\nslot = \"imm20\"\n",
        "reloc = \"bogus\"",
    );
    let msg = validation_msg(&doc);
    assert!(msg.contains("未在 [[reloc]] 声明"), "msg: {msg}");
    assert!(msg.contains("pcrel_hi"), "应列出可用名：{msg}");
}

/// 表项绑定的槽未声明 / 不是 imm 槽 ⇒ 报错。
#[test]
fn reloc_bad_slot_rejected() {
    let doc = reloc_doc(
        "[[reloc]]\nname = \"x\"\nsemantics = \"pc_relative\"\nslot = \"nope\"\n",
        "",
    );
    let msg = validation_msg(&doc);
    assert!(msg.contains("未在 [operand."), "msg: {msg}");

    let doc = reloc_doc(
        "[[reloc]]\nname = \"x\"\nsemantics = \"pc_relative\"\nslot = \"g\"\n",
        "",
    );
    let msg = validation_msg(&doc);
    assert!(msg.contains("必须是 imm 槽"), "msg: {msg}");
}

/// 表项名重复 ⇒ 报错。
#[test]
fn reloc_duplicate_name_rejected() {
    let doc = reloc_doc(
        "[[reloc]]\nname = \"x\"\nsemantics = \"pc_relative\"\nslot = \"imm20\"\n\
         [[reloc]]\nname = \"x\"\nsemantics = \"absolute\"\nslot = \"imm20\"\n",
        "",
    );
    let msg = validation_msg(&doc);
    assert!(msg.contains("重定位名重复"), "msg: {msg}");
}

/// 指令有 reloc 但操作数里没有那个槽 ⇒ 报错（否则 encoder 会 panic 到 unreachable）。
#[test]
fn reloc_slot_not_in_operands_rejected() {
    let doc = reloc_doc(
        "[[reloc]]\nname = \"x\"\nsemantics = \"absolute\"\nslot = \"imm20\"\n",
        "reloc = \"x\"\nmatch = { rd = 0 }",
    )
    .replace(
        "ops = [\"dst:g:out\", \"imm:imm20\"]",
        "ops = [\"dst:g:out\"]",
    )
    .replace("asm = \"auipc.g {dst}, {imm}\"", "asm = \"auipc.g {dst}\"");
    let msg = validation_msg(&doc);
    assert!(msg.contains("不在该指令的操作数里"), "msg: {msg}");
}

/// `absolute` 语义的补丁宽度取自**槽宽**（不是写死的 8 字节）：64 位槽 → Absolute(8)、
/// 32 位槽 → Absolute(4)。
#[test]
fn reloc_absolute_width_comes_from_slot() {
    let doc = reloc_doc(
        "[[reloc]]\nname = \"abs\"\nsemantics = \"absolute\"\nslot = \"imm20\"\n",
        "reloc = \"abs\"",
    );
    let m = parse_and_validate(&doc).expect("合法");
    let s: String = super::codegen::generate(&m)
        .unwrap()
        .to_string()
        .chars()
        .filter(|c| !c.is_whitespace())
        .collect();
    // imm20 → 20 位 → ceil(20/8) = 3 字节
    assert!(s.contains("Absolute(3)"), "补丁宽度应 = ceil(槽宽/8)：{s}");
}

/// 语义名非法 ⇒ serde 拒绝（宿主语义是有限集合）。
#[test]
fn reloc_semantics_must_be_host_known() {
    let doc = reloc_doc(
        "[[reloc]]\nname = \"x\"\nsemantics = \"got\"\nslot = \"imm20\"\n",
        "",
    );
    let err = parse_and_validate(&doc).unwrap_err();
    let msg = format!("{err:?}");
    assert!(msg.contains("got"), "msg: {msg}");
    assert!(
        msg.contains("pc_relative") || msg.contains("absolute"),
        "应列出可用语义：{msg}"
    );
}

/// `effect = ["Move"]` 语义标签解析（is_move 声明，替代指令名前缀启发式）。
#[test]
fn effect_move_label_parses() {
    let doc = r#"
[meta]
name = "t"
[encoding]
kind = "fixed"
bits = 32
[reg.gpr32]
count = 8
[operand.g]
kind = "reg"
class = "gpr32"
[[forms]]
name = "R"
fields = [
  "u7[6:0]:opcode",
  "u5[11:7]:rd",
  "u5[19:15]:rs1",
  "u5[24:20]:rs2",
  "u3[14:12]:funct3",
  "u7[31:25]:funct7",
]
[[instructions]]
name = "MY_MOV"
form = "R"
opcode = 0x33
match = { funct3 = 0, funct7 = 0 }
ops = ["dst:g:out", "src:g", "src2:g"]
asm = "mymov {dst}, {src}, {src2}"
effect = ["Move"]
"#;
    let m = parse_and_validate(doc).expect("doc with Move effect must parse");
    let inst = m.instructions.iter().find(|i| i.name == "MY_MOV").unwrap();
    assert!(inst.effect.contains(&crate::dsl::model::Effect::Move));
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
[encoding]
kind = "fixed"
bits = 32
[reg.gpr32]
count = 8
[operand.g]
kind = "reg"
class = "gpr32"
roles = "inout"
[[forms]]
name = "RR"
fields = [
  "u8[7:0]:opcode",
  "u3[10:8]:rd",
  "u3[13:11]:rs1",
]
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

/// `[[pattern]]` 宿主谱：在最小谱基础上补几条指令，供 pattern 的 `insts` 引用。
fn pattern_doc(patterns: &str) -> String {
    let mut doc = lowering_doc("");
    for (i, name) in ["ADD", "SUB", "MUL", "ADDSS", "SUBSS"].iter().enumerate() {
        doc.push_str(&format!(
            "\n[[instructions]]\nname = \"{name}\"\nform = \"RR\"\nopcode = {}\n\
             ops = [\"dst:g:out\", \"src:g\"]\nasm = \"{lower} {{dst}}, {{src}}\"\n",
            i + 2,
            name = name,
            lower = name.to_lowercase(),
        ));
    }
    doc.push_str(patterns);
    doc
}

fn lowering_err(rule: &str) -> String {
    match parse_and_validate(&lowering_doc(rule)).unwrap_err() {
        DslError::Validation { msg, .. } => msg,
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

/// 模板展开出的实例指令名也算已声明（实例名 = `rows[].inst`）。
#[test]
fn lowering_accepts_template_instance_name() {
    let rule = "[[templates]]\nname = \"F\"\n\
                body = { form = \"RR\", ops = [\"dst:g:out\", \"src:g\"], \
                asm = \"{inst.lower} {dst}, {src}\" }\n\
                rows = [ { inst = \"NEG\", opcode = 9 } ]\n\
                [[lowering]]\nop = \"Ineg\"\ninsts = [\"NEG {out}, {0}\"]";
    parse_and_validate(&lowering_doc(rule)).expect("模板实例指令名必须被识别");
}

// ─────────── 指令级 `ref`（多态引用 / 1:1 / 校验） ───────────
//
// 旧 `[[aliases]]` 已并入指令属性：`ref = "名字"`。多条指令共用同一个 `ref`
// 即多态引用（原先"别名指向多成员"的场景），单条指令给 `ref` 即 1:1 引用。
// `ref` 与指令名同池（lowering/pattern/emit 行首按它分派），故不得与指令名冲突。

fn ref_doc(r16: &str, r32: &str, extra: &str) -> String {
    format!(
        r#"
[meta]
name = "x"
[encoding]
kind = "fixed"
bits = 32
[reg.gpr32]
count = 8
[operand.g]
kind = "reg"
class = "gpr32"
roles = "inout"
[[forms]]
name = "RR"
fields = [
  "u8[7:0]:opcode",
  "u3[10:8]:rd",
  "u3[13:11]:rs1",
]
[[instructions]]
name = "MOV16"
{r16}form = "RR"
opcode = 1
ops = ["dst:g:out", "src:g"]
asm = "mov {{dst}}, {{src}}"
[[instructions]]
name = "MOV32"
{r32}form = "RR"
opcode = 2
ops = ["dst:g:out", "src:g"]
asm = "mov {{dst}}, {{src}}"
{extra}
"#
    )
}

/// 注入一行 `ref = "名字"`（空名字用于"ref 不能为空"用例）。
fn ref_line(name: &str) -> String {
    format!("ref = \"{name}\"\n")
}

#[test]
fn ref_polymorphic_lowering_accepted() {
    // 两条指令共用 `ref = "mov"`；lowering 引用该引用名 → 解析通过
    //（具体选哪条由 codegen 按操作数签名消歧）
    let doc = ref_doc(
        &ref_line("mov"),
        &ref_line("mov"),
        "[[lowering]]\nop = \"Copy\"\ninsts = [\"mov {out}, {0}\"]",
    );
    parse_and_validate(&doc).expect("多态 ref + lowering 引用必须通过");
}

#[test]
fn ref_1_to_1_lowering_accepted() {
    // 单条指令给 `ref`（1:1）——等价于直接引用指令名，仍须被 lowering 接受
    let doc = ref_doc(
        &ref_line("copy"),
        "",
        "[[lowering]]\nop = \"Copy\"\ninsts = [\"copy {out}, {0}\"]",
    );
    parse_and_validate(&doc).expect("1:1 ref + lowering 引用必须通过");
}

#[test]
fn ref_rejects_conflict_with_inst_name() {
    // 引用名与指令名同池：`MOV32` 已被指令占用
    let doc = ref_doc(&ref_line("MOV32"), "", "");
    let msg = match parse_and_validate(&doc).unwrap_err() {
        DslError::Validation { msg, .. } => msg,
        other => panic!("expected Validation error, got {other:?}"),
    };
    assert!(msg.contains("与指令名冲突"), "msg: {msg}");
}

#[test]
fn ref_rejects_empty() {
    let doc = ref_doc(&ref_line(""), "", "");
    let msg = match parse_and_validate(&doc).unwrap_err() {
        DslError::Validation { msg, .. } => msg,
        other => panic!("expected Validation error, got {other:?}"),
    };
    assert!(msg.contains("ref 不能为空"), "msg: {msg}");
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
    let rs: Vec<_> = m
        .lowering
        .iter()
        .filter(|r| r.op.name() == "Vadd")
        .collect();
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
        DslError::Parse { msg, .. } => msg,
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
        DslError::Parse { msg, .. } => msg,
        other => panic!("expected Parse error, got {other:?}"),
    };
    assert!(msg.contains("必须是整数"), "msg: {msg}");
}

/// S5a：`op` 名单 = 一条规则服务多个同类 op，解析期展开成逐 op 的规则。
#[test]
fn op_list_expands_to_one_rule_per_op() {
    let doc = lowering_doc(
        "[[lowering]]\nop = [\"Copy\", \"Uextend\", \"Freeze\"]\ninsts = [\"MOV {out}, {0}\"]",
    );
    let m = parse_and_validate(&doc).expect("op 名单必须通过");
    assert_eq!(m.lowering.len(), 3, "3 个 op → 3 条规则");
    let names: Vec<&str> = m.lowering.iter().map(|r| r.op.name()).collect();
    assert_eq!(names, ["Copy", "Uextend", "Freeze"], "展开序 = 名单序");
    for r in &m.lowering {
        assert_eq!(r.insts, vec!["MOV {out}, {0}"], "三个 op 共用同一份序列");
        assert_eq!(r.op.names().len(), 1, "展开后每条规则只有一个 op");
    }
    assert_eq!(m.lowering_by_op().len(), 3, "按 op 分组 = 3 组");
}

/// S5a：名单展开后，**每个 op 内部的规则相对顺序**与逐条写开完全一致（裁决序不变）。
#[test]
fn op_list_keeps_declaration_order_within_each_op() {
    let doc = lowering_doc(
        "[[lowering]]\nop = [\"A\", \"B\"]\ninsts = [\"MOV {out}, {0}\"]\n\n\
         [[lowering]]\nop = \"A\"\nwhen = { eq = [\"rd\", 32] }\ninsts = [\"MOV {out}, {0}\"]",
    );
    let m = parse_and_validate(&doc).expect("名单 + 具体规则必须通过");
    let by_op = m.lowering_by_op();
    // 每个 op 的规则数（名单那条给 A/B 各一条；具体那条只给 A）
    let count = |name: &str| {
        by_op
            .iter()
            .find(|(op, _)| *op == name)
            .map(|(_, rs)| rs.len())
            .unwrap_or(0)
    };
    assert_eq!(count("A"), 2, "A：名单那条 + 具体那条");
    assert_eq!(count("B"), 1, "B：只有名单那条");
    // 裁决序 = (priority 降, 谓词叶子数降, 声明序升)：带 when 的那条（1 叶子）先试。
    let a_rules: &Vec<_> = &by_op.iter().find(|(op, _)| *op == "A").unwrap().1;
    assert!(a_rules[0].when.is_some(), "叶子多的排前面");
    assert!(a_rules[1].when.is_none(), "名单那条无 when");
    assert_eq!(a_rules[1].insts, vec!["MOV {out}, {0}"], "名单那条的序列");
}

/// S5a：名单里的空名 / 重复名必须报错（而不是静默产生两条同名规则）。
#[test]
fn op_list_rejects_empty_and_duplicate_names() {
    for (body, want) in [
        (
            "[[lowering]]\nop = []\ninsts = [\"MOV {out}, {0}\"]",
            "不能为空",
        ),
        (
            "[[lowering]]\nop = [\"Copy\", \"\"]\ninsts = [\"MOV {out}, {0}\"]",
            "op 名不能为空",
        ),
        (
            "[[lowering]]\nop = [\"Copy\", \"Copy\"]\ninsts = [\"MOV {out}, {0}\"]",
            "重复",
        ),
    ] {
        let doc = lowering_doc(body);
        let msg = match parse_and_validate(&doc).unwrap_err() {
            DslError::Parse { msg, .. } => msg,
            other => panic!("expected Parse error, got {other:?}"),
        };
        assert!(msg.contains(want), "期望 {want:?}，实际：{msg}");
    }
}

/// S5c：`[[pattern]]` 的裁决序 = (`priority` 降, Op 节点数降, when 叶子数降, 声明序升)。
#[test]
fn pattern_order_uses_priority_then_specificity() {
    // A：单节点树、无 when；B：两节点树、无 when；C：单节点树 + 1 个 when 叶子但 priority=1。
    let doc = pattern_doc(
        "[[pattern]]\nmatch = \"Fadd(a, b)\"\ninsts = [\"ADD {out}, {a}, {b}\"]\n\n\
         [[pattern]]\nmatch = \"Fadd(Fmul(a, b), c)\"\ninsts = [\"MOV {out}, {a}\", \"MUL {out}, {b}\", \"ADD {out}, {c}\"]\n\n\
         [[pattern]]\nmatch = \"Fadd(a, b)\"\nwhen = { eq = [\"elem\", 1] }\npriority = 5\ninsts = [\"ADDSS {out}, {a}, {b}\"]",
    );
    let m = parse_and_validate(&doc).expect("pattern 必须通过");
    let order = m.pattern_order().expect("裁决序");
    // priority=5 的排第一；其余按 Op 节点数降（B=3 节点 > A=1 节点）。
    assert_eq!(order[0], 2, "priority 最大者先");
    assert_eq!(order[1], 1, "其次 Op 节点数多的");
    assert_eq!(order[2], 0, "最后是同形状但无 priority 的");
}

/// S5c：同一匹配树、`when` 被前序模式覆盖 → 死模式，编译期报错。
#[test]
fn dead_pattern_is_rejected() {
    let doc = pattern_doc(
        "[[pattern]]\nmatch = \"Fadd(a, b)\"\nwhen = { le = [\"elem\", 2] }\ninsts = [\"ADD {out}, {a}, {b}\"]\n\n\
         [[pattern]]\nmatch = \"Fadd(a, b)\"\nwhen = { eq = [\"elem\", 1] }\ninsts = [\"ADDSS {out}, {a}, {b}\"]",
    );
    // `DslError` 只有 Parse/Validation 两个变体（死模式属校验期，但两种都接受）。
    let msg = match parse_and_validate(&doc).unwrap_err() {
        DslError::Validation { msg, .. } | DslError::Parse { msg, .. } => msg,
    };
    assert!(msg.contains("死模式"), "msg: {msg}");
}

/// S5c：同一匹配树 + 同一 `when`（完全重复）也是死模式。
#[test]
fn duplicate_pattern_is_rejected() {
    let one = "[[pattern]]\nmatch = \"Fadd(a, b)\"\ninsts = [\"ADD {out}, {a}, {b}\"]";
    let doc = pattern_doc(&format!("{one}\n\n{one}"));
    // `DslError` 只有 Parse/Validation 两个变体（死模式属校验期，但两种都接受）。
    let msg = match parse_and_validate(&doc).unwrap_err() {
        DslError::Validation { msg, .. } | DslError::Parse { msg, .. } => msg,
    };
    assert!(msg.contains("死模式"), "msg: {msg}");
}

/// S5c：**不同**匹配树之间不做覆盖推断（保守：不误报）。
#[test]
fn different_trees_are_not_dead() {
    let doc = pattern_doc(
        "[[pattern]]\nmatch = \"Fadd(a, b)\"\nwhen = { eq = [\"elem\", 1] }\ninsts = [\"ADDSS {out}, {a}, {b}\"]\n\n\
         [[pattern]]\nmatch = \"Fsub(a, b)\"\nwhen = { eq = [\"elem\", 1] }\ninsts = [\"SUBSS {out}, {a}, {b}\"]",
    );
    assert!(parse_and_validate(&doc).is_ok(), "不同树不应报死模式");
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
        DslError::Validation { msg, .. } => msg,
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
[encoding]
kind = "prefix_scan"
max_len = 15
[reg.gpr64]
count = 16
[operand.g]
kind = "reg"
class = "gpr64"
roles = "inout"
[[instructions]]
name = "MOVQ_REV"
modrm = { reg = "src", rm = "dst" }
opsize = 64
escape = [0x0F]
prefix = "field"
opcode = 0x7E
match = { prefix = 0x66, w = 1 }
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
[encoding]
kind = "prefix_scan"
[reg.gpr64]
count = 16
[operand.g]
kind = "reg"
class = "gpr64"
roles = "inout"
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
    assert_eq!(enc.opsize, Some(crate::dsl::model::Opsize::Slot(0)));
}

/// opsize 固定宽度写**位宽整数**；旧的 "rN"（字节）写法给出明确迁移提示。
#[test]
fn opsize_fixed_width_is_bits() {
    use crate::dsl::model::Opsize;
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
[encoding]
kind = "prefix_scan"
[reg.gpr64]
count = 16
[operand.gx]
kind = "reg"
class = "gpr64"
roles = "inout"
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
        Some(crate::dsl::model::Opsize::Named("dst".into()))
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
[encoding]
kind = "prefix_scan"
[reg.gpr64]
count = 16
[operand.gx]
kind = "reg"
class = "gpr64"
roles = "inout"
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
        DslError::Validation { msg, .. } => msg,
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
    assert_eq!(mm.reg, Some(crate::dsl::model::ModrmReg::Op("src".into())));
    assert_eq!(mm.rm_operand(), (false, Some("dst")));
}

/// `reg = <整数>` = 固定扩展码（取代 `"ext"` + `fields.ext` 两处声明）。
#[test]
fn modrm_map_reg_integer_is_ext_code() {
    let m = parse_and_validate(&modrm_doc(r#"modrm = { reg = 3, rm = "dst" }"#)).expect("valid");
    let mm = m.instructions[0].enc.modrm.as_ref().unwrap();
    assert_eq!(mm.reg, Some(crate::dsl::model::ModrmReg::Ext(3)));
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
[encoding]
kind = "prefix_scan"
[reg.gpr64]
count = 16
[operand.gx]
kind = "reg"
class = "gpr64"
roles = "inout"
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
/// `require_symbol`（重定位修饰形态专用）：只接受符号/带修饰的引用。
///
/// 校验层钉住三条：① 必须同时声明 `symbols = true`；② 只能落在 `imm` 槽上；
/// ③ 与 `symbols` 同款要求 `unit == 1`。**运行时行为**（数值写法落回数值槽）交给
/// aarch64 语料的字节对拍档验证——那里有真实的反例（`ldr x0, [x1, #8]` 曾被带修饰的
/// 变体接走、编成字段 8 而不是 1）。
#[test]
fn require_symbol_validates() {
    const BASE: &str = r#"
[meta]
name = "x"
[encoding]
kind = "fixed"
bits = 32
[reg.gpr64]
count = 16
[operand.gx]
kind = "reg"
class = "gpr64"
roles = "inout"
"#;
    // ③ unit != 1：`symbols` 那条先拦（`require_symbol` 依赖它）
    let doc = BASE.to_string()
        + r#"
[operand.sym12]
kind = "imm"
signed = false
bits = 12
unit = 8
symbol = { allow = true, require = true }
[[instructions]]
name = "I"
opcode = 1
ops = ["dst:gx:inout", "imm:sym12"]
asm = "i {dst}, {imm}"
"#;
    let err = parse_and_validate(&doc).expect_err("unit != 1 必须报错");
    assert!(format!("{err:?}").contains("unit == 1"), "{err:?}");

    // ① 没声明 `symbols`
    let doc = BASE.to_string()
        + r#"
[operand.sym12]
kind = "imm"
signed = false
bits = 12
symbol = { require = true }
[[instructions]]
name = "I"
opcode = 1
ops = ["dst:gx:inout", "imm:sym12"]
asm = "i {dst}, {imm}"
"#;
    let err = parse_and_validate(&doc).expect_err("缺 symbols 必须报错");
    assert!(format!("{err:?}").contains("symbol.require"), "{err:?}");

    // ② 非 imm 槽
    let doc = BASE.to_string()
        + r#"
[operand.gl]
kind = "label"
bits = 4
symbol = { allow = true, require = true }
[[instructions]]
name = "I"
opcode = 1
ops = ["dst:gx:inout", "t:gl"]
asm = "i {dst}, {t}"
"#;
    let err = parse_and_validate(&doc).expect_err("非 imm 槽必须报错");
    assert!(format!("{err:?}").contains("symbol.require"), "{err:?}");

    // 正例：symbols + require_symbol + unit 1 通过校验
    let doc = BASE.to_string()
        + r#"
[operand.sym12]
kind = "imm"
signed = false
bits = 12
symbol = { allow = true, require = true }
[[instructions]]
name = "I"
opcode = 1
ops = ["dst:gx:inout", "imm:sym12"]
asm = "i {dst}, {imm}"
"#;
    parse_and_validate(&doc).expect("symbols + require_symbol + unit 1 必须通过");
}

const PATTERN_BASE: &str = r#"
[meta]
name = "x"
[encoding]
kind = "prefix_scan"
[reg.gpr64]
count = 16
[operand.gx]
kind = "reg"
class = "gpr64"
roles = "inout"
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
    let s = gen_mem_parser(&[items], &[], false).to_string();
    let c = s.replace(' ', "");
    assert!(
        c.contains("__expr"),
        "MIPS disp 走共用的值语法（含符号常量/一元 ±）：{s}"
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
[encoding]
kind = "fixed"
bits = 32
[reg.gpr32]
count = 8
[operand.mem]
kind = "mem"
text = ["{disp}({base})"]
[operand.g]
kind = "reg"
class = "gpr"
[[forms]]
name = "R"
fields = [
  "u7[6:0]:opcode",
  "u3[9:7]:rd",
]
[[instructions]]
name = "ADD"
form = "R"
opcode = 0x13
ops = ["dst:g:out"]
asm = "add {dst}"
"#;
    parse_and_validate(base).expect("MIPS 模板合法");
    // 不写 `{base}` = **无基址写法**（v20 V10）：只要有 {disp}/{index} 就合法。
    let ok = base.replace(
        r#"text = ["{disp}({base})"]"#,
        r#"text = ["{disp}"]"#,
    );
    parse_and_validate(&ok).expect("无基址模板（只有 {disp}）合法");
    // 无基址且**没有任何地址组件** → 报错（那只能匹配空文本）
    let bad = base.replace(r#"text = ["{disp}({base})"]"#, r#"text = ["[]"]"#);
    let err = parse_and_validate(&bad).unwrap_err().to_string();
    assert!(err.contains("无基址模板"), "err: {err}");
    // 两个 {base} → 报错
    let bad = base.replace(
        r#"text = ["{disp}({base})"]"#,
        r#"text = ["[{base}+{base}]"]"#,
    );
    let err = parse_and_validate(&bad).unwrap_err().to_string();
    assert!(err.contains("最多只能有一个"), "err: {err}");
    // scale 未紧随 index → 报错
    let bad = base.replace(
        r#"text = ["{disp}({base})"]"#,
        r#"text = ["[{base}*{scale}]"]"#,
    );
    let err = parse_and_validate(&bad).unwrap_err().to_string();
    assert!(err.contains("index"), "err: {err}");
    // 未知占位符 → 报错
    let bad = base.replace(
        r#"text = ["{disp}({base})"]"#,
        r#"text = ["[{base}+{foo}]"]"#,
    );
    let err = parse_and_validate(&bad).unwrap_err().to_string();
    assert!(err.contains("foo"), "err: {err}");
    // 空列表 → 报错（第 0 条是渲染形态，不能省）
    let bad = base.replace(r#"text = ["{disp}({base})"]"#, r#"text = []"#);
    let err = parse_and_validate(&bad).unwrap_err().to_string();
    assert!(err.contains("templates"), "err: {err}");
    // 用了 {size} 却没说 size_keywords → 报错
    let bad = base.replace(
        r#"text = ["{disp}({base})"]"#,
        r#"text = ["{size}{disp}({base})"]"#,
    );
    let err = parse_and_validate(&bad).unwrap_err().to_string();
    assert!(err.contains("size_keywords"), "err: {err}");
    // 声明了 size_keywords 但模板里没 {size} → 合法（表只是能力申报）
    let ok = base.replace(
        r#"text = ["{disp}({base})"]"#,
        "text = [\"{disp}({base})\"]\nsize_words = [\"qword ptr\"]",
    );
    parse_and_validate(&ok).expect("多余的 size_keywords 不报错");
}

/// 内存模板列表（v20 V10）：第 0 条渲染、其余解析专用；`{size}` 关键字表。
#[test]
fn mem_template_list_and_size_component() {
    use super::codegen::mem::{
        Comp, Item, effective_templates, gen_mem_parser, gen_render_mem, parse_mem_template,
        size_keyword_toks,
    };
    use super::model::MemTemplate;

    // `{size}` 解析：列进 Comp::Size。
    let items = parse_mem_template("{size}[{base}]").unwrap();
    assert!(matches!(&items[0], Item::Comp(Comp::Size)), "{items:?}");
    assert!(matches!(&items[1], Item::Lit { text, .. } if text == "["));
    assert!(matches!(&items[2], Item::Comp(Comp::Base)));

    // 渲染只用第 0 条：`{size}` 输出空串，紧邻它的 `[` **不**被当条件前缀吞掉。
    let render = gen_render_mem(&items).to_string().replace(' ', "");
    assert!(
        render.contains("\"[\""),
        "紧邻 {{size}} 的字面量应恒发出：{render}"
    );
    assert!(
        !render.contains("QWORD"),
        "渲染不该输出尺寸关键字：{render}"
    );

    // 解析：两条模板 → `__mem_try0`/`__mem_try1`，`__mem` 按序兜底。
    let t0 = parse_mem_template("{size}[{base}+{disp}]").unwrap();
    let t1 = parse_mem_template("{disp}[{base}]").unwrap();
    let kws = size_keyword_toks(&Some(MemTemplate {
        templates: vec!["{size}[{base}]".into(), "{disp}[{base}]".into()],
        size_keywords: vec!["qword ptr".into()],
    }))
    .unwrap();
    assert_eq!(kws.len(), 1);
    assert_eq!(kws[0].len(), 2, "两词关键字 → 两个 token");
    let parser = gen_mem_parser(&[t0, t1], &kws, true)
        .to_string()
        .replace(' ', "");
    assert!(parser.contains("fn__mem_try0"), "{parser}");
    assert!(parser.contains("fn__mem_try1"), "{parser}");
    assert!(parser.contains("fn__eat_size"), "{parser}");
    assert!(
        parser.contains("__eat_lit_ci"),
        "大小写豁免应走 __eat_lit_ci：{parser}"
    );

    // 生效列表：未声明 → 单条缺省模板。
    assert_eq!(effective_templates(&None).len(), 1);
    assert_eq!(
        effective_templates(&None)[0],
        super::codegen::mem::DEFAULT_MEM_TEMPLATE
    );
}

/// 立即数槽的 `wrap`（v20 V10）：字面量按 **W 位位模式**读——两种读数都收，规范化到
/// `signed` 的读数；解码/渲染/边界仍用规范值域。
#[test]
fn imm_slot_wrap_reads_the_bit_pattern() {
    use super::model::{OperandKind, OperandSlot};
    let slot = |kind, signed, width, wrap| OperandSlot {
        name: "imm".into(),
        kind,
        class: None,
        classes: None,
        zr31: None,
        byte_reg: None,
        width,
        signed,
        float: None,
        min: None,
        max: None,
        wrap,
        unit: None,
        roles: None,
        encode: None,
        fields: None,
        table: None,
        names: None,
        table_entries: Vec::new(),
        name_entries: Vec::new(),
        symbols: None,
        require_symbol: None,
        imm_fns: None,
        arrangement: None,
    };

    // 32 位有符号 + wrap：规范域是 i32，接受域多收 `0x90909090` 这类无符号写法。
    let s = slot(OperandKind::Imm, Some(true), Some(32), Some(true));
    assert_eq!(s.imm_range(), Some((-2147483648, 2147483647)));
    assert_eq!(s.imm_accept_range(), Some((-2147483648, 4294967295)));
    assert_eq!(s.imm_wrap_modulus(), Some(4294967296));

    // 无符号 + wrap：反方向——`add al, -1` 这类负数写法按 2^8 回绕成 255。
    let u = slot(OperandKind::Imm, None, Some(8), Some(true));
    assert_eq!(u.imm_range(), Some((0, 255)));
    assert_eq!(u.imm_accept_range(), Some((-128, 255)));

    // 未开 wrap ⇒ 接受域 = 规范域（现有谱逐字节不变）；宽度 ≥ 64 无从回绕。
    let plain = slot(OperandKind::Imm, Some(true), Some(32), None);
    assert_eq!(plain.imm_accept_range(), plain.imm_range());
    assert_eq!(plain.imm_wrap_modulus(), None);
    let w64 = slot(OperandKind::Imm, None, Some(64), Some(true));
    assert_eq!(w64.imm_wrap_modulus(), None);
    assert_eq!(w64.imm_accept_range(), w64.imm_range());
}

/// `wrap` 的声明侧校验：只对 imm 槽有意义，且宽度必须 < 64。
#[test]
fn imm_slot_wrap_validation() {
    let base = r#"
[meta]
name = "t"
[encoding]
kind = "fixed"
bits = 32
[reg.gpr32]
count = 8
[operand.imm12]
kind = "imm"
signed = true
bits = 12
literal = "bits"
[operand.r]
kind = "reg"
class = "gpr32"
[[forms]]
name = "I"
fields = [
  "u7[6:0]:opcode",
  "u3[9:7]:rd",
  "u12[21:10]:imm",
]
[[instructions]]
name = "ADDI"
form = "I"
opcode = 0x13
ops = ["dst:r:out", "imm:imm12"]
asm = "addi {dst}, {imm}"
"#;
    parse_and_validate(base).expect("12 位有符号 + wrap 合法");
    // wrap 用在 reg 槽 → 报错
    let bad = base.replace("\nliteral = \"bits\"", "").replace(
        "[operand.r]\nkind = \"reg\"\nclass = \"gpr32\"",
        "[operand.r]\nkind = \"reg\"\nclass = \"gpr32\"\nliteral = \"bits\"",
    );
    let err = parse_and_validate(&bad).unwrap_err().to_string();
    assert!(err.contains("literal"), "err: {err}");
    // literal + bits 64 → 报错
    let bad = base.replace("bits = 12", "bits = 64");
    let err = parse_and_validate(&bad).unwrap_err().to_string();
    assert!(err.contains("bits < 64"), "err: {err}");
}

// ─────────────── `unit`：源值单位（分支偏移是字节） ───────────────

/// `unit`（源值单位）：源文本与 `Inst` 字段是**字节**偏移，字段数的是"几个单位"。
///
/// 本例 = A64 分支的形状（`imm26` × 4 字节字）：值域按源单位放大、非整数倍不匹配、
/// 解码乘回来（`Inst` 与 `disassemble` 始终是源单位）、越界报错。
#[test]
fn imm_slot_unit_scales_the_field() {
    let doc = r#"
[meta]
name = "t"
[encoding]
kind = "fixed"
bits = 32
[reg.gpr32]
count = 8
[operand.off26]
kind = "label"
signed = true
bits = 26
unit = 4
[operand.imm12]
kind = "imm"
signed = true
bits = 12
unit = 1
[[forms]]
name = "BR"
fields = [
  "u6[5:0]:opcode",
  "u26[31:6]:imm26",
]
[[instructions]]
name = "B"
form = "BR"
opcode = 0x05
ops = ["target:off26"]
asm = "b {target}"
"#;
    let m = parse_and_validate(doc).expect("unit 合法");
    let off = m
        .operand_slots
        .iter()
        .find(|s| s.name == "off26")
        .expect("槽存在");
    assert_eq!(off.unit(), 4);
    assert_eq!(off.unit_shift(), 2);
    // 值域放大到源单位：26 位有符号字段 ⇒ ±2^25 个字段单位，×4 = ±2^27 字节。
    assert_eq!(
        off.imm_range(),
        Some((-134_217_728, 134_217_724)),
        "值域必须是源单位（字段值 × unit），且上界按字段上下取整"
    );
    // 非 imm/label 槽恒为 1（`imm12` 没写 unit）。
    let imm12 = m
        .operand_slots
        .iter()
        .find(|s| s.name == "imm12")
        .expect("槽存在");
    assert_eq!((imm12.unit(), imm12.unit_shift()), (1, 0));

    // 声明侧校验：reg 槽写 unit → 报错。
    let bad = doc.replace("kind = \"label\"", "kind = \"reg\"\nclass = \"gpr32\"");
    let err = parse_and_validate(&bad).unwrap_err().to_string();
    assert!(err.contains("unit"), "err: {err}");
    // 非 2 的幂 → 报错（编码期按移位换算）。
    let bad = doc.replace("unit = 4", "unit = 3");
    let err = parse_and_validate(&bad).unwrap_err().to_string();
    assert!(err.contains("2 的幂"), "err: {err}");
    // 变长（prefix_scan）ISA 还没实现这条换算 → 明确拒绝，不静默当 1。
    let bad = doc.replace(
        "kind = \"fixed\"\nbits = 32",
        "kind = \"prefix_scan\"\nmax_len = 8",
    );
    let err = parse_and_validate(&bad).unwrap_err().to_string();
    assert!(err.contains("prefix_scan"), "err: {err}");
}

// ─────────────── `encode`/`fields`：一个值摊到多个字段（位切片） ───────────────

/// 多字段落点（`encode = "slice"`）：值的**低位段**按 `fields` 声明序切给各位域，
/// 每段长度 = 该位域宽度（校验期要求 Σ宽度 == 槽宽）。
///
/// 形状取自 A64 `tbz/tbnz`：6 位位序号 = `b40`[23:19]（低 5 位）+ `b5`[31]（第 5 位）——
/// 两个位域**不相邻**，正是"一个值 → 多字段"的用武之地。
#[test]
fn slot_fields_slice_splits_the_value() {
    let doc = r#"
[meta]
name = "t"
[encoding]
kind = "fixed"
bits = 32
[reg.gpr32]
count = 8
[operand.bitpos]
kind = "imm"
signed = false
bits = 6
encode = "slice"
fields = ["b40", "b5"]
[operand.r]
kind = "reg"
class = "gpr32"
[operand.off14]
kind = "label"
signed = true
bits = 14
[[forms]]
name = "TBZ"
fields = [
  "u7[30:24]:op7",
  "u5[4:0]:rt",
  "u5[23:19]:b40",
  "u14[18:5]:imm14",
  "u1[31]:b5",
]
[[instructions]]
name = "TBZ"
form = "TBZ"
opcode = 0x36
ops = ["src:r", "bit:bitpos", "target:off14"]
asm = "tbz {src}, #{bit}, {target}"
"#;
    let m = parse_and_validate(doc).expect("slice 合法");
    let bit = m
        .operand_slots
        .iter()
        .find(|s| s.name == "bitpos")
        .expect("槽存在");
    assert_eq!(bit.encode, Some(super::model::SlotEncode::Slice));
    assert_eq!(
        bit.fields.as_deref(),
        Some(["b40".to_string(), "b5".to_string()].as_slice())
    );
    // 槽宽 6 = 5 + 1（各字段宽度之和）——值域就是位序号 0..63。
    assert_eq!(bit.imm_range(), Some((0, 63)));

    // 校验：`encode` 与 `fields` 必须成对；字段必须已声明；Σ宽度必须等于槽宽。
    let err = parse_and_validate(&doc.replace("fields = [\"b40\", \"b5\"]\n", ""))
        .unwrap_err()
        .to_string();
    assert!(err.contains("encode"), "err: {err}");
    let err = parse_and_validate(&doc.replace("encode = \"slice\"\n", ""))
        .unwrap_err()
        .to_string();
    assert!(err.contains("fields"), "err: {err}");
    let err = parse_and_validate(&doc.replace("\"b5\"]", "\"b7\"]"))
        .unwrap_err()
        .to_string();
    assert!(err.contains("b7"), "err: {err}");
    let err = parse_and_validate(&doc.replace("bits = 6\nencode", "bits = 7\nencode"))
        .unwrap_err()
        .to_string();
    assert!(err.contains("宽度之和"), "err: {err}");
    // v21 W2：`operand_fields` 由 `fields` + `ops` 派生——多字段落点的**每个分片**都要在
    // form 的 `fields` 里声明；少一个就报"不是声明的字段"。
    let err = parse_and_validate(&doc.replace("  \"u5[23:19]:b40\",\n", ""))
        .unwrap_err()
        .to_string();
    assert!(err.contains("b40"), "err: {err}");
    // 变长 ISA 还没实现多字段落点 → 明确拒绝。
    let err = parse_and_validate(&doc.replace(
        "kind = \"fixed\"\nbits = 32",
        "kind = \"prefix_scan\"\nmax_len = 8",
    ))
    .unwrap_err()
    .to_string();
    assert!(err.contains("prefix_scan"), "err: {err}");
}

// ─────────────── `encode = "logical_imm"`：值 → N/immr/imms 三分量 ───────────────

/// 逻辑立即数方案的**声明侧契约**：三个位域（宽度依序 1/6/6）、槽宽 = 元素宽度（32|64）。
///
/// 算法本身（值 ↔ 分量）由生成物里的 `__encode_logical_imm`/`__decode_logical_imm` 实现；
/// 端到端由 `isa/arm64.toml` 的谱内向量与语料字节对拍守（上游注释里的
/// `and w0, w0, #1`、`and sp, x5, #~15`、`eor x1, x2, #0x8000` 逐字节相同）。
#[test]
fn slot_fields_logical_imm_contract() {
    let doc = r#"
[meta]
name = "t"
[encoding]
kind = "fixed"
bits = 32
[reg.gpr32]
count = 8
[operand.logicimm]
kind = "imm"
signed = false
bits = 32
encode = "logical_imm"
fields = ["nbit", "immr", "imms"]
[operand.r]
kind = "reg"
class = "gpr32"
[[forms]]
name = "LOGIMM"
fields = [
  "u9[31:23]:op9",
  "u5[4:0]:rd",
  "u5[9:5]:rn",
  "u1[22]:nbit",
  "u6[21:16]:immr",
  "u6[15:10]:imms",
]
[[instructions]]
name = "ANDI"
form = "LOGIMM"
opcode = 0x24
ops = ["dst:r:out", "src:r", "imm:logicimm"]
asm = "and {dst}, {src}, #{imm}"
"#;
    let m = parse_and_validate(doc).expect("logical_imm 合法");
    let imm = m
        .operand_slots
        .iter()
        .find(|s| s.name == "logicimm")
        .expect("槽存在");
    let enc = imm.encode.expect("方案");
    assert_eq!(enc, super::model::SlotEncode::LogicalImm);
    // 可编码集不是区间 ⇒ 不生成 lo/hi 边界风味；代表值由方案给（最低位单个 1 恒合法）。
    assert!(!enc.wants_boundary_flavors());
    assert_eq!(enc.sample_value(), 1);

    // 校验：字段个数/宽度不对 → 报错；槽宽不是 32/64 → 报错。
    let bad = doc.replace(
        "fields = [\"nbit\", \"immr\", \"imms\"]",
        "fields = [\"nbit\", \"immr\"]",
    );
    let err = parse_and_validate(&bad).unwrap_err().to_string();
    assert!(err.contains("1/6/6"), "err: {err}");
    let bad = doc.replace("bits = 32\nencode", "bits = 16\nencode");
    let err = parse_and_validate(&bad).unwrap_err().to_string();
    assert!(err.contains("32 或 64"), "err: {err}");
}

// ─────────────── `kind = "bits"`：命名位集合（字母拼接） ───────────────

/// 命名位集合：源文本是表里若干名字的**拼接**（`iorw` = i|o|r|w），编码取位或。
///
/// 表是**数据**（`[conventions.bitsets.<table>]`），DSL 不认识任何名字；`kind = "bits"` 的槽
/// 必须给 `table`（解析期摊平进 `slot.table_entries`，反汇编按名字字典序拼回）。
#[test]
fn slot_kind_bits_named_sets() {
    let doc = r#"
[meta]
name = "t"
[encoding]
kind = "fixed"
bits = 32
[reg.gpr32]
count = 8
[conventions.bitsets.fence]
i = 8
o = 4
r = 2
w = 1
[operand.fset]
kind = "bits"
enum = "fence"
bits = 4
[[forms]]
name = "FENCE"
fields = [
  "u7[6:0]:opcode",
  "u4[27:24]:pred",
  "u4[23:20]:succ",
]
[[instructions]]
name = "FENCE"
form = "FENCE"
opcode = 0x0F
ops = ["pred:fset", "succ:fset"]
asm = "fence {pred}, {succ}"
"#;
    let m = parse_and_validate(doc).expect("bits 槽合法");
    let s = m
        .operand_slots
        .iter()
        .find(|s| s.name == "fset")
        .expect("槽存在");
    // 解析期摊平：名字 → 位，按名字字典序（渲染口径）。
    assert_eq!(
        s.table_entries,
        vec![
            ("i".to_string(), 8),
            ("o".to_string(), 4),
            ("r".to_string(), 2),
            ("w".to_string(), 1),
        ]
    );
    assert_eq!(s.imm_range(), None, "位集合槽不是立即数槽（没有区间）");

    // 校验：缺 table / 表未声明 / 位超宽 都要报错。
    let err = parse_and_validate(&doc.replace("enum = \"fence\"\n", ""))
        .unwrap_err()
        .to_string();
    assert!(err.contains("table"), "err: {err}");
    let err = parse_and_validate(&doc.replace("enum = \"fence\"", "enum = \"nope\""))
        .unwrap_err()
        .to_string();
    assert!(err.contains("nope"), "err: {err}");
    let err = parse_and_validate(&doc.replace("i = 8", "i = 16"))
        .unwrap_err()
        .to_string();
    assert!(err.contains("超出") || err.contains("16"), "err: {err}");
    // `table` 只对 bits 槽有意义：写到 imm 槽上要报错。
    let err = parse_and_validate(&doc.replace("kind = \"bits\"", "kind = \"imm\"\nsigned = false"))
        .unwrap_err()
        .to_string();
    assert!(err.contains("bits"), "err: {err}");
}

// ─────────────── `kind = "imm"` + `names`：命名立即数（一个名字 = 一个值） ───────────────

/// 命名立即数：与 `kind = "bits"`（名字**拼接**、按位或）相反——**一个名字 = 一个值**
/// （RISC-V 的 CSR 名 `mstatus` = 0x300、浮点舍入模式 `rtz` = 1）。
///
/// 表是**数据**（`[conventions.imm_names.<table>]`），DSL 不认识任何名字；解析期摊平进
/// `slot.name_entries`（按名字字典序，渲染取**同值多名**里字典序最小者）。端到端由
/// riscv64 语料守：9 个 CSR 名字文件（`machine-csr-names.s` 等）整文件解析通过，
/// 而语料自带的上游期望字节保证值没错。
#[test]
fn slot_names_named_immediates() {
    let doc = r#"
[meta]
name = "t"
[encoding]
kind = "fixed"
bits = 32
[reg.gpr32]
count = 8
[conventions.imm_names.csr]
# 故意乱序：摊平后必须按**名字字典序**（渲染口径）。
mstatus = 0x300
fflags = 0x001
stval = 0x143
sbadaddr = 0x143
[operand.r]
kind = "reg"
class = "gpr32"
[operand.csr12]
kind = "imm"
signed = false
bits = 12
enum = "csr"
[[forms]]
name = "I"
fields = [
  "u7[6:0]:opcode",
  "u5[11:7]:rd",
  "u12[31:20]:csr",
]
[[instructions]]
name = "CSRRS"
form = "I"
opcode = 0x73
ops = ["dst:r:out", "csr:csr12"]
asm = "csrrs {dst}, {csr}"
"#;
    let m = parse_and_validate(doc).expect("names 槽合法");
    let s = m
        .operand_slots
        .iter()
        .find(|s| s.name == "csr12")
        .expect("槽存在");
    assert_eq!(
        s.name_entries,
        vec![
            ("fflags".to_string(), 0x001),
            ("mstatus".to_string(), 0x300),
            ("sbadaddr".to_string(), 0x143),
            ("stval".to_string(), 0x143),
        ],
        "摊平按名字字典序（同值多名时渲染取字典序最小者）"
    );
    assert_eq!(s.imm_range(), Some((0, 0xFFF)), "命名表不改变槽的值域");

    // 表未声明。
    let err = parse_and_validate(&doc.replace("enum = \"csr\"", "enum = \"nope\""))
        .unwrap_err()
        .to_string();
    assert!(err.contains("nope"), "err: {err}");
    // 值超出本槽的接受值域（12 位无符号 ⇒ 0..4095）。
    let err = parse_and_validate(&doc.replace("mstatus = 0x300", "mstatus = 0x1000"))
        .unwrap_err()
        .to_string();
    assert!(err.contains("mstatus"), "err: {err}");
    // 名字不是 ident。
    let err = parse_and_validate(&doc.replace("sbadaddr = 0x143", "\"sb-ad\" = 0x143"))
        .unwrap_err()
        .to_string();
    assert!(err.contains("ident"), "err: {err}");
    // `names` 只对 imm 槽有意义。
    let err = parse_and_validate(&doc.replace(
        "[operand.csr12]\nkind = \"imm\"\nsigned = false\nbits = 12\nenum = \"csr\"",
        "[operand.csr12]\nkind = \"reg\"\nclass = \"gpr32\"\nenum = \"csr\"",
    ))
    .unwrap_err()
    .to_string();
    assert!(err.contains("imm"), "err: {err}");
}

// ─────────────── `symbols`：立即数位置上的符号引用 ───────────────

/// `[[operand_slots]].symbols`：声明后 imm/label 槽的值可以是**符号引用**（`%hi(foo)`、
/// 裸 `foo`）——解析期当 0 参与算术、两遍回填时解析成"标签的块下标"并**过一遍立即数修饰**；
/// 未声明的槽照旧"未知 ident 即不匹配"（**能力是声明的**）。
///
/// 端到端由 riscv64 语料守：`%hi(foo)`/`%pcrel_hi(foo)` 这类**未定义**符号 → `UndefinedLabel`
/// （准确诊断，不再是"没有这条指令"），而 `.Lpcrel_hi0` 这类**已定义**的局部标签 → 真装配；
/// 顺带钉住"点开头的局部标签名是一个 ident"（`__lex` 的 `is_ident_start`）。
#[test]
fn slot_symbols_capability_contract() {
    let doc = r#"
[meta]
name = "t"
[encoding]
kind = "fixed"
bits = 32
[reg.gpr32]
count = 8
[operand.sym20]
kind = "imm"
signed = false
bits = 20
symbol = { allow = true }
[operand.off12]
kind = "imm"
signed = true
bits = 12
unit = 4
[operand.r]
kind = "reg"
class = "gpr32"
[[forms]]
name = "U"
fields = [
  "u7[6:0]:opcode",
  "u5[11:7]:rd",
  "u20[31:12]:imm",
]
[[instructions]]
name = "LUI"
form = "U"
opcode = 0x37
ops = ["dst:r:out", "imm:sym20"]
asm = "lui {dst}, {imm}"
"#;
    let m = parse_and_validate(doc).expect("symbols 合法");
    let s = m
        .operand_slots
        .iter()
        .find(|s| s.name == "sym20")
        .expect("槽存在");
    assert_eq!(s.symbols, Some(true));

    // 只对 imm/label 有意义：reg 槽写它 → 报错。
    let bad = doc.replace(
        "[operand.r]\nkind = \"reg\"\nclass = \"gpr32\"",
        "[operand.r]\nkind = \"reg\"\nclass = \"gpr32\"\nsymbol = { allow = true }",
    );
    let err = parse_and_validate(&bad).unwrap_err().to_string();
    assert!(err.contains("symbol"), "err: {err}");
    // `unit != 1` 的槽写它 → 直接拒（"修饰作用于块下标还是字节偏移"有歧义）。
    let bad = doc.replace(
        "bits = 12\nunit = 4",
        "bits = 12\nunit = 4\nsymbol = { allow = true }",
    );
    let err = parse_and_validate(&bad).unwrap_err().to_string();
    assert!(err.contains("unit == 1"), "err: {err}");
}

// ─────────────── W1：栈参数必须由角色（标签）驱动，不按指令名兜底 ───────────────

/// 栈参数的**能力申报**是角色（v20 A5-3 起谱面没有 `[abi.stack_args]` 键可写了）：
///
/// ① 有 `stack_arg_load` 而缺 `stack_arg_store` ⇒ **生成期**点名角色的明确错误
///    （把栈参数写进 spill 槽需要它），绝不用 x86 指令名兜底；
/// ② 连 `stack_arg_load` 都没有 ⇒ 这个 ISA 只是**不支持栈参数**，生成照旧成功
///    （收参路径在运行期给 Unsupported，而不是生成期报错）。
///
/// 夹具：直接拿真实 `isa/x86.toml` 做字符串手术删掉那两条 `roles = [...]`，
/// 其余保持原样——测的是**发货 ISA** 的真实生成路径，而不是人造小模型。
#[test]
fn stack_args_capability_is_declared_by_roles() {
    let src = include_str!("../../../../../isa/x86.toml");
    // 正例：原样 → 全量生成成功（x86 声明了两个角色）。
    let m = parse_and_validate(src).expect("x86 doc parses");
    crate::dsl::codegen::generate(&m).expect("x86 有 stack_arg_* 角色 → 生成必须成功");

    // ① 有 load、缺 store → 生成期报错并点名角色。
    let no_store = src.replace("roles = [\"stack_arg_store\"]", "");
    assert_ne!(
        no_store, src,
        "夹具失效：x86 TOML 应含 stack_arg_store 角色"
    );
    // 缺口可能在 validate 或 codegen 暴露，两处都算合格——但必须**点名角色**。
    let err = match parse_and_validate(&no_store) {
        Ok(m2) => crate::dsl::codegen::generate(&m2)
            .expect_err("有 stack_arg_load 却缺 stack_arg_store → 必须生成期报错")
            .to_string(),
        Err(e) => e.to_string(),
    };
    eprintln!("[W1] 缺 stack_arg_store → {err}");
    assert!(
        err.contains("stack_arg_store"),
        "错误信息必须点名缺失的角色 stack_arg_store（而不是回退到某个 ISA 的指令名）：{err}"
    );

    // ② 连 load 都没有 → 能力降级（不支持栈参数），生成照旧成功。
    let no_load = src.replace("roles = [\"stack_arg_load\"]", "");
    assert_ne!(no_load, src, "夹具失效：x86 TOML 应含 stack_arg_load 角色");
    let m3 = parse_and_validate(&no_load).expect("删掉 load 角色后仍应可解析");
    crate::dsl::codegen::generate(&m3)
        .expect("没有 stack_arg_load 角色 = 本 ISA 不支持栈参数，生成不该失败");
}

// ─────────── P0：宽度/类元数据派生（去「宽度写死」，2026-09-12） ───────────

/// 仅声明 1 字节 GPR 组的 ISA：全部类/宽度必须由元数据派生，
/// 历史实现锚定 `GPR(8).or(GPR(4))` → 名字表为空（静默失效）。
fn one_byte_doc(meta_extra: &str) -> String {
    one_byte_doc_at(meta_extra, "")
}

/// 同上，但额外键进 **`[encoding]`**（`default_opsize` 等编码期键，v18 S4 归位）。
fn one_byte_doc_enc(enc_extra: &str) -> String {
    one_byte_doc_at("", enc_extra)
}

fn one_byte_doc_at(meta_extra: &str, enc_extra: &str) -> String {
    format!(
        r#"
[meta]
name = "tiny8"
{meta_extra}
[encoding]
kind = "fixed"
bits = 32
{enc_extra}
[reg.gpr8]
names = ["A0", "A1", "A2", "A3"]
[operand.a8]
kind = "reg"
class = "gpr8"
roles = "inout"
[[instructions]]
name = "MOV8"
form = "RR"
opcode = 1
ops = ["dst:a8:out", "src:a8"]
asm = "mov {{dst}}, {{src}}"
[[forms]]
name = "RR"
fields = [
  "u8[7:0]:opcode",
  "u3[10:8]:rd",
  "u3[13:11]:rs1",
]
"#
    )
}

/// 1 字节寄存器 ISA：`main_gpr_class`/`addr_class`/`slot_bytes`/`fp_overhead_bytes`
/// 全部 = 1 字节；名字表可解析（不再为空）。
#[test]
fn width_metadata_one_byte_gpr_is_derived() {
    let m = parse_and_validate(&one_byte_doc("")).expect("1 字节寄存器 ISA 必须合法");
    assert_eq!(m.main_gpr_class().unwrap(), RegClass::GPR(8));
    assert_eq!(m.addr_class().unwrap(), RegClass::GPR(8));
    assert_eq!(m.value_gpr_class().unwrap(), RegClass::GPR(8));
    assert_eq!(m.slot_bytes().unwrap(), 1, "栈槽单位 = 地址宽（1 字节）");
    assert_eq!(m.fp_overhead_bytes().unwrap(), 1);
    assert_eq!(m.main_fpr_class().unwrap(), None, "无 FPR 组");
    assert_eq!(m.value_fpr_class().unwrap(), None, "无 fpr64 组");
    let idx = m.main_gpr_name_to_idx().expect("名字表必须解析成功");
    assert_eq!(idx.get("A0"), Some(&0));
    assert_eq!(idx.get("A3"), Some(&3));
    assert_eq!(idx.len(), 4);
    // 索引含 base_index（历史实现只按组内序号，base_index≠0 的组会错位）。
    let m2 = parse_and_validate(&one_byte_doc("")).unwrap();
    assert!(
        m2.names_of(RegClass::GPR(8))
            .unwrap()
            .contains(&"A2".into())
    );
}

/// 主 GPR 类 = 已声明 GPR 组中最宽者（x86 四视图 → 8）。
#[test]
fn width_metadata_main_gpr_is_widest_group() {
    let doc = r#"
[meta]
name = "w"
[reg.gpr8]
count = 4
[reg.gpr16]
base_index = 0
count = 4
[reg.gpr64]
base_index = 0
count = 4
[operand.g]
kind = "reg"
class = "gpr64"
roles = "inout"
"#;
    let m = parse_and_validate(doc).expect("合法");
    assert_eq!(m.main_gpr_class().unwrap(), RegClass::GPR(64));
    assert_eq!(m.addr_class().unwrap(), RegClass::GPR(64));
    assert_eq!(m.slot_bytes().unwrap(), 8);
}



/// 主 FPR 类保留历史规则：**优先 16 字节组**（XMM 基准），而非"最宽"
/// （x86 最宽是 32 字节 ZMM——若按最宽推导，SSE/ABI 占位会变成 ZMM 视图）。
#[test]
fn width_metadata_main_fpr_prefers_xmm16() {
    let doc = one_byte_doc("").replace(
        "[reg.gpr8]",
        "[reg.fpr32]\ncount = 8\n[reg.fpr128]\ncount = 16\n[reg.fpr256]\ncount = 32\n[reg.gpr8]",
    );
    let m = parse_and_validate(&doc).expect("合法");
    assert_eq!(m.main_fpr_class().unwrap(), Some(RegClass::FPR(128)));
    // 显式键优先。
    let forced = parse_and_validate(&format!(
        "{}\n[machine]\nfpr = \"fpr32\"\n",
        one_byte_doc("").replace(
            "[reg.gpr8]",
            "[reg.fpr32]\ncount = 8\n[reg.fpr128]\ncount = 16\n[reg.gpr8]",
        )
    ))
    .expect("合法");
    assert_eq!(forced.main_fpr_class().unwrap(), Some(RegClass::FPR(32)));
}

/// 没有任何 GPR 组 → 派生失败（历史实现静默回退 `GPR(8)`/`GPR(4)` + 空名字表）。
#[test]
fn width_metadata_missing_gpr_group_is_error() {
    let doc = r#"
[meta]
name = "floatonly"
[reg.fpr32]
count = 8
[operand.f]
kind = "reg"
class = "fpr32"
roles = "inout"
"#;
    let msg = validation_msg(doc);
    assert!(msg.contains("GPR"), "msg: {msg}");
}

/// 显式类指针键必须指向已声明组（不允许"声明一个不存在的类"）。
#[test]
fn width_metadata_explicit_key_needs_group() {
    let msg = validation_msg(&format!("{}\n[machine]\ngpr = \"gpr128\"\n", one_byte_doc("")));
    assert!(msg.contains("[machine].gpr"), "msg: {msg}");
    let msg = validation_msg(&format!("{}\n[machine]\naddr = \"gpr16\"\n", one_byte_doc("")));
    assert!(msg.contains("[machine].addr"), "msg: {msg}");
    // `[stack]` 键值域（2026-09-13 从 [meta]/[abi] 归并而来）
    let msg =
        validation_msg(&one_byte_doc("").replace("[reg.gpr8]", "[stack]\nslot = 0\n\n[reg.gpr8]"));
    assert!(msg.contains("slot"), "msg: {msg}");
}

/// `[encoding].default_opsize`（位）必须与某个已声明 GPR 组一致
/// （生成代码里的 `__opsize` 是字节，1 字节 ISA 需显式声明 8）。
#[test]
fn width_metadata_default_opsize_needs_matching_group() {
    let m = parse_and_validate(&one_byte_doc_enc("default_opsize = 8")).expect("合法");
    assert_eq!(m.encoding.default_opsize, Some(8));
    let msg = validation_msg(&one_byte_doc_enc("default_opsize = 8").replace("gpr8", "gpr64"));
    // 注意：替换后 class = "gpr64" 与组一致，但 default_opsize=8 找不到 gpr8 → 报错。
    assert!(
        msg.contains("default_opsize"),
        "1 字节 opsize 必须要求 [reg.gpr8]：{msg}"
    );
    let msg = validation_msg(&one_byte_doc_enc("default_opsize = 12"));
    assert!(msg.contains("8 的倍数"), "msg: {msg}");
}

// ─────────── 残余收敛（2026-09-13）：冲突/静默丢弃类缺口逐条 fail-closed ───────────

/// 两宽度视图的最小 ISA（主 GPR = gpr64；`gpr32` 用来制造"声明在别组"的名字），
/// 可注入 `[abi]`/`[[instructions]]`/`[spill.*]` 片段。
fn two_view_doc(extra: &str) -> String {
    format!(
        r#"
[meta]
name = "twoview"
[encoding]
kind = "fixed"
bits = 32
[reg.gpr64]
names = ["R0", "R1", "R2", "R3"]
[reg.gpr32]
base_index = 0
names = ["E0", "E1", "E2", "E3"]
[reg.fpr128]
base_index = 0
names = ["F0", "F1"]
[operand.g]
kind = "reg"
class = "gpr64"
roles = "inout"
[[instructions]]
name = "NOP"
form = "W"
opcode = 0
# 序言/尾声由生成器生成（v20 A4）：作为可编译目标，谱必须申报返回能力。
roles = ["ret"]
asm = "nop"
[[instructions]]
name = "MOV"
data_width = 64
form = "RR"
opcode = 16
ops = ["dst:g:out", "src:g"]
asm = "mov {{dst}}, {{src}}"
[[forms]]
name = "RR"
fields = [
  "u8[7:0]:opcode",
  "u3[10:8]:rd",
  "u3[13:11]:rs1",
]
[[forms]]
name = "W"
fields = [
  "u8[7:0]:opcode",
  "u3[10:8]:rd",
  "u3[13:11]:rs1",
]
{extra}
"#
    )
}

/// 生成期错误（`generate` 阶段）——与 `validation_msg` 分开，因为 R2/R3 属于
/// codegen 而非 validate。
fn codegen_msg(doc: &str) -> String {
    match parse_and_validate(doc) {
        Ok(m) => crate::dsl::codegen::generate(&m)
            .err()
            .unwrap_or_else(|| panic!("预期生成期报错，实际生成成功")),
        Err(e) => panic!("预期生成期报错，解析/校验先失败：{e}"),
    }
}

/// 校验期**或**生成期报错都算合格（缺口可能被任一层拦住）——与
/// `stack_args_requires_role_tags` 的既有写法一致。
fn any_stage_msg(doc: &str) -> String {
    match parse_and_validate(doc) {
        Ok(m) => crate::dsl::codegen::generate(&m)
            .err()
            .unwrap_or_else(|| panic!("预期报错（校验或生成期），实际两阶段都成功")),
        Err(e) => e.to_string(),
    }
}

/// R3：`[machine.frame].sp` 声明了却不在**主 GPR 组**内 → 生成期报错，
/// **不得**被惯例名（RSP/SP）顶替。
#[test]
fn frame_sp_declared_outside_main_group_is_error() {
    let msg = codegen_msg(&two_view_doc(
        r#"
[machine.frame]
sp = "E0"
fp = "E1"
"#,
    ));
    assert!(msg.contains("sp"), "必须点名 [machine.frame].sp：{msg}");
    assert!(msg.contains("E0"), "必须点名冲突的名字：{msg}");
    assert!(
        !msg.contains("已解析为") && msg.contains("生成期 fail-closed"),
        "必须是 fail-closed 而非静默替换：{msg}"
    );
}

/// R3 反例：`sp` 落在主 GPR 组内 → 正常解析（不报错）。
#[test]
fn frame_sp_inside_main_group_resolves() {
    let m = parse_and_validate(&two_view_doc(
        r#"
[machine.frame]
sp = "R3"
fp = "R2"
"#,
    ))
    .expect("主组内的 sp/fp 合法");
    crate::dsl::codegen::generate(&m).expect("生成成功");
}

/// R2：`[[instructions]].implicit_regs` 里的名字解析不到 → 校验期/生成期报错
/// （历史实现 `filter_map` 静默丢弃 ⇒ clobber 集缺失）。
#[test]
fn implicit_regs_unknown_name_is_error() {
    let msg = any_stage_msg(&two_view_doc(
        r#"
[[instructions]]
name = "CQO"
form = "W"
opcode = 1
implicit_regs = ["NOPE"]
asm = "cqo"
"#,
    ));
    assert!(msg.contains("implicit_regs"), "msg: {msg}");
    assert!(msg.contains("NOPE"), "必须点名未知寄存器：{msg}");
    assert!(msg.contains("CQO"), "必须点名指令：{msg}");
}

/// R2 反例：`implicit_regs` 用主组内的名字 → 正常生成。
#[test]
fn implicit_regs_known_name_generates() {
    let m = parse_and_validate(&two_view_doc(
        r#"
[[instructions]]
name = "CQO"
form = "W"
opcode = 1
implicit_regs = ["R1"]
asm = "cqo"
"#,
    ))
    .expect("合法");
    crate::dsl::codegen::generate(&m).expect("生成成功");
}

/// R5：比浮点值池更宽的浮点/向量类必须有 `[spill.FPR<bytes>]` 档位——
/// 缺失在校验期点名（历史实现要等 IR 编译期才报 Unsupported）。
#[test]
fn wide_fpr_class_requires_spill_tier() {
    // fpr128（16 字节 > 缺省标量 8）但没有 [spill.FPR16]
    let msg = validation_msg(&two_view_doc(
        r#"
[spill.GPR]
load = "NOP"
store = "NOP"
[spill.FPR]
load = "NOP"
store = "NOP"
"#,
    ));
    assert!(msg.contains("FPR16"), "必须点名缺哪个键：{msg}");
    assert!(msg.contains("reg.fpr128"), "必须点名哪个类：{msg}");
    // 声明档位后合法
    let m = parse_and_validate(&two_view_doc(
        r#"
[spill.GPR]
load = "NOP"
store = "NOP"
[spill.FPR]
load = "NOP"
store = "NOP"
[spill.FPR16]
load = "NOP"
store = "NOP"
"#,
    ))
    .expect("声明档位后合法");
    assert!(m.spill.contains_key("FPR16"));
}

// ─────────── B2：`[types]` 显式类型→类映射（ISA 数据，优先于通用值池规则） ───────────

/// 合法：显式映射到已声明组；`"unsupported"` 也合法；`ptr` 按 ISA 地址宽判定。
#[test]
fn types_explicit_map_parses_and_validates() {
    let doc = format!(
        "{}\n[machine]\naddr = \"gpr32\"\n",
        two_view_doc(
            r#"
[types]
i8 = "gpr64"
f64 = "fpr128"
ptr = "gpr32"
i64 = "unsupported"
"#,
        )
    );
    let m = parse_and_validate(&doc).expect("合法显式映射");
    let map = m.explicit_type_map().unwrap();
    assert!(map.contains(&("i8".to_string(), Some(RegClass::GPR(64)))));
    assert!(map.contains(&("ptr".to_string(), Some(RegClass::GPR(32)))));
    assert!(
        map.contains(&("i64".to_string(), None)),
        "unsupported 记 None"
    );
}

/// 非法：类型名未知 / 目标组未声明 / 类宽 < 类型字节宽（会静默截断）/ void 映射。
#[test]
fn types_explicit_map_rejects_bad_entries() {
    let msg = validation_msg(&two_view_doc("[types]\nfoo = \"gpr64\"\n"));
    assert!(msg.contains("未知类型名"), "msg: {msg}");
    let msg = validation_msg(&two_view_doc("[types]\ni8 = \"gpr256\"\n"));
    assert!(msg.contains("未声明"), "msg: {msg}");
    let msg = validation_msg(&two_view_doc("[types]\ni64 = \"gpr32\"\n"));
    assert!(msg.contains("静默截断"), "msg: {msg}");
    let msg = validation_msg(&two_view_doc("[types]\nvoid = \"gpr64\"\n"));
    assert!(msg.contains("void"), "msg: {msg}");
}

/// `ptr` 的健全性按 **ISA 地址宽**（不是 `TypeId::bits()` 的 8）：
/// 地址宽 4 的 ISA 可以 `ptr = "gpr32"`；地址宽 2 的 ISA 不能 `ptr = "gpr8"`。
#[test]
fn types_ptr_uses_isa_address_width() {
    let doc = format!(
        "{}\n[machine]\naddr = \"gpr32\"\n",
        two_view_doc("[types]\nptr = \"gpr32\"\n")
    );
    parse_and_validate(&doc).expect("地址宽 4 → ptr = gpr32 合法");
    let doc2 = format!(
        "{}\n[machine]\naddr = \"gpr16\"\n",
        two_view_doc("[types]\nptr = \"gpr8\"\n").replace(
            "[reg.gpr64]",
            "[reg.gpr16]\nbase_index = 0\nnames = [\"E0\", \"E1\"]\n\n\
             [reg.gpr8]\nbase_index = 0\nnames = [\"B0\", \"B1\"]\n\n[reg.gpr64]",
        )
    );
    let msg = validation_msg(&doc2);
    assert!(
        msg.contains("静默截断"),
        "addr_width=2 > gpr8 → 报错：{msg}"
    );
}

// ─────────── B6：指令字宽 = ISA 数据（任意 1..=64 位，无白名单） ───────────

/// 定宽 ISA 的指令字宽取自 `[encoding].bits`，字节数 = `ceil(位/8)`。
/// 用"低位 opcode + 补集零 guard"形式——**任何字宽**都成立（含 > 64 位：
/// 单个位域 ≤ 64 位是值表示上限，与字长无关）。
fn word_doc(bits: u32) -> String {
    let op_w = bits.min(8);
    // v21：没有"全字 opcode"后门——必须显式声明字段，且单个位域 ≤ 64 位
    //（值承载在 u64/i64）。超出的高位不声明 = 未指定位（合法，按缺省 0 发射）。
    let fw = bits.clamp(1, 64);
    let fhi = fw - 1;
    format!(
        r#"
[meta]
name = "w{b}"
[encoding]
kind = "fixed"
bits = {bits}
[reg.gpr32]
count = 8
[operand.g]
kind = "reg"
class = "gpr32"
roles = "inout"
[[forms]]
name = "W"
fields = ["u{fw}[{fhi}:0]:opcode"]
[[instructions]]
name = "NOP"
form = "W"
opcode = 0
asm = "nop"
"#,
        fw = fw,
        fhi = fhi,
        bits = bits,
        b = bits
    )
}

/// 字宽**不设白名单/上限**：1 位、12 位（非 8 倍数）、100 位（超机器字）、
/// 4096 位都合法，字节数 = ceil(位/8)；只有 0 位（非宽度）才报错。
#[test]
fn inst_width_accepts_arbitrary_bit_widths() {
    for (bits, bytes) in [
        (1u32, 1u32),
        (8, 1),
        (12, 2),
        (16, 2),
        (24, 3),
        (32, 4),
        (64, 8),
        (65, 9),
        (100, 13),
        (128, 16),
        (1000, 125),
        (4096, 512),
    ] {
        let m = parse_and_validate(&word_doc(bits))
            .unwrap_or_else(|e| panic!("{bits} 位字应合法：{e}"));
        assert_eq!(m.inst_bytes().unwrap(), bytes, "{bits} 位 → {bytes} 字节");
    }
    let msg = validation_msg(&word_doc(0));
    assert!(msg.contains("must be > 0"), "0 位不是宽度：{msg}");
}

/// 单个位域 > 64 位报错（值表示上限：位域值是 u64/i64）——**不是**字长限制：
/// 同一个 100 位字里可以有多个 ≤64 位的域（见 `word_doc` 与夹具 demo_inst100）。
#[test]
fn inst_width_field_over_64_bits_rejected() {
    let doc = r#"
[meta]
name = "wide"
[encoding]
kind = "fixed"
bits = 100
[reg.gpr32]
count = 8
[operand.g]
kind = "reg"
class = "gpr32"
roles = "inout"
[[forms]]
name = "W"
fields = [
  "u8[7:0]:opcode",
  "u65[64:0]:wide",
]
[[instructions]]
name = "NOP"
form = "W"
opcode = 0
asm = "nop"
"#;
    let msg = validation_msg(doc);
    assert!(
        msg.contains("超过值表示上限 64 位"),
        "65 位单个域应被拒绝（值表示上限），实际：{}",
        &msg[..msg.len().min(200)]
    );
}

/// 定宽 label/global fixup 的 `RelocKind::Relative(字长字节数, 0)` 由字宽派生
/// （历史实现写死 4）——12 位字 → `Relative(2, 0)`，64 位字 → `Relative(8, 0)`。
#[test]
fn inst_width_drives_reloc_width() {
    for (bits, reloc) in [(12u32, "Relative (2 , 0)"), (32, "Relative (4 , 0)")] {
        let doc = format!(
            r#"
[meta]
name = "br{b}"
[encoding]
kind = "fixed"
bits = {bits}
[reg.gpr32]
count = 8
[operand.g]
kind = "reg"
class = "gpr32"
roles = "inout"
[operand.l]
kind = "label"
signed = true
bits = 4
[[forms]]
name = "B"
fields = [
  "u4[3:0]:opcode",
  "u3[6:4]:rs1",
  "u4[11:8]:lab",
]
[[instructions]]
name = "BRZ"
form = "B"
opcode = 1
effect = ["Branch"]
ops = ["src:g", "target:l"]
asm = "brz {{src}}, {{target}}"
"#,
            bits = bits,
            b = bits
        );
        let m = parse_and_validate(&doc).unwrap_or_else(|e| panic!("{bits} 位字应合法：{e}"));
        let ts = crate::dsl::codegen::generate(&m)
            .unwrap_or_else(|e| panic!("{bits} 位字应生成成功：{e}"))
            .to_string();
        assert!(
            ts.contains(reloc),
            "{bits} 位字的 fixup 宽度应为 {reloc}（由字长派生）"
        );
    }
}

/// 位域必须落在指令字内：12 位字里 `offset = 8, width = 4`（最高位 12）合法，
/// 但 `offset = 9, width = 4`（最高位 13）报错——**不静默移位出字**。
#[test]
fn inst_width_rejects_bitfield_beyond_word() {
    // 用 4 位 opcode 位域（可移位到字内任意位置）+ 全字常量指令
    let doc = |field: &str| {
        format!(
            r#"
[meta]
name = "fit"
[encoding]
kind = "fixed"
bits = 12
[reg.gpr32]
count = 8
[operand.g]
kind = "reg"
class = "gpr32"
roles = "inout"
[[forms]]
name = "W"
fields = [
  "{field}",
]
[[instructions]]
name = "NOP"
form = "W"
opcode = 0
asm = "nop"
"#
        )
    };
    parse_and_validate(&doc("u4[11:8]:word")).expect("bits 8..12 落在 12 位字内");
    let msg = validation_msg(&doc("u4[12:9]:word"));
    assert!(
        msg.contains("超出指令字宽"),
        "13 位 > 12 位字宽 → 报错：{msg}"
    );
}

// ─────────── v20 V10：立即数修饰（谱声明的数据） ───────────

/// `[[conventions.imm_fn]]`：**怎么写（text）+ 算成什么值（expr）**两行数据。
/// 形状非法要在声明期报出来（它是汇编期行为的全部依据，写错不该拖到运行期）。
#[test]
fn imm_fn_decl_shape_is_validated() {
    // 合法：带括号的 GAS 风格、不带括号的前缀风格、纯后缀风格都收
    for block in [
        "[[conventions.imm_fn]]\nname = \"hi\"\ntext = \"%hi({0})\"\nexpr = \"({0} + 0x800) >> 12\"",
        "[[conventions.imm_fn]]\nname = \"lower16\"\ntext = \":lower16:{0}\"\nexpr = \"{0} & 0xffff\"",
        "[[conventions.imm_fn]]\nname = \"suffix\"\ntext = \"{0}:lo\"\nexpr = \"{0} & 0xff\"",
    ] {
        parse_and_validate(&lowering_doc(block)).unwrap_or_else(|e| panic!("{block}: {e:?}"));
    }

    // text / expr 各必须恰好一个 `{0}`
    let no_hole =
        lowering_doc("[[conventions.imm_fn]]\nname = \"hi\"\ntext = \"%hi(x)\"\nexpr = \"{0}\"");
    assert!(
        validation_msg(&no_hole).contains("text 必须恰好含一个"),
        "{}",
        validation_msg(&no_hole)
    );
    let two_holes = lowering_doc(
        "[[conventions.imm_fn]]\nname = \"hi\"\ntext = \"%hi({0})-{0}\"\nexpr = \"{0}\"",
    );
    assert!(validation_msg(&two_holes).contains("恰好含一个"));
    let expr_no_hole =
        lowering_doc("[[conventions.imm_fn]]\nname = \"hi\"\ntext = \"%hi({0})\"\nexpr = \"1\"");
    assert!(validation_msg(&expr_no_hole).contains("expr 必须恰好含一个"));

    // text 光一个 `{0}` = 通配（会吞掉任意表达式）⇒ 拒
    let bare =
        lowering_doc("[[conventions.imm_fn]]\nname = \"hi\"\ntext = \"{0}\"\nexpr = \"{0}\"");
    assert!(
        validation_msg(&bare).contains("没有任何字面"),
        "{}",
        validation_msg(&bare)
    );

    // name 重复 / 为空
    let dup = lowering_doc(concat!(
        "[[conventions.imm_fn]]\nname = \"hi\"\ntext = \"%hi({0})\"\nexpr = \"{0}\"\n",
        "[[conventions.imm_fn]]\nname = \"hi\"\ntext = \"%HI({0})\"\nexpr = \"{0}\"",
    ));
    assert!(
        validation_msg(&dup).contains("重复"),
        "{}",
        validation_msg(&dup)
    );
    let empty =
        lowering_doc("[[conventions.imm_fn]]\nname = \"\"\ntext = \"%hi({0})\"\nexpr = \"{0}\"");
    assert!(validation_msg(&empty).contains("name 不能为空"));
}

/// 没声明 `imm_fn` 的 ISA：生成物里**没有**修饰机制（与引入前逐字相同）——
/// 这条纪律与 `[[pseudo]]`/`[[conventions.mem]]` 一致：能力按谱数据出现。
#[test]
fn imm_fn_absent_keeps_generated_code_unchanged() {
    let m = parse_and_validate(&lowering_doc("")).expect("合法");
    let s = super::codegen::generate(&m).unwrap().to_string();
    assert!(!s.contains("__imm_fn_lit"), "不该生成修饰匹配器：{s}");
    assert!(!s.contains("__imm_fn0"), "不该生成修饰值函数：{s}");

    let m2 = parse_and_validate(&lowering_doc(
        "[[conventions.imm_fn]]\nname = \"hi\"\ntext = \"%hi({0})\"\nexpr = \"({0} + 0x800) >> 12\"",
    ))
    .expect("合法");
    let s2 = super::codegen::generate(&m2).unwrap().to_string();
    assert!(
        s2.contains("__imm_fn_lit") && s2.contains("__imm_fn0"),
        "声明了就该有修饰机制：{s2}"
    );
}

/// `PrefixEffect`：谱面拼法 ↔ 枚举变体（**有值变体** `addr<bits>` 也在内）。
///
/// 这条单测盯着三件事：① 每个效果名的谱面写法能往返；② `addr32`/`addr16` 是**同一支
/// 效果的参数**（不是两个变体）；③ 未知/坏拼写在**解析期**就报错——校验期因此不再维护
/// 第二份效果名单（名单只有 `PrefixEffect::NAMES` 一份）。
#[test]
fn prefix_effects_parse_from_their_spec_spellings() {
    use crate::dsl::model::{PrefixEffect as P, PrefixScanEntry};
    for (s, want) in [
        ("opsize16", P::Opsize16),
        ("lock", P::Lock),
        ("repe", P::Repe),
        ("repne", P::Repne),
        ("rex", P::Rex),
        ("rex2", P::Rex2),
        // 有值变体：位宽是参数（模型内部换算成字节，见 `addr_size_override`）。
        ("addr32", P::AddrSize(32)),
        ("addr16", P::AddrSize(16)),
    ] {
        assert_eq!(P::try_from(s).ok(), Some(want), "解析 {s}");
        assert_eq!(want.name(), s, "{s} 的谱面写法应当往返");
    }
    // REX2 是唯一**多吃一个字节**的效果（`0xD5` + payload）——前缀扫描的宽度靠它。
    assert_eq!(P::Rex2.extra_bytes(), 1);
    assert_eq!(P::Rex.extra_bytes(), 0);
    assert_eq!(P::AddrSize(32).extra_bytes(), 0);

    for bad in ["rex3", "addr0", "addr7", "addr12", "OPSIZE16", ""] {
        assert!(P::try_from(bad).is_err(), "{bad:?} 应当被拒");
    }
    // 走一遍反序列化（谱里写错名字 → 解析期报错，不是静默忽略）。
    let ok: PrefixScanEntry = toml::from_str("byte = 0xD5\neffects = [\"rex2\"]").unwrap();
    assert_eq!(ok.effects, vec![P::Rex2]);
    let bad = toml::from_str::<PrefixScanEntry>("byte = 0xD5\neffects = [\"rex3\"]");
    assert!(bad.is_err(), "未知效果名必须在解析期就拒：{bad:?}");
    let msg = format!("{}", bad.unwrap_err());
    assert!(msg.contains("rex3"), "报错要点名那个拼法：{msg}");
}
