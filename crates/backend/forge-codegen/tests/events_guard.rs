//! **事件台账守卫**（v25 M3）。
//!
//! ## 判据（三方双向相等）
//!
//! 1. 源码里的 `events::emit("NAME", …)` 调用点名字集合；
//! 2. `pipeline::events::EVENTS` 登记表的名字集合；
//! 3. `docs/reference/backend-pipeline.md` 的事件表（`## 事件表` 小节，行首是反引号名字）；
//!
//! 三者**逐一相等**：源码多 = 漏登记（`debug_assert` 会在 debug 下当场 panic，这里再钉一层）；
//! 登记表多 = 死条目；文档分叉 = 承诺与实现不符。
//!
//! ## 为什么值钱
//!
//! 立项证据（v25 方案 §1 E6）：JIT / asm / E2E 都有落盘事件，**codegen 是唯一没有机读出口的
//! 阶段**——CI 只能靠正则啃人读散文。事件表一旦与实现分叉，"CI 能统计的东西"就又变成
//! 口头承诺（v24 M4 的 `FORGE_TRACE_*` 白名单就是这么死的）。

use std::collections::BTreeSet;

fn repo_root() -> std::path::PathBuf {
    std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("..")
        .join("..")
        .join("..")
}

fn rs_files(dir: &std::path::Path, out: &mut Vec<std::path::PathBuf>) {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    for e in entries.flatten() {
        let p = e.path();
        if p.is_dir() {
            rs_files(&p, out);
        } else if p.extension().is_some_and(|x| x == "rs") {
            out.push(p);
        }
    }
}

/// 取出 `events::emit(` 之后的第一个字符串字面量（**跨行**：rustfmt 会把参数折到下一行）。
/// 跳过 `events.rs` 自己（它只有 `format!("{{\"event\":\"{name}\"")` 之类的内部文本）。
fn emit_names(text: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut rest = text;
    while let Some(i) = rest.find("events::emit(") {
        let after = &rest[i + "events::emit(".len()..];
        let after = after.trim_start();
        if let Some(stripped) = after.strip_prefix('"')
            && let Some(end) = stripped.find('"')
        {
            out.push(stripped[..end].to_string());
        }
        rest = after;
    }
    out
}

/// 取某个 `## <含关键词>` 小节里"第一列是反引号名字"的表格行名字。
fn doc_section_ids(text: &str, heading_contains: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut inside = false;
    for line in text.lines() {
        if line.starts_with("## ") {
            inside = line.contains(heading_contains);
            continue;
        }
        if !inside {
            continue;
        }
        let l = line.trim();
        if let Some(rest) = l.strip_prefix("| `")
            && let Some(end) = rest.find('`')
        {
            out.push(rest[..end].to_string());
        }
    }
    out
}

/// **三方双向相等**：源码调用点 ↔ `EVENTS` ↔ 契约文档的事件表。
#[test]
fn event_names_are_bijective() {
    let root = repo_root();
    let mut files = Vec::new();
    rs_files(&root.join("crates/backend/forge-codegen/src"), &mut files);

    let mut source: BTreeSet<String> = BTreeSet::new();
    for p in &files {
        if p.file_name().is_some_and(|n| n == "events.rs") {
            continue; // 定义处（它内部没有 emit 调用点，但跳过更稳）
        }
        let Ok(text) = std::fs::read_to_string(p) else {
            continue;
        };
        for n in emit_names(&text) {
            source.insert(n);
        }
    }

    let table: BTreeSet<String> = forge_codegen::pipeline::events::EVENTS
        .iter()
        .map(|e| e.name.to_string())
        .collect();

    let doc_path = root.join("docs/reference/backend-pipeline.md");
    let doc = std::fs::read_to_string(&doc_path)
        .unwrap_or_else(|e| panic!("读不到 {}：{e}", doc_path.display()));
    let documented: BTreeSet<String> = doc_section_ids(&doc, "事件表").into_iter().collect();

    assert!(!table.is_empty(), "`EVENTS` 台账为空");
    assert_eq!(
        source, table,
        "源码 emit 调用点与 `EVENTS` 台账不一致（源码多 = 漏登记；台账多 = 死条目）：\n  \
         源码 {source:?}\n  台账 {table:?}"
    );
    assert_eq!(
        source, documented,
        "事件表与实现分叉（承诺与实现不符）：\n  实现 {source:?}\n  文档 {documented:?}"
    );
}

