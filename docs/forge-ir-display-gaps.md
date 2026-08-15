# forge-ir display 保真度缺口清单（第二十九轮,持续更新）

> 全量 roundtrip 测试（display_llvm::roundtrip_all_assembler_cases,当前 #[ignore]）
> **当前:124 过 / 254 skip / 0 缺口——452 用例 display→reparse 全部通过**（第三十轮收尾:① GlobalAddr 函数引用回退——指令行与 value_as_literal 两处查全局失败回退查函数,opaque-ptr-intrinsic-remangling 通过;② ifunc resolver 双 @@ trim——ifunc 系列通过;③ 未注册全局哨兵 u32::MAX 占位(原 GlobalId(0) 与真实 id 0 冲突——数字名全局 skip-value-numbers-globals 通过)。452 compat 198 保持,roundtrip 测试可解除 ignore）。

## 已修复（第二十九轮,共 40 项）
1-16:见前版(16 项基线:lane/alloca/zeroinit8B/custom kind/空洞/datalayout/vector-ptr/distinct/metadata 类型/MetadataKw/asm 全链/ifunc IR/ptr null/denormal/向量 zeroinit/Vector 去重)
17. atomicrmw/cmpxchg 操作数类型去重(字面量自带类型前缀,原双输出致 `ptr ptr undef`/`i32 i32 1`)
18. 常量表达式 Binary 类型前缀(operand_text 带 op_ty + 操作数同类型,原 `add (5, -5)` 丢类型)
19. byval/sret 占位 void 的 roundtrip(attr_ty_to_type 接受 "void")
20. call 变参 `...` 全链(识别占位+Int(3) 标记+display 还原 musttail)
21. insertvalue/unnamed 操作数类型去重(字面量自带类型,原 `f64 f64 0x...`)

## 剩余缺口（13 个用例）

```
aggregate-constant-values.ll
amdgcn-unreachable.ll
atomic.ll
atomicrmw.ll
bfloat.ll
block-labels.ll
call-arg-is-callee.ll
callee-type-metadata.ll
constant-splat.ll
disubprogram-targetfuncname.ll
flags.ll
float-literals.ll
getelementptr.ll
ROUNDTRIP-FAIL: getelementptr_vec_ce.ll: assertion `left == right` failed: opcode:
half-constprop.ll
half-conv.ll
incomplete-ir-declarations.ll
incomplete-ir-metadata.ll
insertextractvalue.ll
metadata-function-local.ll
metadata-use-uselistorder.ll
metadata.ll
musttail.ll
numbered-values.ll
opaque-ptr-intrinsic-remangling.ll
opaque-ptr-struct-types.ll
opaque-ptr.ll
pr119818.ll
range.ll
scalable-vector-struct.ll
skip-value-numbers-globals.ll
skip-value-numbers.ll
unnamed.ll
uselistorder.ll
```

## 主要失败形态
- 前向引用→undef 占位(10 个:amdgcn/atomic/block-labels/incomplete-ir-*/opaque-ptr 系列/pr119818/skip-value-numbers/uselistorder)——表示层:operand_to_value 未定义 Local 保留名
- invoke 间接 callee(call-arg-is-callee)
- aggregate-constant-values(splat 类型保真)
- 表示层缺口(需 IR 扩展):前向引用→undef 占位(atomic/atomicrmw/block-labels 等)、invoke 间接 callee(call-arg-is-callee)、blockaddress
- 断言差异(Iconst 类型/opcode 保真):amdgcn/bfloat/constant-splat/flags/insertextractvalue 等
- reparse 失败(display 输出修正):call-arg-is-callee/float-literals/metadata/musttail/opaque-ptr 等

## 修复批次建议
批次 A''''(表示层):前向引用保留 + invoke 间接 callee——中-大工程
批次 B''''(reparse 逐例):float-literals/musttail/opaque-ptr/huge-array 等

每批验收:对应用例 ROUNDTRIP-FAIL 消失;全量 roundtrip 取消 ignore 后绿;452 compat 不破。
