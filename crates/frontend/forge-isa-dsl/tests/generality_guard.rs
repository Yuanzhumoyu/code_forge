//! 通用性守卫：**生成期代码不得出现"某个 ISA 的常量"**。
//!
//! 背景：ISA-DSL 的既有纪律是"ISA 形状全在数据里（`isa/*.toml`），生成器只处理数据"。
//! 但历史上多次回潮——`lowering.rs` 里硬编码 x86 的 `IntCC → setcc` 码表、`asm.rs` 里
//! 内建 x86 条件码缺省表（详见 `docs/archive/forge-dsl-v18-plan.md` §2.4/§2.8）。
//! 本守卫把这条纪律变成会失败的测试。
//!
//! # 判据（数据驱动，不维护手写名单）
//!
//! 从仓库根 `isa/*.toml`（发行后端）里**读出**该 ISA 的：
//!
//! 1. **指令名**（`[inst.<名>]` 表头与 `[family.<名>].rows[].inst`）；
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
/// **空列表 = 生成期代码里不允许任何 ISA 常量**（v18 S3 的目标状态，已达成）：
/// 指令名、寄存器名、条件码一律来自 `isa/*.toml`。防腐烂检查会强迫删除失效条目，
/// 所以这里保持为空并让新欠账显式加进来（带理由 + 删除它的切片）。
#[allow(clippy::type_complexity)]
const ALLOWED: &[(&str, &str, &str)] = &[];

/// **写死的「IR 条件 → 机器码」配对**才是缺陷（v21 W6+ 起的口径）。
///
/// 生成物的条件码字段类型**就是** IR 的条件枚举（`IntCC`/`FloatCC`/`FlagCC`——
/// IR 解释汇编层、赋予新语义）⇒ 生成期点名这些类型是**设计要求**；被禁的是把
/// **变体与机器码写死**（如 `IntCC::Eq => 4`）——映射必须来自条件表的 `ir = "…"`。
const COND_ENUM_TYPES: &[&str] = &["IntCC::", "FloatCC::", "FlagCC::"];

/// 行里有没有"枚举变体 → 字面机器码"这种写死的配对。
fn is_hardcoded_cond_pair(text: &str) -> bool {
    COND_ENUM_TYPES.iter().any(|p| text.contains(p))
        && text.split("=>").nth(1).is_some_and(|rhs| {
            rhs.trim_start()
                .chars()
                .next()
                .is_some_and(|c| c.is_ascii_digit() || c == '-')
        })
}

/// IR 类型常量的标识符（`TypeId::F16` 等）——与物理寄存器名**同名但不同命名空间**
/// （riscv 的 `F16` 是浮点寄存器 16，`model.rs` 的 `"F16"` 是 IR 类型常量名）。
const IR_TYPE_IDENTS: &[&str] = &[
    "VOID", "BOOL", "I8", "I16", "I32", "I64", "I128", "F16", "F32", "F64", "F128", "PTR", "V64",
    "V128", "V256",
];

/// 仓库根：`crates/frontend/forge-isa-dsl` 上溯三级。
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
/// 只认 `[reg.*]` 块内的 `prefix`：`[form.<名>]`/enc 键也有 `prefix`（如 `prefix = "field"`），
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

/// 抽出某个 TOML 里声明的指令名（v21 W10：**表头即名字** —— `[inst.<名>]`）。
///
/// 旧面是 `[inst.<名>]` 块内的 `name = "X"`；表头化之后没有那一行，按表头取。
/// `inst = "X"`（模板行 / `insts` 列表）仍由 [`inst_literals`] 单列收集。
fn instruction_names(src: &str) -> Vec<String> {
    let mut out = Vec::new();
    for line in src.lines() {
        let t = line.trim();
        if let Some(rest) = t.strip_prefix("[inst.")
            && let Some(end) = rest.find(']')
        {
            let name = &rest[..end];
            if !name.is_empty() {
                out.push(name.to_string());
            }
        }
    }
    out
}

