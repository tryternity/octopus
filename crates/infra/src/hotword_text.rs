//! 热词文本工具：拼音首字母 + 写入规范化（切词→去重→排序→拼接）。
//! 纯函数、无 DB、无全局状态——供 db.rs（迁移/写 words_text）与 asr-local/desktop 复用。

use pinyin::ToPinyin;
use uuid::Uuid;

/// 词 → 拼音首字母串（大写，非汉字跳过）。如「八爪鱼」→`BZY`、「浮窗」→`FC`。
pub fn pinyin_initials(word: &str) -> String {
    word.chars()
        .filter_map(|c| c.to_pinyin().and_then(|p| p.plain().chars().next()))
        .map(|c| c.to_ascii_uppercase())
        .collect()
}

// ── 热词单记录（hotword_words 表，2026-08-01 schema v57）──────────────────────
// 每词一条记录，业务键 (set_id, word)，id 用确定性 UUID v5 跨设备一致。

/// 热词 UUID v5 的固定 namespace（任意固定值，跨设备/版本必须一致）。
/// 一旦定下不可改（改了所有词 id 变化，sync 全量冲突）。
// hex = "octopus_hotword" ASCII 编码 + _00 后缀，语义固定不重排
#[allow(clippy::unusual_byte_groupings)]
pub const HOTWORD_NAMESPACE: Uuid = Uuid::from_u128(0x6f63746f7075735f686f74776f7264_00);

/// 生成热词词记录的确定性 UUID（v5 SHA1-based）。
/// `(set_id, word)` 相同 → UUID 相同 → 跨设备独立加同词到同词典天然合并。
/// 输出标准带连字符 UUID 格式，复用 sync `shard_dir`（filter hex take 2）。
pub fn hotword_word_uuid(set_id: &str, word: &str) -> String {
    // 用 "{set_id}/{word}" 拼接作 v5 name，避免 set_id 含分隔符歧义（set_id 是 UUID，不含 /）。
    Uuid::new_v5(&HOTWORD_NAMESPACE, format!("{set_id}/{word}").as_bytes()).to_string()
}

/// 词 → 原始拼音 Vec（每字 `to_pinyin().plain()`，非汉字跳过）。
/// 如「八爪鱼」→ `["ba", "zhao", "yu"]`。**不经归一化**（方言规则运行时生效，DB 只存原始）。
/// 含非汉字的词返回的 Vec 长度 < 字数，调用方可据此判断是否有效热词。
pub fn word_plain_pinyins(word: &str) -> Vec<String> {
    word.chars()
        .filter_map(|c| c.to_pinyin().map(|p| p.plain().to_string()))
        .collect()
}

// ── 热词词记录 sync md5 指纹（2026-08-01 word 级 merge）─────────────────────────

/// 词记录 sync md5（hex 32 字符）——长度前缀分隔防 `|` 碰撞。
///
/// 拼接格式：`{set_id_len}|{set_id}|{word_len}|{word}`
///
/// **不含 created_at/updated_at**（时间戳跨设备必然不同）。词文本含任意 Unicode（含 `|`），
/// 故用长度前缀分隔——单纯用 `|` 分隔时 `{a}|{b}` 与 `{a|b}|{}` 会碰撞。长度前缀无歧义。
///
/// **不含 pinyin/is_deleted**（v58 修正）：md5 是身份指纹（这个词是谁），不是状态快照。
/// pinyin 是从 word 派生的数据（word 变了拼音自然变），is_deleted 是状态（靠 updated_at
/// 时间戳比较决定 merge 方向）。含这些会导致删除/拼音重算后 md5 变化但 outline 没同步 → 不必要 diff。
///
/// 此函数放在 infra（无项目内依赖的底层 crate），sync crate 与 db crate 都调它。
pub fn hotword_word_md5_from_fields(
    set_id: &str,
    word: &str,
) -> String {
    let input = format!(
        "{}|{}|{}|{}",
        set_id.len(),
        set_id,
        word.len(),
        word,
    );
    use md5::{Digest, Md5};
    let mut hasher = Md5::new();
    hasher.update(input.as_bytes());
    let result = hasher.finalize();
    let mut s = String::with_capacity(32);
    for b in result.iter() {
        s.push_str(&format!("{:02x}", b));
    }
    s
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pinyin_initials_basic() {
        assert_eq!(pinyin_initials("八爪鱼"), "BZY");
        assert_eq!(pinyin_initials("浮窗"), "FC");
        assert_eq!(pinyin_initials("热词"), "RC");
        assert_eq!(pinyin_initials("AI助手"), "ZS"); // 非汉字跳过
        assert_eq!(pinyin_initials(""), "");
    }

    // ── hotword_word_uuid 确定性（v5）──

    #[test]
    fn hotword_word_uuid_is_deterministic() {
        // 同 (set_id, word) → 同 UUID（跨设备一致，v5 SHA1-based）
        let a = hotword_word_uuid("00000000-0000-0000-0000-000000000001", "八爪鱼");
        let b = hotword_word_uuid("00000000-0000-0000-0000-000000000001", "八爪鱼");
        assert_eq!(a, b);
    }

    #[test]
    fn hotword_word_uuid_differs_by_set_or_word() {
        let base = hotword_word_uuid("00000000-0000-0000-0000-000000000001", "八爪鱼");
        // 不同词 → 不同 UUID
        assert_ne!(base, hotword_word_uuid("00000000-0000-0000-0000-000000000001", "浮窗"));
        // 不同 set → 不同 UUID（同词跨 set 是独立记录）
        assert_ne!(base, hotword_word_uuid("00000000-0000-0000-0000-000000000002", "八爪鱼"));
    }

    #[test]
    fn hotword_word_uuid_is_valid_uuid_format() {
        let id = hotword_word_uuid("00000000-0000-0000-0000-000000000001", "八爪鱼");
        // 标准 8-4-4-4-12 带连字符，v5 版本号（第三段开头 5）
        assert_eq!(id.len(), 36);
        assert!(id.starts_with("00000") || id.chars().nth(14) == Some('5'),
            "v5 UUID 第三段应以 5 开头，got {id}");
    }

    // ── word_plain_pinyins 原始拼音 ──

    #[test]
    fn word_plain_pinyins_basic() {
        assert_eq!(word_plain_pinyins("八爪鱼"), vec!["ba".to_string(), "zhao".to_string(), "yu".to_string()]);
        assert_eq!(word_plain_pinyins("浮窗"), vec!["fu".to_string(), "chuang".to_string()]);
    }

    #[test]
    fn word_plain_pinyins_skips_non_hanzi() {
        // 含非汉字：返回 Vec 长度 < 字数（调用方可据此判断有效性）
        let py = word_plain_pinyins("AI助手");
        assert_eq!(py, vec!["zhu".to_string(), "shou".to_string()]); // 只有「助手」两字
        assert_eq!(py.len(), 2); // < 4 字符
    }

    #[test]
    fn word_plain_pinyins_empty_for_pure_non_hanzi() {
        assert!(word_plain_pinyins("ABC123").is_empty());
        assert!(word_plain_pinyins("").is_empty());
    }
}
