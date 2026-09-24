//! v18 S6：**生成期自测**（`#[cfg(test)] mod __spec_tests`）。
//!
//! 目的：每条指令自动进回归网——新指令写进 TOML 就自动得到规格测试，不必再手写
//! "第 N 条指令编出来是不是这几个字节"的样板（S0 基线：生成面手写测试 2,952 行）。
//!
//! ## 断言的是不变式，不是黄金值
//!
//! 生成器不知道"正确字节"是什么（那正是被测对象），所以这里断言的是**闭环不变式**：
//!
//! 1. `encode` 成功，且长度 = 该指令的字长（`prefix_scan` 不适用，长度由前缀链决定）；
//! 2. `decode(bytes)` 成功、**消费恰好 `bytes.len()`**；
//! 3. `encode(decode(bytes)) == bytes`（字节稳定，别名/宽度视图落到同字节的另一名字也算过）；
//! 4. `decode_partial(bytes)` 与 `decode` 一致；
//! 5. 解码回来的字段**原样**：立即数/条件码取原值、寄存器取原索引、内存取原 base/disp
//!    （对称的错位——如 reg/rm 互换——字节级检查抓不到，值级检查能）；
//! 6. 文本闭环：`disassemble` → `assemble` 成功、文本幂等、再编码稳定；当该指令的
//!    汇编文本**唯一**时还要求"回到同一字节"（`SPEC_TEXT_AMBIGUOUS` 列出文本不唯一的
//!    那些：同名同形但编码不同，如 x86 的 `89`/`8B` 两条 `mov r/m, r`——文本本就分不清）；
//! 7. **立即数边界**：槽值域的 `min`/`max` 必须能编码并原样解码；`min-1`/`max+1`
//!    必须在 `encode` 处**报错**（P0-16 的"静默截断"回归）——只在生成器真的做这个检查时
//!    才断言（判据与 `gen_encode` 共享 [`imm_encode_checked`]，不做假保证）。
//!
//! ## 覆盖率可核对
//!
//! 生成模块导出 `SPEC_TOTAL` / `SPEC_COVERED` / `SPEC_SKIPPED`（名字 + 原因）/
//! `SPEC_TEXT_AMBIGUOUS`，并有 `spec_coverage_is_complete` 断言"零跳过"。S6 的判据是
//! **全指令覆盖**：跳过清单必须为空（真的做不到的指令要在此写明原因，而不是静默漏掉）。

use std::collections::BTreeMap;

use proc_macro2::{Literal, TokenStream};
use quote::{format_ident, quote};

use super::super::model::{EncodingKind, OperandKind, OperandSlot, V12Model};
use super::asm::{self, Seg};
use super::{InstInfo, reg_class_expr};
use syn::Ident;

/// 该 imm 槽是否受 **`encode`** 的值域检查（与 `gen_encode` 同一判据）。
///
/// `None` = 不检查（重定位指令的槽是链接期内部值、预移位散布位域存的是已移位值、
/// `prefix_scan` 的变长编码器不做值域检查）；`Some((min,max))` = 检查。
pub(crate) fn imm_encode_checked(
    m: &V12Model,
    info: &InstInfo,
    slot: &OperandSlot,
    field: &str,
) -> Option<(i64, i64)> {
    if slot.kind != OperandKind::Imm {
        return None;
    }
    if info.inst.reloc.is_some() || m.encoding.kind == EncodingKind::PrefixScan {
        return None;
    }
    let bf = m.conventions.bitfields.get(field)?;
    let preshifted = bf
        .pieces
        .as_ref()
        .is_some_and(|ps| !ps.is_empty() && ps.iter().all(|p| p.offset == p.shift));
    if preshifted {
        return None;
    }
    slot.imm_range()
}

