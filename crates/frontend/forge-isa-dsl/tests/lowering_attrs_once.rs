//! 谓词属性源的结构不变量（v18 S8b-1 + S8d）。
//!
//! S8b-1 把属性块从**每个 op 臂**里移出来（x86 曾 100 份逐字节相同的 2.5 KB）；
//! S8d 又把它从"每次 `lower_inst` 先把 9 个属性全算一遍 + `match name { "rd" => … }`
//! 字符串分派"改成**按需 + 无分派**：
//!
//! - `__AC`（`done` 位图 + 取值表）+ `__ac_get` + 每个核心属性一个 `#[inline]` 助手
//!   `__a_<属性>(op, args, results, ctx)`，整份生成物里**各只发射一次**；
//! - 谓词在使用点直接展开成 `__ac_get(&mut __ac, 槽, || __a_<属性>(…))`——
//!   用不到的属性一次都不算（旧实现每次调用都把 9 个全算一遍）；
//! - `__attr` / `__attr_core` 这两个运行时名字分派闭包**彻底消失**。
//!
//! 另外钉住"不用 `{cc}` 的规则不发射 `let __cc: u8 = 0;`"（死代码）。

use forge_isa_dsl::{ExpandOptions, Parts, expand_file};

const SPECS: [(&str, &str); 3] = [
    ("x86_v12", "isa/x86_v12.toml"),
    ("riscv64_v12", "isa/riscv64_v12.toml"),
    ("arm64_v12", "isa/arm64_v12.toml"),
];

/// 与 `v12/pred.rs::PRED_ATTRS` 一一对应的核心属性（槽序即此序）。
const ATTRS: [&str; 9] = [
    "rd",
    "rs1_width",
    "rs2_width",
    "rd_vec",
    "rs1_vec",
    "elem",
    "cond",
    "imm0",
    "iconst",
];

fn tm_text(path: &str) -> String {
    let opts = ExpandOptions {
        krate: None,
        spec_tests: false,
        name: None,
        parts: Parts {
            encode: false,
            decode: false,
            asm: false,
            tm: true,
        },
    };
    expand_file(path, &opts)
        .unwrap_or_else(|e| panic!("展开 {path} 失败：{e}"))
        .to_string()
}

#[test]
fn predicate_attrs_are_lazy_and_emitted_once() {
    for (name, path) in SPECS {
        let text = tm_text(path);

        // ① 运行时名字分派必须彻底消失。
        for dead in ["__attr", "__attr_core"] {
            assert_eq!(
                text.matches(dead).count(),
                0,
                "{name}: 生成物里还有 `{dead}`——属性必须在生成期解析成具体助手调用，\
                 不能再走 `match name {{ … }}` 字符串分派"
            );
        }

        // ② 缓存类型 + 构造 + 9 个属性方法，各只发射一次（9 个方法各一次 ⇒ impl 只有一份）。
        for item in [
            "struct __AC <",
            "let mut __ac = __AC :: new (op , args , results)",
        ] {
            assert_eq!(
                text.matches(item).count(),
                1,
                "{name}: `{item} …` 应只发射一次（跟着 op 臂走 = 每个 op 重复一份）"
            );
        }
        for a in ATTRS {
            let f = format!("fn {a} (& mut self , ctx : ");
            assert_eq!(
                text.matches(&f).count(),
                1,
                "{name}: 属性方法 `{f} …` 应只发射一次"
            );
        }

        // ③ 谓词在使用点直接取属性方法（有 `when` 就必须是这样取的）。
        let calls = text.matches("__ac . ").count();
        assert!(
            calls > 0,
            "{name}: 生成物里没有任何 `__ac . <属性>` 调用——谓词是怎么取属性的？"
        );

        // ④ 不用 `{cc}` 的规则不许留 `let __cc: u8 = 0;` 死代码。
        let dead_cc = text.matches("let __cc : u8 = 0 ;").count();
        let real_cc = text.matches("let __cc : u8 = match").count();
        assert_eq!(
            dead_cc, 0,
            "{name}: 还有 {dead_cc} 处 `let __cc: u8 = 0;`（不用 `{{cc}}` 的规则不该有）；\
             真用 `{{cc}}` 的规则是 {real_cc} 处"
        );
    }
}
