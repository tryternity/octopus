# 模型下载统一 Manifest 设计

> 2026-07-14 · 模型下载功能重构

## 1. 背景与问题

### 1.1 现状

模型下载功能分散在三套独立机制中：

| Domain | 下载方式 | 清单来源 | DB 状态 | 本地路径 |
|--------|---------|---------|---------|---------|
| ASR（12+ 模型） | HF repo + glob 过滤 + bootstrap | `secret_key` 存 sha256 manifest（无 source URL） | 有 seed | `~/.octopus/models/{repo}/` 或 HF cache |
| 翻译（2 模型） | HF repo + glob 过滤 | **代码常量写死**（`KNOWN_MODELS`） | **无 DB seed** | `~/.octopus/models/translate/opus-mt/` 或 HF cache |
| OCR（2 模型） | 手动软链 | 无 manifest | seed 指向旧 GitHub MNN URL | `~/.octopus/models/ocr/{name}/` |

### 1.2 核心问题

1. **翻译模型不在 DB 中**：`crates/translation/src/discovery.rs:22-26` 的 `KNOWN_MODELS` 常量是唯一真相源，DB 的 `models` 表零 `domain='translate'` 行。
2. **manifest 缺 source URL**：ASR 的 `bootstrap_manifest` 只算 sha256 + 文件名，不记录下载来源 URL。
3. **下载逻辑不统一**：ASR 走 `resolve_tasks`（HF API 解析 + glob），翻译也走同一路径但模型列表写死，OCR 完全没有下载 API。
4. **ASR 路径不统一**：ASR 直接用 HF repo 做 path（`~/.octopus/models/{owner}/{name}/`），OCR/翻译用 `{domain}/{name}/`。
5. **`#[tauri::command]` 标错位置**（`model_commands.rs:79-80`）。
6. **Desktop 下载忽略 mirror 配置**。

## 2. 设计目标

**统一所有本地模型的下载清单为声明式 manifest + 统一路径结构**：

- 一个模型 = 一份文件清单 JSON（存储在 `secret_key` 字段）
- 每个文件声明 `source`（下载 URL）+ `sha256` + `size`
- 一个模型可从多个来源下载文件（opus-mt 双方向 / PP-OCRv6-small 三 repo）
- `{env.huggingface}` / `{env.github}` 模板变量支持镜像切换
- 所有 domain（ASR / 翻译 / OCR）走同一下载逻辑
- `~/.octopus/models/{domain}/{model_name}/` 统一路径结构

## 3. 统一路径结构

### 3.1 目录约定

```
~/.octopus/models/
├── asr/
│   ├── whisper-small/          # model_name
│   ├── paraformer-zh/
│   ├── zipformer/
│   └── ...
├── translate/
│   ├── opus-mt/                # 含 zh-en/ en-zh/ 子目录
│   │   ├── zh-en/onnx/*.onnx
│   │   └── en-zh/onnx/*.onnx
│   └── m2m100-418M/
│       └── onnx/*.onnx
└── ocr/
    ├── PP-OCRv5/               # 已有，不变
    └── PP-OCRv6-small/         # 已有，不变
```

DB `source` 字段 = **本地路径标识**（相对于 `~/.octopus/models/`）：
- ASR: `asr/whisper-small`
- 翻译: `translate/opus-mt` / `translate/m2m100-418M`
- OCR: `ocr/PP-OCRv6-small`

`resolve_model_dir(source)` 在 level 3 查找时拼为 `~/.octopus/models/{source}/`，天然适配。

### 3.2 迁移策略

现有 ASR 模型在 HF cache 中，新路径 `~/.octopus/models/asr/{name}/` 无文件。迁移策略：

1. **首次下载/校验时 bootstrap**：从 HF cache 读已有文件 + 计算 sha256 + 生成 manifest → 写入 secret_key → 软链/复制到新路径
2. **resolve_model_dir 加新路径查找**：`~/.octopus/models/asr/{name}/`（manifest 下载路径）→ 旧路径 `~/.octopus/models/{repo}/` → HF cache fallback

## 4. Manifest 格式

### 4.1 统一扁平格式

```json
{
  "<相对路径>": {
    "source": "<下载 URL，支持 {env.*} 模板>",
    "sha256": "<hex 或空字符串>",
    "size": <字节数 或 0>
  }
}
```

