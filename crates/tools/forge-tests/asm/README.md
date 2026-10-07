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
├── fetch.mjs                 # 拉取/取舍/写出处（Node，一条命令；见「刷新语料」）
├── PROVENANCE.md             # 每个语料文件的出处/ref/许可/摘要（**由 fetch.mjs 生成**）
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
| `CorpusOnly` | 失败原因是标签在别处（`UndefinedLabel`），**或者这行根本不是指令**（注释/伪指令/宏/预处理——语料抽取时记的 `skipped`，也照算进 `lines`） | 记账（上下文不足 / 语料特性） |

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
不是"语料不对"。**x86 现在没有汇编器侧缺口**（连"设计内非缺口"都在下一节列明），
riscv64 只剩一条登记为设计取舍；aarch64 有一条**已定位、未落地的机制缺口**：

- aarch64（重定位修饰族，**已修**，2026-10-07 第十一轮）：`add x0, x0, #:lo12:sym`、
  `ldr w0, [sp, #:lo12:hiddenvar]`、`movz x2, #:abs_g0:sym` 这类**修饰 + 符号**的写法
  （语料 **382 行、47 个修饰名**，横跨 `basic-pic.s`、`elf-reloc-*.s`、`tls-*.s`、
  `jump-table.s`、`arm64-ilp32.s` 等十几个文件）。落地时踩到**三件事**，都已处理：
  ① **生成侧的真缺陷**：`parse_insts` 的标签识别把"第一个冒号之前的一切"当标签定义——
     于是 `add x8, x8, :lo12:sizes` 被切成"标签 `add x8, x8,` + 指令 `lo12:sizes`"并报
     `no matching instruction`。**这才是这族 382 行一直落红的根因**（语料抽取侧有"名字不含空白"
     的守卫，生成侧没有）。修法 = 同一口径的"纯标识符才算标签"判据。
  ② **DSL 新能力 `require_symbol`**：符号位置必须开在 `unit == 1` 的槽上（校验硬要求），
     而立即数槽的候选特异性**按接受域**裁决——unit-1 槽比 unit-8 窄 ⇒ 优先命中，
     会把数值写法接走（实测 `ldr x0, [x1, #8]` 编成字段 8 而不是 1）。`require_symbol = true`
     让该槽在"没记到符号"时**直接不匹配**，数值写法自然落回数值槽。
     判据在 `__imm` **内部**判——它成功时会提交并清空 `__SYMREF`，外层测长度增量恒为 0（踩过）。
  ③ **形状必须不同**：修饰的 `#` 放进 `[[conventions.imm_fn]]` 的 `text`（`#:lo12:{0}` 与
     `:lo12:{0}` 各一条），指令模板**不带 `#`** ⇒ 重定位变体的文本形状与数值变体不同。
     同一形状的多候选里分派会**提交到排第一的那条**（不按操作数失败回退）——实测形状相同会
     互相遮蔽（加了重定位变体后 `add W0, W1, #0` 反而装配不出来）。
  另外：这类形态是**仅装配**的（渲染出的无修饰文本由数值兄弟接手），生成期自测对它们
  **跳过文本闭环**、只测编解码闭环。上游口径也对得上：`elf-reloc-addsubimm.s` 的 OBJ 期望写着
  `91000062 add x2, x3, #0`——**未重定位字段就是 0**，与"符号引用按 0 参与算术"一致。
  结果：aarch64 `parsed` 1572 → **1618**、红 1877 → **1734**、`corpus_only` +97（未定义符号现在
  被正确分类成"上下文不足"而非缺陷）、编码 `known` 仍 **53**（无字节回归）；
  **`arm64-ilp32.s`、`basic-pic.s`、`jump-table.s`、`tls-add-shift.s` 四个文件整文件转绿**。
  仍缺（留在红桶）：`add x2, x3, #:lo12:sym, lsl #12`（移位立即数族）、`str q0, [sp, …]`（FP 视图）、
  `movz`/`movk`/`movn` 的 `abs_g*` 形态、`adrp`/`adr` 的 `:got:` 形态、`ldr x0, =sym`（字面池伪指令）。

- riscv64（红桶 **1** 条，已登记为设计取舍）：
  - `jal a0, a0`：**不是**"两寄存器形式的 `jal`"——RISC-V 的 `jal rd, symbol` 只收符号，上游把它当
    **未定义符号 a0**（`CHECK-OBJ: R_RISCV_JAL a0`）。我们拒了"寄存器样子的 ident"当符号（以免
    `jmp rax` 这类走错候选），所以它落在红桶——**不改**（改法牵动所有 ISA 的 label 槽）。
  - **已修**（2026-10-04）：三操作数 `jalr rd, rs1, imm`、位置符号 `.`（回填成"当前指令自身的块下标"）、
    `fence` 的**字母集合**（`kind = "bits"` + `[conventions.bitsets.<表名>]`）、以及**立即数位置上的
    符号引用**（`symbols = true`：`%hi(foo)` 这类未定义符号现在报 `UndefinedLabel`（准确诊断）、
    `.Lpcrel_hi0` 这类已定义局部标签真装配；顺带修掉"点开头的标签名被切成 `.` + `Lp`"的**词法缺陷**）。

- riscv64（第二十六批：**浮点 `rm` 成为操作数 + 省略写法的缺省值按**字节证据**定**）：
  见下面「已经修掉的」里的第二十六批——里面记了两件不在计划里的事：一是"省略 rm"的缺省值
  在**算术族是 `dyn`、在 `fcvt` 族是 `rne`**（从语料自带的期望字节**量**出来的，不是猜的），
  二是 `FCVT.S.D` 的 `rs2` 必须是 `1`（我们的旧声明与 `fcvt.d.s` 同形，编出来少一位）。
  结果：`rv32f-valid.s` / `rv32d-valid.s` / `rv64d-valid.s` / `fp-default-rounding-mode.s`
  **整文件转绿**，且它们的上游期望字节**逐字节全等**（`rv32d-valid.s` 49 条期望 / 38 条对拍上 /
  `known` 0）。
- aarch64：**没有剩余缺口**——语料里本汇编器能认出来的写法全部通过（`parsed` 197、
  红桶 0、编码对拍 **125/125** 逐字节全等）；余下 390 条 `no_prefix` + 1450 行 `corpus_only`
  是"上游比我们全"（SVE/SME/NEON 之外那批不收，见下节）与语料特性（伪指令/宏/一行多语句/
  跨文件标签）。已修的各族见下面「已经修掉的」里的第十二～十六批。
- x86：**语料全部归因完毕**——`llvm-mc` 档 `parsed` 116 + `corpus_only` 354 =
  471 行全覆盖（`no_prefix` 1），红桶 **0**；编码对拍 **checked 110 / unparsed 0 /
  dropped 0 / known 0**。`gnu-gas-intel` 余下的 7 条红桶是 32 位模式专有语法
  （下面「不是缺口」那节）。

- riscv64（v20 V10，第十七批：**`fence` 的字母集合 + 三操作数 `jalr` + 位置符号 `.`**）：
  ① `fence pred, succ` 的 pred/succ 是 i/o/r/w 的**组合**（`iorw`、`io`、`r,w`、`w,ir`）——
  这次做成**通用能力**：`[conventions.bitsets.<表名>]` 是数据（名字 → 位），槽写
  `kind = "bits"` + `table`；解析按名字**贪心最长匹配**拼接（`ir` = i|r），编码取位或，
  反汇编按名字字典序拼回。顺带把 `fence`/`fence.i`/`fence.tso` 从**整字常量**改成字段形——
  整字常量会与 `fence pred, succ` 的 opcode 边在 [0,7) 上判成同一条 trie 边（生成期直接报歧义）。
  ② `jalr rd, rs1, imm`（三操作数，RISC-V 手册的规范形）与两操作数写法**同编码**，只是文本不同；
  ③ 位置符号 `.`：`jal zero, .`——`__label` 把它记成**自引用**（空名），两遍回填时换成"当前指令
  自身的块下标"，与"标签回填块下标"同一口径。
  结果：riscv64 `parsed` 129 → **135**、红桶 13 → **7**，字节对拍 **129 → 134 条逐字节全等
  （`unparsed` 5 → 0）**——即"有期望字节的用例全部对上"（`known` 仍 0）。

**不是缺口：`gnu-gas-intel` 余下的是 32 位模式专有语法**（`asm/parse/x86/gnu-gas-intel`
摘自 GAS 的 32 位 Intel 用例）。`push es` / `pop ds` 这类**段寄存器 push/pop 在 64 位
模式下不存在**（长模式只留 `fs`/`gs`），`daa`/`das`/`aaa`/`aas` 也已在长模式移除——
本谱是 `[meta].mode = 64`，**照抄它们等于给 x86-64 编出非法指令**，因此明确不做。
`gnu-gas-intel` 剩下的 7 条红桶全是这类段寄存器 push/pop，已在此登记为"边界"而非"缺口"。

**已经修掉的**（2026-10-03，字节 oracle 抓出来当天就改，逐条见 `CHANGELOG.md`）：

- x86：`xor/or/cmp/add/adc/sbb` 的 16/32 位目的地、`xor rax, 12` 的**符号扩展 imm8**
  短形式（原先出 7 字节 imm32，且解码器连 `48 83 /6 ib` 都解不出）、`shl edi, 1`（`D1 /4`）、
  `mov eax, 0x1234`（`B8+r id`）、`ret 8` / `retf` / `retf 8` / `pushf/popf/pushfw/popfw`；
  x86 的字节对拍从「6 条对上 / 5 条已知差异」变成「**27 条对上 / 0 条差异**」。
- riscv64：**ABI 别名寄存器**（`a2`/`s3`/`fp`…，走新的 `[reg.*].aliases`）与 RV64I 的
  `lwu` / `addiw`；顺带修掉 `lui`/`auipc` 的**立即数口径**（我们以前按"绝对值"解释、
  编码器再 `>>12`；GAS/LLVM 的立即数就是 **imm20 字段值**——`lui a0, 2` → rd = 8192）；
  解析档 `parsed` 12 → **72**、编码对拍 12 → **72 条逐字节全等**。
- x86（v20 V10，四批）：
  ① **内存操作数的文本模板列表 + 尺寸前缀**（`[conventions.mem]` 的 `templates` 列表与
  `{size}` 组件）与**内存形式的 ALU 族**（`*_MR` / `*_R_MEM` 各 8 条）→ `parsed` 0 → **16**、
  `llvm-mc` 31 → **32**；
  ② **8 位 ALU 族**（`*_MR_8` / `*_R_MEM_8` / `*_RM8_IMM8` 各 8 条）→ **40**；
  ③ 立即数槽的 **`wrap`**（字面量按 W 位位模式读，见 `docs/reference/isa-dsl.md`
  的「立即数字面量的两种读数」）→ **48**：`add eax, 0x90909090` 这类无符号写法能汇编，
  而 `add rax, -12` 的文本与字节不变；
  ④ 一元 **`inc`/`dec`**（`FF /0`、`FF /1`、`FE /0`、`FE /1`，4 条）→ **50**：
  `gnu-gas-intel` 的 64 条里现在只剩 7 条红桶（全是 32 位模式专有的段寄存器 push/pop）、
  4 条 `no_prefix`（`daa`/`das`/`aaa`/`aas`，同样长模式已删）与 3 条 `corpus_only`；
  ⑤ **无基址（绝对）寻址**：`[conventions.mem]` 的模板**不写 `{base}`** 就是"这条写法
  没有基址"（`MemRef.base` 改 `Option<Reg>`），x86 用 `mod=00` + `SIB.base=101` + disp32
  表示它 → `llvm-mc` 解析档 32 → **33**、字节对拍 27 → **28 条逐字节全等**（`known` 仍 0：
  `movsd XMM5, QWORD PTR [-8]` 与上游 `f2 0f 10 2c 25 f8 ff ff ff` 一致）。
- x86 / riscv64（v20 V10，第六批：**符号常量 + 地址尺寸前缀**）：

  ⑥ **符号常量**（`.equ`/`.set` 同一个伪指令）与**位移与立即数共用一套值语法**：位移改走
  `__expr`（原来只认字面量），一元 `+` 变恒等（`lwu x2, +4(x3)`）——**语料侧的根因**是抽取
  只喂标签、不喂符号定义，且整篇一起喂会把 `rv32i-valid.s` 里前后两次 `.equ CONST`（30 → 16）
  解成同一个值，所以符号定义**按行号**进上下文（只喂本行之前的，标签仍整篇喂）；
  ⑦ **地址尺寸覆盖前缀**（`[conventions.prefix_scan]` 的 `addr32` 效果 = x86 `0x67`）：
  内存 base/index 宽度 ≠ `[meta].addr_width` 时编码器发它、解码器按覆盖宽度建 base/index
  （`[eax]` ≠ `[rax]`；`[base]` 简写的槽也按地址类解）。改前 `add eax, [eax]` **静默编成**
  `03 00`（真值 `67 03 00`：地址尺寸错，字节却认得出），`gnu-gas-intel` 那 32 条 32 位地址
  行全靠它才对得上。

  结果：解析档 `gnu-gas-intel` 50 条不变（但 32 位地址那些行的**字节**由错转对）、
  `llvm-mc` 33 → **35**（`cmp eax, FOO` 与 `cmp eax, FOO[eax]` 都进 `parsed`）、
  riscv64 72 → **82**；字节对拍 x86 28 → **30 条逐字节全等**、riscv64 72 → **82 条逐字节全等**
  （`known` 全 0）。

  同一批还修掉一处**配对假象**：`intel-syntax-encoding.s` 前 94 行是"注释在指令前"、
  尾部两条是"注释在指令后"，整篇一个风格会把 `[0x83,0xf8,0x02]` 配给后一条指令，
  凭空造出一条字节差异——`encoding_sides` 因此改成**带起始行的逐段声明**。

