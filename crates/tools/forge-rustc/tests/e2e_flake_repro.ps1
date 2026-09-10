# e2e FLAKY 用例的取证复现脚本（vec/alloc 族偶发失败）
#
# 背景：`tests/e2e.rs` 的 5 个 FLAKY 用例（vec_push / vec_string / vec_from_slice /
# vec_iter_enumerate / box_value）历史上偶发失败，本机 ≈1/100。判定失败形态必须拿到
# 失败轮的 `KNOWN <case> error: …` 文案与产物——脚本在这里做三件事：
#   ① `-Mode hammer`：加严复跑（5 轮全量 stage_a + 并行变体 + 5 用例各单跑 ×10）；
#   ② `-Mode load`：N 个并发 worker 直接跑 e2e 测试二进制（单用例），制造负载直到复现；
#   ③ 失败即写 `target/tmp/load_fail_*.txt`，并把 KEEP 保留的失败工作目录**复制回工作区**
#      （`%TEMP%` 会随会话轮换被清理，历史上因此丢过证据）。
#
# 该脚本存在仓库内的理由：`target/tmp` 是临时目录，2026-09-10 会话轮换即被清空，
# 当时 hammer/复现脚本全部丢失。见 docs/plans/forge-rustc-vec_push-plan.md §9.2。
#
# 用法（在仓库根执行）：
#   & crates\tools\forge-rustc\tests\e2e_flake_repro.ps1 -Mode hammer
#   & crates\tools\forge-rustc\tests\e2e_flake_repro.ps1 -Mode load -Case vec_from_slice -Workers 4 -Rounds 30
param(
    [ValidateSet('hammer', 'load')]
    [string]$Mode = 'hammer',
    [string]$Case = 'vec_from_slice',
    [int]$Workers = 4,
    [int]$Rounds = 30,
    # 工具链：CI 钉 nightly-2026-09-05；本机可用 FORGE_E2E_NIGHTLY/此参数指向已装 channel。
    [string]$Toolchain = 'nightly-2026-09-05'
)

$ErrorActionPreference = 'Continue'
$root = (Resolve-Path (Join-Path $PSScriptRoot '..\..\..\..')).Path
$tmp = Join-Path $root 'target\tmp'
New-Item -ItemType Directory -Force -Path $tmp | Out-Null
Set-Location $root

# 钉版工具链（含 rustc-dev）默认放 target\rustup_home，避免本机 .rustup 被拦截。
if (-not $env:RUSTUP_HOME) { $env:RUSTUP_HOME = Join-Path $root 'target\rustup_home' }
$env:RUSTUP_TOOLCHAIN = $Toolchain
$env:CARGO_TERM_COLOR = 'never'
$env:FORGE_E2E_KEEP = '1'   # 失败轮保留工作目录（取证）
Remove-Item Env:FORGE_E2E_TRACE, Env:FORGE_TRACE_ALLOC, Env:FORGE_TRACE_SPILL -ErrorAction SilentlyContinue

$log = Join-Path $tmp "flake_repro_${Mode}_${Case}.txt"
$strict = if ($env:FORGE_E2E_STRICT_FLAKY) { 'strict' } else { 'tolerant' }
"=== flake repro mode=$Mode case=$Case workers=$Workers rounds=$Rounds toolchain=$Toolchain flaky=$strict start $(Get-Date -Format 'HH:mm:ss') ===" |
    Out-File $log -Encoding utf8

function Invoke-CargoTest {
    param([string[]]$CargoArgs)
    $out = & cargo test @CargoArgs 2>&1 | Out-String
    return @{ Out = $out; Exit = $LASTEXITCODE }
}