**设计决策：扁平 key = 目标相对路径。** 不用嵌套对象（用户初始提案），因为 key 直接 = 目标路径，遍历即得完整文件列表，无需递归。

### 4.2 模板变量

source URL 中 `{env.huggingface}` / `{env.github}` / `{env.modelscope}` 替换为 `app_config` 表 `category='env'` 的值。已 seed：

| 变量 | 默认值 |
|------|--------|
| `env.huggingface` | `https://hf-mirror.com` |
| `env.modelscope` | `https://modelscope.cn` |
| `env.github` | `https://github.com` |

## 5. 各 Domain Manifest 设计

### 5.1 OCR 模型（sha256/size 预填——本地已有文件）

#### PP-OCRv6-small

source = `ocr/PP-OCRv6-small`，目标目录 `~/.octopus/models/ocr/PP-OCRv6-small/`

文件来自 3 个不同 HF repo + GitHub dict：

```json
{
  "cls.onnx": {
    "source": "{env.huggingface}/bukuroo/PPOCRv5-ONNX/resolve/main/ppocrv5-cls.onnx",
    "sha256": "f4bb53707100c5f3d59ba834eb05bb400369f20aed35d4b26807b1bfadd2a70e",
    "size": 582663
  },
  "det.onnx": {
    "source": "{env.huggingface}/PaddlePaddle/PP-OCRv6_small_det_onnx/resolve/main/inference.onnx",
    "sha256": "d73e0058b7a8086bbd57f3d10b8bcd4ff95363f67e06e2762b5e814fe9c9410e",
    "size": 9880512
  },
  "rec.onnx": {
    "source": "{env.huggingface}/PaddlePaddle/PP-OCRv6_small_rec_onnx/resolve/main/inference.onnx",
    "sha256": "5435fd747c9e0efe15a96d0b378d5bd157e9492ed8fd80edf08f30d02fa24634",
    "size": 21159378
  },
  "keys_v6.txt": {
    "source": "{env.github}/PaddlePaddle/PaddleOCR/raw/main/ppocr/utils/dict/ppocrv6_dict.txt",
    "sha256": "b5f2bfe2bdd9448429e3e82b51c789775d9b42f2403d082b00662eb77e401c5d",
    "size": 74947
  },
  "keys.txt": {
    "source": "{env.github}/PaddlePaddle/PaddleOCR/raw/main/ppocr/utils/dict/ppocrv6_dict.txt",
    "sha256": "b5f2bfe2bdd9448429e3e82b51c789775d9b42f2403d082b00662eb77e401c5d",
    "size": 74947
  }
}
```

注：`keys.txt` 与 `keys_v6.txt` 内容相同（下载器检测相同 source+sha256 自动复用缓存）。

#### PP-OCRv5

source = `ocr/PP-OCRv5`，所有文件来自同一 repo `bukuroo/PPOCRv5-ONNX`：

```json
{
  "cls.onnx": {
    "source": "{env.huggingface}/bukuroo/PPOCRv5-ONNX/resolve/main/ppocrv5-cls.onnx",
    "sha256": "f4bb53707100c5f3d59ba834eb05bb400369f20aed35d4b26807b1bfadd2a70e",
    "size": 582663
  },
  "det.onnx": {
    "source": "{env.huggingface}/bukuroo/PPOCRv5-ONNX/resolve/main/ppocrv5-mobile-det.onnx",
    "sha256": "d7fe3ea74652890722c0f4d02458b7261d9f5ae6c92904d05707c9eb155c7924",
    "size": 4748769
  },
  "rec.onnx": {
    "source": "{env.huggingface}/bukuroo/PPOCRv5-ONNX/resolve/main/ppocrv5-mobile-rec.onnx",
    "sha256": "bf66820f48fa99f779974c4df78e5274a9d8e0458c4137e8c5357e40e2c3faf2",
    "size": 16517247
  },
  "keys.txt": {
    "source": "{env.huggingface}/bukuroo/PPOVR5-ONNX/resolve/main/ppocrv5_dict.txt",
    "sha256": "1ea29636956177e400af712d9782e7693f3fb25f98617bed10479d2965a836fd",
    "size": 92395
  }
}
```

#### OCR DB seed 变更

现有 seed source 指向旧 GitHub MNN URL（已不使用），改为新格式：

