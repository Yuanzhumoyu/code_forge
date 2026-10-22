# `std` 探测工程（forge-rustc 后端 · v27 M3-②）

用 **forge-rustc** 当 rustc 后端、`std` 由 **LLVM 预编译**（`-Zbuild-std` + `rustc-wrapper`
剥离系统 crate 的 `-Zcodegen-backend`）编译一个真的用到 `std` 的程序。

> 这是**探测**工程，不是产品示例：目标是量出"真 `std` 程序还差什么"。

## 现状（2026-10-18 本机实测）：**已跑通，且与 LLVM 逐行同输出**

```text
$ powershell -NoProfile -ExecutionPolicy Bypass -File .\run-offline.ps1
hello from std
[forge + std] len=11
vec_sum=385
args=1
map_sum=3
fs_len=10
thread_sum=10
all_ok=true
forge exit=0
llvm exit=0
oracle_ok=true（forge 与 LLVM 的完整输出逐行相同）
```

即：`std` 由 **LLVM** 编译、**我们的 crate** 由 forge-rustc 编译。程序有两重身份：

1. **能力梯度**（①–⑦）：`println!`、`String`+`format!`、`Vec`、`env::args`、`HashMap`、
   `std::fs`（写→读→删）、`std::thread::spawn` + `join`（线程启动链 + TLS 初始化/析构 +
   `Box<dyn FnOnce>` 跨线程）；
2. **自校验折磨段**（⑧，20 项）：泛型单态化（i32/u64/f64）、`Box<dyn Fn>` 虚调用、
   迭代器链（`filter`/`map`/`fold`/`collect`）、`sort_by`/`sort_unstable` 闭包比较器、
   浮点 `sqrt`/`powi`/NaN 比较、带数据枚举匹配、整数 rotate/溢出包装/前导零、
   `parse::<i32>`、`BTreeMap` 有序遍历、`HashSet` 去重、**Drop 顺序**、递归（`fib(20)`）。
   **任一项不符就打印 `MISMATCH` 并非零退出**。

**第三方 crate 语料（⑪）**：这些依赖**不在** wrapper 的「系统 crate」名单里 ⇒ **它们也由
forge 后端从源码编译**（`cargo build` 日志会看到 `Compiling itoa v1.0.18 …` 六个）。
这一下把真实写法带进了验证面：`itoa` 的 unsafe 指针写、`ryu` 的 f64 位技巧 +
`transmute`、`memchr` 的 SIMD intrinsic + `cfg` 实现分派、`smallvec` 的 union +
`MaybeUninit`、`once_cell` 的原子态、`bitflags` 的宏展开。**六个都编过且行为正确**，
与 LLVM 对照也逐行相同。

**进阶语料（⑫）**——这三条是本工程最硬的自证：

| crate | 断言 | 为什么硬 |
| --- | --- | --- |
| `crc32fast 1.5.2` | `hash(b"123456789") == 0xCBF43926` | `#[target_feature]` 函数 + 切片表算法；CRC-32 的**标准校验值**，错一位就露 |
| `blake3 1.8.7` | `hash(b"abc")` 的十六进制 = 官方向量 `6437b3ac…d5bd9d85` | 宽 lane 哈希（SIMD 分派 + 大量 unsafe 读）；官方测试向量 |
| `dashu-int 0.6.0` | `2^128` 的十进制 = `3402823669…8211456`；`10^30 % 7 == 1` | 大整数**乘除链**（64×64→128 位乘法、长除），直接压 `u128`/宽值 lowering |
| `hashbrown 0.17.1` | 4 个 key 的 len/sum | swiss-table 的分组位掩码探测 |
| `base64 0.22.1` | `encode(b"hello") == "aGVsbG8="` 且 decode 回原串 | 查表编解码 + 标准向量 |

