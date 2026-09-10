# forge-ir 剩余任务总清单（第二十八轮）

## ⚠️ ARCHIVED（2026-09）

> forge-ir 剩余任务总清单（P0 已实现；P1/P2 与审计遗留，第 28→32 轮）。
> 本文为历史记录，仅供参考；代码现状以仓库代码与现行文档为准，不再维护。
> 状态:LLVM Assembler 兼容 **452 全收敛**(198 正向 + 254 拒绝 + 0 误接受)、
> workspace 48 suite 全绿、clippy 手写代码 8 条低风险遗留。
> 本文档汇总**全部剩余工作**:路线图 P0(已实现——状态标记)、
> P1/P2(远期工程,含问题解析与代码演示)、审计遗留(评估结论)。

## §0 已实现确认(路线图 P0,历轮迭代覆盖——本轮复核)

| 路线图项 | 状态 | 证据 |
| --- | --- | --- |
| 2.1 declare 尾部属性 | ✅ | DeclareTail 的 FuncAttrs(含 Alwaysinline,grammar:1336) |
| 2.2 命名 metadata | ✅ | 第二十一轮 MetadataFieldTupleLit/DI 校验器 |
| 2.3 指令 metadata 附加 | ✅ | BinaryOps/CmpOps/ExtractElement/ShuffleVector/Call/Fneg 等均 CommaAttach* |
| 2.4 终结符 metadata | ✅ | Ret/Br/Switch/Invoke/Resume/Unreachable/Callbr 全部 CommaAttach* |
| 2.5 addrspacecast / va_arg | ✅ | grammar:851(AddrspaceCastKw LParen 形态)/ 1696(VaArgKw) |
| 2.6 linkage/visibility/dllstorage | ✅ | DllImportKw/dllexport/AvailableExternallyKw(lexer:197-201) |
| 2.7 参数/返回属性 | ✅ | FuncRetKw + parse_param_attrs |
| 2.8 模块级 item | ✅ | SourceFilenameKw/ModuleAsmKw(grammar:325-326) |

## §1 P1 内部表示扩展(远期工程,每项含解析与代码演示)

### 1.1 异常处理全套 IR 表示（landingpad/resume 已实现；仅 catch 族缺，已归档）

**问题解析**：invoke 终结符、`landingpad` 指令（含 cleanup/catch/filter clause）与
`resume` 终结符已实现（第二十八轮：`Opcode::LandingPad`=opcode.rs:193、
`Terminator::Resume`=terminator.rs:66，grammar/semantics/display 全链）；
`catchpad`/`cleanuppad` 指令与 `catchswitch`/`cleanupret`/`catchret` 终结符
**评估归档**——452 用例零命中、需 3 个 `Terminator` 变体扩展（中-大工程）、
无目标平台语义（Windows EH/Emscripten），与 metadata kind 白名单同理（无用例驱动，
见 §7）。`!llvm.eh.*` intrinsics 与函数 personality 属性文本层透传。

**代码演示**(opcode.rs + semantics.rs):

```rust
// opcode.rs 新增
Landingpad, Catchpad, Cleanuppad,   // 指令(结果带 clause 列表)
Catchswitch, Cleanupret, Catchret,  // 终结符
// semantics.rs:landingpad 分支(宽松:clause 解析为 metadata 引用,payload 透传)
"landingpad" => {
    // <ty> <personality>? [clause ...]   → Opcode::Landingpad
}
```

**验收**:LLVM test/Exceptions 子集入 cases;0 冲突。**工作量:中-大**(终结符+指令双面)。

### 1.2 聚合常量深层折叠(extractvalue/insertvalue 常量操作数)

**问题解析**:`extractvalue {i64, {i32, i32}} {...}, 1, 0` 的嵌套索引折叠——
**parse 层深层索引已支持**（semantics.rs:2256，第二十九轮重建：链式
extractvalue）；缺口仅在 **builder API**：`fb.extract_value` 现有单层路径,
AggConst 递归 child 定位缺深层索引（fold/消费方）。

**代码演示**(builder.rs):

```rust
pub fn extract_value(&mut self, agg: Value, idx: &[u32]) -> Value {
    // 现:单层 → 改:递归 walk agg.children(经 ConstantPool::get_aggregate),
    // 末层索引命中后 emit extractvalue(深层索引已折叠为单层)
}
```

**验收**:deep-extractvalue.ll 正向用例 + 常量折叠单测。**工作量:小-中**。

### 1.3 常量表达式折叠补全(gep/ptrtoint/bitcast)——✅ 第三十一轮已完成

**问题解析**:`@g = global ptr getelementptr(...)` 的 GEP 常量表达式折叠
(gep 的 inbounds/结构索引 offset 计算)缺——现仅 trunc/inttoptr 等。

**落地**:第三十一轮完成 GEP 字节偏移折叠（semantics.rs:1101-1119：数组/向量索引
× elem_size、结构按字段偏移累积，`size_of_parsed_type` helper）；本条保留为历史
记录。

**代码演示**(semantics.rs to_type_result 常量路径):GEP 常量 → 计算字节偏移 →
`ConstantPool::insert_int`(标量)或保留结构。**工作量:中**。

### 1.4 metadata kind 白名单校验 —— ⚠️ 评估后不做

**问题解析**:LLVM 对未知 metadata kind 报错;但当前误接受已 0(无带非法
kind 的负向用例),白名单只防未来用例;而正向用例 kind 面广(!llvm.* 前缀 +
自定义),漏一个即破坏 452 收敛。**结论:记录不做**(除非引入新负向用例子集)。

## §2 P2 测试与工具链强化(远期)

