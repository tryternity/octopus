//! K_machine（保护 app_key 的本机密钥）持久化。
//!
//! ## 历史背景
//! 早期实现把 K_machine 存进 OS Keychain（macOS Keychain / Windows Credential
//! Manager / Linux Secret Service）。但 octopus-desktop 是 adhoc 签名的开发版，
//! macOS 对此类二进制写入 Keychain 的项是 **session-only**——重启进程后立刻丢失：
//! ```text
//! process 1: keyring.set("K") → "OK"
//! process 2: keyring.get("K") → "NoEntry"
//! ```
//! 这让"本机无感启动"完全失效（每次重启都丢 K_machine → 解不开 app_key_local_enc
//! → 强制弹主密码）。
//!
//! ## 现行方案：本地混淆文件（#13 修订 2026-07-24：如实表述防护强度）
//!
//! 把 K_machine 写到 `~/.octopus/machine-key.enc`，用 HKDF-SHA256 派生的
//! file_key 做 AES-256-GCM 加密。**但需明确：这是 obfuscation 而非真加密**——
//! `derive_file_key()` 的四个输入全是公开或硬编码的：
//!   - `machine_id`（IOPlatformUUID / machine-id / MachineGuid）——本机任意进程可读
//!   - `username`（USER 环境变量）——本机任意进程可读
//!   - `FILE_KEY_SALT` / `FILE_KEY_INFO`——源码常量
//!
//! 因此任何能读到 `machine-key.enc` 的**同机进程**都能解出 K_machine。
//! 实际防护等价于文件权限 0600——真正保障是：
//!   - 换机 / 拷走 DB 单独（没拿到 machine-key.enc）→ 解不开 app_key（仍需主密码）
//!   - 重启 / kill -9 后仍可用（同机 + 同用户 → 同 file_key）
//!
//! 这是 adhoc 签名无法用 Keychain 的**已知妥协**（生产签名后应切回 Keychain 方案）。
//! 之前注释反复称「AES-256-GCM 加密」夸大了防护强度——算法本身没问题，问题在
//! 加密 key 的派生输入非秘密。详见 docs/superpowers/specs/2026-07-18-password-vault-design.md §6.2。
//!
//! ## 跨平台 machine_id
//! - macOS：`ioreg` 读 `IOPlatformUUID`（重启稳定，每机唯一）
//! - Linux：`/etc/machine-id` 或 `/var/lib/dbus/machine-id`
//! - Windows：`reg query ... MachineGuid`
//!
//! ## 公开 API（不变）
//! 仍暴露 `load_or_create_machine_key` / `load_machine_key` /
//! `save_machine_key` / `delete_machine_key`，调用方（unlock.rs / tests）无需改动。

use anyhow::{ensure, Context, Result};
use hkdf::Hkdf;
use sha2::Sha256;
use std::path::PathBuf;

use crate::crypto::util::random_32;
use crate::crypto::DerivedKey;
use crate::Zeroizing;

/// 文件名（相对 `~/.octopus/`）。`.enc` 后缀提示这是密文。
const MACHINE_KEY_FILE: &str = "machine-key.enc";

/// HKDF salt + info（固定，spec INV：改了会让现有 file 解不开）。
const FILE_KEY_SALT: &[u8] = b"octopus-vault-machine-key-file/v1";
const FILE_KEY_INFO: &[u8] = b"file-key";

// 向后兼容：保留这两个常量名（test override 仍用它们做复合 key），避免破坏
// 已有的测试字符串引用。
pub const KEYCHAIN_SERVICE: &str = "octopus-vault";
pub const KEYCHAIN_USER: &str = "machine-key";

// 测试专用：thread-local 的 in-memory Keychain 覆盖。
//
// 与 `octopus_infra::db::set_test_db()` 同构：
//   - thread_local! + RefCell<Option<...>>（不用 OnceLock<Mutex<...>>，与 Wave 1 保持一致）
//   - 不用 `#[cfg(test)]` 门控，保持运行时可见，便于跨 crate
//     （octopus-vault / octopus-desktop）的单元测试调用
//   - 未设置时（生产 / 普通测试）所有公开函数走真实文件路径，
//     仅多一次 thread_local 读，对生产零影响。
thread_local! {
    static TEST_KEYCHAIN_OVERRIDE: std::cell::RefCell<Option<InMemoryKeychain>>
        = std::cell::RefCell::new(None);
}

