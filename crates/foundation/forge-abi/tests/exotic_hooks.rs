//! **A7：异域钩子（`AbiHooks`）示例**——Swift 与 Go。
//!
//! 这两个例子的作用是**证明有一条正规出口**：语言专属、数据表达不了的部分走
//! [`AbiHooks`]（分类回调 + plan 后调整），而**不是**去改引擎或往谱里塞特例。
//!
//! 三个"数据里没有"的事实（本例各自演示一个）：
//!
//! 1. **隐式上下文寄存器**：Swift 的 `self`（AAPCS64 = `X20`）、Go 的 `g`（amd64 = `R14`）
//!    都不是用户参数列表里的东西，所以既不能写进 `Signature::params`，也没有任何池能
//!    表达它——只有 [`AbiPlan::hidden`]（`HiddenSlots::context`）能承载，而它**只有钩子
//!    能填**（`grep hidden.context` 可见数据侧没有写入者）。
//! 2. **语言专属类型**：Swift 的 `error` 结果在 LLVM 里靠 `swifterror` **属性**标识，
//!    类型本身（`ptr`）与普通指针没有区别 ⇒ `classify` 是唯一能按语言规则改判的地方。
//!    本示例的约定：前端把 errortype 标成 `TyKind::Other` + 8 字节。
//! 3. **保留语义与"保存"语义不同**：`X21` 是 Swift 的 error 槽，它是**出参**——被调方
//!    可以改它，所以它必须**从 callee-saved 里摘掉**（LLVM 的
//!    `CSR_AArch64_AAPCS_SwiftError = CSR_AArch64_AAPCS − X21` 就是这个意思），
//!    而数据里的 `cs_gpr` 池只能表达"保存这组"。
//!
//! 参考：LLVM `AArch64CallingConvention.td`（`CCIfSwiftSelf → X20`、`CCIfSwiftError → X21`、
//! `CSR_AArch64_AAPCS_SwiftError`）。

mod common;

use common::{arm64_v12_with_fpr, x86_64_v12};
use forge_abi::builtin;
use forge_abi::plan::RegRef;
use forge_abi::{
    AbiHooks, AbiPlan, AbiRegistry, AbiRules, AbiTarget, ClassAction, ClassDir, RetLoc, Signature,
    SlotsSpec, TyKind, TyView,
};

/// 按名字在目标上取一个 `RegRef`（索引 + 类名都从目标查，不硬编码类名）。
fn reg(t: &impl AbiTarget, name: &str) -> RegRef {
    let index = t
        .reg_index(name)
        .unwrap_or_else(|| panic!("{name} 不在目标里"));
    RegRef::new(index, t.reg_class_name(index), name)
}

fn has(regs: &[RegRef], index: u32) -> bool {
    regs.iter().any(|r| r.index == index)
}

// ─────────────────────────── Swift（aapcs64 方言）───────────────────────────

/// Swift 的约定：AAPCS64 + `self`（X20）/ `error`（X21）。
const SWIFTCC_RULES: &str = r#"
name = "swiftcc"
parent = "aapcs64"
note = "Swift 的 aapcs64 方言：语言专属部分（self/error）由 AbiHooks 补，谱与规则里不写特例"
"#;

/// Swift 方言的寄存器绑定：AAPCS64 的池 + 一个 `swift_error` 池（X21）。
///
/// **注意 `cs_gpr` 里仍然列着 X21**——数据侧只能说"这台机器的帧会保存这组"；
/// "X21 在本方言下是出参、因此不保"是**语言规则**，由钩子摘掉（见下）。
const SWIFTCC_BINDING: &str = r#"
isa = "arm64_v12"
conv = "swiftcc"

[pools]
int = ["X0", "X1", "X2", "X3", "X4", "X5", "X6", "X7"]
sret = ["X8"]
cs_gpr = ["X19", "X20", "X21", "X22", "X23", "X24", "X25", "X26", "X27", "X28"]
cs_fpr = ["V8", "V9", "V10", "V11", "V12", "V13", "V14", "V15"]
float = ["V0", "V1", "V2", "V3", "V4", "V5", "V6", "V7"]
ret_int = ["X0", "X1"]
ret_float = ["V0", "V1", "V2", "V3"]
swift_error = ["X21"]
"#;

/// Swift 的钩子：`self` 进上下文槽并跨调用存活；`error` 是出参、不进保留集。
struct SwiftHooks {
    self_reg: RegRef,
    error_reg: RegRef,
}

