# 归档设计文档（2026-06-15 ~ 2026-06-16，已实现）

> 本文件合并以下**已实现功能**的原始设计 spec，作为历史记录归档（2026-06-18）。
> 各功能已在 main 实现，**权威现状以 [`architecture.md`](../../architecture.md) / [`configuration.md`](../../configuration.md) 为准**。
> 其中 `denoise-deepfilternet` 已被 dfn3 方案取代、`model-spec-prefix` 已被 3-part spec 部分取代——演进说明见各小节内原文标注。
> 归档内各 spec 之间的交叉引用可能指向已归档的同级文件——所需内容均在本文内，请按下方标题搜索。

## 包含的原 spec

- `2026-06-15-asr-hardware-acceleration-design.md`
- `2026-06-15-result-window-toolbar-design.md`
- `2026-06-16-asr-llm-model-menu-design.md`
- `2026-06-16-denoise-deepfilternet-design.md`（已被 dfn3 取代，续作见 `2026-06-17-denoise-deepfilternet3-integration-design.md`）
- `2026-06-16-model-spec-prefix-design.md`（已被 3-part 部分取代，见 `2026-06-17-aliyun-cloud-apis-design.md` §3.3）

---

## `2026-06-15-asr-hardware-acceleration-design.md`

# 设计文档：ASR 硬件加速（手动开关 + 优雅回退）

> 为 ASR 推理增加硬件加速（CUDA / DirectML / CoreML execution provider）的手动开关与平滑降级：配置开启时在 ONNX Runtime 注册 EP，加载失败/异常自动回退 CPU；关闭时纯 CPU 推理。VAD 不受影响（固定 CPU）。

> **实现状态（2026-06-15）**：已实现并经 macOS CoreML 手动验证。`cargo test -p octopus-asr` 16 passed / 0 failed。

## 1. 背景与目标

ASR 大模型（Qwen3-1.7B、SenseVoice 等）CPU 推理慢（RTF ~1.3x）。目标：可选启用 GPU/CoreML/DirectML 加速。

**为何手动开关而非自动**：部分大模型（如 Qwen3-1.7B）含大量动态 Shape，CoreML 不完全支持其算子，构建 session 时会被 EP 拦截中止。若默认开启会导致此类模型无法加载。因此提供显式开关 `asr_hardware_accelerated`，默认 `false`（CPU，稳定），用户按需开启。

## 2. 设计

### 2.1 EP 注册顺序
开启时按序尝试注册（ort 按序匹配首个可用）：

1. `CUDAExecutionProvider`（NVIDIA GPU）
2. `DirectMLExecutionProvider`（Windows GPU）
3. `CoreMLExecutionProvider`（macOS Apple Silicon/GPU）

### 2.2 优雅回退
- 注册成功 → 用加速 session。
- 注册失败（EP 不可用 / 算子不支持）→ `log::warn` + **重建一个干净的 CPU session builder**（非降级到部分 EP，而是整体回退 CPU），保证识别不中断。

### 2.3 VAD 不受影响
VAD（silero_vad_v4，1.8M 微小模型）保持纯 CPU——`find_silero_vad` 固定路径加载，不走 `apply_session_acceleration`。微小模型加载加速器的额外开销（EP 初始化）不划算。

## 3. 配置

| 字段 | 类型 | 默认 | 说明 |
|---|---|---|---|
| `asr_hardware_accelerated` | bool | `false` | true 时 ASR 推理启用 CUDA/DirectML/CoreML EP（失败回退 CPU）；VAD 不受影响 |

```yaml
# ~/.octopus/config.yaml
asr_hardware_accelerated: false  # true 启用 GPU/CoreML/DirectML（失败回退 CPU）
```

## 4. 变更点

### 4.1 infra（config schema）
- `crates/infra/src/config.rs`：`AppConfig` 加 `asr_hardware_accelerated: bool`（`#[serde(default = "default_asr_hardware_accelerated")]`，默认 `false`）+ `default_asr_hardware_accelerated()` + `Default` impl + 测试 `asr_hardware_accelerated_defaults_to_false`。

### 4.2 asr（依赖 + 加速包装 + config 缓存）
- `crates/asr/Cargo.toml`：ort features 加 `cuda` / `coreml` / `directml`（编译平台特定 EP）。
- `crates/asr/src/config.rs`：
  - `pub fn apply_session_acceleration(builder: SessionBuilder) -> Result<SessionBuilder>`：查 `asr_hardware_accelerated`，true 则注册 3 个 EP，失败重建 CPU builder。
  - **config 缓存**：`static APP_CONFIG: OnceLock<AppConfig>` + `load_app_config_cached() -> &'static AppConfig`。首次读 config.yaml 后缓存，避免每次 session 构建重复读文件 + 解析 yaml（paraformer 一次识别建 encoder+decoder 两个 session，streaming 引擎更频繁）。读失败回退 `AppConfig::default()`（ASR 保持 CPU）。手编 config.yaml 需重启进程生效（与 `RUNTIME_CONFIG` 一致）。

### 4.3 引擎接入
8 个引擎的 `Session::builder()?` 改为 `apply_session_acceleration(Session::builder()?)?`：

- `whisper.rs`（encoder / dec_init / dec_past，3 处）
- `qwen3_asr.rs`（conv_session / encoder_session / decoder_session，3 处）
- `paraformer.rs` + `streaming_paraformer.rs`（encoder_session / decoder_session）
- `zipformer.rs` + `streaming_zipformer.rs`（session）
- `sensevoice.rs`（session）

## 5. 验证

### 5.1 自动化回归
`cargo test -p octopus-asr` → 16 passed / 0 failed。

### 5.2 macOS CoreML 手动
| 模型 | 加速 | 结果 |
|---|---|---|
| qwen3-asr-1.7B | false（CPU） | 正常，4.13s（RTF 1.36x） |
| SenseVoice | true（CoreML） | 成功加载 CoreML EP，识别正确，4.15s（RTF 1.35x） |
| qwen3-asr-1.7B | true（CoreML） | encoder_session 构建时被 CoreML 拦截（动态 Shape 算子不支持）→ **印证手动开关的必要性** |

## 6. 约束与风险

- **config 缓存**：`APP_CONFIG` OnceLock 首次读取后固化，手编 `asr_hardware_accelerated` 需重启进程（与 DB 配置一致）。
- **大模型 EP 兼容性**：Qwen3-1.7B 等动态 Shape 模型在 CoreML 下会失败 → 用户需按模型特性决定是否开启（小/中模型可开，大动态模型建议关）。
- **EP 注册顺序固定**：CUDA → DirectML → CoreML，不可配置（YAGNI；多数平台只有一个可用）。

---

## `2026-06-15-result-window-toolbar-design.md`

# 结果窗口工具栏设计（result window toolbar）

- 日期：2026-06-15
- 状态：✅ 已实现（已合并 main，2026-06-15）。实现过程中的细节修订见 §14。
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
| 4 | 切换润色 LLM 模型 | 芯片/对话（cpu-chip，实装 `llm-model.svg`） | ✅ 实现 | 点击→浮层（DB `models` 表 `is_enabled` LLM，当前项 ●），选→`switch_polish_llm` 即时 + 持久化。详见 §16 |

显隐：鼠标移入展示区 → 工具栏显示（窗口动态长高）；移开 → 工具栏隐藏（窗口缩回，等同现状）。

---

## 3. 已确认决策（brainstorming 结论）

