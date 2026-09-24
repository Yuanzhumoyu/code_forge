//! `forge-isa abi` —— **调用约定的静态体检**（v20 A1）。
//!
//! 三件事，全部**不需要后端**（只读谱 + 数据）：
//!
//! | 命令 | 作用 |
//! | --- | --- |
//! | `abi list` | 列出内置约定与绑定（每份约定的关键事实、每个绑定的池） |
//! | `abi check <谱>` | 从谱建**能力视图**（`forge_isa_dsl::abi_view`），对每个可用的内置绑定跑一组代表签名：报**硬错**（写错的名字/约定）与**缺口**（这台机器做不了，fail-closed） |
//! | `abi plan <谱>` | 跑**一个**签名，打印 `AbiPlan` 的确定性文本 |
//!
//! 三条口径（与 `docs/reference/calling-conventions.md` 一致）：
//!
//! - **缺口不是错**：`MissingPool`/`Unsupported`/`CapabilityGap`/`PoolExhausted` 是
//!   "这台 ISA 暂时做不了这件事"（fail-closed，不会产出错值），默认只报 `GAP`；
//!   `--strict` 才让它们决定退出码。**硬错**（`UnresolvedReg`/`BadRules`/`Parse`）
//!   一律退出 1——那是数据写错了。
//! - 退出码：`0` 成功（可能有缺口）、`1` 硬错或 `--strict` 下有缺口、`2` 用法错误。
//! - 寄存器编号 = `abi_view` 的"GPR 区 + FP 区"（与 `TargetRegInfo::num_gp_regs`/
//!   `num_fp_regs` 对齐）；**绑定优先用寄存器名**，整数选择子是宿主编号语义。

use std::collections::BTreeMap;
use std::path::PathBuf;
use std::process::ExitCode;

use forge_abi::{AbiError, AbiTarget, Capability, Elem, Signature, TyView, builtin};
use forge_isa_dsl::abi_view::{self, MachineView};

/// 主入口。`args` = 子命令之后的全部参数。
pub fn run(args: &[String], json: bool) -> Result<ExitCode, String> {
    match args.first().map(String::as_str) {
        Some("list") => cmd_list(json),
        Some("check") => cmd_check(&args[1..], json),
        Some("plan") => cmd_plan(&args[1..], json),
        Some(other) => Err(format!(
            "未知的 `abi` 子命令 `{other}`（可用：list / check / plan）"
        )),
        None => Err("`abi` 需要一个子命令（list / check / plan）".into()),
    }
}

// ─────────────────────────── abi list ───────────────────────────

