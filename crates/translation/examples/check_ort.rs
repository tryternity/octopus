fn main() {
    println!("ORT init test");
    // This will force ort to initialize and we can check which version is loaded
    let builder = ort::session::Session::builder();
    println!("Builder created: {:?}", builder.is_ok());
    
    // Try loading the model
    let path = std::env::var("HOME").unwrap() + "/.cache/huggingface/hub/models--venddair--m2m100-418M-onnx-int8/snapshots/c85ea6ecedffa9ff3fc60a930438745b060cd167/encoder_model.onnx";
    let real = std::fs::canonicalize(&path).unwrap_or(path.into());
    println!("Loading: {:?}", real);
    let mut b = builder.unwrap();
    match b.commit_from_file(&real) {
        Ok(_) => println!("SUCCESS!"),
        Err(e) => println!("FAILED: {:?}", e),
    }
}
