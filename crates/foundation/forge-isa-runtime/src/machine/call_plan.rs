//! **调用点布局查询**（v20 A5-3）：按 `(ISA, 约定, 实参形状)` 算**被调方**的调用布局。
//!
//! ## 为什么需要这一层
//!
//! 调用点在 lowering 时只有实参的**形状**（[`ArgShape`]：大小/对齐/族/成员），没有被调方的
//! `Function`；而"第 i 个实参该进哪个寄存器、哪个栈槽"只有 `forge-abi` 的引擎算得出来
//! （规则 + 绑定 = 使用者的数据）。谱面里那份 `[abi.arg_class]`/`ret_regs` 只是这份数据的
//! **生成期近似**——按位置/按类计数、by-ref 阈值、sret 槽，引擎比生成器知道得多。
//!
//! ```text
//! 生成物 lowering（调用点）
//!   │  形状 = (size, align, kind, members)
//!   ▼
//! plan_call(isa, conv, args, rets)         ← 本模块（注册表）
//!   │  宿主注册的 CallPlanner（闭包捕获自己的 TargetMachine）
//!   ▼
//! forge-abi 引擎：Signature → AbiPlan → CallLayout
//! ```
//!
//! 分层纪律：本 crate **不依赖 forge-abi**（[`ArgShape`] 是中性 POD，见
//! [`call_layout`](crate::machine::call_layout)），把形状摊成 `forge_abi::TyView` 与跑引擎
//! 都是**宿主**的事（`forge-codegen` 在 `pipeline_hooks::ensure_registered` 里注册）。
//!
//! 未注册 ⇒ [`plan_call`] 返回 `None`（生成物据此 **fail-closed**，不猜落点）。

use std::collections::HashMap;
use std::sync::{Arc, OnceLock, RwLock};

use crate::machine::call_layout::{ArgShape, CallLayout};

/// 算一份**被调方布局**的宿主钩子。
///
/// 实现者通常是宿主里捕获了自己 `TargetMachine` 的闭包（本模块为闭包提供 blanket impl）。
pub trait CallPlanner: Send + Sync {
    /// `conv` = 宿主注册表里的约定名（`AbiRules::name`）；`rets` = 返回值的形状（空 = void，
    /// 多个 = **多值返回**，每个值各自占一个返回寄存器，见 v20 A6）。
    ///
    /// `variadic` = **被调方是不是变参、命名了几个**（`None` = 非变参/未知，变参 D6）：
    /// 调用点只看得见实参形状，而"未命名实参在**只走栈**的约定里要改判到栈"只有知道被调方
    /// 签名才判得出来（宿主把模块签名表查给它，IR 不必带签名句柄）。
    ///
    /// 算不出来（约定未注册 / 池缺 / 能力缺口）⇒ `None`（fail-closed，调用方不猜）。
    fn plan_call(
        &self,
        conv: &str,
        args: &[ArgShape],
        rets: &[ArgShape],
        variadic: Option<(bool, u32)>,
    ) -> Option<CallLayout>;
}

impl<F> CallPlanner for F
where
    F: Fn(&str, &[ArgShape], &[ArgShape], Option<(bool, u32)>) -> Option<CallLayout> + Send + Sync,
{
    fn plan_call(
        &self,
        conv: &str,
        args: &[ArgShape],
        rets: &[ArgShape],
        variadic: Option<(bool, u32)>,
    ) -> Option<CallLayout> {
        self(conv, args, rets, variadic)
    }
}

static PLANNERS: OnceLock<RwLock<HashMap<String, Arc<dyn CallPlanner>>>> = OnceLock::new();

fn planners() -> &'static RwLock<HashMap<String, Arc<dyn CallPlanner>>> {
    PLANNERS.get_or_init(|| RwLock::new(HashMap::new()))
}

/// 为某个 ISA 名（`IsaInfo::name()`）注册调用点布局钩子。重复注册同名 ISA ⇒ 后者生效。
pub fn register_call_planner(isa: &str, planner: Arc<dyn CallPlanner>) {
    if let Ok(mut map) = planners().write() {
        map.insert(isa.to_string(), planner);
    }
}

/// 该 ISA 是否注册过调用点布局钩子。
pub fn has_call_planner(isa: &str) -> bool {
    planners()
        .read()
        .map(|m| m.contains_key(isa))
        .unwrap_or(false)
}

/// 查一份调用点布局（生成物 lowering 的入口）。
///
/// `variadic` 见 [`CallPlanner::plan_call`]（`None` = 非变参/调用点不知道）。
pub fn plan_call(
    isa: &str,
    conv: &str,
    args: &[ArgShape],
    rets: &[ArgShape],
    variadic: Option<(bool, u32)>,
) -> Option<CallLayout> {
    let planner = planners().read().ok().and_then(|m| m.get(isa).cloned())?;
    planner.plan_call(conv, args, rets, variadic)
}
