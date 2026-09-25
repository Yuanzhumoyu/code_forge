//! v20 A3b-2b-2a / 2b-2b 结构不变量：**收参由生成器发射，谱不再定义调用约定**。
//!
//! 收参的来源寄存器以前是**生成器自己数**出来的（"第几个 int 槽"、`sret` 偏移、
//! 按位置还是按类）——三处硬编码对不上就出错（旧实现里 sret 永远取首 int 槽）。
//! v20 改成读 `AllocResult::call_layout`（管线由 `AbiPlan` 折出来的**中性数据**），
//! 本文件钉住四件事：
//!
//! 1. **布局路径确实发射**（三份发行谱都有），且 `__layout_ok` / `call_layout` /
//!    `ArgPlace :: Reg` 三样俱全；
//! 2. **排在旧路径之前**：布局块必须在旧的 `param_by_ref` 判定前，否则旧路径先
//!    `continue`，布局永远轮不到（"接了但没生效"只有文字级守卫抓得到）；
//! 3. **`@move_args` 已从谱面撤出**：写了它必须明确报错（不是静默忽略），
//!    而**缺席的序言模板**照样发射收参；
//! 4. **位置**：收参在 callee-saved 保存之后、帧分配之前（顺序错了会保存实参值而不是
//!    调用者的寄存器值）。

use std::path::Path;

use forge_isa_dsl::{ExpandOptions, Parts, expand_file, read_isa_file, validate_source};

const SPECS: [(&str, &str); 3] = [
    ("x86_v12", "isa/x86_v12.toml"),
    ("riscv64_v12", "isa/riscv64_v12.toml"),
    ("arm64_v12", "isa/arm64_v12.toml"),
];

/// 夹具：`[emit.prologue]` 已删除（收参不靠模板）。
const FIXTURE_NO_PROLOGUE: &str = "crates/backend/forge-codegen/tests/isa/demo_v12.toml";

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

/// **序/尾声模板与伪指令已从谱面撤出**：写回去必须**明确报错**（静默忽略 = "谱里写了
/// 却没人读"，正是这次重设计要消灭的东西）。
#[test]
fn writing_a_prologue_template_again_is_rejected() {
    let src = read_isa_file("isa/x86_v12.toml").expect("读 x86 谱").0;
    // 恢复"手写序言"的写法：加回 `[emit.prologue]`（含当初的伪指令）。
    let mutated = format!(
        "{src}\n[emit.prologue]\ninsts = [\"PUSH RBP\", \"@push_callee\", \"@frame_alloc\"]\n"
    );
    let errs = validate_source(&mutated, Path::new("x86_prologue_again.toml"))
        .expect_err("序言模板已删除，写回去必须被拒绝");
    let joined = errs.join("\n");
    assert!(
        joined.contains("prologue"),
        "错误应点名 `prologue`（哪一段写错了）：{joined}"
    );
}

/// **序言模板缺席也要发射收参**（收参不由模板决定）：夹具的 `[emit.prologue]` 已删除。
#[test]
fn a_spec_without_a_prologue_template_still_receives_its_arguments() {
    let text = text_of(FIXTURE_NO_PROLOGUE);
    for needle in ["__layout_ok", "param_vregs"] {
        assert!(
            text.contains(needle),
            "缺席序言模板的谱必须仍然发射收参（生成器插入），生成物里没有 `{needle}`"
        );
    }
}

/// **位置**：收参在 callee-saved 保存之后、帧分配之前。
///
/// 顺序错了不是风格问题：保存若发生在收参之后，保存下来的是**实参值**而不是调用者的
/// 寄存器值，尾声恢复时会毁掉调用者的寄存器。
#[test]
fn the_receive_lands_after_the_saves_and_before_the_frame_alloc() {
    let text = text_of("isa/x86_v12.toml");
    let start = text
        .rfind("fn emit_prologue")
        .expect("生成物里没有 emit_prologue");
    let end = text[start..]
        .find("fn emit_epilogue")
        .map(|i| start + i)
        .expect("生成物里没有 emit_epilogue");
    let body = &text[start..end];
    let recv = body.find("__layout_ok").expect("序言里没有收参");
    let last_push = body
        .rfind("Inst :: Push")
        .expect("序言里没有 callee-saved 的 push");
    assert!(
        last_push < recv,
        "收参排在 callee-saved 保存之前（push @ {last_push} > 收参 @ {recv}）——\
         保存的会是实参值而不是调用者的寄存器值"
    );
    // x86 的 `@frame_alloc` = `SUB64_R_IMM32 RSP, {frame_size}`（在收参之后）。
    let frame_alloc = body.find("Sub64RImm32").expect("序言里没有帧分配");
    assert!(
        recv < frame_alloc,
        "收参应排在帧分配之前（收参 @ {recv} > 帧分配 @ {frame_alloc}）"
    );
}