| 项 | 说明 | 状态 |
| --- | --- | --- |
| 4.1 round-trip fuzz | display_llvm 测试已有 roundtrip 基础;规模化随机用例(display→parse 往返) | ✅ 第三十一轮:452 全量 roundtrip 常规回归(解除 ignore,198 checked 0 failures);随机 fuzz 化延后 |
| 4.2 负向测试 | 已全(254 拒绝);扩充随官方子集 | 基线已强 |
| 4.3 LLVM 官方 Assembler 子集 | 452 用例已收敛;下一批:Exceptions/Features/Attributes 目录 | 中量(需逐例判 RUN) |
| 4.4 端到端执行对照 | forge-tests exec 已对照;扩充 jit 用例 | 持续 |

## §3 审计遗留(评估结论)

| 项 | 结论 |
| --- | --- |
| make_inst 三层收敛 | ✅ 本轮:中间层 make_inst_with_meta 删除(仅 1 处转发),clone_inst 补写保留(已封装) |
| egraph→algebraic 改名 | ✅ 本轮:改名 + 模块引用/注释修正(EGraphPass 名保留最小化 diff) |
| isel_strategy 标签接通 | ⚠️ 记录:DSL lower_pattern arm 名(lea-merge-iadd-imul-*)与手写标签(lea_sib)两套体系对接会改变指令序列(第二十七轮 agg_ret_abi 教训)——作为功能开发项,需专项验证 |
| forge-grammar semantic 层接入 | ⚠️ 记录:30KB 零外部调用;接入 mini_c 是产品决策,标注 #[doc(hidden)] 可选 |
| forge-tests fuzz 归并 | ⚠️ 记录:harness 重复合并,孤儿用例并入执行链 |
| jit register_external 吞错 | ⚠️ 记录:改 Result 透传(API 破坏性小,待 JIT 面改动批次) |
| 巨型文件拆分(forge-dsl codegen/mod.rs 175KB 等) | 远期(用户决策 26 轮:不拆) |
| forge-dsl 双语法体系统一(Frag/AsmFrag) | 远期 |

## §4 验收命令速查

```bash
cargo check -p forge-ir                                  # 0 LALR 冲突
cargo test -p forge-ir --test llvm_assembler_compat -- --nocapture  # 452 收敛
cargo test -p forge-ir / -p forge-opt / -p forge-codegen # 全量
cargo test --workspace --exclude forge-rustc -j 4        # 交付前必跑
```

## §5 后续状态

- **第二十八轮**:P0 复核确认全部实现;make_inst 收敛 + egraph 改名落地;
  1.4 metadata kind 白名单评估为不做;剩余全部为远期工程(P1 三项 +
  P2 四项 + 审计遗留六项记录)。

## §6 第三十一轮交付记录

- **GEP 常量字节偏移折叠**:const_expr_value 的 GetElementPtr 分支按元素类型缩放索引
  (数组/向量 idx × elem_size、结构按字段偏移累积),新增 size_of_parsed_type helper;
  原仅值级相加([4 x i32] 索引 1 误加 1 字节而非 4)。
- **函数地址引用 String 编码**:GlobalAddr 的 Immediate::Global 与 FuncRef 共用
  GlobalId 空间——display 查全局表撞名误输出(skip-value-numbers-globals 的
  @25 函数引用显示成全局 @"");semantics 函数引用改 Immediate::String(函数名),
  display 两处输出 @name,reparse 查 func_refs 幂等。
- **roundtrip 常规化**:roundtrip_all_assembler_cases 解除 #[ignore](0 缺口),
  452 用例 display→reparse 全量回归(198 checked)。
- **哨兵守卫**:GlobalAddr 的 GlobalId(u32::MAX) 未注册哨兵跳过函数回退
  (防越界 panic)。
- **收尾状态**:compat 198/452(0 误接受)、roundtrip 0 缺口、forge-ir 7 suite、
  workspace 48 suite 全绿。
- **剩余**:metadata kind 白名单(评估不做——无用例驱动,记录)、LLVM 官方
  Assembler 子集扩充(下轮)、forge-dsl 拆分(远期记录)。

## §7 第三十二轮:剩余交付项结论(全部收敛)

- **异常处理全套**:✅ landingpad 指令 + clause(cleanup/catch/filter)已在
  第二十八轮实现(grammar/semantics/display 全链);catchswitch/cleanupret/
  catchret 终结符**评估归档**——452 用例零命中、需 IR 表示扩展(3 个
  Terminator 变体 + display + semantics 中-大工程)、无目标平台语义
  (Windows EH/Emscripten)——与 metadata kind 白名单同理(无用例驱动)。
- **常量表达式折叠补全**:✅ 第三十一轮完成——GEP 字节偏移折叠
  (数组/向量 × elem_size、结构字段偏移累积、size_of_parsed_type)。
- **metadata kind 白名单**:评估不做(记录)——正向 kind 面广,漏一破坏 452。
- **round-trip fuzz 规模化**:✅ 第三十一轮完成——452 全量 roundtrip
  常规回归(198 checked 0 failures,解除 ignore);随机组合 fuzz 延后
  (收益边际——452 官方用例已系统性覆盖)。
- **LLVM 官方 Assembler 子集扩充**:评估归档——452 个用例已覆盖本地
  llvm_assembler_cases 目录全量;新用例需外部 LLVM 源码树(当前环境
  离线,无新用例来源);接入新用例时延续逐例判 RUN 方法论。
- **远期记录**:forge-dsl 巨型文件拆分(codegen/mod.rs 175KB)与双语法
  体系统一(Frag/AsmFrag)——记录不排期(用户已决策)。
