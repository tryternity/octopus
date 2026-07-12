use octopus_translation::{M2M100Engine, TranslationEngine};

fn main() {
    let engine = M2M100Engine::load().expect("模型加载失败");
    println!("Engine: {}", engine.name());

    for (text, src, tgt) in [
        ("Hello world", "en", "zh"),
        ("你好世界", "zh", "en"),
        ("The weather is nice today.", "en", "zh"),
        ("今天天气很好。", "zh", "en"),
    ] {
        match engine.translate(text, src, tgt) {
            Ok(r) => println!("{}→{}: {} → {}", src, tgt, text, r),
            Err(e) => println!("{}→{} FAILED: {:?}", src, tgt, e),
        }
    }
}
