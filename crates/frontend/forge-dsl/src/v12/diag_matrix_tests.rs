//! S1 验收：**诊断矩阵**（多错误 + 精确位置 + 错误码 + 附注）。
//!
//! 取代 S0 的 `s0_baseline_tests.rs`（那份基线钉记录了旧行为：fail-fast 单错、
//! 无名节退化 1:1、`[emit]`/`[spill]` 完全不校验；实测记录保留在
//! `docs/plans/forge-dsl-v18-plan.md` §12.4）。
//!
//! 断言方式：**位置的期望值由文档文本自己算出**（`line_of(doc, needle)` 找含该声明的行），
//! 因此"诊断指到哪一行"是可复核的，而不是把行号抄死在测试里。
//!
//! 运行：`cargo test -p forge-dsl --lib diag_matrix -- --nocapture`

use super::diag::Diag;
use super::parse_and_validate;

/// 合法骨架（riscv 风格定宽 ISA，含 lowering / ref / abi / emit / spill）。
const BASE: &str = r#"
[meta]
name = "s1_base"
[encoding]
kind = "fixed"
bits = 32

[reg.gpr8]
names = ["X0", "X1", "X2", "X3", "X8", "X9", "X10"]

[conventions.bitfields]
rd     = { offset = 7,  width = 5 }
rs1    = { offset = 15, width = 5 }
rs2    = { offset = 20, width = 5 }
opcode = { offset = 0,  width = 7 }
funct3 = { offset = 12, width = 3 }

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
ref = "add"
opcode = 0x33
fields = { funct3 = 0 }
ops = ["dst:gpr:out", "src:gpr", "src2:gpr"]
asm = "add {dst}, {src}, {src2}"

[[lowering]]
op = "Iadd"
insts = ["add {out}, {0}, {1}"]

[abi]
scratch = ["X9"]
ret_regs = ["X10"]

[emit.prologue]
insts = ["ADD X1, X2, X3"]

[spill.GPR]
load = "ADD {0}, {1}, X0"
store = "ADD {0}, {1}, X0"
base = "X8"
"#;

/// 追加一段声明（TOML 合法即可）。
fn plus(extra: &str) -> String {
    format!("{BASE}\n{extra}\n")
}

/// 取出全部诊断（成功即 panic）。
fn errs(doc: &str) -> Vec<Diag> {
    match parse_and_validate(doc) {
        Ok(_) => panic!("期望报错，但校验通过了"),
        Err(e) => e.diags(),
    }
}

/// 同上，但"没报错"返回 `None`（矩阵里用来把无效用例报成失败而不是 panic）。
fn errs_opt(doc: &str) -> Option<Vec<Diag>> {
    match parse_and_validate(doc) {
        Ok(_) => None,
        Err(e) => Some(e.diags()),
    }
}

/// 含 `needle` 的那一行（1-based）；找不到即 panic。
fn line_of(doc: &str, needle: &str) -> usize {
    doc.lines()
        .position(|l| l.contains(needle))
        .map(|i| i + 1)
        .unwrap_or_else(|| panic!("夹具里找不到 `{needle}`"))
}

/// 含 `needle` 的**最后**一行（重复声明取第二处用）。
fn line_of_last(doc: &str, needle: &str) -> usize {
    doc.lines()
        .enumerate()
        .filter(|(_, l)| l.contains(needle))
        .map(|(i, _)| i + 1)
        .collect::<Vec<_>>()
        .last()
        .copied()
        .unwrap_or_else(|| panic!("夹具里找不到 `{needle}`"))
}

// ─────────────────────── 逐条：错误码 + 位置 ───────────────────────

