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

/// **`@move_args` 已从谱面撤出**：写了它必须**明确报错**（静默忽略 = "谱里写了却没人读"，
/// 正是这次重设计要消灭的东西），且提示要能指路。
#[test]
fn writing_move_args_in_the_prologue_is_rejected() {
    let src = read_isa_file("isa/x86_v12.toml").expect("读 x86 谱").0;
    let mutated = src.replace(
        "\"@push_callee\", \"@frame_alloc\"",
        "\"@push_callee\", \"@move_args\", \"@frame_alloc\"",
    );
    assert!(mutated.contains("@move_args"), "变异没生效");
    let errs = validate_source(&mutated, Path::new("x86_move_args.toml"))
        .expect_err("谱里写 @move_args 必须被拒绝");
    let joined = errs.join("\n");
    for needle in ["@move_args", "已删除", "调用约定"] {
        assert!(joined.contains(needle), "错误应提到 `{needle}`：{joined}");
    }
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
