//! 严格 TOML 解析：TOML 文本 → ISA-DSL 模型。
//!
//! 委托 `toml` crate 做标准 TOML 解析；`deny_unknown_fields` 保证未知键/表
//! 一律报错（v11 文件无法通过本解析器——这是"不兼容"的直接体现）。
//! 错误带 TOML 行列（`toml::de::Error::span()` → `diag::line_col`），
//! 由调用方拼上文件路径构成可点击的 `路径:行:列`。

use super::DslError;
use super::diag;
use super::model::IsaModel;

/// 解析 TOML 文本为 ISA-DSL 模型（不做语义校验）。
///
/// 解析后立刻展开 `[[lowering]].vary` 行表：下游（validate/codegen）只看到
/// 具体规则，两者不会因为"谁展开、怎么展开"而分叉。
pub fn parse(source: &str) -> Result<IsaModel, DslError> {
    let mut model: IsaModel = toml::from_str(source).map_err(|e| {
        let (line, col) = e
            .span()
            .map(|s| diag::line_col(source, s.start))
            .unwrap_or((1, 1));
        // toml 的 Display 自带多行 caret 片段，这里保留（信息量大），
        // 位置由前缀给出，两者不冲突。
        DslError::Parse {
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
        DslError::Parse {
            line: a.line,
            col: a.col,
            msg,
        }
    };
    model.expand_derives().map_err(anchor_err)?;
    // v21 W6 进行中：只放行已完成接线并验证等价的 `[lower.Jmp]`
    crate::dsl::model::check_term_lowering_supported(&model).map_err(anchor_err)?;
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
    // v21 W5：`[family.<名>]` 先降级成 `[[templates]]`（族名兼作 `group`），**再**展开模板。
    crate::dsl::stream_decl::lower_families(&mut model, source).map_err(anchor_err)?;
    // v21 W7：`[asm.pseudo.<名>]` 降级进内部 `pseudo`（**表头 = 名字**；与 `[[pseudo]]`
    // 撞名报错）——与 `[family.<名>]` → `templates` 同一手法：下游只见内部表示。
    if let Some(asm) = model.asm.take() {
        for (name, body) in asm.pseudo {
            if model.pseudo.iter().any(|p| p.name == name) {
                return Err(anchor_err(format!(
                    "[asm.pseudo.{name}]: 与已有的 `[[pseudo]]` 声明同名（v21 W7：名字必须唯一）"
                )));
            }
            model.pseudo.push(crate::dsl::model::PseudoDef {
                name,
                asm: body.asm,
                emit: body.emit,
                only_variants: body.only_variants,
            });
        }
        // `[asm.immfn.<名>]` 降级进内部 `conventions.imm_fn`（表头 = 修饰名）。
        for (name, body) in asm.immfn {
            let defs = model.conventions.imm_fn.get_or_insert_with(Vec::new);
            if defs.iter().any(|d| d.name == name) {
                return Err(anchor_err(format!(
                    "[asm.immfn.{name}]: 修饰名与已有的 `[[conventions.imm_fn]]` 重复\
                     （v21 W7：名字必须唯一）"
                )));
            }
            defs.push(crate::dsl::model::ImmFnDef {
                name,
                text: None,
                texts: Some(body.texts),
                expr: body.expr,
            });
        }
        // `[asm.reloc.<名>]` 降级进内部 `reloc`（表头 = 重定位名）。
        for (name, body) in asm.reloc {
            if model.reloc.iter().any(|r| r.name == name) {
                return Err(anchor_err(format!(
                    "[asm.reloc.{name}]: 重定位名重复（v21 W8：名字必须唯一）"
                )));
            }
            model.reloc.push(crate::dsl::model::RelocDef {
                name,
                semantics: body.semantics,
                operand: body.operand,
                addend: body.addend,
            });
        }
    }
    model.expand_templates().map_err(|msg| {
        let idx = diag::DeclIndex::build(source);
        let a = idx.anchor(&msg);
        DslError::Parse {
            line: a.line,
            col: a.col,
            msg,
        }
    })?;
    // v21 W5：降级重载展开（`word_size` 分派）——指令表齐了之后、生成之前。
    crate::dsl::model::expand_lowering_overloads(&mut model).map_err(anchor_err)?;
    // 命名位集合表摊平进槽（`kind = "bits"`）：生成器与反汇编直接用槽上的条目，
    // 不再回头查 `[conventions.bitsets]`。
    model.resolve_bitset_tables().map_err(anchor_err)?;
    // 命名立即数表同理（`kind = "imm"` + `names`）。
    model
        .resolve_imm_name_tables()
        .map_err(anchor_err)?;
    Ok(model)
}
