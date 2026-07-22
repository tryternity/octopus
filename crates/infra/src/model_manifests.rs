//! 所有本地模型的预填下载清单（从 HF cache 本地文件计算）。
//!
//! DB v28 迁移时写入 models.secret_key，供 manifest 驱动下载。
//! 格式：`{"<相对路径>": {"source", "sha256", "size"}}`

/// ASR 模型 manifest。key = model_name，value = JSON 字符串。
pub fn asr_manifest(name: &str) -> Option<&'static str> {
    match name {
        "moonshine-base-en" => Some(MOONSHINE_BASE_EN),
        "moonshine-tiny-en" => Some(MOONSHINE_TINY_EN),
        "paraformer-bilingual" => Some(PARAFORMER_BILINGUAL),
        "paraformer-multi-zh" => Some(PARAFORMER_MULTI_ZH),
        "paraformer-streaming" => Some(PARAFORMER_STREAMING),
        "paraformer-zh" => Some(PARAFORMER_STREAMING), // 同 repo
        "qwen3-asr-0.6B" => Some(QWEN3_ASR_06B),
        "qwen3-asr-1.7B" => Some(QWEN3_ASR_17B),
        "sensevoice-orig-small" => Some(SENSEVOICE_ORIG_SMALL),
        "firered-asr2" => Some(FIRERED_ASR2),
        "whisper-small" => Some(WHISPER_SMALL),
        "zipformer" => Some(ZIPFORMER),
        "zipformer-large" => Some(ZIPFORMER_LARGE),
        "zipformer-small" => Some(ZIPFORMER_SMALL_CTC), // builtin 兜底引擎
        _ => None,
    }
}

/// 翻译模型 manifest。
pub fn translate_manifest(name: &str) -> Option<&'static str> {
    match name {
        "opus-mt" => Some(OPUS_MT),
        "m2m100-418M" => Some(M2M100_418M),
        _ => None,
    }
}

/// OCR 模型 manifest。
pub fn ocr_manifest(name: &str) -> Option<&'static str> {
    match name {
        "PP-OCRv6-small" => Some(OCR_V6_SMALL),
        "PP-OCRv5" => Some(OCR_V5),
        _ => None,
    }
}

const MOONSHINE_BASE_EN: &str = r#"{"cached_decode.int8.onnx":{"source":"{huggingface}/csukuangfj/sherpa-onnx-moonshine-base-en-int8/resolve/main/cached_decode.int8.onnx","sha256":"2db74e51cedf64a8b1be3c8192e0bb5e4923af0e90bd9e87f8e8771873f8ea03","size":99983837},"encode.int8.onnx":{"source":"{huggingface}/csukuangfj/sherpa-onnx-moonshine-base-en-int8/resolve/main/encode.int8.onnx","sha256":"7e38770f776f2e5583a53b052936005df2ba5c833d7e09c2a5fd796b94bf73e2","size":50311494},"preprocess.onnx":{"source":"{huggingface}/csukuangfj/sherpa-onnx-moonshine-base-en-int8/resolve/main/preprocess.onnx","sha256":"ffa630d395c5ccf76f5d4954be5b882df76aaf6491519ec01fd82ea7a3819fb2","size":14077290},"tokens.txt":{"source":"{huggingface}/csukuangfj/sherpa-onnx-moonshine-base-en-int8/resolve/main/tokens.txt","sha256":"1165c2aeb9f72f457a83be2d459a09054f27490acd9b41bd43794dfd25e296ea","size":436688},"uncached_decode.int8.onnx":{"source":"{huggingface}/csukuangfj/sherpa-onnx-moonshine-base-en-int8/resolve/main/uncached_decode.int8.onnx","sha256":"c01f4b35093bcac20d352d23a75a539e772964579f9d024a90e5e6f09cae9987","size":122120451}}"#;

const MOONSHINE_TINY_EN: &str = r#"{"cached_decode.int8.onnx":{"source":"{huggingface}/csukuangfj/sherpa-onnx-moonshine-tiny-en-int8/resolve/main/cached_decode.int8.onnx","sha256":"2aff28bba6a03d8dcf5c9feac45462629bae37317442299f28115ad09da773f6","size":45264830},"encode.int8.onnx":{"source":"{huggingface}/csukuangfj/sherpa-onnx-moonshine-tiny-en-int8/resolve/main/encode.int8.onnx","sha256":"8774dfba578de027ec6595c2c654a0836434489bc963a0db124a7f181f571acb","size":18249187},"preprocess.onnx":{"source":"{huggingface}/csukuangfj/sherpa-onnx-moonshine-tiny-en-int8/resolve/main/preprocess.onnx","sha256":"f33addce61a143460fe753b5ee5b7db255e5140b5b779c065b94f6c83ff0bf4e","size":6800738},"tokens.txt":{"source":"{huggingface}/csukuangfj/sherpa-onnx-moonshine-tiny-en-int8/resolve/main/tokens.txt","sha256":"1165c2aeb9f72f457a83be2d459a09054f27490acd9b41bd43794dfd25e296ea","size":436688},"uncached_decode.int8.onnx":{"source":"{huggingface}/csukuangfj/sherpa-onnx-moonshine-tiny-en-int8/resolve/main/uncached_decode.int8.onnx","sha256":"216737000dd5881a17aa043f6bbd286add33e4c3b0ae257153e2ec15438bdc41","size":53216096}}"#;

