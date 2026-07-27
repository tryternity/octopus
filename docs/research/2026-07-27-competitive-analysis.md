# octopus 竞品对比分析报告（2026-07-27）

> 基于对 `.tolaria/` 文档 + `~/workspace/agent/` 下 20+ 竞品源码的系统调研，逐功能对比 octopus 现状，给出查缺补漏建议。

---

## 总体定位

**octopus 是「本地优先的桌面 AI 工具集」**——以语音输入（ASR）为核心，集成 OCR、翻译、截屏、录屏、剪贴板、密码箱、AI 命令面板于一体。差异化壁垒：

1. **本地推理优先**（ASR/OCR/翻译/LLM 四域本地 ONNX，云端兜底）
2. **功能间深度集成**（ASR→润色→粘贴、OCR→翻译、选中→AI、Vault Auto-Type 与启动器同源）
3. **中文场景精耕**（热词纠错、拼音模糊、方言、简繁归一、中文 passphrase）

竞品多为「单功能极致」（CapsWriter/EcoPaste/snow-shot/QuickRecorder）或「通用平台」（cherry-studio/Wox），octopus 的「多合一 + 本地 + 中文」组合是独特生态位。

---

## 逐功能对比

### 1. 语音输入（ASR）

**竞品**：CapsWriter-Offline（Windows 离线语音输入）、transcribe-rs（Rust STT 库）、Handy（跨平台离线 STT）、sherpa-onnx（底层依赖）

| 特性 | octopus | CapsWriter | transcribe-rs/Handy |
|---|---|---|---|
| 本地引擎 | **7 个**（Whisper/SenseVoice/Paraformer/Qwen3-ASR/Zipformer CTC+RNN-T/Moonshine/FireRed） | 4 个（Paraformer/SenseVoice/Fun-ASR/Qwen3） | 9 个（Parakeet/Canary/whisper.cpp 等） |
| 云端引擎 | **4 家**（Aliyun 3 协议/ByteDance/Tencent/Baidu WSS） | ❌ | OpenAI 远程 |
| 流式实时 | ✅ 真流式（200ms tick + Active Flush）+ 伪流式 + 云端流式 | ✅ 按住→松开上屏 | ❌ Handy 非真流式 |
| 热词纠错 | ✅ 有界热词（拼音模糊 + Bigram + 方言 4 组）| ✅ 音素 RAG 两阶段（FastRAG+AccuRAG 双阈值） | ❌ |
| 标点 | 引擎自带（Zipformer/Moonshine 靠静音插逗号） | ✅ **外挂 CT-Transformer 标点模型** | Canary 内置 PnC |
| 数字 ITN | ❌ 依赖引擎 | ✅ 复杂数字格式化 | Canary 内置 |
| LLM 润色 | ✅ 深集成（polish_mode 0/1/2 + 多 prompt + 增量润色） | ✅ 角色系统（前缀触发） | ❌ |
| 跨平台 | ✅ 三端（mac CoreML/Linux CUDA/Win DirectML） | ❌ 仅 Windows | ✅ |
| 文件转录格式 | ❌ 仅纯文本 | ✅ srt/json + 时间戳 | ✅ 库层支持 |

**查缺补漏**：
- 🔴 **P0 数字 ITN**：「二零二六年」→「2026 年」，轻量后处理层，挂在 corrector 后
- 🔴 **P0 独立标点模型**（CT-Transformer）：Zipformer/Moonshine 无标点场景的外挂方案
- 🟡 **P1 文件转录字幕/时间戳**：server `/transcribe` 加 `format=srt` + token 时间戳
- 🟡 **P1 LLM 角色前缀触发**：「翻译…」触发翻译、「润色…」触发润色（语音命令入口）

**octopus 优势**：引擎矩阵最全（7 本地 + 4 云端）、热词+拼音+简繁后处理链最深、LLM 润色与 ASR 流水线一体化。

---

### 2. OCR

**竞品**：paddle-ocr-rs（上游库）、rapidocr（Python 多平台）、umi-ocr（桌面 GUI，插件式引擎）、ocrs（纯 Rust）、surya/monkeyocr/OvisOCR2（VLM 类）