/// 生成 `#[cfg(test)] mod __spec_tests`。
pub(crate) fn gen_spec_tests(infos: &[InstInfo], m: &V12Model) -> Result<TokenStream, String> {
    let cond_first = m
        .conventions
        .cond
        .as_ref()
        .and_then(|t| t.values().map(|e| e.code as u8).min());

    // (指令, 宽度视图) 用例表：多类寄存器槽逐宽度各测一遍（见 [`ViewSel`]）。
    let mut cases: Vec<(usize, usize)> = Vec::new();
    let mut all_views: Vec<Vec<ViewSel>> = Vec::with_capacity(infos.len());
    for (i, info) in infos.iter().enumerate() {
        let views = views_of(info);
        for v in 0..views.len() {
            cases.push((i, v));
        }
        all_views.push(views);
    }
    // 歧义分组按**用例**（指令 + 视图）算：同一指令的不同视图渲染文本不同，不算歧义。
    let keys: Vec<String> = cases
        .iter()
        .map(|(i, v)| text_key(&infos[*i], &all_views[*i][*v], cond_first))
        .collect();
    let amby = group_ambiguous_by_key(&cases, &keys, infos);

    // 谱内向量（v19 V3）：作者写在 `[[vectors]]` 里的字节/错误断言，形态已由
    // `validate::validate_vectors` 钉过，这里直接发射用例（不再判内容）。
    let vector_tests = gen_vector_tests(m);
    let n_vectors = m.vectors.len();
    // 谱内派生枚举器（v19 V3d）：宿主侧"全指令往返"用它替代手抄清单。
    let all_insts_item = gen_all_insts(infos, m)?;

    let mut used_names: Vec<String> = Vec::new();
    let mut bodies: Vec<TokenStream> = Vec::new();
    let mut skipped: Vec<(String, String)> = Vec::new();
    let mut covered_insts: Vec<usize> = Vec::new();
    for (ci, (i, v)) in cases.iter().enumerate() {
        let info = &infos[*i];
        let view = &all_views[*i][*v];
        let suffix = match &view.label {
            Some(l) => format!("_v{l}"),
            None => String::new(),
        };
        let mut fn_name = format!("spec_{}{}", sanitize_ident(&info.inst.name), suffix);
        while used_names.contains(&fn_name) {
            fn_name.push('_');
        }
        used_names.push(fn_name.clone());
        match gen_one(info, m, view, &fn_name, amby.get(&ci).map(Vec::as_slice)) {
            Ok(ts) => {
                bodies.push(ts);
                if !covered_insts.contains(i) {
                    covered_insts.push(*i);
                }
            }
            Err(reason) => {
                let name = match &view.label {
                    Some(l) => format!("{}[{l}]", info.inst.name),
                    None => info.inst.name.clone(),
                };
                skipped.push((name, reason));
            }
        }
    }

    let total = infos.len();
    let covered = covered_insts.len();
    let n_cases = cases.len();
    let skipped_ts: Vec<TokenStream> = skipped
        .iter()
        .map(|(n, why)| {
            let n = syn::LitStr::new(n, proc_macro2::Span::call_site());
            let why = syn::LitStr::new(why, proc_macro2::Span::call_site());
            quote! { (#n, #why) }
        })
        .collect();
    let amb_names: Vec<String> = {
        let mut v: Vec<String> = amby
            .values()
            .flat_map(|peers| peers.iter().cloned())
            .collect();
        v.sort();
        v.dedup();
        v
    };
    let amb_ts: Vec<TokenStream> = amb_names
        .iter()
        .map(|n| {
            let s = syn::LitStr::new(n, proc_macro2::Span::call_site());
            quote! { #s }
        })
        .collect();

    Ok(quote! {
        /// **生成期自测**（v18 S6）：每条指令的规格往返 + 边界，随 TOML 自动更新。
        ///
        /// 断言的是**闭环不变式**（`encode∘decode`/`decode∘encode`/文本幂等/字段原样/
        /// 立即数边界），不是手抄的黄金字节——"字节对不对"由各 ISA 的编码参考文档
        /// （`docs/reference/aarch64-encoding-ref.md` 等）与其黄金值测试守着。这里抓的是
        /// **不对称**缺陷：编码与解码不一致、汇编与反汇编不一致、静默截断/掩码、
        /// 别名撞车、宽度视图串味。
        #[cfg(test)]
        pub(crate) mod __spec_tests {
            use super::*;

            /// `[[templates]]` 展开后的指令总数。
            pub(crate) const SPEC_TOTAL: usize = #total;
            /// 自动覆盖的指令数（= `SPEC_TOTAL` 才叫全指令覆盖）。
            pub(crate) const SPEC_COVERED: usize = #covered;
            /// 生成的用例数（指令 × 宽度视图）：多类寄存器槽逐宽度各一条用例。
            pub(crate) const SPEC_CASES: usize = #n_cases;
            /// 无法自动构造操作数的指令（名字, 原因）——S6 判据：**必须为空**。
            pub(crate) const SPEC_SKIPPED: &[(&str, &str)] = &[#(#skipped_ts),*];
            /// 汇编文本不唯一的指令（同名同形、编码不同）：文本本就分不清是哪一条，
            /// 故不要求 `disasm → asm → encode` 回到同一字节，只要求文本幂等与自洽。
            pub(crate) const SPEC_TEXT_AMBIGUOUS: &[&str] = &[#(#amb_ts),*];
            /// 谱内 `[[vectors]]` 的条数（v19 V3）：由作者声明，不参与覆盖率判据。
            pub(crate) const SPEC_VECTORS: usize = #n_vectors;

            #all_insts_item

            /// 覆盖率自检：全指令覆盖（有跳过就报出名字与原因）。
            #[test]
            fn spec_coverage_is_complete() {
                assert_eq!(
                    SPEC_COVERED + SPEC_SKIPPED.len(),
                    SPEC_TOTAL,
                    "覆盖计数与指令总数不符"
                );
                assert!(SPEC_CASES >= SPEC_TOTAL, "用例数不应少于指令数");
                assert!(
                    SPEC_SKIPPED.is_empty(),
                    "有 {} 条指令（或视图）没能自动构造操作数：{:?}",
                    SPEC_SKIPPED.len(),
                    SPEC_SKIPPED
                );
            }

            /// 单条指令的闭环检查：返回 (解码结果, 编码字节)。
            fn __spec_roundtrip(name: &str, inst: &Inst, want_len: Option<usize>) -> (Inst, Vec<u8>) {
                let bytes = encode(inst)
                    .unwrap_or_else(|e| panic!("[{name}] encode 失败: {e}"));
                if let Some(w) = want_len {
                    assert_eq!(bytes.len(), w, "[{name}] 编码长度 (bytes={bytes:02x?})");
                }
                let (dec, n) = decode(&bytes)
                    .unwrap_or_else(|| panic!("[{name}] decode 失败 (bytes={bytes:02x?})"));
                assert_eq!(n, bytes.len(), "[{name}] decode 消费字节数 (bytes={bytes:02x?})");
                let re = encode(&dec)
                    .unwrap_or_else(|e| panic!("[{name}] encode(decode(..)) 失败: {e}"));
                assert_eq!(re, bytes, "[{name}] encode∘decode 字节不稳定 (dec={dec:?})");
                let (dp, dn) = decode_partial(&bytes)
                    .unwrap_or_else(|e| panic!("[{name}] decode_partial 失败: Err({e})"));
                assert_eq!(dn, n, "[{name}] decode_partial 与 decode 的消费量不一致");
                assert_eq!(
                    encode(&dp).unwrap_or_else(|e| panic!("[{name}] encode(decode_partial) 失败: {e}")),
                    bytes,
                    "[{name}] decode_partial 结果字节不稳定"
                );
                (dec, bytes)
            }

            /// 文本闭环：`disassemble` → `assemble` → 编码。`strict` = 该指令的汇编文本
            /// 唯一（回到同一字节）；否则只要求文本幂等与自洽。
            fn __spec_text(name: &str, inst: &Inst, bytes: &[u8], strict: bool) {
                let text = disassemble(inst);
                let back = assemble(&text)
                    .unwrap_or_else(|e| panic!("[{name}] assemble({text:?}) 失败: {e}"));
                assert_eq!(
                    disassemble(&back),
                    text,
                    "[{name}] 反汇编不幂等 (text={text:?}, back={back:?})"
                );
                let bb = encode(&back)
                    .unwrap_or_else(|e| panic!("[{name}] encode(assemble(text)) 失败: {e}"));
                if strict {
                    assert_eq!(bb, bytes, "[{name}] disasm→asm→encode 字节不一致 (text={text:?})");
                } else {
                    let (dec, n) = decode(&bb)
                        .unwrap_or_else(|| panic!("[{name}] assemble 结果 decode 失败 (bytes={bb:02x?})"));
                    assert_eq!(n, bb.len(), "[{name}] assemble 结果 decode 未消费完");
                    assert_eq!(
                        encode(&dec).unwrap_or_else(|e| panic!("[{name}] 再编码失败: {e}")),
                        bb,
                        "[{name}] assemble 结果 encode∘decode 不稳定"
                    );
                }
            }

            /// 立即数越界：`encode` **必须报错**（不得静默截断/掩码）。
            fn __spec_imm_oob(name: &str, inst: &Inst) {
                match encode(inst) {
                    Ok(b) => panic!("[{name}] 越界立即数竟然编码成功: {b:02x?}"),
                    Err(_) => {}
                }
            }

            #(#bodies)*

            // ── 谱内测试向量（`[[vectors]]`，v19 V3）──
            #(#vector_tests)*
        }
    })
}

/// 把 `[[vectors]]` 翻成用例（v19 V3）。
///
/// 四种形态与 `validate::validate_vectors` 的判据一一对应（那里挡"写坏的向量"，
/// 这里断言"写错的字节"）：
///
/// - `asm` + `bytes`：`assemble → encode` 逐字节相等，且 `decode` 吃满、再编码一致；
/// - `asm` + `error`：`assemble` 或 `encode` 必须失败且消息包含子串；
/// - `bytes` + `error = "DECODE"`：`decode` 必须失败（`partial` 另钉 `decode_partial`）；
/// - `bytes`：`decode` 成功且再编码逐字节相等。
///
/// 用例名 `spec_vector_<下标>`（下标从 0 起，与谱里 `[[vectors]]` 的顺序一致）；
/// 消息都带 `[向量 N]` 前缀——libtest 只报用例名与 panic 文本，下标能让作者直接回谱里定位。
fn gen_vector_tests(m: &V12Model) -> Vec<TokenStream> {
    let mut out = Vec::with_capacity(m.vectors.len());
    for (i, v) in m.vectors.iter().enumerate() {
        let doc_text = match &v.comment {
            Some(c) => c.clone(),
            None => vector_summary(v, i),
        };
        let doc = syn::LitStr::new(&doc_text, proc_macro2::Span::call_site());
        let tag = syn::LitStr::new(&format!("[向量 {i}]"), proc_macro2::Span::call_site());
        let ident = syn::Ident::new(&format!("spec_vector_{i}"), proc_macro2::Span::call_site());
        let byte_lits: Vec<proc_macro2::Literal> = v
            .bytes
            .iter()
            .flatten()
            .map(|b| proc_macro2::Literal::u8_unsuffixed(*b as u8))
            .collect();
        let asm_lit = v
            .asm
            .as_ref()
            .map(|a| syn::LitStr::new(a, proc_macro2::Span::call_site()));
        let body = match (asm_lit.as_ref(), &v.error, v.partial, v.bytes.is_some()) {
            // 正向：assemble → encode == bytes，且 decode 吃满 + 再编码一致
            (Some(asm), None, _, true) => quote! {
                let tag = #tag;
                let want: &[u8] = &[#(#byte_lits),*];
                let text = #asm;
                let inst = assemble(text)
                    .unwrap_or_else(|e| panic!("{tag} assemble {text:?} 失败: {e}"));
                let got = encode(&inst)
                    .unwrap_or_else(|e| panic!("{tag} encode {text:?} 失败: {e}"));
                assert_eq!(got.as_slice(), want, "{tag} {text:?} 编码字节不符");
                let (back, used) = decode(want)
                    .unwrap_or_else(|| panic!("{tag} decode {want:02x?} 返回 None"));
                assert_eq!(used, want.len(), "{tag} decode 未吃满 {want:02x?}");
                let re = encode(&back).unwrap_or_else(|e| panic!("{tag} 再编码失败: {e}"));
                assert_eq!(re.as_slice(), want, "{tag} decode∘encode 字节不稳");
            },
            // 汇编负向：assemble 或 encode 必须失败，消息含子串
            (Some(asm), Some(err), _, _) => {
                let err_lit = syn::LitStr::new(err, proc_macro2::Span::call_site());
                quote! {
                    let tag = #tag;
                    let text = #asm;
                    let want = #err_lit;
                    let msg = match assemble(text) {
                        Ok(inst) => match encode(&inst) {
                            Ok(b) => panic!("{tag} {text:?} 本应失败，却编出 {b:02x?}"),
                            Err(e) => e,
                        },
                        Err(e) => e,
                    };
                    assert!(
                        msg.contains(want),
                        "{tag} {text:?} 的错误消息不含 {want:?}: {msg}"
                    );
                }
            }
            // 解码负向：decode 必须失败（partial 另钉 decode_partial 的消费量）
            (None, Some(_), partial, true) => {
                let partial_ts = match partial {
                    Some(p) => {
                        let lit = proc_macro2::Literal::usize_unsuffixed(p);
                        quote! {
                            assert_eq!(
                                decode_partial(want),
                                Err(#lit),
                                "{tag} decode_partial 应在第 {} 字节处拒绝 {want:02x?}",
                                #lit
                            );
                        }
                    }
                    None => quote! {},
                };
                quote! {
                    let tag = #tag;
                    let want: &[u8] = &[#(#byte_lits),*];
                    assert!(
                        decode(want).is_none(),
                        "{tag} decode {want:02x?} 本应失败，却解出了指令"
                    );
                    #partial_ts
                }
            }
            // 解码正向：decode 成功且再编码等于原字节
            (None, None, _, true) => quote! {
                let tag = #tag;
                let want: &[u8] = &[#(#byte_lits),*];
                let (inst, used) = decode(want)
                    .unwrap_or_else(|| panic!("{tag} decode {want:02x?} 返回 None"));
                assert_eq!(used, want.len(), "{tag} decode 未吃满 {want:02x?}");
                let re = encode(&inst).unwrap_or_else(|e| panic!("{tag} 再编码失败: {e}"));
                assert_eq!(re.as_slice(), want, "{tag} decode∘encode 字节不稳");
            },
            // 只给 `asm`：**闭环向量**——不断言黄金字节，只断言"编解码 + 文本闭环稳定"
            // （v19 V3c）。给"我只知道这条文本合法、字节对不对由别处守"的清单用；
            // 文本歧义的指令（`SPEC_TEXT_AMBIGUOUS`）也安全：不比字节，只比稳定性。
            (Some(asm), None, _, false) => quote! {
                let tag = #tag;
                let text = #asm;
                let inst = assemble(text)
                    .unwrap_or_else(|e| panic!("{tag} assemble {text:?} 失败: {e}"));
                let bytes = encode(&inst)
                    .unwrap_or_else(|e| panic!("{tag} encode {text:?} 失败: {e}"));
                let (back, used) = decode(&bytes)
                    .unwrap_or_else(|| panic!("{tag} decode {bytes:02x?} 返回 None"));
                assert_eq!(used, bytes.len(), "{tag} decode 未吃满 {bytes:02x?}");
                let re = encode(&back).unwrap_or_else(|e| panic!("{tag} 再编码失败: {e}"));
                assert_eq!(re, bytes, "{tag} encode∘decode 字节不稳（{text:?}）");
                let rendered = disassemble(&back);
                let again = assemble(&rendered)
                    .unwrap_or_else(|e| panic!("{tag} assemble({rendered:?}) 失败: {e}"));
                assert_eq!(
                    disassemble(&again),
                    rendered,
                    "{tag} 反汇编不幂等（{text:?} → {rendered:?}）"
                );
            },
            // 其余组合已被 `validate_vectors` 拦下——这里 fail-closed，不静默跳过。
            _ => {
                let msg = syn::LitStr::new(
                    &format!("[[vectors]] 第 {i} 条形态非法（validate 应已报错）"),
                    proc_macro2::Span::call_site(),
                );
                quote! { compile_error!(#msg); }
            }
        };
        out.push(quote! {
            #[doc = #doc]
            #[test]
            fn #ident() {
                #body
            }
        });
    }
    out
}

/// 没写 `comment` 时的用例文档注释（形态 + 内容摘要）。
fn vector_summary(v: &crate::v12::model::Vector, i: usize) -> String {
    let what = match (&v.asm, &v.error, v.bytes.is_some()) {
        (Some(a), None, true) => format!("assemble({a:?}) → encode == bytes"),
        (Some(a), None, false) => format!("assemble({a:?}) → 闭环（编解码 + 文本稳定）"),
        (Some(a), Some(e), _) => format!("assemble({a:?}) 必须失败且消息含 {e:?}"),
        (None, Some(_), true) => "decode(bytes) 必须失败".to_string(),
        (None, None, true) => "decode(bytes) 成功且再编码一致".to_string(),
        _ => "（形态非法，见 validate 诊断）".to_string(),
    };
    format!("谱内向量 #{i}：{what}")
}

/// 该 imm 字段的"解码还原值"投影：给定用户值 `v`，解码应当还原出什么。
///
/// 直接从**位域定义**派生（不写死任何 ISA 常量）：
///
/// - **普通/散布位域**（各块 `shift = 0`，如 imm12、arm64 imm26）：编码取 `v` 的低
///   `Σwidth` 位，解码按 `signed` 符号/零扩展还原 ⇒ `expect = ext(v & mask)`；
/// - **预移位位域**（各块 `offset == shift`，如 riscv U 型 imm20
///   `{offset=12,width=20,shift=12}`）：槽里存的是**已左移**的值，编码取 `v >> shift`，
///   解码左移回 ⇒ `expect = ((v >> shift) & mask) << shift`（即"向下取到 2^shift 的倍数"）。
///
/// 第二类**没有**值域检查（`imm_encode_checked` 返回 None，lowering 传的就是已移位值），
/// 所以自测据实断言投影值，而不是假装 `v` 被原样保留。
fn field_expectation(m: &V12Model, field: &str, signed: bool, v: i64) -> i64 {
    use super::super::model::BitfieldPiece;
    let Some(bf) = m.conventions.bitfields.get(field) else {
        return v;
    };
    let pieces: Vec<BitfieldPiece> = match &bf.pieces {
        Some(ps) if !ps.is_empty() => ps.clone(),
        _ => vec![BitfieldPiece {
            offset: bf.offset.unwrap_or(0),
            width: bf.width.unwrap_or(64),
            shift: 0,
        }],
    };
    let preshifted = pieces.iter().all(|p| p.offset == p.shift);
    let shift = pieces.iter().map(|p| p.shift).max().unwrap_or(0);
    let total: u32 = pieces.iter().map(|p| p.width).sum();
    let mask = |w: u32| -> u64 { if w >= 64 { u64::MAX } else { (1u64 << w) - 1 } };
    // 组装"字段位内容"（未移位域）：各块取 `(v >> shift) & mask` 后按块拼接。
    let mut content: u64 = 0;
    let mut off_sum: u32 = 0;
    for p in &pieces {
        let bits = ((v as u64) >> p.shift) & mask(p.width);
        content |= bits << off_sum;
        off_sum += p.width;
    }
    if preshifted && shift > 0 {
        return ((content & mask(total)) << shift) as i64;
    }
    if signed {
        sign_extend(content & mask(total), total)
    } else {
        (content & mask(total)) as i64
    }
}

/// 把 `total` 位的位模式按有符号扩展成 i64。
fn sign_extend(v: u64, total: u32) -> i64 {
    if total == 0 || total >= 64 {
        return v as i64;
    }
    let shift = 64 - total;
    ((v << shift) as i64) >> shift
}

/// 宽度视图：多类寄存器槽（`classes = [...]`）**逐宽度各测一遍**。
///
/// 背景：`classes = ["gpr2","gpr4","gpr8"]` 的槽（x86 `gprx`）在 16/32/64 位视图下走
/// **不同编码路径**（66 前缀 / 无 REX.W / REX.W）——只测最宽视图会漏掉整条路径。
/// 视图内所有多类槽用**同一宽度**（正是汇编器看到的形态：`mov ax, bx`）。
struct ViewSel {
    /// 每个操作数的强制宽度（位）；`None` = 用槽声明/缺省类。
    forced: Vec<Option<u16>>,
    /// 视图标签：`None` = 默认（最宽）视图，其余 = `w16`/`w32`/`hi`…（进函数名与消息）。
    label: Option<String>,
    /// **高编号寄存器**变体：所有 Reg 操作数取该组的最高几个索引。
    ///
    /// 为什么单独来一遍：低索引（0/1/2）永远走不到编码的扩展位路径——x86 的
    /// REX.R/B/X（R8..R15）、8 位寄存器的 REX 强制（`spl/bpl/sil/dil` vs
    /// `ah/ch/dh/bh`，见 `byte_reg`），以及寄存器组宽 >8 的任何 ISA 的高位编码。
    high: bool,
}

/// `RegClass` 的字节宽度。
fn class_bytes(c: crate::v12::model::RegClass) -> u16 {
    use crate::v12::model::RegClass;
    match c {
        RegClass::GPR(w) | RegClass::FPR(w) | RegClass::VEC(w) | RegClass::KReg(w) => w,
    }
}

/// 槽里与给定宽度匹配的类（`classes` 列表里的那一项）。
fn view_class(slot: &OperandSlot, bytes: u16) -> Option<crate::v12::model::RegClass> {
    slot.classes
        .as_ref()?
        .iter()
        .copied()
        .find(|c| class_bytes(*c) == bytes)
}

/// 一条指令的全部用例视图：
///
/// - **宽度视图**：多类槽（`classes` 列了多个宽度，如 x86 `gprx` = 16/32/64 位）
///   逐宽度各一条 —— 三个宽度走**不同编码路径**（66 前缀 / 无 REX.W / REX.W）；
/// - **高编号寄存器视图**：有 Reg 操作数时再加一条（见 [`ViewSel::high`]）。
fn views_of(info: &InstInfo) -> Vec<ViewSel> {
    let n = info.operands.len();
    let multi: Vec<usize> = info
        .operands
        .iter()
        .enumerate()
        .filter(|(_, (_, _, s, _))| {
            s.kind == OperandKind::Reg && s.classes.as_ref().is_some_and(|c| c.len() > 1)
        })
        .map(|(i, _)| i)
        .collect();
    let has_reg = info
        .operands
        .iter()
        .any(|(_, _, s, _)| s.kind == OperandKind::Reg);
    let mut widths: Vec<u16> = Vec::new();
    for &i in &multi {
        if let Some(cs) = &info.operands[i].2.classes {
            for c in cs {
                let w = class_bytes(*c);
                if !widths.contains(&w) {
                    widths.push(w);
                }
            }
        }
    }
    widths.sort_unstable();
    let mut out: Vec<ViewSel> = if widths.len() <= 1 {
        vec![ViewSel {
            forced: vec![None; n],
            label: None,
            high: false,
        }]
    } else {
        let widest = *widths.last().expect("non-empty");
        widths
            .iter()
            .map(|w| {
                let mut forced = vec![None; n];
                for &i in &multi {
                    forced[i] = Some(*w);
                }
                ViewSel {
                    forced,
                    label: if *w == widest {
                        None
                    } else {
                        Some(format!("w{w}"))
                    },
                    high: false,
                }
            })
            .collect()
    };
    if has_reg {
        let base = out[0].forced.clone();
        out.push(ViewSel {
            forced: base,
            label: Some("hi".into()),
            high: true,
        });
    }
    out
}

/// 单个用例的汇编文本形状键：字面段序列 + **实参描述**序列。
///
/// 两个用例的键相同 ⇒ 同一段文本既可能装配成 A、也可能装配成 B（汇编器按声明序
/// 取值），故"文本 → 编码"对它们不作字节相等的强要求。
///
/// 实参描述必须含**具体操作数**（寄存器类 + 索引、立即数值…），不能只写"reg/imm"
/// 这类槽类型——`add x0, x1, x2` 与 `add w0, w1, w2` 的槽类型一样但文本不同，
/// 只按槽类型判会把它们误记为歧义（白白放弃强断言）。
fn text_key(info: &InstInfo, view: &ViewSel, cond_first: Option<u8>) -> String {
    let lits: Vec<String> = asm::parse_template(&info.inst.asm)
        .map(|segs| {
            segs.iter()
                .filter_map(|s| match s {
                    Seg::Lit(l) => Some(l.clone()),
                    Seg::Op(_) => None,
                })
                .collect()
        })
        .unwrap_or_default();
    // 字段名不入键（文本里看不见字段名——两条指令字段名不同但文本一样时，
    // 汇编器照样分不清）。
    let args: Vec<String> = info
        .operands
        .iter()
        .enumerate()
        .map(|(j, (_, _, slot, _))| match slot.kind {
            OperandKind::Reg => {
                let cls = match view
                    .forced
                    .get(j)
                    .copied()
                    .flatten()
                    .and_then(|w| view_class(slot, w))
                {
                    Some(c) => format!("{c:?}"),
                    None => slot
                        .class
                        .map(|c| format!("{c:?}"))
                        .or_else(|| slot.classes.as_ref().map(|cs| format!("{cs:?}")))
                        .unwrap_or_else(|| "default".into()),
                };
                // 索引进键：低/高视图渲染出的寄存器名不同（`rax` vs `r15`），
                // 不区分会让两条用例互相误判成"文本歧义"。
                format!("reg{j}:{cls}:{}", if view.high { "hi" } else { "lo" })
            }
            OperandKind::Mem => format!("mem{j}"),
            OperandKind::Imm => {
                let (lo, hi) = slot.imm_range().unwrap_or((0, 0));
                let v = if lo <= 0 && 0 <= hi { 0 } else { lo };
                format!("imm{j}={v}")
            }
            OperandKind::Label => format!("label{j}"),
            OperandKind::Cond => format!("cond{j}={}", cond_first.unwrap_or(0)),
        })
        .collect();
    format!("{lits:?}|{args:?}")
}

/// 文本不唯一的用例：用例下标 → 同键的**其它指令名**。
fn group_ambiguous_by_key(
    cases: &[(usize, usize)],
    keys: &[String],
    infos: &[InstInfo],
) -> BTreeMap<usize, Vec<String>> {
    let mut by_key: BTreeMap<&str, Vec<usize>> = BTreeMap::new();
    for (ci, k) in keys.iter().enumerate() {
        by_key.entry(k.as_str()).or_default().push(ci);
    }
    let mut out: BTreeMap<usize, Vec<String>> = BTreeMap::new();
    for (_, members) in by_key {
        if members.len() < 2 {
            continue;
        }
        for &ci in &members {
            let me = infos[cases[ci].0].inst.name.clone();
            let mut peers: Vec<String> = members
                .iter()
                .filter(|&&cj| cj != ci)
                .map(|&cj| infos[cases[cj].0].inst.name.clone())
                .filter(|n| *n != me)
                .collect();
            peers.sort();
            peers.dedup();
            if !peers.is_empty() {
                out.insert(ci, peers);
            }
        }
    }
    out
}

/// 一条指令的测试函数（一个宽度视图一条）。
/// 一个视图下每个操作数的**样本取值表达式**（生成期自测与派生枚举器共用一份）。
///
/// 抽出来是为了让"派生枚举器"（[`gen_all_insts`]）与生成期自测用**同一套**取值规则：
/// 两处各写一遍必然漂移（枚举器里的样本一旦不可编码，往返测试才红——发现得太晚）。
struct SampleOperands {
    /// 与 `info.operands` 同序的取值表达式。
    exprs: Vec<TokenStream>,
    /// 解码字段"原样"断言。
    checks: Vec<TokenStream>,
    /// (操作数下标, 字段名, lo, hi, 是否做越界检查)。
    imm_slots: Vec<(usize, String, i64, i64, bool)>,
    has_cond: bool,
    /// (操作数下标, 类表达式)：内存槽的寄存器类，供枚举器派生 disp/index 风味。
    mem_cls: Vec<(usize, TokenStream)>,
}

/// 生成一个视图下的样本操作数（见 [`SampleOperands`]）。
fn sample_operands(
    info: &InstInfo,
    m: &V12Model,
    view: &ViewSel,
    label: &str,
    name: &str,
    cond_codes: &[u8],
) -> Result<SampleOperands, String> {
    let mut exprs: Vec<TokenStream> = Vec::new();
    let mut checks: Vec<TokenStream> = Vec::new();
    let mut imm_slots: Vec<(usize, String, i64, i64, bool)> = Vec::new();
    let mut mem_cls: Vec<(usize, TokenStream)> = Vec::new();
    let mut has_cond = false;

    for (i, (fname, fid, slot, _)) in info.operands.iter().enumerate() {
        match slot.kind {
            OperandKind::Reg => {
                // 宽度视图：多类槽按本视图的宽度取槽自己的类；否则用槽声明/缺省类。
                let forced = view
                    .forced
                    .get(i)
                    .copied()
                    .flatten()
                    .and_then(|w| view_class(slot, w));
                let (cls, count) = match forced {
                    Some(c) => (class_ts(&c), m.names_of(c)?.len() as u32),
                    None => (reg_class_expr(slot), slot_reg_count(m, slot)?),
                };
                // 索引：低视图按操作数序号（不同操作数取不同寄存器，便于抓 reg/rm 互换）；
                // 高视图取该组最高的几个索引（走编码扩展位路径）。
                let idx = if view.high {
                    let n_reg = info
                        .operands
                        .iter()
                        .filter(|(_, _, s, _)| s.kind == OperandKind::Reg)
                        .count() as u32;
                    let k = info.operands[..i]
                        .iter()
                        .filter(|(_, _, s, _)| s.kind == OperandKind::Reg)
                        .count() as u32;
                    count
                        .saturating_sub(n_reg)
                        .saturating_add(k)
                        .min(count.saturating_sub(1))
                } else {
                    (i as u32).min(count.saturating_sub(1))
                };
                exprs.push(quote! { <Reg as forge_ir::PhysReg>::from_index(#idx, #cls) });
                let msg = syn::LitStr::new(
                    &format!("{label}: 寄存器 {fname} 索引未原样解码（index={idx}）"),
                    proc_macro2::Span::call_site(),
                );
                let idx_lit = Literal::u32_unsuffixed(idx);
                checks.push(quote! {
                    assert_eq!(
                        <Reg as forge_ir::PhysReg>::to_index(*#fid),
                        #idx_lit,
                        #msg
                    );
                });
            }
            OperandKind::Imm => {
                let (lo, hi) = slot.imm_range().unwrap_or((0, 0));
                let base = if lo <= 0 && 0 <= hi { 0 } else { lo };
                let lit = Literal::i64_suffixed(base);
                exprs.push(quote! { #lit });
                let msg = syn::LitStr::new(
                    &format!("{name}: 立即数 {fname} 未按位域语义解码"),
                    proc_macro2::Span::call_site(),
                );
                let want = field_expectation(m, fname, slot.signed.unwrap_or(false), base);
                let base_lit = Literal::i64_suffixed(want);
                checks.push(quote! { assert_eq!(*#fid, #base_lit, #msg); });
                let checked = imm_encode_checked(m, info, slot, fname).is_some();
                imm_slots.push((i, fname.clone(), lo, hi, checked));
            }
            OperandKind::Label => {
                exprs.push(quote! { 0i64 });
            }
            OperandKind::Cond => {
                has_cond = true;
                let Some(first) = cond_codes.first() else {
                    return Err(format!("{name}: 有 cond 操作数但无 [conventions.cond] 表"));
                };
                let lit = Literal::u8_suffixed(*first);
                exprs.push(quote! { #lit });
                // 条件码断言在 cond 循环里逐值做（见 `gen_one`），基线这里跳过。
            }
            OperandKind::Mem => {
                let cls = match (&slot.class, &slot.classes) {
                    (Some(c), _) => class_ts(c),
                    (None, Some(cs)) if !cs.is_empty() => class_ts(&cs[cs.len() - 1]),
                    _ => quote! { __DEFAULT_GPR_CLASS },
                };
                exprs.push(mem_ref_expr(&cls, 0, None, 1));
                mem_cls.push((i, cls));
                let msg_b = syn::LitStr::new(
                    &format!("{name}: 内存操作数 {fname} 的 base 未原样解码"),
                    proc_macro2::Span::call_site(),
                );
                let msg_d = syn::LitStr::new(
                    &format!("{name}: 内存操作数 {fname} 的 disp 未原样解码"),
                    proc_macro2::Span::call_site(),
                );
                checks.push(quote! {
                    assert_eq!(<Reg as forge_ir::PhysReg>::to_index((#fid).base), 0u32, #msg_b);
                    assert_eq!((#fid).disp, 0i64, #msg_d);
                });
            }
        }
    }

    Ok(SampleOperands {
        exprs,
        checks,
        imm_slots,
        has_cond,
        mem_cls,
    })
}

/// `MemRef { base, disp, index, scale }` 表达式（`index` = 寄存器索引或 `None`）。
fn mem_ref_expr(cls: &TokenStream, disp: i64, index: Option<u32>, scale: u8) -> TokenStream {
    let idx = match index {
        Some(i) => {
            let i = Literal::u32_unsuffixed(i);
            quote! { Some(<Reg as forge_ir::PhysReg>::from_index(#i, #cls)) }
        }
        None => quote! { None },
    };
    let disp = Literal::i64_suffixed(disp);
    quote! {
        MemRef {
            base: <Reg as forge_ir::PhysReg>::from_index(0u32, #cls),
            disp: #disp,
            index: #idx,
            scale: #scale,
        }
    }
}

/// **谱内派生「代表实例」枚举器**（v19 V3d）：每条指令 × 每个宽度视图一个 `Inst`，
/// 操作数取值与生成期自测**同源**（[`sample_operands`]），另加两类风味以补足覆盖面：
///
/// - **立即数边界**：每个 imm 槽的 `lo`/`hi`（手抄清单里那些"负数/最大值"用例的替代）；
/// - **内存风味**：每个 mem 槽额外派生 `disp=8` / `disp=-8` / `index+scale=4` 三种，
///   因为自测基线只用 `disp=0`（手抄清单里 `[RAX+8]`/`[RAX+RBX*4]` 那类的替代）。
///
/// 用途：**宿主侧的"全指令编解码往返"不必再手抄一份 `all_insts()`**（x86 曾有 650 行，
/// 且谱加指令时那份清单不会自动跟上）。只在 `#[cfg(test)]` 下发射，宿主成品零成本。
/// 守卫 `crate::isa_roundtrip_guard` 用它跑三份发行谱的往返 + 覆盖清点。
pub(crate) fn gen_all_insts(infos: &[InstInfo], m: &V12Model) -> Result<TokenStream, String> {
    let cond_codes: Vec<u8> = match &m.conventions.cond {
        Some(t) => {
            let mut v: Vec<u8> = t.values().map(|e| e.code as u8).collect();
            v.sort_unstable();
            v.dedup();
            v
        }
        None => Vec::new(),
    };
    let mut entries: Vec<(String, TokenStream)> = Vec::new();
    for info in infos {
        let vn = &info.vn;
        let name = info.inst.name.clone();
        for view in views_of(info) {
            let label = match &view.label {
                Some(l) => format!("{name}[{l}]"),
                None => name.clone(),
            };
            let Ok(sample) = sample_operands(info, m, &view, &label, &name, &cond_codes) else {
                continue; // 构造不出来的（与自测同判据）由 `SPEC_SKIPPED` 报出，这里跳过。
            };
            let ctor = |over: &[(usize, TokenStream)]| -> TokenStream {
                if info.operands.is_empty() {
                    return quote! { Inst::#vn };
                }
                let fields: Vec<TokenStream> = info
                    .operands
                    .iter()
                    .enumerate()
                    .map(|(i, (_, fid, _, _))| {
                        let e = over
                            .iter()
                            .find(|(k, _)| *k == i)
                            .map(|(_, e)| e.clone())
                            .unwrap_or_else(|| sample.exprs[i].clone());
                        quote! { #fid: #e }
                    })
                    .collect();
                quote! { Inst::#vn { #(#fields),* } }
            };
            entries.push((label.clone(), ctor(&[])));

            // 立即数边界（lo/hi 去重）。
            for (slot_i, fname, lo, hi, _) in &sample.imm_slots {
                let mut vals = vec![*lo, *hi];
                vals.dedup();
                for v in vals {
                    let lit = Literal::i64_suffixed(v);
                    entries.push((
                        format!("{label}[{fname}={v}]"),
                        ctor(&[(*slot_i, quote! { #lit })]),
                    ));
                }
            }
            // 内存风味：disp≠0 与 index+scale。
            for (slot_i, cls) in &sample.mem_cls {
                for (tag, e) in [
                    ("disp=8", mem_ref_expr(cls, 8, None, 1)),
                    ("disp=-8", mem_ref_expr(cls, -8, None, 1)),
                    ("idx*4", mem_ref_expr(cls, 0, Some(1), 4)),
                ] {
                    entries.push((format!("{label}[{tag}]"), ctor(&[(*slot_i, e)])));
                }
            }
        }
    }
    let pairs: Vec<TokenStream> = entries
        .iter()
        .map(|(label, ctor)| {
            let s = syn::LitStr::new(label, proc_macro2::Span::call_site());
            quote! { (#s, #ctor) }
        })
        .collect();
    let names: Vec<TokenStream> = infos
        .iter()
        .map(|i| {
            let s = syn::LitStr::new(&i.inst.name, proc_macro2::Span::call_site());
            quote! { #s }
        })
        .collect();
    Ok(quote! {
        /// 全部指令名（`[[templates]]` 展开后）——枚举器覆盖清点用。
        pub(crate) const SPEC_INSTS: &[&str] = &[#(#names),*];

        /// **谱内派生的代表实例**（v19 V3d）：`(名字, Inst)`。
        ///
        /// 每条指令 × 每个宽度视图一条，外加立即数边界与内存风味（见 `gen_all_insts`）。
        /// 宿主侧"全指令编解码往返"直接遍历它即可——**不要手抄 `all_insts()`**：
        /// 手抄清单在谱加指令时不会自动跟上（x86 曾有 650 行）。只在 `#[cfg(test)]` 下发射。
        pub(crate) fn all_insts() -> Vec<(&'static str, Inst)> {
            vec![#(#pairs),*]
        }
    })
}

fn gen_one(
    info: &InstInfo,
    m: &V12Model,
    view: &ViewSel,
    fn_name: &str,
    peers: Option<&[String]>,
) -> Result<TokenStream, String> {
    let vn = &info.vn;
    let name = info.inst.name.clone();
    let label = match &view.label {
        Some(l) => format!("{name}[{l}]"),
        None => name.clone(),
    };
    let name_lit = syn::LitStr::new(&label, proc_macro2::Span::call_site());
    let fn_ident = format_ident!("{fn_name}");

    // 该指令的字长（字节）：fixed/mixed 有确定值；prefix_scan 由前缀链决定。
    let want_len: Option<u32> = if m.encoding.kind == EncodingKind::PrefixScan {
        None
    } else {
        Some(m.inst_width_bytes(&info.inst)?)
    };
    let want_len_ts = match want_len {
        Some(w) => {
            let lit = Literal::u32_unsuffixed(w);
            quote! { Some(#lit as usize) }
        }
        None => quote! { None },
    };

    // 条件码表的全部编码（`cond` 槽逐编码测一遍）。
    let cond_codes: Vec<u8> = match &m.conventions.cond {
        Some(t) => {
            let mut v: Vec<u8> = t.values().map(|e| e.code as u8).collect();
            v.sort_unstable();
            v.dedup();
            v
        }
        None => Vec::new(),
    };

    // 样本操作数与"解码字段原样"断言：与派生枚举器同源（`sample_operands`）。
    let SampleOperands {
        exprs,
        checks,
        imm_slots,
        has_cond,
        ..
    } = sample_operands(info, m, view, &label, &name, &cond_codes)?;

    let inst_of = |over: Option<(usize, i64)>| -> TokenStream {
        if info.operands.is_empty() {
            return quote! { Inst::#vn };
        }
        let fields: Vec<TokenStream> = info
            .operands
            .iter()
            .enumerate()
            .map(|(i, (_, fid, _, _))| {
                let expr = match over {
                    Some((k, v)) if k == i => {
                        let lit = Literal::i64_suffixed(v);
                        quote! { #lit }
                    }
                    _ => exprs[i].clone(),
                };
                quote! { #fid: #expr }
            })
            .collect();
        quote! { Inst::#vn { #(#fields),* } }
    };

    let strict = peers.is_none();
    let strict_lit = syn::LitBool::new(strict, proc_macro2::Span::call_site());
    let peers_doc = match peers {
        Some(p) => {
            let s = syn::LitStr::new(
                &format!("同名同形（文本分不清）：{}", p.join(" / ")),
                proc_macro2::Span::call_site(),
            );
            quote! { #[doc = #s] }
        }
        None => quote! {},
    };

    let base_inst = inst_of(None);

    // cond 循环（该指令有 cond 槽时逐编码测一遍）。
    let cond_loop = if has_cond {
        let codes: Vec<TokenStream> = cond_codes
            .iter()
            .map(|c| {
                let lit = Literal::u8_suffixed(*c);
                quote! { #lit }
            })
            .collect();
        // 循环内重建指令：cond 槽换成循环变量。
        let cond_idx = info
            .operands
            .iter()
            .position(|(_, _, s, _)| s.kind == OperandKind::Cond)
            .expect("has_cond ⇒ 至少一个 cond 槽");
        let fields: Vec<TokenStream> = info
            .operands
            .iter()
            .enumerate()
            .map(|(i, (_, fid, s, _))| {
                if s.kind == OperandKind::Cond {
                    quote! { #fid: __cc }
                } else {
                    let e = &exprs[i];
                    quote! { #fid: #e }
                }
            })
            .collect();
        let loop_inst = if info.operands.is_empty() {
            quote! { Inst::#vn }
        } else {
            quote! { Inst::#vn { #(#fields),* } }
        };
        let cond_fid = &info.operands[cond_idx].1;
        let msg = syn::LitStr::new(
            &format!("{name}: 条件码未原样解码"),
            proc_macro2::Span::call_site(),
        );
        quote! {
            for __cc in [#(#codes),*] {
                let __ci = #loop_inst;
                let (__cd, __cb) = __spec_roundtrip(#name_lit, &__ci, #want_len_ts);
                __spec_text(#name_lit, &__ci, &__cb, #strict_lit);
                if let Inst::#vn { #cond_fid: __c, .. } = &__cd {
                    assert_eq!(*__c, __cc, #msg);
                }
            }
        }
    } else {
        quote! {}
    };

    // 立即数边界：可编码边界（按位域语义解码）+ 越界必须报错。
    let mut imm_tests: Vec<TokenStream> = Vec::new();
    for (slot_i, fname, lo, hi, checked) in &imm_slots {
        let mut values = vec![*lo, *hi];
        values.dedup();
        let signed = info.operands[*slot_i].2.signed.unwrap_or(false);
        for v in values {
            let inst = inst_of(Some((*slot_i, v)));
            let want = field_expectation(m, fname, signed, v);
            let label = syn::LitStr::new(
                &format!("{name}[{fname}={v}]"),
                proc_macro2::Span::call_site(),
            );
            let fid = &info.operands[*slot_i].1;
            let msg = syn::LitStr::new(
                &format!("{name}: 立即数 {fname} 边界值 {v} 未按位域语义解码（期望 {want}）"),
                proc_macro2::Span::call_site(),
            );
            let v_lit = Literal::i64_suffixed(want);
            imm_tests.push(quote! {
                {
                    let __ii = #inst;
                    let (__id, _) = __spec_roundtrip(#label, &__ii, #want_len_ts);
                    if let Inst::#vn { #fid: __iv, .. } = &__id {
                        assert_eq!(*__iv, #v_lit, #msg);
                    }
                }
            });
        }
        if *checked {
            for v in [lo.checked_sub(1), hi.checked_add(1)].into_iter().flatten() {
                let inst = inst_of(Some((*slot_i, v)));
                let label = syn::LitStr::new(
                    &format!("{name}[{fname}={v} 越界]"),
                    proc_macro2::Span::call_site(),
                );
                imm_tests.push(quote! { __spec_imm_oob(#label, &#inst); });
            }
        }
    }

    // 无操作数指令（ret/nop/…）是 unit 变体，不能写 `{ .. }` 模式。
    let field_checks = if info.operands.is_empty() {
        quote! {}
    } else {
        let fids: Vec<&Ident> = info.operands.iter().map(|(_, fid, _, _)| fid).collect();
        quote! {
            if let Inst::#vn { #(#fids),* } = &__dec {
                #(#checks)*
            }
        }
    };

    Ok(quote! {
        /// `#name`：闭环 + 字段原样 + 文本幂等 + 立即数边界。
        #peers_doc
        #[test]
        fn #fn_ident() {
            let __base = #base_inst;
            let (__dec, __bytes) = __spec_roundtrip(#name_lit, &__base, #want_len_ts);
            __spec_text(#name_lit, &__base, &__bytes, #strict_lit);
            #field_checks
            #(#imm_tests)*
            #cond_loop
        }
    })
}

/// 槽的寄存器组名（`RegClass` 字面量）。
fn class_ts(c: &crate::v12::model::RegClass) -> TokenStream {
    use crate::v12::model::RegClass;
    match c {
        RegClass::GPR(w) => quote! { forge_ir::RegClass::GPR(#w) },
        RegClass::FPR(w) => quote! { forge_ir::RegClass::FPR(#w) },
        RegClass::VEC(w) => quote! { forge_ir::RegClass::VEC(#w) },
        RegClass::KReg(w) => quote! { forge_ir::RegClass::KReg(#w) },
    }
}

/// Reg 槽可用的寄存器个数（多类槽取主 GPR 类）。
fn slot_reg_count(m: &V12Model, slot: &OperandSlot) -> Result<u32, String> {
    let cls = match (&slot.class, &slot.classes) {
        (Some(c), _) => *c,
        (None, Some(cs)) if !cs.is_empty() => cs[cs.len() - 1],
        _ => m.main_gpr_class()?,
    };
    Ok(m.names_of(cls)?.len() as u32)
}

/// Rust 标识符安全化（指令名 → 测试函数名后缀）。
fn sanitize_ident(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for ch in s.chars() {
        if ch.is_ascii_alphanumeric() || ch == '_' {
            out.push(ch.to_ascii_lowercase());
        } else {
            out.push('_');
        }
    }
    if out.is_empty() || out.starts_with(|c: char| c.is_ascii_digit()) {
        out.insert(0, 'i');
    }
    out
}
