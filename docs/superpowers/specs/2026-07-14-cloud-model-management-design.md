# 云端模型新增/编辑/删除 设计

> 2026-07-14 · 云端模型（LLM 润色 + ASR）用户自管理

## 1. 背景

现有云端模型（`domain='asr' AND is_local=0`、`domain='llm'`）全部在 DB seed 中预定义。用户无法自行添加新 provider/模型、无法编辑 api_key 之外的配置、无法删除不需要的模型。

本设计：移除所有云端模型 seed，改为用户自行添加。通过 `app_config` 表存储各 provider 的参考模型列表，帮助用户正确配置。

## 2. 设计概要

### 2.1 移除 seed 云端模型

DB v31 迁移：
```sql
DELETE FROM models WHERE domain='asr' AND is_local=0;
DELETE FROM models WHERE domain='llm';
```

db.sql 中删除所有云端 ASR INSERT 和 LLM INSERT（只保留 local 模型 seed）。

### 2.2 参考模型列表存 app_config

`category='asr_cloud_model'`，`config_key='{provider}:{category}'`，`config_value` = 分号分隔的参考模型名列表：

```sql
INSERT OR IGNORE INTO app_config (config_key, config_value, description, category) VALUES
('aliyun:Fun-ASR', 'fun-asr-realtime;fun-asr-realtime-2026-02-28;fun-asr-realtime-2025-11-07;fun-asr-flash-8k-realtime;fun-asr-flash-8k-realtime-2026-01-28', '阿里云 FunASR 实时模型列表', 'asr_cloud_model'),
('aliyun:Paraformer', 'paraformer-realtime-v1;paraformer-realtime-v2;paraformer-realtime-8k-v1;paraformer-realtime-8k-v2', '阿里云 Paraformer 实时模型列表', 'asr_cloud_model'),
('aliyun:Qwen-ASR', 'qwen3-asr-flash-realtime;qwen3-asr-flash-realtime-2026-02-10;qwen3-asr-flash-realtime-2025-10-27', '阿里云 Qwen3-ASR Realtime 模型列表', 'asr_cloud_model'),
('bytedance:Doubao-ASR', 'doubao-asr-1.0-streaming', '火山引擎豆包 ASR 1.0（bigmodel_async，Resource ID=volc.bigasr.sauc.duration）', 'asr_cloud_model'),
('bytedance:Doubao-ASR-2.0', 'doubao-asr-2.0-streaming;seedasr-2.0-streaming', '火山引擎豆包 ASR 2.0（bigmodel_async，Resource ID=volc.seedasr.sauc.duration）', 'asr_cloud_model'),
('tencent:Tencent-ASR', '16k_zh;16k_zh_large;16k_zh-PY;16k_zh-TW;16k_yue;16k_zh_dialect;16k_wuu-SH', '腾讯云实时语音识别中文引擎列表', 'asr_cloud_model'),
('tencent:Tencent-ASR-Multi', '16k_zh_en;16k_multi_lang;16k_en;16k_en_large', '腾讯云实时语音识别多语种引擎列表', 'asr_cloud_model'),
('baidu:Baidu-ASR', '15372;15376;1537', '百度智能云实时语音识别中文模型（dev_pid）', 'asr_cloud_model'),
('baidu:Baidu-ASR-EN', '17372;1737', '百度智能云实时语音识别英文模型（dev_pid）', 'asr_cloud_model');
```

### 2.3 各 provider 固定配置

每个 ASR provider 有固定的 source 端点和 source 字段语义，用户选 provider + category 后自动填入，不可改：

| provider | category | source（自动填） | secret_key 语义 | model_name 用途 |
|----------|----------|-----------------|----------------|----------------|
| aliyun | Fun-ASR / Paraformer | `wss://dashscope.aliyuncs.com/api-ws/v1/inference` | DashScope API Key | model 参数 |
| aliyun | Qwen-ASR | `wss://dashscope.aliyuncs.com/api-ws/v1/realtime` | DashScope API Key | model 参数 |
| bytedance | Doubao-ASR | `volc.bigasr.sauc.duration` | X-Api-Key | 不用（硬编码 bigmodel） |
| bytedance | Doubao-ASR-2.0 | `volc.seedasr.sauc.duration` | X-Api-Key | 不用 |
| tencent | Tencent-ASR / Tencent-ASR-Multi | `{appid}:{secretid}` | SecretKey（签名密钥） | engine_model_type |
| baidu | Baidu-ASR / Baidu-ASR-EN | `{appid}` | API Key（appkey） | dev_pid |

