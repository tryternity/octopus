# i18n 全面覆盖设计

> **状态**：设计阶段
> **日期**：2026-07-12
> **scope**：将 octopus 桌面应用全部 UI 文案（前端 6 个 page 模块 + Settings 全面板 + Rust tray.rs）纳入 i18n 体系；locale 文件从 JSON flat 迁移为 YAML 嵌套；新增 Rust 侧 i18n 能力

---

## 1. 背景与动机

### 1.1 现状

目前仅有图文编辑器（CompactEditor）完整实现了 i18n：

- **i18n 引擎**：手写 ~67 行 `lib/i18n.ts`，无第三方依赖
- **locale 文件**：`src/locales/zh-CN.json` + `en.json`，flat dotted keys，共 22 个 key
- **已覆盖**：CompactEditor 完整、Settings/GeneralPanel 的语言选择器（部分）
- **Settings/GeneralPanel 语言切换**：调用 `setVal("ui_language", lang)` 写入 config + `setLocale(lang)` 更新前端

### 1.2 问题

其余 ~90% 的 UI 文案全部硬编码中文：

| 模块 | 位置 | 状态 |
|------|------|------|
| Screenshot | `pages/Screenshot/` | 全部硬编码 |
| ImagePreview | `pages/ImagePreview/` | 全部硬编码 |
| Result（ASR） | `pages/Result/` | 全部硬编码 |
| Clipboard | `pages/Clipboard/` | 全部硬编码 |
| ActionBar | `pages/ActionBar/` | 全部硬编码 |
| Settings（大部分面板） | `pages/Settings/` | 仅语言选择器有 i18n |
| Tray 托盘菜单 | `src/tray.rs`（Rust） | 全部硬编码 |

### 1.3 目标

- 将全部 UI 文案提取为 i18n key
- locale 文件从 JSON flat 迁移为 YAML 嵌套（支持注释、更优可读性）
- Rust 后端（tray.rs）也能使用同一份 locale 数据
- 语言切换时前端 + Rust 托盘菜单同步更新
- 为未来新增语言留好扩展点（当前仅 zh-CN + en）

---

## 2. 架构设计

### 2.1 总览

```
单一真相源：frontend/src/locales/{zh-CN,en}.yaml
    │
    ├── 前端：vite-plugin-yaml import → 嵌套对象 → flatten → translate(key, params?)
    │         ↑ initI18n() 从 config 读 ui_language
    │
    └── Rust：include_str! 编译期嵌入 → serde_yaml 解析 → flatten → t(key, params)
              ↑ 从 config 读 ui_language
```

**不变量**：
- locale YAML 是唯一真相源，前后端绝不各维护一套
- 语言选择始终从 `config.ui_language` 读取
- `translate()` / `t()` 对外接口保持 flat dotted key（如 `"screenshot.tool.rect"`）

### 2.2 locale 文件路径

保持现有位置 `crates/desktop/frontend/src/locales/`，只换格式：

```
frontend/src/locales/
├── zh-CN.yaml    # 原 zh-CN.json
└── en.yaml       # 原 en.json
```

前端 import 路径不变，Rust 侧用 `include_str!("../frontend/src/locales/zh-CN.yaml")` 编译期嵌入。

### 2.3 YAML 格式规范

嵌套结构，按模块分块注释，key 前缀即模块名：

```yaml
# ════════ Editor 图文编辑器 ════════
editor:
  undo: 撤销
  redo: 重做
  charCount: ${n} 字
  save: 保存
  # ... 其他 editor key

# ════════ Tab 标签页 ════════
tab:
  image: 图片
  close: 关闭

# ════════ ActionBar 悬浮操作栏 ════════
actionbar:
  processing: 处理中
  timeout: 请求超时（${n}s）

# ════════ Tray 托盘菜单 ════════
tray:
  startAsr: 语音识别（${shortcut}）
  stopAsr: 停止识别
  processing: 处理中…
  screenshot: 开始截图（${shortcut}）
  clipboard: 剪  贴  板（${shortcut}）
  compactEditor: 图文编辑
  settings: 系统管理
  quit: 退出系统
  engineInfo: 引擎  ${engine} · ${mode}
```

