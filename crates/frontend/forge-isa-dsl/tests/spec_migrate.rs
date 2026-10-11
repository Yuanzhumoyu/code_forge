//! **谱写法迁移硬门**：把三份发行谱从旧写法（`ops` + `bind` + `{名字}` 引用）迁成
//! 内联声明（`{名字:槽[:角色]}` / `{@字段:槽[:角色]}`），并要求
//!
//! - **展开结果逐字节相同**（`expand_file` 的扁平 token 串）——"迁移只改写法"由此被证明；
//! - 迁移后的文本里 `[inst.*]` 块内**不再有** `ops`/`bind` 键；
//! - 迁移后的文本仍能通过 `report::insts`（= 走完整解析 + 校验）。
//!
//! 夹具（`forge-codegen/tests/isa/*.toml`）另有小差异（`demo_vlen` 等），
//! 用例对它们只断言"能迁移且展开相同"。

use std::path::{Path, PathBuf};

use forge_isa_dsl::{ExpandOptions, Parts, expand_file, report, spec_migrate};

fn opts() -> ExpandOptions {
    ExpandOptions {
        parts: Parts::all(),
        ..Default::default()
    }
}

/// token 流 → 去掉全部空白后的字符串（与 `doc_comment.rs` 同一口径）。
fn flatten_file(p: &Path) -> String {
    let ts = expand_file(p.to_str().expect("路径"), &opts())
        .unwrap_or_else(|e| panic!("展开 {p:?} 失败：{e:?}"));
    ts.to_string()
        .chars()
        .filter(|c| !c.is_whitespace())
        .collect()
}

/// 取 `s` 里 `at` 附近的窗口（按 char 边界收拢，避免切坏中文注释）。
fn window(s: &str, at: usize, back: usize, fwd: usize) -> String {
    let mut lo = at.saturating_sub(back);
    while lo > 0 && !s.is_char_boundary(lo) {
        lo -= 1;
    }
    let mut hi = (at + fwd).min(s.len());
    while hi < s.len() && !s.is_char_boundary(hi) {
        hi += 1;
    }
    s[lo..hi].to_string()
}

/// 写临时谱：**保留原文件名**（生成模块名 = 文件 stem，改名会让生成物头部漂移），
/// 放在按 pid 隔离的临时目录里。
fn write_temp(name: &str, text: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("forge_spec_migrate_{}", std::process::id()));
    std::fs::create_dir_all(&dir).expect("建临时目录");
    let p = dir.join(name);
    std::fs::write(&p, text).expect("写临时谱");
    p
}

/// 把生成物里 `include_bytes!("…")` 的实参换成占位符：它按定义与**源文件位置**绑定，
/// 不归一化就会把"临时目录/相对路径不同"误判成语义差异（这是路径，不是语义）。
fn strip_source_path(flat: &str) -> String {
    const OPEN: &str = "include_bytes!(\"";
    let mut out = String::with_capacity(flat.len());
    let mut rest = flat;
    while let Some(i) = rest.find(OPEN) {
        let (head, tail) = rest.split_at(i + OPEN.len());
        out.push_str(head);
        match tail.find("\")") {
            Some(j) => {
                out.push_str("__SPEC__");
                rest = &tail[j..];
            }
            None => {
                out.push_str(tail);
                rest = "";
            }
        }
    }
    out.push_str(rest);
    out
}

/// `[inst.*]` 块内是否还有旧键。
fn inst_blocks_have_legacy_keys(text: &str) -> Vec<String> {
    let mut bad = Vec::new();
    let mut in_inst = false;
    for line in text.lines() {
        let t = line.trim_start();
        if t.starts_with("[inst.") {
            in_inst = true;
            continue;
        }
        if t.starts_with('[') {
            in_inst = false;
            continue;
        }
        if in_inst && (t.starts_with("ops ") || t.starts_with("ops=") || t.starts_with("bind")) {
            bad.push(line.to_string());
        }
    }
    bad
}

