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

use std::path::Path;

#[cfg(feature = "plugins")]
use {
    forge_codegen::machine::target::ErasedTargetMachine,
    forge_codegen::target_registry,
    forge_codegen::{CompiledFunction, IrError, Registry},
    libloading::Library,
    std::ffi::CStr,
    std::os::raw::c_char,
    std::sync::Arc,
};

/// 插件侧编译入口的函数指针类型（`export_isa_plugin!` 生成）。
#[allow(improper_ctypes_definitions)]
#[cfg(feature = "plugins")]
type PluginCompile =
    unsafe extern "C" fn(&forge_codegen::ir::Function) -> Result<CompiledFunction, IrError>;

/// 宿主侧 `ErasedTargetMachine` 适配器：把插件导出的函数包成 Registry 后端。
#[cfg(feature = "plugins")]
struct PluginBackend {
    name: String,
    compile: PluginCompile,
}

#[cfg(feature = "plugins")]
impl ErasedTargetMachine for PluginBackend {
    fn erased_name(&self) -> &str {
        &self.name
    }

    fn compile(&self, func: &forge_codegen::ir::Function) -> Result<CompiledFunction, IrError> {
        // SAFETY：函数指针来自已加载并保持存活的插件库；ABI 由
        // `export_isa_plugin!` 固定（同一 rustc/forge 构建）。
        unsafe { (self.compile)(func) }
    }
}

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
            if let Some(ext) = path.extension().and_then(|e| e.to_str())
                && matches!(ext, "dll" | "so" | "dylib")
            {
                let _ = self.load_plugin(&path);
            }
        }
    }

    #[cfg(not(feature = "plugins"))]
    fn load_directory(&mut self, _dir: &Path) {}

    /// 加载单个插件文件（v2：`__forge_isa_compile` / `__forge_isa_name` /
    /// `__forge_isa_targets`；v1 `__codegen_isa_register` 仍兼容）。
    #[cfg(feature = "plugins")]
    pub fn load_plugin(&mut self, path: &Path) -> Result<(), String> {
        let lib = unsafe {
            Library::new(path)
                .map_err(|e| format!("failed to load plugin '{}': {}", path.display(), e))?
        };

        // v2：宿主侧注册适配器 + 目标前缀；不调用插件内部的 Registry（那个
        // static 在 dylib 自己那份 runtime 里，宿主看不见）。
        if let Ok(compile) = unsafe { lib.get::<PluginCompile>(b"__forge_isa_compile") } {
            let name_fn =
                unsafe { lib.get::<unsafe extern "C" fn() -> *const c_char>(b"__forge_isa_name") }
                    .map_err(|e| {
                        format!("plugin '{}' missing __forge_isa_name: {e}", path.display())
                    })?;
            let raw = unsafe { name_fn() };
            if raw.is_null() {
                return Err(format!(
                    "plugin '{}' __forge_isa_name 返回空指针",
                    path.display()
                ));
            }
            let name = unsafe { CStr::from_ptr(raw) }
                .to_string_lossy()
                .into_owned();
            let mut targets = Vec::new();
            if let Ok(tf) = unsafe {
                lib.get::<unsafe extern "C" fn() -> *const c_char>(b"__forge_isa_targets")
            } {
                let raw = unsafe { tf() };
                if !raw.is_null() {
                    let s = unsafe { CStr::from_ptr(raw) }.to_string_lossy();
                    targets = s
                        .split(';')
                        .map(str::trim)
                        .filter(|x| !x.is_empty())
                        .map(str::to_string)
                        .collect();
                }
            }
            Registry::global().register_backend(Arc::new(PluginBackend {
                name: name.clone(),
                compile: *compile,
            }));
            let refs: Vec<&str> = targets.iter().map(String::as_str).collect();
            target_registry::register(&name, &refs);
            log::info!("Loaded ISA plugin v2: {} ({name})", path.display());
            self.libraries.push(lib);
            return Ok(());
        }

        // v1 兼容：插件自己接收 `&Registry` 并注册（旧文档写法）。
        let register: libloading::Symbol<fn(&Registry)> = unsafe {
            lib.get(b"__codegen_isa_register").map_err(|e| {
                format!(
                    "plugin '{}' missing __forge_isa_compile (v2) or __codegen_isa_register (v1): {e}",
                    path.display()
                )
            })?
        };
        register(Registry::global());
        log::info!("Loaded ISA plugin v1: {}", path.display());
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

    #[cfg(feature = "plugins")]
    #[test]
    fn test_empty_loader() {
        let loader = PluginLoader::new();
        assert!(loader.libraries.is_empty());
    }

    #[test]
    fn test_load_from_env_not_set() {
        // Ensure CODEGEN_ISA_PLUGINS is not set
        unsafe { std::env::remove_var("CODEGEN_ISA_PLUGINS") };
        // Should return empty loader without error
        let _loader = PluginLoader::load_from_env();
    }
}
