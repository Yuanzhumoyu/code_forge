# forge-ir 迭代路线图（LLVM IR 文本层对齐）

## ⚠️ ARCHIVED（2026-09）

> forge-ir 迭代路线图（LLVM IR 文本层对齐；历轮成果至第 31 轮，2026-08 停更）。
> 本文为历史记录，仅供参考；代码现状以仓库代码与现行文档为准，不再维护。
> 状态：持续更新 · 当前代码已达第 31 轮（六轮后的历轮执行记录见附录 §7 与
> `forge-ir-iteration-checklist.md` §0.1 / `forge-ir-remaining-tasks.md` §5-§7）
> 适用 crate：`crates/foundation/forge-ir`（及其依赖方 `forge-opt` / `forge-codegen` / `forge-hir`）
> 基线：LLVM Assembler 452 全收敛（198 正向 / 254 正确拒绝 / 0 误接受）、
> roundtrip 0 缺口、workspace 48 suite 全绿、`cargo check --workspace` 0 error

---

## 0. 总览

`forge-ir` 的文本层（`ir_parser`）以 **LLVM IR 文本为交换格式**，实现
`text → parse → Module → display → text` 的双向 round-trip。前六轮迭代记录：

| 轮次 | 内容 |
|---|---|
| 1-2 | 文本 round-trip 修复、builder 宏化、验证器补强、分析模块重构 |
| 3 | 原子指令（atomicrmw/cmpxchg/fence）、call 属性、zeroinitializer、聚合常量（global init）、`cc N` |
| 4 | 类型定义（`%struct.X = type`）、call 实参属性、全局属性、declare 裸类型参数 |
| 5 | call-site 属性 + attribute groups（`#0`）、metadata（`!dbg`）、向量 lane 指令、全局扩展（addrspace/thread_local） |
| 6 | 类型定义引用、win64cc、visibility/comdat、聚合字面量 extractvalue、Vextract/Vinsert 变量 idx |

> 六轮后的迭代（第七～三十二轮）逐轮记录于附录 §7 与配套文档
> （`forge-ir-iteration-checklist.md` §0.1 / `forge-ir-remaining-tasks.md` §5-§7）；
> 本文 §2/§3 各"待办"现状均已实现，状态标注见各节。官方语料已 452 全收敛
> （198 正向 / 254 拒绝 / 0 误接受）。

本清单按 **P0（近期、文本层高价值）/ P1（中期、需内部表示扩展）/ P2（测试与工具链）/ 性能** 分级，
每项给出：现状引用、目标语法、难点、解决思路、代码演示、验收标准。

---

## 1. 架构约定与 LALR 约束（必读）

**Golden rules**（历轮沉淀，勿违反）：

1. **内部 IR 不变，标准化在文本层**——DFG/块参数/常量池结构保持；新语法一律经
   parse/display 双向转换，除非内部表示确有缺口（P1 的聚合常量例外）。
2. **LALR(1) 显式分支**——grammar.lalrpop 中任何"可选/可空"构造都优先
   **展开成显式分支**，不依赖嵌套 optionals：
   ```lalrpop
   // 错误示范：`(X)* X` 闭包 + 尾部可空 → 2-lookahead 冲突
   TypeOps: Vec<ParsedOperand> = { <mut v: (<TypeOp> Comma)*> <p: TypeOp> => … };
   // 正确示范：显式列分支
   CallArg: (ParsedOperand, Vec<String>) = {
       <ty: ValueType> <v: Value> => …,
       <ty: ValueType> <attrs: ParamAttrs> <v: Value> => …,
   };
   ```
3. **`(X Comma)* X` 闭包的 2-lookahead 陷阱**——当 `X` 的 first 集合含
   `LocalId`（命名类型/指令结果名）或聚合字面量（`LBrace`/`LBracket`/`ZeroInitKw`）时，
   LALR(1) 无法区分"闭包继续"与"闭包结束"（需看第 2 个 token）。规避：
   - 命名类型引用只放**确定性位置**（如 `extractvalue` agg 的独立规则 `NamedOrType`）；
   - 参数/实参类型保持 `ValueType`（不含 LocalId）。
4. **关键字用 lexer token（priority 2）**——避免与 Ident 指令名冲突；
   但要意识到**新增 token 会改变全局 LALR 状态图**（第 6 轮 TypeKw 曾引发连锁冲突）。
5. **display 与 parse 必须互逆**——display 输出的每个 token 都必须是 parse 可接受的；
   聚合常量等"内部无表示"的语法用 `Immediate::String(InternedStr)` 存**序列化文本**原样还原。
6. **测试模式固定**：`assert_roundtrip(src)`（parse₁ → display → parse₂，结构+常量池值等价）
   每轮新增用例 + 全量 `cargo test -p forge-ir` + workspace 44 suite + `cargo check --workspace`。

**LALR 已知硬限制**（第 5-6 轮实证，均不可在纯 LALR(1) 内解决）：
- `add i32 %a, i32 %b, !tbaa !0`（通用指令尾部逗号附加）：`TypeOps` 尾逗号与
  `InstFlagSeq`/模块 item 状态合并冲突；
- `!t = !{...}` 命名 metadata 定义：`MetadataNameKw` 同时是模块 item first 与
  attach name → 与 declare/define 尾 epsilon 链的 FOLLOW 交集；
- `declare i32 @f(i32) nounwind` 尾部属性：`RParen`（归约）与 `RParen FuncAttrs`（shift）
  在 LALR 状态合并后 lookahead 污染。
  **解决方向**：lexer 层预处理（见 2.1/2.2）或 GLR。

---

## 2. P0 — 文本层 LLVM 对齐（高价值）

