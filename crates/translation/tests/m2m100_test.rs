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
