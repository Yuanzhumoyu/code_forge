#!/bin/bash
# codegen-lib rustc backend — 快速测试脚本
BACKEND_DLL="${1:?Usage: $0 <path/to/rustc_codegen_codegenlib.dll>}"
TMPDIR="/tmp/cg_tests"
mkdir -p "$TMPDIR"
P=0; F=0; S=0

test_one() {
    local name="$1" expected="$2"
    shift 2
    local code="$@"
    local src="$TMPDIR/${name}.rs"
    local exe="$TMPDIR/${name}.exe"
    cat > "$src" << ENDRUST
#![no_std] #![no_main]
#[unsafe(no_mangle)] pub extern "C" fn mainCRTStartup() -> isize { $code }
#[panic_handler] fn panic(_: &core::panic::PanicInfo) -> ! { loop {} }
ENDRUST
    if rustc +nightly -Zcodegen-backend="$BACKEND_DLL" "$src" -C panic=abort --edition 2024 -C link-args="/ENTRY:mainCRTStartup /SUBSYSTEM:CONSOLE" -o "$exe" 2>/dev/null; then
        timeout 3 "$exe" 2>/dev/null
        local actual=$?
        if [ "$actual" = "$expected" ]; then
            echo "  ✅ $name"; P=$((P+1))
        else
            echo "  ❌ $name => $actual (expected $expected)"; F=$((F+1))
        fi
    else
        echo "  🔨 $name BUILD FAIL"; F=$((F+1))
    fi
}

echo "=== codegen-lib Rust Syntax Tests ==="

# ── Constants ──
echo "── Constants"
test_one const0 0 "0"
test_one const42 42 "42"
test_one const10 10 "10"

# ── Arithmetic ──
echo "── Arithmetic"
test_one add 3 "1+2"
test_one sub 42 "100-58"
test_one mul 42 "6*7"
test_one neg 5 "-5+10"

# ── Bitwise ──
echo "── Bitwise"
test_one and 15 "255&15"
test_one or 255 "240|15"
test_one xor 240 "255^15"

# ── Shifts ──
echo "── Shifts"
test_one shl 32 "1<<5"
test_one shr 2 "8>>2"

# ── Comparisons ──
echo "── Comparisons"
test_one eq1 42  "if 3==3{42}else{0}"
test_one eq2 0   "if 3==4{42}else{0}"
test_one ne1 42  "if 3!=4{42}else{0}"
test_one ne2 0   "if 3!=3{42}else{0}"
test_one lt1 42  "if 1<2{42}else{0}"
test_one lt2 0   "if 2<1{42}else{0}"
test_one gt1 42  "if 2>1{42}else{0}"
test_one gt2 0   "if 1>2{42}else{0}"
test_one le1 42  "if 2<=2{42}else{0}"
test_one le2 0   "if 3<=2{42}else{0}"
test_one ge1 42  "if 2>=2{42}else{0}"
test_one ge2 0   "if 1>=2{42}else{0}"

# ── If/Else ──
echo "── Control Flow"
test_one if_t  42  "if 1<2{42}else{0}"
test_one if_f  99  "if 1>2{42}else{99}"
test_one nest  42  "if 1<2{if 3==3{42}else{0}}else{0}"

# ── Loops ──
echo "── Loops"
test_one while_dec 0   "{let mut x=3;while x>0{x-=1;}x}"
test_one loop_brk 42  "{loop{break 42;}}"

# ── Complex ──
echo "── Complex"
test_one expr1 33  "(1+2)*11"
test_one expr2 50  "if 1<2{10*5}else{0}"
test_one expr3 100 "if 2>1{if 3==3{100}else{0}}else{0}"

echo ""
echo "=== Passed: $P | Failed: $F | Total: $((P+F)) ==="
exit $F
