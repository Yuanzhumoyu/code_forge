# forge-ir 迭代清单（前瞻 backlog）

> 定位：本文件是**前瞻迭代计划**（与 `forge-ir-iteration-roadmap.md` 的"历轮成果记录"
> 互补）。每轮开工前从本文件挑选迭代项，完成后把成果回写 roadmap 附录 §7 与本文件
> 状态列。最后更新：2026-08（当前基线：compat 452 全收敛 198/254/0、roundtrip
> 0 缺口、workspace 48 suite 全绿）。**本文件所列 S1-S6 与 L3-L5（文本层部分）已全部
> 落地，§1 现为历史实现记录。**

---

## §0 迭代项总览

| # | 迭代项 | 档位 | 现状 | 依赖 |
| --- | -------- | ------ | ------ | ------ |
| S1 | 嵌套聚合字段提取（内存化） | 短期 | ✅ 第十轮已实现（compiler.rs:270-277 内存化） | — |
| S2 | TypeOps 单类型第二操作数 | 短期 | ✅ 第十轮已实现（grammar BinaryOp/BinaryOps 专用规则） | — |
| S3 | 文本层剩余小项（opaque/属性组/global 尾 metadata/fpext-fptrunc） | 短期 | ✅ 第十轮全部落地（S3.1-S3.4，见 §1） | — |
| S4 | verify 语义校验剩余单例 | 短期 | ✅ 第十轮已实现（S4.1-S4.3，见 §1） | — |
| S5 | 工程稳健性（determinism 偶发、load 快照） | 短期 | ✅ 第十轮：determinism 定位为 cargo 链接竞争；快照语义记录说明 | — |
| S6 | metadata 形状校验扩展（可选） | 短期 | ✅ 第十轮已实现（splat/浮点 hex/denormal_fpenv 等） | — |
| M1 | grammar 拆分评估（5.1） | 中期 | 增量编译 5-10s 可接受，>15s 再议 | — |
| M2 | 解析基准细化（5.2） | 中期 | ~57µs 基线已建 | — |
| M3 | 官方用例正向扩充 | 中期 | ✅ 已达成 **198/452**（第二十三轮收敛，误接受 0） | — |
| L1 | DI 验证器（~40-46 负向误接受） | 长期 | 明确不做（独立工程）；终态误接受 0（DI 校验器收口） | — |
| L2 | >16 字节聚合 ABI 栈传递 | 长期 | 需先定栈帧 ABI | — |
| L3 | vscale 可伸缩向量 | 长期 | ✅ 文本层已实现（第十二轮 lexer 单 token）；TypeId/codegen 扩展仍不做 | — |
| L4 | inline asm | 长期 | ✅ 文本层已实现（第十六轮 call/tail/callbr 三形态）；编码器集成仍不做 | — |
| L5 | callbr/token/statepoint/gc/blockaddress | 长期 | callbr（十二轮）/token（十轮）/gc（十七轮）/blockaddress（十六轮）文本层已实现；statepoint 仍不做 | — |
| L6 | 裸全局引用 init（`@p = global ptr @h`） | 长期 | LALR 本质歧义（附录 §6） | — |
| L7 | SjLj 异常代码生成（3.2 P1.2） | 长期 | 文本层已完成 | — |
| L8 | Windows SEH / ARM EH | 长期 | 随 ABI 选择 | — |
| L9 | 旧式指针/intrinsic/autoupgrade | 长期 | 按旧格式排除 | — |
| L10 | 多值 ret（`ret i32 1, i32 2`） | 长期 | 非 LLVM 标准 | — |

---

## §1 短期迭代项

> ⚠️ **本章为历史实现记录**：S1-S6 已全部由第十轮落地（执行明细见
> `forge-ir-iteration-checklist.md` §0.1 第十轮日志），以下问题解析/代码演示保留供
> 维护参考，不再作为待办。

### S1 嵌套聚合字段提取（内存化方案）——✅ 已实现（第十轮）

>**现状与失败现象**

`extractvalue` 的字段类型本身是聚合时（如 `{{i32, i32}, i32}` 取字段 0），
`rewrite_agg_value_uses` 直接报 Unsupported：