**正则引擎族（⑬）——目前最重的语料**：`regex 1.13.1` 连同 `regex-automata 0.4.18`、
`regex-syntax 0.8.11`、`aho-corasick 1.1.5` **全部由 forge 从源码编译**（这是生态里泛型/
状态机最重的代码之一：惰性 DFA 构建、大量 `unsafe`、`#[inline]` 泛型巨兽）。行为断言覆盖：
捕获组（`a(b+)c` → `bb`）、`find_iter` 计数、`replace_all` 反向引用（`$2 $1`）、
Unicode 类（`\p{Greek}` → `αβγ`）、`RegexSet` 多模式命中（`[1, 2]`）、
惰性量词 + 多行标志（`(?m)^a.*?c$` 命中 2 次）。**全对**，且与 LLVM 逐行相同。

（这一段的 `regex_set` 期望值第一版我写成 `[2]`，实际 `foo` 同时命中 `^[a-z]+$` 与 `fo{2}`
⇒ 自校验段又当场纠正了我，而不是后端出错。）

**proc-macro 线（⑭）——分水岭能力**：`serde = { features = ["derive"] }` 意味着编译期要
**先编出一个 proc-macro dylib**（`serde_derive`），再由 rustc 加载执行它来展开 `#[derive]`。
实测这个 dylib 也是 **forge 编出来的**（日志：`Compiling proc-macro2 → unicode-ident →
quote → syn v3.0.7 → serde_derive v1.0.229`），随后：

- 结构体 derive：`{"x":1,"y":-2,"label":"origin"}` 与反序列化往返都对；
- 带字段枚举 derive：`{"Square":3}` 往返对。

这说明后端不仅能编普通代码，还能编 **proc-macro crate（dylib + `proc_macro` 链接面）**，
并让 rustc 正常执行它的产物。

**crypto + CLI 线（⑮）——最大的真实依赖图**：`clap 4.6.6`（`features=["derive"]`）+ `sha2 0.10.9`。
构建日志里能数出整条链都由 forge 编：`clap_lex → anstyle/anstyle-parse/anstyle-query →
windows-sys 0.61.2 → anstyle-wincon → anstream → clap_builder → clap_derive（proc-macro）→
clap`，以及 `generic-array → digest → sha2`。断言：

| 断言 | 结果 |
| --- | --- |
| `Sha256::digest(b"abc")` 十六进制 | `ba7816bf…20015ad`（官方向量） |
| `Cli::try_parse_from(["probe","--count","5","--name","forge"])` | `5 Some("forge")` |
| `try_parse_from(["probe","--bogus"])` 的错误类别 | `UnknownArgument` |

`windows-sys` 是**纯 FFI 声明**（`extern "system"` 那一族），`anstyle-wincon` 会真的调
Win32 控制台 API——这条链能编过并跑对，说明后端的**外部符号引用 + Windows 调用约定**在
真实第三方代码上站得住。

**独立 oracle**：脚本把同一份源码用 **LLVM**（清 `RUSTFLAGS` + `rustc-wrapper`）
再编一遍，**逐行比对完整输出与退出码**。实测两者完全相同——所以"forge 算对了"不是靠
程序自己的断言，而是与 LLVM 的结果一致。

**优化档位矩阵**（`-AllOptLevels`）：`0/1/2/3/s/z` **六档 × {forge, LLVM}** 全部
`all_ok=true` 且输出逐行相同（默认只跑 `0,2` 两档省算力）：

```text
=== opt-level=0 ===  oracle_ok=true
=== opt-level=1 ===  oracle_ok=true
=== opt-level=2 ===  oracle_ok=true
=== opt-level=3 ===  oracle_ok=true
=== opt-level=s ===  oracle_ok=true
=== opt-level=z ===  oracle_ok=true
all_levels_ok=true（0,1,2,3,s,z）
```

这同时把后端的**优化管线**（`forge-opt` 各 pass + legalize + regalloc）在真实 std 代码上跑了一遍。

