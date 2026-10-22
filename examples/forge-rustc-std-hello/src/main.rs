//! `std` 探测：forge-rustc 后端 + LLVM 预编译的 std（v27 M3-②）。
//!
//! 跑法见本目录 README（离线口径见 `run-offline.ps1`）。
//!
//! 这个程序有两重身份：
//!
//! 1. **能力梯度**（①–⑦ + ⑨⑩）：`println!` / `String`+`format!` / `Vec` / `env::args` /
//!    `HashMap` / `std::fs` / `std::thread::spawn` / `std::net`（回环 TCP）/
//!    `std::process::Command`（子进程 + 管道）；
//! 2. **自校验折磨段**（⑧）：泛型 + trait 对象 + 迭代器链 + 排序闭包 + 浮点/整数运算 +
//!    枚举匹配 + Drop 顺序 + 字符串解析。**每项都与本进程内用同样 std 算出的期望值比对**，
//!    任一不符就打印 `MISMATCH` 并以非零码退出——比"看输出对不对"更严；
//! 3. **第三方 crate 语料**（⑪ 基础批 + ⑫ 进阶批）：`itoa`/`ryu`/`memchr`/`smallvec`/
//!    `once_cell`/`bitflags` 与 `crc32fast`/`blake3`/`hashbrown`/`base64`/`dashu-int`
//!    也是由 forge 后端**从源码编译**的（它们不在 wrapper 的系统 crate 名单里）——
//!    于是 unsafe 指针写、f64 位技巧、SIMD intrinsic 分派、泛型 + union、
//!    `#[target_feature]` 函数、swiss-table 位运算、大整数乘除都进了验证面；
//! 4. **正则引擎族**（⑬）：`regex`（+ `regex-automata`/`regex-syntax`/`aho-corasick`）
//!    是生态里泛型/状态机最重的代码之一，同样由 forge 从源码编译；
//! 5. **proc-macro 线**（⑭）：`serde` 的 `derive` 需要先编出 `serde_derive`
//!    （proc-macro **dylib**）再由编译器加载 —— 那个 dylib 也是 forge 编的；
//! 6. **crypto + CLI 线**（⑮）：`sha2`（`typenum` 泛型 + 位运算）与 `clap(derive)`
//!    （当前能拉的最大真实依赖图：`clap_derive` 巨量展开 + `windows-sys` FFI +
//!    `anstream`/`anstyle-wincon` 终端栈）。
//!
//! 期望值的来源：同一段代码在**同一次运行**里先算一遍（这些计算本身也走 forge 后端，
//! 所以它不是独立的 oracle）；真正的 oracle 是"这些值在 LLVM 编译时是多少"——
//! 见 README 里记录的对照结果。

use std::collections::{BTreeMap, HashSet};

/// ⑧-a 泛型 + trait bound：一阶求和（测试单态化 + 内联后的常量传播）。
fn sum_all<T: Copy + std::ops::Add<Output = T> + Default>(xs: &[T]) -> T {
    let mut acc = T::default();
    for &x in xs {
        acc = acc + x;
    }
    acc
}

/// ⑧-b 动态派发：`Box<dyn Fn(i64) -> i64>` 的虚调用。
fn apply_all(fns: &[Box<dyn Fn(i64) -> i64>], x: i64) -> i64 {
    let mut acc = 0;
    for f in fns {
        acc += f(x);
    }
    acc
}

/// ⑧-c 递归 + 记忆化外壳（测递归调用约定与非尾递归栈帧）。
fn fib(n: u64) -> u64 {
    if n < 2 { n } else { fib(n - 1) + fib(n - 2) }
}

/// ⑧-f 带数据的枚举 + 匹配（测 niche/判别值 + 字段偏移）。
#[derive(Debug, Clone, Copy, PartialEq)]
enum Shape {
    Dot,
    Line(f64),
    Rect { w: f64, h: f64 },
}

fn area(s: Shape) -> f64 {
    match s {
        Shape::Dot => 0.0,
        Shape::Line(l) => l,
        Shape::Rect { w, h } => w * h,
    }
}

/// ⑧-i Drop 顺序观测器（Drop 顺序 = 声明逆序）。
///
/// 用 `RefCell` 共享日志：三个观察者同时活着（借用检查器不允许三个 `&mut Vec`）。
struct Noisy<'a>(&'static str, &'a std::cell::RefCell<Vec<&'static str>>);

