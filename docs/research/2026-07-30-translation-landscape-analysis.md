# 翻译功能竞品调研与差距分析

**日期**：2026-07-30
**来源**：`~/.tolaria/` 下 20 篇核心翻译工具文档 + octopus 当前翻译功能对比
**目的**：查缺补漏，识别 octopus 翻译功能的改进方向

---

## 一、octopus 当前翻译能力

| 维度 | 现状 |
|---|---|
| **引擎** | Opus-MT（30M 本地中英互译）+ m2m100-418M（100+ 语言本地 ONNX int8）+ CloudLlmEngine（OpenAI/DeepSeek/阿里/智谱/Moonshot/MiniMax 5+1 家云端 LLM） |
| **架构** | `TranslationEngine` async trait + 三引擎统一接口 + DB 模型激活管理（`resolve_active_engine("translate")`）+ FallbackLlm 兜底（复用润色 LLM） |
| **触发入口** | ① ActionBar 翻译菜单（选中文本→翻译→CompactEditor 展示）；② Result 窗口双语视图（语音识别实时翻译）；③ CompactEditor 翻译对照模式（左右分栏原文/译文） |
| **模式** | 流式分段翻译（按换行切段串行）+ 双语对照（上下/左右分栏）+ 自动定时重译（4/8/12s）+ 手动翻译 + 停止后终翻 |
| **语言** | 源语言自动检测 + 目标语言可配（zh/en/ja/ko 等） |
| **缓存** | 本地引擎 HashMap 缓存（opus-mt 按 spec+方向，m2m100 按 spec） |
| **离线** | ✅ Opus-MT + m2m100 完全本地，无需 API Key |
| **字幕翻译** | ✅ 录屏自动字幕（generate_subtitle）+ LLM 润色（subtitle_polish） |

---

## 二、tolaria 调研：20 篇核心翻译工具

### 2.1 桌面划词/截图翻译（7 篇）

| 工具 | 平台 | 核心翻译功能 |
|---|---|---|
| **Pot Desktop**（19.1k ⭐） | Tauri+React 跨平台 | 划词/输入/剪贴板监听/截图/外部 HTTP 多模式；21+ 引擎（OpenAI/Gemini/Ollama/百度/腾讯/火山/Google/DeepL 等）；截图翻译+多 OCR；多接口并行并列展示；插件系统 `.potext`；生词本导出（Anki/欧路） |
| **STranslate**（7.6k ⭐） | Windows WPF | 划词翻译；Ctrl+C+C 触发；截图翻译（OCR→翻译）；图片翻译（矢量覆盖层在原图位置呈现译文）；静默 OCR；增量翻译；剪贴板监听翻译；21 个插件；AI 润色/总结 |
| **Manggo**（Pot 作者新作） | Qt 跨平台 | 输入翻译；划词翻译；翻译并替换（Pro）；截图 OCR+截图翻译；原图翻译（Pro）；多引擎；兼容 Pot/Bob 插件；生词本（Anki/欧路）；内置英汉词典 |
| **eSearch** | Electron 跨平台 | OCR 后即时翻译；选词翻译；多引擎并行；屏幕翻译（生成贴图窗口替换图片文字为译文，支持定时翻译，适合视频/游戏）；Anki 集成 |
| **KISS Translator**（11.6k ⭐） | 浏览器扩展+油猴 | 网页双语对照；输入框翻译；划词翻译（多服务对比）；鼠标悬停翻译；YouTube 字幕翻译（双语+AI 断句）；AI 上下文会话记忆；术语词典；跨端同步 |
| **FluentRead**（7.4k ⭐） | 浏览器扩展（WXT+Vue） | 20+ 引擎；双语对照/仅译文；划词翻译；全文翻译悬浮球；输入框翻译；24h 智能缓存；12+ 译文样式；回译 |
| **Read Frog**（8.7k ⭐） | 浏览器扩展（TS） | 沉浸式翻译；选中文本流式翻译+AI 词汇洞察；YouTube/Shorts 字幕翻译+双语字幕+SRT 导出；AI 字幕生成；TTS 朗读；闪卡间隔重复 |

### 2.2 文档/PDF 翻译（3 篇）