```sql
-- 旧（删除）:
('ocr','paddleocr','ocr','PP-OCRv6-small',
 'https://github.com/zibo-chen/rust-paddle-ocr/raw/next/models/PP-OCRv6_small_det.mnn', ...)

-- 新:
('ocr','paddleocr','ocr','PP-OCRv6-small',
 'ocr/PP-OCRv6-small', '<manifest JSON>', 'auto', 'PP-OCRv6 small (det 9.7M + rec 21.5M + keys 73K)', 1, 1, 0),
('ocr','paddleocr','ocr','PP-OCRv5',
 'ocr/PP-OCRv5', '<manifest JSON>', 'auto', 'PP-OCRv5 mobile (det 4.5M + rec 16M + keys 92K)', 1, 0, 0)
```

`is_enabled=1` 仅 PP-OCRv6-small（当前激活），PP-OCRv5 默认 `is_enabled=0`。

### 5.2 翻译模型（sha256/size 预填——HF cache 已有文件）

翻译模型文件全部在 HF cache 中，sha256/size 预填实际值。

#### m2m100-418M

source = `translate/m2m100-418M`，目标目录 `~/.octopus/models/translate/m2m100-418M/`，HF repo `lazycodepersona/m2m100_418m`

```json
{
  "onnx/encoder_model_quantized.onnx": {
    "source": "{env.huggingface}/lazycodepersona/m2m100_418m/resolve/main/onnx/encoder_model_quantized.onnx",
    "sha256": "13a94e354a9140764eb81102d77d3ec6952d796e6f113c651eeb3c3443da0386",
    "size": 287856370
  },
  "onnx/decoder_model_quantized.onnx": {
    "source": "{env.huggingface}/lazycodepersona/m2m100_418m/resolve/main/onnx/decoder_model_quantized.onnx",
    "sha256": "6015e31c8976659aedb06058c4dadf0f400d087a3f9830f838e68f220d79bcb6",
    "size": 339181945
  },
  "tokenizer.json": {
    "source": "{env.huggingface}/lazycodepersona/m2m100_418m/resolve/main/tokenizer.json",
    "sha256": "df0873cc1c747fb4003a65e4e1e676ac4ebc98171bc351f1a0a5db2b461cf7db",
    "size": 7964703
  },
  "config.json": {
    "source": "{env.huggingface}/lazycodepersona/m2m100_418m/resolve/main/config.json",
    "sha256": "1dbdf77ddc7809acd4c54ccf0eab46f840b40174afb1b6f6de8787244e832938",
    "size": 908
  },
  "generation_config.json": {
    "source": "{env.huggingface}/lazycodepersona/m2m100_418m/resolve/main/generation_config.json",
    "sha256": "722210dd0bee7bef4e8e7f9a8574d8c56a2dfff723d73f390ce67892740b9009",
    "size": 233
  }
}
```

引擎加载只需 `onnx/encoder_model_quantized.onnx` + `onnx/decoder_model_quantized.onnx` + `tokenizer.json`（`m2m100.rs:26-28`），config.json / generation_config.json 为元数据。

#### opus-mt（1 行，双方向）

source = `translate/opus-mt`，目标目录 `~/.octopus/models/translate/opus-mt/`，zh-en 来自 `Xenova/opus-mt-zh-en`，en-zh 来自 `Xenova/opus-mt-en-zh`