1. **显隐尺寸行为**：**动态高度**。移开时窗口 = 100px（只有文本框，等同现状）；hover 时窗口长高到 ~132px，工具栏从顶部滑入；移开缩回。文本永不被遮挡。
2. **持久化**：切换 ASR 引擎 / polish mode 后，**写回 `~/.octopus/config.yaml`**，当前会话生效 + 重启保留。需新增运行时写盘能力（当前架构只读不写）。
3. **选项呈现**：**统一浮层面板**。点任一图标 → 图标下方弹出 in-webview 浮层（绝对定位，单选列表，当前项高亮 ●）。polish = 3 行；ASR = 8 行可滚动。点外部 / 再点图标关闭。
4. **可用时机**：**随时可切**（录音/识别进行中也可）。ASR 引擎切换**下次会话生效**（不重建当前引擎、不丢缓冲中的音频/partial）；polish mode **立即生效**（影响当前及后续润色）。
5. **占位工具（1）**：渲染但置灰 + tooltip，点击无动作。（工具 4 润色 LLM 已实现，见 §16）
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
│   └─ [🤖 LLM模型]     点击→#popup（DB LLM 列表，当前项 ●），见 §16
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
- **占位工具**：工具 1（设置）`disabled` + `title="敬请期待"`，CSS 置灰；工具 4（润色 LLM）已实现，见 §16。

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

### 6.4 占位工具（1）
```
click → 无动作（disabled，仅 tooltip）
```
> 工具 4（润色 LLM）已实现，数据流见 §16。

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
- 占位工具（1）：置灰 + tooltip，点击无动作。
- 浮层打开时移开鼠标：工具栏钉住不收。
- DB 不存在的引擎名：toast 报错，不切换。

---

## 11. 非目标（YAGNI）

- 应用设置页（工具 1）：本轮仅占位，不实现设置 UI。
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

---

## 14. 实现后修订（实际落地与原设计的差异，2026-06-15）

功能已合并入 main。实现过程中 UI 反复打磨，以下点与上文原设计不同：

1. **触发事件 `mouseenter` → `mousemove`**：macOS WKWebView 对「窗口可见但非前台」的悬浮窗常漏派发 `mouseenter`。改用 `mousemove` 触发 `showToolbar()`，并加 `toolbarVisible` 状态守卫避免高频 `setSize` 抖动。**已知限制（已接受）**：窗口完全无焦点时鼠标进入展示区仍可能不弹工具栏——属 macOS 系统级对非聚焦悬浮窗鼠标事件的限制，非代码 bug。
2. **`mouseleave` 行为变更（非「钉住」）**：原 §3.6 / §6.1 设计「浮层打开时 mouseleave 钉住工具栏」**未实现**。实际 `hideToolbar()` 先 `closePopup()` 收浮层、再 `setSize(520,100)` 缩回——即鼠标离开展示区时浮层与工具栏**一起收起**。
3. **`EngineOption` DTO 增 `is_local: bool`**（原 §5.2 表只有 name/category/current）。前端据此渲染引擎名前缀：`is_local=true` 显示「本地-<name>」，否则「远程-<name>」。
4. **弹层宽度 `width: 360px`**（原设计/plan 未定宽，经 180→240→360 演进）。`.option` 用 `.nm`（`flex:1; min-width:0; white-space:nowrap; text-overflow:ellipsis`）+ `.cat`（`flex-shrink:0`）做单行布局，防长引擎名换行折叠。
5. **图标缓存清除 `?v=2`**：4 个 mask `url(icons/*.svg?v=2)` 末尾加版本号，强制 WKWebView 重新拉取更新后的 SVG（原 §5.4 / §7.1 未涉及 WebView 子资源缓存；macOS WKWebView 会缓存 mask 引用的 SVG）。
6. **debug 自动 devtools**：`result_window` 创建后 `#[cfg(debug_assertions)] window.open_devtools()`，debug 构建自动开 devtools 便于排查前端，release 自动剔除无副作用。
7. **启动脚本 `run-octopus.sh`**（仓库根）：`pkill` 杀进程 + 等 1s + 清 `~/Library/{WebKit,Caches,HTTPStorages}/com.octopus.desktop` + `cargo run --release --features embedded`，一步保证前端/二进制最新，规避缓存与 profile 不匹配。
8. **ASR 引擎列表数量动态**：`list_asr_engines` 经 `load_models_at`（`WHERE domain='asr' AND is_enabled=1`）过滤，**非固定 8 个**；用户改 DB `models.is_enabled` 即可控制工具栏可见引擎。
9. **托盘引擎项实时刷新**（原 §6.2 未涉及）：`switch_asr_engine` 写 RuntimeConfig + yaml 后，额外调 `tray::update_tray_engine_label(name, engine_mode)` 实时更新系统托盘菜单的「引擎: <name> (<mode>)」项（`TRAY_ITEMS` 缓存 `engine_info` handle，`set_text` 更新避免重复 ID panic）。引擎切换的反馈现覆盖两处 UI：结果窗工具栏（选中态）+ 系统托盘菜单（标签）。

---

## 15. 第二批扩展：降噪模式 + 立即润色 + hide_toolbar（2026-06-17）

在原设计 4 工具基础上扩展为 6 工具。工具栏顺序：系统设置 → 语音模型(ASR) → 降噪模式 → 润色模型(LLM) → 润色模式 → 立即润色。

### 15.1 降噪模式工具（`tool-denoise`）

- **图标**：Font Awesome headphone（`denoise.svg`，CSS mask 方式同 §7.1）
- **浮层 3 选**：无 / 轻度 / 深度 → 对应 `denoise_mode` 0 / 1 / 2
- **Tauri 命令**：`set_denoise_mode(mode: u8)` —— 校验 0/1/2 → 写 `RuntimeConfig.denoise_mode` → 持久化 `config.yaml.denoise_mode`
- **生效语义**：`denoise_mode=0` 关闭环境降噪（直通）；`1`（默认）=RNNoise / `2`=DeepFilterNet3，由 mode 选可插拔后端（详见 [DF3 整合 spec](2026-06-17-denoise-deepfilternet3-integration-design.md)）。旧 `denoise_enabled: bool` 字段已删除
- **config.yaml**：新增 `denoise_mode: u8`（默认 `1`），`AppConfig` 同步加字段
- **选中态**：`refreshActive()` 读 `toolbar_state.denoise_mode`，mode≠0 时按钮 `.active`

### 15.2 立即润色工具（`tool-polish-now`）

- **图标**：Font Awesome bolt（`polish-now.svg`）
- **行为**：点击 → `invoke('polish_now')` → 按钮置 `disabled` + toast「润色中…」→ 后端异步润色 → `listen('polish-done')` 恢复按钮
- **后端**：`Coordinator::polish_now()` 发 `Command::PolishNow` → `handle_polish_now`：
  - 仅在 `Streaming` / `VadSegmented` 阶段生效（需持 `Transcript`）
  - **忽略 `polish_mode`**（区别于 `check_and_trigger_polish` 的 mode=2 限制）——用 `llm_config_ignore_mode()` 取 LLM 配置，绕过 `polish_mode==Disabled` 的 None 短路
  - 复用现有 `snapshot_for_polish()` → `mark_polish_pending()` → `spawn_polish_thread(text, config, tx, true)` 路径
  - `spawn_polish_thread` 增加 `ignore_mode: bool` 参数：`true` 调 `llm_config_ignore_mode`，`false`（原自动润色路径）调 `llm_config`
- **PolishDone 回显**：`handle_polish_done` 接受 `Streaming` / `VadSegmented` / **`WaitingCompletion`**（防止用户点按钮后停止录音，stage 切换导致润色结果丢弃），把 `polished` 写回 Transcript 后调 `update_result` 刷新展示区；结尾 `emit("polish-done")` 通知前端恢复按钮（无论成功/失败/stage 不匹配）。**`handle_polish_now` 的所有早退路径同样必须 emit `polish-done`**（stage 不匹配 / transcript 空 / `polish_pending` 已置 / LLM 配置缺失）——否则前端 `disabled=true` 永久卡死（2026-06-18 修复：polish_llm 未配置时点击按钮走早退但曾漏 emit，按钮灰色无法恢复）
- **Transcript.display_text() 变更**：原仅 `mode==Intermediate` 展示 polished；现改为 **polished 非空即展示**（`polished + increase`），使 PolishNow 在 mode=0/1 下也能让润色结果覆盖 raw 文本
- **空配置兜底**：`llm_config_ignore_mode()` 返回 None → `show_result("未配置润色模型")`，不进入润色流程