impl AbiHooks for SwiftHooks {
    fn classify(&self, _rules: &AbiRules, ty: &TyView, dir: ClassDir) -> Option<ClassAction> {
        // 语言专属返回类型：约定的口径是"errortype 在前端被标成 Other/8"，
        // 于是钩子能把它从普通指针里分出来，落到 X21。
        (dir == ClassDir::Ret && ty.size == 8 && ty.kind == TyKind::Other).then(|| {
            ClassAction::Direct {
                pool: "swift_error".into(),
                slots: SlotsSpec::Fixed(1),
            }
        })
    }

    fn adjust_plan(&self, plan: &mut AbiPlan) {
        // self（X20）：隐式上下文 ⇒ 上下文槽；跨调用存活 ⇒ 从破坏集里摘掉、进保留集。
        plan.hidden.context = Some(self.self_reg.clone());
        plan.clobbers.retain(|r| r.index != self.self_reg.index);
        if !has(&plan.callee_saved.regs, self.self_reg.index) {
            plan.callee_saved.regs.push(self.self_reg.clone());
        }
        // error（X21）：**出参**，被调方可以改它 ⇒ 从保留集里摘掉、加进破坏集。
        plan.callee_saved
            .regs
            .retain(|r| r.index != self.error_reg.index);
        if !has(&plan.clobbers, self.error_reg.index) {
            plan.clobbers.push(self.error_reg.clone());
        }
        let note = plan.note.get_or_insert_with(String::new);
        if !note.is_empty() {
            note.push('；');
        }
        note.push_str("swiftcc: self=X20（保留）、error=X21（出参，可被被调方改写）");
    }
}

fn swift_registry(t: &impl AbiTarget) -> AbiRegistry {
    let mut registry = builtin::registry().expect("内置注册表");
    registry
        .insert_rules_toml(SWIFTCC_RULES)
        .expect("注册 swiftcc 规则");
    registry
        .insert_binding_toml(SWIFTCC_BINDING)
        .expect("注册 swiftcc 绑定");
    registry.insert_hooks(
        "swiftcc",
        Box::new(SwiftHooks {
            self_reg: reg(t, "X20"),
            error_reg: reg(t, "X21"),
        }),
    );
    registry
}

/// `self` 落到上下文槽 + 保留集；`error` 落到破坏集。
#[test]
fn swift_hooks_place_self_and_error() {
    let target = arm64_v12_with_fpr();
    let reg = swift_registry(&target);
    let sig = Signature::new(
        vec![("a".into(), TyView::int(8, 8))],
        Some(TyView::int(8, 8)),
    );
    let plan = reg.plan(&target, "swiftcc", &sig).expect("swiftcc plan");

    let x20 = target.reg_index("X20").expect("X20");
    let x21 = target.reg_index("X21").expect("X21");

    let ctx = plan
        .hidden
        .context
        .as_ref()
        .expect("钩子必须填上上下文槽（数据里没有隐式参数这个概念）");
    assert_eq!(ctx.name, "X20", "Swift 的 self 在 X20");
    assert!(has(&plan.callee_saved.regs, x20), "self 必须跨调用存活");
    assert!(!has(&plan.callee_saved.regs, x21), "error 是出参，不保");
    assert!(has(&plan.clobbers, x21), "error 必须算进破坏集");
    assert!(
        plan.note.as_deref().unwrap_or("").contains("swiftcc"),
        "注明出处"
    );
}

/// 语言专属返回类型（errortype）按钩子改判到 `swift_error` 池（X21）。
#[test]
fn swift_error_return_uses_the_language_pool() {
    let target = arm64_v12_with_fpr();
    let reg = swift_registry(&target);
    // 无标签时是普通指针 ⇒ 走 aapcs64 的 ret_int（X0）。
    let plain = Signature::new(vec![], Some(TyView::new(8, 8, TyKind::Ptr)));
    match reg.plan(&target, "swiftcc", &plain).expect("plan").ret {
        RetLoc::Reg { reg } => assert_eq!(reg.name, "X0"),
        other => panic!("{other:?}"),
    }
    // 标成 Other/8（前端对 errortype 的口径）⇒ 钩子改判到 X21。
    let err = Signature::new(vec![], Some(TyView::new(8, 8, TyKind::Other)));
    match reg.plan(&target, "swiftcc", &err).expect("plan").ret {
        RetLoc::Reg { reg } => assert_eq!(reg.name, "X21"),
        other => panic!("{other:?}"),
    }
}

