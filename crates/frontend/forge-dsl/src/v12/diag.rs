//! 诊断定位——把错误消息锚定到 ISA TOML 的行列。
//!
//! 生成器是 proc 宏：所有错误都只能报在 `isa_from_file!("…")` 那一行，
//! 编辑器无法跳转。本模块把错误锚到 TOML 自身的位置，最终消息形如
//! `D:/…/isa/x86_v12.toml:1234:5: [[instructions.ADD_RM_R]]: …`
//! （终端与 IDE 都能识别 `路径:行:列` 并点击跳转）。
//!
//! 两条来源：
//! - **解析错误**：`toml::de::Error::span()` 给出字节区间，直接换算行列。
//! - **语义/生成错误**：消息前缀本身是机器可读的声明路径
//!   （`[[instructions.NAME]]` / `[[forms.NAME]]` / `[[lowering.OP]]` /
//!   `[[operand_slots]] #i ('NAME')` / `[reg.NAME]` / `[abi]` …）——从前缀抽出
//!   声明名，回到源里找它的声明行。不做模糊猜测：抽不出名字就退化到文件首行，
//!   消息内容不变（只丢失跳转，不产生错误定位）。

/// 字节偏移 → 1-based (行, 列)。
pub(crate) fn line_col(source: &str, offset: usize) -> (usize, usize) {
    let off = offset.min(source.len());
    let before = &source[..off];
    let line = before.matches('\n').count() + 1;
    let col = before.rsplit('\n').next().map(|l| l.chars().count()).unwrap_or(0) + 1;
    (line, col)
}

/// 从错误消息前缀抽出声明名：`[[instructions.ADD_RM_R]]…` → `Some("ADD_RM_R")`；
/// `[[operand_slots]] #3 ('gprx')…` → `Some("gprx")`；`[reg.gpr8]…` → `Some("gpr8")`。
/// 无名前缀（`[abi]` / `[meta]` / `[[forms]]`）→ None。
fn declared_name(msg: &str) -> Option<&str> {
    // `[[operand_slots]] #i ('NAME')` 形态
    if let Some(rest) = msg.strip_prefix("[[operand_slots]]")
        && let Some(open) = rest.find("('")
        && let Some(close) = rest[open + 2..].find("')")
    {
        return Some(&rest[open + 2..open + 2 + close]);
    }
    // `[[kind.NAME]]` / `[kind.NAME]` 形态
    let body = msg
        .strip_prefix("[[")
        .or_else(|| msg.strip_prefix('['))?;
    let end = body.find(']')?;
    let head = &body[..end];
    let (_kind, name) = head.split_once('.')?;
    // `[conventions.bitfields]` 这类"节名.子节名"不是声明名——排除已知节
    if matches!(
        _kind,
        "conventions" | "meta" | "emit" | "abi" | "spill" | "preset"
    ) && !matches!(_kind, "spill")
    {
        return None;
    }
    (!name.is_empty()).then_some(name)
}

/// 在源里定位声明：优先 `name = "<name>"`（forms/instructions/slots/families 的
/// 统一声明形式），其次表头 `[kind.<name>]`，再其次 `op = "<name>"`（lowering）。
/// 返回 1-based (行, 列)；找不到 → None。
pub(crate) fn locate(source: &str, name: &str) -> Option<(usize, usize)> {
    let needles = [
        format!("name = \"{name}\""),
        format!(".{name}]"),
        format!("op = \"{name}\""),
    ];
    for n in &needles {
        if let Some(off) = source.find(n.as_str()) {
            return Some(line_col(source, off));
        }
    }
    None
}

/// 消息 → 位置：能锚定则返回声明行列，否则 (1, 1)。
pub(crate) fn anchor(source: &str, msg: &str) -> (usize, usize) {
    declared_name(msg)
        .and_then(|n| locate(source, n))
        .unwrap_or((1, 1))
}

#[cfg(test)]
mod tests {
    use super::*;

    const DOC: &str = "[meta]\nname = \"x\"\n\n[[forms]]\nname = \"RR\"\n\n[[instructions]]\nname = \"ADD\"\nform = \"RR\"\n";

    #[test]
    fn line_col_counts_from_one() {
        assert_eq!(line_col(DOC, 0), (1, 1));
        assert_eq!(line_col(DOC, 7), (2, 1)); // 第 2 行行首
        assert_eq!(line_col(DOC, 9), (2, 3));
        // 越界偏移钳到末尾而不 panic
        let _ = line_col(DOC, DOC.len() + 999);
    }

    #[test]
    fn declared_name_extracts_from_prefixes() {
        assert_eq!(
            declared_name("[[instructions.ADD_RM_R]]: bad"),
            Some("ADD_RM_R")
        );
        assert_eq!(declared_name("[[forms.MRR]].escape empty"), Some("MRR"));
        assert_eq!(declared_name("[[lowering.Iadd]]: x"), Some("Iadd"));
        assert_eq!(
            declared_name("[[operand_slots]] #3 ('gprx'): oops"),
            Some("gprx")
        );
        assert_eq!(declared_name("[reg.gpr8]: dup"), Some("gpr8"));
        // 无声明名的节 → None（退化到首行，不乱指）
        assert_eq!(declared_name("[abi].stack_align must be"), None);
        assert_eq!(declared_name("[[forms]]: duplicate"), None);
        assert_eq!(declared_name("[conventions.bitfields]: x"), None);
    }

    #[test]
    fn anchor_points_at_declaration() {
        // ADD 声明在第 8 行
        assert_eq!(anchor(DOC, "[[instructions.ADD]]: bad"), (8, 1));
        // RR 声明在第 5 行
        assert_eq!(anchor(DOC, "[[forms.RR]]: bad"), (5, 1));
        // 锚不到 → 首行
        assert_eq!(anchor(DOC, "[abi]: bad"), (1, 1));
        assert_eq!(anchor(DOC, "[[instructions.NOPE]]: bad"), (1, 1));
    }
}