```rust
// crates/backend/forge-codegen/src/pipeline/compiler.rs:127-133
let seg_ty = ts.aggregate_elem_type(agg_ty, idx).ok_or_else(|| {
    IrError::Unsupported(format!("extractvalue 字段类型缺失：{idx}"))
})?;
if ts.is_aggregate(seg_ty) {
    return Err(IrError::Unsupported(format!(
        "大聚合参数嵌套字段提取暂不支持：type {seg_ty:?}"
    )));
}
```

同类：聚合常量打包处 `compiler.rs:1220`（"聚合常量打包：嵌套聚合缺失"）。

>**解决思路（4 步，全 ISA 通用）**

1. **外层提取**：字段类型为聚合时不再按"段值"展开，改为分配一个 alloca 帧槽
   （复用 `StackAddr`/`lea_off` 帧槽机制，按 `size_bytes` 对齐），把字段的各段
   写入槽内；
2. **段来源**：与现有 `expand_large_aggs` 段链一致——load 结果（追踪 addr 插段
   load 后逐段 store）、AggConst（`pack_agg_segments` 字节常量逐段 store）；
3. **结果 Value**：类型保持聚合，def 记为槽地址（复用 `ValueDef` 的地址语义，
   与 GEP 结果一致）；
4. **内层提取**：`extractvalue` 的 operands[0] 为槽地址时——标量字段走
   GEP（`field_offset`）+ load；聚合字段递归走步骤 1。

>**代码演示（rewrite 分支改造草图）**

```rust
// rewrite_agg_value_uses 的 ExtractValue 分支（compiler.rs:110 附近）
Opcode::ExtractValue => {
    let off = ts.field_offset(agg_ty, idx)?;
    let seg_ty = ts.aggregate_elem_type(agg_ty, idx)?;
    if ts.is_aggregate(seg_ty) {
        // 新：内存化路径——分配帧槽，把字段段写入槽，结果 = 槽地址
        let slot = alloc_frame_slot(&mut func.ctx, ts.size_bytes(seg_ty))?; // 复用 alloca 预扫描
        let field_segs = slice_field_segments(segs, off, ts.size_bytes(seg_ty))?;
        for (seg_off, seg) in field_segs {
            let addr = gep_from_slot(slot, seg_off)?; // lea_off
            store_seg(addr, seg)?;                     // 段宽 store
        }
        result_def = ValueDef::SlotAddr(slot, seg_ty);
    } else {
        // 现有标量路径（off/8 段 + off%8 移位）保持不变
    }
}
```

**相关问题**

- **跨段切片**：`off % 8 != 0` 时字段跨越两个 64 位段——标量路径用移位拼接；
  聚合路径建议先支持"段对齐字段"（off 为 8 的倍数），非对齐字段保持 Unsupported
  （时间盒口径）；
- **槽生命周期**：帧槽在函数入口预扫描分配（与 Alloca 同机制），extractvalue
  所在块插入 store 序列——需保证 store 在 extractvalue 之前、且只执行一次
  （提取结果作为值传播，避免重复插 store）；
- **与 `expand_large_agg_params` 的互操作**：参数聚合的嵌套提取同样走此路径。

**验收**：`{{i32,i32}, i32}` 两级提取端到端执行对照 clang；`verify` 全量回归。

---

### S2 TypeOps 单类型第二操作数（最大兼容性拦路）——✅ 已实现（第十轮）

**现状与失败现象**

`TypeOps` 闭包（`grammar.lalrpop:1771`）要求**每个**操作数都带类型：

```lalrpop
TypeOps: Vec<ParsedOperand> = {
    <mut v: (<TypeOp> Comma)*> <p: TypeOp> => { v.push(p); v }
};
```

而 LLVM 现代语法二元指令第二操作数**无类型**（类型从第一推导）：
`add i32 %a, %b`、`add i32 1, 1`。失败用例：`block-labels.ll`（`add i32 1, 1`）、
`flags.ll`（`%y` 期待类型）——错误形如
`UnrecognizedToken { expected: ["PtrTy", "IntTy", ...] }`。

**已解锁的模式（第九轮）**：`fcmp`/`icmp` 通过 `CmpScalarValue`（1782）支持
单类型第二操作数：

```lalrpop
CmpOps = {
    <a: TypeOp> Comma <b: TypeOp> <metas: CommaAttach*> => (vec![a, b], metas),
    // LLVM 现代语法：第二操作数无类型（类型从第一推导）
    <a: TypeOp> Comma <b: CmpScalarValue> <metas: CommaAttach*> => {
        let a_ty = a.ty.clone();
        (vec![a, ParsedOperand { ty: a_ty, op: b }], metas)
    },
};
```