| 特性 | octopus | umi-ocr | VLM 类（surya/OvisOCR2） |
|---|---|---|---|
| 引擎 | PP-OCRv6-small（ONNX，vendored 自 paddle-ocr-rs） | 插件式（PaddleOCR/RapidOCR/Pix2Text/Tesseract/WeChat/Mistral） | VLM 端到端（surya 650M / OvisOCR2 0.8B） |
| 检测+识别 vs VLM | 两阶段 det→cls→rec | 两阶段（继承插件） | 端到端 VLM |
| 表格识别 | ❌ | ❌ | ✅ |
| 公式识别 | ❌ | Pix2Text 插件 | ✅ LaTeX |
| 长图切分 | ✅（1280+200 overlap 坐标去重） | ✅ PDF/xps/epub | ✅ Unlimited OCR 32K |
| 布局 markdown | ✅ 自研（标题/列表/段落/code fence 启发式） | ✅ TBPU 8 方案 | ✅ VLM 原生 |
| 多语言 | 中英繁日（仅 seed 2 模型） | 简繁英日韩俄 | surya 91 语言 |
| 速度/内存 | 快/低（~30M 权重，idle 60s 释放） | 中 | 慢/高（9-50GB RAM，需 GPU） |

**查缺补漏**：
- 🔴 **P0 抽象 OCR backend trait**：学 umi-OCR 插件化，`trait OcrBackend` 让本地 PP-OCR/云端 VLM/未来本地 VLM 可切换（当前 PP-OCRv6 焊死在 OcrEngine）
- 🟡 **P1 云端 VLM OCR 入口**：覆盖表格/公式长尾（模型激活已支持 cloud source_type=2，schema 不用改）
- 🟡 **P1 多语言切换路径**：恢复 paddle-ocr-rs 的 model_registry（16 lang），或文档明确仅中英繁日
- 🟢 **P2 忽略区域**：借鉴 umi-OCR TBPU `ignore_area`，前端画框后端按 bbox 过滤

**octopus 优势**：截图 OCR 闭环打磨到位（长图切分 + 英文分词 + idle 释放 + 并发互斥），轻量 CPU 友好。不应追本地 VLM（与定位冲突）。

---

### 3. 翻译

**竞品**：CopyTranslator（复制即翻译，15+ 引擎）

| 特性 | octopus | CopyTranslator |
|---|---|---|
| 本地引擎 | ✅ **Opus-MT + m2m100**（完全离线） | ❌ |
| 云端引擎 | OpenAI 兼容（DeepSeek/智谱/百炼等） | **15+**（Google/Bing/Baidu/DeepL/彩云/腾讯...） |
| 监听剪贴板翻译 | ❌ 未打通 | ✅ **核心定位**（白/黑名单 + 防自身循环） |
| PDF 换行处理 | ❌ | ✅ **招牌特性** |
| 双语对照 | ✅ Result 浮窗 + CompactEditor | ✅ Contrast/Focus 模式 |
| 术语表/TM | ❌ | ⚠️ prompt 定制，无正式术语表 |
| 智能词典 | ❌ | ✅ 独立子系统（Youdao/Bing） |
| 多引擎对比 | ❌ 单激活 | ✅ 并发对比 |
| ASR 集成 | ✅ **独有**（流式翻译 + 终翻粘贴） | ❌ |
| 智能互译 | ❌ 固定 source/target | ✅ 自动换向 + 语言检测 |

**查缺补漏**：
- 🔴 **P0 剪贴板→翻译自动链路**：octopus 有剪贴板监听 + 翻译引擎但两者没打通
- 🔴 **P0 PDF 换行处理**：`do_translate` 入口加可选文本预处理
- 🟡 **P1 OCR→翻译闭环**：截图 OCR 后加「立即翻译」或自动翻译
- 🟡 **P1 术语表**：DB 加 `translation_glossary` 表，LLM 翻译注入 prompt（差异化机会）
- 🟡 **P1 智能互译**：加 `whatlang` 轻量语言检测，源=auto 时自动判断

**octopus 优势**：本地翻译引擎（CopyTranslator 完全没有）、ASR+翻译流式集成（独有）。

---

### 4. 截屏

**竞品**：xcap（库）、snow-shot（Tauri 截图工具，Excalidraw 标注）、screencapture（Win 单文件）、PixPin/eSearch（.tolaria 参考）

