//! **v22：操作数声明与字段绑定从 `asm` 模板里就地抽取**。
//!
//! 作者面只写一处——`asm` 模板本身；`ops` / `bind` / `modrm` 三个键不再需要
//! （实测：`bind` 在两条代码路径上方向相反、`modrm` 全仓 0 处使用——正是
//! "接口太多 + 看不清哪个参数对应哪个字段"的根源）。
//!
//! ## 占位符文法
//!
//! ```text
//! '{' ( '@' 字段名 | 名字 ) ':' 槽 [':' 角色] '}'    # 声明：@ 开头 = 绑定字段，否则 = 操作数
//! '{' 名字 '}'                                       # 引用（须已在同一模板里声明过）
//! ```
//!
//! - **绑定占位符** `{@字段:槽[:角色]}` 作用于**紧随其后的那个操作数声明**，自身不打印；
//!   它的 `槽`/角色与目标操作数**必须一致**（不一致 = 编译期报错，这就是"参数↔字段"的显式校验）。
//! - **顺序**：操作数声明的出现序 = 编码序（与旧 `ops` 数组序同义）。
//! - 本 pass 把每条模板**改写成旧形态**（声明后缀剥掉、绑定占位符删掉，只留 `{名字}`），
//!   并回填 `inst.ops`/`inst.bind` ⇒ 下游（`normalize_named_template`、编码/解码/汇编/降级
//!   生成、`validate`/`lint`/`report`）**一行不改**，生成物因此逐字节不变。
//!
//! ## 与旧写法并存（迁移期）
//!
//! 模板里**没有**声明占位符时走旧路径（`ops` 键 + `{名字}` 引用）；两种写法同时出现 ⇒ 报错。
//! 迁移逐谱进行，全部迁完即删旧键（届时本文件即为唯一入口）。

use std::collections::{BTreeMap, BTreeSet};

use super::model::{AsmList, EncodingKind, Instruction, IsaModel, OperandKind, OperandRole};

/// 占位符的三种形态。
#[derive(Debug, Clone, PartialEq)]
enum Ph {
    /// `{名字:槽[:角色]}`：声明操作数。可带 `#序号`（= 旧 `ops` 里的下标，顺序 = 编码序）。
    Decl {
        name: String,
        slot: String,
        role: OperandRole,
        idx: Option<u32>,
    },
    /// `{@字段:槽[:角色]}`：把**后续**那个操作数绑到 `字段`；
    /// 字段写成 `[字段]`（方括号）= 该操作数按**内存形态**（MemRef）走——与旧 `bind` 的
    /// `rm = "[mem]"` 同形（这条信息**不能**由槽 kind 推出：reg 槽也会这么用）。
    Bind {
        field: String,
        mem: bool,
        slot: String,
        role: OperandRole,
    },
    /// `{名字}`：纯引用（或非操作数的生成器占位符）。
    Ref(String),
}

/// 角色字符串 → 枚举（取值与旧 `ops` 一致）。
fn parse_role(s: &str) -> Option<OperandRole> {
    match s.trim() {
        "in" => Some(OperandRole::In),
        "out" => Some(OperandRole::Out),
        "inout" => Some(OperandRole::InOut),
        _ => None,
    }
}

/// 解析 `名字:槽[:角色][#序号]`（绑定形态的 `名字` 已去掉 `@` 前缀）。
fn split_decl(body: &str) -> Option<(String, String, OperandRole, Option<u32>)> {
    // 末尾 `#<数字>` = 显式编码序（顺序敏感指令用它把"出现序 ≠ 编码序"说出来）。
    let (body, idx) = match body.rsplit_once('#') {
        Some((b, n)) if !n.is_empty() && n.chars().all(|c| c.is_ascii_digit()) => {
            (b.trim(), n.parse::<u32>().ok())
        }
        _ => (body.trim(), None),
    };
    let mut it = body.split(':');
    let name = it.next()?.trim().to_string();
    let slot = it.next()?.trim().to_string();
    let role = match it.next() {
        None => OperandRole::In,
        Some(r) => parse_role(r)?,
    };
    if it.next().is_some() || name.is_empty() || slot.is_empty() {
        return None;
    }
    Some((name, slot, role, idx))
}

