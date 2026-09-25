//! v12 语义校验：引用完整 / 位域越界 / 重复声明 / 类型约束。
//!
//! 错误信息带 TOML 路径（如 `[[instructions.MOV_R_RM]]`），由 [`super::diag::DeclIndex`]
//! 换算成精确 `行:列`。**S1 起收集全部错误**：每个"按节"的校验器各报一条，
//! 逐条校验器（指令 / lowering / 模板）对每条声明各报一条，最后由 `Diags` 一次渲染。

use super::diag::{DeclIndex, Diags};
use super::model::*;
use super::shared::parse_u64;
use std::collections::{BTreeMap, BTreeSet};

/// 逐节收集：每节最多一条（节内逐条收集的见 `*_all`）。
fn collect(d: &mut Diags, idx: &DeclIndex, r: Result<(), String>) {
    if let Err(msg) = r {
        d.push_anchored(idx, &msg);
    }
}

/// 校验档位（v19 V6b）：默认档 = 历史行为；`--strict-overlap` 才加"部分重叠"体检。
///
/// 为什么默认关：这类规则**合法**（后一条在"前一条管不着"的取值上照常生效），
/// 真谱里"特化规则 + 泛化兜底"遍地都是——默认开会把几百条合法写法变成错误。
/// 先量化噪音（计划 §5 V6b 的判据），再决定是否以及如何在 CI 上开。
#[derive(Debug, Clone, Default)]
pub struct ValidateOpts {
    /// 报"同 op 两条规则取值域相交但互不包含"（`DSL-OVERLAP`）。
    pub strict_overlap: bool,
    /// **变体参数**（v19 V5，只读投影）：`{ 参数名 → 取值 }`。
    ///
    /// 空 = 不过滤、不替换（**默认行为逐字节不变**）。非空时：参数必须在
    /// `[meta].variants` 里声明且取值合法，逐指令 `only_variants` 不匹配的整条丢掉，
    /// `{参数名}` 在 `asm` 与 `[[lowering]].insts` 里替换成取值。
    pub params: BTreeMap<String, i64>,
}

/// 变体投影报告（v19 V5）：投影到底动了什么——给 CLI / 测试当证据，不参与语义。
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Projection {
    /// 生效的参数（空 = 没投影）。
    pub params: BTreeMap<String, i64>,
    /// 被投影掉的**指令名**（原声明序）。
    pub dropped_insts: Vec<String>,
    /// 逐节丢掉的条数：`(节名, 条数)`——`[[lowering]]` / `[emit.prologue]` /
    /// `[spill.GPR]` / `[[pseudo]]` / `[[pattern]]`。
    pub dropped_decls: Vec<(String, usize)>,
    /// 投影后的指令数 / lowering 规则数（默认档 = 全量）。
    pub inst_count: usize,
    pub lowering_count: usize,
}

/// 参数化变体投影（v19 V5，**只读投影**）：校验参数 → 过滤声明 → 连带丢 lowering → 文本替换。
///
/// 返回的错误消息以 TOML 路径开头（`[meta].variants: …`），由调用方 `push_anchored` 锚行。
/// **只做投影**：不注册后端、不生成变体专属的运行期表——"能跑"仍由 `tm` 部件与宿主负责。
///
/// 过滤对象 = **所有承载指令引用的声明**（六处，判定统一走 [`variants_keep`]）：
/// 指令/模板行、`[emit.prologue|epilogue]`、`[spill.*]`、`[[pseudo]]`、`[[pattern]]`；
/// 另外**连带**丢掉引用了被投影掉指令的 `[[lowering]]` 规则（这类依赖可推断，
/// 见 [`Projection::dropped_decls`]）。**其余引用**（例如 `[abi]` 里的寄存器名）
/// 若指向被投影掉的声明，校验器照旧 fail-closed 报错——不给静默通道。
pub fn apply_variants(
    m: &mut V12Model,
    params: &BTreeMap<String, i64>,
) -> Result<Projection, String> {
    if params.is_empty() {
        return Ok(Projection {
            inst_count: m.instructions.len(),
            lowering_count: m.lowering.len(),
            ..Projection::default()
        });
    }
    let declared = m.meta.variants.clone().unwrap_or_default();
    for (k, v) in params {
        let Some(domain) = declared.get(k) else {
            let known = if declared.is_empty() {
                "（谱里没有声明任何变体参数）".to_string()
            } else {
                declared.keys().cloned().collect::<Vec<_>>().join(", ")
            };
            return Err(format!(
                "[meta].variants: 传了参数 `{k} = {v}`，但谱里没有声明它——请加 `[meta].variants` 条目（已声明：{known}）"
            ));
        };
        if !domain.contains(v) {
            return Err(format!(
                "[meta].variants: 参数 `{k} = {v}` 不在声明域 {domain:?} 内"
            ));
        }
    }
    let mut dropped_decls: Vec<(String, usize)> = Vec::new();
    // ① 指令（含模板展开出的实例）：`only_variants` 里**给了值**的参数不匹配 ⇒ 整条丢掉；
    //    没给的参数不构成排除（调用方只关心自己传的那些）。
    let refs_before = declared_refs(m);
    let names_before: Vec<String> = m.instructions.iter().map(|i| i.name.clone()).collect();
    m.instructions
        .retain(|i| variants_keep(i.only_variants.as_ref(), params));
    let refs_after = declared_refs(m);
    let dropped_insts: Vec<String> = names_before
        .into_iter()
        .filter(|n| !refs_after.contains(n))
        .collect();
    // ② 其余承载指令引用的声明节：逐节 retain + 记账（节名 = TOML 里的写法）。
    let dropped_spills: Vec<String> = m
        .spill
        .iter()
        .filter(|(_, s)| !variants_keep(s.only_variants.as_ref(), params))
        .map(|(k, _)| k.clone())
        .collect();
    m.spill
        .retain(|_, s| variants_keep(s.only_variants.as_ref(), params));
    for key in dropped_spills {
        dropped_decls.push((format!("[spill.{key}]"), 1));
    }
    let n = retain_counted(&mut m.pseudo, |p| {
        variants_keep(p.only_variants.as_ref(), params)
    });
    if n > 0 {
        dropped_decls.push(("[[pseudo]]".into(), n));
    }
    let n = retain_counted(&mut m.pattern, |p| {
        variants_keep(p.only_variants.as_ref(), params)
    });
    if n > 0 {
        dropped_decls.push(("[[pattern]]".into(), n));
    }
    // ③ 连带丢 lowering：规则行首（含 `@引用名`）点了被投影掉的**引用名**。
    //    只认"投影前存在、投影后消失"的名字 ⇒ 本来就写错的助记符仍由校验器照常报错。
    let dangling: BTreeSet<String> = refs_before.difference(&refs_after).cloned().collect();
    if !dangling.is_empty() {
        let n = retain_counted(&mut m.lowering, |l| {
            !l.insts
                .iter()
                .any(|line| inst_head_ref(line).is_some_and(|h| dangling.contains(&h)))
        });
        if n > 0 {
            dropped_decls.push(("[[lowering]]".into(), n));
        }
    }
    // ④ 文本替换：`{参数名}` → 取值（模板里既有的 `{inst.lower}` 等不受影响）。
    for i in &mut m.instructions {
        i.asm = substitute_params(&i.asm, params);
    }
    for l in &mut m.lowering {
        for line in &mut l.insts {
            *line = substitute_params(line, params);
        }
    }
    Ok(Projection {
        params: params.clone(),
        dropped_insts,
        dropped_decls,
        inst_count: m.instructions.len(),
        lowering_count: m.lowering.len(),
    })
}

/// `retain` 并返回丢掉条数（各节投影共用的记账）。
fn retain_counted<T>(v: &mut Vec<T>, keep: impl Fn(&T) -> bool) -> usize {
    let before = v.len();
    v.retain(keep);
    before - v.len()
}

/// 模板行首的引用名（`ADD {out}, …` → `ADD`；`@frame_alloc` → `frame_alloc`）。
///
/// 与 `validate_inst_lines` 的取法一致（`{out} = INST …` 取 `=` 右侧、首个空白词），
/// 但把 `@` 前缀归一成裸引用名，好与 `declared_refs` 比对。lint 侧复用同一取法
/// （`pub(crate)`，v19 V4c：判"`ref` 有没有被模板行首引用"必须与校验用同一口径）。
pub(crate) fn inst_head_ref(line: &str) -> Option<String> {
    let trimmed = line.trim();
    if trimmed.is_empty() || trimmed.starts_with('#') {
        return None;
    }
    let rhs = trimmed.split_once('=').map_or(trimmed, |(_, r)| r.trim());
    let head = rhs.split_whitespace().next()?;
    Some(head.strip_prefix('@').unwrap_or(head).to_string())
}

/// 把字符串里 `{参数名}` 换成取值（只替换**名字命中参数**的占位符）。
fn substitute_params(s: &str, params: &BTreeMap<String, i64>) -> String {
    let mut out = s.to_string();
    for (k, v) in params {
        out = out.replace(&format!("{{{k}}}"), &v.to_string());
    }
    out
}

/// 全部校验（收集式，一次报全）。
pub fn validate_all(m: &V12Model, idx: &DeclIndex, d: &mut Diags, opts: &ValidateOpts) {
    // 组合键（`include` / `[[override]]`）由 `forge_isa_dsl::loader` 在**合并阶段**
    // 消费；走到这里说明文本没经过 loader（裸文本入口），此时它们不生效——必须
    // 明确报错，否则"写了 include 却没被包含"会静默变成一个缺指令的谱。
    if !m.include.is_empty() || !m.r#override.is_empty() {
        let which = if !m.include.is_empty() {
            "include"
        } else {
            "[[override]]"
        };
        d.push_anchored(
            idx,
            &format!(
                "{which} 需要经多文件加载器展开：请用 `isa_from_file!` / `forge-isa` CLI /                  `forge_isa_dsl::expand_file`（它们自动处理 include 与 [[override]]），                 或把结果合并成单文件"
            ),
        );
    }
    collect(d, idx, validate_meta(m));
    collect(d, idx, validate_variant_gates(m));
    collect(d, idx, validate_encoding(m));
    collect(d, idx, validate_regs(m));
    collect(d, idx, validate_widths(m));
    collect(d, idx, validate_conventions(m));
    collect(d, idx, validate_cond(m));
    collect(d, idx, validate_derives(m));
    collect(d, idx, validate_pseudos(m));
    collect(d, idx, validate_relocs(m));
    collect(d, idx, validate_operand_slots(m));
    collect(d, idx, validate_forms(m));
    validate_instructions_all(m, idx, d);
    collect(d, idx, validate_references(m));
    validate_lowering_all(m, idx, d);
    collect(d, idx, validate_patterns(m));
    collect(d, idx, validate_abi(m));
    validate_spill_all(m, idx, d);
    collect(d, idx, validate_vectors(m));
    if opts.strict_overlap {
        validate_lowering_overlap(m, idx, d);
    }
}