fn cmd_list(json: bool) -> Result<ExitCode, String> {
    let reg = builtin::registry().map_err(|e| e.to_string())?;
    if json {
        let mut out = String::from("{\n  \"conventions\": [");
        for (i, name) in reg.conv_names().iter().enumerate() {
            let r = reg.rules(name).expect("已注册");
            if i > 0 {
                out.push(',');
            }
            out.push_str(&format!(
                "\n    {{\"name\": {}, \"position\": \"{:?}\", \"stack_align\": {}, \"shadow_bytes\": {}, \
                 \"red_zone\": {}, \"variadic_stack_only\": {}, \"callee_saved\": \"{:?}\", \
                 \"sret_pool\": {}, \"va_list\": \"{:?}\"}}",
                q(name),
                r.position,
                r.stack_align,
                r.shadow_bytes,
                r.red_zone.map(|v| v.to_string()).unwrap_or_else(|| "null".into()),
                r.variadic_stack_only,
                r.callee_saved.mechanism,
                r.hidden
                    .sret_pool
                    .as_ref()
                    .map(|p| q(p))
                    .unwrap_or_else(|| "null".into()),
                r.hidden.va_list,
            ));
        }
        out.push_str("\n  ],\n  \"bindings\": [");
        for (i, (isa, conv)) in reg.binding_names().iter().enumerate() {
            let b = reg.binding(isa, conv).expect("已注册");
            if i > 0 {
                out.push(',');
            }
            let pools: Vec<String> = b
                .pools
                .iter()
                .map(|(k, v)| format!("{}: {}", q(k), v.len()))
                .collect();
            out.push_str(&format!(
                "\n    {{\"isa\": {}, \"conv\": {}, \"pools\": {{{}}}}}",
                q(isa),
                q(conv),
                pools.join(", ")
            ));
        }
        out.push_str("\n  ]\n}");
        println!("{out}");
        return Ok(ExitCode::SUCCESS);
    }

    println!("内置调用约定（{} 份）：", reg.conv_names().len());
    for name in reg.conv_names() {
        let r = reg.rules(name).expect("已注册");
        println!(
            "  {name:<8} 位置={:?} 栈对齐={} shadow={} 红区={} 变参走栈={} 保存={:?} sret池={} va_list={:?}",
            r.position,
            r.stack_align,
            r.shadow_bytes,
            r.red_zone
                .map(|v| v.to_string())
                .unwrap_or_else(|| "-".into()),
            r.variadic_stack_only,
            r.callee_saved.mechanism,
            r.hidden.sret_pool.as_deref().unwrap_or("-"),
            r.hidden.va_list,
        );
        if let Some(note) = &r.note {
            println!("           备注：{note}");
        }
    }
    println!(
        "\n内置绑定（{} 份，随 crate 数据）：",
        reg.binding_names().len()
    );
    for (isa, conv) in reg.binding_names() {
        let b = reg.binding(&isa, &conv).expect("已注册");
        let pools: Vec<String> = b
            .pools
            .iter()
            .map(|(k, v)| format!("{k}={}", v.len()))
            .collect();
        println!("  {isa:<13} {conv:<8} {}", pools.join(" "));
    }
    println!("\n看某台机器做不做得到：forge-isa abi check isa/x86_v12.toml");
    Ok(ExitCode::SUCCESS)
}

// ─────────────────────────── abi check ───────────────────────────

/// 体检结论的严重度。
#[derive(PartialEq, Eq, PartialOrd, Ord)]
enum Level {
    Gap,
    Error,
}

