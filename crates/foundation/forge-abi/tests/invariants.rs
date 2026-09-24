//! **不变量**：快照能抓"变了"，抓不到"错了"。这里逐条断言 plan 的语义约束，
//! 每一条都对应一个真实约定事实或一处曾经写错的地方。
//!
//! 全部断言只用 `AbiPlan` 的公开数据；不碰生成器、不跑机器码（那是 A3+ 的事）。

mod common;

use common::*;
use forge_abi::{
    AbiBinding, AbiError, AbiHooks, AbiPlan, AbiRegistry, AbiRules, AbiTarget, ArgLoc, ClassAction,
    ClassDir, Placement, Purpose, RetLoc, Signature, TyView,
};

fn registry() -> AbiRegistry {
    forge_abi::builtin::registry().expect("内置注册表")
}

fn plan(reg: &AbiRegistry, isa: &str, conv: &str, sig: &Signature) -> AbiPlan {
    let t = target_for(isa).expect("合成目标");
    reg.plan(&t, conv, sig)
        .unwrap_or_else(|e| panic!("{isa}/{conv} 规划失败：{e}"))
}

fn one(ty: TyView) -> Signature {
    Signature::new(vec![("a".into(), ty.clone())], Some(ty))
}

/// 取 plan 里第一个实参的落点。
fn arg0(plan: &AbiPlan) -> &Placement {
    &plan.args[0].place
}

// ───────────────── 位置计数：按位置 vs 按类 ─────────────────

/// Windows x64 的 int/float **共享位置游标**：第 2 个参数即使是浮点也进 XMM1。
#[test]
fn win64_counts_positions_not_classes() {
    let reg = registry();
    let sig = Signature::new(
        vec![("n".into(), i64_()), ("x".into(), f64_())],
        Some(i64_()),
    );
    let p = plan(&reg, "x86_64_v12", "win64", &sig);
    assert_eq!(place_reg(&p.args[0].place), "RCX");
    assert_eq!(place_reg(&p.args[1].place), "XMM1");
}

/// SysV 与 RISC-V 按**类**各自计数：第 2 个参数是浮点 → XMM0 / F10。
#[test]
fn by_class_advances_two_independent_cursors() {
    let reg = registry();
    let sig = Signature::new(
        vec![("n".into(), i64_()), ("x".into(), f64_())],
        Some(i64_()),
    );
    let p = plan(&reg, "x86_64_v12", "sysv64", &sig);
    assert_eq!(place_reg(&p.args[0].place), "RDI");
    assert_eq!(place_reg(&p.args[1].place), "XMM0");

    let p = plan(&reg, "riscv64_v12", "lp64d", &sig);
    assert_eq!(place_reg(&p.args[0].place), "X10");
    assert_eq!(place_reg(&p.args[1].place), "F10");
}

fn place_reg(p: &Placement) -> String {
    match p {
        Placement::Reg { reg, .. } => reg.name.clone(),
        Placement::RegPair { lo, hi } => format!("{}:{}", lo.name, hi.name),
        Placement::RegGroup { regs } => regs
            .iter()
            .map(|r| r.name.clone())
            .collect::<Vec<_>>()
            .join(":"),
        Placement::Stack { offset, .. } => format!("stack@{offset}"),
        Placement::Indirect { ptr, .. } => {
            format!(
                "indirect({})",
                ptr.as_ref().map(|r| r.name.clone()).unwrap_or("-".into())
            )
        }
        Placement::Ignore => "ignore".into(),
    }
}

// ───────────────── HFA：只认浮点、槽数按类型 ─────────────────

/// `struct {i64,i64}` **不是** HFA：RISC-V 把它放 a0:a1（整数槽），不是 fa0:fa1。
///
/// 这条断言守住的是一个真实错值路径：早期把"同族同宽成员"不分整数/浮点都当 HFA，
/// 会让 16 字节整数结构体进浮点寄存器。
#[test]
fn integer_aggregate_is_not_hfa() {
    let reg = registry();
    let p = plan(&reg, "riscv64_v12", "lp64d", &one(agg_ii()));
    assert_eq!(place_reg(arg0(&p)), "X10:X11");

    let p = plan(&reg, "riscv64_v12", "lp64d", &one(hfa2()));
    assert_eq!(place_reg(arg0(&p)), "F10:F11");
}