#[test]
fn diagnostic_matrix_has_codes_and_exact_lines() {
    // (标签, 文档, 期望错误码, 消息里的关键片段)
    let cases: Vec<(&str, String, &str, &str)> = vec![
        (
            "meta 名非法",
            BASE.replace("name = \"s1_base\"", "name = \"1bad\""),
            "DSL-META",
            "1bad",
        ),
        (
            "reg 名字条数不符",
            BASE.replace(
                "names = [\"X0\", \"X1\", \"X2\", \"X3\", \"X8\", \"X9\", \"X10\"]",
                "names = [\"X0\", \"X1\"]\ncount = 4",
            ),
            "DSL-REG",
            "count",
        ),
        (
            "缺 GPR 组",
            BASE.replace("[reg.gpr8]", "[reg.fpr4]"),
            "DSL-REG",
            "GPR",
        ),
        (
            "位域越界",
            BASE.replace(
                "funct3 = { offset = 12, width = 3 }",
                "funct3 = { offset = 12, width = 3 }\nwide = { offset = 30, width = 8 }",
            ),
            "DSL-CONV",
            "wide",
        ),
        (
            "槽重复",
            plus("[[operand_slots]]\nname = \"gpr\"\nkind = \"reg\"\nclass = \"gpr8\""),
            "DSL-SLOT",
            "gpr",
        ),
        (
            "槽宽为 0",
            plus("[[operand_slots]]\nname = \"imm0\"\nkind = \"imm\"\nwidth = 0"),
            "DSL-SLOT",
            "imm0",
        ),
        (
            "form 引用未声明位域",
            plus(
                "[[forms]]\nname = \"BAD\"\nopcode_field = \"opcode\"\noperand_fields = [\"nope\"]",
            ),
            "DSL-FORM",
            "nope",
        ),
        (
            "指令引用未声明 form",
            BASE.replace("form = \"R\"", "form = \"NOPE\""),
            "DSL-INST",
            "NOPE",
        ),
        (
            "指令引用未声明槽",
            plus(
                "[[instructions]]\nname = \"SUB\"\nform = \"R\"\nopcode = 0x33\n\
                 fields = { funct3 = 0 }\nops = [\"dst:nope:out\"]\nasm = \"sub {dst}\"",
            ),
            "DSL-INST",
            "nope",
        ),
        (
            "指令重名",
            plus(
                "[[instructions]]\nname = \"ADD\"\nform = \"R\"\nopcode = 0x33\n\
                 fields = { funct3 = 0 }\nops = [\"dst:gpr:out\"]\nasm = \"add {dst}\"",
            ),
            "DSL-INST",
            "duplicate instruction name",
        ),
        (
            "指令定宽字段未声明",
            BASE.replace("fields = { funct3 = 0 }", "fields = { nope = 0 }"),
            "DSL-INST",
            "nope",
        ),
        (
            "模板行缺编码信息",
            plus(
                "[[templates]]\nname = \"F\"\n\
                 body = { form = \"R\", ops = [\"dst:gpr:out\"], asm = \"f {dst}\" }\n\
                 rows = [ { inst = \"V1\" } ]",
            ),
            "DSL-TEMPLATE",
            "编码信息",
        ),
        (
            "模板实例重名",
            plus(
                "[[templates]]\nname = \"F\"\n\
                 body = { form = \"R\", opcode = 0x33, ops = [\"dst:gpr:out\"], asm = \"f {dst}\" }\n\
                 rows = [ { inst = \"V1\" }, { inst = \"V1\" } ]",
            ),
            "DSL-INST",
            "duplicate instruction name",
        ),
        (
            "引用名与指令名冲突",
            plus(
                "[[instructions]]\nname = \"SUB\"\nref = \"ADD\"\nform = \"R\"\nopcode = 0x33\n\
                 fields = { funct3 = 0 }\nops = [\"dst:gpr:out\"]\nasm = \"sub {dst}\"",
            ),
            "DSL-INST",
            "与指令名冲突",
        ),
        (
            "引用名为空",
            plus(
                "[[instructions]]\nname = \"SUB\"\nref = \"\"\nform = \"R\"\nopcode = 0x33\n\
                 fields = { funct3 = 0 }\nops = [\"dst:gpr:out\"]\nasm = \"sub {dst}\"",
            ),
            "DSL-INST",
            "ref 不能为空",
        ),
        (
            "lowering 未知引用",
            plus("[[lowering]]\nop = \"Isub\"\ninsts = [\"nope {out}, {0}, {1}\"]"),
            "DSL-LOWER",
            "nope",
        ),
        (
            "lowering 未知属性",
            plus(
                "[[lowering]]\nop = \"Isub\"\ninsts = [\"add {out}, {0}, {1}\"]\n\
                 when = { eq = [\"rd_width\", 32] }",
            ),
            "DSL-LOWER",
            "rd_width",
        ),
        (
            "lowering 完全重复",
            plus("[[lowering]]\nop = \"Iadd\"\ninsts = [\"add {out}, {0}, {1}\"]"),
            "DSL-LOWER",
            "完全重复",
        ),
        (
            "lowering 死规则",
            plus(
                "[[lowering]]\nop = \"Iand\"\ninsts = [\"add {out}, {0}, {1}\"]\n\
                 when = { in = [\"rs1_width\", [32, 64]] }\n\
                 [[lowering]]\nop = \"Iand\"\ninsts = [\"add {out}, {0}, {1}\"]\n\
                 when = { eq = [\"rs1_width\", 32] }",
            ),
            "DSL-LOWER",
            "死规则",
        ),
        (
            "pattern 未知属性",
            plus(
                "[[pattern]]\nmatch = \"Fadd(Fmul(a, b), c)\"\nwhen = { eq = [\"nope\", 1] }\n\
                 insts = [\"add {out}, {a}, {b}\"]",
            ),
            "DSL-PATTERN",
            "nope",
        ),
        (
            "pattern 叶变量重复",
            plus(
                "[[pattern]]\nmatch = \"Fadd(Fmul(a, a), c)\"\n\
                 insts = [\"add {out}, {a}, {c}\"]",
            ),
            "DSL-PATTERN",
            "重复出现",
        ),
        (
            "abi 未知寄存器名",
            BASE.replace("scratch = [\"X9\"]", "scratch = [\"NOPE\"]"),
            "DSL-ABI",
            "NOPE",
        ),
        (
            "emit 未知指令引用",
            plus("[emit.epilogue]\ninsts = [\"NO_SUCH RSP, RBP\"]"),
            "DSL-EMIT",
            "NO_SUCH",
        ),
        (
            "emit 未知伪指令",
            plus("[emit.epilogue]\ninsts = [\"@nope\"]"),
            "DSL-EMIT",
            "@nope",
        ),
        (
            "emit 未知占位符",
            plus("[emit.epilogue]\ninsts = [\"ADD {0}, {1}, {bogus}\"]"),
            "DSL-EMIT",
            "{bogus}",
        ),
        (
            "emit 空模板",
            plus("[emit.epilogue]\ninsts = []"),
            "DSL-EMIT",
            "must not be empty",
        ),
        (
            "spill 未知指令引用",
            plus("[spill.FPR]\nload = \"NO_SUCH {0}, {1}\"\nstore = \"NO_SUCH {0}, {1}\""),
            "DSL-SPILL",
            "NO_SUCH",
        ),
        (
            "spill 未知占位符",
            plus("[spill.FPR]\nload = \"ADD {0}, {1}, {dst}\"\nstore = \"ADD {0}, {1}, X0\""),
            "DSL-SPILL",
            "{dst}",
        ),
        (
            "spill 基址未声明",
            plus(
                "[spill.FPR]\nload = \"ADD {0}, {1}, X0\"\nstore = \"ADD {0}, {1}, X0\"\nbase = \"NOPE\"",
            ),
            "DSL-SPILL",
            "NOPE",
        ),
        (
            "模板 rows 为空",
            plus(
                "[[templates]]\nname = \"F\"\n\
                 body = { form = \"R\", opcode = 0x33, ops = [\"dst:gpr:out\"], asm = \"f {dst}\" }\n\
                 rows = []",
            ),
            "DSL-TOML",
            "rows 不能为空",
        ),
    ];

    let mut failures = Vec::new();
    for (label, doc, code, needle) in &cases {
        let Some(diags) = errs_opt(doc) else {
            failures.push(format!("{label}: 夹具**没有报错**——用例无效（应改夹具）"));
            continue;
        };
        let hit = diags.iter().find(|d| d.msg.contains(needle));
        match hit {
            None => failures.push(format!(
                "{label}: 没有诊断提到 `{needle}`；实际 {:#?}",
                diags
                    .iter()
                    .map(|d| format!("{}@{}:{}", d.code, d.line, d.msg))
                    .collect::<Vec<_>>()
            )),
            Some(d) => {
                if d.code != *code {
                    failures.push(format!("{label}: 错误码应为 {code}，实际 {}", d.code));
                }
                // 位置：必须落在"含该 needle 的声明块"里。精确到具体行的断言由
                // `headline_positions_are_exact` 承担（矩阵里同一个 needle 可能出现在
                // 多处声明，例如 `ADD` 既是指令名又出现在别名里）。
                if d.line <= 1 {
                    failures.push(format!(
                        "{label}: 位置退化成第 {} 行（不该发生：至少能落到节头）",
                        d.line
                    ));
                }
            }
        }
    }
    assert!(
        failures.is_empty(),
        "诊断矩阵 {} 例中有 {} 例不符合预期：\n{}",
        cases.len(),
        failures.len(),
        failures.join("\n")
    );
    assert!(cases.len() >= 30, "矩阵至少 30 例，实际 {}", cases.len());
}

