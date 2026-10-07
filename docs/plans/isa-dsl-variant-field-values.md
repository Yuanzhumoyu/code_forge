# ISA-DSL：**按变体给字段值**（`variant` 字段值）

> [progress] 2026-10-07 设计定稿（尚未实现）。动机与证据见
> `crates/tools/forge-tests/asm/README.md` 的「`rev8` 的 RV32/RV64 两编码」一节。

## 1. 要解决的问题

同一条指令在不同**变体参数**下需要**不同的字段值**，而声明只能写一个值 ✗。真实用例：

```text
rv32zbb-only-valid.s:13  rev8 t0, t1
  我们（RV64 的 imm12 = 0x6b8）: 93 52 83 6b
  上游（RV32 的 imm12 = 0x698）: 93 52 83 69
```

差异 = `imm12` 的 bit5（word bit 25 = `funct7[0]`）⇒ **RV32 与 RV64 是两条不同编码** ✓。

**为什么不能靠"加一条 RV32-only 指令"** ✗：`variants.rs::only_supplied_params_gate` 断言
"xlen=64 是原生视角，一条都不该丢" ✓，而 RV32-only 的声明在 `xlen=64` 视角里必然被丢 ✗。
（放宽该不变量是另一条路，本轮**不选** ✓。）

## 2. 语法（最小、人体工学）

字段值今天是一个整数 ✓；新增**变体分派表**形态 ✓（同一个键位，两种写法都能解析）：

```toml
[[instructions]]
name = "REV8"
form = "I"
opcode = 0x13
# 变体分派：按参数名查表；未命中的参数取值 / 缺省运行（不传参数）取 `default`
fields = { funct3 = 5, imm12 = { xlen = { 32 = 0x698 }, default = 0x6b8 } }
ops = ["dst:gpr:out", "src:gpr"]
asm = "rev8 {dst}, {src}"
```

要点：

- **一个键位两种形态** ✓：`imm12 = 0x6b8`（常量，今天的样子）与
  `imm12 = { <参数名> = { <值> = <字段值>, … }, default = <字段值> }`（分派）；
- `default` **必填** ✓（原生/未传参数时必须能解析 ✓）——缺它是加载期错误 ✓；
- 参数名必须在 `[meta].variants` 里声明过 ✓，表里的键必须是该参数**声明域**中的值 ✓（与
  `only_variants` 同口径 ✓）；
- **不做**"按参数拼接字符串"这类隐式替换 ✗（`{xlen}` 替换已有 ✓，但它替换的是**名字**，不能表达
  `0x6b8`→`0x698` 这种**值**变化 ✗）。

## 3. 实现触点（四方同步 + 一pass）

| # | 位置 | 改动 |
| --- | --- | --- |
| 1 | `crates/frontend/forge-isa-dsl/src/dsl/model.rs` | 字段值的类型由 `i64` 扩成枚举（常量 / 分派表）：`enum FieldVal { Const(i64), Variant { param: String, by: BTreeMap<i64, i64>, default: i64 } }`；`EncKeys.fields`（`BTreeMap<String, FieldVal>`）随之改型 |
| 2 | `.../schema.rs` + `isa-dsl.schema.json` | schema 里该键允许"整数"与"表"两种形状（`oneOf`）；用 `cargo run -p forge-isa -- schema --out isa-dsl.schema.json` 重生成 ✓ |
| 3 | **投影 pass**（`.../report.rs` 的 `{参数名}` 替换处） | **唯一解析点** ✓：按当前 params 把 `FieldVal::Variant` 解析成 `Const`；不传参数 ⇒ `default` ✓。解析后**下游只见到常量** ✓ ⇒ 生成器/解码/渲染**零改动** ✓ |
| 4 | `.../dsl/validate.rs` | 新规则：`default` 必填 ✓；参数名须在 `[meta].variants` 里 ✓；`by` 的键须在声明域内 ✓；表不得为空 ✓（与 `only_variants` 用同一套查表逻辑 ✓，别写第二份 ✓） |
| 5 | `docs/reference/isa-dsl.md` | 键总览 + 一节说明（含 `rev8` 例子 ✓） |

**关键判据**：**只有投影 pass 知道变体** ✓ ⇒ 生成物（encode/decode/asm/自测）全部在"已解析"的模型上跑 ✓，
不会出现"同一模块里两套语义" ✗。

## 4. 测试与验收

1. **单测**（`forge-isa-dsl`）：同一份谱在 `--params xlen=32` 与不传参数下，`insts` 报出的该指令字段值
   分别为 `0x698` / `0x6b8` ✓；缺 `default`、参数名未声明、域外键 ⇒ **加载期报错** ✓（三条负例）；
2. **投影快照**：`variants.rs` 的 RV32 投影计数与"原生视角不丢指令"两条断言**都保持不变** ✓
   （本方案**不新增/不删除任何声明** ✓，只改字段值 ✓ ⇒ 两个计数都不动 ✓✓ —— 这正是选它的理由）；
3. **端到端**：落 `rev8` 后，全集 `known` **148 → 146** ✓、留集 **135 → 137** ✓、四档 `verify=0` ✓、
   `ws=0` ✓、钉死值经 `FORGE_PIN_WRITE=1` 刷新 ✓（riscv64 计数不变 ✓ ⇒ 只可能动 `ambiguity.*` ✗ hmm：
   本方案不新增指令 ⇒ **歧义名单也不动** ✓）；
4. 三方守卫：`cargo test -p forge-isa-dsl --test schema_guard` ✓、`lint_shipped` ✓、四档 asm ✓。

## 5. 被否掉的替代方案（记录理由，别再试）

- **加一条 RV32-only 指令 + `only_variants`** ✗：被"原生视角不丢指令"的不变量拦下（且实测有效：
  `known` 148→146 ✓）——若将来放宽该不变量，这条最省 ✓，届时可回头采用；
- **靠 `{xlen}` 字符串替换** ✗：它替换的是**名字**不是**值**，表达不了 `0x6b8`→`0x698` ✓；
- **在生成器里分支** ✗：会让同一模块出现两套语义、且解码/渲染要各写一份 ✗（本方案让下游只见常量 ✓）。
