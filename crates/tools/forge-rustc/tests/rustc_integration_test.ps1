# forge-rustc 端到端集成测试
# 构建后端并测试：rustc -Zcodegen-backend=forge_rustc.dll 编译 no_std 程序，
# 运行产物并校验退出码（入口函数返回值 = 进程退出码）。
#
# 用法：
#   powershell -ExecutionPolicy Bypass -File tests\rustc_integration_test.ps1 [-Release] [-Verbose]
# 或直接：cargo test -p forge-rustc --test e2e

param(
    [switch]$Release,
    [switch]$Verbose
)

$ErrorActionPreference = "Stop"
$backend_dir = Split-Path -Parent $PSScriptRoot
# workspace 根（cargo workspace 共享 target 目录）
$workspace_root = (Resolve-Path "$backend_dir\..\..\..").Path
$profile = if ($Release) { "release" } else { "debug" }
$backend_dll = "$workspace_root\target\$profile\forge_rustc.dll"
$nightly = "nightly"

Write-Host "=== forge-rustc rustc backend — 端到端真值测试 ===" -ForegroundColor Cyan
Write-Host ""

# ━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━
# 1. 构建后端
# ━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━
Write-Host "[1/4] Building backend..." -ForegroundColor Yellow
Push-Location $backend_dir
try {
    $build_flag = if ($Release) { "--release" } else { "" }
    # PS 5.1 会把 native 命令的 stderr 输出当 ErrorRecord，配合
    # $ErrorActionPreference=Stop 会误抛 RemoteException——先降级再构建
    $ErrorActionPreference = "Continue"
    $build_out = & cargo +$nightly build $build_flag 2>&1
    $build_code = $LASTEXITCODE
    $ErrorActionPreference = "Stop"
    if ($build_code -ne 0) {
        throw "Backend build failed: $build_out"
    }
    Write-Host "  [OK] Backend built: $backend_dll" -ForegroundColor Green
} finally {
    Pop-Location
}

# ━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━
# 2. 测试用例表：{ name, body, expected_exit }
#    body 是 mainCRTStartup 的函数体（i32 表达式，可含辅助 fn）。
#    入口函数返回值 = 进程退出码（/ENTRY:mainCRTStartup）。
# ━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━
$tests = @(
    @{ Name = "ret_const";   Body = "42";                                      Expected = 42  },
    @{ Name = "add";         Body = "let a = 1i32; let b = 2i32; a + b";       Expected = 3   },
    @{ Name = "sub";         Body = "5i32 - 3";                                Expected = 2   },
    @{ Name = "mul";         Body = "3i32 * 4";                                Expected = 12  },
    @{ Name = "div";         Body = "10i32 / 3";                               Expected = 3   },
    @{ Name = "rem";         Body = "10i32 % 3";                               Expected = 1   },
    @{ Name = "conditional"; Body = "let x = 5i32; if x > 0 { 1 } else { -1 }"; Expected = 1  },
    @{ Name = "simple_loop"; Body = "let mut acc = 0i32; let mut i = 0i32; while i < 5 { acc += i; i += 1; } acc"; Expected = 10 },
    @{ Name = "loop_break";  Body = "let mut acc = 0i32; loop { acc += 1; if acc >= 3 { break; } } acc"; Expected = 3 },
    @{ Name = "bool_ops";    Body = "let a = true; let b = true; if (a && b) || (!a && !b) { 1 } else { 0 }"; Expected = 1 },
    @{ Name = "early_return"; Body = "let x = 5i32; if x < 0 { return 0; } x * 2"; Expected = 10 }
)

$rustc_flags = @(
    "-Zcodegen-backend=$backend_dll",
    "-C", "panic=abort",
    "-C", "overflow-checks=off",
    "--edition", "2024",
    "-C", "link-args=/ENTRY:mainCRTStartup /SUBSYSTEM:CONSOLE /DEFAULTLIB:kernel32.lib /DEFAULTLIB:vcruntime.lib"
)

$passed = 0
$failed = 0
$results = @()

# ━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━
# 3. 编译 → 运行 → 校验退出码
# ━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━
Write-Host "[2/4] Compiling + running test programs..." -ForegroundColor Yellow

$tmpdir = Join-Path $env:TEMP "forge_rustc_e2e_ps"
New-Item -ItemType Directory -Force -Path $tmpdir | Out-Null

foreach ($test in $tests) {
    $name = $test.Name
    $src = Join-Path $tmpdir "$name.rs"
    $out = Join-Path $tmpdir "$name.exe"
    $source = @"
#![no_std]
#![no_main]

#[unsafe(no_mangle)]
pub extern "C" fn mainCRTStartup() -> i32 {
    $($test.Body)
}

#[panic_handler]
fn panic(_info: &core::panic::PanicInfo) -> ! {
    loop {}
}
"@
    Set-Content -Path $src -Value $source -Encoding UTF8

    Write-Host "  Testing: $name..." -NoNewline

    $ErrorActionPreference = "Continue"
    $compile = & rustc +$nightly @rustc_flags $src -o $out 2>&1
    $compile_code = $LASTEXITCODE
    $ErrorActionPreference = "Stop"
    if ($compile_code -ne 0) {
        Write-Host " FAIL (compile)" -ForegroundColor Red
        $failed++
        $results += @{ Name = $name; Status = "FAIL(compile)" }
        if ($Verbose) { Write-Host "    $compile" -ForegroundColor Gray }
        continue
    }

    & $out
    $exit = $LASTEXITCODE
    if ($exit -eq $test.Expected) {
        Write-Host " PASS (exit=$exit)" -ForegroundColor Green
        $passed++
        $results += @{ Name = $name; Status = "PASS" }
    } else {
        Write-Host " FAIL (exit=$exit, want=$($test.Expected))" -ForegroundColor Red
        $failed++
        $results += @{ Name = $name; Status = "FAIL(run)" }
    }
}

# ━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━
# 4. 总结
# ━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━
Write-Host ""
Write-Host "[3/4] === 结果总结 ===" -ForegroundColor Cyan
Write-Host "  Passed: $passed / $($tests.Count)" -ForegroundColor Green
if ($failed -gt 0) {
    Write-Host "  Failed: $failed" -ForegroundColor Red
    $results | Where-Object { $_.Status -ne "PASS" } | ForEach-Object {
        Write-Host "    - $($_.Name): $($_.Status)" -ForegroundColor Red
    }
}
Write-Host ""

if ($failed -gt 0) {
    exit 1
} else {
    Write-Host "所有测试通过！ 🎉" -ForegroundColor Green
}
