//! **搬运族派生**（v20 V8）：ISA 只写"这条指令搬多少位"（`data_width`），方向 / 寄存器族 /
//! 立即数还是内存**全部由操作数结构派生**。
//!
//! 这是 `gpr_mov`/`fpr_mov`/`vec_mov`/`gpr_mov_imm`/`fpr_to_gpr_mov`/`gpr_to_fpr_mov`/
//! `wide_vec_load`/`wide_vec_store` 八个手写角色的替代品——它们要么把**可派生**的事实
//! （方向、类别、立即数）手抄一遍，要么把**宽度**编进角色名，两者都会让"另一种寄存器组/
//! 另一种位宽的 ISA"接不进来。
//!
//! 本文件钉住四件事：
//!
//! 1. **位宽是谱的数据**——把 x86 的 `data_width` 从 32/64 改成 16/128，生成物跟着走，
//!    写死的 32/64 分派臂一个不留（回潮成 `if size == 4 { MOVSS } else { MOVSD }` 即变红）；
//! 2. **三操作数搬移的第三槽自动填成源**（riscv `fsgnj.s rd, rs, rs` 就是寄存器搬移）；
//! 3. **缺搬运 ⇒ 生成物里 fail-closed**（类间位搬移方向尤其危险：拿同类搬移顶上会把
//!    FPR 的号当 GPR 号用 = 静默错值）；
//! 4. **歧义与坏形状在生成期报错**（同形状同宽度两条候选、`data_width` 写在非纯搬运指令上），
//!    并**列全候选**——不设"钉选"注解（那就是第二套机制）。
//!
//! 端到端那一半（真编出 `fmv.d.x`/`fmv.x.d`）在
//! `crates/backend/forge-codegen/tests/bank_mov.rs`（合成约定"浮点进 int 池"）。

use std::path::Path;

use forge_isa_dsl::{expand_str, read_isa_file};

fn flat_file(path: &str, mod_name: &str) -> (String, String) {
    let (src, _) = read_isa_file(path).unwrap_or_else(|e| panic!("读 {path} 失败：{e}"));
    let t: String = expand_str(&src, mod_name, Path::new(path))
        .unwrap_or_else(|e| panic!("展开 {path} 失败：{e}"))
        .to_string()
        .chars()
        .filter(|c| !c.is_whitespace())
        .collect();
    (src, t)
}

fn flat_mut(src: &str, mod_name: &str, path: &str) -> Result<String, String> {
    Ok(expand_str(src, mod_name, Path::new(path))?
        .to_string()
        .chars()
        .filter(|c| !c.is_whitespace())
        .collect())
}

/// 取 `t` 中下标 `at` 前后各若干字节（对齐到字符边界，仅用于失败时的诊断打印）。
fn win(t: &str, at: usize, before: usize, after: usize) -> &str {
    let mut lo = at.saturating_sub(before);
    while lo < t.len() && !t.is_char_boundary(lo) {
        lo += 1;
    }
    let mut hi = (at + after).min(t.len());
    while hi > lo && !t.is_char_boundary(hi) {
        hi -= 1;
    }
    &t[lo..hi]
}

/// "最窄覆盖"分派臂的文本形状：`if (<位宽表达式>) as u32 <= <N>u32 { <构造> }`。
/// `quote` 把 `N: u16` 渲染成 `<N>u32`（生成器里已按 u32 插值），去空白后即 `<=<N>u32`。
fn le_arm(bits: u32) -> String {
    format!("<={bits}u32")
}

/// 删掉 `anchor` **之后**最近的一整行 `data_width = …`（谱文件是 CRLF，按行尾切）。
fn strip_data_width_after(src: &str, anchor: &str) -> String {
    let at = src
        .find(anchor)
        .unwrap_or_else(|| panic!("谱里应有 {anchor}"));
    let k = src[at..]
        .find("data_width = ")
        .map(|i| at + i)
        .expect("该处应有 data_width");
    let end = src[k..].find('\n').map(|i| k + i + 1).unwrap_or(src.len());
    format!("{}{}", &src[..k], &src[end..])
}

