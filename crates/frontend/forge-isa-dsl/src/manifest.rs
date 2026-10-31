//! ISA 注册清单（v28 M2）：新 ISA 只加 TOML + 一条 manifest，不改 Rust。
//!
//! `isa/manifest.toml` 是**发行后端清单**：每个 `[[isa]]` 描述一份谱、它的模块名、
//! 可选部件、它服务的 rustc 目标三元组前缀，以及是否注入 alloc shim。
//!
//! `forge-codegen/build.rs` 读它，做两件事：
//!
//! 1. 为每条调 [`forge_isa_dsl::gen_file::pregenerate`] 预生成 `$OUT_DIR` 落盘件；
//! 2. 生成 `arch_registry.rs`（模块 include + 目标表 + `ensure_all`）与
//!    `backend_hooks.rs`（管线工厂/调用点钩子的注册）。
//!
//! 这样新增 ISA 只需要改数据，不再动 `arch/*.rs`、`pipeline_hooks.rs`、`compile.rs`。

use crate::gen_file::MacroArgs;
use crate::{ExpandOptions, Parts};
use serde::{Deserialize, Serialize};
use std::path::Path;

/// `isa/manifest.toml` 的根。
#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct Manifest {
    #[serde(default)]
    pub isa: Vec<ManifestEntry>,
}

/// 单条发行后端。
#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct ManifestEntry {
    /// ISA 注册名（`[meta].name`，必须与谱一致）。
    pub name: String,
    /// Rust 模块名（`arch::<module>::<module>`）。
    pub module: String,
    /// 谱路径，相对 workspace 根（`isa/x86.toml`）。
    pub spec: String,
    /// 可选部件名（`encode`/`decode`/`asm`/`tm`）；省略 = 全开。
    #[serde(default)]
    pub parts: Option<Vec<String>>,
    /// rustc 目标三元组**前缀**（`x86_64`、`aarch64`…）。
    #[serde(default)]
    pub targets: Vec<String>,
    /// 是否允许 rustc 后端注入 alloc/mem shim（Windows x64 运行时）。
    #[serde(default)]
    pub alloc_shim: bool,
}

impl ManifestEntry {
    /// 构造 `isa_from_file!` 的等价参数（build script 预生成与生成物宏必须同源）。
    pub fn macro_args(&self) -> Result<MacroArgs, String> {
        if self.name.trim().is_empty() {
            return Err("[[isa]].name 不能为空".into());
        }
        if self.module.trim().is_empty() {
            return Err(format!("[[isa.{}]].module 不能为空", self.name));
        }
        if self.spec.trim().is_empty() {
            return Err(format!("[[isa.{}]].spec 不能为空", self.name));
        }
        let mut opts = ExpandOptions::new();
        opts.name = Some(self.module.clone());
        if let Some(parts) = &self.parts {
            opts.parts = Parts::from_names(parts)
                .map_err(|e| format!("[[isa.{}]].parts: {e}", self.name))?;
        }
        Ok(MacroArgs {
            path: self.spec.clone(),
            opts,
        })
    }

    /// 是否生成 TargetMachine 集成层（`parts` 省略 = 全开）。
    pub fn has_tm(&self) -> bool {
        self.parts
            .as_ref()
            .is_none_or(|p| p.iter().any(|x| x == "tm"))
    }

    /// alloc shim 的架构键（rustc `Arch` 的 Debug 小写）；目前只有 x86_64。
    pub fn alloc_shim_arch(&self) -> String {
        match self.targets.first().map(String::as_str) {
            Some("amd64") => "x86_64".to_string(),
            Some(t) => t.to_string(),
            None => self.name.clone(),
        }
    }

    /// `parts` 的 TOML 数组文本（写进生成物；省略 = 不写）。
    pub fn parts_toml(&self) -> Option<String> {
        self.parts.as_ref().map(|p| {
            let items: Vec<String> = p.iter().map(|x| format!("\"{x}\"")).collect();
            format!(", parts = [{}]", items.join(", "))
        })
    }
}