const PARAFORMER_BILINGUAL: &str = r#"{"decoder.int8.onnx":{"source":"{huggingface}/csukuangfj/sherpa-onnx-streaming-paraformer-bilingual-zh-en/resolve/main/decoder.int8.onnx","sha256":"f3cca9f77bb9d93c8fcbfb63ae617b6b1ee96818df3aa3b151c40658fe38594f","size":71664561},"encoder.int8.onnx":{"source":"{huggingface}/csukuangfj/sherpa-onnx-streaming-paraformer-bilingual-zh-en/resolve/main/encoder.int8.onnx","sha256":"81a70226a8934e6ed92aa1d4fc486b428b5398e2f2619ed4897b7294cab90e9a","size":165462184},"tokens.txt":{"source":"{huggingface}/csukuangfj/sherpa-onnx-streaming-paraformer-bilingual-zh-en/resolve/main/tokens.txt","sha256":"59aba8873a2ed1e122c25fee421e25f283b63290efbde85c1f01a853d83cb6e6","size":75756}}"#;

const PARAFORMER_MULTI_ZH: &str = r#"{"am.mvn":{"source":"{huggingface}/csukuangfj/sherpa-onnx-streaming-paraformer-trilingual-zh-cantonese-en/resolve/main/am.mvn","sha256":"29b3c740a2c0cfc6b308126d31d7f265fa2be74f3bb095cd2f143ea970896ae5","size":11203},"config.yaml":{"source":"{huggingface}/csukuangfj/sherpa-onnx-streaming-paraformer-trilingual-zh-cantonese-en/resolve/main/config.yaml","sha256":"a8f30eed55b8fe7e67aa8890409b52433f76e7bffb1f5d8965776da553bcba4c","size":62125},"decoder.int8.onnx":{"source":"{huggingface}/csukuangfj/sherpa-onnx-streaming-paraformer-trilingual-zh-cantonese-en/resolve/main/decoder.int8.onnx","sha256":"545427acf508452b7d89969be082c8128c681e3432ff43aef09f6159f4b61a7e","size":72062549},"encoder.int8.onnx":{"source":"{huggingface}/csukuangfj/sherpa-onnx-streaming-paraformer-trilingual-zh-cantonese-en/resolve/main/encoder.int8.onnx","sha256":"6047a644b41b236d9d8e89e3b94ef39d1b7037daab028131b722ca52e10b0357","size":166362800},"tokens.txt":{"source":"{huggingface}/csukuangfj/sherpa-onnx-streaming-paraformer-trilingual-zh-cantonese-en/resolve/main/tokens.txt","sha256":"45b31504211675dd52aa88f998a6f6161703a2834e86760c1cda645a22538085","size":81289}}"#;

const PARAFORMER_STREAMING: &str = r#"{"decoder.int8.onnx":{"source":"{huggingface}/csukuangfj/sherpa-onnx-streaming-paraformer-zh/resolve/main/decoder.int8.onnx","sha256":"f3cca9f77bb9d93c8fcbfb63ae617b6b1ee96818df3aa3b151c40658fe38594f","size":71664561},"encoder.int8.onnx":{"source":"{huggingface}/csukuangfj/sherpa-onnx-streaming-paraformer-zh/resolve/main/encoder.int8.onnx","sha256":"81a70226a8934e6ed92aa1d4fc486b428b5398e2f2619ed4897b7294cab90e9a","size":165462184},"tokens.txt":{"source":"{huggingface}/csukuangfj/sherpa-onnx-streaming-paraformer-zh/resolve/main/tokens.txt","sha256":"59aba8873a2ed1e122c25fee421e25f283b63290efbde85c1f01a853d83cb6e6","size":75756}}"#;

