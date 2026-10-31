//! 动态 ISA 插件加载器。
//!
//! 通过环境变量 `CODEGEN_ISA_PLUGINS` 加载外部编译的 ISA 后端 `.dll`/`.so` 文件。
//! 推荐 **v2**：插件导出 `__forge_isa_compile` / `__forge_isa_name` /
//! `__forge_isa_targets`（由 `forge_codegen::export_isa_plugin!` 一行生成）。
//! 加载器把编译入口包成宿主侧 `ErasedTargetMachine` 注册进 Registry，并登记目标前缀；
//! **宿主不需要任何 ISA 代码**。
//!
//! v1（`__codegen_isa_register(&Registry)`）仍兼容，但需要插件自己完成注册。
//!
//! # 环境变量
//! - `CODEGEN_ISA_PLUGINS` — 分号 (Windows) 或冒号 (Unix) 分隔的路径列表或目录
//!
//! # 插件规范（v2）
//! ```ignore
//! // Cargo.toml: crate-type = ["dylib"]；依赖 forge-codegen + forge-isa-runtime
//! forge_dsl::isa_from_file!("isa/my.toml", name = "myisa");
//! forge_codegen::export_isa_plugin!(myisa, targets = ["myisa64"]);
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

/// 插件侧编译入口的函数指针类型（原始字节 C ABI）。
#[allow(improper_ctypes_definitions)]
#[cfg(feature = "plugins")]
type PluginCompile = unsafe extern "C" fn(&forge_codegen::ir::Function, *mut u8, usize) -> isize;
#[cfg(feature = "plugins")]
type PluginCompileRequired = unsafe extern "C" fn(&forge_codegen::ir::Function) -> isize;
#[cfg(feature = "plugins")]
type PluginRelocRequired = unsafe extern "C" fn(&forge_codegen::ir::Function) -> isize;
#[cfg(feature = "plugins")]
type PluginRelocCopy = unsafe extern "C" fn(&forge_codegen::ir::Function, *mut u8, usize) -> isize;
#[cfg(feature = "plugins")]
type PluginAbiVersion = unsafe extern "C" fn() -> u32;
#[cfg(feature = "plugins")]
type PluginLastError = unsafe extern "C" fn() -> *const c_char;

/// 宿主侧 `ErasedTargetMachine` 适配器：把插件导出的函数包成 Registry 后端。
#[cfg(feature = "plugins")]
struct PluginBackend {
    name: String,
    compile: PluginCompile,
    required: PluginCompileRequired,
    reloc_required: PluginRelocRequired,
    reloc_copy: PluginRelocCopy,
    last_error: Option<PluginLastError>,
}

#[cfg(feature = "plugins")]
impl PluginBackend {
    fn error_text(&self) -> String {
        let Some(f) = self.last_error else {
            return String::new();
        };
        let p = unsafe { f() };
        if p.is_null() {
            return String::new();
        }
        unsafe { CStr::from_ptr(p) }.to_string_lossy().into_owned()
    }
}

#[cfg(feature = "plugins")]
impl ErasedTargetMachine for PluginBackend {
    fn erased_name(&self) -> &str {
        &self.name
    }

