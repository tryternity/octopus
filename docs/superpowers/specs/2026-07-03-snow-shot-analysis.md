# snow-shot 功能对比分析

> 项目：[mg-chao/snow-shot](https://github.com/mg-chao/snow-shot)
> 本地路径：`/Users/wudarui/workspace/agent/snow-shot`
> 分析日期：2026-07-03
> 技术栈：Tauri 2 + React + Rust + Excalidraw + Ant Design

## 1. snow-shot 功能全景

snow-shot 是一个功能极其丰富的桌面截图/标注工具（Windows/macOS），以 Excalidraw 为绘制核心。按功能域分类：

### 1.1 截图

| 功能 | 说明 |
|------|------|
| 区域截图 | 基础选区截图，含跨屏选区 |
| 全屏截图 | 整屏捕获 |
| 滚动截图 | 自动/手动滚动拼接长截图（Rust 原生实现，FAST 角点 + 描述子 + HNSW 近邻索引缝合；**非 NCC**，2026-07-06 订正） |
| 延时截图 | 定时截图 |
| HDR 截图 | HDR 显示器宽色域捕获（Linear/None 色彩算法） |

### 1.2 标注工具（DrawState 枚举，17 种工具）

| 工具 | octopus 对应 |
|------|-------------|
| 选择 | ✅ 有 |
| 矩形 | ✅ 有 |
| 菱形 | ❌ 缺 |
| 椭圆 | ✅ 有 |
| 箭头 | ✅ 有 |
| 线条 | ✅ 有 |
| 画笔 | ✅ 有 |
| 文本 | ✅ 有 |
| 序列号（自动编号） | ✅ 已有（`Annotation.type = "number"`，带圆圈数字） |
| **模糊/马赛克** | ❌ 缺 |
| **橡皮擦** | ❌ 缺 |
| **水印** | ❌ 缺 |
| **高亮** | ❌ 缺 |
| **自由绘制模糊** | ❌ 缺 |
| **激光笔**（演示指引） | ❌ 缺 |
| 撤销/重做 | ✅ 撤销有，重做缺 |
| 颜色拾取器 | ❌ 缺 |

### 1.3 OCR

| 功能 | snow-shot | octopus |
|------|-----------|---------|
| OCR 引擎 | RapidOCR v4/v5（paddle_ocr_rs） | PP-OCRv6（paddle，ocr-rs crate） |
| OCR 模型管理 | 插件式（plugin path + 内存加载 + 热启动） | 固定内置模型 |
| 识别后操作 | 可配置：复制文本/复制并关闭/翻译/转 HTML/转 Markdown | 仅插入剪贴板条目 + 打开编辑器 |
| **OCR 文本块可视化** | ✅ 选区内显示文本块边界框 + 可逐块选择/编辑 | ❌ 纯文本返回 |
| **OCR + 翻译** | ✅ 识别后直接翻译（多翻译引擎） | ❌ 缺 |
| **OCR → 视觉模型** | ✅ 图片 → LLM → HTML/Markdown | ❌ 缺 |
| 方向检测 | ✅ detectAngle（旋转文字识别） | ❌ 缺 |
| 多模型切换 | ✅ RapidOCR v4/v5 切换 | ❌ 固定 |

### 1.4 其他独有功能

| 功能 | 说明 |
|------|------|
| **视频录制** | 区域录屏（FFmpeg），MP4/GIF/APNG/WebP，麦克风+系统音频 |
| **固定贴图** | 截图后钉在屏幕上（类似 Snipaste） |
| **全屏画板** | 整屏可绘制的白板 |
| **鼠标穿透** | 固定贴图/全屏画板的鼠标穿透模式 |
| **翻译工具** | 独立翻译窗口（多翻译引擎） |
| **AI 对话** | OpenAI 兼容 API 的聊天窗口（支持 Workflow） |
| **截图历史** | 截图历史管理（可配置保留时间） |
| **S3 上传** | 截图直接上传到 S3 |
| **主题/外观** | 暗色/亮色主题 |
| **国际化** | 中文简繁 + 英文 |
| **插件系统** | OCR/翻译/AI 等以插件形式加载 |
| **Excalidraw 核心** | 基于 Excalidraw fork 的完整矢量绘制引擎 |

---

## 2. octopus 功能缺失对比

### 高价值缺失（建议借鉴）

| 功能 | 价值 | 复杂度 |
|------|------|--------|
| **模糊/马赛克工具** | 截图标注高频需求（隐藏敏感信息） | 中——canvas 像素操作 + 区域选择 |
| **OCR 文本块可视化** | 识别后直接在图上显示文本块边界 + 可逐块编辑/复制 | 高——需要 OCR 返回坐标（当前只返回纯文本） |
| **重做** | 标注历史管理完整性 | 低——镜像 undo 的 stack |

### 中价值缺失

| 功能 | 价值 | 复杂度 |
|------|------|--------|
| **高亮工具** | 标注时高亮区域（半透明色块） | 低——半透明 rect |
| **方向检测 OCR** | 旋转/倾斜文字识别 | 中——OCR 引擎需支持 angle detection |
| **延时截图** | 定时截屏 | 低——setTimeout |

### 低价值 / 不建议借鉴

| 功能 | 原因 |
|------|------|
| 视频录制 | 与 octopus 定位（ASR 工具）偏离太远 |
| AI 对话/翻译工具 | 功能发散，octopus 有 LLM 润色已够 |
| Excalidraw 核心 | 过重（~500KB），octopus 的 SVG overlay 已够用 |
| S3 上传 | 场景不匹配 |
| 插件系统 | 过度工程 |

---

## 3. 借鉴意义

### 3.1 OCR 增强方向（最有价值）

snow-shot 的 OCR 返回 `text_blocks`（含 `box_points` 坐标 + `text` + `text_score`），支持：
- 在截图上**可视化显示文本块**
- 用户**逐块选择/编辑**识别结果
- OCR 结果**直接翻译**或**送 LLM 转 HTML/Markdown**

octopus 当前 OCR 只返回纯文本字符串。如果后端 `ocr_image` 改为返回带坐标的文本块，前端可以在 ImagePreview 里直接叠加文本块可视化层——这在处理超长滚动截图时特别有用（识别整页文字 + 定位）。

### 3.2 标注工具补全

**模糊/马赛克**是截图标注的刚需——隐藏密码/隐私信息。实现方式：
- 矩形模糊：选定区域像素降采样 + 放大
- 自由绘制模糊：沿画笔轨迹的像素模糊

**序列号**工具已有数据类型（`Annotation.type = "number"`），只需：
- 工具栏加序号按钮
- 点击放置时自动递增编号（读取已有 number 标注的最大值 +1）

### 3.3 架构对比

| 维度 | snow-shot | octopus |
|------|-----------|---------|
| 绘制引擎 | Excalidraw fork（完整矢量编辑器） | 原生 Canvas 2D + SVG overlay |
| 性能 | Excalidraw 内部 WebGL 渲染，复杂但强大 | 视口渲染 + SVG overlay，轻量高效 |
| UI 框架 | Ant Design（重量级） | 内联 style + Tailwind（轻量） |
| Rust 后端 | 模块化（tauri-commands 拆 crate） | 单 crate（desktop） |
| OCR | paddle_ocr_rs（Rust 原生） | ocr-rs（Rust 原生） |
| 滚动截图 | FAST 角点 + 描述子 + HNSW 近邻索引缝合（Rust；**非 NCC**，2026-07-06 订正） | CAPX（canvas-anchored + Sobel/NCC 模板匹配，已有） |

**octopus 的优势**：ASR 是核心能力（snow-shot 没有），图片预览的视口渲染 + SVG overlay 在超大图上比 Excalidraw 更轻量。
