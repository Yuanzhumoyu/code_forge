# rust-analyzer 已知假阳性与二分方法

> [active]（2026-09-22）。本文件记录**本仓在 rust-analyzer 下看到、但 `rustc`/`clippy`/
> 测试全绿**的诊断，以及定位它们的方法。**一切以 `rustc`/门禁为准**；本文件只为省掉
> "再排查一遍"的时间。

## 1. `isa_from_file!` 调用行上的 `expected expression` / `expected R_PAREN`

**症状**：RA 在 `crates/backend/forge-codegen/src/arch/*.rs` 的
`forge_dsl::isa_from_file!("isa/*.toml");` 那一行报若干条

```text
Error SyntaxError ... Syntax Error in Expansion: expected expression
Error SyntaxError ... Syntax Error in Expansion: expected R_PAREN
```

（成对出现，各 N 条 ⇒ 该行共 2N 条。）**`cargo build/check/test/clippy` 全绿**，与这些
诊断无关。

**实测分布**（2026-09-22，`rust-analyzer diagnostics <file>`，本机）。计数单位是**对**
（每对 = 一条 `expected expression` + 一条 `expected R_PAREN`，同一 span；日志里按
`file …: Error SyntaxError` 分组后 2 条 = 1 对）：

| 文件 | 对数（每类） |
| --- | ---: |
| `src/arch/riscv64_v12.rs` | 15 |
| `src/arch/arm64_v12.rs` | 4 |
| `tests/common/mod.rs`（6 个夹具） | 9 |
| `tests/spec_tests_v12.rs`（3 个夹具） | 6 |
| **`src/arch/x86_v12.rs`** | **0** |

**已排除的原因**（都有实测证据，别再重复排查）：

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

**二分结果**（一次 RA 运行，探针文件里放多个 `parts` 变体）：

| `parts` | 结果 |
| --- | --- |
| `["encode"]` | 0 |
| `["decode"]` / `["asm"]` / `["tm"]` | 0 |
| `["encode","decode"]` / `["encode","asm"]` / `["encode","tm"]` | **8** |
| `["encode","decode","asm"]` | **8** |
| `["decode","asm"]` / `["decode","tm"]` / `["asm","tm"]` | 0 |

⇒ 触发规律：**`encode` 与至少一个别的部件同时生成**；且只在**定宽/混合字长**的 ISA
（arm64、riscv64、demo 夹具）上出现，**变长 `prefix_scan`（x86）从不出现**。
据此判断为 **RA 侧 proc-macro 展开管线的问题**（我们的 token 流本身合法），不是本仓缺陷。

**补充线索**：诊断的 span 是**调用行上宏路径那一段**——`src/arch/riscv64_v12.rs`
第 6 行（RA 的 `LineCol` 0 基第 5 行）`col 0..25` 正好是 `forge_dsl::isa_from_file!`
这 25 个字符。也就是说 RA 把"展开结果没解析成功"的位置**回填到宏路径本身**，
而不是任何具体 token 或生成物内部位置。

**状态**：**未修**（不阻塞任何门禁）。

**已试过且无效的方向（2026-09-22 实测，别再试）**：

- **给生成物 token 换 span 卫生性**：在 `expand_loaded` 里对生成好的 `inner`/`helpers`
  整体调 `set_spans(ts, Span::mixed_site())`（commit 前已回退）。结果：
  `cargo build -p forge-isa-dsl` 无警告、`cargo test -p forge-codegen`（27 个二进制）
  全绿——**没有卫生性破坏**，但 RA 诊断条数**逐文件一模一样**
  （`src/arch/arm64_v12.rs` 4 对、`src/arch/riscv64_v12.rs` 15 对、
  `tests/common/mod.rs` 9 对、`tests/spec_tests_v12.rs` 6 对，与上表改动前完全一致）
  ⇒ 与 span / 卫生性、与 token 的 `SyntaxContext` **无关**，此路不通。

同一次 RA 运行里还能看到 `forge_rustc` 的 `unresolved-extern-crate` / `E0282`
等诊断——它需要 rustc 私有组件才成立，本机 RA 配不全，属同类"RA 环境性"噪声；
`cargo check -p forge-rustc` 门禁是绿的。

**二分方法（可复用，别只看调用行）**：

```bash
# 1. 造探针（一次 RA 运行就能按调用行分别计数）：文件里放多个 isa_from_file!，
#    各给不同 name、spec_tests = false、不同 parts。
#    crates/backend/forge-codegen/tests/zz_ra_probe.rs
# 2. 跑 RA（10–20 分钟，务必用后台任务：前台工具调用有 10 分钟上限）
rust-analyzer diagnostics crates/backend/forge-codegen/tests/zz_ra_probe.rs > target/ra.log 2>&1
# 3. 统计：注意 RA 的 LineCol 是 **0 基**，且文件路径里含盘符冒号，
#    正则要用 file (.+?\.rs): Error SyntaxError ... from LineCol \{ line: (\d+)
#    这一正则数的是**条数**（2 条 = 1 对），写进上表前除以 2。
# 4. 跑完删探针文件，保持工作树干净。
```

**注意**：`rust-analyzer diagnostics <单个文件>` 打出来的日志是**整个工作区**的诊断
（含 `forge_rustc` 等别的 crate），按文件分组统计，别把总量当成本文件条数。

## 2. `Invalid escape`（已修，v18 S10a）

RA 会把 Rust 的**字符串续行**（`"…\<换行><缩进>…"`，语义 = 换行与前导空白被吃掉）
判成非法转义。2026-09-22 已把本仓 6 个文件里的这类写法（22 处诊断）并成一行，
逐字节保持字符串值不变；提交 `1405047`。