/// 扫描一条模板，按出现序取出占位符，并把**重写后**的文本写进 `out`。
fn scan(tpl: &str, ctx: &str, out: &mut String) -> Result<Vec<Ph>, String> {
    let mut phs = Vec::new();
    let mut i = 0usize;
    while i < tpl.len() {
        if !tpl[i..].starts_with('{') {
            let ch = tpl[i..].chars().next().expect("合法 UTF-8 边界");
            out.push(ch);
            i += ch.len_utf8();
            continue;
        }
        let Some(rel) = tpl[i..].find('}') else {
            return Err(format!("{ctx}: 未闭合的 '{{'"));
        };
        let inner = tpl[i + 1..i + rel].trim();
        if let Some(rest) = inner.strip_prefix('@') {
            // 绑定占位符：自身不打印（从模板里删掉）。两种写法：
            //   `@字段:槽[:角色]`      —— 普通绑定
            //   `@[字段]:槽[:角色]`    —— 同上，且该操作数按**内存形态**（MemRef）走
            let rest = rest.trim();
            let (field, mem, tail) = match rest.strip_prefix('[') {
                Some(r) => {
                    let Some((f, tail)) = r.split_once(']') else {
                        return Err(format!(
                            "{ctx}: 绑定占位符 '{{{inner}}}' 的方括号未闭合（`{{@[字段]:槽}}`）"
                        ));
                    };
                    (
                        f.trim().to_string(),
                        true,
                        tail.trim_start_matches(':').trim(),
                    )
                }
                None => {
                    let Some((f, tail)) = rest.split_once(':') else {
                        return Err(format!(
                            "{ctx}: 绑定占位符 '{{{inner}}}' 形如 `{{@字段:槽[:角色]}}`（槽必写）"
                        ));
                    };
                    (f.trim().to_string(), false, tail.trim())
                }
            };
            // 两种写法殊途同归：`tail` 就是 `槽[:角色]`。
            let mut it = tail.split(':');
            let slot = it.next().unwrap_or("").trim().to_string();
            let role = match it.next() {
                None => OperandRole::In,
                Some(r) => parse_role(r).ok_or_else(|| {
                    format!("{ctx}: 绑定占位符 '{{{inner}}}' 的角色 '{r}' 非法（in/out/inout）")
                })?,
            };
            if it.next().is_some() || slot.is_empty() || field.is_empty() {
                return Err(format!(
                    "{ctx}: 绑定占位符 '{{{inner}}}' 形如 `{{@字段:槽[:角色]}}`（槽必写，角色取 in/out/inout）"
                ));
            }
            phs.push(Ph::Bind {
                field,
                mem,
                slot,
                role,
            });
        } else if inner.contains(':') {
            let (name, slot, role, idx) = split_decl(inner).ok_or_else(|| {
                format!(
                    "{ctx}: 声明占位符 '{{{inner}}}' 形如 `{{名字:槽[:角色][#序号]}}`（角色取 in/out/inout）"
                )
            })?;
            out.push_str(&format!("{{{name}}}"));
            phs.push(Ph::Decl {
                name,
                slot,
                role,
                idx,
            });
        } else {
            out.push_str(&format!("{{{inner}}}"));
            phs.push(Ph::Ref(inner.to_string()));
        }
        i += rel + 1;
    }
    Ok(phs)
}

/// 一份模板的操作数声明（含绑定字段与"内存形态"位）。
type Decl = (
    String,
    String,
    OperandRole,
    Option<String>,
    bool,
    Option<u32>,
);

/// 一条指令的抽取结果：声明表 + 重写后的模板表。
struct Extraction {
    decls: Vec<Decl>,
    asm: Vec<String>,
}

