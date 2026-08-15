//! forge-rustc cargo 模板工程：用 code-forge backend 编译的 no_std 程序。
//!
//! 用法（workspace 根）：
//! ```bash
//! cargo build -p forge-rustc   # 先构建 backend dll
//! cd examples/forge-rustc-hello
//! RUSTFLAGS="-Zcodegen-backend=$(pwd)/../../target/debug/forge_rustc.dll \
//!   -C link-args=/SUBSYSTEM:CONSOLE\ /DEFAULTLIB:kernel32.lib\ /DEFAULTLIB:vcruntime.lib" \
//!   cargo +nightly build
//! ./target/debug/forge-rustc-hello.exe
//! ```
//!
//! 说明：
//! - `-Zcodegen-backend` 指向 forge_rustc.dll（需 nightly + rustc-dev 构建）
//! - no_std + no_main + `#[no_mangle] extern "C" fn main() -> i32`：
//!   MSVC 默认入口 `mainCRTStartup` 自动转调 `main`，无需 /ENTRY
//! - `/DEFAULTLIB:vcruntime.lib` 提供 `__CxxFrameHandler3`（SEH 符号）
#![no_std]
#![no_main]

#[panic_handler]
fn panic(_info: &core::panic::PanicInfo) -> ! {
    loop {}
}

/// 程序入口：返回值 = 进程退出码。
#[unsafe(no_mangle)]
pub extern "C" fn main() -> i32 {
    let mut acc = 0i32;
    let mut i = 0;
    while i < 10 {
        acc += i;
        i += 1;
    }
    acc // 45
}