- x86（v20 V10，第七批：**内存形式的 mov 族**）：补 6 条 `MRR_MEMREF_AUTO`/`MRR_MEMREF`
  内存形式的指令——`MOV_R_MEM_AUTO`(8B) / `STORE_MEM_R_AUTO`(89) / `MOV_R_MEM_8_AUTO`(8A) /
  `STORE_MEM_R_8_AUTO`(88) / `MOVSXD_R_MEM`(63) / `XCHG_MEM_R_AUTO`(87)，加一个 16/32 位
  寄存器槽 `gpr24`。改前 `mov eax, [rbx+8]` **一条候选都没有**：`MOV_R_MEM` 系列是 `[{reg}]`
  简写（给 IR 的 vreg 基址用，没有位移/尺寸前缀），`MOV64_RM`/`MOV64_MR` 又固定 `opsize = 64`。
  现在 8/16/32 位的 `mov r, [mem]` 与 `mov [mem], r` 全部走**完整内存模板**（位移、`index*scale`、
  尺寸前缀、无基址、高编号寄存器、66 前缀），`movsxd r64, dword ptr [mem]` 与带位移的
  `xchg [mem], r` 也补上了。

  这批**不动语料棘轮**（vendored 语料里没有这些写法，全量语料里遍地都是），所以证据换成了
  **真执行**：`asm/exec/x86/` 新增两个小程序（`mem_mov32.s` 32 位存/读、`mem_mov16.s` 16 位
  存/读），`ASM-EXEC-SUMMARY ran=8 → **10** skipped=0`——新编码在 x86 宿主机上**真跑**出期望值。

- x86（v20 V10，第八批：**SSE 比较谓词族 + 别名**）：`cmpltps XMM2, XMM1` 一直是登记的缺口，
  根因是谱里根本没有 SSE 谓词比较指令。补 `CMPPS`/`CMPPD`/`CMPSS`/`CMPSD_SCALAR`
  （`0F C2 /r ib`：`ps`/`pd`/`ss`/`sd` 靠强制前缀区分，谓词是立即数第 3 操作数），八个经典
  packed 别名（`cmpeq/lt/le/unord/neq/nlt/nle/ord` × `ps`/`pd`）用 `[[pseudo]]` **文本展开**
  表达（一行别名换一行 `cmpps {a}, {b}, <谓词>`）。结果：`llvm-mc` 解析档 `parsed` 35 → **36**
  （`cmpltps` 从 `no_prefix` 桶进 `parsed`）、字节对拍 x86 `checked` 30 → **31**（上游
  `0F C2 D1 01` 逐字节相同，`known` 仍 0）。

- riscv64（v20 V10，第九批：**立即数修饰 `%hi`/`%lo`**）：这批**没有在生成器里加任何 RISC-V 分支**——
  改成通用的 **`[[conventions.imm_fn]]`**（谱声明的数据：`text` = 源文本形态，`expr` = 值语义），
  RISC-V 的 `%hi(x)`/`%lo(x)` 只是 `isa/riscv64.toml` 里的两行；ARM 风格 `:lower16:x`（无括号、掩码）
  在夹具 `demo.toml` 里声明，同一机制、零代码改动（见 `docs/reference/isa-dsl.md` 的
  「立即数修饰」节与 `demo_tests::imm_fn_is_spec_data_not_isa_knowledge`）。
  结果：riscv64 解析档 `parsed` 82 → **89**、红桶 22 → **15**，字节对拍 `checked` 82 → **89
  条逐字节全等**（上游 `lui a0, %hi(2)` = `37 05 00 00`、`jalr a0, %lo(2048)(a1)` = `67 85 05 80`…，
  `known` 仍 0）。

- riscv64（v20 V10，第十批：**字节/半字访存 + W 立即数移位 + 一元 `!`**）：`lb`/`lh`/`lbu`/`lhu`/`sb`/`sh`
  （与 `lw`/`sw` 同形，只差 `funct3`）与 `slliw`/`srliw`/`sraiw`（opcode 0x1B + `shamt_w`）；
  顺带修掉一处**编码错误**：`SLLW`/`SRLW`/`SRAW` 原先被声明成**立即数**移位（0x3B + shamt），
  而 ISA 里它们是 **R 型寄存器**移位——立即数那条是 0x1B 的 `slliw`/`srliw`/`sraiw`。
  三条改成 `WW` 模板的行（与 `ADDW`…`REMUW` 同形）。另外求值器补一元 `!` = **逻辑非**
  （上游期望字节实证：`lh t1, !1(zero)` 位移是 **0**，不是按位取反的 -2）。
  结果：riscv64 解析档 `parsed` 89 → **115**、红桶 15 → **13**、`no_prefix` 38 → **14**，
  字节对拍 89 → **115 条逐字节全等**（`known` 仍 0——两族新指令的每个字节都与上游注释相同）。

- riscv64（v20 V10，第十一批：**清空 `no_prefix`**）：`BGEU`（`BB` 模板少的一行）、Zicsr 六条
  （`CSRRW`/`CSRRS`/`CSRRC`/`CSRRWI`/`CSRRSI`/`CSRRCI`）、`FENCE_I`/`FENCE_TSO` 整字常量，以及别名 `unimp`。
  两个坑值得记：① CSR 号是 12 位**无符号**（0xfff = 4095），用有符号 `imm12` 槽会让 `csrrw t0, 0xfff, t1` 报越界 ⇒ 新增 `csr12` 槽；
  ② `unimp` 的真实位型就是 `csrrw x0, cycle, x0`（0xC0001073）——它是**别名**不是新编码，
  声明成整字指令会与 `CSRRW` 撞车（解码器按"编码不相交"建 trie 直接报错）⇒ 用 `[[pseudo]]` 展开一条。
  结果：riscv64 `parsed` 115 → **129**、**`no_prefix` 14 → 0**、红桶 13 不变，字节对拍 115 → **129 条逐字节全等**（`known` 仍 0）。

- aarch64（v20 V10，第十二批：**系统/异常族 + 逻辑（移位寄存器）族**）：
  ① 系统/异常族 `SVC`/`HVC`/`SMC`/`BRK`/`HLT`/`DCPS1..3` + 整字常量 `ERET`/`DRPS`
  （`SYSEXC` 顶层写成 11 位——写成 16 位会与 `imm16` 在 bit16-20 重叠，派生守卫当场抓到）；
  ② 逻辑族的移位后缀（`, lsl/lsr/asr/ror #n`，8 助记符 × 4 移位 × X/W = 64 条）与
  `ands`/`bics` 的无后缀形态 4 条——**纯谱数据**：移位种类是 2 位常量（`fields.shift`）、
  移位量是操作数槽（X 6 位 / W 5 位，5 位槽就是 A64 "32 位形式 imm6<32" 这条约束）。
  ③ 顺带修掉两处 `#` 被当注释（语料档与谱各一处）与生成器的**位 trie `else if` 链**缺陷：
  同一节点上两条边可以"位区间不相交却同时命中一个字"（`bics` 的 N 位 [21,22) 对逻辑族的
  移位 [22,24)），`else if` 会让前一条边吞掉后面的边（`bics x0, x1, x2` 编得出、解不回），
  改成平铺 `if` 后按声明序逐条尝试。结果：aarch64 `parsed` 47 → **115**、`no_prefix` 35 → **11**、
  红桶 71 → **27**，字节对拍 25 → **93 条逐字节全等**（`known` 仍 0）。

- aarch64（v20 V10，第十三批：**分支目标按字节**）：`b #28` / `cbz w1, #28` 这类写法的偏移是
  **字节**（`28 / 4 = 7` 进 imm26），而槽原先直接收原值——于是 >imm26 的合法偏移被判越界、
  能编的又编成错的数。本次给 `[[operand_slots]]` 加**源值单位** `unit`（本例 `unit = 4`）：
  值域按源单位给、非整数倍不匹配、解码乘回来（`Inst`/`disassemble` 始终是字节）、符号标签
  回填"块下标 × unit"。这与宿主 `Arm64RelocPatcher` 的 `offset >> 2` 是同一口径，于是
  "汇编器里的数字写法"与"编译器打补丁"不再各说各话（`unit` 只实现于定宽/mixed 编解码，
  `prefix_scan` 写它会在校验期 fail-closed）。另补 `[meta].imm_prefix = "#"`——A64 的模板
  虽然都写 `#{imm}`，但**分支目标槽前面没有字面 `#`**，前缀得由这条声明来吃。
  结果：aarch64 `parsed` 115 → **123**、红桶 27 → **19**，字节对拍 93 → **98 条逐字节全等**
  （`known` 仍 0；`cbz w20, #1048572` 这类上游注释里的期望字节原样对上）。

- aarch64（v20 V10，第十四批：**`tbz`/`tbnz`——一个值摊到多个字段**）：位序号是 6 位值，却被
  拆到**两个不相邻**的位域（`b40`[23:19] + `b5`[31]）；DSL 此前只能"位域 ← 常量或操作数"，
  没有"一个值 → 多个字段"。本次给 `[[operand_slots]]` 加 `encode` + `fields`：方案由谱按名
  选择（`slice` = 位切片：按声明序从值的最低位切起、每段长度 = 位域宽度，解码反向拼回；校验
  期要求 Σ宽度 == 槽宽），实现由 DSL 提供——与 `kind = "cond"`/`wrap`/`unit` 同一类"能力是
  声明的"。表单 `operand_fields` 里这一项写**首字段**，其余字段由槽声明。
  结果：aarch64 `parsed` 123 → **130**、**`no_prefix` 11 → 4**、红桶 19 不变，字节对拍
  98 → **100 条逐字节全等**（`known` 仍 0——上游注释里 `tbz w3, #5, #32764` =
  `[0xe3,0xff,0x2b,0x36]` 与 `tbnz x3, #8, #-32768` = `[0x03,0x00,0x44,0x37]` 都原样对上）。

- aarch64（v20 V10，第十五/十六批：**逻辑（立即数）族 + 收尾三条写法——aarch64 语料清零**）：
  ① **逻辑（位掩码）立即数**：值是"一段连续 1 循环填充"的位模式，编码成 N/immr/imms 三分量
  ——位切片表达不了这种**非线性**映射，于是给 `encode` 加第二个方案 `logical_imm`
  （`fields` 依序 = N(1)/immr(6)/imms(6)，槽宽 = 元素宽度 32|64；不可编码的值编码期报错）。
  落地 `and`/`orr`/`eor`/`ands` × X/W = 8 条。两个坑：顶层要用 **op9**（[31:23]）而不是 op8
  ——用 op8 会与 `MOVZ/MOVN/MOVK` 的 op9 在 bit23 上撞成同一条 trie 边（0x1A4 vs 0x1A5）；
  W 形式要 `wrap + min = -2^32` 才收得下 `#~(0xfe<<24)` 这种按位取反写出来的负数。
  ② 收尾三条写法（**都是纯数据**）：`ret xN`（第二条 RET，rn 成操作数；解码靠"叶 arm 先试 +
  平铺 if"回退）、`b.al`/`b.nv`（A64 保留码，上游汇编器收）、裸 `dcps1/2/3`（= `dcpsN #0`，
  声明成 imm16==0 的叶 arm——试过 `[[pseudo]]`，但伪指令的前导字面与指令重名会被校验期拒）；
  另加 `[reg.*].aliases` 的 `XZR`/`WZR`（31 号在零寄存器语境下）与 `LR`，`orr w8, wzr, #0x1`
  与 `ret lr` 才能解析。
  结果：aarch64 `parsed` 130 → **153**、**`no_prefix` 4 → 0、红桶 19 → 0**（语料里能认出来的
  写法全部通过；余下 267 行是语料特性/伪指令/宏），字节对拍 100 → **118 条逐字节全等**
  （`known` 仍 0）。

- x86 / riscv64（v20 V10，第十八批：**APX/REX2 + 真实形态补齐——x86 红桶清零**）：
  ① **REX2 / EGPR**：四个 GPR 宽度组扩到 32 项（`r16`..`r31` 四个视图），编码器见任一 GPR
  字段索引 ≥16 就改发 `D5` + payload（`M0 R4 X4 B4 W R3 X3 B3`，`M0` **取代 `0F` 字节**）；
  解码侧在 `M0=1` 时补一个虚拟 `0F` 再走原派发树（消费长度减一），派发树一行不改。
  `[[conventions.prefix_scan]]` 的 `rex2` 效果吃两个字节。新键 `[reg.<组>].alloc_count`
  （`[reg.gpr8]` = 16）：EGPR **能编码不能分配**——分配器一用 r16+，JIT 产物在没有 APX 的
  机器上就是非法指令（全量套件实测 `STATUS_ILLEGAL_INSTRUCTION`）。
  ② 补 **16/32 位 `mov` 的 MR 形态**（`89`，`MOV_RM_R_24`）：上游 LLVM 对 reg-reg 用 MR，
  我们原先只给 64 位建了它 ⇒ 那类上游编码**解不回来**。
  ③ 补 APX 语料用到的**真实形态**（14 条）：一元族内存形态 8 条（`NOT/NEG/INC/DEC/MUL/IMUL/
  DIV/IDIV_MEM32`，模板里写死 `dword ptr`）、`MUL_RM`/`IMUL_RM`（`F7 /4,/5`）、
  `IMUL_R_MEM`（`0F AF` + 内存源）、`NOP_RM`/`NOP_MEM32`（`0F 1F /0`）、`MOVSXD_R_RM32`
  （真实汇编的 32 位源写法）。顺带修掉一处**解码树漏洞**：固定扩展码（`F7 /2` 的 2）
  的守卫只加在寄存器分支、内存分支漏了 ⇒ `neg dword ptr [rax]` 解成 `NotMem32`。
  结果：`llvm-mc` 解析档 `parsed` 36 → **95**、**红桶 55 → 0**、`no_prefix` 20 → **16**；
  字节对拍 x86 `checked` 31 → **90 条逐字节全等**、`known` 全 0。

