# octopus-download 架构与 TLS 依赖分析

> 2026-06-21 整理。记录 `octopus-download` crate 接入 workspace 后的 TLS 栈分布、体积影响，
> 以及既有 `llm` / `dlp` crate 为何依赖 native-tls。本文聚焦 TLS 选型与体积，crate 功能设计详见
> `docs/superpowers/specs/2026-06-21-model-download-design.md`。

---

## 1. octopus-download 概述

通用下载 crate（`crates/download`），替代 `huggingface-cli` 解决三个终端用户痛点：

1. 终端用户原本需要装 Python + hf-cli 才能下模型；
2. hf-cli 在国内需要手动切镜像；
3. 无参的 hf-cli 会拉整个仓库，而 int8 量化文件才是实际需要的。

能力：分块并发下载 + 断点续传（`<dest>.part.resume.json` sidecar）+ SHA256 整文件校验 + 镜像 fallback + HF 适配层（`api` / `glob` / `resolve`）。

**尚未接入模型管理**（仍走 `~/.cache/huggingface/hub/`）；下一步是用 `resolve_tasks` + `Downloader::download` 替换 hf-cli，下载到 `~/.octopus/models/<repo>/<path>`。

---

## 2. TLS 选型：download 选 rustls，workspace 其余走 native-tls

| crate | reqwest 配置（`Cargo.toml`） | TLS 栈 | 选型方式 |
|---|---|---|---|
| **download** | `default-features = false`, `features = ["stream", "http2", "rustls-tls", "json"]` | **rustls**（ring，静态编进 bin） | 主动显式选 |
| **llm** | `features = ["blocking", "json"]`（未禁 default-features） | **native-tls** | reqwest 默认（被动） |
| **dlp** | `features = ["json", "stream"]`（未禁 default-features） | **native-tls** | reqwest 默认（被动） |

download 选 rustls 的动机：它要替代 hf-cli 给**终端用户**下大模型，必须零系统依赖、跨平台分发友好——rustls 把 TLS 实现静态编进二进制（ring），代价是约 1.5 MB 体积。

---

## 3. llm / dlp 依赖 native-tls 的场景与路径

### 3.1 场景：两者都是对外 HTTPS，但用途不同

**llm crate**（`crates/llm/src/client.rs`）：
- `reqwest::blocking::Client`（**同步**）→ POST `{base_url}/chat/completions`，带 `Authorization: Bearer {secret_key}`；
- 用途：调云端 LLM API（OpenAI 兼容协议）做**文本润色** + 设置页**连接测试**（`test_connection`，max_tokens=1 的极简探测请求）；
- 用户自配 `base_url`（OpenAI / DeepSeek / Qwen 等兼容端点），desktop 依赖它做润色与连接测试。

**dlp crate**（`crates/dlp/src/main.rs`，独立 bin）：
- `reqwest::get(url).await` + `bytes_stream()`（**异步流式**）；
- 用途：**下载 yt-dlp 二进制 / 视频资源**到 `~/.octopus/bin/`（yt-dlp 的引导 / 包装工具，见 `YtdlpMetadata`、`get_binary_path`）；
- `stream` feature 用于流式落盘。

### 3.2 引入路径：是 reqwest 默认带，**不是显式选的**

```
llm / dlp 的 Cargo.toml:
  reqwest = { version = "0.12", features = ["blocking"/"json"/"stream"] }
                  ↑ 没写 default-features = false，也没指定任何 tls feature

→ reqwest 0.12 启用默认 features，其中含 default-tls
→ reqwest 0.12 在 native 平台: default-tls == native-tls
    ├─ native-tls          0.2.18
    ├─ tokio-native-tls    0.3.1   (dlp 异步路径用；llm blocking 也间接拉入)
    └─ hyper-tls           0.6     (reqwest 的 native-tls 连接器)
```

**关键**：llm / dlp 是「被动吃 reqwest 默认值」，Cargo.toml 里**没有任何一行提到 native-tls**。这与 download 的 `default-features=false, features=["rustls-tls"]`（主动选）形成对比。

### 3.3 平台后端 + 为什么几乎不占 bin 体积

native-tls 是**系统 TLS 的薄封装**，动态链接系统库、不把 TLS 实现编进二进制：

| 平台 | native-tls 后端 | 链接方式 | bin 体积代价 |
|---|---|---|---|
| macOS | `security-framework` | 系统库，动态 | ≈ 0 |
| Linux | `openssl` → `libssl.so` | 系统库，动态 | ≈ 0（但**要求目标机装 OpenSSL**） |
| Windows | `schannel` | 系统 API | ≈ 0 |

这就是 workspace 一直用 native-tls 没感觉到体积的原因——TLS 实现在系统库里。**代价转嫁成了「运行期系统依赖」**：Linux 桌面包若目标机缺 OpenSSL 会启动失败。

---

## 4. 体积影响分析

download 接入后，release 二进制体积增量主要来自其 **rustls 栈**：

- rustls（ring）静态编进 bin ≈ **+1.5 ~ 2 MB**（ring ~0.85 MB + rustls ~0.3 MB + 其余）；
- reqwest / tokio / serde_json 等**已被 workspace 复用**，不重复计体积；
- **但 TLS 无法复用**：workspace 既有 TLS 栈是 native-tls（系统库，不进 bin），download 的 rustls 是新增的、独立的静态栈——两者本就不同，没有「reqwest 共享省下 TLS」一说。

> 结论：download 接入后那 +1.5 ~ 2 MB **几乎全是 rustls 栈新增**，而非 reqwest 重复。
> 如需精确到 ±0.1 MB，可临时给某目标 bin（cli / desktop）加 `octopus-download` 依赖，
> 跑 release 构建 diff 体积。

---

## 5. 对比与启示

| | llm / dlp（native-tls） | download（rustls） |
|---|---|---|
| TLS 实现 | 系统库，动态链接 | ring，静态编进 bin |
| bin 体积 | ≈ 0 | +1.5 MB |
| 系统依赖 | 需 OpenSSL（Linux 痛点） | 无（纯静态，跨平台分发友好） |
| 选型方式 | reqwest 默认（被动） | 显式 `rustls-tls`（主动） |

**启示：**

1. **download 选 rustls 是对的**——替代 hf-cli 给终端用户下大模型，零系统依赖是硬约束；1.5 MB 是合理代价。
2. **若将来想统一去 native-tls**（消除 Linux OpenSSL 分发隐患 + 顺带省掉双 TLS 栈），改法很小：给 llm / dlp 的 reqwest 加 `default-features = false, features = ["blocking"/"json"/"stream", "rustls-tls"]`。llm 的 `blocking` 在 rustls 下支持正常。属跨 crate 改动，且当前未引发问题，优先级低。
3. **dlp 的 `download_file` 与 octopus-download 功能重叠**——dlp 现在用裸 `reqwest::get` 流式下载（无断点续传 / 校验）。未来 yt-dlp binary 下载可改用 `octopus-download`（带校验，防半截损坏）。不过 yt-dlp 才几 MB，收益有限，仅作架构观察。

---

## 6. 参考路径

- download reqwest 配置：`crates/download/Cargo.toml`（`rustls-tls` feature）
- llm reqwest 使用：`crates/llm/src/client.rs`（`blocking::Client`，润色 + `test_connection`）
- dlp reqwest 使用：`crates/dlp/src/main.rs`（`reqwest::get` 流式下载）
- 功能设计：`docs/superpowers/specs/2026-06-21-model-download-design.md`
- 实施计划：`docs/superpowers/plans/2026-06-21-model-download.md`
- 架构概览：`docs/architecture.md`（### octopus-download 模块）
