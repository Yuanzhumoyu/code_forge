//! **G1 守卫**（ISA-DSL v19 V2）：依赖面 + 生成物可用性。
//!
//! 「生成物只依赖运行时 crate」如果只写在文档里，会随一次顺手改 `Cargo.toml`
//! 而失效。这里把它钉成两条测试：
//!
//! 1. **依赖面**：运行期依赖恰好 = `forge-isa-runtime`，build 依赖恰好 =
//!    `forge-isa-dsl`，全文件不出现 forge-codegen/forge-dsl/forge-ir。
//! 2. **生成物**：`$OUT_DIR` 里那份文件（build script 预生成）带着 lint 门闩与
//!    `__spec_tests`，路径一律指向 `forge_isa_runtime`、**不再**出现 `crate::`。
//!
//! 外加玩具 ISA 的编码/汇编往返（证明这份"只有运行时"的宿主真的能用）。

use std::collections::BTreeMap;
use std::path::PathBuf;

fn manifest_dir() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
}

/// 解析 `Cargo.toml` 的依赖节：`节名 → [依赖名]`（只认 `name = …` 这类行）。
fn dependency_sections(text: &str) -> BTreeMap<String, Vec<String>> {
    let mut out: BTreeMap<String, Vec<String>> = BTreeMap::new();
    let mut section = String::new();
    for line in text.lines() {
        let t = line.trim();
        if let Some(inner) = t.strip_prefix('[').and_then(|r| r.strip_suffix(']')) {
            section = inner.to_string();
            continue;
        }
        if t.is_empty() || t.starts_with('#') {
            continue;
        }
        if section.contains("dependencies")
            && let Some((key, _)) = t.split_once('=')
        {
            out.entry(section.clone())
                .or_default()
                .push(key.trim().to_string());
        }
    }
    out
}

/// 运行期依赖只有 `forge-isa-runtime`；build 依赖只有 `forge-isa-dsl`。
#[test]
fn dependency_surface_is_runtime_only() {
    let text = std::fs::read_to_string(manifest_dir().join("Cargo.toml")).expect("Cargo.toml");
    let deps = dependency_sections(&text);

    assert_eq!(
        deps.get("dependencies").map(Vec::as_slice),
        Some(["forge-isa-runtime".to_string()].as_slice()),
        "运行期依赖必须**只有** forge-isa-runtime（生成物只指它）"
    );
    assert_eq!(
        deps.get("build-dependencies").map(Vec::as_slice),
        Some(["forge-isa-dsl".to_string()].as_slice()),
        "build 依赖必须**只有** forge-isa-dsl（生成器本体）"
    );
    assert!(
        !deps.contains_key("dev-dependencies"),
        "本 crate 不允许有 dev-dependencies（测试不得借道别的 crate）"
    );

    // 注释里可以解释"为什么不要 forge-codegen"——只扫代码行（与上面的节解析同一口径）。
    let code_only: String = text
        .lines()
        .filter(|l| !l.trim_start().starts_with('#'))
        .collect::<Vec<_>>()
        .join("\n");
    for forbidden in ["forge-codegen", "forge-dsl", "forge-ir"] {
        assert!(
            !code_only.contains(forbidden),
            "外部宿主的依赖面里不得出现 {forbidden}：这是 G1（生成物只依赖运行时 crate）的硬约束"
        );
    }
}

/// 生成物（build script 预生成到 `$OUT_DIR`）：lint 门闩 + 生成期自测 + 绝对运行时路径。
#[test]
fn generated_file_targets_the_runtime_crate_only() {
    let path = PathBuf::from(env!("OUT_DIR")).join(env!("ISA_HOST_DEMO_GEN"));
    let text = std::fs::read_to_string(&path).unwrap_or_else(|e| {
        panic!(
            "生成物必须存在（build script 预生成）{}：{e}",
            path.display()
        )
    });

    // 生成物是**紧凑 token 文本**（`TokenStream::to_string()`），断言一律压平空白后做；
    // 先剥掉 `#[doc = r"…"]` 的**注释文本**——那是散文，允许提历史名字
    // （实测例：内部短名 `__RC` 的说明里写着 `forge_ir::RegClass`）。
    let flat: String = strip_doc_attrs(&text).split_whitespace().collect();

    assert!(
        text.starts_with("// 由 forge-dsl"),
        "生成物必须是 build script 落盘的那份（带头部注释）"
    );
    assert!(
        flat.contains("#[allow(warnings,clippy::all)]"),
        "生成物必须带 lint 门闩（改 include! 后它不再享受宏的 lint 豁免）"
    );
    assert!(
        flat.contains("mod__spec_tests"),
        "parts 不含 tm 时也应有生成期自测（v19 V2 放宽）"
    );
    assert!(
        flat.contains("forge_isa_runtime::"),
        "生成物必须指向 forge_isa_runtime"
    );
    assert!(
        !flat.contains("crate::"),
        "生成物不得再出现 crate:: 路径（v19 V1b 起一律固定根改写）"
    );
    for forbidden in ["forge_codegen", "forge_dsl::", "forge_ir::"] {
        assert!(
            !flat.contains(forbidden),
            "生成物不得依赖 {forbidden}（G1）"
        );
    }
}

