//! **调用点布局查询**（v20 A5-3）：按 `(ISA, 约定, 形状)` 算**被调方**的调用布局。
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
//!   │  CallRequest { conv, args, rets, variadic }      ← 本模块（中性 POD）
//!   ▼
//! plan_call(isa, &req)                                 ← 注册表
//!   │  宿主注册的 CallPlanner（闭包捕获自己的 TargetMachine）
//!   ▼
//! forge-abi 引擎：Signature → AbiPlan → CallLayout
//! ```
//!
//! ## 两条人体工学纪律（v20，2026-09-30）
//!
//! 1. **一次调用用一个结构体描述**（[`CallRequest`]，带 builder），不散成 4 个位置参数：
//!    `args`/`rets` 的顺序、`variadic` 的含义都容易搞错；结构体让调用点写出来自解释，
//!    以后再加字段（比如"按调用的 `CallConvId`"）也不用改所有调用方。
//! 2. **失败必须带诊断**（[`CallPlanError`]）：早期版本返回 `Option`，把引擎的
//!    `PoolExhausted`/`MissingPool`/能力缺口原因**丢掉了**，用户只看到一句"布局不可得"。
//!    现在失败要么是"这台 ISA 没装 planner"（[`CallPlanError::NoPlanner`]），要么是
//!    "算了但不行"（[`CallPlanError::Failed`]，正文来自宿主/引擎）。
//!
//! 分层纪律：本 crate **不依赖 forge-abi**（[`ArgShape`] 是中性 POD，见
//! [`call_layout`](crate::machine::call_layout)），把形状摊成 `forge_abi::TyView` 与跑引擎
//! 都是**宿主**的事（`forge-codegen` 在 `pipeline_hooks::ensure_registered` 里注册）。

use std::collections::HashMap;
use std::fmt;
use std::sync::{Arc, OnceLock, RwLock};

use crate::machine::call_layout::{ArgShape, CallLayout};

/// **一次调用的形状描述**（调用点能提供的全部信息）。
///
/// 为什么是一个结构体而不是四个位置参数：三个形状类参数（`args`/`rets`/`variadic`）彼此
/// 没有类型区分，位置写反了编译器不会拦；builder 让调用点写成
/// `CallRequest::new(conv).args(a).rets(r).variadic(2)`，读起来就是意图。
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct CallRequest {
    /// 宿主注册表里的约定名（`AbiRules::name`）。
    pub conv: String,
    /// 实参形状（顺序 = 位置）。
    pub args: Vec<ArgShape>,
    /// **全部**返回值的形状（空 = void；多个 = 多值返回，每个值各自占一个返回寄存器）。
    pub rets: Vec<ArgShape>,
    /// **被调方是不是变参、命名了几个**（`None` = 非变参 / 调用点不知道）。
    ///
    /// 调用点只看得见实参形状，而"未命名实参在**只走栈**的约定里要改判到栈"只有知道被调方
    /// 签名才判得出来——宿主把模块签名表查给它（IR 不必带签名句柄）。
    pub variadic: Option<(bool, u32)>,
}

impl CallRequest {
    /// 一份"非变参、无参无返回"的请求（builder 起点）。
    pub fn new(conv: impl Into<String>) -> Self {
        Self {
            conv: conv.into(),
            ..Self::default()
        }
    }

    /// 实参形状（`IntoIterator<Item: Into<ArgShape>>` ⇒ 数组、`Vec`、`&Vec`、`iter()` 都能直接传）。
    pub fn args<I, S>(mut self, args: I) -> Self
    where
        I: IntoIterator<Item = S>,
        S: Into<ArgShape>,
    {
        self.args = args.into_iter().map(Into::into).collect();
        self
    }

    /// 全部返回值形状（同上，接受借用/拥有两种形状来源）。
    pub fn rets<I, S>(mut self, rets: I) -> Self
    where
        I: IntoIterator<Item = S>,
        S: Into<ArgShape>,
    {
        self.rets = rets.into_iter().map(Into::into).collect();
        self
    }

    /// 标记"被调方是变参、命名 `named` 个"（未命名的按约定规则落位）。
    pub fn variadic(mut self, named: u32) -> Self {
        self.variadic = Some((true, named));
        self
    }
}

/// **调用点布局查询失败**（带诊断正文）。
///
/// 早期版本 `plan_call` 返回 `Option`：引擎算失败时原因（池不够/缺绑定/能力缺口）被丢掉，
/// 调用点只能报一句"布局不可得"。现在两类原因分开，且 [`CallPlanError::Failed`] 的正文
/// 原样来自宿主/引擎，用户据此就能定位（与 `forge-isa abi check` 的口径一致）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CallPlanError {
    /// 这台 ISA 没有注册 planner——宿主没接
    /// [`register_call_planner`]，也没走 `forge_codegen::pipeline_hooks::ensure_registered`。
    NoPlanner { isa: String },
    /// planner 算了但没算出布局（约定未注册 / 池不够 / 能力缺口…），`why` 是原文。
    Failed { conv: String, why: String },
}

impl fmt::Display for CallPlanError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::NoPlanner { isa } => write!(
                f,
                "ISA `{isa}` 没有注册调用点 planner（宿主应调用 \
                 forge_isa_runtime::machine::call_plan::register_call_planner，\
                 或走宿主侧的 pipeline_hooks::ensure_registered）"
            ),
            Self::Failed { conv, why } => write!(
                f,
                "约定 `{conv}` 算不出这次调用的布局：{why}（自查：`forge-isa abi check <谱>`；\
                 口径见 docs/reference/calling-conventions.md）"
            ),
        }
    }
}

impl std::error::Error for CallPlanError {}

/// 算一份**被调方布局**的宿主钩子。
///
/// 实现者通常是宿主里捕获了自己 `TargetMachine` 的闭包（本模块为闭包提供 blanket impl）。
pub trait CallPlanner: Send + Sync {
    /// 按 [`CallRequest`] 算布局；失败时返回**带诊断**的 [`CallPlanError`]（不猜落点）。
    fn plan_call(&self, req: &CallRequest) -> Result<CallLayout, CallPlanError>;
}

impl<F> CallPlanner for F
where
    F: Fn(&CallRequest) -> Result<CallLayout, CallPlanError> + Send + Sync,
{
    fn plan_call(&self, req: &CallRequest) -> Result<CallLayout, CallPlanError> {
        self(req)
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
pub fn plan_call(isa: &str, req: &CallRequest) -> Result<CallLayout, CallPlanError> {
    let planner = planners()
        .read()
        .ok()
        .and_then(|m| m.get(isa).cloned())
        .ok_or_else(|| CallPlanError::NoPlanner {
            isa: isa.to_string(),
        })?;
    planner.plan_call(req)
}
