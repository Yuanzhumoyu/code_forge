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

- riscv64（红桶 **1** 条，已登记为设计取舍；`no_prefix` 已清零）：
  - `jal a0, a0`：**不是**"两寄存器形式的 `jal`"——RISC-V 的 `jal rd, symbol` 只收符号，上游把它当
    **未定义符号 a0**（`CHECK-OBJ: R_RISCV_JAL a0`）。我们拒了"寄存器样子的 ident"当符号（以免
    `jmp rax` 这类走错候选），所以它落在红桶——**不改**（改法牵动所有 ISA 的 label 槽）。
  - **已修**（2026-10-04）：三操作数 `jalr rd, rs1, imm`、位置符号 `.`（回填成"当前指令自身的块下标"）、
    `fence` 的**字母集合**（`kind = "bits"` + `[conventions.bitsets.<表名>]`）、以及**立即数位置上的
    符号引用**（`symbols = true`：`%hi(foo)` 这类未定义符号现在报 `UndefinedLabel`（准确诊断）、
    `.Lpcrel_hi0` 这类已定义局部标签真装配；顺带修掉"点开头的标签名被切成 `.` + `Lp`"的**词法缺陷**）。
- aarch64：**没有剩余缺口**——语料里本汇编器能认出来的写法全部通过（`parsed` 153、
  `no_prefix` 0、红桶 0、编码对拍 118/119 逐字节全等）；余下 267 行是语料特性（伪指令/宏/一行多语句/
  跨文件标签）。已修的各族见下面「已经修掉的」里的第十二～十六批。
- x86：**语料全部归因完毕**——`llvm-mc` 档 `parsed` 111 + `corpus_only` 336 = 447 行全覆盖，
  `no_prefix` **0**、红桶 **0**；编码对拍 **checked 104 / unparsed 0 / known 0**。
  `gnu-gas-intel` 余下的 7 条红桶是 32 位模式专有语法（下面「不是缺口」那节）。
- **语料抽取器的一处已知限制**（不是汇编器缺口）：上游把长编码拆成**多条** `CHECK: encoding:`
  注释时（`acquire lock add …` 就是 `[0xf2]` + `[0xf0,0x48,0x01,0x00]` 两行），
  `extract_cases` 按"字节注释 run 长 ≥ 2 ⇒ 整段丢掉"处理（保守配对，避免把几条指令的字节
  错配给同一条）。这两条因此**不在编码档的 104 条里**（字节已人工核对：我们
  `f2 f0 48 01 00` = 上游 ✓）。要纳入得先定"run 里的字节如何归属到哪条指令"。
- x86 `movzx` / `movsx` 的**内存源**形式（`movzx eax, byte ptr [rbx]`、`movsx rax, word ptr [rbx]`）：
  瓶颈不在操作码，而在**文本分不出源宽度**——`byte`/`word` 是 `{size}` 组件，按设计**不携带
  宽度**（只是给人读的提示），于是 `MOVZX_R8_MEM` 与 `MOVZX_R16_MEM` 的汇编文本与类型签名
  完全一样（已在 `spec_coverage_guard` 的歧义名单里登记）；再照搬一套 16/32 位目的地的
  variant 只会让 `movzx eax, word ptr [rbx]` **静默编成 byte 那条**（按声明序取首匹配）。
  要修得先定一件事：`{size}` 是否携带宽度，或把尺寸关键字写成模板字面量——**独立的 DSL 设计项**。
  一元族的**内存形态**（`inc dword ptr [rax]` 这类）已经按同一个思路解决：模板里写死
  `dword ptr` 字面量（`NOT_MEM32` 一族），文本因此不歧义。

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

## 现有语料与计数

（`2026-10-05` 实测；解析档数值以 `asm/ratchet/<isa>.txt` 为准，编码档以
`asm/ratchet/encoding.txt` 为准。x86 是补完 APX/REX2 与真实形态之后的数；riscv64 / aarch64
分别在立即数修饰/访存族与逻辑立即数族之后，此后未变）

| ISA | suite | 文件 | 行数 | parsed | no_prefix | tail_mismatch | corpus_only | 字节 oracle |
| --- | --- | --- | --- | --- | --- | --- | --- | --- |
| x86 | `gnu-gas-intel` | 1 | 64 | **50** | 4 | 7 | 3 | 无（GAS 用例不带期望字节） |
| x86 | `llvm-mc` | 2 | 447 | **111** | **0** | **0** | 336 | **有**：104 条期望 / **104 条逐字节全等 / 0 条差异** |
| riscv64 | `llvm-mc` | 3 | 503 | **136** | **0** | **1** | 366 | **有**：134 条期望 / **134 条逐字节全等** |
| aarch64 | `llvm-mc` | 3 | 420 | **153** | **0** | **0** | 267 | **有**：119 条期望 / **118 条逐字节全等** |

执行档：x86 5 条、riscv64 3 条、aarch64 2 条，**三架构都真跑通**（`ran=10 skipped=0`）。

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
