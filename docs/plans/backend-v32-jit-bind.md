# 后端 v32：JIT 调用绑定 API（Bevy/Wasmtime 式）

> 状态：**[progress]**（2026-11-01 立项；M1 已落地）
>
> 目标：把 `runtime/jit/signature.rs` 从“宿主编译期的 `TypeId` 常量表”改成
> **目标感知的调用绑定层**：`usize`/指针/`bool`/函数指针/128 位/聚合/变参全部由
> 产物自己的 `JitTarget` 解析；`bind` 在装载期校验，返回带生命周期的 `JitFn`。

## 1. 设计依据（网络研究）

- **Bevy**：`SystemParam` 用 `State`+GAT `Item`+`get_param` 描述“参数如何取”；
  `SystemParamFunction<Marker>` 用 `In/Out/Param`+`run` 描述“函数如何执行”，按元数
  实现到 16（[SystemParam](https://docs.rs/bevy/latest/bevy/ecs/system/trait.SystemParam.html)、
  [SystemParamFunction](https://docs.rs/bevy/latest/bevy/ecs/system/trait.SystemParamFunction.html)、
  [PR #1805](https://github.com/bevyengine/bevy/pull/1805)）。
- **Wasmtime**：`WasmTy` 每个值类型自带 `valtype`/`typecheck`；`WasmParams`/`WasmResults`
  为元组做检查；`TypedFunc<Params, Results>` 是已静态类型化的函数，`new_unchecked`
  是逃生口（[typed.rs](https://docs.wasmtime.dev/api/src/wasmtime/runtime/func/typed.rs.html)）。
- **Rust 1.99**：C 变参**定义**稳定（`...` 类型为 `VaList`，[公告](https://blog.rust-lang.org/2026/10/01/Rust-1.99.0/)）；
  Rust 早就能调用外部 C 变参函数。
- **i128/u128**：已从 `improper_ctypes_definitions` 移除（[#141206](https://github.com/rust-lang/rust/issues/141206)），
  FFI 可用，但目标 ABI 需后端规划/交叉执行验证。
- **bool**：Rust `bool` 与 C99 `_Bool` 同表示（1 字节、0/1），IR 的 `i1` 在 ABI 边界按 8 位槽规范化。
- **IR 函数类型**：`forge-ir` 的 `TypeEntry::Function { params, rets, is_vararg }` 已存在，
  `JitType::Function` 可以真正与产物源码签名比较。

## 2. 类型面

```rust
pub enum JitType {
    Void, Bool, Int { bits: u16 }, Float { bits: u16 },
    Ptr { addr_space: u32 },
    Function { params: Vec<JitType>, rets: Vec<JitType>, variadic: bool },
    Struct { size: u32, align: u16, packed: bool, fields: Vec<JitType> },
    Enum { repr: Box<JitType>, variants: Vec<JitEnumVariant> },
    Union { size: u32, align: u16, fields: Vec<JitType> },
    Array { elem: Box<JitType>, len: u64 },
    Vector { elem: Box<JitType>, lanes: u32 },
    UnsupportedAbi(&'static str),
}
```

- `JitTarget { arch, pointer_bits, int_widths, endian }` 从产物 `TypeStore.data_layout`
  与 `TargetMachine` 捕获；**没有** `cfg(target_pointer_width)`。
- `JitType::from_ir` 递归构造，深度上限 64；`to_abi` 做 ABI 槽投影（`Bool→Int8`、
  函数指针→`Ptr`）；聚合/向量/128 位在 M1 明确 `Unsupported`，不静默猜。

## 3. API（M1 已落地）

```rust
pub unsafe trait JitValue { fn jit_type(target: &JitTarget) -> JitType; }
pub unsafe trait JitCallSignature: Copy + 'static {
    fn source(target: &JitTarget) -> JitSigShape;  // IR 语义
    fn abi(target: &JitTarget) -> JitSigShape;     // 生成码真实接口
}
impl<M: TargetMachine> JitLoader<M> {
    pub fn bind<F: JitCallSignature>(&self, name: &str) -> Result<JitFn<'_, F>, JitBindError>;
    pub fn bind_source<F: JitCallSignature>(&self, name: &str) -> Result<JitSigShape, JitBindError>;
    pub unsafe fn bind_raw<F>(&self, name: &str) -> Result<JitFn<'_, F>, JitBindError>;
}
```

- `JitFn::call`（0–16 元数）在宿主一致时安全；跨目标 `call` **panic**（fail-closed），
  `raw()` 自担。`bind` 不拒绝跨目标——只做签名校验。
- `bind` 按 ABI 签名；`bind_source` 只校验源码，不返回可调用指针。
- 旧 `get`/`get_unchecked` 保留（V1 源码路径），新代码用 `bind`/`bind_raw`。

## 4. M1 已落地（2026-11-01）

- `CompiledUnit` 新增 `jit_sig: JitSig`（source + ABI + target）；`compile_with_jit_sig`
  在合法化后捕获 ABI 签名。
- `JitValue`/`JitCallSignature` 0–16：整数/浮点/`bool`/`char`/`usize`/`isize`/指针/
  可空指针/`NonNull`/函数指针/`i128`/`u128`（类型层）。
- `bind`/`bind_source`/`bind_raw` + `JitFn` + `JitBindError`（含形参下标与 hint）。
- 演示 `tests/jit_bind_demo.rs`（x86_64）：

  ```text
  add(2,3)=5
  not(true)=false
  ```

  并验证 `u8` 对 `i1` 源码签名必须失败、`bool` 源码签名通过。
- 单测：32 位 `JitTarget` 下 `usize→Int32`、函数指针 `JitType::Function`、
  `i128` ABI 显式 `UnsupportedAbi`；`Enum`/`Union` 的 C ABI 投影（无字段枚举→repr、
  带字段枚举/union 按值显式 `UnsupportedAbi`）；lib 3109 条、guards、clippy 绿。

## 4.1 M1.5：Enum/Union 与 `jit_rust_abi` 边界（2026-11-01）

- `JitType` 新增 `Enum { repr, variants }` 与 `Union { size, align, fields }`；
  无字段枚举按 `repr.to_abi()`（C 无字段枚举就是整数）；带字段枚举/union 按值
  返回 `UnsupportedAbi`（M2 planner / M3 反射桥），不静默猜布局。
- `forge-codegen` 增加 Cargo feature `jit_rust_abi`（默认关，不另建 crate）；
  `runtime/jit/rust_abi.rs` 固定 `JitReflect` 边界。当前 registry 未 vendor
  `bevy_reflect`，因此**先稳定 C ABI**，Bevy 适配器在依赖可用时实现同一 trait；
  不在核心 crate 硬编码 Bevy。
- 目标：Rust 类型不必 `repr(C)`，反射读取字段/变体后生成 tag+payload 线格式 +
  `extern "C"` shim，实现 Rust ABI ↔ C ABI；官方反射稳定后只换反射来源。

## 4.2 M3-a：变参绑定（2026-11-01）

- `JitCallSignature`/`JitValue` 增加 `unsafe extern "C" fn(fixed..., ...) -> R`
  实现（固定前缀 0–16，`variadic: true`）；`JitFn` 不生成泛型 `call`（varargs
  无法泛型表达），调用走 `raw()`。
- `JitLoader::bind_variadic`（**unsafe**）：校验产物为变参 + 固定前缀 ABI，返回
  `JitFn`；非变参 `bind` 继续拒绝并提示。安全性：当前后端只支持 **JIT 生成 caller**
  的变参保存区契约，callee 的 `va_start` 不保存外部调用者的参数寄存器；host 直接
  调用会 `va_arg` 的变参 JIT 函数会读垃圾。修复 callee 保存前只能 `unsafe` 使用。
- demo：`vsum(i64, ...)` 返回常量 7，`raw()(1,2,3)` → `variadic_sum(1,2,3)=7`；
  `vsum_n(n, ...)` 用 `n` 驱动循环 → `variadic_counted(3,10,20,30)=60`；
  哨兵版扫描到 0 → `variadic_sentinel(10,20,30,0)=60`。
- **截止契约（M3-b）**：C/Rust 的 `...` 本身**没有计数**；IR 的 `VaArg` 只取一个值，
  无法从 ABI 推断调用者传了几个。必须由固定参数 `n`、sentinel 或 format 约定；
  demo `vsum_n(n, ...)` 用 `n` 驱动 IR 循环，`vsum_n(3,10,20,30)=60`（模块路径，
  非编译期固定 3 次）。
- **长度契约 API**：`VariadicTermination::{Unspecified, Counted{param}, Sentinel, Format}`；
  `bind_variadic_with_termination` 在绑定期校验 `Counted` 的固定参数是整数。
  `Sentinel`/`Format` 只记录契约，callee 负责扫描/解析；不假装 ABI 能自动计数。
- **已知问题**：host 直接 `add_function` 一个变参函数再 `raw()` 调用，`va_arg`
  目前读到垃圾（模块路径正常）；这是 `add_function` 的变参 ABI 装配缺口，
  不能把 `bind_variadic` 的宣传当成“直接调用已支持”。

## 4.3 M3-d：安全 `ptr+len` 变参替代（2026-11-01）

- `JitLoader::bind_slice::<T, R>(name)` 绑定 ABI 为 `(ptr: *const T, len: usize) -> R`
  的 JIT 函数，返回 `JitSliceFn`；`call(&[T])` 安全、长度运行时可变。
- 这是“用户决定长度怎么处理”的安全形态：显式 `len`，不依赖 C `...` 自动计数，
  也没有外部 C 变参保存区问题。
- demo：`sum_slice(ptr, n)` 循环读切片，`slice_sum(10,20,30)=60`；
  `bind_slice_mut` 原地写回，`slice_fill(4)=[7,7,7,7]`。
- C `...` 仍保留 `bind_variadic`（unsafe）；真正动态 C 变参需要 libffi 或
  callee 保存参数寄存器，不在本切片里伪装成安全。

## 4.4 M1b-partial：i128 by-ref 入口（2026-11-01）

- win64 C ABI 的 `i128` 形参是**按引用**传（`cross_abi.rs` 已由 LLVM 对拍证实：
  `mov 0x8(%rcx),%rax`，返回走 XMM0）。`agg_param` 现在识别 `TypeId::I128` 的
  `ByRef` 落点：签名改 `ptr`，入口发 `load i128 [ptr]`，再交给 `wide_split` 拆肢。
- `tests/i128_entry_by_ref.rs`：直接 IR `(i128,i128)->i64` 先编译、再以
  `extern "C" fn(*const i128,*const i128)->i64` 真执行，`2+3=5`。
- 仍待：128 位**返回**（win64 XMM0）、sysv64 整数对/by-value、`JitSig` 的
  i128 ABI 投影（当前 `to_abi` 仍是 `UnsupportedAbi`，测试用 `bind_raw`）。
- **返回失败的根因（2026-11-01 修正）**：`edges.ret` 早已是
  `Reg { FPR(128), XMM0 }`；坏 IR `load i64, i64 %a` 来自
  `agg_call::expand_large_agg_ret` 把 i128 当成 >8B 聚合去物化。修复后 i128 不再
  走聚合返回展开；`wide_split::is_fully_supported` 单独检查终结符宽返回，直接 IR
  现在明确 `WIDE_INT_SHAPE: Ret`（guard `i128_return_fail_closed`），不再产出坏 IR。
- **返回支持仍待**：需要在允许 Ret 的路径里用 `set_return_values` 正确重写终结符
  并打包 V128；只改 `inst.operands` 会留下 use-list 泄漏。

## 5. 未完成（M1b / M2 / M3）

| 里程碑 | 内容 | 验收 |
| --- | --- | --- |
| M1b | 128 位入口/返回 ABI：`wide_split` 入口拆分、返回配对/sret；`plan_jit_abi` 初版；`rustc-abi-cross` 对拍 | i128/u128 by-value 真执行；三 ISA 编译。**进展**：win64 by-ref `i128` 形参已编译+真执行（`2+3=5`）；128 位返回 XMM0、sysv64 整数对、`JitSig` ABI 投影仍待做 |
| M2 | `#[derive(JitValue)]`、聚合/数组/向量、`ByVal`/`ByRef`/`Out`、sret/by-ref；完整 `plan_jit_abi`；转换 12 处 unchecked 指针站点 | win64/sysv64 聚合真执行；三 ISA；demo 聚合 |
| M3 | ~~`bind_variadic`~~（已落地）、~~counted loop~~（已落地，`n` 驱动循环）、sentinel/format 截止约定、`add_function` 直接变参调用修复、`jit_rust_abi` 的 Bevy `Reflect` 适配器、函数指针 JIT 回调 demo、跨目标 bind/call 测试、错误文档 | 变参 demo；带字段枚举/union 真执行；错误带下标；54/54 |

## 6. 边界

- 跨目标执行不在本方案（`bind` 允许检查，`call` panic，`raw()` 自担）。
- 变参的 `...` 部分由 Rust 变参类型与调用者保证；泛型安全调用无法表达，`raw()` 是语言边界内的正确形态。
- 聚合 by-value 的 Rust/C ABI 与 JIT 后端必须一致；M2 的 `plan_jit_abi` 是单一事实源，不一致 fail-closed。

## 7. Rust ABI 反射桥（`jit_rust_abi` feature）

> 目标：Rust 类型**不必** `#[repr(C)]`，由反射读取逻辑布局（字段/变体/判别值），
> 生成 **C ABI 线格式 + 适配器**，在 Rust ABI 与 JIT 的 C ABI 之间无缝转换；
> 官方反射稳定后只替换反射来源，不动 `JitType`/`bind`。

- **不另建 crate**：`forge-codegen` 的 Cargo feature `jit_rust_abi`（默认关）启用
  `bevy_reflect` 适配层；核心 `JitType`/`bind` 不依赖 Bevy。
- 类型映射：`ReflectKind::Struct/TupleStruct` → `Struct`；`ReflectKind::Enum`
  → `Enum`（字段变体保留 payload）；union 由 `Reflect` 的 opaque/bytes + 手工
  `JitReflect` 提供 `Union` 描述（Bevy 没有 union kind）；`Value` 标量按位宽映射。
- **C ABI 先稳定**：无字段枚举按 `repr` 整数传值；`Union` 先按引用；带字段枚举/
  union 的按值走 M2 的 `plan_jit_abi`，M3 的反射桥生成 tag+payload 线布局 +
  `extern "C"` shim（把 Rust 值展开成线布局、调用、再收回来），Rust 侧无需 C 布局。
- **交换格式**：反射桥只产出 `JitType::Enum/Union` 与 ABI 槽，`bind`/`JitFn`
  不感知 Bevy；后续官方反射通过同一 `JitReflect` trait 接入。
- **当前环境**：registry 未 vendor `bevy_reflect`，所以先落地 Enum/Union 描述与
  C ABI 规则（fieldless enum、union by-ref），feature 骨架与依赖在可用时接入；
  不把未编译的反射 API 伪装成已支持。