### 2.1 declare 尾部属性

**状态：✅ 已实现**——`DeclareTail` 显式分支（grammar.lalrpop:1155-1271，Declare 各
变体统一尾部 `<tail: DeclareTail>`，空 / attrs / attrs+#N / #N 四分支）；`declare i32
@printf(ptr noundef, ...) nounwind` 等 round-trip 通过（第二十八轮复核确认）。

**历史现状（实现前）**：`declare` 分支（grammar.lalrpop ~467-506）为 `Declare … RParen =>`，
尾部只有 `FuncAttrs?` 被裁剪掉了——`declare i32 @printf(ptr noundef, ...) nounwind`
是 clang 的**最常见输出**，当时无法 round-trip（`roundtrip_unnamed_declare_params`
测试只能写裸 declare）。

**目标语法**：
```llvm
declare dso_local noundef i32 @printf(ptr noundef, ...) nounwind
declare i32 @memcpy(ptr, ptr, i64) #0
declare void @f() !dbg !3
```

**难点**：LALR 状态合并（`DeclareDef = … RParen (*)` 归约 lookahead 被
`DeclareTail` 的 first 污染 → shift/reduce 冲突，第 6 轮实证 346→0 冲突的收敛过程）。

**解决思路**：
1. **lexer 预处理（推荐）**：parse 前把 `declare` 行整体识别为"声明头 token"——
   保留原样文本，semantics 阶段二次解析。规避 grammar 的所有 epsilon 链。
2. **或者**：`DeclareDef` 每分支尾部用**必选单非终结符** `DeclareTail`（显式 4 分支：
   空 / attrs / attrs+#N / #N），且**绝不与裸分支共存**（裸分支并入空分支）。
3. display 同步：`fn fmt_func_head` 的 declare 分支补 attrs/group/metadata 输出。

**代码演示**（方案 2——显式 4 分支，已验证思路）：
```lalrpop
DeclareTail: (Vec<String>, Vec<(String, u32)>) = {
    <metas: MetadataAttach*> => (vec![], metas),
    <attrs: FuncAttrs> <metas: MetadataAttach*> => (attrs, metas),
    <attrs: FuncAttrs> <g: AttrGroupId> <metas: MetadataAttach*> => { … },
    <g: AttrGroupId> <metas: MetadataAttach*> => (vec![format!("#{g}")], metas),
};
DeclareDef: ParsedFunction = {
    Declare <fr: FuncRet> <name: GlobalId> LParen <params: ParamList> RParen <tail: DeclareTail> => …,
};
```
> ⚠️ 第 6 轮实测：`DeclareTail?`（可空）与裸分支共存必冲突；必须让 DeclareTail
> **必选**且空分支内含 `=> (vec![], vec![])`。若仍冲突，回退方案 1（lexer 预处理）。

**验收（✅ 已通过）**：`declare i32 @printf(ptr noundef, ...) nounwind` / `declare i32 @f(i32) #0` /
`declare void @f() !dbg !0` 三例 round-trip 已通过；`roundtrip_unnamed_declare_params` 已恢复 nounwind。

---

### 2.2 命名 metadata（`!t = !{...}` + `!dbg !t`）

**状态：✅ 已实现**——grammar.lalrpop:301 `ParsedItem::NamedMetadata(name, node)`
分支已恢复；lexer 以 `MetadataDefKw`（后跟 `=`）/`MetadataNameKw` 消歧（第二十一轮
MetadataFieldTupleLit 合并 + DI 校验器加固）；`!t = !{...}` + `define … !dbg !t`
round-trip 通过。

**历史现状（实现前）**：`ParsedItem::NamedMetadata(String, MetadataNodeDef)` 变体仍在 ast_items.rs:21，
但 grammar item 被移除（LALR 硬冲突）；`MetadataStore` 已预留
`define_named`/`lookup_named`/`name_of`（metadata.rs）。

**目标语法**：
```llvm
!t = !{i32 1}
define void @f() !dbg !t { … }
```

**难点**：`!t`（`MetadataNameKw`）同时是模块 item first 与 attach 的 name——
`(MetadataAttach)*` 闭包在 lookahead `MetadataNameKw` 时无法区分"下一个 attach 的 name"
与"下一个模块 item（命名定义）"。

**解决思路**：
1. **lexer 层消歧**：`!name =`（后跟 `=`）lex 成 `MetadataDefKw`；`!name`（后跟 `!`/行尾）lex 成
   `MetadataNameKw`——lexer 有 lookahead 能力（`logos` 的 `lex.suffix` 检查）。
   这是**纯 lexer 改动**，grammar 无需 epsilon。
2. semantics：`NamedMetadata` 定义先 intern → `define_named(name, id)`；
   attach/Val 的命名引用 `lookup_named` 解析为 id。
3. display：定义行输出 `!name = …`（`name_of(id)` 反查）；引用输出 `!name`。

**代码演示**（lexer 层）：
```rust
// lexer.rs —— 利用 logos 的 lookahead 消歧
#[regex(r"![a-zA-Z_][a-zA-Z0-9_]*", |lex| {
    let s = lex.slice().to_string();
    if lex.remainder().starts_with('=') { Token::MetadataDefKw(s) }
    else { Token::MetadataNameKw(s) }
})]
```

**验收**：`!t = !{i32 1}` + `define … !dbg !t` + `!{!t}` 嵌套引用 round-trip。

---

### 2.3 通用指令 metadata 附加（`add …, !tbaa !0`）

