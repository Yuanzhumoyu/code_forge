/// 宿主 AbiTarget 适配器（真实后端 → forge-abi 引擎；v20 A3）。
pub mod abi_setup;
pub mod abi_target;
pub mod agg_const;
pub mod agg_expand;
pub mod alloc_config;
pub mod compiler;
/// 宿主调用约定注册表（IR 的 `CallConvId` → 注册名；未注册即 fail-closed）。
pub mod conv_registry;
pub mod emission;
pub mod frame_layout;
pub mod frame_slots;
/// **能力缺口台账的代码侧锚点**（v24 M3）：`capability(ID, msg)` ⇒ 消息带 `[cap:ID]`，
/// 让"这台机器/这份约定/这份谱做不了什么"成为**可机读的集合**（守卫与台账逐一相等）。
pub mod gaps;
pub mod legalize;
pub mod liverange;
pub mod lowering;
pub mod regalloc_bt;
/// 模块级编译编排：`CompiledModule` + 整模块/单函数入口（M1：唯一编译入口）。
pub mod session;
/// IR 视图 → 引擎视图（签名 + 参数属性；v20 A2b）。
pub mod sig_view;
/// **观测开关台账**（v24 M4）：诊断开关不可能静默失效（见模块文档）。
pub mod trace;
pub mod va_expand;

pub use forge_isa_runtime::{alloc_result, emit, vcode};
