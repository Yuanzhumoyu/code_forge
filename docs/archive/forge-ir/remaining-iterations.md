# 剩余迭代内容总清单（forge-ir LLVM 汇编兼容）

## ⚠️ ARCHIVED（2026-09）

> forge-ir 剩余迭代行动版（第 18–23 轮；452 收敛后已无剩余迭代）。
> 本文为历史记录，仅供参考；代码现状以仓库代码与现行文档为准，不再维护。
> 本文档是 `docs/archive/forge-ir/iteration-checklist.md` 的**后续行动版**：后者记录已完成的
> 第十轮～第十七轮执行结果，本文档列出**接下来所有需要迭代的内容**——每个迭代项
> 含问题解析、完整代码演示、解决思路、风险与验收命令。
>
> 基线：compat 正向 **198/452**（第十八轮 +5、第十九轮 +3、第二十轮 +2、
> 第二十一轮 +8、第二十二轮 +2、第二十三轮：IntLit Big 化重构 + DIExpression
> 状态机 + target 布局语义 + summary 校验——误接受 5→0）、
> 负向正确拒绝 **254** / 误接受 **0**。
> **🎯 452 个用例全部收敛且误接受清零：198 + 254 + 0 = 452，正向失败 0、误接受 0。**

## §0 现状总览

| 指标 | 值 |
| --- | --- |
| 正向 parse 通过 | 198/452 |
| 负向正确拒绝 | 254 |
| 负向误接受 | **0**（上限——断言 `neg_ok <= 80`） |
| LALR 冲突 | 0（硬约束：任何改动不得引入） |
| workspace 全量 | 全绿（`cargo test --workspace --exclude forge-rustc -j 4`） |

**剩余迭代内容：无。** 全部 LLVM 官方 test/Assembler 用例收敛。

---

## §1 第二十三轮落地摘要（误接受清零轮）

### 23.1 IntLit 任意精度 Big 化重构（+ 附带修复 3 例）

**问题解析**：`!DIExpression(18446744073709551616)`（超 u64::MAX）应拒
"element too large, limit is 18446744073709551615"；正向
`18446744073709551615`（u64::MAX）合法。旧 `IntLit(i64)` 溢出折叠 i64::MIN
哨兵,两值不可区分且 display 丢原文。

**落地改动**（采纳 Big 类型方案,dashu 任意精度）：

```rust
// ① lexer.rs:IntLit payload 改 Big（dashu Integer,任意长度十进制）
#[regex(r"-?[0-9]+", |lex| {
    let s = lex.slice();
    crate::big::Big::Signed(s.parse::<dashu::Integer>().unwrap_or_default())
})]
IntLit(crate::big::Big),
// ② ast_items.rs:转换 helper（grammar reducer 用,超界 → 上限哨兵,
//    各值域校验自然拒绝）
pub fn big_as_i64(n: &Big) -> i64 { i64::try_from(n.clone()).unwrap_or(i64::MAX) }
pub fn big_as_u32(n: &Big) -> u32 { /* 同理 */ }
pub fn big_as_u64(n: &Big) -> u64 { /* 同理 */ }
// ③ grammar.lalrpop:53 处消费点错误驱动改（align/addrspace/uselistorder
//    idx/extractvalue/Array/AddrSpaceVal/SwitchCase 等）
// ④ MetadataVal/MetadataValue 加 IntBig 变体（IR 层存原文 ImmStr 满足 Hash,
//    display 精确输出）;MetadataValPrim 分支:
<n: IntLit> => match i64::try_from(n.clone()) {
    Ok(v) => MetadataVal::Int(v),
    Err(_) => MetadataVal::IntBig(n),
},
// ⑤ check_di_node:check_range 遇 IntBig 必拒;DIExpression 元素值域
//    [0, u64::MAX]（超限拒、u64::MAX 合法——Big 精确比较）;
//    lowerBound lo 恢复 i64::MIN（哨兵法废弃）
```

**附带收益**：invalid-disubrange-count-large / lowerBound-max / lowerBound-min
三例哨兵法用例的校验逻辑简化为精确值比较（Big 化后自然区分）。

**验收**：误接受 5→4（diexpression-large 回正）,三例 disubrange 回归修复,
DIEnumeratorBig 等大数正向 roundtrip 回归绿,0 冲突。

### 23.2 DIExpression 操作码状态机

**问题解析**：`!DIExpression(0, 1, 9, 7, 2)` 的数字操作码序列——LLVM
dwarf::Operation 合法数字域为 LLVM 扩展区（≥4096）,0/1/9 非法;正向用例
全部名字操作码（grep 实测 10 个全集）。

**落地改动**（check_di_node DIExpression 分支）：

