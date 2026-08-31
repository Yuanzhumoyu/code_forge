# forge-rustc WORKAROUNDS（机读清单）

> 本文件是 rustc 内部 API / MSVC 工具链 / forge-codegen 行为的**绕法登记表**。
> 每个条目：`[WA-NN]` 编号 + 位置 + 问题 + 绕法 + 根治方向（是否需主库修复）。
> 代码注释引用编号（如 `[WA-01]`）便于 grep 定位；新 workaround 必须在此登记。

## 工具链/链接器层

| 编号 | 位置 | 问题 | 绕法 | 根治方向 |
| --- | --- | --- | --- | --- |
| WA-01 | backend.rs `add_data_with_relocs` / func_ref.rs `vtables` | MSVC 链接器不应用 `.rodata` 段的 ADDR64 重定位（实测链接后全 0） | vtable 指针表放 `.data` 段（`add_data_with_relocs`） | 需主库：COFF writer 对 `.rodata` 重定位的兼容或文档化 |
| WA-02 | backend.rs `collect_instances` | alloc crate 的 `__rust_alloc` 等 shim 实例若编译，与手写 shim（alloc_runtime.rs）重复定义 → LNK2005 | `collect_instances` 过滤 alloc_shims 名单 | 无（设计取舍：手写 shim 绕过 Layout/Alignment lowering） |
| WA-03 | backend.rs `MonoItem::GlobalAsm` | global_asm 无实现 | 直接 `dcx().err`（编译失败而非静默） | 主库：GlobalAsm → 对象文件段支持 |
| WA-04 | README（用法） | `-Zshare-generics=yes` 下 core/alloc 泛型实例的辅助函数（`is_null`/`precondition_check`）LLVM 侧未生成 → LNK2019 | 依赖 LLVM 侧生成；文档列为已知残余 | 主库：补生成"被引用但上游未生成的实例" |
| WA-05 | README（用法） | 常规 `+`/`-`/`*` 的溢出 Assert 分支未覆盖（checked 元组结果本身已实现：`sadd_overflow` 等 API + tuple 拆写完整路径） | 测试统一 `-C overflow-checks=off` | 主库：常规算术溢出 Assert 语义（checked 路径已通） |

## 布局/降级层

