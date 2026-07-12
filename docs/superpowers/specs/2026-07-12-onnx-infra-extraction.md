# onnx-infra — ONNX 推理基础设施抽取（公共 crate）

> 抽取 asr-local 和 translation 共享的 ONNX 基础设施到独立 crate。

## 抽取内容

从 `crates/asr-local/src/config.rs` 抽取以下函数到 `crates/onnx-infra/`：

| 函数 | 当前位置 | 依赖 |
|------|---------|------|
| `find_hf_cache(source)` | config.rs:56 | HOME 环境变量 |
| `find_latest_snapshot(model_dir)` | config.rs:138 | 纯文件系统 |
| `resolve_local_in(source, home)` | config.rs:90 | octopus_config_home() |
| `resolve_model_dir(source)` | config.rs:114 | 调用上面三个 |
| `find_onnx_dir(hf_path)` | config.rs:74 | 纯文件系统 |
| `apply_session_acceleration(builder)` | config.rs:512 | AppConfig（asr_hardware_accelerated）|

**注意**：`apply_session_acceleration` 当前含 ASR 特有逻辑（`resolve_engine_category` 检查 qwen3-asr 跳 CoreML）。抽取时做 generic 化——接受一个 `skip_coreml: bool` 参数，ASR 传 `true`（qwen3-asr），翻译传 `false`。

## crate 结构

```
crates/onnx-infra/
├── Cargo.toml
└── src/
    ├── lib.rs       # re-exports
    ├── paths.rs     # 模型路径查找（find_hf_cache, resolve_model_dir, ...）
    └── session.rs   # apply_session_acceleration（generic 化）
```

## asr-local 改动

`asr-local/src/config.rs` 中的函数改为从 `onnx-infra` re-export（或直接引用），避免破坏现有调用。

## Cargo 依赖链变化

```
之前: infra ← asr-local ← desktop
之后: infra ← onnx-infra ← (asr-local, translation) ← desktop
```
