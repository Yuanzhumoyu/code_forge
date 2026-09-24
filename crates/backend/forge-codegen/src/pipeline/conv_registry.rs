//! **宿主约定注册表**：把 IR 里的 [`CallConvId`] 解析成"这份约定叫什么名字"，
//! 名字拿去查 `forge-abi` 的规则/绑定（A3 起按它发射）。
//!
//! ## 为什么要有这一层（这是旧设计最大的坑）
//!
//! 旧实现里 `CallConv` 是个 16 变体的枚举，`ctx.call_conv` **只被写、从没被读**
//! （`CompileState::new` 里赋一次值就没了）——等于文档。这里把它变成一条**真实的读路径**：
//!
//! ```text
//! FunctionSignature.calling_convention: CallConvId
//!        │  resolve()                    ← 本模块（宿主数据）
//!        ▼
//!  约定名（"win64"/"sysv64"/"my_conv"/"cc42"）
//!        │  forge_abi::AbiRegistry::rules(name)
//!        ▼
//!   AbiRules + AbiBinding → AbiPlan（A3 按它发射调用点/入口/序尾声）
//! ```
//!
//! ## 三条纪律
//!
//! 1. **未注册 ⇒ fail-closed**：`CallConvId::Named("nope")` / `Index(9999)` 一律报错并列出
//!    已注册的名字。**不**退回某个缺省约定（旧设计的 `CallConv::Default` 就是"静默兜底"
//!    制造的死值）。
//! 2. **约定是使用者的数据**：内置只提供 `forge-abi` 那五份（`c`/`win64`/`sysv64`/`aapcs64`/
//!    `lp64d`）；宿主用 [`register_rules_toml`] / [`register_binding_toml`] 加自己的。
//! 3. **数值约定按编号分派**：`CallConvId::Index(n)` 的注册键是 `"cc{n}"`（LLVM `cc N` 的
//!    文本形），因此"按编号分派"的前端与"按名字分派"的前端走同一条查表路径。

use std::sync::{OnceLock, RwLock};

use forge_abi::AbiRegistry;
use forge_ir::CallConvId;
use forge_ir::error::IrError;

/// 全局注册表（进程内单例；宿主启动时注册自己的约定）。
static REGISTRY: OnceLock<RwLock<AbiRegistry>> = OnceLock::new();

/// 取注册表（首次调用时装**内置五份**规则与四份绑定）。
pub fn registry() -> &'static RwLock<AbiRegistry> {
    REGISTRY.get_or_init(|| {
        let reg = forge_abi::builtin::registry()
            .expect("内置约定/绑定必须能解析（forge-abi 的单测同源守护）");
        RwLock::new(reg)
    })
}

/// 加一份**自定义约定**（TOML；`parent` 必须已注册）。
pub fn register_rules_toml(toml: &str) -> Result<(), String> {
    registry()
        .write()
        .expect("约定注册表被投毒（持锁线程 panic）")
        .insert_rules_toml(toml)
        .map_err(|e| e.to_string())
}

/// 加一份 `(ISA, 约定)` 的寄存器绑定。
pub fn register_binding_toml(toml: &str) -> Result<(), String> {
    registry()
        .write()
        .expect("约定注册表被投毒（持锁线程 panic）")
        .insert_binding_toml(toml)
        .map_err(|e| e.to_string())
}

/// `CallConvId` → 注册表键。
///
/// - `Builtin(n)` / `Named(s)` → 名字本身；
/// - `Index(n)` → `"cc{n}"`（数值约定的文本形）。
pub fn registry_key(id: &CallConvId) -> String {
    match id {
        CallConvId::Builtin(n) => n.as_str().to_string(),
        CallConvId::Named(s) => s.as_str().to_string(),
        CallConvId::Index(n) => format!("cc{n}"),
    }
}

/// **解析并校验**一个调用约定：返回注册表里的名字（A3 用它查规则/绑定）。
///
/// 未注册 ⇒ [`IrError::Unsupported`]，消息里带"已注册的名字"，让使用者能自己修。
pub fn resolve(id: &CallConvId) -> Result<String, IrError> {
    let key = registry_key(id);
    let reg = registry().read().expect("约定注册表被投毒");
    if reg.rules(&key).is_some() {
        return Ok(key);
    }
    let known = reg.conv_names();
    Err(IrError::Unsupported(format!(
        "调用约定 `{key}` 未在宿主注册表里注册（已注册：{}）——约定是使用者的数据，\
         请用 forge_codegen::pipeline::conv_registry::register_rules_toml 注册，\
         或改用已注册的名字（内置：{}）",
        if known.is_empty() {
            "（空）".to_string()
        } else {
            known.join(", ")
        },
        forge_abi::builtin::ALL
            .iter()
            .map(|(n, _)| *n)
            .collect::<Vec<_>>()
            .join(", ")
    )))
}

/// 已注册的约定名（诊断/测试用；排序）。
pub fn registered_names() -> Vec<String> {
    registry()
        .read()
        .expect("约定注册表被投毒")
        .conv_names()
        .into_iter()
        .map(str::to_string)
        .collect()
}
