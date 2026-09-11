# PE `.text` 段 SHA256 —— 跨机器可比的产物指纹
#
# 为什么不用整文件哈希：PE 里嵌了 PDB 路径与时间戳，CI 与本机的路径必然不同，
# 整文件哈希恒不等。`.text` 段字节才是"生成代码是否一致"的判据——计划 §9.1 的
# "CI 产物 vs 本机产物逐字节对照"就用这个值。
#
# 用法：
#    . .\pe_text_hash.ps1                      # 只定义函数（CI 里 dot-source）
#    .\pe_text_hash.ps1 a.exe b.exe             # 打印每个文件的 .text SHA256
function Get-PeTextSha256 {
    [CmdletBinding()]
    param([Parameter(Mandatory)][string]$Path)

    $b = [System.IO.File]::ReadAllBytes($Path)
    if ($b.Length -lt 0x40) { return 'too-small' }
    $pe = [BitConverter]::ToInt32($b, 0x3C)
    if ($pe -le 0 -or $pe + 24 -gt $b.Length) { return 'bad-pe' }
    $nSec = [BitConverter]::ToUInt16($b, $pe + 6)
    $optSize = [BitConverter]::ToUInt16($b, $pe + 20)
    $sec = $pe + 24 + $optSize
    for ($i = 0; $i -lt $nSec; $i++) {
        $o = $sec + $i * 40
        if ($o + 40 -gt $b.Length) { break }
        $name = [System.Text.Encoding]::ASCII.GetString($b, $o, 8).TrimEnd([char]0)
        if ($name -eq '.text') {
            $size = [BitConverter]::ToInt32($b, $o + 16)   # SizeOfRawData
            $ptr = [BitConverter]::ToInt32($b, $o + 20)    # PointerToRawData
            if ($ptr -le 0 -or $ptr + $size -gt $b.Length) { return 'bad-section' }
            $sha = [System.Security.Cryptography.SHA256]::Create().ComputeHash($b, $ptr, $size)
            return (($sha | ForEach-Object { $_.ToString('x2') }) -join '')
        }
    }
    return 'no-.text'
}

# 直接执行（非 dot-source）且带参数时打印指纹
if ($args.Count -gt 0) {
    foreach ($p in $args) {
        "{0}  {1}" -f (Get-PeTextSha256 -Path $p), $p
    }
}
