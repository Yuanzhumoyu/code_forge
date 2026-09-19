//! 通用性守卫：**生成期代码不得出现"某个 ISA 的常量"**。
//!
//! 背景：ISA-DSL 的既有纪律是"ISA 形状全在数据里（`isa/*.toml`），生成器只处理数据"。
//! 但历史上多次回潮——`lowering.rs` 里硬编码 x86 的 `IntCC → setcc` 码表、`asm.rs` 里
//! 内建 x86 条件码缺省表（详见 `docs/plans/forge-dsl-v18-plan.md` §2.4/§2.8）。
//! 本守卫把这条纪律变成会失败的测试。
//!
//! # 判据（数据驱动，不维护手写名单）
//!
//! 从仓库根 `isa/*.toml`（发行后端）里**读出**该 ISA 的：
//!
//! 1. **指令名**（`[[instructions]].name` 与 `[[families.variants]].name`）；
//! 2. **物理寄存器名**（`[reg.*].names` 列表，以及生成式声明的 `prefix`）；
//!
//! 然后在 `src/**`（除 `tests.rs` 与 `#[cfg(test)]` 之后的部分）里查找这些名字的**字符串
//! 字面量**：出现即失败——说明生成器把某个 ISA 的常量写进了代码，而不是从模型取。
//!
//! 另有两条与"名字"无关的检查：
//!
//! 3. `IntCC` / `FloatCC` 不得出现在生成期代码里——IR 条件到**本 ISA 编码**的映射必须是
//!    `[conventions.cond]` 数据（当前 `lowering.rs` 是已知欠账，见 `ALLOWED`）。
//! 4. 白名单**防腐烂**：每条允许项必须被命中一次，否则报错要求删除条目
//!    （沿用 `tests/no_hardcoded_widths.rs` 的机制）。
//!
//! 扫描范围只含"名字字面量"与两个 IR 条件类型标识符：注释行与 `#[cfg(test)]` 之后的内容
//! 不扫描（测试夹具里出现 `"RAX"` 是测试数据，不是缺陷）。

/// 允许出现的行：(文件后缀, 该行 trim 后的内容, 理由)。
///
/// 空列表 = 生成期代码里不允许任何 ISA 常量。当前条目都是**已知欠账**，
/// 每条都注明由哪个切片删除（`docs/plans/forge-dsl-v18-plan.md` §7）——
/// S3 收尾时本列表必须回到空（防腐烂检查会强迫删除条目）。
#[allow(clippy::type_complexity)]
const ALLOWED: &[(&str, &str, &str)] = &[
    (
        "frame.rs",
        ".unwrap_or_else(|| format_ident!(\"R10\"));",
        "S0 基线欠账（S3 删）：[abi].scratch 缺失时回退 x86 的 R10 —— 应报 Unsupported，\
         不得回退别家寄存器",
    ),
    (
        "frame.rs",
        "let has_mov_inst = inst_exists(infos, \"MOV_RM8_R64\");",
        "S0 基线欠账（S3 删）：gpr_mov 角色缺失时回退 x86 指令名 —— 违反 S4「按角色查指令」",
    ),
    (
        "lowering.rs",
        "use crate::prelude::IntCC::*;",
        "S0 基线欠账（S3 删）：IR 条件 → 本 ISA 编码的映射必须走 [conventions.cond]",
    ),
    (
        "lowering.rs",
        "match crate::prelude::IntCC::from_code(__raw) {",
        "S0 基线欠账（S3 删）：同上（x86 setcc 码表硬编码在通用 lowering 生成器里）",
    ),
    (
        "lowering.rs",
        "let ret_reg = abi.call_ret_reg.clone().unwrap_or_else(|| \"X1\".to_string());",
        "S0 基线欠账（S3 删）：call_ret_reg 缺省 = riscv 的 X1，应 fail-closed 报错",
    ),
];

/// 与"名字"无关的禁止标识符（IR 条件 → ISA 编码的映射必须走数据）。
const FORBIDDEN_IDENTS: &[&str] = &["IntCC", "FloatCC"];

