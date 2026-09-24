//! metadata **单写**守卫（v3 方案 S5：metadata 单写）。
//!
//! 附件 metadata（`(kind, node)` 对）此前有**多条写路径**：指令可以经构造参数、
//! 直接 `inst.metadata.push(..)`，终结符还要经过 `term_metadata_mut` 返回的
//! `&mut SmallVec` 逃逸可变引用，函数是 `func.metadata.push(..)`，全局变量则是
//! `gv.metadata = attached`（整表替换，与函数的追加语义还不一致）。
//!
//! 本步把每个载体的写入收成**一个入口**，字段私有：
//!
//! | 载体 | 读 | 写 |
//! | --- | --- | --- |
//! | 指令 | `Instruction::metadata()` | `Instruction::attach_metadata()` |
//! | 终结符 | `DataFlowGraph::term_metadata()` | `DataFlowGraph::attach_term_metadata()` |
//! | 函数 | `Function::metadata()` | `Function::attach_metadata()` |
//! | 全局变量 | `GlobalVariable::metadata()` | `GlobalVariable::attach_metadata()` |
//! | 别名 | `GlobalAlias::metadata()` | `GlobalAlias::attach_metadata()` |
//!
//! 创建期的初始表仍走 `make_inst_with_meta_and_loc`（构造参数，不是"事后写"）。
//! 追加语义：同一 kind 再次附加即多一条（与文本里多处 `!dbg !N` 一一对应）。

use forge_ir::ir::builder::FunctionBuilder;
use forge_ir::ir::dfg::{TermMetadataAttach, ValueDef};
use forge_ir::ir::metadata::{AttachedMetadata, MetadataId, MetadataKind};
use forge_ir::ir::types::{FunctionSignature, TypeContext};
use forge_ir::text::parser::parse_module;
use forge_ir::{CallConvId, Function, Inst, InstFlags, Opcode, TypeId, Value};

fn am(kind: MetadataKind, node: u32) -> AttachedMetadata {
    AttachedMetadata {
        kind,
        node: MetadataId(node),
    }
}

/// 一个未终止的单块函数：块内一条 `Iadd`。
fn fixture() -> (Function, forge_ir::Block, Inst, Value) {
    let ctx = TypeContext::new();
    let sig_ref = ctx.register_signature(FunctionSignature::new(&[], &[ctx.i32_ty()]));
    let mut func = Function::new("f", ctx, sig_ref, CallConvId::default());
    let (b0, params) = func.dfg.make_block_with_params(&[TypeId::I32, TypeId::I32]);
    func.entry_block = Some(b0);
    let iadd = func.dfg.make_inst(
        Opcode::Iadd,
        b0,
        smallvec::smallvec![params[0], params[1]],
        smallvec::SmallVec::new(),
        &[TypeId::I32],
        InstFlags::NONE,
    );
    let result = func.dfg.inst_results(iadd)[0];
    (func, b0, iadd, result)
}

/// 指令附件：经唯一写入口追加，按附加序读回。
#[test]
fn instruction_metadata_appends_in_order() {
    let (mut func, _b0, iadd, _) = fixture();
    let inst = func.dfg.inst_mut(iadd);
    assert!(inst.metadata().is_empty(), "默认无附件");

    inst.attach_metadata(am(MetadataKind::DebugLoc, 0));
    inst.attach_metadata(am(MetadataKind::TBAA, 1));

    let kinds: Vec<&str> = inst.metadata().iter().map(|m| m.kind.name()).collect();
    assert_eq!(kinds, vec!["dbg", "tbaa"], "按附加序");
    assert_eq!(inst.metadata()[1].node, MetadataId(1));
}