**溢出检查开启**（`-OverflowChecks on`）：默认配置里是 `-Coverflow-checks=off`（跑得快）；
传 `on` 就变成"**开着安全特性**跑整个 std 语料"。2026-10-19 实测 **`-O0` 与 `-O2` 都
`oracle_ok=true`**（forge 与 LLVM 输出逐行相同），说明 8/16/32/64 位的溢出检查在
真实依赖图（serde/regex/clap/blake3/sha2…）上都成立——后端对窄类型用的是
"扩宽算 + 比较区间"，不依赖 x86 的 CF/OF。

```bash
./run-offline.ps1 -OptLevels 0,2 -OverflowChecks on
```

**计时段落（⑯，`FORGE_BENCH=1` 才打印）**：默认不输出——oracle 是**逐行比对完整输出**，
打时间戳会让每次跑都不同。开 `FORGE_BENCH=1` 后能量化优化档位的效果（2026-10-19 实测，
本机 Windows x64 / `-C opt-level`，单位 ms）：

| 计时项 | forge `-C opt-level=0` | forge `=2` | forge `=3` | LLVM `=2` |
| --- | --- | --- | --- | --- |
| `fib32`（递归/调用） | 12 | 6 | 8 | 7 |
| `sum_sq`（50M 次紧循环） | 342 | 0 | 0 | 0 |
| `sieve2m`（200 万筛法，内存写 + 分支） | 83 | 5 | 5 | 5 |
| `sort200k`（20 万 `sort_unstable`） | 219 | 8 | 5 | 8 |

`forge` 与 `LLVM` 在 O2 上**基本持平**；O0 → O2 的 16–27 倍差距就是"优化管线此前从未启用"的
代价（mem2reg 上来后局部才不再逐个落栈）。**`sum_sq=0ms` 不是"快了 342 倍"**——那是循环
被整段优化掉了（结果仍是定值，`bench_checks` 里对得上），别当真实吞吐看。

**自校验段的价值已体现**：第一版有两项 MISMATCH，查下来是**期望值手算错**
（`dyn_dispatch` 应为 11+70+7=88 而我写了 78；`bit_rot` 的 `0x34567812` 是按十进制字符串
比的），后端算出来的才是对的。

入口链（`mainCRTStartup` → `std::rt::lang_start`）、TLS、`abort()`、alloc 运行时、
线程启动这些此前怀疑的阻塞点，实测都没问题。

### 空间要求（2026-10-19 事故后加）

`-Zbuild-std` 会为**每个 (后端, 优化档)** 各编一遍 std，per-level 目录 350–420 MB。
默认两档约 1.5 GB；`-AllOptLevels`（6 档 × 2 后端）约 5–7 GB。

> **磁盘满是静默错值的经典伪装**：2026-10-19 本机盘可用 0 GB 时，后端 e2e 有两条用例
> 跑出 `exit=0`（应为 342/42），整套门禁还挂了 3 小时——清出 18 GB 后全部恢复正常。
> `run-offline.ps1` 因此开头会做**空间体检**并警告；跑后端测试前请先看磁盘。

### 之前判"跑不通"的真实原因（2026-10-18 纠正）

旧结论写的是"沙箱没有网络凭证"——**错**。实测：

- `-Zbuild-std=std` 要解析的 `std` 构建依赖（`cfg-if`/`hashbrown`/`getopts`/`rustc-demangle`…）
  **本来就在 registry 缓存里**，`compiler_builtins` 等更是 **rust-src 内的路径依赖**；
- 真正失败的是 cargo 的**解包写盘**：`failed to create directory
  ~/.cargo/registry/src/…/getopts-0.2.24: 拒绝访问 (os error 5)`——沙箱只允许写工作区。

## 离线跑法（本机沙箱适用）

**推荐：一条命令**，且**不往本目录写任何生成物**（配置与每档产物都在仓库的 `target/` 下）：

```powershell
powershell -NoProfile -ExecutionPolicy Bypass -File .\run-offline.ps1
```

