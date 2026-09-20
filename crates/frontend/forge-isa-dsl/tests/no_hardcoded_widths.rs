//! 反回潮守卫：**生成期代码不得再出现写死的寄存器类**。
//!
//! 背景（2026-09-12 去「寄存器宽度写死」）：DSL 语法层支持任意寄存器宽度，但
//! 生成期曾把主 GPR 组锚定 `GPR(8).or(GPR(4))`、地址类/值池/槽宽/帧开销内置
//! 8 字节缺省——只声明 1 字节寄存器组的 ISA 会静默退化（空名字表、构造不存在
//! 的类）。修完后用本守卫防止回潮：`src/v12/**`（除单测 `tests.rs`）里出现
//! 写死类字面量即失败。
//!
//! 白名单机制：允许的行必须**逐条列出**（文件后缀 + 该行 trim 后内容），且每条
//! 白名单必须真的被命中一次（防白名单腐烂——改了代码却不删条目会立刻报错）。
//! 文件内 `#[cfg(test)]` 之后的内容不扫描（单测里的 `GPR(8)` 是测试数据）。

/// 禁止在生成期代码里出现的写死类（这些宽度必须来自 `[meta]`/`[reg.*]` 元数据
/// 或 `V12Model` 的派生方法）。
///
/// **已知覆盖边界**：本守卫只匹配**类字面量**；裸数字宽度（如栈槽步长 `8`、
/// 帧开销 `16`）不在匹配范围内——那些由 `[meta].slot_bytes` /
/// `fp_overhead_bytes` 派生并由各自测试守护（见
/// `docs/reference/isa-dsl.md` 的「宽度元数据」节）。
const FORBIDDEN: &[&str] = &[
    "RegClass::GPR(1)",
    "RegClass::GPR(2)",
    "RegClass::GPR(4)",
    "RegClass::GPR(8)",
    "RegClass::GPR(16)",
    "RegClass::GPR(32)",
    "RegClass::GPR(64)",
    "RegClass::GPR64",
    "RegClass::FPR(4)",
    "RegClass::FPR(8)",
    "RegClass::FPR(16)",
    "RegClass::FPR(32)",
    "RegClass::FPR(64)",
    "RegClass::FPR64",
    "RegClass::VEC(8)",
    "RegClass::VEC(16)",
    "RegClass::VEC(32)",
    "RegClass::VEC(64)",
    "RegClass::VEC(128)",
    "RegClass::VEC(256)",
    "RegClass::KReg(4)",
    "RegClass::KReg(8)",
    "RegClass::KReg(16)",
    "RegClass::KReg(32)",
    "RegClass::KReg(64)",
];

/// 白名单：(文件后缀, 该行 trim 后的内容, 理由)。
#[allow(clippy::type_complexity)]
const ALLOWED: &[(&str, &str, &str)] = &[
    (
        "model.rs",
        "return Ok(RegClass::GPR(4));",
        "`\"gpr\"` 缩写的文档化缺省（RegClass::from_str；显式宽度键才是正路）",
    ),
    (
        "model.rs",
        "return Ok(RegClass::FPR(4));",
        "`\"fpr\"` 缩写的文档化缺省",
    ),
    (
        "model.rs",
        "return Ok(RegClass::KReg(8));",
        "`\"kreg\"` 缩写的文档化缺省",
    ),
    (
        "model.rs",
        "Ok(Some(RegClass::FPR(8)).filter(|rc| self.reg.contains_key(rc)))",
        "浮点值池缺省 = FPR(8)（历史 FPR64 的 f64 值语义，**不得**按最宽 FPR 组推导——\
         x86 最宽是 ZMM、fpr8 是 MMX）",
    ),
    (
        "machine.rs",
        "let fpr_main = model.main_fpr_class()?.unwrap_or(RegClass::FPR(8));",
        "无 FPR 组的 ISA（arm64/demo）缺省浮点类 = FPR(8)（历史 `unwrap_or(8)` 语义：\
         它只作 `alloc_xreg` 的浮点目标类，需有浮点指令才可达）",
    ),
    (
        "machine.rs",
        "let value_fpr = model.value_fpr_class()?.unwrap_or(RegClass::FPR(8));",
        "同上：宿主浮点值池缺省（无 FPR 组时保持历史语义）",
    ),
    (
        "integration.rs",
        "let value_fpr_eff = model.value_fpr_class()?.unwrap_or(RegClass::FPR(8));",
        "类表登记时用**同一个**有效浮点值池类（与 machine.rs 的 __VALUE_FPR_CLASS \
         同规则）——riscv（只有 fpr4 组、值类为 FPR(8)）必须把 FPR(8) 也登记为可分配类",
    ),
];

fn scan_dir(dir: &std::path::Path, out: &mut Vec<(String, usize, String)>) {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    for e in entries.flatten() {
        let p = e.path();
        if p.is_dir() {
            scan_dir(&p, out);
        } else if p.extension().is_some_and(|x| x == "rs") {
            // 单测文件整体豁免（测试数据里的类字面量不是缺陷）。
            if p.file_name().is_some_and(|n| n == "tests.rs") {
                continue;
            }
            let Ok(text) = std::fs::read_to_string(&p) else {
                continue;
            };
            // `#[cfg(test)]` 之后不扫描（文件尾部的单测模块）。
            let body = text.split("#[cfg(test)]").next().unwrap_or(&text);
            for (i, line) in body.lines().enumerate() {
                let t = line.trim();
                if t.starts_with("//") {
                    continue;
                }
                if FORBIDDEN.iter().any(|f| t.contains(f)) {
                    out.push((
                        p.file_name().unwrap().to_string_lossy().to_string(),
                        i + 1,
                        t.to_string(),
                    ));
                }
            }
        }
    }
}

#[test]
fn no_hardcoded_register_classes_in_codegen() {
    let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("src/v12");
    let mut hits = Vec::new();
    scan_dir(&root, &mut hits);

    let mut used_allow: Vec<usize> = Vec::new();
    let mut violations: Vec<String> = Vec::new();
    for (file, line, text) in &hits {
        match ALLOWED
            .iter()
            .position(|(f, t, _)| file.ends_with(f) && text == t)
        {
            Some(i) => used_allow.push(i),
            None => violations.push(format!("{file}:{line}: {text}")),
        }
    }
    assert!(
        violations.is_empty(),
        "生成期代码出现写死的寄存器类（请改为元数据派生：V12Model::main_gpr_class/\
         addr_class/value_*_class/slot_bytes/fp_overhead_bytes，或用生成的 \
         __DEFAULT_GPR_CLASS / __ADDR_CLASS / __SLOT_BYTES 常量）：\n{}",
        violations.join("\n")
    );
    // 白名单腐烂检测：每条必须被命中一次（代码改好后忘了删条目 → 失败）。
    for (i, (f, t, why)) in ALLOWED.iter().enumerate() {
        assert!(
            used_allow.contains(&i),
            "白名单条目已失效（未被命中），请删除：{f} :: `{t}`（理由曾是：{why}）"
        );
    }
}
