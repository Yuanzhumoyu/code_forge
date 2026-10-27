//! **诊断事件出口**（v26 M1）：四个 `FORGE_*_EVENTS` 通道的**唯一**落盘实现 + 通道注册表。
//!
//! ## 为什么在这里，而不是各自的 crate
//!
//! 立项审查（2026-10-11）实测：同一件事（"设了环境变量就把一行追加到文件，写失败静默"）
//! 被**抄了四遍**——`forge-codegen` 的 codegen 事件与 `jit_event`、`forge-tests` 的 asm 事件、
//! `forge-rustc` e2e 的事件；四个环境变量名也散在四处。这一层把"开文件 / 追加 / 静默失败"
//! 收成一个实现，并把**通道名 ↔ 环境变量**变成一张可守卫的表。
//!
//! 放在 `forge-isa-runtime`：它是**叶层**（无内部依赖），生成物的运行期支持、JIT 宿主与各测试面
//! 都依赖它——与 `cpu.rs`（主机探测）同性质：都是"宿主侧基础设施"，而 `pipeline::trace` 的
//! 开关台账也一并下沉到这里（见 [`KNOBS`]）。
//!
//! ## 契约
//!
//! * **两个渲染**：`emit` 写一行 JSON（codegen 的机读面）；`emit_text` 写一行**逐字文本**——
//!   jit/asm/e2e 三个通道的既有格式被 CI 的 `Select-String` 依赖（`MATRIX-SKIP`、`RETRY`、
//!   `ASM-EXEC-SKIP`…），本层**不改它们的格式**，只统一"怎么落盘"。
//! * **写失败静默**：可见性辅助**不得**影响编译/测试结论（四个通道一致）。
//! * **未登记通道不写**：debug 下 `debug_assert` 当场暴露，release 下警告一次并放弃
//!   （诊断面 fail-closed：宁可少一行事件，也不写进一个没人认识的通道）。
//!
//! 本模块同时持有**观测开关台账** [`KNOBS`]（`FORGE_TRACE_*`）：v26 M1 起后端与 `forge-rustc`
//! 共用这一张表，`knob` 是唯一查询入口（未登记不静默）。

use std::sync::{Mutex, OnceLock};

/// 一个诊断通道：名字 + 它读的环境变量 + 用途。
#[derive(Debug, Clone, Copy)]
pub struct Channel {
    /// 通道名（`emit`/`emit_text` 的第一个参数）。
    pub name: &'static str,
    /// 指向落盘文件的环境变量。
    pub env: &'static str,
    /// 什么时候用、什么格式。
    pub what: &'static str,
}

/// **通道注册表**（唯一登记处；守卫 `event_channels_are_bijective` 与契约文档表双向相等）。
pub static CHANNELS: [Channel; 4] = [
    Channel {
        name: "codegen",
        env: "FORGE_CODEGEN_EVENTS",
        what: "编译管线的机读事件（**JSON 行**：`{\"event\":…}`）",
    },
    Channel {
        name: "jit",
        env: "FORGE_JIT_EVENTS",
        what: "JIT 矩阵的可见性事件（文本：`<事件> <用例>`）",
    },
    Channel {
        name: "asm",
        env: "FORGE_ASM_EVENTS",
        what: "汇编语料测试台事件（文本：`<事件> <说明>`）",
    },
    Channel {
        name: "e2e",
        env: "FORGE_E2E_EVENTS",
        what: "forge-rustc e2e 事件（文本：`[<事件>] <用例> <细节>`）",
    },
];

/// 事件的字段值（固定类型集合 ⇒ 输出确定、无需 JSON 依赖）。
#[derive(Debug, Clone, Copy)]
pub enum Field<'a> {
    Str(&'a str),
    U64(u64),
    /// **有符号**整数（v27 M6-87）：帧偏移这类值可能是负数（`[rbp-96]`）。
    /// 早先只有无符号变体 ⇒ 调用方只能报绝对值，读的人分不清高低。
    I64(i64),
    Usize(usize),
}

