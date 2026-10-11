//! **谱写法迁移**：把旧写法（`ops` 键 + `bind` 键 + `{名字}` 纯引用模板）改成
//! **内联声明**（`{名字:槽[:角色]}` / `{@字段:槽[:角色]}`，见 [`crate::dsl::inline_ops`]）。
//!
//! 这是**纯文本**变换：只动 `[inst.*]` 块里的 `ops`/`bind`/`asm` 三行，其余逐字节保留
//! （注释、缩进、节序、行尾都不碰）。硬门在测试里：迁移前后
//! `expand_file` 的展开结果必须**逐字节相同**（`tests/v22_migration.rs`），
//! 因此"迁移只改写法、不改语义"是被证明的，而不是被相信的。
//!
//! 实测（2026-10）：三份发行谱的 `ops`/`bind` **全部**落在 `[inst.*]` 块内，
//! `[family]` 的行内表里一处也没有 ⇒ 行级变换足够，不必重排 TOML。

/// 一个操作数的声明（= 旧 `ops` 的一项）。
#[derive(Debug, Clone)]
struct OpDecl {
    name: String,
    slot: String,
    role: Option<String>,
    /// 显式绑定到的字段名（旧 `bind`；方向在解析期已归一成"操作数 → 字段"）。
    field: Option<String>,
    /// 旧 `bind` 的值写成 `[名字]` ⇒ 该操作数按**内存形态**走（新语法里写 `@[字段]`）。
    mem_form: bool,
}

impl OpDecl {
    /// 声明形态：`名字:槽[:角色]`；顺序敏感时追加 `#序号`（= 旧 `ops` 里的下标）。
    fn decl(&self, idx: usize, with_idx: bool) -> String {
        let body = match &self.role {
            Some(r) => format!("{}:{}:{}", self.name, self.slot, r),
            None => format!("{}:{}", self.name, self.slot),
        };
        if with_idx {
            format!("{body}#{idx}")
        } else {
            body
        }
    }

    /// 绑定形态：`@字段:槽[:角色]`；内存形态 `@[字段]:槽[:角色]`（与旧 `[名字]` 同形）。
    fn bind(&self) -> String {
        let field = if self.mem_form {
            format!("[{}]", self.field.as_deref().unwrap_or(""))
        } else {
            self.field.as_deref().unwrap_or("").to_string()
        };
        match &self.role {
            Some(r) => format!("@{field}:{}:{r}", self.slot),
            None => format!("@{field}:{}", self.slot),
        }
    }
}

/// `ops = ["dst:g:out", …]` 的内容 → 声明表（不含绑定）。
fn parse_ops(line: &str) -> Result<Vec<OpDecl>, String> {
    let body = line
        .split_once('[')
        .and_then(|(_, r)| r.rsplit_once(']'))
        .map(|(b, _)| b)
        .ok_or_else(|| format!("无法解析 ops 行：{line}"))?;
    let mut out = Vec::new();
    for item in body.split(',') {
        let s = item.trim().trim_matches('"').trim();
        if s.is_empty() {
            continue;
        }
        let mut it = s.split(':');
        let name = it.next().unwrap_or("").trim().to_string();
        let slot = it.next().unwrap_or("").trim().to_string();
        let role = it.next().map(|r| r.trim().to_string());
        if name.is_empty() || slot.is_empty() {
            return Err(format!("ops 项 '{s}' 形如 名字:槽[:角色]"));
        }
        out.push(OpDecl {
            name,
            slot,
            role,
            field: None,
            mem_form: false,
        });
    }
    if out.is_empty() {
        return Err("ops 为空数组".into());
    }
    Ok(out)
}

/// `bind = { reg = "dst", rm = "[mem]" }` → (键, 值) 对。
fn parse_bind(line: &str) -> Result<Vec<(String, String)>, String> {
    let body = line
        .split_once('{')
        .and_then(|(_, r)| r.rsplit_once('}'))
        .map(|(b, _)| b)
        .ok_or_else(|| format!("无法解析 bind 行：{line}"))?;
    let mut out = Vec::new();
    for item in body.split(',') {
        let Some((k, v)) = item.split_once('=') else {
            continue;
        };
        out.push((k.trim().to_string(), v.trim().trim_matches('"').to_string()));
    }
    Ok(out)
}

/// `asm = "…"` 或 `asm = ["…", "…"]` → 模板列表。
fn parse_asm(line: &str) -> Result<Vec<String>, String> {
    let (_, rhs) = line
        .split_once('=')
        .ok_or_else(|| format!("无法解析 asm 行：{line}"))?;
    let rhs = rhs.trim();
    let mut out = Vec::new();
    if rhs.starts_with('[') {
        let body = rhs
            .strip_prefix('[')
            .and_then(|r| r.rsplit_once(']'))
            .map(|(b, _)| b)
            .ok_or_else(|| format!("asm 列表未闭合：{line}"))?;
        for item in body.split("\",") {
            let s = item.trim().trim_matches('"').trim();
            if !s.is_empty() {
                out.push(s.to_string());
            }
        }
    } else {
        out.push(rhs.trim_matches('"').to_string());
    }
    if out.is_empty() {
        return Err("asm 为空".into());
    }
    Ok(out)
}

