<#
.SYNOPSIS
  按固定 ref 拉取真实汇编语料到 asm/parse/<isa>/<suite>/，并打印可直接粘进 PROVENANCE.md 的表格行。

.DESCRIPTION
  本机（2026-10-03）没有 TLS（curl/git 报 SEC_E_NO_CREDENTIALS），所以 P1 只 vendored 了
  三份小样。在能联网的机器上跑这个脚本补齐/刷新语料：

    pwsh crates/tools/forge-tests/asm/fetch.ps1 -List            # 只看计划（不下载）
    pwsh crates/tools/forge-tests/asm/fetch.ps1 -Isa riscv64     # 只拉一套
    pwsh crates/tools/forge-tests/asm/fetch.ps1 -Isa all -Force  # 全拉（覆盖已存在文件）

  拉完必须做两件事（脚本会在结尾提醒）：
    1. 用打印出来的表格行更新 asm/PROVENANCE.md（出处/ref/许可/摘要）；
    2. 重新刷计数棘轮并**看 diff**：
       $env:FORGE_ASM_WRITE_RATCHET = "1"; cargo test -p forge-tests --test asm_parse

.NOTES
  许可随上游（LLVM MC/XED = Apache-2.0 WITH LLVM-exception / Apache-2.0，
  NASM/YASM = BSD-2-Clause，GAS = GPL-3.0-or-later）。脚本只下载，不改上游文件内容。
#>
[CmdletBinding()]
param(
    # 只拉哪个 ISA（x86 / riscv64 / aarch64 / all）
    [string]$Isa = 'all',
    # 只打印计划
    [switch]$List,
    # 覆盖已存在的文件
    [switch]$Force
)

Set-StrictMode -Version Latest
$ErrorActionPreference = 'Stop'

# 语料根 = 本脚本所在目录（asm/）。
$CorpusRoot = $PSScriptRoot

# 每套语料：Isa / Suite / Repo / Ref / License / Files（显式路径）/ Dirs（整个目录，按扩展名筛）。
# Ref 一律钉到 tag 或 commit——**不要用分支名**（P1 的 x86 节选就是因为镜像回退到 master 才难复现）。
$Sources = @(
    @{
        Isa     = 'riscv64'
        Suite   = 'llvm-mc'
        Repo    = 'llvm/llvm-project'
        Ref     = 'llvmorg-19.1.0'
        License = 'Apache-2.0 WITH LLVM-exception'
        Files   = @()
        Dirs    = @(@{ Path = 'llvm/test/MC/RISCV'; Ext = @('.s') })
    }
    @{
        Isa     = 'aarch64'
        Suite   = 'llvm-mc'
        Repo    = 'llvm/llvm-project'
        Ref     = 'llvmorg-19.1.0'
        License = 'Apache-2.0 WITH LLVM-exception'
        Files   = @()
        Dirs    = @(@{ Path = 'llvm/test/MC/AArch64'; Ext = @('.s', '.txt') })
    }
    @{
        Isa     = 'x86'
        Suite   = 'llvm-mc'
        Repo    = 'llvm/llvm-project'
        Ref     = 'llvmorg-19.1.0'
        License = 'Apache-2.0 WITH LLVM-exception'
        Files   = @()
        Dirs    = @(@{ Path = 'llvm/test/MC/X86'; Ext = @('.s') })
    }
    @{
        # 替换 P1 那份节选（intel.excerpt.s）用的：GAS 全量 i386 Intel 语法用例。
        # 钉 tag 或 commit；镜像 ahjragaas/binutils-gdb 只能按分支取，故此处留空待填。
        Isa     = 'x86'
        Suite   = 'gnu-gas-intel'
        Repo    = 'bminor/binutils-gdb'
        Ref     = 'PIN-A-COMMIT-OR-TAG'
        License = 'GPL-3.0-or-later'
        Files   = @('gas/testsuite/gas/i386/intel.s')
        Dirs    = @()
    }
)

# 只支持文本型语料；二进制型（XED 的 .reference/.hex）用 -Force 直接落盘即可，
# 这里不按扩展名过滤它们（走 Files 显式列出）。
function Get-RelDir([string]$isa, [string]$suite) {
    Join-Path (Join-Path (Join-Path $CorpusRoot 'parse') $isa) $suite
}

