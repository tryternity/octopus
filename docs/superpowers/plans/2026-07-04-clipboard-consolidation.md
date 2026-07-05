# 剪贴板管理整合需求实施计划

> 日期：2026-07-04
> 状态：✅ 完成（全部 Task✅，`668300d` z-sync 回写，已合 main）
> 分支：`image-viewer-perf`（已合 main）

## 需求总览

废弃独立识别记录管理，整合到剪贴板管理。所有文本截断 200 字。加链接检测/复制图标/级联删除。

## Task 分解

### Task 1: 所有文本截断 200 字 ✅

- ClipboardItem 浮窗 + ClipboardPanel 管理页：text/ocr/asr 截断 200 字 + ……

### Task 2: 左侧导航改名 + 废弃识别记录管理 ✅

- Settings 左侧「剪贴板」→「剪贴管理」
- 删除「识别记录」tab + HistoryPanel import

### Task 3: 剪贴板条目元信息 ✅

- 浮窗：语音条目显示时间戳（不显示引擎）
- 管理页：语音条目显示「时间戳 · 引擎」

### Task 4: 语音条目可编辑 ✅

- 无需改（CompactEditor 已支持 source=clipboard text 编辑）

### Task 5: 级联删除 transcriptions ✅

- delete_clipboard_item / delete_clipboard_items / clear_clipboard_history 加 cascade_delete_transcriptions

### Task 6: 链接检测 + 打开 ✅

- 文本条目检测 http/https → 链接图标 → openUrl 打开浏览器（tauri-plugin-opener）

### Task 7: 复制图标 → 合并到类型图标 ✅

- 类型图标 = 单击复制（frontend-design 重设计后合并，去掉独立复制图标）

### 额外：列表项重设计（frontend-design）✅

- 合并图标/统一操作/图片放大/间距优化/分隔线柔和

## 实施记录

| Task | commit | 说明 |
|------|--------|------|
| 1+2 | `0dc7c97` | 截断 200 字 + 改名剪贴管理 + 废弃识别记录管理 |
| 5 | `b968aef` | 级联删除 transcriptions |
| 6+7 | `71cc941` | 链接检测（window.open）+ 复制图标 |
| — 浮窗引擎→时间戳 | `8e1fa38` | 语音条目浮窗显示时间戳 |
| — 管理页元数据顺序 | `694f185` | 时间戳 · 引擎 |
| — 链接 openUrl 修复 | `d0a2935` | tauri-plugin-opener Rust 插件 + ACL |
| — 复制图标常显 | `7af2cb2` | opacity-30 常显 |
| — 列表项重设计 | `a01cee4` | frontend-design 重做 |
