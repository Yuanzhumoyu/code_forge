//! mini_c Direct 后端测试（原 v12_backend_tests——Backend::V12 曾是与
//! Direct 相同的代码路径的假对比，P0-19 已删除；测试保留期望值断言）。
//!
//! 验证 mini_c 子集（Iconst/Iadd/Isub/Band/Bor/Bxor/Imul + Return）经
//! x86_v12 TargetMachine JIT 执行结果正确。
#![cfg(target_arch = "x86_64")]

use mini_c::compiler::{Backend, compile_and_run, compile_and_run_with};

/// Direct 后端运行。
fn run_direct(source: &str) -> i32 {
    compile_and_run_with(source, Backend::Direct)
        .unwrap_or_else(|e| panic!("direct backend failed for `{source}`: {e}"))
}

fn assert_direct_matches_expected(source: &str, expected: i32) {
    let v11 = compile_and_run(source).expect("compile_and_run failed");
    let direct = run_direct(source);
    assert_eq!(
        direct, expected,
        "direct backend wrong for `{source}` (expected {expected})"
    );
    assert_eq!(
        v11, expected,
        "v11 backend regression for `{source}` (expected {expected})"
    );
}

#[test]
fn v12_constant() {
    assert_direct_matches_expected("int main() { return 42; }", 42);
    assert_direct_matches_expected("int main() { return -1; }", -1);
    assert_direct_matches_expected("int main() { return 0; }", 0);
}

#[test]
fn v12_add() {
    assert_direct_matches_expected("int main() { return 2 + 3; }", 5);
    assert_direct_matches_expected("int main() { return 2 + 3 + 4; }", 9);
}

#[test]
fn v12_mixed_arith() {
    assert_direct_matches_expected("int main() { return 2 + 3 * 4; }", 14);
    assert_direct_matches_expected("int main() { return 10 - 3; }", 7);
    assert_direct_matches_expected("int main() { return 7 & 3; }", 3);
    assert_direct_matches_expected("int main() { return 5 | 2; }", 7);
    assert_direct_matches_expected("int main() { return 6 ^ 3; }", 5);
}

#[test]
fn v12_const_arith_chain() {
    // 多步：多个 Iconst + 多个算术，寄存器压力更高
    assert_direct_matches_expected("int main() { return 1 + 2 + 3 + 4 + 5; }", 15);
    assert_direct_matches_expected("int main() { return 100 - 20 - 30; }", 50);
}

#[test]
fn v12_bitwise_not() {
    assert_direct_matches_expected("int main() { int x = 5; return ~x; }", !5);
    assert_direct_matches_expected("int main() { int x = 0; return ~x; }", !0);
}

#[test]
fn v12_local_load_store() {
    // 局部变量：store 到栈槽 + load 回来
    assert_direct_matches_expected("int main() { int x = 7; return x; }", 7);
    assert_direct_matches_expected("int main() { int x = 3; int y = 4; return x + y; }", 7);
}

#[test]
fn v12_compound_assign() {
    assert_direct_matches_expected("int main() { int x = 5; x += 3; return x; }", 8);
    assert_direct_matches_expected("int main() { int x = 10; x -= 4; return x; }", 6);
    assert_direct_matches_expected("int main() { int x = 6; x &= 3; return x; }", 2);
}

#[test]
fn v12_if_else() {
    // 条件分支：Branch terminator（test + je + jmp）
    assert_direct_matches_expected(
        "int main() { int x = 5; if (x > 3) { return 1; } else { return 0; } }",
        1,
    );
    assert_direct_matches_expected(
        "int main() { int x = 1; if (x > 3) { return 1; } else { return 0; } }",
        0,
    );
    assert_direct_matches_expected(
        "int main() { int x = 3; if (x > 2) { return 1; } return 0; }",
        1,
    );
    assert_direct_matches_expected(
        "int main() { int x = 0; if (x == 0) { return 7; } return 0; }",
        7,
    );
}

