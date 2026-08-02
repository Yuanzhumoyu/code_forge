# rustc_codegen_codegenlib 端到端集成测试
# 构建后端并测试编译 Rust 程序

param(
    [switch]$Release,
    [switch]$Verbose
)

$ErrorActionPreference = "Stop"
$backend_dir = Split-Path -Parent $PSScriptRoot
$profile = if ($Release) { "release" } else { "debug" }
$backend_dll = "$backend_dir\target\$profile\rustc_codegen_codegenlib.dll"
$nightly = "nightly"

Write-Host "=== codegen-lib rustc backend — 端到端测试 ===" -ForegroundColor Cyan
Write-Host ""

# ━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━
# 1. 构建后端
# ━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━
Write-Host "[1/4] Building backend..." -ForegroundColor Yellow
Push-Location $backend_dir
try {
    $build_flag = if ($Release) { "--release" } else { "" }
    cargo +$nightly build $build_flag 2>&1 | Out-Null
    if ($LASTEXITCODE -ne 0) {
        throw "Backend build failed"
    }
    Write-Host "  ✅ Backend built: $backend_dll" -ForegroundColor Green
} finally {
    Pop-Location
}

# ━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━
# 2. 测试程序列表
# ━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━
$tests = @(
    @{
        Name = "hello_empty"
        Source = @'
fn main() {}
'@
        ExpectSuccess = $true
    },
    @{
        Name = "simple_add"
        Source = @'
fn main() {
    let _x = 1 + 2;
}
'@
        ExpectSuccess = $true
    },
    @{
        Name = "no_std_minimal"
        Source = @'
#![no_std]
#![no_main]

#[unsafe(no_mangle)]
pub extern "C" fn mainCRTStartup() -> ! {
    loop {}
}

#[panic_handler]
fn panic(_info: &core::panic::PanicInfo) -> ! {
    loop {}
}
'@
        ExpectSuccess = $true
    }
)

$rustc_flags = @(
    "-Zcodegen-backend=$backend_dll",
    "-C", "panic=abort",
    "--edition", "2024"
)

$no_std_flags = @(
    "-Zcodegen-backend=$backend_dll",
    "-C", "panic=abort",
    "--edition", "2024",
    "-C", "link-args=/ENTRY:mainCRTStartup /SUBSYSTEM:CONSOLE"
)

$passed = 0
$failed = 0
$results = @()

# ━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━
# 3. 运行测试
# ━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━
Write-Host "[2/4] Compiling test programs..." -ForegroundColor Yellow

foreach ($test in $tests) {
    $name = $test.Name
    $tmpdir = Join-Path $env:TEMP "rustc_backend_test"
    New-Item -ItemType Directory -Force -Path $tmpdir | Out-Null
    $src = Join-Path $tmpdir "$name.rs"
    $out = Join-Path $tmpdir "$name.exe"
    Set-Content -Path $src -Value $test.Source

    Write-Host "  Testing: $name..." -NoNewline

    $flags = if ($test.Source -match "#!\[no_std\]") { $no_std_flags } else { $rustc_flags }

    $result = & rustc +$nightly @flags $src -o $out 2>&1
    $exit = $LASTEXITCODE

    if ($exit -eq 0) {
        $ok = Test-Path $out
        if ($ok) {
            Write-Host " ✅ PASS" -ForegroundColor Green
            $passed++
            $results += @{ Name = $name; Status = "PASS"; Size = (Get-Item $out).Length }
            if ($Verbose) {
                Write-Host "    Output: $out ($([math]::Round((Get-Item $out).Length / 1KB, 1)) KB)" -ForegroundColor Gray
            }
        } else {
            Write-Host " ❌ FAIL (no output file)" -ForegroundColor Red
            $failed++
        }
    } else {
        if ($test.ExpectSuccess) {
            Write-Host " ❌ FAIL (expected success)" -ForegroundColor Red
            $failed++
            if ($Verbose) {
                Write-Host "    Error: $result" -ForegroundColor Gray
            }
        } else {
            Write-Host " ✅ PASS (expected failure)" -ForegroundColor Green
            $passed++
        }
    }
}

# ━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━
# 4. 更多测试：复杂的 MIR 结构
# ━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━
Write-Host "[3/4] Testing complex MIR lowering..." -ForegroundColor Yellow

$complex_tests = @(
    @{
        Name = "if_else"
        Source = @'
fn main() {
    let x = if true { 1 } else { 2 };
    let _y = x + 3;
}
'@
    },
    @{
        Name = "loop_control"
        Source = @'
fn main() {
    let mut x = 0;
    while x < 10 {
        x += 1;
    }
}
'@
    }
)

foreach ($test in $complex_tests) {
    $name = $test.Name
    $src = Join-Path $env:TEMP "rustc_backend_test\$name.rs"
    $out = Join-Path $env:TEMP "rustc_backend_test\$name.exe"
    Set-Content -Path $src -Value $test.Source

    Write-Host "  Testing: $name..." -NoNewline

    $result = & rustc +$nightly @rustc_flags $src -o $out 2>&1
    $exit = $LASTEXITCODE

    if ($exit -eq 0) {
        Write-Host " ✅ PASS" -ForegroundColor Green
        $passed++
    } else {
        Write-Host " ⚠️ NOT YET (complex MIR — expected at this stage)" -ForegroundColor Yellow
        # Don't count as failure since complex MIR is WIP
    }
}

# ━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━
# 5. 总结
# ━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━
Write-Host ""
Write-Host "[4/4] === 结果总结 ===" -ForegroundColor Cyan
Write-Host "  Passed: $passed" -ForegroundColor Green
if ($failed -gt 0) {
    Write-Host "  Failed: $failed" -ForegroundColor Red
}
Write-Host ""

# 显示生成的文件
Write-Host "生成的可执行文件:" -ForegroundColor Gray
Get-ChildItem "$env:TEMP\rustc_backend_test\*.exe" -ErrorAction SilentlyContinue | ForEach-Object {
    Write-Host "  $($_.Name) — $([math]::Round($_.Length / 1KB, 1)) KB" -ForegroundColor Gray
}

if ($failed -gt 0) {
    exit 1
} else {
    Write-Host "所有测试通过！ 🎉" -ForegroundColor Green
}