fn cmd_check(args: &[String], json: bool) -> Result<ExitCode, String> {
    let strict = args.iter().any(|a| a == "--strict");
    let (rest, conv_filter) = take_value(args, "--conv")?;
    let files: Vec<PathBuf> = rest
        .iter()
        .filter(|a| !a.starts_with('-'))
        .map(PathBuf::from)
        .collect();
    if files.is_empty() {
        return Err("abi check 需要至少一个谱文件".into());
    }
    let reg = builtin::registry().map_err(|e| e.to_string())?;
    let mut hard = 0usize;
    let mut gaps = 0usize;
    let mut out_json: Vec<String> = Vec::new();

    for file in &files {
        let view = match abi_view::inspect(file) {
            Ok(v) => v,
            Err(msg) => {
                println!("{}: 谱读不了：\n{msg}", file.display());
                hard += 1;
                continue;
            }
        };
        let target = ViewTarget::new(&view);
        println!("== {} ({})", view.isa, file.display());
        println!(
            "   寄存器：GPR {} 个 + FP {} 个 = {}；固定用途 {} 个；scratch {} 个；链接寄存器 {}",
            view.n_gpr,
            view.n_fp,
            view.n_gpr + view.n_fp,
            view.pinned.len(),
            view.scratch.len(),
            view.link
                .and_then(|i| view.reg_name(i).map(|s| s.to_string()))
                .unwrap_or_else(|| "-".into())
        );
        let caps = abi_view::declared_capabilities(&view);
        println!(
            "   声明的能力：{}",
            if caps.is_empty() {
                "（无）".to_string()
            } else {
                caps.iter()
                    .map(|(k, bits)| format!("{k}@{bits}"))
                    .collect::<Vec<_>>()
                    .join(" ")
            }
        );
        for n in &view.notes {
            println!("   提示：{n}");
        }

        // 这台机器适配哪些内置绑定？
        let mut pairs: Vec<(String, String)> = reg
            .binding_names()
            .into_iter()
            .filter(|(isa, conv)| {
                *isa == view.isa && conv_filter.as_deref().is_none_or(|c| c == conv)
            })
            .collect();
        pairs.sort();
        if pairs.is_empty() {
            println!(
                "   ⚠ 没有与 ISA `{}` 匹配的内置绑定（--conv 可指定约定）",
                view.isa
            );
        }
        let probes = probe_corpus(&view);
        for (isa, conv) in pairs {
            let mut fatal_here = false;
            let mut gap_here: Vec<String> = Vec::new();
            for (label, sig) in &probes {
                match reg.plan(&target, &conv, sig) {
                    Ok(_) => {}
                    Err(e) => {
                        let line = format!("{conv}/{label}: {e}");
                        if level_of(&e) == Level::Error {
                            fatal_here = true;
                            hard += 1;
                            println!("   ✗ 硬错 {line}");
                        } else {
                            gaps += 1;
                            gap_here.push(line);
                        }
                    }
                }
            }
            // 绑定里的名字在这台机器上解析得动吗？（写错名字 = 硬错）
            let b = reg.binding(&isa, &conv).expect("已注册");
            for pool in b.pools.keys() {
                if let Err(e) = b.resolve_pool(pool, &target) {
                    fatal_here = true;
                    hard += 1;
                    println!("   ✗ 硬错 {conv} 的池 `{pool}`：{e}");
                }
            }
            if fatal_here {
                out_json.push(format!(
                    "{{\"isa\": {}, \"conv\": {}, \"verdict\": \"error\"}}",
                    q(&isa),
                    q(&conv)
                ));
            } else if gap_here.is_empty() {
                println!("   ✓ {conv}：代表签名全部可规划（{} 条）", probes.len());
                out_json.push(format!(
                    "{{\"isa\": {}, \"conv\": {}, \"verdict\": \"ok\"}}",
                    q(&isa),
                    q(&conv)
                ));
            } else {
                println!(
                    "   ⚠ {conv}：{} 条缺口（fail-closed，不是错值）",
                    gap_here.len()
                );
                for g in &gap_here {
                    println!("       GAP {g}");
                }
                out_json.push(format!(
                    "{{\"isa\": {}, \"conv\": {}, \"verdict\": \"gap\", \"gaps\": {}}}",
                    q(&isa),
                    q(&conv),
                    gap_here.len()
                ));
            }
        }
    }

    if json {
        println!(
            "{{\"hard_errors\": {hard}, \"gaps\": {gaps}, \"results\": [{}]}}",
            out_json.join(", ")
        );
    }
    println!("\n合计：硬错 {hard}，缺口 {gaps}（缺口 = 这台机器做不了，fail-closed）");
    if hard > 0 || (strict && gaps > 0) {
        return Ok(ExitCode::from(1));
    }
    Ok(ExitCode::SUCCESS)
}

/// 硬错 vs 缺口：**数据写错**是硬错，**能力/池不够**是缺口。
fn level_of(e: &AbiError) -> Level {
    match e {
        AbiError::UnresolvedReg { .. } | AbiError::BadRules { .. } | AbiError::Parse(_) => {
            Level::Error
        }
        AbiError::MissingPool { .. }
        | AbiError::PoolExhausted { .. }
        | AbiError::CapabilityGap { .. }
        | AbiError::Unsupported { .. } => Level::Gap,
    }
}

