# 语料出处（`asm/`）

本文件登记 `asm/parse/**` 里每个**第三方**文件的出处、固定 ref、许可与摘要，
便于评审与再下载。`asm/exec/**`、`asm/ratchet/**` 都是本仓库自己写的，不在下表。

采集日期：**2026-10-03**。采集方式：本机 shell **没有 TLS**（`curl`/`git` 都报
`SEC_E_NO_CREDENTIALS`），联网通道只有 `web_fetch`（走 CDN
`https://cdn.jsdelivr.net/gh/<owner>/<repo>@<ref>/<path>`），取回后逐字落盘；
**二进制型**的上游期望文件（XED 的 `.reference`/`.hex`、LLVM 的 `.d`）CDN 拒发
（`application/octet-stream`），留给能联网的机器用 `fetch.ps1` 补（见文末清单）。

## 已 vendored 的语料

| 路径 | 上游 | 固定 ref | 许可 | 字节 | 行 | 摘要（sha256） | 处理 |
| --- | --- | --- | --- | --- | --- | --- | --- |
| `parse/riscv64/llvm-mc/rv64i-valid.s` | [llvm/llvm-project `llvm/test/MC/RISCV/rv64i-valid.s`](https://github.com/llvm/llvm-project/blob/llvmorg-19.1.0/llvm/test/MC/RISCV/rv64i-valid.s) | `llvmorg-19.1.0` | `Apache-2.0 WITH LLVM-exception` | 3471 | 107 | `93a312ce56ad71dfd27570ffec6fe0572e7eb697e0e9d40c48e931b67c31a1e3` | **整文件**，未删节（已与上游逐行核对一致） |
| `parse/aarch64/llvm-mc/arm64-separator.s` | [llvm/llvm-project `llvm/test/MC/AArch64/arm64-separator.s`](https://github.com/llvm/llvm-project/blob/llvmorg-19.1.0/llvm/test/MC/AArch64/arm64-separator.s) | `llvmorg-19.1.0` | `Apache-2.0 WITH LLVM-exception` | 693 | 19 | `6168aa947fd3cd605855011f07fb6b9e21e012995528c6626bff3785cdeb66ae` | **整文件**，未删节 |
| `parse/x86/gnu-gas-intel/intel.excerpt.s` | [binutils-gdb `gas/testsuite/gas/i386/intel.s`](https://sourceware.org/git/?p=binutils-gdb.git;a=blob;f=gas/testsuite/gas/i386/intel.s)（取回走镜像 [`ahjragaas/binutils-gdb`](https://github.com/ahjragaas/binutils-gdb) 的 `master`） | **分支 `master`（未钉）** | `GPL-3.0-or-later` | 1479 | 64 | `319eb18610a8e9e1947c6634a8700365d9e8cb007b6f6fc7f4340e3617d9554b` | **节选**：上游文件开头 64 行（含 `.text` / `.intel_syntax noprefix` / `foo:` 三行头）；制表符与对齐空白已归一化为空格，指令/操作数文本逐字 |
| `parse/x86/llvm-mc/intel-syntax-encoding.s` | [llvm/llvm-project `llvm/test/MC/X86/intel-syntax-encoding.s`](https://github.com/llvm/llvm-project/blob/llvmorg-19.1.0/llvm/test/MC/X86/intel-syntax-encoding.s) | `llvmorg-19.1.0` | `Apache-2.0 WITH LLVM-exception` | 2222 | 100 | `410889ee7fecc9e727029624111907a63e51f5a33b1a32015c5c8711638c9cdb` | **整文件**，未删节（Intel 语法 + `encoding:` 期望字节，40 条） |
| `parse/x86/llvm-mc/apx-rex2-format-intel.s` | [llvm/llvm-project `llvm/test/MC/X86/apx/rex2-format-intel.s`](https://github.com/llvm/llvm-project/blob/llvmorg-19.1.0/llvm/test/MC/X86/apx/rex2-format-intel.s) | `llvmorg-19.1.0` | `Apache-2.0 WITH LLVM-exception` | 8611 | 347 | `84d1cc1bc069fcfcc366288af642788a63015cd8ff57bf21490cb14a92a19de3` | **整文件**，未删节（**文件名把上游的 `apx/` 目录段折进来了**——本目录不建子目录，改的是本仓库的落盘名，文件内容逐字未动；APX/REX2 形式，73 条期望字节） |
| `parse/aarch64/llvm-mc/arm64-logical-encoding.s` | [llvm/llvm-project `llvm/test/MC/AArch64/arm64-logical-encoding.s`](https://github.com/llvm/llvm-project/blob/llvmorg-19.1.0/llvm/test/MC/AArch64/arm64-logical-encoding.s) | `llvmorg-19.1.0` | `Apache-2.0 WITH LLVM-exception` | 9619 | 231 | `fc6cc18123d4892ad182a364383c4b90d2cf03eca750ceaeb00e43f65ee3114f` | **整文件**，未删节（`; CHECK: <asm> ; encoding: [..]` 块式，95 条） |
| `parse/aarch64/llvm-mc/arm64-branch-encoding.s` | [llvm/llvm-project `llvm/test/MC/AArch64/arm64-branch-encoding.s`](https://github.com/llvm/llvm-project/blob/llvmorg-19.1.0/llvm/test/MC/AArch64/arm64-branch-encoding.s) | `llvmorg-19.1.0` | `Apache-2.0 WITH LLVM-exception` | 6175 | 169 | `078fa982f97aa47dff06efe8a692b14e5cf572c8da0d6b9130bbcf737d68d186` | **整文件**，未删节（**注释在指令之后**，见 `Suite::encoding_sides`；24 条） |
| `parse/riscv64/llvm-mc/rv32i-valid.s` | [llvm/llvm-project `llvm/test/MC/RISCV/rv32i-valid.s`](https://github.com/llvm/llvm-project/blob/llvmorg-19.1.0/llvm/test/MC/RISCV/rv32i-valid.s) | `llvmorg-19.1.0` | `Apache-2.0 WITH LLVM-exception` | 11923 | 375 | `d76e2741ab5e5a0dd570729b233df9b6b716e6394aa2cf314494db7cca55e860` | **整文件**，未删节（106 条期望，含重定位 `A` 形式） |
| `parse/riscv64/llvm-mc/rv64m-valid.s` | [llvm/llvm-project `llvm/test/MC/RISCV/rv64m-valid.s`](https://github.com/llvm/llvm-project/blob/llvmorg-19.1.0/llvm/test/MC/RISCV/rv64m-valid.s) | `llvmorg-19.1.0` | `Apache-2.0 WITH LLVM-exception` | 826 | 21 | `49990196abbd6bde407cfb9fd58f3de8655a907630dfd6d013fe2caf0b7e4842` | **整文件**，未删节（`mulw`/`divw` 等 M 扩展，5 条） |

采集方式补充：本机 shell 没有 TLS，网络通道只有 `web_fetch`（走 jsDelivr 的文本文件），
所以上表是**一份份取回**的；`asm/fetch.ps1` 供能联网的机器按目录批量补齐。

三点必须知道的：

1. **x86 的 GAS 那份是节选且未钉 commit**——jsDelivr 对该镜像返回的 `resolved.version` 为
   `null`（仓库过大、未索引），只能按分支取，因此**可复现性弱于其余各份**；在能联网的机器上
   按固定 commit/tag 换成全量文件即可（`fetch.ps1` 里已留条目，ref 待钉）。
2. **唯一的空白归一化发生在 GAS 那份**（原文件用制表符缩进，`Get-Content` 往返后落成空格）；
   对汇编器而言空白无语义，但"逐字保留上游原文"这句话对它**不成立**，此处如实登记。
   LLVM 那六份是 `web_fetch` 取回后**逐字落盘**，未做任何空白改动。
3. 表里的 sha256 是**本仓库落盘后的文件**摘要（用于发现后续被改动），不是上游文件的摘要。
   为了让它在任何机器上都可核，仓库根 `.gitattributes` 把 `crates/tools/forge-tests/asm/**`
   钉成 `text eol=lf`——否则 `core.autocrlf` 会在 Windows 上把工作区改成 CRLF，摘要就对不上了
   （语义不受影响：汇编器把 `\r` 当空白，但"逐字保留上游原文"这句话会不成立）。

## 计划补齐（需 TLS 的机器，跑 `fetch.ps1`）

| ISA | 语料 | 固定 ref | 许可 | 补进来解决什么 |
| --- | --- | --- | --- | --- |
| x86 | LLVM MC `llvm/test/MC/X86/**` **全量**（现在只取了两份 Intel 语法文件） | `llvmorg-19.1.0` | `Apache-2.0 WITH LLVM-exception` | 更多 Intel 语法形态与字节期望（AT&T 那批不进，见 README「边界」） |
| aarch64 | LLVM MC `llvm/test/MC/AArch64/**` **全量**（现在三份） | `llvmorg-19.1.0` | `Apache-2.0 WITH LLVM-exception` | 覆盖更多 A64 编码族与别名 |
| riscv64 | LLVM MC `llvm/test/MC/RISCV/**` **全量**（现在三份） | `llvmorg-19.1.0` | `Apache-2.0 WITH LLVM-exception` | 覆盖 RVC/RVV/伪指令与别名寄存器 |
| x86 | Intel XED `tests-syntax` + `bulk-tests`（`cmd`/`.reference`/`.hex`） | 固定 commit | `Apache-2.0` | 逐条 asm↔字节的第三方对照（CDN 拒发二进制，需真 TLS 通道） |
| x86 | NASM `test/**` + `golden/**`、YASM `modules/arch/x86/tests/**` | 固定 commit/tag | `BSD-2-Clause` | 真实项目写法的 Intel 语法与黄金输出 |
| x86 | binutils-gdb `gas/testsuite/gas/i386/intel*.s` **全量**（替换现有节选） | 固定 commit/tag | `GPL-3.0-or-later` | 消掉"节选 + 空白归一化"两个弱点 |

**不进解析档的语料（有意排除）**：`riscv-tests` / `riscv-arch-test` 的 `.S`——它们靠 C
预处理器（`encoding.h`）、链接脚本与 `tohost`/`spike` 自检协议，且正文来自 git 子模块；
本汇编器没有预处理器，执行通道也不是 `tohost`（见 `README.md` 的「边界」）。

## 许可与再分发

八份语料各自沿用上游许可（见上表：LLVM 六份 = `Apache-2.0 WITH LLVM-exception`，
GAS 一份 = `GPL-3.0-or-later`）。仓库根**没有**统一的 `LICENSE`/`COPYING` 文件，
所以 GAS 那份被放在**独立目录** `parse/x86/gnu-gas-intel/` 下，与本目录的
`PROVENANCE.md` 一起构成出处与许可记录；若将来要对外发行，请连同上游许可正文一并带出，
或把该目录替换为 `Apache-2.0`/`BSD-2-Clause` 的语料（NASM/XED）。
