//! 二进制序列化吞吐基准（criterion）：`Module::to_binary` / `Module::from_binary`。
//!
//! 运行：`cargo bench -p forge-ir --bench ir_binary`
//!
//! 基线（2026-09-19 本机首次采集，见 `docs/reference/binary-format.md` §9）：
//! 中等模块（含 globals / 常量表达式 / 聚合 / 元数据 / 两函数）编码与解码都在
//! 亚毫秒量级——**先有基线再谈压缩**（压缩属于未来工作，届时与本表对照）。
//!
//! 与 `ir_parse` 基准的分工：那里量的是**文本层**（logos+lalrpop）解析吞吐，
//! 这里量的是**二进制层**编解码吞吐，两者共同回答"缓存省了多少"。

use criterion::{Criterion, Throughput, criterion_group, criterion_main};
use forge_ir::Module;
use forge_ir::text::parser::parse_module;

/// 中等规模模块：覆盖 globals（含常量表达式/聚合）、元数据、命名类型、两个函数
/// （含分支与块参数）——二进制层各段都会出现在字节流里。
const MEDIUM_MODULE: &str = r#"
source_filename = "bench.c"
target triple = "x86_64-pc-windows-msvc"

@h = global i32 0
@arr = global [4 x i32] [i32 1, i32 2, i32 3, i32 42]
@g32 = global i32 ptrtoint (ptr @h to i32)
@agg = global {i32, i32} {i32 7, i32 9}
@v = global <4 x float> zeroinitializer, !tbaa !0

!0 = !{!"tbaa root"}
!1 = !{!"branch_weights", i32 1, i32 10}

define i32 @pick(i1 %c, i32 %a, i32 %b) {
  %entry:
    br i1 %c, label %t, label %f, !prof !1
  %t:
    %x = add i32 %a, 1
    br label %m
  %f:
    %y = mul i32 %b, 2
    br label %m
  %m:
    %r = phi i32 [ %x, %t ], [ %y, %f ]
    ret i32 %r
}

define i32 @sum(i32 %n) {
  %entry:
    %c = icmp ult i32 %n, i32 2
    br i1 %c, label %base, label %recur
  %base:
    ret i32 %n
  %recur:
    %n1 = sub i32 %n, 1
    %n2 = sub i32 %n, 2
    %a = call i32 @sum(i32 %n1)
    %b = call i32 @sum(i32 %n2)
    %s = add i32 %a, %b
    ret i32 %s
}
"#;

fn bench(c: &mut Criterion) {
    let module = parse_module(MEDIUM_MODULE).expect("基准模块必须可解析");
    let bytes = module.to_binary();
    eprintln!(
        "ir_binary: 源码 {} B → 字节流 {} B（{:.2}×，{} 函数）",
        MEDIUM_MODULE.len(),
        bytes.len(),
        bytes.len() as f64 / MEDIUM_MODULE.len() as f64,
        module.function_count()
    );

    let mut group = c.benchmark_group("ir_binary");
    group.throughput(Throughput::Bytes(bytes.len() as u64));
    group.bench_function("encode", |b| b.iter(|| module.to_binary()));
    group.bench_function("decode", |b| {
        b.iter(|| Module::from_binary(&bytes).expect("基准字节流必须可解码"))
    });
    group.finish();
}

criterion_group!(benches, bench);
criterion_main!(benches);
