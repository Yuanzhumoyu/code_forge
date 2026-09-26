//! 集成测试共用的**合成目标**（`AbiTarget`）与语料/黄金文件工具。
//!
//! 为什么不直接接真实的 `TargetRegInfo`：A1 的目标是**把数据面钉住**，
//! 而 A1 还没接宿主（A3 才做）。这里按三份发行谱的 `[reg.*]` / `[abi]` 事实
//! 手工复刻一份最小视图——**名字与固定性都对着谱抄**（RAX..R15 / X0..X31 / F0..F31），
//! 于是绑定文件（`conventions/*.toml`）里的名字解析结果与真实宿主一致；
//! 一旦宿主接上（A3），这些测试可以直接换成真实适配器对照。

#![allow(dead_code)]

use std::path::{Path, PathBuf};

use forge_abi::{AbiTarget, Capability, Elem, Signature, TyView};

/// 一个物理寄存器条目。
struct Reg {
    name: &'static str,
    class: &'static str,
    width: u8,
    pinned: bool,
}

/// 合成目标：寄存器表 + 可分配集 + 能力集。
pub struct TestTarget {
    isa: &'static str,
    regs: Vec<Reg>,
    /// 额外名字 → 索引（同号不同宽度的别名，如 EAX → RAX 槽）。
    aliases: Vec<(&'static str, u32)>,
    allocatable: Vec<u32>,
    scratch: Vec<u32>,
    link: Option<u32>,
    caps: Vec<(Capability, u16)>,
}

impl std::fmt::Debug for TestTarget {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "TestTarget({})", self.isa)
    }
}

fn push(regs: &mut Vec<Reg>, names: &[&'static str], class: &'static str, width: u8) {
    for n in names {
        regs.push(Reg {
            name: n,
            class,
            width,
            pinned: false,
        });
    }
}

/// x86_64_v12：GPR(8) RAX..R15 → 0..15，FPR(16) XMM0-15 → 16..31，VEC(64) ZMM0-31 → 32..63。
///
/// 固定性：RSP(4)/RBP(5)（谱里 `[abi.frame] sp=RSP fp=RBP`）；`[abi] scratch = [R10,R11]`。
pub fn x86_64_v12() -> TestTarget {
    let mut regs: Vec<Reg> = Vec::new();
    push(
        &mut regs,
        &[
            "RAX", "RCX", "RDX", "RBX", "RSP", "RBP", "RSI", "RDI", "R8", "R9", "R10", "R11",
            "R12", "R13", "R14", "R15",
        ],
        "GPR(8)",
        8,
    );
    for i in 0..16 {
        regs.push(Reg {
            name: xmm_name(i),
            class: "FPR(16)",
            width: 16,
            pinned: false,
        });
    }
    // ZMM 与 XMM 是同一批物理寄存器的宽视图（谱里是两组）；测试只需要名字可解析，
    // 这里让 ZMM 落在独立号段，避免与 XMM 撞号。
    for i in 0..32 {
        regs.push(Reg {
            name: zmm_name(i),
            class: "VEC(64)",
            width: 64,
            pinned: false,
        });
    }
    regs[4].pinned = true; // RSP
    regs[5].pinned = true; // RBP
    TestTarget {
        isa: "x86_64_v12",
        allocatable: (0..16)
            .filter(|i| *i != 4 && *i != 5)
            .chain(16..32)
            .collect(),
        scratch: vec![10, 11],
        link: None,
        caps: vec![
            (Capability::GprMov, 64),
            (Capability::FprMov, 64),
            (Capability::VecMov, 128),
            (Capability::SpAdjust, 64),
            (Capability::StackArgLoad, 64),
            (Capability::StackArgStore, 64),
            (Capability::FrameAddr, 64),
            (Capability::WideVecMove, 512),
            (Capability::Call, 64),
            (Capability::CallIndirect, 64),
            (Capability::Ret, 64),
        ],
        aliases: x86_aliases(),
        regs,
    }
}

fn xmm_name(i: u32) -> &'static str {
    const N: [&str; 16] = [
        "XMM0", "XMM1", "XMM2", "XMM3", "XMM4", "XMM5", "XMM6", "XMM7", "XMM8", "XMM9", "XMM10",
        "XMM11", "XMM12", "XMM13", "XMM14", "XMM15",
    ];
    N[i as usize]
}

