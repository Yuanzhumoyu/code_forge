//! **不变量**：快照能抓"变了"，抓不到"错了"。这里逐条断言 plan 的语义约束，
//! 每一条都对应一个真实约定事实或一处曾经写错的地方。
//!
//! 全部断言只用 `AbiPlan` 的公开数据；不碰生成器、不跑机器码（那是 A3+ 的事）。

mod common;

use common::*;
use forge_abi::{
    AbiBinding, AbiError, AbiHooks, AbiPlan, AbiRegistry, AbiRules, AbiTarget, ArgLoc, ClassAction,
    ClassDir, DeclAttrs, Extension, Placement, Purpose, RetLoc, Signature, TyView,
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

            // 栈参数区要能装下最后一个字节。落点偏移是**被调方视角**（已含 shadow），
            // 而 `arg_area_bytes = 参数区 + shadow` —— 所以两边的差就是 `first_arg_offset`。
            if let Some(max_end) = ends.iter().max() {
                let need = max_end - p.stack.first_arg_offset as u32;
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
        RetLoc::RegGroup { regs } => out.extend(regs.iter().map(|r| r.index)),
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
hidden = { sret_pool = "sret", sret_slot = 0, context_pool = "ctx", context_slot = 0, va_len_pool = "va_len", va_list = "sysv_reg_save" }
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

/// **声明属性真的改变规划**（v20 A2b）——不是装饰：
///
/// - `byval(N)` → 该形参变"调用方栈上副本 + 指针"（`Indirect{on_stack}`），副本进 byval 区；
/// - `sret` → 该形参占约定声明的 **hidden sret 槽**（不再按普通参数分类），并记进 `hidden.sret`；
/// - `zeroext`/`signext` → 落点带 `Extension`；
/// - `inreg` → 分类说走栈时再试一次寄存器；
/// - `align(N)` → 栈落点对齐抬到 N。
#[test]
fn declared_attributes_change_the_plan() {
    let reg = registry();

    // ① byval(24)：win64 上 24B 聚合本来就 byval；这里用 i64 标量做对照——无属性时落寄存器，
    //    声明 byval 后必须变成"栈上副本 + 指针"。
    let plain = plan(&reg, "x86_64_v12", "win64", &one(i64_()));
    assert_eq!(place_reg(arg0(&plain)), "RCX");
    let byval_sig = Signature::new(vec![("p".into(), i64_())], None).with_attrs(vec![DeclAttrs {
        byval: Some(24),
        ..DeclAttrs::default()
    }]);
    let p = plan(&reg, "x86_64_v12", "win64", &byval_sig);
    match arg0(&p) {
        Placement::Indirect {
            ptr,
            at: Some(0),
            on_stack: true,
        } => assert_eq!(ptr.as_ref().map(|r| r.name.as_str()), Some("RCX")),
        other => panic!("byval 应变成栈上副本 + 指针，实际 {other:?}"),
    }
    assert_eq!(p.stack.byval_area_bytes, 24, "副本区要按声明的 N 字节算");

    // ② sret：形参占 hidden sret 槽（win64 = RCX；AAPCS64 = **x8**，本片 arm64 无浮点池不影响）。
    let sret_sig = Signature::new(vec![("out".into(), ptr_())], None).with_attrs(vec![DeclAttrs {
        sret: true,
        ..DeclAttrs::default()
    }]);
    for (isa, conv, want) in [
        ("x86_64_v12", "win64", "RCX"),
        ("riscv64_v12", "lp64d", "X10"),
    ] {
        let p = plan(&reg, isa, conv, &sret_sig);
        assert_eq!(
            p.hidden.sret.as_ref().map(|r| r.name.as_str()),
            Some(want),
            "{conv} 的 sret 槽"
        );
        // 形参本身是"间接"落点（指针在 sret 槽里），不是普通寄存器参数。
        match arg0(&p) {
            Placement::Indirect {
                ptr: Some(reg),
                at: None,
                on_stack: false,
            } => assert_eq!(reg.name, want, "{conv}：sret 指针所在寄存器"),
            other => panic!("{conv}：`sret` 形参应是间接落点，实际 {other:?}"),
        }
        // 用户实参不能与 sret 指针撞号：下一参数从别的槽开始。
        let two = Signature::new(vec![("out".into(), ptr_()), ("x".into(), i64_())], None)
            .with_attrs(vec![
                DeclAttrs {
                    sret: true,
                    ..DeclAttrs::default()
                },
                DeclAttrs::default(),
            ]);
        let p = plan(&reg, isa, conv, &two);
        assert_ne!(
            place_reg(&p.args[1].place),
            want,
            "{conv}：第二个参数不能撞 sret 槽"
        );
    }

    // ③ 扩展属性折进落点。
    for (attrs, want) in [
        (
            DeclAttrs {
                zeroext: true,
                ..DeclAttrs::default()
            },
            Extension::ZeroExt,
        ),
        (
            DeclAttrs {
                signext: true,
                ..DeclAttrs::default()
            },
            Extension::SignExt,
        ),
    ] {
        let sig = Signature::new(vec![("v".into(), i32_())], None).with_attrs(vec![attrs]);
        let p = plan(&reg, "x86_64_v12", "win64", &sig);
        match arg0(&p) {
            Placement::Reg { ext, .. } => assert_eq!(*ext, want),
            other => panic!("{other:?}"),
        }
    }
    // signext 优先（两个都写时按 LLVM 语义取 signext）。
    let both = DeclAttrs {
        zeroext: true,
        signext: true,
        ..DeclAttrs::default()
    };
    assert_eq!(both.extension(), Extension::SignExt);

    // ④ align(32)：栈落点对齐抬到 32。
    let sig = Signature::new(vec![("s".into(), i64_())], None).with_attrs(vec![DeclAttrs {
        align: Some(32),
        ..DeclAttrs::default()
    }]);
    // riscv 的 int 池只有 8 个槽，9 个参数就把第 9 个挤到栈上。
    let mut params: Vec<(String, TyView)> = (0..8).map(|i| (format!("a{i}"), i64_())).collect();
    params.push(("s".into(), i64_()));
    let mut attrs = vec![DeclAttrs::default(); 8];
    attrs.push(DeclAttrs {
        align: Some(32),
        ..DeclAttrs::default()
    });
    let sig9 = Signature::new(params, None).with_attrs(attrs);
    let p = plan(&reg, "riscv64_v12", "lp64d", &sig9);
    match &p.args[8].place {
        Placement::Stack { align, .. } => assert_eq!(*align, 32, "声明的对齐要生效"),
        other => panic!("{other:?}"),
    }
    let _ = sig;
}

/// `inreg` 让"分类说要走栈"的参数改走寄存器（池够时）。
#[test]
fn inreg_overrides_a_stack_classification() {
    let reg = registry();
    // win64 的 byval 规则会把 >8B 聚合判成"栈上副本"（indirect），`inreg` 不该改它；
    // 这里用**第 5 个 int 参数**（分类说走栈）来验：`inreg` 时优先寄存器。
    let mut params: Vec<(String, TyView)> = (0..4).map(|i| (format!("a{i}"), i64_())).collect();
    params.push(("x".into(), i64_()));
    let plain = plan(
        &reg,
        "x86_64_v12",
        "win64",
        &Signature::new(params.clone(), None),
    );
    assert!(
        matches!(plain.args[4].place, Placement::Stack { .. }),
        "win64 第 5 个 int 参数走栈"
    );
    let mut attrs = vec![DeclAttrs::default(); 4];
    attrs.push(DeclAttrs {
        inreg: true,
        ..DeclAttrs::default()
    });
    // win64 的 int 池只有 4 个槽且已耗尽 ⇒ `inreg` 也只能走栈（不静默换寄存器）。
    let p = plan(
        &reg,
        "x86_64_v12",
        "win64",
        &Signature::new(params.clone(), None).with_attrs(attrs),
    );
    assert!(
        matches!(p.args[4].place, Placement::Stack { .. }),
        "池耗尽时 inreg 不硬凑"
    );

    // sysv64 的 int 池有 6 个槽：第 7 个参数走栈、声明 inreg 后抢到寄存器（本测试只取前者）。
    let many: Vec<(String, TyView)> = (0..6).map(|i| (format!("a{i}"), i64_())).collect();
    let p = plan(&reg, "x86_64_v12", "sysv64", &Signature::new(many, None));
    assert!(matches!(p.args[5].place, Placement::Reg { .. }));
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

// ─────────────── 多值返回（v20 A6：`rets.len() > 1`）───────────────

/// **每个返回值各自分类、各自占一个返回寄存器**——这是"多值返回"的全部语义。
///
/// 为什么值得单独钉：旧实现把"第二个返回寄存器"写死成**类内号 1**（x86 的 RDX），
/// 换个 ISA 就错（riscv 的类内号 1 是 X1 = ra，返回槽是 X10/X11）。现在落点由绑定的
/// `ret_int`/`ret_float` 池给。
#[test]
fn multi_value_returns_take_one_register_each() {
    let reg = registry();

    // win64：两个独立标量 → RAX:RDX（与 forge-rustc 的 ScalarPair IR 形态一致）。
    {
        let sig = Signature::with_rets(vec![], vec![i64_(), i64_()]);
        match plan(&reg, "x86_64_v12", "win64", &sig).ret {
            RetLoc::RegPair { lo, hi } => {
                assert_eq!((lo.name.as_str(), hi.name.as_str()), ("RAX", "RDX"))
            }
            other => panic!("{other:?}"),
        }
    }
    // lp64d：同一个签名在 riscv 上落到 X10:X11（**不是 X0:X1**）。
    {
        let sig = Signature::with_rets(vec![], vec![i64_(), i64_()]);
        match plan(&reg, "riscv64_v12", "lp64d", &sig).ret {
            RetLoc::RegPair { lo, hi } => {
                assert_eq!((lo.name.as_str(), hi.name.as_str()), ("X10", "X11"))
            }
            other => panic!("{other:?}"),
        }
    }
    // aapcs64：**四个** f32 返回 → V0..V3（`RegGroup`；AAPCS64 的浮点返回池有 4 个槽）。
    {
        let sig = Signature::with_rets(vec![], vec![f32_(), f32_(), f32_(), f32_()]);
        match plan(&reg, "arm64_v12", "aapcs64", &sig).ret {
            RetLoc::RegGroup { regs } => assert_eq!(
                regs.iter().map(|r| r.name.as_str()).collect::<Vec<_>>(),
                ["V0", "V1", "V2", "V3"]
            ),
            other => panic!("{other:?}"),
        }
    }
    // 混合类：int + float 各取自己那一池（Win64 按位置共享游标 ⇒ XMM1）。
    {
        let sig = Signature::with_rets(vec![], vec![i64_(), f64_()]);
        match plan(&reg, "x86_64_v12", "win64", &sig).ret {
            RetLoc::RegPair { lo, hi } => {
                assert_eq!(
                    (lo.name.as_str(), hi.name.as_str()),
                    ("RAX", "XMM1"),
                    "浮点返回走自己的池，且位置游标与整数共享"
                )
            }
            other => panic!("{other:?}"),
        }
    }
}

/// 多值返回的**边界**同样是 fail-closed（明确拒绝，不猜落点）：
/// ① 池不够（Win64 的返回池只有 RAX:RDX，三个独立标量没有第三个返回寄存器）；
/// ② 分量是聚合（多槽类型属于"单值聚合"路径，不在多值返回里拆）。
#[test]
fn multi_value_returns_fail_closed_on_gaps() {
    let reg = registry();
    let t = target_for("x86_64_v12").expect("合成目标");

    let three = Signature::with_rets(vec![], vec![i64_(), i64_(), i64_()]);
    let e = reg.plan(&t, "win64", &three).unwrap_err();
    assert!(matches!(e, AbiError::PoolExhausted { .. }), "{e:?}");

    let with_agg = Signature::with_rets(vec![], vec![i64_(), agg24()]);
    let e = reg.plan(&t, "win64", &with_agg).unwrap_err();
    assert!(matches!(e, AbiError::Unsupported { .. }), "{e:?}");
}

/// **变参形状的文档 ↔ 引擎一致性**（v20 A8）：`docs/plans/varargs-plan.md` §2 那张表
/// （四份内置约定的 `va_list` 形态）必须与引擎实际算出来的 `AbiPlan` 逐格相同。
///
/// 这张表的价值在于它是"变参现在做到哪一步"的索引——写歪了就会把后来的人引错，
/// 所以用引擎输出钉住它（同 `schema_guard` 对键表的做法）。
#[test]
fn va_shapes_match_the_documented_table() {
    let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../../docs/plans/varargs-plan.md");
    let doc =
        std::fs::read_to_string(&path).unwrap_or_else(|e| panic!("读不到 {}：{e}", path.display()));

    // 表行：| `conv` | `形状预置名` | size | align | 只走栈/继续用寄存器 | `REG`/— |
    // 形状是**数据**（`hidden.va_list` 的预置名或显式表名），不再是枚举变体。
    let shape_name = |va: &forge_abi::plan::VaArea| va.shape.clone().unwrap_or_else(|| "—".into());

    // 命名 1 个 + 未命名 1 个：足以触发出 va_area 与 va_meta。
    let sig = Signature::new(vec![("a".into(), i64_()), ("b".into(), i64_())], None).variadic(1);

    let reg = registry();
    let cases = [
        ("win64", "x86_64_v12"),
        ("sysv64", "x86_64_v12"),
        ("aapcs64", "arm64_v12"),
        ("lp64d", "riscv64_v12"),
    ];

    let mut checked = 0usize;
    for (conv, isa) in cases {
        let p = plan(&reg, isa, conv, &sig);
        let va = p
            .va_area
            .as_ref()
            .unwrap_or_else(|| panic!("{conv}: 变参应有 va_area"));
        // 表行：`| \`conv\` | \`shape\` | size | align | 只走栈/继续用寄存器 | \`REG\`/— |`
        let row = doc
            .lines()
            .find(|l| l.starts_with(&format!("| `{conv}` |")))
            .unwrap_or_else(|| panic!("varargs-plan.md §2 缺 `{conv}` 一行"));
        let cells: Vec<&str> = row.split('|').map(|c| c.trim()).collect();
        let want = [
            format!("`{}`", shape_name(va)),
            va.size.to_string(),
            va.align.to_string(),
            if va.stack_only {
                "只走栈"
            } else {
                "继续用寄存器"
            }
            .to_string(),
        ];
        for (i, w) in want.iter().enumerate() {
            assert_eq!(
                cells[2 + i],
                *w,
                "{conv} 第 {} 列：文档与引擎不一致（整行：{row}）",
                i + 1
            );
        }
        // va_meta 列：有寄存器就写名字（含反引号），否则 `—`。
        let want_meta = match p.hidden.va_meta.as_ref() {
            Some(r) => format!("`{}`", r.name),
            None => "—".to_string(),
        };
        assert_eq!(
            cells[6], want_meta,
            "{conv} 的 va_meta 列：文档与引擎不一致（整行：{row}）"
        );
        checked += 1;
    }
    assert_eq!(checked, 4, "四份内置约定都要核对到");
}

/// **`va_list` 对象的形状**（v20 变参 V3）：字段布局 + 寄存器保存区必须与 **psABI 的数字**
/// 逐条一致——`va_arg` 的游标按这些槽宽/偏移推进，算错就是**静默错值**（不是编译错误）。
///
/// 这里特意**不拿"寄存器类宽"当槽宽**：psABI 规定的是保存区的槽宽。实测教训：
/// 按绑定宽度算会得到 arm64 `128`（应 `192`：V 槽恒 16 字节）、riscv `96`（应 `128`）。
#[test]
fn va_object_layout_matches_the_psabi_numbers() {
    let reg = registry();
    let sig = Signature::new(vec![("a".into(), i64_())], None).variadic(1);
    // (约定, ISA, 字段 "名@偏移+宽", 保存区 (size, align, GP 数, FP 数, GP 槽宽, FP 槽宽))
    #[allow(clippy::type_complexity)]
    let cases: [(
        &str,
        &str,
        Vec<&str>,
        Option<(u32, u32, usize, usize, u32, u32)>,
    ); 4] = [
        ("win64", "x86_64_v12", vec!["cursor@0+8"], None),
        (
            "sysv64",
            "x86_64_v12",
            vec![
                "gp_offset@0+4",
                "fp_offset@4+4",
                "overflow_arg_area@8+8",
                "reg_save_area@16+8",
            ],
            Some((176, 16, 6, 8, 8, 16)),
        ),
        (
            "aapcs64",
            "arm64_v12",
            vec![
                "__stack@0+8",
                "__gr_top@8+8",
                "__vr_top@16+8",
                "__gr_offs@24+4",
                "__vr_offs@28+4",
            ],
            Some((192, 16, 8, 8, 8, 16)),
        ),
        (
            "lp64d",
            "riscv64_v12",
            vec!["area@0+8"],
            Some((128, 8, 8, 8, 8, 8)),
        ),
    ];
    for (conv, isa, fields, save) in cases {
        let p = plan(&reg, isa, conv, &sig);
        let va = p
            .va_area
            .as_ref()
            .unwrap_or_else(|| panic!("{conv}: 变参应有 va_area"));
        let got: Vec<String> = va
            .fields
            .iter()
            .map(|f| format!("{}@{}+{}", f.name, f.offset, f.size))
            .collect();
        assert_eq!(got, fields, "{conv}: `va_list` 字段布局与 psABI 不一致");
        match save {
            None => assert!(
                va.save.is_none(),
                "{conv}: 该形态不需要寄存器保存区（未命名实参只在栈上）"
            ),
            Some((size, align, n_gp, n_fp, gp_slot, fp_slot)) => {
                let s = va
                    .save
                    .as_ref()
                    .unwrap_or_else(|| panic!("{conv}: 该形态需要寄存器保存区"));
                assert_eq!(
                    (s.size, s.align, s.slots.len()),
                    (size, align, n_gp + n_fp),
                    "{conv}: 保存区大小/对齐/槽数"
                );
                for (i, slot) in s.slots.iter().enumerate() {
                    let (want_off, want_size) = if i < n_gp {
                        (i as u32 * gp_slot, gp_slot)
                    } else {
                        (n_gp as u32 * gp_slot + (i - n_gp) as u32 * fp_slot, fp_slot)
                    };
                    assert_eq!(
                        (slot.offset, slot.size),
                        (want_off, want_size),
                        "{conv}: 第 {i} 个保存区槽（{}）的偏移/宽度",
                        slot.reg.name
                    );
                }
            }
        }
    }
}
