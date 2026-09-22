# rust-analyzer 已知假阳性与二分方法

> [active]（2026-09-23）。本文件记录**本仓在 rust-analyzer 下看到、但 `rustc`/`clippy`/
> 测试全绿**的诊断，以及定位它们的方法。**一切以 `rustc`/门禁为准**；本文件只为省掉
> "再排查一遍"的时间。

## 1. `isa_from_file!` 调用行上的 `expected expression` / `expected R_PAREN` — **已修（v18 S10d）**

**症状（历史）**：RA 在 `crates/backend/forge-codegen/src/arch/*.rs` 的
`forge_dsl::isa_from_file!("isa/*.toml");` 那一行报若干条

```text
Error SyntaxError ... Syntax Error in Expansion: expected expression
Error SyntaxError ... Syntax Error in Expansion: expected R_PAREN
```

（成对出现，各 N 条 ⇒ 该行共 2N 条。）**`cargo build/check/test/clippy` 全绿**，与这些
诊断无关。

**实测分布（修前）**（2026-09-22，`rust-analyzer diagnostics <file>`，本机）。计数单位是
**对**（每对 = 一条 `expected expression` + 一条 `expected R_PAREN`，同一 span；日志里按
`file …: Error SyntaxError` 分组后 2 条 = 1 对）：

| 文件 | 对数（每类） |
| --- | ---: |
| `src/arch/riscv64_v12.rs` | 15 |
| `src/arch/arm64_v12.rs` | 4 |
| `tests/common/mod.rs`（6 个夹具） | 9 |
| `tests/spec_tests_v12.rs`（3 个夹具） | 6 |
| **`src/arch/x86_v12.rs`** | **0** |

**修前已排除的原因**（都有实测证据，别再重复排查）：

1. **不是生成物的语法错**：用 RA 自己的解析器解析整份 arm64 生成物
   （`rust-analyzer parse < %TEMP%\forge_gen_arm64_v12.rs`，先 `FGE_DEBUG_GEN=1` 构建）
   得到 30 MB 语法树、**零 `ERROR` 节点**；
2. **不是"函数体内嵌 item"**（历史上真有过一次，S8d 已修）：生成物所有 item 都在
   `pub mod` 内（brace-depth = 1）；
3. **不是不可见定界组**：全仓无 `Delimiter::None`；
4. **不是 `spec_tests` / `krate`**：探针里 `spec_tests = false` 与带 `krate` 都照样复现；
5. **不是体积上限**：x86 的完整展开（含 `spec_tests`，1.85 MB）**0 条**，而 arm64 的
   `encode + decode`（106 KB）8 条；
6. **不是拼接顺序/接缝**：`parts = ["encode"]` 与 `["encode","decode"]` 两份生成物里，
   `pub fn encode` 正文**逐字节相同**（公共前缀 57,804 字符），发射顺序也稳定
   （各部件在前、共享 base 块在后）。

**修前的二分结果**（一次 RA 运行，探针文件里放多个 `parts` 变体）：

| `parts` | 结果 |
| --- | --- |
| `["encode"]` | 0 |
| `["decode"]` / `["asm"]` / `["tm"]` | 0 |
| `["encode","decode"]` / `["encode","asm"]` / `["encode","tm"]` | **8** |
| `["encode","decode","asm"]` | **8** |
| `["decode","asm"]` / `["decode","tm"]` / `["asm","tm"]` | 0 |

⇒ 触发规律：**`encode` 与至少一个别的部件同时生成**；且只在**定宽/混合字长**的 ISA
（arm64、riscv64、demo 夹具）上出现，**变长 `prefix_scan`（x86）从不出现**。据此判断为
**RA 侧 proc-macro 展开管线的问题**（我们的 token 流本身合法），不是语法缺陷。

**补充线索**：诊断的 span 是**调用行上宏路径那一段**——`src/arch/riscv64_v12.rs`
第 6 行（RA 的 `LineCol` 0 基第 5 行）`col 0..25` 正好是 `forge_dsl::isa_from_file!`
这 25 个字符。也就是说 RA 把"展开结果没解析成功"的位置**回填到宏路径本身**，
而不是任何具体 token 或生成物内部位置。

**修法与实测（v18 S10d，2026-09-23）**：生成物不再作为宏展开结果交给 RA——它落到
`$OUT_DIR/forge_gen_<模块名>_<参数哈希>.rs`，展开只剩一句
`include!(concat!(env!("OUT_DIR"), "/…"))`；文件由**宿主的 build script 预生成**
（`forge_isa_dsl::pregenerate_host()`，见 `docs/reference/isa-dsl.md`「宿主接入」）。