const QWEN3_ASR_06B: &str = r#"{"conv_frontend.onnx":{"source":"{huggingface}/csukuangfj2/sherpa-onnx-qwen3-asr-0.6B-int8-2026-03-25/resolve/main/conv_frontend.onnx","sha256":"d22dc4423e0940e49884e903d2ea2f7e5567c14fc1aed97e4e26d6b8f208ef9e","size":44148281},"decoder.int8.onnx":{"source":"{huggingface}/csukuangfj2/sherpa-onnx-qwen3-asr-0.6B-int8-2026-03-25/resolve/main/decoder.int8.onnx","sha256":"4f6885be5959ae26af3089d38ee7972c5fafbeeb1cf8d5e76eab6d8b61ca5771","size":755914231},"encoder.int8.onnx":{"source":"{huggingface}/csukuangfj2/sherpa-onnx-qwen3-asr-0.6B-int8-2026-03-25/resolve/main/encoder.int8.onnx","sha256":"60748d3e6744a57c9c91e1b17424a6c2990567e8adceb0783940c03ed98fa9d9","size":182491662},"tokenizer/merges.txt":{"source":"{huggingface}/csukuangfj2/sherpa-onnx-qwen3-asr-0.6B-int8-2026-03-25/resolve/main/tokenizer/merges.txt","sha256":"8831e4f1a044471340f7c0a83d7bd71306a5b867e95fd870f74d0c5308a904d5","size":1671853},"tokenizer/tokenizer_config.json":{"source":"{huggingface}/csukuangfj2/sherpa-onnx-qwen3-asr-0.6B-int8-2026-03-25/resolve/main/tokenizer/tokenizer_config.json","sha256":"4942d005604266809309cabc9f4e9cb89ce855d59b14681fdc0e1cc62ea26c4c","size":12487},"tokenizer/vocab.json":{"source":"{huggingface}/csukuangfj2/sherpa-onnx-qwen3-asr-0.6B-int8-2026-03-25/resolve/main/tokenizer/vocab.json","sha256":"ca10d7e9fb3ed18575dd1e277a2579c16d108e32f27439684afa0e10b1440910","size":2776833}}"#;

const QWEN3_ASR_17B: &str = r#"{"conv_frontend.onnx":{"source":"{huggingface}/ilmina/qwen3-asr-1.7b-sherpa-onnx/resolve/main/conv_frontend.onnx","sha256":"fa894a4ba53da6a4238f2a6ca0b09362e505d39cecbd646051b033e2e8d7e2fb","size":48080441},"decoder.int8.onnx":{"source":"{huggingface}/ilmina/qwen3-asr-1.7b-sherpa-onnx/resolve/main/decoder.int8.onnx","sha256":"c43c853fa6e97d08365cb8a5502b360b595cd43c00dc60e4d8ca7cc18cad460b","size":2037458645},"encoder.int8.onnx":{"source":"{huggingface}/ilmina/qwen3-asr-1.7b-sherpa-onnx/resolve/main/encoder.int8.onnx","sha256":"436fbd910a0c8914851e5ac1354e807be9f283d08a5da728adaa609731c41469","size":314222162},"tokenizer/merges.txt":{"source":"{huggingface}/ilmina/qwen3-asr-1.7b-sherpa-onnx/resolve/main/tokenizer/merges.txt","sha256":"8831e4f1a044471340f7c0a83d7bd71306a5b867e95fd870f74d0c5308a904d5","size":1671853},"tokenizer/tokenizer_config.json":{"source":"{huggingface}/ilmina/qwen3-asr-1.7b-sherpa-onnx/resolve/main/tokenizer/tokenizer_config.json","sha256":"4942d005604266809309cabc9f4e9cb89ce855d59b14681fdc0e1cc62ea26c4c","size":12487},"tokenizer/vocab.json":{"source":"{huggingface}/ilmina/qwen3-asr-1.7b-sherpa-onnx/resolve/main/tokenizer/vocab.json","sha256":"ca10d7e9fb3ed18575dd1e277a2579c16d108e32f27439684afa0e10b1440910","size":2776833}}"#;

const SENSEVOICE_ORIG_SMALL: &str = r#"{"am.mvn":{"source":"{huggingface}/WisemeAI/sensevoice-small-quant/resolve/main/am.mvn","sha256":"29b3c740a2c0cfc6b308126d31d7f265fa2be74f3bb095cd2f143ea970896ae5","size":11203},"config.yaml":{"source":"{huggingface}/WisemeAI/sensevoice-small-quant/resolve/main/config.yaml","sha256":"f71e239ba36705564b5bf2d2ffd07eece07b8e3f2bbf6d2c99d8df856339ac19","size":1855},"configuration.json":{"source":"{huggingface}/WisemeAI/sensevoice-small-quant/resolve/main/configuration.json","sha256":"c57f6a580d63f7465c6a22ba95847aee05a1ae1181f5abddffb943d9febda061","size":56},"gitattributes":{"source":"{huggingface}/WisemeAI/sensevoice-small-quant/resolve/main/gitattributes","sha256":"11ad7efa24975ee4b0c3c3a38ed18737f0658a5f75a0a96787b576a78a023361","size":1519},"model.onnx":{"source":"{huggingface}/WisemeAI/sensevoice-small-quant/resolve/main/model.onnx","sha256":"21dc965f689a78d1604717bf561e40d5a236087c85a95584567835750549e822","size":241216270},"tokens.json":{"source":"{huggingface}/WisemeAI/sensevoice-small-quant/resolve/main/tokens.json","sha256":"a2594fc1474e78973149cba8cd1f603ebed8c39c7decb470631f66e70ce58e97","size":352064}}"#;