/// IR 类型常量的标识符（`TypeId::F16` 等）——与物理寄存器名**同名但不同命名空间**
/// （riscv 的 `F16` 是浮点寄存器 16，`model.rs` 的 `"F16"` 是 IR 类型常量名）。
const IR_TYPE_IDENTS: &[&str] = &[
    "VOID", "BOOL", "I8", "I16", "I32", "I64", "I128", "F16", "F32", "F64", "F128", "PTR", "V64",
    "V128", "V256",
];

/// 仓库根：`crates/frontend/forge-dsl` 上溯三级。
fn repo_root() -> std::path::PathBuf {
    std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../..")
}

/// 从 `names = [ ... ]` 形式的数组里抽出所有字符串字面量（支持跨行）。
fn quoted_in_arrays(src: &str) -> Vec<String> {
    let mut out = Vec::new();
    let bytes = src.as_bytes();
    let mut i = 0usize;
    while let Some(pos) = src[i..].find("names = [") {
        let start = i + pos + "names = [".len();
        let Some(end_rel) = src[start..].find(']') else {
            break;
        };
        let body = &src[start..start + end_rel];
        for cap in body.split('"').skip(1).step_by(2) {
            if !cap.is_empty() {
                out.push(cap.to_string());
            }
        }
        i = start + end_rel;
        if i >= bytes.len() {
            break;
        }
    }
    out
}

/// 从 `[reg.*]` 块里的 `prefix = "..."` 抽出生成式寄存器组的前缀。
///
/// 只认 `[reg.*]` 块内的 `prefix`：`[[forms]]`/enc 键也有 `prefix`（如 `prefix = "field"`），
/// 那是编码键取值，不是寄存器名——S0 的首版抽取器把它当寄存器名，产生了 4 处误报。
fn prefixes(src: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut in_reg = false;
    for line in src.lines() {
        let t = line.trim();
        if t.starts_with('[') {
            in_reg = t.starts_with("[reg.");
            continue;
        }
        if in_reg
            && let Some(rest) = t.strip_prefix("prefix = \"")
            && let Some(end) = rest.find('"')
        {
            out.push(rest[..end].to_string());
        }
    }
    out
}

/// 抽出某个 TOML 里声明的指令名（`[[instructions]]` / `[[families.variants]]` 块内）。
fn instruction_names(src: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut in_block = false;
    for line in src.lines() {
        let t = line.trim();
        if t.starts_with("[[instructions]]") || t.starts_with("[[families.variants]]") {
            in_block = true;
            continue;
        }
        if t.starts_with("[[") || (t.starts_with('[') && t.ends_with(']')) {
            in_block = false;
            continue;
        }
        if in_block
            && let Some(rest) = t.strip_prefix("name = \"")
            && let Some(end) = rest.find('"')
        {
            out.push(rest[..end].to_string());
            in_block = false;
        }
    }
    out
}

/// 收集全部 ISA 常量（指令名 + 寄存器名/前缀）。
fn isa_constants() -> (Vec<String>, Vec<String>, usize) {
    let dir = repo_root().join("isa");
    let mut files = 0usize;
    let mut insts = Vec::new();
    let mut regs = Vec::new();
    let entries = std::fs::read_dir(&dir).unwrap_or_else(|e| {
        panic!(
            "读不到 ISA 谱目录 {}：{e}——守卫必须能看到发行 ISA（本测试假定在仓库内运行）",
            dir.display()
        )
    });
    for e in entries.flatten() {
        let p = e.path();
        if p.extension().is_none_or(|x| x != "toml") {
            continue;
        }
        let Ok(text) = std::fs::read_to_string(&p) else {
            continue;
        };
        files += 1;
        insts.extend(instruction_names(&text));
        regs.extend(quoted_in_arrays(&text));
        regs.extend(prefixes(&text));
    }
    assert!(files > 0, "isa/ 下没有 TOML：守卫形同虚设");
    // IR 类型常量名与物理寄存器名撞名（`F16`/`F32`…）：剔除类型常量名，只留真寄存器名。
    regs.retain(|n| !IR_TYPE_IDENTS.contains(&n.as_str()));
    insts.sort();
    insts.dedup();
    regs.sort();
    regs.dedup();
    (insts, regs, files)
}

