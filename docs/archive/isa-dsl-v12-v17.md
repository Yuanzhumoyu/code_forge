# ISA-DSL v12–v17 语法史（已归档）

> ⚠️ ARCHIVED（2026-09-21）。本文只记录 **v12–v17** 的语法形态与 v18 删掉的机制，
> 不代表代码现状。**现行语法 = [`docs/reference/isa-dsl.md`](../reference/isa-dsl.md)（v18）**；
> 错误码目录见 [`docs/reference/isa-dsl-errors.md`](../reference/isa-dsl-errors.md)；
> 30 分钟上手见 [`docs/guides/isa-dsl-tutorial.md`](../guides/isa-dsl-tutorial.md)；
> 变更动机与逐节设计见 [`docs/archive/forge-dsl-v18-plan.md`](forge-dsl-v18-plan.md)。
> v12 时代的路线图另见 [`docs/archive/isa-dsl-v12-roadmap.md`](isa-dsl-v12-roadmap.md)。

v18 是**破坏性重设计**（用户明确"允许破坏性更新、无需兼容旧版本"）：旧写法不保留兼容层，
现在直接报错并给出迁移提示。本文的价值只有一个——查"某个旧键当年是什么意思、现在换成什么"。

## 1. v12–v13：模型起点

- **唯一语法 = 严格 TOML**（`deny_unknown_fields`）：v11 的 `encoding` 字符串、`@原语`、
  紧凑 `fields` 串、`when` 谓词串全部被删除。v11 语法层已物理删除。
- 宽度是**全局单值**：`[meta].default_inst_width` + `[meta].variable_length` + `[meta].max_inst_len`
  三个键描述"定宽还是变长"，无法表达"短编码与长编码共存"（RVC/Thumb 类）。
- 指令复用有两套并行机制：`[[families]]`（族 + 变体平行数组）与 `[[aliases]]`（别名清单）。
- 生成模块名 = 文件 stem；生成代码默认 `crate::` 路径。

## 2. v14：增量扩展与三类结构债

v14 是在 v12 上的增量扩展，沉淀出三类债（v15 的动因）：

| 债 | 形态 | 实测 |
| --- | --- | --- |
| `[[forms]]` 组合爆炸 | 把多个事实编进 form 名（`MRR_0F_NOOS_MEM` 与 `MRR_MEM_0F_NOOS` 只差 `rex_w`） | x86 47 个 form，其中 17 个只被一条指令使用 |
| `when` 表达力不足 | 缺 `in`/`not` 组合与行表展开，只能复制粘贴规则 | x86 261 条 lowering 里 66 条 op 相同、`insts` 逐字节相同，只差 `when` |
| x86 缺省写进生成器 | 生成器里 16 处按指令名兜底（缺声明时默认套 `MOV_RM8_R64` 这类 x86 指令名） | 别的 ISA 会静默拿到 x86 形状 |

v14 的具体写法（**已删除**）：

```toml
# ① opsize 用字节单位字符串（与 [conventions] 的位单位矛盾）
opsize = "r8"                  # 意思是 64 位！现在是 opsize = 64 或 "dst"

# ② modrm 用六个魔法串表达位置
modrm = "rr"                   # 同一个 "rr" 在 ADD_RM_R 里 reg=源、在 MOV_R_RM 里 reg=目的

# ③ asm 里内联声明操作数（槽 + 角色）
asm = "add {0:[gprx:inout]}, {1:[gprx]}"
```

## 3. v15（S1–S6）：六步破坏性简化

每步门禁全绿、逐字节等价（细节见当时的提交与 `docs/archive/forge-dsl-v18-plan.md`）：

| 阶段 | 内容 |
| --- | --- |
| S1 | 删死键（`forms.opcode_bytes`/`operand_slots.field_width`/`values`/`forms.operand_slots`/`[conventions.rex]`/`[conventions.opsize_prefix]`/死 form）；`Spanned` 行号诊断；`include_bytes!` 依赖跟踪（删 `touch` workaround）；补齐 lowering 校验；字符串键枚举化 |
| S2 | lowering `vary` 行表 + `in` 集合谓词 + 特异性排序（去声明序依赖）+ 重复/死规则编译期报错 |
| S3 | 命名操作数（`ops` 声明、`asm` 只引用）；`modrm` 六魔法串 → 显式映射；`[[forms]]` 降为可选预设（指令逐键覆盖）；`opsize` 单位统一为位 |
| S4 | `[abi]` 13 个 `*_inst` 名指针 + `tags` → 指令上的 `roles` 枚举；删 16 处 x86 硬编码默认 |
| S5 | `[abi.frame]` 4 个 riscv 旋钮 → `layout` 枚举 + 运行期推导；`{callee_saved_bytes}` 占位符替掉 x86 尾声魔法数 56 |
| S6 | `[[pattern]]` 树型多 op 匹配（新增能力）；删 `ext/pattern_isel.rs` 死模块 |

