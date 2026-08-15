//! Integration tests for the Mini C compiler.
//!
//! Each test compiles and JIT-executes a Mini C program, asserting the return value.
//!
//! ## Note
//! 历史上的 x86 后端 Branch lowering 缺陷（所有分支路由到 then 块）已在
//! `b87f0b7`（mini_c break SEGV 根治）等提交修复；`test_if_not_taken` /
//! `test_equal_false` 等 false 分支行为测试现在真实执行并通过，无需 `#[ignore]`。

use mini_c::compiler::compile_and_run;

fn run(source: &str) -> i32 {
    compile_and_run(source).expect("compilation failed")
}

// ============================================================
// Simple arithmetic
// ============================================================

#[test]
fn test_return_constant() {
    assert_eq!(run("int main() { return 42; }"), 42);
}

#[test]
fn test_return_addition() {
    assert_eq!(run("int main() { return 40 + 2; }"), 42);
}

#[test]
fn test_return_multiplication() {
    assert_eq!(run("int main() { return 6 * 7; }"), 42);
}

#[test]
fn test_return_subtraction() {
    assert_eq!(run("int main() { return 50 - 8; }"), 42);
}

#[test]
fn test_return_division() {
    assert_eq!(run("int main() { return 84 / 2; }"), 42);
}

#[test]
fn test_return_modulo() {
    assert_eq!(run("int main() { return 45 % 43; }"), 2);
}

#[test]
fn test_complex_arithmetic() {
    assert_eq!(run("int main() { return 2 + 3 * 8 + 16; }"), 42);
}

// ============================================================
// Local variables
// ============================================================

#[test]
fn test_variable_decl_and_use() {
    assert_eq!(run("int main() { int x = 42; return x; }"), 42);
}

#[test]
fn test_variable_arithmetic() {
    assert_eq!(
        run("int main() { int a = 40; int b = 2; return a + b; }"),
        42
    );
}

#[test]
fn test_variable_assignment() {
    assert_eq!(run("int main() { int x = 0; x = 42; return x; }"), 42);
}

// ============================================================
// Comparisons — used as values (not in branch conditions)
// ============================================================

#[test]
fn test_comparison_value_true() {
    // 10 > 5 is 1, stored in variable and returned
    assert_eq!(run("int main() { int r = 10 > 5; return r + 41; }"), 42);
}

#[test]
fn test_comparison_value_false() {
    // 3 > 5 is 0
    assert_eq!(run("int main() { int r = 3 > 5; return r + 42; }"), 42);
}

#[test]
fn test_equal_value_true() {
    assert_eq!(run("int main() { int r = 42 == 42; return r + 41; }"), 42);
}

#[test]
fn test_equal_value_false() {
    assert_eq!(run("int main() { int r = 42 == 0; return r + 42; }"), 42);
}

#[test]
fn test_not_equal_value() {
    assert_eq!(run("int main() { int r = 42 != 0; return r + 41; }"), 42);
}

#[test]
fn test_less_than_value() {
    assert_eq!(run("int main() { int r = 10 < 20; return r + 41; }"), 42);
}

#[test]
fn test_greater_than_value() {
    assert_eq!(run("int main() { int r = 50 > 40; return r + 41; }"), 42);
}

#[test]
fn test_less_equal_value() {
    assert_eq!(run("int main() { int r = 42 <= 42; return r + 41; }"), 42);
}

#[test]
fn test_greater_equal_value() {
    assert_eq!(run("int main() { int r = 42 >= 41; return r + 41; }"), 42);
}

// ============================================================
// Logical operators — as values
// ============================================================

#[test]
fn test_logical_and_value_true() {
    assert_eq!(
        run("int main() { int a = 1; int b = 2; int r = a && b; return r + 41; }"),
        42
    );
}

#[test]
fn test_logical_and_value_false() {
    assert_eq!(
        run("int main() { int a = 1; int b = 0; int r = a && b; return r + 42; }"),
        42
    );
}

#[test]
fn test_logical_or_value_true() {
    assert_eq!(
        run("int main() { int a = 0; int b = 1; int r = a || b; return r + 41; }"),
        42
    );
}

#[test]
fn test_logical_or_value_false() {
    assert_eq!(
        run("int main() { int a = 0; int b = 0; int r = a || b; return r + 42; }"),
        42
    );
}

// ============================================================
// Unary operators
// ============================================================

#[test]
fn test_unary_minus() {
    assert_eq!(run("int main() { return -42; }"), -42);
}

#[test]
fn test_unary_not_value_true() {
    // !0 = 1 in C
    assert_eq!(run("int main() { int r = !0; return r + 41; }"), 42);
}

