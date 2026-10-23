# forge-rustc

将 `code-forge` 作为 rustc codegen backend（`-Zcodegen-backend`）使用，
通过标准 rust 工具链把 Rust 代码编译为可执行文件。

## 用法

### 推荐：cargo forge（`crates/tools/cargo-forge`，cargo 外部子命令）

把 forge-rustc 融进 rust 工具链的最省事入口——`cargo forge <build|run|doctor|backend|init>`
（cargo 外部子命令约定：`cargo forge …` = 执行 `cargo-forge …`）。

安装（任选其一，工具本身只依赖 clap/anyhow）：

```bash
# 仓库内直接构建（cargo-forge 是主 workspace 成员；产物 target/debug/cargo-forge.exe，
# 把仓库 target/debug 加入 PATH 即可）
cargo build -p cargo-forge
# 或安装到 cargo bin（~/.cargo/bin）
cargo install --path crates/tools/cargo-forge
```

```bash
# 环境自检矩阵（rustc / 组件 / backend dll / wrapper / host；全 PASS → exit 0）
cargo forge doctor

# 仓库内先构建 backend dll（target/debug/forge_rustc.dll；--release 切 release）
cargo forge backend

# cargo 工程模式（最小 no_std 工程；入口用下方「标准入口模板」：
# no_main + #[unsafe(no_mangle)] extern "C" fn main() -> i32，返回值 = 进程退出码）
cd <你的 no_std 工程>
cargo forge build                                     # cargo +nightly -Zbuild-std=core build
cargo forge run                                       # 构建并运行；-- 后参数透传 exe
cargo forge build --release
cargo forge build --debuginfo 2 --codegen-units 4 --threads 4  # -Cdebuginfo / -Ccodegen-units / -Zthreads
cargo forge run --alloc                               # Vec/String：build-std=core,alloc + -Zshare-generics=yes

# 单文件模式（免 build-std：裸 rustc，core/alloc 用 sysroot rlib）
cargo forge run --file hello.rs
cargo forge build --file hello.rs --debuginfo 2

# init：为当前 cargo 工程写 .cargo/config.toml（rustflags 组 + rustc-wrapper +
# [unstable] build-std）与 rust-toolchain.toml（channel=nightly）——之后
# **裸 `cargo build` / `cargo run`** 即走 forge 后端（不经本工具）
cargo forge init
cargo build && ./target/debug/<包名>.exe

# `cargo test`：**必须加 `-Z panic-abort-tests`**
#   默认的 libtest 会捕获 panic ⇒ 要 unwind 的 std ⇒ 本后端按 [cap:UNWIND] 明确拒绝
#  （`error: the crate panic_abort does not have the panic strategy abort`）。
#   加了 `-Z panic-abort-tests` 后单元测试与 doc-test 都会跑（2026-10-19 实测：
#   2 条 `#[test]` + 0 条 doc-test，全绿）。
cargo test -Z panic-abort-tests
```

顶层全局参数：`--backend-dll <path>`（默认：env `FORGE_RUSTC_DLL` → 仓库
`target/debug/forge_rustc.dll`）、`--toolchain <str>`（默认 `+nightly`，原样作为
rustc/cargo 首参）、`-v/--verbose`（打印组装好的 env 摘要与完整子进程输出）。
工具不在仓库内时：build/run/init 需 `--backend-dll`，backend 需 `--backend-src <repo>`。
`RUSTUP_HOME` 未设且仓库存在 `target/rustup_home` 时自动注入（本仓库构建/验证
环境约定——backend dll 与浮点 nightly 配对，见 README 验证路径）。

> 限制：cargo 模式经 env `RUSTFLAGS` 注入参数，cargo 按空格分词——**dll 路径含
> 空格会解析错误**（init 生成的 `.cargo/config.toml` 用数组元素，无此限制；本
> 仓库场景 `target/debug` 无空格，可接受）。单文件模式直接传 rustc 参数，无此问题。
> cargo 模式 `--alloc`（build-std=core,alloc）仍受下方手工方式 alloc 一段记录的
> 已知残余影响（core 泛型辅助符号 is_null/precondition_check 双份定义/缺失，
> LNK2005/LNK2019）；单文件模式 `--file --alloc` 走 sysroot alloc rlib 无此问题。

### 底层手工方式（方式一/二/三）

```bash
# 构建 backend（需要 nightly + rustc-dev 组件）
cargo build -p forge-rustc

# 方式一：裸 rustc 编译 no_std 程序（mainCRTStartup 入口）
rustc +nightly \
  -Zcodegen-backend=target/debug/forge_rustc.dll \
  -C panic=abort -C overflow-checks=off \
  --edition 2024 \
  -C link-args="/ENTRY:mainCRTStartup /SUBSYSTEM:CONSOLE /DEFAULTLIB:kernel32.lib /DEFAULTLIB:vcruntime.lib" \
  hello.rs -o hello.exe

# 方式二：标准入口（no_main + #[no_mangle] extern "C" fn main() -> i32，
# MSVC 默认入口自动转调，无需 /ENTRY）
rustc +nightly \
  -Zcodegen-backend=target/debug/forge_rustc.dll \
  -C panic=abort -C overflow-checks=off \
  --edition 2024 \
  -C link-args="/SUBSYSTEM:CONSOLE /DEFAULTLIB:kernel32.lib /DEFAULTLIB:vcruntime.lib" \
  hello_main.rs -o hello_main.exe

