# 剪贴板管理整合需求实施计划

> 日期：2026-07-04
> 状态：📋 设计中
> 分支：`image-viewer-perf`

## 需求总览

废弃独立识别记录管理，整合到剪贴板管理。所有文本截断 200 字。加链接检测/复制图标/级联删除。

## Task 分解

### Task 1: 所有文本截断 200 字（最简单，先做）

- ClipboardItem 浮窗：text/ocr/asr 条目文本截断 200 字 + ……
- ClipboardPanel 管理页：同上

### Task 2: 左侧导航改名 + 废弃识别记录管理

- Settings 左侧 "剪贴板" → "剪贴管理"
- 删除 "识别记录" tab + HistoryPanel 组件
- HistoryPanel 的「查看」入口（openCompactEditorTab transcription）保留——从其他入口进

### Task 3: 剪贴板条目元信息

- ClipboardItem 浮窗：语音条目显示时间戳 + 语音秒数（不显示引擎/模型）
- ClipboardPanel 管理页：语音条目显示时间戳 + 引擎 + 语音秒数（不显示润色状态）
- 后端：clipboard_history 已有 engine/model 列；duration_ms 需要从 transcriptions 表关联查或冗余存

### Task 4: 语音条目编辑

- 语音条目（source=asr）在 CompactEditor 里可编辑
- 编辑只改 clipboard_history.content（现有 set_clipboard_item_text 已支持）
- 不影响 transcriptions 表（transcriptions 是原始记录，clipboard 是展示层）

### Task 5: 级联删除 transcriptions

- 删除 clipboard_history source=asr 条目时，同时删 transcriptions 表对应记录（transcription_id 关联）
- 后端 delete_clipboard_item 加级联逻辑

### Task 6: 链接检测 + 打开

- 前端：检测文本是否为 URL（http/https 开头）
- 浮窗 + 管理页：URL 条目加链接图标，点击 openUrl 打开浏览器

### Task 7: 复制图标

- ClipboardItem 浮窗：类型图标后加复制图标
- 两个图标点击都触发复制（现有左侧类型图标单击已复制，复制图标做同样的事）

## 依赖关系

Task 1-2 独立可并行。Task 3 需要 duration_ms 数据源确认。Task 5 依赖 Task 2（废弃后剪贴板管理是唯一删除入口）。
