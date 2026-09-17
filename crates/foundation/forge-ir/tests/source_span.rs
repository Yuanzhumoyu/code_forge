//! 源码位置（span）贯穿守卫 —— v3 S7 文本层。
//!
//! 改前实测：语法层根本不留字节区间（`ParsedBlock` 只有 `insts`），所以
//! `Instruction::loc` 在整条解析链路上恒为 `None`——即便 `FunctionBuilder` 早就
//! 有 `set_current_loc`/`emit1` 把它写进指令（builder.rs），也没有任何调用者。
//! 用户看到的后果：诊断只能指到"函数级"，改一行报一行做不到。
//!
//! 现在：`grammar.lalrpop` 的 `SpannedInst` 给每条指令留 `@L` 字节偏移，
//! `parse_to_ast` 用 `locate` 换算成 1-based `行:列` 存进
//! `ParsedBlock::inst_line_cols`，语义层发射指令前 `set_current_loc(...)`。
//!
//! 本文件钉住四条：位置真的落到它自己那条指令上（含不同缩进的列号）、
//! 合成指令（常量/占位）不会继承上一条的位置、phi/终结符的现状（尚未贯穿，
//! 见计划 S7 余项）、以及位置不进文本层（打印-重解析仍幂等）。

use forge_ir::function::Function;
use forge_ir::ir_parser::parse_module;
use forge_ir::opcode::Opcode;

/// `(行, 列)`；任一维缺失即视为无位置。
type Loc = Option<(u32, u32)>;

fn locs(f: &Function) -> Vec<(Opcode, Loc)> {
    f.dfg
        .block_data_iter()
        .flat_map(|b| b.inst_order.iter())
        .map(|&i| {
            let d = f.dfg.inst_data(i);
            (
                d.opcode,
                d.loc.as_ref().and_then(|l| Some((l.line?, l.column?))),
            )
        })
        .collect()
}

/// 某 opcode 的全部位置（按发射顺序）。
fn locs_of(f: &Function, op: Opcode) -> Vec<Loc> {
    locs(f)
        .into_iter()
        .filter(|(o, _)| *o == op)
        .map(|(_, l)| l)
        .collect()
}

fn parse_one(src: &str) -> Function {
    let m = parse_module(src).expect("parse");
    let mut it = m.iter_functions();
    let f = it.next().expect("at least one function");
    assert!(it.next().is_none(), "夹具只应有一个函数");
    f.clone()
}

/// 位置落在**它自己**那条指令上：行号随指令走，列号 = 该行的缩进宽度 + 1
/// （夹具故意用两种缩进，防止列号被写死成 3）。
#[test]
fn instruction_spans_flow_into_ir_locations() {
    let f = parse_one(
        "define i32 @f(i32 %a, i32 %b) {\nentry:\n  %x = add i32 %a, %b\n    %y = mul i32 %x, %x\n  ret i32 %y\n}\n",
    );
    assert_eq!(
        locs_of(&f, Opcode::Iadd),
        vec![Some((3, 3))],
        "`%x = add …` 在第 3 行、缩进 2 空格 ⇒ 列 3"
    );
    assert_eq!(
        locs_of(&f, Opcode::Imul),
        vec![Some((4, 5))],
        "`%y = mul …` 在第 4 行、缩进 4 空格 ⇒ 列 5（列号必须来自源码，不能写死）"
    );
    // 每块只发射这两条（ret 是终结符，见下一个用例）。
    assert_eq!(
        locs(&f).len(),
        2,
        "夹具应只发射 2 条指令（实测 {:?}）",
        locs(&f)
    );
}

/// 合成指令不继承位置：为 `add i32 %a, 5` 物化的 `iconst 5` 属于第 3 行
/// （它就是那条指令的一部分）；而指令循环**结束之后**为终结符物化的
/// `iconst 42` 没有源码位置——它由语义层合成，不是用户写的。
///
/// 这条同时是 `fb.set_current_loc(None)` 的负向守卫：删掉循环尾部的复位，
/// 第二个 `iconst` 会继承第 3 行。
#[test]
fn synthesized_constants_do_not_inherit_a_stale_location() {
    let f = parse_one("define i32 @g(i32 %a) {\nentry:\n  %x = add i32 %a, 5\n  ret i32 42\n}\n");
    assert_eq!(
        locs_of(&f, Opcode::Iadd),
        vec![Some((3, 3))],
        "`add` 在第 3 行第 3 列"
    );
    let icons = locs_of(&f, Opcode::Iconst);
    assert_eq!(
        icons.len(),
        2,
        "应有两个常量：`5`（第 3 行，指令内物化）+ `42`（终结符操作数，循环后物化）实测 {icons:?}"
    );
    assert_eq!(
        icons[0],
        Some((3, 3)),
        "指令内物化的 `5` 属于它服务的那条指令（第 3 行）"
    );
    assert_eq!(
        icons[1], None,
        "循环后为终结符物化的 `42` 是合成值，不应继承第 3 行（实测 {icons:?}）"
    );
}

