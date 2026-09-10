# forge-ir 代码质量审计与重构记录（第二十四轮）

## ⚠️ ARCHIVED（2026-09）

> forge-ir 代码质量审计与重构记录（第 24–47 轮，2026-08 停更）。
> 未关闭的结构性遗留见 `docs/forge-ir/backlog.md`。
> 本文为历史记录，仅供参考；代码现状以仓库代码与现行文档为准，不再维护。
> 本轮为**全面代码质量审计与重构**：LLVM 官方 test/Assembler 452 用例已全收敛
> （198 正向 / 254 正确拒绝 / 0 误接受）,在此基础上清理冗余结构、简化复杂逻辑、
> 收口未使用 API 与生成代码警告。兼容指标为本轮硬约束：**任何改动不得破坏
> 452 收敛**。

## 审计基线（clippy 实测）

| 指标 | 重构前 | 重构后 |
| --- | --- | --- |
| clippy 警告总数 | ~4620 | **8** |
| 其中 unused variable（sync/addrsp/pre/pro/rest/params） | 45 | 0 |
| 无意义 cast（u64/u32/i64/f64） | 27 | 0（手写代码；生成代码豁免） |
| very complex type（生成 grammar.rs） | 4595 | 0（生成代码豁免） |
| collapsible if / auto-deref / identity map / div_ceil 等 | ~55 | 0（clippy --fix） |
| compat 452 收敛 | 198/254/0 | **198/254/0 不变** |

## §1 冗余结构清理（高严重度）

### 1.1 `split_md_tuple_lit` dead pub 删除

**问题解析**：`ast_items.rs` 的 `split_md_tuple_lit` 是第二十一轮
generic-debug-node 迭代中**中间方案的遗留**——最终 lexer 合并方案走
`split_md_field_tuple_lit`（带 key 前缀）,前者零调用者且与后者元素解析
闭包逐行重复。

**落地改动**：删除整个函数（grep 全 crates 确认零调用）。

### 1.2 uselistorder 6 份重复提取 helper

**问题解析**：`grammar.lalrpop` 模块级 2 分支（304-323）与块级 4 分支
（1420-1451）重复相同的「idx 收集 + Big 转换」代码——历轮迭代逐个复制
粘贴的产物。

**落地改动**：

```rust
// ast_items.rs
pub fn collect_uselistorder_idx<T>(
    first: &crate::big::Big,
    rest: Vec<(T, crate::big::Big)>,
) -> Vec<i64> {
    let mut idx = vec![big_as_i64(first)];
    idx.extend(rest.into_iter().map(|(_, n)| big_as_i64(&n)));
    idx
}
// grammar.lalrpop:6 分支统一调用
let idx = crate::ir_parser::ast_items::collect_uselistorder_idx(&_first, _rest);
```

### 1.3 `ParsedType::Label` 死变体删除

**问题解析**：`ParsedType::Label` 无任何 grammar 构造点（label 类型走
`LabelTy` token 的专用路径）,仅 3 处 match 兜底臂引用——死变体。

**落地改动**：删除枚举变体 + `ast_items.rs:599`、`semantics.rs:1410/1466`
三处 match 臂归一为 `Void | Metadata`。

### 1.4 dbg-record 占位链路评估（保留）

**评估结论**：`dbg_record_inst()` 固定占位 + 7 条 grammar 规则参数全弃 +
语义层丢弃是**设计正确**——dbg record 的价值仅在位置/形状校验（
dbg-record-invalid-0/2/4/6/7/8）,参数本就不该落 IR。保留,文档已注明意图。

## §2 API 收口与哨兵一致性（中严重度）

### 2.1 `big_as_i64/u32/u64` 哨兵语义评估（保留）

**评估结论**：仅 grammar ~40 处调用属实,但 `unwrap_or(MAX)` 哨兵在每个
消费点均有先行/后续值域校验（addrspace < 2^24、align 语义层校验、
uselistorder idx 形状校验）,语义安全。重构为 Result 会让 40 处消费点
全部加错误处理,收益为零。helper 文档注释已明确约束。

### 2.2 `HexLit(i64)` 溢出折叠评估（遗留风险记录）