fn zmm_name(i: u32) -> &'static str {
    const N: [&str; 32] = [
        "ZMM0", "ZMM1", "ZMM2", "ZMM3", "ZMM4", "ZMM5", "ZMM6", "ZMM7", "ZMM8", "ZMM9", "ZMM10",
        "ZMM11", "ZMM12", "ZMM13", "ZMM14", "ZMM15", "ZMM16", "ZMM17", "ZMM18", "ZMM19", "ZMM20",
        "ZMM21", "ZMM22", "ZMM23", "ZMM24", "ZMM25", "ZMM26", "ZMM27", "ZMM28", "ZMM29", "ZMM30",
        "ZMM31",
    ];
    N[i as usize]
}

fn x86_aliases() -> Vec<(&'static str, u32)> {
    const W32: [&str; 16] = [
        "EAX", "ECX", "EDX", "EBX", "ESP", "EBP", "ESI", "EDI", "R8D", "R9D", "R10D", "R11D",
        "R12D", "R13D", "R14D", "R15D",
    ];
    const W8: [&str; 16] = [
        "AL", "CL", "DL", "BL", "SPL", "BPL", "SIL", "DIL", "R8B", "R9B", "R10B", "R11B", "R12B",
        "R13B", "R14B", "R15B",
    ];
    W32.iter()
        .chain(W8.iter())
        .enumerate()
        .map(|(i, n)| (*n, (i % 16) as u32))
        .collect()
}

/// riscv64_v12：GPR(8) X0..X31 → 0..31，FPR(8) F0..F31 → 32..63。
///
/// 固定性对着谱：`[abi] reserved = [X0,X1,X3,X4]`、`sp = X2`、`fp = X8`
/// （X1 同时是链接寄存器）。
pub fn riscv64_v12() -> TestTarget {
    let mut regs: Vec<Reg> = Vec::new();
    for i in 0..32 {
        regs.push(Reg {
            name: x_name(i),
            class: "GPR(8)",
            width: 8,
            pinned: false,
        });
    }
    for i in 0..32 {
        regs.push(Reg {
            name: f_name(i),
            class: "FPR(8)",
            width: 8,
            pinned: false,
        });
    }
    for i in [0u32, 1, 2, 3, 4, 8] {
        regs[i as usize].pinned = true;
    }
    TestTarget {
        isa: "riscv64_v12",
        allocatable: (0..32)
            .filter(|i| ![0, 1, 2, 3, 4, 8].contains(i))
            .chain(32..64)
            .collect(),
        scratch: vec![5, 6],
        link: Some(1),
        // 谱里只声明了 gpr_mov / frame_alloc / frame_free / call / jump / ret / branch。
        caps: vec![
            (Capability::GprMov, 64),
            (Capability::SpAdjust, 64),
            (Capability::Call, 64),
            (Capability::Ret, 64),
        ],
        aliases: Vec::new(),
        regs,
    }
}

/// arm64_v12：GPR(8) X0..X30 + SP → 0..31（W 别名指向同号）。
///
/// 固定性对着谱：`[abi] reserved = [X18,X30]`、`sp = SP`、`fp = X29`（fp-inside 布局）。
/// **没有 FPR/VEC 组**——这就是 arm64 的浮点缺口，绑定文件也据此不给 `float` 池。
pub fn arm64_v12() -> TestTarget {
    let mut regs: Vec<Reg> = Vec::new();
    for i in 0..31 {
        regs.push(Reg {
            name: x_name(i),
            class: "GPR(8)",
            width: 8,
            pinned: false,
        });
    }
    regs.push(Reg {
        name: "SP",
        class: "GPR(8)",
        width: 8,
        pinned: false,
    });
    for i in [18u32, 29, 30, 31] {
        regs[i as usize].pinned = true;
    }
    let aliases = (0..31)
        .map(|i| (w_name(i), i))
        .chain(std::iter::once(("WSP", 31)))
        .collect();
    TestTarget {
        isa: "arm64_v12",
        allocatable: (0..31).filter(|i| ![18, 29, 30].contains(i)).collect(),
        scratch: vec![16, 17],
        link: Some(30),
        // 谱里只声明了 gpr_mov / frame_alloc / frame_free / jump / epilogue_jump / ret / branch。
        caps: vec![
            (Capability::GprMov, 64),
            (Capability::SpAdjust, 64),
            (Capability::Ret, 64),
        ],
        aliases,
        regs,
    }
}

/// 按 ISA 名取目标（`forge-isa abi` 的语料用同一批名字）。
pub fn target_for(isa: &str) -> Option<TestTarget> {
    match isa {
        "x86_64_v12" => Some(x86_64_v12()),
        "riscv64_v12" => Some(riscv64_v12()),
        // v20 A5：真实 arm64 谱已有 `[reg.fpr8]`（V0..V31），合成目标跟着带上——
        // 绑定里的 `float`/`ret_float` 池要能在它上面解析。
        "arm64_v12" => Some(arm64_v12_with_fpr()),
        _ => None,
    }
}

