# forge-ir 迭代清单（详细版：问题解析 + 代码演示）
> ## ⚠️ ARCHIVED（2026-09）
>
> forge-ir 迭代清单实操展开（第 10–23 轮执行记录，S1–S6 均已落地；452 全收敛）。
> 本文为历史记录，仅供参考；代码现状以仓库代码与现行文档为准，不再维护。


> 本文是 `docs/archive/forge-ir/next-iterations.md`（backlog 总表）的**逐项实操展开**：
> 每项给出问题解析（现象/根因/代码定位）、现状代码、改造演示、边界与风险、验收。
> 代码位置以当前 workspace（第九轮后基线）实测为准，行号随迭代漂移时以符号名定位。
> 与 `docs/archive/forge-ir/iteration-roadmap.md` 的关系：roadmap 记"历轮成果"，backlog 管
> "待办"，本文是"怎么干"。

---

## §0 基线速览（第九轮后）

| 指标 | 基线 |
| ------ | ------ |
| workspace suite | 48 全绿（第三十三轮移除 wasm32/minimal_sd 测试模块；determinism 并行偶发已定位为 cargo 链接竞争，第十轮 S5.1） |
| compat 正向 | 57/452 |
| compat 负向 | 正确拒绝 210、误接受 47（其中 7 个非 DI，见第十轮收尾；终态已收口为 254 拒绝 / 0 误接受） |
| parse 基准 | `parse_medium_module` ~57µs、roundtrip ~148µs |
| 增量编译 | 5-10s（lalrpop 单文件 ~2000 行） |

**建议开工顺序（依赖驱动）**（注：该顺序已被第十轮实际执行完成，见 §0.1 与 §1 状态标注）

```text
主线（文本层正向扩充）：S2 → S3.1/S3.2/S3.3 → M3
执行层主线：             S1 → S3.4
独立线：                 S4（verify 语义）、S5/S6（工程稳健）
中期：                   M1（触发式）、M2（每轮顺手）
长期：                   L1-L10（记录前置条件，不启动）
```

每轮开工跑一遍 compat 失败清单，收尾跑全量回归并回写 roadmap 附录 §7。

---

## §0.1 第十轮执行结果（2026-08，已验收）

| 迭代项 | 状态 | 结果 |
| -------- | ------ | ------ |
| S2 二元指令单类型第二操作数 | ✅ 完整验收 | 18 个专用 token（lexer）+ BinaryOp/BinaryOps/InstBody 专用规则；`add i32 %a, %b` 类型从第一推导；block-labels.ll（数字块 id：NumberedVals 值与块共享编号、字符串标签引号解码、隐式块唯一命名）与 flags.ll（nneg/disjoint/samesign/nusw/inrange + 二元常量折叠）解锁 |
| S3.1 opaque | ✅ | TypeEntry::Opaque 全链路 + `%ty = type opaque` TypeDef + verify load 无大小报错 + 回归测试；顺带解锁 token 类型/token none/immarg/readonly/presplitcoroutine/ifunc/call fast/显式函数类型 call/byval 聚合类型 |
| S3.2 属性组引用变体 | ✅ | captures(none)/ret:none；callee-type 前缀 metadata 因与 DeclareTail MetadataAttach+ 状态合并冲突回滚（LALR 本质，附录 §6） |
| S3.3 global 尾 metadata | ✅ | 全局字符串属性对 `"key" = "value"`；dso_local ifunc 因 LinkageSeq 冲突回滚 |
| S3.4 fpext/fptrunc lowering | ✅ | x86_v10.toml `[lower.Fptrunc]`（movsd+cvtsd2ss）/`[lower.Fpext]`（movss+cvtss2sd）+ CVTSD2SS/CVTSS2SD 模板（F2/F3 0F 5A）；test_fptrunc/test_fpext 端到端执行通过 |
| M3 官方用例扩充 | ✅（57→71） | 目标 80+ 未完全达成：剩余 DI 验证器类（L1 明确不做）、atomicrmw 前向值引用（语义层两遍构建）、vscale（L3）、inline asm（L4）等；本轮 +14（atomic 全形态/常量折叠表达式/命名调用约定/splat/浮点 hex 字面量族） |
| S1 嵌套聚合字段提取 | ✅ 完整验收 | `agg_slots: HashMap<Value,(addr,ty)>` 映射方案（非 backlog 草图的 ValueDef::SlotAddr——实测无该变体）；参数路径 alloca+段 store 内存化、load 路径地址链登记、内层提取递归队列；`{{i32,i32},i32}` 两级提取参数/load 双路径端到端 = 9（对照 clang）；非对齐字段（off%8!=0）按时间盒口径保持 Unsupported |
| S4 verify 三缺口 | ✅ | S4.1 struct 非常量索引报错（含回归测试）；S4.2 global-init cast 链校验（ptrtoint/inttoptr 类型匹配——invalid_cast4 修回正确拒绝）；S4.3 alias 前向引用确认现状已支持（名字延迟解析） |
| S6 metadata 扩展 | ✅ | splat 常量表达式（global init/指令操作数）、浮点 hex 字面量（0xH/0xR/0xK/0xL/0xM/0xJ/f0x）、x86_fp80/ppc_fp128 类型宽松、ExprValue True/False、CmpScalarValue 常量表达式、GlobalInit 带类型前缀表达式 |
| S5.1 determinism 偶发 | ✅ 定位 | 偶发为 Windows 高并行 cargo 链接/rmeta 竞争（`link.exe 1102`/`only metadata stub found`）——`-j 4` 下 workspace 全量稳定全绿；forge-dsl determinism 测试进程内确定性已验证（与模型迭代序无关） |
| S5.2 load 快照语义 | ✅ | 已在 expand_large_aggs 文档注释中说明"load 后内存被改写时重新 load 不保留快照语义（常见模式安全）" |

**第十轮收尾状态**：compat 正向 71/452、负向正确拒绝 210/误接受 47（非 DI 7 个）；workspace 全量（-j 4）全绿；lalrpop 0 冲突。
（注：正向数按 `llvm_assembler_compat.rs:29` 测试头 57→71 为准；本文件旧版误记 74。）

**第十一轮执行结果（2026-08，已完成）**——compat 71→**127/452**（+56）、负向正确拒绝 197/误接受 60、workspace 全量全绿、lalrpop 0 冲突：