**评估结论**：`HexLit` 的 hex 字面量超 i64 折叠为 0——i128 位宽十六进制
常量（如 `i128 0xFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFF`）会静默变 0。无测试
覆盖,与 IntLit Big 化对齐属后续工作,本轮记录为遗留风险。

### 2.3 unused variable 清零

`grammar.lalrpop` 的 sync×15 / addrsp×7（Define 3 + Call 4,消费 addrsp
的 2207 变体保留）/ pre×6 / pro×6 绑定下划线化。

### 2.4 无意义 cast 清理

GlobalTail 的 `t.0 as u64`×5、data_layout 的 `bits as u32`（integer_align
路径）、semantics 的 `*b as u32`/`*n as u64`/`*f as f64`/`*bits as u32`——
Big 化与 u32 化后的同型 cast 残留删除；`float_align` 的 u16→u32 为真实
类型转换保留。

## §3 可简化逻辑与风格（clippy --fix + 手修）

### 3.1 clippy --fix 批量清理

collapsible if ×25、auto-deref ×18、identity map ×3、手动 div_ceil ×4
（改 `std::cmp::max(1u32, (*bits).div_ceil(8))`）、unnecessary mut、
冗余括号等——28 个文件,compat 452 回归验证无破坏。

### 3.2 生成代码标准豁免

```rust
// mod.rs:lalrpop 生成代码（构建产物,元组改具名结构体治理无运行时收益）
lalrpop_mod!(
    #[allow(
        clippy::redundant_field_names,
        clippy::type_complexity,      // 4595 条:规则多字段元组
        unused_variables,             // 变体参数全弃
        unreachable_patterns,         // 多变体共享前缀
        clippy::too_many_arguments,
        unused_mut
    )]
    pub grammar,
    "/ir_parser/grammar.rs"
);
```

### 3.3 风格遗留修复

- 单行函数格式损坏（split_global_ptr_init/dbg_record_inst——sed 机械替换
  残留的签名体同行）分行修复
- split_vec_const_lit 的 identity map 与重复行修复
- 孤儿 doc 注释删除（agg 序列化函数的注释残留,函数已重构掉）
- `use crate::big::Big` 未使用 import 删除
- preds_of_target 未使用绑定下划线化

### 3.4 评估保留项（记录理由）

- **宽松折叠 hack**（blockaddress→Int(0)、FloatHexLit→UInt(0)、splat
  双路）:全局 init 常量表达式占位求值,display 按 ConstExpr 变体还原文本
  （不依赖求值结果）;统一为 Unsupported 会破坏已通过用例,保留。
- **Distinct 用 Ident 匹配**:宽松语法设计的直接结果,专用 token 与 Ident
  冲突风险高于收益,保留。

## §4 遗留清单（低风险,未在本轮实施）

| 项 | 位置 | 说明 |
| --- | --- | --- |
| too many arguments ×2 | build_inst(10 参) 等 | 拆参数结构体重构面大,收益低 |
| match→if let ×1 | — | 单模式 match,机械可改 |
| 冗余括号 ×3 | — | 机械可改 |
| identical blocks ×1 | — | 需人工判断两分支是否真的相同 |
| recursion-only param ×1 | — | 递归函数参数仅递归使用,需人工评估 |
| HexLit 超 64 位折叠 | lexer.rs:557 | i128 hex 字面量静默变 0,需 Big 对齐 |

## §5 验证

```bash
cargo check -p forge-ir                                   # 0 LALR 冲突
cargo test -p forge-ir --test llvm_assembler_compat -- --nocapture   # 198/254/0 不破
cargo test -p forge-ir                                    # 7 suite 全绿
cargo test --workspace --exclude forge-rustc -j 4         # workspace 全量
cargo clippy -p forge-ir                                 # 8 条(全部遗留低风险)
```

**终态:兼容指标不变(198/254/0),clippy 警告 4620 → 8,冗余结构 4 项清理,**
**生成代码警告全部豁免,手写代码仅剩 8 条低风险遗留项。**

---

## 第二十五轮增补(forge-ir IR 层 + forge-opt + forge-dsl 全面审查)

审计方法:2 个 explore 子代理(IR 层 / opt+hir+dsl)+ 本地 clippy + grep 复证。

