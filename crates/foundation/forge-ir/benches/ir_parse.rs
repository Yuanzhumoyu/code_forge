//! 5.2 大模块解析基准（criterion——stable 可跑，CI 可用）。
//!
//! 运行：`cargo bench -p forge-ir`
//!
//! 基线（2026-08 第三轮：verify 校验强化 + 聚合常量值表示 + distinct/metadata key 后）：
//! 中等模块 parse ~56µs、parse+display+reparse ~158µs（与上轮持平，语法扩展未引入
//! 可感知开销）。
//! 5.1 grammar 拆分二次评估：增量编译 ~2s（<15s 阈值），暂不拆分。

use criterion::{Criterion, criterion_group, criterion_main};
use forge_ir::ir_parser::parse_module;

/// 中等规模模块（含全局、常量表达式、聚合、多函数、异常处理）——覆盖文本层
/// 主要语法路径的解析吞吐。
const MEDIUM_MODULE: &str = r#"
@h = global i32 0
@arr = global [4 x i32] [i32 1, i32 2, i32 3, i32 42]
@g32 = global i32 ptrtoint (ptr @h to i32)
@gp = global ptr getelementptr (i32, ptr @h, i64 2)
@nested = global i32 ptrtoint (bitcast (ptr @h to ptr) to i32)
@agg = global {i32, i32} {i32 7, i32 9}
@empty = global {} zeroinitializer
%X = type <{ i32, i64 }>
declare void @__gxx_personality_v0()
declare i32 @may_throw(i32)
define i32 @fib(i32 %n) {
  %entry:
    %c = icmp ult i32 %n, i32 2
    br i1 %c, label %base, label %recur
  %base:
    ret i32 %n
  %recur:
    %n1 = sub i32 %n, i32 1
    %a = call i32 @fib(i32 %n1)
    %n2 = sub i32 %n, i32 2
    %b = call i32 @fib(i32 %n2)
    %s = add i32 %a, i32 %b
    ret i32 %s
}
define i32 @main() {
  %entry:
    %x = add i32 20, i32 22
    %y = mul i32 %x, i32 2
    %v = load i32, ptr @arr
    %sum = add i32 %y, i32 %v
    invoke i32 @may_throw(i32 1) to label %ok unwind label %pad
  %ok:
    ret i32 %sum
  %pad:
    %l = landingpad { ptr, i32 } cleanup
    resume { ptr, i32 } %l
}
"#;

fn bench_parse_medium_module(c: &mut Criterion) {
    c.bench_function("parse_medium_module", |b| {
        b.iter(|| {
            let m = parse_module(MEDIUM_MODULE).expect("parse");
            std::hint::black_box(m);
        });
    });
}

fn bench_parse_display_roundtrip(c: &mut Criterion) {
    c.bench_function("parse_display_roundtrip", |b| {
        b.iter(|| {
            let m = parse_module(MEDIUM_MODULE).expect("parse");
            let text = m.to_string();
            let m2 = parse_module(&text).expect("reparse");
            std::hint::black_box(m2);
        });
    });
}

criterion_group!(
    benches,
    bench_parse_medium_module,
    bench_parse_display_roundtrip
);
criterion_main!(benches);
