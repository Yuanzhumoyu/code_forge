//! 动态插件声明的 rustc 目标前缀（v28 M3）。
//!
//! 插件 .dll/.so 加载后调用 `forge_codegen::export_isa_plugin!` 生成的
//! `__codegen_isa_register_v2`；该函数把 `(ISA 名, 目标前缀)` 登记到这里。
//! `forge-rustc` 的 `isa_name_for_target` 先查 manifest，再查本表（进程级、幂等）。
//!
//! 为什么住在 runtime：forge-codegen 与 forge-plugin 都依赖它，放这里避免循环依赖；
//! 表本身只是字符串，无编译管线依赖。

use std::sync::{OnceLock, RwLock};

fn table() -> &'static RwLock<Vec<(String, String)>> {
    static T: OnceLock<RwLock<Vec<(String, String)>>> = OnceLock::new();
    T.get_or_init(|| RwLock::new(Vec::new()))
}

/// 登记一组目标前缀；重复 (isa,target) 幂等。
pub fn register(isa: &str, targets: &[&str]) {
    let mut t = table().write().expect("target registry poisoned");
    for target in targets {
        let pair = (isa.to_string(), target.to_string());
        if !t.contains(&pair) {
            t.push(pair);
        }
    }
}

/// 目标三元组 → 插件 ISA 名（按前缀匹配；未命中 = None）。
pub fn isa_for_target(triple: &str) -> Option<String> {
    let t = table().read().expect("target registry poisoned");
    t.iter()
        .find(|(_, prefix)| triple.starts_with(prefix.as_str()))
        .map(|(isa, _)| isa.clone())
}

/// 已登记条目（测试/诊断用）。
pub fn entries() -> Vec<(String, String)> {
    table().read().expect("target registry poisoned").clone()
}

/// 清空（测试用；生产进程不应调用）。
pub fn clear_for_tests() {
    table().write().expect("target registry poisoned").clear();
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn register_and_lookup() {
        clear_for_tests();
        register("myisa", &["myisa64", "myisa32"]);
        assert_eq!(isa_for_target("myisa64-unknown-none"), Some("myisa".into()));
        assert_eq!(isa_for_target("myisa32-unknown-none"), Some("myisa".into()));
        assert_eq!(isa_for_target("x86_64-pc-windows-msvc"), None);
        register("myisa", &["myisa64"]);
        assert_eq!(entries().len(), 2, "重复登记必须幂等");
        clear_for_tests();
    }
}
