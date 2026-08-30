# HIR 收缩方案（forge-hir / mini_c 手写 lowering）

> 审查方案 P1 项。目标：在不改变行为的前提下收缩 `examples/mini_c/src/codegen_hir.rs`
> （~1300 行手写 AST→IrGraph lowering）与 forge-hir 库的重复结构。
> 守门：`examples/mini_c/tests/dual_backend_tests.rs`（Direct vs Hir 双后端
> 回归对比）——任何收缩必须保持 Hir 后端输出行为等价。
> 已实施的前置项：前端诊断升级（set_span/locate，82e0ece）、SourceSpan
> 专用结构体（dfce133）——错误定位逻辑已从 lowering 函数体收敛。

## 收缩点 1：二元 op 表去重（-40 行，低风险）

`apply_compound_op`（`+=`/`-=`/…）与 `lower_binary`（`+`/`-`/…）各自维护一张
「运算符字符串 → build_xxx」match 表（10 个 op，两处完全重复）。提取共享
helper：

```rust
/// 二元运算分派（compound assign 与普通二元表达式共用）。
fn bin_op(ctx: &mut HirCtx<'_, SymTable>, op: &str, l: GraphValue, r: GraphValue)
    -> Result<GraphValue, HirError>
{
    match op {
        "+" => minic_lowering::build_iadd(ctx.graph, l, r),
        "-" => minic_lowering::build_isub(ctx.graph, l, r),
        /* … 10 op … */
        _ => Err(HirError::Lowering(format!("unknown binary op: {op}"))),
    }
}
```

`lower_compare` 的 icmp cond 表同理（`build_icmp(ctx.graph, cc, …)` 的 cc 映射
与 `IntCC` 常量集中在 forge-hir 侧，mini_c 只保留「运算符 → 谓词」映射）。

## 收缩点 2：循环 lowering 三合一（-80 行，中风险）

`lower_while` / `lower_for` / `lower_do_while` 共享骨架：cond 块 →（条件
branch）body 块 / exit 块 → body 尾部 jump cond 块。三者差异仅在初始化
（for_init）、更新（for_update）与 do-while 的"先体后判"顺序。

提取骨架：

```rust
/// 循环骨架：cond_block + body + exit；init/update/先判后判由参数化覆盖。
fn lower_loop_common(
    ctx: &mut HirCtx<'_, SymTable>,
    cond_node: Option<AstRef<'_>>,   // None = do-while（先体后判）
    body_node: AstRef<'_>,
    update: Option<&dyn Fn(&mut HirCtx<'_, SymTable>) -> Result<(), HirError>>,
) -> Result<(), HirError>
```

注意：break/continue 的 loop 栈语义（`ctx.loops`）与 return 槽在三种循环中
必须保持一致——先做行为等价重构，再由 dual_backend 测试验证。

## 收缩点 3：struct 字段槽分配提取（-30 行，低风险）

`lower_struct_init` 与 `alloc_struct_fields` 各有一段「按字段数分配连续槽 +
登记 locals」的逻辑（`next_offset` 递减、`ctx.locals.insert("x.f", slot)`）。
提取 `alloc_struct(ctx, base: &str, field_names: &[String]) -> Result<(), HirError>`，
成员访问（`lower_member_access`）复用同一槽命名约定。

## 收缩点 4：错误传播统一（-20 行，低风险）

`codegen_function_hir` 中 ~10 处 `.map_err(|e| e.to_string())` 分散在
graph 构建的 `?` 传播点。统一为入口 `map_err(|e| ctx.locate(e).to_string())`
（lower_block 已做——见诊断升级），消除重复包装。

## 收缩点 5：minic_lowering 签名统一（-15 行，低风险）

`build_*` 混用 `Result<GraphValue, HirError>` 与裸 `GraphValue` 两类签名
（调用点被迫 `.map_err(...)` 或直接传）。统一为 Result 签名（与
`define_lowering!` 生成物一致），或按原子分组建 `impl Builder` 扩展。

## 收益估计与顺序

| 步骤 | 减少行数 | 风险 | 前置 |
| --- | ---: | --- | --- |
| 1 二元 op 表去重 | ~40 | 低 | 无 |
| 5 build_* 签名统一 | ~15 | 低 | 无 |
| 3 struct 槽分配提取 | ~30 | 低 | 无 |
| 4 错误传播统一 | ~20 | 低 | 已完成大半 |
| 2 循环三合一 | ~80 | 中 | 1 先落地（循环体含二元表达式） |

合计 codegen_hir.rs ~1300 → ~1100 行（-15%）。每步独立提交，跑
`cargo test -p mini_c`（dual_backend 回归 + 90 集成用例）守门。

## 不建议收缩（记录）

- `lower_expr` 的 precedence 分发（primary/assignment/logical_or/…）——
  与 grammar 规则结构一一对应，合并会破坏可读性；
- `flat_children` 的透明包装器穿透——语法层 rep/opt/seq 必需。
