//! v20 A3b-2b-2a 结构不变量：`@move_args` 的**布局路径确实被发射**。
//!
//! 收参的来源寄存器以前是**生成器自己数**出来的（"第几个 int 槽"、`sret` 偏移、
//! 按位置还是按类）——三处硬编码对不上就出错（旧实现里 sret 永远取首 int 槽）。
//! v20 改成读 `AllocResult::call_layout`（管线由 `AbiPlan` 折出来的**中性数据**），
//! 本文件钉住这条路径在**三份发行谱**里都存在且**排在**旧路径之前：
//!
//! - `__layout_ok`：入场判定——只有"每个参数都落在本片支持的落点"时才启用，
//!   半覆盖的形态整函数退回旧路径（两条路径不混用，游标不会错位）；
//! - `call_layout`：数据来源（`None` = 没接上/算不出 ⇒ 旧路径）；
//! - `ArgPlace :: Reg`：来源由布局给出（**类 + 类内号**），生成器不再数槽位。
//!
//! 顺序也要钉住：布局块必须排在旧的 `param_by_ref` 判定**之前**，否则旧路径先
//! `continue`，布局永远轮不到（这种"接了但没生效"的退步只有文字级守卫抓得到）。

use forge_isa_dsl::{ExpandOptions, Parts, expand_file};

const SPECS: [(&str, &str); 3] = [
    ("x86_v12", "isa/x86_v12.toml"),
    ("riscv64_v12", "isa/riscv64_v12.toml"),
    ("arm64_v12", "isa/arm64_v12.toml"),
];

fn text_of(path: &str) -> String {
    let opts = ExpandOptions {
        spec_tests: false,
        name: None,
        parts: Parts::all(),
        params: Default::default(),
    };
    expand_file(path, &opts)
        .unwrap_or_else(|e| panic!("展开 {path} 失败：{e}"))
        .to_string()
}

/// 三份发行谱都发射了布局路径，且它排在旧路径之前。
#[test]
fn move_args_emits_the_layout_driven_receive_path() {
    for (name, path) in SPECS {
        let text = text_of(path);
        for needle in ["__layout_ok", "call_layout", "ArgPlace :: Reg"] {
            assert!(
                text.contains(needle),
                "{name}: 生成物里没有 `{needle}`——布局驱动的收参没发射"
            );
        }
        let layout_at = text.find("__layout_ok").expect("布局判定");
        let old_at = text.find("param_by_ref").unwrap_or_else(|| {
            panic!("{name}: 旧路径（[abi] 收参）不见了——它仍是未覆盖落点的回退")
        });
        assert!(
            layout_at < old_at,
            "{name}: 布局块排在旧路径之后（`__layout_ok` @ {layout_at} > `param_by_ref` @ {old_at}）\
             ——旧路径先 continue，布局永远轮不到"
        );
    }
}

/// 布局路径只认**中性事实**：类（`is_int`）与布局给的号，不再出现"某台机器的寄存器名"。
///
/// 反向指标同样重要——"接了但按 ISA 名分支"等于把别家的约定写回生成器。
#[test]
fn the_layout_path_stays_machine_neutral() {
    let text = text_of("isa/x86_v12.toml");
    let start = text.find("__layout_ok").expect("布局判定");
    let block = &text[start..text.len().min(start + 4000)];
    for needle in ["is_int ()", "is_fp ()"] {
        assert!(
            block.contains(needle),
            "布局路径应只按寄存器**类**分派（`class.{needle}`），实际片段：{block:.600}"
        );
    }
    // 旧路径靠 `[abi.arg_class]` 的寄存器名表；布局路径读布局，不该再引用进程里
    // 那张表（`<Reg>` 字面量枚举名只允许出现在旧的 `move_args` 段里）。
    for needle in ["int_regs", "float_regs"] {
        assert!(
            !block.contains(needle),
            "布局路径里出现了 `{needle}`——那是 [abi] 表的产物，布局路径应只读布局"
        );
    }
}