```json
{
  "zh-en/onnx/encoder_model_int8.onnx": {
    "source": "{env.huggingface}/Xenova/opus-mt-zh-en/resolve/main/onnx/encoder_model_int8.onnx",
    "sha256": "c285f52c59ae2dee7778050a805ce6af9d6e1579edd9d36e92cd68b58f61ca70",
    "size": 52726552
  },
  "zh-en/onnx/decoder_model_int8.onnx": {
    "source": "{env.huggingface}/Xenova/opus-mt-zh-en/resolve/main/onnx/decoder_model_int8.onnx",
    "sha256": "624c24eed858e55ae1564db8d69e9ad10ccb3328fa18d8909a3f1494078effb4",
    "size": 192658470
  },
  "zh-en/tokenizer.json": {
    "source": "{env.huggingface}/Xenova/opus-mt-zh-en/resolve/main/tokenizer.json",
    "sha256": "b306d0301cf280bfd647d7067b5ade2a97b987e6d678df110703c002433643ff",
    "size": 6381339
  },
  "zh-en/config.json": {
    "source": "{env.huggingface}/Xenova/opus-mt-zh-en/resolve/main/config.json",
    "sha256": "293d318fce41dbf04114eac45037bb88a32d7c4ee21011a75e24a8b98ca45ad1",
    "size": 1389
  },
  "zh-en/generation_config.json": {
    "source": "{env.huggingface}/Xenova/opus-mt-zh-en/resolve/main/generation_config.json",
    "sha256": "8dc29fef0fe82109f94ef3c2e6ea6bded3215d357b226c34cf7b4630726766c9",
    "size": 293
  },
  "en-zh/onnx/encoder_model_int8.onnx": {
    "source": "{env.huggingface}/Xenova/opus-mt-en-zh/resolve/main/onnx/encoder_model_int8.onnx",
    "sha256": "262c0319bd0d8a6570f287211bf962035788954f20697e022cd60aaf62209b9c",
    "size": 52726553
  },
  "en-zh/onnx/decoder_model_int8.onnx": {
    "source": "{env.huggingface}/Xenova/opus-mt-en-zh/resolve/main/onnx/decoder_model_int8.onnx",
    "sha256": "8eb245366039256e29a21c73d6438f7a0878866d570b4e2b8fff5d88ec9bac5e",
    "size": 192658471
  },
  "en-zh/tokenizer.json": {
    "source": "{env.huggingface}/Xenova/opus-mt-en-zh/resolve/main/tokenizer.json",
    "sha256": "d0c7da27056e8f42adce9e76d8e792e5daa64e15f5acd2e7aabf0121877dd4c1",
    "size": 6380952
  },
  "en-zh/config.json": {
    "source": "{env.huggingface}/Xenova/opus-mt-en-zh/resolve/main/config.json",
    "sha256": "4727d1229a04f95bf6f39abf949d8080615433d99d6ebd85f81c09edd247d5fa",
    "size": 1503
  },
  "en-zh/generation_config.json": {
    "source": "{env.huggingface}/Xenova/opus-mt-en-zh/resolve/main/generation_config.json",
    "sha256": "b743baabb7da4c1a2f19fe558bd6b4c0c7c3b0762fcb5ca7a48fe5a2c2219803",
    "size": 293
  }
}
```

引擎加载只需各方向的 `onnx/encoder_model_int8.onnx` + `onnx/decoder_model_int8.onnx` + `tokenizer.json`（`opus_mt.rs:34-36`）+ `generation_config.json`（读 decoder_start/eos/pad token ID，`opus_mt.rs:64`），config.json 为元数据。

### 5.3 ASR 模型（seed 预填——HF cache 已有文件）

所有 12 个 ASR 本地模型的文件都在 HF cache 中，sha256/size 从本地文件计算后预填到 seed。

**文件过滤规则**：manifest 只包含引擎运行必需文件，排除以下：
- `test_wavs/`（测试音频）
- `.gitattributes`、`README.md`、`LICENSE`（元数据）
- `*.py`、`*.sh`（构建脚本）
- `quantize_config.json`（量化配置，非运行时必需）

#### whisper-small 示例

source = `asr/whisper-small`，HF repo `onnx-community/whisper-small.en`

