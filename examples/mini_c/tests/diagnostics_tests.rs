//! 前端诊断升级测试（P1 剩余项）：forge-hir 后端（Backend::Hir）的
//! lowering 错误必须携带源位置（行:列 + 行文本预览 + 指示线）。
//!
//! 使用 Backend::Hir 编译含错误的程序，断言错误消息含 `at L:C` 定位。

#![cfg(all(target_arch = "x86_64", windows))] // JIT 执行 Windows-x64-ABI x86 机器码
use mini_c::compiler::{Backend, compile_and_run_with};

/// 用 Hir 后端编译，返回错误消息。
fn hir_compile_err(source: &str) -> String {
    match compile_and_run_with(source, Backend::Hir) {
        Ok(_) => panic!("期望编译失败：\n{source}"),
        Err(msg) => msg,
    }
}

/// 诊断消息应含 `at 行:列` 定位 + 行文本预览 + 指示线。
fn assert_located(msg: &str) {
    assert!(
        msg.contains(" at "),
        "错误消息应含 'at 行:列' 定位，实际：\n{msg}"
    );
    assert!(
        msg.contains("  | "),
        "错误消息应含行文本预览，实际：\n{msg}"
    );
    assert!(msg.contains('^'), "错误消息应含指示线 ^，实际：\n{msg}");
}

/// 未定义变量：错误定位到引用该变量的行/列（Hir 后端 lookup 路径）。
#[test]
fn undefined_variable_reports_location() {
    // 第 3 行引用 undefined_var——错误应定位到 3 行（引用处）
    let src = "int main() {\n    int a = 1;\n    int b = undefined_var;\n    return a + b;\n}\n";
    let msg = hir_compile_err(src);
    assert!(
        msg.contains("undefined variable"),
        "错误类型应明确：\n{msg}"
    );
    assert_located(&msg);
    assert!(
        msg.contains(" at 3:"),
        "应定位到第 3 行（undefined_var 引用处），实际：\n{msg}"
    );
    // 行文本预览应包含出错的那一行
    assert!(
        msg.contains("int b = undefined_var"),
        "行文本预览应含出错行，实际：\n{msg}"
    );
}

/// 嵌套表达式错误：错误应定位到最内层表达式节点所在行。
#[test]
fn nested_expr_error_reports_innermost_line() {
    // 第 3 行的表达式里有未定义变量——定位到第 3 行
    let src = "int main() {\n    int x = 1;\n    return x + missing + 3;\n}\n";
    let msg = hir_compile_err(src);
    assert!(
        msg.contains("undefined variable"),
        "错误类型应明确：\n{msg}"
    );
    assert_located(&msg);
    assert!(msg.contains(" at 3:"), "应定位到第 3 行，实际：\n{msg}");
}

/// 正常程序不回归：Hir 后端编译成功不受诊断改动影响。
#[test]
fn valid_program_still_compiles() {
    let src = "int main() { int x = 40; return x + 2; }";
    let r = compile_and_run_with(src, Backend::Hir).expect("合法程序应编译成功");
    assert_eq!(r, 42);
}