**状态：✅ 已实现**——BinaryOps/CmpOps/ExtractElement/ShuffleVector/Call/Fneg 等指令
规则均以 `CommaAttach*` 接收尾 metadata（第二十八轮复核确认）；`add i32 %a, %b,
!tbaa !0` round-trip 通过。

**历史现状（实现前）**：通用分支（`Ident InstFlagSeq TypeOps?`）的 `<metas: MetadataAttach*>` 被移除；
仅显式分支（call/load/store/函数头）支持 `, !dbg !N`。`!tbaa`/`!prof`/`!alias.scope`
等**最常用 metadata 都挂在算术/比较/内存指令上**。

**目标语法**：
```llvm
%r = load i32, ptr %p, align 4, !tbaa !0          ; ✓ 已支持
%x = add i32 %a, i32 %b, !tbaa !0                 ; ✗ 目标
%c = icmp slt i32 %a, %b, !prof !1                ; ✗ 目标
```

**难点**：`TypeOps` 尾逗号让 `(TypeOp Comma)*` 闭包与 `!tbaa`（`MetadataNameKw`）冲突。

**解决思路**：**tail 重写**——通用分支解析后，把 `TypeOps` 中**尾部的聚合字面量歧义**
消除掉。具体：通用分支改成两段式——
```lalrpop
// TypeOps 闭包后接 `Comma? MetadataAttach*`——但闭包吃不下 `Comma !tbaa`
// 方案：TypeOps 尾逗号显式分支（第 5 轮方案，曾因 LocalId 冲突回滚）
// 回滚根因是 LocalId 进入 TypeOps first；把 TypeOps 元素保持 ValueType 后重试
```
> 第 5 轮实测：`TypeOps: (Vec<ParsedOperand>, bool)`（尾逗号标记）在
> **去掉 Type 的 LocalId 分支**后可能可行——P0 优先级内**先做最小验证**：
> `icmp/fcmp`（CompareOp 分支，非通用闭包）直接加 `<metas: MetadataAttach*>`，
> 覆盖比较指令；通用算术指令留待 lexer 预处理。

**最小交付（推荐）**：CompareOp 分支 + load/store 已有 + `freeze` 等显式分支补 attach。

**验收**：`icmp slt i32 %a, i32 %b, !prof !0` round-trip。

---

### 2.4 终结符 metadata（`ret …, !range !0` / `br …, !prof !0`）

**状态：✅ 已实现**——`Terminator` 各变体（Branch/Jump/Return/Switch/Invoke/Resume）均
已带 `metadata: SmallVec<[AttachedMetadata; 2]>` 字段（terminator.rs:16-74；
Unreachable 免）；`ret …, !range !0` / `br …, !prof !1` round-trip 通过。

**历史现状（实现前）**：`Terminator` 枚举（terminator.rs:13）无 metadata 字段；
`ParsedTerminator`（ast_items.rs:324）同样无。PGO 的 `!prof`、`!range` 无法保留。

**目标语法**：
```llvm
ret i32 %x, !range !0
br i1 %c, label %t, label %f, !prof !1
```

**难点**：内部 `Terminator` 加字段 → 全仓匹配点（forge-opt 的 inline/jump_thread/
block_param_coalesce 等）+ codegen 的 `lower_terminator` 都要同步。

**解决思路**：
1. `Terminator` 各变体加 `metadata: SmallVec<[(String, MetadataId); 2]>`（Return/Jump/
   Branch/Switch 四个；Unreachable 免）。
2. `Function` 的终结符构造点（builder）默认空；文本层 parse 填、display 输出。
3. 全仓 `matches!(term, Terminator::Jump(_))` 模式改为 `Jump { target, .. }`——批量机械修改。

**验收**：`ret …, !range !0` / `br …, !prof !1` round-trip + workspace 全绿。

---

### 2.5 指令补全：`addrspacecast` / `va_arg`

**状态：✅ 已实现**——`AddrSpaceCast`（opcode.rs:189）/`VaArg`（opcode.rs:191）/
`LandingPad`（opcode.rs:193）已加入，llvm_mapping 双向映射齐全（现 opcode 共 109 个）；
`addrspacecast`/`va_arg` 文本层 round-trip 通过。`va_arg` 的 ABI 布局语义仍属 P1
（codegen 层），文本层存 `(Type, ptr)` 操作数。

**历史现状（实现前）**（opcode.rs 105 个）：`addrspacecast` 缺失（指针地址空间转换）、
`va_arg` 缺失（可变参数读取）、`freeze` 已有 opcode + 文本映射（llvm_mapping.rs:95/294）、`select` 已支持。

**目标语法**：
```llvm
%p2 = addrspacecast ptr %p to ptr addrspace(1)
%f = freeze i32 %x
%v = va_arg ptr %ap, i32
```

**实现要点**：
- `AddrSpaceCast` opcode + llvm_mapping 双向映射 + builder.`addrspacecast`（结果类型
  即目标 addrspace 的指针）；
- `VaArg` 语义需要可变参数布局（ABI 层）——**P1**，文本层先存 `(Type, ptr)` 操作数。

**验收**：`addrspacecast` round-trip + 负向测试（无 addrspacecast 的机器上 codegen 报错）。

---

### 2.6 linkage / visibility / dll storage 补全

**状态：✅ 已实现**——`DllImportKw`/`DllExport`/`AvailableExternallyKw` 等 lexer token
（lexer.rs:197-201），`GlobalSymKw` 全展开（link/dso/unnamed/tls/vis/dll 维度，
`SymbolInfo.dll_storage_class` 接通）；`weak hidden` 等组合 round-trip 通过
（第二十八轮复核确认）。