- x86（v20 V10，第十九批：**助记符条件后缀 → 语料里带期望字节的用例全部对上**）：
  上游写 `sete r16b` / `cmovl eax, r16d`（助记符**自带条件**、没有 `cc` 操作数），而我们只建了
  通用形态（`setcc {dst}, {cc}` / `cmovcc {dst}, {src}, {cc}`，供 lowering 用）。两条"把它做成
  指令"的路都被**解码树**挡住，原因写在 `isa/x86.toml` 注释里：① 把 `{cc}` 嵌进助记符要扫描器
  让"紧贴操作数的字面段"按 ident 前缀匹配（`sete` 是一个 token）；② 按条件码建**精确** opcode
  （`0F 94`）会与通用形态的**掩码**边（`0F 9x`）重叠——那是真歧义（同一批字节两种 `Inst` 形状），
  `check_dec_trie_overlaps` 直接拒。
  **解法是既有机制**：`[[pseudo]]` 文本展开（分派键是**整词** `sete`/`cmovl`，不产生解码 arm；
  SSE 的八个比较别名就是这么做的）。配套补三条**同掩码**的兄弟指令（`SETCC_RM8_B` 8 位名、
  `SETCC_R_MEM` 内存、`CMOVCC_R_MEM`；同掩码 ⇒ 归到同一解码节点、按声明序试，靠 mod 守卫区分，
  互不遮蔽），并把 `CMOVCC_R_RM` 从固定 64 位放宽到 16/32/64。
  顺带修掉同一族的一处**恒假守卫**：`opsize = 8`（`setcc byte ptr`）会生成 `__opsize == 1` 的
  解码条件，而 `__opsize` 只可能取 2/4/8 ⇒ arm 永远解不出来（8 位没有 opsize 前缀可查——
  与 v20 V10 加 8 位 ALU 族时实测过的那条同源，这次补在 `Opsize::Reg` 分支上）。
  结果：`llvm-mc` 解析档 `parsed` 95 → **109**、`no_prefix` 16 → **2**（只剩两条 lock 提示前缀）；
  编码对拍 **checked 104 / unparsed 0 / known 0**——**语料里带期望字节的用例全部逐字节对上**。

- x86（v20 V10，第二十批：**锁 + 内存序提示 → x86 语料全部归因完毕**）：
  `acquire lock add [mem], r` = `F2` + `F0`、`release lock add …` = `F3` + `F0`——**两条前缀**，
  于是 `EncKeys.prefix` 收**列表**（`prefix = ["0xF2", "0xF0"]`）：编码按书写序发
  （66 → 前缀 → REX → opcode），解码逐个按**前缀扫描标志**判。
  改这里踩到一个**必须保留的旧语义**：单条前缀值为 `0` 时生成的"**没有前缀**"断言
  （`!__p66 && !__pF0 && !__pF2 && !__pF3`）不能省——SSE 的 `66` 变体（`ADDPD`）与无前缀
  变体（`ADDPS`）同 opcode，少了它后者会先命中前者的字节（实测 `spec_addpd` 解成 `Addps`）。
  结果：`llvm-mc` 解析档 `parsed` 109 → **111**、**`no_prefix` 2 → 0**（该档 447 行 = 111
  parsed + 336 corpus_only，全部归因）；字节人工核对 `f2 f0 48 01 00` / `f3 f0 48 01 00`
  与上游一致。

- x86 / aarch64（v20 V10，第二十一批：**字节对拍三架构全清**）：
  ① **长编码拆行的期望**：上游把长编码写成**多条**只有字节的注释（`acquire lock add …` 是
  `[0xf2]` + `[0xf0,0x48,0x01,0x00]`；`pushf`/`popf` 是"两条 CHECK 后面跟两条指令"）——
  抽取器原来按"run ≥ 2 就整段丢掉"保守处理。现在按**声明的侧**处理，且**只在块内相邻**：
  注释在指令**后** ⇒ 按序**拼接**成一份期望（绑给紧邻的前一条指令）；注释在指令**前** ⇒
  按序 **1:1** 配给紧随其后的同样多条指令；绑不上就整块丢掉、后续不错位（三条单测钉住）。
  `intel-syntax-encoding.s` 因此补上逐段声明（1..53 前、54..60 后、61..97 前、98.. 后）。
  ② **十六进制立即数是位模式**：词法器原来按 `i64::from_str_radix` 读，64 位槽写不了
  `#0xfffffffffffffff0`（= -16）——aarch64 的 `and sp, x5, #0xfffffffffffffff0` 报
  "bad hex immediate"。改成按 `u64` 读、再按位重解释（二进制同理；十进制仍是**值**）。
  结果：编码对拍 x86 `cases` 104 → **110**、`dropped` 8 → **0**；aarch64 `checked` 118 → **119**、
  `unparsed` 1 → **0** —— **三架构 `cases = checked`、`known` 全 0**（语料里每一条期望字节都对上）。

- x86（v20 V10，第二十二批：**`movzx`/`movsx` 的内存源形态——最后一个已登记缺口**）：
  `movzx eax, byte ptr [rbx]` 的**源宽度只能由尺寸关键字给**（`0F B6` 与 `0F B7` 的差别在
  操作码，没有任何寄存器能表达源宽），而 `{size}` 按设计只是"给人读的提示"、不进类型签名 ⇒
  原来那对 `MOVZX_R{8,16}_MEM`（`[src]` 简写、asm 里**没有关键字**）文本一模一样，只能挂在
  歧义名单上。**修法用既有机制**：把尺寸关键字写成**模板字面量**（`asm = "movzx {dst}, byte
  ptr {mem}"`，与 `NOT_MEM32` 一族同款）——两条写法文本不同、汇编各自命中，**渲染也带关键字**
  ⇒ `disassemble → assemble` 闭合。补 4 条完整内存模板形态（`MOVZX/MOVSX_MEM{8,16}`）；
  简写那对保留（lowering 的零扩展加载要它：`{0}` 是地址 vreg ⇒ 绑寄存器槽），顺带修掉它的
  一处**潜在错编码**：`MOVZX_R16_MEM` 把 `66` 写死了（那是"16 位目的"的编码），配 64 位目的
  会编出 `66 48 0F B7`——现在宽度一律由目的寄存器驱动（`opsize = "s0"` ⇒ `48 0F B7` ✓）。
  另外把简写的基址槽收窄成单一 64 位地址类（多类槽会被**生成期采样器**强推 2/4 字节**地址**，
  那是无意义的组合：地址宽度不是数据宽度）。落地 4 条谱内向量。

- riscv64（**语料抓出来的真缺陷**，第二十三批：`clz`/`ctz`/`cpop`/`rev8` 的编码错了）：
  这四条 Zbb 指令原先写成 **R 型**（`opcode = 0x33` + `funct7`），实际是 **OP-IMM**
  （`opcode = 0x13` + 12 位立即数：`clz = 0x600` / `ctz = 0x601` / `cpop = 0x602` /
  `rev8 = 0x6b8`）。症状由上游语料**直接暴露**：`rv64zbb-valid.s` 的 `rev8 t0, t1` 期望
  `0x93,0x52,0x83,0x6b`，我们出 `0xb3,0x52,0x83,0xd0`；更糟的是**原编码与 `rol` 撞车**
  （同 `funct3=1`/`funct7=0x30`，只差 `rs2`），我们自己的解码器把 `clz X5, X6` 的字节解成
  `rol X5, X6, X0`——也就是说这三条**编出来解不回自己**。修法就是把 form 换成 `I` 型 +
  固定 `imm12`（编码键机制本来就有），谱内 4 条向量同步改成上游字节；修完
  `rv64zbb-valid.s` / `rv64zbkb-valid.s` 从"有字节差异"变成**逐字节全等**、进了 vendored 集。

- riscv64（第二十四批：**浮点寄存器的 ABI 别名**）：`[reg.gpr8.aliases]`（`a0`/`sp`/`fp`…）
  一直有，`[reg.fpr4]` 却只声明了 `F0`…`F31` **数字名**——而真实语料里浮点操作数**几乎全
  写 ABI 名**（全集 `ft*`/`fa*`/`fs*` 出现 **1840** 次，`f0` 这类数字名只有 **47** 次），
  于是 `rv32f-valid.s`、`rv32d-valid.s`、`numeric-reg-names-{f,d}.s`、
  `fp-*-default-rounding-mode.s` 这一整批文件**整文件**落进红桶（失败样本全是
  `fadd.s fa0, fa1, fa2` / `fsqrt.s fa0, f0` 这类）。修法**只加数据**：补 psABI 那套映射
  （0–7 = `ft0`–`ft7`、8–9 = `fs0`–`fs1`、10–17 = `fa0`–`fa7`、18–27 = `fs2`–`fs11`、
  28–31 = `ft8`–`ft11`），口径与 GPR 别名**逐字相同**（解析认、渲染仍出规范名 `F`），
  不新增 DSL 键、不动生成器。
  结果：riscv64 红行 **1240 → 1056**、红文件 76 → 72；`numeric-reg-names-f.s` /
  `numeric-reg-names-d.s` **整文件转绿**并进全绿层（全绿集 83 → **84**，riscv64 语料
  53 → **55** 份、`parsed` 526 → **654**）；同族红行同步下降（`rv32f-valid.s` 26 → 16、
  `rv32d-valid.s` 27 → 18、`fp-default-rounding-mode.s` 16 → 8、`rvf-aliases-valid.s` 10 → 5）。
  顺带把**逐文件红桶样例**写进机读记分板（`file_rows[].samples`）——"按文件归因"以前只能
  看到 12 条/套的汇总样例，看不到每个文件错在哪一行。

- riscv64（第二十五批：**命名 CSR**——新 DSL 能力"命名立即数表"的第一个消费者）：
  `csr12` 槽原先只收**数字**，于是 `machine-csr-names.s`（180 条红）、`user-`/`supervisor-`/
  `hypervisor-csr-names.s` 及其 rv32 变体**整文件**落红桶——而同一批测试里数字与名字是
  **相邻两行**（`csrrs t1, mvendorid, zero` / `csrrs t2, 0xF11, zero`）。
  修法不是给 CSR 开小灶，而是补一个通用能力：`[conventions.imm_names.<表名>]`（表名 → 名字 → 值）
  \+ 槽上 `names = "<表名>"`（**一个名字 = 一个值**，与 `kind = "bits"` 的"名字拼接、按位或"相对）。
  解析认名字也认字面量（名字大小写不敏感、没命中就落回原路径），渲染时值在表里就写名字
  （**同值多名取字典序最小者**），因此 `disassemble → assemble` 闭合。
  CSR 数据从上游 `RISCVSystemOperands.td`（ref `llvmorg-19.1.0`）机械生成 **432 条**
  （含 `foreach` 区间与 `AltName`/`DeprecatedName` 别名），并**用语料自带的期望字节逐条对账**：
  语料里出现的 429 个名字全覆盖、**值零不符**。
  结果：riscv64 红行 **1056 → 465**、红文件 72 → 58；**13 个文件进全绿层**
  （全绿集 84 → **97**、riscv64 语料 55 → **68** 份、`parsed` 654 → **1616**）；
  编码对拍档 `cases` 339 → **1139**、`checked` 221 → **1021**、`known` 仍 **0**
  （新进的 CSR 文件自带 ~800 条期望字节，**全部逐字节相等**——这是 432 条表值正确性的直接证据）。

- riscv64（第二十六批：**浮点 `rm` 成为操作数**——命名立即数表的第二个消费者）：
  F/D 的 `funct3` 那 3 位就是舍入模式，谱里原先写死（每条一个固定值或没有 rm），于是语料里
  带 rm 的写法一条都装配不了。补法照既有机制：`[conventions.imm_names.rm]`（`rne`/`rtz`/`rdn`/
  `rup`/`rmm`/`dyn`）+ 两条新 form（`R_RM` / `R_RM2`：把 `funct3` 当操作数位域），浮点算术 5 族与
  `fcvt` 全 18 条各加一条"显式 rm"声明。
  **两处只有字节才说得清的事**（都记在 `isa/riscv64.toml` 的注释里）：
  ① **省略 rm 的缺省值分族**——算术族 = `dyn`(7)、`fcvt` 族 = `rne`(0)。从**语料自带的期望字节**
  量出来（11 条样本，横跨 `rv32d-valid.s`、`rv32zdinx-valid.s`、`rv32zfhmin-valid.s`、
  `rv64zhinxmin-valid.s`）：两个上游文件对同一件事的**反汇编**写法给过相反的暗示
  （`fp-default-rounding-mode.s` 的 `CHECK-INST` 把省略写法渲染成 `, dyn`），但**字节**说了算。
  ② `FCVT.S.D` 的 **`rs2` 必须是 1**（规范：funct7 `0100000` + rs2 `00001`）——我们原来与
  `fcvt.d.s` 同形（rs2=0），字节对拍当场抓到。
  结果：**7 个文件进全绿层**（`rv32f-valid.s`、`rv32d-valid.s`、`rv64f-valid.s`、`rv64d-valid.s`、
  `fp-default-rounding-mode.s`、`rv64{f,d}-aliases-valid.s`；全绿集 97 → **104**、riscv64 语料
  68 → **75** 份、`parsed` 1616 → **1741**）；riscv64 红行 465 → **446**、红文件 58 → 55；
  编码对拍 `cases` 1139 → **1251**、`checked` 1021 → **1111**、`known` 仍 **0**
  （`rv32d-valid.s` 49 条期望 / 38 条对拍上 / 0 差异）。附带一处**归因迁移**（不是回归）：
  Zfinx/Zdinx 的"浮点算在整数寄存器上"写法（`fadd.s x26, x27, x28`）从 `no_prefix`
  （连助记符都不认识）挪进红桶（认识助记符、操作数对不上）。