/// 一行里所有 `inst = "X"` 的取值（`insts`/`min_inst` 之类不会被误取：
/// `inst` 前必须是行首或非标识符字符）。
fn inst_literals(line: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut i = 0usize;
    while let Some(pos) = line[i..].find("inst = \"") {
        let abs = i + pos;
        let prev_ok = !line[..abs]
            .chars()
            .next_back()
            .is_some_and(|c| c.is_alphanumeric() || c == '_');
        let start = abs + "inst = \"".len();
        let Some(end) = line[start..].find('"') else {
            break;
        };
        if prev_ok {
            out.push(line[start..start + end].to_string());
        }
        i = start + end + 1;
    }
    out
}

/// 抽出 `[family.<名>]` 展开出的**实例名**（v18 S2c：`rows` 里的 `inst`）。
///
/// 两种等价书写都认：`rows = [ { inst = "X", … }, … ]`（可跨行）与
/// `[family.<名>].rows` + `inst = "X"`。守卫只需要"知道这些名字存在"。
fn template_instance_names(src: &str) -> Vec<String> {
    let mut out = Vec::new();
    for line in src.lines() {
        out.extend(inst_literals(line));
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
        insts.extend(template_instance_names(&text));
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
        let is_forbidden_ident = is_hardcoded_cond_pair(text);
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

/// **v21 W6/W11："按形状猜 ISA"的探测不得回潮。**
///
/// 这批标识符曾在生成器里用"**字段个数**"猜终结指令（分支/跳转/返回）的形态：它们自
/// v21 W6 起**全部没有消费者**，已在 W6 清理片里删除（见执行清单 §8 的复核记录）。
/// 它们回来就意味着编译器又在按形状猜 ISA——而形态只能由**谱数据**决定：
/// `[[lower.Br]]`/`[[lower.Jmp]]`/`[[lower.Ret]]` 的终结序列 + 指令自身的槽结构。
///
/// 注释行不扫描（`scan_sources` 跳过 `//` 开头的行），所以本条注释里点名它们是安全的。
const FORBIDDEN_SHAPE_PROBES: &[&str] = &[
    "_has_jal",
    "has_jal",
    "_jal_dest",
    "_jal_target",
    "_b_src",
    "_b_src2",
    "_b_target",
];

#[test]
fn no_isa_shape_guessing_probes_in_generator() {
    let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("src");
    let mut lines = Vec::new();
    scan_sources(&root, &mut lines);
    let mut hits: Vec<String> = Vec::new();
    for (file, lineno, text) in &lines {
        let is_probe = FORBIDDEN_SHAPE_PROBES.iter().any(|p| {
            text.split(|c: char| !c.is_alphanumeric() && c != '_')
                .any(|w| w == *p)
        });
        if is_probe {
            hits.push(format!("{file}:{lineno}: {text}"));
        }
    }
    assert!(
        hits.is_empty(),
        "{}",
        report(
            "生成器里又出现按形状猜 ISA 的探测",
            &hits,
            "终结指令的形态只能由**谱数据**决定（`[[lower.*]]` 的终结序列 + 指令槽结构）；\
             历史上这批探测用「字段个数」猜 jal/beq 形态，v21 W6 已删除"
        )
    );
}

/// **v21 W6：三份发行谱都自己声明终结序列**——编译器不再猜分支/跳转形态。
///
/// 这是 W11「新增 ISA 不必改编译器」的直接证据面：终结序列是**谱数据**（`[[lower.Br]]`/
/// `[[lower.Jmp]]`），三谱各写各的（x86 `test`+`jcc`+`jmp`、riscv `beq …, x0` + `jal x0, …`、
/// arm64 `cbz` + `b`）。
#[test]
fn shipped_isas_declare_their_terminator_sequences() {
    let root = repo_root();
    for isa in ["x86.toml", "riscv64.toml", "arm64.toml"] {
        let path = root.join("isa").join(isa);
        let text = std::fs::read_to_string(&path)
            .unwrap_or_else(|e| panic!("读 {} 失败：{e}", path.display()));
        for key in ["[[lower.Br]]", "[[lower.Jmp]]"] {
            assert!(
                text.contains(key),
                "isa/{isa} 缺 `{key}`：终结序列必须由**谱**声明（v21 W6），编译器不猜形态"
            );
        }
    }
}