/// 模板里操作数名按**打印序**出现的顺序（只认已声明的名字）。
///
/// 与 `ops` 序不同 ⇒ 该指令顺序敏感：内联声明必须显式写 `#序号`（否则"出现序 = 编码序"
/// 会静默改编码）。
fn appearance_order(tpl: &str, decls: &[OpDecl]) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    let mut i = 0usize;
    while let Some(rel) = tpl[i..].find('{') {
        let start = i + rel + 1;
        let Some(end) = tpl[start..].find('}') else {
            break;
        };
        let inner = tpl[start..start + end].trim();
        if !inner.contains(':')
            && !inner.starts_with('@')
            && decls.iter().any(|d| d.name == inner)
            && !out.iter().any(|n| n == inner)
        {
            out.push(inner.to_string());
        }
        i = start + end + 1;
    }
    out
}

/// 一条模板：把 `{名字}` 换成 `{@字段:槽}{名字:槽}`（无绑定的只换声明）。
fn rewrite_template(
    tpl: &str,
    decls: &[OpDecl],
    inst: &str,
    with_idx: bool,
) -> Result<String, String> {
    let mut out = String::with_capacity(tpl.len() + 32 * decls.len());
    let mut i = 0usize;
    while i < tpl.len() {
        if !tpl[i..].starts_with('{') {
            let ch = tpl[i..].chars().next().expect("UTF-8 边界");
            out.push(ch);
            i += ch.len_utf8();
            continue;
        }
        let rel = tpl[i..]
            .find('}')
            .ok_or_else(|| format!("[inst.{inst}] asm: 未闭合的 '{{'"))?;
        let inner = tpl[i + 1..i + rel].trim();
        if inner.contains(':') || inner.starts_with('@') {
            return Err(format!(
                "[inst.{inst}] asm: 占位符 '{{{inner}}}' 已是新写法或带修饰——本迁移只接受纯 `{{名字}}`\
                 （v21 的 `{{名字:修饰}}` 需先人工处理）"
            ));
        }
        let pos = decls
            .iter()
            .position(|d| d.name == inner)
            .ok_or_else(|| format!("[inst.{inst}] asm: 占位符 '{{{inner}}}' 不在 ops 里"))?;
        let d = &decls[pos];
        if d.field.is_some() {
            out.push_str(&format!("{{{}}}", d.bind()));
        }
        out.push_str(&format!("{{{}}}", d.decl(pos, with_idx)));
        i += rel + 1;
    }
    Ok(out)
}

/// 行首是否恰好是这个键（`ops` 不能匹配上 `opsize`；`asm` 不能匹配上 `asm_xxx`）。
fn is_key(line: &str, key: &str) -> bool {
    let t = line.trim_start();
    match t.strip_prefix(key) {
        Some(rest) => rest.starts_with('=') || rest.starts_with(' ') || rest.starts_with('\t'),
        None => false,
    }
}

/// 值部分是否闭合（`[...]`/`{...}`/引号成对）——TOML 允许数组与内联表跨行。
fn balanced(line: &str) -> bool {
    let Some((_, rhs)) = line.split_once('=') else {
        return true;
    };
    let (mut sq, mut cu, mut in_str, mut prev) = (0i32, 0i32, false, '\0');
    for c in rhs.chars() {
        match c {
            '"' if prev != '\\' => in_str = !in_str,
            '[' if !in_str => sq += 1,
            ']' if !in_str => sq -= 1,
            '{' if !in_str => cu += 1,
            '}' if !in_str => cu -= 1,
            _ => {}
        }
        prev = c;
    }
    sq <= 0 && cu <= 0 && !in_str
}

/// 物理行 → **逻辑行**：值未闭合就与后续行合并（`ops`/`bind`/`asm` 都可能跨行）。
/// 返回 `(起始物理行号, 合并后文本, 消费的物理行数)`。
fn logical_lines(block: &[String]) -> Vec<(usize, String, usize)> {
    let mut out = Vec::new();
    let mut i = 0usize;
    while i < block.len() {
        let start = i;
        let mut text = block[i].clone();
        while !balanced(&text) && i + 1 < block.len() {
            i += 1;
            text.push(' ');
            text.push_str(&block[i]);
        }
        out.push((start, text, i - start + 1));
        i += 1;
    }
    out
}