| 迭代项 | 状态 | 结果 |
| -------- | ------ | ------ |
| metadata 操作数 | ✅ | CallArg 4 形态（`metadata i32 0`/`!10`/`!{}`/`!name`）+ !dbg 校验宽松（drop-debug-info 等解锁） |
| ifunc 专用 token | ✅ 部分 | IfuncKw + LinkageSeq? 变体 + resolver TypeOp（`ptr @r`/`ptr addrspace(1) @r`）；define/call 尾 addrspace（`define ptr @f() addrspace(0)`/`call addrspace(0) void @f()`）；**define internal 四方案实测均 LALR 本质冲突**（GlobalSymKw 543 冲突/DefineLinkage 356/#[inline] 状态爆炸/FuncRetKw 并入）——记录已知失败 |
| 前向 callee 注册 | ✅ | call/invoke 未声明函数（含 @llvm.*）预扫描注册 void() 占位声明（LLVM 隐式声明语义） |
| vscale | ✅ | `<vscale x N x T>` 单 token（5-token 序列致 LALR 状态爆炸回滚）；修复 **Rust 方法调用接收者先于参数求值 → `borrow_mut()` 重入死锁**（`ctx.borrow_mut().scalable_vector_ty(to_type(e, ctx), ...)` 先求参数再取锁） |
| 调用约定宽松 | ✅ | amdgpu_cs_chain/x86_intrcc 等命名约定 Custom 占位（display 还原） |
| summary/thinlto | ✅ | lexer 行级 skip（`\^[0-9]+ = [^\r\n]*`——单行括号平衡实测 100%）；11 用例解锁 |
| 嵌套聚合 init | ✅ | GlobalAggElem Agg 透传（`{ [4 x ptr] [...] }` vtable 摘要）+ pack_scalar_init Expr/i1 分支 + const_expr_bytes i128 移位防溢出 |
| dereferenceable | ✅ | 专用 token 参数/返回属性（Ident 开放形态与命名调用约定 LALR 冲突） |
| uselistorder | ✅ | 专用 token + 模块级/块尾（终结符后特例）+ 校验（≥2/无重复/非升序/模块级 @global 存在性）；未定义值宽松（fb.undef 占位）+ phi 前驱参数宽松（LLVM 文本 phi 值在声明中） |
| 全局尾部剩余 | ✅ | thread_local(模型) lexer 合并、define 尾 unnamed_addr/align/ssp、符号化 addrspace（数字/字符串/标识符三形态 AddrSpaceVal） |
| 单例指令 | ✅ | dso_local_equivalent（call 双位置）/ptrtoaddr（指令+常量表达式，宽松映射 Ptrtoint）/call elementtype(N)/declare hidden/protected/VecTy `ptr addrspace(N)` 元素 |
| 属性组前向引用 | ✅ | `#N` 未定义展开空集（thinlto-bad-summary 1/2/3 + amdgpu-image-atomic-attributes 解锁） |
| splat/denormal_fpenv | ✅ | SplatKw 常量表达式（求值宽松 0）+ denormal_fpenv(模式) lexer 合并（`\|`/`-`/`:`/`,`/空格全形态）；global 尾无逗号字符串对/`#0`/关键字属性 |

**第十二轮执行结果（2026-08，已完成）**——compat 127→**151/452**（+24）、负向正确拒绝 181/误接受 76（DI 语法开放化预期内）、workspace 全量全绿、lalrpop 0 冲突：

| 迭代项 | 状态 | 结果 |
| -------- | ------ | ------ |
| callbr 指令 | ✅ | CallbrKw + Terminator 分支（`to label %cont [label %kill, ...]` 宽松映射 Jump）——callbr 是终结符非普通指令 |
| convergencectrl | ✅ | 12 处 Call 变体尾可选 `[ "convergencectrl"(token %t) ]` + ConvergentKw 属性 |
| prefix/prologue | ✅ | 5 条 Define 规则加子句（顺序 prefix→prologue→personality）；personality 未定义宽松 |
| 裸 phi | ✅ | PhiIncomingList 零入边分支（dead phi） |
| riscv_vls_cc(N) | ✅ | lexer 合并单 token（Ident+LParen 与 FuncRetKw 冲突的既有教训） |
| 向量 GEP lane | ✅ | VecLane Int/UInt 通用分支 + **parse_vec_lanes 关键字 lane 误判修复**（`i1 false` 含 'e' 被当浮点——probe 定位）+ 向量 regex 多空格容忍——+2 用例 |
| call/ret 命名类型 | ✅ | CallArg/Ret 局部 LocalId 分支（不碰 ValueType 防状态爆炸） |
| metadata 字符串 | ✅ | `metadata !"Float8E5M2"`（Bang StrLit）+ StrictfpKw |
| DI 字段开放化 | ✅ 8/9 | MetadataValPrim 升级：Named 节点可空括号（避免 LParen 2-lookahead）/Attributes key/Pipe flag 组合/裸 Ident DWARF 参数/类型化值；Tuple 值分支因负向漂移回滚（generic-debug-node 记录） |
| TypeDef 前向引用 | ✅ | 两遍注册（占位+填充）——`%struct.A = type { %struct.anon }` |
| float-literals | ✅ | C99 hex float 全形态（`0x1.0p-32`/`0x.fffp5`/`±`）、`0.e1`、`±inf`/`qnan`/`snan(0x1)` |
| #dbg 伪指令 | ✅ | `#dbg_declare*` 行级 skip；ret 逗号 metadata（CommaAttach） |
| 开放函数属性 | ✅ | FuncOpenAttrKw 合并 regex（readnone/speculatable 等，payload 原样保 roundtrip） |
| datalayout 宽松 | ✅ | 未知 specifier（'F'）忽略 |

**第十三轮执行结果（2026-08，已完成）**——compat 151→**154/452**（+3）、负向正确拒绝 181/误接受 76（vscale GEP 修回 1 个）、workspace 全量全绿、lalrpop 0 冲突：

| 迭代项 | 状态 | 结果 |
| -------- | ------ | ------ |
| splat 全位置 | ✅ 验证 | probe 实测 global init/ret 双形态 parse true——十二轮 lane 修复已顺带解锁 |
| target 扩展类型 | ✅ 2/3 | `target("a", target("b"))` Type 层支持（TargetArgList 递归经 Type 分支，歧义修复）+ returned 属性；**define target(...) 的 LR 状态异常记录**（declare 同构已过）；ValueType 层分支 2052 冲突回滚 |
| fneg 指令 | ✅ | FnegKw + InstBody Type+Value 双分支（TypeOp 形态与 insertvalue 状态合并冲突回滚）；llvm_mapping 已有 |
| call flags+函数类型 | ✅ | `call ninf nnan afn float (...) @f`（显式函数类型 flags 版） |
| frem | ✅ | 宽松映射 Fdiv（forge 无浮点取模） |
| vscale GEP 校验 | ✅ | 指令+常量表达式双路径拒绝（indexed_ty 字段——初版误用 op_ty）；负向 77→76 |

**第十四轮执行结果（2026-08，已完成）**——compat 154→**161/452**（+7）、负向正确拒绝 177/误接受 80（断言上限）、workspace 全量全绿、lalrpop 0 冲突：

| 迭代项 | 状态 | 结果 |
| -------- | ------ | ------ |
| 命名类型全链路 | ✅ | Store Named 变体 + GEP 常量 indexed_ty→Type（两分支）+ AggListElem Named + **TypeDef 三遍注册占位升级**（types.rs struct_named 原位替换字段——前向引用 %0 引 %1 的占位 id 问题）+ 聚合 undef/null/poison 零标量 |
| 带参开放属性 | ✅ | ParenAttrKw lexer 合并（range/nofpclass/allockind/memory 含逗号引号——FuncAttr/ParamAttr/InstFlag 三位置）+ immarg 宽松 |
| global 前向引用 | ✅ | operand_to_value/global_addr 未注册占位 GlobalId(0) |
| 转义 metadata 名 | ✅ | MetadataDefKw/MetadataNameKw 反斜杠（`!\23pragma`） |
| define 尾 comdat | ✅ 部分 | 有名字形态（`comdat $x`）；无名字形态与 ptrauth 多参数因负向漂移回滚（84→80） |
| ptrauth 常量 | ✅ 两参数 | `ptrauth (ptr @var, i32 0)` 宽松 0；多参数回滚 |