/// 位置上必须精确的"头条用例"（消息 needle 与源码 needle 不同的那几条）。
#[test]
fn headline_positions_are_exact() {
    // 重名：诊断必须落在**第二处**声明上，并附注第一处。
    let dup = plus(
        "[[instructions]]\nname = \"ADD\"\nform = \"R\"\nopcode = 0x33\n\
         fields = { funct3 = 0 }\nops = [\"dst:gpr:out\"]\nasm = \"add {dst}\"",
    );
    let d = errs(&dup)
        .into_iter()
        .find(|d| d.msg.contains("duplicate instruction name"))
        .expect("应有重名诊断");
    assert_eq!(
        d.line,
        line_of_last(&dup, "name = \"ADD\""),
        "重名诊断应指向第二处声明（实际消息：{}）",
        d.msg
    );
    assert!(
        d.notes.iter().any(|n| n.contains("同名声明")),
        "重名诊断应附注另一处声明位置：{d:#?}"
    );

    // emit：未知指令引用 / 未知伪指令 / 未知占位符 都要落在那一行。
    for (doc, msg_needle, src_needle) in [
        (
            plus("[emit.epilogue]\ninsts = [\"NO_SUCH RSP, RBP\"]"),
            "NO_SUCH",
            "NO_SUCH RSP, RBP",
        ),
        (
            plus("[emit.epilogue]\ninsts = [\"@nope\"]"),
            "@nope",
            "@nope",
        ),
        (
            plus("[emit.epilogue]\ninsts = [\"ADD {0}, {1}, {bogus}\"]"),
            "{bogus}",
            "{bogus}",
        ),
    ] {
        let d = errs(&doc)
            .into_iter()
            .find(|d| d.msg.contains(msg_needle))
            .unwrap_or_else(|| panic!("应有关于 `{msg_needle}` 的诊断"));
        assert_eq!(d.line, line_of(&doc, src_needle), "消息：{}", d.msg);
    }

    // lowering：未知属性落在该 `[[lowering]]` 节（`op = "..."` 行）。
    let lw = plus(
        "[[lowering]]\nop = \"Isub\"\ninsts = [\"add {out}, {0}, {1}\"]\n\
         when = { eq = [\"rd_width\", 32] }",
    );
    let d = errs(&lw)
        .into_iter()
        .find(|d| d.msg.contains("rd_width"))
        .expect("应有未知属性诊断");
    // 精准到出错的键行（`when = ...`），而不是只到 `op = "..."` 声明行。
    assert_eq!(
        d.line,
        line_of(&lw, "when = { eq = [\"rd_width\""),
        "消息：{}",
        d.msg
    );

    // spill：未知指令引用落在 load/store 那一行。
    let sp = plus("[spill.FPR]\nload = \"NO_SUCH {0}, {1}\"\nstore = \"ADD {0}, {1}, X0\"");
    let d = errs(&sp)
        .into_iter()
        .find(|d| d.msg.contains("NO_SUCH"))
        .expect("应有 spill 诊断");
    assert_eq!(d.line, line_of(&sp, "load = \"NO_SUCH"), "消息：{}", d.msg);
}