#[test]
fn test_unary_not_value_false() {
    // !42 = 0 in C
    assert_eq!(run("int main() { int r = !42; return r + 42; }"), 42);
}

// ============================================================
// If/else — only test the TRUE branch (due to backend limitation)
// ============================================================

#[test]
fn test_if_true_branch_taken() {
    // Condition 1 is always true → then branch executes
    assert_eq!(
        run("int main() { int x = 10; if (x > 5) { return 42; } return 0; }"),
        42
    );
}

#[test]
fn test_if_else_true_branch_taken() {
    assert_eq!(
        run("int main() { int x = 10; if (x > 5) { return 42; } else { return 0; } }"),
        42
    );
}

#[test]
fn test_if_not_taken() {
    // False condition: x=3 is NOT > 5, so skip then-branch, execute return 0
    assert_eq!(
        run("int main() { int x = 3; if (x > 5) { return 42; } return 0; }"),
        0
    );
}

#[test]
fn test_equal_false() {
    // x=42 is NOT == 0, so skip then-branch, execute return 0
    assert_eq!(
        run("int main() { int x = 42; if (x == 0) { return 1; } return 0; }"),
        0
    );
}

// ============================================================
// For loop
// ============================================================

#[test]
fn test_for_loop_countdown() {
    assert_eq!(
        run(
            "int main() { int s = 0; for (int i = 10; i > 0; i = i - 1) { s = s + i; } return s; }"
        ),
        55
    );
}

// ============================================================
// Bitwise operators
// ============================================================

#[test]
fn test_bitwise_and() {
    // 5 & 3 = 1 → +41 = 42
    assert_eq!(run("int main() { int r = 5 & 3; return r + 41; }"), 42);
}

#[test]
fn test_bitwise_or() {
    // 8 | 2 = 10 → +32 = 42
    assert_eq!(run("int main() { int r = 8 | 2; return r + 32; }"), 42);
}

#[test]
fn test_bitwise_xor() {
    // 5 ^ 3 = 6 → +36 = 42
    assert_eq!(run("int main() { int r = 5 ^ 3; return r + 36; }"), 42);
}

#[test]
fn test_bitwise_not() {
    // ~42 = -43
    assert_eq!(run("int main() { int r = ~42; return r; }"), -43);
}

#[test]
fn test_bitwise_not_zero() {
    // ~0 = -1 (all bits set)
    assert_eq!(run("int main() { int r = ~0; return r; }"), -1);
}

#[test]
fn test_shift_left() {
    // 21 << 1 = 42
    assert_eq!(run("int main() { int r = 21 << 1; return r; }"), 42);
}

#[test]
fn test_shift_right() {
    // 84 >> 1 = 42
    assert_eq!(run("int main() { int r = 84 >> 1; return r; }"), 42);
}

#[test]
fn test_shift_by_zero() {
    assert_eq!(run("int main() { int r = 42 << 0; return r; }"), 42);
}

#[test]
fn test_bitwise_precedence() {
    // In C, & binds tighter than |: 1 | 2 & 3 = 1 | 2 = 3 → +39 = 42
    assert_eq!(run("int main() { int r = 1 | 2 & 3; return r + 39; }"), 42);
}

// ============================================================
// Compound assignment
// ============================================================

#[test]
fn test_compound_add() {
    assert_eq!(run("int main() { int x = 40; x += 2; return x; }"), 42);
}

#[test]
fn test_compound_sub() {
    assert_eq!(run("int main() { int x = 50; x -= 8; return x; }"), 42);
}

#[test]
fn test_compound_mul() {
    assert_eq!(run("int main() { int x = 6; x *= 7; return x; }"), 42);
}

#[test]
fn test_compound_div() {
    assert_eq!(run("int main() { int x = 84; x /= 2; return x; }"), 42);
}

#[test]
fn test_compound_mod() {
    assert_eq!(run("int main() { int x = 45; x %= 43; return x; }"), 2);
}

#[test]
fn test_compound_and() {
    // x = 7 (0b111); x &= 2 (0b010) → x = 2 (0b010)
    assert_eq!(run("int main() { int x = 7; x &= 2; return x + 40; }"), 42);
}

#[test]
fn test_compound_or() {
    // x = 8 (0b1000); x |= 2 (0b0010) → x = 10 (0b1010)
    assert_eq!(run("int main() { int x = 8; x |= 2; return x + 32; }"), 42);
}

#[test]
fn test_compound_xor() {
    // x = 5 (0b101); x ^= 3 (0b011) → x = 6 (0b110)
    assert_eq!(run("int main() { int x = 5; x ^= 3; return x + 36; }"), 42);
}

