# 离线跑通 `-Zbuild-std` + forge 后端，并用 **LLVM 做独立 oracle** 对照输出
#
# 三条纪律（本脚本的存在理由）：
#   ① **沙箱只允许写工作区**：cargo 解包 registry 会写 `~/.cargo/registry/src` ⇒ `os error 5`。
#      解法：`CARGO_HOME` 指到 `<repo>/target/cargo-home`（cache+index 从真实 home 拷一次）。
#   ② **生成物一律落 `target/`**：本脚本**不往示例工程目录里写任何东西**——
#      cargo 配置、每档 `--target-dir` 全在 `<repo>/target/forge-rustc-std-hello/` 下
#      （跑完 `git status` 干净）。
#   ③ **集成必须走 `.cargo/config.toml`，不能用 `RUSTFLAGS` 环境变量**：实测两条路不等价——
#      换成 `RUSTFLAGS` 后 `std_detect`（host 侧）也被 forge 编译，ABI 断言直接炸
#      （`rustc FnAbi expects 0, forge packs 1`）。cargo 的 config 只作用于目标构件，
#      正好是我们要的。cargo 从 **CWD 及其父目录**读配置 ⇒ 把配置放进 `target/…/`，
#      再用 `--manifest-path` 指回示例工程即可（配置在外、代码在内）。
#
# 用法（在本目录执行，PowerShell 5.1 亦可）：
#   powershell -NoProfile -ExecutionPolicy Bypass -File .\run-offline.ps1
#   powershell -NoProfile -ExecutionPolicy Bypass -File .\run-offline.ps1 -SkipOracle
#   powershell -NoProfile -ExecutionPolicy Bypass -File .\run-offline.ps1 -AllOptLevels
# 前置：仓库根已构建 `forge_rustc.dll` 与 `tools/forge-rustc-wrapper`，且 nightly 装了 rust-src。
param(
    [string]$RepoRoot = (Resolve-Path "$PSScriptRoot\..\..").Path,
    # 跳过 LLVM 对照（只验 forge；对照要多编一遍 std，约 +35s/档）
    [switch]$SkipOracle,
    # 默认两档（0 = 无优化、2 = 常用优化）；全档位换算力
    [string[]]$OptLevels = @("0", "2"),
    [switch]$AllOptLevels,
    # `-Coverflow-checks` 的取值：默认 `off`（历史默认，跑得快）；
    # 传 `on` 就变成"**开着安全特性**跑整个 std 语料"——后端 8/16 位溢出检查
    # 已改用比较法（不依赖 CF/OF），这条路可以真跑（2026-10-19 实测）。
    [ValidateSet("on", "off")][string]$OverflowChecks = "off"
)

$ErrorActionPreference = "Stop"
if ($AllOptLevels) { $OptLevels = @("0", "1", "2", "3", "s", "z") }
# **兼容两种传参写法**：`powershell -File <脚本> -OptLevels 0,2,3` 会把 `0,2,3` 当成
# **一个字符串**传给 `[string[]]`（`-File` 的参数不做 PowerShell 表达式解析），于是
# `CARGO_PROFILE_DEV_OPT_LEVEL` 变成 "0,2,3" ⇒ cargo 报 "must be 0/1/2/3/s/z"。
# 这里把每个元素再按逗号/空格/分号切开，两种写法都能用（顺手去掉空项与引号）。
$OptLevels = @(
    $OptLevels |
        ForEach-Object { ($_ -replace '["'']', '') -split '[,;\s]+' } |
        Where-Object { $_ -ne '' }
)
if ($OptLevels.Count -eq 0) {
    throw "-OptLevels 为空（示例：-OptLevels 0,2,3 或 -AllOptLevels）"
}

$ProjectDir = $PSScriptRoot
$Manifest = Join-Path $ProjectDir "Cargo.toml"
$ExeName = "forge-rustc-std-hello.exe"

# 注意：不要用 `$home`——PowerShell 的自动变量只读（HOME）。
$cargoHomeDst = Join-Path $RepoRoot "target\cargo-home"
$cargoHomeSrc = if ($env:CARGO_HOME_REAL) { $env:CARGO_HOME_REAL } else { Join-Path $env:USERPROFILE ".cargo" }
$regDst = Join-Path $cargoHomeDst "registry"
$regSrc = Join-Path $cargoHomeSrc "registry"

# 本脚本**全部**生成物都在这里（配置 + 每档 target-dir）。
$workRoot = Join-Path $RepoRoot "target\forge-rustc-std-hello"

$dll = Join-Path $RepoRoot "target\debug\forge_rustc.dll"
$wrapper = Join-Path $RepoRoot "tools\forge-rustc-wrapper\target\debug\forge-rustc-wrapper.exe"
foreach ($p in @($dll, $wrapper)) {
    if (-not (Test-Path $p)) {
        throw "缺 $p —— 先在仓库根构建 forge-rustc 与 wrapper（见 README「前置」）"
    }
}

# ① 播种工作区内的 CARGO_HOME（cache + index，一次即可）
if (-not (Test-Path (Join-Path $regDst "cache"))) {
    Write-Host "播种 $regDst（cache/index，约 250 MB，只做一次）..."
    New-Item -ItemType Directory -Force $regDst | Out-Null
    foreach ($d in @("cache", "index")) {
        Copy-Item -Recurse -Force (Join-Path $regSrc $d) (Join-Path $regDst $d)
    }
}