/// 多块：每块各归各的行；phi 绑到块参数（不发射指令）、终结符走
/// `build_terminator`（不经 builder）⇒ 两者的行都不应出现在任何位置里。
#[test]
fn per_block_spans_and_phi_terminator_gap() {
    let f = parse_one(concat!(
        "define i32 @h(i32 %a, i32 %b) {\n",              // 1
        "entry:\n",                                       // 2
        "  %x = add i32 %a, 5\n",                         // 3
        "  %c = icmp slt i32 %x, %b\n",                   // 4
        "  br i1 %c, label %then, label %else\n",         // 5
        "\n",                                             // 6
        "then:\n",                                        // 7
        "  %p = phi i32 [ %x, %entry ], [ %b, %else ]\n", // 8
        "  %q = add i32 %p, 1\n",                         // 9
        "  br label %exit\n",                             // 10
        "\n",                                             // 11
        "else:\n",                                        // 12
        "  %r = phi i32 [ %a, %entry ], [ 7, %then ]\n",  // 13
        "  br label %exit\n",                             // 14
        "\n",                                             // 15
        "exit:\n",                                        // 16
        "  %s = phi i32 [ %q, %then ], [ %r, %else ]\n",  // 17
        "  ret i32 %s\n",                                 // 18
        "}\n",                                            // 19
    ));
    assert_eq!(
        locs_of(&f, Opcode::Iadd),
        vec![Some((3, 3)), Some((9, 3))],
        "两个块的 add 各归各的行（entry 第 3 行、then 第 9 行）"
    );
    assert_eq!(locs_of(&f, Opcode::Icmp), vec![Some((4, 3))]);
    // 合成值：entry 的 `5`（第 3 行）、then 的 `1`（第 9 行）各归自己那条指令；
    // else 的 phi 入边常量 `7` 在指令循环之后物化 ⇒ 无位置。
    assert_eq!(
        locs_of(&f, Opcode::Iconst),
        vec![Some((3, 3)), Some((9, 3)), None],
        "指令内常量随所属指令，phi 入边常量（终结符路径合成）无位置"
    );
    // phi 行（8/13/17）与终结符行（5/10/14/18）都不应出现在任何位置里。
    let spanned: Vec<u32> = locs(&f)
        .into_iter()
        .filter_map(|(_, l)| l.map(|(line, _)| line))
        .collect();
    for line in [5u32, 8, 10, 13, 14, 17, 18] {
        assert!(
            !spanned.contains(&line),
            "第 {line} 行是 phi/终结符，尚未贯穿（计划 S7 余项），不应有位置（实测 {spanned:?}）"
        );
    }
    // 终结符本身的位置现状：尚未贯穿。一旦 S7 余项补齐，请更新本断言。
    for b in f.dfg.block_data_iter() {
        let t = b.terminator_opt().expect("每块都有终结符");
        assert!(
            f.dfg.inst_data(t).loc.is_none(),
            "终结符 span 尚未贯穿（计划 S7 余项）"
        );
    }
}

/// 位置不进文本层：打印 → 重解析 → 再打印必须逐字节相同（若显示层把位置写进
/// 文本，第二次解析要么多出记号、要么文本漂移）。
#[test]
fn locations_do_not_leak_into_the_text_layer() {
    let src = "define i32 @f(i32 %a, i32 %b) {\nentry:\n  %x = add i32 %a, %b\n    %y = mul i32 %x, %x\n  ret i32 %y\n}\n";
    let t1 = format!("{}", parse_module(src).expect("parse"));
    let t2 = format!("{}", parse_module(&t1).expect("reparse"));
    assert_eq!(t1, t2, "带位置的模块打印后必须仍可幂等重解析");
    assert!(
        !t1.contains("line ") && !t1.contains(":3:3"),
        "位置信息不得出现在文本层（实测：{t1}）"
    );
}