**历史现状（实现前）**：`Linkage` 枚举 12 种（symbol.rs:49：External/AvailableExternally/LinkOnceAny/
LinkOnceOdr/WeakAny/WeakOdr/Appending/Internal/Private/ExternalWeak/Common…），
但 grammar `GlobalSymKw`（grammar.lalrpop:311）只暴露 `private/internal/external`；
`dllimport/dllexport`（`DllStorageClass`）无文本通道。

**目标语法**：
```llvm
@g1 = weak global i32 0
@g2 = linkonce_odr global i32 0
@g3 = available_externally global i32 0
@g4 = dllimport global i32 0
```

**实现要点**：
- lexer 加 `Weak`/`WeakOdr`/`Linkonce`/`LinkonceOdr`/`Appending`/`AvailableExternally`/
  `DllImport`/`DllExport` token；`GlobalSymKw` 展开（注意 `(GlobalSymKw)*` 闭包数量膨胀——
  用**单闭包 + 语义分类**，参考 `LinkageSeq` 现有 4 维模式扩展到 6 维：link/dso/unnamed/tls/vis/dll）；
- `SymbolInfo` 已有 `dll_storage_class` 字段（symbol.rs）——接 `DllImport`/`DllExport`；
- display 按枚举还原。

**验收**：4 例全局 round-trip + 组合（`weak hidden` 等）测试。

---

### 2.7 参数 / 返回属性补全

**状态：✅ 已实现**——`ParamAttrs` 规则已扩（grammar.lalrpop:1395-1440：
signext/zeroext/noalias/noundef/readonly/writeonly/nocapture/nonnull/inreg/byval/sret/
align N + 带参开放属性 ParenAttrKw），`byval(%struct.S)`/`inreg`/`align 8` 组合
round-trip 通过（第二十八轮复核确认）。

**历史现状（实现前）**：`ParamAttributes` 14 字段（zeroext/signext/noalias/readonly/writeonly/byval/sret/
inreg/nocapture/nonnull/align/noundef/…），grammar `ParamAttrs` 只认 8 个；
`byval(Type)`/`sret(Type)`/`inreg`/`align N` 无文本通道。

**目标语法**：
```llvm
define void @f(ptr byval(%struct.S) %p, i32 inreg %x) { … }
call void @f(ptr nonnull align 8 %p, …)
```

**实现要点**：
- `ParamAttrs` 规则扩为 `(signext|zeroext|noalias|noundef|readonly|writeonly|nocapture|
  nonnull|inreg|byval|sret|align N)+`——**注意闭包**：`(X)+` 的 first 集合膨胀后与
  `ValueType`/`LocalId` 的 2-lookahead 风险，沿用显式分支展开；
- `byval(%struct.S)` 的 `Type` 参数 → `ParamAttributes::byval: Option<TypeId>`；
- display `fmt_param_attrs` 补 `byval(%t)`/`sret`/`inreg`/`align N`。

**验收**：`byval(%struct.S)`/`inreg`/`align 8` 组合 round-trip。

---

### 2.8 模块级 item 补全（source_filename / module asm）

**状态：✅ 已实现**——`SourceFilenameKw`/`ModuleAsmKw` 分支（grammar.lalrpop:325-326），
`source_filename = "…"` / `module asm "…"` round-trip 通过。

**目标语法**：
```llvm
source_filename = "test.c"
module asm ".globl _start"
```

**实现要点**：`Module` 加 `source_filename: Option<String>` + `module_asm: Option<String>`；
`ParsedItem` 加两个变体（显式分支，无闭包风险）；display 输出在 target 之前。

**验收**：两例 round-trip。

---

## 3. P1 — 内部表示扩展（需重构）

### 3.1 聚合常量 Value（嵌套提取 + insertvalue 聚合操作数）

**状态：✅ 已实现**——`ConstantPool.aggregates: Vec<AggConst>`（constant.rs:56-57，
`AggChild::Scalar(ConstId) | Agg(AggId)` 树形）；嵌套聚合字段提取已内存化
（compiler.rs:270-277，第十轮 S1：帧槽 + 段 store + `agg_slots` 登记，两级提取
参数/load 双路径端到端执行对照 clang）；`insertvalue` 聚合操作数 round-trip 通过。

**历史现状（实现前）**：`extractvalue [4 x i32] [i32 1, …], 2` 顶层标量提取已支持（第 6 轮，
序列化 tag 方案）；但**嵌套聚合元素**（`extractvalue [2 x [2 x i32]] […], 1` 的结果是
`[2 x i32]` 聚合值）与 `insertvalue` 聚合操作数仍报错——forge 内部**无聚合常量 Value**。

**目标语法**：
```llvm
%c = extractvalue [2 x [2 x i32]] [[2 x i32] [i32 1, i32 2], [2 x i32] [i32 3, i32 4]], 1
%n = insertvalue [4 x i32] %agg, i32 7, 2        ; %agg 是聚合常量 Value
```

**难点**：`ConstantPool` 目前只存 Int/Float/Vector/Byte；聚合需要**树形结构**。
影响面：DFG 的 `Immediate::Const` 消费方（const_fold/interpret/regalloc）都要支持聚合。

**解决思路**：
1. `ConstantPool` 加 `aggregates: Vec<AggConst>`，`AggConst { ty: TypeId, children: Vec<AggChild> }`，
   `AggChild::Scalar(ConstId) | AggChild::Agg(AggId)`；
2. `Immediate::Const` 保持标量；`extractvalue` 聚合 agg 的结果若为聚合 → 发
   `ExtractValue` 指令 + **结果类型=聚合**（值语义），codegen 仅在折叠时读取常量池；
3. verify/codegen 对"聚合类型 Value"的指令分类（load/store 允许，算术拒绝）。