```rust
const OPS: &[(&str, u32)] = &[
    ("DW_OP_deref", 0), ("DW_OP_xderef", 0), ("DW_OP_plus", 0),
    ("DW_OP_swap", 0), ("DW_OP_plus_uconst", 1), ("DW_OP_constu", 1),
    ("DW_OP_stack_value", 0), ("DW_OP_LLVM_fragment", 2),
    // convert 2 参（bits + DW_ATE_* 编码枚举名）
    ("DW_OP_LLVM_convert", 2), ("DW_OP_LLVM_tag_offset", 1),
    ("DW_OP_LLVM_entry_value", 1), ("DW_OP_LLVM_arg", 2),
];
// 状态机：need_op 状态——Str 查白名单(带 arity)、数字 < 4096 拒绝;
// need_args 状态——数字/名字(DW_ATE_* 枚举)消费计数
```

**验收**：误接受 4→3（diexpression-verify 回正）,正向 diexpression.ll
（convert 2 参形态）回归修复,0 冲突。

### 23.3 target 布局语义（target-type-mixed）

**问题解析**（初版假设修正）：LLVM TargetExtType 参数**无需**前缀字母——
正向 target-type-mangled/params 用裸参数且 "a" 布局合法;真实规则：仅
注册布局名 "type" 的裸 IntLit 参数拒绝（"expected uint32 param"）。

**落地改动**（grammar.lalrpop）：

```lalrpop
TargetArgList: (String, bool) = {   // (布局名, 含裸 IntLit 标志)
    => (String::new(), false),
    <s: StrLit> <rest: (Comma TargetArg)*> => (s, rest.iter().any(|(_, b)| *b)),
    <f: TargetArg> <rest: (Comma TargetArg)*> => (String::new(), f || ...),
};
TargetArg: bool = { <_t: Type> => false, <_n: IntLit> => true };
// Type 的 Target 分支 =>? 校验:布局名 "type" 且含裸 IntLit → ParseError::User 拒绝
```

**验收**：误接受 3→2,正向 target-type-mangled/params 回归修复（首版收紧
误杀后回滚重设计）,0 冲突。

### 23.4 summary 条目校验（summary ×2）

**问题解析**：`^0 = gv: (name: "does_not_exist")` 引用未定义全局应拒;
`define void @foo()` + `^1 = gv: (name: "foo")` 函数定义缺 value info
（summaries 子句）应拒;lexer 原 skip 整行丢弃。

**落地改动**：

```rust
// lexer.rs:skip 去掉 ^N 行,加行级合并 token（原文保留）
#[regex(r"\^[0-9]+ = [^\r\n]*", |lex| lex.slice().to_string(), allow_greedy = true)]
SummaryKw(String),
// grammar:模块级 ParsedItem::Summary(s) 分支
// semantics:仅 gv: 条目校验——name 必须已定义（引号全局名归一化匹配）;
//           name 是 define 函数且条目无 "summaries:" → 拒绝;
//           typeidCompatibleVTable 等条目引用外部 RTTI 符号合法（跳过）
```

**验收**：误接受 2→**0**,正向 summary 用例（summary-flags/asm-path-writer
/index-value-order 引号名等）回归修复,0 冲突。

---

## §2 LALR 冲突案例集（历轮实测，避免重复踩坑）

### 2.1 命名符号在嵌套括号（最高频）

lalrpop 规则里 `( ... <x: T> ... )?` 的 `<x: T>` 报
`named symbols like x:T are only allowed at the top-level`。

```lalrpop
// ❌ 错误
<addrsp: (AddrspaceKw LParen <n: IntLit> RParen)?>
// ✅ 正确
<addrsp: (AddrspaceKw LParen AddrSpaceVal RParen)?>   // 无命名；或
AddrSpaceVal: i64 = { <n: IntLit> => n, <_s: StrLit> => 0 };  // 辅助规则
```

### 2.2 可空闭包与后续 first 的重叠

`(A Comma)* A` 模式（2-lookahead）→ 改 `A (Comma A)*`（左到右）。

### 2.3 裸 GlobalId 的模块级歧义（T3——第二十一轮 lexer 合并解决）

```text
GlobalInit = GlobalId        // `global ptr @h`
GlobalDef  = GlobalId Eq ... // 下一行 `@h = ...`
```

GlobalId 后 lookahead=Eq vs 换行/Eof——LALR(1) 无法区分。**解法**：lexer 合并
`global ptr [addrspace(N)] @ref` 单 token（形态限定 ptr,无误吞）。

### 2.4 专用 token 与 Ident 开放分支的冲突

- **解法**：lexer 合并专用 regex（dereferenceable/riscv_vls_cc/denormal_fpenv/
  ParenAttrKw/DefineLinkageKw/DeclarePrefixMdKw/ElementtypeFnKw/GcLiveKw/
  GlobalPtrInitKw/MetadataFieldTupleLit/SummaryKw 全部成功先例）。

### 2.5 TypeOp 的 metadata 分支与 CommaAttach 尾随

TypeOp first 含 MetadataKw 时产生 shift/reduce 歧义（CallArg 局部加分支）。

### 2.6 ValueType 加宽分支的状态爆炸

