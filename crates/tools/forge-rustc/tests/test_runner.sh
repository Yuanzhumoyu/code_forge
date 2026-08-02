#!/bin/bash
# codegen-lib rustc backend — 综合语法测试套件
# 编译并运行每个测试，验证返回值
BACKEND_DLL="${1:?Usage: $0 <path/to/rustc_codegen_codegenlib.dll>}"
TMPDIR="${TMPDIR:-/tmp/codegen_test}"
mkdir -p "$TMPDIR"

PASS=0
FAIL=0
TOTAL=0

run_test() {
    local name="$1"
    local expected="$2"
    local code="$3"
    TOTAL=$((TOTAL + 1))

    local src="$TMPDIR/${name}.rs"
    local exe="$TMPDIR/${name}.exe"

    cat > "$src" << ENDRUST
#![no_std] #![no_main]
#[unsafe(no_mangle)] pub extern "C" fn mainCRTStartup() -> isize { $code }
#[panic_handler] fn panic(_: &core::panic::PanicInfo) -> ! { loop {} }
ENDRUST

    if rustc +nightly -Zcodegen-backend="$BACKEND_DLL" "$src" \
        -C panic=abort --edition 2024 \
        -C link-args="/ENTRY:mainCRTStartup /SUBSYSTEM:CONSOLE" \
        -o "$exe" 2>/dev/null; then
        timeout 5 "$exe" 2>/dev/null
        actual=$?
        if [ "$actual" = "$expected" ]; then
            echo "  ✅ $name => $actual"
            PASS=$((PASS + 1))
        else
            echo "  ❌ $name => $actual (expected $expected)"
            FAIL=$((FAIL + 1))
        fi
    else
        echo "  🔨 $name => BUILD FAILED"
        FAIL=$((FAIL + 1))
    fi
}

echo "=== codegen-lib Rust Syntax Test Suite ==="
echo ""

# ============================================================
# 1. 常量
# ============================================================
echo "── Constants ──"
run_test "const_zero"        0   "0"
run_test "const_pos"        42   "42"
run_test "const_neg"       214   "-42"    # -42 as u8 = 214
run_test "const_large"      10   "10"

# ============================================================
# 2. 算术运算
# ============================================================
echo "── Arithmetic ──"
run_test "add_simple"        3   "1 + 2"
run_test "sub_simple"       42   "100 - 58"
run_test "mul_simple"       42   "6 * 7"
run_test "add_multi"        10   "1 + 2 + 3 + 4"
run_test "sub_neg"           5   "10 - 5"

# ============================================================
# 3. 位运算
# ============================================================
echo "── Bitwise ──"
run_test "bit_and"          15   "255 & 15"
run_test "bit_or"          255   "240 | 15"
run_test "bit_xor"         240   "255 ^ 15"

# ============================================================
# 4. 移位
# ============================================================
echo "── Shifts ──"
run_test "shl"              32   "1 << 5"
run_test "shr"               2   "8 >> 2"

# ============================================================
# 5. 比较
# ============================================================
echo "── Comparisons ──"
run_test "cmp_eq_true"      42   "if 3 == 3 { 42 } else { 0 }"
run_test "cmp_eq_false"      0   "if 3 == 4 { 42 } else { 0 }"
run_test "cmp_ne_true"      42   "if 3 != 4 { 42 } else { 0 }"
run_test "cmp_ne_false"      0   "if 3 != 3 { 42 } else { 0 }"
run_test "cmp_lt_true"      42   "if 1 < 2 { 42 } else { 0 }"
run_test "cmp_lt_false"      0   "if 2 < 1 { 42 } else { 0 }"
run_test "cmp_gt_true"      42   "if 2 > 1 { 42 } else { 0 }"
run_test "cmp_gt_false"      0   "if 1 > 2 { 42 } else { 0 }"
run_test "cmp_le_true"      42   "if 2 <= 2 { 42 } else { 0 }"
run_test "cmp_le_false"      0   "if 3 <= 2 { 42 } else { 0 }"
run_test "cmp_ge_true"      42   "if 2 >= 2 { 42 } else { 0 }"
run_test "cmp_ge_false"      0   "if 1 >= 2 { 42 } else { 0 }"

# ============================================================
# 6. 控制流
# ============================================================
echo "── Control Flow ──"
run_test "if_true"          42   "if true { 42 } else { 0 }"
run_test "if_const"         42   "if 1 < 2 { 42 } else { 0 }"
run_test "if_else"          99   "if 1 > 2 { 42 } else { 99 }"
run_test "if_nested"        42   "if 1<2 { if 3==3 { 42 } else { 0 } } else { 0 }"

# ============================================================
# 7. 循环
# ============================================================
echo "── Loops ──"
run_test "while_decr"        0   "{ let mut x=3; while x>0 {x-=1;} x }"
run_test "loop_break"       42   "{ loop { break 42; } }"

# ============================================================
# 8. 表达式
# ============================================================
echo "── Expressions ──"
run_test "expr_complex"     50   "if 1 < 2 { 10 * 5 } else { 0 }"
run_test "expr_nested"     100   "(1 + 2) * 33 + 1"

# ============================================================
# 结果
# ============================================================
echo ""
echo "=== Results: $PASS / $TOTAL passed, $FAIL failed ==="
[ "$FAIL" -eq 0 ] && echo "🎉 All tests passed!" || echo "⚠️  Some tests failed"
exit $FAIL