/// `check` 的代表签名：每个分类分支一条（与 `forge-abi` 的快照语料同源思路，
/// 但这里独立成表——CLI 不该依赖测试里的语料）。
fn probe_corpus(view: &MachineView) -> Vec<(String, Signature)> {
    let addr = view.n_gpr.max(1); // 只用来说明"多少个整数参数才耗尽"，不参与分类
    let (i8v, i32v, i64v, f32v, f64v, ptrv) = (
        TyView::int(1, 1),
        TyView::int(4, 4),
        TyView::int(8, 8),
        TyView::float(4),
        TyView::float(8),
        TyView::ptr(8),
    );
    let v128 = TyView::vector(Elem::Float, 4, 4);
    let v256 = TyView::vector(Elem::Float, 8, 4);
    let agg_ii = TyView::agg(8, vec![i64v.clone(), i64v.clone()]);
    let hfa1 = TyView::agg(4, vec![f32v.clone()]);
    let hfa2 = TyView::agg(4, vec![f32v.clone(), f32v.clone()]);
    let hfa4 = TyView::agg(
        4,
        vec![f32v.clone(), f32v.clone(), f32v.clone(), f32v.clone()],
    );
    let agg24 = TyView::agg(1, vec![i8v.clone(); 24]);
    let many: Vec<(String, TyView)> = (0..addr + 2)
        .map(|i| (format!("a{i}"), i64v.clone()))
        .collect();

    let mut out: Vec<(String, Signature)> = Vec::new();
    let mut add = |label: &str, params: Vec<(String, TyView)>, ret: Option<TyView>| {
        out.push((label.to_string(), Signature::new(params, ret)));
    };
    let a1 = |ty: TyView| vec![("a".to_string(), ty)];
    add("void", vec![], None);
    add("i32", a1(i32v.clone()), Some(i32v));
    add("i64", a1(i64v.clone()), Some(i64v.clone()));
    add("ptr", a1(ptrv.clone()), Some(ptrv.clone()));
    add("f32", a1(f32v.clone()), Some(f32v));
    add("f64", a1(f64v.clone()), Some(f64v.clone()));
    add("vec16", a1(v128.clone()), Some(v128));
    add("vec32", a1(v256.clone()), Some(v256));
    add("agg16", a1(agg_ii.clone()), Some(agg_ii));
    add("agg24", a1(agg24.clone()), Some(agg24));
    add("hfa1", a1(hfa1.clone()), Some(hfa1));
    add("hfa2", a1(hfa2.clone()), Some(hfa2));
    add("hfa4", a1(hfa4.clone()), Some(hfa4));
    add("many_args", many, None);
    add(
        "variadic",
        vec![("fmt".to_string(), ptrv), ("x".to_string(), f64v)],
        None,
    );
    // 最后一条是真变参（`fixed = 1`）。
    if let Some(last) = out.last_mut()
        && last.0 == "variadic"
    {
        last.1 = last.1.clone().variadic(1);
    }
    out
}

// ─────────────────────────── abi plan ───────────────────────────

fn cmd_plan(args: &[String], json: bool) -> Result<ExitCode, String> {
    let (rest, conv) = take_value(args, "--conv")?;
    let (rest, sig_text) = take_value(&rest, "--sig")?;
    let (rest, variadic) = take_value(&rest, "--variadic")?;
    let files: Vec<PathBuf> = rest
        .iter()
        .filter(|a| !a.starts_with('-'))
        .map(PathBuf::from)
        .collect();
    let [file] = files.as_slice() else {
        return Err("abi plan 需要恰好一个谱文件".into());
    };
    let Some(conv) = conv else {
        return Err("abi plan 需要 `--conv <约定名>`（如 win64；`abi list` 可看全部）".into());
    };
    let Some(sig_text) = sig_text else {
        return Err("abi plan 需要 `--sig \"i64, f64 -> i64\"`".into());
    };
    let view = abi_view::inspect(file).map_err(|m| format!("谱读不了：\n{m}"))?;
    let target = ViewTarget::new(&view);
    let mut sig = parse_sig(&sig_text, target.addr_bytes())?;
    if let Some(n) = variadic {
        let n: usize = n
            .parse()
            .map_err(|_| format!("`--variadic` 需要整数（命名参数个数），得到 `{n}`"))?;
        sig = sig.variadic(n);
    }
    let reg = builtin::registry().map_err(|e| e.to_string())?;
    match reg.plan(&target, &conv, &sig) {
        Ok(plan) => {
            if json {
                println!(
                    "{{\"isa\": {}, \"conv\": {}, \"plan\": {}}}",
                    q(&view.isa),
                    q(&conv),
                    q(&plan.to_text())
                );
            } else {
                println!("# {} / {conv} / {}", view.isa, sig_text);
                print!("{}", plan.to_text());
            }
            Ok(ExitCode::SUCCESS)
        }
        Err(e) => {
            println!("{conv}: {e}");
            Ok(ExitCode::from(1))
        }
    }
}

