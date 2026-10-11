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

/// 把 `#[doc="…"]` 的**内容**换成占位符，返回（归一化文本, doc 内容列表）。
///
/// 迁移只改写法、不改代码与数据；唯一的例外是 `#[doc]` 里的**散文**——族的 doc 模板常写
/// "操作数 `{ops}`"，而 `ops` 键消失后展开器对 `{ops}` 的渲染变成另一种（`src,mem` 而不是
/// `["src:gprx","mem:mem"]`）。所以门禁按"**除 doc 内容外逐字节相同**"判，并把 doc 列表
/// 单独取出来核对条数（防漏比）。
fn strip_doc_bodies(flat: &str) -> (String, Vec<String>) {
    const OPEN: &str = "#[doc=\"";
    let mut out = String::with_capacity(flat.len());
    let mut docs = Vec::new();
    let mut rest = flat;
    while let Some(i) = rest.find(OPEN) {
        let (head, tail) = rest.split_at(i + OPEN.len());
        out.push_str(head);
        // doc 字符串里的引号是转义过的（`\"`），找到未转义的收尾引号。
        let mut end = None;
        let bytes = tail.as_bytes();
        let mut j = 0usize;
        while j < bytes.len() {
            if bytes[j] == b'\\' {
                j += 2;
                continue;
            }
            if bytes[j] == b'"' {
                end = Some(j);
                break;
            }
            j += 1;
        }
        match end {
            Some(j) => {
                docs.push(tail[..j].to_string());
                out.push_str("__DOC__");
                rest = &tail[j..];
            }
            None => {
                out.push_str(tail);
                rest = "";
            }
        }
    }
    out.push_str(rest);
    (out, docs)
}

/// **family 行内表**迁移的等价性证明（v22 续）：`[family.<名>]` 的 `body`/`rows` 里写的
/// `ops`/`bind` 就地写进该行的 `asm` 之后，展开结果必须**逐字节相同**。
///
/// 为拿到这条，修掉的三处（都是实测踩出来的）：
/// ① `referenced_keys` 收不了**嵌套占位符**——内联声明形如 `… {src:{slot}}`，旧实现取第一个
///    `}` ⇒ 漏掉 `slot` ⇒ 行键 `slot = "r8"` 被当成指令字段报 `unknown field`；
/// ② 族的行键插值放行**排在声明之前**——操作数名会与**编码键同名**（x86 的 `imm = 8` 与
///    操作数 `imm:imm8`）⇒ `{imm}` 被当参数放过、声明丢失 ⇒ 必须是**声明优先**；
/// ③ 行自带 `ops` 时 **body 的 `bind` 仍生效**（族的表按 key 合并）——x86 `CMP_MR` 自带
///    跨行 `ops` 而无 `bind`，丢了 body 的 `rm = "[mem]"` 就报"rm 指向 mem 槽——非内存形式"。
///
/// 口径：**除 `#[doc]` 内容外**逐字节相同（doc 是散文；族的 doc 模板里 `{ops}` 的渲染随
/// `ops` 键消失而变，条数由 `strip_doc_bodies` 单独核对）。
#[test]
fn family_tables_migrate_with_byte_identical_expansion() {
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../..");
    for isa in ["x86.toml", "riscv64.toml", "arm64.toml"] {
        let p = root.join("isa").join(isa);
        let src = std::fs::read_to_string(&p).expect("读谱");
        let migrated = spec_migrate::migrate_spec_text(&src)
            .unwrap_or_else(|e| panic!("{isa} family 迁移失败：{e}"));
        let (before, before_docs) = strip_doc_bodies(&strip_source_path(&flatten_file(&p)));
        let tmp = write_temp(isa, &migrated);
        let (after, after_docs) = strip_doc_bodies(&strip_source_path(&flatten_file(&tmp)));
        assert_eq!(
            before_docs.len(),
            after_docs.len(),
            "{isa} doc 条数变了（说明差别不止在散文里）"
        );
        if before != after {
            let at = before
                .bytes()
                .zip(after.bytes())
                .position(|(a, b)| a != b)
                .unwrap_or_else(|| before.len().min(after.len()));
            panic!(
                "{isa} family 迁移后展开不同（{} vs {}，首处差异 @{at}）\n\
                 --- 迁移前 ---\n{}\n--- 迁移后 ---\n{}",
                before.len(),
                after.len(),
                window(&before, at, 240, 300),
                window(&after, at, 240, 300)
            );
        }
    }
}

