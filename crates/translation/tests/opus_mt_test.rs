use octopus_translation::load_opus_mt;

#[tokio::test]
#[ignore = "需要本地 opus-mt 模型（设置 > 模型管理 > 翻译模型 下载 opus-mt）"]
async fn test_opus_mt_zh_to_en() {
    let engine = load_opus_mt("zh", "en").expect("模型加载失败——请确保 opus-mt zh-en 已下载");
    let result = engine.translate("你好世界", "zh", "en").await.expect("翻译失败");
    println!("zh→en: 你好世界 → {}", result);
    assert!(!result.is_empty(), "翻译结果不应为空");
    // 无明显重复：连续相同词不应超 3 次
    let words: Vec<&str> = result.split_whitespace().collect();
    if words.len() >= 4 {
        for w in words.windows(4) {
            let all_same = w.iter().all(|&x| x == w[0]);
            assert!(!all_same, "翻译结果有 4+ 连续重复词: {:?}", w);
        }
    }
}

#[tokio::test]
#[ignore = "需要本地 opus-mt 模型（设置 > 模型管理 > 翻译模型 下载 opus-mt）"]
async fn test_opus_mt_en_to_zh() {
    let engine = load_opus_mt("en", "zh").expect("模型加载失败——请确保 opus-mt en-zh 已下载");
    let result = engine.translate("Hello world", "en", "zh").await.expect("翻译失败");
    println!("en→zh: Hello world → {}", result);
    assert!(!result.is_empty(), "翻译结果不应为空");
}

/// 回归：带空格的中文翻译不应被截断。
/// 曾因 tokenizer (WhitespaceSplit + Metaspace) 对带空格中文产生句中独立 ▁ token，
/// 偏离训练分布导致 decoder 过早 EOS，「要看 猫是主动咬…」只译出 "It depends."（2 词）。
/// 修复 = encode 前 normalize_cjk_spaces 移除 CJK 邻接空格。本测断言带空格版词数足够。
#[tokio::test]
#[ignore = "需要本地 opus-mt 模型（设置 > 模型管理 > 翻译模型 下载 opus-mt）"]
async fn test_opus_mt_cjk_space_not_truncated() {
    let engine = load_opus_mt("zh", "en").expect("模型加载失败——请确保 opus-mt zh-en 已下载");
    let with_space = engine.translate("要看 猫是主动咬 还是被咬人去招", "zh", "en").await.expect("翻译失败");
    println!("带空格: 要看 猫是主动咬… → {}", with_space);
    // 截断时仅 2 词（"It depends."），完整翻译 9 词。阈值 5 安全区分。
    assert!(
        with_space.split_whitespace().count() >= 5,
        "带空格中文翻译被截断: {:?} (仅 {} 词)", with_space, with_space.split_whitespace().count()
    );
}