/// `--sig` 的语法：逗号分隔的类型列表，`->` 之后是返回类型（可省）。
///
/// | 写法 | 类型 |
/// | --- | --- |
/// | `i8`/`i16`/`i32`/`i64`/`i128` | 整数（按名字给宽度） |
/// | `f32`/`f64` | 浮点 |
/// | `ptr` | 指针（宽度 = 本机地址宽） |
/// | `v<N>` | 向量，N 字节、`f32` 元素 |
/// | `agg<N>` | 不透明聚合 N 字节（整数成员 → **不是 HFA**） |
/// | `hfa<N>` | N 个 `f32` 的同质浮点聚合 |
/// | `hf64<N>` | N 个 `f64` 的同质浮点聚合 |
///
/// 参数可以带名字：`a:i64`（只影响报告可读性）。
fn parse_sig(text: &str, addr_bytes: u32) -> Result<Signature, String> {
    let (params_text, ret_text) = match text.split_once("->") {
        Some((a, b)) => (a, b.trim()),
        None => (text, ""),
    };
    let mut params: Vec<(String, TyView)> = Vec::new();
    for (i, raw) in params_text
        .split(',')
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .enumerate()
    {
        let (name, ty) = match raw.split_once(':') {
            Some((n, t)) => (n.trim().to_string(), t.trim()),
            None => (format!("a{i}"), raw),
        };
        params.push((name, parse_ty(ty, addr_bytes)?));
    }
    let ret = if ret_text.is_empty() {
        None
    } else {
        Some(parse_ty(ret_text, addr_bytes)?)
    };
    Ok(Signature::new(params, ret))
}

fn parse_ty(t: &str, addr_bytes: u32) -> Result<TyView, String> {
    let bad = || format!("看不懂的类型 `{t}`（见 abi plan --help 的类型表）");
    if let Some(rest) = t.strip_prefix('i') {
        if let Ok(bits) = rest.parse::<u32>()
            && bits > 0
            && bits % 8 == 0
        {
            return Ok(TyView::int(bits / 8, bits / 8));
        }
        return Err(bad());
    }
    if let Some(rest) = t.strip_prefix('f') {
        if let Ok(bits) = rest.parse::<u32>()
            && bits % 8 == 0
            && bits > 0
        {
            return Ok(TyView::float(bits / 8));
        }
        return Err(bad());
    }
    if t == "ptr" {
        return Ok(TyView::ptr(addr_bytes.max(1)));
    }
    if let Some(rest) = t.strip_prefix('v') {
        if let Ok(bytes) = rest.parse::<u32>()
            && bytes > 0
            && bytes % 4 == 0
        {
            return Ok(TyView::vector(Elem::Float, bytes / 4, 4));
        }
        return Err(format!("`v<N>` 的 N 要是 4 的倍数（f32 元素），得到 `{t}`"));
    }
    if let Some(rest) = t.strip_prefix("hf64") {
        let n: u32 = rest.parse().map_err(|_| bad())?;
        return Ok(TyView::agg(8, vec![TyView::float(8); n as usize]));
    }
    if let Some(rest) = t.strip_prefix("hfa") {
        let n: u32 = rest.parse().map_err(|_| bad())?;
        return Ok(TyView::agg(4, vec![TyView::float(4); n as usize]));
    }
    if let Some(rest) = t.strip_prefix("agg") {
        let bytes: u32 = rest.parse().map_err(|_| bad())?;
        return Ok(opaque_agg(bytes));
    }
    Err(bad())
}

/// 不透明聚合：`n/8` 个 `i64` + 余下的 `i8`（成员宽度不同 ⇒ **不是** HFA）。
fn opaque_agg(bytes: u32) -> TyView {
    let mut members = vec![TyView::int(8, 8); (bytes / 8) as usize];
    if !bytes.is_multiple_of(8) {
        members.push(TyView::int(1, 1));
    }
    if members.is_empty() {
        members.push(TyView::int(1, 1));
    }
    TyView::agg(bytes.clamp(1, 8), members)
}

// ─────────────────────────── 谱 → AbiTarget ───────────────────────────

/// `MachineView` 的 `AbiTarget` 适配器：**只答能力，不答约定**。
struct ViewTarget {
    isa: String,
    count: u32,
    names: Vec<Option<String>>,
    /// 名字（含别名 `EAX`/`W0`/`ZMM0`…）→ 物理号。
    alias: BTreeMap<String, u32>,
    classes: Vec<String>,
    widths: Vec<u8>,
    pinned: Vec<bool>,
    allocatable: Vec<u32>,
    scratch: Vec<u32>,
    link: Option<u32>,
    caps: BTreeMap<&'static str, u16>,
    addr_bytes: u32,
}