- aarch64（第二十七批：**访存立即数的"单位"修正 + 无位移形态**）：A64 的 LDR/STR/LDP/STP 立即数
  在**文本里是字节**、在**字段里是"字节 ÷ 访问尺寸"**（`ldr x2, [sp, #32]` ⇒ imm12 = 4、
  `stp x29, x30, [sp, #-16]` ⇒ imm7 = -2）。我们谱里 `imm12u`/`imm7u` 一直按**字段单位**收文本
  （`ldr x0, [x1, #1]` ⇒ imm12 = 1 = 8 字节），于是真实语料里每一条带非零字节偏移的
  `ldr`/`str`/`stp`/`ldp` 都会编出**错的字节**——只是集里一个这样的文件都没有，字节对拍档看不见它。
  修法：按访问宽度分槽（`imm12x` unit 8 / `imm12w` unit 4、`imm7x` unit 8 / `imm7w` unit 4；
  pair 的 imm7 同时改成**有符号**，`stp …, #-16` 才收得下），而 `imm12u` **留在原地**——
  ALU/CMP/CMN 的立即数**不缩放**（`add x0, x1, #5` 就是 5），同一段位的两种解释，所以是两个槽。
  顺带补 **8 条无位移形态**（`ldr x4, [x3]`、`str w9, [x8]`、`stp x0, x1, [x2]`…：编码上就是
  "位移 = 0"那一格，与 DCPS1B 同款——声明序在后、解码仍走带位移那条）。
  **证据**：`arm64-memory.s` 的 CHECK 行给出上游字节，谱内新增 6 条向量
  （`ldr x4, [x3]` = `64 00 40 f9`、`str x2, [sp, #32]` = `e2 13 00 f9`、`ldr w5, [x4, #20]`…）；
  该文件里我们认得的 23 条**逐字节全等**（`known` 0），且改动前后**没有新增任何访存类字节差异**。
  原有 7 条向量只是**文本**从字段单位换成字节（`ldr x0, [x1, #1]` → `#8`），字节一字未动——
  正说明改的是"解释"而非"编码"。结果：aarch64 红行 2183 → **2122**、
  `arm64-leaf-compact-unwind.s` 34 → 16、`seh-large-func-multi-epilog.s` 37 → 8。

- aarch64（第二十八批：**寄存器惯用名 + 前后索引（writeback）**）：真实序言/尾声文件写的不是
  `x29`/`x30` 而是 **`fp`/`lr`**（`mov x29, fp`、`stp x29, lr, [sp, #-16]!`），而我们的 GPR 别名表
  只有 `XZR`/`LR`——**`FP` 一直缺着**（注释里写着、表里没写），一行之差就整行落红。补 `FP = 29`，
  顺带 `IP0 = 16`/`IP1 = 17`（链接器 scratch 的惯用名）。
  再补 **16 条前后索引形态**：单寄存器 4 宽 × 前/后索引（`ldr x0, [x1, #16]!` / `ldr x0, [x1], #16`，
  与 unscaled 家族共用 `op8 = 0xF8`(X)/`0xB8`(W)，靠新位域 `mode`（[11:10]）区分 `11` 前 / `01` 后）；
  pair 4 宽 × 前/后索引（[25:23]：`011` 前 / `001` 后 ⇒ `op8 = 0xA9`/`0xA8`、`idx2 = 2`/`3`）。
  **字节证据**：`docs/reference/aarch64-encoding-ref.md` 第 121/124 行给了权威值
  （`ldr x0,[x1],#16` = `F8410420`、`ldr x0,[x1,#16]!` = `F8410C20`、
  `stp x29,x30,[sp,#-16]!` = `A9BF7BFD`、`ldp x29,x30,[sp],#16` = `A8C17BFD`），
  外加无位移 pair 的两条（`A9000440`/`A9400440`）——6 条谱内向量**一次通过**，说明新形态的
  编码与模式位都对得上。
  结果：aarch64 红行 2122 → **1945**、红文件 76 → **72**，**4 个文件整文件转绿**
  （`cfi.s`、`seh-multi-epilog.s`、`seh-large-func.s`、`seh-large-func-multi-epilog.s`——
  它们此前只差这条序言/尾声写法）；`seh-packed-unwind.s` 111 → 48、`basic-a64-instructions.s` 482 → 442。

- aarch64（第二十九批：**寄存器偏移寻址 + ADD/SUB 移位寄存器带量**）：
  ① **寄存器偏移**（`ldr x9, [x27, x6]`、`ldr w0, [x0, x0, lsl #2]`、`str w14, [x26, w6, uxtw]`、
  `str w13, [x27, x5, sxtx #2]`）：编码是 `size 111 V 00 opc **1** Rm option S **10** Rn Rt`——
  与 imm9 家族**共用 `op8 = 0xF8`(X)/`0xB8`(W)**，只靠 **bit21 = 1** 与 **[11:10] = 10** 区分
  （这两条固定位是 19 条上游期望字节逐条核对出来的）。`option`/`S` 与文本关键字的对应同样是**量**出来的：
  LSL = 3 / UXTW = 2 / SXTW = 6 / SXTX = 7，且 **S=1 ⇔ 文本带 `#N`，N 恒 = log2(访问宽度)**
  （X 形式 3、W 形式 2）⇒ 每种组合都是一条**完整的文本模板**，不需要"可选片段"机制。
  索引寄存器的类由 option 决定（LSL/SXTX 收 X、UXTW/SXTW 收 W）；语料里出现的 8 种组合全做，
  × 2 宽度 × ldr/str = **32 条**。
  ② **ADD/SUB(S) 移位寄存器带量**（`add w12, w13, w14, lsl #12`）：与既有的逻辑族（`ALUSH6`/`ALUSH5`）
  同格局——`shift` 是逐指令常量（LSL = 0），移位量是**操作数**（X 用 `imm6`、W 用 `imm5`），
  4 助记符 × 2 宽度 = **8 条**。
  **字节证据**：谱内一次性加了 12 条向量（8 条寄存器偏移 + 4 条 ADD/SUB 带量），字节全部取自语料
  CHECK 行的上游期望值（`00 68 60 b8` = `ldr w0, [x0, x0]`、`69 6b 26 f8` = `str x9, [x27, x6]`、
  `ac 31 0e 8b` = `add x12, x13, x14, lsl #12`…）——**首跑全过**。
  结果：aarch64 红行 1945 → **1872**、`parsed` 1268 → **1341**；与第二十七批末（aarch64 红 2122）
  相比，`arm64-leaf-compact-unwind.s` 16 → **1**（只差最后一条）、`arm64-memory.s` 121 → 100、
  `arm64-arithmetic-encoding.s` 122 → 110、`seh-packed-unwind.s` 111 → 48。
  本轮**没有文件跨过"整文件无红"的门槛**（近门的几个都在 1–4 条），所以取舍账本与两个棘轮逐字不变。

- aarch64（第三十批：**扩展寄存器操作数**——300 行那一族）：`add x8, x8, w5, sxtw #2`、
  `cmp w0, w1, uxtb` 这类"第三个操作数带扩展关键字、可再带量"的写法。**不新增 DSL 能力**：
  关键字做成**命名立即数操作数**（`{ext}` → `option` [15:13]，两张表按操作数类别分：
  W 表收全部 8 个，X 表只收 `uxtx`/`sxtx`），量做成 `imm3` 操作数（[12:10]）；
  于是每 (助记符, 宽度) 只需 **4 条**（W 扩展 / X 扩展 × 带量 / 省略量），
  6 助记符（`add`/`sub`/`adds`/`subs`/`cmp`/`cmn`）× 2 宽度 × 4 = **48 条**。
  编码与移位寄存器族**共用 `op8`**（0x8B/0x0B/0xAB/0x2B/0xCB/0x4B/0xEB/0x6B），靠 **bit21 = 1** 区分；
  `cmp`/`cmn` 用无 `rd` 的两条形式 + `fields = { rd = 31 }`。
  **字节证据**：8 个 option 的上游字节全在语料里排成一组（`add w1, w2, w3, <ext>` 从 `41 00 23 0b`
  逐项递增到 `41 e0 23 0b`，正好是 option 0..7），谱内一次加 **10 条向量**（含 X 形式的
  `add x1, x2, w3, uxtb` = `41 00 23 8b`）——**首跑全过**。
  这一族还顺手把语料里那句 `add w1, w2, w3, uxtx`（W 形式配 uxtx）暴露出来：**W 表必须收全部 8 个**
  （32 位视图下 uxtx 是恒等），X 表才只收 uxtx/sxtx——两表的分法是按**操作数类别**，不是按"哪些扩展合法"。
  结果：aarch64 红行 1872 → **1666**、`parsed` 1341 → **1547**，**`arm64-leaf-compact-unwind.s`
  整文件转绿**（它最后一条正是 `add x8, x8, w5, sxtw #2`）——全绿集 109 → **110**（aarch64 30 → **31** 份）、
  aarch64 棘轮 `parsed` 267 → **356**。

- aarch64（第三十二批：**重定位修饰族**）：`add x0, x0, #:lo12:sym`、`ldr w0, [sp, #:lo12:hiddenvar]`
  这类写法（语料 382 行、47 个修饰名）。三件事一起落地：**生成侧标签识别缺陷**（见「已知缺口」
  一节，那才是根因）、**新槽能力 `require_symbol`**、**形状隔离**（`#` 进 imm_fn 文本）。
  产物：ALU 立即数 8 条（`add`/`sub`/`adds`/`subs` × X/W）+ LDR/STR 4 条，共 **12 条**；
  `[[conventions.imm_fn]]` **98 条**（49 个修饰名 × 带 `#`/不带 `#` 两种写法）。
  结果：`parsed` 1572 → **1618**、红 1877 → **1734**、`corpus_only` +97（未定义符号归入
  "上下文不足"）、`known` 仍 **53**；**4 个文件整文件转绿**（`arm64-ilp32.s`、`basic-pic.s`、
  `jump-table.s`、`tls-add-shift.s`）——全绿集 109 → **112**、淘汰 624 → **621**。

- aarch64（第三十四批：**FP/NEON 的宽度视图 + SIMD&FP 访存全族**，本轮最大的一批）：
  `ldr d8, [sp, #8]`、`str s7, [sp, #4]`、`ldr q9, [sp, #16]`、`stp d8, d9, [sp, #-64]!` 这类——
  语料里 FP 访存的助记符都是 `ldr`/`str`/`ldp`/`stp`，**宽度只体现在寄存器名上**。
  **设计结论（本轮最值钱的一条）**：宽度必须由**寄存器组**承载——若 B/H/S/D/Q 的名字都在
  同一个组里（都算 FPR(8)），`ldr s0,…` 与 `ldr d0,…` 的**文本形状完全相同**，分派会提交到
  先声明的那条（实测把 S 编成了 D、把 Q 编成了 B）。于是：既有 `[reg.fpr8]`（V0..V31）就是
  **D 视图**并挂上 `D0..D31` 别名，另开 `fpr1/fpr2/fpr4/fpr16`（B/H/S/Q）；并且
  **`[meta].default_fpr_width = 8`** 必须显式写——否则"主 FPR 类"按最宽组取到 Q，
  ABI 拿它去找 `V0` 会报 `UnresolvedReg`（实测 aapcs64 计划直接红）。
  另加 `[spill.FPR16]`（16 字节视图比浮点值池 8 字节宽，校验要求显式声明）。
  **两处真错（都由既有防线抓出来）**：
  ① **FP unscaled 的 op8 一直是错的**：`LDURD`/`STURD`/`LDURS`/`STURS` 写成 `0xFD`/`0xBD`
     （那是**无符号偏移族**），上游是 `0xFC`/`0xBC`（`ldur d8, [sp, #8]` = `e8 83 40 fc`）。
     潜伏至今是因为 FP 槽用 V 名、语料写 `d8`/`s7`，那批行从来没解析过 ⇒ 字节对拍档看不见。
     这两条还带 `callee_save`/`callee_load` 角色、`[spill.FPR]` 也引用它们 ⇒ **arm64 JIT 的
     FPR 保存/恢复一直在按错族发偏移**，这次一并修掉，并用 4 条谱内向量钉住。
  ② **我自己写错的一处**：Q（128 位）单寄存器的 opc 是 **11 载 / 10 存**（D/S/H/B 才是 01/00），
     我照 D 写了 ⇒ `ldr q9, …` 被编成 `ldr B9, …`（字节差在 bit23）。**新解析出来的语料行
     当场把它抓出来**（26 条 FP 形态差异），改完 FP 形态差异归零。
  结果：aarch64 `parsed` 1640 → **1909**、红 1706 → **1437**、编码 `checked` 785 → **945**、
  `known` 53 → **57**（+4 全是新解析出来的 `sp` 别名写法暴露的**既有**编码口径差异，
  非 FP：上游把 `add w1, wsp, w3` 编成**扩展寄存器**形态 `UXTX`）。全绿集 112 → **113**。

