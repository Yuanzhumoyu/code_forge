# HIR 收缩方案（forge-hir / mini_c 手写 lowering）

> 审查方案 P1 项。目标：在不改变行为的前提下收缩 `examples/mini_c/src/codegen_hir.rs`
> （手写 AST→IrGraph lowering）与 forge-hir 库的重复结构。
> 守门：`examples/mini_c/tests/dual_backend_tests.rs`（Direct vs Hir 双后端
> 回归对比）——任何收缩必须保持 Hir 后端输出行为等价。
> 已实施的前置项：前端诊断升级（set_span/locate，82e0ece）、SourceSpan
> 专用结构体（dfce133）——错误定位逻辑已从 lowering 函数体收敛。
>
> **2026-09-10 复核结论（先读）**：本文件原 5 个收缩点中，**0 个已实现**；
> 其中**点 5 的前提自始不成立**、**点 3 前提错误**、**点 1b 归错文件**，
> 收益估计（-185 行 / -15%）按实测不可达（现实 ~-60 – -85 行）。
> 复核同时发现并修掉了**两个真实缺陷**（见「本轮落地」）——优先于收缩本身。
> 下文各点保留原分析，括注【复核】给出修正。

## 本轮落地（2026-09-10）

### 先补守门网（必须先做，已做）

`dual_backend_tests.rs` 此前**没有任何 `continue` 用例**（全仓唯一 continue 测试
`v12_backend_tests::v12_break_continue` 跑的是 V12 后端而非 Hir），而
break/continue 的循环栈语义正是「循环 lowering 合并」最容易破的地方。新增 5 例：
`both_while_continue` / `both_for_continue` / `both_dowhile_continue` /
`both_nested_loop_break_continue` / `both_member_compound_assign`。

**这 5 例立刻暴露两个真实缺陷（均已修）**：

1. **`for` + `continue` 死循环（两后端同源）**：循环栈里 for 存的是
   `(cond_blk, exit_blk)`，`continue` 直接跳条件块 → **update 被跳过** → 变量
   不自增 → 无限循环（`both_for_continue` 挂死 >60s 实证）。
   修法：for 的 `continue` 目标改为 **update 块**（先 update 再判条件，C 语义）；
   Direct 后端（`codegen.rs`）与 Hir 后端（`codegen_hir.rs`）同步修。
2. **表达式级成员赋值被静默忽略（两后端同源）**：语法里语句级
   `assign_stmt ::= member_access "=" expr ";"` **只接受 `=`**，`p.x += 4` 因此
   经 `expr_stmt → assignment(member_access OP assignment)` 以 **`seq`** 形态到达，
   而两后端的赋值表达式处理只认 `IDENT` LHS → 落入二元兜底 → 整个赋值被丢弃
   （用例实证：两后端都返回未修改的字段和 3 而非 11）。
   修法：seq 形态识别（`[IDENT|member_access, OP 令牌, rhs]`）+ 运算符文本按
   **令牌源码区间**取——注意多字符运算符的 `AstRef::text()` 是 `PUNCT_2b3d`
   这类词法记号名而非字面量 `+=`（旧的整体扫描 `extract_assign_op` 也因此在
   "p.x += 4" 上失配）。
   **教训**：这类"静默丢弃"只有双后端对照用例能发现——守门网优先于收缩。

### 收缩点 1（二元 op 表去重）—— **已做**

抽 `bin_op(ctx, op, l, r)`（`codegen_hir.rs`），按「去掉尾随 `=`」归一化，
`apply_compound_op` 与 `lower_binary` 共用一张表（原两张 13 行 match 表）。
**实测净省约 10 行**（原估计 -40 偏高：两表键集不同，`apply_compound_op` 另有
`=` 早退 + `build_load`）。
【复核】真正重复的 icmp-cond 表在**直连后端** `codegen.rs`（两处 6 项表），
不在 `codegen_hir.rs`——本轮顺带把 Hir 侧的 cond 映射收敛为 `intcc_for()`
（`lower_compare` 与兜底链共用）。

