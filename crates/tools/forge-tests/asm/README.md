# 真实汇编语料测试台（`forge-tests/asm`）

用**上游真实汇编语料**检验生成物的汇编器：能不能读真实世界的写法、编出的字节对不对、
编出来的程序**真跑**得起来吗。三件事各一个测试二进制，缺陷落在哪一档一目了然。

## 为什么要有它

谱内自测（`[[vectors]]`）与往返守卫只能证明"我们自洽"——它们用的都是**我们自己写的**
汇编文本。真实语料（GNU as / LLVM MC 的测试文件）是**别人写的**：寄存器别名、内存寻址
写法、立即数语法、伪指令、大小写习惯全都不一样。这件事只有拿真语料喂才看得见。

## 目录

```text
asm/
├── parse/<isa>/<suite>/**    # 上游语料原文（vendored，逐字不改；见 PROVENANCE.md）
├── exec/<isa>/<名>.s         # 我们写的真语法小程序 + 同名 .expect（args/ret）
├── ratchet/<isa>.txt         # 计数棘轮（各桶计数 + 人工核过的缺口样例）
├── fetch.ps1                 # 拉取/刷新语料（需 TLS，见「刷新语料」）
├── PROVENANCE.md             # 每个语料文件的出处/ref/许可/摘要
└── README.md                 # 本文件
```

## 三档测试

| 档 | 二进制 | 命令 | 证明什么 |
| --- | --- | --- | --- |
| 解析 | `tests/asm_parse.rs` | `cargo test -p forge-tests --test asm_parse` | 真语料的行能不能被 `Assembler::parse_insts` 吃掉（四桶 + 计数棘轮） |
| 编码对拍 | `tests/asm_encoding.rs` | `cargo test -p forge-tests --test asm_encoding` | 能解析的那些行，`encode()` 的字节是否与**上游注释里的期望字节**逐字节相等 |
| 执行 | `tests/asm_exec.rs` | `cargo test -p forge-tests --test asm_exec` | `asm/exec/**` 的小程序汇编→装进 `CompiledFunction`→**真跑**，断言返回值 |

跑一档不够看时加 `-- --nocapture`：`ASM-SUMMARY` / `ASM-ENCODING-SUMMARY` /
`ASM-EXEC-SUMMARY` / `ASM-*-SKIP` 都在 stdout 上（libtest 只吞**通过**用例的输出，
所以同时可用 `FORGE_ASM_EVENTS=<文件>` 落盘，见下）。`FORGE_ASM_ENCODING_CASES=1`
会把编码对拍档**逐条配对结果**（指令文本 = 上游字节 + 判定）打出来——配对规则出错时靠它定位。

## 解析档：四桶与计数棘轮

每一条候选指令行判进四个桶（判定实现 `src/asm/classify.rs`）：

| 桶 | 判定 | 门禁含义 |
| --- | --- | --- |
| `Parsed` | 线性扫描整条命中 | 正常 |
| `NoPrefix` | 失败，且没有任何候选的**首段**能对上（探针说"不可能"） | 记账（上游比我们全 / 别家方言） |
| `TailMismatch` | 失败，但某条候选的**首段**对得上——首段之后没对上 | **唯一红桶** |
| `CorpusOnly` | 失败原因是标签在别处（`UndefinedLabel`） | 记账（上下文不足） |

**桶判定不问"首词是不是助记符"**，而是问扫描器自己：`could_be_instruction`（生成物的线性
扫描探针）只跑扫描的第一步——"本 ISA 有没有哪条候选的首段能吃掉这段开头"。它是**必要
条件**：`false` ⇒ 这段一定不是本 ISA 的指令；`true` ⇒ 首段对得上，还得看整条。