/// 单成员 HFA 只占**一个**槽（写死 `slots = 2/4` 会白吃寄存器并把后面的参数挤到栈上）。
#[test]
fn single_member_hfa_takes_one_slot() {
    let reg = registry();
    let p = plan(&reg, "riscv64_v12", "lp64d", &one(hfa1()));
    assert_eq!(place_reg(arg0(&p)), "F10");

    // 1 成员 HFA 之后再放一个 f64：应落在 F11（而不是被 1 成员 HFA 吃掉两个槽）。
    let sig = Signature::new(vec![("s".into(), hfa1()), ("x".into(), f64_())], None);
    let p = plan(&reg, "riscv64_v12", "lp64d", &sig);
    assert_eq!(place_reg(&p.args[1].place), "F11");
}

/// 4 成员 HFA（AAPCS64）用 `RegGroup`：V0-V3 连续四槽。
#[test]
fn four_member_hfa_uses_register_group() {
    let mut reg = registry();
    // 真实 arm64 绑定缺浮点池；这里换成"补齐后"的合成绑定。
    reg.insert_binding_toml(AAPCS64_FULL_BINDING)
        .expect("合成绑定");
    let t = arm64_v12_with_fpr();
    let param_only = |ty: TyView| Signature::new(vec![("s".into(), ty)], None);
    let p = reg
        .plan(&t, "aapcs64", &param_only(hfa4()))
        .expect("HFA4 参数");
    match &p.args[0].place {
        Placement::RegGroup { regs } => {
            let names: Vec<&str> = regs.iter().map(|r| r.name.as_str()).collect();
            assert_eq!(names, ["V0", "V1", "V2", "V3"]);
        }
        other => panic!("期望 RegGroup，实际 {other:?}"),
    }

    // 2 成员 HFA 仍是 RegPair（拆分的两个槽），1 成员是单寄存器。
    let p = reg.plan(&t, "aapcs64", &param_only(hfa2())).expect("HFA2");
    assert_eq!(place_reg(arg0(&p)), "V0:V1");
    let p = reg.plan(&t, "aapcs64", &param_only(hfa1())).expect("HFA1");
    assert_eq!(place_reg(arg0(&p)), "V0");
}

/// 浮点寄存器不够时**整块走栈**（而不是"取一半"）。
#[test]
fn insufficient_hfa_registers_fall_back_to_stack_as_a_whole() {
    let mut reg = registry();
    reg.insert_binding_toml(AAPCS64_FULL_BINDING)
        .expect("合成绑定");
    let t = arm64_v12_with_fpr();
    // 先用掉 V0-V4（5 个 f64），float 池只剩 V5-V7 = 3 个，装不下 4 成员 HFA。
    let mut params: Vec<(String, TyView)> = (0..5).map(|i| (format!("x{i}"), f64_())).collect();
    params.push(("s".into(), hfa4()));
    let sig = Signature::new(params, None);
    let p = reg.plan(&t, "aapcs64", &sig).expect("plan");
    assert_eq!(
        place_reg(&p.args[4].place),
        "V4",
        "前 5 个 f64 应占满 V0-V4"
    );
    match &p.args[5].place {
        Placement::Stack { size, .. } => assert_eq!(*size, 16),
        other => panic!("期望整块走栈，实际 {other:?}"),
    }
}

// ───────────────── 返回位：独立的返回寄存器 ─────────────────

