//! v21 W4 **指令流段模型**（x86 那一族变长编码）：`form.segments = [{ kind = …, … }, …]`
//! 降级回既有的语义键（[`EncKeys`]）+ 段内位域并入 `form.fields`。
//!
//! 与 W2/W3 同一套路（**降级到既有内部表示**，编码器/生成物一行不改）：
//!
//! - **有序段 = 发射序**（内联表数组；比"名字列表 + 命名子表"少一处重复，
//!   且 `[[forms]]` 的子表路径在 TOML 里有二义性——见执行清单的偏离记录）；
//! - 段 kind 是**闭集**（[`SegKind`]）：写错由 serde 报 unknown variant 并列出闭集；
//! - 每段用 W2 的**同一套字段语法**声明自己的字节布局（`"u1[7]:w"`），
//!   段的位域并进 `form.fields`，因此"位域表 / `opcode_field` / `operand_fields`"
//!   仍由 [`crate::dsl::field_decl`] 一处展开。
//!
//! **本片覆盖**：`prefix` / `escape` / `rex` / `opcode` / `modrm` / `imm`。
//! 其余段（`opcode_reg` / `sib` / `disp` / `vex` / `evex`）**明确报错**而不是静默忽略
//! （fail-closed）：x86 谱要用它们，等下一片。

use super::field_decl::FieldDecl;
use super::model::{
    EncKeys, Form, IsaModel, PrefixKey, RexW, SegmentBytes, SegmentDecl, SegKind,
};

/// 把 `kind = "stream"`（写了 `segments`）的 form 降级回语义键 + `fields`；
/// 并把**指令级结构段**（选项 B）也降级。
pub fn lower_stream_forms(m: &mut IsaModel) -> Result<(), String> {
    let fixed = m.encoding.kind != super::model::EncodingKind::PrefixScan;
    for form in &mut m.forms {
        if form.segments.is_none() {
            continue;
        }
        lower_one(form, fixed)?;
    }
    lower_instr_segments(m)?;
    // v21 W4.3：stream form 的指令用 `match` 给**段字段**赋值 ⇒ 翻译成编码器读的标量
    //（`opcode` → `Instruction.opcode`）。**编码器一行不改**，字节按构造不变——这正是
    // "降级到既有内部表示"这条主线在段模型上的延续。
    lower_stream_instructions(m);
    Ok(())
}

/// v21 W4.3b **指令级结构段**（选项 B）：不依赖 form 的指令也能声明 `escape`/`prefix`/`modrm`。
///
/// 只接受**结构段**（段内不给字段）：`opcode` 的值走 [`Instruction::opcode`] 标量，
/// 操作数到 reg/rm 的绑定走 `bind`，常量走 `match`——需要**布局字段**的场合请用 form。
fn lower_instr_segments(m: &mut IsaModel) -> Result<(), String> {
    for inst in &mut m.instructions {
        let Some(segs) = inst.segments.clone() else {
            continue;
        };
        if segs.is_empty() {
            return Err(format!(
                "[[instructions.{}]]: `segments` 不能为空（要么不写，要么至少一段）",
                inst.name
            ));
        }
        let mut keys = EncKeys::default();
        for seg in &segs {
            if seg.fields.as_ref().is_some_and(|f| !f.is_empty()) {
                return Err(format!(
                    "[[instructions.{}]]: 指令级段不支持 `fields`（结构段只写 kind + bytes；\
                     需要布局字段请把段放进 form）",
                    inst.name
                ));
            }
            match seg.kind {
                SegKind::Escape => {
                    let raw = match seg.bytes.clone() {
                        Some(SegmentBytes::Raw(v)) => v,
                        _ => {
                            return Err(format!(
                                "[[instructions.{}]].escape: 转义段要写裸字节列表（`bytes = [\"0x0F\"]`）",
                                inst.name
                            ));
                        }
                    };
                    let mut out = Vec::new();
                    for b in raw {
                        out.push(
                            parse_byte(&b)
                                .map_err(|e| format!("[[instructions.{}]].escape: {e}", inst.name))?,
                        );
                    }
                    keys.escape = Some(out);
                }
                SegKind::Prefix => {
                    let k = prefix_key(&format!("[[instructions.{}]]", inst.name), seg)?;
                    if matches!(k, PrefixKey::One(ref s) if s == "field") {
                        return Err(format!(
                            "[[instructions.{}]].prefix: 字段形态的前缀请放进 form（指令级段只做结构）",
                            inst.name
                        ));
                    }
                    keys.prefix = Some(k);
                }
                SegKind::Modrm => {
                    // 结构上表示"这条指令有 ModRM"。vlen 的 ModRM 发射由操作数驱动
                    // （实测：`MOV_R_RM` 的生效键里没有 `modrm`）⇒ 此处不设键。
                }
                other => {
                    return Err(format!(
                        "[[instructions.{}]]: 指令级段只支持 `escape`/`prefix`/`modrm`\
                         （`{}` 需要布局字段，请放进 form）",
                        inst.name,
                        kind_name(other)
                    ));
                }
            }
        }
        // 与 form 同口径：**段键覆盖、既有键兜底**。
        inst.enc = keys.over(&inst.enc);
    }
    Ok(())
}