> 为什么不用"助记符表"（v20 V9 首版的做法，已废弃）：那等于在生成期假设 `asm` 模板的
> **首个空白分隔词就是助记符**，再拿它当"本 ISA 有哪些指令"的清单。三条都不成立——v17 起
> 模板可以**操作数前置**（`asm = "{dst} = {src}"`，首"词"是操作数）、首段可以是**多 token
> 字面**（`lock cmpxchg [`，拆词就错）、空白分词与扫描器的 token 切分也不是一回事
> （`amoadd.w.aqrl` 是一个 Ident）。现在探针与 `assemble`/`parse_insts` 共用同一份候选、
> 同一个首段匹配器、同一个 lexer——**单一事实源就是扫描器**。

门禁 = **计数棘轮**：`asm/ratchet/<isa>.txt` 里的每个计数必须与实测**逐项相等**，
**两个方向都红**——多一个 = 汇编器出现新缺陷（或换了语料），少一个 = 修好了（该更新棘轮）
或漏跑了。这样"悄悄变绿"和"悄悄变红"都会拦下。

- 确认差异是预期的（例如换了语料）就重新刷棘轮：
  `$env:FORGE_ASM_WRITE_RATCHET = "1"; cargo test -p forge-tests --test asm_parse`
  （刷完**必须看 diff**：棘轮文件里带着红桶样例，那是给人核对的）；
- 机读记分板：每轮跑都刷新 `target/asm-suite/<isa>.json`；
- 事件落盘（CI 用）：`$env:FORGE_ASM_EVENTS = "target/asm-events.txt"`。

## 编码对拍档：字节 oracle 与"已知差异"清单

上游注释里的期望字节按**四种写法**抽出（见 `src/asm/encoding.rs` 模块文档）：
同行尾部、注释自带汇编文本、注释在指令**后**、注释在指令**前**。第四/第三种**逐文件声明**
（`Suite::encoding_sides`）——同一目录两种风格都有（`arm64-branch-encoding.s` 是"注释在指令后"，
riscv 与 x86 Intel 用例是"注释在指令前"），**任何"看第一条推断整篇"的启发式都会在
"首条指令没有期望注释"的文件上整体错位一条**（实测 `rv32i-valid.s` 第 15 行 `.Lpcrel_hi0: auipc …`）。

三种结果分开记（`asm/ratchet/encoding.txt`，与解析档同一套两个方向都红的纪律）：

| 结果 | 含义 | 门禁 |
| --- | --- | --- |
| 一致 | 我们编出的字节与上游逐字节相等 | 过 |
| `variants` | 字节不同，但两边都喂给**我们自己的解码器**后反汇编文本相同（同一指令的两种合法编码） | 记账 |
| `known` | 字节不同且不满足等价条件（常见于**我们解不出**的上游编码） | **逐条写进棘轮**：新增一条红、少一条也红 |

前缀被拆开的期望（`acquire lock add [rax], rax` 的 `[0xf2]` + `[0xf0,0x48,0x01,0x00]`）与
重定位形式（`[0xeb,A]`）不猜：前者整段丢掉并计入 `dropped`，后者**占一个配对位**但不产出
case（否则后面的用例会整体错位）。

## 执行档小程序约定

`asm/exec/<isa>/<名>.s` 是**真语法**汇编，写成各机 ABI 的**叶子函数**（不碰栈）；
同名 `.expect` 给 `args = …` / `ret = …`（`#` 开头的整行是注释）。

| ISA | 实参寄存器 | 返回值 | 结束 | 执行通道 |
| --- | --- | --- | --- | --- |
| x86 | `RCX`/`RDX`/`R8`/`R9`（Windows x64） | `RAX` | `ret` | 原生（**仅 windows-x86_64 宿主**） |
| riscv64 | `x10`…（LP64D） | `x10` | `ret` | QEMU system + semihosting |
| aarch64 | `x0`…（AAPCS64） | `x0` | `ret` | QEMU system + semihosting |

