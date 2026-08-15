//! Dual-backend consistency tests.
//!
//! Every test compiles the SAME source through both backends — the direct
//! FunctionBuilder codegen (`codegen.rs`) and the forge-hir pipeline
//! (`codegen_hir.rs`) — and asserts their JIT results agree with the
//! expected value. This is the regression net that lets us migrate from
//! the direct backend to HIR.
//!
//! Notes:
//! - 历史排除项已全部转正（2026-08 P2 验证）：循环内条件分支、真条件
//!   while/do-while、do-while 双栈变量——Direct/Hir 双后端现均正确（x86
//!   `lower_term.Branch` 编码错误与 do-while 单迭代退出 bug 已随 break SEGV
//!   根治（b87f0b7）与后续修复消除），见本文件 `both_loop_cond_branch` 等测试。
//! - Division/modulo is covered (the x86 `Sdiv`/`Srem` lowering was fixed:
//!   isa/x86_v10.toml — `movsxd` the dividend, `cqo` after the mov, divisor
//!   staged in `R11`).

use mini_c::compiler::{Backend, compile_and_run, compile_and_run_with};

/// Run `source` through both backends, returning (direct, hir).
fn run_both(source: &str) -> (i32, i32) {
    let direct = compile_and_run(source).expect("direct backend failed");
    let hir = compile_and_run_with(source, Backend::Hir).expect("hir backend failed");
    (direct, hir)
}

fn assert_both(source: &str, expected: i32) {
    let (d, h) = run_both(source);
    assert_eq!(
        (d, h),
        (expected, expected),
        "backends disagree on: {}",
        source
    );
}

// ── Constants & arithmetic ──

#[test]
fn both_constant() {
    assert_both("int main() { return 42; }", 42);
}

#[test]
fn both_precedence() {
    assert_both("int main() { return 2 + 3 * 4; }", 14);
    assert_both("int main() { return (2 + 3) * 4; }", 20);
}

// ── Locals, assignment, compound assignment ──

#[test]
fn both_locals() {
    assert_both("int main() { int x = 5; int y = x + 1; return y; }", 6);
    assert_both("int main() { int x = 5; x = x * 2; return x; }", 10);
}

#[test]
fn both_division() {
    assert_both("int main() { return 20 / 4; }", 5);
    assert_both("int main() { return 17 / 5; }", 3);
    assert_both("int main() { return -20 / 4; }", -5);
    assert_both("int main() { return 20 % 7; }", 6);
    assert_both("int main() { int x = 20; x /= 4; return x; }", 5);
    assert_both("int main() { int x = -45; x %= 43; return x; }", -2);
}

#[test]
fn both_compound_assign() {
    assert_both("int main() { int x = 5; x += 3; return x; }", 8);
    assert_both("int main() { int x = 5; x -= 3; return x; }", 2);
    assert_both("int main() { int x = 5; x *= 4; return x; }", 20);
    assert_both("int main() { int x = 6; x &= 3; return x; }", 2);
    assert_both("int main() { int x = 5; x |= 2; return x; }", 7);
    assert_both("int main() { int x = 5; x ^= 3; return x; }", 6);
    assert_both("int main() { int x = 1; x <<= 4; return x; }", 16);
    assert_both("int main() { int x = 16; x >>= 2; return x; }", 4);
}

// ── Function-level if/else (branching outside loops) ──

#[test]
fn both_if_else() {
    assert_both(
        "int main() { if (1) { return 10; } else { return 20; } }",
        10,
    );
    assert_both(
        "int main() { if (0) { return 10; } else { return 20; } }",
        20,
    );
    assert_both(
        "int main() { int x = 3; if (x > 2) { return 1; } return 0; }",
        1,
    );
    assert_both(
        "int main() { int i = 5; if (i == 5) { return 99; } return 0; }",
        99,
    );
}

// ── Loops (no conditional branch inside the body) ──

#[test]
fn both_for_loop() {
    assert_both(
        "int main() { int s = 0; for (int i = 1; i <= 10; i = i + 1) { s = s + i; } return s; }",
        55,
    );
}