const FIRERED_ASR2: &str = r#"{"model.int8.onnx":{"source":"{huggingface}/VidraAI/FireRedASR2-onnx/resolve/main/model.int8.onnx","sha256":"ca3dbabd82170110cc0b343c2890866d449984bc9cd92b9a18371ff80a81bb99","size":775861420},"tokens.txt":{"source":"{huggingface}/VidraAI/FireRedASR2-onnx/resolve/main/tokens.txt","sha256":"1bc613de2112d257e61a349c3e72d1b1a9cf19c33d3ca954197ad2171e5ea07b","size":79172}}"#;

const WHISPER_SMALL: &str = r#"{"added_tokens.json":{"source":"{huggingface}/onnx-community/whisper-small.en/resolve/main/added_tokens.json","sha256":"560be47bea388757f8d4cc185c5d82067426cbb6361e38016dd90ddc01ab203a","size":34604},"config.json":{"source":"{huggingface}/onnx-community/whisper-small.en/resolve/main/config.json","sha256":"8825c4174cb86f94d9fa67614942f8aa17bfbbdf2fae5426d4adfd0bc5893c43","size":2203},"generation_config.json":{"source":"{huggingface}/onnx-community/whisper-small.en/resolve/main/generation_config.json","sha256":"5490747ca976d6b3765280a0697d66489020c2afa6e754244d9cd093e1639331","size":1956},"merges.txt":{"source":"{huggingface}/onnx-community/whisper-small.en/resolve/main/merges.txt","sha256":"1ce1664773c50f3e0cc8842619a93edc4624525b728b188a9e0be33b7726adc5","size":456318},"normalizer.json":{"source":"{huggingface}/onnx-community/whisper-small.en/resolve/main/normalizer.json","sha256":"bf1c507dc8724ca9cf9903640dacfb69dae2f00edee4f21ceba106a7392f26dd","size":52666},"onnx/decoder_model_int8.onnx":{"source":"{huggingface}/onnx-community/whisper-small.en/resolve/main/onnx/decoder_model_int8.onnx","sha256":"a01edeca857292810e090536068afb61510bcf9a4f6c54539ae45a07ccefb32c","size":155988577},"onnx/decoder_with_past_model_int8.onnx":{"source":"{huggingface}/onnx-community/whisper-small.en/resolve/main/onnx/decoder_with_past_model_int8.onnx","sha256":"ae47a64cbac82c1772f3b9150d9f8b45badcb32a3303792f93fe950c84fef847","size":141651939},"onnx/encoder_model_int8.onnx":{"source":"{huggingface}/onnx-community/whisper-small.en/resolve/main/onnx/encoder_model_int8.onnx","sha256":"0a143c26b5aa5f549bef89a9363a56a5610a00985afe1e56443a71852bd642d4","size":92326127},"preprocessor_config.json":{"source":"{huggingface}/onnx-community/whisper-small.en/resolve/main/preprocessor_config.json","sha256":"a6a76d28c93edb273669eb9e0b0636a2bddbb1272c3261e47b7ca6dfdbac1b8d","size":339},"special_tokens_map.json":{"source":"{huggingface}/onnx-community/whisper-small.en/resolve/main/special_tokens_map.json","sha256":"98bdf3ec5b32e31575b02f64b0a32bde7c0449075d34484a7df9bdd3cdeb9fb9","size":2173},"tokenizer_config.json":{"source":"{huggingface}/onnx-community/whisper-small.en/resolve/main/tokenizer_config.json","sha256":"93879c3dccdd4b976f709acd85b44778873f30c275e67026f30ca1e4c975230c","size":282662},"tokenizer.json":{"source":"{huggingface}/onnx-community/whisper-small.en/resolve/main/tokenizer.json","sha256":"5eb60cec1e77aeeb6869a2bb5a8e01a84c3fe5d072d75369343021fe6f5310d0","size":2405679},"vocab.json":{"source":"{huggingface}/onnx-community/whisper-small.en/resolve/main/vocab.json","sha256":"f6bd25a65e4e63ca31360e9fb11c7e4f9a391a78385d640acd814092dd6eee4f","size":999186}}"#;