/// `--strict-overlap`：同 op 的两条规则**取值域相交、但互不包含**（部分重叠）时报出来。
///
/// 与"死规则"检测共用同一套判定域机器（`pred::domain_of`/`subsumes`/`overlaps`）：
/// 死规则 = 后者被完全吃掉（**始终**硬错误）；部分重叠 = 两者都会命中某些取值，
/// 裁决序里靠前者赢——**合法**，但下面是两类真事故的高发形状，所以给了这个可选档：
///
/// - 本意是"特化 + 泛化兜底"，却把特化规则的 `when` 写窄了（大部分取值掉进兜底）；
/// - 两条规则都想要同一批取值，靠 `priority` 硬分胜负（读者很难看出实际覆盖）。
///
/// 保守边界与死规则一致：任一侧含 `or`/`not`（`Opaque`）⇒ 不报。
fn validate_lowering_overlap(m: &V12Model, idx: &DeclIndex, d: &mut Diags) {
    for (op, rules) in m.lowering_by_op() {
        let mut domains: Vec<super::pred::RuleDomain> = Vec::with_capacity(rules.len());
        for r in &rules {
            // 谓词解析错误已由 `validate_lowering_all` 逐条报过，这里静默跳过。
            let pred = match &r.when {
                None => None,
                Some(v) => super::pred::parse(v).ok(),
            };
            domains.push(super::pred::domain_of(pred.as_ref()));
        }
        for (j, dj) in domains.iter().enumerate() {
            for (i, di) in domains[..j].iter().enumerate() {
                if super::pred::subsumes(di, dj) {
                    continue; // 死规则：已由 validate_lowering_order 报出走人
                }
                if super::pred::overlaps(di, dj) {
                    // 用**独立诊断码**：`push_anchored` 会按消息前缀判成 `DSL-LOWER`，
                    // 那样严格档的结论就和普通 lowering 诊断混在一起了。
                    let text = format!(
                        "[[lowering.{op}]]: 第 {} 条与第 {} 条规则的取值域相交（部分重叠）——\
                         裁决序里前者先命中；若本意是「特化 + 兜底」，确认后者的 when 没写窄，\
                         否则请用 priority 明确让谁赢",
                        i + 1,
                        j + 1
                    );
                    let a = idx.anchor(&text);
                    d.push("DSL-OVERLAP", a.line, a.col, text);
                }
            }
        }
    }
}

// ─────────────────────────── [[vectors]] ───────────────────────────

/// `[[vectors]]` 校验（v19 V3）：**形态**合法性与定宽字长。
///
/// 只判定"这条向量写得对不对"（结构）：
///
/// - 至少要给 `asm` 或 `bytes`；`asm` + `bytes` + `error` 三者同给是**歧义**
///   （既说应当成功又说应当失败）→ 报错；
/// - `bytes` 非空、每个元素 `0..=255`（TOML 整数，超界报出下标与原值）；
/// - 没有 `asm` 的向量是**解码负向**，`error` 只接受 `"DECODE"`；有 `asm` 时 `error`
///   是错误消息子串，不接受空串；
/// - `partial` 只与 `bytes` + `error = "DECODE"` 同用且 > 0；
/// - 定宽谱（`[encoding].kind = "fixed"` 且给了 `bits`）里 `bytes` 长度必须等于字长
///   ——变长/混合谱不猜长度；
/// - **同一条 `asm` 给出两种期望字节** → 报错（复制粘贴改一半的典型事故）；完全相同的
///   重复**允许**（`isa/x86_v12.toml` 有一条历史记录：两种编码合并后逐字节相同）。
///
/// **内容**（这条文本真能编出这些字节吗）不在这里判——那是生成用例的职责
/// （`__spec_tests` 里跑，或 `forge-isa test` 真编译一次）。这里挡的是"写坏的向量"，
/// 不是"写错的字节"：后者要执行才可能知道。
fn validate_vectors(m: &V12Model) -> Result<(), String> {
    if m.vectors.is_empty() {
        return Ok(());
    }
    let fixed_len: Option<usize> = match m.encoding.kind {
        EncodingKind::Fixed => m.encoding.bits.map(|b| (b as usize).div_ceil(8)),
        _ => None,
    };
    let mut seen: BTreeMap<String, String> = BTreeMap::new();
    for (i, v) in m.vectors.iter().enumerate() {
        let at = format!("[[vectors]] 第 {} 条", i + 1);
        let (has_asm, has_bytes) = (v.asm.is_some(), v.bytes.is_some());
        if !has_asm && !has_bytes {
            return Err(format!("{at}：至少要有 `asm` 或 `bytes`"));
        }
        if has_asm && has_bytes && v.error.is_some() {
            return Err(format!(
                "{at}：`asm` + `bytes` + `error` 三者同给有歧义——正向向量不写 `error`，负向向量不写 `bytes`（解码负向除外，它没有 `asm`）"
            ));
        }
        if let Some(bs) = &v.bytes {
            if bs.is_empty() {
                return Err(format!("{at}：`bytes` 不能是空数组"));
            }
            for (k, b) in bs.iter().enumerate() {
                if !(0..=255).contains(b) {
                    return Err(format!(
                        "{at}：`bytes[{k}]` = {b} 不是字节（允许 0..=255，可写十六进制如 0x48）"
                    ));
                }
            }
            // 定宽字长只对**必须成整条指令**的向量成立：解码负向常常**故意**给截断的输入
            //（`bytes = [0x13]` 测"短输入必须被拒"），那里不查长度。
            let expect_full = v.error.as_deref() != Some("DECODE");
            if expect_full
                && let Some(w) = fixed_len
                && bs.len() != w
            {
                return Err(format!(
                    "{at}：定宽谱 `[encoding].bits` = {} 位 ⇒ 一条指令 {w} 字节，但 `bytes` 给了 {} 字节",
                    m.encoding.bits.unwrap_or(0),
                    bs.len()
                ));
            }
        }
        match (&v.error, has_asm) {
            (Some(e), false) => {
                if e != "DECODE" {
                    return Err(format!(
                        "{at}：没有 `asm` 的向量是**解码负向**，`error` 只接受 \"DECODE\"（给的是 `{e}`）——要断言编码失败请写 `asm`"
                    ));
                }
            }
            (Some(e), true) => {
                if e.is_empty() {
                    return Err(format!(
                        "{at}：`error` 不能是空串（它是错误消息里的稳定子串）"
                    ));
                }
            }
            (None, _) => {}
        }
        if let Some(p) = v.partial {
            if !has_bytes || has_asm || v.error.as_deref() != Some("DECODE") {
                return Err(format!(
                    "{at}：`partial` 只与 `bytes` + `error = \"DECODE\"` 同用（它钉 `decode_partial` 吃掉的字节数）"
                ));
            }
            if p == 0 {
                return Err(format!("{at}：`partial` 必须 > 0（吃掉 0 字节不算解码）"));
            }
        }
        // **冲突**才算错：同一条汇编文本给了两种期望字节（复制粘贴改一半的典型事故）。
        // 完全相同的重复是**允许**的——`isa/x86_v12.toml` 里就有一条历史记录：
        // `mov RAX, RBX` 两种编码合并后逐字节相同，原表刻意留了两条并注明"真重复"。
        if let Some(asm) = &v.asm
            && let Some(bs) = &v.bytes
        {
            let hex = bs
                .iter()
                .map(|b| format!("{b:02X}"))
                .collect::<Vec<_>>()
                .join(",");
            if let Some(prev) = seen.insert(asm.clone(), hex.clone())
                && prev != hex
            {
                return Err(format!(
                    "{at}：`{asm}` 这段文本已经有了期望字节 [{prev}]，这里又给 [{hex}]——同一条文本不能有两种期望"
                ));
            }
        }
    }
    Ok(())
}

// ─────────────────────────── [encoding] ───────────────────────────

