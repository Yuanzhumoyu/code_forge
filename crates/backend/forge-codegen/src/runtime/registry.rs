//! 后端注册表。
//!
//! 提供全局后端注册和查找功能。ISA 后端通过 `ensure_registered()`
//! (DSL 生成) 注册其 `TargetMachine` 到全局注册表。
//!
//! 注册表存储 `Arc<dyn ErasedTargetMachine>`，支持名称查找和类型擦除编译。

use crate::machine::target::ErasedTargetMachine;
use std::collections::HashMap;
use std::sync::{Arc, OnceLock, RwLock};

/// 全局后端注册表。
pub struct Registry {
    backends: RwLock<HashMap<String, Arc<dyn ErasedTargetMachine>>>,
}

impl Registry {
    /// 获取全局注册表单例。
    pub fn global() -> &'static Self {
        static INSTANCE: OnceLock<Registry> = OnceLock::new();
        INSTANCE.get_or_init(|| Registry {
            backends: RwLock::new(HashMap::new()),
        })
    }

    /// 注册一个类型擦除的后端编译器。
    pub fn register_backend(&self, backend: Arc<dyn ErasedTargetMachine>) {
        let name = backend.erased_name().to_string();
        let mut backends = self.backends.write().expect("registry lock poisoned");
        backends.insert(name, backend);
    }

    /// 按名称查找后端。
    pub fn lookup(&self, name: &str) -> Option<Arc<dyn ErasedTargetMachine>> {
        let backends = self.backends.read().expect("registry lock poisoned");
        backends.get(name).cloned()
    }

    /// 使用已注册的后端编译一个 IR 函数。
    /// 列出所有已注册的后端名称。
    /// 后端是否已注册。
    pub fn contains(&self, name: &str) -> bool {
        let backends = self.backends.read().expect("registry lock poisoned");
        backends.contains_key(name)
    }
}
