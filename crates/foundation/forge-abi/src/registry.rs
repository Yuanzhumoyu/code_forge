//! 约定注册表 + 宿主钩子。
//!
//! **约定是使用者的数据**：注册表把 `约定名 → AbiRules`、`(ISA, 约定名) → AbiBinding`
//! 与可选的宿主钩子放在一起；引擎不经注册表就不认任何约定名（未注册 ⇒ 明确报错，
//! 绝不静默退回 `Default`——旧设计里 IR 的 `CallConv::Default` 就是这么变成死值的）。

use std::collections::BTreeMap;

use crate::binding::AbiBinding;
use crate::engine::{AbiTarget, ClassDir, Signature, plan_fn};
use crate::error::AbiError;
use crate::plan::AbiPlan;
use crate::rules::{AbiRules, ClassAction};
use crate::ty::TyView;

/// 宿主钩子：数据表达不了的**语言专属**约定（Swift `self`/`error`、Go context/GC、
/// 自定义分类…）。默认实现什么都不做——引擎只用数据也能工作。
pub trait AbiHooks: Send + Sync {
    /// 覆盖分类（返回 `Some` 即接管；`None` = 继续用规则表）。
    ///
    /// `dir` 是分类方向：同一个类型在**参数位**与**返回位**的落点可以不同
    /// （AAPCS64 的 >16B 聚合：参数 byval、返回 x8 sret），钩子必须能区分。
    fn classify(&self, _rules: &AbiRules, _ty: &TyView, _dir: ClassDir) -> Option<ClassAction> {
        None
    }

    /// plan 出锅后的最后调整（例如按语言规则加上下文寄存器、改 callee-saved）。
    fn adjust_plan(&self, _plan: &mut AbiPlan) {}
}

/// 约定注册表。
#[derive(Default)]
pub struct AbiRegistry {
    rules: BTreeMap<String, AbiRules>,
    bindings: BTreeMap<(String, String), AbiBinding>,
    hooks: BTreeMap<String, Box<dyn AbiHooks>>,
}

impl std::fmt::Debug for AbiRegistry {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("AbiRegistry")
            .field("rules", &self.rules.keys().collect::<Vec<_>>())
            .field(
                "bindings",
                &self
                    .bindings
                    .keys()
                    .map(|(isa, conv)| format!("{isa}/{conv}"))
                    .collect::<Vec<_>>(),
            )
            .field("hooks", &self.hooks.keys().collect::<Vec<_>>())
            .finish()
    }
}

impl AbiRegistry {
    pub fn new() -> Self {
        Self::default()
    }

    /// 只装内置约定（`c`/`win64`/`sysv64`/`aapcs64`/`lp64d`）。
    pub fn with_builtin_rules() -> Result<Self, AbiError> {
        let mut r = Self::new();
        for rule in crate::builtin::rules()? {
            r.insert_rules(rule)?;
        }
        Ok(r)
    }

    /// 加一份约定（`parent` 必须**已注册**；继承在插入时展开）。
    pub fn insert_rules(&mut self, rules: AbiRules) -> Result<(), AbiError> {
        rules.validate()?;
        let expanded = match &rules.parent {
            Some(p) => {
                let parent = self.rules.get(p).ok_or_else(|| AbiError::BadRules {
                    name: rules.name.clone(),
                    why: format!("parent `{p}` 未注册——先注册父约定"),
                })?;
                rules.merge_parent(parent)
            }
            None => rules,
        };
        self.rules.insert(expanded.name.clone(), expanded);
        Ok(())
    }

    /// 从 TOML 文本加一份约定。
    pub fn insert_rules_toml(&mut self, text: &str) -> Result<(), AbiError> {
        self.insert_rules(AbiRules::from_toml(text)?)
    }

    /// 加一份 (ISA, 约定) 绑定。
    pub fn insert_binding(&mut self, binding: AbiBinding) -> Result<(), AbiError> {
        binding.validate()?;
        if !self.rules.contains_key(&binding.conv) {
            return Err(AbiError::BadRules {
                name: binding.isa.clone(),
                why: format!(
                    "绑定指向未注册的约定 `{}`（先 `insert_rules`）",
                    binding.conv
                ),
            });
        }
        self.bindings
            .insert((binding.isa.clone(), binding.conv.clone()), binding);
        Ok(())
    }

    /// 从 TOML 文本加一份绑定。
    pub fn insert_binding_toml(&mut self, text: &str) -> Result<(), AbiError> {
        self.insert_binding(AbiBinding::from_toml(text)?)
    }

    /// 挂宿主钩子（按约定名）。
    pub fn insert_hooks(&mut self, conv: &str, hooks: Box<dyn AbiHooks>) {
        self.hooks.insert(conv.to_string(), hooks);
    }

    pub fn rules(&self, name: &str) -> Option<&AbiRules> {
        self.rules.get(name)
    }

    pub fn binding(&self, isa: &str, conv: &str) -> Option<&AbiBinding> {
        self.bindings.get(&(isa.to_string(), conv.to_string()))
    }

    /// 已注册的约定名（排序）。
    pub fn conv_names(&self) -> Vec<&str> {
        self.rules.keys().map(String::as_str).collect()
    }

    /// 已注册的 (ISA, 约定) 对。
    pub fn binding_names(&self) -> Vec<(String, String)> {
        self.bindings
            .keys()
            .map(|(isa, conv)| (isa.clone(), conv.clone()))
            .collect()
    }

    /// **算一个签名的调用计划**（引擎入口的唯一推荐入口）。
    pub fn plan(
        &self,
        target: &dyn AbiTarget,
        conv: &str,
        sig: &Signature,
    ) -> Result<AbiPlan, AbiError> {
        let rules = self.rules.get(conv).ok_or_else(|| AbiError::BadRules {
            name: conv.to_string(),
            why: format!(
                "约定 `{conv}` 未注册（已注册：{}）——约定必须由使用者提供（内置见 forge_abi::builtin）",
                if self.rules.is_empty() {
                    "（空）".to_string()
                } else {
                    self.conv_names().join(", ")
                }
            ),
        })?;
        let isa = target.isa_name();
        let binding = self.binding(isa, conv).ok_or_else(|| AbiError::BadRules {
            name: format!("{isa}/{conv}"),
            why: format!(
                "ISA `{isa}` 没有为约定 `{conv}` 注册寄存器绑定（AbiBinding）——\
                     规则是平台无关的，寄存器由绑定给出（已注册：{:?}）",
                self.binding_names()
            ),
        })?;
        let mut plan = plan_fn(
            target,
            rules,
            binding,
            sig,
            self.hooks.get(conv).map(|b| &**b),
        )?;
        if let Some(h) = self.hooks.get(conv) {
            h.adjust_plan(&mut plan);
        }
        Ok(plan)
    }
}