| 特性 | octopus | snow-shot | PixPin/eSearch |
|---|---|---|---|
| 区域/全屏/窗口 | ✅（窗口仅 macOS） | ✅ + **智能元素吸附**（UIAutomation） | ✅ |
| 滚动截图 | ✅ NCC+Sobel 实时预览（仅垂直） | ✅ HNSW+FAST（纵向/横向/任意方向） | ✅ |
| 标注工具 | 9 类（rect/oval/diamond/line/arrow/pen/text/number/blur） | **Excalidraw 全套**（+荧光笔/马赛克画笔/橡皮擦/二次编辑） | 荧光笔/聚光灯/放大镜/水印/折线 |
| 贴图 pin | ✅ 三平台原生浮窗 | ✅ fixedContent（标注层+OCR 层叠加） | ✅ |
| 智能元素吸附 | ❌ | ✅ Win RTree（滚轮切粒度） | ✅ |
| OCR 集成 | ✅ 深度（长图+布局 markdown） | ✅ 插件式 RapidOCR | ✅ |
| 取色器 | ❌ | ✅ | ✅ RGB/HEX/CMYK |
| 二维码识别 | ❌ | ✅ | ✅ |

**查缺补漏**：
- 🔴 **P0 智能元素/窗口吸附**：复用已有 `windows_uia.rs` UIAutomation，截图时按鼠标位置返回元素 rect
- 🟡 **P1 标注工具扩充**：荧光笔(highlight)、折线(polygon)、马赛克画笔(freedraw blur)
- 🟡 **P1 横向滚动截图**：参考 snow-shot 的 `ScrollDirection` 抽象
- 🟢 **P2 取色器 + 二维码识别**（`rqrr` crate）

**octopus 优势**：滚动拼接实时预览（NCC watch 通道）、OCR 深度集成、IPC 二进制传输、三平台 pin_window。

---

### 5. 录屏

**竞品**：QuickRecorder（macOS SCK 轻量录屏）、openscreen（跨平台，已 archived）、screenity（浏览器扩展）

| 特性 | octopus（MVP） | QuickRecorder | openscreen |
|---|---|---|---|
| 区域录制 | ✅ 拖框选区 | ✅ | ❌ 仅窗口/全屏 |
| 实时标注 | ✅ **9 工具 overlay**（area 录制中画框/箭头） | ❌ | ❌ 仅后期编辑 |
| 暂停/继续 | ✅ PTS 重写 | ❌ | ❌ |
| 系统音频 | ✅ SCK | ✅ **driver-free loopback** | ✅ |
| 多音轨分离 | ❌ 混入同一 mp4 | ✅ 独立可剪 | ✅ |
| 鼠标高亮 | ❌（仅 hide_cursor 开关） | ✅ overlay 窗口 | ✅ 可编辑光标+点击特效 |
| GIF 导出 | ✅（单遍 ffmpeg） | ❌ | ✅（多分辨率） |
| 后期编辑 | ❌（spec P3） | ✅ trimmer | ✅ 完整时间线 |
| ASR 字幕 | ❌（spec F15 P2） | ❌ | ✅ whisper.cpp |
| 摄像头 PiP | ❌（spec F21 P3） | ✅ Presenter Overlay | ✅ |

**查缺补漏**：
- 🔴 **P0 ASR 自动字幕**（应提优先级到 P1）：octopus 有 `crates/asr-local`，喂录屏音轨即可——**这是竞品都没有的护城河**
- 🔴 **P0 driver-free 音频 loopback**：SCK 失败时降级到 Core Audio tap
- 🟡 **P1 多音轨分离**：`mic_to_main_track: bool`，false 时 mic 独立 track
- 🟡 **P1 鼠标点击高亮**：教程/demo 视频
- 🟢 **P2 后期编辑（trim 起点）**：ffmpeg `-ss/-to` 单命令

**octopus 战略**：「标注 + 录屏 + ASR 字幕」三位一体是独有差异化。

---

### 6. 剪贴板

**竞品**：EcoPaste（纯剪贴板管理器，7100+ stars）、EcoPaste-Pro（fork，云同步）

