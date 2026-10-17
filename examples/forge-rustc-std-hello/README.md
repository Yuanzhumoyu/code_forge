# `std` 探测工程（forge-rustc 后端 · v27 M3-②）

用 **forge-rustc** 当 rustc 后端、`std` 由 **LLVM 预编译**（`-Zbuild-std` + `rustc-wrapper`
剥离系统 crate 的 `-Zcodegen-backend`）编译一个最小的 `println!` 程序。

> 这是**探测**工程，不是产品示例：目标是量出"真 `std` 程序还差什么"。
> 现状（2026-10-11）：本仓 sandbox 内**跑不通**——`-Zbuild-std=std` 需要从 registry 解析
> `std` 的构建依赖（`cfg-if`/`getopts` 等），而 sandbox **没有网络凭证**
> （实测：`SSL connect error … SEC_E_NO_CREDENTIALS`）。**沙箱外一条命令即可得到结论**。

## 前置

1. 仓库根构建后端 dll 与 wrapper：

   ```bash
   cargo build -p forge-rustc
   cargo build --manifest-path tools/forge-rustc-wrapper/Cargo.toml
   ```

2. `rust-src` 组件（`-Zbuild-std` 需要）：

   ```bash
   rustup component add rust-src --toolchain nightly
   ```

3. 本工程带 `rust-toolchain.toml`（**浮动 `nightly`**）——用浮动通道是因为本机/CI 通常只装了
   浮动 nightly；如果你的环境装的是固定日期通道，把 `channel` 改成那一版即可。

## 跑

在**本目录**下：

```bash
# ① 写 .cargo/config.toml（rustflags + rustc-wrapper + build-std=["std","panic_abort"]）
#    `--std` 是 v27 新增的口径；它**不加** `-Zshare-generics=yes`（本后端明确拒绝，见 WA-04）。
cargo forge init --std

# ② 构建并运行
cargo build
./target/debug/forge-rustc-std-hello.exe
```

期望：打印 `hello from std`。

## 已排除的前置阻塞（v27 已落地）

- **TLS**：`std` 的 `thread_local!` 依赖线程局部静态；后端已用动态 TLS
  （`TlsAlloc`/`TlsGetValue` + 首触初始化）实现，e2e `thread_local_roundtrip` 守护。
- **`abort()`**：`std` 的 `abort_internal` 走 `core::intrinsics::abort()`；后端已实现
  （`ExitProcess(3)`），e2e `intrinsic_abort` 守护。
- **alloc 运行时**：链 `std` 时不再注入手写 alloc shim（`std` 自带那一族），
  避免 LNK2005——见 `WA-02`。

## 还不知道的（这正是要探测的）

- `std` 的入口链（`mainCRTStartup` → `std::rt::lang_start`）与我们的 `main` 约定是否对得上；
- `std` 的 SEH/unwind 表（`panic=abort` 下是否仍需要 `.pdata`/`.xdata`）；
- 其它"`std` 用到、而我们还没实现"的 intrinsic/ABI 落点——**预期会以明确的
  `[cap:…]` 或编译错误的形式暴露**（后端的原则是不静默降级）。