```json
{
  "onnx/encoder_model_int8.onnx": {
    "source": "{env.huggingface}/onnx-community/whisper-small.en/resolve/main/onnx/encoder_model_int8.onnx",
    "sha256": "0a143c26b5aa5f549bef89a9363a56a5610a00985afe1e56443a71852bd642d4",
    "size": 92326127
  },
  "onnx/decoder_model_int8.onnx": {
    "source": "{env.huggingface}/onnx-community/whisper-small.en/resolve/main/onnx/decoder_model_int8.onnx",
    "sha256": "a01edeca857292810e090536068afb61510bcf9a4f6c54539ae45a07ccefb32c",
    "size": 155988577
  },
  "onnx/decoder_with_past_model_int8.onnx": {
    "source": "{env.huggingface}/onnx-community/whisper-small.en/resolve/main/onnx/decoder_with_past_model_int8.onnx",
    "sha256": "ae47a64cbac82c1772f3b9150d9f8b45badcb32a3303792f93fe950c84fef847",
    "size": 141651939
  },
  "config.json": {
    "source": "{env.huggingface}/onnx-community/whisper-small.en/resolve/main/config.json",
    "sha256": "8825c4174cb86f94d9fa67614942f8aa17bfbbdf2fae5426d4adfd0bc5893c43",
    "size": 2203
  },
  "generation_config.json": {
    "source": "{env.huggingface}/onnx-community/whisper-small.en/resolve/main/generation_config.json",
    "sha256": "5490747ca976d6b3765280a0697d66489020c2afa6e754244d9cd093e1639331",
    "size": 1956
  },
  "tokenizer.json": {
    "source": "{env.huggingface}/onnx-community/whisper-small.en/resolve/main/tokenizer.json",
    "sha256": "5eb60cec1e77aeeb6869a2bb5a8e01a84c3fe5d072d75369343021fe6f5310d0",
    "size": 2405679
  },
  "preprocessor_config.json": {
    "source": "{env.huggingface}/onnx-community/whisper-small.en/resolve/main/preprocessor_config.json",
    "sha256": "a6a76d28c93edb273669eb9e0b0636a2bddbb1272c3261e47b7ca6dfdbac1b8d",
    "size": 339
  },
  "added_tokens.json": {
    "source": "{env.huggingface}/onnx-community/whisper-small.en/resolve/main/added_tokens.json",
    "sha256": "560be47bea388757f8d4cc185c5d82067426cbb6361e38016dd90ddc01ab203a",
    "size": 34604
  },
  "special_tokens_map.json": {
    "source": "{env.huggingface}/onnx-community/whisper-small.en/resolve/main/special_tokens_map.json",
    "sha256": "98bdf3ec5b32e31575b02f64b0a32bde7c0449075d34484a7df9bdd3cdeb9fb9",
    "size": 2173
  },
  "tokenizer_config.json": {
    "source": "{env.huggingface}/onnx-community/whisper-small.en/resolve/main/tokenizer_config.json",
    "sha256": "93879c3dccdd4b976f709acd85b44778873f30c275e67026f30ca1e4c975230c",
    "size": 282662
  },
  "vocab.json": {
    "source": "{env.huggingface}/onnx-community/whisper-small.en/resolve/main/vocab.json",
    "sha256": "f6bd25a65e4e63ca31360e9fb11c7e4f9a391a78385d640acd814092dd6eee4f",
    "size": 999186
  },
  "merges.txt": {
    "source": "{env.huggingface}/onnx-community/whisper-small.en/resolve/main/merges.txt",
    "sha256": "1ce1664773c50f3e0cc8842619a93edc4624525b728b188a9e0be33b7726adc5",
    "size": 456318
  },
  "normalizer.json": {
    "source": "{env.huggingface}/onnx-community/whisper-small.en/resolve/main/normalizer.json",
    "sha256": "bf1c507dc8724ca9cf9903640dacfb69dae2f00edee4f21ceba106a7392f26dd",
    "size": 52666
  }
}
```

#### 全部 12 个 ASR 模型的 manifest 生成方式

实际实现时用脚本从 HF cache 自动生成（已验证全部存在）：

| model_name | HF repo | 必需文件 |
|---|---|---|
| `moonshine-base-en` | `csukuangfj/sherpa-onnx-moonshine-base-en-int8` | `encode.int8.onnx`, `cached_decode.int8.onnx`, `uncached_decode.int8.onnx`, `preprocess.onnx`, `tokens.txt` |
| `moonshine-tiny-en` | `csukuangfj/sherpa-onnx-moonshine-tiny-en-int8` | 同上（tiny 版） |
| `paraformer-bilingual` | `csukuangfj/sherpa-onnx-streaming-paraformer-bilingual-zh-en` | `encoder.int8.onnx`, `decoder.int8.onnx`, `tokens.txt` |
| `paraformer-multi-zh` | `csukuangfj/sherpa-onnx-streaming-paraformer-trilingual-zh-cantonese-en` | `encoder.int8.onnx`, `decoder.int8.onnx`, `tokens.txt`, `am.mvn`, `config.yaml` |
| `paraformer-streaming` | `csukuangfj/sherpa-onnx-streaming-paraformer-zh` | `encoder.int8.onnx`, `decoder.int8.onnx`, `tokens.txt` |
| `qwen3-asr-0.6B` | `csukuangfj2/sherpa-onnx-qwen3-asr-0.6B-int8-2026-03-25` | `encoder.int8.onnx`, `decoder.int8.onnx`, `conv_frontend.onnx`, `tokenizer/*` |
| `qwen3-asr-1.7B` | `ilmina/qwen3-asr-1.7b-sherpa-onnx` | 同上（1.7B 版） |
| `sensevoice-orig-small` | `WisemeAI/sensevoice-small-quant` | `model.onnx`, `am.mvn`, `config.yaml`, `tokens.json` |
| `firered-asr2` | `VidraAI/FireRedASR2-onnx` | `model.int8.onnx`, `tokens.txt` |
| `whisper-small` | `onnx-community/whisper-small.en` | `onnx/*.onnx` (3件套), `config.json`, `tokenizer.json` 等 |
| `zipformer` | `csukuangfj/sherpa-onnx-streaming-zipformer-zh-int8-2025-06-30` | `encoder.int8.onnx`, `decoder.onnx`, `joiner.int8.onnx`, `tokens.txt` |
| `zipformer-large` | `csukuangfj/sherpa-onnx-streaming-zipformer-zh-xlarge-int8-2025-06-30` | 同上 + `bpe.model` |