/// 进程内 in-memory Keychain（仅测试用）。
///
/// 用 `service + "\0" + user` 作为复合 key 存 32B raw bytes（不走 base64，
/// 因为这不是真实 OS Keychain，无需 string 化）。`\0` 在 service / user 中不合法，
/// 可安全作为分隔符。
#[derive(Default)]
#[doc(hidden)]
pub struct InMemoryKeychain {
    store: std::collections::HashMap<String, Vec<u8>>,
}

impl InMemoryKeychain {
    fn make_key(service: &str, user: &str) -> String {
        format!("{}\0{}", service, user)
    }

    fn set(&mut self, service: &str, user: &str, val: Vec<u8>) {
        self.store.insert(Self::make_key(service, user), val);
    }

    fn get(&self, service: &str, user: &str) -> Option<&Vec<u8>> {
        self.store.get(&Self::make_key(service, user))
    }

    fn delete(&mut self, service: &str, user: &str) -> bool {
        self.store
            .remove(&Self::make_key(service, user))
            .is_some()
    }
}

/// 测试专用：注入一个空的 in-memory Keychain，后续 4 个公开函数会优先使用它。
///
/// 多次调用替换前一次注入（不累积）。线程隔离：仅影响调用线程。
#[doc(hidden)]
pub fn set_test_keychain() {
    TEST_KEYCHAIN_OVERRIDE.with(|cell| {
        *cell.borrow_mut() = Some(InMemoryKeychain::default());
    });
}

/// 测试专用：清除 thread-local Keychain 覆盖，恢复真实文件路径。
#[doc(hidden)]
pub fn clear_test_keychain() {
    TEST_KEYCHAIN_OVERRIDE.with(|cell| {
        *cell.borrow_mut() = None;
    });
}

// ============================================================================
// 跨平台 machine_id / username 派生
// ============================================================================

#[cfg(target_os = "macos")]
fn read_machine_id() -> Result<String> {
    let output = std::process::Command::new("ioreg")
        .args(["-d2", "-c", "IOPlatformExpertDevice"])
        .output()
        .context("调用 ioreg 失败（无法读取本机 UUID）")?;
    let stdout = String::from_utf8_lossy(&output.stdout);
    for line in stdout.lines() {
        if line.contains("IOPlatformUUID") {
            // 行格式示例：`    "IOPlatformUUID" = "DEAD-BEEF-...."`
            if let Some(start) = line.rfind("= \"") {
                let rest = &line[start + 3..];
                if let Some(end) = rest.find('"') {
                    let uuid = rest[..end].trim().to_string();
                    if !uuid.is_empty() {
                        return Ok(uuid);
                    }
                }
            }
        }
    }
    anyhow::bail!("无法从 ioreg 输出中找到 IOPlatformUUID")
}

#[cfg(target_os = "linux")]
fn read_machine_id() -> Result<String> {
    for path in ["/etc/machine-id", "/var/lib/dbus/machine-id"] {
        if let Ok(s) = std::fs::read_to_string(path) {
            let trimmed = s.trim();
            if !trimmed.is_empty() {
                return Ok(trimmed.to_string());
            }
        }
    }
    anyhow::bail!(
        "无法读取 machine-id（试过 /etc/machine-id 和 /var/lib/dbus/machine-id）"
    )
}

#[cfg(target_os = "windows")]
fn read_machine_id() -> Result<String> {
    let output = std::process::Command::new("reg")
        .args([
            "query",
            r"HKLM\SOFTWARE\Microsoft\Cryptography",
            "/v",
            "MachineGuid",
        ])
        .output()
        .context("调用 reg query 失败（无法读取 MachineGuid）")?;
    let stdout = String::from_utf8_lossy(&output.stdout);
    for line in stdout.lines() {
        if line.contains("MachineGuid") {
            // 行格式示例：`    MachineGuid    REG_SZ    abc-def-...`
            if let Some(idx) = line.find("REG_SZ") {
                let guid = line[idx + 6..].trim().to_string();
                if !guid.is_empty() {
                    return Ok(guid);
                }
            }
        }
    }
    anyhow::bail!("无法从 reg query 输出中读取 MachineGuid")
}