/// 删掉**模板行**里的 `, data_width = …`（行内键，删到该行的 `}` 之前）。
fn strip_row_data_width(src: &str, inst: &str) -> String {
    let anchor = format!("inst = \"{inst}\"");
    let at = src
        .find(&anchor)
        .unwrap_or_else(|| panic!("谱里应有模板行 {inst}"));
    let k = src[at..]
        .find("data_width = ")
        .map(|i| at + i)
        .expect("该行应有 data_width");
    let start = src[..k].rfind(',').unwrap_or(k);
    let end = src[k..].find('}').map(|i| k + i).expect("行尾应有 `}`");
    format!("{}{}", &src[..start], &src[end..])
}

/// ① **位宽是谱的数据**：把 x86 的**所有** `data_width` 往上搬一档
/// （32→64、64→128、128→256、256→512、512→1024），生成器必须跟着走。
///
/// 覆盖四处生成点——降低侧的返回/实参/取值、收参侧（`move_args`）、类间位搬移、宽向量
/// 拷贝——所以断言是"生成物里出现的分派档位**恰好**等于谱里声明的那些"。
#[test]
fn x86_move_widths_follow_the_spec() {
    let (src, _) = read_isa_file("isa/x86.toml").expect("读 x86 谱");
    // 从大到小替换（否则 64→128 之后又被 128→256 二次替换）。
    let mut mutated = src.clone();
    for (from, to) in [
        (512u32, 1024u32),
        (256, 512),
        (128, 256),
        (64, 128),
        (32, 64),
    ] {
        mutated = mutated.replace(
            &format!("data_width = {from}"),
            &format!("data_width = {to}"),
        );
    }
    // 谱里声明了哪些档（改完全都往上搬了一档）。
    let declared: std::collections::BTreeSet<u32> = mutated
        .match_indices("data_width = ")
        .map(|(at, _)| {
            let rest = &mutated[at + "data_width = ".len()..];
            rest.split(|c: char| !c.is_ascii_digit())
                .next()
                .unwrap()
                .parse::<u32>()
                .unwrap()
        })
        .collect();
    assert!(
        declared.contains(&128) && declared.contains(&1024) && !declared.contains(&32),
        "变异没生效：{declared:?}"
    );
    let t = flat_mut(&mutated, "x86", "isa/x86.toml").expect("改了位宽也该照样展开");

    // 生成物里每一处**搬运分派臂**的档位都必须是谱里声明过的宽度（多一个 = 写死了）。
    // 只看紧跟着搬运指令构造的那些比较（别的生成代码里也有 `<= N u32`，与此无关）。
    for op in ["<=", "=="] {
        let mut at = 0usize;
        while let Some(i) = t[at..].find(op) {
            let k = at + i + op.len();
            let digits: String = t[k..].chars().take_while(|c| c.is_ascii_digit()).collect();
            if !digits.is_empty() && t[k + digits.len()..].starts_with("u32") {
                let end = k + digits.len() + 3;
                let nxt: String = t[end..].chars().take(260).collect();
                if ["Inst::Mov", "Inst::Vmov", "Inst::Fmv", "Inst::Fsgnj"]
                    .iter()
                    .any(|v| nxt.contains(v))
                {
                    let w: u32 = digits.parse().unwrap();
                    assert!(
                        declared.contains(&w),
                        "搬运分派臂用了谱里没声明的 {w} 位（写死了？）——声明：{declared:?}"
                    );
                }
            }
            at = k;
        }
    }
    // 宽向量栈拷贝（by-ref/sret）也走同一张表，而且是**精确**宽度。
    for w in [512u32, 1024] {
        assert!(
            t.contains(&format!("=={w}u32")),
            "谱里声明的 {w} 位档没进生成物（内存族按精确宽度分派）"
        );
    }
    // 每档指向**它自己**那条指令（64 位档 = MOVSS、128 位档 = MOVSD）。
    for (bits, vn) in [(64u32, "Movss"), (128u32, "Movsd")] {
        let arm = le_arm(bits);
        let hit = t
            .match_indices(&arm)
            .any(|(at, _)| win(&t, at, 0, 300).contains(vn));
        assert!(hit, "应有 {bits} 位的分派臂指向 `{vn}`");
    }
    // 类间位搬移与宽向量拷贝也走同一张表：x86 的 MOVQ / VMOVUPS。
    for vn in ["MovqXmmR64", "MovqR64Xmm", "VmovupsZmmMem"] {
        assert!(t.contains(vn), "搬运族指令 `{vn}` 应参与生成");
    }
    // 两处生成点（降低侧 + 收参侧）都按表发指令，不是只改一处。
    assert!(
        t.matches("Movsd").count() >= 2,
        "128 位档应在降低侧与收参侧都被引用（实测 {}）",
        t.matches("Movsd").count()
    );
}