#[test]
fn v12_ternary_cond() {
    // 条件表达式 + sextend + icmp
    assert_direct_matches_expected(
        "int main() { int x = 7; if (x != 0) { return x; } else { return -1; } }",
        7,
    );
}

#[test]
fn v12_icmp_result_direct() {
    // icmp 结果直接返回（不经 Branch），隔离 setcc 正确性
    assert_direct_matches_expected("int main() { int x = 5; int y = (x > 3); return y; }", 1);
    assert_direct_matches_expected("int main() { int x = 1; int y = (x > 3); return y; }", 0);
    assert_direct_matches_expected("int main() { int x = 3; int y = (x > 3); return y; }", 0);
    assert_direct_matches_expected("int main() { int x = 5; int y = (x == 5); return y; }", 1);
    assert_direct_matches_expected("int main() { int x = 4; int y = (x == 5); return y; }", 0);
}

#[test]
fn v12_while_loop() {
    // while 循环：icmp + Branch + 变量更新
    assert_direct_matches_expected(
        "int main() { int i = 0; while (i < 5) { i = i + 1; } return i; }",
        5,
    );
    assert_direct_matches_expected(
        "int main() { int s = 0; int i = 1; while (i <= 10) { s = s + i; i = i + 1; } return s; }",
        55,
    );
}

#[test]
fn v12_for_loop() {
    // for 循环（init/cond/update 三部分）
    assert_direct_matches_expected(
        "int main() { int s = 0; for (int i = 1; i <= 10; i = i + 1) { s = s + i; } return s; }",
        55,
    );
}

#[test]
fn v12_nested_loop() {
    // 嵌套循环：双重 while（修复：mini_c stack_addr(0)+iadd 模式的槽深计入
    // max_stack_bytes，spill 槽不再覆盖内层作用域局部变量）
    assert_direct_matches_expected(
        "int main() { int s = 0; int i = 0; while (i < 3) { int j = 0; while (j < 3) { s = s + 1; j = j + 1; } i = i + 1; } return s; }",
        9,
    );
    // 外层累加 + 内层独立计数（内层不触碰 s）
    assert_direct_matches_expected(
        "int main() { int s = 0; int i = 0; while (i < 3) { int j = 0; while (j < 2) { j = j + 1; } s = s + 1; i = i + 1; } return s; }",
        3,
    );
    // 深层嵌套（3 层）+ 内层累加
    assert_direct_matches_expected(
        "int main() { int s = 0; int i = 0; while (i < 2) { int j = 0; while (j < 2) { int k = 0; while (k < 2) { s = s + 1; k = k + 1; } j = j + 1; } i = i + 1; } return s; }",
        8,
    );
}

#[test]
fn v12_division() {
    // 除法：Sdiv/Srem（movsxd RAX + cqo + idiv + 物理寄存器序列）
    assert_direct_matches_expected("int main() { return 10 / 3; }", 3);
    assert_direct_matches_expected("int main() { return 10 % 3; }", 1);
    assert_direct_matches_expected("int main() { return -10 / 3; }", -3);
    assert_direct_matches_expected("int main() { return -10 % 3; }", -1);
    assert_direct_matches_expected("int main() { int x = 100; int y = 7; return x / y; }", 14);
    assert_direct_matches_expected("int main() { int x = 100; int y = 7; return x % y; }", 2);
    // 回归：除法在循环内（除数 {t} 曾被 cqo 隐式写 RDX 污染 → 除零崩溃；
    // CQO/IDIV/DIV 声明 implicit_regs 后 regalloc 避开 RDX）
    assert_direct_matches_expected(
        "int main() { int x = 100; while (x > 0) { x = x / 2; } return x; }",
        0,
    );
    assert_direct_matches_expected(
        "int main() { int s = 0; for (int i = 1; i <= 5; i = i + 1) { if (i % 2 == 0) { s = s + i; } } return s; }",
        6,
    );
    assert_direct_matches_expected(
        "int main() { int x = 7; int y = 3; return (x / y) + (x % y) + (x << 1) - (y >> 1); }",
        16,
    );
}