### 15.3 hide_toolbar 配置项

- **config.yaml**：新增 `hide_toolbar: bool`（默认 `true`）
- **生效语义**：`true`=hover 显隐工具栏（原行为，窗口 100↔132px 动态高度）；`false`=工具栏始终显示（窗口恒 132px）
- **前端**：`toolbar_state` 命令返回 `hide_toolbar`，`refreshActive()` 双向切换：`false`→移除 hover 监听 + 常驻展开；`true`→（重新）注册 hover 监听 + 立即收起。**运行时切换立即生效**（2026-06-18 改进）：`set_config` 改 `hide_toolbar` 后 emit `config-changed` 事件，result window 监听该事件重调 `refreshActive()` 即时切换工具栏显隐模式——无需等 `show-result` 或重启。

### 15.4 RuntimeConfig 扩展

`RuntimeConfig` 新增字段：
- `denoise_mode: u8` —— 运行时镜像 `config.yaml.denoise_mode`，供 `set_denoise_mode` 命令读写
- `ToolbarState` DTO 新增 `hide_toolbar: bool` + `denoise_mode: u8`，前端经 `toolbar_state` 命令一次性获取

### 15.5 VAD 段间拼接标点去重（2026-06-17 修订）

**问题**：VAD 伪流式模式下，每段识别结果由 ASR 引擎返回时自带句尾标点（`。` `？` 等），而 `consume_completed_results` 在段间无条件补逗号 → 拼出 `。，` `？，` 等连续标点。

**修复**：`consume_completed_results` 段间加逗号条件从两个增至三个（`coordinator.rs:797`）：
1. 已有文本非空（原条件）
2. 新段不以标点开头（原条件，防 `，。` 倒序）
3. **已有文本不以标点结尾**（新增，防 `。，` `？，`）

标点集合：`,.，。！？!?\n`。引号 `" '` `「」` 不在此集合中，段间仍可正常补逗号。

---

## 16. 润色模型（LLM）工具实现（2026-06-17）

工具 4（`tool-llm`）从 v1 占位转为可用：前端点击 + 后端 `list_llm_models` / `switch_polish_llm`，与 ASR 引擎工具（§6.2）同构。浮层列出 DB 启用的 LLM，当前项 ●，选→即时切 RuntimeConfig + 持久化 config.yaml。

### 16.1 RuntimeConfig 扩展（补充 §15.4）

`RuntimeConfig` 新增 `polish_llm: String`（运行时镜像 `config.yaml.polish_llm`，`from_config` 初始化镜像）。润色链路（`coordinator.rs` 的 `check_and_trigger_polish` / `start_pasting` / 立即润色）读 `config.polish_llm`——**但 `config` 是启动时 move 进 coordinator 线程的快照，不会自动跟随 RuntimeConfig 更新**。

**同步机制（2026-06-18 改进为立即生效）**：新增 `Command::UpdateRuntime`，外部修改 RuntimeConfig 后通过 `coordinator.update_runtime()` 主动通知 coordinator 重读 RuntimeConfig 把 `polish_llm` / `polish_mode` / `asr_correct` / `output_simplified` / `hide_toolbar` 同步到 `config` 快照（详见 `sync_runtime_fields` 辅助函数，与 Toggle 时复用同一逻辑）。`set_config`（设置窗口）和 `switch_polish_llm`（工具栏浮层）在写完 RuntimeConfig 后调 `coordinator.update_runtime()`——**用户在录音中改 polish_llm 也能立即生效**（下次润色用新模型），无需 Toggle。`asr_engine` 不在此路径（需重建引擎实例，仍只能 Toggle 时切）。`polish_mode` 仍保留每 tick 读 + `set_mode` 立即生效（双保险）。

历史修复：2026-06-18 曾遗漏 Toggle 时同步 `polish_llm`，导致「立即润色」报 `no LLM config available` 按钮卡死；本次 UpdateRuntime 路径彻底解决外部修改即时同步问题。

### 16.2 后端命令（`runtime_config.rs`）

| 命令 | 行为 |
|---|---|
| `list_llm_models` | 读 `RuntimeConfig.polish_llm` → `parse_model_spec` 取裸名 → `db::list_llm_models()`（`is_enabled` LLM）→ `build_llm_options` 标 `current` + `label`（`本地:NAME` / `CATEGORY:NAME`） |
| `switch_polish_llm(name)` | 校验 `name` 在 DB LLM 列表（`find`）→ 构造 spec（`is_local`→`local:NAME`，否则 `CATEGORY:NAME`）→ 写 `RuntimeConfig.polish_llm`（即时）→ `persist_polish_llm` 写 config.yaml（持久） |

DTO `LlmOption { name, category, is_local, current, label }`（与 `EngineOption` 同构，**无 fallback 固定项**——与 ASR 不同）。后端解析 `polish_llm` 走 `config::llm_config()`（`polish_mode=Disabled` 或 DB 不命中返回 `None`，降级直通不润色，仅 warn）。

### 16.3 前端（`index.html`）

- **点击 `#tool-llm`**：`invoke('list_llm_models')` → 空则 toast「无可用润色模型」→ 否则渲染浮层（当前项 ●）→ 点选项 `invoke('switch_polish_llm', {name})` → 关浮层 + toast「已切换润色模型：X」。
- **`refreshActive`**：当前 `#tool-llm` **恒 `.active`**（`classList.toggle('active', true)`）——不反映 `polish_llm` 是否有效解析。

### 16.4 「不选择模型」+ DB 回退 + 图标灰（2026-06-17 已实现）

原缺口：① 浮层无「不选择模型」项；② `polish_llm` 在 DB 找不到时仅后端 warn、前端无感知；③ 无有效模型时图标仍 active（误导）。

实现：
- **`build_llm_options`**：首项固定「不选择模型」（`name: ""`）。`current` 有效（裸名非空且在 DB 列表）→ 首项非 current；`current` 空 / 裸名不在 DB / spec 不命中 → 首项 current（**DB 找不到回退无模型**）。
- **`switch_polish_llm`**：空 `name` → `polish_llm` 置空（不查 DB）；非空走原 DB 校验 + spec 构造。
- **`ToolbarState`** 新增 `polish_llm_valid: bool`：`toolbar_state` 命令查 DB 计算（裸名非空且在启用 LLM 列表；DB 失败保守 false）。
- **前端**：浮层渲染首项；点「不选择模型」→ toast「已关闭润色模型」+ `refreshActive()`；`#tool-llm` `active = st.polish_llm_valid`（无模型时深灰、**仍可点击**，非 `disabled`——区别于工具 1）。**`refreshActive()` 调用时机**：① webview 初始化；② `show-result` 事件（每次显示结果窗时重读 toolbar_state，确保用户在设置窗口改了 polish_llm/polish_mode/denoise_mode 后下次显示即刷新——2026-06-18 修复：曾仅初始化调一次，导致设置改了 polish_llm 后工具栏高亮状态不刷新）；③ `config-changed` 事件（设置窗口改 hide_toolbar 后后端 emit，前端立即切换工具栏显隐模式——双向：`false`→移除 hover + 常驻展开，`true`→恢复 hover + 立即收起）；④ 浮层切换 / polish-done 等。

单测：`build_llm_options_none_current_when_polish_llm_empty_or_not_in_db`（空/裸名不在 DB/spec 不命中 → 首项 current）+ 更新 `build_llm_options_marks_current_and_labels` / `build_llm_options_current_in_spec_format`（首项偏移）。



---

## `2026-06-16-asr-llm-model-menu-design.md`

# ASR/LLM 模型选择菜单设计