| 工具 | 核心翻译功能 |
|---|---|
| **PDFMathTranslate**（pdf2zh） | PDF 全文翻译保留公式/图表/目录；纯译文+双语对照 PDF；多后端（Google/DeepL/OpenAI/Ollama）；CLI/GUI/Docker/MCP/Zotero |
| **BabelDOC**（8.9k ⭐） | PDF 原排版保留翻译；双语对照；公式保护；扫描件 OCR；CSV 术语表注入+自动术语提取；大文档分片 |
| **Paper Burner X** | 纯前端文献工作站：OCR+批量翻译+对照阅读；BYOK 多 Key 轮询；数万条术语库；段落级原译文智能对齐 |

### 2.3 视频/语音翻译（7 篇）

| 工具 | 核心翻译功能 |
|---|---|
| **pyVideoTrans**（18.2k ⭐） | 9 阶段流水线：预处理→ASR→说话人分离→**字幕翻译**→配音→音画对齐；24 种翻译渠道（含 M2M100 本地离线）；声音克隆 |
| **KrillinAI** | LLM 上下文翻译；术语替换；双语 SRT；横竖版双语字幕视频渲染；Agent Skills 编排 |
| **Violin**（974 ⭐） | Whisper 词级时间戳→LLM 逐段翻译+6 种风格→TTS→ffmpeg 对齐；33 种目标语言；16 种母语音色 |
| **MioSub**（746 ⭐） | Gemini AI 全自动字幕：转录→翻译→CTC 毫秒级时间轴→压制；100+ 语言互译；双语字幕导出（SRT/ASS） |
| **JZSub**（867 ⭐） | Codex Skill：视频下载→字幕提取→GPT 翻译→双语字幕渲染→FFmpeg 硬烧录；紧凑分批翻译（批间共享上下文保术语连贯） |
| **WhisperSubTranslate**（531 ⭐） | whisper.cpp 生成 SRT→**离线翻译：腾讯 Hy-MT2 模型（1.8B/7B，无需 API Key）**；100% 本地隐私 |
| **AirTranslate** | macOS 实时系统音频→转写→翻译→悬浮字幕；5 种模式含 Apple Translation 完全离线；原文译文并排 |

### 2.4 AI Agent 翻译技能（1 篇）

| 工具 | 核心翻译功能 |
|---|---|
| **Rainman translate-book**（964 ⭐） | PDF/DOCX/EPUB 整书翻译；**并行子代理**（8 并发）；术语表一致性系统（采样提取→glossary.json→每 chunk 注入→反馈闭环→选择性重译）；断点续译 |

---

## 三、差距分析

| 功能点 | octopus | 竞品覆盖度 | 差距评估 | 实现难度 |
|---|---|---|---|---|
| **截图翻译** | ❌ | Pot/STranslate/Manggo/eSearch | 🔴 高 | 🟢 低——已有 OCR（paddle-ocr）+ 翻译引擎 + CompactEditor，串联即可 |
| **图片翻译（矢量覆盖）** | ❌ | STranslate/Manggo/eSearch | 🔴 高 | 🟡 中——需 OCR 位置信息 + 译文渲染覆盖层 |
| **划词翻译即时弹窗** | ❌（ActionBar 选中文本可翻译，但非即时悬浮弹窗） | Pot/STranslate/Manggo/KISS/FluentRead | 🟡 中 | 🟡 中——需监听全局选中文本 + 悬浮窗 |
| **输入框翻译** | ❌ | FluentRead/KISS/Pot | 🟡 中 | 🟡 中——需模拟键盘输入替换 |
| **剪贴板监听翻译** | ❌（有剪贴板历史，无"复制即翻译"） | STranslate/Pot | 🟡 中 | 🟢 低——watcher 已有监听，加翻译分支即可 |
| **多引擎并行对比** | ❌（单引擎翻译） | Pot/STranslate/KISS | 🟡 中 | 🟢 低——已有三引擎，并发调用+并列展示 |
| **术语词典** | ❌ | KISS/BabelDOC/Rainman/PaperBurnerX | 🟡 中 | 🟡 中——需 DB 表 + 翻译时注入 |
| **实时翻译字幕** | ❌（有 ASR 但无实时翻译输出） | AirTranslate | 🟡 中 | 🟡 中——已有 Result 窗口双语视图雏形，需串联 ASR 流式 + 翻译流式 |
| **翻译历史/生词本** | ❌ | Pot/STranslate/Manggo（Anki 导出） | 🟢 低 | 🟢 低——DB 表 + CRUD |
| **回译** | ❌ | FluentRead | 🟢 低 | 🟢 低——反向调用翻译引擎 |
| **TTS 朗读译文** | ❌（有 TTS 能力但未串联） | ReadFrog/Trancy | 🟢 低 | 🟢 低——翻译完成后调 TTS |
| **网页沉浸式翻译** | ❌ | FluentRead/ReadFrog/KISS/Trancy | ⚪ 不适用 | — octopus 是桌面 app 非浏览器扩展 |
| **视频翻译配音** | 部分（录屏字幕 ASR+翻译） | pyVideoTrans/KrillinAI/Violin | ⚪ 超范围 | — 完整配音流水线（TTS+音画对齐+声音克隆）超出 octopus 定位 |
| **PDF/文档翻译** | ❌ | PDFMathTranslate/BabelDOC | ⚪ 超范围 | — PDF 排版保留翻译是独立产品领域 |