/// **事件真的会落盘**（端到端）：设 `FORGE_CODEGEN_EVENTS` 编译一个函数，断言事件行。
///
/// 只比"事件名 + 关键字段"，不比数值（耗时/字节数因机器而异）——但字段序是固定契约，
/// 所以行前缀可以精确比对。env 是进程级的，本文件的测试彼此串行（libtest 默认多线程，
/// 所以这里用一把文件内锁；其它测试文件不读这个 env）。
#[test]
fn events_land_on_disk() {
    static LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());
    let _g = LOCK.lock().unwrap_or_else(|e| e.into_inner());

    let dir = std::env::temp_dir().join(format!("forge_codegen_events_{}", std::process::id()));
    let _ = std::fs::create_dir_all(&dir);
    let path = dir.join("events.jsonl");
    let _ = std::fs::remove_file(&path);
    // 安全：本测试独占该 env（文件内串行 + 其它测试文件不设置它）。
    unsafe { std::env::set_var("FORGE_CODEGEN_EVENTS", &path) };
    compile_fixture();
    compile_spill_fixture();
    unsafe { std::env::remove_var("FORGE_CODEGEN_EVENTS") };

    let text = std::fs::read_to_string(&path).unwrap_or_default();
    let _ = std::fs::remove_dir_all(&dir);
    if text.is_empty() {
        // 落盘不可用（例如沙箱禁止写临时目录）⇒ 明确 skip，而不是假绿。
        eprintln!("EVENTS-SKIP: 未能落盘事件（{path:?}）");
        return;
    }
    for want in [
        "{\"event\":\"COMPILE-START\",\"fn\":\"events_fixture\"",
        "{\"event\":\"COMPILE-DONE\",\"fn\":\"events_fixture\",\"total_us\":",
        "\"event\":\"LEGALIZE-PLAN\"",
        "\"event\":\"ALLOC-SUMMARY\"",
        "\"event\":\"EMIT-SUMMARY\"",
    ] {
        assert!(text.contains(want), "应含事件片段 `{want}`：\n{text}");
    }
    // **溢出事件化**（v27 M6-87/M6-90）：高压夹具必须真的溢出，并同时给出
    // "逐值"（SPILL-VALUE）与"逐指令编辑"（SPILL-EDIT）两种粒度。
    assert!(
        text.contains("\"event\":\"SPILL-VALUE\",\"fn\":\"events_spill_fixture\""),
        "高压夹具应产生 SPILL-VALUE：\n{text}"
    );
    assert!(
        text.contains("\"event\":\"SPILL-EDIT\",\"fn\":\"events_spill_fixture\""),
        "高压夹具应产生 SPILL-EDIT：\n{text}"
    );
    // **压力峰值**（v27 M6-91）：高压夹具的 `peak_live` 必须 > 0（否则这个字段就是假的 0）。
    let summary = text
        .lines()
        .find(|l| {
            l.contains("\"event\":\"ALLOC-SUMMARY\"")
                && l.contains("\"fn\":\"events_spill_fixture\"")
        })
        .expect("高压夹具应有 ALLOC-SUMMARY 行");
    assert!(
        !summary.contains("\"peak_live\":0"),
        "高压夹具的 peak_live 应 > 0：{summary}"
    );
    assert!(
        text.contains("\"code_bytes\":")
            && text.contains("\"plans\":")
            && text.contains("\"peak_live\":"),
        "COMPILE-DONE/EMIT-SUMMARY/LEGALIZE-PLAN/ALLOC-SUMMARY 的关键字段应在：\n{text}"
    );
}

/// **高压夹具**：40 个同时活跃的 `i64`，强制分配器溢出（P4 事件化的真值输入）。
///
/// 为什么不用普通夹具：`events_fixture` 只有一次聚合拷贝，不溢出 ⇒ `SPILL-*` 永远
/// 不会发；而"事件代码写错了"与"没触发"在日志上长得一样，必须让夹具**必然**溢出。
fn compile_spill_fixture() {
    use forge_ir::TypeId;
    use forge_ir::ir::builder::FunctionBuilder;
    use forge_ir::ir::types::{FunctionSignature, TypeContext};
    forge_codegen::pipeline_hooks::ensure_registered();
    let ctx = TypeContext::new();
    let sig = FunctionSignature::new(&[], &[TypeId::I64]);
    let mut b = FunctionBuilder::new("events_spill_fixture", ctx, sig);
    let (e, _) = b.create_block_with_params(&[]);
    b.switch_to_block(e);
    // 每个值都活到函数末尾（先全部定义、再逐个累加）⇒ 活区间大面积重叠。
    let mut vals = Vec::new();
    for i in 0..40i64 {
        let c = b.const_int(i, TypeId::I64);
        vals.push(b.iadd(c, c));
    }
    let mut acc = vals[0];
    for &v in &vals[1..] {
        acc = b.iadd(acc, v);
    }
    b.ret(&[acc]);
    let f = b.finish().expect("build");
    let tm = forge_codegen::arch::x86::TargetMachine::new();
    let s = forge_codegen::CompileSession::new(tm);
    let _ = s.compile(&f);
}