#[test]
fn v12_shift() {
    // 移位：Ishl/Ushr/Sshr（计数搬 RCX → D3 /digit；CL 隐式计数）
    assert_direct_matches_expected("int main() { return 1 << 4; }", 16);
    assert_direct_matches_expected("int main() { return 1 << 10; }", 1024);
    assert_direct_matches_expected("int main() { return 32 >> 2; }", 8);
    assert_direct_matches_expected("int main() { int x = 256; return x >> 4; }", 16);
    assert_direct_matches_expected("int main() { return -16 >> 2; }", -4);
    assert_direct_matches_expected("int main() { return 0x80 >> 4; }", 8);
    // 变量计数
    assert_direct_matches_expected("int main() { int x = 1; int n = 5; return x << n; }", 32);
    // 复合赋值
    assert_direct_matches_expected("int main() { int x = 8; x <<= 2; return x; }", 32);
    assert_direct_matches_expected("int main() { int x = 32; x >>= 3; return x; }", 4);
    // 回归：shift 结果赋局部变量 + 循环（RCX clobber 点 use 占用者曾被错误
    // spill 致崩溃——spill_vreg 不 store 当前值，reload 读空槽垃圾）
    assert_direct_matches_expected(
        "int main() { int v = 1; int i = 0; while (i < 4) { int s = v << i; i = i + 1; } return 7; }",
        7,
    );
    assert_direct_matches_expected(
        "int main() { int v = 1; int t = 0; int i = 0; while (i < 4) { int s = v << i; i = i + 1; } return t; }",
        0,
    );
    // 回归：t += shift 结果 + 循环（spill scratch R10/R11 曾可被 regalloc 分配，
    // emission 的 spill load 用 scratch 覆盖活跃值 → 崩溃；现已排除 scratch）
    assert_direct_matches_expected(
        "int main() { int v = 1; int t = 0; int i = 0; while (i < 4) { t += v << i; i = i + 1; } return t; }",
        15,
    );
    assert_direct_matches_expected(
        "int main() { int v = 1; int t = 0; int i = 0; while (i < 4) { int s = v << i; t += s; i = i + 1; } return t; }",
        15,
    );
    assert_direct_matches_expected(
        "int main() { int v = 1; int t = 0; int i = 0; while (i < 4) { int s = v << i; t = t + 1; i = i + 1; } return t; }",
        4,
    );
    assert_direct_matches_expected(
        "int main() { struct S { int v; }; struct S s = {1}; int t = 0; int i = 0; while (i < 4) { t += s.v << i; i = i + 1; } return t; }",
        15,
    );
}

#[test]
fn v12_inlined_call() {
    // AST 内联调用（mini_c 的 Call 在 AST 层内联，不生成 CALL 指令）
    assert_direct_matches_expected(
        "int add(int a, int b) { return a + b; } int main() { return add(2, 3); }",
        5,
    );
    assert_direct_matches_expected(
        "int add(int a, int b) { return a + b; } int main() { return add(add(10, 20), 12); }",
        42,
    );
    // 调用组合：多参/多函数链/表达式参数/嵌套调用
    assert_direct_matches_expected(
        "int add(int a, int b) { return a + b; } int main() { return add(40, 2); }",
        42,
    );
    assert_direct_matches_expected(
        "int answer() { return 42; } int main() { return answer(); }",
        42,
    );
    assert_direct_matches_expected(
        "int square(int x) { return x * x; } int main() { return square(6 + 1); }",
        49,
    );
    assert_direct_matches_expected(
        "int sum3(int a, int b, int c) { return a + b + c; } int main() { return sum3(10, 20, 12); }",
        42,
    );
    assert_direct_matches_expected(
        "int f(int x) { return x + 1; } int g(int x) { return f(x) + 1; } int h(int x) { return g(x) + 1; } int main() { return h(10); }",
        13,
    );
    assert_direct_matches_expected(
        "int add(int a, int b) { return a + b; } int main() { int x = 10; return add(x, add(x, x)); }",
        30,
    );
    assert_direct_matches_expected(
        "int f(int x) { return x - 1; } int main() { int r = f(10); return r + f(r); }",
        17,
    );
    assert_direct_matches_expected(
        "int neg(int x) { return 0 - x; } int main() { return neg(7); }",
        -7,
    );
}