function Get-RemoteFiles($src) {
    $out = New-Object System.Collections.Generic.List[string]
    foreach ($f in $src.Files) { $out.Add($f) }
    foreach ($d in $src.Dirs) {
        $api = "https://api.github.com/repos/$($src.Repo)/contents/$($d.Path)?ref=$($src.Ref)"
        $items = Invoke-RestMethod -Uri $api -Headers @{ 'User-Agent' = 'forge-tests-fetch' }
        foreach ($it in $items) {
            if ($it.type -ne 'file') { continue }
            $ext = [System.IO.Path]::GetExtension($it.name)
            if ($d.Ext -contains $ext) { $out.Add($it.path) }
        }
    }
    return $out | Sort-Object -Unique
}

$selected = $Sources | Where-Object { $Isa -eq 'all' -or $_.Isa -eq $Isa }
if (-not $selected) { throw "没有匹配 -Isa '$Isa' 的语料（可选：x86 / riscv64 / aarch64 / all）" }

if ($List) {
    foreach ($src in $selected) {
        Write-Host "[$($src.Isa)/$($src.Suite)] $($src.Repo)@$($src.Ref)  ($($src.License))"
        foreach ($f in $src.Files) { Write-Host "    file  $f" }
        foreach ($d in $src.Dirs) { Write-Host "    dir   $($d.Path)  ext=$($d.Ext -join ',')" }
    }
    return
}

$rows = New-Object System.Collections.Generic.List[string]
foreach ($src in $selected) {
    if ($src.Ref -like 'PIN-*') {
        Write-Warning "[$($src.Isa)/$($src.Suite)] ref 未钉（$($src.Ref)）——先在脚本里改成真实 tag/commit 再拉"
        continue
    }
    $dest = Get-RelDir $src.Isa $src.Suite
    New-Item -ItemType Directory -Force -Path $dest | Out-Null
    foreach ($path in (Get-RemoteFiles $src)) {
        $name = Split-Path $path -Leaf
        $target = Join-Path $dest $name
        if ((Test-Path $target) -and -not $Force) {
            Write-Host "skip   $name（已存在，-Force 覆盖）"
        }
        else {
            $url = "https://raw.githubusercontent.com/$($src.Repo)/$($src.Ref)/$path"
            Write-Host "fetch  $url"
            Invoke-WebRequest -Uri $url -OutFile $target -Headers @{ 'User-Agent' = 'forge-tests-fetch' }
        }
        $fi = Get-Item $target
        $hash = (Get-FileHash -Algorithm SHA256 -Path $target).Hash.ToLower()
        $lines = (Get-Content -LiteralPath $target | Measure-Object -Line).Lines
        $rel = (Resolve-Path -Relative $target).Replace('\', '/').Replace('./', '')
        $rows.Add("| ``$rel`` | [$($src.Repo) ``$path``](https://github.com/$($src.Repo)/blob/$($src.Ref)/$path) | ``$($src.Ref)`` | ``$($src.License)`` | $($fi.Length) | $lines | ``$hash`` | **整文件**，未删节 |")
    }
}

Write-Host ''
Write-Host "## 已 vendored 的语料（fetch.ps1 于 $(Get-Date -Format 'yyyy-MM-dd') 生成）"
Write-Host ''
Write-Host '| 路径 | 上游 | 固定 ref | 许可 | 字节 | 行 | 摘要（sha256） | 处理 |'
Write-Host '| --- | --- | --- | --- | --- | --- | --- | --- |'
foreach ($r in $rows) { Write-Host $r }

Write-Host ''
Write-Host '下一步：'
Write-Host '  1. 把上面的表格行粘进 asm/PROVENANCE.md（替换同路径的旧行）；'
Write-Host '  2. 若加了新 suite，先在 src/asm/corpus.rs 的 SUITES 里登记；'
Write-Host '  3. 刷棘轮并**看 diff**：$env:FORGE_ASM_WRITE_RATCHET = "1"; cargo test -p forge-tests --test asm_parse'