**LALR 冲突根因**：`<b: Value>`（完整版）与 `<b: TypeOp>` 的 first 集在
`LBrace`/`LBracket`/`LAngle` 重叠——struct/数组**类型**（`{i32, i64}`、`[4 x i32]`）
与 struct/数组**常量值**（`{i32 1}`、`[i32 1, i32 2]`、`<{...}>`）同前缀。
`CmpScalarValue` 用"标量子集"（LocalId/常量/undef/poison 等，无聚合字面量）规避。

**候选方案**

| 方案 | 做法 | 工作量 | 风险 |
|------|------|--------|------|
| A（推荐） | 每个二元指令规则加 `<a: TypeOp> Comma <b: CmpScalarValue>` 分支（复用 CmpScalarValue） | 每个指令规则 1 处 | LALR 状态膨胀（fcmp 已验证可行） |
| B | lexer 层把聚合常量值单 token 化（`{...}`/`[...]`/`<{...}>` → 专用 token），first 集彻底分离后 TypeOps 直接放宽 | 大（lexer + display + 嵌套聚合） | 根本解但改动面广 |
| C | display 改单类型输出 + parse 只收单类型 | 小 | 不兼容双类型输入（存量 round-trip 测试大量双类型）——不推荐 |

**代码演示（方案 A 草图）**

```lalrpop
// 以 Add 为例（各二元指令同模式）
BinaryOps: (Vec<ParsedOperand>, Vec<(String, MetadataRef)>) = {
    <a: TypeOp> Comma <b: TypeOp> <metas: CommaAttach*> => (vec![a, b], metas),
    <a: TypeOp> Comma <b: CmpScalarValue> <metas: CommaAttach*> => {
        let a_ty = a.ty.clone();
        (vec![a, ParsedOperand { ty: a_ty, op: b }], metas)
    },
};
```

**验收**：`block-labels.ll`/`flags.ll` 等通过，正向 57→70+；`lalrpop` 冲突保持 0；
`cargo test -p forge-ir` 全量回归。

---

### S3 文本层剩余小项——✅ 全部已实现（第十轮 S3.1-S3.4）

#### S3.1 `opaque` 类型

**现状**：grammar 无 `opaque` 类型规则（`grammar.lalrpop:1962` 仅注释旧式指针）。
涉及用例：`auto_upgrade_intrinsics.ll` 等。

**解决思路**：`ValueType` 规则加 `opaque` 关键字分支。LLVM 的 `opaque` 类型是
"不透明占位类型"（与 `ptr` 并列的容器外类型）——文本层建议宽松接受为
`ParsedType::Opaque`（display 原样还原），不参与大小计算（`size_bytes` 为 0 或
Unsupported 时显式报错）。

```lalrpop
// Type 规则新增分支
OpaqueKw => ParsedType::Opaque,
```

**相关问题**：`opaque` 出现在 load/store 目标时无大小——verify 需报
"opaque type has no size"；`opaque ptr` 组合（旧语法）继续拒绝。

#### S3.2 属性组引用变体

**现状**：`attributes #N = { ... }` 定义与函数级 `#N` 引用已支持（第九轮
attribute-builtin 通过）。剩余变体：call 指令上的 `#N`、`declare` 尾部属性组合。

**解决思路**：先跑 compat 失败清单确认具体 token（`cargo test -p forge-ir
--test llvm_assembler_compat -- --nocapture` 打印 `FAIL xxx: 错误`），按错误
token 逐类补 grammar 分支。

#### S3.3 global 尾 metadata

**现状**：`GlobalTail` 列表（`(Comma Opt)*` 模式）已支持（4.3 轮实证）。
剩余：特定 metadata kind（`!dbg` 在 global 上等）组合。

**解决思路**：同 S3.2——按失败清单补。

#### S3.4 fpext / fptrunc lowering

**现状**：opcode 存在（`opcode.rs:138-139`），但 `isa/x86_v10.toml` **无任何
lowering 规则**（`grep -c "Fpext|Fptrunc" isa/x86_v10.toml` = 0）——执行层
Unsupported。

**解决思路**：x86_v10.toml 加规则（复用第九轮 fcmp 的 elem 变体模式）：

