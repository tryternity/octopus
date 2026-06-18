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