/// `[encoding]` 校验（v18 S4 宽度三态）。
///
/// 三态各自的**必填/禁用**关系必须在编译期钉死，否则"声明了却不生效"这类错
/// 会在几层之后才浮现：
///
/// - `fixed`：`bits` > 0（写了 0 就是写错）；`widths`/`max_len` 不适用（写了就是写错了）。
///   **`[encoding]` 整个省略**（= 缺省 `kind = "fixed"` 且无 `bits`）是合法的骨架文档——
///   字长由生成期 `inst_bytes()` 明确报错，不在解析期拦。
/// - `mixed`：`widths` 必填非空、每项 > 0、去重后至少一项；`bits`（缺省字长）可省；
///   每条指令的 `width`（若给）必须落在 `widths` 里；
/// - `prefix_scan`：`max_len` 可省（缺省 15），必须 > 0；`bits`/`widths` 不适用；
/// - 无论哪种：指令的 `width`（若给）> 0，且 `fixed` 下必须等于 `bits`
///   （显式重复允许，写别的值就是写错了）。
fn validate_encoding(m: &V12Model) -> Result<(), String> {
    let e = &m.encoding;
    if e.default_opsize == Some(0) {
        return Err("[encoding].default_opsize must be > 0".into());
    }
    match e.kind {
        EncodingKind::Fixed => {
            if e.bits == Some(0) {
                return Err("[encoding].bits must be > 0".into());
            }
            if !e.widths.is_empty() {
                return Err(
                    "[encoding].widths 只适用于 kind = \"mixed\"（fixed 只有一个字长，写 bits）"
                        .into(),
                );
            }
            if e.max_len.is_some() {
                return Err(
                    "[encoding].max_len 只适用于 kind = \"prefix_scan\"（fixed 的字长写 bits）"
                        .into(),
                );
            }
        }
        EncodingKind::Mixed => {
            if e.widths.is_empty() {
                return Err(
                    "[encoding].widths 不能为空：kind = \"mixed\" 必须列出允许的字长集（位）"
                        .into(),
                );
            }
            if e.widths.contains(&0) {
                return Err("[encoding].widths 里的字长必须 > 0".into());
            }
            let mut sorted = e.widths.clone();
            sorted.sort_unstable();
            sorted.dedup();
            if sorted.len() != e.widths.len() {
                return Err("[encoding].widths 里有重复字长（同一字长写一次即可）".into());
            }
            if e.max_len.is_some() {
                return Err("[encoding].max_len 只适用于 kind = \"prefix_scan\"".into());
            }
            if let Some(bits) = e.bits
                && !e.widths.contains(&bits)
            {
                return Err(format!(
                    "[encoding].bits ({bits}) 必须也在 widths 里（它是 width 缺省值）"
                ));
            }
        }
        EncodingKind::PrefixScan => {
            if let Some(l) = e.max_len
                && l == 0
            {
                return Err("[encoding].max_len must be > 0".into());
            }
            if e.bits.is_some() {
                return Err(
                    "[encoding].bits 只适用于 kind = \"fixed\"/\"mixed\"（prefix_scan 是逐指令变长）"
                        .into(),
                );
            }
            if !e.widths.is_empty() {
                return Err("[encoding].widths 只适用于 kind = \"mixed\"".into());
            }
        }
    }
    // 逐指令 width
    for inst in &m.instructions {
        let Some(w) = inst.width else { continue };
        if w == 0 {
            return Err(format!("[[instructions.{}]]: width must be > 0", inst.name));
        }
        match e.kind {
            EncodingKind::Fixed => {
                if e.bits.is_none() {
                    return Err(format!(
                        "[[instructions.{}]]: width = {w} 不能替代 [encoding].bits\
                         （kind = \"fixed\" 全 ISA 只有一个字长，请在 [encoding].bits 声明一次）",
                        inst.name
                    ));
                }
                if Some(w) != e.bits {
                    return Err(format!(
                        "[[instructions.{}]]: width {w} 与 [encoding].bits ({}) 不一致\
                         （fixed ISA 只有一个字长）",
                        inst.name,
                        e.bits.unwrap_or(0)
                    ));
                }
            }
            EncodingKind::Mixed => {
                if !e.widths.contains(&w) {
                    return Err(format!(
                        "[[instructions.{}]]: width {w} 不在 [encoding].widths ({:?}) 里",
                        inst.name, e.widths
                    ));
                }
            }
            EncodingKind::PrefixScan => {
                return Err(format!(
                    "[[instructions.{}]]: prefix_scan ISA 不得写 width（字长由前缀扫描决定）",
                    inst.name
                ));
            }
        }
    }
    Ok(())
}

// ─────────────────────────── [meta] ───────────────────────────

fn validate_meta(m: &V12Model) -> Result<(), String> {
    let name = &m.meta.name;
    if !is_valid_meta_name(name) {
        return Err(format!(
            "[meta].name '{name}' is not a valid ISA name (letters/digits/_/-, must start with a letter or _)"
        ));
    }
    if m.meta.comment_char.chars().count() != 1 {
        return Err("[meta].comment_char must be exactly one char".into());
    }
    if m.meta.label_suffix.is_empty() {
        return Err("[meta].label_suffix must not be empty".into());
    }
    if let Some(p) = &m.meta.imm_prefix
        && p.chars().count() != 1
    {
        return Err("[meta].imm_prefix must be exactly one char or absent".into());
    }
    if m.meta.directive_prefix.is_empty() {
        return Err("[meta].directive_prefix must not be empty".into());
    }
    // `[meta].variants`（v19 V5）：参数名非空、取值域非空——空域等于"任何取值都非法"，
    // 写出来只会让所有投影调用报"不在声明域内"，必须在声明处就说清。
    if let Some(vars) = &m.meta.variants {
        for (k, domain) in vars {
            if k.trim().is_empty() {
                return Err("[meta].variants: 参数名不能为空".into());
            }
            if domain.is_empty() {
                return Err(format!(
                    "[meta].variants: 参数 `{k}` 的取值域为空——至少给一个取值（如 `{k} = [32, 64]`）"
                ));
            }
        }
    }
    Ok(())
}

/// `only_variants` 的声明一致性（v19 V5）：六处声明里提到的参数名必须在
/// `[meta].variants` 里声明过，取值必须是声明域的子集。
///
/// **为什么硬报**：未声明的参数永远不会出现在 `params` 里 ⇒ `variants_keep` 永远返回真
/// ⇒ 标了 `only_variants` 却**从不生效**（最难查的一类：看起来"变体机制没起作用"）；
/// 取值超出声明域同理（那个取值永远传不进来）。
fn validate_variant_gates(m: &V12Model) -> Result<(), String> {
    let declared = m.meta.variants.clone().unwrap_or_default();
    let known = || {
        if declared.is_empty() {
            "（谱里没有声明任何变体参数）".to_string()
        } else {
            declared.keys().cloned().collect::<Vec<_>>().join(", ")
        }
    };
    let check = |gate: Option<&VariantGate>| -> Result<(), String> {
        let Some(g) = gate else { return Ok(()) };
        for (k, domain) in g {
            let Some(allowed) = declared.get(k) else {
                return Err(format!(
                    "[meta].variants: `only_variants` 用了未声明的参数 `{k}`（已声明：{}）——\
                     未声明的参数永远不会传进来，这条标记恒不生效",
                    known()
                ));
            };
            if domain.is_empty() {
                return Err(format!(
                    "[meta].variants: `only_variants` 里参数 `{k}` 的取值域为空——该声明在任何变体下都不存在（要删就删掉声明本身）"
                ));
            }
            if let Some(bad) = domain.iter().find(|v| !allowed.contains(v)) {
                return Err(format!(
                    "[meta].variants: `only_variants` 里参数 `{k}` 的取值 {bad} 不在声明域 {allowed:?} 内"
                ));
            }
        }
        Ok(())
    };
    for i in &m.instructions {
        check(i.only_variants.as_ref())?;
    }
    for s in m.spill.values() {
        check(s.only_variants.as_ref())?;
    }
    for p in &m.pseudo {
        check(p.only_variants.as_ref())?;
    }
    for p in &m.pattern {
        check(p.only_variants.as_ref())?;
    }
    Ok(())
}

/// 合法 ISA 名：字母/数字/`_`/`-`，首字符为字母或 `_`（派生模块名：小写 + `-`→`_`）。
fn is_valid_meta_name(s: &str) -> bool {
    let mut chars = s.chars();
    match chars.next() {
        Some(c) if c.is_ascii_alphabetic() || c == '_' => {}
        _ => return false,
    }
    s.chars()
        .all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '-')
}

// ─────────────────────────── [reg.*] ───────────────────────────

fn validate_regs(m: &V12Model) -> Result<(), String> {
    if m.reg.is_empty() {
        return Err("missing [reg.*] sections: at least one register group is required".into());
    }
    for (gname, g) in &m.reg {
        if gname.width() == 0 {
            return Err(format!("[reg.{gname}].width must be > 0"));
        }
        match &g.names {
            Some(names) => {
                if names.is_empty() {
                    return Err(format!("[reg.{gname}].names must not be empty"));
                }
                let mut seen = BTreeSet::new();
                for n in names {
                    if n.trim().is_empty() {
                        return Err(format!("[reg.{gname}].names contains an empty name"));
                    }
                    if !seen.insert(n.clone()) {
                        return Err(format!("[reg.{gname}].names has duplicate '{n}'"));
                    }
                }
                if let Some(c) = g.count
                    && c as usize != names.len()
                {
                    return Err(format!(
                        "[reg.{gname}]: count ({c}) != names.len ({})",
                        names.len()
                    ));
                }
            }
            None => {
                let Some(c) = g.count else {
                    return Err(format!(
                        "[reg.{gname}]: either `names` or `count` is required"
                    ));
                };
                if c == 0 {
                    return Err(format!("[reg.{gname}].count must be > 0"));
                }
            }
        }
    }
    Ok(())
}

// ─────────────────────── 类/宽度元数据（去写死）───────────────────────

/// 宽度元数据校验（2026-09-12 去「宽度写死」）：
/// 显式宽度键必须指向已声明组、为合法字节宽度；`default_opsize` 若声明则必须
/// 是 8 的倍数并与某个已声明 GPR 组的位宽一致。缺组一律 Err（fail-closed），
/// 不静默回退到 `GPR(8)`/`GPR(4)` 这类 x86 缺省。
fn validate_widths(m: &V12Model) -> Result<(), String> {
    // 显式宽度键 → 必须存在对应组（派生方法内部即校验）。
    let _ = m.main_gpr_class()?;
    let _ = m.main_fpr_class()?;
    let _ = m.addr_class()?;
    let _ = m.value_gpr_class()?;
    let _ = m.value_fpr_class()?;
    // 向量 by-ref 阈值：`strategy`/`limit` 成对性 + 唯一性 + 8 的倍数
    // （此前只在 `gen_abi` 生成期检查，读 TOML 的人拿不到早期反馈）。
    let _ = m.vector_by_ref_limit_bytes()?;
    if m.slot_bytes()? == 0 {
        return Err("[stack].slot must be > 0".into());
    }
    if m.fp_overhead_bytes()? == 0 && m.stack.as_ref().and_then(|s| s.fp_save).is_some() {
        return Err("[stack].fp_save must be > 0".into());
    }
    if m.stack.as_ref().and_then(|s| s.slot).is_some() && m.slot_bytes()? == 0 {
        return Err("[stack].slot must be > 0".into());
    }
    if m.stack.as_ref().and_then(|s| s.align).is_some() && m.stack_align()? == 0 {
        return Err("[stack].align must be > 0".into());
    }
    // `[abi.stack_args]`：基址只能是 fp|sp；槽数与步长必须 > 0。
    if let Some(sa) = m.abi.as_ref().and_then(|a| a.stack_args.as_ref()) {
        for (key, base) in [
            ("callee_base", sa.callee_base.as_ref()),
            ("caller_base", sa.caller_base.as_ref()),
        ] {
            if let Some(base) = base
                && base != "fp"
                && base != "sp"
            {
                return Err(format!(
                    "[abi.stack_args].{key} = \"{base}\" 非法（只能是 \"fp\" 或 \"sp\"）"
                ));
            }
        }
        if sa.first_offset_slots == Some(0) {
            return Err("[abi.stack_args].first_offset_slots must be > 0".into());
        }
        if sa.stride_slots == Some(0) {
            return Err("[abi.stack_args].stride_slots must be > 0".into());
        }
    }
    for (key, w) in [
        ("default_gpr_width", m.meta.default_gpr_width),
        ("default_fpr_width", m.meta.default_fpr_width),
        ("addr_width", m.meta.addr_width),
        ("value_gpr_width", m.meta.value_gpr_width),
        ("value_fpr_width", m.meta.value_fpr_width),
    ] {
        if let Some(w) = w
            && w == 0
        {
            return Err(format!("[meta].{key} must be > 0"));
        }
    }
    // 向量档位：升序、非 0、去重（生成代码按"最小的 ≥ 请求字节数的档位"选择）。
    if let Some(tiers) = &m.meta.vector_tiers {
        if tiers.is_empty() {
            return Err("[meta].vector_tiers must not be empty".into());
        }
        let mut prev = 0u16;
        for t in tiers {
            if *t == 0 {
                return Err("[meta].vector_tiers must be > 0".into());
            }
            if *t <= prev {
                return Err(format!(
                    "[meta].vector_tiers must be strictly ascending (got {tiers:?})"
                ));
            }
            prev = *t;
        }
    }
    if let Some(bits) = m.encoding.default_opsize {
        if bits == 0 || bits % 8 != 0 {
            return Err(format!(
                "[encoding].default_opsize = {bits} 必须是 8 的倍数（单位：位）"
            ));
        }
        let want = (bits / 8) as u16;
        if !m
            .reg
            .keys()
            .any(|rc| matches!(rc, RegClass::GPR(w) if *w == want))
        {
            return Err(format!(
                "[encoding].default_opsize = {bits}（{want} 字节）没有对应的 [reg.gpr{want}] 组"
            ));
        }
    }
    // 结构化寄存器名字段必须能解析到**已声明组**内的名字（fail-closed）：
    // 历史实现用 `filter_map`/`unwrap_or_default` 静默丢弃未知名字 ⇒ sp/fp 落回
    // 索引 0、scratch/callee_saved 缺失（regalloc 会分配被占用寄存器）。
    // 自由模板（lowering/emit 的 `insts`）不在这里猜——它含助记符与内存语法。
    validate_reg_names(m)?;
    validate_spill_coverage(m)?;
    validate_types(m)?;
    Ok(())
}

