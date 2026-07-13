use octopus_translation::{TranslationEngine, M2M100Engine};

#[test]
#[ignore = "需要本地 m2m100 模型（lazycodepersona/m2m100_418m）"]
fn test_m2m100_load() {
    match M2M100Engine::load() {
        Ok(_) => println!("Load OK"),
        Err(e) => {
            println!("Load failed: {:?}", e);
            match onnx_infra::resolve_model_dir("lazycodepersona/m2m100_418m") {
                Ok(p) => {
                    println!("resolve_model_dir: {:?}", p);
                    println!("canonicalize: {:?}", std::fs::canonicalize(&p));
                }
                Err(e2) => println!("resolve_model_dir error: {:?}", e2),
            }
        }
    }
}

#[test]
#[ignore = "需要本地 m2m100 模型（lazycodepersona/m2m100_418m）"]
fn test_m2m100_zh_to_en() {
    let engine = M2M100Engine::load().expect("模型加载失败——请确保 m2m100 已下载");
    let result = engine.translate("你好世界", "zh", "en").expect("翻译失败");
    println!("zh→en: 你好世界 → {}", result);
    assert!(!result.is_empty());
}

#[test]
#[ignore = "需要本地 m2m100 模型（lazycodepersona/m2m100_418m）"]
fn test_m2m100_en_to_zh() {
    let engine = M2M100Engine::load().expect("模型加载失败");
    let result = engine.translate("Hello world", "en", "zh").expect("翻译失败");
    println!("en→zh: Hello world → {}", result);
    assert!(!result.is_empty());
}

/// 对照 opus-mt 截断 bug：m2m100 对带空格中文**不应**截断。
/// m2m100 训练惯例为分词后空格连接的中文，带空格符合分布（与 opus-mt 的连续字符训练不同），
/// 故 m2m100 不套用 normalize_cjk_spaces（opus-mt 专属修复）。本测实证：同一段带空格中文，
/// opus-mt 截断为 "It depends."（2 词），m2m100 完整译出（实测 12 词）。断言带空格版词数充足。
#[test]
#[ignore = "需要本地 m2m100 模型（lazycodepersona/m2m100_418m）"]
fn test_m2m100_cjk_space_not_truncated() {
    let engine = M2M100Engine::load().expect("模型加载失败——请确保 m2m100 已下载");
    let with_space = engine.translate("要看 猫是主动咬 还是被咬人去招", "zh", "en").expect("翻译失败");
    println!("带空格: 要看 猫是主动咬… → {}", with_space);
    // 实测 12 词（完整句）。截断会只剩 2-3 词（如 opus-mt 的 "It depends."）。
    assert!(
        with_space.split_whitespace().count() >= 5,
        "m2m100 带空格中文疑似截断: {:?} (仅 {} 词)——若如此，需查 token 序列重估是否归一化",
        with_space,
        with_space.split_whitespace().count()
    );
}