它做的事（想手工复现时照这个顺序）：

```powershell
# ① 前置：后端 dll + wrapper + rust-src（nightly）
cargo build -p forge-rustc
cargo build --manifest-path tools/forge-rustc-wrapper/Cargo.toml
rustup component add rust-src --toolchain nightly

# ② 把 CARGO_HOME 指到**工作区内**（解包/锁文件都落在可写区），
#    并把真实 registry 的 cache+index 拷过去（离线解析要用；约 250 MB，一次即可）
$env:CARGO_HOME = "<repo>/target/cargo-home"
New-Item -ItemType Directory -Force "$env:CARGO_HOME\registry" | Out-Null
Copy-Item -Recurse ... "$env:CARGO_HOME\registry\cache", "$env:CARGO_HOME\registry\index"
$env:CARGO_NET_OFFLINE = "true"

# ③ 把 cargo 配置写进 <repo>/target/forge-rustc-std-hello/forge-cwd/.cargo/config.toml
#    （内容与 `cargo forge init --std` 写进工程的那份等价：rustflags 组 + rustc-wrapper
#      + build-std=["std","panic_abort"]）——cargo 从 **CWD 及其父目录**读配置，
#    所以下面用 `--manifest-path` 指回本工程，配置留在 target 里。

# ④ 构建并运行（在 forge-cwd 里执行）
cargo +nightly build --manifest-path <本工程>/Cargo.toml `
                     --target-dir <repo>/target/forge-rustc-std-hello/forge-o0
& <repo>/target/forge-rustc-std-hello/forge-o0/debug/forge-rustc-std-hello.exe
```

有网络的环境不需要 ②：直接 `cargo forge init --std && cargo build` 即可（那时配置落在工程里，
属正常用法）。`run-offline.ps1` 额外提供 `-AllOptLevels`（`0/1/2/3/s/z` 六档矩阵）与
`-SkipOracle`（只验 forge，跳过 LLVM 对照）。

**两条踩过的坑**：

- **别用 `RUSTFLAGS` 环境变量代替 config**：实测两条路不等价——`RUSTFLAGS` 会让 host 侧
  `std_detect` 也走 forge 后端，ABI 断言当场炸（`rustc FnAbi expects 0, forge packs 1`）；
  `[build] rustflags` 只作用于目标构件。
- **wrapper 只认 `RUSTC_WRAPPER`**：`CARGO_BUILD_RUSTC_WRAPPER` 不是有效覆盖（cargo 不读它），
  漏了就会用 forge 去编 `core`/`std`，报一堆 SIMD/intrinsic 不支持。

## 前置

1. 后端 dll 与 wrapper：

   ```bash
   cargo build -p forge-rustc
   cargo build --manifest-path tools/forge-rustc-wrapper/Cargo.toml
   ```

2. `rust-src` 组件（`-Zbuild-std` 需要）：`rustup component add rust-src --toolchain nightly`
3. 本工程带 `rust-toolchain.toml`（**浮动 `nightly`**）——本机/CI 通常只装浮动 nightly；
   装的是固定日期通道时把 `channel` 改成那一版。

## 程序覆盖的 std 面（`src/main.rs`）

| 档 | 用到的 std | 观测 |
| --- | --- | --- |
| ① | `println!`（stdout 锁 + 格式化） | `hello from std` |
| ② | `String` + `format!` | `[forge + std] len=11` |
| ③ | `Vec`（增长 + 迭代求和） | `vec_sum=385` |
| ④ | `std::env::args`（`OsString`） | `args=1` |
| ⑤ | `HashMap`（`RandomState` 随机种子） | `map_sum=3` |
| ⑥ | `std::fs`（Windows 文件 API + `io::Error`） | `fs_len=10` |
| ⑦ | `std::thread::spawn` + `join`（线程启动链 + TLS + 闭包跨线程） | `thread_sum=10` |
| ⑧ | 泛型/`dyn`/迭代器/排序/浮点/枚举/位运算/解析/`BTreeMap`/`HashSet`/Drop/递归（20 项） | `all_ok=true` + 与 LLVM 逐行相同 |
| ⑨ | `std::net`（Winsock 初始化 + 回环 TCP 收发） | `ping`↔`pong` 双向对上 |
| ⑩ | `std::process::Command`（子进程 + 管道读 stdout） | `cmd-ok` |
| ⑪ | **第三方 crate 语料（基础批）**：`itoa`/`ryu`/`memchr`/`smallvec`/`once_cell`/`bitflags` | 6 个 crate 都由 forge 从源码编译，各自断言正确 |
| ⑫ | **第三方 crate 语料（进阶批）**：`crc32fast`/`blake3`/`hashbrown`/`base64`/`dashu-int` | `#[target_feature]`+intrinsic、swiss-table、大整数乘除；CRC-32 标准校验值 / BLAKE3 官方向量都对 |
| ⑬ | **正则引擎族**：`regex`(+`regex-automata`/`regex-syntax`/`aho-corasick`) | 捕获组/find_iter/replace_all/Unicode 类/RegexSet/惰性量词全部对 |
| ⑭ | **proc-macro 线**：`serde(derive)` + `serde_json` | 结构体/枚举 derive 的 JSON 往返全对；`serde_derive`/`syn`/`quote`/`proc-macro2` 都由 forge 编成 dylib |
| ⑮ | **crypto + CLI 线**：`sha2` + `clap(derive)` | SHA-256 官方向量对；clap 解析/报错分类对。这条依赖图最大（`clap_derive` + `windows-sys` + `anstream`/`anstyle-wincon`） |
| ⑯ | **`u128` 跨边界**（v27 M6-69）：`u128::from_str` / `Display for u128` / `checked_add` / `u128::from(u64)` | 这些都在 **LLVM 编的 core/std** 里 ⇒ 走 Rust ABI 的 128 位传参/返回；五个自校验全对，且 forge 与 LLVM 输出**逐行相同**。此前 e2e 的 u128 用例全是 forge 自编自调，这条第一次把"与 LLVM 的 128 位约定一致"钉住 |

