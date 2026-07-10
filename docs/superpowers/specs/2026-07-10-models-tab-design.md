# 模型管理页面 Tab 化 + 环境变量配置设计

> **状态**：设计完成，待实现
> **日期**：2026-07-10
> **scope**：设置页「模型管理」从单页面改为 5 tab（环境变量 / ASR / Text-Chat / OCR / 翻译），模型下载地址支持变量模板替换

---

## 1. 背景与动机

### 1.1 问题

当前模型管理页面（`ModelsPanel`）只有 ASR 可下载模型列表 + HF mirror 输入框。设置分散：
- ASR 模型选择在 GeneralPanel
- 润色模型（LLM）选择在 GeneralPanel
- OCR 模型选择在 GeneralPanel
- 下载地址前缀（mirror）只在 ModelsPanel
- 翻译没有独立入口（走 LLM prompt）

模型类型多了后单页面拥挤，下载地址配置不灵活（只能全局前缀拼接）。

### 1.2 目标

- 5 tab 分类管理：环境变量 / ASR / Text-Chat / OCR / 翻译
- 环境变量模板替换：模型 source 字段用 `{huggingface}` 引用，下载时替换为实际值
- GeneralPanel 的模型选择保留不动（集中管理），模型管理 tab 只管模型列表 + toggle

---

## 2. Tab 结构

```
[常量]  [ASR]  [Text/Chat]  [OCR]  [翻译]
```

### 2.1 Tab 1：常量（环境变量编辑器）

- DB `app_config` 表，`category='env'`
- 内置 3 个（不可删除、名称不可改，值可改）：
  - `huggingface` = `https://hf-mirror.com`
  - `modelscope` = `https://modelscope.cn`
  - `github` = `https://github.com`
- 用户可新增其他环境变量（key + value 都可改/删）
- 模型下载地址中用 `{huggingface}` 引用，下载时替换为实际值

### 2.2 Tab 2：语音识别

分**本地**和**云端**两个 section：
- **本地模型**（`is_local=1`）：现有 `list_downloadable_models` 列表，每行 名称 + category 标签 + 下载进度 + is_enabled 状态 + 下载/校验按钮
- **云端引擎**（`asr_engines` 过滤 `is_local=false`）：只读展示 label + current 标记（选择在 GeneralPanel）
- 顶部「当前使用」模型（只读，从 `asr_engines.find(e => e.current)` 读）

### 2.3 Tab 3：文本模型

分**本地**和**云端**两个 section：
- DB `models` 表 `domain='llm'` 列表，按 `is_local` 分区
- 每行：名称 + is_enabled toggle
- 顶部「当前使用」模型（只读，从 config `polish_llm` 读）

### 2.4 Tab 4：扫描识别

分**本地**和**云端**两个 section：
- DB `models` 表 `domain='ocr'` 列表，按 `is_local` 分区（OcrOption/OcrModelInfo 已加 `is_local` 字段）
- 每行：名称 + is_enabled toggle
- 顶部「当前使用」模型（只读，从 config `ocr_model` 读）

### 2.5 Tab 5：翻译模型

- 占位页面：「即将支持翻译 API / 本地翻译模型」
- 未来接入翻译引擎配置（DeepL / Google / 百度 / 本地 Argos）
- 虚线边框空状态 + 未来引擎列表预览（降低透明度）

### 2.6 视觉设计（frontend-design skill）

- **Tab 条**：胶囊式 pill tabs（选中=深色背景白字），替代文字+下划线
- **「当前使用」横幅**：左 voice 色条（`border-l-2 border-voice`）+ 浅 voice 背景
- **模型行**：左色条编码状态（voice=就绪/启用、border=禁用）+ hover 微卡片背景
- **本地/云端 section**：`CollapsibleSection` 组件——点击标题展开/收起（ChevronDown 旋转），默认展开，减少视觉干扰
- **EnvironmentTab**：变量行卡片化 + hover 显示删除按钮 + input 无边框 focus 出边框
- **宽度限制**：`max-w-[560px]`，字号 `text-[11px]` 提升信息密度
- **签名元素**：左侧色条与剪贴板面板 voice 色条一脉相承

---

## 3. 变量替换机制

### 3.1 模板格式

模型 `source`/`repo` 字段中的 `{variable}` 在下载时替换：

```
DB source: "{huggingface}/sherpa-onnx-models/silero_vad.onnx"
↓ 替换
实际 URL: "https://hf-mirror.com/sherpa-onnx-models/silero_vad.onnx"
```

### 3.2 替换实现

Rust 侧 `model_commands.rs` 的 `download_model` 中：
1. 读取 DB `app_config` `category='env'` 的所有 key-value
2. 对 source 字符串做 `{key}` → value 替换（遍历所有变量）
3. 未定义的 `{key}` 保留原样（不替换）

### 3.3 迁移

- 旧 `download_mirror`（category='setting'）→ 启动时检测：如果值非空，写入 `env.huggingface`，清空旧值
- 现有 ASR 模型 source 字段改为 `{huggingface}/...` 模板格式（db.sql seed）

---

## 4. 文件变更

### 4.1 后端

| 文件 | 变更 |
|------|------|
| `crates/infra/src/db.sql` | seed 加 3 行 env（huggingface/modelscope/github，category='env'）；ASR 模型 source 改为 `{huggingface}/...` 模板 |
| `crates/infra/src/db.rs` | `list_env_vars()` / `save_env_var(key, value, category)` / `delete_env_var(key)` |
| `crates/desktop/src/model_commands.rs` | `download_model` 改为模板替换；移除旧 `download_mirror` 前缀逻辑 |
| `crates/desktop/src/settings_commands.rs` | `get_env_vars` / `set_env_var` / `delete_env_var_cmd` Tauri 命令 |
| `crates/desktop/src/main.rs` | invoke_handler 注册新命令 |

### 4.2 前端

| 文件 | 变更 |
|------|------|
| `ModelsPanel.tsx` | 重构为 5 tab（Tabs 容器） |
| 新建 `Settings/Models/EnvironmentTab.tsx` | 变量编辑器（内置 3 个 + 用户自定义增删） |
| 新建 `Settings/Models/AsrTab.tsx` | 现有可下载 ASR 模型列表（从 ModelsPanel 提取） |
| 新建 `Settings/Models/LlmTab.tsx` | domain='llm' 模型列表 + 启用 toggle |
| 新建 `Settings/Models/OcrTab.tsx` | domain='ocr' 模型列表 + 启用 toggle |
| 新建 `Settings/Models/TranslateTab.tsx` | 占位页面 |

---

## 5. 不在本次范围

- GeneralPanel 的模型选择不动（集中管理入口）
- 翻译 API 接入（Tab 5 只占位）
- 模型增删（LLM/OCR tab 暂只 toggle，增删走 DB/CLI）
- 用户自定义环境变量的验证（允许任意 key，不做白名单校验）

---

## 6. 不变式

- **GeneralPanel 与 ModelsPanel 双入口同步**——两处操作同一个 config 字段（asr_engine/polish_llm/ocr_model），config-changed 事件驱动刷新
- **环境变量不替换非下载用途的 source**——只对 ASR 模型下载路径替换，LLM/OCR 的 `source`（API base URL）不替换
- **内置 3 变量不可删**——前端 UI 禁用删除按钮，后端 `delete_env_var` 拒绝内置 key
