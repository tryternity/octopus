# 剪贴板预览面板设计

> **状态**：设计阶段
> **日期**：2026-07-12
> **scope**：为剪贴板浮窗新增独立预览窗口，跟随鼠标 hover / 键盘 ↑↓ 选中条目实时展示完整内容

---

## 1. 背景与动机

### 1.1 现状

剪贴板浮窗（300×600 无边框透明置顶）列表中每条目只显示截断内容（~200 字 / `line-clamp-1`）。查看完整内容需打开 CompactEditor（点击「编辑」/「预览」按钮）。键盘 ↑↓ 导航选中条目后，无快速预览途径。

### 1.2 目标

- 选中条目（hover 或键盘 ↑↓）时，在浮窗旁弹出预览窗口展示完整内容
- 预览窗口跟随选中变化实时更新
- 浮窗有焦点时常驻显示，失焦时自动隐藏
- 根据浮窗位置自动选择左/右弹出方向

---

## 2. 架构设计

### 2.1 总览

```
剪贴板浮窗 (300×600)          预览窗口 (360×自适应, max 600)
┌──────────────┐              ┌─────────────────────┐
│ 标题栏        │              │                     │
│ 搜索 + 过滤   │    ←或→     │  完整内容展示区      │
│ 条目列表      │──────────────│  文本可滚动 /       │
│ ↓↑ 选中条目   │  emit 同步   │  图片缩略图          │
└──────────────┘              └─────────────────────┘
```

**窗口属性**（与浮窗同风格）：
- 无边框、圆角、透明、置顶
- `skip_taskbar`、`resizable: false`
- 不抢焦点（`accept_first_mouse: false`）
- `always_on_top: true`

### 2.2 预览窗口位置计算

浮窗显示时，Rust 侧计算预览窗口位置：

1. 读取浮窗 `outer_position()` + `outer_size()`（物理坐标）
2. 读取当前显示器 `Monitor::position()` + `Monitor::size()`
3. 右侧剩余空间 ≥ 360px → 预览窗口贴浮窗右侧
4. 右侧不够 → 贴浮窗左侧
5. 两侧都不够 → 弹空间更大的一侧，预览宽度收缩

坐标全部统一到逻辑坐标（物理 ÷ `scale_factor()`），与 Tauri `inner_position` 一致。

### 2.3 生命周期

| 事件 | 预览窗口动作 |
|------|-------------|
| 浮窗 `Focused(true)` | 显示 + 定位 |
| 浮窗 `Focused(false)`（非预览窗口获焦） | 隐藏 |
| 浮窗隐藏（`hide()`） | 隐藏 |
| 选中条目变化（hover / ↑↓） | emit 更新内容 |
| 列表为空 | 显示空状态提示 |

**焦点守卫**：浮窗失焦后短暂延迟（200ms）再隐藏预览，避免浮窗→预览窗口的焦点抖动导致预览闪退。

---

## 3. 组件设计

### 3.1 Rust：`clipboard_preview_window.rs`（新建）

```rust
const PREVIEW_W: f64 = 360.0;
const PREVIEW_MAX_H: f64 = 600.0;
const WINDOW_LABEL: &str = "clipboard_preview_window";

/// 创建预览窗口（透明无边框置顶，不抢焦点）
pub fn create_preview_window(app: &AppHandle) -> Result<()>;

/// 显示预览窗口 + 计算位置
pub fn show_preview_window(app: &AppHandle);

/// 隐藏预览窗口
pub fn hide_preview_window(app: &AppHandle);

/// 更新预览内容（emit 到前端）
pub fn update_preview_content(app: &AppHandle, item: &ClipboardItem);
```

**位置计算**：
```rust
fn compute_preview_position(app: &AppHandle) -> (f64, f64) {
    // 1. 获取剪贴板浮窗位置 + 尺寸（逻辑坐标）
    let clip_win = app.get_webview_window("clipboard_window")?;
    let (cx, cy) = clip_win.outer_position()?;  // 物理
    let (cw, ch) = clip_win.outer_size()?;
    let sf = clip_win.scale_factor()?;
    let (cx, cy, cw, ch) = (cx/sf, cy/sf, cw/sf, ch/sf);  // → 逻辑

    // 2. 获取显示器边界
    let monitor = clip_win.current_monitor()?;
    let (mw, mh) = monitor.size();  // 物理
    let (mw, mh) = (mw/sf, mh/sf);

    // 3. 右侧空间
    let right_space = mw - (cx + cw);
    if right_space >= PREVIEW_W + 8.0 {
        // 弹右边
        (cx + cw + 8.0, cy)
    } else if cx >= PREVIEW_W + 8.0 {
        // 弹左边
        (cx - PREVIEW_W - 8.0, cy)
    } else if right_space >= cx {
        // 右侧更大，贴右（可能部分溢出）
        (cx + cw + 4.0, cy)
    } else {
        // 左侧更大，贴左
        (cx - PREVIEW_W - 4.0, cy)
    }
}
```