/// `[types]` 显式映射校验（B2）：类型名合法、目标组已声明（在
/// `explicit_type_map` 里查）、映射的类宽 ≥ 该类型的字节宽（`ptr` 按 ISA
/// 地址宽；`void` 只能 unsupported）——防止把 `i64` 映射到 1 字节组这类
/// "看起来能用、实际截断"的配置。**显式条目优先于通用值池规则**。
fn validate_types(m: &V12Model) -> Result<(), String> {
    let addr_w = m.addr_class()?.width();
    for (ty, target) in m.explicit_type_map()? {
        let Some(rc) = target else {
            // 显式 unsupported：合法（把"宿主会拒绝"变成"ISA 明确声明不支持"）。
            continue;
        };
        if ty == "void" {
            return Err("[types].void: void 不承载寄存器，只能写 \"unsupported\"".into());
        }
        // 向量类型：类宽必须 ≥ 元素字节数（tier 语义），其余按标量字节宽。
        let need = match ty.as_str() {
            "ptr" => Some(addr_w),
            other => crate::v12::model::type_name_bytes(other),
        };
        if let Some(need) = need
            && rc.width() < need
        {
            return Err(format!(
                "[types].{ty} = \"{rc}\": 类宽 {} 字节 < 该类型 {} 字节（会静默截断）",
                rc.width(),
                need
            ));
        }
    }
    Ok(())
}

/// 溢出模板覆盖校验（R5）：**每个比宿主浮点值池更宽的浮点/向量类**都必须有
/// 对应的 `[spill.FPR<bytes>]` 模板——生成器的 FPR 溢出按宽度档分派，缺档会在
/// **编译期**（IR 编译时）才报 `Unsupported`，读 TOML 的人很难定位。
///
/// 这里把它提前到 DSL 校验期：点名"哪个类需要哪个键"。（没有浮点组、或只有
/// 标量宽度的 ISA 不需要任何档位。）
fn validate_spill_coverage(m: &V12Model) -> Result<(), String> {
    let scalar = m.value_fpr_class()?.map(|c| c.width()).unwrap_or(0);
    // 有 `[spill.FPR]` 才谈档位（否则 FPR 溢出整条路径本来就是 no-op）。
    let Some(_base) = m.spill.get("FPR") else {
        return Ok(());
    };
    let declared_tiers: BTreeSet<u16> = m
        .spill
        .keys()
        .filter_map(|k| k.strip_prefix("FPR"))
        .filter_map(|n| n.parse::<u16>().ok())
        .collect();
    for rc in m.reg.keys() {
        let w = match rc {
            RegClass::FPR(w) | RegClass::VEC(w) => *w,
            _ => continue,
        };
        if w <= scalar || declared_tiers.contains(&w) {
            continue;
        }
        return Err(format!(
            "[reg.{rc}] 宽 {w} 字节 > 浮点值池 {scalar} 字节，但缺少 [spill.FPR{w}] 溢出模板\
             ——生成器按宽度档分派，缺档会在 IR 编译期才报 Unsupported（此处提前点名）"
        ));
    }
    Ok(())
}

/// 结构化寄存器名引用校验：`[abi]` 的 sp/fp/scratch/reserved/callee_saved/
/// ret_regs/call_ret_reg/call_clobbers/implicit_regs/arg_class.regs 与
/// `[spill.*].base` 必须在某个已声明寄存器组内。
fn validate_reg_names(m: &V12Model) -> Result<(), String> {
    let mut declared: BTreeSet<String> = BTreeSet::new();
    for (rc, g) in &m.reg {
        for n in super::shared::group_names(g).map_err(|e| format!("[reg.{rc}]: {e}"))? {
            declared.insert(n);
        }
    }
    let check = |names: &[String], key: &str| -> Result<(), String> {
        for n in names {
            if !declared.contains(n.as_str()) {
                return Err(format!(
                    "{key}: 物理寄存器名 \"{n}\" 不在任何已声明 [reg.*] 组内\
                     （生成期 fail-closed；历史实现静默丢弃 ⇒ 该寄存器不被排除/不被保存）"
                ));
            }
        }
        Ok(())
    };
    if let Some(abi) = &m.abi {
        check(&abi.scratch, "[abi].scratch")?;
        check(&abi.reserved, "[abi].reserved")?;
        check(&abi.ret_regs, "[abi].ret_regs")?;
        check(
            &abi.call_clobbers.clone().unwrap_or_default(),
            "[abi].call_clobbers",
        )?;
        if let Some(r) = &abi.call_ret_reg {
            check(std::slice::from_ref(r), "[abi].call_ret_reg")?;
        }
        if let Some(cs) = &abi.callee_saved {
            check(&cs.gpr, "[abi.callee_saved].gpr")?;
        }
        for ac in &abi.arg_class {
            check(&ac.regs, "[abi.arg_class].regs")?;
        }
        if let Some(f) = &abi.frame {
            check(std::slice::from_ref(&f.sp), "[abi.frame].sp")?;
            if let Some(fp) = &f.fp {
                check(std::slice::from_ref(fp), "[abi.frame].fp")?;
            }
        }
    }
    for (name, t) in &m.spill {
        if let Some(b) = &t.base {
            check(std::slice::from_ref(b), &format!("[spill.{name}].base"))?;
        }
    }
    for inst in &m.instructions {
        if let Some(ir) = &inst.implicit_regs {
            check(
                ir,
                &format!("[[instructions.{name}]].implicit_regs", name = inst.name),
            )?;
        }
    }
    Ok(())
}

// ─────────────────────── [conventions.*] ───────────────────────