/// ② 三操作数搬移：**第三槽自动填成源**（riscv `fsgnj.s rd, rs, rs` 就是寄存器搬移）。
///
/// 这条同时钉住 riscv 谱里"双精度档真的申报了 `data_width`"——缺了它 `f64` 的参数/返回
/// 全编不出来（v20 V6 修的那个洞）。
#[test]
fn riscv_three_operand_move_fills_the_third_slot() {
    let (_src, t) = flat_file("isa/riscv64.toml", "riscv64");
    // 发射侧把第三槽填成**同一个源**（编码器/反汇编里是 `*src2`，不是这个形状）。
    assert!(
        t.contains("Inst::FsgnjD{dst:__dst,src:__ph(0,__DEFAULT_FPR_CLASS),src2:__ph(0,__DEFAULT_FPR_CLASS),}"),
        "三操作数搬移的第三槽应自动填成源（发射侧）"
    );
    // 第三槽（Reg 字段序号 2）也要映射进指令包，否则 regalloc 不知道它读的是同一个 vreg。
    assert!(
        t.contains("map_reg_field(__a,__idx,2u8,false)")
            || t.contains("map_reg_field(val,__fidx,2u8,false)")
            || t.contains("map_reg_field(__r,__idx,2u8,true)"),
        "第三个寄存器的映射不能漏（否则第三槽读不到源）"
    );
}

/// ③ 把类间位搬移的 `data_width` 删掉 ⇒ 生成物里两条跨类路径都变成明确的 fail-closed。
///
/// riscv 的 `fmv.x.d`/`fmv.d.x` 正是"整数值寄存器 ↔ 浮点寄存器"的按位搬移；缺它时拿同类
/// 搬移顶上会把 FPR 的号当 GPR 号用（`Reg::from_index(10, GPR)` = `x10`）——静默错值。
#[test]
fn stripping_the_data_width_makes_the_cross_bank_path_fail_closed() {
    let (src, _) = read_isa_file("isa/riscv64.toml").expect("读 riscv 谱");
    // 只删**类间搬移**那两条的 `data_width`（FSGNJ 的寄存器搬移留着，
    // 否则连"浮点进浮点"也没了，测不到跨类那一支）。
    let mut mutated = src.clone();
    for inst in ["FMV_W_X", "FMV_D_X"] {
        mutated = strip_data_width_after(&mutated, &format!("name = \"{inst}\""));
    }
    for inst in ["FMV_X_W", "FMV_X_D"] {
        mutated = strip_row_data_width(&mutated, inst);
    }
    assert!(
        !mutated.contains("data_width = 32") || mutated.contains("FSGNJ_S"),
        "变异没生效"
    );
    assert!(
        mutated.contains("FSGNJ_S") && mutated.contains("data_width = 32"),
        "FSGNJ（同类搬移）应保留，否则测不到跨类那一支"
    );
    let t = flat_mut(&mutated, "riscv64", "isa/riscv64.toml")
        .expect("删掉 data_width 后仍应能展开（只是 fail-closed）");
    // 生成物里的中文字面量按原文渲染（未转义），但**行继续符会原样留在文本里**
    //（`跨类的\` + 换行 + `位搬移指令`）——所以断言取**同一行内**的片段。
    assert!(
        t.contains("ABI落点的寄存器类与值的类不同（整数约定收浮点），需要跨类的"),
        "缺搬运时生成物里必须有类间位搬移的 fail-closed 分支（不退化、不猜）"
    );
    // 指令变体本身还在（只删了 `data_width`，没删指令），但它们不再被发射引用。
    assert!(
        t.contains("FmvXD") && t.contains("FmvDX"),
        "指令变体应仍存在（谱里没删指令）"
    );
    // （不能靠"生成物里没有 `Inst::FmvXD`"来判——编码器/反汇编器照样构造它；
    //   判据是上面那条 fail-closed 分支。）
}