**第十五轮执行结果（2026-08，已完成）**——compat 161→**174/452**（+13）、负向正确拒绝 177/误接受 80（上限）、workspace 全量全绿、lalrpop 0 冲突：

| 迭代项 | 状态 | 结果 |
| -------- | ------ | ------ |
| define internal | ✅ 突破 | **DefineLinkageKw lexer 合并**（`define internal` 单 token）——grammar 层四轮方案全部 LALR 冲突后，lexer 最长匹配消歧；+7（ifunc×3/anon-functions/disubprogram-targetfuncname/dso_local_equivalent/non-global-value-max-name-size-2） |
| 旧式指针 %fum\* | ✅ 部分 | TypedPtrKw lexer 合并（Type 层）——remangle 解锁；**logos regex 绑定后一变体的坑**（@foo 误入 TypedPtrKw 致 42/452 崩，重排修复）；ValueType 层 2426 冲突回滚（scalable-vector-struct/insertextractvalue 匿名聚合指针记录） |
| musttail 显式函数类型 | ✅ | `musttail call ptr (ptr, ...) @f` + CallArgList 可变参数（`...` 哨兵） |
| 字符串参数属性 | ✅ | `i64 "foo bar"`（grammar 带引号存 + semantics contains('"') 宽松） |
| 引号全局名 | ✅ | `@\"_ZTV...\"`（GlobalId 第二 regex） |
| 杂项 | ✅ | unreachable 逗号 metadata/extractvalue CommaAttach/comdat 无括号/index-value-order（引号名） |

**第十六轮执行结果（2026-08，已完成）**——compat 174→**177/452**（+3）、负向正确拒绝 177/误接受 80（上限）、workspace 全量全绿、lalrpop 0 冲突：

| 迭代项 | 状态 | 结果 |
| -------- | ------ | ------ |
| inline asm | ✅ 突破 | AsmKw 专用 token + Call/Tail Call/callbr 三种 asm 变体（约束串/副作用标识丢弃）——L4 历史排除经重评估解锁 +2（alignstack/call-arg-is-callee） |
| blockaddress | ✅ | `ptr blockaddress(@f, %label)` 宽松 0——pr119818 |
| ptrauth 四参数 | ❌ 回滚 | 固定四参数形态致负向 82 超上限——回滚（ptrauth-const 记录） |
| max-inttype | ❌ 记录 | IntTy u16→u32 需 TypeEntry/TypeKey/lexer/grammar 全链改 |
| 散点三连 | ❌ 记录 | masked-load/skip-value-numbers-globals/opaque-ptr-intrinsic-remangling 时间盒内未定位 |

**第十七轮执行结果（2026-08，已完成）**——compat 177→**178/452**（+1）、负向正确拒绝 177/误接受 80（上限）、workspace 全量全绿、lalrpop 0 冲突：

| 迭代项 | 状态 | 结果 |
| -------- | ------ | ------ |
| masked-load | ✅ | `<8  x i1>` 数字后双空格——向量 regex `+x +` 再放宽 |
| gc 子句 | ✅ | `gc "statepoint-example"`（GcKw + 6 处 Define，顺序 gc→personality） |
| skip-value-numbers-globals | ✅ 确认 | 十六轮引号名修复顺带解锁 |
| large-comdat 无名字 | ❌ 归档 | ComdatOpt 无名字分支实测负向 81 超限——回滚（与负向纪律不可调和，终确认） |
| elementtype 函数类型 | ❌ 归档 | `ptr elementtype(void ()) @func` 内嵌函数类型 885 LR 冲突（单 Type 版保留） |
| B3/B4 | ❌ 归档 | max-inttype 位宽全链与 12 个顽固类时间盒内未投入 |

**后续迭代行动版**：剩余 17 个正向失败的逐项问题解析、代码演示、LALR 冲突案例集
与负向纪律规则见 **`docs/archive/forge-ir/remaining-iterations.md`**（§1 逐项清单 / §2 冲突
案例集 / §3 纪律规则 / §5 建议执行顺序）。

**第十八～二十三轮记录断档说明**：第十七轮后的负向计数在
`docs/archive/forge-ir/remaining-iterations.md` §5 逐轮记录。其中第二十一轮的**判定修正**
把部分用例在正向/负向桶间重判（该轮 +8 正向与误接受 67→6 同源），使正确拒绝计数
在该轮前后**非单调**（修正前曾达 257，修正后收敛 254）；终态以 remaining-iterations
的 **198/452、254 正确拒绝、0 误接受**（=452 全收敛）为准。

**第十七轮新增已知失败**：elementtype 函数类型内嵌形态（LR 冲突）；其余与历轮归档一致。

**第十六轮新增已知失败**：ptrauth-const（4 参数 vs 负向纪律）、max-inttype（位宽全链）、masked-load/skip-value-numbers-globals/opaque-ptr-intrinsic-remangling（未定位）。

**第十五轮新增已知失败**：insertextractvalue/scalable-vector-struct（匿名聚合指针 `{{...}}*` 与 ValueType 层 TypedPtrKw 冲突）、masked-load/pr119818/opaque-ptr-intrinsic-remangling（未投入散点）、inline asm（重评估维持 L4）。

**第十四轮新增已知失败**：max-inttype（IntTy u16→u32 需全链改）、musttail/index-value-order/metadata-decl/incorrect-tdep/pr119818/skip-value-numbers-globals/opaque-ptr-intrinsic-remangling（散点未投入）、noalias-addrspace-md（atomicrmw 逗号 metadata 与负向漂移权衡）。

**第十三轮新增已知失败**：define target（LR 状态异常，declare 同构可过——待深挖）、CallArg metadata Named 可空括号（LR 冲突回滚，`metadata !DIExpression()` 待散点方案）、call 非零 addrspace 校验（需 12 处 call 变体传值）。

**第十二轮新增已知失败**：uselistorder use 数校验×5（需模块级 global 引用图——forge-ir 仅 Function.use_lists）、generic-debug-node Tuple 值形态（`operands: {!0, !3}`——负向漂移权衡放弃）、remangle 旧式指针（`%fum*`——LocalId 状态爆炸同源）。

**第十一轮新增已知失败**：split-file 工具语义×2（symbolic-addrspace 系列多子模块拼接）；typed 命名类型（`store %0 %r`——LocalId 进 ValueType 1445 冲突）；uselistorder use 数校验×5（indexes-empty/one/range/toofew/toomany 需符号表 use 列表）；call 非零 addrspace/vscale GEP 校验缺口×2；define internal（上轮确认 LALR 本质冲突复验）。

**新增已知失败（LALR 本质冲突，附录 §6 类）**：declare 前缀 metadata（`!type !2 declare`）、dso_local ifunc、define linkage（`define internal` 与 FuncRetKw 闭包冲突）、裸全局引用 init（`@p = global ptr @h`）、ConstantExprFoldCast 的 @B 行。

---

## §1 短期迭代项

> ⚠️ **本章为历史规划记录**：S1-S6 已全部由第十轮落地（见 §0.1 第十轮执行结果），
> 以下问题解析/改造演示保留供维护参考，不再作为待办。

### S1 嵌套聚合字段提取（内存化方案）——✅ 已实现（第十轮）

