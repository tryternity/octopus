# 设置页「模型选择」Card 设计

> 日期：2026-06-28
> 状态：✅ 实施完成（ASR引擎/LLM模型/OCR模型选择 + 热重载预热 + 前端 select 下拉）
> 关联：[model-mgmt-ui GUI 模型管理页](./)（侧栏「模型管理」Tab，下载/校验）、[ocr 引擎](../../../crates/ocr/src/engine.rs)（`OcrEngine::instance` OnceLock 单例）

## 1. 背景 / 动机

系统设置页（`GeneralPanel.tsx`）当前有 5 个 Card：交互 / 快捷键 / 语音识别 / 语音识别润色 / 剪贴板。其中「模型选择」分散在两个 Card：

- **语音识别模型**（`asr_engine`）嵌在「语音识别」Card（与语言/硬件加速/纠错/简繁/停顿混在一起）。
- **润色模型**（`polish_llm`）嵌在「语音识别润色」Card（与模式/提示词/间隔/停顿阈值混在一起）。
- **OCR 模型**（`ocr_model`）**完全不在设置页**——它走 `ocr/engine.rs::OcrEngine::instance()` 里的 `load_config_key("ocr_model")` 旁路读取，用户无法在 GUI 切换。

更隐蔽的问题：`ocr_model` 在 `app_config` 表**已有 seed**（`db.sql:286`，值 `PP-OCRv6-small`），但 `AppConfig` 结构体（`config.rs`）**漏了该字段**。所以它不参与 `load_app_config_at` / `save_app_config_at` 的统一读写，无法通过 `set_config` 持久化——这是历史遗漏。

用户要求：在「交互」Card 正下方新增独立的「模型选择」Card，把语音识别模型、润色模型、OCR 模型**集中**在一起。

## 2. 设计目标

- **集中**：三类模型选择（ASR / 润色 / OCR）归拢到单一「模型选择」Card，从原所在 Card 移走（不重复）。
- **补漏**：`ocr_model` 纳入 `AppConfig`，走统一的 DB load/save + `set_config` 持久化链路，与 `asr_engine` / `polish_llm` 对齐。
- **OCR 可选**：OCR 下拉选项从 `models` 表 `domain='ocr'` 查（对齐 asr/llm 的 DB 查询模式），当前仅 1 项（`PP-OCRv6-small`），未来加模型不改 UI。
- **生效语义清晰**：每行标注真实生效时机（asr 下次录音 / polish 立即 / ocr 下次启动）。

## 3. 设计

### 3.1 UI：新增「模型选择」Card（`GeneralPanel.tsx`）

Card 顺序（新 Card 紧跟「交互」之后，符合"在交互下面"）：

1. 交互（Mic，不变）
2. **模型选择（新，`Layers` 图标）**
3. 快捷键（Keyboard）
4. 语音识别（Volume2）—— **删「识别引擎」行**
5. 语音识别润色（Sparkles）—— **删「润色模型」行**
6. 剪贴板（ClipboardList）

新「模型选择」Card 三行（复用现有 `Row` + `select` + `selectClass`）：

| 行 label | config key | 选项来源 | effect |
|---|---|---|---|
| 语音识别模型 | `asr_engine` | `asr_engines`（已有） | 下次录音 |
| 润色模型 | `polish_llm` | `llm_models`（已有） | 立即 |
| OCR 模型 | `ocr_model` | `ocr_models`（**新增**） | 下次启动 |

- ASR / 润色行的下拉逻辑**原样搬运**（`asr_engines`/`llm_models` 的 `value`/`onChange` 不变），仅换所属 Card。
- OCR 行：`<select value={cfg.ocr_model}>` 选项来自 `ocr_models`（`m.name` / `m.label`），`onChange` 调 `setVal("ocr_model", e.target.value)`。
- 新 Card 用 `Layers` 图标，区别于侧栏「模型管理」Tab 已用的 `Box`。

### 3.2 `ocr_model` 纳入 AppConfig（补漏，核心）

`ocr_model` 当前是 `load_config_key("ocr_model")` 旁路读，不进 `AppConfig`。补齐使其走统一链路：

- **`config.rs`**：加字段 `pub ocr_model: String`（`#[serde(default = "default_ocr_model")]`，默认 `"PP-OCRv6-small"`）+ `fn default_ocr_model()` + `Default` impl 初始化 + 单测断言 `cfg.ocr_model == "PP-OCRv6-small"`。
  - 字段位置：紧邻 `asr_engine`（同为"模型选择"语义）或字段区末尾均可；放末尾减少对既有字段顺序的扰动。
- **`db.rs::load_app_config_at`**：字符串区分支加 `"ocr_model" => cfg.ocr_model = value,`。
- **`db.rs::save_app_config_at`**：`fields` 数组加 `("ocr_model", cfg.ocr_model.clone())`，长度 `27 → 28`；注释 `27 字段` → `28 字段`。
- **`db.sql`**：`app_config` seed 行（L286 `('ocr_model', 'PP-OCRv6-small', ...)`）**已存在，不动**；新装用户有 seed，老库缺行时 serde default 兜底。

补齐后，`ocr/engine.rs::OcrEngine::instance()` 仍用 `load_config_key("ocr_model")` 读取（行为不变）——本设计**不改 OCR 引擎读取入口**，只补 AppConfig 持久化链路，让设置页能写。

### 3.3 OCR 下拉数据源（新增）

对齐 `list_llm_models` 模式：