/// ④ **同一形状 + 同一宽度两条候选** ⇒ 生成期报错并**列全候选**。
///
/// x86 的 `MOVD_FREG_IREG`（32）与 `MOVQ_XMM_R64`（64）同形状（`fpr ← gpr`），把前者也
/// 写成 64 位就撞车。这里要求报错点名两条指令——歧义是作者该消歧的事，不引入"钉选"注解。
#[test]
fn two_candidates_of_the_same_width_are_rejected_with_the_list() {
    let (src, _) = read_isa_file("isa/x86.toml").expect("读 x86 谱");
    // 只改 MOVD_FREG_IREG 那一处（按指令名定位，避免误伤别的 32 位档）：
    // 把它的 `data_width = 32` 换成 64，与 MOVQ_XMM_R64 撞车。
    let mutated = strip_data_width_after(&src, "name = \"MOVD_FREG_IREG\"");
    let mutated = mutated.replacen(
        "name = \"MOVD_FREG_IREG\"",
        "name = \"MOVD_FREG_IREG\"\ndata_width = 64",
        1,
    );
    assert!(
        mutated.contains("MOVD_FREG_IREG\"\ndata_width = 64"),
        "变异没生效"
    );
    let err = flat_mut(&mutated, "x86", "isa/x86.toml").expect_err("同形状同宽度两条候选必须报错");
    assert!(err.contains("候选"), "错误应说明候选：{err}");
    assert!(
        err.contains("MOVD_FREG_IREG") && err.contains("MOVQ_XMM_R64"),
        "错误应列全候选指令名：{err}"
    );
}

/// ⑤ `data_width` 写在**非纯搬运**指令上 ⇒ 报错（不能静默不生效）。
///
/// x86 `CMPXCHG_MEM_R` 有两个 Reg 输入槽、**没有目的槽**——它不是搬运。
#[test]
fn data_width_on_a_non_mover_shape_is_rejected() {
    let (src, _) = read_isa_file("isa/x86.toml").expect("读 x86 谱");
    let mutated = src.replacen(
        "name = \"CMPXCHG_MEM_R\"",
        "name = \"CMPXCHG_MEM_R\"\ndata_width = 64",
        1,
    );
    assert!(
        mutated.contains("CMPXCHG_MEM_R\"\ndata_width = 64"),
        "变异没生效"
    );
    let err =
        flat_mut(&mutated, "x86", "isa/x86.toml").expect_err("非搬运形状上的 data_width 必须报错");
    assert!(
        err.contains("纯搬运") || err.contains("目的槽"),
        "错误应说明形状要求：{err}"
    );
}

/// ⑥ **发行谱里不再有任何搬运角色**（反回潮）：八个手写角色一个都不许回来。
///
/// 它们各自对应一条可派生或可申报的事实——回到手写角色就是回到"为某个 ISA 开洞"。
#[test]
fn shipped_specs_declare_no_move_roles() {
    const GONE: [&str; 8] = [
        "gpr_mov",
        "gpr_mov_imm",
        "fpr_mov",
        "vec_mov",
        "fpr_to_gpr_mov",
        "gpr_to_fpr_mov",
        "wide_vec_load",
        "wide_vec_store",
    ];
    for path in ["isa/x86.toml", "isa/riscv64.toml", "isa/arm64.toml"] {
        let (src, _) = read_isa_file(path).unwrap_or_else(|e| panic!("读 {path} 失败：{e}"));
        for name in GONE {
            assert!(
                !src.contains(&format!("roles = [\"{name}\"]")),
                "{path} 不该再有 `roles = [\"{name}\"]`（改用指令的 `data_width`）"
            );
            assert!(
                !src.contains(&format!("role = \"{name}\"")),
                "{path} 不该再有 `role = \"{name}\"`（改用指令的 `data_width`）"
            );
        }
    }
}