> 日期：2026-06-16
> 状态：✅ 已实现（2026-06-16）。后续 2026-06-17 阿里云 taxonomy 重构后，label 远程前缀从 `category` 改为 `provider`（见 §3 更新），`LlmModelInfo` 增 `provider`/`model_name` 字段。

## 背景与目标

octopus desktop 结果窗口（`crates/desktop/dist/result/index.html`）工具栏现有 `#tool-asr`（ASR 模型）与 `#tool-polish`（润色模式 0/1/2）两个按钮，点击弹 `#popup`。本次：

1. **改造 ASR 菜单**：固定首条兜底项「本地:zipformer-small-ctc」（不依赖 DB），其余按 `is_local desc, category` 排序，统一显示「本地:{name} / {category}:{name}」。
2. **新增 LLM 润色模型菜单**：工具栏加按钮，列 `domain='llm' AND is_enabled=1` 的模型，同排序与显示规则，选中切换 `polish_llm`。

动机：兜底引擎已被用户从运行时 DB 删除，现有菜单不再显示它且 `switch_asr_engine` 选它会报错；排序规则不符预期；LLM 模型此前无运行时切换入口。

## 现状（关键代码）

- `crates/asr/src/config.rs:216` `list_engines()` → `Vec<EngineInfo>{name, category(enum), description, is_local}`，遍历内存 config 5 个 section，**按 category 硬编码排序**（SenseVoice=0…Zipformer=4）。不过滤 `is_enabled`——`load_models_at` 在 DB 层已过滤 `is_enabled=0`，内存 config 本就只含启用项。
- `crates/desktop/src/runtime_config.rs:109` `list_asr_engines` 命令 → `Vec<EngineOption>{name, category(str), current, is_local}`。
- `runtime_config.rs:130` `switch_asr_engine`：校验 `name` 在 `list_engines()`，否则 `Err("引擎 '{}' 不存在，未切换")`。
- `runtime_config.rs:90` `EngineOption`；`:47` `category_str`（enum→"whisper"/"sensevoice"/…）。
- `crates/infra/src/db.rs`：`ModelEntry`（含 `is_enabled`）；`load_models_at` 过滤 `is_enabled=0`；`load_llm_model_at`（`WHERE domain='llm' AND name=? AND is_enabled=1`，按名加载单个 LLM）。
- 前端 `result/index.html:303-320` ASR popup（渲染 name+category 两列，点击 `switch_asr_engine`）；`:281-300` polish popup。

## 设计

### §1 ASR 菜单改造

**(a) `list_engines` 排序**（asr/config.rs:216）：把现有 category 硬编码 match 改为 **`is_local` 降序优先，再 `category` 字母序**（基于 `category_str`，与 SQL `ORDER BY category` 语义一致；同 category 内 `name` 字母序作 tiebreak）。

**(b) `EngineOption` 加 `label`**（runtime_config.rs:90）：新增 `label: String`，后端拼（`engine_label`）：
- `is_local == true` → `"本地:{name}"`
- 否则 → **`"{provider}:{name}"`**（2026-06-17 更新：远程前缀从 `category` 改为 `provider`，以区分 deepseek 直连 vs aliyun 代管同名模型；本地 `provider` 恒为 `"local"` 无信息量故仍走「本地:」前缀）

**(c) `list_asr_engines` 注入兜底**（runtime_config.rs:109）：结果最前插入：
```
EngineOption {
    name: "zipformer-small-ctc", category: "zipformer", is_local: true,
    current: asr_engine 为空 或 == "zipformer-small-ctc",
    label: "本地:zipformer-small-ctc",
}
```
若 DB 返回结果已含 `name == "zipformer-small-ctc"`，跳过注入（去重）。

**(d) `switch_asr_engine` 放宽兜底**（runtime_config.rs:130）：`name == "zipformer-small-ctc"` 时跳过 DB 存在性校验，直接允许切换（仅写 RuntimeConfig.asr_engine + persist + tray label；真正加载在录音时由 `resolve_active_engine` 兜底硬构造 `DEFAULT_ASR_MODEL_DIR`）。其余 name 维持 DB 校验。

### §2 LLM 菜单新增

**(a) `db.rs` 新增列表查询**：
```rust
pub struct LlmModelInfo { pub provider: String, pub category: String, pub model_name: String, pub is_local: bool }

// SQL: SELECT provider, category, model_name, is_local FROM models
//      WHERE domain='llm' AND is_enabled = 1
//      ORDER BY is_local DESC, category
fn list_llm_models_at(conn: &Connection) -> Result<Vec<LlmModelInfo>>;
pub fn list_llm_models() -> Result<Vec<LlmModelInfo>>;  // 经 with_db
```
（2026-06-17 更新：`name` 字段重命名为 `model_name`，新增 `provider`——配合 3-part spec `{provider}:{category}:{model_name}`。仿 `load_llm_model_at` 的 SQL 模式，去 `model_name=?`、加 `ORDER BY`。）

**(b) `runtime_config.rs`**：
- `RuntimeConfig` 加 `polish_llm: String`（`from_config` 取 `cfg.polish_llm`，默认 `"glm-4-flashx"`）。
- 新增 `LlmOption { name, category, is_local, current, label }`（label 同 §1(b) 规则）。
- 新增命令 `list_llm_models(rc)` → `Vec<LlmOption>`，`current = (rc.polish_llm == name)`。
- 新增命令 `switch_polish_llm(name, rc)`：校验 `name` 在 `list_llm_models()`；写 `rc.polish_llm` + `persist_polish_llm(name)`。
- 新增 `persist_polish_llm(value)`：load config → 覆盖 `polish_llm` → `write_config_yaml`（仿 `persist_polish_mode`）。

**(c) 前端 `result/index.html`**：工具栏加 `#tool-llm` 按钮（润色模型 + 图标），复用 `#popup`：
- 点击 → `invoke('list_llm_models')` → 渲染每个 `label`（current 高亮）。
- 点击选项 → `invoke('switch_polish_llm', { name })` → 重绘 popup。
- 列表为空 → `showToast('无可用润色模型（请在 DB 启用 is_enabled=1）')` 提示，不渲染空菜单。
- `#tool-llm` 恒显示（`active` 处理同 `#tool-asr`）。
- ASR popup 同步改用 `e.label` 直显（替换现有 name+category 两列拼装）。

### §3 显示规则（统一）

两菜单 label 后端拼（`engine_label`，`runtime_config.rs`）：
- `is_local` → `"本地:{name}"`
- 否则 → **`"{provider}:{name}"`**（2026-06-17 更新：远程用 `provider` 而非 `category` 前缀——`deepseek` 直连与 `aliyun` 代管的同名模型 category 相同，只有 provider 不同，用 provider 才能在 UI 分辨供应商）
- 本地引擎 `provider` 恒为 `"local"` 无信息量，故本地仍走 `"本地:{name}"`。

### §4 兜底与持久化

- **ASR 兜底**：固定显示 + switch 放宽；运行时加载由 `resolve_active_engine` 硬构造（已有）。
- **LLM**：`switch_polish_llm` persist `config.yaml.polish_llm` + RuntimeConfig 镜像；润色时 `load_llm_model(polish_llm)`（已有）。
- switch 校验失败返 `Err`（前端可提示），不 panic。

## 文件改动清单

| 文件 | 改动 |
|---|---|
| `crates/asr/src/config.rs` | `list_engines` 排序改为 `is_local desc` + `category` 字母序 |
| `crates/infra/src/db.rs` | 新增 `LlmModelInfo` + `list_llm_models_at` + `list_llm_models` |
| `crates/desktop/src/runtime_config.rs` | `EngineOption` 加 `label`；`list_asr_engines` 注入兜底；`switch_asr_engine` 放宽兜底；`RuntimeConfig` 加 `polish_llm`；新增 `LlmOption` / `list_llm_models` / `switch_polish_llm` / `persist_polish_llm` |
| `crates/desktop/dist/result/index.html` | ASR 显示改 `label`；新增 `#tool-llm` 按钮 + popup 逻辑 |
| `crates/desktop/src/main.rs`（命令注册处） | 注册 `list_llm_models` / `switch_polish_llm` |