/// 前缀段 → 经典 `PrefixKey`（三种形态一处实现，form 与指令级共用）。
fn prefix_key(where_: &str, seg: &SegmentDecl) -> Result<PrefixKey, String> {
    if seg.bytes.is_none() && seg.fields.is_some() {
        // ② 字段形态：字节由指令的 `match = { prefix = … }` 给。
        if seg.fields.as_ref().is_some_and(|f| f.is_empty()) {
            return Err(format!("{where_}.prefix: 字段形式的前缀段不能给空的 `fields`"));
        }
        return Ok(PrefixKey::One("field".to_string()));
    }
    if let Some(SegmentBytes::Raw(list)) = seg.bytes.clone() {
        // ③ 有序字节列表：发射序 = 写序。
        if list.is_empty() {
            return Err(format!("{where_}.prefix: 有序字节列表不能为空"));
        }
        let mut out = Vec::new();
        for b in list {
            parse_byte(&b).map_err(|e| format!("{where_}.prefix: {e}"))?;
            out.push(b);
        }
        return Ok(if out.len() == 1 {
            PrefixKey::One(out.remove(0))
        } else {
            PrefixKey::Many(out)
        });
    }
    // ① 字节 → 效果名字典。
    let effects = match seg.bytes.clone() {
        Some(SegmentBytes::Effects(map)) => map,
        None => {
            return Err(format!(
                "{where_}.prefix: 前缀段必须给 `bytes`（字节 → 效果名字典、或有序字节列表）\
                 或 `fields`（字节由字段给）"
            ));
        }
        Some(SegmentBytes::Raw(_)) => unreachable!("裸字节列表已在上面处理"),
    };
    let mut parts = Vec::new();
    for (byte, effect) in effects {
        if !super::model::PrefixEffect::NAMES.contains(&effect.as_str()) {
            return Err(format!(
                "{where_}.prefix: 未知的前缀效果名 `{effect}`（可用：{}）",
                super::model::PrefixEffect::NAMES.join(" / ")
            ));
        }
        parse_byte(&byte).map_err(|e| format!("{where_}.prefix: {e}"))?;
        if effect == "opsize16" {
            parts.push("opsize".to_string());
        } else {
            parts.push(byte);
        }
    }
    Ok(if parts.len() == 1 {
        PrefixKey::One(parts.remove(0))
    } else {
        PrefixKey::Many(parts)
    })
}

/// 段字段值 → 经典标量（目前只做 `opcode`/扩展码；`imm` 值留给后续片）。
fn lower_stream_instructions(m: &mut IsaModel) {
    let stream_forms: Vec<String> = m
        .forms
        .iter()
        .filter(|f| f.segments.is_some())
        .map(|f| f.name.clone())
        .collect();
    for inst in &mut m.instructions {
        // 走段路径的两种指令：**form 是段形态**，或**自己带结构段**（选项 B）。
        let via_form = inst
            .form
            .as_ref()
            .is_some_and(|f| stream_forms.contains(f));
        if !via_form && inst.segments.is_none() {
            continue;
        }
        // **没有 `match` 也要往下走**：`bind`（操作数 → reg/rm）与它无关，
        // 早退会让"只有 segments + bind 的指令"丢掉 `modrm`（实测：MOVZX_R8_MEM 的
        // 生效键退化成 `{escape, opsize}`，而 validate 依然通过）。
        if let Some(mt) = inst.fields.clone() {
            let mut rest = std::collections::BTreeMap::new();
            // 段字段的**常量**值走 `match`（整数），**操作数绑定**走 `bind`（名字）——两者分工：
            //   match = { opcode = 0x8B }            → Instruction.opcode
            //   match = { reg = 2 }                  → Modrm.reg = Ext(2)（x86 的 `/2` 扩展码）
            //   bind  = { reg = "dst", rm = "[mem]" }→ Modrm.reg = Op("dst") / rm = "[mem]"
            let mut ext_reg: Option<u64> = None;
            for (k, v) in mt {
                match k.as_str() {
                    "opcode" => {
                        inst.opcode = Some(v);
                    }
                    "reg" => {
                        // 扩展码（x86 的 `/0`..`/7`）：整数常量。
                        ext_reg = Some(v);
                    }
                    _ => {
                        rest.insert(k, v);
                    }
                }
            }
            if let Some(n) = ext_reg {
                // 与 `bind`（可能已经设了 rm）**合并**，不覆盖另一半。
                let base = inst
                    .enc
                    .modrm
                    .clone()
                    .unwrap_or(super::model::ModrmMap { reg: None, rm: None });
                inst.enc.modrm = Some(super::model::ModrmMap {
                    reg: Some(super::model::ModrmReg::Ext(n)),
                    rm: base.rm,
                });
            }
            inst.fields = Some(rest);
        }
        // **无 form 的指令**（选项 B）拿不到 `field_decl` 里的 vlen `bind` 翻译
        //（那条路要求指令有 form）⇒ 在这里补上同样的 reg/rm 绑定。
        if !via_form && let Some(bind) = inst.bind.clone() {
            let reg = bind
                .get("reg")
                .map(|n| super::model::ModrmReg::Op(n.clone()));
            let rm = bind.get("rm").cloned();
            if reg.is_some() || rm.is_some() {
                let base = inst
                    .enc
                    .modrm
                    .clone()
                    .unwrap_or(super::model::ModrmMap { reg: None, rm: None });
                inst.enc.modrm = Some(super::model::ModrmMap {
                    reg: reg.or(base.reg),
                    rm: rm.or(base.rm),
                });
            }
        }
    }
}