## 4. v16：内存操作数的文本模板

- 新增 `[conventions.mem] template`（内存操作数在 `disassemble`/`assemble` 里的文本形状，
  缺省 x86 的 `[{base}+{index}*{scale}+{disp}]`）。v16 落地时**没有写进文档**，v18 起在
  现行规范里有正式一节。

## 5. v17：命名操作数成为唯一形态

- 指令用 `ops = ["名字:槽[:角色]", …]` 声明操作数（**数组序 = 编码序**），`asm` 只用
  `{名字}` 引用——声明与打印彻底分离。
- `asm` 变成**必填**；不再"拆分助记符"（助记符只是模板首段字面量，不再是独立分派键）。
- v14 的内联声明 `{i:[槽:角色]}` 删除，无兼容层。

## 6. v18 的删除与改名总表

| 旧 | 新 | 说明 |
| --- | --- | --- |
| `[meta].default_inst_width` / `variable_length` / `max_inst_len` | `[encoding].kind` / `bits` / `widths` / `max_len` | 宽度三态（`fixed` / `mixed` / `prefix_scan`） |
| `[meta].default_opsize` | `[encoding].default_opsize` | 归位 |
| `[[families]]` | `[[templates]]` 的 `body` + `rows` | 变体 = 一行，可覆盖任意指令字段（含 `ops`/`form`/编码键） |
| `[[aliases]]` | 指令属性 `ref`（多条共用同一 `ref` = 多态） | 与指令名同池 + 冲突校验 |
| `Instruction.global_reloc` | `Instruction.reloc = "<[[reloc]].name>"` | 重定位数据化（`[[reloc]]` 表给出语义与绑定槽） |
| `[conventions.cond]`（x86 名 → 码） | 同名节，键 = 条件名（`code` + 可选 `ir`） | 删除 Rust 侧 x86 表与 `cond_default()` |
| `Instruction.when`（三份谱 0 处使用） | 删除 | 死键 |
| `[conventions.mem]`（v16，未文档化） | 保留并文档化 | 已是数据 |
| 生成器内 x86 缺省（cond / prefix_scan / `[abi]` / `value_fpr_width` / `vector_tiers`） | 必填或 fail-closed 报错 | 不再静默给别家形状 |
| `[abi]` 的 13 个 `*_inst` 名指针 + `tags` | 指令上的 `roles = ["gpr_mov", …]` | 生成器按角色查指令，不按指令名特判 |
| 把位宽编进角色名（v18 S9 前的 `fpr_mov_f32`/`fpr_mov_f64`、`wide_vec_store_32`/`_64`、`wide_vec_load_32`/`_64`） | `roles = [{ role = "fpr_mov", bits = 32 }]` 这类**带宽度声明**（角色名去掉宽度后缀） | 宽度是数据、不是名字的一部分；同角色多条按 (角色, 位宽) 唯一，其它位宽的 ISA 无需改 DSL 源码即可接入 |

另有 v18 新增的机制（`[[reloc]]` / `[[pseudo]]` / `[[derive]]` / `include` + `[[override]]` /
`isa_from_file!` 的 `name`/`parts` / 生成期自测）——这些**不是**历史，见现行规范。

**方案里提过但落地时没采纳的改名**（写了就是写错，以现行规范为准）：方案 §5.1 曾计划加顶层
`schema = 18` 必填键、把 `[meta].name` 提到顶层、并把生成器目录 `src/v12/` 改名 `src/schema/`。
落地时都没有做——`[meta].name` / `[meta].version` / `[meta].mode` 保留原样（`version` 本来就是
自由的 ISA 版本串），生成器仍在 `crates/frontend/forge-isa-dsl/src/v12/`。语法版本靠仓库内的
规范文档与 `docs/CHANGELOG` 追踪，不靠 TOML 里的版本键。

## 7. 迁移时最容易踩的三点

1. **`opsize` 单位是位**：`"r8"`（字节单位）现在报错并提示改 `64`（或 `"dst"`/`"max"`）。
2. **定宽 ISA 的字段名 = `ops` 里声明的名字**（v18 S7d 修正）：旧写法从
   `[forms].operand_fields` 的位域名取字段名（`Inst::Iadd { rd, rs1 }`），现在是不声明就
   没有的那个名字（`Inst::Iadd { dst, src }`）。
3. **多文件谱用 `include` + `[[override]]`**：没有"后出现的赢"这种隐式覆盖——同名标量
   冲突直接报错，要覆盖必须显式写 `[[override]]`。