## 测试

- `list_engines`：构造混合 `is_local`/`category`，断言 `is_local desc` + `category` 字母序。
- `list_asr_engines`：DB 有/无 `zipformer-small-ctc` 两场景，断言兜底注入 + 去重 + current 标记。
- `list_llm_models_at`：构造多条 LLM（含 `is_enabled=0`），断言过滤 + 排序。
- `switch_asr_engine`：兜底名通过、非兜底不存在名 `Err`。
- `switch_polish_llm`：persist 往返（写 `config.yaml.polish_llm`，重读一致）。

## 验收标准

1. ASR 菜单首条固定「本地:zipformer-small-ctc」（无论 DB 是否有），选中可切换、不报错。
2. 其余 ASR 项按 `is_local desc` + `category` 字母序，显示「本地:{name} / {category}:{name}」。
3. LLM 菜单列出 `is_enabled=1` 的 LLM 模型，同排序与显示；选中切换 `polish_llm` 并持久化（重启仍生效）。
4. `is_enabled=0` 的模型不出现（load 层过滤，含 LLM 空列表场景）。
5. `cargo check --workspace --all-targets` + 相关单测通过。

## 非目标（YAGNI）

- 不改 `toolbar_state`（`list_*` 命令的 `current` 字段已够前端标当前；按钮恒显示）。
- 不显示 `is_thinking` 标记。
- 不改润色模式（0/1/2）菜单。
- 不动 `.worktrees/fix-polish-llm-category-prefix`（用户并行分支，独立处理 polish_llm category 前缀）。

---

## `2026-06-16-denoise-deepfilternet-design.md`

# 环境降噪（DeepFilterNet3）设计

> 日期：2026-06-16（初版），2026-06-16 修复 4 个 bug（对齐 libDF）
> 状态：✅ 已实现 + bug 修复完成
> 关联：独立于 `config-infra-and-engine-truth` plan（见 §8 落点注记）

---

## ⚠️ 修订记录（2026-06-16）：弃用 DeepFilterNet3，换 RNNoise（nnnoiseless）

**本文档以下内容（DeepFilterNet3 / dfn3.onnx 设计）已废弃，仅作历史与调查记录保留。** 当前实现以本节为准。

### 弃用原因：dfn3.onnx 模型层缺陷

经完整诊断链确认 `penta2himajin/deepfilternet3-onnx/dfn3.onnx`（流式逐帧 ONNX 导出）**把正常语音当噪声压到约 10%**，开降噪反而损害 ASR（与用户实测一致：关降噪识别效果更好）。证据：

- DSP/链路全对：spec 量级 ~0.30（含 wnorm）、ERB/DF 特征正常、GRU shape 正确、**完美重构**（增量 vs 批处理 max_diff < 1e-4）；
- `ort` 对其他模型（whisper/paraformer/vad）推理正常 → 排除 ort；
- 唯一异常：`enhanced_spec ≈ 0.10 · spec`（正常应 ≈1.0·spec 或 mask），即推理输出压语音；
- mellonella 的流式逐帧导出测试只验形状不验质量，缺陷未被覆盖。

### 当前实现：RNNoise（nnnoiseless）

改用 `nnnoiseless`（Xiph RNNoise 的纯 Rust 移植，BSD-3，无 C 依赖，内置默认训练模型 `weights.rnn`）：

- `DenoiseProcessor` 包装 `nnnoiseless::DenoiseState`，接口不变（`new`/`reset`/`process_samples`/`flush`；`new()` 已移除 `model_path` 参数）；
- `FRAME_SIZE = 480`（10ms @48kHz）匹配 octopus HOP；样本在 `[-1,1]` ↔ `[-32768,32767]`（i16 PCM 等价）间转换；
- **无外部模型文件依赖**——`audio.rs` 不再 `find_df3`，`config.rs` 删 `find_df3`/`DF3_HF_REPO`/`DF3_ONNX_FILE`，`Cargo.toml` 删 `df`（deep_filter）依赖；
- GRU 状态跨帧保持、会话起点 `reset()` 的语义不变（与本文档 §6 一致）。

### 验证（denoise.rs 测试，无 `#[ignore]` 即可跑）

- `diag_clean_speech_preserved`：干净合成语音 gain≈**0.993x**（dfn3 是 0.10）——**不压语音，dfn3 缺陷已消除**；
- `diag_pure_noise_suppressed`：稳态白噪声抑制 ~1.8dB（RNNoise 对稳态宽带噪声保守，避免 musical noise——非缺陷）；
- `streaming_incremental_equals_batch`：分块 = 批处理逐位相同（缓冲逻辑正确）；
- `length_invariant_within_one_frame` / `diag_silence_output` / `processor_basic_roundtrip`：结构与守恒；
- 真实语音（macOS `say`）诊断 `diag_denoise_tts_wav` / `diag_real_speech_noisy_denoise_effect`（`#[ignore]`，需 `/tmp/voice48k.wav`）：gain≈1.0。

**度量警示**：评估滤波效果不可用「逐样本 SNR」——RNNoise 频带增益 + OLA 引入相位/群延迟，逐样本相减被相位偏移主导（连干净语音亦显示 ~-3dB）。应用能量保留（gain）或频谱/感知度量。

---

## 1. 背景与目标

octopus 麦克风录音链路当前**无任何噪声消除处理**（已核查：`audio.rs` 仅做多声道下混、格式转换、重采样；`filter_speech`/VAD 是切静音段而非降噪；`normalize_whisper_features` 是 mel 特征域归一化，不在波形上降噪）。环境噪声完全靠 ASR 模型自身鲁棒性硬扛。

**目标**：在语音识别前增加一层基于 ONNX 小模型（DeepFilterNet3）的环境降噪（Noise Suppression, NS），降低稳态/非稳态背景噪声对识别的干扰。

**边界声明**：NS 对「正在播放的音乐」抑制有限（音乐是有结构的信号，非稳态噪声模型会部分压制但无法彻底消除）。降环境噪声（空调/键盘/风扇/背景人声）是其强项。

## 2. 范围

### 2.1 在范围内
- DeepFilterNet3（`dfn3.onnx`）流式环境降噪
- 流式（`drain_samples` 周期取）与非流式（`stop` 整段）两条路径统一受益
- 跨平台：macOS / Windows / Linux

### 2.2 不在范围内（明确排除）
- **回声消除（AEC）**：放弃。octopus 自身不播放任何音频（已核查全仓无 `output_stream`/`playback`/`tts`/`speak`），AEC 所需的「回放参考信号」无法从应用内部获取；系统级音频回环（macOS CoreAudio Tap/ScreenCaptureKit、BlackHole 虚拟声卡）代价过大且侵入用户音频路由。背景音乐干扰由 NS 部分承担。
- **多降噪模型切换**：DF3 是**唯一固定模型**，不进 DB `models` 表，不走 `AsrEngineManager`/`resolve_active_engine`。
- **数据库配置管理**：DF3 不入数据库（与 ASR 引擎模型的管理路径完全不同）。
- **AGC/归一化/高通滤波**：不做（YAGNI）。

## 3. 模型选型

### 3.1 为什么是 `penta2himajin/deepfilternet3-onnx/dfn3.onnx`

候选模型核查（HF 缓存实测 IO 契约）：

| 来源 | 结构 | 是否含 GRU 状态入参 | 流式 |
|---|---|---|---|
| bitsydarel / tonythethompson | 3 文件（enc + erb_dec + df_dec） | 否（`S` 序列维，一次喂多帧） | ❌ 离线展开版 |
| **penta2himajin** | 单文件 `dfn3.onnx`（8.5MB） | 是（`enc_h`/`erb_h`/`df_h`，每帧 S=1） | ✅ 真正的流式有状态版 |