/// 迁移一份谱的文本。返回新文本（行尾/缩进/注释原样保留）。
pub fn migrate_spec_text(src: &str) -> Result<String, String> {
    // 变长 ISA（`prefix_scan`）的 `bind` 是"字段 → 操作数"，定宽 ISA 是"操作数 → 字段"。
    let prefix_scan = src
        .lines()
        .skip_while(|l| !l.trim_start().starts_with("[encoding]"))
        .take_while(|l| {
            !l.trim_start().starts_with('[') || l.trim_start().starts_with("[encoding]")
        })
        .any(|l| l.contains("prefix_scan"));

    let mut out: Vec<String> = Vec::new();
    let mut block: Vec<String> = Vec::new();
    let mut in_inst = false;
    let flush =
        |block: &mut Vec<String>, out: &mut Vec<String>, prefix_scan: bool| -> Result<(), String> {
            if block.is_empty() {
                return Ok(());
            }
            let inst = block[0]
                .trim()
                .trim_start_matches('[')
                .trim_end_matches(']')
                .trim()
                .to_string();
            let logical = logical_lines(block);
            let find = |key: &str| logical.iter().position(|(_, text, _)| is_key(text, key));
            let ops_line = find("ops");
            let bind_line = find("bind");
            let asm_line = find("asm");
            let (Some(oi), Some(ai)) = (ops_line, asm_line) else {
                // 无操作数指令（ret/nop）：原样保留。
                out.append(block);
                return Ok(());
            };
            let mut decls = parse_ops(&logical[oi].1)?;
            if let Some(bi) = bind_line {
                for (k, v) in parse_bind(&logical[bi].1)? {
                    // x86：值是操作数名（`[mem]` 表示该操作数走**内存形态**）；定宽：键是操作数名。
                    let (op, field, mem_form) = if prefix_scan {
                        (
                            v.trim_matches(['[', ']']).to_string(),
                            k,
                            v.starts_with('['),
                        )
                    } else {
                        (k, v, false)
                    };
                    if let Some(d) = decls.iter_mut().find(|d| d.name == op) {
                        d.field = Some(field);
                        d.mem_form = mem_form;
                    } else {
                        return Err(format!(
                            "[inst.{inst}] bind 指向的操作数 `{op}` 不在 ops 里（ops: {}）",
                            decls
                                .iter()
                                .map(|d| d.name.as_str())
                                .collect::<Vec<_>>()
                                .join(", ")
                        ));
                    }
                }
            }
            let tpls = parse_asm(&logical[ai].1)?;
            // 顺序标记：**任一**模板的打印序 ≠ `ops` 序就必须全标（一条指令的多个模板打印序
            // 可能不同，只看第一条会漏标 ⇒ 那条模板的"出现序 = 编码序"就不再成立）。
            let ops_order: Vec<String> = decls.iter().map(|d| d.name.clone()).collect();
            let with_idx = tpls
                .iter()
                .any(|t| appearance_order(t, &decls) != ops_order);
            let mut new_tpls = Vec::new();
            for t in &tpls {
                new_tpls.push(rewrite_template(t, &decls, &inst, with_idx)?);
            }
            // 写回：跳过 ops/bind 的**整个逻辑行**（含跨行续行）；asm 的那个逻辑行换成单行新写法。
            let span = |l: Option<usize>| -> (usize, usize) {
                match l {
                    Some(i) => {
                        let (start, _, consumed) = &logical[i];
                        (*start, start + consumed)
                    }
                    None => (usize::MAX, usize::MAX),
                }
            };
            let (ops_lo, ops_hi) = span(ops_line);
            let (bind_lo, bind_hi) = span(bind_line);
            let (asm_lo, asm_hi) = span(asm_line);
            let mut new_block: Vec<String> = Vec::new();
            let mut idx = 0usize;
            while idx < block.len() {
                if (idx >= ops_lo && idx < ops_hi) || (idx >= bind_lo && idx < bind_hi) {
                    idx += 1;
                    continue;
                }
                if idx >= asm_lo && idx < asm_hi {
                    if tpls.len() == 1 {
                        new_block.push(format!("asm = \"{}\"", new_tpls[0]));
                    } else {
                        let items: Vec<String> =
                            new_tpls.iter().map(|t| format!("\"{t}\"")).collect();
                        new_block.push(format!("asm = [{}]", items.join(", ")));
                    }
                    idx = asm_hi;
                    continue;
                }
                new_block.push(block[idx].clone());
                idx += 1;
            }
            out.append(&mut new_block);
            Ok(())
        };

    for line in src.split_inclusive('\n') {
        let trimmed = line.trim_start();
        if trimmed.starts_with('[') {
            // 任何 `[` 开头的行都是边界（含 `[[数组表头]]`——它们也必须结束上一个 inst 块，
            // 否则 `[machine.spill.*]` 的行会被并进 inst 块里被改写）。
            if in_inst {
                flush(&mut block, &mut out, prefix_scan)?;
                block.clear();
                in_inst = false;
            }
            if trimmed.starts_with("[inst.") {
                in_inst = true;
                block.push(line.trim_end_matches(['\r', '\n']).to_string());
                continue;
            }
            out.push(line.trim_end_matches(['\r', '\n']).to_string());
            continue;
        }
        if in_inst {
            block.push(line.trim_end_matches(['\r', '\n']).to_string());
        } else {
            out.push(line.trim_end_matches(['\r', '\n']).to_string());
        }
    }
    if in_inst {
        flush(&mut block, &mut out, prefix_scan)?;
    }
    let mut text = out.join("\n");
    if src.ends_with('\n') {
        text.push('\n');
    }
    if src.contains("\r\n") {
        text = text.replace('\n', "\r\n");
    }
    Ok(text)
}