- aarch64（第三十五批：**FP 成对的符号 imm7**）：`stp d8, d9, [sp, #-64]!` 这类**前/后索引**的
  成对写法原先借用了**无符号**的 `imm12fp*` 槽 ⇒ 负偏移直接越界、报"没有匹配的指令"。
  镜像 GPR 成对的 `imm7x`/`imm7w`，补 `imm7fp4`/`imm7fp8`/`imm7fp16`（**有符号**、width 7、
  单位 = 元素尺寸）并替换 18 条 FP 成对的 imm 槽。结果：`parsed` 1909 → **1917**、红 1437 → **1429**；
  **`seh-optimize.s`、`seh.s` 整文件转绿** ⇒ 全绿集 113 → **115**、淘汰 620 → **618**。

- aarch64（第三十六批：**注释标记改字符串**——A64 的 `/` 曾把除法截断）：`strip_comment` 按
  **单个字符**切注释，而 A64/GAS 的注释是 `//`；`comment_char = "/"` 于是让
  `movz x0, #(32 / 2)` 被截成 `movz x0, #(32` ⇒ 报"没有匹配的指令"。上游为此专门写了
  `single-slash.s`（**这就是可复现的语料证据**）。修法 = 注释标记改成 1–4 字符的**字符串**、
  生成物按整串匹配，`isa/arm64.toml` 用 `"//"`。结果：`parsed` 1917 → **1918**、
  红 1429 → **1428**；**`single-slash.s` 转绿** ⇒ 全绿集 115 → **116**、淘汰 618 → **617**。

- aarch64（第三十三批：**移位立即数** `add w3, w4, #1024, lsl #12`）：A64 的 add/sub(imm) 用
  **bit22 = sh** 表示"左移 12 位"，文本里的立即数**始终是未移位的值**（`#1024, lsl #12` ⇒ 字段 1024）。
  16 条：数值形态 8 条（`add`/`sub`/`adds`/`subs` × X/W，槽 `imm12u`）+ **重定位 + 移位** 8 条
  （槽 `imm12sym`，覆盖 `add x2, x3, #:lo12:sym, lsl #12`——`elf-reloc-addsubimm.s` 就那一条）。
  结果：aarch64 红行 1734 → **1706**、`parsed` 1618 → **1640**、`corpus_only` +6
  （重定位 + 移位的写法现在解析得住，未定义符号按"上下文不足"记账）。
  **一处要注意的 `known` +1**（不是我们的回归）：`basic-a64-instructions.s:312`
  `subs xzr, sp, #20, lsl #12` 我们编成 64 位形态（`0xf1`，与文本里的 `xzr`/`sp` 一致），
  而上游那行的 CHECK 期望是 **32 位**形态（`0x31`）——那是**语料抽取把 CHECK 行配错了对**
  （该文件另有 `subs wzr, wsp, …` 的行）。该文件本就不在全绿层，`known` 也只影响它自己。

- aarch64（第三十一批：**字节/半字 GPR 访存**）：`ldrb`/`ldrh`/`ldrsb`/`ldrsh`/`ldrsw`/`strb`/
  `strh` —— 9 组助记符/类别 × {带位移, 无位移} = **18 条**（语料 117 行）。编码与既有 LDR/STR
  同族：`size 111 V 00 opc imm12 Rn Rt`，`opc` 00 = store / 01 = 无符号载 / 10 = 符号载到 X /
  11 = 符号载到 W；imm12 的单位 = 访问宽度（byte 1 / halfword 2 / word 4，槽上按单位分开）。
  **一处真错由字节对拍档当场抓到**：无符号偏移族（imm12）的 `[25:24] = 01` ⇒ op8 是
  `0x39`/`0x79`/`0xB9`，我最初按 unscaled 族的 `0x38`/`0x78`/`0xB8` 写
  （`ldrb w4, [x3]` 我们 `…38` ≠ 上游 `…39`）；修正后 `known` 该族清零。顺带：`0xB9` 让
  `ldrsw` 的无位移形态不再与既有 `LDURSW`（`0xB8`，`mode` 占 bit10 起 2 位）在**解码位 trie**
  里重叠 —— 这正是"位 trie 会把结构冲突在生成期喊出来"的一次现场。
  **向量**：生成器从语料 CHECK 行自动挑具体字节的样本（5 条：`ldrb`/`ldrsh`/`strb`/`ldrsw`/`strh`），
  codegen 里全过。
  结果：`parsed` 1547 → **1572**、`no_prefix` 14558 → **14322**、编码 `known` 78 → **53**；
  新认出的 236 行里 25 条整条通过、**211 条按归因落红**（它们的地址形态/寄存器族还没做：
  寄存器偏移、前后索引、`ldr q0` 这类 FP/向量访存）——README 说的"认识助记符了就往出红挪"，
  不是回归。取舍账本与棘轮**逐字不变**（这族 117 行都在因别的原因而落红的文件里）。

### 上游全集里没收的那一层（**算出来的**淘汰面，2026-10-06）

`asm/fetch.mjs` 把上游 LLVM MC 三套目录的**顶层**候选全拉下来评了一遍
（riscv64 339 + aarch64 337 + x86 只取 Intel 语法名的 57 = **733** 个 `.s`），
按「整条解析得住 + 不产生红桶 + 编码逐字节相等」留下 **83** 个；淘汰的 **651** 个分三类：

| 淘汰原因 | 个数 | 典型 |
| --- | ---: | --- |
| `parsed = 0`（一个候选都不认识） | 576 | SVE/SME/FP8（aarch64 子目录）、AVX-512/AMX/APX 之外的 SIMD（x86）、RVV/corev（riscv 子目录）、 加密与位操作扩展 |
| 出红（有候选首段对得上、整条没对上） | 66 | aarch64 移位/扩展寄存器操作数（`add x2, x4, w5, uxtb`）、立即数 `lsl #12`、`stp/ldp`、寄存器偏移寻址、`ldr x0, =…`；riscv64 带符号 **CSR 名**（`csrrs t1, mstatus, zero`）、重定位表达式（`%tlsdesc_hi(a-4)`） |
| 有字节差异（解析得过、编码不一样） | 9 | RVC 压缩编码（`compress-*.s`、`option-rvc.s`、`xwchc-compress.s`）、Zcb、RV32 变体的替换编码（`rv32zbb-*`/`rv32zbkb-*`；本谱是 riscv64） |

这不是"忘了收"：这三类都会把红桶喂成"永久红"或让编码对拍清单全是噪声，门禁就废了。
**谱长本事之后重跑 `fetch.mjs` 会自动把它们补回来**（判据在脚本里，不在人脑里）。

> **第三十批之后的同一张账**（`2026-10-07` 重跑）：留 **110**（候选 109 + 子目录手工 1）、
> 淘汰 **624** —— `parsed = 0` **557** + 出红 **55** + 有字节差异 **12**。前四批 riscv64 修补让
> 20 个文件转绿进集；第二十七～三十批（aarch64 访存单位、惯用名 + 前后索引、寄存器偏移 +
> 带量、**扩展寄存器**）让 5 个 aarch64 文件进集（最新一个是 `arm64-leaf-compact-unwind.s`，
> 它最后一条正是 `add x8, x8, w5, sxtw #2`）。其余文件仍在这三桶之间移动（认识助记符了就往
> "出红"挪）。这正是"取舍是算出来的"该有的样子：**改的是谱，账自己重算**——而"没进桶"
> 同样是一个可复核的结论。
>
> 新进集的文件里有 `machine-csr-names-invalid.s`（上游"故意写错"的文件）——它那些行在
> **rv32** 目标上被 LLVM 拒（"system register use requires an option to be enabled"），
> 但在 **riscv64** 目标上"名字 → 12 位编码"完全合法（上游 `rv64-machine-csr-names.s`
> 的注释原话：这些名字是 RV32-only，但 RV64 只要给得出值就能编码/反汇编）。
> 本档只判"能不能装配 + 字节对不对"，**不判上游的 feature 门**——所以这不是假绿。

## 现有语料与计数

（`2026-10-07` 实测；解析档数值以 `asm/ratchet/<isa>.txt` 为准，编码档以
`asm/ratchet/encoding.txt` 为准。本表是"上游全集 → 全绿子集"那一轮之后的数：
候选 733 收 **110**（+1 份 GAS 节选）——全绿层随"谱长本事"自动长大：riscv64 因浮点 ABI
别名、**命名 CSR**、**浮点 rm** 三批，aarch64 因**访存单位 + 无位移**、**惯用名 + 前后索引**、
**寄存器偏移 + 带量**、**扩展寄存器**四批，`parsed` 526 → **1741**(riscv64) / 197 → **356**(aarch64)、
语料 53 → **75** / 26 → **31** 份、编码对拍 `checked` 221 → **1111**）

| ISA | suite | 文件 | 行数 | parsed | no_prefix | tail_mismatch | corpus_only | 字节 oracle |
| --- | --- | --- | --- | --- | --- | --- | --- | --- |
| x86 | `gnu-gas-intel` | 1 | 64 | **50** | 4 | 7 | 3 | 无（GAS 用例不带期望字节） |
| x86 | `llvm-mc` | 4 | 471 | **116** | 1 | **0** | 354 | **有**：110 条期望 / **110 条逐字节全等 / 0 条差异** |
| riscv64 | `llvm-mc` | 75 | 10835 | **1741** | 224 | **1** | 8869 | **有**：1251 条期望 / **1111 条对拍上 / 0 条差异** |
| aarch64 | `llvm-mc` | 31 | 2832 | **356** | 393 | **0** | 2083 | **有**：488 条期望 / 125 条对拍上 / **0 条差异** |

> 三档都有一份**逐文件**记分板（`target/asm-suite/`）：解析档的 `<isa>.json`（`file_rows`）
> 与编码档的 `encoding-files.json`。它们不是"报告附件"，而是**语料取舍的输入**
> （见「刷新语料」）——被砍掉的文件名与原因也落盘（`target/asm-dropped.json`）。
>
> `file_rows[].samples` 是**逐文件**的红桶样例（`文件:行号 ⇥ 原文 ⇥ 错误`，每文件最多 8 条），
> 修谱时按文件看"错在哪一行"——汇总样例只留 12 条/套，按文件归因时不够用。JSON 里的
> 制表符等控制字符由 `report.rs::esc` 转义（裸控制字符会让 `JSON.parse` 整份失败）。

执行档：x86 5 条、riscv64 3 条、aarch64 2 条，**三架构都真跑通**（`ran=10 skipped=0`）。

## 刷新语料

```bash
node crates/tools/forge-tests/asm/fetch.mjs              # 默认：拉全集 → 打分 → 取舍 → 重写 PROVENANCE.md
node crates/tools/forge-tests/asm/fetch.mjs --list       # 只打印计划（不下网）
node crates/tools/forge-tests/asm/fetch.mjs --isa riscv64 # 只处理一套
node crates/tools/forge-tests/asm/fetch.mjs --all        # 连子目录一起拉（SVE/SME/AMX/apx/rvv，评估全量用）
node crates/tools/forge-tests/asm/fetch.mjs --keep-all   # 拉全集但不裁剪（评估模式）
node crates/tools/forge-tests/asm/fetch.mjs --no-fetch   # 跳过下载，只用本地文件重算
```

一条命令做三件事：① 按钉死的 ref（`llvmorg-19.1.0`）拉候选全集（x86 只取 Intel 语法
文件名那批；**只改落盘名不改内容**，子目录段折进文件名）；② 跑两个档拿逐文件记分板，
按「**整条解析得住（`parsed >= 1`）**、**不产生红桶（`tail_mismatch` ≤ 已登记缺口）**、
**编码对拍没有字节差异**」三条淘汰——判据是**算出来的**，淘汰清单落
`target/asm-dropped.json`；③ 重写 `asm/PROVENANCE.md`（逐文件 字节/行/sha256）。
打分那一步用 `--no-fail-fast` 跑两个档：cargo 默认在**第一个失败的测试目标**就停，而换谱/换语料时
两个档的棘轮几乎必然同时不一致 ⇒ 少了它排在后面的解析档根本不跑、记分板停在上一轮，新下载的候选
会被记成 `not-scored` 砍掉（2026-10-07 实测踩到：364 个候选被误记，已修）。

跑完**必须**刷棘轮并**看 diff**（脚本会打印命令）：

```powershell
$env:FORGE_ASM_WRITE_RATCHET = "1"; cargo test -p forge-tests --test asm_parse --test asm_encoding
```

新增**一整套**语料（比如 XED/NASM）时另有四步：① 在 `src/asm/corpus.rs` 的 `SUITES` 里登记
（key/目录/注释前缀/许可/来源；`encoding:` 注释位置与缺省不同时补 `encoding_sides`）；
② 让 `fetch.mjs` 的 `SOURCES`/`MANUAL` 认它；③ 刷两个棘轮；④ `PROVENANCE.md` 由脚本重写，
**不要手抄行**——`tests/asm_provenance.rs` 会守（表里表外、字节数、行数，两个方向都红）。

**谱里的数据表也有生成器**：riscv64 的命名 CSR（`isa/riscv64.toml` 的
`[conventions.imm_names.csr]`，432 条）由
`asm/gen-riscv-csr-table.mjs` 从上游 `RISCVSystemOperands.td`（同一个钉死的 ref）
机械生成并**语料对账**后原地重写——换 ref/加 CSR 时跑它，别手改那张表；
它幂等（连跑两次无 diff），对账不过（缺名/值不符/语料自相矛盾）会直接失败。