### 本轮落地(全部 grep 复证零调用者后删除)

| 项 | 文件 | 说明 |
| --- | --- | --- |
| **INALLOCA/INBOUNDS 位冲突(真实 bug)** | inst_flags.rs | bit 13 撞车(16 位已占满)→ INALLOCA 换 1<<16 + u16→u32 |
| stack_slot.rs 全模块 | stack_slot.rs/function.rs/lib.rs | StackSlots 零外部读写(forge-codegen 用自有 frame_layout) |
| MetadataValue as_uint/as_int/as_string/as_node | metadata.rs | 零调用者 |
| MetadataStore tbaa_root/tbaa_tag/branch_weights | metadata.rs | 仅自身测试(TBAA 走文本层) |
| ConstantPool::constants() | constant.rs | "兼容旧 API" 遗留 |
| MemFlags is_acquire/is_release/is_nontemporal/has_conflicting_endian/display_suffix | mem_flags.rs | 仅自身测试(display 手写 volatile 输出) |
| Big trunc_to_i64/cmp_signed/eq_signed | big.rs | 零调用者(trunc_to_u64 保留——forge-opt const_fold 跨 crate 调用,误删后修复) |
| Immediate as_i64/as_block | immediate.rs | 仅自身测试(改 matches! 断言) |
| support/interpreter.rs + alias_analysis.rs | forge-opt/support | 17.8KB+3.7KB 死模块(与 const_fold 重叠) |
| ModulePass trait | forge-opt/lib.rs | 定义后零实现(被废弃的设计) |
| ISelPass | forge-opt/egraph.rs | 与 EGraphPass 实现完全相同 |
| 注释修复 | forge-opt lib.rs/ipa/mod.rs/const_value.rs | O2 12→13 passes、sroa 残留、interpreter 引用 |

### 遗留清单(结构性重构,单独处理)

- symbol::Alias 双轨(Module::add_alias 零调用,GlobalAlias 为实际路径)
- Function::with_const/with_attributes/retarget、TypeContext::type_count/signature_count、dfg::compact/live_inst_count(仅测试)
- dfg make_inst 三层包装 + 字段事后修补(clone_inst 补写 isel_strategy 等)
- egraph.rs 改名 algebraic.rs(名不副实,功能存在)
- lto/func_specialize/pgo 未接入生产管道(产品决策)
- forge-dsl 双语法体系(lalrpop + forge-grammar)+ Frag/AsmFrag 同构重复(远期统一)
- 类型格式化三份实现(types.rs/display.rs/ast_items)
- TypeContext 20 个转发方法(Deref 化评估)
- 巨型文件拆分(codegen/mod.rs 175KB、cst_codegen.rs 80KB、bitstring.rs 90KB、const_fold.rs 51KB)
- composite.rs 前瞻 API 标注 #[doc(hidden)]
- TypeId 常量与 TypeStore 预填充顺序双处硬编码(索引 9 空洞,脆弱布局)
- HexLit 超 64 位折叠(需 Big 对齐)

---

## 第二十六轮增补(遗留清单重构)

用户决策:lto/pgo/func_specialize 保留并标注;巨型文件本轮不拆(记录远期)。

### 本轮落地

| 项 | 说明 |
| --- | --- |
| symbol::Alias 双轨删除 | Alias/AliasTarget/Module::aliases 表删除;add_alias 重定向 GlobalAlias 统一入口 |
| Function::with_const/with_attributes/retarget | 零调用删除(跳转改写走 Terminator::retarget) |
| TypeContext::type_count/signature_count | 零调用删除 |
| dfg::compact/live_inst_count | 零生产调用删除(墓碑评估:remove_inst 有生产调用,Nop 已被遍历过滤,删除无行为变化) |
| HexLit Big 对齐 | lexer HexLit(i64)→Big(dashu),grammar 8 处消费点 big_as_u64;超 64 位 hex 不再静默折叠 0 |
| composite.rs #[doc(hidden)] | forge-hir 前瞻 API 标注 |
| TypeId 空洞注释 | 索引 9 保留空洞原因与维护约定注释 |
| fmt_llvm_type 全量覆盖 | display LLVM 输出补齐全部 TypeEntry 分支(Int/Float/Vector/Array/Function/Token/Metadata/BFloat/ScalableVector),消除 `_ => store.fmt_type` 方言兜底泄漏 |
| lto/pgo/func_specialize #[doc(hidden)] | 未接入管道的前瞻 pass 标注 |

