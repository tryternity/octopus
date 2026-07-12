use octopus_translation::{M2M100Engine, TranslationEngine};

fn main() {
    let engine = M2M100Engine::load().expect("模型加载失败");
    println!("Engine: {}", engine.name());

    // 短文本
    for (text, src, tgt) in [
        ("Hello world", "en", "zh"),
        ("你好世界", "zh", "en"),
    ] {
        match engine.translate(text, src, tgt) {
            Ok(r) => println!("{}→{}: {} → {}", src, tgt, text, r),
            Err(e) => println!("{}→{} FAILED: {:?}", src, tgt, e),
        }
    }

    // 长文本分段测试
    let long_text = "The weather is nice today. I want to go for a walk in the park. \
        The sun is shining brightly and the birds are singing. \
        It is a perfect day for a picnic with friends. \
        We should bring some sandwiches and drinks. \
        Let us meet at the entrance of the park at noon. \
        Do not forget to bring your camera. \
        The cherry blossoms are in full bloom this time of year. \
        We can take many beautiful photos together. \
        I am looking forward to seeing you all there.";
    match engine.translate(long_text, "en", "zh") {
        Ok(r) => println!("\nen→zh (long):\n{}", r),
        Err(e) => println!("long FAILED: {:?}", e),
    }
}
