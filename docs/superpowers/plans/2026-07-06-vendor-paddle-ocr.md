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
- [x] ~~opencv-backend 死代码保留在 `#[cfg]` 门控下~~ → 已在 Task 9 中彻底清理

## Task 9：opencv 死代码深度清理（2026-07-06 增补）
- [x] `config.rs`：简化 `VisionBackend` enum，移除 `cfg_attr` 守卫，修复 `is_supported` 直接返回 `false`
- [x] `vision/backend.rs`：简化 `resolve_backend_strict`，移除所有 `#[cfg]` 守卫，`OpenCv` 直接返回 `Err`
- [x] `vision/image_backend.rs`：移除 opencv resize/rotate 函数 + dispatch 包装，`OpenCv` match 臂改为 `unreachable!()`
- [x] `vision/resize.rs`：移除 opencv 比对测试模块（`#[cfg(all(test, feature = "opencv-backend"))]`）
- [x] `vision/rotate_crop.rs`：移除 `rotate_crop_image_opencv` + dispatch 函数 + opencv 测试模块
- [x] `rec/word_boxes.rs`：移除 `reverse_rotate_crop_image_opencv` 函数，简化测试 cfg 守卫
- [x] `rec/preprocess.rs`：展开测试中的 `#[cfg(not(feature = "opencv-backend"))]` 守卫
- [x] `det/postprocess/mod.rs`：移除 `run_with_opencv`、`boxes_from_bitmap_opencv`、`mini_box_from_rotated_rect_opencv`、`mini_box_from_contour_opencv`、`mini_box_from_points_opencv`、`pred_view_to_mat`、`box_score_fast_opencv`、`find_contours_from_mask_opencv`、`contour_score_opencv`、`min_area_rect_from_points_opencv`；清理 `process_contour_candidate_pure` 中 opencv/pure 交织分支（`pred_mat` 参数、opencv score 回退逻辑、mini_box 二次覆盖）
- [x] `Cargo.toml`：删除 `[features] opencv-backend = []`
- **保留**：算法名函数（`sklansky_like_opencv`、`convex_hull_like_opencv`、`unclip_polygon_like_opencv_db`）是纯 Rust 实现，非 opencv 调用
- **验证**：`cargo build -p octopus-paddle-ocr`（零 warning）+ `cargo test -p octopus-paddle-ocr`（37 passed）+ `cargo build -p octopus-desktop --features embedded`（零 warning）+ `cargo test -p octopus-ocr`（2 passed）
- **结果**：9 文件，-1055/+47 行（净删除 1008 行死代码）

## Task 10：删除 VisionBackend enum（2026-07-06 增补）
- [x] 评估影响范围：`crates/ocr` + `crates/desktop` 零引用 VisionBackend，确认完全内部类型
- [x] `config.rs`：删除 `VisionBackend` enum + `RuntimeConfig.vision_backend` 字段
- [x] `vision/backend.rs`：清空（删除 `resolve_backend_strict` / `resolve_backend_or_pure_rust` / `OPENCV_BACKEND_DISABLED_MESSAGE` / `default_backend`）
- [x] `vision/image_backend.rs`：移除 `backend` 参数，直接调用 pure rust 实现
- [x] `vision/rotate_crop.rs`：移除 `backend` 参数 + `rotate_crop_image_with_resolved_backend` 中间层
- [x] `det/preprocess.rs`：移除 `vision_backend` 字段 + match 分支，直接内联 pure rust 路径
- [x] `det/postprocess/mod.rs`：移除 `vision_backend` 字段 + `resolve_backend_or_pure_rust` 调用，直接内联 `run_pure`
- [x] `det/detector.rs`：移除 `resolve_backend_strict` 调用 + `vision_backend` 字段传递
- [x] `cls/classifier.rs`：移除 `vision_backend` 字段 + `resolve_backend_strict` 调用
- [x] `cls/preprocess.rs`：移除所有 `_with_backend` 参数（`backend` 始终为 PureRust）
- [x] `rec/preprocess.rs`：移除所有 `_with_backend` 参数 + `resolve_backend_strict` 调用
- [x] `rec/word_boxes.rs`：移除 `compute_word_boxes_with_backend` → 合并为 `compute_word_boxes`，删除 `reverse_rotate_crop_image_with_backend`
- [x] `rec/recognizer.rs`：移除 `vision_backend` 字段 + `resolve_backend_strict` 调用
- [x] `pipeline/image_ops.rs`：移除所有 `backend` 参数，删除多余的 `resize_image` 包装函数
- [x] `pipeline/rapid_ocr.rs`：移除 `preprocessing_backend` 选择逻辑，`prepare_image` 移除 `use_det` 参数
- [x] `lib.rs`：移除 `VisionBackend` re-export
- **验证**：`cargo build -p octopus-paddle-ocr`（零 warning）+ `cargo test -p octopus-paddle-ocr`（32 passed）+ `cargo build -p octopus-desktop --features embedded`（零 warning）

## Task 11：后续功能审计修复（2026-07-07）

vendor 合入 main 后对核心计算逻辑的独立审计修复。

- [x] `det/postprocess/filter.rs`：`sort_boxes_like_python` 由固定 `i+1` 改 **j+1 相邻交换**（复刻 PaddleOCR `predict_system.py` 的 `sorted_boxes`），同行 N≥3 逆序框排序完全（`d5c07a5`）
- [x] `det/postprocess/tests.rs`：两个回归测试断言对齐官方 j+1 输出（cross_row `[A,B,C]` / same_line `[C,B,A]`），改回 i+1 会失败（`d5c07a5`）
- [x] `ocr/src/engine.rs`：长图切分去重由「文本逐字相等」改为**坐标去重** `drop_overlapped_blocks(covered_until_y)`（`510d475`）
- [x] `pipeline/rapid_ocr.rs`：`filter_by_text_score_for_full` 保留行恒 push `word_line`，维护 word_boxes 与行 1:1 对齐（`510d475`）
- [x] `rec/decode.rs`：`get_word_info` 加 `debug_assert_eq!(chars.len(), valid_col.len())` 守护字符级 CTC 字典假设（`510d475`）
- [x] `rec/decode.rs` + `det/postprocess/box_score.rs`：行优先假设处 `as_slice_memory_order` → `as_slice`，F-contiguous 安全降级（`5cd793e`）
- **验证**：`cargo test -p octopus-paddle-ocr`（45 passed）
