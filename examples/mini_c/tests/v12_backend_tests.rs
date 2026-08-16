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