### 收缩点 3（struct 字段槽命名）—— **按修正版做**

`field_key(base, field)` 收敛 5 处 `format!("{}.{}", …)`。
【复核】原描述「`lower_struct_init` 与 `alloc_struct_fields` 各有一段分配逻辑」
**不成立**：`lower_struct_init` 自始就调用 `alloc_struct_fields`
（写入时点 blob 逐字比对）；`lower_member_access` 也不分配槽。真实重复只有
字段名查询样板与键格式化——**实测净省约 10 行**（原估计 -30 偏高）。

### 循环结构（收缩点 2 的前置）

`HirCtx.loops` 由 `Vec<(BlockId, BlockId)>` 改为 `Vec<LoopFrame>`
（`cond_blk` + 懒创建的 `update_blk` + `update_needed` + `exit_blk`）。
**懒创建是必需的**：eagerly 建 update 块会让"体内只有 break/return、无 continue"
的 for 循环留下**不可达块**，被 IR 校验拒绝（`test_hir_e2e_break` 实证
`UnreachableBlock` / `DominanceViolation`）。

## 剩余工作（按优先级）

### 收缩点 2：循环三合一（仍开放，收益 ~45-50 行）

`lower_while` / `lower_for` / `lower_do_while` 现为三份实现（~126 行）。骨架应
覆盖真正的三写部分（建块 + cond 块 `iconst/icmp/branch` + "取当前块判
terminator"），三个入口保留薄壳。**注意**：

- 计划原给的 `update: Option<&dyn Fn(&mut HirCtx)->…>` 形参会与 `&mut HirCtx`
  形成双重可变借用——改为传 `AstRef`/枚举；
- 合并中心是 `LoopFrame` 的构造差异（`new_cond` vs `new_for`），语义已由本轮的
  `continue` 用例守门。

### 收缩点 4：错误传播统一（可选，净省 ~2 行）

`codegen_function_hir` 的 6 处 `.map_err(|e| e.to_string())` 需靠结构调整才减少；
且 `Display for Located` 已内联 `at L:C`，**不涉及诊断信息丢失**——纯计数项，
不建议为它增加复杂度。

### 收缩点 5：~~minic_lowering 签名统一~~（**删除**）

【复核】宏生成物 `forge-hir-macro/src/codegen.rs` **一律**返回
`Result<…, HirError>`（0 输出 → `()`，否则 `GraphValue`），自 `4fcdff1`
（2026-08-02，早于本计划）起即如此；forge-hir 全库无裸 `-> GraphValue`。
该收缩点无可做工作。

## 行数基线与现实收益

| 量 | 值 | 说明 |
| --- | ---: | --- |
| 本计划提交时（79b015c） | 1300 行 | 计划中的 "~1300" 准确 |
| 2026-09-10 复核前 | 1302 行 | 计划后唯一改动是测试模块 cfg 门控 |
| 2026-09-10 落地后 | **1427 行** | 净增：两个缺陷修复 + 守门网 + `LoopFrame`；同时做了点 1/点 3 的去重 |
| 收缩点 2 完成预期 | ~1380 行 | 若要回到 "1100 行" 需另找结构性收缩点（不在本计划范围） |

**结论**：本计划的收益估计应下调为 **~-60 – -85 行**（且只在收缩点 2 落地后
才体现），**"1300 → 1100（-15%）" 不可达**。收缩本身的价值低于"用守门网找出
静默错码"——本轮即为证明。

## 不建议收缩（记录）

- `lower_expr` 的 precedence 分发（primary/assignment/logical_or/…）——
  与 grammar 规则结构一一对应，合并会破坏可读性；
- `flat_children` 的透明包装器穿透——语法层 rep/opt/seq 必需
  （且本轮证明 `seq` 还承载"未命名规则的序列"信息：成员赋值即靠它识别）。
