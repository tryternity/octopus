use octopus_translation::{TranslationEngine, load_opus_mt};

#[test]
#[ignore = "需要本地 opus-mt 模型（~/.octopus/models/translate/opus-mt/zh-en）"]
fn test_opus_mt_zh_to_en() {
    let engine = load_opus_mt("zh", "en").expect("模型加载失败——请确保 opus-mt zh-en 已下载");
    let result = engine.translate("你好世界", "zh", "en").expect("翻译失败");
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

#[test]
#[ignore = "需要本地 opus-mt 模型（~/.octopus/models/translate/opus-mt/en-zh）"]
fn test_opus_mt_en_to_zh() {
    let engine = load_opus_mt("en", "zh").expect("模型加载失败——请确保 opus-mt en-zh 已下载");
    let result = engine.translate("Hello world", "en", "zh").expect("翻译失败");
    println!("en→zh: Hello world → {}", result);
    assert!(!result.is_empty(), "翻译结果不应为空");
}