const ZIPFORMER: &str = r#"{"decoder.onnx":{"source":"{huggingface}/csukuangfj/sherpa-onnx-streaming-zipformer-zh-int8-2025-06-30/resolve/main/decoder.onnx","sha256":"06522ad63cec0fdf6809f4e1db9bb4f7d710c34582e3b35db62ac60eccafac7e","size":5165083},"encoder.int8.onnx":{"source":"{huggingface}/csukuangfj/sherpa-onnx-streaming-zipformer-zh-int8-2025-06-30/resolve/main/encoder.int8.onnx","sha256":"5ac51e27981bb4dab01bb9be4958453ba50c3b61c063ddda0eab23fd3671aa4f","size":161141793},"joiner.int8.onnx":{"source":"{huggingface}/csukuangfj/sherpa-onnx-streaming-zipformer-zh-int8-2025-06-30/resolve/main/joiner.int8.onnx","sha256":"b34584dc6f561089e1d747fedebb3765f2caa72c927ef54d7ca55e5ae40a814b","size":1033416},"tokens.txt":{"source":"{huggingface}/csukuangfj/sherpa-onnx-streaming-zipformer-zh-int8-2025-06-30/resolve/main/tokens.txt","sha256":"6193c7ea1c96d0d9a1e9652789b40d13a8a913b434a5451e93158f5a09fd6652","size":20628}}"#;

const ZIPFORMER_LARGE: &str = r#"{"bpe.model":{"source":"{huggingface}/csukuangfj/sherpa-onnx-streaming-zipformer-zh-xlarge-int8-2025-06-30/resolve/main/bpe.model","sha256":"867a7355801cb43939962ad757ba1cb7941b6171b5a6902772483b4e3a623377","size":263956},"decoder.onnx":{"source":"{huggingface}/csukuangfj/sherpa-onnx-streaming-zipformer-zh-xlarge-int8-2025-06-30/resolve/main/decoder.onnx","sha256":"8f9c903da2818f207304a3f30b9eeb30028e30398f333c1e95e12c97704173e6","size":8533022},"encoder.int8.onnx":{"source":"{huggingface}/csukuangfj/sherpa-onnx-streaming-zipformer-zh-xlarge-int8-2025-06-30/resolve/main/encoder.int8.onnx","sha256":"f2c543a0330e1ed0bd09c82e4ae7d3f1cbee10a15feca638fcc4f88083a36b8a","size":761133737},"joiner.int8.onnx":{"source":"{huggingface}/csukuangfj/sherpa-onnx-streaming-zipformer-zh-xlarge-int8-2025-06-30/resolve/main/joiner.int8.onnx","sha256":"f76ffce14b6ef80098cfdbce8846896ff68133970abc314eafab632f910df0d7","size":1545417},"tokens.txt":{"source":"{huggingface}/csukuangfj/sherpa-onnx-streaming-zipformer-zh-xlarge-int8-2025-06-30/resolve/main/tokens.txt","sha256":"6722bd1585f46f84456b29c3550a343a3cc375b971645773c02ed8e0b4e2405c","size":18626}}"#;

/// zipformer-small-ctc（builtin 兜底引擎，27M，首次启动下载）。
/// repo: csukuangfj/sherpa-onnx-streaming-zipformer-small-ctc-zh-int8-2025-04-01
/// sha256 + size 由本地实际文件计算（2026-07-22）。
const ZIPFORMER_SMALL_CTC: &str = r#"{"bbpe.model":{"source":"{huggingface}/csukuangfj/sherpa-onnx-streaming-zipformer-small-ctc-zh-int8-2025-04-01/resolve/main/bbpe.model","sha256":"503204e0690eff065e30d0e01898c9ab06d0e6dc376a741eb6846198f95b2f82","size":255180},"model.int8.onnx":{"source":"{huggingface}/csukuangfj/sherpa-onnx-streaming-zipformer-small-ctc-zh-int8-2025-04-01/resolve/main/model.int8.onnx","sha256":"68c9c943840f7d9cf3e8a4970ba50f404feb5277f611fa82b7e72267786fa84a","size":26342340},"tokens.txt":{"source":"{huggingface}/csukuangfj/sherpa-onnx-streaming-zipformer-small-ctc-zh-int8-2025-04-01/resolve/main/tokens.txt","sha256":"6fed8c6c248516f38e7faa19404b57413e8ce259f1cbc1fa4aebc86eac32fdfd","size":13366}}"#;