### 2.4 LLM provider 预设

`category='llm_provider'`，存 base_url 模板：

```sql
INSERT OR IGNORE INTO app_config (config_key, config_value, description, category) VALUES
('deepseek', 'https://api.deepseek.com/', 'DeepSeek API', 'llm_provider'),
('aliyun', 'https://dashscope.aliyuncs.com/compatible-mode/v1', '阿里云 DashScope OpenAI 兼容端点', 'llm_provider'),
('bigmodel', 'https://open.bigmodel.cn/api/paas/v4', '智谱 BigModel API', 'llm_provider'),
('openai', 'https://api.openai.com/v1', 'OpenAI API', 'llm_provider'),
('ollama', 'http://localhost:11434/v1', 'Ollama 本地 API', 'llm_provider');
```

## 3. 用户操作流程

### 3.1 LLM 模型新增

1. 点「添加模型」按钮
2. 表单：
   - provider：下拉（deepseek / aliyun / bigmodel / openai / ollama / 自定义）
   - 选预设 → base_url 自动填（从 `llm_provider` 配置读）
   - model_name：手填（如 `deepseek-chat`、`gpt-4o`）
   - api_key：密码框
   - is_stream：默认 true
   - is_thinking：默认 false
3. 保存：INSERT 到 `models` 表

### 3.2 ASR 模型新增

1. 点「添加模型」按钮
2. 表单：
   - provider：下拉（aliyun / bytedance / tencent / baidu）
   - category：下拉（选 provider 后过滤——aliyun 3 种 / bytedance 2 种 / tencent 2 种 / baidu 2 种）
   - source：自动填（provider+category 固定端点），不可改
   - model_name：下拉（从 `asr_cloud_model` 配置读参考列表）+ 自由输入
   - api_key：密码框（label 按 provider 变化：API Key / SecretKey / AppKey）
3. 保存：INSERT 到 `models` 表

### 3.3 编辑

全部字段可编辑（provider / category / source / model_name / api_key / is_stream / is_thinking）。

### 3.4 删除

从 `models` 表物理删除。

## 4. DB 操作

新增函数：

```rust
/// 新增云端模型。
pub fn insert_cloud_model(
    domain: &str, provider: &str, category: &str,
    model_name: &str, source: &str, secret_key: &str,
    is_streaming: bool, is_thinking: bool,
) -> Result<i64>

/// 更新云端模型（按 id）。
pub fn update_cloud_model(
    id: i64, provider: &str, category: &str,
    model_name: &str, source: &str, secret_key: &str,
    is_streaming: bool, is_thinking: bool,
) -> Result<()>

/// 删除云端模型（物理删除）。
pub fn delete_cloud_model(id: i64) -> Result<()>

/// 读取 app_config 中 category='asr_cloud_model' 的全部参考模型列表。
pub fn list_asr_cloud_presets() -> Result<Vec<(String, String, String)>>
// 返回 (provider, category, "model1;model2;model3")

/// 读取 app_config 中 category='llm_provider' 的预设 base_url。
pub fn list_llm_provider_presets() -> Result<Vec<(String, String)>>
// 返回 (provider, base_url)
```

## 5. 各 provider 适配状态

现有代码已适配的接口协议：

| provider | 协议 | 端点切换 | model_name 影响 |
|----------|------|:--------:|----------------|
| aliyun | run-task + OpenAI Realtime | source 切端点 | 传给 API 作 model 参数 |
| bytedance | bigmodel_async | source 切 Resource ID | 不用（硬编码） |
| tencent | HMAC-SHA1 签名 WS | source 固定 | engine_model_type 参数 |
| baidu | START/STOP JSON 帧 | source 固定 | dev_pid 参数 |

**结论：现有代码不需要额外适配。** 用户选对 source + model_name 即可。

## 6. 前端

### 6.1 LLM Tab（LlmTab.tsx）