**一句话目标**：`extractvalue` 的字段类型本身是聚合时（如 `{{i32, i32}, i32}` 取字段 0），不再报 Unsupported，而是内存化后继续提取。

#### 1.1 问题解析

LLVM IR 允许两级提取：

```llvm
define i32 @f({{i32, i32}, i32} %a) {
  %inner = extractvalue {{i32, i32}, i32} %a, 0   ; 字段 0 是 {i32, i32}（聚合）
  %x     = extractvalue {i32, i32} %inner, 1      ; 内层再提标量
  ret i32 %x
}
```

forge-codegen 的大聚合处理管线把 >8 字节聚合拆成 64 位段值（`Value` 列表），
但"聚合类型"没有寄存器表示——段值只能拼标量。当前**三处**遇到"字段是聚合"
直接拒绝：

| 位置 | 场景 | 现状 |
| ------ | ------ | ------ |
| `compiler.rs:126-130`（`rewrite_agg_value_uses` ExtractValue 分支） | 大聚合**参数**的嵌套提取 | `Unsupported("大聚合参数嵌套字段提取暂不支持")` |
| `compiler.rs:892-896`（`expand_large_aggs` Extract 分支） | 大聚合 **load 结果**的嵌套提取 | `Unsupported("大聚合嵌套字段提取暂不支持（字段是聚合）")` |
| `compiler.rs:1218-1223`（`pack_agg_into_bytes` 的 `AggChild::Agg` 分支） | 聚合**常量**打包 | ⚠ 实际已是递归实现（见 1.4 核对） |

#### 1.2 现状代码

标量路径的核心假设（`compiler.rs:132-162`）：字段 = "段值 + 移位拼接"。

```rust
// compiler.rs:119-130（rewrite_agg_value_uses，节选）
let off = ts.field_offset(agg_ty, idx)...? as usize;
let seg_ty = ts.aggregate_elem_type(agg_ty, idx)...?;
if ts.is_aggregate(seg_ty) {
    return Err(IrError::Unsupported(format!(
        "大聚合参数嵌套字段提取暂不支持：type {seg_ty:?}"   // ← 卡点
    )));
}
drop(ts);
let k = off / 8;
let inner = off % 8;
let base = segs.get(k).copied()...?;
// 标量：段值 → （off%8 移位）→ Copy/bitcast 截断出字段
```

load 路径（`compiler.rs:886-896`）同样在 `is_aggregate(field_ty)` 处拒绝，
但它的"地址语义"（`li.addr` + `field_offset` + `load`）正是内存化方案的现成范本：

```rust
// compiler.rs:886-910（expand_large_aggs Extract 分支，节选）
} else if let Some(idx) = extract_idx {
    // 字段提取：%p2 = add ptr addr, field_offset; %s = load field_ty, ptr %p2
    let field_ty = ts.aggregate_elem_type(li.ty, idx)...?;
    if ts.is_aggregate(field_ty) {
        return Err(IrError::Unsupported(format!(
            "大聚合嵌套字段提取暂不支持（字段是聚合）：type {field_ty:?}"
        )));                                        // ← 卡点
    }
    ...
}
```

#### 1.3 改造演示（内存化，全 ISA 通用）

**核心思想**：字段是聚合时放弃"段值→寄存器"路径，改为"帧槽 + 地址映射"：

1. **外层提取**：分配一个 alloca 帧槽（按 `size_bytes(seg_ty)` 对齐），把字段覆盖的
   段写入槽内；结果 Value 保持聚合类型，并在**编译器内部映射表**记录
   `(value → 槽地址, 槽类型)`——注意：这不需要改 `ValueDef`（见 1.4 核对），
   与 `expand_large_aggs` 的 `LoadInfo{result, addr, ty}`（`compiler.rs:742-747`）
   同构；
2. **内层提取**：后续 `extractvalue` 的 operands[0] 命中映射表时——标量字段走
   GEP（`field_offset`）+ load；聚合字段递归走步骤 1。

```rust
// ── 改造：rewrite_agg_value_uses 的 ExtractValue 分支 ──
// 新增：聚合值 → 槽地址映射（函数级，跨两次遍历维护；仿 LoadInfo）
struct SlotInfo { addr: Value, ty: TypeId }           // addr 是 PTR
// 在编译状态（CompilerState 或函数局部）维护：
//   agg_slots: HashMap<Value, SlotInfo>

Opcode::ExtractValue => {
    let idx = ...; // 不变：取 Immediate::Uint
    let ts = func.types.borrow();
    let off = ts.field_offset(agg_ty, idx)...? as usize;
    let seg_ty = ts.aggregate_elem_type(agg_ty, idx)...?;
    if ts.is_aggregate(seg_ty) {
        drop(ts);
        // 新：内存化路径（仅支持段对齐字段，off % 8 == 0——时间盒口径）
        debug_assert!(off % 8 == 0, "非对齐聚合字段暂不支持");
        let slot_size = ts.size_bytes(seg_ty) as usize;
        // 1) 分配帧槽：插入 Alloca 指令（预扫描分配帧偏移，lowering 经
        //    ctx.alloca_offsets + lea_off 取址——compiler.rs:1615-1616）
        let alloca = func.dfg.make_inst(
            Opcode::Alloca, use_pos.block, smallvec::smallvec![],
            smallvec::smallvec![Immediate::Const(
                func.constants.insert_int(slot_size as i128, 64))],
            &[TypeId::PTR], InstFlags::NONE,
        );
        let slot = func.dfg.insts[alloca.0 as usize].results[0];
        // 2) 字段覆盖的段（segs[off/8 .. off/8 + slot_size/8]）逐段 store 进槽
        //    （复刻 compiler.rs:184-221 Store 分支的"store 段 + add ptr, 8"模式）
        let mut cur = slot;
        for (k, s) in segs.iter().skip(off / 8)
                          .take(slot_size.div_ceil(8)).enumerate() {
            func.dfg.make_inst(Opcode::Store, use_pos.block,
                smallvec::smallvec![*s, cur], smallvec::smallvec![],
                &[], InstFlags::SIDE_EFFECT);
            if k + 1 < slot_size.div_ceil(8) {
                let ci = func.dfg.make_inst(Opcode::Iconst, use_pos.block,
                    smallvec::smallvec![],
                    smallvec::smallvec![Immediate::Const(
                        func.constants.insert_int(8, 64))],
                    &[TypeId::I64], InstFlags::NONE);
                let cv = func.dfg.insts[ci.0 as usize].results[0];
                let add = func.dfg.make_inst(Opcode::Iadd, use_pos.block,
                    smallvec::smallvec![cur, cv], smallvec::smallvec![],
                    &[TypeId::PTR], InstFlags::NONE);
                cur = func.dfg.insts[add.0 as usize].results[0];
            }
        }
        // 3) 结果登记：原 extractvalue 结果 value 记入映射（def 不变，
        //    类型保持聚合），原指令标 Nop
        agg_slots.insert(inst.results[0], SlotInfo { addr: slot, ty: seg_ty });
        let inst = &mut func.dfg.insts[use_pos.inst.0 as usize];
        inst.opcode = Opcode::Nop;
        inst.operands = smallvec::smallvec![];
        inst.immediates = smallvec::smallvec![];
        return Ok(());
    }
    // 标量路径（off/8 段 + off%8 移位 + Copy/bitcast）保持不变
    ...
}

// ── 改造：内层提取（在 rewrite_agg_value_uses / expand_large_aggs 的
//    Extract 分支入口处加一段）──
// 命中 agg_slots 时：标量字段 = gep(槽, field_offset) + load；
// 聚合字段 = 递归分配新槽（走外层路径）。
if let Some(slot) = agg_slots.get(&operands[0]) {
    // %p = add ptr slot, off; %v = load seg_ty, ptr %p
    // （直接复用 compiler.rs:901-930 的现有 GEP+load 代码，仅 addr 来源改为 slot）
    ...
}
```