| 特性 | octopus | EcoPaste |
|---|---|---|
| 监听 | clipboard-rs（Win 事件/Linux XFixes/Wayland 两级轮询） | clipboard-x（仅 X11） |
| 支持类型 | text/image/file/**voice/ocr** | text/image/file/**rich/html** |
| 富文本/HTML | ❌ | ✅ rtf.js + dompurify XSS 过滤 |
| 搜索 | ✅ **FTS5 trigram** | ❌ 仅 LIKE |
| 收藏/分组 | ✅ 收藏 + 6 类 item_type tab | ✅ 收藏 + **用户自定义分组** + 备注 |
| 软删回收站 | ✅ **独有**（deleted_at + TTL 3 天 + 500 上限） | ❌ 硬删 |
| dock 边缘吸附 | ✅ **独有**（macOS 8px 细条 hover 展开） | ❌ DockMode 空 stub |
| hover 预览 | ✅ 200×200 智能定位 | ❌ 行内预览 |
| Wayland | ✅ | ❌ |
| CJK IME 防乱码 | ✅ **独有** | ❌ |
| 跨设备同步 | ❌（octopus-sync 未覆盖剪贴板） | ❌ 官方不做（Pro fork 做） |

**查缺补漏**：
- 🔴 **P0 富文本/HTML 支持**：clipboard-rs 已提供 `get_rich_text()/get_html()`，扩展 ItemType + **前端必须加 XSS 过滤**
- 🟡 **P1 用户自定义分组/标签**：`clipboard_groups` 表
- 🟡 **P1 备注字段**：`note TEXT` + FTS5 索引覆盖
- 🟢 **P2 跨设备同步**：复用 octopus-sync，需防回环（`suppress_flag` 可复用）

**octopus 优势**：voice/ocr 一体化、FTS5 trigram、软删回收站、dock 吸附、CJK 防乱码——EcoPaste 都没有。

---

### 7. 密码箱（Vault）

**竞品**：vaultwarden（Bitwarden 服务端）、keepassxc（桌面 KDBX）、gopass（CLI git）

| 特性 | octopus | vaultwarden | keepassxc |
|---|---|---|---|
| 加密 | Argon2id + AES-256-GCM 双层密钥 + **KDF 远程参数安全上下限** | Argon2id/PBKDF2 | Argon2id/AES-KDF |
| Auto-Type | ✅ 全局热键 + VaultPicker + 3 模式 + eTLD+1 防钓鱼 | ❌（靠浏览器扩展） | ✅ 虚拟键盘 + 窗口标题关联 |
| 浏览器扩展 | ❌ | ✅ 官方 Bitwarden | ✅ KeePassXC-Browser |
| passkey/WebAuthn | ❌ | ✅ 完整 | ✅ 2.7.7+ |
| SSH agent | ❌ | ❌（数据模型有 SshKey 类型） | ✅ 内建 |
| TOTP | ✅ RFC 6238 | ✅ | ✅ |
| 中文 passphrase | ✅ **独有**（4096 中文词表） | ❌ | ❌ |
| git 同步 | ✅ + stamp 双向冲突解决 | centralized server | 文件级（用户自选） |
| 团队共享 | ❌ | ✅ 完整组织 | KeeShare |
| 紧急联系人 | ❌ | ✅ 完整 | ❌ |
| 附件/Send | ❌ | ✅ | ✅ 附件 |

**查缺补漏**：
- 🔴 **P0 浏览器扩展**（最高 ROI）：轻量扩展经本地 Tauri HTTP/WS 桥接，表单检测+URL 上送，填充走现有后端
- 🔴 **P0 passkey/WebAuthn**：先做存储（新增 cipher 类型），再做认证（替代/补充主密码 2FA）
- 🟡 **P1 SSH agent**：开发者工具定位，加 SSH key cipher 类型 + `SSH_AUTH_SOCK`
- 🟡 **P1 cipher 类型扩展**：SecureNote/Card/Identity（已预留枚举）
- 🟢 **P2 导入导出扩展**：KDBX（吃下 KeePass/1Password 全量迁移）

**octopus 优势**：中文 passphrase（无竞品）、KDF 安全上下限（防 git 同步投毒）、URL 防钓鱼 eTLD+1、字段级加密+Zeroizing 清零、与 ASR/剪贴板/启动器集成。

---

### 8. Action Bar（命令面板/启动器）

**竞品**：Wox（全平台启动器）、Sidey（macOS 菜单栏 AI 助手）

| 特性 | octopus | Wox | Sidey |
|---|---|---|---|
| 唤起 | 全局热键 + 选中文本感知 | 全局热键（Alt+Space/双 Ctrl） | 菜单栏 + **随窗助手** + **选中文本浮动按钮** |
| 搜索 | App/文件/书签/命令/菜单/**计算器**/URL（7 provider） | App/文件/书签/网页/计算器/**单位换算/货币**/emoji/窗口管理 | 无通用搜索 |
| 选中文本感知 | ✅ CGEvent + osascript 模拟 Cmd+C | ✅ + **AI Command Run And Paste** | ✅ Accessibility API + 浮动按钮 |
| AI 集成 | ✅ 润色/摘要/解释/翻译/PPT + prompt `@文件名` | ✅ **11 LLM provider + MCP 原生** + AI Command 模板商店 | ✅ 公共 AI + **app-aware 助手人设** |
| 插件系统 | .octopusext（config.yaml + 脚本路径） | **Python+Node SDK + 独立 host 进程 + 在线商店 35 插件** | 无 |
| App-aware | ✅ per-菜单项 `app_bundle_ids` | 部分 | ✅ per-app 助手人设（自动生成） |
| 跨平台 | ❌ macOS only（CGEvent+osascript） | ✅ 全平台一等公民 | ❌ macOS only |