fn read_username() -> Result<String> {
    std::env::var("USER")
        .or_else(|_| std::env::var("USERNAME"))
        .context("USER / USERNAME 环境变量未设置，无法派生 file_key")
}

/// HKDF-SHA256(machine_id:username, salt) → 32B file_key。
///
/// 同机 + 同用户必然返回同一把 key；任一变化则完全不同。
///
/// ⚠️ **注意（#13）**：file_key 的派生输入（machine_id / username / salt / info）
/// 全是公开或硬编码的——这把 key **不是秘密**。用它加密 machine-key.enc 是
/// obfuscation（防"拷走 DB 单独"的场景），不是真加密（同机进程都能解出）。
/// 详见模块顶部注释。
/// E-ZEROIZE-RESIDUE 修复（2026-07-26）：返 Zeroizing<[u8;32]> 而非裸 [u8;32]。
/// 与 random_32 同型修复——file_key 是敏感密钥（加密磁盘上的 K_machine），
/// HKDF 派生后裸 [u8;32] 在调用方 from_raw 会 Copy 残留。现返 Zeroizing 让
/// 调用方 from_zeroizing move，无栈残留。
fn derive_file_key() -> Result<Zeroizing<[u8; 32]>> {
    let machine_id = read_machine_id()?;
    let user = read_username()?;
    let input = format!("{}:{}", machine_id, user);

    let hk = Hkdf::<Sha256>::new(Some(FILE_KEY_SALT), input.as_bytes());
    let mut okm = Zeroizing::new([0u8; 32]);
    hk.expand(FILE_KEY_INFO, &mut *okm)
        .map_err(|_| anyhow::anyhow!("HKDF-SHA256 expand 失败（输出长度非法）"))?;
    Ok(okm)
}

// ============================================================================
// 文件路径
// ============================================================================

fn machine_key_path() -> Result<PathBuf> {
    // octopus_config_home() 返回 &'static Path（进程内 Lazy），这里 join 出新 PathBuf
    Ok(octopus_infra::octopus_config_home().join(MACHINE_KEY_FILE))
}

// ============================================================================
// 公开 API（签名保持不变）
// ============================================================================

/// 读取或创建 K_machine。
///
/// - 首次调用（文件不存在）→ 生成新 32B 随机 key 加密落盘，返回
/// - 后续调用 → 解密读出已有 key 返回
pub fn load_or_create_machine_key() -> Result<Zeroizing<[u8; 32]>> {
    if let Some(existing) = load_machine_key()? {
        return Ok(existing);
    }
    // E-ZEROIZE-RESIDUE 修复（2026-07-26）：random_32 现返 Zeroizing<[u8;32]>，
    // 这里直接 move 进返回值——无栈残留（之前 Zeroizing::new(裸数组) 是 Copy，
    // new_key 栈变量 drop no-op → K_machine 字节残留）。
    let new_key = random_32();
    save_machine_key(&*new_key)?;
    Ok(new_key)
}