    fn compile(&self, func: &forge_codegen::ir::Function) -> Result<CompiledFunction, IrError> {
        // 先问精确大小，再让插件只拷机器码——避免固定 1 MiB，也避免
        // Vec/String 跨 dylib 由错误分配器释放。
        let need = unsafe { (self.required)(func) };
        if need < 0 {
            let detail = self.error_text();
            return Err(IrError::Unsupported(format!(
                "[cap:PLUGIN_COMPILE] 插件 `{}` 编译失败{}{}",
                self.name,
                if detail.is_empty() { "" } else { "：" },
                detail
            )));
        }
        let mut buf = vec![0u8; need as usize];
        let n = unsafe { (self.compile)(func, buf.as_mut_ptr(), buf.len()) };
        if n < 0 {
            let detail = self.error_text();
            return Err(IrError::Unsupported(format!(
                "[cap:PLUGIN_COMPILE] 插件 `{}` 拷贝机器码失败（code {n}）{}{}",
                self.name,
                if detail.is_empty() { "" } else { "：" },
                detail
            )));
        }
        buf.truncate(n as usize);
        if buf.len() != need as usize {
            return Err(IrError::Unsupported(format!(
                "[cap:PLUGIN_COMPILE] 插件 `{}` 返回长度不一致（{} != {need}）",
                self.name,
                buf.len()
            )));
        }

        // 重定位单独传：链接/JIT 需要符号名与 addend，不能丢。
        let reloc_need = unsafe { (self.reloc_required)(func) };
        if reloc_need < 0 {
            let detail = self.error_text();
            return Err(IrError::Unsupported(format!(
                "[cap:PLUGIN_COMPILE] 插件 `{}` 重定位编码失败{}{}",
                self.name,
                if detail.is_empty() { "" } else { "：" },
                detail
            )));
        }
        let mut rbuf = vec![0u8; reloc_need as usize];
        let rn = unsafe { (self.reloc_copy)(func, rbuf.as_mut_ptr(), rbuf.len()) };
        if rn < 0 {
            return Err(IrError::Unsupported(format!(
                "[cap:PLUGIN_COMPILE] 插件 `{}` 重定位拷贝失败（code {rn}）",
                self.name
            )));
        }
        rbuf.truncate(rn as usize);
        let relocations = forge_codegen::plugin_abi::decode_relocations(&rbuf)
            .map_err(|e| IrError::Unsupported(format!("[cap:PLUGIN_COMPILE] {e}")))?;

        Ok(CompiledFunction {
            code_size: n as usize,
            code: buf,
            relocations,
            line_entries: Vec::new(),
            cfi: None,
        })
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
        if let Ok(compile) = unsafe { lib.get::<PluginCompile>(b"__forge_isa_compile_raw") } {
            let version = unsafe {
                lib.get::<PluginAbiVersion>(b"__forge_plugin_abi_version")
                    .map_err(|e| {
                        format!(
                            "plugin '{}' missing __forge_plugin_abi_version（需重新用当前 forge 构建）: {e}",
                            path.display()
                        )
                    })?
            };
            if unsafe { version() } != forge_codegen::PLUGIN_ABI_VERSION {
                return Err(format!(
                    "plugin '{}' ABI 版本不匹配：插件 {}，宿主 {}",
                    path.display(),
                    unsafe { version() },
                    forge_codegen::PLUGIN_ABI_VERSION
                ));
            }
            let required = unsafe {
                lib.get::<PluginCompileRequired>(b"__forge_isa_compile_required")
                    .map_err(|e| {
                        format!(
                            "plugin '{}' missing __forge_isa_compile_required: {e}",
                            path.display()
                        )
                    })?
            };
            let reloc_required = unsafe {
                lib.get::<PluginRelocRequired>(b"__forge_isa_reloc_required")
                    .map_err(|e| {
                        format!(
                            "plugin '{}' missing __forge_isa_reloc_required: {e}",
                            path.display()
                        )
                    })?
            };
            let reloc_copy = unsafe {
                lib.get::<PluginRelocCopy>(b"__forge_isa_reloc_copy")
                    .map_err(|e| {
                        format!(
                            "plugin '{}' missing __forge_isa_reloc_copy: {e}",
                            path.display()
                        )
                    })?
            };
            let last_error =
                unsafe { lib.get::<PluginLastError>(b"__forge_isa_last_error").ok() }.map(|s| *s);
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
                required: *required,
                reloc_required: *reloc_required,
                reloc_copy: *reloc_copy,
                last_error,
            }));
            let refs: Vec<&str> = targets.iter().map(String::as_str).collect();
            target_registry::register(&name, &refs);

            // 先在插件自己的 registry 里注册 ABI 数据（编译入口在 dylib 内执行，
            // 看不到宿主 static）；再在宿主注册一份，供宿主侧 plan/诊断使用。
            if let Ok(init) = unsafe { lib.get::<unsafe extern "C" fn()>(b"__forge_isa_init_abi") }
            {
                unsafe { init() };
            }
            for (sym, what, register) in [
                (
                    b"__forge_isa_abi_rules".as_slice(),
                    "ABI rules",
                    forge_codegen::pipeline::conv_registry::register_rules_toml
                        as fn(&str) -> Result<(), String>,
                ),
                (
                    b"__forge_isa_abi_binding".as_slice(),
                    "ABI binding",
                    forge_codegen::pipeline::conv_registry::register_binding_toml
                        as fn(&str) -> Result<(), String>,
                ),
            ] {
                let Ok(f) = (unsafe { lib.get::<unsafe extern "C" fn() -> *const c_char>(sym) })
                else {
                    continue;
                };
                let raw = unsafe { f() };
                if raw.is_null() {
                    continue;
                }
                let text = unsafe { CStr::from_ptr(raw) }.to_string_lossy();
                if text.trim().is_empty() {
                    continue;
                }
                register(&text)
                    .map_err(|e| format!("plugin '{}' {what} 注册失败：{e}", path.display()))?;
            }
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