**查缺补漏**：
- 🔴 **P0 Run And Paste 重评估**：octopus 当前放弃（浏览器安全策略），但对原生 app 可行，按 app 白名单启用
- 🟡 **P1 单位/货币换算 provider**：octopus 已有 calculator，扩展一个 converter 成本低
- 🟡 **P1 LLM 流式输出**：`client.rs` 改 SSE，润色/翻译体感延迟大降
- 🟢 **P2 插件 host 进程模型**：若做插件生态的基础设施

**最大威胁**：Wox 的 AI Command + Run And Paste + MCP 组合正在侵蚀 octopus 差异化。octopus 必须在「ASR 联动 + 本地模型 + Vault 集成」三个 Wox 短期补不上的维度加深。

---

### 9. AI 集成（LLM）

**竞品**：cherry-studio（通用 LLM 客户端）、AionUi（Agent 平台）、nanobot（轻量 Agent）

| 特性 | octopus | cherry-studio | AionUi |
|---|---|---|---|
| 定位 | **功能内嵌增强**（ASR 润色/Actionbar/PPT） | 通用 LLM 聊天客户端 | Cowork Agent 平台 |
| 流式输出 | ❌ blocking 一次性 | ✅ SSE | ✅ |
| 多轮对话 | ❌ 单轮 system+user | ✅ 多 topic 多模型 | ✅ |
| 多模态 | ❌ 纯文本 | ✅ 图片/Office/PDF | ✅ 文生图/视频 |
| Agent 工作流 | 仅召唤外部 CLI agent（PPT） | 300+ assistant + MCP | ✅ 内置 agent engine + Team Mode |
| 知识库/RAG | ❌ | ✅ | ✅ |
| 提示词管理 | DB prompts 表 + action_data 模板 | 300+ 预置 assistant | 21 assistant + 三层 skill |

**核心判断：octopus 不应做独立 AI 聊天窗口**（赛道死局，cherry-studio/AionUi 已红海）。

**查缺补漏**（保持「嵌入式增强剂」定位）：
- 🔴 **P0 LLM 流式输出**：`client.rs` 加 SSE 解析，最低成本最高契合
- 🟡 **P1 多模态图片输入**：截图/OCR 的图片直接喂视觉模型（「解释这张图」）
- 🟢 **P2 MCP 工具协议**：低成本接入工具生态（不用自建工具市场）

**明确不建议**：知识库/RAG（重资产，与轻交互理念冲突）、独立聊天窗口（稀释定位）。

---

## 查缺补漏汇总（按优先级）

### 🔴 P0（强烈建议，高 ROI）

| # | 功能 | 缺口 | 来源 | 实现成本 |
|---|---|---|---|---|
| 1 | ASR | 数字 ITN 后处理 | CapsWriter | 低（正则+词典） |
| 2 | ASR | 独立标点模型 CT-Transformer | CapsWriter | 中（外挂 ONNX） |
| 3 | 剪贴板 | 富文本/HTML 支持 | EcoPaste | 低（clipboard-rs 已有，+XSS 过滤） |
| 4 | 翻译 | 剪贴板→翻译自动链路 | CopyTranslator | 低（打通现有模块） |
| 5 | 录屏 | ASR 自动字幕 | openscreen + 自有 asr-local | 中（喂音轨给 ASR） |
| 6 | 密码箱 | 浏览器扩展 | keepassxc/vaultwarden | 高（但最高差异化价值） |
| 7 | Vault | passkey/WebAuthn | vaultwarden | 中（先存储后认证） |
| 8 | AI/LLM | 流式输出 | 全部竞品 | 低（client.rs 改 SSE） |
| 9 | 截屏 | 智能元素/窗口吸附 | snow-shot | 中（复用 windows_uia） |

