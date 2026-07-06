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