/// 去掉 `#[doc = r"…"]` 的正文（只留标记本身之外的代码）。
///
/// 生成物把每条指令/位域的**文档注释**也带出来，它们是散文（会提到 `forge_ir::` 这类
/// 历史名字）；路径级断言只对代码成立，因此先剥注释体再断言。
fn strip_doc_attrs(text: &str) -> String {
    const OPEN: &str = "# [doc = r\"";
    let mut out = String::new();
    let mut rest = text;
    while let Some(i) = rest.find(OPEN) {
        out.push_str(&rest[..i]);
        out.push_str("#[doc]");
        let after = &rest[i + OPEN.len()..];
        match after.find("\"]") {
            Some(j) => rest = &after[j + 2..],
            None => return out,
        }
    }
    out.push_str(rest);
    out
}

/// 玩具 ISA 的字节布局：定宽 16 位、小端，位域 `op{12,4} rd{8,3} rs1{5,3} imm{0,5}`。
fn word(op: u16, rd: u16, rs1: u16, imm: u16) -> Vec<u8> {
    (op << 12 | rd << 8 | rs1 << 5 | imm).to_le_bytes().to_vec()
}

fn encode_asm(text: &str) -> Vec<u8> {
    let inst = isa_host_demo::assemble(text).unwrap_or_else(|e| panic!("assemble `{text}`: {e}"));
    isa_host_demo::encode(&inst).unwrap_or_else(|e| panic!("encode `{text}`: {e}"))
}

/// 编码：`RR`（op rd rs1）与 `RI`（op rd imm）两种形式各一条，外加负立即数。
#[test]
fn encodes_the_two_forms_from_the_spec() {
    assert_eq!(
        encode_asm("add R1, R2"),
        word(1, 1, 2, 0),
        "ADD = op1, rd, rs1"
    );
    assert_eq!(encode_asm("sub R3, R0"), word(2, 3, 0, 0), "SUB = op2");
    assert_eq!(
        encode_asm("movi R2, 5"),
        word(3, 2, 0, 5),
        "MOVI = op3, 5 位无符号视图"
    );
    assert_eq!(
        encode_asm("movi R0, -16"),
        word(3, 0, 0, 16),
        "imm5 是有符号的：-16 → 位模式 0b10000"
    );
    assert_eq!(
        encode_asm("brz R1, -1"),
        word(4, 1, 0, 31),
        "BRZ = op4，target 进 imm"
    );
}

/// `decode∘encode` 与文本幂等：字节 → 指令 → 字节必须逐字节稳定。
#[test]
fn decode_and_text_round_trip_is_stable() {
    for text in [
        "add R1, R2",
        "sub R3, R0",
        "movi R2, 5",
        "movi R0, -16",
        "movi R3, 15",
        "brz R1, -1",
    ] {
        let inst =
            isa_host_demo::assemble(text).unwrap_or_else(|e| panic!("assemble `{text}`: {e}"));
        let bytes = isa_host_demo::encode(&inst).expect("encode");
        // 生成物的解码入口是 `decode(&[u8]) -> Option<(Inst, 用掉的字节数)>`。
        let (back, used) =
            isa_host_demo::decode(&bytes).unwrap_or_else(|| panic!("decode {bytes:?}（{text}）"));
        assert_eq!(used, bytes.len(), "定宽 16 位必须正好吃掉 2 字节（{text}）");
        assert_eq!(back, inst, "decode∘encode 必须回到同一条指令（{text}）");
        assert_eq!(
            isa_host_demo::encode(&back).expect("encode"),
            bytes,
            "字节必须稳定（{text}）"
        );
        let rendered = isa_host_demo::disassemble(&inst);
        assert_eq!(
            isa_host_demo::assemble(&rendered).expect("assemble(disassemble)"),
            inst,
            "disassemble → assemble 必须幂等（{rendered}）"
        );
    }
}

/// 5 位有符号立即数的边界：`[-16, 15]` 可编码，越界必须报错（不是静默截断）。
#[test]
fn imm5_out_of_range_is_rejected() {
    for ok in ["movi R0, -16", "movi R0, 15", "movi R1, 0"] {
        let _ = encode_asm(ok);
    }
    for bad in ["movi R0, 16", "movi R0, -17", "movi R0, 1000"] {
        let rejected = match isa_host_demo::assemble(bad) {
            Ok(inst) => isa_host_demo::encode(&inst).is_err(),
            Err(_) => true,
        };
        assert!(rejected, "`{bad}` 超出 imm5 范围，必须在汇编或编码阶段报错");
    }
}