---

## 四、优先级建议（查缺补漏）

### 🔴 P0：高优先级（与 octopus 现有架构最契合，实现成本低）

1. **截图翻译**——octopus 已有截图 + OCR + 翻译引擎 + CompactEditor，只需串联：
   - 截图标注工具栏加「翻译」按钮 → OCR 识别 → 翻译 → 双语对照展示
   - 或：剪贴板图片条目右键「翻译」→ OCR → 翻译
   - 预估工作量：1-2 天

2. **图片翻译（OCR→翻译）**——已有 `ocr_image` + `translate_text` 命令：
   - 图片预览窗口加「翻译」按钮 → OCR → 翻译 → 译文覆盖或对照
   - 预估工作量：1 天

### 🟡 P1：中优先级（有价值但需新组件）

3. **多引擎并行对比**——已有三引擎（Opus-MT / m2m100 / CloudLLM），并发调用并列展示：
   - 翻译结果窗加「多引擎对比」模式，同一段文本同时调 2-3 个引擎
   - 预估工作量：1 天

4. **实时翻译字幕**——已有 Result 窗口双语视图 + ASR 流式输出：
   - ASR 流式识别结果实时送翻译引擎 → 双语字幕实时更新
   - 预估工作量：2-3 天（流式 ASR + 流式翻译的时序协调）

5. **划词翻译即时弹窗**——需全局选中文本监听 + 轻量悬浮窗：
   - 类似 macOS 词典弹窗，选中文本后 200ms 延迟弹翻译结果
   - 预估工作量：3-5 天（需系统级文本选中监听）

### 🟢 P2：低优先级（锦上添花）

6. **剪贴板监听翻译**——watcher 已有，加翻译分支
7. **翻译历史/生词本**——DB 表 + CRUD
8. **术语词典**——DB 表 + 翻译时注入
9. **回译**——反向调用翻译引擎
10. **TTS 朗读译文**——翻译完成后调 TTS

### ⚪ 不建议做

- 网页沉浸式翻译——octopus 非浏览器扩展，定位不同
- PDF/文档排版翻译——独立产品领域
- 视频配音流水线——超出桌面工具定位

---

## 五、octopus 翻译的独特优势（vs 竞品）

octopus 翻译有几个竞品不具备的差异化优势：

1. **语音识别 + 翻译一体化**——ASR 流式识别 + 实时翻译双语字幕，Pot/STranslate 等纯翻译工具没有 ASR
2. **本地离线翻译**——Opus-MT（30M 极轻量）+ m2m100（100+ 语言），大多数竞品本地翻译依赖大模型
3. **LLM 翻译 + 润色串联**——翻译完成后可直接 LLM 润色，竞品通常是独立功能
4. **录屏字幕翻译**——录屏 ASR → 字幕翻译 → LLM 润色 → SRT 导出，完整链路
5. **CompactEditor 双语对照**——左右分栏编辑 + Markdown 预览，翻译结果可直接编辑保存

建议优先利用这些优势，把「语音/录屏翻译」做深，而非与 Pot/STranslate 在「划词翻译」赛道正面竞争。