fn validate_conventions(m: &V12Model) -> Result<(), String> {
    let conv = &m.conventions;
    for (name, bf) in &conv.bitfields {
        match (&bf.offset, &bf.width, &bf.pieces) {
            (Some(off), Some(w), None) => {
                if *w == 0 {
                    return Err(format!("[conventions.bitfields.{name}].width must be > 0"));
                }
                // 字侧（offset）无上限——字长是 ISA 数据、由字节数组承载；
                // 只有**单个位域的值宽**受 u64/i64 值表示限制。
                if *w > 64 {
                    return Err(format!(
                        "[conventions.bitfields.{name}].width = {w} 超过值表示上限 64 位\
                         （位域值承载在 u64 常量键与 i64 操作数上；字长本身无上限，\
                         可拆成多个 ≤64 位的域）"
                    ));
                }
                let _ = off;
            }
            (None, None, Some(pieces)) => {
                if pieces.is_empty() {
                    return Err(format!(
                        "[conventions.bitfields.{name}].pieces must not be empty"
                    ));
                }
                // 字位域 [offset, offset+width) 两两不重叠 + 值位域 [shift, shift+width) 两两不重叠
                let mut word_ranges: Vec<(u32, u32)> = Vec::new();
                let mut value_ranges: Vec<(u32, u32)> = Vec::new();
                for (i, p) in pieces.iter().enumerate() {
                    if p.width == 0 {
                        return Err(format!(
                            "[conventions.bitfields.{name}].pieces[{i}].width must be > 0"
                        ));
                    }
                    // 字侧 piece.offset 无上限；值侧 shift+width ≤ 64（值表示上限）。
                    if p.width > 64 || p.shift + p.width > 64 {
                        return Err(format!(
                            "[conventions.bitfields.{name}].pieces[{i}]: width/shift+width 超过值表示上限 64 位\
                             （字长无上限，可拆更多 piece）"
                        ));
                    }
                    if word_ranges
                        .iter()
                        .any(|(s, e)| p.offset < *e && p.offset + p.width > *s)
                    {
                        return Err(format!(
                            "[conventions.bitfields.{name}].pieces: word-bit ranges overlap at piece {i}"
                        ));
                    }
                    if value_ranges
                        .iter()
                        .any(|(s, e)| p.shift < *e && p.shift + p.width > *s)
                    {
                        return Err(format!(
                            "[conventions.bitfields.{name}].pieces: value-bit ranges overlap at piece {i}"
                        ));
                    }
                    word_ranges.push((p.offset, p.offset + p.width));
                    value_ranges.push((p.shift, p.shift + p.width));
                }
            }
            _ => {
                return Err(format!(
                    "[conventions.bitfields.{name}]: declare either {{ offset, width }} or {{ pieces = [...] }}, not both/neither"
                ));
            }
        }
    }
    // 定宽/混合字长 ISA：每个位域必须落在**最宽**指令字内（字宽 = ISA 数据；
    // 历史实现把"定宽 = 32 位"写死在 codegen，超宽位域会被静默移位出字/截断）。
    let widest = match m.encoding.kind {
        EncodingKind::Fixed => m.encoding.bits,
        EncodingKind::Mixed => m.encoding.widths.iter().copied().max(),
        EncodingKind::PrefixScan => None,
    };
    if let Some(bits) = widest {
        for (name, bf) in &conv.bitfields {
            let hi = match &bf.pieces {
                Some(ps) => ps.iter().map(|p| p.offset + p.width).max(),
                None => bf.offset.zip(bf.width).map(|(o, w)| o + w),
            };
            if let Some(hi) = hi
                && hi > bits
            {
                return Err(format!(
                    "[conventions.bitfields.{name}]: 位域最高位 {hi} 超出指令字宽 {bits} 位\
                     （[encoding]；请缩短位域或加宽指令字）"
                ));
            }
        }
    }
    if let Some(modrm) = &conv.modrm {
        for f in [&modrm.reg_field, &modrm.rm_field].into_iter().flatten() {
            if !conv.bitfields.contains_key(f) {
                return Err(format!(
                    "[conventions.modrm]: bitfield '{f}' is not declared in [conventions.bitfields]"
                ));
            }
        }
        let mut seen = BTreeSet::new();
        for b in &modrm.force_disp_base {
            if *b >= 16 {
                return Err(format!(
                    "[conventions.modrm].force_disp_base: {b} out of range (0..=15, REX.X-extended rm)"
                ));
            }
            if !seen.insert(*b) {
                return Err(format!(
                    "[conventions.modrm].force_disp_base: duplicate {b}"
                ));
            }
        }
    }
    if let Some(ps) = &conv.prefix_scan {
        const KNOWN: [&str; 6] = ["opsize16", "lock", "repe", "repne", "addr16", "rex"];
        for (i, e) in ps.iter().enumerate() {
            let path = format!("[conventions.prefix_scan][{i}]");
            if e.byte.is_none() && e.range.is_none() {
                return Err(format!("{path}: entry needs `byte` or `range`"));
            }
            if e.byte.is_some() && e.range.is_some() {
                return Err(format!("{path}: `byte` and `range` are mutually exclusive"));
            }
            if let Some(b) = e.byte
                && b > 0xFF
            {
                return Err(format!("{path}: byte 0x{b:x} exceeds one byte"));
            }
            for fx in &e.effects {
                if !KNOWN.contains(&fx.as_str()) {
                    return Err(format!(
                        "{path}: unknown effect '{fx}' (opsize16/lock/repe/repne/addr16/rex)"
                    ));
                }
            }
        }
    }
    if let Some(mt) = &conv.mem {
        let items = super::codegen::mem::parse_mem_template(&mt.template)
            .map_err(|e| format!("[conventions.mem]: {e}"))?;
        super::codegen::mem::validate_mem_template(&items)
            .map_err(|e| format!("[conventions.mem]: {e}"))?;
    }
    Ok(())
}

/// `[[pseudo]]` 校验（v18 S3e 汇编器伪指令）。
///
/// 伪指令是**汇编期**行为，编译器看不到它的调用点，因此声明侧必须自洽：
///
/// - 名字非空、唯一、**不得与任何指令助记符重名**（重名会让汇编器永远匹配不到它，
///   或静默遮蔽——两者都是最难查的一类错）；
/// - `params` 非空、每项非空、唯一；
/// - `emit` 非空、每行非空；行首词必须是**已声明指令助记符或别的伪指令名**
///   （或 `.` 开头的伪指令），否则那是拼错；
/// - 每行的 `{…}` 必须是已声明参数（拼错即报），且**每个参数都至少用一次**
///   （没用到的参数几乎总是写错了名字）。
fn validate_pseudos(m: &V12Model) -> Result<(), String> {
    if m.pseudo.is_empty() {
        return Ok(());
    }
    // 指令助记符（asm 首词）+ 伪指令名（emit 行首的合法取值）
    let mut mnemonics: BTreeSet<String> = BTreeSet::new();
    for inst in &m.instructions {
        if let Some(w) = inst.asm.split_whitespace().next()
            && !w.is_empty()
        {
            mnemonics.insert(w.to_string());
        }
    }
    let pseudo_names: BTreeSet<&str> = m.pseudo.iter().map(|p| p.name.as_str()).collect();
    let mut seen: BTreeSet<&str> = BTreeSet::new();
    for p in &m.pseudo {
        if p.name.trim().is_empty() {
            return Err("[[pseudo]]: name 不能为空".into());
        }
        if !seen.insert(p.name.as_str()) {
            return Err(format!("[[pseudo.{}]]: 伪指令名重复", p.name));
        }
        if mnemonics.contains(&p.name) {
            return Err(format!(
                "[[pseudo.{}]]: 伪指令名与指令助记符重名——汇编器会先匹配到指令，伪指令永不生效",
                p.name
            ));
        }
        if p.params.is_empty() {
            return Err(format!("[[pseudo.{}]]: params 不能为空", p.name));
        }
        let mut pseen: BTreeSet<&str> = BTreeSet::new();
        for a in &p.params {
            if a.trim().is_empty() {
                return Err(format!("[[pseudo.{}]]: params 里的名字不能为空", p.name));
            }
            if !pseen.insert(a.as_str()) {
                return Err(format!("[[pseudo.{}]]: 参数 '{a}' 重复", p.name));
            }
        }
        if p.emit.is_empty() {
            return Err(format!(
                "[[pseudo.{}]]: emit 不能为空（至少要展开出一行）",
                p.name
            ));
        }
        let mut used: BTreeSet<String> = BTreeSet::new();
        for line in &p.emit {
            let l = line.trim();
            if l.is_empty() {
                return Err(format!("[[pseudo.{}]]: emit 里有空行", p.name));
            }
            let head = l.split_whitespace().next().unwrap_or("");
            if !head.starts_with('.') && !mnemonics.contains(head) && !pseudo_names.contains(head) {
                return Err(format!(
                    "[[pseudo.{}]]: emit 行 '{l}' 的首词 '{head}' 既不是指令助记符、\
                     也不是别的伪指令名（拼错了？）",
                    p.name
                ));
            }
            for tok in placeholder_tokens(l) {
                let name = tok.trim_start_matches('{').trim_end_matches('}');
                if !pseen.contains(name) {
                    return Err(format!(
                        "[[pseudo.{}]]: emit 里的 '{{{name}}}' 不是声明过的参数（可用：{}）",
                        p.name,
                        p.params.join(" / ")
                    ));
                }
                used.insert(name.to_string());
            }
        }
        for a in &p.params {
            if !used.contains(a.as_str()) {
                return Err(format!(
                    "[[pseudo.{}]]: 参数 '{a}' 在 emit 里没用到（写错了名字？）",
                    p.name
                ));
            }
        }
    }
    Ok(())
}

/// `[[derive]]` 校验（v18 S3f 派生谓词属性）。
///
/// 解析期已查过：名称非空/唯一/不与核心属性重名、`expr` 语法合法。这里查语义：
/// **派生只能引用核心属性**（不支持派生引用派生——见 `expand_derives` 的理由），
/// 且 `expr` 里出现的每个属性名都必须是核心属性。
fn validate_derives(m: &V12Model) -> Result<(), String> {
    for d in &m.derive {
        let Some(pred) = m.derived_preds.get(&d.name) else {
            continue;
        };
        let mut attrs = Vec::new();
        super::pred::attrs_of(pred, &mut attrs);
        for a in &attrs {
            if !super::pred::PRED_ATTRS.contains(&a.as_str()) {
                let hint = if m.derive.iter().any(|o| &o.name == a) {
                    "（派生不能引用派生——把这条条件直接写进 expr）"
                } else {
                    ""
                };
                return Err(format!(
                    "[[derive.{}]] expr: 未知属性 '{a}'{hint}（可用：{}）",
                    d.name,
                    super::pred::PRED_ATTRS.join(" / ")
                ));
            }
        }
    }
    Ok(())
}

/// `[[reloc]]` 校验（v18 S3d 重定位数据化）。
///
/// - 表项名非空、唯一；
/// - `slot` 必须已声明且是 `kind = "imm"` 的槽（absolute 的补丁宽度 = 槽宽；
///   pc_relative 的 fixup 落在指令起始，槽只是"这条指令把重定位值承载在哪"）；
/// - 指令的 `reloc = "<名>"` 必须指向已声明表项，且该指令确实有那个槽的操作数。
fn validate_relocs(m: &V12Model) -> Result<(), String> {
    let mut seen = BTreeSet::new();
    for r in &m.reloc {
        if r.name.trim().is_empty() {
            return Err("[[reloc]]: name must not be empty".into());
        }
        if !seen.insert(r.name.clone()) {
            return Err(format!("[[reloc.{}]]: 重定位名重复", r.name));
        }
        let Some(slot) = m.operand_slots.iter().find(|s| s.name == r.slot) else {
            return Err(format!(
                "[[reloc.{}]]: 绑定的操作数槽 '{}' 未在 [[operand_slots]] 声明",
                r.name, r.slot
            ));
        };
        if slot.kind != OperandKind::Imm {
            return Err(format!(
                "[[reloc.{}]]: 绑定的槽 '{}' 必须是 imm 槽（重定位值要写进一个数值槽），实际是 {:?}",
                r.name, r.slot, slot.kind
            ));
        }
    }
    for inst in &m.instructions {
        let Some(name) = inst.reloc.as_deref() else {
            continue;
        };
        let Some(def) = m.reloc.iter().find(|r| r.name == name) else {
            return Err(format!(
                "[[instructions.{}]]: reloc '{name}' 未在 [[reloc]] 声明（可用：{}）",
                inst.name,
                if m.reloc.is_empty() {
                    "<空>".to_string()
                } else {
                    m.reloc
                        .iter()
                        .map(|r| r.name.as_str())
                        .collect::<Vec<_>>()
                        .join(" / ")
                }
            ));
        };
        let ops = super::codegen::parse_asm_decl(
            &inst.asm,
            inst.ops.as_deref(),
            &inst.name,
            &m.variant_param_names(),
        )?
        .0;
        if !ops.iter().any(|o| o.slot == def.slot) {
            return Err(format!(
                "[[instructions.{}]]: reloc '{name}' 绑定的槽 '{}' 不在该指令的操作数里（asm '{}'）",
                inst.name, def.slot, inst.asm
            ));
        }
    }
    Ok(())
}