```toml
[lower.Fptrunc]
variants = [
  { when = "elem==F64 && imm0==F32", insts = ["movsd rd, rs1", "cvtsd2ss rd, rs1"] },
]
[lower.Fpext]
variants = [
  { when = "elem==F32 && imm0==F64", insts = ["movss rd, rs1", "cvtss2sd rd, rs1"] },
]
```

（`imm0` 为目标类型谓词——需确认 DSL 对"目标类型"的谓词支持；若不可行则
lowering.rs 特判分派。）

**验收**：`fptrunc double 1.5 to float` / `fpext float 1.5 to double` 端到端执行
对照 clang。

---

### S4 verify 语义校验剩余单例——✅ 已实现（第十轮 S4.1-S4.3）

**现状**（roadmap §7）：`check_gep_indices` 已做（标量位置后续索引、struct
i32 常量）；剩余 3 个缺口：
- `getelementptr_struct` 索引**常量**（非常量索引的 struct 位置校验）；
- `global-init cast`（global 初始化的 bitcast/ptrtoint 链类型匹配）；
- `alias` 前向引用（alias 指向未定义符号的延迟校验）。

**解决思路**：每类在 `semantics.rs` 的 verify 路径加一处检查 + `verify_negative.rs`
补负向回归：

```rust
// 示例：alias 前向引用
if let Some((name, _)) = &alias.target_name {
    if !module.symbols.contains_key(name) {
        return Err(IrError::Semantic(format!(
            "alias target {name} is not defined in the module"
        )));
    }
}
```

**验收**：对应负向用例从"误接受"转"正确拒绝"（DI 之外的非 DI 误接受进一步下降）。

---

### S5 工程稳健性——✅ 已处理（第十轮：S5.1 定位、S5.2 记录）

#### S5.1 determinism 测试偶发

**现状**：`forge-dsl/src/codegen/mod.rs:3497` `determinism_tests`——workspace
并行（`-j 4`）下偶发 FAILED（生成 3MB diff），单独跑必过（160 次循环未复现）。

**排查思路**：
1. 抓现场：`cargo test -p forge-dsl --lib determinism` 单独跑；若不过则
   `diff` 两份生成串定位首个分歧 token；
2. 并行干扰假设：build.rs / 多 crate 并行编译竞争同一生成文件——对比
   `-j 1` 与 `-j 4` 复现率；
3. 若确认为 HashMap 迭代序：生成路径改用 `BTreeMap` 或排序收集（`is_negative`
   已证明 sorted 收集可修）。

**复现命令**：
```bash
for i in $(seq 1 10); do cargo test --workspace --exclude forge-rustc -j 4 2>&1 | grep -c FAILED; done
```

#### S5.2 load 快照语义

**现状**（roadmap §7）：`load` 后内存被改写时，重新 load 不保留旧值（编译器
"load 值传播"模型）。常见模式安全（内存写入与 load 在同一函数内顺序执行）。

**解决思路**：若出现"load 快照被破坏"用例——在 store 到 load 地址时插入显式
拷贝（≤8 字节小聚合可整值拷贝）；文档记录该语义限制，避免误用。

---

### S6 metadata 形状校验扩展（可选）——✅ 已实现（第十轮）

**现状**（`semantics.rs:483-498`）：`!dbg` → 必须 `DILocation`；`!tbaa` →
tuple tag 结构（`!{!{...}, i64 1}`）；`!range` → 整数区间形状。

**可选扩展**：`!prof`（权重元组）、`!alias.scope`/`!noalias`（作用域节点）、
`!llvm.loop`（循环 metadata）——按失败用例驱动，无用例不扩展（避免过度工程）。

---

## §2 中期迭代项

### M1 grammar 拆分评估（roadmap 5.1）

**现状**：lalrpop 生成单文件 `ir_parser/grammar.lalrpop`（~2000 行），增量编译
5-10s（第九轮实测可接受）。附录 §6 备忘：**新增 token 后必全量回归**
（模块 item first 集合膨胀影响全局状态图）。

**评估触发条件**：增量编译 >15s 或 lalrpop 冲突难收敛时再议拆分。

**拆分候选**（若触发）：
- 按模块 item 类别拆：`Types`（Type/TypeDef）/ `Globals`（GlobalKind 系）/
  `Functions`（FuncAttrs/Inst 系）——但 LALR 状态图是全局的，拆文件不拆状态；