| 编号 | 位置 | 问题 | 绕法 | 根治方向 |
| --- | --- | --- | --- | --- |
| WA-06 | layout.rs `scalar_pair_offsets` | niche ScalarPair（`Option<i32>` 等）的 `fields()`/`variants` 缺字段（rustc_abi 越界 panic） | 优先用 `BackendRepr::ScalarPair` 标量宽度（第 1 标量@0、第 2 标量紧随） | 无（rustc 内部布局 API 行为，跟随 nightly 适配） |
| WA-07 | backend.rs（链接） | MSVC 链接器需要 `__CxxFrameHandler3` | `-C link-arg=/DEFAULTLIB:vcruntime.lib` | 无（工具链要求） |
| WA-08 | backend.rs `join_codegen` | rustc 增量编译需要 WorkProduct 元数据 | 实现 `join_codegen` 输出 `WorkProductMap` | 已实现（非 workaround，随 rustc trait 演进） |
| WA-09 | lower/mod.rs `build_signature` vs `lower_body` | sret 判定在签名构造与入口收参两处独立实现，不一致会导致收参错位 | 两处共用 `is_agg_mem` 判定（注释互相引用） | 已消除（单一判定函数） |
| WA-10 | lower/mod.rs `mono_symbol` / `new` | 闭包/内部 shim 的 DefId 无 `item_name`（直接调用会 ICE） | 用 `tcx.def_path_str` 安全取名字符串 | 无（rustc API 行为） |
| WA-11 | forge-codegen 主库分支/选择降级 + forge-rustc `lower/` | Vec grow 链运行期值错（e2e `vec_push`/`vec_string` known_failure） | **2026-08 十九轮定性**：`Vec::new` 构造全对（RawVec 新嵌套布局 cap@0/ptr@8/len@16）；`with_capacity+push` 全对；**new+push（触发 grow）错**：cap=0（应 1）、len=5（应 1）；**maxmin 实验证明 `Ord::max/min` 本身降级正确**（0.max(1)=1）——cap=0 根因收敛为 grow_amortized 的 new_cap 计算/实参错（两个 max 调用 + `_17=const 8/4/1` 候选）；vecwc2（with_capacity(2)+2 push）挂起 124（grow 链值错与挂起输入敏感交替） | 主库/本库：grow 链 new_cap 实参逐路径 gdb 定位；[lower.Select] 修复（2026-08 已落库 x86/aarch64——待 regalloc 重叠检查后恢复原生 Select） |
| WA-17 | 主库 `[lower.Select]`（isa TOML）+ regalloc | Select 降级无条件 true 臂（cf5 嵌套 niche 判别曾全错） | **2026-08 已修规则层**：x86 `[lower.Select]` 从 `mov rd, rs2` 改为 `mov rd, rs3; test rs1, rs1; cmovne rd, rs2`（条件选择）；aarch64 同步（SD_CMP+SD_CSEL NE）；riscv64 标记 TODO（无 csel，未验证目标）。**恢复原生 Select（builder.select）验证**：nested_enum_break 仍 SEGV——根因假设（待查）：①regalloc 的 XReg 重叠（mov rd,rs3 先写 rd，若 rd 与 cond 同寄存器则 test 读到被覆盖值）②cond 位宽（icmp 结果 I32 vs test 64 位）——当前保留算术公式规避（rvalue.rs），Select 修复留作框架正确性 | 主库：regalloc 同块 def/use 重叠检查 + cond 位宽；修后恢复 rvalue.rs 原生 Select |
| WA-18 | 主库 AtomicRmw（isa TOML `xadd {g1}, [{0}]`）+ forge-rustc static 落段 | 原子 RMW（fetch_add 等）运行期 SEGV——**2026-08 定性为两因叠加**：①内部可变 static（`static A: AtomicI32`，UnsafeCell）被 `is_mutable_static`（仅语法 mut）判为只读 → 落 `.rodata` → `xaddq` 写只读页 SEGV（di_atom gdb 实证 rip=0x…14e7、r11=&A 在 RVA 0x3000）；②JIT 高压测试（10 存活值 + atomic_rmw）证明主库 regalloc 无 spill 缺失（原假设推翻） | **已关闭**：forge-rustc 侧 `global_data.rs` 落段判定改为 `is_mutable_static \|\| !Ty::is_freeze`（内部可变 → `.data`）；intrinsics.rs 解除 RMW 门控——Xchg/Add/Sub 转正（`atomic_rmw_gated` 负用例删除，新增 `atomic_fetch_add`/`atomic_xchg`/`atomic_fetch_sub` 正用例）；And/Or/Xor/Nand/Max/Min/Umax/Umin/Cmpxchg 仍编译期拒绝（需 CMPXCHG 循环/cmpxchg 指令扩展） | 已关闭 |
| WA-19 | backend.rs `collect_instances`（build-std 全量场景） | `-Z build-std=core,alloc` 把 forge 当全量后端编译用户依赖（smallvec/serde/keccak 等），intrinsic 长尾（已补 ~20 个：bswap/cttz_nonzero/caller_location/compare_bytes/typed_swap_nonoverlapping/copy/fabs 等）+ AtomicRmw 门控阻塞 | 依赖的 intrinsic 已逐步补齐；atomic_compare_exchange 被 WA-18 门控拒绝；闭包 item_name ICE 已修（opt_item_name 兜底） | 主库：WA-18 根治后 atomic 解锁；build-std 全量编译依赖作为远期（非本后端目标场景，e2e 子集为主） |

## 使用约定

| 编号 | 位置 | 约定 |
| --- | --- | --- |
| WA-12 | README 用法 | `no_main` + `#[no_mangle] extern "C" fn main() -> i32` 走 MSVC 默认入口，无需 `/ENTRY` |
| WA-13 | tests/e2e.rs | 诊断输出全部 `FORGE_TRACE_*` env 门控（trace.rs 登记），不污染正常 stderr |
| WA-14 | lower/intrinsics.rs `write_bytes` + 主库 lowering `mem_opsize_from_type` + forge-rustc `rvalue.rs` cast mask | write_bytes 内联循环越界读写（e2e `write_bytes_loop`） | **2026-08 已修并转正**：①块参数传参两层（map_terminator 复用 arg 映射 + pre_allocate 顺序）；②**窄类型宽度**——主库 Load/Store 用真实内存宽度（u8 读/写 1 字节不再越界 4 字节，`mem_opsize_from_type` 不枚举不截断、自定义非常规宽度原样传递）+ forge-rustc IntToInt cast 对 u8/u16 无符号源零扩展 mask——`write_bytes_loop` PASS exit=342（e2e 56/58） | 已关闭 |