/// 从**所有**模板里抽出同一份声明表（模板之间必须完全一致）。
///
/// 返回 `None` = 本指令没有声明占位符（无操作数指令，或仍在用旧 `ops` 写法）。
fn extract_one(inst: &Instruction) -> Result<Option<Extraction>, String> {
    let ctx = format!("[inst.{}] asm", inst.name);
    let tpls = inst.asm.all();
    let mut decls: Option<Vec<Decl>> = None;
    let mut rewritten: Vec<String> = Vec::new();
    for tpl in tpls {
        let mut out = String::with_capacity(tpl.len());
        let phs = scan(tpl, &ctx, &mut out)?;
        let mut cur: Vec<Decl> = Vec::new();
        let mut pending: Option<(String, String, OperandRole, bool)> = None;
        for ph in &phs {
            match ph {
                Ph::Bind {
                    field,
                    mem,
                    slot,
                    role,
                } => {
                    if pending.is_some() {
                        return Err(format!(
                            "{ctx}: 连续两个绑定占位符（`{{@{field}:{slot}}}` 前面那个还没找到操作数）"
                        ));
                    }
                    pending = Some((field.clone(), slot.clone(), *role, *mem));
                }
                Ph::Decl {
                    name,
                    slot,
                    role,
                    idx,
                } => {
                    if cur.iter().any(|d| d.0 == *name) {
                        return Err(format!("{ctx}: 操作数名 '{name}' 重复声明"));
                    }
                    let (bound, mem) = match pending.take() {
                        None => (None, false),
                        Some((field, bslot, brole, bmem)) => {
                            if bslot != *slot {
                                return Err(format!(
                                    "{ctx}: `{{@{field}:{bslot}}}` 指向的操作数 `{name}` 的槽是 `{slot}`——槽必须一致"
                                ));
                            }
                            if brole != *role {
                                return Err(format!(
                                    "{ctx}: `{{@{field}:{bslot}}}` 的角色与操作数 `{name}` 不一致——角色必须一致"
                                ));
                            }
                            // **允许多个操作数绑到同一个字段**：arm64 的
                            // `bind = { arrd = "vq", arrs = "vq", arrs2 = "vq" }` 就是这种
                            // （三个 `varr` 操作数各自写进同一个 `vq` 位）；旧路径
                            // `bind_operands` 同样产出 `["vq","vq","vq"]`，所以这里不能判重。
                            (Some(field.clone()), bmem)
                        }
                    };
                    cur.push((name.clone(), slot.clone(), *role, bound, mem, *idx));
                }
                Ph::Ref(_) => {}
            }
        }
        if let Some((field, slot, _, _)) = pending {
            return Err(format!(
                "{ctx}: 绑定占位符 `{{@{field}:{slot}}}` 后面没有操作数声明——绑定必须紧挨着它的操作数"
            ));
        }
        // **顺序标记**（`#序号`）：内联后"出现序 = 编码序"，但实测 100 条指令的出现序 ≠ 旧 `ops` 序
        // （75 条原本靠位置回退才编对）⇒ 顺序敏感时用 `#序号` 把编码序**说出来**。
        // 一条指令内要么都带、要么都不带；带了就按序号排序，并要求序号正好是 0..n-1（不重不漏）。
        if cur.iter().any(|d| d.5.is_some()) {
            if cur.iter().any(|d| d.5.is_none()) {
                return Err(format!(
                    "{ctx}: 声明占位符要么都写 `#序号`、要么都不写——部分标注无法确定编码序"
                ));
            }
            cur.sort_by_key(|d| d.5.expect("都带序号"));
            for (i, d) in cur.iter().enumerate() {
                if d.5 != Some(i as u32) {
                    return Err(format!(
                        "{ctx}: 操作数 `{}` 的 `#序号` 必须是 0..{} 且不重复（顺序 = 编码序）",
                        d.0,
                        cur.len().saturating_sub(1)
                    ));
                }
            }
        }
        if cur.is_empty() {
            // 本模板没有声明：交给旧路径处理（旧 `ops` + 引用，或纯生成器占位符）。
            rewritten.push(tpl.clone());
            continue;
        }
        match &mut decls {
            None => decls = Some(cur),
            Some(d) => {
                // 同一指令的多个模板是同一声明表的**子序列**（模板可以只打印一部分操作数，
                // 旧写法共享一份 `ops` 时就是这样）：共享名字的 槽/角色/绑定 必须一致，
                // 新名字按它在本模板里的相邻关系插进汇总序。
                for (i, (name, slot, role, field, mem, idx)) in cur.iter().enumerate() {
                    if let Some(pos) = d.iter().position(|x| x.0 == *name) {
                        let (_, s0, r0, f0, m0, i0) = &d[pos];
                        if s0 != slot || r0 != role || f0 != field || m0 != mem || i0 != idx {
                            return Err(format!(
                                "{ctx}: 操作数 `{name}` 在不同 asm 模板里的声明不一致\
                                 （槽/角色/绑定/内存形态/序号必须一致）"
                            ));
                        }
                    } else {
                        let at = cur[..i]
                            .iter()
                            .rev()
                            .find_map(|(p, ..)| d.iter().position(|x| x.0 == *p))
                            .map(|p| p + 1)
                            .unwrap_or(0);
                        d.insert(
                            at,
                            (name.clone(), slot.clone(), *role, field.clone(), *mem, *idx),
                        );
                    }
                }
                // 汇总序必须与本模板的相对序一致（顺序 = 编码序）。
                let mut last: Option<usize> = None;
                for (name, ..) in &cur {
                    let pos = d.iter().position(|x| x.0 == *name).expect("刚补齐");
                    if last.is_some_and(|l| pos < l) {
                        return Err(format!(
                            "{ctx}: 操作数 `{name}` 在各 asm 模板里的相对顺序不一致（顺序 = 编码序）"
                        ));
                    }
                    last = Some(pos);
                }
            }
        }
        rewritten.push(out);
    }
    Ok(decls.map(|decls| Extraction {
        decls,
        asm: rewritten,
    }))
}

