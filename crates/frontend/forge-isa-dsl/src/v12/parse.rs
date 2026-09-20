//! 严格 TOML 解析：TOML 文本 → v12 模型。
//!
//! 委托 `toml` crate 做标准 TOML 解析；`deny_unknown_fields` 保证未知键/表
//! 一律报错（v11 文件无法通过本解析器——这是"不兼容"的直接体现）。
//! 错误带 TOML 行列（`toml::de::Error::span()` → `diag::line_col`），
//! 由调用方拼上文件路径构成可点击的 `路径:行:列`。

use super::V12Error;
use super::diag;
use super::model::V12Model;

/// 解析 TOML 文本为 v12 模型（不做语义校验）。
///
/// 解析后立刻展开 `[[lowering]].vary` 行表：下游（validate/codegen）只看到
/// 具体规则，两者不会因为"谁展开、怎么展开"而分叉。
pub fn parse(source: &str) -> Result<V12Model, V12Error> {
    let mut model: V12Model = toml::from_str(source).map_err(|e| {
        let (line, col) = e
            .span()
            .map(|s| diag::line_col(source, s.start))
            .unwrap_or((1, 1));
        // toml 的 Display 自带多行 caret 片段，这里保留（信息量大），
        // 位置由前缀给出，两者不冲突。
        V12Error::Parse {
            line,
            col,
            msg: format!("TOML: {e}"),
        }
    })?;
    // `[[derive]]` 展开（v18 S3f）：先于 `vary`——`vary` 的"键是谓词属性还是纯替换
    // 变量"判定要看派生名。
    let anchor_err = |msg: String| {
        let idx = diag::DeclIndex::build(source);
        let a = idx.anchor(&msg);
        V12Error::Parse {
            line: a.line,
            col: a.col,
            msg,
        }
    };
    model.expand_derives().map_err(anchor_err)?;
    let mut expanded = Vec::with_capacity(model.lowering.len());
    for rule in &model.lowering {
        // 先展开 `op` 名单（一个规则服务多个同类 op）→ 再展开 `vary` 行表。
        // 两步都在解析期完成，下游（校验/生成/报告）只见"一个 op、一行取值"的规则。
        for one in rule.expand_ops().map_err(anchor_err)? {
            let rows = one
                .expand_vary(&model.pred_attr_names())
                .map_err(anchor_err)?;
            expanded.extend(rows);
        }
    }
    model.lowering = expanded;
    // `[[templates]]` 展开（v18 S2）：实例拼进指令表，`ref` 合成别名——下游只见普通指令。
    model.expand_templates().map_err(|msg| {
        let idx = diag::DeclIndex::build(source);
        let a = idx.anchor(&msg);
        V12Error::Parse {
            line: a.line,
            col: a.col,
            msg,
        }
    })?;
    Ok(model)
}