fn lower_one(form: &mut Form, fixed: bool) -> Result<(), String> {
    let segs = form.segments.clone().unwrap_or_default();
    if segs.is_empty() {
        return Err(format!(
            "[[forms.{}]]: `segments` 不能为空（stream 形态至少要一段）",
            form.name
        ));
    }
    let fname = form.name.clone();
    let mut keys = EncKeys::default();
    let mut fields: Vec<FieldDecl> = form.fields.clone().unwrap_or_default();
    let mut seen: Vec<SegKind> = Vec::new();
    let mut has_modrm = false;
    let mut imm_bits: u32 = 0;
    for seg in &segs {
        if let Some(prev) = seen.iter().find(|k| **k == seg.kind) {
            return Err(format!(
                "[[forms.{fname}]]: 段 kind `{}` 出现两次（每段至多一次）",
                kind_name(*prev)
            ));
        }
        seen.push(seg.kind);
        // 段内位域并进 form 的字段表（W2 一处展开：位域表 / opcode_field / operand_fields）。
        if let Some(fs) = &seg.fields {
            fields.extend(fs.iter().cloned());
        }
        match seg.kind {
            SegKind::Prefix => {
                // 三种前缀段：
                // ① `bytes = { "0x66" = "opsize16" }`——**字节是常量 + 效果名**（哪个字节代表什么效果）；
                // ② `fields = ["u8[7:0]:prefix"]`——**字节由字段给**（SSE 的 66/F2/F3/0 逐指令不同，
                //    值写在指令的 `match = { prefix = 0xF2 }` 里）；
                // ③ `bytes = ["0xF2", "0xF0"]`——**有序字节列表**（发射序 = 写序；效果名沿用
                //    全局前缀表，W4.4 才收敛到 form）。XACQUIRE+XACQUIRE 这类多前缀指令必须用它：
                //    字典形态按字节字典序排，会把 `0xF2 0xF0` 排反。
                if seg.bytes.is_none() && seg.fields.is_some() {
                    keys.prefix = Some(PrefixKey::One("field".to_string()));
                    // 段内字段已经并入 `fields`（上面统一处理），指令的 `match.prefix` 继续生效。
                    // 效果名/字节校验留给后续片（那时前缀表按 form 显式化）。
                    if seg.fields.as_ref().is_some_and(|f| f.is_empty()) {
                        return Err(format!(
                            "[[forms.{fname}]].prefix: 字段形式的前缀段不能给空的 `fields`"
                        ));
                    }
                    // 跳过下面的 bytes 分支
                } else if let Some(SegmentBytes::Raw(list)) = seg.bytes.clone() {
                    // ③ 有序字节列表：**发射序 = 写序**（效果名沿用全局前缀表，W4.4 才收敛到 form）。
                    if list.is_empty() {
                        return Err(format!(
                            "[[forms.{fname}]].prefix: 有序字节列表不能为空"
                        ));
                    }
                    let mut out = Vec::new();
                    for b in list {
                        parse_byte(&b).map_err(|e| format!("[[forms.{fname}]].prefix: {e}"))?;
                        out.push(b);
                    }
                    keys.prefix = Some(if out.len() == 1 {
                        PrefixKey::One(out.remove(0))
                    } else {
                        PrefixKey::Many(out)
                    });
                } else {
                let effects = match seg.bytes.clone() {
                    Some(SegmentBytes::Effects(map)) => map,
                    // 裸字节列表在上面的 `else if` 里已经处理；这里不可能到达。
                    Some(SegmentBytes::Raw(_)) => Vec::new().into_iter().collect(),
                    None => {
                        return Err(format!(
                            "[[forms.{fname}]].prefix: 前缀段必须给 `bytes`（字节 → 效果名字典、\
                             或有序字节列表）或 `fields`（字节由字段给）"
                        ));
                    }
                };
                // 效果名是**闭集**（[`PrefixEffect::NAMES`] 是唯一来源）：写错在这里报，
                // 不留给下游按字符串比对（三处硬编码名字集一旦不同步就是静默漏派发）。
                //
                // 降级到经典 `prefix` 键时口径不同：经典键只在 `"opsize"` 处表达语义
                //（`PrefixEffect::Opsize16` ⇔ 经典写法的 `"opsize"`），其余前缀按**字节**写。
                //
                // 已知限制（记入执行清单）：经典校验要求 `"opsize"` **单独出现**，所以
                // "同时有 opsize 与其它前缀"的 form 现在还降不过去；W4 的"前缀表按 form
                // 显式化"（段成为唯一事实源）会取代该限制。
                let mut parts = Vec::new();
                for (byte, effect) in effects {
                    if !super::model::PrefixEffect::NAMES.contains(&effect.as_str()) {
                        return Err(format!(
                            "[[forms.{fname}]].prefix: 未知的前缀效果名 `{effect}`\
                             （可用：{}）",
                            super::model::PrefixEffect::NAMES.join(" / ")
                        ));
                    }
                    parse_byte(&byte).map_err(|e| format!("[[forms.{fname}]].prefix: {e}"))?;
                    if effect == "opsize16" {
                        parts.push("opsize".to_string());
                    } else {
                        parts.push(byte);
                    }
                }
                // 单来源写 `One`（与经典写法/序列化往返一致），多来源写 `Many`。
                keys.prefix = Some(if parts.len() == 1 {
                    PrefixKey::One(parts.remove(0))
                } else {
                    PrefixKey::Many(parts)
                });
                }
            }
            SegKind::Escape => {
                let raw = match seg.bytes.clone() {
                    Some(SegmentBytes::Raw(v)) => v,
                    Some(SegmentBytes::Effects(_)) => {
                        return Err(format!(
                            "[[forms.{fname}]].escape: 转义段要写裸字节列表（`bytes = [\"0x0F\"]`）"
                        ));
                    }
                    None => {
                        return Err(format!(
                            "[[forms.{fname}]].escape: 转义段必须给 `bytes`（裸字节列表）"
                        ));
                    }
                };
                let mut out = Vec::new();
                for b in raw {
                    out.push(parse_byte(&b).map_err(|e| format!("[[forms.{fname}]].escape: {e}"))?);
                }
                keys.escape = Some(out);
            }
            SegKind::Rex => {
                keys.rex = Some("auto".to_string());
                // REX 的高 4 位恒为 0x40——由段 kind **蕴含**，谱里不必再写一遍
                //（设计文档的例子写了 `u4[3:0]=0x4`；无名常量槽不在 W2 支持面内，而
                // "这堆字节是 REX"本来就是结构承载的事实 ⇒ 不再写第二遍）。
                //
                // W 位来源：段里 `w` 字段**带显式默认值** ⇒ 固定（`=0` 不置位 / `=1` 恒置位）；
                // **没有默认值** ⇒ 值由指令的 `match = { w = 0|1 }` 给（经典 `rex_w = "field"`）。
                let w_default = seg.fields.as_ref().and_then(|fs| {
                    fs.iter()
                        .find(|f| f.name.as_deref() == Some("w"))
                        .and_then(|f| f.default)
                });
                let has_w_field = seg
                    .fields
                    .as_ref()
                    .is_some_and(|fs| fs.iter().any(|f| f.name.as_deref() == Some("w")));
                keys.rex_w = Some(match (has_w_field, w_default) {
                    (true, None) => RexW::Field,
                    (_, Some(0)) => RexW::Auto,
                    (_, Some(_)) => RexW::Always,
                    (false, None) => RexW::Auto,
                });
            }
            SegKind::Opcode => {
                // 操作码字段必须叫 `opcode`（值由指令的 `match = { opcode = … }` 给）。
                match form_opcode_field(&fields) {
                    Some(name) => {
                        // **只有定宽 ISA 才写 `opcode_field`**：`validate` 用
                        // `opcode_field.is_some()` 判"这是定宽 ISA"（`is_fixed`），vlen 上写它
                        // 会把校验带进定宽分支。vlen 的操作码值走 `Instruction.opcode`
                        // （由 `lower_stream_instructions` 从 `match` 翻译过来）。
                        if fixed {
                            keys.opcode_field = Some(name);
                        }
                    }
                    None => {
                        return Err(format!(
                            "[[forms.{fname}]].opcode: 段里必须有名为 `opcode` 的字段\
                             （位宽 = 操作码字节数）；它的值由指令的 `match` 给"
                        ));
                    }
                }
            }
            SegKind::Modrm => {
                // **不设 `keys.modrm`**：vlen 编码器是**由操作数驱动**发射 ModRM 的
                //（实测 `MOV_R_RM` 的生效键只有 `{opsize="s0"}`，没有 `modrm`，字节仍正确）。
                // 段在这里的职责是声明**字节布局**（`mod`/`reg`/`rm` 三个字段 ⇒ 进
                // `form.fields`，供 `bind`/`match` 引用与 `--bits` 清点）；谁进 reg、谁进 rm
                // 由指令的 `bind` 给出（见 `field_decl` 的 vlen 分支）。强设一个默认
                // `ModrmMap` 会把"操作数驱动"改成"键驱动"，是**行为改变**。
                has_modrm = true;
            }
            SegKind::OpcodeReg => {
                // `+r` 形式：opcode 字节 = 基值 | (op0 & 7)，REX.B = op0>>3。
                let base = seg.value.ok_or_else(|| {
                    format!(
                        "[[forms.{fname}]].opcode_reg: `+r` 段必须给 `value`（操作码基值，\
                         低 3 位留给寄存器号）"
                    )
                })?;
                keys.opcode_reg = Some(base);
            }
            SegKind::Vex => {
                let spec = seg.vex_spec();
                if spec.map.is_none()
                    && spec.pp.is_none()
                    && spec.w.is_none()
                    && spec.l.is_none()
                {
                    return Err(format!(
                        "[[forms.{fname}]].vex: VEX 段至少要给一个来源键\
                         （`map`/`pp`/`w`/`l`）——空头等于没声明"
                    ));
                }
                keys.vex = Some(spec);
            }
            SegKind::Evex => {
                let spec = seg.vex_spec();
                if spec.map.is_none()
                    && spec.pp.is_none()
                    && spec.w.is_none()
                    && spec.l.is_none()
                {
                    return Err(format!(
                        "[[forms.{fname}]].evex: EVEX 段至少要给一个来源键\
                         （`map`/`pp`/`w`/`l`）——空头等于没声明"
                    ));
                }
                keys.evex = Some(spec);
            }
            SegKind::Sib | SegKind::Disp => {
                // 布局段：**没有**对应的语义键——SIB/位移字节由编码器按内存操作数派生
                //（base/index/scale/disp）。段在这里的价值是"把字节布局写进谱"、
                // 让段内字段进 `form.fields` 参与声明期校验与 `--bits` 清点。
            }
            SegKind::Imm => {
                let fs = seg.fields.as_ref().ok_or_else(|| {
                    format!("[[forms.{fname}]].imm: 立即数段必须给 `fields`（声明宽度）")
                })?;
                for f in fs {
                    imm_bits = imm_bits.max(f.bits());
                }
                keys.imm = Some(imm_bits);
            }
        }
    }
    if !has_modrm {
        // `imm` 之外没有操作数编码段时，不许悄悄留空：stream 形态至少要有 modrm 或 imm。
        let has_operand_seg = segs
            .iter()
            .any(|s| matches!(s.kind, SegKind::Modrm | SegKind::Imm));
        if !has_operand_seg {
            return Err(format!(
                "[[forms.{fname}]]: stream 形态至少要有一段操作数编码（`modrm` 或 `imm`）"
            ));
        }
    }
    form.fields = Some(fields);
    // **段键覆盖、既有键兜底**：form 自己写的语义键（`opsize`/`prefix`/`rex_w`/`escape`…）
    // 必须留下——段只描述"这堆字节怎么排"。整体覆写会把 `opsize = "out"` 这类键冲掉，
    // 后果是解码/编码按错误宽度裁决（实测：`MOV64_RR` 的 `48 89 C1` 被解成 32 位）。
    form.keys = keys.over(&form.keys);
    Ok(())
}