# 方式三：cargo 工作流（rustc wrapper + build-std，系统 crate 回退 LLVM）
cargo build --manifest-path tools/forge-rustc-wrapper/Cargo.toml   # 先构建 wrapper
cd examples/forge-rustc-hello
RUSTC_WRAPPER=../../tools/forge-rustc-wrapper/target/debug/forge-rustc-wrapper.exe \
  RUSTFLAGS="-Zcodegen-backend=../../target/debug/forge_rustc.dll -C panic=abort \
  -C link-arg=/SUBSYSTEM:CONSOLE -C link-arg=/DEFAULTLIB:kernel32.lib -C link-arg=/DEFAULTLIB:vcruntime.lib" \
  cargo +nightly -Z build-std=core build
./target/debug/forge-rustc-hello.exe
```

使用 `alloc`（Vec/String 等）需 `-Z build-std=core,alloc`，并加 `-Zshare-generics=yes`：

```bash
RUSTFLAGS="... -Zshare-generics=yes ..." cargo +nightly -Z build-std=core,alloc build
```

`-Zshare-generics=yes` 消除上游（core/alloc）泛型实例的双重复定义（LNK2005：
`checked_mul`/`abs_diff` 等同时出现在 libcore 与 forge 产出的对象文件）。
**已知残余**：`GlobalAlloc::realloc` 经 forge 编译后引用 `core::ptr::mut_ptr::is_null`
实例，而 LLVM 侧 libcore（share-generics）未生成该符号 → LNK2019 待补
（需 forge 侧补生成"被引用但上游未生成的实例"）。**同类实证（裸 rustc 场景）**：
`core::slice::iter::Iter::next` 在用户 crate 实例化时引用
`core::num::unchecked_sub::precondition_check`（`v.as_ptr()`/`capacity()` 等
经 `iter().enumerate()` 路径触发），LLVM 侧未生成 → LNK2019 —— 同一机制：
core 泛型实例的辅助函数（is_null/precondition_check）缺失，需统一补生成。

## 对象文件形态（M6 Stage B：每 CGU 独立对象）

forge 消费 rustc 的真 CGU 分区（`collect_and_partition_mono_items`）：

- 多 CGU 时**每 CGU 一个对象文件** `forge_codegen_output.<cgu>.o`（rustc 原生
  多对象形态——rustc 分区出 >1 个 CGU 才触发；rustc 非增量默认会把过小
  CGU 合并，小 crate 常为单 CGU → 单对象 `forge_codegen_output.o`）；
- join_codegen 为每个对象产出独立 WorkProduct（CGU 级增量元数据）；
- 数据（vtable/promoted）由首见 CGU（owner）定义，其余对象 UNDEF 引用；
  实例（Fn/Static）跨 CGU 同名去重——MSVC link.exe/lld-link 多 .obj 链接，
  无 LNK2005；
- `-C debuginfo>=1` 与多对象并存（B-v2）：每个有函数的 CGU 对象带独立
  DWARF CU（per-CGU CU，reloc 目标符号 = 本对象定义）——不再强制单对象；
  `FORGE_SINGLE_OBJECT=1` 仍可强制单对象（`-C codegen-units=1` 为单对象
  回归锚，单 CU dwarf 形态与 Stage A 逐字节一致）；`-C incremental` 增量
  会话下多 CGU 编译可用（二次/含变更的编译不 ICE，CGU 级 WorkProduct 落盘）。

入口程序模板（`mainCRTStartup` 的返回值作为退出码）：

```rust
#![no_std]
#![no_main]

#[unsafe(no_mangle)]
pub extern "C" fn mainCRTStartup() -> i32 {
    // ... 程序逻辑 ...
    0
}

#[panic_handler]
fn panic(_info: &core::panic::PanicInfo) -> ! {
    loop {}
}
```

标准入口模板（推荐，无需 `/ENTRY`）：

```rust
#![no_std]
#![no_main]

#[unsafe(no_mangle)]
pub extern "C" fn main() -> i32 {
    // ... 程序逻辑（返回值 = 进程退出码）...
    0
}

#[panic_handler]
fn panic(_info: &core::panic::PanicInfo) -> ! {
    loop {}
}
```

## 测试

```bash
# 端到端真值测试（编译→链接→执行→校验退出码）
cargo test -p forge-rustc --test e2e

