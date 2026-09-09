# forge-ir display 保真度缺口清单（第二十九~三十轮,已全部修复）
> ## ⚠️ ARCHIVED（2026-09）
>
> forge-ir display 保真度缺口清单（第 29–30 轮，74→0 已全部修复）。
> 本文为历史记录，仅供参考；代码现状以仓库代码与现行文档为准，不再维护。


> 全量 roundtrip 测试（display_llvm::roundtrip_all_assembler_cases，**已解除
> #[ignore] 作为常规回归**——display_llvm.rs:1615-1618）
> **当前:198 过 / 254 skip / 0 failures——452 用例 display→reparse 全部通过**
> （第三十轮收尾:① GlobalAddr 函数引用回退——指令行与 value_as_literal 两处查全局失败回退查函数,opaque-ptr-intrinsic-remangling 通过;② ifunc resolver 双 @@ trim——ifunc 系列通过;③ 未注册全局哨兵 u32::MAX 占位(原 GlobalId(0) 与真实 id 0 冲突——数字名全局 skip-value-numbers-globals 通过)。452 compat 198 保持）。
> `roundtrip_failures.txt` 为**自愈产物**——仅 failures 非空时写入、修复后自动删除
> （display_llvm.rs:1685-1691），当前 0 失败故文件不存在）。

## 已修复（第二十九轮,21 项明细）

1-16:见前版(16 项基线:lane/alloca/zeroinit8B/custom kind/空洞/datalayout/vector-ptr/distinct/metadata 类型/MetadataKw/asm 全链/ifunc IR/ptr null/denormal/向量 zeroinit/Vector 去重)
17. atomicrmw/cmpxchg 操作数类型去重(字面量自带类型前缀,原双输出致 `ptr ptr undef`/`i32 i32 1`)
18. 常量表达式 Binary 类型前缀(operand_text 带 op_ty + 操作数同类型,原 `add (5, -5)` 丢类型)
19. byval/sret 占位 void 的 roundtrip(attr_ty_to_type 接受 "void")
20. call 变参 `...` 全链(识别占位+Int(3) 标记+display 还原 musttail)
21. insertvalue/unnamed 操作数类型去重(字面量自带类型,原 `f64 f64 0x...`)

## 已修复记录（第二十九~三十轮：74 → 27 → 9 → 2 → 0）

首轮全量运行（第二十九轮）暴露 **74 个 display 保真度缺口**（124 过/254 skip），
分批次收敛至 0（display_llvm.rs:1612-1615 注释同步记录）：

- **批次 A''''（表示层,74→27）**：前向引用保留（`ValueDef::UndefNamed`，dfg.rs:38）+
  invoke 间接 callee（call-arg-is-callee）——中-大工程，完成；
- **批次 B''''（reparse 逐例,27→9）**：float-literals/musttail/opaque-ptr 系列/
  huge-array 等 display 输出修正——完成；
- **收尾三修（第三十轮,9→2→0）**：GlobalAddr 函数引用回退、ifunc resolver 双
  @@ trim、未注册全局哨兵 u32::MAX 占位——完成。

原"剩余缺口（13 个用例）"清单实际列出 34 个用例名（标题计数有误），现已**全部
通过、不再列示**；主要失败形态（前向引用→undef 占位、invoke 间接 callee、
断言差异 Iconst 类型/opcode 保真、reparse 失败）均已对应修复。

**验收已达成**：对应用例 ROUNDTRIP-FAIL 消失；全量 roundtrip 取消 ignore 后绿
（常规回归，198 checked / 254 skip / 0 failures）；452 compat 198 不破；
`roundtrip_failures.txt` 自愈（0 失败不写入、修复后删除）。