对 load 结果路径（`compiler.rs:886-896`）做同样的分支改造：字段是聚合时
**直接**复用 `li.addr`（load 源地址）+ `field_offset` 得到字段地址，登记
`agg_slots`——连 alloca 都省了，比参数路径更简单。

#### 1.4 边界与风险（含对 backlog 草图的核对）

- ⚠ **`ValueDef::SlotAddr` 不存在**：`dfg.rs:28-39` 的 `ValueDef` 有
  `Inst / Param / AggConst / UndefNamed` 四个变体（第二十九轮新增 `UndefNamed`——
  前向引用占位保留名字）。backlog 草图写"def 记为槽地址（复用
  ValueDef 的地址语义）"是**示意**——真加变体要同步 display、verify、regalloc
  等所有消费方。推荐落地形态是 1.3 的**映射表方案**（仿 `LoadInfo`），
  零 IR 结构改动；`ValueDef` 扩展留作备选。
- ⚠ **`pack_agg_into_bytes` 的嵌套聚合实际已支持**：`compiler.rs:1218-1223`
  的 `AggChild::Agg` 分支是**递归**调用 `pack_agg_into_bytes`，"聚合常量打包：
  嵌套聚合缺失"只是 `cp.get_aggregate()` 返回 None 的兜底报错。backlog 把
  1220 列为"同类卡点"不准确——聚合常量路径应重点验证而非改造。
- **跨段切片**：`off % 8 != 0` 时字段跨两个 64 位段。标量路径用移位拼接；
  聚合路径按时间盒口径先只支持**段对齐字段**（`off % 8 == 0`），非对齐保持
  Unsupported。
- **store 只执行一次**：槽填充 store 必须位于 extractvalue 之前、且不被重复
  执行（提取结果作为值传播——多使用处共用同一映射条目，第二次提取直接命中
  映射，不再插 store）。
- **与 `expand_large_agg_params` 互操作**：参数聚合的嵌套提取走同一路径
  （1.3 的参数分支）；与 `expand_large_agg_call_results`（`compiler.rs:536`）
  的 call 结果提取也要核对映射生命周期。

#### 1.5 验收

- 端到端执行：`{{i32,i32}, i32}` 两级提取（参数路径 + load 路径 + 常量路径）
  对照 clang 输出；
- `cargo test --workspace --exclude forge-rustc` 全量回归；
- 非对齐聚合字段仍报 Unsupported（不回归成错误代码）。

---

### S2 TypeOps 单类型第二操作数（最大兼容性拦路）——✅ 已实现（第十轮）

**一句话目标**：支持 LLVM 现代语法 `add i32 %a, %b`（第二操作数无类型，
类型从第一操作数推导）。

#### 2.1 问题解析

`TypeOps` 闭包（`grammar.lalrpop:1771-1776`）要求**每个**操作数都带类型：

```lalrpop
TypeOps: Vec<ParsedOperand> = {
    <mut v: (<TypeOp> Comma)*> <p: TypeOp> => { v.push(p); v }
};
```

而 LLVM 现代语法二元指令第二操作数无类型。失败用例：
`block-labels.ll`（`add i32 1, 1`）、`flags.ll`（`%y` 期待类型）——错误形如
`UnrecognizedToken { expected: ["PtrTy", "IntTy", ...] }`。

**LALR(1) 冲突根因**：若把第二操作数放宽为完整 `<b: Value>`，其 first 集与
`<b: TypeOp>` 在 `LBrace`/`LBracket`/`LAngle` 重叠——struct/数组**类型**
（`{i32, i64}`、`[4 x i32]`）与 struct/数组**常量值**（`{i32 1}`、
`[i32 1, i32 2]`、`<{...}>`）同前缀。`CmpScalarValue`（`grammar.lalrpop:1782-1794`）
用"标量子集"（LocalId/常量/undef/poison 等，无聚合字面量）规避——这是**已
验证可行的先例**（icmp/fcmp 第九轮已解锁）。

**关键认知**：当前 grammar 中**二元指令没有独立规则**——`add`/`sub`/`fadd` 等
全部走通用规则 `<op: Ident> <flags: InstFlagSeq> <args: TypeOps?>`（
`grammar.lalrpop:1650-1677`）。所以"每个二元指令规则加 1 处分支"的落地形态
是：**新建二元指令专用规则**（仿 icmp/fcmp 的 `CompareOp + CmpOps` 模式，
`grammar.lalrpop:991-1004`），而不是改 `TypeOps` 本身。

#### 2.2 现状代码（icmp/fcmp 先例）

```lalrpop
// grammar.lalrpop:991-1004 —— 专用指令规则（置于通用规则之前）
<op: CompareOp> <cond: Ident> <args: CmpOps> => ParsedInst {
    ...
    opcode: op, cond: Some(cond.to_string()), args: args.0,
    metadata_attach: args.1, ...
},

// grammar.lalrpop:1796-1803 —— 单类型第二操作数（第九轮已解锁的模式）
CmpOps: (Vec<ParsedOperand>, Vec<(String, MetadataRef)>) = {
    <a: TypeOp> Comma <b: TypeOp> <metas: CommaAttach*> => (vec![a, b], metas),
    // LLVM 现代语法：第二操作数无类型（类型从第一推导）
    <a: TypeOp> Comma <b: CmpScalarValue> <metas: CommaAttach*> => {
        let a_ty = a.ty.clone();
        (vec![a, ParsedOperand { ty: a_ty, op: b }], metas)
    },
};
```

#### 2.3 改造演示（方案 A：复用 CmpScalarValue）

```lalrpop
// 1) lexer：新增二元指令专用 token（priority = 2，与 Ident 分离）——
//    仿 Icmp/Fcmp（grammar.lalrpop:59-60 的 extern 映射 + lexer #[token]）。
//    必须专用 token：走 Ident 会让专用规则与通用规则 first 集重叠 → 冲突。
BinaryOp: String = {
    Add => "add".to_string(),
    Sub => "sub".to_string(),
    Mul => "mul".to_string(),
    Udiv => "udiv".to_string(),
    Sdiv => "sdiv".to_string(),
    Urem => "urem".to_string(),
    Srem => "srem".to_string(),
    Shl => "shl".to_string(),
    Lshr => "lshr".to_string(),
    Ashr => "ashr".to_string(),
    And => "and".to_string(),
    Or => "or".to_string(),
    Xor => "xor".to_string(),
    Fadd => "fadd".to_string(),
    Fsub => "fsub".to_string(),
    Fmul => "fmul".to_string(),
    Fdiv => "fdiv".to_string(),
    Frem => "frem".to_string(),
};

// 2) 操作数规则（与 CmpOps 同构；去掉尾逗号 attach 也可保留）
BinaryOps: (Vec<ParsedOperand>, Vec<(String, MetadataRef)>) = {
    <a: TypeOp> Comma <b: TypeOp> <metas: CommaAttach*> => (vec![a, b], metas),
    <a: TypeOp> Comma <b: CmpScalarValue> <metas: CommaAttach*> => {
        let a_ty = a.ty.clone();
        (vec![a, ParsedOperand { ty: a_ty, op: b }], metas)
    },
};

// 3) InstBody 专用规则（仿 CompareOp 分支，置于通用规则之前）
<op: BinaryOp> <flags: InstFlagSeq> <args: BinaryOps> => ParsedInst {
    endian: false, volatile: false, align: 0,
    result: None, opcode: op, cond: None,
    args: args.0, phi_incomings: vec![],
    arg_attrs: vec![], call_fn_attrs: vec![],
    metadata_attach: args.1, flags: flags,
},
```

