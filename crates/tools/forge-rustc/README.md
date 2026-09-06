# forge-rustc

将 `code-forge` 作为 rustc codegen backend（`-Zcodegen-backend`）使用，
通过标准 rust 工具链把 Rust 代码编译为可执行文件。

## 用法

### 推荐：cargo forge（`tools/cargo-forge`，cargo 外部子命令）

把 forge-rustc 融进 rust 工具链的最省事入口——`cargo forge <build|run|doctor|backend|init>`
（cargo 外部子命令约定：`cargo forge …` = 执行 `cargo-forge …`）。

安装（任选其一，工具本身只依赖 clap/anyhow）：

```bash
# 仓库内直接构建（产物 tools/cargo-forge/target/debug/cargo-forge.exe，加入 PATH 即可）
cargo build --manifest-path tools/cargo-forge/Cargo.toml
# 或安装到 cargo bin（~/.cargo/bin）
cargo install --path tools/cargo-forge
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

### ✅ 已验证（e2e 硬断言，85 用例全绿——2026-09 WA-29 后 vec_iter_enumerate 转正，known_failure 清零）

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
| Vec/String 完整运行 | **已转正（2026-09，e2e 全绿）**：`vec_push`/`vec_string`/`vec_from_slice`/`string_concat_len` 均 PASS。历史定性为**嵌套 niche 传播**（grow 链 Result/ControlFlow/TryReserveError 错误传播）与最小复现 cf5（`CF::Break(Err(5u8))`）同源；累积修复：field_offset Primitive 防护 + 窄类型宽度（write_bytes_loop）+ 编译层（sret 计数/Ignore 参数）+ **WA-29**（Niche CONSTRUCT 写入宽度按 tag 标量——8 字节指针 tag 恒 movl 残留高 4 字节 → 判别误判，vec_iter_enumerate 由此转正）。[WA-11] 收尾。[WA-29] 已关闭。**诊断注意：编译非确定性**（HashMap 顺序影响函数布局；reloc 用符号名不受影响）。诊断 env FORGE_TRACE_ABI/CALL/TERM/VCODE/LOWER 保留 |
| ~~write_bytes 内联循环（count>1）~~ | **已转正（PASS exit=342）**：两层块参数传参修复（映射覆盖 + pre_allocate 顺序）+ **窄类型宽度修复**（主库 Load/Store 真实内存宽度 `mem_opsize_from_type`——u8 读/写 1 字节不再越界 4 字节、位宽不枚举不截断自定义非常规宽度原样传递；forge-rustc IntToInt cast 对 u8/u16 无符号源零扩展 mask）——`write_bytes_loop` 移除 known_failure 转硬断言，[WA-14] 已关闭。最小复现回归测试 `test_loop_block_param_write_bytes_style`（forge-codegen lib） |
| Assert 失败路径 | **已修复**：assert 失败不再裸 ud2，改走 panic_handler（`rust_begin_unwind`，lang_items().panic_impl() 解析符号；占位 &PanicInfo=0）——失败可观测（panic loop 挂起，与 e2e 超时判挂起对齐）；成功路径不受影响。**注意**：当前传空指针占位，panic_handler 内不得解引用 `info`（e2e 用例的 handler 为 `loop {}`，安全） |
| 浮点比较分支（float_args） | **已修复**：`if a + b > 3.0 { 1 } else { 0 }` 最小用例 f(1.5, 2.0) 实测 exit=1 通过——Fcmp 模板（xor rd,rd; comisd rs1,rs2; set$CC rd）的 rd 类别/Setcc 宽度/分支链均正确，known_failure 为过时标志，已移除（float_args 转 PASS） |
| Box 写回 | **已修复（转正）**：`let mut b = Box::new(7); *b = 42; *b` 曾 0xC000001D——五轮定位：movabs 绝对地址 + .reloc VirtualSize 截断（ASLR 下第 5 个重定位不应用）已**根治**（[lower.GlobalAddr] 重构为 RIP 相对 lea + REL32，@lea_rip_rel）；六轮后（统一聚合框架：槽零初始化 + enum 嵌套投影 field_ty 修复 + copy_agg/pack_sp 统一原语）累积修复了两次 deref 的 [rbp-0x68] 槽未初始化垃圾与 Box 链 enum 解构偏移——**ASLR 与固定基址下均稳定 exit=42（多次运行确认），e2e 中 box_write 用例已实际通过，known_failure 移除** |
| checked 算术 | `AddWithOverflow` 等元组结果已实现（`sadd_overflow`/`uadd_overflow` API + 元组 0/4 偏移拆写）：简单场景（多次 `+=`、if 内）通过；**循环体内 checked 加法已修复（转正）**——曾 0xC000001D（assert 指针槽 32 位截断 0x15fed4，循环块 phi/槽 bug）——统一聚合框架（1.1 sret 判定统一 + field_ty enum 嵌套投影 + 槽零初始化）累积修复——`while i<3 { acc+=i; i+=1 }` 最小用例 ASLR 与固定基址均稳定 exit=3（多次运行确认）；测试与 cargo 工作流仍统一 `-C overflow-checks=off`（README 用法已包含） |

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
- **对象文件形态（M6 Stage B）**：默认按 rustc 真 CGU 分区产出对象——rustc 分区出 >1 个 CGU 时**每 CGU 一个 `.o`**（`forge_codegen_output.<cgu>.o` + 每 CGU 一个 WorkProduct，CGU 级增量；debuginfo per-CGU CU）；单 CGU/`FORGE_SINGLE_OBJECT=1` = 单对象 `forge_codegen_output.o`（Stage A 锚，与旧产物逐字节一致）。详见上文「对象文件形态」节。
- **M3/M4 并行 CGU（Stage A，2026-09）**：`-C codegen-units=N` 只影响 rustc 的 CGU 划分——forge 在 `collect_instances`（按符号名稳定排序）后把**每个函数**作为独立任务（任务私有 `FuncRefTable`：`@N/G{N}` 编号任务内分配、每函数编译完就地 resolve，零加锁）；`merge_task_table` 把任务登记条目（vtables/promoted 按 alloc_id 去重、line/var/enum 按全局函数序）归并回主表；主线程按原实例序串行 emission（单对象单模块，产物与 -C codegen-units 无关）。**并行机制（M4，根治 WA-38）**：rustc 1.99+ 的 tcx 查询只能在 rustc 自建查询池线程上执行（WorkerLocal registry / 作业 ImplicitCtxt / per-thread SessionGlobals，无注册 API）——forge 直接复用 rustc 自家并行原语 `rustc_data_structures::sync::par_map`（rustc_codegen_ssa 在 `-Z threads` 下同款），把函数降级任务作为**嵌套池作业**提交 rustc 查询池。启用 = `FORGE_CODEGEN_THREADS>1` + rustc `-Z threads>=2`（RUSTFLAGS；`--jobs-frontend` 亦可）+ 函数数>1；否则串行（`-Z threads` 缺失时 par_* 自动串行/显式 map——**默认 T=1 与旧路径逐字节一致**）。`FORGE_CODEGEN_THREADS>1` 但无 `-Z threads` 仅 stderr 提示。

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
> 自诊 + M1 abbrev 字节 diff 直接命中发射侧 form 错误（收敛优先）。路线图
> 无剩余开放项。