# PowerShell 集成测试（简化版，11 个标量用例子集；完整覆盖见 e2e）
powershell -ExecutionPolicy Bypass -File tests/rustc_integration_test.ps1
```

## 支持矩阵（x86_64-pc-windows-msvc 宿主）

### ✅ 已验证（e2e 197 用例：**197 硬断言 + 0 个 `FLAKY` 容忍项**）

> **2026-10-19 核（v27 M6 实测）**：`tests/e2e.rs` 的 `CASES` 现为 **197 个 `Case`**、
> **`known_failure: true` = 0、`FLAKY` 名单为空** ⇒ 套件是**真全过**，不是容忍式全绿。
> 早先的「103 用例 + 5 个 `FLAKY`」（2026-09-06 回归）里的那 5 例
> （`vec_push`/`vec_string`/`vec_from_slice`/`vec_iter_enumerate`/`box_value`）已随
> **WA-42**（niche tag 写入宽度）在 2026-09-12 全部转正（见 WORKAROUNDS [WA-11]/[WA-42]）。
> v27 新增用例：`overflow_runtime_trap`（trap 语义）、`intrinsic_abort`（`abort()` →
> `ExitProcess(3)`）、`global_asm_call` / `global_asm_sym_const`（`global_asm!`）、
> `inline_asm_nop` / `inline_asm_ud2`（M2.5/S1 内联 `asm!`）、`inline_asm_reg_inout`
> （S2a 寄存器约束）、`inline_asm_const`（S3 `const` 占位符）、`inline_asm_named_reg`
> （S2b 显式寄存器名）、`inline_asm_clobber_abi`（S4 `clobber_abi`）、
> `inline_asm_noreturn`（S4 `options(noreturn)`）、`inline_asm_sym` / `inline_asm_sym_call`
> （S3 `sym`：函数/静态地址物化到寄存器，可作 `mov`/`call` 目标）、
> `inline_asm_label` / `inline_asm_label_cond` / `inline_asm_label_cond_skip`
> （S4 `label`：目标基本块 → 外部标签表 → 相对位移，命中/不命中两个方向都钉住）；
> 负向 `neg_inline_asm` / `neg_panic_unwind` / `neg_thread_local` 钉住缺口 ID 的 fail-closed，
> `inline_asm_sym_literal` 钉住「`sym` 只保证寄存器形态；`[rip + {0}]` 这类字面符号写法
> 编译期明确拒绝」，
> `neg_link_section` 钉住「`#[link_section]` 静默忽略 = 写了不生效」的明确拒绝；
> `static_align_64` 钉住「`#[repr(align(64))]` 的符号地址真 64 对齐」，
> `neg_u128` 钉住「`i128`/`u128` 此前被静默当 64 位算 ⇒ 现在 `[cap:WIDE_INT]` 拒绝」，
> `narrow_shift_semantics` / `narrow_arith_signed` 钉住窄宽度移位与算术的**符号性**，
> `cast_int_float_matrix` 钉住整数↔浮点转换（含 `f64 → i8/u8`）；
> `float_cmp_semantics` 钉住浮点比较语义（NaN：`==`/`<` 全 false、`!=` 为 true）、
> `bit_intrinsics_narrow` 钉住窄类型位运算内建按位宽、`ptr_arith_size12` 钉住非 2 幂元素大小的指针算术、
> `range_contains` 钉住 `Range::contains`（枚举判别宽度按变体数 + `tag_field` 偏移）、
> `char_from_u32` 钉住 `char::from_u32` 的合法性判定（niche 值按真实宽度读），
> `local_align16` 钉住栈槽按类型 ABI 对齐、`u128_trunc` 钉住 128 位截断（u64/u32/u8）、`u128_cmp` 钉住 128 位比较、
> `zst_field_struct`/`iter_map` 钉住 pair 半区越界、
> `u128_byval`/`u128_addsub`/`u128_bitops`/`u128_shift`/`u128_shift_var`（128 位全运算面）
> 、`range_fold_sum`/`range_sum`/`filter_product`（`Variants::Single` 判别）
> 、`struct12_byval`/`range_inclusive_count`
> 钉住聚合传递阈值（>8 字节按引用）、八条 CFG 形态用例（短路 `&&`/`||`、
> else-if 链、元组/Option `match`、`while`+`continue`、
> early return、match guard；期望值 LLVM 对照）钉住分支 lowering，
> `overflow_panic_shl` / `overflow_panic_div` 钉住 **溢出检查真的会 panic**（exit=101，`black_box` 防常量折叠）；期望值均由 LLVM 对照；
> v27 M6 新增负向三条 `flag_lto` / `flag_coverage` / `flag_sanitizer`：靠 LLVM 管线的
> 代码生成选项（`-C lto`、`-C instrument-coverage`、`-Zsanitizer`、PGO、CFG 防护、
> `prefer-dynamic`、非默认 `relocation-model`）**此前全部静默接受**，现在统一按
> `[cap:CODEGEN_OPTS]` 明确拒绝（静默忽略最坏等于"用户以为开了 ASan 而实际没开"）；
> 负向 `neg_sysv64` 钉住**非 Win64 调用约定**（`extern "sysv64"` ⇒ `[cap:CALL_CONV]`）——
> 此前一律当 C 打包（RCX/RDX/R8/R9），而 sysv64 是 RDI/RSI/RDX…；调用方与被调方都由
> 本后端编译时"自洽"看不出来，跨到 LLVM 编的代码或 `extern` 声明的外部函数就是错值；
> `narrow_switch_compare` 钉住 **`switchInt` 的 case 常量类型**（u8 判别值 + i32 常量曾是
> 非法 IR，`-O` 下 verifier 拒绝；rustc 会把 `b[0] == 7` 优化成 `switchInt`）；
> **`opt_const_index` 是第一条用 `-C opt-level=2` 编译的用例**（`opt_*` 前缀由 harness
> 注入该 flag）：优化过的 MIR 把 `a[1]` 写成 `ConstantIndex`（未优化是 `Index(_const)`），
> 而 `ConstantIndex` 曾被静默忽略（读到 `a[0]`）——正是"外部调用写完缓冲后 `buf[1]`
> 读到旧值"的真因。顺带修掉：`Call` 的 verifier 规则错误地要求"callee 在操作数 0"
> （实际在 `Immediate::Func` 立即数通道）⇒ 零参数调用在优化档下校验失败；
> `flag_emit_asm` 钉住 **`--emit=asm` 必须明确拒绝**（`[cap:EMIT_KINDS]`）——此前是
> "成功但没产物"（退出码 0、用户点名的 `.s` 根本不出现）；本后端只产机器码对象
> （`--emit=obj/link/metadata/mir` 正常），汇编请对产物反编译，不由后端伪造文本；
> 删除了被正当推翻的 `neg_global_asm`。
>
> <!-- 计数（机读，守卫 rustc_doc_counts_guard 钉住）：cases=208 hard=208 flaky=0 known=0 expect_trap=9 -->
>
> **上面这段机读标记与 `tests/e2e.rs` 双向相等**：改用例数/容忍名单时必须同步它，
> 否则 `cargo forge gate --guards` 会红（守卫就是为了治「文档与代码分叉」这件事）。

