//! 文件级 IR 缓存：把"已解析的模块"按**内容键**落盘，重复读同一份源码时直接反序列化。
//!
//! # 为什么键是内容而不是路径
//!
//! 缓存要回答的问题是"这份**源码文本**我已经编译过了吗"。用内容（源码字节 + 格式版本
//! + producer）做键有两个直接好处：
//!
//! 1. **改文件即失效**：源码变了，键就变了，不存在"忘了清缓存"导致的陈旧结果；
//! 2. **跨机器/跨路径可共享**：同一份语料放在不同目录、不同 CI 检出路径下命中同一个
//!    条目（bench/CI 夹具正是这个形态）。
//!
//! # 键的强度（诚实交代）
//!
//! 不引入依赖 ⇒ 用自研 128 位指纹（两个不同基底的 FNV-1a 64 拼接）。这不是密码学
//! 哈希：碰撞概率 ~2⁻¹²⁸ 量级（本项目语料规模下可忽略），但**如果**真碰撞了，缓存会
//! 返回"另一个同样合法的模块"——因此本缓存的定位是**可随时删除的加速层**，不是
//! 事实源：任何时候删掉整个目录只损失速度，不损失正确性。
//!
//! # 自愈
//!
//! - 条目缺失、读不动、版本不符、字节流损坏 ⇒ 一律当 **miss**（`load` 返回 `None`），
//!   绝不 panic、绝不"半信半疑地用"；
//! - 落盘走"先写临时文件再 `rename`"（同目录 rename 在 Windows/POSIX 都是原子替换），
//!   因此读者不会看到半截条目；
//! - [`IrCache::get_or_insert_with`] 在 miss 时重算并**覆盖**坏条目 ⇒ 坏条目自动修好。

use std::fs;
use std::io;
use std::path::{Path, PathBuf};

use crate::ir::function::Module;

use super::format::{IR_FORMAT_VERSION, PRODUCER};

/// 缓存条目的扩展名。
const ENTRY_EXT: &str = "fir";
/// 落盘临时文件的扩展名（写完 `rename` 成正式名）。
const TMP_EXT: &str = "fir-tmp";

/// 内容键（128 位指纹的十六进制形式）。
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct CacheKey(String);

impl CacheKey {
    /// 由源码文本算出键。
    ///
    /// 参与指纹的还有**格式版本与 producer**：格式一变，旧条目整体失效（不需要任何
    /// 手工清理）。
    pub fn of_source(source: &str) -> Self {
        let mut h = Fnv128::new();
        h.write(b"forge-ir/ir-cache/v1\0");
        h.write(&IR_FORMAT_VERSION.to_le_bytes());
        h.write(PRODUCER.as_bytes());
        h.write(b"\0");
        h.write(source.as_bytes());
        let (lo, hi) = h.finish();
        Self(format!("{lo:016x}{hi:016x}"))
    }

    /// 键的十六进制文本。
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

/// 目录型 IR 缓存（目录不存在时按需创建）。
#[derive(Clone, Debug)]
pub struct IrCache {
    dir: PathBuf,
}

impl IrCache {
    /// 打开（必要时创建）一个缓存目录。
    pub fn new(dir: impl AsRef<Path>) -> io::Result<Self> {
        let dir = dir.as_ref().to_path_buf();
        fs::create_dir_all(&dir)?;
        Ok(Self { dir })
    }

    /// 缓存目录。
    pub fn dir(&self) -> &Path {
        &self.dir
    }

    /// 某源码对应的条目路径（无论是否已存在）。
    pub fn entry_path(&self, source: &str) -> PathBuf {
        self.dir.join(format!(
            "{}.{ENTRY_EXT}",
            CacheKey::of_source(source).as_str()
        ))
    }

    /// 命中即返回解码后的模块；未命中、读失败、损坏、版本不符一律 `None`。
    ///
    /// 缓存层**不解释失败原因**：调用方本来就有一条"自己算一遍"的正确路径。
    pub fn load(&self, source: &str) -> Option<Module> {
        let bytes = fs::read(self.entry_path(source)).ok()?;
        Module::from_binary(&bytes).ok()
    }

    /// 落盘（同键覆盖）。写失败只影响速度、不影响正确性，因此调用方通常可忽略错误。
    pub fn store(&self, source: &str, module: &Module) -> io::Result<()> {
        let target = self.entry_path(source);
        // 临时名带 pid：并发写同一个键时不至于互相踩（rename 仍是最后一步的仲裁）。
        let tmp = self.dir.join(format!(
            "{}.{}.{TMP_EXT}",
            CacheKey::of_source(source).as_str(),
            std::process::id()
        ));
        fs::write(&tmp, module.to_binary())?;
        fs::rename(&tmp, &target)
    }

    /// 命中则返回缓存模块；未命中则调 `build`，成功则落盘并返回。
    ///
    /// `build` 返回 `None` 表示"这份源码造不出模块"（例如解析失败）——此时不落盘，
    /// 也不缓存失败结果。
    pub fn get_or_insert_with(
        &self,
        source: &str,
        build: impl FnOnce() -> Option<Module>,
    ) -> Option<Module> {
        if let Some(hit) = self.load(source) {
            return Some(hit);
        }
        let module = build()?;
        // 落盘失败只是没缓存上，不影响本次结果。
        let _ = self.store(source, &module);
        Some(module)
    }

    /// 缓存里当前有多少条目（诊断/测试用；不递归子目录）。
    pub fn entry_count(&self) -> usize {
        fs::read_dir(&self.dir)
            .map(|it| {
                it.filter_map(Result::ok)
                    .filter(|e| {
                        e.path()
                            .extension()
                            .is_some_and(|x| x == std::ffi::OsStr::new(ENTRY_EXT))
                    })
                    .count()
            })
            .unwrap_or(0)
    }
}

/// 128 位指纹：两个不同基底的 FNV-1a 64。
///
/// 只要确定性、无依赖、够快（每份源码算一次）；不是密码学哈希（见模块文档）。
struct Fnv128 {
    lo: u64,
    hi: u64,
}

impl Fnv128 {
    fn new() -> Self {
        // 两个公认的 64 位 FNV-1a offset basis（第二个换基底启动，避免两条轨道同步）
        Self {
            lo: 0xcbf2_9ce4_8422_2325,
            hi: 0x9E37_79B9_7F4A_7C15,
        }
    }

    fn write(&mut self, bytes: &[u8]) {
        for &b in bytes {
            self.lo ^= u64::from(b);
            self.lo = self.lo.wrapping_mul(0x0000_0100_0000_01B3);
            self.hi ^= u64::from(b).wrapping_add(0x9E37_79B9);
            self.hi = self.hi.wrapping_mul(0x0000_0100_0000_01B3);
        }
    }

    fn finish(self) -> (u64, u64) {
        (self.lo, self.hi)
    }
}