- 实质拆分：grammar 模块化（`include!` 或 lalrpop 的 `include`）只改善组织，
  不改善编译时间；真正提速需缩小 token 集合（专用 token 化已做）或换生成器。

**结论（现状）**：维持不拆；每次语法改动后跑 `cargo check -p forge-ir` 确认
lalrpop 0 冲突 + 增量编译计时。

### M2 解析基准细化（roadmap 5.2）

**现状**：`benches/ir_parse.rs`（criterion）——`parse_medium_module` ~57µs、
`parse_display_roundtrip` ~148µs（第九轮基线）。

**细化方向**：
1. 用例扩充：异常（invoke/landingpad）、聚合（嵌套字面量）、metadata 密集模块；
2. 对比基线：`docs/bench_baseline.md` 每轮更新；
3. 热点分析（roadmap 5.2 候选）：`value_map` 查找、display 的
   `self.types.borrow()` 重复借用——收益不确定，按需推进。

### M3 官方用例正向扩充 57→80+——✅ 已达成（198/452）

**结果**：第二十三轮收敛至 **198/452**（误接受 0、正确拒绝 254，452 全收敛）；
中途各轮 +N 见 `forge-ir-iteration-checklist.md` §0.1 与 `forge-ir-remaining-iterations.md` §5。

**历史前置**：S2（TypeOps 单类型）解锁 block-labels/flags 类；S3（opaque/属性组）
解锁 auto_upgrade 类。

**历史推进方式**：
1. 重跑失败清单（`--nocapture` 打印 `FAIL xxx: 错误`）；
2. 按错误 token 聚类（每类 ≥1 个用例通过即算解锁）；
3. 每类补语法 → `cargo test -p forge-ir` 回归 → compat 统计更新 → 文件头
   分类清单同步；
4. 验收：正向 ≥80，误接受 47 不回升；剩余失败收敛到长期排除项。

---

## §3 长期 / 已排除项（启动前置条件）

### L1 DI 验证器（~40-46 负向误接受）

**现状**：文本层已支持 distinct/key:value（布尔、DWARF 枚举、`!N` 数字引用）、
`type:`/`align:` 等 key；字段值域/必填校验不做——LLVM DI 验证器是独立工程。
负向断言按防回归口径（非 0 上限）落地。

**启动前置**：无（明确不做；若未来要收，需按 `invalid-di*` 用例逐个实现
DINode 字段校验——工作量大且收益低）。

### L2 >16 字节聚合 ABI 栈传递

**现状**：≤16 字节走 SysV 整数寄存器（RAX/RDX + RCX/RDX 参数）；>16 字节
Unsupported（栈传递未实现）。

**启动前置**：栈帧 ABI 设计——参数区布局（red zone 之外、与 Alloca 帧槽
共存）、对齐（16 字节）、调用方/被调方职责（`expand_agg_call_args` 拆段 →
栈槽 store；`expand_large_agg_params` 收参 → 栈槽 load）。

### L3 vscale 可伸缩向量

**现状**：✅ 文本层已实现（第十二轮 `<vscale x N x T>` 单 token，lexer.rs:51-55——
5-token 序列致 LALR 状态爆炸回滚后采用）；TypeId 扩展与指令选择仍不做。

**启动前置**：TypeId 加 vscale 维度 + `size_bytes` 动态化（运行时 xlen 查询）+
指令选择（SVE 指令族）——独立子项目。

### L4 inline asm（`call void asm sideeffect "..."`）

**现状**：✅ 文本层已实现（第十六轮 AsmKw 专用 token + call/tail/callbr 三种 asm
变体）；编码器集成仍不做。

**启动前置**：无（文本层已完备；编码器集成若启动需 inline-asm 解析 + 指令编码）。

### L5 callbr / token / statepoint / gc / blockaddress

**现状**：callbr（第十二轮）、token 类型（第十轮）、gc 子句（第十七轮）、
blockaddress（第十六轮，`ptr blockaddress(@f, %label)`）文本层均已实现；仅
**statepoint** 仍未支持（依赖 intrinsics 体系）。

**启动前置**：statepoint 需 intrinsics 体系扩展——无近期计划。

### L6 裸全局引用 init（`@p = global ptr @h`）