三文件版是展开计算图（无状态、需整段喂入），实时延迟不可接受。**`dfn3.onnx` 是唯一带 GRU 隐状态、支持逐帧实时推理的版本**，故选之。

### 3.2 IO 契约（每帧 hop=480 样本 @48kHz = 10ms）

```
入:
  spec      [1,1,1,481,2]   当前帧 STFT 复数频谱（实部+虚部），n_fft=960 → 481 bins
  feat_erb  [1,1,1,32]      32 个 ERB 频带能量（dB + EMA 均值归一化 /40）
  feat_spec [1,1,1,96,2]    前 96 bin 复数（单位归一化：除以 √EMA(|z|)）
  注：feat_erb/feat_spec 经 conv_lookahead=2 帧对齐（spec[t] 配 feat[t+2]）
  enc_h     [1,1,256]       encoder GRU 状态（初始 0）
  erb_h     [2,1,256]       erb decoder GRU 状态（2 层，初始 0）
  df_h      [2,1,256]       df decoder GRU 状态（2 层，初始 0）
出:
  enhanced_spec [1,1,1,481,2]   增强后的复数频谱（coefs/mask 已在图内应用）
  new_enc_h / new_erb_h / new_df_h   更新后的状态（下一帧入参）
```

模型直接输出增强频谱，**无需手写滤波系数应用**，后处理仅需 STFT→推理→iSTFT。

## 4. 架构

### 4.1 集成位置：采集层（`SharedAudioState` 内），coordinator 无感

`SharedAudioState` 本就承担「把麦克风原始流转成 ASR 可用的 16k 流」之责，NS 是该职责的自然延伸；且它已持有有状态资源（`AudioResampler`/`Stream`），再加一个 `DenoiseProcessor` 模式一致。VAD/ASR 拿到的仍是干净 16k，**流式/非流式两条路径统一受益，无需改 coordinator**。

### 4.2 数据流

```
cpal 回调（设备原生 SR：mac/win/linux 各异）
   │  多声道下混 → samples buffer（不变）
   ▼
drain_samples() / stop()   ← coordinator 线程调用
   │
   ├─ raw(原生SR) →[重采样 48k]→ DenoiseProcessor(48k) →[重采样 16k]→ out   （denoise_enabled）
   │                            │
   │        每 480 样本(10ms)：STFT(Vorbis 窗, n_fft=960) → feat_erb(dB+EMA归一化) / feat_spec(单位归一化)
   │                            │  dfn3.onnx(spec + 3 组 GRU 状态) → enhanced_spec
   │                            │  iSTFT + overlap-add → 480 增强样本
   │                            └─ GRU 状态跨帧保持（录音会话内）
   │
   └─ raw(原生SR) →[重采样 16k]→ out                                          （denoise 关闭，原逻辑）
```

### 4.3 采样率桥接

- DeepFilterNet 是 **48kHz**，octopus ASR 是 **16kHz**。NS 层工作在 48k 域，前后各一次重采样。
- 「重采样 48k」以 **cpal 报告的设备 SR 为准动态判断**（不写死平台假设）：`if rate==48000 { 直通 } else { 升/降到 48k }`。mac/win/linux 默认输入 SR 各异（48k/44.1k 等），统一桥接到 48k。
- STFT 参数（n_fft=960 / hop=480 / 481 bins / 32 ERB / 96 df）是**模型契约，硬绑 48kHz**——任何平台都必须先重采样到 48k 进 NS，频带映射才正确。这是跨平台一致的硬约束。
- `DenoiseProcessor` 内部维护 48k 输入缓冲 + OLA 输出缓冲（跨次 `drain_samples` 保留残帧），与现有 `AudioResampler` 的增量 + flush 模式同构。

## 5. 组件

### 5.1 新模块 `crates/asr/src/denoise.rs`

逻辑集中于此；`desktop/src/audio.rs` 只持有 `Option<DenoiseProcessor>` 并调用，保持薄。

```rust
pub struct DenoiseProcessor {
    session: ort::session::Session,       // dfn3.onnx
    // GRU 隐状态（持久，跨帧传递）
    enc_h: Array3<f32>,   // [1,1,256]
    erb_h: Array3<f32>,   // [2,1,256]
    df_h:  Array3<f32>,   // [2,1,256]
    // 流式增量状态
    in_buf:   Vec<f32>,   // 48k 输入累积，每满 480 触发一帧
    out_buf:  Vec<f32>,   // 已增强样本待 drain
    ola_prev: Vec<f32>,   // 上一帧 iSTFT（overlap-add 用）
    // DSP 常量（构造时算一次）
    window:     Vec<f32>,        // Vorbis 分析/合成窗（COLA 50% overlap）
    erb_widths:  Vec<usize>,       // 32 ERB 带宽度（对齐 libDF erb_fb）
    erb_norm_state: Vec<f32>,  // [32] feat_erb EMA 状态
    df_norm_state:  Vec<f32>,  // [96] feat_spec EMA 状态
    spec_queue:  VecDeque,      // conv_lookahead 环形缓冲
    fft: rustfft::FftPlanner<f32>,  // n_fft=960
}

impl DenoiseProcessor {
    pub fn new(model_path: &Path) -> Result<Self>;   // 加载 session + 算窗/ERB 表 + 状态归零
    pub fn process_samples(&mut self, s48k: &[f32]) -> Vec<f32>;  // 增量：in_buf 累积，逐帧 STFT→feat→run→iSTFT+OLA
    pub fn flush(&mut self) -> Vec<f32>;              // 尾部零填，吐残留（同 AudioResampler::flush 模式）
    pub fn reset(&mut self);                          // GRU + 缓冲清零
}
```

### 5.2 每帧处理流水（`process_samples` 内）

```
in_buf 凑满 480 → 取 [上帧尾 480 .. 上帧尾+960] = 960 样本
  → × window → rustfft(960) → spec[481] 复数
  → feat_erb[32]    = band 互相关功率 → 10·log10 → EMA 均值归一化 /40
  → feat_spec[96,2] = spec 前 96 bin → 单位归一化（EMA 跟踪 |z|，除以 √state）
  → conv_lookahead 队列：spec[t] 配 feat[t+2]
  → ort run(spec[t], feat_erb[t+2], feat_spec[t+2], enc_h, erb_h, df_h)
       → enhanced_spec[481,2], new_enc_h, new_erb_h, new_df_h
  → rustfft 逆变换(enhanced_spec) → × window → OLA(减上帧重叠) → 480 增强样本入 out_buf
  → in_buf 弹出 480（保留余数供下次）
```

### 5.3 STFT：复用现有 `rustfft`

`crates/asr/Cargo.toml` 已依赖 `rustfft = "6"`，**零新依赖**（不引入 realfft）。窗类型与 OLA 增益系数实施时对齐 `deepfilter-rt`（https://github.com/shimondoodkin/deepfilter-rt）参考代码（见 §13 实施前提）。

## 6. 状态管理（呼应上次 VAD 状态污染教训，但 NS 语义相反）

| | filter_vad（已修） | DenoiseProcessor |
|---|---|---|
| 状态本质 | 「当前是否语音段」——每段语义独立 | 「噪声环境稳态估计」——连续物理过程 |
| 段间 | **每段 reset**（独立语义） | **保持**（reset 会丢噪声估计，段首几帧降噪失效=温启问题） |
| 会话边界 | start 时新建实例 | `start()` 调 `reset()`（新噪声环境起点） |

- **录音会话内**：GRU 状态跨 `drain_samples` 周期、跨 VAD 分段**连续保持**（与 filter_vad 的每段 reset **故意相反**，因噪声估计不应被分段打断）。
- **会话边界**：`SharedAudioState::start()` 调 `denoise.reset()`，与现有「`start` 重置 resampler」同模式。
- `Send/Sync`：`ort::Session` 本身 `Send+Sync`，加入 `SharedAudioState` 不破坏其既有 unsafe impl 的不变量（仍只 coordinator 单线程访问）。