/// `[conventions.cond]` 校验（v18 S3b 条件码数据化）。
///
/// 这张表同时服务三处（汇编解析名、反汇编渲染名、lowering 的 `{cc}`），因此校验也
/// 三面都盖：
///
/// 1. 键非空、`code` 落在 4 位条件字段内（0..=15）；
/// 2. `ir` 必须是 [`IR_INT_COND_NAMES`] 之一，且**每个 IR 条件只能被映射一次**
///    （否则 `{cc}` 该取哪个编码就没有答案）；
/// 3. **用到 `cond` 槽却没有表** ⇒ 报错（v18 S3b 起不再回退 x86 的 16 项表）；
///    **用到 `{cc}` 却没把 10 个 IR 条件映射全** ⇒ 报错（缺的那个会在运行期
///    静默退化成 0 = 溢出条件，是最难查的一类错）。
fn validate_cond(m: &V12Model) -> Result<(), String> {
    let table = m.conventions.cond.as_ref();
    if let Some(t) = table {
        if t.is_empty() {
            return Err(
                "[conventions.cond]: 条件码表不能为空（要么整节不写，要么至少一条）".into(),
            );
        }
        let mut owner: BTreeMap<&str, &str> = BTreeMap::new();
        for (name, e) in t {
            if name.trim().is_empty() {
                return Err("[conventions.cond]: 条件名不能为空".into());
            }
            if e.code > 15 {
                return Err(format!(
                    "[conventions.cond.{name}]: code {} 超出条件字段宽度（4 位，0..=15）",
                    e.code
                ));
            }
            if let Some(ir) = e.ir_condition(name.as_str()) {
                if !IR_INT_COND_NAMES.contains(&ir) {
                    return Err(format!(
                        "[conventions.cond.{name}].ir '{ir}' 不是 IR 整数条件名（可用：{}）",
                        IR_INT_COND_NAMES.join(" / ")
                    ));
                }
                if let Some(prev) = owner.insert(ir, name) {
                    return Err(format!(
                        "[conventions.cond]: IR 条件 '{ir}' 被 '{prev}' 与 '{name}' 重复映射\
                         （每个 IR 条件只能映射到一个编码）"
                    ));
                }
            }
        }
    }
    // `cond` 槽（汇编侧）需要表
    if table.is_none()
        && let Some(slot) = m.operand_slots.iter().find(|s| s.kind == OperandKind::Cond)
    {
        return Err(format!(
            "[conventions.cond]: 操作数槽 '{}' 的 kind = \"cond\" 需要条件码表——\
             声明 [conventions.cond]（键 = 本 ISA 汇编可见的条件名）",
            slot.name
        ));
    }
    // `{cc}`（lowering 侧）需要**全部** IR 整数条件
    if m.lowering
        .iter()
        .any(|r| r.insts.iter().any(|s| s.contains("{cc}")))
    {
        let missing: Vec<&str> = IR_INT_COND_NAMES
            .iter()
            .copied()
            .filter(|n| !m.cond_ir_codes().iter().any(|(k, _)| k == n))
            .collect();
        if !missing.is_empty() {
            return Err(format!(
                "[[lowering]]: 用了 `{{cc}}`（当前 IR 条件 → 本 ISA 编码）但 [conventions.cond] \
                 没把所有 IR 整数条件映射全——缺 {}（每条用 ir = \"<条件名>\" 指出它实现哪个条件）",
                missing.join(" / ")
            ));
        }
    }
    Ok(())
}

// ──────────────────── [[operand_slots]] ────────────────────

fn validate_operand_slots(m: &V12Model) -> Result<(), String> {
    if m.operand_slots.is_empty() {
        return Err("missing [[operand_slots]]: at least one operand slot is required".into());
    }
    let mut seen = BTreeSet::new();
    for (i, s) in m.operand_slots.iter().enumerate() {
        let path = format!("[[operand_slots]] #{i} ('{}')", s.name);
        if !is_valid_meta_name(&s.name) {
            return Err(format!("{path}: invalid slot name '{}'", s.name));
        }
        if !seen.insert(s.name.clone()) {
            return Err(format!(
                "[[operand_slots]]: duplicate slot name '{}'",
                s.name
            ));
        }
        match s.kind {
            OperandKind::Reg => {
                // class/classes 可省略 → 任意寄存器类（不推荐，会吞掉更具体的
                // 重载形式）。指定时必须是已声明的 [reg.*] 组（悬空引用报错）。
                let mut all = Vec::new();
                if let Some(c) = &s.class {
                    all.push(*c);
                }
                if let Some(cs) = &s.classes {
                    all.extend(cs.iter().cloned());
                }
                for c in &all {
                    if !m.reg.contains_key(c) {
                        return Err(format!(
                            "{path}: class '{c}' is not a declared [reg.*] group"
                        ));
                    }
                }
            }
            OperandKind::Imm => match s.width {
                None => {
                    return Err(format!("{path}: imm slot requires `width` (value bits)"));
                }
                Some(0) => {
                    return Err(format!("{path}: width must be > 0"));
                }
                _ => {}
            },
            _ => {}
        }
        // 立即数约束一致性：min ≤ max。
        if let (Some(lo), Some(hi)) = (s.min, s.max)
            && lo > hi
        {
            return Err(format!("{path}: min ({lo}) > max ({hi})"));
        }
        if let Some(roles) = &s.roles {
            if roles.is_empty() {
                return Err(format!("{path}: roles must not be empty"));
            }
            let mut rseen = BTreeSet::new();
            for r in roles {
                if !rseen.insert(*r) {
                    return Err(format!("{path}: duplicate role {r:?}"));
                }
            }
        }
    }
    Ok(())
}

// ──────────────────────── [[forms]] ────────────────────────

fn validate_forms(m: &V12Model) -> Result<(), String> {
    let mut seen = BTreeSet::new();
    for f in &m.forms {
        if !is_valid_meta_name(&f.name) {
            return Err(format!("[[forms]]: invalid form name '{}'", f.name));
        }
        if !seen.insert(f.name.clone()) {
            return Err(format!("[[forms]]: duplicate form name '{}'", f.name));
        }
        if let Some(bf) = &f.keys.opcode_field
            && !m.conventions.bitfields.contains_key(bf)
        {
            return Err(format!(
                "[[forms.{}]].opcode_field '{bf}' is not declared in [conventions.bitfields]",
                f.name
            ));
        }
        if let Some(of) = &f.keys.operand_fields {
            for (i, bf) in of.iter().enumerate() {
                if !m.conventions.bitfields.contains_key(bf) {
                    return Err(format!(
                        "[[forms.{}]].operand_fields[{i}] '{bf}' is not declared in [conventions.bitfields]",
                        f.name
                    ));
                }
            }
        }
        if let Some(esc) = &f.keys.escape {
            if esc.is_empty() {
                return Err(format!("[[forms.{}]].escape must not be empty", f.name));
            }
            if esc.len() > 4 {
                return Err(format!(
                    "[[forms.{}]].escape: at most 4 bytes, got {}",
                    f.name,
                    esc.len()
                ));
            }
        }
        // 变长语义键校验
        if let Some(p) = &f.keys.prefix
            && p != "field"
            && p != "opsize"
            && parse_u64(p).is_none()
        {
            return Err(format!(
                "[[forms.{}]].prefix must be \"field\", \"opsize\" or a byte literal, got '{p}'",
                f.name
            ));
        }
        if let Some(o) = &f.keys.opsize {
            // v12.1：opsize = 操作数序号（宽度由该操作数寄存器推导）；
            // 范围/类型校验在 codegen（vlen_ctx 需指令操作数解析后）。
            let _ = o;
        }
        // rex_w 值域由 `RexW` 枚举在反序列化期强制（未知值 → serde 报错 + 候选列表）
        if f.keys.vex.is_some() && f.keys.evex.is_some() {
            return Err(format!(
                "[[forms.{}]]: `vex` and `evex` are mutually exclusive",
                f.name
            ));
        }
        if let Some(imm) = f.keys.imm
            && imm == 0
        {
            return Err(format!("[[forms.{}]].imm must be > 0", f.name));
        }
    }
    Ok(())
}

fn slot_exists(m: &V12Model, name: &str) -> bool {
    m.operand_slots.iter().any(|s| s.name == name)
}

fn form_exists(m: &V12Model, name: &str) -> bool {
    m.forms.iter().any(|f| f.name == name)
}

// ──────────────────── [[instructions]] ────────────────────

/// 逐条收集：角色唯一性 + 重名（整表层）各报一次，然后**每条指令**各报一条。
///
/// 重名诊断由 `DeclIndex` 自动附注"同名声明也出现在 行:列"（S1 新增能力）。
fn validate_instructions_all(m: &V12Model, idx: &DeclIndex, d: &mut Diags) {
    // 角色唯一性（v18 S9）：键是 **(角色, 位宽)**——有宽度语义的角色
    // （`roles = [{ role = "fpr_mov", bits = 32 }]`）可以有多条，靠 `bits` 区分；
    // 无宽度语义的角色（`"gpr_mov"`）仍旧全 ISA 唯一。同一角色一处写 bits、一处不写
    // = 歧义，也拒绝。
    let mut role_owner: std::collections::BTreeMap<Role, Vec<(Option<u16>, &str)>> =
        Default::default();
    for inst in &m.instructions {
        for r in &inst.roles {
            role_owner
                .entry(r.role())
                .or_default()
                .push((r.bits(), inst.name.as_str()));
        }
    }
    for (role, owners) in &role_owner {
        for (i, (bits, name)) in owners.iter().enumerate() {
            for (obits, oname) in &owners[i + 1..] {
                if bits == obits {
                    d.push_anchored(
                        idx,
                        &format!(
                            "[[instructions.{name}]]: 角色 \"{role}\" 与 {oname} 冲突——\
                             同角色同宽度只能有一条声明（不同宽度请写 `bits`）"
                        ),
                    );
                } else if bits.is_none() != obits.is_none() {
                    d.push_anchored(
                        idx,
                        &format!(
                            "[[instructions.{name}]]: 角色 \"{role}\" 与 {oname} 冲突——\
                             同一角色不能一处写 `bits`、一处不写"
                        ),
                    );
                }
            }
        }
    }

    let mut seen = BTreeSet::new();
    for inst in &m.instructions {
        if !seen.insert(inst.name.clone()) {
            // 重复声明：诊断落在**后出现**的那一处，并附注首次声明（索引里的同名位置）。
            d.push_duplicate(
                idx,
                &format!(
                    "[[instructions.{}]]: duplicate instruction name '{}'",
                    inst.name, inst.name
                ),
            );
        }
    }
    for inst in &m.instructions {
        if let Err(msg) = check_instruction(m, inst) {
            // 模板展开出的实例：把错误锚回**模板声明**（`[[templates.X]]`），
            // 而不是一个在源里根本不存在的 `[[instructions.实例名]]`。
            let msg = match &inst.from_template {
                Some(t) => msg
                    .replacen(
                        &format!("[[instructions.{}]]", inst.name),
                        &format!("[[templates.{t}]]"),
                        1,
                    )
                    .replacen("[[instructions]]", &format!("[[templates.{t}]]"), 1),
                None => msg,
            };
            d.push_anchored(idx, &msg);
        }
    }
}

