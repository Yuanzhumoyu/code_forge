//! 二进制序列化吞吐基准（criterion）：`Module::to_binary` / `Module::from_binary`，
//! 外加"文本解析 vs 缓存命中"这一对（回答缓存值不值得）。
//!
//! 运行：`cargo bench -p forge-ir --bench ir_binary`
//!
//! 基线（2026-09-19 本机首次采集，见 `docs/reference/binary-format.md` §10 与
//! `docs/performance/bench_baseline.md`）：中等模块编码与解码都在亚毫秒量级。
//! v2 起编码含段体压缩（体积减半、编码 +2.1×、解码 −12%），数字在同一张表里对照。
//!
//! 与 `ir_parse` 基准的分工：那里量的是**文本层**（logos+lalrpop）解析吞吐，
//! 这里量的是**二进制层**编解码吞吐；`ir_binary_cache/*` 组把两者放在一起量，
//! 直接回答"缓存省了多少"（`parse_text` vs `cache_hit`）。

use criterion::{Criterion, Throughput, criterion_group, criterion_main};
use forge_ir::IrCache;
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

/// 大模块：LLVM `test/Assembler` 语料里文本最大的一个（43 KB，字节流 8.5 KB）。
///
/// 用 `include_str!` 编译进来（而不是运行时读文件）：基准要量的是"解析 vs 缓存"，
/// 不该把"找文件"的成本混进夹具准备。
const LARGE_MODULE: &str = include_str!(concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/tests/llvm_assembler_cases/auto_upgrade_nvvm_intrinsics.ll"
));

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

    // 缓存对照组：夹具模块每跑一次基准都要"文本 → 模块"，这一步可以被文件级缓存跳过。
    // 两个测量点用**同一份源码**：`parse_text` = 每次都解析；`cache_hit` = 直接反序列化。
    let cache_dir = std::path::PathBuf::from(env!("CARGO_TARGET_TMPDIR")).join("ir_binary_cache");
    let _ = std::fs::remove_dir_all(&cache_dir);
    let cache = IrCache::new(&cache_dir).expect("建缓存目录");
    cache
        .store(MEDIUM_MODULE, &module)
        .expect("预置缓存条目（模拟「已经编译过一次」）");
    // 预置必须真的可命中：否则这组基准量的是"缓存未命中 + 解析"，会给出错误结论。
    let hit = cache.load(MEDIUM_MODULE).is_some();
    assert!(hit, "预置的缓存条目必须可命中");
    eprintln!(
        "ir_binary_cache: 缓存条目 {} 个，目录 {}",
        cache.entry_count(),
        cache.dir().display()
    );

    let mut group = c.benchmark_group("ir_binary_cache");
    group.throughput(Throughput::Bytes(MEDIUM_MODULE.len() as u64));
    group.bench_function("parse_text", |b| {
        b.iter(|| parse_module(MEDIUM_MODULE).expect("解析"))
    });
    group.bench_function("cache_hit", |b| {
        b.iter(|| cache.load(MEDIUM_MODULE).expect("命中"))
    });
    // 把 `cache_hit` 拆开，好判断"缓存到底省在哪、亏在哪"：
    // 没有这两个测量点，就只能看到"缓存比解析慢"这个结论而说不出原因。
    let entry_path = cache.entry_path(MEDIUM_MODULE);
    let entry_bytes = std::fs::read(&entry_path).expect("读缓存条目");
    group.bench_function("read_entry", |b| {
        b.iter(|| std::fs::read(&entry_path).expect("读条目"))
    });
    group.bench_function("decode_entry", |b| {
        b.iter(|| Module::from_binary(&entry_bytes).expect("解码条目"))
    });
    group.finish();

    // 大模块对照组：缓存值不值得**取决于"解析成本 vs 文件读取代价"**，所以必须给两个
    // 尺度。884 B 的中等模块看不出收益（见上组：命中 ≈ 读文件 + 解码，而读文件本身
    // 就是主要开销）；43 KB 的语料最大模块才是有代表性的"值得缓存"形态。
    let large_src = LARGE_MODULE;
    let large = parse_module(large_src).expect("大夹具必须可解析");
    let large_dir =
        std::path::PathBuf::from(env!("CARGO_TARGET_TMPDIR")).join("ir_binary_cache_large");
    let _ = std::fs::remove_dir_all(&large_dir);
    let large_cache = IrCache::new(&large_dir).expect("建缓存目录");
    large_cache.store(large_src, &large).expect("预置大条目");
    assert!(large_cache.load(large_src).is_some(), "大条目必须可命中");
    let large_entry = std::fs::read(large_cache.entry_path(large_src)).expect("读大条目");
    eprintln!(
        "ir_binary_cache_large: 源码 {} B → 字节流 {} B（{:.2}×）",
        large_src.len(),
        large_entry.len(),
        large_entry.len() as f64 / large_src.len() as f64
    );

    let mut group = c.benchmark_group("ir_binary_cache_large");
    group.throughput(Throughput::Bytes(large_src.len() as u64));
    group.bench_function("parse_text", |b| {
        b.iter(|| parse_module(large_src).expect("解析"))
    });
    group.bench_function("cache_hit", |b| {
        b.iter(|| large_cache.load(large_src).expect("命中"))
    });
    group.bench_function("read_entry", |b| {
        b.iter(|| std::fs::read(large_cache.entry_path(large_src)).expect("读条目"))
    });
    group.bench_function("decode_entry", |b| {
        b.iter(|| Module::from_binary(&large_entry).expect("解码条目"))
    });
    group.finish();
}

criterion_group!(benches, bench);
criterion_main!(benches);