### 新遗留(结构性,风险面大)

- make_inst 三层包装收敛(make_inst_with_meta_and_loc 收全字段,消除 clone_inst 事后补写 isel_strategy/param_attrs/fn_attrs)
- egraph.rs → algebraic.rs 改名(名不副实)
- 墓碑永久累积(compact 已删——如后续需要回收,Nop 过滤路径在 dfg.rs:457/603 已有)
- 巨型文件拆分(forge-dsl codegen/mod.rs 175KB 等)——远期
- forge-dsl 双语法体系统一——远期

---

## 第二十七轮增补(forge-codegen + 工具链全面审查)

### 审计结论修正:pattern_isel 实际生效(重要)

- **原审计判断(关闭)前提有误**:explore 认为 isel_strategy 标签零消费→"匹配了但不优化"→建议关闭。实测发现:**standard_matcher 是手写注册路径**(与 TOML 生成的 lower_pattern 是两套),enable_pattern_isel=true 时 `apply` 的指令融合(lea/cmp-select/fma)**真实执行**,关闭后指令序列改变,暴露聚合返回 ABI 的槽位顺序 bug(text_parse_compile_run_large_agg_ret_abi 5 vs 6)。
- **最终处理**:恢复 enable_pattern_isel=true(保持既有行为);泄漏修复保留(Box::leak → 静态标签,apply 功能不变);真正断点(isel_strategy 标签零消费 + DSL lower_pattern arm 名不匹配)记录为后续接通工作。

### 本轮落地