/// 递归收集 `src/**` 下需要扫描的 (文件后缀, 行号, trim 后内容)。
fn scan_sources(dir: &std::path::Path, out: &mut Vec<(String, usize, String)>) {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    for e in entries.flatten() {
        let p = e.path();
        if p.is_dir() {
            scan_sources(&p, out);
            continue;
        }
        if p.extension().is_none_or(|x| x != "rs") {
            continue;
        }
        // 纯测试模块整体豁免（`tests.rs` 或 `*_tests.rs`）：测试夹具里的 ISA 名字是
        // 测试数据，不是"生成器写死了 ISA 常量"。
        if p.file_name().is_some_and(|n| {
            let n = n.to_string_lossy();
            n == "tests.rs" || n.ends_with("_tests.rs")
        }) {
            continue;
        }
        let Ok(text) = std::fs::read_to_string(&p) else {
            continue;
        };
        // `#[cfg(test)]` 之后不扫描（文件尾部的单测模块）。
        let body = text.split("#[cfg(test)]").next().unwrap_or(&text);
        let fname = p.file_name().unwrap().to_string_lossy().to_string();
        for (i, line) in body.lines().enumerate() {
            let t = line.trim();
            if t.starts_with("//") {
                continue;
            }
            out.push((fname.clone(), i + 1, t.to_string()));
        }
    }
}

fn report(kind: &str, hits: &[String], hint: &str) -> String {
    format!(
        "{kind}（{n} 处）：\n{hits}\n\n修法：{hint}",
        n = hits.len(),
        hits = hits.join("\n")
    )
}

#[test]
fn no_isa_constants_in_generator() {
    let (insts, regs, files) = isa_constants();
    let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("src");
    let mut lines = Vec::new();
    scan_sources(&root, &mut lines);

    let mut violations: Vec<String> = Vec::new();
    let mut used_allow: Vec<usize> = Vec::new();
    for (file, lineno, text) in &lines {
        let allowed = ALLOWED
            .iter()
            .position(|(f, t, _)| file.ends_with(f) && text == t);
        let is_isa_literal = insts
            .iter()
            .chain(regs.iter())
            .any(|n| text.contains(&format!("\"{n}\"")));
        let is_forbidden_ident = FORBIDDEN_IDENTS.iter().any(|id| {
            text.split(|c: char| !c.is_alphanumeric() && c != '_')
                .any(|w| w == *id)
        });
        if is_isa_literal || is_forbidden_ident {
            match allowed {
                Some(i) => used_allow.push(i),
                None => violations.push(format!("{file}:{lineno}: {text}")),
            }
        }
    }

    assert!(
        violations.is_empty(),
        "{}",
        report(
            &format!(
                "生成期代码出现 ISA 常量（已扫描 {files} 份发行 ISA 谱：{} 个指令名、{} 个寄存器名）",
                insts.len(),
                regs.len()
            ),
            &violations,
            "改为从模型取数据（指令/寄存器名一律来自 isa/*.toml；条件码走 [conventions.cond]），\
             或把该行加入 ALLOWED 并写明理由与删除它的切片"
        )
    );
    for (i, (f, t, why)) in ALLOWED.iter().enumerate() {
        assert!(
            used_allow.contains(&i),
            "白名单条目已失效（未被命中），请删除：{f} :: `{t}`（理由曾是：{why}）"
        );
    }
}

/// 守卫自身的健全性：抽取器必须真的从发行 ISA 谱里读到东西（否则守卫会静默永远通过）。
#[test]
fn guard_extractor_sees_the_shipped_isas() {
    let (insts, regs, files) = isa_constants();
    assert!(files >= 3, "发行 ISA 谱至少 3 份，实际 {files}");
    for expect in ["ADD_RM_R", "ADDIMMX", "FADD_S"] {
        assert!(
            insts.iter().any(|n| n == expect),
            "指令名抽取器漏了 {expect}（共 {} 个）",
            insts.len()
        );
    }
    for expect in ["RAX", "RBP", "X0", "X1"] {
        assert!(
            regs.iter().any(|n| n == expect),
            "寄存器名抽取器漏了 {expect}（共 {} 个）",
            regs.len()
        );
    }
}
