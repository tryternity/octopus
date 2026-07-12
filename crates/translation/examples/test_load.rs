use octopus_translation::{M2M100Engine, TranslationEngine};

fn main() {
    // First: test direct ort loading from HF cache (original model)
    let path = std::env::var("HOME").unwrap() + "/.cache/huggingface/hub/models--venddair--m2m100-418M-onnx-int8/snapshots/c85ea6ecedffa9ff3fc60a930438745b060cd167/encoder_model.onnx";
    let real = std::fs::canonicalize(&path).unwrap();
    println!("Test 1: ort direct load from HF cache");
    let mut b = ort::session::Session::builder().unwrap();
    match b.commit_from_file(&real) {
        Ok(_) => println!("  OK!"),
        Err(e) => println!("  FAILED: {:?}", e),
    }

    // Test 2: M2M100Engine::load()
    println!("\nTest 2: M2M100Engine::load()");
    match M2M100Engine::load() {
        Ok(e) => {
            println!("  OK! engine={}", e.name());
            match e.translate("Hello", "en", "zh") {
                Ok(r) => println!("  translate: Hello → {}", r),
                Err(e) => println!("  translate FAILED: {:?}", e),
            }
        }
        Err(e) => println!("  FAILED: {:?}", e),
    }
}