要点：

- `InstFlagSeq` 必须保留——`add nsw nuw i32 %a, %b` 的标志是常见语法；
- 双类型形式 `add i32 %a, i32 %b`（存量 round-trip 测试大量使用）仍走第一
  分支，**不破坏双类型输入**；
- 语义层（semantics）对 `args[1].ty` 来自第一操作数推导的值无需额外处理
  （`ParsedOperand` 已是完整类型）；
- 方案 B（lexer 层聚合常量单 token 化）是根本解但改动面广（lexer + display +
  嵌套聚合），留作冲突不可收敛时的备选；方案 C（display 改单类型输出）会
  破坏存量双类型 round-trip，不推荐。

#### 2.4 边界与风险

- **LALR 状态膨胀**：每加一个专用 token 都会膨胀状态图（附录 §6 备忘：新增
  token 后必全量回归）。fcmp 已验证此模式可行，但 18 个 token 一起加需准备
  处理 `Local ambiguity` 报错；
- 若个别指令 token 与现有关键字冲突（如 `And` 与 metadata 或属性名），需要
  priority 调整或拆分批次（先整数 13 个，再浮点 5 个）；
- 改完必跑 `cargo check -p forge-ir` 确认 lalrpop 0 冲突。

#### 2.5 验收

- `block-labels.ll` / `flags.ll` 等通过，compat 正向 57 → 70+；
- lalrpop 冲突保持 0；`cargo test -p forge-ir` 全量回归；
- 双类型输入的 round-trip 测试零回归。

---

### S3 文本层剩余小项——✅ 全部已实现（第十轮 S3.1-S3.4）

#### S3.1 `opaque` 类型

**问题解析**：LLVM 的 `opaque` 是"不透明占位类型"（容器外类型，与 `ptr` 并列）。
当前 `ValueType` 规则（`grammar.lalrpop:1947-1964`）无此分支，涉及用例
`auto_upgrade_intrinsics.ll` 等。宽松接受为 `ParsedType::Opaque`（display 原样
还原），不参与大小计算。

**改造演示**：

```lalrpop
// grammar.lalrpop ValueType 规则（1947）新增分支：
OpaqueKw => ParsedType::Opaque,
```

配套改动（4 处）：

1. lexer 新增 `OpaqueKw` 专用 token（`#[token("opaque", priority = 2)]`）+ extern
   映射（仿 `VoidTy`/`PtrTy`）；
2. `ParsedType` 加 `Opaque` 变体 + display 输出 `"opaque"`；
3. `size_bytes` 对 `Opaque` 返回 0 或显式 Unsupported——load/store 目标为
   opaque 时 verify 报 `"opaque type has no size"`；
4. `opaque ptr` 组合（旧语法）继续拒绝。

**验收**：`auto_upgrade_intrinsics.ll` 类正向用例通过；`opaque` 参与大小计算
的用例正确报错（verify 负向）。

#### S3.2 属性组引用变体（call 上 `#N`、declare 尾属性）

**问题解析**：`attributes #N = { ... }` 定义与函数级 `#N` 引用已支持（第九轮
attribute-builtin 通过）。剩余变体在 call 指令与 declare 尾部。**先取证后动手**：

```bash
cargo test -p forge-ir --test llvm_assembler_compat -- --nocapture
# 输出 FAIL xxx.ll: parse error: ...  → 按错误 token 聚类
```

按失败 token 逐类补 grammar 分支（属性组合规则参考 `grammar.lalrpop` 的
`FuncAttrs`/`CallArgList` 现状，`CallArgList` 在 1824 附近）。验收：对应用例
通过 + 全量回归。

#### S3.3 global 尾 metadata

**问题解析**：`GlobalTail` 列表（`(Comma Opt)*` 模式）已支持（4.3 轮实证）；
剩余为特定 metadata kind（如 `!dbg` 挂 global）组合。推进方式同 S3.2（失败
清单驱动，按 token 补分支）。注意 metadata 附加后需过 `validate_metadata_shapes`
（`semantics.rs:483`，见 S6）。

#### S3.4 fpext / fptrunc lowering

**问题解析**：⚠ 与 backlog 表述不同——**文本层已完成**：`grammar.lalrpop:1716-1730`
的 `ConvOp` 规则已含 `Fptrunc`/`Fpext`（专用 token 已存在），parse 层可用。
真正缺口在**执行层**：`isa/x86_v10.toml` 无 `[lower.Fptrunc]`/`[lower.Fpext]`
（`grep -c "Fpext|Fptrunc" isa/x86_v10.toml` = 0），lowering 时报 Unsupported。

**谓词能力核对**（`crates/frontend/forge-dsl/src/codegen/mod.rs:1817-1831`）：
DSL variants 谓词支持 `immN`（立即数，`strip_prefix("imm")`）、`rs1`/`rd`
（**位宽**，`__sb`/`__rdb`）、`elem`（元素 TypeId）。⚠ backlog 草图的
`imm0==F32` 不可行——`imm0` 是立即数谓词不是类型谓词。正确写法：用
`rd`（结果位宽，fptrunc 目标 F32 → 32；fpext 目标 F64 → 64）+ `elem`（源元素）。

**改造演示**（`isa/x86_v10.toml`，参考 `[lower.Ftrunc]`（1567）的 movsd 模式
与 `[lower.Fcmp.*]`（1642-1665）的 elem 变体模式）：

```toml
# Fptrunc：double → float（源 F64 在 rs1，结果 F32 在 rd）
[lower.Fptrunc]
variants = [
  { when = "elem==F64 && rd==32", insts = ["movsd rd, rs1", "cvtsd2ss rd, rs1"] },
]

# Fpext：float → double
[lower.Fpext]
variants = [
  { when = "elem==F32 && rd==64", insts = ["movss rd, rs1", "cvtss2sd rd, rs1"] },
]
```

注意：`movsd`/`movss` 是 XMM 间的位宽拷贝（movsd 拷贝 64 位——double 源；
cvtsd2ss 转换后结果在 rd 低 32 位，与 rd==32 的 opsize 匹配）。若 `rd==N`
位宽谓词对 Freg 不可用（需实测 `parse_when_cond` 生成代码），退路：
`lowering.rs` 特判分派（按结果类型手工选模板）。

**验收**：`fptrunc double 1.5 to float` / `fpext float 1.5 to double` 端到端
执行对照 clang；`cargo test -p forge-tests --lib "isa::x86_64::text_to_exec"`。

---