/// 单条指令的全部校验（原 `validate_instructions` 的循环体）。
fn check_instruction(m: &V12Model, inst: &Instruction) -> Result<(), String> {
    if let Some(f) = &inst.form
        && !form_exists(m, f)
    {
        return Err(format!(
            "[[instructions.{}]]: form '{f}' is not declared in [[forms]]",
            inst.name
        ));
    }
    // `reloc` 的取值域由 `validate_relocs` 校验（名字须在 [[reloc]] 表里）
    let asm = inst.asm.clone();
    if asm.trim().is_empty() {
        return Err(format!(
            "[[instructions.{}]]: asm must not be empty",
            inst.name
        ));
    }
    // 操作数声明（asm 占位符内联）：解析 + 槽存在/角色合法/序号连续校验
    let (uses, _) = super::codegen::parse_asm_decl(
        &asm,
        inst.ops.as_deref(),
        &inst.name,
        &m.variant_param_names(),
    )?;
    for op in &uses {
        if !slot_exists(m, &op.slot) {
            return Err(format!(
                "[[instructions.{}]]: operand slot '{}' is not declared in [[operand_slots]] (asm '{}')",
                inst.name, op.slot, asm
            ));
        }
        if let Some(slot) = m.operand_slots.iter().find(|s| s.name == op.slot)
            && let (Some(r), Some(roles)) = (op.role, &slot.roles)
        {
            // 兼容性：InOut 槽支持 in/out/inout（读改写能力的子集）；
            // In 槽仅 in、Out 槽仅 out。
            let ok = roles.iter().any(|x| match x {
                OperandRole::InOut => true,
                OperandRole::In => r == OperandRole::In,
                OperandRole::Out => r == OperandRole::Out,
            });
            if !ok {
                return Err(format!(
                    "[[instructions.{}]]: operand role {:?} not allowed by slot '{}' roles {:?}",
                    inst.name, r, op.slot, roles
                ));
            }
        }
    }
    // 定宽（opcode_field 存在）要求操作数数量 ≤ operand_fields。
    // 键取 form 预设 ⊕ 指令级覆盖（与 codegen 的 `EncKeys::over` 同语义）。
    let preset = inst
        .form
        .as_ref()
        .and_then(|n| m.forms.iter().find(|f| &f.name == n))
        .map(|f| f.keys.clone())
        .unwrap_or_default();
    let enc = inst.enc.over(&preset);
    let is_fixed = enc.opcode_field.is_some();
    if is_fixed {
        let of_len = enc.operand_fields.as_ref().map(|f| f.len()).unwrap_or(0);
        if uses.len() > of_len {
            return Err(format!(
                "[[instructions.{}]]: {} operands exceed operand_fields count {}",
                inst.name,
                uses.len(),
                of_len
            ));
        }
    }
    if let Some(fields) = &inst.fields
        && is_fixed
    {
        for k in fields.keys() {
            if !m.conventions.bitfields.contains_key(k) {
                return Err(format!(
                    "[[instructions.{}]]: fixed field '{k}' is not declared in [conventions.bitfields]",
                    inst.name
                ));
            }
        }
    }
    // 编码信息下限：至少要有一个**编码来源**（`opcode`/`fields`，或任一变长编码键）。
    // 原先只对 `[[families]]` 变体检查；S2c 统一机制后族/模板/指令同一套，故对所有
    // 指令生效——什么都不编码的指令会生成"空编码臂"，是静默错。
    // 注意 `opcode_field`/`operand_fields`/`prefix`/`opsize`/`rex_w` 只是**修饰**：
    // 只有它们（form 预设给的）而没有主编码，仍属缺编码信息。
    let has_enc = inst.opcode.is_some()
        || inst.fields.is_some()
        || enc.opcode_reg.is_some()
        || enc.imm.is_some()
        || enc.escape.is_some()
        || enc.modrm.is_some()
        || enc.modrm_fixed.is_some()
        || enc.vex.is_some()
        || enc.evex.is_some();
    if !has_enc {
        return Err(format!(
            "[[instructions.{}]]: 指令缺少编码信息——至少要给 `opcode`/`fields`，\
             或 `opcode_reg`/`modrm`/`vex`/`evex`/`imm` 之一",
            inst.name
        ));
    }
    Ok(())
}

// ──────────────────────── [[lowering]] ────────────────────────

/// 全部指令名（含 `[[templates]]` 展开出的实例——展开发生在解析期，此处看到的
/// 就是完整指令表）。lowering/pattern/emit 模板行首可引用其任一。
fn declared_names(m: &V12Model) -> BTreeSet<String> {
    m.instructions.iter().map(|i| i.name.clone()).collect()
}

/// 引用名全集 = 指令名 ∪ 各指令声明的 `ref`。lowering/pattern/emit 行首必须命中其一。
fn declared_refs(m: &V12Model) -> BTreeSet<String> {
    let mut out = declared_names(m);
    for i in &m.instructions {
        if let Some(r) = &i.reference {
            out.insert(r.clone());
        }
    }
    out
}

/// `ref` 校验：非空、不与指令名冲突（`ref` 与指令名同池——都是 lowering 行首的引用名）。
fn validate_references(m: &V12Model) -> Result<(), String> {
    let names = declared_names(m);
    for i in &m.instructions {
        let Some(r) = &i.reference else { continue };
        if r.trim().is_empty() {
            return Err(format!("[[instructions.{}]]: ref 不能为空", i.name));
        }
        if names.contains(r) {
            return Err(format!(
                "[[instructions.{}]]: ref '{r}' 与指令名冲突（引用名与指令名同池）",
                i.name
            ));
        }
    }
    Ok(())
}

/// `[[lowering]]` 校验（逐条收集 + 死规则检测）。
///
/// 历史状态：本函数只查 `op`/`insts` 非空（14 行），于是三类写错**静默通过**：
/// 1. 助记符打错 → 直到 codegen 才报，且消息无位置；
/// 2. 占位符打错（`{iconst_lo}` 少 `12`）→ 落 fallback 装成字面量 0，
///    生成能编译但语义错的代码；
/// 3. `when` 属性名打错 → `pred::eval` 对未知属性返回 false，规则**永不命中**，
///    既不报错也不生效（最难查的一类）。
///
/// 现在三类都在编译期拒绝，另加同 op 完全重复规则检测；S1 起**逐条收集**。
fn validate_lowering_all(m: &V12Model, idx: &DeclIndex, d: &mut Diags) {
    let refs = declared_refs(m);
    // `when` 可用属性 = 核心属性 + `[[derive]]` 名（v18 S3f）
    let pred_attrs = m.pred_attr_names();
    // (op, 规范化 when, insts) → 首次出现的下标；用于重复检测
    let mut seen: std::collections::HashMap<(String, String, Vec<String>), usize> =
        std::collections::HashMap::new();
    for (i, l) in m.lowering.iter().enumerate() {
        if l.op.name().trim().is_empty() {
            d.push_anchored(idx, &format!("[[lowering]] #{i}: op must not be empty"));
            continue;
        }
        // 展开后每条规则只有一个 op（`expand_ops` 在解析期完成）；名单写法在这里
        // 只会出现于"解析未展开"的直接构造（测试），故用 name() 取首个即够。
        let path = format!("[[lowering.{}]]", l.op.name());
        if l.insts.is_empty() {
            d.push_anchored(idx, &format!("{path}: insts must not be empty"));
            continue;
        }
        if let Err(msg) = validate_inst_lines(&path, &l.insts, &refs, &[]) {
            d.push_anchored(idx, &msg);
        }
        let when_key = match &l.when {
            None => String::new(),
            Some(v) => match super::pred::parse(v) {
                Err(e) => {
                    d.push_anchored(idx, &format!("{path}.when: {e}"));
                    continue;
                }
                Ok(p) => {
                    let mut attrs = Vec::new();
                    super::pred::attrs_of(&p, &mut attrs);
                    let mut ok = true;
                    for a in &attrs {
                        if !pred_attrs.iter().any(|x| x == a) {
                            d.push_anchored(
                                idx,
                                &format!(
                                    "{path}.when: 未知属性 '{a}'（可用：{}）——未知属性恒为假，规则永不命中",
                                    pred_attrs.join("/")
                                ),
                            );
                            ok = false;
                        }
                    }
                    if !ok {
                        continue;
                    }
                    format!("{p:?}")
                }
            },
        };
        let key = (l.op.name().to_string(), when_key, l.insts.clone());
        if let Some(first) = seen.insert(key, i) {
            d.push_anchored(
                idx,
                &format!("{path}: 与 #{first} 完全重复（同 op、同 when、同 insts）——删掉一条"),
            );
        }
    }
    collect(d, idx, validate_lowering_order(m));
}

/// 死规则检测：按**裁决序**（`V12Model::lowering_by_op`）逐对判断前序规则是否
/// 已覆盖后序规则的全部取值域。
///
/// 意义：排序改为按特异性裁决后，"被前面更宽的规则完全吃掉"就成了纯粹的作者
/// 错误（写了永不生效的规则）。判定域是每属性的闭区间集合（`eq/ne/lt/le/gt/ge/in`
/// 都能归一化），含 `or`/`not` 的规则记为 Opaque 并跳过——保守，宁可漏报也不误报。
fn validate_lowering_order(m: &V12Model) -> Result<(), String> {
    for (op, rules) in m.lowering_by_op() {
        let mut domains: Vec<super::pred::RuleDomain> = Vec::with_capacity(rules.len());
        for r in &rules {
            let pred = match &r.when {
                None => None,
                Some(v) => {
                    Some(super::pred::parse(v).map_err(|e| format!("[[lowering.{op}]]: {e}"))?)
                }
            };
            domains.push(super::pred::domain_of(pred.as_ref()));
        }
        for (j, dj) in domains.iter().enumerate() {
            for di in domains[..j].iter() {
                if super::pred::subsumes(di, dj) {
                    return Err(format!(
                        "[[lowering.{op}]]: 第 {} 条规则是死规则——裁决序里排在它前面的规则\
                         已覆盖它的全部取值域（priority 降 / 谓词叶子数降 / 声明序升）。\
                         删掉它，或给它更高的 priority",
                        j + 1
                    ));
                }
            }
        }
    }
    Ok(())
}