// ─────────────────────── 一次性列全 / 附注 / 上限 ───────────────────────

#[test]
fn three_independent_errors_are_reported_together() {
    // S0 基线：此处只报 1 条（fail-fast）。S1 起三条一次给全。
    let doc = BASE.replace("form = \"R\"", "form = \"NOPE\"")
        + r#"
[[instructions]]
name = "ADD"
form = "R"
opcode = 0x33
fields = { funct3 = 0 }
ops = ["dst:gpr:out"]
asm = "add {dst}"

[[lowering]]
op = "Isub"
insts = ["add {out}, {0}, {1}"]
when = { eq = ["rd_width", 32] }
"#;
    let diags = errs(&doc);
    assert!(
        diags.len() >= 3,
        "三条独立错误应一次报出（实际 {} 条）：{diags:#?}",
        diags.len()
    );
    let mut kinds: Vec<&str> = diags.iter().map(|d| d.code).collect();
    kinds.sort_unstable();
    kinds.dedup();
    assert!(
        kinds.contains(&"DSL-INST") && kinds.contains(&"DSL-LOWER"),
        "应同时含 DSL-INST 与 DSL-LOWER，实际 {kinds:?}"
    );
    // 渲染文本也必须是多行、每行带位置与错误码。
    let text = match parse_and_validate(&doc) {
        Err(e) => e.render(None),
        Ok(_) => unreachable!(),
    };
    assert!(text.lines().count() >= 3, "{text}");
    assert!(text.contains("DSL-INST:"), "{text}");
    assert!(text.contains("DSL-LOWER:"), "{text}");
}

