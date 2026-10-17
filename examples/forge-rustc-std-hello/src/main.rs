//! `std` 探测：forge-rustc 后端 + LLVM 预编译的 std（v27 M3-②）。
//!
//! 跑法见本目录 README（需要网络：`-Zbuild-std=std` 要解析 std 的构建依赖）。
fn main() {
    println!("hello from std");
}