**同一套做法的第二条**：`asm/gen-aarch64-byteloads.mjs` 生成 aarch64 的**字节/半字 GPR 访存**
（`ldrb`/`ldrh`/`ldrsb`/`ldrsh`/`ldrsw`/`strb`/`strh`，9 组 × {带位移, 无位移} = 18 条指令 +
两个按单位分的 imm12 槽）。它的内容是**量出来的**：`op8` = 0x38/0x78/0xB8 与
`opc2` = 00 store / 01 无符号载 / 10 符号载到 X / 11 符号载到 W 来自语料
（117 行、逐条解期望字节），imm12 的单位 = 访问宽度（byte 1 / halfword 2 / word 4）。
生成后 `validate`/`lint` 均过；**尚未入库到 `isa/arm64.toml`**——落地还差"快照守卫钉死值 →
全集语料的字节对拍（`known` 必须为 0）→ 裁回全绿层 → 刷棘轮 → 全门禁 → 文档/CHANGELOG"这一条链，
按顺序跑完再提交（`git checkout -- isa/arm64.toml` 可随时回到干净状态）。

**许可**：LLVM MC 与 XED = `Apache-2.0 WITH LLVM-exception`，NASM/YASM = `BSD-2-Clause`，
GAS = `GPL-3.0-or-later`（上游各文件的许可逐条见 `PROVENANCE.md`；GAS 那份在独立目录
`parse/x86/gnu-gas-intel/`）。

- aarch64（第三十七批：**movz/movk/movn 的 abs_g* 类重定位 24 条**）：批本身是**纯解析**能力
  （新变体与数值形态同 opcode、一律 require_symbol 把关），落地后第一次量账：parsed 1918 →
  **1919**、**红行 1428 → 1379（−49）**、corpus_only +48（那些行的符号在别处 ⇒ 归入"上下文
  不足"而不是缺陷）、编码 known 仍 **57**（**零字节风险得到验证**）。取舍账本与棘轮**逐字不变**
  （这些行都在因别的原因而落红的文件里）。仍红的 mov-expr-as-immediate.s 卡的是**数字局部标签**
  （`mov x0, #:abs_g0_s:1b` 的 `1b`）与 mov 的立即数别名，不是修饰族本身。

- aarch64（第三十八批：**mov 立即数别名**）：批后第一次量账：parsed 1919 → **1922**、红 1379 →
  **1376**（−3）、编码 known 仍 **57**；**align.s 整文件转绿**（它三条都是 mov x0, #0）⇒ 全绿集
  116 → **117**、淘汰 617 → **616**（PROVENANCE 119 行）。仍在红桶的同族写法是 mov 的**宽立即数**
  （上游会改用 movn/orr）与 **数字局部标签**（`1b`/`1f`）。

> **宽立即数这条路暂时走不通（2026-10-07 勘察）**：`mov <Rd>, #imm` 的可表示值之外，
> 上游会展开成 `movn`（`mov x0, #-1` ⇒ `movn x0, #0`）或**多条** `movz`+`movk`。
> 但本仓库的 `[[pseudo]]` **分派键不得与任何指令的首段字面相同**（`mov` 已被 MOV 系列占用），
> 所以多指令展开对 `mov` 不可用；而 `movn` 的展开需要"对立即数做算术"（`#-1` → `#0`），
> 现有模板只有字面段 + 占位符，也没有 `imm_fn` 那种"前缀即变换"的钩子（`#-1` 没有前缀）。
> ⇒ 宽度覆盖 `#0`/`movz` 可表示值的部分**已经做完**（第三十八批），其余要动的是**展开机制**
> 本身（伪指令键冲突规则或立即数变换钩子），不是再加两条变体。
> **数字局部标签 `1b`/`1f` 的真实规模（2026-10-07 严格量法）**：引用 **13 行 / 共 21 处**、
> 定义 **7 行**、涉及 **5 个文件**。上一轮报的 2494 行 / 65 文件是**量法错**——那个模式把
> NEON 的元素后缀（`abs.8b v0, v0` 的 `8b`）也算成了标签。判据应当是：标签 token 的前导字符
> 是空白/`,`/`#`/`(`/`+`/`-` 且**不是 `.`**（元素后缀永远长在 `.` 后面）。⇒ 这条线**面小**
> （5 个文件、13 行），但**文件翻转密度高**，而且与 NEON 的 `v0.16b` 在词法层互相干扰，
> 两条线应当一起设计。

- 汇编器（第三十九批：**数字局部标签 `1b`/`1f`**，GNU as / LLVM MC 语义、**ISA 无关**）：
  词法器把「数字 + b/f」吐成普通 `Ident`（判据：其后不再接标识符字符或 `.`）⇒ 下游的符号
  引用/立即数修饰原样可用（`#:abs_g0_s:1b` 里的 `1b` 就是普通符号名），**零新增操作数种类、
  零新增槽键**；两遍布局里数字标签单独收集成「编号 → 位置列表」，`1b` 取最近的上一个（允许
  与分支同址）、`1f` 取下一个，找不到报 `UndefinedLabel`（准确诊断 ⇒ 归「上下文不足」桶）。
  与 NEON 元素后缀互不干扰（`v0.16b` 由 dot-ident 规则吃掉）。结果：**全绿集 117 → 123**
  （六个都是 **riscv64** 文件——能力在共享汇编器里，跨 ISA 一起受益）；riscv64 棘轮刷新、
  PROVENANCE 125 行。aarch64 侧含局部标签的 5 个文件仍被别的缺口挡着（`arm64-adr.s` 缺
  ADR/ADRP 本身；`mov-expr-as-immediate.s` 缺 `mov` 立即数的符号/表达式形态），是本轮能力的
  下一批靶子。

- aarch64（第四十批：**mov 立即数的符号/表达式形态 8 条 + 同型指令模板化**）：形态侧零新增
  概念（槽复用 movz/movk/movn 重定位那批的 `imm16g0..g3`，编码沿用 `MOVXIMM`/`MOVWIMM`，
  模板不带 `#` 保持形状隔离）。效果：`parsed` 1922 → **1924**、红 1376 → **1369**；
  **`mov-expr-as-immediate.s` 与 `mov-unsupported-expr-as-immediate.s` 转绿** ⇒ 全绿集
  123 → **125**（PROVENANCE 127 行）。同时按仓库既有风格把最近三批 **34 条逐条声明压成
  21 个 `[[templates]]` + rows**（`isa/arm64.toml` 减约 **99 行**）：**展开后完全等价**——
  两处计数（454 / 2213）与歧义名单（38 条）重构前后一字未变；三个一次性生成器已删除，
  免得"谱已紧凑、生成器还吐冗长版本"。

> **ADR/ADRP 不是白捡的（2026-10-07 勘察，已回退）**：谱里补 `adr`/`adrp`（位切片 `immlo[30:29]` +
> `immhi[23:5]`、`unit` 分 ADR=1/ADRP=4096、`imm_prefix` 让 label 槽同时吃 `#0` 与 `1f`）实测：
> **`parsed` 1924 → 1953（+29）、`no_prefix` 14322 → 14215（−107）**，但**红行 1369 → 1429（+60）**
> ——它把原来静静躺在 `no_prefix` 里的**重定位/fixup 形态**（`adr x0, 1f`、`adrp x, :pg_hi21:foo`，
> 上游期望字节是通配 `A` 或 fixup 记录）转成了真红。本线的硬纪律是**红行只许降**，所以整批回退
> （spec/DSL/计数钉死值均恢复到 HEAD，`ws=0`、留 125 不变）。
> 要真正做这条线，得先让**上游期望为通配/fixup 的行**有正确的归因（判成"不可比"而不是红），
> 那是**语料抽取侧**的规则问题，不是谱能力问题——这条结论比"再加两条指令"值钱。
> 同一批还顺带证实了一个**通用缺口**并已定位：`unit` 缩放原先只在**单字段**编码路径生效，
> **位切片路径跳过**（`adrp x0, #4096` 因此把字节值写进字段，实测 [0,128,0,144] vs 上游
> [0,0,0,176]）。修法（编码侧先除、解码侧再乘，imm 槽查整除、label 槽不查）已实测可行
> （向量全过），但随本批一起回退了——它值得作为**独立的通用修复**单独进（配一条自带的切片
> 向量做证据），而不是搭在一条会抬高红行的批次上。
> **更正（同日复查）**：上面那条把 ADR/ADRP 的红行归因于"通配期望被当红"——**错了**。
> 读 [encoding.rs](../../../src/asm/encoding.rs) 的 `parse_encoding_comment`（抽取 `encoding: [...]`
> 的唯一入口）可见：它要求**每个 token 都是 `0x..`**，遇到 `A` / `0x10'A'` / `A'A'` 这类通配会
> **整体 `?` 提前返回 `None`** ⇒ 通配期望**从来就不进对拍**。所以那 +60 条是**真实的字节差异**：
> `adr`/`adrp` 的**符号/重定位形态**（`adr x0, 1f`、`adrp x, :pg_hi21:foo`）在**有具体期望字节**的
> 行上，上游写的是"重定位解析后的值"，而我们写的是"块下标 × unit"占位。
> ⇒ 结论改写：ADR/ADRP 要变成净收益，**前置是符号/重定位形态的语义对齐**（或把这些行如实地判成
> "本档不可比"并让 `no_prefix` 的计数语义一致），**不是**去改抽取规则。数值形态那部分
> （4 条向量全过）本身是对的，`unit` 切片缺口也是真的——两者都可独立落地，别再把它们和
> "抬高红行"的整批捆绑。
>
> **落地 `unit` 切片修复时撞到的第二处（同日）**：把修复配"夹具 ISA + 谱内向量"独立进时，
> 生成期的**采样/边界自测**还不认 `unit`——`kind = "imm"` 的切片槽报"立即数未按位域语义解码"
> （采样值不是 unit 的整数倍 ⇒ 编码/解码口径在自测里对不上），换成 `kind = "label"` 后边界自测
> 过了，但**文本形态**要求标识符或带前缀的数字（`ldw r0, 12` 直接 no match）。
> ⇒ 这条通用修复要独立落地，**前置 = 让 `spec::sample_operands` / 边界自测按 `unit` 取采样值**
> （imm 槽取 unit 整数倍、label 槽沿用"块下标 × unit"），并给夹具配一条 `#` 前缀文本的向量。
> 两处都定位到了，改动本身很小——但必须与"自测口径"一起改，否则绿不了。
>
> **`unit` 切片修复已落地（同日，独立于 ADR/ADRP）**：编码侧"先除"、解码侧"再乘"、并把生成期
> 采样对齐到单位整数倍（`spec.rs`）——三份发行谱与全工作区门禁 **exit 0**，ADR/ADRP 的 4 条
> 上游字节向量在该修复下逐字节全过。**夹具仍缺**：给 `demo_inst12` 加"切片 + unit"夹具时，
> `ldw r0, #12` 报 no matching（该夹具的 imm 文本形态与我的模板不合，夹具已撤）；下一步要么先
> 摸清该夹具的 imm 文本约定，要么直接用 ADR/ADRP 的谱侧落地当证据（但那需要先解决符号/重定位
> 语义那一关）。
>
> **NEON 元素后缀的真实规模（2026-10-07 严格量法）**：**排列后缀**（`v0.16b`/`v1.4s`，即
> `v<reg>.<数字><b|h|s|d>`）共 **4676 行 / 88 个文件**；**lane 访问**（`v0.b[1]`）另有
> **656 行 / 27 个文件**。⇒ 这是剩余**最大**的一块面（远超此前各批的几十到几百行）。
> 计量口径：只看"像指令"的行（行首字母开头，剔除注释），且要求 `v<数字>.` 后紧跟排列或
> `[`；因此不会把 `abs.8b` 这类"助记符带后缀"的行误算成寄存器后缀（上一轮数字局部标签量法
> 就是栽在这一点上）。
> 设计含义（两条机制必须**一起**做，否则只解一半）：① **寄存器**侧——reg 槽接受可选的
> `.Nt` 排列后缀（宽度/元素类型由后缀决定，与第 11 轮确立的"宽度必须由寄存器组承载"同源，
> 但这里的宽度写在文本后缀里）；② **助记符**侧——`add.16b`、`ld1.4s` 这类"助记符 + `.` +
> 排列"的形态。二者在词法上都以 `.` 后接数字/字母出现，与数字局部标签 `1b`/`1f` 的判据
> （标签 token 不得以 `.` 前导）互不干扰。
>
> **NEON 落地路径收窄（2026-10-07 勘察结论）**：NEON 的两条机制里，**助记符侧不需要任何
> DSL 改动**——`abs.8b v0, v0` 这类写法里，后缀写在**助记符**上（扫描器只需首段字面 `abs`），
> 而寄存器 `v0` 恰好就是我们 **`fpr16` 组**（`V0..V31`）里的名字；于是"按（族 × 排列）逐条声明、
> 把后缀直接写进 `asm`"就够（`asm = "abs.8b {dst}, {src}"`），**零新槽键、零词法改动**。
> 真正需要新机制的只有**寄存器侧后缀**（`add v0.16b, v1.16b, v2.16b`——后缀长在寄存器名后，
> 现有 reg 槽只吃一个 token，余下的 `.16b` 会变成 TailMismatch）。⇒ 建议顺序：
> ① 先按"助记符带后缀"的族铺开（纯谱工作，可用 `[[templates]]`+rows 压紧）；
> ② 再给 reg 槽加**可选排列后缀**（一个槽键 + 解析器合并相邻 `.Nt` + 校验），一次覆盖 4676 行里的
> 其余部分。
>
> **NEON 首个族（`abs`）的上游字节与字段分解（2026-10-07 取自语料，可直接实施）**：
>
> | 形态 | 上游字节（LE） | 32 位字 |
> | --- | --- | --- |
> | `abs.8b v0, v0` | `00 b8 20 0e` | 0x0E20B800 |
> | `abs.16b v0, v0` | `00 b8 20 4e` | 0x4E20B800 |
> | `abs.4h v0, v0` | `00 b8 60 0e` | 0x0E60B800 |
> | `abs.8h v0, v0` | `00 b8 60 4e` | 0x4E60B800 |
> | `abs.2s v0, v0` | `00 b8 a0 0e` | 0x0EA0B800 |
> | `abs.4s v0, v0` | `00 b8 a0 4e` | 0x4EA0B800 |
>
> 规律：`0 Q U 01110 size 10000 01000 Rn Rd`（`Q` = 排列宽度是否为 128 位、`size` = 元素宽度
> b/h/s = 00/01/10、`U` = 0 取绝对值 / 1 取负数即 `neg`；`abs` 不支持 `.d`）。**常量段不连续**，
> 所以字段要这样切（每段都与 `Q`/`U`/`size`/`Rn`/`Rd`/`Rm` 不相交）：
> `vec_a = [28:24] = 0x0E`、`vec_c = [21:21] = 1`、`vec_d = [15:12] = 0x8`；
> 变量段 `vq = [30]`、`vu = [29]`、`vsize = [23:22]`、`rn`、`rd`、`rm`（2 寄存器族的 `rm = 0`，
> 用 `fields = { rm = 0 }` 常量即可，与 `MOVR` 的 `fields = { rn = 31 }` 同法）。
> ⇒ 一个 `[[forms]]`（`opcode_field = "vec_a"`, `opcode = 0x0E`, `operand_fields = ["rd", "rn"]`）
> 再加 6 条指令（各带 `fields = { vq = …, vu = 0, vsize = …, vec_c = 1, vec_d = 8, rm = 0 }`）
> 与 6 条 `[[vectors]]`（上表字节）。`neg` 同形、`vu = 1`（其字节同法从语料取，别凭记忆写）。
>
> **`abs` 族首落地受挫（2026-10-07，已回退）**：按上表字节与字段分解生成 6 条 `abs.<arr>`（槽先取
> `fpr`，`validate`/`lint` 均过），但**生成期用例成片失败**（`spec_abs8b`/`abs2s`/`abs8h`/`abs4h`/
> `abs4s` 及其 `_vhi` 变体，共 8 条）⇒ 已整批回退（`isa/arm64.toml` 与 HEAD 逐字相同，门禁不受影响）。
> 两个候选原因（下次先各花一步定位，不要一起改）：① **显式 `vq` 字段与槽类宽度的耦合**——槽的
> 寄存器类自带宽度，而我又把 `Q` 位显式拆成 `vq`，两者可能互相矛盾；② **D/Q 视图选错**——
> `abs.8b` 是 64 位视图（D）、`abs.16b` 是 128 位视图（Q），而我给 6 条用了同一个槽。
> 生成器（3 个常量位域 + `VEC2R` form + 6 指令 + 6 上游向量）已按 ASCII 锚点写好，改动很小，
> 定位到原因后即可一次落地。
>
> **`abs` 字段分解错在哪（2026-10-07 实证，已回退）**：带 `--nocapture` 跑出断言原文——不是解析问题，
> 而是**编出来的字与上游不同**，且解码器连自己编的字都解不开：`ABS2S` 得 `0x0F2403FE`（应为
> `0x0EA0B800`）、`ABS4H` 得 `0x0EA40020`（应为 `0x0E60B800`）、`ABS16B` 得 `0x4E240020`（应为
> `0x4E20B800`）⇒ 我的常量段切法**错了**，别照抄上一轮表里的 `vec_a/vec_c/vec_d`。
> 从对照可直接读出两处：① `vec_d` 不该是 `[15:12] = 0x8`——上游字里 `0xB800 >> 11 = 0b10111`
> ⇒ 常量段至少要到 `[15:11] = 0x17`；② 编出的字里 `[28:24]` 出现多余置位（`0x0F…` 而非 `0x0E…`），
> 说明 `vec_a`/`vsize` 这一带的落点也要重核。**正确做法**：先把 `abs.8b v0, v0` 一条按"上游字节
> 逐位反推字段"做对（一位一位对 `0x0E20B800`），再复制到其余排列——而不是一次铺 6 条。