QEMU 路径：`$env:QEMU_RISCV64` / `$env:QEMU_AARCH64`，否则用默认安装路径
（`D:\Program Files\qemu\qemu-system-riscv64.exe` 等）。**缺通道 ⇒ 该 ISA 打印
`ASM-EXEC-SKIP <原因>` 并只做汇编（仍然要求汇编成功），不假绿**；只有在
"一条都没真跑成"时用例才红。

## 边界（明确不做，别当成漏做）

- **只接本汇编器支持的那一档方言**：x86 只接 Intel 语法；AT&T 语法（`movl $1, %eax`）
  不在范围内（这类行按 `NoPrefix`/`CorpusOnly` 记账）；
- **没有 C 预处理器**：`riscv-tests` / `riscv-arch-test` 的 `.S` 依赖 `cpp` 与子模块
  环境（`encoding.h`、`link.ld`、`spike`/`tohost`），**不进解析档**——它们的设计是
  "整程序 + 自检退出码"，与本 crate 的 QEMU semihosting 通道不是一路；
- **不做"随机指令逐条执行"smoke**：随机字节大概率是 trap/非法指令，噪声远大于信号；
  执行档只跑**人工写的、语义明确的**小程序；
- **不改语料、不改谱**：语料的注释前缀/一行多语句（arm64 的 `%%`）由 `src/asm/corpus.rs`
  的 suite 档案**归一化**，谱侧不动。

## 已知缺口（登记，供后续修谱）

这些是语料跑出来的真实差异（红桶 `TailMismatch` 的样例就写在 `asm/ratchet/*.txt` 里），
不是"语料不对"：

- riscv64：`%lo(2048)(x7)` 这类**重定位修饰的立即数**写法（`%lo`/`%hi`/`%pcrel_lo`）不支持；
- aarch64：`ret lr`（带操作数的两操作数形式）不支持，裸 `ret` 可以；
- x86：`gnu-gas-intel` 还剩两类——
  ① **无符号 imm32**（`add eax, 0x90909090`，8 行）：`imm32` 槽是**有符号**的
  （`−2^31..2^31−1`），放不下 `0x90909090`；放宽会连带影响解码的符号扩展口径
  （`-12` 与 `0xFFFFFFF4` 是同一批字节），需要"无符号字面量按字段位宽规范化"的设计，未做；
  ② **段寄存器 push/pop**（`push es` / `pop ds`…，7 行）：`[reg.*]` 里没有 16 位段寄存器组；
- x86：**无基址寻址**（`[-8]`、`[0x12345678]`）不支持——`MemRef.base` 是必需字段；
  解码侧对 `mod=00` + `SIB.base=101` 明确 fail-closed（见 `docs/reference/isa-dsl.md`
  的 `[conventions.mem]` 节）；
- x86（余下几条，来自 `intel-syntax-encoding.s`）：`cmpltps`（SSE 比较谓词别名）、
  `cmp eax, FOO` / `cmp eax, FOO[eax]`（`.set` 符号进立即数/位移）、
  `acquire/release lock add …`（锁前缀 + 内存序提示）；
  以及 `apx-rex2-format-intel.s` 整份（APX/REX2：`r16d`、`dword ptr [r16 + rax]` 等）。

**已经修掉的**（2026-10-03，字节 oracle 抓出来当天就改，逐条见 `CHANGELOG.md`）：

- x86：`xor/or/cmp/add/adc/sbb` 的 16/32 位目的地、`xor rax, 12` 的**符号扩展 imm8**
  短形式（原先出 7 字节 imm32，且解码器连 `48 83 /6 ib` 都解不出）、`shl edi, 1`（`D1 /4`）、
  `mov eax, 0x1234`（`B8+r id`）、`ret 8` / `retf` / `retf 8` / `pushf/popf/pushfw/popfw`；
  x86 的字节对拍从「6 条对上 / 5 条已知差异」变成「**27 条对上 / 0 条差异**」。