**现状**：LALR(1) 本质 2-lookahead 歧义（`@h` 后 lookahead `=` 才知归属——
与"下一条模块级 global"冲突，附录 §6）；`ptr @h` 带类型形式可行
（GlobalConstExpr）。

**启动前置**：无（语法本质不可解）；若未来需要，用 `ptr @h` 带类型形式。

### L7 SjLj 异常代码生成（3.2 P1.2）

**现状**：异常文本层已完成（invoke/landingpad/resume/personality 全链路，
含 unwind 边 dominance 豁免）；codegen 对含异常函数报 Unsupported。

**启动前置**：`setjmp/longjmp` 模拟（`_setjmp` + 跳转表）绕开平台 EH ABI——
需要 libc 链接支持与帧展开设计。

### L8 Windows SEH / ARM EH

**现状**：随 3.2 的 ABI 选择——当前 x86_64 走 Dwarf/SysV。

**启动前置**：SEH（`.xdata`/`.pdata` 展开表）或 ARM EH（`.ARM.exidx`）——
平台 ABI 工程。

### L9 旧式指针 / 旧 intrinsic / autoupgrade

**现状**：`i32*`/`ptr*`、`metadata !{}` 旧语法、autoupgrade 旧 intrinsic
（`llvm.aarch64.thread.pointer` 等）按旧格式长期排除——仅支持最新版 LLVM IR。

**启动前置**：无（设计决策：只跟最新版）。

### L10 多值 ret（`ret i32 1, i32 2`）

**现状**：内部 `Return(Vec<Value>)` 已支持多值（大聚合返回用）；文本层 `ret`
单值——多值语法非 LLVM 标准。

**启动前置**：无（非标准语法，不做）。

---

## §4 工程速查附录

### 回归与统计命令

```bash
# workspace 全量（forge-rustc 需特殊 sysroot，排除）
cargo test --workspace --exclude forge-rustc

# forge-ir 全量（语法改动后必跑）
cargo test -p forge-ir

# compat 统计 + 失败清单（每轮开工/收尾跑一次）
cargo test -p forge-ir --test llvm_assembler_compat -- --nocapture
#   输出示例：parse ok 198/452（负向正确拒绝 254，误接受 0）
#   FAIL xxx.ll: parse error: ...（按错误 token 聚类推进 S3/M3）

# 负向语义校验（verify_negative 新增回归测试）
cargo test -p forge-ir --test verify_negative

# 执行层（text_to_exec 全量）
cargo test -p forge-tests --lib "isa::x86_64::text_to_exec"

# 解析基准（对比 docs/bench_baseline.md）
cargo bench -p forge-ir --bench ir_parse
```

### lalrpop 冲突检查（附录 §6 约束）

每次 grammar.lalrpop 改动后：

```bash
cargo check -p forge-ir   # 冲突会以 build 错误报出（Local ambiguity / ambiguity）
```

- 新增关键字 → **专用 lexer token**（`#[token("xxx", priority = 2)]`）+ extern
  块映射（终结符 first 与指令 Ident 分离——3.2 实证）；
- 列表式可选参数 → 每项以 `Comma` 开头整体匹配（AllocaOpt/GlobalTailOpt 模式）；
- 可空规则避免双重嵌套（内层 `(X)+`，可空只留外层 `?`）；
- lalrpop 0.23 无 `ambiguous_grammar` 宏——冲突只能重构语法。

### 每轮文档维护约定

1. 轮末把成果回写 `docs/forge-ir-iteration-roadmap.md` 附录 §7（历轮记录）；
2. 同步本文件状态列（已解锁项移到 §0 表尾标注"已解锁"或删除）；
3. 同步 compat 文件头基线（正向通过数/误接受数/已解锁类）；
4. 基准变化更新 `docs/bench_baseline.md`；
5. 新增 LALR 冲突形态 → 附录 §6 备忘表加行。

### 已知长期基线（当前，452 全收敛后）

| 指标 | 基线 |
|------|------|
| workspace suite | 48 全绿（第三十三轮移除 wasm32/minimal_sd 测试模块） |
| compat 正向 | 198/452 |
| compat 负向 | 正确拒绝 254、误接受 0 |
| roundtrip | 0 缺口（198 checked / 254 skip，常规回归） |
| parse 基准 | parse_medium_module ~57µs、roundtrip ~148µs |
| 增量编译 | 5-10s（lalrpop 单文件） |