/// 四份约定的 sret 落点**各不相同**——这正是旧实现"永远取首 int 参数槽"错的地方。
#[test]
fn wide_return_uses_each_conventions_own_sret_slot() {
    let reg = registry();
    let cases = [
        ("x86_64_v12", "win64", "RCX"),
        ("x86_64_v12", "sysv64", "RDI"),
        ("arm64_v12", "aapcs64", "X8"),
        ("riscv64_v12", "lp64d", "X10"),
    ];
    for (isa, conv, want) in cases {
        let p = plan(&reg, isa, conv, &one(agg24()));
        assert!(matches!(p.ret, RetLoc::Indirect { .. }), "{conv}: {p:?}");
        let sret = p
            .hidden
            .sret
            .as_ref()
            .unwrap_or_else(|| panic!("{conv} 没给 sret"));
        assert_eq!(sret.name, want, "{conv} 的 sret 落点");
        // sret 指针**不占**用户参数：第一个用户参数不能与它同号。
        let a0 = place_reg(arg0(&p));
        assert_ne!(a0, want, "{conv}: 第一个实参与 sret 指针撞号（{a0}）");
    }
}

/// 标量返回走**返回池**，不是参数池：Win64 在 RAX（参数却从 RCX 起）。
#[test]
fn scalar_return_uses_return_pool() {
    let reg = registry();
    let p = plan(&reg, "x86_64_v12", "win64", &one(i64_()));
    assert_eq!(ret_reg(&p.ret), "RAX");
    let p = plan(&reg, "x86_64_v12", "sysv64", &one(i64_()));
    assert_eq!(ret_reg(&p.ret), "RAX");
    // 浮点返回：XMM0 / F10（aapcs64 缺浮点池 → 见 errors.rs 的 GAP 断言）。
    let p = plan(&reg, "x86_64_v12", "win64", &one(f64_()));
    assert_eq!(ret_reg(&p.ret), "XMM0");
    let p = plan(&reg, "riscv64_v12", "lp64d", &one(f64_()));
    assert_eq!(ret_reg(&p.ret), "F10");
}

fn ret_reg(r: &RetLoc) -> String {
    match r {
        RetLoc::Reg { reg } => reg.name.clone(),
        RetLoc::RegPair { lo, hi } => format!("{}:{}", lo.name, hi.name),
        other => format!("{other:?}"),
    }
}

/// ≤16B 聚合的**双槽返回**：SysV 用 RAX:RDX（返回池的两槽，与参数池无关）。
#[test]
fn two_slot_return_uses_return_pool_pair() {
    let reg = registry();
    let p = plan(&reg, "x86_64_v12", "sysv64", &one(agg_ii()));
    assert_eq!(ret_reg(&p.ret), "RAX:RDX");
    let p = plan(&reg, "riscv64_v12", "lp64d", &one(agg_ii()));
    assert_eq!(ret_reg(&p.ret), "X10:X11");
}

// ───────────────── 全局不变量 ─────────────────