#[test]
fn v12_unary_logical() {
    // 单目（-5、!x）+ 逻辑（icmp + sextend）
    assert_direct_matches_expected("int main() { return -5; }", -5);
    assert_direct_matches_expected("int main() { int x = 0; return !x; }", 1);
    assert_direct_matches_expected("int main() { int x = 1; return !x; }", 0);
    assert_direct_matches_expected("int main() { int x = 7; int y = (x != 0); return y; }", 1);
}

#[test]
fn v12_logical_and_or() {
    // && / ||：icmp(NotEqual) + band/bor + sextend（非短路求值）
    assert_direct_matches_expected("int main() { return 1 && 1; }", 1);
    assert_direct_matches_expected("int main() { return 1 && 0; }", 0);
    assert_direct_matches_expected("int main() { return 0 && 1; }", 0);
    assert_direct_matches_expected("int main() { return 0 || 1; }", 1);
    assert_direct_matches_expected("int main() { return 0 || 0; }", 0);
    assert_direct_matches_expected(
        "int main() { int x = 3; int y = 5; return (x < y) && (y > 2); }",
        1,
    );
    assert_direct_matches_expected(
        "int main() { int x = 3; int y = 5; return (x > y) || (y == 5); }",
        1,
    );
}

#[test]
fn v12_enum_const() {
    // 枚举常量（符号表解析为数字，需在函数体内声明）
    assert_direct_matches_expected(
        "int main() { enum Color { RED, GREEN, BLUE }; return BLUE; }",
        2,
    );
    assert_direct_matches_expected("int main() { enum X { A = 5, B }; return B; }", 6);
}

#[test]
fn v12_do_while() {
    // do-while：先执行后判断（cond 在 body 之后，jmp 后向）
    assert_direct_matches_expected(
        "int main() { int i = 0; do { i = i + 1; } while (i < 5); return i; }",
        5,
    );
    assert_direct_matches_expected(
        "int main() { int i = 10; do { i = i + 1; } while (i < 5); return i; }",
        11,
    );
}

#[test]
fn v12_break_continue() {
    // break 提前退出 + continue 跳过本次迭代（有界循环；continue 用 while——
    // mini_c 的 for-continue 会跳过 update 导致死循环，属前端既有语义）
    assert_direct_matches_expected(
        "int main() { int i = 0; while (i < 100) { i = i + 1; if (i >= 3) { break; } } return i; }",
        3,
    );
    assert_direct_matches_expected(
        "int main() { int s = 0; int i = 0; while (i < 5) { i = i + 1; if (i == 3) { continue; } s = s + i; } return s; }",
        12,
    );
}

#[test]
fn v12_struct_field() {
    // struct 定义 + 字段访问（struct_decl + member_access → load/store）
    assert_direct_matches_expected(
        "int main() { struct Point { int x; int y; }; struct Point p; p.x = 3; p.y = 4; return p.x + p.y; }",
        7,
    );
    assert_direct_matches_expected(
        "int main() { struct Point { int x; int y; }; struct Point p; p.x = 3; p.x = p.x + 2; return p.x; }",
        5,
    );
}