#[test]
fn duplicate_declaration_points_at_both_lines() {
    let doc = plus(
        "[[instructions]]\nname = \"ADD\"\nform = \"R\"\nopcode = 0x33\n\
         fields = { funct3 = 0 }\nops = [\"dst:gpr:out\"]\nasm = \"add {dst}\"",
    );
    let dups = errs(&doc)
        .into_iter()
        .find(|d| d.msg.contains("duplicate instruction name"))
        .expect("应有重名诊断");
    // 第一处声明（BASE 里的 ADD）与第二处（追加的 ADD）都要给出来。
    assert!(
        dups.notes.iter().any(|n| n.contains("同名声明")),
        "重名诊断应附注另一处声明位置，实际：{dups:#?}"
    );
    assert_eq!(
        dups.line,
        line_of_last(&doc, "name = \"ADD\""),
        "重名诊断应指向第二处声明"
    );
}

#[test]
fn diagnostics_are_capped_with_a_tail_note() {
    // 40 条独立错误 ⇒ 最多 32 条 + "另有 N 条"。
    let mut extra = String::new();
    for i in 0..40 {
        extra.push_str(&format!(
            "\n[[instructions]]\nname = \"X{i}\"\nform = \"NOPE\"\nopcode = 0x33\n\
             fields = {{ funct3 = 0 }}\nops = [\"dst:gpr:out\"]\nasm = \"x {{dst}}\"\n"
        ));
    }
    let doc = format!("{BASE}{extra}");
    let text = match parse_and_validate(&doc) {
        Err(e) => e.render(None),
        Ok(_) => panic!("应报错"),
    };
    assert!(
        text.contains("另有") && text.contains("条错误未列出"),
        "超出上限时应给出计数尾巴：{text}"
    );
    let n = errs(&doc).len();
    assert!(n <= super::diag::MAX_DIAGS, "诊断条数应封顶，实际 {n}");
}

#[test]
fn unknown_name_falls_back_to_section_header_not_line_one() {
    // S0 基线：`[stack].align` 的错误退化成 1:1。S1 起落到 `[stack]` 那一行。
    let doc = BASE.replace("[meta]", "[stack]\nalign = 0\n\n[meta]");
    let d = errs(&doc)
        .into_iter()
        .find(|d| d.msg.contains("align"))
        .expect("应有 align 诊断");
    assert_eq!(
        d.line,
        line_of(&doc, "[stack]"),
        "无名节的错误应落在节头行（不是 1:1）"
    );
    assert!(d.line > 1, "不应退化到第 1 行");
}

#[test]
fn base_fixture_is_valid() {
    parse_and_validate(BASE).expect("S1 骨架必须合法");
}