/// 发行谱**已经迁完**：迁移器不再改动它们，`[inst]` 块内也没有旧键。
///
/// **迁移当轮**的等价性证明是这条用例的前身——当时它断言"迁移前后 `expand_file` 展开逐字节
/// 相同"（三份 + 夹具全绿后才落盘）。迁完之后它守的是"不回头"：谱面不再出现 `ops`/`bind`，
/// 迁移器对已迁完的谱**幂等**，且展开照旧。
#[test]
fn shipped_specs_are_already_inline() {
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../..");
    for isa in ["x86.toml", "riscv64.toml", "arm64.toml"] {
        let p = root.join("isa").join(isa);
        let src = std::fs::read_to_string(&p).expect("读谱");
        // 迁移器**幂等**：`[inst.*]` 与 `[family.*]` 都已迁净 ⇒ 再跑一次应当一字不改。
        let migrated = spec_migrate::migrate_spec_text(&src)
            .unwrap_or_else(|e| panic!("{isa} 应已迁完（迁移器不该再报错）：{e}"));
        assert_eq!(
            src, migrated,
            "{isa} 又冒出旧写法（`ops`/`bind`）——迁移器是幂等的，相等才说明迁干净了"
        );
        let legacy = inst_blocks_have_legacy_keys(&src);
        assert!(
            legacy.is_empty(),
            "{isa} 的 [inst] 块里还有旧键：{legacy:?}"
        );
        report::insts(&src).unwrap_or_else(|e| panic!("{isa} 应仍能解析：{e:?}"));

        // 展开**不依赖源文件位置**：把同一份谱放到另一个目录（文件名不变）再展开，结果必须相同。
        // 生成物头部会写"模块名 = stem"与"内嵌源文件绝对路径"，这两处都归一化掉再比
        //（归一化掉的是路径，不是语义）。
        let before = strip_source_path(&flatten_file(&p));
        let tmp = write_temp(isa, &src);
        let after = strip_source_path(&flatten_file(&tmp));
        if before != after {
            let at = before
                .bytes()
                .zip(after.bytes())
                .position(|(a, b)| a != b)
                .unwrap_or_else(|| before.len().min(after.len()));
            panic!(
                "{isa} 的展开依赖了源文件位置（同名异目录结果不同，首处差异 @{at}）\n\
                 --- {p:?} ---\n{}\n--- {tmp:?} ---\n{}",
                window(&before, at, 200, 260),
                window(&after, at, 200, 260)
            );
        }
    }
}

/// 夹具谱同样**已经迁完**（迁移器幂等 + 旧键清零）。
///
/// 这条也守护"迁移器的等价性结论仍然成立"：夹具被大量守卫消费（往返/覆盖/黄金值），
/// 它们跟着发行谱一起迁，一旦语义漂移就会在那些守卫上先炸。
#[test]
fn fixture_specs_are_already_inline() {
    let dir = Path::new(env!("CARGO_MANIFEST_DIR")).join("../forge-codegen/tests/isa");
    if !dir.is_dir() {
        return; // 夹具不在预期位置时跳过（目标谱已在上面断言）
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
        assert_eq!(src, migrated, "{name} 又冒出旧写法（`ops`/`bind`）");
        let legacy = inst_blocks_have_legacy_keys(&src);
        assert!(
            legacy.is_empty(),
            "{name} 的 [inst] 块里还有旧键：{legacy:?}"
        );
        checked += 1;
    }
    assert!(checked > 0, "至少应检查到一份夹具（否则用例没覆盖到东西）");
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

/// 迁移器对**已经迁净**的输入是**幂等**的：内联写法（`{名字:槽}` / `{@字段:槽}`）原样放行，
/// 因此"再跑一次"必须一字不改（`[inst.*]` 与 `[family.*]` 都算）。
///
/// （旧版这里断言的是"报错"，与幂等性要求矛盾——已迁净的谱跑第二遍是常态。）
#[test]
fn already_inline_input_is_untouched() {
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
asm = "a {@reg:g:out}{dst:g:out}"
"#;
    let out = spec_migrate::migrate_spec_text(src).expect("已迁净的输入不该报错");
    assert_eq!(src, out, "迁移器必须是幂等的");
}