注：`paraformer-zh`（DB seed 第 6 行）与 `paraformer-streaming`（第 5 行）source 相同（同一 repo），manifest 内容一致——两个 model_name 共用同一份 manifest。

#### ASR DB seed 变更（v28 迁移）

```sql
-- source 从 HF repo 改为路径标识 asr/{model_name}
-- secret_key 从空改为预填 manifest JSON
UPDATE models SET 
  source = 'asr/' || model_name,
  secret_key = '<对应 manifest JSON>'
WHERE domain = 'asr' AND is_local = 1;
```

## 6. DB Schema 变更（v27 → v28）

### 6.1 domain 注释

```sql
domain TEXT NOT NULL,  -- 'asr' | 'llm' | 'ocr' | 'translate'
```

### 6.2 新增 translate seed

```sql
INSERT OR IGNORE INTO models (domain, provider, category, model_name, source, secret_key, language, description, is_local, is_enabled, is_streaming)
VALUES
    ('translate','local','opus-mt','opus-mt','translate/opus-mt','<manifest JSON>','auto','opus-mt 中英互译（轻量快速，~500M）',1,0,0),
    ('translate','local','m2m100','m2m100-418M','translate/m2m100-418M','<manifest JSON>','auto','m2m100 多语言翻译（100+ 语言互译，~600M）',1,0,0);
```

### 6.3 更新 OCR seed

```sql
-- PP-OCRv6-small: source 改为路径标识 + secret_key 写入 manifest
UPDATE models SET source = 'ocr/PP-OCRv6-small', secret_key = '<manifest JSON>'
WHERE domain = 'ocr' AND model_name = 'PP-OCRv6-small';

-- 新增 PP-OCRv5
INSERT OR IGNORE INTO models (domain, provider, category, model_name, source, secret_key, language, description, is_local, is_enabled, is_streaming)
VALUES
    ('ocr','paddleocr','ocr','PP-OCRv5','ocr/PP-OCRv5','<manifest JSON>','auto','PP-OCRv5 mobile (det 4.5M + rec 16M + keys 92K)',1,0,0);
```

### 6.4 更新 ASR seed

```sql
-- source 从 HF repo 改为路径标识 asr/{model_name}
UPDATE models SET source = 'asr/' || model_name
WHERE domain = 'asr' AND is_local = 1;
```

### 6.5 迁移逻辑

- 已有库（v27→v28）：执行上述 UPDATE + INSERT OR IGNORE
- 全新库：db.sql 直接含新格式 seed
- ASR 旧 secret_key（无 source URL 的 sha256 manifest）：下次 bootstrap 自动升级

## 7. Manifest 来源总结

| Domain | seed manifest | sha256/size | 来源 |
|--------|-------------|-------------|------|
| OCR | 预填完整 manifest | **预填实际值**（本地有文件） | seed 硬编码 |
| 翻译 | 预填完整 manifest | **预填实际值**（HF cache 有文件） | seed 硬编码 |
| ASR | 预填完整 manifest | **预填实际值**（HF cache 有文件） | 脚本从 HF cache 生成 → seed 硬编码 |

## 8. 代码变更

### 8.1 `crates/onnx-infra/src/paths.rs`

- `resolve_model_dir(source)` 不变（`source` = 路径标识如 `asr/whisper-small`，level 3 拼为 `~/.octopus/models/asr/whisper-small/`）
- ASR HF cache fallback 需要 model_name → repo 映射（从 manifest source URL 解析，或 DB 保留 repo 信息）

### 8.2 `crates/asr/src/manifest.rs`