/// 读取已有 K_machine。文件不存在或无法解密 → `Ok(None)`。
pub fn load_machine_key() -> Result<Option<Zeroizing<[u8; 32]>>> {
    // 测试覆盖优先：若设置了 thread-local InMemoryKeychain（Some(_)），全部
    // 走它（无论是否存过 K_machine）——保持生产行为完全不变仅在 override 被启用时。
    let test_hit =
        TEST_KEYCHAIN_OVERRIDE.with(|cell| -> Result<Option<Option<Zeroizing<[u8; 32]>>>> {
            let b = cell.borrow();
            match *b {
                None => Ok(None), // 未设测试覆盖 → 交还真实文件路径
                Some(ref kc) => match kc.get(KEYCHAIN_SERVICE, KEYCHAIN_USER) {
                    None => Ok(Some(None)), // override 设了但没存过
                    Some(bytes) => {
                        ensure!(bytes.len() == 32, "K_machine 长度异常：{} bytes", bytes.len());
                        // E-ZEROIZE-RESIDUE 修复（第五十六轮，与 :315 生产路径同型）——
                        // 测试 K_machine 是随机值残留无害，但保持模式一致防未来 copy-paste。
                        let mut arr = Zeroizing::new([0u8; 32]);
                        arr.copy_from_slice(bytes);
                        Ok(Some(Some(arr)))
                    }
                },
            }
        })?;
    if let Some(inner) = test_hit {
        return Ok(inner);
    }

    let path = machine_key_path()?;
    if !path.exists() {
        return Ok(None);
    }
    let ciphertext = std::fs::read_to_string(&path)
        .with_context(|| format!("读取 K_machine 文件失败：{}", path.display()))?;
    if !ciphertext.starts_with(crate::crypto::symmetric::CIPHERTEXT_PREFIX) {
        // 不是我们写的文件（或被篡改），视为不存在
        return Ok(None);
    }

    // E-ZEROIZE-RESIDUE 修复：derive_file_key 现返 Zeroizing，from_zeroizing move
    let file_key = DerivedKey::from_zeroizing(derive_file_key()?);
    // KC1 修复（2026-07-24）：解密失败（file_key 不匹配 / 文件损坏）→ Ok(None)，
    // 与上面「前缀不符 → Ok(None)」对称。两者语义都是「无可用 K_machine，需重建」。
    //
    // 换机迁移是合法触发场景：用户把整个 ~/.octopus/（含 machine-key.enc）拷到
    // 新机器，新机 machine_id 不同 → file_key 不同 → 解密失败。若返 Err：
    //   - unlock_app_key_local（unlock.rs:154）用 `?` 传播 → vault_state.rs:178
    //     只 log::warn 不提示输主密码 → app 僵死在「未解锁且无提示」
    //   - refresh_app_key_local_enc（unlock.rs:351）`?` 传播 → 主密码校验通过但
    //     整体 Err → 用户输对密码也解不开（死循环）
    //   - load_or_create（:237）`?` 传播 → 走不到「创建新 K_machine」分支
    // 改 Ok(None) 后三处全部自愈：流程 B 降级提示输主密码 / 流程 C 创建新
    // K_machine 重写 local_enc / load_or_create 走创建分支。
    //
    // K_machine 本就是 obfuscation 而非真秘密（模块注释 :14-30 已说明），重建
    // 无损安全性，仅丢失「检测到文件被篡改」的诊断信息（可接受——换机是合法
    // 场景，把诊断让位给可用性）。unlock.rs:148 文档已承诺「解密失败 → Ok(None)」，
    // 此处让实现对齐文档。
    let bytes = match file_key.decrypt(&ciphertext) {
        Ok(b) => b,
        Err(_) => return Ok(None),
    };
    ensure!(
        bytes.len() == 32,
        "K_machine 文件内容长度异常：{} bytes",
        bytes.len()
    );
    // E-ZEROIZE-RESIDUE 修复（2026-07-26，第五十六轮）：与 unlock.rs:172-174 同型
    // （decrypt → 裸栈 → Zeroizing::new Copy → 残留）。K_machine 是顶层机器主密钥，
    // load_machine_key 被 4 条路径调用（setup/unlock/refresh/change_password），每次
    // 启动都触发残留。改 Zeroizing::new([0u8;32]) 直接写入 + move，无栈残留。
    let mut arr = Zeroizing::new([0u8; 32]);
    arr.copy_from_slice(&bytes);
    Ok(Some(arr))
}