- riscv64：**ABI 别名寄存器**（`a2`/`s3`/`fp`…，走新的 `[reg.*].aliases`）与 RV64I 的
  `lwu` / `addiw`；顺带修掉 `lui`/`auipc` 的**立即数口径**（我们以前按"绝对值"解释、
  编码器再 `>>12`；GAS/LLVM 的立即数就是 **imm20 字段值**——`lui a0, 2` → rd = 8192）；
  解析档 `parsed` 12 → **72**、编码对拍 12 → **72 条逐字节全等**。
- x86（v20 V10，前两批）：**内存操作数的文本模板列表 + 尺寸前缀**（`[conventions.mem]`
  的 `templates` 列表与 `{size}` 组件）与**内存形式的 ALU 族**（`*_MR` / `*_R_MEM`
  各 8 条）→ `gnu-gas-intel` 解析档 `parsed` 0 → **16**、`llvm-mc` 31 → **32**；
  第三批补**8 位 ALU 族**（`*_MR_8` / `*_R_MEM_8` / `*_RM8_IMM8` 各 8 条）→ 再 → **40**，
  只剩上面的无符号 imm32 与段寄存器两类。

## 现有语料与计数

（`2026-10-03` 实测；解析档数值以 `asm/ratchet/<isa>.txt` 为准，编码档以
`asm/ratchet/encoding.txt` 为准。x86 两套是 v20 V10 补完内存模板、内存形式 ALU 族与
8 位 ALU 族之后的数）

| ISA | suite | 文件 | 行数 | parsed | no_prefix | tail_mismatch | corpus_only | 字节 oracle |
| --- | --- | --- | --- | --- | --- | --- | --- | --- |
| x86 | `gnu-gas-intel` | 1 | 64 | **40** | 6 | 15 | 3 | 无（GAS 用例不带期望字节） |
| x86 | `llvm-mc` | 2 | 447 | **32** | 29 | 50 | 336 | **有**：103 条期望 / **27 条对拍上 / 0 条差异** |
| riscv64 | `llvm-mc` | 3 | 503 | **72** | 38 | 31 | 362 | **有**：134 条期望 / **72 条逐字节全等** |
| aarch64 | `llvm-mc` | 3 | 420 | 37 | 45 | 71 | 267 | **有**：119 条期望 / **15 条逐字节全等** |

执行档：x86 3 条、riscv64 3 条、aarch64 2 条，**三架构都真跑通**（`ran=8 skipped=0`）。

## 刷新语料

`asm/fetch.ps1` 按固定 ref 拉取上游文件、算 sha256、打印可直接粘进 `PROVENANCE.md` 的表格行：

```powershell
pwsh crates/tools/forge-tests/asm/fetch.ps1 -Isa x86      # 只拉一套
pwsh crates/tools/forge-tests/asm/fetch.ps1 -List         # 列出各套的来源与固定 ref
```

**当前机器没有 TLS**（`curl`/`git` 都报 `SEC_E_NO_CREDENTIALS`），网络通道只有 `web_fetch`
（走 jsDelivr 的文本文件），所以语料是**一份份取回**的（`PROVENANCE.md` 逐条记了 URL 与
sha256）；`fetch.ps1` 供能联网的机器按目录**批量**补齐全量（LLVM MC X86/AArch64/RISCV 全量、
XED `tests-syntax`/`bulk-tests`、NASM `test/`+`golden`、YASM、GAS 全量 i386/aarch64）。

新增语料的三步：① 在 `src/asm/corpus.rs` 的 `SUITES` 里登记（key/目录/注释前缀/许可/来源，
`encoding:` 注释位置与 suite 缺省不同时再补 `encoding_sides`）；② 把文件放进
`asm/parse/<isa>/<suite>/`；③ 刷棘轮（解析档与编码档各一次）并在 `PROVENANCE.md` 记出处。
**许可**：LLVM MC 与 XED = `Apache-2.0 WITH LLVM-exception`，NASM/YASM = `BSD-2-Clause`，
GAS = `GPL-3.0-or-later`（上游各文件的许可，逐条见 `PROVENANCE.md`）。