/// 校验一段发射模板行：助记符已声明 + 占位符已知。`extra_known` 是额外允许
/// 的 `{...}` token（pattern 的叶变量，codegen 会改写成 `{N}` 编号操作数）。
fn validate_inst_lines(
    path: &str,
    insts: &[String],
    refs: &BTreeSet<String>,
    extra_known: &[String],
) -> Result<(), String> {
    for t in insts {
        let trimmed = t.trim();
        if trimmed.is_empty() || trimmed.starts_with('#') {
            continue;
        }
        // `{out} = INST …` 的 lhs 仅文档性，取 `=` 右侧
        let rhs = trimmed.split_once('=').map_or(trimmed, |(_, r)| r.trim());
        let head = rhs.split_whitespace().next().unwrap_or("");
        if !head.starts_with('@') && !refs.contains(head) {
            return Err(format!(
                "{path}: insts 引用了未声明的指令/别名 '{head}'（行: {trimmed}）"
            ));
        }
        for tok in placeholder_tokens(trimmed) {
            let known = crate::v12::codegen::placeholder::is_known(&tok)
                || extra_known.iter().any(|k| k == &tok);
            if !known {
                return Err(format!("{path}: 未知占位符 '{tok}'（行: {trimmed}）"));
            }
        }
    }
    Ok(())
}

// ──────────────────────── [[pattern]] ────────────────────────

/// `[[pattern]]` 校验：匹配树解析 / Opcode 合法性（禁 payload 与特判 op）/
/// 叶变量唯一 / insts 模板 / when 属性名。
fn validate_patterns(m: &V12Model) -> Result<(), String> {
    let refs = declared_refs(m);
    for (i, p) in m.pattern.iter().enumerate() {
        let path = format!("[[pattern]] #{i}");
        let tree =
            super::match_tree::parse(&p.r#match).map_err(|e| format!("{path}.match: {e}"))?;

        // Opcode 合法性：仅单元变体可作匹配节点——Fcmp/Icmp 带 payload（运行期
        // 按完整 Opcode 值比较，无法结构匹配）；Copy/Nop 在 forward 循环 pattern
        // 分发之前就被 continue 特判。未知名交给 codegen `Opcode::#name` 报错。
        let mut ops = Vec::new();
        super::match_tree::op_names(&tree, &mut ops);
        for op in &ops {
            if matches!(op.as_str(), "Fcmp" | "Icmp" | "Copy" | "Nop") {
                return Err(format!(
                    "{path}.match: Op '{op}' 不能作模式节点（Fcmp/Icmp 带 payload，\
                     Copy/Nop 被 forward 循环特判）"
                ));
            }
        }

        // 叶变量：非空 + 唯一（重复变量会让 {名字}→{N} 改写歧义）。
        let mut vars = Vec::new();
        super::match_tree::leaf_vars(&tree, &mut vars);
        if vars.is_empty() {
            return Err(format!("{path}.match: 匹配树没有叶变量（纯常量树无意义）"));
        }
        let mut seen: std::collections::HashSet<&str> = std::collections::HashSet::new();
        for v in &vars {
            if !seen.insert(v) {
                return Err(format!("{path}.match: 叶变量 '{v}' 重复出现"));
            }
        }

        // insts 非空 + 助记符 declared + 占位符已知（叶变量视为 {N} 改写后的编号操作数）。
        if p.insts.is_empty() {
            return Err(format!("{path}: insts must not be empty"));
        }
        let extra: Vec<String> = vars.iter().map(|v| format!("{{{v}}}")).collect();
        validate_inst_lines(&path, &p.insts, &refs, &extra)?;

        // when 属性名 ∈ 核心属性 ∪ [[derive]]（与 lowering 一致——未知属性恒假，模式永不命中）。
        if let Some(w) = &p.when {
            let pred = super::pred::parse(w).map_err(|e| format!("{path}.when: {e}"))?;
            let mut attrs = Vec::new();
            super::pred::attrs_of(&pred, &mut attrs);
            let known = m.pred_attr_names();
            for a in &attrs {
                if !known.iter().any(|x| x == a) {
                    return Err(format!(
                        "{path}.when: 未知属性 '{a}'（可用：{}）",
                        known.join("/")
                    ));
                }
            }
        }
    }

    // 死模式检测（v18 S5c）：**匹配树结构相同**的两个模式，若裁决序里靠前的那个
    // `when` 覆盖靠后的那个，后者永远轮不到（写了既不报错也不生效）。
    // 只在结构相同时判定——不同树之间的覆盖关系不做推断（保守：宁可漏报不误报）。
    let order = m.pattern_order()?;
    let mut trees: Vec<super::match_tree::MatchNode> = Vec::with_capacity(order.len());
    let mut domains: Vec<super::pred::RuleDomain> = Vec::with_capacity(order.len());
    for &i in &order {
        let p = &m.pattern[i];
        trees.push(super::match_tree::parse(&p.r#match)?);
        let pred = match &p.when {
            None => None,
            Some(v) => Some(super::pred::parse(v)?),
        };
        domains.push(super::pred::domain_of(pred.as_ref()));
    }
    for (j, &pj) in order.iter().enumerate() {
        for (di, &pi) in order[..j].iter().enumerate() {
            if trees[di] == trees[j] && super::pred::subsumes(&domains[di], &domains[j]) {
                return Err(format!(
                    "[[pattern]] #{pj} 是死模式——裁决序里靠前的 [[pattern]] #{pi}                      （同一匹配树，priority 降 / Op 节点数降 / when 叶子数降 / 声明序升）                     已覆盖它的全部取值域。删掉它，或给它更高的 priority"
                ));
            }
        }
    }
    Ok(())
}

/// 抽出模板里的 `{…}` token（含花括号）。
fn placeholder_tokens(line: &str) -> Vec<String> {
    let mut out = Vec::new();
    let bytes = line.as_bytes();
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'{'
            && let Some(end) = line[i..].find('}')
        {
            out.push(line[i..i + end + 1].to_string());
            i += end + 1;
            continue;
        }
        i += 1;
    }
    out
}

// ───────────────────────── [abi] ─────────────────────────

fn validate_abi(m: &V12Model) -> Result<(), String> {
    let Some(abi) = &m.abi else {
        return Ok(());
    };
    // arg_slot / arg_class.strategy 的值域由枚举在反序列化期强制

    // [abi.stack_args].shadow_bytes：>0 且 **栈槽单位** 的倍数（元数据派生：x86 = 8 字节槽；
    // 1 字节寄存器 ISA 的槽是 1 字节——历史实现写死"8 的倍数"）。
    if let Some(shadow) = abi.stack_args.as_ref().and_then(|s| s.shadow_bytes) {
        if shadow == 0 {
            return Err(format!(
                "[abi.stack_args].shadow_bytes must be > 0, got {shadow}"
            ));
        }
        let unit = m.slot_bytes()? as u32;
        if unit > 1 && shadow % unit != 0 {
            return Err(format!(
                "[abi.stack_args].shadow_bytes must be a positive multiple of the stack slot unit \
                 ({unit} bytes), got {shadow}"
            ));
        }
    }
    let mut seen = std::collections::HashSet::new();
    for ac in &abi.arg_class {
        if !seen.insert(ac.class) {
            return Err(format!(
                "[abi.arg_class]: duplicate class '{}'",
                ac.class.name()
            ));
        }
        // regs 为空仅当有传参策略（by-ref 等按引用策略不占用寄存器）。
        if ac.regs.is_empty() && ac.strategy.is_none() {
            return Err(format!(
                "[abi.arg_class.{}]: regs must not be empty (or declare a strategy)",
                ac.class.name()
            ));
        }
    }
    // [abi.frame]：sp 必填且非空；alloc/free 指令名非空。
    if let Some(f) = &abi.frame
        && f.sp.trim().is_empty()
    {
        return Err("[abi.frame].sp must not be empty".into());
    }
    Ok(())
}

// ───────────────────────── [spill.*] ─────────────────────────

/// `[spill.*]` 校验（S1：引用名 + `{N}` 占位符 + 基址寄存器名）。
fn validate_spill_all(m: &V12Model, idx: &DeclIndex, d: &mut Diags) {
    let refs = declared_refs(m);
    for (name, s) in &m.spill {
        for (kind, tpl) in [("load", &s.load), ("store", &s.store)] {
            let path = format!("[spill.{name}].{kind}");
            if tpl.trim().is_empty() {
                d.push_anchored(idx, &format!("{path} must not be empty"));
                continue;
            }
            let first = tpl.split_whitespace().next().unwrap_or("");
            if !refs.contains(first) {
                d.push_anchored(
                    idx,
                    &format!("{path}: 未知指令引用 '{first}'（须是已声明指令名或某条指令的 ref）"),
                );
            }
            for ph in placeholder_tokens(tpl) {
                let inner = ph.trim_matches(|c| c == '{' || c == '}');
                if inner.parse::<usize>().is_err() {
                    d.push_anchored(
                        idx,
                        &format!(
                            "{path}: 未知占位符 '{ph}'——spill 模板只接受编号操作数 {{N}}\
                             （{{0}} = 寄存器、{{1}} = 帧偏移）"
                        ),
                    );
                }
            }
        }
        if let Some(base) = &s.base {
            let mut declared: BTreeSet<String> = BTreeSet::new();
            for (rc, g) in &m.reg {
                if let Ok(names) = super::shared::group_names(g) {
                    declared.extend(names);
                }
                let _ = rc;
            }
            if !declared.contains(base.as_str()) {
                d.push_anchored(
                    idx,
                    &format!(
                        "[spill.{name}].base: 物理寄存器名 \"{base}\" 不在任何已声明 [reg.*] 组内"
                    ),
                );
            }
        }
    }
}
