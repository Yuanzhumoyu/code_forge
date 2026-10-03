# 语料出处（`asm/`）

本文件登记 `asm/parse/**` 里每个**第三方**文件的出处、固定 ref、许可与摘要，
便于评审与再下载。`asm/exec/**`、`asm/ratchet/**` 都是本仓库自己写的，不在下表。

采集日期：**2026-10-03**。采集方式：本机 shell **没有 TLS**（`curl`/`git` 都报
`SEC_E_NO_CREDENTIALS`），所以文本文件走 CDN（`https://cdn.jsdelivr.net/gh/<owner>/<repo>@<ref>/<path>`）
取回后逐字落盘；**二进制型**的上游期望文件（XED 的 `.reference`/`.hex`、LLVM 的 `.d`）
CDN 拒发（`application/octet-stream`），留给 P2 在能联网的机器上用 `fetch.ps1` 补。

## 已 vendored 的语料

| 路径 | 上游 | 固定 ref | 许可 | 字节 | 行 | 摘要（sha256） | 处理 |
| --- | --- | --- | --- | --- | --- | --- | --- |
| `parse/riscv64/llvm-mc/rv64i-valid.s` | [llvm/llvm-project `llvm/test/MC/RISCV/rv64i-valid.s`](https://github.com/llvm/llvm-project/blob/llvmorg-19.1.0/llvm/test/MC/RISCV/rv64i-valid.s) | `llvmorg-19.1.0` | `Apache-2.0 WITH LLVM-exception` | 3471 | 107 | `93a312ce56ad71dfd27570ffec6fe0572e7eb697e0e9d40c48e931b67c31a1e3` | **整文件**，未删节（已与上游逐行核对一致） |
| `parse/aarch64/llvm-mc/arm64-separator.s` | [llvm/llvm-project `llvm/test/MC/AArch64/arm64-separator.s`](https://github.com/llvm/llvm-project/blob/llvmorg-19.1.0/llvm/test/MC/AArch64/arm64-separator.s) | `llvmorg-19.1.0` | `Apache-2.0 WITH LLVM-exception` | 693 | 19 | `6168aa947fd3cd605855011f07fb6b9e21e012995528c6626bff3785cdeb66ae` | **整文件**，未删节 |
| `parse/x86/gnu-gas-intel/intel.excerpt.s` | [binutils-gdb `gas/testsuite/gas/i386/intel.s`](https://sourceware.org/git/?p=binutils-gdb.git;a=blob;f=gas/testsuite/gas/i386/intel.s)（取回走镜像 [`ahjragaas/binutils-gdb`](https://github.com/ahjragaas/binutils-gdb) 的 `master`） | **分支 `master`（未钉）** | `GPL-3.0-or-later` | 1479 | 64 | `319eb18610a8e9e1947c6634a8700365d9e8cb007b6f6fc7f4340e3617d9554b` | **节选**：上游文件开头 64 行（含 `.text` / `.intel_syntax noprefix` / `foo:` 三行头）；制表符与对齐空白已归一化为空格，指令/操作数文本逐字 |

三点必须知道的：

1. **x86 是节选且未钉 commit**——jsDelivr 对该镜像返回的 `resolved.version` 为 `null`
   （仓库过大、未索引），只能按分支取，因此**可复现性弱于另两份**；P2 换成
   固定 commit（或 `sourceware` 的 tag）的全量文件。
2. **唯一的空白归一化发生在 x86 那份**（原文件用制表符缩进，`Get-Content` 往返后落成空格）；
   对汇编器而言空白无语义，但"逐字保留上游原文"这句话对它**不成立**，此处如实登记。
3. 表里的 sha256 是**本仓库落盘后的文件**摘要（用于发现后续被改动），不是上游文件的摘要。
   为了让它在任何机器上都可核，仓库根 `.gitattributes` 把 `crates/tools/forge-tests/asm/**`
   钉成 `text eol=lf`——否则 `core.autocrlf` 会在 Windows 上把工作区改成 CRLF，摘要就对不上了
   （语义不受影响：汇编器把 `\r` 当空白，但"逐字保留上游原文"这句话会不成立）。

## 计划在 P2 补齐（需 TLS 的机器）

| ISA | 语料 | 固定 ref | 许可 | 补进来解决什么 |
| --- | --- | --- | --- | --- |
| x86 | LLVM MC `llvm/test/MC/X86/**`（`-show-encoding` 带 `encoding:` 字节） | `llvmorg-19.1.0` | `Apache-2.0 WITH LLVM-exception` | x86 的**字节 oracle**（当前为 0 条） |
| aarch64 | LLVM MC `llvm/test/MC/AArch64/**` | `llvmorg-19.1.0` | `Apache-2.0 WITH LLVM-exception` | aarch64 的**字节 oracle**（当前为 0 条） |
| riscv64 | LLVM MC `llvm/test/MC/RISCV/**`（全量，不止 `rv64i-valid.s`） | `llvmorg-19.1.0` | `Apache-2.0 WITH LLVM-exception` | 覆盖 RVC/RVV/伪指令与别名寄存器 |
| x86 | Intel XED `tests-syntax` + `bulk-tests`（`cmd`/`.reference`/`.hex`） | 固定 commit | `Apache-2.0` | 逐条 asm↔字节的第三方对照 |
| x86 | NASM `test/**` + `golden/**`、YASM `modules/arch/x86/tests/**` | 固定 commit/tag | `BSD-2-Clause` | 真实项目写法的 Intel 语法与黄金输出 |
| x86 | binutils-gdb `gas/testsuite/gas/i386/intel*.s` **全量**（替换现有节选） | 固定 commit/tag | `GPL-3.0-or-later` | 消掉"节选 + 空白归一化"两个弱点 |

**不进解析档的语料（有意排除）**：`riscv-tests` / `riscv-arch-test` 的 `.S`——它们靠 C
预处理器（`encoding.h`）、链接脚本与 `tohost`/`spike` 自检协议，且正文来自 git 子模块；
本汇编器没有预处理器，执行通道也不是 `tohost`（见 `README.md` 的「边界」）。

## 许可与再分发

三份语料各自沿用上游许可（见上表）。仓库根**没有**统一的 `LICENSE`/`COPYING` 文件，
所以 GAS 那份（`GPL-3.0-or-later`）被放在**独立目录** `parse/x86/gnu-gas-intel/` 下，
与本目录的 `PROVENANCE.md` 一起构成出处与许可记录；若将来要对外发行，请连同上游许可
正文一并带出，或把该目录替换为 `Apache-2.0`/`BSD-2-Clause` 的语料（NASM/XED）。