### 🟡 P1（值得做，差异化或体验提升）

| # | 功能 | 缺口 | 来源 |
|---|---|---|---|
| 10 | ASR | 文件转录字幕/时间戳 | CapsWriter |
| 11 | ASR | LLM 角色前缀触发 | CapsWriter |
| 12 | OCR | 抽象 backend trait + 云端 VLM OCR 入口 | umi-OCR |
| 13 | 翻译 | OCR→翻译闭环 | CopyTranslator |
| 14 | 翻译 | 术语表 Glossary | BabelDOC/translate-book |
| 15 | 翻译 | 智能互译语言检测 | CopyTranslator |
| 16 | 截屏 | 标注扩充（荧光笔/折线/马赛克画笔） | snow-shot/PixPin |
| 17 | 截屏 | 横向滚动截图 | snow-shot |
| 18 | 录屏 | driver-free 音频 loopback | QuickRecorder |
| 19 | 录屏 | 多音轨分离 | QuickRecorder |
| 20 | 录屏 | 鼠标点击高亮 | openscreen |
| 21 | 剪贴板 | 用户自定义分组/标签 | EcoPaste |
| 22 | 剪贴板 | 备注字段 | EcoPaste |
| 23 | 密码箱 | SSH agent | keepassxc |
| 24 | 密码箱 | cipher 类型扩展（Card/Identity/Note） | 1Password/Bitwarden |
| 25 | ActionBar | Run And Paste 重评估（原生 app 白名单） | Wox |
| 26 | ActionBar | 单位/货币换算 provider | Wox |
| 27 | AI/LLM | 多模态图片输入 | cherry-studio |

### 🟢 P2（可选，长期/锦上添花）

| # | 功能 | 缺口 |
|---|---|---|
| 28 | ASR | whisper.cpp 大模型后端 |
| 29 | OCR | 忽略区域（TBPU ignore_area） |
| 30 | 翻译 | 多引擎并发对比 |
| 31 | 翻译 | DeepL/彩云等传统 MT API 直连 |
| 32 | 截屏 | 取色器 + 二维码识别 |
| 33 | 录屏 | 后期编辑（trim 起点） |
| 34 | 录屏 | 摄像头 PiP（Presenter Overlay） |
| 35 | 剪贴板 | 跨设备同步（复用 octopus-sync） |
| 36 | 剪贴板 | 来源 app 图标追溯 |
| 37 | 密码箱 | 紧急联系人 |
| 38 | 密码箱 | 团队共享（gopass 模式） |
| 39 | AI/LLM | MCP 工具协议 |

### ⚫ 明确不建议

| 项 | 理由 |
|---|---|
| 本地 VLM OCR（surya/OvisOCR） | 9-50GB RAM，与轻量 CPU 定位冲突 |
| 独立 AI 聊天窗口 | 赛道死局，cherry-studio/AionUi 红海 |
| 知识库/RAG | 重资产，与轻交互理念冲突 |
| Centralized vault server | 与本地优先+git 同步哲学冲突 |
| 浏览器录屏（screenity） | 桌面 app 形态不匹配 |
| Wox 式主题商店 | 工具集不是启动器，ROI 低 |

---

## octopus 的差异化护城河（应强化）

1. **本地推理四域**（ASR/OCR/翻译/LLM）——竞品最多覆盖 1-2 个
2. **ASR→润色→粘贴** 语音输入闭环——CapsWriter/Handy 无润色深度
3. **选中即用 AI 命令面板**——Wox 有但无 ASR 联动
4. **标注+录屏+ASR 字幕三位一体**——QuickRecorder/openscreen 无 ASR
5. **voice/ocr 一体化剪贴板**——EcoPaste 无 ASR/OCR
6. **中文 passphrase + 热词纠错 + 拼音模糊 + 简繁归一**——无竞品覆盖
7. **Vault 与 ASR/剪贴板/启动器集成**——keepassxc/vaultwarden 是独立工具
8. **KDF 安全上下限 + URL 防钓鱼 + 字段级加密**——密码学工程精细度

---

*报告基于 2026-07-27 代码状态 + 竞品源码调研。竞品代码来源：`~/workspace/agent/` 下 20+ 项目 + `.tolaria/` 文档库。*