/// **合成** arm64 + FPR 组（V0-V31 → 32..63）：用来验证"浮点池补齐之后"的
/// AAPCS64 路径（HFA 1/2/4 成员的槽数、4 成员的 `RegGroup`、浮点返回）。
///
/// 真实的 `isa/arm64_v12.toml` **还没有** FPR 组（A5 才加），所以这份目标只属于测试：
/// 它证明"缺的只是谱里的寄存器组与指令角色，模型与引擎早已能表达"。
pub fn arm64_v12_with_fpr() -> TestTarget {
    let mut t = arm64_v12();
    for i in 0..32 {
        t.regs.push(Reg {
            name: v_name(i),
            class: "FPR(16)",
            width: 16,
            pinned: false,
        });
    }
    t.allocatable.extend(32..64);
    t.caps.push((Capability::FprMov, 64));
    t.caps.push((Capability::VecMov, 128));
    t
}

fn v_name(i: u32) -> &'static str {
    const N: [&str; 32] = [
        "V0", "V1", "V2", "V3", "V4", "V5", "V6", "V7", "V8", "V9", "V10", "V11", "V12", "V13",
        "V14", "V15", "V16", "V17", "V18", "V19", "V20", "V21", "V22", "V23", "V24", "V25", "V26",
        "V27", "V28", "V29", "V30", "V31",
    ];
    N[i as usize]
}

/// **合成** AAPCS64 绑定（给 `arm64_v12_with_fpr()` 用）：`isa` 名与真实绑定相同，
/// 由测试注册到自己的注册表里（内置表里没有 `float`/`ret_float`）。
pub const AAPCS64_FULL_BINDING: &str = r#"
isa = "arm64_v12"
conv = "aapcs64"
[pools]
int = ["X0", "X1", "X2", "X3", "X4", "X5", "X6", "X7"]
sret = ["X8"]
float = ["V0", "V1", "V2", "V3", "V4", "V5", "V6", "V7"]
cs_gpr = ["X19", "X20", "X21", "X22", "X23", "X24", "X25", "X26", "X27", "X28"]
ret_int = ["X0", "X1"]
ret_float = ["V0", "V1", "V2", "V3"]
"#;

fn x_name(i: u32) -> &'static str {
    const N: [&str; 32] = [
        "X0", "X1", "X2", "X3", "X4", "X5", "X6", "X7", "X8", "X9", "X10", "X11", "X12", "X13",
        "X14", "X15", "X16", "X17", "X18", "X19", "X20", "X21", "X22", "X23", "X24", "X25", "X26",
        "X27", "X28", "X29", "X30", "X31",
    ];
    N[i as usize]
}

fn w_name(i: u32) -> &'static str {
    const N: [&str; 31] = [
        "W0", "W1", "W2", "W3", "W4", "W5", "W6", "W7", "W8", "W9", "W10", "W11", "W12", "W13",
        "W14", "W15", "W16", "W17", "W18", "W19", "W20", "W21", "W22", "W23", "W24", "W25", "W26",
        "W27", "W28", "W29", "W30",
    ];
    N[i as usize]
}

fn f_name(i: u32) -> &'static str {
    const N: [&str; 32] = [
        "F0", "F1", "F2", "F3", "F4", "F5", "F6", "F7", "F8", "F9", "F10", "F11", "F12", "F13",
        "F14", "F15", "F16", "F17", "F18", "F19", "F20", "F21", "F22", "F23", "F24", "F25", "F26",
        "F27", "F28", "F29", "F30", "F31",
    ];
    N[i as usize]
}

impl AbiTarget for TestTarget {
    fn isa_name(&self) -> &str {
        self.isa
    }

    fn reg_count(&self) -> u32 {
        self.regs.len() as u32
    }

    fn reg_name(&self, index: u32) -> Option<String> {
        self.regs.get(index as usize).map(|r| r.name.to_string())
    }

    fn reg_class_name(&self, index: u32) -> String {
        self.regs
            .get(index as usize)
            .map(|r| r.class.to_string())
            .unwrap_or_else(|| "?".into())
    }

    fn reg_index(&self, name: &str) -> Option<u32> {
        if let Some(i) = self.regs.iter().position(|r| r.name == name) {
            return Some(i as u32);
        }
        self.aliases
            .iter()
            .find(|(n, _)| *n == name)
            .map(|(_, i)| *i)
    }