- aarch64（第四十一批：**NEON `abs` 族 6 条**）：排列写在**助记符后缀**上，**零机制改动**（`v0` 就是既有 `fpr` 槽里的名字）——这是 NEON"两步走"第一步的首次落地。编码
  `0 Q U 01110 size 10000 01000 Rn Rd`，常量段不连续 ⇒ 三段 `vec_a[28:24]=0x0E`、`vec_c[21]=1`、`vec_d[15:10]=0x2E` + 变量段
  `vq[30]`/`vu[29]`/`vsize[23:22]`（**`vsize` 的 offset 是 22 不是 23**：写成 23 会与 `vec_a` 在 bit 24 重叠，
  `LINT-BITFIELD-OVERLAP` 会抓出来）。6 条字节取自语料 `arm64-advsimd.s` 上游 CHECK 行并进 `[[vectors]]`。
  结果：parsed 1924 → **1930**、`no_prefix` −6、**红行 1369 不变**（零红增益）；
  守卫：454 → **460**、枚举器 2213 → **2225**、未指定位 176 → **182**；棘轮刷新。

- aarch64（第四十二批：**NEON `neg` 族 6 条**）：复用 `VEC2R` 与既有位域，只把 `vu` 置 1——语料
  `neg.8b v0, v0` = `0x2E20B800` 正是 `abs.8b` 的字翻 bit29，**规则经上游字节证实**；其余排列按
  同一规则推得，并由生成期往返 + 6 条 `[[vectors]]` 双向钉住。结果：`parsed` 1930 → **1936**、
  `no_prefix` −6、**红行 1369 不变**；守卫 460 → **466**、枚举器 2225 → **2237**。
  另：`cnt`/`mvn`/`cls`/`clz`/`rev16/32/64` 的上游字已由脚本自动提取（`target/neon-pairs.json`），
  按同一张表即可铺开——**别手抄字节**。

- aarch64（第四十三批：**NEON 双寄存器 7 族 42 条**）：新工具 `gen-aarch64-neon-pairs.mjs` **读语料自动派生**——从各族 .8b 的上游字反推 (U,c,d)，
  其余排列只换 (Q,size)。
  向量只写**有上游字节**的排列（8 条），其余靠生成期往返钉住；守卫 466 → **508**、枚举器 2237 → **2321**。

## 语料扫描的并行度（2026-10-07）

已并行：`src/asm/par.rs::map_parallel`（`std::thread::scope` + 原子取号，结果按入参序归并 ⇒ 与串行
逐字节一致；`AsmTarget: Sync`），`asm_parse` 接入；`fetch.mjs` 的打分改成两个档**并发**子进程。
实测（20 核、vendored 集 125 份）：`asm_parse` **3 秒**、`fetch.mjs` 裁回整轮 **167 秒**。

**还剩一处（结构已看清，可直接照做）**：`tests/asm_encoding.rs` 的逐文件循环（第 45–115 行的
`for (file, src) in &files {}`）仍是串行。它与共享累加器耦合（逐例改 `r`/`row`、发事件、pushes
`variants`/`known`）。改法：把该段抽成**每文件纯函数**返回一个局部结构
`{ row, dropped, variants, known, events: Vec<(case, verdict)> }`（`report_case` 的三元进 `events`
而不当场发），用 `asm::par::map_parallel(&files, …)` 并行算，然后**按文件序**回放：累加 `r`、
按序 `report_case` 发事件、pushes 两个 Vec。这样记分板/棘轮/事件序与串行完全一致。

> 编码档内部并行已落地（每文件纯函数 + 按文件序回放，erify 证明与串行逐字节一致）：sm_encoding **3 秒**、etch.mjs 整轮 167 → **96 秒**（20 核、125 份）。

- aarch64（第四十四批：**NEON 三寄存器族 13 族 19 条**）：生成器 `gen-aarch64-neon-3r.mjs` 从语料字节反推 `(Q,U,size,vec_c,vec_d)`，只新增
  `VEC3R` form。三个坑：操作数计数、注释尾、**lane 形态**（`v0[1]` 属另一族，连卡三轮的真因）。守卫 508 → **527**、枚举器 2321 → **2359**、未指定位 230 →
  **249**（该值只在工作区门禁里出现）。

  实测（第四十四批）：aarch64 `parsed` 1940 → **1960**、红行 1369 → **1371**、编码 `known` 57 → **57**。
  
  **归因（+2 红行）**：`parsed` +20 里有 2 条是 **lane 形态**（`mla.8h v0, v0, v0[1]`）——以前整条 `no_prefix`（不计缺陷），
  现在我们的三寄存器形态吃掉了前三段、尾巴 `[1]` 不齐 ⇒ 记为 tail_mismatch。
  **棘轮不受影响**（那两条在**已淘汰**的文件里，棘轮只计 vendored 集）；要消掉得把 lane 族**实现**（`v0[1]` 是另一族），已排入 NEON 队列。

## lane 族（`v0[1]`）的落地设计（2026-10-07 勘察，含证据）

**结论：不需要任何新机制**——把方括号写成**模板里的字面段**、把索引当成**普通 imm 槽**即可：

```toml
ops = ["dst:fpr:out", "src:fpr", "src2:fpr", "lane:imm_lane5"]
asm = "mla.8h {dst}, {src}, {src2}[{lane}]"
```

`{lane}` 用普通 imm 槽（宽度按族取，如 5 位），字面段 `[` / `]` 由模板承担 —— 与"助记符后缀"那批
同一套思路（关键词/标点在模板里，值走槽）。语料证据已抽好：**157 条 lane 写法**（脚本
`target/lane-extract.mjs`，输出 `target/lane-pairs.json`），例如

```text
dup.2d  0x4E180460 / dup.2s 0x0E0C0460 / dup.16b 0x4E030460
mla.8h  v0, v0, v0[1]（此前被三寄存器过滤误收，是第四十四批 +2 红行的来源）
mov x16, … 0x5E030483（寄存器↔lane 的混合族）
```

按族落地时照既有两个 NEON 生成器的三步走：**取上游字节 → 反推 (Q,U,size,imm 落点) → 写指令 + 向量**；
每族先只做有上游字节的排列。

### lane 族的**反推可行性**约束（2026-10-07 勘察，落地前必读）

前两批 NEON 之所以能"零记忆、全反推"，是因为每个 `(族, 排列)` 在语料里都有上游字节，且常量段
与变量段可分离。**lane 族不满足这一点**：lane 索引与常量段**共用同一批位**（如 `imm5`），要从字节
反推"哪几位是索引"，**同一个族至少要两条 lane 索引不同的上游行**才能分离——只有一条时无法区分
（`mla.8h v0, v0, v0[1]` 在语料里就只出现一次 ⇒ 索引位与常量位不可分离）。

⇒ 两条可行路径（择一，别硬推）：

1. **按 ARM ARM 写死 lane 落点**（如 5 位 `imm5` = `{offset=16,width=5}`，各族按规范给 size/Q），
   再用**现有向量**校验（`dup` 已抽到 7 个排列、`mov.{b,h,s,d}` 各 8~12 条，够校验）；
2. **先做"有 ≥2 条不同索引"的族**（`mov.s` 12 条 / `mov.d` 12 条 / `dup.*` 各 2 条），用它们的
   字节差**自己反推**索引位落在哪——这也是把路径 1 的假设**验证掉**的办法。

口径提醒：落地时仍按"取上游字节 → 写指令 + 向量"三步走，且**先只做有上游字节的排列**。

**路径 2 已实测不可用（2026-10-07）**：我写了脚本把"去掉索引后操作数完全相同、仅索引不同"的 lane 行
配对做 XOR 来反推索引位（`target/lane-bits.mjs`），跑遍 `arm64-advsimd.s` 的 157 条 lane 行——
**一组都没配上**：语料里 lane 行的**寄存器也各不相同**，没有干净对照 ⇒ **无法用字节差反推索引落点**。

⇒ **lane 族只能走路径 1**：按 ARM ARM 给出 lane 落点（如 5 位 `imm5`，各族按规范定位 size/Q），
再拿现有 157 条向量**校验**。也就是说：这条族的落地**必须查规范**，"零记忆"那套在这里不适用——
这是它和前三批 NEON 的本质差别。

### 寄存器侧排列后缀（`v0.16b`）的**正确机制**（2026-10-07 两次实测后的结论）

**先做了错的那一版**：在 reg 槽解析后"吃掉"紧随的点号段（`.16b`/`.b`…），不改词法。实测**零变化**
（`parsed` 1960 不动）⇒ 因为语料里带寄存器后缀的行大多是**助记符不带后缀**的写法：

```text
add v0.16b, v1.16b, v2.16b      ← 排列信息**只在寄存器上**
add.16b v0, v1, v2              ← 排列写在助记符上（这批已落地：双/三寄存器族）
```

第一种写法里，**排列正是编码依据**（决定 `Q` 与 `size`）⇒ **"吃掉"是错的**（丢掉了选择变体所需的
信息），而且不能靠"同形多候选"分派（同文本形状的 7 个排列会都命中、分派只会取第一条 ✗）。

⇒ **正确机制**（与 `require_symbol` / `imm_fns` 同型的**槽键**）：