/// 一条指令的抽取计划（先只读地算，再统一写回，避开对 `m` 的重复借用）。
struct Plan {
    idx: usize,
    ops: Vec<String>,
    asm: Vec<String>,
    bind: BTreeMap<String, String>,
}

/// **入口**：把所有指令的 `asm` 内联声明抽成 `ops`/`bind`（等价旧内部形态）。
///
/// 旧写法（`ops` 键 + 纯引用模板）原样保留；两种写法同现 ⇒ 报错。
pub fn extract_inline_ops(m: &mut IsaModel) -> Result<(), String> {
    // 绑定方向按**消费方的读取方向**填（迁移期的桥，见文件头）：
    // - vlen（`prefix_scan`）/ 段形态：消费方读 `字段 → 操作数`
    // - 定宽：消费方读 `操作数 → 字段`（`field_decl::bind_operands`）
    let vlen = m.encoding.kind == EncodingKind::PrefixScan;
    // "`:` 后面是**槽名**还是 v21 的**占位修饰**（affix）"的判据：槽名以 `[operand.<名>]` 为准
    // （affix 是任意词缀，不会恰好是已声明的槽名）。于是旧的 `ops` 写法与 `{name:affix}`
    // 修饰都不受影响；迁移完成后 `ops`/affix 一并退场，这条判据也随之消失。
    let slot_names: BTreeSet<String> = m.operand_slots.iter().map(|s| s.name.clone()).collect();
    // 内存槽名（用于把"这条操作数是内存形态"补回旧拼法 `[mem]`，见下面的桥）。
    let mem_slots: BTreeSet<String> = m
        .operand_slots
        .iter()
        .filter(|s| s.kind == OperandKind::Mem)
        .map(|s| s.name.clone())
        .collect();
    let streamed_form: Vec<String> = m
        .forms
        .iter()
        .filter(|f| f.segments.is_some())
        .map(|f| f.name.clone())
        .collect();

    let mut planned: Vec<Plan> = Vec::new();
    for (i, inst) in m.instructions.iter().enumerate() {
        let ctx = format!("[inst.{}] asm", inst.name);
        if inst.ops.is_some() {
            // 旧写法：模板只许引用（`{名字}`）与 affix 修饰；真声明/绑定 ⇒ 报错（不许两套并存）。
            for tpl in inst.asm.all() {
                let mut sink = String::new();
                let phs = scan(tpl, &ctx, &mut sink)?;
                let conflict = phs.iter().any(|p| match p {
                    Ph::Bind { .. } => true,
                    Ph::Decl { slot, .. } => slot_names.contains(slot),
                    Ph::Ref(_) => false,
                });
                if conflict {
                    return Err(format!(
                        "{ctx}: 本指令已用 `ops` 键声明操作数，模板里就不能再写声明/绑定占位符\
                         （`{{名字:槽}}` / `{{@字段:槽}}`）——两者只写一处"
                    ));
                }
            }
            continue;
        }
        let Some(Extraction { decls, asm }) = extract_one(inst)? else {
            continue; // 无操作数指令（ret/nop）或纯生成器占位符
        };
        let streamed = vlen
            || inst.segments.is_some()
            || inst
                .form
                .as_deref()
                .is_some_and(|f| streamed_form.iter().any(|n| n == f));
        let mut bind: BTreeMap<String, String> = BTreeMap::new();
        let ops: Vec<String> = decls
            .iter()
            .map(|(name, slot, role, field, mem, _idx)| {
                if let Some(f) = field {
                    if streamed {
                        // 旧拼法：内存形态的操作数写成 `[名字]`——**作者显式标注优先**
                        // （`{@[rm]:mem}`），否则由槽 `kind = "mem"` 推断。
                        let val = if *mem || mem_slots.contains(slot) {
                            format!("[{name}]")
                        } else {
                            name.clone()
                        };
                        bind.insert(f.clone(), val);
                    } else {
                        bind.insert(name.clone(), f.clone());
                    }
                }
                match role {
                    OperandRole::Out => format!("{name}:{slot}:out"),
                    OperandRole::InOut => format!("{name}:{slot}:inout"),
                    OperandRole::In => format!("{name}:{slot}"),
                }
            })
            .collect();
        planned.push(Plan {
            idx: i,
            ops,
            asm,
            bind,
        });
    }
    for p in planned {
        let inst = &mut m.instructions[p.idx];
        inst.ops = Some(p.ops);
        inst.asm = AsmList(p.asm);
        if !p.bind.is_empty() {
            inst.bind = Some(p.bind);
        }
    }
    Ok(())
}
