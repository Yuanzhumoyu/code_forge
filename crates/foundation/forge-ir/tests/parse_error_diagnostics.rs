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

use forge_ir::ir_parser::parse_module;

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