if ($Mode -eq 'hammer') {
    Remove-Item Env:FORGE_E2E_ONLY -ErrorAction SilentlyContinue
    for ($i = 1; $i -le 5; $i++) {
        $r = Invoke-CargoTest @('-p', 'forge-rustc', '--test', 'e2e', '--', '--nocapture', 'e2e_stage_a_scalar_cases')
        $passed = (($r.Out -split "`n" | Where-Object { $_ -match '^passed: ' } | Select-Object -First 1) -replace '\s+$', '')
        $known = (($r.Out -split "`n" | Where-Object { $_ -match '^KNOWN |^FAIL ' }) -join ' || ')
        "stage_a run${i}: exit=$($r.Exit)  $passed  KNOWN=[$known]" | Out-File $log -Append -Encoding utf8
    }
    $r = Invoke-CargoTest @('-p', 'forge-rustc', '--test', 'e2e', '--', '--nocapture', 'e2e_parallel_pool_threads')
    $par = (($r.Out -split "`n" | Where-Object { $_ -match 'test result|parallel-pool' } | Select-Object -Last 2) -join ' ; ') -replace '\s+$', ''
    "parallel: exit=$($r.Exit)  $par" | Out-File $log -Append -Encoding utf8

    foreach ($c in 'vec_push', 'vec_string', 'vec_from_slice', 'vec_iter_enumerate', 'box_value') {
        $ok = 0; $fail = 0; $codes = @()
        for ($k = 1; $k -le 10; $k++) {
            $env:FORGE_E2E_ONLY = $c
            $r = Invoke-CargoTest @('-p', 'forge-rustc', '--test', 'e2e', '--', '--nocapture', 'e2e_stage_a_scalar_cases')
            # 失败形态二选一：`KNOWN <case> exit=N (want M)`（错码，容忍模式）或
            # `KNOWN|FAIL <case> error: …`（Err）；严格模式下错码走 `FAIL`。
            $known = (($r.Out -split "`n" | Where-Object { $_ -match "^KNOWN $c |^FAIL $c " } | Select-Object -First 1) -replace '\s+$', '')
            $m = [regex]::Match($r.Out, "$c\s+exit=(-?\d+)")
            if ($m.Success) { $codes += $m.Groups[1].Value }
            if ($known) {
                $fail++
                $r.Out | Out-File (Join-Path $tmp "hammer_fail_${c}_${k}.txt") -Encoding utf8
                "    run${k} KNOWN: $known" | Out-File $log -Append -Encoding utf8
            } else { $ok++ }
        }
        Remove-Item Env:FORGE_E2E_ONLY -ErrorAction SilentlyContinue
        "$c : pass=$ok fail=$fail  exits=[$($codes -join ',')]" | Out-File $log -Append -Encoding utf8
    }
}
else {
    $env:FORGE_E2E_ONLY = $Case
    $build = & cargo test -p forge-rustc --test e2e --no-run 2>&1 | Out-String
    ($build -split "`n" | Where-Object { $_ -match 'Executable|^error' }) -join "`n" | Out-File $log -Append -Encoding utf8
    # cargo 打印形如：`  Executable tests\e2e.rs (target\debug\build\...\out\e2e-<hash>.exe)`
    # —— 真实路径在括号内，且是相对仓库根的路径。
    $exeLineRaw = ($build -split "`n" | Where-Object { $_ -match 'Executable .*\(' } | Select-Object -First 1)
    $exePath = ''
    if ([string]$exeLineRaw -match '\(([^)]+)\)') { $exePath = $Matches[1].Trim() }
    if ($exePath -and $exePath -notmatch '^[A-Za-z]:') { $exePath = Join-Path $root $exePath }
    if (-not $exePath -or -not (Test-Path $exePath)) {
        "no e2e test binary found (line: '$exeLineRaw') —— 先修好工具链：rustup component add rustc-dev" |
            Out-File $log -Append -Encoding utf8
        Get-Content $log; exit 1
    }
    "test binary: $exePath" | Out-File $log -Append -Encoding utf8

    $sb = {
        param($exePath, $case, $wid, $rounds, $tmp)
        $dts = @()
        $retries = @()
        for ($k = 1; $k -le $rounds; $k++) {
            $t0 = Get-Date
            $out = & $exePath e2e_stage_a_scalar_cases --nocapture 2>&1 | Out-String
            $dts += [int]((Get-Date) - $t0).TotalMilliseconds
            # 超时重试（harness 侧一次同产物复跑）也留痕：这是"宿主侧延迟"的直接证据
            foreach ($rt in ($out -split "`n" | Where-Object { $_ -match '^RETRY ' })) {
                $retries += "worker${wid} run${k}: $($rt -replace '\s+$', '')"
            }
            $known = (($out -split "`n" | Where-Object { $_ -match "^KNOWN $case |^FAIL $case " } |
                    Select-Object -First 1) -replace '\s+$', '')
            if ($known) {
                $raw = Join-Path $tmp "load_fail_w${wid}_r${k}.txt"
                $out | Out-File $raw -Encoding utf8
                # `[keep] 失败用例 … —— 工作目录保留：<path>（.rs/.exe 可复跑对照）`
                # 只取「保留：」到下一个全角括号之间的路径（否则 Test-Path 必失败）。
                $keepPath = ''
                if ([string]$out -match '保留：([^（]+)') { $keepPath = $Matches[1].Trim() }
                $copied = ''
                if ($keepPath -and (Test-Path $keepPath)) {
                    $dst = Join-Path $tmp "fail_evidence_w${wid}_r${k}"
                    New-Item -ItemType Directory -Force -Path $dst | Out-Null
                    Copy-Item "$keepPath\*" $dst -Force -Recurse -ErrorAction SilentlyContinue
                    $copied = "copied->$dst"
                }
                return "FAIL worker${wid} run${k} dt=$($dts[-1])ms :: $known :: keep=$keepPath $copied"
            }
        }
        $max = ($dts | Measure-Object -Maximum).Maximum
        $avg = [int](($dts | Measure-Object -Average).Average)
        $slow = ($dts | Where-Object { $_ -gt 5000 }).Count
        $rt = if ($retries.Count) { "`n" + ($retries -join "`n") } else { '' }
        return "ok   worker${wid} ${rounds} runs  avg=${avg}ms max=${max}ms slow(>5s)=${slow} retries=$($retries.Count)$rt"
    }
    $jobs = 1..$Workers | ForEach-Object { Start-Job -ScriptBlock $sb -ArgumentList $exePath, $Case, $_, $Rounds, $tmp }
    foreach ($line in (Receive-Job -Job $jobs -Wait -AutoRemoveJob)) { $line | Out-File $log -Append -Encoding utf8 }
}

"=== flake repro end $(Get-Date -Format 'HH:mm:ss') ===" | Out-File $log -Append -Encoding utf8
Get-Content $log
