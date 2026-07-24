//! 动态 ISA 插件加载器。
//!
//! 通过环境变量 `CODEGEN_ISA_PLUGINS` 加载外部编译的 ISA 后端 `.dll`/`.so` 文件。
//! 每个插件必须导出 `__codegen_isa_register` 符号，接受 `&Registry` 参数。
//!
//! # 环境变量
//! - `CODEGEN_ISA_PLUGINS` — 分号 (Windows) 或冒号 (Unix) 分隔的路径列表或目录
//!
//! # 插件规范
//! ```ignore
//! // Cargo.toml: crate-type = ["dylib"]
//! #[no_mangle]
//! pub fn __codegen_isa_register(registry: &codegen_lib::Registry) {
//!     registry.register(Arc::new(IsaCompiler::<MyIsa>::new()));
//! }
//! ```

use crate::Registry;
use std::path::Path;

#[cfg(feature = "plugins")]
use libloading::Library;

/// 动态插件加载器 — 持有已加载的 Library 句柄，防止提前卸载。
pub struct PluginLoader {
    #[cfg(feature = "plugins")]
    libraries: Vec<Library>,
}

impl PluginLoader {
    /// 创建空的加载器。
    pub fn new() -> Self {
        Self {
            #[cfg(feature = "plugins")]
            libraries: Vec::new(),
        }
    }

    /// 从 `CODEGEN_ISA_PLUGINS` 环境变量加载插件。
    ///
    /// 支持的值格式：
    /// - 单个文件路径：`/path/to/plugin.dll`
    /// - 分号分隔的多个路径：`/path/a.dll;/path/b.so`
    /// - 目录路径：扫描目录下所有 `.dll`/`.so`/`.dylib` 文件
    ///
    /// 未设置环境变量时返回空加载器（无操作）。
    pub fn load_from_env() -> Self {
        let mut loader = Self::new();

        let env_val = match std::env::var("CODEGEN_ISA_PLUGINS") {
            Ok(v) => v,
            Err(_) => return loader,
        };

        // Windows 用分号，Unix 用冒号分隔
        let separator = if cfg!(windows) { ';' } else { ':' };

        for entry in env_val.split(separator) {
            let entry = entry.trim();
            if entry.is_empty() {
                continue;
            }

            let path = Path::new(entry);
            if path.is_dir() {
                loader.load_directory(path);
            } else {
                let _ = loader.load_plugin(path);
            }
        }

        loader
    }

    /// 扫描目录并加载所有匹配的插件文件。
    #[cfg(feature = "plugins")]
    fn load_directory(&mut self, dir: &Path) {
        let Ok(entries) = std::fs::read_dir(dir) else {
            return;
        };
        for entry in entries.filter_map(|e| e.ok()) {
            let path = entry.path();
            if let Some(ext) = path.extension().and_then(|e| e.to_str()) {
                if matches!(ext, "dll" | "so" | "dylib") {
                    let _ = self.load_plugin(&path);
                }
            }
        }
    }

    #[cfg(not(feature = "plugins"))]
    fn load_directory(&mut self, _dir: &Path) {}

    /// 加载单个插件文件并调用其 `__codegen_isa_register` 符号。
    #[cfg(feature = "plugins")]
    pub fn load_plugin(&mut self, path: &Path) -> Result<(), String> {
        type RegisterFn = fn(&Registry);

        let lib = unsafe {
            Library::new(path)
                .map_err(|e| format!("failed to load plugin '{}': {}", path.display(), e))?
        };

        let register: libloading::Symbol<RegisterFn> = unsafe {
            lib.get(b"__codegen_isa_register").map_err(|e| {
                format!(
                    "plugin '{}' missing __codegen_isa_register: {}",
                    path.display(),
                    e
                )
            })?
        };

        let registry = Registry::global();
        register(registry);

        log::info!("Loaded ISA plugin: {}", path.display());

        // 保持 Library 存活 — 插件中的代码（如 Arc<dyn Compiler>）依赖它
        self.libraries.push(lib);
        Ok(())
    }

    #[cfg(not(feature = "plugins"))]
    pub fn load_plugin(&mut self, _path: &Path) -> Result<(), String> {
        Err("plugin support not enabled (compile with --features plugins)".into())
    }
}

impl Default for PluginLoader {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_empty_loader() {
        let loader = PluginLoader::new();
        assert!(loader.libraries.is_empty());
    }

    #[test]
    fn test_load_from_env_not_set() {
        // Ensure CODEGEN_ISA_PLUGINS is not set
        unsafe { std::env::remove_var("CODEGEN_ISA_PLUGINS") };
        let loader = PluginLoader::load_from_env();
        // Should return empty loader without error
        drop(loader);
    }
}