/// **A4 反回潮**：四份发行谱 + 夹具的 TOML 里**不许再有**序/尾声模板与伪指令——
/// 它们是调用约定的事，写回谱里必须被抓住（哪怕生成器碰巧还能跑）。
#[test]
fn no_spec_writes_prologue_templates_or_pseudo_instructions_any_more() {
    // 注意路径基准：`std::fs` 相对**本 crate 目录**，而 `expand_file`/`read_isa_file`
    // 走仓库根解析（两者差一层 `../../`）。
    let mut specs: Vec<String> = [
        "../../../isa/x86_v12.toml",
        "../../../isa/riscv64_v12.toml",
        "../../../isa/arm64_v12.toml",
    ]
    .iter()
    .map(|p| (*p).to_string())
    .collect();
    specs.extend(
        std::fs::read_dir("../../backend/forge-codegen/tests/isa")
            .expect("夹具目录")
            .filter_map(|e| e.ok())
            .map(|e| e.path().to_string_lossy().replace('\\', "/"))
            .filter(|p| p.ends_with(".toml")),
    );
    for path in specs {
        let src = std::fs::read_to_string(&path).unwrap_or_else(|e| panic!("读 {path} 失败：{e}"));
        for needle in [
            "[emit.prologue]",
            "[emit.epilogue]",
            "@push_callee",
            "@pop_callee",
            "@frame_alloc",
            "@frame_free",
            "@move_args",
        ] {
            assert!(
                !src.contains(needle),
                "{path}: 仍写着 `{needle}`——序/尾声与收参是调用约定的事（v20 A4 起\
                 由生成器按角色 + [abi.frame] 生成），谱里只留裸指令"
            );
        }
    }
}

/// **帧内机制（riscv/arm64）的顺序**：`frame_alloc` → 存 ra/fp → 建帧指针 →
/// 存 callee-saved → 收参；尾声 = 恢复 → 恢复 ra/fp → `frame_free` → `ret`。
#[test]
fn the_store_to_frame_sequence_keeps_its_order() {
    for (name, path, alloc, save, load, free, ret) in [
        (
            "riscv64_v12",
            "isa/riscv64_v12.toml",
            "Inst :: Addi { dst : Reg :: X2",
            "Inst :: Sd {",
            "Inst :: Ld {",
            "Inst :: Addi { dst : Reg :: X2 , src : Reg :: X2 , imm : __frame_size as i64",
            "Inst :: Ret",
        ),
        (
            "arm64_v12",
            "isa/arm64_v12.toml",
            "Inst :: Subimmx {",
            "Inst :: Sturx {",
            "Inst :: Ldurx {",
            "Inst :: Addimmx { dst : Reg :: SP",
            "Inst :: Ret",
        ),
    ] {
        let text = text_of(path);
        let start = text.rfind("fn emit_prologue").expect("emit_prologue");
        let end = text[start..]
            .find("fn emit_spill_load")
            .map(|i| start + i)
            .expect("emit_spill_load");
        let body = &text[start..end];
        let p_end = body.find("fn emit_epilogue").expect("emit_epilogue");
        let (pro, epi) = (&body[..p_end], &body[p_end..]);
        let seq = |hay: &str, needle: &str, what: &str| {
            hay.find(needle)
                .unwrap_or_else(|| panic!("{name}: {what} 里没有 `{needle}`"))
        };
        let a = seq(pro, alloc, "序言");
        let s = seq(pro, save, "序言");
        let r = seq(pro, "__layout_ok", "序言");
        assert!(a < s && s < r, "{name}: 序言必须是 分配 → 保存 → 收参");
        let l = seq(epi, load, "尾声");
        let f = seq(epi, free, "尾声");
        let rt = seq(epi, ret, "尾声");
        assert!(
            l < f && f < rt,
            "{name}: 尾声必须是 恢复 → 释放帧 → 返回（实际 {l} / {f} / {rt}）"
        );
    }
}