/// 所有 (ISA, 约定) × 语料：寄存器不重复记账、clobber 与被保存集不交、栈参数不重叠。
#[test]
fn plan_invariants_hold_for_every_case() {
    let reg = registry();
    let cases: Vec<Case> = corpus().into_iter().chain(variadic_cases()).collect();
    let combos = [
        ("x86_64_v12", "win64"),
        ("x86_64_v12", "sysv64"),
        ("arm64_v12", "aapcs64"),
        ("riscv64_v12", "lp64d"),
    ];
    let mut checked = 0usize;
    for (isa, conv) in combos {
        let t = target_for(isa).unwrap();
        for case in &cases {
            let Ok(p) = reg.plan(&t, conv, &case.sig) else {
                continue; // 规划不了的组合由 errors.rs 的 GAP 断言覆盖
            };
            checked += 1;
            let (mut regs, mut ends) = (Vec::new(), Vec::new());
            let mut wants_byval = false;
            for a in &p.args {
                collect_regs(&a.place, &mut regs);
                if let Placement::Stack { offset, size, .. } = &a.place {
                    assert!(
                        (*offset as u32) >= p.stack.first_arg_offset as u32,
                        "{isa}/{conv}/{}: 栈偏移 {offset} 低于首参偏移 {}",
                        case.name,
                        p.stack.first_arg_offset
                    );
                    ends.push(*offset as u32 + *size as u32);
                }
                if let Placement::Indirect {
                    at: Some(at),
                    on_stack: true,
                    ..
                } = &a.place
                {
                    wants_byval = true;
                    assert!(
                        *at >= 0 && (*at as u32) < p.stack.byval_area_bytes,
                        "{isa}/{conv}/{}: byval 副本偏移 {at} 不在临时区（{} 字节）内",
                        case.name,
                        p.stack.byval_area_bytes
                    );
                }
            }
            // 副本区与传出参数区**分开记账**：有 byval 副本就得有临时区，反之亦然。
            assert_eq!(
                wants_byval,
                p.stack.byval_area_bytes > 0,
                "{isa}/{conv}/{}: byval 临时区记账不一致（{} 字节）",
                case.name,
                p.stack.byval_area_bytes
            );
            for h in [
                &p.hidden.sret,
                &p.hidden.context,
                &p.hidden.va_meta,
                &p.hidden.va_len,
            ]
            .into_iter()
            .flatten()
            {
                regs.push(h.index);
            }
            let mut uniq = regs.clone();
            uniq.sort_unstable();
            uniq.dedup();
            assert_eq!(
                uniq.len(),
                regs.len(),
                "{isa}/{conv}/{}: 同一寄存器被记了两次（{regs:?}）",
                case.name
            );

            // 返回寄存器**单独**校验：它可以与参数同号（AAPCS64 的 x0 既是首个参数也是
            // 返回寄存器、RISC-V 的 a0 同理），但必须是本 ISA 的真实寄存器。
            let mut rets = Vec::new();
            collect_ret_regs(&p.ret, &mut rets);
            for i in rets {
                assert!(
                    i < t.reg_count(),
                    "{isa}/{conv}/{}: 返回寄存器号 {i} 超出寄存器总数",
                    case.name
                );
            }

            // 被保存集 ∩ clobber = ∅，且 clobber 都是可分配的物理寄存器。
            let cs: Vec<u32> = p.callee_saved.regs.iter().map(|r| r.index).collect();
            for c in &p.clobbers {
                assert!(
                    !cs.contains(&c.index),
                    "{isa}/{conv}/{}: {} 既被保存又被列为 clobber",
                    case.name,
                    c.name
                );
                assert!(
                    t.allocatable().contains(&c.index),
                    "{isa}/{conv}/{}: clobber {} 不可分配",
                    case.name,
                    c.name
                );
            }

            // 栈参数区要能装下最后一个字节。
            if let Some(max_end) = ends.iter().max() {
                let need = max_end - p.stack.first_arg_offset as u32 + p.stack.shadow_bytes;
                assert!(
                    p.stack.arg_area_bytes >= need,
                    "{isa}/{conv}/{}: arg_area {} < 需要 {need}",
                    case.name,
                    p.stack.arg_area_bytes
                );
            }
        }
    }
    assert!(checked > 60, "语料覆盖太少（只算了 {checked} 条）");
}

fn collect_regs(p: &Placement, out: &mut Vec<u32>) {
    match p {
        Placement::Reg { reg, .. } => out.push(reg.index),
        Placement::RegPair { lo, hi } => {
            out.push(lo.index);
            out.push(hi.index);
        }
        Placement::RegGroup { regs } => out.extend(regs.iter().map(|r| r.index)),
        Placement::Indirect { ptr, .. } => {
            if let Some(p) = ptr {
                out.push(p.index);
            }
        }
        Placement::Stack { .. } | Placement::Ignore => {}
    }
}

fn collect_ret_regs(r: &RetLoc, out: &mut Vec<u32>) {
    match r {
        RetLoc::Reg { reg } => out.push(reg.index),
        RetLoc::RegPair { lo, hi } => {
            out.push(lo.index);
            out.push(hi.index);
        }
        RetLoc::Indirect { .. } | RetLoc::Void => {}
    }
}

