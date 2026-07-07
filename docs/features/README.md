# 功能特性说明书总览

> 本目录存放各功能域的特性说明书——描述"当前是什么"（功能行为、接口、约束），
> 不描述"怎么设计的"（设计决策记录在已归档的 superpowers specs 中，git history 可追溯）。
>
> 与 [architecture.md](../architecture.md) 的关系：architecture.md 是全局结构文档，
> 本目录各文件是各功能域的详细特性说明。

## 文档索引

| 文档 | 功能域 | 说明 |
|---|---|---|
| [asr-engine.md](./asr-engine.md) | ASR 引擎 | 本地引擎（Paraformer/Zipformer/Whisper/Qwen3-ASR/SenseVoice）、云端引擎（Aliyun/ByteDance/Tencent/Baidu）、VAD 分段、流式/离线模式 |
| [coordinator.md](./coordinator.md) | 录音协调器 | Coordinator 状态机、Stage/Command、引擎分支（streaming/cloud/VAD segmented）、录音→处理→粘贴→润色流水线 |
| [clipboard.md](./clipboard.md) | 剪贴板管理 | 剪贴板历史、监听线程、图片/文本/OCR/语音条目、FTS5 搜索、清理策略 |
| [screenshot.md](./screenshot.md) | 截图系统 | 区域截图、滚动截图（拼接引擎 NCC+Sobel）、标注工具栏、贴图浮窗（pin_window）、截图 OCR |
| [ocr.md](./ocr.md) | OCR 识别 | paddle-ocr 管线（det→cls→rec）、ONNX Runtime 后端、文本块叠加显示 |
| [result-window.md](./result-window.md) | 结果窗口 | ASR 识别结果展示、流式追加、编辑态、闪烁光标、润色集成、快捷键 |
| [compact-editor.md](./compact-editor.md) | 统一编辑器 | tab 栏（文本/图片/语音）、编辑保存、与剪贴板历史联动 |
| [db-and-config.md](./db-and-config.md) | 数据持久化与配置 | SQLite schema（models + transcriptions + clipboard_history）、config.yaml、RuntimeConfig、DB 写入队列 actor |
| [desktop-app.md](./desktop-app.md) | 桌面应用集成 | Tauri 2 架构、窗口管理、快捷键、托盘菜单、平台特性（macOS/Windows/Linux） |

## 维护规则

- **功能行为变化时同步更新**——这些是 vibecoding 的参考，过时描述会误导
- **不记录设计决策**——决策动机在 git history + commit message 中
- **与 architecture.md 保持一致**——architecture.md 是结构层，features 是行为层
