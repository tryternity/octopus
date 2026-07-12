use octopus_translation::{TranslationEngine, M2M100Engine};

#[test]
fn test_m2m100_load() {
    match M2M100Engine::load() {
        Ok(_) => println!("Load OK"),
        Err(e) => {
            println!("Load failed: {:?}", e);
            // Print what resolve_model_dir returns
            match onnx_infra::resolve_model_dir("venddair/m2m100-418M-onnx-int8") {
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
fn test_m2m100_zh_to_en() {
    let engine = M2M100Engine::load().expect("模型加载失败——请确保 m2m100 已下载");
    let result = engine.translate("你好世界", "zh", "en").expect("翻译失败");
    println!("zh→en: 你好世界 → {}", result);
    assert!(!result.is_empty());
}

#[test]
fn test_m2m100_en_to_zh() {
    let engine = M2M100Engine::load().expect("模型加载失败");
    let result = engine.translate("Hello world", "en", "zh").expect("翻译失败");
    println!("en→zh: Hello world → {}", result);
    assert!(!result.is_empty());
}
