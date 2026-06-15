# 结果窗口工具栏设计（result window toolbar）

- 日期：2026-06-15
- 状态：🔴 待实现（设计已确认，方案 A）
- 相关代码：`crates/desktop/src/result_window.rs`、`crates/desktop/dist/result/index.html`、`crates/desktop/src/coordinator.rs`、`crates/desktop/src/transcript.rs`

---

## 1. 背景与动机

`result_window`（识别结果展示窗，520×100，透明无边框、置顶）当前只有顶部 8px 拖拽条（`#drag-handle`，`cursor: grab`）+ 文本展示区。用户希望在展示区上方增加一行**工具栏**，提供运行时快捷切换能力，减少为改配置而重启 / 手编 `config.yaml` 的频率。

当前 desktop crate **没有任何 `#[tauri::command]`**——前端（单个 vanilla HTML，`window.__TAURI__` 全局，无构建步骤）只能接收 Rust→前端事件（`show-result` / `update-result` / `clear-result` / `hide-result`）。因此工具栏的可交互能力必须新打通**前端→Rust 命令通道**，这是本设计的核心工作量。

---

## 2. 功能范围

工具栏 4 个工具：

| # | 工具 | 图标语义 | 本轮状态 | 行为 |
|---|---|---|---|---|
| 1 | 打开应用设置页 | 齿轮（settings） | 🔴 占位 | 渲染置灰 + tooltip「敬请期待」，点击无动作 |
| 2 | 切换 LLM 润色 mode | 闪光（sparkles） | ✅ 实现 | 点击→浮层 3 选（关闭/仅最终/中间+最终），**立即生效** + 持久化 |
| 3 | 切换 ASR 识别模型 | 麦克风（microphone） | ✅ 实现 | 点击→浮层 8 选（DB `models` 表），**下次会话生效** + 持久化 |
| 4 | 切换润色 LLM 模型 | 芯片/对话（cpu-chip） | 🔴 占位 | 渲染置灰 + tooltip「敬请期待」，点击无动作 |

显隐：鼠标移入展示区 → 工具栏显示（窗口动态长高）；移开 → 工具栏隐藏（窗口缩回，等同现状）。

---

## 3. 已确认决策（brainstorming 结论）

1. **显隐尺寸行为**：**动态高度**。移开时窗口 = 100px（只有文本框，等同现状）；hover 时窗口长高到 ~132px，工具栏从顶部滑入；移开缩回。文本永不被遮挡。
2. **持久化**：切换 ASR 引擎 / polish mode 后，**写回 `~/.octopus/config.yaml`**，当前会话生效 + 重启保留。需新增运行时写盘能力（当前架构只读不写）。
3. **选项呈现**：**统一浮层面板**。点任一图标 → 图标下方弹出 in-webview 浮层（绝对定位，单选列表，当前项高亮 ●）。polish = 3 行；ASR = 8 行可滚动。点外部 / 再点图标关闭。
4. **可用时机**：**随时可切**（录音/识别进行中也可）。ASR 引擎切换**下次会话生效**（不重建当前引擎、不丢缓冲中的音频/partial）；polish mode **立即生效**（影响当前及后续润色）。
5. **占位工具（1、4）**：渲染但置灰 + tooltip，点击无动作。
6. **触发区**：hover 整个 `#container` 显示工具栏；浮层 `#popup` 打开时**钉住**工具栏（不因 mouseleave 隐藏），浮层关闭后才允许收起。

---

## 4. 架构（方案 A：Tauri 命令直连 + 轻量共享状态）

```
result_window (520 × 动态高, 透明置顶)
├─ #drag-handle        拖拽条（常驻，现有）
├─ #toolbar            hover 显示：4 个图标按钮
│   ├─ [⚙ 设置]        占位置灰
│   ├─ [✨ 润色mode]    点击→#popup (3 选)
│   ├─ [🎙 ASR引擎]     点击→#popup (8 选, DB models)
│   └─ [🤖 LLM模型]     占位置灰
├─ #popup              浮层，点图标弹出，绝对定位叠文本上
└─ #result-text        现有文本区（不变）

Rust 侧新增：
├─ RuntimeConfig { asr_engine: String, polish_mode: PolishMode }
│      包装为 Arc<RwLock<RuntimeConfig>>，挂 tauri::State
├─ 4 个 #[tauri::command]
│      toolbar_state / list_asr_engines / switch_asr_engine / set_polish_mode
├─ persist_config_override()   改单字段 + 序列化写回 ~/.octopus/config.yaml
└─ Coordinator 关键点读 RuntimeConfig
       · 下次会话取 asr_engine（传给引擎创建）
       · 每次润色前取 polish_mode（live）
```