云端 section 顶部加「+ 添加模型」按钮。点击弹出表单弹窗（或 inline 表单）：
- provider select → base_url 自动填
- model_name input
- api_key password input
- is_stream / is_thinking checkbox
- 保存 / 取消

每个云端模型行加「编辑」「删除」按钮（ModelRow 扩展，或独立 CloudModelRow）。

### 6.2 ASR Tab（AsrTab.tsx）

云端 section 顶部加「+ 添加模型」按钮。点击弹出表单：
- provider select → category select（按 provider 过滤）
- source 自动显示（只读）
- model_name datalist（参考项 + 自由输入）
- api_key input（label 按 provider 变化）
- 保存 / 取消

### 6.3 组件复用

新增 `CloudModelForm.tsx` 组件（Modal 弹窗），含 provider/category/model_name/api_key/source 字段，按 domain（asr/llm）渲染不同字段组合。

## 7. Tauri 命令

```rust
#[tauri::command]
pub async fn add_cloud_model(input: CloudModelInput) -> Result<i64, String>

#[tauri::command]
pub async fn edit_cloud_model(id: i64, input: CloudModelInput) -> Result<(), String>

#[tauri::command]
pub fn remove_cloud_model(id: i64) -> Result<(), String>

#[tauri::command]
pub fn list_asr_cloud_presets() -> Result<Vec<AsrCloudPreset>, String>

#[tauri::command]
pub fn list_llm_provider_presets() -> Result<Vec<LlmProviderPreset>, String>
```

## 8. 不变量

1. **本地模型 seed 不变**：只删云端模型 seed，本地 ASR/翻译/OCR seed 保持不动
2. **app_config 参考列表可远程更新**：以后可通过更新 app_config 表远程推送新模型参考列表
3. **删除后如果当前激活的模型被删**：⚠️ 实施时简化为直接删除不回退（见 §10.7）；原计划「回退到兜底引擎（ASR）或空（LLM）」未实施
4. **source 字段语义不变**：ASR source = 端点/Resource ID/appid，LLM source = base_url

## 9. 降级

- app_config 中无参考列表 → model_name 变纯手填输入（无参考项）
- 用户填错 source/model_name → 连接失败时显示明确错误（已有错误处理）

## 10. 实施偏差与补充（2026-07-14 实施）

### 10.1 Tauri 命令名称

spec 中写的 `insert_cloud_model` / `update_cloud_model` / `delete_cloud_model`，实际实现为 `add_cloud_model` / `edit_cloud_model` / `remove_cloud_model`（前端友好的 camelCase 命名）。

### 10.2 新增命令

实施时新增了 spec 未设计的命令：
- `test_cloud_model(source, secret_key)` — GET `{base_url}/models` 验证 LLM 连接（ASR 不适用，source 非 HTTP）
- `get_model_detail(id)` — 返回真实 source + secret_key（未脱敏），用于编辑表单回填 + 连接测试（脱敏值不会发到 API）
- `get_model_id(domain, model_name, provider)` — DB 查模型 id（infra 层）
- `get_model_source_key(id)` — DB 查 source + secret_key（infra 层）

### 10.3 EngineOption / LlmOption 补全字段

两个 DTO 新增 `id` / `source` / `secret_key`（脱敏）/ `is_streaming` / `is_thinking` 字段：
- `id`：DB 行 id，用于前端编辑/删除
- `source`：base_url 或端点，编辑时回填
- `secret_key`：脱敏显示（`mask_key`：前4 + ******** + 后4），编辑保存时空值不覆盖原 key
- `is_streaming` / `is_thinking`：编辑时回填 checkbox

### 10.4 LlmModelInfo 扩展

DB `LlmModelInfo` 从 4 字段扩展到 9 字段（加 `id` / `source` / `secret_key` / `is_streaming` / `is_thinking`），`list_llm_models_at` SELECT 列同步扩展。

### 10.5 API Key 脱敏策略

`mask_key`：长度 <= 8 全掩码 `********`；否则 `前4********后4`。
- 编辑表单显示脱敏值（用户可见但不泄露）
- 保存时检测脱敏值（含 `********`）→ 传空 → 后端 `update_cloud_model` 空值不覆盖
- 测试连接时检测脱敏值 → 调 `get_model_detail` 取真实 key → 用真实 key 测试

