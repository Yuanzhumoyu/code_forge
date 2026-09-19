//! 二进制 IR 缓存的**消费者示例**——只用 `forge_ir` 的公开 API（因此它同时是
//! "公开面够不够用"的编译期检验：示例是独立 crate，拿不到 crate 内部口）。
//!
//! ```bash
//! # 往返 + 逐函数校验 + 尺寸/耗时对照（默认模式）
//! cargo run -p forge-ir --example ir_binary -- tests/llvm_assembler_cases/fib.ll
//!
//! # 顺带把字节流落盘成 .fir
//! cargo run -p forge-ir --example ir_binary -- --write target/ir-cache a.ll b.ll
//!
//! # 只解析头部报兼容性（不建 IR）——缓存方的"先看能不能读"路径
//! cargo run -p forge-ir --example ir_binary -- --check a.ll
//!
//! # 走文件级 IR 缓存（内容键、自愈）：第一次 MISS 并落盘，之后 HIT 直接反序列化
//! cargo run -p forge-ir --example ir_binary -- --cache target/ir-cache tests/llvm_assembler_cases/*.ll
//! ```
//!
//! 判据（任何一条不满足 ⇒ 该文件记 FAIL 并最终 exit 1）：
//! 1. `from_binary(to_binary(m))` 成功；
//! 2. 解码后每个函数过 `Verifier`；
//! 3. 解码后**文本打印与首次一致**（文本 = 用户可见语义的规范形）；
//! 4. `decode → encode` 与首次编码**逐字节相同**（编码幂等）。

use std::error::Error;
use std::path::{Path, PathBuf};
use std::process::ExitCode;
use std::time::Instant;

use forge_ir::text::parser::parse_module;
use forge_ir::verify::Verifier;
use forge_ir::{IrCache, Module, check_binary_compat};

struct Args {
    files: Vec<PathBuf>,
    write_dir: Option<PathBuf>,
    cache_dir: Option<PathBuf>,
    check_only: bool,
}

fn parse_args() -> Result<Args, String> {
    let mut args = Args {
        files: Vec::new(),
        write_dir: None,
        cache_dir: None,
        check_only: false,
    };
    let mut it = std::env::args().skip(1);
    while let Some(a) = it.next() {
        match a.as_str() {
            "--write" => {
                args.write_dir = Some(PathBuf::from(it.next().ok_or("--write 需要一个目录参数")?));
            }
            "--cache" => {
                args.cache_dir = Some(PathBuf::from(it.next().ok_or("--cache 需要一个目录参数")?));
            }
            "--check" => args.check_only = true,
            "-h" | "--help" => return Err("help".to_string()),
            other if other.starts_with('-') => return Err(format!("未知参数 {other}")),
            other => args.files.push(PathBuf::from(other)),
        }
    }
    if args.files.is_empty() {
        return Err("至少给一个 .ll 文件".to_string());
    }
    Ok(args)
}

fn usage() {
    eprintln!(
        "用法：ir_binary [--check] [--write <dir>] [--cache <dir>] <file.ll>...\n\
         \n\
         默认：parse → to_binary → from_binary，逐函数校验 + 文本一致 + 字节幂等，打印尺寸/耗时。\n\
         --check：只 encode 后读头部（check_binary_compat），报版本/producer/段表。\n\
         --write <dir>：把字节流写为 <dir>/<名字>.fir。\n\
         --cache <dir>：走文件级 IR 缓存（内容键）；命中则跳过 parse，未命中则 parse 后落盘。"
    );
}

fn main() -> ExitCode {
    let args = match parse_args() {
        Ok(a) => a,
        Err(msg) => {
            if msg != "help" {
                eprintln!("错误：{msg}");
            }
            usage();
            return ExitCode::from(2);
        }
    };
    let cache = match args.cache_dir.as_ref().map(IrCache::new).transpose() {
        Ok(c) => c,
        Err(e) => {
            eprintln!("错误：缓存目录不可用：{e}");
            return ExitCode::from(2);
        }
    };

    let mut failures = 0usize;
    let mut total_ir = 0usize;
    let mut total_src = 0usize;
    let mut max_ir = (0usize, String::new());
    let mut hits = 0usize;

    for path in &args.files {
        match run_one(path, &args, cache.as_ref()) {
            Ok(stats) => {
                total_src += stats.src_bytes;
                total_ir += stats.ir_bytes;
                hits += usize::from(stats.cache_hit);
                if stats.ir_bytes > max_ir.0 {
                    max_ir = (stats.ir_bytes, stats.name.clone());
                }
            }
            Err(e) => {
                eprintln!("FAIL {}：{e}", path.display());
                failures += 1;
            }
        }
    }

    if args.files.len() > 1 {
        println!(
            "合计：{} 文件，源码 {} B → 字节流 {} B（{:.2}×），最大 {} B（{}）{}",
            args.files.len(),
            total_src,
            total_ir,
            total_ir as f64 / total_src.max(1) as f64,
            max_ir.0,
            max_ir.1,
            if cache.is_some() {
                format!("，缓存命中 {hits}/{}", args.files.len())
            } else {
                String::new()
            }
        );
    }
    if failures > 0 {
        eprintln!("{failures} 个文件失败");
        return ExitCode::FAILURE;
    }
    ExitCode::SUCCESS
}

