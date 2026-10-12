//! S8a 结构不变量（v18 S8）：`MachineInst for Inst` 必须是**表驱动**的。
//!
//! S8a 把 `uses`/`defs`/`use_constraints`/`def_constraints`/`effects`/`reg_field`/
//! `set_reg_field`/`is_reg_field_settable` 这 8 个"每指令一条臂"的方法换成了
//! 一张 `__SHAPES` 形状表 + 三个通用字段访问器（x86 −107 KB、riscv −69 KB、
//! arm64 −51 KB 生成物）。这里钉住两件事：
//!
//! 1. **这 8 个方法体里不许再出现任何 `Inst::` 臂**——退化成逐指令展开会立刻红；
//! 2. **形状表与两个字段访问器覆盖同一批变体**，且行数 = 变体数——漏掉一个变体
//!    会让该指令的 `uses`/`defs`/`reg_field` 静默返回空值（最危险的失败模式）。
//!
//! 尺寸本身不在这里断言（那属于度量，见 `docs/performance/bench_baseline.md`）。

use std::collections::BTreeSet;

use forge_isa_dsl::{ExpandOptions, Parts, expand_file};

/// `MachineInst` 里必须是通用实现（循环）而非 `match` 分派的方法。
///
/// v23 M4 批次 B 起，`uses`/`defs`/`use_constraints`/`def_constraints` **不再由生成物发射**
/// ——它们由 `operands()` 派生（缺省实现），清单随之缩到 5 个；同时新增
/// [`DERIVED_FROM_OPERANDS`] 检查"这 4 个不许再出现在生成物里"（回潮守卫）。
const TABLE_DRIVEN: [&str; 5] = [
    "fn effects",
    "fn operands",
    "fn reg_field",
    "fn set_reg_field",
    "fn is_reg_field_settable",
];

/// v23 M4：这些方法由 `MachineInst::operands` 的**缺省实现**派生，生成物里**不得**再出现
/// ——每多发射一份就是"同一事实两处表示"（改一处忘一处 = 静默错值）。
const DERIVED_FROM_OPERANDS: [&str; 4] = [
    "fn uses",
    "fn defs",
    "fn use_constraints",
    "fn def_constraints",
];

const SPECS: [(&str, &str); 3] = [
    ("x86", "isa/x86.toml"),
    ("riscv64", "isa/riscv64.toml"),
    ("arm64", "isa/arm64.toml"),
];

/// 只生成 TargetMachine 集成层（含 `impl MachineInst for Inst`）。
fn tm_text(path: &str) -> String {
    let opts = ExpandOptions {
        spec_tests: false,
        name: None,
        parts: Parts {
            encode: false,
            decode: false,
            asm: false,
            tm: true,
        },
        params: Default::default(),
    };
    expand_file(path, &opts)
        .unwrap_or_else(|e| panic!("展开 {path} 失败：{e}"))
        .to_string()
}

/// 取一个方法体：从 `sig` 到下一个 `fn ` 之前（生成物里这些方法依次紧邻）。
fn method_body<'a>(text: &'a str, sig: &str) -> &'a str {
    let i = text
        .find(sig)
        .unwrap_or_else(|| panic!("生成物里找不到 `{sig}`"));
    let rest = &text[i + sig.len()..];
    let j = rest
        .find("fn ")
        .unwrap_or_else(|| panic!("`{sig}` 之后找不到下一个 `fn `"));
    &text[i..i + sig.len() + j]
}

/// 文本里出现的 `Inst :: <Ident>` 变体名集合。
fn variants(body: &str) -> BTreeSet<String> {
    let mut set = BTreeSet::new();
    let mut rest = body;
    while let Some(i) = rest.find("Inst ::") {
        rest = rest[i + "Inst ::".len()..].trim_start();
        let name: String = rest
            .chars()
            .take_while(|c| c.is_alphanumeric() || *c == '_')
            .collect();
        if !name.is_empty() {
            set.insert(name);
        }
    }
    set
}

#[test]
fn machine_inst_methods_are_table_driven() {
    for (name, path) in SPECS {
        let text = tm_text(path);
        for sig in TABLE_DRIVEN {
            let body = method_body(&text, sig);
            let arms = body.matches("Inst ::").count();
            assert_eq!(
                arms, 0,
                "{name}: `{sig}` 里出现 {arms} 条逐指令臂——S8a 之后这里必须是通用循环，\
                 形状信息只放 `__SHAPES`（回退会重新长出近百 KB 生成代码）"
            );
        }
    }
}

/// **v23 M4 回潮守卫**：由 `operands()` 派生的那 4 个方法**不许**再出现在生成物里。
///
/// 判据是"生成物里没有这个方法体"，而不是"值对不对"——值对不对由
/// `operand_model_guard`（三谱全部指令变体逐条对拍）守。
#[test]
fn derived_methods_are_not_emitted_any_more() {
    for (name, path) in SPECS {
        let text = tm_text(path);
        for sig in DERIVED_FROM_OPERANDS {
            assert!(
                !text.contains(sig),
                "{name}: 生成物里又出现了 `{sig}`——它应由 `MachineInst::operands` 的缺省实现\
                 派生（v23 M4）；再发射一份就是「同一事实两处表示」（改一处忘一处 = 静默错值）"
            );
        }
        assert!(
            text.contains("fn operands"),
            "{name}: 生成物里没有 `operands()`——单一操作数模型没发射"
        );
    }
}

#[test]
fn shape_table_covers_every_variant() {
    for (name, path) in SPECS {
        let text = tm_text(path);
        assert!(
            text.contains("static __SHAPES"),
            "{name}: 生成物里没有 `__SHAPES` 形状表"
        );
        assert!(
            text.contains("static __SLOT_CLASSES") && text.contains("static __EFFECT_SETS"),
            "{name}: 生成物里缺少去重后的槽类表 / effect 序列表"
        );

        let shape_arms = variants(method_body(&text, "fn __shape"));
        let slot_arms = variants(method_body(&text, "fn __reg_slot"));
        let set_arms = variants(method_body(&text, "fn __set_reg_slot"));

        assert!(
            shape_arms.contains("Raw"),
            "{name}: 形状表的变体集合里没有 `Raw`"
        );
        assert_eq!(
            shape_arms, slot_arms,
            "{name}: 形状表与 `__reg_slot` 覆盖的变体不一致——缺的那条指令 `uses`/`defs`/`reg_field` 会静默变空"
        );
        assert_eq!(
            shape_arms, set_arms,
            "{name}: 形状表与 `__set_reg_slot` 覆盖的变体不一致——缺的那条指令 `set_reg_field` 会静默失效"
        );

        // 行字面量形如 `__Shape { uses : … }`；结构体定义（`struct __Shape { pub uses`）
        // 与访问器返回类型（`-> & 'static __Shape {`）都不会命中这个串。
        let rows = text.matches("__Shape { uses").count();
        assert_eq!(
            rows,
            shape_arms.len(),
            "{name}: `__SHAPES` 行数 {rows} ≠ 变体数 {}",
            shape_arms.len()
        );
    }
}
