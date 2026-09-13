//! 反回潮守卫：**宿主流水线不得再出现写死的寄存器类**。
//!
//! 背景（2026-09-12 去「寄存器宽度写死」）：宿主的寄存器类/栈槽单位曾写死为
//! x86 的 8 字节（`GPR64`/`FPR64`、栈槽 8 字节对齐、scratch 类 `GPR64`、类表
//! 9 项写死数组），1 字节寄存器 ISA 无法通过元数据表达自己。修完后用本守卫
//! 防止回潮：`src/**`（`#[cfg(test)]` 之前的部分）里出现写死类字面量即失败；
//! 允许的少数"历史缺省/占位"必须逐条白名单化（含理由），且每条必须被命中
//! 一次（防白名单腐烂）。
//!
//! 正路：`TargetRegInfo::{addr_class, value_gpr_class, value_fpr_class,
//! slot_bytes, vector_tiers, class_for_type, default_*_class}` 或
//! `LowerCtx` 上的同名字段（由 `CompileState::new` 注入）。

/// 禁止在宿主生产代码里出现的写死类。
///
/// **已知覆盖边界**：只匹配**类字面量**；裸数字宽度（栈槽步长 `8`、帧开销
/// `16`、sret 槽 `72` 等）不在范围内——见
/// `docs/reference/isa-dsl.md` 的「宽度元数据」节（那些由 `[stack].slot` /
/// `[stack].fp_save` 派生，其中 `[abi.stack_args]` 与 `wide_vec_*` /
/// `frame_rbp_addr` 路径的角色目前只有 x86 声明）。
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

/// 白名单：(相对 `src/` 的路径, 该行 trim 后的内容, 理由)。
const ALLOWED: &[(&str, &str, &str)] = &[
    (
        "lib.rs",
        "value_gpr_class: RegClass::GPR64,",
        "LowerCtx::new 的缺省 = 历史值（CompileState::new 立即用机器元数据覆盖；\
         直连 LowerCtx 的工具/测试保持历史语义）",
    ),
    (
        "lib.rs",
        "value_fpr_class: RegClass::FPR64,",
        "同上（FPR(8) = f64 值池，历史语义）",
    ),
    ("lib.rs", "addr_class: RegClass::GPR64,", "同上"),
    (
        "machine/encoder.rs",
        "let xreg = forge_ir::XReg::new(vreg, forge_ir::RegClass::GPR64);",
        "`encoded_size` 的 trait 默认实现只造 dummy 映射用于**估尺寸**，类不参与字节数",
    ),
    (
        "machine/encoder.rs",
        "forge_ir::PReg::new(vreg % 16, forge_ir::RegClass::GPR64),",
        "同上",
    ),
    (
        "machine/lowering.rs",
        "self.xregs.alloc_default(forge_ir::RegClass::GPR64);",
        "InstPacket::append 的占位分配：仅推进 XReg 编号计数器，类被立即丢弃",
    ),
    (
        "machine/reg_info.rs",
        "RegClass::GPR64",
        "TargetRegInfo::default_gpr_class 的 trait 缺省（无 ISA 元数据时的历史值；\
         DSL ISA 一律覆写）",
    ),
    (
        "machine/reg_info.rs",
        "RegClass::FPR64",
        "TargetRegInfo::default_fpr_class 的 trait 缺省（同上）",
    ),
    (
        "machine/reg_info.rs",
        "RegClass::FPR(8)",
        "TargetRegInfo::value_fpr_class 的 trait 缺省 = 历史 FPR64（f64 值池宽），\
         **不等于** default_fpr_class（后者是 ABI/SSE 占位基准，x86 = FPR(16)）",
    ),
    (
        "pipeline/alloc_config.rs",
        "RegClass::GPR64,",
        "RegAllocConfig::new：测试辅助构造（生产路径走 build_regalloc_config）",
    ),
    ("pipeline/alloc_config.rs", "RegClass::FPR64,", "同上"),
    (
        "pipeline/alloc_config.rs",
        "main_gpr_class: RegClass::GPR64,",
        "同上",
    ),
    (
        "pipeline/alloc_config.rs",
        "main_fpr_class: RegClass::FPR64,",
        "同上",
    ),
    (
        "pipeline/alloc_result.rs",
        "XReg::new(i, RegClass::GPR64),",
        "dummy_for_sizing：假分配记录（尺寸估算），不是真实分配",
    ),
    (
        "pipeline/alloc_result.rs",
        "PReg::new(i % 16, RegClass::GPR64),",
        "同上",
    ),
    (
        "pipeline/alloc_result.rs",
        ".unwrap_or(RegClass::GPR64)",
        "vreg_class 对**未分配** vreg 的历史缺省（调用方只在已分配时读）",
    ),
];

fn scan_dir(root: &std::path::Path, dir: &std::path::Path, out: &mut Vec<(String, usize, String)>) {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    for e in entries.flatten() {
        let p = e.path();
        if p.is_dir() {
            scan_dir(root, &p, out);
        } else if p.extension().is_some_and(|x| x == "rs") {
            let Ok(text) = std::fs::read_to_string(&p) else {
                continue;
            };
            let rel = p
                .strip_prefix(root)
                .unwrap_or(&p)
                .to_string_lossy()
                .replace('\\', "/");
            // `#[cfg(test)]` 之后不扫描（文件尾部的单测模块）。
            let body = text.split("#[cfg(test)]").next().unwrap_or(&text);
            for (i, line) in body.lines().enumerate() {
                let t = line.trim();
                if t.starts_with("//") {
                    continue;
                }
                if FORBIDDEN.iter().any(|f| t.contains(f)) {
                    out.push((rel.clone(), i + 1, t.to_string()));
                }
            }
        }
    }
}

#[test]
fn no_hardcoded_register_classes_in_host_pipeline() {
    let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("src");
    let mut hits = Vec::new();
    scan_dir(&root, &root, &mut hits);

    let mut used_allow: Vec<usize> = Vec::new();
    let mut violations: Vec<String> = Vec::new();
    for (file, line, text) in &hits {
        match ALLOWED.iter().position(|(f, t, _)| file == f && text == t) {
            Some(i) => used_allow.push(i),
            None => violations.push(format!("{file}:{line}: {text}")),
        }
    }
    assert!(
        violations.is_empty(),
        "宿主生产代码出现写死的寄存器类（请改为 TargetRegInfo::addr_class/\
         value_gpr_class/value_fpr_class/slot_bytes/vector_tiers/class_for_type \
         或 LowerCtx 同名字段）：\n{}",
        violations.join("\n")
    );
    for (i, (f, t, why)) in ALLOWED.iter().enumerate() {
        assert!(
            used_allow.contains(&i),
            "白名单条目已失效（未被命中），请删除：{f} :: `{t}`（理由曾是：{why}）"
        );
    }
}
