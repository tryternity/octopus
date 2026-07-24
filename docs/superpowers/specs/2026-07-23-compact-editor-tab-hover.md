# CompactEditor Tab Hover Meta 浮层设计

> **日期**：2026-07-23
> **状态**：✅ 已实现（e2e 验证通过 2026-07-23）

---

## 0. 目标

CompactEditor 的每个 tab 对应不同来源的内容（剪贴板历史 / 语音识别 / 临时 / 磁盘文件），但用户从 tab 标题（前 5 字 + id hex）看不出它是什么、来自哪里。在 tab 上加 hover 浮层，展示该 tab 内容的 meta 信息。

## 1. 各 source 的 meta 信息

| source | 浮层内容 |
|---|---|
| `clipboard`（text） | 来源：剪贴板历史 · ID · 字数 |
| `clipboard`（image） | 来源：剪贴板历史 · ID · 尺寸 WxH |
| `transcription` | 来源：语音识别 · ID · 只读 |
| `temp` | 来源：临时（未保存）· 状态：未保存 |
| `file` | 来源：磁盘文件 · 路径 · 编辑/已保存状态 |

**纯展示，不查 DB**——只用 Tab 对象已有字段（itemId / text / imgWidth / imgHeight / filePath / originalText），避免 hover 延迟。

## 2. UI 设计

**使用 frontend-design skill**

- **触发**：鼠标 hover tab → 500ms 延迟后显示浮层（防抖，避免快速划过闪烁）
- **位置**：向下弹出（tab 栏下方内容区有空间）。用 **React Portal + `position: fixed`** 渲染到 `document.body`——避免被 tab 栏 `overflow-x-auto` 裁剪（原 `position: absolute` 向上弹被裁掉不可见）
- **消失**：鼠标离开 tab → 立即隐藏（tab 栏窄，不像 PromptEditor 需要移到浮层操作）
- **视觉**：source 色点（file=emerald / transcription=violet / clipboard=muted / temp=amber）+ source 名 + key:value meta 行
- **宽度**：min-w-[180px] max-w-[280px]，文件路径 `break-all` wrap 展示（不截断尾部文件名）+ 等宽字体

### source 色条映射

| source | 色点 | 含义 |
|---|---|---|
| `file` | emerald-500 | 磁盘文件（可编辑可保存） |
| `transcription` | violet-500 | 语音识别（只读） |
| `temp` | amber-500 | 临时（未保存） |
| `clipboard` | muted-foreground | 剪贴板历史 |

与 tabIcon 的颜色一致（file=emerald / transcription=violet）。

## 3. 组件结构

```
TabHoverCard.tsx（新建）
  ├─ source 色点 + 来源名
  └─ MetaRow × N（label : value 紧凑排列）
```

集成到 `CompactEditor/index.tsx` 的 tab 渲染区：每个 tab `<div>` 加 `onMouseEnter`/`onMouseLeave`，hoveredTabKey state 控制浮层显隐。

## 4. 不变量

| # | 不变量 | 保证方式 |
|---|---|---|
| INV-T1 | 浮层纯展示不查 DB（无延迟） | 只用 Tab 对象已有字段 |
| INV-T2 | hover 浮层不影响 tab 点击/关闭 | 浮层 pointer-events-none 或 z-index 不挡按钮 |
| INV-T3 | 500ms 延迟显示（防抖） | setTimeout，鼠标离开 tab 清除 |

## 5. 已知限制

- 不展示 created_at（clipboard tab 未带时间字段，避免查 DB 延迟）
- 不展示 meta_info 详细字段（char_count 等，用 `text.length` 代替）
- 浮层不随 tab 滚动（tab 栏 overflow-x-auto 时浮层位置固定在 tab 上方）
