//! 解析错误诊断守卫（v3 S7 文本层第一步）。
//!
//! 改前实测：`parse_to_ast` 直接 `format!("{:?}", e)`，用户看到的是 lalrpop 的
//! 内部 Debug：
//!
//! ```text
//! UnrecognizedToken { token: (17, Ident("entry"), 22), expected: ["Target", "VoidTy",
//! … 88 项 …] }
//! ```
//!
//! 没有行列、没有出错处的源码行、还把 88 个文法内部记号名倒给用户（`VconstOp` /
//! `UselistorderBbKw` 这类名字会直接出现在错误里）。
//!
//! 现在：`解析错误 <line>:<col>：非预期 <源码切片>` + 源码行 + 插入符 + 收敛到
//! 8 项的期望集合（内部记号名做用户化映射）。
//!
//! 本文件把这几条钉住：位置在、源码行与插入符在、期望集合有界、内部 Debug 形态
//! 不再泄漏、词法错误（非法字符）也有位置。

use forge_ir::text::parser::parse_module;

fn parse_err(src: &str) -> String {
    match parse_module(src) {
        Err(forge_ir::IrError::Parse(msg)) => msg,
        Err(other) => panic!("期望 Parse 错误，实际 {other:?}"),
        Ok(_) => panic!("期望解析错误，实际解析成功"),
    }
}

/// 语法错误带 `行:列`，并画出出错处的源码行与插入符。
#[test]
fn syntax_error_reports_line_column_and_caret() {
    let msg = parse_err("define i32 @f( {\nentry:\n  ret i32 0\n}\n");
    assert!(
        msg.contains("解析错误 2:1"),
        "应报第 2 行第 1 列（实测：{msg}）"
    );
    assert!(
        msg.contains("entry:"),
        "应回显出错处的源码行（实测：{msg}）"
    );
    assert!(msg.contains('^'), "应有插入符（实测：{msg}）");
    assert!(
        msg.contains("非预期 `entry`"),
        "应回显出错的记号原文（实测：{msg}）"
    );
}

/// 期望集合有界（8 项 + “共 N 个”），且不泄漏 lalrpop 内部 Debug 形态。
#[test]
fn syntax_error_expected_list_is_bounded_and_user_facing() {
    let msg = parse_err("garbage !!!\n");
    assert!(
        msg.contains("期望其中之一："),
        "应给出期望集合（实测：{msg}）"
    );
    assert!(
        msg.contains("（共 ") && msg.contains(" 个）"),
        "期望集合超过 8 项时应给总数（实测：{msg}）"
    );
    let listed = msg
        .split("期望其中之一：")
        .nth(1)
        .and_then(|rest| rest.split('（').next())
        .expect("期望列表");
    let items = listed.matches('`').count() / 2;
    assert!(items <= 8, "期望集合最多列 8 项（实测 {items} 项：{msg}）");
    for leaked in [
        "UnrecognizedToken",
        "UnrecognizedEof",
        "expected: [",
        "VconstOp",
        "UselistorderBbKw",
    ] {
        assert!(
            !msg.contains(leaked),
            "不应泄漏内部记号名 `{leaked}`（实测：{msg}）"
        );
    }
    // 用户化映射：`@全局名`/`%局部名`/`iN` 这类提示应出现，而不是 `GlobalId`/`IntTy`
    assert!(
        msg.contains("`define`") || msg.contains("`target`"),
        "首字符应小写化为用户写法（实测：{msg}）"
    );
}

/// 结构未结束（EOF）也有位置，并说明"输入结束"而不是给一堆记号名。
#[test]
fn unexpected_eof_reports_position_and_reason() {
    let msg = parse_err("define void @f() {\nentry:\n  ret void\n");
    assert!(msg.contains("解析错误 3:"), "应报第 3 行（实测：{msg}）");
    assert!(
        msg.contains("输入在结构未结束时结束"),
        "应说明输入意外结束（实测：{msg}）"
    );
    assert!(msg.contains('^'), "应有插入符（实测：{msg}）");
}

/// 非法字符（词法错误）同样有行列——`LexError::BadChar` 现在带偏移。
///
/// 注意 `$name` 是 comdat 记号、`;`/`/* */` 是注释，都不会触发词法错误；
/// `~` 在文法里没有对应记号，才是真正的非法字符。
#[test]
fn lexer_error_reports_position() {
    let msg = parse_err("define i32 @f() {\nentry:\n  ret i32 0\n}\n~\n");
    assert!(
        msg.contains("解析错误 5:1"),
        "非法字符应报第 5 行第 1 列（实测：{msg}）"
    );
    assert!(
        msg.contains("无法识别的字符"),
        "应说明是不可识别的字符（实测：{msg}）"
    );
    assert!(msg.contains('^'), "应有插入符（实测：{msg}）");
}

