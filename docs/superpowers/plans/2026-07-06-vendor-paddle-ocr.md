# Vendor paddle-ocr-rs 实施计划

**日期**：2026-07-06
**spec**：`docs/superpowers/specs/2026-07-06-vendor-paddle-ocr-design.md`

---

## Task 1：创建 crates/paddle-ocr 骨架
- [x] 在 workspace Cargo.toml 加 member `crates/paddle-ocr`
- [x] 创建 `crates/paddle-ocr/Cargo.toml`（精简依赖，ort rc.12）
- [x] 创建空 `crates/paddle-ocr/src/lib.rs`
- **验证**：`cargo check -p octopus-paddle-ocr` 通过

## Task 2：拷贝核心源码
- [x] 拷贝 `det/` `rec/` `cls/` `pipeline/` `runtime/` `vision/` 目录
- [x] 拷贝 `config.rs` `error.rs` `types.rs`
- **不拷贝**：`bin/` `input/` `model_store.rs` `model_registry.rs` `output/` `pipeline/compat_rapidocr/`
- **验证**：编译报错（预期，因依赖未精简完）

## Task 3：精简依赖与修改源码
- [x] `lib.rs`：精简导出，删 input/output/compat/model_store re-export
- [x] `error.rs`：删 `Reqwest`/`Yaml` variant
- [x] `config.rs`：删 serde_yaml 相关
- [x] `pipeline/config.rs`：删 serde_yaml 相关
- [x] `vision/backend.rs`：硬编码 PureRust，删 opencv 分支
- [x] `pipeline/rapid_ocr.rs`：入口接受内存图像而非 OcrInput
- **验证**：`cargo check -p octopus-paddle-ocr` 通过

## Task 4：更新 crates/ocr 封装层
- [x] `crates/ocr/Cargo.toml`：ocr-rs → octopus-paddle-ocr (path)
- [x] `crates/ocr/src/engine.rs`：替换内部引擎
- [x] `crates/ocr/src/model.rs`：模型路径 .mnn → .onnx
- **验证**：`cargo build -p octopus-ocr` 通过

## Task 5：构建验证
- [x] `cargo build -p octopus-paddle-ocr`
- [x] `cargo build -p octopus-ocr`
- [x] `cargo build -p octopus-desktop --features embedded`
- [x] `cargo clippy` 零 warning

## Task 6：文档同步
- [x] 更新 `docs/architecture.md`（ocr 模块描述：MNN→ONNX、PP-OCRv6→v5、.mnn→.onnx）
- [x] 更新本 plan 状态

## Task 7：实施中发现的关键 bug 与后处理（增补）
- [x] `read_character_file` trim() 误删全角空格 U+3000 → CTC 偏移 1 位（改 `strip_suffix('\r')`）
- [x] `merge_same_line_blocks`：det 同行多框合并 + 水平间隙补空格
- [x] `segment_english_words`：英文词库贪心分词（v5 需要，v6 跳过）
- [x] ort rc.10→rc.12 API 适配（outputs/inputs 方法、Builder map_err、inputs! 宏、ndarray 0.17）
- [x] ort 依赖 `download-binaries` feature（不能用 `default-features = false`）
- [x] PP-OCRv5 + PP-OCRv6-small 模型部署 + e2e 验证通过

## Task 8：MNN 残留清理（增补）
- [x] 删除 `crates/ocr/mnn-prebuilt/`（含 7.3MB tarball + README）
- [x] 删除 `crates/ocr/tests/ocr_smoke.rs` + `ocr_concurrent_smoke.rs`（MNN 专属测试）
- [x] `run-octopus.sh` 移除 `seed_mnn_prebuilt` 函数 + 重试逻辑
- [x] 删除 `~/.octopus/models/ocr/PP-OCRv6-small/*.mnn`（旧 MNN 模型）
- [x] 词库精简 `words_alpha.txt`（370K 词/4MB）→ `words_common.txt`（17.7K 词/168KB）
- [x] opencv-backend 死代码保留在 `#[cfg]` 门控下（安全、零编译开销、零 warning；空 feature `opencv-backend = []` 在 Cargo.toml 消除 check-cfg warning）