const OPUS_MT: &str = r#"{"zh-en/config.json":{"source":"{huggingface}/Xenova/opus-mt-zh-en/resolve/main/config.json","sha256":"293d318fce41dbf04114eac45037bb88a32d7c4ee21011a75e24a8b98ca45ad1","size":1389},"zh-en/generation_config.json":{"source":"{huggingface}/Xenova/opus-mt-zh-en/resolve/main/generation_config.json","sha256":"8dc29fef0fe82109f94ef3c2e6ea6bded3215d357b226c34cf7b4630726766c9","size":293},"zh-en/special_tokens_map.json":{"source":"{huggingface}/Xenova/opus-mt-zh-en/resolve/main/special_tokens_map.json","sha256":"5e4d1f5e759d74cb1c2fe1d165cfc62b5237aa904de759380cd6f43042eec723","size":74},"zh-en/tokenizer.json":{"source":"{huggingface}/Xenova/opus-mt-zh-en/resolve/main/tokenizer.json","sha256":"b306d0301cf280bfd647d7067b5ade2a97b987e6d678df110703c002433643ff","size":6381339},"zh-en/tokenizer_config.json":{"source":"{huggingface}/Xenova/opus-mt-zh-en/resolve/main/tokenizer_config.json","sha256":"08849acc0a539c4749d8665e9d6217735503a97871ccebeea8a762d5fba1acf7","size":282},"zh-en/vocab.json":{"source":"{huggingface}/Xenova/opus-mt-zh-en/resolve/main/vocab.json","sha256":"08a119a1defd522fa047cb5e3bfe3e89633e96caa38ced0dc9cee7ef1021a011","size":1747906},"zh-en/onnx/decoder_model_int8.onnx":{"source":"{huggingface}/Xenova/opus-mt-zh-en/resolve/main/onnx/decoder_model_int8.onnx","sha256":"624c24eed858e55ae1564db8d69e9ad10ccb3328fa18d8909a3f1494078effb4","size":192658470},"zh-en/onnx/encoder_model_int8.onnx":{"source":"{huggingface}/Xenova/opus-mt-zh-en/resolve/main/onnx/encoder_model_int8.onnx","sha256":"c285f52c59ae2dee7778050a805ce6af9d6e1579edd9d36e92cd68b58f61ca70","size":52726552},"en-zh/config.json":{"source":"{huggingface}/Xenova/opus-mt-en-zh/resolve/main/config.json","sha256":"4727d1229a04f95bf6f39abf949d8080615433d99d6ebd85f81c09edd247d5fa","size":1503},"en-zh/generation_config.json":{"source":"{huggingface}/Xenova/opus-mt-en-zh/resolve/main/generation_config.json","sha256":"b743baabb7da4c1a2f19fe558bd6b4c0c7c3b0762fcb5ca7a48fe5a2c2219803","size":293},"en-zh/special_tokens_map.json":{"source":"{huggingface}/Xenova/opus-mt-en-zh/resolve/main/special_tokens_map.json","sha256":"5e4d1f5e759d74cb1c2fe1d165cfc62b5237aa904de759380cd6f43042eec723","size":74},"en-zh/tokenizer.json":{"source":"{huggingface}/Xenova/opus-mt-en-zh/resolve/main/tokenizer.json","sha256":"d0c7da27056e8f42adce9e76d8e792e5daa64e15f5acd2e7aabf0121877dd4c1","size":6380952},"en-zh/tokenizer_config.json":{"source":"{huggingface}/Xenova/opus-mt-en-zh/resolve/main/tokenizer_config.json","sha256":"a914596e6bff113a8428d4793b586da87cd0b95697a0e72aba90cc1d95858481","size":282},"en-zh/vocab.json":{"source":"{huggingface}/Xenova/opus-mt-en-zh/resolve/main/vocab.json","sha256":"22c957348eed495ee925afc40a36da3e387c8a34a734c8486967c2dca271613e","size":1747795},"en-zh/onnx/decoder_model_int8.onnx":{"source":"{huggingface}/Xenova/opus-mt-en-zh/resolve/main/onnx/decoder_model_int8.onnx","sha256":"8eb245366039256e29a21c73d6438f7a0878866d570b4e2b8fff5d88ec9bac5e","size":192658471},"en-zh/onnx/encoder_model_int8.onnx":{"source":"{huggingface}/Xenova/opus-mt-en-zh/resolve/main/onnx/encoder_model_int8.onnx","sha256":"262c0319bd0d8a6570f287211bf962035788954f20697e022cd60aaf62209b9c","size":52726553}}"#;

const M2M100_418M: &str = r#"{"config.json":{"source":"{huggingface}/lazycodepersona/m2m100_418m/resolve/main/config.json","sha256":"1dbdf77ddc7809acd4c54ccf0eab46f840b40174afb1b6f6de8787244e832938","size":908},"generation_config.json":{"source":"{huggingface}/lazycodepersona/m2m100_418m/resolve/main/generation_config.json","sha256":"722210dd0bee7bef4e8e7f9a8574d8c56a2dfff723d73f390ce67892740b9009","size":233},"onnx/decoder_model_quantized.onnx":{"source":"{huggingface}/lazycodepersona/m2m100_418m/resolve/main/onnx/decoder_model_quantized.onnx","sha256":"6015e31c8976659aedb06058c4dadf0f400d087a3f9830f838e68f220d79bcb6","size":339181945},"onnx/encoder_model_quantized.onnx":{"source":"{huggingface}/lazycodepersona/m2m100_418m/resolve/main/onnx/encoder_model_quantized.onnx","sha256":"13a94e354a9140764eb81102d77d3ec6952d796e6f113c651eeb3c3443da0386","size":287856370},"special_tokens_map.json":{"source":"{huggingface}/lazycodepersona/m2m100_418m/resolve/main/special_tokens_map.json","sha256":"009ea667e0ca903c10dac22cf7ae3a3a0b173ff33f8c64154fddd8c043805622","size":1559},"tokenizer_config.json":{"source":"{huggingface}/lazycodepersona/m2m100_418m/resolve/main/tokenizer_config.json","sha256":"bacfd4b9da25a61e01f17abe660465f616c9a1a3f5e23ab9ad3326c3788f2d9f","size":1813},"tokenizer.json":{"source":"{huggingface}/lazycodepersona/m2m100_418m/resolve/main/tokenizer.json","sha256":"df0873cc1c747fb4003a65e4e1e676ac4ebc98171bc351f1a0a5db2b461cf7db","size":7964703},"vocab.json":{"source":"{huggingface}/lazycodepersona/m2m100_418m/resolve/main/vocab.json","sha256":"b6e77e474aeea8f441363aca7614317c06381f3eacfe10fb9856d5081d1074cc","size":3708092}}"#;