| 项 | 说明 |
|---|---|
| Box::leak 泄漏修复 | pattern_isel.rs lea_sib/cmovcc 标签改静态字符串(不再泄漏堆内存) |
| with_reg_alloc 删除 | compiler.rs 兼容层 API(零调用) |
| register_riscv_standard_patterns 删除 | pattern_isel.rs 空函数(#[allow(dead_code)]) |
| forge-codegen→forge-grammar 死依赖删除 | Cargo.toml(源码零引用) |
| new_text_only 删除 | forge-object 零调用构造器 |
| compiler.rs 拆分 | pack_agg_bytes/pack_agg_into_bytes 迁至 pipeline/agg_const.rs(纯字节打包模块) |
| avx2_available 保留 | 有跨 crate 调用(forge-tests jit.rs:4277) |

### 事故与恢复记录(git stash 操作教训)

- 验证测试时 git stash + 误 drop,六轮未提交成果一度丢失
- 恢复路径:git fsck --unreachable 找到 stash commit(febe78c,compiler.rs 2268 行)→ stash apply(先清理冲突的 tracked/untracked 文件)→ error.rs 未跟踪文件丢失后从用法反推重建(IrError 枚举 14 变体 + From<String>/&str + Display 前缀大写——eh_unsupported 测试断言 "Unsupported" 修复)
- 教训:stash drop 前必须确认 pop 完全成功;未跟踪文件(如 error.rs)不在 stash 内,需单独备份

### 遗留(记录)

- isel_strategy 标签零消费 + DSL lower_pattern arm 名不匹配(lea-merge-iadd-imul-* vs lea_sib)——接通工作,需统一命名
- forge-grammar semantic 层(30KB)零外部调用——接入 mini_c 或标记
- forge-tests wasm32/minimal_sd 注释停用——路线图
- forge-tests fuzz 模块孤儿化与 harness 重复——归并
- forge-codegen 5 处"暂不支持"分支(聚合>16B 栈传递/128 位 SIMD/invoke)——路线图
- jit register_external 静默吞错、TargetAssembler stub——记录

## 第三十三轮:全面结构优化(三路并行审计 + 落地)

**审计**:3 个并行只读子代理(forge-ir / forge-codegen+dsl / opt+tools)全仓 grep 复证。

**落地(~60 处)**:
- forge-ir:analysis 5 方法、constant 5 迭代器、symbol 11 构造器/查询、metadata 死变体 Type/Value + 3 包装、function add_alias、builder type_store/set_loc/current_loc、types 4(function_ty/make_signature/signature_count/float_format)、terminator args_to_mut/map_values、data_layout 3 平台、big 浮点便捷方法组(恢复 abs——forge-opt const_fold 跨 crate 调用)、测试残留 2 处
- codegen:regalloc_trait.rs 转发壳整体删除(RegisterAllocator 并入 regalloc_bt,compiler/lib.rs 同步)、compile/compile_with/list/add_compiled/function_names、alloc_result 死方法 + 测试、from_alloc_result、isa_info 5 特性方法、is_memory_access、gvn may_alias、mem2reg/sccp 误标 allow 移除
- object:add_rodata_with_relocs/写路径收口(恢复 add_function_with_alignment——add_function 依赖)
- 用户决策:wasm32/minimal_sd 测试模块删除(isa/mod.rs 注释清理);pattern_isel **保持 true**——实测关闭破坏 text_parse_compile_run_large_agg_ret_abi(机制未明,审计"零消费"结论不完整——恢复 true 后通过,记录待诊断)

**回滚**:Frag≡AsmFrag 合并(误删 inst_operands.push 关键行致 proc macro panic len 0——回滚,记录需更仔细迁移)

**遗留(文档记录)**:forge-hir registry 6 + builder 15 零调用、ind_var_simplify 死字段、Frag 合并重试、agg_expand.rs 提取、CodeSink 移 machine/、pattern_isel 关闭诊断、arch 测试 helper、jit.rs 190KB 拆分、双轨生成器 v11 迁移

**验证**:workspace 48 suite 全绿、forge-ir 237 lib 测试、compat 198/452、roundtrip 198/198。

## 第三十四轮:遗留项验证结论(审计可靠性修正)

**教训**:第 33 轮审计结论两次被实测证伪——
1. forge-hir builder 15 个"零调用"方法中 **8 个是生产调用**(iadd/isub/imul/sdiv/srem/bnot/ishl/sshr 被 lowering.rs:237-252 使用,审计漏查);
2. pattern_isel 关闭破坏 agg_ret_abi(审计"isel_strategy 零消费"不完整)。

**本轮回执**:forge-hir 仅 7 个(load/store/alloc_slot/in_block/build_*)可能零调用但 mini_c codegen_hir 经 ctx.alloc_slot 间接使用——**需现场验证,不做删除**;CodeSink 引用面 ~10 文件(machine/encoder.rs、frame.rs、emission.rs、packer.rs、x86_64.rs 等)——机械重构记录;ind_var/Frag/agg_expand/arch helper 同列"需先验证调用关系"。

**收敛**:剩余 9 项遗留全部记录为"验证驱动"——不再凭审计结论直接删改;workspace 48 suite 全绿保持。

## 第三十五轮:根因修复 + 审计方法论修正(第三轮证伪)

**P1 pattern_isel 根因修复(核心)**:
- 根因确认:needs_agg_expand 缺 has_large_agg_ret 检测——大聚合返回的纯 call+extractvalue 函数靠 pattern 开关蹭 func_owned 克隆才展开
- 落地:has_large_agg_ret + has_large_agg_call_result 补入检测(展开独立于开关触发,trace 验证 false 下展开执行);run_pattern_matching 写回禁用(apply 只打标签零消费,写回纯消耗——true+禁用写回验证行为不变);开关保持 true
- **遗留之谜**:false 开关下 5 vs 6 仍复现——matcher=Some/None 路径对 func 净效果相同但行为不同,机制未明(疑似生成代码隐藏差异)——关闭开关需先解谜
- 验证:forge-tests 432 passed 全绿

**P2 Frag≡AsmFrag 合并重试成功**:双份定义删(改 pub use AsmFrag)+ AsmFrag→Frag 转换层删(直接 clone)——保留 inst_operands.push 关键行(上次教训);workspace 48 全绿

**P3a gvn 常量折叠合并**:gvn 的 try_const_fold(9 opcode 手写 + truncate_to_type/bit_mask 双份 helper)统一走 const_fold::fold_opcode(sccp.rs:277 先例)——删 ~70 行;除零 Err→None 保守跳过(与 gvn 原行为一致);forge-opt 80 passed 无回归

**审计方法论修正(第三次证伪)**:forge-ir 死 API 清单(~90 项)大面积不可信——bind_name/load_with_flags/store_with_flags/vconst_bytes/cmpxchg/extract_value/insert_value/ptrtoint 等全是 forge-ir 内部生产调用(semantics/display/verify),子代理 grep 排除自身文件时把内部调用也排掉了。**结论:forge-ir 死 API 批量删除整体取消**——该模块内部调用面最大,子代理方法论不适用,需逐个现场验证

**记录(验证驱动)**:agg_expand.rs 提取(改动点清单已给)、cse 剔除(gvn 超集但行为相关)、双轨生成器合并(错误处理差异)、jit.rs helper 复用(17 个 run_*)、arch 测试骨架(逐字复制)、README wasm32 过期、forge-hir 7 个方法、ind_var 死字段

## 第三十六轮:pattern_isel 之谜机器码定位 + 文档同步

**机器码 diff(决定性定位)**:true/false 下 make/main 编译产物对比——
- make 两态相同(86 字节,双段返回 RAX/RDX 正确)
- main:true 84 字节(双 mov 直接取段)、false 100 字节(shr r14, 64 通用移位路径——x86 shr 64 模 64 = 移 0 位 → 取低段 5)
- **结论**:false 时 main 的 call 结果拆段(expand_large_agg_call_results + rewrite)未生效,extractvalue 1 落入 expand_large_aggs 通用移位路径——但展开代码无条件执行且逻辑正确(静态无法解释;疑点:matcher=Some 执行路径对后续 lowering 的隐式影响/并行编译时序)——**归档:需 probe 展开中间态才能定位,关闭开关前必解**

**README 同步**:WASM32 行加注(编译后端保留,forge-tests 测试模块已移除);forge-tests README 第 3/14 行同步。

**记录(验证驱动)**:jit.rs 17 个 run_* helper 复用 harness(200+ 测试签名改动,风险/收益不佳)、arch 骨架提取(同)。

## 第三十七轮:pattern_isel 关闭之谜终解(编辑事故根因)

**根因(probe 中间态 trace 定位)**:第三十五轮加的 has_large_agg_ret/has_large_agg_call_result 检测在编辑事故中从**实际生效的 needs_agg_expand** 丢失(35 轮 trace 打印的是死代码变量,func_owned 实际用旧版检测)——false 下 main agg=false → func_owned=None → 展开跳过 → 5;true 靠 needs_pattern 掩盖。36 轮"matcher=Some/None 净效果相同行为不同"之谜 = 假象。

**修复**:检测函数重加 + needs_agg_expand 补全(唯一生效处);compiler.rs 经 git checkout 后重放 33-37 轮改动(regalloc_bt 引用、检测、needs_agg_expand)。

**成果**:**enable_pattern_isel=false 下 forge-tests 432 passed 全绿**(agg_ret_abi 通过)——用户"关闭开关"决策兑现;pattern_isel.rs 标注待接通(统一标签命名后重新启用)。

**验证**:forge-tests 432、workspace 48 suite 全绿。

## 第三十八轮:小步清理(loop_info 2 + ind_var 4 字段)

**方法论确认**:现场 grep 验证 + edit_file 精确删除(不用 sed 大范围)的小步模式高效且零事故:
- loop_info.rs 删 `is_loop_header`/`top_level_loops`(零生产调用,仅自身测试——测试改用 all_loops)——forge-ir 235 passed
- ind_var_simplify.rs 删 IndVar 4 个只写不读字段(param_idx/header/step/init_val)+ allow(dead_code)(param_val 唯一活跃)——forge-opt 80 passed
- **子代理清单再证伪**:immediate.rs 的 as_u64/as_type 是活跃 API(display/verify/forge-hir/forge-rustc 调用)——"零调用"结论第四次不可信

**验证**:workspace 48、forge-tests 432 全绿。

## 第三十九轮:小步清理续(sig_mask + 方法论修正)

- **sig_mask 删除**(FloatFormat,现场 grep 零调用——工具 grep 确认)——forge-ir 235 passed
- **constant.rs 7 项/terminator remove_arg/immediate as_u64/as_type/imm_str 6 项现场验证**:全部活跃(display/verify/forge-hir/forge-rustc/forge-dsl 生成代码调用)——子代理"零调用"清单第五次证伪
- **imm_str is_inline/is_static/is_shared**:仅测试用但测试断言面大(~10 处),收益/风险不佳——回滚保留
- **方法论修正**:多段 sed 行号删除仍脆弱(imm_str 连续编辑失控 2 次)——小步模式正确形态 = 单方法 edit_file 精确替换(loop_info/ind_var/sig_mask 成功先例)

**验证**:forge-ir 235、workspace 48 全绿。

## 第四十轮:最终收敛(死 API 清单彻底作废)

- **metadata 模块 16 项现场验证**:AttachedMetadata/MetadataKind/MetadataStore 全部活跃(dfg/function/terminator/semantics/display 核心使用)——35 轮"整模块零依赖"结论荒谬(子代理清单**第六次证伪**)
- **最终结论**:forge-ir 死 API 批量清单彻底作废——历轮现场验证仅 sig_mask/is_loop_header/top_level_loops/IndVar 字段等极少数真死;剩余项(agg_expand 提取/jit helper/arch 骨架/双轨生成器/PrevLit)为结构性重构,均需小步 edit_file 迁移
- **最终状态**:workspace 48 suite、forge-tests 432(enable_pattern_isel=false)、forge-ir 235、forge-opt 80、compat 198/452、roundtrip 198/198 全绿

## 第四十一轮:agg_expand.rs 第一批迁移完成

- **agg_expand.rs 建立**(pipeline/agg_expand.rs,5205 字节):has_* 检测族 7 函数迁入(has_gep/has_large_agg_ret/has_large_agg_call_result/has_large_agg_call_arg/has_large_agg_param/has_large_agg_load/has_agg_mem_access)+ 聚合入口 has_any_agg(合并 7 检测)
- **compiler.rs 瘦身 ~150 行**:7 函数删除 + needs_agg_expand 改调 `agg_expand::has_any_agg`
- **方法论**:git checkout 恢复 + edit_file 精确替换(替代 sed 行号——多次失控后确立)
- **验证**:forge-tests 432 全绿(has_* 迁移无行为变化)、workspace 48 全绿、codegen 0 error
- **后续分批**:rewrite 族(rewrite_agg_value_uses/memoryize_from_segs/memoryize_from_addr/move_to_front + AggSlots/UseJob/UseKind/UsePos 类型)与 expand 族(expand_agg_stores/expand_large_agg_*/expand_agg_call_args/expand_large_aggs/expand_geps)继续按每批 1-2 函数迁移

## 第四十二轮:clippy 修复(22 → 3 项)

**修复清单**(现场验证 + edit_file 精确替换):
- compiler.rs 残留 has_large_agg_ret/has_large_agg_call_result 删除(41 轮迁移残留副本)
- semantics.rs elem_ty(2133)/fr(3935) 未用绑定:下划线化 + contains_key(判定功能保留)
- analysis.rs DominatorTree.idom 死字段删除(compute_idom 局部保留)
- loop_info.rs header_to_loop 死字段删除(build 局部保留)
- types.rs TypeKey::Function 死变体删除
- ind_var_simplify.rs init_val 局部 + get_arg_for_block 死函数删除
- compiler.rs Job.block 死字段删除
- clippy --fix 风格项(empty doc line/单模式 match/等)+ 人工复核
- display.rs 单模式 match → if let(修复多余闭);const_expr_value ctx → _ctx(仅递归传递)
- **恢复**:create_block_with_tys(302 轮误删——含 block_open 修复,licm 回归)+ create_block_with_params 参数名绑定循环(display 测试回归)
- **功能核查结论**:所有 unused 均为纯冗余绑定/死字段/死变体,零功能影响;唯一真实修复是 create_block_with_params 绑定循环(302 误删恢复)

**验证**:clippy 22→3 项(仅 too_many_arguments ×3 记录遗留);forge-ir 7 suite、forge-tests 432、workspace 48 全绿

## 第四十三轮:agg_expand 第二批 + clippy 清零

**P1 agg_expand 第二批(类型迁移)**:AggSlots/UsePos/UseKind/UseJob 4 类型迁入 agg_expand.rs(pub(crate)),compiler.rs 删定义 + use 导入(25+ 引用点经编译驱动)——rewrite 族/expand 族函数迁移为第三批(每批 1-2 函数+编译)。

**P2 too_many_arguments ×3**:gvn_dfs/finalize_phis/build_terminator 为内部遍历上下文参数组——#[allow(clippy::too_many_arguments)] 标注(设计权衡,非缺陷)——clippy 22→0 全清零。

**P3-P6 评估记录**(验证驱动分批):双轨生成器 flush/伪指令共享骨架(错误处理差异行为相关)、PrevLit 三处共享扫描(跨模块)、jit.rs 17 个 run_* 复用 harness(200+ 测试签名)、arch 骨架 compile_ok_arch(逐字复制)。

**验证**:clippy 0 警告、forge-tests 432、forge-codegen 0 FAILED、workspace 48 suite 全绿。

## 第四十四轮:PrevLit 评估证伪 + 状态收口

- **PrevLit 三处重复(P4)评估证伪**:asm_grammar 的 shape_key(构建 PrevLit 状态机)与 frag_to_rule(消费同一 PrevLit 枚举)是**生产-消费关系**(共享枚举合理,非重复);asm_resolver 的 prev_lit 闭包是独立目的(字符串 ends_with([) 内存判断)——35 轮子代理"三处重复"误判,P4 取消。
- **agg_expand 第三批**(rewrite 族 4 函数 ~400 行 + 调用点)记录专轮(大搬移需 write_file 重建 + 调用点改路径)。
- **状态**:clippy 0、forge-codegen 6 suite、workspace 48 suite 全绿。

## 第四十五轮:双轨生成器合并 + PrevLit 评估

**双轨生成器 flush/伪指令共享骨架**(两处逐字重复消除):
- flush_emit_stmts(pending/stmts/emit_inst/rm/sink 参数化拼接)——cst_codegen.rs 与 codegen/mod.rs 的 flush 闭包提取为公共函数(生成代码 token 拼接等价,forge-tests 432 验证)
- gen_pseudo_inst(name/model/stmts)——5 分支伪指令 match 提取(两处调用统一)
- PrevLit(第四十四轮)评估证伪记录保留

**验证**:forge-dsl 0 error、forge-codegen 6 suite、forge-tests 432、workspace 48 suite 全绿。

## 第四十六轮:jit helper 转发 harness + arch 评估

**jit.rs 6 个纯转发 helper** → harness 单实现(run_test→run_i32、run_bool_test→run_bool、run_test_f64→run_f64、run_test_i64→run_i64、run_test_f32→run_f32、run_test_block→run_block);harness 新增 run_f32/run_bool(返回 I8→u8 转换)/run_block 3 个 helper;每 helper 15 行样板 → 1 行转发(约 90 行消除)。vec 系列 5 个 helper 保持(自带 lane 提取逻辑,非转发)。

**arch 骨架评估**:8 文件(aarch64/riscv64 × int/control/float/io)的局部 compile_ok wrapper 已是 harness::compile_ok 的薄包装(无重复);测试体逐字复制为设计选择(两架构同组合验证)——不合并。

**验证**:forge-tests 432、workspace 48 suite 全绿。

## 第四十七轮:forge-hir 验证 + 剩余状态收口

**forge-hir registry 验证**:register_atom/lookup_atom 等 12 个方法全部活跃(lowering.rs 151/618-721 内部大量调用)——子代理"6 个零调用"第 8 次误判(内部调用面),无删除项。

**剩余状态**:
- agg_expand 第三批(rewrite 族 4 函数 ~400 行):当前工具约束(无剪切工具+禁终端改码)下转录风险不可控——记录专用工具/步骤
- vec 系列 5 个 helper:自带 lane 逻辑,保持
- arch 骨架:薄包装,保持

**验证**:forge-hir 3 suite、workspace 48 suite 全绿。