**为何选方案 A**：当前零命令、前端只能收事件。方案 A 用标准 `#[tauri::command]` + `tauri::State` 共享状态，改动局部、命令边界清晰。`RwLock` 仅护 `asr_engine` + `polish_mode` 两字段，UI 级低频访问，竞争极低。备选方案 B（命令走 Coordinator mpsc，无共享锁）更贴合"单 mpsc 串行化"不变量，但 Coordinator 改动更大、读当前态给前端仍需共享快照，收益不抵成本。

---

## 5. 组件

### 5.1 `RuntimeConfig`（新，`crates/desktop/src/runtime_config.rs`）

```rust
pub struct RuntimeConfig {
    pub asr_engine: String,     // DB models 表 name；空 = 兜底 zipformer-small-ctc
    pub polish_mode: PolishMode, // 0/1/2
}
// 挂 tauri::State：Arc<RwLock<RuntimeConfig>>
// 启动时从 infra::config::load_config() 初始化这两个字段。
```

与 `OnceLock` 缓存的 `AppConfig` 的关系：`AppConfig` 仍是启动只读快照；`RuntimeConfig` 是这两个字段的**可变运行时镜像**。两者通过 `persist_config_override` 在写盘时保持一致（运行时改 `RuntimeConfig`，重启读 yaml）。

### 5.2 Tauri 命令（注册于 `main.rs` 的 `invoke_handler`）

| 命令 | 签名 | 说明 |
|---|---|---|
| `toolbar_state` | `() -> { asr_engine: String, polish_mode: u8 }` | 前端初始化 / 刷新选中态 |
| `list_asr_engines` | `() -> Vec<{ name: String, category: String, current: bool }>` | 从 DB `models` 表读全量引擎，`current` = 与 RuntimeConfig.asr_engine 命中 |
| `switch_asr_engine` | `(name: String) -> Result<(), String>` | 校验 DB 命中 → 写 RuntimeConfig → 写 config.yaml → emit `toolbar-state-changed`；**不重建当前引擎** |
| `set_polish_mode` | `(mode: u8) -> Result<(), String>` | 校验 0/1/2（非法返 Err）→ 写 RuntimeConfig → 写 config.yaml → emit `toolbar-state-changed`。**不直接碰 Transcript**（跨线程）；当前 Transcript 的 mode 由 Coordinator 在下次润色检查点读 RuntimeConfig 后 `set_mode` 同步（见 §7.2） |

### 5.3 `persist_config_override`（新，`runtime_config.rs`）

读当前 `~/.octopus/config.yaml`（缺失用 `AppConfig::default()`）→ 改单字段 → `serde_yaml::to_string` → 写回 `~/.octopus/config.yaml`（**绝对路径**，见 CLAUDE.md 约束）。保留所有其他字段。失败返回 `Err`，调用方 best-effort 处理（运行时状态已改则保留）。

> 注意：写盘只影响**下次重启**的 `load_config()` 读取；`OnceLock` 缓存不刷新，运行时生效完全走 `RuntimeConfig`。

### 5.4 前端（`crates/desktop/dist/result/index.html`）

- **DOM**：`#drag-handle` 下方加 `#toolbar`（4 个 `<button>`，每个含图标 + label）；`#container` 内加 `#popup`（绝对定位，默认 `display:none`）。
- **显隐**：`#container` 的 `mouseenter` → `currentWindow.setSize(new LogicalSize(520,132))` + `#toolbar` 显示；`mouseleave` → 若 `#popup` 未开则 `setSize(520,100)` + 隐藏。
- **图标变色**：用 **CSS `mask: url(icons/xxx.svg)` + `background: currentColor`**（见 §7.1）。默认 `color:#1d1d1f`，`:hover`/`.active` `color:#007aff`。
- **浮层**：点工具图标 → `invoke('list_asr_engines')` 或读 `toolbar_state` → 渲染单选列表 → 点选项 → `invoke('switch_asr_engine'|'set_polish_mode')` → 关浮层 + 刷新。点 `document` 空白处关浮层。
- **占位工具**：`disabled` + `title="敬请期待"`，CSS 置灰。

