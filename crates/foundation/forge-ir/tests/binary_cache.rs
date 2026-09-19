//! 文件级 IR 缓存（`IrCache`）：内容键、自愈、命中即跳过解析。
//!
//! 这层是**加速层而非事实源**：任何读失败/损坏/版本不符都当 miss，删掉整个目录只损失
//! 速度。因此这里的断言分三类：
//!
//! 1. **命中语义**：同源码命中且与原件逐字节一致；不同源码绝不命中；
//! 2. **自愈**：坏条目当 miss，`get_or_insert_with` 重算并覆盖后恢复命中；
//! 3. **不缓存失败**：`build` 返回 `None` 时不落盘（否则会把"解析失败"永久钉住）。

use std::fs;
use std::path::PathBuf;

use forge_ir::text::parser::parse_module;
use forge_ir::{CacheKey, IrCache, Module};

/// 每个用例独占一个目录（`CARGO_TARGET_TMPDIR` 是 cargo 给测试目标的临时目录）。
fn tmp_dir(name: &str) -> PathBuf {
    let dir = PathBuf::from(env!("CARGO_TARGET_TMPDIR")).join(name);
    let _ = fs::remove_dir_all(&dir);
    dir
}

fn cache(name: &str) -> IrCache {
    IrCache::new(tmp_dir(name)).expect("建缓存目录")
}

fn module() -> Module {
    parse_module("define void @f() {\nentry:\n  ret void\n}\n").expect("解析夹具")
}

#[test]
fn key_depends_on_content_not_path() {
    let a = CacheKey::of_source("define void @f() {}");
    let b = CacheKey::of_source("define void @f() {}");
    let c = CacheKey::of_source("define void @f() { }");
    assert_eq!(a, b, "同样的源码必须同键");
    assert_ne!(a, c, "差一个字符必须换键");
    assert_eq!(a.as_str().len(), 32, "键 = 128 位十六进制");
    assert!(a.as_str().chars().all(|c| c.is_ascii_hexdigit()));
}

#[test]
fn roundtrip_hit_and_self_healing() {
    let cache = cache("ir_cache_roundtrip");
    let source = "define i32 @g(i32 %x) {\nentry:\n  %y = add i32 %x, 7\n  ret i32 %y\n}\n";
    let m = module();
    assert!(cache.load(source).is_none(), "空缓存必须 miss");
    cache.store(source, &m).expect("落盘");
    assert_eq!(cache.entry_count(), 1);
    let hit = cache.load(source).expect("同源码必须命中");
    assert_eq!(hit.to_binary(), m.to_binary(), "命中的模块与原件逐字节一致");
    // 不同源码不命中（不会"张冠李戴"）
    assert!(
        cache
            .load("define void @other() {\nentry:\n  ret void\n}\n")
            .is_none()
    );

    // 自愈：条目写坏 ⇒ miss；get_or_insert_with 重算并覆盖 ⇒ 再命中
    let path = cache.entry_path(source);
    fs::write(&path, b"not a forgeir stream").expect("写坏条目");
    assert!(cache.load(source).is_none(), "坏条目必须当 miss");
    let rebuilt = cache
        .get_or_insert_with(source, || Some(module()))
        .expect("重算");
    assert_eq!(rebuilt.to_binary(), m.to_binary());
    assert_eq!(
        cache.load(source).expect("自愈后必须命中").to_binary(),
        m.to_binary()
    );
    assert_eq!(
        cache.entry_count(),
        1,
        "自愈不留垃圾条目（临时文件不算条目）"
    );
}

#[test]
fn get_or_insert_with_builds_only_on_miss() {
    let cache = cache("ir_cache_build_count");
    let source = "define void @h() {\nentry:\n  ret void\n}\n";
    let mut builds = 0;
    let first = cache
        .get_or_insert_with(source, || {
            builds += 1;
            Some(module())
        })
        .expect("首次构建");
    assert_eq!(builds, 1, "首次必须构建一次");
    let second = cache
        .get_or_insert_with(source, || {
            builds += 1;
            Some(module())
        })
        .expect("命中");
    assert_eq!(builds, 1, "命中时不得再调用 build");
    assert_eq!(first.to_binary(), second.to_binary());

    // build 失败（返回 None）不得落盘，也不得被缓存
    let bad = "define void @never() {\nentry:\n  ret void\n}\n";
    assert!(cache.get_or_insert_with(bad, || None).is_none());
    assert_eq!(cache.entry_count(), 1, "失败结果不缓存");
}

#[test]
fn entry_path_is_stable() {
    let cache = cache("ir_cache_paths");
    let source = "define void @k() {\nentry:\n  ret void\n}\n";
    assert_eq!(
        cache.entry_path(source),
        cache.entry_path(source),
        "同源码的条目路径必须稳定"
    );
    assert_eq!(
        cache
            .entry_path(source)
            .extension()
            .and_then(|x| x.to_str()),
        Some("fir")
    );
}
