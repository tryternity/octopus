# mengzi-t5-base-chinese-correction → INT8 ONNX 导出指南

脚本：[`export_mengzi_t5_onnx.py`](./export_mengzi_t5_onnx.py)

## 已验证的流程

端到端跑通，包含四个阶段：

1. **FP32 导出**：用 `optimum` 分离导出 encoder / decoder / decoder_with_past（含 KV cache）
2. **INT8 动态量化**：对三个有效子模型做 `quantize_dynamic`，压缩到 FP32 的 36–42%
3. **加载验证**：所有 `*_int8.onnx` 可正常创建 ONNX Runtime 推理会话
4. **推理测试**：`--test` 实测输出正确（中文纠错生效）

## 关键决策（踩坑后修正）

### 1. task 必须用 `text2text-generation-with-past`

最初用 `text2text-generation`，导出后缺少 `decoder_with_past_model.onnx`。

**根因**（`optimum/exporters/utils.py:206`）：`get_encoder_decoder_models_for_export` 根据 `config.use_past` 决定是否导出 KV cache 版本。`use_past` 由 task 名后缀控制：

| task | use_past | 导出的子模型 |
|------|----------|-------------|
| `text2text-generation` | False | encoder + decoder（无 KV cache） |
| `text2text-generation-with-past` | True | encoder + decoder + **decoder_with_past** |

缺少 `decoder_with_past_model.onnx` 时，自回归生成需每步重算全部历史 attention，退化成 O(n²)，实用性极差。

### 2. 跳过 `decoder_model_merged.onnx` 的量化

`-with-past` task 会额外产出 `decoder_model_merged.onnx`（If-分支合并版，把有无 KV cache 两种情况合到一个图里）。实测动态量化对它**无效**——量化后体积 100%（不降反升）。

**原因**：`quantize_dynamic` 无法穿透 ONNX `If` 分支节点内部的 MatMul，分支里的权重被跳过。

**处理**：脚本自动跳过 merged 文件，只量化分离版的三个子模型。推理时用分开的 `decoder_model` + `decoder_with_past_model` 即可（这也是 ONNX Runtime 的标准做法）。

### 3. opset 18

T5 的 `gated-gelu` + KV cache 需要 opset >= 14。optimum 2.1 对 t5 推荐最低 opset 18，低于此会打 warning 说导出可能失败或模型次优。脚本默认 opset 18。

## INT8 产物体积

| 文件 | FP32 | INT8 | 压缩比 |
|------|------|------|--------|
| encoder_model | 439 MB | 184 MB | 42% |
| decoder_model | 651 MB | 237 MB | 36% |
| decoder_with_past_model | 594 MB | 223 MB | 38% |
| **合计（推理用）** | **1684 MB** | **644 MB** | **38%** |
| decoder_model_merged（跳过量化） | 621 MB | — | 保留 FP32 |

## Rust 集成提示

INT8 推理需要加载三个 ONNX 文件：

- `encoder_model_int8.onnx` — 编码输入文本（一次计算）
- `decoder_model_int8.onnx` — 生成首 token（无 KV cache 输入）
- `decoder_with_past_model_int8.onnx` — 生成后续 token（带 KV cache 输入/输出，自回归循环用）

tokenizer 用导出目录里的 `spiece.model` 或 `tokenizer.json`（SentencePiece / T5 Fast Tokenizer）。

## 环境依赖

```bash
pip install "optimum[onnxruntime]" onnx
```

**⚠️ 环境变更告知**：安装 `optimum[onnxruntime]` 会影响全局 Python 环境。实测变更：

| 包 | 安装前 | 安装后 |
|----|--------|--------|
| transformers | 5.4.0 | 4.57.6（降级） |
| onnxruntime | 1.16.3 | 1.27.0（升级） |
| optimum | — | 2.1.0 |
| optimum-onnx | — | 0.1.0 |

若其他工具（如 `mlx-*`）依赖 transformers 5.x，运行 `pip install transformers==5.4.0` 恢复——注意这会移除 optimum 的 ONNX 导出能力。建议用 venv 隔离。

## 用法

```bash
# 默认：用 HF model id 加载，输出到 ./mengzi-t5-onnx/
python3 scripts/export_mengzi_t5_onnx.py

# 指定本地模型路径 & 输出目录（推荐，避免重新下载）
python3 scripts/export_mengzi_t5_onnx.py -m ~/.cache/huggingface/hub/models--shibing624--mengzi-t5-base-chinese-correction/snapshots/<hash>/ -o /path/to/out

# 完整验证（含端到端推理测试）
python3 scripts/export_mengzi_t5_onnx.py -o /path/to/out --test

# 只导出 FP32，不做 INT8 量化
python3 scripts/export_mengzi_t5_onnx.py --fp32-only

# 额外导出一份 FP16（精度高于 INT8，体积约为 FP32 的一半）
python3 scripts/export_mengzi_t5_onnx.py --fp16
```