impl ViewTarget {
    fn new(view: &MachineView) -> Self {
        let count = view.n_gpr + view.n_fp;
        let pinned_set: Vec<u32> = view.pinned.clone();
        let names: Vec<Option<String>> = (0..count)
            .map(|i| view.reg_name(i).map(|s| s.to_string()))
            .collect();
        let mut classes = Vec::with_capacity(count as usize);
        let mut widths = Vec::with_capacity(count as usize);
        for i in 0..count {
            let e = &view.regs[i as usize];
            classes.push(e.class.clone());
            widths.push(e.width);
        }
        Self {
            isa: view.isa.clone(),
            count,
            names,
            alias: view.names.clone(),
            classes,
            widths,
            pinned: (0..count).map(|i| pinned_set.contains(&i)).collect(),
            allocatable: (0..count).filter(|i| !pinned_set.contains(i)).collect(),
            scratch: view.scratch.clone(),
            link: view.link,
            caps: abi_view::declared_capabilities(view),
            addr_bytes: view.regs.first().map(|r| r.width as u32).unwrap_or(8),
        }
    }

    fn addr_bytes(&self) -> u32 {
        self.addr_bytes
    }
}

impl AbiTarget for ViewTarget {
    fn isa_name(&self) -> &str {
        &self.isa
    }
    fn reg_count(&self) -> u32 {
        self.count
    }
    fn reg_name(&self, index: u32) -> Option<String> {
        self.names.get(index as usize).cloned().flatten()
    }
    fn reg_class_name(&self, index: u32) -> String {
        self.classes
            .get(index as usize)
            .cloned()
            .unwrap_or_else(|| "?".into())
    }
    fn reg_index(&self, name: &str) -> Option<u32> {
        self.alias.get(name).copied()
    }
    fn reg_width(&self, index: u32) -> u8 {
        self.widths.get(index as usize).copied().unwrap_or(0)
    }
    fn pinned(&self, index: u32) -> bool {
        self.pinned.get(index as usize).copied().unwrap_or(true)
    }
    fn allocatable(&self) -> Vec<u32> {
        self.allocatable.clone()
    }
    fn spill_scratch(&self) -> Vec<u32> {
        self.scratch.clone()
    }
    fn link_reg(&self) -> Option<u32> {
        self.link
    }
    fn cap(&self, cap: Capability) -> Option<u16> {
        self.caps.get(capability_name(cap)).copied()
    }
}

fn capability_name(c: Capability) -> &'static str {
    match c {
        Capability::GprMov => "gpr_mov",
        Capability::FprMov => "fpr_mov",
        Capability::VecMov => "vec_mov",
        Capability::SpAdjust => "sp_adjust",
        Capability::StackArgLoad => "stack_arg_load",
        Capability::StackArgStore => "stack_arg_store",
        Capability::FrameAddr => "frame_addr",
        Capability::WideVecMove => "wide_vec_move",
        Capability::Call => "call",
        Capability::CallIndirect => "call_indirect",
        Capability::Ret => "ret",
    }
}

// ─────────────────────────── 小工具 ───────────────────────────

/// 取 `--flag <值>`；返回 (剩余参数, 值)。
fn take_value(args: &[String], flag: &str) -> Result<(Vec<String>, Option<String>), String> {
    let mut rest = Vec::new();
    let mut value = None;
    let mut i = 0;
    while i < args.len() {
        if args[i] == flag {
            let Some(v) = args.get(i + 1) else {
                return Err(format!("`{flag}` 需要一个取值"));
            };
            value = Some(v.clone());
            i += 2;
        } else {
            rest.push(args[i].clone());
            i += 1;
        }
    }
    Ok((rest, value))
}

fn q(s: &str) -> String {
    let mut out = String::with_capacity(s.len() + 2);
    out.push('"');
    for c in s.chars() {
        match c {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            c if (c as u32) < 0x20 => out.push_str(&format!("\\u{:04x}", c as u32)),
            c => out.push(c),
        }
    }
    out.push('"');
    out
}