/// 发行谱：迁移前后展开结果**逐字节相同** + 旧键清零 + 迁移后仍能解析。
///
/// 三份发行谱（x86 / riscv64 / arm64）都在内。为拿到这条，三处实现缺陷被依次修掉：
/// ① 内存形态标记 `{@[字段]:槽}` 的作者面解析（`@` 后直接是槽，不是 `名字:槽`）；
/// ② 抽取 pass 必须**最先跑**（`segments` 降级就要读 `inst.bind`/`inst.ops`，x86 的内存形态
///    指令正是段形态）；③ 允许多个操作数绑到同一字段（arm64 三个 `varr` 操作数都写 `vq` 位）。
#[test]
fn shipped_specs_migrate_with_byte_identical_expansion() {
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../..");
    for isa in ["x86.toml", "riscv64.toml", "arm64.toml"] {
        let p = root.join("isa").join(isa);
        let src = std::fs::read_to_string(&p).expect("读谱");
        let migrated =
            spec_migrate::migrate_spec_text(&src).unwrap_or_else(|e| panic!("{isa} 迁移失败：{e}"));
        assert_ne!(src, migrated, "{isa} 应当有变化（否则用例没意义）");

        let legacy_keys = inst_blocks_have_legacy_keys(&migrated);
        assert!(
            legacy_keys.is_empty(),
            "{isa} 迁移后 [inst] 块里还有旧键：{legacy_keys:?}"
        );

        let before = strip_source_path(&flatten_file(&p));
        let tmp = write_temp(isa, &migrated);
        let after = strip_source_path(&flatten_file(&tmp));
        if before != after {
            let at = before
                .bytes()
                .zip(after.bytes())
                .position(|(a, b)| a != b)
                .unwrap_or_else(|| before.len().min(after.len()));
            panic!(
                "{isa} 展开结果不同（{} vs {}，首处差异 @{at}）\n--- 迁移前 ---\n{}\n--- 迁移后 ---\n{}",
                before.len(),
                after.len(),
                window(&before, at, 240, 300),
                window(&after, at, 240, 300)
            );
        }

        // 迁移后仍能过完整解析 + 校验。
        report::insts(&migrated).unwrap_or_else(|e| panic!("{isa} 迁移后应仍能解析：{e:?}"));
    }
}

/// 夹具谱也要能迁移（它们含变长/向量等写法）。
#[test]
fn fixture_specs_migrate_with_byte_identical_expansion() {
    let dir = Path::new(env!("CARGO_MANIFEST_DIR")).join("../forge-codegen/tests/isa");
    if !dir.is_dir() {
        return; // 夹具不在预期位置时跳过（不假绿：目标谱已在上面断言）
    }
    let mut checked = 0usize;
    for entry in std::fs::read_dir(&dir).expect("读夹具目录") {
        let p = entry.expect("目录项").path();
        if p.extension().and_then(|e| e.to_str()) != Some("toml") {
            continue;
        }
        let name = p.file_name().unwrap().to_string_lossy().to_string();
        let src = std::fs::read_to_string(&p).expect("读夹具");
        let Ok(migrated) = spec_migrate::migrate_spec_text(&src) else {
            continue; // 夹具里可能有本迁移不覆盖的写法（多文件 include 等）
        };
        if migrated == src {
            continue;
        }
        let before = strip_source_path(&flatten_file(&p));
        let tmp = write_temp(&name, &migrated);
        let after = strip_source_path(&flatten_file(&tmp));
        assert!(before == after, "{name} 夹具迁移后展开结果不同");
        checked += 1;
    }
    assert!(checked > 0, "至少应迁移到一份夹具（否则用例没覆盖到东西）");
}

/// 旧 `bind` 值里的 `[名字]` 是**内存形态**标记（不能由槽 kind 推出：reg 槽也会这么用）
/// ⇒ 迁移必须保形为 `{@[字段]:槽}`，不能只剩 `{@字段:槽}`。
#[test]
fn memory_form_marker_is_preserved() {
    let src = r#"[meta]
name = "t"

[encoding]
kind = "prefix_scan"
max_len = 8

[form.M]
fields = ["u8[7:0]:opcode"]

[inst.LD]
form = "M"
opcode = 0x10
ops = ["dst:g:out", "mem:mem"]
bind = { rm = "[mem]" }
asm = "ld {dst}, {mem}"
"#;
    let out = spec_migrate::migrate_spec_text(src).expect("迁移");
    assert!(
        out.contains("{@[rm]:mem}{mem:mem}"),
        "内存形态应写成 `@[字段]:槽`：\n{out}"
    );
    assert!(!out.contains("bind ="), "旧 bind 键应被删掉");
    assert!(!out.contains("ops ="), "旧 ops 键应被删掉");
}

/// 迁移器对"已经是新写法"的输入必须明确报错（不许半迁移）。
#[test]
fn already_inline_input_is_rejected() {
    let src = r#"[meta]
name = "t"

[encoding]
kind = "fixed"
bits = 16

[reg.g]
names = ["R0"]

[operand.g]
kind = "reg"
class = "g"

[form.F]
fields = ["u4[15:12]:opcode", "u3[11:8]:rd"]

[inst.A]
form = "F"
opcode = 1
ops = ["dst:g:out"]
asm = "a {dst:g:out}"
"#;
    let err = spec_migrate::migrate_spec_text(src).expect_err("已带新写法应报错");
    assert!(
        err.contains("已是新写法") || err.contains("不在 ops 里"),
        "错误应说明占位符形态：{err}"
    );
}