/// 生成 `arch_registry.rs` 的文本（build.rs 落盘；测试可直接断言）。
pub fn render_arch_registry(m: &Manifest) -> String {
    let mut s = String::from(
        "// 由 build.rs 从 isa/manifest.toml 生成（v28 M2）——请勿手工编辑。\n\
         // 新增 ISA 只改 isa/manifest.toml。\n\n",
    );
    for e in &m.isa {
        let parts = e.parts_toml().unwrap_or_default();
        s.push_str(&format!(
            "#[allow(clippy::module_inception)]\npub mod {m} {{\n    forge_dsl::isa_from_file!(\"{spec}\", name = \"{m}\"{parts});\n    pub use self::{m}::*;\n}}\n",
            m = e.module,
            spec = e.spec,
            parts = parts
        ));
    }
    s.push_str("\n/// (rustc 目标三元组前缀, ISA 注册名)。\n");
    s.push_str("pub const TARGETS: &[(&str, &str)] = &[\n");
    for e in &m.isa {
        for t in &e.targets {
            s.push_str(&format!("    (\"{t}\", \"{}\"),\n", e.name));
        }
    }
    s.push_str("];\n\n");
    s.push_str(
        "/// 目标三元组 → ISA 名（按 manifest 前缀匹配；未命中 = None，不回落）。\n\
         pub fn isa_for_target(triple: &str) -> Option<&'static str> {\n\
         \x20   TARGETS\n\
         \x20       .iter()\n\
         \x20       .find(|(prefix, _)| triple.starts_with(prefix))\n\
         \x20       .map(|(_, name)| *name)\n\
         }\n\n",
    );
    s.push_str("/// 注册全部发行后端的 ISA 数据（Registry；幂等）。\n");
    s.push_str("pub fn ensure_all() {\n");
    for e in &m.isa {
        s.push_str(&format!("    {}::ensure_registered();\n", e.module));
    }
    s.push_str("}\n\n");
    s.push_str(
        "/// 该架构是否有 alloc/mem shim（Windows x64 运行时）。\n\
         pub fn has_alloc_shim(arch: &str) -> bool {\n",
    );
    let shims: Vec<String> = m
        .isa
        .iter()
        .filter(|e| e.alloc_shim)
        .map(|e| format!("\"{}\"", e.alloc_shim_arch()))
        .collect();
    s.push_str(&format!("    [{}].contains(&arch)\n}}\n", shims.join(", ")));
    s
}

/// 生成 `backend_hooks.rs` 的文本（build.rs 落盘；测试可直接断言）。
pub fn render_backend_hooks(m: &Manifest) -> String {
    let mut s = String::from(
        "// 由 build.rs 从 isa/manifest.toml 生成（v28 M2）——请勿手工编辑。\n\
         // 每个带 tm 部件的后端：管线工厂 + 调用点布局钩子。\n\n",
    );
    let tm_entries: Vec<&ManifestEntry> = m.isa.iter().filter(|e| e.has_tm()).collect();
    for e in &tm_entries {
        s.push_str(&format!(
            "register_backend!({m}_factory, crate::arch::{m}::TargetMachine, \"{name}\");\n",
            m = e.module,
            name = e.name
        ));
    }
    s.push_str("\npub(crate) fn register_generated_backends() {\n");
    for e in &tm_entries {
        s.push_str(&format!(
            "    let f: &'static forge_isa_runtime::PipelineFactory =\n\
             \x20       Box::leak(Box::new(|tm: &dyn Any| {m}_factory(tm)));\n\
             \x20   forge_isa_runtime::register_pipeline(\"{name}\", f);\n\
             \x20   crate::pipeline::abi_target::register_isa_call_planner(\n\
             \x20       \"{name}\",\n\
             \x20       crate::arch::{m}::TargetMachine::new(),\n\
             \x20   );\n",
            m = e.module,
            name = e.name
        ));
    }
    s.push_str("}\n");
    s
}

/// 读并校验清单；路径错误、未知键、重复名/模块/目标都报错。
pub fn load(path: &Path) -> Result<Manifest, String> {
    let text = std::fs::read_to_string(path)
        .map_err(|e| format!("读 manifest {} 失败：{e}", path.display()))?;
    let m: Manifest =
        toml::from_str(&text).map_err(|e| format!("manifest {} TOML 错误：{e}", path.display()))?;
    validate(&m)?;
    Ok(m)
}