#[test]
fn v12_struct_init_list() {
    // struct 初始化列表 {v1, v2, ...}：按字段序 store
    assert_direct_matches_expected(
        "int main() { struct Point { int x; int y; }; struct Point p = {10, 20}; return p.x; }",
        10,
    );
    assert_direct_matches_expected(
        "int main() { struct Point { int x; int y; }; struct Point p = {10, 42}; return p.y; }",
        42,
    );
    assert_direct_matches_expected(
        "int main() { struct Point { int x; int y; }; struct Point p = {0, 0}; p.x = 42; return p.x; }",
        42,
    );
    assert_direct_matches_expected(
        "int main() { struct Point { int x; int y; }; struct Point p = {10, 20}; p.x = 99; return p.y; }",
        20,
    );
    assert_direct_matches_expected(
        "int main() { struct P { int a; int b; int c; }; struct P p = {1, 2, 3}; return p.a + p.b + p.c; }",
        6,
    );
    assert_direct_matches_expected(
        "int main() { struct P { int a; int b; }; struct P p = {7, 8}; struct P q = {3, 4}; return p.a + q.b; }",
        11,
    );
    assert_direct_matches_expected(
        "int main() { struct Dir { int n; int e; int s; int w; }; struct Dir d = {1, 2, 3, 4}; return d.n + d.e + d.s + d.w; }",
        10,
    );
}

#[test]
fn v12_literal_forms() {
    // hex / octal / char 字面量
    assert_direct_matches_expected("int main() { return 0x2A; }", 42);
    assert_direct_matches_expected("int main() { return 0x2a; }", 42);
    assert_direct_matches_expected("int main() { return 077; }", 63);
    assert_direct_matches_expected("int main() { return 'A'; }", 65);
    assert_direct_matches_expected("int main() { return 0x2A + 1; }", 43);
}

#[test]
fn v12_integration_style() {
    // 集成风格组合用例（多运算/循环/调用混合）
    assert_direct_matches_expected(
        "int main() { int a = 3; int b = 4; int c = 5; return a * b + c; }",
        17,
    );
    assert_direct_matches_expected(
        "int main() { int x = 5; x = x * 3; x = x + 2; return x; }",
        17,
    );
    assert_direct_matches_expected("int main() { int x = 8; return (x & 3) | (x >> 2); }", 2);
    assert_direct_matches_expected(
        "int main() { int s = 0; int i = 0; while (i < 5) { s = s + i * i; i = i + 1; } return s; }",
        30,
    );
    assert_direct_matches_expected(
        "int main() { int x = 0; if (x) { return 1; } else { return 2; } }",
        2,
    );
    assert_direct_matches_expected(
        "int main() { int x = 3; int y = 4; x += y; y *= 2; return x * y; }",
        56,
    );
    assert_direct_matches_expected(
        "int main() { int n = 6; int f = 1; while (n > 1) { f = f * n; n = n - 1; } return f; }",
        720,
    );
    assert_direct_matches_expected(
        "int max(int a, int b) { if (a > b) { return a; } return b; } int main() { return max(3, 7); }",
        7,
    );
    assert_direct_matches_expected(
        "int fib(int n) { return n; } int main() { return fib(5); }",
        5,
    );
}

#[test]
fn test_fibonacci() {
    assert_direct_matches_expected(
        "int fib(int n) { if(n <= 1){ return n; }  return fib(n-1) + fib(n-2); } int main() { return fib(5); }",
        5,
    );
}

#[test]
fn test_recursive_depth2() {
    // 深度 2 递归：f(2) = f(1) + 1 = 1
    assert_direct_matches_expected(
        "int f(int n) { if(n <= 1){ return n; }  return f(n-1) + 1; } int main() { return f(2); }",
        2,
    );
}

#[test]
fn test_recursive_fib3() {
    // fib(3) = fib(2) + fib(1) = 1 + 1 = 2
    assert_direct_matches_expected(
        "int fib(int n) { if(n <= 1){ return n; }  return fib(n-1) + fib(n-2); } int main() { return fib(3); }",
        2,
    );
}