const OCR_V6_SMALL: &str = r#"{"cls.onnx":{"source":"{huggingface}/bukuroo/PPOCRv5-ONNX/resolve/main/ppocrv5-cls.onnx","sha256":"f4bb53707100c5f3d59ba834eb05bb400369f20aed35d4b26807b1bfadd2a70e","size":582663},"det.onnx":{"source":"{huggingface}/PaddlePaddle/PP-OCRv6_small_det_onnx/resolve/main/inference.onnx","sha256":"d73e0058b7a8086bbd57f3d10b8bcd4ff95363f67e06e2762b5e814fe9c9410e","size":9880512},"rec.onnx":{"source":"{huggingface}/PaddlePaddle/PP-OCRv6_small_rec_onnx/resolve/main/inference.onnx","sha256":"5435fd747c9e0efe15a96d0b378d5bd1579e492ed8fd80edf08f30d02fa24634","size":21159378},"keys_v6.txt":{"source":"{github}/PaddlePaddle/PaddleOCR/raw/main/ppocr/utils/dict/ppocrv6_dict.txt","sha256":"b5f2bfe2bdd9448429e3e82b51c789775d9b42f2403d082b00662eb77e401c5d","size":74947},"keys.txt":{"source":"{github}/PaddlePaddle/PaddleOCR/raw/main/ppocr/utils/dict/ppocrv6_dict.txt","sha256":"b5f2bfe2bdd9448429e3e82b51c789775d9b42f2403d082b00662eb77e401c5d","size":74947}}"#;

const OCR_V5: &str = r#"{"cls.onnx":{"source":"{huggingface}/bukuroo/PPOCRv5-ONNX/resolve/main/ppocrv5-cls.onnx","sha256":"f4bb53707100c5f3d59ba834eb05bb400369f20aed35d4b26807b1bfadd2a70e","size":582663},"det.onnx":{"source":"{huggingface}/bukuroo/PPOCRv5-ONNX/resolve/main/ppocrv5-mobile-det.onnx","sha256":"d7fe3ea74652890722c0f4d02458b7261d9f5ae6c92904d05707c9eb155c7924","size":4748769},"rec.onnx":{"source":"{huggingface}/bukuroo/PPOCRv5-ONNX/resolve/main/ppocrv5-mobile-rec.onnx","sha256":"bf66820f48fa99f779974c4df78e5274a9d8e0458c4137e8c5357e40e2c3faf2","size":16517247},"keys.txt":{"source":"{huggingface}/bukuroo/PPOCRv5-ONNX/resolve/main/ppocrv5_dict.txt","sha256":"1ea29636956177e400af712d9782e7693f3fb25f98617bed10479d2965a836fd","size":92395}}"#;

#[cfg(test)]
mod tests {
    use super::*;

    /// 每个返回的 manifest JSON 应是有效 JSON，且每个条目含 source/sha256/size 三字段。
    fn validate_manifest(name: &str, json: &str) {
        let parsed: serde_json::Value = serde_json::from_str(json)
            .unwrap_or_else(|e| panic!("{} manifest 不是有效 JSON: {e}", name));
        let obj = parsed.as_object()
            .unwrap_or_else(|| panic!("{} manifest 顶层应是 object", name));
        assert!(!obj.is_empty(), "{} manifest 不应为空", name);
        for (path, meta) in obj {
            let source = meta.get("source").unwrap_or_else(|| panic!(
                "{} manifest 条目 '{}' 缺 source 字段", name, path
            )).as_str().unwrap_or_else(|| panic!(
                "{} manifest 条目 '{}' source 不是 string", name, path
            ));
            assert!(!source.is_empty(), "{} manifest 条目 '{}' source 不应为空", name, path);
            assert!(
                source.contains("/resolve/main/") || source.contains("/raw/main/"),
                "{} manifest 条目 '{}' source 应含 resolve/main 或 raw/main: {}",
                name, path, source
            );
            let sha = meta.get("sha256").unwrap_or_else(|| panic!(
                "{} manifest 条目 '{}' 缺 sha256 字段", name, path
            )).as_str().unwrap_or_else(|| panic!(
                "{} manifest 条目 '{}' sha256 不是 string", name, path
            ));
            assert_eq!(sha.len(), 64, "{} manifest 条目 '{}' sha256 应为 64 字符 hex", name, path);
            let size = meta.get("size").unwrap_or_else(|| panic!(
                "{} manifest 条目 '{}' 缺 size 字段", name, path
            )).as_u64().unwrap_or_else(|| panic!(
                "{} manifest 条目 '{}' size 不是 u64", name, path
            ));
            assert!(size > 0, "{} manifest 条目 '{}' size 应 > 0", name, path);
        }
    }