**插值语法不变**：`${name}` 占位符，与现有 `translate()` 逻辑一致。

### 2.4 flatten 算法

YAML 嵌套对象递归拍平为 flat dict：

```typescript
// 输入: { screenshot: { tool: { rect: "矩形" } } }
// 输出: { "screenshot.tool.rect": "矩形" }
function flatten(obj: Record<string, unknown>, prefix = ""): Record<string, string> {
  const result: Record<string, string> = {};
  for (const [key, val] of Object.entries(obj)) {
    const fullKey = prefix ? `${prefix}.${key}` : key;
    if (typeof val === "string") {
      result[fullKey] = val;
    } else if (typeof val === "object" && val !== null) {
      Object.assign(result, flatten(val as Record<string, unknown>, fullKey));
    }
  }
  return result;
}
```

Rust 侧用相同逻辑实现。

---

## 3. 组件设计

### 3.1 前端 i18n.ts 改造

**改动点**：
1. import 从 `.json` 改为 `.yaml`（需 `vite-plugin-yaml`）
2. 新增 `flatten()` 函数
3. DICTS 构建方式从直接 import flat JSON 改为 `flatten(yamlObj)`
4. `translate()`、`setLocale()`、`useT()`、`t`、`getLocale()` — **接口完全不变**

**对外 API 不变**（所有现有调用方零改动）：
```typescript
export async function initI18n(): Promise<void>
export function setLocale(locale: Locale): void
export function getLocale(): Locale
export function useT(): (key: string, params?: Record<string, string | number>) => string
export const t: typeof translate
```

**Locale 类型扩展**：后续新增语言时，在此 type 加一个值即可：
```typescript
type Locale = "zh-CN" | "en";
// 未来: | "ja" | "ko" ...
```

### 3.2 Rust i18n 模块

新增 `crates/desktop/src/i18n.rs`：

```rust
/// 编译期嵌入 locale YAML
const ZH_CN_YAML: &str = include_str!("../frontend/src/locales/zh-CN.yaml");
const EN_YAML: &str = include_str!("../frontend/src/locales/en.yaml");

type Dict = std::collections::HashMap<String, String>;

/// 从 config.ui_language 初始化全局 locale dict
pub fn init(ui_language: &str) { ... }

/// flat key 查找 + ${name} 插值
pub fn t(key: &str, params: &[(&str, &str)]) -> String { ... }

/// 重新加载（语言切换时调用）
pub fn reload(ui_language: &str) { ... }
```

**实现细节**：
- 用 `serde_yaml` 解析 YAML 为 `serde_json::Value`（嵌套 map）
- 用递归函数 flatten 为 `HashMap<String, String>`
- 全局存储使用 `once_cell::sync::Lazy<Mutex<Dict>>`
- 插值用简单的 `replace("${name}", value)` 循环（与前端一致）
- key 缺失时返回 key 本身（与前端一致）

**依赖**：在 `crates/desktop/Cargo.toml` 加 `serde_yaml`（若 infra 已有则直接用）。

### 3.3 tray.rs 改造

**create_tray()**：
- 从 config 读 `ui_language`，调 `i18n::init()`
- 所有硬编码中文替换为 `i18n::t("tray.startAsr", &[("shortcut", &sc)])` 调用

**update_tray_label()**：
- `TrayState::Idle` → `i18n::t("tray.startAsr", ...)`
- `TrayState::Recording` → `i18n::t("tray.stopAsr", &[])`
- `TrayState::Processing` → `i18n::t("tray.processing", &[])`

**update_tray_engine_label()**：
- `i18n::t("tray.engineInfo", &[("engine", engine_name), ("mode", engine_mode)])`

### 3.4 语言切换 → 托盘重建

**流程**：
1. 用户在 Settings/GeneralPanel 切换语言
2. 前端：`setVal("ui_language", lang)` 写 config + `setLocale(lang)` 更新前端
3. 前端：新增 `emit("locale-changed", lang)` 通知 Rust 后端
4. Rust：监听 `locale-changed` 事件 → `i18n::reload(lang)` → 重建托盘菜单

