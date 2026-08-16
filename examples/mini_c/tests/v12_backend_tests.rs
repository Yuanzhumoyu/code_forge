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
fn v12_nested_loop() {
    // 嵌套循环：双重 while（修复：mini_c stack_addr(0)+iadd 模式的槽深计入
    // max_stack_bytes，spill 槽不再覆盖内层作用域局部变量）
    assert_v12_matches_v11(
        "int main() { int s = 0; int i = 0; while (i < 3) { int j = 0; while (j < 3) { s = s + 1; j = j + 1; } i = i + 1; } return s; }",
        9,
    );
    // 外层累加 + 内层独立计数（内层不触碰 s）
    assert_v12_matches_v11(
        "int main() { int s = 0; int i = 0; while (i < 3) { int j = 0; while (j < 2) { j = j + 1; } s = s + 1; i = i + 1; } return s; }",
        3,
    );
    // 深层嵌套（3 层）+ 内层累加
    assert_v12_matches_v11(
        "int main() { int s = 0; int i = 0; while (i < 2) { int j = 0; while (j < 2) { int k = 0; while (k < 2) { s = s + 1; k = k + 1; } j = j + 1; } i = i + 1; } return s; }",
        8,
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

#[test]
fn v12_inlined_call() {
    // AST 内联调用（mini_c 的 Call 在 AST 层内联，不生成 CALL 指令）
    assert_v12_matches_v11(
        "int add(int a, int b) { return a + b; } int main() { return add(2, 3); }",
        5,
    );
    assert_v12_matches_v11(
        "int add(int a, int b) { return a + b; } int main() { return add(add(10, 20), 12); }",
        42,
    );
}

#[test]
fn v12_unary_logical() {
    // 单目（-5、!x）+ 逻辑（icmp + sextend）
    assert_v12_matches_v11("int main() { return -5; }", -5);
    assert_v12_matches_v11("int main() { int x = 0; return !x; }", 1);
    assert_v12_matches_v11("int main() { int x = 1; return !x; }", 0);
    assert_v12_matches_v11("int main() { int x = 7; int y = (x != 0); return y; }", 1);
}

#[test]
fn v12_logical_and_or() {
    // && / ||：icmp(NotEqual) + band/bor + sextend（非短路求值）
    assert_v12_matches_v11("int main() { return 1 && 1; }", 1);
    assert_v12_matches_v11("int main() { return 1 && 0; }", 0);
    assert_v12_matches_v11("int main() { return 0 && 1; }", 0);
    assert_v12_matches_v11("int main() { return 0 || 1; }", 1);
    assert_v12_matches_v11("int main() { return 0 || 0; }", 0);
    assert_v12_matches_v11(
        "int main() { int x = 3; int y = 5; return (x < y) && (y > 2); }",
        1,
    );
    assert_v12_matches_v11(
        "int main() { int x = 3; int y = 5; return (x > y) || (y == 5); }",
        1,
    );
}

#[test]
fn v12_enum_const() {
    // 枚举常量（符号表解析为数字，需在函数体内声明）
    assert_v12_matches_v11(
        "int main() { enum Color { RED, GREEN, BLUE }; return BLUE; }",
        2,
    );
    assert_v12_matches_v11("int main() { enum X { A = 5, B }; return B; }", 6);
}

#[test]
fn v12_do_while() {
    // do-while：先执行后判断（cond 在 body 之后，jmp 后向）
    assert_v12_matches_v11(
        "int main() { int i = 0; do { i = i + 1; } while (i < 5); return i; }",
        5,
    );
    assert_v12_matches_v11(
        "int main() { int i = 10; do { i = i + 1; } while (i < 5); return i; }",
        11,
    );
}

#[test]
fn v12_break_continue() {
    // break 提前退出 + continue 跳过本次迭代（有界循环；continue 用 while——
    // mini_c 的 for-continue 会跳过 update 导致死循环，属前端既有语义）
    assert_v12_matches_v11(
        "int main() { int i = 0; while (i < 100) { i = i + 1; if (i >= 3) { break; } } return i; }",
        3,
    );
    assert_v12_matches_v11(
        "int main() { int s = 0; int i = 0; while (i < 5) { i = i + 1; if (i == 3) { continue; } s = s + i; } return s; }",
        12,
    );
}