1. reg 槽加键 `arrangement = "16b"`（或 `8b`/`4h`/…）——含义是"**本变体要求**该寄存器的后缀是它"；
2. **解析器**解析完寄存器后，把紧随的排列后缀**读出来并交给候选选择**（匹配则记下、不匹配则整条不匹配），
   即排序/分派要能看见它；
3. 谱侧为每个 `(族, 排列)` 各写一条**助记符不带后缀**的变体（`asm = "add {dst}, {src}, {src2}"`），
   由槽键区分 ⇒ 文本形状相同的多条不再互相遮蔽。

这条路覆盖 **4676 行 / 88 个文件**，是 NEON 里最大的一块；机制三处（槽键 + 校验 + 解析/分派），
与 `imm_fns` 那次同规模。

## 【破坏性重设计】NEON 排列：从"每族×每排列各一条"改为"排列是槽的参数"

**现状（冗余设计，应当删掉）**：NEON 的排列被展开成**指令 ×7** ——`ABS8B`/`ABS16B`/`ABS4H`…
`VADD8B`/`VADD16B`… 约 **100 条**声明，只差 `Q`/`size` 两个字段。更要命的是它**只覆盖语料的少数写法**：
语料里带寄存器后缀的行（`add v0.16b, v1.16b, v2.16b`）是**助记符不带后缀**的，现有声明**一条也接不住**
（实测 `parsed` 零变化 ⇒ 4676 行/88 文件仍全在门外）。

**目标设计（一条指令覆盖全排列）**：排列**是寄存器的后缀**，因此它是**槽的参数**，不是指令的身份。

```toml
[[operand_slots]]
name = "v"
kind  = "reg"
class = "fpr16"
arrangement = ["8b", "16b", "4h", "8h", "2s", "4s", "2d"]   # 允许的排列（空/省略 = 不带后缀）
arrangement_fields = { q = "vq", size = "vsize" }           # 后缀 → 字段：16b/8h/4s/2d ⇒ q=1；size 按 b/h/s/d

[[instructions]]
name = "VADD"
form = "VEC3R"
opcode = 0x0E
ops = ["dst:v:out", "src:v", "src2:v"]
asm = "add {dst}, {src}, {src2}"        # 助记符**不带**后缀（与语料一致）
```

**要落地的三件事（一次做完，不逐族打补丁）**：

1. **槽键**（模型 + 校验 + schema + 文档）：`arrangement`（允许集）与 `arrangement_fields`（映射到哪两个字段）；
2. **解析**：reg 槽读完寄存器后，把紧随的 `.Nt` **读出来**（不是吃掉 ✗ ——吃掉正是上一轮零收益的原因），
   校验它在允许集内，并把 `Q`/`size` **写进对应字段**；渲染时按同一张表**写回**后缀，
   使 `disassemble → assemble` 闭合；
3. **谱侧破坏性替换**：删掉约 **100 条** `族×排列` 声明 ✗，改为**每族一条**（`add/sub/and/orr/eor/bic/orn/eon/
   smax/smin/umax/umin/mul/mla/abs/neg/cnt/mvn/cls/clz/rev16/32/64` 等）✓；守卫计数随之**下降**
   （这是好事：删冗余 ⇒ 指令总数与未指定位清点都要按实际重算）。

**收益**：覆盖从"少数写法"扩到 **4676 行 / 88 文件**（本线剩余最大一块），且谱面**净减约 85 条声明**。

**风险与顺序**：先在一个族（`add`）打通"解析→字段→渲染→向量"全链，再一次性替换其余族并删旧声明；
`spec_coverage_guard` 的 `arm64` 计数与歧义名单会**大幅变化**（预期下降）——按"日志证明失败项"的口径重算。

## 【实现决策点】排列代码在 `Inst` 里的落脚处（2026-10-07 验证后的结论）

`arrangement` 的**契约层**已落地（模型/校验/schema/文档 ✓）。动手做解析与渲染前必须先定这一条：

**约束**：编码与解码都只读 **`Inst` 字段的值**（`encode` 里 `<Reg as PhysReg>::to_index(*#fid)`、
`slice` 分支里 `*#fid as u64`）。而 reg 槽的字段类型是 **`Reg`** ✗ —— 排列的**代码值在 `Inst` 里
没有落脚处** ✗。所以"把后缀读出来交给切片编码"这句话，落地时等于**给 Inst 增字段**。

两个候选（择一，别两个都试）：

1. **每个排列槽一个附加字段**（建议 ✓）：生成器为每个带 `arrangement` 的 reg 槽追加一个
   `Inst` 字段（机械命名，如 `arr_dst: u8` ✓），存放代码值。于是：编码把**寄存器**落进它的字段、
   把**代码值**按 `fields` 切片落进 `vq`/`vsize` ✓；渲染把它反查成后缀写回 ✓；解码把两块分别读回 ✓。
   **不破坏"一个操作数 ↔ 一个字段"的既有不变量**（排列是**附加**字段）✓，也不需要新 DSL 概念 ✓。
2. **让解码从位域反推**（不加字段）：`vq`/`vsize` 本来就在字里 ✓，理论上可反推后缀 ⇒ 不必存。
   但解码侧目前只会把**槽声明的字段**读回 `Inst` ✗，而 `vq`/`vsize` 不是槽 ✗ ⇒ 仍要新增
   "解码后重建排列"的通路 ✗，且**文本往返**要额外保证 ⇒ 比候选 1 复杂 ✓ 不推荐。

**建议顺序**（一次做完、一次提交）：① 按候选 1 生成附加字段 + 编码/解码/渲染三处接通；
② 在 `add` 一族用 `arrangement = { "8b" = 0, … }` + `fields = ["vq","vsize"]` 打通全链（配向量）；
③ 通过后**破坏性替换**：删掉约 100 条 `族×排列` 声明、改为每族一条，重算守卫计数（预期下降）。

### 排列机制第三次尝试：**命名立即数 + 切片**（结构可行，卡在一个匹配细节）

新发现的最省路线（**不需要 `arrangement` 槽键、也不需要动 `Inst`**）：

```toml
[conventions.imm_names.arr]        # 直接键（不是 names = {...} 包裹！模型是 name -> i64）
"8b" = 0 ; "16b" = 1 ; "4h" = 2 ; "8h" = 3 ; "2s" = 4 ; "4s" = 5 ; "2d" = 6

[[operand_slots]]
name = "varr" ; kind = "imm" ; width = 3 ; names = "arr"
encode = "slice" ; fields = ["vq", "vsize"]      # 代码 = size*2 + q

[[forms]]  name = "VEC3RA" ; operand_fields = ["rt","rn","rm","vq","vq","vq"]
[[instructions]] name = "VADD" ; form = "VEC3RA"
ops = ["dst:fpr:out","src:fpr","src2:fpr","arrd:varr","arrs:varr","arrs2:varr"]
asm = "add {dst}.{arrd}, {src}.{arrs}, {src2}.{arrs2}"
```

**已实测到的**：`validate`/`lint` **通过** ✓；删掉 `add` 的 6 条 `族×排列` 变体、只留 1 条 ⇒
指令总数 **527 → 522**（冗余确实在减少 ✓）；旧拼法的上游向量可**同字节改写**成新拼法
（`add.16b v0, v0, v0` → `add v0.16b, v0.16b, v0.16b`，证据不丢 ✓）。

**卡住的一步**：生成期用例渲染出 `add V0.8b, V1.8b, V2.8b` 后**解析不回来**
（`no matching instruction`）✗ —— 即 `{arrd}` 的**命名立即数匹配**没吃下 `.8b` 这一段
（token 流是 `Ident("V0") Dot Ident("8b")`；模板字面 `.` 该匹配 `Dot`，随后命名表该匹配 `"8b"`，
但实际没匹配上）。下一步只需查清"命名立即数的名字是**按 token 匹配**还是按**文本**匹配"
（`dsl/codegen/asm.rs` 的 names 分支），据此调一处即可——**机制方向已被证明可行**。

### 排列机制第四次尝试：**成功但词法改动会崩**——改用"读时切分"（2026-10-07）

**成功的那一半**：`[conventions.imm_names.arr]`（直接键）+ `varr` 槽（`names`/`slice`/`fields`）+
`VEC3RA` 表单 + 模板 `add {dst}.{arrd}, {src}.{arrs}, {src2}.{arrs2}`，在**词法把 `V0.8b` 切成
`Ident("V0") + Dot + Ident("8b")`** 的前提下：`validate`/`lint` 通过 ✓，`cargo test -p forge-codegen
--lib spec_vadd` **14 passed / 0 failed** ✓ —— 即"**一条指令覆盖全排列**"确实成立 ✓（这是本设计的目标）。

**崩掉的那一半**：那处词法改动（标识符延续**只认点开头**的点号）会让**整跑**崩
（`STATUS_STACK_BUFFER_OVERRUN`，0xC0000409 ✗）⇒ 它有别的输入会踩到，**不能这么改** ✗。

⇒ **安全做法（下次直接照做）**：**不碰词法** ✗，改为**读时切分**——词法器仍给出 `Ident("V0.8b")`
✗，由**操作数解析**（reg 槽那一步）把它**在第一个 `.` 处切成"寄存器名 + 排列后缀"** ✓：
寄存器部分照常查表 ✓，后缀部分交给排列逻辑 ✓，且**只对带 `arrangement` 的槽生效** ✓（其它槽一字不动 ✓
⇒ 零回归面 ✓）。渲染侧反向拼回 `V0.8b` ✓。这样既拿到 14/14 的效果，又不引入词法崩溃 ✗。

### 排列机制第五次尝试：读时切分**编译通过**，但暴露出"两侧不一致"

读数：`arm64=522`、`enum=2361` ✓（与第四次试验一致 ⇒ **机制侧成立** ✓）；`spec_vadd` 14/14 ✓。
但整跑里 **`abs` 族全体失败** ✗，断言原文是**渲染文本回不去**：

```text
[ABS8B] assemble("abs.8b V0, V1") 失败: no matching instruction
```

**根因（一致性，不是设计）**：我的"读时切分"只作用于**运行时文本**的词元流 ✓，而模板里的**字面段
匹配器是生成期烤好的** ✗（`lit_seg_expr` 在生成期把 `abs.8b` 编成**单个** `Ident("abs.8b")` ✗）⇒
运行时被切成 `Ident("abs") + Dot + Ident("8b")` ✗、生成期仍是整块 ✗ ⇒ 两边形状不一致 ✗。

⇒ **修法只有一条**（下次直接做）：切分必须**两侧同源**——要么在 `lit_seg_expr` 里对字面段做**同一套
切分** ✗（生成期 ✓），要么**不做整行预切** ✗、改为**在 reg 槽的操作数解析里就地切**（只对该槽看到的
那一个 token ✓，字面段根本不经过那里 ✓ ⇒ 天然一致 ✓✓，且只对带 `arrangement` 的槽生效 ⇒ 零回归面）。
**推荐后者**（第二方案）✓。

## 排列机制已落地（2026-10-07）：读时切分 + 两侧同源

运行时 `__lex` 后把 `Ident("V0.8b")` 切成 `Ident` + `Dot` + `Ident`；**生成期字面段做同一套切分**
（`split_arrangement_ident`）——只切一侧会让 `abs.8b` 族全体失败（实测）。`add` 族新增 `VEC3RA`
表单 + `varr` 槽（`imm_names.arr` + 既有 `slice`/`fields`）⇒ **一条指令覆盖全排列**，并保留助记符
后缀拼法（两种语法并存，不是冗余）。排列代码 `code = size*2 + q`，**`2d` 是 7 不是 6**（写 6 会让
`.2d` 编成 `q=0`，编码 `known` +1）。账：`parsed` 1960 → **1968**、`no_prefix` −65、`checked`
997 → **1004**、`known` 57 不变；红行 +57 已归因（切分让"支持助记符但未支持其后缀拼法"的行由
`no_prefix` 变 `tail_mismatch`，都在已淘汰文件里，棘轮不受影响）。

### 新拼法铺开的实测（2026-10-07，已回退）

用同一套"从语料 `.8b` 字反推 `(vu, c, d)`"的办法给 12 个族（`sub/and/orr/eor/bic/orn/smax/smin/
umax/umin/mul/mla`）加了新拼法指令（`VEC3RA` + `varr`），实测：

| 指标 | 铺开前 | 铺开后 |
| --- | --- | --- |
| 指令总数 / 枚举器 | 528 / 2373 | 540 / 2541 |
| 语料 `parsed` | 1968 | **2053（+85）** |
| `no_prefix` | 14219 | **14137（−82）** |
| 红行 | 1428 | **1425（−3）** |
| 编码 `checked` | 1004 | **1057（+53）** |
| 编码 **`known`** | 57 | **63（+6 ✗）** |

**为什么回退**：`known` +6 = 有 6 行**新拼法**的编码与上游不符 ⇒ 是**真错** ✗（不是"可见化"）。
⇒ 结论：**只用 `.8b` 一个字反推 `(vu, c, d)` 对某些族不够** —— 族的常量在别的排列上可能还随
size 变化（`smax/smin/umax/umin` 这类带"有符号/宽度"语义的族最可疑；`2d` 排列也要单独核对 ✗）。
**下次正确做法**：每族**至少取两个排列的上游字**（如 `.8b` + `.4s`）来验证常量是否与排列无关 ✓，
不一致就按排列分别给常量（或另设字段）✓；**别再一次铺 12 族**。

`add` 族（单族、逐排列核过）**保留** ✓：它是零 `known` 增长、红行零上升的那一个 ✓。
