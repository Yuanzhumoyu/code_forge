//! `forge-isa new` 脚手架：① 资源与教程 §0 **同源**；② 产物直接可 `validate`；③ 不覆盖已有文件。

use std::process::Command;

/// 被测二进制（cargo 为 bin target 注入的路径）。
fn bin() -> &'static str {
    env!("CARGO_BIN_EXE_forge-isa")
}

/// 资源 = 教程 §0 的那段 TOML（教程在它前面多一行 `# toy16.toml` 标题）。
#[test]
fn scaffold_asset_matches_tutorial() {
    let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../..");
    let tut =
        std::fs::read_to_string(root.join("docs/guides/isa-dsl-tutorial.md")).expect("读教程");
    let tut = tut.replace("\r\n", "\n");
    let asset = include_str!("../assets/toy16.toml").replace("\r\n", "\n");
    assert!(
        tut.contains(asset.trim_end()),
        "assets/toy16.toml 必须与教程 §0 的那段谱同源（否则两处会各自漂移）"
    );
}

/// `new` 写出可校验的谱；第二次调用拒绝覆盖。
#[test]
fn new_writes_valid_spec_and_refuses_overwrite() {
    let dir = std::env::temp_dir().join(format!("forge_new_{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).expect("建临时目录");

    let out = Command::new(bin())
        .current_dir(&dir)
        .args(["new", "my_isa"])
        .output()
        .expect("跑 forge-isa new");
    assert!(
        out.status.success(),
        "new 应成功：{}",
        String::from_utf8_lossy(&out.stderr)
    );
    let spec = dir.join("my_isa.toml");
    assert!(spec.is_file(), "应写出 my_isa.toml");

    let v = Command::new(bin())
        .current_dir(&dir)
        .args(["validate", "my_isa.toml"])
        .output()
        .expect("跑 validate");
    assert!(
        v.status.success(),
        "生成的谱应直接通过 validate：{}",
        String::from_utf8_lossy(&v.stderr)
    );

    let again = Command::new(bin())
        .current_dir(&dir)
        .args(["new", "my_isa"])
        .output()
        .expect("再跑一次 new");
    assert!(!again.status.success(), "已存在时应拒绝覆盖");

    // 带 `.toml` 后缀也不该生成双重后缀。
    let with_ext = Command::new(bin())
        .current_dir(&dir)
        .args(["new", "other.toml"])
        .output()
        .expect("跑 new other.toml");
    assert!(with_ext.status.success());
    assert!(dir.join("other.toml").is_file());
    assert!(!dir.join("other.toml.toml").exists(), "后缀不该叠加");

    let _ = std::fs::remove_dir_all(&dir);
}