/// form 的字段里名为 `opcode` 者。
fn form_opcode_field(fields: &[FieldDecl]) -> Option<String> {
    fields
        .iter()
        .find(|f| f.name.as_deref() == Some("opcode"))
        .and_then(|f| f.name.clone())
}

fn kind_name(k: SegKind) -> &'static str {
    match k {
        SegKind::Prefix => "prefix",
        SegKind::Escape => "escape",
        SegKind::Rex => "rex",
        SegKind::Opcode => "opcode",
        SegKind::OpcodeReg => "opcode_reg",
        SegKind::Modrm => "modrm",
        SegKind::Sib => "sib",
        SegKind::Disp => "disp",
        SegKind::Imm => "imm",
        SegKind::Vex => "vex",
        SegKind::Evex => "evex",
    }
}

fn parse_byte(s: &str) -> Result<u8, String> {
    let t = s.trim();
    let v = if let Some(hex) = t.strip_prefix("0x").or_else(|| t.strip_prefix("0X")) {
        u16::from_str_radix(hex, 16).map_err(|_| format!("`{s}` 不是合法字节（0x00..0xFF）"))?
    } else {
        t.parse::<u16>()
            .map_err(|_| format!("`{s}` 不是合法字节（0x00..0xFF）"))?
    };
    u8::try_from(v).map_err(|_| format!("`{s}` 超出字节范围（0x00..0xFF）"))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 段写法与"经典语义键"写法必须降级到**同一份** EncKeys（等价性证明）。
    const STREAM: &str = r#"
[meta]
name = "t"
[encoding]
kind = "prefix_scan"
max_len = 15
[reg.gpr64]
count = 16
[operand.g]
kind = "reg"
class = "gpr64"
[[forms]]
name = "S"
segments = [
  { kind = "prefix", bytes = { "0xF0" = "lock" } },
  { kind = "escape", bytes = ["0x0F"] },
  { kind = "rex", fields = ["u1[7]:w", "u1[6]:r", "u1[5]:x", "u1[4]:b"] },
  { kind = "opcode", fields = ["u8[7:0]:opcode"] },
  { kind = "modrm", fields = ["u2[7:6]:mod", "u3[5:3]:reg", "u3[2:0]:rm"] },
]
[[instructions]]
name = "N"
form = "S"
match = { opcode = 0x8B }
ops = ["dst:g:out", "src:g"]
bind = { reg = "dst", rm = "src" }
asm = "n {dst}, {src}"
"#;

    const CLASSIC: &str = r#"
[meta]
name = "t"
[encoding]
kind = "prefix_scan"
max_len = 15
[reg.gpr64]
count = 16
[operand.g]
kind = "reg"
class = "gpr64"
[[forms]]
name = "S"
prefix = "0xF0"
escape = [0x0F]
rex = "auto"
rex_w = "field"
[[instructions]]
name = "N"
form = "S"
modrm = { reg = "dst", rm = "src" }
ops = ["dst:g:out", "src:g"]
asm = "n {dst}, {src}"
"#;

    fn form_of(src: &str) -> Form {
        let m = crate::dsl::parse_and_validate(src).expect("谱合法");
        m.forms.into_iter().find(|f| f.name == "S").expect("form S")
    }

    #[test]
    fn stream_form_lowers_to_the_same_keys_as_classic() {
        let s = form_of(STREAM);
        let c = form_of(CLASSIC);
        assert_eq!(s.keys.prefix, c.keys.prefix, "prefix");
        assert_eq!(s.keys.escape, c.keys.escape, "escape");
        assert_eq!(s.keys.rex, c.keys.rex, "rex");
        // 段里 `w` 字段**无默认值** ⇒ 值来自指令的 `match`（经典 `rex_w = "field"`）。
        assert_eq!(s.keys.rex_w, c.keys.rex_w, "rex_w");
        // vlen ISA：`opcode_field` 是**定宽**键（`validate` 用它判 `is_fixed`），不设；
        // 指令侧的值由 `match = { opcode = … }` 翻译成 `Instruction.opcode`。
        assert!(s.keys.opcode_field.is_none(), "vlen 不该设 opcode_field");
        // vlen 的 ModRM 发射由操作数驱动 ⇒ form 的 keys 里**不该**被塞一个默认 ModrmMap。
        assert!(s.keys.modrm.is_none(), "modrm 段不应强设 keys.modrm");
        // 段内位域并进了 form.fields（于是 W2 的展开照旧）
        let names: Vec<&str> = s
            .fields
            .as_ref()
            .unwrap()
            .iter()
            .filter_map(|f| f.name.as_deref())
            .collect();
        for n in ["w", "r", "x", "b", "opcode", "mod", "reg", "rm"] {
            assert!(names.contains(&n), "缺字段 {n}：{names:?}");
        }
    }

    #[test]
    fn unknown_segment_kind_lists_the_closed_set() {
        let bad = STREAM.replace("kind = \"escape\"", "kind = \"nope\"");
        let err = format!("{:?}", crate::dsl::parse_and_validate(&bad).unwrap_err());
        assert!(err.contains("unknown variant"), "err: {err}");
        assert!(err.contains("prefix"), "闭集应列在消息里：{err}");
    }

    /// v21 W4.3：段字段值 → 经典标量。指令用 `match = { opcode = … }` 给段字段赋值，
    /// 降级把它翻译成编码器读的 `Instruction.opcode`（**编码器一行不改** ⇒ 字节按构造不变）。
    #[test]
    fn match_opcode_translates_to_instruction_opcode_scalar() {
        let m = crate::dsl::parse_and_validate(STREAM).expect("谱合法");
        let inst = m.instructions.iter().find(|i| i.name == "N").expect("指令 N");
        assert_eq!(inst.opcode, Some(0x8B), "match.opcode 应翻译成标量");
        assert!(
            !inst.fields.as_ref().is_some_and(|f| f.contains_key("opcode")),
            "翻译后 match 里不该再留 opcode"
        );
    }
    /// v21 W4.3：变长 ISA 的 `bind` 落到 `modrm`（此前 vlen 完全不读 `bind`，
    /// ModRM 的角色只能靠位置缺省）。
    #[test]
    fn vlen_bind_lands_on_modrm() {
        let m = crate::dsl::parse_and_validate(STREAM).expect("谱合法");
        let inst = m.instructions.iter().find(|i| i.name == "N").expect("指令 N");
        let modrm = inst.enc.modrm.as_ref().expect("bind 应写进 modrm");
        assert_eq!(modrm.reg, Some(crate::dsl::model::ModrmReg::Op("dst".into())));
        assert_eq!(modrm.rm.as_deref(), Some("src"));
    }

    /// v21 W4.3b：**指令级结构段**（选项 B）与经典内联键等价。
    #[test]
    fn instruction_level_segments_match_classic_inline_keys() {
        let stream = r#"
[meta]
name = "t"
[encoding]
kind = "prefix_scan"
max_len = 15
[reg.gpr64]
count = 16
[operand.g]
kind = "reg"
class = "gpr64"
[[instructions]]
name = "N"
segments = [
  { kind = "prefix", bytes = ["0xF2"] },
  { kind = "escape", bytes = ["0x0F"] },
  { kind = "modrm" },
]
opcode = 0x10
match = { reg = 2 }
bind = { rm = "dst" }
ops = ["dst:g:out", "src:g"]
asm = "n {dst}, {src}"
"#;
        let classic = r#"
[meta]
name = "t"
[encoding]
kind = "prefix_scan"
max_len = 15
[reg.gpr64]
count = 16
[operand.g]
kind = "reg"
class = "gpr64"
[[instructions]]
name = "N"
prefix = "0xF2"
escape = [0x0F]
opcode = 0x10
modrm = { reg = 2, rm = "dst" }
ops = ["dst:g:out", "src:g"]
asm = "n {dst}, {src}"
"#;
        let a = crate::dsl::parse_and_validate(stream).expect("段形态合法");
        let b = crate::dsl::parse_and_validate(classic).expect("经典形态合法");
        let (ia, ib) = (&a.instructions[0], &b.instructions[0]);
        assert_eq!(ia.opcode, ib.opcode, "opcode");
        assert_eq!(ia.enc.escape, ib.enc.escape, "escape");
        assert_eq!(ia.enc.prefix, ib.enc.prefix, "prefix");
        assert_eq!(ia.enc.modrm, ib.enc.modrm, "modrm（扩展码 + rm 绑定）");
    }

    /// 指令级段只接受结构段：`vex` 这类需要布局字段的要放进 form。
    #[test]
    fn instruction_level_segment_rejects_non_structural_kinds() {
        let bad = r#"
[meta]
name = "t"
[encoding]
kind = "prefix_scan"
max_len = 15
[reg.gpr64]
count = 16
[operand.g]
kind = "reg"
class = "gpr64"
[[instructions]]
name = "N"
segments = [{ kind = "vex", map = "field" }]
opcode = 0x10
ops = ["dst:g:out"]
asm = "n {dst}"
"#;
        let err = format!("{:?}", crate::dsl::parse_and_validate(bad).unwrap_err());
        assert!(err.contains("只支持"), "err: {err}");
        assert!(err.contains("vex"), "err: {err}");
    }

    #[test]
    fn unknown_prefix_effect_is_rejected_with_the_closed_set() {
        let bad = STREAM.replace("\"0xF0\" = \"lock\"", "\"0xF0\" = \"locked\"");
        let err = format!("{:?}", crate::dsl::parse_and_validate(&bad).unwrap_err());
        assert!(err.contains("locked"), "err: {err}");
        assert!(err.contains("opsize16"), "闭集应列在消息里：{err}");
    }

    /// `opsize16`（闭集名）降级到经典键的 `"opsize"`（经典口径），单独出现时合法。
    #[test]
    fn opsize16_effect_lowers_to_classic_opsize() {
        let stream = STREAM.replace(
            "{ kind = \"prefix\", bytes = { \"0xF0\" = \"lock\" } },",
            "{ kind = \"prefix\", bytes = { \"0x66\" = \"opsize16\" } },",
        );
        let classic = CLASSIC.replace("prefix = \"0xF0\"", "prefix = \"opsize\"");
        let s = form_of(&stream);
        let c = form_of(&classic);
        assert_eq!(s.keys.prefix, c.keys.prefix, "opsize16 ⇔ 经典 \"opsize\"");
    }

    #[test]
    fn vex_segment_without_any_source_key_is_rejected() {
        let bad = STREAM.replace(
            "{ kind = \"opcode\", fields = [\"u8[7:0]:opcode\"] }",
            "{ kind = \"opcode\", fields = [\"u8[7:0]:opcode\"] }, { kind = \"vex\" }",
        );
        let err = format!("{:?}", crate::dsl::parse_and_validate(&bad).unwrap_err());
        assert!(err.contains("vex"), "err: {err}");
        assert!(err.contains("至少要给一个来源键"), "err: {err}");
    }

    #[test]
    fn opcode_reg_segment_needs_value() {
        let bad = STREAM.replace(
            "{ kind = \"opcode\", fields = [\"u8[7:0]:opcode\"] }",
            "{ kind = \"opcode_reg\" }",
        );
        let err = format!("{:?}", crate::dsl::parse_and_validate(&bad).unwrap_err());
        assert!(err.contains("opcode_reg"), "err: {err}");
        assert!(err.contains("value"), "err: {err}");
    }

    /// VEX 段 + `+r` 段 + 布局段（sib/disp）与经典写法等价。
    #[test]
    fn vex_and_opcode_reg_segments_lower_like_classic_keys() {
        let src = r#"
[meta]
name = "t"
[encoding]
kind = "prefix_scan"
max_len = 15
[reg.gpr64]
count = 16
[reg.fpr128]
count = 16
[operand.g]
kind = "reg"
class = "gpr64"
[operand.f]
kind = "reg"
class = "fpr128"
[[forms]]
name = "V"
segments = [
  { kind = "vex", map = "field", pp = "0x0F", w = "0", l = "0" },
  { kind = "opcode", fields = ["u8[7:0]:opcode"] },
  { kind = "modrm", fields = ["u2[7:6]:mod", "u3[5:3]:reg", "u3[2:0]:rm"] },
  { kind = "sib", fields = ["u2[7:6]:scale", "u3[5:3]:index", "u3[2:0]:base"] },
  { kind = "disp", fields = ["u8[7:0]:disp8"] },
]
[[instructions]]
name = "N"
form = "V"
match = { opcode = 0x58 }
ops = ["dst:f:out", "src:f"]
bind = { reg = "dst", rm = "src" }
asm = "n {dst}, {src}"
"#;
        let m = crate::dsl::parse_and_validate(src).expect("谱合法");
        let f = m.forms.iter().find(|f| f.name == "V").unwrap();
        let vex = f.keys.vex.as_ref().expect("vex 键");
        assert_eq!(vex.map.as_deref(), Some("field"));
        assert_eq!(vex.pp.as_deref(), Some("0x0F"));
        assert_eq!(vex.w.as_deref(), Some("0"));
        // 布局段的字段也进了 form.fields（参与校验与 --bits 清点）
        let names: Vec<&str> = f
            .fields
            .as_ref()
            .unwrap()
            .iter()
            .filter_map(|x| x.name.as_deref())
            .collect();
        for n in ["opcode", "mod", "reg", "rm", "scale", "index", "base", "disp8"] {
            assert!(names.contains(&n), "缺字段 {n}：{names:?}");
        }
    }

    #[test]
    fn duplicate_segment_kind_is_rejected() {
        let bad = STREAM.replace(
            "{ kind = \"modrm\", fields = [\"u2[7:6]:mod\", \"u3[5:3]:reg\", \"u3[2:0]:rm\"] }",
            "{ kind = \"modrm\", fields = [\"u3[2:0]:rm\"] }, { kind = \"modrm\" }",
        );
        let err = format!("{:?}", crate::dsl::parse_and_validate(&bad).unwrap_err());
        assert!(err.contains("modrm"), "err: {err}");
        assert!(err.contains("两次"), "err: {err}");
    }

    #[test]
    fn bad_byte_literal_is_reported() {
        let bad = STREAM.replace("bytes = [\"0x0F\"]", "bytes = [\"0x1FF\"]");
        let err = format!("{:?}", crate::dsl::parse_and_validate(&bad).unwrap_err());
        assert!(err.contains("0x1FF"), "err: {err}");
    }
}