/// 变参：`variadic_stack_only` 的约定把**未命名实参**全赶到栈上；SysV 继续用寄存器。
#[test]
fn variadic_unnamed_arguments_follow_the_convention() {
    let reg = registry();
    // 命名 1 个（i64）+ 未命名 3 个（i64）。
    let sig = Signature::new(
        vec![
            ("a".into(), i64_()),
            ("b".into(), i64_()),
            ("c".into(), i64_()),
            ("d".into(), i64_()),
        ],
        Some(i64_()),
    )
    .variadic(1);

    let p = plan(&reg, "x86_64_v12", "win64", &sig);
    assert!(
        matches!(p.args[0].place, Placement::Reg { .. }),
        "命名参数应在寄存器"
    );
    for a in &p.args[1..] {
        assert!(
            matches!(a.place, Placement::Stack { .. }),
            "win64 的未命名实参必须走栈：{a:?}"
        );
    }
    assert!(p.va_area.is_some(), "变参应有 va_area");

    let p = plan(&reg, "x86_64_v12", "sysv64", &sig);
    assert!(
        matches!(p.args[1].place, Placement::Reg { .. }),
        "SysV 的未命名实参继续用寄存器"
    );
    assert_eq!(
        p.hidden.va_meta.as_ref().map(|r| r.name.as_str()),
        Some("RAX"),
        "SysV 变参要报 `%al`"
    );
}

/// callee-saved 的保存机制来自约定（x86 = push，riscv/arm64 = 存帧内）。
#[test]
fn callee_save_mechanism_comes_from_the_convention() {
    let reg = registry();
    let p = plan(&reg, "x86_64_v12", "win64", &one(i64_()));
    assert_eq!(
        format!("{:?}", p.callee_saved.mechanism),
        "Push",
        "x86 用 push/pop"
    );
    assert!(p.callee_saved.includes_fp, "x86 的 prologue 保存帧指针");
    let names: Vec<&str> = p
        .callee_saved
        .regs
        .iter()
        .map(|r| r.name.as_str())
        .collect();
    assert!(names.contains(&"RBX") && names.contains(&"RDI"));
    assert!(!names.contains(&"RBP"), "RBP 由帧件负责，不进池");

    let p = plan(&reg, "riscv64_v12", "lp64d", &one(i64_()));
    assert_eq!(format!("{:?}", p.callee_saved.mechanism), "StoreToFrame");
    assert!(p.callee_saved.includes_link, "riscv 的 ra 也要保存");
}

// ───────────────── 宿主钩子与"数据表达不了的部分" ─────────────────

/// 一份**自定义**约定：PoC 语言约定（context 寄存器 + 变参长度寄存器 + 被叫方弹栈 +
/// 整数对拆分），用来证明这些格子真的被引擎执行（内置约定都没启用它们）。
const POC_RULES: &str = r#"
name = "poc"
position = "by_class"
int_pool = "int"
float_pool = "float"
stack_align = 16
stack = { slot_bytes = 8, first_offset_slots = 1 }
classify = [
  { when = { kind = "float", size_le = 8 }, do = { direct = { pool = "float" } } },
  { when = { kind = "aggregate", size_le = 16 }, do = { pair = { lo = "int", hi = "int" } } },
  { when = { kind = "scalar", size_le = 8 }, do = { direct = { pool = "int" } } },
]
ret_classify = [
  { when = { kind = "scalar", size_le = 8 }, do = { direct = { pool = "ret_int" } } },
]
hidden = { sret_pool = "sret", sret_slot = 0, context_pool = "ctx", context_slot = 0, va_len_pool = "va_len", va_list = "sysv_reg_save", va_list_size = 24, va_list_align = 8 }
callee_saved = { mechanism = "push", pools = ["cs_gpr"], includes_fp = true }
callee_pop = "sum_stack_args"
variadic_stack_only = true
"#;

const POC_BINDING: &str = r#"
isa = "poc_isa"
conv = "poc"
[pools]
int = ["R1", "R2", "R3"]
float = ["F1", "F2"]
cs_gpr = ["R10"]
ret_int = ["R0"]
sret = ["R8"]
ctx = ["R7"]
va_len = ["R9"]
"#;