impl Drop for Noisy<'_> {
    fn drop(&mut self) {
        self.1.borrow_mut().push(self.0);
    }
}

fn main() {
    let mut failures: Vec<String> = Vec::new();
    let mut check = |name: &str, got: String, want: String| {
        if got != want {
            failures.push(format!("{name}: 得 {got} 期望 {want}"));
        }
    };

    // ── ① 最基础的 std 输出 ──
    println!("hello from std");

    // ── ② 字符串格式化与增长 ──
    let mut s = String::from("forge");
    s.push_str(" + std");
    let f = format!("[{s}] len={}", s.len());
    println!("{f}");
    check("string", f, "[forge + std] len=11".to_string());

    // ── ③ Vec 增长 + 求和 ──
    let mut v: Vec<u64> = Vec::new();
    for i in 1..=10u64 {
        v.push(i * i);
    }
    let sum: u64 = v.iter().sum();
    println!("vec_sum={sum}");
    check("vec_sum", sum.to_string(), "385".to_string());

    // ── ④ 进程参数个数（本程序无参 ⇒ 1）──
    let argc = std::env::args().count();
    println!("args={argc}");
    check("args", argc.to_string(), "1".to_string());

    // ── ⑤ HashMap（std 的随机种子 + 哈希）──
    let mut m = std::collections::HashMap::new();
    m.insert("a", 1u32);
    m.insert("b", 2u32);
    let hm: u32 = m.values().sum();
    println!("map_sum={hm}");
    check("map_sum", hm.to_string(), "3".to_string());

    // ── ⑥ 文件 IO：写→读→删 ──
    let path = std::env::temp_dir().join("forge-std-probe.txt");
    std::fs::write(&path, b"file-io-ok").expect("write");
    let back = std::fs::read(&path).expect("read");
    let _ = std::fs::remove_file(&path);
    println!("fs_len={}", back.len());
    check("fs_len", back.len().to_string(), "10".to_string());

    // ── ⑦ 线程：跨线程跑闭包 + join 取回值 ──
    let handle = std::thread::spawn(|| {
        std::thread::sleep(std::time::Duration::from_millis(1));
        let t: Vec<u32> = (0..5).collect();
        t.iter().sum::<u32>()
    });
    let joined = handle.join().expect("join");
    println!("thread_sum={joined}");
    check("thread_sum", joined.to_string(), "10".to_string());

    // ── ⑧ 自校验折磨段 ──

    // a) 泛型单态化：i32 / u64 / f64 三种实例
    check("generic_i32", sum_all(&[1i32, 2, 3, 4]).to_string(), "10".into());
    check(
        "generic_u64",
        sum_all(&[u64::MAX - 1, 1]).to_string(),
        u64::MAX.to_string(),
    );
    check("generic_f64", format!("{:.2}", sum_all(&[0.5f64, 0.25, 0.125])), "0.88".into());

    // b) trait 对象虚调用（3 个闭包，分别是闭包捕获/常量/函数项）
    let k = 7i64;
    let fns: Vec<Box<dyn Fn(i64) -> i64>> = vec![
        Box::new(|x| x + 1),
        Box::new(move |x| x * k),
        Box::new(|x| x - 3),
    ];
    // 11 + 10*7 + 7 = 88（原期望 78 是手算错——自校验段当场发现）
    check("dyn_dispatch", apply_all(&fns, 10).to_string(), "88".into());

    // c) 迭代器链：map/filter/fold/collect + 惰性求值顺序
    let chain: Vec<i64> = (0..20).filter(|x| x % 2 == 0).map(|x| x * 3).collect();
    check("iter_chain", format!("{chain:?}"), "[0, 6, 12, 18, 24, 30, 36, 42, 48, 54]".into());
    let folded = (1..=10u64).fold(0u64, |a, b| a ^ b);
    check("iter_fold_xor", folded.to_string(), "11".into());

    // d) 排序闭包（sort_by 走切片排序 + 比较器虚/单态调用）
    let mut words = vec!["pear", "fig", "apple", "kiwi", "banana"];
    words.sort_by(|a, b| a.len().cmp(&b.len()).then(a.cmp(b)));
    check("sort_by", format!("{words:?}"), r#"["fig", "kiwi", "pear", "apple", "banana"]"#.into());
    let mut nums = vec![5i32, 3, 9, 1, 3];
    nums.sort_unstable();
    check("sort_unstable", format!("{nums:?}"), "[1, 3, 3, 5, 9]".into());

    // e) 浮点：sqrt/powi/round/NaN 比较 + 格式化
    let x = 2.0f64;
    check("float_math", format!("{:.6}", x.sqrt()), "1.414214".into());
    check("float_powi", format!("{:.1}", 1.5f64.powi(3)), "3.4".into());
    let nan = f64::NAN;
    check(
        "float_nan",
        format!("{} {} {}", nan == nan, nan != nan, nan < 1.0),
        "false true false".into(),
    );

    // f) 枚举 + 匹配（含 f64 字段）
    let shapes = [Shape::Dot, Shape::Line(3.5), Shape::Rect { w: 2.0, h: 2.5 }];
    let total: f64 = shapes.iter().copied().map(area).sum();
    check("enum_area", format!("{total:.2}"), "8.50".into());

    // g) 整数位运算：rotates / 溢出包装 / 前导零
    // 0x3456_7812 = 878082066（原期望写成十六进制字面量，比的是十进制字符串）
    check(
        "bit_rot",
        0x1234_5678u32.rotate_left(8).to_string(),
        "878082066".into(),
    );
    check("bit_wrapping", 250u8.wrapping_add(10).to_string(), "4".into());
    check("bit_clz", 1u32.leading_zeros().to_string(), "31".into());

    // h) 字符串解析 + BTreeMap 有序遍历
    let parsed: i32 = "  -42 ".trim().parse().expect("parse");
    check("parse_int", parsed.to_string(), "-42".into());
    let mut bt: BTreeMap<&str, u8> = BTreeMap::new();
    bt.insert("z", 26);
    bt.insert("a", 1);
    bt.insert("m", 13);
    check(
        "btree",
        format!("{:?}", bt.keys().collect::<Vec<_>>()),
        r#"["a", "m", "z"]"#.into(),
    );

    // i) HashSet 去重 + Drop 顺序
    let uniq: HashSet<u32> = [3u32, 1, 4, 1, 5, 9, 2, 6, 5].into_iter().collect();
    check("hashset", uniq.len().to_string(), "7".into());
    let drop_log = std::cell::RefCell::new(Vec::new());
    {
        let _a = Noisy("a", &drop_log);
        let _b = Noisy("b", &drop_log);
        let _c = Noisy("c", &drop_log);
    }
    let drop_order = format!("{:?}", drop_log.borrow());
    check("drop_order", drop_order, r#"["c", "b", "a"]"#.into());

    // j) 递归（非尾递归栈帧 + 参数传递）
    check("fib", fib(20).to_string(), "6765".into());

    // ── ⑨ 网络：回环 TCP 一来一回（std 的 Winsock 初始化 + socket 句柄面）──
    {
        use std::io::{Read, Write};
        use std::net::{TcpListener, TcpStream};

        let listener = TcpListener::bind("127.0.0.1:0").expect("bind");
        let addr = listener.local_addr().expect("local_addr");
        let server = std::thread::spawn(move || {
            let (mut s, _) = listener.accept().expect("accept");
            let mut buf = [0u8; 4];
            s.read_exact(&mut buf).expect("server read");
            s.write_all(b"pong").expect("server write");
            buf
        });
        let mut client = TcpStream::connect(addr).expect("connect");
        client.write_all(b"ping").expect("client write");
        let mut rb = [0u8; 4];
        client.read_exact(&mut rb).expect("client read");
        let got = server.join().expect("server join");
        check(
            "net",
            format!(
                "{}{}",
                String::from_utf8_lossy(&got),
                String::from_utf8_lossy(&rb)
            ),
            "pingpong".into(),
        );
    }

    // ── ⑩ 子进程：创建 + 管道读取（std 的 CreateProcess/管道句柄面）──
    {
        let out = std::process::Command::new("cmd")
            .args(["/C", "echo cmd-ok"])
            .output()
            .expect("Command::output");
        check(
            "command",
            String::from_utf8_lossy(&out.stdout).trim().to_string(),
            "cmd-ok".into(),
        );
    }

    // ── ⑪ 第三方 crate（同样由 forge 编译，见 Cargo.toml 注释）──
    {
        // itoa：unsafe 指针写入 + 整数转十进制的循环
        let mut buf = itoa::Buffer::new();
        check("itoa", buf.format(1234567890i64).to_string(), "1234567890".into());

        // ryu：f64 的位技巧 + 最短表示
        let mut rbuf = ryu::Buffer::new();
        check("ryu_1_5", rbuf.format(1.5f64).to_string(), "1.5".into());
        check("ryu_pi", rbuf.format(3.14159265358979f64).to_string(), "3.14159265358979".into());

        // memchr：SIMD 分派（cfg 选实现）+ 前向/反向搜索
        let hay = b"the quick brown fox";
        check(
            "memchr",
            format!("{:?}", memchr::memchr(b'q', hay)),
            "Some(4)".into(),
        );
        check(
            "memrchr",
            format!("{:?}", memchr::memrchr(b'o', hay)),
            "Some(17)".into(),
        );

        // smallvec：内联缓冲溢出（union + MaybeUninit）
        let mut sv: smallvec::SmallVec<[u8; 4]> = smallvec::SmallVec::new();
        for i in 0u8..10 {
            sv.push(i);
        }
        check(
            "smallvec",
            format!("{} {}", sv.len(), sv.iter().sum::<u8>()),
            "10 45".into(),
        );

        // once_cell：原子态 + 惰性初始化
        static CELL: once_cell::sync::Lazy<u64> = once_cell::sync::Lazy::new(|| 6 * 7);
        check("once_cell", (*CELL).to_string(), "42".into());

        // bitflags：宏生成 + 位运算 trait
        bitflags::bitflags! {
            #[derive(Debug, Clone, Copy, PartialEq)]
            struct Perm: u32 {
                const READ = 0b001;
                const WRITE = 0b010;
                const EXEC = 0b100;
            }
        }
        let mut pm = Perm::READ | Perm::EXEC;
        pm.insert(Perm::WRITE);
        check(
            "bitflags",
            format!("{} {}", pm.bits(), pm.contains(Perm::WRITE | Perm::EXEC)),
            "7 true".into(),
        );
    }

    // ── ⑫ 进阶第三方语料：`#[target_feature]`/intrinsic、swiss-table、大整数 ──
    {
        // crc32fast：`#[target_feature]` 函数 + 切片表算法（CRC-32 标准校验值）
        check(
            "crc32fast",
            crc32fast::hash(b"123456789").to_string(),
            3421780262u32.to_string(),
        );

        // blake3：宽 lane 哈希（SIMD 分派 + 大量 unsafe 读）
        check(
            "blake3",
            blake3::hash(b"abc").to_hex().to_string(),
            "6437b3ac38465133ffb63b75273a8db548c558465d79db03fd359c6cd5bd9d85".into(),
        );

        // base64：查表编解码 + 标准测试向量
        {
            use base64::Engine as _;
            let enc = base64::engine::general_purpose::STANDARD.encode(b"hello");
            let dec = base64::engine::general_purpose::STANDARD
                .decode(&enc)
                .expect("decode");
            check(
                "base64",
                format!("{enc} {}", String::from_utf8_lossy(&dec)),
                "aGVsbG8= hello".into(),
            );
        }

        // hashbrown：swiss-table 的分组位掩码探测
        {
            let mut hb: hashbrown::HashMap<&str, u32> = hashbrown::HashMap::new();
            for (i, k) in ["a", "b", "c", "d"].iter().enumerate() {
                hb.insert(*k, i as u32);
            }
            check(
                "hashbrown",
                format!("{} {}", hb.len(), hb.values().sum::<u32>()),
                "4 6".into(),
            );
        }

        // dashu-int：大整数（128 次乘 2、10^30 取模 7）
        {
            let mut two128 = dashu_int::UBig::from(1u32);
            for _ in 0..128 {
                two128 *= dashu_int::UBig::from(2u32);
            }
            check(
                "dashu_pow2_128",
                two128.to_string(),
                "340282366920938463463374607431768211456".into(),
            );

            let mut ten30 = dashu_int::UBig::from(1u32);
            for _ in 0..30 {
                ten30 *= dashu_int::UBig::from(10u32);
            }
            let rem = ten30 % dashu_int::UBig::from(7u32);
            check("dashu_mod7", rem.to_string(), "1".into());
        }
    }

    // ── ⑬ 正则引擎族（泛型状态机 + 惰性 DFA，第三方最重的语料）──
    {
        use regex::{Regex, RegexSet};

        // 捕获组 + 贪婪量词
        let re = Regex::new(r"a(b+)c").expect("compile re");
        let caps = re.captures("xxabbcxx").expect("captures");
        check("regex_caps", caps[1].to_string(), "bb".into());

        // find_iter 计数（\d{3} 在 "a1 234 56 789" 里命中 2 次）
        let re3 = Regex::new(r"\d{3}").expect("compile re3");
        check(
            "regex_find_iter",
            re3.find_iter("a1 234 56 789").count().to_string(),
            "2".into(),
        );

        // replace_all + 反向引用（$2 $1 换位）
        let pair = Regex::new(r"(\w+)\s+(\w+)").expect("compile pair");
        check(
            "regex_replace",
            pair.replace_all("hello world", "$2 $1").to_string(),
            "world hello".into(),
        );

        // Unicode 类（\p{Greek}）——走 regex-syntax 的 unicode 表
        let greek = Regex::new(r"\p{Greek}+").expect("compile greek");
        check(
            "regex_unicode",
            greek.find("abcαβγdef").map(|m| m.as_str().to_string()).unwrap_or_default(),
            "αβγ".into(),
        );

        // RegexSet：一次扫描多个模式（aho-corasick 的匹配机）
        // "foo" 同时命中 `^[a-z]+$`(1) 与 `fo{2}`(2)——第一版期望写成 `[2]` 是我的手算错，
        // 自校验段当场把它揪出来了（后端是对的）。
        let set = RegexSet::new([r"^\d+$", r"^[a-z]+$", r"fo{2}"]).expect("compile set");
        let hits: Vec<usize> = set.matches("foo").into_iter().collect();
        check("regex_set", format!("{hits:?}"), "[1, 2]".into());

        // 惰性量词 + 多行标志
        let lazy = Regex::new(r"(?m)^a.*?c$").expect("compile lazy");
        check(
            "regex_multiline",
            lazy.find_iter("axc\naxxc\nb").count().to_string(),
            "2".into(),
        );
    }

    // ── ⑭ proc-macro 线：`serde` derive（`serde_derive` 本身也由 forge 编译成 dylib）──
    {
        use serde::{Deserialize, Serialize};

        #[derive(Serialize, Deserialize, Debug, PartialEq)]
        struct Point {
            x: i32,
            y: i32,
            label: String,
        }

        #[derive(Serialize, Deserialize, Debug, PartialEq)]
        enum Kind {
            Circle { r: f64 },
            Square(u32),
        }

        // 结构体 → JSON → 反序列化回原值（derive 生成的 impl 全在后端编出）
        let pt = Point {
            x: 1,
            y: -2,
            label: "origin".into(),
        };
        let js = serde_json::to_string(&pt).expect("ser");
        check(
            "serde_json",
            js.clone(),
            r#"{"x":1,"y":-2,"label":"origin"}"#.into(),
        );
        let back: Point = serde_json::from_str(&js).expect("de");
        check("serde_roundtrip", (back == pt).to_string(), "true".into());

        // 带字段的枚举（外部标记表示）
        let sq = Kind::Square(3);
        let js2 = serde_json::to_string(&sq).expect("ser enum");
        let back2: Kind = serde_json::from_str(&js2).expect("de enum");
        check(
            "serde_enum",
            format!("{js2} {}", back2 == sq),
            r#"{"Square":3} true"#.into(),
        );
    }

    // ── ⑮ crypto + CLI：`sha2`（typenum 泛型）与 `clap` derive（最大依赖图）──
    {
        // sha2：SHA-256 官方向量（"abc"）
        {
            use sha2::{Digest, Sha256};
            let d = Sha256::digest(b"abc");
            let mut hex = String::new();
            for b in d.iter() {
                hex.push_str(&format!("{b:02x}"));
            }
            check(
                "sha256",
                hex,
                "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad".into(),
            );
        }

        // clap derive：解析 + 报错分类
        {
            use clap::Parser as _;

            #[derive(Debug, clap::Parser)]
            #[command(name = "probe", version, about = "forge std probe cli")]
            struct Cli {
                #[arg(long, default_value_t = 3)]
                count: u32,
                #[arg(long)]
                name: Option<String>,
            }

            let cli = Cli::try_parse_from(["probe", "--count", "5", "--name", "forge"])
                .expect("clap parse");
            check(
                "clap_parse",
                format!("{} {:?}", cli.count, cli.name),
                "5 Some(\"forge\")".into(),
            );

            let err = Cli::try_parse_from(["probe", "--bogus"]).expect_err("应拒绝未知参数");
            check(
                "clap_err",
                format!("{:?}", err.kind()),
                "UnknownArgument".into(),
            );
        }
    }

    // ── ⑯ 计时段（**默认不输出**：只有 `FORGE_BENCH=1` 才打印）──
    //   为什么默认关：oracle 对照是**逐行比对**完整输出，打时间戳会让每次跑都不同。
    //   这一段用来量化 `forge-opt` 的效果（`-C opt-level=0` vs `2/3`）——mem2reg 上来之后
    //   递归/循环/内存写的差距最能说明问题。
    if std::env::var("FORGE_BENCH").is_ok() {
        fn ms<T>(label: &str, f: impl FnOnce() -> T) -> T {
            let t = std::time::Instant::now();
            let v = f();
            println!("bench {label}={}ms", t.elapsed().as_millis());
            v
        }

        // 递归（调用约定 + 栈帧）
        let fib = ms("fib32", || {
            fn fib(n: u64) -> u64 {
                if n < 2 { n } else { fib(n - 1) + fib(n - 2) }
            }
            fib(32)
        });
        // 紧循环（整数算术 + 归纳变量）；**用 wrapping 折叠**——50M 个平方和必然溢出，
        // debug 档开着溢出检查（第一版就是在这里 panic 的：`attempt to add with overflow`）。
        let sum = ms("sum_sq", || {
            (0u64..50_000_000).fold(0u64, |acc, i| {
                acc.wrapping_add(i.wrapping_mul(i))
            })
        });
        // 内存写 + 分支（筛法）
        let primes = ms("sieve2m", || {
            let n = 2_000_000usize;
            let mut sieve = vec![true; n];
            let mut count = 0usize;
            let mut i = 2usize;
            while i * i < n {
                if sieve[i] {
                    let mut j = i * i;
                    while j < n {
                        sieve[j] = false;
                        j += i;
                    }
                }
                i += 1;
            }
            for is_prime in sieve.iter().skip(2) {
                if *is_prime {
                    count += 1;
                }
            }
            count
        });
        // 排序（库内快排：比较器/交换/递归全在 std）
        let sorted = ms("sort200k", || {
            let mut xs: Vec<u64> = Vec::with_capacity(200_000);
            let mut s = 0x1234_5678_9abc_def0u64;
            for _ in 0..200_000 {
                s = s.wrapping_mul(6364136223846793005).wrapping_add(1442695040888963407);
                xs.push(s);
            }
            xs.sort_unstable();
            xs[100_000]
        });
        // 结果只用来防优化掉（打印前 8 位，避免输出随优化档位变化——这三个值本身是定值）
        println!(
            "bench_checks fib={fib} sum={sum} primes={primes} sorted_mid_hi={:x}",
            sorted >> 32
        );
    }

    // ── ⑯ u128 跨边界（v27 M6-69）──
    //
    // 这些都是 **LLVM 编的 core/std**：`u128::from_str`、`Display for u128`、
    // `checked_add`/`checked_mul`、`u128::MAX` 常量。于是它们走的是 **Rust ABI 的
    // 128 位传参/返回**，正好把"我们与 LLVM 的 128 位约定是否一致"钉住——
    // 此前 e2e 里的 u128 用例全在 forge 自编自调的内部，这条是第一次跨到 LLVM 侧。
    {
        let max_u128: u128 = "340282366920938463463374607431768211455"
            .parse()
            .expect("u128 解析");
        check(
            "u128_parse",
            format!("{max_u128}"),
            "340282366920938463463374607431768211455".to_string(),
        );
        check(
            "u128_wrapping_add",
            format!("{}", max_u128.wrapping_add(1)),
            "0".to_string(),
        );
        check(
            "u128_hi_half",
            format!("{}", (max_u128 >> 64) as u64),
            "18446744073709551615".to_string(),
        );
        check(
            "u128_checked_add",
            format!("{:?}", max_u128.checked_add(1)),
            "None".to_string(),
        );
        check(
            "u128_from_u64",
            format!("{}", u128::from(u64::MAX)),
            "18446744073709551615".to_string(),
        );
    }

    // ── 汇总 ──
    if failures.is_empty() {
        println!("all_ok=true");
    } else {
        println!("all_ok=false");
        for f in &failures {
            println!("MISMATCH {f}");
        }
        std::process::exit(1);
    }
}
