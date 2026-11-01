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
    Array { elem: Box<JitType>, len: u64 },
    Vector { elem: Box<JitType>, lanes: u32 },
    Unsupported(&'static str),
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
  `i128` ABI 显式 `Unsupported`；lib 3107 条、guards、clippy 绿。

## 5. 未完成（M1b / M2 / M3）

| 里程碑 | 内容 | 验收 |
| --- | --- | --- |
| M1b | 128 位入口/返回 ABI：`wide_split` 入口拆分、返回配对/sret；`plan_jit_abi` 初版；`rustc-abi-cross` 对拍 | i128/u128 by-value 真执行；三 ISA 编译 |
| M2 | `#[derive(JitValue)]`、聚合/数组/向量、`ByVal`/`ByRef`/`Out`、sret/by-ref；完整 `plan_jit_abi`；转换 12 处 unchecked 指针站点 | win64/sysv64 聚合真执行；三 ISA；demo 聚合 |
| M3 | `bind_variadic`（固定前缀 + `raw()` 调用）、函数指针 JIT 回调 demo、跨目标 bind/call 测试、错误文档 | 变参 demo；错误带下标；54/54 |

## 6. 边界

- 跨目标执行不在本方案（`bind` 允许检查，`call` panic，`raw()` 自担）。
- 变参的 `...` 部分由 Rust 变参类型与调用者保证；泛型安全调用无法表达，`raw()` 是语言边界内的正确形态。
- 聚合 by-value 的 Rust/C ABI 与 JIT 后端必须一致；M2 的 `plan_jit_abi` 是单一事实源，不一致 fail-closed。