#[test]
fn test_compound_shl() {
    assert_eq!(run("int main() { int x = 21; x <<= 1; return x; }"), 42);
}

#[test]
fn test_compound_shr() {
    assert_eq!(run("int main() { int x = 168; x >>= 2; return x; }"), 42);
}

// ============================================================
// Integer literal formats
// ============================================================

#[test]
fn test_hex_literal() {
    assert_eq!(run("int main() { return 0x2A; }"), 42);
}

#[test]
fn test_hex_literal_uppercase() {
    assert_eq!(run("int main() { return 0X2A; }"), 42);
}

#[test]
fn test_octal_literal() {
    // 052 (octal) = 5*8 + 2 = 42
    assert_eq!(run("int main() { return 052; }"), 42);
}

#[test]
fn test_char_literal() {
    // '*' has ASCII value 42
    assert_eq!(run("int main() { int x = '*'; return x; }"), 42);
}

#[test]
fn test_char_literal_zero() {
    assert_eq!(run("int main() { int x = '0'; return x; }"), 48);
}

// ============================================================
// Unary plus
// ============================================================

#[test]
fn test_unary_plus() {
    assert_eq!(run("int main() { return +42; }"), 42);
}

#[test]
fn test_unary_plus_with_var() {
    assert_eq!(run("int main() { int x = 42; return +x; }"), 42);
}

// ============================================================
// Break statement
// ============================================================

#[test]
fn test_break_in_for() {
    // Break always exits the loop (uses unconditional jump)
    assert_eq!(
        run(
            "int main() { int x = 0; for (int i = 0; i < 100; i = i + 1) { x = 42; break; } return x; }"
        ),
        42
    );
}

#[test]
fn test_break_in_while() {
    assert_eq!(
        run("int main() { int x = 0; while (1) { x = 42; break; } return x; }"),
        42
    );
}

// ============================================================
// Do-while loop
// ============================================================

#[test]
fn test_do_while_executes_once() {
    // do-while always executes body at least once, then break exits
    assert_eq!(
        run("int main() { int x = 0; do { x = 42; break; } while (1); return x; }"),
        42
    );
}

// ============================================================
// Combined feature tests
// ============================================================

#[test]
fn test_bitwise_compound_combined() {
    // Use bitwise ops with compound assignment
    assert_eq!(
        run("int main() { int a = 0xFF; int b = 0x0F; int c = a & b; c <<= 2; return c + 2; }"),
        62
    );
    // 0xFF & 0x0F = 0x0F = 15; 15 << 2 = 60; 60 + 2 = 62
}

#[test]
fn test_hex_with_arithmetic() {
    assert_eq!(
        run("int main() { int x = 0x20; int y = 0x0A; return x + y; }"),
        42
    );
}

// ============================================================
// Edge cases
// ============================================================

#[test]
fn test_empty_return() {
    assert_eq!(run("int main() { return; }"), 0);
}

#[test]
fn test_implicit_return_zero() {
    assert_eq!(run("int main() { int x = 42; }"), 0);
}

#[test]
fn test_parenthesized_expr() {
    assert_eq!(run("int main() { return (42); }"), 42);
}

#[test]
fn test_parenthesized_complex() {
    assert_eq!(run("int main() { return (10 + 4) * 3; }"), 42);
}

// ============================================================
// Function calls (AST-level inlining)
// ============================================================

#[test]
fn test_call_simple() {
    assert_eq!(
        run("int add(int a, int b) { return a + b; } int main() { return add(40, 2); }"),
        42
    );
}

#[test]
fn test_call_zero_args() {
    assert_eq!(
        run("int answer() { return 42; } int main() { return answer(); }"),
        42
    );
}

#[test]
fn test_call_expression_arg() {
    assert_eq!(
        run("int square(int x) { return x * x; } int main() { return square(6 + 1); }"),
        49
    );
}

#[test]
fn test_call_multiple_args() {
    assert_eq!(
        run(
            "int sum3(int a, int b, int c) { return a + b + c; } int main() { return sum3(10, 20, 12); }"
        ),
        42
    );
}

#[test]
fn test_call_return_value_used() {
    assert_eq!(
        run("int twice(int x) { return x * 2; } int main() { int r = twice(21); return r; }"),
        42
    );
}

#[test]
fn test_call_single_func_declared_first() {
    // A single helper function used once
    assert_eq!(
        run("int helper() { return 42; } int main() { return helper(); }"),
        42
    );
}

#[test]
fn test_call_nested() {
    // add(add(10, 20), 12) → 30 + 12 = 42
    assert_eq!(
        run("int add(int a, int b) { return a + b; } int main() { return add(add(10, 20), 12); }"),
        42
    );
}