---

## 6. 数据流

### 6.1 显隐
```
mouseenter #container → setSize(520,132) + show #toolbar
mouseleave #container → if #popup closed: setSize(520,100) + hide #toolbar
                      → else: 保持（钉住）
```

### 6.2 切 ASR 引擎（下次会话生效）
```
click[🎙] → invoke list_asr_engines → 渲染浮层（DB 8 引擎，当前项 ●）
click 选项 → invoke switch_asr_engine(name)
           → Rust: 校验 DB 命中
                   → RuntimeConfig.asr_engine = name
                   → persist_config_override("asr_engine", name)
                   → emit "toolbar-state-changed"
           → 前端: 关浮层 + 刷新选中
生效时机: 下次 Toggle 开始 → Coordinator 读 RuntimeConfig.asr_engine
          → engine_embedded 用新引擎创建（当前会话引擎不重建、缓冲不丢）
```

### 6.3 切 polish mode（立即生效）
```
click[✨] → 浮层 3 选（关闭/仅最终/中间+最终）
click 选项 → invoke set_polish_mode(mode)
           → Rust(命令线程): 校验 0/1/2
                   → RuntimeConfig.polish_mode = mode
                   → persist_config_override("polish_mode", mode)
                   → emit "toolbar-state-changed"
           → 前端: 关浮层 + 刷新选中
生效时机: 立即——Coordinator(单线程) 每次 check_and_trigger_polish / start_pasting 前
          读 RuntimeConfig 最新 mode → 若当前 Stage 持 Transcript 则 set_mode 同步
```

### 6.4 占位工具（1、4）
```
click → 无动作（disabled，仅 tooltip）
```

---

## 7. 关键实现风险点

### 7.1 图标 hover 变色（CSS mask 方案）
`<img src=*.svg>` 不支持 `currentColor`，无法用 CSS 单点改色。采用 **CSS mask**：
```css
.icon { width:20px; height:20px; background:currentColor;
        mask:url(icons/asr-engine.svg) no-repeat center / contain;
        -webkit-mask:url(icons/asr-engine.svg) no-repeat center / contain; }
.tool:hover .icon, .tool.active .icon { color:#007aff; }
```
单文件 SVG 即可随 `color` 变色。否则需每个图标准备「默认 + hover」两色版本——本轮不采用。

### 7.2 polish mode 立即生效（Transcript.set_mode）
当前 `Transcript.mode` 是**构造时固定**（`Transcript::new(id, mode)`），中间/最终润色逻辑（`snapshot_for_polish` / `should_polish` 等）读它。要 live 化：
- `Transcript` 增 `pub fn set_mode(&mut self, mode: PolishMode)`，`mode` 字段改为可变（`PolishMode` 已 `Copy`，无需 `mut self` 内部可变）。
- Coordinator 收到 `set_polish_mode`（经命令→共享状态）后，**在下一次 tick / 润色检查点**读 RuntimeConfig 最新值；若当前 Stage 持有 Transcript，按需 `set_mode`。

  > 由于 Transcript 在 Stage 变体里按值持有、仅 Coordinator 单线程访问，无需额外同步。命令 handler 只写 RuntimeConfig（共享 RwLock），不直接碰 Transcript；Coordinator 在自己的线程上消费最新 mode。

---

## 8. config.yaml 写回语义