**验收**：嵌套提取（结果是聚合）round-trip + const_fold 对常量聚合折叠 + 负向测试。

---

### 3.2 异常处理全套（invoke / landingpad / resume / catch 族）

**状态：✅ 文本层已实现（P1.1 完成）**——`Terminator::Invoke`（terminator.rs:53-63，
正常边 + unwind 边）、`Opcode::LandingPad`（opcode.rs:193）、`Terminator::Resume`
（terminator.rs:66）全链路（含 personality 透传、unwind 边 dominance 豁免，第二十八轮
落地）；clang `-fexceptions` 典型输出 round-trip 通过。catch 族（catchpad/cleanuppad
指令、catchswitch/cleanupret/catchret 终结符）**评估归档**——452 用例零命中、需 3 个
Terminator 变体扩展、无目标平台语义（第三十二轮记录）。codegen 对含异常函数仍报
`Unsupported`（P1.2 SjLj 属长期，见 §7）。

**历史现状（实现前）**：opcode 列表无 invoke/landingpad/resume/catchpad/catchswitch/
catchret/cleanuppad/cleanupret/indirectbr/callbr。这是**文本层最大的结构缺口**：
clang `-fexceptions` 输出含 `invoke` + `landingpad` + `personality`。

**目标语法**：
```llvm
define void @f() personality ptr @__gxx_personality_v0 {
  %entry:
    invoke void @g() to label %ok unwind label %lpad
  %lpad:
    %lp = landingpad { ptr, i32 } cleanup
    resume { ptr, i32 } %lp
}
```

**难点**：
- `invoke` 的终结符语义：正常边 + unwind 边 → `Terminator::Invoke { callee, args, normal, unwind, … }`；
- `landingpad` 是**非终结符**指令（异常值）；
- 异常边破坏 **SSA dominance**（unwind 块不在正常支配树内）——verify 需豁免；
- codegen 的 CFG 需要 unwind 边（Dwarf EH / SjLj 两套 ABI）。

**分期**：
1. **文本层 round-trip**（P1.1）：parse/display/verify 支持全部异常语法，内部
   `Terminator::Invoke` + `Opcode::LandingPad`；codegen 对含异常函数报
   `Unsupported`（明确错误）——保证"能读 clang -fexceptions 输出"；
2. **SjLj 代码生成**（P1.2，长期）：`setjmp/longjmp` 模拟，绕开平台 EH ABI。

**验收（P1.1 ✅ 已通过）**：clang `-fexceptions -S -emit-llvm` 的典型输出 round-trip（含 personality）；
含 invoke 的函数 `compile` 报明确 Unsupported。

---

### 3.3 常量表达式（gep/ptrtoint/bitcast 折叠）

**状态：✅ 已实现**——`GlobalInitVal` 常量表达式链（ptrtoint/inttoptr/bitcast/GEP）；
GEP 字节偏移折叠第三十一轮补全（semantics.rs:1101-1119：数组/向量索引 × elem_size、
结构按字段偏移累积，`size_of_parsed_type` helper）；`@g = global i32 ptrtoint (ptr @h
to i32)` 两例 round-trip 通过。

**历史现状（实现前）**：全局初始化只支持字面量/null/zeroinitializer/聚合字面量/cstring；
`@g = global i32 ptrtoint (ptr @h to i32)` 这类**常量表达式**缺失。

**目标语法**：
```llvm
@g = global i32 ptrtoint (ptr @h to i32)
@a = global [2 x ptr] [ptr @f, ptr @g]
```

**解决思路**：`GlobalInitVal` 加 `Expr(ConstExpr)` 变体，`ConstExpr` 树
（`PtrToInt/IntToPtr/Bitcast/GetElementPtr/AddrOf(Global)`）；display 递归输出；
常量池侧在折叠时求值（链接期解析）。

**验收**：两例 round-trip + `@g` 被 `GlobalAddr` 消费编译。

---

### 3.4 metadata kind 校验

**状态：✅ 已实现**——`MetadataKind` 现 14 种（metadata.rs:60-89，含 `Custom(ImmStr)`）；
`validate_metadata_shapes`（semantics.rs:483）对 attach 做 kind×形状校验（`!dbg` →
Named(DILocation)；`!tbaa` → 嵌套 tag 结构），非法组合（如 `!dbg !{i32 1}`）verify
报错。

**历史现状（实现前）**：`MetadataKind` 13 种（metadata.rs:91），但 semantics 对 attach 的
`!name` 只按字符串存（`metadata_kind_of` 13 名映射），**不校验节点形状**——
`!dbg` 指向 `!{i32 1}` 这种非法组合不会报错。

**目标语法**（校验）：`!dbg` 必须指向 `!DILocation(...)`；`!tbaa` 指向 `!{!{…}, i64 1}` 等。

**解决思路**：`attach_inst_metadata`/`attach_func_metadata` 后加 kind×形状校验
（`!dbg` → Named(DILocation)；`!tbaa` → 嵌套 tag 结构）；verify 阶段跑。

**验收**：合法组合通过 + 非法组合（`!dbg !{i32 1}`）报 verify 错误。

---

## 4. P2 — 测试与工具链强化

### 4.1 round-trip fuzz

**现状**：tests/display_llvm.rs 61 个手写用例（`assert_roundtrip`）。

**目标**：随机生成合法 LLVM IR（类型/指令/常量按比例抽样）→ parse → display → parse₂，
断言模块等价 + 常量池值等价 + 无 panic。种子固定（`--seed`）保证可复现。