| 类别 | 内容 |
| ------ | ------ |
| 标量算术 | 整数加减乘除取余、移位、位运算、比较（有符号/无符号按类型分派） |
| 控制流 | if/else、while、loop+break、match（SwitchInt 多分支含 otherwise）、早期返回 |
| 函数调用 | 直接调用、递归、跨 crate 泛型单态化（如 `core` 的 `wrapping_add`）、`i64`/`f64` 返回值 |
| 调用约定 | Windows x64：参数按位置分类（前 4 位置寄存器/第 5+ 压栈，栈参数地址虚拟 XReg 参与分配）、浮点 XMM0-3（FPR→XMM 用 movsd，0F 6E 的 rm 是 GPR 命名空间会读到 GPR 垃圾）、32 字节 shadow space、sret（经 sret 指针参数）、**返回值 RAX 传递方向正确** |
| FFI | `extern "C"` 外部符号 → UNDEF 重定位 + 系统库链接（验证：`ExitProcess`）；diverging call（`-> !`）后发 unreachable 终结符 |
| 聚合类型 | 结构体/元组构造与字段读写、嵌套结构体、可变字段、引用（`&`/`&mut`/解引用）、数组多索引 |
| 浮点/混合参数 | 纯浮点参数、整数+浮点混合参数；浮点常量经 `Fconst` 加载到 FPR（iconst 走 GPR）、比较按 `is_fp` 分派 `fcmp`、`Fload`/`Fstore` 按类型分派（MOVSD_RM/MR）——f64 值全程 FPR，不再走 GPR 位模式 |
| 内存模型 | 局部变量栈槽（layout 驱动大小）、`stack_addr`+`load/store`、帧布局（spill+locals） |
| 枚举 | Direct tag 与 niche 枚举的构造、判别读取、match（`enum_basic`/`enum_payload`/`option_*`/`if_let`） |
| alloc 运行时 | `__rust_alloc`/`dealloc`/`realloc`/`alloc_zeroed`（VirtualAlloc）+ 裸指针写读；CRT mem shim（`memcpy`/`memset`/`memmove`/`memcmp`/`strlen` 由后端注入） |
| 标准入口 | `no_main` + `#[no_mangle] extern "C" fn main() -> i32`（MSVC 默认入口自动转调，无需 `/ENTRY`） |
| cargo 工作流 | `tools/forge-rustc-wrapper`（系统 crate 回退 LLVM）+ `cargo +nightly -Z build-std=core build` + 模板工程 `examples/forge-rustc-hello`（RUSTFLAGS 需 `-C overflow-checks=off`，见限制区） |
| 闭包 | 捕获、`FnOnce`/`FnMut` 调用（`closure_capture`/`closure_fnmut`） |
| Drop | `DropInPlace` shim 编译 + 显式/作用域末 drop 调用（`drop_glue`） |
| static 数据段 | `MonoItem::Static` → `.data`/`.rodata` 符号 + `const{allocN}` 指针 → `global_addr`；只读（`static_read`/`static_two_fields`）与 `static mut` 写回（`static_mut_counter`） |
| Box 封装层 | `Unique`/`NonNull` 聚合 + `ptr::write` + 解引用读/写回（`box_value`/`box_write`，需 `#[global_allocator]`） |
| 链接/增量 | 多 codegen unit 合并、`-C incremental` 增量缓存、MSVC 链接器（`/DEFAULTLIB:vcruntime.lib` 提供 `__CxxFrameHandler3`） |

### ⚠️ 已知限制（记录为待办）

> 绕法编号（[WA-NN]）见 [WORKAROUNDS.md](./WORKAROUNDS.md)（机读清单）。