    fn reg_width(&self, index: u32) -> u8 {
        self.regs.get(index as usize).map(|r| r.width).unwrap_or(0)
    }

    fn pinned(&self, index: u32) -> bool {
        self.regs
            .get(index as usize)
            .map(|r| r.pinned)
            .unwrap_or(true)
    }

    fn allocatable(&self) -> Vec<u32> {
        self.allocatable.clone()
    }

    fn spill_scratch(&self) -> Vec<u32> {
        self.scratch.clone()
    }

    fn link_reg(&self) -> Option<u32> {
        self.link
    }

    fn cap(&self, cap: Capability) -> Option<u16> {
        self.caps
            .iter()
            .find(|(c, _)| *c == cap)
            .map(|(_, bits)| *bits)
    }
}

// ─────────────────────────── 类型语料 ───────────────────────────

pub fn i32_() -> TyView {
    TyView::int(4, 4)
}

pub fn i64_() -> TyView {
    TyView::int(8, 8)
}

pub fn f32_() -> TyView {
    TyView::float(4)
}

pub fn f64_() -> TyView {
    TyView::float(8)
}

pub fn ptr_() -> TyView {
    TyView::ptr(8)
}

/// `<4 x f32>`（16 字节）。
pub fn v128() -> TyView {
    TyView::vector(Elem::Float, 4, 4)
}

/// `<8 x f32>`（32 字节，>16B 走引用/byval 路径）。
pub fn v256() -> TyView {
    TyView::vector(Elem::Float, 8, 4)
}

/// `struct { i64; i64 }`（16 字节、**整数**聚合——不是 HFA）。
pub fn agg_ii() -> TyView {
    TyView::agg(8, vec![i64_(), i64_()])
}

/// `struct { f32; f32 }`（8 字节，2 成员 HFA）。
pub fn hfa2() -> TyView {
    TyView::agg(4, vec![f32_(), f32_()])
}

/// `struct { f32 }`（4 字节，1 成员 HFA——写死 `slots = 4` 会在这里多吃寄存器）。
pub fn hfa1() -> TyView {
    TyView::agg(4, vec![f32_()])
}

/// `struct { f32; f32; f32; f32 }`（16 字节，4 成员 HFA → 需要 `RegGroup`）。
pub fn hfa4() -> TyView {
    TyView::agg(4, vec![f32_(), f32_(), f32_(), f32_()])
}

/// `struct { f32; f64 }`（非 HFA：成员不同宽）。
pub fn agg_mixed_float() -> TyView {
    TyView::agg(8, vec![f32_(), f64_()])
}

/// `struct { i8 × 24 }`（24 字节，>16B 走引用）。
pub fn agg24() -> TyView {
    TyView::agg(1, vec![TyView::int(1, 1); 24])
}

/// 一个语料条目：名字 + 签名。
pub struct Case {
    pub name: &'static str,
    pub sig: Signature,
}

fn p(name: &str, ty: TyView) -> (String, TyView) {
    (name.to_string(), ty)
}

fn add(
    cases: &mut Vec<Case>,
    name: &'static str,
    params: Vec<(String, TyView)>,
    ret: Option<TyView>,
) {
    cases.push(Case {
        name,
        sig: Signature::new(params, ret),
    });
}