```rust
// tests/roundtrip_fuzz.rs（草稿）
fn gen_inst(rng: &mut impl Rng, ctx: &mut GenCtx) -> String {
    match rng.gen_range(0..10) {
        0 => format!("%r{} = add i32 %a, %b, {}", ctx.fresh(), ctx.gen_flags(rng)),
        1 => format!("%r{} = load i32, ptr %p, align 4", ctx.fresh()),
        _ => format!("%r{} = icmp slt i32 %a, %b", ctx.fresh()),
    }
}
```
**验收**：10k 随机模块 round-trip 0 失败；发现的 bug 固定回归测试。

### 4.2 负向测试（非法 IR 拒绝）

**现状**：parse 报错路径覆盖零散。目标：`verify` 错误码全覆盖测试
（SelectCondNotBool/InvalidImmediate/越界/终结符缺失等）+
非法语法拒绝（`add i32`（缺操作数）/`ret`（缺类型）/未知 opcode）。

### 4.3 LLVM 官方 Assembler 测试子集

从 `llvm/test/Assembler/*.ll` 挑选**不依赖未支持特性**的用例导入为 round-trip 测试
（`cstring.ll`/`aggregates.ll`/`atomic.ll`/`vector.ll` 等），作为兼容性基准。
**验收**：导入 ≥50 个用例，标注跳过清单（异常/asm/元数据扩展）。

### 4.4 端到端执行对照

**现状**：forge-codegen 的 `tests/` 有编译冒烟；文本层无"parse→compile→执行"链。
**目标**：`text → parse → compile_raw → run` 对照 clang 输出（整数/浮点/SIMD 小程序），
验证文本层与 codegen 的语义一致。

---

## 5. 性能

### 5.1 lalrpop 生成状态与编译时间

`grammar.lalrpop` 约 1500 行，生成的 `grammar.rs` 大；每次改 grammar 全量重生成。
**候选**：拆分 grammar（模块级/指令级/类型级独立 `.lalrpop`）+ `lalrpop_mod!` 分别编译。

### 5.2 大模块解析基准

`parse_module` 对 10k 指令模块：`TypeContext` 的 RwLock 粒度、`operand_to_value` 的
`value_map` 查找、display 的 `self.types.borrow()` 重复借用。
**候选**：`benches/ir_parse.rs`（criterion）+ 热点分析后优化。

---

## 6. 附录：LALR(1) 限制备忘（历轮实证）

| 冲突形态 | 触发构造 | 规避手段 | 状态 |
|---|---|---|---|
| `(X)* X` 2-lookahead | `CallArgList`/`ParamList`/`StructList` + `LocalId` 开头 | 元素保持 `ValueType`；Named 用独立规则 | ✅ 已收敛 |
| epsilon 尾链 reduce/reduce | declare `FuncAttrs? grp? metas*` | 合并单非终结符或裁剪 | ✅ 已实现（2.1 DeclareTail 显式分支） |
| shift/reduce 状态污染 | `!t` 模块 item vs attach | lexer lookahead 消歧 | ✅ 已实现（2.2 MetadataDefKw/MetadataNameKw） |
| InstFlagSeq 闭包 | `Ident InstFlagSeq TypeOps?` + LocalId | TypeOps 元素 ValueType | ✅ |
| 模块 item first 膨胀 | TypeKw/TypeDef 加入 | 影响全局状态图——新增 token 后必全量回归 | ✅ 需保持警惕 |
| 裸全局引用 init 2-lookahead | `@g = global i32 @h`（init）vs `@g = global i32\n@h = ...`（下一条） | **无 LALR 解**——`@h` 后 lookahead `=` 才知归属，LLVM 自身靠上下文；带类型形式 `ptr @h`（GlobalConstExpr）可行 | ❌ 长期排除（4.1） |
| 尾选项 `Comma X` 部分消费 | `(Comma <Opt>)*` 中 Opt 失败回退不了已消费的 Comma | 每项以 **Comma 开头**整体匹配（AllocaOpt/GlobalTailOpt 模式）；Opt 内部不再含 Comma | ✅（本轮实证，4.3） |
| 可空规则双重嵌套 | `(X)*` 包在 `Y?` 内（GlobalTail 列表 + 可选） | 内层用 `(X)+`（至少一项），可空只留外层 `?` | ✅（本轮实证，4.3） |
| 终结符以 Ident 开头 | resume/invoke 用 Ident 与指令名冲突 | **专用 lexer token**（priority=2）使终结符 first 与指令 Ident 分离 | ✅（3.2 实证） |
| lalrpop 0.23 无 `ambiguous_grammar` | 旧文档的 `ambiguous_grammar` 宏已移除 | 冲突需重构语法（列表式/专用 token），无宏可依赖 | ✅ 已确认 |

## 7. 已排除项说明（长期/决策）

**仅支持最新版 LLVM IR**：旧式指针（`i32*`/`ptr*`——已拒）、`metadata !{}` 旧语法
（MetadataKw token 已拒）、autoupgrade 旧 intrinsic（`llvm.aarch64.thread.pointer`
等）均按旧格式长期排除。

- **内联汇编**（`call void asm sideeffect "…"`）：文本层已支持（第十六轮 AsmKw 专用
  token + call/tail/callbr 三种 asm 变体，约束串/副作用标识丢弃，+2 用例）；编码器
  集成仍属独立子项目（不做）。
- **`ret` 多返回值**（`ret { i32, i32 } %v`）：内部 `Return(Vec<Value>)` 已支持，
  文本层 `ret` 单值——多值语法（`ret i32 1, i32 2`）非 LLVM 标准，不做。
- **`declare` 的 `...` 可变参数**：✅ 已随 2.1 的 `DeclareTail` 一并支持——
  `printf(ptr noundef, ...)` 参数列表可变参数 round-trip 通过。
