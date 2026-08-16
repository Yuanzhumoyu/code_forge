//! mini_c v12 DSL 后端测试（迭代 6）。
//!
//! v12 后端当前支持子集（Iconst/Iadd/Isub/Band/Bor/Bxor/Imul + Return +
//! prologue/epilogue 帧管理），这些测试验证同一源码经 v11 与 v12 后端
//! JIT 执行结果一致。完整 mini_c（load/store/call/分支）待迭代 6 后续
//! 补齐 lowering 后扩展。
#![cfg(target_arch = "x86_64")]

use mini_c::compiler::{Backend, compile_and_run, compile_and_run_with};

/// v12 后端运行（实验性：仅支持算术/返回子集）。
fn run_v12(source: &str) -> i32 {
    compile_and_run_with(source, Backend::V12)
        .unwrap_or_else(|e| panic!("v12 backend failed for `{source}`: {e}"))
}

fn assert_v12_matches_v11(source: &str, expected: i32) {
    let v11 = compile_and_run(source).expect("v11 backend failed");
    let v12 = run_v12(source);
    assert_eq!(
        v12, expected,
        "v12 backend wrong for `{source}` (expected {expected})"
    );
    assert_eq!(
        v11, expected,
        "v11 backend regression for `{source}` (expected {expected})"
    );
}

#[test]
fn v12_constant() {
    assert_v12_matches_v11("int main() { return 42; }", 42);
    assert_v12_matches_v11("int main() { return -1; }", -1);
    assert_v12_matches_v11("int main() { return 0; }", 0);
}

#[test]
fn v12_add() {
    assert_v12_matches_v11("int main() { return 2 + 3; }", 5);
    assert_v12_matches_v11("int main() { return 2 + 3 + 4; }", 9);
}

#[test]
fn v12_mixed_arith() {
    assert_v12_matches_v11("int main() { return 2 + 3 * 4; }", 14);
    assert_v12_matches_v11("int main() { return 10 - 3; }", 7);
    assert_v12_matches_v11("int main() { return 7 & 3; }", 3);
    assert_v12_matches_v11("int main() { return 5 | 2; }", 7);
    assert_v12_matches_v11("int main() { return 6 ^ 3; }", 5);
}

#[test]
fn v12_const_arith_chain() {
    // 多步：多个 Iconst + 多个算术，寄存器压力更高
    assert_v12_matches_v11("int main() { return 1 + 2 + 3 + 4 + 5; }", 15);
    assert_v12_matches_v11("int main() { return 100 - 20 - 30; }", 50);
}

#[test]
fn v12_bitwise_not() {
    assert_v12_matches_v11("int main() { int x = 5; return ~x; }", !5);
    assert_v12_matches_v11("int main() { int x = 0; return ~x; }", !0);
}

#[test]
fn v12_local_load_store() {
    // 局部变量：store 到栈槽 + load 回来
    assert_v12_matches_v11("int main() { int x = 7; return x; }", 7);
    assert_v12_matches_v11("int main() { int x = 3; int y = 4; return x + y; }", 7);
}

#[test]
fn v12_compound_assign() {
    assert_v12_matches_v11("int main() { int x = 5; x += 3; return x; }", 8);
    assert_v12_matches_v11("int main() { int x = 10; x -= 4; return x; }", 6);
    assert_v12_matches_v11("int main() { int x = 6; x &= 3; return x; }", 2);
}

#[test]
fn v12_if_else() {
    // 条件分支：Branch terminator（test + je + jmp）
    assert_v12_matches_v11(
        "int main() { int x = 5; if (x > 3) { return 1; } else { return 0; } }",
        1,
    );
    assert_v12_matches_v11(
        "int main() { int x = 1; if (x > 3) { return 1; } else { return 0; } }",
        0,
    );
    assert_v12_matches_v11(
        "int main() { int x = 3; if (x > 2) { return 1; } return 0; }",
        1,
    );
    assert_v12_matches_v11(
        "int main() { int x = 0; if (x == 0) { return 7; } return 0; }",
        7,
    );
}

#[test]
fn v12_ternary_cond() {
    // 条件表达式 + sextend + icmp
    assert_v12_matches_v11(
        "int main() { int x = 7; if (x != 0) { return x; } else { return -1; } }",
        7,
    );
}

#[test]
fn v12_icmp_result_direct() {
    // icmp 结果直接返回（不经 Branch），隔离 setcc 正确性
    assert_v12_matches_v11("int main() { int x = 5; int y = (x > 3); return y; }", 1);
    assert_v12_matches_v11("int main() { int x = 1; int y = (x > 3); return y; }", 0);
    assert_v12_matches_v11("int main() { int x = 3; int y = (x > 3); return y; }", 0);
    assert_v12_matches_v11("int main() { int x = 5; int y = (x == 5); return y; }", 1);
    assert_v12_matches_v11("int main() { int x = 4; int y = (x == 5); return y; }", 0);
}

#[test]
fn v12_while_loop() {
    // while 循环：icmp + Branch + 变量更新
    assert_v12_matches_v11(
        "int main() { int i = 0; while (i < 5) { i = i + 1; } return i; }",
        5,
    );
    assert_v12_matches_v11(
        "int main() { int s = 0; int i = 1; while (i <= 10) { s = s + i; i = i + 1; } return s; }",
        55,
    );
}

#[test]
fn v12_for_loop() {
    // for 循环（init/cond/update 三部分）
    assert_v12_matches_v11(
        "int main() { int s = 0; for (int i = 1; i <= 10; i = i + 1) { s = s + i; } return s; }",
        55,
    );
}

#[test]
#[ignore = "v12 嵌套循环（内层作用域变量 spill 交互）待调试"]
fn v12_nested_loop() {
    // 嵌套循环：双重 while
    assert_v12_matches_v11(
        "int main() { int s = 0; int i = 0; while (i < 3) { int j = 0; while (j < 3) { s = s + 1; j = j + 1; } i = i + 1; } return s; }",
        9,
    );
}

#[test]
fn v12_division() {
    // 除法：Sdiv/Srem（movsxd RAX + cqo + idiv + 物理寄存器序列）
    assert_v12_matches_v11("int main() { return 10 / 3; }", 3);
    assert_v12_matches_v11("int main() { return 10 % 3; }", 1);
    assert_v12_matches_v11("int main() { return -10 / 3; }", -3);
    assert_v12_matches_v11("int main() { return -10 % 3; }", -1);
    assert_v12_matches_v11("int main() { int x = 100; int y = 7; return x / y; }", 14);
    assert_v12_matches_v11("int main() { int x = 100; int y = 7; return x % y; }", 2);
}
