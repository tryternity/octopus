fn main() {
    println!("ORT init test");
    let builder = ort::session::Session::builder();
    println!("Builder created: {:?}", builder.is_ok());

    let path = std::env::var("HOME").unwrap()
        + "/.cache/huggingface/hub/models--lazycodepersona--m2m100_418m/snapshots/"
        + "onnx/encoder_model_quantized.onnx";
    let real = std::fs::canonicalize(&path).unwrap_or(path.into());
    println!("Loading: {:?}", real);
    let mut b = builder.unwrap();
    match b.commit_from_file(&real) {
        Ok(_) => println!("SUCCESS!"),
        Err(e) => println!("FAILED: {:?}", e),
    }
}