### S4 verify 语义校验剩余单例——✅ 已实现（第十轮 S4.1-S4.3）

**一句话目标**：补 3 个负向校验缺口，让对应"误接受"转"正确拒绝"。

#### 4.1 缺口 1：getelementptr struct 索引非常量

**问题解析**：`check_gep_indices`（`verify.rs:900-964`）已做：
标量位置后续索引报错（915-924）、struct 位置非 i32 **类型**索引报错
（927-937）。但 struct 位置**非常量**索引（LLVM LangRef：struct 索引必须是
**常量** i32）目前保守跳过——`verify.rs:938-963` 中 `idx_const` 取不到时
`None => return`。

```rust
// verify.rs:938-963（现状，节选）
let idx_const = match func.dfg.values[v.0 as usize].def { ... };
let next = match idx_const {
    Some(c) if is_struct => ts.aggregate_elem_type(cur, c as u32),
    Some(_) => ts.element_type(cur),
    None => return,          // ← 缺口：struct 位置的非常量索引应报错
};
```

**改造演示**：

```rust
let next = match idx_const {
    Some(c) if is_struct => ts.aggregate_elem_type(cur, c as u32),
    Some(_) => ts.element_type(cur),
    None if is_struct => {
        // LLVM：struct 索引必须是常量 i32（非常量无法推进类型）
        self.errors.push(VerifyError::TypeMismatch {
            inst,
            expected: "constant i32 struct index".into(),
            found: "non-constant value".into(),
        });
        return;
    }
    None => return,
};
```

#### 4.2 缺口 2：global-init cast 链类型匹配

**问题解析**：global 初始化的 `bitcast`/`ptrtoint` 表达式链（`GlobalConstExpr`，
常量表达式折叠在 `grammar.lalrpop:523-551` 的 `ValueType` 驱动下解析）目前
不校验两端类型匹配。定位：semantics.rs 的 global init 处理（`ptrtoint`/
`bitcast` 字符串在 `semantics.rs:1674-1676` 附近分派）→ 常量表达式求值处加
类型检查。

**改造演示**（示意，位置以实测为准）：

```rust
// 常量表达式求值：转换类节点校验源/目标类型
ExprKind::Ptrtoint { val, to } => {
    let src = expr_ty(val)?;
    if !matches!(src, ParsedType::Ptr(_) | ParsedType::PtrAddrSpace(_)) {
        return Err(IrError::Semantic(format!(
            "ptrtoint: expected pointer source, got {src:?}"
        )));
    }
    ...
}
ExprKind::Bitcast { val, to } => {
    let src = expr_ty(val)?;
    let dst = to;
    if bits_of(src) != bits_of(dst) {
        return Err(IrError::Semantic(format!(
            "bitcast: size mismatch {src:?} -> {dst:?}"
        )));
    }
    ...
}
```

#### 4.3 缺口 3：alias 前向引用延迟校验

**问题解析**：alias 定义在目标符号之前时，`semantics.rs:217-245`
（`add_global_alias`）只校验 linkage/visibility 组合，不校验目标存在性——
应在模块构建收尾（所有 global 已登记后）做延迟校验。

**改造演示**（backlog 已给骨架，补位置说明）：

```rust
// build_module 收尾（validate_metadata_shapes 附近，semantics.rs:478 之后）
for alias in module.iter_global_aliases() {
    if let Some(name) = alias.target_name() {
        if !module.symbols.contains_key(name) {
            return Err(IrError::Semantic(format!(
                "alias target {name} is not defined in the module"
            )));
        }
    }
}
```

#### 4.4 测试与验收

负向回归按 `verify_negative.rs` 现有模式（`verify_errs`/`has_any`，17-48 行）
补 3 个用例；验收：对应负向用例从"误接受"转"正确拒绝"（compat 负向的
非 DI 误接受进一步下降），`cargo test -p forge-ir --test verify_negative` 全绿。

---

### S5 工程稳健性——✅ 已处理（第十轮：S5.1 定位、S5.2 记录）

#### S5.1 determinism 测试偶发

**问题解析**：`crates/frontend/forge-dsl/src/codegen/mod.rs:3497-3525` 的
`determinism_tests`——同一 model 两次 `generate()` 应字节一致（防 HashMap
迭代序回潮）。workspace 并行（`-j 4`）下偶发 FAILED（生成 3MB diff），单独跑
必过（160 次循环未复现）。

**排查 SOP**：

```bash
# 1) 单独跑，确认是否复现
cargo test -p forge-dsl --lib determinism
# 2) 复现时 diff 两份生成串，定位首个分歧 token
# 3) 并行干扰假设：对比 -j 1 与 -j 4 复现率（build.rs/多 crate 竞争同一生成文件？）
for i in $(seq 1 10); do cargo test --workspace --exclude forge-rustc -j 4 2>&1 | grep -c FAILED; done
```

**修复方向**：若确认为 HashMap 迭代序 → 生成路径改 `BTreeMap` 或排序收集
（`is_negative` 已证明 sorted 收集可修）；若是文件竞争 → build.rs 输出
路径隔离/加锁。**验收**：`-j 4` 连跑 10 轮 0 FAILED。

#### S5.2 load 快照语义

**问题解析**：`load` 后内存被改写时，重新 load 不保留旧值（编译器"load 值
传播"模型）。常见模式安全（写与 load 同函数内顺序执行）。**措施**：文档记录
该语义限制（roadmap §7 已有说明），出现"load 快照被破坏"用例时在 store 到
load 地址时插入显式拷贝（≤8 字节小聚合可整值拷贝）。无用例不实现。

---

### S6 metadata 形状校验扩展（可选）——✅ 已实现（第十轮）

**现状**：`validate_metadata_shapes`（`semantics.rs:483-524`）已做：
`!dbg` → 必须 `DILocation`；`!tbaa` → tuple tag 结构；`!range` → 整数区间
tuple。**扩展候选**（按失败用例驱动，无用例不扩展）：

```rust
// semantics.rs check 闭包内新增分支（示意）
MetadataKind::Prof => match node {
    // !prof !{i32 30, i32 70}：权重元组（偶数字数、全部整数）
    MetadataNode::Tuple(vals)
        if vals.len() % 2 == 0 && vals.iter().all(|v| matches!(v, MetadataValue::Int(_))) => Ok(()),
    other => Err(IrError::Semantic(format!(
        "!prof must reference an even-length tuple of integer weights, got {other:?}"
    ))),
},
// !alias.scope / !noalias → 作用域节点；!llvm.loop → 循环 metadata
```

`MetadataKind` 枚举已有 `AliasScope`/`NoAlias`（`semantics.rs:2024-2025` 映射）。
验收：新增负向用例正确拒绝，防回归断言（非 0 上限）保持。

---

## §2 中期迭代项

### M1 grammar 拆分评估

**问题解析**：lalrpop 单文件 `crates/foundation/forge-ir/src/ir_parser/grammar.lalrpop`
（~2000 行），增量编译 5-10s 可接受。关键认知：LALR 状态图是**全局**的——
按 item 类别拆文件（`include!`）只改善组织、不改善编译时间；实质提速需缩小
token 集合（S2 的专用 token 化反而会**膨胀**状态，需权衡）或换生成器。

**触发条件**：增量编译 >15s 或冲突难收敛。**维持现状时的动作**：每次语法
改动后 `cargo check -p forge-ir` 确认 0 冲突 + 增量编译计时（S2 落地后重点
观察）。