- `bootstrap_manifest` → `bootstrap_manifest_with_source(repo, dir)` — 每个文件补 source URL
- manifest 格式从 `[{path, sha256}]` 升级为 `{path: {source, sha256, size}}`
- `verify_against_manifest` 适配新格式
- 旧格式（无 source）自动升级

### 8.3 `crates/translation/src/discovery.rs`

- 删除 `KNOWN_MODELS` 和 `OPUS_REPOS` 常量
- `discover_translation_models` → 从 DB 读 translate 模型 + 文件系统检查就绪
- `list_downloadable_translation_models` → 删除（统一走 `list_downloadable_models`）
- `m2m100.rs`：`M2M100_REPO` 常量删除，从 DB source 读
- `opus_mt.rs`：`resolve_opus_dir` 路径从 DB source 读

### 8.4 `crates/ocr/src/model.rs`

- `is_model_ready` 改为读 manifest 校验（如 secret_key 非空）
- `model_dir` 不变（已用 `ocr/{model_name}` 路径）

### 8.5 `crates/desktop/src/model_commands.rs`

- `download_model` 重构为 manifest 驱动：
  1. 从 DB 读 secret_key manifest
  2. 解析 JSON → 文件列表（路径 + source + sha256 + size）
  3. 替换 `{env.*}` 模板变量
  4. 逐文件下载到 `~/.octopus/models/{source}/{path}` + SHA256 校验
  5. 下载后 bootstrap 回填 sha256/size（空字段）→ 更新 secret_key
  6. is_enabled = true
- `list_downloadable_models` 加 `domain` 参数
- 修复 `#[tauri::command]` 错位
- 下载时读 `app_config.download_mirror`

### 8.6 `crates/desktop/src/translation_commands.rs`

- `list_downloadable_translation_models` → 删除
- `discover_translation_models` → 薄封装 DB 查询
- `translate_status` 逻辑不变

### 8.7 前端

- `TranslateTab.tsx`：改调 `list_downloadable_models({domain: "translate"})` + `download_model`，与 AsrTab 统一
- `OcrTab.tsx`：加下载按钮（已有 is_enabled，但缺下载入口）
- `AsrTab.tsx`：调用加 `{domain: "asr"}` 参数

## 9. 不变量

1. **`secret_key` 语义**：`is_local=1` → manifest JSON（空 = 未下载/未 bootstrap）；`is_local=0` → API Key
2. **manifest 格式**：扁平 key = 目标相对路径，value = `{source, sha256, size}`
3. **`source` 字段语义**（本地模型）：路径标识 `{domain}/{model_name}`，相对于 `~/.octopus/models/`
4. **opus-mt 是 1 行 DB**：source = `translate/opus-mt`，manifest 含双方向
5. **路径与引擎加载代码一致**：opus-mt → `translate/opus-mt/{direction}/onnx/`，m2m100 → `translate/m2m100-418M/onnx/`，OCR → `ocr/{name}/`

## 10. 降级路径

- bootstrap_manifest 逻辑保留用于校验旧库（v27→v28 迁移时 ASR secret_key 空 → 自动升级）
- OCR keys_v6.txt GitHub URL 不可达 → 用户可手动放置（`keys_v6.txt` 和 `keys.txt` 内容相同）

## 11. 测试覆盖

| 测试 | 位置 | 验证点 |
|------|------|--------|
| `manifest_parse_flat` | `asr/manifest.rs` | 扁平 JSON 解析为文件列表 |
| `manifest_parse_opus_mt` | `asr/manifest.rs` | opus-mt 双方向 manifest 解析 |
| `manifest_parse_ocr` | `asr/manifest.rs` | PP-OCRv6 三来源 manifest 解析 |
| `bootstrap_with_source` | `asr/manifest.rs` | 本地文件反推带 source 的 manifest |
| `verify_against_new_manifest` | `asr/manifest.rs` | 新格式校验（含 source/sha256/size） |
| `translate_models_in_db` | `infra/db.rs` | DB seed 含 translate 模型 |
| `ocr_models_manifest_in_db` | `infra/db.rs` | DB seed 含 OCR manifest |
| `download_by_manifest` | `desktop/model_commands.rs` | 按 manifest 逐文件下载 |
| `env_template_resolution` | `desktop/model_commands.rs` | `{env.huggingface}` 替换 |
| `resolve_asr_path` | `onnx-infra/paths.rs` | `asr/{name}` 路径查找 + HF cache fallback |
