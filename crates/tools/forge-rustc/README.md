# forge-rustc

将 `code-forge` 作为 rustc codegen backend（`-Zcodegen-backend`）使用，
通过标准 rust 工具链把 Rust 代码编译为可执行文件。

## 用法

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
`checked_mul`/`abs_diff` 等同时出现在 libcore 与 forge_codegen_output.o）。
**已知残余**：`GlobalAlloc::realloc` 经 forge 编译后引用 `core::ptr::mut_ptr::is_null`
实例，而 LLVM 侧 libcore（share-generics）未生成该符号 → LNK2019 待补
（需 forge 侧补生成"被引用但上游未生成的实例"）。**同类实证（裸 rustc 场景）**：
`core::slice::iter::Iter::next` 在用户 crate 实例化时引用
`core::num::unchecked_sub::precondition_check`（`v.as_ptr()`/`capacity()` 等
经 `iter().enumerate()` 路径触发），LLVM 侧未生成 → LNK2019 —— 同一机制：
core 泛型实例的辅助函数（is_null/precondition_check）缺失，需统一补生成。

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

### ✅ 已验证（e2e 硬断言，56 用例全部非 known 全绿 + cargo 集成；共 58 项，另有 2 个 known_failure 显式清单：vec_push / vec_string——见下方限制表）

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
| Box 封装层 | `Unique`/`NonNull` 聚合 + `ptr::write` + 解引用读（`box_value`，需 `#[global_allocator]`）；写回 `*b = 42` 的 deref 检查链回归中（见限制区） |
| 链接/增量 | 多 codegen unit 合并、`-C incremental` 增量缓存、MSVC 链接器（`/DEFAULTLIB:vcruntime.lib` 提供 `__CxxFrameHandler3`） |

### ⚠️ 已知限制（记录为待办）

> 绕法编号（[WA-NN]）见 [WORKAROUNDS.md](./WORKAROUNDS.md)（机读清单）。

| 限制 | 原因 |
| ------ | ------ |
| 动态分发（trait object） | **已修复（转正）**：`&Dog → &dyn Speak` 的 unsize cast 完整实现——① vtable 数据段生成（`tcx.vtable_allocation` + `vtable_entries`→VtblEntry 列表→8 字节指针表：drop_in_place/size/align/方法指针；**MSVC 链接器不应用 .rodata 的 ADDR64 重定位（实测全 0），必须放 .data 段**——object_writer 新增 `add_data_with_relocs`）；② self 类型传具体类型（`&Dog → Dog`，否则 `<&Dog as Speak>::speak` 实例解析 ICE）；③ **间接调用返回值修复**：`[lower.CallIndirect]` 缺 `MOV_RM8_R64 rd, RAX`（直接调用 `[lower.Call]` 有）——结果 XReg 被 regalloc 乱分配读到垃圾（曾返回 vtable+24）。新增 e2e 用例 `dyn_trait_call`（exit=7 稳定）；最小用例 3 次运行确认。遗留：trait upcasting（TraitVPtr 槽占位 0） |
| Vec/String 完整运行 | **known_failure（e2e 显式清单：`vec_push` SEGV / `vec_string` len 错）**：十二轮深挖最终定性——**嵌套 niche 传播**（rustc 的 niche 布局传播：外层枚举判别与内层 payload 判别共享/嵌入字节——LLVM 级特性）：grow 链（Result/ControlFlow/TryReserveError 错误传播）与最小复现 cf5（`CF::Break(Err(5u8))` 应=7 现=1）同源；已落库修复：field_offset Primitive 防护（嵌套枚举投影编译 ICE→正确）+ 窄类型宽度（write_bytes_loop 转正）+ 编译层（sret 计数/Ignore 参数）；[WA-11]。**诊断注意：编译非确定性**（HashMap 顺序影响函数布局；reloc 用符号名不受影响）。诊断 env FORGE_TRACE_ABI/CALL/TERM/VCODE/LOWER 保留 |
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
- **单对象文件**：所有实例合并进一个 `.o`（多 CGU 场景由 rustc 接受），`join_codegen` 提供 WorkProduct 元数据支持增量缓存。

## 调试信息（`-C debuginfo=1`，C1 line-tables-only）

**已实现（2026-09）**：`-C debuginfo=1` 生成最小 DWARF 并入对象文件：

- `.debug_line`：行号程序（单 CU 单文件），每函数一个条目
  （`set_address` + `advance_line` + `copy` + `end_sequence`）
- `.debug_info`：单编译单元（`DW_TAG_compile_unit`：producer/
  name/language=Rust）+ 每函数 `DW_TAG_subprogram`（name/low_pc/decl_line）
- `.debug_abbrev`：缩写表（字符串属性用 `DW_FORM_string` 内联）

实现：`src/dwarf.rs`（生成）+ forge-object `add_dwarf`（COFF 调试段）。
行号表来源：`LowerCtxt` 采集每个函数的 `body.span` 起始行 →
`FuncRefTable.line_entries` → backend.rs 写盘前生成。
**已知限制**：low_pc 地址为 0 占位（reloc 待 object crate 的
`DebugSectionReloc` 完善）——`llvm-objdump` 可见段结构与行号条目，
gdb 断点定位需地址 reloc 后可用（后续）。`-C debuginfo=0` 零开销
（不生成段）。e2e `debuginfo_line_tables` 用例守护。

## 路线图（远期，P4.7/P4.8 评估结论）

| 项 | 评估 | 前置依赖 |
| --- | --- | --- |
| **并行 CGU（`-Z codegen-units=N`）** | 中工程量：当前单对象文件（backend.rs 合并输出）。并行化需 FuncRefTable 并发化（`intern`/`intern_global` 加锁或 thread-local）+ 每 CGU 独立 ObjectWriter + `join_codegen` 多模块归并 | 主库 ObjectWriter 并发支持 |