### M2 解析基准细化

**现状**：`crates/foundation/forge-ir/benches/ir_parse.rs`（criterion）——
`parse_medium_module` ~57µs、`parse_display_roundtrip` ~148µs。

**细化方向**：

1. 用例扩充：异常（invoke/landingpad）、嵌套聚合字面量、metadata 密集模块；
2. `docs/performance/bench_baseline.md` 每轮更新基线；
3. 热点分析（按需）：`value_map` 查找、display 的 `self.types.borrow()` 重复
   借用——收益不确定，先用 `cargo bench -p forge-ir --bench ir_parse` 出
   profile 再决定。

### M3 官方用例正向扩充 57→80+——✅ 已达成（198/452）

**结果**：第二十三轮收敛至 **198/452**（误接受 0、正确拒绝 254）；逐轮增量见
`forge-ir-remaining-iterations.md` §5。

**历史前置**：S2（解锁 block-labels/flags 类）、S3（opaque/属性组解锁
auto_upgrade 类）。

**历史推进方式**（每轮重复直到收敛）：

1. `cargo test -p forge-ir --test llvm_assembler_compat -- --nocapture`
   重跑失败清单；
2. 按错误 token 聚类（每类 ≥1 个用例通过即算解锁——如 `expected: ["PtrTy",
   "IntTy", ...]` → S2；`expected: [opaque]` → S3.1）；
3. 每类补语法 → `cargo test -p forge-ir` 回归 → compat 统计更新 → 用例目录
   `tests/llvm_assembler_cases`（452 个 .ll）文件头分类清单同步；
4. 验收：正向 ≥80，误接受 47 不回升；剩余失败收敛到长期排除项。

---

## §3 长期 / 已排除项（启动前置条件一览）

| # | 迭代项 | 启动前置 | 备注 |
| --- | -------- | ---------- | ------ |
| L1 | DI 验证器（~40-46 负向误接受） | 无（明确不做） | DINode 字段校验是独立工程，工作量大收益低 |
| L2 | >16 字节聚合 ABI 栈传递 | 栈帧 ABI 设计（参数区布局、16 字节对齐、调用方/被调方职责） | 现 ≤16 字节走 SysV 整数寄存器；`compiler.rs:300-303` 报 Unsupported |
| L3 | vscale 可伸缩向量 | TypeId 加 vscale 维度 + `size_bytes` 动态化（运行时 xlen）+ SVE 指令选择 | ✅ 文本层已实现（第十二轮 lexer 单 token）；TypeId/codegen 仍不做 |
| L4 | inline asm（`call void asm sideeffect`） | 编码器集成（若启动） | ✅ 文本层已实现（第十六轮 call/tail/callbr 三形态）；编码器集成仍不做 |
| L5 | callbr / token / statepoint / gc / blockaddress | statepoint 需 intrinsics 体系 | callbr（十二轮）/token（十轮）/gc（十七轮）/blockaddress（十六轮）文本层已实现；仅 statepoint 未支持 |
| L6 | 裸全局引用 init（`@p = global ptr @h`） | 无（语法本质不可解） | LALR(1) 2-lookahead 歧义；用 `ptr @h` 带类型形式 |
| L7 | SjLj 异常代码生成 | `setjmp/longjmp` 模拟 + libc 链接 + 帧展开设计 | 文本层已完成（invoke/landingpad/resume/personality 全链路） |
| L8 | Windows SEH / ARM EH | 平台 ABI 工程（`.xdata`/`.pdata` 或 `.ARM.exidx`） | 随 ABI 选择 |
| L9 | 旧式指针 / 旧 intrinsic / autoupgrade | 无（设计决策：只跟最新版） | `i32*`、`metadata !{}`、`llvm.aarch64.thread.pointer` 等按旧格式排除 |
| L10 | 多值 ret（`ret i32 1, i32 2`） | 无（非 LLVM 标准，不做） | 内部 `Return(Vec<Value>)` 已支持多值（大聚合返回用） |

---

## §4 工程速查附录

### 回归与统计命令

```bash
cargo test --workspace --exclude forge-rustc                      # workspace 全量
cargo test -p forge-ir                                            # forge-ir 全量（语法改动后必跑）
cargo test -p forge-ir --test llvm_assembler_compat -- --nocapture # compat 统计 + 失败清单
cargo test -p forge-ir --test verify_negative                     # 负向语义校验
cargo test -p forge-tests --lib "isa::x86_64::text_to_exec"       # 执行层全量
cargo bench -p forge-ir --bench ir_parse                          # 解析基准
cargo check -p forge-ir                                           # lalrpop 0 冲突检查
cargo test -p forge-dsl --lib determinism                         # determinism 单独复现
```

### lalrpop 约束（附录 §6 备忘）

- 新增关键字 → **专用 lexer token**（`#[token("xxx", priority = 2)]`）+ extern
  块映射（终结符 first 与指令 Ident 分离——3.2 实证）；
- 列表式可选参数 → 每项以 `Comma` 开头整体匹配（AllocaOpt/GlobalTailOpt 模式）；
- 可空规则避免双重嵌套（内层 `(X)+`，可空只留外层 `?`）；
- lalrpop 0.23 无 `ambiguous_grammar` 宏——冲突只能重构语法；
- **新增 token 后必全量回归**（模块 item first 集合膨胀影响全局状态图）。

### 每轮文档维护约定

1. 轮末成果回写 `docs/archive/forge-ir/iteration-roadmap.md` 附录 §7；
2. 同步 `docs/archive/forge-ir/next-iterations.md` 状态列（已解锁项移出或标注）；
3. 同步 compat 文件头基线（正向通过数/误接受数/已解锁类）；
4. 基准变化更新 `docs/performance/bench_baseline.md`；
5. 新增 LALR 冲突形态 → backlog 附录 §6 备忘表加行。

### 与 backlog 的差异核对记录（本文写作时实测）

| 项 | backlog 表述 | 实测结论 |
| ---- | -------------- | ---------- |
| S1 | `ValueDef::SlotAddr` 复用地址语义 | `dfg.rs:28-36` 无此变体；推荐映射表方案（仿 `LoadInfo`） |
| S1 | `compiler.rs:1220` 聚合常量打包"嵌套聚合缺失"是同类卡点 | `pack_agg_into_bytes` 的 `AggChild::Agg` 分支**已是递归实现**（1218-1223），报错仅为常量池缺项兜底 |
| S2 | "每个二元指令规则加 1 处分支" | 真实 grammar 二元指令无独立规则（通用 `Ident+TypeOps?`，1650/1664）；落地 = 新建 `BinaryOp` token 集 + 专用规则（仿 icmp/fcmp） |
| S3.4 | `imm0==F32` 目标类型谓词 | `immN` 是立即数谓词（codegen/mod.rs:1817）；用 `rd` 位宽（__rdb，1830）+ `elem` |
| S3.4 | 文本层缺 fpext/fptrunc | `ConvOp`（grammar.lalrpop:1716-1730）已含两 token，文本层已完成；缺口仅在 `isa/x86_v10.toml` lowering |
| S5.1 | `forge-dsl/src/codegen/mod.rs:3497` | 实际路径 `crates/frontend/forge-dsl/src/codegen/mod.rs:3497` |