/// 最小 `AbiTarget`（PoC 用）：R0-R10，名字即号。
struct PocTarget;

fn poc_target() -> PocTarget {
    PocTarget
}

impl forge_abi::AbiTarget for PocTarget {
    fn isa_name(&self) -> &str {
        "poc_isa"
    }
    fn reg_count(&self) -> u32 {
        11
    }
    fn reg_name(&self, index: u32) -> Option<String> {
        Some(format!("R{index}"))
    }
    fn reg_class_name(&self, _index: u32) -> String {
        "GPR(8)".into()
    }
    fn reg_index(&self, name: &str) -> Option<u32> {
        name.strip_prefix('R')?.parse().ok()
    }
    fn reg_width(&self, _index: u32) -> u8 {
        8
    }
    fn pinned(&self, _index: u32) -> bool {
        false
    }
    fn allocatable(&self) -> Vec<u32> {
        (0..11).collect()
    }
    fn cap(&self, cap: forge_abi::Capability) -> Option<u16> {
        match cap {
            forge_abi::Capability::GprMov => Some(64),
            _ => None,
        }
    }
}

/// 自省钩子：既按方向改分类，又在 plan 出锅后改 clobber。
struct Hook {
    dirs: std::sync::Mutex<Vec<ClassDir>>,
}

impl AbiHooks for Hook {
    fn classify(&self, _rules: &AbiRules, ty: &TyView, dir: ClassDir) -> Option<ClassAction> {
        self.dirs.lock().unwrap().push(dir);
        // 语言规则：16 字节聚合**当参数**按值拆两槽，**当返回**走 sret。
        if ty.is_aggregate() && dir == ClassDir::Ret {
            return Some(ClassAction::Indirect {
                via: forge_abi::IndirectVia::HiddenSret,
            });
        }
        None
    }

    fn adjust_plan(&self, plan: &mut AbiPlan) {
        plan.note = Some("hooked".into());
    }
}

#[test]
fn hooks_can_override_classification_and_adjust_the_plan() {
    let mut reg = AbiRegistry::with_builtin_rules().expect("内置规则");
    reg.insert_rules_toml(POC_RULES).expect("PoC 规则");
    reg.insert_binding_toml(POC_BINDING).expect("PoC 绑定");
    reg.insert_hooks(
        "poc",
        Box::new(Hook {
            dirs: std::sync::Mutex::new(Vec::new()),
        }),
    );
    let t = poc_target();

    // 变参 + context + 返回值：hidden 槽全部落位；未命名实参走栈；被叫方弹栈按栈参字节数。
    let sig = Signature::new(
        vec![("a".into(), i64_()), ("b".into(), f64_())],
        Some(agg_ii()),
    )
    .variadic(1);
    let p = reg.plan(&t, "poc", &sig).expect("plan");
    assert_eq!(
        p.hidden.context.as_ref().map(|r| r.name.as_str()),
        Some("R7")
    );
    assert_eq!(
        p.hidden.va_len.as_ref().map(|r| r.name.as_str()),
        Some("R9")
    );
    assert_eq!(p.note.as_deref(), Some("hooked"));
    // 返回值被钩子改成 sret（独立池 → R8），指针不占用户参数槽。
    assert!(matches!(p.ret, RetLoc::Indirect { .. }));
    assert_eq!(p.hidden.sret.as_ref().map(|r| r.name.as_str()), Some("R8"));
    assert_eq!(place_reg(arg0(&p)), "R1");
    assert!(p.callee_pop_bytes > 0, "SumStackArgs 应算出实际栈参字节数");
}