| 限制 | 原因 |
| ------ | ------ |
| 动态分发（trait object） | **已修复（转正）**：`&Dog → &dyn Speak` 的 unsize cast 完整实现——① vtable 数据段生成（`tcx.vtable_allocation` + `vtable_entries`→VtblEntry 列表→8 字节指针表：drop_in_place/size/align/方法指针；**MSVC 链接器不应用 .rodata 的 ADDR64 重定位（实测全 0），必须放 .data 段**——object_writer 新增 `add_data_with_relocs`）；② self 类型传具体类型（`&Dog → Dog`，否则 `<&Dog as Speak>::speak` 实例解析 ICE）；③ **间接调用返回值修复**：`[lower.CallIndirect]` 缺 `MOV_RM8_R64 rd, RAX`（直接调用 `[lower.Call]` 有）——结果 XReg 被 regalloc 乱分配读到垃圾（曾返回 vtable+24）。新增 e2e 用例 `dyn_trait_call`（exit=7 稳定）；最小用例 3 次运行确认。遗留：trait upcasting（TraitVPtr 槽占位 0） |
| Vec/String 完整运行 | **⚠️ 2026-09-06 回归（重标 `known_failure` + 列入 `FLAKY`）**：曾于 2026-09 全部转正（下述历史修复链仍然有效），回归后 `vec_push`/`vec_string`/`vec_from_slice`/`vec_iter_enumerate`/`box_value` 一并进入容忍名单（见上节），根因指向 WORKAROUNDS [WA-40]（regalloc def-spill × 写死物理寄存器）。历史修复链：`vec_push`/`vec_string`/`vec_from_slice`/`string_concat_len` 均 PASS。历史定性为**嵌套 niche 传播**（grow 链 Result/ControlFlow/TryReserveError 错误传播）与最小复现 cf5（`CF::Break(Err(5u8))`）同源；累积修复：field_offset Primitive 防护 + 窄类型宽度（write_bytes_loop）+ 编译层（sret 计数/Ignore 参数）+ **WA-29**（Niche CONSTRUCT 写入宽度按 tag 标量——8 字节指针 tag 恒 movl 残留高 4 字节 → 判别误判，vec_iter_enumerate 由此转正）。[WA-11] 收尾。[WA-29] 已关闭。**诊断注意：编译非确定性**（HashMap 顺序影响函数布局；reloc 用符号名不受影响）。诊断 env FORGE_TRACE_ABI/CALL/TERM/VCODE/LOWER 保留 |
| ~~write_bytes 内联循环（count>1）~~ | **已转正（PASS exit=342）**：两层块参数传参修复（映射覆盖 + pre_allocate 顺序）+ **窄类型宽度修复**（主库 Load/Store 真实内存宽度 `mem_opsize_from_type`——u8 读/写 1 字节不再越界 4 字节、位宽不枚举不截断自定义非常规宽度原样传递；forge-rustc IntToInt cast 对 u8/u16 无符号源零扩展 mask）——`write_bytes_loop` 移除 known_failure 转硬断言，[WA-14] 已关闭。最小复现回归测试 `test_loop_block_param_write_bytes_style`（forge-codegen lib） |
| Assert 失败路径 | **已修复**：assert 失败不再裸 ud2，改走 panic_handler（`rust_begin_unwind`，lang_items().panic_impl() 解析符号；占位 &PanicInfo=0）——失败可观测（panic loop 挂起，与 e2e 超时判挂起对齐）；成功路径不受影响。**注意**：当前传空指针占位，panic_handler 内不得解引用 `info`（e2e 用例的 handler 为 `loop {}`，安全） |
| 浮点比较分支（float_args） | **已修复**：`if a + b > 3.0 { 1 } else { 0 }` 最小用例 f(1.5, 2.0) 实测 exit=1 通过——Fcmp 模板（xor rd,rd; comisd rs1,rs2; set$CC rd）的 rd 类别/Setcc 宽度/分支链均正确，known_failure 为过时标志，已移除（float_args 转 PASS） |
| Box 写回 | **已修复（转正）**：`let mut b = Box::new(7); *b = 42; *b` 曾 0xC000001D——五轮定位：movabs 绝对地址 + .reloc VirtualSize 截断（ASLR 下第 5 个重定位不应用）已**根治**（[lower.GlobalAddr] 重构为 RIP 相对 lea + REL32，@lea_rip_rel）；六轮后（统一聚合框架：槽零初始化 + enum 嵌套投影 field_ty 修复 + copy_agg/pack_sp 统一原语）累积修复了两次 deref 的 [rbp-0x68] 槽未初始化垃圾与 Box 链 enum 解构偏移——**ASLR 与固定基址下均稳定 exit=42（多次运行确认），e2e 中 box_write 用例已实际通过，known_failure 移除** |
| checked 算术 | **已可用（含 8/16 位窄类型）**：`AddWithOverflow` 等元组结果的拆写在任意宽度都走通了。8/16 位的加/减/乘**不再依赖 x86 的 CF/OF**——`lower_checked_op` 改成"扩宽算 + 比较区间 + 截回"（x86 的 8 位 reg-reg 算术是另一个 opcode，落到 16/32 位编码上会让 `setb/seto` 读到加宽后的进位）。e2e 由 `oc_*` 用例钉住（harness 注入 `-C overflow-checks=on`）：`200u8+100u8`、`-100i8-100i8`、`200u8*2u8`、`1000u16*1000u16` 都必须 trap，`200u16*100u16` 必须正常返回 20000。**残留**：直接对 IR `uadd_overflow(I8)` 喂值仍拿不到正确标志（JIT 夹具 `jit_uadd_overflow_flag_is_correct` 钉住，等 reg-reg form 支持按 opsize 换 opcode） |

### ❌ 明确不支持（编译报错而非静默错误）

- 带 `std` 的程序（运行时符号未提供，编译失败）
- async/await、生成器
- 非 x86_64 目标的端到端验证（aarch64/riscv64 backend 已注册，指令覆盖未验证）

## 架构

```text
rustc (codegen_crate)
  └─ collect_and_partition_mono_items → MonoItem::Fn 实例
      └─ tcx.instance_mir → MIR Body
          └─ LowerCtxt（forge-rustc）MIR → forge-ir Function
              ├─ 栈槽内存模型：全部 local 落栈，load/store 读写
              ├─ 直接调用：FnDef → Instance → 符号名 → FuncRef（@N 重定位）
              └─ 聚合：rustc layout_of 驱动槽大小/字段偏移
                  └─ forge-codegen 编译 → CompiledFunction（机器码+重定位）
                      └─ forge-object ObjectWriter（ELF/COFF/MachO）
                          └─ rustc 链接器（COFF REL32 addend 已补偿 -4）
```

关键设计决策：

- **无 phi**：forge-ir 无 phi 节点，循环回边重定义靠栈槽内存模型（重新 load）保证正确。
- **失败即报错**：任何不支持的 MIR 构造 → `tcx.dcx().err(...)` 使编译失败，绝不产出桩函数。
- **`overflow-checks=off`**：常规 `+`/`-`/`*` 的溢出 Assert 分支未覆盖（checked 元组结果本身已实现——见支持矩阵），测试统一关闭溢出检查。
- **对象文件形态（M6 Stage B）**：默认按 rustc 真 CGU 分区产出对象——rustc 分区出 >1 个 CGU
  时**每 CGU 一个 `.o`**（`forge_codegen_output.<cgu>.o` + 每 CGU 一个 WorkProduct，CGU 级增量；
  debuginfo per-CGU CU）；单 CGU/`FORGE_SINGLE_OBJECT=1` = 单对象 `forge_codegen_output.o`
  （Stage A 锚，与旧产物逐字节一致）。详见上文「对象文件形态」节。