$env:CARGO_HOME = $cargoHomeDst
$env:CARGO_NET_OFFLINE = "true"
$env:CARGO_INCREMENTAL = "0"
$env:RUSTC_ICE = "0"   # ICE 转储别落进工作目录（rustc 默认写 CWD；`0` = 关掉）
# 清掉可能残留的等价环境变量，免得与 config 打架（见纪律 ③）
foreach ($v in @("RUSTFLAGS", "RUSTC_WRAPPER", "CARGO_BUILD_RUSTC_WRAPPER", "CARGO_UNSTABLE_BUILD_STD")) {
    Remove-Item "Env:\$v" -ErrorAction SilentlyContinue
}

# ② 两套 cargo 配置（都在 target/ 下）：forge（后端 + wrapper + build-std）与 llvm（只 build-std）
function Write-ForgeConfig {
    $dir = Join-Path $workRoot "forge-cwd"
    New-Item -ItemType Directory -Force (Join-Path $dir ".cargo") | Out-Null
    $body = @"
# 由 examples/forge-rustc-std-hello/run-offline.ps1 生成（落在 target/ 下，不污染工程目录）。
[build]
rustflags = [
    "-Zcodegen-backend=$($dll -replace '\\', '\\')",
    "-Cpanic=abort",
    "-Coverflow-checks=$OverflowChecks",
    "-Clink-arg=/SUBSYSTEM:CONSOLE",
    "-Clink-arg=/DEFAULTLIB:kernel32.lib",
    "-Clink-arg=/DEFAULTLIB:vcruntime.lib",
]
rustc-wrapper = "$($wrapper -replace '\\', '\\')"

[unstable]
build-std = ["std", "panic_abort"]
"@
    Set-Content -Path (Join-Path $dir ".cargo\config.toml") -Value $body -Encoding utf8
    return $dir
}

function Write-LlvmConfig {
    $dir = Join-Path $workRoot "llvm-cwd"
    New-Item -ItemType Directory -Force (Join-Path $dir ".cargo") | Out-Null
    $body = @"
# LLVM oracle 用：只保留 build-std（同一套 std 配置），**不带** forge 后端与 wrapper。
[unstable]
build-std = ["std", "panic_abort"]
"@
    Set-Content -Path (Join-Path $dir ".cargo\config.toml") -Value $body -Encoding utf8
    return $dir
}

New-Item -ItemType Directory -Force $workRoot | Out-Null
$forgeCwd = Write-ForgeConfig
$llvmCwd = Write-LlvmConfig

# 在指定 CWD（决定读哪份 .cargo/config.toml）里构建示例工程，target-dir 也在 target/ 下。
function Build-In {
    param([string]$Cwd, [string]$TargetDir)
    Push-Location $Cwd
    try {
        & cargo +nightly build --manifest-path $Manifest --target-dir $TargetDir
        if ($LASTEXITCODE -ne 0) { throw "构建失败（$Cwd，exit=$LASTEXITCODE）" }
    }
    finally { Pop-Location }
}

function Invoke-OptLevel {
    param([string]$Level)

    Write-Host "=== opt-level=$Level ==="
    $env:CARGO_PROFILE_DEV_OPT_LEVEL = $Level
    $forgeDir = Join-Path $workRoot "forge-o$Level"
    $llvmDir = Join-Path $workRoot "llvm-o$Level"

    # forge 后端
    Build-In -Cwd $forgeCwd -TargetDir $forgeDir
    $forgeOut = & (Join-Path $forgeDir "debug\$ExeName")
    $forgeCode = $LASTEXITCODE
    Write-Host ("forge exit=" + $forgeCode)
    if ($forgeCode -ne 0) {
        $forgeOut | ForEach-Object { $_ }
        throw "forge 产物运行失败（opt-level=$Level，exit=$forgeCode）"
    }
    if ($forgeOut -notcontains "all_ok=true") {
        $forgeOut | ForEach-Object { $_ }
        throw "forge 自校验未通过（opt-level=$Level）"
    }

    if ($SkipOracle) { return }

    # LLVM oracle
    Build-In -Cwd $llvmCwd -TargetDir $llvmDir
    $llvmOut = & (Join-Path $llvmDir "debug\$ExeName")
    $llvmCode = $LASTEXITCODE
    Write-Host ("llvm exit=" + $llvmCode)

    $diff = Compare-Object @($llvmOut) @($forgeOut)
    if ($llvmCode -ne $forgeCode -or $null -ne $diff) {
        if ($null -ne $diff) { $diff | Format-Table -AutoSize | Out-String | Write-Host }
        throw "与 LLVM oracle 不一致（opt-level=$Level：forge exit=$forgeCode，llvm exit=$llvmCode）"
    }
    Write-Host "oracle_ok=true（opt-level=$Level：forge 与 LLVM 完整输出逐行相同）"
}

try {
    foreach ($lvl in $OptLevels) { Invoke-OptLevel -Level $lvl }
    Write-Host ("all_levels_ok=true（" + ($OptLevels -join ",") + "）")
    Write-Host "生成物目录：$workRoot（工程目录不留文件）"
}
finally {
    Remove-Item Env:\CARGO_PROFILE_DEV_OPT_LEVEL -ErrorAction SilentlyContinue
}
