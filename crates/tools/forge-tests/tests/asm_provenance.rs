//! **语料出处守卫**：`asm/PROVENANCE.md`（由 `asm/fetch.mjs` 生成）与磁盘上的
//! vendored 语料**双向一致**——两个方向都红：
//!
//! - 磁盘上有、表里没有 ⇒ 有人塞了份来路不明的语料；
//! - 表里有、磁盘上没有 ⇒ 表是旧的（或文件被删）；
//! - 字节数/行数对不上 ⇒ 文件被改过（"逐字保留上游原文"这句话不成立了）。
//!
//! **只比字节数与行数**（不重算 sha256）：仓库没有任何哈希依赖，为一条守卫引入
//! 一个密码学依赖不值当；sha256 由 `asm/fetch.mjs` **拉取时**校验并写进表里，
//! 真要核对内容就重跑它（它会因摘要不符而报错）。
//!
//! 行数口径与生成端一致（`str::lines().count()`）：尾随换行不算多一行，末尾没换行的
//! 那半行算一行。

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

/// `PROVENANCE.md` 里的一行（路径 → 字节数/行数）。
fn table_rows(doc: &str) -> BTreeMap<String, (u64, usize)> {
    let mut out = BTreeMap::new();
    for line in doc.lines() {
        // 只认生成器写出来的数据行：首格是 `` `parse/…` `` 的反引号路径。
        if !line.starts_with("| `parse/") {
            continue;
        }
        let cells: Vec<&str> = line.trim_matches('|').split('|').map(str::trim).collect();
        assert!(
            cells.len() >= 6,
            "PROVENANCE.md 的数据行列数不对（表格被改坏了？）：{line}"
        );
        let path = cells[0].trim_matches('`').to_string();
        let bytes: u64 = cells[4]
            .parse()
            .unwrap_or_else(|e| panic!("{path}: 字节列不是数字（{}）：{e}", cells[4]));
        let lines: usize = cells[5]
            .parse()
            .unwrap_or_else(|e| panic!("{path}: 行数列不是数字（{}）：{e}", cells[5]));
        assert!(
            out.insert(path.clone(), (bytes, lines)).is_none(),
            "{path}: 在 PROVENANCE.md 里登记了两次"
        );
    }
    out
}

/// 递归收集语料文件（相对语料根的路径，`/` 分隔）。
fn collect(root: &Path, dir: &Path, out: &mut Vec<String>) -> std::io::Result<()> {
    for e in std::fs::read_dir(dir)? {
        let p = e?.path();
        if p.is_dir() {
            collect(root, &p, out)?;
        } else {
            out.push(
                p.strip_prefix(root)
                    .expect("语料文件在语料根下")
                    .to_string_lossy()
                    .replace('\\', "/"),
            );
        }
    }
    Ok(())
}

#[test]
fn provenance_matches_the_vendored_corpus() {
    let root: PathBuf = forge_tests::asm::corpus_root();
    let doc = std::fs::read_to_string(root.join("PROVENANCE.md"))
        .expect("asm/PROVENANCE.md 应当存在（由 asm/fetch.mjs 生成）");
    let rows = table_rows(&doc);
    assert!(
        !rows.is_empty(),
        "PROVENANCE.md 里一行数据都没解析出来——表格格式变了（生成器与守卫要一起改）"
    );

    let mut on_disk = Vec::new();
    collect(&root, &root.join("parse"), &mut on_disk).expect("asm/parse 可读");
    on_disk.sort();

    for rel in &on_disk {
        let (bytes, lines) = rows.get(rel).unwrap_or_else(|| {
            panic!("{rel}: 磁盘上有这份语料，PROVENANCE.md 里没有——跑 `node asm/fetch.mjs` 补登记")
        });
        let buf = std::fs::read(root.join(rel)).expect("语料文件可读");
        let text = String::from_utf8_lossy(&buf);
        let (want_bytes, want_lines) = (buf.len() as u64, text.lines().count());
        assert_eq!(
            (*bytes, *lines),
            (want_bytes, want_lines),
            "{rel}: 与 PROVENANCE.md 登记的不一致（表里 {bytes} 字节/{lines} 行，实测 \
             {want_bytes} 字节/{want_lines} 行）——语料必须逐字保留上游原文"
        );
    }

    for rel in rows.keys() {
        assert!(
            root.join(rel).is_file(),
            "{rel}: PROVENANCE.md 里登记了，磁盘上没有——表是旧的（跑 `node asm/fetch.mjs`）"
        );
    }
}