/// 固定语料：每个分类分支至少一条，四份内置约定都跑它。
///
/// 命名规则：`p` 前缀 = 参数、`ret` 前缀 = 返回值；`agg_ii` 这类名字直接对应上面的构造函数。
pub fn corpus() -> Vec<Case> {
    let mut cases: Vec<Case> = Vec::new();
    add(&mut cases, "void0", vec![], None);
    add(&mut cases, "i64", vec![p("a", i64_())], Some(i64_()));
    add(
        &mut cases,
        "i32_i32",
        vec![p("a", i32_()), p("b", i32_())],
        Some(i32_()),
    );
    add(
        &mut cases,
        "i64_x6",
        vec![
            p("a", i64_()),
            p("b", i64_()),
            p("c", i64_()),
            p("d", i64_()),
            p("e", i64_()),
            p("f", i64_()),
        ],
        Some(i64_()),
    );
    add(
        &mut cases,
        "i64_x8",
        vec![
            p("a", i64_()),
            p("b", i64_()),
            p("c", i64_()),
            p("d", i64_()),
            p("e", i64_()),
            p("f", i64_()),
            p("g", i64_()),
            p("h", i64_()),
        ],
        None,
    );
    add(&mut cases, "f64", vec![p("x", f64_())], Some(f64_()));
    add(
        &mut cases,
        "f64_x4",
        vec![
            p("a", f64_()),
            p("b", f64_()),
            p("c", f64_()),
            p("d", f64_()),
        ],
        Some(f64_()),
    );
    add(&mut cases, "ptr", vec![p("p", ptr_())], Some(ptr_()));
    add(&mut cases, "vec16", vec![p("v", v128())], Some(v128()));
    add(&mut cases, "vec32", vec![p("v", v256())], Some(v256()));
    add(&mut cases, "agg_ii", vec![p("s", agg_ii())], Some(agg_ii()));
    add(
        &mut cases,
        "agg_ii_f64",
        vec![p("s", agg_ii()), p("x", f64_())],
        None,
    );
    add(&mut cases, "agg24", vec![p("s", agg24())], Some(agg24()));
    add(&mut cases, "hfa1", vec![p("s", hfa1())], Some(hfa1()));
    add(&mut cases, "hfa2", vec![p("s", hfa2())], Some(hfa2()));
    add(&mut cases, "hfa4", vec![p("s", hfa4())], Some(hfa4()));
    add(
        &mut cases,
        "agg_mixed_float",
        vec![p("s", agg_mixed_float())],
        None,
    );
    add(
        &mut cases,
        "mixed_wide_ret",
        vec![p("a", i64_()), p("x", f64_())],
        Some(agg24()),
    );
    add(
        &mut cases,
        "i64_x9",
        (0..9).map(|i| p(&format!("a{i}"), i64_())).collect(),
        Some(i64_()),
    );
    cases
}

/// 变参语料：`fixed` 是**命名**参数个数，其余为未命名实参。
pub fn variadic_cases() -> Vec<Case> {
    vec![
        Case {
            name: "va2p1",
            sig: Signature::new(
                vec![p("fmt", ptr_()), p("n", i32_()), p("x", f64_())],
                Some(i32_()),
            )
            .variadic(2),
        },
        Case {
            name: "va0p3",
            sig: Signature::new(
                vec![p("a", i64_()), p("b", f64_()), p("c", i64_())],
                Some(i64_()),
            )
            .variadic(0),
        },
    ]
}

// ─────────────────────────── 黄金文件 ───────────────────────────

fn golden_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/golden")
}

/// 与签入的黄金文本比对；`FORGE_ABI_BLESS=1` 时改写（**只在人看过 diff 之后用**）。
///
/// **必须先归一化换行**：Windows 上 git 可能按 CRLF 检出签入的 LF 文件（CI 的
/// `Test (Windows)` 就这么红过一次）。`str::lines()` 会**吃掉** `\r`，所以只比
/// `lines()` 看不出这个差异，而直接比字符串又会在换行符上红——两者叠加出来的
/// 现象是"差异点报在文件末尾 + 期望 <缺行>"，极难排查。这里统一折成 `\n` 再比。
pub fn check_golden(rel: &str, actual: &str) {
    let path = golden_root().join(rel);
    if std::env::var_os("FORGE_ABI_BLESS").is_some() {
        if let Some(dir) = path.parent() {
            std::fs::create_dir_all(dir).expect("建目录");
        }
        // 写出的是 LF（`actual` 本身），因此签入件恒为 LF。
        std::fs::write(&path, actual).expect("写黄金文件");
        eprintln!("[blessed] {}", path.display());
        return;
    }
    let expected = std::fs::read_to_string(&path).unwrap_or_else(|e| {
        panic!(
            "缺黄金文件 {}（{}）——确认输出无误后用 FORGE_ABI_BLESS=1 生成",
            path.display(),
            e
        )
    });
    let (expected_n, actual_n) = (normalize_newlines(&expected), normalize_newlines(actual));
    if expected_n != actual_n {
        let line = first_diff(&expected_n, &actual_n);
        panic!(
            "黄金文本不一致：{}（第 {line} 行起）\n期望: {}\n实际: {}\n\
             确认无误后用 FORGE_ABI_BLESS=1 重新生成",
            path.display(),
            expected_n.lines().nth(line - 1).unwrap_or("<缺行>"),
            actual_n.lines().nth(line - 1).unwrap_or("<缺行>"),
        );
    }
}

/// CRLF → LF（只做这一件事；不碰其它空白）。
fn normalize_newlines(s: &str) -> String {
    if s.contains('\r') {
        s.replace("\r\n", "\n").replace('\r', "\n")
    } else {
        s.to_string()
    }
}

fn first_diff(a: &str, b: &str) -> usize {
    a.lines()
        .zip(b.lines())
        .position(|(x, y)| x != y)
        .map(|i| i + 1)
        .unwrap_or_else(|| a.lines().count().min(b.lines().count()) + 1)
}