/// 保存 K_machine（覆盖式）。Unix 下文件权限 0600。
pub fn save_machine_key(key: &[u8; 32]) -> Result<()> {
    // 测试覆盖优先。
    let was_test = TEST_KEYCHAIN_OVERRIDE.with(|cell| {
        let mut b = cell.borrow_mut();
        if let Some(ref mut kc) = *b {
            kc.set(KEYCHAIN_SERVICE, KEYCHAIN_USER, key.to_vec());
            true
        } else {
            false
        }
    });
    if was_test {
        return Ok(());
    }

    // E-ZEROIZE-RESIDUE 修复：derive_file_key 现返 Zeroizing，from_zeroizing move
    let file_key = DerivedKey::from_zeroizing(derive_file_key()?);
    let ciphertext = file_key.encrypt(key)?;
    let path = machine_key_path()?;

    // #6 修复：原子写——temp file + sync_all + rename。
    //
    // 旧实现 `truncate(true) + write_all` 在 write_all 中途失败（进程崩溃 /
    // 磁盘满 / kill -9）会留下空 / 残缺文件——下次启动 `load_machine_key` 读
    // 到非 `v1:` 前缀的损坏数据时，按本文件 load 路径的"前缀不符视为不存在"
    // 分支会静默回退到"无 K_machine"，触发主密码流程；但若损坏数据恰好以
    // `v1:` 开头（极小概率），则会报"解密失败"。审计 #6 标为 medium。
    //
    // POSIX `rename(2)` 保证目标路径名要么是旧文件要么是新文件，永不会是部分
    // 新文件——这是文件级原子的标准模式。
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).with_context(|| {
            format!("创建 K_machine 父目录失败：{}", parent.display())
        })?;
    }
    let tmp_path = path.with_extension("enc.tmp");

    #[cfg(unix)]
    {
        use std::io::Write;
        use std::os::unix::fs::OpenOptionsExt;
        {
            let mut f = std::fs::OpenOptions::new()
                .write(true)
                .create(true)
                .truncate(true)
                .mode(0o600)
                .open(&tmp_path)
                .with_context(|| format!("打开 K_machine 临时文件失败：{}", tmp_path.display()))?;
            f.write_all(ciphertext.as_bytes()).with_context(|| {
                format!("写入 K_machine 临时文件失败：{}", tmp_path.display())
            })?;
            // fsync 保证内容落盘后再 rename——否则 rename 后内容可能仍是旧的。
            f.sync_all().with_context(|| {
                format!(
                    "fsync K_machine 临时文件失败：{}",
                    tmp_path.display()
                )
            })?;
        }
        std::fs::rename(&tmp_path, &path).with_context(|| {
            format!(
                "原子替换 K_machine 文件失败：{} -> {}",
                tmp_path.display(),
                path.display()
            )
        })?;
    }
    #[cfg(not(unix))]
    {
        // 非 Unix 也走 temp+rename（去掉 mode(0o600)）。Windows 上 ReplaceFile/
        // MoveFileEx 是原子的，std::fs::rename 在 Windows 内部使用 MoveFileEx
        // with REPLACE_EXISTING，跨文件系统会失败但 octopus_config_home 与其
        // 父目录总在同一卷，安全。
        {
            let mut f = std::fs::OpenOptions::new()
                .write(true)
                .create(true)
                .truncate(true)
                .open(&tmp_path)
                .with_context(|| {
                    format!("打开 K_machine 临时文件失败：{}", tmp_path.display())
                })?;
            use std::io::Write;
            f.write_all(ciphertext.as_bytes()).with_context(|| {
                format!("写入 K_machine 临时文件失败：{}", tmp_path.display())
            })?;
            f.sync_all().with_context(|| {
                format!(
                    "fsync K_machine 临时文件失败：{}",
                    tmp_path.display()
                )
            })?;
        }
        std::fs::rename(&tmp_path, &path).with_context(|| {
            format!(
                "原子替换 K_machine 文件失败：{} -> {}",
                tmp_path.display(),
                path.display()
            )
        })?;
    }
    Ok(())
}