- ValueType 加 `LocalId` → 1445 冲突；加 `Target` → 2052；加 `TypedPtrKw` → 2426。
- **解法**：类型扩展只在 **Type 层**,指令操作数用局部规则（TypeOp/CallArg/
  Ret/Store 专用分支——18/19 轮 TypeOp 加 Target/TypedPtrKw 均 0 冲突先例）。

### 2.7 logos 的 #[regex] 绑定后一个变体

```rust
// ❌ GlobalId(String) 声明在前、regex 在后 → regex 绑定 TypedPtrKw
// ✅ regex 属性紧跟其变体声明
```

### 2.8 可空后缀与共享前缀的 ε 冲突

invoke 可空后缀 `<_gc: (LBracket ...)?>` 与无后缀变体并存 → 冲突。**解法**：
后缀经 lexer 合并为必选单 token 分支（GcLiveKw）。

### 2.9 指令级 metadata 必须 CommaAttach

atomicrmw/cmpxchg 曾用 `MetadataAttach*`（无逗号）→ 指令级一律 `CommaAttach*`。

### 2.10 裸 LBrace 在 metadata 值位置与 Type 的冲突（21 轮新例）

MetadataValPrim 加裸 `LBrace ... RBrace` 分支与 `<_ty: Type>` 分支的匿名结构
LBrace 冲突。**解法**：lexer 合并 `ident: {...}` 整段 token。

### 2.11 #dbg 参数 grammar 化（21 轮新例）

 #dbg 整行 lexer 合并时参数形状校验不可行;拆 6 个前缀 token + 参数化 grammar
分支——正向 declare_value/assign 7 参形态需完整覆盖（两轮回归教训）。

### 2.12 TargetArgList 首参 StrLit 双分支冲突（23 轮新例）

TargetArgList 的 `<s: StrLit> <rest>` 首参分支与 TargetArg 的 StrLit 分支在
lookahead=StrLit 时冲突。**解法**：StrLit 仅首参合法——从 TargetArg 移除,
首参分支独占。

---

## §3 负向纪律规则（铁律）

1. **上限 80**：`neg_ok <= 80` 断言。任何新语法/校验使误接受超 80 → 立即回滚。
   （第二十三轮实际基线 **0**——新增语法/校验不得使误接受回退变差。）
2. **回滚先例**：comdat 无名字（+1,20 轮语义配套零净增）、ptrauth 四参数
   （+2,19 轮值级校验配套）、Prim Tuple 值（+8,21 轮 lexer 合并方案）、
   split-file 无条件预处理（+26,22 轮仅正向后安全）、target 参数收紧
   （误杀正向,23 轮回滚改布局名语义）——语法宽松的代价都是负向计数。
3. **宽松与校验的分层**：语法层宽松（parse 通过优先）→ 语义层值级校验
   （ptrauth/range/linkage+visibility/call addrspace/递归类型/前向引用类型/
   unnamed comdat/DI 批量校验器/uselistorder/dialect/DIExpression 状态机/
   target 布局语义/summary gv 条目）→ 负向用例专项回归。
4. **每次改动后三查**：`cargo check` 0 冲突 → 目标用例 probe → 负向计数不增。

## §4 验收命令速查

```bash
cargo check -p forge-ir                                   # LALR 冲突（硬约束 0）
cargo test -p forge-ir --test llvm_assembler_compat -- --nocapture   # 兼容统计+断言
cargo build -p forge-ir --example probe && ./target/debug/examples/probe.exe   # 单用例
cargo test -p forge-ir                                    # 全量回归
cargo test --workspace --exclude forge-rustc -j 4         # workspace 全量（交付前必跑）
```

## §5 最终状态

| 轮次 | 内容 | 净效果 |
| --- | --- | --- |
| 第十八轮 | T1-1 位宽 u32 全链、T2-1/2/3 | +5 |
| 第十九轮 | T4-3 ptrauth、T5-1/2 TypedPtrKw、P3 校验 | +3、误接受 -7 |
| 第二十轮 | T4-1 large-comdat、T4-2 CommaAttach、递归/前向校验 | +2、误接受 -4 |
| 第二十一轮 | DI 批量校验器、T3×4、dbg-record、判定修正、uselistorder | +8、误接受 67→6 |
| 第二十二轮 | split-file 预处理（仅正向）、dialect 白名单 | +2、误接受 -1 |
| 第二十三轮 | IntLit Big 化、DIExpression 状态机、target 布局、summary | 误接受 5→**0** |

> 注：第二十一轮行的"误接受 67→6"——67 为该轮**开工时实测基线**（承接第十七轮
> 上限 80 经第十八~二十轮负向纪律/校验收口的中间值，非逐轮累计推导；该轮 DI
> 批量校验器一次性清零至 6，正确拒绝计数随之非单调跃升）。

**终态：正向 198/452、正确拒绝 254、误接受 0、FAIL 0——452 个用例全部收敛,
误接受清零。**