/// 结构校验（与 `deny_unknown_fields` 互补的语义检查）。
pub fn validate(m: &Manifest) -> Result<(), String> {
    if m.isa.is_empty() {
        return Err("manifest 里没有任何 [[isa]]".into());
    }
    let mut names = std::collections::BTreeSet::new();
    let mut modules = std::collections::BTreeSet::new();
    let mut targets = std::collections::BTreeSet::new();
    for e in &m.isa {
        if !names.insert(e.name.clone()) {
            return Err(format!("manifest 重复的 ISA 名 `{}`", e.name));
        }
        if !modules.insert(e.module.clone()) {
            return Err(format!("manifest 重复的模块名 `{}`", e.module));
        }
        // 构造一次参数，顺带校验 spec/parts。
        e.macro_args()?;
        for t in &e.targets {
            if t.trim().is_empty() {
                return Err(format!("[[isa.{}]].targets 里有空串", e.name));
            }
            if !targets.insert(t.clone()) {
                return Err(format!("manifest 目标前缀 `{t}` 被多条 ISA 声明"));
            }
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample() -> Manifest {
        Manifest {
            isa: vec![
                ManifestEntry {
                    name: "a".into(),
                    module: "a".into(),
                    spec: "isa/a.toml".into(),
                    parts: None,
                    targets: vec!["a64".into()],
                    alloc_shim: false,
                },
                ManifestEntry {
                    name: "b".into(),
                    module: "b".into(),
                    spec: "isa/b.toml".into(),
                    parts: Some(vec!["encode".into(), "asm".into()]),
                    targets: vec!["b64".into()],
                    alloc_shim: true,
                },
            ],
        }
    }

    #[test]
    fn manifest_roundtrip_and_options() {
        let m = sample();
        let text = toml::to_string(&m).expect("serialize");
        let back: Manifest = toml::from_str(&text).expect("parse");
        validate(&back).expect("valid");
        assert_eq!(back.isa[1].parts.as_ref().unwrap().len(), 2);
        let args = back.isa[1].macro_args().unwrap();
        assert_eq!(args.opts.name.as_deref(), Some("b"));
        assert!(args.opts.parts.encode && args.opts.parts.asm && !args.opts.parts.tm);
    }

    #[test]
    fn duplicate_names_and_targets_are_rejected() {
        let mut m = sample();
        m.isa[1].name = "a".into();
        assert!(validate(&m).unwrap_err().contains("重复的 ISA 名"));
        let mut m = sample();
        m.isa[1].targets = vec!["a64".into()];
        assert!(validate(&m).unwrap_err().contains("目标前缀"));
    }

    #[test]
    fn bad_parts_are_rejected() {
        let mut m = sample();
        m.isa[0].parts = Some(vec!["vm".into()]);
        assert!(validate(&m).unwrap_err().contains("未知部件"));
    }

    /// M2 的核心：新增一条 manifest 条目就出现在生成注册表里，不需要改 Rust。
    #[test]
    fn registry_renders_any_manifest_entry() {
        let mut m = sample();
        m.isa.push(ManifestEntry {
            name: "c".into(),
            module: "c".into(),
            spec: "isa/c.toml".into(),
            parts: None,
            targets: vec!["c32".into()],
            alloc_shim: true,
        });
        let reg = render_arch_registry(&m);
        assert!(reg.contains("pub mod c {"));
        assert!(reg.contains("(\"c32\", \"c\")"));
        assert!(reg.contains("    c::ensure_registered();"));
        assert!(reg.contains("has_alloc_shim"));
        assert!(reg.contains("\"c32\""));
        let hooks = render_backend_hooks(&m);
        assert!(
            hooks.contains("register_backend!(c_factory, crate::arch::c::TargetMachine, \"c\")")
        );
        assert!(hooks.contains("register_pipeline(\"c\", f)"));
    }

    /// 仓库里真实的 manifest：每条谱都能按 manifest 参数预生成（数据驱动接入的实证）。
    #[test]
    fn shipped_manifest_entries_pregenerate() {
        let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../..");
        let m = load(&root.join("isa/manifest.toml")).expect("load shipped manifest");
        let out = root.join("target/tmp/manifest_pregenerate");
        let _ = std::fs::remove_dir_all(&out);
        std::fs::create_dir_all(&out).expect("temp out");
        for e in &m.isa {
            let args = e.macro_args().expect("macro args");
            crate::gen_file::pregenerate(&args, &out)
                .unwrap_or_else(|err| panic!("pregenerate {}: {err}", e.name));
        }
        let n = std::fs::read_dir(&out).unwrap().count();
        assert!(n >= m.isa.len(), "每条 manifest 都应落盘一份生成物");
    }
}