- **Windows SEH / ARM EH**：随 3.2 的 ABI 选择。
- **裸全局引用 init**（`@p = global ptr @h`）：LALR(1) 下与"下一条模块级 global"
  本质 2-lookahead 歧义（见 §6），长期排除；`ptr @h` 带类型形式可行（GlobalConstExpr）。
- **`<vscale x N x ty>` 可伸缩向量**：文本层已实现（第十二轮 lexer 单 token，
  lexer.rs:51-55——5-token 序列致 LALR 状态爆炸回滚后采用）；TypeId 扩展与
  codegen（SVE/RVV）仍不做（独立子项目）。
- **packed struct 语法补充说明**：`<{ i32, i32 }>` 已支持（2026-08 第四轮）；
  aggregate-constant-values 用例已全部通过（vscale 文本层已实现，非失败点）。
- **DI debug info 字段语义校验**（~46 个 `invalid-di*` 负向用例）：LLVM DI 验证器
  独立工程；文本层已支持 distinct/key:value（含布尔、DWARF 枚举、`!N` 数字引用——
  MetadataValPrim 子集规避 LALR 状态膨胀）、`type:`/`align:` 等 key；字段值域/必填
  校验不做——负向断言按防回归口径（非 0 上限）落地（4.3）。
- **verify 语义校验**：值域（int ≤2^23 / addrspace <2^24 / align ≤2^30）、
  atomicrmw/cmpxchg 类型与序、bitcast 大小（size_bytes）、insertvalue 值类型、
  可见性冲突（internal+hidden）、alias 重复、metadata 前向引用、comdat 声明等已校验
  （负向正确拒绝 208 → 第二十三轮终态 254）；单例缺口（getelementptr_struct 索引常量、
  global-init cast、alias 前向引用）已由第十轮 S4.1-S4.3 补齐（见 checklist §0.1）。
- **x86_64 聚合执行（2026-08 第五轮已解锁）**：`store {i32,i32} {...}` / `load` /
  `extractvalue` / `insertvalue`（≤8 字节，GPR 64 位域移位/掩码）端到端执行通过；
  `ret` 聚合（单/多元素按整数返回）与聚合传参（常量经打包展开、值经 GPR）全链路
  对照 clang 数值正确（3+4=7 等）。**修复的根因链**：聚合类型 bits()=0 → store 宽度 0
  （mem_opsize_for 改用 size_bytes）；AggConst 值从不加载（compile 期打包展开为
  i64 常量 store）；展开的 make_value 双 Value / inst_order 重复插入 / CompileState
  常量池时序错位（movabs 0）；zeroinitializer 数据段为空（JitCompiler 按类型大小
  零填充）；Call 结果 mov 32 位截断（default_opsize 取 size_bytes）。
- **Alloca（2026-08 第六轮已修复）**：无操作数 `alloca <ty>` 的 lowering 原是
  `lea rd, [rs1+rs2*1+0]`（无操作数时 rs1/rs2 为垃圾寄存器 → SEGV）；现预扫描
  分配帧槽（从 StackAddr 区之下连续、8 字节对齐、并入 max_stack_bytes），
  `lea_off rd, [rbp+disp]`。多 alloca 共存、聚合/数组槽执行通过。
- **GEP 寻址（2026-08 第六轮已修复）**：旧机器规则 `lea rd, [rs1+rs2*4+0]` 只支持
  单索引（scale=4 硬编码、多索引被忽略）；现 compile 期 `expand_geps` 展开为算术
  序列——常量索引折叠（struct 走 field_offset、数组走 elem_size×idx、第一索引
  作用于 indexed_ty 自身），动态索引（数组下标）展开为 `mul i64 %idx, size` +
  `add`，GEP 原地改 Copy（SSA 引用一致）。嵌套数组两级 GEP、结构体数组动态索引
  循环（pts[i].0 累加）执行通过。局限：struct 索引必须常量（LLVM 语义）、索引按
  无符号处理。
- **嵌套聚合字面量（2026-08 第六轮已修复）**：`[[i32 1, i32 2], ...]` /
  `{{i32 1}, i32 2}` parse 支持（GlobalAggElem 嵌套分支）+ pack_agg_init 递归打包；
  嵌套字段的 extractvalue（字段类型是聚合）**第十轮 S1 已内存化**（compiler.rs:270-277：
  帧槽 + 段 store + `agg_slots` 登记，两级提取端到端执行对照 clang）。
- **聚合 >8 字节（2026-08 第七轮已支持函数内）**：`store {i64,i64} {...}` 分段展开
  （pack_agg_bytes 字节布局 → 8/4/2/1 段宽 store + Iadd 地址推进）；大聚合 load 值
  传播（expand_large_aggs：store 拷贝逐段 load+store、extractvalue 偏移+标量 load、
  load 标 Nop）；大聚合 + GEP 寻址组合（数组元素/字段累加循环）执行通过。
  **残留**：load 快照语义（load 后内存被改写时重新 load 不保留旧值——常见
  模式安全；第十轮 S5.2 已在 expand_large_aggs 文档注释说明）。嵌套大聚合字段
  提取（字段本身是聚合）**第十轮 S1 已内存化**（compiler.rs:270-277），不再
  Unsupported。