- **M3/M4 并行 CGU（Stage A，2026-09）**：`-C codegen-units=N` 只影响 rustc 的 CGU 划分——forge
  在 `collect_instances`（按符号名稳定排序）后把**每个函数**作为独立任务（任务私有
  `FuncRefTable`：`@N/G{N}` 编号任务内分配、每函数编译完就地 resolve，零加锁）；
  `merge_task_table` 把任务登记条目（vtables/promoted 按 alloc_id 去重、line/var/enum 按全局
  函数序）归并回主表；主线程按原实例序串行 emission（单对象单模块，产物与 `-C codegen-units`
  无关）。**并行机制（M4，根治 WA-38）**：rustc 1.99+ 的 tcx 查询只能在 rustc 自建查询池线程
  上执行（WorkerLocal registry / 作业 ImplicitCtxt / per-thread SessionGlobals，无注册 API）——
  forge 直接复用 rustc 自家并行原语 `rustc_data_structures::sync::par_map`（rustc_codegen_ssa
  在 `-Z threads` 下同款），把函数降级任务作为**嵌套池作业**提交 rustc 查询池。启用 =
  `FORGE_CODEGEN_THREADS>1` + rustc `-Z threads>=2`（RUSTFLAGS；`--jobs-frontend` 亦可）+
  函数数>1；否则串行（`-Z threads` 缺失时 par_* 自动串行/显式 map——**默认 T=1 与旧路径
  逐字节一致**）。`FORGE_CODEGEN_THREADS>1` 但无 `-Z threads` 仅 stderr 提示。

## 调试信息（`-C debuginfo=1`，C1 line-tables-only）

**已实现（2026-09）**：`-C debuginfo=1` 生成 DWARF **v5** 并入对象文件：

- `.debug_line`：行号程序（v5 头：opcode_base=13 + standard_opcode_lengths/
  line_base=-5；文件表 [DW_LNCT_path→DW_FORM_line_strp] **两条同串条目**
  （0/1 基读者 hack），路径存 `.debug_line_str`）。**每函数一个 sequence**
  （gcc/clang 布局——每行一个 sequence 会让行零跨度，gdb 丢弃零长度行）：
  函数级条目 + **per-statement 条目**（B1：主库 emission 逐指令收集
  (机器码偏移, 行)，逐行 set_address reloc + advance_line 累加）；序列
  末行 copy 后发 **DW_LNS_advance_pc 到函数真实末地址**（M2：终端行不再
  零跨度——否则末行 [X, X) 被 gdb 丢弃，序列终端行显示 line 0）
- `.debug_info`：单编译单元（v5 头 unit_type/address_size；
  producer/name=**源文件路径**/language=Rust/low_pc+high_pc（代码范围，
  reloc 首函数 + data8 总长）/stmt_list）+ 每函数 `DW_TAG_subprogram`
  （name/low_pc/**high_pc=函数代码字节**/decl_line）
- `.debug_aranges`：单 CU 地址范围（gdb 16 cooked index 的 pc→CU 映射）
- `.debug_abbrev`：缩写表（字符串属性用 `DW_FORM_string` 内联）
- `.debug_frame`（**M2 CFI，2026-09**）：x86_64 每函数一条 FDE（CIE +
  initial_location ADDR64 reloc→函数符号）。gdb 16.2 amd64-windows 无
  SEH(.pdata) 时落 dwarf2-frame 解 .debug_frame 建栈帧——**此前无 CFI 是
  bt / info args / info locals 全空的根因**（实证）；CFA=rbp+16 定帧 +
  7 callee-saved 保存槽（forge-codegen 新 `machine/cfi.rs` 对发射机器码
  前缀扫描——emission 经 `TargetMachine::function_cfi` 按 ISA 名派发，不
  直接引用 ISA 专属代码；自校验安全退化）

**版本必须是 v5**：w64devkit gdb 16.2 对 PE 只支持 DWARF5——gcc 同源代码
`-gdwarf-4` 编译后 gdb 同样全失效（断点不解析/无行号，对照实证）。
实现：`src/dwarf.rs`（生成）+ forge-object `add_dwarf`（COFF 调试段）。
行号表来源：`LowerCtxt` 每语句 `set_current_loc`（stmt span → 行）→
forge-ir `Instruction.loc` → 主库 vcode `inst_lines` → emission
`CompiledFunction.line_entries` → `FuncRefTable` → backend.rs 写盘前生成。
地址 reloc（set_address/low_pc/aranges）ADDR64 发射，链接器解析为函数
真实地址。`-C debuginfo=0` 零开销。e2e `debuginfo_line_tables` 用例守护。

**C2 变量级 debuginfo（-C debuginfo=2 full，2026-09 已实现）**：
lower 从 rustc `body.var_debug_info` 采集源变量（无投影 local——forge 全
栈槽模型，fbreg = 实际槽相对 rbp 的偏移，含主库 callee-saved shift 64）
→ `VarEntry`/`FnVarEntries` 结构体 → dwarf.rs 生成：

- `DW_TAG_subprogram` 加 `DW_AT_frame_base`（DW_OP_reg6 = rbp）+
  `DW_AT_high_pc`（函数代码字节——**gdb 需函数结束地址建 function
  block，缺则变量 DIE 全部被丢**，对照 gcc 实证）
- 参数 `DW_TAG_formal_parameter` / 局部 `DW_TAG_variable`（name/type/
  location=DW_OP_fbreg/decl_line）