### 3.2 Tauri 事件

| 事件 | 方向 | Payload | 说明 |
|------|------|---------|------|
| `clipboard-preview://update` | Rust → 前端 | `{ id, itemType, content, refData }` | 选中条目变化时推送完整内容 |
| `clipboard-preview://show` | Rust → 前端 | `null` | 窗口显示 |
| `clipboard-preview://hide` | Rust → 前端 | `null` | 窗口隐藏（清空内容） |
| `update_clipboard_preview` | 前端 → Rust | `{ id: number }` | 前端请求更新预览（hover / 键盘选中时） |

### 3.3 Tauri Command

```rust
#[tauri::command]
pub fn update_clipboard_preview(app: AppHandle, id: i64) {
    // 从 DB 读 item → emit clipboard-preview://update
}
```

前端在选中条目变化时调用此命令。Rust 侧从 DB 查询完整 item 数据（含 image 的 `get_image_thumb`）后 emit 给预览窗口。

### 3.4 前端：`pages/ClipboardPreview/index.tsx`（新建）

```typescript
export default function ClipboardPreview() {
  const [item, setItem] = useState<PreviewItem | null>(null);
  const [thumbSrc, setThumbSrc] = useState<string | null>(null);

  // 监听更新事件
  useTauriEvent("clipboard-preview://update", (payload) => {
    setItem(payload as PreviewItem);
  });

  // 图片类型拉缩略图
  useEffect(() => {
    if (item?.itemType === "image") {
      invoke<string>("get_image_thumb", { id: item.id })
        .then(setThumbSrc)
        .catch(() => setThumbSrc(null));
    } else {
      setThumbSrc(null);
    }
  }, [item]);

  if (!item) return <EmptyState />;

  return (
    <div className="flex flex-col h-full">
      {/* 类型标签 + 元数据 */}
      <Header item={item} />
      {/* 内容区 */}
      <Content item={item} thumbSrc={thumbSrc} />
    </div>
  );
}
```

**内容渲染规则**：

| itemType | 展示方式 |
|----------|---------|
| text / ocr | 完整纯文本，等宽字体，可滚动 |
| voice (asr) | 完整纯文本 + 润色状态标签 |
| image | `get_image_thumb` 缩略图居中展示 |
| file | `formatFilePaths` 路径列表 |

### 3.5 前端：`Clipboard/index.tsx` 改动

```typescript
// 选中变化时更新预览
useEffect(() => {
  if (selectedIndex === null) return;
  const item = items[selectedIndex];
  if (item) {
    invoke("update_clipboard_preview", { id: item.id });
  }
}, [selectedIndex, items]);
```

hover 条目时也触发更新（`onSelect` 已被 `onClick` 调用，hover 需新增 `onMouseEnter`）。

### 3.6 Rust：`main.rs` 改动

浮窗窗口事件中集成预览窗口生命周期：

```rust
// clipboard_window 的 WindowEvent
WindowEvent::Focused(true) => {
    clipboard_preview_window::show_preview_window(app);
}
WindowEvent::Focused(false) => {
    // 延迟 200ms 隐藏（防焦点抖动）
    let app_clone = app.clone();
    std::thread::spawn(move || {
        std::thread::sleep(Duration::from_millis(200));
        // 再次检查：如果此时浮窗又获得了焦点，不隐藏
        clipboard_preview_window::hide_preview_window(&app_clone);
    });
}
```

### 3.7 Capabilities

`capabilities/default.json` 的 `windows` 数组加入 `"clipboard_preview_window"`。

---

## 4. 物理/逻辑坐标注意事项

预览窗口位置计算使用逻辑坐标（物理 ÷ `scale_factor()`），与 Tauri `set_position(LogicalPosition)` 一致。多显示器不同 DPI 下 `current_monitor()` 返回正确的显示器边界。

---

## 5. 不做的事

- 预览内容不可编辑（只读展示）
- 不改变剪贴板浮窗的现有布局和尺寸
- 预览窗口不可拖拽、不可调大小（跟随浮窗定位）
- 不做分页（预览只展示当前选中条目的完整内容）
- 不做 markdown/富文本渲染（纯文本展示）

---

## 6. 测试策略

- `cargo test` — Rust 单测（位置计算逻辑可提取为纯函数测试）
- 前端单测 — 预览组件渲染各类型条目
- 手动验证：
  - 浮窗显示时预览窗口弹出，位置正确（左/右）
  - hover / ↑↓ 切换时预览内容实时更新
  - 浮窗失焦时预览窗口隐藏
  - 多显示器 + 不同 DPI 下位置正确
  - 图片条目缩略图正常展示