struct Stats {
    name: String,
    src_bytes: usize,
    ir_bytes: usize,
    cache_hit: bool,
}

fn run_one(path: &Path, args: &Args, cache: Option<&IrCache>) -> Result<Stats, Box<dyn Error>> {
    let src = std::fs::read_to_string(path)?;
    let name = path
        .file_stem()
        .map(|s| s.to_string_lossy().to_string())
        .unwrap_or_else(|| "<unnamed>".to_string());

    // 有缓存时：命中就直接拿模块（跳过 parse），未命中才 parse 并落盘。
    // 注意判据 1–4 仍然全跑——缓存只替代"文本 → 模块"这一步，不替代任何校验。
    let t_parse = Instant::now();
    let (module, cache_hit, parse_mode) = match cache {
        Some(cache) => match cache.load(&src) {
            Some(m) => (m, true, "HIT"),
            None => {
                let m = parse_module(&src)?;
                // 落盘失败只影响下次速度，不影响本次正确性 ⇒ 忽略错误。
                let _ = cache.store(&src, &m);
                (m, false, "MISS")
            }
        },
        None => (parse_module(&src)?, false, "-"),
    };
    let parse_us = t_parse.elapsed().as_secs_f64() * 1e6;

    let t_enc = Instant::now();
    let bytes = module.to_binary();
    let encode_us = t_enc.elapsed().as_secs_f64() * 1e6;

    if args.check_only {
        let compat = check_binary_compat(&bytes)?;
        println!(
            "{name}: format_version={} producer={} 段=[{}] 字节={}（源码 {} B，{:.2}×）",
            compat.format_version,
            compat.producer.as_str(),
            compat
                .sections
                .iter()
                .map(|s| s.name())
                .collect::<Vec<_>>()
                .join(","),
            bytes.len(),
            src.len(),
            bytes.len() as f64 / src.len().max(1) as f64
        );
        return Ok(Stats {
            name,
            src_bytes: src.len(),
            ir_bytes: bytes.len(),
            cache_hit,
        });
    }

    let t_dec = Instant::now();
    let back = Module::from_binary(&bytes)?;
    let decode_us = t_dec.elapsed().as_secs_f64() * 1e6;

    // 2. 逐函数过校验器
    for f in back.iter_functions() {
        let mut v = Verifier::with_ctx(f.types.clone());
        if let Err(errs) = v.verify(f) {
            return Err(format!("解码后函数 {} 未过校验：{errs:?}", f.name).into());
        }
    }
    // 3. 文本一致（文本是用户可见语义的规范形）
    let before = module.to_string();
    let after = back.to_string();
    if before != after {
        let first = before
            .lines()
            .zip(after.lines())
            .position(|(a, b)| a != b)
            .map(|i| format!("第 {} 行", i + 1))
            .unwrap_or_else(|| {
                format!(
                    "行数 {} vs {}",
                    before.lines().count(),
                    after.lines().count()
                )
            });
        return Err(format!("解码后文本不一致（{first}）").into());
    }
    // 4. 编码幂等
    let again = back.to_binary();
    if again != bytes {
        return Err(format!("字节流不幂等（{} vs {} 字节）", bytes.len(), again.len()).into());
    }

    if let Some(dir) = &args.write_dir {
        std::fs::create_dir_all(dir)?;
        let out = dir.join(format!("{name}.fir"));
        std::fs::write(&out, &bytes)?;
        println!(
            "{name}: PASS 源码 {} B → 字节流 {} B（{:.2}×，{} 函数）→ {}  \
             {parse_mode} {parse_us:.0}µs / encode {encode_us:.0}µs / decode {decode_us:.0}µs",
            src.len(),
            bytes.len(),
            bytes.len() as f64 / src.len().max(1) as f64,
            back.function_count(),
            out.display()
        );
    } else {
        println!(
            "{name}: PASS 源码 {} B → 字节流 {} B（{:.2}×，{} 函数）  \
             {parse_mode} {parse_us:.0}µs / encode {encode_us:.0}µs / decode {decode_us:.0}µs",
            src.len(),
            bytes.len(),
            bytes.len() as f64 / src.len().max(1) as f64,
            back.function_count(),
        );
    }

    Ok(Stats {
        name,
        src_bytes: src.len(),
        ir_bytes: bytes.len(),
        cache_hit,
    })
}