// ─────────────────────────── Go（amd64 内部 ABI）───────────────────────────

/// Go 的内部 ABI：int/float **共享位置游标**、无 C 意义上的 callee-saved、
/// 隐式上下文 `g` 在 R14（数据里没有"隐式参数"，所以只能由钩子填）。
const GOCONV_RULES: &str = r#"
name = "goconv"
parent = "c"
position = "by_position"
note = "Go 内部 ABI（示例）：寄存器参数按位置共享游标；GC 安全点用的 g 由 AbiHooks 补"
"#;

const GOCONV_BINDING: &str = r#"
isa = "x86_64_v12"
conv = "goconv"

[pools]
# Go 内部 ABI 的整数参数序列（与 SysV 的 RDI/RSI… 不同，这正是"约定是使用者的数据"）。
int = ["RAX", "RBX", "RCX", "RDI", "RSI", "R8", "R9", "R10", "R11"]
ret_int = ["RAX", "RBX"]
float = ["XMM0", "XMM1", "XMM2", "XMM3", "XMM4", "XMM5", "XMM6", "XMM7"]
ret_float = ["XMM0", "XMM1"]
# 没有 `cs_gpr` 池：Go 没有"被调方负责保存"的寄存器（调用方负责 spill），
# 数据侧表达这件事的方式就是**不声明**（声明了空池会被校验挡下）。
"#;

/// Go 的钩子：`g` 在 R14——它是**隐式上下文**（不是参数），且**在整个函数体里
/// 必须可寻址**（GC 安全点），因此不能出现在破坏集里。
struct GoHooks {
    g_reg: RegRef,
}

impl AbiHooks for GoHooks {
    fn adjust_plan(&self, plan: &mut AbiPlan) {
        plan.hidden.context = Some(self.g_reg.clone());
        plan.clobbers.retain(|r| r.index != self.g_reg.index);
        if !has(&plan.callee_saved.regs, self.g_reg.index) {
            plan.callee_saved.regs.push(self.g_reg.clone());
        }
        let note = plan.note.get_or_insert_with(String::new);
        if !note.is_empty() {
            note.push('；');
        }
        note.push_str("goconv: g=R14（隐式上下文，任何调用点都必须还活着 → GC 安全点）");
    }
}

/// Go 的 `g` 进上下文槽、不在破坏集、且在保留集里。
#[test]
fn go_hooks_keep_the_g_register_alive() {
    let target = x86_64_v12();
    let mut reg = builtin::registry().expect("内置注册表");
    reg.insert_rules_toml(GOCONV_RULES)
        .expect("注册 goconv 规则");
    reg.insert_binding_toml(GOCONV_BINDING)
        .expect("注册 goconv 绑定");
    reg.insert_hooks(
        "goconv",
        Box::new(GoHooks {
            g_reg: reg_of(&target, "R14"),
        }),
    );

    let sig = Signature::new(
        vec![
            ("a".into(), TyView::int(8, 8)),
            ("b".into(), TyView::int(8, 8)),
        ],
        Some(TyView::int(8, 8)),
    );
    let plan = reg.plan(&target, "goconv", &sig).expect("goconv plan");

    let r14 = target.reg_index("R14").expect("R14");
    assert_eq!(
        plan.hidden.context.as_ref().map(|r| r.name.as_str()),
        Some("R14"),
        "g 是隐式上下文（数据里没有这个槽）"
    );
    assert!(!has(&plan.clobbers, r14), "g 不能被算作调用点可破坏");
    assert!(has(&plan.callee_saved.regs, r14), "g 必须跨调用存活");
    // 数据侧那半：绑定里**没有** `cs_gpr` 池 ⇒ 引擎本来一个保留寄存器都不给
    //（Go 的"无 callee-saved"）。这说明**保留语义可以是语言规则**，不必写进谱与池。
    assert!(
        plan.callee_saved.regs.iter().all(|r| r.index == r14),
        "除了钩子加的 g，Go 没有别的保留寄存器：{:?}",
        plan.callee_saved.regs
    );
}

/// 与 [`reg`] 同义，避免在闭包里再借一次 `target`。
fn reg_of(t: &impl AbiTarget, name: &str) -> RegRef {
    reg(t, name)
}