## 已排除的前置阻塞（v27 已落地）

- **TLS**：`std` 的 `thread_local!` 依赖线程局部静态；后端用动态 TLS
  （`TlsAlloc`/`TlsGetValue` + 首触初始化）实现，e2e `thread_local_roundtrip` 守护。
- **`abort()`**：`std` 的 `abort_internal` 走 `core::intrinsics::abort()`；后端已实现
  （`ExitProcess(3)`），e2e `intrinsic_abort` 守护。
- **alloc 运行时**：链 `std` 时不再注入手写 alloc shim（`std` 自带那一族），避免 LNK2005
  ——见 `WA-02`。

## 还没试的（继续探测的方向）

- panic 运行时：当前 `panic=abort`；`panic=unwind` 由后端**明确拒绝**（`[cap:UNWIND]`，
  缺 `.pdata`/`.xdata` 与 `__rust_eh_personality`）——要做得先补 SEH 展开表；
- 动态链接（`-C prefer-dynamic`）与 `proc_macro`（要 dylib 侧 ABI）；
- `std::net` 的 UDP/IPv6、`std::fs` 的目录遍历与元数据（目前只测了回环 TCP + 单文件读写）；
- 多线程竞争形态（目前只有一个后台线程 + join；没有并发压力测试）；
- 带 `asm!` 的内联汇编 crate（那类会直接撞到 `sym`/`label` 的 fail-closed 边界）；
- 需要 `unwind` 的 crate（`anyhow`/`thiserror` 的 `backtrace` 路径、`catch_unwind`）；
- 目前语料只验"编得过 + 行为对"，**没有**验"编出来的代码够快"（性能是另一条线）。
