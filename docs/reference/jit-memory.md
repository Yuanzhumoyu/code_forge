# JIT 可执行内存（`forge_isa_runtime::mem`）

> 状态：active（2026-10-31）· 实现：`crates/foundation/forge-isa-runtime/src/mem.rs`
> 参考的外部实现（仅作设计资料，未引入依赖）：
> [jamesgober/pager-lang](https://github.com/jamesgober/pager-lang)（Region/Protection/W^X/guard page）
> 与 [tremwil/jit-allocator2](https://github.com/tremwil/jit-allocator2)（RX/RW dual mapping、块分配、填充、icache）。

## 安全模型

- **W^X**：代码写入期间可写不可执行；`seal()` 后只读可执行。
- **Windows dual mapping（默认优先）**：同一物理页映射成两个视图——
  RX（`FILE_MAP_READ | FILE_MAP_EXECUTE`）与 RW（`FILE_MAP_READ | FILE_MAP_WRITE`）。
  写入只走 RW，执行只走 RX，不依赖临时改保护。
- **单映射回退**：`CreateFileMappingW`/`MapViewOfFile` 失败时退回
  `VirtualAlloc(RW) → write → VirtualProtect(RX)`。
- **guard pages**：`ExecutableMemory::with_guard_pages(code)` 在代码区前后各放一页
  `PAGE_NOACCESS` / `PROT_NONE`，越界立即 fault（不静默踩内存）。
- **填充**：未用尾部写 `0xCC`（x86 `int3`），防止意外执行到空白区。
- **icache**：写入后统一 `FlushInstructionCache`（Windows）/`mprotect` 后刷新；
  x86 为 no-op，ARM 必需。

## 块分配器（jit-allocator2 风格）

- 64 KiB dual-mapped 块 + 64 B 子页位图；多个小函数共享同一块，`ExecutableMemory`
  只持有块内偏移，不再每个函数一次 `VirtualAlloc`/`CreateFileMapping`。
- 首次适配找连续空闲子页；释放只清位图，空块保留最多 4 个，超出即 unmap。
- 分配时整个子页区间先填 `0xCC` 再写代码；`len()` 返回子页对齐后的分配长度。
- guard page / `new_writable` 仍走整页独立映射（需要单独保护或改保护）。
- 非 Windows 无 dual mapping，回退整页单映射；块分配器只在 Windows 启用。

## 与参考实现的对应

| 参考点 | 当前状态 |
| --- | --- |
| pager-lang `Region::with_guard_pages` | ✅ `ExecutableMemory::with_guard_pages` |
| pager-lang `Protection`/`protect` | ⚠️ 内部 W^X 切换；未暴露通用 `Protection` API |
| pager-lang 缓存 `page_size` | ✅ `OnceLock` 进程级缓存 |
| jit-allocator2 dual mapping | ✅ Windows 默认优先，失败回退 |
| jit-allocator2 block/pool、bitvector 子页 | ✅ 64 KiB 块 + 64 B 位图 + 首次适配 |
| jit-allocator2 fill pattern | ✅ `0xCC` 填充子页；可配置未做 |
| jit-allocator2 `reset`/`shrink`/`query` | ❌ 不做：当前无 `JitAllocator` 句柄 API |

## 测试

- `cargo test -p forge-isa-runtime --lib mem`：子页 64 B、同块共享、释放复用、
  真实执行、guard page、dual mapping。
- `cargo test -p forge-codegen --lib --all-features test_jit_compile_and_call_constant`：
  JIT 路径经块分配器执行。

## 范围边界

`query/shrink/reset` 需要把块分配器暴露成带句柄的 `JitAllocator` API；当前
`ExecutableMemory` 是 RAII 句柄，未提供这些操作。它不是遗留 TODO，而是当前 API
形态的取舍。