/// 未注册的约定 / 未注册的绑定 —— fail-closed，且错误消息要指路。
#[test]
fn unregistered_convention_and_binding_fail_closed() {
    let reg = AbiRegistry::with_builtin_rules().expect("内置规则");
    let t = target_for("arm64_v12").unwrap();
    let err = reg.plan(&t, "aapcs64", &one(i64_())).unwrap_err();
    assert!(matches!(err, AbiError::BadRules { .. }), "{err:?}");
    assert!(err.to_string().contains("绑定"), "消息要指出缺绑定：{err}");

    let err = reg.plan(&t, "nosuchconv", &one(i64_())).unwrap_err();
    assert!(err.to_string().contains("未注册"), "{err}");
}

/// 自定义约定的最小可用形态：参数/返回/hidden 都按数据走。
#[test]
fn custom_convention_is_usable_from_data_only() {
    let mut reg = AbiRegistry::new();
    reg.insert_rules_toml(POC_RULES).expect("PoC 规则");
    reg.insert_binding_toml(POC_BINDING).expect("PoC 绑定");
    let t = poc_target();
    let sig = Signature::new(
        vec![
            ("a".into(), i64_()),
            ("b".into(), i64_()),
            ("c".into(), i64_()),
        ],
        Some(i64_()),
    );
    let p = reg.plan(&t, "poc", &sig).expect("plan");
    assert_eq!(place_reg(arg0(&p)), "R1");
    assert_eq!(ret_reg(&p.ret), "R0");
    // `pair { lo = "int", hi = "int" }` 路径。
    let sig = Signature::new(vec![("s".into(), agg_ii())], None);
    let p = reg.plan(&t, "poc", &sig).expect("plan");
    assert_eq!(place_reg(arg0(&p)), "R1:R2");
    // 池耗尽 → 走栈（不是报错）。
    let sig = Signature::new(
        vec![
            ("a".into(), i64_()),
            ("b".into(), i64_()),
            ("c".into(), i64_()),
            ("d".into(), i64_()),
        ],
        None,
    );
    let p = reg.plan(&t, "poc", &sig).expect("plan");
    assert!(matches!(p.args[3].place, Placement::Stack { .. }));
}

/// `AbiBinding` 的具名选择子与索引选择子等价（同一台机器两种写法）。
#[test]
fn binding_selectors_accept_names_and_indices() {
    let t = x86_64_v12();
    let by_name = AbiBinding::from_toml(
        "isa = \"x86_64_v12\"\nconv = \"c\"\n[pools]\nint = [\"RCX\", \"RDX\"]\n",
    )
    .unwrap();
    let by_index =
        AbiBinding::from_toml("isa = \"x86_64_v12\"\nconv = \"c\"\n[pools]\nint = [1, 2]\n")
            .unwrap();
    let a: Vec<u32> = by_name
        .resolve_pool("int", &t)
        .unwrap()
        .iter()
        .map(|r| r.index)
        .collect();
    let b: Vec<u32> = by_index
        .resolve_pool("int", &t)
        .unwrap()
        .iter()
        .map(|r| r.index)
        .collect();
    assert_eq!(a, b);
    assert_eq!(a, vec![1, 2]);
}

/// `Purpose` 的默认值是 `Normal`（未用到的标记不污染快照）。
#[test]
fn normal_purpose_is_the_default_and_not_printed() {
    let reg = registry();
    let p = plan(&reg, "x86_64_v12", "win64", &one(i64_()));
    match arg0(&p) {
        Placement::Reg { purpose, .. } => assert_eq!(*purpose, Purpose::Normal),
        other => panic!("{other:?}"),
    }
}

/// `ArgLoc` 的序号就是形参位置（管线按它对齐调用方/被调方）。
#[test]
fn arg_indices_are_the_parameter_positions() {
    let reg = registry();
    let sig = Signature::new(
        vec![
            ("a".into(), i64_()),
            ("b".into(), f64_()),
            ("c".into(), ptr_()),
        ],
        None,
    );
    let p = plan(&reg, "riscv64_v12", "lp64d", &sig);
    let idx: Vec<Option<usize>> = p.args.iter().map(|a: &ArgLoc| a.index).collect();
    assert_eq!(idx, vec![Some(0), Some(1), Some(2)]);
    assert_eq!(p.args[0].size, 8);
}