- **触发**：`switch_asr_engine` / `set_polish_mode` 成功改变 RuntimeConfig 后调用 `persist_config_override`。
- **保留字段**：整体 `serde_yaml::to_string(AppConfig)`，仅覆盖目标字段，其他字段（含用户注释外的所有键）原样保留。**已知限制**：`serde_yaml` 序列化会丢失 yaml 注释——可接受（config.yaml 无关键注释依赖），若需保留注释则改用 `yaml-rust` 等保留式编辑器（本轮不做，记为后续优化）。
- **路径**：`/Users/wudarui/.octopus/config.yaml`（绝对路径，CLAUDE.md 约束，避免 `config/` 符号链接被安全分类器误拦）。
- **失败处理**：写盘失败 → `warn` log + 命令返回成功但前端 toast「保存失败，本次仍生效，重启后回退」（运行时状态已改，best-effort 保留）。

---

## 9. 错误处理

| 场景 | 处理 |
|---|---|
| `switch_asr_engine` DB 不命中 | `Err`，前端 toast「引擎 X 不存在，未切换」，RuntimeConfig 不变 |
| `set_polish_mode` 非法值（非 0/1/2） | `Err`，前端 toast「mode 非法」，RuntimeConfig 不变 |
| config.yaml 写失败 | `warn` log + toast「保存失败，本次仍生效，重启后回退」，运行时状态保留 |
| 录音中切 ASR | 仅写 RuntimeConfig + yaml，不重建当前引擎（符合「下次生效」） |
| 浮层打开时 mouseleave | 钉住工具栏；浮层关闭后才允许收起 |
| RuntimeConfig RwLock 中毒 | `.unwrap_or_else` 回退默认，`warn` log，不崩 |

---

## 10. 测试

### Rust 单测
- `persist_config_override`：改单字段后其他字段保留；缺失文件时从默认建。
- `switch_asr_engine`：DB 命中成功 / 不命中 Err 两分支。
- `set_polish_mode`：0/1/2 成功 / 非法值 Err。
- `RuntimeConfig` 初始化：从 `AppConfig` 正确镜像两字段。

### 手动 e2e
- hover 长高（100→132）/ 移开缩回（132→100）。
- 切 ASR 引擎：当前会话引擎不变，重启后新引擎生效，`config.yaml.asr_engine` 已更新。
- 切 polish mode：边录边看润色行为立即变化（关↔仅最终↔中间+最终），`config.yaml.polish_mode` 已更新。
- 占位工具（1、4）：置灰 + tooltip，点击无动作。
- 浮层打开时移开鼠标：工具栏钉住不收。
- DB 不存在的引擎名：toast 报错，不切换。

---

## 11. 非目标（YAGNI）

- 应用设置页（工具 1）：本轮仅占位，不实现设置 UI。
- 润色 LLM 模型切换（工具 4）：本轮仅占位，不实现 LLM 模型运行时切换。
- config.yaml 注释保留式编辑：本轮 `serde_yaml` 整体序列化，丢注释可接受。
- 工具栏图标的多分辨率 PNG：本轮 SVG（mask）单文件方案，不准备 PNG。
- 工具栏位置/顺序可配置：固定 4 工具固定顺序。

---

## 12. 相关文件清单

| 文件 | 改动 |
|---|---|
| `crates/desktop/src/runtime_config.rs` | **新建**：`RuntimeConfig` + `persist_config_override` |
| `crates/desktop/src/main.rs` | 注册 `invoke_handler`（4 命令）+ `tauri::State` 挂 `Arc<RwLock<RuntimeConfig>>` |
| `crates/desktop/src/coordinator.rs` | 引擎创建读 RuntimeConfig.asr_engine；润色检查点读 RuntimeConfig.polish_mode |
| `crates/desktop/src/transcript.rs` | `set_mode()` + mode 字段可 live 更新 |
| `crates/desktop/dist/result/index.html` | `#toolbar` + `#popup` + hover 高度逻辑 + mask 图标 + invoke 调用 |
| `crates/desktop/dist/result/icons/*.svg` | 4 个图标（用户提供，放此目录） |
| `docs/configuration.md` | 补注：`asr_engine` / `polish_mode` 现可经工具栏运行时改写并持久化（启动仍读 yaml） |
| `docs/architecture.md` | result_window 段补工具栏 + RuntimeConfig 子系统说明 |

---

## 13. 后续（实现完成后）

实现 + 验证通过后，转 `writing-plans` 生成的实施计划，按计划落地。文档同步（configuration.md / architecture.md）随代码变更一并完成（CLAUDE.md 强制）。