/// 终结符附件：唯一写入口 + 三种"没写成"的原因分开报。
#[test]
fn terminator_metadata_uses_single_entry() {
    let ctx = TypeContext::new();
    let sig_ref = ctx.register_signature(FunctionSignature::new(&[], &[]));
    let mut func = Function::new("g", ctx, sig_ref, CallConvId::default());
    let (b0, _) = func.dfg.make_block_with_params(&[]);
    func.entry_block = Some(b0);

    // 未终止 → NoTerminator（不写）
    assert_eq!(
        func.dfg
            .attach_term_metadata(b0, am(MetadataKind::DebugLoc, 0)),
        TermMetadataAttach::NoTerminator
    );

    // ret 终结符 → Attached，读回经 term_metadata
    func.ret(b0, []);
    assert_eq!(
        func.dfg
            .attach_term_metadata(b0, am(MetadataKind::DebugLoc, 0)),
        TermMetadataAttach::Attached
    );
    assert_eq!(func.dfg.term_metadata(b0).len(), 1);

    // unreachable → Unreachable（接口拒绝，且读口也恒为空）
    let (b1, _) = func.dfg.make_block_with_params(&[]);
    func.unreachable(b1);
    assert_eq!(
        func.dfg
            .attach_term_metadata(b1, am(MetadataKind::DebugLoc, 1)),
        TermMetadataAttach::Unreachable
    );
    assert!(func.dfg.term_metadata(b1).is_empty());
}

/// 函数/全局变量附件：同一个入口，追加语义一致（全局不再是"整表替换"）。
#[test]
fn function_and_global_metadata_append() {
    let (mut func, _b0, _iadd, _) = fixture();
    func.attach_metadata(am(MetadataKind::DebugLoc, 0));
    func.attach_metadata(am(
        MetadataKind::Custom(forge_ir::ImmStr::from("absolute_symbol")),
        1,
    ));
    assert_eq!(func.metadata().len(), 2, "函数附件追加（不覆盖）");

    let mut gv = forge_ir::GlobalVariable::constant("g", TypeId::I32);
    assert!(gv.metadata().is_empty());
    gv.attach_metadata(am(MetadataKind::DebugLoc, 0));
    assert_eq!(gv.metadata().len(), 1);
    assert_eq!(gv.metadata()[0].kind, MetadataKind::DebugLoc);
}

/// 端到端：文本层四个载体的 metadata 都经唯一写入口落位。
#[test]
fn parser_lands_metadata_on_all_carriers() {
    let src = "!0 = !{i32 1}\n\
               @g = global i32 0, !dbg !0\n\
               define i32 @f(ptr %p) !dbg !0 {\n  \
               %e:\n    \
               %v = load i32, ptr %p, align 4, !dbg !0\n    \
               ret i32 %v, !dbg !0\n\
               }\n";
    let m = parse_module(src).unwrap_or_else(|e| panic!("parse: {e:?}\n{src}"));

    let (fr, func) = m
        .iter_func_refs()
        .find(|(_, f)| f.name.as_str() == "f")
        .expect("函数 f");
    let _ = fr;
    assert_eq!(func.metadata().len(), 1, "函数头 !dbg");
    assert_eq!(func.metadata()[0].kind, MetadataKind::DebugLoc, "kind 解析");

    let entry = func.entry_block.expect("entry");
    assert_eq!(func.dfg.term_metadata(entry).len(), 1, "ret 终结符 !dbg");

    let load = func
        .dfg
        .block(entry)
        .inst_order
        .iter()
        .copied()
        .find(|&i| func.dfg.inst_opcode(i) == Some(&Opcode::Load))
        .expect("load 指令");
    assert_eq!(func.dfg.inst_data(load).metadata().len(), 1);

    let gv = m
        .iter_globals()
        .find(|(_, g)| g.name.as_str() == "g")
        .map(|(_, g)| g)
        .expect("全局 g");
    assert_eq!(gv.metadata().len(), 1, "全局尾 !dbg");
}