    #[test]
    fn asr_manifests_all_valid_json() {
        for name in [
            "moonshine-base-en", "moonshine-tiny-en",
            "paraformer-bilingual", "paraformer-multi-zh",
            "paraformer-streaming", "paraformer-zh",
            "qwen3-asr-0.6B", "qwen3-asr-1.7B",
            "sensevoice-orig-small", "firered-asr2",
            "whisper-small", "zipformer", "zipformer-large",
        ] {
            let json = asr_manifest(name)
                .unwrap_or_else(|| panic!("asr_manifest('{}') 返回 None", name));
            validate_manifest(name, json);
        }
    }

    #[test]
    fn translate_manifests_all_valid_json() {
        for name in ["opus-mt", "m2m100-418M"] {
            let json = translate_manifest(name)
                .unwrap_or_else(|| panic!("translate_manifest('{}') 返回 None", name));
            validate_manifest(name, json);
        }
    }

    #[test]
    fn ocr_manifests_all_valid_json() {
        for name in ["PP-OCRv6-small", "PP-OCRv5"] {
            let json = ocr_manifest(name)
                .unwrap_or_else(|| panic!("ocr_manifest('{}') 返回 None", name));
            validate_manifest(name, json);
        }
    }

    #[test]
    fn asr_manifest_returns_none_for_unknown() {
        assert!(asr_manifest("nonexistent-model").is_none());
    }

    #[test]
    fn translate_manifest_returns_none_for_unknown() {
        assert!(translate_manifest("nonexistent-model").is_none());
    }

    #[test]
    fn ocr_manifest_returns_none_for_unknown() {
        assert!(ocr_manifest("nonexistent-model").is_none());
    }

    /// paraformer-zh 与 paraformer-streaming 共用同一 manifest（同 repo）。
    #[test]
    fn paraformer_zh_shares_streaming_manifest() {
        assert_eq!(
            asr_manifest("paraformer-zh"),
            asr_manifest("paraformer-streaming"),
        );
    }

    /// opus-mt manifest 应含 zh-en 和 en-zh 两个方向的文件。
    #[test]
    fn opus_mt_manifest_has_both_directions() {
        let json = translate_manifest("opus-mt").unwrap();
        let parsed: serde_json::Value = serde_json::from_str(json).unwrap();
        let obj = parsed.as_object().unwrap();
        let has_zh_en = obj.keys().any(|k| k.starts_with("zh-en/"));
        let has_en_zh = obj.keys().any(|k| k.starts_with("en-zh/"));
        assert!(has_zh_en, "opus-mt manifest 应含 zh-en/ 前缀文件");
        assert!(has_en_zh, "opus-mt manifest 应含 en-zh/ 前缀文件");
    }

    /// PP-OCRv6-small manifest 的文件来自多个来源（HuggingFace + GitHub）。
    #[test]
    fn ocr_v6_manifest_has_multi_source() {
        let json = ocr_manifest("PP-OCRv6-small").unwrap();
        let parsed: serde_json::Value = serde_json::from_str(json).unwrap();
        let obj = parsed.as_object().unwrap();
        let has_hf = obj.values().any(|v| {
            v.get("source").unwrap().as_str().unwrap().contains("huggingface")
        });
        let has_github = obj.values().any(|v| {
            v.get("source").unwrap().as_str().unwrap().contains("github")
        });
        assert!(has_hf, "PP-OCRv6-small manifest 应有 HuggingFace 来源文件");
        assert!(has_github, "PP-OCRv6-small manifest 应有 GitHub 来源文件 (keys_v6.txt)");
    }

    /// 所有 manifest 的 source URL 都应使用 {*} 模板变量。
    #[test]
    fn asr_manifests_use_env_template() {
        let json = asr_manifest("whisper-small").unwrap();
        assert!(
            json.contains("{huggingface}"),
            "whisper-small manifest source 应含 {{huggingface}} 模板"
        );
    }
}
