//! S8b-1 结构不变量（v18 S8）：谓词属性绑定块**只发射一次**。
//!
//! `__a_*`（9 个 `Option<i64>` 预计算）+ `__attr` 闭包共约 2.5 KB，只与
//! `args`/`results`/`ctx` 有关、**与 op 无关**，因此必须放在 `match op` 之前发射一次。
//! 历史实现把它塞进每个 op 臂：x86 有 100 个 op 臂 → 243 KB 的逐字节重复
//!（实测 `tm` 770,348 → 511,774 B，−33.6%）。
//!
//! 这里钉住"每个生成模块只出现一次"；哪天有人再让它跟着 op 臂走，这条会立刻红。

use forge_isa_dsl::{ExpandOptions, Parts, expand_file};

const SPECS: [(&str, &str); 3] = [
    ("x86_v12", "isa/x86_v12.toml"),
    ("riscv64_v12", "isa/riscv64_v12.toml"),
    ("arm64_v12", "isa/arm64_v12.toml"),
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
fn lowering_attrs_emitted_once() {
    for (name, path) in SPECS {
        let text = tm_text(path);
        // 属性绑定块的两个标志性声明（token 文本，见 `gen_lowering_attrs`）。
        for probe in [
            "let __a_rd = results . first ()",
            "let __attr = | name : & str |",
        ] {
            let n = text.matches(probe).count();
            assert_eq!(
                n, 1,
                "{name}: `{probe} …` 出现 {n} 次——属性绑定块必须整块发射一次、放在 `match op` \
                 之前（跟着 op 臂走 = 每个 op 重复一份，x86 曾因此多 243 KB）"
            );
        }
        // 有多个 op 臂才算真的钉住了（否则"只出现一次"是废话）。
        let arms = text.matches("let mut __pack = crate :: prelude :: InstPacket :: new ()").count();
        assert!(
            arms > 1,
            "{name}: 只找到 {arms} 个 lowering op 臂，测试失去意义"
        );
    }
}