#[test]
fn test_call_three_funcs() {
    // Three functions calling each other
    assert_eq!(
        run(
            "int one() { return 10; } int two() { return 20; } int main() { int x = one(); int y = two(); return x + y + 12; }"
        ),
        42
    );
}

#[test]
fn test_call_in_expression() {
    // Call result used directly in arithmetic
    assert_eq!(
        run("int twice(int x) { return x * 2; } int main() { return twice(20) + 2; }"),
        42
    );
}

// ============================================================
// Enum tests
// ============================================================

#[test]
fn test_enum_basic() {
    assert_eq!(
        run("int main() { enum Color { RED, GREEN, BLUE }; return GREEN; }"),
        1
    );
}

#[test]
fn test_enum_expression() {
    // Auto-increment: A=0, B=1, C=2
    assert_eq!(
        run("int main() { enum Color { RED, GREEN, BLUE }; int x = RED + BLUE; return x + 40; }"),
        42
    );
}

#[test]
fn test_enum_explicit_values() {
    // Explicit values: RED=10, GREEN=20, BLUE=30
    assert_eq!(
        run("int main() { enum Color { RED=10, GREEN=20, BLUE=30 }; return GREEN; }"),
        20
    );
}

#[test]
fn test_enum_mixed_values() {
    // Mixed: A=5 (explicit), B (auto=6), C=100 (explicit), D (auto=101)
    assert_eq!(
        run("int main() { enum E { A=5, B, C=100, D }; int x = A + B + C + D; return x; }"),
        212 // 5 + 6 + 100 + 101
    );
}

#[test]
fn test_enum_hex_value() {
    assert_eq!(run("int main() { enum E { X=0x2A }; return X; }"), 42);
}

// ============================================================
// Struct tests
// ============================================================

#[test]
fn test_struct_field_read() {
    // Declare inside main, init, read field x
    assert_eq!(
        run(
            "int main() { struct Point { int x; int y; }; struct Point p = {10, 20}; return p.x; }"
        ),
        10
    );
}

#[test]
fn test_struct_field_read_second() {
    // Read second field
    assert_eq!(
        run(
            "int main() { struct Point { int x; int y; }; struct Point p = {10, 42}; return p.y; }"
        ),
        42
    );
}

#[test]
fn test_struct_field_write() {
    // Modify a field after init
    assert_eq!(
        run(
            "int main() { struct Point { int x; int y; }; struct Point p = {0, 0}; p.x = 42; return p.x; }"
        ),
        42
    );
}

#[test]
fn test_struct_field_write_preserves_other() {
    // Write one field, verify other is unchanged
    assert_eq!(
        run(
            "int main() { struct Point { int x; int y; }; struct Point p = {10, 20}; p.x = 99; return p.y; }"
        ),
        20
    );
}

#[test]
fn test_struct_init_list() {
    // Init with 3 values
    assert_eq!(
        run(
            "int main() { struct Triple { int a; int b; int c; }; struct Triple t = {10, 20, 12}; return t.a + t.b + t.c; }"
        ),
        42
    );
}

#[test]
fn test_struct_nested_expr() {
    // Field access in expression
    assert_eq!(
        run(
            "int main() { struct Point { int x; int y; }; struct Point p = {20, 22}; return p.x + p.y; }"
        ),
        42
    );
}

#[test]
fn test_struct_in_loop() {
    // Struct field updated in for loop
    assert_eq!(
        run(
            "int main() { struct Acc { int val; }; struct Acc a = {0}; for (int i = 0; i < 10; i = i + 1) { a.val = a.val + i; } return a.val; }"
        ),
        45 // 0+1+2+...+9 = 45
    );
}

#[test]
fn test_multi_struct() {
    // Multiple struct types don't interfere
    assert_eq!(
        run(
            "int main() { struct A { int v; }; struct B { int w; }; struct A a = {10}; struct B b = {32}; return a.v + b.w; }"
        ),
        42
    );
}

#[test]
fn test_struct_and_enum() {
    // Struct + enum combined
    assert_eq!(
        run(
            "int main() { enum Dir { N, E=10, S, W=20 }; struct Point { int x; int y; }; struct Point p = {E, S}; return p.x + p.y; }"
        ),
        21 // E=10, S=11 (auto-increment from E)
    );
}

#[test]
fn test_struct_decl_then_init() {
    // Separate decl and field writes
    assert_eq!(
        run(
            "int main() { struct Pair { int a; int b; }; struct Pair p; p.a = 30; p.b = 12; return p.a + p.b; }"
        ),
        42
    );
}