impl std::fmt::Display for Field<'_> {
    /// JSON 最小转义：只需处理 `"`、`\` 与换行（事件字段都是函数名/ID 这类短标识）。
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Field::Str(s) => {
                write!(f, "\"")?;
                for c in s.chars() {
                    match c {
                        '"' => write!(f, "\\\"")?,
                        '\\' => write!(f, "\\\\")?,
                        '\n' => write!(f, "\\n")?,
                        _ => write!(f, "{c}")?,
                    }
                }
                write!(f, "\"")
            }
            Field::U64(v) => write!(f, "{v}"),
            Field::I64(v) => write!(f, "{v}"),
            Field::Usize(v) => write!(f, "{v}"),
        }
    }
}

/// 追加**一行 JSON**（`{"event":"…",…}`；字段序 = 调用点给的顺序 ⇒ 输出确定）。
pub fn emit(channel: &str, event: &str, fields: &[(&str, Field<'_>)]) {
    let mut line = format!("{{\"event\":\"{event}\"");
    for (k, v) in fields {
        line.push_str(&format!(",\"{k}\":{v}"));
    }
    line.push('}');
    append(channel, &line);
}

/// 追加**一行文本**（逐字，不解析；三个文本通道的格式由各自调用点决定，见模块头）。
pub fn emit_text(channel: &str, line: &str) {
    append(channel, line);
}

/// 通道的落盘环境变量（未登记 ⇒ `None`）。
pub fn env_of(channel: &str) -> Option<&'static str> {
    CHANNELS.iter().find(|c| c.name == channel).map(|c| c.env)
}

fn append(channel: &str, line: &str) {
    use std::io::Write;
    let Some(env) = env_of(channel) else {
        debug_assert!(
            false,
            "未登记的事件通道 `{channel}`（见 diagnostics::CHANNELS）"
        );
        warn_unregistered_once(channel);
        return;
    };
    let Some(path) = std::env::var_os(env) else {
        return;
    };
    let Ok(mut f) = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(path)
    else {
        return; // 写失败静默：可见性辅助不得影响结论
    };
    let _ = writeln!(f, "{line}");
}

/// release 下"未登记通道"只警告一次（debug 下上面那句 `debug_assert` 已经当场暴露）。
fn warn_unregistered_once(channel: &str) {
    use std::sync::Once;
    static ONCE: Once = Once::new();
    ONCE.call_once(|| {
        eprintln!(
            "[forge] 事件通道 `{channel}` 未登记进 diagnostics::CHANNELS——本行事件被丢弃（诊断面 fail-closed）"
        );
    });
}

// ─────────────────────────── 观测开关台账（v24 M4 → v26 M1 下沉） ───────────────────────────

/// 一个观测开关：环境变量名 + 它看什么。
pub struct Knob {
    /// 环境变量名（`FORGE_TRACE_*`）。
    pub name: &'static str,
    /// 这个开关打开后会看到什么（写给"要不要加它"的人看）。
    pub what: &'static str,
    /// 查询结果缓存（环境变量在运行时不会变；热路径上不能每次 `var_os`）。
    on: OnceLock<bool>,
}

impl Knob {
    const fn new(name: &'static str, what: &'static str) -> Self {
        Self {
            name,
            what,
            on: OnceLock::new(),
        }
    }
}

/// **观测开关台账**（唯一登记处；守卫 `trace_knobs_are_bijective` 钉住
/// 「源码里的 `FORGE_TRACE_*` 字面量 ↔ 本表 ↔ 契约文档 §7 的表」三方双向相等）。
///
/// 用 `static` 数组而不是 `const &[…]`：每行带一个 [`OnceLock`] 缓存（内部可变性），
/// `const` 提升到 `'static` 的临时量不允许带内部可变共享借用。
///
/// v26 M1 从 `forge-codegen::pipeline::trace` 下沉到这里，并把**两侧**的名字并成一张表
/// （后端 11 项 + 直读项 + `forge-rustc` 27 项）。下沉的连带收获：原先 5 处**绕过台账直读**
/// 环境变量的地方（`FN_SIZE`/`CODE_HEX`/`LIVE`/`DW`）改走本 API，"漏登记"这一类再无处可藏。
pub static KNOBS: [Knob; 44] = [
    Knob::new(
        "FORGE_TRACE_ABI",
        "调用约定规划与落点回填（forge-codegen abi_setup/compiler；forge-rustc FnAbi 形态）",
    ),
    Knob::new(
        "FORGE_TRACE_AGG",
        "聚合展开与合法化（legalize 各 pass / 计划相 / 物化相）",
    ),
    Knob::new("FORGE_TRACE_ALLOC", "寄存器分配主循环（regalloc_bt）"),
    Knob::new(
        "FORGE_TRACE_CALLCLOBBER",
        "调用点的约定级破坏集合并（pipeline/clobbers；调用跨寄存器分配的对拍点）",
    ),
    Knob::new("FORGE_TRACE_ARGS", "调用实参打包明细（forge-rustc mod.rs）"),
    Knob::new(
        "FORGE_TRACE_ASSERT",
        "编译期断言与 const 求值路径（forge-rustc）",
    ),
    Knob::new(
        "FORGE_TRACE_BC",
        "机器码字节 dump（forge-rustc compile.rs；配 llvm-objdump 对照）",
    ),
    Knob::new(
        "FORGE_TRACE_BLOCK",
        "基本块 lowering 边界（forge-rustc mod.rs）",
    ),
    Knob::new(
        "FORGE_TRACE_CALL",
        "直接调用：callee 签名与实参打包（forge-rustc mod.rs）",
    ),
    Knob::new("FORGE_TRACE_CAST", "类型转换 lowering（forge-rustc）"),
    Knob::new("FORGE_TRACE_CGU", "codegen unit 划分（forge-rustc）"),
    Knob::new(
        "FORGE_TRACE_CODE_HEX",
        "函数机器码 hex dump（forge-codegen compiler；`FN_SIZE` 的细节档）",
    ),
    Knob::new("FORGE_TRACE_CONST", "常量求值（forge-rustc const_eval.rs）"),
    Knob::new(
        "FORGE_TRACE_DISCR",
        "判别式（discriminant）取值与布局（forge-rustc）",
    ),
    Knob::new(
        "FORGE_TRACE_DW",
        "DWARF 调试信息生成（forge-rustc dwarf.rs）",
    ),
    Knob::new(
        "FORGE_TRACE_EMIT",
        "机器码发射（emission；`[machine.text]` 相关诊断也归此）",
    ),
    Knob::new("FORGE_TRACE_EXPIRE", "活跃区间到期处理（regalloc_bt）"),
    Knob::new("FORGE_TRACE_FIELD", "字段偏移计算（forge-rustc mod.rs）"),
    Knob::new(
        "FORGE_TRACE_FN",
        "每个函数的 lowering 开始（forge-rustc mod.rs）",
    ),
    Knob::new(
        "FORGE_TRACE_FN_SIZE",
        "每个函数的入口/体积摘要（forge-codegen compiler）",
    ),
    Knob::new("FORGE_TRACE_GLOBAL", "全局与重定位符号解析（forge-rustc）"),
    Knob::new(
        "FORGE_TRACE_IR",
        "IR 构建期 dump（forge-ir builder —— 该层不允许依赖运行时，故实现在本 crate 内）",
    ),
    Knob::new(
        "FORGE_TRACE_LIVE",
        "活区间全量 dump（forge-codegen liverange；调 regalloc 用）",
    ),
    Knob::new(
        "FORGE_TRACE_LIVE_FN",
        "活区间 dump 的**函数名过滤**（FORGE_TRACE_LIVE 同时开；值 = 子串匹配）",
    ),
    Knob::new("FORGE_TRACE_LOAD", "load lowering（forge-rustc）"),
    Knob::new("FORGE_TRACE_LOC", "源码位置/行号映射（forge-rustc mod.rs）"),
    Knob::new("FORGE_TRACE_LOWER", "指令选择（lowering；每指令一条）"),
    Knob::new("FORGE_TRACE_M4", "绑定折入与多类槽回填类诊断（compiler）"),
    Knob::new("FORGE_TRACE_META", "实例元数据（forge-rustc）"),
    Knob::new(
        "FORGE_TRACE_MIR",
        "每条 MIR statement（forge-rustc mod.rs）",
    ),
    Knob::new("FORGE_TRACE_PAIR", "ScalarPair 判定与偏移（forge-rustc）"),
    Knob::new(
        "FORGE_TRACE_PLACE",
        "place 投影 lowering（forge-rustc place.rs）",
    ),
    Knob::new("FORGE_TRACE_REGALLOC", "寄存器分配编排（regalloc_bt 入口）"),
    Knob::new("FORGE_TRACE_SLOT", "栈槽分配（forge-rustc mod.rs）"),
    Knob::new("FORGE_TRACE_SPILL", "溢出与回填（emission / regalloc_bt）"),
    Knob::new("FORGE_TRACE_STACK", "栈槽与帧布局（lowering）"),
    Knob::new(
        "FORGE_TRACE_TIMING",
        "编译阶段计时（`[codegen 名字] total/lower/peep/regalloc/frame/emit`；基准与专项分析用；原名 `CF_CODEGEN_TIMING`）",
    ),
    Knob::new("FORGE_TRACE_STMT", "MIR statement 级跟踪（forge-rustc）"),
    Knob::new("FORGE_TRACE_STORE", "store lowering（forge-rustc）"),
    Knob::new(
        "FORGE_TRACE_SUBSTS",
        "泛型替换解析（FnDef → instance，forge-rustc mod.rs）",
    ),
    Knob::new("FORGE_TRACE_SYM", "符号名计算（forge-rustc mod.rs）"),
    Knob::new("FORGE_TRACE_TERM", "终结符 lowering（forge-rustc mod.rs）"),
    Knob::new(
        "FORGE_TRACE_VAR",
        "局部变量与调试变量（forge-rustc mod.rs）",
    ),
    Knob::new("FORGE_TRACE_VCODE", "虚拟指令流（compiler）"),
];

/// 查询观测开关 `name`（**全名**，如 `FORGE_TRACE_AGG`）是否打开。
///
/// * 已登记：命中台账并缓存结果（热路径零开销）。
/// * **未登记**：debug 下 panic（让"忘登记/拼错"当场暴露）；release 下照常按环境变量生效，
///   但**警告一次**——绝不静默（`FORGE_TRACE_AGG` / `FORGE_TRACE_M4` 就是这样静默死了几个月）。
pub fn knob(name: &str) -> bool {
    if let Some(row) = KNOBS.iter().find(|k| k.name == name) {
        return *row.on.get_or_init(|| std::env::var_os(row.name).is_some());
    }
    debug_assert!(
        false,
        "观测开关 `{name}` 没有登记进 `diagnostics::KNOBS` —— 登记它（或改用已登记的名字）。"
    );
    warn_unregistered(name);
    std::env::var_os(name).is_some()
}

/// 查询 `FORGE_TRACE_<suffix>`（`forge-rustc` 侧的写法：只给后缀，如 `"SLOT"`）。
pub fn trace(suffix: &str) -> bool {
    knob(&format!("FORGE_TRACE_{suffix}"))
}

/// 未登记开关只警告一次（避免诊断面刷屏）。
fn warn_unregistered(name: &str) {
    static WARNED: OnceLock<Mutex<std::collections::BTreeSet<String>>> = OnceLock::new();
    let set = WARNED.get_or_init(|| Mutex::new(std::collections::BTreeSet::new()));
    let Ok(mut set) = set.lock() else {
        return;
    };
    if set.insert(name.to_string()) {
        eprintln!(
            "[forge] 观测开关 `{name}` 没登记进 `diagnostics::KNOBS` —— 它仍按环境变量生效，\
             但台账/文档/守卫都看不到它（补一行登记）"
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 注册表自洽：名字/环境变量唯一、非空、环境变量有 `FORGE_` 前缀。
    #[test]
    fn channels_table_is_sane() {
        let mut names: Vec<&str> = CHANNELS.iter().map(|c| c.name).collect();
        let mut envs: Vec<&str> = CHANNELS.iter().map(|c| c.env).collect();
        let n = names.len();
        names.sort_unstable();
        names.dedup();
        envs.sort_unstable();
        envs.dedup();
        assert_eq!(names.len(), n, "通道名必须唯一");
        assert_eq!(envs.len(), n, "环境变量必须唯一");
        for c in CHANNELS.iter() {
            assert!(!c.name.is_empty() && !c.what.is_empty(), "登记项不得留空");
            assert!(
                c.env.starts_with("FORGE_"),
                "环境变量应以 FORGE_ 开头：{}",
                c.env
            );
        }
    }

    /// 字段转义：字符串带引号/反斜杠/换行不得破坏 JSON 行。
    #[test]
    fn field_escaping() {
        assert_eq!(format!("{}", Field::Str("a\"b\\c")), "\"a\\\"b\\\\c\"");
        assert_eq!(format!("{}", Field::Str("a\nb")), "\"a\\nb\"");
        assert_eq!(format!("{}", Field::U64(7)), "7");
        assert_eq!(format!("{}", Field::Usize(9)), "9");
    }

    /// 台账本身要自洽：名字唯一、前缀统一、说明非空。
    #[test]
    fn knobs_table_is_sane() {
        let mut seen = std::collections::BTreeSet::new();
        for k in KNOBS.iter() {
            assert!(seen.insert(k.name), "重复登记：{}", k.name);
            assert!(k.name.starts_with("FORGE_TRACE_"), "名字前缀：{}", k.name);
            assert!(!k.what.is_empty(), "缺说明：{}", k.name);
        }
        // 条数 = KNOBS 表长，且必须与 `docs/reference/backend-pipeline.md` 的开关表一致
        //（44 = 含 `FORGE_TRACE_CALLCLOBBER` 与 v27 的 `FORGE_TRACE_LIVE_FN`；**改表就一起
        //  改这里**——这条守卫就是为此存在，它此前漏在 `quick`/`rustc` 子集之外，条数漂了
        //  整整一轮才被抓到）。
        assert_eq!(KNOBS.len(), 44, "台账条数变化时同步文档表与守卫");
    }

    /// 已登记的开关真的按环境变量生效（缓存路径给出确定答案，不 panic、不警告）。
    #[test]
    fn registered_knob_reads_env() {
        let a = knob("FORGE_TRACE_ABI");
        let b = knob("FORGE_TRACE_ABI");
        assert_eq!(a, b, "同一开关两次查询必须一致（缓存语义）");
        assert_eq!(trace("ABI"), a, "`trace(后缀)` 与全名查询必须同源");
    }

    /// 未登记的名字在 debug 下 panic（拼错/忘登记当场暴露，而不是静默恒假）。
    #[test]
    fn unregistered_knob_panics_in_debug() {
        let r = std::panic::catch_unwind(|| knob("FORGE_TRACE_NOT_REGISTERED_AT_ALL"));
        if cfg!(debug_assertions) {
            assert!(r.is_err(), "未登记的开关名必须在 debug 下 panic");
        } else {
            assert!(r.is_ok());
        }
    }

    /// 未登记通道查不到环境变量（fail-closed ⇒ `append` 只警告不写）。
    #[test]
    fn unknown_channel_has_no_env() {
        assert_eq!(env_of("nope-not-registered"), None);
        assert_eq!(env_of("codegen"), Some("FORGE_CODEGEN_EVENTS"));
    }
}