## 7. 跨平台

- **ort EP 矩阵已是三平台**：`Cargo.toml` 的 `ort = { features = ["download-binaries","cuda","coreml","directml"] }` 在运行时按平台自动选最优 EP（mac→CoreML、win→DirectML、linux→CUDA/CPU）。DF3 推理三平台都走最优路径。
- **cpal 跨平台采集**：CoreAudio / WASAPI / ALSA-Pulse。多声道下混已在 `audio.rs` 回调内完成（均值），NS 拿到 mono，跨平台一致。
- **验证前提**（写进实施计划）：三平台采集实测（WASAPI shared mode / ALSA 默认设备的 SR 报告与下混）；性能下限（弱 CPU：低档 win 笔记本 / linux ARM，单帧推理 <10ms 实时预算，兜底用 `ort::with_intra_threads` 可配线程数）。

## 8. 配置

- 仅加一个配置项 `denoise_enabled: bool`（默认 `true`）。
- 模型固定走 HF cache，**不**暴露为配置项（DF 只一个模型，无切换需求）。
- DF3 **不进数据库**，不参与 `models` 表 / `AsrEngineManager` / `resolve_active_engine` 体系。
- 新增 `crates/asr/src/config.rs::find_df3()`：从 `~/.cache/huggingface/hub/models--penta2himajin--deepfilternet3-onnx/snapshots/*/dfn3.onnx` 定位（glob snapshots 子目录，复刻现有 HF cache `find_*` 模式）。**缺失时错误信息固定**：
  ```
  DeepFilterNet3 模型缺失，请先下载：hf download penta2himajin/deepfilternet3-onnx
  ```
- `denoise_enabled` 字段加到 `infra::AppConfig`（配置 schema 已下沉 infra），默认 `true`（在 `Default` impl 中设置）。

## 9. 错误降级

「前处理是增强，失败降级直通，不阻断识别」——与现有 `mic missing→silent` / `DB init failed→storage disabled` 同哲学。

| 故障 | 行为 |
|---|---|
| `find_df3()` 缺失 / onnx 解析失败 | `DenoiseProcessor::new` 返回 Err → `SharedAudioState` 持 `None` → drain/stop 走原逻辑（直接 16k 重采样），日志 `warn`（含下载提示），**录音不阻断** |
| 单帧推理失败（罕见） | 该帧 bypass（输出未降噪原样本），日志 `warn`，GRU 状态保持，继续下一帧，**不 panic** |
| `denoise_enabled=false` | 不建 session，零开销直通 |

## 10. 测试策略

- **DSP 正确性**：STFT→iSTFT（不经模型）OLA 重建误差，干净信号重建 SNR > 40dB；`feat_erb` 分带能量对已知频谱的数值正确性。
- **样本守恒**：带噪 wav 经 `process_samples`→`flush`，输出总长 == 输入长（OLA 不丢不增）。
- **流式一致性**（强制项，呼应 paraformer 边界 bug）：同一信号「分 N 次增量 `process_samples`」与「一次性」输出应逐样本相等——验证无状态漂移、无边界丢帧。
- **状态语义**：连续两帧 GRU 状态更新；`reset()` 后归零。
- **跨平台 CI**：mac 默认跑（win/linux 视 CI）；模型从 HF cache 拉。
- 不强求「降噪后识别准确率提升」端到端指标（难量化），但实施后手动对比脏/净样本听感与识别结果。

## 11. 模型分发

HF cache 模式（与 ASR 引擎模型同源）。用户 `hf download penta2himajin/deepfilternet3-onnx` 下载到 `~/.cache/huggingface/hub/`，`find_df3()` 读取。零 bundle 体积，三平台一致。

## 12. 验收标准

1. `denoise_enabled=true` 时，带噪录音经处理后环境噪声听感明显降低，识别结果改善（手动验证）。
2. `denoise_enabled=false` 时，行为与现状完全一致（零回归）。
3. 模型缺失时，应用正常启动、录音正常工作（仅日志告警 + 下载提示），不崩溃。
4. 流式增量与一次性处理输出逐样本相等。
5. mac/win/linux 三平台单帧推理 <10ms。

## 13. 实施前提（已确认）

1. **STFT 窗类型**：Vorbis 窗 `sin(π/2·sin²(π(n+0.5)/N))`（对齐 libDF），50% overlap COLA 增益=1。~~初版误用 sqrt-Hann~~。
2. **ERB 尺度**：分母 228.833 = 24.7×9.265（对齐 libDF `freq2erb`）。~~初版误用 24.863，带边界错 9.2 倍~~。
3. **特征归一化**：feat_erb 需 dB + EMA 归一化 + /40；feat_spec 需单位归一化（EMA 跟踪 |z|，除以 √state）。~~初版直接传原始值~~。
4. **conv_lookahead=2**：spec[t] 配 feat[t+2]，环形缓冲 + flush 填零。~~初版缺失~~。

## 关键文件

- `crates/asr/src/denoise.rs`（新建：`DenoiseProcessor` + STFT/feat/OLA）
- `crates/asr/src/config.rs`（新增 `find_df3()`）
- `crates/desktop/src/audio.rs`（`SharedAudioState` 持 `Option<DenoiseProcessor>`，drain/stop 接入，start 调 reset）
- `crates/infra/src/config.rs`（加 `denoise_enabled` 字段，默认 true）
- `crates/infra/src/consts.rs`（可选：DF3 HF repo 名常量）

---

## `2026-06-16-model-spec-prefix-design.md`

# 模型选择 spec 设计（`PREFIX:NAME` 统一格式）

> 状态：⚠️ 已被 3-part 演进**部分取代**（2026-06-17，阿里云云端 API 接入）。
>
> **本文是 2-part `PREFIX:NAME`（Local/Category/NameOnly）的原始设计记录**，作为历史决策动机保留。引入 `provider` 维度后（区分 deepseek 直连 vs aliyun 代管同名模型），spec 已演进为 3-part `{provider}:{category}:{model_name}`，`ModelSpec` 枚举改为 `Full{provider,category,model_name}` / `NameOnly`。**当前权威设计见 [`2026-06-17-aliyun-cloud-apis-design.md`](2026-06-17-aliyun-cloud-apis-design.md) §3.3「选择规格 parse_model_spec → 3-part」**，[`architecture.md`](../../../architecture.md)「模型管理」段，以及 `infra/src/db.rs` 现实现。
>
> 本文中「裸名等价 local」「`local` 特殊前缀」的设计动机（减少本地模型配置心智负担）仍成立，3-part 沿用同一思路（`provider=local` 时仍可跨 category 命中）。

## 背景

重构前 `config.yaml.asr_engine` 和 `polish_llm` 仅按 DB `models.name` 精确匹配。但 DB schema 的唯一键是 `UNIQUE(domain, name, is_local, category)`——**不同 category 下允许同名模型**（例如 `deepseek` 和 `aliyun` 两个 category 下都有 `deepseek-v4-flash`）。旧查询仅按 name 过滤，遇到同名模型时 SQLite 返回不确定行，导致取错 provider / base_url / API Key。

此外，ASR 本地引擎（`is_local=1`）与远程 API 引擎（`is_local=0`）未来可能同名，需要一种方式在配置字符串中显式区分。

## 目标

1. **统一 `asr_engine` 和 `polish_llm` 的配置格式**为 `PREFIX:NAME`，从 DB `models` 表唯一定位模型。
2. **`local` 作为特殊前缀**——映射 `is_local=true`，不对应 DB `category` 列值。
3. **其他前缀按 DB `category` 精确匹配**（如 `bigmodel`、`deepseek`、`aliyun`）。
4. **裸名默认走 local 语义**——不含冒号的裸名等价于 `local:NAME`，筛 `is_local=true`。这样设计是因为绝大多数场景使用本地模型，裸名即足；远程模型必须用 category 前缀显式指定。
5. **ASR 与 LLM 统一语义**——同一套 `parse_model_spec` 规则服务两个 domain。