| 文件 | 修前对数 | 修后对数 |
| --- | ---: | ---: |
| `src/arch/riscv64_v12.rs` | 15 | **0** |
| `src/arch/arm64_v12.rs` | 4 | **0** |
| `tests/common/mod.rs` | 9 | **0** |
| `tests/spec_tests_v12.rs` | 6 | **0** |

同一批 `include!` 的生成物也没有引入新的 unresolved import（20+ 个使用生成模块的
文件恢复 0 错误——这一点必须一起看，否则"语法错没了、项全解析不出来"是**更糟**的结果）。

**改用 `include!` 后必须知道的两条实测约束（别再试别的路子）**：

1. **RA 只在分析开始前登记一次可加载文件**。它自己会跑 build script（本仓 `forge-ir` 的
   `$OUT_DIR/opcode_gen.rs` 就是这么被 RA 正常加载的），但**看不见**分析开始后才由 proc
   宏写出的文件：实测那个文件确实存在于 RA 自己的 OUT_DIR、mtime 也比诊断早，RA 仍报
   `macro-error: failed to load file …`，随后生成模块的项在 20+ 个文件里全变
   "unresolved import"。所以生成物**必须**在 build script 阶段就位。
2. **`TokenStream::to_string()` 与上下文有关**：同一 token 流，在 proc 宏里（rustc 的
   美化打印，带换行缩进）与在普通二进制里（proc-macro2 的紧凑打印）产出的文本不同
   （x86 全部件：2,655,722 B vs 1,336,624 B，首个差异在偏移 15）。若宏侧与 build script
   侧都写文件，两侧会**互相覆盖**，每次构建都多一轮重编（实测 3 次构建都不收敛）。因此
   生成物**只允许一个写者**：一律由 build script 写，宏侧只发 `include!`；文件缺失
   就 fail-closed 报错。

**已试过且无效的方向（2026-09-22 实测，别再试）**：

- **给生成物 token 换 span 卫生性**：在 `expand_loaded` 里对生成好的 `inner`/`helpers`
  整体调 `set_spans(ts, Span::mixed_site())`（未入库）。结果：
  `cargo build -p forge-isa-dsl` 无警告、`cargo test -p forge-codegen`（27 个二进制）
  全绿——**没有卫生性破坏**，但 RA 诊断条数**逐文件一模一样**
  （arm64 4 对、riscv64 15 对、`tests/common/mod.rs` 9 对、`tests/spec_tests_v12.rs` 6 对，
  与上表修前完全一致）⇒ 与 span / 卫生性、与 token 的 `SyntaxContext` **无关**，此路不通。

同一次 RA 运行里还能看到 `forge_rustc` 的 `unresolved-extern-crate` / `E0282`
等诊断——它需要 rustc 私有组件才成立，本机 RA 配不全，属同类"RA 环境性"噪声；
`cargo check -p forge-rustc` 门禁是绿的。

**验证方法（改生成管线后必跑）**：

```bash
# 1. 先让写者就位（build script 预生成 = RA 也会走的路径）
cargo build -p forge-codegen
# 2. 跑 RA（10–20 分钟，务必用后台任务：前台工具调用有 10 分钟上限）
rust-analyzer diagnostics crates/backend/forge-codegen/src/arch/arm64_v12.rs > target/ra.log 2>&1
# 3. 统计两类：SyntaxError 必须为 0；同时看有没有新的
#    RustcHardError（unresolved import / no such value）——后者代表 include! 没被加载。
#    注意 RA 的 LineCol 是 **0 基**，路径含盘符冒号；参考脚本 target/s10d_ra_compare.py。
# 4. `rust-analyzer diagnostics` 打的是**整个工作区**的诊断，按文件分组统计，
#    别把总量当成本文件条数。
```

**二分方法（可复用，别只看调用行）**：探针文件里放多个 `isa_from_file!`（各给不同
`name`、`spec_tests = false`、不同 `parts`），一次 RA 运行就能按调用行分别计数
（`crates/backend/forge-codegen/tests/zz_ra_probe.rs`）；跑完删探针、保持工作树干净。

## 2. `Invalid escape`（已修，v18 S10a）

RA 会把 Rust 的**字符串续行**（`"…\<换行><缩进>…"`，语义 = 换行与前导空白被吃掉）
判成非法转义。2026-09-22 已把本仓 6 个文件里的这类写法（22 处诊断）并成一行，
逐字节保持字符串值不变；提交 `1405047`。

**写新代码时的规矩**：需要长字符串就**写成一行**（rustfmt 不拆字符串字面量），不要用
`\` 续行——S10d 期间又在新增代码里踩过一次（`lib.rs` 的报错消息与生成物头部注释）。
