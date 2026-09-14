//! 文本名/助记符查表的每次查找耗时（回归工具，默认不跑）。
//!
//! 背景（2026-09-14）：S1 把 LLVM 文本名表搬进 `ops.toml` 时，查表一度写成
//! `Opcode::ALL.iter().find(...)`（109 次 `&str` 比较/次），比手写 `match`
//! 慢一个量级。现在是**生成 match**（build.rs 从 ops.toml 生成，名字唯一性在
//! 生成期断言），本文件用来盯住这一点。
//!
//! 本机实测（debug 构建，200k 轮 × 28 个名字 = 560 万次查找）：
//!
//! | 查找 | 线性扫 ALL | 生成 match | 提升 |
//! | --- | --- | --- | --- |
//! | `from_llvm_name` | 2770.5 ns | 203.5 ns | 13.6× |
//! | `from_mnemonic` | 2532.2 ns | 300.8 ns | 8.4× |
//! | `from_name` | 2806.0 ns | 551.2 ns | 5.1× |
//!
//! 跑法：`cargo test -p forge-ir --test llvm_name_lookup_perf -- --ignored --nocapture`
//! （计时型测试不进默认 CI：不做断言，留作工具）。

use forge_ir::Opcode;
use std::hint::black_box;
use std::time::Instant;

/// 命中的文本名（跨标量/向量/内存/原子各类）
const HITS: &[&str] = &[
    "add",
    "mul",
    "fdiv",
    "lshr",
    "ashr",
    "ctpop",
    "sadd.with.overflow",
    "load",
    "store",
    "getelementptr",
    "call",
    "callbr",
    "extractelement",
    "vextractelement",
    "ptrtoaddr",
    "select",
    "atomicrmw",
    "fence",
    "isnotnull",
    "landingpad",
];

/// 未命中（强制走完整扫描）
const MISSES: &[&str] = &[
    "nosuchop", "addx", "mulx", "ldiv", "cmpxchgx", "faddx", "va_argx", "trapx",
];

fn bench(label: &str, f: impl Fn(&str) -> Option<Opcode>) {
    const ROUNDS: usize = 200_000;
    // 预热
    for _ in 0..1000 {
        for n in HITS.iter().chain(MISSES) {
            black_box(f(black_box(n)));
        }
    }
    let calls = ROUNDS * (HITS.len() + MISSES.len());
    let start = Instant::now();
    let mut sink = 0usize;
    for _ in 0..ROUNDS {
        for n in HITS.iter().chain(MISSES) {
            if f(black_box(n)).is_some() {
                sink += 1;
            }
        }
    }
    let elapsed = start.elapsed();
    black_box(sink);
    println!(
        "{label}: {calls} 次查找 / {:.1} ms = {:.1} ns/次",
        elapsed.as_secs_f64() * 1e3,
        elapsed.as_secs_f64() * 1e9 / calls as f64
    );
}

#[test]
#[ignore = "计时型回归工具：cargo test -p forge-ir --test llvm_name_lookup_perf -- --ignored --nocapture"]
fn llvm_name_lookup_perf() {
    bench("from_llvm_name", Opcode::from_llvm_name);
    bench("from_mnemonic", Opcode::from_mnemonic);
    bench("from_name", Opcode::from_name);
}