/// 删除 K_machine（仅测试 / reset 用）。文件不存在视为成功。
pub fn delete_machine_key() -> Result<()> {
    // 测试覆盖优先。
    let was_test = TEST_KEYCHAIN_OVERRIDE.with(|cell| {
        let mut b = cell.borrow_mut();
        if let Some(ref mut kc) = *b {
            kc.delete(KEYCHAIN_SERVICE, KEYCHAIN_USER);
            true
        } else {
            false
        }
    });
    if was_test {
        return Ok(());
    }

    let path = machine_key_path()?;
    match std::fs::remove_file(&path) {
        Ok(()) => Ok(()),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(e) => Err(e).context("删除 K_machine 文件失败"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// derive_file_key 必须是确定性的（同机同用户 → 同一 32B）。
    /// 否则本机持久化方案根本不成立。
    #[test]
    fn test_derive_file_key_deterministic() {
        let k1 = derive_file_key().expect("derive_file_key 应在测试环境成功");
        let k2 = derive_file_key().expect("二次调用应同样成功");
        assert_eq!(k1, k2, "file_key 必须确定");
    }

    /// derive_file_key 输出 32B。
    #[test]
    fn test_derive_file_key_length() {
        let k = derive_file_key().expect("derive_file_key 应在测试环境成功");
        assert_eq!(k.len(), 32);
    }

    /// 测试覆盖路径下的 round-trip（走 thread-local，不碰真实文件）。
    #[test]
    fn test_machine_key_round_trip_via_override() {
        set_test_keychain();
        let _ = delete_machine_key();

        // 首次读应不存在
        assert!(load_machine_key().unwrap().is_none());

        // 创建
        let k1 = load_or_create_machine_key().unwrap();
        assert_eq!(k1.len(), 32);

        // 再次读应能拿到同一把
        let k2 = load_machine_key().unwrap().unwrap();
        assert_eq!(k1.as_ref(), k2.as_ref());

        // 再调 load_or_create 应返回同一把（不覆盖）
        let k3 = load_or_create_machine_key().unwrap();
        assert_eq!(k1.as_ref(), k3.as_ref());

        // 清理
        delete_machine_key().unwrap();
        assert!(load_machine_key().unwrap().is_none());

        clear_test_keychain();
    }

    /// 真实文件 round-trip（覆盖加密/解密 + 0600 + 跨进程语义）。
    ///
    /// #7 修复：加 `#[ignore]`。
    /// 此测试用 `set_var("HOME")` 隔离但 octopus_config_home 是 once_cell Lazy
    /// 缓存——若被其他测试先触发过，HOME 替换不生效，写入会落到开发者本机真实
    /// 的 `~/.octopus/machine-key.enc`，删掉用户已有的 K_machine。
    /// 用 `#[ignore]` 让它仅在 `cargo test -- --ignored` 时跑，避免 cargo test
    /// 默认运行的潜在副作用。
    #[test]
    #[ignore = "需要真实文件系统操作（round-trip via file），用 cargo test -- --ignored 跑"]
    fn test_machine_key_round_trip_via_file() {
        // 用临时 HOME 隔离：避免污染开发者本机的 ~/.octopus/machine-key.enc
        let tmp = tempfile_dir();
        let prev_home = std::env::var_os("HOME");
        std::env::set_var("HOME", &tmp);

        // 注意：octopus_infra::octopus_config_home 用 once_cell Lazy 缓存了第一次
        // 解析的路径——如果此测试运行前已被其他测试触发过，HOME 替换不会生效。
        // 因此这里改用直接调用 save / load 之外的"端到端"断言：仅校验
        // save→load 在同一进程内可往返（这是核心目标）。
        // 真正的"跨进程"验证依赖用户手工跑 manual test plan（见 commit message）。

        // 先清理可能存在的旧文件
        let _ = delete_machine_key();
        assert!(load_machine_key().unwrap().is_none());

        let key = random_32();
        save_machine_key(&*key).expect("save 应成功");
        let loaded = load_machine_key()
            .expect("load 应成功")
            .expect("应读到刚保存的 key");
        // 两边都是 Zeroizing<[u8;32]>，比较内部数组（* 解引用）
        assert_eq!(*loaded, *key);

        // #6 修复验证：原子写完成后 temp file 不应残留——rename 后 .tmp 路径已
        // 转移到目标路径，残留即说明 rename 未发生（fallback 到旧的 truncate+write）。
        let path = machine_key_path().unwrap();
        let tmp_path = path.with_extension("enc.tmp");
        assert!(
            !tmp_path.exists(),
            "#6 修复：save_machine_key 完成后 temp file 不应残留，但存在 {}",
            tmp_path.display()
        );

        // 文件权限 0600（仅 Unix）
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let path = machine_key_path().unwrap();
            let mode = std::fs::metadata(&path).unwrap().permissions().mode();
            assert_eq!(
                mode & 0o777,
                0o600,
                "K_machine 文件权限必须 0600，实际 {:o}",
                mode
            );
        }

        // 清理
        let _ = delete_machine_key();

        // 恢复 HOME
        match prev_home {
            Some(v) => std::env::set_var("HOME", v),
            None => std::env::remove_var("HOME"),
        }
    }

    /// KC1 回归守护：换机迁移场景——machine-key.enc 是合法 v1: 密文但用不同的
    /// file_key 加密（模拟新机 machine_id 不同 → file_key 不同）。
    ///
    /// 修复前：load_machine_key 返 Err → unlock_app_key_local 用 `?` 传播 →
    /// vault_state.rs Err 分支只 log 不提示 → app 僵死；refresh_app_key_local_enc
    /// 传播 Err → 主密码校验通过但整体失败（死循环）。
    ///
    /// 修复后：load_machine_key 返 Ok(None) → 三处调用点全部自愈（降级提示
    /// 输主密码 / 创建新 K_machine / load_or_create 走创建分支）。
    ///
    /// 同 via_file 测试用 #[ignore]：octopus_config_home 是 once_cell Lazy 缓存，
    /// HOME 替换在缓存已触发后不生效，默认 cargo test 不跑避免污染开发者本机。
    #[test]
    #[ignore = "需要真实文件系统操作（模拟换机解密失败），用 cargo test -- --ignored 跑"]
    fn load_machine_key_returns_none_on_decrypt_failure_machine_change() {
        let tmp = tempfile_dir();
        let prev_home = std::env::var_os("HOME");
        std::env::set_var("HOME", &tmp);
        let _ = delete_machine_key();

        // 用一把「错误」的 key（非本机 file_key）加密 32B 写成合法 v1: 密文——
        // 模拟旧机器的 machine-key.enc 被拷到新机，file_key 不匹配。
        let wrong_key = DerivedKey::from_raw([0xAA; 32]); // 任意非本机 file_key
        let ciphertext = wrong_key.encrypt(&[0xBB; 32]).expect("encrypt 应成功");
        let path = machine_key_path().unwrap();
        // 父目录可能不存在（临时 HOME 下无 .octopus），write 不创建目录，需先建
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent).expect("创建父目录应成功");
        }
        std::fs::write(&path, ciphertext.as_bytes()).expect("写文件应成功");

        // KC1 核心断言：解密失败应返 Ok(None)（而非 Err）
        let result = load_machine_key().expect("解密失败应返 Ok(None)，不是 Err");
        assert_eq!(
            result, None,
            "KC1: 换机解密失败应返 Ok(None)（需重建 K_machine），不能 Err 传播致启动僵死"
        );

        // 进一步验证自愈链路：load_or_create 应能创建新 K_machine（不因解密失败被堵）
        let new_key = load_or_create_machine_key().expect("load_or_create 应成功创建新 key");
        assert_eq!(new_key.len(), 32);
        // 新 key 应已落盘（用本机 file_key 重新加密），再 load 应拿到同一把
        let reloaded = load_machine_key().expect("reload 应成功").expect("应读到新 key");
        assert_eq!(new_key.as_ref(), reloaded.as_ref());

        // 清理
        let _ = delete_machine_key();
        match prev_home {
            Some(v) => std::env::set_var("HOME", v),
            None => std::env::remove_var("HOME"),
        }
    }

    fn tempfile_dir() -> std::path::PathBuf {
        // 不引入 tempfile crate 依赖：用进程 pid + 计数器造唯一目录。
        use std::sync::atomic::{AtomicU64, Ordering};
        static COUNTER: AtomicU64 = AtomicU64::new(0);
        let n = COUNTER.fetch_add(1, Ordering::SeqCst);
        let dir = std::env::temp_dir().join(format!(
            "octopus-kmachine-test-{}-{}-{}",
            std::process::id(),
            n,
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_millis()
        ));
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }
}