- **大聚合 ABI（2026-08 第八轮已支持 ≤16 字节）**：SysV 整数寄存器路径全链路——
  调用方 `expand_agg_call_args`（聚合实参拆 i64 段：AggConst 段常量 / load 结果
  段 load），被调方 `expand_large_agg_params`（签名拆开为 ceil(size/8) 个 i64 参数
  + entry 块参数重建 + 使用处重写：extractvalue 段值移位、store 段 store、return/
  转传段值），`expand_large_agg_ret`（ret 大聚合 → Return 多值 RAX/RDX），
  `expand_large_agg_call_results`（调用方 call 结果拆 2 段 + 使用处重写——DSL 的
  rd/r2 双结果 mov 复用）。跨函数传参/返回全链路对照 clang 数值正确（3+4=7、
  make 返回 5/6 传 sum 累加 11）。**>16 字节仍 Unsupported**（栈传递未实现）。
  顺带修复的 ABI 相关 bug：pre_allocate_phi_vregs 的 Copy 复用/按结果类型分配/
  xreg_types 注册（FPR 与 GPR 编码别名 r15/xmm15 冲突）、Copy 特判写回 result 映射。
- **浮点字段（2026-08 第八轮）**：`extractvalue {f64,f64} %p, i` / `{f32×4}` 字段
  位模式转换——DSL 新谓词 `rs1elem`（区分源类型方向）+ [lower.Bitcast] 组合变体
  （rs1elem==I64&&elem==F64 → movq_to_xmm；F64→I64 → movq_to_gpr；同类型 → mov）、
  [lower.ExtractValue] F32/F64 变体、Fadd/Fsub/Fmul/Fdiv F32 变体（新 [inst.SS_FMOV]
  addss/subss/mulss/divss）、变体回退默认 insts（DSL：variants 全不匹配时用规则
  默认 insts——纯 variants 规则仍 Unsupported）。{f64,f64} 1.5+2.5=4.0、{f32×4}
  字段 1/3 提取消费执行通过。**已知限制**：fcmp float 仍走 comisd（double 语义）
  ——float 比较需 comiss（conditions 模板机制不支持类型分派，后续）——测试用
  位模式 icmp 替代。
- **verify GEP 索引校验（2026-08 第八轮）**：`check_gep_indices`——标量位置后续
  索引拒绝（"indexing into scalar"，LLVM 语义）、struct 位置索引必须 i32；负向
  误接受 51→50（getelementptr_struct 用例），正向 50 不变（无误伤）。
  **残留非 DI 误接受**：byref/inalloca 裸参数属性（LLVM 要求带类型参数——属性
  parse 层）、alloca addrspace 参数（语法层）；DI 类 ~40 保持 LLVM DI 验证器差距。
- **非 DI 负向清零 + 正向扩充（2026-08 第九轮）**：byref/inalloca 裸参数属性拒绝
  （LLVM 要求带类型）、alloca 参数顺序校验（addrspace 必须最后——官方负向用例
  alloca-addrspace-parse-error-1）；误接受 50→47（剩余全为 DI 类）、正确拒绝
  207→210。正向 50→57（+7 类）：align 2^32 上限放宽（align-inst）、`b<N>`
  bit-precise 整数类型（lexer 单 token + VecTy 元素）、captures/builtin/nobuiltin
  属性（开放集合 + 属性组联动）、`/* */` 块注释（logos skip）、数字/字符串/`$`
  前缀标签（`3:`/`%"2"`/`-N-`——LocalId 控制符排除修 invalid-name 误收）、
  datalayout P specifier、`externally_initialized` global 修饰。fcmp/icmp 第二
  操作数单类型（`fcmp olt float 1.5, 2.5`——CmpScalarValue 标量子集，聚合常量
  值 `<{...}>` 与 TypeOp 的 LALR(1) 本质冲突记录附录 §6）。shufflevector 掩码
  兼容 VecConstLit 合并格式（lane 带类型正则）。
- **浮点标量补齐（2026-08 第九轮）**：fcmp F32→comiss / F64→comisd 类型分派
  （DSL fcmp_cases 变体支持 + 每条件一条规则 + COMISS 指令——注意 F3 前缀在
  Windows JIT 触发 STATUS_ILLEGAL_INSTRUCTION，用无前缀 0F 2F）；f32 常量位
  模式存储修复（原按 f64 bits 存导致 comiss 读低 32 全零——semantics/display/
  hex 路径同步改 f32 位模式）；Fsqrt/Fabs/Fneg F32 变体（sqrtss/andps 0x7FFFFFFF/
  xorps 0x80000000——SQRTSS 指令）。float 直接比较 1.5<2.5、fsqrt(2.25)=1.5f、
  fabs(-2.5f)=2.5f、fneg(1.5f)=-1.5f 执行对照位模式全对。
- **残留（第九轮确认，均已落地）**：嵌套聚合字段提取——第十轮 S1 内存化
  （compiler.rs:270-277）；TypeOps 单类型第二操作数——第十轮 BinaryOp/BinaryOps
  专用规则（grammar.lalrpop:2814/2946）；vscale——第十二轮 lexer 单 token
  （lexer.rs:52）；opaque 类型——第十轮 `TypeEntry::Opaque` 全链路。仅
  determinism 测试在 workspace 并行下偶发一项已由第十轮 S5.1 定位（Windows 高
  并行 cargo 链接/rmeta 竞争，`-j 4` 下全量稳定全绿）。
- **operand 嵌套聚合字面量（2026-08 第七轮已修复）**：`store [2 x {i64,i64}]
  [{i64 1, i64 2}, ...]` 等 parse 支持（AggListElem 嵌套分支——非空嵌套；空 `{{}}`
  与"空 struct 类型+值"的 TypeOp 本质 LALR 歧义，排除）。
- **浮点字段 extractvalue/insertvalue**：位模式在 GPR 但后续 FPR 消费需 movq
  转换——Unsupported（规则不匹配自动拒绝）。