**托盘重建**：现有代码已用 `MenuItem::set_text()` 动态更新文案（见 `update_tray_label`）。采用相同方式：
- 扩展 `TrayItems` 结构体，存储**全部** MenuItem handle（当前仅 toggle / engine_info / screenshot，需补 clipboard / compact_editor / settings / quit）
- 新增 `rebuild_tray_labels()` 函数，遍历全部 item 逐个 `set_text()` 更新为当前语言文案
- 菜单事件绑定不受影响（ID 不变）

---

## 4. 模块提取顺序

从简单到复杂，分阶段推进。每个模块的步骤一致：
1. 遍历该模块所有 `.tsx` 文件，找出硬编码中文
2. 在 `zh-CN.yaml` 和 `en.yaml` 中新增对应 key
3. 替换为 `t("module.key")` 调用
4. 运行测试验证

| 顺序 | 模块 | 估计 key 数 | 说明 |
|------|------|------------|------|
| — | **Phase 1：基建** | — | 见第 3 节 |
| 1 | ActionBar | ~5 | 最少字符串，快速验证基建 |
| 2 | Result（ASR） | ~25 | 模式标签 + 状态文案 |
| 3 | Screenshot | ~35 | 工具标签 + 状态 |
| 4 | ImagePreview | ~45 | 工具栏 + popover + 状态 |
| 5 | Clipboard | ~50 | 最多字符串（filter tabs + popover + 清理确认） |
| 6 | Settings（全面板） | ~80 | 最重，但纯静态文案 |
| 7 | tray.rs | ~10 | Phase 1 已建好基建，直接提取 |

总计约 250 个 key（含现有 22 个）。

---

## 5. 前端依赖变更

### 5.1 新增 Vite 插件

需要一个能让 Vite import `.yaml` 文件的插件。常用选项：

- `@modyfi/vite-plugin-yaml`（ESM，Vite 5+ 兼容）
- `vite-plugin-yaml`（较老，CommonJS）

实施时验证与 Vite 8 的兼容性，优先选 ESM 兼容的。如果两者都不兼容 Vite 8，可手写一个极简 Vite 插件（用 `js-yaml` 解析 `.yaml` → 转为 JS 对象 export），代码不超过 20 行。

### 5.2 vite.config.ts 改动

```typescript
import yaml from "<chosen-yaml-plugin>";

export default defineConfig({
  plugins: [react(), tailwindcss(), yaml()],
  // ... 其余不变
});
```

### 5.3 Rust 依赖

检查 `serde_yaml` 是否已在 workspace 中使用。若没有，在 `crates/desktop/Cargo.toml` 添加。

---

## 6. 扩展点：新增语言

当前设计对未来新增语言很友好，仅需 3 步小改动：

1. 新增 `src/locales/ja.yaml`
2. `i18n.ts`：加一行 import + `Locale` type 加值 + `DICTS` 加 entry
3. `i18n.rs`：加一行 `include_str!` + `match` 分支

**不需要**：动态语言发现、插件化、懒加载——对 2-3 种语言规模是过度设计。

---

## 7. 不做的事

- 不引入第三方 i18n 库（i18next / react-intl / formatjs 等）——现有手写方案完全够用
- 不改现有 22 个 key 的命名——保持 `editor.*`、`tab.*`、`settings.uiLanguage*` 不变
- 不做 locale 文件的动态加载 / 懒加载——编译期嵌入 + import 即可
- 不改 `translate()` / `useT()` 的对外签名——所有现有调用方零改动
- 不处理 Rust 侧 tray.rs 以外的硬编码中文（如日志、error message）——这些不是 UI 文案

---

## 8. 测试策略

### 8.1 前端

- `i18n.test.ts` 现有测试全部保持通过（flatten 后的 flat key 查找行为不变）
- 新增 flatten 函数的单测（嵌套 → flat key 映射）
- 每个模块提取后，手动验证中文/英文切换正常

### 8.2 Rust

- `i18n.rs` 内联单测：YAML 解析 → flatten → key 查找 → 插值
- tray 重建：手动验证语言切换后托盘菜单文案更新

### 8.3 验证命令

```bash
# 前端测试
cd crates/desktop/frontend && npm test

# Rust 测试
cargo test -p octopus-desktop

# 构建验证
cargo build --release -p octopus-desktop --features embedded
```