## 设计决策

### ModelSpec 枚举（`infra/src/db.rs`）

> ⚠️ 下方为**原始 2-part 设计**。演进后为 `Full{provider,category,model_name}` / `NameOnly`，详见 [aliyun spec §3.3](2026-06-17-aliyun-cloud-apis-design.md)。

原始（已被取代）：
```rust
pub enum ModelSpec<'a> {
    Local(&'a str),          // "local:NAME" → is_local=true AND name
    Category(&'a str, &'a str), // "CATEGORY:NAME" → category AND name
    NameOnly(&'a str),       // "NAME" → 等价 Local，筛 is_local=true AND name
}
```

演进后（现实现）：
```rust
pub enum ModelSpec<'a> {
    Full { provider: &'a str, category: &'a str, model_name: &'a str }, // "p:c:m"
    NameOnly(&'a str),   // 裸名：仅全局默认 fallback 用（跨 provider/category 搜，优先 local）
}
```

`parse_model_spec` 现按冒号数：2 冒号 → `Full`，0 冒号 → `NameOnly`，1 冒号（旧 2-part）→ warn + 按 `NameOnly` 兜底。`ModelSpec::model_name()` 返回裸名。

### 裸名等价 local

裸名（无冒号）的语义是**筛本地模型**（`is_local=true`），而非遍历所有 section。这意味着：
- `"zipformer-small-ctc"` 等价于 `"local:zipformer-small-ctc"`。
- 远程模型必须用 category 前缀显式指定（如 `"aliyun:deepseek-v4-flash"`）。
- 这一设计减少了配置心智负担：绝大多数场景使用本地模型，裸名即足；仅在远程 / 多 category 同名时才需前缀。

### 为什么 `local` 是特殊前缀

ASR 引擎的 category（`whisper` / `sensevoice` / `paraformer` / `qwen3-asr` / `zipformer`）是引擎**类型**分类，而 `is_local` 是**部署位置**标记。`local:zipformer-small-ctc` 比 `zipformer:zipformer-small-ctc` 更贴近用户心智（「我要本地的那个 zipformer」），且 `local` 前缀可跨 category 复用（任何 `is_local=1` 的模型都能用 `local:NAME` 命中）。

远程模型（如未来 `aliyun:zipformer-small-ctc`）直接用 category 前缀精确匹配。

### LLM 查询（`load_llm_model_at`）

按 `ModelSpec` 两分支构建不同 SQL（`Local` 和 `NameOnly` 共用同一查询）：

| spec | SQL WHERE 子句 |
|------|---------------|
| `Local(name)` / `NameOnly(name)` | `domain='llm' AND is_local=1 AND name=?` |
| `Category(cat, name)` | `domain='llm' AND category=? AND name=?` |

### ASR 引擎解析（`asr::config`）

- `engine_category_from_str(s)` — DB `category` 字符串 → `EngineCategory` 枚举映射（5 个 ASR 类型；远程 category 如 `aliyun` 返回 `None`）。
- `resolve_engine_in_config(cfg, spec)` — 统一解析入口：
  - `Local` / `NameOnly` → 遍历 5 个 section，找 `is_local=true AND name` 的条目
  - `Category` → `engine_category_from_str` 映射后 `pick_entry`
- `resolve_engine_category(spec)` / `resolve_active_engine(spec)` 内部调用 `resolve_engine_in_config`。

### 裸名传播

下游组件（引擎缓存、流式构造器、transcribe 函数）都按**裸名**工作，不感知前缀：
- `AsrEngineManager.switch_model(spec)` — 解析 spec → 裸名做缓存键
- `StreamingSession::new(spec)` — 解析 spec → 裸名传给 `StreamingParaformer::new` / `StreamingZipformer::new`
- CLI `do_transcribe` / `run_e2e` / `stream_test` — 剥离前缀后传给各引擎 `transcribe` 函数

`ResolvedEngine.name` 始终是裸名（去掉前缀），保证缓存命中率。

## 接口

### 公开 API（`infra::db`）

```rust
pub enum ModelSpec<'a> { Local(&'a str), Category(&'a str, &'a str), NameOnly(&'a str) }

pub fn parse_model_spec(spec: &str) -> ModelSpec<'_>;
impl<'a> ModelSpec<'a> { pub fn name(&self) -> &'a str; }

pub fn load_llm_model(spec: &str) -> Result<Option<CompatibleLlmConfig>>;
```

### 公开 API（`asr::config`）

```rust
pub fn resolve_engine_in_config<'a, 'b>(cfg: &'a AsrConfig, spec: &'b str)
    -> Option<(EngineCategory, &'b str, &'a ModelEntry)>;
pub fn resolve_engine_category(spec: &str) -> Option<EngineCategory>;
pub fn resolve_active_engine(asr_engine: &str) -> Result<ResolvedEngine>;
```

## 配置示例

> ⚠️ 本节示例为原始 2-part 形式，**已演进为 3-part**。当前正确写法见 [`configuration.md`](../../configuration.md)「模型选择 spec」节与 [aliyun spec §3.3](2026-06-17-aliyun-cloud-apis-design.md)。

演进后（现实现）：
```yaml
# ASR 引擎
asr_engine: "local:zipformer:zipformer-small-ctc"   # 本地模型（provider=local）
# asr_engine: "aliyun:asr:fun-asr-..."              # 阿里云云端 ASR（provider=aliyun）

# LLM 润色
polish_llm: "bigmodel:glm:glm-4-flashx"             # provider:category:model_name
# polish_llm: "aliyun:qwen:qwen-plus"               # 阿里云代管
```

## 关键约束

- **裸名等价 local**：裸名格式（无冒号）筛 `is_local=true`，远程模型必须用 category 前缀显式指定。
- **`local` 前缀跨 category**：`local:NAME` 遍历所有 section，若多个 category 下有同名且 `is_local=true` 的模型，返回第一个匹配（按 whisper→sensevoice→paraformer→qwen3-asr→zipformer 顺序）。建议避免此情况。
- **Category 前缀仅限 ASR 已知类型**：~~`aliyun:zipformer-small-ctc` 中 `aliyun` 不是已知 ASR 引擎 category → `resolve_engine_in_config` 返回 `None`。远程 ASR 路由（如阿里云远程 ASR）尚未实现，当前所有 ASR 均为本地。~~ **（已被取代）** 阿里云云端 ASR 已在 2026-06-17 接入——`provider=aliyun` 经 `resolve_category` 的 provider 分支路由到 `EngineCategory::Aliyun` + `DashscopeEngine`（`is_streaming=0` 块路径），不走 `engine_category_from_str`。详见 [aliyun spec §5.2](2026-06-17-aliyun-cloud-apis-design.md)。
- **`OnceLock` 缓存不变**：手编 DB `models` 表后仍需重启进程生效。

## 影响范围

| 模块 | 变更 |
|------|------|
| `infra/src/db.rs` | 新增 `ModelSpec` + `parse_model_spec`；`load_llm_model_at` 按 spec 两分支查询（`Local`/`NameOnly` 合并） |
| `asr/src/config.rs` | 新增 `engine_category_from_str` / `all_sections` / `resolve_engine_in_config`；`resolve_engine_category` / `resolve_active_engine` 改走 spec 解析 |
| `asr/src/engine.rs` | `switch_model` 解析 spec → 裸名缓存 |
| `asr/src/streaming_engine.rs` | `StreamingSession::new` 解析 spec → 裸名传构造器 |
| `cli/src/main.rs` | `do_transcribe` / `run_e2e` / `stream_test` 剥离前缀 |
| `infra/src/config.rs` | `polish_llm` 默认值 `glm-4-flashx` → `bigmodel:glm-4-flashx` |
| `docs/configuration.md` | 新增「模型选择 spec」节 + 表格行更新 |

---