/// 源码断言：`src/` 里 metadata 的写入只能出现在唯一写入口的实现体里。
#[test]
fn metadata_writes_only_inside_the_single_entry() {
    // 除写入口实现体外，任何对附件表的就地修改/整表替换都是"第二条写路径"。
    const FORBIDDEN: &[&str] = &[
        ".metadata.push(",
        ".metadata.extend(",
        ".metadata.insert(",
        ".metadata.append(",
        ".metadata.clear(",
        ".metadata.retain(",
        ".metadata =",
    ];
    // 白名单 = 各处 `attach_metadata(&mut self, ..)` 的实现体（同一行文本在
    // function.rs 出现 3 次：Function/GlobalVariable/GlobalAlias 三个载体）。
    // 路径是 `src/` 下的相对路径（目录归类后为 `ir/…`）。
    const ALLOWED: &[(&str, &str, &str)] = &[
        (
            "ir/dfg.rs",
            "self.metadata.push(metadata);",
            "Instruction::attach_metadata 唯一写入口",
        ),
        (
            "ir/function.rs",
            "self.metadata.push(metadata);",
            "Function/GlobalVariable/GlobalAlias::attach_metadata 唯一写入口",
        ),
        (
            "ir/dfg.rs",
            "i.metadata.clear();",
            "墓碑化（DataFlowGraph::tombstone_inst_low）清掉陈旧附件——删除语义的一部分，\
             不是第三条写路径（S2 墓碑语义显式化）",
        ),
    ];

    fn walk(root: &std::path::Path, dir: &std::path::Path, out: &mut Vec<(String, usize, String)>) {
        let Ok(entries) = std::fs::read_dir(dir) else {
            return;
        };
        for e in entries.flatten() {
            let p = e.path();
            if p.is_dir() {
                walk(root, &p, out);
            } else if p.extension().is_some_and(|x| x == "rs") {
                let Ok(text) = std::fs::read_to_string(&p) else {
                    continue;
                };
                let rel = p
                    .strip_prefix(root)
                    .unwrap_or(&p)
                    .to_string_lossy()
                    .replace('\\', "/");
                let body = text.split("#[cfg(test)]").next().unwrap_or(&text);
                for (i, line) in body.lines().enumerate() {
                    let t = line.trim();
                    if t.starts_with("//") {
                        continue;
                    }
                    if FORBIDDEN.iter().any(|f| t.contains(f)) {
                        out.push((rel.clone(), i + 1, t.to_string()));
                    }
                }
            }
        }
    }

    let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("src");
    let mut hits = Vec::new();
    walk(&root, &root, &mut hits);

    let mut used: Vec<usize> = Vec::new();
    let mut violations: Vec<String> = Vec::new();
    for (file, line, text) in &hits {
        match ALLOWED.iter().position(|(f, t, _)| f == file && t == text) {
            Some(i) => {
                if !used.contains(&i) {
                    used.push(i);
                }
            }
            None => violations.push(format!("{file}:{line}: {text}")),
        }
    }
    assert!(
        violations.is_empty(),
        "metadata 附表只能经唯一写入口修改（Instruction::attach_metadata / \
         DataFlowGraph::attach_term_metadata / Function::attach_metadata / \
         GlobalVariable::attach_metadata / GlobalAlias::attach_metadata）：\n{}",
        violations.join("\n")
    );
    for (i, (f, t, why)) in ALLOWED.iter().enumerate() {
        assert!(
            used.contains(&i),
            "白名单条目已失效（未被命中），请删除：{f} :: `{t}`（理由曾是：{why}）"
        );
    }
}

/// 只用公开入口也能覆盖 builder 生产路径（`ret` 之后仍可挂附件）。
#[test]
fn builder_built_function_accepts_metadata() {
    let ctx = TypeContext::new();
    let sig = FunctionSignature::new(&[], &[ctx.i32_ty()]);
    let mut fb = FunctionBuilder::new("h", ctx, sig);
    let (entry, _) = fb.create_entry_block();
    let v = fb.iconst_i32(7);
    fb.ret(&[v]);
    let mut func = fb.finish().expect("build");

    let ValueDef::Inst(iconst, _) = *func.dfg.value_def(v).expect("结果已定义") else {
        panic!("iconst 的结果应指令定义");
    };
    func.dfg
        .inst_mut(iconst)
        .attach_metadata(am(MetadataKind::DebugLoc, 0));
    assert_eq!(func.dfg.inst_data(iconst).metadata().len(), 1);
    assert_eq!(func.dfg.term_metadata(entry).len(), 0);
}
