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

## 复用（页级 dual cache）

`jit-allocator2` 用块分配器 + 位图管理子页；当前实现先做**页级**简化：

- Windows dual mapping 释放后进入进程级缓存（上限 16 MiB），同尺寸再分配直接复用，
  避免编译-释放循环反复 `CreateFileMapping`/`MapViewOfFile`。
- 缓存只在锁内访问；跨线程移动用 `CachedDual` 的 `unsafe impl Send` 明确承担。
- 超出上限或非 Windows 路径仍立即 unmap；没有子页/多池，也没有 `query`/`shrink`。

## 与参考实现的对应

| 参考点 | 当前状态 |
| --- | --- |
| pager-lang `Region::with_guard_pages` | ✅ `ExecutableMemory::with_guard_pages` |
| pager-lang `Protection`/`protect` | ⚠️ 内部 W^X 切换；未暴露通用 `Protection` API |
| pager-lang 缓存 `page_size` | ✅ `OnceLock` 进程级缓存 |
| jit-allocator2 dual mapping | ✅ Windows 默认优先，失败回退 |
| jit-allocator2 block/pool、bitvector 子页 | ❌ 仅页级缓存；子页分配列为后续 |
| jit-allocator2 fill pattern | ✅ `0xCC` 填充尾部；可配置未做 |
| jit-allocator2 `reset`/`shrink`/`query` | ❌ 未做 |

## 测试

- `cargo test -p forge-isa-runtime --lib mem`：分配/释放、真实执行、guard page、
  dual mapping 强制、页级缓存复用。
- `cargo test -p forge-codegen --lib --all-features test_jit_compile_and_call_constant`：
  JIT 路径经新分配器执行。

后续（若需要）：把页级 cache 升级为 `JitAllocator` 风格的块 + 位图分配器，
补 `query/shrink/reset`，并评估 dual mapping 在非 Windows 的 memfd 实现。