// ── 错误恢复（v3 S7）：一次尽量报多处 ──

/// 计数诊断条数（每个诊断都以 `解析错误 ` 开头）。
fn count_diagnostics(msg: &str) -> usize {
    msg.matches("解析错误 ").count()
}

/// 两处坏行 ⇒ 两条诊断，且都指向各自的行；首条与无恢复时**逐字节相同**。
#[test]
fn multiple_parse_errors_are_reported() {
    let src = "define i32 @f() {\nentry:\n  %a = zzz 1\n  %b = zzz 2\n  ret i32 0\n}\n";
    let msg = parse_err(src);
    assert_eq!(msg, parse_err(src), "诊断必须确定性（同输入同输出）");
    assert!(
        msg.contains("解析错误 3:12") && msg.contains("解析错误 4:12"),
        "应同时报第 3、4 行（实测：{msg}）"
    );
    assert!(
        msg.contains("（跳过第 3 行出错处后继续检查）"),
        "后续诊断应标明它的来源（实测：{msg}）"
    );
    assert_eq!(count_diagnostics(&msg), 2, "本例应恰好 2 条诊断：{msg}");
    // 首条诊断仍是原有格式（行号 + 源码行 + 插入符）
    assert!(
        msg.starts_with("解析错误 3:12：非预期 `1`\n3 |   %a = zzz 1\n"),
        "首条诊断格式不得改变（实测：{msg}）"
    );
    assert!(msg.contains("4 |   %b = zzz 2"), "第二条应回显它自己的行");
}

/// 诊断条数有上限（首条 + 最多两条恢复所得）。
#[test]
fn recovery_is_capped() {
    let src = "define i32 @f() {\nentry:\n  %a = zzz 1\n  %b = zzz 2\n  %c = zzz 3\n  %d = zzz 4\n  %e = zzz 5\n  ret i32 0\n}\n";
    let msg = parse_err(src);
    assert_eq!(
        count_diagnostics(&msg),
        3,
        "应恰好报 3 条（首条 + 2 条恢复所得）：{msg}"
    );
    assert!(
        !msg.contains("解析错误 6:12"),
        "超出上限的后续错误不再报（实测：{msg}）"
    );
}

/// 恢复**只用于报告**：删掉出错行的剩余内容能解析，也绝不放行。
#[test]
fn recovery_never_accepts_invalid_source() {
    // 出错处之后剩余部分本可解析（`%a = zzz` 是合法文法形态），
    // 但源码里有错误 ⇒ 必须仍然 Err。
    let src = "define i32 @f() {\nentry:\n  %a = zzz 1\n  ret i32 0\n}\n";
    assert!(
        forge_ir::text::parser::parse_module(src).is_err(),
        "恢复不得让非法源码变成 Ok"
    );
}

/// 没有恢复余地时（出错处就在行尾/输入末尾、或位置不前进）只报一条。
#[test]
fn recovery_stops_when_no_progress() {
    // 纯垃圾行：删掉后为空模块；不得重复报同一条
    let msg = parse_err("garbage !!!\n");
    assert_eq!(count_diagnostics(&msg), 1, "不得重复报同一处：{msg}");
    // 未闭合结构（EOF 错误）：出错位置在行尾，没有可删内容 ⇒ 单条
    let msg2 = parse_err("define void @f() {\nentry:\n  %x = add i32 1, 2\n");
    assert_eq!(count_diagnostics(&msg2), 1, "EOF 错误不恢复：{msg2}");
    assert!(msg2.contains("输入在结构未结束时结束"));
}

/// **边界（如实记录）**：恢复只覆盖**语法层**。语义阶段（未知 opcode、SSA 违规等）
/// 仍是"报第一处就停"——要让语义层也一次报多处，需要 IR 构建带着错误继续往下走
/// （占位值/坏实体），是另一类改动，不在本切片。
#[test]
fn semantic_errors_still_report_first_only() {
    let src = "define i32 @f() {\nentry:\n  zzz\n  yyy\n  ret i32 0\n}\n";
    let err = match forge_ir::text::parser::parse_module(src) {
        Err(forge_ir::IrError::Semantic(m)) => m,
        Err(other) => panic!("期望语义错误，实际 {other}"),
        Ok(_) => panic!("期望语义错误，实际解析成功"),
    };
    assert!(err.contains("Unknown opcode"), "实测：{err}");
    assert_eq!(
        err.matches("Unknown opcode").count(),
        1,
        "语义阶段仍只报第一处（实测：{err}）"
    );
}