- **`db.rs`**：新增 `OcrModelInfo { model_name, description }`（`#[derive(Debug, Clone, serde::Serialize)]`）+ `list_ocr_models_at(conn)`（`SELECT model_name, description FROM models WHERE domain='ocr' AND is_enabled=1`）+ `pub fn list_ocr_models()`（经 `with_db`）。
- **`runtime_config.rs`**：新增 `OcrOption { name, label, current }`（`#[derive(Serialize)]`）+ `build_ocr_options(current, ocrs)` + `pub fn build_ocr_options_public(current, ocrs)`。
  - **不做「不选择模型」首项**（区别于 `build_llm_options`）：OCR 必须有一个模型，空值无意义。列表即 DB 启用的 OCR 模型，`current` 按 `m.model_name == current` 标记。
  - `label`：优先 `description`（如 "PP-OCRv6 small (det 4.7M + rec 10M + keys 73K)，中/英/繁体/日"），description 空时回退 `model_name`。
- **`settings_commands.rs`**：`ConfigResponse` 加 `pub ocr_models: Vec<crate::runtime_config::OcrOption>`；`get_config` 加 `let ocrs = octopus_infra::db::list_ocr_models()...; let ocr_models = build_ocr_options_public(&g.ocr_model, ocrs);` 并填入返回。
- **前端 `ConfigResponse`**（`Settings/index.tsx`）：接口加 `ocr_models: { name: string; label: string; current: boolean }[]`。

### 3.4 OCR 生效时机 = 下次启动（有意取舍）

`ocr/engine.rs::OcrEngine::instance()` 用 `OnceLock` 缓存，首次加载后整个进程固定。改 `ocr_model` 写入 DB 后，**当前会话不热替换**——原因：

- OCR 引擎实例化需反序列化 3 个 `.mnn` 文件 + 建 session，成本高。
- OCR 使用频率远低于 ASR（截图识别才触发），不值得为热切换加 `OnceLock` 清空 + 重载逻辑。
- 重启后 `instance()` 重新读 DB，自然生效。

故 OCR 行 effect 标「下次启动」。`set_config` 处理 `ocr_model` 时**只持久化、不热重载**（对比 `asr_shortcut` 等的热重载块，OCR 无对应块）。

### 3.5 `apply_config_value` 加 `ocr_model` 分支

`ocr_model` 是裸 `model_name`（非 3-part spec，OCR 引擎直接拿来当目录名），简单字符串校验即可（照 `asr_shortcut` 字符串分支模板）：

```rust
"ocr_model" => {
    cfg.ocr_model = value.as_str().ok_or("ocr_model 需要字符串")?.to_string();
}
```

**不**调 `build_*_spec` 构造（OCR 无 spec 解析），**不**校验模型是否在 DB（当前仅 1 个，且未启用时报错反而碍事；持久化即可，`instance()` 加载时 `is_model_ready` 兜底）。

## 4. 边界与权衡

| 场景 | 行为 |
|------|------|
| 切 OCR 模型（当前仅 PP-OCRv6-small） | 写 DB 持久化；当前会话 OCR 仍用旧实例（OnceLock）；重启后生效 |
| OCR 模型未下载（`is_model_ready=false`） | 下拉仍显示（DB `is_enabled=1` 即列）；选了重启后 `instance()` bail「OCR 模型未就绪」（既有兜底，不在本设计改） |
| `ocr_models` 为空（DB 无 domain='ocr' 启用行） | 下拉空，`cfg.ocr_model` 保持当前值；不崩（前端 map 空数组） |
| 老库 `app_config` 无 `ocr_model` 行 | `load_app_config_at` 无匹配分支走 serde default `PP-OCRv6-small`；首次 `set_config` 触发 `save_app_config_at` 写入该行 |
| ASR/润色行从原 Card 移走 | 原「语音识别」Card 剩语言/硬件加速/纠错/简繁/停顿；原「语音识别润色」Card 剩模式/提示词/间隔/停顿阈值——职责更清晰 |

## 5. 不改动 / 持久化

- **不改** `ocr/engine.rs::OcrEngine::instance()` 读取入口（仍 `load_config_key("ocr_model")`）、`OcrEngine::recognize`、OCR 单例缓存机制。
- **不改** `asr_engine` / `polish_llm` 的后端逻辑（仅前端换 Card 归属）。
- **不改** 侧栏「模型管理」Tab（`ModelsPanel.tsx`，下载/校验 ASR 模型）——本设计是设置页内 Card 重组，不涉及模型下载。
- **不改** `db.sql`（OCR 的 models seed L101-107 + app_config seed L286 均已存在）。

### 5.1 DB 持久化（隐藏前提，同 polish_global_shortcut）

`load_app_config_at` / `save_app_config_at` 是显式字段列表，每加一个 `AppConfig` 字段必须同步补行（漏则 `set_config` 不写 DB + `get_config` load 不到 → 设置页回退 serde default）。`ocr_model` 必须在两处补（见 §3.2）。

`load_app_config_at` 用 `WHERE category='setting'` 过滤。`db.sql` 的 `ocr_model` seed 行 `category` 走列 DEFAULT（当前 `='setting'`），新装/已迁移库正确。老库若列 DEFAULT 仍是 `'default'`，该行 load 漏读 → 回退 serde default（功能可用，仅显示值非 DB 值）；既有 migration（`UPDATE ... 'default'→'setting'`）已覆盖。