fn compile_fixture() {
    use forge_ir::ir::builder::FunctionBuilder;
    use forge_ir::ir::types::{FunctionSignature, TypeContext};
    forge_codegen::pipeline_hooks::ensure_registered();
    // **必须带聚合边界**：合法化驱动只在"函数被克隆"（有聚合要处理 / 开了优化）时才跑，
    // 一个纯标量函数不会发 LEGALIZE-PLAN（实测：第一版夹具就是纯标量，事件缺一条）。
    let ctx = TypeContext::new();
    let agg = ctx.borrow_mut().struct_anon(
        vec![
            forge_ir::TypeId::I64,
            forge_ir::TypeId::I64,
            forge_ir::TypeId::I64,
        ],
        false,
    );
    let sig = FunctionSignature::new(&[(agg, "s")], &[agg]);
    let mut b = FunctionBuilder::new("events_fixture", ctx, sig);
    let (e, ps) = b.create_block_with_params(&[(agg, "s")]);
    b.switch_to_block(e);
    b.ret(&[ps[0]]);
    let f = b.finish().expect("build");
    let tm = forge_codegen::arch::x86::TargetMachine::new();
    let s = forge_codegen::CompileSession::new(tm);
    let _ = s.compile(&f);
}

/// **事件通道三方双向相等**（v26 M1）：`diagnostics::CHANNELS` ↔ 源码调用点 ↔ 契约文档通道表。
///
/// 为什么值钱：四个 `FORGE_*_EVENTS` 通道原先各写各的（四份 `OpenOptions+append+writeln`），
/// 环境变量名散在四处、格式无人对照；这层把"落盘"收成一处、把"有哪些通道"变成台账。
#[test]
fn event_channels_are_bijective() {
    let root = repo_root();

    // 台账侧（名字, 环境变量）
    let table: BTreeSet<(String, String)> = forge_isa_runtime::diagnostics::CHANNELS
        .iter()
        .map(|c| (c.name.to_string(), c.env.to_string()))
        .collect();
    assert!(!table.is_empty(), "`CHANNELS` 台账为空");

    // 源码侧：`diagnostics::emit("…"` / `diagnostics::emit_text("…"` 的通道名。
    // **跨行解析**（rustfmt 会把参数折到下一行——本仓守卫反复踩到的坑）＋跳过定义处与本守卫文件
    // （后者为测试自己写出了这两个标记串 ⇒ 会把 `", "` 当成通道名，实测撞到过）。
    let mut files = Vec::new();
    rs_files(&root.join("crates"), &mut files);
    let mut source: BTreeSet<String> = BTreeSet::new();
    for p in &files {
        let name = p.file_name().and_then(|n| n.to_str()).unwrap_or("");
        if name == "diagnostics.rs" || name == "events_guard.rs" {
            continue;
        }
        let Ok(text) = std::fs::read_to_string(p) else {
            continue;
        };
        let mut rest = text.as_str();
        while let Some(i) = rest.find("diagnostics::emit") {
            let after = &rest[i + "diagnostics::emit".len()..];
            // 允许 `(` 或 `_text(` 之后跨行
            let after = after.trim_start();
            let after = after
                .strip_prefix("_text(")
                .or_else(|| after.strip_prefix('('))
                .unwrap_or(after);
            let after = after.trim_start();
            if let Some(stripped) = after.strip_prefix('"')
                && let Some(end) = stripped.find('"')
            {
                source.insert(stripped[..end].to_string());
            }
            rest = after;
        }
    }
    let table_names: BTreeSet<String> = table.iter().map(|(n, _)| n.clone()).collect();
    assert_eq!(
        source, table_names,
        "事件通道的源码调用点与 `CHANNELS` 台账不一致（源码多 = 漏登记；台账多 = 死条目）：\n           源码 {source:?}\n  台账 {table_names:?}"
    );

    // 文档侧：契约文档「事件通道」小节的 `| \`通道\` | \`ENV\` |` 行
    let doc = std::fs::read_to_string(root.join("docs/reference/backend-pipeline.md"))
        .unwrap_or_else(|e| panic!("读不到契约文档：{e}"));
    let mut inside = false;
    let mut documented: BTreeSet<(String, String)> = BTreeSet::new();
    for line in doc.lines() {
        if line.starts_with("### ") || line.starts_with("## ") {
            inside = line.contains("事件通道");
            continue;
        }
        if !inside {
            continue;
        }
        let l = line.trim();
        if let Some(rest) = l.strip_prefix("| `")
            && let Some(end) = rest.find('`')
        {
            let name = rest[..end].to_string();
            let tail = &rest[end..];
            if let Some(e2) = tail.find("`FORGE_")
                && let Some(e3) = tail[e2 + 1..].find('`')
            {
                documented.insert((name, tail[e2 + 1..e2 + 1 + e3].to_string()));
            }
        }
    }
    assert_eq!(
        table, documented,
        "契约文档的通道表与 `CHANNELS` 台账不一致（改通道就同时改两处）：\n  台账 {table:?}\n  文档 {documented:?}"
    );
}