#[test]
fn both_while_loop_sum() {
    assert_both(
        "int main() { int s = 0; int i = 1; while (i <= 10) { s = s + i; i = i + 1; } return s; }",
        55,
    );
    assert_both(
        "int main() { int s = 0; int i = 1; while (i < 10) { s = s + i; i = i + 1; } return s; }",
        45,
    );
    assert_both(
        "int main() { int i = 0; while (i < 5) { i = i + 1; } return i; }",
        5,
    );
}

#[test]
fn both_unconditional_break() {
    assert_both(
        "int main() { int x = 0; for (int i = 0; i < 100; i = i + 1) { x = 42; break; } return x; }",
        42,
    );
    assert_both(
        "int main() { int x = 0; while (1) { x = 42; break; } return x; }",
        42,
    );
}

// ── Bitwise / logical / unary / literals ──

#[test]
fn both_bitwise() {
    assert_both("int main() { return 6 & 3; }", 2);
    assert_both("int main() { return 5 | 2; }", 7);
    assert_both("int main() { return 5 ^ 3; }", 6);
    assert_both("int main() { return 1 << 4; }", 16);
    assert_both("int main() { return 16 >> 2; }", 4);
    assert_both("int main() { return ~0; }", -1);
}

#[test]
fn both_logical() {
    assert_both("int main() { return 1 && 1; }", 1);
    assert_both("int main() { return 1 && 0; }", 0);
    assert_both("int main() { return 0 || 1; }", 1);
    assert_both("int main() { return (1 && 0) || 1; }", 1);
}

#[test]
fn both_unary() {
    assert_both("int main() { return -5; }", -5);
    assert_both("int main() { return !0; }", 1);
    assert_both("int main() { return !1; }", 0);
}

#[test]
fn both_literals() {
    assert_both("int main() { return 'A'; }", 65);
    assert_both("int main() { return 0x10; }", 16);
    assert_both("int main() { return 0XFF; }", 255);
    assert_both("int main() { return 010; }", 8);
    assert_both("int main() { return 0x10 + 010; }", 24);
}

// ── Function calls (AST-level inlining) ──

#[test]
fn both_calls() {
    assert_both(
        "int add(int a, int b) { return a + b; } int main() { return add(2, 3); }",
        5,
    );
    assert_both(
        "int add(int a, int b) { return a + b; } int main() { return add(add(10, 20), 12); }",
        42,
    );
    assert_both("int zero() { return 0; } int main() { return zero(); }", 0);
}

// ── Enum constants ──

#[test]
fn both_enums() {
    assert_both("int main() { enum E { A, B, C }; return B; }", 1);
    assert_both("int main() { enum E { A = 5, B, C }; return C; }", 7);
}

// ── Structs (per-field slots) ──

#[test]
fn both_structs() {
    assert_both(
        "int main() { struct P { int x; int y; }; struct P p; p.x = 3; p.y = 4; return p.x + p.y; }",
        7,
    );
    assert_both(
        "int main() { struct P { int x; int y; }; struct P p = {10, 20}; p.x = 99; return p.y; }",
        20,
    );
}

// ── 历史排除项转正（P2 验证：Direct/Hir 双后端均正确）──

#[test]
fn both_loop_cond_branch() {
    // 循环内条件分支（曾排除：x86 lower_term.Branch 编码错误）
    assert_both(
        "int main() { int s = 0; for (int i = 0; i < 10; i = i + 1) { if (i == 5) { s = s + 100; } s = s + 1; } return s; }",
        110,
    );
    assert_both(
        "int main() { int x = 0; for (int i = 0; i < 100; i = i + 1) { if (i == 3) { break; } x = x + 1; } return x; }",
        3,
    );
}

#[test]
fn both_while_real_cond() {
    // 真条件 while（头部注释曾自相矛盾：一说覆盖、一说排除）
    assert_both(
        "int main() { int s = 0; int i = 1; while (i <= 10) { s = s + i; i = i + 1; } return s; }",
        55,
    );
}

#[test]
fn both_dowhile_two_locals() {
    // do-while 双栈变量（曾排除：Direct 后端单迭代退出）
    assert_both(
        "int main() { int s = 0; int i = 1; do { int t = s + i; s = t; i = i + 1; } while (i <= 10); return s; }",
        55,
    );
}