- `DW_TAG_base_type`（标量：name/byte_size/encoding——i8..i128/u8..u128/
  f32/f64/bool/char/usize/isize）+ `DW_TAG_pointer_type`（&T/*const T，
  指向内层标量 base_type；聚合/嵌套指针 pointee 未建模 → DW_AT_type 引用
  兜底 `DW_TAG_unspecified_type`——**绝不写 ref 0**（0 指 CU 头非 DIE，
  gdb 报 "Cannot find DIE at 0x0" 拒整个 CU，M10 实证））
- **`DW_TAG_structure_type` + `DW_TAG_member`（聚合类型，2026-09）**：
  lower 对命名 struct 变量采集成员清单（layout `fields().offset(i)` 实测
  字节偏移——含 repr/对齐重排，与槽内布局一致）→ structure_type DIE
  （name/byte_size）+ member 子项（name/type ref4/data_member_location）。
  类型区顺序 base → pointer → structure（成员反指前两类），变量/成员
  type ref4 占位统一回填（未解析引用 → unspecified_type 兜底）。V1：仅非
  enum/union 的命名 struct；成员聚合
  类型（嵌套 struct/enum 字段）= 未解析 → unspecified_type（递归结构待续）。rustc 2026
  field.ty Debug 形态带 "Unnormalized { value: X, .. }" 包装——layout.rs
  `normalize_ty_debug` 剥壳与变量侧 ty_desc 对齐（WA-34）。
- **`DW_TAG_enumeration_type` + `DW_TAG_enumerator`（C-like 枚举，2026-09）**：
  unit 变体枚举（无 payload）→ enumeration_type（name/byte_size）+
  enumerator 子项（name/const_value data8，判别值取 rustc
  `adt.discriminants`）。枚举变量 type ref 同样占位回填。带 payload 变体
  （discriminant+payload 布局）V1 不设类型。

**验证路径（2026-09）**：objdump `--dwarf=info/decodedline/rawline/frames`
全解析干净（类型/变量/fbreg/行号正确；FDE 行流 = CIE(RA 16) + CFA=rbp+16 +
7 callee-saved 保存槽，gcc mingw 字节形态对照）；**w64devkit gdb 16.2
实证（M2，.debug_frame 落地后）**：函数名断点设点/命中 ✓；**`bt 3` =
helper ← mainCRTStartup 双帧正确**（返回地址经 CFA-8 读取——此前无 CFI
时 bt/info args 全空）；`info args`/`info locals` 列出参数/局部（名/位置
正确，`set language c` 后 `p (int)y` 转型可读值）；源码断点/`list`/`next`
单步 ✓；序列终端行解码到函数末地址（decodedline 末行 end = fn end，不再
line 0）。**注意**：①MSVC link.exe 截断 COFF 段名
（`.debug_l`）gdb 读不到——验证用 `-C linker=lld-link`（保留完整段名）
或 GNU ld（另含 COFF 符号表，但其 `__end__`/`___tls_*` 伪符号压到
.text 起点 0x1000 与首函数冲突——GNU ld 会话里首函数入口断点命中后
帧名显示 `__end__`，其余函数正常；lld-link 无符号表 → 名称断点走
cooked index 可设但命中帧同受 0x1000 冲突影响）；②~~gdb-PE 残余~~
**gdb-PE 类型打印（M10 已根治，2026-09）**：`p y` 曾显示 "< unknown type >"
（`ptype i32`/`info types` 正常、typedef 已注册、`info scope` 位置正确、
`set language c` 后 `p (int)y` = 42）——根因 = abbrev 表 **DW_AT_type(0x49)
的 form 字节误写 0x06（DW_FORM_data4 常量类）而非 0x13（DW_FORM_ref4 引用
类）**（code 4/5/7/9 四处）：gdb `die_type` 拒收非引用 form → "DWARF Error:
Bad type attribute"（`set complaints 10` 首屏实证）→ 变量类型落 error type；
`typedef i32` 仍注册故 ptype i32 正常——与 CFI/行表/gdb-PE 边界无关，是
发射侧单字节错误（objdump 宽容解码自洽造成"发射正确"假象）。修复后 gdb
16.2 实证：**`p y` = 42、`ptype y` = i32、`info scope` length=4、
complaints 清零**（详见 WORKAROUNDS WA-33 M10 增补）；③类型/行号语义以
objdump 解码 + dwarf 结构单测（12 个：逐字节解析 info/line/aranges/frames +
abbrev 表单回归——M10 新增 abbrev_type_attr_uses_ref4_form /
unresolved_type_refs_fall_back_to_unspecified）为准。e2e `debuginfo_full` + dwarf 单测守护。
**待续**：结构成员类型递归/枚举变体（DW_TAG_enumeration_type/variant）；
嵌套聚合字段类型。

## 路线图（远期，2026-09 调研修订）

> **已移除：并行 CGU（`-C codegen-units=N`）目标** —— 2026-09 M3→M6/B-v2 全链路
> 已完成并关闭（提交 `b43ce8d`…`42bf37e`；task 化并行（M4 par_map 上 rustc 查询池）、
> 每 CGU 独立对象 + 多 WorkProduct（M6 Stage B）、debuginfo per-CGU CU（B-v2）、
> `-C incremental` 多 CGU 端到端；验证见 e2e/determinism 用例与 WORKAROUNDS
> WA-38/WA-39，形态说明见上文「对象文件形态（M6 Stage B）」节）。
> **已关闭：gdb-PE 类型打印（M10 对照矩阵，2026-09）** —— 最后开放项命中
> 并根治（提交见 WORKAROUNDS WA-33 M10 增补）：`p y` "< unknown type >" 的
> 根因是 **abbrev 表 DW_AT_type form 误写 0x06（DW_FORM_data4 常量类）而非
> 0x13（DW_FORM_ref4 引用类）**（dwarf.rs `gen_debug_abbrev` code 4/5/7/9
> 四处）——gdb die_type 拒收非引用 form（`set complaints 10` 报 "Bad type
> attribute"），变量/参数类型全部落 error type；`typedef i32` 仍注册故
> `ptype i32` 正常。objdump 宽容解码自洽，掩盖了该单字节错误。修后 gdb
> 16.2：`p y`=42、`ptype y`=i32、complaints 清零。对照矩阵其余项（M3-M9：
> external/decl_column/prototyped、comp_dir/name 形态、language C11、
> decl_file/文件表、类型 DIE 排位、ref_addr、双 CU）**全无需执行**——M10
> 自诊 + M1 abbrev 字节 diff 直接命中发射侧 form 错误（收敛优先）。
>
> ⚠️ **2026-10-11 起：路线图不再是"无剩余开放项"**——v27 专项（
> [`docs/plans/forge-rustc-v27-plan.md`](../../docs/plans/forge-rustc-v27-plan.md)）
> 重新定义了"什么算完整后端"，并把下面这些缺口**变成带 ID 的台账项**（见下一节）。

## v27 能力与缺口面（2026-10-11）

**新增能力**（v27 M2 落地）：

- **`global_asm!` 全支持**：字面文本（指令/标签/伪指令，交给 LLVM 的 asm parser）、
  `const` 占位符（按常量类型符号性格式化）、`sym` 占位符（复用 crate 内**同一份**
  `symbol_name_for_instance`）。实现走 cranelift 后端同构的"委托汇编器"路线：
  另起 `rustc` 子进程（默认后端 = LLVM，`--emit obj`）汇编成**独立对象**，经
  `CompiledModule::global_asm_object` 交给 rustc 链接。e2e：`global_asm_call`、
  `global_asm_sym_const`（`sym` 名字写错 ⇒ **链接期**报未定义符号）。
- **e2e 表达"必须 trap"**：`Case::expect_trap` + 三态退出结局（正常/异常终止/超时）；
  panic 出口是**确定性 abort**（FFI `ExitProcess(101)`，经 `#[link_name]` 别名）。
  e2e 新用例 `overflow_runtime_trap` 钉住"运行期溢出必须被拦"。
- **`known_failure` 清零**：`known_failure: true` = **0**、`FLAKY` 为空 ⇒ 110 用例**真全过**。

**四个缺口 ID**（进 `docs/reference/backend-gaps.md` 台账，由 `cargo forge gate --guards`
的 `capability_gaps` 双向钉住；源码侧在 `crates/tools/forge-rustc/src`）：

| ID | 决策 | 现状 |
| --- | --- | --- |
| `INLINE_ASM` | 待做 | 内联 `asm!` 明确拒绝（此前是静默降级成 `unreachable` ⇒ 编译无声通过、运行退出 `0xC000001D`）；真实现需 IR 层「原始文本 + 寄存器约束」，分期 S1–S4 |
| `UNWIND` | 不做 | `panic=unwind` 明确拒绝（缺 SEH `.pdata`/`.xdata` + `__rust_eh_personality`） |
| `TLS` | 待做 | `#[thread_local]` 明确拒绝（此前会发射成普通 `.data`、**线程私有语义静默丢失**） |
| `MIR_TERMINATOR` | 不做 | MIR 终结符兜底**明确拒绝**，不再静默 `unreachable` |

**裸函数（`#[unsafe(naked)]`）已支持（2026-10-19）**：函数体里的汇编自带 `ret`，后端
**不发射**序言/尾声、不保存 callee-saved（IR `FunctionAttributes::NO_FRAME`），lowering 也
不给它建任何栈槽（`LowerCtxt::new` 的 `naked` 形参）。`const` 操作数（`k = const 7`）走
"就地替换成字面文本"的既有路径，同样无栈帧（e2e `naked_const_operand` 钉住）。

**语言面澄清**：`naked_asm!` 里的**寄存器操作数**是 rustc 前端自己禁止的
（原文 `the ... operand cannot be used with naked_asm!`，对 LLVM 后端同样报），不是本后端的限制
⇒ 裸函数的操作数形态只有 `const`/`sym`；`sym` 已支持（v27 M6-93：地址物化到输入寄存器，
可作 `mov`/`call` 目标；把 sym 当字面符号的写法在编译期明确拒绝）。

**IR 优化管线已接入（2026-10-19，v27 M6）**：此前 `forge-opt` 在生产路径上**一次都没跑过**
（`CompileSession::with_opt_level` 只有测试在调），所以每个 MIR 局部都留在栈槽、`-C opt-level`
几乎无效。现在按 rustc 的 `-C opt-level` 映射档位（0 = 不优化、1/2/3 = O1/O2/O3、s/z = O2），
`FORGE_OPT_LEVEL=0..3` 可覆盖（对照实验用）。接线时一并修掉了 6 处既有非法 IR 与 2 个
pass 记账 bug（详见方案账本「从未启用 IR 优化管线」一节）。

**门禁**：`cargo forge gate` 现含 `rustc-e2e`（197 用例真执行）、`rustc-e2e-guards`
（trap 语义/零容忍名单/模板契约不变量）、`rustc-std`（真 `std` 语料：forge 编 vs LLVM 编
逐行对拍，`-O0`/`-O2`）、`rustc-determinism`、`rustc-opt-levels`、
`rustc-dwarf` 与 `rustc-clippy`（`-p forge-rustc --all-targets -- -D warnings`）七步
（仅 Windows 口径；跨目标见方案 M4），共 **51 步**；另有 `rustc_doc_counts`
（本 README 的计数标记 ↔ `tests/e2e.rs` 双向相等）与 `workarounds_guard`
（`[WA-NN]` 编号唯一 + 引用可解析）两个守卫。步数用 `cargo forge gate --list` 复核。

**`std` 探测工程**：
[`examples/forge-rustc-std-hello/`](../../examples/forge-rustc-std-hello/README.md)
——`std` 由 LLVM 预编译、wrapper 按 crate 名剥掉系统 crate 的 `-Zcodegen-backend`；
需要网络与 `rust-src`。库内两个前置阻塞已排除：动态 TLS（`thread_local_roundtrip`）
与 `abort()`（`intrinsic_abort`）。

**下一批（方案里编号）**：**M2.5** 内联 `asm!` 真实现（S1–S4）→ **TLS 真实现** →
**M3** `std` 可用性（依赖前两者：std 的 `thread_local!` 与平台胶水层用到它们）→
**M4** 第二目标 + 对象层根治 → **M6** 契约回写与账本归档。