### 10.6 translate_status 修复

原 `translate_status` 对 `local:NAME` spec 返回第一个 downloaded 模型（可能不是用户选的）。修复为从 spec 提取 model_name 精确匹配。

### 10.7 删除回退简化

spec §8.3 写了删除当前激活模型时回退兜底引擎的逻辑。实施时简化：`remove_cloud_model` 直接物理删除，不做回退检查（前端 `confirm()` 二次确认作为保护）。后续如需可补。

### 10.8 LLM provider 预设改为 JSON 格式

spec 原设计 `llm_provider` 的 `config_value` 为纯 base_url 字符串。实施时改为 JSON：

```json
{"base_url":"https://api.deepseek.com/","models":["deepseek-chat","deepseek-v4-flash"]}
```

`models` 数组提供参考模型列表（前端 datalist 下拉），允许为空（如 Ollama 全手填）。`list_llm_provider_presets` 解析 JSON 返回 `LlmProviderPresetRow { provider, base_url, models }`。

### 10.9 test_cloud_model 验证模型可用性

spec 原设计只验证 base_url + api_key 连通性（GET `/models`）。实施时增强：有 `model_name` 时改为 POST `/chat/completions`（`max_tokens: 1`），验证模型真实可用。错误时提取 API 返回的 `error.message`。无 `model_name` 时回退到 GET `/models`。

### 10.10 审查修复

- `remove_cloud_model` 简化为同步 fn（删死代码 M2）
- `set_model_enabled_at` / `set_model_secret_key_at` WHERE 扩展到 `domain IN ('asr','translate','ocr')`（M3/N4）
- `build_asr_options` 批量查询替代 N+1（M1），`is_streaming`/`is_thinking` 从 DB 取真值（I2）
- `get_model_detail` `is_streaming`/`is_thinking` 从 DB 取真值（N3）
- `CloudModelForm.handleSave` 错误提示改用 `setTestResult`（N1），空字段校验显示提示（M8）
- 删除确认改用 `@tauri-apps/plugin-dialog`（WKWebView 不支持原生 `confirm()`）

### 10.11 LLM 保存事务性测试（最终设计）

LLM 模型保存是事务性操作——后端在写 DB 前先测试连接，测试通过才入库：

- `add_cloud_model` / `edit_cloud_model` 改为 `async`，先调 `test_llm_connection`（POST `/chat/completions` + thinking disable 参数）
- 测试失败 → 返回 `Err` → 模型不入库，前端显示错误
- 测试通过 → 写入 DB（`is_enabled=1`）
- 编辑时 secret_key 为空（脱敏未改）→ 从 DB 取真实 key 测试
- `test_llm_connection` 抽为内部共享函数，`test_cloud_model`（前端手动按钮）和 add/edit（保存时）共用

前端 `handleSave` 简化：只调 `add/edit_cloud_model`，不再做前端预测试。错误信息通过 catch 显示。

### 10.12 is_thinking 默认值

`CloudModelForm` 中 `is_thinking` 默认勾选（`true`）。大多数 flash 模型（deepseek-v4-flash、glm-4.5-flash）是思考模型，默认开更安全。非思考模型（glm-4-flashx、qwen-plus）用户取消勾选即可——取消后测试也能通过（不需要关 thinking）。

### 10.13 润色失败前端提示

后端在所有润色失败路径（`PolishDone` Err / 空 content / `FinalPolishDone` Err）emit `polish-error` 事件（含错误信息）。前端结果窗口监听后显示红色气泡「⚠ 润色失败：xxx」，`POLISH_ERROR_TIMEOUT_MS = 2500` 后自动消失。用户不再面对静默失败。

### 10.14 防御性 is_local 守卫

`delete_cloud_model` / `update_cloud_model` 的 WHERE 子句加 `AND is_local=0`，防止前端传错 id 时误删/误改本地模型 DB 记录。与本地模型写操作的 `AND is_local=1` 对称。

### 10.15 编辑模式 provider 切换更新 base_url

CloudModelForm 编辑模式下切换 LLM provider 时自动更新 base_url（不再因 `!editModel` 跳过）。用户切换 provider 后 base_url 始终与预设同步，避免请求发到错误端点。
